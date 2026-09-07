use super::*;
use crate::fast_model::model_memory_store::{ProjectionCommit, RootReceipt};
use e3d_io::refno::RefNo;
use std::io::Write;

fn envelope(n: u32) -> FrozenRootEnvelope {
    FrozenRootEnvelope {
        version: FORMAT_VERSION,
        job_id: format!("{n:064x}"),
        run_id: "aabb".into(),
        dbnum: 7997,
        source_sesno: 112,
        source_token: "frozen-source".into(),
        database_incarnation: "database-1".into(),
        intent: RootIntent {
            root: format!("24381/{n}"),
            revision: 2,
            token: "ccdd".into(),
        },
        target_hash: "eeff".into(),
        created_ms: now_ms(),
        projection: ProjectionCommit {
            receipt: Some((
                RefNo::new(24381, n),
                RootReceipt {
                    source_sesno: 112,
                    model_target: None,
                    manifest_hash: "empty".into(),
                    geometry_count: 0,
                },
            )),
            ..Default::default()
        },
    }
}

fn publishable_envelope(n: u32) -> FrozenRootEnvelope {
    let mut envelope = envelope(n);
    let target = crate::fast_model::e3d_model_service::ModelTarget {
        project: "test".into(),
        design: crate::fast_model::e3d_model_service::ModelFileTarget {
            dbnum: 7997,
            db_type: "DESI".into(),
            file: "frozen".into(),
            session: 112,
            digest: "digest".into(),
        },
        catalogue: vec![],
        template_attlib_digest: "template".into(),
        generator_fingerprint: "generator".into(),
        tessellation_profile: "profile".into(),
    };
    envelope.projection.receipt.as_mut().unwrap().1.model_target = Some(target);
    envelope.target_hash = digest(
        &serde_json::to_vec(&envelope.projection.receipt.as_ref().unwrap().1.model_target).unwrap(),
    );
    envelope
}
fn manifest() -> RunManifest {
    RunManifest {
        version: FORMAT_VERSION,
        run_id: "aabb".into(),
        dbnum: 7997,
        source_sesno: 112,
        source_token: "frozen-source".into(),
        database_incarnation: "database-1".into(),
        model_projection_empty_at_reservation: false,
        dependency_tokens: vec![],
        room_source_keys: vec![],
        room_context_fingerprint: String::new(),
        target_hash: "eeff".into(),
        roots: vec![envelope(1).intent],
        created_ms: now_ms(),
    }
}

#[test]
fn empty_projection_proof_is_additive_and_durable() {
    let legacy = manifest();
    let legacy_json = serde_json::to_string(&legacy).unwrap();
    assert!(!legacy_json.contains("model_projection_empty_at_reservation"));

    let mut clean = legacy;
    clean.model_projection_empty_at_reservation = true;
    let clean_json = serde_json::to_string(&clean).unwrap();
    assert!(clean_json.contains("\"model_projection_empty_at_reservation\":true"));
    assert!(
        serde_json::from_str::<RunManifest>(&clean_json)
            .unwrap()
            .model_projection_empty_at_reservation
    );
}

#[tokio::test]
async fn empty_projection_proof_checks_both_model_tables_and_dbnum() {
    let db = surrealdb::engine::any::connect("mem://").await.unwrap();
    db.use_ns("empty_projection_proof")
        .use_db("test")
        .await
        .unwrap();
    assert!(
        super::runtime::model_projection_is_empty_on(&db, 7997)
            .await
            .unwrap()
    );
    db.query("CREATE inst_relate:other SET dbnum=8000; CREATE tubi_relate:target SET dbnum=7997;")
        .await
        .unwrap()
        .check()
        .unwrap();
    assert!(
        !super::runtime::model_projection_is_empty_on(&db, 7997)
            .await
            .unwrap()
    );
    assert!(
        super::runtime::model_projection_is_empty_on(&db, 7998)
            .await
            .unwrap()
    );
}

/// 初始化的硬不变量：非空投影（含旧清单默认的 `false`）在登记与消费两侧都被拒绝，
/// 错误里点名 dbnum / run 与「先按 ref0 清库」的处置；只有 `Some(true)` 放行。
#[test]
fn non_empty_model_projection_is_refused_at_both_gates() {
    use super::runtime::require_empty_model_projection;

    require_empty_model_projection(Some(true), 7997, None).unwrap();
    require_empty_model_projection(Some(true), 7997, Some("aabb")).unwrap();

    let at_registration = require_empty_model_projection(Some(false), 7997, None)
        .unwrap_err()
        .to_string();
    assert!(at_registration.contains("dbnum=7997"), "{at_registration}");
    assert!(at_registration.contains("fast_delete"), "{at_registration}");
    assert!(!at_registration.contains("run="), "{at_registration}");

    let at_consumption = require_empty_model_projection(Some(false), 7997, Some("aabb"))
        .unwrap_err()
        .to_string();
    assert!(at_consumption.contains("run=aabb"), "{at_consumption}");

    let without_proof = require_empty_model_projection(None, 7997, None)
        .unwrap_err()
        .to_string();
    assert!(
        without_proof.contains("no emptiness proof"),
        "{without_proof}"
    );

    // 旧清单反序列化出来的默认值就是 `false`：它必须落在拒绝这一侧。
    let legacy = manifest();
    assert!(
        require_empty_model_projection(
            Some(legacy.model_projection_empty_at_reservation),
            legacy.dbnum,
            Some(&legacy.run_id),
        )
        .is_err()
    );
}

#[test]
fn handed_off_result_survives_restart_and_cache_eviction() {
    let tmp = tempfile::tempdir().unwrap();
    let first = envelope(1);
    {
        let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024).unwrap();
        q.register(manifest()).unwrap();
        q.mark_reserved("aabb").unwrap();
        q.activate("aabb").unwrap();
        q.handoff(vec![first.clone(), envelope(2)]).unwrap();
        assert_eq!(q.health().memory_bytes, 0);
        assert!(
            Journal::open(tmp.path().into(), 1, 1024 * 1024).is_err(),
            "exclusive directory ownership"
        );
    }
    let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024).unwrap();
    assert!(q.active("aabb"));
    assert_eq!(q.next_ready(), Some(first.job_id.clone()));
    assert_eq!(
        q.load(&first.job_id).unwrap().projection.receipt,
        first.projection.receipt
    );
    assert_eq!(q.health().handed_off, 2);
}

