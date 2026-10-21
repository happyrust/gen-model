//! dbnum 路由目录：RefNo → dbnum → 文件，**不含任何逐元素表**。
//!
//! 母计划 `docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md` §5.1；执行切片
//! `docs/plans/2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` W2。
//!
//! core.dll 的对应物是两张按 dbno 键的表：C++ 层 `DB_DB::DBLookup_`（`DB_DB::findDB(int)`），
//! DABACON 层 `sub_5AF58E0` 在有序表上二分。两张表都只回答「dbno → 打开态」，refno 到 dbno
//! 靠位解码（[`super::refno_route`]），MDB 只决定「开哪些库」（`DB_MDB::internalOpenDB` 读
//! `DBNO/ISTYP`）。这里同一形状：目录从 SYS/CURD 成员清单造出来，键是 dbnum，值是文件；
//! 构造**不读任何元素**——JEU 现场 417 个 DESI / 1,221,858 个元素那 68 秒全在旧骨架那条路上。
//!
//! 比 core 多出来的两件职责（core 在 `internalOpenDB` 一次做完）：
//! * **选文件**：dbnum 只在项目内唯一，同号跨项目按 `preferred_project` 选同类型副本，
//!   与 `e3d_model_service::select_dependency_file` 同一规则；
//! * **文件自证**：候选副本用前核一次 60 字节头（不变量 3）；MDB 自己那份的核头留给开库方
//!   （`DbSet::db_slot` 的 `DbNumberMismatch` / `DirectStore` W3），这里不重复开文件。
//!
//! 作为 [`CataDbLocator`] 兼容壳时 `dbnum_of_ref0` 恒 `None`：`DirectStore::dbnum_of` 已改为位解码
//! （切片 W3），只拿 dbnum 问 `file_of` / `db_type_of`，ref0 那一问没有调用方了。可以直接塞给
//! `DirectStore::new`。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aios_core::RefU64;
use e3d_io::db_element::{DbFilePin, DbFileResolver, template_file_for};
use e3d_io::refno::RefNo;

use super::cata_closure::CataDbLocator;

// The HTTP health reader observes the exact directory used by the watcher.
// Reading health must never initialize a provider or open source files.
static ACTIVE_DIRECTORY: RwLock<Option<Arc<MdbRouteDirectory>>> = RwLock::new(None);

pub fn register_active(directory: Arc<MdbRouteDirectory>) {
    *ACTIVE_DIRECTORY.write().unwrap_or_else(PoisonError::into_inner) = Some(directory);
}

pub fn active(project: &str, mdb: &str) -> Option<Arc<MdbRouteDirectory>> {
    ACTIVE_DIRECTORY.read().unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .filter(|directory| directory.project() == project
            && directory.mdb().trim_start_matches('/') == mdb.trim_start_matches('/'))
        .cloned()
}

/// 当前进程登记的那份目录，不问身份——给只拿到一段错误文本、没有 `(project, mdb)` 的读者
/// （[`explain_unregistered_dbs`]）。
fn active_any() -> Option<Arc<MdbRouteDirectory>> {
    ACTIVE_DIRECTORY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// 给 e3d-io 的「db N 没注册进这个 DbSet，resolver 也给不出它的文件」补上路由目录知道的原因
/// （ADR-078 D5）：那句话到现场只剩一个库号，而路由目录知道它是「MDB 声明了、项目 ACP 的目录
/// 没定位到」还是「压根不在 MDB 里」。只加文字、不改任何判定；文本里没有那句话就原样返回。
pub fn explain_unregistered_dbs(message: &str) -> String {
    match active_any() {
        Some(directory) => explain_unregistered_dbs_with(message, &directory),
        None => message.to_string(),
    }
}

/// e3d-io「db N 没注册进这个 DbSet」点名的库号：一段报错里可能有好几句，一句都没有就是空集。
pub fn unregistered_dbnums(message: &str) -> BTreeSet<u32> {
    const MARKER: &str = " 没注册进这个 DbSet";
    let mut dbnums = BTreeSet::new();
    let mut rest = message;
    while let Some(at) = rest.find(MARKER) {
        let before = &rest[..at];
        let digits: String = before
            .chars()
            .rev()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if let Ok(dbnum) = digits.parse::<u32>()
            && before[..before.len() - digits.len()].ends_with("db ")
        {
            dbnums.insert(dbnum);
        }
        rest = &rest[at + MARKER.len()..];
    }
    dbnums
}

/// [`explain_unregistered_dbs`] 的纯函数芯：目录由调用方给。
pub fn explain_unregistered_dbs_with(message: &str, directory: &MdbRouteDirectory) -> String {
    let dbnums = unregistered_dbnums(message);
    if dbnums.is_empty() {
        return message.to_string();
    }
    let reasons: Vec<String> = dbnums
        .into_iter()
        .filter_map(|dbnum| match directory.route_dbno(dbnum) {
            // GitHub #46：路由目录里有它，却是依赖目录扫描补上的（MDB 没声明）。生成的 resolver 只认 MDB
            // 声明的库，所以这几件画不出——与 E3D 用这个 MDB 打开时一样读不到它；它前移也就不会让任何已发布的根过期。
            Ok(route) if route.origin == RouteOrigin::Dependency => Some(format!(
                "dbnum {dbnum}：MDB {} 没声明它，只在依赖目录扫描里找到（项目 {}，{}）——生成只读 MDB 声明的库，\
                 E3D 用这个 MDB 同样读不到它",
                directory.mdb(),
                route.project,
                route.path.display()
            )),
            Ok(_) => None,
            Err(error) => Some(format!("dbnum {dbnum}：{error}")),
        })
        .collect();
    if reasons.is_empty() {
        return message.to_string();
    }
    format!("{message}；路由目录给出的原因：{}", reasons.join("；"))
}
use super::db_file_header::{DbBasicInfo, DbFileHeader};
use super::db_generation::{
    DbGeneration, FileIdentity, GenerationChange, GenerationLedger, Observation,
};
use super::mdb_membership::{MdbDatabase, MdbMembership};
use super::refno_route::{refno_dbnum_of, to_refno};

/// SYS `STYP` → 库类型名（模板文件名与 `DbFilePin.db_type` 用它）。
///
/// 与 `e3d_model_service::styp_name` 同一张表；那一份是私有的，W3/W5 换调用点时收敛成这一份。
pub fn styp_name(value: i64) -> Option<&'static str> {
    match value {
        1 => Some("DESI"),
        2 => Some("CATA"),
        4 => Some("PROP"),
        6 => Some("ISOD"),
        7 => Some("PADD"),
        8 => Some("DICT"),
        9 => Some("ENGI"),
        14 => Some("SCHE"),
        _ => None,
    }
}

/// 依赖项目目录里**会**被并进目录的库类型：被引用端（目录侧）。
///
/// DESI / ISOD 不并——别的项目的设计库不是本 MDB 的成员；DICT 不并——UDA 字典只认 MDB 声明
/// （`MdbMembership::dictionary_paths`）。比 `InMemoryCataLocator::merge_cata_files` 只并 CATA
/// 多收 PROP：属性引用也会指进属性库，今天那些引用读不通只是因为旧表没收它。
pub const DEPENDENCY_ROUTE_TYPES: [&str; 2] = ["CATA", "PROP"];

fn is_dependency_route_type(db_type: &str) -> bool {
    DEPENDENCY_ROUTE_TYPES
        .iter()
        .any(|wanted| wanted.eq_ignore_ascii_case(db_type))
}

fn project_db_key(project: &str, dbnum: u32) -> (String, u32) {
    (project.trim().to_ascii_lowercase(), dbnum)
}

/// 一条路由是从哪来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOrigin {
    /// 当前 MDB 的 CURD 声明。
    Mdb,
    /// `included_projects` 依赖项目目录里发现的、MDB 没声明的目录库。
    Dependency,
}

/// dbnum → 文件。没有 noun / owner / NAME / 属性，也没有 ref0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRoute {
    pub dbnum: u32,
    /// `DESI` / `CATA` / `DICT` …… 决定模板文件。
    pub db_type: String,
    /// 文件出自哪个项目。
    pub project: String,
    pub path: PathBuf,
    /// 同号的其它文件（本项目 extract family 的下层、别的项目的同号库）。只记不选。
    pub shadowed: Vec<PathBuf>,
    pub origin: RouteOrigin,
}

/// 同号跨项目的一个候选文件（`preferred_project` 选择用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDbFile {
    pub db_type: String,
    pub project: String,
    pub path: PathBuf,
}

