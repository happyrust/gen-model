//! Intentionally red regressions. Only a new embedded mem:// fixture is written;
//! no SUL_DB, configuration, source models or process-global mirror are accessed.

use super::*;

#[derive(Deserialize)]
struct Alignment {
    #[serde(default)]
    model_room_generation: Option<String>,
}

async fn alignment(db: &surrealdb::Surreal<surrealdb::engine::any::Any>) -> Option<String> {
    let row: Option<Alignment> = db
        .query("SELECT model_room_generation FROM ONLY dbnum_watermark:7000;")
        .await
        .expect("query fixture alignment")
        .check()
        .expect("alignment statement succeeds")
        .take(0)
        .expect("decode fixture alignment");
    row.and_then(|row| row.model_room_generation)
}

async fn count(db: &surrealdb::Surreal<surrealdb::engine::any::Any>, table: &str) -> u64 {
    #[derive(Deserialize)]
    struct CountRow {
        count: u64,
    }
    let counts: Vec<CountRow> = db
        .query(format!("SELECT count() AS count FROM {table} GROUP ALL;"))
        .await
        .expect("count fixture edges")
        .check()
        .expect("count statement succeeds")
        .take(0)
        .expect("decode fixture edge count");
    counts.first().map_or(0, |row| row.count)
}

#[tokio::test]
async fn room_rebuild_regression_same_generation_failure_keeps_alignment_owed() {
    let db = surrealdb::engine::any::connect("mem://")
        .await
        .expect("new isolated memory database");
    db.use_ns("room_rebuild_regression")
        .use_db("same_generation_failure")
        .await
        .expect("fixture namespace and database");
    db.query(
        "CREATE pe:room; CREATE pe:panel; CREATE pe:member; \
         RELATE pe:panel->room_relate:edge->pe:member; \
         RELATE pe:room->room_panel_relate:edge->pe:panel; \
         CREATE dbnum_watermark:7000 SET applied_sesno = 1, model_sesno = 1, \
           model_room_generation = 'g1', model_room_stray_generations = [];",
    )
    .await
    .expect("create fixture")
    .check()
    .expect("valid fixture SQL");
    assert_eq!(count(&db, "room_relate").await, 1);
    assert_eq!(count(&db, "room_panel_relate").await, 1);
    let before = alignment(&db).await;
    assert_eq!(before.as_deref(), Some("g1"));
    assert!(
        crate::data_interface::room_recompute::library_alignment_owed(
            before.as_deref(),
            Some("g1")
        )
        .is_none(),
        "control: original edges are aligned"
    );

    // Execute the real production clear boundary. Inject failure immediately
    // afterwards by publishing neither replacement edges nor a success stamp.
    super::clear_room_rebuild_edges_on(&db)
        .await
        .expect("clear fixture edges");
    assert_eq!(count(&db, "room_relate").await, 0);
    assert_eq!(count(&db, "room_panel_relate").await, 0);
    let after = alignment(&db).await;
    assert!(
        crate::data_interface::room_recompute::library_alignment_owed(after.as_deref(), Some("g1"))
            .is_some(),
        "cleared edges plus failed same-generation recompute must remain owed; alignment={after:?}"
    );
}

#[test]
fn room_rebuild_regression_success_stamp_has_mirror_invalidation_contract() {
    // Structural contract, not a cache-behavior test. Do not initialize the
    // process-global database/mirror to inspect this write boundary.
    let source = include_str!("../data_interface/room_recompute.rs");
    let body = source
        .split_once("async fn recompute_dbnum_room_edges_now(")
        .expect("recompute boundary exists")
        .1
        .split_once("\n#[cfg(test)]")
        .expect("recompute boundary ends before the test module")
        .0;
    let write = body
        .find("UPDATE dbnum_watermark:")
        .expect("room generation stamp exists");
    let invalidate = body
        .find("DbnumState::invalidate_mirrored(dbnum)")
        .or_else(|| body.find("DbnumState::invalidate_mirrored_on("));
    assert!(
        invalidate.is_some_and(|position| position > write),
        "successful room generation stamp must invalidate its mirror after the write (source contract)"
    );
}