#[test]
fn inflight_is_held_on_restart_and_never_automatically_replayed() {
    let tmp = tempfile::tempdir().unwrap();
    let e = envelope(1);
    {
        let mut q = Journal::open(tmp.path().into(), 10000, 1024 * 1024).unwrap();
        q.handoff(vec![e.clone()]).unwrap();
        q.transition(&e.job_id, TaskState::Inflight, None).unwrap();
        q.checkpoint().unwrap();
    }
    let mut q = Journal::open(tmp.path().into(), 10000, 1024 * 1024).unwrap();
    assert_eq!(q.state(&e.job_id), Some(TaskState::UnknownHold));
    assert_eq!(q.next_ready(), None);
    q.transition(&e.job_id, TaskState::Ready, None).unwrap();
    assert_eq!(q.next_ready(), Some(e.job_id));
}

#[test]
fn failure_does_not_block_independent_roots_or_erase_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let a = envelope(1);
    let b = envelope(2);
    let mut q = Journal::open(tmp.path().into(), 10000, 1024 * 1024).unwrap();
    q.handoff(vec![a.clone(), b.clone()]).unwrap();
    assert_eq!(q.next_ready(), Some(a.job_id.clone()));
    q.transition(&a.job_id, TaskState::FailedHold, Some("bad root".into()))
        .unwrap();
    assert_eq!(q.next_ready(), Some(b.job_id));
    assert!(q.load(&a.job_id).is_ok());
    assert_eq!(q.health().failed_hold, 1);
}

#[test]
fn capacity_rejects_whole_group_without_partial_admission() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 100, 400).unwrap();
    assert!(q.handoff(vec![envelope(1), envelope(2)]).is_err());
    assert_eq!(q.health().handed_off, 0);
    assert_eq!(q.health().disk_bytes, 0);
}

#[test]
fn generation_handoff_precedes_attribute_commit_but_consumption_waits() {
    let tmp = tempfile::tempdir().unwrap();
    let e = envelope(1);
    {
        let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
        q.register(manifest()).unwrap();
        q.handoff(vec![e.clone()]).unwrap();
        assert_eq!(
            q.health().handed_off,
            1,
            "generation runs while attributes are uncommitted"
        );
        assert_eq!(
            q.next_ready(),
            None,
            "database publication must wait for the data baseline"
        );
    }
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    assert_eq!(
        q.next_ready(),
        None,
        "restart must not bypass the attribute barrier"
    );
    q.mark_reserved("aabb").unwrap();
    q.activate("aabb").unwrap();
    assert_eq!(q.next_ready(), Some(e.job_id));
}

#[test]
fn tail_without_ack_can_be_truncated_but_corruption_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let e = envelope(1);
    {
        let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
        q.handoff(vec![e.clone()]).unwrap();
    }
    let path = tmp.path().join("00000000000000000000.journal");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0; 8])
        .unwrap();
    {
        let q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
        assert!(q.load(&e.job_id).is_ok());
    }
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[41] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(Journal::open(tmp.path().into(), 0, 1024 * 1024).is_err());
    assert!(path.exists());
}

#[test]
fn hundred_thousand_results_use_indexed_admission_and_bounded_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 64 * 1024, 256 * 1024 * 1024).unwrap();
    for chunk in (0..100_000).collect::<Vec<_>>().chunks(500) {
        q.handoff(chunk.iter().map(|n| envelope(*n)).collect())
            .unwrap();
    }
    assert_eq!(q.health().handed_off, 100_000);
    assert!(q.health().memory_bytes <= 64 * 1024);
    assert_eq!(
        q.load(&envelope(77_777).job_id).unwrap().intent.root,
        "24381/77777"
    );
    q.checkpoint().unwrap();
    drop(q);
    let q = Journal::open(tmp.path().into(), 64 * 1024, 256 * 1024 * 1024).unwrap();
    assert_eq!(q.health().handed_off, 100_000);
}

async fn db() -> surrealdb::Surreal<surrealdb::engine::any::Any> {
    let db = surrealdb::engine::any::connect("mem://").await.unwrap();
    db.use_ns("queue").use_db("test").await.unwrap();
    db
}
async fn reserve_test(db: &surrealdb::Surreal<surrealdb::engine::any::Any>) {
    db.query("CREATE model_update_pending:regen_root_24381_1 SET dbnum=7997,revision=2,init_run_id='aabb',init_token='ccdd',init_target_hash='eeff',init_source_hash='432c80b0bd6e7684105a1139ae713087a3787feefebba92ac6e54b47549b1a32',source_end_sesno=112; CREATE gen_root:24381_1 SET desired_revision=2; CREATE dbnum_watermark:7997 SET applied_sesno=112,applied_source_hash='432c80b0bd6e7684105a1139ae713087a3787feefebba92ac6e54b47549b1a32'; CREATE model_publish_database:current SET incarnation='database-1'; CREATE probe:counter SET value=0;")
        .await.unwrap().check().unwrap();
}

async fn reserve_second_test_root(db: &surrealdb::Surreal<surrealdb::engine::any::Any>) {
    db.query("CREATE model_update_pending:regen_root_24381_2 SET dbnum=7997,revision=2,init_run_id='aabb',init_token='ccdd',init_target_hash='eeff',init_source_hash='432c80b0bd6e7684105a1139ae713087a3787feefebba92ac6e54b47549b1a32',source_end_sesno=112; CREATE gen_root:24381_2 SET desired_revision=2;")
        .await
        .unwrap()
        .check()
        .unwrap();
}

#[tokio::test]
async fn one_request_can_commit_multiple_independent_root_transactions() {
    use crate::fast_model::model_db_adapter::{PersistMedium, PersistTarget};

    let db = db().await;
    reserve_test(&db).await;
    reserve_second_test_root(&db).await;
    let first = publishable_envelope(1);
    let second = publishable_envelope(2);
    db.query(format!(
        "UPDATE model_update_pending:regen_root_24381_1 SET init_target_hash='{}'; UPDATE model_update_pending:regen_root_24381_2 SET init_target_hash='{}';",
        first.target_hash, second.target_hash
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
    let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
    let first_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &first, true,
    )
    .await
    .unwrap()
    .0;
    let second_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &second, true,
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first_plan.chunks.len(), 1);
    assert_eq!(second_plan.chunks.len(), 1);
    let outcomes = crate::fast_model::model_db_adapter::persist_prepared_frozen_batch_on(
        target,
        &[(&first, &first_plan), (&second, &second_plan)],
    )
    .await
    .unwrap();
    assert_eq!(outcomes.len(), 2);
    let mut response = db
        .query("SELECT count() AS count FROM model_publish_ack GROUP ALL; SELECT count() AS count FROM model_update_pending GROUP ALL;")
        .await
        .unwrap()
        .check()
        .unwrap();
    let acknowledgements: Vec<serde_json::Value> = response.take(0).unwrap();
    let pending: Vec<serde_json::Value> = response.take(1).unwrap();
    assert_eq!(acknowledgements[0]["count"], 2);
    assert_eq!(
        pending.first().and_then(|row| row["count"].as_u64()),
        Some(0)
    );
}

