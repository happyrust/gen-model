//! 工具和项目默认值。账号密码不进这里。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::data_interface::project_paths::{normalize_path_input, path_key};
use crate::options::RvmCheckSection;

#[derive(Clone, Debug)]
pub struct ResolvedTools {
    pub launcher: PathBuf,
    pub work_dir: PathBuf,
    pub alive_timeout: Duration,
    /// E3D 导出子进程的超时，也是进程内生成的叫停期限。
    pub timeout: Duration,
    pub launcher_found: bool,
}

#[derive(Clone, Debug)]
pub struct ProjectDefaults {
    pub projects_dir: PathBuf,
    pub project_name: String,
    pub project_dir: PathBuf,
    /// `<code>000` 目录名去掉 `000` 后的大写项目码，例如 `AMS`。多个时取目录扫描的第一个。
    pub e3d_project: Option<String>,
    pub e3d_projects: Vec<String>,
    pub mdb: String,
    /// gen-model 自己的项目编号（`DbOption.project_code`），不是 E3D 的三位项目码。
    pub numeric_project_code: String,
}

pub struct ToolSearch {
    pub configured: RvmCheckSection,
    pub exe_dir: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
}

/// 显式给的项目父目录（命令行 `--projects-dir` / 页面「项目父目录」）与配置的 `project_path` 不是同一个目录时的
/// 那一笔（GitHub #145）。
///
/// 两边管的不是一回事，一次核对里各管一段：
/// - **配置的搜索根**（`project_path` 在前，后面是 `project_search_paths`）：枚举根（WORLD 下的 SITE / APPLDW）、
///   模型树、根所在的设计库文件。这一段**不看**显式给的目录——模型树由 `DirectTreeService::open(项目名, MDB)` 开，
///   往下读的是全局配置。
/// - **显式给的目录**：预检的路径检查、项目的 evars 文件、E3D 导出基准的会话、生成时按库号找被引用的库
///   （`<项目>/<码>000`，目录库等）。
///
/// 两处指向同一份数据时没有问题。指向内容不同的两份时（比如一份是较早的项目副本），根的设计库来自一份、基准和
/// 被引用的库来自另一份，比出来的差异照常判档、照常入库，与真的模型差异分不开（2026-11-09 踩过一次：一个根多出
/// 1 件 0.384 mm 的差异，先被当成模型回归查）。所以不一致时要出声、要留痕；不拒绝运行——同一份数据有两种合法写法的
/// 部署（一边网络路径、一边本地路径）字面上也不一样。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectDirMismatch {
    /// 显式给的目录（修掉首尾空白与引号、斜杠方向，别的原样）。
    pub projects_dir: String,
    /// 配置的搜索根，按优先级：`project_path` 在前，后面是 `project_search_paths`。
    pub configured_roots: Vec<String>,
}

impl ProjectDirMismatch {
    /// 显式给的目录字面上是不是配置里排在后面的搜索根之一。是的话，项目若正是在那个根下找到的，两边读的其实是
    /// 同一份——这里不碰文件系统，判不了，只在告警里说一句。
    pub fn names_a_later_root(&self) -> bool {
        let given = path_key(Path::new(&self.projects_dir));
        self.configured_roots
            .iter()
            .skip(1)
            .any(|root| path_key(Path::new(root)) == given)
    }

    /// 给人看的那一句：两个目录各是什么、各管哪一段、该怎么办。批量打到 stderr、写进 `summary.txt`；
    /// 页面进预检的 `notes` 与报告的 `warnings`。
    pub fn warning(&self) -> String {
        let configured = self
            .configured_roots
            .first()
            .map(String::as_str)
            .unwrap_or("（未配置）");
        let later = if self.configured_roots.len() > 1 {
            format!(
                "（其后的搜索根：{}）",
                self.configured_roots[1..].join("、")
            )
        } else {
            String::new()
        };
        let maybe_same = if self.names_a_later_root() {
            "（这个目录是配置里排在后面的搜索根之一：项目若正是在它下面找到的，两边读的是同一份，这一条可以不理。）"
        } else {
            ""
        };
        format!(
            "警告：项目父目录（--projects-dir）{} 与配置的 project_path {}{later} 不是同一个目录。\
             这次核对里两边各管一段：枚举根、模型树、根所在的设计库文件按配置找，不看 --projects-dir；\
             预检、E3D 导出基准的会话、生成时找被引用的库（<项目>/<码>000）用 --projects-dir。\
             两处若是内容不同的两份项目数据，比出来的差异里会混进不是模型造成的那一种，而且与真的模型差异分不开\
             ——让两边指向同一份再跑（去掉 --projects-dir，或把配置的 project_path 改成这个目录）。{maybe_same}",
            self.projects_dir, configured
        )
    }
}

