//! 暂存 Transform 写路由的实机验证（2026-08-07 修复计划 §3 / 审核 P0 闭环）。
//!
//! 前置：E3D 宏已对目标元素做**纯位姿** SAVEWORK（如 `BY U 10`），fork Surreal
//! 服务器已指向 E2E 数据副本（`scripts/Run-RoomE3DE2E.ps1` 同一套环境）。本测试
//! 跑一轮**暂存窗口**增量——要求 `GEN_MODEL_DIRECT_INCREMENT` 未设置，这正是
//! 与既有 `issue7_e2e_increment`（直写回退路径）的区别——并断言：
//!
//! 0. **前提**（按 `task_id` 读**本批**任务终态，不看进程级指标）：本批 `success`
//!    且零告警、数据 `applied`；且没有重生成靶子的生成根——这次变更在计划层真的
//!    走了 Transform 便宜路径。少了后半条，改判成 `RegenRoot` 时下面 3、4 照样
//!    绿，被测的写路由却一步没走到（假绿）；
//! 1. 批次确实经过暂存写回而非直写（`staged_commit_metrics` 非零；它是进程级的
//!    「最近一次」，靠「这一轮只消费了本批」绑定到本批）；
//! 2. 水位推进到文件最新会话；
//! 3. `inst_relate.world_trans` 改指新 trans 记录且 `.d` 可解引用。注意这**不是**
//!    P0 的判别式：提交成功后 trans 记录已随 journal 落盘，修复前后都解得开。
//!    悬空只在「写回成功前」与「窗口废弃后」可观测，那两个时刻由
//!    `staging::parity` 的两条零落盘用例（中途 diff / 废弃后指针仍自洽）钉住；
//! 4. `inst_relate.aabb` 跟随位姿变化——**修复前必红**的那一条（暂存窗口拿旧
//!    指针算包围盒，恒为旧值）；
//! 5. 房间归属收敛回基线（同房间位移；暂存房间轮在窗口内收敛，残余由
//!    `drain_rooms` 兜底）。同房位移下它抓的是「归属被错误清空/搬错房间」，
//!    抓不到「压根没重算」——判别性要靠跨房搬迁靶子。
//!
//! 默认靶子：CONE =24381/110021（dbnum 7997，R312）——EQUI 名下的几何体，
//! 纯位姿变更在计划层保持 Transform 工作项（不像 BRAN/HANG 成员会改判重生成）。
//! 这个「保持」由断言 0 当场判定，不靠注释保证。
//!
//! ```text
//! $env:RUST_MIN_STACK = "134217728"
//! cargo test --features http_api --test staged_transform_e2e -- --ignored --exact --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use aios_core::{RefnoEnum, SUL_DB, get_db_option};
use aios_database::data_interface::batch_scheduler::{BatchScheduler, DiscoveredBatch};
use aios_database::data_interface::batch_worker::{drain_queue_until_empty, staged_commit_metrics};
use aios_database::data_interface::generation_root::{
    configured_delivery_unit_types, resolve_live_element_generation_root,
};
use aios_database::data_interface::model_update_pending::drain_rooms;
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::room_model::{
    ElementRoomHistory, load_panel_index, load_room_panel_map, recalc_element_membership,
};
use pdms_io::io::PdmsIO;
use serde::Deserialize;
use surrealdb::opt::{Config, auth::Root};

const PROJECT: &str = "AvevaMarineSample";

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

async fn world_trans_id(element: &str) -> Option<String> {
    let mut response = SUL_DB
        .query(format!(
            "RETURN record::id(inst_relate:{element}.world_trans);"
        ))
        .await
        .expect("query world_trans id")
        .check()
        .expect("valid world_trans query");
    response.take(0).expect("decode world_trans id")
}

async fn world_trans_resolvable(element: &str) -> bool {
    let mut response = SUL_DB
        .query(format!(
            "RETURN inst_relate:{element}.world_trans.d != NONE;"
        ))
        .await
        .expect("query world_trans deref")
        .check()
        .expect("valid world_trans deref query");
    response
        .take::<Option<bool>>(0)
        .expect("decode deref")
        .unwrap_or(false)
}

/// 本批任务的终态 `DataBatchTaskResult`。
///
/// 按 `task_id` 精确取——同一轮 drain 会把队列里排着的**所有**批次消费掉
/// （`batch_queue` 是持久化的，上一轮的遗留也在里面），所以进程级指标和
/// 「跑了几个批次」都说不清这一批的死活，只有任务行能。
fn batch_result(task_id: &str) -> serde_json::Value {
    let entry = TaskRegistry::global()
        .get(task_id)
        .expect("本批任务行必须在注册表里");
    entry
        .result
        .unwrap_or_else(|| panic!("本批任务必须已进终态（当前 {:?}）", entry.state))
}

