//! 项目目录与监控目录（watch dirs）的唯一解析口径。
//!
//! 一个「项目」可以落在两类位置上，而且常常混用——设计库在本机盘、目录库在共享盘：
//!
//! - 本地路径：`project_path = "D:/AVEVA/Projects/E3D3.1"` + 项目名；
//! - 共享目录：UNC（`\\host\share\...` 或 `//host/share/...`）、已映射的网络盘，
//!   既可以整个 `project_path` 指过去，也可以只让某个项目在 `project_dirs` 里
//!   单独写成绝对路径。
//!
//! 这里取代 `aios_core::file_helper::collect_db_dirs`。那个实现把整批项目
//! `collect::<io::Result<Vec<_>>>()`，于是**任何一个**项目 `read_dir` 失败
//! （共享盘掉线、路径写错、没权限）都会让整批结果变成 `Err`，而唯一的调用点
//! 又是 `.unwrap_or_default()`——错误被吞成空列表，看门狗「起来了但一个目录都
//! 没监听」，现场只剩下一句「没有任何监控目录挂载成功」，看不出是哪个项目、
//! 为什么失败。共享盘场景下这是必然会踩到的形状，所以解析必须逐项目容错，并把
//! 每个项目的结论（用了哪个根、找到几个库目录、失败原因）原样报出来。

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use aios_core::options::DbOption;

/// 库目录的命名约定：项目目录下以 `000` 结尾的那个（`ams000` / `acp000` / `ZDJ000`）。
const DB_DIR_SUFFIX: &str = "000";

/// 把配置里写的路径归一成本平台能用的形式。
///
/// Windows 下统一成反斜杠：`//host/share` 这种正斜杠 UNC 写法在 TOML 里最常见
/// （反斜杠要写成 `\\\\host\\share` 才不被转义吃掉），但混着 `/` 的 UNC 路径在
/// 部分 Win32 调用上会退化成「找不到网络路径」。顺手剥掉两侧引号——从资源管理器
/// 「复制为路径」粘过来的值自带引号。
pub fn normalize_path_input(raw: &str) -> PathBuf {
    let trimmed = raw.trim().trim_matches('"');
    #[cfg(windows)]
    let normalized = trimmed.replace('/', "\\");
    #[cfg(not(windows))]
    let normalized = trimmed.to_string();
    PathBuf::from(normalized)
}

/// 这个配置值是不是一个「自带完整位置」的路径（绝对路径 / UNC），而不是要拼到
/// `project_path` 底下的相对目录名。
pub fn is_absolute_input(raw: &str) -> bool {
    let trimmed = raw.trim().trim_matches('"');
    // `Path::is_absolute` 认不出只写到主机名的 `\\host`（缺 share 段时没有 root），
    // 而那种写法本来就该按「网络路径写错了」报出来，不能当成相对目录名去拼。
    trimmed.starts_with("\\\\")
        || trimmed.starts_with("//")
        || normalize_path_input(trimmed).is_absolute()
}

/// 解析某个项目的根目录，与 [`DbOption::get_project_path`] 同语义，但多认两件事：
///
/// 1. `project_dirs` 里的条目可以直接写绝对路径 / UNC，从而实现「本地项目 + 共享
///    盘项目」混排——`get_project_path` 只会把它当目录名拼到 `project_path` 后面，
///    拼出 `D:/AVEVA/Projects/E3D3.1\\host\share\AMS` 这种必然不存在的路径；
/// 2. `project_dirs` 比 `included_projects` 短时返回 `None` 而不是下标越界 panic。
pub fn resolve_project_root(db_option: &DbOption, project: &str) -> Option<PathBuf> {
    if is_absolute_input(project) {
        return Some(normalize_path_input(project));
    }
    let base = normalize_path_input(&db_option.project_path);
    let Some(overrides) = db_option.project_dirs.as_ref() else {
        return Some(base.join(normalize_path_input(project)));
    };
    let index = db_option
        .included_projects
        .iter()
        .position(|name| name.trim().eq_ignore_ascii_case(project.trim()))?;
    let entry = overrides.get(index)?;
    Some(join_project_entry(&base, entry))
}

