//for test
// let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;

use aios_core::pdms_types::RefU64;
use regex::Regex;
use aios_core::pdms_types::UdaMajorType::T;
use crate::aql_api::pdms_room;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

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
async fn test_query_through_element_rooms() -> anyhow::Result<()> {
    //测试样例1
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83722").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R530".to_string(), "R561".to_string())));

    //测试样例2
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_84073").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R630".to_string(), "R663".to_string())));

    //测试样例3
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83694").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));

    //测试样例4
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83561").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));

    //测试样例5
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83697").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));




    //测试样例6
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_84009").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));

    //测试样例7
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83974").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));

    //测试样例8
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83939").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R430".to_string(), "R461".to_string())));

    //测试样例9
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83869").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R430".to_string(), "R461".to_string())));

    //测试样例10
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83995").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R510".to_string(), "R562".to_string())));




    //测试样例11
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83729").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R530".to_string(), "R561".to_string())));

    //测试样例12
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_84079").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R630".to_string(), "R663".to_string())));

    //测试样例13
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83596").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));

    //测试样例14
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83708").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R710".to_string(), "R761".to_string())));

    //测试样例15
    let room_number = pdms_room::query_through_element_rooms(RefU64::from_url_refno("24383_83813").unwrap()).await;
    assert_ne!(room_number.unwrap(), Some(("R710".to_string(), "R761".to_string())));




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
