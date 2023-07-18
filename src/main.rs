#![feature(let_chains)]
#![feature(default_free_fn)]
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
use aios_core::{get_default_pdms_db_info, prim_geo};
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_from_bin};
use aios_database::api::admin::sync_system_db;
use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::ssc_data::{get_ancestor_till_type, query_all_room_data, update_ssc_type};
use aios_database::aql_api::foreign_refnos::query_foreign_name_aql;
use aios_database::aql_api::pdms_room::{
    query_all_need_compute_room_refno, RoomEdge, RoomElement,
};
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
use aios_database::ssc::{async_total_ssc_data, get_rooms_from_excel};
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
use aios_core::options::DbOption;
use aios_core::tool::direction_parse::parse_expr_to_dir;
use aios_core::tool::math_tool::{cal_mat3_by_zdir, quat_to_pdms_ori_str, to_pdms_ori_str, to_pdms_vec_str};
use approx::abs_diff_eq;
use tokio::spawn;
use env_logger::{Builder, fmt::Target};
use glam::{Mat3, Quat, Vec3};
use log::{error, LevelFilter};
use tokio::sync::RwLock;
use aios_database::aql_api::children::query_deep_children_refnos_fuzzy;
use aios_database::cata::resolve_helper::parse_str_axis_to_vec3;
use aios_database::consts::*;
#[cfg(feature = "gen_model")]
use aios_database::data_interface::gen_model::gen_geos_data;

fn test_sbfi() -> anyhow::Result<()> {


    let owner_mat3 =  Mat3::from_cols(
        Vec3::X,
        Vec3::NEG_Y,
        Vec3::NEG_Z,
    );

    let axis_str = "Y27.041-X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    let mut mat3 = cal_mat3_by_zdir(addition_axis);
    dbg!(to_pdms_ori_str(&mat3));
    let final_mat3 = owner_mat3 * mat3;
    dbg!(to_pdms_ori_str(&final_mat3));
    let quat = Quat::from_mat3(&final_mat3);
    let m = Mat3::from_quat(quat);
    dbg!(to_pdms_ori_str(&m));

    return Ok(());


    let axis_str = "-X45-Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));

    let axis_str = "-X30-Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));

    let axis_str = "-Y";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));

    let axis_str = "X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));

    let axis_str = "-X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));

    let axis_str = "-Y30X";
    let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    dbg!(to_pdms_ori_str(&cal_mat3_by_zdir(addition_axis)));


    // let refno = RefU64::from_two_nums(17496, 116749);
    // let transform = mgr.get_world_transform(refno).await?.unwrap_or_default();
    // dbg!(quat_to_pdms_ori_str(&transform.rotation));
    // dbg!(transform);
    //
    // let refno = RefU64::from_two_nums(17496, 137937);
    // let transform = mgr.get_world_transform(refno).await?.unwrap_or_default();
    // dbg!(quat_to_pdms_ori_str(&transform.rotation));
    // dbg!(transform);
    //
    // let refno = RefU64::from_two_nums(17496, 137938);
    // let transform = mgr.get_world_transform(refno).await?.unwrap_or_default();
    // dbg!(quat_to_pdms_ori_str(&transform.rotation));
    // dbg!(transform);
    //
    // let refno = RefU64::from_two_nums(17496, 137936);
    // let transform = mgr.get_world_transform(refno).await?.unwrap_or_default();
    // dbg!(quat_to_pdms_ori_str(&transform.rotation));
    // dbg!(transform);


    return Ok(());
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};

    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();

    if db_option.enable_log {
        let now = chrono::offset::Local::now();
        let filename = format!("{}-{}-{}-{}-{}-{}_dblog.txt", now.year(), now.month(), now.day(), now.hour(), now.minute(), now.second());
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
    if let Ok(cache_mesh) = PlantMeshesData::deserialize_from_bin_file(&"assets/mesh/mesh.bin") {
        Arc::get_mut(&mut mgr).unwrap().cached_mesh_mgr = Arc::new(RwLock::new(cache_mesh));
    }

    ///生成ssc 树
    if db_option.rebuild_ssc_tree {
        println!("正在同步SSC");
        if let Some(project_db) = mgr.project_map.get(&mgr.db_option.project_name) {
            // 保存ssc
            async_total_ssc_data(&project_db.value(), mgr.clone()).await?;
            set_arangodb_all_ssc_nodes(project_db.value(), &mgr.get_arango_db().await?).await?;
        }
        println!("SSC同步完成");
    }

    if db_option.only_sync_sys {
        sync_system_db(&mgr).await?;
    }

    //房间树要重写
    if db_option.gen_spatial_tree {
        mgr.calculate_rooms().await.expect("房间计算失败");
    }

    Ok(())
}


/// 提前创建图数据库需要的几个collection
async fn create_arangodb_docs(db_option: &DbOption) -> anyhow::Result<()> {
    let pool = connect_arangodb(db_option).await?;
    let database = pool.get().await?.db(db_option.arangodb_database.as_str()).await?;
    create_arango_document(&database, AQL_DATA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_DESPARA_ELES_COLLECTION, Document).await?;
    create_arango_document(&database, AQL_FOREIGN_EDGES_COLLECTION, Edge).await?;
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
    create_arango_document(&database, AQL_HOLE_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_EMBED_EDGE_COLLECTION, Edge).await?;
    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "GENSEC";
    let hash = db1_hash(noun);
    dbg!(hash);
    let hashes = [0xAF0C5];
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

// #[test]
// fn test_inst_mgr() {
//     let map = CachedInstanceMgr::deserialize_from_bin_file(&"assets/instance/7999.inst").unwrap();
//     let refno = RefU64::from_refno_str("24381/34919").unwrap();
//     if let Some(value) = map.inst_data.inst_map.get(&refno) {
//         dbg!(&value.value());
//     };
// }
//
// #[test]
// fn test_compare_attr_info_file() {
//     let new_info = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../all_attr_info.json")).unwrap();
//     let old_info = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../all_attr_info.json")).unwrap();
//     let old_map = old_info.noun_attr_info_map;
//     for (noun, new_attr) in new_info.noun_attr_info_map {
//         if let Some(old_attr) = old_map.get(&noun) {
//             for (new_key, new_value) in new_attr {
//                 let old_value = old_attr.get(&new_key);
//                 if old_value.is_none() { continue; }
//                 let old_value = old_value.unwrap();
//
//                 if new_value.att_type != old_value.att_type || new_value.name != old_value.name {
//                     dbg!(&noun);
//                     dbg!(&new_value);
//                     dbg!("");
//                 }
//             }
//         } else {
//             dbg!(&noun);
//             dbg!(&db1_dehash(noun as u32));
//             dbg!("");
//         }
//     }
// }

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

#[test]
fn test_db_info_json() {
    let json = get_default_pdms_db_info();
    let noun_map = json.noun_attr_info_map;
    if let Some(value) = noun_map.get(&8830234) {
        dbg!(&value.value());
    };
}