fn join_project_entry(base: &Path, entry: &str) -> PathBuf {
    if is_absolute_input(entry) {
        normalize_path_input(entry)
    } else {
        base.join(normalize_path_input(entry))
    }
}

fn ends_with_db_suffix(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().ends_with(DB_DIR_SUFFIX))
        .unwrap_or(false)
}

/// 列出这个项目根下所有的 `*000` 库目录。
///
/// 根目录本身就是库目录时直接认它：共享盘上把 `\\host\share\ams000` 单独共享出来
/// 是常见做法，此时再往下找一层只会一无所获。
///
/// 单个条目读不动（网络抖动、权限）只跳过该条目并告警，不牵连整个项目；只有
/// `read_dir` 本身失败才算这个项目解析失败。找到多个库目录时全部返回——历史实现
/// 只取第一个，剩下的库既监听不到也摄入不了，数据落库之后此后永不更新。
pub fn collect_db_dirs_in(root: &Path) -> io::Result<Vec<PathBuf>> {
    if ends_with_db_suffix(root) && root.is_dir() {
        return Ok(vec![root.to_path_buf()]);
    }

    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("跳过读不动的目录项（{}）: {error}", root.display());
                continue;
            }
        };
        let path = entry.path();
        if !ends_with_db_suffix(&path) {
            continue;
        }
        let is_dir = entry
            .file_type()
            .map(|kind| kind.is_dir())
            .unwrap_or_else(|_| path.is_dir());
        if is_dir {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// 单个项目的解析结论。
#[derive(Debug, Clone)]
pub struct ProjectWatchDirs {
    pub project: String,
    pub root: Option<PathBuf>,
    pub db_dirs: Vec<PathBuf>,
    /// 一个库目录都没解析出来时的原因，可直接展示给人看。
    pub problem: Option<String>,
}

/// 全部项目的解析结论。空列表本身就是一种故障，所以连「为什么空」一起带出来。
#[derive(Debug, Clone, Default)]
pub struct WatchDirPlan {
    pub projects: Vec<ProjectWatchDirs>,
}

impl WatchDirPlan {
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.projects
            .iter()
            .flat_map(|project| project.db_dirs.iter().cloned())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.projects.iter().all(|project| project.db_dirs.is_empty())
    }

    /// 逐项目的失败原因，一行一条。
    pub fn problems(&self) -> Vec<String> {
        self.projects
            .iter()
            .filter_map(|project| {
                let problem = project.problem.as_ref()?;
                let root = project
                    .root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| "<未解析出目录>".to_string());
                Some(format!("{}（{root}）: {problem}", project.project))
            })
            .collect()
    }

    /// 启动时打给人看的一段说明：每个项目用了哪个根、找到哪些库目录、失败在哪。
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        for project in &self.projects {
            let root = project
                .root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "<未解析出目录>".to_string());
            match &project.problem {
                Some(problem) => lines.push(format!("  - {} -> {root} [跳过] {problem}", project.project)),
                None => lines.push(format!(
                    "  - {} -> {root} [{} 个库目录] {}",
                    project.project,
                    project.db_dirs.len(),
                    project
                        .db_dirs
                        .iter()
                        .map(|dir| dir.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        lines.join("\n")
    }
}

/// 按配置解析出全部监控目录。
///
/// 逐项目独立解析：一个共享盘掉线只让它自己缺席，其余项目照常监听。
pub fn plan_watch_dirs(db_option: &DbOption) -> WatchDirPlan {
    let names: Vec<String> = if !db_option.included_projects.is_empty() {
        db_option.included_projects.clone()
    } else {
        db_option.project_dirs.clone().unwrap_or_default()
    };

    let mut plan = WatchDirPlan::default();
    // 两个项目指到同一个目录时只监听一次：`duplicate_dbnums_across_watch_dirs`
    // 会把重复目录里的同一个库看成「同 dbnum 两个文件」而整库阻断。
    let mut seen: HashSet<String> = HashSet::new();

    for name in names {
        let Some(root) = resolve_project_root(db_option, &name) else {
            plan.projects.push(ProjectWatchDirs {
                project: name,
                root: None,
                db_dirs: Vec::new(),
                problem: Some(
                    "project_dirs 与 included_projects 对不上（数量或名字），解析不出项目目录"
                        .to_string(),
                ),
            });
            continue;
        };

        let (db_dirs, problem) = match collect_db_dirs_in(&root) {
            Ok(dirs) if dirs.is_empty() => (
                Vec::new(),
                Some(format!("目录可读但下面没有 *{DB_DIR_SUFFIX} 库目录")),
            ),
            Ok(dirs) => (dirs, None),
            Err(error) => (
                Vec::new(),
                Some(format!("目录不可读（共享盘掉线 / 路径写错 / 无权限）: {error}")),
            ),
        };

        let db_dirs: Vec<PathBuf> = db_dirs
            .into_iter()
            .filter(|dir| seen.insert(path_identity(dir)))
            .collect();

        plan.projects.push(ProjectWatchDirs {
            project: name,
            root: Some(root),
            db_dirs,
            problem,
        });
    }

    plan
}

#[cfg(windows)]
const SEP: char = '\\';
#[cfg(not(windows))]
const SEP: char = '/';

/// 去重用的 key：先尽量落到真实路径上（吃掉 `..`、8.3 短名、大小写差异），
/// 拿不到（目录当下不可达）就退回字面量。
///
/// 同一个目录被列两次的代价不是「多轮询一遍」：`duplicate_dbnums_across_watch_dirs`
/// 会在两份列举里看到同一个库的两个副本，判成「同 dbnum 多文件」而**整库阻断**。
pub fn path_identity(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path_key(&resolved)
}

/// 路径比较用的归一 key：Windows 下大小写与分隔符都不该影响「是不是同一个目录」。
fn path_key(path: &Path) -> String {
    let mut text = path.to_string_lossy().to_string();
    if cfg!(windows) {
        text = text.replace('/', "\\").to_ascii_lowercase();
    }
    let trimmed = text.trim_end_matches(SEP);
    // 根本身（`/`、`C:\`）修完会变空，那时保留原值。
    if trimmed.is_empty() {
        text
    } else {
        trimmed.to_string()
    }
}

/// 已经挂到文件监控器上的目录集合。
///
/// 重复 `watch()` 同一个路径会让 PollWatcher 对它列两遍目录，F6 于是看到同一个库的
/// 两份候选、判成同 dbnum 重复而**整库阻断**——所以「挂过没有」必须记账，且要按
/// [`path_identity`] 记（`D:\a\ams000` 与 `\\host\D$\a\ams000` 写法不同、
/// 大小写不同都可能指向同一处）。
#[derive(Debug, Default)]
pub struct MountState {
    /// 字面路径 key → 挂载那一刻算出的 [`path_identity`]。
    ///
    /// 主键必须是**字面路径**而不是 identity：identity 走 `canonicalize`，目录一掉线
    /// 就解析不出来、退化成字面量，于是同一个目录在「在线」与「掉线」两个时刻算出
    /// 两个不同的 key。用 identity 当主键的话，掉线目录既查不出「已挂载」（重挂轮
    /// 看不见它失联），恢复后又会被当成新目录**再 watch 一次**——PollWatcher 把它列
    /// 两遍，F6 立刻把里面每个库判成同号重复而整库阻断。
    mounted: HashMap<String, String>,
    /// 已挂目录的 identity 集合，只用来挡「同一个物理目录的两种写法」。
    identities: HashSet<String>,
    /// 曾经挂上、但目录当下不可达的那些。留着原始路径是为了恢复后能 unwatch + 重挂。
    lost: Vec<PathBuf>,
}

impl MountState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.mounted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mounted.is_empty()
    }

    pub fn contains(&self, dir: &Path) -> bool {
        self.mounted.contains_key(&path_key(dir)) || self.identities.contains(&path_identity(dir))
    }

    /// 挂载还没挂上的目录，返回逐目录的失败原因（已挂上的静默跳过）。
    pub fn mount<W: notify::Watcher>(&mut self, watcher: &mut W, dirs: &[PathBuf]) -> Vec<String> {
        let mut failures = Vec::new();
        for dir in dirs {
            if self.contains(dir) {
                continue;
            }
            match watcher.watch(dir.as_path(), notify::RecursiveMode::NonRecursive) {
                Ok(()) => {
                    let identity = path_identity(dir);
                    self.mounted.insert(path_key(dir), identity.clone());
                    self.identities.insert(identity);
                }
                Err(error) => {
                    log::error!("文件监控设置失败，跳过该目录 {dir:?}: {error:?}");
                    eprintln!("文件监控设置失败，跳过该目录 {dir:?}: {error:?}");
                    failures.push(format!("{} -> {error}", dir.display()));
                }
            }
        }
        failures
    }

    /// `dirs` 里还没挂上的那些。
    pub fn missing(&self, dirs: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
        dirs.into_iter()
            .filter(|dir| !self.contains(dir))
            .collect()
    }

    /// 复查已挂目录还在不在，把不可达的降级为「失联」并从已挂集合里摘掉。
    ///
    /// 「挂上过」不等于「还在被监听」：共享盘中途掉线时那条记录还在，
    /// [`Self::missing`] 于是认为它不缺席，重挂轮永远不会回头看它——PollWatcher 那边
    /// 也不保证 root 消失又回来之后还能补发 Create 事件。返回本轮新失联的目录。
    pub fn refresh_health(&mut self, dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut newly_lost = Vec::new();
        for dir in dirs {
            let key = path_key(dir);
            if !self.mounted.contains_key(&key) || dir.is_dir() {
                continue;
            }
            if let Some(identity) = self.mounted.remove(&key) {
                self.identities.remove(&identity);
            }
            self.lost.push(dir.clone());
            newly_lost.push(dir.clone());
        }
        newly_lost
    }

    /// 失联目录恢复之前必须先 `unwatch`，再走正常的 [`Self::mount`]。
    ///
    /// 不 unwatch 直接重 watch 的话，PollWatcher 会对同一个目录列两遍，F6 立刻把里面
    /// 每个库都看成「同 dbnum 两个文件」而整库阻断——比漏监听更糟。
    pub fn unwatch_lost<W: notify::Watcher>(&mut self, watcher: &mut W) -> usize {
        let mut released = 0usize;
        self.lost.retain(|dir| {
            if !dir.is_dir() {
                return true;
            }
            match watcher.unwatch(dir.as_path()) {
                Ok(()) => released += 1,
                Err(error) => {
                    // 已经不在监听列表里也算达成目的，继续走重挂。
                    log::debug!("解除监听 {dir:?} 未成功（可能本就未挂载）: {error}");
                }
            }
            false
        });
        released
    }
}

