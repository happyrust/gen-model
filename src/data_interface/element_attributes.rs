//! Complete element attributes read directly from E3D database files.
//!
//! This module is deliberately the public boundary: HTTP callers do not see a
//! `DirectStore`, descriptor layouts, or the backup database.  A file snapshot
//! is rendered first; DESI/read-through snapshots are then offered to a bounded
//! best-effort queue.  Queue or database failure never changes the file result.

use std::collections::HashSet;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::record::descriptor::{AttributeExtractionStatus, DescriptorValue, ExtractedAttribute};
use e3d_io::record::explicit::{ExplicitAttribute, ExplicitValue};
use e3d_io::uda_catalog::UdaCatalog;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock, mpsc};

use super::cata_closure::InMemoryCataLocator;
use super::direct_store::{DirectElementSnapshot, DirectSchema, DirectStore, DirectStoreError};
use super::e3d_collector::open_pinned_set;
use super::mdb_membership::MdbMembership;
use super::mirror_spool::MirrorSpool;
use super::read_through::{
    ReadThroughSource, render_rows_with_catalog, rendered_transaction_batches,
    write_through_on_with_catalog_and_prerequisites,
};
use super::uda_keys::UdaKeyMap;

const BACKUP_QUEUE_CAPACITY: usize = 256;

type EnsureResult = Result<EnsureReceipt, String>;

struct EnsureFlight {
    result: Mutex<Option<EnsureResult>>,
    notify: tokio::sync::Notify,
}