/// 显式给的项目父目录与配置的第一顺位搜索根（`project_path`）字面上是不是同一处；不是就给出那一笔。
///
/// 只在**显式给了**的时候比：没给时用的就是配置的值，不存在「不一致」。归一规则与搜索根去重同一套
/// （[`normalize_path_input`] + [`path_key`]：首尾空白与引号、斜杠方向、大小写、末尾分隔符），**只看字面、不碰
/// 文件系统**——根可能是掉线的 UNC，`canonicalize` 会在调用方线程上等一个 OS 超时（`mdb_closure::collect_search_roots`
/// 的说明）。代价是联接、8.3 短名、网络路径与本地路径这类「字面不同、实为一处」的写法会被当成不一致：告警不拒绝，
/// 就是为它们留的余地。配置里一个搜索根都没有时无从比较，不出声。
pub fn project_dir_mismatch_of(
    given: &str,
    configured_roots: &[PathBuf],
) -> Option<ProjectDirMismatch> {
    if given.trim().trim_matches('"').is_empty() {
        return None;
    }
    let given = normalize_path_input(given);
    let first = configured_roots.first()?;
    if path_key(&given) == path_key(first) {
        return None;
    }
    Some(ProjectDirMismatch {
        projects_dir: given.display().to_string(),
        configured_roots: configured_roots
            .iter()
            .map(|root| root.display().to_string())
            .collect(),
    })
}

/// 配置的搜索根（`mdb_closure::search_roots`：`[project_path] ++ project_search_paths`，字面去重、保持先后）。
/// 只读配置，不碰文件系统。
pub fn configured_search_roots() -> Vec<PathBuf> {
    crate::data_interface::mdb_closure::search_roots(&aios_core::get_db_option())
}

pub fn resolve_tools() -> ResolvedTools {
    resolve_tools_from(ToolSearch {
        configured: crate::options::rvm_check_section(),
        exe_dir: std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        cwd: std::env::current_dir().ok(),
    })
}

pub fn resolve_tools_from(search: ToolSearch) -> ResolvedTools {
    let launcher = pick(
        search.configured.e3d_launcher.as_deref(),
        &launcher_candidates(&search),
    );
    let work_dir = configured_path(search.configured.work_dir.as_deref())
        .unwrap_or_else(|| default_work_dir(search.exe_dir.as_deref(), search.cwd.as_deref()));
    ResolvedTools {
        launcher_found: launcher.is_file(),
        launcher,
        work_dir,
        alive_timeout: Duration::from_secs(
            search
                .configured
                .alive_timeout_secs
                .unwrap_or(300)
                .clamp(30, 3600),
        ),
        timeout: Duration::from_secs(
            search
                .configured
                .timeout_secs
                .unwrap_or(1200)
                .clamp(60, 7200),
        ),
    }
}

pub fn project_defaults() -> ProjectDefaults {
    let option = aios_core::get_db_option();
    let configured_path = PathBuf::from(option.project_path.trim());
    let project_name = option.project_name.trim().to_string();
    // ADR-078 允许 `project_path` 直接指向**项目目录本身**（例如 `\\...\YCYK-E3D`），
    // 而不是它的上一级。服务侧的项目定位器一直认这种写法（`project_locator::locate_project_folder`
    // 按文件夹名匹配，NOMAIN 摆法下根本身就是项目目录），rvm-check 以前硬拼
    // `projects_dir.join(project_name)`，于是报「项目目录下没有 <码>000」（GitHub #141）。
    //
    // 判据只看磁盘事实，不看配置写法：
    //   * `<project_path>/<project_name>` 存在 → 老写法（上一级目录），照旧；
    //   * 否则 `<project_path>` 自己就是个项目目录（末尾那段等于 `project_name`，
    //     或者底下直接有 `<码>000`）→ 认它，并把 projects_dir 退回它的上一级。
    // 两种写法都没有时保持老行为，让调用方照旧报错——不猜。
    let (projects_dir, project_dir) = match project_dir_from_configured(&configured_path, &project_name)
    {
        Some(pair) => pair,
        None => {
            let project_dir = configured_path.join(&project_name);
            (configured_path.clone(), project_dir)
        }
    };
    let e3d_projects = suggest_e3d_projects(&project_dir);
    let e3d_project = e3d_projects.first().cloned();
    let mdb = normalize_mdb(option.mdb_name.trim());
    ProjectDefaults {
        projects_dir,
        project_name,
        project_dir,
        e3d_project,
        e3d_projects,
        mdb,
        numeric_project_code: option.project_code.trim().to_string(),
    }
}

