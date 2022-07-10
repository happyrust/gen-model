use std::collections::{BTreeMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use itertools::Itertools;
use aios_core::pdms_types::{AttrMap, AttrVal, DbAttributeType, NounHash, PdmsDatabaseInfo, RefI32Tuple, RefU64};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use dashmap::DashMap;
use futures::StreamExt;
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
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::ssc::async_total_ssc_data;
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
        sync_pdms(&db_option).await?;
    }


    let mgr = Arc::new(AiosDBManager::init_form_config().await?);

    // dbg!(mgr.get_attr(RefU64::from_refno_str("15213/494985").unwrap()).await).expect("TODO: panic message");
    // dbg!(&mgr.dbno_mgr.ref0_dbnos_map.iter().filter(|x| x.1.len() == 1).collect_vec());
    // mgr.dbno_mgr.serialize_to_specify_file("instance/dbno_mgr.num");
    // return Ok(());

    let b_recreate_ssc = false;
    if b_recreate_ssc {
        for project_db in mgr.project_map.iter() {
            // 保存ssc
            async_total_ssc_data(&project_db.value()).await?;
        }
    }
    let mut time = Instant::now();
    AiosDBManager::cache_geos_data(mgr.clone(), db_option).await?;

    // mgr.mesh_mgr.serialize_to_json_file();
    // mgr.mesh_instance_mgr.serialize_to_specify_file("AIOSModel.bin");
    // mgr.mesh_mgr.serialize_to_specify_file("/Users/dongpengcheng/rust-projects/new/AIOSEditor/assets/mesh/AIOSModel.bin");
    // mgr.mesh_mgr.serialize_to_json_file();
    std::fs::create_dir_all("mesh").unwrap();
    mgr.cached_mesh_mgr.serialize_to_specify_file("mesh/mesh.bin");

    std::fs::create_dir_all("instance").unwrap();
    for k in mgr.mesh_instance_mgr.iter() {
        let db_no = *k.key();
        k.value().serialize_to_specify_file(&format!("instance/{db_no}.inst"));
        // k.value().level_shape_mgr.serialize_to_specify_file(&format!("instance/level_{db_no}.bin"));
    }

    mgr.dbno_mgr.serialize_to_specify_file("instance/dbno_mgr.num");

    // mgr.mesh_instance_mgr.inst_mgr.serialize_to_specify_file("inst.bin");
    // mgr.mesh_instance_mgr.level_shape_mgr.serialize_to_specify_file("level.bin");


    println!("花费时间: {} ms", time.elapsed().as_millis());

    Ok(())
}