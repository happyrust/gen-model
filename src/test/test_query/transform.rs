use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

use crate::test::test_helper::get_test_ams_db_manager_async;
use aios_core::RefU64;
use aios_core::SUL_DB;
use std::sync::Arc;
use surrealdb::sql::Thing;

#[tokio::test]
async fn test_query_transform() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    let mgr = get_test_ams_db_manager_async().await;
    let transform = mgr.get_world_transform(refno).await.unwrap().unwrap();
    dbg!(&transform);

    let pe = aios_core::get_pe("17496_107068".into())
        .await
        .unwrap();
    dbg!(pe);
    Ok(())
}
