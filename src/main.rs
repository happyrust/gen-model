use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use itertools::Itertools;
use aios_core::pdms_types::{AttrMap, AttrVal, CachedMeshesMgr, DbAttributeType, NounHash, PdmsDatabaseInfo, PdmsMeshInstanceMgr, RefI32Tuple, RefU64};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_from_bin};
use dashmap::DashMap;
use futures::StreamExt;
use nom_derive::Parse;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
use regex::internal::Input;
use aios_database::{BATCH_CHUNKS_CNT};
use sqlx::{MySql, MySqlPool, Pool};
use sqlx::pool::PoolConnection;
use aios_database::database::*;
use aios_database::helper::{qualified_column_name, qualified_table_name};
use aios_database::options::DbOption;
use aios_database::consts::*;

use sqlx::Executor;
use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::project_mdb::insert_project_mdb;
use aios_database::api::ssc_data::{get_ancestor_till_type, update_ssc_type};
use aios_database::cata::resolve::parse_to_i32;
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::graph_db::arango::sync_graph_db;
use aios_database::ssc::{async_total_ssc_data, get_rooms_from_excel, set_arangodb_all_ssc_fixed_nodes};
use aios_database::tables::gen_create_attr_info_tables_sql;


#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

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


#[test]
fn test_hash() {
    dbg!(db1_dehash(612916));
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
        sync_pdms(&db_option).await.unwrap();
    }

    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    if let Some(cache_mesh) = CachedMeshesMgr::deserialize_from_bin_file("./assets/mesh/mesh.bin") {
        Arc::get_mut(&mut mgr).unwrap().cached_mesh_mgr = Arc::new(cache_mesh);
        dbg!("read cached mesh ok.");
    }

    let b_recreate_ssc = db_option.rebuild_ssc_tree;
    if b_recreate_ssc {
        dbg!("正在同步SSC");
        for project_db in mgr.project_map.iter() {
            // 保存ssc
            async_total_ssc_data(&project_db.value(), &mgr.arango_database).await?;
        }
        dbg!("SSC同步完成");
    }

    //同步到图数据库
    if db_option.rebuild_arangodb {
        dbg!("正在同步图数据库");
        sync_graph_db(mgr.clone(), db_option.clone()).await?;
        dbg!("图数据库同步完成");
    }

    if db_option.gen_model_mesh {
        dbg!("正在生成模型");
        let mut time = Instant::now();
        AiosDBManager::cache_geos_data(mgr.clone(), db_option).await?;
        println!("生成模型花费时间: {} ms", time.elapsed().as_millis());
    }


    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "PIPCA";
    let noun = "PTCA";
    let hash = db1_hash(noun);
    dbg!(hash);
}


#[test]
fn read_info_bin() {
    let info_map = read_attr_info_config_from_bin("all_attr_info.bin");
    let data = serde_json::to_string(&info_map).unwrap();
    let mut file = File::create("all_attr_info_new.json").unwrap();
    file.write(data.as_bytes()).unwrap();
    // let info = info_map.noun_attr_info_map;
    // if let Some(v) = info.get(&621602){
    //     dbg!(&v.value());
    // };
}