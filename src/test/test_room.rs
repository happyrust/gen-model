//for test
// let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;

use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use regex::Regex;
use aios_core::pdms_types::UdaMajorType::T;
use crate::aql_api::pdms_room;
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
    let through_room_map = mgr.query_through_element_room_nums(  &[test_equip]).await?;
    dbg!(&through_room_map);

    // assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R532".to_string(), "R320".to_string())));

    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_subfit() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    let test_sbfi = &[RefU64::from_url_refno("17496_145366").unwrap()];
    let through_room_map = mgr.query_through_element_room_nums(test_sbfi).await?;
    dbg!(&through_room_map);
    Ok(())
}

///测试ele所在房间的底和顶标高
#[tokio::test]
async fn test_query_ele_own_room_elevations() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    //有可能该房间未在计算里，所以这里暂时使用实时查询计算，首先查询到相交的room
    //查询到ele对应的aabb
    let target_refno = RefU64::from_str("24381/70704").unwrap();
    let elevs = mgr.query_own_room_panel_elevations(target_refno).await?;
    dbg!(&elevs);

    Ok(())
}

///按类型查询周边的构件，指定范围和类型
#[tokio::test]
async fn test_query_eles_around_target() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    //有可能该房间未在计算里，所以这里暂时使用实时查询计算，首先查询到相交的room
    //查询到ele对应的aabb
    let target_refno = RefU64::from_str("24381/70704").unwrap();
    let refnos = mgr.query_around_eles_within_radius(target_refno, true, None,
                                                     true, vec![],vec![]).await?;
    //支吊架，风管
    dbg!(&refnos);
    let refnos = mgr.query_around_owner_within_radius(target_refno, true, None,
                                                      true, vec!["BRAN"]).await?;

    dbg!(&refnos);
    assert_eq!(refnos[0].to_refno_string().as_str(), "24381/58848");

    //拖臂，桥架
    let target_refno = RefU64::from_str("24383/96911").unwrap();
    // let refnos = mgr.query_around_owner_within_radius(target_refno, true, None,
    //                                                   false, vec![]).await?;
    // dbg!(refnos);
    let refnos = mgr.query_around_owner_within_radius(target_refno, true, None,
                                                      true, vec!["BRAN"]).await?;

    dbg!(&refnos);
    assert_eq!(refnos[0].to_refno_string().as_str(), "24383/95023");

    //拖臂，桥架
    let target_refno = RefU64::from_str("24383/96561").unwrap();
    let refnos = mgr.query_around_owner_within_radius(target_refno, true, None,
                                                      true, vec!["BRAN"]).await?;

    dbg!(&refnos);
    assert_eq!(refnos[0].to_refno_string().as_str(), "24383/94706");

    Ok(())
}

///测试托盘附近的桥架branch
#[tokio::test]
async fn test_query_msup_attach_branch() -> anyhow::Result<()> {

    let mgr = get_test_ams_db_manager_async().await;


    Ok(())
}

// #[tokio::test]
// async fn test_query_through_element_rooms_9() -> anyhow::Result<()> {
//     //测试样例9
//     let mgr = get_test_ams_db_manager_async().await;
//     let room_number = mgr.query_through_element_room_nums( &[&[RefU64::from_url_refno("24383_83869").unwrap()]]).await;
//     assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R430".to_string(), "R461".to_string())));
//     Ok(())
// }



//15组贯穿件房间号测试样例
#[tokio::test]
async fn test_query_through_element_rooms_1() -> anyhow::Result<()> {
    //测试样例1
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_str("24383/83722").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R530".to_string(), "R561".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_2() -> anyhow::Result<()> {
    //测试样例2
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_84073").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R630".to_string(), "R663".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_3() -> anyhow::Result<()> {
    //测试样例3
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83694").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R610".to_string(), "R661".to_string())));

    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_4() -> anyhow::Result<()> {
    //测试样例4
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83561").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R610".to_string(), "R661".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_5() -> anyhow::Result<()> {
    //测试样例5
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_str("24383/83697").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R310".to_string(), "R361".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_6() -> anyhow::Result<()> {
    //测试样例6
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_84009").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R310".to_string(), "R361".to_string())));
    Ok(())
}
#[tokio::test]
async fn test_query_through_element_rooms_7() -> anyhow::Result<()> {
    //测试样例7
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83974").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R310".to_string(), "R361".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_8() -> anyhow::Result<()> {
    //测试样例8
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83939").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R430".to_string(), "R461".to_string())));

    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_9() -> anyhow::Result<()> {
    //测试样例9
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83869").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R430".to_string(), "R461".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_10() -> anyhow::Result<()> {
    //测试样例10
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83995").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R510".to_string(), "R562".to_string())));
    Ok(())
}


#[tokio::test]
async fn test_query_through_element_rooms_11() -> anyhow::Result<()> {
    //测试样例11
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83729").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R530".to_string(), "R561".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_12() -> anyhow::Result<()> {
    //测试样例12
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_84079").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R630".to_string(), "R663".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_13() -> anyhow::Result<()> {
    //测试样例13
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83596").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R610".to_string(), "R661".to_string())));
    Ok(())
}


#[tokio::test]
async fn test_query_through_element_rooms_14() -> anyhow::Result<()> {
    //测试样例14
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83708").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R710".to_string(), "R761".to_string())));
    Ok(())
}

#[tokio::test]
async fn test_query_through_element_rooms_15() -> anyhow::Result<()> {
    //测试样例15
    let mgr = get_test_ams_db_manager_async().await;
    let room_number = mgr.query_through_element_room_nums(&[RefU64::from_url_refno("24383_83813").unwrap()]).await;
    assert_eq!(room_number.unwrap().iter().next().map(|x| x.1.clone()), Some(("R710".to_string(), "R761".to_string())));
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