#[tokio::test]
async fn failed_root_transaction_does_not_rollback_its_batched_neighbour() {
    use crate::fast_model::model_db_adapter::{PersistMedium, PersistTarget};

    let db = db().await;
    reserve_test(&db).await;
    reserve_second_test_root(&db).await;
    let first = publishable_envelope(1);
    let second = publishable_envelope(2);
    db.query(format!(
        "UPDATE model_update_pending:regen_root_24381_1 SET init_target_hash='{}'; UPDATE model_update_pending:regen_root_24381_2 SET init_target_hash='{}';",
        first.target_hash, second.target_hash
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
    let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
    let first_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &first, true,
    )
    .await
    .unwrap()
    .0;
    let second_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &second, true,
    )
    .await
    .unwrap()
    .0;
    db.query("UPDATE gen_root:24381_2 SET desired_revision=3;")
        .await
        .unwrap()
        .check()
        .unwrap();
    let error = crate::fast_model::model_db_adapter::persist_prepared_frozen_batch_on(
        target,
        &[(&first, &first_plan), (&second, &second_plan)],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("model_publish_superseded"));
    let acknowledgements: Vec<String> = db
        .query("SELECT VALUE job_id FROM model_publish_ack ORDER BY job_id;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(acknowledgements, vec![first.job_id]);
    let pending: Vec<String> = db
        .query("SELECT VALUE <string>id FROM model_update_pending;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].contains("24381_2"));
}

/// H2（2026-09-07 审核）：superseded 的根排在**前面**，后面的根因自己的原因失败。
/// 批量错误必须逐根归因——第二根的错误里不能出现邻居的 `model_publish_superseded`，
/// 否则它会被错误地判成不可重放的 Superseded。顺带钉住：引擎在一段事务失败之后
/// 继续执行同一请求里后续的独立事务（第二根有自己的错误，而不是什么都没有）。
#[tokio::test]
async fn batch_errors_are_attributed_per_root_when_superseded_comes_first() {
    use crate::fast_model::model_db_adapter::{
        BatchPublicationError, PersistMedium, PersistTarget,
    };

    let db = db().await;
    reserve_test(&db).await;
    reserve_second_test_root(&db).await;
    let first = publishable_envelope(1);
    let mut second = publishable_envelope(2);
    // 第二根自己的、与 superseded 无关的失败：数据库 incarnation 对不上。
    second.database_incarnation = "database-2".into();
    db.query(format!(
        "UPDATE model_update_pending:regen_root_24381_1 SET init_target_hash='{}'; UPDATE model_update_pending:regen_root_24381_2 SET init_target_hash='{}';",
        first.target_hash, second.target_hash
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
    let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
    let first_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &first, true,
    )
    .await
    .unwrap()
    .0;
    let second_plan = crate::fast_model::model_db_adapter::prepare_frozen_profiled_for_baseline_on(
        target, &second, true,
    )
    .await
    .unwrap()
    .0;
    // 第一根被 supersede。
    db.query("UPDATE gen_root:24381_1 SET desired_revision=3;")
        .await
        .unwrap()
        .check()
        .unwrap();
    let error = crate::fast_model::model_db_adapter::persist_prepared_frozen_batch_on(
        target,
        &[(&first, &first_plan), (&second, &second_plan)],
    )
    .await
    .unwrap_err();
    let batch_error = format!("{error:#}");
    let attributed = error
        .downcast_ref::<BatchPublicationError>()
        .expect("batched publication failures carry per-root attribution");
    assert!(attributed.attributed(), "{attributed:?}");
    assert_eq!(attributed.roots, 2);
    let first_errors = attributed
        .root_errors(0)
        .expect("first root failed")
        .join("; ");
    let second_errors = attributed
        .root_errors(1)
        .expect("the engine keeps executing the neighbour's independent transaction")
        .join("; ");
    assert!(
        first_errors.contains("model_publish_superseded"),
        "{first_errors}"
    );
    assert!(
        second_errors.contains("model_publish_database_mismatch"),
        "{second_errors}"
    );
    assert!(
        !second_errors.contains("model_publish_superseded"),
        "neighbour's superseded leaked into the second root: {second_errors}"
    );
    // 整条文本仍然带着两根的错误，连接类判定与日志照旧看得见全貌。
    assert!(batch_error.contains("model_publish_superseded"));
    assert!(batch_error.contains("model_publish_database_mismatch"));

    let acknowledgements: Vec<String> = db
        .query("SELECT VALUE job_id FROM model_publish_ack;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert!(acknowledgements.is_empty(), "{acknowledgements:?}");

    // 消费者据此给的终态：第一根 Superseded，第二根 FailedHold（可重放），而不是两根都 Superseded。
    let (state, message) =
        super::runtime::classify_unacked_batch_root(Some(&first_errors), false, &batch_error);
    assert_eq!(state, TaskState::Superseded);
    assert!(message.contains("model_publish_superseded"));
    let (state, message) =
        super::runtime::classify_unacked_batch_root(Some(&second_errors), false, &batch_error);
    assert_eq!(state, TaskState::FailedHold);
    assert!(!message.contains("model_publish_superseded"), "{message}");
}

/// 归因规则本身：没有归因到本根的错误时绝不判 Superseded；连接类一律 UnknownHold。
#[test]
fn unacked_batch_root_is_classified_only_by_its_own_errors() {
    use super::runtime::classify_unacked_batch_root;

    let batch = "publication statements failed: root 0: statement 7: model_publish_superseded";
    let (state, message) = classify_unacked_batch_root(None, false, batch);
    assert_eq!(state, TaskState::FailedHold);
    assert!(message.contains("not attributed to this root"), "{message}");
    assert!(message.contains(batch), "{message}");

    let (state, _) = classify_unacked_batch_root(None, true, "connection reset by peer");
    assert_eq!(state, TaskState::UnknownHold);

    let (state, message) =
        classify_unacked_batch_root(Some("statement 3: write conflict"), false, batch);
    assert_eq!(state, TaskState::FailedHold);
    assert_eq!(message, "statement 3: write conflict");

    let (state, _) = classify_unacked_batch_root(
        Some("statement 7: model_publish_superseded"),
        true,
        "connection reset by peer",
    );
    assert_eq!(
        state,
        TaskState::Superseded,
        "the database said so in this root's own transaction"
    );
}

#[tokio::test]
async fn chunk_ack_prevents_duplicate_effects_and_final_chunk_settles_exact_revision() {
    let db = db().await;
    reserve_test(&db).await;
    let e = envelope(1);
    let sql = crate::fast_model::model_db_adapter::render_frozen_chunk(
        &e,
        0,
        true,
        "UPDATE probe:counter SET value += 1;",
    );
    db.query(sql.clone()).await.unwrap().check().unwrap();
    db.query(sql).await.unwrap().check().unwrap();
    let counts: Vec<u64> = db
        .query("SELECT VALUE value FROM probe:counter;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(counts, vec![1]);
    let pending: Vec<serde_json::Value> = db
        .query("SELECT * FROM model_update_pending;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn wrong_revision_token_or_target_cannot_write_even_at_same_session() {
    for variant in 0..3 {
        let db = db().await;
        reserve_test(&db).await;
        let mut e = envelope(1);
        match variant {
            0 => e.intent.revision = 1,
            1 => e.intent.token = "different".into(),
            _ => {
                db.query("UPDATE gen_root:24381_1 SET init_target_hash='new-target';")
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
        }
        let sql = crate::fast_model::model_db_adapter::render_frozen_chunk(
            &e,
            0,
            true,
            "UPDATE probe:counter SET value += 1;",
        );
        assert!(db.query(sql).await.unwrap().check().is_err());
        let v: Vec<u64> = db
            .query("SELECT VALUE value FROM probe:counter;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(v, vec![0]);
    }
}

#[tokio::test]
async fn dependency_receipt_range_deletes_only_the_exact_root() {
    let db = db().await;
    db.query("CREATE root_dependency:⟨24381_1_7000⟩ SET root_refno='24381/1'; CREATE root_dependency:⟨24381_10_7000⟩ SET root_refno='24381/10'; CREATE gen_root:24381_1 SET desired_revision=2;")
        .await.unwrap().check().unwrap();
    let mut receipt = envelope(1).projection.receipt.unwrap().1;
    receipt.model_target = Some(crate::fast_model::e3d_model_service::ModelTarget {
        project: "test".into(),
        design: crate::fast_model::e3d_model_service::ModelFileTarget {
            dbnum: 7997,
            db_type: "DESI".into(),
            file: "frozen".into(),
            session: 112,
            digest: "digest".into(),
        },
        catalogue: vec![],
        template_attlib_digest: "template".into(),
        generator_fingerprint: "generator".into(),
        tessellation_profile: "profile".into(),
    });
    db.query(
        crate::fast_model::model_db_adapter::render_receipt(RefNo::new(24381, 1), &receipt)
            .unwrap(),
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    let roots: Vec<String> = db
        .query("SELECT VALUE root_refno FROM root_dependency;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(roots, vec!["24381/10"]);
}

#[test]
fn prepared_plan_and_consumer_pause_survive_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let e = envelope(1);
    let plan = PreparedRootPublication {
        version: FORMAT_VERSION,
        memory_removals: vec![],
        memory_tube_branch_resets: vec![],
        envelope_hash: digest(&serde_json::to_vec(&e).unwrap()),
        chunks: vec!["DELETE probe:old;".into(), "CREATE probe:new;".into()],
        upserted: 1,
        removed: 1,
        receipts: 1,
    };
    {
        let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
        q.register(manifest()).unwrap();
        q.mark_reserved("aabb").unwrap();
        q.activate("aabb").unwrap();
        q.handoff(vec![e.clone()]).unwrap();
        q.prepare(&e.job_id, plan.clone()).unwrap();
        assert!(q.prepare(&e.job_id, plan.clone()).is_err());
        q.transition(&e.job_id, TaskState::Inflight, None).unwrap();
        q.set_pause(Some("database connection lost".into()))
            .unwrap();
        q.checkpoint().unwrap();
    }
    let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    assert_eq!(
        q.load_prepared(&e.job_id).unwrap().unwrap().hash().unwrap(),
        plan.hash().unwrap()
    );
    assert!(q.pause_reason().is_some());
    assert!(q.finish_candidates().is_empty());
    q.transition(&e.job_id, TaskState::Committed, None).unwrap();
    q.transition(&e.job_id, TaskState::Committed, None).unwrap();
    assert_eq!(q.finish_candidates().len(), 1);
    q.checkpoint().unwrap();
    drop(q);
    let q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    assert_eq!(q.finish_candidates().len(), 1);
}

#[tokio::test]
async fn applied_watermark_and_exact_chunk_content_fence_replay() {
    let db = db().await;
    reserve_test(&db).await;
    let e = envelope(1);
    let sql = crate::fast_model::model_db_adapter::render_frozen_chunk(
        &e,
        0,
        true,
        "UPDATE probe:counter SET value += 1;",
    );
    db.query("UPDATE dbnum_watermark:7997 SET applied_sesno=111;")
        .await
        .unwrap()
        .check()
        .unwrap();
    assert!(db.query(sql.clone()).await.unwrap().check().is_err());
    db.query("UPDATE dbnum_watermark:7997 SET applied_sesno=112;")
        .await
        .unwrap()
        .check()
        .unwrap();
    db.query(sql).await.unwrap().check().unwrap();
    let changed = crate::fast_model::model_db_adapter::render_frozen_chunk(
        &e,
        0,
        true,
        "UPDATE probe:counter SET value += 100;",
    );
    assert!(db.query(changed).await.unwrap().check().is_err());
    let mut r = db
        .query("SELECT VALUE value FROM probe:counter;")
        .await
        .unwrap();
    assert_eq!(r.take::<Vec<i64>>(0).unwrap(), vec![1]);
}

#[tokio::test]
async fn spatial_fragments_have_transactional_order_and_indexed_range() {
    let db = db().await;
    let a = envelope(1);
    let b = envelope(2);
    for (e, refresh, removed) in [
        (&a, vec!["x".into()], vec![]),
        (&b, vec!["y".into()], vec!["x".into()]),
    ] {
        let s = render_spatial_fragment(e, &refresh, &removed).unwrap();
        db.query(format!("BEGIN; {s} COMMIT;"))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    let mut r=db.query("SELECT VALUE sequence FROM model_publish_fragment:['aabb',1]..['aabb',9223372036854775807] LIMIT 1;").await.unwrap().check().unwrap();
    assert_eq!(r.take::<Vec<u64>>(0).unwrap(), vec![1]);
    let mut r=db.query("SELECT VALUE sequence FROM model_publish_fragment:['aabb',2]..['aabb',9223372036854775807] LIMIT 1;").await.unwrap().check().unwrap();
    assert_eq!(r.take::<Vec<u64>>(0).unwrap(), vec![2]);
}

#[test]
fn frozen_receipt_must_match_its_declared_root_and_session() {
    let mut e = envelope(1);
    e.projection.receipt.as_mut().unwrap().1.model_target=Some(serde_json::from_value(serde_json::json!({
        "project":"AMS", "design":{"dbnum":7997,"db_type":"DESI","file":"frozen","session":112,"digest":"identity"},
        "catalogue":[],"template_attlib_digest":"template","generator_fingerprint":"generator","tessellation_profile":"profile"
    })).unwrap());
    e.target_hash = digest(
        &serde_json::to_vec(&e.projection.receipt.as_ref().unwrap().1.model_target).unwrap(),
    );
    e.validate().unwrap();
    let mut wrong = e.clone();
    wrong.projection.receipt.as_mut().unwrap().0 = RefNo::new(24381, 2);
    assert!(wrong.validate().is_err());
    let mut wrong = e.clone();
    wrong.projection.receipt.as_mut().unwrap().1.source_sesno = 113;
    assert!(wrong.validate().is_err());
    let mut wrong = e.clone();
    wrong
        .projection
        .tube_branch_resets
        .push(RefNo::new(24381, 2));
    assert!(wrong.validate().is_err());
}

#[test]
fn generation_failure_is_not_a_handoff_and_requires_manual_regeneration() {
    let tmp = tempfile::tempdir().unwrap();
    let mut e = envelope(1);
    e.projection = Default::default();
    {
        let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
        q.generation_failure(e.clone(), "failed computation".into())
            .unwrap();
        assert_eq!(q.health().handed_off, 0);
        assert_eq!(q.health().failed_hold, 1);
        assert!(q.next_ready().is_none());
        q.checkpoint().unwrap();
    }
    let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    assert_eq!(q.state(&e.job_id), Some(TaskState::GenerationHold));
    q.retry_generation(&e.job_id).unwrap();
    assert!(q.state(&e.job_id).is_none());
    q.handoff(vec![envelope(1)]).unwrap();
    assert_eq!(q.health().handed_off, 1);
    assert_eq!(q.health().failed_hold, 0);
}

#[test]
fn checkpoint_does_not_hide_payload_corruption() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
        q.handoff(vec![envelope(1)]).unwrap();
        q.checkpoint().unwrap();
    }
    let p = tmp.path().join("00000000000000000000.journal");
    let mut b = std::fs::read(&p).unwrap();
    b[45] ^= 1;
    std::fs::write(&p, b).unwrap();
    assert!(Journal::open(tmp.path().into(), 0, 1024 * 1024).is_err());
}

/// 在飞 run 的口径：已开始生成、未收口、且还有根没停在 hold 上。整 run 全 hold 的不占槽位。
#[test]
fn in_flight_runs_count_started_unfinished_runs_that_still_have_live_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    let mut a = manifest();
    a.run_id = "run-a".into();
    a.roots = vec![envelope(1).intent, envelope(2).intent];
    let mut b = manifest();
    b.run_id = "run-b".into();
    b.dbnum = 7999;
    b.roots = vec![envelope(3).intent];
    q.register(a).unwrap();
    q.register(b).unwrap();
    let started: std::collections::BTreeSet<String> = ["run-a".to_string()].into_iter().collect();
    // 只开始了 a：一个在飞；b 还没开始，不算。
    assert_eq!(q.in_flight_runs(&started), 1);
    assert_eq!(q.in_flight_runs(&Default::default()), 0);
    // a 的两个根一个交接、一个生成失败：仍有活根，仍在飞。
    let mut e1 = envelope(1);
    e1.run_id = "run-a".into();
    q.handoff(vec![e1]).unwrap();
    let mut e2 = envelope(2);
    e2.run_id = "run-a".into();
    e2.projection = Default::default();
    q.generation_failure(e2, "broken".into()).unwrap();
    assert_eq!(q.in_flight_runs(&started), 1);
    // 全部根都停在 hold 上（等人工 replay）：不再占生成槽位。
    let mut e1 = envelope(1);
    e1.run_id = "run-a".into();
    e1.projection = Default::default();
    q.transition(
        &e1.job_id,
        TaskState::FailedHold,
        Some("conflict exhausted".into()),
    )
    .unwrap();
    assert_eq!(q.in_flight_runs(&started), 0);
}

#[test]
fn manifest_cannot_activate_before_durable_reservation() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    q.register(manifest()).unwrap();
    assert!(q.activate("aabb").is_err());
    assert!(q.pending_manifests(&Default::default()).is_empty());
    q.mark_reserved("aabb").unwrap();
    drop(q);
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    assert!(q.reserved("aabb"));
    q.activate("aabb").unwrap();
}

#[tokio::test]
async fn reservation_recovery_never_overwrites_a_new_target_or_repairs_implicitly() {
    let db = db().await;
    reserve_test(&db).await;
    let m = manifest();
    assert!(runtime::reserve_on(&db, &m, true).await.is_err());
    runtime::reserve_on(&db, &m, false).await.unwrap();
    runtime::reserve_on(&db, &m, true).await.unwrap();
    db.query("UPDATE gen_root:24381_1 SET init_target_hash='new-target'; UPDATE model_update_pending:regen_root_24381_1 SET init_run_id=NONE,init_token=NONE,init_source_hash=NONE,init_target_hash='new-target';").await.unwrap().check().unwrap();
    assert!(runtime::reserve_on(&db, &m, false).await.is_err());
    let hash: Vec<String> = db
        .query("SELECT VALUE init_target_hash FROM gen_root:24381_1;")
        .await
        .unwrap()
        .check()
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(hash, vec!["new-target"]);
}

#[tokio::test]
async fn physical_database_and_exact_source_fence_every_chunk() {
    for field in ["database", "source"] {
        let db = db().await;
        reserve_test(&db).await;
        db.query(if field == "database" {
            "UPDATE model_publish_database:current SET incarnation='recreated';"
        } else {
            "UPDATE dbnum_watermark:7997 SET applied_source_hash='same-session-different-source';"
        })
        .await
        .unwrap()
        .check()
        .unwrap();
        let sql = crate::fast_model::model_db_adapter::render_frozen_chunk(
            &envelope(1),
            0,
            true,
            "UPDATE probe:counter SET value += 1;",
        );
        assert!(db.query(sql).await.unwrap().check().is_err());
        let v: Vec<u64> = db
            .query("SELECT VALUE value FROM probe:counter;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(v, vec![0]);
    }
}

#[tokio::test]
async fn partial_reservation_retains_the_unowned_tail_and_allows_room_work() {
    let db = db().await;
    let mut m = manifest();
    m.roots = (1..=501).map(|n| envelope(n).intent).collect();
    let mut seed =
        String::from("CREATE model_publish_database:current SET incarnation='database-1';");
    for root in &m.roots {
        let key = root.root.replace('/', "_");
        seed.push_str(&format!("CREATE gen_root:{key} SET dbnum=7997,desired_revision=2; CREATE model_update_pending:regen_root_{key} SET dbnum=7997,revision=2,source_end_sesno=112,action='regen_root',status='pending',attempts=0;"));
    }
    seed.push_str("CREATE model_update_pending:other SET dbnum=8000,action='regen_root',status='pending'; CREATE model_update_pending:room SET dbnum=7997,action='room_recalc_panel',status='pending'; DEFINE EVENT reject_last ON TABLE model_update_pending WHEN $after.id = model_update_pending:regen_root_24381_501 AND $after.init_run_id = 'aabb' THEN { THROW 'injected reservation boundary'; };");
    db.query(seed).await.unwrap().check().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    q.register(m.clone()).unwrap();
    assert!(runtime::reserve_on(&db, &m, false).await.is_err());
    assert!(runtime::reserve_on(&db, &m, true).await.is_err());
    assert!(!q.reserved(&m.run_id));
    assert_eq!(q.unfinished_dbnums(), vec![7997]);
    let removed = crate::data_interface::model_update_pending::clear_stale_with_protected_on(
        &db,
        &q.unfinished_dbnums(),
    )
    .await
    .unwrap();
    assert_eq!(removed, 1, "only unrelated stale data work is discarded");
    let exclusion = crate::data_interface::model_update_pending::render_initialization_exclusion(
        &q.unfinished_dbnums(),
    );
    let mut r = db.query(format!("SELECT VALUE id FROM model_update_pending WHERE action='regen_root' AND init_run_id IS NONE {exclusion}; SELECT VALUE id FROM model_update_pending WHERE action='room_recalc_panel' {exclusion}; SELECT VALUE id FROM model_update_pending WHERE action='regen_root';")).await.unwrap().check().unwrap();
    assert!(r.take::<Vec<surrealdb::RecordId>>(0).unwrap().is_empty());
    assert_eq!(r.take::<Vec<surrealdb::RecordId>>(1).unwrap().len(), 1);
    assert_eq!(r.take::<Vec<surrealdb::RecordId>>(2).unwrap().len(), 501);
    db.query("REMOVE EVENT reject_last ON TABLE model_update_pending;")
        .await
        .unwrap()
        .check()
        .unwrap();
    runtime::reserve_on(&db, &m, false).await.unwrap();
    runtime::reserve_on(&db, &m, true).await.unwrap();
    q.mark_reserved(&m.run_id).unwrap();
    q.activate(&m.run_id).unwrap();
}

fn mesh_envelope(n: u32) -> FrozenRootEnvelope {
    use crate::fast_model::model_memory_store::{ProjectedGeometry, ProjectedShape};
    let mut e = envelope(n);
    let root = RefNo::new(24381, n);
    e.projection.upserts.push(ProjectedGeometry {
        record_id: format!("24381_{n}"),
        geometry_id: e3d_model::elmodl::GeometryId::Element {
            refno: root.to_string(),
        },
        source_refno: root,
        dbnum: 7997,
        source_sesno: 112,
        noun: "EQUI".into(),
        mesh_id: "mesh_shared".into(),
        world_transform: glam::DMat4::IDENTITY,
        world_bounds: [[0.; 3], [1.; 3]],
        ancestors: vec![],
        shape: ProjectedShape::Baked,
        tube: None,
    });
    e
}

#[test]
fn mesh_budget_counts_unique_live_dependencies_and_recovers_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let meshes = tempfile::tempdir().unwrap();
    std::fs::write(meshes.path().join("mesh_shared.mesh"), vec![1; 4096]).unwrap();
    let a = mesh_envelope(1);
    let b = mesh_envelope(2);
    let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024)
        .unwrap()
        .with_mesh_root(meshes.path().into())
        .unwrap();
    q.handoff(vec![a.clone(), b.clone()]).unwrap();
    assert_eq!(q.health().queued_mesh_bytes, 4096);
    q.transition(&a.job_id, TaskState::Committed, None).unwrap();
    q.transition(&b.job_id, TaskState::FailedHold, Some("retain mesh".into()))
        .unwrap();
    assert_eq!(q.health().queued_mesh_bytes, 4096);
    q.checkpoint().unwrap();
    drop(q);
    let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024)
        .unwrap()
        .with_mesh_root(meshes.path().into())
        .unwrap();
    assert_eq!(q.health().queued_mesh_bytes, 4096);
    q.verify_meshes(&b).unwrap();
    q.transition(&b.job_id, TaskState::Committed, None).unwrap();
    assert_eq!(q.health().queued_mesh_bytes, 0);
    assert!(meshes.path().join("mesh_shared.mesh").exists());
}

/// 让数据库写带宽是**按库**让的：别的库的属性批次不该停掉本库的发布，否则 28 个库串
/// 起来就是 Σ属性 + Σ发布、一次都不重叠。同库仍然让路——那条不变量（`blocks_data_batch`
/// 不让活跃 run 的库进 Running）在上游，这道门只是钉住它。
#[test]
fn only_the_same_dbnums_data_batch_yields_publication_bandwidth() {
    use super::runtime::data_batch_blocks_publication;

    assert!(
        !data_batch_blocks_publication(&[8000], &[7997]),
        "8000 在解析属性，不该停掉 7997 的发布"
    );
    assert!(
        !data_batch_blocks_publication(&[8000, 8009], &[7997, 1112]),
        "多库并行时同样只看有没有交集"
    );
    assert!(data_batch_blocks_publication(&[7997], &[7997]));
    assert!(data_batch_blocks_publication(&[8000, 7997], &[7997]));
    assert!(!data_batch_blocks_publication(&[], &[7997]));
    assert!(!data_batch_blocks_publication(&[7997], &[]));
}

/// 活跃 run 的库才算「正在发布」：登记了但还没 activate、或者已经收口的，都不参与
/// 抢带宽——`next_ready` 本来就不交它们的根。
#[test]
fn active_dbnums_only_counts_runs_that_can_hand_out_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
    let mut m = manifest();
    q.register(m.clone()).unwrap();
    assert!(q.active_dbnums().is_empty(), "只登记还不算在发布");

    q.mark_reserved(&m.run_id).unwrap();
    q.activate(&m.run_id).unwrap();
    assert_eq!(q.active_dbnums(), vec![7997]);

    m.run_id = "second".into();
    m.dbnum = 8000;
    q.register(m.clone()).unwrap();
    q.mark_reserved(&m.run_id).unwrap();
    q.activate(&m.run_id).unwrap();
    let mut dbnums = q.active_dbnums();
    dbnums.sort_unstable();
    assert_eq!(dbnums, vec![7997, 8000]);

    q.finish("second").unwrap();
    assert_eq!(q.active_dbnums(), vec![7997], "收口了就不再抢带宽");
}

/// 长度没变、内容变了的 mesh 必须拦下来。mesh 文件是内容寻址的不可变件，它变了就说明
/// 要发布的那份几何已经不是入册时校验过的那一份——而只比长度的旧口径看不见这一类。
#[test]
fn a_same_length_mesh_rewrite_is_caught_by_the_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let meshes = tempfile::tempdir().unwrap();
    let path = meshes.path().join("mesh_shared.mesh");
    std::fs::write(&path, vec![1; 4096]).unwrap();
    let e = mesh_envelope(1);
    let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024)
        .unwrap()
        .with_mesh_root(meshes.path().into())
        .unwrap();
    q.handoff(vec![e.clone()]).unwrap();
    q.verify_meshes(&e).unwrap();

    std::fs::write(&path, vec![2; 4096]).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        4096,
        "这一条要验的正是「长度骗得过去」的那种改动"
    );
    let error = format!("{:#}", q.verify_meshes(&e).unwrap_err());
    assert!(error.contains("immutable mesh changed"), "{error}");
}

