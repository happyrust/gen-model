//! memory-routed 库的整库即时模型生成。
//!
//! 本入口与 durable 的 `model_rebuild` 分工固定：
//!
//! - 只在 [`super::model_read_route::ModelReadSource::Memory`] 路由下使用；
//! - 冻结一次设计文件身份/会话，并从这份冻结源枚举固定生成根集合；
//! - 只有 single-flight 准入的创建者会启动 worker；
//! - worker 整段运行在 `scope_memory_only`，不进数据批次队列、不推进水位；read-through
//!   仍向本进程 kv-mem 发布模型行、房间边与数据库回执；
//! - 任务进度与 roots API 以候选几何回执、记录数和（需要时）完整发布 ACK 共同判 ready。
//!
//! 任务行只保存数量和失败摘要。完整根集合在本模块的有界进程内快照中短期保留，
//! 供 `GET .../model/roots?task_id=...` 在运行中和终态后对账；服务重启后两者一起消失。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use indexmap::IndexMap;
use serde::Serialize;

use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::generation_root::{
    GenerationRoot, configured_delivery_unit_types, enumerate_generation_roots,
};
use crate::data_interface::model_read_route::ModelPublicationPolicy;
use crate::data_interface::model_refresh::{RootGenerationFailure, TargetedGenerationReport};
use crate::data_interface::task_registry::{
    DbnumModelEnsureAdmission, TaskEntry, TaskRegistry, TaskState,
};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::e3d_model_service::{E3dModelService, scan_index};
use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope, RootReceipt};

const MAX_TASK_SNAPSHOTS: usize = 32;
const TERMINAL_SNAPSHOT_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_FAILURE_SAMPLES: usize = 32;
const MAX_MISSING_SAMPLES: usize = 64;

#[derive(Debug, Clone)]
struct ExpectedRoot {
    root: GenerationRoot,
    min_source_sesno: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsurePurpose {
    OnDemand,
    StartupRoomBootstrap { room_generation: String },
    ForcedRebuild,
}

impl EnsurePurpose {
    fn as_str(&self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::StartupRoomBootstrap { .. } => "startup_room_bootstrap",
            Self::ForcedRebuild => "forced_rebuild",
        }
    }

    fn explicit_room_generation(&self) -> Option<&str> {
        match self {
            Self::StartupRoomBootstrap { room_generation } => Some(room_generation),
            _ => None,
        }
    }

    fn forces_geometry(&self) -> bool {
        matches!(self, Self::ForcedRebuild)
    }

    fn is_startup(&self) -> bool {
        matches!(self, Self::StartupRoomBootstrap { .. })
    }
}

#[derive(Debug)]
struct TaskSnapshot {
    task_id: Option<String>,
    dbnum: u32,
    source: FrozenSource,
    source_token_sha256: String,
    roots: Arc<[ExpectedRoot]>,
    purpose: EnsurePurpose,
    publication_policy: ModelPublicationPolicy,
    room_generation: Option<String>,
    created_at: Instant,
    terminal_at: Mutex<Option<Instant>>,
    completion: tokio::sync::Notify,
    progress: tokio::sync::watch::Sender<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbnumModelEnsureReceipt {
    pub task_id: String,
    pub dbnum: u32,
    pub expected_roots: usize,
    pub already_ready: usize,
    pub source_sesno: u32,
    pub state: String,
}

#[derive(Debug)]
pub struct DbnumModelEnsureRequiresMemory {
    pub dbnum: u32,
}

impl std::fmt::Display for DbnumModelEnsureRequiresMemory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "dbnum={} became database-routed before whole-dbnum generation started",
            self.dbnum
        )
    }
}

impl std::error::Error for DbnumModelEnsureRequiresMemory {}

#[derive(Debug)]
pub struct RoomBootstrapInProgress {
    pub status: String,
}

impl std::fmt::Display for RoomBootstrapInProgress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "room bootstrap is {}; complete model publication is not available",
            self.status
        )
    }
}

impl std::error::Error for RoomBootstrapInProgress {}

