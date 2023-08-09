
//for test
// let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;

use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use regex::Regex;
use aios_core::pdms_types::UdaMajorType::T;
use crate::aql_api::pdms_room;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper::get_test_ams_db_manager_async;

///  测试获取有负实体的parent
#[tokio::test]
async fn test_query_refnos_has_neg_geom() -> anyhow::Result<()> {
    let test_room_refno = RefU64::from_refno_str("24381/35621").unwrap();
    // let mgr = get_test_ams_db_manager_async().await;
    // let result = interface.query_refnos_has_neg_pos_map(refno).await?;
    // let arango_db = get_arangodb_conn_from_db_option_for_test();
    // dbg!(&result);
    // query_refnos_has_neg_map
    // let result = query_room_refnos_aql(test_room_refno, None, &arango_db).await?;
    // dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_equip() -> anyhow::Result<()> {

    let mgr = get_test_ams_db_manager_async().await;

    let test_equip = RefU64::from_str("24383_83638").unwrap();
    let through_room_map = pdms_room::query_through_element_room_nums(&mgr, &[test_equip]).await?;
    dbg!(&through_room_map);

    // assert_eq!(room_number.unwrap(), Some(("R532".to_string(), "R320".to_string())));

    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_subfit() -> anyhow::Result<()> {
    //测试样例1
    // let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83638").unwrap()).await;
    // assert_ne!(room_number.unwrap(), Some(("R661".to_string(), "".to_string())));

    // //测试样例2
    // let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83589").unwrap()).await;
    // assert_ne!(room_number.unwrap(), Some(("R661".to_string(), "".to_string())));
    //
    // //测试样例3
    // let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83960").unwrap()).await;
    // assert_ne!(room_number.unwrap(), Some(("R361".to_string(), "".to_string())));

    let mgr = get_test_ams_db_manager_async().await;

    let test_sbfi = RefU64::from_url_refno("17496_145366").unwrap();
    let through_room_map = pdms_room::query_through_element_room_panels(&mgr, &[test_sbfi]).await?;
    dbg!(&through_room_map);

    // assert_eq!(room_number.unwrap(), Some(("R532".to_string(), "R320".to_string())));

    Ok(())
}

///测试ele所在房间的底和顶标高
#[tokio::test]
async fn test_query_ele_own_room_elevations() -> anyhow::Result<()> {

    let mgr = get_test_ams_db_manager_async().await;
    //有可能该房间未在计算里，所以这里暂时使用实时查询计算，首先查询到相交的room
    //查询到ele对应的aabb

    Ok(())
}

///测试托盘附近的桥架branch
#[tokio::test]
async fn test_query_msup_attach_branch() -> anyhow::Result<()> {

    let mgr = get_test_ams_db_manager_async().await;

    Ok(())
}



#[tokio::test]
async fn test_query_refno_belong_rooms() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    use aios_core::options::DbOption;
    use crate::aql_api::pdms_room;
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_url_refno("24383_68084").unwrap();
    let name = pdms_room::query_refno_belong_rooms(refno, &database).await?;
    dbg!(&name);
    Ok(())
}

#[tokio::test]
async fn test_query_room_info_from_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    use aios_core::options::DbOption;
    use crate::aql_api::pdms_room;
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_url_refno("24381_178638").unwrap();
    let name = pdms_room::query_room_info_from_refno(refno, "FRMW", &database).await?.unwrap();
    let room_name = pdms_room::get_room_name_split(&name).unwrap();
    dbg!(&room_name);
    Ok(())
}


#[test]
fn test_json() {
    let str = vec![T];
    let json = serde_json::to_string(&str).unwrap();
    dbg!(&json);
}

#[test]
fn test_match_room_name() {
    let re = Regex::new(r"^/\d+[A-Z]{2}-RM\d{2}-R\d{3}$").unwrap();

    dbg!(re.is_match("/123AB-RM03-R310"));
    dbg!(re.is_match("/456CD-RM03-R312"));
    dbg!(re.is_match("/789EF-RM11-R976"));
    dbg!(!re.is_match("/1RA-RM03-R312"));
    dbg!(!re.is_match("/1NX-RM11-R976"));
    dbg!(!re.is_match("/12A-RM11-R976"));
}
