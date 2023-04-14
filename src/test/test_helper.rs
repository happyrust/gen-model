use crate::data_interface::tidb_manager::AiosDBManager;
use config::{Config, ConfigError, Environment, File};
use std::env;
use arangors_lite::collection::CollectionType::{Document, Edge};
use std::sync::Arc;
use arangors_lite::Database;
use tokio::runtime::Runtime;
use crate::graph_db::pdms_arango::*;
use crate::options::DbOption;
use crate::plot_data::hangers;

/// ams 的测试数据库
pub fn get_test_ams_db_manager() -> AiosDBManager {
    let s = Config::builder()
        .add_source(File::with_name("DbOption_ams"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    Runtime::new().unwrap().block_on(
        AiosDBManager::init_form_config()
    ).unwrap()
}


/// ams 的图数据库
pub async fn get_test_ams_arrango_db() -> Database {
    let s = Config::builder()
        .add_source(File::with_name("DbOption_ams"))
        .build().unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    get_arangodb_conn_from_db_option(&db_option).await.unwrap()
}