#[derive(Debug, Clone, Serialize)]
pub struct DbnumModelRootStatus {
    pub generation_root: String,
    pub noun: String,
    pub name: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbnumModelRootsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub dbnum: u32,
    pub source_sesno: u32,
    pub source_file: String,
    pub total: usize,
    pub ready_total: usize,
    pub roots: Vec<DbnumModelRootStatus>,
}

#[derive(Default)]
struct SnapshotRegistry {
    by_task: IndexMap<String, Arc<TaskSnapshot>>,
}

static SNAPSHOTS: OnceLock<Mutex<SnapshotRegistry>> = OnceLock::new();

fn snapshots() -> MutexGuard<'static, SnapshotRegistry> {
    SNAPSHOTS
        .get_or_init(|| Mutex::new(SnapshotRegistry::default()))
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("整库即时生成快照锁曾因 panic 中毒，已恢复继续使用");
            poisoned.into_inner()
        })
}

fn prune_snapshots(store: &mut SnapshotRegistry) {
    let now = Instant::now();
    let tasks = TaskRegistry::global();
    store.by_task.retain(|task_id, snapshot| {
        let Some(entry) = tasks.get(task_id) else {
            return false;
        };
        if !entry.state.is_terminal() {
            return true;
        }
        let terminal_at = *snapshot
            .terminal_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        now.saturating_duration_since(terminal_at.unwrap_or(snapshot.created_at))
            < TERMINAL_SNAPSHOT_TTL
    });
}

fn make_capacity(store: &mut SnapshotRegistry) -> anyhow::Result<()> {
    prune_snapshots(store);
    while store.by_task.len() >= MAX_TASK_SNAPSHOTS {
        let victim = store
            .by_task
            .keys()
            .find(|task_id| {
                TaskRegistry::global()
                    .get(task_id)
                    .is_none_or(|entry| entry.state.is_terminal())
            })
            .cloned();
        let Some(victim) = victim else {
            anyhow::bail!(
                "dbnum model ensure snapshot capacity exhausted: {MAX_TASK_SNAPSHOTS} active tasks"
            );
        };
        store.by_task.shift_remove(&victim);
    }
    Ok(())
}

async fn capture_snapshot(
    dbnum: u32,
    task_id: Option<String>,
    purpose: EnsurePurpose,
    publication_policy: ModelPublicationPolicy,
    room_generation: Option<String>,
) -> anyhow::Result<Arc<TaskSnapshot>> {
    let service = E3dModelService::from_current().await?;
    let (file, expected_type) = service.source_pin(dbnum)?;
    tokio::task::spawn_blocking(move || {
        let source = FrozenSource::capture(&file)
            .with_context(|| format!("freeze dbnum {dbnum} source {}", file.display()))?;
        anyhow::ensure!(
            source.dbnum() as u32 == dbnum,
            "frozen source {} identifies dbnum {}, requested {dbnum}",
            source.path().display(),
            source.dbnum()
        );
        anyhow::ensure!(
            source.db_type().eq_ignore_ascii_case(&expected_type),
            "frozen source {} type {} differs from MDB source type {}",
            source.path().display(),
            source.db_type(),
            expected_type
        );
        source.verify_path_identity()?;

        let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()?;
        let set = crate::data_interface::e3d_collector::open_pinned_set(
            &schema,
            source.path(),
            source.db_type(),
            source.target_sesno(),
        )?;
        let index = scan_index(source.path(), Some(source.target_sesno()))?;
        anyhow::ensure!(
            index.dbnum == dbnum,
            "frozen root index identifies dbnum {}, requested {dbnum}",
            index.dbnum
        );
        let mut roots =
            enumerate_generation_roots(&set, &index.roots, &configured_delivery_unit_types())?;
        source.verify_path_identity()?;

        // Shared traversal should already be unique; retain this guard so a malformed member
        // graph cannot silently change task totals and generation work into two different sets.
        let mut seen = HashSet::new();
        roots.retain(|root| seen.insert(root.root.to_pdms_str()));
        let source_sesno = source.target_sesno();
        let roots = roots
            .into_iter()
            .map(|root| ExpectedRoot {
                root,
                min_source_sesno: source_sesno,
            })
            .collect::<Vec<_>>();
        let token = source.commit_token_json()?;
        let (progress, _) = tokio::sync::watch::channel(0usize);
        Ok(Arc::new(TaskSnapshot {
            task_id,
            dbnum,
            source,
            source_token_sha256: crate::data_interface::digest::digest(token.as_bytes()),
            roots: roots.into(),
            purpose,
            publication_policy,
            room_generation,
            created_at: Instant::now(),
            terminal_at: Mutex::new(None),
            completion: tokio::sync::Notify::new(),
            progress,
        }))
    })
    .await
    .context("dbnum model root snapshot task failed")?
}

