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
//! A `CURD` entry is not always a `DB`: E3D lets it be a `DBSET`, which
//! `openMDB` unfolds through `DBL.DBF` / `DBSTL.DBSTF` before opening anything
//! (core.dll `CURRFU`). [`expand_curd_dbsets`] does the same here, so a set
//! declared in ADMIN is not read as one declaration with no `DBNO`.
//!
//! SYS PROJ is a character vector (ACP / SCB / APS), not an integer flag.
//! An empty vector selects the declaring project. Never substitute another
//! project merely because it contains a file with the same dbnum.
//!
//! For the configured MDB, [`crate::data_interface::update_scope::UpdateScope`]
//! reads the membership registered here (ADR-085): the execution scope and the
//! Dictionary set come from one SYS declaration. This reads the file, so it is
//! available during initialization — before anything has been synced — which is
//! the point at which the Dictionary set has to be known. The registered copy is
//! rebuilt only by [`rebind`]: at startup, after a SYST batch lands (the
//! declaration is re-read first), and after a share remount.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

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

/// 一个 CURD 声明元素（SYS 侧 `TYPE=DB`）上取到的写权限证据。
///
/// E3D 声明面上没有显式「可写」位：`ACCESSDB` 只在收窄过访问的库上落值
/// （样本项目 AMS 的声明面上整列缺席是常态），`LREFO` 标 reference-only
/// 声明。裁决按会话默认——声明进 MDB 的库默认可写，除非证据收窄它；
/// 读不到声明面的库根本产不出 `DbAccess`，上游按不可读 fail-closed。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbAccess {
    /// `ACCESSDB` 原文（`Read`/`Update`/`Multiwrite`，旧库可能 `Controlled`）。
    /// 缺席是常态——不等于只读。
    pub access: Option<String>,
    /// `LREFO`——reference-only 声明。
    pub reference_only: bool,
    /// `CLAI` 声明面 claim 模式原文。只留证据，不参与裁决——claim 是会话
    /// 动态状态，不是文件面事实。
    pub claim: Option<String>,
}

