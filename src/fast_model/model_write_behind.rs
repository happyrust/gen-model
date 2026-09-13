//! Process-local model write-behind state machine.
//!
//! The durable store is never read into this module. Root identities and rows
//! come only from [`ModelMemoryStore`]; RocksDB is an output and cold-read
//! baseline, never a cache-fill source.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use e3d_io::refno::RefNo;
use serde::Serialize;

use super::e3d_model_service::ModelTarget;
use super::model_db_adapter::{PersistTarget, frozen_room_calculator, persist_roots_batched};
use super::model_memory_store::{ModelMemoryStore, ProjectionScope, RootReceipt, packed_refno};

const ROOTS_PER_FLUSH: usize = 512;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const BLOCKED_SAMPLE_LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RootIdentity {
    source_sesno: u32,
    manifest_hash: String,
    geometry_count: usize,
    model_target: Option<ModelTarget>,
}

impl From<&RootReceipt> for RootIdentity {
    fn from(receipt: &RootReceipt) -> Self {
        Self {
            source_sesno: receipt.source_sesno,
            manifest_hash: receipt.manifest_hash.clone(),
            geometry_count: receipt.geometry_count,
            model_target: receipt.model_target.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingStatus {
    Pending,
    Persisting,
    Retryable,
    Blocked,
}

#[derive(Debug, Clone)]
struct PendingRoot {
    root: RefNo,
    identity: RootIdentity,
    pending_bytes: usize,
    attempts: u32,
    next_retry_at: Instant,
    status: PendingStatus,
    first_pending_at: Instant,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct Settlement {
    target_sesno: i32,
    target_time: Option<String>,
    attempts: u32,
    next_retry_at: Instant,
    last_error: Option<String>,
}

#[derive(Debug)]
struct State {
    accepting: bool,
    pending: BTreeMap<(u32, i64), PendingRoot>,
    settlements: BTreeMap<u32, Settlement>,
    superseded_submissions: u64,
    stale_jobs_dropped: u64,
    last_success_at: Option<String>,
    last_error: Option<String>,
    durable_target_by_dbnum: BTreeMap<u32, i32>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            accepting: true,
            pending: BTreeMap::new(),
            settlements: BTreeMap::new(),
            superseded_submissions: 0,
            stale_jobs_dropped: 0,
            last_success_at: None,
            last_error: None,
            durable_target_by_dbnum: BTreeMap::new(),
        }
    }
}

struct Controller {
    state: Mutex<State>,
    notify: tokio::sync::Notify,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Controller {
    fn global() -> &'static Self {
        static CONTROLLER: OnceLock<Controller> = OnceLock::new();
        CONTROLLER.get_or_init(|| Controller {
            state: Mutex::new(State::default()),
            notify: tokio::sync::Notify::new(),
            worker: Mutex::new(None),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_worker(&'static self) -> anyhow::Result<()> {
        let mut slot = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Ok(());
        }
        anyhow::ensure!(
            self.state().accepting,
            "model write-behind is shutting down"
        );
        *slot = Some(tokio::spawn(worker_loop(self)));
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BlockedRootSample {
    pub root: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SettlementSnapshot {
    pub dbnum: u32,
    pub memory_target: i32,
    pub durable_target: Option<i32>,
    pub state: &'static str,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WriteBehindSnapshot {
    pub accepting: bool,
    pub pending_unique_roots: usize,
    pub pending_bytes: usize,
    pub in_flight_roots: usize,
    pub retrying_roots: usize,
    pub blocked_roots: usize,
    pub superseded_submissions: u64,
    pub stale_jobs_dropped: u64,
    pub oldest_pending_ms: Option<u64>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub memory_target_by_dbnum: BTreeMap<u32, i32>,
    pub durable_target_by_dbnum: BTreeMap<u32, i32>,
    pub settlement_state_by_dbnum: BTreeMap<u32, &'static str>,
    pub settlements: Vec<SettlementSnapshot>,
    pub blocked_samples: Vec<BlockedRootSample>,
}

pub(crate) fn snapshot() -> WriteBehindSnapshot {
    let now = Instant::now();
    let state = Controller::global().state();
    let blocked_samples = state
        .pending
        .values()
        .filter(|root| root.status == PendingStatus::Blocked)
        .filter_map(|root| {
            Some(BlockedRootSample {
                root: root.root.to_string(),
                error: root.last_error.clone()?,
            })
        })
        .take(BLOCKED_SAMPLE_LIMIT)
        .collect();
    let settlements = state
        .settlements
        .iter()
        .map(|(dbnum, settlement)| SettlementSnapshot {
            dbnum: *dbnum,
            memory_target: settlement.target_sesno,
            durable_target: state.durable_target_by_dbnum.get(dbnum).copied(),
            state: if state
                .pending
                .values()
                .any(|root| root.root.dbno() == *dbnum && root.status == PendingStatus::Blocked)
            {
                "blocked"
            } else {
                "pending"
            },
            last_error: settlement.last_error.clone(),
        })
        .collect();
    let memory_target_by_dbnum = state
        .settlements
        .iter()
        .map(|(dbnum, settlement)| (*dbnum, settlement.target_sesno))
        .collect();
    let durable_target_by_dbnum = state.durable_target_by_dbnum.clone();
    let settlement_state_by_dbnum = state
        .settlements
        .keys()
        .chain(state.durable_target_by_dbnum.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|dbnum| {
            let status = if state
                .pending
                .values()
                .any(|root| root.root.dbno() == dbnum && root.status == PendingStatus::Blocked)
            {
                "blocked"
            } else if state.settlements.contains_key(&dbnum) {
                "pending"
            } else {
                "durable"
            };
            (dbnum, status)
        })
        .collect();
    WriteBehindSnapshot {
        accepting: state.accepting,
        pending_unique_roots: state.pending.len(),
        pending_bytes: state.pending.values().map(|root| root.pending_bytes).sum(),
        in_flight_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Persisting)
            .count(),
        retrying_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Retryable)
            .count(),
        blocked_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Blocked)
            .count(),
        superseded_submissions: state.superseded_submissions,
        stale_jobs_dropped: state.stale_jobs_dropped,
        oldest_pending_ms: state
            .pending
            .values()
            .map(|root| {
                now.saturating_duration_since(root.first_pending_at)
                    .as_millis() as u64
            })
            .max(),
        last_success_at: state.last_success_at.clone(),
        last_error: state.last_error.clone(),
        memory_target_by_dbnum,
        durable_target_by_dbnum,
        settlement_state_by_dbnum,
        settlements,
        blocked_samples,
    }
}

pub(crate) async fn submit_roots(dbnum: u32, roots: Vec<RefNo>) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::options::store_mode().is_durable(),
        "model write-behind requires a durable process store"
    );
    let controller = Controller::global();
    controller.ensure_worker()?;
    let store = ModelMemoryStore::global();
    let now = Instant::now();
    let mut state = controller.state();
    anyhow::ensure!(state.accepting, "model write-behind is shutting down");
    let roots = roots
        .into_iter()
        .map(|root| (packed_refno(root), root))
        .collect::<BTreeMap<_, _>>();
    for root in roots.into_values() {
        anyhow::ensure!(
            root.dbno() == dbnum,
            "root {root} does not belong to dbnum {dbnum}"
        );
        let Some(receipt) = store.receipt(ProjectionScope::Current, root) else {
            continue;
        };
        if !store.root_needs_persist(root) {
            continue;
        }
        let identity = RootIdentity::from(&receipt);
        let pending_bytes = store.resident_bytes_of_root(root);
        upsert_pending(&mut state, dbnum, root, identity, pending_bytes, now);
    }
    drop(state);
    controller.notify.notify_one();
    Ok(())
}

pub(crate) async fn request_settlement(
    dbnum: u32,
    target_sesno: i32,
    target_time: Option<String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::options::store_mode().is_durable(),
        "model watermark write-behind requires a durable process store"
    );
    let controller = Controller::global();
    controller.ensure_worker()?;
    reconcile_untracked_roots(controller, dbnum);
    let mut state = controller.state();
    anyhow::ensure!(state.accepting, "model write-behind is shutting down");
    let now = Instant::now();
    state
        .settlements
        .entry(dbnum)
        .and_modify(|current| {
            if target_sesno >= current.target_sesno {
                current.target_sesno = target_sesno;
                current.target_time = target_time.clone().or_else(|| current.target_time.clone());
            }
            current.next_retry_at = now;
        })
        .or_insert(Settlement {
            target_sesno,
            target_time,
            attempts: 0,
            next_retry_at: now,
            last_error: None,
        });
    drop(state);
    controller.notify.notify_one();
    Ok(())
}

pub(crate) fn shutdown_now() {
    let controller = Controller::global();
    controller.state().accepting = false;
    controller.notify.notify_waiters();
    if let Some(worker) = controller
        .worker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        worker.abort();
    }
}

