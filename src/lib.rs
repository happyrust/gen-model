#![feature(let_chains)]
#![feature(async_closure)]
#![feature(exact_size_is_empty)]
#![feature(slice_take)]
#![feature(const_async_blocks)]
#![feature(type_alias_impl_trait)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
use crate::fast_model::room_model::{
    StartupRoomBuild, build_room_relations, reconcile_startup_room_build,
};
use crate::versioned_db::database::*;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::options::DbOption;
use aios_core::pdms_data::AttInfoMap;
use aios_core::pdms_types::*;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::{SUL_DB, build_cate_relate, pdms_types::*};
use aios_core::{get_db_option, init_demo_test_surreal, init_surreal};
use anyhow::anyhow;
use chrono::{Datelike, Local, Timelike};
use dashmap::mapref::one::Ref;
use dashmap::{DashMap, DashSet};
use itertools::Itertools;
use lazy_static::lazy_static;
use nom::combinator::map;
use serde_json::from_str;
use std::any::TypeId;
use std::collections::BTreeSet;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use surrealdb::opt::auth::Root;
use team_data::sync_team_data;
// use tokio::sync::mpsc::Sender;
use aios_core::material::save_all_material_data;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use versioned_db::database::{define_dbnum_event, sync_pdms};

use log::{LevelFilter, error};
use simplelog::*;

struct ProcessInstanceLock {
    /// Keeping this handle alive keeps the lock alive: a deny-share open on
    /// Windows, the kernel's `flock` on Unix. Both evaporate with the process
    /// — `SIGKILL` included — so a stale lock never needs manual cleanup.
    _file: File,
    path: PathBuf,
    project: String,
}

static PROCESS_INSTANCE_LOCK: OnceLock<Result<ProcessInstanceLock, String>> = OnceLock::new();

#[cfg(windows)]
fn open_process_instance_lock(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        // No other process may open the same project lock while this handle
        // lives. Windows releases it even after an ungraceful process exit.
        .share_mode(0)
        .open(path)
}

/// The Unix branch's whole body, compiled on every platform on purpose: the
/// development and CI machines are Windows, and ISSUE-023 happened precisely
/// because the non-Windows code (`Ok(())`) and its tests never got built
/// there. `File::try_lock` is `flock(LOCK_EX | LOCK_NB)` on Unix and
/// `LockFileEx` on Windows — same acquire/refuse/release-on-drop shape — so a
/// Windows test run type-checks and exercises the exact logic the CentOS
/// deployment runs.
#[cfg_attr(windows, allow(dead_code))] // Windows 生产路径走 deny-share；这里测试在用
fn open_advisory_process_instance_lock(path: &std::path::Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            // The holder wrote its identity into the file (project / pid /
            // started_at); read it back so the refusal names who is holding.
            // On Windows the holder's exclusive range makes this read fail —
            // acceptable, the advisory path is only a test vehicle there.
            let mut owner = String::new();
            let _ = std::io::Read::read_to_string(&mut (&file), &mut owner);
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("锁被占用：{}", owner.trim().replace('\n', "，")),
            )
        }
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

/// Unix side of the same contract. ISSUE-023: this used to be a bare
/// `Ok(())`, which let a second CentOS process become a second writer of the
/// same dabacon files, SurrealDB and meshes — every "process-wide is
/// global-wide" invariant (geometry gate, staged executor, pending cleanup)
/// silently lost its premise.
///
/// The `flock` lives on the open file description, so it holds exactly as
/// long as the handle and the kernel reclaims it after any exit, `SIGKILL`
/// included. Deliberately not "does the lock file exist" — that shape leaves
/// stale locks for an operator to clean up. One flock caveat stays real:
/// network filesystems may not honour it, so a project directory on NFS
/// weakens this to best-effort (the deployment guide's problem; the code must
/// never turn that into a silent pass — a silent pass is exactly the shape
/// this issue had).
#[cfg(not(windows))]
fn open_process_instance_lock(path: &std::path::Path) -> std::io::Result<File> {
    open_advisory_process_instance_lock(path)
}

const INSTANCE_LOCK_FILE_NAME: &str = ".gen-model.instance.lock";

/// 锁落在**运行目录**（进程 CWD），不落在项目文件夹。
///
/// 项目树是甲方的数据，我们的运行期文件不该往里写；运行目录本来就归这一次运行
/// 所有——`DbOption.toml`、`accel_tree` 快照、日志都在那儿，锁跟它们同处一地。
///
/// 换锚点也换了这把锁挡的是什么：过去是「一个项目文件树只许一个进程」，现在是
/// 「一个运行目录只许一个进程」。两个运行目录指着同一个工程不再被它拦下，这正是
/// 并排跑两个实例所需要的。它并没有交出一条原本成立的保证：按项目根隔离的旧锁
/// 同样挡不住两个部署包（各自的 `project_path`）写同一台 SurrealDB，那种互踩一直
/// 由 `full_init` 的同工程活服务探测兜着（`python/src/exec_api.rs`
/// `conflicting_services`），锚点换了它照旧管用。
fn process_instance_lock_path() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|run_dir| run_dir.join(INSTANCE_LOCK_FILE_NAME))
}

static NO_INSTANCE_LOCK_NOTICE: std::sync::Once = std::sync::Once::new();

/// 这一趟要不要拿锁；不拿时返回那句给人看的理由。
///
/// 内存模式（`options::in_memory_db`）不拿。锁挡的是「两个进程驱动同一份持久
/// 状态」，而内存模式下持久层就在进程里：两个实例各连各的 kv-mem，谁也读不到谁
/// 写的字节，锁没有保护对象。放开它正是为了让两个实例并排跑起来。
///
/// 放开不是白送的。盘上还有东西是共享的：`meshes_path` 指的网格目录首当其冲——
/// 增量路径内部会强制落网格，`gen_mesh=false` 拦不住它。运行目录相对的那些
/// （`accel_tree` 快照、日志、`cata_dep_cache.bin`）天然分开，绝对路径的不会。
/// 所以并排跑之前得先让两份配置的 `meshes_path` 分家。
fn instance_lock_skip_reason(in_memory_db: bool) -> Option<&'static str> {
    in_memory_db.then_some(
        "in_memory_db=true，持久层在进程内，两个实例不共享库（meshes_path 仍是共享的，先让它分家）",
    )
}

