//! MDB 闭包（ADR-078 D1）：这个服务要开哪些项目，由 MDB 说了算。
//!
//! 主项目 SYS 里该 MDB 的 CURD 声明每条自带项目码（`PROJ` 字符向量，空 = 主项目），按码分组就是
//! 「项目闭包」；每个外部项目码交给 [`super::project_locator`]，只在人指定的搜索根里按
//! `<code>000` 找目录。core.dll 从头到尾就是这么做的（`DB_DB::makeDirectoryName` 拼
//! `/%<PROJ>000%`），没有「项目名单」这回事；仓里过去用人手勾的 `included_projects` 替它，
//! 少勾一个就成批落空（2026-09-15 部署事故：AMS `/ALL` 41 条声明落空，ACP 34 / SCB 4 / APS 2 / ZDJ 1）。
//!
//! 两层缓存，都只活在进程内、配置文件里不写第二份真值：
//!
//! - **声明**（[`declaration`]）：SYS 读一次，按 `(project, mdb)` 缓存。它只要项目码，不绑定文件——
//!   绑定是 [`super::mdb_membership::resolve`] 的事，那边拿同一份声明加上这里定位出来的目录去绑。
//! - **闭包计划**（[`latest`]）：每次 [`resolve_for`] 现算（搜索根要重扫，掉线的共享盘回来了
//!   要能看见）并留下最近一份，给 `/health.mdb_closure`、启动横幅与
//!   [`super::project_paths::resolve_project_root`] 这类不该自己再去扫盘的读者。
//!
//! 缺口不拒绝启动（d-682 第 5 条）：`unresolved` 非空或有根不可达只进 `degraded_sections`，
//! 已定位项目的数据面照常。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use aios_core::options::DbOption;
use serde_json::json;

use super::mdb_membership::MdbDatabase;
use super::project_locator::{
    DEFAULT_ROOT_SCAN_TIMEOUT, LocateFailure, ProjectLocation, RootScan, locate_code,
    locate_project_folder, scan_roots,
};
use super::project_paths::{
    ProjectWatchDirs, WatchDirPlan, normalize_path_input, path_identity, path_key,
};

/// 主项目 SYS 里一个 MDB 的声明，**未绑定文件**：每条 `MdbDatabase` 的 `path` / `project` /
/// `shadowed` 都是空的，只有 dbnum / 名字 / STYP / 项目码。
#[derive(Debug, Clone)]
pub struct MdbDeclaration {
    pub project: String,
    /// 归一成 `/ALL` 形式。
    pub mdb: String,
    /// 声明它的 SYS 文件。
    pub sys: PathBuf,
    /// CURD 序。
    pub databases: Vec<MdbDatabase>,
    /// 读 SYS 时该让人看见的事（DBSET 展开问题、跳过的候选）。
    pub problems: Vec<String>,
}

impl MdbDeclaration {
    /// 外部项目码 → 声明的 dbnum（CURD 序，首次出现的顺序排码）。空码（主项目）不在里面。
    pub fn dbnums_by_code(&self) -> Vec<(String, Vec<&MdbDatabase>)> {
        let mut order: Vec<String> = Vec::new();
        let mut grouped: BTreeMap<String, Vec<&MdbDatabase>> = BTreeMap::new();
        for db in &self.databases {
            let Some(code) = db.declared_project_code.as_deref() else {
                continue;
            };
            let code = code.to_ascii_uppercase();
            if !grouped.contains_key(&code) {
                order.push(code.clone());
            }
            grouped.entry(code).or_default().push(db);
        }
        order
            .into_iter()
            .map(|code| {
                let dbs = grouped.remove(&code).unwrap_or_default();
                (code, dbs)
            })
            .collect()
    }

    /// 主项目自己的声明（`PROJ` 为空）。
    pub fn own_dbnums(&self) -> Vec<u32> {
        self.databases
            .iter()
            .filter(|db| db.declared_project_code.is_none())
            .map(|db| db.dbnum)
            .collect()
    }
}

fn normalize_mdb(mdb: &str) -> String {
    format!("/{}", mdb.trim().trim_start_matches('/'))
}

type DeclarationCache = Vec<(String, String, Arc<MdbDeclaration>)>;

fn declarations() -> &'static RwLock<DeclarationCache> {
    static CACHE: OnceLock<RwLock<DeclarationCache>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

/// 已缓存的声明，没有就 `None`——不读 SYS。
pub fn cached_declaration(project: &str, mdb: &str) -> Option<Arc<MdbDeclaration>> {
    let wanted = normalize_mdb(mdb);
    let cache = declarations().read().ok()?;
    cache
        .iter()
        .find(|(known_project, known_mdb, _)| known_project == project && *known_mdb == wanted)
        .map(|(_, _, declaration)| declaration.clone())
}

/// 登记一份声明（替换同 `(project, mdb)` 的旧条目）。
pub fn install_declaration(declaration: MdbDeclaration) -> Arc<MdbDeclaration> {
    let shared = Arc::new(declaration);
    if let Ok(mut cache) = declarations().write() {
        let key = (shared.project.clone(), shared.mdb.clone());
        cache.retain(|(project, mdb, _)| (project.clone(), mdb.clone()) != key);
        cache.push((key.0, key.1, shared.clone()));
    }
    shared
}

/// 主项目 `project` 的 SYS 里 `mdb` 的声明：有缓存用缓存，没有就在 `main_db_dirs` 里找 SYS 读一次。
///
/// 读不到（目录里没有 SYS、SYS 里没有这个 MDB、解析失败）原样把 [`super::mdb_membership`]
/// 那句带候选名单的错误抛出来，不缓存失败——重挂轮下一次还会再试。
pub fn declaration(
    main_db_dirs: &[PathBuf],
    project: &str,
    mdb: &str,
) -> anyhow::Result<Arc<MdbDeclaration>> {
    if let Some(cached) = cached_declaration(project, mdb) {
        return Ok(cached);
    }
    reload_declaration(main_db_dirs, project, mdb)
}

