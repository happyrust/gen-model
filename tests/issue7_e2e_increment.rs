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
//!
//! `AIOS_ROOM_IDEMPOTENT=1` 时不再驱动增量，而是断言「第二遍是无操作」：
//! 水位守卫成立（file == applied）、队列消费为零、水位/归属边/AABB/拓扑原地不动
//! （无人值守回归的幂等检查）。

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
use serde::{Deserialize, Serialize};
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
    target: String,
    element: String,
    noun_refno: String,
    expected_noun: String,
    expect_room: bool,
    prepare_baseline: bool,
    delete_baseline: bool,
    dynamic_baseline: bool,
    check_topology: bool,
    room_keyword: String,
    expected_edges: Option<Vec<Edge>>,
    expect_geometry: bool,
    baseline_file: Option<std::path::PathBuf>,
}

impl Case {
    fn from_env() -> Self {
        let change = std::env::var("AIOS_ROOM_CHANGE").unwrap_or_else(|_| "element".into());
        let (default_dbnum, action, target) = match change.as_str() {
            "element" => (
                7999,
                "room_recalc_element",
                std::env::var("AIOS_ROOM_ELEMENT").unwrap_or_else(|_| ELEMENT.into()),
            ),
            "room" => (7997, "room_recalc_panel", PANEL.into()),
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
            element: std::env::var("AIOS_ROOM_ELEMENT").unwrap_or_else(|_| ELEMENT.into()),
            noun_refno: std::env::var("AIOS_ROOM_EXPECT_NOUN_REFNO").unwrap_or_else(|_| {
                std::env::var("AIOS_ROOM_ELEMENT").unwrap_or_else(|_| ELEMENT.into())
            }),
            expected_noun: std::env::var("AIOS_ROOM_EXPECT_NOUN").unwrap_or_else(|_| "CAP".into()),
            expect_room: env_flag("AIOS_ROOM_EXPECT_ROOM", true),
            prepare_baseline: env_flag("AIOS_ROOM_PREPARE_BASELINE", true),
            delete_baseline: env_flag("AIOS_ROOM_DELETE_BASELINE", true),
            dynamic_baseline: env_flag("AIOS_ROOM_DYNAMIC_BASELINE", false),
            check_topology: env_flag("AIOS_ROOM_CHECK_TOPOLOGY", true),
            room_keyword: std::env::var("AIOS_ROOM_KEYWORD")
                .unwrap_or_else(|_| "-RM05-R512".into()),
            expected_edges: std::env::var("AIOS_ROOM_EXPECT_EDGES")
                .ok()
                .map(|value| serde_json::from_str(&value).expect("AIOS_ROOM_EXPECT_EDGES JSON")),
            expect_geometry: env_flag("AIOS_ROOM_EXPECT_GEOMETRY", true),
            baseline_file: std::env::var_os("AIOS_ROOM_BASELINE_FILE").map(Into::into),
        }
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(default)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct Edge {
    panel: String,
    part: String,
    room_num: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedBaseline {
    edges: Vec<Edge>,
    aabb: Vec<String>,
    panel_members: Option<Vec<String>>,
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

async fn edges(element: &str) -> Vec<Edge> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT record::id(in) AS panel, record::id(out) AS part, room_num \
             FROM room_relate WHERE out = pe:{element} ORDER BY panel;"
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

async fn panel_members(panel: &str) -> Vec<String> {
    let mut members = rows(&format!(
        "SELECT VALUE <string>record::id(out) FROM room_relate WHERE in = pe:{panel};"
    ))
    .await;
    members.sort();
    members
}

async fn aabb(element: &str) -> Vec<String> {
    rows(&format!(
        "SELECT VALUE <string>aabb.* FROM inst_relate \
         WHERE id = inst_relate:{element} AND aabb != NONE;"
    ))
    .await
}

async fn snapshot(tag: &str, case: &Case) {
    let total =
        rows("SELECT VALUE <string>c FROM (SELECT count() AS c FROM room_relate GROUP ALL);").await;
    let watermark = rows(&format!(
        "SELECT VALUE <string>[dbnum, applied_sesno, file_latest_sesno] \
         FROM dbnum_watermark WHERE dbnum = {};",
        case.dbnum
    ))
    .await;
    let model = rows(&format!(
        "SELECT VALUE <string>[noun, POS, AABB] FROM pe:{};",
        case.element
    ))
    .await;
    println!(
        "[e2e/{tag}] 归属边={:?} 房间拓扑={:?} room_relate={total:?} 水位={watermark:?}",
        edges(&case.element).await,
        topology().await
    );
    println!("[e2e/{tag}] 模型={model:?}");
}

/// restore 收敛后的第二遍
/// 必须是无操作。生产入口的水位守卫拦在入队之前——`file_latest == applied` 的
/// 空区间根本不进队列（`batch_queue` 空区间规则），所以这里断言守卫成立、
/// 队列消费为零、水位与归属边与 AABB 与拓扑全部原地不动。
async fn assert_second_pass_is_a_no_op(mgr: &Arc<AiosDBManager>, case: &Case) {
    assert_eq!(
        rows(&format!(
            "SELECT VALUE <string>noun FROM pe:{};",
            case.noun_refno
        ))
        .await,
        vec![case.expected_noun.clone()],
        "幂等轮：E3D 模型类型基线漂移"
    );
    let file_latest_sesno = PdmsIO::new(PROJECT, &case.db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno") as i32;
    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{};",
        case.dbnum
    ))
    .await;
    assert_eq!(
        file_latest_sesno, applied_sesno,
        "幂等轮前置：restore 收敛后不得再有待应用会话"
    );
    let edges_before = edges(&case.element).await;
    let aabb_before = aabb(&case.element).await;
    let topology_before = topology().await;
    let ran = drain_queue_until_empty(mgr).await;
    assert_eq!(ran, 0, "幂等轮不得消费任何数据批次");
    assert_eq!(
        scalar_i32(&format!(
            "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{};",
            case.dbnum
        ))
        .await,
        applied_sesno,
        "幂等轮水位不得移动"
    );
    assert_eq!(
        edges(&case.element).await,
        edges_before,
        "幂等轮归属边不得变化"
    );
    assert_eq!(
        aabb(&case.element).await,
        aabb_before,
        "幂等轮 AABB 不得变化"
    );
    if case.check_topology {
        assert_eq!(topology().await, topology_before, "幂等轮房间拓扑不得变化");
    }
    assert!(
        rows(&format!(
            "SELECT VALUE <string>id FROM model_update_pending:{0}_{1};",
            case.action, case.target
        ))
        .await
        .is_empty(),
        "幂等轮不得留下本案例的房间任务"
    );
    println!(
        "[e2e/idem] 第二遍零工作：dbnum={} 水位 {applied_sesno} 未动，边/AABB/拓扑不变",
        case.dbnum
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
    if env_flag("AIOS_ROOM_IDEMPOTENT", false) {
        assert_second_pass_is_a_no_op(&mgr, &case).await;
        return;
    }
    let element = RefnoEnum::from(case.element.as_str());

    assert_eq!(
        rows(&format!(
            "SELECT VALUE <string>noun FROM pe:{};",
            case.noun_refno
        ))
        .await,
        vec![case.expected_noun.clone()],
        "E3D 模型类型基线漂移"
    );

    // 只清本案例靶子的陈旧房间任务，避免共享实库里的旧任务制造假通过。
    SUL_DB
        .query(format!(
            "DELETE model_update_pending:{0}_{1};",
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
    db_option.room_key_word = Some(vec![case.room_keyword.clone()]);
    db_option.gen_spatial_tree = true;
    let mut baseline = vec![Edge {
        panel: PANEL.into(),
        part: case.element.clone(),
        room_num: ROOM.into(),
    }];
    let baseline_topology = vec![Topology {
        room: ROOM_REF.into(),
        panel: PANEL.into(),
        room_num: ROOM.into(),
    }];
    if case.prepare_baseline {
        let prepare_targets = if case.dynamic_baseline {
            vec![case.element.as_str()]
        } else {
            vec![PANEL, case.element.as_str()]
        };
        for target in prepare_targets {
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
        if case.dynamic_baseline {
            baseline = edges(&case.element).await;
            if case.action == "room_recalc_element" {
                assert_eq!(
                    !baseline.is_empty(),
                    case.expect_room,
                    "动态房间归属基线与案例声明不符"
                );
            }
        } else {
            assert_eq!(
                edges(&case.element).await,
                baseline,
                "apply 前房间归属基线漂移"
            );
        }
        if case.check_topology {
            assert_eq!(
                topology().await,
                baseline_topology,
                "apply 前房间拓扑基线漂移"
            );
        }
    } else if case.dynamic_baseline && case.baseline_file.is_none() {
        baseline = edges(&case.element).await;
    }

    snapshot("before", &case).await;
    let before_aabb = aabb(&case.element).await;
    if case.prepare_baseline {
        assert_eq!(
            !before_aabb.is_empty(),
            case.expect_geometry,
            "增量前模型几何基线与案例声明不符"
        );
    }
    let saved_baseline = if let Some(path) = &case.baseline_file {
        Some(if case.prepare_baseline {
            let saved = SavedBaseline {
                edges: baseline.clone(),
                aabb: before_aabb.clone(),
                panel_members: if case.action == "room_recalc_panel" {
                    Some(panel_members(&case.target).await)
                } else {
                    None
                },
            };
            std::fs::write(
                path,
                serde_json::to_vec(&saved).expect("serialize baseline"),
            )
            .expect("write baseline");
            saved
        } else {
            serde_json::from_slice(&std::fs::read(path).expect("read baseline"))
                .expect("decode baseline")
        })
    } else {
        None
    };
    if let Some(saved) = &saved_baseline {
        baseline = saved.edges.clone();
    }
    if case.delete_baseline {
        SUL_DB
            .query(format!(
                "DELETE room_relate WHERE out = pe:{};",
                case.element
            ))
            .await
            .expect("delete room edges")
            .check()
            .expect("valid delete");
        assert!(
            edges(&case.element).await.is_empty(),
            "删除基线边后不该还有归属边"
        );
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
    let applied_after = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{};",
        case.dbnum
    ))
    .await;
    let file_latest_after = PdmsIO::new(PROJECT, &case.db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno after batch") as i32;
    assert!(
        applied_after >= file_latest_sesno && applied_after <= file_latest_after,
        "批次成功后必须至少推进到捕获会话且不得越过当前文件: captured={file_latest_sesno} applied={applied_after} file={file_latest_after}"
    );
    snapshot("after-batch", &case).await;
    if !before_aabb.is_empty() {
        let after_aabb = aabb(&case.element).await;
        if case.prepare_baseline {
            assert_ne!(
                after_aabb, before_aabb,
                "模型属性变化后 AABB 必须由增量更新"
            );
        } else if let Some(saved) = &saved_baseline {
            assert_eq!(
                after_aabb, saved.aabb,
                "restore 后 AABB 必须回到 apply 前基线"
            );
        }
    }
    if !case.expect_geometry {
        assert!(
            aabb(&case.element).await.is_empty(),
            "无独立几何类型不应生成 AABB"
        );
    }

    let has_room_task = !rows(&format!(
        "SELECT VALUE <string>id FROM model_update_pending:{0}_{1};",
        case.action, case.target
    ))
    .await
    .is_empty();
    assert_eq!(
        has_room_task, case.expect_geometry,
        "{} 的 {} 排队状态与几何声明不符",
        case.target, case.action
    );

    // 共享实库可能已有超过一页的房间积压；只把本案例自己的确定性任务提到首页。
    if has_room_task {
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
    }

    if case.action == "room_recalc_panel" {
        rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before panel membership recalculation");
    }

    let rooms_done = drain_rooms(&db_option).await.expect("drain room phase");
    println!("[e2e] 房间轮消化 {rooms_done} 条");

    snapshot("after", &case).await;
    let expected_edges = case
        .expected_edges
        .clone()
        .unwrap_or_else(|| if case.expect_room { baseline } else { vec![] });
    assert_eq!(
        edges(&case.element).await,
        expected_edges,
        "元件房间归属未按场景收敛"
    );
    let expected_topology = if case.action == "room_recalc_panel" && !case.expect_room {
        vec![]
    } else {
        baseline_topology
    };
    if case.check_topology {
        assert_eq!(topology().await, expected_topology, "房间拓扑未按场景收敛");
    }
    if case.action == "room_recalc_panel"
        && case.prepare_baseline
        && let Some(expected) = saved_baseline
            .as_ref()
            .and_then(|saved| saved.panel_members.as_ref())
    {
        assert_ne!(
            &panel_members(&case.target).await,
            expected,
            "面板变化后成员集合必须发生预期重算"
        );
    }
    if case.action == "room_recalc_panel"
        && !case.prepare_baseline
        && let Some(expected) = saved_baseline.and_then(|saved| saved.panel_members)
    {
        assert_eq!(
            panel_members(&case.target).await,
            expected,
            "面板恢复后成员集合必须逐项回到 apply 前基线"
        );
    }
}
