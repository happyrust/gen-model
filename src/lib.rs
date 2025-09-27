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
// Removed GLOBAL_AABB_TREE dependency - using SQLite R*-tree instead
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
use std::fs::{self, File};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use surrealdb::opt::auth::Root;
use team_data::sync_team_data;
// use tokio::sync::mpsc::Sender;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use versioned_db::database::{define_dbnum_event, sync_pdms};

use log::{LevelFilter, error};
use simplelog::*;

pub mod api;
pub mod cata;
pub mod consts;
pub mod data_interface;
pub mod expression_fix;
pub mod tables;
// pub mod ssc;
pub mod defines;
pub mod team_data;

pub mod graph_db;
pub mod test;

#[cfg(feature = "gui")]
pub mod gui;

#[cfg(feature = "gen_model")]
pub mod fast_model;

// #[cfg(feature = "gen_model")]
// pub mod xeokit_xtk_generator; // 暂时注释掉，待实现

pub mod versioned_db;

pub mod mqtt_service;

pub mod options;

#[cfg(feature = "grpc")]
pub mod grpc_service;
pub mod spatial_index;

#[cfg(feature = "grpc")]
pub mod test_spatial_query;

// 添加options模块的重导出
pub use options::DbOptionExt;
pub use options::get_db_option_ext;

// 重新导出MDB相关函数供GRPC服务使用
// pub use crate::api::element::query_types_refnos_names;
// pub use crate::api::attr::{query_explicit_attr, query_numbdbs_by_mdb};

#[cfg(feature = "sql")]
pub use mdb::get_project_mdb;

// // 添加get_project_mdb函数的重新导出
// #[cfg(feature = "grpc")]
// pub async fn get_project_mdb(project_pool: &sqlx::Pool<sqlx::MySql>) -> anyhow::Result<dashmap::DashMap<String, Vec<u32>>> {
//     use crate::api::attr::{query_explicit_attr, query_numbdbs_by_mdb};
//     use crate::api::element::query_types_refnos_names;
//     use dashmap::DashMap;

//     let mut result = DashMap::new();
//     // 获取到所有的 mdb
//     let mdb = query_types_refnos_names(&vec!["MDB"], project_pool, None).await?;
//     for (mdb_refno, mut mdb_name) in mdb {
//         if mdb_name.starts_with("/") { mdb_name.remove(0); }
//         let mdb_attr = query_explicit_attr(mdb_refno, project_pool).await?;
//         let dbs = mdb_attr.get_refu64_vec("CURD");
//         if dbs.is_none() { continue; }
//         let dbs = dbs.unwrap();
//         let numbdbs = query_numbdbs_by_mdb(dbs, project_pool).await?;
//         result.entry(mdb_name).or_insert(numbdbs);
//     }
//     Ok(result)
// }

#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;

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

    if let Err(e) = aios_core::function::define_common_functions().await {
        eprintln!("初始化通用函数失败: {} (忽略并继续)", e);
    }
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
        return Ok(());
    }

    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    /// 创建db manager
    if sync_live {
        mgr.init_watcher().await?;
    }

    // SQLite R*-tree initialization is handled automatically
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
        gen_all_geos_data(vec![], &db_option, None, None).await?;
        //保存
        // println!("生成完所有模型花费时间: {} ms", time.elapsed().as_millis());
    }

    if db_option.gen_spatial_tree {
        println!("房间关键字为: {:?}", db_option.get_room_key_word());
        println!("正在生成空间树");
        println!("正在计算房间");
        // SQLite R*-tree will be used for spatial indexing
        let mut time = Instant::now();
        if let Err(e) = build_room_relations(&db_option).await {
            eprintln!("计算房间失败: {}", e);
            return Err(e);
        }
        println!("计算房间花费时间: {} ms", time.elapsed().as_millis());
        update_cal_equip().await?;
        update_cal_bran_component().await?;
    }

    let aios_mgr = AiosDBMgr::init_from_db_option().await?;
    // 生成材料表单
    let gen_material = db_option.gen_material.unwrap_or(false);
    if gen_material {
        // save_all_material_data().await?;
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

    if sync_live {
        // cur_mgr.clone().unwrap().async_watch().await.unwrap();

        //todo 如何处理初始化的同步，第一次启动一定要同步一次，首先生成archive文件，然后再同步
        //是否需要重构下面的这行代码？
        #[cfg(feature = "mqtt")]
        tokio::join!(
            mgr.async_watch(),
            AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
        );
        #[cfg(not(feature = "mqtt"))]
        mgr.async_watch().await;
    }

    Ok(())
}

/// 运行app
pub async fn run_app(option: Option<DbOptionExt>) -> anyhow::Result<()> {
    use std::sync::mpsc;

    use crate::fast_model::aabb_tree::manual_update_aabbs;
    use aios_core::init_surreal;

    // 如果传入的是DbOptionExt，则取其内部的DbOption
    let db_option_ext = option.unwrap_or_else(|| get_db_option_ext());
    let db_option: DbOption = db_option_ext.inner.clone();

    // 检查是否需要启动GRPC服务器
    #[cfg(feature = "grpc")]
    let start_grpc = std::env::var("AIOS_GRPC_ENABLED")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    #[cfg(feature = "grpc")]
    if start_grpc {
        // 在后台启动GRPC服务器
        let grpc_handle = tokio::spawn(async {
            if let Err(e) = crate::grpc_service::start_grpc_server().await {
                eprintln!("GRPC server error: {}", e);
            }
        });

        // 继续执行正常的应用逻辑，但不阻塞GRPC服务器
        let app_handle = tokio::spawn(async move { run_app_internal(db_option).await });

        // 等待任一任务完成
        tokio::select! {
            result = app_handle => result?,
            _ = grpc_handle => {},
        }

        return Ok(());
    }
    // 调用内部实现
    run_app_internal(db_option).await
}