/// 从 `main_db_dirs` 里的 SYS 重读 `mdb` 的声明，**读成功才替换**缓存（ADR-085 D2：先读后换）。
///
/// 读失败原样返回、缓存不动：SYST 刚落库时读不动 SYS（被 E3D 占着、共享盘抖一下）不该让进程
/// 丢掉手里那份还能用的声明——先清后读的话，在下一次读成功之前，闭包、路由目录与本期执行范围
/// 全都会退化成「主项目 SYS 未读到」。
pub fn reload_declaration(
    main_db_dirs: &[PathBuf],
    project: &str,
    mdb: &str,
) -> anyhow::Result<Arc<MdbDeclaration>> {
    // 远程盘上这是绑定链里最重的一段读：列 SYS 候选（read_dir）+ SysDb 建索引 +
    // 逐 DB 属性读，全无超时。打一对边界，卡住时知道停在「读 SYS」而不是目录枚举
    // （2026-10-15 JEU 现场排查缺的就是这一句）。
    let started = Instant::now();
    println!("[mdb-bind] 声明解析开始: project={project} mdb={mdb} dirs={main_db_dirs:?}");
    let read = super::mdb_membership::read_unbound_declaration(main_db_dirs, project, mdb)?;
    println!(
        "[mdb-bind] 声明解析完成: project={project} mdb={mdb} sys={} declarations={} problems={} elapsed_ms={}",
        read.sys.display(),
        read.databases.len(),
        read.problems.len(),
        started.elapsed().as_millis()
    );
    Ok(install_declaration(MdbDeclaration {
        project: project.to_string(),
        mdb: normalize_mdb(mdb),
        sys: read.sys,
        databases: read.databases,
        problems: read.problems,
    }))
}

/// 单测 / 探针用：不读 SYS，直接把一份声明当成主项目 `project` 的 `mdb` 声明。
/// `entries` 是 `(项目码或空, dbnum, 名字)`。
pub fn seed_declaration(project: &str, mdb: &str, entries: &[(Option<&str>, u32, &str)]) {
    let databases = entries
        .iter()
        .map(|(code, dbnum, name)| MdbDatabase {
            dbnum: *dbnum,
            styp: super::mdb_membership::DESI_STYP,
            proj: code.map_or(0, |code| code.len() as i64),
            declared_project_code: code.map(|code| code.to_ascii_uppercase()),
            name: name.to_string(),
            path: None,
            project: None,
            shadowed: Vec::new(),
        })
        .collect();
    install_declaration(MdbDeclaration {
        project: project.to_string(),
        mdb: normalize_mdb(mdb),
        sys: PathBuf::from("<seeded>"),
        databases,
        problems: Vec::new(),
    });
}

/// 忘掉 `(project, mdb)` 的声明缓存；下一次 [`declaration`] 重读 SYS。
pub fn forget_declaration(project: &str, mdb: &str) {
    let wanted = normalize_mdb(mdb);
    if let Ok(mut cache) = declarations().write() {
        cache.retain(|(known_project, known_mdb, _)| {
            !(known_project == project && *known_mdb == wanted)
        });
    }
}

/// 一个搜索根的结论。
#[derive(Debug, Clone)]
pub struct ClosureRoot {
    pub path: PathBuf,
    pub reachable: bool,
    /// 这个根里见到的项目码（小写）。
    pub codes_seen: Vec<String>,
    pub problem: Option<String>,
    pub elapsed: Duration,
}

/// 闭包里的一个项目（主项目或 MDB 声明的外部项目），目录已定位。
#[derive(Debug, Clone)]
pub struct ClosureProject {
    /// 项目名：主项目是配置的 `project_name`；外部项目是项目目录的文件夹名（`AvevaCatalogue`），
    /// 与过去写在 `included_projects` 里的一致，所以数据库命名空间、路由来源、归属登记都不换名。
    pub name: String,
    /// 大写项目码；一个项目目录下多个库目录时全部列出。
    pub codes: Vec<String>,
    pub is_main: bool,
    /// 在哪个搜索根里找到的（下标进 [`ClosurePlan::roots`]）。
    pub root_index: usize,
    pub project_dir: PathBuf,
    pub db_dirs: Vec<PathBuf>,
    /// MDB 声明中属于这个项目的 dbnum（CURD 序）。
    pub dbnums: Vec<u32>,
    /// 后面的根里同码的副本：只记不选。
    pub shadowed: Vec<PathBuf>,
}

/// 搜索根里见到的一个项目目录，不论 MDB 有没有声明它：向导挑主项目的候选、`/health` 里
/// 「盘上有什么」那一栏。
#[derive(Debug, Clone)]
pub struct SeenProject {
    /// 项目目录的文件夹名。
    pub name: String,
    /// 大写项目码，一个目录下多个库目录时全部列出。
    pub codes: Vec<String>,
    pub root_index: usize,
    pub project_dir: PathBuf,
}

/// MDB 声明了、搜索根里却找不到目录的项目码。
#[derive(Debug, Clone)]
pub struct UnresolvedProject {
    pub code: String,
    pub dbnums: Vec<u32>,
    pub names: Vec<String>,
    pub reason: String,
}

/// 一次闭包解析的全部结论。
#[derive(Debug, Clone)]
pub struct ClosurePlan {
    pub project: String,
    pub mdb: String,
    pub roots: Vec<ClosureRoot>,
    /// 主项目在 `projects` 里的下标；`None` = 没定位到（`problems` 里有原因）。
    pub main: Option<usize>,
    pub projects: Vec<ClosureProject>,
    /// 搜索根里见到的全部项目目录（按根序、再按路径），不论 MDB 有没有声明。
    pub seen_projects: Vec<SeenProject>,
    pub unresolved: Vec<UnresolvedProject>,
    /// 定位期的冲突、命名撞车等，给人看。
    pub problems: Vec<String>,
    /// 声明读到了就在这里；读不到时 `declaration_error` 有值。
    pub declaration: Option<Arc<MdbDeclaration>>,
    pub declaration_error: Option<String>,
    pub computed_at: SystemTime,
}

