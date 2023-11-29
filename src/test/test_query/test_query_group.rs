use crate::data_interface::interface::PdmsDataInterface;
use crate::surreal_service;
use crate::test::test_helper::get_test_ams_db_manager_async;
use aios_core::RefU64;
use aios_core::SUL_DB;
use glam::Vec3;
use std::sync::Arc;

#[tokio::test]
async fn test_group_cata_hash() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    // let refnos: Vec<RefU64> = vec!["15302_2194".into()];
    let refnos: Vec<RefU64> = vec!["24381_47118".into()];
    let group = surreal_service::query_group_by_cata_hash(&refnos)
        .await
        .unwrap();
    dbg!(&group);
    Ok(())
}