/// 同一格的**第二个镜像**：库上 `model_room_generation` 的进程镜像
/// （`ModelMemoryStore::stamped_room_generations`，`ack_is_stray_room_evidence` 拿它判一枚 ACK 还能不能逐）。
///
/// 上一枚钉子只护到 `DbnumState` 那一层镜像；逐出候选看的是这一层。库上那一格改了、这一层没跟上的两种后果：
/// 库上盖了**新**代而镜像还留**旧**代 → 旧代的 ACK 被当成「库上已经是这一代」放行逐出，可它是
/// 「那几根的边是另一代算的」的唯一证据（`room_recompute::request_for_task` 的准入与
/// `query_service::room_calculation_status` 的一票否决都靠它）；清边那一笔把**整表**写成 `NONE` 而镜像
/// 没清 → 同样放行。两处写库的地方因此都要镜像，且都在库上确认成功之后。
///
/// 仍是源码契约守卫（与第 3 条同一种证据等级）：它证明调用在写库之后、且在成功分支那一侧；
/// 「真的跑到那一支」靠代码评审。
#[test]
fn room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror() {
    let source = include_str!("../data_interface/room_recompute.rs");
    let body = source
        .split_once("async fn recompute_dbnum_room_edges_now(")
        .expect("recompute boundary exists")
        .1
        .split_once("\n#[cfg(test)]")
        .expect("recompute boundary ends before the test module")
        .0;
    let write = body
        .find("UPDATE dbnum_watermark:")
        .expect("room generation stamp exists");
    let mirror = body
        .find("note_room_generation_stamped(dbnum, Some(")
        .expect("整库重算盖章之后要把这一代告诉进程逐出镜像（否则另一代的 ACK 会被误判成库上那一代、放行逐出）");
    assert!(mirror > write, "先写库、确认成功之后再镜像");
    assert!(
        !body[write..mirror].contains("Err("),
        "镜像要落在成功分支那一侧（写库与镜像之间不该出现失败分支）"
    );

    let rebuild = include_str!("handlers.rs")
        .split_once("async fn run_rooms_rebuild(")
        .expect("rebuild body exists")
        .1;
    let clear = rebuild
        .find("clear_room_rebuild_edges_on(")
        .expect("清边那一笔存在");
    let wipe = rebuild
        .find("clear_stamped_room_generations(")
        .expect("清边把整表写成 NONE，进程逐出镜像要跟着整表清（否则旧代号仍放行逐出）");
    assert!(wipe > clear, "清库成功之后才清镜像");
}

