//! Which databases an MDB declares, resolved from the SYS database file.
//!
//! An MDB's `CURD` is the authoritative list, and it reaches across projects:
//! `AvevaMarineSample /ALL` declares six Dictionary databases, four of which
//! live under `AvevaCatalogue` and `SCB`. Picking dictionaries by scanning a
//! project directory gets this wrong in both directions at once — the first
//! attempt at it here missed two that `/ALL` declares and included one it does
//! not — and neither mistake reports anything. A UDA whose dictionary was left
//! out reads exactly like a UDA with no value.
//!
//! SYS PROJ is a character vector (ACP / SCB / APS), not an integer flag.
//! An empty vector selects the declaring project. Never substitute another
//! project merely because it contains a file with the same dbnum.
//!
//! [`crate::data_interface::update_scope::UpdateScope`] answers the same
//! question for `STYP = DESI` through SurrealDB, which needs the SYS database
//! parsed and synced first. This reads the file, so it is available during
//! initialization — before anything has been synced — which is the point at
//! which the Dictionary set has to be known.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use aios_core::RefU64;
use aios_core::options::DbOption;
use aios_core::types::named_attvalue::NamedAttrValue;
use anyhow::Context;

/// The `STYP` a Dictionary database carries in the SYS record.
///
/// Not `aios_core`'s `DBType::DICT`, which is 6 and matches nothing in the AMS
/// SYS database: every dictionary `/ALL` declares reads 8. `DESI = 1` does
/// agree, which is why `UpdateScope` has never tripped over this — a query
/// asking for 6 would come back empty rather than wrong, and read as "this
/// MDB declares no dictionaries".
pub const DICT_STYP: i64 = 8;
pub const DESI_STYP: i64 = 1;
/// Isometric element database.  It participates in the source model exactly
/// like DESI; CATA remains catalogue-only and is deliberately excluded.
pub const ISOD_STYP: i64 = 6;

/// SYST 库（dbno 8191）的 WORL：`24575/3`——`24575 = 8191 | 0x4000`，系统元素落在第 2 个 extent，
/// 不是元素库那种 `primary_extent_ref / 0`。E3D 2.1 + 3.1 全部 14 个 `*sys` 库实测同一位置（2026-09-12）。
pub const SYS_WORLD: e3d_io::RefNo = e3d_io::RefNo::new(24575, 3);
/// SYST 库的 MDB 世界 `MDBW`（名 `/*M`）：`24575/7`，owner 是 [`SYS_WORLD`]，每个 MDB 元素都是它的直接成员。
pub const SYS_MDB_WORLD: e3d_io::RefNo = e3d_io::RefNo::new(24575, 7);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdbDatabase {
    pub dbnum: u32,
    pub styp: i64,
    /// Legacy display field: length of PROJ, never a project identifier.
    pub proj: i64,
    /// Exact SYS PROJ character vector. None means the declaring project.
    pub declared_project_code: Option<String>,
    /// The SYS element's name, e.g. `*MASTER/DICT`.
    pub name: String,
    /// `None` when the declaration names a database with no file under any
    /// configured project directory. Kept rather than dropped: a declared
    /// database that is not on disk is a deployment problem worth seeing.
    pub path: Option<PathBuf>,
    /// Which project `path` came out of.
    pub project: Option<String>,
    /// The other files carrying this dbnum: layers this project's own
    /// extract family put underneath (ADR-028), plus whatever another project
    /// numbers the same. A dbnum is unique inside a project only, so losing
    /// these silently is how a declaration binds to a foreign file unnoticed.
    pub shadowed: Vec<PathBuf>,
}

