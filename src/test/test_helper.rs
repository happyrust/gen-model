use crate::data_interface::tidb_manager::AiosDBManager;
use config::{Config, ConfigError, Environment, File};
use std::env;
use bb8_arangodb::arangors_lite::collection::CollectionType::{Document, Edge};
use std::sync::Arc;
use bb8_arangodb::arangors_lite::Database;
use tokio::runtime::Runtime;
use crate::graph_db::pdms_arango::*;
use crate::plot_data::hangers;
use aios_core::options::DbOption;

/// ams 的测试数据库
pub fn get_test_ams_db_manager() -> AiosDBManager {
    let s = Config::builder()
        .add_source(File::with_name("DbOption_ams"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    futures::executor::block_on(
        AiosDBManager::init_form_config()
    ).unwrap()
}

pub async fn get_test_ams_db_manager_async() -> AiosDBManager {
    let s = Config::builder()
        .add_source(File::with_name("DbOption_ams"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    AiosDBManager::init_form_config().await.unwrap()
}


/// ams 的图数据库
pub async fn connect_test_ams_arrango_db() -> ArPool {
    let s = Config::builder()
        .add_source(File::with_name("DbOption_ams"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    // get_arangodb_conn_from_db_option_for_test(&db_option).await.unwrap()
    connect_arangodb(&db_option).await.unwrap()
}