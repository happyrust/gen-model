//! 生产环境唯一的 `e3d-model` 生成与增量入口。
//!
//! 生成出来的几何只落进程内投影存储（[`crate::fast_model::model_memory_store`]）。
//! **本模块不认识数据库**：没有 SurrealDB 句柄，不拼 SQL，不开发布事务。把投影搬进库里
//! 是另一层适配器的事，它从存储里读结构化记录，不必回头再算一遍几何。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{RefU64, RefnoEnum};
use anyhow::{Context, bail};
use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
use e3d_io::engine::{ReadOnlyEngine, ScanTier};
use e3d_io::refno::RefNo;
use e3d_io::session::DbView;
use e3d_model::catalogue::CatalogueMeshCache;
use e3d_model::elmodl::{GeneratedElement, GeometryId};
use e3d_model::increment::{IncrementReport, collect_window, increment_update};
use e3d_model::pipeline::{Report, generate_subtree_with_cache};
use e3d_model::primitive_instance::canonical_primitive_mesh;
use e3d_model::transform::dmat4_to_affine4x3;
use glam::DMat4;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::data_interface::cata_closure::{CataDbLocator, InMemoryCataLocator};
use crate::data_interface::direct_store::{DbPin, DirectSchema};
use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::data_interface::mdb_membership;
use crate::data_interface::model_source;
use crate::data_interface::rigid_motion::{
    RepublishGate, TransformAnchor, republish_gate, republish_records,
};
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectedTube, ProjectionCommit,
    ProjectionScope, RootReceipt, packed_refno,
};

/// 刚体前移执行体（ADR-066 B6）一轮的结果：哪些根走成便宜路（`completed`），哪些是「回执已在 T」
/// 的窗口重放（`replayed`——投影一字未动、按已完成计，见 [`republish_gate`]），哪些回退整根重算
/// （`fallbacks`，带原因）。回退永远正确、只是不省。用 pdms `a/b` 串标识根，方便与 `UnitTask` 对齐。
#[derive(Debug, Default)]
pub struct RigidMotionReport {
    pub completed: Vec<String>,
    pub replayed: Vec<String>,
    pub fallbacks: Vec<(String, String)>,
}

/// 刚体前移执行体一次吃的一根。`source_start_sesno` 是规划器算 `Δ` 的窗口基线 S（随
/// [`TransformUnit`](crate::data_interface::rigid_motion::TransformUnit) 的线格式带过来）：
/// `Δ = W_T·W_S⁻¹` 只对「投影还停在 S」的记录成立，重放闸拿它对回执，对不上就回退 regen。
#[derive(Debug, Clone)]
pub struct RigidMotionUnit {
    pub root_refno: String,
    pub source_start_sesno: u32,
    pub anchors: Vec<TransformAnchor>,
}

/// 一个生成根这一轮的投影决策：现在要投的目标是什么，上一轮投成了什么。
///
/// 它取代了旧的根凭证库行。跟过来的是「目标没变就不必重算」那条判据；没跟过来的是
/// revision CAS——那把锁护的是多个进程抢同一个库行，进程内投影没有那个竞争。
#[derive(Debug, Clone)]
struct RootPublication {
    root: RefNo,
    model_target: Option<ModelTarget>,
    published: Option<RootReceipt>,
    projection_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFileTarget {
    pub dbnum: u32,
    pub db_type: String,
    pub file: String,
    pub session: u32,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTarget {
    pub project: String,
    pub design: ModelFileTarget,
    pub catalogue: Vec<ModelFileTarget>,
    pub template_attlib_digest: String,
    pub generator_fingerprint: String,
    pub tessellation_profile: String,
}
use crate::fast_model::e3d_mesh_store::{
    E3dPersistReport, MeshWrite, baked_mesh_id, canonical_mesh_id, ensure_mesh_file,
    geometry_record_id,
};

/// One database's element index at a pinned session: its top-level elements
/// (owner missing, self, or not indexed — normally the WORL) and the owner of
/// every indexed element.  `roots` is also the entry set for the file-side
/// generation-root enumeration (`generation_root::enumerate_generation_roots`).
#[derive(Debug, Clone)]
pub(crate) struct SourceIndex {
    /// 文件自报的库号（描述符 `db_mark`）——扫的是哪个库由文件说了算，不由调用方转述。
    pub(crate) dbnum: u32,
    pub(crate) roots: Vec<RefNo>,
    /// Shared, never mutated after the scan: one in-memory batch holds as many
    /// `GeneratedSnapshot`s as it has roots, and each used to carry its own copy
    /// of this whole-database table.
    pub(crate) owners: Arc<BTreeMap<(u32, u32), RefNo>>,
}

/// Typed failures of [`E3dModelService::generate_snapshot_source`] that a caller
/// must be able to distinguish without parsing message text.  Everything else
/// that path returns is a plain `anyhow` error meaning "geometry could not be
/// generated"; downcast to this type to recognise the source-side cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotSourceError {
    /// The session's element index has no entry for the requested root: at that
    /// session the refno simply did not exist (yet, or any more).
    RootNotInSession { root: RefNo, dbnum: u32, sesno: u32 },
}

impl std::fmt::Display for SnapshotSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotInSession { root, dbnum, sesno } => write!(
                f,
                "refno {root} is not in the element index of dbnum {dbnum} at session {sesno}"
            ),
        }
    }
}

impl std::error::Error for SnapshotSourceError {}

/// Pure, session-pinned generation result.  It contains no database handle and
/// is safe to route either to the current projection or to the historical
/// in-memory projection.
pub struct GeneratedSnapshot {
    pub elements: Vec<GeneratedElement>,
    pub owners: Arc<BTreeMap<(u32, u32), RefNo>>,
    pub report: Report,
    /// e3d-model DFS 访问到的子树元素（含根、含不出几何的容器）。读透形态下这就是
    /// 「生成一根要顺手写透哪些元素」的名单（`GenerationOutcome::visited`）。
    pub visited: Vec<RefNo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectDbFile {
    db_type: String,
    project: String,
    path: PathBuf,
}

type ProjectDbFiles = Arc<HashMap<(String, u32), ProjectDbFile>>;
type DependencySessions = Arc<std::sync::Mutex<BTreeMap<(u32, u32), u32>>>;

fn project_db_key(project: &str, dbnum: u32) -> (String, u32) {
    (project.trim().to_ascii_lowercase(), dbnum)
}

/// Pick a dependency in the target design database's project when that project
/// has a same-type copy of the selected MDB database. Foreign DESI files carry
/// project-relative catalogue references: ACP 7015, for example, needs ACP
/// CATA 7000 even though `/ALL` also declares AMS CATA 7000.
fn select_dependency_file(
    locator: &InMemoryCataLocator,
    project_files: &ProjectDbFiles,
    preferred_project: Option<&str>,
    dbnum: u32,
) -> Option<ProjectDbFile> {
    let selected_type = locator.db_type_of(dbnum)?;
    if let Some(candidate) = preferred_project
        .and_then(|project| project_files.get(&project_db_key(project, dbnum)))
        .filter(|candidate| candidate.db_type.eq_ignore_ascii_case(&selected_type))
    {
        return Some(candidate.clone());
    }
    let (project, path) = locator.file_of(dbnum)?;
    Some(ProjectDbFile {
        db_type: selected_type,
        project,
        path,
    })
}

/// A source-session-pinned batch of generated roots.  Generation fills this
/// structure without touching the persisted model projection; a later
/// consumer may publish its snapshots in bounded transactions.
pub struct GeneratedMemoryBatch {
    pub(crate) dbnum: u32,
    pub(crate) source_sesno: u32,
    pub(crate) set: Arc<DbSet>,
    pub(crate) index: Arc<SourceIndex>,
    dependency_sessions: DependencySessions,
    pub(crate) snapshots: Vec<(RefNo, BakedSnapshot)>,
    /// Roots whose generation raised a hard error, kept as dead letters instead
    /// of aborting the batch (ADR-041 §6).  The consumer counts them as failed.
    pub(crate) failures: Vec<(RefNo, String)>,
}

/// An initialization producer outcome. Success is a frozen projection, not a
/// database completion and not a mutation of the process Current projection.
pub struct FrozenGeneration {
    pub root: RefNo,
    pub outcome: Result<ProjectionCommit, String>,
}

/// The run's catalogue mesh caches, one per resident worker, checked out for
/// the length of a wave and returned afterwards.  Only the checkout touches the
/// mutex; evaluation never runs inside it (ADR-052 background 2).
type CachePool = Arc<std::sync::Mutex<Vec<CatalogueMeshCache>>>;

fn checkout_cache(pool: &CachePool) -> CatalogueMeshCache {
    pool.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .unwrap_or_default()
}

fn return_cache(pool: &CachePool, cache: CatalogueMeshCache) {
    pool.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(cache);
}

/// The roots of one production run, and the sliding window that keeps their
/// generated geometry bounded.
///
/// Workers claim by index and write back **into their own slot**; the assembler
/// takes them off the front in source order.  Order is not decoration: it is
/// what makes slice boundaries and the `Required` verdict reproducible, and it
/// is why `geometry_workers = 1` replays the old sequential run exactly.
///
/// The window is what replaces a per-cohort barrier.  Waiting for a whole
/// cohort to finish before starting the next one costs the slowest root of
/// every cohort, and geometry load is long-tailed; here a worker may run up to
/// `window` roots ahead of the last one the assembler took, so only the head of
/// the line can stall anyone.
struct RootQueue {
    roots: Vec<RefNo>,
    /// Next index a worker may claim.
    next: std::sync::atomic::AtomicUsize,
    /// How many roots the assembler has taken off the front.
    committed: std::sync::atomic::AtomicUsize,
    window: usize,
    slots: std::sync::Mutex<Vec<Option<anyhow::Result<BakedSnapshot>>>>,
}

impl RootQueue {
    fn new(roots: Vec<RefNo>, window: usize) -> Self {
        let slots = std::sync::Mutex::new((0..roots.len()).map(|_| None).collect());
        Self {
            roots,
            next: std::sync::atomic::AtomicUsize::new(0),
            committed: std::sync::atomic::AtomicUsize::new(0),
            window,
            slots,
        }
    }

    /// How many roots a worker could still start right now.  The assembler uses
    /// it to decide how many workers to keep resident.
    fn claimable(&self) -> usize {
        let next = self.next.load(std::sync::atomic::Ordering::Acquire);
        let ceiling = self
            .roots
            .len()
            .min(self.committed.load(std::sync::atomic::Ordering::Acquire) + self.window);
        ceiling.saturating_sub(next)
    }

    /// Claim the next root, or `None` when the run is done or the window is
    /// full.  Returning `None` is how a worker hands its permit back rather
    /// than waiting on the assembler: a permit holder that waits is exactly the
    /// capacity inversion ADR-052 opens with.
    fn claim(&self) -> Option<(usize, RefNo)> {
        let mut next = self.next.load(std::sync::atomic::Ordering::Acquire);
        loop {
            let ceiling = self
                .roots
                .len()
                .min(self.committed.load(std::sync::atomic::Ordering::Acquire) + self.window);
            if next >= ceiling {
                return None;
            }
            match self.next.compare_exchange_weak(
                next,
                next + 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => return Some((next, self.roots[next])),
                Err(observed) => next = observed,
            }
        }
    }

    fn store(&self, slot: usize, outcome: anyhow::Result<BakedSnapshot>) {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot] = Some(outcome);
    }

    /// Take the next root in source order, if its worker has finished.  Taking
    /// it is also what slides the window forward.
    fn take_ready(&self) -> Option<(RefNo, anyhow::Result<BakedSnapshot>)> {
        let committed = self.committed.load(std::sync::atomic::Ordering::Acquire);
        let outcome = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(committed)?
            .take()?;
        self.committed
            .store(committed + 1, std::sync::atomic::Ordering::Release);
        Some((self.roots[committed], outcome))
    }
}

/// One resident worker: one permit, one catalogue cache, and a loop that keeps
/// claiming until the run is done or the window is full.
///
/// Dropping the returned future does not cancel the closure — a synchronous CPU
/// segment cannot be interrupted.  An abandoned worker stops on its own within
/// at most `window` roots, because nothing is advancing `committed` any more.
fn spawn_generation_worker(
    set: Arc<DbSet>,
    index: Arc<SourceIndex>,
    queue: Arc<RootQueue>,
    caches: CachePool,
    mesh_dir: PathBuf,
) -> impl std::future::Future<Output = anyhow::Result<()>> {
    crate::fast_model::concurrency::run_gated_cpu(move || {
        let mut cache = checkout_cache(&caches);
        while let Some((slot, root)) = queue.claim() {
            // Bake here, not on the consumer: triangulation, the SHA-256 and
            // the mesh file write are the rest of this root's CPU cost, and
            // doing them here is what lets the `Manifold` die on this thread
            // instead of riding into the serial publication step.
            let outcome = generate_snapshot_from_set(&set, &index, root, &mut cache)
                .and_then(|snapshot| bake_snapshot(snapshot, &mesh_dir));
            queue.store(slot, outcome);
        }
        return_cache(&caches, cache);
        Ok(())
    })
}

/// One slice under construction on the producer side.  It exists so that the
/// element budget is counted where the elements are produced, not recovered by
/// walking a finished batch.
#[derive(Default)]
struct MemorySlice {
    snapshots: Vec<(RefNo, BakedSnapshot)>,
    failures: Vec<(RefNo, String)>,
    elements: usize,
}

impl MemorySlice {
    fn push(&mut self, root: RefNo, snapshot: BakedSnapshot) {
        self.elements += snapshot.elements.len();
        self.snapshots.push((root, snapshot));
    }

    fn is_full(&self, root_budget: usize, element_budget: usize) -> bool {
        self.snapshots.len() >= root_budget || self.elements >= element_budget
    }

    /// Hand the sealed slice over and start an empty one.  A slice carrying
    /// nothing but dead letters still goes: the consumer is where they are
    /// counted.  Returns false once the consumer is gone — the producer stops
    /// quietly then, so that the consumer's error is the one that surfaces.
    async fn deliver(
        &mut self,
        dbnum: u32,
        source_sesno: u32,
        input: &GenerationInput,
        sink: &tokio::sync::mpsc::Sender<GeneratedMemoryBatch>,
    ) -> bool {
        if self.snapshots.is_empty() && self.failures.is_empty() {
            return true;
        }
        self.elements = 0;
        let batch = GeneratedMemoryBatch {
            dbnum,
            source_sesno,
            set: input.set.clone(),
            index: input.index.clone(),
            dependency_sessions: input.dependency_sessions.clone(),
            snapshots: std::mem::take(&mut self.snapshots),
            failures: std::mem::take(&mut self.failures),
        };
        sink.send(batch).await.is_ok()
    }
}

static DB_GENERATION_LOCKS: OnceLock<dashmap::DashMap<u32, Arc<Mutex<()>>>> = OnceLock::new();

#[derive(Clone)]
struct GenerationInput {
    set: Arc<DbSet>,
    index: Arc<SourceIndex>,
    dependency_sessions: DependencySessions,
}

type GenerationInputKey = (u32, u32, String);

static GENERATION_INPUT_CACHE: OnceLock<
    std::sync::Mutex<HashMap<GenerationInputKey, GenerationInput>>,
> = OnceLock::new();

fn generation_input(
    service: &E3dModelService,
    dbnum: u32,
    source_sesno: u32,
) -> anyhow::Result<GenerationInput> {
    let cache = GENERATION_INPUT_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let identity = service.generation_identity.clone().unwrap_or_else(|| {
        let pin = service
            .pin(dbnum)
            .expect("generation db pin checked by caller");
        std::fs::canonicalize(&pin.file)
            .unwrap_or_else(|_| pin.file.clone())
            .to_string_lossy()
            .into_owned()
    });
    let key = (dbnum, source_sesno, identity);
    if let Some(value) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return Ok(value);
    }
    let pin = service.pin(dbnum)?;
    let index = Arc::new(scan_index(&pin.file, Some(source_sesno))?);
    let set = service.build_set(dbnum, Some(source_sesno))?;
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(cached_dbnum, _, _), _| *cached_dbnum != dbnum);
    let input = GenerationInput {
        set,
        index,
        dependency_sessions: service.dependency_sessions.clone(),
    };
    guard.insert(key, input.clone());
    Ok(input)
}

