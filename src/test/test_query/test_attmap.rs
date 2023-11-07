use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::surreal_service;
use std::sync::Arc;

#[tokio::test]
async fn test_query_att_by_refno() {
    super::init_test_surreal().await;
    let attmap = surreal_service::get_named_attmap("17496_107068".into()).await;
    dbg!(attmap);
}

#[tokio::test]
async fn test_query_children() {
    super::init_test_surreal().await;
    let refnos = surreal_service::get_children_refnos("17496_171555".into()).await;
    dbg!(refnos);
}