/// `/health` 的欠账点名（issue #57）：`room_build.alignment_debt` 必须由第三条电平触发
/// 同一个判据（`DurableAlignment::owed()`）产出，不许长出一个并行的简化判据——不然
/// health 报的欠账和触发真正会补的欠账对不上（少报 = 以为收敛其实在欠；多报 = 以为欠账
/// 触发却不来）。这一格不作数时（开关关着 / 非落盘 / 常驻没装上）必须是 `null`，
/// 不能冒充零欠账。
#[test]
fn room_rebuild_regression_health_alignment_debt_uses_the_same_owed_verdict() {
    let source = include_str!("../data_interface/room_recompute.rs");
    let face = source
        .split_once("async fn alignment_debt_health(")
        .expect("欠账点名面存在")
        .1
        .split_once("\n/// 一段换代")
        .expect("欠账点名面边界")
        .0;
    assert!(
        face.contains("is_durable()"),
        "非落盘形态这一面不作数（水位行活不过重启）"
    );
    assert!(
        face.contains("frozen_room_calculator"),
        "常驻代号从冻计算器拿——开关关着 / 没装上时这面不答话"
    );
    assert!(
        face.contains("DurableAlignment") && face.contains(".owed()"),
        "欠账判定复用第三条电平触发同一个 DurableAlignment::owed()，不长并行判据"
    );
    assert!(
        face.contains("stray_generations_written"),
        "欠账含 stray 账（盖章之后按别代写过根的证词）"
    );
    // 期望集拆分（2026-10-17 线上实证）：注册水位行的库比整库重建的期望清单宽——
    // 期望集外的欠账（重建够不着、只有任务轮到才追）要和期望集内的分开报，
    // 不然那批永远还不掉的账会训练操作员对这面免疫。
    assert!(
        face.contains("expected_dbnums()") && face.contains("owed_outside_scope"),
        "欠账要按期望集（重建逐库清单的同源口径）拆「还得掉 / 够不着」两格"
    );
    // 「为什么欠」格（issue #57 第二半）：逐库失败原因不能再只活在 stderr——
    // 每个欠账条目要挂上最近一次整库重算的留底（`recompute_attempt_for`），
    // 远端不读日志就能看见 ENOENT 一类原因。
    assert!(
        face.contains("recompute_attempt_for") && face.contains("\"last_attempt\""),
        "欠账条目要挂最近一次重算留底（last_attempt）"
    );
    let recompute = source
        .split_once("async fn recompute_dbnum_room_edges_now(")
        .expect("整库重算入口存在")
        .1;
    assert!(
        recompute.matches("note_recompute_attempt").count() >= 5,
        "recompute_dbnum_room_edges_now 的每个失败出口都要记一笔——漏一个出口，health 就答不出那一种欠: {recompute}"
    );
    let handlers = include_str!("handlers.rs");
    assert!(
        handlers.contains(
            r#""alignment_debt": crate::data_interface::room_recompute::alignment_debt_health().await"#
        ),
        "room_build 要挂上 alignment_debt——不读 stderr 就能看见哪几间库欠着整库房间重算"
    );
}

/// 「在飞」不许只活在展示帧里（2026-10-15）：整库重建的第三段跑在 `Ready` 下，而任务开工前对齐的
/// 换代（`refresh_before_task` → `initialize_file_backed_at_startup` → `mark_loading`）会把展示帧
/// **清零重写成别的相位**——拿 `phase` 与逐库计数当准入判据，换代一过闸就被抹开，第二趟
/// 「重新生成房间」踩着第一趟的逐库循环进来，两趟交错清边、互相覆盖盖章（#53 的老面孔）。
/// 因此：「在飞」单列一格（`ResidentProgress::edge_recompute_in_flight`），清零展示帧时由
/// `reset_progress_in` 从被换掉的那帧接手，准入读那一格；`run_rooms_rebuild` 的第三段由守卫
/// （`start_edge_recompute`，见 `fast_model::room_resident::RoomRebuildFlight`）开工、由它的
/// `Drop` 放开——手工成对在 panic 展开的路上到不了（第五笔）。
/// 行为侧（换代清零之后闸仍拒、循环跑完闸重开）见
/// `fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate`。
#[test]
fn room_rebuild_regression_the_edge_loop_flight_is_not_a_display_frame_fact() {
    let source = include_str!("handlers.rs");
    let rebuild = source
        .split_once("async fn run_rooms_rebuild(")
        .expect("rebuild body exists")
        .1;
    let guard = rebuild
        .find("let _flight = resident::start_edge_recompute(")
        .expect("第三段开工：相位 / 库数 / 「在飞」由守卫一次立起，且要绑在名字上活到函数末尾");
    let loop_start = rebuild.find("for dbnum in &dbnums {").expect("逐库循环存在");
    assert!(guard < loop_start, "开工在循环之前: {rebuild}");
    assert!(
        !rebuild.contains("note_edge_recompute_finished("),
        "放开只许走守卫 Drop（panic 展开的那条路上手工成对到不了）: {rebuild}"
    );

    let resident = include_str!("../fast_model/room_resident.rs");
    let reset = resident
        .split_once("fn reset_progress_in(progress: &RwLock<ResidentProgress>, phase: &str) {")
        .expect("reset_progress_in exists")
        .1;
    let reset_body = reset
        .split_once("\r\n}\r\n")
        .or_else(|| reset.split_once("\n}\n"))
        .expect("reset_progress_in body ends")
        .0;
    assert!(
        reset_body.contains("edge_recompute_in_flight: in_flight"),
        "清零展示帧要接手「在飞」那一格: {reset_body}"
    );
    assert!(
        reset_body.contains("let in_flight = slot.edge_recompute_in_flight;")
            && !reset_body.contains("dbnums_done"),
        "接手只读「在飞」那一格；相位 + 计数是展示，停在中途不是判据（第五笔）: {reset_body}"
    );

    let gate = resident
        .split_once("fn try_begin_rebuild_in(")
        .expect("rebuild gate exists")
        .1;
    let gate_head = &gate[..gate
        .find("reset_progress_in(progress, \"source-snapshot\")")
        .expect("gate admits after the checks")];
    assert!(
        gate_head.contains("edge_recompute_in_flight"),
        "准入读那一格，不只读相位: {gate_head}"
    );
    assert!(
        !gate_head.contains("edge-recompute") && !gate_head.contains("dbnums_done"),
        "准入不许再读相位 / 计数兜底——循环里炸掉之后停在半路的展示帧会把重建口焊死到重启: {gate_head}"
    );
}

