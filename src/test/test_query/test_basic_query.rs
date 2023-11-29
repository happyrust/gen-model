use crate::data_interface::interface::PdmsDataInterface;
use crate::surreal_service;
use crate::test::test_helper::get_test_ams_db_manager_async;
use aios_core::SUL_DB;
use aios_core::{NamedAttrMap, RefU64};
use glam::Vec3;
use std::sync::Arc;

#[tokio::test]
async fn test_query_pe_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    // let pe = surreal_service::get_pe("17496/254421".into()).await.unwrap();
    let pe = surreal_service::get_pe("17496_266621".into())
        .await
        .unwrap();
    dbg!(pe);
    Ok(())
}

#[tokio::test]
async fn test_query_ancestor_by_refno() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let refno: RefU64 = "17496_107068".into();
    let type_name = surreal_service::get_type_name(refno)
        .await
        .unwrap_or_default();
    dbg!(&type_name);
    let ancestor = surreal_service::get_ancestor("17496_107068".into())
        .await
        .unwrap();
    dbg!(ancestor);
    let ancestor_maps = surreal_service::get_ancestor_attmaps("17496_107068".into())
        .await
        .unwrap();
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

    let wtrans = mgr
        .get_world_transform("17496_259211".into())
        .await
        .unwrap();
    assert_eq!(
        wtrans.unwrap().translation,
        Vec3::new(79800.0, -19000.0, 3460.0)
    );
    Ok(())
}

#[tokio::test]
async fn test_query_att_by_refno() {
    super::init_test_surreal().await;
    let attmap = surreal_service::get_named_attmap("15302_2194".into()).await;
    dbg!(attmap);
}

#[tokio::test]
async fn test_query_children() {
    super::init_test_surreal().await;
    let refnos = surreal_service::get_children_refnos("17496_171104".into()).await;
    dbg!(refnos);
    let refnos = surreal_service::get_children_refnos("17496_171188".into()).await;
    dbg!(refnos);
    let refnos = surreal_service::get_children_refnos("17496_0".into()).await;
    dbg!(refnos);
}

#[tokio::test]
async fn test_query_children_att() {
    super::init_test_surreal().await;
    // let children_atts = surreal_service::get_children_named_attmaps("17496_171555".into()).await;
    // dbg!(children_atts);
    // let children_pes = surreal_service::get_children_pes("17496_171555".into()).await;
    // dbg!(children_pes);
    let children_atts = surreal_service::get_children_named_attmaps("15192_338265".into())
        .await
        .unwrap();
    dbg!(children_atts);
}

#[tokio::test]
async fn test_query_custom() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let mut response = SUL_DB
        .query(r#"(select owner, owner.noun as o_noun from (type::thing("pe", $refno)))[0]"#)
        .bind(("refno", "17496_171555"))
        .await
        .unwrap();
    let owner_noun: Option<String> = response.take("o_noun").unwrap();
    dbg!(owner_noun);
    let owner: RefU64 = response.take::<Option<String>>("owner")?.unwrap().into();
    dbg!(owner);
    Ok(())
}

#[tokio::test]
async fn test_query_cata() -> anyhow::Result<()> {
    super::init_test_surreal().await;

    let cat_refno = surreal_service::get_cat_refno("17496_266621".into())
        .await
        .unwrap();
    dbg!(cat_refno);
    // get_cat_attmap
    let cat_attmap = surreal_service::get_cat_attmap("17496_266621".into())
        .await
        .unwrap();
    dbg!(cat_attmap);
    Ok(())
}

#[tokio::test]
async fn test_query_path() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let cat_refno =
        surreal_service::query_single_map_by_paths("15194/5835".into(), &["->GMRE"], &[])
            .await
            .unwrap();
    dbg!(cat_refno);
    Ok(())
}

#[tokio::test]
async fn test_query_paths() -> anyhow::Result<()> {
    super::init_test_surreal().await;
    let cat_refno = surreal_service::query_single_map_by_paths(
        "15194/5835".into(),
        &["->GMRE", "->GMSR"],
        &["id"],
    )
    .await
    .unwrap();
    dbg!(cat_refno);
    Ok(())
}

#[tokio::test]
async fn test_query_record_link() -> anyhow::Result<()> {
    super::init_test_surreal().await;

    let str1 = r#"
            {
    "id": "25688_32684",
    "refno": FITT:25688_32684,
    "owner": pe:25688_32682,
    "name": "/1AR07WW0002R",
    "noun": "FITT",
    "dbnum": 1112,
    "e3d_version": 0,
    "cata_hash": "110329119932332",
    "lock": false
}
        "#;

    let str2 = r#"
    {
         "id": "25688_32682",
        "refno": TEST:25688_32682,
        "owner": pe:25688_32681,
        "name": "/1AR07WW0002R",
        "noun": "TEST",
        "dbnum": 1112,
        "e3d_version": 0,
        "cata_hash": "110329119932332",
        "lock": false
    }
        "#;

    // let d1: pdms_element::Model = serde_json::from_str(
    //     str1
    // ).unwrap();
    //
    // let d2: pdms_element::Model = serde_json::from_str(
    // str2
    // ).unwrap();

    SUL_DB
        .query("delete pe:25688_32684; delete pe:25688_32682;")
        // .query("use ns 1516;use db AvevaMarineSample;INSERT IGNORE INTO pe $values")
        // .bind(("values", &[str1, str2]))
        .await
        .unwrap();

    // let response = SUL_DB
    //     .query("use ns 1516;use db AvevaMarineSample;INSERT IGNORE INTO pe $values")
    //     .bind(("values", &[serde_json::to_string_pretty(&d1).unwrap(),
    //         serde_json::to_string_pretty(&d2).unwrap()]))
    //     .await
    //     .unwrap();
    // let s1 = serde_json::to_string_pretty(&d1).unwrap();
    // let s2 = serde_json::to_string_pretty(&d2).unwrap();
    // let mut v1: serde_json::Value = serde_json::to_value(d1.clone()).unwrap();
    // v1.as_object_mut().unwrap().insert("owner".into(), format!("pe:{}", d1.owner.to_string()).into());
    //
    // let mut v2: serde_json::Value = serde_json::to_value(d2).unwrap();
    // v2.as_object_mut().unwrap().insert("owner".into(), format!("pe:{}", "0_0").into());

    let mut sql = format!(
        r#"use ns 1516;use db AvevaMarineSample;INSERT IGNORE INTO pe [{}, {}];"#,
        // serde_json::to_string_pretty(&[str1, str2]).unwrap()
        str1,
        str2
    );

    println!("{}", &sql);

    let response = SUL_DB
        .query(&sql)
        // .query("use ns 1516;use db AvevaMarineSample;INSERT IGNORE INTO pe $values")
        // .bind(("values", &[str1, str2]))
        .await
        .unwrap();

    let q = SUL_DB
        .query("select owner.* from pe:25688_32684;")
        .await
        .unwrap();
    dbg!(q);
    // let q1 = SUL_DB.query("select owner.* from pe:17496_100102;").await.unwrap();
    // dbg!(q1);
    //
    Ok(())
}
