//! 元件库（CATA）按需解析 — 移植自 `../../plant-model-gen` 的 refno 级引用闭包。
//!
//! 设计见 `docs/adr/ADR-004-on-demand-cata-parsing-port.md`、
//! `docs/plans/on-demand-cata-parsing-port.md`。
//!
//! 已落地：
//! - **Phase 1**：ref0→dbnum 定位器（就地内存实现，Q2=B），复用 `dbnum_watermark`
//!   + 文件 `ref0` 扫描（磁盘指纹缓存），不引 sqlite。
//! - **Phase 2**：`parse_db_refnos`（by-refno 部分解析）+ `CataClosureResolver`
//!   （refno 级 BFS 引用闭包：全出向 `RefU64` + owner 链 + 容器子树，`db_type` 收口 CATA）
//!   + `run_cata_closure_pass_for_refnos`（给定生成根→其 CATA 闭包）。
//!
//! Phase 3（`ensure_cata_refnos_parsed` 惰性兜底落 `pe`/`ATT_*`）与生成期接入见计划文档。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

use aios_core::SUL_DB;
use aios_core::db::DbBasicData;
use aios_core::helper::normalize_sql_string;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64, RefnoEnum, get_db_option};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as TokioMutex;

use crate::data_interface::dbnum_state::WATERMARK_TABLE;

/// B+树索引起始标记 / 无效 ref0（需跳过）。
const INVALID_REF0_SENTINEL: u32 = 0x8000_0001;

#[inline]
fn is_valid_ref0(ref0: u32) -> bool {
    ref0 != 0 && ref0 != INVALID_REF0_SENTINEL
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 1：ref0→dbnum 定位器（Q2=B）
// ─────────────────────────────────────────────────────────────────────────────

/// 把一个引用（`RefU64`）定位到它所属的 db —— 闭包引擎唯一的外部依赖。
///
/// 抽象成 trait 是为了让闭包引擎不强依赖具体存储（surreal / 文件扫描 / 将来的 sqlite），
/// 便于替换实现与单测。语义与参考项目 `data_interface::cata_closure::CataDbLocator` 对齐。
pub trait CataDbLocator {
    /// `ref0`（`RefU64::get_0()`）-> 所属 dbnum。
    fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32>;
    /// dbnum -> db_type（如 "CATA" / "DESI"）。
    fn db_type_of(&self, dbnum: u32) -> Option<String>;
    /// dbnum -> (project, db 文件路径)。
    fn file_of(&self, dbnum: u32) -> Option<(String, PathBuf)>;
}

/// 一个 dbnum 的文件身份（供 `db_type_of` / `file_of`）。
#[derive(Debug, Clone)]
struct DbFileEntry {
    db_type: String,
    project: String,
    path: PathBuf,
}

/// 就地内存定位器：`ref0→dbnum` + `dbnum→(db_type, project, path)`。
///
/// 用 [`InMemoryCataLocator::build_for_project`] 从 `dbnum_watermark` + 文件 `ref0` 扫描构建；
/// 或用 [`InMemoryCataLocator::from_parts`] 直接注入（单测 / 自定义来源）。
#[derive(Debug, Default, Clone)]
pub struct InMemoryCataLocator {
    ref0_to_dbnum: HashMap<u32, u32>,
    dbnum_files: HashMap<u32, DbFileEntry>,
}

static LOCATOR_CACHE: Lazy<TokioMutex<HashMap<String, InMemoryCataLocator>>> =
    Lazy::new(|| TokioMutex::new(HashMap::new()));

impl InMemoryCataLocator {
    /// 直接注入（单测 / 自定义来源）。`dbnum_files`：dbnum -> (db_type, project, path)。
    pub fn from_parts(
        ref0_to_dbnum: HashMap<u32, u32>,
        dbnum_files: HashMap<u32, (String, String, PathBuf)>,
    ) -> Self {
        let dbnum_files = dbnum_files
            .into_iter()
            .map(|(dbnum, (db_type, project, path))| {
                (
                    dbnum,
                    DbFileEntry {
                        db_type,
                        project,
                        path,
                    },
                )
            })
            .collect();
        Self {
            ref0_to_dbnum,
            dbnum_files,
        }
    }

    /// 已登记的 `ref0` 数（诊断用）。
    pub fn ref0_count(&self) -> usize {
        self.ref0_to_dbnum.len()
    }

    /// 已登记的 dbnum 数（诊断用）。
    pub fn dbnum_count(&self) -> usize {
        self.dbnum_files.len()
    }

    /// 端到端构建：读 `dbnum_watermark` 得各库文件身份 → 扫描各库 `ref0` 集（带磁盘指纹缓存）。
    ///
    /// `project`：工程名（`parse_file_db_basic_data` 语义需要 + `file_of` 返回）。
    pub async fn build_for_project(project: &str) -> anyhow::Result<Self> {
        if let Some(locator) = LOCATOR_CACHE.lock().await.get(project).cloned() {
            return Ok(locator);
        }

        let dbnum_files = load_dbnum_files_from_watermark(project).await?;

        // ref0 扫描（磁盘指纹缓存：文件未变则复用上次结果）。
        let mut cache = Ref0IndexCache::load(project);
        let mut ref0_to_dbnum: HashMap<u32, u32> = HashMap::new();
        let mut dirty = false;

        for (dbnum, entry) in &dbnum_files {
            let fp = file_fingerprint(&entry.path);
            let ref0s = if let Some(cached) = cache.get_if_fresh(*dbnum, &fp) {
                cached.clone()
            } else {
                let scanned = scan_db_ref0s(&entry.path, project);
                cache.put(*dbnum, fp, scanned.clone());
                dirty = true;
                scanned
            };
            for ref0 in ref0s {
                ref0_to_dbnum.insert(ref0, *dbnum);
            }
        }

        if dirty {
            cache.save(project);
        }

        let mut locator = Self::from_parts(
            ref0_to_dbnum,
            dbnum_files
                .into_iter()
                .map(|(dbnum, e)| (dbnum, (e.db_type, e.project, e.path)))
                .collect(),
        );

        // A project may have only its DESI dbnum parsed. In that state the
        // watermark cannot locate catalogue references yet, so discover CATA
        // files from the project itself and configured dependency projects on
        // the first on-demand request.
        if !locator
            .dbnum_files
            .values()
            .any(|entry| entry.db_type.eq_ignore_ascii_case("CATA"))
        {
            let option = get_db_option();
            let mut projects = vec![project.to_string()];
            projects.extend(option.included_projects.iter().cloned());
            projects.sort_unstable();
            projects.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            for catalogue_project in projects {
                let Some(catalogue_dir) =
                    crate::data_interface::project_paths::resolve_project_root(
                        option,
                        &catalogue_project,
                    )
                else {
                    continue;
                };
                if !catalogue_dir.is_dir() {
                    continue;
                }
                let discovered = Self::build_cata_from_dir(&catalogue_project, &catalogue_dir);
                locator.merge_cata_files(discovered);
            }
        }

        LOCATOR_CACHE
            .lock()
            .await
            .insert(project.to_string(), locator.clone());
        Ok(locator)
    }

    fn merge_cata_files(&mut self, other: Self) {
        let cata_dbnums: HashSet<u32> = other
            .dbnum_files
            .iter()
            .filter_map(|(&dbnum, entry)| {
                entry.db_type.eq_ignore_ascii_case("CATA").then_some(dbnum)
            })
            .collect();
        for (ref0, dbnum) in other.ref0_to_dbnum {
            if cata_dbnums.contains(&dbnum) {
                self.ref0_to_dbnum.insert(ref0, dbnum);
            }
        }
        for (dbnum, entry) in other.dbnum_files {
            if cata_dbnums.contains(&dbnum) {
                self.dbnum_files.insert(dbnum, entry);
            }
        }
    }

    /// 目录扫描构建（无需 SurrealDB）：遍历工程目录下全部 db 文件，建
    /// `ref0→dbnum` + `dbnum→(type, file)`。每个文件 index-only 解析（不解析属性）；
    /// 供离线校验 / 无库环境定位用。
    pub fn build_from_dir(project: &str, root_dir: &Path) -> Self {
        Self::build_from_dir_matching(project, root_dir, |_| true)
    }

    fn build_cata_from_dir(project: &str, root_dir: &Path) -> Self {
        Self::build_from_dir_matching(project, root_dir, |db_type| {
            db_type.eq_ignore_ascii_case("CATA")
        })
    }

    fn build_from_dir_matching(
        project: &str,
        root_dir: &Path,
        include_type: impl Fn(&str) -> bool,
    ) -> Self {
        let mut ref0_to_dbnum: HashMap<u32, u32> = HashMap::new();
        let mut dbnum_files: HashMap<u32, (String, String, PathBuf)> = HashMap::new();
        for entry in walkdir::WalkDir::new(root_dir)
            .max_depth(8)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.components().any(|c| {
                matches!(
                    c.as_os_str()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "back" | "backup"
                )
            }) {
                continue;
            }
            let Some(header) = scan_db_header(path) else {
                continue;
            };
            if !include_type(&header.db_type) {
                continue;
            }
            if let Some((dbnum, db_type, ref0s)) = scan_db_file(path, project) {
                for r in ref0s {
                    ref0_to_dbnum.insert(r, dbnum);
                }
                dbnum_files.entry(dbnum).or_insert((
                    db_type,
                    project.to_string(),
                    path.to_path_buf(),
                ));
            }
        }
        Self::from_parts(ref0_to_dbnum, dbnum_files)
    }
}

