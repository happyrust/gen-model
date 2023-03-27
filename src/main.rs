#![feature(drain_filter)]
#![feature(let_chains)]
#![feature(default_free_fn)]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, UNIX_EPOCH};

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::db_number::DbNumMgr;
use aios_core::pdms_types::*;
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::prim_geo;
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_from_bin};
use arangors_lite::collection::CollectionType::{Document, Edge};
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
use regex::internal::Input;
use sqlx::{Acquire, MySql, MySqlPool, Pool, Row};
use sqlx::Executor;
use sqlx::pool::PoolConnection;

use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::ssc_data::{get_ancestor_till_type, update_ssc_type};
use aios_database::aql_api::foreign_refnos::query_foreign_name_aql;
use aios_database::BATCH_CHUNKS_CNT;
use aios_database::aql_api::pdms_room::{query_all_need_compute_room_refno, RoomEdgeAql, RoomElementAql, save_room_info_to_arangodb};
use aios_database::cata::resolve::parse_to_i32;
use aios_database::consts::*;
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::database::*;
use aios_database::graph_db::pdms_arango::*;
use aios_database::graph_db::pdms_inst_arango::{query_instance_with_refno_in_arangodb, sync_instance_to_graph_db};
use aios_database::graph_db::pdms_mesh_arango::sync_mesh_to_graph_db;
use aios_database::graph_db::ssc_arango::set_arangodb_all_ssc_nodes;
use aios_database::helper::{qualified_column_name, qualified_table_name};
use aios_database::options::DbOption;
use aios_database::ssc::{async_total_ssc_data, get_rooms_from_excel};
use aios_database::tables::*;
use bevy::prelude::*;
use bevy::transform::components::Transform;
use parse_pdms_db::parse_file;
use tokio::spawn;
use aios_database::api::admin::query_all_db_infos;
use aios_database::aql_api::tubi::{insert_tubi_value, query_all_tubi_from_node};
use aios_database::negative::{compute_boolean_mesh, query_negative_refnos_aql};
use aios_database::rvm::elements::create_rvm_file;
use aios_database::spatial_tree::recompute_spatial_tree;


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    /// 是否全部同步模型
    if db_option.total_sync {
        create_arangodb_conns(&db_option).await.expect("Failed to create arangodb conns");
        // 把pdms数据同步到mysql
        sync_pdms(&db_option).await.unwrap();
    }

    /// 创建db manager
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    if let Some(cache_mesh) = CachedMeshesMgr::deserialize_from_bin_file("assets/mesh/mesh.bin") {
        Arc::get_mut(&mut mgr).unwrap().cached_mesh_mgr = Arc::new(cache_mesh);
        dbg!("read cached mesh ok.");
    }

    if db_option.rebuild_ssc_tree {
        dbg!("正在同步SSC");
        for project_db in mgr.project_map.iter() {
            // 保存ssc
            async_total_ssc_data(&project_db.value(), mgr.clone()).await?;
            set_arangodb_all_ssc_nodes(&project_db.value(), &mgr.arango_database).await?;
        }
        dbg!("SSC同步完成");
    }

    let mut all_insts_mgr = HashMap::new();
    if db_option.gen_model_mesh {
        dbg!("正在生成模型");
        let mut time = Instant::now();
        AiosDBManager::cache_geos_data(mgr.clone(), db_option.clone()).await?;
        println!("生成模型花费时间: {} ms", time.elapsed().as_millis());
    }

    {
        // 将 instance 保存到图数据库
        let children_files = fs::read_dir("assets/instance/")?;
        for path in children_files {
            let path = path?.path();
            let filename = path.file_name().unwrap().to_str().unwrap();
            if !filename.ends_with("inst") { continue; }
            let dbno: u32 = path.file_stem().unwrap().to_str().unwrap().parse().unwrap();
            let mut file = fs::File::open(path)?;
            let mut data = vec![];
            file.read_to_end(&mut data)?;
            let instance_mgr = bincode::deserialize::<PdmsMeshInstanceMgr>(&data)?;
            if db_option.save_model_mesh_to_graph_db {
                dbg!("正在保存Instances");
                sync_instance_to_graph_db(mgr.clone(), &instance_mgr).await?;
            }
            all_insts_mgr.insert(dbno, instance_mgr);
        }

        if let Some(project_pool) = mgr.project_map.get(&db_option.project_name) {
            let create_table_sql = gen_create_pdms_mesh_table_sql();
            let mut conn = project_pool.acquire().await?;
            let result = conn.execute(create_table_sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                }
            }

            let children_files = fs::read_dir("assets/mesh/")?;
            dbg!("正在保存Meshes");
            if db_option.save_model_mesh_to_graph_db {
                for path in children_files {
                    let path = path?.path();
                    let filename = path.file_name().unwrap().to_str().unwrap().to_string();
                    if !filename.ends_with("bin") { continue; }
                    let mut file = fs::File::open(path)?;
                    let mut data = vec![];
                    file.read_to_end(&mut data)?;
                    let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
                    dbg!(&mesh_mgr.meshes.len());

                    sync_mesh_to_graph_db(&mgr, &mesh_mgr).await?;
                    // save_pdms_mesh_tidb(mesh_mgr, project_pool.value()).await?;
                }
            }
        }
    }

    //生成rtree 结构

    let mut collider_shape_mgr = CachedColliderShapeMgr::default();
    let mut file = fs::File::open("assets/mesh/mesh.bin")?;
    let mut data = vec![];
    file.read_to_end(&mut data)?;
    let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
    dbg!(&mesh_mgr.meshes.len());
    if db_option.gen_spatial_tree {
        let mut timer = Instant::now();
        let mut rstar_objs = vec![];

        let children_files = fs::read_dir("assets/instance/")?;
        let arch_db_nums = db_option.clone().arch_db_nums.unwrap_or_default();
        for path in children_files {
            let path = path?.path();
            dbg!(&path);
            let filename = path.file_name().unwrap().to_str().unwrap().to_string();
            if !filename.contains("inst") { continue; }
            let dbno_str = filename.split('.').collect::<Vec<_>>();
            let dbno = dbno_str.first().unwrap_or(&"");
            if arch_db_nums.contains(&dbno.parse().unwrap_or(0)) { continue; }

            let mut file = fs::File::open(path)?;
            let mut data = vec![];
            file.read_to_end(&mut data)?;
            let instance_mgr = bincode::deserialize::<PdmsMeshInstanceMgr>(&data)?;
            dbg!(&instance_mgr.inst_mgr.inst_map.len());
            for kv in &instance_mgr.inst_mgr.inst_map {
                if let Some(aabb) = kv.value().aabb {
                    if aabb.extents().magnitude().is_finite() {
                        rstar_objs.push(RStarBoundingBox::from_aabb(&aabb, *kv.key()));
                    } else {
                        // println!("Aabb {:?} is not ok : {:?}", kv.key(), &aabb);
                    }
                }
            }
        }
        println!("收集空间包围盒时间: {}s", timer.elapsed().as_secs_f32());
        timer = Instant::now();
        let rtree = AccelerationTree::load(rstar_objs);
        println!("生成空间树费时: {}s", timer.elapsed().as_secs_f32());
        let mut file = fs::File::create("accel.spa").unwrap();
        let serialized = bincode::serialize(&rtree).unwrap();
        file.write_all(serialized.as_slice()).unwrap();
    }

    if db_option.save_spatial_tree_to_db {
        let mut site_major_map = HashMap::new();
        let room_infos = vec![RefU64::from_two_nums(24381,34919)];
        let map = recompute_spatial_tree(room_infos, all_insts_mgr, collider_shape_mgr, &db_option).await?;
        save_room_info_to_arangodb(&mgr, map, &db_option, &mut site_major_map).await?;
    }

    if false {
        if let Some(pool) = mgr.project_map.get(&db_option.project_name) {
            let brans = query_types_refnos(&vec!["BRAN"], &pool, db_option.manual_db_nums.clone()).await?;
            let mut tubi_map = Arc::new(DashMap::new());
            let mut handles = vec![];
            for bran in brans {
                dbg!(&bran);
                let pool_clone = pool.value().clone();
                // let db_option_clone = db_option.clone();
                let database = mgr.arango_database.clone();
                let mut tubi_map_clone = tubi_map.clone();
                let handle = tokio::spawn(async move {
                    // let database = get_arangodb_conn_from_db_option(&db_option_clone).await.unwrap();
                    query_all_tubi_from_node(bran, &mut tubi_map_clone, &database, &pool_clone).await.unwrap_or_default();
                });
                handles.push(handle);
            }
            futures::future::join_all(&mut handles).await;
            let mut file = fs::File::create("tubi_map.txt")?;
            file.write_all(&serde_json::to_vec(&tubi_map).unwrap_or_default())?;
            insert_tubi_value(Arc::try_unwrap(tubi_map).unwrap_or_default(), pool.value()).await?;
        }
    }

    if db_option.only_sync_sys {
        query_all_db_infos(&mgr).await?;
    }
    Ok(())
}