/// Drop every cached generation input of one database.
///
/// The cache is keyed by `(dbnum, sesno, identity)` and only ever evicts the
/// *same* dbnum's older entry when a new one is inserted, so an entry outlives
/// the run that built it for as long as the process lives.  An initialization
/// run keys its entry by the digest of its frozen tokens — a key no later
/// service (`from_current`, `from_frozen_source`) asks for again — which makes
/// the entry dead weight once the run has finished: one parsed `attlib.dat`,
/// one open `DbSet` over every MDB design file, and one whole-database owner
/// table per initialized dbnum, held until exit.
///
/// Callers still holding an `Arc` clone keep their input alive; the next
/// `generation_input` for the dbnum rebuilds from the file.  The removed values
/// are dropped outside the cache lock.  Returns how many entries were removed.
pub(crate) fn forget_generation_input(dbnum: u32) -> usize {
    let Some(cache) = GENERATION_INPUT_CACHE.get() else {
        return 0;
    };
    let removed: Vec<GenerationInput> = {
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = guard
            .keys()
            .filter(|(cached_dbnum, _, _)| *cached_dbnum == dbnum)
            .cloned()
            .collect::<Vec<_>>();
        keys.iter().filter_map(|key| guard.remove(key)).collect()
    };
    removed.len()
}

fn db_generation_lock(dbnum: u32) -> Arc<Mutex<()>> {
    DB_GENERATION_LOCKS
        .get_or_init(dashmap::DashMap::new)
        .entry(dbnum)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Serializes the final durable-watermark fence with every current-model
/// generation for this database. The write-behind worker holds it only for the
/// last pending check and the short control-plane transaction.
pub(crate) async fn lock_db_generation(dbnum: u32) -> tokio::sync::OwnedMutexGuard<()> {
    db_generation_lock(dbnum).lock_owned().await
}

pub struct E3dModelService {
    schema: DirectSchema,
    pins: Vec<DbPin>,
    locator: InMemoryCataLocator,
    project_files: ProjectDbFiles,
    dependency_sessions: DependencySessions,
    mesh_dir: PathBuf,
    /// Digest of the durable file-generation token for a file update round.
    /// It prevents a replaced file at the same path/dbnum/session from reusing
    /// another generation's cached DbSet and index.
    generation_identity: Option<String>,
    /// Initialization reuses the verified, durable source token, not mutable
    /// file length/mtime that may advance while the pinned session is generated.
    frozen_file_digests: BTreeMap<(u32, u32), String>,
}

#[cfg(test)]
#[path = "initialization_target_tests.rs"]
mod initialization_target_tests;

#[derive(Clone)]
struct E3dDbResolver {
    locator: InMemoryCataLocator,
    project_files: ProjectDbFiles,
    target_dbnum: u32,
    preferred_project: Option<String>,
    template_dir: PathBuf,
    sessions: DependencySessions,
}

impl DbFileResolver for E3dDbResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        let selected = select_dependency_file(
            &self.locator,
            &self.project_files,
            self.preferred_project.as_deref(),
            dbno,
        )?;
        let template = template_file_for(&self.template_dir, &selected.db_type).ok()?;
        let sesno = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = (self.target_dbnum, dbno);
            if let Some(&sesno) = sessions.get(&key) {
                sesno
            } else {
                let mut view = DbView::open(&selected.path).ok()?;
                let sesno = view.walk_sessions().ok()?.first()?.sesno;
                sessions.insert(key, sesno);
                sesno
            }
        };
        Some(DbFilePin {
            file: selected.path,
            template,
            db_type: Some(selected.db_type),
            sesno: Some(sesno),
        })
    }
}

impl E3dModelService {
    /// Check the same immutable target and mesh completeness as generation.
    /// Receipt existence alone is not freshness: SAVEWORK, replacement/rollback,
    /// catalogue dependencies and schema changes can all invalidate it.
    pub(crate) fn current_cached_roots(&self, roots: &[RefNo]) -> anyhow::Result<std::collections::HashSet<RefNo>> {
        let store = ModelMemoryStore::global();
        let mut current = std::collections::HashSet::new();
        for &root in roots {
            let Some(receipt) = store.receipt(ProjectionScope::Current, root) else {
                continue;
            };
            // Keep each root's real dependency set separate; combining unrelated
            // roots would invalidate otherwise-current targets on every request.
            let sessions = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
            self.hydrate_published_dependencies(&receipt, &sessions)?;
            let source_sesno = self.source_session(self.pin(root.dbno())?)?;
            let publication = RootPublication {
                root,
                model_target: Some(self.model_target(root.dbno(), source_sesno, &sessions)?),
                published: Some(receipt),
                projection_complete: false,
            };
            if cached_root_target_matches(&publication, source_sesno)
                && store.root_projection_complete(ProjectionScope::Current, root, &self.mesh_dir)
            {
                current.insert(root);
            }
        }
        Ok(current)
    }

