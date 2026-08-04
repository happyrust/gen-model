//! 监控目录解析自检：不连数据库、不起服务，只回答「按当前 DbOption.toml，
//! 增量看门狗会监听哪些目录，解析不出来的又卡在哪一步」。
//!
//! 看门狗那句「没有任何监控目录挂载成功」在现场无从下手——目录列表为空、共享盘
//! 掉线、路径写错在日志里长得一样。这个探针把 `plan_watch_dirs` 的逐项目结论连同
//! 每个目录的可读性一起打成 JSON，先跑它再决定改配置还是查网络。
//!
//! 用法（在含 DbOption.toml 的目录下执行）：
//!
//! ```text
//! cargo run --bin watch_dirs_probe
//! cargo run --bin watch_dirs_probe -- --pretty
//! cargo run --bin watch_dirs_probe -- --remount-selftest
//! ```

use std::path::PathBuf;

use aios_database::data_interface::increment_manager::is_candidate_db_file;
use aios_database::data_interface::project_paths::{MountState, plan_watch_dirs};
use notify::{Config, PollWatcher};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let pretty = std::env::args().any(|arg| arg == "--pretty");
    if std::env::args().any(|arg| arg == "--remount-selftest") {
        return remount_selftest(pretty);
    }
    let option = aios_core::get_db_option();
    let plan = plan_watch_dirs(option);

    let projects = plan
        .projects
        .iter()
        .map(|project| {
            let root = project.root.as_ref();
            let dirs = project
                .db_dirs
                .iter()
                .map(|dir| {
                    let listing = std::fs::read_dir(dir);
                    let (readable, candidates, error) = match listing {
                        Ok(entries) => {
                            let count = entries
                                .filter_map(Result::ok)
                                .filter(|entry| is_candidate_db_file(&entry.path()))
                                .count();
                            (true, Some(count), None)
                        }
                        Err(error) => (false, None, Some(error.to_string())),
                    };
                    json!({
                        "dir": dir.display().to_string(),
                        "readable": readable,
                        "candidate_db_files": candidates,
                        "error": error,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "project": project.project,
                "root": root.map(|root| root.display().to_string()),
                "root_exists": root.map(|root| root.is_dir()),
                "problem": project.problem,
                "db_dirs": dirs,
            })
        })
        .collect::<Vec<_>>();

    let report = json!({
        "project_path": option.project_path,
        "included_projects": option.included_projects,
        "project_dirs": option.project_dirs,
        "watch_dirs": plan.dirs().iter().map(|dir| dir.display().to_string()).collect::<Vec<_>>(),
        "watch_dir_count": plan.dirs().len(),
        "watchdog_would_start": !plan.is_empty(),
        "problems": plan.problems(),
        "projects": projects,
    });

    emit(&report, pretty)
}

fn emit(report: &serde_json::Value, pretty: bool) -> anyhow::Result<()> {
    let text = if pretty {
        serde_json::to_string_pretty(report)?
    } else {
        serde_json::to_string(report)?
    };
    println!("{text}");
    Ok(())
}

/// 共享盘晚上线的自检：在临时目录上跑 `plan_watch_dirs` + [`MountState::mount`]
/// 这两个真实函数，只是把「共享盘恢复」换成「把目录建出来」。
///
/// 不连数据库、不起服务，因此可以随时跑；它盖住的正是重挂轮的两步——重新解析
/// （启动时不存在的项目根，此刻才列得出 `*000` 目录）与只补挂缺席的那些。
fn remount_selftest(pretty: bool) -> anyhow::Result<()> {
    let root = std::env::temp_dir().join(format!("aios-remount-selftest-{}", std::process::id()));
    let early = root.join("ProjEarly").join("early000");
    let late_root = root.join("ProjLate");
    let late = late_root.join("late000");
    std::fs::create_dir_all(&early)?;

    let mut option = aios_core::get_db_option().clone();
    option.project_path = root.to_string_lossy().into_owned();
    option.included_projects = vec!["ProjEarly".into(), "ProjLate".into()];
    option.project_dirs = None;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut watcher = PollWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default().with_poll_interval(std::time::Duration::from_secs(30)),
    )?;
    let mut mounted = MountState::new();

    // 第一轮：ProjLate 还不存在，等价于共享盘没上线。
    let boot_plan = plan_watch_dirs(&option);
    let boot_failures = mounted.mount(&mut watcher, &boot_plan.dirs());
    let boot = json!({
        "resolved": display(boot_plan.dirs()),
        "problems": boot_plan.problems(),
        "mounted": mounted.len(),
        "mount_failures": boot_failures,
    });

    // 共享盘恢复。
    std::fs::create_dir_all(&late)?;

    // 第二轮：重新解析 → 只补挂缺席的那个。
    let remount_plan = plan_watch_dirs(&option);
    let missing = mounted.missing(remount_plan.dirs());
    let before = mounted.len();
    let remount_failures = mounted.mount(&mut watcher, &missing);
    let remount = json!({
        "resolved": display(remount_plan.dirs()),
        "problems": remount_plan.problems(),
        "missing_before_remount": display(missing),
        "newly_mounted": mounted.len() - before,
        "mounted_total": mounted.len(),
        "mount_failures": remount_failures,
    });

    // 第三轮：什么都没变，缺席集合应当为空——重复 watch 同一个目录会让 F6 把库
    // 判成同号重复而整库阻断，所以「不重复挂」和「能补挂」一样重要。
    let idle_missing = mounted.missing(plan_watch_dirs(&option).dirs());
    let idle_clean = idle_missing.is_empty();

    drop(watcher);
    let _ = std::fs::remove_dir_all(&root);

    let report = json!({
        "fixture_root": root.display().to_string(),
        "boot_with_share_offline": boot,
        "after_share_recovered": remount,
        "idle_round_missing": display(idle_missing),
        "passed": boot["mounted"] == 1
            && remount["newly_mounted"] == 1
            && remount["mounted_total"] == 2
            && idle_clean,
    });
    emit(&report, pretty)
}

fn display(dirs: Vec<PathBuf>) -> Vec<String> {
    dirs.iter().map(|dir| dir.display().to_string()).collect()
}