// // #[tokio::main]
// async fn main_1() -> anyhow::Result<()> {
//     use config::{Config, ConfigError, Environment, File};
//     let s = Config::builder()
//         .add_source(File::with_name("DbOption"))
//         .build()?;
//     let db_option: DbOption = s.try_deserialize().unwrap();
//     let database = get_arangodb_conn_from_db_option(&db_option).await?;
//     // let aios_mgr = AiosDBManager::init_form_config().await?;
//     // let database = aios_mgr.get_arangodb_conn().await?;
//     let refno = RefU64::from_refno_str("23584/5386").unwrap();
//
//     let negative_refnos = query_negative_refnos_aql(refno, &aios_mgr,&database).await?.get(&refno).unwrap().to_vec();
//     let result = compute_boolean_mesh(refno, negative_refnos, &database).await?;
//     // dbg!(&result);
//     Ok(())
// }

// #[tokio::main]
// async fn main() -> anyhow::Result<()> {
//     let mgr = Arc::new(AiosDBManager::init_form_config().await?);
//     let refno = RefU64::from_refno_str("23584/5495").unwrap();
//     let data = create_rvm_file(refno, &mgr).await?;
//     let mut file = std::fs::File::create("test_rvm.rvm").unwrap();
//     file.write_all(&data).unwrap();
//     Ok(())
// }

