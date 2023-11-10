use std::sync::Arc;
use crate::data_interface::gen_model::gen_all_geos_data;
use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::surreal_service;
use crate::test::test_helper::get_test_ams_db_manager_async;
use crate::test::test_query::init_test_surreal;

#[tokio::test]
async fn test_gen_box() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    // incr_log.prim_refnos.push("17496_171666".into());
    incr_log.loop_refnos.push("17496_266255".into());
    // dbg!(surreal_service::get_pe("17496_266255".into()).await);
    // dbg!(surreal_service::get_named_attmap("17496_266255".into()).await);
    // dbg!(surreal_service::get_children_named_attmaps("17496_266255".into()).await);
    gen_all_geos_data(mgr.clone(), Some(incr_log)).await.unwrap();
}