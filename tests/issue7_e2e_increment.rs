//! issue #7 端到端：E3D 那边 SAVEWORK 之后，跑一遍生产增量，看房间归属回不回来。
//!
//! 只驱动 7999 这一个批次。`enqueue_manual_update` 走的是 MDB 声明口径（2026-08-06
//! 起手写名单不再收窄增量范围，issue #10 的修法），一调就会把 MDB 里 14 个从没导入过
//! 的库一并排进来——那是另一件事，会把本次结论淹掉。
//!
//! ```text
//! $env:RUST_MIN_STACK = "134217728"
//! cargo test --features http_api --test issue7_e2e_increment -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use aios_core::{SUL_DB, get_db_option};
use aios_database::data_interface::batch_scheduler::{BatchScheduler, DiscoveredBatch};
use aios_database::data_interface::batch_worker::drain_queue_until_empty;
use aios_database::data_interface::model_update_pending::drain_rooms;
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use surrealdb::opt::{Config, auth::Root};

const ELEMENT: &str = "24383_66460";
const PROJECT: &str = "AvevaMarineSample";
const DBNUM: u32 = 7999;
const DB_FILE: &str = "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7999_0001";

async fn connect_live() {
    let endpoint = std::env::var("AIOS_LIVE_WS").unwrap_or_else(|_| "ws://localhost:8009".into());
    let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
    let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());
    SUL_DB
        .connect((endpoint, Config::default().ast_payload()))
        .with_capacity(1000)
        .await
        .expect("connect live");
    SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
    SUL_DB
        .signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
}

async fn rows(sql: &str) -> Vec<String> {
    let mut response = SUL_DB
        .query(sql)
        .await
        .expect("query")
        .check()
        .expect("valid query");
    response.take(0).expect("decode")
}

async fn edges() -> Vec<String> {
    rows(&format!(
        "SELECT VALUE <string>[record::id(id), room_num] FROM room_relate WHERE out = pe:{ELEMENT};"
    ))
    .await
}

async fn snapshot(tag: &str) {
    let total =
        rows("SELECT VALUE <string>c FROM (SELECT count() AS c FROM room_relate GROUP ALL);").await;
    let watermark = rows(&format!(
        "SELECT VALUE <string>[dbnum, applied_sesno, file_latest_sesno] \
         FROM dbnum_watermark WHERE dbnum = {DBNUM};"
    ))
    .await;
    let pos = rows(&format!("SELECT VALUE <string>POS FROM CAP:{ELEMENT};")).await;
    println!(
        "[e2e/{tag}] 归属边={:?} room_relate={total:?} 水位={watermark:?} POS={pos:?}",
        edges().await
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: applies the pending 7999 increment to the real project db"]
async fn issue7_e2e_room_comes_back_after_e3d_save() {
    connect_live().await;
    snapshot("before").await;

    // 报告人的第一步。已经删过就是空操作。
    SUL_DB
        .query(format!("DELETE room_relate WHERE out = pe:{ELEMENT};"))
        .await
        .expect("delete room edges")
        .check()
        .expect("valid delete");
    assert!(edges().await.is_empty(), "第一步之后不该还有归属边");

    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );

    // 只把 7999 这一批放进队列。左端 worker 执行时会自己重读水位，这里给的是入队用的。
    let found = DiscoveredBatch {
        project: PROJECT.to_string(),
        dbnum: DBNUM,
        db_type: "DESI".to_string(),
        path: std::path::PathBuf::from(DB_FILE),
        file_name: "ams7999_0001".to_string(),
        applied_sesno: 41,
        file_latest_sesno: 42,
    };
    let outcome = BatchScheduler::global().enqueue(TaskRegistry::global(), &found);
    println!("[e2e] 入队 7999: {outcome:?}");

    let started = Instant::now();
    let ran = drain_queue_until_empty(&mgr).await;
    println!(
        "[e2e] 消费了 {ran} 个批次，耗时 {} ms",
        started.elapsed().as_millis()
    );
    snapshot("after-batch").await;

    let mut db_option = get_db_option().clone();
    db_option.gen_spatial_tree = true;
    let rooms_done = drain_rooms(&db_option).await.expect("drain room phase");
    println!("[e2e] 房间轮消化 {rooms_done} 条");

    snapshot("after").await;
    assert!(
        !edges().await.is_empty(),
        "issue #7：删掉的归属边必须被增量建回来，实得空集"
    );
}