async fn worker_loop(controller: &'static Controller) {
    loop {
        if !controller.state().accepting {
            return;
        }
        if let Some((dbnum, roots)) = take_due_roots(controller) {
            persist_batch(controller, dbnum, roots).await;
            continue;
        }
        if let Some((dbnum, settlement)) = take_due_settlement(controller) {
            settle(controller, dbnum, settlement).await;
            continue;
        }
        tokio::select! {
            _ = controller.notify.notified() => {},
            _ = tokio::time::sleep(POLL_INTERVAL) => {},
        }
    }
}

fn take_due_roots(controller: &Controller) -> Option<(u32, Vec<RefNo>)> {
    let now = Instant::now();
    let mut state = controller.state();
    let dbnum = state
        .pending
        .values()
        .find(|root| root.status != PendingStatus::Blocked && root.next_retry_at <= now)?
        .root
        .dbno();
    let roots = state
        .pending
        .values_mut()
        .filter(|root| {
            root.root.dbno() == dbnum
                && root.status != PendingStatus::Blocked
                && root.next_retry_at <= now
        })
        .take(ROOTS_PER_FLUSH)
        .map(|root| {
            root.status = PendingStatus::Persisting;
            root.root
        })
        .collect();
    Some((dbnum, roots))
}

async fn persist_batch(controller: &Controller, dbnum: u32, roots: Vec<RefNo>) {
    let store = ModelMemoryStore::global();
    let before = roots
        .iter()
        .filter_map(|root| {
            store
                .receipt(ProjectionScope::Current, *root)
                .map(|receipt| (packed_refno(*root), RootIdentity::from(&receipt)))
        })
        .collect::<BTreeMap<_, _>>();
    let outcome = async {
        let rooms = frozen_room_calculator()?;
        persist_roots_batched(
            PersistTarget::process(),
            store,
            dbnum,
            &roots,
            rooms.as_ref(),
        )
        .await
    }
    .await;
    let mut failures = BTreeMap::<i64, String>::new();
    let common_error = match outcome {
        Ok(report) => {
            for (root, error) in report.failed {
                failures.insert(packed_refno(root), error);
            }
            None
        }
        Err(error) => Some(format!("{error:#}")),
    };
    let now = Instant::now();
    let mut state = controller.state();
    for root in roots {
        let key = (dbnum, packed_refno(root));
        let current = store
            .receipt(ProjectionScope::Current, root)
            .as_ref()
            .map(RootIdentity::from);
        let Some(entry_identity) = state.pending.get(&key).map(|entry| entry.identity.clone())
        else {
            continue;
        };
        if current.as_ref() != before.get(&packed_refno(root))
            || current.as_ref() != Some(&entry_identity)
        {
            state.stale_jobs_dropped += 1;
            if let Some(receipt) = store.receipt(ProjectionScope::Current, root) {
                if let Some(entry) = state.pending.get_mut(&key) {
                    entry.identity = RootIdentity::from(&receipt);
                    entry.pending_bytes = store.resident_bytes_of_root(root);
                    entry.attempts = 0;
                    entry.status = PendingStatus::Pending;
                    entry.next_retry_at = now;
                    entry.last_error = None;
                }
            } else {
                state.pending.remove(&key);
            }
            continue;
        }
        if !store.root_needs_persist(root) {
            state.pending.remove(&key);
            state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
            continue;
        }
        let error = common_error
            .clone()
            .or_else(|| failures.remove(&packed_refno(root)))
            .unwrap_or_else(|| "durable ACK did not match the current root receipt".to_owned());
        let blocked = deterministic_failure(&error);
        if let Some(entry) = state.pending.get_mut(&key) {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.status = if blocked {
                PendingStatus::Blocked
            } else {
                PendingStatus::Retryable
            };
            entry.next_retry_at = if blocked {
                now + Duration::from_secs(365 * 24 * 3600)
            } else {
                now + retry_delay(entry.attempts)
            };
            entry.last_error = Some(error.clone());
        }
        state.last_error = Some(error);
    }
    controller.notify.notify_one();
}