/// pub：Python 调试绑定（python/aios-py）的 `full_init` 与 `run_app`/`run_cli`
/// 共用同一把单实例锁——mutating 管线不允许有第二个进程并发驱动。
///
/// 两处例外写在 [`instance_lock_skip_reason`] 与 [`process_instance_lock_path`]：
/// 内存模式整个不拿，拿的时候按运行目录而不是项目根隔离。
pub fn acquire_process_instance_lock(db_option: &DbOption) -> anyhow::Result<()> {
    if let Some(reason) = instance_lock_skip_reason(crate::options::in_memory_db()) {
        // run_app 与 run_cli 会各调一次，这句只该出现一遍。
        NO_INSTANCE_LOCK_NOTICE.call_once(|| println!("*** 不拿单实例锁：{reason}"));
        return Ok(());
    }
    let project = db_option.project_name.clone();
    let held = PROCESS_INSTANCE_LOCK.get_or_init(|| {
        let path = process_instance_lock_path()
            .ok_or_else(|| "未解析到运行目录（CWD），单实例锁无处可放".to_string())?;
        let mut file = open_process_instance_lock(&path).map_err(|error| {
            format!(
                "这个运行目录已有 gen-model 实例，或单实例锁不可访问（{}）: {error}",
                path.display()
            )
        })?;
        file.set_len(0)
            .map_err(|error| format!("清空单实例锁 {} 失败: {error}", path.display()))?;
        let owner = format!(
            "project={project}\npid={}\nstarted_at={}\n",
            std::process::id(),
            Local::now().to_rfc3339()
        );
        std::io::Write::write_all(&mut file, owner.as_bytes())
            .map_err(|error| format!("写单实例锁 {} 失败: {error}", path.display()))?;
        // ponytail: one deny-share handle held in OnceLock is the whole
        // single-instance mechanism; no lease table or second lock layer.
        Ok(ProcessInstanceLock {
            _file: file,
            path,
            project: project.clone(),
        })
    });

    match held {
        Ok(lock) if lock.project == project => Ok(()),
        Ok(lock) => anyhow::bail!(
            "本进程已持有项目 {} 的单实例锁 {}，不能再启动项目 {project}",
            lock.project,
            lock.path.display()
        ),
        Err(error) => anyhow::bail!("{error}"),
    }
}

#[cfg(test)]
mod process_instance_lock_tests {
    use super::{
        INSTANCE_LOCK_FILE_NAME, instance_lock_skip_reason, open_advisory_process_instance_lock,
        open_process_instance_lock, process_instance_lock_path,
    };

    /// 锁落在运行目录，不落在项目树里。把锚点改回项目根这条就红。
    #[test]
    fn the_lock_file_sits_in_the_run_directory_not_the_project_tree() {
        let run_dir = std::env::current_dir().expect("运行目录总该拿得到");
        let path = process_instance_lock_path().expect("运行目录能解析出锁路径");

        assert_eq!(
            path.parent(),
            Some(run_dir.as_path()),
            "锁必须就在运行目录下，不能跟着 project_path 走"
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(INSTANCE_LOCK_FILE_NAME)
        );
    }

    /// 内存模式一把锁都不拿——两个实例并排跑靠的就是这一句。外部持久层照旧要拿。
    #[test]
    fn memory_mode_needs_no_single_instance_lock() {
        assert!(
            instance_lock_skip_reason(false).is_none(),
            "持久层在进程外时锁仍有保护对象，不许跳过"
        );
        let reason = instance_lock_skip_reason(true).expect("不拿锁必须说出为什么");
        assert!(
            reason.contains("in_memory_db"),
            "理由要点名是哪个开关放开的，否则日志里只是一句没有出处的「不拿锁」：{reason}"
        );
    }