/// 路由失败的每一种原因都说清是哪个库 / 哪个文件；不存在「解不出就凑一个」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// `word1` 为负或解出 dbnum 0：这串数字不指向任何库。
    InvalidRefNo { refno: String },
    /// 编号合法，但当前 MDB 与依赖目录都没有这个库。
    DbNotInMdb { dbnum: u32, mdb: String },
    /// MDB 声明了它，搜索根里却没有文件——部署问题，要看见。`project_code` 是 SYS `PROJ`
    /// 说的它属于哪个项目（`None` = 声明它的主项目自己）；一路带到失败消息里，现场人看到的
    /// 才是「项目 ACP 的目录没定位到」而不是一个库号（ADR-078 D5）。
    NoFileForDbnum {
        dbnum: u32,
        mdb: String,
        name: String,
        project_code: Option<String>,
    },
    /// 库类型太短，拼不出 `*vir.dat` 模板名。
    TemplateMissing {
        dbnum: u32,
        db_type: String,
        detail: String,
    },
    /// 候选文件打不开或前 60 字节不是描述符页。
    HeaderUnreadable { dbnum: u32, path: PathBuf },
    /// 候选文件自报的库号 / 类型与目录说的不一致。
    HeaderMismatch {
        dbnum: u32,
        path: PathBuf,
        header_dbnum: u32,
        header_type: String,
        expected_type: String,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRefNo { refno } => {
                write!(
                    f,
                    "{refno} 不是合法 RefNo（word1 为负或解出 dbnum 0），无库可路由"
                )
            }
            Self::DbNotInMdb { dbnum, mdb } => {
                write!(
                    f,
                    "dbnum {dbnum} 不在 MDB {mdb} 的声明里，也不在依赖项目的目录库里"
                )
            }
            Self::NoFileForDbnum {
                dbnum,
                mdb,
                name,
                project_code: Some(code),
            } => write!(
                f,
                "MDB {mdb} 声明了 dbnum {dbnum}（{name}，项目 {code}），搜索根里没有 {}000 库目录或其中没有这个库的文件；\
                 把装着它的目录（本地或 \\\\服务器\\共享）加进 project_search_paths，详见 /health.mdb_closure",
                code.to_ascii_lowercase()
            ),
            Self::NoFileForDbnum {
                dbnum,
                mdb,
                name,
                project_code: None,
            } => write!(
                f,
                "MDB {mdb} 声明了 dbnum {dbnum}（{name}，本项目），主项目的库目录里没有它的文件"
            ),
            Self::TemplateMissing {
                dbnum,
                db_type,
                detail,
            } => write!(
                f,
                "dbnum {dbnum} 的库类型 {db_type:?} 选不出模板文件: {detail}"
            ),
            Self::HeaderUnreadable { dbnum, path } => write!(
                f,
                "dbnum {dbnum} 的候选文件 {} 读不出描述符页",
                path.display()
            ),
            Self::HeaderMismatch {
                dbnum,
                path,
                header_dbnum,
                header_type,
                expected_type,
            } => write!(
                f,
                "dbnum {dbnum} 的候选文件 {} 自报 dbnum {header_dbnum} / 类型 {header_type:?}，目录要的是 {dbnum} / {expected_type:?}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RouteError {}

/// 一次依赖目录扫描的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyScan {
    /// 通过类型过滤、按 extract family 折叠后登记为候选的文件数。
    pub candidates: usize,
    /// 其中 MDB 没声明、因此新增为路由的库数。
    pub added_routes: usize,
    /// extract family 折叠时丢掉的同号兄弟数。
    pub duplicate_keys: usize,
}

/// 目录级统计，给日志与将来的 `/health.core`。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RouteDirectoryStats {
    pub routes: usize,
    pub mdb_routes: usize,
    pub dependency_routes: usize,
    pub element_routes: usize,
    pub declared_without_file: usize,
    pub project_candidates: usize,
    pub problems: usize,
}

/// `poll_generations` 报出的一条变化：哪个库、哪个文件、从哪一版到哪一版、哪一种变化
/// （`docs/plans/2026-09-12-lazy-startup-dev-plan.md` §3.4）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RouteRefresh {
    pub dbnum: u32,
    pub db_type: String,
    pub path: PathBuf,
    pub from: DbGeneration,
    pub to: DbGeneration,
    pub kind: GenerationChange,
}

/// 轮询状态：世代账本 + 此刻读不出头的库（只在进出这个集合时各说一次，别每 2 s 刷屏）。
#[derive(Debug, Clone, Default)]
struct PollState {
    ledger: GenerationLedger,
    unreadable: BTreeSet<u32>,
}

/// MDB 声明了、盘上却没有文件的一个成员：名字与它属于哪个项目（`None` = 主项目自己）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingMember {
    pub name: String,
    pub project_code: Option<String>,
}

/// 见模块头。
#[derive(Debug)]
pub struct MdbRouteDirectory {
    project: String,
    mdb: String,
    template_dir: PathBuf,
    routes: BTreeMap<u32, DbRoute>,
    /// CURD 原序的 dbnum：`routes` 是按号排的，树首页与增量排工要按 CURD 序。
    curd_order: Vec<u32>,
    /// 声明了但磁盘上没文件的库：`dbnum → (SYS 元素名, 项目码)`。报 `NoFileForDbnum`，不混进 `DbNotInMdb`。
    declared_without_file: BTreeMap<u32, MissingMember>,
    /// `(project, dbnum) → 文件`，同号跨项目候选。
    project_files: HashMap<(String, u32), ProjectDbFile>,
    /// 构造期发现、该让人看见的事（CURD 重复声明、认不出的 STYP）。
    problems: Vec<String>,
    /// 每个路由库文件的世代（母计划不变量 7）：[`Self::poll_generations`] 更新，`/health.core` 读。
    /// 目录本身在构造后只读，账本是它唯一会动的部分，所以单独上锁而不是把整个目录锁起来。
    generations: Mutex<PollState>,
}

impl Clone for MdbRouteDirectory {
    fn clone(&self) -> Self {
        Self {
            project: self.project.clone(),
            mdb: self.mdb.clone(),
            template_dir: self.template_dir.clone(),
            routes: self.routes.clone(),
            curd_order: self.curd_order.clone(),
            declared_without_file: self.declared_without_file.clone(),
            project_files: self.project_files.clone(),
            problems: self.problems.clone(),
            generations: Mutex::new(self.poll_state().clone()),
        }
    }
}

impl MdbRouteDirectory {
    /// 从已解析的 MDB 成员清单造目录。零元素 I/O；`template_dir` 只参与拼模板路径。
    pub fn from_membership(membership: &MdbMembership, template_dir: impl Into<PathBuf>) -> Self {
        Self::from_databases(
            membership.project(),
            membership.mdb(),
            membership.databases(),
            template_dir,
        )
    }

    /// 同上，输入是 CURD 序的库声明——单测与探针不必先造一个 `MdbMembership`。
    pub fn from_databases(
        project: &str,
        mdb: &str,
        databases: &[MdbDatabase],
        template_dir: impl Into<PathBuf>,
    ) -> Self {
        let mut directory = Self {
            project: project.to_string(),
            mdb: format!("/{}", mdb.trim_start_matches('/')),
            template_dir: template_dir.into(),
            routes: BTreeMap::new(),
            curd_order: Vec::new(),
            declared_without_file: BTreeMap::new(),
            project_files: HashMap::new(),
            problems: Vec::new(),
            generations: Mutex::new(PollState::default()),
        };
        for db in databases {
            let Some(db_type) = styp_name(db.styp) else {
                directory.problems.push(format!(
                    "MDB {} 声明的 dbnum {}（{}）STYP={} 认不出库类型，不进路由",
                    directory.mdb, db.dbnum, db.name, db.styp
                ));
                continue;
            };
            let Some(path) = db.path.clone() else {
                directory
                    .declared_without_file
                    .entry(db.dbnum)
                    .or_insert_with(|| MissingMember {
                        name: db.name.clone(),
                        project_code: db.declared_project_code.clone(),
                    });
                continue;
            };
            let source_project = db.project.clone().unwrap_or_else(|| project.to_string());
            directory.register_project_file(&source_project, db.dbnum, db_type, path.clone());
            for shadowed in &db.shadowed {
                // 影子文件属于哪个项目由监视目录归属表回答；表里没有的（单测、未登记目录）
                // 只留在 `shadowed` 里备查，不进候选——候选必须知道自己的项目才可能被选中。
                if let Some(shadow_project) = super::project_paths::project_of_path(shadowed) {
                    directory.register_project_file(
                        &shadow_project,
                        db.dbnum,
                        db_type,
                        shadowed.clone(),
                    );
                }
            }
            if let Some(existing) = directory.routes.get(&db.dbnum) {
                // CURD 序决定赢家（与 `MdbMembership` 对重复 UKEY 的口径相同）：先声明的留，
                // 后声明的记下来给人看。
                directory.problems.push(format!(
                    "MDB {} 重复声明 dbnum {}：保留 {}，忽略 {}（{}）",
                    directory.mdb,
                    db.dbnum,
                    existing.path.display(),
                    path.display(),
                    db.name
                ));
                continue;
            }
            directory.curd_order.push(db.dbnum);
            directory.routes.insert(
                db.dbnum,
                DbRoute {
                    dbnum: db.dbnum,
                    db_type: db_type.to_string(),
                    project: source_project,
                    path,
                    shadowed: db.shadowed.clone(),
                    origin: RouteOrigin::Mdb,
                },
            );
        }
        directory
    }

