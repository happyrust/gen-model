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
use crate::fast_model::{gen_inst_meshes, process_meshes_update_db_deep, EXIST_MESH_GEO_HASHES};
use crate::versioned_db::database::*;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::options::DbOption;
use aios_core::pdms_data::AttInfoMap;
use aios_core::pdms_types::*;
use aios_core::room::room::{load_aabb_tree, GLOBAL_AABB_TREE};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::{build_cate_relate, pdms_types::*, SUL_DB};
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
use aios_core::material::save_all_material_data;
use versioned_db::database::{define_dbnum_event, sync_pdms};

use log::{error, LevelFilter};
use simplelog::*;

pub mod api;
pub mod cata;
pub mod consts;
pub mod data_interface;
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

pub mod versioned_db;

pub mod mqtt_service;

pub mod options;

// 添加options模块的重导出
pub use options::get_db_option_ext;
pub use options::DbOptionExt;

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

    aios_core::function::define_common_functions()
        .await
        .unwrap();
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
    }

    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    /// 创建db manager
    if sync_live {
        mgr.init_watcher().await?;
    }

    load_aabb_tree().await.unwrap();
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
        gen_all_geos_data(vec![], &db_option, None).await?;
        //保存
        // println!("生成完所有模型花费时间: {} ms", time.elapsed().as_millis());
    }

    if db_option.gen_spatial_tree {
        println!("房间关键字为: {:?}", db_option.get_room_key_word());
        println!("正在生成空间树");
        println!("正在计算房间");
        println!(
            "房间空间数的数量为: {}",
            GLOBAL_AABB_TREE.read().await.tree.size()
        );
        let mut time = Instant::now();
        build_room_relations(&db_option).await.unwrap();
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
    let db_option: DbOption = option
        .map(|o| o.inner)
        .unwrap_or_else(|| get_db_option().clone());
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

    if db_option.gen_spatial_tree {
        // Try to load existing AABB tree first
        load_aabb_tree().await?;

        // Check if tree is empty after loading
        if GLOBAL_AABB_TREE.read().await.is_empty() {
            println!("AABB tree is empty after loading, performing manual update...");
            manual_update_aabbs(true).await?;
            println!("Manual update aabb tree completed");
        }
    }
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
pub mod xkt_generator;
