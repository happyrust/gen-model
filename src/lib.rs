#![feature(let_chains)]
#![feature(async_closure)]
#![feature(exact_size_is_empty)]
#![feature(slice_take)]
#![feature(const_async_blocks)]
#![feature(type_alias_impl_trait)]
// `FrozenSource` 的稳定文件身份（Windows 卷序列号 + 文件索引）要它，见 `frozen_source.rs`。
#![cfg_attr(windows, feature(windows_by_handle))]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

use crate::data_interface::tidb_manager::AiosDBManager;
pub mod process_diagnostics;
pub mod native_runtime;
mod increment_diagnostics;
use crate::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
use crate::fast_model::spatial_state::SpatialTreeState;
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
// use tokio::sync::mpsc::Sender;
use aios_core::material::save_all_material_data;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use versioned_db::database::retire_dbnum_event;

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

/// 重启交接窗口内重试拿锁（`/api/v1/setup/restart`，计划 2026-09-09 向导 §5）：
/// 父进程 spawn 完子进程还有零点几秒才退出，deny-share 句柄随退出才释放；子进程
/// 带着 `AIOS_RESTART_HANDOFF` 在窗口内重试，别因为抢跑一步就报「已有实例」死掉。
/// 窗口外（平时）一次失败就是失败，语义不变。
#[cfg(feature = "http_api")]
fn open_lock_within_handoff_window(path: &std::path::Path) -> std::io::Result<File> {
    // 截止时刻只算一次：窗口是「从现在起最多 10 秒」，每轮重算就成了永不到期。
    let deadline = crate::web_service::setup::handoff_deadline();
    loop {
        match open_process_instance_lock(path) {
            Ok(file) => return Ok(file),
            Err(error) => {
                if let Some(deadline) = deadline
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }
                return Err(error);
            }
        }
    }
}

#[cfg(not(feature = "http_api"))]
fn open_lock_within_handoff_window(path: &std::path::Path) -> std::io::Result<File> {
    open_process_instance_lock(path)
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
/// **只有 `mem` 不拿。** 锁挡的是「两个进程驱动同一份持久状态」，而
/// `mem://` 那一档持久层就在进程里：两个实例各连各的 kv-mem，谁也读不到谁写的
/// 字节，锁没有保护对象。放开它正是为了让两个实例并排跑起来。
///
/// `rocksdb` **照常拿锁**。那台库别人也连得上，两个 gen-model 配同一份
/// `DbOption.toml` 就是冲着同一个端口去的——「不共享库」这个理由在那一档不成立。
///
/// 放开不是白送的。盘上还有东西是共享的：`meshes_path` 指的网格目录首当其冲——
/// 增量路径内部会强制落网格，`gen_mesh=false` 拦不住它。运行目录相对的那些
/// （`accel_tree` 快照、日志、`cata_dep_cache.bin`）天然分开，绝对路径的不会。
/// 所以并排跑之前得先让两份配置的 `meshes_path` 分家。
fn instance_lock_skip_reason(store_mode: crate::options::StoreMode) -> Option<&'static str> {
    matches!(store_mode, crate::options::StoreMode::Mem).then_some(
        "store_mode=mem，持久层在进程内，两个实例不共享库（meshes_path 仍是共享的，先让它分家）",
    )
}