    /// 登记一个同号跨项目候选。同一 `(project, dbnum)` 先到先得。
    pub fn register_project_file(
        &mut self,
        project: &str,
        dbnum: u32,
        db_type: &str,
        path: PathBuf,
    ) {
        self.project_files
            .entry(project_db_key(project, dbnum))
            .or_insert_with(|| ProjectDbFile {
                db_type: db_type.to_string(),
                project: project.to_string(),
                path,
            });
    }

    /// 把一个依赖项目目录下的目录库并进来（[`DEPENDENCY_ROUTE_TYPES`]）：只读每个文件的 60 字节头，
    /// 按 extract family 折叠，登记为 `(project, dbnum)` 候选；MDB 没声明的 dbnum 新增为
    /// [`RouteOrigin::Dependency`] 路由。声明了但没文件的（`NoFileForDbnum`）**不**被这里补上——
    /// 那是部署问题，补上就看不见了。
    ///
    /// 目录遍历口径照 `InMemoryCataLocator::build_from_dir_matching`（深度 8、跳过 `back`/`backup`），
    /// 只是不再逐库扫 ref0。
    pub fn add_dependency_dir(&mut self, project: &str, root_dir: &Path) -> DependencyScan {
        let mut discovered: Vec<(DbBasicInfo, PathBuf)> = Vec::new();
        for entry in walkdir::WalkDir::new(root_dir)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.components().any(|component| {
                matches!(
                    component
                        .as_os_str()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "back" | "backup"
                )
            }) {
                continue;
            }
            let Some(header) = DbBasicInfo::read(path) else {
                continue;
            };
            if header.db_no == 0 || !is_dependency_route_type(&header.db_type) {
                continue;
            }
            discovered.push((header, path.to_path_buf()));
        }
        let collapsed = super::extract_family::collapse_extract_families(
            discovered
                .iter()
                .map(|(header, path)| (project.to_string(), header.db_no, path.clone())),
        );
        let by_path: HashMap<PathBuf, DbBasicInfo> =
            discovered.into_iter().map(|(h, p)| (p, h)).collect();
        let mut scan = DependencyScan {
            duplicate_keys: collapsed.duplicate_keys.len(),
            ..DependencyScan::default()
        };
        for selected in collapsed.selected {
            let Some(header) = by_path.get(&selected.leaf_path) else {
                continue;
            };
            self.register_project_file(
                project,
                header.db_no,
                &header.db_type,
                selected.leaf_path.clone(),
            );
            scan.candidates += 1;
            if self.routes.contains_key(&header.db_no)
                || self.declared_without_file.contains_key(&header.db_no)
            {
                continue;
            }
            self.routes.insert(
                header.db_no,
                DbRoute {
                    dbnum: header.db_no,
                    db_type: header.db_type.clone(),
                    project: project.to_string(),
                    path: selected.leaf_path,
                    shadowed: Vec::new(),
                    origin: RouteOrigin::Dependency,
                },
            );
            scan.added_routes += 1;
        }
        scan
    }

    // ---- 查询 ----------------------------------------------------------------

    /// refno → 路由：先位解码，再查表。两步各有各的错。
    pub fn route_refno(&self, refno: RefNo) -> Result<&DbRoute, RouteError> {
        let dbnum = refno_dbnum_of(refno).ok_or_else(|| RouteError::InvalidRefNo {
            refno: refno.to_string(),
        })?;
        self.route_dbno(dbnum)
    }

    /// 同上，输入是 `RefU64`。
    pub fn route_refu64(&self, refno: RefU64) -> Result<&DbRoute, RouteError> {
        self.route_refno(to_refno(refno))
    }

    /// dbnum → 路由。「声明了没文件」与「根本没声明」分开报。
    pub fn route_dbno(&self, dbnum: u32) -> Result<&DbRoute, RouteError> {
        if let Some(route) = self.routes.get(&dbnum) {
            return Ok(route);
        }
        if let Some(missing) = self.declared_without_file.get(&dbnum) {
            return Err(RouteError::NoFileForDbnum {
                dbnum,
                mdb: self.mdb.clone(),
                name: missing.name.clone(),
                project_code: missing.project_code.clone(),
            });
        }
        Err(RouteError::DbNotInMdb {
            dbnum,
            mdb: self.mdb.clone(),
        })
    }

    /// 开库三元组。`sesno: None` = 开库那一刻解一次文件最新会话然后冻住（ADR-054；
    /// `DbFilePin` 文档），**不是**「每次读最新」。
    pub fn pin(&self, dbnum: u32) -> Result<DbFilePin, RouteError> {
        self.pin_for(dbnum, None)
    }

    /// 同号跨项目：`preferred_project` 有**同类型**副本、且该副本文件头自证与目录一致时用它
    /// （`select_dependency_file` 的规则：ACP 7015 要 ACP 的 CATA 7000，虽然 `/ALL` 声明的是 AMS 的 7000）；
    /// 副本头核不过就回落到 MDB 那份并记 warn——与 `current_mdb_source_inventory` 跳过头不符影子文件同一口径。
    pub fn pin_for(
        &self,
        dbnum: u32,
        preferred_project: Option<&str>,
    ) -> Result<DbFilePin, RouteError> {
        let route = self.route_dbno(dbnum)?;
        let mut file = route.path.clone();
        if let Some(candidate) = preferred_project
            .and_then(|project| self.project_files.get(&project_db_key(project, dbnum)))
            .filter(|candidate| candidate.db_type.eq_ignore_ascii_case(&route.db_type))
            .filter(|candidate| candidate.path != route.path)
        {
            match verify_header(dbnum, &candidate.path, &route.db_type) {
                Ok(()) => file = candidate.path.clone(),
                Err(error) => log::warn!(
                    "[route] dbnum {dbnum} 在项目 {} 的同号副本核头失败，回落到 MDB 声明的文件 {}: {error}",
                    candidate.project,
                    route.path.display()
                ),
            }
        }
        let template = template_file_for(&self.template_dir, &route.db_type).map_err(|error| {
            RouteError::TemplateMissing {
                dbnum,
                db_type: route.db_type.clone(),
                detail: error.to_string(),
            }
        })?;
        Ok(DbFilePin {
            file,
            template,
            db_type: Some(route.db_type.clone()),
            sesno: None,
        })
    }

    /// 全部路由的文件头自证（不变量 3），给启动复核 / 手动真库门用；正常读路径不调它——
    /// 开库方核头一次就够。
    pub fn verify_all_headers(&self) -> Vec<RouteError> {
        self.routes
            .values()
            .filter_map(|route| verify_header(route.dbnum, &route.path, &route.db_type).err())
            .collect()
    }

    fn poll_state(&self) -> std::sync::MutexGuard<'_, PollState> {
        self.generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 看一眼每个路由库的文件现在是哪一版，回答**哪些库变了**（母计划不变量 7；
    /// `docs/plans/2026-09-12-lazy-startup-dev-plan.md` §3.4）。
    ///
    /// 每条路由先 `stat`（[`FileIdentity::of`]）；身份没变就不开文件——E3D 的保存是追加写，会话号
    /// 只可能随文件内容一起变。身份变了才开一次头（描述符页 + 会话页，[`DbFileHeader::read`]）读
    /// `latest_sesno`，交给账本 [`GenerationLedger::observe`] 分成前进 / 回滚 / 换文件。不读任何元素；
    /// 成本与库数成正比、与元素数无关。第一次见到一个库只记基线，不算变化——所以启动时先叫一次
    /// 把基线记下来，之后每一次返回的才是「自上次以来变了的库」。
    ///
    /// 读不出头 / stat 不到的库这一轮跳过、不算变化，进出「读不出」集合时各说一次。
    /// 目录库（CATA / DICT / PROP）的变化同样报出来——它们的依赖失效是阶段 5 余下的活，
    /// 调用方自己决定对哪些类型动手。
    ///
    /// 一个已知的边：SAVEWORK 写到一半被看见时，长度已变、描述符页还指着旧会话，会先报一次
    /// `FileReplaced`，下一轮再报 `SessionAdvanced`。两次都只落在这一个 dbnum 上，多做一次
    /// 该库的定向刷新，不会错到别的库。
    pub fn poll_generations(&self) -> Vec<RouteRefresh> {
        let mut state = self.poll_state();
        let mut changes = Vec::new();
        for route in self.routes.values() {
            let identity = match FileIdentity::of(&route.path) {
                Ok(identity) => identity,
                Err(error) => {
                    if state.unreadable.insert(route.dbnum) {
                        crate::process_diagnostics::event(0, "increment.file.failed", &format!("dbnum={} path={} error={error}", route.dbnum, route.path.display()));
                        log::warn!(
                            "[route-poll] dbnum {} 的文件 {} stat 不到，本轮跳过：{error}",
                            route.dbnum,
                            route.path.display()
                        );
                    }
                    continue;
                }
            };
            let previous = state.ledger.get(route.dbnum);
            let (latest_sesno, base_sesno, crossed_backtrack) = match previous {
                Some(old) if old.identity == identity => (old.latest_sesno, old.base_sesno, false),
                _ => {
                    let read_started = std::time::Instant::now();
                    let observed = (|| -> anyhow::Result<_> {
                        let mut view = e3d_io::session::DbView::open(&route.path)?;
                        let chain = view.walk_sessions()?;
                        let latest = chain.first().ok_or_else(|| anyhow::anyhow!("empty session chain"))?;
                        let crossed = previous.is_some_and(|old| chain.windows(2).any(|pair| {
                            pair[0].sesno > old.latest_sesno && pair[0].tail.as_ref().is_some_and(|tail|
                                tail.base_sesno != 0 && tail.base_sesno < pair[1].sesno)
                        }));
                        Ok((latest.sesno, latest.tail.as_ref().map(|tail| tail.base_sesno), crossed))
                    })();
                    if read_started.elapsed() >= std::time::Duration::from_millis(250) {
                        println!("[route-poll] slow-session dbnum={} elapsed_ms={} initial={} ok={} path={}",
                            route.dbnum, read_started.elapsed().as_millis(), previous.is_none(),
                            observed.is_ok(), route.path.display());
                    }
                    match observed {
                        Ok(value) => value,
                        Err(error) => {
                            if state.unreadable.insert(route.dbnum) {
                                crate::process_diagnostics::event(0, "increment.file.failed", &format!("dbnum={} session error={error:#}", route.dbnum));
                            }
                            log::warn!("[route-poll] dbnum {} session read failed: {error:#}", route.dbnum);
                            continue;
                        }
                    }
                }
            };
            if state.unreadable.remove(&route.dbnum) {
                crate::process_diagnostics::event(0, "increment.file.recovered", &format!("dbnum={} path={}", route.dbnum, route.path.display()));
                log::info!(
                    "[route-poll] dbnum {} 的文件 {} 又能读了",
                    route.dbnum,
                    route.path.display()
                );
            }
            if let Observation::Changed { from, to, kind } =
                state.ledger.observe_session(route.dbnum, super::db_generation::Observed::new(identity, latest_sesno, base_sesno))
            {
                changes.push(RouteRefresh {
                    dbnum: route.dbnum,
                    db_type: route.db_type.clone(),
                    path: route.path.clone(),
                    from,
                    to,
                    kind: if crossed_backtrack { super::db_generation::GenerationChange::RolledBack } else { kind },
                });
            }
        }
        changes
    }

    /// 某库当前世代；没轮询过 → `None`。
    pub fn generation_of(&self, dbnum: u32) -> Option<DbGeneration> {
        self.poll_state().ledger.get(dbnum)
    }

    /// 全部世代之和：任何一个库变了它就变（`/health.core.snapshot_epoch` 的新来源，阶段 4 切片 §8-3）。
    pub fn generations_epoch(&self) -> u64 {
        self.poll_state().ledger.epoch()
    }

    /// 账本快照，dbnum 升序。
    pub fn generations(&self) -> Vec<(u32, DbGeneration)> {
        self.poll_state()
            .ledger
            .iter()
            .map(|(dbnum, entry)| (dbnum, *entry))
            .collect()
    }

    /// Process-local observation only; no filesystem access on a health request.
    pub fn health(&self) -> serde_json::Value {
        let state = self.poll_state();
        let failed: BTreeSet<_> = state.unreadable.iter().copied()
            .chain(self.declared_without_file.keys().copied()).collect();
        let stale: Vec<_> = state.unreadable.iter().copied()
            .filter(|dbnum| state.ledger.get(*dbnum).is_some()).collect();
        serde_json::json!({
            "status": if failed.is_empty() { "ready" } else { "degraded" },
            "route_count": self.len(),
            "snapshot_epoch": state.ledger.epoch(),
            "session_vector": state.ledger.iter().map(|(dbnum, entry)|
                serde_json::json!({"dbnum": dbnum, "sesno": entry.latest_sesno,
                    "generation": entry.generation})).collect::<Vec<_>>(),
            "stale_dbnums": stale,
            "failed_dbnums": failed,
            "problems": self.problems(),
            "last_targeted_refresh": super::db_generation::last_targeted_refresh(),
        })
    }

    /// DESI / ISOD 路由，CURD 原序。
    pub fn element_databases(&self) -> impl Iterator<Item = &DbRoute> {
        self.curd_order
            .iter()
            .filter_map(|dbnum| self.routes.get(dbnum))
            .filter(|route| matches!(route.db_type.as_str(), "DESI" | "ISOD"))
    }

    /// 全部路由，dbnum 升序。
    pub fn routes(&self) -> impl Iterator<Item = &DbRoute> {
        self.routes.values()
    }

    pub fn contains(&self, dbnum: u32) -> bool {
        self.routes.contains_key(&dbnum)
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    pub fn template_dir(&self) -> &Path {
        &self.template_dir
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// MDB 声明了、搜索根里却没有文件的库：`(dbnum, 成员)`，dbnum 升序。
    /// 启动横幅按项目码归并点名用；查询时它们报 `NoFileForDbnum`，不混进 `DbNotInMdb`。
    pub fn declared_without_file(&self) -> impl Iterator<Item = (u32, &MissingMember)> {
        self.declared_without_file
            .iter()
            .map(|(dbnum, missing)| (*dbnum, missing))
    }

    /// 声明落空的库按项目码归并：`项目码（None = 本项目）→ dbnum 升序`。
    pub fn declared_without_file_by_project(&self) -> BTreeMap<Option<String>, Vec<u32>> {
        let mut grouped: BTreeMap<Option<String>, Vec<u32>> = BTreeMap::new();
        for (dbnum, missing) in &self.declared_without_file {
            grouped
                .entry(missing.project_code.clone())
                .or_default()
                .push(*dbnum);
        }
        grouped
    }

    pub fn stats(&self) -> RouteDirectoryStats {
        RouteDirectoryStats {
            routes: self.routes.len(),
            mdb_routes: self
                .routes
                .values()
                .filter(|route| route.origin == RouteOrigin::Mdb)
                .count(),
            dependency_routes: self
                .routes
                .values()
                .filter(|route| route.origin == RouteOrigin::Dependency)
                .count(),
            element_routes: self.element_databases().count(),
            declared_without_file: self.declared_without_file.len(),
            project_candidates: self.project_files.len(),
            problems: self.problems.len(),
        }
    }
}

/// 文件自证：前 60 字节的描述符页自报的 dbnum / 类型必须与目录一致。
pub fn verify_header(dbnum: u32, path: &Path, expected_type: &str) -> Result<(), RouteError> {
    let Some(header) = DbBasicInfo::read(path) else {
        return Err(RouteError::HeaderUnreadable {
            dbnum,
            path: path.to_path_buf(),
        });
    };
    if header.db_no != dbnum || !header.db_type.eq_ignore_ascii_case(expected_type) {
        return Err(RouteError::HeaderMismatch {
            dbnum,
            path: path.to_path_buf(),
            header_dbnum: header.db_no,
            header_type: header.db_type,
            expected_type: expected_type.to_string(),
        });
    }
    Ok(())
}

/// `DbSet` 的按需开库口：池里没有这个 dbnum 时问一次。回 `None` 就地报 `DbNotRegistered`，
/// 不做第二次猜测（e3d-io 的约定）。
impl DbFileResolver for MdbRouteDirectory {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        match self.pin(dbno) {
            Ok(pin) => Some(pin),
            Err(error) => {
                // 指向本工程之外的引用是常态（扫一个元素身上的引用属性时），不是 warn。
                log::debug!("[route] dbnum {dbno} 无路由: {error}");
                None
            }
        }
    }
}

