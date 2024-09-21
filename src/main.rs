#![feature(let_chains)]
#![feature(duration_constructors)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

extern crate strum;

#[macro_use]
extern crate strum_macros;

use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use std::fs;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

// use aios_core::material::save_all_material_data;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::room::room::{load_aabb_tree, GLOBAL_AABB_TREE};
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::SUL_DB;
use aios_core::{build_cate_relate, get_db_option};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
#[cfg(feature = "gen_model")]
use aios_database::fast_model::gen_all_geos_data;
use aios_database::fast_model::room_model::build_room_relations;
use aios_database::fast_model::{
    gen_inst_meshes, process_meshes_update_db_deep, EXIST_MESH_GEO_HASHES,
};
use aios_database::versioned_db::database::*;
// use aios_database::versioned_db::task::initialize_global_db_sender;
use bevy_reflect::List;
use chrono::{Datelike, Local, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use log::{error, LevelFilter};
use simplelog::*;
use surrealdb::opt::auth::Root;
use aios_database::team_data::sync_system_db;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_option: DbOption = get_db_option().clone();
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
    SUL_DB
        .connect(db_option.get_version_db_conn_str())
        .with_capacity(1000)
        .await?;
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
    println!("数据库已经连接到 {}, 站点: {}", db_option.project_name, db_option.get_version_db_conn_str());
    aios_core::function::define_common_functions()
        .await
        .unwrap();
    println!("预加载方法完成。");
    let sync_live = db_option.sync_live.unwrap_or(false);
    // initialize_global_db_sender().await;

    /// 是否全部同步模型
    if db_option.total_sync || db_option.incr_sync || db_option.only_sync_sys || db_option.is_sync_history() {
        println!("开始同步解析数据。");
        // 同步pdms数据
        sync_pdms(&db_option).await.unwrap();
        return Ok(());
    }



    if db_option.build_cate_relate() {
        //检查cate_relate 是否创建了
        println!("初始化创建Cate relate关系");
        build_cate_relate(false).await.unwrap();
    }

    let mut cur_mgr = None;
    /// 创建db manager
    if sync_live {
        let mgr = Arc::new(AiosDBManager::init_form_config().await?);
        mgr.init_watcher().await.unwrap();
        cur_mgr = Some(mgr);
    }

    load_aabb_tree().await.unwrap();
    //todo 还有个问题，可能需要通过队列来排队任务
    //如果没有生成完，需要等待
    #[cfg(feature = "gen_model")]
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
        println!("房间空间数的数量为: {}", GLOBAL_AABB_TREE.read().await.tree.size());
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
        cur_mgr.clone().unwrap().async_watch().await.unwrap();
    }

    //todo 如何处理初始化的同步，第一次启动一定要同步一次，首先生成archive文件，然后再同步
    //是否需要重构下面的这行代码？
    // #[cfg(feature = "mqtt")]
    // tokio::join!(
    //     // AiosDBManager::run_e3d_clone_bg_task(mgr.clone()),
    //     AiosDBManager::spawn_exec_watcher(mgr.clone()),
    //     AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
    //     // AiosDBManager::demo_mqtt_requests(),
    // );

    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "DB";
    let hash = db1_hash(noun);
    dbg!(hash);
    let hashes = [919309, 640481, 919399];
    for hash in hashes {
        let str = db1_dehash(hash);
        dbg!(&hash);
        dbg!(str);
    }
}

#[test]
fn test_time() {
    use chrono::prelude::*;
    let local: DateTime<Local> = Local::now();
    println!(
        "year:{} , month: {} , day: {}, week_day:{},hour:{} , min: {} , sec:{}",
        local.year(),
        local.month(),
        local.day(),
        local.weekday(),
        local.hour(),
        local.minute(),
        local.second()
    );
}

/// 将 all_attr_info.bin 文件转成 json
#[test]
fn test_turn_bin_into_json() {
    let mut file = File::open("all_attr_info.bin").unwrap();
    let mut data = vec![];
    file.read_to_end(&mut data).unwrap();
    let map = bincode::deserialize::<PdmsDatabaseInfo>(&data).unwrap();
    let json = serde_json::to_string(&map).unwrap();
    let mut new_file = File::create("all_attr_info_1.json").unwrap();
    new_file.write_all(&json.into_bytes()).unwrap();
}
