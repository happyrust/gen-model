use super::*;
use crate::data_interface::frozen_source::FrozenSource;
use crate::fast_model::{
    e3d_model_service::E3dModelService,
    model_db_adapter::{self, PersistTarget},
};
use aios_core::SUL_DB;
use anyhow::{Context, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, mpsc, oneshot};

struct Handoff {
    payload: FrozenRootEnvelope,
    reply: oneshot::Sender<Result<(), String>>,
}
struct Runtime {
    journal: Mutex<Journal>,
    started: AtomicBool,
    paused: AtomicBool,
    producing: Mutex<std::collections::BTreeSet<String>>,
    generating: Mutex<std::collections::BTreeSet<String>>,
    hydrated: Mutex<std::collections::BTreeSet<String>>,
    wake: Notify,
    handoff_tx: mpsc::Sender<Handoff>,
    /// 停放的 handoff 接收端：初始化时放进来一只，writer 出错退出时停回来；`None` = writer
    /// 正拿着它在跑。见 [`spawn_writer_if_parked`]。
    handoff_rx: Mutex<Option<mpsc::Receiver<Handoff>>>,
    /// writer 任务是否在跑（`/health` 的 `writer_running`）。
    writer_running: AtomicBool,
    last_error: Mutex<Option<String>>,
    timings: ConsumerTimings,
    /// 连接 / 超时类错误把消费者暂停后，由消费者自己在下一轮做对账（[`auto_reconcile`]）；
    /// 其它原因的暂停仍只等人工 `resume`。启动时按持久化的暂停原因恢复：原因是连接类就
    /// 直接置位，对账不因进程重启而丢（[`auto_reconcile_pending_on_open`]）。
    auto_reconcile_pending: AtomicBool,
    reconcile: ReconcileStats,
    /// 当前一次数据库请求最多打包多少根。连接 / 超时类错误后减半，连续成功后翻倍回到
    /// [`PUBLISH_BATCH_ROOTS`]：一批越大，一次超时牵连的根越多、对账越慢。
    batch_limit: AtomicUsize,
    batch_successes: AtomicUsize,
    /// 发布事务撞上乐观冲突后在原地重试的次数（[`publish_with_conflict_retry`]）。
    publish_conflict_retries: AtomicU64,
    /// 收口步撞上乐观冲突、推迟到下一轮而不是暂停消费者的次数。
    finalization_conflict_deferrals: AtomicU64,
}
#[derive(Default)]
struct ReconcileStats {
    rounds: AtomicU64,
    acked: AtomicU64,
    replayed: AtomicU64,
    exhausted: AtomicU64,
    unreachable: AtomicU64,
    /// 每个任务被自动重放过几次；到 [`AUTO_RECONCILE_MAX_REPLAYS`] 就留在 UnknownHold 等人。
    attempts: Mutex<BTreeMap<String, u32>>,
}
#[derive(Default)]
struct ConsumerTimings {
    validate_mesh: AtomicU64,
    prepare: AtomicU64,
    prepare_reconcile_read: AtomicU64,
    prepare_render_plan: AtomicU64,
    prepare_reconcile_queries: AtomicU64,
    prepared_plans: AtomicU64,
    prepared_chunks: AtomicU64,
    prepared_sql_bytes: AtomicU64,
    journal_begin: AtomicU64,
    publish: AtomicU64,
    journal_ack: AtomicU64,
}
fn record_elapsed(counter: &AtomicU64, since: Instant) {
    counter.fetch_add(
        since.elapsed().as_micros().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}
static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

fn runtime() -> anyhow::Result<&'static Arc<Runtime>> {
    if let Some(r) = RUNTIME.get() {
        return Ok(r);
    }
    let (root, memory, disk) = crate::options::model_publish_queue_limits();
    let option = aios_core::get_db_option();
    let identity = digest(
        format!(
            "{}\n{}\n{}",
            option.surreal_ns, option.project_name, option.mdb_name
        )
        .as_bytes(),
    );
    let journal = Journal::open(root.join(&identity[..24]), memory, disk)?
        .with_mesh_root(option.get_meshes_path())?;
    RUNTIME
        .set(Arc::new(Runtime::new(journal)))
        .map_err(|_| anyhow::anyhow!("model queue initialization raced"))?;
    Ok(RUNTIME.get().unwrap())
}

impl Runtime {
    /// 从已打开的日志建进程内状态：暂停标志与「待自动对账」都按持久化的暂停原因恢复。
    fn new(journal: Journal) -> Self {
        let pause_reason = journal.pause_reason();
        let (handoff_tx, handoff_rx) = mpsc::channel(1);
        Runtime {
            journal: Mutex::new(journal),
            started: AtomicBool::new(false),
            paused: AtomicBool::new(pause_reason.is_some()),
            producing: Mutex::new(Default::default()),
            wake: Notify::new(),
            generating: Mutex::new(Default::default()),
            hydrated: Mutex::new(Default::default()),
            handoff_tx,
            handoff_rx: Mutex::new(Some(handoff_rx)),
            writer_running: AtomicBool::new(false),
            auto_reconcile_pending: AtomicBool::new(auto_reconcile_pending_on_open(
                pause_reason.as_deref(),
            )),
            last_error: Mutex::new(pause_reason),
            timings: ConsumerTimings::default(),
            reconcile: ReconcileStats::default(),
            batch_limit: AtomicUsize::new(PUBLISH_BATCH_ROOTS),
            batch_successes: AtomicUsize::new(0),
            publish_conflict_retries: AtomicU64::new(0),
            finalization_conflict_deferrals: AtomicU64::new(0),
        }
    }
}

pub fn health() -> serde_json::Value {
    match RUNTIME.get() {
        Some(r) => {
            let mut value = serde_json::to_value(r.journal.lock().unwrap().health()).unwrap();
            let (geometry, receipts) =
                crate::fast_model::model_memory_store::ModelMemoryStore::global().current_counts();
            value["current_projection"] =
                serde_json::json!({"geometry":geometry,"receipts":receipts});
            value["consumer_paused"] = r.paused.load(Ordering::Acquire).into();
            value["last_error"] =
                serde_json::to_value(r.last_error.lock().unwrap().clone()).unwrap();
            value["consumer_timing_us"] = serde_json::json!({
                "validate_mesh":r.timings.validate_mesh.load(Ordering::Relaxed),
                "prepare":r.timings.prepare.load(Ordering::Relaxed),
                "prepare_reconcile_read":r.timings.prepare_reconcile_read.load(Ordering::Relaxed),
                "prepare_render_plan":r.timings.prepare_render_plan.load(Ordering::Relaxed),
                "journal_begin":r.timings.journal_begin.load(Ordering::Relaxed),
                "publish":r.timings.publish.load(Ordering::Relaxed),
                "journal_ack":r.timings.journal_ack.load(Ordering::Relaxed),
            });
            value["consumer_work"] = serde_json::json!({
                "prepare_reconcile_queries":r.timings.prepare_reconcile_queries.load(Ordering::Relaxed),
                "prepared_plans":r.timings.prepared_plans.load(Ordering::Relaxed),
                "prepared_chunks":r.timings.prepared_chunks.load(Ordering::Relaxed),
                "prepared_sql_bytes":r.timings.prepared_sql_bytes.load(Ordering::Relaxed),
                "journal_file_syncs":r.journal.lock().unwrap().file_syncs(),
            });
            value["publish_batch_limit"] = r.batch_limit.load(Ordering::Relaxed).into();
            value["publish_conflict_retries"] =
                r.publish_conflict_retries.load(Ordering::Relaxed).into();
            value["finalization_conflict_deferrals"] = r
                .finalization_conflict_deferrals
                .load(Ordering::Relaxed)
                .into();
            value["generation_admission"] = serde_json::json!({
                "max_active_runs": max_active_runs(),
                "in_flight_runs": r.journal.lock().unwrap().in_flight_runs(&r.producing.lock().unwrap()),
                "generating_runs": r.generating.lock().unwrap().len(),
            });
            value["writer_running"] = r.writer_running.load(Ordering::Acquire).into();
            value["auto_reconcile"] = serde_json::json!({
                "pending":r.auto_reconcile_pending.load(Ordering::Acquire),
                "rounds":r.reconcile.rounds.load(Ordering::Relaxed),
                "acked":r.reconcile.acked.load(Ordering::Relaxed),
                "replayed":r.reconcile.replayed.load(Ordering::Relaxed),
                "exhausted":r.reconcile.exhausted.load(Ordering::Relaxed),
                "unreachable":r.reconcile.unreachable.load(Ordering::Relaxed),
            });
            value
        }
        None => serde_json::json!({"active_runs":0,"pending_publish":0}),
    }
}
pub fn blocked_dbnums() -> Vec<u32> {
    RUNTIME
        .get()
        .map(|r| r.journal.lock().unwrap().unfinished_dbnums())
        .unwrap_or_default()
}
pub fn blocks_dbnum(dbnum: u32) -> bool {
    RUNTIME
        .get()
        .is_some_and(|r| r.journal.lock().unwrap().run_for_dbnum(dbnum).is_some())
}
pub fn preparing_dbnum(dbnum: u32) -> bool {
    RUNTIME.get().is_some_and(|r| {
        let journal = r.journal.lock().unwrap();
        journal
            .run_for_dbnum(dbnum)
            .is_some_and(|m| !journal.active(&m.run_id))
    })
}
pub fn planned_roots(dbnum: u32) -> usize {
    RUNTIME
        .get()
        .map(|r| {
            r.journal
                .lock()
                .unwrap()
                .run_for_dbnum(dbnum)
                .map(|m| m.roots.len())
                .unwrap_or(0)
        })
        .unwrap_or(0)
}
pub fn blocks_data_batch(batch: &crate::data_interface::batch_queue::DataBatch) -> bool {
    RUNTIME.get().is_some_and(|r| {
        let journal = r.journal.lock().unwrap();
        journal.run_for_dbnum(batch.dbnum).is_some_and(|m| {
            retained_run_blocks_batch(
                journal.active(&m.run_id),
                m.source_sesno as i32,
                batch.intent,
                batch.end_sesno,
            )
        })
    })
}

pub(super) fn retained_run_blocks_batch(
    active: bool,
    retained_target: i32,
    intent: crate::data_interface::batch_queue::BatchIntent,
    batch_target: i32,
) -> bool {
    let exact_recovery = intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly
        && batch_target == retained_target;
    batch_target != retained_target || (active && !exact_recovery)
}

