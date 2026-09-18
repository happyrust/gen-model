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
use e3d_io::db_element::DbSet;
use e3d_io::noun_catalog::NounCatalog;
use e3d_io::record::descriptor::{
    AttributeExtractionStatus, DescriptorValue, ElementExtraction, ExtractedAttribute,
};
use e3d_io::record::explicit::{ExplicitAttribute, ExplicitValue};
use e3d_io::refno::RefNo;
use e3d_io::uda_catalog::UdaCatalog;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock, mpsc};

use super::direct_store::{DirectElementSnapshot, DirectSchema, DirectStoreError};
use super::e3d_collector::open_pinned_set;
use super::mdb_membership::MdbMembership;
use super::mdb_read_store::{MdbReadProvider, MdbReadStore};
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

/// 这一份属性读得完整不完整，答案分两层：**只有前三格算「不完整」**，后三格是
/// 说明，进响应供排查、不参与 `complete`。混在一起数过——`NonImplicit` 这种「本就
/// 没有值」也算一笔，于是 BRAN 常年 40 多项诊断，界面那条告警成了常驻，真出事的
/// `Unsupported` / `OutOfBounds` 反倒淹在里头看不见。
#[derive(Debug, Clone, Default, Serialize)]
pub struct AttributeDiagnostics {
    /// 文件里有值、任何表示都落不下去。真读丢了。
    pub shape_conflicts: Vec<Value>,
    /// 该有值却没解出来（`Unsupported` / `OutOfBounds` 这类）。
    ///
    /// `NonImplicit`（描述符里压根没有隐式槽位）与 `DefaultRequired`（槽位是未设
    /// 哨兵且无模板默认）**不在此列**：引擎已经依次试过显式流与 noun 默认记录，
    /// 留在这两个状态就是 E3D 自己的 unset，不是解码失败。判据与
    /// [`super::direct_attmap::undecoded_reason`] 同一处，不另立一套。
    pub undecoded: Vec<Value>,
    /// **本元素**的 UDA 问题（字典里查无此定义，只能按哈希名显示）。
    pub uda_issues: Vec<String>,
    /// 应用 schema 不认识的键。值照样按文件自然形状给出，面板显示是对的。
    pub outside_schema: Vec<String>,
    /// schema 声明与文件形状对不上，但 DB 读侧拿同一份声明同样读不出（`TYPEX` /
    /// `SPAMAP` / `UNIPAR` 这些簿记键）。两侧同样读不出就谈不上直读读错，
    /// 见 [`super::direct_attmap::DirectAttrs::view_divergence`]。
    pub view_divergence: Vec<Value>,
    /// 整个项目 Dictionary 的定义问题，与当前元素无关——一条坏定义不该让全项目
    /// 每个元素都挂上「属性不完整」。
    pub catalog_issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub status: &'static str,
}

/// 一个会话钉死的读取点：[`ElementAttributeService::read_pinned`] 往回执里写的库身份，
/// 也是 [`ElementAttributeService::open_pinned`] 开池的依据。`db_type` 决定模板文件
/// （`DESI` → `desvir.dat`），从库文件头反哈希得来。
#[derive(Debug, Clone)]
pub struct PinnedRead {
    pub dbnum: i32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
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
    store: Arc<MdbReadStore>,
    reader: Arc<MdbReadProvider>,
    catalog: Arc<super::uda_catalog_provider::UdaCatalogProvider>,
    membership: Arc<MdbMembership>,
    backup_tx: mpsc::Sender<()>,
    spool: Arc<MirrorSpool>,
    stats: Arc<QueueStats>,
    /// Process-local read-view epoch.
    snapshot_epoch: u64,
}