impl CataDbLocator for InMemoryCataLocator {
    fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32> {
        self.ref0_to_dbnum.get(&ref0).copied()
    }

    fn db_type_of(&self, dbnum: u32) -> Option<String> {
        self.dbnum_files.get(&dbnum).map(|e| e.db_type.clone())
    }

    fn file_of(&self, dbnum: u32) -> Option<(String, PathBuf)> {
        self.dbnum_files
            .get(&dbnum)
            .map(|e| (e.project.clone(), e.path.clone()))
    }
}

/// `dbnum_watermark` 的最小投影行（只取定位所需字段，反序列化保持简单）。
#[derive(Debug, Default, Deserialize)]
struct WatermarkFileRow {
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default)]
    db_type: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
}

/// 从 `dbnum_watermark` 读各库文件身份：`dbnum → DbFileEntry`。
///
/// 该表在（只读）扫描期即被 `DbnumState::record_scan` 登记，含**未解析**的 CATA 库，
/// 因此按需定位不依赖 CATA 是否已整库解析。
async fn load_dbnum_files_from_watermark(
    project: &str,
) -> anyhow::Result<HashMap<u32, DbFileEntry>> {
    let sql = format!("SELECT dbnum, db_type, file_path FROM {WATERMARK_TABLE};");
    let mut response = SUL_DB.query(sql).await?;
    let rows: Vec<WatermarkFileRow> = response.take(0).unwrap_or_default();

    let mut out = HashMap::new();
    for row in rows {
        let (Some(dbnum), Some(file_path)) = (row.dbnum, row.file_path) else {
            continue;
        };
        if file_path.is_empty() {
            continue;
        }
        out.insert(
            dbnum,
            DbFileEntry {
                db_type: row.db_type.unwrap_or_default(),
                project: project.to_string(),
                path: PathBuf::from(file_path),
            },
        );
    }
    Ok(out)
}

/// 扫描单个 db 文件的 `ref0` 集（供 `ref0→dbnum` 反查）。
///
/// 复用 `parse_file_db_basic_data` 的 `children_map`（owner→children 树）派生涉及的
/// 全部 refno，再取其 `ref0`。失败返回空集（该库暂不可定位，由上层惰性兜底/日志覆盖）。
fn scan_db_ref0s(path: &Path, project: &str) -> Vec<u32> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let db_basic = match parse_pdms_db::parse::parse_file_db_basic_data(
        &path.to_path_buf(),
        &file_name,
        project,
    ) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    let mut set: HashSet<u32> = HashSet::new();
    for (parent, children) in db_basic.children_map.iter() {
        set.insert(parent.get_0());
        for child in children.iter() {
            set.insert(child.get_0());
        }
    }
    for entry in db_basic.refno_table_map.iter() {
        set.insert(entry.key().get_0());
    }
    set.into_iter().collect()
}

/// 读单个 db 文件一次，返回 `(dbnum, db_type, 该库 ref0 集)`；非 db 文件返回 `None`。
fn scan_db_file(path: &Path, project: &str) -> Option<(u32, String, Vec<u32>)> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let db_basic =
        parse_pdms_db::parse::parse_file_db_basic_data(&path.to_path_buf(), &file_name, project)
            .ok()?;
    let info = parse_pdms_db::parse::parse_file_basic_info(&db_basic.bytes);
    if info.db_no == 0 {
        return None;
    }
    let mut set: HashSet<u32> = HashSet::new();
    for (parent, children) in db_basic.children_map.iter() {
        set.insert(parent.get_0());
        for child in children.iter() {
            set.insert(child.get_0());
        }
    }
    for entry in db_basic.refno_table_map.iter() {
        set.insert(entry.key().get_0());
    }
    Some((info.db_no, info.db_type, set.into_iter().collect()))
}

fn scan_db_header(path: &Path) -> Option<parse_pdms_db::parse::DbBasicInfo> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 60];
    file.read_exact(&mut header).ok()?;
    let info = parse_pdms_db::parse::parse_file_basic_info(&header);
    (info.db_no != 0 && !info.db_type.is_empty()).then_some(info)
}

/// 文件指纹（size + mtime 毫秒）；无法取得时为空串（视为始终“脏”，强制重扫）。
fn file_fingerprint(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{size}:{mtime}")
}

/// 每 dbnum 的 `ref0` 扫描缓存条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ref0CacheEntry {
    fingerprint: String,
    ref0s: Vec<u32>,
}

/// `ref0` 扫描的磁盘指纹缓存（json；best-effort，出错即忽略）。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Ref0IndexCache {
    by_dbnum: HashMap<u32, Ref0CacheEntry>,
}

impl Ref0IndexCache {
    fn cache_path(project: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aios_cata_locator_{project}.json"))
    }