    /// The current projection (ADR-054): source files come from the MDB
    /// declaration, and every pin is left unpinned so that generation resolves
    /// the target database's **latest file session** at the moment it runs
    /// (`source_session`). `dbnum_watermark.applied_sesno` is the ingestion
    /// watermark, not a generation timepoint, and an empty watermark table must
    /// not stop a never-parsed project from generating.
    pub async fn from_current() -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        let (pins, locator, project_files) = current_mdb_source_inventory()?;
        Ok(Self {
            schema: DirectSchema::open_from_env()?,
            // CATA identity stays with the project-aware locator (see `build_set`
            // and `model_target`); the pins carry the design-side files only, as
            // the watermark pins did before.
            pins: pins
                .into_iter()
                .filter(|pin| !pin.db_type.eq_ignore_ascii_case("CATA"))
                .collect(),
            locator,
            project_files,
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir: option.get_meshes_path(),
            generation_identity: None,
            frozen_file_digests: BTreeMap::new(),
        })
    }

    /// Build the normal MDB-aware service while pinning one DESI input to the
    /// exact file generation and session captured by the data file round.
    pub async fn from_frozen_source(
        source: &crate::data_interface::frozen_source::FrozenSource,
    ) -> anyhow::Result<Self> {
        Self::from_frozen_sources(std::slice::from_ref(source), false).await
    }

    /// Initialization pins the entire resolver inventory before the manifest
    /// becomes active. A dependency can never fall back to a later session.
    pub async fn from_initialization_sources(
        source: &crate::data_interface::frozen_source::FrozenSource,
        dependencies: &[String],
    ) -> anyhow::Result<Self> {
        use crate::data_interface::frozen_source::FrozenSource;
        let mut service = Self::from_frozen_source(source).await?;
        let mut files = HashMap::new();
        let mut tokens = vec![source.commit_token_json()?];
        let target_dbnum = source.dbnum() as u32;
        for token in dependencies.iter().chain(tokens.clone().iter()) {
            let frozen = FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
            service.frozen_file_digests.insert(
                (frozen.dbnum() as u32, frozen.target_sesno()),
                hex::encode(Sha256::digest(frozen.commit_token_json()?.as_bytes())),
            );
            let project = service
                .locator
                .file_of(frozen.dbnum() as u32)
                .map(|(project, _)| project)
                .context("frozen dependency is no longer in MDB")?;
            files.insert(
                frozen.dbnum() as u32,
                (
                    frozen.db_type().to_string(),
                    project,
                    frozen.path().to_path_buf(),
                ),
            );
            service
                .dependency_sessions
                .lock()
                .unwrap()
                .insert((target_dbnum, frozen.dbnum() as u32), frozen.target_sesno());
            if let Some(pin) = service.pins.iter_mut().find(|p| p.dbnum == frozen.dbnum()) {
                pin.file = frozen.path().to_path_buf();
                pin.sesno = Some(frozen.target_sesno());
            }
        }
        service.locator = InMemoryCataLocator::from_parts(HashMap::new(), files);
        // The initialization token list is already the exact dependency
        // inventory. Do not replace one of those frozen files with a live
        // same-project alternative.
        service.project_files = Arc::new(HashMap::new());
        tokens.extend_from_slice(dependencies);
        tokens.sort_unstable();
        service.generation_identity =
            Some(hex::encode(Sha256::digest(tokens.join("\n").as_bytes())));
        Ok(service)
    }

    pub fn initialization_target_hash(&self, dbnum: u32, session: u32) -> anyhow::Result<String> {
        let target = self.model_target(dbnum, session, &self.dependency_sessions)?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(&Some(
            target,
        ))?)))
    }

    /// Build one service whose selected DESI inputs are all pinned to their
    /// durable file generations. Room topology passes `published_only=true`
    /// so an MDB file without a successful published version cannot leak in at
    /// its live latest session.
    pub(crate) async fn from_frozen_sources(
        sources: &[crate::data_interface::frozen_source::FrozenSource],
        published_only: bool,
    ) -> anyhow::Result<Self> {
        let sources = sources
            .iter()
            .map(|source| source.reopen_verified())
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut service = Self::from_current().await?;
        if published_only {
            let published = sources
                .iter()
                .map(|source| source.dbnum())
                .collect::<std::collections::HashSet<_>>();
            service.pins.retain(|pin| {
                !pin.db_type.eq_ignore_ascii_case("DESI") || published.contains(&pin.dbnum)
            });
        }
        let mut tokens = Vec::with_capacity(sources.len());
        for source in &sources {
            let pin = service
                .pins
                .iter_mut()
                .find(|pin| pin.dbnum == source.dbnum())
                .with_context(|| format!("current MDB has no DESI dbnum {}", source.dbnum()))?;
            anyhow::ensure!(
                pin.db_type.eq_ignore_ascii_case(source.db_type()),
                "frozen source type {} differs from MDB pin {}",
                source.db_type(),
                pin.db_type
            );
            pin.file = source.path().to_path_buf();
            pin.sesno = Some(source.target_sesno());
            tokens.push(source.commit_token_json()?);
        }
        tokens.sort_unstable();
        service.generation_identity =
            Some(hex::encode(Sha256::digest(tokens.join("\n").as_bytes())));
        Ok(service)
    }

    /// The DESI database that owns a batch of generation roots.
    ///
    /// Resolved from the MDB's DESI files by index lookup (ADR-054 constraint 3),
    /// not from `pe`: a never-parsed project has no `pe` rows at all.
    pub async fn dbnum_for_roots(roots: &[String]) -> anyhow::Result<u32> {
        let root = roots.first().context("generation roots are empty")?;
        let refno = parse_refno(root)?;
        tokio::task::spawn_blocking(move || model_source::dbnum_of_root(refno))
            .await
            .map_err(|error| anyhow::anyhow!("resolve root dbnum task failed: {error}"))?
            .with_context(|| format!("root {root} has no source database in the current MDB"))
    }

    pub async fn generate_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &input.set,
            &input.index,
            &roots,
            failure_policy,
            &input.dependency_sessions,
            None,
        )
        .await
    }

    /// Producer half of the Core.dll-style split: read the source, evaluate the
    /// geometry, and hand the consumer one **bounded** slice at a time.  It
    /// performs no model writes.
    ///
    /// A slice is sealed as soon as it reaches either budget:
    /// `model_regen_execution_group` roots (the publication cohort width, which
    /// is what decided transaction boundaries before) or
    /// `model_memory_slice_elements` generated elements.  Root count alone is
    /// not a memory budget — a BRAN and an EQUI differ by two orders of
    /// magnitude, and what sits in the slice is `GeneratedElement`, the fattest
    /// representation on the chain.  A root is never split across slices: it is
    /// the atomic publication unit.
    ///
    /// Generation runs continuously: `min(quota, claimable)` resident workers
    /// keep claiming roots off a shared queue, and may run up to
    /// `model_regen_execution_group` roots ahead of the last one folded into a
    /// slice.  That run-ahead is what decouples generation from publication —
    /// the workers do not stop at every cohort boundary waiting for the slowest
    /// root in it — and it is also the memory bound: the peak is one window of
    /// generated roots plus the partial slice.  Under parallel workers the
    /// element budget can only seal *publication* slices, not cap the in-flight
    /// set: N workers means N roots being built no matter what the budget says.
    /// Projects whose roots are unusually fat should lower
    /// `model_regen_execution_group`, which narrows the window and the cohort
    /// together.
    ///
    /// The producer takes no `db_generation_lock`: reading the source file and
    /// evaluating geometry touches neither the projection store nor the root
    /// receipts that lock protects.  What each slice carries out of here is the
    /// pinned `source_sesno`; the consumer re-checks it under the lock before it
    /// commits anything.
    async fn produce_slices(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
        sink: &tokio::sync::mpsc::Sender<GeneratedMemoryBatch>,
    ) -> anyhow::Result<()> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let root_budget = crate::options::model_regen_execution_group().max(1);
        let element_budget = crate::options::model_memory_slice_elements();
        // A production run is one catalogue reuse window: identical evaluated
        // primitives across different delivery roots must share their local
        // mesh.  One cache per worker rather than one shared behind a mutex —
        // evaluating inside that mutex is the convoy ADR-052 opens with — and
        // the pool outlives the waves so the window is the whole run.
        let caches = CachePool::default();
        let queue = Arc::new(RootQueue::new(roots, root_budget));
        let mut workers = futures::stream::FuturesUnordered::new();
        let mut slice = MemorySlice::default();
        loop {
            // Top the pool back up: a worker that hit the window returned its
            // permit rather than sit on it, so it has to be re-launched once
            // the assembler slid the window forward.
            let wanted = crate::fast_model::concurrency::resident_workers(queue.claimable());
            while workers.len() < wanted {
                workers.push(spawn_generation_worker(
                    input.set.clone(),
                    input.index.clone(),
                    queue.clone(),
                    caches.clone(),
                    self.mesh_dir.clone(),
                ));
            }
            let Some(finished) = futures::StreamExt::next(&mut workers).await else {
                break;
            };
            finished?;
            while let Some((root, outcome)) = queue.take_ready() {
                // A root that cannot be read or evaluated is one dead letter,
                // not a reason to drop the whole page: a single permanently
                // broken component would otherwise wedge every batch that
                // contains it.  The verdict stays sequential and in root order,
                // so `geometry_workers = 1` reproduces the old run exactly.
                match outcome {
                    Ok(snapshot) => {
                        if !snapshot.report.failed.is_empty()
                            && matches!(failure_policy, GeometryFailurePolicy::Required)
                        {
                            bail!(
                                "e3d-model root {root} has {} failed element(s)",
                                snapshot.report.failed.len()
                            );
                        }
                        slice.push(root, snapshot);
                    }
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        log::error!("e3d-model root {root} failed: {error:#}");
                        slice.failures.push((root, format!("{error:#}")));
                    }
                    Err(error) => {
                        return Err(classify_required_root_error(root, error))
                            .with_context(|| format!("generate root {root}"));
                    }
                }
                if slice.is_full(root_budget, element_budget)
                    && !slice.deliver(dbnum, source_sesno, &input, sink).await
                {
                    return Ok(());
                }
            }
        }
        slice.deliver(dbnum, source_sesno, &input, sink).await;
        Ok(())
    }

    /// Consume an in-memory batch without re-reading or re-generating any
    /// root.  Only this phase commits into the projection store.
    pub async fn persist_memory_batch(
        &self,
        mut batch: GeneratedMemoryBatch,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(batch.dbnum).lock_owned().await;
        ensure_not_older_than_persisted(batch.dbnum, batch.source_sesno).await?;
        let chunk_size = crate::options::model_regen_execution_group().max(1);
        let snapshots = std::mem::take(&mut batch.snapshots);
        let mut total = E3dPersistReport::default();
        // Producer-side dead letters are failures of this publication too: the
        // roots they name keep their previously published geometry, and the
        // caller must still see a non-zero `failed`.
        for (root, error) in std::mem::take(&mut batch.failures) {
            log::error!("e3d-model root {root} was not generated: {error}");
            total.failed += 1;
            total.failed_roots.push((root, error));
        }
        let mut snapshots = snapshots;
        while !snapshots.is_empty() {
            let take = snapshots.len().min(chunk_size);
            let mut memory_chunk = snapshots.drain(..take).collect::<Vec<_>>();
            let roots = memory_chunk
                .iter()
                .map(|(root, _)| *root)
                .collect::<Vec<_>>();
            let report = self
                .generate_refs(
                    batch.dbnum,
                    batch.source_sesno,
                    &batch.set,
                    &batch.index,
                    &roots,
                    failure_policy,
                    &batch.dependency_sessions,
                    Some(&mut memory_chunk),
                )
                .await?;
            total.merge_counts(report);
        }
        Ok(total)
    }

    /// Generate `roots` and publish them slice by slice, producer and consumer
    /// running concurrently across a **depth-1 bounded** channel (ADR-041 §8).
    ///
    /// The channel does not make the write side faster — publication is
    /// serial by construction — it moves the producer's wait from "the whole
    /// page is published" to "there is room for one more slice", which is the
    /// `(P + C) × N → max(P, C) × N` the ADR costs out.  Its depth is the whole
    /// point: an unbounded channel would not block either, at the price of
    /// unbounded memory, and that is the trade the ADR rejected.  At most three
    /// slices are alive at once — one being published, one buffered, one under
    /// construction — instead of the entire page.
    pub async fn generate_frozen_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        sink: tokio::sync::mpsc::Sender<FrozenGeneration>,
    ) -> anyhow::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<GeneratedMemoryBatch>(1);
        let produce = async {
            let result = self
                .produce_slices(dbnum, roots, GeometryFailurePolicy::BestEffortFallback, &tx)
                .await;
            drop(tx);
            result
        };
        let freeze = async move {
            while let Some(batch) = rx.recv().await {
                for (root, error) in batch.failures {
                    sink.send(FrozenGeneration {
                        root,
                        outcome: Err(error),
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("initialization journal receiver closed"))?;
                }
                for (root, snapshot) in batch.snapshots {
                    let outcome = (|| -> anyhow::Result<ProjectionCommit> {
                        anyhow::ensure!(
                            snapshot.report.failed.is_empty(),
                            "root {root} has {} failed geometry elements: {}",
                            snapshot.report.failed.len(),
                            summarize_geometry_incidents(&snapshot.report.failed)
                        );
                        let publication = RootPublication {
                            root,
                            model_target: Some(self.model_target(
                                dbnum,
                                batch.source_sesno,
                                &batch.dependency_sessions,
                            )?),
                            published: None,
                            projection_complete: false,
                        };
                        let (projection, _) = project_geometry_delta(
                            dbnum,
                            batch.source_sesno,
                            snapshot.elements,
                            Vec::new(),
                            Some(&publication),
                            &batch.index.owners,
                        )?;
                        // Mesh files are content addressed. Flush dependencies before
                        // the queue may acknowledge this envelope after a crash.
                        for mesh in projection
                            .upserts
                            .iter()
                            .map(|g| g.mesh_id.as_str())
                            .collect::<BTreeSet<_>>()
                        {
                            let path = self.mesh_dir.join(format!("{mesh}.mesh"));
                            std::fs::OpenOptions::new()
                                .read(true)
                                .write(true)
                                .open(&path)
                                .with_context(|| {
                                    format!("missing handed-off mesh {}", path.display())
                                })?
                                .sync_all()?;
                        }
                        Ok(projection)
                    })()
                    .map_err(|e| format!("{e:#}"));
                    sink.send(FrozenGeneration { root, outcome })
                        .await
                        .map_err(|_| anyhow::anyhow!("initialization journal receiver closed"))?;
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        futures::future::try_join(produce, freeze).await?;
        Ok(())
    }

    pub async fn generate_and_persist_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        crate::native_runtime::ensure_geometry_runtime()?;
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<GeneratedMemoryBatch>(1);
        let produce = async move {
            let outcome = self
                .produce_slices(dbnum, roots, failure_policy, &sender)
                .await;
            // Closing the last sender is what ends the consumer's loop.
            drop(sender);
            outcome
        };
        let consume = async move {
            let mut total = E3dPersistReport::default();
            while let Some(slice) = receiver.recv().await {
                match self.persist_memory_batch(slice, failure_policy).await {
                    Ok(report) => total.merge_counts(report),
                    Err(error) => {
                        // Stop the producer at its next hand-off rather than
                        // let it keep filling memory for a consumer that is
                        // already done; its error is the one worth reporting.
                        receiver.close();
                        return Err(error);
                    }
                }
            }
            Ok(total)
        };
        let (produced, consumed) = futures::future::join(produce, consume).await;
        let total = consumed?;
        produced?;
        Ok(total)
    }

    /// 刚体前移执行体（ADR-066 B6）的**存储半边**：把「只动摆放」的根从整根重算改成「投影记录左乘
    /// Δ + 前移回执到 T」，只左乘并提交进内存投影——**生成服务不碰数据库**（登记 / durable flush 由
    /// 调用方在 batch_worker 里做，见 `run_rigid_motion_units`）。**保守优先**：任一前置不满足就把这根
    /// 放进 `fallbacks` 退回整根重算，回退永远正确。本轮只做**纯平移**（`republish_records` 内部走
    /// `translate_bounds`）；含旋转（要读网格重算盒）让盒闭包报错 → 回退。`completed` 的根已把左乘后的
    /// 记录与前移到 T 的回执提交进投影，等调用方登记后由 `publish_completed_file_round` 统一刷 durable。
    /// 开关 `model_transform_cheap_path` 关着时规划器不产 transform 单元，本函数不会被调到。
    ///
    /// 幂等（S4b 审核 P0）：transform 不像 regen 天然幂等——动记录之前先过 [`republish_gate`]：
    /// 回执已在 T（发布成功但回合没收尾的窗口重放）跳过、按已完成计（`replayed`）；回执不在规划
    /// 基线 S 上（半截状态 / 旧计划 / 别的路径动过它）`Δ` 对不上，回退整根重算。
    ///
    /// 待补（开关翻开前 B8 对拍门会挡住）：含旋转时读网格顶点重算盒；`model_target.design` 落款推进到
    /// pin@T（现在沿用旧的，最坏是下一窗不命中缓存、照旧重算，安全）。
    pub(crate) async fn republish_roots_by_rigid_motion(
        &self,
        dbnum: u32,
        target_sesno: u32,
        units: &[RigidMotionUnit],
    ) -> anyhow::Result<RigidMotionReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        ensure_not_older_than_persisted(dbnum, target_sesno).await?;
        let store = ModelMemoryStore::global();
        let mesh_dir = self.mesh_dir.as_path();
        let mut report = RigidMotionReport::default();
        for unit in units {
            let root_refno = &unit.root_refno;
            let anchors = unit.anchors.as_slice();
            let root = match parse_refno(root_refno) {
                Ok(root) => root,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("unparsable root: {error:#}")));
                    continue;
                }
            };
            // 前置：投影完整（记录数与回执相符、mesh 文件都在）且有旧回执可前移。
            if !store.root_projection_complete(ProjectionScope::Current, root, mesh_dir) {
                report
                    .fallbacks
                    .push((root_refno.clone(), "projection incomplete".into()));
                continue;
            }
            let Some(old_receipt) = store.receipt(ProjectionScope::Current, root) else {
                report
                    .fallbacks
                    .push((root_refno.clone(), "no cached receipt".into()));
                continue;
            };
            // 幂等闸（S4b 审核 P0 / mem-42）：只有「投影还停在规划基线 S」才许左乘。
            match republish_gate(
                old_receipt.source_sesno,
                unit.source_start_sesno,
                target_sesno,
            ) {
                RepublishGate::Proceed => {}
                RepublishGate::SkipAlreadyAtTarget => {
                    report.replayed.push(root_refno.clone());
                    continue;
                }
                RepublishGate::FallbackBaselineMismatch => {
                    report.fallbacks.push((
                        root_refno.clone(),
                        format!(
                            "projection receipt at sesno {}, but Δ was planned against baseline \
                             {} (target {}); falling back to regen",
                            old_receipt.source_sesno, unit.source_start_sesno, target_sesno
                        ),
                    ));
                    continue;
                }
            }
            let records = store.records_of_root(ProjectionScope::Current, root);
            // 左乘 Δ：纯平移直接加 t；含旋转让盒闭包报错 → 回退（读网格那半边留作 B6 后续）。
            let moved = match republish_records(records, anchors, target_sesno, |_, _| {
                anyhow::bail!("rotation bounds read not yet wired; falling back to regen")
            }) {
                Ok(moved) => moved,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("{error:#}")));
                    continue;
                }
            };
            let (manifest_digest, geometry_count) = match manifest_hash(&moved) {
                Ok(pair) => pair,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("manifest hash: {error:#}")));
                    continue;
                }
            };
            // 回执整根前移到 T；`model_target` 暂沿用旧的（见函数头「待补」）。发布调用只同步
            // 提交内存投影并登记 write-behind；后台 worker 独立完成 durable flush。
            super::model_write_behind::publish(vec![ProjectionCommit {
                upserts: moved,
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: target_sesno,
                        model_target: old_receipt.model_target.clone(),
                        manifest_hash: manifest_digest,
                        geometry_count,
                        published_via: "transform".into(),
                    },
                )),
                ..ProjectionCommit::default()
            }])?;
            report.completed.push(root_refno.clone());
        }
        Ok(report)
    }

    pub async fn generate_dbnum(
        &self,
        dbnum: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let input = generation_input(self, dbnum, source_sesno)?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &input.set,
            &input.index,
            &input.index.roots,
            failure_policy,
            &input.dependency_sessions,
            None,
        )
        .await
    }

    /// Generate one root at an exact source session without reading or writing
    /// the persisted model projection.
    ///
    /// A root that the session's element index does not contain fails **before**
    /// geometry evaluation with [`SnapshotSourceError::RootNotInSession`], so a
    /// caller can tell "this refno did not exist at that session" apart from
    /// "its geometry could not be generated" by type instead of by message text.
    pub async fn generate_snapshot_source(
        &self,
        dbnum: u32,
        root: RefNo,
        source_sesno: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<GeneratedSnapshot> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let input = generation_input(self, dbnum, source_sesno)?;
        if !input.index.owners.contains_key(&(root.word0, root.word1)) {
            return Err(SnapshotSourceError::RootNotInSession {
                root,
                dbnum,
                sesno: source_sesno,
            }
            .into());
        }
        let mut cache = CatalogueMeshCache::default();
        let snapshot = generate_snapshot_from_set(&input.set, &input.index, root, &mut cache)?;
        if !snapshot.report.failed.is_empty()
            && matches!(failure_policy, GeometryFailurePolicy::Required)
        {
            bail!(
                "e3d-model root {root} has {} failed element(s)",
                snapshot.report.failed.len()
            );
        }
        Ok(snapshot)
    }

    pub(crate) fn source_file(&self, dbnum: u32) -> anyhow::Result<&Path> {
        self.pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .map(|pin| pin.file.as_path())
            .with_context(|| format!("MDB has no source file for dbnum {dbnum}"))
    }

    pub(crate) fn from_source_files(
        pins: Vec<DbPin>,
        locator: InMemoryCataLocator,
        mesh_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            schema: DirectSchema::open_from_env()?,
            pins,
            locator,
            project_files: Arc::new(HashMap::new()),
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir,
            generation_identity: None,
            frozen_file_digests: BTreeMap::new(),
        })
    }

    pub async fn apply_window(
        &self,
        dbnum: u32,
        base_sesno: u32,
        target_sesno: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        // The window's right end is an explicit timepoint (ADR-054 Q2): open the
        // file at exactly that session instead of demanding that the current pin
        // happens to equal it. A newer session already published for this
        // database means the window has been superseded, not that it failed.
        if let Some(newer) = persisted_session_newer_than(dbnum, target_sesno).await? {
            log::info!(
                "e3d-model window {base_sesno}..{target_sesno} for dbnum {dbnum} is already covered by published session {newer}; nothing to apply"
            );
            return Ok(E3dPersistReport::default());
        }
        let window = collect_window(&pin.file, base_sesno, target_sesno)?;
        let base = self.build_set(dbnum, Some(base_sesno))?;
        let target = self.build_set(dbnum, Some(target_sesno))?;
        let outcome = increment_update(&base, &target, &window);
        let failed = increment_failure_count(&outcome.report);
        let generation_report = serde_json::to_value(&outcome.report)?;
        if failed > 0 {
            if matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model incremental generation failed for {failed} element(s)");
            }
            // An incremental delta is only a candidate until every affected
            // element was evaluated. Publishing a partial delta could delete
            // valid geometry from the last complete snapshot.
            return Ok(E3dPersistReport {
                failed,
                generation_report,
                ..E3dPersistReport::default()
            });
        }
        let index = scan_index(&pin.file, Some(target_sesno))?;
        if let Some(newer) = persisted_session_newer_than(dbnum, target_sesno).await? {
            log::info!(
                "e3d-model window {base_sesno}..{target_sesno} for dbnum {dbnum} was superseded by published session {newer} during evaluation; discarding the delta"
            );
            return Ok(E3dPersistReport {
                generation_report,
                ..E3dPersistReport::default()
            });
        }
        let mut report = apply_geometry_delta(
            dbnum,
            target_sesno,
            outcome.upserts,
            outcome.removals,
            &index.owners,
            &self.mesh_dir,
        )
        .await?;
        report.failed += failed;
        report.generation_report = generation_report;
        Ok(report)
    }

    async fn generate_refs(
        &self,
        dbnum: u32,
        source_sesno: u32,
        set: &Arc<DbSet>,
        index: &SourceIndex,
        roots: &[RefNo],
        failure_policy: GeometryFailurePolicy,
        dependency_sessions: &DependencySessions,
        mut memory_snapshots: Option<&mut Vec<(RefNo, BakedSnapshot)>>,
    ) -> anyhow::Result<E3dPersistReport> {
        let store = ModelMemoryStore::global();
        let mut total = E3dPersistReport {
            source_sesno: Some(source_sesno),
            ..E3dPersistReport::default()
        };
        let mut generation = Report::default();
        let mut commits = Vec::new();
        let mut prepared_reports = Vec::new();
        // 上一轮的回执要先读齐、依赖会话要先补齐，本轮的目标才算得准——目标里带着每个
        // CATA 依赖的文件身份，逐根现算会让先算的那几根少看见后面才发现的依赖。
        let published = roots
            .iter()
            .copied()
            .map(|root| store.receipt(ProjectionScope::Current, root))
            .collect::<Vec<_>>();
        let cohort_was_empty = published.iter().all(Option::is_none);
        for receipt in published.iter().flatten() {
            self.hydrate_published_dependencies(receipt, dependency_sessions)?;
        }
        let model_target = self.model_target(dbnum, source_sesno, dependency_sessions)?;
        let publications = roots
            .iter()
            .copied()
            .zip(published)
            .map(|(root, published)| {
                let mut publication = RootPublication {
                    root,
                    model_target: Some(model_target.clone()),
                    published,
                    projection_complete: false,
                };
                publication.projection_complete =
                    cached_root_target_matches(&publication, source_sesno)
                        && store.root_projection_complete(
                            ProjectionScope::Current,
                            root,
                            &self.mesh_dir,
                        );
                publication
            })
            .collect::<Vec<_>>();
        let cached = publications
            .iter()
            .map(|publication| publication.projection_complete)
            .collect::<Vec<_>>();
        let prior = roots
            .iter()
            .copied()
            .zip(cached.iter().copied())
            .map(|(root, cached)| {
                if cached {
                    Vec::new()
                } else {
                    store.geometry_ids_of_root(ProjectionScope::Current, dbnum, root)
                }
            })
            .collect::<Vec<_>>();
        // A production batch is one reuse window: identical evaluated catalogue
        // primitives across different delivery roots must share their local mesh.
        let mut catalogue_mesh_cache = CatalogueMeshCache::default();
        for (root_index, &root) in roots.iter().enumerate() {
            let mut publication = publications[root_index].clone();
            if cached[root_index] {
                // 目标与上一轮逐位相同，投影也还完整：只重盖一次章，几何一条都不动。
                commits.push(cached_root_commit(&publication, source_sesno)?);
                prepared_reports.push(E3dPersistReport::default());
                continue;
            }
            let incomplete = incomplete_root_policy(
                publication.published.is_some(),
                !prior[root_index].is_empty(),
            );
            let snapshot = if let Some(snapshots) = memory_snapshots.as_deref_mut() {
                let index = snapshots
                    .iter()
                    .position(|(candidate, _)| *candidate == root)
                    .with_context(|| format!("memory batch missing root {root}"))?;
                snapshots.swap_remove(index).1
            } else {
                match generate_snapshot_from_set(set, index, root, &mut catalogue_mesh_cache)
                    .and_then(|snapshot| bake_snapshot(snapshot, &self.mesh_dir))
                {
                    Ok(outcome) => outcome,
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        total.failed += 1;
                        total.failed_roots.push((root, format!("{error:#}")));
                        log::error!("e3d-model root {root} failed: {error:#}");
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| format!("generate root {root}"));
                    }
                }
            };
            let root_failed = snapshot.report.failed.len();
            if root_failed > 0 && matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model root {root} has {root_failed} failed element(s)");
            }
            let failure = (root_failed > 0).then(|| {
                eprintln!(
                    "e3d-model root {root} failed geometry details: {:?}",
                    snapshot.report.failed
                );
                format!(
                    "{root_failed} geometry element(s) failed: {:?}",
                    snapshot.report.failed
                )
            });
            if let Some(failure) = failure {
                if incomplete == IncompleteRoot::Discard {
                    // Best-effort generation may report successful siblings next
                    // to failed elements. Publishing that partial manifest would
                    // delete the missing GeometryIds, so retain the previous
                    // complete root.
                    let root_skipped = snapshot.report.skipped.len();
                    generation.merge(snapshot.report);
                    total.failed += root_failed;
                    total.failed_roots.push((root, failure));
                    total.skipped += root_skipped;
                    log::error!(
                        "e3d-model root {root} produced an incomplete candidate ({root_failed} failure(s)); preserving published snapshot"
                    );
                    continue;
                }
                // Published partial: the root lands with the elements that did
                // evaluate, and says so.  `failed` still counts every element
                // that did not, so `/health` and the ensure receipt keep the
                // omission on the books instead of calling this a clean root.
                log::warn!(
                    "e3d-model root {root} publishing {} element(s) of an incomplete candidate; {root_failed} did not evaluate",
                    snapshot.elements.len()
                );
                total.partial_roots.push((root, failure));
            }
            let root_skipped = snapshot.report.skipped.len();
            // 子树访问名单只报出去，不在这里写库：读透形态下编排层拿它在发布模型行之前
            // 把子树元素写透（`docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` §3.2）。
            total.visited_by_root.insert(root, snapshot.visited);
            let generated_ids = snapshot
                .elements
                .iter()
                .map(|element| {
                    Ok((
                        serde_json::to_string(&element.geometry_id)?,
                        element.geometry_id.clone(),
                    ))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            let existing_ids = &prior[root_index];
            let removals = existing_ids
                .iter()
                .cloned()
                .filter_map(|geometry_id| {
                    let key = serde_json::to_string(&geometry_id).ok()?;
                    (!generated_ids.contains_key(&key)).then_some(geometry_id)
                })
                .collect::<BTreeSet<_>>();
            generation.merge(snapshot.report);
            // Catalogue resolution during generation may have discovered
            // cross-database dependencies that were not in watermark pins.
            publication.model_target =
                Some(self.model_target(dbnum, source_sesno, dependency_sessions)?);
            let (commit, mut projected) = project_geometry_delta(
                dbnum,
                source_sesno,
                snapshot.elements,
                removals.into_iter().collect(),
                Some(&publication),
                &index.owners,
            )?;
            projected.skipped += root_skipped;
            projected.failed += root_failed;
            commits.push(commit);
            prepared_reports.push(projected);
        }
        if total.failed == 0 || cohort_was_empty {
            // One generate_roots call is one projection cohort: an OWNER move
            // (old root cleanup + new root takeover) and a PIPE expansion
            // containing several BRANs land in the store together. A cold
            // process store has no old ownership to preserve, so complete
            // siblings may still land when another root cannot be generated.
            super::model_write_behind::publish(commits)?;
            for report in prepared_reports {
                total.merge_counts(report);
            }
        } else {
            // On a warm projection, committing only the roots that happened to
            // generate could delete an element from its old root while its new
            // (failed) root never publishes it. Keep the established cohort
            // atomic and report every held sibling explicitly instead of
            // letting an upper layer mistake an uncommitted candidate for
            // success.
            let failed = total
                .failed_roots
                .iter()
                .map(|(root, _)| (root.word0, root.word1))
                .collect::<BTreeSet<_>>();
            for root in roots
                .iter()
                .copied()
                .filter(|root| !failed.contains(&(root.word0, root.word1)))
            {
                total.failed += 1;
                total.failed_roots.push((
                    root,
                    "projection cohort held because a peer root failed".into(),
                ));
            }
        }
        total.generation_report = serde_json::to_value(generation)?;
        Ok(total)
    }

    fn pin(&self, dbnum: u32) -> anyhow::Result<&DbPin> {
        self.pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .with_context(|| format!("current MDB has no source file for dbnum {dbnum}"))
    }

    /// 当前 MDB 里 `dbnum` 那份设计文件的路径与库类型。读透（`data_interface::read_through`）
    /// 要拿它反查 last-touch 会话——与生成读的是同一个文件。
    pub(crate) fn source_pin(&self, dbnum: u32) -> anyhow::Result<(PathBuf, String)> {
        let pin = self.pin(dbnum)?;
        Ok((pin.file.clone(), pin.db_type.clone()))
    }

    /// Every design database of the current MDB as `(dbnum, file, source session)`,
    /// the session being the one generation would read right now (ADR-054 Q1).
    /// Derived faces that must not depend on parsed `pe` rows (room topology,
    /// `room_topology`) enumerate the files through this.
    pub(crate) fn design_sources(&self) -> anyhow::Result<Vec<(u32, PathBuf, u32)>> {
        self.pins
            .iter()
            .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
            .map(|pin| {
                Ok((
                    pin.dbnum as u32,
                    pin.file.clone(),
                    self.source_session(pin)?,
                ))
            })
            .collect()
    }

    /// The session a database is generated from (ADR-054 Q1): an explicit pin
    /// wins; an unpinned database means the file's latest session right now.
    fn source_session(&self, pin: &DbPin) -> anyhow::Result<u32> {
        match pin.sesno {
            Some(sesno) => Ok(sesno),
            None => Ok(model_source::latest_source_version_of_file(
                pin.dbnum as u32,
                &pin.db_type,
                &pin.file,
            )?
            .sesno),
        }
    }

    fn model_target(
        &self,
        dbnum: u32,
        source_sesno: u32,
        dependency_sessions: &DependencySessions,
    ) -> anyhow::Result<ModelTarget> {
        let design_pin = self.pin(dbnum)?;
        let sessions = dependency_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let file_target = |dbnum: u32,
                           db_type: &str,
                           file: &Path,
                           session: u32|
         -> anyhow::Result<ModelFileTarget> {
            Ok(ModelFileTarget {
                dbnum,
                db_type: db_type.to_string(),
                file: file.to_string_lossy().into_owned(),
                session,
                digest: match self.frozen_file_digests.get(&(dbnum, session)) {
                    Some(digest) => digest.clone(),
                    None => model_file_identity_digest(file, session)?,
                },
            })
        };
        let design = file_target(
            design_pin.dbnum as u32,
            &design_pin.db_type,
            &design_pin.file,
            source_sesno,
        )?;
        let mut catalogue = BTreeMap::new();
        for pin in self
            .pins
            .iter()
            .filter(|pin| pin.db_type.eq_ignore_ascii_case("CATA"))
        {
            let session = sessions
                .get(&(dbnum, pin.dbnum as u32))
                .copied()
                .or(pin.sesno)
                .unwrap_or_default();
            catalogue.insert(
                pin.dbnum as u32,
                file_target(pin.dbnum as u32, &pin.db_type, &pin.file, session)?,
            );
        }
        for (&(target_dbnum, dependency_dbnum), &session) in sessions.iter() {
            if target_dbnum != dbnum {
                continue;
            }
            if catalogue.contains_key(&dependency_dbnum) {
                continue;
            }
            let Some(selected) = self.dependency_file(dbnum, dependency_dbnum) else {
                continue;
            };
            if !selected.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            catalogue.insert(
                dependency_dbnum,
                file_target(dependency_dbnum, &selected.db_type, &selected.path, session)?,
            );
        }
        let project = self
            .locator
            .file_of(dbnum)
            .map(|(project, _)| project)
            .unwrap_or_default();
        Ok(ModelTarget {
            project,
            design,
            catalogue: catalogue.into_values().collect(),
            template_attlib_digest: model_file_identity_digest(
                &self.schema.template_dir().join("attlib.dat"),
                0,
            )?,
            generator_fingerprint: format!(
                "e3d-model:{};aios-database:{};route-atta-pass-through-v1;route-atta-anci-only-v1;catalogue-atta-paxi-frame-v2;catalogue-route-scyl-paxi-frame-v1;catalogue-pml-sqrt-atan-v1;catalogue-pml-flex-mask-v1;catalogue-lpyr-negative-pbax-v1;section-fitt-posl-frame-v1;catalogue-cylinder-cuts-v1;design-cyli-centered-z-v1;design-snout-revolution-facets-v1;design-negative-ctor-grid-v2;polyhedron-member-order-v1;pohe-polygon-panels-v1;cable-tray-missing-rpath-origin-v1;section-finite-empty-length-v1;explicit-word-angle-units-v1",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_VERSION")
            ),
            tessellation_profile: "e3d-default-v1".to_string(),
        })
    }

    fn hydrate_published_dependencies(
        &self,
        receipt: &RootReceipt,
        dependency_sessions: &DependencySessions,
    ) -> anyhow::Result<()> {
        let Some(published) = receipt.model_target.as_ref() else {
            return Ok(());
        };
        let mut sessions = dependency_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for dependency in &published.catalogue {
            let key = (published.design.dbnum, dependency.dbnum);
            if sessions.contains_key(&key) {
                continue;
            }
            let Some(selected) = self.dependency_file(published.design.dbnum, dependency.dbnum)
            else {
                continue;
            };
            let mut view = DbView::open(&selected.path)?;
            if let Some(session) = view.walk_sessions()?.first() {
                sessions.insert(key, session.sesno);
            }
        }
        Ok(())
    }

    fn dependency_file(&self, target_dbnum: u32, dependency_dbnum: u32) -> Option<ProjectDbFile> {
        let preferred_project = self
            .locator
            .file_of(target_dbnum)
            .map(|(project, _)| project);
        select_dependency_file(
            &self.locator,
            &self.project_files,
            preferred_project.as_deref(),
            dependency_dbnum,
        )
    }

    /// Open the MDB's DESI files as one `DbSet`, pinning `target_dbnum` at
    /// `target_sesno` (`None` = the file's latest session) and every other
    /// database at its current pin.  Catalogue databases resolve lazily through
    /// `E3dDbResolver`.  The increment planner uses it for the `DbSet@S` /
    /// `DbSet@T` pair (`window_root_plan`).
    pub(crate) fn build_set(
        &self,
        target_dbnum: u32,
        target_sesno: Option<u32>,
    ) -> anyhow::Result<Arc<DbSet>> {
        let preferred_project = self
            .locator
            .file_of(target_dbnum)
            .map(|(project, _)| project);
        let set = Arc::new(DbSet::with_attlib_file_and_resolver(
            self.schema.template_dir().join("attlib.dat"),
            Box::new(E3dDbResolver {
                locator: self.locator.clone(),
                project_files: self.project_files.clone(),
                target_dbnum,
                preferred_project,
                template_dir: self.schema.template_dir().to_path_buf(),
                sessions: self.dependency_sessions.clone(),
            }),
        )?);
        for pin in &self.pins {
            // CATA identity belongs to the project-aware locator.  Preloading a
            // watermark path here can shadow the resolver with a same-dbnum file
            // from another project (for example APS7600 vs ZDJ7600).
            if pin.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            let sesno = if pin.dbnum == target_dbnum as i32 {
                target_sesno
            } else {
                pin.sesno
            };
            set.add_db(DbFilePin {
                file: pin.file.clone(),
                template: template_file_for(self.schema.template_dir(), &pin.db_type)?,
                db_type: Some(pin.db_type.clone()),
                sesno,
            })?;
        }
        Ok(set)
    }
}

