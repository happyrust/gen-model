use crate::fast_model::gen_all_geos_data;
use crate::data_interface::increment_record::IncrGeoUpdateLog;

use crate::test::test_helper::get_test_ams_db_manager_async;
use crate::test::test_query::init_test_surreal;
use std::sync::Arc;

#[tokio::test]
async fn test_gen_bran() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    //17496_171190
    incr_log.bran_hanger_refnos.insert("17496/171134".into());
    // incr_log.bran_hanger_refnos.insert("17496_266620".into());
    gen_all_geos_data(mgr.clone(), Some(incr_log))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_gen_ele_of_bran() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    let mut incr_log = IncrGeoUpdateLog::default();
    // incr_log.basic_cata_refnos.insert("17496/266632".into());
    // incr_log.basic_cata_refnos.insert("17496/266621".into());
    incr_log.basic_cata_refnos.insert("17496_266828".into());
    gen_all_geos_data(mgr.clone(), Some(incr_log))
        .await
        .unwrap();
}