/// A startup ModelOnly Task may adopt a retained initialization only when it
/// targets the exact frozen source version recorded by that run.
pub fn can_adopt_retained_initialization(dbnum: u32, target_sesno: i32) -> bool {
    RUNTIME.get().is_some_and(|r| {
        r.journal
            .lock()
            .unwrap()
            .run_for_dbnum(dbnum)
            .is_some_and(|m| m.source_sesno as i32 == target_sesno)
    })
}
pub async fn wait_for_dbnum(dbnum: u32) {
    while blocks_dbnum(dbnum) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Wait until every root in this Task's initialization run has a durable ACK.
/// The run intentionally remains active until the owning Task publishes spatial
/// and room results and advances the strict model watermark.
pub async fn wait_for_task_root_acks(
    mgr: &crate::data_interface::tidb_manager::AiosDBManager,
    dbnum: u32,
    source_sesno: u32,
    roots: &[String],
) -> anyhow::Result<bool> {
    loop {
        let Some(r) = RUNTIME.get() else {
            return Ok(false);
        };
        let (manifest, ready, failure) = {
            let journal = r.journal.lock().unwrap();
            let Some(manifest) = journal.run_for_dbnum(dbnum).cloned() else {
                return Ok(false);
            };
            let ready = journal
                .finish_candidates()
                .iter()
                .any(|candidate| candidate.run_id == manifest.run_id);
            let failure = journal.run_failure(&manifest.run_id);
            (manifest, ready, failure)
        };
        ensure!(
            manifest.source_sesno == source_sesno,
            "initialization run for dbnum {dbnum} targets {}, Task targets {source_sesno}",
            manifest.source_sesno
        );
        let manifest_roots = manifest
            .roots
            .iter()
            .map(|intent| intent.root.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            roots
                .iter()
                .all(|root| manifest_roots.contains(root.as_str())),
            "initialization run for dbnum {dbnum} does not cover every Task root"
        );
        if let Some(error) = failure {
            anyhow::bail!("initialization root publication failed for dbnum {dbnum}: {error}");
        }
        if ready {
            // Hydrate the acknowledged projection and collect its spatial fragments.
            // This helper no longer publishes a file round or advances a watermark.
            finish_ready_initializations_inner(mgr).await?;
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 该库活着的初始化 run 钉的那一版文件（`RunManifest.source_token` 还原成 `FrozenSource`）。
///
/// 给「只写内存」的按需生成用（plan 2026-09-06 §8 Q9）：按需投影与 run 稍后发布的行必须同版，
/// 翻面时 `commit_prepared_frozen_projection` 覆盖进来才零差异。没有 run、或令牌还原不出来
/// （文件已换）都回 `None`，调用方退到文件最新。
pub fn initialization_source(dbnum: u32) -> Option<FrozenSource> {
    let runtime = RUNTIME.get()?;
    let source_token = {
        let journal = runtime.journal.lock().unwrap();
        journal.run_for_dbnum(dbnum)?.source_token.clone()
    };
    FrozenSource::from_commit_token_json(&source_token).ok()
}

/// Run before the old process-ledger cleanup. A missing local manifest must
/// never be treated as permission to discard database-owned work.
pub async fn recover_before_cleanup() -> anyhow::Result<()> {
    if !crate::options::store_mode().is_durable() {
        return Ok(());
    }
    let r = runtime()?;
    let manifests = r.journal.lock().unwrap().manifests();
    #[derive(Deserialize)]
    struct Owner {
        init_run_id: String,
    }
    let owners: Vec<Owner> = SUL_DB.query("SELECT init_run_id FROM model_update_pending WHERE init_run_id IS NOT NONE GROUP BY init_run_id;")
        .await?.check()?.take(0)?;
    for owner in owners {
        ensure!(
            manifests.iter().any(|m| m.run_id == owner.init_run_id),
            "initialization {} has database-owned work but no local manifest; retain and repair queue",
            owner.init_run_id
        );
    }
    check_retained_runs_on(&SUL_DB, &manifests).await?;
    for manifest in manifests {
        validate_run_binding(&manifest).await?;
        if !r.journal.lock().unwrap().reserved(&manifest.run_id) {
            // Recovery may confirm a completed reservation, never reissue it.
            if let Err(error) = reserve_on(&SUL_DB, &manifest, true).await {
                let reason = format!(
                    "initialization reservation incomplete; manual resume required: {error:#}"
                );
                report_error(r, &manifest.run_id, manifest.dbnum, &reason);
                pause_consumer(r, &reason);
                continue;
            }
            r.journal.lock().unwrap().mark_reserved(&manifest.run_id)?;
        }
        let applied = source_is_applied(&manifest).await?;
        if r.journal.lock().unwrap().active(&manifest.run_id) {
            ensure!(
                applied,
                "active initialization attribute source changed; queue retained"
            );
        } else if applied {
            r.journal.lock().unwrap().activate(&manifest.run_id)?;
        }
    }
    // Finished runs can discard journal payloads: their committed structured
    // database rows remain the source for process-memory recovery.
    let completed = r.journal.lock().unwrap().completed_manifests();
    let mut restored = std::collections::BTreeSet::new();
    for m in completed {
        if blocks_dbnum(m.dbnum) || !restored.insert(m.dbnum) {
            continue;
        }
        ensure!(
            database_incarnation(false).await? == m.database_incarnation,
            "completed projection belongs to a different database"
        );
        model_db_adapter::restore_projection_on(
            &SUL_DB,
            crate::fast_model::model_memory_store::ModelMemoryStore::global(),
            m.dbnum,
        )
        .await?;
    }

    Ok(())
}

/// Capture all roots once while the initialization batch still owns its dbnum.
/// Repeated page claims and page-tail lease release do not participate afterward.
pub async fn admit_baseline(dbnum: u32, source: FrozenSource) -> anyhow::Result<()> {
    register_baseline(dbnum, source, true).await
}
pub async fn prepare_baseline(dbnum: u32, source: FrozenSource) -> anyhow::Result<()> {
    register_baseline(dbnum, source, false).await
}
async fn register_baseline(
    dbnum: u32,
    source: FrozenSource,
    publish_ready: bool,
) -> anyhow::Result<()> {
    if source.db_type() != "DESI" {
        return Ok(());
    }
    let r = runtime()?;
    let existing = r
        .journal
        .lock()
        .unwrap()
        .manifests()
        .into_iter()
        .find(|m| m.dbnum == dbnum);
    if let Some(m) = existing {
        ensure!(
            m.source_sesno == source.target_sesno()
                && m.source_token == source.commit_token_json()?,
            "baseline source differs from retained initialization"
        );
        validate_run_binding(&m).await?;
        ensure!(
            r.journal.lock().unwrap().reserved(&m.run_id),
            "initialization reservation incomplete; use manual resume"
        );
        if publish_ready {
            ensure!(
                source_is_applied(&m).await?,
                "initialization attribute source is not committed"
            );
            r.journal.lock().unwrap().activate(&m.run_id)?;
        }
        kick();
        return Ok(());
    }
    if !crate::options::init_async_publish() {
        return Ok(());
    }
    let session = source.target_sesno();
    let rows: Vec<crate::data_interface::model_update_pending::PendingModelWork> = SUL_DB
        .query(format!("SELECT * FROM model_update_pending WHERE dbnum={dbnum} AND action='regen_root' AND source_end_sesno={session};"))
        .await?.check()?.take(0)?;
    if rows.is_empty() {
        return Ok(());
    }
    let source_token = source.commit_token_json()?;
    let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
    let dependency_tokens = pins
        .into_iter()
        .filter(|p| p.dbnum as u32 != dbnum)
        .map(|p| FrozenSource::capture(&p.file)?.commit_token_json())
        .collect::<anyhow::Result<Vec<_>>>()?;
    let service = E3dModelService::from_initialization_sources(&source, &dependency_tokens).await?;
    let target_hash = service.initialization_target_hash(dbnum, session)?;
    let model_projection_empty_at_reservation =
        model_projection_is_empty_on(&SUL_DB, dbnum).await?;
    // 初始化的前提是该库的模型投影为空：清库按 dbnum → ref0 区间做（`fast_delete`），
    // `needs_initial_load` 之外没有别的入口。非空不再走逐根对账那条路——它对初始化是
    // 死路，而且批内跨根 reparent 时冻结的 removal 会删掉新根刚写的几何（2026-09-07
    // 审核 H1）。这里直接拒绝，留给人工清库；字段本身保留，供旧清单反序列化与库侧
    // `registry_on` 的空投影守卫使用。
    require_empty_model_projection(Some(model_projection_empty_at_reservation), dbnum, None)?;
    let room_key = crate::fast_model::room_topology_source::source_key(&source_token);
    if !r.journal.lock().unwrap().has_room_source(&room_key) {
        let topology = crate::fast_model::room_topology_source::capture(&source).await?;
        r.journal.lock().unwrap().append_room_source(topology)?;
    }
    let run_id = digest(format!("{}:{}:{}", source_token, now_ms(), std::process::id()).as_bytes());
    let roots = rows
        .into_iter()
        .map(|row| RootIntent {
            token: digest(format!("{}:{}:{}", run_id, row.target_refno, row.revision).as_bytes()),
            root: row.target_refno,
            revision: row.revision,
        })
        .collect();
    let manifest = RunManifest {
        version: FORMAT_VERSION,
        run_id,
        dbnum,
        source_sesno: session,
        source_token,
        database_incarnation: database_incarnation(true).await?,
        model_projection_empty_at_reservation,
        dependency_tokens,
        room_source_keys: vec![room_key],
        room_context_fingerprint: crate::fast_model::room_topology_source::context_fingerprint(),
        target_hash,
        roots,
        created_ms: now_ms(),
    };
    r.journal.lock().unwrap().register(manifest.clone())?;
    reserve_on(&SUL_DB, &manifest, false).await?;
    r.journal.lock().unwrap().mark_reserved(&manifest.run_id)?;
    // Initialization generates roots before the ordinary data-window finalize path
    // can create its spatial side-effect row. Seed the same collecting round here so
    // each root publish can attach fragments and finalization can release rooms.
    crate::data_interface::batch_worker::with_model_publication_window(
        manifest.dbnum,
        manifest.source_sesno as i32,
        &manifest.source_token,
        async {
            let sql = crate::data_interface::side_effect_pending::SideEffectCompensator::render_spatial_reconcile_upsert(
                manifest.dbnum,
                manifest.source_sesno as i32,
                &[],
                &[],
            )?;
            SUL_DB.query(sql).await?.check()?;
            anyhow::Ok(())
        },
    )
    .await?;
    if publish_ready {
        ensure!(
            source_is_applied(&manifest).await?,
            "initialization attribute source is not committed"
        );
        r.journal.lock().unwrap().activate(&manifest.run_id)?;
    }
    println!(
        "[model-publish] dbnum={dbnum} manifest roots={} publish_ready={publish_ready}",
        manifest.roots.len()
    );
    kick();
    Ok(())
}

async fn database_incarnation(create: bool) -> anyhow::Result<String> {
    if create {
        SUL_DB.query("UPSERT model_publish_database:current SET incarnation=incarnation ?? type::string(rand::uuid()) RETURN NONE;")
            .await?.check()?;
    }
    let values: Vec<String> = SUL_DB
        .query("SELECT VALUE incarnation FROM model_publish_database:current;")
        .await?
        .check()?
        .take(0)?;
    values.into_iter().next().context(
        "database incarnation missing; retained queues must not attach to a replacement database",
    )
}
async fn validate_run_binding(m: &RunManifest) -> anyhow::Result<()> {
    ensure!(
        m.version == FORMAT_VERSION,
        "incompatible initialization protocol {}; retained queue requires its original renderer",
        m.version
    );
    ensure!(
        m.room_context_fingerprint
            == crate::fast_model::room_topology_source::context_fingerprint(),
        "initialization room publication configuration changed; retain the original run"
    );
    ensure!(
        !m.database_incarnation.is_empty()
            && database_incarnation(false).await? == m.database_incarnation,
        "initialization belongs to a different database incarnation"
    );
    let source = FrozenSource::from_commit_token_json(&m.source_token)?;
    ensure!(
        source.dbnum() as u32 == m.dbnum
            && source.target_sesno() == m.source_sesno
            && source.db_type() == "DESI",
        "initialization source metadata mismatch"
    );
    let key = crate::fast_model::room_topology_source::source_key(&m.source_token);
    ensure!(
        m.room_source_keys == vec![key.clone()],
        "initialization room context identity mismatch"
    );
    let r = runtime()?;
    let reserved = r.journal.lock().unwrap().reserved(&m.run_id);
    if reserved {
        registry_on(&SUL_DB, m, false, false).await?;
    }
    let stored = r.journal.lock().unwrap().room_source_stored(&key);
    let topology = if stored {
        crate::fast_model::room_topology_source::load_on(&SUL_DB, &m.source_token).await?
    } else {
        r.journal.lock().unwrap().load_room_source(&key)?
    };
    ensure!(
        topology.source_token == m.source_token,
        "initialization room topology token mismatch"
    );
    Ok(())
}
async fn source_is_applied(m: &RunManifest) -> anyhow::Result<bool> {
    let rows: Vec<serde_json::Value> = SUL_DB
        .query(format!(
            "SELECT applied_sesno,applied_source_hash FROM dbnum_watermark:{};",
            m.dbnum
        ))
        .await?
        .check()?
        .take(0)?;
    Ok(rows.first().is_some_and(|row| {
        row["applied_sesno"].as_u64() == Some(m.source_sesno as u64)
            && row["applied_source_hash"].as_str()
                == Some(digest(m.source_token.as_bytes()).as_str())
    }))
}
/// This registry outlives root tasks, including the model/space publication gap.
fn binding_hash(m: &RunManifest) -> anyhow::Result<String> {
    Ok(digest(&serde_json::to_vec(m)?))
}
/// 初始化 run 的硬不变量：登记与消费两侧都只接受「模型投影为空」的库。
///
/// `empty = Some(false)` 是在非空投影上登记（或保留下来的旧清单）；`None` 是清单里
/// 根本没有这枚证据（协议 2 之前的 JSON 反序列化默认 false，所以实际只会以 `Some(false)`
/// 到这里，`None` 留给未来把字段改成 Option 的情形）。两种都拒绝：非空的库先按
/// dbnum → ref0 清库（`fast_delete::wipe_dbnum_for_reinit`）再初始化，不在发布路径上
/// 逐根对账。
pub(super) fn require_empty_model_projection(
    empty: Option<bool>,
    dbnum: u32,
    run_id: Option<&str>,
) -> anyhow::Result<()> {
    if empty == Some(true) {
        return Ok(());
    }
    let run = run_id.map(|id| format!(" run={id}")).unwrap_or_default();
    anyhow::bail!(
        "dbnum={dbnum}{run} initialization requires an empty model projection (inst_relate / tubi_relate rows exist{}); \
         wipe the dbnum by ref0 ranges (fast_delete) and re-initialize instead of reconciling per root",
        if empty.is_none() {
            " or the manifest carries no emptiness proof"
        } else {
            ""
        }
    )
}

pub(super) async fn model_projection_is_empty_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    dbnum: u32,
) -> anyhow::Result<bool> {
    let mut result = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={dbnum} LIMIT 1)) = 0 AND array::len((SELECT VALUE id FROM tubi_relate WHERE dbnum={dbnum} LIMIT 1)) = 0;"
        ))
        .await?
        .check()?;
    Ok(result.take::<Option<bool>>(0)?.unwrap_or(false))
}
pub(super) async fn registry_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    m: &RunManifest,
    create: bool,
    finish: bool,
) -> anyhow::Result<()> {
    let run = serde_json::to_string(&m.run_id)?;
    let binding = serde_json::to_string(&binding_hash(m)?)?;
    let incarnation = serde_json::to_string(&m.database_incarnation)?;
    let missing = if create {
        let empty_guard = if m.model_projection_empty_at_reservation {
            format!(
                "IF array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={} LIMIT 1)) > 0 OR array::len((SELECT VALUE id FROM tubi_relate WHERE dbnum={} LIMIT 1)) > 0 {{ THROW 'initialization empty model projection proof changed'; }};",
                m.dbnum, m.dbnum
            )
        } else {
            String::new()
        };
        format!(
            "{empty_guard} CREATE type::thing('model_publish_run',{run}) SET run_id={run},binding_hash={binding},manifest_version={},dbnum={},source_sesno={},database_incarnation={incarnation},status='reserved' RETURN NONE;",
            m.version, m.dbnum, m.source_sesno
        )
    } else {
        "THROW 'initialization registry missing; retain local queue';".into()
    };
    let done = if finish {
        format!("UPDATE type::thing('model_publish_run',{run}) SET status='done' RETURN NONE;")
    } else {
        String::new()
    };
    let sql = format!("BEGIN TRANSACTION;
        IF (SELECT VALUE incarnation FROM model_publish_database:current)[0] != {incarnation} {{ THROW 'initialization database changed'; }};
        LET $registry = (SELECT * FROM type::thing('model_publish_run',{run}))[0];
        IF $registry = NONE {{ {missing} }} ELSE IF $registry.binding_hash != {binding} OR $registry.database_incarnation != {incarnation} {{ THROW 'initialization registry binding changed'; }};
        {done} COMMIT TRANSACTION;");
    model_db_adapter::publication_request(async {
        db.query(sql).await?.check()?;
        Ok(())
    })
    .await
}
pub(super) async fn check_retained_runs_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    manifests: &[RunManifest],
) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    struct Registry {
        run_id: String,
        binding_hash: String,
    }
    let rows: Vec<Registry> = db.query("SELECT run_id,binding_hash FROM model_publish_run WHERE manifest_version >= 3 AND (status IS NONE OR status != 'done');").await?.check()?.take(0)?;
    for row in rows {
        let m = manifests.iter().find(|m| m.run_id == row.run_id)
            .with_context(|| format!("initialization {} has unfinished database publication but no local manifest; retain and repair queue", row.run_id))?;
        ensure!(
            binding_hash(m)? == row.binding_hash,
            "initialization registry manifest mismatch; retain queue"
        );
    }
    Ok(())
}
pub(super) async fn reserve_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    manifest: &RunManifest,
    verify_only: bool,
) -> anyhow::Result<()> {
    registry_on(db, manifest, !verify_only, false).await?;
    let quoted = |s: &str| serde_json::to_string(s).expect("string");
    let run = quoted(&manifest.run_id);
    let hash = quoted(&manifest.target_hash);
    let source_hash = quoted(&digest(manifest.source_token.as_bytes()));
    let incarnation = quoted(&manifest.database_incarnation);
    for chunk in manifest.roots.chunks(500) {
        let mut sql = format!(
            "BEGIN TRANSACTION;\nIF (SELECT VALUE incarnation FROM model_publish_database:current)[0] != {incarnation} {{ THROW 'initialization database changed'; }};\n"
        );
        for root in chunk {
            let id = pending_id(&root.root)?;
            let root_key = root.root.replace('/', "_");
            let token = quoted(&root.token);
            let rev = root.revision;
            let dbnum = manifest.dbnum;
            let session = manifest.source_sesno;
            sql.push_str(&format!(
                "LET $work = (SELECT * FROM {id} LIMIT 1)[0];\n\
                 LET $root = (SELECT * FROM gen_root:⟨{root_key}⟩ LIMIT 1)[0];\n\
                 IF $work = NONE OR $root = NONE OR $root.desired_revision != {rev} OR $work.revision != {rev} \
                    OR $work.dbnum != {dbnum} OR $work.source_end_sesno != {session} \
                    OR ($work.init_run_id IS NOT NONE AND $work.init_run_id != {run}) \
                    OR ($work.init_token IS NOT NONE AND $work.init_token != {token}) \
                    OR ($work.init_target_hash IS NOT NONE AND $work.init_target_hash != {hash}) \
                    OR ($work.init_source_hash IS NOT NONE AND $work.init_source_hash != {source_hash}) \
                    OR ($root.init_target_hash IS NOT NONE AND $root.init_target_hash != {hash}) \
                    {{ THROW 'initialization manifest reservation changed'; }};\n"));
            if verify_only {
                sql.push_str(&format!("IF $work.init_run_id != {run} OR $work.init_token != {token} OR $work.init_target_hash != {hash} OR $work.init_source_hash != {source_hash} OR $root.init_target_hash != {hash} {{ THROW 'initialization reservation incomplete'; }};\n"));
            } else {
                sql.push_str(&format!("UPDATE {id} SET init_run_id={run},init_token={token},init_target_hash={hash},init_source_hash={source_hash},lease_owner=NONE,lease_until=NONE RETURN NONE;\nUPDATE gen_root:⟨{root_key}⟩ SET init_target_hash={hash} WHERE desired_revision={rev} RETURN NONE;\n"));
            }
        }
        sql.push_str("COMMIT TRANSACTION;");
        db.query(sql).await?.check()?;
    }
    Ok(())
}
fn pending_id(root: &str) -> anyhow::Result<String> {
    let r = crate::fast_model::e3d_model_service::parse_refno(root)?;
    Ok(format!(
        "model_update_pending:regen_root_{}_{}",
        r.word0, r.word1
    ))
}
fn job_id(run: &str, root: &RootIntent) -> String {
    digest(format!("{run}:{}:{}", root.root, root.revision).as_bytes())
}