fn take_due_settlement(controller: &Controller) -> Option<(u32, Settlement)> {
    let now = Instant::now();
    let state = controller.state();
    state
        .settlements
        .iter()
        .find(|(dbnum, settlement)| {
            settlement.next_retry_at <= now
                && !state
                    .pending
                    .values()
                    .any(|root| root.root.dbno() == **dbnum)
        })
        .map(|(dbnum, settlement)| (*dbnum, settlement.clone()))
}

async fn settle(controller: &Controller, dbnum: u32, settlement: Settlement) {
    reconcile_untracked_roots(controller, dbnum);
    if controller
        .state()
        .pending
        .values()
        .any(|root| root.root.dbno() == dbnum)
    {
        return;
    }
    let outcome = async {
        let _guard = super::e3d_model_service::lock_db_generation(dbnum).await;
        reconcile_untracked_roots(controller, dbnum);
        anyhow::ensure!(
            !controller
                .state()
                .pending
                .values()
                .any(|root| root.root.dbno() == dbnum),
            "new model roots appeared while settling"
        );
        let store = ModelMemoryStore::global();
        let epoch = store.commit_epoch(dbnum);
        let receipt = crate::data_interface::task_end::commit_task_end_on(
            &aios_core::SUL_DB,
            &crate::data_interface::task_end::TaskEnd {
                dbnum,
                target_sesno: settlement.target_sesno,
                target_time: settlement.target_time.clone(),
                model: Some(crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                }),
                info: None,
                closes_task: false,
            },
        )
        .await?;
        anyhow::ensure!(
            store.commit_epoch(dbnum) == epoch,
            "model commit epoch changed during settlement"
        );
        anyhow::ensure!(
            receipt.model_sesno == Some(settlement.target_sesno),
            "durable model watermark was blocked by the data watermark"
        );
        anyhow::Ok(())
    }
    .await;
    let mut state = controller.state();
    match outcome {
        Ok(()) => {
            state.settlements.remove(&dbnum);
            state
                .durable_target_by_dbnum
                .insert(dbnum, settlement.target_sesno);
            state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
        }
        Err(error) => {
            let text = format!("{error:#}");
            if let Some(current) = state.settlements.get_mut(&dbnum) {
                current.attempts = current.attempts.saturating_add(1);
                current.next_retry_at = Instant::now() + retry_delay(current.attempts);
                current.last_error = Some(text.clone());
            }
            state.last_error = Some(text);
        }
    }
}