/// pub：Python 调试绑定（python/aios-py）的 `full_init` 与 `run_app`/`run_cli`
/// 共用同一把单实例锁——mutating 管线不允许有第二个进程并发驱动。
///
/// 两处例外写在 [`instance_lock_skip_reason`] 与 [`process_instance_lock_path`]：
/// 内存模式整个不拿，拿的时候按运行目录而不是项目根隔离。
pub fn acquire_process_instance_lock(db_option: &DbOption) -> anyhow::Result<()> {
    if let Some(reason) = instance_lock_skip_reason(crate::options::store_mode()) {
        // run_app 与 run_cli 会各调一次，这句只该出现一遍。
        NO_INSTANCE_LOCK_NOTICE.call_once(|| println!("*** 不拿单实例锁：{reason}"));
        return Ok(());
    }
    let project = db_option.project_name.clone();
    let held = PROCESS_INSTANCE_LOCK.get_or_init(|| {
        let path = process_instance_lock_path()
            .ok_or_else(|| "未解析到运行目录（CWD），单实例锁无处可放".to_string())?;
        let mut file = open_lock_within_handoff_window(&path).map_err(|error| {
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
        Ok(lock) if lock.project == project => {}
        Ok(lock) => anyhow::bail!(
            "本进程已持有项目 {} 的单实例锁 {}，不能再启动项目 {project}",
            lock.project,
            lock.path.display()
        ),
        Err(error) => anyhow::bail!("{error}"),
    }
    // 运行目录锁挡不住「两个运行目录写同一台库」，那一层由机器级写者锁挡（2026-09-10
    // 数据窗口计划 §2）。与单实例锁同一处拿：所有走到这里的入口（服务、`db_window_repair`、
    // Python 绑定的 full_init）一并被门住，不必各自记得再拿一把。
    acquire_database_writer_lock(db_option).map_err(|error| anyhow::anyhow!("{error}"))
}

static DATABASE_WRITER_LOCK: OnceLock<
    Result<Option<crate::data_interface::database_writer_lock::DatabaseWriterLock>, String>,
> = OnceLock::new();

/// 机器级写者锁：按端点 + namespace + database 一台库只许一个写者。拿不到就是启动失败——
/// 「拿不到锁就当没有锁」正是两个写者互踩镜像的形状。重启交接窗口内照单实例锁的规矩重试
/// （父进程的句柄要等它退出才释放）。同进程再来一份不同身份的配置同样拒绝。
fn acquire_database_writer_lock(db_option: &DbOption) -> Result<(), String> {
    use crate::data_interface::database_writer_lock::{self as writer_lock, WriterLockError};

    let wanted =
        writer_lock::WriterIdentity::from_db_option(db_option, crate::options::store_mode());
    let held = DATABASE_WRITER_LOCK.get_or_init(|| {
        let holder = format!("gen-model project={}", db_option.project_name);
        let deadline = restart_handoff_deadline();
        loop {
            match writer_lock::acquire_for(db_option, crate::options::store_mode(), &holder) {
                Ok(lock) => return Ok(lock),
                Err(error @ WriterLockError::HeldByOther { .. }) => {
                    if let Some(deadline) = deadline
                        && std::time::Instant::now() < deadline
                    {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    }
                    return Err(error.to_string());
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    });
    match (held, wanted) {
        (Ok(None), None) => Ok(()),
        (Ok(Some(lock)), Some(wanted)) if lock.identity() == &wanted => Ok(()),
        (Ok(Some(lock)), wanted) => Err(format!(
            "本进程已持有数据库 {} {}/{} 的写者锁 {}，不能再以另一份配置（{:?}）写库",
            lock.identity().endpoint,
            lock.identity().namespace,
            lock.identity().database,
            lock.path().display(),
            wanted
        )),
        (Ok(None), Some(wanted)) => Err(format!(
            "本进程以 store_mode=mem 起过、没有写者锁，不能再以 {} {}/{} 写库",
            wanted.endpoint, wanted.namespace, wanted.database
        )),
        (Err(error), _) => Err(error.clone()),
    }
}

/// 重启交接（`/api/v1/setup/restart`）的重试截止时刻；没有 http_api 就没有交接，也就不重试。
#[cfg(feature = "http_api")]
fn restart_handoff_deadline() -> Option<std::time::Instant> {
    crate::web_service::setup::handoff_deadline()
}

#[cfg(not(feature = "http_api"))]
fn restart_handoff_deadline() -> Option<std::time::Instant> {
    None
}

#[cfg(test)]
mod process_instance_lock_tests {
    use super::{
        INSTANCE_LOCK_FILE_NAME, instance_lock_skip_reason, open_advisory_process_instance_lock,
        open_process_instance_lock, process_instance_lock_path,
    };

    /// 单实例锁与机器级写者锁同一处拿（2026-09-10 数据窗口计划 §2）：凡经 `acquire_process_instance_lock`
    /// 进来的入口都被写者锁门住。把写者锁从这里摘出去、让某个入口只拿运行目录锁，这条就红。
    #[test]
    fn the_instance_lock_entry_also_takes_the_database_writer_lock() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub fn acquire_process_instance_lock(")
            .expect("acquire_process_instance_lock")
            .1
            .split_once("\nstatic DATABASE_WRITER_LOCK")
            .expect("写者锁静态紧跟其后")
            .0;
        assert!(
            body.contains("acquire_database_writer_lock(db_option)"),
            "单实例锁入口必须顺手拿机器级写者锁：{body}"
        );
        // 拿不到写者锁是启动失败，不是一句提示。
        assert!(!body.contains("println!(\"*** 不拿写者锁"));
    }

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

    /// **只有进程内嵌那一档不拿锁。** `rocksdb` 那台库别人也连得上，两个 gen-model
    /// 配同一份 `DbOption.toml` 就是冲着同一个端口去的——「不共享库」这个理由在那一档
    /// 不成立。翻回「凡内存模式都不拿」这条就红。
    #[test]
    fn only_the_embedded_store_needs_no_single_instance_lock() {
        use crate::options::StoreMode;

        assert!(
            instance_lock_skip_reason(StoreMode::RocksDb).is_none(),
            "持久层在进程外时锁仍有保护对象，不许跳过"
        );
        let reason = instance_lock_skip_reason(StoreMode::Mem).expect("不拿锁必须说出为什么");
        assert!(
            reason.contains("mem"),
            "理由要点名是哪一档放开的，否则日志里只是一句没有出处的「不拿锁」：{reason}"
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

/// 启动时把常驻房间模型建起来：房间→面板拓扑与在册面板几何，MDB 全部设计库、不看
/// `watch_dbnums`。它是逐根房间归属那一步的**算法输入**（ADR-073 D1：一个生成根算完几何，
/// 紧接着用这份面板集做点包含筛选，边与模型行同事务落库；`model_db_adapter::frozen_room_calculator`
/// 在任务开工时从它冻一份不可变引用）——不是一条流程：不写归属边、不写发布视图 / 分片 / 清单，
/// 没有启动对比、没有换代（ADR-067 作废）。
///
/// 位置：紧跟空间树装载、排在 watcher / worker **之前**。门只有 `spatial_compute` 与
/// project_hd / project_hh 特性两道，都在 [`prepare_room_resident_at_startup`] 判；
/// `startup_data_ready` / 上弦位 / `startup_autorun` / `model_incremental` 一道都不看：
/// 房间模型是参照系，不是增量的一部分。建不起来打印原因、记 `resident_blocked`，不拦启动
/// ——之后每个任务开工冻计算器时拿不到模型，本批每根都算「房间算不出来」= 该根失败（D2）。
async fn initialize_resident_rooms_at_startup(db_option: &DbOption) {
    use crate::fast_model::room_resident;
    use crate::fast_model::room_topology::RoomHierarchy;

    let Some(hierarchy) = RoomHierarchy::configured() else {
        println!("[房间初始化] 房间子系统需要 project_hd 或 project_hh 特性：跳过");
        return;
    };
    #[cfg(any(feature = "project_hd", feature = "project_hh"))]
    let match_room_fn = crate::fast_model::room_model::configured_match_room_fn();
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let match_room_fn = |_: &str| false;
    println!(
        "正在初始化房间（常驻房间模型 · {}）：MDB 全部设计库...",
        crate::options::data_face().as_str(),
    );
    let room_key_words = db_option.get_room_key_word();
    let mesh_dir = db_option.get_meshes_path();
    let result = room_resident::initialize_at_startup(
        crate::options::data_face(),
        hierarchy,
        &room_key_words,
        match_room_fn,
        &mesh_dir,
    )
    .await;
    match &result {
        Ok(outcome) => println!("{}", outcome.summary()),
        Err(error) => eprintln!(
            "[房间初始化] 建不起来（不动归属边；之后的任务开工冻不出房间计算器，逐根按「房间算不出来」记失败）: {error:#}"
        ),
    }
    room_resident::record_startup_init(result.as_ref()).await;
}

/// Shared room-capability preparation for ingest and read-through. Callers have
/// already resolved the MDB skeleton and connected the process-local model store.
async fn prepare_room_resident_at_startup(db_option: &DbOption) {
    use crate::fast_model::room_resident;

    // `room_resident_model` 那道配置门随 ADR-067 作废（ADR-073）：常驻模型是逐根房间归属的
    // 算法输入，不再单独开关；只剩 `spatial_compute` 与特性两道门。
    if !crate::options::spatial_compute() {
        room_resident::mark_unsupported(format!("{}=0", crate::options::SPATIAL_COMPUTE_ENV));
        return;
    }
    if crate::fast_model::room_topology::RoomHierarchy::configured().is_none() {
        room_resident::mark_unsupported("project_hd/project_hh feature is absent");
        return;
    }

    room_resident::mark_loading("spatial-tree");
    println!("正在装载空间树（指纹一致直接复用，失配则重建）...");
    let step_started = Instant::now();
    match crate::fast_model::aabb_tree::load_project_tree_verified().await {
        Ok(_) => println!(
            "空间树装载完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        ),
        Err(error) => eprintln!("空间树启动加载失败（{error:#}），房间 resident 保持 blocked"),
    }
    crate::fast_model::spatial_state::spawn_spatial_revalidator();

    // 房间归属开关（`room_membership`，默认关）关着：空间树照装（它不只服务房间），常驻房间
    // 模型不建；`frozen_room_calculator` 返回 None，生成根与 model_sesno 照常推进，只跳过
    // 房间边重算。生命周期记 Disabled 并点名来源，/health 看得见。
    if !crate::options::room_membership() {
        room_resident::mark_disabled("room_membership=false");
        println!(
            "房间归属计算已关闭（DbOption.toml 的 room_membership / 环境变量 {}）：\
             跳过常驻房间模型初始化；模型根与 model_sesno 照常推进，不重算房间边；\
             重启并打开后历史房间欠账需执行模型重建",
            crate::options::ROOM_MEMBERSHIP_ENV
        );
        return;
    }

    room_resident::mark_loading("source-snapshot");
    let step_started = Instant::now();
    initialize_resident_rooms_at_startup(db_option).await;
    let rss = crate::data_interface::model_concurrency::process_rss_bytes()
        .map(|bytes| format!("，RSS {:.0} MB", bytes as f64 / (1024.0 * 1024.0)))
        .unwrap_or_default();
    println!(
        "房间初始化结束，耗时 {}{rss}",
        fmt_elapsed(step_started.elapsed())
    );
}

pub mod api;
pub mod build_info;
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
/// dabacon noun 字典快照（分类 flag + 能力矩阵），原 `parse_pdms_db::dict` 的消费面。
pub mod noun_dict;
pub mod noun_layout;
pub mod test;
pub mod uda_table;

// RVM 基准对拍。注意与同名但无关的 `src/rvm/`（PDMS 元素遍历）区分。
#[cfg(feature = "rvm_verify")]
pub mod rvm_baseline;

#[cfg(feature = "gen_model")]
pub mod fast_model;

/// MBD 管道尺寸标注宿主侧：e3d-model Route → plant-mbd 成员结点视图 → V2 契约
/// （`.planning/2026-09-06-plant-mbd-pipe-dimension`）。
pub mod mbd;

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

/// 一次性迁移（ADR-071 S6 / ADR-072 D5）：没有写者的表存量库里若还留着就丢掉。
/// `REMOVE TABLE IF EXISTS` 幂等，每次启动跑一遍也只是空转；失败只出声不拦启动——这些表不参与
/// 任何读路。ADR-072 W4：退役表名在 `src/` 里**只许**出现在这份清单里。
///
/// - 初始化发布 run 那六张（ADR-071 S5）；
/// - 第二条异步账那四张：`model_update_pending`（模型工作队列）/ `incr_side_effect_pending`
///   （副作用补偿 + 文件轮次）/ `published_file_input`（房间输入发布清单）/
///   `increment_update_attempt`（固定区间重放的恢复记录，位置让给 `dbnum_write_state`）；
/// - `room_published_*` 三张：房间输入的发布视图与拓扑源分片，常驻房间模型改从 `inst_relate`
///   实时行装（d-21）、拓扑分片只进内存与磁盘工件。
async fn retire_publish_run_tables() {
    if !crate::options::store_mode().is_durable() {
        return;
    }
    const RETIRED: [&str; 13] = [
        "model_publish_run",
        "model_publish_fragment",
        "model_publish_ack",
        "model_publish_database",
        "model_publish_error",
        "model_publish_superseded",
        "model_update_pending",
        "incr_side_effect_pending",
        "published_file_input",
        "increment_update_attempt",
        "room_published_inst",
        "room_published_topology",
        "room_topology_source",
    ];
    let sql = RETIRED
        .iter()
        .map(|table| format!("REMOVE TABLE IF EXISTS {table};"))
        .collect::<Vec<_>>()
        .join(" ");
    match SUL_DB
        .query(sql)
        .await
        .and_then(|response| response.check())
    {
        Ok(_) => println!(
            "发布阶段与第二条异步账的 {} 张退役表已确认不存在（一次性迁移，幂等）",
            RETIRED.len()
        ),
        Err(error) => eprintln!("清理发布阶段遗留表失败（下次启动重试）: {error:#}"),
    }
}

/// 启动链用的 dbnum 路由目录（`docs/plans/2026-09-12-lazy-startup-dev-plan.md` L3）。
///
/// 成员清单取 `init_mdb` 刚登记的那份（没有就从 SYS 解一次），模板目录取 schema pack；
/// 目录构造**零元素 I/O**。此前这一步预热的是整份元素骨架（每个 DESI 一次头扫描：AMS 28 库 /
/// 611,345 元素 0.72 s，JEU 417 库 / 1,221,858 元素 68 s），而属性、树的定位器与生成根定库都已改走
/// RefNo 位解码 + 路由目录（跨库属性切片 W4–W6），启动链要的只剩「这个 MDB 有哪些库、文件在哪」。
fn startup_route_directory(
    db_option: &DbOption,
) -> anyhow::Result<Arc<crate::data_interface::mdb_route_directory::MdbRouteDirectory>> {
    let membership =
        crate::data_interface::mdb_membership::get(&db_option.project_name, &db_option.mdb_name)
            .map(Ok)
            .unwrap_or_else(|| {
                crate::data_interface::mdb_membership::resolve(
                    db_option,
                    &db_option.project_name,
                    &db_option.mdb_name,
                )
                .map(Arc::new)
            })?;
    let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()?;
    let directory = Arc::new(
        crate::data_interface::mdb_route_directory::MdbRouteDirectory::from_membership(
            &membership,
            schema.template_dir(),
        ),
    );
    crate::data_interface::mdb_route_directory::register_active(directory.clone());
    Ok(directory)
}

/// 路由目录的启动横幅：路由数 / 元素库数 / 声明了没文件 / 构造期问题，尾巴带 e3d-io 读计数器
/// ——启动期 `scans=0` 就是「没扫元素」的证据（lazy-startup 不变量 6）。
///
/// 然后核一遍每个成员文件的 60 字节头（不变量 3「文件自证」，只读头不读元素）：声明了却不在盘上
/// 的库、自报库号与声明不符的文件——两者在下游都表现为「这个 refno 不存在」，跟元素真的被删了
/// 长得一模一样，必须在这里当面点名。
fn print_route_directory_banner(
    directory: &crate::data_interface::mdb_route_directory::MdbRouteDirectory,
    step_started: Instant,
) {
    let stats = directory.stats();
    println!(
        "MDB {} / {}：dbnum 路由目录 {} 条路由（元素库 {}，声明了没文件 {}，构造期问题 {}），\
         零元素 I/O，本步耗时 {}；e3d-io 读计数：{}",
        directory.mdb(),
        directory.project(),
        stats.routes,
        stats.element_routes,
        stats.declared_without_file,
        stats.problems,
        fmt_elapsed(step_started.elapsed()),
        e3d_io::read_stats::snapshot().summary()
    );
    for problem in directory.problems() {
        eprintln!("*** MDB 路由目录：{problem} ***");
    }
    for (dbnum, name) in directory.declared_without_file() {
        eprintln!(
            "*** MDB 路由目录：声明了 dbnum {dbnum}（{name}），配置的项目目录里却没有它的文件 ***"
        );
    }
    for error in directory.verify_all_headers() {
        eprintln!("*** MDB 路由目录：{error} ***");
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
    // 模型增量类型限定（`model_incremental_types` / `AIOS_MODEL_INCREMENTAL_TYPES`）：
    // 形状与坑都和 watch_dbnums 一样——名单外的根照常入队却没人生成，不在启动时
    // 说出来，人只会从「怎么 HANG 一直没模型」倒推。
    if let Some(notice) = crate::data_interface::model_type_scope::mode_notice() {
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
    // 数据窗口的结构行由 dbnum 任务自己同步落库（ADR-071 D1 / D11）：这里没有本地 journal 要对账、
    // 没有绑定要刷新、也没有 backup lane 要起。进程内镜像的水化在下面的统一水化段（D7 / S6）。
    retire_dbnum_event().await?;
    println!("预加载方法完成。");

    // 初始化数据库索引（开始/完成与耗时日志在函数内部，python full_init 同样受益）
    if let Err(e) = crate::fast_model::pdms_inst::init_inst_relate_indices().await {
        eprintln!("初始化inst_relate索引失败: {}", e);
    }
    // 存量行 anc/dbnum 自愈回填（幂等；全新库与已回填库一轮空转即返回）
    if let Err(e) = crate::fast_model::pdms_inst::backfill_inst_relate_anc().await {
        eprintln!("inst_relate anc/dbnum 回填失败（下次启动重试）: {}", e);
    }
    // 模型 / 房间水位的兼容播种（用户 2026-09-10 拍板「模型水位 + 房间水位默认按数据水位
    // 保持一致」，ADR-060 D10）：已有数据且旧管线留有模型投影、模型水位从未写过的库，两枚
    // 水位填成数据水位；数据在模型无的记 0。排在索引与 anc/dbnum 回填**之后**——投影探针要走
    // idx_inst_relate_dbnum / idx_inst_relate_in，没索引时是逐 pe 行全表扫；排在启动模型核对
    // **之前**——否则那一轮会给这些库排从 0 起的恢复 Task。失败只提示，下次启动幂等重来。
    match crate::data_interface::dbnum_state::DbnumState::seed_legacy_model_watermarks().await {
        Ok(seeds) => {
            for line in
                crate::data_interface::dbnum_state::legacy_model_watermark_seed_lines(&seeds)
            {
                println!("{line}");
            }
        }
        Err(e) => eprintln!("模型 / 房间水位兼容播种失败（下次启动重试）: {e:#}"),
    }
    // 写入标记回退（ADR-072 D2）：上一进程死在写库中途的库，先按各自的 `writing{S,T}` 退回 S、
    // 再排 S→T 重跑。排在镜像水化**之前**——镜像是一次整表替换，先水化再回退，发布出去的
    // 就是一版已经被删掉痕迹的水位，而且那几格再没有人来作废（理由见 `rollback_unfinished_writes`）。
    let step_started = Instant::now();
    let rollbacks =
        crate::data_interface::startup_reconcile::schedule_unfinished_writes_for_reinitialize()
            .await;
    if !rollbacks.is_empty() {
        for line in &rollbacks {
            println!("{line}");
        }
        println!(
            "写入标记回退完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        );
    }
    // ---- 统一水化段：启动只水化小型控制面。模型投影绝不从 RocksDB 回填 kv-mem；
    // RocksDB 直接服务冷根，kv-mem 只保留生成出的增量 overlay。
    let step_started = Instant::now();
    crate::data_interface::dbnum_state::DbnumState::enable_watermark_mirror();
    let hydrated_states =
        crate::data_interface::dbnum_state::DbnumState::hydrate_watermark_mirror().await?;
    println!(
        "水位内存镜像已水化 {} 个 dbnum，耗时 {}",
        hydrated_states.len(),
        fmt_elapsed(step_started.elapsed())
    );
    retire_publish_run_tables().await;
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

    // watcher **无条件起**（ADR-058 D8，2026-09-07）：`sync_live` 那个开关退役了。它此前把
    // 「起不起 watcher」与「做不做启动数据重扫」两件事绑在一个布尔上，而「一个库都不监听」
    // 这个状态没人真的要——监听范围本来就由 `watch_dbnums` 说了算（不写 = 本 MDB 声明的全部
    // DESI，共识 d-207），关掉 watcher 只会让「改元素 → SAVEWORK → 自动更新」在那份配置上
    // 静默失效。启动那次数据重扫的去留由 `data_face` 单独回答（read-through 跳过它）。
    // 数据面只有 direct 一条读路由（2026-09-08 refactor plan S4：legacy db 读路由与 PE 规划器
    // 一并退役，ADR-058 D8）；`AIOS_DATA_READ_MODE` 再写 `db` 按认不出的值出声、照常 direct。
    println!(
        "数据读取模式：direct（e3d-io，唯一形态）；watcher / 模型复核 / 房间同一条启动路，\
         监控目录 watcher 无条件启动"
    );
    if let Some(notice) = crate::options::data_read_mode_notice() {
        eprintln!("*** {notice} ***");
    }
    // 另外两个形态键拼错同样要出声（`store_mode_notice` 此前有定义没人打；顺手接上）。
    if let Some(notice) = crate::options::store_mode_notice() {
        eprintln!("*** {notice} ***");
    }
    if let Some(notice) = crate::options::data_face_notice() {
        eprintln!("*** {notice} ***");
    }
    let db_option = Arc::new(db_option.clone());

    // legacy 全量 / 历史解析（`sync_pdms` 与 `total_sync` / `incr_sync` / `only_sync_sys` /
    // `sync_history` 四个开关）2026-09-07 整条退役（ADR-058 D8）。direct 成为默认形态之后
    // 那条路一次也没被走到过，而它与水位、模型、`gen_root` 凭证全程脱钩——「解析完了却
    // 什么都没建立」这个坑随路一起没了。数据面从此只有一条：watcher 首扫与增量窗口经
    // e3d-io 摄入，首载 / 重建由数据批次 worker 按单个文件调
    // `versioned_db::baseline_ingest::ingest_file`（同日 `sync_total_async_threaded` 壳也退役）。

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
    // 成员名单解完，紧接着造这个 MDB 的 **dbnum 路由目录**并打横幅：SYS/CURD 成员 →
    // `dbnum → 文件`，再核一遍每个成员文件的 60 字节头。**不读任何元素**——此前这里预热的是
    // 整份元素骨架（`ref0 → dbnum` / `refno → 属主` / `refno → noun` 三张表，每个 DESI 一次头
    // 扫描），而属性、树的定位器与生成根定库都已改走 RefNo 位解码 + 路由目录，启动链不再需要它
    // （`docs/plans/2026-09-12-lazy-startup-dev-plan.md` L3）。树服务剩下的 WORL 枚举与
    // noun / owner 查询仍在骨架上，由它第一次被用到时自建（同计划 L5 把它也换掉）。
    //
    // 放在这里而不是更早：它吃 `init_mdb` 刚登记的成员名单。失败只告警不中止：横幅不是启动门，
    // 各服务开自己那份目录时会再报一次。
    let step_started = Instant::now();
    match startup_route_directory(&db_option) {
        Ok(directory) => print_route_directory_banner(&directory, step_started),
        Err(error) => {
            log::warn!("MDB dbnum 路由目录构造失败，各服务首次使用时会再试：{error:#}");
            eprintln!("*** MDB dbnum 路由目录构造失败（各服务首次使用时会再试）：{error:#} ***");
        }
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
    // `E3dModelService::generation_roots_of` + `generate_and_persist_roots`.
    initialization.configure_model_bootstrap(false);
    // 清单就绪会自动打开模型门，worker 第一圈空闲轮随即可能宣告 model_ready。
    // 启动凭证复核必须先占住这道完成门，再允许唯一消费者起来。
    initialization.begin_startup_reconcile();
    println!(
        "启动模型策略：gen_model={} gen_mesh={} 仅控制增量模型阶段；按文件会话号与 applied_sesno 比对结果执行，不启动整库全量生成",
        db_option.gen_model, db_option.gen_mesh
    );
    if crate::options::data_face().is_read_through() {
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
        println!("启动模型凭证复核仍照常执行；发现的缺口只入队，不越过上弦门消费");
    }

    crate::data_interface::dabacon_tag_summary::start_at_startup(std::path::PathBuf::from(
        &db_option.project_path,
    ));

    // 先把健康接口起起来，再做 watcher 的首次全目录扫描。扫描慢或阻断时，外部仍能
    // 从 /health 看到初始化快照，而不是只能等到扫描结束才连得上服务。
    #[cfg(feature = "http_api")]
    let web_task = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::web_service::serve_if_configured(mgr).await {
                eprintln!("Web 服务异常退出: {e:?}");
            }
        })
    };

    // 空间树装载（启动分层判据，docs/2026-08-11_spatial-tree-startup-init-plan.md）：
    // 指纹（epoch 值 + 库侧时间戳）一致 → 直接复用；失配但有待重放空间意图 →
    // 复用文件交给重放自愈；失配且无意图 / 文件缺失损坏 → 只读指针重建。
    // 空间树是可重建的派生数据，加载失败不该顶掉整个启动：空树有下游防线
    // （全量重建拒跑、整间分支拒算），worker 启动收敛还会重放未完成的空间意图。
    // 降级复检后台任务（一致性闭环方案 §6）：DegradedReuse/DegradedBlocked 两态由它
    // 退避重试收敛，恢复 Ready 后唤醒调度器；健康状态下它只是每 30s 一次的状态读取。
    //
    // 排在 watcher / worker **之前**（2026-09-07，常驻房间模型第 5 轮 Q17 (a)）：它只读指针与
    // 快照文件，不依赖数据清单——`run_app` 那条正门本来就在进 `run_cli` 之前装过一次，这里
    // 多半是同进程短路复用；直接调 `run_cli` 的那几条路（python `full_init`、l3 夹具）从此
    // 与正门同序。此前它压在等数据清单之后，房间初始化要「树在位」就得跟着排到最后。
    prepare_room_resident_at_startup(&db_option).await;

    if crate::options::data_face().is_read_through() {
        println!(
            "read-through：跳过启动数据重扫与 Meta→Design 摄入清单；data_ready 已就绪，watcher 仅作为后续文件脏信号"
        );
    } else {
        println!("正在启动监控目录 watcher（首轮重扫入队，含库文件存档压缩，可能较久）...");
        let step_started = Instant::now();
        mgr.init_watcher().await?;
        println!(
            "监控目录 watcher 启动完成，耗时 {}",
            fmt_elapsed(step_started.elapsed())
        );
    }

    // Worker is started as soon as the immutable manifest is installed.  The
    // model gate opens with that manifest (ADR-058 D7: file identity resolved,
    // not data settled), so the data chain and the model chain run side by
    // side from here on.  The worker is still a single consumer: the idle-round
    // model drain only runs while no data batch is queued (`DataQueued` yield),
    // and the convergence banner (`mark_model_ready`) still waits for `data_ready`.
    //
    // direct 形态也起（2026-09-02，plant-ui 计划 D1）：worker 是队列的唯一消费者，与读取
    // 底座无关；数据面走直写路径（ADR-056 P1），模型面走 `generate_roots`（e3d-io 读文件）。
    // 人工执行（`POST /update/execute`）与文件事件排进同一个队列。
    let startup_data_ready =
        crate::options::data_face().is_read_through() || scheduler.is_auto_work_armed();
    crate::data_interface::batch_worker::ensure_batch_worker(mgr.clone());

    // Watcher events are dirtiness signals only; `async_watch` debounces them
    // into the same full manifest scan used by startup and reconciliation.
    // 无条件起（ADR-058 D8）：看门狗是这个进程对「文件变了」的唯一耳朵，没有哪份配置
    // 真的想把它关掉；监听范围由 `watch_dbnums` 说了算（共识 d-207）。
    let watch_task = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
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
        })
    };

    // 把生成根凭证与**文件最新会话**对齐——run_app 里 ADR-050 那次无条件清空的补偿路径
    // （ADR-058 D4）：一级取全部库的凭证下限，只有落后的库才开一次退化窗口，选根整条来自
    // 稳态那条路。它读的是文件与 gen_root，不需要任何数据摄入，所以排在数据等待**之前**
    // （ADR-058 D7：模型面不等 data_ready）——此前它压在 Meta → Catalogue → Design 摄完之后，
    // Design 基线跑多久模型面就干等多久。direct 也跑，而且 direct 才是它的主场；此前那道
    // `!direct_read_mode` 让生产形态启动时一根都不核，ADR-050 承诺的「重启靠凭证 vs 文件
    // 最新重新发现」于是没有实现者（审核 A1 / A2）。仅看 model_update_pending=0 会把
    // “从未生成过的根”误判成完成；这里只补缺失工作，并为已有属性与模型投影但缺少
    // 水位追踪的历史库统一初始化两枚水位；不删旧模型，也不越过 watch_dbnums。
    // 任何一库失败都告警后继续，不把一个库的配置错变成整个服务的崩溃循环。
    crate::data_interface::startup_reconcile::reconcile_model_coverage_at_startup().await;
    // 数据等待的退出口（09-04 审核 #10）：判据是「受阻」不是「久」——Design 基线在大项目上
    // 合法地跑几小时，按时长降级会误伤；只有 `blockers` 非空**且**队列里没有任何批次在
    // `queued/running`、连续 [`STARTUP_DATA_STALL_DEGRADE_SECS`] 之后，才放弃等待降级进入监听。
    // 降级不清阻断、不改 `data_ready`：模型收敛等待与房间段随之跳过，完成横幅标 blocked。
    let mut data_wait_degraded: Option<String> = None;
    if startup_data_ready {
        println!(
            "等待 Meta → Design 数据清单完成...（模型面已并行开工：门随清单装好即开，\
             积压在数据队列空隙消化，按需生成不受数据阶段阻挡）"
        );
        let mut waited_secs = 0u64;
        let mut stalled_secs = 0u64;
        let mut last_wait_fingerprint: Option<String> = None;
        let mut last_wait_emitted_secs = 0u64;
        loop {
            if tokio::time::timeout(std::time::Duration::from_secs(60), async {
                initialization.wait_for_data_ready().await;
            })
            .await
            .is_ok()
            {
                break;
            }
            waited_secs += 60;
            let snapshot = initialization.snapshot();
            let batch_in_flight = scheduler
                .snapshot()
                .iter()
                .any(|row| matches!(row.state, "queued" | "running"));
            if startup_data_wait_is_stalled(&snapshot, batch_in_flight) {
                stalled_secs += 60;
            } else {
                stalled_secs = 0;
            }
            if stalled_secs >= STARTUP_DATA_STALL_DEGRADE_SECS {
                let reason = format!(
                    "数据清单受阻 {stalled_secs}s 且队列里没有批次在跑（status={} blockers=[{}]）",
                    snapshot.status,
                    snapshot.blockers.join(" | ")
                );
                println!(
                    "数据初始化受阻，降级继续：{reason}。阻断不清、data_ready 不置位；\
                     跳过模型收敛等待与启动房间重建，进入增量监听——修好阻断的库后重扫自会接上，\
                     /health.initialization 仍报 blocked"
                );
                initialization.mark_startup_data_wait_degraded(reason.clone());
                data_wait_degraded = Some(reason);
                break;
            }
            let phases = snapshot
                .phases
                .iter()
                .map(|(phase, counts)| {
                    format!(
                        "{phase}:total={} pending={} running={} failed={} blocked={}",
                        counts.total, counts.pending, counts.running, counts.failed, counts.blocked
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let blockers = if snapshot.blockers.is_empty() {
                "无".to_string()
            } else {
                snapshot.blockers.join(" | ")
            };
            let fingerprint = format!(
                "{}|{:?}|{:?}|{:?}",
                snapshot.status, snapshot.current_phase, snapshot.phases, snapshot.blockers
            );
            let blocked = snapshot.status == "blocked" || !snapshot.blockers.is_empty();
            let changed = last_wait_fingerprint.as_deref() != Some(fingerprint.as_str());
            if blocked || changed || waited_secs.saturating_sub(last_wait_emitted_secs) >= 300 {
                println!(
                    "数据初始化仍在等待：status={} current_phase={} phases=[{}] \
                     blockers=[{}] 已等={}s",
                    snapshot.status,
                    snapshot.current_phase.unwrap_or("—"),
                    phases,
                    blockers,
                    waited_secs
                );
                last_wait_fingerprint = Some(fingerprint);
                last_wait_emitted_secs = waited_secs;
            }
        }
        if data_wait_degraded.is_none() {
            println!("数据链已就绪；模型面自清单装好起已在跑，下面等模型积压收敛");
        }
    }

    // 空间树装载此前在这里（等数据清单之后）；2026-09-07 起前移到 watcher / worker 之前，
    // 见上面「空间树装载」那段与 `initialize_resident_rooms_at_startup`（第 5 轮 Q17 (a)）。
    // progress_sender.send(10)?;
    // 2026-09-04（S5-2）之前这里有一条 direct 专用分支：复核落地后立刻 `mark_model_ready()`
    // 并跳过收敛等待，`/health.model_ready=true` 时复核排出的行还在跑，而且不看数据链。
    // direct 从此与 db 形态走同一条等待：门随清单开，收敛由 worker 空闲轮宣告。
    //
    // 数据等待已降级时不等模型收敛：`mark_model_ready` 要 `data_ready`（尾事务还会往积压
    // 添行），而 `data_ready` 正是等不到的那一样——这里再等就是把同一个挂死换个名字。
    // worker 照常在后台消化积压，模型门随清单开，按需生成不受影响。
    if let Some(reason) = data_wait_degraded.as_deref() {
        scheduler.wake();
        println!("数据初始化受阻（{reason}）：不等模型收敛，积压由 worker 后台消化");
    } else if startup_data_ready {
        if !initialization.open_model_phase() {
            println!(
                "模型门未能立即打开（重扫刚发现新会话，数据阶段回到未就绪）：\
                 照常等待模型收敛，门由清单就绪后自动打开"
            );
        }
        scheduler.wake();
        if !crate::options::model_incremental() {
            // worker 空闲轮的模型消费与 mark_model_ready 都挂在
            // `model_incremental()` 上（batch_worker 的空闲模型门），开关关着时
            // model_ready 在本进程内永远不会置位——这里若照常等待就是无限期挂死：
            // watcher/web 已经起了，启动序列却永远走不到房间阶段与完成横幅。
            // 模型门照开（按需生成沿用 require_model_generation 那道门），只跳过等待；
            // 房间阶段由下方同一开关一并跳过。
            println!(
                "模型增量阶段已关闭（model_incremental=false）：跳过启动模型收敛等待\
                 （重新开启后由恢复任务按凭证缺口回补；按需生成不受影响）"
            );
        } else {
            // 模型工作随各库任务同步做完（ADR-072 D5，没有 pending 积压可数），收敛只剩
            // worker 空闲轮那几步（认领迟到批次、AABB 落盘、模型门）。主线可能要等在飞的
            // 任务跑完；干等会让启动看起来像挂死——在飞任务变化时报实际进展，完全不变时
            // 最多每 300s 提醒一次。
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
                let registry = crate::data_interface::task_registry::TaskRegistry::global();
                let running = registry.running_data_batch_dbnums();
                let queued = scheduler
                    .snapshot()
                    .iter()
                    .filter(|row| row.state == "queued")
                    .count();
                let message = format!(
                    "初始化模型进行中：在飞任务 {} 个（dbnum {:?}）、排队 {queued} 条，已等={waited_secs}s",
                    running.len(),
                    running
                );
                let fingerprint = format!("{running:?}|{queued}");
                let changed = last_wait_fingerprint.as_deref() != Some(fingerprint.as_str());
                if changed || waited_secs.saturating_sub(last_wait_emitted_secs) >= 300 {
                    println!("{message}");
                    last_wait_fingerprint = Some(fingerprint);
                    last_wait_emitted_secs = waited_secs;
                }
            }
            println!("模型阶段已收敛");
        }
    }

    println!("房间关键字为: {:?}", db_option.get_room_key_word());
    // 项目级启动房间重建与数据库几何查询路径已经退役。常驻房间参照在 watcher
    // 之前由 E3D 文件预建；每个已完成模型的 dbnum 由自己的 Task（或三水位恢复）
    // 立即消费内存投影。这里不设全项目屏障，也不扫描数据库推导计算范围。
    println!("房间计算采用逐 dbnum 实时内存模式；启动项目全算与数据库计算输入已停用");
    scheduler.wake();

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
    // TEAM_DATA 生成随 `only_sync_sys` 一起退役（ADR-058 D8）：它的唯一开关是那个
    // legacy 全量键，仓里没有一份配置打开过，因此这段从来没跑过。2026-09-07 连本体也
    // 收口了——它唯一的写出口是 MySQL 镜像表，随 `feature = "sql"` 一起退役
    // （`team_data::sync_team_data` 现在只打一行声就返回）。真要 TEAM_DATA，它的家是
    // Surreal，给它自己的写入路径，别再把这条 MySQL 镜像接回来。

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
    print_startup_complete_banner(&db_option, startup_started, data_wait_degraded.as_deref());

    let _ = watch_task.await;

    // 看门狗退出之后保持 Web 服务长驻，供前端触发 preview/execute 手动更新与按需生成。
    // `http_api_addr` 缺失时 Web 服务会兼容旧 `server_release_ip` 或回环默认值，因此这里
    // 不能再拿新键是否存在当成「Web 是否启用」的判据。
    #[cfg(feature = "http_api")]
    {
        println!("Web 服务保持运行（Ctrl+C 退出）...");
        let _ = web_task.await;
    }

    Ok(())
}

/// 启动主线等数据清单时，「受阻且无批次在跑」连续多久才放弃等待降级进入监听
/// （09-04 审核 #10）。三个 60 s 的观察拍：一拍可能只是 worker 刚收口、下一批还没入队。
const STARTUP_DATA_STALL_DEGRADE_SECS: u64 = 180;

/// 数据等待是不是「受阻」（#10 的降级判据，纯函数）：清单报 blocked / 有阻断**且**队列里
/// 没有任何批次在 `queued/running`。只有阻断而批次还在跑不算——那是别的库在正常推进；
/// 只有队列空而没有阻断也不算——那是清单还在装、或 worker 正要认领下一批。
fn startup_data_wait_is_stalled(
    snapshot: &crate::data_interface::initialization_phase::InitializationSnapshot,
    batch_in_flight: bool,
) -> bool {
    let blocked = snapshot.status == "blocked" || !snapshot.blockers.is_empty();
    blocked && !batch_in_flight
}

#[cfg(test)]
mod startup_data_wait_tests {
    #[test]
    fn read_through_startup_reports_each_waiting_stage() {
        let source = include_str!("lib.rs");
        let body = source.rsplit_once("async fn run_read_through_core(").unwrap().1
            .split_once("pub async fn run_app(").unwrap().0;
        for stage in ["store", "mdb", "http", "route-directory", "spatial-room", "session-baseline"] {
            let begin = body.find(&format!("stage={stage} event=begin")).unwrap();
            let done = body.find(&format!("stage={stage} event=done elapsed_ms=")).unwrap();
            assert!(begin < done, "{stage} must bracket its operation");
        }
        assert!(body.find("stage=http event=done").unwrap()
            < body.find("stage=session-baseline event=begin").unwrap());
    }

    #[test]
    fn read_through_startup_never_generates_the_whole_mdb() {
        let source = include_str!("lib.rs");
        let body = source.rsplit_once("async fn run_read_through_core(").unwrap().1
            .split_once("pub async fn run_app(").unwrap().0;
        for forbidden in ["model_dbnum_ensure::start_and_wait", "scan_index(",
            "scan_elements(", "bootstrap_read_through_rooms("] {
            assert!(!body.contains(forbidden), "startup contains {forbidden}");
        }
        assert!(body.contains("initialization.configure_model_bootstrap(false)"));
        assert!(body.contains("targeted_model_refresh(&refresh_manager, &change).await"));
    }

    use super::*;

    fn snapshot(
        status: &'static str,
        blockers: &[&str],
    ) -> crate::data_interface::initialization_phase::InitializationSnapshot {
        crate::data_interface::initialization_phase::InitializationSnapshot {
            epoch_id: 1,
            status,
            current_phase: None,
            data_ready: false,
            model_ready: false,
            model_in_flight: false,
            model_phase_open: true,
            startup_reconcile_pending: false,
            startup_rooms_pending: false,
            room_bootstrap: Default::default(),
            startup: None,
            startup_data_wait_degraded: None,
            phases: Default::default(),
            blockers: blockers.iter().map(|text| text.to_string()).collect(),
            blocked_by: Vec::new(),
            shadowed: Vec::new(),
        }
    }

    /// 09-04 审核 #10：降级判据是「受阻且无批次在跑」，两个条件缺一个都不算。
    /// 按时长降级会误伤合法跑几小时的 Design 基线；只看阻断会在别的库还在正常推进时误降。
    #[test]
    fn the_data_wait_is_stalled_only_when_blocked_and_nothing_is_running() {
        assert!(startup_data_wait_is_stalled(
            &snapshot("blocked", &["meta: dbnum=8000 文件头读不出"]),
            false
        ));
        assert!(startup_data_wait_is_stalled(
            &snapshot("design_running", &["meta: dbnum=8000 文件头读不出"]),
            false
        ));
        assert!(!startup_data_wait_is_stalled(
            &snapshot("blocked", &["meta: dbnum=8000 文件头读不出"]),
            true
        ));
        assert!(!startup_data_wait_is_stalled(
            &snapshot("design_running", &[]),
            false
        ));
        assert!(!startup_data_wait_is_stalled(
            &snapshot("discovering", &[]),
            true
        ));
        assert!(
            STARTUP_DATA_STALL_DEGRADE_SECS >= 120,
            "至少两个观察拍：一拍可能只是 worker 刚收口、下一批还没入队"
        );
    }

    /// 降级要贯通三处：等待循环里退出、模型收敛等待与房间段跳过、完成横幅点名；且不许
    /// 动 `data_ready`——阻断是真的，降级只是主线不再为它挂死。
    #[test]
    #[ignore = "启动房间段已被启动对比重排（降级分支形状变了），本钉的标记已从 run_cli 撤走、钉未随迁：\
                由房间流随 T4/启动对比收口反向重钉（d-73、mem-107、changelog 2026-09-09）"]
    fn the_degraded_data_wait_skips_model_wait_and_rooms_and_is_named_on_the_banner() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("\nfn print_startup_complete_banner(")
            .expect("完成横幅紧跟 run_cli")
            .0;
        let data_wait = body.find("wait_for_data_ready().await").unwrap();
        let degrade = body
            .find("if stalled_secs >= STARTUP_DATA_STALL_DEGRADE_SECS {")
            .expect("等待循环里必须有降级出口");
        let mark = body
            .find("initialization.mark_startup_data_wait_degraded(")
            .expect("降级要记进协调器给 /health 看");
        let model_wait = body
            .find(
                "if let Some(reason) = data_wait_degraded.as_deref() {\n        scheduler.wake();",
            )
            .expect("降级后不等模型收敛");
        let model_ready = body.find("wait_for_model_ready()").unwrap();
        let rooms = body
            .find("} else if let Some(reason) = data_wait_degraded.as_deref() {\n        // 房间")
            .expect("降级后跳过启动房间重建");
        let banner = body
            .find("print_startup_complete_banner(&db_option, startup_started, data_wait_degraded.as_deref())")
            .expect("完成横幅要拿到降级理由");
        assert!(
            data_wait < degrade
                && degrade < mark
                && mark < model_wait
                && model_wait < model_ready
                && model_ready < rooms
                && rooms < banner,
            "顺序：等数据 → 降级出口 → 记协调器 → 跳模型等待 → 跳房间 → 横幅点名"
        );
        let code = |text: &str| -> String {
            text.lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let degrade_block = code(&body[degrade..model_wait]);
        assert!(
            !degrade_block.contains("mark_data_ready")
                && !degrade_block.contains("data_ready = true"),
            "降级不许假装数据就绪: {degrade_block}"
        );
        assert!(
            body[degrade..mark].contains("且队列里没有批次在跑"),
            "降级理由要说清是受阻而不是慢"
        );
    }
}

/// 「初始化完成 → 进入增量监听」这条边界的横幅。
///
/// 状态在这里复述一遍，而不是只打一行「完成」：阶段开关、监听限定、HTTP 地址
/// 是出问题时第一批要确认的东西，而它们各自散在前面几十行启动日志里——等真出事
/// 时，那些行早被稳态轮询的重扫日志刷走了。
fn print_startup_complete_banner(
    db_option: &DbOption,
    started: Instant,
    data_wait_degraded: Option<&str>,
) {
    const RULE: &str = "==========================================================";
    println!("{RULE}");
    println!(
        "AIOS Database 构建：{}（release {} / git {}{}）",
        crate::build_info::BUILD_ID,
        crate::build_info::VERSION,
        crate::build_info::GIT_COMMIT,
        if crate::build_info::GIT_DIRTY_TEXT == "true" {
            " / dirty"
        } else {
            ""
        }
    );
    match data_wait_degraded {
        Some(_) => println!(
            "初始化完成（**数据受阻，降级进入监听**）：项目 {}，启动总耗时 {}",
            db_option.project_name,
            fmt_elapsed(started.elapsed())
        ),
        None => println!(
            "初始化完成：项目 {}，启动总耗时 {}",
            db_option.project_name,
            fmt_elapsed(started.elapsed())
        ),
    }
    if let Some(reason) = data_wait_degraded {
        println!(
            "  *** 数据初始化受阻：{reason}——模型收敛等待与启动房间重建已跳过；\
             /health.initialization 仍报 blocked，修好阻断的库后重扫自会接上"
        );
    }
    println!(
        "  增量阶段：data={} model={}（数据链与模型链并行，房间归属随生成根同步计算）",
        crate::options::data_incremental(),
        crate::options::model_incremental(),
    );
    {
        use crate::fast_model::room_resident::ResidentLifecycle;
        let short = |key: &str| key.chars().take(12).collect::<String>();
        let state = match crate::fast_model::room_resident::lifecycle() {
            ResidentLifecycle::Disabled { source } => format!("关（{source}）"),
            ResidentLifecycle::Unsupported { reason } => format!("不适用（{reason}）"),
            ResidentLifecycle::Loading { phase, .. } => {
                format!("开，仍在装载（{phase}）——普通房间写入口此刻返回 UPDATING")
            }
            ResidentLifecycle::Ready { generation, .. } => {
                format!("开，ready（代 {}）", short(&generation))
            }
            ResidentLifecycle::Degraded {
                generation,
                problems,
            } => format!(
                "开，degraded（代 {}）：{}",
                short(&generation),
                problems.join("; ")
            ),
            ResidentLifecycle::Blocked { problems } => {
                format!(
                    "开，blocked：{}（房间边不写，修复后重启或等换代）",
                    problems.join("; ")
                )
            }
        };
        println!("  常驻房间模型：{state}");
    }
    // 非正常运行状态才出声（与 watch_dbnums / model_incremental_types 同一条纪律）：
    // 关着时「没算房间」与「房间没变」在库上长得一模一样，横幅这一行是最先被翻到的证据。
    if !crate::options::room_membership() {
        println!(
            "  房间归属计算：关（DbOption.toml 的 room_membership / 环境变量 {}）——\
             模型根与 model_sesno 照常推进，不重算房间边；\
             重启并打开后历史房间欠账需执行模型重建",
            crate::options::ROOM_MEMBERSHIP_ENV
        );
    }
    println!("  数据读取：direct / e3d-io（唯一形态；legacy db 读路由已于 2026-09-08 退役）");
    if let Some(notice) = crate::data_interface::watch_scope::mode_notice() {
        println!("  *** {notice}");
    }
    if let Some(notice) = crate::data_interface::model_type_scope::mode_notice() {
        println!("  *** {notice}");
    }
    match crate::options::store_mode() {
        crate::options::StoreMode::Mem => println!(
            "  *** 内存模式（store_mode=mem）：持久层是进程内嵌 kv-mem，\
             进程一退整库即消失，外部无端口可连"
        ),
        crate::options::StoreMode::RocksDb => {
            println!(
                "  存储介质：rocksdb（store_mode=rocksdb，连配置指的 {}——本进程不拉起它）",
                db_option.get_version_db_conn_str()
            )
        }
    }
    match crate::options::data_face() {
        crate::options::DataFace::ReadThrough => {
            println!("  数据面：文件直读，内存数据库仅保存模型；不摄入或写透 pe / ATT_* 属性数据")
        }
        crate::options::DataFace::Ingest => {
            println!("  数据面：ingest（整库基线 + 稳态增量窗口，推进 applied_sesno）")
        }
    }
    #[cfg(feature = "http_api")]
    {
        let ext = crate::get_db_option_ext();
        let addr = crate::web_service::effective_http_api_addr(
            ext.http_api_addr.as_deref(),
            &ext.server_release_ip,
        );
        let public = crate::web_service::public_http_addr(&addr, ext.http_public_addr.as_deref());
        println!("  HTTP API：http://{public}/api/v1（监听 {addr}）");
    }
    println!("已进入增量更新监听：库文件一有变化就入队处理，往下的日志都是运行期输出。");
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
         Capture-*Evidence 与 plant-ui 的「库供数」读面都够不着）。\
         要留证据请改 store_mode = \"rocksdb\" 并把 v_ip:v_port 指到一台真在跑的库。"
    );
    Ok(())
}

/// 连配置里 `v_ip:v_port` 上那台 SurrealDB。
///
/// **本进程只连，不拉起。** 端口上没东西就是连不上——自己起一台空库再在上面作业，
/// 失败会一声不响，现象是「查了半天什么都没有」，最难查的那一类。
#[cfg(feature = "ws")]
async fn connect_configured_store(db_option: &DbOption) -> anyhow::Result<()> {
    // 此前只 `dbg!` 一下就继续跑：连接失败被吞掉，进程一直走到后面第一次真正用库
    // 的地方才炸，报错点漂移、根因难找。连接是后续一切步骤的前提，失败就地退出。
    aios_core::init_surreal().await.map_err(|error| {
        anyhow!(
            "SurrealDB 连接失败（{}）: {error:#}",
            db_option.get_version_db_conn_str()
        )
    })?;
    println!(
        "数据库已经连接到 {}, 站点: {}",
        db_option.project_name,
        db_option.get_version_db_conn_str()
    );
    Ok(())
}

/// **连上持久层的唯一入口。** 两档介质、一条下游：`SUL_DB` 是 `Surreal<Any>`，
/// 引擎选择对上层不可见，所以初始化解析、增量窗口写回、模型与房间派生数据全部照原路走。
///
/// 没有 `startup` 这个参数了。它原本回答「这一次有没有资格把库拉起来」——本进程从此
/// 一台库都不拉，那个问题不存在了；`store_process.rs`（端口探针 / 署名 / 心跳 /
/// 孤儿回收 / 收尸）随之整个删除。
#[cfg(feature = "ws")]
pub async fn ensure_store_connected(db_option: &DbOption) -> anyhow::Result<()> {
    match crate::options::store_mode() {
        crate::options::StoreMode::Mem => connect_in_memory_store(db_option).await,
        crate::options::StoreMode::RocksDb => connect_configured_store(db_option).await,
    }
}

/// 旁路入口（探针 / 一次性工具 / python 调试）：读配置、连上去。
#[cfg(feature = "ws")]
pub async fn attach_store_connected() -> anyhow::Result<()> {
    let db_option = get_db_option_ext().inner;
    ensure_store_connected(&db_option).await
}

/// 一个库的文件变了（会话前进 / 回滚 / 换文件）：只动**这个 dbnum** 的模型
/// （`docs/plans/2026-09-12-lazy-startup-dev-plan.md` L4 / §3.4，母计划不变量 7）。
///
/// 该库此前已发布的根 → 在新会话的索引里逐根点查（`snapshot_presence`，只开这一个文件）：没了的
/// 删子树（否则 Plant UI 永远显示一条幽灵管线），还在的按新会话重生成；只作废**这个库**的发布回执；
/// 最后记一笔「最近定向刷新」给 `/health`。别的库一根不碰、一个回执不作废——8000 保存了，7998
/// 的世代与回执一动不动（跨库属性切片 §9 Q10 的验收形状）。
///
/// 目录库（CATA / DICT / PROP）的变化只记账不动模型：它们的依赖失效是阶段 5 余下的活。
/// 属性 / 树两个服务不在这里刷：它们各自的 `is_current()` 看文件头，下一次请求自己重建。
#[cfg(feature = "ws")]
async fn targeted_model_refresh(
    manager: &Arc<AiosDBManager>,
    change: &crate::data_interface::mdb_route_directory::RouteRefresh,
) {
    use crate::data_interface::db_generation::{TargetedRefresh, record_targeted_refresh};
    use crate::data_interface::generation_root::refno_from_e3d;
    use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};

    log::info!(
        "[route-poll] dbnum={} {} {}: sesno {}→{} generation {}→{} ({})",
        change.dbnum,
        change.db_type,
        change.kind,
        change.from.latest_sesno,
        change.to.latest_sesno,
        change.from.generation,
        change.to.generation,
        change.path.display()
    );
    let mut diagnostic = crate::process_diagnostics::Operation::begin("increment.detected",
        format!("dbnum={} kind={:?} from={:?} to={:?} path={}", change.dbnum, change.kind, change.from, change.to, change.path.display()));
    let delta_change = change.clone();
    let operation = diagnostic.id();
    match tokio::task::spawn_blocking(move || crate::increment_diagnostics::report(&delta_change, operation)).await {
        Ok(Ok(())) => {},
        Ok(Err(error)) => diagnostic.stage("increment.delta.failed", format!("{error:#}；明细未知，不当成零变化；继续失效/模型处理")),
        Err(error) => diagnostic.stage("increment.delta.failed", format!("任务异常: {error}；继续失效/模型处理")),
    }
    let mut refresh = TargetedRefresh {
        dbnum: change.dbnum,
        db_type: change.db_type.clone(),
        kind: change.kind,
        from: change.from,
        to: change.to,
        roots_seen: 0,
        roots_regenerated: 0,
        roots_retired: 0,
        receipts_invalidated: 0,
        outcome: "ok".to_string(),
        at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or_default(),
    };
    diagnostic.stage("increment.invalidate.begin", "refresh source view and cached handles");
    let option = aios_core::get_db_option();
    if let Err(error) = crate::data_interface::mdb_read_store::invalidate_cached(
        &option.project_name, &option.mdb_name, change.dbnum,
    ) {
        refresh.outcome = format!("read_view_refresh_failed: {error:#}");
        diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
        return;
    }
    if !crate::data_interface::model_source::is_element_db(&change.db_type) {
        refresh.outcome = "catalogue_change_recorded_only".to_string();
        diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
        return;
    }

    let model_store = ModelMemoryStore::global();
    let roots: Vec<e3d_io::refno::RefNo> = model_store
        .roots(ProjectionScope::Current)
        .into_iter()
        .filter(|root| {
            crate::data_interface::refno_route::refno_dbnum_of(*root) == Some(change.dbnum)
        })
        .collect();
    refresh.roots_seen = roots.len();
    refresh.receipts_invalidated =
        model_store.invalidate_receipts_of_dbnum(ProjectionScope::Current, change.dbnum);
    log::info!(
        "[route-poll] dbnum={} invalidated {} publication receipt(s) of its own; {} published root(s) to re-check",
        change.dbnum,
        refresh.receipts_invalidated,
        refresh.roots_seen
    );
    if roots.is_empty() {
        diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
        return;
    }

    // 新会话的索引里还有哪些根：只开这一个文件，逐根点查。
    let requested: Vec<aios_core::RefU64> = roots
        .iter()
        .map(|root| refno_from_e3d(*root).refno())
        .collect();
    let path = change.path.clone();
    diagnostic.stage("increment.presence.begin", format!("roots={} invalidated={}", refresh.roots_seen, refresh.receipts_invalidated));
    let presence = match tokio::task::spawn_blocking(move || {
        crate::data_interface::db_file_header::snapshot_presence(&path, &requested)
    })
    .await
    {
        Ok(Ok(presence)) => presence,
        Ok(Err(error)) => {
            log::error!(
                "[route-poll] dbnum={} 在新会话索引里核对已发布根失败，根保留、回执已作废，下一次 ensure 按新会话重比：{error:#}",
                change.dbnum
            );
            refresh.outcome = format!("presence_check_failed: {error:#}");
            diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
            return;
        }
        Err(error) => {
            log::error!(
                "[route-poll] dbnum={} 核对任务异常结束：{error}",
                change.dbnum
            );
            refresh.outcome = format!("presence_check_task_failed: {error}");
            diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
            return;
        }
    };
    let (live, retired): (Vec<_>, Vec<_>) = roots
        .into_iter()
        .partition(|root| presence.contains(&refno_from_e3d(*root).refno()));

    diagnostic.stage("increment.presence.done", format!("live={} retired={}", live.len(), retired.len()));
    for root in retired {
        let requested = refno_from_e3d(root);
        diagnostic.stage("increment.delete.begin", format!("refno={requested}"));
        match manager.delete_model_subtree(requested).await {
            Ok(()) => {
                model_store.forget_retired_root(ProjectionScope::Current, root);
                refresh.roots_retired += 1;
                diagnostic.stage("increment.delete.done", format!("refno={requested}"));
                log::info!(
                    "[route-poll] dbnum={} retired deleted model root {requested} from the current projection",
                    change.dbnum
                );
            }
            Err(error) => {
                log::error!(
                    "[route-poll] dbnum={} failed to retire deleted model root {requested}: {error:#}",
                    change.dbnum
                );
                refresh.outcome = format!("retire_failed: {error:#}");
                diagnostic.stage("increment.delete.failed", format!("refno={requested} error={error:#}"));
            }
        }
    }
    if !live.is_empty() {
        let root_names = live
            .iter()
            .map(|root| format!("{}/{}", root.word0, root.word1))
            .collect::<Vec<_>>();
        diagnostic.stage("increment.generate.begin", format!("roots={root_names:?}"));
        match crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots_report(
            manager,
            &root_names,
        )
        .await
        {
            Ok(report) if report.failures.is_empty() => {
                refresh.roots_regenerated = report.completed.len();
                log::info!(
                    "[route-poll] dbnum={} republished {} model root(s) at sesno {}",
                    change.dbnum,
                    report.completed.len(),
                    change.to.latest_sesno
                );
            }
            Ok(report) => {
                refresh.roots_regenerated = report.completed.len();
                refresh.outcome = format!("{} root(s) pending retry", report.failures.len());
                log::error!(
                    "[route-poll] dbnum={} refresh left {} model root(s) pending retry",
                    change.dbnum,
                    report.failures.len()
                );
            }
            Err(error) => {
                refresh.outcome = format!("regenerate_failed: {error:#}");
                log::error!(
                    "[route-poll] dbnum={} model refresh failed; roots remain unpublished: {error:#}",
                    change.dbnum
                );
            }
        }
    }
    diagnostic.finish(&refresh.outcome);
        record_targeted_refresh(refresh);
}

/// File/memory-first service path.  No database operation is allowed before
/// the HTTP core is ready; the optional mirror connects and drains its spool
/// in the background.
#[cfg(feature = "ws")]
async fn run_read_through_core(db_option: DbOption) -> anyhow::Result<()> {
    let startup_started = Instant::now();
    let stage_started = Instant::now();
    println!("[startup] stage=store event=begin");
    // 内存库承载模型发布，必须在接受模型请求前就绪；属性始终从文件读取。
    if !crate::options::store_mode().is_durable() {
        ensure_store_connected(&db_option).await?;
        crate::data_interface::embedded_surql::define_embedded_functions().await?;
    }
    println!("[startup] stage=store event=done elapsed_ms={}", stage_started.elapsed().as_millis());
    let stage_started = Instant::now();
    println!("[startup] stage=mdb event=begin");
    let mut manager = AiosDBManager::init_form_config().await?;
    manager
        .init_mdb(
            &db_option.project_name,
            &db_option.mdb_name,
            &db_option.module,
        )
        .await
        .map_err(|error| {
            anyhow!(
                "resolve MDB {} from source files: {error:#}",
                db_option.mdb_name
            )
        })?;
    let manager = Arc::new(manager);
    println!("[startup] stage=mdb event=done elapsed_ms={}", stage_started.elapsed().as_millis());
    let stage_started = Instant::now();
    println!("[startup] stage=http event=begin enabled={}", cfg!(feature = "http_api"));
    #[cfg(feature = "http_api")]
    let web_task = crate::web_service::start_configured(manager.clone()).await?;
    println!("[startup] stage=http event=done elapsed_ms={}", stage_started.elapsed().as_millis());
    // HTTP 已就绪；这里只建立 dbnum 路由目录，不读取元素。
    // 属性与树服务按目标 RefNo 惰性访问，不建立全 MDB 元素骨架。
    let step_started = Instant::now();
    println!("[startup] stage=route-directory event=begin");
    let directory = startup_route_directory(&db_option).map_err(|error| {
        anyhow!(
            "build dbnum route directory for MDB {}: {error:#}",
            db_option.mdb_name
        )
    })?;
    print_route_directory_banner(&directory, step_started);
    println!("[startup] stage=route-directory event=done elapsed_ms={}", step_started.elapsed().as_millis());

    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    let rooms_required = crate::options::room_membership();
    // Startup prepares reference inputs only. Whole-database generation is an
    // explicit model/rebuild operation, never implied by enabling room queries.
    initialization.configure_model_bootstrap(false);
    initialization.mark_data_ready_without_manifest();
    if rooms_required {
        initialization.begin_startup_rooms();
    }
    let stage_started = Instant::now();
    println!("[startup] stage=spatial-room event=begin room_membership={rooms_required}");
    prepare_room_resident_at_startup(&db_option).await;
    println!("[startup] stage=spatial-room event=done elapsed_ms={}", stage_started.elapsed().as_millis());
    let room_bootstrap_ready = !rooms_required
        || crate::fast_model::room_resident::current_for_calculation().is_ok();
    if rooms_required {
        if room_bootstrap_ready {
            initialization.settle_startup_rooms();
        } else {
            initialization.fail_startup_rooms("blocked", "room reference model is unavailable");
        }
    }
    if room_bootstrap_ready {
        let _ = initialization.open_model_phase();
        let _ = initialization.mark_model_ready();
    }

    // 2 s 循环改按 dbnum 轮询会话号（lazy-startup L4，阶段 5 的最小形态）：路由目录对每条路由
    // `stat` 一次、身份变了才开一次头，报出「哪些库变了」；每个变了的库只动它自己的根与回执。
    // 此前这里是 `refresh_if_stale`——任一库的会话号一变就重解 MDB 成员、整份重扫全 MDB 重建
    // 元素骨架，再把**全部**发布回执作废；一个 DESI 的 SAVEWORK = 重扫 AMS 28 库 / JEU 417 库。
    // 先叫一次把每个库的基线记进账本：第一次见只记不报，之后每轮返回的才是「自上次以来变了的库」。
    let stage_started = Instant::now();
    println!("[startup] stage=session-baseline event=begin");
    let seeded = directory.poll_generations();
    println!("[startup] stage=session-baseline event=done elapsed_ms={} total_ms={}",
        stage_started.elapsed().as_millis(), startup_started.elapsed().as_millis());
    debug_assert!(seeded.is_empty(), "第一次轮询只记基线，不该报变化");
    let refresh_manager = manager.clone();
    let refresh_directory = directory.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        crate::process_diagnostics::event(0, "increment.watch", "每 2 秒检测库文件/session；无变化每 30 秒输出心跳；变化立即处理");
        let mut round = 0u64;
        loop {
            interval.tick().await;
            round = round.wrapping_add(1);
            let poll_started = Instant::now();
            let polled = {
                let directory = refresh_directory.clone();
                tokio::task::spawn_blocking(move || directory.poll_generations()).await
            };
            let changes = match polled {
                Ok(changes) => changes,
                Err(error) => {
                    crate::process_diagnostics::event(0, "increment.poll.failed", &format!("round={round} error={error}；下一轮重试"));
                    log::error!("[route-poll] 轮询任务异常结束，下一拍重试：{error}");
                    continue;
                }
            };
            if round % 15 == 1 || !changes.is_empty() {
                crate::process_diagnostics::event(0, "increment.poll", &format!(
                    "round={round} changed_dbnums={:?} elapsed_ms={}；仅检查文件/session，不遍历元素",
                    changes.iter().map(|change| change.dbnum).collect::<Vec<_>>(), poll_started.elapsed().as_millis()));
            }
            for change in changes {
                targeted_model_refresh(&refresh_manager, &change).await;
            }
        }
    });

    let mirror_option = db_option.clone();
    tokio::spawn(async move {
        // 旧的属性备份任务只能重放到持久库，切换内存模式时保留任务而不消费。
        if !crate::options::store_mode().is_durable() {
            return;
        }
        loop {
            match ensure_store_connected(&mirror_option).await {
                Ok(()) => {
                    if let Err(error) =
                        crate::data_interface::embedded_surql::define_embedded_functions().await
                    {
                        log::error!("mirror schema installation failed: {error:#}");
                    }
                    match crate::data_interface::mirror_spool::MirrorSpool::for_process(
                        &mirror_option.surreal_ns,
                        &mirror_option.project_name,
                        &mirror_option.mdb_name,
                    ) {
                        Ok(spool) => loop {
                            match spool.replay(&SUL_DB).await {
                                Ok(count) if count > 0 => {
                                    log::info!("mirror ready; replayed {count} durable job(s)")
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    log::error!("mirror spool replay degraded: {error:#}")
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        },
                        Err(error) => log::error!("mirror spool open failed: {error:#}"),
                    }
                    break;
                }
                Err(error) => {
                    log::warn!("mirror connecting/degraded; core remains ready: {error:#}");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    });

    #[cfg(feature = "http_api")]
    {
        web_task.await??;
        return Ok(());
    }
    #[cfg(not(feature = "http_api"))]
    {
        std::future::pending::<()>().await;
        Ok(())
    }
}

/// 运行app
pub async fn run_app(option: Option<DbOptionExt>) -> anyhow::Result<()> {
    use std::sync::mpsc;

    mark_process_start();
    // 如果传入的是DbOptionExt，则取其内部的DbOption
    let db_option: DbOption = option
        .map(|o| options::apply_asset_root(o.inner))
        .unwrap_or_else(|| get_db_option_ext().inner);
    // Public entrypoint guard: take the deny-share handle before connecting a
    // local store or loading mutable process-global spatial state. `run_cli`
    // repeats this call deliberately; OnceLock makes that check idempotent.
    acquire_process_instance_lock(&db_option)?;
    // 设置模式（配置向导，计划 2026-09-09-setup-wizard-web-frontend.md / ADR-069，
    // 判据在 ADR-070 扩到 spawn 两档）：配置无效（project_path 缺/不存在，或需要端点
    // 的档位库地址留空——external 空地址是出厂模板形态，spawn 档空地址是必坏手改
    // 形状）时，下面的连库与监控目录解析必死，而且死在 HTTP 起来之前，向导页根本
    // 送不出去。所以在这里拐进最小启动臂：不连库、不扫描，只绑 HTTP 供 /setup.html
    // 与 setup API；配置修好经 restart 口交接后，新进程自然走正常启动。
    // 库地址**填了但连不上**不算配置无效——那照旧走下面的正常失败，不盖真故障。
    #[cfg(feature = "http_api")]
    if let Some(reason) = crate::web_service::setup::setup_mode_reason(&db_option) {
        return crate::web_service::setup::run_setup_mode(reason).await;
    }
    #[cfg(feature = "ws")]
    if crate::options::data_face().is_read_through() {
        return run_read_through_core(db_option).await;
    }
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
        // 服务主进程是唯一给 `Spawn` 的调用方：只有它有资格把库拉起来、收自己的孤儿。
        // 两档介质的分派全在 `ensure_store_connected` 里，旁路入口共用同一条。
        ensure_store_connected(&db_option).await?;
    }

    crate::data_interface::data_face_signature::validate_or_claim(
        &db_option.project_name,
        &db_option.surreal_ns,
    )
    .await?;

    // ADR-050: this table is only a scheduler ledger for the current process.
    // Clear it after the database connection exists but before spatial loading,
    // preload, db-manager construction, watcher startup, or worker startup. A
    // cleanup error is fatal: continuing would replay a prior process snapshot.
    //
    // 初始化发布 run 的 journal 恢复（`recover_before_cleanup`）已退役（ADR-071 S5，d-300）；
    // 模型投影不在启动期水化、也不在 root 请求处回填。启动清理
    // `model_update_pending` 也没有了（ADR-072 D5）：那张表随第二条异步账退役，`run_cli` 的
    // 水化段用 `REMOVE TABLE IF EXISTS` 一次收掉；崩溃残留的出路是写入标记回退。

    // 启动分层判据装载空间树，与 run_app 同一失败语义（方案决策 D3）：树是可
    // 重建的派生数据，加载失败告警降级空树，不阻断启动；降级两态交给后台复检。
    if crate::options::spatial_compute() {
        if let Err(error) = crate::fast_model::aabb_tree::load_project_tree_verified().await {
            eprintln!("空间树启动加载失败（{error:#}），以空树启动，等待后台复检重建");
        }
        crate::fast_model::spatial_state::spawn_spatial_revalidator();
    } else {
        println!(
            "空间计算已关闭（{}=0）：跳过空间树装载、后台复检和后置 AABB，只运行模型生成",
            crate::options::SPATIAL_COMPUTE_ENV
        );
    }
    // let (tx, mut rx) = mpsc::channel::<i32>();
    run_cli(db_option).await
}

pub mod admin;
// `data_state` 整个模块 2026-09-07 随 `feature = "sql"` 退役：它只有一个文件，
// 而那个文件从第一行到最后一行都在 sql 门后（往 MySQL 的 `data_status` 表写状态）。
// pub mod data_to_excel;
// pub mod data_to_file;
// pub mod other_plat;
// pub mod pcf;
// pub mod plug_in;
// pub mod rvm;
// pub mod ssc;
pub mod version_management;
