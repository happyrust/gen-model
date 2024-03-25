//for test
// let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;

use crate::aql_api::pdms_room;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper::get_test_ams_db_manager_async;
use aios_core::pdms_types::RefU64;
use aios_core::pdms_types::UdaMajorType::T;
use glam::Vec3;
use regex::Regex;
use std::str::FromStr;
use aios_core::pdms_types::GeoBasicType::*;
use parry3d::utils::hashmap::HashMap;
use crate::data_interface::interface::PdmsDataInterface;

///  测试获取有负实体的parent
#[tokio::test]
async fn test_query_refnos_has_neg_geom() -> anyhow::Result<()> {
    let test_room_refno = RefU64::from_str("24381/35621").unwrap();
    // let mgr = get_test_ams_db_manager_async().await;
    // let result = interface.query_refnos_has_neg_pos_map(refno).await?;
    // let arango_db = get_arangodb_conn_from_db_option_for_test();
    // dbg!(&result);
    // query_refnos_has_neg_map
    // let result = query_room_refnos_aql(test_room_refno, None, &arango_db).await?;
    // dbg!(&result);
    Ok(())
}

// //15组贯穿件房间号测试样例，只算出了内房间的情况
#[tokio::test]
async fn test_query_through_element_rooms_1() -> anyhow::Result<()> {
    //测试样例1   内房间号：R610，外房间号：R661
    let mgr = get_test_ams_db_manager_async().await;
    let target_refno = "24383/83477".into();

    let room_number_map = mgr
        .query_through_element_room_nums(&[target_refno], None)
        .await?;
    dbg!(room_number_map);
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_3() -> anyhow::Result<()> {
    //测试样例1   内房间号：R610，外房间号：R661
    let mgr = get_test_ams_db_manager_async().await;
    let target_refno = "17496/156874".into();

    let room_number_map = mgr
        .query_through_element_room_nums(&[target_refno], Some(&vec![Neg, CateNeg,CataCrossNeg]))
        .await?;
    dbg!(room_number_map);
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_4() -> anyhow::Result<()> {
    //测试样例1   内房间号：R610，外房间号：R661
    let mgr = get_test_ams_db_manager_async().await;
    let target_refno = "17496/145284".into();

    let room_number_map = mgr
        .query_through_element_room_nums(&[target_refno], Some(&vec![Neg, CateNeg,CataCrossNeg]))
        .await?;
    dbg!(room_number_map);
    Ok(())
}


///测试查询点所在的房间
#[tokio::test]
async fn test_query_rooms_pts() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    let pts = vec![Vec3::new(10271.33, -140.43, 14275.37)];

    let room_nums = mgr
        .query_pts_own_room_number(&pts)
        .await?;
    dbg!(room_nums);
    Ok(())
}


#[tokio::test]
async fn test_query_through_element_rooms_sbfi() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    let target_refno = "17496/143434".into();

    let room_number_map = mgr
        .query_through_element_room_nums(&[target_refno], None)
        .await?;
    dbg!(room_number_map);
    Ok(())
}


///测试房间号是否正确
#[tokio::test]
async fn test_query_through_element_rooms_2() -> anyhow::Result<()> {
    //测试样例2
    use std::collections::{HashMap, HashSet};
    let mgr = get_test_ams_db_manager_async().await;
    let target_refno = "24383/83477".into();
    let room_number = mgr
        .query_through_element_room_nums(&[target_refno], None)
        .await?;
    let mut map = HashMap::new();
    map.insert(target_refno, ("R610".to_string(), "R661".to_string()));
    assert_eq!(room_number, map);
    Ok(())
}

// #[tokio::test]
// async fn test_query_through_element_rooms_3() -> anyhow::Result<()> {
//     //测试样例3
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83694").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));
//
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_4() -> anyhow::Result<()> {
//     //测试样例4
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83561").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_5() -> anyhow::Result<()> {
//     //测试样例5
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83697").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_6() -> anyhow::Result<()> {
//     //测试样例6
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_84009").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));
//     Ok(())
// }
// #[tokio::test]
// async fn test_query_through_element_rooms_7() -> anyhow::Result<()> {
//     //测试样例7
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83974").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R310".to_string(), "R361".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_8() -> anyhow::Result<()> {
//     //测试样例8
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83939").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R430".to_string(), "R461".to_string())));
//
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_9() -> anyhow::Result<()> {
//     //测试样例9
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83869").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R430".to_string(), "R461".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_10() -> anyhow::Result<()> {
//     //测试样例10
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83995").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R510".to_string(), "R562".to_string())));
//     Ok(())
// }
//
//
// #[tokio::test]
// async fn test_query_through_element_rooms_11() -> anyhow::Result<()> {
//     //测试样例11
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83729").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R530".to_string(), "R561".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_12() -> anyhow::Result<()> {
//     //测试样例12
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_84079").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R630".to_string(), "R663".to_string())));
//     Ok(())
// }
//
// #[tokio::test]
// async fn test_query_through_element_rooms_13() -> anyhow::Result<()> {
//     //测试样例13
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83596").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R610".to_string(), "R661".to_string())));
//     Ok(())
// }
//
//
// #[tokio::test]
// async fn test_query_through_element_rooms_14() -> anyhow::Result<()> {
//     //测试样例14
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83708").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R710".to_string(), "R761".to_string())));
//     Ok(())
// }

// #[tokio::test]
// async fn test_query_through_element_rooms_15() -> anyhow::Result<()> {
//     //测试样例15
//     let room_number = pdms_room::query_through_element_rooms(RefU64::from_str("24383_83813").unwrap()).await;
//     assert_eq!(room_number.unwrap(), Some(("R710".to_string(), "R761".to_string())));
//     Ok(())
// }

#[tokio::test]
async fn test_query_refno_belong_rooms() -> anyhow::Result<()> {
    use crate::aql_api::pdms_room;
    use aios_core::options::DbOption;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_str("24383_68084").unwrap();
    let name = pdms_room::query_refno_belong_rooms(refno, &database).await?;
    dbg!(&name);
    Ok(())
}

#[tokio::test]
async fn test_query_room_info_from_refno() -> anyhow::Result<()> {
    use crate::aql_api::pdms_room;
    use aios_core::options::DbOption;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let mgr = get_test_ams_db_manager_async().await;
    let refno = RefU64::from_str("24381_178638").unwrap();
    let name = mgr
        .query_room_names_of_ele(refno)
        .await?
        .into_iter()
        .next()
        .unwrap_or_default();
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

#[tokio::test]
async fn test_query_room_of_refno() -> anyhow::Result<()> {
    //测试样例1
    let mgr = get_test_ams_db_manager_async().await;
    let room_refnos = mgr.query_room_refno_of_ele("17496/198243".into()).await?;
    dbg!(room_refnos);
    let room_names = mgr.query_room_names_of_ele("17496/198243".into()).await?;
    dbg!(room_names);
    let rooms = mgr.query_room_eles_of_ele("17496/198243".into()).await?;
    dbg!(rooms);
    let around_eles = mgr
        .get_refnos_within_bound_radius("17496/198243".into(), 100.0)
        .await?;
    dbg!(around_eles);
    //
    Ok(())
}