fn reconcile_untracked_roots(controller: &Controller, dbnum: u32) {
    let store = ModelMemoryStore::global();
    let now = Instant::now();
    let roots = store.pending_roots_of_dbnum(dbnum);
    let live = roots
        .iter()
        .map(|root| packed_refno(*root))
        .collect::<std::collections::BTreeSet<_>>();
    let mut state = controller.state();
    state
        .pending
        .retain(|(candidate_dbnum, packed), _| *candidate_dbnum != dbnum || live.contains(packed));
    for root in roots {
        let Some(receipt) = store.receipt(ProjectionScope::Current, root) else {
            continue;
        };
        let identity = RootIdentity::from(&receipt);
        let pending_bytes = store.resident_bytes_of_root(root);
        upsert_pending(&mut state, dbnum, root, identity, pending_bytes, now);
    }
}

fn upsert_pending(
    state: &mut State,
    dbnum: u32,
    root: RefNo,
    identity: RootIdentity,
    pending_bytes: usize,
    now: Instant,
) {
    let key = (dbnum, packed_refno(root));
    match state.pending.get(&key) {
        Some(existing) if existing.identity == identity => {
            state.pending.get_mut(&key).unwrap().pending_bytes = pending_bytes;
            return;
        }
        Some(_) => state.superseded_submissions += 1,
        None => {}
    }
    state.pending.insert(
        key,
        PendingRoot {
            root,
            identity,
            pending_bytes,
            attempts: 0,
            next_retry_at: now,
            status: PendingStatus::Pending,
            first_pending_at: now,
            last_error: None,
        },
    );
}

