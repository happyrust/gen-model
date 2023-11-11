use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::surreal_service;
use std::sync::Arc;
use aios_core::RefU64;
use glam::Vec3;
use surrealdb::sql::Thing;
use crate::surreal_service::SUL_DB;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[tokio::test]
async fn test_query_pe_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    //serde_json::from_str("17496_107068").unwrap();
    dbg!(serde_json::to_string(&refno).unwrap());
    let pe = surreal_service::get_pe("17496_107068".into()).await.unwrap();
    dbg!(pe);
    Ok(())
}

#[tokio::test]
async fn test_query_ancestor_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    let type_name = surreal_service::get_type_name(refno).await.unwrap_or_default();
    dbg!(&type_name);
    let ancestor = surreal_service::get_ancestor("17496_107068".into()).await.unwrap();
    dbg!(ancestor);
    let ancestor_maps = surreal_service::get_ancestor_attmaps("17496_107068".into()).await.unwrap();
    dbg!(ancestor_maps);
    Ok(())
}

#[tokio::test]
async fn test_query_wtrans_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    // let wtrans = mgr.get_world_transform("17496_118635".into()).await.unwrap();
    // dbg!(wtrans);
    //todo fix POSL attribute
    // let wtrans = mgr.get_world_transform("17496_107068".into()).await.unwrap();
    // dbg!(wtrans);

    let wtrans = mgr.get_world_transform("17496_259211".into()).await.unwrap();
    assert_eq!(wtrans.unwrap().translation, Vec3::new(79800.0, -19000.0, 3460.0));
    Ok(())
}

#[tokio::test]
async fn test_query_att_by_refno() {
    super::init_test_surreal().await;
    let attmap = surreal_service::get_named_attmap("17496_118635".into()).await;
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