impl ClosurePlan {
    /// 纯函数：把扫描结论、主项目定位、声明拼成闭包。不碰磁盘，单测直接喂。
    pub fn build(
        project_name: &str,
        mdb: &str,
        scans: Vec<RootScan>,
        main: Result<ProjectLocation, LocateFailure>,
        declaration: Result<Arc<MdbDeclaration>, String>,
    ) -> Self {
        let roots = scans
            .iter()
            .map(|scan| ClosureRoot {
                path: scan.root.clone(),
                reachable: scan.reachable,
                codes_seen: scan.codes_seen(),
                problem: scan.problem.clone(),
                elapsed: scan.elapsed,
            })
            .collect();
        let mut seen_projects: Vec<SeenProject> = Vec::new();
        for (root_index, scan) in scans.iter().enumerate() {
            for hit in &scan.hits {
                let identity = path_identity(&hit.project_dir);
                let code = hit.code.to_ascii_uppercase();
                match seen_projects.iter_mut().find(|seen| {
                    seen.root_index == root_index && path_identity(&seen.project_dir) == identity
                }) {
                    Some(seen) => {
                        if !seen.codes.contains(&code) {
                            seen.codes.push(code);
                            seen.codes.sort();
                        }
                    }
                    None => seen_projects.push(SeenProject {
                        name: hit
                            .project_dir
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| hit.project_dir.display().to_string()),
                        codes: vec![code],
                        root_index,
                        project_dir: hit.project_dir.clone(),
                    }),
                }
            }
        }
        let mut plan = Self {
            project: project_name.to_string(),
            mdb: normalize_mdb(mdb),
            roots,
            main: None,
            projects: Vec::new(),
            seen_projects,
            unresolved: Vec::new(),
            problems: Vec::new(),
            declaration: None,
            declaration_error: None,
            computed_at: SystemTime::now(),
        };

        match main {
            Ok(location) => {
                plan.problems.extend(location.problems.iter().cloned());
                plan.projects.push(ClosureProject {
                    name: project_name.trim().to_string(),
                    codes: split_codes(&location.code),
                    is_main: true,
                    root_index: location.root_index,
                    project_dir: location.project_dir,
                    db_dirs: location.db_dirs,
                    dbnums: Vec::new(),
                    shadowed: location.shadowed,
                });
                plan.main = Some(0);
            }
            Err(failure) => plan.problems.push(format!(
                "主项目 {project_name:?}：{failure}。project_name 要么是搜索根下装着 <code>000 的项目文件夹名，\
                 要么是项目码本身"
            )),
        }