fn projection_key(root: &ExpectedRoot) -> Option<e3d_io::refno::RefNo> {
    crate::fast_model::e3d_model_service::parse_refno(&root.root.root.to_pdms_str()).ok()
}

fn receipt_satisfies_source(receipt: Option<&RootReceipt>, min_source_sesno: u32) -> bool {
    receipt.is_some_and(|receipt| receipt.source_sesno >= min_source_sesno)
}

fn root_ready(snapshot: &TaskSnapshot, root: &ExpectedRoot) -> bool {
    let Some(key) = projection_key(root) else {
        return false;
    };
    let store = ModelMemoryStore::global();
    let receipt = store.receipt(ProjectionScope::Current, key);
    let Some(receipt) =
        receipt.filter(|receipt| receipt_satisfies_source(Some(receipt), root.min_source_sesno))
    else {
        return false;
    };
    if store.records_of_root(ProjectionScope::Current, key).len() != receipt.geometry_count {
        return false;
    }
    let ModelPublicationPolicy::ProcessStore { durable } = snapshot.publication_policy else {
        return true;
    };
    let Some(ack) = store.publication_ack(ProjectionScope::Current, key) else {
        return false;
    };
    ack.dbnum == snapshot.dbnum
        && ack.source_sesno >= root.min_source_sesno
        && ack.geometry_count == receipt.geometry_count
        && ack.medium.is_durable() == durable
        && ack.room_membership == snapshot.room_generation.is_some()
        && ack.room_generation.as_deref() == snapshot.room_generation.as_deref()
}

fn ready_count(snapshot: &TaskSnapshot) -> usize {
    snapshot
        .roots
        .iter()
        .filter(|root| root_ready(snapshot, root))
        .count()
}

fn source_file_name(source: &FrozenSource) -> String {
    source
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<non-utf8>")
        .to_string()
}

fn detail(
    snapshot: &TaskSnapshot,
    ready: usize,
    failed: usize,
    already_ready: usize,
) -> serde_json::Value {
    serde_json::json!({
        "dbnum": snapshot.dbnum,
        "expected_roots": snapshot.roots.len(),
        "already_ready": already_ready,
        "ready_roots": ready,
        "room_ready_roots": if snapshot.room_generation.is_some() { ready } else { 0 },
        "failed_roots": failed,
        "durable": false,
        "purpose": snapshot.purpose.as_str(),
        "room_generation": snapshot.room_generation,
        "publication": {
            "process_store": snapshot.publication_policy.uses_process_store(),
            "durable": snapshot.publication_policy.durable(),
        },
        "frozen_source": {
            "file": source_file_name(&snapshot.source),
            "db_type": snapshot.source.db_type(),
            "source_sesno": snapshot.source.target_sesno(),
            "opened_len": snapshot.source.opened_len(),
            "token_sha256": snapshot.source_token_sha256.clone(),
        },
        "pipeline": "e3d-model generate_and_persist_roots",
    })
}

