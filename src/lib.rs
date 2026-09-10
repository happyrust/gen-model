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
use crate::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
use crate::fast_model::room_model::{
    StartupRoomBuild, build_room_relations, reconcile_startup_room_build,
};
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
/// **只有 `embedded-mem` 不拿。** 锁挡的是「两个进程驱动同一份持久状态」，而
/// `mem://` 那一档持久层就在进程里：两个实例各连各的 kv-mem，谁也读不到谁写的
/// 字节，锁没有保护对象。放开它正是为了让两个实例并排跑起来。
///
/// `spawned-mem` **照常拿锁**（ADR 计划 Q4）。它的库有真端口、别人连得上，
/// 两个 gen-model 配同一份 `DbOption.toml` 就是冲着同一个端口去的——「不共享库」
/// 这个理由在那一档不成立。
///
/// 放开不是白送的。盘上还有东西是共享的：`meshes_path` 指的网格目录首当其冲——
/// 增量路径内部会强制落网格，`gen_mesh=false` 拦不住它。运行目录相对的那些
/// （`accel_tree` 快照、日志、`cata_dep_cache.bin`）天然分开，绝对路径的不会。
/// 所以并排跑之前得先让两份配置的 `meshes_path` 分家。
fn instance_lock_skip_reason(store_mode: crate::options::StoreMode) -> Option<&'static str> {
    matches!(store_mode, crate::options::StoreMode::EmbeddedMem).then_some(
        "store_mode=embedded-mem，持久层在进程内，两个实例不共享库（meshes_path 仍是共享的，先让它分家）",
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
            "本进程以 embedded-mem 起过、没有写者锁，不能再以 {} {}/{} 写库",
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

    /// **只有进程内嵌那一档不拿锁。** `spawned-mem` 的库有真端口、别人连得上，
    /// 两个 gen-model 配同一份 `DbOption.toml` 就是冲着同一个端口去的——「不共享库」
    /// 这个理由在那一档不成立（计划 Q4）。翻回「凡内存模式都不拿」这条就红。
    #[test]
    fn only_the_embedded_store_needs_no_single_instance_lock() {
        use crate::options::StoreMode;

        assert!(
            instance_lock_skip_reason(StoreMode::External).is_none(),
            "持久层在进程外时锁仍有保护对象，不许跳过"
        );
        assert!(
            instance_lock_skip_reason(StoreMode::SpawnedMem).is_none(),
            "自带存储有真端口，两个实例会抢同一个，锁不许跳过"
        );
        let reason =
            instance_lock_skip_reason(StoreMode::EmbeddedMem).expect("不拿锁必须说出为什么");
        assert!(
            reason.contains("embedded-mem"),
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

/// 启动房间初始化（常驻房间模型，`docs/plans/2026-09-07-resident-room-model-plan.md` §3 P3，
/// 第 5 轮 Q17–Q20）。
///
/// 位置：紧跟空间树装载、排在 watcher / worker **之前**。门只有两道——`room_resident_model`
/// 开关与 `spatial_compute`（关了就没有房间子系统），都由 `run_cli` 在调用处判；
/// `startup_data_ready` / 上弦位 / `startup_autorun` / `model_incremental` 一道都不看
/// （Q18 (a)）：房间是参照系，不是增量的一部分。它只建模、写分片与清单，不写归属边、
/// 不写 `resident_reference`；建不起来打印原因、记 `resident_blocked`，不拦启动——下面的
/// 增量轮与启动对比拿不到模型就止步（原则 2）。
///
/// 哪些文件的清单行归**文件轮次**管在这里按监听限定域算好传进去：限定域生效 = 限定域里
/// 的库，其余库没有轮次推它、由初始化推进到文件最新会话（Q19 (a)）；未生效 = 每个文件都有
/// 自己的轮次，初始化一行不动。房间那三份文件不认识 `watch_scope`（源码钉
/// `room_files_never_consult_the_watch_scope`），限定域只在这里被问一次。
async fn initialize_resident_rooms_at_startup(db_option: &DbOption) {
    use crate::fast_model::room_resident::{self, RoundOwnership};
    use crate::fast_model::room_topology::RoomHierarchy;

    let Some(hierarchy) = RoomHierarchy::configured() else {
        println!("[房间初始化] 房间子系统需要 project_hd 或 project_hh 特性：跳过");
        return;
    };
    #[cfg(any(feature = "project_hd", feature = "project_hh"))]
    let match_room_fn = crate::fast_model::room_model::configured_match_room_fn();
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let match_room_fn = |_: &str| false;
    let ownership = if crate::data_interface::watch_scope::active() {
        RoundOwnership::Only(
            crate::data_interface::watch_scope::dbnums()
                .into_iter()
                .collect(),
        )
    } else {
        RoundOwnership::EveryFile
    };
    println!(
        "正在初始化房间（常驻房间模型 · {}）：MDB 全部设计库，{}...",
        crate::options::data_face().as_str(),
        ownership.describe()
    );
    let room_key_words = db_option.get_room_key_word();
    let mesh_dir = db_option.get_meshes_path();
    let result = room_resident::initialize_at_startup(
        crate::options::data_face(),
        &ownership,
        hierarchy,
        &room_key_words,
        match_room_fn,
        &mesh_dir,
    )
    .await;
    match &result {
        Ok(outcome) => println!("{}", outcome.summary()),
        Err(error) => eprintln!(
            "[房间初始化] 建不起来（不动归属边；增量轮与启动对比拿不到模型就止步）: {error:#}"
        ),
    }
    room_resident::record_startup_init(result.as_ref()).await;
}

/// Shared room-capability preparation for ingest and read-through. Callers have
/// already resolved the MDB skeleton and connected the process-local model store.
async fn prepare_room_resident_at_startup(db_option: &DbOption) {
    use crate::fast_model::room_resident;

    if !crate::options::room_resident_model() {
        room_resident::mark_disabled("configuration");
        return;
    }
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

/// 环境变量名：无论如何都不跑启动全量房间重建（既有运维止血口，语义不变）。
pub const SKIP_STARTUP_ROOM_BUILD_ENV: &str = "AIOS_SKIP_STARTUP_ROOM_BUILD";

/// 第三道门的判据（纯函数）：空间树校验为空 → 全量重建是平凡的，给出跳过的理由。
///
/// 只认 `ReadyEmpty`（库里 0 条可用几何指针，`spatial_state::set_ready_by_entries(0)`）。
/// `Ready` 交给库侧对账；`Loading` / `DegradedBlocked` 之类**不在这里判**——它们在
/// `build_room_relations` 里由 `ensure_room_tree_coverage` fail-closed 拦住，那道门说的是
/// 「判不了就不写」，这里说的是「没什么可判」，两句话不合成一句。
pub(crate) fn vacuous_room_build_reason(state: SpatialTreeState) -> Option<String> {
    (state == SpatialTreeState::ReadyEmpty).then(|| {
        "空间树已验证为空（库里没有可用几何指针）：没有已发布几何可算，全量重建无事可做\
         ——读透 + 空库的常态，房间归属由发布事务排的重算收敛"
            .to_string()
    })
}

/// 环境变量名：本次启动显式要求跑一次全量房间重建（`1/true/yes/on`）。常驻房间模型开着时
/// 启动对比接管了房间段，全量降为六条显式入口，这是运维那一条（共识 d-42，
/// `docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md` §4.3）。开关关着时它没有意义
/// ——那条路每次启动本来就按凭据对账决定跑不跑。
pub const ROOM_FULL_REBUILD_ENV: &str = "AIOS_ROOM_FULL_REBUILD";

/// 常驻房间模型开着时的房间段（d-42 §4.2 / §4.6）：启动对比接管——参照摘要 == 现状零写入；
/// 变了只对变了的面板删 / 重算 / 改列；全量降为**六**条显式入口（无参照 / **读参照失败** /
/// 同键不等 / 参照不可读 / 持久 `rebuild_required` / [`ROOM_FULL_REBUILD_ENV`]），由
/// `room_startup::startup_room_reconcile` 判出、交回这里跑。「读参照失败」与「无参照」同判
/// （多跑一次全量远好过让一次查询故障静默变成房间永不收敛），方案稿写的五条漏了它。
///
/// 门：止血口 `AIOS_SKIP_STARTUP_ROOM_BUILD` 与空树平凡两道在 `startup_room_reconcile` 里照旧最强
/// （对比与全量一起跳）；`startup_autorun` **只门全量那一臂**——对比是定向的，成本随变更量走，
/// 不该为它付那道「别自动干活」的门。ADR-048 D3 照旧：这里不读监听限定域，限定域上弦不得
/// 拖回全量修复。凭据对账（`reconcile_startup_room_build`）不再决定跑不跑（§4.6）——对比接管了
/// 它拓扑那一半，几何那一半本来就归发布事务的 `RoomRecalcPanel`；每次成功全量照旧盖章。
async fn reconcile_rooms_with_resident_model(db_option: &DbOption) -> bool {
    use crate::fast_model::room_startup::{self, StartupRoomVerdict};

    let verdict = room_startup::startup_room_reconcile(db_option).await;
    // 「初始化前遗留的逐构件房间待办」那一档随 pending 表退了（ADR-072 D5）：房间随各库任务
    // 整库算，启动这里只认对比判出来的去向。
    let (reason, expected) = match verdict {
        StartupRoomVerdict::FullRebuild { reason, expected } => (reason, expected),
        StartupRoomVerdict::Settled => return true,
    };
    if !crate::options::startup_autorun() {
        println!(
            "跳过启动全量房间重建（startup_autorun=false；本来要它的原因：{reason}）：\
             房间归属由增量队列收敛，下次启动再比，或显式打开自动执行"
        );
        return false;
    }
    println!("启动全量房间重建：{reason}");
    println!(
        "正在计算房间（空间树 {} 条）",
        GLOBAL_AABB_TREE.read().await.tree.size()
    );
    let time = Instant::now();
    // 几何视图按数据面取（审核 F-H）：摄入读已发布房间几何、读透读实时行——与增量两条分支、
    // 启动对比同一份几何，ADR-010 §9「增量 == 全量逐边」才是在同一份数据上比。开关关着的
    // 老臂（下面 `build_room_relations`）仍恒 Live，一字不变。
    let cutoff = chrono::Utc::now().to_rfc3339();
    println!("[ams-phase] at={cutoff} phase=project_rooms state=start");
    let settled = match crate::fast_model::room_model::build_room_relations_with_view(
        db_option,
        crate::fast_model::room_model::consumption_view(),
    )
    .await
    {
        // 写点 ②（d-42 §4.5）：全部成功才写参照；盖章仍在 build_room_relations 收尾那一处。
        // 「从未生成」的面板（`without_geometry`）不算没成功（D-A (a)），只随回执点名。
        Ok(outcome) => {
            let reference_written =
                room_startup::settle_full_rebuild(db_option, expected.as_ref(), &outcome).await;
            if !reference_written {
                eprintln!("项目房间完成基线未提交，保留初始化屏障");
                return false;
            }
            true
        }
        Err(error) => {
            eprintln!("计算房间未完全成功: {error:#}");
            false
        }
    };
    println!("计算房间花费时间: {} ms", time.elapsed().as_millis());
    println!(
        "[ams-phase] at={} phase=project_rooms state=end success={settled} elapsed_ms={}",
        chrono::Utc::now().to_rfc3339(),
        time.elapsed().as_millis()
    );
    // 与开关关着的那一臂同姿态：全量跑了才顺带算支管部件派生量，失败只告警。
    if let Err(error) = update_cal_bran_component().await {
        eprintln!("支管部件派生量计算未完全成功（可由下次启动或手动重算回补）: {error:#}");
    }
    settled
}

/// 本次启动要不要跳过全量房间重建；跳过时返回那句给人看的理由。
///
/// 四道门按优先级排：
/// 1. [`SKIP_STARTUP_ROOM_BUILD_ENV`] —— 永不重建。它比自动执行开关更强，因为
///    「跑增量」与「跑 2 万面板级的全量重建」是两件事：L3 夹具、增量演练这类
///    场景要前者不要后者。
/// 2. `startup_autorun=false`（默认 true）—— 本次启动显式要求不自动干活。
/// 3. 空间树校验为空（[`vacuous_room_build_reason`]）—— 没有已发布几何，全量重建
///    会把每块在册面板算成 `NoGeometry`、一条边都写不出，白扫一遍全部 DESI 的房间拓扑。
///    读透 + spawned-mem 的每次冷启动都是这个形状（合并计划 §9 R3）。只读进程态，
///    不为它付库查询；跳过时**照样盖章**（[`room_model::stamp_vacuous_room_build`]），
///    让 `/health.room_build` 与下次启动的对账都有比较对象。
/// 4. 三道门都放行，才与库侧凭据对账：只有空间状态真的变过才重建。
///
/// 第四道门是 2026-08 那次改动的要点。此前这一步是无条件的，于是每次重启都要为一件
/// 多半无事可做的事付上十几秒——而它真正的兜底价值只在「空间状态变了、增量
/// 房间队列又没收干净」时才兑现。
async fn skip_startup_room_build() -> Option<String> {
    if std::env::var(SKIP_STARTUP_ROOM_BUILD_ENV).is_ok() {
        return Some(format!("{SKIP_STARTUP_ROOM_BUILD_ENV} 已设置"));
    }
    if !crate::options::startup_autorun() {
        return Some("startup_autorun=false".to_string());
    }
    if let Some(reason) =
        vacuous_room_build_reason(crate::fast_model::spatial_state::current_state())
    {
        // 平凡重建也盖章：凭据没有比较对象的话，/health 会一直说「没有成功的全量房间
        // 重建凭据」，下次启动也只能判「跑」。盖不上不拦启动，只是下次多对账一次。
        if let Err(error) = crate::fast_model::room_model::stamp_vacuous_room_build().await {
            println!("[房间全量] 空世界的凭据没盖上（下次启动会再对账一次）: {error:#}");
        }
        return Some(reason);
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

pub mod store_process;
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

/// 启动水化（ADR-071 D7）：把每个已落过模型的库（`model_sesno > 0`）的模型投影从库整体反序列化进
/// 进程内 `ModelMemoryStore`。这是「库 = 内存结果的序列化」的正当读法，取代了此前只对「有发布
/// run 清单的库」做的 `recover_before_cleanup`。失败只出声不拦启动：投影缺席时按需生成会自己补
/// （`/model/ensure` 对内存里没有的根按凭证重建），不会读到错的东西。
///
/// `states` 是同一水化段里刚读回的水位镜像（`list_registered`）——排在 ADR-060 D10 的兼容播种
/// **之后**，接管的历史库才会带着 `model_sesno > 0` 进来。
async fn hydrate_model_projections(states: &[crate::data_interface::dbnum_state::DbnumState]) {
    if !crate::options::store_mode().is_durable() {
        return;
    }
    let started = Instant::now();
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let (mut hydrated, mut failed) = (0usize, 0usize);
    for state in states
        .iter()
        .filter(|state| state.model_sesno > 0 && state.db_type.eq_ignore_ascii_case("DESI"))
    {
        match crate::fast_model::model_db_adapter::restore_projection_on(
            &aios_core::SUL_DB,
            store,
            state.dbnum,
        )
        .await
        {
            Ok(()) => hydrated += 1,
            Err(error) => {
                failed += 1;
                eprintln!(
                    "模型投影启动水化 dbnum={} 失败（按需生成会自行重建）：{error:#}",
                    state.dbnum
                );
            }
        }
    }
    println!(
        "模型投影启动水化完成：{hydrated} 个库已读回进程内投影，{failed} 个失败，耗时 {}",
        fmt_elapsed(started.elapsed())
    );
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
    let rollbacks = crate::data_interface::startup_reconcile::rollback_unfinished_writes().await;
    if !rollbacks.is_empty() {
        for line in &rollbacks {
            println!("{line}");
        }
        println!("写入标记回退完成，耗时 {}", fmt_elapsed(step_started.elapsed()));
    }
    // ---- 统一水化段（ADR-071 D7 / S6）：库 → 进程内，一次，排在全部播种之后、任何 worker /
    // watcher 之前。① 三水位镜像：`hydrate_watermark_mirror` 一次读全（水位行 + info 表旧水位）
    // 并单次替换发布，读者看不到半张快照——运行期的 `list_registered` 不再碰镜像，整表替换只在
    // 这个没有写者的时刻做；② 模型投影 + `gen_root` 根凭证回执：每个已落过模型的库整体读回
    // `ModelMemoryStore`（`restore_projection_on` 一并带回每根的 `RootReceipt`）；③ 一次性迁移：
    // 删掉已退役的发布 run 表。`gen_root` 凭证的整库口径（安定下限 / 未安定根）不另建镜像——
    // 启动核对与规划器按需读库里刚落的那一份（d-227 的镜像口径只覆盖水位行）。
    let step_started = Instant::now();
    crate::data_interface::dbnum_state::DbnumState::enable_watermark_mirror();
    let hydrated_states =
        crate::data_interface::dbnum_state::DbnumState::hydrate_watermark_mirror().await?;
    println!(
        "水位内存镜像已水化 {} 个 dbnum，耗时 {}",
        hydrated_states.len(),
        fmt_elapsed(step_started.elapsed())
    );
    hydrate_model_projections(&hydrated_states).await;
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
    // 成员名单解完，紧接着把这个 MDB 的**元素骨架**装进内存：每个声明的 DESI 做一次
    // e3d-io 头扫描，落成 `ref0 → dbnum` 归属、`refno → 属主`、`refno → noun` 三张表。
    // 属性不在里面——2026-09-05 实测 AMS `/ALL`（28 库 / 611,345 元素）：骨架 0.72s /
    // 19.8 MiB，全属性 8.02s / **1291 MiB**，而那 2215 B/元素大半是 per-element
    // `HashMap<String, _>` 的表示开销。换紧凑布局之前不谈全属性常驻。
    //
    // 放在这里而不是更早：它吃 `init_mdb` 刚登记的成员名单。放在这里而不是更晚：
    // 树接口（`/api/v1/tree/*`）与按需生成的选根都要它，此前是第一次树请求时现扫，
    // 那 0.7 秒落在用户点第一下树的那一刻。
    //
    // 失败只告警不中止：`mdb_memory_store::ensure` 在第一次用到时会自己再建一遍，
    // 行为退回到预热之前，不是回归。
    let step_started = Instant::now();
    match crate::data_interface::mdb_memory_store::ensure(
        &db_option.project_name,
        &db_option.mdb_name,
    ) {
        Ok(skeleton) => {
            println!(
                "{}，本步耗时 {}",
                skeleton.describe(),
                fmt_elapsed(step_started.elapsed())
            );
            // 声明了却不在盘上的库、自报库号与声明不符的文件——两者在下游都表现为
            // 「这个 refno 不存在」，跟元素真的被删了长得一模一样，必须当面点名。
            for problem in skeleton.problems() {
                eprintln!("*** MDB 骨架：{problem} ***");
            }
            if skeleton.stats().unreadable_records > 0 {
                eprintln!(
                    "*** MDB 骨架：{} 条记录索引指得到、内容读不出来，它们不在骨架里 ***",
                    skeleton.stats().unreadable_records
                );
            }
        }
        Err(error) => {
            log::warn!("MDB 元素骨架预热失败，退回按需现建：{error:#}");
            eprintln!(
                "*** MDB 元素骨架预热失败，退回第一次树请求时现建（行为与预热之前相同）：\
                 {error:#} ***"
            );
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
    // `E3dModelService::generate_dbnum`.
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
                let queued = scheduler.snapshot().iter().filter(|row| row.state == "queued").count();
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

    // 看门狗退出之后若 Web 服务已启用，保持进程长驻对外服务，供前端触发
    // preview/execute 手动更新与按需生成。
    #[cfg(feature = "http_api")]
    if crate::get_db_option_ext().http_api_addr.is_some() {
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
        "  增量阶段：data={} model={} room={}（数据链与模型链并行，房间排在模型之后）",
        crate::options::data_incremental(),
        crate::options::model_incremental(),
        crate::options::room_incremental()
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
    println!("  数据读取：direct / e3d-io（唯一形态；legacy db 读路由已于 2026-09-08 退役）");
    if let Some(notice) = crate::data_interface::watch_scope::mode_notice() {
        println!("  *** {notice}");
    }
    if let Some(notice) = crate::data_interface::model_type_scope::mode_notice() {
        println!("  *** {notice}");
    }
    match crate::options::store_mode() {
        crate::options::StoreMode::EmbeddedMem => println!(
            "  *** 内存模式（store_mode=embedded-mem）：持久层是进程内嵌 kv-mem，\
             进程一退整库即消失，外部无端口可连"
        ),
        crate::options::StoreMode::SpawnedMem => println!(
            "  存储介质：memory（store_mode=spawned-mem，本进程自带的 {}）——\
             进程一退整库即消失，但端口是真的",
            crate::store_process::spawned_endpoint()
                .unwrap_or_else(|| db_option.get_version_db_conn_str())
        ),
        crate::options::StoreMode::SpawnedRocks => println!(
            "  存储介质：rocksdb（store_mode=spawned-rocksdb，本进程自带的 {}，数据落 {}）——\
             活得过重启；退出收尸只杀进程，下次启动重新拉起即接上",
            crate::store_process::spawned_endpoint()
                .unwrap_or_else(|| db_option.get_version_db_conn_str()),
            crate::store_process::default_rocksdb_dir(
                &db_option.project_code,
                &db_option.project_name
            )
            .display()
        ),
        crate::options::StoreMode::External => {
            println!("  存储介质：{}", db_option.get_version_db_conn_str())
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
        if let Some(addr) = ext.http_api_addr.as_deref() {
            println!("  HTTP API：http://{addr}/api/v1");
        }
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
         Capture-*Evidence 都够不着）。要留证据请改 store_mode = \"spawned-mem\"。"
    );
    Ok(())
}

/// 连配置里那台 SurrealDB（`external` 与 `spawned-mem` 走同一条）。
///
/// 两档的分别只在「那台服务是谁起的」——到这一步它已经在监听了，连接语句一个字不差。
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

/// `spawned-mem` / `spawned-rocksdb`：让配置里那对 `v_ip:v_port` 上有一台**本进程负责的**
/// SurrealDB（memory 或 rocksdb 后端，由 `datastore` 说了算——两档只差这一个位置参数）。
///
/// 端口上已经有东西时按 `store_process::judge_port` 那张表行事（附着 / 收孤儿 / 拒绝）。
/// 自己起的那一台登记进程内单例，正常退出、`Ctrl+C` 与 panic 三条路都收尸；
/// 附着到的那一台**不收**——它不是我们的。rocksdb 档收尸杀的只是进程，数据在盘上，
/// 下次启动重新拉起即接上（ADR-070）。
#[cfg(feature = "ws")]
async fn ensure_own_store(
    db_option: &DbOption,
    startup: crate::store_process::StoreStartup,
    datastore: &crate::store_process::SpawnDatastore,
) -> anyhow::Result<()> {
    use crate::store_process::{
        DEFAULT_READY_TIMEOUT, StoreHandle, StoreIdentity, ensure_spawned_store,
        install_spawned_store, shutdown_spawned_store,
    };

    let identity = StoreIdentity {
        project: db_option.project_name.clone(),
        namespace: db_option.surreal_ns.clone(),
        data_face: crate::options::data_face().as_str().to_owned(),
    };
    let handle = ensure_spawned_store(
        &db_option.v_ip,
        db_option.v_port,
        &db_option.v_user,
        &db_option.v_password,
        &identity,
        startup,
        DEFAULT_READY_TIMEOUT,
        datastore,
    )
    .await?;
    match handle {
        StoreHandle::Attached(_) => {}
        StoreHandle::Spawned(store) => {
            install_spawned_store(store);
            // `Drop` 兜正常退出与 panic，兜不住 `Ctrl+C`——那一路 tokio 直接结束进程，
            // 析构不跑，留下的孤儿会占着端口，而下一次启动只能靠署名把它认出来。
            // 少这一条不是「偶尔漏一个」，是**每一次 Ctrl+C 都漏**。
            tokio::spawn(async {
                if tokio::signal::ctrl_c().await.is_ok() {
                    println!("收到 Ctrl+C，先收掉自带存储再退出…");
                    shutdown_spawned_store();
                }
                std::process::exit(130);
            });
        }
    }
    Ok(())
}

/// **连上持久层的唯一入口。** 三档介质、一条下游：`SUL_DB` 是 `Surreal<Any>`，
/// 引擎选择对上层不可见，所以初始化解析、增量窗口写回、模型与房间派生数据全部照原路走。
///
/// `startup` 决定这一次有没有资格把库拉起来——**只有服务主进程给 `Spawn`**。
/// 探针、一次性工具与 python 调试入口给 `AttachOnly`：它们要读的是服务手上那个库，
/// 端口空着时正确的反应是响亮地说「服务没起」，而不是自己拉一台空库在上面作业。
#[cfg(feature = "ws")]
pub async fn ensure_store_connected(
    db_option: &DbOption,
    startup: crate::store_process::StoreStartup,
) -> anyhow::Result<()> {
    match crate::options::store_mode() {
        crate::options::StoreMode::EmbeddedMem => connect_in_memory_store(db_option).await,
        crate::options::StoreMode::SpawnedMem => {
            ensure_own_store(
                db_option,
                startup,
                &crate::store_process::SpawnDatastore::Memory,
            )
            .await?;
            connect_configured_store(db_option).await
        }
        crate::options::StoreMode::SpawnedRocks => {
            let datastore = crate::store_process::SpawnDatastore::RocksDb(
                crate::store_process::default_rocksdb_dir(
                    &db_option.project_code,
                    &db_option.project_name,
                ),
            );
            ensure_own_store(db_option, startup, &datastore).await?;
            connect_configured_store(db_option).await
        }
        crate::options::StoreMode::External => connect_configured_store(db_option).await,
    }
}

/// 旁路入口（探针 / 一次性工具 / python 调试）用的那一档：读配置、只附着、不拉起。
#[cfg(feature = "ws")]
pub async fn attach_store_connected() -> anyhow::Result<()> {
    let db_option = get_db_option_ext().inner;
    ensure_store_connected(&db_option, crate::store_process::StoreStartup::AttachOnly).await
}

/// File/memory-first service path.  No database operation is allowed before
/// the HTTP core is ready; the optional mirror connects and drains its spool
/// in the background.
#[cfg(feature = "ws")]
async fn run_read_through_core(db_option: DbOption) -> anyhow::Result<()> {
    // 内存库承载模型发布，必须在接受模型请求前就绪；属性始终从文件读取。
    if !crate::options::store_mode().is_durable() {
        ensure_store_connected(&db_option, crate::store_process::StoreStartup::Spawn).await?;
        crate::data_interface::embedded_surql::define_embedded_functions().await?;
    }
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
    #[cfg(feature = "http_api")]
    let web_task = {
        let manager = manager.clone();
        tokio::spawn(async move { crate::web_service::serve_if_configured(manager).await })
    };
    let snapshot = crate::data_interface::mdb_memory_store::ensure(
        &db_option.project_name,
        &db_option.mdb_name,
    )?;
    println!("read-through core snapshot ready: {}", snapshot.describe());
    prepare_room_resident_at_startup(&db_option).await;

    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    initialization.configure_model_bootstrap(false);
    initialization.mark_data_ready_without_manifest();
    let _ = initialization.open_model_phase();
    let _ = initialization.mark_model_ready();

    let refresh_manager = manager.clone();
    let project = db_option.project_name.clone();
    let mdb = db_option.mdb_name.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            match crate::data_interface::mdb_memory_store::refresh_if_stale(&project, &mdb) {
                Ok(true) => {
                    let model_store =
                        crate::fast_model::model_memory_store::ModelMemoryStore::global();
                    let previous_roots = model_store
                        .roots(crate::fast_model::model_memory_store::ProjectionScope::Current);
                    // A source snapshot can remove a whole delivery root. Regenerating every
                    // previous root treats that absence as a generation failure and deliberately
                    // retains its old rows, which makes Plant UI show a ghost pipeline forever.
                    // Partition against the newly-published skeleton first: missing roots are
                    // successful tombstones, while only live roots enter regeneration.
                    let current = crate::data_interface::mdb_memory_store::get(&project, &mdb);
                    let (refresh_roots, deleted_roots): (Vec<_>, Vec<_>) =
                        previous_roots.into_iter().partition(|root| {
                            current.as_ref().is_some_and(|snapshot| {
                                snapshot.contains(
                                    crate::data_interface::generation_root::refno_from_e3d(*root)
                                        .refno(),
                                )
                            })
                        });
                    for root in deleted_roots {
                        let requested =
                            crate::data_interface::generation_root::refno_from_e3d(root);
                        if let Err(error) = refresh_manager.delete_model_subtree(requested).await {
                            log::error!(
                                "source snapshot failed to retire deleted model root {requested}: {error:#}"
                            );
                        } else {
                            log::info!(
                                "source snapshot retired deleted model root {requested} from the current projection"
                            );
                        }
                    }
                    let invalidated = model_store.invalidate_all_receipts(
                        crate::fast_model::model_memory_store::ProjectionScope::Current,
                    );
                    log::info!(
                        "source snapshot advanced atomically for {project}/{mdb}; invalidated {invalidated} model publication receipt(s)"
                    );
                    if !refresh_roots.is_empty() {
                        let roots = refresh_roots
                            .into_iter()
                            .map(|root| format!("{}/{}", root.word0, root.word1))
                            .collect::<Vec<_>>();
                        match crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots_report(
                            &refresh_manager,
                            &roots,
                        )
                        .await
                        {
                            Ok(report) if report.failures.is_empty() => log::info!(
                                "source snapshot refresh published {} model root(s) from the new epoch",
                                report.completed.len()
                            ),
                            Ok(report) => log::error!(
                                "source snapshot refresh left {} model root(s) pending retry",
                                report.failures.len()
                            ),
                            Err(error) => log::error!(
                                "source snapshot model refresh failed; roots remain unpublished: {error:#}"
                            ),
                        }
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    log::error!("source snapshot refresh failed; old epoch retained: {error:#}")
                }
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
            match ensure_store_connected(&mirror_option, crate::store_process::StoreStartup::Spawn)
                .await
            {
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
        // 三档介质的分派全在 `ensure_store_connected` 里，旁路入口共用同一条。
        ensure_store_connected(&db_option, crate::store_process::StoreStartup::Spawn).await?;
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
    // 模型投影的启动水化在 `run_cli` 的统一水化段里（ADR-071 D7 / S6）。启动清理
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
    #[ignore = "启动房间段已被启动对比重排（项目级启动全量重建退役），`skip_startup_room_build().await` \
                已不在启动段里、钉未随迁：由房间流随 T4/启动对比收口反向重钉（d-73、mem-107、\
                changelog 2026-09-09）"]
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

    /// 四道门的优先级：止血口 > 冷启动开关 > 空树平凡 > 库侧对账。
    ///
    /// 顺序不能乱。`AIOS_SKIP_STARTUP_ROOM_BUILD` 排在最前是因为「跑增量」与
    /// 「跑 2 万面板级全量重建」是两件事——L3 夹具正是要前者不要后者；空树那道门只读
    /// 进程态，排在对账之前是因为对账要读库，没什么可判的时候不该为它付这次查询；
    /// 对账排在最后。
    #[test]
    fn the_four_gates_keep_their_precedence() {
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
        let vacuous_at = body
            .find("vacuous_room_build_reason(crate::fast_model::spatial_state::current_state())")
            .expect("空树那道门必须读的是进程态空间状态");
        let reconcile_at = body
            .find("reconcile_startup_room_build().await")
            .expect("放行后必须与库侧凭据对账");
        assert!(
            env_at < autorun_at && autorun_at < vacuous_at && vacuous_at < reconcile_at,
            "四道门的优先级是 止血口 → 冷启动开关 → 空树平凡 → 库侧对账: {body}"
        );
    }

    /// 第三道门的判据是纯函数，真值表钉死：**只有** `ReadyEmpty` 算平凡重建。
    ///
    /// `Ready` 必须交给对账（树非空就可能有该重算的房间）；`Loading` / `Rebuilding` /
    /// 两个 Degraded / `ReplayRequired` / `Uninitialized` 也不许在这里被折成「跳过」——
    /// 它们该撞的是 `build_room_relations` 里 fail-closed 的覆盖率闸门，那一撞会出声，
    /// 这里悄悄跳过则不会。
    #[test]
    fn only_a_verified_empty_tree_makes_the_full_rebuild_vacuous() {
        use crate::fast_model::spatial_state::SpatialTreeState as S;
        for state in [
            S::Uninitialized,
            S::Loading,
            S::Ready,
            S::ReadyEmpty,
            S::ReplayRequired,
            S::Rebuilding,
            S::DegradedReuse,
            S::DegradedBlocked,
        ] {
            let verdict = super::vacuous_room_build_reason(state);
            assert_eq!(
                verdict.is_some(),
                state == S::ReadyEmpty,
                "{}: {verdict:?}",
                state.as_str()
            );
        }
        let reason = super::vacuous_room_build_reason(S::ReadyEmpty).unwrap();
        assert!(
            reason.contains("空间树已验证为空") && reason.contains("发布事务"),
            "理由要说清楚为什么无事可做、以及房间归属从此靠什么收敛: {reason}"
        );
    }

    /// 空树那道门跳过时**盖章而不重建**：凭据没有比较对象的话 `/health.room_build`
    /// 会一直报「没有成功的全量房间重建凭据」，下次启动也只能判「跑」。盖章走
    /// `stamp_vacuous_room_build`（树非空拒绝），这里不许直接碰 `build_room_relations`。
    #[test]
    fn the_vacuous_gate_stamps_instead_of_rebuilding() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("async fn skip_startup_room_build()")
            .expect("门必须存在")
            .1
            .split_once("\npub mod api;")
            .expect("门之后是模块声明")
            .0;
        let branch = body
            .split_once("vacuous_room_build_reason(")
            .expect("空树那道门必须存在")
            .1
            .split_once("reconcile_startup_room_build().await")
            .expect("对账在它之后")
            .0;
        assert!(
            branch.contains("stamp_vacuous_room_build().await"),
            "平凡重建也要盖章: {branch}"
        );
        assert!(
            !branch.contains("build_room_relations"),
            "空树上不许跑全量重建: {branch}"
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

    /// 房间预加载只建立内存参照；启动主流程不再查询数据库做项目全算。
    #[test]
    fn startup_room_segment_only_releases_per_dbnum_memory_processing() {
        let source = include_str!("lib.rs");
        let segment = source
            .split_once(r#"println!("房间关键字为: {:?}""#)
            .expect("启动房间段必须存在")
            .1
            .split_once("let aios_mgr = AiosDBMgr {")
            .expect("房间段之后是 aios_mgr")
            .0;
        assert!(
            segment.contains("房间计算采用逐 dbnum 实时内存模式"),
            "启动段必须明确释放逐库实时内存处理: {segment}"
        );
        for forbidden in [
            "reconcile_rooms_with_resident_model",
            "skip_startup_room_build",
            "build_room_relations(",
            "build_room_relations_with_view(",
            "startup_room_barrier",
            "begin_startup_rooms",
        ] {
            assert!(
                !segment.contains(forbidden),
                "启动段不再走项目级房间路径（{forbidden}）: {segment}"
            );
        }
    }

    /// `run_cli` 的启动段不允许恢复任何项目级全量房间调用。
    #[test]
    fn startup_segment_has_zero_project_room_calculation_calls() {
        let source = include_str!("lib.rs");
        let run = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli boundary")
            .0;
        assert!(!run.contains("build_room_relations("));
        assert!(!run.contains("build_room_relations_with_view("));
        assert!(!run.contains("startup_room_reconcile("));
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

    /// ADR-058 D7：数据链与模型链并行。启动模型复核只读文件与 `gen_root`，必须排在
    /// 数据等待**之前**；主线仍按「数据就绪 → 模型收敛 → 房间」的顺序等，因为房间吃
    /// 模型产物、收敛宣告要等数据尾事务不再往积压里添行。2026-09-03 之前这里钉的是
    /// `data < open_model`（模型面等数据），那正是 D7 要拆掉的串行。
    #[test]
    #[ignore = "启动房间段已被启动对比重排（房间计算逐 dbnum 实时内存模式），本钉的标记已从 run_cli 撤走、\
                钉未随迁：由房间流随 T4/启动对比收口反向重钉（d-73、mem-107、changelog 2026-09-09）"]
    fn startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let worker = body.find("ensure_batch_worker(mgr.clone())").unwrap();
        let web = body.find("serve_if_configured(mgr)").unwrap();
        let reconcile = body
            .find("reconcile_model_coverage_at_startup().await")
            .unwrap();
        let data = body.find("wait_for_data_ready().await").unwrap();
        // 等待包在带周期播报的 timeout 循环里，锚点只认调用本身，不认 `.await`。
        let model_ready = body.find("wait_for_model_ready()").unwrap();
        let room = body.find("build_room_relations(&db_option).await").unwrap();
        assert!(
            worker < reconcile,
            "复核排出的工作项由 worker 消化，worker 先起"
        );
        assert!(
            reconcile < data,
            "启动模型复核不等数据摄入（ADR-058 D7）：它读的是文件与 gen_root"
        );
        assert!(
            web < data,
            "健康接口先起，数据等待期间外部要看得见初始化快照"
        );
        assert!(
            data < model_ready && model_ready < room,
            "主线仍按 数据就绪 → 模型收敛 → 房间 等：房间吃模型产物"
        );
    }

    /// ADR-060 D10：模型 / 房间水位的兼容播种夹在两道门之间——索引与 anc/dbnum 回填**之后**
    /// （投影探针的 `WHERE in = $parent.id` 没有 `idx_inst_relate_in` 时是逐 pe 行全表扫），
    /// 启动模型核对**之前**（否则那一轮会给这些库排从 0 起的恢复 Task，接管等于没做）。
    /// 失败只提示不上抛：这一步是幂等填空，下次启动重来。
    #[test]
    fn legacy_model_watermark_seeding_runs_after_the_indices_and_before_the_model_reconcile() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let indices = body
            .find("init_inst_relate_indices().await")
            .expect("索引初始化");
        let backfill = body
            .find("backfill_inst_relate_anc().await")
            .expect("anc/dbnum 回填");
        let seed = body
            .find("DbnumState::seed_legacy_model_watermarks().await")
            .expect("模型 / 房间水位兼容播种必须挂进启动");
        let reconcile = body
            .find("reconcile_model_coverage_at_startup().await")
            .expect("启动模型核对");
        assert!(
            indices < backfill && backfill < seed && seed < reconcile,
            "顺序必须是 索引 → 回填 → 兼容播种 → 启动模型核对"
        );
        let seed_block = &body[seed..reconcile];
        assert!(
            seed_block.contains("legacy_model_watermark_seed_lines(&seeds)")
                && seed_block.contains("模型 / 房间水位兼容播种失败（下次启动重试）"),
            "播种要出声、失败不上抛: {seed_block}"
        );
    }

    /// 常驻房间模型第 5 轮 Q17 (a) / Q18 (a)：空间树装载与房间初始化排在 watcher / worker
    /// **之前**，房间初始化紧跟树装载；两者之间与初始化本体都不许出现 `startup_data_ready` /
    /// 上弦位 / `startup_autorun` / `model_incremental` 任何一道门——房间是参照系，不是增量的
    /// 一部分；写归属边的启动对比与全量修复仍在最后的房间段受门。树装载只此一处，不许在老位置
    /// 留一份副本。
    #[test]
    fn resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let web = body.find("serve_if_configured(mgr)").unwrap();
        let rooms = body
            .find("prepare_room_resident_at_startup(&db_option).await")
            .expect("共享房间准备必须接进 ingest 启动序");
        let watcher = body.find("mgr.init_watcher().await").unwrap();
        let worker = body.find("ensure_batch_worker(mgr.clone())").unwrap();
        let data = body.find("wait_for_data_ready().await").unwrap();
        assert!(
            web < rooms && rooms < watcher && watcher < worker && worker < data,
            "顺序是 健康接口 → 共享房间准备 → watcher → worker → 等数据"
        );
        // 注释里可以说「不看哪几道门」，代码行里不许出现它们。
        let code = |text: &str| -> String {
            text.lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let helper = code(
            source
                .split_once("async fn prepare_room_resident_at_startup(")
                .expect("共享准备入口存在")
                .1
                .split_once("pub const SKIP_STARTUP_ROOM_BUILD_ENV")
                .expect("入口之后是止血口常量")
                .0,
        );
        for forbidden in [
            "startup_data_ready",
            "is_auto_work_armed",
            "startup_autorun",
            "model_incremental",
            "skip_startup_room_build",
            "build_room_relations",
        ] {
            assert!(
                !helper.contains(forbidden),
                "房间初始化本体不看这道门、不写归属边（{forbidden}）: {helper}"
            );
        }
        assert!(
            helper.contains("load_project_tree_verified().await")
                && helper.contains("initialize_resident_rooms_at_startup(db_option).await"),
            "共享入口必须先装空间树再初始化 resident: {helper}"
        );
        assert!(
            helper.find("load_project_tree_verified().await").unwrap()
                < helper
                    .find("initialize_resident_rooms_at_startup(db_option).await")
                    .unwrap(),
            "空间树必须先于 resident 构建: {helper}"
        );
        let read_through = source
            .split_once("async fn run_read_through_core(")
            .expect("read-through core exists")
            .1
            .split_once("pub async fn run_cli(")
            .expect("read-through core end exists")
            .0;
        assert!(
            read_through
                .find("serve_if_configured(manager).await")
                .unwrap()
                < read_through
                    .find("prepare_room_resident_at_startup(&db_option).await")
                    .unwrap(),
            "read-through 必须先开放健康探针；房间写入口由 resident ready 门拦住"
        );
    }

    /// 启动凭证复核必须先占住模型完成门，再让唯一消费者（batch worker）起来；
    /// 补偿核对在 `run_cli` 顶层无条件执行（ADR-050：清空无条件，补偿也无条件），
    /// 不许再躲回任何 `if startup_data_ready` 块里（审核 P1/P2）。
    #[test]
    fn startup_reconcile_is_armed_before_the_worker_and_runs_unconditionally() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let configure = body
            .find("initialization.configure_model_bootstrap(false)")
            .expect("模型门配置必须存在");
        let begin = body
            .find("initialization.begin_startup_reconcile();")
            .expect("启动复核必须先占住完成门");
        let worker = body.find("ensure_batch_worker(mgr.clone())").unwrap();
        assert!(
            configure < begin && begin < worker,
            "顺序必须是 配置模型门 → 占住完成门 → 起 worker，否则第一圈空闲轮可能抢先宣告收敛"
        );
        assert!(
            body.contains(
                "\n    crate::data_interface::startup_reconcile::reconcile_model_coverage_at_startup().await;\n"
            ),
            "覆盖核对必须在 run_cli 顶层（4 空格缩进、不在任何 if 内）无条件执行（ADR-050）"
        );
        let reconcile = body
            .find("reconcile_model_coverage_at_startup().await")
            .unwrap();
        assert!(
            worker < reconcile,
            "worker 必须先起来：复核落地后排出的工作项由它消化"
        );
    }

    /// Read-through startup must have an explicit source guard: it marks the
    /// file-side service ready without entering the startup ingest sweep.
    #[test]
    fn read_through_startup_cannot_enter_the_ingest_sweep() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        // watcher 无条件起（ADR-058 D8），read-through 的分野只剩「跳不跳启动重扫」——
        // 那一支现在是唯一还看形态的地方，两句话各自钉一处。
        assert!(
            body.contains("if crate::options::data_face().is_read_through() {\n        println!(\n            \"read-through：跳过启动数据重扫"),
            "read-through 必须跳过启动重扫，其余形态照常 init_watcher"
        );
        assert!(
            body.contains("if crate::options::data_face().is_read_through() {\n        initialization.mark_data_ready_without_manifest();"),
            "read-through must mark data_ready without an ingest manifest"
        );
    }

    /// A read-through snapshot advance must treat a vanished generation root as a
    /// tombstone. Feeding it back to `generate_roots_report` retains its previous
    /// records by design on generation failure and creates a ghost pipeline.
    #[test]
    fn read_through_snapshot_retires_roots_missing_from_the_new_skeleton() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("async fn run_read_through_core(")
            .expect("read-through core exists")
            .1
            .split_once("pub async fn run_cli(")
            .expect("read-through core end exists")
            .0;
        let partition = body
            .find(".partition(|root|")
            .expect("previous roots must be partitioned against the new skeleton");
        let retire = body
            .find("delete_model_subtree(requested).await")
            .expect("missing roots must retire their projection");
        let regenerate = body
            .find("ModelRefreshPolicy::generate_roots_report(")
            .expect("live roots must still regenerate");
        assert!(partition < retire && retire < regenerate);
        assert!(body.contains("snapshot.contains("));
    }

    /// 模型门返回 false 只能说明「此刻还开不了」（重扫刚发现新会话、数据回到未就绪），
    /// 不许再把整段模型等待连同收敛横幅一起跳过（审核 P1）。返回值只用于播报，
    /// 之后照常叫醒调度器并进入等待循环；门由 `install_manifest` 在清单就绪时自动打开。
    #[test]
    fn the_model_gate_open_result_is_never_used_as_a_skip() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        assert!(
            !body.contains("if initialization.open_model_phase() {"),
            "open_model_phase 的返回值不得再把模型等待整段跳过（P1）"
        );
        let gate = body
            .find("if !initialization.open_model_phase() {")
            .expect("模型门返回值只用于播报");
        let after = &body[gate..];
        let wake = after
            .find("scheduler.wake();")
            .expect("开门尝试之后必须叫醒调度器");
        let wait = after
            .find("wait_for_model_ready()")
            .expect("模型收敛等待必须无条件可达");
        assert!(
            wake < wait,
            "无论门此刻开没开，都要叫醒调度器并进入收敛等待"
        );
    }

    /// ADR-058 D8 / refactor plan S5（2026-09-04）：`direct_read_mode` 在启动序列里只回答
    /// 「数据面读哪儿」。模型门 / 收敛等待 / 房间不再有 direct 专用分支（direct 曾在复核落地后
    /// 立刻 `mark_model_ready`，积压还在跑就宣告收敛，并跳过房间）。2026-09-02 那条「direct
    /// 也起 worker」的钉留在本测试里：worker 启动不由 direct 把门。
    ///
    /// 2026-09-07 起 `sync_live` 整个退役（同一条 D8）：watcher **无条件起**，这里连带钉
    /// 「那个字段不许回来」——它此前把「起不起 watcher」与「做不做启动重扫」绑在一个布尔上，
    /// 后者现在由 `data_face` 单独回答。
    ///
    /// 2026-09-08 起（refactor plan S4 / Q24）legacy db 读路由整个退役：`direct_read_mode()`
    /// 这个函数不存在了，启动序列里**一处**都不许再出现它——数据面只有一条路，没有可选的东西。
    #[test]
    fn direct_read_mode_only_selects_the_data_read_route() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        assert!(
            !body.contains(&format!("db_option.{}", "sync_live")),
            "sync_live 已退役（ADR-058 D8）：启动序列不许再读它"
        );
        assert!(
            body.contains("let watch_task = {"),
            "看门狗无条件 spawn，不许再挂在任何开关上"
        );
        let worker = body
            .find("ensure_batch_worker(mgr.clone())")
            .expect("worker 必须在启动序列里起");
        // 注释行里可以提历史，代码行不行。
        let code_lines = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| line.contains("direct_read_mode"))
                .map(|line| line.trim().to_string())
                .collect()
        };
        assert!(
            code_lines(&body[..worker]).is_empty(),
            "legacy db 读路由退役后（ADR-058 D8，2026-09-08 S4），启动序列不许再问 direct_read_mode: {:?}",
            code_lines(&body[..worker])
        );
        assert!(
            body.contains("数据读取模式：direct（e3d-io，唯一形态）"),
            "读路由只剩一条也要在启动时说一声，读日志的人不必去翻 ADR"
        );
        // `body` 一直延到 `run_app`，中间夹着完成横幅那个函数：它只把读模式当标签打一行，
        // 不在本条的管辖内，启动序列到 `run_cli` 的 `Ok(())` 为止。
        let run_cli_tail = body[worker..]
            .split_once("\nfn print_startup_complete_banner(")
            .expect("完成横幅函数紧跟 run_cli")
            .0;
        assert!(
            code_lines(run_cli_tail).is_empty(),
            "worker 起来之后的启动序列（模型门 / 收敛等待 / 房间）不得再看 direct_read_mode（S5-2）: {:?}",
            code_lines(run_cli_tail)
        );
        let guard = body[..worker].rfind("if ").unwrap();
        assert!(
            !body[guard..worker].contains("direct_read_mode"),
            "worker 启动不得由 direct_read_mode 把门: {}",
            &body[guard..worker]
        );
        assert!(
            !run_cli_tail
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| line.contains("mark_model_ready()")),
            "启动主线不得自己宣告模型收敛；收敛由 worker 空闲轮判定（S5-2）"
        );
    }

    /// ADR-071 不变量 S5（任务与库之间没有队列）在启动序列上的钉：`run_cli` 不得再打开本地
    /// journal、对账 Prepared 窗口、刷新数据库绑定或起结构备份 lane——那是 (a) 的异步账本
    /// （共识 d-285 / d-288 已作废）。数据窗口的结构行由任务自己在 `apply_one` 里同步落库。
    #[test]
    fn startup_neither_opens_a_local_journal_nor_starts_a_backup_lane() {
        let source = include_str!("lib.rs");
        let body = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        assert!(
            body.contains("DbnumState::hydrate_watermark_mirror().await?"),
            "启动仍要从库水化水位镜像（整表替换只在这一步）"
        );
        for forbidden in [
            "increment_local_runtime(",
            "reconcile_prepared_data_windows(",
            "record_startup_reconciliation(",
            "bind_database(",
            "start_increment_backup_lane(",
            "observe_database_applied(",
        ] {
            assert!(
                !body.contains(forbidden),
                "run_cli 不得再有本地账本 / lane 的接线：{forbidden}"
            );
        }
    }

    /// ADR-071 D7 / S6 的统一水化段：全部播种（ADR-001 的 `ensure_increment_state_storage`、
    /// ADR-060 D10 的 `seed_legacy_model_watermarks`）之后、启动模型核对之前，按「三水位镜像 →
    /// 模型投影 → 一次性迁移」一次做完；`run_app` 不得再在连库后自己先水化一遍。
    #[test]
    fn startup_hydrates_mirrors_once_after_seeding_and_before_the_model_reconcile() {
        let source = include_str!("lib.rs");
        let cli = source
            .split_once("pub async fn run_cli(")
            .expect("run_cli exists")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli end exists")
            .0;
        let at = |needle: &str| cli.find(needle).unwrap_or_else(|| panic!("缺 {needle}"));
        let state_storage = at("DbnumState::ensure_increment_state_storage().await?");
        let seeding = at("DbnumState::seed_legacy_model_watermarks().await");
        let rollback = at("startup_reconcile::rollback_unfinished_writes().await");
        let mirror = at("DbnumState::enable_watermark_mirror();");
        let states = at("DbnumState::hydrate_watermark_mirror().await?");
        let projections = at("hydrate_model_projections(&hydrated_states).await");
        let migration = at("retire_publish_run_tables().await");
        let reconcile = at("reconcile_model_coverage_at_startup().await");
        assert!(
            state_storage < seeding
                && seeding < rollback
                && rollback < mirror
                && mirror < states
                && states < projections
                && projections < migration
                && migration < reconcile,
            "水化段顺序：状态表播种 → 兼容播种 → 写入标记回退 → 开镜像 → 读全水位 → 投影 → 迁移 → 启动核对"
        );
        // ADR-072 D2：回退必须落在整表水化之前。先水化再回退，发布出去的就是一版
        // 已经被删掉痕迹的水位，而那几格再没有人来作废。
        assert!(
            rollback < states,
            "写入标记回退必须排在 hydrate_watermark_mirror 之前"
        );
        assert_eq!(
            cli.matches("hydrate_model_projections(").count(),
            1,
            "投影水化只做一次"
        );
        assert_eq!(
            cli.matches("hydrate_watermark_mirror(").count(),
            1,
            "镜像整表水化只做一次；运行期读全表走 list_registered，它不碰镜像"
        );
        let app = source
            .split_once("pub async fn run_app(")
            .expect("run_app exists")
            .1;
        let app = app.split_once("\n}\n").map_or(app, |(body, _)| body);
        assert!(
            !app.contains("hydrate_model_projections("),
            "run_app 不得抢在 run_cli 之前水化投影（播种还没跑）"
        );
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

/// ADR-072 W4：退役表名在 `src/` 里**只许**出现在 [`retire_publish_run_tables`] 的
/// `REMOVE TABLE IF EXISTS` 清单里（计划 §5 R4 / §6-4 验收）。
///
/// 扫的是 `src/` 下每个 `.rs` 的**生产部分**——`#[cfg(test)]` 块整段摘掉、整行注释去掉之后
/// 的文本。历史注释里提这些名字是在解释「为什么没了」，不算回来了；各文件自己的源码钉
/// 本来就用 `concat!` 拆着写表名。`room_topology_source` 不在名单里：那个名字同时是磁盘
/// 分片模块（d-89），表退了、模块留着。
#[cfg(test)]
mod retired_table_names_pin {
    use std::path::{Path, PathBuf};

    const RETIRED_TABLES: [&str; 6] = [
        "model_update_pending",
        "incr_side_effect_pending",
        "published_file_input",
        "increment_update_attempt",
        "room_published_inst",
        "room_published_topology",
    ];

    fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// 摘掉每个 `#[cfg(test)]` 引导的条目：带花括号的（`mod tests { … }` / 带体的 fn）按
    /// 花括号配对整段去掉，不带的（`#[cfg(test)] use …;`）去到分号。
    fn without_cfg_test_items(source: &str) -> String {
        let mut out = String::new();
        let mut rest = source;
        while let Some(idx) = rest.find("#[cfg(test)]") {
            out.push_str(&rest[..idx]);
            let after = &rest[idx..];
            let brace = after.find('{');
            let semi = after.find(';');
            rest = match (brace, semi) {
                (Some(b), Some(s)) if s < b => &after[s + 1..],
                (None, Some(s)) => &after[s + 1..],
                (Some(b), _) => {
                    let mut depth = 0usize;
                    let mut end = None;
                    for (i, ch) in after[b..].char_indices() {
                        match ch {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = Some(b + i + 1);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    end.map_or("", |e| &after[e..])
                }
                (None, None) => "",
            };
        }
        out.push_str(rest);
        out
    }

    /// 参与检查的文本：去测试块、去整行注释（`//` / `///` / `//!`）。
    fn production_code(source: &str) -> String {
        without_cfg_test_items(source)
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 整个文件都是测试的模块：父模块里以 `#[cfg(test)] mod name;` 声明的那些
    /// （`direct_window_replay_parity` / `room_increment_audit` …），文件里自己不再写
    /// `#[cfg(test)]`，只能从声明处认出来。
    fn test_only_files(files: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for path in files {
            let source = std::fs::read_to_string(path).expect("read source");
            let dir = path.parent().expect("source has a parent");
            let mut rest = source.as_str();
            while let Some(idx) = rest.find("#[cfg(test)]") {
                rest = &rest[idx + "#[cfg(test)]".len()..];
                let head = rest.trim_start();
                let Some(head) = head
                    .strip_prefix("pub(crate) ")
                    .or_else(|| head.strip_prefix("pub "))
                    .or(Some(head))
                    .and_then(|h| h.strip_prefix("mod "))
                else {
                    continue;
                };
                let Some(name) = head.split(';').next().map(str::trim) else {
                    continue;
                };
                if name.is_empty() || name.contains('{') {
                    continue;
                }
                out.push(dir.join(format!("{name}.rs")));
                out.push(dir.join(name).join("mod.rs"));
            }
        }
        out
    }

    #[test]
    fn retired_table_names_only_survive_in_the_remove_table_migration() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);
        files.sort();
        assert!(
            files.len() > 100,
            "src/ 下应当有上百个 .rs，只扫到 {}",
            files.len()
        );
        let test_only = test_only_files(&files);
        assert!(
            test_only
                .iter()
                .any(|p| p.ends_with("direct_window_replay_parity.rs")),
            "整文件测试模块的识别要认得出 direct_window_replay_parity: {test_only:?}"
        );

        let mut offenders = Vec::new();
        for path in &files {
            if test_only.contains(path) {
                continue;
            }
            let source = std::fs::read_to_string(path).expect("read source");
            let mut code = production_code(&source);
            if path.parent() == Some(src.as_path()) && path.ends_with("lib.rs") {
                // 唯一许可的出现点：`retire_publish_run_tables` 的 RETIRED 清单。
                let (head, rest) = code
                    .split_once("const RETIRED: [&str; ")
                    .expect("retire_publish_run_tables 的 RETIRED 清单必须在");
                let (_, tail) = rest.split_once("];").expect("RETIRED 清单要收口");
                code = format!("{head}{tail}");
            }
            let shown = path
                .strip_prefix(&src)
                .unwrap_or(path)
                .display()
                .to_string();
            for table in RETIRED_TABLES {
                for line in code.lines().filter(|line| line.contains(table)) {
                    offenders.push(format!("{shown}: `{table}` — {}", line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "ADR-072 W4：退役表名只许出现在 REMOVE TABLE IF EXISTS 清单里，生产代码里还有：\n{}",
            offenders.join("\n")
        );
    }

    /// 摘测试块的办法要对得住 lib.rs 自己：三个 `#[cfg(test)] mod` 散在文件中段，摘掉之后
    /// `retire_publish_run_tables` 仍在、这个测试自己不在。
    #[test]
    fn the_cfg_test_stripper_keeps_production_and_drops_every_test_module() {
        let source = include_str!("lib.rs");
        let production = without_cfg_test_items(source);
        assert!(production.contains("async fn retire_publish_run_tables()"));
        assert!(!production.contains("mod retired_table_names_pin"));
        assert!(!production.contains("mod startup_room_build_gate_tests"));
        assert!(!production.contains("mod process_instance_lock_tests"));
        assert_eq!(
            without_cfg_test_items("a\n#[cfg(test)]\nuse x::y;\nb"),
            "a\n\nb"
        );
    }
}