/// Resolve the configured MDB once into the source pins and dependency locator
/// used by both current and historical generation.  Current projection replaces
/// the returned pins with watermark-pinned DESI pins, while historical generation
/// keeps these paths and selects a session per request.
fn current_mdb_source_inventory()
-> anyhow::Result<(Vec<DbPin>, InMemoryCataLocator, ProjectDbFiles)> {
    let option = aios_core::get_db_option();
    let project = option.project_name.clone();
    let mdb = option.mdb_name.clone();
    let membership = mdb_membership::get(&project, &mdb)
        .map(Ok)
        .unwrap_or_else(|| {
            mdb_membership::resolve(option, &project, &mdb).map(std::sync::Arc::new)
        })?;
    let watch_dirs = crate::data_interface::project_paths::plan_watch_dirs(option);
    crate::data_interface::project_paths::record_watch_dir_owners(&watch_dirs);
    let mut files = HashMap::new();
    let mut project_files = HashMap::new();
    let mut pins = Vec::new();
    for item in membership.databases() {
        let Some(path) = item.path.clone() else {
            continue;
        };
        let Some(db_type) = styp_name(item.styp) else {
            continue;
        };
        let source_project = item.project.clone().unwrap_or_else(|| project.clone());
        project_files
            .entry(project_db_key(&source_project, item.dbnum))
            .or_insert_with(|| ProjectDbFile {
                db_type: db_type.to_string(),
                project: source_project.clone(),
                path: path.clone(),
            });
        for shadowed in &item.shadowed {
            let Ok(header) = crate::data_interface::db_file_header::DbFileHeader::read(shadowed)
            else {
                continue;
            };
            if header.dbnum != item.dbnum as i32 || !header.db_type.eq_ignore_ascii_case(db_type) {
                continue;
            }
            let Some(shadowed_project) =
                crate::data_interface::project_paths::project_of_path(shadowed)
            else {
                continue;
            };
            project_files
                .entry(project_db_key(&shadowed_project, item.dbnum))
                .or_insert_with(|| ProjectDbFile {
                    db_type: header.db_type,
                    project: shadowed_project,
                    path: shadowed.clone(),
                });
        }
        files.insert(
            item.dbnum,
            (db_type.to_string(), source_project, path.clone()),
        );
        pins.push(DbPin {
            dbnum: item.dbnum as i32,
            db_type: db_type.to_string(),
            file: path,
            sesno: None,
        });
    }
    Ok((
        pins,
        InMemoryCataLocator::from_parts(HashMap::new(), files),
        Arc::new(project_files),
    ))
}

pub(crate) fn current_mdb_sources() -> anyhow::Result<(Vec<DbPin>, InMemoryCataLocator)> {
    let (pins, locator, _) = current_mdb_source_inventory()?;
    Ok((pins, locator))
}

fn styp_name(value: i64) -> Option<&'static str> {
    match value {
        1 => Some("DESI"),
        2 => Some("CATA"),
        4 => Some("PROP"),
        6 => Some("ISOD"),
        7 => Some("PADD"),
        8 => Some("DICT"),
        9 => Some("ENGI"),
        14 => Some("SCHE"),
        _ => None,
    }
}

/// The requested root itself is absent from the generator's pinned source.
/// A missing catalogue/owner dependency is deliberately not this outcome.
#[derive(Debug)]
pub(crate) struct GenerationRootMissing {
    pub root: RefNo,
    pub dbnum: u32,
    pub source_sesno: u32,
    source: anyhow::Error,
}

