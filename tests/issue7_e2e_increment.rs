//! issue #7 端到端：E3D 那边 SAVEWORK 之后，跑一遍生产增量，看房间归属回不回来。
//!
//! 走的是报告人的两步：先 `DELETE` 掉这个构件的归属边，再让增量把 E3D 刚写下的
//! 会话应用进来（扫描 → 入队 → 消费 → 房间轮），最后断言那条边原样回来。
//!
//! ```text
//! cargo test --features http_api --test issue7_e2e_increment -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use aios_core::{SUL_DB, get_db_option};
use aios_database::data_interface::batch_worker::drain_queue_until_empty;
use aios_database::data_interface::model_update_pending::drain_rooms;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use surrealdb::opt::{Config, auth::Root};

const ELEMENT: &str = "24383_66460";
const PROJECT: &str = "AvevaMarineSample";

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

async fn snapshot(tag: &str) {
    let edges = rows(&format!(
        "SELECT VALUE <string>[id, room_num] FROM room_relate WHERE out = pe:{ELEMENT};"
    ))
    .await;
    let total = rows("SELECT VALUE <string>c FROM (SELECT count() AS c FROM room_relate GROUP ALL);").await;
    let watermark = rows(
        "SELECT VALUE <string>[dbnum, applied_sesno, file_latest_sesno] \
         FROM dbnum_watermark WHERE dbnum = 7999;",
    )
    .await;
    let pos = rows(&format!("SELECT VALUE <string>POS FROM CAP:{ELEMENT};")).await;
    println!("[e2e/{tag}] 归属边={edges:?} room_relate={total:?} 7999水位={watermark:?} POS={pos:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: applies the pending 7999 increment to the real project db"]
async fn issue7_e2e_room_comes_back_after_e3d_save() {
    connect_live().await;
    snapshot("before").await;

    // 报告人的第一步：手动删掉这个构件的房间归属边。
    SUL_DB
        .query(format!("DELETE room_relate WHERE out = pe:{ELEMENT};"))
        .await
        .expect("delete room edges")
        .check()
        .expect("valid delete");
    snapshot("edge-deleted").await;

    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );

    let preview = mgr
        .preview_manual_update(PROJECT, None)
        .await
        .expect("preview manual update");
    println!("[e2e] 预览: {} 个 dbnum 有待应用窗口", preview.dbnums.len());
    for dbnum in &preview.dbnums {
        println!("[e2e]   {dbnum:?}");
    }

    let receipt = mgr.enqueue_manual_update(PROJECT, None).await;
    println!(
        "[e2e] 入队回执: mdb={} ns={} scanned={} enqueued={:?}",
        receipt.mdb, receipt.namespace, receipt.scanned, receipt.enqueued
    );

    let started = Instant::now();
    let ran = drain_queue_until_empty(&mgr).await;
    println!("[e2e] 消费了 {ran} 个批次，耗时 {} ms", started.elapsed().as_millis());

    let mut db_option = get_db_option().clone();
    db_option.gen_spatial_tree = true;
    let rooms_done = drain_rooms(&db_option).await.expect("drain room phase");
    println!("[e2e] 房间轮消化 {rooms_done} 条");

    snapshot("after").await;

    let edges = rows(&format!(
        "SELECT VALUE <string>[id, room_num] FROM room_relate WHERE out = pe:{ELEMENT};"
    ))
    .await;
    assert!(
        !edges.is_empty(),
        "issue #7：删掉的归属边必须被增量建回来，实得空集"
    );
}