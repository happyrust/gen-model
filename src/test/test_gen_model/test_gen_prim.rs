use std::sync::Arc;
use crate::data_interface::gen_model::gen_all_geos_data;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[tokio::test]
async fn test_gen_box() {
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    gen_all_geos_data(mgr.clone(), Some(vec!["17496/171678".into()])).await.unwrap();
}