fn receipt_from_entry(entry: &TaskEntry) -> DbnumModelEnsureReceipt {
    DbnumModelEnsureReceipt {
        task_id: entry.task_id.clone(),
        dbnum: entry.dbnum.unwrap_or_default(),
        expected_roots: entry.total_units.unwrap_or_default() as usize,
        already_ready: entry
            .detail
            .as_ref()
            .and_then(|detail| detail.get("already_ready"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default() as usize,
        source_sesno: entry
            .detail
            .as_ref()
            .and_then(|detail| detail.pointer("/frozen_source/source_sesno"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default(),
        state: entry.state.as_str().to_string(),
    }
}

/// 起一个整库即时任务。根枚举先完成，因而 202 回执、任务分母与 roots 查询消费的是
/// 同一固定集合；并发同库请求只会让 `Created` 一方启动 worker。
pub async fn start(
    mgr: Arc<AiosDBManager>,
    project: &str,
    dbnum: u32,
) -> anyhow::Result<DbnumModelEnsureReceipt> {
    start_with_purpose(mgr, project, dbnum, EnsurePurpose::OnDemand).await
}

pub async fn start_forced(
    mgr: Arc<AiosDBManager>,
    project: &str,
    dbnum: u32,
) -> anyhow::Result<DbnumModelEnsureReceipt> {
    start_with_purpose(mgr, project, dbnum, EnsurePurpose::ForcedRebuild).await
}

async fn start_with_purpose(
    mgr: Arc<AiosDBManager>,
    project: &str,
    dbnum: u32,
    purpose: EnsurePurpose,
) -> anyhow::Result<DbnumModelEnsureReceipt> {
    let registry = TaskRegistry::global();
    if let Some(existing) = registry.active_dbnum_model_ensure(dbnum) {
        return Ok(receipt_from_entry(&existing));
    }
    if !purpose.is_startup()
        && crate::options::room_membership()
        && crate::options::data_face().is_read_through()
    {
        let status =
            crate::data_interface::initialization_phase::InitializationCoordinator::global()
                .snapshot()
                .room_bootstrap
                .status
                .to_string();
        if status != "ready" {
            return Err(RoomBootstrapInProgress { status }.into());
        }
    }

    let proposed_id = TaskRegistry::new_task_id(&format!("dbnum-model-ensure-{dbnum}"));
    let publication_policy = crate::data_interface::model_read_route::publication_policy(true);
    let current_room_generation = crate::fast_model::model_db_adapter::required_room_generation()?;
    let room_generation = match purpose.explicit_room_generation() {
        Some(expected) => {
            anyhow::ensure!(
                current_room_generation.as_deref() == Some(expected),
                "startup room generation changed before dbnum={dbnum} admission: expected {expected}, actual {current_room_generation:?}"
            );
            Some(expected.to_string())
        }
        None => current_room_generation,
    };
    let force_geometry = purpose.forces_geometry();
    let snapshot = capture_snapshot(
        dbnum,
        Some(proposed_id.clone()),
        purpose,
        publication_policy,
        room_generation,
    )
    .await?;
    if crate::data_interface::model_read_route::route_for(dbnum)
        .await?
        .is_database()
    {
        return Err(anyhow::Error::new(DbnumModelEnsureRequiresMemory { dbnum }));
    }
    let expected = u32::try_from(snapshot.roots.len())
        .context("dbnum generation-root count exceeds task progress range")?;
    if force_geometry {
        for root in snapshot.roots.iter().filter_map(projection_key) {
            ModelMemoryStore::global().invalidate_receipt(ProjectionScope::Current, root);
        }
    }
    let already_ready = ready_count(&snapshot);
    let initial_detail = detail(&snapshot, already_ready, 0, already_ready);

    // 这把锁同时是快照容量闸。所有本模块 start 都按 snapshot → TaskRegistry 的固定
    // 锁序进入；worker 只碰 TaskRegistry，不反向取快照锁。
    let created = {
        let mut store = snapshots();
        prune_snapshots(&mut store);
        if let Some(existing) = registry.active_dbnum_model_ensure(dbnum) {
            return Ok(receipt_from_entry(&existing));
        }
        make_capacity(&mut store)?;
        match registry.get_or_create_dbnum_model_ensure(
            &proposed_id,
            project,
            dbnum,
            snapshot.source.db_type(),
            expected,
            initial_detail,
        ) {
            DbnumModelEnsureAdmission::Existing(existing) => {
                return Ok(receipt_from_entry(&existing));
            }
            DbnumModelEnsureAdmission::Created(entry) => {
                store
                    .by_task
                    .insert(entry.task_id.clone(), snapshot.clone());
                entry
            }
        }
    };

    registry.set_units_done(&created.task_id, already_ready as u32);
    if snapshot.roots.is_empty() || already_ready == snapshot.roots.len() {
        finish_task(
            &created.task_id,
            &snapshot,
            already_ready,
            GenerationConclusion::default(),
        );
    } else {
        spawn_worker(mgr, created.task_id.clone(), snapshot, already_ready);
    }
    Ok(registry
        .get(&created.task_id)
        .as_ref()
        .map(receipt_from_entry)
        .unwrap_or_else(|| receipt_from_entry(&created)))
}

/// 启动主线的内部等待口：直接等任务快照上的 completion，不轮询 HTTP 或任务列表。
pub async fn start_and_wait(
    mgr: Arc<AiosDBManager>,
    project: &str,
    dbnum: u32,
    room_generation: String,
) -> anyhow::Result<TaskEntry> {
    let receipt = start_with_purpose(
        mgr,
        project,
        dbnum,
        EnsurePurpose::StartupRoomBootstrap { room_generation },
    )
    .await?;
    let snapshot = {
        let store = snapshots();
        store.by_task.get(&receipt.task_id).cloned()
    }
    .with_context(|| format!("startup room task {} has no snapshot", receipt.task_id))?;
    let mut progress = snapshot.progress.subscribe();
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    let base = initialization.snapshot().room_bootstrap;
    let wait_started = Instant::now();
    initialization
        .set_startup_room_totals(base.dbnums_total, base.roots_total + receipt.expected_roots);
    loop {
        let Some(entry) = TaskRegistry::global().get(&receipt.task_id) else {
            anyhow::bail!(
                "startup room task {} disappeared before completion",
                receipt.task_id
            );
        };
        if entry.state.is_terminal() {
            return Ok(entry);
        }
        tokio::select! {
            _ = snapshot.completion.notified() => {}
            changed = progress.changed() => {
                if changed.is_ok() {
                    initialization.record_startup_room_progress(
                        base.dbnums_done,
                        base.dbnums_failed,
                        base.roots_ready + *progress.borrow(),
                        base.roots_failed,
                        Some(dbnum),
                        "model_and_room_publication",
                        base.room_edges_written,
                        base.elapsed_ms + wait_started.elapsed().as_millis(),
                        base.rss_mib,
                    );
                }
            }
        }
    }
}

async fn progress_loop(
    task_id: String,
    snapshot: Arc<TaskSnapshot>,
    already_ready: usize,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let registry = TaskRegistry::global();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let ready = ready_count(&snapshot);
                registry.set_units_done(&task_id, ready as u32);
                registry.set_detail(&task_id, detail(&snapshot, ready, 0, already_ready));
                let _ = snapshot.progress.send(ready);
            }
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct GenerationConclusion {
    reported_completed: usize,
    failures: Vec<RootGenerationFailure>,
    top_error: Option<String>,
    room_edges_written: usize,
    rows_upserted: usize,
    commits: usize,
    max_transaction_bytes: usize,
}

fn conclude_generation(
    joined: Result<anyhow::Result<TargetedGenerationReport>, tokio::task::JoinError>,
) -> GenerationConclusion {
    match joined {
        Ok(Ok(report)) => GenerationConclusion {
            reported_completed: report.completed.len(),
            failures: report.failures,
            top_error: None,
            room_edges_written: report.room_edges_written,
            rows_upserted: report.rows_upserted,
            commits: report.commits,
            max_transaction_bytes: report.max_transaction_bytes,
        },
        Ok(Err(error)) => GenerationConclusion {
            top_error: Some(format!("{error:#}")),
            ..Default::default()
        },
        Err(error) => GenerationConclusion {
            top_error: Some(if error.is_panic() {
                format!("dbnum model generation task panicked: {error}")
            } else if error.is_cancelled() {
                format!("dbnum model generation task was cancelled: {error}")
            } else {
                format!("dbnum model generation task failed to join: {error}")
            }),
            ..Default::default()
        },
    }
}

fn spawn_worker(
    mgr: Arc<AiosDBManager>,
    task_id: String,
    snapshot: Arc<TaskSnapshot>,
    already_ready: usize,
) {
    tokio::spawn(async move {
        let pending = snapshot
            .roots
            .iter()
            .filter(|root| !root_ready(&snapshot, root))
            .map(|root| root.root.root.to_pdms_str())
            .collect::<Vec<_>>();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let ticker = tokio::spawn(progress_loop(
            task_id.clone(),
            snapshot.clone(),
            already_ready,
            stop_rx,
        ));

        let worker_mgr = mgr.clone();
        let worker_source = snapshot.source.clone();
        let worker_dbnum = snapshot.dbnum;
        let worker_room_generation = snapshot.room_generation.clone();
        let generation = tokio::spawn(async move {
            crate::data_interface::model_read_route::scope_memory_only(
                worker_dbnum,
                crate::data_interface::model_refresh::ModelRefreshPolicy::
                    generate_roots_report_from_frozen_source(
                        &worker_mgr,
                        &pending,
                        &worker_source,
                        worker_room_generation.as_deref(),
                    ),
            )
            .await
        });
        let conclusion = conclude_generation(generation.await);
        let _ = stop_tx.send(true);
        if let Err(error) = ticker.await {
            log::warn!("dbnum={worker_dbnum} 整库即时生成进度任务异常结束: {error}");
        }
        finish_task(&task_id, &snapshot, already_ready, conclusion);
    });
}

fn terminal_state(expected: usize, ready: usize, has_failure: bool) -> TaskState {
    if expected == 0 || (ready == expected && !has_failure) {
        TaskState::Succeeded
    } else if ready > 0 {
        TaskState::Partial
    } else {
        TaskState::Failed
    }
}

fn finish_task(
    task_id: &str,
    snapshot: &TaskSnapshot,
    already_ready: usize,
    conclusion: GenerationConclusion,
) {
    let registry = TaskRegistry::global();
    let ready = ready_count(snapshot);
    let missing = snapshot
        .roots
        .iter()
        .filter(|root| !root_ready(snapshot, root))
        .map(|root| root.root.root.to_pdms_str())
        .collect::<Vec<_>>();
    let primary_error = conclusion.top_error.clone().or_else(|| {
        conclusion
            .failures
            .first()
            .map(|failure| format!("{}: {}", failure.root, failure.error))
    });

    let mut failed_roots = HashSet::new();
    let mut failure_samples = Vec::new();
    for failure in &conclusion.failures {
        if failed_roots.insert(failure.root.clone()) && failure_samples.len() < MAX_FAILURE_SAMPLES
        {
            failure_samples.push(serde_json::json!({
                "generation_root": failure.root,
                "error": failure.error,
            }));
        }
    }
    for root in &missing {
        if failed_roots.insert(root.clone()) && failure_samples.len() < MAX_FAILURE_SAMPLES {
            failure_samples.push(serde_json::json!({
                "generation_root": root,
                "error": format!(
                    "projection receipt missing at required source session {}",
                    snapshot.source.target_sesno()
                ),
            }));
        }
    }
    if let Some(error) = conclusion.top_error.as_ref()
        && failure_samples.len() < MAX_FAILURE_SAMPLES
    {
        failure_samples.push(serde_json::json!({
            "generation_root": serde_json::Value::Null,
            "error": error,
        }));
    }
    let top_error_only = usize::from(conclusion.top_error.is_some() && failed_roots.is_empty());
    let failed = failed_roots.len() + top_error_only;
    let has_failure = failed > 0 || !missing.is_empty() || conclusion.top_error.is_some();
    let state = terminal_state(snapshot.roots.len(), ready, has_failure);

    registry.set_units_done(task_id, ready as u32);
    let _ = snapshot.progress.send(ready);
    registry.set_detail(task_id, detail(snapshot, ready, failed, already_ready));
    *snapshot
        .terminal_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    registry.finish(
        task_id,
        state,
        serde_json::json!({
            "expected": snapshot.roots.len(),
            "already_ready": already_ready,
            "newly_ready": ready.saturating_sub(already_ready),
            "ready": ready,
            "reported_completed": conclusion.reported_completed,
            "room_edges_written": conclusion.room_edges_written,
            "rows_upserted": conclusion.rows_upserted,
            "commits": conclusion.commits,
            "max_transaction_bytes": conclusion.max_transaction_bytes,
            "failed": failed,
            "missing": missing.len(),
            "missing_samples": missing.iter().take(MAX_MISSING_SAMPLES).collect::<Vec<_>>(),
            "failure_samples": failure_samples,
            "failure_samples_truncated": failed.saturating_sub(MAX_FAILURE_SAMPLES),
            "error": primary_error,
            "source_sesno": snapshot.source.target_sesno(),
            "durable": false,
        }),
    );
    snapshot.completion.notify_waiters();
}

fn roots_view(snapshot: &TaskSnapshot) -> DbnumModelRootsView {
    let roots = snapshot
        .roots
        .iter()
        .map(|root| DbnumModelRootStatus {
            generation_root: root.root.root.to_pdms_str(),
            noun: root.root.noun.clone(),
            name: root.root.name.clone(),
            ready: root_ready(snapshot, root),
        })
        .collect::<Vec<_>>();
    DbnumModelRootsView {
        task_id: snapshot.task_id.clone(),
        dbnum: snapshot.dbnum,
        source_sesno: snapshot.source.target_sesno(),
        source_file: source_file_name(&snapshot.source),
        total: roots.len(),
        ready_total: roots.iter().filter(|root| root.ready).count(),
        roots,
    }
}

/// 读取某个任务冻结的根集合。活动任务永不被容量剔除；终态快照保留 30 分钟或直到
/// 容量需要给更新的任务让位。
pub fn task_roots(task_id: &str, dbnum: u32) -> Option<DbnumModelRootsView> {
    let snapshot = {
        let mut store = snapshots();
        prune_snapshots(&mut store);
        store.by_task.get(task_id)?.clone()
    };
    (snapshot.dbnum == dbnum).then(|| roots_view(&snapshot))
}

/// 不带 `task_id` 的诊断形态：冻结当前文件一次并从同一会话枚举、判 ready。
/// 这份集合不能用于任何既有任务的进度对账。
pub async fn current_roots(dbnum: u32) -> anyhow::Result<DbnumModelRootsView> {
    let snapshot = capture_snapshot(
        dbnum,
        None,
        EnsurePurpose::OnDemand,
        crate::data_interface::model_read_route::publication_policy(true),
        crate::fast_model::model_db_adapter::required_room_generation()?,
    )
    .await?;
    Ok(roots_view(&snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_requires_a_receipt_not_older_than_the_frozen_source() {
        let old = RootReceipt {
            source_sesno: 41,
            model_target: None,
            manifest_hash: "old".into(),
            geometry_count: 0,
            published_via: "regen".into(),
        };
        let exact = RootReceipt {
            source_sesno: 42,
            ..old.clone()
        };
        let newer = RootReceipt {
            source_sesno: 43,
            ..old.clone()
        };
        assert!(!receipt_satisfies_source(None, 42));
        assert!(!receipt_satisfies_source(Some(&old), 42));
        assert!(receipt_satisfies_source(Some(&exact), 42));
        assert!(receipt_satisfies_source(Some(&newer), 42));
    }

    #[test]
    fn room_required_roots_need_a_complete_publication_ack_not_only_geometry() {
        let source = include_str!("model_dbnum_ensure.rs").replace("\r\n", "\n");
        let ready = source
            .split_once("fn root_ready(")
            .expect("root_ready")
            .1
            .split_once("fn ready_count(")
            .expect("ready_count follows root_ready")
            .0;
        assert!(
            ready.contains("publication_ack("),
            "a geometry receipt alone must not complete a room-enabled root"
        );
        assert!(
            ready.contains("room_generation"),
            "the publication ACK must belong to the resident generation frozen by the task"
        );
    }

    #[test]
    fn terminal_state_distinguishes_zero_full_partial_and_failed() {
        assert_eq!(terminal_state(0, 0, false), TaskState::Succeeded);
        assert_eq!(terminal_state(8, 8, false), TaskState::Succeeded);
        assert_eq!(terminal_state(8, 8, true), TaskState::Partial);
        assert_eq!(terminal_state(8, 3, false), TaskState::Partial);
        assert_eq!(terminal_state(8, 0, true), TaskState::Failed);
        assert_eq!(terminal_state(8, 0, false), TaskState::Failed);
    }

    #[test]
    fn root_enumeration_and_generation_share_one_frozen_source() {
        let source = include_str!("model_dbnum_ensure.rs");
        let capture = source
            .split_once("async fn capture_snapshot(")
            .expect("snapshot capture")
            .1
            .split_once("fn projection_key(")
            .expect("capture end")
            .0;
        assert!(capture.contains("FrozenSource::capture(&file)"));
        assert!(capture.contains("source.target_sesno()"));
        assert!(capture.contains("open_pinned_set("));
        assert!(capture.contains("enumerate_generation_roots("));
        assert!(
            capture.matches("source.verify_path_identity()?").count() >= 2,
            "the source identity must be checked before and after root enumeration"
        );

        let worker = source
            .split_once("fn spawn_worker(")
            .expect("worker")
            .1
            .split_once("fn terminal_state(")
            .expect("worker end")
            .0;
        assert!(worker.contains("worker_source = snapshot.source.clone()"));

        let start = source
            .split_once("pub async fn start(")
            .expect("start")
            .1
            .split_once("async fn progress_loop(")
            .expect("start end")
            .0;
        let capture = start.find("capture_snapshot(").expect("capture");
        let route_recheck = start
            .find("model_read_route::route_for(dbnum)")
            .expect("recheck");
        let admission = start
            .find("get_or_create_dbnum_model_ensure(")
            .expect("admission");
        assert!(
            capture < route_recheck && route_recheck < admission,
            "a Memory -> Database flip must be rejected before creating a task"
        );
    }

    #[tokio::test]
    async fn top_level_errors_and_panics_become_terminal_failure_evidence() {
        let error = conclude_generation(Ok(Err(anyhow::anyhow!("source changed"))));
        assert!(
            error
                .top_error
                .as_deref()
                .is_some_and(|message| message.contains("source changed"))
        );

        let joined = tokio::spawn(async {
            panic!("injected generation panic");
            #[allow(unreachable_code)]
            Ok::<TargetedGenerationReport, anyhow::Error>(TargetedGenerationReport::default())
        })
        .await;
        let panic = conclude_generation(joined);
        assert!(
            panic
                .top_error
                .as_deref()
                .is_some_and(|message| message.contains("panicked"))
        );

        let cancelled = tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok::<TargetedGenerationReport, anyhow::Error>(TargetedGenerationReport::default())
        });
        cancelled.abort();
        let cancelled = conclude_generation(cancelled.await);
        assert!(
            cancelled
                .top_error
                .as_deref()
                .is_some_and(|message| message.contains("cancelled"))
        );
    }

    #[test]
    fn worker_uses_one_frozen_pipeline_inside_the_memory_only_scope() {
        let source = include_str!("model_dbnum_ensure.rs");
        let worker = source
            .split_once("fn spawn_worker(")
            .expect("worker")
            .1
            .split_once("fn terminal_state(")
            .expect("terminal helper follows worker")
            .0;
        let scope = worker
            .find("scope_memory_only(")
            .expect("memory-only scope");
        let frozen = worker
            .find("generate_roots_report_from_frozen_source(")
            .expect("frozen pipeline");
        assert!(scope < frozen);
        assert_eq!(
            worker
                .matches("generate_roots_report_from_frozen_source(")
                .count(),
            1
        );
        assert!(!worker.contains("model_rebuild"));
        assert!(!worker.contains("batch_queue"));
        assert!(!worker.contains("model_watermark"));
    }
}