#[test]
fn mesh_capacity_and_missing_dependencies_fail_before_publication() {
    let tmp = tempfile::tempdir().unwrap();
    let meshes = tempfile::tempdir().unwrap();
    let path = meshes.path().join("mesh_shared.mesh");
    std::fs::write(&path, vec![1; 4096]).unwrap();
    let e = mesh_envelope(1);
    let mut q = Journal::open(tmp.path().into(), 1, 4096)
        .unwrap()
        .with_mesh_root(meshes.path().into())
        .unwrap();
    assert!(q.handoff(vec![e.clone()]).is_err());
    assert_eq!(q.health().handed_off, 0);
    drop(q);
    let mut q = Journal::open(tmp.path().into(), 1, 1024 * 1024)
        .unwrap()
        .with_mesh_root(meshes.path().into())
        .unwrap();
    q.handoff(vec![e.clone()]).unwrap();
    q.checkpoint().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(q.verify_meshes(&e).is_err());
    drop(q);
    assert!(
        Journal::open(tmp.path().into(), 1, 1024 * 1024)
            .unwrap()
            .with_mesh_root(meshes.path().into())
            .is_err()
    );
    assert!(tmp.path().join("checkpoint.json").exists());
}

#[test]
fn journal_recovery_preserves_transform_bits_and_payload_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let mut e = mesh_envelope(1);
    let mut matrix = glam::DMat4::IDENTITY.to_cols_array();
    matrix[0] = -0.9063077870366499;
    matrix[5] = -5.549534652183772e-17;
    matrix[9] = 9.868110562787479;
    matrix[10] = 471.35466923302613;
    e.projection.upserts[0].world_transform = glam::DMat4::from_cols_array(&matrix);
    let before = serde_json::to_vec(&e).unwrap();
    {
        let mut q = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
        q.handoff(vec![e.clone()]).unwrap();
    }
    let q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    let recovered = q.load(&e.job_id).unwrap();
    assert_eq!(
        matrix.map(f64::to_bits),
        recovered.projection.upserts[0]
            .world_transform
            .to_cols_array()
            .map(f64::to_bits)
    );
    assert_eq!(before, serde_json::to_vec(recovered.as_ref()).unwrap());
}