/// 监控目录 → 它属于哪个项目。
///
/// 「文件归属由路径决定，数据库命名空间由配置决定」——两者过去共用一个 `project`
/// 字段，于是 `acp000\acp7006_0001` 被记成配置里的主项目 `AvevaMarineSample`，
/// 执行侧再按这个名字去 `ams000` 找它，必然找不到，整个批次 failed 且每轮重来。
/// 归属只能来自「文件落在哪个监控目录下」，而监控目录的项目归属只有
/// [`plan_watch_dirs`] 知道，所以在解析的同时把这份映射记下来。
static WATCH_DIR_OWNERS: OnceLock<RwLock<Vec<(PathBuf, String)>>> = OnceLock::new();

fn owners_registry() -> &'static RwLock<Vec<(PathBuf, String)>> {
    WATCH_DIR_OWNERS.get_or_init(|| RwLock::new(Vec::new()))
}

/// 把一次解析结果里的「目录 → 项目」登记下来。
///
/// 合并而不是覆盖：重挂轮拿到的 plan 里，掉线项目的目录会整批缺席，直接替换会把
/// 它们的归属抹掉，而队列里可能还压着它们的批次。
pub fn record_watch_dir_owners(plan: &WatchDirPlan) {
    let Ok(mut owners) = owners_registry().write() else {
        return;
    };
    for project in &plan.projects {
        for dir in &project.db_dirs {
            let key = path_key(dir);
            match owners.iter_mut().find(|(known, _)| path_key(known) == key) {
                Some((_, owner)) => *owner = project.project.clone(),
                None => owners.push((dir.clone(), project.project.clone())),
            }
        }
    }
    // 最长前缀优先：库目录本身又被单独配成一个项目根时，取更具体的那条。
    owners.sort_by_key(|(dir, _)| std::cmp::Reverse(path_key(dir).len()));
}