/// 收尾那一笔落定之前不许放开「在飞」（第五笔）：守卫活到函数末尾，`run_rooms_rebuild` 里既没有
/// 提前的手工放开，收尾（写 `room_update_record`）也落在守卫的作用域之内。
#[test]
fn room_rebuild_regression_the_flight_is_held_until_the_settle_lands() {
    let source = include_str!("handlers.rs");
    let rebuild = source
        .split_once("async fn run_rooms_rebuild(")
        .expect("rebuild body exists")
        .1;
    let end = rebuild
        .find("\n}\r\n")
        .or_else(|| rebuild.find("\n}\n"))
        .unwrap_or(rebuild.len());
    let body = &rebuild[..end];
    let guard = body
        .find("start_edge_recompute(")
        .expect("守卫开工：第三段的「在飞」由它持有");
    let loop_start = body.find("for dbnum in &dbnums {").expect("逐库循环存在");
    let settle = body
        .find("refresh_room_update_record_with_model(")
        .expect("收尾要把「这一版全好了」写成记录");
    assert!(guard < loop_start, "守卫在循环之前立起: {body}");
    assert!(loop_start < settle, "收尾在循环之后: {body}");
    assert!(
        !body.contains("note_edge_recompute_finished("),
        "放开只走守卫 Drop——它活到函数末尾（收尾之后），谁都不许提前手工放开: {body}"
    );
}