impl MdbDatabase {
    /// Detect a declaration/path mismatch without reducing PROJ to a flag.
    pub fn off_declared_project(&self, declaring_project: &str) -> bool {
        let Some(project) = self.project.as_deref() else {
            return false;
        };
        match self.declared_project_code.as_deref() {
            None => !project
                .trim()
                .eq_ignore_ascii_case(declaring_project.trim()),
            Some(code) => !self
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
                .is_some_and(|file| file.prefix.eq_ignore_ascii_case(code)),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MdbMembership {
    mdb: String,
    project: String,
    /// In `CURD` order. Order decides which definition wins a duplicated
    /// `UKEY`, so it is preserved rather than sorted.
    databases: Vec<MdbDatabase>,
    /// Things that went wrong while resolving this list and that a person
    /// should see. They deliberately do not live only in `log::warn!`: the
    /// sandbox and plenty of deployments run `enable_log = false`, the logger
    /// is then never initialised, and not one of those lines ever appears.
    problems: Vec<String>,
}

impl MdbMembership {
    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn databases(&self) -> &[MdbDatabase] {
        &self.databases
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn of_type(&self, styp: i64) -> impl Iterator<Item = &MdbDatabase> {
        self.databases.iter().filter(move |db| db.styp == styp)
    }

    pub fn element_databases(&self) -> impl Iterator<Item = &MdbDatabase> {
        self.databases
            .iter()
            .filter(|db| matches!(db.styp, DESI_STYP | ISOD_STYP))
    }

    /// Declared Dictionary databases that exist on disk, in `CURD` order —
    /// the list a UDA catalog should be built from.
    pub fn dictionary_paths(&self) -> Vec<PathBuf> {
        self.of_type(DICT_STYP)
            .filter_map(|db| db.path.clone())
            .collect()
    }

    pub fn counts_by_type(&self) -> BTreeMap<i64, usize> {
        let mut out = BTreeMap::new();
        for db in &self.databases {
            *out.entry(db.styp).or_default() += 1;
        }
        out
    }

    /// Declared databases with no file under any configured project directory.
    pub fn unresolved(&self) -> impl Iterator<Item = &MdbDatabase> {
        self.databases.iter().filter(|db| db.path.is_none())
    }

    /// A complete room reference frame requires every declared design file.
    /// An absent file may contain a room or a panel; it is not an empty database.
    pub(crate) fn require_design_files(&self) -> anyhow::Result<()> {
        let missing = self
            .unresolved()
            .filter(|db| db.styp == DESI_STYP)
            .map(|db| format!("{} ({})", db.dbnum, db.name))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            missing.is_empty(),
            "MDB {} / {} room inputs incomplete: missing DESI source files: {}",
            self.project,
            self.mdb,
            missing.join(", ")
        );
        Ok(())
    }
}

fn report_sys_info(message: &str) {
    log::info!("{message}");
    println!("{message}");
}

fn report_sys_error(message: &str) {
    log::error!("{message}");
    eprintln!("{message}");
}

fn sys_basic_info_message(
    sys: &Path,
    header: &crate::data_interface::db_file_header::DbFileHeader,
    selected_schema: &str,
    binary_version: &str,
) -> String {
    let saved_at = header
        .latest_session
        .rfc3339()
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "[SYS 解析] 基本信息: file={} bytes={} dbnum={} db_type={} latest_sesno={} \
         latest_ses_pgno={} session_pgno={} previous_session_pgno={:?} end_pgno={} \
         index_root_pgno={} saved_at={} machine={:?} comment={:?} selected_schema={} \
         binary_version={}",
        sys.display(),
        header.file_size,
        header.dbnum,
        header.db_type,
        header.latest_sesno(),
        header.latest_ses_pgno,
        header.latest_session.pgno,
        header.latest_session.previous_pgno,
        header.latest_session.end_pgno,
        header.latest_session.index_root_pgno,
        saved_at,
        header.latest_session.machine,
        header.latest_session.comment,
        selected_schema,
        binary_version
    )
}

fn mdb_parse_summary_message(
    sys: &Path,
    mdb: &str,
    root: RefU64,
    curd_entries: usize,
    decoded_databases: usize,
    skipped_databases: usize,
    counts_by_styp: &BTreeMap<i64, usize>,
) -> String {
    format!(
        "[SYS 解析] MDB 解析成功: file={} mdb={} root={}/{} curd_entries={} \
         decoded_databases={} skipped_databases={} styp={counts_by_styp:?}",
        sys.display(),
        mdb,
        root.get_0(),
        root.get_1(),
        curd_entries,
        decoded_databases,
        skipped_databases
    )
}

fn named_attr_value_kind(value: &NamedAttrValue) -> &'static str {
    match value {
        NamedAttrValue::InvalidType => "InvalidType",
        NamedAttrValue::IntegerType(_) => "IntegerType",
        NamedAttrValue::StringType(_) => "StringType",
        NamedAttrValue::F32Type(_) => "F32Type",
        NamedAttrValue::F32VecType(_) => "F32VecType",
        NamedAttrValue::Vec3Type(_) => "Vec3Type",
        NamedAttrValue::StringArrayType(_) => "StringArrayType",
        NamedAttrValue::BoolArrayType(_) => "BoolArrayType",
        NamedAttrValue::IntArrayType(_) => "IntArrayType",
        NamedAttrValue::BoolType(_) => "BoolType",
        NamedAttrValue::ElementType(_) => "ElementType",
        NamedAttrValue::WordType(_) => "WordType",
        NamedAttrValue::RefU64Type(_) => "RefU64Type",
        NamedAttrValue::RefU64Array(_) => "RefU64Array",
        NamedAttrValue::LongType(_) => "LongType",
        NamedAttrValue::RefnoEnumType(_) => "RefnoEnumType",
    }
}

/// Read `<project>`'s SYS database and resolve `mdb`'s declaration.
pub fn resolve(db_option: &DbOption, project: &str, mdb: &str) -> anyhow::Result<MdbMembership> {
    let plan = crate::data_interface::project_paths::plan_watch_dirs(db_option);
    let own_dirs: Vec<PathBuf> = plan
        .projects
        .iter()
        .filter(|entry| entry.project == project)
        .flat_map(|entry| entry.db_dirs.iter().cloned())
        .collect();
    if own_dirs.is_empty() {
        anyhow::bail!("项目 {project} 没有解析出任何库目录，无从读取它的 SYS 库");
    }
    // Kept per project rather than flattened into one directory list: a dbnum
    // is unique inside a project only, so "which file" cannot be answered
    // before "which project".
    let projects: Vec<(String, Vec<PathBuf>)> = plan
        .projects
        .iter()
        .map(|entry| (entry.project.clone(), entry.db_dirs.clone()))
        .collect();
    let mut problems = Vec::new();
    let wanted = format!("/{}", mdb.trim_start_matches('/'));

    let mut tried = Vec::new();
    let mut candidate_names = BTreeSet::new();
    let mut candidate_scan_errors = Vec::new();
    let mut declaration_errors = Vec::new();
    for sys in sys_candidates(&own_dirs) {
        tried.push(sys.display().to_string());
        match read_declaration(&sys, project, &wanted, &projects) {
            Ok(Some(databases)) => {
                for db in databases.iter().filter(|db| db.path.is_none()) {
                    problems.push(format!(
                        "MDB {wanted}: dbnum={} ({}) declared project {} has no matching configured source; same-dbnum foreign fallback disabled",
                        db.dbnum, db.name, db.declared_project_code.as_deref().unwrap_or(project)
                    ));
                }
                return Ok(MdbMembership {
                    mdb: wanted,
                    project: project.to_string(),
                    databases,
                    problems,
                });
            }
            Ok(None) => {
                match read_mdb_candidate_names(&sys) {
                    Ok(names) => candidate_names.extend(names),
                    Err(error) => {
                        candidate_scan_errors.push(format!("{}：{error:#}", sys.display()))
                    }
                }
                continue;
            }
            Err(error) => {
                let detail = format!(
                    "[SYS 解析] 解析失败: file={} project={} mdb={} reason={error:#}",
                    sys.display(),
                    project,
                    wanted
                );
                report_sys_error(&detail);
                log::warn!("读取 {} 失败，继续找下一个：{error:#}", sys.display());
                problems.push(format!("读取 {} 失败，已跳过：{error:#}", sys.display()));
                declaration_errors.push(format!("{}：{error:#}", sys.display()));
            }
        }
    }
    let candidates = if candidate_names.is_empty() {
        "（没有解析到任何 MDB 名称）".to_string()
    } else {
        candidate_names
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("、")
    };
    let scan_errors = if candidate_scan_errors.is_empty() {
        String::new()
    } else {
        format!("\n候选名称扫描失败：{}", candidate_scan_errors.join(" / "))
    };
    let parse_errors = if declaration_errors.is_empty() {
        String::new()
    } else {
        format!("\nSYS 解析失败：{}", declaration_errors.join(" / "))
    };
    let message = format!(
        "在项目 {project} 的 SYS 库里找不到名为 {wanted} 的 MDB。\n\
         MDB 候选名称（{} 个）：{candidates}\n\
         已尝试 SYS 库：{}{}{}",
        candidate_names.len(),
        if tried.is_empty() {
            "（没有找到任何 SYS 库文件）".to_string()
        } else {
            tried.join(" / ")
        },
        scan_errors,
        parse_errors
    );
    println!("[warn] {message}");
    anyhow::bail!(message)
}

/// Files whose name ends in `sys`, which is how E3D names the SYSTEM database
/// (`amssys`, `acpsys`). Sibling `*mis` / `*com` files are other things.
fn sys_candidates(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with("sys"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Best-effort diagnostic for a misspelled `mdb_name`.
///
/// The normal lookup above asks e3d-io for one exact stored `NAME`. On a miss,
/// scan the same SYS file's Named tier and show every element whose noun is
/// `MDB`, so the error tells the operator what can actually be configured.
/// This deliberately runs only on the error path: a Named scan parses every
/// record in the SYS database.
fn read_mdb_candidate_names(sys: &Path) -> anyhow::Result<Vec<String>> {
    let mut engine = e3d_io::ReadOnlyEngine::open(sys)
        .map_err(|error| anyhow::anyhow!("打开 SYS 库扫描 MDB 候选失败：{error}"))?;
    let rows = engine
        .scan_elements(e3d_io::ScanTier::Named)
        .map_err(|error| anyhow::anyhow!("扫描 SYS 库 Named 档失败：{error}"))?;
    let mdb_noun = e3d_attlib::db1_hash("MDB");
    let mut names = BTreeSet::new();
    let mut skipped = Vec::new();
    for row in rows {
        match row {
            Ok(row) if row.noun_hash == mdb_noun => {
                if let Some(name) = row.name {
                    let name = name.trim();
                    if !name.is_empty() {
                        names.insert(format!("/{}", name.trim_start_matches('/')));
                    }
                }
            }
            Ok(_) => {}
            Err(error) => skipped.push(error.to_string()),
        }
    }
    anyhow::ensure!(
        !names.is_empty() || skipped.is_empty(),
        "扫描中有 {} 条记录无法读取，且未解析出 MDB 名称；首条错误：{}",
        skipped.len(),
        skipped.first().map(String::as_str).unwrap_or_default()
    );
    if !skipped.is_empty() {
        log::warn!(
            "扫描 {} 的 MDB 候选时跳过 {} 条损坏记录；已保留其余候选",
            sys.display(),
            skipped.len()
        );
    }
    Ok(names.into_iter().collect())
}

/// `Ok(None)` when the file parses but holds no MDB of that name — the caller
/// moves on to the next candidate rather than failing.
fn read_declaration(
    sys: &Path,
    project: &str,
    wanted: &str,
    projects: &[(String, Vec<PathBuf>)],
) -> anyhow::Result<Option<Vec<MdbDatabase>>> {
    // e3d-io 读法（parse_pdms_db 退役清单 §B5）：SYS 库按最新会话钉成一个单库 `DbSet`
    // （模板 `sysvir.dat`），MDB 元素按存储的 NAME 找（`find_named` 是一次 Named 档全树扫，
    // 与旧读法整库 `gen_ref_type_pos_table` 同量级），属性经 `direct_attmap::to_named_attmap`
    // 落成与旧 `whole_attmap.merge()` 同一形状的 `NamedAttrMap`——下面对 `CURD` / `DBNO` /
    // `PROJ` / `STYP` / `NAME` 的读法逐字不变。
    let sys_db = SysDb::open(sys)?;

    // Design and catalogue SYS databases both declare `/ALL`, and the
    // catalogue one's CURD is often nearly empty. `UpdateScope` takes the
    // longest for the same reason; picking either is otherwise a coin flip
    // that silently shrinks the scope.
    let named = sys_db
        .mdbs_named(wanted)
        .with_context(|| format!("在 {} 查找 MDB {wanted}", sys.display()))?;
    let named_matches = named.len();
    report_sys_info(&format!(
        "[SYS 解析] MDB 查找完成: file={} mdb={} named_matches={named_matches}",
        sys.display(),
        wanted
    ));

    let mut candidate_errors = Vec::new();
    let mut best: Option<(RefU64, Vec<RefU64>)> = None;
    for refno in named {
        let merged = match sys_db.attrs(refno) {
            Ok(Some(merged)) => merged,
            Ok(None) => {
                let message = format!(
                    "[SYS 解析] MDB 候选解析失败: file={} mdb={} root={}/{} \
                     stage=element_lookup reason=元素不在最新会话索引中",
                    sys.display(),
                    wanted,
                    refno.get_0(),
                    refno.get_1()
                );
                report_sys_error(&message);
                candidate_errors.push(message);
                continue;
            }
            Err(error) => {
                let message = format!(
                    "[SYS 解析] MDB 候选解析失败: file={} mdb={} root={}/{} \
                     stage=attribute_extract reason={error:#}",
                    sys.display(),
                    wanted,
                    refno.get_0(),
                    refno.get_1()
                );
                report_sys_error(&message);
                candidate_errors.push(message);
                continue;
            }
        };
        let actual_type = merged.get_as_string("TYPE").unwrap_or_default();
        let actual_name = merged.get_as_string("NAME").unwrap_or_default();
        if actual_type.trim() != "MDB" || actual_name.trim() != wanted {
            let message = format!(
                "[SYS 解析] MDB 候选解析失败: file={} mdb={} root={}/{} \
                 stage=identity_check reason=TYPE/NAME 不匹配 actual_type={actual_type:?} \
                 actual_name={actual_name:?}",
                sys.display(),
                wanted,
                refno.get_0(),
                refno.get_1()
            );
            report_sys_error(&message);
            candidate_errors.push(message);
            continue;
        }
        let items = match merged.get_val("CURD") {
            Some(NamedAttrValue::RefU64Array(items)) => items,
            Some(other) => {
                let message = format!(
                    "[SYS 解析] MDB 候选解析失败: file={} mdb={} root={}/{} \
                     stage=curd_decode reason=CURD 类型错误 actual={}",
                    sys.display(),
                    wanted,
                    refno.get_0(),
                    refno.get_1(),
                    named_attr_value_kind(other)
                );
                report_sys_error(&message);
                candidate_errors.push(message);
                continue;
            }
            None => {
                let message = format!(
                    "[SYS 解析] MDB 候选解析失败: file={} mdb={} root={}/{} \
                     stage=curd_decode reason=没有解析出 CURD 属性",
                    sys.display(),
                    wanted,
                    refno.get_0(),
                    refno.get_1()
                );
                report_sys_error(&message);
                candidate_errors.push(message);
                continue;
            }
        };
        let members: Vec<RefU64> = items.iter().map(|item| item.refno()).collect();
        report_sys_info(&format!(
            "[SYS 解析] MDB 候选读取成功: file={} mdb={} root={}/{} curd_entries={}",
            sys.display(),
            wanted,
            refno.get_0(),
            refno.get_1(),
            members.len()
        ));
        if best
            .as_ref()
            .is_none_or(|(_, held)| members.len() > held.len())
        {
            best = Some((refno, members));
        }
    }
    let Some((root, members)) = best else {
        if named_matches > 0 {
            anyhow::bail!(
                "SYS {} 找到 {named_matches} 个名为 {wanted} 的元素，但没有一个可用的 MDB \
                 声明；候选错误：{}",
                sys.display(),
                candidate_errors.join(" | ")
            );
        }
        return Ok(None);
    };

    let curd_entries = members.len();
    let mut databases = Vec::with_capacity(members.len());
    let mut skipped_databases = 0usize;
    for (curd_index, refno) in members.into_iter().enumerate() {
        let merged = match sys_db.attrs(refno) {
            Ok(Some(merged)) => merged,
            Ok(None) => {
                skipped_databases += 1;
                report_sys_error(&format!(
                    "[SYS 解析] DB 声明解析失败: file={} mdb={} root={}/{} curd_index={} \
                     refno={}/{} stage=element_lookup reason=元素不在最新会话索引中",
                    sys.display(),
                    wanted,
                    root.get_0(),
                    root.get_1(),
                    curd_index,
                    refno.get_0(),
                    refno.get_1()
                ));
                continue;
            }
            Err(error) => {
                skipped_databases += 1;
                report_sys_error(&format!(
                    "[SYS 解析] DB 声明解析失败: file={} mdb={} root={}/{} curd_index={} \
                     refno={}/{} stage=attribute_extract reason={error:#}",
                    sys.display(),
                    wanted,
                    root.get_0(),
                    root.get_1(),
                    curd_index,
                    refno.get_0(),
                    refno.get_1()
                ));
                continue;
            }
        };
        let number = |key: &str| -> Option<i64> {
            merged
                .get_as_string(key)
                .and_then(|value| value.trim().parse::<i64>().ok())
        };
        let Some(dbnum) = number("DBNO").and_then(|value| u32::try_from(value).ok()) else {
            skipped_databases += 1;
            report_sys_error(&format!(
                "[SYS 解析] DB 声明解析失败: file={} mdb={} root={}/{} curd_index={} \
                 refno={}/{} stage=dbno_decode reason=DBNO 缺失、不是整数或超出 u32 \
                 raw={:?}",
                sys.display(),
                wanted,
                root.get_0(),
                root.get_1(),
                curd_index,
                refno.get_0(),
                refno.get_1(),
                merged.get_as_string("DBNO")
            ));
            continue;
        };
        let styp = number("STYP").unwrap_or_else(|| {
            report_sys_error(&format!(
                "[SYS 解析] DB 声明字段异常: file={} mdb={} root={}/{} curd_index={} \
                 refno={}/{} dbnum={} field=STYP reason=缺失或不是整数，保留为 -1 raw={:?}",
                sys.display(),
                wanted,
                root.get_0(),
                root.get_1(),
                curd_index,
                refno.get_0(),
                refno.get_1(),
                dbnum,
                merged.get_as_string("STYP")
            ));
            -1
        });
        // Read the native character vector before NamedAttrMap can stringify
        // it as an array length. Missing/undecoded evidence is not an empty PROJ.
        let declared_project_code = sys_db.project_code(refno).with_context(|| {
            format!(
                "解析 SYS {} 的 MDB {wanted}（root={}/{}, curd_index={curd_index}, \
                 refno={}/{}, dbnum={dbnum}）PROJ 失败",
                sys.display(),
                root.get_0(),
                root.get_1(),
                refno.get_0(),
                refno.get_1()
            )
        })?;
        // Keep the legacy numeric field as a length, not a project identifier.
        let proj = declared_project_code
            .as_ref()
            .map_or(0, |code| code.len() as i64);
        let located = locate_declared(projects, project, declared_project_code.as_deref(), dbnum)
            .with_context(|| {
            format!(
                "定位 SYS {} 的 MDB {wanted} CURD[{curd_index}] dbnum={dbnum} 文件失败",
                sys.display()
            )
        })?;
        databases.push(MdbDatabase {
            dbnum,
            styp,
            proj,
            declared_project_code,
            name: merged
                .get_as_string("NAME")
                .unwrap_or_default()
                .trim()
                .to_string(),
            path: located.as_ref().map(|found| found.path.clone()),
            project: located.as_ref().map(|found| found.project.clone()),
            shadowed: located.map(|found| found.shadowed).unwrap_or_default(),
        });
    }
    let mut counts_by_styp = BTreeMap::new();
    for database in &databases {
        *counts_by_styp.entry(database.styp).or_default() += 1;
    }
    if curd_entries > 0 && databases.is_empty() {
        let message = format!(
            "[SYS 解析] MDB 解析失败: file={} mdb={} root={}/{} curd_entries={} \
             decoded_databases=0 skipped_databases={} reason=CURD 非空但所有 DB 声明均解析失败",
            sys.display(),
            wanted,
            root.get_0(),
            root.get_1(),
            curd_entries,
            skipped_databases
        );
        report_sys_error(&message);
        anyhow::bail!(message);
    }
    report_sys_info(&mdb_parse_summary_message(
        sys,
        wanted,
        root,
        curd_entries,
        databases.len(),
        skipped_databases,
        &counts_by_styp,
    ));
    Ok(Some(databases))
}

/// One SYS database file, pinned at its latest session and read through e3d-io.
struct SysDb {
    set: Arc<e3d_io::DbSet>,
    dbno: u32,
}

impl SysDb {
    fn project_code(&self, refno: RefU64) -> anyhow::Result<Option<String>> {
        let extraction = self
            .set
            .element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()))
            .extraction()
            .map_err(|error| {
                anyhow::anyhow!(
                    "提取 SYS DB 元素 {}/{} 的 PROJ 失败: {error}",
                    refno.get_0(),
                    refno.get_1()
                )
            })?;
        let attr = extraction
            .attributes
            .iter()
            .find(|attr| attr.name == "PROJ")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SYS DB 元素 {}/{} 没有 PROJ descriptor",
                    refno.get_0(),
                    refno.get_1()
                )
            })?;
        decode_project_code(attr.value.as_ref()).with_context(|| {
            format!(
                "解码 SYS DB 元素 {}/{} 的 PROJ",
                refno.get_0(),
                refno.get_1()
            )
        })
    }

    fn open(sys: &Path) -> anyhow::Result<Self> {
        let header = crate::data_interface::db_file_header::DbFileHeader::read(sys)
            .with_context(|| format!("读取 SYS 基本信息 {}", sys.display()))?;
        let detected = e3d_io::detect_database_version(sys);
        let binary_version = match &detected {
            Ok(version) => version.id.to_string(),
            Err(error) => {
                report_sys_error(&format!(
                    "[SYS 解析] 二进制版本识别失败: file={} reason={error}",
                    sys.display()
                ));
                "unknown".to_string()
            }
        };
        let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()
            .with_context(|| format!("为 SYS {} 加载 schema", sys.display()))?;
        let selected_schema = schema
            .version()
            .map(|version| version.id)
            .unwrap_or("explicit-directory");
        if let Ok(detected) = detected
            && schema
                .version()
                .is_some_and(|selected| selected.id != detected.id)
        {
            report_sys_error(&format!(
                "[SYS 解析] 版本不匹配: file={} selected_schema={} binary_version={}；\
                 继续尝试解析并打印诊断，请核对配置后再使用结果",
                sys.display(),
                selected_schema,
                detected.id
            ));
        }
        let set = crate::data_interface::e3d_collector::open_pinned_set(
            &schema,
            sys,
            &header.db_type,
            header.latest_sesno(),
        )
        .with_context(|| {
            format!(
                "按 dbnum={} db_type={} latest_sesno={} 打开 SYS {}",
                header.dbnum,
                header.db_type,
                header.latest_sesno(),
                sys.display()
            )
        })?;
        report_sys_info(&sys_basic_info_message(
            sys,
            &header,
            selected_schema,
            &binary_version,
        ));
        Ok(Self {
            set,
            dbno: header.dbnum as u32,
        })
    }

    /// MDB elements whose stored `NAME` is exactly `name` (leading `/` included), found by
    /// **navigation, not by scanning**: SYS `WORL` → `MDBW` (`/*M`) → members, each a point lookup.
    ///
    /// 位置是确定的（2026-09-12 对 E3D 2.1 + 3.1 两套安装下全部 14 个 `*sys` 库实测，0 例外）：
    /// SYST 的 WORL 在 `24575/3`（dbno 8191 的第 2 个 extent，owner 0，名 `/*`），MDB 世界 `MDBW`
    /// 在 `24575/7`（owner = WORL，名 `/*M`），**每一个** MDB 元素都是它的直接成员（AMS 50 个、
    /// APS 5 个、ZDJ 12 个…）。所以找一个 MDB = 读 MDBW 的成员表 + 逐成员核 noun 与 NAME，
    /// `read_stats.scan_calls` 为 0——此前这里是 `find_named` 的一次 Named 档全库扫
    /// （AMS 1,207 个元素），是启动链上最后一次非零 `scan_calls`。
    ///
    /// 位置对不上（不是 SYST、或那两处不是 WORL / MDBW）就**出声**回落到扫描，不静默：
    /// 那是格式假设被打破，得让人看见。
    fn mdbs_named(&self, name: &str) -> anyhow::Result<Vec<RefU64>> {
        let mdb_noun = e3d_attlib::db1_hash("MDB");
        let mdbw_noun = e3d_attlib::db1_hash("MDBW");
        let mdbw = self.set.element(SYS_MDB_WORLD);
        let mdbw_ok = mdbw
            .exists()
            .map_err(|error| anyhow::anyhow!("检查 SYS MDBW {SYS_MDB_WORLD} 是否存在失败: {error}"))?
            && mdbw
                .noun_hash()
                .map_err(|error| anyhow::anyhow!("读 SYS MDBW {SYS_MDB_WORLD} 的 noun 失败: {error}"))?
                == mdbw_noun;
        if !mdbw_ok {
            report_sys_error(&format!(
                "[SYS 解析] dbno={} 的 {SYS_MDB_WORLD} 不是 MDBW（格式假设被打破），回落到 Named 档全库扫描找 {name}",
                self.dbno
            ));
            return self.named(name);
        }
        let mut out = Vec::new();
        for member in mdbw
            .members()
            .map_err(|error| anyhow::anyhow!("读 SYS MDBW {SYS_MDB_WORLD} 成员表失败: {error}"))?
        {
            let refno = member.refno();
            let noun = member
                .noun_hash()
                .map_err(|error| anyhow::anyhow!("读 SYS 元素 {refno} 的 noun 失败: {error}"))?;
            if noun != mdb_noun {
                continue;
            }
            let stored = member
                .get_string("NAME")
                .map_err(|error| anyhow::anyhow!("读 SYS MDB {refno} 的 NAME 失败: {error}"))?;
            if stored.as_deref() == Some(name) {
                out.push(RefU64::from_two_nums(refno.word0, refno.word1));
            }
        }
        Ok(out)
    }

    /// Elements whose stored `NAME` is exactly `name` (leading `/` included).
    ///
    /// 一次 Named 档全库扫；正常路径不走它（见 [`Self::mdbs_named`]），只留给格式假设被打破时的回落。
    fn named(&self, name: &str) -> anyhow::Result<Vec<RefU64>> {
        Ok(self
            .set
            .find_named(self.dbno, name)
            .map_err(|error| anyhow::anyhow!("scan SYS database names: {error}"))?
            .into_iter()
            .map(|refno| RefU64::from_two_nums(refno.word0, refno.word1))
            .collect())
    }

    /// The element's attributes in the shape the legacy `whole_attmap.merge()`
    /// produced. Absence and parsing failure are deliberately separate: the
    /// old `Option` shape swallowed every e3d-io/conversion error and reduced
    /// them all to the misleading startup line "声明 0 个库".
    fn attrs(&self, refno: RefU64) -> anyhow::Result<Option<aios_core::NamedAttrMap>> {
        let element = self
            .set
            .element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()));
        let exists = element.exists().map_err(|error| {
            anyhow::anyhow!(
                "检查 SYS 元素 {}/{} 是否存在失败: {error}",
                refno.get_0(),
                refno.get_1()
            )
        })?;
        if !exists {
            return Ok(None);
        }
        let extraction = element.extraction().map_err(|error| {
            anyhow::anyhow!(
                "提取 SYS 元素 {}/{} 属性失败: {error}",
                refno.get_0(),
                refno.get_1()
            )
        })?;
        let attrs = crate::data_interface::direct_attmap::to_named_attmap(&extraction).map_err(
            |error| {
                anyhow::anyhow!(
                    "转换 SYS 元素 {}/{} 属性失败: {error}",
                    refno.get_0(),
                    refno.get_1()
                )
            },
        )?;
        let undecoded = attrs
            .undecoded
            .iter()
            .filter(|attr| {
                matches!(
                    attr.name.as_str(),
                    "CURD" | "DBNO" | "STYP" | "PROJ" | "NAME"
                )
            })
            .map(|attr| format!("{}={}", attr.name, attr.reason))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            undecoded.is_empty(),
            "SYS 关键属性未解码: {}",
            undecoded.join(", ")
        );
        Ok(Some(attrs.map))
    }
}

