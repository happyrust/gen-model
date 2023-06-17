
//for test
// let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;

use aios_core::pdms_types::RefU64;
use crate::aql_api::pdms_room::query_room_refnos_aql;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper;
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