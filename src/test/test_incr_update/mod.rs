use std::sync::Arc;
use crate::data_interface::gen_model::gen_all_geos_data;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::test::test_helper::get_test_ams_db_manager_async;


#[tokio::test]
async fn test_watch_update() {
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    //是否需要重构下面的这行代码？
    AiosDBManager::exec_watcher(mgr.clone()).await.expect("test_watch_update: panic message");
}