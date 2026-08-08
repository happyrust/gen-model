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
#[cfg(feature = "gen_model")]
use crate::fast_model::gen_all_geos_data;
use crate::fast_model::room_model::build_room_relations;
use crate::fast_model::{EXIST_MESH_GEO_HASHES, gen_inst_meshes, process_meshes_update_db_deep};
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

#[cfg(windows)]
struct ProcessInstanceLock {
    /// Keeping this handle alive keeps Windows' deny-share lock alive.
    _file: File,
    path: PathBuf,
    project: String,
}

#[cfg(windows)]
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

#[cfg(windows)]
fn acquire_process_instance_lock(db_option: &DbOption) -> anyhow::Result<()> {
    let project = db_option.project_name.clone();
    let held = PROCESS_INSTANCE_LOCK.get_or_init(|| {
        let root = crate::data_interface::project_paths::resolve_project_root(db_option, &project)
            .ok_or_else(|| format!("未解析到项目 {project} 的单实例锁目录"))?;
        let path = root.join(".gen-model.instance.lock");
        let mut file = open_process_instance_lock(&path).map_err(|error| {
            format!(
                "项目 {project} 已有 gen-model 实例，或单实例锁不可访问（{}）: {error}",
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

#[cfg(not(windows))]
fn acquire_process_instance_lock(_db_option: &DbOption) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod process_instance_lock_tests {
    use super::open_process_instance_lock;

    #[test]
    fn deny_share_handle_blocks_a_second_process_style_open_until_drop() {
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
            "share_mode(0) must reject a concurrent opener"
        );
        drop(first);
        let reopened = open_process_instance_lock(&path).expect("lock releases with handle");
        drop(reopened);
        let _ = std::fs::remove_file(path);
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

// RVM 基准对拍。注意与同名但无关的 `src/rvm/`（PDMS 元素遍历）区分。
#[cfg(feature = "rvm_verify")]
pub mod rvm_baseline;

#[cfg(feature = "gen_model")]
pub mod fast_model;

pub mod surreal_retry;

pub mod versioned_db;

pub mod mqtt_service;

pub mod options;

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

pub async fn run_cli(db_option: DbOption) -> anyhow::Result<()> {
    // Must precede logging, schema repair, watcher startup and every model/
    // room write. A second process exits here instead of becoming another
    // consumer of the same durable pending table.
    acquire_process_instance_lock(&db_option)?;
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

    aios_core::function::define_common_functions().await?;
    // D11（ADR-010）：define_common_functions 按文件名顺序无条件加载 resource/surreal
    // 全目录，`fn_query_room_code_hh.surql` 排在 `_hd` 版之后，同名 fn::room_code 永远
    // 被 hh 版覆盖——与 Rust 侧编译的 project_hd feature 错位。加载顺序在 rs-core 里
    // 改不到，这里在加载完成后按 feature 重放正确版本，把覆盖再覆盖回来。
    // project_hh 构建无需处理：hh 版本来就是最后加载的那份。
    #[cfg(feature = "project_hd")]
    {
        const HD_ROOM_CODE: &str = "resource/surreal/fn_query_room_code.surql";
        match std::fs::read_to_string(HD_ROOM_CODE) {
            Ok(text) => match SUL_DB.query(text).await {
                Ok(_) => println!("已按 project_hd 重放 fn::room_code（矫正 _hh 文件的覆盖）"),
                Err(e) => eprintln!("重放 hd 版 fn::room_code 失败（生效的仍是 hh 版）: {e}"),
            },
            Err(e) => eprintln!("读取 {HD_ROOM_CODE} 失败（生效的仍是 hh 版 fn::room_code）: {e}"),
        }
    }
    crate::data_interface::increment_pipeline::selfcheck_surreal_functions().await?;
    let migrated =
        crate::data_interface::dbnum_state::DbnumState::ensure_increment_state_storage().await?;
    println!("增量状态表检查完成（兼容检查 {migrated} 个旧 DBNUM 水位）");
    // 解析完成后重新定义EVENT
    println!("正在重新定义dbnum_event...");
    match define_dbnum_event().await {
        Ok(_) => println!("成功重新定义update_dbnum_event"),
        Err(e) => println!("重新定义update_dbnum_event失败: {:?}", e),
    }
    println!("预加载方法完成。");

    // 初始化数据库索引
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

    let sync_live = db_option.sync_live.unwrap_or(false);
    let db_option = Arc::new(db_option.clone());
    // initialize_global_db_sender().await;

    // start_sync_task(db_option.clone(), progress_sender.clone()).await?;
    //如果是解析任务，运行完就应该跳出
    if db_option.total_sync
        || db_option.incr_sync
        || db_option.only_sync_sys
        || db_option.is_sync_history()
    {
        // println!("开始同步解析数据。");
        // tokio::spawn(async move {
        if let Err(e) = sync_pdms(&db_option).await {
            eprintln!("同步PDMS数据失败: {}", e);
        }
        //记录进度
        // progress_sender.send(90)?;
        if db_option.build_cate_relate() {
            println!("初始化创建Cate relate关系");
            build_cate_relate(false).await?;
        }
        // progress_sender.send(100)?;
    }

    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    /// 创建db manager
    match crate::data_interface::batch_scheduler::BatchScheduler::global()
        .restore_persisted_pause()
        .await
    {
        Ok(true) => println!("队列处于暂停状态（重启前设置），启动重扫只入队不消费"),
        Ok(false) => {}
        Err(error) => println!("启动时恢复队列暂停标志失败（worker 启动前会重试）: {error:#}"),
    }
    if sync_live {
        mgr.init_watcher().await?;
    }

    // 启动加载：sidecar epoch 与库一致才信项目树文件，否则从库指针重建
    // （ADR-010 §6 修订：裸文件搬运与条数对账一并退役）。
    // 空间树是可重建的派生数据，加载失败不该顶掉整个启动：空树有下游防线
    // （全量重建拒跑、整间分支拒算），worker 启动收敛还会重放未完成的空间意图。
    if let Err(error) = crate::fast_model::aabb_tree::load_project_tree_verified().await {
        eprintln!("空间树启动加载失败（{error:#}），以空树启动，等待修复后重建");
    }
    // progress_sender.send(10)?;
    //todo 还有个问题，可能需要通过队列来排队任务
    //如果没有生成完，需要等待
    if db_option.is_gen_mesh_or_model() {
        println!("正在生成模型");
        let mut time = Instant::now();
        fs::create_dir_all("assets/meshes")?;
        //统计一下assets mesh 目录下有多少个mesh，直接忽略去生成
        let path: PathBuf = "assets/meshes".into();
        //收集目录下的文件名
        // let paths = fs::read_dir(path).unwrap();
        // for entry in paths {
        //     let entry = entry.unwrap();
        //     let path = entry.path();
        //     let geo_hash = path
        //         .file_stem()
        //         .unwrap()
        //         .to_str()
        //         .unwrap()
        //         .to_string();
        //     // 反序列成PlantMesh
        //     if let Ok(mesh) = PlantMesh::des_mesh_file(&geo_hash) && let Some(aabb) = mesh.aabb{
        //         EXIST_MESH_GEO_HASHES.insert(geo_hash, aabb);
        //     }
        // }
        gen_all_geos_data(&db_option).await?;
        //保存
        // println!("生成完所有模型花费时间: {} ms", time.elapsed().as_millis());
    }

    println!("房间关键字为: {:?}", db_option.get_room_key_word());
    // 快速重启 / 仅靠增量收敛时可跳过启动全量房间重建（本项目 2 万面板级、很重）。
    // 增量队列照常入队与消费，房间归属靠增量收敛。
    if std::env::var("AIOS_SKIP_STARTUP_ROOM_BUILD").is_ok() {
        println!(
            "AIOS_SKIP_STARTUP_ROOM_BUILD 已设置：跳过启动全量房间重建，房间归属仅靠增量队列收敛"
        );
    } else {
        println!("正在生成空间树");
        println!("正在计算房间");
        println!(
            "房间空间数的数量为: {}",
            GLOBAL_AABB_TREE.read().await.tree.size()
        );
        let mut time = Instant::now();
        // 单块面板算不出来不该拦住启动：这里在 `async_watch` 之前，panic 等于整个服务
        // 起不来，而房间归属是可以事后重建的派生数据。函数内已按面板逐条聚合失败原因，
        // 打出来即可定位——此前那些失败是被 `unwrap_or_default()` 吞成「这间房 0 个成员」的。
        if let Err(error) = build_room_relations(&db_option).await {
            eprintln!("计算房间未完全成功: {error:#}");
        }
        println!("计算房间花费时间: {} ms", time.elapsed().as_millis());
        // update_cal_equip().await?;
        update_cal_bran_component().await?;
    }

    let aios_mgr = AiosDBMgr::init_from_db_option().await?;
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
                dbg!(&e.to_string());
            }
        }
    }

    if db_option.rebuild_ssc_tree {
        dbg!("生成pbs节点");
        set_pdms_major_code(&aios_mgr).await?;
        let mut handles = vec![];
        set_pbs_fixed_node(&mut handles).await?;
        let rooms = set_pbs_room_node(&mut handles).await?;
        set_pbs_room_major_node(&rooms, &mut handles).await?;
        set_pbs_node(&mut handles).await?;
        futures::future::join_all(handles).await;
    }

    // 数据批次队列的唯一消费者：无条件启动、不分 sync_live（ADR-011；rollout
    // 第九节第 5 条）——合流后手动模式的执行也走队列，worker 若只活在自动分支，
    // 手动模式的队列就没有消费者。刻意放在全量生成 / 房间重建**之后**：批次执行
    // 与 `gen_all_geos_data` 并发会在同一批生成根上互踩；sync_live 启动重扫入队
    // 的批次会等到这里才开始被消费。
    crate::data_interface::batch_worker::ensure_batch_worker(mgr.clone());

    // Web 服务（REST + WebSocket）：配置了 http_api_addr 才真正监听；
    // 与 async_watch 并行运行，未启用时零影响（docs/specs/web-service-api.md）。
    #[cfg(feature = "http_api")]
    let web_task = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::web_service::serve_if_configured(mgr).await {
                eprintln!("Web 服务异常退出: {e:?}");
            }
        })
    };

    if sync_live {
        // cur_mgr.clone().unwrap().async_watch().await.unwrap();

        //todo 如何处理初始化的同步，第一次启动一定要同步一次，首先生成archive文件，然后再同步
        //是否需要重构下面的这行代码？
        // 看门狗退出必须留下痕迹，不能把 Result 直接丢掉（T903）。
        #[cfg(feature = "mqtt")]
        {
            let (watch_result, _) = tokio::join!(
                mgr.async_watch(),
                AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
            );
            if let Err(e) = watch_result {
                log::error!("async_watch 退出，增量看门狗已停止: {e:?}");
                eprintln!("async_watch 退出，增量看门狗已停止: {e:?}");
            }
        }
        #[cfg(not(feature = "mqtt"))]
        if let Err(e) = mgr.async_watch().await {
            log::error!("async_watch 退出，增量看门狗已停止: {e:?}");
            eprintln!("async_watch 退出，增量看门狗已停止: {e:?}");
        }
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

/// 运行app
pub async fn run_app(option: Option<DbOptionExt>) -> anyhow::Result<()> {
    use std::sync::mpsc;

    use aios_core::init_surreal;
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
        match init_surreal().await {
            Ok(_) => {
                println!(
                    "数据库已经连接到 {}, 站点: {}",
                    db_option.project_name,
                    db_option.get_version_db_conn_str()
                );
            }
            Err(e) => {
                dbg!(&e.to_string());
            }
        }
    }

    // epoch 校验通过才信项目树文件，失配从库指针重建；条数对账
    // （sync_aabb_tree_with_db）退役为手工诊断工具（ADR-010 §6 修订）。
    crate::fast_model::aabb_tree::load_project_tree_verified().await?;
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