/// 内部应用运行逻辑
async fn run_app_internal(db_option: DbOption) -> anyhow::Result<()> {
    use crate::fast_model::aabb_tree::manual_update_aabbs;
    use aios_core::SUL_DB;
    use aios_core::init_surreal;

    let config = surrealdb::opt::Config::default().ast_payload(); // 启用AST格式

    #[cfg(feature = "local")]
    SUL_DB
        .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
        .with_capacity(1000)
        .await?;

    println!("数据库连接中...");

    #[cfg(feature = "ws")]
    {
        // 使用改进的数据库连接初始化
        match init_surreal_with_retry(&db_option).await {
            Ok(_) => {
                println!(
                    "✅ 数据库连接成功: {} -> {}",
                    db_option.get_version_db_conn_str(),
                    db_option.project_name
                );
            }
            Err(e) => {
                eprintln!("❌ 数据库连接失败: {}", e);
                eprintln!("   配置信息: {}", db_option.get_connection_summary());
                eprintln!("   请检查 SurrealDB 服务是否运行，配置是否正确");
                // 不要直接返回错误，让应用继续运行但标记数据库不可用
            }
        }
    }

    if db_option.gen_spatial_tree {
        // SQLite R*-tree initialization is handled in spatial_index_builder
    }

    run_cli(db_option).await
}

/// 改进的数据库连接初始化，支持重试和详细错误诊断
async fn init_surreal_with_retry(db_option: &DbOption) -> anyhow::Result<()> {
    use aios_core::{SUL_DB, init_surreal};
    use std::time::Duration;

    // 验证配置
    db_option
        .validate_connection_config()
        .map_err(|e| anyhow::anyhow!("配置验证失败: {}", e))?;

    let max_retries = 3;
    let mut last_error = None;

    for attempt in 1..=max_retries {
        println!("🔄 数据库连接尝试 {}/{}", attempt, max_retries);

        match try_connect_database(db_option).await {
            Ok(_) => {
                println!("✅ 数据库连接成功");
                return Ok(());
            }
            Err(e) => {
                let error_msg = e.to_string();
                last_error = Some(anyhow::anyhow!("{}", error_msg));
                eprintln!("❌ 连接尝试 {} 失败: {}", attempt, error_msg);

                if attempt < max_retries {
                    let wait_time = attempt * 2;
                    println!("⏳ {}秒后重试...", wait_time);
                    tokio::time::sleep(Duration::from_secs(wait_time)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("连接失败")))
}

/// 尝试连接数据库的核心逻辑
async fn try_connect_database(_db_option: &DbOption) -> anyhow::Result<()> {
    use aios_core::{SUL_DB, init_surreal};

    println!("使用 aios_core::init_surreal 初始化数据库...");
    match init_surreal().await {
        Ok(_) => {
            println!("✓ 数据库初始化完成");
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Already connected") {
                println!("⚠️ 已经连接，跳过重复初始化");
            } else {
                return Err(anyhow::anyhow!("数据库初始化失败: {}", msg));
            }
        }
    }

    // 功能性验证
    SUL_DB
        .query("SELECT 1 as test")
        .await
        .map_err(|e| anyhow::anyhow!("测试查询失败: {}", e))?;

    println!("✓ 功能测试通过");
    Ok(())
}

/// DbOption 扩展 trait，提供验证和连接摘要功能
trait DbOptionValidation {
    fn validate_connection_config(&self) -> Result<(), String>;
    fn get_connection_summary(&self) -> String;
}

impl DbOptionValidation for aios_core::options::DbOption {
    fn validate_connection_config(&self) -> Result<(), String> {
        if self.v_ip.is_empty() {
            return Err("数据库IP不能为空".to_string());
        }

        if self.v_port == 0 {
            return Err("数据库端口不能为0".to_string());
        }

        if self.v_user.is_empty() {
            return Err("数据库用户名不能为空".to_string());
        }

        if self.project_name.is_empty() {
            return Err("项目名称不能为空".to_string());
        }

        // 检查端口范围
        if self.v_port > 65535 {
            return Err("数据库端口超出有效范围 (1-65535)".to_string());
        }

        // 检查项目代码
        if self.project_code.is_empty() || self.project_code == "0" {
            return Err("项目代码不能为空或0".to_string());
        }

        Ok(())
    }

    fn get_connection_summary(&self) -> String {
        format!(
            "连接信息: {}:{} (用户: {}, 项目: {}, 命名空间: {})",
            self.v_ip, self.v_port, self.v_user, self.project_name, self.project_code
        )
    }
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
#[cfg(feature = "web_ui")]
pub mod web_ui;
pub mod xkt_generator;