/// 这个文件（或目录）落在哪个项目下。
pub fn project_of_path(file: &Path) -> Option<String> {
    let owners = owners_registry().read().ok()?;
    owners
        .iter()
        .find(|(dir, _)| path_starts_with(file, dir))
        .map(|(_, project)| project.clone())
}

/// 归属解析失败、退回主项目名时的**可见**告警（按所在目录去重，每目录只喊一次）。
///
/// 退化不是可以静默接受的默认值：归属记错会让执行侧去错误的项目目录里找文件，
/// F6 的判重键也随之退化成「主项目 + dbnum」——acp/ZDJ 与 ams 各自的 sys 库
/// （共用 dbnum=8191）重新互相误判成重复而整库阻断，回到修这批代码之前的老病。
/// 它发生在逐文件的热路径上，直接每次 eprintln 会刷屏，所以按父目录去重；
/// 但必须上 stderr 而不能只有 log::warn——现场的日志面是 stdout/stderr，
/// warn 级别在那里根本看不见，退化就成了静默的。
pub fn warn_unattributed_once(path: &Path, fallback: &str) {
    static WARNED_DIRS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let dir_key = path_key(path.parent().unwrap_or(path));
    let warned = WARNED_DIRS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut warned) = warned.lock() else {
        return;
    };
    if !warned.insert(dir_key) {
        return;
    }
    let msg = format!(
        "无法判定 {} 的归属项目（所在目录不在任何监控目录登记里），退回主项目 {fallback}；\
         该目录下的库会按主项目寻址，F6 判重键同时退化——若这里存在跨项目同号（如 sys 库 8191）\
         会被误判为重复而整库阻断。检查 record_watch_dir_owners 是否覆盖了这个入口（同目录只报一次）",
        path.display()
    );
    log::warn!("{msg}");
    eprintln!("{msg}");
}