#[test]
fn publication_barrier_recovers_exact_plan_and_holds_unknown_write() {
    let tmp = tempfile::tempdir().unwrap();
    let e = envelope(1);
    let plan = PreparedRootPublication {
        version: FORMAT_VERSION,
        envelope_hash: digest(&serde_json::to_vec(&e).unwrap()),
        chunks: vec!["RETURN 'immutable chunk';".into()],
        memory_removals: vec![],
        memory_tube_branch_resets: vec![],
        upserted: 0,
        removed: 0,
        receipts: 1,
    };
    {
        let mut q = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
        q.handoff(vec![e.clone()]).unwrap();
        assert!(q.begin_publication(&e.job_id, None).is_err());
        q.begin_publication(&e.job_id, Some(plan.clone())).unwrap();
        assert_eq!(q.state(&e.job_id), Some(TaskState::Inflight));
    }
    let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    assert_eq!(q.state(&e.job_id), Some(TaskState::UnknownHold));
    assert_eq!(
        q.load_prepared(&e.job_id).unwrap().unwrap().hash().unwrap(),
        plan.hash().unwrap()
    );
    assert_eq!(q.next_ready(), None);
    assert!(q.begin_publication(&e.job_id, None).is_err());
    q.transition(&e.job_id, TaskState::Ready, None).unwrap();
    assert!(q.begin_publication(&e.job_id, Some(plan)).is_err());
    q.begin_publication(&e.job_id, None).unwrap();
    drop(q);
    let q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    assert_eq!(q.state(&e.job_id), Some(TaskState::UnknownHold));
}