/// Where a declared dbnum resolved to.
struct Located {
    project: String,
    path: PathBuf,
    shadowed: Vec<PathBuf>,
}

// core.dll DB_DB::setFromDbele (0x5912440) reads PROJ as a character
// vector, passing it to CHFRAN, not as a numeric foreign-project flag.
fn decode_project_code(
    value: Option<&e3d_io::record::descriptor::DescriptorValue>,
) -> anyhow::Result<Option<String>> {
    use e3d_io::record::descriptor::DescriptorValue;
    let words = match value {
        Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words)) => words.clone(),
        Some(DescriptorValue::IntArray(words)) => words.iter().map(|word| *word as u32).collect(),
        Some(DescriptorValue::Unset) => return Ok(None),
        other => anyhow::bail!("SYS PROJ is not a decoded character vector: {other:?}"),
    };
    if words.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        words.len() == 3
            && words
                .iter()
                .all(|word| *word <= 127 && (*word as u8).is_ascii_alphabetic()),
        "invalid SYS project code: {words:?}"
    );
    Ok(Some(
        words
            .into_iter()
            .map(|word| (word as u8 as char).to_ascii_uppercase())
            .collect(),
    ))
}

/// Bind only the declared project. A similarly numbered file in another
/// project is diagnostic evidence, never a fallback input.
fn locate_declared(
    projects: &[(String, Vec<PathBuf>)],
    declaring: &str,
    code: Option<&str>,
    dbnum: u32,
) -> anyhow::Result<Option<Located>> {
    let mut matches = Vec::new();
    let mut shadowed = Vec::new();
    for (project, dirs) in projects {
        let Some((path, rest)) = pick_within_project(dirs, dbnum) else {
            continue;
        };
        let selected = match code {
            None => project.trim().eq_ignore_ascii_case(declaring.trim()),
            Some(code) => path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
                .is_some_and(|file| file.prefix.eq_ignore_ascii_case(code)),
        };
        if selected {
            matches.push((project.clone(), path, rest));
        } else {
            shadowed.push(path);
            shadowed.extend(rest);
        }
    }
    anyhow::ensure!(
        matches.len() <= 1,
        "ambiguous MDB project binding: project={declaring}, PROJ={code:?}, dbnum={dbnum}"
    );
    Ok(matches.pop().map(|(project, path, mut rest)| {
        rest.extend(shadowed);
        Located {
            project,
            path,
            shadowed: rest,
        }
    }))
}