/// Called after initialization gates are available, and whenever work arrives.
pub fn kick() {
    let Some(r) = RUNTIME.get().cloned() else {
        return;
    };
    if crate::data_interface::initialization_phase::require_model_generation().is_err() {
        return;
    }
    // writer 与 consumer / 生产循环分开管：后两者一次性拉起（`started`），writer 则
    // 「有停放的接收端就拉」——它出错退出时把接收端停回 `handoff_rx`（见 [`writer`]），
    // 人工 `/resume` 再 `kick()` 就能重建，不必重启进程。
    spawn_writer_if_parked(&r);
    if r.started.swap(true, Ordering::AcqRel) {
        r.wake.notify_one();
        return;
    }
    tokio::spawn(consumer(r.clone()));
    tokio::spawn(async move {
        let mut announced_limit = None;
        loop {
            let limit = max_active_runs();
            let (pending, in_flight) = {
                let journal = r.journal.lock().unwrap();
                let producing = r.producing.lock().unwrap();
                (
                    journal.pending_manifests(&producing),
                    journal.in_flight_runs(&producing),
                )
            };
            let manifests = admit_runs(pending, in_flight, limit);
            if announced_limit != Some(limit) && !manifests.is_empty() {
                announced_limit = Some(limit);
                println!(
                    "[model-publish] 初始化生成准入：同时在飞 run 上限 = {}（AIOS_INIT_MAX_ACTIVE_RUNS，0 = 不限），大库先排",
                    limit
                );
            }
            for manifest in manifests {
                if r.producing.lock().unwrap().insert(manifest.run_id.clone()) {
                    r.generating.lock().unwrap().insert(manifest.run_id.clone());
                    let r = r.clone();
                    tokio::spawn(async move {
                        if let Err(e) = produce_run(r.clone(), &manifest).await {
                            report_error(
                                &r,
                                &format!("run:{}", manifest.run_id),
                                manifest.dbnum,
                                &format!("generation: {e:#}"),
                            );
                            for root in &manifest.roots {
                                let payload = FrozenRootEnvelope {
                                    version: FORMAT_VERSION,
                                    job_id: job_id(&manifest.run_id, root),
                                    run_id: manifest.run_id.clone(),
                                    dbnum: manifest.dbnum,
                                    source_sesno: manifest.source_sesno,
                                    source_token: manifest.source_token.clone(),
                                    database_incarnation: manifest.database_incarnation.clone(),
                                    intent: root.clone(),
                                    target_hash: manifest.target_hash.clone(),
                                    projection: Default::default(),
                                    created_ms: now_ms(),
                                };
                                if let Err(error) = r
                                    .journal
                                    .lock()
                                    .unwrap()
                                    .generation_failure(payload, format!("generation: {e:#}"))
                                {
                                    report_error(
                                        &r,
                                        &format!("run:{}", manifest.run_id),
                                        manifest.dbnum,
                                        &format!("record generation hold: {error:#}"),
                                    );
                                    break;
                                }
                            }
                        }
                        r.generating.lock().unwrap().remove(&manifest.run_id);
                        // Do not automatically restart failed generation. The run
                        // remains in producing until explicit operator recovery.
                    });
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// 有停放的接收端就拉起 writer；返回是否真的拉了。首次 `kick()` 拉的是初始化时放进去的
/// 那只；writer 出错退出后停回去的那只由下一次 `kick()`（通常来自 `/resume`）再拉。
fn spawn_writer_if_parked(r: &Arc<Runtime>) -> bool {
    let Some(rx) = r.handoff_rx.lock().unwrap().take() else {
        return false;
    };
    r.writer_running.store(true, Ordering::Release);
    tokio::spawn(writer(r.clone(), rx));
    true
}

async fn writer(r: Arc<Runtime>, mut rx: mpsc::Receiver<Handoff>) {
    let stopped = writer_loop(&r, &mut rx).await;
    // 出错退出：接收端停回去而不是丢掉——通道还在，生产者的 `send` 会等在原地（容量 1），
    // 不会再得到「journal writer unavailable」；暂停原因持久化（`set_pause`），重启后仍然
    // 看得见；`/resume` → `kick()` → [`spawn_writer_if_parked`] 重建 writer。
    *r.handoff_rx.lock().unwrap() = Some(rx);
    r.writer_running.store(false, Ordering::Release);
    if let Some(error) = stopped {
        report_error(&r, "journal", 0, &error);
        pause_consumer(&r, &format!("journal writer stopped: {error}"));
    }
}

/// writer 主循环；`Some(错误)` = 因错误退出，`None` = 所有发送端都没了（Runtime 持有一只
/// 发送端，正常不会发生）。
async fn writer_loop(r: &Arc<Runtime>, rx: &mut mpsc::Receiver<Handoff>) -> Option<String> {
    while let Some(first) = rx.recv().await {
        let mut group = vec![first];
        let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
        let mut bytes = serde_json::to_vec(&group[0].payload)
            .map(|v| v.len())
            .unwrap_or(0);
        while bytes < 8 * 1024 * 1024 {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(next)) => {
                    bytes += serde_json::to_vec(&next.payload)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    group.push(next);
                }
                _ => break,
            }
        }
        let payloads: Vec<_> = group.iter().map(|h| h.payload.clone()).collect();
        loop {
            let capacity = r.journal.lock().unwrap().can_handoff_batch(&payloads);
            match capacity {
                Ok(true) => break,
                Ok(false) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => {
                    let error = format!("{error:#}");
                    for h in group {
                        let _ = h.reply.send(Err(error.clone()));
                    }
                    return Some(error);
                }
            }
        }
        let worker = r.clone();
        let result =
            tokio::task::spawn_blocking(move || worker.journal.lock().unwrap().handoff(payloads))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| format!("{e:#}")));
        let failed = result.clone().err();
        for h in group {
            let _ = h.reply.send(result.clone());
        }
        r.wake.notify_one();
        if let Some(error) = failed {
            return Some(error);
        }
    }
    None
}

async fn produce_run(r: Arc<Runtime>, manifest: &RunManifest) -> anyhow::Result<()> {
    let roots: BTreeMap<String, RootIntent> = manifest
        .roots
        .iter()
        .filter(|root| {
            r.journal
                .lock()
                .unwrap()
                .state(&job_id(&manifest.run_id, root))
                .is_none()
        })
        .map(|root| (root.root.clone(), root.clone()))
        .collect();
    if roots.is_empty() {
        return Ok(());
    }
    let source = FrozenSource::from_commit_token_json(&manifest.source_token)?.reopen_verified()?;
    let service =
        E3dModelService::from_initialization_sources(&source, &manifest.dependency_tokens).await?;
    ensure!(
        service.initialization_target_hash(manifest.dbnum, manifest.source_sesno)?
            == manifest.target_hash,
        "frozen initialization target changed; run retained"
    );
    let (tx, mut rx) = mpsc::channel(1);
    let list = roots.keys().cloned().collect::<Vec<_>>();
    let generating = service.generate_frozen_roots(manifest.dbnum, &list, tx);
    let accepting = async move {
        // Keep acknowledgements in flight so a single producer can fill an
        // 8 MiB / 20 ms fsync group. Waiting after every send forces one fsync
        // deadline per root even when the generation channel is already full.
        let mut acknowledgements = std::collections::VecDeque::new();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(result) = rx.recv().await {
            let key = format!("{}/{}", result.root.word0, result.root.word1);
            let intent = roots
                .get(&key)
                .context("generator returned an unowned root")?
                .clone();
            ensure!(seen.insert(key), "generator returned a root twice");
            let (projection, failure) = match result.outcome {
                Ok(p) => (p, None),
                Err(e) => (Default::default(), Some(e)),
            };
            let target_hash = digest(&serde_json::to_vec(
                &projection.receipt.as_ref().map(|(_, r)| &r.model_target),
            )?);
            ensure!(
                failure.is_some() || target_hash == manifest.target_hash,
                "generated target differs from frozen manifest"
            );
            let id = job_id(&manifest.run_id, &intent);
            let payload = FrozenRootEnvelope {
                version: FORMAT_VERSION,
                job_id: id.clone(),
                run_id: manifest.run_id.clone(),
                dbnum: manifest.dbnum,
                source_sesno: manifest.source_sesno,
                source_token: manifest.source_token.clone(),
                database_incarnation: manifest.database_incarnation.clone(),
                intent,
                target_hash,
                projection,
                created_ms: now_ms(),
            };
            if let Some(error) = failure {
                r.journal
                    .lock()
                    .unwrap()
                    .generation_failure(payload, error.clone())?;
                report_error(&r, &id, manifest.dbnum, &error);
                continue;
            }
            payload.validate()?;
            while !r.journal.lock().unwrap().can_handoff(&payload)? {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let (tx, reply) = oneshot::channel();
            r.handoff_tx
                .send(Handoff { payload, reply: tx })
                .await
                .map_err(|_| anyhow::anyhow!("journal writer unavailable"))?;
            acknowledgements.push_back((id, None, reply));
            if acknowledgements.len() >= 64 {
                let (id, failure, reply) = acknowledgements.pop_front().unwrap();
                confirm_handoff(&r, manifest.dbnum, &id, failure, reply).await?;
            }
        }
        while let Some((id, failure, reply)) = acknowledgements.pop_front() {
            confirm_handoff(&r, manifest.dbnum, &id, failure, reply).await?;
        }
        ensure!(seen.len() == roots.len(), "generator omitted planned roots");
        Ok::<_, anyhow::Error>(())
    };
    finish_generation(generating, accepting).await
}

async fn finish_generation(
    generating: impl std::future::Future<Output = anyhow::Result<()>>,
    accepting: impl std::future::Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let (generated, accepted) = futures::future::join(generating, accepting).await;
    // Either side may fail first: a rejected envelope closes the sender's
    // receiver, while a failed generator makes the accepted root set partial.
    // Keep both causes rather than masking the first with its consequence.
    match (generated, accepted) {
        (Err(generated), Err(accepted)) => {
            if generated
                .chain()
                .any(|cause| cause.to_string() == "initialization journal receiver closed")
            {
                anyhow::bail!(
                    "initialization handoff failed: {accepted:#}; generation also failed: {generated:#}"
                );
            }
            anyhow::bail!(
                "initialization generation failed: {generated:#}; handoff also failed: {accepted:#}"
            );
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
#[path = "generation_handoff_tests.rs"]
mod generation_handoff_tests;

#[cfg(test)]
mod writer_recovery_tests {
    use super::*;

    fn runtime_with_disk_budget(disk: u64) -> (tempfile::TempDir, Arc<Runtime>) {
        let tmp = tempfile::tempdir().unwrap();
        let journal = Journal::open(tmp.path().into(), 1024 * 1024, disk).unwrap();
        (tmp, Arc::new(Runtime::new(journal)))
    }

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
            projection: Default::default(),
        }
    }

    async fn handoff(r: &Runtime, payload: FrozenRootEnvelope) -> Result<(), String> {
        let (tx, reply) = oneshot::channel();
        r.handoff_tx
            .send(Handoff { payload, reply: tx })
            .await
            .map_err(|_| "journal writer unavailable".to_string())?;
        reply.await.map_err(|e| e.to_string())?
    }

    async fn wait_until(mut ready: impl FnMut() -> bool) {
        for _ in 0..200 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached in time");
    }

    /// I（2026-09-07 审核）：writer 出错退出后，接收端停回 `handoff_rx`、暂停原因持久化，
    /// 再一次 `kick`（这里直接调 [`spawn_writer_if_parked`]）就能重建 writer——不用重启进程。
    #[tokio::test]
    async fn writer_parks_its_receiver_on_failure_and_can_be_respawned() {
        // 400 字节的盘预算：任何一组 handoff 都超预算，`can_handoff_batch` 直接报错。
        let (_tmp, r) = runtime_with_disk_budget(400);
        assert!(spawn_writer_if_parked(&r), "first kick spawns the writer");
        assert!(
            !spawn_writer_if_parked(&r),
            "a running writer owns the receiver"
        );
        assert!(r.writer_running.load(Ordering::Acquire));

        let error = handoff(&r, envelope(1)).await.unwrap_err();
        assert!(error.contains("exceed the entire queue budget"), "{error}");
        wait_until(|| !r.writer_running.load(Ordering::Acquire)).await;
        assert!(
            r.handoff_rx.lock().unwrap().is_some(),
            "receiver is parked, not dropped"
        );
        assert!(r.paused.load(Ordering::Acquire));
        let reason = r
            .journal
            .lock()
            .unwrap()
            .pause_reason()
            .expect("pause is persisted");
        assert!(reason.starts_with("journal writer stopped: "), "{reason}");
        assert!(
            !auto_reconcile_pending_on_open(Some(&reason)),
            "a journal failure is not a connection-class pause"
        );

        assert!(spawn_writer_if_parked(&r), "resume can respawn the writer");
        assert!(r.writer_running.load(Ordering::Acquire));
        assert!(r.handoff_rx.lock().unwrap().is_none());
        // 新 writer 真的在服务：同一条通道上的下一次交接得到的是它的回执，不是「不可用」。
        let error = handoff(&r, envelope(2)).await.unwrap_err();
        assert!(error.contains("exceed the entire queue budget"), "{error}");
    }

    /// M1：连接 / 超时类的持久暂停原因在启动时恢复为「待自动对账」，其它原因不恢复。
    #[test]
    fn connection_class_pause_reopens_as_pending_reconcile() {
        assert!(auto_reconcile_pending_on_open(Some(
            "model publication database request timed out; retain result and reconcile database ACK before replay"
        )));
        assert!(auto_reconcile_pending_on_open(Some(
            "publication statements failed: statement 3: websocket connection closed"
        )));
        assert!(!auto_reconcile_pending_on_open(Some(
            "payload: decode frozen envelope failed"
        )));
        assert!(!auto_reconcile_pending_on_open(Some(
            "journal writer stopped: one handoff group and mesh dependencies exceed the entire queue budget"
        )));
        assert!(!auto_reconcile_pending_on_open(None));
    }

    #[test]
    fn runtime_opens_with_pending_reconcile_only_for_connection_pauses() {
        let tmp = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
        journal
            .set_pause(Some("model publication database request timed out".into()))
            .unwrap();
        let r = Runtime::new(journal);
        assert!(r.paused.load(Ordering::Acquire));
        assert!(r.auto_reconcile_pending.load(Ordering::Acquire));

        let tmp = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(tmp.path().into(), 1024 * 1024, 1024 * 1024).unwrap();
        journal
            .set_pause(Some("payload: bad frame".into()))
            .unwrap();
        let r = Runtime::new(journal);
        assert!(r.paused.load(Ordering::Acquire));
        assert!(!r.auto_reconcile_pending.load(Ordering::Acquire));
    }
}

async fn confirm_handoff(
    r: &Runtime,
    dbnum: u32,
    id: &str,
    failure: Option<String>,
    reply: oneshot::Receiver<Result<(), String>>,
) -> anyhow::Result<()> {
    reply.await?.map_err(anyhow::Error::msg)?;
    if let Some(error) = failure {
        r.journal
            .lock()
            .unwrap()
            .transition(id, TaskState::FailedHold, Some(error.clone()))?;
        report_error(r, id, dbnum, &error);
    }
    Ok(())
}

async fn consumer(r: Arc<Runtime>) {
    // Read-only reconciliation of interrupted writes is not an automatic retry.
    let held = r.journal.lock().unwrap().held();
    for id in held {
        let e = match r.journal.lock().unwrap().load(&id) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let plan = match r.journal.lock().unwrap().load_prepared(&id) {
            Ok(Some(plan)) => plan,
            _ => continue,
        };
        if final_ack(&e, &plan).await == Ok(true) {
            // All chunks are already acknowledged: the adapter skips their
            // writes and only re-establishes the process-local spatial view.
            if model_db_adapter::persist_prepared_frozen_on(PersistTarget::process(), &e, &plan)
                .await
                .is_ok()
            {
                let _ = r
                    .journal
                    .lock()
                    .unwrap()
                    .transition(&id, TaskState::Committed, None);
            }
        }
    }
    loop {
        if r.paused.load(Ordering::Acquire) {
            if r.auto_reconcile_pending.swap(false, Ordering::AcqRel) {
                auto_reconcile(&r).await;
                continue;
            }
        } else {
            // 属性解析与模型生成可以并发；两边同时向 RocksDB/ingest 发布会让写入
            // 延迟成倍增长。属性批次运行时只暂停数据库消费者，生成器仍可继续把
            // 冻结结果持久交接到本地日志。短轮询保证批次结束后无需跨模块通知也能
            // 立即恢复，同时不会领取一页任务后再长期占住它们。
            //
            // 让带宽**按库让**：别的库的属性批次不该停掉本库的发布，否则 28 个库串起来
            // 就是 Σ属性 + Σ发布，一次都不重叠。
            let data_writing = crate::data_interface::batch_scheduler::BatchScheduler::global()
                .data_writing_dbnums();
            if !data_writing.is_empty()
                && data_batch_blocks_publication(
                    &data_writing,
                    &r.journal.lock().unwrap().active_dbnums(),
                )
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let limit = r
                .batch_limit
                .load(Ordering::Relaxed)
                .clamp(1, PUBLISH_BATCH_ROOTS);
            let ids = r.journal.lock().unwrap().next_ready_batch(limit);
            if !ids.is_empty() {
                consume_ready_jobs(&r, ids).await;
                continue;
            }
        }
        tokio::select! { _ = r.wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
    }
}

/// 一次数据库请求最多打包多少根。
///
/// 2026-09-07 从 32 提到 96：AMS 7997 那轮实测每批平均只有 1.45 MB，卡住的是根数而不是
/// [`PUBLISH_BATCH_BYTES`]（字节那一维还空着 2.8 倍），212 批 / 74.6 s = 352 ms 一个往返，
/// 而服务侧 0.30 核、库侧 0.82 核都远没跑满——每批都在付一笔固定开销。批量失败已按
/// statement index 逐根归因（不再连坐），所以「一批越大牵连越多」的代价比当初便宜了。
const PUBLISH_BATCH_ROOTS: usize = 32;
const PUBLISH_BATCH_BYTES: usize = 4 * 1024 * 1024;
/// 连续成功这么多批之后把批量上限翻倍回去（最多到 [`PUBLISH_BATCH_ROOTS`]）。
const PUBLISH_BATCH_REGROW_AFTER: usize = 8;
/// 连接 / 超时类暂停之后先等一段再对账：让还在路上的事务在库端落定或彻底失败。
const AUTO_RECONCILE_GRACE: Duration = Duration::from_secs(5);
/// 每次探活的期限，以及最多探几轮（探活之间也等 [`AUTO_RECONCILE_PROBE_TIMEOUT`]）。
const AUTO_RECONCILE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const AUTO_RECONCILE_PROBE_ROUNDS: u32 = 12;
/// 同一根最多自动重放几次；用尽后留在 UnknownHold，消费者保持暂停等人工处理。
const AUTO_RECONCILE_MAX_REPLAYS: u32 = 3;
/// 一组发布事务撞上乐观冲突时在原地最多试几次（含第一次）。
///
/// SurrealDB 的「read or write conflict」是多个写者碰到同一批键：事务整体回滚，什么都没落，
/// 重试即自愈——它和语法 / 约束错误不是一回事。2026-09-06 全 DESI 那一轮里它被当成终态
/// 失败记成 FailedHold，4,090 根 hold 把 7 个库的文件轮次永久堵住。次数用尽仍撞上才是真的
/// 失败，照旧进 FailedHold 等人。
const PUBLISH_CONFLICT_ATTEMPTS: usize = 8;
/// 同时处于「已开始生成、尚未收口」的初始化 run 上限的默认值。
///
/// 每个 run 在收口前一直握着自己的生成输入（`DbSet` + `SourceIndex`，实测 ≈ 6× 库文件大小：
/// 7997 的 57 MB 文件 → +360 MB RSS）与在飞几何；几何闸只管 CPU（ADR-052），不管这一头。
/// 全 DESI 28 个 run 同时在飞实测服务 RSS 3.86 GB；2 个在飞 ≈ 160 + 2×360 MB。发布是
/// 单消费者、本来就是关键路径，多于两三个 run 同时生成只是把几何攒在 journal 里排队。
const DEFAULT_MAX_ACTIVE_RUNS: usize = 2;

/// 在飞 run 上限：环境变量 `AIOS_INIT_MAX_ACTIVE_RUNS`，0 = 不限（2026-09-06 之前的行为）。
fn max_active_runs() -> usize {
    std::env::var("AIOS_INIT_MAX_ACTIVE_RUNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_ACTIVE_RUNS)
}

/// 从待启动的 run 里挑这一轮可以开始生成的：**先大后小**（大库先跑，尾巴都是小库，
/// 单消费者不会最后卡在一个大库上），数量 = 上限 − 已在飞；上限 0 = 全放行。
fn admit_runs(mut pending: Vec<RunManifest>, in_flight: usize, limit: usize) -> Vec<RunManifest> {
    pending.sort_by(|a, b| {
        b.roots
            .len()
            .cmp(&a.roots.len())
            .then_with(|| a.run_id.cmp(&b.run_id))
    });
    if limit != 0 {
        pending.truncate(limit.saturating_sub(in_flight));
    }
    pending
}

/// 消费者要不要为正在跑的数据批次让路：**只让同一个库的**。
///
/// 让路是为了不跟属性/结构写回抢同一份数据库写带宽，而这件事只在同一个库上真正互斥；
/// 别的库的批次跟本库的发布没有先后关系，停下来只是白白把 28 个库串成 Σ属性 + Σ发布。
///
/// 同库其实撞不上：`blocks_data_batch` 不让「该库 run 还活着」的批次进 Running
/// （`batch_scheduler.rs` 的 `freeze_next_concurrent` 那道谓词），而
/// [`Journal::next_ready`] 只交出 active run 的根。这道门是把那条不变量钉在原地，
/// 万一哪天上面两处松了，让路的行为还在，而不是悄悄变成两个写者对着干。
pub(super) fn data_batch_blocks_publication(
    data_writing_dbnums: &[u32],
    active_publish_dbnums: &[u32],
) -> bool {
    data_writing_dbnums
        .iter()
        .any(|dbnum| active_publish_dbnums.contains(dbnum))
}

/// 这一发失败要不要在原地再试：只认乐观冲突的错误文本，且次数未用尽。
fn should_retry_publication_conflict(error: &str, attempt: usize) -> bool {
    attempt < PUBLISH_CONFLICT_ATTEMPTS
        && crate::surreal_retry::is_retryable_surreal_write_error(error)
}

/// 发一组已备好的根；乐观冲突按指数抖动退避在原地重试（[`PUBLISH_CONFLICT_ATTEMPTS`]）。
///
/// 安全前提：发布语句块是一个事务（冲突 = 整体回滚、一行未落），且本来就按 ADR-064 幂等
/// 重放设计；不确定的结果（超时 / 断连）不在这里重试，仍交给 ACK 对账。
async fn publish_with_conflict_retry(r: &Runtime, jobs: &[PreparedJob]) -> anyhow::Result<()> {
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        let result = if jobs.len() == 1 {
            model_db_adapter::persist_prepared_frozen_on(
                PersistTarget::process(),
                &jobs[0].envelope,
                &jobs[0].plan,
            )
            .await
            .map(|_| ())
        } else {
            let publications = jobs
                .iter()
                .map(|job| (job.envelope.as_ref(), &job.plan))
                .collect::<Vec<_>>();
            model_db_adapter::persist_prepared_frozen_batch_on(
                PersistTarget::process(),
                &publications,
            )
            .await
            .map(|_| ())
        };
        match result {
            Err(error) if should_retry_publication_conflict(&format!("{error:#}"), attempt) => {
                r.publish_conflict_retries.fetch_add(1, Ordering::Relaxed);
                if attempt == 1 {
                    println!(
                        "[model-publish] dbnum={} 发布事务撞上乐观冲突（{} 根），原地退避重试",
                        jobs[0].envelope.dbnum,
                        jobs.len()
                    );
                }
                tokio::time::sleep(crate::surreal_retry::conflict_retry_backoff(attempt)).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    fn manifest(run_id: &str, roots: usize) -> RunManifest {
        RunManifest {
            version: FORMAT_VERSION,
            run_id: run_id.into(),
            dbnum: 7997,
            source_sesno: 1,
            source_token: String::new(),
            database_incarnation: String::new(),
            model_projection_empty_at_reservation: false,
            dependency_tokens: vec![],
            room_source_keys: vec![],
            room_context_fingerprint: String::new(),
            target_hash: String::new(),
            roots: (0..roots)
                .map(|n| RootIntent {
                    root: format!("24381/{n}"),
                    revision: 1,
                    token: String::new(),
                })
                .collect(),
            created_ms: 0,
        }
    }

    fn ids(runs: &[RunManifest]) -> Vec<&str> {
        runs.iter().map(|m| m.run_id.as_str()).collect()
    }

    #[test]
    fn admission_takes_the_largest_runs_first_and_only_fills_free_slots() {
        let pending = vec![
            manifest("small", 15),
            manifest("large", 39213),
            manifest("mid", 6769),
        ];
        assert_eq!(ids(&admit_runs(pending.clone(), 0, 2)), ["large", "mid"]);
        assert_eq!(ids(&admit_runs(pending.clone(), 1, 2)), ["large"]);
        assert!(admit_runs(pending.clone(), 2, 2).is_empty());
        assert!(admit_runs(pending.clone(), 5, 2).is_empty());
    }

    #[test]
    fn admission_limit_zero_means_unbounded_but_still_orders_by_size() {
        let pending = vec![manifest("b", 1), manifest("a", 1), manifest("c", 3)];
        assert_eq!(ids(&admit_runs(pending, 27, 0)), ["c", "a", "b"]);
    }

    #[test]
    fn only_optimistic_conflicts_are_retried_and_only_within_budget() {
        let conflict = "persist frozen root 24381/105154 chunk 1/1: publication statements failed: \
            statement 0: The query was not executed due to a failed transaction. Failed to commit \
            transaction due to a read or write conflict. This transaction can be retried";
        assert!(should_retry_publication_conflict(conflict, 1));
        assert!(should_retry_publication_conflict(
            conflict,
            PUBLISH_CONFLICT_ATTEMPTS - 1
        ));
        assert!(!should_retry_publication_conflict(
            conflict,
            PUBLISH_CONFLICT_ATTEMPTS
        ));
        assert!(!should_retry_publication_conflict(
            "publication statements failed: statement 0: Parse error",
            1
        ));
        assert!(!should_retry_publication_conflict(
            "model publication database request timed out; retain result and reconcile database ACK before replay",
            1
        ));
    }
}

fn shrink_batch_limit(r: &Runtime) {
    r.batch_successes.store(0, Ordering::Relaxed);
    let current = r
        .batch_limit
        .load(Ordering::Relaxed)
        .clamp(1, PUBLISH_BATCH_ROOTS);
    r.batch_limit.store((current / 2).max(1), Ordering::Relaxed);
}

fn note_batch_success(r: &Runtime) {
    let current = r
        .batch_limit
        .load(Ordering::Relaxed)
        .clamp(1, PUBLISH_BATCH_ROOTS);
    if current >= PUBLISH_BATCH_ROOTS {
        return;
    }
    if r.batch_successes.fetch_add(1, Ordering::Relaxed) + 1 >= PUBLISH_BATCH_REGROW_AFTER {
        r.batch_successes.store(0, Ordering::Relaxed);
        r.batch_limit
            .store((current * 2).min(PUBLISH_BATCH_ROOTS), Ordering::Relaxed);
    }
}

async fn database_reachable() -> bool {
    model_db_adapter::publication_request_with_timeout(
        async {
            SUL_DB.query("RETURN 1;").await?.check()?;
            Ok::<_, anyhow::Error>(())
        },
        AUTO_RECONCILE_PROBE_TIMEOUT,
    )
    .await
    .is_ok()
}

/// 连接 / 超时类错误后的自动对账。
///
/// 一次 30 s 超时并不说明事务没提交：库端可能已经落了 ACK，只是回执没送回来。旧口径是
/// 全部留给人工 `resume` + 逐根 `replay`，全量初始化里一次超时就让 3 万根等一夜
/// （2026-09-06 run-ad 现场：16:38 五根超时，此后 4 小时零进展）。这里做的仍是原来
/// 人工那两步，只是自动、有界：
/// 1. 等宽限期，再探库是否可达；探不到就保持暂停（人工路径不变）。
/// 2. 逐根查 `final_ack`：有 ACK 记 Committed（不重发 SQL）；确认没有 ACK 才重放，
///    每根最多 [`AUTO_RECONCILE_MAX_REPLAYS`] 次。
/// 3. 只有 UnknownHold 清空才恢复消费者；FailedHold（非连接类错误）仍不自动重试。
async fn auto_reconcile(r: &Runtime) {
    r.reconcile.rounds.fetch_add(1, Ordering::Relaxed);
    tokio::time::sleep(AUTO_RECONCILE_GRACE).await;
    let mut reachable = false;
    for _ in 0..AUTO_RECONCILE_PROBE_ROUNDS {
        if database_reachable().await {
            reachable = true;
            break;
        }
        tokio::time::sleep(AUTO_RECONCILE_PROBE_TIMEOUT).await;
    }
    if !reachable {
        r.reconcile.unreachable.fetch_add(1, Ordering::Relaxed);
        println!(
            "[模型发布队列] 自动对账：数据库 {}s 内不可达，消费者保持暂停，等待人工 resume",
            AUTO_RECONCILE_PROBE_TIMEOUT.as_secs() * (AUTO_RECONCILE_PROBE_ROUNDS as u64) * 2
        );
        return;
    }
    let held = r.journal.lock().unwrap().held();
    let total = held.len();
    let (mut acked, mut replayed, mut remaining) = (0usize, 0usize, 0usize);
    for id in held {
        let loaded = {
            let journal = r.journal.lock().unwrap();
            match (journal.load(&id), journal.load_prepared(&id)) {
                (Ok(envelope), Ok(Some(plan))) => Some((envelope, plan)),
                _ => None,
            }
        };
        let Some((envelope, plan)) = loaded else {
            remaining += 1;
            continue;
        };
        match final_ack(&envelope, &plan).await {
            Ok(true) => {
                if let Err(error) =
                    model_db_adapter::commit_prepared_frozen_projection(&envelope, &plan)
                {
                    report_error(
                        r,
                        &id,
                        envelope.dbnum,
                        &format!("auto reconcile: restore committed projection: {error:#}"),
                    );
                    remaining += 1;
                    continue;
                }
                finish_job(r, &envelope, TaskState::Committed, None);
                r.reconcile.attempts.lock().unwrap().remove(&id);
                r.reconcile.acked.fetch_add(1, Ordering::Relaxed);
                acked += 1;
            }
            Ok(false) => {
                let exhausted = {
                    let mut attempts = r.reconcile.attempts.lock().unwrap();
                    let entry = attempts.entry(id.clone()).or_default();
                    if *entry >= AUTO_RECONCILE_MAX_REPLAYS {
                        true
                    } else {
                        *entry += 1;
                        false
                    }
                };
                if exhausted {
                    r.reconcile.exhausted.fetch_add(1, Ordering::Relaxed);
                    remaining += 1;
                    continue;
                }
                let transition = r
                    .journal
                    .lock()
                    .unwrap()
                    .transition(&id, TaskState::Ready, None);
                match transition {
                    Ok(()) => {
                        r.reconcile.replayed.fetch_add(1, Ordering::Relaxed);
                        replayed += 1;
                    }
                    Err(error) => {
                        report_error(
                            r,
                            &id,
                            envelope.dbnum,
                            &format!("auto reconcile: replay transition: {error:#}"),
                        );
                        remaining += 1;
                    }
                }
            }
            Err(error) => {
                report_error(
                    r,
                    &id,
                    envelope.dbnum,
                    &format!("auto reconcile: ACK check: {error}"),
                );
                remaining += 1;
            }
        }
    }
    // 只解除连接 / 超时类的 hold。日志损坏、载荷读不出这类直接置位的暂停没有登记原因，
    // 或者原因不是连接类，都留给人工。
    let pause_reason = r.journal.lock().unwrap().pause_reason();
    let resumable = pause_reason.as_deref().is_some_and(is_connection_error);
    if remaining == 0 && !resumable {
        println!(
            "[模型发布队列] 自动对账：{total} 根不确定已全部收口，但当前暂停原因不是连接类（{}），不自动恢复",
            pause_reason.unwrap_or_else(|| "未登记".into())
        );
    } else if remaining == 0 {
        match resume_consumer().await {
            Ok(()) => println!(
                "[模型发布队列] 自动对账完成：{total} 根不确定，{acked} 根库端已有 ACK 记为已提交，{replayed} 根确认未提交已重放；消费者恢复，批量上限 {}",
                r.batch_limit.load(Ordering::Relaxed)
            ),
            Err(error) => report_error(
                r,
                "auto_reconcile",
                0,
                &format!("auto reconcile: resume consumer: {error:#}"),
            ),
        }
    } else {
        println!(
            "[模型发布队列] 自动对账未收口：{total} 根不确定，{acked} 根已提交，{replayed} 根已重放，{remaining} 根仍为 UnknownHold（重放次数用尽或对账失败）；消费者保持暂停，等待人工 resume / replay"
        );
    }
}

struct PreparedJob {
    envelope: Arc<FrozenRootEnvelope>,
    plan: PreparedRootPublication,
}

async fn consume_ready_jobs(r: &Runtime, ids: Vec<String>) {
    let mut group = Vec::<PreparedJob>::new();
    let mut group_bytes = 0usize;
    for id in ids {
        let envelope = match r.journal.lock().unwrap().load(&id) {
            Ok(envelope) => envelope,
            Err(error) => {
                report_error(r, &id, 0, &format!("payload: {error:#}"));
                r.paused.store(true, Ordering::Release);
                return;
            }
        };
        if envelope.projection.receipt.is_none() {
            finish_job(
                r,
                &envelope,
                TaskState::FailedHold,
                Some("generation did not produce a receipt".into()),
            );
            continue;
        }
        let plan = match prepare_journal_payload(r, &envelope).await {
            Ok(plan) => plan,
            Err(error) => {
                finish_error(r, &envelope, format!("{error:#}"));
                continue;
            }
        };
        let bytes = plan.chunks.iter().map(String::len).sum::<usize>();
        let batchable = plan.chunks.len() == 1 && bytes <= PUBLISH_BATCH_BYTES;
        if !batchable
            || (!group.is_empty() && group_bytes.saturating_add(bytes) > PUBLISH_BATCH_BYTES)
        {
            publish_prepared_group(r, std::mem::take(&mut group)).await;
            group_bytes = 0;
        }
        if batchable {
            group_bytes = group_bytes.saturating_add(bytes);
            group.push(PreparedJob { envelope, plan });
        } else {
            publish_prepared_group(r, vec![PreparedJob { envelope, plan }]).await;
        }
    }
    publish_prepared_group(r, group).await;
}

async fn publish_prepared_group(r: &Runtime, jobs: Vec<PreparedJob>) {
    if jobs.is_empty() {
        return;
    }
    // 组级持久化屏障：本组每根的 Prepared/Inflight 帧在这里一次落盘。拿不到 Ok 就一条
    // SQL 都不发——这些根留在 Inflight，重启后恢复成 UnknownHold 走 ACK 对账；此刻日志
    // 已拒绝一切追加，所以只报错并暂停，不再试图写终态。
    let started = Instant::now();
    let barrier = r.journal.lock().unwrap().barrier();
    record_elapsed(&r.timings.journal_begin, started);
    if let Err(error) = barrier {
        let message = format!("journal barrier before publication: {error:#}");
        for job in &jobs {
            report_error(r, &job.envelope.job_id, job.envelope.dbnum, &message);
        }
        r.paused.store(true, Ordering::Release);
        return;
    }
    let started = Instant::now();
    let result = publish_with_conflict_retry(r, &jobs).await;
    record_elapsed(&r.timings.publish, started);
    match result {
        Ok(()) => {
            for job in &jobs {
                finish_job(r, &job.envelope, TaskState::Committed, None);
            }
            note_batch_success(r);
        }
        Err(error) if jobs.len() == 1 => {
            finish_error(r, &jobs[0].envelope, format!("{error:#}"));
        }
        Err(error) => {
            let batch_error = format!("{error:#}");
            let connection_error = is_connection_error(&batch_error);
            if connection_error {
                shrink_batch_limit(r);
            }
            // 逐根归因：只有本根自己的事务抛的错才算本根的；传输 / 超时类错误没有
            // 语句索引，`attributed` 为 None，全批按连接类处理。
            let attributed = error.downcast_ref::<model_db_adapter::BatchPublicationError>();
            for (position, job) in jobs.iter().enumerate() {
                match final_ack(&job.envelope, &job.plan).await {
                    Ok(true) => {
                        if let Err(error) = model_db_adapter::commit_prepared_frozen_projection(
                            &job.envelope,
                            &job.plan,
                        ) {
                            finish_error(
                                r,
                                &job.envelope,
                                format!("restore committed projection: {error:#}"),
                            );
                        } else {
                            finish_job(r, &job.envelope, TaskState::Committed, None);
                        }
                    }
                    Ok(false) => {
                        let own = attributed
                            .and_then(|e| e.root_errors(position))
                            .map(|errors| errors.join("; "));
                        let (state, message) = classify_unacked_batch_root(
                            own.as_deref(),
                            connection_error,
                            &batch_error,
                        );
                        report_error(r, &job.envelope.job_id, job.envelope.dbnum, &message);
                        finish_job(r, &job.envelope, state, Some(message));
                    }
                    Err(ack_error) => {
                        let message = format!(
                            "batched publication result uncertain: {batch_error}; ACK check: {ack_error}"
                        );
                        report_error(r, &job.envelope.job_id, job.envelope.dbnum, &message);
                        finish_job(
                            r,
                            &job.envelope,
                            TaskState::UnknownHold,
                            Some(message.clone()),
                        );
                        pause_for_reconcile(r, &message);
                    }
                }
            }
            if connection_error {
                pause_for_reconcile(r, &batch_error);
            }
        }
    }
}

/// 批量请求失败、且本根没有 final ACK 时的终态，只认**归因到本根**的语句错误：
///
/// - 本根自己的事务抛了 `model_publish_superseded` → Superseded（终态，不重放）；
/// - 连接 / 超时类 → UnknownHold（交给自动对账）；
/// - 本根自己有别的错误 → FailedHold（可重放），消息就是它自己的错误；
/// - 本根没有任何归因错误（引擎在邻居失败后没执行它、或响应条数对不上无法归因）→
///   FailedHold，消息写明是邻居的失败连坐——**不**拿邻居的 superseded 给本根判终态。
///   ACK 与效果同事务，ACK=false 就是没提交，重放是安全的。
pub(super) fn classify_unacked_batch_root(
    own_errors: Option<&str>,
    connection_error: bool,
    batch_error: &str,
) -> (TaskState, String) {
    match own_errors {
        Some(own) if own.contains("model_publish_superseded") => {
            (TaskState::Superseded, own.to_owned())
        }
        _ if connection_error => (TaskState::UnknownHold, batch_error.to_owned()),
        Some(own) => (TaskState::FailedHold, own.to_owned()),
        None => (
            TaskState::FailedHold,
            format!(
                "batched publication: this root's transaction reported no statement error yet has no final ACK \
                 (a sibling root failed; not attributed to this root, replay is safe); batch error: {batch_error}"
            ),
        ),
    }
}

fn finish_error(r: &Runtime, envelope: &FrozenRootEnvelope, message: String) {
    let state = if message.contains("model_publish_superseded") {
        TaskState::Superseded
    } else if is_connection_error(&message) {
        TaskState::UnknownHold
    } else {
        TaskState::FailedHold
    };
    report_error(r, &envelope.job_id, envelope.dbnum, &message);
    if is_connection_error(&message) {
        shrink_batch_limit(r);
        pause_for_reconcile(r, &message);
    }
    finish_job(r, envelope, state, Some(message));
}

fn finish_job(r: &Runtime, envelope: &FrozenRootEnvelope, state: TaskState, error: Option<String>) {
    let started = Instant::now();
    let transition = if state == TaskState::Committed {
        r.journal
            .lock()
            .unwrap()
            .transition_after_database_commit(&envelope.job_id)
    } else {
        r.journal
            .lock()
            .unwrap()
            .transition(&envelope.job_id, state, error)
    };
    if let Err(error) = transition {
        report_error(
            r,
            &envelope.job_id,
            envelope.dbnum,
            &format!("journal ack: {error:#}"),
        );
        r.paused.store(true, Ordering::Release);
    }
    record_elapsed(&r.timings.journal_ack, started);
    if state != TaskState::Committed || !r.journal.lock().unwrap().finish_candidates().is_empty() {
        crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    }
}

async fn prepare_journal_payload(
    r: &Runtime,
    e: &FrozenRootEnvelope,
) -> anyhow::Result<PreparedRootPublication> {
    let started = Instant::now();
    e.validate()?;
    r.journal.lock().unwrap().verify_meshes(e)?;
    record_elapsed(&r.timings.validate_mesh, started);
    let started = Instant::now();
    let prepared = r.journal.lock().unwrap().load_prepared(&e.job_id)?;
    let (plan, is_new) = match prepared {
        Some(plan) => (plan, false),
        None => {
            let empty_baseline = r
                .journal
                .lock()
                .unwrap()
                .model_projection_was_empty(&e.run_id)
                .context("frozen envelope belongs to an unknown initialization run")?;
            // 消费侧再守一次：保留下来的旧清单若是在非空投影上登记的，这里以 FailedHold
            // 停下，而不是悄悄走 `skip_reconcile_query=false` 的逐根对账分支。
            require_empty_model_projection(Some(empty_baseline), e.dbnum, Some(&e.run_id))?;
            let (plan, profile) = model_db_adapter::publication_request(
                model_db_adapter::prepare_frozen_profiled_for_baseline_on(
                    PersistTarget::process(),
                    e,
                    empty_baseline,
                ),
            )
            .await?;
            r.timings
                .prepare_reconcile_read
                .fetch_add(profile.reconcile_read_micros, Ordering::Relaxed);
            r.timings
                .prepare_render_plan
                .fetch_add(profile.render_plan_micros, Ordering::Relaxed);
            r.timings
                .prepare_reconcile_queries
                .fetch_add(profile.reconcile_queries, Ordering::Relaxed);
            r.timings.prepared_plans.fetch_add(1, Ordering::Relaxed);
            (plan, true)
        }
    };
    r.timings
        .prepared_chunks
        .fetch_add(plan.chunks.len() as u64, Ordering::Relaxed);
    r.timings.prepared_sql_bytes.fetch_add(
        plan.chunks
            .iter()
            .map(|chunk| chunk.len() as u64)
            .sum::<u64>(),
        Ordering::Relaxed,
    );
    record_elapsed(&r.timings.prepare, started);
    // Persist both plan and write intent before issuing the first SQL mutation.
    // 落盘留给 `publish_prepared_group` 发 SQL 前的那一次组级屏障，本组 N 根共用。
    let started = Instant::now();
    r.journal
        .lock()
        .unwrap()
        .begin_publication_deferred(&e.job_id, is_new.then(|| plan.clone()))?;
    record_elapsed(&r.timings.journal_begin, started);
    Ok(plan)
}

/// 进程启动时 `auto_reconcile_pending` 的初值：持久化的暂停原因是连接 / 超时类，就带着
/// 「待对账」起来，消费者第一轮自己做 [`auto_reconcile`]（探活 → 逐根查 ACK → 重放 /
/// 恢复），而不是让一次 30 s 超时撞上重启就变成永久的人工暂停。其它原因（日志损坏、
/// 载荷读不出、生成失败）照旧只等人工 `resume`。重放次数是进程内计数，重启后从零数。
pub(super) fn auto_reconcile_pending_on_open(pause_reason: Option<&str>) -> bool {
    pause_reason.is_some_and(is_connection_error)
}

pub(super) fn is_connection_error(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    crate::surreal_retry::is_retryable_sul_db_transport_error(error)
        || [
            "connection",
            "disconnect",
            "websocket",
            "transport",
            "timed out",
            "channel closed",
        ]
        .iter()
        .any(|s| e.contains(s))
}
async fn final_ack(e: &FrozenRootEnvelope, plan: &PreparedRootPublication) -> Result<bool, String> {
    let hash = plan.hash().map_err(|e| e.to_string())?;
    let index = plan
        .chunks
        .len()
        .checked_sub(1)
        .ok_or("empty prepared plan")?;
    let sql = format!(
        "SELECT VALUE final FROM model_publish_ack:⟨{}_{index}⟩ WHERE run_id='{}' AND revision={} AND target_hash='{}' AND source_sesno={} AND plan_hash='{hash}' AND database_incarnation={} AND source_hash={} AND renderer_version=2 AND final=true;",
        e.job_id,
        e.run_id,
        e.intent.revision,
        e.target_hash,
        e.source_sesno,
        serde_json::to_string(&e.database_incarnation).unwrap(),
        serde_json::to_string(&digest(e.source_token.as_bytes())).unwrap()
    );
    let result = model_db_adapter::publication_request(async {
        Ok::<_, anyhow::Error>(
            SUL_DB
                .query(sql)
                .await?
                .check()?
                .take::<Vec<bool>>(0)?
                .first()
                == Some(&true),
        )
    })
    .await;
    result.map_err(|e| format!("{e:#}"))
}
pub async fn finish_ready_initializations(
    mgr: &crate::data_interface::tidb_manager::AiosDBManager,
) -> anyhow::Result<()> {
    let result = finish_ready_initializations_inner(mgr).await;
    if let Err(error) = &result {
        if let Some(r) = RUNTIME.get() {
            let message = format!("initialization finalization: {error:#}");
            report_error(r, "finalization", 0, &message);
            if is_connection_error(&message) {
                pause_for_reconcile(r, &message);
            } else if finalization_defers(error) {
                r.finalization_conflict_deferrals
                    .fetch_add(1, Ordering::Relaxed);
                println!(
                    "[model-publish] 收口撞上并发写者，推迟到下一轮重试（不暂停消费者）：{message}"
                );
            } else {
                pause_consumer(r, &message);
            }
        }
    }
    result
}
/// 收口这一轮的失败该「推迟到下一轮」而不是暂停消费者。收口每一步都幂等、由
/// `finish_candidates` 每轮重新驱动，所以撞上别的写者不必惊动人工：
///
/// - 乐观冲突：这一轮跟别的写者撞上了。以前这里跟语法错误一样直接暂停消费者等人工
///   `resume`——全 DESI 初始化就是这样停在 `consumer_paused` 的。
/// - [`model_db_adapter::ProjectionRestoreConflict`]：读库镜像那几秒里有「只写内存」的
///   按需生成往同一个库提交过，镜像已旧、没敢换。下一轮重读即可。
pub(super) fn finalization_defers(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<model_db_adapter::ProjectionRestoreConflict>()
        .is_some()
        || crate::surreal_retry::is_retryable_surreal_write_error(&format!("{error:#}"))
}

async fn finish_ready_initializations_inner(
    _mgr: &crate::data_interface::tidb_manager::AiosDBManager,
) -> anyhow::Result<()> {
    let Some(r) = RUNTIME.get() else {
        return Ok(());
    };
    if r.paused.load(Ordering::Acquire) {
        return Ok(());
    }
    let manifests = r.journal.lock().unwrap().finish_candidates();
    for m in manifests {
        // 收口分段计时：全部根落库之后到「文件轮次发布完成」这一段在 7997 单库上要 35 s，
        // 服务 CPU 不动、库端单核满跑；28 个库串起来就是十几分钟。哪一步慢要有数。
        let finalize_started = Instant::now();
        let mut step_started = Instant::now();
        let mut lap = |name: &str, laps: &mut Vec<String>| {
            laps.push(format!("{name}={}ms", step_started.elapsed().as_millis()));
            step_started = Instant::now();
        };
        let mut laps = Vec::<String>::new();
        for key in &m.room_source_keys {
            let stored = r.journal.lock().unwrap().room_source_stored(key);
            if !stored {
                let source = r.journal.lock().unwrap().load_room_source(key)?;
                crate::fast_model::room_topology_source::persist_on(&SUL_DB, &source).await?;
                r.journal.lock().unwrap().mark_room_source_stored(key)?;
            }
        }
        lap("room_sources", &mut laps);
        if !r.hydrated.lock().unwrap().contains(&m.run_id) {
            model_db_adapter::restore_projection_on(
                &SUL_DB,
                crate::fast_model::model_memory_store::ModelMemoryStore::global(),
                m.dbnum,
            )
            .await?;
            r.hydrated.lock().unwrap().insert(m.run_id.clone());
        }
        lap("restore_projection", &mut laps);
        let mut round =
            crate::data_interface::side_effect_pending::SideEffectCompensator::file_round(
                m.dbnum,
                m.source_sesno as i32,
            )
            .await?;
        if round.is_none() {
            // Older initialization manifests could be persisted before the
            // collecting side-effect row was introduced. Recreate that row
            // from the manifest token so restart recovery is idempotent.
            crate::data_interface::batch_worker::with_model_publication_window(
                m.dbnum,
                m.source_sesno as i32,
                &m.source_token,
                async {
                    let sql = crate::data_interface::side_effect_pending::SideEffectCompensator::render_spatial_reconcile_upsert(
                        m.dbnum,
                        m.source_sesno as i32,
                        &[],
                        &[],
                    )?;
                    SUL_DB.query(sql).await?.check()?;
                    anyhow::Ok(())
                },
            )
            .await?;
            round = crate::data_interface::side_effect_pending::SideEffectCompensator::file_round(
                m.dbnum,
                m.source_sesno as i32,
            )
            .await?;
        }
        let round = round.context("initialization file round missing")?;
        ensure!(
            round.commit_token.as_deref() == Some(m.source_token.as_str()),
            "initialization finalization source token mismatch"
        );
        if round.status == "collecting" {
            collect_spatial_fragments(&m).await?;
        }
        lap("spatial_fragments", &mut laps);
        let probe = crate::data_interface::model_update_pending::read_model_watermark_probe(
            &SUL_DB, m.dbnum,
        )
        .await?;
        lap("model_watermark", &mut laps);
        let round = crate::data_interface::side_effect_pending::SideEffectCompensator::file_round(
            m.dbnum,
            m.source_sesno as i32,
        )
        .await?
        .context("initialization file round disappeared")?;
        if round.status != "done"
            || !round.room_released
            || probe.recorded != m.source_sesno as i32
            || probe.supported() != m.source_sesno as i32
            || probe.abandoned != 0
        {
            println!(
                "[model-publish] dbnum={} 等待 Task 收尾（round={} rooms={} recorded={} supported={} abandoned={} 目标={}）：{}",
                m.dbnum,
                round.status,
                round.room_released,
                probe.recorded,
                probe.supported(),
                probe.abandoned,
                m.source_sesno,
                laps.join(" ")
            );
            continue;
        }
        let sql = format!(
            "RETURN count(SELECT id FROM model_update_pending WHERE dbnum={});\n\
            RETURN count(SELECT id FROM incr_side_effect_pending WHERE dbnum={} AND status IN ['collecting','pending','failed','dead']);",
            m.dbnum, m.dbnum
        );
        let mut result = SUL_DB.query(sql).await?.check()?;
        let tasks = result
            .take::<Option<usize>>(0)?
            .context("missing pending count")?;
        let effects = result
            .take::<Option<usize>>(1)?
            .context("missing side-effect count")?;
        lap("pending_counts", &mut laps);
        if tasks == 0 && effects == 0 {
            registry_on(&SUL_DB, &m, false, true).await?;
            r.journal.lock().unwrap().finish(&m.run_id)?;
            lap("registry_finish", &mut laps);
            println!(
                "[model-publish] dbnum={} 收口分段耗时 total={}ms {}",
                m.dbnum,
                finalize_started.elapsed().as_millis(),
                laps.join(" ")
            );
            // 翻面（plan 2026-09-06 §8 R5）：run 收口的这一刻起该库的模型取数源 = rocksdb。
            crate::data_interface::model_read_route::announce_source_changed(
                m.dbnum,
                crate::data_interface::model_read_route::ModelReadSource::Database,
            );
            // 收口即释放该库的生成输入缓存（`DbSet` + `SourceIndex`）：run 的缓存键此后
            // 不会再被任何服务命中，留着只是每库一份 attlib + 整库 owner 表的常驻。
            let rss_before = crate::data_interface::model_concurrency::process_rss_bytes();
            let dropped = crate::fast_model::e3d_model_service::forget_generation_input(m.dbnum);
            let rss_after = crate::data_interface::model_concurrency::process_rss_bytes();
            let mb = |bytes: Option<u64>| {
                bytes.map_or_else(
                    || "?".to_string(),
                    |b| format!("{:.1}", b as f64 / 1048576.0),
                )
            };
            println!(
                "[model-publish] dbnum={} run finished: dropped {dropped} generation input(s), rss {} MB -> {} MB",
                m.dbnum,
                mb(rss_before),
                mb(rss_after)
            );
        }
    }
    Ok(())
}
fn report_error(r: &Runtime, id: &str, dbnum: u32, error: &str) {
    *r.last_error.lock().unwrap() = Some(error.into());
    crate::data_interface::batch_failure_log::record_model_publish_error(id, dbnum, error);
}
fn pause_consumer(r: &Runtime, reason: &str) {
    r.paused.store(true, Ordering::Release);
    if let Err(error) = r.journal.lock().unwrap().set_pause(Some(reason.into())) {
        report_error(
            r,
            "journal",
            0,
            &format!("could not persist consumer hold: {error:#}"),
        );
    }
}
/// 连接 / 超时类原因的暂停：照旧持久化 hold，但让消费者在下一轮自己做 [`auto_reconcile`]。
fn pause_for_reconcile(r: &Runtime, reason: &str) {
    pause_consumer(r, reason);
    r.auto_reconcile_pending.store(true, Ordering::Release);
    r.wake.notify_one();
}
pub async fn resume_consumer() -> anyhow::Result<()> {
    let r = runtime()?;
    let manifests = r.journal.lock().unwrap().manifests();
    for m in manifests {
        validate_run_binding(&m).await?;
        if !r.journal.lock().unwrap().reserved(&m.run_id) {
            reserve_on(&SUL_DB, &m, false).await?;
            r.journal.lock().unwrap().mark_reserved(&m.run_id)?;
        }
        if !r.journal.lock().unwrap().active(&m.run_id) && source_is_applied(&m).await? {
            r.journal.lock().unwrap().activate(&m.run_id)?;
        }
    }
    r.journal.lock().unwrap().set_pause(None)?;
    *r.last_error.lock().unwrap() = None;
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    r.auto_reconcile_pending.store(false, Ordering::Release);
    r.paused.store(false, Ordering::Release);
    r.wake.notify_one();
    kick();
    Ok(())
}
pub fn replay_job(id: &str) -> anyhow::Result<()> {
    let r = runtime()?;
    let mut journal = r.journal.lock().unwrap();
    if journal.state(id) == Some(TaskState::GenerationHold) {
        let run = journal.load(id)?.run_id.clone();
        ensure!(
            !r.generating.lock().unwrap().contains(&run),
            "wait for this generation attempt to finish before replaying its failures"
        );
        journal.retry_generation(id)?;
        drop(journal);
        r.producing.lock().unwrap().remove(&run);
        kick();
        return Ok(());
    }
    ensure!(
        matches!(
            journal.state(id),
            Some(TaskState::FailedHold | TaskState::UnknownHold)
        ),
        "job is not held"
    );
    journal.transition(id, TaskState::Ready, None)?;
    drop(journal);
    r.wake.notify_one();
    Ok(())
}

/// Append one constant-size-in-backlog fragment in the root's final transaction.
/// Its sequence is allocated in that same transaction, so restart ordering is exact.
pub(crate) fn render_spatial_fragment(
    e: &FrozenRootEnvelope,
    refresh: &[String],
    removed: &[String],
) -> anyhow::Result<String> {
    let run = serde_json::to_string(&e.run_id)?;
    let refresh = serde_json::to_string(refresh)?;
    let removed = serde_json::to_string(removed)?;
    let hash = serde_json::to_string(&e.target_hash)?;
    Ok(format!(
        "UPSERT type::thing('model_publish_run',{run}) SET next_sequence=(next_sequence ?? 0)+1;\n\
        LET $fragment_sequence = (SELECT VALUE next_sequence FROM type::thing('model_publish_run',{run}))[0];\n\
        CREATE type::thing('model_publish_fragment',[{run},$fragment_sequence]) SET sequence=$fragment_sequence, \
        run_id={run}, target_hash={hash}, refresh={refresh}, removed={removed};\n"
    ))
}

async fn collect_spatial_fragments(m: &RunManifest) -> anyhow::Result<()> {
    use crate::data_interface::side_effect_pending::SideEffectCompensator;
    use std::collections::BTreeSet;
    #[derive(Deserialize)]
    struct Fragment {
        sequence: u64,
        target_hash: String,
        refresh: Vec<String>,
        removed: Vec<String>,
    }
    let run = serde_json::to_string(&m.run_id)?;
    let mut refresh = BTreeSet::new();
    let mut removed = BTreeSet::new();
    let mut cleared = BTreeSet::new();
    let mut sequence = 0u64;
    loop {
        let sql = format!(
            "SELECT sequence,target_hash,refresh,removed FROM model_publish_fragment:[{run},{}]..[{run},9223372036854775807] LIMIT 500;",
            sequence + 1
        );
        let mut rows: Vec<Fragment> = SUL_DB.query(sql).await?.check()?.take(0)?;
        if rows.is_empty() {
            break;
        }
        rows.sort_by_key(|r| r.sequence);
        for row in rows {
            ensure!(
                row.sequence == sequence + 1 && row.target_hash == m.target_hash,
                "initialization spatial fragment identity/gap mismatch"
            );
            sequence = row.sequence;
            for key in row.removed {
                refresh.remove(&key);
                cleared.insert(key.clone());
                removed.insert(key);
            }
            for key in row.refresh {
                removed.remove(&key);
                refresh.insert(key);
            }
        }
    }
    ensure!(
        sequence == m.roots.len() as u64,
        "initialization spatial fragments incomplete"
    );
    let refresh = refresh.into_iter().collect::<Vec<_>>();
    let removed = removed.into_iter().collect::<Vec<_>>();
    let cleared = cleared.into_iter().collect::<Vec<_>>();
    let sql = crate::data_interface::batch_worker::with_model_publication_window(
        m.dbnum,
        m.source_sesno as i32,
        &m.source_token,
        async {
            SideEffectCompensator::render_file_spatial_reconcile_upsert(
                m.dbnum,
                m.source_sesno as i32,
                &refresh,
                &removed,
                &cleared,
            )
        },
    )
    .await?;
    SUL_DB
        .query(format!("BEGIN TRANSACTION; {sql} COMMIT TRANSACTION;"))
        .await?
        .check()?;
    Ok(())
}
