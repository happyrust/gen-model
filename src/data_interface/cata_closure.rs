//! 元件库（CATA）按需解析 — 移植自 `../../plant-model-gen` 的 refno 级引用闭包。
//!
//! 设计见 `docs/adr/ADR-004-on-demand-cata-parsing-port.md`、
//! `docs/plans/on-demand-cata-parsing-port.md`。
//!
//! 已落地：
//! - **Phase 1**：ref0→dbnum 定位器（就地内存实现，Q2=B），复用 `dbnum_watermark`
//!   + 文件 `ref0` 扫描（磁盘指纹缓存），不引 sqlite。
//! - **Phase 2**：`parse_db_refnos`（by-refno 部分解析）+ `CataClosureResolver`
//!   （refno 级 BFS 引用闭包：全出向 `RefU64` + owner 链 + 容器子树，`db_type` 收口 CATA）。
//!   它今天的生产调用方是 `cata_dependency`（ADR-064 D5：每批 DESI 写回前按目录链属性播种，
//!   `precise()` 容器白名单，只收 CATA）。
//!
//! Phase 3 的惰性兜底 `ensure_cata_refnos_parsed` 只剩 `staging::preload`（spec 035 P3 待拆）引用，
//! 落库也走 `cata_dependency`（`element_batch` `Fresh`、`pe.dependency = true`）。**2026-09-07 随
//! ADR-064 Q2 一并退役、已删除**：`cata_ref_rev` 补偿队列、根子树播种 `collect_design_subtree_outbound` /
//! `run_cata_closure_pass_for_refnos` / `ensure_cata_parsed_for_roots`、resolve 层 `try_lazy_cata_fallback`、
//! 依赖缓存 `CataDepCache` / `preload_cata_for_roots`、env 开关 `AIOS_CATA_CLOSURE_MODE`、
//! 开/关几何冒烟 `geo_smoke_digest`（`bin/cata_smoke`）。目录行进库只有 `cata_dependency` 一个出口。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use aios_core::SUL_DB;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64, RefnoEnum, get_db_option};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as TokioMutex;

use crate::data_interface::dbnum_state::WATERMARK_TABLE;
use crate::data_interface::increment_pipeline::wrap_in_transaction;
use crate::data_interface::on_demand_db::OnDemandDbSession;

/// B+树索引起始标记 / 无效 ref0（需跳过）。
const INVALID_REF0_SENTINEL: u32 = 0x8000_0001;

#[inline]
pub(crate) fn is_valid_ref0(ref0: u32) -> bool {
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
    /// Membership is independent of source-file availability.
    fn contains_dbnum(&self, dbnum: u32) -> bool { self.file_of(dbnum).is_some() }
    /// `ref0`（`RefU64::get_0()`）-> 所属 dbnum。
    fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32>;
    /// Resolve an affiliation without hiding an ambiguous Ref0 behind iteration order.
    fn resolve_ref0(&self, ref0: u32) -> anyhow::Result<Option<u32>> {
        Ok(self.dbnum_of_ref0(ref0))
    }
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
    ref0_conflicts: HashMap<u32, BTreeSet<u32>>,
    dbnum_files: HashMap<u32, DbFileEntry>,
}

fn insert_ref0_affiliation(
    resolved: &mut HashMap<u32, u32>,
    conflicts: &mut HashMap<u32, BTreeSet<u32>>,
    ref0: u32,
    dbnum: u32,
) {
    if let Some(set) = conflicts.get_mut(&ref0) {
        set.insert(dbnum);
        return;
    }
    match resolved.get(&ref0).copied() {
        None => {
            resolved.insert(ref0, dbnum);
        }
        Some(existing) if existing == dbnum => {}
        Some(existing) => {
            resolved.remove(&ref0);
            conflicts.insert(ref0, BTreeSet::from([existing, dbnum]));
        }
    }
}

/// 完整候选扫描经过项目优先级裁决后的 CATA 文件。它只提供身份与定位，部分解析
/// 不因此取得推进 CATA `applied_sesno` 的资格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CataDependencyFile {
    pub project: String,
    pub dbnum: u32,
    pub path: PathBuf,
    pub fingerprint: String,
}

/// 完整头扫描得到的文件身份。它只用于把 Ref0 判定为 CATA / 非 CATA，
/// 不会让范围外 DESI 进入数据批次，也不会推进任何依赖库水位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyIdentityFile {
    pub project: String,
    pub dbnum: u32,
    pub db_type: String,
    pub path: PathBuf,
    pub fingerprint: String,
}

static DEPENDENCY_MANIFEST: Lazy<RwLock<Vec<CataDependencyFile>>> =
    Lazy::new(|| RwLock::new(Vec::new()));
static DEPENDENCY_IDENTITY_MANIFEST: Lazy<RwLock<Vec<DependencyIdentityFile>>> =
    Lazy::new(|| RwLock::new(Vec::new()));
static DEPENDENCY_MANIFEST_VERSION: AtomicU64 = AtomicU64::new(0);

pub fn install_dependency_manifest(
    entries: impl IntoIterator<Item = (String, u32, PathBuf)>,
) -> u64 {
    let mut next = entries
        .into_iter()
        .map(|(project, dbnum, path)| CataDependencyFile {
            project,
            dbnum,
            fingerprint: locator_fingerprint(&path),
            path,
        })
        .collect::<Vec<_>>();
    next.sort_by(|left, right| {
        (left.dbnum, &left.project, &left.path).cmp(&(right.dbnum, &right.project, &right.path))
    });
    let mut manifest = DEPENDENCY_MANIFEST
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *manifest != next {
        *manifest = next;
        DEPENDENCY_MANIFEST_VERSION.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        DEPENDENCY_MANIFEST_VERSION.load(Ordering::SeqCst)
    }
}

