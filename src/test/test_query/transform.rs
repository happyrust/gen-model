use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::surreal_service;
use std::sync::Arc;
use aios_core::RefU64;
use surrealdb::sql::Thing;
use crate::surreal_service::SUL_DB;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[tokio::test]
async fn test_query_transform() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    let mgr = get_test_ams_db_manager_async().await;
    let transform = mgr.get_world_transform(refno).await.unwrap().unwrap();
    dbg!(&transform);


    let pe = surreal_service::get_pe("17496_107068".into()).await.unwrap();
    dbg!(pe);
    Ok(())
}

