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
    let source = include_str!("handlers.rs");
    let body = source
        .split_once("async fn recompute_dbnum_room_edges_now(")
        .expect("recompute boundary exists")
        .1
        .split_once("\npub async fn health(")
        .expect("recompute boundary ends before health")
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
    let source = include_str!("handlers.rs");
    let body = source
        .split_once("async fn recompute_dbnum_room_edges_now(")
        .expect("recompute boundary exists")
        .1
        .split_once("\npub async fn health(")
        .expect("recompute boundary ends before health")
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

    let rebuild = source
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
