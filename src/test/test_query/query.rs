use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::surreal_service;
use std::sync::Arc;
use aios_core::RefU64;
use surrealdb::sql::Thing;
use crate::surreal_service::SUL_DB;

#[tokio::test]
async fn test_query_pe_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    //serde_json::from_str("17496_107068").unwrap();
    dbg!( serde_json::to_string(&refno).unwrap());
    let pe = surreal_service::get_pe("17496_107068".into()).await.unwrap();
    dbg!(pe);
    Ok(())
}

#[tokio::test]
async fn test_query_att_by_refno() {
    super::init_test_surreal().await;
    let attmap = surreal_service::get_named_attmap("17496_171555".into()).await;
    dbg!(attmap);
}

#[tokio::test]
async fn test_query_children() {
    super::init_test_surreal().await;
    let refnos = surreal_service::get_children_refnos("17496_171555".into()).await;
    dbg!(refnos);
}

#[tokio::test]
async fn test_query_children_att() {
    super::init_test_surreal().await;
    let children_atts = surreal_service::get_children_named_attmaps("17496_171555".into()).await;
    dbg!(children_atts);
}



#[tokio::test]
async fn test_query_custom() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let mut response = SUL_DB
        .query(r#"(select owner, owner.noun as o_noun from (type::thing("pe", $refno)))[0]"#)
        .bind(("refno", "17496_171555"))
        .await.unwrap();
    let owner_noun: Option<String> = response.take("o_noun").unwrap();
    dbg!(owner_noun);
    let owner: RefU64 = response.take::<Option<String>>("owner")?.unwrap().into();
    // let owner: Option<RefU64> = response.take("owner").unwrap();
    dbg!(owner);
    Ok(())
}