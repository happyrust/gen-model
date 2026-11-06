//! 工具和项目默认值。账号密码不进这里。

use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let projects_dir = PathBuf::from(option.project_path.trim());
    let project_name = option.project_name.trim().to_string();
    let project_dir = projects_dir.join(&project_name);
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

    #[test]
    fn mdb_and_project_code_normalize() {
        assert_eq!(normalize_mdb("ALL"), "/ALL");
        assert_eq!(normalize_mdb("/ALL"), "/ALL");
        assert_eq!(normalize_mdb("  "), "/ALL");
    }
}
