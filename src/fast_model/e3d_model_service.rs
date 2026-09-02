//! 生产环境唯一的 `e3d-model` 生成、增量和 Plant UI 兼容持久化入口。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aios_core::{RefU64, RefnoEnum};
use aios_core::shape::pdms_shape::PlantMesh;
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
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::sql::Thing;
use tokio::sync::Mutex;

use crate::data_interface::cata_closure::{CataDbLocator, InMemoryCataLocator};
use crate::data_interface::direct_store::{DbPin, DirectSchema};
use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::data_interface::mdb_membership;
use crate::data_interface::model_source;
use crate::fast_model::room_publication::RoomEffectPolicy;

#[derive(Debug, Clone)]
struct RootPublishClaim {
    root: RefNo,
    revision: u64,
    desired_source_sesno: u32,
    model_target: Option<ModelTarget>,
    published_model_target: Option<ModelTarget>,
    published_manifest_hash: Option<String>,
    published_geometry_count: Option<usize>,
    published_projection_complete: bool,
}

#[derive(Debug, Deserialize)]
struct RootPublishClaimRow {
    desired_revision: u64,
    #[serde(default)]
    desired_target: Option<RootDesiredTargetRow>,
    #[serde(default)]
    published_model_target: Option<ModelTarget>,
    #[serde(default)]
    published_manifest_hash: Option<String>,
    #[serde(default)]
    published_geometry_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RootDesiredTargetRow {
    #[serde(default)]
    source_end_sesno: u32,
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
#[derive(Debug)]
pub(crate) struct SourceIndex {
    pub(crate) roots: Vec<RefNo>,
    pub(crate) owners: BTreeMap<(u32, u32), RefNo>,
}

/// Pure, session-pinned generation result.  It contains no database handle and
/// is safe to route either to the current projection or to the historical
/// in-memory projection.
pub struct GeneratedSnapshot {
    pub elements: Vec<GeneratedElement>,
    pub owners: BTreeMap<(u32, u32), RefNo>,
    pub report: Report,
}

static DB_GENERATION_LOCKS: OnceLock<dashmap::DashMap<u32, Arc<Mutex<()>>>> = OnceLock::new();

fn db_generation_lock(dbnum: u32) -> Arc<Mutex<()>> {
    DB_GENERATION_LOCKS
        .get_or_init(dashmap::DashMap::new)
        .entry(dbnum)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub struct E3dModelService {
    schema: DirectSchema,
    pins: Vec<DbPin>,
    locator: InMemoryCataLocator,
    dependency_sessions: Arc<std::sync::Mutex<BTreeMap<u32, u32>>>,
    mesh_dir: PathBuf,
}

#[derive(Clone)]
struct E3dDbResolver {
    locator: InMemoryCataLocator,
    template_dir: PathBuf,
    sessions: Arc<std::sync::Mutex<BTreeMap<u32, u32>>>,
}

impl DbFileResolver for E3dDbResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        let db_type = self.locator.db_type_of(dbno)?;
        let (_project, file) = self.locator.file_of(dbno)?;
        let template = template_file_for(&self.template_dir, &db_type).ok()?;
        let sesno = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(&sesno) = sessions.get(&dbno) {
                sesno
            } else {
                let mut view = DbView::open(&file).ok()?;
                let sesno = view.walk_sessions().ok()?.first()?.sesno;
                sessions.insert(dbno, sesno);
                sesno
            }
        };
        Some(DbFilePin {
            file,
            template,
            db_type: Some(db_type),
            sesno: Some(sesno),
        })
    }
}