/// 限定域口径（2026-10-15，用户）：**watch 目前只是作为增量监测的范围，不要限定房间的初始化**。
/// `room_resident` 早有同一条源码钉（`room_files_never_consult_the_watch_scope`：房间是参照系，
/// 限定域圈的是数据批次）。这里把房间侧另外两处也钉上：
///
/// - 启动整库房间初始化（`check_and_init_rooms_at_startup` 的 `NeedsFullInit` 臂）：目标库集合是
///   `room_dbnums ∪ expected_dbnums`，**不看 `watch_scope`**。此前它按 `admits` 过滤（全被滤掉时才退回
///   `room_dbnums`），未监听库的房间因此从没被初始化过；
/// - 整库重建（`run_rooms_rebuild` 的逐库列表）：同上，此前也按 `admits` 过滤。
///
/// 增量那一臂**保留** `watch_scope`（监测范围）——但它跳过的库必须在记录里留白
/// （`hold_unchecked_room_sesnos`），不得替它说「已看过最新」。
#[test]
fn room_initialization_and_the_full_rebuild_never_consult_the_watch_scope() {
    let source = include_str!("handlers.rs");
    let rebuild = source
        .split_once("async fn run_rooms_rebuild(")
        .expect("rebuild body exists")
        .1
        .split_once("\n}\r\n")
        .or_else(|| source.split_once("async fn run_rooms_rebuild(").expect("rebuild body exists").1.split_once("\n}\n"))
        .expect("rebuild body ends")
        .0;
    assert!(
        rebuild.contains("expected_dbnums()"),
        "重建逐库列表来自本次初始化覆盖到的设计库号: {rebuild}"
    );
    assert!(
        !rebuild.contains("watch_scope"),
        "整库重建不得按限定域圈库（房间是参照系，不是增量的一部分）: {rebuild}"
    );

    let room_recompute = include_str!("../data_interface/room_recompute.rs");
    let startup = room_recompute
        .split_once("pub(crate) async fn check_and_init_rooms_at_startup(")
        .expect("startup room check exists")
        .1;
    let full_init = startup
        .split_once("StartupRoomCheckPlan::NeedsFullInit => {")
        .expect("full init arm exists")
        .1
        .split_once("StartupRoomCheckPlan::UpToDate")
        .expect("full init arm ends at the up-to-date arm")
        .0;
    assert!(
        full_init.contains("startup_full_init_targets("),
        "首次整库初始化的目标集合走那个纯函数（∪，不看限定域）: {full_init}"
    );
    assert!(
        !full_init.contains("watch_scope"),
        "首次整库初始化不得被限定域过滤: {full_init}"
    );

    let incremental = startup
        .split_once("StartupRoomCheckPlan::NeedsIncrementalCheck {")
        .expect("incremental arm exists")
        .1;
    assert!(
        incremental.contains("watch_scope::admits(dbnum)"),
        "增量监测的范围仍是限定域（这一条保留）: {incremental}"
    );
    assert!(
        incremental.contains("unchecked.insert(dbnum)") && incremental.contains("&unchecked,"),
        "被限定域跳过、这一轮没看的库要进留白集合，并随记录构造交下去: {incremental}"
    );
    let refresh = room_recompute
        .split_once("pub(crate) async fn refresh_room_update_record_with_model(")
        .expect("watermark refresh exists")
        .1;
    assert!(
        refresh.contains("hold_unchecked_room_sesnos("),
        "留白在记录构造里落地（排在 source_sessions 回填之后，免得回填替没看的库说话）: {refresh}"
    );
}

/// 整库清边是清掉本轮不会枚举到的旧根之唯一保证；清边失败不能仍从 all_ok=true 起步，
/// 否则列出的库即使逐一算齐，run_rooms_rebuild 仍会把整轮盖成最新。
#[test]
fn room_rebuild_regression_a_failed_global_clear_must_veto_success_stamp() {
    let source = include_str!("handlers.rs");
    let rebuild = source
        .split_once("async fn run_rooms_rebuild(")
        .expect("rebuild body exists")
        .1;
    let clear_result = rebuild
        .find("let clear_succeeded = match clear_room_rebuild_edges_on(")
        .expect("清边结果必须流入整轮成功判据；失败时不能盖最新记录");
    let all_ok = rebuild
        .find("let mut all_ok = clear_succeeded;")
        .expect("逐库全成功也不能盖过全表清边失败");
    let loop_start = rebuild.find("for dbnum in &dbnums {").expect("逐库循环存在");
    let stamp = rebuild
        .find("if all_ok {")
        .expect("成功记录只在全表清边 + 逐库重算都成功时写入");
    assert!(clear_result < all_ok && all_ok < loop_start && loop_start < stamp);
}

// 2026-10-16 audit: intentionally red until the corresponding production fixes.
// Behavior tests and narrowly scoped source contracts; no production DB.
#[test]
fn room_startup_audit_regression_missing_watermark_is_not_up_to_date() {
    use crate::data_interface::room_recompute::{plan_startup_room_check, StartupRoomCheckPlan};
    use crate::fast_model::room_resident::RoomUpdateRecord;
    use std::collections::{BTreeMap, BTreeSet};

    let record = RoomUpdateRecord {
        room_updated_at: "2026-10-16T10:00:00+08:00".into(),
        room_dbnum_sesnos: BTreeMap::from([(7000, 10)]),
        generation: "g1".into(),
    };
    let rooms = BTreeSet::from([7000]);
    let control = crate::data_interface::dbnum_state::DbnumState {
        dbnum: 7000,
        applied_sesno: 10,
        applied_sesno_time: Some("2026-10-16T09:00:00+08:00".into()),
        initialized: true,
        ..Default::default()
    };
    assert!(matches!(
        plan_startup_room_check(&rooms, &[control], Some(&record), "g1"),
        StartupRoomCheckPlan::UpToDate { .. }
    ), "control: an unchanged known watermark can be up to date");
    let plan = plan_startup_room_check(&rooms, &[], Some(&record), "g1");
    assert!(
        !matches!(plan, StartupRoomCheckPlan::UpToDate { .. }),
        "a room-bearing library with no watermark has no alignment proof: {plan:?}"
    );
}