/// 启动之后才解析出来的监控目录。
///
/// 共享盘在启动那一刻不可达时，`plan_watch_dirs` 连它的 `*000` 目录都列不出来——
/// 这类目录不在 `PdmsWatcher::watch_dirs` 里，而那个字段在 `Arc<AiosDBManager>`
/// 背后改不动。看门狗的重挂轮把新解析出来的目录登记到这里，摄入侧（启动重扫、
/// 重复 dbnum 复查、手动候选扫描）读的是「启动列表 ∪ 这里」，两条路径才不会
/// 分家：只被轮询、不被摄入的目录等于没监听。
static DISCOVERED_WATCH_DIRS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();

fn discovered_registry() -> &'static RwLock<Vec<PathBuf>> {
    DISCOVERED_WATCH_DIRS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn discovered_watch_dirs() -> Vec<PathBuf> {
    discovered_registry()
        .read()
        .map(|dirs| dirs.clone())
        .unwrap_or_default()
}

/// 登记新解析出来的目录，返回其中此前没见过的那些。
pub fn record_discovered_watch_dirs(
    dirs: impl IntoIterator<Item = PathBuf>,
    known: &HashSet<String>,
) -> Vec<PathBuf> {
    let Ok(mut registry) = discovered_registry().write() else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = registry.iter().map(|dir| path_identity(dir)).collect();
    seen.extend(known.iter().cloned());
    let mut added = Vec::new();
    for dir in dirs {
        if seen.insert(path_identity(&dir)) {
            registry.push(dir.clone());
            added.push(dir);
        }
    }
    added
}