/// 收口撞上并发写者只该推迟到下一轮，撞上真错误才暂停消费者。恢复投影时被「只写内存」
/// 的按需生成插队（镜像变旧、没敢换）属于前者——每一步都幂等，下一轮重读就是了。
#[test]
fn a_stale_projection_restore_defers_the_finalization_instead_of_pausing() {
    use super::runtime::finalization_defers;
    use crate::fast_model::model_db_adapter::ProjectionRestoreConflict;

    let conflict = anyhow::Error::from(ProjectionRestoreConflict {
        dbnum: 7997,
        attempts: 3,
    });
    assert!(finalization_defers(&conflict));
    assert!(
        finalization_defers(&conflict.context("initialization finalization")),
        "外面裹了一层 context 也得认得出来"
    );

    assert!(!finalization_defers(&anyhow::anyhow!(
        "committed projection is missing structured recovery data in inst_relate"
    )));
}

/// 一批根共用一次 fsync：逐根 `begin_publication_deferred` 不落盘，发本组第一条 SQL
/// 之前一次 `barrier` 覆盖整组。重开日志后三根都得是 UnknownHold（写意图确实落了盘），
/// 而这一组只多付一次 `file_syncs`。
#[test]
fn one_group_barrier_makes_every_root_intent_durable() {
    let tmp = tempfile::tempdir().unwrap();
    let roots: Vec<FrozenRootEnvelope> = (1..=3).map(envelope).collect();
    let plans: Vec<PreparedRootPublication> = roots
        .iter()
        .map(|e| PreparedRootPublication {
            version: FORMAT_VERSION,
            envelope_hash: digest(&serde_json::to_vec(e).unwrap()),
            chunks: vec![format!("RETURN '{}';", e.job_id)],
            memory_removals: vec![],
            memory_tube_branch_resets: vec![],
            upserted: 0,
            removed: 0,
            receipts: 1,
        })
        .collect();
    {
        let mut q = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
        q.handoff(roots.clone()).unwrap();
        let synced = q.file_syncs();
        for (e, plan) in roots.iter().zip(&plans) {
            q.begin_publication_deferred(&e.job_id, Some(plan.clone()))
                .unwrap();
            assert_eq!(q.state(&e.job_id), Some(TaskState::Inflight));
        }
        assert_eq!(q.file_syncs(), synced, "逐根登记写意图不该各自落盘");
        q.barrier().unwrap();
        assert_eq!(q.file_syncs(), synced + 1, "一组三根只落一次盘");
        q.barrier().unwrap();
        assert_eq!(q.file_syncs(), synced + 1, "没有新帧的屏障不再付一次 fsync");
    }
    let mut q = Journal::open(tmp.path().into(), 0, 1024 * 1024).unwrap();
    for (e, plan) in roots.iter().zip(&plans) {
        assert_eq!(q.state(&e.job_id), Some(TaskState::UnknownHold));
        assert_eq!(
            q.load_prepared(&e.job_id).unwrap().unwrap().hash().unwrap(),
            plan.hash().unwrap()
        );
    }
    assert_eq!(q.next_ready(), None);
}

