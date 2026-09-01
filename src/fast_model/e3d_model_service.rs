//! 生产环境唯一的 `e3d-model` 生成、增量和 Plant UI 兼容持久化入口。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{bail, Context};
use e3d_io::db_element::{template_file_for, DbFilePin, DbFileResolver, DbSet};
use e3d_io::engine::{ReadOnlyEngine, ScanTier};
use e3d_io::refno::RefNo;
use e3d_model::catalogue::CatalogueMeshCache;
use e3d_model::elmodl::{GeneratedElement, GeometryId};
use e3d_model::increment::{collect_window, increment_update};
use e3d_model::pipeline::{generate_subtree_with_cache, Report};
use e3d_model::primitive_instance::canonical_primitive_mesh;
use e3d_model::transform::dmat4_to_affine4x3;
use glam::DMat4;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::data_interface::cata_closure::{CataDbLocator, InMemoryCataLocator};
use crate::data_interface::direct_store::{pins_from_watermark, DbPin, DirectSchema};
use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::fast_model::e3d_mesh_store::{
    baked_mesh_id, canonical_mesh_id, ensure_mesh_file, geometry_record_id, E3dPersistReport,
    MeshWrite,
};

#[derive(Debug)]
struct SourceIndex {
    roots: Vec<RefNo>,
    owners: BTreeMap<(u32, u32), RefNo>,
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
    mesh_dir: PathBuf,
}

#[derive(Clone)]
struct E3dDbResolver {
    locator: InMemoryCataLocator,
    template_dir: PathBuf,
}

impl DbFileResolver for E3dDbResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        let db_type = self.locator.db_type_of(dbno)?;
        let (_project, file) = self.locator.file_of(dbno)?;
        let template = template_file_for(&self.template_dir, &db_type).ok()?;
        Some(DbFilePin {
            file,
            template,
            db_type: Some(db_type),
            sesno: None,
        })
    }
}