impl std::fmt::Display for GenerationRootMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "generation root {} is absent from db {} session {}",
            self.root, self.dbnum, self.source_sesno)
    }
}
impl std::error::Error for GenerationRootMissing {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn classify_required_root_error(root: RefNo, error: anyhow::Error) -> anyhow::Error {
    let absent = error.chain().find_map(|cause| {
        match cause.downcast_ref::<e3d_io::db_element::DbElementError>() {
            Some(e3d_io::db_element::DbElementError::NotFound { refno, dbno, sesno })
                if *refno == root => Some((*dbno, *sesno)),
            _ => None,
        }
    });
    if let Some((dbnum, source_sesno)) = absent {
        return GenerationRootMissing { root, dbnum, source_sesno, source: error }.into();
    }
    error
}

fn generate_snapshot_from_set(
    set: &Arc<DbSet>,
    index: &SourceIndex,
    root: RefNo,
    cache: &mut CatalogueMeshCache,
) -> anyhow::Result<GeneratedSnapshot> {
    let outcome = generate_subtree_with_cache(set.element(root), cache)
        .with_context(|| format!("generate root {root}"))?;
    Ok(GeneratedSnapshot {
        elements: outcome.elements,
        owners: Arc::clone(&index.owners),
        report: outcome.report,
        visited: outcome.visited,
    })
}

/// One evaluated element, reduced to what publication actually needs.
///
/// Triangulation, the SHA-256 content id and the mesh file write all happen
/// while building this, so the `Manifold` — a world-space f64 CSG solid, the
/// fattest representation on the chain — dies where it was computed instead of
/// riding into the serial publication step.  What is left is a mesh id, two
/// boxes and the element's own metadata.
pub struct BakedElement {
    pub geometry_id: GeometryId,
    pub refno: RefNo,
    pub noun: String,
    pub is_invalid_tubi: bool,
    pub world: DMat4,
    pub primitive_instance: Option<e3d_model::primitive_instance::PrimitiveInstance>,
    /// Content-addressed id; the file is already published under it.
    pub mesh_id: String,
    pub mesh_write: MeshWrite,
    pub world_bounds: [[f32; 3]; 2],
    /// Only a canonical primitive persists a local box.
    pub local_bounds: Option<[[f32; 3]; 2]>,
}

/// A generated root after baking.  It carries no `owners`: publication reads
/// the batch's own source index, and the single-root historical path keeps
/// using [`GeneratedSnapshot`].
pub struct BakedSnapshot {
    pub elements: Vec<BakedElement>,
    pub report: Report,
    /// 随 [`GeneratedSnapshot::visited`] 原样带过来；烤制不改子树。
    pub visited: Vec<RefNo>,
}

/// Triangulate, hash and publish one element's mesh.
///
/// Baking here means a root that later fails its `Required` check — or whose
/// publication transaction rolls back — leaves its mesh on disk.  That is the
/// content-addressed contract rather than a new risk: the id *is* the content
/// hash, so an orphan is simply unreferenced and waits for reachability GC.
fn bake_element(element: GeneratedElement, mesh_dir: &Path) -> anyhow::Result<BakedElement> {
    let world_bounds = {
        let world_mesh = element.parts.as_deref().map_or_else(
            || crate::fast_model::manifold_csg::manifold_to_plant_mesh(&element.solid),
            crate::fast_model::manifold_csg::manifolds_to_plant_mesh,
        );
        let aabb = world_mesh.aabb.context("world PlantMesh missing AABB")?;
        [point3(&aabb.mins), point3(&aabb.maxs)]
    };
    let (mesh_id, mesh_write, local_bounds) = match element.primitive_instance.as_ref() {
        Some(instance) => {
            let canonical = canonical_primitive_mesh(instance.key)?;
            let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical);
            let mesh_id = canonical_mesh_id(instance.key, &mesh);
            let aabb = mesh.aabb.context("canonical PlantMesh missing AABB")?;
            let bounds = [point3(&aabb.mins), point3(&aabb.maxs)];
            let write = ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?;
            (mesh_id, write, Some(bounds))
        }
        None => {
            let mesh = local_generated_mesh(&element);
            let mesh_id = baked_mesh_id(&mesh);
            let write = ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?;
            (mesh_id, write, None)
        }
    };
    Ok(BakedElement {
        geometry_id: element.geometry_id,
        refno: element.refno,
        noun: element.noun,
        is_invalid_tubi: element.is_invalid_tubi,
        world: element.world,
        primitive_instance: element.primitive_instance,
        mesh_id,
        mesh_write,
        world_bounds,
        local_bounds,
    })
}

