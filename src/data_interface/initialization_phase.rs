//! Strict data-initialization ordering (ADR-025).
//!
//! The queue itself is deliberately non-durable.  This coordinator therefore
//! keeps only reconstructible process state; watermarks and durable model work
//! remain the recovery authorities across restarts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use tokio::time::{Duration, sleep};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPhase {
    Meta,
    Catalogue,
    Design,
}

impl DataPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Catalogue => "catalogue",
            Self::Design => "design",
        }
    }

    pub fn of_db_type(db_type: &str) -> Self {
        match db_type.trim().to_ascii_uppercase().as_str() {
            "SYST" | "DICT" | "GLB" | "GLOB" => Self::Meta,
            "CATA" => Self::Catalogue,
            _ => Self::Design,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationStatus {
    Discovering,
    AwaitingTrigger,
    Running,
    Blocked,
    DataReady,
    ModelRunning,
    ModelReady,
}

impl InitializationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovering => "discovering",
            Self::AwaitingTrigger => "awaiting_trigger",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::DataReady => "data_ready",
            Self::ModelRunning => "model_running",
            Self::ModelReady => "model_ready",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PhaseCounts {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub failed: usize,
    pub blocked: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitializationSnapshot {
    pub epoch_id: u64,
    pub status: &'static str,
    pub current_phase: Option<&'static str>,
    pub data_ready: bool,
    pub model_ready: bool,
    pub model_in_flight: bool,
    pub phases: BTreeMap<&'static str, PhaseCounts>,
    pub blockers: Vec<String>,
    pub shadowed: Vec<String>,
}

#[derive(Debug, Clone)]
struct CoordinatorState {
    epoch_id: u64,
    status: InitializationStatus,
    current_phase: Option<DataPhase>,
    data_ready: bool,
    model_ready: bool,
    full_model_required: bool,
    model_phase_open: bool,
    model_in_flight: bool,
    phases: BTreeMap<DataPhase, PhaseCounts>,
    blockers: Vec<(DataPhase, String)>,
    shadowed: Vec<String>,
    needs_rescan: bool,
}

impl Default for CoordinatorState {
    fn default() -> Self {
        Self {
            epoch_id: 0,
            status: InitializationStatus::Discovering,
            current_phase: None,
            data_ready: false,
            model_ready: false,
            full_model_required: false,
            model_phase_open: true,
            model_in_flight: false,
            phases: BTreeMap::new(),
            blockers: Vec::new(),
            shadowed: Vec::new(),
            needs_rescan: false,
        }
    }
}

pub struct InitializationCoordinator {
    state: Mutex<CoordinatorState>,
}

static COORDINATOR: OnceLock<InitializationCoordinator> = OnceLock::new();

impl InitializationCoordinator {
    pub fn global() -> &'static Self {
        COORDINATOR.get_or_init(|| Self {
            state: Mutex::new(CoordinatorState::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::error!("初始化阶段锁曾因 panic 中毒，已恢复继续使用");
            poisoned.into_inner()
        })
    }

    /// Start a complete observation pass.  No model write is admitted until
    /// the resulting manifest is installed and all of its data rows settle.
    pub fn begin_discovery(&self) -> u64 {
        let mut state = self.state();
        state.epoch_id = state.epoch_id.saturating_add(1);
        state.status = InitializationStatus::Discovering;
        state.current_phase = None;
        state.data_ready = false;
        state.model_ready = false;
        state.model_phase_open = false;
        state.phases.clear();
        state.blockers.clear();
        state.shadowed.clear();
        state.needs_rescan = false;
        state.epoch_id
    }

    pub fn install_manifest(
        &self,
        epoch_id: u64,
        phases: impl IntoIterator<Item = DataPhase>,
        held: bool,
        blockers: impl IntoIterator<Item = (DataPhase, String)>,
    ) -> bool {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return false;
        }
        state.phases.clear();
        for phase in phases {
            let counts = state.phases.entry(phase).or_default();
            counts.total += 1;
            counts.pending += 1;
        }
        state.blockers = blockers.into_iter().collect();
        state.needs_rescan = false;
        let blocked_phases = state
            .blockers
            .iter()
            .map(|(phase, _)| *phase)
            .collect::<Vec<_>>();
        for phase in blocked_phases {
            state.phases.entry(phase).or_default().blocked += 1;
        }
        state.current_phase = state
            .blockers
            .iter()
            .map(|(phase, _)| *phase)
            .chain(state.phases.keys().copied())
            .min();
        state.data_ready = state.current_phase.is_none();
        state.model_ready = false;
        state.model_phase_open = state.data_ready && !state.full_model_required;
        let current_is_blocked = state
            .current_phase
            .is_some_and(|current| state.blockers.iter().any(|(phase, _)| *phase == current));
        state.status = if current_is_blocked {
            InitializationStatus::Blocked
        } else if state.data_ready {
            InitializationStatus::DataReady
        } else if held {
            InitializationStatus::AwaitingTrigger
        } else {
            InitializationStatus::Running
        };
        true
    }

    pub fn arm(&self) {
        let mut state = self.state();
        if state.status == InitializationStatus::AwaitingTrigger {
            state.status = InitializationStatus::Running;
        }
    }

    pub fn allows(&self, phase: DataPhase, epoch_id: u64) -> bool {
        // Legacy/direct test callers that do not participate in an installed
        // manifest remain executable; production discovery always uses a
        // positive epoch.
        if epoch_id == 0 {
            return true;
        }
        let state = self.state();
        epoch_id == state.epoch_id
            && state.status == InitializationStatus::Running
            && state.current_phase == Some(phase)
            && !state.model_in_flight
            && !state
                .blockers
                .iter()
                .any(|(blocked_phase, _)| *blocked_phase == phase)
    }

    /// Recompute the active barrier from the queue after a row settles.  The
    /// caller supplies only rows from the current manifest.
    pub fn reconcile_pending(
        &self,
        epoch_id: u64,
        pending: impl IntoIterator<Item = (DataPhase, bool)>,
    ) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        for counts in state.phases.values_mut() {
            counts.pending = 0;
            counts.running = 0;
        }
        for (phase, running) in pending {
            let counts = state.phases.entry(phase).or_default();
            if running {
                counts.running += 1;
            } else {
                counts.pending += 1;
            }
        }
        let previous_phase = state.current_phase;
        state.current_phase = state
            .phases
            .iter()
            .find_map(|(phase, counts)| ((counts.pending + counts.running) > 0).then_some(*phase));
        state.current_phase = state
            .current_phase
            .into_iter()
            .chain(state.blockers.iter().map(|(phase, _)| *phase))
            .min();
        if state.current_phase != previous_phase {
            // A barrier transition is not complete merely because the current
            // in-memory rows drained.  The authoritative file snapshot must be
            // rebuilt before a later phase (or models) can run.
            state.status = InitializationStatus::Discovering;
            state.data_ready = false;
            state.needs_rescan = true;
        } else if state
            .current_phase
            .is_some_and(|current| state.blockers.iter().any(|(phase, _)| *phase == current))
        {
            state.status = InitializationStatus::Blocked;
            state.data_ready = false;
        }
    }

    pub fn take_rescan_requested(&self) -> bool {
        let mut state = self.state();
        std::mem::take(&mut state.needs_rescan)
    }

    pub fn mark_failed(&self, epoch_id: u64, phase: DataPhase, message: String) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        state.status = InitializationStatus::Blocked;
        state.current_phase = Some(phase);
        state.data_ready = false;
        state.model_ready = false;
        state.phases.entry(phase).or_default().failed += 1;
        state.blockers.push((phase, message));
    }

    pub fn set_shadowed(&self, epoch_id: u64, shadowed: Vec<String>) {
        let mut state = self.state();
        if epoch_id == state.epoch_id {
            state.shadowed = shadowed;
        }
    }

    pub fn set_phase_totals(&self, epoch_id: u64, phases: impl IntoIterator<Item = DataPhase>) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        let mut totals = BTreeMap::new();
        for phase in phases {
            *totals.entry(phase).or_insert(0usize) += 1;
        }
        for (phase, total) in totals {
            state.phases.entry(phase).or_default().total = total;
        }
    }

    pub fn data_ready(&self) -> bool {
        self.state().data_ready
    }

    /// Configure whether startup must finish a configured full-model pass
    /// before durable/on-demand model generation may begin.  This is process
    /// policy, so it survives manifest replacement within the same startup.
    pub fn configure_model_bootstrap(&self, required: bool) {
        let mut state = self.state();
        state.full_model_required = required;
        state.model_phase_open = state.data_ready && !required;
        state.model_ready = false;
    }

    /// Full/manual synchronization has already awaited its three global data
    /// phases before the incremental manager is created, so it has no manifest
    /// rows to reconstruct.  Record that fact explicitly instead of relying on
    /// the legacy epoch-zero bypass for model writes.
    pub fn mark_data_ready_without_manifest(&self) {
        let mut state = self.state();
        if state.epoch_id == 0 {
            state.data_ready = true;
            state.status = InitializationStatus::DataReady;
            state.model_phase_open = !state.full_model_required;
        }
    }

    /// Wait for the installed data manifest to settle.  A blocked phase stays
    /// observable through health while the watcher is alive; a later complete
    /// scan installs a new epoch and releases this wait.
    pub async fn wait_for_data_ready(&self) {
        loop {
            if self.data_ready() {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn wait_for_model_ready(&self) {
        loop {
            if self.state().model_ready {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    /// Release durable model work after the configured startup-wide full model
    /// pass.  For deployments without that pass, the manifest opens this gate
    /// automatically as soon as all data phases are ready.
    pub fn open_model_phase(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || state.model_in_flight {
            return false;
        }
        state.model_phase_open = true;
        state.status = InitializationStatus::ModelRunning;
        true
    }

    pub fn begin_full_model(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || state.model_in_flight {
            return false;
        }
        state.model_in_flight = true;
        state.status = InitializationStatus::ModelRunning;
        state.model_ready = false;
        true
    }

    pub fn end_full_model(&self) {
        let mut state = self.state();
        state.model_in_flight = false;
        if state.data_ready {
            state.status = InitializationStatus::DataReady;
        }
    }

    pub fn model_generation_allowed(&self) -> bool {
        let state = self.state();
        !state.model_in_flight
            && ((state.epoch_id == 0 && !state.full_model_required)
                || (state.data_ready && state.model_phase_open))
    }

    /// Room/spatial post-processing is one barrier later than model writes.
    /// Epoch zero preserves the standalone Python/full-sync compatibility path,
    /// which does not install an incremental manifest.
    pub fn postprocess_allowed(&self) -> bool {
        let state = self.state();
        state.epoch_id == 0 || (state.data_ready && state.model_ready)
    }

    pub fn mark_model_running(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || !state.model_phase_open {
            return false;
        }
        state.status = InitializationStatus::ModelRunning;
        state.model_ready = false;
        true
    }

    pub fn mark_model_ready(&self) -> bool {
        let mut state = self.state();
        if state.data_ready && state.model_phase_open {
            let changed = !state.model_ready;
            state.status = InitializationStatus::ModelReady;
            state.model_ready = true;
            changed
        } else {
            false
        }
    }

    pub fn snapshot(&self) -> InitializationSnapshot {
        let state = self.state();
        InitializationSnapshot {
            epoch_id: state.epoch_id,
            status: state.status.as_str(),
            current_phase: state.current_phase.map(DataPhase::as_str),
            data_ready: state.data_ready,
            model_ready: state.model_ready,
            model_in_flight: state.model_in_flight,
            phases: state
                .phases
                .iter()
                .map(|(phase, counts)| (phase.as_str(), counts.clone()))
                .collect(),
            blockers: state
                .blockers
                .iter()
                .map(|(phase, message)| format!("{}: {message}", phase.as_str()))
                .collect(),
            shadowed: state.shadowed.clone(),
        }
    }
}

/// Typed model-write rejection shared by REST, Python and background drains.
#[derive(Debug, Clone)]
pub struct InitializationNotReady {
    pub snapshot: InitializationSnapshot,
}

impl std::fmt::Display for InitializationNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "initialization_not_ready: status={} epoch={} current_phase={} blockers={}",
            self.snapshot.status,
            self.snapshot.epoch_id,
            self.snapshot.current_phase.unwrap_or("none"),
            self.snapshot.blockers.join(" | ")
        )
    }
}

impl std::error::Error for InitializationNotReady {}

pub fn require_model_generation() -> Result<(), InitializationNotReady> {
    let coordinator = InitializationCoordinator::global();
    if coordinator.model_generation_allowed() {
        Ok(())
    } else {
        Err(InitializationNotReady {
            snapshot: coordinator.snapshot(),
        })
    }
}

pub fn require_postprocess() -> Result<(), InitializationNotReady> {
    let coordinator = InitializationCoordinator::global();
    if coordinator.postprocess_allowed() {
        Ok(())
    } else {
        Err(InitializationNotReady {
            snapshot: coordinator.snapshot(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueCandidate {
    pub project: String,
    pub dbnum: u32,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueSelection {
    pub selected: Vec<CatalogueCandidate>,
    pub shadowed: Vec<CatalogueCandidate>,
    pub blockers: Vec<String>,
}

/// Resolve the repository's naked-dbnum limitation before any observation is
/// written.  Same-project duplicates always block; cross-project collisions
/// need an explicit project priority winner.
pub fn select_catalogue_candidates(
    candidates: impl IntoIterator<Item = CatalogueCandidate>,
    included_projects: &[String],
    priority: &[String],
) -> CatalogueSelection {
    let included: HashSet<String> = included_projects
        .iter()
        .map(|project| project.trim().to_ascii_lowercase())
        .collect();
    let mut rank = HashMap::new();
    let mut blockers = Vec::new();
    for (index, project) in priority.iter().enumerate() {
        let key = project.trim().to_ascii_lowercase();
        if key.is_empty() || !included.contains(&key) {
            blockers.push(format!("catalogue_project_priority 含未知项目 {project:?}"));
            continue;
        }
        if rank.insert(key.clone(), index).is_some() {
            blockers.push(format!("catalogue_project_priority 重复项目 {project:?}"));
        }
    }

    let mut by_dbnum: BTreeMap<u32, Vec<CatalogueCandidate>> = BTreeMap::new();
    let mut seen_project_dbnum: HashMap<(String, u32), PathBuf> = HashMap::new();
    let mut extract_shadowed = Vec::new();
    let included_candidates: Vec<CatalogueCandidate> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let project_key = candidate.project.trim().to_ascii_lowercase();
            if !included.contains(&project_key) {
                blockers.push(format!(
                    "元件库候选归属项目不在 included_projects: {} dbnum={}",
                    candidate.project, candidate.dbnum
                ));
                return None;
            }
            Some(candidate)
        })
        .collect();
    let collapsed = crate::data_interface::extract_family::collapse_extract_families(
        included_candidates.iter().map(|candidate| {
            (
                candidate.project.clone(),
                candidate.dbnum,
                candidate.path.clone(),
            )
        }),
    );
    for mismatch in &collapsed.mismatches {
        blockers.push(format!(
            "CATA/DICT 文件名库号与文件头不一致: {} filename={} header={}",
            mismatch.path.display(),
            mismatch.filename_dbnum,
            mismatch.header_dbnum
        ));
    }
    let mut by_path: HashMap<PathBuf, CatalogueCandidate> = included_candidates
        .into_iter()
        .map(|candidate| (candidate.path.clone(), candidate))
        .collect();
    for parent in &collapsed.shadowed_parents {
        if let Some(candidate) = by_path.remove(parent) {
            extract_shadowed.push(candidate);
        }
    }
    for key in &collapsed.duplicate_keys {
        let paths: Vec<String> = by_path
            .values()
            .filter(|candidate| {
                candidate.dbnum == key.1 && candidate.project.trim().eq_ignore_ascii_case(&key.0)
            })
            .map(|candidate| candidate.path.display().to_string())
            .collect();
        blockers.push(format!(
            "项目 {} 内 CATA/DICT dbnum={} 有多个抽取或副本: {}",
            key.0,
            key.1,
            paths.join(" / ")
        ));
    }
    for sel in collapsed.selected {
        if collapsed
            .duplicate_keys
            .contains(&(sel.project.clone(), sel.dbnum))
        {
            continue;
        }
        let Some(candidate) = by_path.remove(&sel.leaf_path) else {
            continue;
        };
        let project_key = candidate.project.trim().to_ascii_lowercase();
        if let Some(previous) =
            seen_project_dbnum.insert((project_key, candidate.dbnum), candidate.path.clone())
        {
            blockers.push(format!(
                "项目 {} 内 CATA/DICT dbnum={} 有多个文件: {} / {}",
                candidate.project,
                candidate.dbnum,
                previous.display(),
                candidate.path.display()
            ));
        }
        by_dbnum.entry(candidate.dbnum).or_default().push(candidate);
    }

    let mut selected = Vec::new();
    let mut shadowed = Vec::new();
    for (dbnum, mut group) in by_dbnum {
        if group.len() == 1 {
            selected.push(group.pop().expect("one candidate"));
            continue;
        }
        group.sort_by_key(|candidate| {
            rank.get(&candidate.project.trim().to_ascii_lowercase())
                .copied()
                .unwrap_or(usize::MAX)
        });
        let Some(winner_rank) = rank
            .get(&group[0].project.trim().to_ascii_lowercase())
            .copied()
        else {
            blockers.push(format!(
                "跨项目 CATA/DICT dbnum={dbnum} 冲突且没有 catalogue_project_priority 选主"
            ));
            continue;
        };
        let tied = group.iter().filter(|candidate| {
            rank.get(&candidate.project.trim().to_ascii_lowercase())
                .copied()
                == Some(winner_rank)
        });
        if tied.count() != 1 {
            blockers.push(format!("跨项目 CATA/DICT dbnum={dbnum} 优先级仍有歧义"));
            continue;
        }
        selected.push(group.remove(0));
        shadowed.extend(group);
    }
    shadowed.extend(extract_shadowed);
    selected.sort_by_key(|candidate| candidate.dbnum);
    shadowed.sort_by_key(|candidate| candidate.dbnum);
    CatalogueSelection {
        selected,
        shadowed,
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinator() -> InitializationCoordinator {
        InitializationCoordinator {
            state: Mutex::new(CoordinatorState::default()),
        }
    }

    fn candidate(project: &str, dbnum: u32, path: &str) -> CatalogueCandidate {
        CatalogueCandidate {
            project: project.into(),
            dbnum,
            path: path.into(),
        }
    }

    #[test]
    fn phases_are_dependency_ordered() {
        assert!(DataPhase::Meta < DataPhase::Catalogue);
        assert!(DataPhase::Catalogue < DataPhase::Design);
        assert_eq!(DataPhase::of_db_type("DICT"), DataPhase::Meta);
        assert_eq!(DataPhase::of_db_type("CATA"), DataPhase::Catalogue);
        assert_eq!(DataPhase::of_db_type("DESI"), DataPhase::Design);
    }

    #[test]
    fn old_epoch_cannot_satisfy_new_manifest() {
        let coordinator = coordinator();
        let old = coordinator.begin_discovery();
        coordinator.install_manifest(old, [DataPhase::Meta], false, []);
        let new = coordinator.begin_discovery();
        coordinator.install_manifest(new, [DataPhase::Catalogue], false, []);
        coordinator.reconcile_pending(old, []);
        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.epoch_id, new);
        assert_eq!(snapshot.current_phase, Some("catalogue"));
        assert!(!snapshot.data_ready);
    }

    #[test]
    fn a_blocked_meta_phase_never_admits_design() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Design],
            false,
            [(DataPhase::Meta, "DICT unreadable".into())],
        );
        assert!(!coordinator.allows(DataPhase::Design, epoch));
        assert_eq!(coordinator.snapshot().current_phase, Some("meta"));
    }

    #[test]
    fn a_catalogue_blocker_does_not_prevent_meta_from_settling_first() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Meta, DataPhase::Catalogue, DataPhase::Design],
            false,
            [(DataPhase::Catalogue, "duplicate CATA".into())],
        );
        assert!(coordinator.allows(DataPhase::Meta, epoch));
        assert!(!coordinator.allows(DataPhase::Catalogue, epoch));
        assert!(!coordinator.allows(DataPhase::Design, epoch));
    }

    #[test]
    fn one_real_trigger_arms_the_whole_manifest() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Meta, DataPhase::Catalogue, DataPhase::Design],
            true,
            [],
        );
        assert!(!coordinator.allows(DataPhase::Meta, epoch));
        coordinator.arm();
        assert!(coordinator.allows(DataPhase::Meta, epoch));
    }

    #[test]
    fn configured_full_model_keeps_all_model_writes_closed_until_completion() {
        let coordinator = coordinator();
        coordinator.configure_model_bootstrap(true);
        coordinator.mark_data_ready_without_manifest();
        assert!(coordinator.data_ready());
        assert!(!coordinator.model_generation_allowed());
        assert!(!coordinator.mark_model_ready());
        assert!(!coordinator.snapshot().model_ready);

        assert!(coordinator.open_model_phase());
        assert!(coordinator.model_generation_allowed());
        assert!(coordinator.mark_model_ready());
        assert!(coordinator.snapshot().model_ready);
    }

    #[test]
    fn postprocessing_waits_for_model_settlement() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [], false, []);
        assert!(coordinator.data_ready());
        assert!(coordinator.model_generation_allowed());
        assert!(!coordinator.postprocess_allowed());

        assert!(coordinator.mark_model_ready());
        assert!(coordinator.postprocess_allowed());
    }

    #[test]
    fn earlier_data_epoch_waits_for_an_in_flight_full_model_to_settle() {
        let coordinator = coordinator();
        coordinator.mark_data_ready_without_manifest();
        assert!(coordinator.begin_full_model());
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [DataPhase::Meta], false, []);
        assert!(!coordinator.allows(DataPhase::Meta, epoch));
        coordinator.end_full_model();
        assert!(coordinator.allows(DataPhase::Meta, epoch));
    }

    #[test]
    fn cross_project_collision_uses_explicit_priority() {
        let result = select_catalogue_candidates(
            [
                candidate("Main", 7000, "main7000"),
                candidate("Catalogue", 7000, "cat7000"),
                candidate("Catalogue", 7001, "cat7001"),
            ],
            &["Main".into(), "Catalogue".into()],
            &["Catalogue".into(), "Main".into()],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected[0].project, "Catalogue");
        assert_eq!(result.shadowed, vec![candidate("Main", 7000, "main7000")]);
    }

    #[test]
    fn cross_project_collision_without_priority_blocks() {
        let result = select_catalogue_candidates(
            [
                candidate("Main", 7000, "main7000"),
                candidate("Catalogue", 7000, "cat7000"),
            ],
            &["Main".into(), "Catalogue".into()],
            &[],
        );
        assert!(result.selected.is_empty());
        assert_eq!(result.blockers.len(), 1);
    }

    #[test]
    fn same_project_extract_parent_and_leaf_are_not_blockers() {
        let result = select_catalogue_candidates(
            [
                candidate("AMS", 7355, "ams000/ams7355"),
                candidate("AMS", 7355, "ams000/ams7355_0001"),
            ],
            &["AMS".into()],
            &[],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(
            result.selected,
            vec![candidate("AMS", 7355, "ams000/ams7355_0001")]
        );
        assert_eq!(
            result.shadowed,
            vec![candidate("AMS", 7355, "ams000/ams7355")]
        );
    }

    #[test]
    fn sibling_extracts_still_block_catalogue_selection() {
        let result = select_catalogue_candidates(
            [
                candidate("AMS", 9990, "ams000/ams9990_0001"),
                candidate("AMS", 9990, "ams000/ams9990_0002"),
            ],
            &["AMS".into()],
            &[],
        );
        assert!(result.selected.is_empty());
        assert_eq!(result.blockers.len(), 1);
    }
}