    /// 两个平台同一句守卫语义：第一把手柄活着时第二个打开者被拒，手柄一掉锁随之
    /// 释放。Windows 靠 deny-share，Unix 靠 flock——flock 挂在 open file
    /// description 上，同进程内第二次 `open` 就是一个新的 OFD，内核对它的裁决与
    /// 第二个进程完全相同，所以这条测试在锁原语层面等价于双进程。ISSUE-023：
    /// Unix 分支曾是空 `Ok(())`，而这条测试原先只在 Windows 下编译，缺口因此
    /// 没有回归网；退回任何一种旧写法它都会红。
    #[test]
    fn the_project_lock_rejects_a_second_opener_until_the_handle_drops() {
        let path = std::env::temp_dir().join(format!(
            "gen-model-lock-test-{}-{}.lock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = open_process_instance_lock(&path).expect("first lock open");
        assert!(
            open_process_instance_lock(&path).is_err(),
            "第二个打开者必须被拒（Windows deny-share / Unix flock）"
        );
        drop(first);
        let reopened = open_process_instance_lock(&path).expect("lock releases with handle");
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    /// CentOS 真跑的那条 advisory 路径（Unix `flock`）在 Windows 上以
    /// `LockFileEx` 同形复现：拿到即排他、第二个尝试立刻被拒且错误里带
    /// 「锁被占用」、手柄一掉即释放。ISSUE-023 的教训就是非 Windows 代码在
    /// Windows 机器上零编译零测试——这条让它至少天天被这台机器跑到。
    #[test]
    fn the_advisory_lock_used_on_unix_holds_refuses_and_releases() {
        let path = std::env::temp_dir().join(format!(
            "gen-model-advisory-lock-test-{}-{}.lock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = open_advisory_process_instance_lock(&path).expect("first advisory lock");
        let refused = open_advisory_process_instance_lock(&path)
            .expect_err("advisory lock must reject a second opener");
        assert!(
            refused.to_string().contains("锁被占用"),
            "拒绝信息要说明是锁冲突，而不是一个裸 IO 错误：{refused}"
        );
        drop(first);
        let reopened =
            open_advisory_process_instance_lock(&path).expect("advisory lock releases with handle");
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }
}

/// 环境变量名：无论如何都不跑启动全量房间重建（既有运维止血口，语义不变）。
pub const SKIP_STARTUP_ROOM_BUILD_ENV: &str = "AIOS_SKIP_STARTUP_ROOM_BUILD";

/// 本次启动要不要跳过全量房间重建；跳过时返回那句给人看的理由。
///
/// 三道门按优先级排：
/// 1. [`SKIP_STARTUP_ROOM_BUILD_ENV`] —— 永不重建。它比自动执行开关更强，因为
///    「跑增量」与「跑 2 万面板级的全量重建」是两件事：L3 夹具、增量演练这类
///    场景要前者不要后者。
/// 2. `startup_autorun=false`（默认 true）—— 本次启动显式要求不自动干活。
/// 3. 两道门都放行，才与库侧凭据对账：只有空间状态真的变过才重建。
///
/// 第三道门是这次改动的要点。此前这一步是无条件的，于是每次重启都要为一件
/// 多半无事可做的事付上十几秒——而它真正的兜底价值只在「空间状态变了、增量
/// 房间队列又没收干净」时才兑现。
async fn skip_startup_room_build() -> Option<String> {
    if std::env::var(SKIP_STARTUP_ROOM_BUILD_ENV).is_ok() {
        return Some(format!("{SKIP_STARTUP_ROOM_BUILD_ENV} 已设置"));
    }
    if !crate::options::startup_autorun() {
        return Some("startup_autorun=false".to_string());
    }
    match reconcile_startup_room_build().await {
        StartupRoomBuild::Skip(reason) => Some(reason),
        StartupRoomBuild::Run(reason) => {
            println!("启动全量房间重建：{reason}");
            None
        }
    }
}

pub mod api;
pub mod cata;
pub mod consts;
pub mod data_interface;
pub mod e3d_query;
pub mod query_service;
pub mod tables;
// pub mod ssc;
pub mod defines;
pub mod team_data;

pub mod graph_db;
pub mod noun_layout;
pub mod test;
pub mod uda_table;

// RVM 基准对拍。注意与同名但无关的 `src/rvm/`（PDMS 元素遍历）区分。
#[cfg(feature = "rvm_verify")]
pub mod rvm_baseline;

#[cfg(feature = "gen_model")]
pub mod fast_model;

pub mod surreal_retry;

pub mod versioned_db;

pub mod mqtt_service;

pub mod options;

pub mod runtime_lag;

#[cfg(feature = "http_api")]
pub mod web_service;

// 添加options模块的重导出
pub use options::DbOptionExt;
pub use options::get_db_option_ext;

#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;
extern crate anyhow;

// pub async fn start_sync_task(
//     db_option: Arc<DbOption>,
//     progress_sender: Sender<f32>,
// ) -> anyhow::Result<()> {
//     if db_option.total_sync
//         || db_option.incr_sync
//         || db_option.only_sync_sys
//         || db_option.is_sync_history()
//     {
//         // println!("开始同步解析数据。");
//         // tokio::spawn(async move {
//         if let Err(e) = sync_pdms(&db_option).await {
//             eprintln!("同步PDMS数据失败: {}", e);
//         }
//         //记录进度
//         progress_sender.send(50.0).await?;
//     }

//     if db_option.build_cate_relate() {
//         println!("初始化创建Cate relate关系");
//         build_cate_relate(false).await?;
//     }
//     Ok(())
// }

/// 进程起点，供「初始化完成」横幅报一次总耗时。
///
/// `run_app`（服务正门）与 `run_cli`（python 绑定、l3_suite 夹具直接调的那道门）
/// 都在入口点一次名。`OnceLock` 让重复点名幂等，取到的永远是最早那次。
static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();

fn mark_process_start() -> Instant {
    *PROCESS_STARTED.get_or_init(Instant::now)
}

/// 启动步骤耗时的人类可读渲染："0.42s" / "3m07.3s" / "1h23m45s"。
pub(crate) fn fmt_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.2}s")
    } else if secs < 3600.0 {
        format!("{}m{:04.1}s", (secs / 60.0) as u64, secs % 60.0)
    } else {
        format!(
            "{}h{}m{:.0}s",
            (secs / 3600.0) as u64,
            ((secs % 3600.0) / 60.0) as u64,
            secs % 60.0
        )
    }
}

pub async fn run_cli(db_option: DbOption) -> anyhow::Result<()> {
    let startup_started = mark_process_start();
    // Must precede logging, schema repair, watcher startup and every model/
    // room write. A second process started from the same run directory exits
    // here instead of becoming another consumer of the same durable pending
    // table. In-memory mode takes no lock at all — see
    // `instance_lock_skip_reason`, there is no shared durable state to guard.
    acquire_process_instance_lock(&db_option)?;
    // 退役的收集口径开关（ADR-031）：留着它的部署以为自己关着净收集，实际相反。
    // 配置层吃掉未知键是静默的，这里把它变成有声的。
    if let Some(notice) = crate::options::retired_net_window_notice() {
        eprintln!("{notice}");
    }
    // 几何并发闸额度（specs/023）：非法值启动失败而非静默回退——额度是唯一的
    // 性能旋钮兼回滚开关（=1 即串行），必须在任何生成路径起跑之前定死。
    let geometry_workers =
        crate::fast_model::concurrency::validate_geometry_concurrency_config()
            .map_err(|error| anyhow::anyhow!("几何并发闸配置非法，拒绝启动：{error}"))?;
    println!("几何并发闸额度 = {geometry_workers}（geometry_workers 未配置时取物理核数）");
    // 调度延迟采样（specs/033 T003）：只观测、不改行为。放在这里是因为它要量的正是
    // 后面这一整段启动与生成期间 runtime 有多挤，起晚了就错过了最拥挤的那一段。
    crate::runtime_lag::spawn_sampler();
    // 监听限定域（`watch_dbnums` / `--watch-dbnum`）：配置里的那份能躺一个月，
    // 所以它必须在启动时就自报家门，而不是等人从「怎么只有一个库在动」倒推。
    if let Some(notice) = crate::data_interface::watch_scope::mode_notice() {
        eprintln!("*** {notice} ***");
    }
    // dbg!("begin run task");
    // 如果启用了日志功能
    if db_option.enable_log {
        let now = Local::now();
        let filename = format!(
            "{}-{}-{}-{}-{}-{}_dblog.txt",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );

        // 创建日志文件
        let file = File::create(filename).unwrap();

        CombinedLogger::init(vec![
            TermLogger::new(
                LevelFilter::Warn,
                Config::default(),
                TerminalMode::Mixed,
                ColorChoice::Auto,
            ),
            WriteLogger::new(LevelFilter::Info, Config::default(), file),
        ])
        .unwrap();
    }

    // progress_sender.send(5).await?;
    // progress_sender.send(5)?;

    // 磁盘脚本先加载：站点自有的额外 surql 继续生效。目录不存在不再是致命错——
    // 下面的内置快照兜底，部署包可以不带 resource/surreal。
    let preload_started = Instant::now();
    let step_started = Instant::now();
    if std::path::Path::new("resource/surreal").is_dir() {
        aios_core::function::define_common_functions().await?;
        println!(
            "磁盘 surql 函数脚本加载完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        );
    } else {
        println!("resource/surreal 不存在：磁盘脚本跳过，函数集完全来自二进制内置快照");
        if aios_core::function::ensure_inst_meta_functions_on(&SUL_DB).await? {
            println!("已从二进制内置定义补装缺失的 inst_meta 兼容函数");
        }
    }
    // 内置快照收尾（含 D11 的 hd/hh 矫正，见模块 doc）：部署包的 resource/surreal
    // 会漂移——现场 bin 停在 2025-06、整个缺 gen_root.surql——磁盘加载之后再灌一遍
    // 编译期快照，同名函数以内置版为准，旧运行环境被抬到当前函数集。
    let step_started = Instant::now();
    crate::data_interface::embedded_surql::define_embedded_functions().await?;
    match crate::data_interface::embedded_surql::missing_embedded_functions().await {
        Ok(missing) if missing.is_empty() => {}
        Ok(missing) => eprintln!(
            "内置函数集灌入后仍缺 fn::{}——引擎可能拒绝了新脚本（语句错误按惯例吞掉），\
             房间语义与 gen-root 巡检将退化",
            missing.join(" / fn::")
        ),
        Err(e) => eprintln!("内置函数集核验未跑成（{e}），不据此定罪"),
    }
    println!(
        "内置函数快照灌入并核验完成，耗时 {}",
        fmt_elapsed(step_started.elapsed())
    );
    let step_started = Instant::now();
    crate::data_interface::increment_pipeline::selfcheck_surreal_functions().await?;
    println!(
        "surreal 函数自检完成，耗时 {}",
        fmt_elapsed(step_started.elapsed())
    );
    let step_started = Instant::now();
    let migrated =
        crate::data_interface::dbnum_state::DbnumState::ensure_increment_state_storage().await?;
    println!(
        "增量状态表检查完成（兼容检查 {migrated} 个旧 DBNUM 水位），耗时 {}",
        fmt_elapsed(step_started.elapsed())
    );
    // 解析完成后重新定义EVENT
    println!("正在重新定义dbnum_event...");
    let step_started = Instant::now();
    match define_dbnum_event().await {
        Ok(_) => println!(
            "成功重新定义update_dbnum_event，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        ),
        Err(e) => println!("重新定义update_dbnum_event失败: {:?}", e),
    }
    println!("预加载方法完成。");

    // 初始化数据库索引（开始/完成与耗时日志在函数内部，python full_init 同样受益）
    if let Err(e) = crate::fast_model::pdms_inst::init_inst_relate_indices().await {
        eprintln!("初始化inst_relate索引失败: {}", e);
    }
    // 存量行 anc/dbnum 自愈回填（幂等；全新库与已回填库一轮空转即返回）
    if let Err(e) = crate::fast_model::pdms_inst::backfill_inst_relate_anc().await {
        eprintln!("inst_relate anc/dbnum 回填失败（下次启动重试）: {}", e);
    }
    // 存量行平表副本清扫（P4 写时物化；幂等——pre-P4 库首轮付清，之后空转即返回。
    // 崩溃在空闲轮清扫前留下的 NONE 行也在这里自愈）
    if let Err(e) = crate::fast_model::pdms_inst::sweep_inst_relate_flat().await {
        eprintln!("inst_relate 平表副本清扫失败（下次启动重试）: {}", e);
    }
    // 老格式再现探针（Spec 025 FR-9 盲区补口）：迁移标记已落的库上又出现
    // booled_id 与 insts_flat 不符的行，只可能来自旧 writer 混跑——migration
    // 按库上标记跳过、清扫两段也够不着，FR-8/T20 读侧自检落地前这里是唯一报告点。
    // 只挂启动序列（FR-1：inst_relate 全表谓词只许启动序列与人工诊断入口）。
    if let Err(e) =
        crate::fast_model::pdms_inst::probe_booled_flat_regression_after_migration().await
    {
        eprintln!("布尔平表老格式再现探针失败（下次启动再探）: {}", e);
    }
    println!(
        "启动预加载与自愈维护全部完成，总耗时 {}",
        fmt_elapsed(preload_started.elapsed())
    );

    let configured_sync_live = db_option.sync_live.unwrap_or(false);
    let direct_read_mode = crate::options::direct_read_mode();
    let sync_live = configured_sync_live && !direct_read_mode;
    if direct_read_mode {
        println!(
            "数据读取模式：direct（e3d-io）；保留 MDB 初始化与 Web API，跳过旧增量 watcher/worker"
        );
    }
    let db_option = Arc::new(db_option.clone());
    // initialize_global_db_sender().await;

    // start_sync_task(db_option.clone(), progress_sender.clone()).await?;
    //如果是解析任务，运行完就应该跳出
    if !direct_read_mode
        && (db_option.total_sync
            || db_option.incr_sync
            || db_option.only_sync_sys
            || db_option.is_sync_history())
    {
        let step_started = Instant::now();
        // println!("开始同步解析数据。");
        // tokio::spawn(async move {
        sync_pdms(&db_option).await?;
        println!(
            "PDMS 数据同步解析阶段完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        );
        //记录进度
        // progress_sender.send(90)?;
        if db_option.build_cate_relate() {
            println!("初始化创建Cate relate关系");
            let step_started = Instant::now();
            build_cate_relate(false).await?;
            println!(
                "Cate relate 关系创建完成，耗时 {}",
                fmt_elapsed(step_started.elapsed())
            );
        }
        // progress_sender.send(100)?;
        // 遗留路径断点当面说清（审计 2026-08-29 F6）：`sync_pdms` 只解析数据，
        // 全程不写 dbnum_state 水位、不产模型工作单、不建 gen_root 凭证。这个
        // 脱钩在两种组合下各有一个坑，以前一个字都不说，出了事只能翻代码。
        if db_option.total_sync {
            if sync_live {
                eprintln!(
                    "*** total_sync + sync_live=true：全量解析不建立增量水位，watcher 首扫会把\
                     这些库判成「有数据、水位 0」并按首次导入 wipe+重解析——整库将被解析两遍。\
                     只想全量导数据请改 sync_live=false；想直接服务化请去掉 total_sync，\
                     由首载批次一次完成解析+建模+水位"
                );
            } else {
                eprintln!(
                    "*** total_sync 完成：数据已入库，但增量水位未建立、模型未生成（本分支与\
                     水位/模型完全脱钩）。模型生成的出口：下次以 sync_live=true 启动（启动播种\
                     回填水位后按首载批次收口），或对目标库人工触发 POST /model/rebuild"
                );
            }
        }
    }

    println!("正在初始化 db manager（解析监控目录配置）...");
    let step_started = Instant::now();
    let mut manager = AiosDBManager::init_form_config().await?;
    // 本 MDB 声明了哪些库，在这里解一次并登记。位置是有讲究的：它读的是 SYS 库
    // 文件而不是 SurrealDB，所以能在任何东西被同步之前回答——而字典库名单恰恰
    // 是初始化期就得知道的东西（`UpdateScope` 那条查询要等 SYS 库先解析完，
    // 拿它来定字典范围会形成「要范围得先同步、要同步得先有范围」的死结）。
    //
    // 失败只告警不中止：目前还没有消费者，把一个新增的诊断变成启动阻断，
    // 代价与收益完全不成比例。真正要人看的那一条是「MDB 声明了这个库、
    // 但配置的项目目录里没有它」——`init_mdb` 内部逐条 warn。
    if let Err(error) = manager
        .init_mdb(
            &db_option.project_name,
            &db_option.mdb_name,
            &db_option.module,
        )
        .await
    {
        log::warn!(
            "解析 MDB {} / {} 的成员名单失败，按未登记继续：{error:#}",
            db_option.project_name,
            db_option.mdb_name
        );
    }
    let mgr = Arc::new(manager);
    println!(
        "db manager 初始化完成，耗时 {}",
        fmt_elapsed(step_started.elapsed())
    );
    /// 创建db manager
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    // `gen_model` / `gen_mesh` are model-stage capability switches, not a
    // request to rebuild every root at each process start. Startup discovery
    // has already compared file_latest_sesno with applied_sesno and queued only
    // first-load/reinit/increment windows; let those batches create the exact
    // model work for this process. Explicit full-build tools enter through
    // `E3dModelService::generate_dbnum`.
    initialization.configure_model_bootstrap(false);
    println!(
        "启动模型策略：gen_model={} gen_mesh={} 仅控制增量模型阶段；按文件会话号与 applied_sesno 比对结果执行，不启动整库全量生成",
        db_option.gen_model, db_option.gen_mesh
    );
    if !sync_live {
        initialization.mark_data_ready_without_manifest();
    }
    let step_started = Instant::now();
    match scheduler.restore_persisted_pause().await {
        Ok(true) => println!("队列处于暂停状态（重启前设置），启动重扫只入队不消费"),
        Ok(false) => {}
        Err(error) => println!("启动时恢复队列暂停标志失败（worker 启动前会重试）: {error:#}"),
    }
    println!(
        "队列暂停标志恢复完成，耗时 {}",
        fmt_elapsed(step_started.elapsed())
    );
    if !scheduler.is_auto_work_armed() {
        println!(
            "startup_autorun=false 且未声明 watch_dbnums：本次启动不执行任何增量与\
             全量房间重建；重扫照常发现并入队，但排出来的行挂起，等各自的 dbnum \
             真的来增量再跑（想只跑几个库就写 watch_dbnums，见 ADR-048）"
        );
        if !sync_live {
            // 解封条件是「某个 dbnum 真的来一次增量」，而增量只能由 watcher 或
            // 人工执行送来。watcher 没起，就只剩人工那一条——这话必须当面说清楚，
            // 否则队列里的行会安静地躺到下次重启。
            println!(
                "⚠ 同时 sync_live=false：watcher 没启动，不会有文件事件来解封任何一行。\
                 除非走人工执行 / 按需生成，持久积压（model_update_pending）本次不会\
                 有任何进展"
            );
        }
    }
    if sync_live {
        println!("正在启动监控目录 watcher（首轮重扫入队，含库文件存档压缩，可能较久）...");
        let step_started = Instant::now();
        mgr.init_watcher().await?;
        println!(
            "监控目录 watcher 启动完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        );
    }

    // Worker is started as soon as the immutable manifest is installed.  The
    // coordinator keeps it data-only until Meta -> Catalogue -> Design settles.
    if !direct_read_mode {
        crate::data_interface::batch_worker::ensure_batch_worker(mgr.clone());
    }

    // Expose initialization progress before waiting for data/model readiness.
    #[cfg(feature = "http_api")]
    let web_task = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::web_service::serve_if_configured(mgr).await {
                eprintln!("Web 服务异常退出: {e:?}");
            }
        })
    };

    // Watcher events are dirtiness signals only; `async_watch` debounces them
    // into the same full manifest scan used by startup and reconciliation.
    let watch_task = if sync_live {
        let mgr = mgr.clone();
        Some(tokio::spawn(async move {
            #[cfg(feature = "mqtt")]
            {
                let (watch_result, _) = tokio::join!(
                    mgr.async_watch(),
                    AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
                );
                if let Err(error) = watch_result {
                    log::error!("async_watch 退出，增量看门狗已停止: {error:?}");
                    eprintln!("async_watch 退出，增量看门狗已停止: {error:?}");
                }
            }
            #[cfg(not(feature = "mqtt"))]
            if let Err(error) = mgr.async_watch().await {
                log::error!("async_watch 退出，增量看门狗已停止: {error:?}");
                eprintln!("async_watch 退出，增量看门狗已停止: {error:?}");
            }
        }))
    } else {
        None
    };

    let startup_data_ready = !sync_live || scheduler.is_auto_work_armed();
    if sync_live && startup_data_ready {
        println!("等待 Meta → Catalogue → Design 数据清单完成...");
        initialization.wait_for_data_ready().await;
        println!("数据初始化完成，模型阶段可以开始");
    }

    // 数据基线完成后把权威生成根覆盖与当前水位快照对齐——run_app 里 ADR-050 那次
    // 无条件清空的补偿路径，因此**不**只挂在监听限定域上：限定域逐库 sync + 补种，
    // 未限定域按 gen_root 凭证点查、过期才补种（口径见函数注释）。仅看
    // model_update_pending=0 会把“从未生成过的根”误判成完成；这里只补缺失工作，
    // 不改水位、不删旧模型，也不越过 watch_dbnums。任何一库失败都告警后继续，
    // 不把一个库的配置错变成整个服务的崩溃循环。
    if startup_data_ready && !direct_read_mode {
        crate::data_interface::model_update_pending::reconcile_model_coverage_at_startup().await;
    }

    // 启动分层判据装载空间树（docs/2026-08-11_spatial-tree-startup-init-plan.md）：
    // 指纹（epoch 值 + 库侧时间戳）一致 → 直接复用；失配但有待重放空间意图 →
    // 复用文件交给重放自愈；失配且无意图 / 文件缺失损坏 → 只读指针重建。
    // 空间树是可重建的派生数据，加载失败不该顶掉整个启动：空树有下游防线
    // （全量重建拒跑、整间分支拒算），worker 启动收敛还会重放未完成的空间意图。
    println!("正在装载空间树（指纹一致直接复用，失配则重建）...");
    let step_started = Instant::now();
    match crate::fast_model::aabb_tree::load_project_tree_verified().await {
        Ok(_) => println!(
            "空间树装载完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        ),
        Err(error) => {
            eprintln!("空间树启动加载失败（{error:#}），以空树启动，等待后台复检重建");
        }
    }
    // 降级复检后台任务（一致性闭环方案 §6）：DegradedReuse/DegradedBlocked 两态
    // 由它退避重试收敛，恢复 Ready 后唤醒调度器；健康状态下它只是每 30s 一次的
    // 状态读取。
    crate::fast_model::spatial_state::spawn_spatial_revalidator();
    // progress_sender.send(10)?;
    if direct_read_mode {
        initialization.open_model_phase();
        initialization.mark_model_ready();
        println!("direct 模式初始化门已打开：按需模型请求可直接进入 e3d-io 读取与缓存判定");
    } else if startup_data_ready {
        if initialization.open_model_phase() {
            scheduler.wake();
            if !crate::options::model_incremental() {
                // worker 空闲轮的模型消费与 mark_model_ready 都挂在
                // `model_incremental()` 上（batch_worker 的空闲模型门），开关关着时
                // model_ready 在本进程内永远不会置位——这里若照常等待就是无限期挂死：
                // watcher/web 已经起了，启动序列却永远走不到房间阶段与完成横幅。
                // 模型门照开（按需生成沿用 require_model_generation 那道门），durable
                // 积压原样留存，只跳过等待；房间阶段由下方同一开关一并跳过。
                println!(
                    "模型增量阶段已关闭（model_incremental=false）：durable 模型积压留存不消化，\
                     跳过启动模型收敛等待（重新开启后由空闲轮回补；按需生成不受影响）"
                );
            } else {
                // 收敛发生在 worker 空闲轮里（模型积压分页消化、空间收敛、AABB 落盘，
                // 任一环失败都按 30s 退避重试），主线可能要等很久。干等会让启动看起来
                // 像挂死——数量/阶段变化时报实际进展，完全不变时最多每 300s 提醒一次。
                let mut waited_secs = 0u64;
                let mut last_wait_fingerprint: Option<String> = None;
                let mut last_wait_emitted_secs = 0u64;
                loop {
                    if tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        initialization.wait_for_model_ready(),
                    )
                    .await
                    .is_ok()
                    {
                        break;
                    }
                    waited_secs += 60;
                    let pending =
                        crate::data_interface::model_update_pending::model_pending_status().await;
                    let telemetry =
                        crate::data_interface::model_update_pending::model_drain_telemetry_snapshot(
                        );
                    let (message, fingerprint) = match pending {
                        Ok(status) => {
                            let count = |action: &str| {
                                status
                                    .by_action
                                    .get(action)
                                    .map_or(0, |counts| counts.retryable)
                            };
                            let non_regen = count("transform")
                                + count("delete_cleanup")
                                + count("cascade_expand");
                            let regen = count("regen_root");
                            let aabb = count("post_regen_aabb");
                            let stage = telemetry
                                .get("last_stage")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("waiting");
                            let page_claimed = telemetry
                                .get("last_page_claimed")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            let page_completed = telemetry
                                .get("last_page_completed")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            let phase = if non_regen > 0 {
                                "non_regen"
                            } else if regen > 0 {
                                "regen_root"
                            } else if aabb > 0 {
                                "post_regen_aabb"
                            } else {
                                "spatial_persist"
                            };
                            let message = if regen > 0 {
                                format!(
                                    "初始化模型进行中：阶段={phase} regen_root剩余={regen} \
                                     当前页={page_completed}/{page_claimed} 子阶段={stage} 已等={waited_secs}s；AABB尚未开始"
                                )
                            } else {
                                format!(
                                    "初始化模型进行中：阶段={phase} non_regen剩余={non_regen} \
                                     AABB剩余={aabb} 当前页={page_completed}/{page_claimed} \
                                     子阶段={stage} 已等={waited_secs}s"
                                )
                            };
                            let fingerprint = format!(
                                "{phase}|{non_regen}|{regen}|{aabb}|{stage}|{page_completed}|{page_claimed}"
                            );
                            (message, fingerprint)
                        }
                        Err(error) => (
                            format!("读取初始化模型进展失败（已等 {waited_secs}s）: {error:#}"),
                            format!("status_error:{error:#}"),
                        ),
                    };
                    // 「数字不变就退到 300 秒一行」这条退避在卡死时是反的：越卡越安静。
                    // 2026-08-27 现场前 19 分钟每 60 秒一行，真卡住之后反而变成 300 秒一行。
                    // 页级停滞一旦确认，就按 60 秒照常出声，并把停滞页数带上。
                    let starved =
                        crate::data_interface::model_update_pending::model_drain_page_starved();
                    let message = if starved {
                        format!(
                            "{message}；模型页已连续 {} 页整页认领、零收口——这不是慢是收不掉，\
                             看日志里点名的根与 /health 的 blocking_conditions",
                            crate::data_interface::model_update_pending::model_drain_starved_pages(
                            )
                        )
                    } else {
                        message
                    };
                    let changed = last_wait_fingerprint.as_deref() != Some(fingerprint.as_str());
                    if starved
                        || changed
                        || waited_secs.saturating_sub(last_wait_emitted_secs) >= 300
                    {
                        println!("{message}");
                        last_wait_fingerprint = Some(fingerprint);
                        last_wait_emitted_secs = waited_secs;
                    }
                }
                println!("持久模型工作单与 AABB 阶段已收敛");
            }
        }
    }

    println!("房间关键字为: {:?}", db_option.get_room_key_word());
    if !startup_data_ready {
        println!("数据初始化尚未释放，跳过启动房间重建");
    } else if direct_read_mode {
        println!("direct 模式跳过启动房间重建；按需模型由 Web API 独立生成");
    } else if !crate::options::model_incremental() {
        // 房间归属只在「数据与模型都已写入 RocksDB」之后才整体重算（ADR-010 §7 /
        // ADR-011 §8：房间依赖几何与 AABB 都已收敛）。模型开关关着时 durable 模型
        // 积压不消化，此刻全量重建就是在缺模型的空间树上算归属——先清后写还会把
        // 存量归属边改坏。跳过；重新开启模型阶段后由凭据对账（stamp 失配→启动全量
        // 重建）与模型收敛链派生的房间目标回补。
        println!(
            "模型增量阶段已关闭（model_incremental=false）：模型积压未收敛，跳过启动房间重建\
             （房间只在数据与模型都落库后才整体重算，重新开启模型阶段后回补）"
        );
    } else if let Some(reason) = skip_startup_room_build().await {
        println!("跳过启动全量房间重建（{reason}）：房间归属由增量队列收敛");
    } else {
        println!(
            "正在计算房间（空间树 {} 条）",
            GLOBAL_AABB_TREE.read().await.tree.size()
        );
        let time = Instant::now();
        // 单块面板算不出来不该拦住启动：这里在 `async_watch` 之前，panic 等于整个服务
        // 起不来，而房间归属是可以事后重建的派生数据。函数内已按面板逐条聚合失败原因，
        // 打出来即可定位——此前那些失败是被 `unwrap_or_default()` 吞成「这间房 0 个成员」的。
        if let Err(error) = build_room_relations(&db_option).await {
            eprintln!("计算房间未完全成功: {error:#}");
        }
        println!("计算房间花费时间: {} ms", time.elapsed().as_millis());
        // update_cal_equip().await?;
        // 支管部件派生量（阀门距楼板高度）与房间同姿态：可事后重建的派生数据。
        // 此前用 `?` 上抛，一次查询抖动就把整个服务打死在启动尾段——与上面
        // build_room_relations 的降级口径自相矛盾。
        if let Err(error) = update_cal_bran_component().await {
            eprintln!("支管部件派生量计算未完全成功（可由下次启动或手动重算回补）: {error:#}");
        }
    }

    // `run_app` has already connected the process-global `SUL_DB`.  Calling
    // `AiosDBMgr::init_from_db_option()` here tries to connect that same
    // singleton a second time.  Recent SurrealDB clients reject the second
    // connect with `Already connected` and, worse, leave the original router
    // channel closed.  This legacy manager is only a thin option holder for
    // the material/SYS/PBS calls below, so construct it over the established
    // connection instead of reconnecting.
    let aios_mgr = AiosDBMgr {
        db_option: db_option.as_ref().clone(),
    };
    // 生成材料表单
    let gen_material = db_option.gen_material.unwrap_or(false);
    if gen_material {
        save_all_material_data().await?;
    }
    // sync TEAM_DATA数据
    if db_option.only_sync_sys {
        println!("开始生成SYS DATA");
        match sync_team_data(&aios_mgr).await {
            Ok(_) => {
                println!("TEAM DATA生成完成");
            }
            Err(e) => {
                eprintln!("TEAM DATA生成失败: {e:#}");
            }
        }
    }

    if db_option.rebuild_ssc_tree {
        println!("生成PBS节点...");
        set_pdms_major_code(&aios_mgr).await?;
        let mut handles = vec![];
        set_pbs_fixed_node(&mut handles).await?;
        let rooms = set_pbs_room_node(&mut handles).await?;
        set_pbs_room_major_node(&rooms, &mut handles).await?;
        set_pbs_node(&mut handles).await?;
        futures::future::join_all(handles).await;
    }

    // 到这里为止的每一步都只在启动跑一次；再往下 `watch_task.await` 就只是挂着
    // 等增量了。此前日志里没有这条边界线——「还在初始化」与「已经在监听、只是
    // 这一轮没事发生」打出来的东西一模一样，只能靠猜。
    print_startup_complete_banner(&db_option, sync_live, startup_started);

    if let Some(watch_task) = watch_task {
        let _ = watch_task.await;
    }

    // 手动模式（sync_live=false）下若 Web 服务已启用，保持进程长驻对外服务，
    // 供前端触发 preview/execute 手动更新与按需生成。
    #[cfg(feature = "http_api")]
    if !sync_live && crate::get_db_option_ext().http_api_addr.is_some() {
        println!("手动模式下 Web 服务保持运行（Ctrl+C 退出）...");
        let _ = web_task.await;
    }

    Ok(())
}

/// 「初始化完成 → 进入增量监听」这条边界的横幅。
///
/// 状态在这里复述一遍，而不是只打一行「完成」：阶段开关、监听限定、HTTP 地址
/// 是出问题时第一批要确认的东西，而它们各自散在前面几十行启动日志里——等真出事
/// 时，那些行早被稳态轮询的重扫日志刷走了。
fn print_startup_complete_banner(db_option: &DbOption, sync_live: bool, started: Instant) {
    const RULE: &str = "==========================================================";
    println!("{RULE}");
    println!(
        "初始化完成：项目 {}，启动总耗时 {}",
        db_option.project_name,
        fmt_elapsed(started.elapsed())
    );
    println!(
        "  增量阶段：data={} model={} room={}（顺序：数据 → 模型 → 房间）",
        crate::options::data_incremental(),
        crate::options::model_incremental(),
        crate::options::room_incremental()
    );
    if crate::options::direct_read_mode() {
        println!("  数据读取：direct / e3d-io（旧增量 watcher/worker 未启动）");
    }
    if let Some(notice) = crate::data_interface::watch_scope::mode_notice() {
        println!("  *** {notice}");
    }
    if crate::options::in_memory_db() {
        println!(
            "  *** 内存模式（in_memory_db）：持久层是进程内嵌 kv-mem，进程一退整库即消失，\
             外部无端口可连"
        );
    } else {
        println!("  存储介质：{}", db_option.get_version_db_conn_str());
    }
    #[cfg(feature = "http_api")]
    {
        let ext = crate::get_db_option_ext();
        if let Some(addr) = ext.http_api_addr.as_deref() {
            println!("  HTTP API：http://{addr}/api/v1");
        }
    }
    if sync_live {
        println!("已进入增量更新监听：库文件一有变化就入队处理，往下的日志都是运行期输出。");
    } else {
        println!("sync_live=false：不进文件监听，增量只能由 HTTP preview/execute 手动触发。");
    }
    println!("{RULE}");
}

/// 把进程全局 `SUL_DB` 接到进程内嵌的 kv-mem 引擎上（[`options::in_memory_db`]）。
///
/// 换掉的只是介质。`SUL_DB` 是 `Surreal<Any>`，引擎选择对调用方不可见，所以初始化
/// 解析、增量窗口写回、模型与房间派生数据全部走原来那条路，只是落点从 rocksdb 变成
/// kv-mem。ADR-017 的暂存窗口本来就各自占一个 `mem://` 实例，这里连的是**另一个**
/// 实例：窗口与持久层仍是两个库，journal 分块重放与水位发布一个字都没改。
///
/// 嵌入式引擎没有认证面，因此不 `signin`——对着它调 `Root` 登录会直接失败。
/// 命名空间与库名仍取配置里的那一对，让同一份 `DbOption.toml` 在两种介质下指向
/// 同一个逻辑库。
///
/// 函数集在这里先灌一遍，与 ws 路径的 `init_surreal` 对齐：`run_app` 到 `run_cli`
/// 之间还有清理与空间树装载要跑。灌不动不在这里定罪——`run_cli` 随后会用磁盘脚本
/// 加内置快照再灌一遍并自检，那才是权威的那道门。
#[cfg(feature = "ws")]
async fn connect_in_memory_store(db_option: &DbOption) -> anyhow::Result<()> {
    SUL_DB.connect("mem://").with_capacity(1000).await?;
    SUL_DB
        .use_ns(&db_option.surreal_ns)
        .use_db(&db_option.project_name)
        .await?;
    if let Err(error) = aios_core::function::define_common_functions().await {
        eprintln!("内存库预装磁盘 surql 函数未成（{error:#}），等 run_cli 的内置快照兜底");
    }
    println!(
        "*** 内存模式：持久层是进程内嵌 kv-mem，ns={} db={}（未连 {}）",
        db_option.surreal_ns,
        db_option.project_name,
        db_option.get_version_db_conn_str()
    );
    println!(
        "*** 进程一退整库即消失，外部没有端口可连（rvm_verify / /sql 探针 / \
         Capture-*Evidence 都够不着）。要留证据请关掉 in_memory_db。"
    );
    Ok(())
}

/// 运行app
pub async fn run_app(option: Option<DbOptionExt>) -> anyhow::Result<()> {
    use std::sync::mpsc;

    use aios_core::init_surreal;

    mark_process_start();
    // 如果传入的是DbOptionExt，则取其内部的DbOption
    let db_option: DbOption = option
        .map(|o| options::apply_asset_root(o.inner))
        .unwrap_or_else(|| get_db_option_ext().inner);
    // Public entrypoint guard: take the deny-share handle before connecting a
    // local store or loading mutable process-global spatial state. `run_cli`
    // repeats this call deliberately; OnceLock makes that check idempotent.
    acquire_process_instance_lock(&db_option)?;
    let config = surrealdb::opt::Config::default()
    .ast_payload()  // 启用AST格式
    ; // 设置容
    #[cfg(feature = "local")]
    SUL_DB
        .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
        .with_capacity(1000)
        .await?;
    println!("数据库连接中...");
    #[cfg(feature = "ws")]
    {
        // 内存模式换掉的只是介质：下面这条 ws 连接与它连的那台服务器整个不出现，
        // 持久层改为进程内嵌 kv-mem，其余每一步照原样跑。
        if crate::options::in_memory_db() {
            connect_in_memory_store(&db_option).await?;
        } else {
            match init_surreal().await {
                Ok(_) => {
                    println!(
                        "数据库已经连接到 {}, 站点: {}",
                        db_option.project_name,
                        db_option.get_version_db_conn_str()
                    );
                }
                Err(e) => {
                    // 此前只 `dbg!` 一下就继续跑：连接失败被吞掉，进程一直走到
                    // 后面第一次真正用库的地方才炸，报错点漂移、根因难找。
                    // 连接是后续一切步骤的前提，失败就地退出。
                    return Err(anyhow!(
                        "SurrealDB 连接失败（{}）: {e:#}",
                        db_option.get_version_db_conn_str()
                    ));
                }
            }
        }
    }

    // ADR-050: this table is only a scheduler ledger for the current process.
    // Clear it after the database connection exists but before spatial loading,
    // preload, db-manager construction, watcher startup, or worker startup. A
    // cleanup error is fatal: continuing would replay a prior process snapshot.
    let cleared = crate::data_interface::model_update_pending::clear_stale_at_process_start()
        .await
        .map_err(|error| anyhow!("启动清理 model_update_pending 失败，拒绝继续：{error:#}"))?;
    println!("启动清理 model_update_pending 完成：删除 {cleared} 条历史工作单");

    // 启动分层判据装载空间树，与 run_app 同一失败语义（方案决策 D3）：树是可
    // 重建的派生数据，加载失败告警降级空树，不阻断启动；降级两态交给后台复检。
    if let Err(error) = crate::fast_model::aabb_tree::load_project_tree_verified().await {
        eprintln!("空间树启动加载失败（{error:#}），以空树启动，等待后台复检重建");
    }
    crate::fast_model::spatial_state::spawn_spatial_revalidator();
    // let (tx, mut rx) = mpsc::channel::<i32>();
    run_cli(db_option).await
}

pub mod admin;
pub mod data_state;
// pub mod data_to_excel;
// pub mod data_to_file;
// pub mod other_plat;
// pub mod pcf;
// pub mod plug_in;
// pub mod rvm;
// pub mod ssc;
pub mod version_management;

/// 启动全量房间重建那道门的源码钉子。
///
/// 刻意放在文件末尾：这两个测试用源码里的字面量当分隔符，而 `split_once` 取的是
/// **首次**出现。写在被测代码前面的话，分隔符会先匹配到测试自己那份拷贝，测的就
/// 成了测试自身。
#[cfg(test)]
mod startup_room_build_gate_tests {
    /// 启动路径必须**问**这道门，而不是自己判环境变量。
    ///
    /// 反向不变量：那一段里不许再直接出现旧的环境变量名。它此前是唯一的出口，
    /// 谁顺手在这里加回一个 `env::var` 分支，三道门的优先级就散了。
    #[test]
    fn the_startup_path_asks_the_gate_before_rebuilding_rooms() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once(r#"println!("房间关键字为: {:?}""#)
            .expect("启动房间段必须存在")
            .1
            .split_once("let aios_mgr = AiosDBMgr {")
            .expect("房间段之后是 aios_mgr")
            .0;

        assert!(
            body.contains("skip_startup_room_build().await"),
            "启动全量房间重建必须经这道门: {body}"
        );
        assert!(
            !body.contains("AIOS_SKIP_STARTUP_ROOM_BUILD"),
            "环境变量只在门里判一次，启动段不许再判: {body}"
        );
    }

    /// 三道门的优先级：止血口 > 冷启动开关 > 库侧对账。
    ///
    /// 顺序不能乱。`AIOS_SKIP_STARTUP_ROOM_BUILD` 排在最前是因为「跑增量」与
    /// 「跑 2 万面板级全量重建」是两件事——L3 夹具正是要前者不要后者；而对账
    /// 排在最后是因为它要读库，前两道门放行之前不该为它付这次查询。
    #[test]
    fn the_three_gates_keep_their_precedence() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("async fn skip_startup_room_build()")
            .expect("门必须存在")
            .1
            .split_once("\npub mod api;")
            .expect("门之后是模块声明")
            .0;

        let env_at = body
            .find("SKIP_STARTUP_ROOM_BUILD_ENV")
            .expect("止血口必须还在");
        let autorun_at = body
            .find("crate::options::startup_autorun()")
            .expect("冷启动开关必须把门");
        let reconcile_at = body
            .find("reconcile_startup_room_build().await")
            .expect("放行后必须与库侧凭据对账");
        assert!(
            env_at < autorun_at && autorun_at < reconcile_at,
            "三道门的优先级是 止血口 → 冷启动开关 → 库侧对账: {body}"
        );
    }

    /// ADR-048 的边界：监听限定域给批次与持久积压上弦，**不许**把启动全量房间
    /// 重建一起拖回来。
    ///
    /// 收窄到几个库的人要的正是「别为 2 万面板的全量重建付那十几秒」。这道门要是
    /// 改读 `is_auto_work_armed()` 或 `watch_scope`，写一句 `watch_dbnums = [8000]`
    /// 就会顺带把全量重建拉起来——而那与限定域的字面意思正好相反。
    #[test]
    fn the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("async fn skip_startup_room_build()")
            .expect("门必须存在")
            .1
            .split_once("\npub mod api;")
            .expect("门之后是模块声明")
            .0;

        assert!(
            !body.contains("watch_scope"),
            "启动全量房间重建这道门不得读监听限定域（ADR-048）: {body}"
        );
        assert!(
            !body.contains("is_auto_work_armed"),
            "启动全量房间重建这道门只认 startup_autorun，不许改读上弦位（ADR-048）: {body}"
        );
    }

    /// The MDB declaration has to be resolved while the manager is still owned
    /// — `init_mdb` takes `&mut self`, and one line later the manager is behind
    /// an `Arc` for the rest of the process. It also has to survive failing:
    /// nothing consumes the declaration yet, so a missing SYS database must not
    /// become a startup blocker.
    #[test]
    fn startup_resolves_the_mdb_before_the_manager_is_shared_and_survives_failure() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let resolved = body.find(".init_mdb(").expect("启动链必须解一次 MDB");
        let shared = body.find("Arc::new(manager)").expect("manager 随后进 Arc");
        assert!(
            resolved < shared,
            "init_mdb 要 &mut self，进 Arc 之后就没机会了"
        );
        assert!(
            body[resolved..shared].contains("log::warn!"),
            "解析失败只告警不中止：目前还没有消费者，阻断启动不成比例"
        );
    }