fn retry_delay(attempts: u32) -> Duration {
    Duration::from_secs(1u64 << attempts.min(5))
}

fn deterministic_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "does not belong to dbnum",
        "projection incomplete",
        "exceeds",
        "serialize",
        "invalid root",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(manifest: &str) -> RootIdentity {
        RootIdentity {
            source_sesno: 42,
            manifest_hash: manifest.into(),
            geometry_count: 1,
            model_target: None,
        }
    }

    #[test]
    fn ten_thousand_submissions_of_one_root_coalesce_to_one_entry() {
        let mut state = State::default();
        let root = RefNo::new(7997, 100);
        let now = Instant::now();
        for bytes in 0..10_000 {
            upsert_pending(&mut state, 7997, root, identity("same"), bytes, now);
        }
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending.values().next().unwrap().pending_bytes, 9_999);
        assert_eq!(state.superseded_submissions, 0);
    }

    #[test]
    fn a_new_manifest_supersedes_and_unblocks_the_old_root_state() {
        let mut state = State::default();
        let root = RefNo::new(7997, 100);
        let now = Instant::now();
        upsert_pending(&mut state, 7997, root, identity("old"), 10, now);
        let entry = state.pending.values_mut().next().unwrap();
        entry.status = PendingStatus::Blocked;
        entry.attempts = 7;
        upsert_pending(&mut state, 7997, root, identity("new"), 20, now);
        let entry = state.pending.values().next().unwrap();
        assert_eq!(entry.identity.manifest_hash, "new");
        assert_eq!(entry.status, PendingStatus::Pending);
        assert_eq!(entry.attempts, 0);
        assert_eq!(state.superseded_submissions, 1);
    }

    #[test]
    fn deterministic_errors_block_while_database_errors_retry() {
        assert!(deterministic_failure("projection incomplete: missing mesh"));
        assert!(deterministic_failure("single root exceeds cache limit"));
        assert!(!deterministic_failure("transaction conflict"));
        assert!(!deterministic_failure("connection reset by peer"));
        assert_eq!(retry_delay(1), Duration::from_secs(2));
        assert_eq!(retry_delay(99), Duration::from_secs(32));
    }

    #[test]
    fn production_current_projection_writes_use_admission_and_generation_fence() {
        let generation = include_str!("e3d_model_service.rs");
        assert!(generation.contains("db_generation_lock(dbnum).lock_owned()"));
        assert!(generation.contains("try_commit_current(commits)?"));
        assert!(generation.contains("try_commit_current(vec![commit])?"));

        let deletion = include_str!("../data_interface/on_demand_model.rs")
            .split_once("#[cfg(test)]")
            .expect("on-demand tests")
            .0;
        assert!(deletion.contains("lock_db_generation(root.dbno())"));
        assert!(deletion.contains("try_commit_current"));
    }
}
