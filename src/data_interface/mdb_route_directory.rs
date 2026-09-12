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
//! **W3 之前不要把它塞给 `DirectStore::new`**：它作为 [`CataDbLocator`] 兼容壳时
//! `dbnum_of_ref0` 恒 `None`——那条老路本来就该断，但 `DirectStore::dbnum_of` 换成位解码
//! 之前，断在这里只会让每次读都报 `UnresolvedRef0`。

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use aios_core::RefU64;
use e3d_io::db_element::{DbFilePin, DbFileResolver, template_file_for};
use e3d_io::refno::RefNo;

use super::cata_closure::CataDbLocator;
use super::db_file_header::DbBasicInfo;
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
    /// MDB 声明了它，配置的目录下却没有文件——部署问题，要看见。
    NoFileForDbnum {
        dbnum: u32,
        mdb: String,
        name: String,
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
            Self::NoFileForDbnum { dbnum, mdb, name } => write!(
                f,
                "MDB {mdb} 声明了 dbnum {dbnum}（{name}），但配置的目录下没有它的文件"
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

/// 见模块头。
#[derive(Debug, Clone)]
pub struct MdbRouteDirectory {
    project: String,
    mdb: String,
    template_dir: PathBuf,
    routes: BTreeMap<u32, DbRoute>,
    /// CURD 原序的 dbnum：`routes` 是按号排的，树首页与增量排工要按 CURD 序。
    curd_order: Vec<u32>,
    /// 声明了但磁盘上没文件的库：`dbnum → SYS 元素名`。报 `NoFileForDbnum`，不混进 `DbNotInMdb`。
    declared_without_file: BTreeMap<u32, String>,
    /// `(project, dbnum) → 文件`，同号跨项目候选。
    project_files: HashMap<(String, u32), ProjectDbFile>,
    /// 构造期发现、该让人看见的事（CURD 重复声明、认不出的 STYP）。
    problems: Vec<String>,
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
                    .or_insert_with(|| db.name.clone());
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
        if let Some(name) = self.declared_without_file.get(&dbnum) {
            return Err(RouteError::NoFileForDbnum {
                dbnum,
                mdb: self.mdb.clone(),
                name: name.clone(),
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
                name: "/AMS/CATA2".to_string()
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