#[test]
fn completed_cache_entries_do_not_accumulate_behind_a_held_result() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 1024 * 1024, 8 * 1024 * 1024).unwrap();
    let held = envelope(0);
    q.handoff(vec![held.clone()]).unwrap();
    q.transition(&held.job_id, TaskState::FailedHold, Some("retain".into()))
        .unwrap();
    for n in 1..=1000 {
        let e = envelope(n);
        q.handoff(vec![e.clone()]).unwrap();
        q.transition(&e.job_id, TaskState::Committed, None).unwrap();
        assert_eq!(q.cache_entry_counts(), (1, 1));
    }
    assert!(q.load(&held.job_id).is_ok());
    assert_eq!(
        q.health().memory_bytes,
        serde_json::to_vec(&held).unwrap().len() as u64
    );
}

#[tokio::test]
async fn disconnect_and_pending_response_timeout_are_uncertain_not_root_failures() {
    let sdk = "persist frozen root: Internal error: receiving from an empty and closed channel";
    assert!(runtime::is_connection_error(sdk));
    assert!(!runtime::is_connection_error(
        "invalid geometry: non-finite coordinate"
    ));
    let error = crate::fast_model::model_db_adapter::publication_request_with_timeout(
        std::future::pending::<anyhow::Result<()>>(),
        std::time::Duration::from_millis(5),
    )
    .await
    .unwrap_err();
    assert!(runtime::is_connection_error(&error.to_string()));
    let value = crate::fast_model::model_db_adapter::publication_request(async { Ok(7) })
        .await
        .unwrap();
    assert_eq!(value, 7);
}

