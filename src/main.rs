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
use aios_core::pdms_types::*;
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_from_bin};
use arangors_lite::collection::CollectionType::{Document, Edge};
use chrono::{Datelike, Timelike};
use dashmap::DashMap;
use futures::StreamExt;
use itertools::Itertools;
use nalgebra::{Quaternion, UnitQuaternion};
use nom_derive::Parse;
use parry3d::bounding_volume::AABB;
use parry3d::math::{Isometry, Vector, Point};
use parry3d::transformation::vhacd;
use parry3d::transformation::vhacd::VHACD;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
use regex::internal::Input;
use sqlx::{Acquire, MySql, MySqlPool, Pool, Row};
use sqlx::Executor;
use sqlx::pool::PoolConnection;

use aios_database::BATCH_CHUNKS_CNT;
use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::project_mdb::insert_project_mdb;
use aios_database::api::ssc_data::{get_ancestor_till_type, update_ssc_type};
use aios_database::aql_api::foreign_refnos::query_foreign_name_aql;
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
use aios_database::tables::{gen_create_attr_info_tables_sql, gen_create_pdms_mesh_table_sql};


pub async fn test_batch_insert(url: &str) {
    let connection = MySqlPool::connect(&url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();
    let sql = format!(r#"INSERT {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, NAME) VALUES (1, 100, 'test', 'unset'), (2, 100, 'test', 'unset')"#);
    let result = sqlx::query(&sql).execute(&mut pool).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}

#[tokio::test]
async fn test_get_att() -> anyhow::Result<()> {
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let attr = mgr.get_attr(RefU64::from_two_nums(23584, 6169)).await?;

    dbg!(attr.to_string_hashmap());

    let world_transform = mgr.get_world_transform(RefU64::from_two_nums(23584, 6169)).await?;
    dbg!(&world_transform);

    Ok(())
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    dbg!(&db_option);

    if db_option.total_sync {
        create_arangodb_conns(&db_option).await.expect("Failed to create arangodb conns");
        // 把pdms数据同步到mysql
        sync_pdms(&db_option).await.unwrap();
    }

    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    if let Some(cache_mesh) = CachedMeshesMgr::deserialize_from_bin_file("./assets/mesh/mesh.bin") {
        Arc::get_mut(&mut mgr).unwrap().cached_mesh_mgr = Arc::new(cache_mesh);
        dbg!("read cached mesh ok.");
    }

    if db_option.rebuild_ssc_tree {
        dbg!("正在同步SSC");
        for project_db in mgr.project_map.iter() {
            // 保存ssc
            // async_total_ssc_data(&project_db.value(), mgr.clone()).await?;
            set_arangodb_all_ssc_nodes(&project_db.value(), &mgr.arango_database).await?;
        }
        dbg!("SSC同步完成");
    }

    if db_option.gen_model_mesh {
        dbg!("正在生成模型");
        let mut time = Instant::now();
        AiosDBManager::cache_geos_data(mgr.clone(), db_option.clone()).await?;
        println!("生成模型花费时间: {} ms", time.elapsed().as_millis());

        // 将 instance 保存到图数据库
        dbg!("正在保存图数据库");
        let children_files = fs::read_dir("assets/instance/")?;
        for path in children_files {
            let path = path?.path();
            let filename = path.file_name().unwrap().to_str().unwrap().to_string();
            if !filename.ends_with("inst") { continue; }
            dbg!(&filename);
            let mut file = fs::File::open(path)?;
            let mut data = vec![];
            file.read_to_end(&mut data)?;
            let instance_mgr = bincode::deserialize::<PdmsMeshInstanceMgr>(&data)?;
            // let instance_mgr = Arc::new(change_instance_mgr_old_into_new(instance_mgr));
            dbg!(&instance_mgr.inst_mgr.inst_map.len());
            sync_instance_to_graph_db(mgr.clone(), Arc::new(instance_mgr)).await?;
        }
        dbg!("正在保存mesh");
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
            let params = vhacd::VHACDParameters::default();
            for path in children_files {
                let path = path?.path();
                let filename = path.file_name().unwrap().to_str().unwrap().to_string();
                if !filename.ends_with("bin") { continue; }
                dbg!(&filename);
                let mut file = fs::File::open(path)?;
                let mut data = vec![];
                file.read_to_end(&mut data)?;
                let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
                // for m in &mesh_mgr.meshes{
                //     let points = m.vertices.iter().map(|x| Point::from_slice(x)).collect::<Vec<Point<f32>>>();
                //     let indices: Vec<[u32; 3]> = m.indices.chunks(3).map(|x| [x[0], x[1], x[2]]).collect();
                //     let vhacd = VHACD::decompose(&params, &points, &indices, false);
                    // let convex_hulls = vhacd.compute_convex_hulls(1);
                // }

                save_pdms_mesh_tidb(mesh_mgr, project_pool.value()).await?;
            }
        }
        dbg!("图数据库保存完成");
    }

    //生成rtree 结构
    if db_option.gen_spatial_tree {
        let dir_path = "assets/instance";
        let mut db_nos = db_option.manual_db_nums.clone().unwrap_or_default();
        let mut timer = Instant::now();
        let mut rstar_objs = vec![];
        for db_no in db_nos {
            let mut file = fs::File::open(format!("{}/{}.inst", dir_path, db_no))?;
            let mut data = vec![];
            file.read_to_end(&mut data)?;
            let instance_mgr = bincode::deserialize::<PdmsMeshInstanceMgr>(&data)?;
            dbg!(&instance_mgr.inst_mgr.inst_map.len());
            for kv in &instance_mgr.inst_mgr.inst_map {
                let aabb = kv.value().aabb;
                if aabb.extents().magnitude().is_finite() {
                    rstar_objs.push(RStarBoundingBox::from_aabb(&aabb, *kv.key()));
                } else {
                    // println!("AABB {:?} is not ok : {:?}", kv.key(), &aabb);
                }
            }
        }
        println!("收集空间包围盒时间: {}s", timer.elapsed().as_secs_f32());
        timer = Instant::now();
        let rtree = AccelerationTree::load(rstar_objs);

        let test_aabb = AABB::new(Point::new(-20221.703,-5851.465,-3200.0),
                                  Point::new(21765.613,31888.738,-599.9999));
        let target_refnos = rtree
            .locate_intersecting_bounds(&test_aabb).collect::<Vec<_>>();
        // dbg!(target_refnos.len());

        println!("生成空间树费时: {}s", timer.elapsed().as_secs_f32());
        let mut file = fs::File::create("accel.spa").unwrap();
        let serialized = bincode::serialize(&rtree).unwrap();
        file.write_all(serialized.as_slice()).unwrap();
    }

    Ok(())
}


/// 提前创建图数据库需要的几个collection
async fn create_arangodb_conns(db_option: &DbOption) -> anyhow::Result<()> {
    let database = get_arangodb_conn_from_db_option(db_option).await?;
    create_arangodb_conn(&database, "data_eles", Document).await?;
    create_arangodb_conn(&database, "despara_eles", Document).await?;
    create_arangodb_conn(&database, "foreign_edges", Edge).await?;
    create_arangodb_conn(&database, "instance_edges", Edge).await?;
    create_arangodb_conn(&database, "para_eles", Document).await?;
    create_arangodb_conn(&database, "pdms_edges", Edge).await?;
    create_arangodb_conn(&database, "pdms_eles", Document).await?;
    create_arangodb_conn(&database, "pdms_instances", Edge).await?;
    create_arangodb_conn(&database, "plin_eles", Document).await?;
    create_arangodb_conn(&database, "sibl_edges", Edge).await?;
    create_arangodb_conn(&database, "ssc_edges", Edge).await?;
    create_arangodb_conn(&database, "ssc_eles", Document).await?;
    create_arangodb_conn(&database, "tubi_edges", Edge).await?;
    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "DB";
    let hash = db1_hash(noun);
    let str = db1_dehash(0xDC34AB5);
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