pub fn dependency_manifest_snapshot() -> Vec<CataDependencyFile> {
    DEPENDENCY_MANIFEST
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub fn install_dependency_identity_manifest(
    entries: impl IntoIterator<Item = (String, u32, String, PathBuf)>,
) -> u64 {
    let mut next = entries
        .into_iter()
        .map(|(project, dbnum, db_type, path)| DependencyIdentityFile {
            project,
            dbnum,
            db_type,
            fingerprint: locator_fingerprint(&path),
            path,
        })
        .collect::<Vec<_>>();
    next.sort_by(|left, right| {
        (left.dbnum, &left.project, &left.db_type, &left.path).cmp(&(
            right.dbnum,
            &right.project,
            &right.db_type,
            &right.path,
        ))
    });
    let mut manifest = DEPENDENCY_IDENTITY_MANIFEST
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *manifest != next {
        *manifest = next;
        DEPENDENCY_MANIFEST_VERSION.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        DEPENDENCY_MANIFEST_VERSION.load(Ordering::SeqCst)
    }
}

pub fn dependency_identity_manifest_snapshot() -> Vec<DependencyIdentityFile> {
    DEPENDENCY_IDENTITY_MANIFEST
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

static LOCATOR_CACHE: Lazy<TokioMutex<HashMap<String, (u64, InMemoryCataLocator)>>> =
    Lazy::new(|| TokioMutex::new(HashMap::new()));

/// 文件身份门控改变后丢弃项目 locator；下一次按当前 watermark 身份重建。
pub(crate) async fn invalidate_project_locator(project: &str) {
    LOCATOR_CACHE.lock().await.remove(project);
}

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
            ref0_conflicts: HashMap::new(),
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
    /// `project`：工程名（legacy/compare 回读语义需要 + `file_of` 返回）。
    pub async fn build_for_project(project: &str) -> anyhow::Result<Self> {
        let manifest_version = DEPENDENCY_MANIFEST_VERSION.load(Ordering::SeqCst);
        if let Some((version, locator)) = LOCATOR_CACHE.lock().await.get(project).cloned()
            && version == manifest_version
        {
            return Ok(locator);
        }

        let mut dbnum_files = load_dbnum_files_from_watermark(project).await?;
        let manifest = dependency_manifest_snapshot();
        let identity_manifest = dependency_identity_manifest_snapshot();
        for entry in &identity_manifest {
            dbnum_files.insert(
                entry.dbnum,
                DbFileEntry {
                    db_type: entry.db_type.clone(),
                    project: entry.project.clone(),
                    path: entry.path.clone(),
                },
            );
        }
        crate::data_interface::batch_worker::note_dependency_progress(
            "dependency_index",
            None,
            None,
            manifest.len() as u64,
            0,
            0,
        );
        if crate::data_interface::watch_scope::active() {
            if manifest.is_empty() {
                anyhow::bail!("监听限定模式缺少经优先级裁决的 CATA 依赖清单");
            }
            // 限定模式只索引被监听 DESI 与权威 CATA 清单；旧水位表里其它几百个
            // DESI 不是本次依赖定位输入，扫描它们正是 8000 长时间无进展的主因。
            // 身份表保留完整：闭包必须能区分范围外 DESI/PROP 与真正缺失的 CATA。
            // 只缩小“需要建 Ref0 索引”的集合，不能删掉类型与路径身份。
        }

        // ref0 扫描（磁盘指纹缓存：文件未变则复用上次结果）。
        let mut cache = Ref0IndexCache::load(project);
        let mut ref0_to_dbnum: HashMap<u32, u32> = HashMap::new();
        let mut ref0_conflicts: HashMap<u32, BTreeSet<u32>> = HashMap::new();
        let mut dirty = false;

        let selected_cata = manifest
            .iter()
            .map(|entry| entry.dbnum)
            .collect::<HashSet<_>>();
        // The selected CATA manifest is the authoritative identity when a CATA
        // dbnum collides with a DESI of the same number in another project.
        // This overlay must also run with an unrestricted watcher; otherwise the
        // watermark/identity map can leave the DESI path selected for closure reads.
        for entry in &manifest {
            dbnum_files.insert(
                entry.dbnum,
                DbFileEntry {
                    db_type: "CATA".to_string(),
                    project: entry.project.clone(),
                    path: entry.path.clone(),
                },
            );
        }
        let index_candidates = dbnum_files
            .iter()
            .filter(|(dbnum, entry)| {
                !crate::data_interface::watch_scope::active()
                    || should_eager_index_identity(**dbnum, &entry.db_type, &selected_cata)
            })
            .map(|(dbnum, entry)| (*dbnum, entry.clone()))
            .collect::<Vec<_>>();

        for (dbnum, entry) in &index_candidates {
            crate::data_interface::batch_worker::note_dependency_location(
                "dependency_index",
                Some(*dbnum),
                Some(entry.path.display().to_string()),
                index_candidates.len() as u64,
            );
            let fp = locator_fingerprint(&entry.path);
            let ref0s = if let Some(cached) = cache.get_if_fresh(*dbnum, &fp) {
                cached.clone()
            } else {
                let scanned = scan_identity_ref0s(&entry.path, &entry.db_type, &entry.project)?;
                cache.put(*dbnum, fp, scanned.clone());
                dirty = true;
                scanned
            };
            for ref0 in ref0s {
                insert_ref0_affiliation(&mut ref0_to_dbnum, &mut ref0_conflicts, ref0, *dbnum);
            }
            crate::data_interface::batch_worker::note_dependency_progress(
                "dependency_index",
                Some(*dbnum),
                Some(entry.path.display().to_string()),
                index_candidates.len() as u64,
                ref0_to_dbnum.len() as u64,
                0,
            );
        }

        // 范围外 DESI 不主动扫盘；若已有同指纹缓存，则复用它完成“非 CATA”判定。
        for (dbnum, entry) in &dbnum_files {
            if index_candidates
                .iter()
                .any(|(candidate, _)| candidate == dbnum)
            {
                continue;
            }
            let fp = locator_fingerprint(&entry.path);
            let Some(ref0s) = cache.get_if_fresh(*dbnum, &fp) else {
                continue;
            };
            for ref0 in ref0s {
                insert_ref0_affiliation(&mut ref0_to_dbnum, &mut ref0_conflicts, *ref0, *dbnum);
            }
        }

        if dirty {
            cache.save(project);
        }

        // 扫库是同步的，攒下的扫描失败在这里落库（见 `scan_db_ref0s`）。
        if let Err(error) = crate::data_interface::parse_error::flush().await {
            log::warn!("{error:#}");
        }

        let mut locator = Self::from_parts(
            ref0_to_dbnum,
            dbnum_files
                .into_iter()
                .map(|(dbnum, e)| (dbnum, (e.db_type, e.project, e.path)))
                .collect(),
        );
        locator.ref0_conflicts = ref0_conflicts;

        // Outside the watcher lifecycle there is no installed priority manifest.
        // Watermarks may still contain a handful of stale/foreign CATA identities;
        // treating their mere presence as a complete dependency set selected the
        // wrong same-dbnum file (AMS8000 ref0 23984 lives in ZDJ/zdj7600, not
        // AvevaPlantSample/aps7600).  In standalone Python/HTTP generation, scan
        // every configured dependency project and merge the deterministic CATA
        // selection.  Watcher mode keeps using its authoritative manifest.
        if manifest.is_empty() {
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
                let discovered = Self::build_cata_from_dir(&catalogue_project, &catalogue_dir)?;
                locator.merge_cata_files(discovered);
            }
        }

        LOCATOR_CACHE
            .lock()
            .await
            .insert(project.to_string(), (manifest_version, locator.clone()));
        Ok(locator)
    }

    pub(crate) fn merge_cata_files(&mut self, other: Self) {
        let cata_dbnums: HashSet<u32> = other
            .dbnum_files
            .iter()
            .filter_map(|(&dbnum, entry)| {
                entry.db_type.eq_ignore_ascii_case("CATA").then_some(dbnum)
            })
            .collect();
        for (ref0, dbnum) in other.ref0_to_dbnum {
            if cata_dbnums.contains(&dbnum) {
                insert_ref0_affiliation(
                    &mut self.ref0_to_dbnum,
                    &mut self.ref0_conflicts,
                    ref0,
                    dbnum,
                );
            }
        }
        for (ref0, dbnums) in other.ref0_conflicts {
            for dbnum in dbnums {
                if cata_dbnums.contains(&dbnum) {
                    insert_ref0_affiliation(
                        &mut self.ref0_to_dbnum,
                        &mut self.ref0_conflicts,
                        ref0,
                        dbnum,
                    );
                }
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
        Self::build_from_dir_matching(project, root_dir, |_| true, None).unwrap_or_else(|error| {
            log::warn!(
                "[cata_locator] best-effort directory scan incomplete: root={} error={error:#}",
                root_dir.display()
            );
            Self::default()
        })
    }

    fn build_cata_from_dir(project: &str, root_dir: &Path) -> anyhow::Result<Self> {
        let mut cache = Ref0IndexCache::load(project);
        let locator = Self::build_from_dir_matching(
            project,
            root_dir,
            |db_type| db_type.eq_ignore_ascii_case("CATA"),
            Some(&mut cache),
        )?;
        cache.save(project);
        Ok(locator)
    }

    fn build_from_dir_matching(
        project: &str,
        root_dir: &Path,
        include_type: impl Fn(&str) -> bool,
        mut cache: Option<&mut Ref0IndexCache>,
    ) -> anyhow::Result<Self> {
        let mut discovered = Vec::new();
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
            discovered.push((header, path.to_path_buf()));
        }
        let collapsed = crate::data_interface::extract_family::collapse_extract_families(
            discovered
                .iter()
                .map(|(header, path)| (project.to_string(), header.db_no, path.clone())),
        );
        if !collapsed.duplicate_keys.is_empty() {
            log::warn!(
                "[cata_locator] skip sibling extract duplicates: {:?}",
                collapsed.duplicate_keys
            );
        }
        let by_path: HashMap<_, _> = discovered
            .into_iter()
            .map(|(header, path)| (path, header))
            .collect();
        let mut ref0_to_dbnum: HashMap<u32, u32> = HashMap::new();
        let mut ref0_conflicts: HashMap<u32, BTreeSet<u32>> = HashMap::new();
        let mut dbnum_files: HashMap<u32, (String, String, PathBuf)> = HashMap::new();
        for sel in collapsed.selected {
            let Some(header) = by_path.get(&sel.leaf_path) else {
                continue;
            };
            let dbnum = header.db_no;
            let fingerprint = locator_fingerprint(&sel.leaf_path);
            let ref0s = match cache.as_deref_mut() {
                Some(cache) => match cache.get_if_fresh(dbnum, &fingerprint) {
                    Some(ref0s) => ref0s.clone(),
                    None => {
                        let ref0s = scan_db_ref0s(&sel.leaf_path, project)?;
                        cache.put(dbnum, fingerprint, ref0s.clone());
                        ref0s
                    }
                },
                None => scan_db_ref0s(&sel.leaf_path, project)?,
            };
            for ref0 in ref0s {
                insert_ref0_affiliation(&mut ref0_to_dbnum, &mut ref0_conflicts, ref0, dbnum);
            }
            dbnum_files.entry(dbnum).or_insert((
                header.db_type.clone(),
                project.to_string(),
                sel.leaf_path,
            ));
        }
        let mut locator = Self::from_parts(ref0_to_dbnum, dbnum_files);
        locator.ref0_conflicts = ref0_conflicts;
        Ok(locator)
    }
}

impl CataDbLocator for InMemoryCataLocator {
    fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32> {
        self.ref0_to_dbnum.get(&ref0).copied()
    }

    fn resolve_ref0(&self, ref0: u32) -> anyhow::Result<Option<u32>> {
        if let Some(dbnums) = self.ref0_conflicts.get(&ref0) {
            anyhow::bail!("Ref0 affiliation conflict: ref0={ref0} dbnums={dbnums:?}");
        }
        Ok(self.dbnum_of_ref0(ref0))
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
    // 读失败必须上浮，不得吞成空表（2026-08-13 审计 P2）：老写法
    // `take(0).unwrap_or_default()` 把语句/解码失败静默降级成「一个登记行都没有」，
    // 整轮闭包的每个 ref0 都会被解析成 missing——一次读表失败被伪装成「登记缺失」，
    // 排障方向从持久层故障被带偏到扫描/登记链路上。
    let mut response = SUL_DB
        .query(sql)
        .await
        .map_err(|e| anyhow::anyhow!("读取 dbnum_watermark 文件身份失败（CATA 定位器）: {e}"))?
        .check()
        .map_err(|e| {
            anyhow::anyhow!("读取 dbnum_watermark 文件身份语句失败（CATA 定位器）: {e}")
        })?;
    let rows: Vec<WatermarkFileRow> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码 dbnum_watermark 文件身份失败（CATA 定位器）: {e}"))?;

    let mut out = HashMap::new();
    for row in rows {
        let (Some(dbnum), Some(file_path)) = (row.dbnum, row.file_path) else {
            continue;
        };
        let db_type = row.db_type.unwrap_or_default();
        // 空路径是 Duplicate 身份阻断的无 schema tombstone：保留 CATA 类型可防止
        // 后面的目录 fallback 在歧义尚未解除时又自行扫描并任选一份文件。
        if file_path.is_empty() && !db_type.eq_ignore_ascii_case("CATA") {
            continue;
        }
        out.insert(
            dbnum,
            DbFileEntry {
                db_type,
                project: project.to_string(),
                path: PathBuf::from(file_path),
            },
        );
    }
    Ok(out)
}

fn should_eager_index_identity(dbnum: u32, db_type: &str, selected_cata: &HashSet<u32>) -> bool {
    if db_type.eq_ignore_ascii_case("CATA") {
        return selected_cata.contains(&dbnum);
    }
    if db_type.eq_ignore_ascii_case("DESI") {
        return crate::data_interface::watch_scope::admits(dbnum);
    }
    // PROP 只建立 Ref0 身份索引，不解析属性、不进入批次。其它元数据文件没有
    // CATA 几何依赖归属，避免为数百个 SYSTEM/MISC 文件做无意义的全索引。
    db_type.eq_ignore_ascii_case("PROP")
}

fn scan_identity_ref0s(path: &Path, _db_type: &str, project: &str) -> anyhow::Result<Vec<u32>> {
    // Every dependency identity comes from the selected e3d-io live index.
    // A whole-file fallback would also see obsolete copy-on-write index pages.
    scan_db_ref0s(path, project)
}

/// 扫描单个 db 文件的 `ref0` 集（供 `ref0→dbnum` 反查）。
///
/// paged 模式流式遍历快照索引键并只保留去重后的 `ref0`；legacy/compare 保留旧
/// `children_map` 基线作灰度与回滚。失败或成功空扫描均上浮（该库暂不可定位，且不发布缓存）。
/// 失败只攒进解析错误账本、不落库：本函数是同步的，而三个调用点里有一个也在
/// 同步路径上。落库交给 [`CataLocator::build_for_project`] 那个 async 收口点。
fn scan_db_ref0s(path: &Path, project: &str) -> anyhow::Result<Vec<u32>> {
    match crate::data_interface::on_demand_db::scan_ref0s(path, project) {
        Ok(ref0s) if !ref0s.is_empty() => {
            crate::data_interface::parse_error::note_file_success(path);
            Ok(ref0s)
        }
        Ok(_) => {
            let error = anyhow::anyhow!(
                "CATA Ref0 权威扫描返回空集合，拒绝发布空缓存: {}",
                path.display()
            );
            crate::data_interface::parse_error::note_file_failure(path, &format!("{error:#}"));
            Err(error)
        }
        Err(error) => {
            log::warn!(
                "[paged_db] locator_scan_failed path={} error={error:#}",
                path.display()
            );
            crate::data_interface::parse_error::note_file_failure(path, &format!("{error:#}"));
            Err(error)
        }
    }
}

fn scan_db_header(path: &Path) -> Option<crate::data_interface::db_file_header::DbBasicInfo> {
    // 库类型取 `+0x20` 的类型字（`DbDescriptor::db_type_hash`），与另外三条读头路径
    // （自动重扫 / 手动候选扫描 / CATA 候选枚举）同一口径。此前这里反哈希的是
    // `descriptor().noun`——e3d-io 现版里那是 `+0x04` 第 0 页自身的 noun 字，不是库类型。
    let info = crate::data_interface::db_file_header::DbBasicInfo::read(path)?;
    (info.db_no != 0 && !info.db_type.is_empty()).then_some(info)
}

fn locator_fingerprint(path: &Path) -> String {
    let leaf = file_fingerprint(path);
    match crate::data_interface::extract_family::parent_path_of(path)
        .filter(|parent| parent.is_file())
    {
        Some(parent) => format!("{leaf}|{}", file_fingerprint(&parent)),
        None => leaf,
    }
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
const REF0_INDEX_CACHE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Ref0IndexCache {
    #[serde(default)]
    version: u32,
    by_dbnum: HashMap<u32, Ref0CacheEntry>,
}

impl Default for Ref0IndexCache {
    fn default() -> Self {
        Self {
            version: REF0_INDEX_CACHE_VERSION,
            by_dbnum: HashMap::new(),
        }
    }
}

impl Ref0IndexCache {
    fn cache_path(project: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aios_cata_locator_{project}.json"))
    }

    fn load(project: &str) -> Self {
        std::fs::read_to_string(Self::cache_path(project))
            .ok()
            .and_then(|s| Self::decode(&s))
            .unwrap_or_default()
    }

    fn decode(json: &str) -> Option<Self> {
        serde_json::from_str::<Self>(json)
            .ok()
            .filter(|cache| cache.version == REF0_INDEX_CACHE_VERSION)
    }

    fn get_if_fresh(&self, dbnum: u32, fingerprint: &str) -> Option<&Vec<u32>> {
        self.by_dbnum.get(&dbnum).and_then(|e| {
            (!fingerprint.is_empty() && e.fingerprint == fingerprint && !e.ref0s.is_empty())
                .then_some(&e.ref0s)
        })
    }

    fn put(&mut self, dbnum: u32, fingerprint: String, ref0s: Vec<u32>) {
        if ref0s.is_empty() {
            self.by_dbnum.remove(&dbnum);
            return;
        }
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
fn open_db_session(_project: &str, path: &Path) -> anyhow::Result<OnDemandDbSession> {
    OnDemandDbSession::open(path)
}

/// 成员表收敛成可落库的形态：滤掉无效 ref0，同一 child 只保留首个槽位。
///
/// `unique_pe_owner` 建在 `(in, out)` 上，同一 owner 下同一个 child 占两个槽位在库里
/// 根本无法表示，整块重插必撞唯一索引。这里就地收敛到唯一可表示的终态并告警定位到
/// 元素，而不是中断整批解析：本函数同时服务 CATA 闭包与 DESI 设计子树遍历，且几行
/// 之下的**解析失败**也只是跳过按 cache-miss 处理——一条成员表毛刺没有理由把整个
/// 生成单元推进死信。
pub(crate) fn dedupe_members(refno: RefU64, raw: &[RefU64]) -> Vec<RefU64> {
    let mut seen = HashSet::with_capacity(raw.len());
    let mut children = Vec::with_capacity(raw.len());
    for &child in raw {
        if !is_valid_ref0(child.get_0()) {
            continue;
        }
        if seen.insert(child) {
            children.push(child);
        } else {
            log::warn!(
                "[cata_closure] 元素 {} 的成员表重复列出 {}，只保留首个槽位",
                refno.to_pe_key(),
                child.to_pe_key()
            );
        }
    }
    children
}

/// 用已打开会话解析一批 refno（不重读文件 / 不重建索引）。
///
/// `attmap_sink`：可选保留完整属性表（Phase 3 惰性兜底落库需要；闭包发现 pass 传 `None` 省内存）。
async fn parse_refnos_with_session(
    session: &mut OnDemandDbSession,
    refnos: &[RefU64],
    mut attmap_sink: Option<&mut HashMap<RefU64, (NamedAttrMap, Vec<RefU64>)>>,
) -> anyhow::Result<HashMap<RefU64, ParsedCataEle>> {
    let mut out = HashMap::with_capacity(refnos.len());
    for &refno in refnos {
        match session.parse_element(refno).await {
            Ok(Some(ele)) => {
                let merged = ele.att;
                let outbound = outbound_refs_of(&merged);
                let children = dedupe_members(refno, &ele.children);
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
                crate::data_interface::parse_error::note_element_success(&refno.to_pe_key());
            }
            Ok(None) => {}
            Err(error) => {
                if session.is_compare() {
                    return Err(error);
                }
                // 解析失败：跳过，由调用方按 cache-miss 处理。日志之外还要留痕——
                // 跳过是静默降级，事后没有任何查询能说出「哪些元素解析不出来」。
                let target = refno.to_pe_key();
                log::warn!("[paged_db] element_parse_skipped refno={target} error={error:#}");
                crate::data_interface::parse_error::note_element_failure(
                    &target,
                    &format!("{error:#}"),
                );
            }
        }
    }
    // 攒在进程内的这一批在这里落库：本函数是逐元素循环最近的 async 收口点，
    // 循环里逐条 UPSERT 会把「一个坏文件」变成「解析卡死」。
    if let Err(error) = crate::data_interface::parse_error::flush().await {
        log::warn!("{error:#}");
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
    let mut session = open_db_session(project, path)?;
    parse_refnos_with_session(&mut session, refnos, None).await
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
    /// （GMSE/GMSS/NGMS/PTSE/PTSS/SPRO/DTSE），不展开 SPEC/SELE 等规格容器。
    ///
    /// 挤出 / 回转体的轮廓是三层：`SEXT|NSEX|SREV|NSRE → SLOO → 顶点`。名单停在
    /// 几何集这一层时闭包仍报 `missing=0`，落库的却是没有顶点的空壳几何。
    ///
    /// 2026-09-07 对拍 oracle（e3d-model `catalogue_visited`，ams8000 全库）抓到两处漏、一并补进名单：
    /// - **正体名下的负体**：Core3D `CSG_TreeBuilderCat::getCSGTree` 对每一个有成员的正体都开差集，
    ///   e3d-model 照做（`collect_owned_negatives` 读正体的 `members()`），所以 SBOX / SCYL / LCYL / SCON /
    ///   LSNO / LPYR / SCTO / SRTO / SSPH / SSLC / SDSH / SDIS 名下的 NSBO / NLPY / NSRT / NLCY … 都是实读；
    ///   此前只展开 SEXT / SREV，7002 库里 SBOX 名下 8 只 NSBO、LPYR 名下 3 只 NLPY 就落不进库。
    /// - **p-line 集**：型材对齐读 `SPRF.PSTR → PTSS → PLIN`（`section.rs`），容器 noun 是 `PTSS`；
    ///   此前名单里写的是属性名 `PSTR`（没有这个 noun），PTSS 的 8 只 PLIN 一个都没展开。
    pub fn precise() -> Self {
        let container: HashSet<String> = [
            // 几何集 / 点集 / p-line 集 / 数据集 / 二维轮廓
            "GMSE", "GMSS", "NGMS", "PTSE", "PTSS", "SPRO", "DTSE",
            // 轮廓类正体与负体：SLOO 是它们的剖面成员
            "SEXT", "NSEX", "SREV", "NSRE", "SLOO",
            // 其余正体：名下的成员就是它的刀（负体）
            "SBOX", "SCYL", "LCYL", "SCON", "LSNO", "LPYR", "SCTO", "SRTO", "SSPH", "SSLC", "SDSH",
            "SDIS",
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
    /// 缺失样例，供现场区分定位失败、清单缺失和选中文件内不存在。
    pub missing_samples: Vec<String>,
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
    sessions: HashMap<u32, OnDemandDbSession>,
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
        let mut missing_samples = Vec::new();
        let mut rounds = 0usize;
        // 缺 db_type 的登记行每库只告警一次（frontier 里同库引用成百上千）。
        let mut warned_untyped: HashSet<u32> = HashSet::new();

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
                let Some(dbnum) = self.locator.resolve_ref0(ref0)? else {
                    missing += 1;
                    if missing_samples.len() < 32 {
                        missing_samples.push(format!("{r:?}: no dbnum mapping"));
                    }
                    continue;
                };
                if excluded_dbnums.contains(&dbnum) {
                    continue;
                }
                // 登记行缺 db_type（或为空）时判不出是不是 CATA。老写法吞成空串
                // 被当「非 CATA」静默跳过（2026-08-13 审计 P3）；现在按 missing
                // 记账并每库告警一次，与「无 dbnum 映射」走同一条出路，闭包
                // manifest 的 missing 计数上看得见。
                let db_type = match self.locator.db_type_of(dbnum) {
                    Some(db_type) if !db_type.trim().is_empty() => db_type,
                    _ => {
                        if warned_untyped.insert(dbnum) {
                            log::warn!(
                                "[cata_closure] dbnum={dbnum} 的登记行缺 db_type，\
                                 无法判定是否 CATA，其引用计入 missing（登记补全后自愈）"
                            );
                        }
                        missing += 1;
                        if missing_samples.len() < 32 {
                            missing_samples.push(format!("{r:?}: dbnum={dbnum} has no db_type"));
                        }
                        continue;
                    }
                };
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

                crate::data_interface::batch_worker::note_dependency_location(
                    "dependency_closure",
                    Some(dbnum),
                    self.locator
                        .file_of(dbnum)
                        .map(|(_, path)| path.display().to_string()),
                    seed_count as u64,
                );

                // 确保该库会话已打开（一次性 open + 建索引；后续轮复用）。
                if !self.sessions.contains_key(&dbnum) {
                    let Some((project, path)) = self.locator.file_of(dbnum) else {
                        missing += to_parse.len();
                        for r in &to_parse {
                            if missing_samples.len() < 32 {
                                missing_samples
                                    .push(format!("{r:?}: dbnum={dbnum} has no selected file"));
                            }
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
                                if missing_samples.len() < 32 {
                                    missing_samples.push(format!(
                                        "{r:?}: dbnum={dbnum} selected file open failed: {e}"
                                    ));
                                }
                                self.visited.insert(*r);
                            }
                            continue;
                        }
                    }
                }

                let parsed = {
                    let session = self.sessions.get_mut(&dbnum).expect("session just ensured");
                    let attmap_sink = if self.retain_attmaps {
                        Some(&mut self.attmaps)
                    } else {
                        None
                    };
                    parse_refnos_with_session(session, &to_parse, attmap_sink).await?
                };
                crate::data_interface::batch_worker::note_dependency_progress(
                    "dependency_closure",
                    Some(dbnum),
                    self.locator
                        .file_of(dbnum)
                        .map(|(_, path)| path.display().to_string()),
                    seed_count as u64,
                    parsed.len() as u64,
                    missing as u64,
                );

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
                            if missing_samples.len() < 32 {
                                let path = self
                                    .locator
                                    .file_of(dbnum)
                                    .map(|(_, path)| path.display().to_string())
                                    .unwrap_or_else(|| "<none>".to_string());
                                missing_samples
                                    .push(format!("{r:?}: dbnum={dbnum} not found in {path}"));
                            }
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

        if missing > 0 {
            eprintln!("[cata_closure] 闭包缺失明细: missing={missing} samples={missing_samples:?}");
        }
        Ok(CataClosureManifest {
            by_dbnum,
            seed_count,
            visited_count: self.visited.len(),
            rounds,
            missing,
            missing_samples,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3：运行期惰性兜底（命中未解析 CATA refno → 小闭包 → 落 pe/ATT_* → 重试）
// ─────────────────────────────────────────────────────────────────────────────

/// 替换一个 owner 完整成员块的两条语句：先按复合 id 的 owner 前缀范围删掉旧块，
/// 再整块重插。
///
/// 前缀范围直走 record range，不经过 `WHERE out = owner` 的全表扫。两条语句必须落在
/// 同一个事务里才收敛（换槽、缩短、失败重放都靠这一点）。生产落库已改走 `element_batch`
/// （同一条「owner 前缀范围删 + 整块重插」纪律）；这里保留给 `fork_surreal_compat` 双引擎对拍
/// 与本模块的语义测试当标准形态。空 children 也必须发删除，才能清掉成员表缩短到 0 后留下的幽灵边。
///
/// 重复 child 在这里是硬错：解析路径已由 [`dedupe_members`] 收敛过，能走到这里说明
/// 调用方绕开了那道收敛，宁可这一个 owner 带着 owner/child 报错，也不要放一段必然
/// 撞 `unique_pe_owner` 的 SQL 进 journal。
fn render_pe_owner_statements(op: &str, children: &[RefU64]) -> anyhow::Result<Vec<String>> {
    let mut seen = HashSet::with_capacity(children.len());
    let mut inserts = Vec::with_capacity(children.len());
    for (slot, child) in children.iter().enumerate() {
        if !seen.insert(child) {
            anyhow::bail!(
                "owner {} 的成员块重复列出 child {}，与 unique_pe_owner 冲突",
                op,
                child.to_pe_key()
            );
        }
        let cp = child.to_pe_key();
        inserts.push(format!(
            "{{ id: pe_owner:[{op}, {slot}], in: {cp}, out: {op} }}"
        ));
    }

    let mut statements = vec![format!("DELETE pe_owner:[{op}, NONE]..=[{op}, ..]")];
    if !inserts.is_empty() {
        statements.push(format!(
            "INSERT RELATION INTO pe_owner [{}]",
            inserts.join(",")
        ));
    }
    Ok(statements)
}

/// 单个 owner 自成一个事务。原子替换的最小单元，也是 ReplaySafe 断言与 fork 对拍
/// 用的标准形态。
pub(crate) fn render_pe_owner_replace(op: &str, children: &[RefU64]) -> anyhow::Result<String> {
    let statements = render_pe_owner_statements(op, children)?;
    Ok(wrap_in_transaction(&statements).expect("owner 替换至少有一条删除语句"))
}

/// 惰性兜底全局互斥：并发 miss 串行化，避免重复解析同一批元素（落库 INSERT IGNORE 幂等）。
static LAZY_CATA_FALLBACK_LOCK: Lazy<TokioMutex<()>> = Lazy::new(|| TokioMutex::new(()));

/// 惰性兜底结果统计。
#[derive(Debug, Default, Clone)]
pub struct LazyFallbackOutcome {
    pub parsed: usize,
    pub missing: usize,
}

/// 运行期惰性兜底：对未解析的 CATA refno 跑**小闭包**并即时落库
/// （`pe` + `<NOUN>` + `ATT_UDA` + `pe_owner`），使随后 `get_named_attmap` 透明命中。
///
/// **落库走 `cata_dependency` 那一条路**（ADR-064 Q2，2026-09-07）：同一个闭包配置（`precise()`
/// 容器白名单、db_type 只收 CATA）、同一个渲染器（`element_batch` `Fresh`，`pe.dependency = true`）、
/// 同一份目录版本前移刷新（D7：先 `Replace` 再 `Fresh`）与 `cata_dependency_state`。此前这里
/// 逐元素 `UPSERT … CONTENT`：对拍 oracle 修好 `on_demand_db` 的库类型字之后，这条路在真库上第一次
/// 真的写进了行——`CONTENT` 会把 P4 落的 `dependency = true` 整行抹掉（P5 前移刷新与 `/dbnums`
/// 从此认不出这些行），`precise()` 原样跑还会把回指到别的 DESI 库的元素一并写进去。
///
/// 同日 `cata_ref_rev` 补偿队列、resolve 层 `try_lazy_cata_fallback`、根子树预解析
/// `ensure_cata_parsed_for_roots` / `preload_cata_for_roots` 与依赖缓存 `CataDepCache` 一并退役，
/// 生产路径上没有人再走到这里；留下它只因 `staging::preload`（spec 035 P3 待拆）还引用。
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

    // 2. 小闭包 → 行（与三个写入点同一条闭包路，只收 CATA）。
    let seeds: BTreeSet<RefU64> = seeds.iter().copied().collect();
    let rows = super::cata_dependency::resolve_catalogue_rows_with(&locator, &seeds).await?;
    for (dbnum, elements) in &rows.by_dbnum {
        crate::data_interface::batch_worker::note_dependency_location(
            "dependency_write",
            Some(*dbnum),
            locator
                .file_of(*dbnum)
                .map(|(_, path)| path.display().to_string()),
            elements.len() as u64,
        );
    }

    // 3. 落库：目录行只有这一个出口（`Fresh` 命中即零成本；文件前移的先 Replace 刷新）。
    let report = super::cata_dependency::persist_resolved_rows(project, &rows).await?;
    crate::data_interface::batch_worker::note_dependency_progress(
        "dependency_write",
        None,
        None,
        rows.elements() as u64,
        report.elements as u64,
        report.missing as u64,
    );

    log::info!(
        "[cata_closure] 惰性兜底完成: seeds={} parsed={} missing={} rounds={} 目录库={} 事务={}",
        seeds.len(),
        report.elements,
        rows.manifest.missing,
        rows.manifest.rounds,
        report.dbnums.len(),
        report.transactions
    );
    Ok(LazyFallbackOutcome {
        parsed: report.elements,
        missing: rows.manifest.missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_scope_indexes_prop_identity_without_reopening_unwatched_design() {
        let selected_cata = HashSet::from([7600]);
        assert!(
            should_eager_index_identity(5200, "PROP", &selected_cata),
            "PROP Ref0 身份必须可用于把非 CATA 引用排除出必需闭包"
        );
        assert!(should_eager_index_identity(7600, "CATA", &selected_cata));
        assert!(!should_eager_index_identity(7355, "CATA", &selected_cata));
        assert!(!should_eager_index_identity(8191, "DICT", &selected_cata));
    }

    #[test]
    /// ADR-064 Q2：惰性兜底的落库只有 `cata_dependency` 这一条路——闭包配置与三个写入点同源
    /// （`resolve_catalogue_rows_with`），行经 `persist_resolved_rows`（`element_batch` `Fresh`、
    /// `pe.dependency = true`、目录前移先刷新）。不再有逐元素 `UPSERT … CONTENT`：它会把 P4 落的
    /// `dependency = true` 整行抹掉，P5 的前移刷新就再也认不出这些行。
    #[test]
    fn lazy_fallback_lands_rows_through_cata_dependency_and_renders_no_upsert() {
        let source = include_str!("cata_closure.rs");
        let body = source
            .split_once("pub async fn ensure_cata_refnos_parsed(")
            .expect("lazy fallback exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("lazy fallback boundary")
            .0;
        assert!(
            body.contains("cata_dependency::resolve_catalogue_rows_with("),
            "闭包要走 cata_dependency 的同一条路: {body}"
        );
        assert!(
            body.contains("cata_dependency::persist_resolved_rows("),
            "落库要走 cata_dependency 的唯一出口: {body}"
        );
        assert!(
            !body.contains("UPSERT") && !body.contains("execute_model_write"),
            "惰性兜底不得自己渲染 / 直写: {body}"
        );

        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for line in production.lines() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains("UPSERT "),
                "cata_closure.rs 生产代码不得渲染 UPSERT（元素数据行只有 element_batch 一个出口）: {line}"
            );
        }
    }

    #[test]
    fn locator_scan_failure_is_a_result_and_cannot_cache_an_empty_success() {
        let source = include_str!("cata_closure.rs");
        let build = source
            .split_once("pub async fn build_for_project")
            .expect("build_for_project")
            .1
            .split_once("pub fn build_from_dir")
            .expect("build boundary")
            .0;
        // 针要跟着调用点走：依赖身份清单那一版把这里从 `scan_db_ref0s(&entry.path,
        // project)?` 换成了 `scan_identity_ref0s(...)`，针没跟着改，于是 `unwrap()` 在
        // `None` 上炸开——这条守卫连着几周没有在守任何东西。用 `expect` 而不是
        // `unwrap`，下一次改名至少能读到「谁没跟上」。
        let scan_at = build
            .find("scan_identity_ref0s(&entry.path, &entry.db_type, &entry.project)?")
            .expect(
                "身份扫描的调用点必须留在 build_for_project 里，且失败要用 `?` 上浮；\
                 改了名就把这根针一起改",
            );
        let put_at = build
            .find("cache.put(*dbnum")
            .expect("写缓存的调用点必须留在 build_for_project 里");
        assert!(scan_at < put_at, "先成功扫描，后写缓存");

        let scan = source
            .split_once("fn scan_db_ref0s(")
            .expect("scan function")
            .1
            .split_once("fn scan_db_header")
            .expect("scan boundary")
            .0;
        assert!(scan.contains("anyhow::Result<Vec<u32>>"));
        assert!(scan.contains("Err(error)"));
        assert!(!scan.contains("Vec::new()"));
        assert!(
            scan.contains("Ok(ref0s) if !ref0s.is_empty()") && scan.contains("拒绝发布空缓存"),
            "成功空扫描也必须保持不可缓存并触发下次重试: {scan}"
        );
    }

    #[test]
    fn locator_stays_on_the_persistent_store() {
        let source = include_str!("cata_closure.rs");
        let locator = source
            .split_once("async fn load_dbnum_files_from_watermark(")
            .expect("watermark locator exists")
            .1
            .split_once("fn scan_db_ref0s(")
            .expect("watermark locator boundary")
            .0;
        // 定位器读的是持久层（登记契约在 dbnum_watermark），不许被路由进暂存。
        assert!(
            locator.contains("SUL_DB") && locator.contains(".query("),
            "{locator}"
        );
    }

    /// 定位器读 dbnum_watermark 失败必须上浮，不得吞成空表（2026-08-13 审计 P2）。
    ///
    /// 老写法把 take 的语句/解码失败降级成空定位器，整轮闭包的每个 ref0 都假
    /// missing——一次持久层读失败被伪装成「登记缺失」。
    #[test]
    fn a_failed_watermark_read_surfaces_instead_of_an_empty_locator() {
        let source = include_str!("cata_closure.rs");
        let body = source
            .split_once("async fn load_dbnum_files_from_watermark(")
            .expect("watermark locator exists")
            .1
            .split_once("fn scan_db_ref0s(")
            .expect("watermark locator boundary")
            .0;
        assert!(
            body.contains(".check()"),
            "读表必须 check() 上浮语句级错误: {body}"
        );
        let take_at = body.find(".take(0)").expect("必须有 take(0) 读行集");
        let rest = &body[take_at..];
        // 200 字节窗口，向后走到字符边界（周边是中文注释，硬切会劈开多字节字符）。
        let mut end = rest.len().min(200);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let window = &rest[..end];
        assert!(
            !window.contains("unwrap_or_default"),
            "take 失败不得吞成空表（空定位器 = 全部 ref0 假 missing）: {window}"
        );
        assert!(
            window.contains("map_err"),
            "take 失败必须带上下文上浮: {window}"
        );
    }

    /// 登记行缺 db_type 时不许被静默当「非 CATA」跳过（2026-08-13 审计 P3）：
    /// 判不出类型的库按 missing 记账，闭包 manifest 上看得见，而不是无声少解析。
    #[tokio::test]
    async fn an_untyped_dbnum_is_counted_missing_not_silently_skipped() {
        let locator = InMemoryCataLocator::from_parts(
            HashMap::from([(100u32, 7320u32)]),
            HashMap::from([(
                7320u32,
                (String::new(), "P".to_string(), PathBuf::from("unused")),
            )]),
        );
        let mut resolver = CataClosureResolver::new(&locator, CataClosureConfig::default());
        resolver.seed([RefU64((100u64 << 32) | 1)]);
        let manifest = resolver.resolve().await.expect("resolve");
        assert_eq!(manifest.missing, 1, "缺 db_type 的引用必须计入 missing");
        assert!(
            manifest.by_dbnum.is_empty(),
            "判不出类型的库不得被下探: {:?}",
            manifest.by_dbnum
        );
    }

    /// pe_owner owner 块替换契约：范围删除 + 完整重插必须是一个 ReplaySafe 事务。
    #[test]
    fn pe_owner_replace_is_atomic_and_owner_scoped() {
        let child_a = RefU64((24381u64 << 32) | 34109);
        let child_b = RefU64((24381u64 << 32) | 34110);
        let sql = render_pe_owner_replace("pe:16189_0", &[child_a, child_b])
            .expect("无重复的成员块必须能渲染");
        assert!(
            sql.contains("DELETE pe_owner:[pe:16189_0, NONE]..=[pe:16189_0, ..]"),
            "必须按 owner 的复合 id 前缀清掉完整旧块: {sql}"
        );
        assert!(
            sql.starts_with("BEGIN TRANSACTION;") && sql.ends_with("COMMIT TRANSACTION;"),
            "删除与重插必须共用一个显式事务: {sql}"
        );
        crate::data_interface::staging::replay_safe::validate_statement(&sql)
            .expect("owner 替换事务必须满足 ReplaySafe");
    }

    /// 成员表毛刺的处置分工：解析边界就地收敛（不中断整批），渲染边界兜底硬拒
    /// （挡住绕开收敛的调用方，且错误要能定位到 owner 与 child）。
    #[test]
    fn duplicate_members_converge_at_parse_and_are_refused_at_render() {
        let owner = RefU64((16189u64 << 32) | 0);
        let child_a = RefU64((24381u64 << 32) | 34109);
        let child_b = RefU64((24381u64 << 32) | 34110);

        assert_eq!(
            dedupe_members(owner, &[child_a, child_b, child_a]),
            vec![child_a, child_b],
            "重复成员只保留首个槽位，其余照旧解析下去"
        );
        let sentinel = RefU64((INVALID_REF0_SENTINEL as u64) << 32);
        assert_eq!(
            dedupe_members(owner, &[RefU64(0), child_a, sentinel]),
            vec![child_a],
            "无效 ref0 仍在同一处滤掉"
        );

        let message = render_pe_owner_replace("pe:16189_0", &[child_a, child_a])
            .expect_err("渲染边界不放行必然撞唯一索引的成员块")
            .to_string();
        assert!(
            message.contains("pe:16189_0"),
            "错误必须带 owner: {message}"
        );
        assert!(
            message.contains("pe:24381_34109"),
            "错误必须带重复 child: {message}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pe_owner_replace_isolates_owner_removes_tail_and_handles_large_blocks() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("issue14")
            .use_db("issue14")
            .await
            .expect("select db");
        db.query(
            "DEFINE TABLE pe_owner TYPE RELATION IN pe OUT pe SCHEMALESS; \
             DEFINE INDEX unique_pe_owner ON pe_owner FIELDS in, out UNIQUE;",
        )
        .await
        .expect("define schema")
        .check()
        .expect("valid schema");

        let child_a = RefU64((24381u64 << 32) | 34109);
        let child_b = RefU64((24381u64 << 32) | 34110);
        let child_c = RefU64((24381u64 << 32) | 34111);
        let initial = render_pe_owner_replace("pe:16189_0", &[child_a, child_b, child_c])
            .expect("render initial block");
        db.query(initial)
            .await
            .expect("write initial block")
            .check()
            .expect("valid initial block");
        db.query(render_pe_owner_replace("pe:16189_1", &[child_c]).expect("render adjacent owner"))
            .await
            .expect("write adjacent owner")
            .check()
            .expect("valid adjacent owner");

        let shrunk =
            render_pe_owner_replace("pe:16189_0", &[child_b]).expect("render shrunk block");
        for _ in 0..2 {
            db.query(&shrunk)
                .await
                .expect("replace owner block")
                .check()
                .expect("replacement must be replay-safe");
        }

        let mut response = db
            .query("SELECT VALUE in FROM pe:16189_0<-pe_owner")
            .await
            .expect("read owner block")
            .check()
            .expect("valid owner query");
        let children = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .expect("decode children");
        assert_eq!(children.len(), 1, "缩短后的尾槽必须全部删除");
        assert_eq!(children[0].to_string(), child_b.to_pe_key());

        let mut response = db
            .query("SELECT VALUE in FROM pe:16189_1<-pe_owner")
            .await
            .expect("read adjacent owner")
            .check()
            .expect("valid adjacent owner query");
        let adjacent = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .expect("decode adjacent owner");
        assert_eq!(adjacent.len(), 1, "owner 范围删除不能越界到相邻 owner");
        assert_eq!(adjacent[0].to_string(), child_c.to_pe_key());

        let large = (1..=1000)
            .map(|slot| RefU64((50000u64 << 32) | slot))
            .collect::<Vec<_>>();
        for children in [&large[..], &large[..10], &large[..]] {
            db.query(render_pe_owner_replace("pe:50000_0", children).expect("render large block"))
                .await
                .expect("replace large owner")
                .check()
                .expect("large owner transaction must succeed");
        }
        let mut response = db
            .query("SELECT VALUE in FROM pe:50000_0<-pe_owner")
            .await
            .expect("read large owner")
            .check()
            .expect("valid large owner query");
        let large_children = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .expect("decode large owner");
        assert_eq!(large_children.len(), 1000);
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
    fn conflicting_ref0_blocks_only_that_reference() {
        let mut loc = locator();
        insert_ref0_affiliation(&mut loc.ref0_to_dbnum, &mut loc.ref0_conflicts, 100, 7355);

        let error = loc
            .resolve_ref0(100)
            .expect_err("the ambiguous Ref0 must be rejected");
        assert!(error.to_string().contains("7320"));
        assert!(error.to_string().contains("7355"));
        assert_eq!(loc.resolve_ref0(200).unwrap(), Some(8001));
        assert_eq!(loc.resolve_ref0(999).unwrap(), None);
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
    fn fallback_ref0_cache_reuses_only_an_unchanged_file() {
        let mut cache = Ref0IndexCache::default();
        cache.put(7320, "100:200".into(), vec![13244]);

        assert_eq!(cache.get_if_fresh(7320, "100:200"), Some(&vec![13244]));
        assert_eq!(cache.get_if_fresh(7320, "100:201"), None);
    }

    #[test]
    fn legacy_or_empty_ref0_cache_entries_are_never_fresh() {
        let legacy = r#"{"by_dbnum":{"7320":{"fingerprint":"100:200","ref0s":[]}}}"#;
        assert!(
            Ref0IndexCache::decode(legacy).is_none(),
            "pre-ADR-037 cache format must be invalidated"
        );

        let mut cache = Ref0IndexCache::default();
        cache.put(7320, "100:200".into(), Vec::new());
        assert_eq!(cache.get_if_fresh(7320, "100:200"), None);
        assert!(!cache.by_dbnum.contains_key(&7320));
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
        // 2026-09-07 对拍 oracle 抓到的两处漏：正体名下的负体、`SPRF.PSTR` 指向的 p-line 集 PTSS。
        for positive in [
            "SBOX", "SCYL", "LCYL", "SCON", "LSNO", "LPYR", "SCTO", "SRTO", "SSPH", "SSLC", "SDSH",
            "SDIS", "SEXT", "SREV",
        ] {
            assert!(
                allow.contains(positive),
                "正体 {positive} 名下的负体要随闭包入库"
            );
        }
        assert!(allow.contains("PTSS"), "型材对齐读 PTSS 名下的 PLIN");
        assert!(
            !allow.contains("PSTR"),
            "PSTR 是属性名不是 noun，写进名单等于没写"
        );
    }
}