        match declaration {
            Ok(declaration) => {
                if let Some(main) = plan.main {
                    plan.projects[main].dbnums = declaration.own_dbnums();
                }
                for (code, dbs) in declaration.dbnums_by_code() {
                    let dbnums: Vec<u32> = dbs.iter().map(|db| db.dbnum).collect();
                    match locate_code(&scans, &code) {
                        Ok(location) => plan.admit(code, location, dbnums),
                        Err(failure) => plan.unresolved.push(UnresolvedProject {
                            code: code.clone(),
                            dbnums,
                            names: dbs.iter().map(|db| db.name.clone()).collect(),
                            reason: format!(
                                "搜索根里没有 {}000 库目录：{failure}",
                                code.to_ascii_lowercase()
                            ),
                        }),
                    }
                }
                plan.declaration = Some(declaration);
            }
            Err(error) => plan.declaration_error = Some(error),
        }
        plan
    }

    /// 把定位到的一个外部项目码并进闭包：与已有项目同目录的合并（主项目自己的码被显式声明、
    /// 一个项目目录下多个码），否则新开一条；文件夹名与已有项目撞车时改名并记问题。
    fn admit(&mut self, code: String, location: ProjectLocation, dbnums: Vec<u32>) {
        self.problems.extend(location.problems.iter().cloned());
        let identity = path_identity(&location.project_dir);
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|project| path_identity(&project.project_dir) == identity)
        {
            if !existing.codes.contains(&code) {
                existing.codes.push(code);
            }
            for dir in location.db_dirs {
                if !existing
                    .db_dirs
                    .iter()
                    .any(|known| path_identity(known) == path_identity(&dir))
                {
                    existing.db_dirs.push(dir);
                }
            }
            existing.dbnums.extend(dbnums);
            existing.shadowed.extend(location.shadowed);
            return;
        }
        let mut name = location
            .project_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| code.clone());
        if self
            .projects
            .iter()
            .any(|project| project.name.eq_ignore_ascii_case(&name))
        {
            let renamed = format!("{name}@root{}", location.root_index + 1);
            self.problems.push(format!(
                "项目码 {code} 定位到 {}，文件夹名 {name:?} 与闭包里另一个项目同名，登记为 {renamed:?}",
                location.project_dir.display()
            ));
            name = renamed;
        }
        self.projects.push(ClosureProject {
            name,
            codes: vec![code],
            is_main: false,
            root_index: location.root_index,
            project_dir: location.project_dir,
            db_dirs: location.db_dirs,
            dbnums,
            shadowed: location.shadowed,
        });
    }

    /// 闭包里的项目名，主项目在前，其余按 CURD 里首次出现的顺序——过去读 `included_projects`
    /// 的地方（目录库选主、依赖目录扫描）改读它。
    pub fn project_names(&self) -> Vec<String> {
        self.projects
            .iter()
            .map(|project| project.name.clone())
            .collect()
    }

    /// 按项目名或项目码（都不区分大小写）找项目目录。
    pub fn root_of_project(&self, project: &str) -> Option<PathBuf> {
        let wanted = project.trim();
        self.projects
            .iter()
            .find(|candidate| {
                candidate.name.eq_ignore_ascii_case(wanted)
                    || candidate
                        .codes
                        .iter()
                        .any(|code| code.eq_ignore_ascii_case(wanted))
            })
            .map(|candidate| candidate.project_dir.clone())
    }

    /// 有没有该进 `degraded_sections` 的事：声明落空、根不可达、主项目没定位、SYS 读不到。
    pub fn is_degraded(&self) -> bool {
        !self.unresolved.is_empty()
            || self.roots.iter().any(|root| !root.reachable)
            || self.main.is_none()
            || self.declaration_error.is_some()
    }

    /// 监控目录计划：主项目、闭包里的每个项目各一条；没定位到的项目码与读不到的声明也各占一条
    /// （`root: None` + `problem`），启动日志「监控目录解析」一段照旧逐条打出来。
    ///
    /// 两个项目指到同一个库目录时只列一次：`duplicate_dbnums_across_watch_dirs` 会把重复目录里的
    /// 同一个库看成「同 dbnum 两个文件」而整库阻断。
    pub fn watch_dir_plan(&self) -> WatchDirPlan {
        let mut plan = WatchDirPlan::default();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for project in &self.projects {
            let db_dirs: Vec<PathBuf> = project
                .db_dirs
                .iter()
                .filter(|dir| seen.insert(path_identity(dir)))
                .cloned()
                .collect();
            let problem = if project.db_dirs.is_empty() {
                Some("目录可读但下面没有 *000 库目录".to_string())
            } else {
                None
            };
            plan.projects.push(ProjectWatchDirs {
                project: project.name.clone(),
                root: Some(project.project_dir.clone()),
                db_dirs,
                problem,
            });
        }
        if self.main.is_none() {
            plan.projects.push(ProjectWatchDirs {
                project: self.project.clone(),
                root: None,
                db_dirs: Vec::new(),
                problem: Some(
                    self.problems
                        .iter()
                        .find(|problem| problem.starts_with("主项目"))
                        .cloned()
                        .unwrap_or_else(|| "主项目未定位".to_string()),
                ),
            });
        }
        for unresolved in &self.unresolved {
            plan.projects.push(ProjectWatchDirs {
                project: unresolved.code.clone(),
                root: None,
                db_dirs: Vec::new(),
                problem: Some(format!(
                    "MDB {} 声明的 {} 个库（{}）属于它，{}",
                    self.mdb,
                    unresolved.dbnums.len(),
                    summarize_dbnums(&unresolved.dbnums),
                    unresolved.reason
                )),
            });
        }
        if let Some(error) = &self.declaration_error {
            plan.projects.push(ProjectWatchDirs {
                project: format!("MDB {}", self.mdb),
                root: None,
                db_dirs: Vec::new(),
                problem: Some(format!("读不到声明，外部项目无从派生：{error}")),
            });
        }
        plan
    }

    /// 这份闭包在监控口径上有没有缺口：有搜索根不可达，或 [`Self::watch_dir_plan`] 里有
    /// problem 条目（主项目没定位到 / 声明的项目码没目录 / 项目目录下没有 `*000` / 声明读不到）。
    ///
    /// 重挂轮的健康闸用它决定要不要重扫搜索根：缺口存在 = 盘上可能有没见过面的目录
    /// （启动时掉线的盘连它的 `*000` 都列不出来），值得重解析；全健康 = 重扫不会发现
    /// 任何新东西，整轮跳过。`roots` 可达性单查是因为「根掉了但它下面没有声明项目」
    /// 折不出 problem 条目，可那根恢复后正要靠重解析发现新目录，不能漏。
    /// 纯内存判定，不碰盘。
    pub fn has_watch_gaps(&self) -> bool {
        self.roots.iter().any(|root| !root.reachable)
            || !self.watch_dir_plan().problems().is_empty()
    }

    /// 启动横幅那一句（D6）。
    pub fn banner_line(&self) -> String {
        let unreachable: Vec<String> = self
            .roots
            .iter()
            .filter(|root| !root.reachable)
            .map(|root| {
                format!(
                    "{} {}",
                    root.path.display(),
                    root.problem.as_deref().unwrap_or("不可达")
                )
            })
            .collect();
        let roots = if unreachable.is_empty() {
            format!("搜索根 {}", self.roots.len())
        } else {
            format!(
                "搜索根 {}（{} 不可达：{}）",
                self.roots.len(),
                unreachable.len(),
                unreachable.join("；")
            )
        };
        let projects = self
            .projects
            .iter()
            .map(|project| {
                let codes = project.codes.join("+");
                if project.is_main {
                    format!("{codes} 主→{}", project.name)
                } else {
                    format!("{codes}→{}@根#{}", project.name, project.root_index + 1)
                }
            })
            .collect::<Vec<_>>()
            .join(" / ");
        let external_declared: usize = self
            .projects
            .iter()
            .filter(|project| !project.is_main)
            .map(|project| project.dbnums.len())
            .sum::<usize>()
            + self
                .unresolved
                .iter()
                .map(|unresolved| unresolved.dbnums.len())
                .sum::<usize>();
        let located: usize = self
            .projects
            .iter()
            .filter(|project| !project.is_main)
            .map(|project| project.dbnums.len())
            .sum();
        let unresolved = if self.unresolved.is_empty() {
            "未定位项目 0".to_string()
        } else {
            format!(
                "未定位项目 {}：{}",
                self.unresolved.len(),
                self.unresolved
                    .iter()
                    .map(|unresolved| format!("{}({} 库)", unresolved.code, unresolved.dbnums.len()))
                    .collect::<Vec<_>>()
                    .join("、")
            )
        };
        let declaration = match &self.declaration_error {
            Some(error) => format!("；声明读不到：{error}"),
            None => String::new(),
        };
        format!(
            "MDB {}：{roots}；闭包 {} 个项目（{projects}），外部库 {located}/{external_declared} 已定位；{unresolved}{declaration}",
            self.mdb,
            self.projects.len()
        )
    }

    /// `/health.mdb_closure`。
    pub fn health_json(&self) -> serde_json::Value {
        json!({
            "status": if self.is_degraded() { "degraded" } else { "ok" },
            "project": self.project,
            "mdb": self.mdb,
            "roots": self.roots.iter().map(|root| json!({
                "path": root.path.display().to_string(),
                "reachable": root.reachable,
                "codes_seen": root.codes_seen,
                "problem": root.problem,
                "scan_ms": root.elapsed.as_millis(),
            })).collect::<Vec<_>>(),
            "projects": self.projects.iter().map(|project| json!({
                "name": project.name,
                "codes": project.codes,
                "is_main": project.is_main,
                "root_index": project.root_index,
                "project_dir": project.project_dir.display().to_string(),
                "db_dirs": project.db_dirs.iter().map(|dir| dir.display().to_string()).collect::<Vec<_>>(),
                "dbnums": project.dbnums,
                "shadowed": project.shadowed.iter().map(|dir| dir.display().to_string()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "seen_projects": self.seen_projects.iter().map(|seen| json!({
                "name": seen.name,
                "codes": seen.codes,
                "root_index": seen.root_index,
                "project_dir": seen.project_dir.display().to_string(),
            })).collect::<Vec<_>>(),
            "unresolved": self.unresolved.iter().map(|unresolved| json!({
                "code": unresolved.code,
                "dbnums": unresolved.dbnums,
                "names": unresolved.names,
                "reason": unresolved.reason,
                "hint": format!(
                    "把装着 {}000 的目录（本地或 \\\\服务器\\共享）加进 project_search_paths",
                    unresolved.code.to_ascii_lowercase()
                ),
            })).collect::<Vec<_>>(),
            "problems": self.problems,
            "declaration_error": self.declaration_error,
            "sys": self.declaration.as_ref().map(|declaration| declaration.sys.display().to_string()),
        })
    }
}

fn split_codes(joined: &str) -> Vec<String> {
    joined
        .split('+')
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(|code| code.to_ascii_uppercase())
        .collect()
}

/// `7001, 7009, 7011 … 共 34 个`——横幅里不刷屏。
pub fn summarize_dbnums(dbnums: &[u32]) -> String {
    const SHOWN: usize = 8;
    let shown = dbnums
        .iter()
        .take(SHOWN)
        .map(|dbnum| dbnum.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if dbnums.len() > SHOWN {
        format!("{shown} … 共 {} 个", dbnums.len())
    } else {
        shown
    }
}

/// 搜索根 = `[project_path] ++ project_search_paths`，整理规则见 [`collect_search_roots`]。
pub fn search_roots(db_option: &DbOption) -> Vec<PathBuf> {
    collect_search_roots(
        std::iter::once(db_option.project_path.clone())
            .chain(crate::options::project_search_paths()),
    )
}

/// 把配置 / 向导页面上写的几个根整理成扫描用的列表：逐项 [`normalize_path_input`]，空项跳过，
/// 写法不同但字面上是同一处的（斜杠方向、大小写、末尾分隔符）按 [`path_key`] 去重，
/// 先写的在前——顺序即优先级。
///
/// 这里**只看字面、不碰文件系统**。根可能是掉线的 UNC：`path_identity` 走 `canonicalize`，
/// 会在调用方线程上阻塞到 OS 自己放弃（2026-09-16 用现场服务器 `\\10.181.11.245\PDM` 实测 21 s），
/// 把 [`scan_roots`] 每根独立线程 + 15 s 的上限整个绕过——启动、重扫、向导「看闭包」每个掉线根
/// 都要多等一个 OS 超时，而且报出来的是 OS 的 error 53 而不是「扫描 15 s 未答」。
/// 代价是联接 / 符号链接与它的目标、8.3 短名这类「字面不同、实为一处」的写法不再合并：
/// 它们会被当成两个根各扫一遍，后到的那份在闭包里列为 `shadowed`，不阻断也不算冲突
/// （UNC 与本地同一目录本来 `canonicalize` 也认不成一处，一直就是这么报的）。
pub fn collect_search_roots<I>(candidates: I) -> Vec<PathBuf>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in candidates {
        let raw = raw.as_ref();
        if raw.trim().trim_matches('"').is_empty() {
            continue;
        }
        let path = normalize_path_input(raw);
        if seen.insert(path_key(&path)) {
            roots.push(path);
        }
    }
    roots
}

/// 主项目定位：先按文件夹名，找不到再把 `project_name` 当项目码（§8 Q4）。
fn locate_main(scans: &[RootScan], project_name: &str) -> Result<ProjectLocation, LocateFailure> {
    match locate_project_folder(scans, project_name) {
        Ok(location) => Ok(location),
        Err(by_name) => match locate_code(scans, project_name) {
            Ok(location) => Ok(location),
            Err(_) => Err(by_name),
        },
    }
}

/// 现算一次闭包：扫根 → 定位主项目 → 读（或取缓存的）声明 → 逐码定位。
pub fn resolve(roots: &[PathBuf], project_name: &str, mdb: &str, timeout: Duration) -> ClosurePlan {
    let scans = scan_roots(roots, timeout);
    let main = locate_main(&scans, project_name);
    let declaration = match &main {
        Ok(location) => {
            declaration(&location.db_dirs, project_name, mdb).map_err(|error| format!("{error:#}"))
        }
        Err(failure) => cached_declaration(project_name, mdb)
            .ok_or_else(|| format!("主项目未定位（{failure}），无从读取它的 SYS")),
    };
    ClosurePlan::build(project_name, mdb, scans, main, declaration)
}

type LatestPlan = Option<(Vec<String>, Arc<ClosurePlan>)>;

fn latest_slot() -> &'static RwLock<LatestPlan> {
    static LATEST: OnceLock<RwLock<LatestPlan>> = OnceLock::new();
    LATEST.get_or_init(|| RwLock::new(None))
}

/// 最近一份闭包的 key。根用字面 [`path_key`]，不用 `path_identity`：后者走 `canonicalize`，
/// 掉线的根会让每次 [`latest_for`] 都在调用方线程上等一个 OS 超时，而且同一个根在线 / 掉线
/// 两个时刻算出的值不同（见 [`super::project_paths::MountState`]），共享盘每抖一次缓存就
/// 白失效一次、多重扫一遍。
fn plan_key(roots: &[PathBuf], project_name: &str, mdb: &str) -> Vec<String> {
    let mut key: Vec<String> = roots.iter().map(|root| path_key(root)).collect();
    key.push(project_name.trim().to_ascii_lowercase());
    key.push(normalize_mdb(mdb));
    key
}

/// 按配置现算闭包并留作最近一份。
pub fn resolve_for(db_option: &DbOption) -> Arc<ClosurePlan> {
    let roots = search_roots(db_option);
    let plan = Arc::new(resolve(
        &roots,
        &db_option.project_name,
        &db_option.mdb_name,
        DEFAULT_ROOT_SCAN_TIMEOUT,
    ));
    if let Ok(mut slot) = latest_slot().write() {
        *slot = Some((
            plan_key(&roots, &db_option.project_name, &db_option.mdb_name),
            plan.clone(),
        ));
    }
    plan
}

/// 最近一份闭包（任何配置的）。`/health` 与横幅用；没算过就 `None`。
pub fn latest() -> Option<Arc<ClosurePlan>> {
    latest_slot()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(|(_, plan)| plan.clone()))
}

/// 这份配置的最近闭包：有且是同一组（搜索根、主项目、MDB）就直接用，否则现算一次。
/// 给不该自己扫盘的读者（`resolve_project_root` 这类每次操作都会问一遍的口）。
pub fn latest_for(db_option: &DbOption) -> Arc<ClosurePlan> {
    let roots = search_roots(db_option);
    let key = plan_key(&roots, &db_option.project_name, &db_option.mdb_name);
    if let Ok(slot) = latest_slot().read()
        && let Some((known, plan)) = slot.as_ref()
        && *known == key
    {
        return plan.clone();
    }
    resolve_for(db_option)
}

/// 这份配置的最近闭包，**只查缓存、不现算**：没有或 key 不符返回 `None`。
///
/// 给「要不要重新解析」的判定口用（重挂轮的健康闸）——它问的是盘上有没有可能藏着
/// 没见过面的目录，查不到缓存本身就算一种缺口，该交给 [`resolve_for`] 去现算；
/// 这里绝不能为了回答这个问题自己去碰盘（调用方是每 `AIOS_WATCH_REMOUNT_SECS`
/// 一拍的事件循环）。
pub fn cached_plan_for(db_option: &DbOption) -> Option<Arc<ClosurePlan>> {
    let roots = search_roots(db_option);
    let key = plan_key(&roots, &db_option.project_name, &db_option.mdb_name);
    latest_slot()
        .read()
        .ok()?
        .as_ref()
        .and_then(|(known, plan)| (*known == key).then(|| plan.clone()))
}

/// `included_projects` 非空时启动打的那一句（D3）：它已不再参与解析，本次由 MDB 派生了什么。
pub fn included_projects_notice(db_option: &DbOption, plan: &ClosurePlan) -> Option<String> {
    if db_option.included_projects.is_empty() {
        return None;
    }
    let derived = plan
        .projects
        .iter()
        .map(|project| {
            if project.is_main {
                format!("{}(主)", project.name)
            } else {
                format!("{}→{}", project.codes.join("+"), project.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" / ");
    Some(format!(
        "included_projects = {:?} 已不再参与项目解析（ADR-078）：项目集合由 MDB {} 派生，\
         本次得到 {derived}{}；这一项可以从 DbOption.toml 删掉，远程项目改填 project_search_paths",
        db_option.included_projects,
        plan.mdb,
        if plan.unresolved.is_empty() {
            String::new()
        } else {
            format!(
                "，未定位 {}",
                plan.unresolved
                    .iter()
                    .map(|unresolved| unresolved.code.clone())
                    .collect::<Vec<_>>()
                    .join("、")
            )
        }
    ))
}

/// 一个路径是不是像搜索根（存在且是目录）。向导探测用，不走超时线程。
pub fn looks_like_root(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "aios-closure-{name}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create fixture root");
            Self { root }
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(&path).expect("create fixture dir");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn declaration(project: &str, entries: &[(Option<&str>, u32, &str)]) -> Arc<MdbDeclaration> {
        seed_declaration(project, "ALL", entries);
        cached_declaration(project, "ALL").expect("seeded")
    }

    fn identities(dirs: &[PathBuf]) -> Vec<String> {
        let mut out: Vec<String> = dirs.iter().map(|dir| path_identity(dir)).collect();
        out.sort();
        out
    }

    /// CURD 夹具（ACP×3、SCB×1、主项目×2）→ 三个项目，dbnum 分组正确，空码归主项目。
    #[test]
    fn closure_groups_declarations_by_project_code() {
        let fixture = Fixture::new("groups");
        let ams = fixture.dir("Marine/ams000");
        let acp = fixture.dir("AvevaCatalogue/acp000");
        let scb = fixture.dir("SCB/scb000");
        let declaration = declaration(
            "closure-groups-Marine",
            &[
                (None, 8000, "/DESI"),
                (Some("ACP"), 7001, "*MASTER/STLCATA"),
                (Some("acp"), 7009, "/ACPCATA"),
                (Some("SCB"), 6003, "/SCBDESI"),
                (None, 7999, "/DESI2"),
                (Some("ACP"), 7015, "/ACPDESI"),
            ],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Marine");
        let plan = ClosurePlan::build("Marine", "ALL", scans, main, Ok(declaration));

        assert_eq!(plan.projects.len(), 3, "{:?}", plan.projects);
        let main = &plan.projects[plan.main.expect("主项目定位到")];
        assert!(main.is_main);
        assert_eq!(main.dbnums, vec![8000, 7999]);
        assert_eq!(identities(&main.db_dirs), identities(&[ams]));

        let acp_project = plan.projects.iter().find(|p| p.codes == ["ACP"]).expect("ACP");
        assert_eq!(acp_project.name, "AvevaCatalogue");
        assert_eq!(acp_project.dbnums, vec![7001, 7009, 7015]);
        assert_eq!(identities(&acp_project.db_dirs), identities(&[acp]));

        let scb_project = plan.projects.iter().find(|p| p.codes == ["SCB"]).expect("SCB");
        assert_eq!(scb_project.dbnums, vec![6003]);
        assert_eq!(identities(&scb_project.db_dirs), identities(&[scb]));
        assert!(plan.unresolved.is_empty());
        assert!(!plan.is_degraded());
        assert_eq!(plan.project_names(), vec!["Marine", "AvevaCatalogue", "SCB"]);
    }

    /// 定位器全空 → `unresolved = [{code: "ACP", dbnums: [...]}]`，原因点名找过的根，且不拒绝。
    #[test]
    fn unresolved_codes_keep_their_dbnums() {
        let fixture = Fixture::new("unresolved");
        fixture.dir("Main/mai000");
        let declaration = declaration(
            "unresolved-Main",
            &[(Some("ACP"), 7001, "/A"), (Some("ACP"), 7002, "/B"), (Some("ZDJ"), 7600, "/Z")],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Main");
        let plan = ClosurePlan::build("Main", "ALL", scans, main, Ok(declaration));

        assert_eq!(plan.projects.len(), 1);
        assert_eq!(plan.unresolved.len(), 2);
        assert_eq!(plan.unresolved[0].code, "ACP");
        assert_eq!(plan.unresolved[0].dbnums, vec![7001, 7002]);
        assert_eq!(plan.unresolved[0].names, vec!["/A", "/B"]);
        assert!(plan.unresolved[0].reason.contains("acp000"), "{}", plan.unresolved[0].reason);
        assert!(plan.unresolved[0].reason.contains(&fixture.root.display().to_string()));
        assert!(plan.is_degraded());
        let watch = plan.watch_dir_plan();
        assert_eq!(watch.problems().len(), 2, "{:?}", watch.problems());
        assert!(watch.problems()[0].contains("7001"));
        let health = plan.health_json();
        assert_eq!(health["status"], "degraded");
        assert_eq!(health["unresolved"][1]["code"], "ZDJ");
    }

    /// 主项目自己的码被显式声明、或一个目录下两个码，都并进同一条，不重复列目录。
    #[test]
    fn codes_that_land_in_a_known_project_dir_are_merged() {
        let fixture = Fixture::new("merge");
        let ams = fixture.dir("Marine/ams000");
        let amx = fixture.dir("Marine/amx000");
        let declaration = declaration(
            "merge-Marine",
            &[(None, 8000, "/D"), (Some("AMS"), 8001, "/E"), (Some("AMX"), 8002, "/F")],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Marine");
        let plan = ClosurePlan::build("Marine", "ALL", scans, main, Ok(declaration));
        assert_eq!(plan.projects.len(), 1, "{:?}", plan.projects);
        assert_eq!(plan.projects[0].dbnums, vec![8000, 8001, 8002]);
        assert_eq!(identities(&plan.projects[0].db_dirs), identities(&[ams, amx]));
        assert_eq!(plan.watch_dir_plan().dirs().len(), 2);
    }

    /// 主项目没定位到：计划里点名、`is_degraded`，缓存里有声明时外部项目照常派生。
    #[test]
    fn a_missing_main_project_is_named_and_does_not_hide_the_rest() {
        let fixture = Fixture::new("no-main");
        fixture.dir("AvevaCatalogue/acp000");
        seed_declaration("Ghost-no-main", "ALL", &[(Some("ACP"), 7001, "/A")]);
        let plan = resolve(&[fixture.root.clone()], "Ghost-no-main", "ALL", DEFAULT_ROOT_SCAN_TIMEOUT);
        assert!(plan.main.is_none());
        assert!(plan.problems.iter().any(|p| p.contains("主项目 \"Ghost-no-main\"")), "{:?}", plan.problems);
        assert_eq!(plan.projects.len(), 1);
        assert_eq!(plan.projects[0].codes, vec!["ACP"]);
        assert!(plan.is_degraded());
        let watch = plan.watch_dir_plan();
        assert!(watch.problems().iter().any(|p| p.contains("Ghost-no-main")), "{:?}", watch.problems());
    }

    /// ADR-085 D2：先读后换。SYST 落库后重读 SYS 失败（目录里压根没有 SYS）时，手里那份声明原样留着；
    /// 改成「先清后读」，这一条就红——闭包、路由目录与本期执行范围会一起退化成「SYS 未读到」。
    #[test]
    fn a_failed_declaration_reload_keeps_the_previous_declaration() {
        let project = "Reload-keeps-previous";
        seed_declaration(project, "ALL", &[(None, 8000, "/D")]);
        let fixture = Fixture::new("reload-no-sys");
        let empty = fixture.dir("Marine/ams000");

        let error = reload_declaration(&[empty], project, "ALL").expect_err("目录里没有 SYS，必须报错");
        assert!(error.to_string().contains("/ALL"), "错误要点名是哪个 MDB：{error:#}");

        let kept = cached_declaration(project, "ALL").expect("旧声明必须还在");
        assert_eq!(kept.databases.iter().map(|db| db.dbnum).collect::<Vec<_>>(), [8000]);
        forget_declaration(project, "ALL");
    }

    /// `project_name` 写成项目码也认（§8 Q4）。
    #[test]
    fn the_main_project_may_be_named_by_its_code() {
        let fixture = Fixture::new("main-by-code");
        let dir = fixture.dir("SomeFolder/qzx000");
        seed_declaration("qzx", "ALL", &[(None, 8000, "/D")]);
        let plan = resolve(&[fixture.root.clone()], "qzx", "ALL", DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = &plan.projects[plan.main.expect("按码定位到")];
        assert_eq!(identities(&main.db_dirs), identities(&[dir]));
        assert_eq!(main.name, "qzx");
        assert_eq!(main.codes, vec!["QZX"]);
    }

    /// 横幅一句与 included_projects 提示的形状。
    #[test]
    fn banner_and_notice_name_every_project_and_gap() {
        let fixture = Fixture::new("banner");
        fixture.dir("Marine/ams000");
        fixture.dir("Cat/acp000");
        let declaration = declaration(
            "banner-Marine",
            &[(None, 8000, "/D"), (Some("ACP"), 7001, "/A"), (Some("SCB"), 6003, "/S")],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Marine");
        let plan = ClosurePlan::build("Marine", "ALL", scans, main, Ok(declaration));
        let banner = plan.banner_line();
        assert!(banner.contains("闭包 2 个项目"), "{banner}");
        assert!(banner.contains("ACP→Cat@根#1"), "{banner}");
        assert!(banner.contains("外部库 1/2 已定位"), "{banner}");
        assert!(banner.contains("未定位项目 1：SCB(1 库)"), "{banner}");

        let mut option = aios_core::get_db_option().clone();
        option.included_projects = vec!["Marine".into(), "Cat".into()];
        let notice = included_projects_notice(&option, &plan).expect("非空名单要提示");
        assert!(notice.contains("ADR-078"), "{notice}");
        assert!(notice.contains("ACP→Cat"), "{notice}");
        assert!(notice.contains("未定位 SCB"), "{notice}");
        option.included_projects.clear();
        assert!(included_projects_notice(&option, &plan).is_none());
    }

    /// 搜索根按字面去重：斜杠方向、大小写、末尾分隔符、两侧引号都不算差异；空项跳过；
    /// 先写的在前、保留第一种写法。不存在的根照样收下——存在与否是扫描线程的事。
    #[cfg(windows)]
    #[test]
    fn search_roots_are_deduplicated_literally_and_in_order() {
        let roots = collect_search_roots([
            "D:/AVEVA/Projects/E3D3.1/",
            "",
            r#"  ""  "#,
            r"d:\aveva\projects\e3d3.1",
            r#""\\nas01\PDM""#,
            "//nas01/pdm/",
            r"\\10.181.11.245\PDM\PDM000",
        ]);
        assert_eq!(
            roots,
            vec![
                PathBuf::from(r"D:\AVEVA\Projects\E3D3.1"),
                PathBuf::from(r"\\nas01\PDM"),
                PathBuf::from(r"\\10.181.11.245\PDM\PDM000"),
            ]
        );
    }

    /// 最近闭包的 key 不随根的可达性变：同一个根在线时与掉线后算出的 key 必须一样，
    /// 否则共享盘每抖一次 `latest_for` 就白重扫一遍；而且算 key 不能去碰文件系统
    /// （掉线 UNC 上 `canonicalize` 会把调用方阻塞到 OS 超时）。
    #[test]
    fn plan_key_does_not_depend_on_whether_the_root_is_reachable() {
        let fixture = Fixture::new("plan-key");
        let literal = fixture.root.to_string_lossy().into_owned();
        let roots = collect_search_roots([literal.as_str()]);
        assert_eq!(roots, vec![fixture.root.clone()]);
        let online = plan_key(&roots, "Main", "ALL");

        std::fs::remove_dir_all(&fixture.root).expect("模拟根掉线");
        assert!(!fixture.root.exists());
        let offline = plan_key(&roots, "Main", "ALL");

        assert_eq!(online, offline, "根掉线不该换 key");
        assert_eq!(online.len(), 3, "根 + 主项目 + MDB");
        assert_eq!(online[1], "main");
        assert_eq!(online[2], "/ALL");
    }

    /// 监控口径的缺口判定真值表（重挂轮健康闸的判据）：干净闭包 false；根不可达、
    /// 声明的码定位不到、主项目没定位到、声明读不到，各是一格 true。
    #[test]
    fn watch_gaps_cover_every_way_a_plan_can_be_incomplete() {
        // 干净：主项目 + 外部项目都定位到，唯一的根可达。
        let fixture = Fixture::new("gaps-clean");
        fixture.dir("Marine/ams000");
        fixture.dir("Cat/acp000");
        let decl = declaration(
            "gaps-clean-Marine",
            &[(None, 8000, "/D"), (Some("ACP"), 7001, "/A")],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Marine");
        let clean = ClosurePlan::build("Marine", "ALL", scans, main, Ok(decl));
        assert!(
            !clean.has_watch_gaps(),
            "{:?}",
            clean.watch_dir_plan().problems()
        );

        // 根不可达但项目侧没有问题条目（那根下没有声明项目）也要算缺口——
        // 它恢复之后，重解析是发现它下面目录的唯一机会。
        let fixture = Fixture::new("gaps-dead-root");
        fixture.dir("Main/mai000");
        let decl = declaration("gaps-dead-root-Main", &[(None, 8000, "/D")]);
        let mut scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Main");
        let dead = fixture.root.join("dead-share"); // 不存在 → 扫描按不可达记
        scans.extend(scan_roots(&[dead], DEFAULT_ROOT_SCAN_TIMEOUT));
        let plan = ClosurePlan::build("Main", "ALL", scans, main, Ok(decl));
        assert!(
            plan.watch_dir_plan().problems().is_empty(),
            "{:?}",
            plan.watch_dir_plan().problems()
        );
        assert!(plan.has_watch_gaps(), "不可达的根单靠自己就算缺口");

        // 声明的码定位不到 → unresolved → problem 条目。
        let fixture = Fixture::new("gaps-unresolved");
        fixture.dir("Main/mai000");
        let decl = declaration(
            "gaps-unresolved-Main",
            &[(None, 8000, "/D"), (Some("ACP"), 7001, "/A")],
        );
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Main");
        let plan = ClosurePlan::build("Main", "ALL", scans, main, Ok(decl));
        assert!(!plan.unresolved.is_empty());
        assert!(plan.has_watch_gaps());

        // 主项目没定位到。
        let fixture = Fixture::new("gaps-no-main");
        fixture.dir("Main/mai000");
        let decl = declaration("gaps-no-main-Ghost", &[(None, 8000, "/D")]);
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Ghost");
        let plan = ClosurePlan::build("Ghost", "ALL", scans, main, Ok(decl));
        assert!(plan.main.is_none());
        assert!(plan.has_watch_gaps());

        // 声明读不到。
        let fixture = Fixture::new("gaps-no-decl");
        fixture.dir("Main/mai000");
        let scans = scan_roots(&[fixture.root.clone()], DEFAULT_ROOT_SCAN_TIMEOUT);
        let main = locate_project_folder(&scans, "Main");
        let plan = ClosurePlan::build("Main", "ALL", scans, main, Err("SYS 读不到".to_string()));
        assert!(plan.has_watch_gaps());
    }

    /// 重挂轮闸用的只读缓存口：查不到就是 `None`（不许为了回答而现算扫盘），
    /// `resolve_for` 写过之后同 key 命中同一份；配置换了（key 不符）照旧 `None`。
    #[test]
    fn cached_plan_for_never_computes_and_hits_after_resolve_for() {
        let fixture = Fixture::new("cached-plan");
        fixture.dir("Marine/ams000");
        let mut option = aios_core::get_db_option().clone();
        option.project_path = fixture.root.to_string_lossy().into_owned();
        option.project_name = "Marine".to_string();
        option.mdb_name = format!("CACHED-{}", std::process::id());
        option.project_dirs = None;

        // 这个 key 没人算过：None——它不会去扫 fixture.root（有别的测试写过 latest
        // 也一样，key 不符就是 None）。
        assert!(cached_plan_for(&option).is_none());

        let plan = resolve_for(&option);
        let cached = cached_plan_for(&option).expect("resolve_for 之后必须命中");
        assert!(Arc::ptr_eq(&plan, &cached), "命中的必须就是写进去的那份");

        option.project_name = "Other".to_string();
        assert!(cached_plan_for(&option).is_none(), "key 不符不命中、也不现算");
    }
}
