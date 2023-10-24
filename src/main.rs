#![feature(let_chains)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

use aios_core::pdms_types::*;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_database::api::admin::sync_system_db;
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::database::*;
use aios_database::ssc::{get_room_info_from_excel_refactor, insert_ssc_room_node_refactor, save_ssc_level_excel};
use chrono::{Datelike, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use nom::Parser;
use nom_derive::Parse;
// use regex::internal::Input;
use aios_core::options::DbOption;
use aios_core::tool::direction_parse::parse_expr_to_dir;
use aios_core::tool::math_tool::{
    cal_mat3_by_zdir, to_pdms_ori_str,
};
#[cfg(feature = "gen_model")]
use aios_database::data_interface::gen_model::gen_geos_data;
use env_logger::{Builder, fmt::Target};
use log::{error, LevelFilter};
use std::fs::File;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;
use aios_database::arangodb::create::create_arangodb_docs;
use aios_database::versioned_db::create::create_versioned_schemas;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();

    if db_option.enable_log {
        let now = chrono::offset::Local::now();
        let filename = format!(
            "{}-{}-{}-{}-{}-{}_dblog.txt",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(filename)
            .unwrap();
        let mut builder = Builder::from_default_env();
        builder.filter(Some("aios_database"), LevelFilter::Info);
        builder.filter(Some("aios_core"), LevelFilter::Info);
        builder.target(Target::Pipe(Box::new(file))).init();
    }

    /// 是否全部同步模型
    if db_option.total_sync {
        // create_versioned_schemas(&db_option.project_name).await.expect("create versioned docs");
        if db_option.sync_graph_db.unwrap_or(false) {
            create_arangodb_docs(&db_option)
                .await
                .expect("Failed to create arangodb docs");
        }
        // 同步pdms数据
        sync_pdms(&db_option).await.unwrap();
        return Ok(());
    }
    /// 创建db manager
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);

    #[cfg(feature = "gen_model")]
    if db_option.gen_model {
        println!("正在生成模型");
        let mut time = Instant::now();

        gen_geos_data(mgr.clone()).await?;
        println!("生成模型花费时间: {} ms", time.elapsed().as_millis());
    }

    ///生成ssc 树
    /// 需要 resource 下文档 ssc_level.xlsx  ssc_room.xlsx 专业分类.xlsx
    if db_option.rebuild_ssc_tree {
        println!("正在同步SSC");
        if let Ok(database) = mgr.get_arango_db().await {
            // 保存ssc
            // async_total_ssc_data(&project_db.value(), mgr.clone()).await?;
            // set_arangodb_all_ssc_nodes(project_db.value(), &mgr.get_arango_db().await?).await?;
            let _ = save_ssc_level_excel(&database).await?;
            let _result = get_room_info_from_excel_refactor(&database).await.unwrap();
            let _result = insert_ssc_room_node_refactor(&database).await.unwrap();
        }
        println!("SSC同步完成");
    }

    if db_option.only_sync_sys {
        println!("正在同步TEAM DATA");
        sync_system_db(&mgr).await?;
    }

    //房间树要重写
    if db_option.gen_spatial_tree {
        mgr.calculate_rooms().await.expect("房间计算失败");
    }

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

#[test]
fn test_log() {
    use env_logger::{fmt::Target, Builder};
    use log::error;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("log.txt")
        .unwrap();

    let mut builder = Builder::from_default_env();
    builder.target(Target::Pipe(Box::new(file))).init();
    error!("Some error");
}

#[tokio::test]
async fn test_db1_dehash() {
    let mgr = AiosDBManager::init_form_config().await.unwrap();
    let refno = RefU64::from_refno_str("24383/91850").unwrap();
    let children = mgr.get_children_within_project(refno, "AvevaMarineSample").unwrap();
    dbg!(&children);
    let hash = db1_hash(":STACbeam");
    dbg!(&hash);
}