/// `path` 是否落在 `prefix` 下（含相等）。
///
/// 不能直接用 `Path::starts_with`：它逐段做**区分大小写**的比较，而 Windows 上
/// 同一个目录写成 `D:/AVEVA/...ZDJ` 与 `d:\aveva\...zdj` 是同一个地方。判错的
/// 后果不是少打一行日志——`ingestible_dirs` 会返回空，手动扫描于是「一个候选都
/// 没有」，而自动侧明明在监听同一个目录。
pub fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    let child = path_key(path);
    let parent = path_key(prefix);
    if parent.is_empty() {
        return true;
    }
    if child == parent {
        return true;
    }
    child.starts_with(&format!("{parent}{SEP}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 每个用例一个独立的临时项目根，`Drop` 时清掉。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "aios-watchdirs-{name}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create fixture root");
            Self { root }
        }

        /// 造一个目录（相对 fixture 根），返回它的绝对路径。
        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(&path).expect("create fixture dir");
            path
        }

        /// 以这个 fixture 为 `project_path` 的配置。
        fn options(&self, projects: &[&str], overrides: Option<&[&str]>) -> DbOption {
            let mut option = aios_core::get_db_option().clone();
            option.project_path = self.root.to_string_lossy().into_owned();
            option.included_projects = projects.iter().map(|name| name.to_string()).collect();
            option.project_dirs = overrides
                .map(|dirs| dirs.iter().map(|dir| dir.to_string()).collect::<Vec<_>>());
            option
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn identities(dirs: &[PathBuf]) -> HashSet<String> {
        dirs.iter().map(|dir| path_identity(dir)).collect()
    }

    /// 这是本次故障的形状：一个项目的目录读不动，其余项目必须照常解析出来。
    ///
    /// 旧口径（`aios_core::file_helper::collect_db_dirs`）把整批 collect 成一个
    /// `io::Result`，坏掉的那个让整批变 `Err`，调用点再 `unwrap_or_default()`
    /// 吞成空列表——于是「三个本地库目录都在，看门狗却一个都没监听」。
    #[test]
    fn one_unreadable_project_does_not_erase_the_others() {
        let fixture = Fixture::new("offline-share");
        let good = fixture.dir("Good/good000");
        let also_good = fixture.dir("AlsoGood/alsogood000");
        // Offline 这个项目根压根不存在，等价于共享盘掉线。
        let option = fixture.options(&["Good", "Offline", "AlsoGood"], None);

        let plan = plan_watch_dirs(&option);

        assert_eq!(
            identities(&plan.dirs()),
            identities(&[good, also_good]),
            "掉线项目不能带走其余项目的监控目录"
        );
        let problems = plan.problems();
        assert_eq!(problems.len(), 1, "只该有掉线那一个项目报错: {problems:?}");
        assert!(
            problems[0].contains("Offline"),
            "失败原因必须点名是哪个项目: {problems:?}"
        );
    }

    /// `project_dirs` 里的绝对路径 / UNC 要按原样用，而不是拼到 `project_path` 后面。
    ///
    /// 拼接的后果是 `D:/proj` + `\\nas\e3d\ZDJ` = 一个必然不存在的路径，
    /// 于是共享盘上的项目永远解析不出来。
    #[test]
    fn absolute_and_unc_entries_are_used_as_is() {
        let fixture = Fixture::new("absolute-entries");
        let elsewhere = Fixture::new("absolute-elsewhere");
        let remote = elsewhere.dir("Remote/remote000");
        fixture.dir("Local/local000");

        let elsewhere_project = elsewhere.root.join("Remote");
        let option = fixture.options(
            &["Local", "Remote"],
            Some(&["Local", &elsewhere_project.to_string_lossy()]),
        );

        let plan = plan_watch_dirs(&option);

        assert!(
            identities(&plan.dirs()).contains(&path_identity(&remote)),
            "写成绝对路径的项目应当落在它自己的位置上: {:?}",
            plan.dirs()
        );
        assert!(plan.problems().is_empty(), "{:?}", plan.problems());
    }

    /// UNC 的两种写法（`//host/share` 与 `\\host\share`）必须归一到同一个路径，
    /// 且都要被认成绝对路径而不是相对目录名。
    #[test]
    fn unc_inputs_are_normalized_and_absolute() {
        assert!(is_absolute_input(r"\\nas01\e3d\Projects"));
        assert!(is_absolute_input("//nas01/e3d/Projects"));
        assert!(is_absolute_input(r#"  "\\nas01\e3d\Projects"  "#));
        assert!(!is_absolute_input("AvevaMarineSample"));

        if cfg!(windows) {
            assert_eq!(
                normalize_path_input("//nas01/e3d/Projects"),
                normalize_path_input(r"\\nas01\e3d\Projects"),
            );
        }
    }

    /// `project_dirs` 比 `included_projects` 短时返回 `None`，而不是像
    /// `DbOption::get_project_path` 那样下标越界 panic。
    #[test]
    fn short_project_dirs_yields_none_instead_of_panicking() {
        let fixture = Fixture::new("short-overrides");
        let option = fixture.options(&["A", "B"], Some(&["A"]));

        assert!(resolve_project_root(&option, "B").is_none());
        let plan = plan_watch_dirs(&option);
        assert!(
            plan.problems().iter().any(|problem| problem.contains('B')),
            "对不上的项目要报出来: {:?}",
            plan.problems()
        );
    }

    /// 一个项目下有多个 `*000` 时全部收下。
    ///
    /// 旧实现 `.next()` 只取第一个，剩下的既监听不到也摄入不了——数据落库之后
    /// 此后永不更新，面板上却一直显示得很新鲜。
    #[test]
    fn every_db_dir_under_a_project_is_collected() {
        let fixture = Fixture::new("multi-db-dirs");
        let first = fixture.dir("Proj/aaa000");
        let second = fixture.dir("Proj/bbb000");
        fixture.dir("Proj/notadb");
        let option = fixture.options(&["Proj"], None);

        assert_eq!(
            identities(&plan_watch_dirs(&option).dirs()),
            identities(&[first, second])
        );
    }

    /// 共享盘上常把库目录本身单独共享出来（`\\host\share\ams000`），此时项目根
    /// 就是库目录，不该再往下找一层。
    #[test]
    fn a_root_that_is_itself_a_db_dir_is_used_directly() {
        let fixture = Fixture::new("root-is-db-dir");
        let db_dir = fixture.dir("Shared/ams000");
        let option = fixture.options(&["Shared"], Some(&[&db_dir.to_string_lossy()]));

        assert_eq!(
            identities(&plan_watch_dirs(&option).dirs()),
            identities(&[db_dir])
        );
    }

    /// 同一个目录被两个项目指到时只监听一次。
    ///
    /// 列两次不是「多轮询一遍」：`duplicate_dbnums_across_watch_dirs` 会在两份
    /// 列举里看到同一个库的两份候选，判成同 dbnum 重复而**整库阻断**。
    #[test]
    fn a_directory_listed_twice_is_watched_once() {
        let fixture = Fixture::new("duplicate-dir");
        let db_dir = fixture.dir("Proj/proj000");
        let option = fixture.options(
            &["Proj", "SameThing"],
            Some(&["Proj", &db_dir.to_string_lossy()]),
        );

        assert_eq!(plan_watch_dirs(&option).dirs().len(), 1);
    }

    /// 项目可读但没有库目录，与「目录不可读」是两回事，都要报且都不能阻断其他项目。
    #[test]
    fn a_project_without_db_dirs_is_reported_not_silent() {
        let fixture = Fixture::new("no-db-dir");
        fixture.dir("Empty/nothing-here");
        let option = fixture.options(&["Empty"], None);

        let plan = plan_watch_dirs(&option);
        assert!(plan.is_empty());
        assert!(
            plan.problems()
                .iter()
                .any(|problem| problem.contains("没有 *000")),
            "{:?}",
            plan.problems()
        );
    }

    /// `ingestible_dirs` 用它把监控目录还原成「本项目的那几个」。Windows 上大小写
    /// 与分隔符写法不同仍是同一个目录，判错会让手动侧扫出 0 个候选，而自动侧明明
    /// 在监听同一个地方——两条路径就此分家。
    #[test]
    fn prefix_match_ignores_case_and_separator_style() {
        let child = Path::new(r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000");
        assert!(path_starts_with(child, Path::new(r"D:\AVEVA\Projects\E3D3.1\ZDJ")));
        assert!(path_starts_with(child, child));
        assert!(!path_starts_with(
            child,
            Path::new(r"D:\AVEVA\Projects\E3D3.1\ZD")
        ));

        if cfg!(windows) {
            assert!(path_starts_with(
                child,
                Path::new("d:/aveva/projects/e3d3.1/zdj")
            ));
        }
    }

    /// 文件的归属项目来自它所在的监控目录，而不是配置里的主项目名。
    ///
    /// 记错的代价是实测过的：`acp000\acp7006_0001` 被记成主项目 `AvevaMarineSample`，
    /// 执行侧就拿这个名字去 `ams000` 里找它，必然找不到，批次每轮 failed 一次。
    #[test]
    fn a_file_is_attributed_to_the_project_of_its_watch_dir() {
        let fixture = Fixture::new("owner-attribution");
        fixture.dir("Marine/ams000");
        fixture.dir("Catalogue/acp000");
        let option = fixture.options(&["Marine", "Catalogue"], None);

        record_watch_dir_owners(&plan_watch_dirs(&option));

        assert_eq!(
            project_of_path(&fixture.root.join("Catalogue/acp000/acp7006_0001")).as_deref(),
            Some("Catalogue")
        );
        assert_eq!(
            project_of_path(&fixture.root.join("Marine/ams000/ams8000_0001")).as_deref(),
            Some("Marine")
        );
        assert_eq!(
            project_of_path(Path::new("D:/not/a/watched/place/x")),
            None,
            "监控目录之外的路径判不出归属，由调用方决定怎么兜底"
        );
    }

    /// 已挂目录中途掉线：必须降级成「失联」，否则 `missing()` 一直认为它在被监听，
    /// 重挂轮永远不会回头看它。恢复时先 unwatch 再挂，避免 PollWatcher 列两遍
    /// 同一个目录（那会让 F6 把里面每个库都判成同号重复而整库阻断）。
    #[test]
    fn a_dropped_directory_is_marked_lost_then_remounted_exactly_once() {
        use notify::{Config, PollWatcher};

        let fixture = Fixture::new("mount-health");
        let dir = fixture.dir("Proj/proj000");
        let option = fixture.options(&["Proj"], None);

        let (tx, _rx) = std::sync::mpsc::channel();
        let mut watcher = PollWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default().with_poll_interval(std::time::Duration::from_secs(30)),
        )
        .expect("create poll watcher");
        let mut mounted = MountState::new();

        mounted.mount(&mut watcher, &plan_watch_dirs(&option).dirs());
        assert_eq!(mounted.len(), 1);

        // 共享盘掉线。
        std::fs::remove_dir_all(&dir).expect("drop the watched directory");
        let lost = mounted.refresh_health(&[dir.clone()]);
        assert_eq!(identities(&lost), identities(&[dir.clone()]));
        assert_eq!(mounted.len(), 0, "失联目录必须从已挂集合里摘掉");

        // 还没恢复时不该抢着 unwatch。
        assert_eq!(mounted.unwatch_lost(&mut watcher), 0);

        // 恢复。
        std::fs::create_dir_all(&dir).expect("bring the directory back");
        assert_eq!(mounted.unwatch_lost(&mut watcher), 1);
        mounted.mount(&mut watcher, &mounted.missing(plan_watch_dirs(&option).dirs()));
        assert_eq!(mounted.len(), 1, "恢复后只能有一份挂载");
        assert!(mounted.missing(plan_watch_dirs(&option).dirs()).is_empty());
    }

    /// 重挂轮的两步：共享盘恢复后**重新解析**（启动时列不出来的 `*000` 此刻才有），
    /// 以及只补挂缺席的那些（重复 `watch()` 同一个目录会把库判成同号重复）。
    #[test]
    fn a_share_that_comes_back_is_replanned_and_mounted_once() {
        use notify::{Config, PollWatcher};

        let fixture = Fixture::new("remount");
        fixture.dir("Early/early000");
        let option = fixture.options(&["Early", "Late"], None);

        let (tx, _rx) = std::sync::mpsc::channel();
        let mut watcher = PollWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default().with_poll_interval(std::time::Duration::from_secs(30)),
        )
        .expect("create poll watcher");
        let mut mounted = MountState::new();

        // 启动：Late 还不在线。
        mounted.mount(&mut watcher, &plan_watch_dirs(&option).dirs());
        assert_eq!(mounted.len(), 1);

        // 共享盘恢复。
        let late = fixture.dir("Late/late000");

        let missing = mounted.missing(plan_watch_dirs(&option).dirs());
        assert_eq!(
            identities(&missing),
            identities(&[late]),
            "只有新出现的那个目录算缺席"
        );
        mounted.mount(&mut watcher, &missing);
        assert_eq!(mounted.len(), 2);

        // 空转轮：什么都没变就不该再挂一次。
        assert!(
            mounted
                .missing(plan_watch_dirs(&option).dirs())
                .is_empty(),
            "已挂上的目录不能被重复挂载"
        );
    }
}
