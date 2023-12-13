use std::sync::Arc;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::test::test_helper::get_test_ams_db_manager_async;
use crate::test::test_query::init_test_surreal;

#[tokio::test]
async fn test_watch_update() {
    init_test_surreal().await;
    let mgr = Arc::new(get_test_ams_db_manager_async().await);
    //是否需要重构下面的这行代码？
    futures::executor::block_on(async {
        AiosDBManager::exec_watcher(mgr.clone()).await.expect("watcher error");
    });

}