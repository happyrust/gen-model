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
use aios_core::tool::db_tool::{db1_hash, read_attr_info_config, /*read_attr_info_config_from_json*/};
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

#[test]
fn change_att_info_data() {
    let mut config = read_attr_info_config_from_json("all_attr_info.json");
    let mut att = &mut config.noun_attr_info_map;
    for mut kv  in att.iter_mut() {
        for mut kkv in kv.iter_mut() {
            if kkv.name == "LEVE" {
                kkv.att_type = DbAttributeType::INTVEC;
                kkv.default_val = aios_core::pdms_types::AttrVal::IntArrayType(Vec::new());
            }
        }
    }
    // if let Some(value) = att.get(&(db1_hash("DB") as i32)){
    //     if let Some(mut v) = value.value().get_mut(&865153){
    //         v.att_type = aios_core::pdms_types::DbAttributeType::INTEGER;
    //         v.default_val = aios_core::pdms_types::AttrVal::IntegerType(1);
    //     }
    // };
    // config.noun_attr_info_map = att;
    let mut file = File::create("all_attr_info_new.json").unwrap();
    file.write((&serde_json::to_string(&config).unwrap()).as_ref()).expect("TODO: panic message");

    // 查看是否修改成功
    // let att = read_attr_info_config("all_attr_info_new.bin").noun_attr_info_map;
    // if let Some(value) = att.get(&(db1_hash("DB") as i32)) {
    //     if let Some(mut v) = value.value().get(&865153) {
    //         println!("v={:?}", v.value());
    //     }
    // };
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
    AiosDBManager::cache_geos_data(mgr.clone(), "Sample", "SAMPLE").await?;

    // mgr.mesh_mgr.serialize_to_json_file();
    mgr.mesh_mgr.serialize_to_bin_file("Sample");
    mgr.mesh_mgr.serialize_to_json_file();

    Ok(())
}