#[test]
fn room_source_is_durable_before_manifest_and_survives_handoff_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let source = crate::fast_model::room_topology_source::fixture_source(7997, 1);
    let key = source.key();
    let mut m = manifest();
    m.room_source_keys = vec![key.clone()];
    m.source_token = source.source_token.clone();
    {
        let mut q = Journal::open(dir.path().into(), 1024 * 1024, 4 * 1024 * 1024).unwrap();
        assert!(q.register(m.clone()).is_err());
        q.append_room_source(source.clone()).unwrap();
        q.register(m).unwrap();
        q.handoff(vec![envelope(1)]).unwrap();
    }
    let mut q = Journal::open(dir.path().into(), 0, 4 * 1024 * 1024).unwrap();
    assert_eq!(
        q.load_room_source(&key).unwrap().source_token,
        source.source_token
    );
    assert_eq!(q.health().handed_off, 1);
    q.mark_room_source_stored(&key).unwrap();
    q.checkpoint().unwrap();
    drop(q);
    let q = Journal::open(dir.path().into(), 0, 4 * 1024 * 1024).unwrap();
    assert!(q.room_source_stored(&key));
    assert!(q.load_room_source(&key).is_err());
}

#[tokio::test]
async fn registry_retains_publication_after_last_model_task_and_fences_manifest() {
    let db = db().await;
    reserve_test(&db).await;
    let m = manifest();
    runtime::reserve_on(&db, &m, false).await.unwrap();
    db.query("DELETE model_update_pending;")
        .await
        .unwrap()
        .check()
        .unwrap();
    assert!(runtime::check_retained_runs_on(&db, &[]).await.is_err());
    runtime::check_retained_runs_on(&db, &[m.clone()])
        .await
        .unwrap();
    let mut changed = m.clone();
    changed.roots[0].revision += 1;
    assert!(
        runtime::check_retained_runs_on(&db, &[changed.clone()])
            .await
            .is_err()
    );
    assert!(
        runtime::registry_on(&db, &changed, false, true)
            .await
            .is_err()
    );
    runtime::registry_on(&db, &m, false, true).await.unwrap();
    runtime::check_retained_runs_on(&db, &[]).await.unwrap();
    // A lost local Finish is safe: the same binding may confirm completion again.
    runtime::registry_on(&db, &m, false, true).await.unwrap();
}

#[test]
fn completed_v2_manifests_remain_available_for_database_projection_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    let mut m = manifest();
    m.version = 2;
    assert!(
        q.register(m.clone()).is_err(),
        "new legacy runs remain disallowed"
    );
    drop(q);
    let event = serde_json::json!({"Manifest": m});
    let bytes = serde_json::to_vec(&event).unwrap();
    let segment = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "journal"))
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(segment)
        .unwrap();
    file.write_all(&(bytes.len() as u64).to_le_bytes()).unwrap();
    file.write_all(&hex::decode(digest(&bytes)).unwrap())
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mut q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    q.finish("aabb").unwrap();
    drop(q);
    let q = Journal::open(tmp.path().into(), 1024, 1024 * 1024).unwrap();
    assert_eq!(q.completed_manifests().len(), 1);
    assert!(q.manifests().is_empty());
}