/// ADR-028 inside one project: the highest extract leaf is the working file,
/// the unsuffixed master is the parent layer and only answers when no extract
/// exists. Everything it did not pick comes back rather than being dropped.
///
/// Matching goes through the extract-family parser rather than a name-suffix
/// check. The first cut here was `ends_with("{dbnum}_0001")`, which is wrong
/// twice over: dbnum 100's suffix also tails `ams8100_0001`, handing dbnum 100
/// another database's file without a sound; and a master with no `_NNNN`
/// suffix — or an extract other than `_0001` — is a legal identity for the
/// same logical database, yet never matched at all and got reported as a
/// deployment gap.
fn pick_within_project(dirs: &[PathBuf], dbnum: u32) -> Option<(PathBuf, Vec<PathBuf>)> {
    let mut masters: Vec<PathBuf> = Vec::new();
    let mut leaves: Vec<(u32, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(parsed) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
            else {
                continue;
            };
            if parsed.dbnum != dbnum {
                continue;
            }
            match parsed.extract {
                Some(number) => leaves.push((number, path)),
                None => masters.push(path),
            }
        }
    }
    // `read_dir` order is not a contract; sorting pins the pick when several
    // directories answer.
    leaves.sort();
    masters.sort();

    let mut rest: Vec<PathBuf> = Vec::new();
    let Some(top) = leaves.last().map(|(extract, _)| *extract) else {
        let mut masters = masters.into_iter();
        let winner = masters.next()?;
        rest.extend(masters);
        return Some((winner, rest));
    };
    let (working, older): (Vec<_>, Vec<_>) =
        leaves.into_iter().partition(|(extract, _)| *extract == top);
    let mut working = working.into_iter().map(|(_, path)| path);
    let winner = working
        .next()
        .expect("the top extract has at least one file");
    rest.extend(working);
    rest.extend(older.into_iter().map(|(_, path)| path));
    rest.extend(masters);
    Some((winner, rest))
}