impl DbAccess {
    /// E3D 会话默认：声明进 MDB 的库可写，除非 `ACCESSDB=Read` 或 `LREFO`
    /// 收窄。`ACCESSDB` 其余枚举值（`Update`/`Multiwrite`/`Controlled`）均可写。
    pub fn writable(&self) -> bool {
        if self.reference_only {
            return false;
        }
        match self.access.as_deref().map(str::trim) {
            Some(access) => !access.eq_ignore_ascii_case("read"),
            None => true,
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
    /// 各 CURD 声明元素上取到的写权限证据（dbnum → [`DbAccess`]）。
    /// 缺席键 = 声明面没留证据，消费方按会话默认可写。
    access: BTreeMap<u32, DbAccess>,
    /// Things that went wrong while resolving this list and that a person
    /// should see. They deliberately do not live only in `log::warn!`: the
    /// sandbox and plenty of deployments run `enable_log = false`, the logger
    /// is then never initialised, and not one of those lines ever appears.
    problems: Vec<String>,
}

impl MdbMembership {
    #[cfg(test)]
    pub(crate) fn test_with_databases(project: &str, mdb: &str, databases: Vec<MdbDatabase>) -> Self {
        Self { mdb: mdb.into(), project: project.into(), databases, access: BTreeMap::new(), problems: Vec::new() }
    }
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

    /// 该库 CURD 声明元素上的写权限证据。`None` = 声明面没留证据（或测试
    /// 构造没喂）——按 E3D 会话默认可写；要 fail-closed 的消费方自己裁决。
    pub fn access_of(&self, dbnum: u32) -> Option<&DbAccess> {
        self.access.get(&dbnum)
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

    /// MDB 声明了、搜索根里却没有文件的 DESI `(dbnum, 名字)`，按声明序。
    ///
    /// 一个缺席的文件里可能有房间或面板——它**不是**一个空库，所以要按号点名而不是当成
    /// 「声明为空」。消费者是常驻房间模型（ADR-073 D8）：按有的文件建、把这份清单带到
    /// `/health.room_build.resident.missing_dbnums` 与启动横幅、生命周期记 `Degraded`。
    /// 此前这里是 `require_design_files()`——缺一个就整份 Err，之后每个任务的全部根判死。
    pub(crate) fn missing_design_files(&self) -> Vec<(u32, String)> {
        self.unresolved()
            .filter(|db| db.styp == DESI_STYP)
            .map(|db| (db.dbnum, db.name.clone()))
            .collect()
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

/// e3d-io 探针读到的版本证据，压成 `{field=… raw_value=… raw_text=… session_page=…}`。
///
/// 探针不给版本结论——目前没有任何库字段被证明能分开 2.1 与 3.1（e3d-io
/// docs/evidence/2026-09-11-e3d-database-version.md）。SYS 库本身不是 COMM，
/// 通常只会给到 `field=none` 与会话页号：这一行的价值在于「e3d-io 能按已知页身份
/// 打开它」，读不出来才是异常。
fn sys_version_evidence_summary(evidence: &e3d_io::DatabaseVersionEvidence) -> String {
    format!(
        "{{field={} raw_value={:?} raw_text={:?} session_page={:?}}}",
        evidence.source.field_name(),
        evidence.raw_value,
        evidence.raw_text,
        evidence.session_page
    )
}

fn sys_basic_info_message(
    sys: &Path,
    header: &crate::data_interface::db_file_header::DbFileHeader,
    selected_schema: &str,
    version_evidence: &str,
) -> String {
    let saved_at = header
        .latest_session
        .rfc3339()
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "[SYS 解析] 基本信息: file={} bytes={} dbnum={} db_type={} latest_sesno={} \
         latest_ses_pgno={} session_pgno={} previous_session_pgno={:?} end_pgno={} \
         index_root_pgno={} saved_at={} user={:?} comment={:?} selected_schema={} \
         version_evidence={}",
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
        header.latest_session.user,
        header.latest_session.comment,
        selected_schema,
        version_evidence
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
///
/// Which projects the declared files may come from is the MDB closure
/// (ADR-078): the declaration is read once (cached by
/// [`crate::data_interface::mdb_closure::declaration`]), every declared
/// project code is located in the configured search roots, and only then is
/// each declaration bound to a file inside its own project's directories.
pub fn resolve(db_option: &DbOption, project: &str, mdb: &str) -> anyhow::Result<MdbMembership> {
    // 绑定前的远程 IO 各打边界：闭包定位（搜索根重扫 + 主项目 SYS 首读，见
    // `mdb_closure::resolve`/`reload_declaration` 内的行）与后面的目录枚举。
    // 远程盘上这三段每一段都能卡——只给 bind_declared 打行的话，
    // 前一段静默照旧看不出停在哪（2026-10-15 JEU 现场的教训）。
    let closure_started = Instant::now();
    report_sys_info(&format!("[mdb-bind] 闭包定位开始: project={project} mdb={mdb}"));
    let closure = crate::data_interface::mdb_closure::resolve_for(db_option);
    let plan = closure.watch_dir_plan();
    report_sys_info(&format!(
        "[mdb-bind] 闭包定位完成: project={project} mdb={mdb} projects={} dirs={} elapsed_ms={}",
        plan.projects.len(),
        plan.dirs().len(),
        closure_started.elapsed().as_millis()
    ));
    let own_dirs = own_db_dirs(&plan, project)?;
    // Kept per project rather than flattened into one directory list: a dbnum
    // is unique inside a project only, so "which file" cannot be answered
    // before "which project".
    let projects: Vec<(String, Vec<PathBuf>)> = plan
        .projects
        .iter()
        .filter(|entry| !entry.db_dirs.is_empty())
        .map(|entry| (entry.project.clone(), entry.db_dirs.clone()))
        .collect();
    let declaration = crate::data_interface::mdb_closure::declaration(&own_dirs, project, mdb)?;
    let bound = bind_declared(&declaration.databases, &projects, project)?;
    let databases = bound.databases;
    let mut problems = declaration.problems.clone();
    problems.extend(bound.problems);
    for db in databases.iter().filter(|db| db.path.is_none()) {
        let reason = db
            .declared_project_code
            .as_deref()
            .and_then(|code| {
                closure
                    .unresolved
                    .iter()
                    .find(|unresolved| unresolved.code.eq_ignore_ascii_case(code))
                    .map(|unresolved| unresolved.reason.clone())
            })
            .unwrap_or_else(|| "项目目录已定位，但里面没有这个 dbnum 的文件".to_string());
        problems.push(format!(
            "MDB {}: dbnum={} ({}) 属于项目 {}，没有绑定到文件：{reason}；不拿别的项目里同号的文件顶替",
            declaration.mdb,
            db.dbnum,
            db.name,
            db.declared_project_code.as_deref().unwrap_or(project)
        ));
    }
    Ok(MdbMembership {
        mdb: declaration.mdb.clone(),
        project: project.to_string(),
        databases,
        access: declaration.access.clone(),
        problems,
    })
}

/// `project`'s own database directories in `plan` — where its SYS file lives.
/// Empty is an error rather than an empty answer: there is no SYS to read.
fn own_db_dirs(
    plan: &crate::data_interface::project_paths::WatchDirPlan,
    project: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    let own_dirs: Vec<PathBuf> = plan
        .projects
        .iter()
        .filter(|entry| entry.project == project)
        .flat_map(|entry| entry.db_dirs.iter().cloned())
        .collect();
    if own_dirs.is_empty() {
        anyhow::bail!(
            "项目 {project} 没有解析出任何库目录，无从读取它的 SYS 库；监控目录解析：{}",
            plan.problems().join(" / ")
        );
    }
    Ok(own_dirs)
}

/// A declaration read from SYS with nothing bound yet: every entry's `path` /
/// `project` / `shadowed` is empty. The MDB closure only needs the project
/// codes; binding happens in [`resolve`] once the closure has located the
/// project directories.
pub struct UnboundDeclaration {
    pub sys: PathBuf,
    pub databases: Vec<MdbDatabase>,
    /// 各声明元素上取到的写权限证据（dbnum → [`DbAccess`]），随声明一起走
    /// 到 [`MdbMembership::access_of`]。绑定只换文件归属，不换声明语义。
    pub access: BTreeMap<u32, DbAccess>,
    pub problems: Vec<String>,
}

/// Find the SYS file among `own_dirs` that declares `mdb` and read the
/// declaration without binding files. The error on a miss lists every MDB
/// name the SYS files do declare, so a misspelled `mdb_name` is diagnosable.
pub(crate) fn read_unbound_declaration(
    own_dirs: &[PathBuf],
    project: &str,
    mdb: &str,
) -> anyhow::Result<UnboundDeclaration> {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    let mut tried = Vec::new();
    let mut candidate_names = BTreeSet::new();
    let mut candidate_scan_errors = Vec::new();
    let mut declaration_errors = Vec::new();
    let mut problems = Vec::new();
    for sys in sys_candidates(own_dirs) {
        tried.push(sys.display().to_string());
        match read_declaration(&sys, project, &wanted, &[]) {
            Ok(Some((databases, expansion_problems, access))) => {
                problems.extend(expansion_problems);
                return Ok(UnboundDeclaration {
                    sys,
                    databases,
                    access,
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

/// `bind_declared` 的产物：绑定完的清单 + 目录枚举期的问题。
///
/// 目录枚举失败不走错误返回——那个目录缺席而已，其余目录照常绑定；但「读不动」
/// 与「里面没有」在下游都长成「声明了没文件」，所以失败必须以 problems 的形式进
/// membership，跟声明解析问题同一个出口。
pub(crate) struct BindOutcome {
    pub databases: Vec<MdbDatabase>,
    pub problems: Vec<String>,
    /// 实际枚举过的目录数（`DirInventory` 全程的磁盘触点）：「每目录一次」的
    /// 结构性断言钉在它上面。
    pub dirs_scanned: usize,
    /// 收到的文件条目总数，进完成行的体量证据。
    pub entries: usize,
}

/// Bind an unbound declaration to files: each entry is looked up only inside
/// the project its `PROJ` names (`None` = `declaring`), through
/// [`locate_declared`]. Entries whose project has no directory in `projects`
/// stay unbound (`path: None`) and are reported by the caller; a same-numbered
/// file in another project is never substituted.
///
/// 磁盘触点集中在开头的 `DirInventory::from_dirs`：每个项目目录各枚举**一次**，
/// 之后几百条声明全部在内存索引上定位。曾经这里是逐声明 `read_dir` + 逐文件
/// `is_file()`——共享盘上是几百次全枚举加几十万次 stat 往返，SMB 半死时单次调用
/// 无超时可无限阻塞（2026-10-15 现场：JEU /ALL 480 库，UNC 根，启动静默于此）。
pub(crate) fn bind_declared(
    databases: &[MdbDatabase],
    projects: &[(String, Vec<PathBuf>)],
    declaring: &str,
) -> anyhow::Result<BindOutcome> {
    let started = Instant::now();
    report_sys_info(&format!(
        "[mdb-bind] 成员绑定开始: declaring={declaring} declarations={} projects={} dirs={}",
        databases.len(),
        projects.len(),
        projects.iter().map(|(_, dirs)| dirs.len()).sum::<usize>()
    ));
    let inventories: Vec<(String, DirInventory)> = projects
        .iter()
        .map(|(project, dirs)| (project.clone(), DirInventory::from_dirs(project, dirs)))
        .collect();
    let scan_problems: Vec<String> = inventories
        .iter()
        .flat_map(|(_, inventory)| inventory.problems.iter().cloned())
        .collect();
    let dirs_scanned = inventories.iter().map(|(_, i)| i.dirs_scanned).sum();
    let entries = inventories.iter().map(|(_, i)| i.entries).sum();
    let mut bound = Vec::with_capacity(databases.len());
    for db in databases {
        let located = locate_declared(
            &inventories,
            declaring,
            db.declared_project_code.as_deref(),
            db.dbnum,
        )
        .with_context(|| {
            format!(
                "定位 MDB 声明 dbnum={}（{}，项目 {}）的文件失败",
                db.dbnum,
                db.name,
                db.declared_project_code.as_deref().unwrap_or(declaring)
            )
        })?;
        bound.push(MdbDatabase {
            dbnum: db.dbnum,
            styp: db.styp,
            proj: db.proj,
            declared_project_code: db.declared_project_code.clone(),
            name: db.name.clone(),
            path: located.as_ref().map(|found| found.path.clone()),
            project: located.as_ref().map(|found| found.project.clone()),
            shadowed: located.map(|found| found.shadowed).unwrap_or_default(),
        });
    }
    let unbound = bound.iter().filter(|db| db.path.is_none()).count();
    report_sys_info(&format!(
        "[mdb-bind] 成员绑定完成: declaring={declaring} declarations={} bound={} unbound={unbound} \
         dirs_scanned={dirs_scanned} entries={entries} elapsed_ms={}",
        databases.len(),
        bound.len() - unbound,
        started.elapsed().as_millis()
    ));
    Ok(BindOutcome {
        databases: bound,
        problems: scan_problems,
        dirs_scanned,
        entries,
    })
}

/// Files whose name ends in `sys`, which is how E3D names the SYSTEM database
/// (`amssys`, `acpsys`). Sibling `*mis` / `*com` / `*glb` files are not MDB
/// roots; [`SysMetadataResolver`] still indexes them for cross-database refs.
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

/// What the CURD expansion needs from a SYS element, narrowed so the walk can be
/// exercised against an in-memory tree in tests without a database file.
trait CurdElements {
    /// Noun name of the element, `None` when it is not in the pinned session's index.
    fn element_type(&self, refno: RefU64) -> anyhow::Result<Option<String>>;
    fn member_refnos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>>;
    /// A reference attribute; `None` for unset, Nulref, or an attribute the noun
    /// does not carry (a `DBL` without `DBF` is reported by the caller, not here).
    fn get_ref(&self, refno: RefU64, attribute: &str) -> anyhow::Result<Option<RefU64>>;
    /// Whether the DBSET carries a `DBCRITERIA` expression.
    fn has_db_criteria(&self, refno: RefU64) -> anyhow::Result<bool>;
}

/// One database element of the expanded current list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CurdEntry {
    refno: RefU64,
    /// `None` for a `DB` listed directly in `CURD`; otherwise the DBSET chain
    /// that pulled it in, for diagnostics.
    via: Option<String>,
}

#[derive(Debug, Default)]
struct CurdExpansion {
    entries: Vec<CurdEntry>,
    /// How many raw `CURD` entries were DBSETs.
    dbset_entries: usize,
    /// Anything the walk had to skip or approximate; a person should see these.
    problems: Vec<String>,
}

/// `MDB.CURD` as E3D opens it: what core.dll's `CURRFU` pseudo-attribute yields
/// (E3D 2.1 `DB_PseudoCURRFULLonMDB::get` `0x105C06B0` feeding `DB_MDB::openMDB`).
///
/// A `DB` entry passes through. A `DBSET` entry is replaced by the databases its
/// `DBL` members reference through `DBF`, nested sets reached through
/// `DBSTL.DBSTF` included (`DBSLIS` `0x105C03B0` / `0x105C0010`). Each set is
/// entered once, so a cycle terminates, and each database is kept at its first
/// position: E3D dedupes inside one set, and `DB_DB::construct` reuses the
/// already-open `DB_DB` for a repeat, so one occurrence is what it ends up with.
///
/// Anything that is neither `DB` nor `DBSET` — including an entry missing from
/// the session index — is left in place so the per-database decode reports it
/// with its usual stage. What this does not reproduce is `DBCRITERIA`: E3D also
/// pulls in every database matching that expression (`sub_105BD020`), which is
/// reported as a problem rather than silently approximated.
fn expand_curd_dbsets<E: CurdElements>(
    elements: &E,
    curd: &[RefU64],
) -> anyhow::Result<CurdExpansion> {
    let mut out = CurdExpansion::default();
    let mut seen_dbs: BTreeSet<RefU64> = BTreeSet::new();
    let mut seen_dbsets: BTreeSet<RefU64> = BTreeSet::new();
    for (index, refno) in curd.iter().enumerate() {
        let noun = elements.element_type(*refno)?;
        if noun.as_deref() == Some("DBSET") {
            out.dbset_entries += 1;
            let chain = format!("CURD[{index}] DBSET {}/{}", refno.get_0(), refno.get_1());
            expand_one_dbset(
                elements,
                *refno,
                &chain,
                &mut seen_dbs,
                &mut seen_dbsets,
                &mut out,
            )?;
            continue;
        }
        if seen_dbs.insert(*refno) {
            out.entries.push(CurdEntry {
                refno: *refno,
                via: None,
            });
        } else {
            out.problems.push(format!(
                "CURD[{index}] {}/{} 已在前面出现（直接声明或经 DBSET），按首次位置保留一份",
                refno.get_0(),
                refno.get_1()
            ));
        }
    }
    Ok(out)
}

fn expand_one_dbset<E: CurdElements>(
    elements: &E,
    dbset: RefU64,
    chain: &str,
    seen_dbs: &mut BTreeSet<RefU64>,
    seen_dbsets: &mut BTreeSet<RefU64>,
    out: &mut CurdExpansion,
) -> anyhow::Result<()> {
    if !seen_dbsets.insert(dbset) {
        return Ok(());
    }
    if elements.has_db_criteria(dbset)? {
        out.problems.push(format!(
            "{chain} 带 DBCRITERIA 动态条件；本实现只展开 DBL/DBSTL 静态成员，E3D 还会把满足条件的库补进来，\
             该 DBSET 可能少库"
        ));
    }
    for member in elements.member_refnos(dbset)? {
        let member_id = format!("{}/{}", member.get_0(), member.get_1());
        match elements.element_type(member)?.as_deref() {
            Some("DBL") => match elements.get_ref(member, "DBF")? {
                Some(db) => {
                    if seen_dbs.insert(db) {
                        out.entries.push(CurdEntry {
                            refno: db,
                            via: Some(chain.to_string()),
                        });
                    }
                }
                None => out.problems.push(format!(
                    "{chain} 成员 DBL {member_id} 没有 DBF 引用，已跳过"
                )),
            },
            Some("DBSTL") => match elements.get_ref(member, "DBSTF")? {
                Some(nested) => {
                    let nested_chain =
                        format!("{chain} > DBSET {}/{}", nested.get_0(), nested.get_1());
                    expand_one_dbset(elements, nested, &nested_chain, seen_dbs, seen_dbsets, out)?;
                }
                None => out.problems.push(format!(
                    "{chain} 成员 DBSTL {member_id} 没有 DBSTF 引用，已跳过"
                )),
            },
            Some(other) => out.problems.push(format!(
                "{chain} 成员 {member_id} 类型 {other} 不是 DBL/DBSTL，已跳过"
            )),
            None => out.problems.push(format!(
                "{chain} 成员 {member_id} 不在最新会话索引中，已跳过"
            )),
        }
    }
    Ok(())
}

/// `Ok(None)` when the file parses but holds no MDB of that name — the caller
/// moves on to the next candidate rather than failing. `Ok(Some)` carries the
/// databases plus the problems the CURD expansion had to report.
fn read_declaration(
    sys: &Path,
    project: &str,
    wanted: &str,
    projects: &[(String, DirInventory)],
) -> anyhow::Result<Option<(Vec<MdbDatabase>, Vec<String>, BTreeMap<u32, DbAccess>)>> {
    // e3d-io 读法（parse_pdms_db 退役清单 §B5）：SYS 库按最新会话钉进 `DbSet`
    // （模板 `sysvir.dat`）；Global Project 的 MDB 在本地 SYS、DB 声明在同目录 GLB，
    // `DbSet` 经文件头索引按 refno 自带的 dbno 惰性补开后者。MDB 元素按 SYS 的固定结构
    // 导航找（WORL `24575/3` → MDBW `24575/7` → 成员，逐个点查核 noun 与 NAME，零扫描；
    // 见 `SysDb::mdbs_named`），属性经 `direct_attmap::to_named_attmap` 落成与旧
    // `whole_attmap.merge()` 同一形状的 `NamedAttrMap`——下面对 `CURD` / `DBNO` / `PROJ` /
    // `STYP` / `NAME` 的读法逐字不变。
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
    // `curd_index` below is the position in this expanded list, not in the raw
    // CURD, once a DBSET has been unfolded into its databases.
    let expansion = expand_curd_dbsets(&sys_db, &members).with_context(|| {
        format!(
            "展开 SYS {} 的 MDB {wanted}（root={}/{}）CURD 里的 DBSET",
            sys.display(),
            root.get_0(),
            root.get_1()
        )
    })?;
    let mut problems = Vec::with_capacity(expansion.problems.len());
    for problem in expansion.problems {
        let message = format!(
            "[SYS 解析] CURD DBSET 展开: file={} mdb={} root={}/{} {problem}",
            sys.display(),
            wanted,
            root.get_0(),
            root.get_1()
        );
        report_sys_error(&message);
        problems.push(message);
    }
    if expansion.dbset_entries > 0 {
        report_sys_info(&format!(
            "[SYS 解析] CURD DBSET 展开完成: file={} mdb={} root={}/{} curd_entries={} \
             dbset_entries={} expanded_entries={}",
            sys.display(),
            wanted,
            root.get_0(),
            root.get_1(),
            curd_entries,
            expansion.dbset_entries,
            expansion.entries.len()
        ));
    }
    let members = expansion.entries;
    let mut databases = Vec::with_capacity(members.len());
    let mut access = BTreeMap::new();
    let mut skipped_databases = 0usize;
    for (curd_index, entry) in members.into_iter().enumerate() {
        let refno = entry.refno;
        // Where the entry came from: `CURD` itself, or the DBSET chain that unfolded to it.
        let origin = entry.via.as_deref().unwrap_or("CURD");
        let merged = match sys_db.attrs(refno) {
            Ok(Some(merged)) => merged,
            Ok(None) => {
                skipped_databases += 1;
                report_sys_error(&format!(
                    "[SYS 解析] DB 声明解析失败: file={} mdb={} root={}/{} curd_index={} \
                     refno={}/{} origin={origin} stage=element_lookup reason=元素不在最新会话索引中",
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
                     refno={}/{} origin={origin} stage=attribute_extract reason={error:#}",
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
                 refno={}/{} origin={origin} stage=dbno_decode reason=DBNO 缺失、不是整数或超出 u32 \
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
        // 写权限证据在同一声明元素上：`ACCESSDB`/`LREFO`/`CLAI`。样本项目
        // 整列缺席是常态——缺席按会话默认可写，见 `DbAccess::writable`。
        access.insert(
            dbnum,
            DbAccess {
                access: merged.get_as_string("ACCESSDB"),
                reference_only: matches!(
                    merged.get_val("LREFO"),
                    Some(NamedAttrValue::BoolType(true))
                ),
                claim: merged.get_as_string("CLAI"),
            },
        );
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
    Ok(Some((databases, problems, access)))
}

/// Same-directory project administration files, keyed by the dbnum read from
/// each file's descriptor page.
///
/// A Global Project stores the MDB element in `<project>sys`, while the DB
/// elements referenced by `MDB.CURD` live in `<project>glb`.  Their reserved
/// numbers are installation data, not parser constants: discover every
/// SYS/COMM/MISC/GLB sibling by name (the Global header type is `GLOB`), then
/// trust only its file header for the routing key, database type and pinned
/// session.
#[derive(Debug, Clone)]
struct SysMetadataResolver {
    source: PathBuf,
    by_dbno: BTreeMap<u32, e3d_io::DbFilePin>,
}

impl SysMetadataResolver {
    fn discover(sys: &Path, template_dir: &Path) -> anyhow::Result<Self> {
        let directory = sys
            .parent()
            .ok_or_else(|| anyhow::anyhow!("SYS 路径没有父目录: {}", sys.display()))?;
        let entries = std::fs::read_dir(directory)
            .with_context(|| format!("扫描 SYS 同目录元数据库 {}", directory.display()))?;
        let mut headers = Vec::new();
        let mut unreadable = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    unreadable.push(format!("目录项读取失败: {error}"));
                    continue;
                }
            };
            let path = entry.path();
            let is_file = entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or_else(|_| path.is_file());
            if !is_file {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !crate::data_interface::increment_manager::is_project_runtime_db_file_name(name) {
                continue;
            }
            match crate::data_interface::db_file_header::DbFileHeader::read(&path) {
                Ok(header)
                    if crate::data_interface::increment_manager::PROJECT_RUNTIME_SYS_TYPES
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&header.db_type)) =>
                {
                    headers.push((path, header));
                }
                Ok(header) => unreadable.push(format!(
                    "{} 名字符合项目元数据库，但文件自报类型为 {}，已跳过",
                    path.display(),
                    header.db_type
                )),
                Err(error) => unreadable.push(format!("{}: {error:#}", path.display())),
            }
        }
        for problem in unreadable {
            report_sys_error(&format!(
                "[SYS 解析] 同目录元数据库头读取失败（继续使用其余文件）: \
                 source={} reason={problem}",
                sys.display()
            ));
        }
        let resolver = Self::from_headers(sys, template_dir, headers)?;
        let inventory = resolver
            .by_dbno
            .iter()
            .map(|(dbno, pin)| {
                format!(
                    "{dbno}:{}@{}",
                    pin.db_type.as_deref().unwrap_or("unknown"),
                    pin.file.display()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        report_sys_info(&format!(
            "[SYS 解析] 同目录元数据库索引: source={} entries={} files=[{}]",
            sys.display(),
            resolver.by_dbno.len(),
            inventory
        ));
        Ok(resolver)
    }

    fn from_headers(
        sys: &Path,
        template_dir: &Path,
        headers: impl IntoIterator<
            Item = (PathBuf, crate::data_interface::db_file_header::DbFileHeader),
        >,
    ) -> anyhow::Result<Self> {
        let mut by_dbno: BTreeMap<u32, e3d_io::DbFilePin> = BTreeMap::new();
        for (path, header) in headers {
            let dbno = u32::try_from(header.dbnum).with_context(|| {
                format!(
                    "项目元数据库 {} 自报非法 dbnum={}",
                    path.display(),
                    header.dbnum
                )
            })?;
            anyhow::ensure!(dbno > 0, "项目元数据库 {} 自报非法 dbnum=0", path.display());
            let template = e3d_io::db_element::template_file_for(template_dir, &header.db_type)
                .with_context(|| {
                    format!(
                        "为项目元数据库 {}（dbnum={dbno}, db_type={}）选择模板",
                        path.display(),
                        header.db_type
                    )
                })?;
            let latest_sesno = header.latest_sesno();
            let db_type = header.db_type;
            let pin = e3d_io::DbFilePin {
                file: path.clone(),
                template,
                db_type: Some(db_type),
                sesno: Some(latest_sesno),
            };
            if let Some(existing) = by_dbno.insert(dbno, pin) {
                anyhow::bail!(
                    "SYS {} 同目录发现两个自报 dbnum={} 的项目元数据库：{} / {}；\
                     拒绝按文件名或遍历顺序任选一个",
                    sys.display(),
                    dbno,
                    existing.file.display(),
                    path.display()
                );
            }
        }
        Ok(Self {
            source: sys.to_path_buf(),
            by_dbno,
        })
    }
}

impl e3d_io::DbFileResolver for SysMetadataResolver {
    fn resolve(&self, dbno: u32) -> Option<e3d_io::DbFilePin> {
        let pin = self.by_dbno.get(&dbno)?.clone();
        report_sys_info(&format!(
            "[SYS 解析] 跨库元数据按文件头定位成功: source={} requested_dbno={} \
             file={} db_type={} pinned_sesno={:?}",
            self.source.display(),
            dbno,
            pin.file.display(),
            pin.db_type.as_deref().unwrap_or("unknown"),
            pin.sesno
        ));
        Some(pin)
    }
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
        // 版本证据只进基本信息那一行，不裁决。e3d-io 2c6f11d 起自动识别一律不可用
        // （页身份 / WORL.UPGDNO / COMM.VERS 都分不开 2.1 与 3.1），schema 只认
        // DbOption 的 e3d_version；从前这里那句「版本不匹配」比的是一条已被推翻的规则，
        // 现场看到它只会去改一个本来对的配置。探针开不了文件才值得上 stderr。
        let version_evidence = match e3d_io::database_version_evidence(sys) {
            Ok(evidence) => sys_version_evidence_summary(&evidence),
            Err(error) => {
                report_sys_error(&format!(
                    "[SYS 解析] 二进制版本证据读取失败（仅诊断，不影响解析）: file={} reason={error}",
                    sys.display()
                ));
                "unavailable".to_string()
            }
        };
        let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()
            .with_context(|| format!("为 SYS {} 加载 schema", sys.display()))?;
        let selected_schema = schema
            .version()
            .map(|version| version.id)
            .unwrap_or("explicit-directory");
        let resolver = SysMetadataResolver::discover(sys, schema.template_dir())?;
        let set = Arc::new(
            e3d_io::DbSet::with_attlib_file_and_resolver(
                schema.template_dir().join("attlib.dat"),
                Box::new(resolver),
            )
            .map_err(|error| anyhow::anyhow!("打开 SYS attlib 失败: {error}"))?,
        );
        let template =
            e3d_io::db_element::template_file_for(schema.template_dir(), &header.db_type).map_err(
                |error| anyhow::anyhow!("SYS 库类型 {} 找不到模板: {error}", header.db_type),
            )?;
        set.add_db(e3d_io::DbFilePin {
            file: sys.to_path_buf(),
            template,
            db_type: Some(header.db_type.clone()),
            sesno: Some(header.latest_sesno()),
        })
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
            &version_evidence,
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

impl SysDb {
    fn element(&self, refno: RefU64) -> e3d_io::DbElement {
        self.set
            .element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()))
    }
}

impl CurdElements for SysDb {
    fn element_type(&self, refno: RefU64) -> anyhow::Result<Option<String>> {
        let element = self.element(refno);
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
        element.element_type().map(Some).map_err(|error| {
            anyhow::anyhow!(
                "读取 SYS 元素 {}/{} 类型失败: {error}",
                refno.get_0(),
                refno.get_1()
            )
        })
    }

    fn member_refnos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        Ok(self
            .element(refno)
            .member_refnos()
            .map_err(|error| {
                anyhow::anyhow!(
                    "读取 SYS 元素 {}/{} 成员失败: {error}",
                    refno.get_0(),
                    refno.get_1()
                )
            })?
            .into_iter()
            .map(|member| RefU64::from_two_nums(member.word0, member.word1))
            .collect())
    }

    fn get_ref(&self, refno: RefU64, attribute: &str) -> anyhow::Result<Option<RefU64>> {
        match self.element(refno).get_ref(attribute) {
            Ok(target) => {
                Ok(target.map(|target| RefU64::from_two_nums(target.word0, target.word1)))
            }
            // The noun has no such attribute: the caller reports the member as unusable.
            Err(e3d_io::DbElementError::UnknownAttribute { .. }) => Ok(None),
            Err(error) => Err(anyhow::anyhow!(
                "读取 SYS 元素 {}/{} 的 {attribute} 失败: {error}",
                refno.get_0(),
                refno.get_1()
            )),
        }
    }

    fn has_db_criteria(&self, refno: RefU64) -> anyhow::Result<bool> {
        use e3d_io::record::descriptor::DescriptorValue;
        match self.element(refno).attribute("DBCRITERIA") {
            Ok(None) => Ok(false),
            // The stored expression is an integer word array; an all-zero array is
            // the template default, not a criterion.
            Ok(Some(DescriptorValue::IntArray(words))) => Ok(words.iter().any(|word| *word != 0)),
            Ok(Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words))) => {
                Ok(words.iter().any(|word| *word != 0))
            }
            Ok(Some(_)) => Ok(true),
            Err(e3d_io::DbElementError::UnknownAttribute { .. }) => Ok(false),
            Err(error) => Err(anyhow::anyhow!(
                "读取 SYS 元素 {}/{} 的 DBCRITERIA 失败: {error}",
                refno.get_0(),
                refno.get_1()
            )),
        }
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

/// 单个库目录的枚举限时，与根扫描同值：掉线 SMB 的 `read_dir` 实测挂 20–40 s
/// （`project_locator` 头注），15 s 给慢共享盘留了余量，又比「等它自然报错」短得多。
const DIR_BIND_SCAN_TIMEOUT: Duration =
    crate::data_interface::project_locator::DEFAULT_ROOT_SCAN_TIMEOUT;

/// 一个项目若干库目录的一次性快照：`dbnum → (extract 号, 文件路径)`。
///
/// 存在的原因：`bind_declared` 要给几百个声明库各挑一份文件，而库名里读不出
/// dbnum→文件的可靠映射（后缀匹配会撞号，见 [`pick_within_project`] 的注释），只能
/// 枚举目录逐名解析。「每问一个库就重扫一次目录、对每个目录项各发一次 stat」在
/// 共享盘（UNC）上等于几百次全枚举加几十万次 SMB 往返——2026-10-15 现场
/// （JEU /ALL 480 库，`\\服务器\共享` 根）启动在这一步静默几十分钟；SMB 会话
/// 半死时单次 `read_dir`/`is_file` 无超时可无限挂死。磁盘 IO 收成**每目录一次
/// 枚举**之后，定位全部走内存。
#[derive(Debug, Default)]
struct DirInventory {
    /// `dbnum → (extract 号, 路径)`；无后缀 master 的 extract 是 `None`。
    /// 一个 dbnum 可能对应多份文件（各层 extract + master），全留着交给
    /// [`pick_within_project`] 按原规则挑。
    by_dbnum: BTreeMap<u32, Vec<(Option<u32>, PathBuf)>>,
    /// 枚举不动 / 超时的目录及原因——进 `BindOutcome::problems` 再上 membership。
    problems: Vec<String>,
    /// 成功枚举的目录数与文件条目总数：诊断计数，进 `[mdb-bind]` 明细行。
    dirs_scanned: usize,
    entries: usize,
}

impl DirInventory {
    /// 逐目录各枚举一遍收齐库文件名。`dirs` 内顺序枚举；单目录失败只让它自己
    /// 缺席并记 `problems`，不连累其余目录（与 [`scan_roots`] 各根独立的纪律一致）。
    fn from_dirs(project: &str, dirs: &[PathBuf]) -> Self {
        let mut inventory = Self::default();
        for dir in dirs {
            // 枚举前先出声：远程盘上 `read_dir` 真卡死时，15 s 超时到达之前这一句是唯一能
            // 指着「哪个目录在卡」的证据——只有完成行的话，现场又是一段无特征静默。
            report_sys_info(&format!(
                "[mdb-bind] 目录枚举开始: project={project} dir={}",
                dir.display()
            ));
            let started = Instant::now();
            match scan_db_dir(dir, DIR_BIND_SCAN_TIMEOUT) {
                Ok(paths) => {
                    let entry_count = paths.len();
                    let mut kept = 0usize;
                    inventory.dirs_scanned += 1;
                    inventory.entries += entry_count;
                    for path in paths {
                        let Some(parsed) = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .and_then(
                                crate::data_interface::extract_family::parse_extract_file_name,
                            )
                        else {
                            continue;
                        };
                        kept += 1;
                        inventory
                            .by_dbnum
                            .entry(parsed.dbnum)
                            .or_default()
                            .push((parsed.extract, path));
                    }
                    report_sys_info(&format!(
                        "[mdb-bind] 目录枚举完成: project={project} dir={} entries={entry_count} db_files={kept} elapsed_ms={}",
                        dir.display(),
                        started.elapsed().as_millis()
                    ));
                }
                Err(error) => {
                    let problem = format!("库目录 {}（项目 {project}）枚举失败：{error}", dir.display());
                    report_sys_error(&format!("[mdb-bind] {problem}"));
                    inventory.problems.push(problem);
                }
            }
        }
        inventory
    }
}

/// 单个库目录的限时枚举：工作线程干活、调用方 `recv_timeout` 等截止，超时按该
/// 目录不可达处理，线程自行收尾（与 `scan_roots` 同一模式——接收方放弃后发送
/// 自然落空）。
///
/// 目录项判文件用枚举自带的 `entry.file_type()`（SMB 上零额外往返）；`file_type`
/// 读不到才回落 `path.is_file()` 补一次 stat——`SysMetadataResolver::discover` 与
/// `project_locator::subdirs` 已是这个写法。
fn scan_db_dir(dir: &Path, timeout: Duration) -> Result<Vec<PathBuf>, String> {
    let (sender, receiver) = mpsc::channel();
    let dir_owned = dir.to_path_buf();
    std::thread::Builder::new()
        .name(format!("mdb-bind-scan {}", dir.display()))
        .spawn(move || {
            let result = match std::fs::read_dir(&dir_owned) {
                Ok(entries) => Ok(entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_type()
                            .map(|kind| kind.is_file())
                            .unwrap_or_else(|_| entry.path().is_file())
                    })
                    .map(|entry| entry.path())
                    .collect::<Vec<PathBuf>>()),
                Err(error) => Err(error.to_string()),
            };
            let _ = sender.send(result);
        })
        .map_err(|error| format!("起不了目录枚举线程: {error}"))?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(format!(
            "枚举 {} s 未答，按目录不可达处理（共享盘掉线时 read_dir 会挂很久）",
            timeout.as_secs()
        )),
    }
}

/// Bind only the declared project. A similarly numbered file in another
/// project is diagnostic evidence, never a fallback input.
fn locate_declared(
    projects: &[(String, DirInventory)],
    declaring: &str,
    code: Option<&str>,
    dbnum: u32,
) -> anyhow::Result<Option<Located>> {
    let mut matches = Vec::new();
    let mut shadowed = Vec::new();
    for (project, inventory) in projects {
        let Some((path, rest)) = pick_within_project(inventory, dbnum) else {
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
///
/// 纯内存操作：目录枚举在 [`DirInventory::from_dirs`] 已经做完，这里只查索引。
/// 排序口径不变——`leaves`/`masters` 各自排序、最高 extract 叶胜出、其余全进
/// `rest`——与枚举先后无关，所以索引化不改动任何选主结果。
fn pick_within_project(
    inventory: &DirInventory,
    dbnum: u32,
) -> Option<(PathBuf, Vec<PathBuf>)> {
    let mut masters: Vec<PathBuf> = Vec::new();
    let mut leaves: Vec<(u32, PathBuf)> = Vec::new();
    for (extract, path) in inventory.by_dbnum.get(&dbnum)? {
        match extract {
            Some(number) => leaves.push((*number, path.clone())),
            None => masters.push(path.clone()),
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

/// The membership last bound for this pair — by [`rebind`] at startup, after a
/// SYST batch landed, or after a remount — or `None` if none has been. Deliberately
/// not a lazy resolve: reading a SYS database is not something that should happen
/// behind an innocent-looking getter.
pub fn get(project: &str, mdb: &str) -> Option<Arc<MdbMembership>> {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    let cache = registry().read().ok()?;
    cache
        .iter()
        .find(|(known_project, known_mdb, _)| known_project == project && *known_mdb == wanted)
        .map(|(_, _, membership)| membership.clone())
}

/// 这一次为什么要重做**成员绑定**——打在 `mdb_scope` 阶段行的 `reason=` 上（ADR-085 D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebindReason {
    /// 启动时 `init_mdb` 那一次。
    Startup,
    /// SYST 批次落库后重读了声明（ADR-085 D2）。
    SystRefresh,
    /// 共享盘重挂轮补上了目录（ADR-085 D3）。
    Remount,
    /// 取范围时登记表里没有（`init_mdb` 失败过），现做一次（ADR-085 D1）。
    Fallback,
}

impl RebindReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::SystRefresh => "syst-refresh",
            Self::Remount => "remount",
            Self::Fallback => "fallback",
        }
    }
}

/// 重做一次**成员绑定**并登记（ADR-085）：[`resolve`] → [`install`]，打一行 `mdb_scope` 阶段行。
///
/// 全仓只有这里打那一行：取范围（`UpdateScope::fetch`）只读登记表、不经过这里，所以稳态下它一行不出——
/// 2026-09-29 现场那种「每取一次范围就重扫一遍盘、打一行」正是取范围直接调 [`resolve`] 的结果。
/// 声明本身取 `mdb_closure` 的缓存，这里不重读 SYS；要换声明先走 [`reload_declaration_and_rebind`]。
///
/// 同步 I/O（重扫搜索根、逐库 `read_dir`），async 侧必须放进 `spawn_blocking`。
pub fn rebind(
    db_option: &DbOption,
    project: &str,
    mdb: &str,
    reason: RebindReason,
) -> anyhow::Result<Arc<MdbMembership>> {
    let started = std::time::Instant::now();
    let previous = get(project, mdb);
    let membership = install(resolve(db_option, project, mdb)?);
    println!(
        "{}",
        rebind_phase_line(
            &membership,
            previous.as_deref(),
            reason,
            started.elapsed().as_millis()
        )
    );
    if reason == RebindReason::SystRefresh
        && let Some(notice) =
            retired_element_dbnums_notice(&removed_element_dbnums(&membership, previous.as_deref()))
    {
        println!("{notice}");
    }
    Ok(membership)
}

/// 新声明比旧声明少了的元素库（DESI / ISOD），升序。没有旧登记（首次绑定）时为空。
fn removed_element_dbnums(
    membership: &MdbMembership,
    previous: Option<&MdbMembership>,
) -> Vec<u32> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    let now: BTreeSet<u32> = membership
        .element_databases()
        .map(|database| database.dbnum)
        .collect();
    previous
        .element_databases()
        .map(|database| database.dbnum)
        .filter(|dbnum| !now.contains(dbnum))
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect()
}

/// SYST 刷新移出了元素库时的那一行（GitHub #41，ADR-078 追记 2026-09-30）：它们离开本期执行范围，库里的
/// 数据 / 模型面状态与水位行**不自动清**；下次启动在 `/health.startup.reconcile.retired_from_mdb` 点名，
/// 人工出口是整库快删。空列表不出声。
fn retired_element_dbnums_notice(removed: &[u32]) -> Option<String> {
    (!removed.is_empty()).then(|| {
        format!(
            "[mdb-scope] 移出 MDB 的元素库 {removed:?}：离开本期执行范围，库里的数据 / 模型面状态与水位行不自动清\
             （GitHub #41）；下次启动在 /health.startup.reconcile.retired_from_mdb 点名，确认不再需要时先暂停队列，\
             再逐库 DELETE /api/v1/dbnums/{{dbnum}}/data"
        )
    })
}

/// SYST 批次落库后的声明刷新（ADR-085 D2）：先从主项目库目录重读 SYS 里 `mdb` 的声明
/// （[`crate::data_interface::mdb_closure::reload_declaration`]，读成功才替换），再按新声明重做
/// 成员绑定并登记。任何一步失败都原样返回，旧声明与旧登记表不动——调用方把原因写进那一批的回执。
///
/// 同步 I/O，async 侧放进 `spawn_blocking`。
pub fn reload_declaration_and_rebind(
    db_option: &DbOption,
    project: &str,
    mdb: &str,
) -> anyhow::Result<Arc<MdbMembership>> {
    let plan = crate::data_interface::mdb_closure::resolve_for(db_option).watch_dir_plan();
    let own_dirs = own_db_dirs(&plan, project)?;
    let reload_started = Instant::now();
    crate::data_interface::mdb_closure::reload_declaration(&own_dirs, project, mdb)?;
    // 重读与重绑分开报（GitHub #69）：重绑那段的 `[ams-phase]` 阶段行自带 `elapsed_ms=`（[`rebind_phase_line`]），
    // 重读 43 MB 的 SYS 文件这一段此前没有数。
    println!(
        "[SYS 解析] SYST 刷新重读声明 project={project} mdb={mdb} reload_ms={}",
        reload_started.elapsed().as_millis()
    );
    rebind(db_option, project, mdb, RebindReason::SystRefresh)
}

/// `mdb_scope` 阶段行本身（纯函数，单测直接比字符串）。`dbnums` / `unbound` 只数元素库（DESI / ISOD）
/// ——与本期执行范围同一口径；新增 / 移除只在 SYST 刷新时带，别的时刻声明没换，比了也是空。
fn rebind_phase_line(
    membership: &MdbMembership,
    previous: Option<&MdbMembership>,
    reason: RebindReason,
    elapsed_ms: u128,
) -> String {
    let dbnums: BTreeSet<u32> = membership
        .element_databases()
        .map(|database| database.dbnum)
        .collect();
    let unbound = membership
        .element_databases()
        .filter(|database| database.path.is_none())
        .count();
    let mut line = format!(
        "[ams-phase] phase=mdb_scope project={} mdb={} source=sys-file reason={} dbnums={} unbound={unbound}",
        membership.project(),
        membership.mdb(),
        reason.as_str(),
        dbnums.len()
    );
    if reason == RebindReason::SystRefresh {
        let before: BTreeSet<u32> = previous
            .map(|previous| {
                previous
                    .element_databases()
                    .map(|database| database.dbnum)
                    .collect()
            })
            .unwrap_or_default();
        let added: Vec<u32> = dbnums.difference(&before).copied().collect();
        // 与 GitHub #41 那一行点名同一份口径：阶段行的 `removed=` 与「移出 MDB 的元素库」不许各算各的。
        let removed = removed_element_dbnums(membership, previous);
        line.push_str(&format!(" added={added:?} removed={removed:?}"));
    }
    line.push_str(&format!(" elapsed_ms={elapsed_ms}"));
    line
}

/// 读写登记表的用例串成一条跑：登记表是进程级单例，并行的用例会互相覆盖对方装进去的那一份。
#[cfg(test)]
pub(crate) static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 从登记表里拿掉 `(project, mdb)`——只给用例还原现场用；生产路径只换不删。
#[cfg(test)]
pub(crate) fn uninstall(project: &str, mdb: &str) {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    if let Ok(mut cache) = registry().write() {
        cache.retain(|(known_project, known_mdb, _)| {
            !(known_project == project && *known_mdb == wanted)
        });
    }
}

/// 用例结束（含 panic）时把登记表里的 `(project, mdb)` 恢复成用例开始前的样子。
#[cfg(test)]
pub(crate) struct RegistryRestore {
    project: String,
    mdb: String,
    previous: Option<Arc<MdbMembership>>,
}

#[cfg(test)]
impl RegistryRestore {
    pub(crate) fn capture(project: &str, mdb: &str) -> Self {
        Self {
            project: project.to_string(),
            mdb: mdb.to_string(),
            previous: get(project, mdb),
        }
    }
}

#[cfg(test)]
impl Drop for RegistryRestore {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => {
                install((*previous).clone());
            }
            None => uninstall(&self.project, &self.mdb),
        }
    }
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
                user: "SITE-A".to_string(),
                comment: "SAVEWORK".to_string(),
            },
            file_size: 123_456,
        };

        let evidence = sys_version_evidence_summary(&e3d_io::DatabaseVersionEvidence {
            version: None,
            source: e3d_io::VersionEvidenceSource::Unknown,
            raw_value: None,
            raw_text: None,
            session_page: Some(42),
            database: std::path::PathBuf::from(r"D:\release\9001\project\JEU\JEU000\jeusys"),
        });
        let line = sys_basic_info_message(
            Path::new(r"D:\release\9001\project\JEU\JEU000\jeusys"),
            &header,
            "e3d210",
            &evidence,
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
            "version_evidence={field=none raw_value=None raw_text=None session_page=Some(42)}",
            "user=\"SITE-A\"",
            "comment=\"SAVEWORK\"",
        ] {
            assert!(line.contains(required), "基本信息缺 {required}: {line}");
        }
        // 探针从不给版本结论，这一行也不得再拿一条已被推翻的规则去比「版本不匹配」。
        let source = include_str!("mdb_membership.rs");
        assert!(
            !source.contains(concat!("e3d_io::", "detect_database_version(")),
            "SYS 解析不得再调 detect_*：e3d-io 2c6f11d 起它一律失败，只会在 stderr 留一行噪音"
        );
        assert!(
            !source.contains(concat!("[SYS 解析] ", "版本不匹配")),
            "「版本不匹配」比的是已被推翻的识别规则，不得再打"
        );
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
    fn sys_metadata_resolver_uses_file_header_dbnum_instead_of_reserved_constants() {
        let header = crate::data_interface::db_file_header::DbFileHeader {
            dbnum: 4242,
            db_type: "GLOB".to_string(),
            latest_ses_pgno: 12,
            latest_session: crate::data_interface::db_file_header::SessionStamp {
                sesno: 9,
                ..Default::default()
            },
            file_size: 123,
        };
        let global = PathBuf::from(r"D:\project\JEU000\jeuglb");
        let resolver = SysMetadataResolver::from_headers(
            Path::new(r"D:\project\JEU000\jeusys"),
            Path::new(r"D:\runtime\schema\e3d210\binaries"),
            [(global.clone(), header)],
        )
        .expect("header-discovered resolver");

        assert!(
            e3d_io::DbFileResolver::resolve(&resolver, 8188).is_none(),
            "文件没自报 8188 时不得凭 Global 角色猜一个保留号"
        );
        let pin = e3d_io::DbFileResolver::resolve(&resolver, 4242)
            .expect("文件头自报的任意 dbnum 都能定位");
        assert_eq!(pin.file, global);
        assert_eq!(pin.db_type.as_deref(), Some("GLOB"));
        assert_eq!(pin.sesno, Some(9));
        assert!(pin.template.ends_with("glbvir.dat"), "{pin:?}");

        let production = include_str!("mdb_membership.rs")
            .split_once("#[cfg(test)]")
            .expect("test module marker")
            .0;
        assert!(
            !production.contains("8188"),
            "生产解析器不得写死 Global DB 的保留号"
        );
    }

    #[test]
    fn sys_metadata_resolver_rejects_duplicate_header_dbnums() {
        let header = || crate::data_interface::db_file_header::DbFileHeader {
            dbnum: 4242,
            db_type: "GLOB".to_string(),
            latest_session: crate::data_interface::db_file_header::SessionStamp {
                sesno: 9,
                ..Default::default()
            },
            ..Default::default()
        };
        let error = SysMetadataResolver::from_headers(
            Path::new(r"D:\project\JEU000\jeusys"),
            Path::new(r"D:\runtime\schema\e3d210\binaries"),
            [
                (PathBuf::from(r"D:\project\JEU000\jeuglb"), header()),
                (PathBuf::from(r"D:\project\JEU000\jeuglb-copy"), header()),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("两个自报 dbnum=4242") && error.contains("拒绝"),
            "{error}"
        );
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
            access: BTreeMap::new(),
            problems: Vec::new(),
        }
    }

    /// `mdb_scope` 阶段行只数元素库：字典库既不进 `dbnums` 也不进 `unbound`（与本期执行范围同一口径）；
    /// 新增 / 移除只在 SYST 刷新时带（ADR-085 D5）。
    #[test]
    fn the_rebind_line_counts_element_databases_and_diffs_only_on_syst_refresh() {
        let before = MdbMembership::test_with_databases(
            "AvevaMarineSample",
            "/ALL",
            vec![
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(7999, DESI_STYP, "*AMS/OLD", true),
                db(6002, DICT_STYP, "*MASTER/SCBDICT", false),
            ],
        );
        let after = MdbMembership::test_with_databases(
            "AvevaMarineSample",
            "/ALL",
            vec![
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(9001, ISOD_STYP, "*AMS/ISO", false),
                db(6002, DICT_STYP, "*MASTER/SCBDICT", false),
            ],
        );

        let refreshed =
            rebind_phase_line(&after, Some(&before), RebindReason::SystRefresh, 4120);
        // 拆开写：下一条用例全仓数这一行的字面量，只许生产代码那一处。
        assert_eq!(
            refreshed,
            concat!(
                "[ams-phase] phase=",
                "mdb_scope project=AvevaMarineSample mdb=/ALL source=sys-file ",
                "reason=syst-refresh dbnums=2 unbound=1 added=[9001] removed=[7999] elapsed_ms=4120"
            )
        );

        let remounted = rebind_phase_line(&after, Some(&before), RebindReason::Remount, 7);
        assert!(
            remounted.contains("reason=remount dbnums=2 unbound=1 elapsed_ms=7"),
            "声明没换的时刻不带新增 / 移除: {remounted}"
        );
        let started = rebind_phase_line(&after, None, RebindReason::Startup, 1);
        assert!(
            started.contains("reason=startup") && !started.contains("added="),
            "{started}"
        );
    }

    /// GitHub #41：SYST 刷新把元素库移出声明时单独点名一行、**不自动清**。只数元素库（DESI / ISOD）——字典库
    /// 移出不算；首次绑定（没有旧登记）与声明没少时不出声。
    #[test]
    fn a_syst_refresh_that_drops_element_databases_names_them_and_cleans_nothing() {
        let before = MdbMembership::test_with_databases(
            "AvevaMarineSample",
            "/ALL",
            vec![
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(7999, DESI_STYP, "*AMS/OLD", true),
                db(9002, ISOD_STYP, "*AMS/ISO-OLD", true),
                db(6002, DICT_STYP, "*MASTER/SCBDICT", true),
            ],
        );
        let after = MdbMembership::test_with_databases(
            "AvevaMarineSample",
            "/ALL",
            vec![
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(9001, ISOD_STYP, "*AMS/ISO", false),
            ],
        );

        let removed = removed_element_dbnums(&after, Some(&before));
        assert_eq!(removed, vec![7999, 9002], "字典库 6002 移出不算元素库");
        let notice = retired_element_dbnums_notice(&removed).expect("移出了元素库要出声");
        assert!(
            notice.contains("[7999, 9002]")
                && notice.contains("不自动清")
                && notice.contains("retired_from_mdb")
                && notice.contains("DELETE /api/v1/dbnums/{dbnum}/data"),
            "{notice}"
        );
        let refreshed = rebind_phase_line(&after, Some(&before), RebindReason::SystRefresh, 1);
        assert!(
            refreshed.contains("removed=[7999, 9002]"),
            "阶段行与点名同一份口径: {refreshed}"
        );

        assert!(
            removed_element_dbnums(&after, None).is_empty(),
            "首次绑定没有旧登记，谈不上移出"
        );
        assert!(removed_element_dbnums(&before, Some(&before)).is_empty());
        assert!(retired_element_dbnums_notice(&[]).is_none(), "空列表不出声");
    }

    /// ADR-085 D5：`mdb_scope` 阶段行只记真实的成员绑定，全仓只许 [`rebind_phase_line`] 一处拼它。
    /// 取范围那条路（`UpdateScope::fetch`）再打一次，2026-09-29 那种稳态刷屏就回来了。
    #[test]
    fn the_mdb_scope_phase_line_is_built_only_by_rebind() {
        fn collect(dir: &Path, needle: &str, hits: &mut Vec<PathBuf>) {
            let entries = std::fs::read_dir(dir).expect("read src dir");
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, needle, hits);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    hits.extend(text.matches(needle).map(|_| path.clone()));
                }
            }
        }
        // 拆开写：这条用例自己的字面量不许算一处。
        let needle = concat!("phase=", "mdb_scope");
        let mut hits = Vec::new();
        collect(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            needle,
            &mut hits,
        );
        assert_eq!(hits.len(), 1, "只许一处: {hits:?}");
        assert!(
            hits[0].ends_with("mdb_membership.rs"),
            "唯一那一处必须在成员绑定入口里: {hits:?}"
        );
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

    /// ADR-073 D8：缺文件的 DESI 按号点名给常驻房间模型降级用，不再是一句整份 Err；
    /// 缺字典是另一项检查，权威的空声明也不算缺文件。
    #[test]
    fn missing_design_files_are_named_for_degradation_not_thrown() {
        let mut declaration = membership();
        assert!(
            declaration.missing_design_files().is_empty(),
            "missing dictionary is a separate check"
        );
        declaration
            .databases
            .push(db(9999, DESI_STYP, "*MASTER/MISSING", false));
        assert_eq!(
            declaration.missing_design_files(),
            vec![(9999, "*MASTER/MISSING".to_string())]
        );
        declaration.databases.clear();
        assert!(
            declaration.missing_design_files().is_empty(),
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
        let inventory = DirInventory::from_dirs("T", &[dir.path().to_path_buf()]);

        assert_eq!(
            pick_within_project(&inventory, 100),
            None,
            "8100 的文件不属于 100"
        );
        assert_eq!(
            pick_within_project(&inventory, 8100).map(|(path, _)| path),
            Some(dir.path().join("ams8100_0001"))
        );
    }

    /// An unsuffixed master and a non-`_0001` extract are both legal identities
    /// for the same logical database (ADR-028); the leaf is the working file
    /// and wins over the master. Hand copies never match at all.
    #[test]
    fn locate_accepts_masters_and_other_extracts_and_prefers_the_leaf() {
        let dir = dir_with(&["scb100", "scb100_0002", "scb100_0002 copy"]);
        let inventory = DirInventory::from_dirs("T", &[dir.path().to_path_buf()]);

        let (winner, shadowed) = pick_within_project(&inventory, 100).expect("解得出");
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
        let master_inventory = DirInventory::from_dirs("T", &[master_only.path().to_path_buf()]);
        assert_eq!(
            pick_within_project(&master_inventory, 100).map(|(path, _)| path),
            Some(master_only.path().join("scb100")),
            "没有抽取时主库本身就是答案，不该被报成缺件"
        );
    }

    /// 结构性约束：一个项目的库目录在整轮绑定里只枚举一次，声明再多也不重扫。
    /// 2026-10-15 现场：480 个声明 × 每次一整遍 UNC 枚举 + 逐文件 `is_file()`
    /// stat，把 `init_mdb` 拖成几十分钟静默直至假死。
    #[test]
    fn each_directory_is_enumerated_once_for_all_declarations() {
        let own = dir_with(&["ams7000_0001", "ams7355_0001"]);
        let other = dir_with(&["acp7000_0001"]);
        let projects = vec![
            ("Main".to_string(), vec![own.path().to_path_buf()]),
            ("Catalogue".to_string(), vec![other.path().to_path_buf()]),
        ];
        let declared: Vec<MdbDatabase> = (7000..7480)
            .map(|dbnum| MdbDatabase {
                dbnum,
                styp: DESI_STYP,
                proj: 0,
                declared_project_code: None,
                name: format!("*DB{dbnum}"),
                path: None,
                project: None,
                shadowed: Vec::new(),
            })
            .collect();

        let outcome = bind_declared(&declared, &projects, "Main").unwrap();

        assert_eq!(outcome.dirs_scanned, 2, "每个项目目录只枚举一次");
        assert_eq!(outcome.entries, 3, "枚举条目数是目录内容、不是声明数");
        assert_eq!(outcome.databases.len(), 480);
        let bound7000 = &outcome.databases[0];
        assert_eq!(
            bound7000.path.as_deref(),
            Some(own.path().join("ams7000_0001").as_path()),
            "无 PROJ 的声明绑主项目的文件"
        );
        assert_eq!(
            bound7000.shadowed,
            vec![other.path().join("acp7000_0001")],
            "别项目同号文件进 shadowed，不顶替"
        );
        assert!(
            outcome.databases[100].path.is_none(),
            "声明了没文件的库照旧 unbound"
        );
    }

    /// 目录枚举不动不连累其余目录，也不静默：进 `problems`，与「声明了没文件」区分
    /// ——两者在下游都长成「库里没值」，修法完全不同。
    #[test]
    fn an_unreadable_directory_surfaces_as_a_problem_and_the_rest_still_bind() {
        let own = dir_with(&["ams7000_0001"]);
        let missing = tempfile::tempdir()
            .expect("tempdir")
            .path()
            .join("no-such-dir");
        let projects = vec![
            ("Main".to_string(), vec![own.path().to_path_buf()]),
            ("Gone".to_string(), vec![missing.clone()]),
        ];
        let declared = vec![
            MdbDatabase {
                dbnum: 7000,
                styp: DESI_STYP,
                proj: 0,
                declared_project_code: None,
                name: "*MAIN/DESIGN".into(),
                path: None,
                project: None,
                shadowed: Vec::new(),
            },
            MdbDatabase {
                dbnum: 7001,
                styp: DESI_STYP,
                proj: 3,
                declared_project_code: Some("GON".into()),
                name: "*GONE/DESIGN".into(),
                path: None,
                project: None,
                shadowed: Vec::new(),
            },
        ];

        let outcome = bind_declared(&declared, &projects, "Main").unwrap();

        assert_eq!(outcome.dirs_scanned, 1, "只枚举通了主项目目录");
        assert_eq!(outcome.problems.len(), 1, "读不动的目录要留痕");
        assert!(
            outcome.problems[0].contains("no-such-dir"),
            "{:?}",
            outcome.problems
        );
        assert_eq!(
            outcome.databases[0].path.as_deref(),
            Some(own.path().join("ams7000_0001").as_path())
        );
        assert!(
            outcome.databases[1].path.is_none(),
            "目录缺席的项目的声明保持 unbound"
        );
    }

    /// 源码钉：定位层（`locate_declared` + `pick_within_project`）不得再出现任何
    /// 文件系统调用——目录枚举集中在 `DirInventory::from_dirs`/`scan_db_dir` 一处，
    /// 定位只查内存索引。
    #[test]
    fn declaration_locating_never_touches_the_disk() {
        let source = include_str!("mdb_membership.rs");
        let body = source
            .split_once("fn locate_declared(")
            .expect("locate_declared 还在")
            .1
            .split_once("type Cache")
            .expect("定位层收尾在 Cache 之前")
            .0;
        // 只禁调用形态——注释里提到 `read_dir` 字样不算触盘。
        for banned in ["std::fs::", "read_dir(", "is_file(", "is_dir(", "metadata("] {
            assert!(!body.contains(banned), "定位层不得触盘: {banned}");
        }
    }

    /// An in-memory SYS tree for the CURD expansion: noun, members and the two
    /// reference attributes the walk reads, keyed by refno.
    #[derive(Default)]
    struct FakeSys {
        nouns: BTreeMap<RefU64, &'static str>,
        members: BTreeMap<RefU64, Vec<RefU64>>,
        refs: BTreeMap<(RefU64, &'static str), RefU64>,
        criteria: BTreeSet<RefU64>,
    }

    impl FakeSys {
        fn element(&mut self, refno: RefU64, noun: &'static str) -> &mut Self {
            self.nouns.insert(refno, noun);
            self
        }

        fn dbset(&mut self, refno: RefU64, members: &[RefU64]) -> &mut Self {
            self.nouns.insert(refno, "DBSET");
            self.members.insert(refno, members.to_vec());
            self
        }

        fn dbl(&mut self, refno: RefU64, db: RefU64) -> &mut Self {
            self.nouns.insert(refno, "DBL");
            self.refs.insert((refno, "DBF"), db);
            self
        }

        fn dbstl(&mut self, refno: RefU64, dbset: RefU64) -> &mut Self {
            self.nouns.insert(refno, "DBSTL");
            self.refs.insert((refno, "DBSTF"), dbset);
            self
        }
    }

    impl CurdElements for FakeSys {
        fn element_type(&self, refno: RefU64) -> anyhow::Result<Option<String>> {
            Ok(self.nouns.get(&refno).map(|noun| noun.to_string()))
        }

        fn member_refnos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
            Ok(self.members.get(&refno).cloned().unwrap_or_default())
        }

        fn get_ref(&self, refno: RefU64, attribute: &str) -> anyhow::Result<Option<RefU64>> {
            Ok(self
                .refs
                .iter()
                .find(|((owner, name), _)| *owner == refno && *name == attribute)
                .map(|(_, target)| *target))
        }

        fn has_db_criteria(&self, refno: RefU64) -> anyhow::Result<bool> {
            Ok(self.criteria.contains(&refno))
        }
    }

    fn r(n: u32) -> RefU64 {
        RefU64::from_two_nums(24575, n)
    }

    fn refnos(expansion: &CurdExpansion) -> Vec<RefU64> {
        expansion.entries.iter().map(|entry| entry.refno).collect()
    }

    /// The common case must stay byte-for-byte what it was: a CURD of plain DB
    /// elements comes back unchanged, in order, with nothing to report.
    #[test]
    fn curd_of_plain_databases_passes_through_untouched() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB")
            .element(r(2), "DB")
            .element(r(3), "DB");
        let expansion = expand_curd_dbsets(&sys, &[r(3), r(1), r(2)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(3), r(1), r(2)]);
        assert!(expansion.entries.iter().all(|entry| entry.via.is_none()));
        assert_eq!(expansion.dbset_entries, 0);
        assert!(expansion.problems.is_empty(), "{:?}", expansion.problems);
    }

    /// A DBSET in CURD is replaced, at its position, by the databases its DBL
    /// members point at — E3D's `CURRFU` — and each carries the set it came from.
    #[test]
    fn dbset_entry_unfolds_into_its_dbl_targets_in_place() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB")
            .element(r(2), "DB")
            .element(r(3), "DB")
            .dbl(r(101), r(2))
            .dbl(r(102), r(3))
            .dbset(r(100), &[r(101), r(102)]);
        let expansion = expand_curd_dbsets(&sys, &[r(1), r(100)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(1), r(2), r(3)]);
        assert_eq!(expansion.entries[0].via, None);
        assert_eq!(
            expansion.entries[1].via.as_deref(),
            Some("CURD[1] DBSET 24575/100")
        );
        assert_eq!(expansion.dbset_entries, 1);
        assert!(expansion.problems.is_empty(), "{:?}", expansion.problems);
    }

    /// Nested sets are reached through DBSTL.DBSTF; a set referenced twice, or
    /// one that refers back to an ancestor, is entered once and the walk ends.
    #[test]
    fn nested_and_cyclic_dbsets_are_entered_once() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB")
            .element(r(2), "DB")
            .dbl(r(201), r(1))
            .dbstl(r(202), r(300))
            .dbstl(r(203), r(300))
            .dbset(r(200), &[r(201), r(202), r(203)])
            .dbl(r(301), r(2))
            .dbstl(r(302), r(200))
            .dbset(r(300), &[r(301), r(302)]);
        let expansion = expand_curd_dbsets(&sys, &[r(200)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(1), r(2)]);
        assert_eq!(
            expansion.entries[1].via.as_deref(),
            Some("CURD[0] DBSET 24575/200 > DBSET 24575/300")
        );
        assert!(expansion.problems.is_empty(), "{:?}", expansion.problems);
    }

    /// One database reached twice — directly and through a set, or through two
    /// sets — is kept once at its first position, and the repeat is reported.
    #[test]
    fn a_database_reached_twice_is_kept_once_at_its_first_position() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB")
            .element(r(2), "DB")
            .dbl(r(101), r(1))
            .dbl(r(102), r(2))
            .dbset(r(100), &[r(101), r(102)]);
        let expansion = expand_curd_dbsets(&sys, &[r(2), r(100), r(2)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(2), r(1)]);
        assert_eq!(expansion.problems.len(), 1, "{:?}", expansion.problems);
        assert!(expansion.problems[0].contains("CURD[2] 24575/2"));
    }

    /// What E3D would do with these is fail or guess; here each is a reported
    /// problem and the rest of the set still comes through.
    #[test]
    fn dbset_anomalies_are_reported_not_swallowed() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB")
            .dbl(r(101), r(1))
            .element(r(102), "DBL") // no DBF
            .element(r(103), "DBSTL") // no DBSTF
            .element(r(104), "TEAM") // not a set member type
            .dbset(r(100), &[r(101), r(102), r(103), r(104), r(105)]); // 105 missing
        sys.criteria.insert(r(100));
        let expansion = expand_curd_dbsets(&sys, &[r(100)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(1)]);
        let problems = expansion.problems.join("\n");
        for required in [
            "DBCRITERIA",
            "DBL 24575/102 没有 DBF",
            "DBSTL 24575/103 没有 DBSTF",
            "24575/104 类型 TEAM",
            "24575/105 不在最新会话索引中",
        ] {
            assert!(problems.contains(required), "缺少 {required}:\n{problems}");
        }
        assert_eq!(expansion.problems.len(), 5, "{problems}");
    }

    /// Entries that are neither DB nor DBSET — a missing element included —
    /// stay in the list so the per-database decode reports them with its stage.
    #[test]
    fn unknown_and_missing_curd_entries_are_left_for_the_decoder_to_report() {
        let mut sys = FakeSys::default();
        sys.element(r(1), "DB").element(r(9), "TEAM");
        let expansion = expand_curd_dbsets(&sys, &[r(1), r(9), r(42)]).unwrap();
        assert_eq!(refnos(&expansion), vec![r(1), r(9), r(42)]);
        assert!(expansion.problems.is_empty(), "{:?}", expansion.problems);
    }

    fn two_projects(
        main_dir: &Path,
        other_dir: &Path,
    ) -> (Vec<(String, DirInventory)>, Vec<String>) {
        (
            vec![
                (
                    "Main".to_string(),
                    DirInventory::from_dirs("Main", &[main_dir.to_path_buf()]),
                ),
                (
                    "Catalogue".to_string(),
                    DirInventory::from_dirs("Catalogue", &[other_dir.to_path_buf()]),
                ),
            ],
            vec!["Main".into(), "Catalogue".into()],
        )
    }
}

/// SYS 固定结构导航（WORL → MDBW → 成员）的门：真库两条装了 AMS 才跑，文本护栏一条哪儿都跑。
#[cfg(test)]
#[path = "mdb_membership_sys_nav_tests.rs"]
mod sys_nav_tests;