/// 从配置的 `project_path` 与 `project_name` 定出（项目父目录，项目目录）。
///
/// 返回 `None` 表示两种写法都对不上——调用方按老行为报错，不猜。
fn project_dir_from_configured(configured: &Path, project_name: &str) -> Option<(PathBuf, PathBuf)> {
    // 老写法优先：`<project_path>/<project_name>` 真是个目录就用它。
    if !project_name.is_empty() {
        let joined = configured.join(project_name);
        if joined.is_dir() {
            return Some((configured.to_path_buf(), joined));
        }
    }
    // 新写法：`project_path` 自己就是项目目录。
    if !project_name.is_empty() {
        let last_segment_matches = configured
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(project_name));
        if last_segment_matches {
            let parent = configured.parent().map(Path::to_path_buf)?;
            return Some((parent, configured.to_path_buf()));
        }
    }
    // 兜底：`project_path` 底下直接有 `<码>000`（没有同名子目录），也算它本身就是项目目录。
    // `project_name` 为空时也走这一条——此时靠目录内容认，不靠名字。
    if !suggest_e3d_projects(configured).is_empty() {
        let parent = configured.parent().map(Path::to_path_buf)?;
        return Some((parent, configured.to_path_buf()));
    }
    None
}

pub fn suggest_e3d_projects(project_dir: &Path) -> Vec<String> {
    let Ok(dirs) = crate::data_interface::project_paths::collect_db_dirs_in(project_dir) else {
        return Vec::new();
    };
    let mut codes = Vec::new();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(code) = name.strip_suffix("000").filter(|prefix| !prefix.is_empty()) else {
            continue;
        };
        if code.ends_with(|ch: char| ch.is_ascii_digit()) {
            continue;
        }
        let code = code.to_ascii_uppercase();
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    codes
}

pub fn normalize_mdb(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "/ALL".to_string();
    }
    if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
    }
}

fn pick(configured: Option<&str>, candidates: &[PathBuf]) -> PathBuf {
    if let Some(path) = configured_path(configured) {
        return path;
    }
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .or_else(|| candidates.first().cloned())
        .unwrap_or_else(|| PathBuf::from(""))
}

fn configured_path(value: Option<&str>) -> Option<PathBuf> {
    let value = value?.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn launcher_candidates(search: &ToolSearch) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let relative = Path::new("scripts")
        .join("e3d")
        .join("run_ams_c_entrymacro.bat");
    if let Some(dir) = &search.exe_dir {
        out.push(dir.join(&relative));
    }
    if let Some(cwd) = &search.cwd {
        out.push(cwd.join(&relative));
    }
    out
}