impl E3dModelService {
    pub async fn from_current() -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        Ok(Self {
            schema: DirectSchema::open_from_env()?,
            pins: pins_from_watermark().await?,
            locator: InMemoryCataLocator::build_for_project(&option.project_name).await?,
            mesh_dir: option.get_meshes_path(),
        })
    }

    pub async fn dbnum_for_roots(roots: &[String]) -> anyhow::Result<u32> {
        let root = roots.first().context("generation roots are empty")?;
        let refno = parse_refno(root)?;
        let mut response = aios_core::SUL_DB
            .query(dbnum_lookup_query(refno))
            .await?
            .check()?;
        response
            .take::<Vec<u32>>(0)?
            .into_iter()
            .next()
            .filter(|dbnum| *dbnum > 0)
            .with_context(|| format!("root {root} has no persisted dbnum"))
    }

    pub async fn generate_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        let source_sesno = pin.sesno.context("direct model pin has no fixed session")?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let index = scan_index(&pin.file, pin.sesno)?;
        let set = self.build_set(dbnum, pin.sesno)?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.generate_refs(dbnum, source_sesno, &set, &index, &roots, failure_policy)
            .await
    }

    pub async fn generate_dbnum(
        &self,
        dbnum: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        let source_sesno = pin.sesno.context("direct model pin has no fixed session")?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let index = scan_index(&pin.file, pin.sesno)?;
        let set = self.build_set(dbnum, pin.sesno)?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &set,
            &index,
            &index.roots,
            failure_policy,
        )
        .await
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
        anyhow::ensure!(
            pin.sesno == Some(target_sesno),
            "increment target session {target_sesno} is not the current direct pin {:?}",
            pin.sesno
        );
        ensure_not_older_than_persisted(dbnum, target_sesno).await?;
        let window = collect_window(&pin.file, base_sesno, target_sesno)?;
        let base = self.build_set(dbnum, Some(base_sesno))?;
        let target = self.build_set(dbnum, Some(target_sesno))?;
        let outcome = increment_update(&base, &target, &window);
        let failed = outcome.report.regen_failed.len()
            + outcome.report.derived_failed.len()
            + outcome.report.unresolved.len()
            + outcome.report.derived_stale_unreadable.len();
        if failed > 0 && matches!(failure_policy, GeometryFailurePolicy::Required) {
            bail!("e3d-model incremental generation failed for {failed} element(s)");
        }
        let index = scan_index(&pin.file, Some(target_sesno))?;
        ensure_not_older_than_persisted(dbnum, target_sesno).await?;
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
        report.generation_report = serde_json::to_value(outcome.report)?;
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
    ) -> anyhow::Result<E3dPersistReport> {
        let mut total = E3dPersistReport::default();
        let mut generation = Report::default();
        // A production batch is one reuse window: identical evaluated catalogue
        // primitives across different delivery roots must share their local mesh.
        let mut catalogue_mesh_cache = CatalogueMeshCache::default();
        for &root in roots {
            let outcome =
                match generate_subtree_with_cache(set.element(root), &mut catalogue_mesh_cache) {
                    Ok(outcome) => outcome,
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        let stale = existing_geometry_ids(dbnum, root).await?;
                        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
                        let cleared = apply_geometry_delta(
                            dbnum,
                            source_sesno,
                            Vec::new(),
                            stale,
                            &index.owners,
                            &self.mesh_dir,
                        )
                        .await?;
                        total.merge_counts(cleared);
                        total.failed += 1;
                        log::error!("e3d-model root {root} failed: {error:#}");
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| format!("generate root {root}"));
                    }
                };
            let root_failed = outcome.report.failed.len();
            if root_failed > 0 && matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model root {root} has {root_failed} failed element(s)");
            }
            let root_skipped = outcome.report.skipped.len();
            let generated_ids = outcome
                .elements
                .iter()
                .map(|element| {
                    Ok((
                        serde_json::to_string(&element.geometry_id)?,
                        element.geometry_id.clone(),
                    ))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            let removals = existing_geometry_ids(dbnum, root)
                .await?
                .into_iter()
                .filter_map(|geometry_id| {
                    let key = serde_json::to_string(&geometry_id).ok()?;
                    (!generated_ids.contains_key(&key)).then_some(geometry_id)
                })
                .collect();
            generation.merge(outcome.report);
            ensure_not_older_than_persisted(dbnum, source_sesno).await?;
            let mut persisted = apply_geometry_delta(
                dbnum,
                source_sesno,
                outcome.elements,
                removals,
                &index.owners,
                &self.mesh_dir,
            )
            .await?;
            persisted.skipped += root_skipped;
            persisted.failed += root_failed;
            total.merge_counts(persisted);
        }
        total.generation_report = serde_json::to_value(generation)?;
        Ok(total)
    }

    fn pin(&self, dbnum: u32) -> anyhow::Result<&DbPin> {
        self.pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .with_context(|| format!("dbnum_watermark has no file pin for dbnum {dbnum}"))
    }

    fn build_set(
        &self,
        target_dbnum: u32,
        target_sesno: Option<u32>,
    ) -> anyhow::Result<Arc<DbSet>> {
        let set = Arc::new(DbSet::with_attlib_file_and_resolver(
            self.schema.template_dir().join("attlib.dat"),
            Box::new(E3dDbResolver {
                locator: self.locator.clone(),
                template_dir: self.schema.template_dir().to_path_buf(),
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

fn dbnum_lookup_query(refno: RefNo) -> String {
    let id = format!("{}_{}", refno.word0, refno.word1);
    format!("SELECT VALUE dbnum FROM type::thing('pe','{id}') LIMIT 1;")
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
    let mut report = E3dPersistReport::default();
    let mut statements = String::from("BEGIN TRANSACTION;\n");
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
        statements.push_str(&format!("DELETE pe:⟨{}_{}⟩->tubi_relate;\n", word0, word1));
    }

    for geometry_id in &removals {
        let source = geometry_source_refno(&geometry_id)?;
        let id = geometry_record_id(&geometry_id, source);
        append_geometry_removal(&mut statements, &id);
        report.removed += 1;
    }

    let mut tube_indices = BTreeMap::<(u32, u32), usize>::new();
    for element in upserts {
        let source_refno = geometry_source_refno(&element.geometry_id)?;
        let source_id = format!("{}_{}", source_refno.word0, source_refno.word1);
        let id = geometry_record_id(&element.geometry_id, source_refno);
        let geometry_json = sql_value(&serde_json::to_value(&element.geometry_id)?);
        let anc = ancestor_chain(source_refno, owners)?;
        let world_mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&element.solid);
        let world_aabb = world_mesh.aabb.context("world PlantMesh missing AABB")?;
        let world_bounds = [point3(&world_aabb.mins), point3(&world_aabb.maxs)];
        let world_transform = transform_value(element.world);

        append_geometry_representation_cleanup(&mut statements, &id);
        statements.push_str(&format!(
            "UPSERT type::thing('aabb','direct_world_{id}') CONTENT {{d:{{mins:{},maxs:{}}}}};\n\
             UPSERT type::thing('trans','direct_world_{id}') CONTENT {{d:{}}};\n",
            sql_value(&json!(world_bounds[0])),
            sql_value(&json!(world_bounds[1])),
            sql_value(&world_transform),
        ));

        if let GeometryId::ImpliedTube {
            from_refno,
            to_refno,
            ..
        } = &element.geometry_id
        {
            let from = parse_refno(from_refno)?;
            let to = parse_refno(to_refno)?;
            let local = element
                .solid
                .transform(&dmat4_to_affine4x3(element.world.inverse()));
            let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&local);
            let mesh_id = baked_mesh_id(&mesh);
            note_mesh_write(
                &mut report,
                ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?,
            );
            report.mesh_ids.insert(mesh_id.clone());
            let index = tube_indices
                .entry((source_refno.word0, source_refno.word1))
                .and_modify(|index| *index += 1)
                .or_insert(0);
            statements.push_str(&legacy_tubi_relation_sql(
                &source_id,
                *index,
                &mesh_id,
                &id,
                from,
                to,
                &sql_value(&json!(anc)),
                dbnum,
                source_sesno,
                &geometry_json,
            ));
            report.baked_instances += 1;
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
            statements.push_str(&format!(
                "UPSERT type::thing('aabb','direct_shared_{mesh_id}') CONTENT {{d:{{mins:{},maxs:{}}}}};\n\
                 UPSERT type::thing('trans','direct_local_{id}') CONTENT {{d:{}}};\n\
                 UPSERT inst_geo:⟨{mesh_id}⟩ SET meshed=true,visible=true,bad=false,aabb=type::thing('aabb','direct_shared_{mesh_id}'),direct_model={{source:'e3d-model',format:'canonical-primitive-v1'}};\n\
                 UPSERT type::thing('inst_info','direct_{id}') SET dbnum={dbnum},noun='{}',direct_model={{source:'e3d-model',sesno:{source_sesno},geometry_id:{geometry_json}}};\n\
                 RELATE inst_info:⟨direct_{id}⟩->geo_relate->inst_geo:⟨{mesh_id}⟩ SET geom_refno=type::thing('pe','{source_id}'),trans=type::thing('trans','direct_local_{id}'),visible=true,geo_type='Pos';\n\
                 UPSERT type::thing('inst_relate','{id}') SET in=type::thing('pe','{source_id}'),out=type::thing('inst_info','direct_{id}'),booled_id=NONE,booled=false,bad_bool=false,solid=true,generic='{}',dbnum={dbnum},anc={},aabb=type::thing('aabb','direct_world_{id}'),world_trans=type::thing('trans','direct_world_{id}'),insts_flat=[{{geo_hash:'{mesh_id}',transform:{}}}],direct_model={{source:'e3d-model',format:'canonical-primitive-v1',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}',primitive_key:{key}}};\n",
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
            let local = element
                .solid
                .transform(&dmat4_to_affine4x3(element.world.inverse()));
            let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&local);
            let mesh_id = baked_mesh_id(&mesh);
            note_mesh_write(
                &mut report,
                ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?,
            );
            report.mesh_ids.insert(mesh_id.clone());
            statements.push_str(&format!(
                "UPSERT type::thing('inst_relate','{id}') SET in=type::thing('pe','{source_id}'),booled_id='{mesh_id}',booled=true,bad_bool=false,solid=true,generic='{}',dbnum={dbnum},anc={},aabb=type::thing('aabb','direct_world_{id}'),world_trans=type::thing('trans','direct_world_{id}'),insts_flat=[{{geo_hash:'{mesh_id}'}}],direct_model={{source:'e3d-model',format:'baked-v2',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}'}};\n",
                element.noun,
                sql_value(&json!(anc)),
            ));
            report.baked_instances += 1;
        }
        report.upserted += 1;
    }
    statements.push_str("COMMIT TRANSACTION;\n");
    if report.upserted > 0 || report.removed > 0 {
        aios_core::SUL_DB.query(statements).await?.check()?;
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok(report)
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
) -> String {
    format!(
        "UPSERT inst_geo:⟨{mesh_id}⟩ SET meshed=true,visible=true,bad=false,direct_model={{source:'e3d-model',format:'baked-v2'}};\n\
         INSERT RELATION INTO tubi_relate [{{id:tubi_relate:[pe:⟨{source_id}⟩,{index}],in:pe:⟨{source_id}⟩,out:inst_geo:⟨{mesh_id}⟩,leave:pe:⟨{}_{}⟩,arrive:pe:⟨{}_{}⟩,aabb:aabb:⟨direct_world_{representation_id}⟩,world_trans:trans:⟨direct_world_{representation_id}⟩,bore_size:0,invalid:false,anc:{anc_literal},dbnum:{dbnum},direct_model:{{source:'e3d-model',format:'baked-v2',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:'{mesh_id}'}}}}];\n",
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

async fn existing_geometry_ids(dbnum: u32, root: RefNo) -> anyhow::Result<Vec<GeometryId>> {
    let query = existing_geometry_ids_query(dbnum, root);
    let mut response = aios_core::SUL_DB.query(query).await?.check()?;
    let mut rows: Vec<GeometryId> = response.take(0)?;
    rows.extend(response.take::<Vec<GeometryId>>(1)?);
    Ok(rows)
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

async fn ensure_not_older_than_persisted(dbnum: u32, source_sesno: u32) -> anyhow::Result<()> {
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
    let persisted = response
        .take::<Vec<u32>>(0)?
        .into_iter()
        .chain(response.take::<Vec<u32>>(1)?)
        .max();
    anyhow::ensure!(
        session_not_stale(persisted, source_sesno),
        "stale e3d-model session {source_sesno} cannot overwrite persisted session {}",
        persisted.unwrap_or_default()
    );
    Ok(())
}

fn session_not_stale(persisted: Option<u32>, source_sesno: u32) -> bool {
    persisted.is_none_or(|sesno| source_sesno >= sesno)
}

fn note_mesh_write(report: &mut E3dPersistReport, outcome: MeshWrite) {
    match outcome {
        MeshWrite::Written => report.mesh_written += 1,
        MeshWrite::Reused => report.mesh_reused += 1,
    }
}

fn scan_index(path: &Path, sesno: Option<u32>) -> anyhow::Result<SourceIndex> {
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

fn parse_refno(value: &str) -> anyhow::Result<RefNo> {
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
    fn element_and_implied_tubes_have_disjoint_persistence_ids() {
        let source = RefNo::new(17496, 152095);
        let element = GeometryId::Element {
            refno: source.to_string(),
        };
        let tube_a = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            from_refno: "17496/1".into(),
            to_refno: "17496/2".into(),
        };
        let tube_b = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
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
        );
        assert!(sql.contains("INSERT RELATION INTO tubi_relate"));
        assert!(sql.contains("id:tubi_relate:[pe:⟨24383_73948⟩,4]"));
        assert!(sql.contains("leave:pe:⟨24383_10⟩"));
        assert!(sql.contains("arrive:pe:⟨24383_20⟩"));
        assert!(sql.contains("out:inst_geo:⟨e3d_baked_v2_deadbeef⟩"));
        assert!(!sql.contains("UPSERT inst_relate"));
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

    #[test]
    fn generation_root_dbnum_is_read_from_the_pe_not_a_mesh_relation() {
        assert_eq!(
            dbnum_lookup_query(RefNo::new(17496, 152094)),
            "SELECT VALUE dbnum FROM type::thing('pe','17496_152094') LIMIT 1;"
        );
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
    fn root_delete_scan_includes_derived_e3d_geometry() {
        let query = persisted_geometry_rows_query(RefNo::new(24383, 73948));
        assert!(query.contains("direct_model.source='e3d-model'"));
        assert!(query.contains("direct_model.geometry_id != NONE"));
        assert!(query.contains("anc CONTAINS 104724187652316"));
        assert!(query.contains("FROM inst_relate"));
        assert!(query.contains("FROM tubi_relate"));
        assert!(!query.contains("inst_relate:24383_73948"));
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
        };
        let pin = resolver.resolve(7600).expect("selected CATA pin");
        assert_eq!(pin.file, selected);
        assert_eq!(pin.db_type.as_deref(), Some("CATA"));
        assert!(pin.template.ends_with("catvir.dat"));
    }
}