fn ensure_flights() -> &'static dashmap::DashMap<String, Arc<EnsureFlight>> {
    static FLIGHTS: std::sync::OnceLock<dashmap::DashMap<String, Arc<EnsureFlight>>> =
        std::sync::OnceLock::new();
    FLIGHTS.get_or_init(dashmap::DashMap::new)
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributeValue {
    pub name: String,
    pub value_type: String,
    pub value: Value,
    pub display: String,
    pub origin: &'static str,
    pub is_uda: bool,
    pub is_unset: bool,
    pub editable: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AttributeDiagnostics {
    pub outside_schema: Vec<String>,
    pub shape_conflicts: Vec<Value>,
    pub undecoded: Vec<Value>,
    pub uda_issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementAttributesResponse {
    pub source: &'static str,
    pub refno: String,
    pub dbnum: i32,
    pub db_type: String,
    pub sesno: u32,
    pub noun: String,
    pub complete: bool,
    pub attributes: Vec<AttributeValue>,
    pub diagnostics: AttributeDiagnostics,
    pub backup: BackupStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EnsureReceipt {
    pub status: String,
    pub requested: usize,
    pub written: usize,
    pub skipped_stale: usize,
    pub missing: Vec<String>,
    pub sesno: u32,
    pub cache_epoch: u64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReadThroughHealth {
    pub queue_depth: usize,
    pub queued: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub degraded_reason: Option<String>,
    pub last_success_unix_ms: Option<u64>,
    pub spool_depth: usize,
    pub spool_bytes: u64,
    pub spool_oldest_age_ms: Option<u64>,
    pub spool_capacity_bytes: u64,
}

#[derive(Default)]
struct QueueStats {
    depth: AtomicUsize,
    queued: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    last_success_ms: AtomicU64,
    degraded: std::sync::RwLock<Option<String>>,
}

impl QueueStats {
    fn snapshot(&self) -> ReadThroughHealth {
        let last = self.last_success_ms.load(Ordering::Relaxed);
        ReadThroughHealth {
            queue_depth: self.depth.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            degraded_reason: self.degraded.read().ok().and_then(|value| value.clone()),
            last_success_unix_ms: (last != 0).then_some(last),
            ..ReadThroughHealth::default()
        }
    }
}

pub struct ElementAttributeService {
    project: String,
    mdb: String,
    store: Arc<DirectStore>,
    catalog: Arc<UdaCatalog>,
    membership: Arc<MdbMembership>,
    backup_tx: mpsc::Sender<()>,
    spool: Arc<MirrorSpool>,
    stats: Arc<QueueStats>,
    file_fingerprint: u64,
    skeleton_fingerprint: Vec<(u32, std::path::PathBuf, u32)>,
    snapshot_epoch: u64,
}

impl ElementAttributeService {
    fn open(project: &str, mdb: &str) -> anyhow::Result<(Self, mpsc::Receiver<()>)> {
        let option = aios_core::get_db_option();
        let skeleton = super::mdb_memory_store::ensure(project, mdb)?;
        let membership = skeleton.membership();
        let (affiliations, files) = skeleton.locator_parts();
        let mut locator = InMemoryCataLocator::from_parts(affiliations, files);
        // DESI affiliation comes from the already-prewarmed MDB skeleton.  A cold
        // read-through store has no watermark rows, so rebuilding this locator from
        // the database (or a best-effort directory walk) would make valid tree nodes
        // unreadable by the attribute service.  Catalogue files are then merged from
        // every configured dependency project; merge_cata_files deliberately ignores
        // their DESI/DICT files.
        for dependency_project in &option.included_projects {
            let Some(project_root) =
                super::project_paths::resolve_project_root(option, dependency_project)
            else {
                continue;
            };
            locator.merge_cata_files(InMemoryCataLocator::build_from_dir(
                dependency_project,
                &project_root,
            ));
        }
        anyhow::ensure!(
            locator.ref0_count() > 0,
            "no E3D element index could be built for MDB {mdb}"
        );
        let schema = Arc::new(DirectSchema::open_from_env()?);
        let catalog = skeleton.uda_catalog();
        let file_fingerprint = membership_fingerprint(&membership)?;
        let skeleton_fingerprint = skeleton.session_fingerprint();
        let store = Arc::new(DirectStore::new(schema, Arc::new(locator)));
        let spool = Arc::new(MirrorSpool::for_process(&option.surreal_ns, project, mdb)?);
        let (backup_tx, backup_rx) = mpsc::channel(BACKUP_QUEUE_CAPACITY);
        Ok((
            Self {
                project: project.to_owned(),
                mdb: format!("/{}", mdb.trim_start_matches('/')),
                store,
                catalog,
                membership,
                backup_tx,
                spool,
                stats: Arc::new(QueueStats::default()),
                file_fingerprint,
                skeleton_fingerprint,
                snapshot_epoch: skeleton.epoch(),
            },
            backup_rx,
        ))
    }

    pub fn health(&self) -> ReadThroughHealth {
        let mut health = self.stats.snapshot();
        match self.spool.health() {
            Ok(spool) => {
                health.spool_depth = spool.depth;
                health.spool_bytes = spool.bytes;
                health.spool_oldest_age_ms = spool.oldest_age_ms;
                health.spool_capacity_bytes = spool.capacity_bytes;
                health.queue_depth = spool.depth;
            }
            Err(error) => {
                health.degraded_reason = Some(format!("mirror spool unreadable: {error:#}"))
            }
        }
        health
    }

    fn is_current(&self) -> anyhow::Result<bool> {
        if super::mdb_memory_store::stale_reason(&self.project, &self.mdb).is_some() {
            return Ok(false);
        }
        let skeleton_current = super::mdb_memory_store::get(&self.project, &self.mdb)
            .is_some_and(|store| store.session_fingerprint() == self.skeleton_fingerprint);
        Ok(skeleton_current && membership_fingerprint(&self.membership)? == self.file_fingerprint)
    }

    pub fn read(&self, refno: RefU64) -> anyhow::Result<ElementAttributesResponse> {
        let mut snapshot = self.store.element_snapshot(refno)?;
        let mut diagnostics = AttributeDiagnostics {
            uda_issues: self
                .catalog
                .issues()
                .iter()
                .map(|issue| format!("{}: {}", issue.refno, issue.detail))
                .collect(),
            ..AttributeDiagnostics::default()
        };
        if let Ok(projected) = super::direct_attmap::to_named_attmap(&snapshot.extraction) {
            diagnostics.outside_schema = projected.outside_schema;
            diagnostics.shape_conflicts = projected
                .shape_conflicts
                .into_iter()
                .chain(projected.view_divergence)
                .map(|conflict| {
                    json!({
                        "name": conflict.name,
                        "found": conflict.found,
                        "declared": conflict.declared,
                    })
                })
                .collect();
            diagnostics.undecoded = projected
                .undecoded
                .into_iter()
                .map(|item| json!({ "name": item.name, "reason": item.reason }))
                .collect();
        }
        let known_uda_keys = self
            .catalog
            .definitions()
            .iter()
            .flat_map(|definition| [Some(definition.key), definition.old_key])
            .flatten()
            .collect::<HashSet<_>>();
        self.catalog
            .materialize_for_element(&mut snapshot.extraction, &snapshot.explicit_attributes);

        let noun = snapshot
            .extraction
            .noun_name
            .clone()
            .unwrap_or_else(|| format!("NOUN:{:08x}", snapshot.extraction.noun_hash));
        let mut attributes = vec![
            identity_attribute("REFNO", json!(pdms(snapshot.extraction.refno))),
            identity_attribute("OWNER", json!(pdms(snapshot.extraction.owner))),
            identity_attribute("TYPE", json!(noun.clone())),
        ];
        for attribute in &snapshot.extraction.attributes {
            match render_attribute(attribute) {
                Some(value) => attributes.push(value),
                None => diagnostics.undecoded.push(json!({
                    "name": attribute.name,
                    "reason": format!("{:?}", attribute.status),
                })),
            }
        }
        for explicit in snapshot
            .explicit_attributes
            .iter()
            .filter(|attribute| attribute.is_uda && !known_uda_keys.contains(&attribute.hash))
        {
            let name = format!("UDA:{:08x}", explicit.hash);
            let (value_type, value, display, editable) = explicit_json(&explicit.value);
            attributes.push(AttributeValue {
                name: name.clone(),
                value_type,
                value,
                display,
                origin: "uda-stored",
                is_uda: true,
                is_unset: false,
                editable,
            });
            diagnostics
                .uda_issues
                .push(format!("{name}: no DICT definition (value preserved)"));
        }
        attributes.sort_by(|left, right| left.name.cmp(&right.name));
        attributes.dedup_by(|left, right| left.name == right.name);

        let backup = self.offer_backup(&snapshot);
        let complete = diagnostics.outside_schema.is_empty()
            && diagnostics.shape_conflicts.is_empty()
            && diagnostics.undecoded.is_empty()
            && diagnostics.uda_issues.is_empty();
        Ok(ElementAttributesResponse {
            source: "e3d-io",
            refno: pdms(snapshot.extraction.refno),
            dbnum: snapshot.dbnum,
            db_type: snapshot.db_type,
            sesno: snapshot.sesno,
            noun,
            complete,
            attributes,
            diagnostics,
            backup: BackupStatus { status: backup },
        })
    }

    /// Persist an explicit element scope.  Discovery is entirely file-side;
    /// writes are grouped by their frozen `(dbnum, sesno)` source so an owner
    /// in another DESI database cannot be written with the root database pin.
    pub async fn ensure(&self, root: RefU64, depth: usize) -> anyhow::Result<EnsureReceipt> {
        anyhow::ensure!(
            crate::options::data_face().is_read_through(),
            "DATA_FACE_INGEST_OWNS_CACHE"
        );
        let snapshot = self.store.element_snapshot(root)?;
        anyhow::ensure!(
            !snapshot.db_type.eq_ignore_ascii_case("CATA"),
            "CATALOGUE_NOT_PERSISTED"
        );
        let key = format!("{}:{}:{}:{depth}", snapshot.dbnum, root, snapshot.sesno);
        let (flight, owner) = match ensure_flights().entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => (entry.get().clone(), false),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let flight = Arc::new(EnsureFlight {
                    result: Mutex::new(None),
                    notify: tokio::sync::Notify::new(),
                });
                entry.insert(flight.clone());
                (flight, true)
            }
        };
        if owner {
            let result = self
                .ensure_uncollapsed(root, depth)
                .await
                .map_err(|error| format!("{error:#}"));
            *flight.result.lock().await = Some(result.clone());
            flight.notify.notify_waiters();
            ensure_flights().remove(&key);
            return result.map_err(anyhow::Error::msg);
        }
        loop {
            let notified = flight.notify.notified();
            if let Some(result) = flight.result.lock().await.clone() {
                return result.map_err(anyhow::Error::msg);
            }
            notified.await;
        }
    }

    async fn ensure_uncollapsed(
        &self,
        root: RefU64,
        depth: usize,
    ) -> anyhow::Result<EnsureReceipt> {
        anyhow::ensure!(
            crate::options::data_face().is_read_through(),
            "DATA_FACE_INGEST_OWNS_CACHE"
        );
        let started = Instant::now();
        let root_snapshot = self.store.element_snapshot(root)?;
        if !crate::options::store_mode().is_durable() {
            return Ok(EnsureReceipt {
                status: "skipped-memory".into(),
                requested: 1,
                sesno: root_snapshot.sesno,
                elapsed_ms: started.elapsed().as_millis(),
                ..EnsureReceipt::default()
            });
        }
        anyhow::ensure!(
            !root_snapshot.db_type.eq_ignore_ascii_case("CATA"),
            "CATALOGUE_NOT_PERSISTED"
        );

        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        push_limited(&mut ordered, &mut seen, root)?;

        let mut owner = root_snapshot.extraction.owner;
        while owner.is_valid() {
            let value = RefU64::from_two_nums(owner.word0, owner.word1);
            if !seen.insert(value) {
                break;
            }
            if ordered.len() >= 5_000 {
                anyhow::bail!("ENSURE_SCOPE_TOO_LARGE: scope exceeds 5000 elements");
            }
            ordered.push(value);
            owner = self.store.element_snapshot(value)?.extraction.owner;
        }

        let mut queue = VecDeque::from([(root, 0usize)]);
        while let Some((parent, level)) = queue.pop_front() {
            if level >= depth {
                continue;
            }
            for child in self.store.members(parent)? {
                if seen.contains(&child) {
                    continue;
                }
                if ordered.len() >= 5_000 {
                    anyhow::bail!("ENSURE_SCOPE_TOO_LARGE: scope exceeds 5000 elements");
                }
                seen.insert(child);
                ordered.push(child);
                queue.push_back((child, level + 1));
            }
        }

        let mut groups: BTreeMap<
            (i32, String, PathBuf, u32),
            (
                Vec<e3d_io::refno::RefNo>,
                Vec<e3d_io::uda_catalog::UdaDefinition>,
            ),
        > = BTreeMap::new();
        for refno in &ordered {
            let snapshot = self.store.element_snapshot(*refno)?;
            if snapshot.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            let entry = groups
                .entry((
                    snapshot.dbnum,
                    snapshot.db_type,
                    snapshot.file,
                    snapshot.sesno,
                ))
                .or_default();
            entry.0.push(snapshot.extraction.refno);
            entry.1.extend(
                snapshot
                    .explicit_attributes
                    .iter()
                    .filter(|attribute| attribute.is_uda)
                    .filter_map(|attribute| self.catalog.by_key(attribute.hash).cloned()),
            );
        }

        let mut result = EnsureReceipt {
            status: "queued".into(),
            requested: ordered.len(),
            sesno: root_snapshot.sesno,
            ..EnsureReceipt::default()
        };
        for ((dbnum, db_type, file, sesno), (refnos, mut definitions)) in groups {
            let source = ReadThroughSource {
                dbnum: dbnum as u32,
                file,
                sesno,
            };
            let open_source = source.clone();
            let (set, source) = tokio::task::spawn_blocking(move || {
                let schema = DirectSchema::open_from_env()?;
                let set = open_pinned_set(&schema, &open_source.file, &db_type, open_source.sesno)?;
                Ok::<_, anyhow::Error>((set, open_source))
            })
            .await
            .context("ensure file-open task failed")??;
            let uda_keys = UdaKeyMap::from_catalog(&self.catalog);
            definitions.sort_by_key(|definition| definition.key);
            definitions.dedup_by_key(|definition| definition.key);
            let prerequisites = definitions
                .iter()
                .map(uda_definition_upsert)
                .collect::<Vec<_>>();
            let rendered =
                render_rows_with_catalog(&set, &source, &refnos, &uda_keys, Some(&self.catalog))?;
            self.spool.persist(
                self.snapshot_epoch,
                vec![(source.dbnum, source.sesno)],
                "element-ensure",
                rendered_transaction_batches(&rendered, &source, &prerequisites)?,
            )?;
            result.written += rendered.elements.len();
            result.cache_epoch = result.cache_epoch.max(u64::from(source.sesno));
            result.missing.extend(
                rendered
                    .missing
                    .iter()
                    .copied()
                    .map(|value| RefnoEnum::from(value).to_pdms_str()),
            );
        }
        if matches!(
            tokio::time::timeout(Duration::from_secs(2), self.spool.replay(&SUL_DB)).await,
            Ok(Ok(_))
        ) {
            result.status = "committed".into();
        }
        result.elapsed_ms = started.elapsed().as_millis();
        Ok(result)
    }

    /// Refresh only rows that are already present in the read-through cache.
    /// Missing elements become session-guarded tombstones; uncached elements
    /// remain uncached.
    pub async fn refresh_cached(&self) -> anyhow::Result<EnsureReceipt> {
        if !crate::options::store_mode().is_durable() {
            return Ok(EnsureReceipt {
                status: "skipped-memory".into(),
                ..EnsureReceipt::default()
            });
        }
        #[derive(serde::Deserialize)]
        struct CachedRow {
            id: Value,
            dbnum: i32,
        }
        let started = Instant::now();
        let dbnums = self
            .membership
            .element_databases()
            .map(|db| db.dbnum)
            .collect::<Vec<_>>();
        if dbnums.is_empty() {
            return Ok(EnsureReceipt::default());
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT record::id(id) AS id, dbnum FROM pe WHERE dbnum IN {:?} AND !deleted;",
                dbnums
            ))
            .await?
            .check()?;
        let rows: Vec<CachedRow> = response.take(0)?;
        let mut groups: BTreeMap<(i32, String, PathBuf, u32), Vec<e3d_io::refno::RefNo>> =
            BTreeMap::new();
        let mut missing: BTreeMap<i32, Vec<String>> = BTreeMap::new();
        let mut definitions = Vec::new();
        for row in &rows {
            let Some((refno, id)) = value_as_refno_id(&row.id) else {
                continue;
            };
            match self.store.element_snapshot(refno) {
                Ok(snapshot)
                    if snapshot.db_type.eq_ignore_ascii_case("DESI")
                        || snapshot.db_type.eq_ignore_ascii_case("ISOD") =>
                {
                    definitions.extend(
                        snapshot
                            .explicit_attributes
                            .iter()
                            .filter(|attribute| attribute.is_uda)
                            .filter_map(|attribute| self.catalog.by_key(attribute.hash).cloned()),
                    );
                    groups
                        .entry((
                            snapshot.dbnum,
                            snapshot.db_type,
                            snapshot.file,
                            snapshot.sesno,
                        ))
                        .or_default()
                        .push(snapshot.extraction.refno);
                }
                Err(
                    DirectStoreError::NoSuchElement { .. }
                    | DirectStoreError::UnresolvedRef0 { .. },
                ) => {
                    missing.entry(row.dbnum).or_default().push(id);
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "refresh-if-present could not read cached element {} safely: {error}",
                        refno
                    ));
                }
                Ok(_) => {}
            }
        }
        definitions.sort_by_key(|definition| definition.key);
        definitions.dedup_by_key(|definition| definition.key);
        let prerequisites = definitions
            .iter()
            .map(uda_definition_upsert)
            .collect::<Vec<_>>();

        let mut receipt = EnsureReceipt {
            requested: rows.len(),
            ..EnsureReceipt::default()
        };
        let uda_keys = UdaKeyMap::from_catalog(&self.catalog);
        for ((dbnum, db_type, file, sesno), refnos) in groups {
            let source = ReadThroughSource {
                dbnum: dbnum as u32,
                file,
                sesno,
            };
            let opened = source.clone();
            let (set, source) = tokio::task::spawn_blocking(move || {
                let schema = DirectSchema::open_from_env()?;
                let set = open_pinned_set(&schema, &opened.file, &db_type, opened.sesno)?;
                Ok::<_, anyhow::Error>((set, opened))
            })
            .await
            .context("refresh-if-present file-open task failed")??;
            let written = write_through_on_with_catalog_and_prerequisites(
                &SUL_DB,
                &set,
                &source,
                &refnos,
                &uda_keys,
                Some(self.catalog.clone()),
                &prerequisites,
            )
            .await?;
            receipt.written += written.written;
            receipt.skipped_stale += written.skipped_stale;
            receipt.cache_epoch = receipt.cache_epoch.max(written.cache_epoch);
            receipt.sesno = receipt.sesno.max(sesno);
            receipt.missing.extend(written.missing);
        }
        for (dbnum, ids) in missing {
            let deletion_sesno = self
                .membership
                .element_databases()
                .find(|db| db.dbnum == dbnum as u32)
                .and_then(|db| db.path.as_ref())
                .and_then(|path| super::db_file_header::DbFileHeader::read(path).ok())
                .map(|header| header.latest_sesno())
                .unwrap_or_default();
            let (deleted, epoch) = super::read_through::ReadThroughCommitCoordinator::global()
                .soft_delete_missing(&SUL_DB, dbnum as u32, deletion_sesno, &ids)
                .await?;
            receipt.written += deleted;
            receipt.cache_epoch = receipt.cache_epoch.max(epoch);
            receipt.sesno = receipt.sesno.max(deletion_sesno);
        }
        receipt.elapsed_ms = started.elapsed().as_millis();
        Ok(receipt)
    }

    fn offer_backup(&self, snapshot: &DirectElementSnapshot) -> &'static str {
        if !crate::options::store_mode().is_durable() {
            return "skipped-memory";
        }
        if snapshot.db_type.eq_ignore_ascii_case("CATA") {
            return "skipped-cata";
        }
        if crate::options::data_face() == crate::options::DataFace::Ingest {
            return "skipped-ingest";
        }
        let source = ReadThroughSource {
            dbnum: snapshot.dbnum as u32,
            file: snapshot.file.clone(),
            sesno: snapshot.sesno,
        };
        let durable = (|| -> anyhow::Result<()> {
            let schema = DirectSchema::open_from_env()?;
            let set = open_pinned_set(&schema, &source.file, &snapshot.db_type, source.sesno)?;
            let prerequisites = snapshot
                .explicit_attributes
                .iter()
                .filter(|attribute| attribute.is_uda)
                .filter_map(|attribute| self.catalog.by_key(attribute.hash))
                .map(uda_definition_upsert)
                .collect::<Vec<_>>();
            let keys = UdaKeyMap::from_catalog(&self.catalog);
            let rendered = render_rows_with_catalog(
                &set,
                &source,
                &[snapshot.extraction.refno],
                &keys,
                Some(&self.catalog),
            )?;
            self.spool.persist(
                self.snapshot_epoch,
                vec![(source.dbnum, source.sesno)],
                "element",
                rendered_transaction_batches(&rendered, &source, &prerequisites)?,
            )?;
            Ok(())
        })();
        if let Err(error) = durable {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut degraded) = self.stats.degraded.write() {
                *degraded = Some(format!("mirror spool persist failed: {error:#}"));
            }
            return "degraded";
        }
        match self.backup_tx.try_send(()) {
            Ok(()) => {
                self.stats.queued.fetch_add(1, Ordering::Relaxed);
                "queued"
            }
            Err(error) => {
                if let Ok(mut degraded) = self.stats.degraded.write() {
                    *degraded = Some(format!("mirror replay wake unavailable: {error}"));
                }
                "queued"
            }
        }
    }

    async fn run_backup_worker(
        stats: Arc<QueueStats>,
        spool: Arc<MirrorSpool>,
        mut rx: mpsc::Receiver<()>,
    ) {
        if !crate::options::store_mode().is_durable() {
            return;
        }
        while rx.recv().await.is_some() {
            let result = tokio::time::timeout(Duration::from_secs(2), spool.replay(&SUL_DB)).await;
            match result {
                Ok(Ok(_)) => {
                    stats.succeeded.fetch_add(1, Ordering::Relaxed);
                    stats.last_success_ms.store(now_ms(), Ordering::Relaxed);
                    if let Ok(mut degraded) = stats.degraded.write() {
                        *degraded = None;
                    }
                }
                Ok(Err(error)) => {
                    stats.failed.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut degraded) = stats.degraded.write() {
                        *degraded = Some(format!("{error:#}"));
                    }
                    log::warn!("element attribute backup degraded: {error:#}");
                }
                Err(_) => {
                    stats.failed.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut degraded) = stats.degraded.write() {
                        *degraded = Some("mirror replay exceeded 2 seconds".into());
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ElementAttributeProvider {
    project: String,
    mdb: String,
    service: Arc<RwLock<Option<Arc<ElementAttributeService>>>>,
    reload: Arc<Mutex<()>>,
}

impl ElementAttributeProvider {
    pub fn new(project: impl Into<String>, mdb: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            mdb: mdb.into(),
            service: Arc::new(RwLock::new(None)),
            reload: Arc::new(Mutex::new(())),
        }
    }

    pub async fn get(&self) -> anyhow::Result<Arc<ElementAttributeService>> {
        if let Some(service) = self.service.read().await.clone() {
            let probe = service.clone();
            if tokio::task::spawn_blocking(move || probe.is_current())
                .await
                .context("element attribute freshness probe failed")??
            {
                return Ok(service);
            }
        }
        let _reload = self.reload.lock().await;
        if let Some(service) = self.service.read().await.clone() {
            let probe = service.clone();
            if tokio::task::spawn_blocking(move || probe.is_current())
                .await
                .context("element attribute freshness recheck failed")??
            {
                return Ok(service);
            }
        }
        let project = self.project.clone();
        let mdb = self.mdb.clone();
        let (service, rx) =
            tokio::task::spawn_blocking(move || ElementAttributeService::open(&project, &mdb))
                .await
                .context("element attribute service initialization task failed")??;
        let service = Arc::new(service);
        tokio::spawn(ElementAttributeService::run_backup_worker(
            service.stats.clone(),
            service.spool.clone(),
            rx,
        ));
        *self.service.write().await = Some(service.clone());
        Ok(service)
    }

    pub async fn health(&self) -> ReadThroughHealth {
        match self.service.read().await.as_ref() {
            Some(service) => service.health(),
            None => ReadThroughHealth::default(),
        }
    }

    pub async fn refresh_cached(&self) -> anyhow::Result<EnsureReceipt> {
        self.get().await?.refresh_cached().await
    }
}

pub async fn refresh_cached(project: &str, mdb: &str) -> anyhow::Result<EnsureReceipt> {
    let (service, _rx) = tokio::task::spawn_blocking({
        let project = project.to_owned();
        let mdb = mdb.to_owned();
        move || ElementAttributeService::open(&project, &mdb)
    })
    .await
    .context("refresh-if-present service task failed")??;
    service.refresh_cached().await
}

fn value_as_refno_id(value: &Value) -> Option<(RefU64, String)> {
    if let Some(value) = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
    {
        let refno = RefU64::from(value);
        return Some((refno, refno.to_string()));
    }
    let raw = value.as_str()?;
    if let Some((word0, word1)) = raw.split_once('_') {
        let refno = RefU64::from_two_nums(word0.parse().ok()?, word1.parse().ok()?);
        return Some((refno, refno.to_string()));
    }
    let refno = RefU64::from(raw.parse::<u64>().ok()?);
    Some((refno, refno.to_string()))
}

fn membership_fingerprint(membership: &MdbMembership) -> anyhow::Result<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    membership.project().hash(&mut hasher);
    membership.mdb().hash(&mut hasher);
    for db in membership.databases() {
        db.dbnum.hash(&mut hasher);
        db.styp.hash(&mut hasher);
        db.path.hash(&mut hasher);
        if let Some(path) = &db.path {
            let metadata = std::fs::metadata(path)
                .with_context(|| format!("read E3D identity for {}", path.display()))?;
            metadata.len().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
            if let Ok(header) = super::db_file_header::DbFileHeader::read(path) {
                header.latest_sesno().hash(&mut hasher);
            }
        }
    }
    Ok(hasher.finish())
}

pub(crate) fn load_uda_catalog(
    membership: &MdbMembership,
    schema: &DirectSchema,
) -> anyhow::Result<UdaCatalog> {
    let dicvir = schema.template_dir().join("dicvir.dat");
    let mut merged = UdaCatalog::default();
    for dictionary in membership.dictionary_paths() {
        let catalog = UdaCatalog::read(&dictionary, &dicvir, schema.attlib())
            .map_err(|error| anyhow::anyhow!("read DICT {}: {error}", dictionary.display()))?;
        merged.merge_in_order(catalog);
    }
    Ok(merged)
}

fn identity_attribute(name: &str, value: Value) -> AttributeValue {
    let display = value.as_str().unwrap_or_default().to_owned();
    AttributeValue {
        name: name.to_owned(),
        value_type: "text".to_owned(),
        value,
        display,
        origin: "stored",
        is_uda: false,
        is_unset: false,
        editable: false,
    }
}

fn render_attribute(attribute: &ExtractedAttribute) -> Option<AttributeValue> {
    let is_uda = attribute.name.starts_with(':');
    let value = attribute.value.as_ref()?;
    let is_unset = descriptor_is_unset(value);
    let (value_type, value, display, editable) = descriptor_json(value);
    let origin = match attribute.status {
        AttributeExtractionStatus::TemplateDefault => "schema-default",
        AttributeExtractionStatus::CatalogDefault if is_unset => "unset",
        AttributeExtractionStatus::CatalogDefault => "uda-default",
        AttributeExtractionStatus::DecodedExplicit if is_uda => "uda-stored",
        AttributeExtractionStatus::DecodedExplicit => "explicit",
        _ => "stored",
    };
    Some(AttributeValue {
        name: attribute.name.trim_start_matches(':').to_owned(),
        value_type,
        value,
        display,
        origin,
        is_uda,
        is_unset,
        editable: editable && !is_unset,
    })
}

fn descriptor_is_unset(value: &DescriptorValue) -> bool {
    matches!(value, DescriptorValue::Unset)
        || matches!(value, DescriptorValue::Word { raw: 0, text } if text.as_deref().is_none_or(str::is_empty))
}

fn descriptor_json(value: &DescriptorValue) -> (String, Value, String, bool) {
    use DescriptorValue as D;
    match value {
        D::Unset => ("unset".into(), Value::Null, "unset".into(), false),
        D::Bool(value) => ("bool".into(), json!(value), value.to_string(), true),
        D::Int(value) => ("int".into(), json!(value), value.to_string(), true),
        D::Real(value) => ("real".into(), json!(value), value.to_string(), true),
        D::Text(value) => ("text".into(), json!(value), value.clone(), true),
        D::RefNo(value) => ("ref".into(), json!(pdms(*value)), pdms(*value), true),
        // A stored zero WORD is native UNSET. Preserve the durable empty string
        // while exposing unset semantics to the UI; unknown nonzero words keep
        // their numeric fallback for diagnostics.
        D::Word { .. } if descriptor_is_unset(value) => {
            ("word".into(), json!(""), "unset".into(), false)
        }
        D::Word { raw, text } => {
            let value = text
                .clone()
                .or_else(|| e3d_attlib::db1_dehash(*raw))
                .unwrap_or_else(|| raw.to_string());
            ("word".into(), json!(value), value, true)
        }
        D::IntArray(value) => ("int-array".into(), json!(value), display_array(value), true),
        D::RealArray(value) if value.len() == 3 => {
            ("vec3".into(), json!(value), display_array(value), true)
        }
        D::RealArray(value) => (
            "real-array".into(),
            json!(value),
            display_array(value),
            true,
        ),
        D::RefNoArray(value) => {
            let values = value.iter().copied().map(pdms).collect::<Vec<_>>();
            ("ref-array".into(), json!(values), values.join(", "), true)
        }
        D::WordArray(value) => (
            "word-array".into(),
            json!(value),
            display_array(value),
            false,
        ),
        D::BoolArray(value) => (
            "bool-array".into(),
            json!(value),
            display_array(value),
            true,
        ),
        D::RawWords(value) => (
            "raw-words".into(),
            json!(value),
            display_array(value),
            false,
        ),
    }
}

fn explicit_json(value: &ExplicitValue) -> (String, Value, String, bool) {
    match value {
        ExplicitValue::Integer(value) => ("int".into(), json!(value), value.to_string(), true),
        ExplicitValue::Real(value) => ("real".into(), json!(value), value.to_string(), true),
        ExplicitValue::Bool(value) => ("bool".into(), json!(value), value.to_string(), true),
        ExplicitValue::Text(value) => ("text".into(), json!(value), value.clone(), true),
        ExplicitValue::RefNo(value) => ("ref".into(), json!(pdms(*value)), pdms(*value), true),
        ExplicitValue::IntArray(value) => {
            ("int-array".into(), json!(value), display_array(value), true)
        }
        ExplicitValue::RealArray(value) => (
            "real-array".into(),
            json!(value),
            display_array(value),
            true,
        ),
        ExplicitValue::RefNoArray(value) => {
            let values = value.iter().copied().map(pdms).collect::<Vec<_>>();
            ("ref-array".into(), json!(values), values.join(", "), true)
        }
        ExplicitValue::RawWords(value) => (
            "raw-words".into(),
            json!(value),
            display_array(value),
            false,
        ),
    }
}

fn display_array<T: std::fmt::Display>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn pdms(refno: e3d_io::refno::RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn uda_definition_upsert(definition: &e3d_io::uda_catalog::UdaDefinition) -> String {
    let id = RefU64::from_two_nums(definition.refno.word0, definition.refno.word1);
    let name =
        serde_json::to_string(&format!(":{}", definition.name)).unwrap_or_else(|_| "\"\"".into());
    let value_type = serde_json::to_string(definition.value_type.as_deref().unwrap_or(""))
        .unwrap_or_else(|_| "\"\"".into());
    format!(
        "UPSERT UDA:{id} MERGE {{ UKEY: {}, UDNA: {name}, DYUDNA: {name}, UTYP: {value_type} }};",
        definition.key as i32
    )
}

fn push_limited(
    ordered: &mut Vec<RefU64>,
    seen: &mut HashSet<RefU64>,
    refno: RefU64,
) -> anyhow::Result<()> {
    if seen.insert(refno) {
        anyhow::ensure!(
            ordered.len() < 5_000,
            "ENSURE_SCOPE_TOO_LARGE: scope exceeds 5000 elements"
        );
        ordered.push(refno);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_keep_json_shape_and_reference_spelling() {
        let (_, value, display, editable) = descriptor_json(&DescriptorValue::RefNoArray(vec![
            e3d_io::refno::RefNo::new(12, 34),
            e3d_io::refno::RefNo::new(56, 78),
        ]));
        assert_eq!(value, json!(["12/34", "56/78"]));
        assert_eq!(display, "12/34, 56/78");
        assert!(editable);
    }

    #[test]
    fn unset_is_null_and_not_editable() {
        let (kind, value, display, editable) = descriptor_json(&DescriptorValue::Unset);
        assert_eq!(kind, "unset");
        assert_eq!(value, Value::Null);
        assert_eq!(display, "unset");
        assert!(!editable);
    }

    #[test]
    fn empty_word_keeps_durable_empty_value_and_unset_display() {
        for text in [None, Some(String::new())] {
            let word = DescriptorValue::Word { raw: 0, text };
            assert!(descriptor_is_unset(&word));
            let (kind, value, display, editable) = descriptor_json(&word);
            assert_eq!(kind, "word");
            assert_eq!(value, json!(""));
            assert_eq!(display, "unset");
            assert!(!editable);
        }
        let (_, value, _, _) = descriptor_json(&DescriptorValue::Word {
            raw: 0,
            text: Some("OPEN".into()),
        });
        assert_eq!(value, json!("OPEN"));
    }
}
