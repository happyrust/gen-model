use std::sync::Arc;
use crate::fast_model::gen_all_geos_data;
use crate::data_interface::increment_record::IncrGeoUpdateLog;

use crate::test::test_helper::get_test_ams_db_manager_async;
use crate::test::test_query::init_test_surreal;

#[tokio::test]
async fn test_gen_box() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    incr_log.prim_refnos.insert("17496_171666".into());
    gen_all_geos_data(mgr.clone(), Some(incr_log)).await.unwrap();
}

#[tokio::test]
async fn test_gen_loop() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    incr_log.loop_refnos.insert("17496_266255".into());
    gen_all_geos_data(mgr.clone(), Some(incr_log)).await.unwrap();
}

#[tokio::test]
async fn test_gen_cata() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    incr_log.basic_cata_refnos.insert("17496_254421".into());
    gen_all_geos_data(mgr.clone(), Some(incr_log)).await.unwrap();
}