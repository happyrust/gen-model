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

use aios_core::{RefnoEnum, SUL_DB, get_db_option};
use aios_database::data_interface::batch_scheduler::{BatchScheduler, DiscoveredBatch};
use aios_database::data_interface::batch_worker::drain_queue_until_empty;
use aios_database::data_interface::model_update_pending::drain_rooms;
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::room_model::{
    ElementRoomHistory, load_panel_index, load_room_panel_map, recalc_element_membership,
};
use pdms_io::io::PdmsIO;
use serde::Deserialize;
use surrealdb::opt::{Config, auth::Root};

const ELEMENT: &str = "24383_66460";
const PANEL: &str = "24381_35844";
const ROOM: &str = "R512";
const PROJECT: &str = "AvevaMarineSample";
const DBNUM: u32 = 7999;
const DB_FILE: &str = "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7999_0001";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Edge {
    panel: String,
    part: String,
    room_num: String,
}

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

async fn scalar_i32(sql: &str) -> i32 {
    let mut response = SUL_DB
        .query(sql)
        .await
        .expect("query")
        .check()
        .expect("valid query");
    response
        .take::<Option<i32>>(0)
        .expect("decode scalar")
        .expect("scalar exists")
}

async fn edges() -> Vec<Edge> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
             FROM room_relate WHERE out = pe:{ELEMENT} ORDER BY panel;"
        ))
        .await
        .expect("query room edges")
        .check()
        .expect("valid room edge query");
    response.take(0).expect("decode room edges")
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
    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );
    let element = RefnoEnum::from(ELEMENT);

    // 只清本案例两个靶子的陈旧任务，再走生产按需生成与元素房间分支建立确定性基线。
    SUL_DB
        .query("DELETE model_update_pending WHERE action = 'regen_root' AND target_refno IN ['24381/35843', '24383/66459'];")
        .await
        .expect("clear stale target regen work")
        .check()
        .expect("valid target regen cleanup");
    for target in [PANEL, ELEMENT] {
        mgr.ensure_model_generated(RefnoEnum::from(target), false)
            .await
            .unwrap_or_else(|error| panic!("prepare model {target}: {error:#}"));
    }
    let mut db_option = get_db_option().clone();
    db_option.room_key_word = Some(vec!["-RM05-R512".into()]);
    db_option.gen_spatial_tree = true;
    let rooms = load_room_panel_map(&db_option)
        .await
        .expect("load target room");
    let panels = load_panel_index(&db_option, &rooms)
        .await
        .expect("load target panel geometry");
    let history = ElementRoomHistory::load(&[element])
        .await
        .expect("load target room history");
    recalc_element_membership(&rooms, &panels, &history, element)
        .await
        .expect("prepare exact room baseline");

    snapshot("before").await;
    let baseline = edges().await;
    assert_eq!(
        baseline,
        vec![Edge {
            panel: PANEL.into(),
            part: ELEMENT.into(),
            room_num: ROOM.into(),
        }],
        "issue #7 靶子漂移：E3D apply 前必须恰好属于面板 {PANEL} / 房间 {ROOM}"
    );

    // 报告人的第一步。已经删过就是空操作。
    SUL_DB
        .query(format!("DELETE room_relate WHERE out = pe:{ELEMENT};"))
        .await
        .expect("delete room edges")
        .check()
        .expect("valid delete");
    assert!(edges().await.is_empty(), "第一步之后不该还有归属边");

    // 只把 7999 这一批放进队列。不能把现场会话号写死：每次 E3D SAVEWORK 都会递增。
    let db_file = std::env::var("AIOS_ISSUE7_DB_FILE").unwrap_or_else(|_| DB_FILE.into());
    let file_latest_sesno = PdmsIO::new(PROJECT, &db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno") as i32;
    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{DBNUM};"
    ))
    .await;
    assert!(
        file_latest_sesno > applied_sesno,
        "E3D SAVEWORK 后必须有待应用会话，file={file_latest_sesno} applied={applied_sesno}"
    );
    let found = DiscoveredBatch {
        project: PROJECT.to_string(),
        dbnum: DBNUM,
        db_type: "DESI".to_string(),
        path: std::path::PathBuf::from(&db_file),
        file_name: std::path::Path::new(&db_file)
            .file_name()
            .expect("db file name")
            .to_string_lossy()
            .into_owned(),
        applied_sesno,
        file_latest_sesno,
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

    assert!(
        !rows(&format!(
            "SELECT VALUE <string>id FROM model_update_pending:room_recalc_element_{ELEMENT};"
        ))
        .await
        .is_empty(),
        "纯位姿增量必须为 {ELEMENT} 排出 room_recalc_element"
    );

    // 共享实库可能已有超过一页的房间积压；只把本案例自己的确定性任务提到首页。
    SUL_DB
        .query(format!(
            "UPDATE model_update_pending:room_recalc_element_{ELEMENT} \
             SET updated_at = d'1970-01-01T00:00:00Z';"
        ))
        .await
        .expect("prioritize target room task")
        .check()
        .expect("valid target room priority update");

    let rooms_done = drain_rooms(&db_option).await.expect("drain room phase");
    println!("[e2e] 房间轮消化 {rooms_done} 条");

    snapshot("after").await;
    assert_eq!(
        edges().await,
        baseline,
        "issue #7：删掉的归属边必须被增量原样建回来"
    );
}