#[test]
fn room_startup_audit_regression_current_generation_is_checked_before_up_to_date() {
    // Behavior test: unchanged timestamps do not certify edges against a new model.
    use crate::data_interface::room_recompute::{plan_startup_room_check, StartupRoomCheckPlan};
    use crate::fast_model::room_resident::RoomUpdateRecord;
    use std::collections::{BTreeMap, BTreeSet};

    let rooms = BTreeSet::from([7000]);
    let states = vec![crate::data_interface::dbnum_state::DbnumState {
        dbnum: 7000,
        applied_sesno: 10,
        applied_sesno_time: Some("2026-10-16T09:00:00+08:00".into()),
        initialized: true,
        ..Default::default()
    }];
    let mut record = RoomUpdateRecord {
        room_updated_at: "2026-10-16T10:00:00+08:00".into(),
        room_dbnum_sesnos: BTreeMap::from([(7000, 10)]),
        generation: "g1".into(),
    };
    assert!(matches!(
        plan_startup_room_check(&rooms, &states, Some(&record), "g1"),
        StartupRoomCheckPlan::UpToDate { .. }
    ), "control: matching generation and unchanged sessions need no update");
    for (saved, current) in [("g1", "g2"), ("", "g1"), ("g1", ""), ("", "")] {
        record.generation = saved.into();
        assert_eq!(
            plan_startup_room_check(&rooms, &states, Some(&record), current),
            StartupRoomCheckPlan::NeedsFullInit,
            "different or unknown generations cannot certify alignment: saved={saved:?}, current={current:?}"
        );
    }
}


#[test]
fn room_startup_audit_regression_missing_watermark_vetoes_an_incremental_plan() {
    use crate::data_interface::room_recompute::{plan_startup_room_check, StartupRoomCheckPlan};
    use crate::fast_model::room_resident::RoomUpdateRecord;
    use std::collections::{BTreeMap, BTreeSet};

    let record = RoomUpdateRecord {
        room_updated_at: "2026-10-16T10:00:00+08:00".into(),
        room_dbnum_sesnos: BTreeMap::from([(7000, 10), (7001, 20)]),
        generation: "g1".into(),
    };
    let states = vec![crate::data_interface::dbnum_state::DbnumState {
        dbnum: 7000,
        applied_sesno: 11,
        applied_sesno_time: Some("2026-10-16T11:00:00+08:00".into()),
        initialized: true,
        ..Default::default()
    }];
    assert!(matches!(
        plan_startup_room_check(&BTreeSet::from([7000]), &states, Some(&record), "g1"),
        StartupRoomCheckPlan::NeedsIncrementalCheck { .. }
    ), "control: a newer session in the same generation keeps window-based incremental checking");
    assert_eq!(
        plan_startup_room_check(&BTreeSet::from([7000, 7001]), &states, Some(&record), "g1"),
        StartupRoomCheckPlan::NeedsFullInit,
        "a second room-bearing library without a watermark must not disappear from an incremental plan"
    );
}

#[test]
fn room_startup_audit_regression_runner_passes_installed_generation_to_planner() {
    // Wiring contract only; generation truth table is tested above by behavior.
    let source = include_str!("../data_interface/room_recompute.rs");
    let runner = source.split_once("pub(crate) async fn check_and_init_rooms_at_startup(")
        .expect("runner exists").1;
    let call = runner.split_once("match plan_startup_room_check(")
        .expect("planner dispatch exists").1
        .split_once(") {").expect("planner call ends").0;
    assert!(call.contains("&outcome.model.input_version()"),
        "the runner must supply the installed model generation, not the saved record generation");
}

