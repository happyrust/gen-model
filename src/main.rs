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
use parry3d::bounding_volume::AABB;
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
use aios_database::api::project_mdb::insert_project_mdb;
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
use aios_database::tables::{gen_create_attr_info_tables_sql, gen_create_pdms_mesh_table_sql};
use bevy::prelude::*;
use bevy::transform::components::Transform;
use parse_pdms_db::parse_file;
use tokio::spawn;
use aios_database::aql_api::tubi::{insert_tubi_value, query_all_tubi_from_node};


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
            for path in children_files {
                let path = path?.path();
                let filename = path.file_name().unwrap().to_str().unwrap().to_string();
                if !filename.ends_with("bin") { continue; }
                let mut file = fs::File::open(path)?;
                let mut data = vec![];
                file.read_to_end(&mut data)?;
                let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
                dbg!(&mesh_mgr.meshes.len());
                if db_option.save_model_mesh_to_graph_db {
                    save_pdms_mesh_tidb(mesh_mgr, project_pool.value()).await?;
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
                        // println!("AABB {:?} is not ok : {:?}", kv.key(), &aabb);
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

    {
        if db_option.save_spatial_tree_to_db {
            // 保存空间树数据
            let instance_dir_path = "assets/instance";
            let mut file = fs::File::open("accel.spa")?;
            let mut buf = vec![];
            file.read_to_end(&mut buf)?;
            let rtree = bincode::deserialize::<AccelerationTree>(&buf)?;
            //生成TriMesh 的 shape

            let dbno = db_option.arch_db_nums.clone().unwrap_or_default().clone();
            // let room_infos = query_all_need_compute_room_refno(&dbno, "ROOM", Some("ROOMS"), &mgr.project_map.get(&db_option.project_name).unwrap()).await?;
            let room_infos = query_all_need_compute_room_refno(&dbno, "FRMW", Some("-RM"), &mgr.project_map.get(&db_option.project_name).unwrap()).await?;
            // let room_infos = vec![(RefU64::from_two_nums(17544, 15107), "N448".to_string())];
            let dbno_mgr = DbNumMgr::load_file(&format!("{instance_dir_path}/dbno_mgr.num")).unwrap_or_default();
            for (target_refno, room_name) in room_infos {
                // dbg!(&room_name);
                let mut room_info_map = HashMap::new();
                if let Some(dbno) = dbno_mgr.get_dbno(target_refno) {
                    if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
                        if inst_mgr.level_shape_mgr.contains_key(&target_refno) {
                            let all_refnos = inst_mgr.level_shape_mgr.get(&target_refno).unwrap();
                            for room_refno in all_refnos.value().clone().into_iter() {
                                if room_info_map.contains_key(&room_refno) { continue; }
                                let ele_geos_info_map = inst_mgr.get_instants_data(room_refno);
                                for ele_geos_info in &ele_geos_info_map {
                                    //filter None aabb
                                    let ele_refno = *ele_geos_info.key();
                                    let room_colliders = collider_shape_mgr.get_collider(ele_refno, inst_mgr, &mesh_mgr);
                                    if let Some(target_abb) = ele_geos_info.aabb {
                                        let mut withing_room_refnos = rtree
                                            .locate_intersecting_bounds(&target_abb).collect::<Vec<_>>();
                                        dbg!(&withing_room_refnos.len());
                                        if withing_room_refnos.len() > 2000 { continue; }
                                        let mut removed_refnos = vec![];
                                        withing_room_refnos.retain(|x| {
                                            //直接判断点集，可以快速过滤一些构件
                                            if let Some(dbno) = dbno_mgr.get_dbno(*x) {
                                                if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
                                                    let ele_geos_info_map = inst_mgr.get_instants_data(*x);
                                                    let mut has_checked = false;
                                                    for ele_geos_info in &ele_geos_info_map {
                                                        let tr = ele_geos_info.get_transform();
                                                        for pt_kv in &ele_geos_info.ptset_map {
                                                            let p = tr.mul_vec3(pt_kv.1.pt);
                                                            for rc in &room_colliders {
                                                                if rc.as_ref().contains_point(&Isometry::identity(), &Point::new(p.x, p.y, p.z)) {
                                                                    return true;
                                                                }
                                                            }
                                                            has_checked = true;
                                                            let checking_colliders = collider_shape_mgr.get_collider(*x, inst_mgr, &mesh_mgr);
                                                            for rc in &room_colliders {
                                                                for cc in &checking_colliders {
                                                                    let target_pt = if let Some(tri_mesh) = cc.as_ref().as_trimesh() {
                                                                        tri_mesh.triangle(0).local_aabb().center()
                                                                    } else {
                                                                        cc.compute_local_aabb().center()
                                                                    };
                                                                    if rc.as_ref().contains_point(&Isometry::identity(), &target_pt) {
                                                                        return true;
                                                                    }
                                                                    let r = parry3d::query::intersection_test(&Isometry::identity(), rc.as_ref(),
                                                                                                              &Isometry::identity(), cc.as_ref()).unwrap();
                                                                    if r {
                                                                        return true;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            removed_refnos.push(*x);
                                            false
                                        });
                                        let mut file = fs::File::create("removed_refnos.data").unwrap();
                                        let serialized = bincode::serialize(&removed_refnos).unwrap();
                                        file.write_all(serialized.as_slice()).unwrap();
                                        dbg!(removed_refnos.len());
                                        dbg!(&withing_room_refnos.len());
                                        room_info_map.entry(room_refno).or_insert((target_abb, withing_room_refnos));
                                    }
                                }
                            }
                        }
                    }
                }
                save_room_info_to_arangodb(room_info_map, &db_option).await?;
            }
        }
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
    Ok(())
}


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
    create_arangodb_conn(&database, "room_edges", Edge).await?;
    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "PTAP";
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