    #[test]
    fn startup_waits_for_data_then_models_before_rooms() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let worker = body.find("ensure_batch_worker(mgr.clone())").unwrap();
        assert!(
            body.contains("sync_pdms(&db_option).await?"),
            "任一全局数据阶段失败必须终止启动，不能继续打开模型门"
        );
        let web = body.find("serve_if_configured(mgr)").unwrap();
        let data = body.find("wait_for_data_ready().await").unwrap();
        let open_model = body.find("initialization.open_model_phase()").unwrap();
        // 等待包在带周期播报的 timeout 循环里，锚点只认调用本身，不认 `.await`。
        let model_ready = body.find("wait_for_model_ready()").unwrap();
        let room = body.find("build_room_relations(&db_option).await").unwrap();
        assert!(worker < data && web < data);
        assert!(data < open_model);
        assert!(open_model < model_ready && model_ready < room);
    }

    /// ADR-051: enabling model/mesh processing must not become an implicit
    /// whole-db rebuild on every service restart. Startup has one authority for
    /// deciding work: the watcher scan comparing file sessions with watermarks.
    #[test]
    fn startup_model_switches_do_not_bypass_incremental_comparison() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;

        assert!(
            body.contains("initialization.configure_model_bootstrap(false)"),
            "service startup must open the model phase for increment-produced work"
        );
        for forbidden in [
            "is_gen_mesh_or_model()",
            "gen_all_geos_data(&db_option)",
            "begin_full_model()",
        ] {
            assert!(
                !body.contains(forbidden),
                "service startup bypassed watermark comparison through {forbidden}"
            );
        }
    }
}
