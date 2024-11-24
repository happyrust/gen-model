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
use aios_core::get_db_option;
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
use team_data::sync_system_db;
// use tokio::sync::mpsc::Sender;
use std::sync::mpsc::Sender;
use versioned_db::database::sync_pdms;

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

pub mod gui;

#[cfg(feature = "gen_model")]
pub mod fast_model;

pub mod versioned_db;

pub mod mqtt_service;

#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;

#[macro_use]
extern crate strum_macros;

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

pub async fn run_cli(db_option: DbOption, progress_sender: Sender<i32>) -> anyhow::Result<()> {
    dbg!("begin run task");
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

    #[cfg(feature = "local")]
    SUL_DB
        .connect(format!("rocksdb://{}.rdb", db_option.project_name))
        .with_capacity(1000)
        .await?;
    #[cfg(feature = "ws")]
    {
        let mut need_login = true;
        if let Err(e) = SUL_DB
            .connect(db_option.get_version_db_conn_str())
            .with_capacity(1000)
            .await
        {
            if e.to_string().contains("Already connected") {
                println!("SurrealDB is already connected.");
                need_login = false;
            } else {
                return Err(e.into());
            }
        }
        if need_login {
            SUL_DB
                .use_ns(&db_option.surreal_ns)
                .use_db(&db_option.project_name)
                .await?;
            SUL_DB
                .signin(Root {
                    username: &db_option.v_user,
                    password: &db_option.v_password,
                })
                .await?;
        }
    }
    // progress_sender.send(5).await?;
    progress_sender.send(5)?;
    println!(
        "数据库已经连接到 {}, 站点: {}",
        db_option.project_name,
        db_option.get_version_db_conn_str()
    );
    aios_core::function::define_common_functions()
        .await
        .unwrap();
    println!("预加载方法完成。");
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
        if let Err(e) = sync_pdms(&db_option, progress_sender.clone()).await {
            eprintln!("同步PDMS数据失败: {}", e);
        }
        //记录进度
        progress_sender.send(90)?;
        if db_option.build_cate_relate() {
            println!("初始化创建Cate relate关系");
            build_cate_relate(false).await?;
        }
        progress_sender.send(100)?;
        return Ok(());
    }

    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    /// 创建db manager
    if sync_live {
        mgr.init_watcher().await?;
    }

    load_aabb_tree().await.unwrap();
    progress_sender.send(10)?;
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
        update_cal_equip().await?;
        update_cal_bran_component().await?;
    }

    let aios_mgr = AiosDBMgr::init_from_db_option().await?;
    // 生成材料表单
    let gen_material = db_option.gen_material.unwrap_or(false);
    if gen_material {
        // save_all_material_data().await?;
    }
    // 生成 TEAM_DATA数据
    if db_option.only_sync_sys {
        match sync_system_db(&aios_mgr).await {
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
        // #[cfg(feature = "mqtt")]
        tokio::join!(
            // AiosDBManager::run_e3d_clone_bg_task(mgr.clone()),
            mgr.async_watch(),
            AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
            // AiosDBManager::demo_mqtt_requests(),
        );
    }

    Ok(())
}
