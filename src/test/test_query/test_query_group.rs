use crate::data_interface::interface::PdmsDataInterface;
use crate::surreal_service;
use std::sync::Arc;
use aios_core::RefU64;
use glam::Vec3;
use crate::surreal_service::SUL_DB;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[tokio::test]
async fn test_query_pe_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refnos: Vec<RefU64> = vec!["15302_2194".into()];
    let group = surreal_service::query_group_by_cata_hash(&refnos).await.unwrap();
    // dbg!(&group);
    Ok(())
}