/// 迁移期兼容壳：`DirectStore` 还持 `Arc<dyn CataDbLocator>` 时能直接换上；`dbnum` 键的两问照答，
/// `ref0` 那一问**永远不答**——见模块头最后一段。
impl CataDbLocator for MdbRouteDirectory {
    fn contains_dbnum(&self, dbnum: u32) -> bool {
        self.routes.contains_key(&dbnum) || self.declared_without_file.contains_key(&dbnum)
    }
    fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> {
        None
    }

    fn db_type_of(&self, dbnum: u32) -> Option<String> {
        self.routes.get(&dbnum).map(|route| route.db_type.clone())
    }

    fn file_of(&self, dbnum: u32) -> Option<(String, PathBuf)> {
        self.routes
            .get(&dbnum)
            .map(|route| (route.project.clone(), route.path.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE_DIR: &str = "T:/e3d/templates";

    fn declared(dbnum: u32, styp: i64, name: &str, path: Option<&str>) -> MdbDatabase {
        MdbDatabase {
            dbnum,
            styp,
            proj: 0,
            declared_project_code: None,
            name: name.to_string(),
            path: path.map(PathBuf::from),
            project: path.map(|_| "ams".to_string()),
            shadowed: Vec::new(),
        }
    }

    /// `RefNo::dbno` 的逆：低 13 位 + 高位段左移 13，中间放 extent。
    fn word0_for(dbno: u32, extent: u32) -> u32 {
        (dbno & 0x1FFF) | ((dbno & 0x3E000) << 13) | (extent << 13)
    }

    fn directory() -> MdbRouteDirectory {
        MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                declared(8_000, 1, "/AMS/DESI", Some("T:/ams/ams8000_0001")),
                declared(5_052, 2, "/AMS/CATA", Some("T:/ams/ams5052_0001")),
                declared(8_191, 3, "/AMS/SYST", Some("T:/ams/amssys")),
                declared(7_000, 2, "/AMS/CATA2", None),
                declared(8_188, 6, "/AMS/ISOD", Some("T:/ams/ams8188_0001")),
                declared(9_100, 8, "/AMS/DICT", Some("T:/ams/ams9100_0001")),
            ],
            TEMPLATE_DIR,
        )
    }

    #[test]
    fn health_reports_missing_routes_without_opening_or_polling_files() {
        let directory = directory();
        let before = directory.generations();
        let health = directory.health();
        assert_eq!(health["route_count"], 4); // SYS is not an element/catalogue route.
        assert_eq!(health["failed_dbnums"], serde_json::json!([7000]));
        assert_eq!(health["stale_dbnums"], serde_json::json!([]));
        assert_eq!(health["status"], "degraded");
        assert_eq!(directory.generations(), before);
        assert_eq!(health["session_vector"], serde_json::json!([]));
        assert!(health.get("skeleton_elements").is_none());
    }

    /// 路由键是**解码出的 dbnum**，不是 word0：同一库的三个 extent 都落到同一条路由。
    #[test]
    fn routes_by_decoded_dbnum_not_by_word0() {
        let directory = directory();
        for extent in [1u32, 2, 3] {
            let refno = RefNo::new(word0_for(8_000, extent), 115);
            let route = directory.route_refno(refno).expect("8000 is declared");
            assert_eq!(route.dbnum, 8_000);
            assert_eq!(route.db_type, "DESI");
            assert_eq!(route.path, PathBuf::from("T:/ams/ams8000_0001"));
            assert_eq!(route.origin, RouteOrigin::Mdb);
        }
        // JEU 现场样本按同一条路走。
        let jeu = MdbRouteDirectory::from_databases(
            "jeu",
            "ALL",
            &[declared(8_188, 1, "/JEU/GLOB", Some("T:/jeu/jeuglb"))],
            TEMPLATE_DIR,
        );
        assert_eq!(
            jeu.route_refu64(RefU64::from_two_nums(24_572, 115))
                .expect("8188")
                .dbnum,
            8_188
        );
    }

    /// 三种「查不到」三种错：非法引用、没声明、声明了没文件。
    #[test]
    fn each_kind_of_miss_has_its_own_error() {
        let directory = directory();
        assert_eq!(
            directory.route_refno(RefNo::new(0, 0)),
            Err(RouteError::InvalidRefNo {
                refno: "0/0".to_string()
            })
        );
        assert_eq!(
            directory.route_dbno(9_999),
            Err(RouteError::DbNotInMdb {
                dbnum: 9_999,
                mdb: "/ALL".to_string()
            })
        );
        assert_eq!(
            directory.route_dbno(7_000),
            Err(RouteError::NoFileForDbnum {
                dbnum: 7_000,
                mdb: "/ALL".to_string(),
                name: "/AMS/CATA2".to_string(),
                project_code: None,
            })
        );
        // 错误文案里必须有 dbnum，排障时靠它对表。
        assert!(
            directory
                .route_dbno(7_000)
                .unwrap_err()
                .to_string()
                .contains("7000")
        );
    }

    /// 认不出的 STYP 不进路由，但要在 `problems` 里被看见。
    #[test]
    fn unknown_styp_is_reported_not_routed() {
        let directory = directory();
        assert!(!directory.contains(8_191), "STYP=3 这里没有名字");
        assert_eq!(directory.problems().len(), 1);
        assert!(directory.problems()[0].contains("8191"));
    }

    /// 开库三元组：模板按类型拼、`db_type` 带上给模板校验、会话不钉（开库时冻）。
    #[test]
    fn pin_carries_template_type_and_frozen_session() {
        let directory = directory();
        let desi = directory.pin(8_000).expect("pin DESI");
        assert_eq!(desi.file, PathBuf::from("T:/ams/ams8000_0001"));
        assert_eq!(desi.template, Path::new(TEMPLATE_DIR).join("desvir.dat"));
        assert_eq!(desi.db_type.as_deref(), Some("DESI"));
        assert_eq!(desi.sesno, None);

        let cata = directory.pin(5_052).expect("pin CATA");
        assert_eq!(cata.template, Path::new(TEMPLATE_DIR).join("catvir.dat"));
        let dict = directory.pin(9_100).expect("pin DICT");
        assert_eq!(dict.template, Path::new(TEMPLATE_DIR).join("dicvir.dat"));
        assert_eq!(
            directory.pin(8_188).unwrap().db_type.as_deref(),
            Some("ISOD")
        );
    }

    /// 元素库按 CURD 原序给，不按 dbnum 排；目录库不在里面。
    #[test]
    fn element_databases_keep_curd_order() {
        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                declared(8_188, 6, "/ISOD", Some("T:/a/ams8188_0001")),
                declared(5_052, 2, "/CATA", Some("T:/a/ams5052_0001")),
                declared(8_000, 1, "/DESI", Some("T:/a/ams8000_0001")),
                declared(7_015, 1, "/DESI2", Some("T:/a/ams7015_0001")),
            ],
            TEMPLATE_DIR,
        );
        let order: Vec<u32> = directory.element_databases().map(|r| r.dbnum).collect();
        assert_eq!(order, vec![8_188, 8_000, 7_015]);
        assert_eq!(directory.stats().element_routes, 3);
        assert_eq!(directory.stats().routes, 4);
    }

    /// CURD 里同一个 dbnum 出现两次：先声明的赢，后一次进 `problems`。
    #[test]
    fn duplicate_declaration_keeps_the_first_and_reports_the_second() {
        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                declared(8_000, 1, "/DESI", Some("T:/a/ams8000_0001")),
                declared(8_000, 1, "/DESI-AGAIN", Some("T:/b/ams8000_0001")),
            ],
            TEMPLATE_DIR,
        );
        assert_eq!(directory.len(), 1);
        assert_eq!(
            directory.route_dbno(8_000).unwrap().path,
            PathBuf::from("T:/a/ams8000_0001")
        );
        assert_eq!(directory.problems().len(), 1);
        assert!(directory.problems()[0].contains("重复声明 dbnum 8000"));
    }

    /// 兼容壳：dbnum 键的两问照答，ref0 那一问永远不答。
    #[test]
    fn locator_shell_never_answers_ref0() {
        let directory = directory();
        assert_eq!(directory.dbnum_of_ref0(word0_for(8_000, 1)), None);
        assert_eq!(directory.resolve_ref0(word0_for(8_000, 1)).unwrap(), None);
        assert_eq!(directory.db_type_of(5_052).as_deref(), Some("CATA"));
        assert_eq!(
            directory.file_of(8_000),
            Some(("ams".to_string(), PathBuf::from("T:/ams/ams8000_0001")))
        );
        assert_eq!(directory.file_of(7_000), None);
    }

    /// `DbFileResolver`：未命中回 `None`，不凑；命中就是 `pin()`。
    #[test]
    fn resolver_miss_is_none_not_a_guess() {
        let directory = directory();
        assert_eq!(directory.resolve(9_999), None);
        assert_eq!(directory.resolve(7_000), None, "声明了没文件也不能凑");
        assert_eq!(
            directory.resolve(8_000),
            Some(directory.pin(8_000).unwrap())
        );
    }

    // ---- 文件头自证：用 60 字节合成描述符页 ----

    /// 照 `DbDescriptor::parse` 的布局写一页描述符（大端）：+0x08 dbnum、+0x20 类型哈希、
    /// +0x2C ext_no、+0x34 页大小（字）、+0x38 首 extent 的 word0。
    fn write_fake_db(dir: &Path, name: &str, dbnum: u32, db_type: &str) -> PathBuf {
        let mut page = [0u8; 64];
        let put = |page: &mut [u8; 64], off: usize, value: u32| {
            page[off..off + 4].copy_from_slice(&value.to_be_bytes());
        };
        put(&mut page, 0x04, 2);
        put(&mut page, 0x08, dbnum);
        put(&mut page, 0x20, e3d_attlib::db1_hash(db_type));
        put(&mut page, 0x24, u32::MAX);
        put(&mut page, 0x28, 4);
        put(&mut page, 0x2C, 1);
        put(&mut page, 0x30, 4);
        put(&mut page, 0x34, 512);
        put(&mut page, 0x38, word0_for(dbnum, 1));
        let path = dir.join(name);
        std::fs::write(&path, page).expect("write fake descriptor page");
        path
    }

    /// 同号跨项目：`preferred_project` 的同类型副本头核得过就用它；类型不同或头不符都回落到 MDB 那份。
    #[test]
    fn preferred_project_copy_wins_only_with_same_type_and_matching_header() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ams_cata = write_fake_db(tmp.path(), "ams7000_0001", 7_000, "CATA");
        let acp_cata = write_fake_db(tmp.path(), "acp7000_0001", 7_000, "CATA");
        let acp_wrong_type = write_fake_db(tmp.path(), "acp7000_desi", 7_000, "DESI");
        let acp_wrong_dbnum = write_fake_db(tmp.path(), "acp7001_0001", 7_001, "CATA");

        let mut directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[MdbDatabase {
                dbnum: 7_000,
                styp: 2,
                proj: 0,
                declared_project_code: None,
                name: "/AMS/CATA".into(),
                path: Some(ams_cata.clone()),
                project: Some("ams".into()),
                shadowed: Vec::new(),
            }],
            TEMPLATE_DIR,
        );

        // 没有候选：MDB 那份。
        assert_eq!(
            directory.pin_for(7_000, Some("acp")).unwrap().file,
            ams_cata
        );

        // 同类型、头一致：候选赢。
        directory.register_project_file("ACP", 7_000, "CATA", acp_cata.clone());
        assert_eq!(
            directory.pin_for(7_000, Some("acp")).unwrap().file,
            acp_cata
        );
        assert_eq!(
            directory.pin_for(7_000, Some("Acp ")).unwrap().file,
            acp_cata,
            "项目名大小写与空白不敏感"
        );
        assert_eq!(
            directory.pin_for(7_000, None).unwrap().file,
            ams_cata,
            "不指定项目就是 MDB 那份"
        );

        // 类型不同：连头都不看，直接回落。
        let mut wrong_type = directory.clone();
        wrong_type.project_files.clear();
        wrong_type.register_project_file("acp", 7_000, "DESI", acp_wrong_type);
        assert_eq!(
            wrong_type.pin_for(7_000, Some("acp")).unwrap().file,
            ams_cata
        );

        // 类型对、文件自报别的 dbnum：核头失败，回落。
        let mut wrong_dbnum = directory.clone();
        wrong_dbnum.project_files.clear();
        wrong_dbnum.register_project_file("acp", 7_000, "CATA", acp_wrong_dbnum.clone());
        assert_eq!(
            wrong_dbnum.pin_for(7_000, Some("acp")).unwrap().file,
            ams_cata
        );
        assert_eq!(
            verify_header(7_000, &acp_wrong_dbnum, "CATA"),
            Err(RouteError::HeaderMismatch {
                dbnum: 7_000,
                path: acp_wrong_dbnum,
                header_dbnum: 7_001,
                header_type: "CATA".into(),
                expected_type: "CATA".into(),
            })
        );
    }

    /// `verify_all_headers`：一致的静默，不一致的逐条点名，读不出头的也点名。
    #[test]
    fn verify_all_headers_names_every_mismatch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let good = write_fake_db(tmp.path(), "ams8000_0001", 8_000, "DESI");
        let lying = write_fake_db(tmp.path(), "ams8001_0001", 8_002, "DESI");
        let garbage = tmp.path().join("ams8003_0001");
        std::fs::write(&garbage, b"not a database").unwrap();

        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                declared(8_000, 1, "/A", good.to_str()),
                declared(8_001, 1, "/B", lying.to_str()),
                declared(8_003, 1, "/C", garbage.to_str()),
            ],
            TEMPLATE_DIR,
        );
        let mut errors = directory.verify_all_headers();
        errors.sort_by_key(|error| match error {
            RouteError::HeaderMismatch { dbnum, .. }
            | RouteError::HeaderUnreadable { dbnum, .. } => *dbnum,
            _ => u32::MAX,
        });
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(matches!(
            &errors[0],
            RouteError::HeaderMismatch {
                dbnum: 8_001,
                header_dbnum: 8_002,
                ..
            }
        ));
        assert!(matches!(
            &errors[1],
            RouteError::HeaderUnreadable { dbnum: 8_003, .. }
        ));
    }

    /// 依赖目录：只读头、只收目录侧类型、MDB 已声明的不覆盖、声明了没文件的不补。
    #[test]
    fn dependency_dir_adds_catalogue_routes_without_touching_declared_ones() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ams_root = tmp.path().join("ams");
        let acp_root = tmp.path().join("acp");
        std::fs::create_dir_all(acp_root.join("backup")).unwrap();
        std::fs::create_dir_all(&ams_root).unwrap();
        let ams_cata = write_fake_db(&ams_root, "ams7000_0001", 7_000, "CATA");
        let acp_same_number = write_fake_db(&acp_root, "acp7000_0001", 7_000, "CATA");
        let acp_new_cata = write_fake_db(&acp_root, "acp7100_0001", 7_100, "CATA");
        let acp_prop = write_fake_db(&acp_root, "acp7200_0001", 7_200, "PROP");
        let _acp_desi = write_fake_db(&acp_root, "acp7015_0001", 7_015, "DESI");
        let _acp_dict = write_fake_db(&acp_root, "acp7300_0001", 7_300, "DICT");
        let _acp_declared_missing = write_fake_db(&acp_root, "acp7400_0001", 7_400, "CATA");
        let _in_backup = write_fake_db(&acp_root.join("backup"), "acp7500_0001", 7_500, "CATA");

        let mut directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                MdbDatabase {
                    dbnum: 7_000,
                    styp: 2,
                    proj: 0,
                    declared_project_code: None,
                    name: "/AMS/CATA".into(),
                    path: Some(ams_cata.clone()),
                    project: Some("ams".into()),
                    shadowed: Vec::new(),
                },
                declared(7_400, 2, "/AMS/CATA-MISSING", None),
            ],
            TEMPLATE_DIR,
        );
        let scan = directory.add_dependency_dir("acp", &acp_root);
        assert_eq!(
            scan,
            DependencyScan {
                candidates: 4,
                added_routes: 2,
                duplicate_keys: 0
            },
            "{scan:?}"
        );

        // 声明的 7000 仍指 MDB 那份；acp 的同号副本成了 preferred_project 候选。
        assert_eq!(directory.route_dbno(7_000).unwrap().path, ams_cata);
        assert_eq!(
            directory.route_dbno(7_000).unwrap().origin,
            RouteOrigin::Mdb
        );
        assert_eq!(
            directory.pin_for(7_000, Some("acp")).unwrap().file,
            acp_same_number
        );

        // MDB 没声明的目录库新增为依赖路由。
        let new_cata = directory.route_dbno(7_100).unwrap();
        assert_eq!(
            (new_cata.origin, new_cata.path.clone()),
            (RouteOrigin::Dependency, acp_new_cata)
        );
        assert_eq!(new_cata.project, "acp");
        assert_eq!(directory.route_dbno(7_200).unwrap().path, acp_prop);

        // DESI / DICT 不收；backup 目录不看；声明了没文件的照样报 NoFileForDbnum。
        assert!(matches!(
            directory.route_dbno(7_015),
            Err(RouteError::DbNotInMdb { .. })
        ));
        assert!(matches!(
            directory.route_dbno(7_300),
            Err(RouteError::DbNotInMdb { .. })
        ));
        assert!(matches!(
            directory.route_dbno(7_500),
            Err(RouteError::DbNotInMdb { .. })
        ));
        assert!(matches!(
            directory.route_dbno(7_400),
            Err(RouteError::NoFileForDbnum { .. })
        ));

        let stats = directory.stats();
        assert_eq!(
            (
                stats.mdb_routes,
                stats.dependency_routes,
                stats.declared_without_file
            ),
            (1, 2, 1)
        );
    }

    // ---- 会话世代：四页合成库（描述符 + 会话页 + 索引叶 + 记录），照 refno_native_stage0.rs 的写法 ----

    const WORDS_PER_PAGE: u32 = 64;
    const PAGE_BYTES: usize = WORDS_PER_PAGE as usize * 4;

    fn put_be(page: &mut [u8], offset: usize, value: u32) {
        page[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    /// 一个 `ReadOnlyEngine::open` 开得起来的最小库：第 0 页描述符（自报 `dbnum` / 类型 / 会话页在第 1 页）、
    /// 第 1 页会话页（`+0x0C` 会话号）、第 2 页一条索引叶、第 3 页一条记录。
    fn session_db_pages(dbnum: u32, db_type: &str, sesno: u32) -> Vec<u8> {
        let element = RefNo::new(word0_for(dbnum, 1), 42);
        let mut pages = vec![vec![0_u8; PAGE_BYTES]; 4];
        put_be(&mut pages[0], 0x04, 2);
        put_be(&mut pages[0], 0x08, dbnum);
        put_be(&mut pages[0], 0x20, e3d_attlib::db1_hash(db_type));
        put_be(&mut pages[0], 0x28, 1);
        put_be(&mut pages[0], 0x2C, 1);
        put_be(&mut pages[0], 0x30, 1);
        put_be(&mut pages[0], 0x34, WORDS_PER_PAGE);
        put_be(&mut pages[0], 0x38, element.word0);
        put_be(&mut pages[1], 0x00, 3);
        put_be(&mut pages[1], 0x04, u32::MAX);
        put_be(&mut pages[1], 0x0C, sesno);
        put_be(&mut pages[1], 0x1C, 2);
        put_be(&mut pages[1], 0x20, 1);
        put_be(&mut pages[2], 0x00, 5);
        for (index, value) in [13_387_743_u32, 0, 2, 2, 0, 53].into_iter().enumerate() {
            put_be(&mut pages[2], (index + 1) * 4, value);
        }
        for (index, value) in [element.word0, element.word1, 3, (20_u32 << 12) | 1]
            .into_iter()
            .enumerate()
        {
            put_be(&mut pages[2], (7 + index) * 4, value);
        }
        put_be(&mut pages[3], 0x00, 7);
        for (index, value) in [
            11,
            element.word0,
            element.word1,
            0x000A_AA01,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            put_be(&mut pages[3], 40 + index * 4, value);
        }
        pages.concat()
    }

    /// 写一个库；`extra_pages` 模拟追加写过的尾巴（E3D 的保存是追加写，长度随会话一起变）。
    fn write_session_db(
        dir: &Path,
        name: &str,
        dbnum: u32,
        db_type: &str,
        sesno: u32,
        extra_pages: usize,
    ) -> PathBuf {
        let mut bytes = session_db_pages(dbnum, db_type, sesno);
        bytes.extend(std::iter::repeat_n(0_u8, PAGE_BYTES * extra_pages));
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write synthetic session db");
        path
    }

    #[test]
    fn poll_detects_backtrack_between_two_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_session_db(tmp.path(), "ams8000_0001", 8000, "DESI", 242, 0);
        let directory = MdbRouteDirectory::from_databases("ams", "ALL",
            &[declared(8000, 1, "/AMS/DESI", path.to_str())], TEMPLATE_DIR);
        assert!(directory.poll_generations().is_empty());
        // 244 is an ordinary save on 243, but 243 rolled back to 239 from 242.
        let full_pages = |sesno| {
            let short = session_db_pages(8000, "DESI", sesno);
            let mut bytes = Vec::new();
            for page in short.chunks(PAGE_BYTES) {
                bytes.extend_from_slice(page);
                bytes.resize(bytes.len() + 2048 - PAGE_BYTES, 0);
            }
            put_be(&mut bytes, 0x34, 512);
            bytes
        };
        let mut bytes = full_pages(244);
        put_be(&mut bytes, 2048 + 0x7F0, 243);
        put_be(&mut bytes, 2048 + 0x04, 4);
        for (sesno, base, previous) in [(243, 239, 5), (242, 241, u32::MAX)] {
            let source = full_pages(sesno);
            let mut page = source[2048..4096].to_vec();
            put_be(&mut page, 0x7F0, base);
            put_be(&mut page, 0x04, previous);
            bytes.extend(page);
        }
        std::fs::write(&path, bytes).unwrap();
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, GenerationChange::RolledBack);
        assert_eq!(changes[0].to.latest_sesno, 244);
        assert_eq!(changes[0].to.base_sesno, Some(243));
        assert!(directory.poll_generations().is_empty());
    }

    fn generation_pair(directory: &MdbRouteDirectory, dbnum: u32) -> Option<(u64, u32)> {
        directory
            .generation_of(dbnum)
            .map(|entry| (entry.generation, entry.latest_sesno))
    }

    /// lazy-startup L4 / 母计划不变量 7：第一次轮询只记基线；之后 8000 保存了只报 8000，7998 的世代
    /// 一动不动；前进 / 回滚 / 换文件三种变化分得开；目录库的变化同样报出来；声明了没文件的库不进账本。
    #[test]
    fn poll_generations_reports_only_the_dbnum_whose_file_changed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let desi_8000 = write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 37, 0);
        let desi_7998 = write_session_db(tmp.path(), "ams7998_0001", 7_998, "DESI", 5, 0);
        let cata_5052 = write_session_db(tmp.path(), "ams5052_0001", 5_052, "CATA", 189, 0);
        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[
                declared(8_000, 1, "/AMS/DESI", desi_8000.to_str()),
                declared(7_998, 1, "/AMS/DESI2", desi_7998.to_str()),
                declared(5_052, 2, "/AMS/CATA", cata_5052.to_str()),
                declared(7_000, 2, "/AMS/CATA-MISSING", None),
            ],
            TEMPLATE_DIR,
        );

        assert!(
            directory.poll_generations().is_empty(),
            "第一次见只记基线，不算变化"
        );
        assert_eq!(generation_pair(&directory, 8_000), Some((1, 37)));
        assert_eq!(generation_pair(&directory, 7_998), Some((1, 5)));
        assert_eq!(generation_pair(&directory, 5_052), Some((1, 189)));
        assert_eq!(
            generation_pair(&directory, 7_000),
            None,
            "没文件的库不进账本"
        );
        assert_eq!(directory.generations_epoch(), 3);
        assert!(directory.poll_generations().is_empty(), "没动就没变化");

        // 8000 SAVEWORK：会话 37 → 38，文件长了一页。
        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 38, 1);
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1, "{changes:?}");
        let change = &changes[0];
        assert_eq!(
            (change.dbnum, change.db_type.as_str(), change.kind),
            (8_000, "DESI", GenerationChange::SessionAdvanced)
        );
        assert_eq!(change.path, desi_8000);
        assert_eq!((change.from.generation, change.from.latest_sesno), (1, 37));
        assert_eq!((change.to.generation, change.to.latest_sesno), (2, 38));
        assert_eq!(
            generation_pair(&directory, 7_998),
            Some((1, 5)),
            "7998 一动不动"
        );
        assert_eq!(directory.generations_epoch(), 4);

        // 恢复了旧备份：会话号变小。
        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 30, 0);
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].kind, GenerationChange::RolledBack);
        assert_eq!(
            (changes[0].to.generation, changes[0].to.latest_sesno),
            (3, 30)
        );

        // 路径不变、文件换了、会话号恰好相同：只有身份能分出来。
        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 30, 2);
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].kind, GenerationChange::FileReplaced);
        assert_eq!(changes[0].to.generation, 4);

        // 目录库的变化同样报出来（怎么处置是调用方的事）。
        write_session_db(tmp.path(), "ams5052_0001", 5_052, "CATA", 190, 1);
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (
                changes[0].dbnum,
                changes[0].db_type.as_str(),
                changes[0].kind
            ),
            (5_052, "CATA", GenerationChange::SessionAdvanced)
        );

        // 两个库同一拍都变：各报一条，按 dbnum 升序。
        write_session_db(tmp.path(), "ams7998_0001", 7_998, "DESI", 6, 1);
        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 31, 3);
        let dbnums: Vec<u32> = directory
            .poll_generations()
            .iter()
            .map(|change| change.dbnum)
            .collect();
        assert_eq!(dbnums, vec![7_998, 8_000]);

        let snapshot = directory.generations();
        assert_eq!(
            snapshot
                .iter()
                .map(|(dbnum, entry)| (*dbnum, entry.generation))
                .collect::<Vec<_>>(),
            vec![(5_052, 2), (7_998, 2), (8_000, 5)]
        );
        assert_eq!(directory.generations_epoch(), 9);
    }

    /// 文件读不出来（删了、坏了）那一轮跳过、不算变化、账本不动；回来之后按身份 / 会话号照常判。
    #[test]
    fn unreadable_files_are_skipped_without_touching_the_ledger() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let desi = write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 37, 0);
        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[declared(8_000, 1, "/AMS/DESI", desi.to_str())],
            TEMPLATE_DIR,
        );
        assert!(directory.poll_generations().is_empty());

        std::fs::remove_file(&desi).unwrap();
        assert!(directory.poll_generations().is_empty(), "stat 不到：跳过");
        assert_eq!(generation_pair(&directory, 8_000), Some((1, 37)));

        std::fs::write(&desi, b"not a database at all").unwrap();
        assert!(directory.poll_generations().is_empty(), "读不出头：跳过");
        assert_eq!(generation_pair(&directory, 8_000), Some((1, 37)));

        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 38, 1);
        let changes = directory.poll_generations();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, GenerationChange::SessionAdvanced);
        assert_eq!(generation_pair(&directory, 8_000), Some((2, 38)));
    }

    /// 克隆带走账本的快照，之后两边各记各的。
    #[test]
    fn cloning_copies_the_ledger_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let desi = write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 37, 0);
        let directory = MdbRouteDirectory::from_databases(
            "ams",
            "ALL",
            &[declared(8_000, 1, "/AMS/DESI", desi.to_str())],
            TEMPLATE_DIR,
        );
        directory.poll_generations();
        let copy = directory.clone();
        assert_eq!(generation_pair(&copy, 8_000), Some((1, 37)));
        write_session_db(tmp.path(), "ams8000_0001", 8_000, "DESI", 38, 1);
        assert_eq!(directory.poll_generations().len(), 1);
        assert_eq!(generation_pair(&copy, 8_000), Some((1, 37)), "副本没跟着动");
    }

    #[test]
    fn declared_without_file_is_listed_by_dbnum() {
        let directory = directory();
        assert!(directory.contains_dbnum(7_000));
        assert!(directory.file_of(7_000).is_none());
        assert!(!directory.contains_dbnum(999_999));
        assert_eq!(
            directory.declared_without_file().collect::<Vec<_>>(),
            vec![(
                7_000,
                &MissingMember {
                    name: "/AMS/CATA2".to_string(),
                    project_code: None,
                }
            )]
        );
    }

    /// 声明落空的条目带着项目码一路走到底（ADR-078 D5）：`route_dbno` 的错误点名项目 ACP 与
    /// `acp000`，本项目自己的库措辞分开；按项目码归并的视图供横幅用。
    #[test]
    fn a_declaration_without_file_keeps_its_project_code() {
        let databases = vec![
            MdbDatabase {
                dbnum: 7_001,
                styp: 2,
                proj: 3,
                declared_project_code: Some("ACP".to_string()),
                name: "*MASTER/STLCATA".to_string(),
                path: None,
                project: None,
                shadowed: Vec::new(),
            },
            MdbDatabase {
                dbnum: 7_009,
                styp: 2,
                proj: 3,
                declared_project_code: Some("ACP".to_string()),
                name: "/ACP/CATA".to_string(),
                path: None,
                project: None,
                shadowed: Vec::new(),
            },
            MdbDatabase {
                dbnum: 8_100,
                styp: 1,
                proj: 0,
                declared_project_code: None,
                name: "/AMS/LOST".to_string(),
                path: None,
                project: None,
                shadowed: Vec::new(),
            },
        ];
        let directory = MdbRouteDirectory::from_databases("AvevaMarineSample", "ALL", &databases, "T");
        let foreign = directory.route_dbno(7_001).unwrap_err().to_string();
        assert!(foreign.contains("项目 ACP") && foreign.contains("acp000"), "{foreign}");
        assert!(foreign.contains("project_search_paths"), "{foreign}");
        let own = directory.route_dbno(8_100).unwrap_err().to_string();
        assert!(own.contains("本项目") && !own.contains("project_search_paths"), "{own}");

        let grouped = directory.declared_without_file_by_project();
        assert_eq!(grouped.get(&Some("ACP".to_string())), Some(&vec![7_001, 7_009]));
        assert_eq!(grouped.get(&None), Some(&vec![8_100]));
    }

    /// e3d-io 那句「db N 没注册进这个 DbSet」到现场只剩一个库号；补上路由目录的原因后是
    /// 「项目 ACP 的目录没定位到」。没那句话、或库号其实有路由时原样返回。
    #[test]
    fn unregistered_db_messages_get_the_route_directory_reason_appended() {
        let databases = vec![MdbDatabase {
            dbnum: 7_001,
            styp: 2,
            proj: 3,
            declared_project_code: Some("ACP".to_string()),
            name: "*MASTER/STLCATA".to_string(),
            path: None,
            project: None,
            shadowed: Vec::new(),
        }];
        let directory = MdbRouteDirectory::from_databases("explain-test", "ALL", &databases, "T");
        let raw = "14 geometry element(s) failed: [(17496/156982, \"db 7001 没注册进这个 DbSet，resolver 也给不出它的文件\"), (17496/156983, \"db 7001 没注册进这个 DbSet，resolver 也给不出它的文件\")]";
        let explained = explain_unregistered_dbs_with(raw, &directory);
        assert!(explained.starts_with(raw), "{explained}");
        assert!(explained.contains("dbnum 7001：MDB /ALL 声明了 dbnum 7001"), "{explained}");
        assert!(explained.contains("项目 ACP"), "{explained}");
        assert_eq!(explained.matches("项目 ACP").count(), 1, "同一个库号只解释一次");
        assert_eq!(explain_unregistered_dbs_with("nothing to see", &directory), "nothing to see");
        let not_in_mdb = explain_unregistered_dbs_with("db 9999 没注册进这个 DbSet", &directory);
        assert!(not_in_mdb.contains("不在 MDB /ALL 的声明里"), "{not_in_mdb}");

        // GitHub #46：依赖目录扫描补上的路由（MDB 没声明）也要说清——它有路由，但生成不读它。
        let mut directory = directory;
        directory.routes.insert(
            7_016,
            DbRoute {
                dbnum: 7_016,
                db_type: "CATA".to_string(),
                project: "AvevaCatalogue".to_string(),
                path: PathBuf::from(r"D:\acp000\acp7016_0001"),
                shadowed: Vec::new(),
                origin: RouteOrigin::Dependency,
            },
        );
        let undeclared = explain_unregistered_dbs_with(
            "db 7016 没注册进这个 DbSet，resolver 也给不出它的文件",
            &directory,
        );
        assert!(
            undeclared.contains(
                "dbnum 7016：MDB /ALL 没声明它，只在依赖目录扫描里找到（项目 AvevaCatalogue"
            ) && undeclared.contains("E3D 用这个 MDB 同样读不到它"),
            "{undeclared}"
        );
    }

    /// 一段报错里点名的库号去重取出；「db 」后面紧跟库号才算，其它数字不作数。
    #[test]
    fn unregistered_dbnums_are_read_off_every_sentence_in_a_message() {
        assert_eq!(
            unregistered_dbnums(
                "element 17496/7388 failed: db 5052 没注册进这个 DbSet，resolver 也给不出它的文件；\
                 db 7001 没注册进这个 DbSet；db 5052 没注册进这个 DbSet"
            ),
            BTreeSet::from([5_052, 7_001])
        );
        assert!(unregistered_dbnums("session 721 没注册进这个 DbSet").is_empty());
        assert!(unregistered_dbnums("DESP length 2 < 10").is_empty());
    }

    /// 真库门（手动）：当前 DbOption 指向的 MDB，目录构造零元素扫描、每条路由的文件头自证一致、
    /// 每个元素库描述符里的首 extent word0 解码回同一个 dbnum。
    #[test]
    #[ignore = "manual live: needs DbOption.toml pointing at a real E3D project and MDB"]
    fn live_directory_matches_every_file_header() {
        let option = aios_core::get_db_option();
        let membership =
            super::super::mdb_membership::resolve(option, &option.project_name, &option.mdb_name)
                .expect("resolve MDB membership from SYS");
        let started = std::time::Instant::now();
        let directory = MdbRouteDirectory::from_membership(&membership, "T:/unused");
        let elapsed = started.elapsed();
        eprintln!(
            "[route-live] {} routes ({} element) in {elapsed:?}; problems={:?}",
            directory.len(),
            directory.stats().element_routes,
            directory.problems()
        );
        assert!(!directory.is_empty(), "MDB 一个库都没路由出来");
        assert!(
            elapsed.as_millis() < 2_000,
            "目录构造不该读元素：{elapsed:?}"
        );
        let errors = directory.verify_all_headers();
        assert!(errors.is_empty(), "{errors:#?}");
        for route in directory.element_databases() {
            let engine = e3d_io::ReadOnlyEngine::open(&route.path).expect("open element db");
            let first_extent = RefNo::new(engine.descriptor().primary_extent_ref, 1);
            assert_eq!(
                refno_dbnum_of(first_extent),
                Some(route.dbnum),
                "{}: 首 extent word0 解码与目录不一致",
                route.path.display()
            );
        }
    }
}