#[test]
fn room_startup_audit_regression_foreign_hits_are_handed_off_before_panel_writes() {
    // Wiring contract only, not cross-library geometry end-to-end verification.
    // Callback behavior tests below prove ordering and failure propagation.
    let source = include_str!("../data_interface/room_recompute.rs");
    let body = source
        .split_once("async fn apply_targeted_room_update_for_dbnum(")
        .expect("startup targeted updater exists").1
        .split_once("pub(crate) async fn check_and_init_rooms_at_startup(")
        .expect("updater ends before runner").0;
    let write = body.find("match recompute_dbnum_room_edges(")
        .expect("targeted persistence boundary exists");
    let foreign = body.find("affected.foreign.keys()").expect("foreign library keys are consumed");
    let barrier = body.find("return startup_room_update_after_foreign(")
        .expect("the dependency barrier result must be returned, including failures");
    assert!(foreign < barrier && barrier < write,
        "foreign dependencies must gate local persistence, not be discarded");
    assert!(body.contains("startup_room_fallback_targets(") && body.contains("&outcome.expected_dbnums()"),
        "without reverse lookup, fallback must cover MDB targets rather than only the source library");
}

#[tokio::test]
async fn room_startup_audit_regression_foreign_updates_finish_before_local_panel_writes() {
    use crate::data_interface::room_recompute::startup_room_update_after_foreign;
    let calls = std::cell::RefCell::new(Vec::new());
    let ok = startup_room_update_after_foreign(
        [7011, 7010, 7011],
        |dbnum| {
            calls.borrow_mut().push(dbnum);
            std::future::ready(true)
        },
        || {
            calls.borrow_mut().push(7000);
            std::future::ready(true)
        },
    ).await;
    assert!(ok);
    assert_eq!(*calls.borrow(), vec![7010, 7011, 7000],
        "deduplicated foreign libraries must finish before the local panel writer runs");
}

#[tokio::test]
async fn room_startup_audit_regression_a_failed_foreign_update_blocks_local_panel_writes() {
    use crate::data_interface::room_recompute::startup_room_update_after_foreign;
    let calls = std::cell::RefCell::new(Vec::new());
    let ok = startup_room_update_after_foreign(
        [7011, 7010],
        |dbnum| {
            calls.borrow_mut().push(dbnum);
            std::future::ready(dbnum != 7010)
        },
        || {
            calls.borrow_mut().push(7000);
            std::future::ready(true)
        },
    ).await;
    assert!(!ok, "foreign failure must veto the startup completion record");
    assert_eq!(*calls.borrow(), vec![7010, 7011],
        "other foreign libraries may recover, but the local panel writer must never run");
}

#[tokio::test]
async fn room_startup_audit_regression_local_failure_is_not_hidden_by_foreign_success() {
    use crate::data_interface::room_recompute::startup_room_update_after_foreign;
    let calls = std::cell::RefCell::new(Vec::new());
    for foreign in [vec![], vec![7010]] {
        calls.borrow_mut().clear();
        let ok = startup_room_update_after_foreign(
            foreign.clone(),
            |dbnum| {
                calls.borrow_mut().push(dbnum);
                std::future::ready(true)
            },
            || {
                calls.borrow_mut().push(7000);
                std::future::ready(false)
            },
        ).await;
        assert!(!ok);
        let expected: Vec<_> = foreign.into_iter().chain([7000]).collect();
        assert_eq!(*calls.borrow(), expected);
    }
}

#[test]
fn room_startup_audit_regression_unavailable_reverse_lookup_covers_all_mdb_targets() {
    use crate::data_interface::room_recompute::startup_room_fallback_targets;
    use std::collections::BTreeSet;
    assert_eq!(
        startup_room_fallback_targets(7000, &[7010, 7011, 7010], &BTreeSet::from([7012])),
        BTreeSet::from([7000, 7010, 7011, 7012]),
        "missing reverse lookup cannot fall back to the defining library alone"
    );
    assert_eq!(startup_room_fallback_targets(7000, &[], &BTreeSet::new()), BTreeSet::from([7000]));
}