impl E3dModelService {
    /// The current projection (ADR-054): source files come from the MDB
    /// declaration, and every pin is left unpinned so that generation resolves
    /// the target database's **latest file session** at the moment it runs
    /// (`source_session`). `dbnum_watermark.applied_sesno` is the ingestion
    /// watermark, not a generation timepoint, and an empty watermark table must
    /// not stop a never-parsed project from generating.
    pub async fn from_current() -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        let (pins, locator) = current_mdb_sources()?;
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
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir: option.get_meshes_path(),
        })
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
        let index = scan_index(&pin.file, Some(source_sesno))?;
        let set = self.build_set(dbnum, Some(source_sesno))?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &set,
            &index,
            &roots,
            failure_policy,
            RoomEffectPolicy::Directed,
        )
        .await
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
        let index = scan_index(&pin.file, Some(source_sesno))?;
        let set = self.build_set(dbnum, Some(source_sesno))?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &set,
            &index,
            &index.roots,
            failure_policy,
            RoomEffectPolicy::FullDatabase,
        )
        .await
    }

    /// Generate one root at an exact source session without reading or writing
    /// the persisted model projection.
    pub async fn generate_snapshot_source(
        &self,
        dbnum: u32,
        root: RefNo,
        source_sesno: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<GeneratedSnapshot> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self
            .pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .with_context(|| format!("MDB has no source file for dbnum {dbnum}"))?;
        let index = scan_index(&pin.file, Some(source_sesno))?;
        let set = self.build_set(dbnum, Some(source_sesno))?;
        let mut cache = CatalogueMeshCache::default();
        let snapshot = generate_snapshot_from_set(&set, &index, root, &mut cache)?;
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
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir,
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
        room_policy: RoomEffectPolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let mut total = E3dPersistReport::default();
        let mut generation = Report::default();
        let mut publication = String::new();
        let mut prepared_reports = Vec::new();
        let mut spatial_refnos = BTreeSet::new();
        let room_incremental = crate::options::room_incremental();
        let (mut room_recalc_targets, mut room_cleared_sources) = (0usize, 0usize);
        // A production batch is one reuse window: identical evaluated catalogue
        // primitives across different delivery roots must share their local mesh.
        let mut catalogue_mesh_cache = CatalogueMeshCache::default();
        for &root in roots {
            // Capture the durable revision before any expensive source or CATA
            // evaluation. The publication transaction re-checks this token.
            let mut publish_claim = load_root_publish_claim(root).await?;
            if let Some(claim) = publish_claim.as_mut() {
                self.hydrate_published_dependencies(claim)?;
                claim.model_target = Some(self.model_target(dbnum, source_sesno)?);
                claim.published_projection_complete =
                    published_root_projection_complete(claim, &self.mesh_dir).await?;
                if cached_root_target_matches(claim, source_sesno)
                    && claim.published_projection_complete
                {
                    publication.push_str(&prepare_cached_root_publication(claim, source_sesno)?);
                    prepared_reports.push(E3dPersistReport::default());
                    continue;
                }
            }
            let snapshot =
                match generate_snapshot_from_set(set, index, root, &mut catalogue_mesh_cache) {
                    Ok(outcome) => outcome,
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        // A failed candidate is not an empty candidate. Keep the
                        // last complete published root visible until a complete
                        // replacement is available.
                        total.failed += 1;
                        log::error!("e3d-model root {root} failed: {error:#}");
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| format!("generate root {root}"));
                    }
                };
            let root_failed = snapshot.report.failed.len();
            if root_failed > 0 && matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model root {root} has {root_failed} failed element(s)");
            }
            if root_failed > 0 {
                // Best-effort generation may report successful siblings next to
                // failed elements. Publishing that partial manifest would delete
                // the missing GeometryIds, so retain the previous complete root.
                let root_skipped = snapshot.report.skipped.len();
                generation.merge(snapshot.report);
                total.failed += root_failed;
                total.skipped += root_skipped;
                log::error!(
                    "e3d-model root {root} produced an incomplete candidate ({root_failed} failure(s)); preserving published snapshot"
                );
                continue;
            }
            let root_skipped = snapshot.report.skipped.len();
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
            let mut removals = existing_geometry_ids(dbnum, root)
                .await?
                .into_iter()
                .filter_map(|geometry_id| {
                    let key = serde_json::to_string(&geometry_id).ok()?;
                    (!generated_ids.contains_key(&key)).then_some(geometry_id)
                })
                .collect::<BTreeSet<_>>();
            // A root generated by e3d-model owns its complete visible geometry.
            // Pre-GeometryId rows are old OCC/gen_model projections; leaving
            // descendants behind makes consumed cutters visible as positive
            // Plant UI meshes.  Remove only ordinary PE-backed legacy rows in
            // this regenerated root.  Shared inst_geo/mesh content is retained.
            let pre_e3d_removal_count = pre_e3d_relation_count(dbnum, root).await?;
            // The legacy root cleanup below deletes rows that do not carry a
            // GeometryId, so `removal_vec` cannot name their source PE keys.
            // Capture those committed AABB owners before the transaction; they
            // must be removed from the in-process spatial mirror afterwards.
            let pre_e3d_spatial_refnos = if pre_e3d_removal_count == 0 {
                BTreeSet::new()
            } else {
                pre_e3d_spatial_refnos(dbnum, root).await?
            };
            generation.merge(snapshot.report);
            ensure_not_older_than_persisted(dbnum, source_sesno).await?;
            if let Some(claim) = publish_claim.as_mut() {
                // Catalogue resolution during generation may have discovered
                // cross-database dependencies that were not in watermark pins.
                claim.model_target = Some(self.model_target(dbnum, source_sesno)?);
            }
            let removal_vec = removals.into_iter().collect::<Vec<_>>();
            // 房间派生面（ADR-010 §4 / ADR-040 §3）跟着这个根的几何进同一个发布事务：
            // 重写的元素排重算，移除的与失去几何的旧来源清边。折算要在
            // `snapshot.elements` 被搬进 `prepare_geometry_delta` 之前做。
            let room_effects = crate::fast_model::room_publication::room_publication_effects(
                snapshot
                    .elements
                    .iter()
                    .map(|element| (&element.geometry_id, element.refno, element.noun.as_str())),
                &removal_vec,
                &pre_e3d_spatial_refnos,
            )?;
            let mut root_spatial_refnos =
                spatial_refnos_for_delta(&snapshot.elements, &removal_vec)?;
            root_spatial_refnos.extend(pre_e3d_spatial_refnos);
            let (statements, mut persisted) = prepare_geometry_delta(
                ProjectionScope::Current,
                dbnum,
                source_sesno,
                snapshot.elements,
                removal_vec,
                Some((root, pre_e3d_removal_count)),
                publish_claim.as_ref(),
                &index.owners,
                &self.mesh_dir,
            )?;
            persisted.skipped += root_skipped;
            persisted.failed += root_failed;
            publication.push_str(&statements);
            if persisted.upserted > 0 || persisted.removed > 0 {
                spatial_refnos.extend(root_spatial_refnos);
                publication.push_str(
                    &crate::fast_model::room_publication::render_room_publication_effects(
                        &room_effects,
                        room_policy,
                        room_incremental,
                    ),
                );
                if room_policy == RoomEffectPolicy::Directed && room_incremental {
                    room_recalc_targets += room_effects.recalc.len();
                }
                room_cleared_sources += room_effects.cleared.len();
            }
            prepared_reports.push(persisted);
        }
        if total.failed == 0 && !publication.is_empty() {
            // One generate_roots call is one publication cohort. This makes an
            // OWNER move (old root cleanup + new root takeover), and a PIPE
            // expansion containing several BRANs, visible in one commit.
            let _spatial_serial = if spatial_refnos.is_empty() {
                None
            } else {
                Some(crate::fast_model::spatial_state::lock_spatial_serial().await)
            };
            if !spatial_refnos.is_empty() {
                publication.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            }
            aios_core::SUL_DB
                .query(publication_transaction(&publication))
                .await?
                .check()?;
            if !spatial_refnos.is_empty() {
                let refnos = spatial_refnos.into_iter().collect::<Vec<_>>();
                crate::fast_model::aabb_tree::sync_tree_from_committed_pointers(&refnos).await?;
                // A window may also remove PE-backed legacy rows for roots that
                // produce no GeometryId work item.  Re-scan the committed pointer
                // set once per publication cohort so those non-geometry deletes
                // cannot leave stale entries in the process mirror.
                crate::fast_model::aabb_tree::rebuild_tree_from_pointers().await?;
            }
            if room_recalc_targets > 0 || room_cleared_sources > 0 {
                log::info!(
                    "e3d-model publication for dbnum {dbnum}: {room_recalc_targets} room recalc target(s) queued, {room_cleared_sources} removed source(s) cleared of room edges"
                );
            }
            for report in prepared_reports {
                total.merge_counts(report);
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

    /// Every design database of the current MDB as `(dbnum, file, source session)`,
    /// the session being the one generation would read right now (ADR-054 Q1).
    /// Derived faces that must not depend on parsed `pe` rows (room topology,
    /// `room_topology`) enumerate the files through this.
    pub(crate) fn design_sources(&self) -> anyhow::Result<Vec<(u32, PathBuf, u32)>> {
        self.pins
            .iter()
            .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
            .map(|pin| Ok((pin.dbnum as u32, pin.file.clone(), self.source_session(pin)?)))
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

    fn model_target(&self, dbnum: u32, source_sesno: u32) -> anyhow::Result<ModelTarget> {
        let design_pin = self.pin(dbnum)?;
        let sessions = self
            .dependency_sessions
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
                digest: model_file_identity_digest(file, session)?,
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
                .get(&(pin.dbnum as u32))
                .copied()
                .or(pin.sesno)
                .unwrap_or_default();
            catalogue.insert(
                pin.dbnum as u32,
                file_target(pin.dbnum as u32, &pin.db_type, &pin.file, session)?,
            );
        }
        for (&dependency_dbnum, &session) in sessions.iter() {
            if catalogue.contains_key(&dependency_dbnum) {
                continue;
            }
            let Some(db_type) = self.locator.db_type_of(dependency_dbnum) else {
                continue;
            };
            if !db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            let Some((_project, file)) = self.locator.file_of(dependency_dbnum) else {
                continue;
            };
            catalogue.insert(
                dependency_dbnum,
                file_target(dependency_dbnum, &db_type, &file, session)?,
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
                "e3d-model:{};aios-database:{};route-atta-pass-through-v1;catalogue-atta-clfl-frame-v1",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_VERSION")
            ),
            tessellation_profile: "e3d-default-v1".to_string(),
        })
    }

    fn hydrate_published_dependencies(&self, claim: &RootPublishClaim) -> anyhow::Result<()> {
        let Some(published) = claim.published_model_target.as_ref() else {
            return Ok(());
        };
        let mut sessions = self
            .dependency_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for dependency in &published.catalogue {
            if sessions.contains_key(&dependency.dbnum) {
                continue;
            }
            let Some((_project, file)) = self.locator.file_of(dependency.dbnum) else {
                continue;
            };
            let mut view = DbView::open(&file)?;
            if let Some(session) = view.walk_sessions()?.first() {
                sessions.insert(dependency.dbnum, session.sesno);
            }
        }
        Ok(())
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
        let set = Arc::new(DbSet::with_attlib_file_and_resolver(
            self.schema.template_dir().join("attlib.dat"),
            Box::new(E3dDbResolver {
                locator: self.locator.clone(),
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
pub(crate) fn current_mdb_sources() -> anyhow::Result<(Vec<DbPin>, InMemoryCataLocator)> {
    let option = aios_core::get_db_option();
    let project = option.project_name.clone();
    let mdb = option.mdb_name.clone();
    let membership = mdb_membership::get(&project, &mdb)
        .map(Ok)
        .unwrap_or_else(|| {
            mdb_membership::resolve(option, &project, &mdb).map(std::sync::Arc::new)
        })?;
    let mut files = HashMap::new();
    let mut pins = Vec::new();
    for item in membership.databases() {
        let Some(path) = item.path.clone() else {
            continue;
        };
        let Some(db_type) = styp_name(item.styp) else {
            continue;
        };
        let source_project = item.project.clone().unwrap_or_else(|| project.clone());
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
    Ok((pins, InMemoryCataLocator::from_parts(HashMap::new(), files)))
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
        owners: index.owners.clone(),
        report: outcome.report,
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
        self.unique_meshes = self.mesh_ids.len();
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
    let spatial_refnos = spatial_refnos_for_delta(&upserts, &removals)?;
    // 窗口 / 删根路径都是定向的：重写的元素排房间重算，移除的清边（ADR-010 §4 / ADR-040 §3）。
    let room_effects = crate::fast_model::room_publication::room_publication_effects(
        upserts
            .iter()
            .map(|element| (&element.geometry_id, element.refno, element.noun.as_str())),
        &removals,
        &BTreeSet::new(),
    )?;
    let _spatial_serial = if spatial_refnos.is_empty() {
        None
    } else {
        Some(crate::fast_model::spatial_state::lock_spatial_serial().await)
    };
    let (mut statements, mut report) = prepare_geometry_delta(
        ProjectionScope::Current,
        dbnum,
        source_sesno,
        upserts,
        removals,
        None,
        None,
        owners,
        mesh_dir,
    )?;
    let changed = report.upserted > 0 || report.removed > 0;
    if changed {
        statements.push_str(
            &crate::fast_model::room_publication::render_room_publication_effects(
                &room_effects,
                RoomEffectPolicy::Directed,
                crate::options::room_incremental(),
            ),
        );
    }
    if changed && !spatial_refnos.is_empty() {
        statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    }
    if changed {
        aios_core::SUL_DB
            .query(publication_transaction(&statements))
            .await?
            .check()?;
    }
    if changed && !spatial_refnos.is_empty() {
        let refnos = spatial_refnos.into_iter().collect::<Vec<_>>();
        crate::fast_model::aabb_tree::sync_tree_from_committed_pointers(&refnos).await?;
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok(report)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProjectionScope<'a> {
    Current,
    Historical(&'a str),
}

impl ProjectionScope<'_> {
    fn id(self, raw: &str) -> String {
        match self {
            Self::Current => raw.to_string(),
            Self::Historical(snapshot) => format!("hist_{snapshot}_{raw}"),
        }
    }

    fn object_field(self) -> String {
        match self {
            Self::Current => String::new(),
            Self::Historical(snapshot) => format!(",snapshot_key:'{snapshot}'"),
        }
    }

    fn set_assignment(self) -> String {
        match self {
            Self::Current => String::new(),
            Self::Historical(snapshot) => format!(",snapshot_key='{snapshot}'"),
        }
    }

    fn direct_field(self) -> String {
        match self {
            Self::Current => String::new(),
            Self::Historical(snapshot) => format!(",snapshot_key:'{snapshot}'"),
        }
    }
}

fn append_root_dependency_receipt(statements: &mut String, claim: &RootPublishClaim) {
    let Some(target) = claim.model_target.as_ref() else {
        return;
    };
    statements.push_str(&format!(
        "DELETE root_dependency WHERE root_refno='{}';\n",
        claim.root
    ));
    for dependency in &target.catalogue {
        statements.push_str(&format!(
            "UPSERT type::thing('root_dependency','{}_{}_{}') SET \
             root_refno='{}', dependency_kind='CATA', dependency_dbnum={}, \
             dependency_session={}, dependency_digest='{}', updated_at=time::now();\n",
            claim.root.word0,
            claim.root.word1,
            dependency.dbnum,
            claim.root,
            dependency.dbnum,
            dependency.session,
            dependency.digest,
        ));
    }
}

fn prepare_cached_root_publication(
    claim: &RootPublishClaim,
    source_sesno: u32,
) -> anyhow::Result<String> {
    let target = claim
        .model_target
        .as_ref()
        .context("cached publication has no current ModelTarget")?;
    let manifest_hash = claim
        .published_manifest_hash
        .as_deref()
        .context("cached publication has no manifest hash")?;
    let geometry_count = claim
        .published_geometry_count
        .context("cached publication has no geometry count")?;
    let root_id = format!("{}_{}", claim.root.word0, claim.root.word1);
    let model_target = sql_value(&serde_json::to_value(target)?);
    let mut statements = format!(
        "LET $publish_root = (SELECT * FROM type::thing('gen_root', '{root_id}') LIMIT 1)[0];\n\
         IF $publish_root = NONE OR ($publish_root.desired_revision?:0) != {revision} {{ THROW 'stale root publication revision'; }};\n\
         IF {desired_sesno} > 0 AND {source_sesno} != {desired_sesno} {{ THROW 'mixed model target publication'; }};\n\
         UPDATE type::thing('gen_root', '{root_id}') SET \
          status='Generated', publication_status='ready', \
          published_revision={revision}, published_target=desired_target, \
          desired_model_target={model_target}, published_model_target={model_target}, \
          published_manifest_hash='{}', published_geometry_count={geometry_count}, \
          source_end_sesno={source_sesno}, last_error=NONE, updated_at=time::now();\n",
        manifest_hash,
        revision = claim.revision,
        desired_sesno = claim.desired_source_sesno,
    );
    append_root_dependency_receipt(&mut statements, claim);
    Ok(statements)
}

async fn cached_root_projection_complete(
    claim: &RootPublishClaim,
    source_sesno: u32,
    mesh_dir: &Path,
) -> anyhow::Result<bool> {
    cached_root_projection_complete_on(&aios_core::SUL_DB, claim, source_sesno, mesh_dir).await
}

async fn cached_root_projection_complete_on(
    db: &Surreal<Any>,
    claim: &RootPublishClaim,
    source_sesno: u32,
    mesh_dir: &Path,
) -> anyhow::Result<bool> {
    if !cached_root_target_matches(claim, source_sesno) {
        return Ok(false);
    }
    published_root_projection_complete_on(db, claim, mesh_dir).await
}

fn cached_root_target_matches(claim: &RootPublishClaim, source_sesno: u32) -> bool {
    if claim.desired_source_sesno > 0 && claim.desired_source_sesno != source_sesno {
        return false;
    }
    let Some(current) = claim.model_target.as_ref() else {
        return false;
    };
    if claim.published_model_target.as_ref() != Some(current)
        || claim.published_manifest_hash.is_none()
    {
        return false;
    }
    true
}

async fn published_root_projection_complete(
    claim: &RootPublishClaim,
    mesh_dir: &Path,
) -> anyhow::Result<bool> {
    published_root_projection_complete_on(&aios_core::SUL_DB, claim, mesh_dir).await
}

async fn published_root_projection_complete_on(
    db: &Surreal<Any>,
    claim: &RootPublishClaim,
    mesh_dir: &Path,
) -> anyhow::Result<bool> {
    let Some(expected_count) = claim.published_geometry_count else {
        return Ok(false);
    };
    let packed = (((claim.root.word0 as u64) << 32) | claim.root.word1 as u64) as i64;
    let mut response = db
        .query(format!(
            "SELECT VALUE direct_model.mesh_id FROM inst_relate \
             WHERE direct_model.source='e3d-model' AND anc CONTAINS {packed};\
             SELECT VALUE direct_model.mesh_id FROM tubi_relate \
             WHERE direct_model.source='e3d-model' AND anc CONTAINS {packed};"
        ))
        .await?
        .check()?;
    let mut mesh_ids: Vec<Option<String>> = response.take(0)?;
    mesh_ids.extend(response.take::<Vec<Option<String>>>(1)?);
    Ok(mesh_ids.len() == expected_count
        && mesh_ids.iter().all(|mesh_id| {
            mesh_id
                .as_ref()
                .is_some_and(|mesh_id| mesh_dir.join(format!("{mesh_id}.mesh")).is_file())
        }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_geometry_delta_on(
    db: &Surreal<Any>,
    scope: ProjectionScope<'_>,
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    apply_geometry_delta_on_with_pre_e3d(
        db,
        scope,
        dbnum,
        source_sesno,
        upserts,
        removals,
        None,
        None,
        owners,
        mesh_dir,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_geometry_delta_on_with_pre_e3d(
    db: &Surreal<Any>,
    scope: ProjectionScope<'_>,
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    pre_e3d_cleanup: Option<(RefNo, usize)>,
    publish_claim: Option<&RootPublishClaim>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    let (statements, mut report) = prepare_geometry_delta(
        scope,
        dbnum,
        source_sesno,
        upserts,
        removals,
        pre_e3d_cleanup,
        publish_claim,
        owners,
        mesh_dir,
    )?;
    if report.upserted > 0 || report.removed > 0 || publish_claim.is_some() {
        db.query(publication_transaction(&statements))
            .await?
            .check()?;
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn prepare_geometry_delta(
    scope: ProjectionScope<'_>,
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    pre_e3d_cleanup: Option<(RefNo, usize)>,
    publish_claim: Option<&RootPublishClaim>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<(String, E3dPersistReport)> {
    let mut report = E3dPersistReport::default();
    let mut statements = String::new();
    if let Some(claim) = publish_claim {
        let root_id = format!("{}_{}", claim.root.word0, claim.root.word1);
        statements.push_str(&format!(
            "LET $publish_root = (SELECT * FROM type::thing('gen_root', '{root_id}') LIMIT 1)[0];\n\
             IF $publish_root = NONE OR ($publish_root.desired_revision?:0) != {revision} {{ THROW 'stale root publication revision'; }};\n\
             IF {desired_sesno} > 0 AND {source_sesno} != {desired_sesno} {{ THROW 'mixed model target publication'; }};\n",
            revision = claim.revision,
            desired_sesno = claim.desired_source_sesno,
        ));
    }
    let publication_guard_len = statements.len();
    let object_field = scope.object_field();
    let set_assignment = scope.set_assignment();
    let direct_field = scope.direct_field();
    let touched_tube_branches = removals
        .iter()
        .chain(upserts.iter().map(|element| &element.geometry_id))
        .filter_map(|geometry_id| match geometry_id {
            GeometryId::ImpliedTube {
                container_refno, ..
            } => Some(container_refno.as_str()),
            GeometryId::Element { .. } => None,
        })
        .map(|value| parse_refno(value).map(|refno| (refno.word0, refno.word1)))
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    for &(word0, word1) in &touched_tube_branches {
        match scope {
            ProjectionScope::Current => {
                statements.push_str(&format!("DELETE pe:⟨{}_{}⟩->tubi_relate;\n", word0, word1))
            }
            ProjectionScope::Historical(snapshot) => statements.push_str(&format!(
                "DELETE tubi_relate WHERE snapshot_key='{snapshot}' AND source_refno='{}_{}';\n",
                word0, word1
            )),
        }
    }

    for geometry_id in &removals {
        let source = geometry_source_refno(&geometry_id)?;
        let id = scope.id(&geometry_record_id(&geometry_id, source));
        append_geometry_removal(&mut statements, &id);
        report.removed += 1;
    }

    if let Some((root, expected_count)) = pre_e3d_cleanup {
        anyhow::ensure!(
            matches!(scope, ProjectionScope::Current),
            "pre-e3d cleanup is valid only for the current production projection"
        );
        append_pre_e3d_root_cleanup(&mut statements, dbnum, root);
        report.removed += expected_count;
    }

    let mut tube_indices = BTreeMap::<(u32, u32), usize>::new();
    let mut manifest_entries = BTreeMap::<String, Value>::new();
    for element in upserts {
        let source_refno = geometry_source_refno(&element.geometry_id)?;
        let source_id = format!("{}_{}", source_refno.word0, source_refno.word1);
        let id = scope.id(&geometry_record_id(&element.geometry_id, source_refno));
        let geometry_value = serde_json::to_value(&element.geometry_id)?;
        let geometry_key = serde_json::to_string(&element.geometry_id)?;
        let geometry_json = sql_value(&geometry_value);
        let anc = ancestor_chain(source_refno, owners)?;
        let world_mesh = element.parts.as_deref().map_or_else(
            || crate::fast_model::manifold_csg::manifold_to_plant_mesh(&element.solid),
            crate::fast_model::manifold_csg::manifolds_to_plant_mesh,
        );
        let world_aabb = world_mesh.aabb.context("world PlantMesh missing AABB")?;
        let world_bounds = [point3(&world_aabb.mins), point3(&world_aabb.maxs)];
        // TUBI relations expose only one transform to Plant UI. A canonical
        // tube therefore needs the element placement and the canonical local
        // scale/centering composed into that single stored transform.
        let persisted_world = persisted_world_transform(
            &element.geometry_id,
            element.primitive_instance.as_ref(),
            element.world,
        );
        let world_transform = transform_value(persisted_world);

        append_geometry_representation_cleanup(&mut statements, &id);
        statements.push_str(&format!(
            "UPSERT type::thing('aabb','direct_world_{id}') CONTENT {{d:{{mins:{},maxs:{}}}{object_field}}};\n\
             UPSERT type::thing('trans','direct_world_{id}') CONTENT {{d:{}{object_field}}};\n",
            sql_value(&json!(world_bounds[0])),
            sql_value(&json!(world_bounds[1])),
            sql_value(&world_transform),
        ));

        if let GeometryId::ImpliedTube {
            route_ordinal,
            from_refno,
            to_refno,
            ..
        } = &element.geometry_id
        {
            let from = parse_refno(from_refno)?;
            let to = parse_refno(to_refno)?;
            let (mesh_id, mesh, format, primitive_key) =
                if let Some(instance) = element.primitive_instance.as_ref() {
                    let canonical = canonical_primitive_mesh(instance.key)?;
                    let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical);
                    (
                        canonical_mesh_id(instance.key, &mesh),
                        mesh,
                        "canonical-primitive-v1",
                        Some(sql_value(&serde_json::to_value(instance.key)?)),
                    )
                } else {
                    let mesh = local_generated_mesh(&element);
                    (baked_mesh_id(&mesh), mesh, "baked-v2", None)
                };
            note_mesh_write(
                &mut report,
                ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?,
            );
            report.mesh_ids.insert(mesh_id.clone());
            manifest_entries.insert(
                geometry_key,
                json!({
                    "geometry_id": geometry_value,
                    "mesh": mesh_id,
                    "world": world_transform,
                    "noun": element.noun,
                    "solid": true,
                    "route_ordinal": route_ordinal,
                }),
            );
            // `route_ordinal` belongs to one `(from, to)` pair, so unrelated
            // route segments commonly both carry zero. The legacy relation id,
            // however, is branch-global; give every emitted segment its own slot.
            let index = next_tube_relation_slot(&mut tube_indices, source_refno);
            statements.push_str(&legacy_tubi_relation_sql(
                &source_id,
                index,
                &mesh_id,
                &id,
                from,
                to,
                &sql_value(&json!(anc)),
                dbnum,
                source_sesno,
                &geometry_json,
                format,
                primitive_key.as_deref(),
                scope,
            ));
            if primitive_key.is_some() {
                report.shared_instances += 1;
            } else {
                report.baked_instances += 1;
            }
            report.upserted += 1;
            continue;
        }

        if let Some(instance) = element.primitive_instance {
            let canonical = canonical_primitive_mesh(instance.key)?;
            let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical);
            let mesh_id = canonical_mesh_id(instance.key, &mesh);
            note_mesh_write(
                &mut report,
                ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?,
            );
            report.mesh_ids.insert(mesh_id.clone());
            let local_aabb = mesh.aabb.context("canonical PlantMesh missing AABB")?;
            let local_transform = transform_value(instance.local_transform);
            let key = sql_value(&serde_json::to_value(instance.key)?);
            manifest_entries.insert(
                geometry_key,
                json!({
                    "geometry_id": geometry_value,
                    "mesh": mesh_id,
                    "world": world_transform,
                    "local": local_transform,
                    "noun": element.noun,
                    "solid": true,
                    "primitive_key": instance.key,
                }),
            );
            statements.push_str(&format!(
                "UPSERT type::thing('aabb','direct_shared_{mesh_id}') CONTENT {{d:{{mins:{},maxs:{}}}}};\n\
                 UPSERT type::thing('trans','direct_local_{id}') CONTENT {{d:{}{object_field}}};\n\
                 UPSERT inst_geo:⟨{mesh_id}⟩ SET meshed=true,visible=true,bad=false,aabb=type::thing('aabb','direct_shared_{mesh_id}'),direct_model={{source:'e3d-model',format:'canonical-primitive-v1'}};\n\
                 UPSERT type::thing('inst_info','direct_{id}') SET dbnum={dbnum},noun='{}'{set_assignment},direct_model={{source:'e3d-model',sesno:{source_sesno},geometry_id:{geometry_json}{direct_field}}};\n\
                 RELATE inst_info:⟨direct_{id}⟩->geo_relate->inst_geo:⟨{mesh_id}⟩ SET geom_refno=type::thing('pe','{source_id}'),trans=type::thing('trans','direct_local_{id}'),visible=true,geo_type='Pos'{set_assignment};\n\
                 UPSERT type::thing('inst_relate','{id}') SET in=type::thing('pe','{source_id}'),out=type::thing('inst_info','direct_{id}'),booled_id=NONE,booled=false,bad_bool=false,solid=true,generic='{}',dbnum={dbnum},anc={},aabb=type::thing('aabb','direct_world_{id}'),world_trans=type::thing('trans','direct_world_{id}'),insts_flat=[{{geo_hash:'{mesh_id}',transform:{}}}]{set_assignment},direct_model={{source:'e3d-model',format:'canonical-primitive-v1',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}',primitive_key:{key}{direct_field}}};\n",
                sql_value(&json!(point3(&local_aabb.mins))),
                sql_value(&json!(point3(&local_aabb.maxs))),
                sql_value(&local_transform),
                element.noun,
                element.noun,
                sql_value(&json!(anc)),
                sql_value(&local_transform),
            ));
            report.shared_instances += 1;
        } else {
            let mesh = local_generated_mesh(&element);
            let mesh_id = baked_mesh_id(&mesh);
            note_mesh_write(
                &mut report,
                ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?,
            );
            report.mesh_ids.insert(mesh_id.clone());
            manifest_entries.insert(
                geometry_key,
                json!({
                    "geometry_id": geometry_value,
                    "mesh": mesh_id,
                    "world": world_transform,
                    "noun": element.noun,
                    "solid": true,
                    "booled": true,
                }),
            );
            statements.push_str(&format!(
                "UPSERT type::thing('inst_relate','{id}') SET in=type::thing('pe','{source_id}'),booled_id='{mesh_id}',booled=true,bad_bool=false,solid=true,generic='{}',dbnum={dbnum},anc={},aabb=type::thing('aabb','direct_world_{id}'),world_trans=type::thing('trans','direct_world_{id}'),insts_flat=[{{geo_hash:'{mesh_id}'}}]{set_assignment},direct_model={{source:'e3d-model',format:'baked-v2',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}'{direct_field}}};\n",
                element.noun,
                sql_value(&json!(anc)),
            ));
            report.baked_instances += 1;
        }
        report.upserted += 1;
    }
    if let Some(claim) = publish_claim {
        let root_id = format!("{}_{}", claim.root.word0, claim.root.word1);
        let model_target = claim
            .model_target
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .map(|value| sql_value(&value))
            .unwrap_or_else(|| "NONE".to_string());
        let manifest_json = serde_json::to_vec(&manifest_entries)?;
        let manifest_hash = hex::encode(Sha256::digest(&manifest_json));
        let geometry_count = manifest_entries.len();
        if claim.published_projection_complete
            && claim.published_manifest_hash.as_deref() == Some(manifest_hash.as_str())
        {
            // The immutable target changed, but this root's visible manifest did
            // not. Keep the revision/receipt transition and omit every geometry
            // DELETE/UPSERT from the publication transaction.
            statements.truncate(publication_guard_len);
            report.upserted = 0;
            report.removed = 0;
        }
        statements.push_str(&format!(
            "UPDATE type::thing('gen_root', '{root_id}') SET \
             status='Generated', publication_status='ready', \
             published_revision={revision}, published_target=desired_target, \
             desired_model_target={model_target}, published_model_target={model_target}, \
             published_manifest_hash='{manifest_hash}', published_geometry_count={geometry_count}, \
             source_end_sesno={source_sesno}, last_error=NONE, updated_at=time::now();\n",
            revision = claim.revision,
        ));
        append_root_dependency_receipt(&mut statements, claim);
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok((statements, report))
}

fn local_generated_mesh(element: &GeneratedElement) -> PlantMesh {
    let inverse_world = element.world.inverse();
    element.parts.as_deref().map_or_else(
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
    )
}

async fn load_root_publish_claim(root: RefNo) -> anyhow::Result<Option<RootPublishClaim>> {
    let root_id = format!("{}_{}", root.word0, root.word1);
    let mut response = aios_core::SUL_DB
        .query(format!(
            "SELECT desired_revision, desired_target, published_model_target, \
             published_manifest_hash, published_geometry_count \
             FROM ONLY type::thing('gen_root', '{root_id}');"
        ))
        .await?
        .check()?;
    let row: Option<RootPublishClaimRow> = response.take(0)?;
    Ok(row.map(|row| RootPublishClaim {
        root,
        revision: row.desired_revision,
        desired_source_sesno: row
            .desired_target
            .map(|target| target.source_end_sesno)
            .unwrap_or_default(),
        model_target: None,
        published_model_target: row.published_model_target,
        published_manifest_hash: row.published_manifest_hash,
        published_geometry_count: row.published_geometry_count,
        published_projection_complete: false,
    }))
}

fn model_file_identity_digest(path: &Path, session: u32) -> anyhow::Result<String> {
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

fn next_tube_relation_slot(slots: &mut BTreeMap<(u32, u32), usize>, source_refno: RefNo) -> usize {
    let next = slots
        .entry((source_refno.word0, source_refno.word1))
        .or_default();
    let current = *next;
    *next += 1;
    current
}

#[allow(clippy::too_many_arguments)]
fn legacy_tubi_relation_sql(
    source_id: &str,
    index: usize,
    mesh_id: &str,
    representation_id: &str,
    from: RefNo,
    to: RefNo,
    anc_literal: &str,
    dbnum: u32,
    source_sesno: u32,
    geometry_json: &str,
    format: &str,
    primitive_key: Option<&str>,
    scope: ProjectionScope<'_>,
) -> String {
    let object_field = scope.object_field();
    let direct_field = scope.direct_field();
    let source_assignment = match scope {
        ProjectionScope::Current => String::new(),
        ProjectionScope::Historical(_) => format!(",source_refno:'{source_id}'"),
    };
    let relation_id = match scope {
        ProjectionScope::Current => format!("tubi_relate:[pe:⟨{source_id}⟩,{index}]"),
        ProjectionScope::Historical(_) => {
            format!("tubi_relate:⟨{representation_id}_{index}⟩")
        }
    };
    let primitive_field = primitive_key
        .map(|key| format!(",primitive_key:{key}"))
        .unwrap_or_default();
    format!(
        "UPSERT inst_geo:⟨{mesh_id}⟩ SET meshed=true,visible=true,bad=false,direct_model={{source:'e3d-model',format:'{format}'{primitive_field}}};\n\
         INSERT RELATION INTO tubi_relate [{{id:{relation_id},in:pe:⟨{source_id}⟩,out:inst_geo:⟨{mesh_id}⟩,leave:pe:⟨{}_{}⟩,arrive:pe:⟨{}_{}⟩,aabb:aabb:⟨direct_world_{representation_id}⟩,world_trans:trans:⟨direct_world_{representation_id}⟩,bore_size:0,invalid:false,anc:{anc_literal},dbnum:{dbnum}{source_assignment}{object_field},direct_model:{{source:'e3d-model',format:'{format}',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}'{primitive_field}{direct_field}}}}}];\n",
        from.word0, from.word1, to.word0, to.word1,
    )
}

fn append_geometry_representation_cleanup(statements: &mut String, id: &str) {
    statements.push_str(&format!(
        "DELETE geo_relate WHERE in=type::thing('inst_info','direct_{id}');\n\
         DELETE type::thing('inst_info','direct_{id}');\n\
         DELETE type::thing('trans','direct_local_{id}');\n\
         DELETE type::thing('inst_relate','{id}');\n"
    ));
}

fn append_geometry_removal(statements: &mut String, id: &str) {
    append_geometry_representation_cleanup(statements, id);
    statements.push_str(&format!(
        "DELETE type::thing('aabb','direct_world_{id}');\n\
         DELETE type::thing('trans','direct_world_{id}');\n"
    ));
}

fn append_pre_e3d_root_cleanup(statements: &mut String, dbnum: u32, root: RefNo) {
    // Candidate selection and deletion deliberately live in the same database
    // transaction as the e3d replacement.  Re-checking source/table/ancestry at
    // mutation time prevents a row that was concurrently replaced by e3d-model
    // from being deleted.  Only visibility-owning relations are removed here;
    // legacy inst_info/geo_relate/mesh objects are left for reachability GC.
    statements.push_str(&pre_e3d_delete_query(dbnum, root));
}

async fn existing_geometry_ids(dbnum: u32, root: RefNo) -> anyhow::Result<Vec<GeometryId>> {
    let query = existing_geometry_ids_query(dbnum, root);
    let mut response = aios_core::SUL_DB.query(query).await?.check()?;
    let mut rows: Vec<GeometryId> = response.take(0)?;
    rows.extend(response.take::<Vec<GeometryId>>(1)?);
    Ok(rows)
}

async fn pre_e3d_relation_count(dbnum: u32, root: RefNo) -> anyhow::Result<usize> {
    pre_e3d_relation_count_on(&aios_core::SUL_DB, dbnum, root).await
}

/// Source PE keys whose current-projection rows are about to be removed by the
/// pre-e3d root cleanup.  They have no GeometryId manifest, so this query must
/// run before publication and feed the post-commit spatial mirror sync.
async fn pre_e3d_spatial_refnos(
    dbnum: u32,
    root: RefNo,
) -> anyhow::Result<BTreeSet<RefnoEnum>> {
    pre_e3d_spatial_refnos_on(&aios_core::SUL_DB, dbnum, root).await
}

async fn pre_e3d_spatial_refnos_on(
    db: &Surreal<Any>,
    dbnum: u32,
    root: RefNo,
) -> anyhow::Result<BTreeSet<RefnoEnum>> {
    let mut response = db
        .query(pre_e3d_spatial_refnos_query(dbnum, root))
        .await?
        .check()?;
    let mut values = response.take::<Vec<Thing>>(0)?;
    values.extend(response.take::<Vec<Thing>>(1)?);
    values
        .into_iter()
        .map(crate::data_interface::helper::pe_thing_to_refno)
        .collect()
}

async fn pre_e3d_relation_count_on(
    db: &Surreal<Any>,
    dbnum: u32,
    root: RefNo,
) -> anyhow::Result<usize> {
    let mut response = db
        .query(pre_e3d_relation_count_query(dbnum, root))
        .await?
        .check()?;
    let inst = response.take::<Option<usize>>(0)?.unwrap_or_default();
    let tubi = response.take::<Option<usize>>(1)?.unwrap_or_default();
    Ok(inst + tubi)
}

/// Remove every persisted e3d-model geometry owned by one generation root.
///
/// Derived geometry (for example implied tubes) has a content-addressed
/// `inst_relate:derived_*` id, so deleting only the PE subtree's ordinary
/// `inst_relate:<refno>` rows leaves it behind.  Use the same geometry-id
/// cleanup path as regeneration so all representations are removed together.
pub async fn delete_persisted_geometry_root(root: RefNo) -> anyhow::Result<usize> {
    #[derive(serde::Deserialize)]
    struct PersistedGeometryRow {
        dbnum: u32,
        geometry_id: GeometryId,
    }

    let mut response = aios_core::SUL_DB
        .query(persisted_geometry_rows_query(root))
        .await?
        .check()?;
    let mut rows: Vec<PersistedGeometryRow> = response.take(0)?;
    rows.extend(response.take::<Vec<PersistedGeometryRow>>(1)?);
    let mut by_dbnum = BTreeMap::<u32, Vec<GeometryId>>::new();
    for row in rows {
        by_dbnum.entry(row.dbnum).or_default().push(row.geometry_id);
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

fn persisted_geometry_rows_query(root: RefNo) -> String {
    let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
    format!(
        "SELECT dbnum, direct_model.geometry_id AS geometry_id FROM inst_relate \
         WHERE direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};\
         SELECT dbnum, direct_model.geometry_id AS geometry_id FROM tubi_relate \
         WHERE direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};"
    )
}

fn existing_geometry_ids_query(dbnum: u32, root: RefNo) -> String {
    let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
    format!(
        "SELECT VALUE direct_model.geometry_id FROM inst_relate \
         WHERE dbnum={dbnum} AND direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};\
         SELECT VALUE direct_model.geometry_id FROM tubi_relate \
         WHERE dbnum={dbnum} AND direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};"
    )
}

fn pre_e3d_predicates(dbnum: u32, root: RefNo) -> (String, String) {
    let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
    let common = format!(
        "dbnum={dbnum} AND anc CONTAINS {packed} \
         AND (direct_model.source = NONE OR direct_model.source = 'legacy') \
         AND record::tb(in) = 'pe'"
    );
    let inst = format!(
        "{common} AND type::is::string(record::id(id)) \
         AND <string>record::id(id) = <string>record::id(in)"
    );
    (inst, common)
}

fn pre_e3d_relation_count_query(dbnum: u32, root: RefNo) -> String {
    let (inst, tubi) = pre_e3d_predicates(dbnum, root);
    format!(
        "RETURN array::len((SELECT VALUE id FROM inst_relate WHERE {inst}));\
         RETURN array::len((SELECT VALUE id FROM tubi_relate WHERE {tubi}));"
    )
}

fn pre_e3d_spatial_refnos_query(dbnum: u32, root: RefNo) -> String {
    let (inst, tubi) = pre_e3d_predicates(dbnum, root);
    format!(
        "SELECT VALUE in FROM inst_relate WHERE {inst};\
         SELECT VALUE in FROM tubi_relate WHERE {tubi};"
    )
}

fn pre_e3d_delete_query(dbnum: u32, root: RefNo) -> String {
    let (inst, tubi) = pre_e3d_predicates(dbnum, root);
    format!(
        "DELETE inst_relate WHERE {inst};\n\
         DELETE tubi_relate WHERE {tubi};\n"
    )
}

/// The newest e3d-model source session already published for `dbnum`, if any.
async fn persisted_e3d_session(dbnum: u32) -> anyhow::Result<Option<u32>> {
    let mut response = aios_core::SUL_DB
        .query(format!(
            "SELECT VALUE direct_model.sesno FROM inst_relate \
             WHERE dbnum={dbnum} AND direct_model.source='e3d-model' \
             AND direct_model.sesno != NONE \
             ORDER BY direct_model.sesno DESC LIMIT 1;\
             SELECT VALUE direct_model.sesno FROM tubi_relate \
             WHERE dbnum={dbnum} AND direct_model.source='e3d-model' \
             AND direct_model.sesno != NONE \
             ORDER BY direct_model.sesno DESC LIMIT 1;"
        ))
        .await?
        .check()?;
    Ok(response
        .take::<Vec<u32>>(0)?
        .into_iter()
        .chain(response.take::<Vec<u32>>(1)?)
        .max())
}

/// Full generation must never regress the published projection. With the
/// source pinned to the file's latest session this only trips when the file
/// itself was rolled back or replaced, which is a file anomaly, not a window
/// to skip.
async fn ensure_not_older_than_persisted(dbnum: u32, source_sesno: u32) -> anyhow::Result<()> {
    let persisted = persisted_e3d_session(dbnum).await?;
    anyhow::ensure!(
        session_not_stale(persisted, source_sesno),
        "stale e3d-model session {source_sesno} cannot overwrite persisted session {}",
        persisted.unwrap_or_default()
    );
    Ok(())
}

/// An explicit window target that is older than what is already published has
/// been superseded (ADR-054 constraint 5): the caller settles it as covered.
async fn persisted_session_newer_than(
    dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<Option<u32>> {
    Ok(persisted_e3d_session(dbnum)
        .await?
        .filter(|persisted| *persisted > target_sesno))
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

fn publication_transaction(statements: &str) -> String {
    format!("BEGIN TRANSACTION;\n{statements}COMMIT TRANSACTION;\n")
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
    Ok(SourceIndex { roots, owners })
}

pub fn parse_refno(value: &str) -> anyhow::Result<RefNo> {
    let normalized = value.trim().trim_start_matches('=').replace('_', "/");
    let (word0, word1) = normalized
        .split_once('/')
        .with_context(|| format!("invalid refno {value}"))?;
    Ok(RefNo::new(word0.parse()?, word1.parse()?))
}

fn geometry_source_refno(id: &GeometryId) -> anyhow::Result<RefNo> {
    match id {
        GeometryId::Element { refno } => parse_refno(refno),
        GeometryId::ImpliedTube {
            container_refno, ..
        } => parse_refno(container_refno),
    }
}

/// Current-projection AABB rows are keyed by the geometry source PE.  Collect
/// the exact keys whose committed pointers must be mirrored into the process
/// spatial tree after publication.  Historical projections deliberately do
/// not call this helper: their namespaced kv-mem rows never affect the current
/// tree or its epoch.
fn spatial_refnos_for_delta(
    upserts: &[GeneratedElement],
    removals: &[GeometryId],
) -> anyhow::Result<BTreeSet<RefnoEnum>> {
    upserts
        .iter()
        .map(|element| &element.geometry_id)
        .chain(removals.iter())
        .map(|geometry_id| {
            geometry_source_refno(geometry_id).map(|refno| {
                RefnoEnum::from(RefU64::from_two_nums(refno.word0, refno.word1))
            })
        })
        .collect()
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

fn transform_value(matrix: DMat4) -> Value {
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

fn sql_value(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    #[tokio::test]
    async fn cohort_publication_rolls_back_every_root_when_one_claim_is_stale() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem surreal");
        db.use_ns("cohort")
            .use_db("cohort")
            .await
            .expect("select fixture database");
        db.query("CREATE gen_root:a SET published_revision=0; CREATE gen_root:b SET published_revision=0;")
            .await
            .expect("seed roots")
            .check()
            .expect("valid seed");

        let result = db
            .query(publication_transaction(
                "UPDATE gen_root:a SET published_revision=1; \
                 THROW 'stale root publication revision'; \
                 UPDATE gen_root:b SET published_revision=1;",
            ))
            .await
            .and_then(|response| response.check());
        assert!(result.is_err(), "stale cohort must reject the commit");

        let mut response = db
            .query("RETURN [gen_root:a.published_revision, gen_root:b.published_revision];")
            .await
            .expect("read roots")
            .check()
            .expect("valid read");
        let revisions: Vec<u64> = response.take(0).expect("decode revisions");
        assert_eq!(revisions, vec![0, 0]);
    }

    #[test]
    fn unchanged_manifest_emits_only_revision_and_receipt_sql() {
        let root = RefNo::new(1, 2);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let claim = RootPublishClaim {
            root,
            revision: 7,
            desired_source_sesno: 42,
            model_target: None,
            published_model_target: None,
            published_manifest_hash: Some(hex::encode(Sha256::digest(empty_manifest))),
            published_geometry_count: Some(0),
            published_projection_complete: true,
        };
        let (sql, report) = prepare_geometry_delta(
            ProjectionScope::Current,
            8000,
            42,
            Vec::new(),
            Vec::new(),
            Some((root, 5)),
            Some(&claim),
            &BTreeMap::new(),
            Path::new("."),
        )
        .expect("prepare unchanged root");

        assert_eq!(report.upserted, 0);
        assert_eq!(report.removed, 0);
        assert!(!sql.contains("DELETE inst_relate"), "{sql}");
        assert!(!sql.contains("UPSERT inst_relate"), "{sql}");
        assert!(sql.contains("published_revision=7"), "{sql}");
        assert!(sql.contains("published_geometry_count=0"), "{sql}");
    }

    #[test]
    fn unchanged_manifest_repairs_an_incomplete_published_projection() {
        let root = RefNo::new(24383, 73948);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let claim = RootPublishClaim {
            root,
            revision: 7,
            desired_source_sesno: 42,
            model_target: None,
            published_model_target: None,
            published_manifest_hash: Some(hex::encode(Sha256::digest(empty_manifest))),
            published_geometry_count: Some(0),
            published_projection_complete: false,
        };
        let (sql, _) = prepare_geometry_delta(
            ProjectionScope::Current,
            8000,
            42,
            Vec::new(),
            Vec::new(),
            Some((root, 1)),
            Some(&claim),
            &BTreeMap::new(),
            Path::new("."),
        )
        .expect("prepare repair publication");
        assert!(sql.contains("DELETE inst_relate"), "{sql}");
    }

    #[test]
    fn identical_immutable_target_republishes_before_any_geometry_build() {
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
        let claim = RootPublishClaim {
            root: RefNo::new(1, 2),
            revision: 8,
            desired_source_sesno: 42,
            model_target: Some(target.clone()),
            published_model_target: Some(target),
            published_manifest_hash: Some("manifest".into()),
            published_geometry_count: Some(3),
            published_projection_complete: true,
        };
        let sql = prepare_cached_root_publication(&claim, 42).expect("cached publication SQL");
        assert!(sql.contains("published_revision=8"), "{sql}");
        assert!(sql.contains("published_manifest_hash='manifest'"), "{sql}");
        assert!(!sql.contains("DELETE inst_relate"), "{sql}");
        assert!(!sql.contains("UPSERT inst_relate"), "{sql}");
        assert!(!sql.contains("inst_geo"), "{sql}");
    }

    #[tokio::test]
    async fn cached_target_requires_every_published_mesh_file() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("cached_target")
            .use_db("cached_target")
            .await
            .expect("namespace");
        db.query(
            "CREATE inst_relate:a SET anc=[4294967298], \
             direct_model={source:'e3d-model',mesh_id:'mesh-a'};",
        )
        .await
        .expect("seed relation")
        .check()
        .expect("valid relation");
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
        let claim = RootPublishClaim {
            root: RefNo::new(1, 2),
            revision: 8,
            desired_source_sesno: 42,
            model_target: Some(target.clone()),
            published_model_target: Some(target),
            published_manifest_hash: Some("manifest".into()),
            published_geometry_count: Some(1),
            published_projection_complete: true,
        };
        let mesh_dir = std::env::temp_dir().join(format!(
            "e3d-model-cache-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&mesh_dir).unwrap();
        let mesh = mesh_dir.join("mesh-a.mesh");
        std::fs::write(&mesh, b"fixture").unwrap();
        assert!(
            cached_root_projection_complete_on(&db, &claim, 42, &mesh_dir)
                .await
                .unwrap()
        );
        std::fs::remove_file(&mesh).unwrap();
        assert!(
            !cached_root_projection_complete_on(&db, &claim, 42, &mesh_dir)
                .await
                .unwrap(),
            "missing mesh must fall back to full generation"
        );
        std::fs::remove_dir_all(mesh_dir).unwrap();
    }

    #[test]
    fn immutable_target_cache_gate_precedes_snapshot_generation() {
        let source = include_str!("e3d_model_service.rs");
        let body = source
            .split_once("async fn generate_refs(")
            .expect("generate_refs")
            .1
            .split_once("fn pin(&self")
            .expect("generate_refs end")
            .0;
        let cache_gate = body
            .find("published_root_projection_complete(")
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
    fn current_projection_spatial_targets_follow_geometry_sources_and_deduplicate() {
        let element = GeometryId::Element {
            refno: "24384/25729".into(),
        };
        let tube = GeometryId::ImpliedTube {
            container_refno: "24384/25729".into(),
            route_ordinal: 0,
            from_refno: "24384/1".into(),
            to_refno: "24384/2".into(),
        };
        let targets = spatial_refnos_for_delta(&[], &[element, tube]).expect("spatial targets");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets.iter().next().unwrap().to_pdms_str(), "24384/25729");
    }

    #[test]
    fn exact_removal_never_deletes_a_shared_mesh() {
        let mut sql = String::new();
        append_geometry_removal(&mut sql, "derived_deadbeef");
        assert!(sql.contains("inst_relate','derived_deadbeef"));
        assert!(sql.contains("inst_info','direct_derived_deadbeef"));
        assert!(!sql.contains("DELETE inst_geo"));
        assert!(!sql.contains(".mesh"));
        assert!(!sql.contains("booled_id"));
    }

    #[test]
    fn implied_tube_uses_the_legacy_tubi_relate_contract() {
        let sql = legacy_tubi_relation_sql(
            "24383_73948",
            4,
            "e3d_baked_v2_deadbeef",
            "derived_deadbeef",
            RefNo::new(24383, 10),
            RefNo::new(24383, 20),
            "[104724187652316]",
            7997,
            42,
            "{\"kind\":\"implied_tube\"}",
            "baked-v2",
            None,
            ProjectionScope::Current,
        );
        assert!(sql.contains("INSERT RELATION INTO tubi_relate"));
        assert!(sql.contains("id:tubi_relate:[pe:⟨24383_73948⟩,4]"));
        assert!(sql.contains("leave:pe:⟨24383_10⟩"));
        assert!(sql.contains("arrive:pe:⟨24383_20⟩"));
        assert!(sql.contains("out:inst_geo:⟨e3d_baked_v2_deadbeef⟩"));
        assert!(!sql.contains("UPSERT inst_relate"));
    }

    #[test]
    fn pair_local_route_ordinals_do_not_collide_in_branch_relation_slots() {
        let mut slots = BTreeMap::new();
        let branch = RefNo::new(24383, 73948);
        assert_eq!(next_tube_relation_slot(&mut slots, branch), 0);
        assert_eq!(next_tube_relation_slot(&mut slots, branch), 1);
        assert_eq!(next_tube_relation_slot(&mut slots, RefNo::new(24383, 1)), 0);
    }

    #[test]
    fn implied_tube_can_reference_the_existing_cylinder_v3_canonical_mesh() {
        let key = r#"{"family":"cylinder_v3","segments":32}"#;
        let sql = legacy_tubi_relation_sql(
            "24383_73948",
            0,
            "123456789",
            "derived_deadbeef",
            RefNo::new(24383, 10),
            RefNo::new(24383, 20),
            "[104724187652316]",
            7997,
            42,
            "{\"kind\":\"implied_tube\"}",
            "canonical-primitive-v1",
            Some(key),
            ProjectionScope::Current,
        );
        assert!(sql.contains("out:inst_geo:⟨123456789⟩"));
        assert!(sql.contains("format:'canonical-primitive-v1'"));
        assert!(sql.contains("primitive_key:{\"family\":\"cylinder_v3\",\"segments\":32}"));
        assert!(!sql.contains("format:'baked-v2'"));
    }

    #[test]
    fn implied_tube_flattens_canonical_local_transform_into_legacy_world_transform() {
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
            "ordinary element relations retain the separate local-transform branch"
        );
    }

    #[tokio::test]
    async fn legacy_tubi_sql_executes_without_touching_inst_relate() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem surreal");
        db.use_ns("legacy_layout")
            .use_db("legacy_layout")
            .await
            .expect("namespace");
        db.query(
            "CREATE pe:⟨24383_73948⟩; CREATE pe:⟨24383_10⟩; CREATE pe:⟨24383_20⟩;\
             CREATE aabb:⟨direct_world_derived_deadbeef⟩;\
             CREATE trans:⟨direct_world_derived_deadbeef⟩;",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");
        let sql = legacy_tubi_relation_sql(
            "24383_73948",
            0,
            "e3d_baked_v2_deadbeef",
            "derived_deadbeef",
            RefNo::new(24383, 10),
            RefNo::new(24383, 20),
            "[104724187652316]",
            7997,
            42,
            "{\"kind\":\"implied_tube\",\"container_refno\":\"24383/73948\",\"from_refno\":\"24383/10\",\"to_refno\":\"24383/20\"}",
            "baked-v2",
            None,
            ProjectionScope::Current,
        );
        db.query(sql)
            .await
            .expect("legacy tubi write")
            .check()
            .expect("legacy tubi statements");
        let mut response = db
            .query(
                "RETURN array::len((SELECT VALUE id FROM tubi_relate));\
                 RETURN array::len((SELECT VALUE id FROM inst_relate));",
            )
            .await
            .expect("counts")
            .check()
            .expect("count statements");
        assert_eq!(response.take::<Option<i64>>(0).unwrap(), Some(1));
        assert_eq!(response.take::<Option<i64>>(1).unwrap(), Some(0));
    }

    #[test]
    fn representation_switch_clears_the_old_branch_before_upsert() {
        let mut sql = String::new();
        append_geometry_representation_cleanup(&mut sql, "17496_152095");
        assert!(sql.contains("DELETE geo_relate"));
        assert!(sql.contains("DELETE type::thing('inst_info'"));
        assert!(sql.contains("DELETE type::thing('trans','direct_local_17496_152095')"));
        assert!(sql.contains("DELETE type::thing('inst_relate','17496_152095')"));
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
        let source = include_str!("e3d_model_service.rs");
        let body = source
            .split_once("pub async fn from_current(")
            .expect("from_current")
            .1
            .split_once("pub async fn generate_roots(")
            .expect("from_current and dbnum_for_roots end before generate_roots")
            .0;
        assert!(body.contains("current_mdb_sources()"));
        assert!(body.contains("model_source::dbnum_of_root("));
        assert!(!body.contains("dbnum_watermark"), "{body}");
        assert!(!body.contains("type::thing('pe'"), "{body}");
        assert!(!body.contains("pins_from_watermark"), "{body}");
    }

    /// ADR-054 constraint 5: an explicit window target is opened as such; the
    /// current pin is never required to equal it, and a newer published session
    /// settles the window as covered instead of failing it.
    #[test]
    fn apply_window_opens_the_explicit_target_and_yields_to_newer_publications() {
        let source = include_str!("e3d_model_service.rs");
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

    #[test]
    fn stale_geometry_scan_excludes_pre_geometry_id_rows() {
        let query = existing_geometry_ids_query(7997, RefNo::new(17496, 152094));
        assert!(query.contains("direct_model.source='e3d-model'"));
        assert!(query.contains("direct_model.geometry_id != NONE"));
        assert!(query.contains("anc CONTAINS 75144747962910"));
        assert!(query.contains("FROM inst_relate"));
        assert!(query.contains("FROM tubi_relate"));
    }

    #[test]
    fn regenerated_root_cleanup_rechecks_both_legacy_relation_tables() {
        let query = pre_e3d_relation_count_query(1112, RefNo::new(17496, 106253));
        let delete = pre_e3d_delete_query(1112, RefNo::new(17496, 106253));
        assert!(query.contains("dbnum=1112"));
        assert!(query.contains("anc CONTAINS 75144747917069"));
        assert!(query.contains("direct_model.source = NONE"));
        assert!(query.contains("direct_model.source = 'legacy'"));
        assert!(query.contains("record::tb(in) = 'pe'"));
        assert!(query.contains("FROM inst_relate"));
        assert!(query.contains("FROM tubi_relate"));
        assert!(delete.contains("DELETE inst_relate WHERE"));
        assert!(delete.contains("DELETE tubi_relate WHERE"));
    }

    #[tokio::test]
    async fn cleanup_spatial_scan_returns_only_legacy_source_pe_keys() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("pre_e3d_spatial_keys")
            .use_db("pre_e3d_spatial_keys")
            .await
            .expect("namespace");
        let root = RefNo::new(17496, 106253);
        let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
        db.query(format!(
            "CREATE pe:17496_106253; CREATE pe:17496_125194; CREATE pe:17496_125197;\
             CREATE inst_info:legacy;\
             CREATE inst_relate:17496_125194 SET in=pe:17496_125194,out=inst_info:legacy,dbnum=1112,anc=[{packed}],direct_model=NONE;\
             RELATE pe:17496_125197->tubi_relate->inst_geo:legacy SET dbnum=1112,anc=[{packed}],direct_model={{source:'e3d-model',geometry_id:{{kind:'element',refno:'17496/125197'}}}};"
        ))
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let keys = pre_e3d_spatial_refnos_on(&db, 1112, root)
            .await
            .expect("scan source keys");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.iter().next().unwrap().to_pdms_str(), "17496/125194");
    }

    #[tokio::test]
    async fn regenerated_root_removes_stale_legacy_child_but_keeps_e3d_rows() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("pre_e3d_cleanup")
            .use_db("pre_e3d_cleanup")
            .await
            .expect("namespace");
        let root = RefNo::new(17496, 106253);
        let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
        db.query(format!(
            "CREATE pe:17496_106253; CREATE pe:17496_125194; CREATE pe:17496_125197;\
             CREATE inst_info:legacy_fixing; CREATE inst_geo:legacy_tube;\
             CREATE inst_relate:17496_125194 SET in=pe:17496_125194,out=inst_info:legacy_fixing,dbnum=1112,anc=[{packed}],direct_model=NONE;\
             RELATE pe:17496_125194->tubi_relate->inst_geo:legacy_tube SET dbnum=1112,anc=[{packed}],direct_model={{source:'legacy'}};\
             CREATE inst_relate:17496_125197 SET in=pe:17496_125197,dbnum=1112,anc=[{packed}],direct_model={{source:'e3d-model',geometry_id:{{kind:'element',refno:'17496/125197'}}}};"
        ))
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let removal_count = pre_e3d_relation_count_on(&db, 1112, root)
            .await
            .expect("scan stale pre-e3d rows");
        assert_eq!(removal_count, 2);

        let report = apply_geometry_delta_on_with_pre_e3d(
            &db,
            ProjectionScope::Current,
            1112,
            1,
            Vec::new(),
            Vec::new(),
            Some((root, removal_count)),
            None,
            &BTreeMap::new(),
            Path::new("."),
        )
        .await
        .expect("remove stale pre-e3d row");
        assert_eq!(report.removed, 2);

        let mut response = db
            .query(
                "RETURN record::exists(inst_relate:17496_125194);\
                 RETURN record::exists(inst_info:legacy_fixing);\
                 RETURN record::exists(inst_relate:17496_125197);\
                 RETURN array::len((SELECT VALUE id FROM tubi_relate));",
            )
            .await
            .expect("verify")
            .check()
            .expect("verify statements");
        assert_eq!(response.take::<Option<bool>>(0).unwrap(), Some(false));
        assert_eq!(response.take::<Option<bool>>(1).unwrap(), Some(true));
        assert_eq!(response.take::<Option<bool>>(2).unwrap(), Some(true));
        assert_eq!(response.take::<Option<i64>>(3).unwrap(), Some(0));
    }

    #[tokio::test]
    async fn cleanup_scan_cannot_delete_a_row_replaced_by_e3d_before_commit() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("pre_e3d_race")
            .use_db("pre_e3d_race")
            .await
            .expect("namespace");
        let root = RefNo::new(17496, 106253);
        let packed = (((root.word0 as u64) << 32) | root.word1 as u64) as i64;
        db.query(format!(
            "CREATE pe:17496_125194;\
             CREATE inst_relate:17496_125194 SET in=pe:17496_125194,dbnum=1112,anc=[{packed}],direct_model=NONE;"
        ))
        .await
        .expect("seed")
        .check()
        .expect("seed statements");
        let observed = pre_e3d_relation_count_on(&db, 1112, root)
            .await
            .expect("initial legacy scan");
        assert_eq!(observed, 1);
        db.query(
            "UPDATE inst_relate:17496_125194 SET direct_model={source:'e3d-model',geometry_id:{kind:'element',refno:'17496/125194'}};",
        )
        .await
        .expect("replace")
        .check()
        .expect("replace row");

        apply_geometry_delta_on_with_pre_e3d(
            &db,
            ProjectionScope::Current,
            1112,
            1,
            Vec::new(),
            Vec::new(),
            Some((root, observed)),
            None,
            &BTreeMap::new(),
            Path::new("."),
        )
        .await
        .expect("transactional predicate recheck");
        let mut response = db
            .query("RETURN record::exists(inst_relate:17496_125194);")
            .await
            .expect("verify")
            .check()
            .expect("verify statements");
        assert_eq!(response.take::<Option<bool>>(0).unwrap(), Some(true));
    }

    #[test]
    fn root_delete_scan_includes_derived_e3d_geometry() {
        let query = persisted_geometry_rows_query(RefNo::new(24383, 73948));
        assert!(query.contains("direct_model.source='e3d-model'"));
        assert!(query.contains("direct_model.geometry_id != NONE"));
        assert!(query.contains("anc CONTAINS 104724187652316"));
        assert!(query.contains("FROM inst_relate"));
        assert!(query.contains("FROM tubi_relate"));
        assert!(!query.contains("inst_relate:24383_73948"));
    }

    #[tokio::test]
    async fn publication_cas_rejects_an_old_revision_and_a_mixed_target() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("publish_cas")
            .use_db("publish_cas")
            .await
            .expect("namespace");
        let root = RefNo::new(24383, 73948);
        db.query(
            "CREATE gen_root:24383_73948 SET desired_revision=1, \
             desired_target={source_end_sesno:43}, publication_status='pending';",
        )
        .await
        .expect("seed root")
        .check()
        .expect("seed root statements");

        let claim = RootPublishClaim {
            root,
            revision: 1,
            desired_source_sesno: 43,
            model_target: None,
            published_model_target: None,
            published_manifest_hash: None,
            published_geometry_count: None,
            published_projection_complete: false,
        };
        apply_geometry_delta_on_with_pre_e3d(
            &db,
            ProjectionScope::Current,
            8191,
            43,
            Vec::new(),
            Vec::new(),
            None,
            Some(&claim),
            &BTreeMap::new(),
            Path::new("."),
        )
        .await
        .expect("current claim publishes even when manifest is unchanged");

        db.query(
            "UPDATE gen_root:24383_73948 SET desired_revision=2, \
             desired_target={source_end_sesno:44}, publication_status='stale';",
        )
        .await
        .expect("advance target")
        .check()
        .expect("advance target statements");
        let stale = apply_geometry_delta_on_with_pre_e3d(
            &db,
            ProjectionScope::Current,
            8191,
            43,
            Vec::new(),
            Vec::new(),
            None,
            Some(&claim),
            &BTreeMap::new(),
            Path::new("."),
        )
        .await;
        assert!(stale.is_err(), "old worker must lose the publication CAS");

        let mixed_claim = RootPublishClaim {
            root,
            revision: 2,
            desired_source_sesno: 44,
            model_target: None,
            published_model_target: None,
            published_manifest_hash: None,
            published_geometry_count: None,
            published_projection_complete: false,
        };
        let mixed = apply_geometry_delta_on_with_pre_e3d(
            &db,
            ProjectionScope::Current,
            8191,
            43,
            Vec::new(),
            Vec::new(),
            None,
            Some(&mixed_claim),
            &BTreeMap::new(),
            Path::new("."),
        )
        .await;
        assert!(
            mixed.is_err(),
            "a source session cannot publish into another target"
        );

        #[derive(Deserialize)]
        struct RootRow {
            published_revision: u64,
            desired_revision: u64,
            publication_status: String,
        }
        let mut response = db
            .query("SELECT published_revision, desired_revision, publication_status FROM ONLY gen_root:24383_73948;")
            .await
            .expect("read root")
            .check()
            .expect("read root statements");
        let row: Option<RootRow> = response.take(0).expect("decode root");
        let row = row.expect("root exists");
        assert_eq!((row.published_revision, row.desired_revision), (1, 2));
        assert_eq!(row.publication_status, "stale");
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
    fn catalogue_resolver_uses_the_project_selected_same_dbnum_file() {
        let selected = PathBuf::from(r"D:\selected\ZDJ\zdj7600_0001");
        let locator = InMemoryCataLocator::from_parts(
            HashMap::from([(23984, 7600)]),
            HashMap::from([(7600, ("CATA".into(), "ZDJ".into(), selected.clone()))]),
        );
        let resolver = E3dDbResolver {
            locator,
            template_dir: PathBuf::from(r"D:\templates"),
            sessions: Arc::new(std::sync::Mutex::new(BTreeMap::from([(7600, 42)]))),
        };
        let pin = resolver.resolve(7600).expect("selected CATA pin");
        assert_eq!(pin.file, selected);
        assert_eq!(pin.sesno, Some(42));
        assert_eq!(pin.db_type.as_deref(), Some("CATA"));
        assert!(pin.template.ends_with("catvir.dat"));
    }
}
