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
use aios_core::tool::db_tool::{db1_dehash, db1_hash, read_attr_info_config_json};
use dashmap::DashMap;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
use aios_database::{BATCH_CHUNKS_CNT, tables};
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
async fn test_get_att() -> anyhow::Result<()>{

    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let attr = mgr.get_attr(RefU64::from_two_nums(15192, 77164)).await?;

    dbg!(attr.to_string_hashmap());

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
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    // AiosDBManager::cache_geos_data(mgr.clone(), "Sample", "SAMPLE").await?;
    AiosDBManager::cache_geos_data(mgr.clone(), db_option.project_name.as_str(), db_option.mdb_name.to_uppercase().as_str()).await?;

    // mgr.mesh_mgr.serialize_to_json_file();
    // mgr.mesh_mgr.serialize_to_bin_file("Sample");
    // mgr.mesh_mgr.serialize_to_json_file();

    Ok(())
}