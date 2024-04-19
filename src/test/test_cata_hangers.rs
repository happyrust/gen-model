use crate::graph_db::pdms_arango::save_arangodb_with_db_option;
use config::{Config, ConfigError, Environment, File};
use std::env;

use std::sync::Arc;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::plot_data::hangers;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper;
use crate::test::test_helper::get_test_ams_db_manager;

#[tokio::test]
async fn test_save_hangers_data() -> anyhow::Result<()> {
    // let _ = dotenv::dotenv();
    // let url = env::var("DATABASE_URL")?;
    // let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    //
    //
    // let database = get_arangodb_conn_from_db_option_for_test().await?;
    // create_arango_document(&database, "hanger_data", Document).await?;
    // create_arango_document(&database, "hanger_edges", Edge).await?;
    //
    // let mgr = Arc::new(test_helper::get_test_ams_db_manager());
    // let data = hangers::save_hangers_data(mgr.clone()).await?;
    // if let Some(data) = data {
    //     let json = serde_json::to_value(&vec![data]).unwrap();
    //     save_arangodb_with_db_option(json, &mgr.db_option, "hanger_data").await?;
    // }
    Ok(())
}
