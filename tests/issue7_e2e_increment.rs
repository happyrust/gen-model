//! E3D 房间端到端：TTY 宏 SAVEWORK 后跑生产增量，精确检查归属边与房间拓扑。
//!
//! 每轮只驱动宏命中的 7997 或 7999 批次。`enqueue_manual_update` 走的是 MDB 声明口径（2026-08-06
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
use aios_database::fast_model::aabb_tree::rebuild_tree_from_pointers;
use aios_database::fast_model::room_model::{
    ElementRoomHistory, load_panel_index, load_room_panel_map, recalc_element_membership,
};
use pdms_io::io::PdmsIO;
use serde::Deserialize;
use surrealdb::opt::{Config, auth::Root};

const ELEMENT: &str = "24383_66460";
const PANEL: &str = "24381_35844";
const ROOM_REF: &str = "24381_35842";
const ROOM: &str = "R512";
const PROJECT: &str = "AvevaMarineSample";

struct Case {
    dbnum: u32,
    db_file: String,
    action: &'static str,
    target: &'static str,
    expect_room: bool,
    prepare_baseline: bool,
    delete_baseline: bool,
}

impl Case {
    fn from_env() -> Self {
        let change = std::env::var("AIOS_ROOM_CHANGE").unwrap_or_else(|_| "element".into());
        let (default_dbnum, action, target) = match change.as_str() {
            "element" => (7999, "room_recalc_element", ELEMENT),
            "room" => (7997, "room_recalc_panel", PANEL),
            other => panic!("unknown AIOS_ROOM_CHANGE={other}"),
        };
        let dbnum = std::env::var("AIOS_ROOM_DBNUM")
            .ok()
            .map(|value| value.parse().expect("AIOS_ROOM_DBNUM must be a u32"))
            .unwrap_or(default_dbnum);
        let default_file =
            format!("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams{dbnum}_0001");
        Self {
            dbnum,
            db_file: std::env::var("AIOS_ROOM_DB_FILE").unwrap_or(default_file),
            action,
            target,
            expect_room: env_flag("AIOS_ROOM_EXPECT_ROOM", true),
            prepare_baseline: env_flag("AIOS_ROOM_PREPARE_BASELINE", true),
            delete_baseline: env_flag("AIOS_ROOM_DELETE_BASELINE", true),
        }
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(default)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Edge {
    panel: String,
    part: String,
    room_num: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Topology {
    room: String,
    panel: String,
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

async fn topology() -> Vec<Topology> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT record::id(in) AS room, record::id(out) AS panel, room_num \
             FROM room_panel_relate WHERE out = pe:{PANEL} ORDER BY room;"
        ))
        .await
        .expect("query room topology")
        .check()
        .expect("valid room topology query");
    response.take(0).expect("decode room topology")
}

async fn snapshot(tag: &str, dbnum: u32) {
    let total =
        rows("SELECT VALUE <string>c FROM (SELECT count() AS c FROM room_relate GROUP ALL);").await;
    let watermark = rows(&format!(
        "SELECT VALUE <string>[dbnum, applied_sesno, file_latest_sesno] \
         FROM dbnum_watermark WHERE dbnum = {dbnum};"
    ))
    .await;
    let pos = rows(&format!("SELECT VALUE <string>POS FROM CAP:{ELEMENT};")).await;
    println!(
        "[e2e/{tag}] 归属边={:?} 房间拓扑={:?} room_relate={total:?} 水位={watermark:?} POS={pos:?}",
        edges().await,
        topology().await
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: applies one pending E3D room increment to the real project db"]
async fn issue7_e2e_room_comes_back_after_e3d_save() {
    connect_live().await;
    let case = Case::from_env();
    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );
    let element = RefnoEnum::from(ELEMENT);

    // 只清本案例靶子的陈旧任务，避免共享实库里的旧 room 任务制造假通过。
    SUL_DB
        .query(format!(
            "DELETE model_update_pending WHERE \
             (action = 'regen_root' AND target_refno IN ['24381/35843', '24383/66459']) \
             OR id = model_update_pending:{0}_{1};",
            case.action, case.target
        ))
        .await
        .expect("clear stale target regen work")
        .check()
        .expect("valid target regen cleanup");
    assert!(
        rows(&format!(
            "SELECT VALUE <string>id FROM model_update_pending:{0}_{1};",
            case.action, case.target
        ))
        .await
        .is_empty(),
        "目标房间任务清理失败"
    );
    let mut db_option = get_db_option().clone();
    db_option.room_key_word = Some(vec!["-RM05-R512".into()]);
    db_option.gen_spatial_tree = true;
    let baseline = vec![Edge {
        panel: PANEL.into(),
        part: ELEMENT.into(),
        room_num: ROOM.into(),
    }];
    let baseline_topology = vec![Topology {
        room: ROOM_REF.into(),
        panel: PANEL.into(),
        room_num: ROOM.into(),
    }];
    if case.prepare_baseline {
        for target in [PANEL, ELEMENT] {
            mgr.ensure_model_generated(RefnoEnum::from(target), false)
                .await
                .unwrap_or_else(|error| panic!("prepare model {target}: {error:#}"));
        }
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
        assert_eq!(edges().await, baseline, "apply 前房间归属基线漂移");
        assert_eq!(
            topology().await,
            baseline_topology,
            "apply 前房间拓扑基线漂移"
        );
    }

    snapshot("before", case.dbnum).await;
    if case.delete_baseline {
        SUL_DB
            .query(format!("DELETE room_relate WHERE out = pe:{ELEMENT};"))
            .await
            .expect("delete room edges")
            .check()
            .expect("valid delete");
        assert!(edges().await.is_empty(), "删除基线边后不该还有归属边");
    }

    // 只把宏命中的这一批放进队列。不能把现场会话号写死：每次 E3D SAVEWORK 都会递增。
    let file_latest_sesno = PdmsIO::new(PROJECT, &case.db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno") as i32;
    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{};",
        case.dbnum
    ))
    .await;
    assert!(
        file_latest_sesno > applied_sesno,
        "E3D SAVEWORK 后必须有待应用会话，file={file_latest_sesno} applied={applied_sesno}"
    );
    let found = DiscoveredBatch {
        project: PROJECT.to_string(),
        dbnum: case.dbnum,
        db_type: "DESI".to_string(),
        path: std::path::PathBuf::from(&case.db_file),
        file_name: std::path::Path::new(&case.db_file)
            .file_name()
            .expect("db file name")
            .to_string_lossy()
            .into_owned(),
        applied_sesno,
        file_latest_sesno,
    };
    let outcome = BatchScheduler::global().enqueue(TaskRegistry::global(), &found);
    println!("[e2e] 入队 {}: {outcome:?}", case.dbnum);

    let started = Instant::now();
    let ran = drain_queue_until_empty(&mgr).await;
    println!(
        "[e2e] 消费了 {ran} 个批次，耗时 {} ms",
        started.elapsed().as_millis()
    );
    snapshot("after-batch", case.dbnum).await;

    assert!(
        !rows(&format!(
            "SELECT VALUE <string>id FROM model_update_pending:{0}_{1};",
            case.action, case.target
        ))
        .await
        .is_empty(),
        "增量必须为 {} 排出 {}",
        case.target,
        case.action
    );

    // 共享实库可能已有超过一页的房间积压；只把本案例自己的确定性任务提到首页。
    SUL_DB
        .query(format!(
            "UPDATE model_update_pending:{0}_{1} \
             SET updated_at = d'1970-01-01T00:00:00Z';",
            case.action, case.target
        ))
        .await
        .expect("prioritize target room task")
        .check()
        .expect("valid target room priority update");

    if case.action == "room_recalc_panel" {
        rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before panel membership recalculation");
    }

    let rooms_done = drain_rooms(&db_option).await.expect("drain room phase");
    println!("[e2e] 房间轮消化 {rooms_done} 条");

    snapshot("after", case.dbnum).await;
    let expected_edges = if case.expect_room { baseline } else { vec![] };
    assert_eq!(edges().await, expected_edges, "元件房间归属未按场景收敛");
    let expected_topology = if case.action == "room_recalc_panel" && !case.expect_room {
        vec![]
    } else {
        baseline_topology
    };
    assert_eq!(topology().await, expected_topology, "房间拓扑未按场景收敛");
}
