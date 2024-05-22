#![feature(let_chains)]
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

use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

// use regex::internal::Input;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::room::room::load_aabb_tree;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::SUL_DB;
use bevy_reflect::List;
use chrono::{Datelike, Local, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use log::{error, LevelFilter};
use simplelog::*;
use surrealdb::opt::auth::Root;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::versioned_db::database::*;
use aios_database::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
#[cfg(feature = "gen_model")]
use aios_database::fast_model::gen_all_geos_data;
use aios_database::fast_model::room_model::build_room_relations;
use aios_database::fast_model::{EXIST_MESH_GEO_HASHES, gen_inst_meshes, process_meshes_update_db};

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
        .use_ns(&db_option.project_code)
        .use_db(&db_option.project_name)
        .await?;
    SUL_DB
        .signin(Root {
            username: &db_option.v_user,
            password: &db_option.v_password,
        }).await?;

    aios_core::function::define_common_functions().await.unwrap();

    let sync_live = db_option.sync_live.unwrap_or(false);
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    if sync_live{
        mgr.init_watcher().await.unwrap();
    }
    /// 是否全部同步模型
    if db_option.total_sync || db_option.incr_sync {
        // 同步pdms数据
        sync_pdms(&db_option).await.unwrap();
        return Ok(());
    }
    /// 创建db manager

    #[cfg(feature = "gen_model")]
    if db_option.gen_model {
        println!("正在生成模型");
        let mut time = Instant::now();
        gen_all_geos_data(&db_option, None).await?;
        println!("生成模型花费时间: {} ms", time.elapsed().as_millis());
    }

    {
        let mut time = Instant::now();
        let debug_refnos = db_option.get_debug_refnos();
        //统计一下assets mesh 目录下有多少个mesh，直接忽略去生成
        let path: PathBuf = "assets/meshes".into();
        //收集目录下的文件名
        let paths = fs::read_dir(path).unwrap();
        for entry in paths{
            let entry = entry.unwrap();
            let geo_hash = entry.path().file_stem().unwrap().to_str().unwrap().to_string();
            EXIST_MESH_GEO_HASHES.insert(geo_hash);
        }

        process_meshes_update_db(Some(db_option.clone()), &debug_refnos).await.expect("更新模型数据失败");
        println!("处理模型花费时间: {} ms", time.elapsed().as_millis());
    }

    if db_option.gen_spatial_tree {
        println!("正在生成空间树");
        load_aabb_tree().await.unwrap();
        println!("正在计算房间");
        let mut time = Instant::now();
        build_room_relations(&db_option).await.unwrap();
        println!("计算房间花费时间: {} ms", time.elapsed().as_millis());
        update_cal_equip().await?;
        update_cal_bran_component().await?;
    }

    if sync_live{
        mgr.async_watch().await.unwrap();
    }

    //todo 如何处理初始化的同步，第一次启动一定要同步一次，首先生成archive文件，然后再同步
    //是否需要重构下面的这行代码？
    #[cfg(feature = "mqtt")]
    tokio::join!(
        // AiosDBManager::run_e3d_clone_bg_task(mgr.clone()),
        AiosDBManager::spawn_exec_watcher(mgr.clone()),
        AiosDBManager::poll_sync_e3d_mqtt_events(mgr.watcher.clone()),
        // AiosDBManager::demo_mqtt_requests(),
    );

    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "UDA";
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