    fn load(project: &str) -> Self {
        std::fs::read_to_string(Self::cache_path(project))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn get_if_fresh(&self, dbnum: u32, fingerprint: &str) -> Option<&Vec<u32>> {
        self.by_dbnum.get(&dbnum).and_then(|e| {
            (!fingerprint.is_empty() && e.fingerprint == fingerprint).then_some(&e.ref0s)
        })
    }

    fn put(&mut self, dbnum: u32, fingerprint: String, ref0s: Vec<u32>) {
        self.by_dbnum
            .insert(dbnum, Ref0CacheEntry { fingerprint, ref0s });
    }

    fn save(&self, project: &str) {
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::write(Self::cache_path(project), json);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2：by-refno 部分解析 + refno 级引用闭包（BFS）
// ─────────────────────────────────────────────────────────────────────────────

/// 部分解析得到的单个 CATA 元素（闭包扩展所需的最小信息）。
#[derive(Debug, Clone)]
pub struct ParsedCataEle {
    pub refno: RefU64,
    pub owner: RefU64,
    /// noun 的 `db1_hash`。
    pub noun: u32,
    /// noun 名（大写，来自属性表类型；未知时为空串）。
    pub noun_name: String,
    /// 该元素所有出向 `RefU64` 引用（闭包的横向边）。
    pub outbound: Vec<RefU64>,
    /// 该元素的成员/子节点（容器子树的纵向边）。
    pub children: Vec<RefU64>,
}

/// 从元素属性表抽取所有出向 `RefU64` 引用（`RefU64Type` / `RefnoEnumType` / `RefU64Array`）。
///
/// 与参考项目 `outbound_refs_of` 同源：不走白名单，自动覆盖 `GMRE/GSTR/NGMR/PTRE` 以及
/// `XGMREF/UDGEOM/TGEOM/PSPREF/GEOM` 等几何辅助边。
pub fn outbound_refs_of(att: &NamedAttrMap) -> Vec<RefU64> {
    let mut out = Vec::new();
    for value in att.map.values() {
        match value {
            NamedAttrValue::RefU64Type(r) => {
                if is_valid_ref0(r.get_0()) {
                    out.push(*r);
                }
            }
            NamedAttrValue::RefnoEnumType(re) => {
                let r = re.refno();
                if is_valid_ref0(r.get_0()) {
                    out.push(r);
                }
            }
            NamedAttrValue::RefU64Array(arr) => {
                for re in arr.iter() {
                    let r = re.refno();
                    if is_valid_ref0(r.get_0()) {
                        out.push(r);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 打开一个 db 读取会话（一次性读文件 + 建 refno 索引）；跨 BFS 轮复用。
fn open_db_session(project: &str, path: &Path) -> anyhow::Result<DbBasicData> {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    parse_pdms_db::parse::parse_file_db_basic_data(&path.to_path_buf(), file_name, project)
}

/// 用已打开会话解析一批 refno（不重读文件 / 不重建索引）。
///
/// `attmap_sink`：可选保留完整属性表（Phase 3 惰性兜底落库需要；闭包发现 pass 传 `None` 省内存）。
async fn parse_refnos_with_session(
    session: &DbBasicData,
    refnos: &[RefU64],
    mut attmap_sink: Option<&mut HashMap<RefU64, (NamedAttrMap, Vec<RefU64>)>>,
) -> anyhow::Result<HashMap<RefU64, ParsedCataEle>> {
    let db_info = aios_core::get_default_pdms_db_info();
    let mut out = HashMap::with_capacity(refnos.len());
    for &refno in refnos {
        let pos = {
            let Some(entry) = session.refno_table_map.get(&refno) else {
                continue; // 本库不含此 refno
            };
            entry.pos
        };
        if pos < 4 || pos > session.bytes.len() {
            continue;
        }
        match parse_pdms_db::parse::parse_ele_data_with_info(&session.bytes[pos - 4..], &db_info)
            .await
        {
            Ok(ele) => {
                let merged = ele.whole_attmap.merge();
                let outbound = outbound_refs_of(&merged);
                let children: Vec<RefU64> = ele
                    .children
                    .iter()
                    .copied()
                    .filter(|r| is_valid_ref0(r.get_0()))
                    .collect();
                if let Some(sink) = attmap_sink.as_deref_mut() {
                    sink.insert(refno, (merged.clone(), children.clone()));
                }
                let noun_name = merged.get_type_str().trim().to_uppercase();
                out.insert(
                    refno,
                    ParsedCataEle {
                        refno,
                        owner: ele.owner,
                        noun: ele.noun,
                        noun_name,
                        outbound,
                        children,
                    },
                );
            }
            Err(_) => {
                // 解析失败：跳过，由调用方按 cache-miss 处理。
            }
        }
    }
    Ok(out)
}

/// 对单个 db 文件按 refno 子集做部分解析（一次性 `open` + 建索引）。
pub async fn parse_db_refnos(
    project: &str,
    path: &Path,
    refnos: &[RefU64],
) -> anyhow::Result<HashMap<RefU64, ParsedCataEle>> {
    if refnos.is_empty() {
        return Ok(HashMap::new());
    }
    let session = open_db_session(project, path)?;
    parse_refnos_with_session(&session, refnos, None).await
}

/// 闭包行为配置。
#[derive(Debug, Clone)]
pub struct CataClosureConfig {
    /// 是否纳入 owner 祖先链（默认开）。
    pub include_owner_chain: bool,
    /// 是否纳入容器子树（成员，默认开）。
    pub follow_children: bool,
    /// 收口的 db_type 集合（大小写不敏感，默认 {"CATA"}）。
    pub cata_db_types: HashSet<String>,
    /// 闭包解析时显式排除的 dbnum（如根 DESI 自身）。
    pub excluded_dbnums: HashSet<u32>,
    /// 防御性轮数上限。
    pub max_rounds: usize,
    /// 容器子树展开白名单（noun 名，大写）。
    ///
    /// - `None`：全部展开；
    /// - `Some(set)`：仅展开集合内名词的 children —— 避免经 owner 链到达 SPEC/SELE
    ///   后整个规格世界被子树展开拉爆。
    pub container_subtree_nouns: Option<HashSet<String>>,
}

impl Default for CataClosureConfig {
    fn default() -> Self {
        let mut cata_db_types = HashSet::new();
        cata_db_types.insert("CATA".to_string());
        Self {
            include_owner_chain: true,
            follow_children: true,
            cata_db_types,
            excluded_dbnums: HashSet::new(),
            max_rounds: 64,
            container_subtree_nouns: None,
        }
    }
}

impl CataClosureConfig {
    /// 精确模式（refno 级按需 / 惰性小闭包）：children 仅对几何与点集容器展开
    /// （GMSE/GMSS/NGMS/PTSE/PSTR/SPRO/DTSE），不展开 SPEC/SELE 等规格容器。
    ///
    /// 挤出 / 回转体的轮廓是三层：`SEXT|NSEX|SREV|NSRE → SLOO → 顶点`。名单停在
    /// 几何集这一层时闭包仍报 `missing=0`，落库的却是没有顶点的空壳几何。
    pub fn precise() -> Self {
        let container: HashSet<String> = [
            "GMSE", "GMSS", "NGMS", "PTSE", "PSTR", "SPRO", "DTSE", "SEXT", "NSEX", "SREV", "NSRE",
            "SLOO",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut cata_db_types = HashSet::new();
        cata_db_types.insert("CATA".to_string());
        cata_db_types.insert("DESI".to_string());
        Self {
            cata_db_types,
            container_subtree_nouns: Some(container),
            ..Self::default()
        }
    }

    pub fn excluding_dbnum(mut self, dbnum: u32) -> Self {
        self.excluded_dbnums.insert(dbnum);
        self
    }

    pub fn excluding_dbnums(mut self, dbnums: impl IntoIterator<Item = u32>) -> Self {
        self.excluded_dbnums.extend(dbnums);
        self
    }
}

/// 闭包结果：每个 CATA dbnum 需解析的 refno 集合 + 统计。
#[derive(Debug, Clone, Default)]
pub struct CataClosureManifest {
    /// dbnum -> 该库内闭包覆盖到（且成功解析）的 refno 集合。
    pub by_dbnum: BTreeMap<u32, BTreeSet<RefU64>>,
    /// 种子数。
    pub seed_count: usize,
    /// visited 总数。
    pub visited_count: usize,
    /// BFS 轮数。
    pub rounds: usize,
    /// 缺失计数（无 dbnum 映射 / 库内未找到 / 解析失败）。
    pub missing: usize,
}

/// refno 级 CATA 引用闭包引擎（BFS）。
///
/// 用法：`new` → `seed`(DESI 出向引用) → `resolve()`。
pub struct CataClosureResolver<'a, L: CataDbLocator> {
    locator: &'a L,
    cfg: CataClosureConfig,
    visited: HashSet<RefU64>,
    frontier: Vec<RefU64>,
    /// 每个 dbnum 的打开会话缓存（复用页缓存，跨 BFS 轮不重读文件 / 不重建索引）。
    sessions: HashMap<u32, DbBasicData>,
    /// 是否保留完整属性表（Phase 3 惰性兜底落库用；闭包发现 pass 默认关省内存）。
    retain_attmaps: bool,
    /// `retain_attmaps` 开启时收集：refno -> (完整属性表, children)。
    attmaps: HashMap<RefU64, (NamedAttrMap, Vec<RefU64>)>,
}

impl<'a, L: CataDbLocator> CataClosureResolver<'a, L> {
    pub fn new(locator: &'a L, cfg: CataClosureConfig) -> Self {
        Self {
            locator,
            cfg,
            visited: HashSet::new(),
            frontier: Vec::new(),
            sessions: HashMap::new(),
            retain_attmaps: false,
            attmaps: HashMap::new(),
        }
    }

    /// 开启属性表保留（小闭包惰性兜底场景；大闭包慎用，内存随 visited 线性增长）。
    pub fn with_retain_attmaps(mut self, retain: bool) -> Self {
        self.retain_attmaps = retain;
        self
    }

    /// 取走保留的属性表（`retain_attmaps` 开启时在 `resolve()` 后调用）。
    pub fn take_attmaps(&mut self) -> HashMap<RefU64, (NamedAttrMap, Vec<RefU64>)> {
        std::mem::take(&mut self.attmaps)
    }

    /// 播种：把从 DESI 收集到的出向引用作为闭包起点。非 CATA 的种子会在收口阶段被丢弃。
    pub fn seed(&mut self, refs: impl IntoIterator<Item = RefU64>) {
        self.frontier.extend(refs);
    }

    /// 跑完整 BFS 闭包：每轮按 dbnum 聚合 frontier → 部分解析 → 跟随 outbound（横向）
    /// + owner（纵向）+ children（容器子树）→ visited 去重，直至 frontier 空或达 `max_rounds`。
    pub async fn resolve(&mut self) -> anyhow::Result<CataClosureManifest> {
        let include_owner = self.cfg.include_owner_chain;
        let follow_children = self.cfg.follow_children;
        let max_rounds = self.cfg.max_rounds;
        let container_allow = self.cfg.container_subtree_nouns.clone();
        let cata_types: HashSet<String> = self
            .cfg
            .cata_db_types
            .iter()
            .map(|t| t.to_uppercase())
            .collect();
        let excluded_dbnums = self.cfg.excluded_dbnums.clone();

        let seed_count = self.frontier.len();
        let mut by_dbnum: BTreeMap<u32, BTreeSet<RefU64>> = BTreeMap::new();
        let mut missing = 0usize;
        let mut rounds = 0usize;

        while !self.frontier.is_empty() && rounds < max_rounds {
            rounds += 1;
            let current = std::mem::take(&mut self.frontier);

            // 按 dbnum 聚合本轮 frontier（db_type 收口到 CATA）。
            let mut by_db: HashMap<u32, Vec<RefU64>> = HashMap::new();
            for r in current {
                if self.visited.contains(&r) {
                    continue;
                }
                let ref0 = r.get_0();
                if !is_valid_ref0(ref0) {
                    continue;
                }
                let Some(dbnum) = self.locator.dbnum_of_ref0(ref0) else {
                    missing += 1;
                    continue;
                };
                if excluded_dbnums.contains(&dbnum) {
                    continue;
                }
                let db_type = self.locator.db_type_of(dbnum).unwrap_or_default();
                if !cata_types.contains(&db_type.to_uppercase()) {
                    continue; // 非 CATA（回指 DESI/DICT 等）不下探
                }
                by_db.entry(dbnum).or_default().push(r);
            }

            for (dbnum, refs) in by_db {
                let to_parse: Vec<RefU64> = refs
                    .into_iter()
                    .filter(|r| !self.visited.contains(r))
                    .collect();
                if to_parse.is_empty() {
                    continue;
                }

                // 确保该库会话已打开（一次性 open + 建索引；后续轮复用）。
                if !self.sessions.contains_key(&dbnum) {
                    let Some((project, path)) = self.locator.file_of(dbnum) else {
                        missing += to_parse.len();
                        for r in &to_parse {
                            self.visited.insert(*r); // 无文件信息也标记，避免无限重试
                        }
                        continue;
                    };
                    match open_db_session(&project, &path) {
                        Ok(sess) => {
                            self.sessions.insert(dbnum, sess);
                        }
                        Err(e) => {
                            log::warn!(
                                "[cata_closure] 打开闭包依赖库失败: dbnum={} path={} error={}",
                                dbnum,
                                path.display(),
                                e
                            );
                            missing += to_parse.len();
                            for r in &to_parse {
                                self.visited.insert(*r);
                            }
                            continue;
                        }
                    }
                }

                let parsed = {
                    let session = self.sessions.get(&dbnum).expect("session just ensured");
                    let attmap_sink = if self.retain_attmaps {
                        Some(&mut self.attmaps)
                    } else {
                        None
                    };
                    parse_refnos_with_session(session, &to_parse, attmap_sink).await?
                };

                let mut next: Vec<RefU64> = Vec::new();
                for r in &to_parse {
                    if !self.visited.insert(*r) {
                        continue;
                    }
                    match parsed.get(r) {
                        Some(ele) => {
                            by_dbnum.entry(dbnum).or_default().insert(*r);
                            next.extend(ele.outbound.iter().copied());
                            if include_owner && is_valid_ref0(ele.owner.get_0()) {
                                next.push(ele.owner);
                            }
                            if follow_children {
                                let expand_children = match &container_allow {
                                    None => true,
                                    Some(allow) => allow.contains(&ele.noun_name),
                                };
                                if expand_children {
                                    next.extend(ele.children.iter().copied());
                                }
                            }
                        }
                        None => {
                            missing += 1; // 请求了但本库未找到 / 解析失败
                        }
                    }
                }
                for n in next {
                    if !self.visited.contains(&n) {
                        self.frontier.push(n);
                    }
                }
            }
        }

        Ok(CataClosureManifest {
            by_dbnum,
            seed_count,
            visited_count: self.visited.len(),
            rounds,
            missing,
        })
    }
}

/// 设计侧子树出向引用收集（按需播种）。
///
/// 给定设计元素根 refno（如 BRAN / PIPE / ZONE），在其所属 DESI 库内沿 `children`
/// 做子树 BFS（部分解析，**不整库解析**），收集子树内全部元素的出向 `RefU64` 作为
/// 后续 CATA 闭包种子。返回 `(种子集合, 子树元素数)`。
pub async fn collect_design_subtree_outbound<L: CataDbLocator>(
    locator: &L,
    roots: &[RefU64],
) -> anyhow::Result<(Vec<RefU64>, usize)> {
    let mut sessions: HashMap<u32, DbBasicData> = HashMap::new();
    let mut visited: HashSet<RefU64> = HashSet::new();
    let mut seeds: HashSet<RefU64> = HashSet::new();
    let mut frontier: Vec<RefU64> = roots
        .iter()
        .copied()
        .filter(|r| is_valid_ref0(r.get_0()))
        .collect();
    let mut parsed_count = 0usize;

    while !frontier.is_empty() {
        let mut by_db: HashMap<u32, Vec<RefU64>> = HashMap::new();
        for r in frontier.drain(..) {
            if !visited.insert(r) {
                continue;
            }
            match locator.dbnum_of_ref0(r.get_0()) {
                Some(dbnum) => by_db.entry(dbnum).or_default().push(r),
                None => {
                    log::warn!(
                        "[cata_closure] 设计子树 BFS：ref0 {} 无 dbnum 映射，跳过",
                        r.get_0()
                    );
                }
            }
        }
        for (dbnum, refs) in by_db {
            if !sessions.contains_key(&dbnum) {
                let Some((project, path)) = locator.file_of(dbnum) else {
                    log::warn!(
                        "[cata_closure] 设计子树 BFS：dbnum {} 无文件映射，跳过",
                        dbnum
                    );
                    continue;
                };
                match open_db_session(&project, &path) {
                    Ok(s) => {
                        sessions.insert(dbnum, s);
                    }
                    Err(e) => {
                        log::warn!("[cata_closure] 打开设计库失败 dbnum={}: {}", dbnum, e);
                        continue;
                    }
                }
            }
            let session = sessions.get(&dbnum).expect("session 已插入");
            let parsed = parse_refnos_with_session(session, &refs, None).await?;
            parsed_count += parsed.len();
            for ele in parsed.values() {
                seeds.extend(ele.outbound.iter().copied());
                frontier.extend(ele.children.iter().copied());
            }
        }
    }
    Ok((seeds.into_iter().collect(), parsed_count))
}

/// Collect catalogue references from the already-parsed design subtree.
///
/// This is the runtime authority: DESI rows and their typed attribute tables are
/// already present in SurrealDB, and preserve references such as SPRE/HSTU/LSTU
/// even when the partial binary parser cannot decode a design element.
async fn collect_database_subtree_outbound(roots: &[RefU64]) -> anyhow::Result<Vec<RefU64>> {
    let mut visited = HashSet::new();
    let mut scope = Vec::new();
    let mut frontier: Vec<RefnoEnum> = roots.iter().copied().map(RefnoEnum::from).collect();
    while let Some(refno) = frontier.pop() {
        if !visited.insert(refno) {
            continue;
        }
        scope.push(refno);
        for child in aios_core::get_children_pes(refno).await.unwrap_or_default() {
            frontier.push(child.refno);
        }
    }

    let mut seeds = HashSet::new();
    for chunk in scope.chunks(200) {
        let keys = chunk
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT VALUE object::values(refno.*)[WHERE type::is::record($this)] \
             FROM [{keys}];"
        );
        let mut response = SUL_DB.query(sql).await?;
        let refs: Vec<Vec<RefnoEnum>> = response.take(0)?;
        seeds.extend(refs.into_iter().flatten().map(|r| r.refno()));
    }
    Ok(seeds.into_iter().collect())
}

/// refno 级按需闭包入口：以给定生成根（如单个 BRAN）的子树出向引用为种子，跑 CATA 闭包。
///
/// 返回闭包 manifest（各 CATA dbnum 需解析的 refno 集）。Phase 3 据此 `ensure_cata_refnos_parsed`
/// 落库。种子根自身所属 dbnum 会被自动排除（不重解析根 DESI）。
pub async fn run_cata_closure_pass_for_refnos<L: CataDbLocator>(
    locator: &L,
    seed_roots: &[RefU64],
    cfg: CataClosureConfig,
) -> anyhow::Result<CataClosureManifest> {
    let (seeds, subtree_count) = collect_design_subtree_outbound(locator, seed_roots).await?;
    log::info!(
        "[cata_closure] 设计子树元素 {} 个 → 收集种子 {} 个",
        subtree_count,
        seeds.len()
    );
    let seed_count = seeds.len();
    let exclude_dbnums = seed_roots
        .iter()
        .filter_map(|root| locator.dbnum_of_ref0(root.get_0()))
        .collect::<HashSet<_>>();
    let mut resolver = CataClosureResolver::new(locator, cfg.excluding_dbnums(exclude_dbnums));
    resolver.seed(seeds);
    let mut manifest = resolver.resolve().await?;
    manifest.seed_count = seed_count;
    log::info!(
        "[cata_closure] refno 级闭包完成: {} 个闭包库 / visited={} / missing={}",
        manifest.by_dbnum.len(),
        manifest.visited_count,
        manifest.missing
    );
    Ok(manifest)
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3：运行期惰性兜底（命中未解析 CATA refno → 小闭包 → 落 pe/ATT_* → 重试）
// ─────────────────────────────────────────────────────────────────────────────

/// 落库分批大小。
const INSERT_CHUNK: usize = 500;

/// 惰性兜底全局互斥：并发 miss 串行化，避免重复解析同一批元素（落库 INSERT IGNORE 幂等）。
static LAZY_CATA_FALLBACK_LOCK: Lazy<TokioMutex<()>> = Lazy::new(|| TokioMutex::new(()));

/// 惰性兜底结果统计。
#[derive(Debug, Default, Clone)]
pub struct LazyFallbackOutcome {
    pub parsed: usize,
    pub missing: usize,
}

/// 是否启用按需解析。**默认 On**（按需解析 CATA 为默认行为）；env `AIOS_CATA_CLOSURE_MODE`
/// 可双向覆盖：`manifest`/`on`/`1`/`true`/`yes` 强制开，`off`/`0`/`false`/`no` 强制关，
/// 未设置或取值无法识别时取默认(On)。显式关值供冒烟对照(整库 vs 按需)与临时回退使用。
pub fn cata_closure_enabled() -> bool {
    match std::env::var("AIOS_CATA_CLOSURE_MODE") {
        Ok(v) => {
            let v = v.trim();
            if v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("no")
                || v == "0"
            {
                false
            } else {
                // on / manifest / 1 / true / yes / 其它无法识别值 → 默认 On
                true
            }
        }
        // 未设置 → 默认 On
        Err(_) => true,
    }
}

/// 运行期惰性兜底：对未解析的 CATA refno 跑**小闭包**并即时落库
/// （`pe` + `ATT_{noun}` + `ATT_UDA` + `pe_owner`，全部 INSERT IGNORE / RELATION 幂等），
/// 使随后 `get_named_attmap` 透明命中。
///
/// 调用方约定：命中「pe 缺失」再调用（如 `get_named_attmap` 失败路径），成功后重试原查询。
pub async fn ensure_cata_refnos_parsed(
    project: &str,
    seeds: &[RefU64],
) -> anyhow::Result<LazyFallbackOutcome> {
    if seeds.is_empty() {
        return Ok(LazyFallbackOutcome::default());
    }
    let _guard = LAZY_CATA_FALLBACK_LOCK.lock().await;

    // 1. 定位器（内存，复用 dbnum_watermark + ref0 缓存）。
    let locator = InMemoryCataLocator::build_for_project(project).await?;

    // 2. 小闭包（保留属性表；precise 防 SPEC 子树发散）。
    let mut resolver =
        CataClosureResolver::new(&locator, CataClosureConfig::precise()).with_retain_attmaps(true);
    resolver.seed(seeds.iter().copied());
    let delta = resolver.resolve().await?;
    let retained = resolver.take_attmaps();

    // 3. 落库：pe + ATT_{noun} + ATT_UDA + pe_owner，全部幂等。
    let mut parsed = 0usize;
    for (dbnum, refs) in &delta.by_dbnum {
        let mut pe_jsons: Vec<String> = Vec::new();
        let mut att_by_table: HashMap<String, Vec<String>> = HashMap::new();
        let mut uda_jsons: Vec<String> = Vec::new();
        let mut relate_jsons: Vec<String> = Vec::new();

        for refno in refs {
            let Some((att, children)) = retained.get(refno) else {
                continue;
            };
            // pe 行（与 versioned_db::pe::save_pes 同构）。
            let pe_data = att.pe(*dbnum as i32);
            pe_jsons.push(pe_data.gen_sur_json(Some(refno.to_pe_key())));

            // ATT_{noun} / ATT_UDA 行。
            let table = att.get_type_str().to_string();
            if !table.is_empty() {
                if let Some(json) = att.gen_sur_json() {
                    att_by_table.entry(table).or_default().push(json);
                }
                if let Some(json) = att.gen_sur_json_uda(&[]) {
                    uda_jsons.push(normalize_sql_string(&json));
                }
            }

            // pe_owner 关系（与 versioned_db::pe::save_pe_relates 同构）。
            let op = refno.to_pe_key();
            for (i, child) in children.iter().enumerate() {
                let cp = child.to_pe_key();
                relate_jsons.push(format!(
                    "{{ id: pe_owner:[{op}, {i}], in: {cp}, out: {op} }}"
                ));
            }
            parsed += 1;
        }

        // ADR-017 §7：CATA 按需解析产物**随窗口提交**，不再直写持久层——窗口内走
        // `execute_model_write`（暂存生效 + 进 journal），写回时 INSERT IGNORE 对
        // 持久层已有行是空操作、新元素落地；窗口外回落历史持久层直写。载荷全部
        // 带显式 id（pe 显式传入，ATT_*/ATT_UDA/pe_owner 由 rs-core 渲染函数插入），
        // 满足 ReplaySafe，重放幂等。
        for chunk in pe_jsons.chunks(INSERT_CHUNK) {
            let sql = format!("INSERT IGNORE INTO pe [{}]", chunk.join(","));
            crate::surreal_retry::execute_model_write(&sql, "persist CATA pe").await?;
        }
        for (table, jsons) in att_by_table {
            for chunk in jsons.chunks(INSERT_CHUNK) {
                let sql = format!("INSERT IGNORE INTO {} [{}]", table, chunk.join(","));
                crate::surreal_retry::execute_model_write(&sql, "persist CATA attributes")
                    .await?;
            }
        }
        for chunk in uda_jsons.chunks(INSERT_CHUNK) {
            let sql = format!("INSERT IGNORE INTO ATT_UDA [{}]", chunk.join(","));
            crate::surreal_retry::execute_model_write(&sql, "persist CATA UDA").await?;
        }
        for chunk in relate_jsons.chunks(INSERT_CHUNK) {
            let sql = format!("INSERT RELATION INTO pe_owner [{}]", chunk.join(","));
            crate::surreal_retry::execute_model_write(&sql, "persist CATA ownership")
                .await?;
        }
    }

    log::info!(
        "[cata_closure] 惰性兜底完成: seeds={} parsed={} missing={} rounds={}",
        seeds.len(),
        parsed,
        delta.missing,
        delta.rounds
    );
    Ok(LazyFallbackOutcome {
        parsed,
        missing: delta.missing,
    })
}

/// 主动预解析（Phase 4）：给定一组生成根，收集其子树出向引用 → 跑 CATA 闭包 → 批量落库。
///
/// 与惰性兜底并存：主动保效率（每批根一次），惰性收漏边。受 env 开关门控（默认 Off）。
pub async fn ensure_cata_parsed_for_roots(
    project: &str,
    roots: &[RefU64],
) -> anyhow::Result<LazyFallbackOutcome> {
    if roots.is_empty() || !cata_closure_enabled() {
        return Ok(LazyFallbackOutcome::default());
    }
    let locator = InMemoryCataLocator::build_for_project(project).await?;
    let (seeds, subtree_count) = collect_design_subtree_outbound(&locator, roots).await?;
    log::info!(
        "[cata_closure] 主动预解析: roots={} 子树元素={} 种子={}",
        roots.len(),
        subtree_count,
        seeds.len()
    );
    ensure_cata_refnos_parsed(project, &seeds).await
}

/// resolve 层调用的惰性兜底入口：受 env 开关门控（默认 Off 即直接返回 false，零回归）。
///
/// 命中未解析 CATA refno 时调用；返回 `true` 表示已补齐、值得重试原查询。
pub async fn try_lazy_cata_fallback(cata_refno: RefnoEnum) -> bool {
    if !cata_closure_enabled() {
        return false;
    }
    let project = get_db_option().project_name.clone();
    match ensure_cata_refnos_parsed(&project, &[cata_refno.refno()]).await {
        Ok(outcome) if outcome.parsed > 0 => {
            log::info!(
                "[cata_closure] 惰性兜底成功: {} 解析落库 {} 个元素（missing={}），重试原查询",
                cata_refno,
                outcome.parsed,
                outcome.missing
            );
            true
        }
        Ok(outcome) => {
            log::warn!(
                "[cata_closure] 惰性兜底未解析到元素: {}（missing={}）",
                cata_refno,
                outcome.missing
            );
            false
        }
        Err(e) => {
            log::warn!("[cata_closure] 惰性兜底失败: {}: {}", cata_refno, e);
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5：单根几何冒烟摘要（开/关按需 diff 校验；needs 活库 + 数据环境）
// ─────────────────────────────────────────────────────────────────────────────

/// 单根几何冒烟报告：对一组设计参考号逐个 `resolve_desi_comp` 算确定性摘要。
///
/// 用法：同一批 refno 跑两遍（`AIOS_CATA_CLOSURE_MODE` 关 vs 开），`combined_digest`
/// 与 `per_refno` 应逐条一致 —— 证明「按需解析 == 整库解析」不漏/不改几何。
#[derive(Debug, Default, Serialize)]
pub struct GeoSmokeReport {
    /// 本次运行按需开关是否开启。
    pub mode_on: bool,
    pub total: usize,
    pub ok: usize,
    pub err: usize,
    /// 全部 (refno, digest) 排序后的合并摘要（跨运行可直接比对）。
    pub combined_digest: u64,
    /// 每个 refno 的几何摘要（err 记 0），按 refno 排序。
    pub per_refno: Vec<(String, u64)>,
}

fn digest_of<T: std::fmt::Debug>(value: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{value:?}").hash(&mut h);
    h.finish()
}

/// 对一组设计参考号逐个求解几何并算确定性摘要（调用前需已连接 `SUL_DB`）。
///
/// 只读；不落库、不改状态。开关开启时，遇未解析 CATA 会经 resolve 惰性兜底自动补齐。
pub async fn geo_smoke_digest(design_refnos: &[RefnoEnum]) -> GeoSmokeReport {
    let mut per_refno: Vec<(String, u64)> = Vec::new();
    let mut ok = 0usize;
    let mut err = 0usize;
    for &refno in design_refnos {
        match crate::fast_model::resolve_desi_comp(refno, None).await {
            Ok(info) => {
                per_refno.push((refno.to_string(), digest_of(&info)));
                ok += 1;
            }
            Err(_) => {
                per_refno.push((refno.to_string(), 0));
                err += 1;
            }
        }
    }
    per_refno.sort();
    let mut ch = std::collections::hash_map::DefaultHasher::new();
    for (r, d) in &per_refno {
        r.hash(&mut ch);
        d.hash(&mut ch);
    }
    GeoSmokeReport {
        mode_on: cata_closure_enabled(),
        total: design_refnos.len(),
        ok,
        err,
        combined_digest: ch.finish(),
        per_refno,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6：(dbnum, generation root)→CATA 依赖缓存（bincode，applied_sesno 失效）
// ─────────────────────────────────────────────────────────────────────────────

/// 单个生成根的 CATA 依赖缓存条目。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CataDepEntry {
    /// 精确闭包规则版本；白名单变化时递增，避免复用缺少新容器子树的旧依赖。
    closure_schema_version: u32,
    /// 建缓存时源库的 applied_sesno（失效判定：sesno 变则重算）。
    source_sesno: i32,
    /// 展平的 CATA 参考号（`RefU64.0`，bincode 友好）。
    cata_refnos: Vec<u64>,
    updated_at_ms: i64,
}

/// `(源 dbnum, 生成根 refno) → CATA 依赖 ids` 缓存。
///
/// 依赖闭包是从一个生成根的子树收集出来的，不能只按 dbnum 缓存：同一 DESI
/// 库中的不同 BRAN/FTUB 通常引用完全不同的 CATA 元素。
#[derive(Debug, Default, Serialize, Deserialize)]
struct CataDepCache {
    by_source_root: HashMap<(u32, u64), CataDepEntry>,
}

const CATA_CLOSURE_SCHEMA_VERSION: u32 = 3;

impl CataDepCache {
    /// 缓存文件路径：env `AIOS_CATA_DEP_CACHE_PATH` 覆盖，缺省 `output/<project>/cata_dep_cache.bin`。
    fn cache_path(project: &str) -> PathBuf {
        if let Ok(p) = std::env::var("AIOS_CATA_DEP_CACHE_PATH") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        PathBuf::from("output")
            .join(project)
            .join("cata_dep_cache.bin")
    }

    fn load(project: &str) -> Self {
        std::fs::read(Self::cache_path(project))
            .ok()
            .and_then(|b| bincode::deserialize(&b).ok())
            .unwrap_or_default()
    }

    /// 命中且新鲜（source_sesno 相符）时返回缓存的 CATA 参考号。
    fn get_fresh(&self, source_dbnum: u32, root: RefU64, source_sesno: i32) -> Option<&Vec<u64>> {
        self.by_source_root
            .get(&(source_dbnum, root.0))
            // Empty entries may have been produced before dependency-project
            // CATA discovery was available. Treat them as misses so they heal.
            .filter(|e| {
                e.closure_schema_version == CATA_CLOSURE_SCHEMA_VERSION
                    && e.source_sesno == source_sesno
                    && !e.cata_refnos.is_empty()
            })
            .map(|e| &e.cata_refnos)
    }

    fn put(&mut self, source_dbnum: u32, root: RefU64, source_sesno: i32, cata_refnos: Vec<u64>) {
        self.by_source_root.insert(
            (source_dbnum, root.0),
            CataDepEntry {
                closure_schema_version: CATA_CLOSURE_SCHEMA_VERSION,
                source_sesno,
                cata_refnos,
                updated_at_ms: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    fn save(&self, project: &str) {
        let path = Self::cache_path(project);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = bincode::serialize(self) {
            let tmp = path.with_extension("bin.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// 生成入口的**缓存感知预加载**（Phase 6）：按源 dbnum + 生成根读依赖缓存 → 命中即批量预加载；
/// 未命中 / 过期则现算闭包、写缓存、再预加载。受 env 开关门控（默认 Off）。
///
/// 失效口径：源库 `applied_sesno`（权威数据版本）。CATA 定义变不改「依赖哪些 id」，
/// 故仅由源库变更驱动重算。预加载复用 [`ensure_cata_refnos_parsed`]（幂等落库）。
pub async fn preload_cata_for_roots(
    project: &str,
    roots: &[RefU64],
) -> anyhow::Result<LazyFallbackOutcome> {
    if roots.is_empty() || !cata_closure_enabled() {
        return Ok(LazyFallbackOutcome::default());
    }
    let locator = InMemoryCataLocator::build_for_project(project).await?;

    // 按源 dbnum 分组生成根。
    let mut by_src: HashMap<u32, Vec<RefU64>> = HashMap::new();
    for &r in roots {
        if let Some(d) = locator.dbnum_of_ref0(r.get_0()) {
            by_src.entry(d).or_default().push(r);
        }
    }

    let mut cache = CataDepCache::load(project);
    let mut seeds: HashSet<RefU64> = HashSet::new();
    let mut dirty = false;

    for (src_dbnum, src_roots) in by_src {
        let sesno = crate::data_interface::dbnum_state::DbnumState::applied_sesno(src_dbnum)
            .await
            .unwrap_or(0);
        for root in src_roots {
            if let Some(ids) = cache.get_fresh(src_dbnum, root, sesno) {
                log::info!(
                    "[cata_closure] 依赖缓存命中: src_dbnum={} root={:?} sesno={} ids={}",
                    src_dbnum,
                    root,
                    sesno,
                    ids.len()
                );
                seeds.extend(ids.iter().map(|&u| RefU64(u)));
            } else {
                let manifest = run_cata_closure_pass_for_refnos(
                    &locator,
                    &[root],
                    CataClosureConfig::precise(),
                )
                .await?;
                let mut flat: HashSet<u64> = manifest
                    .by_dbnum
                    .values()
                    .flat_map(|s| s.iter().map(|r| r.0))
                    .collect();
                let database_outbound = collect_database_subtree_outbound(&[root]).await?;
                let database_seeds = database_outbound
                    .iter()
                    .copied()
                    .filter(|seed| {
                        locator
                            .dbnum_of_ref0(seed.get_0())
                            .and_then(|dbnum| locator.db_type_of(dbnum))
                            .is_some_and(|db_type| db_type.eq_ignore_ascii_case("CATA"))
                    })
                    .collect::<Vec<_>>();
                log::info!(
                    "[cata_closure] 数据库子树引用: total={} cata={} sample={:?}",
                    database_outbound.len(),
                    database_seeds.len(),
                    database_outbound.iter().take(8).collect::<Vec<_>>()
                );
                if !database_seeds.is_empty() {
                    let mut resolver =
                        CataClosureResolver::new(&locator, CataClosureConfig::precise());
                    resolver.seed(database_seeds);
                    let database_manifest = resolver.resolve().await?;
                    flat.extend(
                        database_manifest
                            .by_dbnum
                            .values()
                            .flat_map(|refs| refs.iter().map(|r| r.0)),
                    );
                }
                let flat: Vec<u64> = flat.into_iter().collect();
                log::info!(
                    "[cata_closure] 依赖缓存更新: src_dbnum={} root={:?} sesno={} ids={} (CATA 库数={})",
                    src_dbnum,
                    root,
                    sesno,
                    flat.len(),
                    manifest.by_dbnum.len()
                );
                cache.put(src_dbnum, root, sesno, flat.clone());
                dirty = true;
                seeds.extend(flat.into_iter().map(RefU64));
            }
        }
    }
    if dirty {
        cache.save(project);
    }

    let seeds: Vec<RefU64> = seeds.into_iter().collect();
    ensure_cata_refnos_parsed(project, &seeds).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_cache_is_scoped_to_each_generation_root() {
        let mut cache = CataDepCache::default();
        let first_root = RefnoEnum::from("24384/22402").refno();
        let second_root = RefnoEnum::from("24384/22404").refno();

        cache.put(
            8000,
            first_root,
            10,
            vec![RefnoEnum::from("13244/108798").refno().0],
        );

        assert!(cache.get_fresh(8000, first_root, 10).is_some());
        assert!(
            cache.get_fresh(8000, second_root, 10).is_none(),
            "同一 DESI dbnum 下的不同 BRAN 不能共用局部 CATA 闭包"
        );

        cache
            .by_source_root
            .get_mut(&(8000, first_root.0))
            .unwrap()
            .closure_schema_version -= 1;
        assert!(
            cache.get_fresh(8000, first_root, 10).is_none(),
            "精确闭包白名单变化后不能复用缺少新容器子树的旧缓存"
        );
    }

    #[test]
    fn locator_merges_only_cata_files_from_dependency_projects() {
        let mut primary = InMemoryCataLocator::from_parts(
            HashMap::from([(24384, 8000)]),
            HashMap::from([(
                8000,
                (
                    "DESI".into(),
                    "AvevaMarineSample".into(),
                    PathBuf::from("desi"),
                ),
            )]),
        );
        let dependency = InMemoryCataLocator::from_parts(
            HashMap::from([(13244, 7320), (999, 5100)]),
            HashMap::from([
                (
                    7320,
                    (
                        "CATA".into(),
                        "AvevaCatalogue".into(),
                        PathBuf::from("cata"),
                    ),
                ),
                (
                    5100,
                    (
                        "DICT".into(),
                        "AvevaCatalogue".into(),
                        PathBuf::from("dict"),
                    ),
                ),
            ]),
        );

        primary.merge_cata_files(dependency);

        assert_eq!(primary.dbnum_of_ref0(13244), Some(7320));
        assert_eq!(primary.dbnum_of_ref0(999), None);
        assert_eq!(primary.file_of(7320).unwrap().0, "AvevaCatalogue");
    }

    fn locator() -> InMemoryCataLocator {
        let mut ref0_to_dbnum = HashMap::new();
        ref0_to_dbnum.insert(100u32, 7320u32);
        ref0_to_dbnum.insert(2013286676u32, 7320u32);
        ref0_to_dbnum.insert(200u32, 8001u32);

        let mut dbnum_files = HashMap::new();
        dbnum_files.insert(
            7320u32,
            (
                "CATA".to_string(),
                "AvevaSample".to_string(),
                PathBuf::from("/p/cata_7320"),
            ),
        );
        dbnum_files.insert(
            8001u32,
            (
                "DESI".to_string(),
                "AvevaSample".to_string(),
                PathBuf::from("/p/desi_8001"),
            ),
        );
        InMemoryCataLocator::from_parts(ref0_to_dbnum, dbnum_files)
    }

    #[test]
    fn dbnum_of_ref0_maps_multiple_ref0s_to_one_dbnum() {
        let loc = locator();
        assert_eq!(loc.dbnum_of_ref0(100), Some(7320));
        assert_eq!(loc.dbnum_of_ref0(2013286676), Some(7320));
        assert_eq!(loc.dbnum_of_ref0(200), Some(8001));
        assert_eq!(loc.dbnum_of_ref0(999), None);
    }

    #[test]
    fn db_type_and_file_resolve_by_dbnum() {
        let loc = locator();
        assert_eq!(loc.db_type_of(7320).as_deref(), Some("CATA"));
        assert_eq!(loc.db_type_of(8001).as_deref(), Some("DESI"));
        assert_eq!(loc.db_type_of(1).as_deref(), None);

        let (project, path) = loc.file_of(7320).expect("file");
        assert_eq!(project, "AvevaSample");
        assert_eq!(path, PathBuf::from("/p/cata_7320"));
        assert!(loc.file_of(1).is_none());
    }

    #[test]
    fn counts_report_sizes() {
        let loc = locator();
        assert_eq!(loc.ref0_count(), 3);
        assert_eq!(loc.dbnum_count(), 2);
    }

    #[test]
    fn is_valid_ref0_rejects_zero_and_sentinel() {
        assert!(!is_valid_ref0(0));
        assert!(!is_valid_ref0(INVALID_REF0_SENTINEL));
        assert!(is_valid_ref0(100));
        assert!(is_valid_ref0(2013286676));
    }

    #[test]
    fn precise_config_limits_container_subtree_and_adds_desi() {
        let cfg = CataClosureConfig::precise();
        assert!(cfg.cata_db_types.contains("CATA"));
        assert!(cfg.cata_db_types.contains("DESI"));
        let allow = cfg.container_subtree_nouns.expect("precise sets allowlist");
        assert!(allow.contains("GMSE"));
        assert!(allow.contains("GMSS"));
        assert!(!allow.contains("SPEC"));
    }

    #[test]
    fn dep_cache_roundtrip_and_sesno_invalidation() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!("cata_dep_test_{unique}.bin"));
        unsafe {
            std::env::set_var("AIOS_CATA_DEP_CACHE_PATH", &tmp);
        }

        let mut c = CataDepCache::default();
        let root = RefU64(123);
        c.put(8000, root, 42, vec![1, 2, 3]);
        c.save("proj");

        let loaded = CataDepCache::load("proj");
        assert_eq!(loaded.get_fresh(8000, root, 42), Some(&vec![1, 2, 3]));
        assert_eq!(loaded.get_fresh(8000, root, 43), None); // source_sesno 变 → 失效
        assert_eq!(loaded.get_fresh(9999, root, 42), None); // 未知源库

        std::fs::remove_file(&tmp).ok();
        unsafe {
            std::env::remove_var("AIOS_CATA_DEP_CACHE_PATH");
        }
    }
}