type Cache = Vec<(String, String, Arc<MdbMembership>)>;

fn registry() -> &'static RwLock<Cache> {
    static REGISTRY: std::sync::OnceLock<RwLock<Cache>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Publish a resolved declaration so callers that are nowhere near
/// initialization can read it. Replaces any earlier entry for the same pair.
pub fn install(membership: MdbMembership) -> Arc<MdbMembership> {
    let shared = Arc::new(membership);
    if let Ok(mut cache) = registry().write() {
        let key = (shared.project.clone(), shared.mdb.clone());
        cache.retain(|(project, mdb, _)| (project.clone(), mdb.clone()) != key);
        cache.push((key.0, key.1, shared.clone()));
    }
    shared
}

/// The declaration `init_mdb` resolved, or `None` if it has not run for this
/// pair. Deliberately not a lazy resolve: reading a SYS database is not
/// something that should happen behind an innocent-looking getter.
pub fn get(project: &str, mdb: &str) -> Option<Arc<MdbMembership>> {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    let cache = registry().read().ok()?;
    cache
        .iter()
        .find(|(known_project, known_mdb, _)| known_project == project && *known_mdb == wanted)
        .map(|(_, _, membership)| membership.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_basic_info_log_names_header_and_version_evidence() {
        let header = crate::data_interface::db_file_header::DbFileHeader {
            dbnum: 8191,
            db_type: "SYST".to_string(),
            latest_ses_pgno: 42,
            latest_session: crate::data_interface::db_file_header::SessionStamp {
                sesno: 37,
                pgno: 42,
                previous_pgno: Some(40),
                end_pgno: 43,
                index_root_pgno: 41,
                stamp_words: [0; 5],
                machine: "SITE-A".to_string(),
                comment: "SAVEWORK".to_string(),
            },
            file_size: 123_456,
        };

        let line = sys_basic_info_message(
            Path::new(r"D:\release\9001\project\JEU\JEU000\jeusys"),
            &header,
            "e3d210",
            "e3d210",
        );
        for required in [
            "[SYS 解析] 基本信息",
            r"file=D:\release\9001\project\JEU\JEU000\jeusys",
            "bytes=123456",
            "dbnum=8191",
            "db_type=SYST",
            "latest_sesno=37",
            "latest_ses_pgno=42",
            "session_pgno=42",
            "index_root_pgno=41",
            "selected_schema=e3d210",
            "binary_version=e3d210",
            "machine=\"SITE-A\"",
            "comment=\"SAVEWORK\"",
        ] {
            assert!(line.contains(required), "基本信息缺 {required}: {line}");
        }
    }

    #[test]
    fn mdb_parse_summary_separates_raw_decoded_and_skipped_members() {
        let line = mdb_parse_summary_message(
            Path::new(r"D:\project\JEU000\jeusys"),
            "/ALL",
            RefU64::from_two_nums(24575, 1308),
            71,
            68,
            3,
            &BTreeMap::from([(DESI_STYP, 29), (DICT_STYP, 6)]),
        );
        for required in [
            "[SYS 解析] MDB 解析成功",
            "mdb=/ALL",
            "root=24575/1308",
            "curd_entries=71",
            "decoded_databases=68",
            "skipped_databases=3",
            "styp={1: 29, 8: 6}",
        ] {
            assert!(line.contains(required), "解析摘要缺 {required}: {line}");
        }
    }

    #[test]
    fn sys_member_parse_failures_keep_the_stage_and_reason() {
        let source = include_str!("mdb_membership.rs");
        assert!(
            !source.contains(concat!(".exists()", ".ok()?"))
                && !source.contains(concat!(".extraction()", ".ok()?")),
            "exists/extraction/conversion 错误不能再被 Option 吞掉"
        );
        for required in [
            "stage=element_lookup",
            "stage=attribute_extract",
            "stage=curd_decode",
            "stage=dbno_decode",
            "reason={error:#}",
            "SYS 关键属性未解码",
            "curd_entries=",
            "skipped_databases=",
        ] {
            assert!(source.contains(required), "SYS 解析失败日志缺少 {required}");
        }
    }

    #[test]
    fn project_characters_are_not_a_foreign_project_flag() {
        use e3d_io::record::descriptor::DescriptorValue::RawWords;
        assert_eq!(
            decode_project_code(Some(&RawWords(vec![65, 67, 80])))
                .unwrap()
                .as_deref(),
            Some("ACP")
        );
        assert_eq!(
            decode_project_code(Some(&RawWords(vec![83, 67, 66])))
                .unwrap()
                .as_deref(),
            Some("SCB")
        );
        assert_eq!(decode_project_code(Some(&RawWords(vec![]))).unwrap(), None);
        assert!(decode_project_code(None).is_err());
        assert!(decode_project_code(Some(&RawWords(vec![3]))).is_err());
    }

    #[test]
    fn exact_project_binding_never_falls_back_to_same_dbnum() {
        let own = dir_with(&["ams7000_0001", "ams7355_0001"]);
        let other = dir_with(&["acp7000_0001"]);
        let (projects, _) = two_projects(own.path(), other.path());
        assert_eq!(
            locate_declared(&projects, "Main", None, 7000)
                .unwrap()
                .unwrap()
                .project,
            "Main"
        );
        assert_eq!(
            locate_declared(&projects, "Main", Some("ACP"), 7000)
                .unwrap()
                .unwrap()
                .project,
            "Catalogue"
        );
        assert!(
            locate_declared(&projects, "Main", Some("APS"), 7355)
                .unwrap()
                .is_none()
        );
        assert!(
            locate_declared(&projects, "Missing", None, 7000)
                .unwrap()
                .is_none()
        );
    }

    fn db(dbnum: u32, styp: i64, name: &str, on_disk: bool) -> MdbDatabase {
        MdbDatabase {
            dbnum,
            styp,
            proj: 0,
            declared_project_code: None,
            name: name.into(),
            path: on_disk.then(|| PathBuf::from(format!("/somewhere/x{dbnum}_0001"))),
            project: on_disk.then(|| "AvevaMarineSample".to_string()),
            shadowed: Vec::new(),
        }
    }

    fn membership() -> MdbMembership {
        MdbMembership {
            mdb: "/ALL".into(),
            project: "AvevaMarineSample".into(),
            databases: vec![
                db(6002, DICT_STYP, "*MASTER/SCBDICT", true),
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(5100, DICT_STYP, "*CNPESTD/DICT", true),
                db(7323, DICT_STYP, "*MASTER/MDSDICT", false),
            ],
            problems: Vec::new(),
        }
    }

    /// CURD order decides which definition wins a duplicated UKEY, so the
    /// dictionary list must come back in it rather than sorted by number.
    #[test]
    fn dictionary_paths_keep_curd_order_and_skip_what_is_not_on_disk() {
        let paths = membership().dictionary_paths();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("x6002_0001"));
        assert!(paths[1].ends_with("x5100_0001"));
    }

    /// A declared database with no file is a deployment problem; dropping it
    /// silently is how it stays one.
    #[test]
    fn a_declared_database_with_no_file_is_reported_not_dropped() {
        let membership = membership();
        let missing: Vec<u32> = membership.unresolved().map(|db| db.dbnum).collect();
        assert_eq!(missing, vec![7323]);
        assert_eq!(membership.databases().len(), 4);
    }

    #[test]
    fn missing_design_file_blocks_complete_room_inputs_but_empty_mdb_is_valid() {
        let mut declaration = membership();
        assert!(
            declaration.require_design_files().is_ok(),
            "missing dictionary is a separate check"
        );
        declaration
            .databases
            .push(db(9999, DESI_STYP, "*MASTER/MISSING", false));
        let error = declaration.require_design_files().unwrap_err().to_string();
        assert!(
            error.contains("9999") && error.contains("*MASTER/MISSING") && error.contains("/ALL")
        );
        declaration.databases.clear();
        assert!(
            declaration.require_design_files().is_ok(),
            "authoritative empty declaration differs from a missing file"
        );
    }

    #[test]
    fn counts_are_grouped_by_styp() {
        assert_eq!(
            membership().counts_by_type(),
            BTreeMap::from([(DESI_STYP, 1), (DICT_STYP, 3)])
        );
    }

    #[test]
    fn lookups_normalise_the_leading_slash() {
        install(membership());
        assert!(get("AvevaMarineSample", "ALL").is_some());
        assert!(get("AvevaMarineSample", "/ALL").is_some());
        assert!(get("AvevaMarineSample", "/NOPE").is_none());
        assert!(get("OtherProject", "/ALL").is_none());
    }

    fn dir_with(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in names {
            std::fs::write(dir.path().join(name), b"").expect("touch");
        }
        dir
    }

    /// `ends_with("100_0001")` also tails `ams8100_0001`: a suffix match hands
    /// dbnum 100 another database's file and nothing reports it. The parser
    /// reads the whole number, so 100 finds nothing here and 8100 finds its own.
    #[test]
    fn locate_reads_the_whole_dbnum_not_a_name_suffix() {
        let dir = dir_with(&["ams8100_0001"]);
        let dirs = vec![dir.path().to_path_buf()];

        assert_eq!(
            pick_within_project(&dirs, 100),
            None,
            "8100 的文件不属于 100"
        );
        assert_eq!(
            pick_within_project(&dirs, 8100).map(|(path, _)| path),
            Some(dir.path().join("ams8100_0001"))
        );
    }

    /// An unsuffixed master and a non-`_0001` extract are both legal identities
    /// for the same logical database (ADR-028); the leaf is the working file
    /// and wins over the master. Hand copies never match at all.
    #[test]
    fn locate_accepts_masters_and_other_extracts_and_prefers_the_leaf() {
        let dir = dir_with(&["scb100", "scb100_0002", "scb100_0002 copy"]);
        let dirs = vec![dir.path().to_path_buf()];

        let (winner, shadowed) = pick_within_project(&dirs, 100).expect("解得出");
        assert_eq!(
            winner,
            dir.path().join("scb100_0002"),
            "抽取叶子是工作文件，压过无后缀主库"
        );
        assert_eq!(
            shadowed,
            vec![dir.path().join("scb100")],
            "被压在下面的主库要交出来，不能静默丢掉"
        );

        let master_only = dir_with(&["scb100"]);
        assert_eq!(
            pick_within_project(&[master_only.path().to_path_buf()], 100).map(|(path, _)| path),
            Some(master_only.path().join("scb100")),
            "没有抽取时主库本身就是答案，不该被报成缺件"
        );
    }

    fn two_projects(
        main_dir: &Path,
        other_dir: &Path,
    ) -> (Vec<(String, Vec<PathBuf>)>, Vec<String>) {
        (
            vec![
                ("Main".to_string(), vec![main_dir.to_path_buf()]),
                ("Catalogue".to_string(), vec![other_dir.to_path_buf()]),
            ],
            vec!["Main".into(), "Catalogue".into()],
        )
    }
}

/// SYS 固定结构导航（WORL → MDBW → 成员）的门：真库两条装了 AMS 才跑，文本护栏一条哪儿都跑。
#[cfg(test)]
#[path = "mdb_membership_sys_nav_tests.rs"]
mod sys_nav_tests;