/// 本批终态里被重生成的交付单元根（`a/b`）。
///
/// `units` 只从 `RegenRoot` 工作项与本库 durable 待重试来，所以「靶子的生成根
/// 不在里面」= 这次变更没走重生成路线。
fn regenerated_roots(result: &serde_json::Value) -> Vec<String> {
    result["units"]
        .as_array()
        .expect("终态结果必须带 units")
        .iter()
        .filter_map(|unit| unit["root_refno"].as_str().map(str::to_owned))
        .collect()
}

async fn aabb(element: &str) -> Vec<String> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE <string>aabb.* FROM inst_relate \
             WHERE id = inst_relate:{element} AND aabb != NONE;"
        ))
        .await
        .expect("query aabb")
        .check()
        .expect("valid aabb query");
    response.take(0).expect("decode aabb")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: applies one pending E3D pose-only increment through the staged window"]
async fn staged_transform_follows_a_pure_pose_move() {
    assert!(
        std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").is_none(),
        "本验证针对暂存路径：不要设置 GEN_MODEL_DIRECT_INCREMENT"
    );
    connect_live().await;

    let element_id =
        std::env::var("AIOS_STAGED_E2E_ELEMENT").unwrap_or_else(|_| "24381_110021".into());
    let dbnum: u32 = std::env::var("AIOS_STAGED_E2E_DBNUM")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(7997);
    let db_file = std::env::var("AIOS_STAGED_E2E_DB_FILE").unwrap_or_else(|_| {
        format!("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams{dbnum}_0001")
    });
    let keyword = std::env::var("AIOS_STAGED_E2E_KEYWORD").unwrap_or_else(|_| "-RM".into());
    let element = RefnoEnum::from(element_id.as_str());

    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );
    // `gen_spatial_tree` 自 2026-08-07 起是死键（全仓零读取，空间/房间计算恒开启），
    // 这里不设它——设了会让人以为本用例控制着空间树行为。
    let mut db_option = get_db_option().clone();
    db_option.room_key_word = Some(vec![keyword]);

    // 基线：模型在场 + 房间归属精确收敛，然后取当前归属边为期望终态（同房间位移，
    // 增量前后归属应当一致）。此刻 E3D 文件里已有待应用的移动会话，但库还在旧位置。
    mgr.ensure_model_generated(element, false)
        .await
        .expect("prepare model");
    let rooms = load_room_panel_map(&db_option)
        .await
        .expect("load room map");
    let panels = load_panel_index(&db_option, &rooms)
        .await
        .expect("load panel index");
    let history = ElementRoomHistory::load(&[element])
        .await
        .expect("load room history");
    recalc_element_membership(&rooms, &panels, &history, element)
        .await
        .expect("prepare exact room baseline");
    let baseline_edges = edges(&element_id).await;
    assert!(
        !baseline_edges.is_empty(),
        "验证目标必须有房间归属基线（换一个在房间里的元素）"
    );

    let before_trans = world_trans_id(&element_id)
        .await
        .expect("基线必须有 world_trans 指针");
    let before_aabb = aabb(&element_id).await;
    assert!(!before_aabb.is_empty(), "基线必须有 AABB");
    println!("[staged-e2e] 基线 world_trans={before_trans} 归属边={baseline_edges:?}");

    // 靶子的生成根按**窗口前**的持久态解析，与计划层同一基准（增量是纯位姿，
    // owner 链不动）。下面用它判别这一批到底走没走重生成路线。
    let generation_root =
        resolve_live_element_generation_root(element, &configured_delivery_unit_types())
            .await
            .expect("resolve generation root")
            .expect("验证目标必须解得出生成根")
            .root
            .to_pdms_str();
    println!("[staged-e2e] 靶子生成根={generation_root}");

    // E3D SAVEWORK 之后：文件最新会话必须超过水位，否则没有可验证的增量。
    let file_latest_sesno = PdmsIO::new(PROJECT, &db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno") as i32;
    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum};"
    ))
    .await;
    assert!(
        file_latest_sesno > applied_sesno,
        "E3D SAVEWORK 后必须有待应用会话 file={file_latest_sesno} applied={applied_sesno}"
    );

    let found = DiscoveredBatch {
        project: PROJECT.to_string(),
        dbnum,
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
    println!("[staged-e2e] 入队 dbnum={dbnum}: {outcome:?}");
    let task_id = outcome.info.task_id.clone();
    let commit_before = staged_commit_metrics();

    let started = Instant::now();
    let ran = drain_queue_until_empty(&mgr).await;
    println!(
        "[staged-e2e] 消费了 {ran} 个批次，耗时 {} ms",
        started.elapsed().as_millis()
    );
    // 只消费我们排的这一个批次。多出来的是上一轮的遗留（`batch_queue` 持久化），
    // 那时进程级的写回指标就说不清是谁写的了——环境不干净得当场知道。
    assert_eq!(
        ran, 1,
        "这一轮应当只消费本批（0=队列被暂停或入队被判重；>1=队列里还有遗留批次，\
         环境不干净）"
    );

    let result = batch_result(&task_id);

    // 0. 前提一：本批自己跑成了。带 post-commit 告警的 Partial（副作用失败、房间目标
    //    保留 pending）不许静悄悄通过——后面几条读的是同一次执行的产物。
    assert_eq!(
        result["status"].as_str(),
        Some("success"),
        "本批终态必须成功: {result:#}"
    );
    assert_eq!(
        result["warnings"].as_array().map(Vec::len),
        Some(0),
        "本批不许带告警: {}",
        result["warnings"]
    );
    assert_eq!(
        result["batch"]["status"].as_str(),
        Some("applied"),
        "数据必须真的落库并推进水位: {}",
        result["batch"]
    );

    // 0. 前提二：这次变更在计划层走的是 Transform 便宜路径，不是整根重生成。
    //    重生成会把 world_trans 与 aabb 一并重算，下面 3、4 两条照样会绿——被测的
    //    暂存 Transform 写路由却一步都没被走到。
    let root_refnos = regenerated_roots(&result);
    assert!(
        !root_refnos.contains(&generation_root),
        "本批重生成了 {generation_root}，说明这次变更被改判成 RegenRoot（owner 链上有 \
         BRAN/HANG？宏顺带改了非 POS/ORI 属性？还是这个根本来就有 durable 待重试积压？）\
         ——那样本用例验的不是暂存 Transform 写路由: {root_refnos:?}"
    );

    // 1. 本批走的是暂存轨而不是直写轨。`staged_commit_metrics` 是进程级的「最近一次」，
    //    绑不到具体批次——绑定靠上面的 `ran == 1`：这一轮只消费了我们排的这一个批次，
    //    那个指标就只能是它写的（进程起来时是 0，见 commit_before）。
    let commit_after = staged_commit_metrics();
    assert!(
        commit_after["last_duration_ms"].as_u64().unwrap_or(0) > 0,
        "本批没有经过暂存写回（是不是设置了 GEN_MODEL_DIRECT_INCREMENT 或水位不足 1？）: \
         {commit_before} -> {commit_after}"
    );

    // 2. 水位推进到文件最新会话。
    assert_eq!(
        scalar_i32(&format!(
            "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum};"
        ))
        .await,
        file_latest_sesno,
        "批次成功后必须推进到 E3D 文件的最新会话"
    );

    // 3. world_trans 指针改指新 trans 记录，且新记录已随 journal 落到持久层
    //    （P0 修复前：指针直写持久层、记录只在暂存——这里 `.d` 取不到）。
    let after_trans = world_trans_id(&element_id)
        .await
        .expect("增量后 world_trans 指针必须在场");
    assert_ne!(
        after_trans, before_trans,
        "纯位姿变更后 world_trans 必须改指新 trans 记录"
    );
    assert!(
        world_trans_resolvable(&element_id).await,
        "world_trans 必须指向持久层里存在的 trans 记录（D9 不悬空）"
    );

    // 4. AABB 跟随位姿（P0 修复前：暂存窗口内刷新拿旧指针算包围盒，恒为旧值）。
    let after_aabb = aabb(&element_id).await;
    assert!(!after_aabb.is_empty(), "增量后 AABB 必须在场");
    assert_ne!(
        after_aabb, before_aabb,
        "位姿变化后 AABB 必须由暂存窗口更新到新位置"
    );

    // 5. 房间归属收敛：暂存房间轮多数在窗口内就地收敛（durable pending 无残留），
    //    残余任务由 drain_rooms 兜底；终态必须回到基线（同房间位移）。
    let rooms_done = drain_rooms(&db_option)
        .await
        .expect("drain room phase")
        .done;
    println!("[staged-e2e] 房间轮兜底消化 {rooms_done} 条");
    assert_eq!(
        edges(&element_id).await,
        baseline_edges,
        "同房间位移后归属边必须收敛回基线"
    );

    println!(
        "[staged-e2e] PASS: world_trans {before_trans} -> {after_trans}，AABB 已更新，归属边收敛"
    );
}