/// 提前创建图数据库需要的几个collection
async fn create_arangodb_conns(db_option: &DbOption) -> anyhow::Result<()> {
    set_arangodb_database_from_db_option(db_option).await?;
    let database = get_arangodb_conn_from_db_option(db_option).await?;
    create_arangodb_conn(&database, "data_eles", Document).await?;
    create_arangodb_conn(&database, "despara_eles", Document).await?;
    create_arangodb_conn(&database, "foreign_edges", Edge).await?;
    create_arangodb_conn(&database, "instance_edges", Edge).await?;
    create_arangodb_conn(&database, "para_eles", Document).await?;
    create_arangodb_conn(&database, "pdms_edges", Edge).await?;
    create_arangodb_conn(&database, "pdms_eles", Document).await?;
    create_arangodb_conn(&database, "pdms_instances", Document).await?;
    create_arangodb_conn(&database, "plin_eles", Document).await?;
    create_arangodb_conn(&database, "sibl_edges", Edge).await?;
    create_arangodb_conn(&database, "ssc_edges", Edge).await?;
    create_arangodb_conn(&database, "ssc_eles", Document).await?;
    create_arangodb_conn(&database, "tubi_edges", Edge).await?;
    create_arangodb_conn(&database, "room_eles", Document).await?;
    create_arangodb_conn(&database, "hole_data", Document).await?;
    create_arangodb_conn(&database, "embed_data", Document).await?;
    create_arangodb_conn(&database, "room_edges", Edge).await?;
    create_arangodb_conn(&database, "geo_infos", Document).await?;
    Ok(())
}


#[test]
fn get_noun_hash() {
    let noun = "USER";
    let hash = db1_hash(noun);
    let str = db1_dehash(0xF423F);
    dbg!(hash);
    dbg!(str);
}

#[test]
fn test_time() {
    use chrono::prelude::*;
    let local: DateTime<Local> = Local::now();
    println!("year:{} , month: {} , day: {}, week_day:{},hour:{} , min: {} , sec:{}", local.year(), local.month(), local.day(), local.weekday(),
             local.hour(), local.minute(), local.second());
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
fn test_inst_mgr() {
    let mut file = File::open("assets/instance/7999.inst").unwrap();
    let mut data = vec![];
    file.read_to_end(&mut data).unwrap();
    let map = bincode::deserialize::<PdmsMeshInstanceMgr>(&data).unwrap();
    let refno = RefU64::from_refno_str("24381/34919").unwrap();
    if let Some(value) = map.inst_mgr.inst_map.get(&refno) {
        dbg!(&value.value());
    };
}