impl ElementAttributeService {
    /// 打开属性服务：接上 `(project, mdb)` 的共享读视图（`MdbReadProvider::shared`），
    /// 与树服务共用同一份路由目录、`DbSet` 与元素缓存。
    ///
    /// **不读任何元素**（切片 W5，母计划阶段 3A）：库文件由 CURD 声明定，refno 属于哪个库
    /// 由 `RefNo::dbno()` 位解码定（`MdbReadStore::dbnum_of`），所以这里不等任何逐元素
    /// `ref0 → dbnum` 骨架——JEU 现场那 68 秒 / 1,221,858 个元素就是它。依赖项目的目录库
    /// （CATA / PROP）按文件头并进目录，同号跨项目副本留作 `preferred_project` 候选。
    fn open(project: &str, mdb: &str) -> anyhow::Result<(Self, mpsc::Receiver<()>)> {
        let option = aios_core::get_db_option();
        let reader = MdbReadProvider::shared(project, mdb)?;
        let membership = reader.membership.clone();
        let schema = reader.schema.clone();
        let store = reader.view()?;
        let snapshot_epoch = store.epoch;
        let catalog = super::uda_catalog_provider::UdaCatalogProvider::shared(&membership, schema.clone());
        let spool = Arc::new(MirrorSpool::for_process(&option.surreal_ns, project, mdb)?);
        let (backup_tx, backup_rx) = mpsc::channel(BACKUP_QUEUE_CAPACITY);
        Ok((
            Self {
                project: project.to_owned(),
                mdb: format!("/{}", mdb.trim_start_matches('/')),
                store,
                reader,
                catalog,
                membership,
                backup_tx,
                spool,
                stats: Arc::new(QueueStats::default()),
                snapshot_epoch,
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
        Ok(Arc::ptr_eq(&self.store, &self.reader.view()?))
    }

    pub fn read(&self, refno: RefU64) -> anyhow::Result<ElementAttributesResponse> {
        let snapshot = self.store.element_snapshot(refno)?;
        self.render(snapshot, true)
    }

    /// 打开一个会话钉死的单库 `DbSet`，给 [`Self::read_pinned`] 用；调用方在同一个池上先做
    /// 存在 / 归属判断（`exists` / `is_descendant_of`），再读属性，不必开两次。
    pub fn open_pinned(&self, pin: &PinnedRead) -> anyhow::Result<Arc<DbSet>> {
        open_pinned_set(&self.reader.schema, &pin.file, &pin.db_type, pin.sesno)
    }

    /// 某构件在**指定会话**下的属性——属性历史对比（`model/history/query tool=attributes`，
    /// ADR-081 候选条）走这里。
    ///
    /// 与 [`Self::read`] 同一个渲染器（identity 行 + `render_attribute` + 类型默认值 / UDA
    /// materialize），属性面板与属性历史对比对同一个属性印同一个字；不同的只有取数——不经
    /// `DirectStore` 那个钉在当前时点的会话，而是在调用方给的、按 `pin.sesno` 钉死的池上
    /// `extraction()` / `parsed()`。**只读文件、不 offer_backup、不落库**（d-179：历史投影
    /// 除 mesh 外一律不进库）。该 refno 在那个会话的索引里不存在（还没建 / 已删）→ `Ok(None)`，
    /// 由调用方按「该版本不存在」的正常态回。
    pub fn read_pinned(
        &self,
        set: &Arc<DbSet>,
        pin: &PinnedRead,
        refno: RefNo,
    ) -> anyhow::Result<Option<ElementAttributesResponse>> {
        let element = set.element(refno);
        let exists = element.exists().map_err(|error| {
            anyhow::anyhow!(
                "会话 {} 的索引查 {}/{} 失败: {error}",
                pin.sesno,
                refno.word0,
                refno.word1
            )
        })?;
        if !exists {
            return Ok(None);
        }
        let extraction = (*element.extraction().map_err(|error| {
            anyhow::anyhow!(
                "会话 {} 下抽取 {}/{} 失败: {error}",
                pin.sesno,
                refno.word0,
                refno.word1
            )
        })?)
        .clone();
        let explicit_attributes = element
            .parsed()
            .map_err(|error| {
                anyhow::anyhow!(
                    "会话 {} 下解析 {}/{} 失败: {error}",
                    pin.sesno,
                    refno.word0,
                    refno.word1
                )
            })?
            .explicit_attributes
            .clone();
        let snapshot = DirectElementSnapshot {
            dbnum: pin.dbnum,
            db_type: pin.db_type.clone(),
            file: pin.file.clone(),
            sesno: pin.sesno,
            extraction,
            explicit_attributes,
        };
        self.render(snapshot, false).map(Some)
    }

    /// 把一份文件快照渲染成公开的属性行。`read` / `read_pinned` 共用这一条：诊断按文件原样记、
    /// 再补 E3D 会印的类型默认值、UDA 字典 materialize、identity 三行、按名排序去重。
    /// `offer_backup` 只有当前时点的 `read` 给 true——历史会话的读不进备份库（d-179）。
    fn render(
        &self,
        mut snapshot: DirectElementSnapshot,
        offer_backup: bool,
    ) -> anyhow::Result<ElementAttributesResponse> {
        let catalog = self.catalog.get()?;
        let mut diagnostics = AttributeDiagnostics {
            catalog_issues: catalog
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
                .map(shape_conflict_json)
                .collect();
            diagnostics.view_divergence = projected
                .view_divergence
                .into_iter()
                .map(shape_conflict_json)
                .collect();
            // `projected.undecoded` 与下面那轮渲染同源同判据，取一处即可——两处都记
            // 会把同一个失败的键数成两笔。留渲染那一轮：它按面板实际画不出来的行记。
        }
        // 诊断已按文件原样记完，这之后才补 E3D 的类型默认值：它们是 E3D 会印的行，
        // 不是文件里的值，不该改变「文件读得完整不完整」的答案。
        if let Some(noun_catalog) = self.reader.schema.noun_catalog() {
            materialize_type_defaults(&noun_catalog, &mut snapshot.extraction);
        }
        let known_uda_keys = catalog
            .definitions()
            .iter()
            .flat_map(|definition| [Some(definition.key), definition.old_key])
            .flatten()
            .collect::<HashSet<_>>();
        catalog
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
            if is_unnamed_placeholder(attribute) {
                continue;
            }
            match render_attribute(attribute) {
                Some(value) => attributes.push(value),
                // 没渲染出来不等于读坏了：`NonImplicit` / `DefaultRequired` 是
                // E3D 的 unset，`undecoded_diagnostic` 把它们挡在诊断之外。
                None => diagnostics
                    .undecoded
                    .extend(undecoded_diagnostic(attribute)),
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

        let backup = if offer_backup {
            self.offer_backup(&snapshot, &catalog)
        } else {
            "skipped-pinned"
        };
        let complete = diagnostics.shape_conflicts.is_empty()
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
        let catalog = self.catalog.get()?;
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
                    .filter_map(|attribute| catalog.by_key(attribute.hash).cloned()),
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
            let uda_keys = UdaKeyMap::from_catalog(&catalog);
            definitions.sort_by_key(|definition| definition.key);
            definitions.dedup_by_key(|definition| definition.key);
            let prerequisites = definitions
                .iter()
                .map(uda_definition_upsert)
                .collect::<Vec<_>>();
            let rendered =
                render_rows_with_catalog(&set, &source, &refnos, &uda_keys, Some(&catalog))?;
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
        let catalog = self.catalog.get()?;
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
                            .filter_map(|attribute| catalog.by_key(attribute.hash).cloned()),
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
                    | DirectStoreError::InvalidRefNo { .. }
                    | DirectStoreError::DbNotInMdb { .. },
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
        let uda_keys = UdaKeyMap::from_catalog(&catalog);
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
                Some(catalog.clone()),
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

    fn offer_backup(&self, snapshot: &DirectElementSnapshot, catalog: &UdaCatalog) -> &'static str {
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
                .filter_map(|attribute| catalog.by_key(attribute.hash))
                .map(uda_definition_upsert)
                .collect::<Vec<_>>();
            let keys = UdaKeyMap::from_catalog(&catalog);
            let rendered = render_rows_with_catalog(
                &set,
                &source,
                &[snapshot.extraction.refno],
                &keys,
                Some(&catalog),
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

    pub async fn cache_stats(&self) -> (Vec<i32>, usize) {
        self.service.read().await.as_ref().map(|service|
            (service.store.opened_dbnums(), service.store.cached_elements()))
            .unwrap_or_default()
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

/// 把 E3D 会印出来的类型默认值补进抽取件。
///
/// `q att` 对一个没存过值的属性不是不印，印的是**类型的零**：`HeadWidth 0mm`、
/// `Tsfbreak false`、`Orrf Nulref`、`Duty unset`。文件直读只拿得到「这里没有值」
/// （`NonImplicit`），于是面板比 E3D 少几十行；哪个属性是什么类型、哪些是伪属性，
/// 只有活 E3D 导出的逻辑 noun 目录知道，所以这一步要它在场。
///
/// 三步与 e3d-io `e3d-descriptor describe` 同序：静态类型零 → 有已知运行时规则的
/// 伪属性（BRAN 圆口的 HEADW / HEADH / TAILW / TAILH 为 0、INCO 为空串、RLST 为
/// unset）→ 其余伪属性的类型零；后两步只认前面没认领的行。
///
/// 只补**目录里有**的属性。描述符里有、E3D 自己的属性面里没有的（BRAN 上的 `UDTYPE`
/// / `WDATUM` / `RREF` / `UDATAB` …）E3D 的 `q` 同样答不上来（2026-09-18 TTY 对拍：
/// 三个 noun 上 9 / 9 / 7 个），它们照旧不画——面板对齐的是 E3D，不是描述符。
fn materialize_type_defaults(catalog: &NounCatalog, extraction: &mut ElementExtraction) {
    catalog.apply_static_defaults(extraction);
    catalog.apply_runtime_derived(extraction);
    catalog.apply_pseudo_defaults(extraction);
}

/// 没起过名的元素，`NAME` 的类型默认是 `unset`——但 E3D 从不印 `Name unset`，它当场
/// 编一个 `FTUBE 4 of BRANCH /C-OR-1R345-C`。那个全名要走属主链与兄弟序号，是 `FLNM`
/// 伪属性那条线的事；在它落地之前这一行不画：画一个 E3D 不会印的 `unset`，比少一行
/// 离 E3D 更远。存过名的 `NAME`（`Decoded`）不受影响。
fn is_unnamed_placeholder(attribute: &ExtractedAttribute) -> bool {
    attribute.name == "NAME" && attribute.status == AttributeExtractionStatus::CatalogDefault
}

/// 一个画不出行的属性，值不值得报一笔诊断。
///
/// 判据借 [`super::direct_attmap::undecoded_reason`]，不在这里另写一份：同一个
/// 响应里两套「什么叫没有值」，是这条告警常驻的根由。
fn undecoded_diagnostic(attribute: &ExtractedAttribute) -> Option<Value> {
    super::direct_attmap::undecoded_reason(&attribute.status)
        .map(|reason| json!({ "name": attribute.name, "reason": reason }))
}

fn shape_conflict_json(conflict: super::direct_attmap::ShapeConflict) -> Value {
    json!({
        "name": conflict.name,
        "found": conflict.found,
        "declared": conflict.declared,
    })
}

fn render_attribute(attribute: &ExtractedAttribute) -> Option<AttributeValue> {
    let is_uda = attribute.name.starts_with(':');
    let value = attribute.value.as_ref()?;
    let is_unset = descriptor_is_unset(value);
    let (value_type, value, display, editable) = descriptor_json(value);
    // E3D 运行时按规则算出来的伪属性值（圆口支路 HEADW=0、INCO 空串）：没有存储槽位，
    // 改它改的不是任何一处字节，所以不给编辑框。
    let derived = matches!(
        attribute.status,
        AttributeExtractionStatus::RuntimeDerived { .. }
    );
    let origin = match attribute.status {
        AttributeExtractionStatus::TemplateDefault => "schema-default",
        AttributeExtractionStatus::CatalogDefault if is_unset => "unset",
        AttributeExtractionStatus::CatalogDefault if is_uda => "uda-default",
        // noun 目录给的类型零：E3D 对没存过值的属性印的那个 `0` / `false` / `Nulref`。
        // 与 `schema-default`（noun 默认记录里存着的值）不同，这里文件与模板都没有值。
        AttributeExtractionStatus::CatalogDefault => "type-default",
        AttributeExtractionStatus::RuntimeDerived { .. } => "derived",
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
        editable: editable && !is_unset && !derived,
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

    /// 守卫（d-179 / ADR-081 候选条）：属性面板与属性历史对比走**同一个**渲染器——`read` 与
    /// `read_pinned` 都只是把一份 `DirectElementSnapshot` 交给 `render`；而历史会话的读
    /// 不进备份库：`offer_backup` 只在 `render` 里、且只有 `read` 那条传 true。
    #[test]
    fn pinned_reads_share_the_renderer_and_never_offer_backup() {
        let source = include_str!("element_attributes.rs");
        let read = source
            .split_once("pub fn read(&self, refno: RefU64)")
            .expect("read exists")
            .1
            .split_once("pub fn open_pinned(")
            .expect("open_pinned follows read")
            .0;
        assert!(read.contains("self.render(snapshot, true)"), "{read}");

        let pinned = source
            .split_once("pub fn read_pinned(")
            .expect("read_pinned exists")
            .1
            .split_once("/// 把一份文件快照渲染成公开的属性行")
            .expect("render's doc follows read_pinned")
            .0;
        assert!(pinned.contains("self.render(snapshot, false)"), "{pinned}");
        assert!(pinned.contains("return Ok(None);"), "不存在 → None，不是错误");
        for forbidden in ["offer_backup", "spool", "SUL_DB", "store.element_snapshot"] {
            assert!(
                !pinned.contains(forbidden),
                "read_pinned 不得走当前时点会话或备份库: {forbidden}"
            );
        }

        let render = source
            .split_once("fn render(")
            .expect("render exists")
            .1
            .split_once("pub async fn ensure(")
            .expect("ensure follows render")
            .0;
        // 针脚拼出来，免得这段测试自己的字面量也被数进去。
        let call = format!("self.{}(", "offer_backup");
        assert_eq!(
            render.matches(call.as_str()).count(),
            1,
            "offer_backup 只在 render 里出现一次"
        );
        assert!(render.contains("if offer_backup {"));
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

    /// 一个没有值的属性。存储布局字段与这里的分类无关，给最小值即可。
    fn valueless(name: &str, status: AttributeExtractionStatus) -> ExtractedAttribute {
        use e3d_io::record::template::{EncodedLocation, StorageDescriptor, StorageLayout};

        ExtractedAttribute {
            hash: 0,
            name: name.to_string(),
            storage_type_code: 0,
            semantic_type_code: None,
            semantic_size: None,
            layout: StorageLayout::Compact,
            storage: StorageDescriptor {
                storage_words: 0,
                location: EncodedLocation { raw: 0 },
                flags: 0,
            },
            status,
            value: None,
        }
    }

    /// E3D 的 unset 不是解码失败。`NonImplicit` 是「描述符里没有隐式槽位」，引擎已经
    /// 依次试过显式流与 noun 默认记录；`DefaultRequired` 是「槽位写着未设哨兵、也没有
    /// 模板默认」。两者都画不出行，但都不该记诊断——BRAN 上这一类有四十几个，全记
    /// 进去，`complete` 就再没有真过。
    #[test]
    fn e3d_unset_never_becomes_a_diagnostic() {
        for status in [
            AttributeExtractionStatus::NonImplicit,
            AttributeExtractionStatus::DefaultRequired,
        ] {
            let attribute = valueless("DUTY", status);
            assert!(render_attribute(&attribute).is_none());
            assert_eq!(undecoded_diagnostic(&attribute), None);
        }
    }

    /// 真读不出来的照记，且带上原因——这一类才是 `complete=false` 要说的事。
    #[test]
    fn a_real_decode_failure_still_reports_its_reason() {
        let attribute = valueless(
            "SPREF",
            AttributeExtractionStatus::OutOfBounds {
                detail: "offset 12 + storage 4 exceeds implicit words 8".into(),
            },
        );
        assert!(render_attribute(&attribute).is_none());
        let diagnostic = undecoded_diagnostic(&attribute).expect("a real failure must be reported");
        assert_eq!(diagnostic["name"], "SPREF");
        assert!(
            diagnostic["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("OutOfBounds")),
            "{diagnostic}"
        );
    }

    /// 一个有值的属性，状态由调用方给。
    fn valued(
        name: &str,
        status: AttributeExtractionStatus,
        value: DescriptorValue,
    ) -> ExtractedAttribute {
        ExtractedAttribute {
            hash: e3d_attlib::db1_hash(name.trim_start_matches(':')),
            value: Some(value),
            ..valueless(name, status)
        }
    }

    /// E3D 对没存过值的属性印的是类型的零，面板照印成行：`0` / `false` / `0/0`（E3D
    /// 拼作 `Nulref`），`origin` 记 `type-default`，界面由此知道它不是文件里的值；字与
    /// 文本的类型零就是 `unset`，沿用原有那一格，不可编辑。
    #[test]
    fn type_defaults_render_as_rows_e3d_would_print() {
        use AttributeExtractionStatus::CatalogDefault;
        let nulref = DescriptorValue::RefNo(e3d_io::refno::RefNo::new(0, 0));
        let empty_word = DescriptorValue::Word { raw: 0, text: None };
        let cases = [
            ("HEADW", DescriptorValue::Real(0.0), "real", "0", "type-default", false, true),
            ("BSTA", DescriptorValue::Int(0), "int", "0", "type-default", false, true),
            ("TSFBR", DescriptorValue::Bool(false), "bool", "false", "type-default", false, true),
            ("ORRF", nulref, "ref", "0/0", "type-default", false, true),
            ("HDMT", empty_word, "word", "unset", "unset", true, false),
            ("DESC", DescriptorValue::Unset, "unset", "unset", "unset", true, false),
        ];
        for (name, value, value_type, display, origin, is_unset, editable) in cases {
            let row = render_attribute(&valued(name, CatalogDefault, value))
                .unwrap_or_else(|| panic!("{name} 必须成行"));
            assert_eq!(
                (
                    row.value_type.as_str(),
                    row.display.as_str(),
                    row.origin,
                    row.is_unset,
                    row.editable,
                ),
                (value_type, display, origin, is_unset, editable),
                "{name}"
            );
            assert!(!row.is_uda, "{name}");
        }
    }

    /// 字典 UDA 自带的默认值仍记 `uda-default`：那是 Dictionary 里声明的值，不是类型的零。
    #[test]
    fn a_uda_dictionary_default_keeps_its_own_origin() {
        let row = render_attribute(&valued(
            ":AREA",
            AttributeExtractionStatus::CatalogDefault,
            DescriptorValue::Int(100),
        ))
        .unwrap();
        assert_eq!(
            (row.name.as_str(), row.origin, row.is_uda, row.editable),
            ("AREA", "uda-default", true, true)
        );
    }

    /// 运行时算出来的伪属性（圆口支路的 HEADW 为 0、INCO 为空串）记 `derived`，成行但
    /// 不给编辑框：它没有存储槽位，改它改的不是任何一处字节。
    #[test]
    fn runtime_derived_rows_are_shown_but_not_editable() {
        let derived = |evaluator: &str| AttributeExtractionStatus::RuntimeDerived {
            evaluator: evaluator.into(),
        };
        let width = render_attribute(&valued(
            "HEADW",
            derived("bran_circular_end_dimensions"),
            DescriptorValue::Real(0.0),
        ))
        .unwrap();
        assert_eq!(
            (width.origin, width.display.as_str(), width.is_unset, width.editable),
            ("derived", "0", false, false)
        );
        let inco = render_attribute(&valued(
            "INCO",
            derived("design_qrofcn_absent_internal_points"),
            DescriptorValue::Text(String::new()),
        ))
        .unwrap();
        assert_eq!(
            (inco.origin, inco.display.as_str(), inco.editable),
            ("derived", "", false)
        );
    }

    /// 没起过名的 `NAME`：类型默认是 `unset`，但 E3D 印的是现编的全名，这一行不画；
    /// 存过名的照画，别的属性的 `unset` 也照画。
    #[test]
    fn an_unnamed_element_gets_no_name_row_until_flnm_lands() {
        use AttributeExtractionStatus::{CatalogDefault, Decoded};
        assert!(is_unnamed_placeholder(&valued(
            "NAME",
            CatalogDefault,
            DescriptorValue::Unset
        )));
        assert!(!is_unnamed_placeholder(&valued(
            "NAME",
            Decoded,
            DescriptorValue::Text("/1WCC1135/B1".into())
        )));
        assert!(!is_unnamed_placeholder(&valued(
            "DESC",
            CatalogDefault,
            DescriptorValue::Unset
        )));
    }

    /// 一份只认识一个 BRAN 的 noun 目录，属性哈希与描述符同源（`db1_hash`）。
    fn tiny_bran_catalog() -> NounCatalog {
        let attribute = |name: &str, type_name: &str, is_pseudo: bool| {
            format!(
                r#"{{"name":"{name}","hash":{hash},"type":0,"typeName":"{type_name}","isArray":false,"maxSize":1,"trueSize":0,"isUda":false,"isPseudo":{is_pseudo},"hidden":false,"noClaim":false,"def":"e2/153"}}"#,
                hash = e3d_attlib::db1_hash(name)
            )
        };
        let attrs = [
            attribute("NAME", "STRING", true),
            attribute("DESC", "STRING", false),
            attribute("HBOR", "DOUBLE", false),
            attribute("HEADW", "DOUBLE", true),
            attribute("BSTA", "INTEGER", false),
            attribute("ORRF", "ELEMENT", false),
        ];
        let layout = format!(
            r#"[{{"noun":"BRANCH","short":"BRAN","hash":{hash},"base":"BRANCH","hard":"BRANCH","isUdet":false,"isPseudo":false,"isWorld":false,"isPrimary":true,"visible":true,"dbTypes":[1],"attrs":[{attrs}]}}]"#,
            hash = e3d_attlib::db1_hash("BRAN"),
            attrs = attrs.join(",")
        );
        let fields = format!(
            "{{{}}}",
            ["NAME", "DESC", "HBOR", "HEADW", "BSTA", "ORRF"]
                .iter()
                .map(|name| format!(
                    r#""{}":{{"name":"{name}","f":{{}}}}"#,
                    e3d_attlib::db1_hash(name)
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        NounCatalog::parse(&layout, &fields).unwrap()
    }

    /// 补类型默认值的三步与 e3d-io 同序，且只补目录里有的属性。
    ///
    /// 圆口支路（HBOR 存了 15）的 HEADW 走运行时规则成 `derived` 0，不被静态那一步
    /// 抢先成 `type-default`；没起过名的 NAME 拿到的是伪属性的类型零；描述符里有、
    /// 目录里没有的 UDTYPE 一个字都不动——E3D 的 `q` 对它同样答不上来。
    #[test]
    fn type_defaults_follow_the_catalog_and_leave_descriptor_only_rows_alone() {
        use AttributeExtractionStatus::{CatalogDefault, Decoded, NonImplicit};
        let stored = |name: &str, value: DescriptorValue| valued(name, Decoded, value);
        let absent = |name: &str| ExtractedAttribute {
            hash: e3d_attlib::db1_hash(name),
            ..valueless(name, NonImplicit)
        };
        let mut extraction = ElementExtraction {
            refno: e3d_io::refno::RefNo::new(24383, 66459),
            owner: e3d_io::refno::RefNo::new(24383, 1),
            noun_hash: e3d_attlib::db1_hash("BRAN"),
            noun_name: Some("BRAN".into()),
            implicit_word_count: 0,
            format_word_10: 0,
            layout: e3d_io::record::template::StorageLayout::Compact,
            attributes: vec![
                absent("NAME"),
                absent("DESC"),
                stored("HBOR", DescriptorValue::Real(15.0)),
                absent("HEADW"),
                absent("BSTA"),
                absent("ORRF"),
                absent("UDTYPE"),
            ],
        };

        materialize_type_defaults(&tiny_bran_catalog(), &mut extraction);

        let status_and_value = |name: &str| {
            let attribute = extraction.find_attribute(name).unwrap();
            (attribute.status.clone(), attribute.value.clone())
        };
        assert_eq!(
            status_and_value("HEADW"),
            (
                AttributeExtractionStatus::RuntimeDerived {
                    evaluator: "bran_circular_end_dimensions".into(),
                },
                Some(DescriptorValue::Real(0.0))
            )
        );
        assert_eq!(
            status_and_value("BSTA"),
            (CatalogDefault, Some(DescriptorValue::Int(0)))
        );
        assert_eq!(
            status_and_value("ORRF"),
            (
                CatalogDefault,
                Some(DescriptorValue::RefNo(e3d_io::refno::RefNo::new(0, 0)))
            )
        );
        assert_eq!(
            status_and_value("DESC"),
            (CatalogDefault, Some(DescriptorValue::Unset))
        );
        assert_eq!(
            status_and_value("NAME"),
            (CatalogDefault, Some(DescriptorValue::Unset))
        );
        assert!(is_unnamed_placeholder(extraction.find_attribute("NAME").unwrap()));
        assert_eq!(
            status_and_value("HBOR"),
            (Decoded, Some(DescriptorValue::Real(15.0)))
        );
        assert_eq!(status_and_value("UDTYPE"), (NonImplicit, None));
    }

    /// E3D TTY 定标（2026-09-18）：面板上多的每一行、少的每一行，都由 E3D 自己回答。
    ///
    /// 三个方向都查，一个 E3D 会话问完：
    ///
    /// 1. **我们渲染不成行**的属性，逐个 `Q <名字>`。E3D 印得出真值而我们一行没画，
    ///    才是真读丢了；`ZZE`（E3D 自己也答不上来）与 `NAME` 的现编全名都不算。
    /// 2. **我们按 noun 目录补出来的行**（`origin` 为 `type-default` / `derived`，以及
    ///    非 UDA 的 `unset`），逐个 `Q <名字>`。E3D 印的值要与面板那一格对上（`0mm` 去
    ///    单位是 `0`、`Nulref` 面板拼作 `0/0`）；E3D 答不上来的名字我们也不许补——面板
    ///    对齐的是 E3D 印的那一页，多一行少一行都算偏。
    /// 3. **E3D 自己那一页**（`VAR !attl ATTLIST`）列出的每一个名字，面板都要有对应的
    ///    行。前两个方向都是拿描述符里的名字去问 E3D，问不到「E3D 印了、描述符里压根
    ///    没有」的行；这一条反过来，以 E3D 那一页为准点名。名字对齐走
    ///    [`NounCatalog::query_heading`]（`HEADW` → `HeadWidth`），UDA 去掉前导 `:`。
    ///
    /// 证据：`verification/attr-diagnostics-20260918/`。这条用例要开 E3D 会话，
    /// 与其它 TTY 用例一样单跑：`-- --ignored --test-threads=1`。
    #[test]
    #[ignore = "requires E3D TTY and local AMS source files"]
    fn e3d_tty_says_the_rows_we_cannot_render_are_merely_unset() {
        // 三个 noun 各一个：设计支路、设计构件、站点根。BRAN 那个就是界面截图里
        // 报「34 项诊断」的 `/1WCC1135/B1`。
        let refnos = [
            RefU64::from_two_nums(24383, 66459),
            RefU64::from_two_nums(24384, 23262),
            RefU64::from_two_nums(24384, 46),
        ];
        let (service, _backup_rx) =
            ElementAttributeService::open("AvevaMarineSample", "ALL").unwrap();
        // 面板的短名与 E3D 那一页上的名字对不上（`HEADW` / `HeadWidth`），靠目录换算。
        let noun_catalog = service
            .reader
            .schema
            .noun_catalog()
            .expect("活 E3D 导出的 noun 目录不在，类型默认值本来也补不出来");

        struct Sample {
            refno: String,
            response: ElementAttributesResponse,
            /// 描述符里有、面板上没有的名字。
            unrendered: Vec<String>,
            /// 面板上按类型默认补出来的行：名字 → 那一格的显示串。
            defaulted: Vec<(String, String)>,
        }
        let samples = refnos
            .iter()
            .map(|refno| {
                let snapshot = service.store.element_snapshot(*refno).unwrap();
                let response = service.read(*refno).unwrap();
                let rendered = response
                    .attributes
                    .iter()
                    .map(|attribute| attribute.name.as_str())
                    .collect::<HashSet<_>>();
                let unrendered = snapshot
                    .extraction
                    .attributes
                    .iter()
                    .map(|attribute| attribute.name.trim_start_matches(':'))
                    .filter(|name| !rendered.contains(name))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                // noun 目录补出来的每一行：类型零（`type-default`）、运行时规则
                // （`derived`），以及类型零恰好是 `unset` 的（`unset` 且不是 UDA——UDA 的
                // `unset` 来自 Dictionary，不是这次补的）。
                let defaulted = response
                    .attributes
                    .iter()
                    .filter(|attribute| {
                        matches!(attribute.origin, "type-default" | "derived")
                            || (attribute.origin == "unset" && !attribute.is_uda)
                    })
                    .map(|attribute| (attribute.name.clone(), attribute.display.clone()))
                    .collect::<Vec<_>>();
                assert!(
                    !defaulted.is_empty(),
                    "{refno} 本来就有几十行类型默认值，一行都没补说明 noun 目录没加载"
                );
                Sample {
                    refno: refno.to_slash_string(),
                    response,
                    unrendered,
                    defaulted,
                }
            })
            .collect::<Vec<_>>();
        // 一个会话问完三个元素：开 E3D 是这条用例里最贵的一步。
        let tty_elements = tty_query_attributes(
            &samples
                .iter()
                .map(|sample| {
                    let names = sample
                        .unrendered
                        .iter()
                        .cloned()
                        .chain(sample.defaulted.iter().map(|(name, _)| name.clone()))
                        .collect::<Vec<_>>();
                    (sample.refno.clone(), names)
                })
                .collect::<Vec<_>>(),
        );

        // 三个元素一起报，一次 E3D 会话就把缺的行看全。
        let mut missing_by_element = Vec::new();
        for sample in &samples {
            let Sample {
                refno,
                response,
                unrendered,
                defaulted,
            } = sample;
            let tty = tty_elements
                .get(refno)
                .unwrap_or_else(|| panic!("TTY 没有回答 {refno} 这一段"));
            let answer = |name: &str| tty.answers.get(name).map_or("(未回答)", String::as_str);

            // E3D 那一页上的名字，面板有没有对应的行。没起过名的元素 E3D 那一页上的
            // `Name` 是它现编的全名，面板按 `is_unnamed_placeholder` 不画，这一个例外。
            let panel_keys = response
                .attributes
                .iter()
                .flat_map(|row| tty_names_of(&noun_catalog, row))
                .collect::<HashSet<_>>();
            let unnamed = e3d_answer_is_nothing("NAME", answer("NAME"));
            let missing_from_panel = tty
                .page
                .iter()
                .map(|name| name.trim_start_matches(':').to_ascii_lowercase())
                .filter(|key| !panel_keys.contains(key))
                .filter(|key| !(unnamed && key == "name"))
                .filter(|key| !E3D_PAGE_ROWS_WITHOUT_A_SCHEMA.contains(&key.as_str()))
                .collect::<Vec<_>>();
            missing_by_element.push((refno.clone(), missing_from_panel.clone()));

            let with_a_real_value = unrendered
                .iter()
                .filter(|name| !e3d_answer_is_nothing(name, answer(name)))
                .map(|name| format!("{name}: {}", answer(name)))
                .collect::<Vec<_>>();
            let misaligned = defaulted
                .iter()
                .filter(|(name, display)| !panel_matches_e3d(display, answer(name)))
                .map(|(name, display)| format!("{name}: 面板={display:?} E3D={:?}", answer(name)))
                .collect::<Vec<_>>();

            println!(
                "TTY_PARITY refno={} noun={} 面板行数={} 类型默认行={} 其中与E3D对不上={} | \
                 E3D那一页={} 面板缺的行={} | \
                 画不出的行={} 其中E3D自己也答不上来={} E3D印出真值={} | 新诊断 undecoded={} \
                 shape_conflicts={} uda_issues={} | 不计入完整性 outside_schema={} \
                 view_divergence={} catalog_issues={} | complete={}",
                response.refno,
                response.noun,
                response.attributes.len(),
                defaulted.len(),
                misaligned.len(),
                tty.page.len(),
                missing_from_panel.len(),
                unrendered.len(),
                unrendered.iter().filter(|name| answer(name) == "ZZE").count(),
                with_a_real_value.len(),
                response.diagnostics.undecoded.len(),
                response.diagnostics.shape_conflicts.len(),
                response.diagnostics.uda_issues.len(),
                response.diagnostics.outside_schema.len(),
                response.diagnostics.view_divergence.len(),
                response.diagnostics.catalog_issues.len(),
                response.complete,
            );
            for (name, display) in defaulted {
                println!("TTY_ANSWER {refno}\t{name}\t{}\t面板={display}", answer(name));
            }
            for name in unrendered {
                println!("TTY_ANSWER {refno}\t{name}\t{}\t面板=(不画)", answer(name));
            }

            assert!(
                misaligned.is_empty(),
                "{refno}：这些行面板与 E3D 印的不是一回事（E3D 答不上来的我们也不该补）：\
                 {misaligned:?}"
            );
            assert!(
                with_a_real_value.is_empty(),
                "{refno}：E3D 印得出值、我们却一行都没画——这才是真读丢了，\
                 得留在诊断里：{with_a_real_value:?}"
            );
            assert!(
                response.diagnostics.undecoded.is_empty(),
                "{refno}：E3D 逐条答得上来，诊断里不该再有未解码：{:?}",
                response.diagnostics.undecoded
            );
            assert!(
                response.diagnostics.shape_conflicts.is_empty(),
                "{refno}：形状冲突要的是「文件有值、任何表示都落不下去」，\
                 视图分歧不算：{:?}",
                response.diagnostics.shape_conflicts
            );
            // `uda_issues` 是本元素自己的字典缺口（值留着、只能按哈希名显示），它该
            // 继续把 `complete` 打成 false——那是唯一还允许留下的理由。
            assert_eq!(
                response.complete,
                response.diagnostics.uda_issues.is_empty(),
                "{refno}：除了本元素的 UDA 缺口，不该再有别的理由说它不完整：{:?}",
                response.diagnostics
            );
        }

        let missing = missing_by_element
            .iter()
            .filter(|(_, names)| !names.is_empty())
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "E3D 那一页上有、面板上没有的行（面板对齐的是这一页；读不出来的那几个\
             写在 E3D_PAGE_ROWS_WITHOUT_A_SCHEMA 里，不在其中就是新缺的）：{missing:?}"
        );
    }

    /// E3D 那一页上印、任何一份 schema 导出里都没有的行。
    ///
    /// `Lock` 是元素锁：`attlib` 的 4271 条属性里没有它（`Brlock` / `Rlock` / `Prjlock`
    /// 都在，独独没有 `Lock`），BRAN 的 86 条 noun 目录声明里也没有——它是内核自己答的，
    /// 文件侧没有可解的槽位。三个样本上 E3D 都印 `Lock false`。补不出来就不补：面板
    /// 宁可少这一行，也不画一个猜来的锁。
    #[cfg(test)]
    const E3D_PAGE_ROWS_WITHOUT_A_SCHEMA: [&str; 1] = ["lock"];

    /// 面板那一格与 E3D 印的值是不是同一个东西。
    ///
    /// E3D 印 `<长名> <值>`；值那一截按面板的拼法归一——`Nulref` 面板拼作 `0/0`，带单位
    /// 的数去掉单位（`0mm` / `0pascal` → `0`），两边都是数就按数比（`0` 与 `0.0` 是一个
    /// 数），其余逐字比、不分大小写（`unset` / `false` / 空）。`ZZE`（E3D 答不上来）
    /// 永远不对：E3D 没有这一行，我们也不该有。
    fn panel_matches_e3d(display: &str, answer: &str) -> bool {
        if answer == "ZZE" {
            return false;
        }
        let value = answer
            .split_once(char::is_whitespace)
            .map_or("", |(_, tail)| tail.trim());
        let value = if value.eq_ignore_ascii_case("nulref") {
            "0/0"
        } else {
            value
        };
        let unitless = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if let (Ok(ours), Ok(theirs)) = (display.trim().parse::<f64>(), unitless.parse::<f64>()) {
            return ours == theirs;
        }
        display.trim().eq_ignore_ascii_case(value)
    }

    #[test]
    fn panel_and_e3d_agree_on_type_defaults_however_e3d_spells_them() {
        assert!(panel_matches_e3d("0", "HeadWidth 0mm"));
        assert!(panel_matches_e3d("0", "TPressure 0pascal"));
        assert!(panel_matches_e3d("0", "Bstatus 0"));
        assert!(panel_matches_e3d("false", "Tsfbreak false"));
        assert!(panel_matches_e3d("0/0", "Orrf Nulref"));
        assert!(panel_matches_e3d("unset", "Duty unset"));
        assert!(panel_matches_e3d("", "Inco"));
        assert!(!panel_matches_e3d("0", "HeadWidth 100mm"));
        assert!(!panel_matches_e3d("unset", "Bstatus 0"));
        assert!(!panel_matches_e3d("0", "ZZE"));
    }

    /// E3D 这句回答等于「这儿什么都没有」。
    ///
    /// 它印的是 `<长名> <值>`，值那半截为空、`unset`、`Nulref`、`false`，或者零
    /// （`0` / `0mm` / `0pascal`——REAL 属性没有存过值时 E3D 给的类型默认）时，
    /// 就是没有值。`ZZE` 是 E3D 自己也答不上来，同样不是我们把值读丢了。
    ///
    /// `NAME` 另算：没起过名的元素 E3D 不印空串，而是当场编一个
    /// `FTUBE 4 of BRANCH /C-OR-1R345-C`。真名一律以 `/` 开头，所以不以 `/` 开头的
    /// 就是那个临时拼的全名——它是 E3D 现算的，库里没有这个值，不能拿来证明我们漏读。
    /// （要不要把这个全名也显示出来，是 `FLNM` 伪属性那条线的事，不在本模块。）
    fn e3d_answer_is_nothing(name: &str, answer: &str) -> bool {
        if answer == "ZZE" {
            return true;
        }
        if name == "NAME" {
            return !answer
                .split_once(char::is_whitespace)
                .is_some_and(|(_, value)| value.trim_start().starts_with('/'));
        }
        let value = answer
            .split_once(char::is_whitespace)
            .map_or("", |(_, tail)| tail.trim());
        value.is_empty()
            || value.eq_ignore_ascii_case("unset")
            || value.eq_ignore_ascii_case("nulref")
            || value.eq_ignore_ascii_case("false")
            || value
                .strip_prefix('0')
                .is_some_and(|unit| unit.chars().all(char::is_alphabetic))
    }

    /// 面板一行在 E3D 那一页上可能用的名字：短名本身，以及目录给的查询抬头
    /// （`HEADW` → `HeadWidth`）。UDA 的抬头不走目录哈希，去掉前导 `:` 后按名字对。
    #[cfg(test)]
    fn tty_names_of(catalog: &NounCatalog, row: &AttributeValue) -> Vec<String> {
        let mut names = vec![row.name.to_ascii_lowercase()];
        if !row.is_uda {
            if let Some(heading) = catalog.query_heading(e3d_attlib::db1_hash(&row.name)) {
                names.push(heading.to_ascii_lowercase());
            }
        }
        names
    }

    /// 一个元素在 E3D 那头的两件事。
    #[cfg(test)]
    #[derive(Default)]
    struct TtyElement {
        /// 逐个 `Q <短名>` 的回答：短名 → 原样回答（答不上来记 `ZZE`）。
        answers: std::collections::BTreeMap<String, String>,
        /// E3D 自己那一页（`ATTLIST`）列的名字，按它印的顺序；UDA 带前导 `:`。
        page: Vec<String>,
    }

    /// 一个会话里问完每个元素：点名的属性逐个 `Q`，再要一份 E3D 自己那一页。
    ///
    /// 名字用的是 DB 短名（`DUTY`），E3D 印回来的是它自己的长名加值（`Duty unset`），
    /// 所以回答要连长名一起原样留着；答不上来记 `ZZE`。
    #[cfg(test)]
    fn tty_query_attributes(
        requests: &[(String, Vec<String>)],
    ) -> std::collections::BTreeMap<String, TtyElement> {
        let repo = std::env::current_dir().unwrap();
        let driver = crate::e3d_query::E3dDriver::from_env(&repo).unwrap();
        let mut source = String::new();
        for (refno, names) in requests {
            source.push_str(&format!("$P ZZR {refno}\n={refno}\n"));
            for name in names {
                source.push_str(&format!(
                    "$P ZZA {name}\nQ {name}\nhandle any\n$P ZZE\nelsehandle none\nendhandle\n"
                ));
            }
            source.push_str("VAR !attl ATTLIST\nDO !a VALUES !attl\n$P ZZP $!a\nENDDO\n");
        }
        let raw = driver
            .run_source(&repo, "tty-attribute-probe", &source)
            .unwrap();

        let mut elements: std::collections::BTreeMap<String, TtyElement> =
            std::collections::BTreeMap::new();
        let mut element: Option<String> = None;
        let mut pending: Option<String> = None;
        for line in raw.lines().map(str::trim) {
            if let Some(refno) = line.strip_prefix("ZZR ") {
                element = Some(refno.to_owned());
                pending = None;
                elements.entry(refno.to_owned()).or_default();
                continue;
            }
            if let Some(name) = line.strip_prefix("ZZP ") {
                if let Some(refno) = element.as_ref() {
                    elements
                        .entry(refno.clone())
                        .or_default()
                        .page
                        .push(name.to_owned());
                }
                continue;
            }
            match line.strip_prefix("ZZA ") {
                Some(name) => pending = Some(name.to_owned()),
                None => {
                    if let (Some(refno), Some(name)) = (element.as_ref(), pending.take()) {
                        elements
                            .entry(refno.clone())
                            .or_default()
                            .answers
                            .insert(name, line.to_owned());
                    }
                }
            }
        }
        elements
    }

    /// 共用证明（母计划 §4 不变量）：树服务与属性服务钉住的是**同一份** `Arc<MdbReadStore>`
    /// ——同一路由目录、同一 `DbSet`、同一元素缓存。AMS `/ALL` 实跑：第二个服务首读已开库
    /// 不再开库，同一元素再读缓存不长，全程零扫描。`read_stats` 是进程全局计数器，所以
    /// 以 `-- --ignored --test-threads=1` 单跑，差值才稳。
    #[test]
    #[ignore = "requires local AMS source files"]
    fn ams_tree_and_attribute_services_share_one_read_view() {
        use super::super::direct_tree::DirectTreeService;

        let branch = RefU64::from_two_nums(24384, 23257);
        e3d_io::read_stats::reset();
        let tree = DirectTreeService::open("AvevaMarineSample", "ALL").unwrap();
        let (attributes, _backup_rx) =
            ElementAttributeService::open("AvevaMarineSample", "ALL").unwrap();
        // 字典归 `UdaCatalogProvider`，不在共享视图里：先暖起来，DICT 的开库不混进下面的差值。
        attributes.catalog.get().unwrap();
        let warmed = e3d_io::read_stats::snapshot();

        // 1. 树服务先读：目标库（以及描述符引用到的目录库）在这里打开。
        let chain = tree.ancestors(branch).unwrap();
        assert!(chain.len() > 1, "{chain:?}");
        let after_tree = e3d_io::read_stats::snapshot();
        assert!(
            after_tree.database_opens > warmed.database_opens,
            "the tree read is the one that opens the target database: {}",
            after_tree.summary()
        );
        let cached_after_tree = attributes.store.cached_elements();
        assert!(cached_after_tree > 0);

        // 2. 属性服务首读同一元素：库已由共享视图打开，开库计数不动；元素已缓存，缓存不长。
        let response = attributes.read(branch).unwrap();
        assert_eq!(response.dbnum, tree.dbnum_of(branch).unwrap() as i32);
        let after_attributes = e3d_io::read_stats::snapshot();
        assert_eq!(
            after_attributes.database_opens, after_tree.database_opens,
            "attribute service reopened an already-open database: {}",
            after_attributes.summary()
        );
        assert_eq!(attributes.store.cached_elements(), cached_after_tree);

        // 3. 同一份视图，不是两份长得一样的。
        let tree_view = tree.read_view();
        assert!(Arc::ptr_eq(&attributes.store, &tree_view));
        assert_eq!(attributes.snapshot_epoch, tree_view.epoch);

        // 4. 两个服务各再读一次同一元素：缓存仍不长。
        attributes.read(branch).unwrap();
        tree.ancestors(branch).unwrap();
        assert_eq!(attributes.store.cached_elements(), cached_after_tree);

        // 5. 全程零扫描。
        let stats = e3d_io::read_stats::snapshot();
        assert_eq!(
            (stats.scan_calls, stats.scanned_elements),
            (0, 0),
            "{}",
            stats.summary()
        );
        eprintln!(
            "AMS shared view: ancestors={} epoch={} opened_dbnums={:?} cached_elements={} \
             database_opens(warm/tree/attributes)={}/{}/{} | {}",
            chain.len(),
            tree_view.epoch,
            attributes.store.opened_dbnums(),
            cached_after_tree,
            warmed.database_opens,
            after_tree.database_opens,
            after_attributes.database_opens,
            stats.summary()
        );
    }
}