async fn assert_room_clear_atomic_with_fault(fault_schema: &str) {
    use crate::fast_model::room_resident::{
        read_room_update_record_on, write_room_update_record_on, RoomUpdateRecord,
    };
    use std::collections::BTreeMap;

    let db = surrealdb::engine::any::connect("mem://").await.expect("isolated mem database");
    db.use_ns("room_startup_audit").use_db("partial_clear").await.expect("fixture ns/db");
    db.query(
        "CREATE pe:room; CREATE pe:panel; CREATE pe:member; \
         RELATE pe:panel->room_relate:edge->pe:member; \
         RELATE pe:room->room_panel_relate:edge->pe:panel; \
         CREATE dbnum_watermark:7000 SET applied_sesno=10, model_sesno=10, \
           model_room_generation='g1', model_room_stray_generations=[];"
    ).await.expect("fixture transport").check().expect("fixture statements");
    let record = RoomUpdateRecord {
        room_updated_at: "2026-10-16T10:00:00+08:00".into(),
        room_dbnum_sesnos: BTreeMap::from([(7000, 10)]),
        generation: "g1".into(),
    };
    write_room_update_record_on(&db, &record).await.expect("initial success proof");
    // Schema faults are local to this fresh mem:// connection, never production.
    // The final-record-only case also proves that an earlier watermark update rolls back.
    if !fault_schema.is_empty() {
        db.query(fault_schema).await.expect("fault schema transport")
            .check().expect("fault schema valid");
    }
    assert_eq!(count(&db, "room_relate").await, 1);
    assert_eq!(count(&db, "room_panel_relate").await, 1);
    assert_eq!(alignment(&db).await.as_deref(), Some("g1"));
    assert_eq!(read_room_update_record_on(&db).await.unwrap(), Some(record.clone()));

    let result = super::clear_room_rebuild_edges_on(&db).await;
    if fault_schema.is_empty() {
        result.expect("atomic clear succeeds without injected faults");
        assert_eq!(count(&db, "room_relate").await, 0);
        assert_eq!(count(&db, "room_panel_relate").await, 0);
        assert_eq!(alignment(&db).await, None);
        assert_eq!(read_room_update_record_on(&db).await.unwrap(), None);
        return;
    }
    assert!(result.is_err(), "control: required fields reject NONE invalidation");
    let members = count(&db, "room_relate").await;
    let panels = count(&db, "room_panel_relate").await;
    let generation = alignment(&db).await;
    let persisted = read_room_update_record_on(&db).await.expect("read proof after fault");
    let damaged = members != 1 || panels != 1;
    let old_proof = generation.as_deref() == Some("g1") && persisted == Some(record.clone());
    assert!(
        !(damaged && old_proof),
        "partial clear destroyed edges but kept both old success credentials: members={members}, panels={panels}, generation={generation:?}, record={persisted:?}"
    );
    assert_eq!(members, 1, "failed transaction must retain member edges");
    assert_eq!(panels, 1, "failed transaction must retain topology edges");
    assert_eq!(generation.as_deref(), Some("g1"), "failed transaction must retain watermark");
    assert_eq!(persisted, Some(record), "failed transaction must retain the old record");
}

#[tokio::test]
async fn room_startup_audit_regression_partial_clear_cannot_leave_old_success_proof() {
    assert_room_clear_atomic_with_fault(
        "DEFINE FIELD model_room_generation ON TABLE dbnum_watermark TYPE string; \
         DEFINE FIELD room_update_record ON TABLE room_build TYPE object;"
    ).await;
}

#[tokio::test]
async fn room_startup_audit_regression_final_record_failure_rolls_back_the_clear() {
    assert_room_clear_atomic_with_fault(
        "DEFINE FIELD room_update_record ON TABLE room_build TYPE object;"
    ).await;
}

#[tokio::test]
async fn room_startup_audit_regression_successful_clear_invalidates_both_credentials() {
    assert_room_clear_atomic_with_fault("").await;
}