/// 失败元素的有界摘要：按 noun 聚合，再带前几条的 refno / noun / detail。
///
/// 整份 `{:?}` 在 7022（CABLETMPL）那种一根 4548 个失败元素的库上会把一行日志和
/// 失败账本条目撑到几百 KB；而只写「N 个失败」又定位不了任何东西——run-ad 的 4,089 个
/// FailedHold 就是这样查不下去的。
pub(crate) fn summarize_geometry_incidents(failed: &[e3d_model::pipeline::Incident]) -> String {
    const SAMPLES: usize = 5;
    const NOUNS: usize = 8;
    const DETAIL_CHARS: usize = 160;
    let mut by_noun = BTreeMap::<&str, usize>::new();
    for incident in failed {
        *by_noun.entry(incident.noun.as_str()).or_default() += 1;
    }
    let mut nouns: Vec<(&str, usize)> = by_noun.into_iter().collect();
    nouns.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let more = nouns.len().saturating_sub(NOUNS);
    let mut nouns = nouns
        .iter()
        .take(NOUNS)
        .map(|(noun, count)| format!("{noun}×{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    if more > 0 {
        nouns.push_str(&format!(", +{more} nouns"));
    }
    let samples = failed
        .iter()
        .take(SAMPLES)
        .map(|incident| {
            let mut detail: String = incident.detail.chars().take(DETAIL_CHARS).collect();
            if incident.detail.chars().count() > DETAIL_CHARS {
                detail.push('…');
            }
            format!("{} {}: {detail}", incident.refno, incident.noun)
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "by noun [{nouns}]; first {}: {samples}",
        failed.len().min(SAMPLES)
    )
}

fn bake_elements(
    elements: Vec<GeneratedElement>,
    mesh_dir: &Path,
) -> anyhow::Result<Vec<BakedElement>> {
    elements
        .into_iter()
        .map(|element| bake_element(element, mesh_dir))
        .collect()
}

/// What a root whose elements did not all evaluate is worth publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncompleteRoot {
    /// Throw the whole candidate away.  Publishing a partial manifest deletes
    /// the GeometryIds it omits, so a root that already published keeps what it
    /// has; the baked survivors become orphans for the reachability GC.
    Discard,
    /// Publish whatever evaluated.  Only for a root with nothing published to
    /// lose: there the discard preserves an empty snapshot, which is how one
    /// dangling catalogue item hides an entire branch.
    Publish,
}

/// A partial manifest deletes the GeometryIds it omits, so publishing one is
/// only safe for a root with nothing to lose: no publication receipt **and** no
/// geometry rows.  Everywhere else the incomplete candidate is dropped and the
/// established snapshot stands.
///
/// The cold case is not a corner: on a read-through store every first display is
/// cold, and dropping the candidate there preserves an empty snapshot — which is
/// how one dangling catalogue item hides a whole branch (SLYK `21491/17101`: one
/// FLAN whose `Spref` points at a catalogue element the project never had, and
/// the other 16 members of the branch never reach the viewport).
fn incomplete_root_policy(published: bool, has_geometry_rows: bool) -> IncompleteRoot {
    if published || has_geometry_rows {
        IncompleteRoot::Discard
    } else {
        IncompleteRoot::Publish
    }
}

/// Bake a generated root for publication.
///
/// An incomplete root is baked like any other: whether its survivors are worth
/// publishing is [`incomplete_root_policy`]'s call, and that needs publication
/// state the producer half of the memory pipeline deliberately cannot see
/// (`memory_generation_reads_only_the_e3d_source_projection`).  Baking here and
/// deciding there costs a discarded candidate its triangulation — its meshes
/// are content-addressed, so a root that published before rewrites nothing —
/// and it is the only order in which both halves can reach the same verdict.
fn bake_snapshot(snapshot: GeneratedSnapshot, mesh_dir: &Path) -> anyhow::Result<BakedSnapshot> {
    let elements = bake_elements(snapshot.elements, mesh_dir)?;
    Ok(BakedSnapshot {
        elements,
        report: snapshot.report,
        visited: snapshot.visited,
    })
}

impl E3dPersistReport {
    fn merge_counts(&mut self, other: Self) {
        self.upserted += other.upserted;
        self.removed += other.removed;
        self.skipped += other.skipped;
        self.failed += other.failed;
        self.shared_instances += other.shared_instances;
        self.baked_instances += other.baked_instances;
        self.mesh_reused += other.mesh_reused;
        self.mesh_written += other.mesh_written;
        self.mesh_ids.extend(other.mesh_ids);
        self.failed_roots.extend(other.failed_roots);
        self.partial_roots.extend(other.partial_roots);
        self.unique_meshes = self.mesh_ids.len();
        self.source_sesno = self.source_sesno.or(other.source_sesno);
        self.visited_by_root.extend(other.visited_by_root);
    }
}

pub async fn apply_geometry_delta(
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    apply_geometry_delta_on(
        ProjectionScope::Current,
        dbnum,
        source_sesno,
        upserts,
        removals,
        owners,
        mesh_dir,
    )
    .await
}

/// 目标没变、投影也还完整时这一根的全部改动：只重盖一次章。
///
/// 旧版这里要发一条根凭证的 `published_*` 更新，外加一批依赖收据行；现在收据就是回执本身，
/// 依赖清单在 `model_target.catalogue` 里带着。
fn cached_root_commit(
    publication: &RootPublication,
    source_sesno: u32,
) -> anyhow::Result<ProjectionCommit> {
    let published = publication
        .published
        .as_ref()
        .context("cached projection has no receipt")?;
    Ok(ProjectionCommit {
        receipt: Some((
            publication.root,
            RootReceipt {
                source_sesno,
                model_target: publication.model_target.clone(),
                manifest_hash: published.manifest_hash.clone(),
                geometry_count: published.geometry_count,
                published_via: "regen".into(),
            },
        )),
        ..ProjectionCommit::default()
    })
}

/// 这一根能不能走「不必重算」那条路：源会话与不可变目标都必须与上一轮逐位相同。
fn cached_root_target_matches(publication: &RootPublication, source_sesno: u32) -> bool {
    let Some(published) = publication.published.as_ref() else {
        return false;
    };
    if published.source_sesno != source_sesno {
        return false;
    }
    let Some(current) = publication.model_target.as_ref() else {
        return false;
    };
    published.model_target.as_ref() == Some(current)
}

/// 把一次增量投进指定投影域。历史快照与当前投影共用这一条。
pub(crate) async fn apply_geometry_delta_on(
    scope: ProjectionScope<'_>,
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    let (commit, mut report) = project_geometry_delta(
        dbnum,
        source_sesno,
        bake_elements(upserts, mesh_dir)?,
        removals,
        None,
        owners,
    )?;
    match scope {
        ProjectionScope::Current => {
            super::model_write_behind::publish(vec![commit])?;
        }
        ProjectionScope::Historical(_) => ModelMemoryStore::global().commit(scope, vec![commit]),
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok(report)
}

/// 把一次增量折成投影存储要的一笔改动。
///
/// 这里点名的每个 mesh 都已经在盘上了（见 [`bake_element`]），所以这一步只做结构化与记账，
/// 可以留在串行的消费端而不把几何 CPU 压在它后面。
fn project_geometry_delta(
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<BakedElement>,
    removals: Vec<GeometryId>,
    publication: Option<&RootPublication>,
    owners: &BTreeMap<(u32, u32), RefNo>,
) -> anyhow::Result<(ProjectionCommit, E3dPersistReport)> {
    let mut report = E3dPersistReport::default();
    let mut commit = ProjectionCommit::default();
    // 一条 BRAN 的隐式管身是整段重写的：碰到的分支先清空旧管身，再插这一轮的。
    commit.tube_branch_resets = removals
        .iter()
        .chain(upserts.iter().map(|element| &element.geometry_id))
        .filter_map(|geometry_id| match geometry_id {
            GeometryId::ImpliedTube {
                container_refno, ..
            } => Some(container_refno.as_str()),
            GeometryId::Element { .. } => None,
        })
        .map(|value| parse_refno(value).map(|refno| (refno.word0, refno.word1)))
        .collect::<anyhow::Result<BTreeSet<_>>>()?
        .into_iter()
        .map(|(word0, word1)| RefNo::new(word0, word1))
        .collect();

    for geometry_id in &removals {
        let source = geometry_source_refno(geometry_id)?;
        commit
            .removals
            .push(geometry_record_id(geometry_id, source));
        report.removed += 1;
    }

    for element in upserts {
        let source_refno = geometry_source_refno(&element.geometry_id)?;
        let record_id = geometry_record_id(&element.geometry_id, source_refno);
        let ancestors = ancestor_chain(source_refno, owners)?;
        // 隐式管身只向 Plant UI 暴露一个变换，所以标准管身要把元素摆放与标准网格的
        // 缩放 / 居中折进同一个存下来的世界变换里。
        let world_transform = persisted_world_transform(
            &element.geometry_id,
            element.primitive_instance.as_ref(),
            element.world,
        );
        note_mesh_write(&mut report, element.mesh_write);
        report.mesh_ids.insert(element.mesh_id.clone());

        let shape = match element.primitive_instance.as_ref() {
            Some(instance) => {
                report.shared_instances += 1;
                ProjectedShape::Canonical {
                    key: instance.key,
                    local_transform: instance.local_transform,
                    local_bounds: element
                        .local_bounds
                        .context("canonical primitive baked without local bounds")?,
                }
            }
            None => {
                report.baked_instances += 1;
                ProjectedShape::Baked
            }
        };
        let tube = match &element.geometry_id {
            GeometryId::ImpliedTube {
                route_ordinal,
                from_refno,
                to_refno,
                ..
            } => Some(ProjectedTube {
                route_ordinal: *route_ordinal,
                from: parse_refno(from_refno)?,
                to: parse_refno(to_refno)?,
                invalid: element.is_invalid_tubi,
            }),
            GeometryId::Element { .. } => None,
        };

        commit.upserts.push(ProjectedGeometry {
            record_id,
            geometry_id: element.geometry_id,
            source_refno,
            dbnum,
            source_sesno,
            noun: element.noun,
            mesh_id: element.mesh_id,
            world_transform,
            world_bounds: element.world_bounds,
            ancestors,
            shape,
            tube,
        });
        report.upserted += 1;
    }

    if let Some(publication) = publication {
        // 这一根「看得见的清单」：目标变了但清单逐位没变时，几何一条都不必重写。
        let (manifest_hash, geometry_count) = manifest_hash(&commit.upserts)?;
        if publication.projection_complete
            && publication
                .published
                .as_ref()
                .is_some_and(|published| published.manifest_hash == manifest_hash)
        {
            // 不可变目标变了，但这一根看得见的清单没变：只留回执，几何一条都不动。
            commit.tube_branch_resets.clear();
            commit.removals.clear();
            commit.upserts.clear();
            report.upserted = 0;
            report.removed = 0;
        }
        commit.receipt = Some((
            publication.root,
            RootReceipt {
                source_sesno,
                model_target: publication.model_target.clone(),
                manifest_hash,
                geometry_count,
                published_via: "regen".into(),
            },
        ));
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok((commit, report))
}

/// 一条投影记录在根回执清单里的那一格：`geometry_key → entry`。
///
/// 这是 `RootReceipt.manifest_hash` 的**配方**：重生成（[`project_geometry_delta`]）与刚体前移
/// （`rigid_motion::republish_records`，ADR-066）都必须从这里取——两条路对同一批记录才会算出
/// 同一个章，对拍门比的正是它。条目只放「看得见」的东西（几何身份、mesh、世界变换、noun、
/// 图元摆放 / 烘焙标记、管段序号与有效位），不放来源会话与祖先链。
pub(crate) fn manifest_entry(record: &ProjectedGeometry) -> anyhow::Result<(String, Value)> {
    let geometry_key = serde_json::to_string(&record.geometry_id)?;
    let mut entry = json!({
        "geometry_id": serde_json::to_value(&record.geometry_id)?,
        "mesh": record.mesh_id,
        "world": transform_value(record.world_transform),
        "noun": record.noun,
        "solid": true,
    });
    match (&record.tube, &record.shape) {
        (Some(tube), _) => {
            entry["route_ordinal"] = json!(tube.route_ordinal);
            entry["invalid"] = json!(tube.invalid);
        }
        (
            None,
            ProjectedShape::Canonical {
                key,
                local_transform,
                ..
            },
        ) => {
            entry["local"] = transform_value(*local_transform);
            entry["primitive_key"] = serde_json::to_value(key)?;
        }
        (None, ProjectedShape::Baked) => entry["booled"] = json!(true),
    }
    Ok((geometry_key, entry))
}

/// 一根记录的清单摘要与条目数。同一 `geometry_id` 只算一条（后者覆盖前者），与它取代的
/// 内联实现逐位相同。
pub(crate) fn manifest_hash(records: &[ProjectedGeometry]) -> anyhow::Result<(String, usize)> {
    let mut entries = BTreeMap::<String, Value>::new();
    for record in records {
        let (key, entry) = manifest_entry(record)?;
        entries.insert(key, entry);
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&entries)?));
    Ok((digest, entries.len()))
}

fn local_generated_mesh(element: &GeneratedElement) -> PlantMesh {
    let inverse_world = element.world.inverse();
    let mut mesh = element.parts.as_deref().map_or_else(
        || {
            let local = element.solid.transform(&dmat4_to_affine4x3(inverse_world));
            crate::fast_model::manifold_csg::manifold_to_plant_mesh(&local)
        },
        |parts| {
            crate::fast_model::manifold_csg::manifolds_to_transformed_plant_mesh(
                parts,
                inverse_world,
            )
        },
    );
    // Catalogue parts are evaluated in world coordinates and localized here.
    // A pure element translation can leave ~1e-14 cancellation residue where
    // the catalogue-local coordinate is exactly zero. PlantMesh stores f32, so
    // canonicalizing only this numerical noise keeps its content id invariant
    // without changing any CAD-scale feature.
    for vertex in &mut mesh.vertices {
        if vertex.x.abs() < 1e-10 {
            vertex.x = 0.0;
        }
        if vertex.y.abs() < 1e-10 {
            vertex.y = 0.0;
        }
        if vertex.z.abs() < 1e-10 {
            vertex.z = 0.0;
        }
    }
    mesh
}

pub(crate) fn model_file_identity_digest(path: &Path, session: u32) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("read model target identity {}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut digest = Sha256::new();
    digest.update(canonical.to_string_lossy().as_bytes());
    digest.update(metadata.len().to_le_bytes());
    digest.update(modified.to_le_bytes());
    digest.update(session.to_le_bytes());
    Ok(hex::encode(digest.finalize()))
}

fn persisted_world_transform(
    geometry_id: &GeometryId,
    primitive: Option<&e3d_model::primitive_instance::PrimitiveInstance>,
    world: DMat4,
) -> DMat4 {
    if matches!(geometry_id, GeometryId::ImpliedTube { .. }) {
        primitive
            .map(|instance| world * instance.local_transform)
            .unwrap_or(world)
    } else {
        world
    }
}

/// Remove every projected e3d-model geometry owned by one generation root.
///
/// Derived geometry (for example implied tubes) has a content-addressed record
/// id, so removing only the PE subtree's ordinary `<refno>` records would leave
/// it behind.  Go through the same geometry-id path as regeneration so every
/// representation of the root is dropped together.
pub async fn delete_persisted_geometry_root(root: RefNo) -> anyhow::Result<usize> {
    let mut by_dbnum = BTreeMap::<u32, Vec<GeometryId>>::new();
    for record in ModelMemoryStore::global().records_of_root(ProjectionScope::Current, root) {
        by_dbnum
            .entry(record.dbnum)
            .or_default()
            .push(record.geometry_id);
    }

    let mut removed = 0;
    for (dbnum, geometry_ids) in by_dbnum {
        removed += apply_geometry_delta(
            dbnum,
            0,
            Vec::new(),
            geometry_ids,
            &BTreeMap::new(),
            Path::new("."),
        )
        .await?
        .removed;
    }
    Ok(removed)
}

/// Full generation must never regress the projection. With the source pinned to
/// the file's latest session this only trips when the file itself was rolled
/// back or replaced, which is a file anomaly, not a window to skip.
async fn ensure_not_older_than_persisted(dbnum: u32, source_sesno: u32) -> anyhow::Result<()> {
    let projected = ModelMemoryStore::global().published_sesno(ProjectionScope::Current, dbnum);
    anyhow::ensure!(
        session_not_stale(projected, source_sesno),
        "stale e3d-model session {source_sesno} cannot overwrite projected session {}",
        projected.unwrap_or_default()
    );
    Ok(())
}

/// An explicit window target that is older than what is already projected has
/// been superseded (ADR-054 constraint 5): the caller settles it as covered.
async fn persisted_session_newer_than(
    dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<Option<u32>> {
    Ok(ModelMemoryStore::global()
        .published_sesno(ProjectionScope::Current, dbnum)
        .filter(|projected| *projected > target_sesno))
}

fn session_not_stale(persisted: Option<u32>, source_sesno: u32) -> bool {
    persisted.is_none_or(|sesno| source_sesno >= sesno)
}

fn increment_failure_count(report: &IncrementReport) -> usize {
    report.regen_failed.len()
        + report.derived_failed.len()
        + report.unresolved.len()
        + report.derived_stale_unreadable.len()
}

fn note_mesh_write(report: &mut E3dPersistReport, outcome: MeshWrite) {
    match outcome {
        MeshWrite::Written => report.mesh_written += 1,
        MeshWrite::Reused => report.mesh_reused += 1,
    }
}

pub(crate) fn scan_index(path: &Path, sesno: Option<u32>) -> anyhow::Result<SourceIndex> {
    let mut engine = match sesno {
        Some(value) => ReadOnlyEngine::open_at(path, value),
        None => ReadOnlyEngine::open(path),
    }?;
    let mut indexed = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for element in engine.scan_elements(ScanTier::Header)? {
        let element = element?;
        indexed.insert((element.refno.word0, element.refno.word1));
        owners.insert((element.refno.word0, element.refno.word1), element.owner);
    }
    let roots = owners
        .iter()
        .filter_map(|(&(word0, word1), &owner)| {
            let refno = RefNo::new(word0, word1);
            (owner == refno || !owner.is_valid() || !indexed.contains(&(owner.word0, owner.word1)))
                .then_some(refno)
        })
        .collect();
    Ok(SourceIndex {
        dbnum: engine.descriptor().db_mark,
        roots,
        owners: Arc::new(owners),
    })
}

pub fn parse_refno(value: &str) -> anyhow::Result<RefNo> {
    let normalized = value.trim().trim_start_matches('=').replace('_', "/");
    let (word0, word1) = normalized
        .split_once('/')
        .with_context(|| format!("invalid refno {value}"))?;
    Ok(RefNo::new(word0.parse()?, word1.parse()?))
}

pub(crate) fn geometry_source_refno(id: &GeometryId) -> anyhow::Result<RefNo> {
    match id {
        GeometryId::Element { refno } => parse_refno(refno),
        GeometryId::ImpliedTube {
            container_refno, ..
        } => parse_refno(container_refno),
    }
}

fn ancestor_chain(refno: RefNo, owners: &BTreeMap<(u32, u32), RefNo>) -> anyhow::Result<Vec<i64>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = refno;
    loop {
        if !seen.insert((current.word0, current.word1)) {
            bail!("OWNER cycle at {current}");
        }
        chain.push((((current.word0 as u64) << 32) | current.word1 as u64) as i64);
        let Some(&owner) = owners.get(&(current.word0, current.word1)) else {
            break;
        };
        if owner == current
            || !owner.is_valid()
            || !owners.contains_key(&(owner.word0, owner.word1))
        {
            break;
        }
        current = owner;
    }
    Ok(chain)
}

pub(crate) fn transform_value(matrix: DMat4) -> Value {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    json!({
        "translation": [translation.x as f32, translation.y as f32, translation.z as f32],
        "rotation": [rotation.x as f32, rotation.y as f32, rotation.z as f32, rotation.w as f32],
        "scale": [scale.x as f32, scale.y as f32, scale.z as f32]
    })
}

fn point3(point: &parry3d::math::Point<f32>) -> [f32; 3] {
    [point.x, point.y, point.z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::packed_refno;
    use std::collections::HashMap;

    #[test]
    fn persist_report_merge_keeps_failed_root_identity() {
        let root = RefNo::new(23399, 13);
        let mut total = E3dPersistReport::default();
        total.merge_counts(E3dPersistReport {
            failed: 3,
            failed_roots: vec![(root, "three geometry incidents".into())],
            ..Default::default()
        });
        assert_eq!(total.failed, 3);
        assert_eq!(
            total.failed_roots,
            vec![(root, "three geometry incidents".into())]
        );
    }

    #[test]
    fn only_a_root_with_nothing_to_lose_publishes_a_partial_manifest() {
        // Cold root: the discard would "preserve" an empty snapshot.
        assert_eq!(incomplete_root_policy(false, false), IncompleteRoot::Publish);
        // A receipt, geometry rows, or both — all of them can lose something.
        assert_eq!(incomplete_root_policy(true, false), IncompleteRoot::Discard);
        assert_eq!(incomplete_root_policy(false, true), IncompleteRoot::Discard);
        assert_eq!(incomplete_root_policy(true, true), IncompleteRoot::Discard);
    }

    /// 生产端烘的是「算出来的那些」，丢不丢由消费端定——它看不见发布状态
    /// （`memory_generation_reads_only_the_e3d_source_projection` 钉的正是这条）。
    /// 让生产端自己判一次，按需生成那条路就只拿得到一个空候选：SLYK
    /// `21491/17101` 一个缺目录件把整根 17 个构件全按下去，就是这么来的。
    #[test]
    fn the_producer_bakes_every_survivor_and_leaves_the_verdict_to_the_consumer() {
        let source = service_source();
        let worker = source
            .split_once("fn spawn_generation_worker(")
            .expect("generation worker")
            .1
            .split_once("\n}\n")
            .expect("generation worker end")
            .0;
        assert!(
            !worker.contains("IncompleteRoot"),
            "发布状态在消费端，生产端判不了这根丢不丢"
        );
        assert!(
            production_half(&source).contains("if incomplete == IncompleteRoot::Discard {"),
            "丢弃不完整候选只能发生在拿得到发布回执的那一侧"
        );
    }

    #[tokio::test]
    #[ignore = "read-only AMS catalogue source diagnosis"]
    async fn ams_catalogue_7000_reference_inventory() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let option = aios_core::get_db_option();
        let membership = mdb_membership::resolve(option, &option.project_name, &option.mdb_name)?;
        for database in membership
            .databases()
            .iter()
            .filter(|db| db.dbnum == 7000 || db.dbnum == 7015)
        {
            println!("MDB_DECLARATION {database:?}");
        }
        println!("MDB_PROBLEMS {:?}", membership.problems());
        for pin in current_mdb_sources()?
            .0
            .iter()
            .filter(|pin| pin.dbnum == 7000 || pin.dbnum == 7015)
        {
            println!(
                "MDB_PIN dbnum={} type={} file={}",
                pin.dbnum,
                pin.db_type,
                pin.file.display()
            );
        }
        for file in [
            "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7000_0001",
            "D:/AVEVA/Projects/E3D3.1/AvevaCatalogue/acp000/acp7000_0001",
        ] {
            let mut view = DbView::open(file)?;
            let session = view
                .walk_sessions()?
                .first()
                .context("catalogue session")?
                .sesno;
            let set = Arc::new(DbSet::with_attlib_file(
                service.schema.template_dir().join("attlib.dat"),
            )?);
            set.add_db(DbFilePin {
                file: file.into(),
                template: template_file_for(service.schema.template_dir(), "CATA")?,
                db_type: Some("CATA".into()),
                sesno: Some(session),
            })?;
            for id in [11893, 19297, 32948, 11899, 19303, 47230] {
                let element = set.element(RefNo::new(15192, id));
                println!(
                    "CATA_REF file={file} session={session} ref=15192/{id} result={:?}",
                    element.element_type()
                );
            }
        }
        let target = service.build_set(7015, None)?;
        for id in [11893, 19297, 32948, 11899, 19303, 47230] {
            anyhow::ensure!(
                target.element(RefNo::new(15192, id)).element_type().is_ok(),
                "ACP DESI 7015 must resolve catalogue ref 15192/{id} through ACP CATA 7000"
            );
        }
        let pin = service.pin(7015)?;
        let source_sesno = service.source_session(pin)?;
        let index = scan_index(&pin.file, Some(source_sesno))?;
        let mut cache = CatalogueMeshCache::default();
        for root in [
            RefNo::new(23399, 13),
            RefNo::new(23399, 22),
            RefNo::new(23399, 24),
            RefNo::new(23399, 33),
        ] {
            let snapshot = generate_snapshot_from_set(&target, &index, root, &mut cache)?;
            anyhow::ensure!(
                snapshot.report.failed.is_empty(),
                "ACP root {root} still has geometry incidents: {:?}",
                snapshot.report.failed
            );
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "read-only AMS initialization root overlap census"]
    async fn ams_initialization_root_overlap_census() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        for dbnum in [7022, 7333] {
            let pin = service.pin(dbnum)?;
            let mut view = DbView::open(&pin.file)?;
            let session = view
                .walk_sessions()?
                .first()
                .context("source session")?
                .sesno;
            let index = scan_index(&pin.file, Some(session))?;
            let roots =
                crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, session)
                    .await?;
            let root_set = roots
                .iter()
                .map(|root| parse_refno(&root.root.to_string()))
                .collect::<anyhow::Result<std::collections::HashSet<_>>>()?;
            let mut nouns = BTreeMap::<String, usize>::new();
            let mut nested = 0;
            for root in roots {
                *nouns.entry(root.noun).or_default() += 1;
                let mut current = parse_refno(&root.root.to_string())?;
                let mut seen = std::collections::HashSet::new();
                while seen.insert(current) {
                    let Some(owner) = index.owners.get(&(current.word0, current.word1)) else {
                        break;
                    };
                    if root_set.contains(owner) {
                        nested += 1;
                        break;
                    }
                    current = *owner;
                }
            }
            println!(
                "ROOT_OVERLAP dbnum={dbnum} session={session} roots={} nested={nested} nouns={nouns:?}",
                root_set.len()
            );
            if dbnum == 7022 {
                let set = service.build_set(dbnum, Some(session))?;
                for id in [6, 8, 10, 14] {
                    let element = set.element(RefNo::new(23406, id));
                    println!(
                        "ROOT_INPUT ref=23406/{id} noun={:?} owner={:?} members={:?}",
                        element.element_type(),
                        element.owner()?.map(|e| e.refno()),
                        element.member_refnos()
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS 7022 missing-RPATH Core3D behavior"]
    async fn ams_cable_tray_without_rpath_uses_core_origin() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let set = service.build_set(7022, Some(254))?;
        let element = set.element(RefNo::new(23406, 8));
        let placement = e3d_model::cable_tray::placement(&element)?;
        assert_eq!(placement.position, glam::DVec3::ZERO);
        assert_eq!(placement.rotation, glam::DMat3::IDENTITY);
        println!(
            "CORE_MISSING_RPATH ref=23406/8 position={:?} rotation={:?}",
            placement.position, placement.rotation
        );
        let index = scan_index(&service.pin(7022)?.file, Some(254))?;
        let snapshot = generate_snapshot_from_set(
            &set,
            &index,
            RefNo::new(23406, 6),
            &mut CatalogueMeshCache::default(),
        )?;
        anyhow::ensure!(
            snapshot.report.failed.is_empty(),
            "{}",
            summarize_geometry_incidents(&snapshot.report.failed)
        );
        anyhow::ensure!(
            !snapshot.elements.is_empty(),
            "repaired root must produce geometry"
        );
        println!(
            "CORE_MISSING_RPATH_MODEL root=23406/6 elements={} failed={}",
            snapshot.elements.len(),
            snapshot.report.failed.len()
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "read-only AMS unfinished model attribute diagnosis"]
    async fn ams_unfinished_model_attribute_inventory() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        for (dbnum, refno, attributes) in [
            (
                7048,
                RefNo::new(23432, 32104),
                vec!["BANG", "POS", "ORI", "TYPE"],
            ),
            (
                7330,
                RefNo::new(23714, 1011),
                vec!["POSS", "POSE", "POS", "ORI"],
            ),
        ] {
            let set = service.build_set(dbnum, None)?;
            let element = set.element(refno);
            for attribute in attributes {
                println!(
                    "MODEL_INPUT dbnum={dbnum} ref={refno} attribute={attribute} value={:?}",
                    element.attribute(attribute)
                );
                if attribute == "BANG" {
                    println!(
                        "MODEL_DESCRIPTOR {:?}",
                        element.extraction()?.find_attribute(attribute)
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS finite zero-length section completion"]
    async fn ams_zero_length_section_root_completes() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let pin = service.pin(7330)?;
        let session = service.source_session(pin)?;
        let set = service.build_set(7330, Some(session))?;
        let index = scan_index(&pin.file, Some(session))?;
        let snapshot = generate_snapshot_from_set(
            &set,
            &index,
            RefNo::new(23714, 997),
            &mut CatalogueMeshCache::default(),
        )?;
        anyhow::ensure!(
            snapshot.report.failed.is_empty(),
            "{}",
            summarize_geometry_incidents(&snapshot.report.failed)
        );
        anyhow::ensure!(
            !snapshot.elements.is_empty(),
            "other valid members must survive"
        );
        println!(
            "ZERO_LENGTH_SECTION root=23714/997 elements={} failed={}",
            snapshot.elements.len(),
            snapshot.report.failed.len()
        );
        Ok(())
    }

    fn baked_element(refno: RefNo, mesh_id: &str) -> BakedElement {
        BakedElement {
            geometry_id: GeometryId::Element {
                refno: refno.to_string(),
            },
            refno,
            noun: "EQUI".into(),
            is_invalid_tubi: false,
            world: DMat4::IDENTITY,
            primitive_instance: None,
            mesh_id: mesh_id.into(),
            mesh_write: MeshWrite::Reused,
            world_bounds: [[0.0; 3], [1.0; 3]],
            local_bounds: None,
        }
    }

    fn baked_tube(branch: RefNo, route_ordinal: u32, mesh_id: &str) -> BakedElement {
        BakedElement {
            geometry_id: GeometryId::ImpliedTube {
                container_refno: branch.to_string(),
                route_ordinal,
                from_refno: "24383/10".into(),
                to_refno: "24383/20".into(),
            },
            refno: branch,
            noun: "TUBI".into(),
            is_invalid_tubi: false,
            world: DMat4::IDENTITY,
            primitive_instance: None,
            mesh_id: mesh_id.into(),
            mesh_write: MeshWrite::Reused,
            world_bounds: [[0.0; 3], [1.0; 3]],
            local_bounds: None,
        }
    }

    #[test]
    fn an_invalid_implied_tube_survives_projection_for_the_ui() {
        let root = RefNo::new(24383, 100);
        let branch = RefNo::new(24383, 73948);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
        ]);
        let mut tube = baked_tube(branch, 0, "diagnostic_tube");
        tube.is_invalid_tubi = true;

        let (commit, _) = project_geometry_delta(7997, 42, vec![tube], Vec::new(), None, &owners)
            .expect("project invalid tube");

        assert!(commit.upserts[0].tube.as_ref().unwrap().invalid);
    }

    /// 本文件的正文，行尾归一。护栏测试按文本形状判定，`core.autocrlf` 决定的检出行尾
    /// 不该让它们时红时绿。
    fn service_source() -> String {
        include_str!("e3d_model_service.rs").replace("\r\n", "\n")
    }

    fn production_half(source: &str) -> &str {
        source.split_once("mod tests {").expect("test module").0
    }

    /// 本模块只生成、只写进程内投影。任何一处库句柄、SQL 片段或事务都是回退：
    /// 落库是另一层适配器的事，从这里长回来的第一行 SQL 就是那条边界破了。
    #[test]
    fn the_generation_service_never_reaches_for_a_database() {
        let source = service_source();
        let production = production_half(&source);
        for forbidden in [
            "SUL_DB",
            "surrealdb",
            "type::thing",
            "UPSERT",
            "INSERT RELATION",
            "BEGIN TRANSACTION",
            "publication_transaction",
            "gen_root",
            "inst_relate",
            "tubi_relate",
        ] {
            assert!(
                !production.contains(forbidden),
                "生成入口不许认识数据库，却出现了 {forbidden}"
            );
        }
    }

    #[test]
    fn baked_persistence_keeps_independent_parts_in_local_coordinates() {
        let world = DMat4::from_translation(glam::DVec3::new(100.0, 200.0, 300.0));
        let local_left = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true);
        let local_right = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true).transform(
            &dmat4_to_affine4x3(DMat4::from_translation(glam::DVec3::new(10.0, 0.0, 0.0))),
        );
        let parts = vec![
            local_left.transform(&dmat4_to_affine4x3(world)),
            local_right.transform(&dmat4_to_affine4x3(world)),
        ];
        let element = GeneratedElement {
            geometry_id: GeometryId::Element {
                refno: "1/2".into(),
            },
            refno: RefNo::new(1, 2),
            noun: "HELE".into(),
            is_invalid_tubi: false,
            name: None,
            solid: manifold_csg::Manifold::compose(&parts),
            parts: Some(parts),
            world,
            primitive_instance: None,
            negatives_applied: 0,
            negatives_skipped: vec![],
            notes: vec![],
        };

        let mesh = local_generated_mesh(&element);

        assert_eq!(
            mesh.indices.len() / 3,
            24,
            "must not boolean-union the parts"
        );
        let bounds = mesh.aabb.expect("local baked mesh must have bounds");
        assert_eq!([bounds.mins.x, bounds.mins.y, bounds.mins.z], [-10.0; 3]);
        assert_eq!(
            [bounds.maxs.x, bounds.maxs.y, bounds.maxs.z],
            [20.0, 10.0, 10.0]
        );
    }

    #[test]
    fn pure_translation_keeps_baked_catalogue_mesh_id() {
        fn element_at(translation: glam::DVec3) -> GeneratedElement {
            let world =
                DMat4::from_translation(translation) * DMat4::from_rotation_z(30_f64.to_radians());
            let local = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true);
            let parts = vec![local.transform(&dmat4_to_affine4x3(world))];
            GeneratedElement {
                geometry_id: GeometryId::Element {
                    refno: "1/2".into(),
                },
                refno: RefNo::new(1, 2),
                noun: "VALV".into(),
                is_invalid_tubi: false,
                name: None,
                solid: manifold_csg::Manifold::compose(&parts),
                parts: Some(parts),
                world,
                primitive_instance: None,
                negatives_applied: 0,
                negatives_skipped: vec![],
                notes: vec![],
            }
        }

        let before =
            local_generated_mesh(&element_at(glam::DVec3::new(-2685.11, -8478.79, 334.34)));
        let after = local_generated_mesh(&element_at(glam::DVec3::new(-2685.11, -8478.79, 659.34)));

        assert_eq!(baked_mesh_id(&before), baked_mesh_id(&after));
    }

    #[test]
    fn every_incomplete_increment_bucket_blocks_publication() {
        let incident = e3d_model::pipeline::Incident {
            refno: "1/2".into(),
            noun: "TEST".into(),
            detail: "fixture".into(),
        };
        let mut report = IncrementReport::default();
        report.regen_failed.push(incident.clone());
        report.derived_failed.push(incident.clone());
        report.unresolved.push(incident.clone());
        report.derived_stale_unreadable.push(incident);
        assert_eq!(increment_failure_count(&report), 4);
    }

    /// 一根重投影的完整形状：碰过的分支先清管身，上一轮留下、这一轮没再生成的几何被删，
    /// 新几何带着祖先链进存储，回执盖在根上。
    #[test]
    fn a_reprojected_root_resets_its_branch_tubes_and_drops_stale_geometry() {
        let store = ModelMemoryStore::default();
        let root = RefNo::new(24383, 100);
        let branch = RefNo::new(24383, 73948);
        // 祖先链走到「owner 不在索引里」为止，所以根自己也要在表里（指向自身）。
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
            ((24383, 5), root),
            ((24383, 6), root),
        ]);

        let (first, _) = project_geometry_delta(
            7997,
            42,
            vec![
                baked_element(RefNo::new(24383, 5), "mesh_five"),
                baked_element(RefNo::new(24383, 6), "mesh_six"),
                baked_tube(branch, 0, "mesh_tube_a"),
            ],
            Vec::new(),
            None,
            &owners,
        )
        .expect("first projection");
        store.commit(ProjectionScope::Current, vec![first]);
        assert_eq!(
            store
                .geometry_ids_of_root(ProjectionScope::Current, 7997, root)
                .len(),
            3
        );

        // 第二轮：5 还在、6 没了、分支上换了一段新管身。
        let stale = store
            .geometry_ids_of_root(ProjectionScope::Current, 7997, root)
            .into_iter()
            .filter(|geometry_id| {
                matches!(geometry_id, GeometryId::Element { refno } if refno.contains("/6"))
            })
            .collect::<Vec<_>>();
        let (second, report) = project_geometry_delta(
            7997,
            43,
            vec![
                baked_element(RefNo::new(24383, 5), "mesh_five"),
                baked_tube(branch, 1, "mesh_tube_b"),
            ],
            stale,
            None,
            &owners,
        )
        .expect("second projection");
        assert_eq!((report.upserted, report.removed), (2, 1));
        assert_eq!(
            second.tube_branch_resets,
            vec![branch],
            "碰过的分支要整段重置，而不是逐段增删"
        );
        store.commit(ProjectionScope::Current, vec![second]);

        let records = store.records_of_root(ProjectionScope::Current, root);
        let mut meshes = records
            .iter()
            .map(|record| record.mesh_id.as_str())
            .collect::<Vec<_>>();
        meshes.sort_unstable();
        assert_eq!(meshes, vec!["mesh_five", "mesh_tube_b"]);
        assert!(
            records
                .iter()
                .all(|record| record.ancestors.contains(&packed_refno(root))),
            "每条记录都要带着到根的祖先链，按根查才捞得回来"
        );
        assert_eq!(
            store.published_sesno(ProjectionScope::Current, 7997),
            Some(43)
        );
    }

    /// 目标变了但这一根看得见的清单逐位没变：只留回执，几何一条都不写。
    #[test]
    fn an_unchanged_manifest_commits_only_the_receipt() {
        let root = RefNo::new(1, 2);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let publication = RootPublication {
            root,
            model_target: None,
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: hex::encode(Sha256::digest(empty_manifest)),
                geometry_count: 0,
                published_via: "regen".into(),
            }),
            projection_complete: true,
        };
        let (commit, report) = project_geometry_delta(
            8000,
            42,
            Vec::new(),
            vec![GeometryId::Element {
                refno: "8000/9".into(),
            }],
            Some(&publication),
            &BTreeMap::new(),
        )
        .expect("prepare unchanged root");

        assert_eq!((report.upserted, report.removed), (0, 0));
        assert!(commit.removals.is_empty(), "{commit:?}");
        assert!(commit.upserts.is_empty(), "{commit:?}");
        let (receipt_root, receipt) = commit.receipt.expect("receipt still lands");
        assert_eq!(receipt_root, root);
        assert_eq!(receipt.geometry_count, 0);
    }

    /// 上一轮的投影不完整（比如 mesh 文件被删过）时，同一份清单必须照旧重写一遍。
    #[test]
    fn an_unchanged_manifest_still_repairs_an_incomplete_projection() {
        let root = RefNo::new(24383, 73948);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let publication = RootPublication {
            root,
            model_target: None,
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: hex::encode(Sha256::digest(empty_manifest)),
                geometry_count: 0,
                published_via: "regen".into(),
            }),
            projection_complete: false,
        };
        let (commit, report) = project_geometry_delta(
            8000,
            42,
            Vec::new(),
            vec![GeometryId::Element {
                refno: "8000/9".into(),
            }],
            Some(&publication),
            &BTreeMap::new(),
        )
        .expect("prepare repair projection");
        assert_eq!(report.removed, 1);
        assert_eq!(commit.removals, vec!["8000_9".to_string()]);
    }

    /// 不可变目标与源会话都没变，投影也完整：这一根连几何都不必求值，只重盖一次章。
    #[test]
    fn an_identical_immutable_target_reprojects_before_any_geometry_build() {
        let target = ModelTarget {
            project: "fixture".into(),
            design: ModelFileTarget {
                dbnum: 8000,
                db_type: "DESI".into(),
                file: "design.db".into(),
                session: 42,
                digest: "design".into(),
            },
            catalogue: Vec::new(),
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "generator".into(),
            tessellation_profile: "tessellation".into(),
        };
        let publication = RootPublication {
            root: RefNo::new(1, 2),
            model_target: Some(target.clone()),
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: Some(target),
                manifest_hash: "manifest".into(),
                geometry_count: 3,
                published_via: "regen".into(),
            }),
            projection_complete: true,
        };
        assert!(cached_root_target_matches(&publication, 42));
        assert!(
            !cached_root_target_matches(&publication, 43),
            "源会话变了就不算命中"
        );

        let commit = cached_root_commit(&publication, 42).expect("cached commit");
        assert!(commit.upserts.is_empty());
        assert!(commit.removals.is_empty());
        assert!(commit.tube_branch_resets.is_empty());
        let (_, receipt) = commit.receipt.expect("receipt");
        assert_eq!(receipt.manifest_hash, "manifest");
        assert_eq!(receipt.geometry_count, 3);
    }

    /// 上一轮没投过、或者投的是另一个目标，都不许走「不必重算」那条路。
    #[test]
    fn a_missing_or_different_target_never_hits_the_cache() {
        let mut publication = RootPublication {
            root: RefNo::new(1, 2),
            model_target: None,
            published: None,
            projection_complete: false,
        };
        assert!(!cached_root_target_matches(&publication, 42));

        publication.published = Some(RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: "manifest".into(),
            geometry_count: 0,
            published_via: "regen".into(),
        });
        assert!(
            !cached_root_target_matches(&publication, 42),
            "本轮算不出目标就不能声称命中"
        );
    }

    #[test]
    fn read_through_receipt_is_invalidated_by_each_target_identity_change() {
        let file = ModelFileTarget {
            dbnum: 8000, db_type: "DESI".into(), file: "design.db".into(),
            session: 459, digest: "generation-a".into(),
        };
        let target = ModelTarget {
            project: "fixture".into(), design: file.clone(),
            catalogue: vec![ModelFileTarget { dbnum: 7000, db_type: "CATA".into(), ..file }],
            template_attlib_digest: "schema-a".into(),
            generator_fingerprint: "generator-a".into(), tessellation_profile: "profile-a".into(),
        };
        let mut publication = RootPublication {
            root: RefNo::new(24384, 1), model_target: Some(target.clone()),
            published: Some(RootReceipt {
                source_sesno: 459, model_target: Some(target.clone()),
                manifest_hash: "manifest".into(), geometry_count: 9, published_via: "regen".into(),
            }), projection_complete: true,
        };
        assert!(cached_root_target_matches(&publication, 459));
        for change in 0..7 {
            let mut changed = target.clone();
            match change {
                0 => changed.design.session = 461, // SAVEWORK
                1 => changed.design.session = 457, // rollback
                2 => changed.design.digest = "generation-b".into(), // replacement
                3 => changed.catalogue[0].session += 1,
                4 => changed.catalogue[0].file = "other-project.db".into(),
                5 => changed.template_attlib_digest = "schema-b".into(),
                _ => changed.generator_fingerprint = "generator-b".into(),
            }
            publication.model_target = Some(changed);
            assert!(!cached_root_target_matches(&publication, 459), "change {change}");
        }
    }

    #[test]
    fn model_file_target_digest_pins_file_identity_and_session() {
        let path = std::env::temp_dir().join(format!("e3d-model-target-{}.db", std::process::id()));
        std::fs::write(&path, b"fixture-v1").expect("write fixture");
        let first = model_file_identity_digest(&path, 41).expect("first digest");
        assert_eq!(
            first,
            model_file_identity_digest(&path, 41).expect("repeat digest"),
            "the same immutable target must serialize deterministically"
        );
        assert_ne!(
            first,
            model_file_identity_digest(&path, 42).expect("new session digest"),
            "session is part of target identity"
        );
        std::fs::write(&path, b"fixture-v2-longer").expect("replace fixture");
        assert_ne!(
            first,
            model_file_identity_digest(&path, 41).expect("replacement digest"),
            "same path with replaced content identity is a new target"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn element_and_implied_tubes_have_disjoint_persistence_ids() {
        let source = RefNo::new(17496, 152095);
        let element = GeometryId::Element {
            refno: source.to_string(),
        };
        let tube_a = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            route_ordinal: 0,
            from_refno: "17496/1".into(),
            to_refno: "17496/2".into(),
        };
        let tube_b = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            route_ordinal: 1,
            from_refno: "17496/2".into(),
            to_refno: "17496/3".into(),
        };

        let element_id = geometry_record_id(&element, source);
        let tube_a_id = geometry_record_id(&tube_a, source);
        let tube_b_id = geometry_record_id(&tube_b, source);
        assert_eq!(element_id, "17496_152095");
        assert_ne!(element_id, tube_a_id);
        assert_ne!(tube_a_id, tube_b_id);
    }

    #[test]
    fn implied_tube_flattens_canonical_local_transform_into_the_stored_world_transform() {
        let geometry_id = GeometryId::ImpliedTube {
            container_refno: "24383/73948".into(),
            route_ordinal: 0,
            from_refno: "24383/10".into(),
            to_refno: "24383/20".into(),
        };
        let world = DMat4::from_translation(glam::DVec3::new(10.0, 20.0, 30.0));
        let local = DMat4::from_scale(glam::DVec3::new(100.0, 100.0, 3000.0));
        let primitive = e3d_model::primitive_instance::PrimitiveInstance {
            key: e3d_model::primitive_instance::PrimitiveMeshKey::CylinderV3 { segments: 32 },
            local_transform: local,
        };
        assert_eq!(
            persisted_world_transform(&geometry_id, Some(&primitive), world),
            world * local
        );

        let element = GeometryId::Element {
            refno: "24383/73948".into(),
        };
        assert_eq!(
            persisted_world_transform(&element, Some(&primitive), world),
            world,
            "ordinary elements keep the separate local-transform branch"
        );
    }

    #[test]
    fn older_session_cannot_overwrite_equal_or_newer_geometry() {
        assert!(session_not_stale(None, 41));
        assert!(session_not_stale(Some(41), 41));
        assert!(session_not_stale(Some(41), 42));
        assert!(!session_not_stale(Some(42), 41));
    }

    /// ADR-054 constraint 3: a root's database comes from the MDB's DESI files,
    /// never from `pe` (a never-parsed project has no `pe` rows), and the current
    /// projection must not read its timepoint from `dbnum_watermark`.
    #[test]
    fn the_current_projection_reads_neither_pe_nor_the_watermark_table() {
        let source = service_source();
        let source = source.as_str();
        let body = source
            .split_once("pub async fn from_current(")
            .expect("from_current")
            .1
            .split_once("pub async fn generate_roots(")
            .expect("from_current and dbnum_for_roots end before generate_roots")
            .0;
        assert!(body.contains("current_mdb_source_inventory()"));
        assert!(body.contains("model_source::dbnum_of_root("));
        assert!(!body.contains("dbnum_watermark"), "{body}");
        assert!(!body.contains("pins_from_watermark"), "{body}");
    }

    /// ADR-054 constraint 5: an explicit window target is opened as such; the
    /// current pin is never required to equal it, and a newer projected session
    /// settles the window as covered instead of failing it.
    #[test]
    fn apply_window_opens_the_explicit_target_and_yields_to_newer_publications() {
        let source = service_source();
        let source = source.as_str();
        let body = source
            .split_once("pub async fn apply_window(")
            .expect("apply_window")
            .1
            .split_once("async fn generate_refs(")
            .expect("apply_window ends before generate_refs")
            .0;
        assert!(!body.contains("pin.sesno == Some(target_sesno)"), "{body}");
        assert!(body.contains("persisted_session_newer_than(dbnum, target_sesno)"));
        assert!(body.contains("build_set(dbnum, Some(target_sesno))"));
        assert!(body.contains("scan_index(&pin.file, Some(target_sesno))"));
    }

    fn memory_producer_body(source: &str) -> &str {
        source
            .split_once("async fn produce_slices(")
            .expect("memory producer")
            .1
            .split_once("pub async fn persist_memory_batch(")
            .expect("memory producer end")
            .0
    }

    fn memory_consumer_body(source: &str) -> &str {
        source
            .split_once("pub async fn persist_memory_batch(")
            .expect("memory consumer")
            .1
            .split_once("pub async fn generate_and_persist_roots(")
            .expect("memory consumer end")
            .0
    }

    #[test]
    fn memory_generation_reads_only_the_e3d_source_projection() {
        let source = service_source();
        let source = source.as_str();
        let body = memory_producer_body(source);
        for forbidden in [
            "ModelMemoryStore",
            "ensure_not_older_than_persisted",
            "root_projection_complete",
        ] {
            assert!(
                !body.contains(forbidden),
                "memory producer must get model input only from e3d-io/e3d-model, found {forbidden}"
            );
        }
        assert!(body.contains("spawn_generation_worker("));
    }

    /// 生产端读源文件、算几何，碰不到已投影的那一份，也碰不到根回执——
    /// `db_generation_lock` 护的是投影侧那条单调链，拿在生产端只会把纯读串起来。
    /// 单根硬错是一条死信，不是整批的死因（ADR-041 §6）：必现失败的坏构件不许
    /// 把整页永久卡死。
    #[test]
    fn the_memory_producer_neither_locks_publication_nor_drops_a_page_for_one_root() {
        let source = service_source();
        let source = source.as_str();
        let producer = memory_producer_body(source);
        assert!(
            !producer.contains("db_generation_lock"),
            "the publication lock belongs to the consumer; generation is pure source reading"
        );
        assert!(
            producer.contains("Err(error) if !matches!(failure_policy,")
                && producer.contains("slice.failures.push("),
            "a root that cannot be generated must become a dead letter, not abort the batch"
        );
        let consumer = memory_consumer_body(source);
        for required in [
            "db_generation_lock",
            "ensure_not_older_than_persisted",
            "batch.failures",
        ] {
            assert!(
                consumer.contains(required),
                "the consumer projects under the dbnum lock, re-checks the pinned session and \
                 counts producer dead letters; missing {required}"
            );
        }
    }

    /// ADR-041 §8：生产与消费之间那条 channel 必须**有界**，深度 1。无界 channel
    /// 一样不阻塞，代价是内存无界——那正是 ADR 的否决方案。整页一次性产进内存
    /// （2720 根的 7997 就是 GB 级）与它是同一个形态。
    #[test]
    fn the_generation_pipeline_hands_over_bounded_slices() {
        let source = service_source();
        let source = source.as_str();
        let pipeline = source
            .split_once("pub async fn generate_and_persist_roots(")
            .expect("generation pipeline")
            .1
            .split_once("pub async fn generate_dbnum(")
            .expect("generation pipeline end")
            .0;
        assert!(
            pipeline.contains("mpsc::channel::<GeneratedMemoryBatch>(1)"),
            "producer and consumer must meet over a depth-1 bounded channel"
        );
        assert!(
            pipeline.contains("futures::future::join(produce, consume)"),
            "the two halves must overlap; running them in sequence is the (P + C) × N shape"
        );
        let producer = memory_producer_body(source);
        assert!(
            producer.contains("crate::options::model_memory_slice_elements()")
                && producer.contains("crate::options::model_regen_execution_group()"),
            "a slice is sealed on the element budget as well as the cohort width: root count \
             alone is not a memory budget"
        );
        assert!(
            !production_half(source).contains("fn generate_dbnum_in_memory("),
            "producing a whole dbnum into one batch is the unbounded shape ADR-041 §8 rejected"
        );
    }

    /// ADR-052 第 4 条：常驻 worker 从共享队列动态领取，宽度 `min(额度, 件数)`。
    /// 静态均分正是被取代的形态（几何负载长尾，一块里摆两个重 BRAN 就把其余 worker
    /// 空在那儿）；「每件一个 task 再去挤信号量」同样被禁；每个 worker 一份 catalogue
    /// 缓存，共享一把 Mutex 并在锁内求值就是 ADR-052 开篇那个 convoy。
    #[test]
    fn generation_workers_claim_dynamically_and_never_share_one_catalogue_cache() {
        let source = service_source();
        let source = source.as_str();
        let worker = source
            .split_once("fn spawn_generation_worker(")
            .expect("generation worker")
            .1
            .split_once("\n}\n")
            .expect("generation worker end")
            .0;
        assert!(
            worker.contains("concurrency::run_gated_cpu(")
                && worker.contains("while let Some((slot, root)) = queue.claim()")
                && worker.contains("queue.store(slot, outcome)"),
            "worker 是「一张许可 + 一个领取循环」，写回按领取序号落槽"
        );
        assert!(
            worker.contains("checkout_cache(&caches)") && worker.contains("return_cache(&caches"),
            "每个 worker 一份缓存，从跨波存活的池里借还"
        );
        let producer = memory_producer_body(source);
        assert!(
            producer.contains("concurrency::resident_workers(queue.claimable())"),
            "常驻 worker 数只能是 min(额度, 还能领的件数)"
        );
        for forbidden in ["chunk_size(", "tokio::spawn(", "roots.chunks("] {
            assert!(
                !producer.contains(forbidden),
                "静态均分 / 每件一个 task / 按批设栏都是被取代的形态，找到 {forbidden}"
            );
        }
    }

    /// 窗口是「无栏并行」与「内存有界」之间那根绳子：worker 最多跑在装配端前面
    /// 一个窗口，再多就得先把许可还回去（攥着许可等装配端正是 ADR-052 开篇那个
    /// 容量倒置）。
    #[test]
    fn the_generation_window_bounds_how_far_workers_may_run_ahead() {
        let roots = (0..5).map(|index| RefNo::new(7, index)).collect::<Vec<_>>();
        let queue = RootQueue::new(roots.clone(), 2);

        assert_eq!(queue.claimable(), 2);
        assert_eq!(queue.claim(), Some((0, roots[0])));
        assert_eq!(queue.claim(), Some((1, roots[1])));
        assert_eq!(queue.claim(), None, "窗口满了就不许再领");
        assert_eq!(queue.claimable(), 0);

        // 装配端还没拿到 0 号，窗口就滑不动——先算完的 1 号不能把它顶过去。
        queue.store(1, Ok(snapshot_with_elements(1)));
        assert!(queue.take_ready().is_none(), "队头没好，后面的不许插队");
        assert_eq!(queue.claim(), None);

        queue.store(0, Ok(snapshot_with_elements(1)));
        assert_eq!(queue.take_ready().map(|(root, _)| root), Some(roots[0]));
        assert_eq!(queue.claimable(), 1, "取走一个，窗口向前滑一格");
        assert_eq!(queue.claim(), Some((2, roots[2])));
    }

    /// 装配端只按源顺序取：切片边界与 `Required` 裁决因此与「谁先算完」无关，
    /// `geometry_workers = 1` 才能逐字节复现改前那条串行路径。
    #[test]
    fn finished_roots_are_assembled_in_source_order_not_completion_order() {
        let roots = (0..4).map(|index| RefNo::new(9, index)).collect::<Vec<_>>();
        let queue = RootQueue::new(roots.clone(), 4);
        for _ in 0..4 {
            queue.claim().expect("窗口容得下全部四根");
        }
        assert_eq!(queue.claim(), None, "领完就没有了");

        for slot in [3_usize, 1, 2] {
            queue.store(slot, Ok(snapshot_with_elements(slot)));
        }
        assert!(queue.take_ready().is_none());

        queue.store(0, Ok(snapshot_with_elements(0)));
        let assembled = std::iter::from_fn(|| queue.take_ready())
            .map(|(root, _)| root)
            .collect::<Vec<_>>();
        assert_eq!(assembled, roots, "完成序是 3/1/2/0，交出去的必须是 0/1/2/3");
        assert!(queue.take_ready().is_none());
        assert_eq!(queue.claimable(), 0);
    }

    /// 封片是「够了就交」，不是「攒满整页」：两条预算任一到线即封，只有死信的
    /// 尾片也要交出去（计数在消费端），空片不交。单根不拆——根是投影的原子单位。
    #[test]
    fn a_slice_seals_on_either_budget_and_never_splits_a_root() {
        let mut slice = MemorySlice::default();
        assert!(!slice.is_full(2, 100), "空片不算满");
        slice.failures.push((RefNo::new(1, 1), "boom".into()));
        assert!(
            !slice.is_full(2, 100),
            "死信不占元素预算，也不该单独把片顶满"
        );

        let mut by_roots = MemorySlice::default();
        by_roots.push(RefNo::new(1, 2), snapshot_with_elements(1));
        assert!(!by_roots.is_full(2, 100));
        by_roots.push(RefNo::new(1, 3), snapshot_with_elements(1));
        assert!(by_roots.is_full(2, 100), "到达 cohort 宽度即封片");
        assert_eq!(by_roots.elements, 2);

        let mut by_elements = MemorySlice::default();
        by_elements.push(RefNo::new(1, 4), snapshot_with_elements(150));
        assert!(
            by_elements.is_full(16, 100),
            "一根就撑破预算时立刻封片，而不是等够 16 根"
        );
        assert_eq!(
            by_elements.snapshots.len(),
            1,
            "根是投影的原子单位，超预算也不拆"
        );
    }

    fn snapshot_with_elements(count: usize) -> BakedSnapshot {
        BakedSnapshot {
            elements: (0..count)
                .map(|index| baked_element(RefNo::new(9, index as u32), &format!("mesh_{index}")))
                .collect(),
            report: Report::default(),
            visited: Vec::new(),
        }
    }

    /// 一页 100 根就是 100 份同一张 owner 表同时在内存里，而拼记录读的一直是批自己
    /// 带的那份索引。快照只借用扫描期的那一份，不再各自复制。
    #[test]
    fn every_snapshot_in_a_batch_shares_one_owner_index() {
        let source = service_source();
        let source = source.as_str();
        for declaration in [
            "pub(crate) owners: Arc<BTreeMap<(u32, u32), RefNo>>,",
            "pub owners: Arc<BTreeMap<(u32, u32), RefNo>>,",
        ] {
            assert!(
                source.contains(declaration),
                "the owner index is shared, not owned per snapshot; missing {declaration}"
            );
        }
        let body = source
            .split_once("fn generate_snapshot_from_set(")
            .expect("snapshot builder")
            .1
            .split_once("impl E3dPersistReport {")
            .expect("snapshot builder end")
            .0;
        assert!(
            body.contains("Arc::clone(&index.owners)") && !body.contains("index.owners.clone()"),
            "每个快照复制一份整库 owner 表，一页就是百 MB 级的纯重复"
        );
    }

    /// 单根快照路：根不在那一会话的元素索引里，要在求几何**之前**以类型化错误退出。
    /// 判在后面（或只靠消息文本）就是 2026-09-13 live 用例 3 那种「根不存在却报几何失败、
    /// 消息里还没原因」。
    #[test]
    fn snapshot_source_rejects_a_root_missing_from_the_session_index_before_generating() {
        let source = service_source();
        let body = source
            .split_once("pub async fn generate_snapshot_source(")
            .expect("snapshot source")
            .1
            .split_once("pub(crate) fn source_file(")
            .expect("snapshot source end")
            .0;
        let index_gate = body
            .find("input.index.owners.contains_key(&(root.word0, root.word1))")
            .expect("the session index gate");
        let typed_error = body
            .find("SnapshotSourceError::RootNotInSession")
            .expect("the typed not-in-session error");
        let generation = body
            .find("generate_snapshot_from_set(")
            .expect("snapshot generation");
        assert!(
            index_gate < generation && typed_error < generation,
            "the index gate and its typed error must come before geometry evaluation"
        );
    }

    fn generate_refs_body(source: &str) -> &str {
        source
            .split_once("async fn generate_refs(")
            .expect("generate_refs")
            .1
            .split_once("fn pin(&self")
            .expect("generate_refs end")
            .0
    }

    /// 存储状态一次读齐再进根循环。逐根现读会把「一次投影一批根」摊成 N 次往返，
    /// 也会让先算的那几根看不到后面才补上的依赖会话。
    #[test]
    fn projection_reads_the_store_once_before_the_root_loop() {
        let source = service_source();
        let source = source.as_str();
        let body = generate_refs_body(source);
        let (prefetch, root_loop) = body
            .split_once("for (root_index, &root) in roots.iter().enumerate()")
            .expect("root preparation loop");
        for read in [
            "store.receipt(",
            "store.root_projection_complete(",
            "store.geometry_ids_of_root(",
            "hydrate_published_dependencies(",
        ] {
            assert!(prefetch.contains(read), "missing batch prefetch for {read}");
            assert!(
                !root_loop.contains(read),
                "root loop must consume prefetched state instead of calling {read}"
            );
        }
    }

    /// 命中缓存要在求几何之前判掉——判在后面等于白算一遍。
    #[test]
    fn immutable_target_cache_gate_precedes_snapshot_generation() {
        let source = service_source();
        let body = generate_refs_body(&source);
        let cache_gate = body
            .find("store.root_projection_complete(")
            .expect("cache gate");
        let generation = body
            .find("generate_snapshot_from_set(")
            .expect("snapshot generation");
        assert!(
            cache_gate < generation,
            "the immutable-target hit must return before geometry evaluation"
        );
    }

    #[test]
    fn catalogue_resolver_prefers_the_target_design_projects_same_dbnum_file() {
        let global = PathBuf::from(r"D:\selected\AMS\ams7000_0001");
        let contextual = PathBuf::from(r"D:\selected\ACP\acp7000_0001");
        let locator = InMemoryCataLocator::from_parts(
            HashMap::from([(23984, 7600)]),
            HashMap::from([
                (7000, ("CATA".into(), "AMS".into(), global)),
                (
                    7015,
                    (
                        "DESI".into(),
                        "ACP".into(),
                        PathBuf::from(r"D:\selected\ACP\acp7015_0001"),
                    ),
                ),
            ]),
        );
        let resolver = E3dDbResolver {
            locator,
            project_files: Arc::new(HashMap::from([(
                project_db_key("ACP", 7000),
                ProjectDbFile {
                    db_type: "CATA".into(),
                    project: "ACP".into(),
                    path: contextual.clone(),
                },
            )])),
            target_dbnum: 7015,
            preferred_project: Some("ACP".into()),
            template_dir: PathBuf::from(r"D:\templates"),
            sessions: Arc::new(std::sync::Mutex::new(BTreeMap::from([((7015, 7000), 42)]))),
        };
        let pin = resolver.resolve(7000).expect("contextual CATA pin");
        assert_eq!(pin.file, contextual);
        assert_eq!(pin.sesno, Some(42));
        assert_eq!(pin.db_type.as_deref(), Some("CATA"));
        assert!(pin.template.ends_with("catvir.dat"));
    }
}