fn default_work_dir(exe_dir: Option<&Path>, cwd: Option<&Path>) -> PathBuf {
    if let Some(dir) = exe_dir {
        if dir.join("aios-database.exe").is_file() || dir.join("tools").is_dir() {
            return dir.join("output").join("rvm-check");
        }
    }
    cwd.unwrap_or(Path::new("."))
        .join("output")
        .join("rvm-check")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_launcher_wins_and_timeout_defaults() {
        let root = std::env::temp_dir().join(format!("rvm-check-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let configured = root.join("configured-launcher.bat");
        std::fs::write(&configured, b"configured").unwrap();
        let resolved = resolve_tools_from(ToolSearch {
            configured: RvmCheckSection {
                e3d_launcher: Some(configured.display().to_string()),
                ..RvmCheckSection::default()
            },
            exe_dir: Some(root.clone()),
            cwd: Some(root.clone()),
        });
        assert_eq!(resolved.launcher, configured);
        assert!(resolved.launcher_found);
        assert_eq!(resolved.timeout, Duration::from_secs(1200));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GitHub #145：显式给的项目父目录与配置的 `project_path` 不是同一处才出声；同一目录的不同写法
    /// （斜杠方向、大小写、末尾分隔符、引号）不算；没给不比；配置里没有搜索根无从比。
    #[test]
    fn an_explicit_projects_dir_is_compared_literally_with_the_configured_project_path() {
        let roots = [
            PathBuf::from(r"D:\AVEVA\Projects"),
            PathBuf::from(r"\\files\share\projects"),
        ];
        // 没给：用的就是配置的值。
        assert_eq!(project_dir_mismatch_of("", &roots), None);
        assert_eq!(project_dir_mismatch_of("  \"\"  ", &roots), None);
        // 同一目录的不同写法。
        let same_spellings: &[&str] = if cfg!(windows) {
            &[
                r"D:\AVEVA\Projects",
                r"D:\AVEVA\Projects\",
                "D:/AVEVA/Projects/",
                r"d:\aveva\projects",
                "  \"D:\\AVEVA\\Projects\"  ",
            ]
        } else {
            &[r"D:\AVEVA\Projects", "  \"D:\\AVEVA\\Projects\"  "]
        };
        for same in same_spellings {
            assert_eq!(
                project_dir_mismatch_of(same, &roots),
                None,
                "同一目录的另一种写法不该算不一致: {same}"
            );
        }
        // 另一个目录：给出两边各是什么。
        let other = project_dir_mismatch_of(r"E:\old-copy\Projects", &roots).expect("不一致");
        assert_eq!(other.projects_dir, r"E:\old-copy\Projects");
        assert_eq!(
            other.configured_roots,
            vec![
                r"D:\AVEVA\Projects".to_string(),
                r"\\files\share\projects".to_string()
            ]
        );
        assert!(!other.names_a_later_root());
        let warning = other.warning();
        assert!(
            warning.starts_with("警告：")
                && warning.contains(r"E:\old-copy\Projects")
                && warning.contains(r"D:\AVEVA\Projects")
                && warning.contains(r"\\files\share\projects")
                && warning.contains("枚举根、模型树、根所在的设计库文件按配置找")
                && warning.contains("E3D 导出基准的会话")
                && !warning.contains("可以不理"),
            "{warning}"
        );
        // 给的是配置里排在后面的搜索根：照样算不一致（项目从哪个根读到的这里判不了），告警里多说一句。
        let later = project_dir_mismatch_of(r"\\files\share\projects", &roots).expect("不一致");
        assert!(later.names_a_later_root());
        assert!(later.warning().contains("可以不理"), "{}", later.warning());
        // 配置里一个搜索根都没有：无从比较。
        assert_eq!(project_dir_mismatch_of(r"E:\old-copy\Projects", &[]), None);
        // 留痕的形状：两格，原样的路径。
        assert_eq!(
            serde_json::to_value(&other).unwrap(),
            serde_json::json!({
                "projects_dir": r"E:\old-copy\Projects",
                "configured_roots": [r"D:\AVEVA\Projects", r"\\files\share\projects"],
            })
        );
    }

    #[test]
    fn mdb_and_project_code_normalize() {
        assert_eq!(normalize_mdb("ALL"), "/ALL");
        assert_eq!(normalize_mdb("/ALL"), "/ALL");
        assert_eq!(normalize_mdb("  "), "/ALL");
    }

    /// 老写法：`project_path` 是上一级目录，项目目录是 `<project_path>/<project_name>`。
    /// 这是原有行为，不能被 #141 的改动动到。
    #[test]
    fn a_projects_dir_with_the_project_folder_inside_keeps_the_old_shape() {
        let tmp = std::env::temp_dir().join(format!("rvm141-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let project = tmp.join("AvevaMarineSample");
        std::fs::create_dir_all(project.join("ams000")).unwrap();
        std::fs::create_dir_all(project.join("acp000")).unwrap();

        let (projects_dir, project_dir) =
            project_dir_from_configured(&tmp, "AvevaMarineSample").expect("老写法应当认得");
        assert_eq!(projects_dir, tmp);
        assert_eq!(project_dir, project);
        // 项目码照旧从项目目录底下的 `<码>000` 扫出来。
        let mut codes = suggest_e3d_projects(&project_dir);
        codes.sort();
        assert_eq!(codes, ["ACP", "AMS"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 新写法（#141）：`project_path` 直接指向项目目录本身。
    /// 项目父目录要退回上一级，项目目录就是它自己，项目码仍能扫出来。
    #[test]
    fn a_project_path_pointing_at_the_project_itself_is_accepted() {
        let tmp = std::env::temp_dir().join(format!("rvm141-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let project = tmp.join("YCYK-E3D");
        std::fs::create_dir_all(project.join("acw000")).unwrap();

        let (projects_dir, project_dir) =
            project_dir_from_configured(&project, "YCYK-E3D").expect("新写法应当认得");
        assert_eq!(projects_dir, tmp, "项目父目录应退回上一级");
        assert_eq!(project_dir, project, "项目目录就是 project_path 本身");
        assert_eq!(suggest_e3d_projects(&project_dir), ["ACW"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 两种写法都对不上时不猜：调用方照旧报「项目目录下没有 <码>000」。
    #[test]
    fn neither_shape_matching_is_not_guessed() {
        let tmp = std::env::temp_dir().join(format!("rvm141-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        assert_eq!(
            project_dir_from_configured(&tmp, "AvevaMarineSample"),
            None,
            "空目录既不是项目目录、底下也没有同名子目录"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
