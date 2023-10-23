#![feature(let_chains)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::db_number::DbNumMgr;
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::pdms_types::*;
use aios_core::prim_geo;
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_from_bin};
use aios_database::api::admin::sync_system_db;
use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::ssc_data::{get_ancestor_till_type, query_all_room_data, update_ssc_type};
use aios_database::aql_api::foreign_refnos::query_foreign_name_aql;
use aios_database::aql_api::pdms_room::{query_all_need_compute_room_refno, RoomEdge, RoomElement};
use aios_database::aql_api::tubi::{insert_tubi_value, query_all_tubi_from_node};
use aios_database::cata::resolve::parse_to_i32;
use aios_database::consts::*;
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::database::*;
use aios_database::graph_db::pdms_arango::*;
use aios_database::graph_db::pdms_inst_arango::*;
use aios_database::graph_db::pdms_mesh_arango::save_mesh_to_arango_db;
use aios_database::graph_db::ssc_arango::set_arangodb_all_ssc_nodes;
use aios_database::spatial_tree::recompute_spatial_tree;
use aios_database::ssc::{async_total_ssc_data, get_room_info_from_excel_refactor, get_rooms_from_excel, insert_ssc_room_node_refactor, save_ssc_level_excel};
use aios_database::tables::*;
use bb8_arangodb::arangors_lite::collection::CollectionType::{Document, Edge};
use bevy_transform::prelude::Transform;
use chrono::{Datelike, Timelike};
use dashmap::DashMap;
use futures::StreamExt;
use itertools::Itertools;
use nalgebra::{max, Quaternion, UnitQuaternion};
use nom::Parser;
use nom_derive::Parse;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Point, Vector};
use parry3d::shape::{Compound, ConvexPolyhedron, SharedShape};
use parry3d::transformation::vhacd;
use parry3d::transformation::vhacd::VHACD;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
// use regex::internal::Input;
use aios_core::options::DbOption;
use aios_core::tool::direction_parse::parse_expr_to_dir;
use aios_core::tool::math_tool::{
    cal_mat3_by_zdir, quat_to_pdms_ori_str, to_pdms_ori_str, to_pdms_vec_str,
};
use aios_database::aql_api::children::query_deep_children_refnos_fuzzy;
use aios_database::cata::resolve_helper::parse_str_axis_to_vec3;
use aios_database::consts::*;
#[cfg(feature = "gen_model")]
use aios_database::data_interface::gen_model::gen_geos_data;
use approx::abs_diff_eq;
use env_logger::{fmt::Target, Builder};
use glam::{Mat3, Quat, Vec3};
use log::{error, LevelFilter};
use sqlx::pool::PoolConnection;
use sqlx::Executor;
use sqlx::{Acquire, MySql, MySqlPool, Pool, Row};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::spawn;
use tokio::sync::RwLock;

fn test_sbfi() -> anyhow::Result<()> {
    // let axis_str = "Y27.041-X";
    let axis_str = "Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    let axis_str = "-Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    let axis_str = "-Y30X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    let axis_str = "Y30-X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    let axis_str = "-Y30-X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    let axis_str = "-X30-Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));

    return Ok(());
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    use chrono::Local;
    use std::fs::OpenOptions;
    // 从配置文件中读取数据库选项
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();

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
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(filename)
            .unwrap();

        // 配置日志过滤器和输出目标
        let mut builder = Builder::from_default_env();
        builder.filter(Some("aios_database"), LevelFilter::Info);
        builder.filter(Some("aios_core"), LevelFilter::Info);
        builder.target(Target::Pipe(Box::new(file))).init();
    }

    create_arangodb_docs(&db_option)
        .await
        .expect("Failed to create arangodb conns");
    /// 是否全部同步模型
    if db_option.total_sync {
        create_arangodb_docs(&db_option)
            .await
            .expect("Failed to create arangodb docs");
        // 同步pdms数据
        sync_pdms(&db_option).await.unwrap();
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

/// 提前创建图数据库需要的几个collection
/**
 * This code is responsible for creating documents in ArangoDB.
 * It connects to the ArangoDB using the provided database options,
 * and then creates various documents and edges in the database.
 */
async fn create_arangodb_docs(db_option: &DbOption) -> anyhow::Result<()> {
    // Connect to ArangoDB
    let pool = connect_arangodb(db_option).await?;
    let database = pool
        .get()
        .await?
        .db(db_option.arangodb_database.as_str())
        .await?;

    // Create ArangoDB documents and edges
    create_arango_document(&database, AQL_DATA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_DESPARA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_FOREIGN_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PDMS_MDBS_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_INSTANCE_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PARA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PDMS_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_MESH_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_COMPOUND_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_NGMS_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_INFO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_GEO_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_TUBI_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_PDMS_INST_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_PLIN_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_SIBL_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_SSC_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_SSC_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_TUBI_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_ROOM_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_ROOM_EDGES_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_GEO_INFOS_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_EMBED_DATA_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_WATER_CALCULATION_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_HOLE_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_EMBED_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_VIRTUAL_HOLE_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_THREED_REVIEW_COLLECTION, Document).await?;
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

/// This code is a test suite for logging and database operations.
/// It sets up a logger using the `env_logger` crate and logs an error message.
/// It also performs database operations using the `AiosDBManager` struct.
/// The `test_log` function logs an error message to a file.
/// The `test_db1_dehash` function initializes a database manager, retrieves children within a project,
/// and calculates a hash value.
/// This code requires the `env_logger`, `log`, and `tokio` crates to be added as dependencies.
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