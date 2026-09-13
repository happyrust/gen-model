//! 模型生成的进程内投影存储。
//!
//! `e3d-model` 生成出来的几何过去直接拼成 SurrealDB 发布事务写进 `SUL_DB`。现在生成
//! 只写这里：一张按投影域分区的进程内记录表。**落库是另一层适配器
//! （[`crate::fast_model::model_db_adapter`]）的事**——本模块与 `e3d_model_service` 都不认识
//! 数据库，也不渲染任何 SQL。
//!
//! 记录的形状是「原来那条发布事务写出去的东西」的结构化版本：一条 `ProjectedGeometry`
//! 携带原来分散在 `inst_relate` / `tubi_relate` / `inst_info` / `aabb` / `trans` 五张表里的
//! 同一份内容（几何身份、来源 PE、祖先链、世界盒与世界变换、mesh 身份、图元共享信息），
//! 适配器照它拼库行即可，不必回头再算一遍。
//!
//! 三条与旧发布事务逐条对应的语义，这里照搬：
//!
//! - **投影域分区**（`ProjectionScope`）。原来靠 `snapshot_key` 字段与 `hist_` 前缀把历史
//!   快照与当前投影隔在同一批表里；这里换成分区，记录 id 因此不再需要前缀。
//! - **管身分支重置**。一个 BRAN 的隐式管身是整段重写的，原来发布前先
//!   `DELETE pe:⟨branch⟩->tubi_relate`；这里是 `ProjectionCommit::tube_branch_resets`。
//! - **根回执**（`RootReceipt`）。原来是 `gen_root` 行上的 `published_model_target` /
//!   `published_manifest_hash` / `published_geometry_count`，用来在目标没变时跳过整根重算。
//!   `gen_root` 的 revision CAS 没有跟过来：那把锁护的是多个进程抢同一个库行，进程内投影
//!   没有那个竞争。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use e3d_model::primitive_instance::PrimitiveMeshKey;
use glam::DMat4;
use serde::{Deserialize, Serialize};

use crate::fast_model::e3d_model_service::ModelTarget;

/// 一条记录落在哪个投影域里。
///
/// 借用形，跟着调用点走；存储内部换成 [`ScopeKey`] 持有。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionScope<'a> {
    /// 生产投影：Plant UI 当前看到的那一份。
    Current,
    /// 一次按需历史快照，键是 `<refno>@<sesno>`。
    Historical(&'a str),
}

impl ProjectionScope<'_> {
    fn key(self) -> ScopeKey {
        match self {
            Self::Current => ScopeKey::Current,
            Self::Historical(snapshot) => ScopeKey::Historical(snapshot.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ScopeKey {
    Current,
    Historical(String),
}

/// 一条几何记录用的是独享烘焙网格还是共享标准图元。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProjectedShape {
    /// 独享烘焙实体：`mesh_id` 只服务这一个元素，本地盒即世界盒的原像。
    Baked,
    /// 共享标准图元：`mesh_id` 是标准网格，`local_transform` 把它摆进元素本地系。
    Canonical {
        key: PrimitiveMeshKey,
        local_transform: DMat4,
        local_bounds: [[f32; 3]; 2],
    },
}

/// 隐式管身独有的两端与路由序号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTube {
    pub route_ordinal: u32,
    #[serde(with = "refno_wire")]
    pub from: RefNo,
    #[serde(with = "refno_wire")]
    pub to: RefNo,
    pub invalid: bool,
}

/// 一件已投影的几何。`record_id` 在同一个投影域里唯一，就是原来那些库行的记录 id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedGeometry {
    pub record_id: String,
    pub geometry_id: GeometryId,
    /// 几何挂在哪个 PE 元素上（隐式管身挂容器）。
    #[serde(with = "refno_wire")]
    pub source_refno: RefNo,
    pub dbnum: u32,
    pub source_sesno: u32,
    pub noun: String,
    pub mesh_id: String,
    /// 存下来的世界变换。隐式管身只暴露一个变换，标准图元的本地摆放已经折进来了。
    pub world_transform: DMat4,
    pub world_bounds: [[f32; 3]; 2],
    /// 「自身 → 顶层」的打包祖先链，与旧 `anc` 字段逐位相同。
    pub ancestors: Vec<i64>,
    pub shape: ProjectedShape,
    pub tube: Option<ProjectedTube>,
}

impl ProjectedGeometry {
    /// Adapter for older room algorithms; all geometry comes from the same
    /// e3d-model projection as the Task calculator, never from database rows.
    pub(crate) fn room_inst(&self) -> aios_core::GeomInstQuery {
        use bevy_transform::components::Transform;
        let refno = crate::data_interface::generation_root::refno_from_e3d(self.source_refno);
        let local = match &self.shape {
            ProjectedShape::Baked => glam::Mat4::IDENTITY,
            ProjectedShape::Canonical {
                local_transform, ..
            } => local_transform.as_mat4(),
        };
        aios_core::GeomInstQuery {
            refno,
            old_refno: None,
            owner: self
                .ancestors
                .get(1)
                .map(|id| aios_core::RefU64(*id as u64).into())
                .unwrap_or(refno),
            world_aabb: parry3d::bounding_volume::Aabb::new(
                self.world_bounds[0].into(),
                self.world_bounds[1].into(),
            ),
            world_trans: Transform::from_matrix(self.world_transform.as_mat4()),
            insts: vec![aios_core::ModelHashInst {
                geo_hash: self.mesh_id.clone(),
                transform: Transform::from_matrix(local),
                ..Default::default()
            }],
            has_neg: self.tube.is_none() && matches!(&self.shape, ProjectedShape::Baked),
            generic: self.noun.clone(),
            pts: None,
            date: None,
        }
    }
}

/// 一个生成根上一次投影留下的回执，用来判断「目标没变，不必重算」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootReceipt {
    pub source_sesno: u32,
    pub model_target: Option<ModelTarget>,
    pub manifest_hash: String,
    pub geometry_count: usize,
    /// 这一根这轮是怎么发布的（ADR-066，观测用）：`"regen"` 整根重算 / `"transform"` 刚体前移便宜路。
    /// 旧回执没有这一列 → `"regen"`。落 `gen_root.published_via` 供观测，不进任何判据。
    #[serde(default = "default_published_via")]
    pub published_via: String,
}

/// 当前进程里一根“模型行 + 房间边 + 数据库回执”已经完整提交的确认。
///
/// 它故意不落 durable 控制面：kv-mem 重启后模型、边和 ACK 一起消失。几何 [`RootReceipt`]
/// 仍表示候选投影；只有 process-store 事务成功后才补这枚 ACK。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPublicationAck {
    pub dbnum: u32,
    pub source_sesno: u32,
    pub geometry_count: usize,
    /// Durable content identity. `sesno` alone is insufficient when a
    /// generator/catalogue change republishes a root at the same source session.
    pub manifest_hash: String,
    pub model_target: Option<ModelTarget>,
    pub room_membership: bool,
    pub room_generation: Option<String>,
    pub medium: crate::fast_model::model_db_adapter::PersistMedium,
}

impl RootPublicationAck {
    /// Exact candidate/durable equality. `published_via` is deliberately not
    /// part of the identity: it is observability metadata, not model content.
    pub fn matches(&self, receipt: &RootReceipt) -> bool {
        self.source_sesno == receipt.source_sesno
            && self.geometry_count == receipt.geometry_count
            && self.manifest_hash == receipt.manifest_hash
            && self.model_target == receipt.model_target
    }
}

/// `RootReceipt::published_via` 的默认值：旧回执与整根重算都是 `"regen"`。
pub fn default_published_via() -> String {
    "regen".to_string()
}

/// 一根（或一次增量）要落进存储的全部改动。攒齐一批再一次提交，一批就是一个原子边界。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectionCommit {
    /// 这些 BRAN 的隐式管身整段重写：先清光旧段再插新的。
    #[serde(with = "refno_vec_wire")]
    pub tube_branch_resets: Vec<RefNo>,
    /// 要删掉的记录 id。
    pub removals: Vec<String>,
    pub upserts: Vec<ProjectedGeometry>,
    /// 本次投影给某个生成根盖的章。
    #[serde(with = "receipt_wire")]
    pub receipt: Option<(RefNo, RootReceipt)>,
}

mod refno_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(r: &RefNo, s: S) -> Result<S::Ok, S::Error> {
        (r.word0, r.word1).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<RefNo, D::Error> {
        let (a, b) = <(u32, u32)>::deserialize(d)?;
        Ok(RefNo::new(a, b))
    }
}
mod refno_vec_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(rs: &[RefNo], s: S) -> Result<S::Ok, S::Error> {
        rs.iter()
            .map(|r| (r.word0, r.word1))
            .collect::<Vec<_>>()
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<RefNo>, D::Error> {
        Ok(Vec::<(u32, u32)>::deserialize(d)?
            .into_iter()
            .map(|(a, b)| RefNo::new(a, b))
            .collect())
    }
}
mod receipt_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        r: &Option<(RefNo, RootReceipt)>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        r.as_ref()
            .map(|(r, c)| ((r.word0, r.word1), c))
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<(RefNo, RootReceipt)>, D::Error> {
        Ok(Option::<((u32, u32), RootReceipt)>::deserialize(d)?
            .map(|((a, b), r)| (RefNo::new(a, b), r)))
    }
}

impl ProjectionCommit {
    pub fn is_empty(&self) -> bool {
        self.tube_branch_resets.is_empty()
            && self.removals.is_empty()
            && self.upserts.is_empty()
            && self.receipt.is_none()
    }
}

// BTree nodes, the packed root key and the receipt allocation all remain
// resident even when a generated root contains no geometry.  This deliberately
// stable estimate is an admission-control unit, not an allocator measurement.
const ROOT_RECEIPT_OVERHEAD_BYTES: usize = 128;

fn receipt_resident_bytes(root: RefNo, receipt: &RootReceipt) -> usize {
    ROOT_RECEIPT_OVERHEAD_BYTES
        .saturating_add(std::mem::size_of_val(&root))
        .saturating_add(serde_json::to_vec(receipt).map_or(0, |encoded| encoded.len()))
}

/// 把 refno 打包成旧 `anc` 数组里那个 i64。
pub fn packed_refno(refno: RefNo) -> i64 {
    (((refno.word0 as u64) << 32) | refno.word1 as u64) as i64
}

#[derive(Default)]
struct ScopeState {
    /// record_id → 记录。
    geometries: BTreeMap<String, ProjectedGeometry>,
    /// Approximate serialized bytes of each resident geometry and their total.
    geometry_bytes: BTreeMap<String, usize>,
    resident_bytes: usize,
    /// 打包祖先 → 挂在它下面的 record_id。旧 `anc CONTAINS <packed>` 的索引形态。
    by_ancestor: BTreeMap<i64, BTreeSet<String>>,
    /// 管段来源 → 管身记录。重置管段不应遍历其它根的全部几何。
    by_tube_branch: BTreeMap<i64, BTreeSet<String>>,
    /// 生成根 → 回执。
    receipts: BTreeMap<i64, RootReceipt>,
    /// Roots remain discoverable after failed refresh, including empty roots.
    invalidated_roots: BTreeSet<i64>,
    /// 生成根 → 当前进程的完整发布确认。任何候选几何变动都先使它失效。
    publication_acks: BTreeMap<i64, RootPublicationAck>,
    /// 每个库投影过的最高源会话。单调，只涨不落——它护的是「旧会话不许盖新会话」。
    published_sesno: BTreeMap<u32, u32>,
    /// 每个库的提交纪元：本作用域里凡动了该库的几何或回执就 +1。
    /// [`ModelMemoryStore::restore_dbnum`] 靠它判断「读库那几秒里有没有人提交过」。
    commit_epochs: BTreeMap<u32, u64>,
    by_source: BTreeMap<(u32, i64), BTreeSet<String>>,
    room_changes: BTreeMap<u32, BTreeMap<i64, GeometryChange>>,
    room_baselines: BTreeSet<u32>,
    model_digests: BTreeMap<u32, [u8; 32]>,
    room_spatial: BTreeMap<u32, aios_core::accel_tree::acceleration_tree::AccelerationTree>,
    room_spatial_dirty: BTreeSet<(u32, i64)>,
    room_spatial_rebuild_count: u64,
    room_spatial_rows_rebuilt: u64,
    room_spatial_rebuild_duration_ms: u64,
}

impl std::fmt::Debug for ScopeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopeState")
            .field("geometries", &self.geometries.len())
            .field("receipts", &self.receipts.len())
            .field("publication_acks", &self.publication_acks.len())
            .field("commit_epochs", &self.commit_epochs)
            .finish_non_exhaustive()
    }
}

/// Coalesced from the last room ACK through the current projection. A deletion
/// keeps its old bounds even when the last geometry of the member disappears.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GeometryChange {
    pub before: Vec<[[f32; 3]; 2]>,
    pub after: Vec<[[f32; 3]; 2]>,
}

impl ScopeState {
    fn total_resident_bytes(&self) -> usize {
        self.resident_bytes.saturating_add(
            self.receipts
                .iter()
                .map(|(packed, receipt)| {
                    let root = RefNo::new((*packed >> 32) as u32, *packed as u32);
                    receipt_resident_bytes(root, receipt)
                })
                .sum::<usize>(),
        )
    }

    fn flush_room_spatial(&mut self) {
        let dirty = std::mem::take(&mut self.room_spatial_dirty);
        let dbnums = dirty
            .into_iter()
            .map(|(dbnum, _)| dbnum)
            .collect::<BTreeSet<_>>();
        for dbnum in dbnums {
            use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
            let started_at = Instant::now();
            let boxes = self
                .by_source
                .range((dbnum, i64::MIN)..=(dbnum, i64::MAX))
                .flat_map(|(_, ids)| ids)
                .filter_map(|id| self.geometries.get(id))
                .map(|row| {
                    RStarBoundingBox::new(
                        parry3d::bounding_volume::Aabb::new(
                            row.world_bounds[0].into(),
                            row.world_bounds[1].into(),
                        ),
                        crate::data_interface::generation_root::refno_from_e3d(row.source_refno),
                        row.noun.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let row_count = boxes.len() as u64;
            self.room_spatial
                .insert(dbnum, AccelerationTree::load(boxes));
            self.room_spatial_rebuild_count = self.room_spatial_rebuild_count.saturating_add(1);
            self.room_spatial_rows_rebuilt = self
                .room_spatial_rows_rebuilt
                .saturating_add(row_count);
            self.room_spatial_rebuild_duration_ms = self
                .room_spatial_rebuild_duration_ms
                .saturating_add(started_at.elapsed().as_millis() as u64);
        }
    }
    fn toggle_digest(&mut self, record: &ProjectedGeometry) {
        use sha2::{Digest, Sha256};
        // Include the unique record identity, so equal geometry cannot cancel.
        let bytes = serde_json::to_vec(record).expect("projection is serializable");
        let hash = Sha256::digest(bytes);
        let digest = self.model_digests.entry(record.dbnum).or_default();
        for (target, byte) in digest.iter_mut().zip(hash) {
            *target ^= byte;
        }
    }

    fn observe_member(&mut self, record: &ProjectedGeometry) {
        let member = packed_refno(record.source_refno);
        if self
            .room_changes
            .get(&record.dbnum)
            .is_some_and(|changes| changes.contains_key(&member))
        {
            return;
        }
        let before = self
            .by_source
            .get(&(record.dbnum, member))
            .into_iter()
            .flatten()
            .filter_map(|id| self.geometries.get(id))
            .map(|row| row.world_bounds)
            .collect();
        self.room_changes.entry(record.dbnum).or_default().insert(
            member,
            GeometryChange {
                before,
                after: Vec::new(),
            },
        );
    }
    fn bump_epoch(&mut self, dbnum: u32) {
        *self.commit_epochs.entry(dbnum).or_default() += 1;
    }

    fn invalidate_publication_acks_for(&mut self, commit: &ProjectionCommit) {
        let mut touched = BTreeSet::new();
        if let Some((root, _)) = &commit.receipt {
            touched.insert(packed_refno(*root));
        }
        for record in &commit.upserts {
            touched.extend(record.ancestors.iter().copied());
        }
        for record_id in &commit.removals {
            if let Some(record) = self.geometries.get(record_id) {
                touched.extend(record.ancestors.iter().copied());
            }
        }
        for branch in &commit.tube_branch_resets {
            if let Some(record_ids) = self.by_tube_branch.get(&packed_refno(*branch)) {
                for record_id in record_ids {
                    if let Some(record) = self.geometries.get(record_id) {
                        touched.extend(record.ancestors.iter().copied());
                    }
                }
            }
        }
        for root in touched {
            self.publication_acks.remove(&root);
        }
    }

    fn insert(&mut self, record: ProjectedGeometry) {
        let dbnum = record.dbnum;
        self.observe_member(&record);
        self.remove(&record.record_id);
        self.toggle_digest(&record);
        self.by_source
            .entry((dbnum, packed_refno(record.source_refno)))
            .or_default()
            .insert(record.record_id.clone());
        for &ancestor in &record.ancestors {
            self.by_ancestor
                .entry(ancestor)
                .or_default()
                .insert(record.record_id.clone());
        }
        if record.tube.is_some() {
            self.by_tube_branch
                .entry(packed_refno(record.source_refno))
                .or_default()
                .insert(record.record_id.clone());
        }
        let sesno = self
            .published_sesno
            .entry(record.dbnum)
            .or_insert(record.source_sesno);
        *sesno = (*sesno).max(record.source_sesno);
        let member = packed_refno(record.source_refno);
        let record_id = record.record_id.clone();
        let bytes = serde_json::to_vec(&record).map_or(0, |encoded| encoded.len());
        self.geometries.insert(record_id.clone(), record);
        self.geometry_bytes.insert(record_id, bytes);
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        self.room_spatial_dirty.insert((dbnum, member));
        self.bump_epoch(dbnum);
    }

    fn remove(&mut self, record_id: &str) -> Option<ProjectedGeometry> {
        self.remove_internal(record_id, true)
    }

    /// Remove cache residency without turning it into a model delta. Indexes and
    /// the resident spatial view still change, but room publication must not see
    /// cache pressure as a source-model deletion.
    fn evict(&mut self, record_id: &str) -> Option<ProjectedGeometry> {
        self.remove_internal(record_id, false)
    }

    fn remove_internal(
        &mut self,
        record_id: &str,
        observe_change: bool,
    ) -> Option<ProjectedGeometry> {
        let previous = self.geometries.get(record_id)?.clone();
        if observe_change {
            self.observe_member(&previous);
        }
        let record = self.geometries.remove(record_id)?;
        if let Some(bytes) = self.geometry_bytes.remove(record_id) {
            self.resident_bytes = self.resident_bytes.saturating_sub(bytes);
        }
        self.toggle_digest(&record);
        let key = (record.dbnum, packed_refno(record.source_refno));
        if let Some(ids) = self.by_source.get_mut(&key) {
            ids.remove(record_id);
            if ids.is_empty() {
                self.by_source.remove(&key);
            }
        }
        self.bump_epoch(record.dbnum);
        for ancestor in &record.ancestors {
            if let Some(bucket) = self.by_ancestor.get_mut(ancestor) {
                bucket.remove(record_id);
                if bucket.is_empty() {
                    self.by_ancestor.remove(ancestor);
                }
            }
        }
        if record.tube.is_some() {
            let key = packed_refno(record.source_refno);
            if let Some(bucket) = self.by_tube_branch.get_mut(&key) {
                bucket.remove(record_id);
                if bucket.is_empty() {
                    self.by_tube_branch.remove(&key);
                }
            }
        }
        self.room_spatial_dirty
            .insert((record.dbnum, packed_refno(record.source_refno)));
        Some(record)
    }

    fn record_ids_under(&self, root: RefNo) -> Vec<String> {
        self.by_ancestor
            .get(&packed_refno(root))
            .map(|bucket| bucket.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn apply_removals(&mut self, commit: &ProjectionCommit) {
        for branch in &commit.tube_branch_resets {
            let stale = self
                .by_tube_branch
                .remove(&packed_refno(*branch))
                .unwrap_or_default();
            for record_id in stale {
                self.remove(&record_id);
            }
        }
        for record_id in &commit.removals {
            self.remove(record_id);
        }
    }

    fn apply_upserts_and_receipt(&mut self, commit: ProjectionCommit) {
        for record in commit.upserts {
            self.insert(record);
        }
        if let Some((root, receipt)) = commit.receipt {
            let sesno = self
                .published_sesno
                .entry(root.dbno())
                .or_insert(receipt.source_sesno);
            *sesno = (*sesno).max(receipt.source_sesno);
            self.invalidated_roots.remove(&packed_refno(root));
            self.receipts.insert(packed_refno(root), receipt);
            self.bump_epoch(root.dbno());
        }
    }

    #[cfg(test)]
    fn apply(&mut self, commit: ProjectionCommit) {
        self.apply_removals(&commit);
        self.apply_upserts_and_receipt(commit);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCacheStats {
    pub mode: &'static str,
    pub max_bytes: Option<usize>,
    pub resident_bytes: usize,
    pub resident_roots: usize,
    pub hits: u64,
    pub misses: u64,
    pub loads: u64,
    pub load_failures: u64,
    pub evictions: u64,
    pub oversize_bypasses: u64,
    pub inflight: usize,
    /// Memory candidates whose exact content identity is not durable yet.
    pub pending_roots: usize,
    /// Resident roots whose exact candidate identity is confirmed by RocksDB.
    pub durable_roots: usize,
    /// Number of per-dbnum overlay spatial bulk rebuilds in this process.
    pub room_spatial_rebuild_count: u64,
    /// Cumulative resident rows fed into those bulk rebuilds.
    pub room_spatial_rows_rebuilt: u64,
    /// Cumulative foreground time spent collecting and rebuilding, in milliseconds.
    pub room_spatial_rebuild_duration_ms: u64,
}

/// The current overlay cannot accept another atomic projection cohort without
/// exceeding its configured hard memory limit.  No part of the rejected cohort
/// has been installed when this error is returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayBackpressure {
    pub projected_bytes: usize,
    pub max_bytes: usize,
}

impl std::fmt::Display for OverlayBackpressure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "model overlay backpressure: projected {} bytes exceeds hard limit {} bytes",
            self.projected_bytes, self.max_bytes
        )
    }
}

impl std::error::Error for OverlayBackpressure {}

#[derive(Debug, Default)]
struct CacheControl {
    clock: u64,
    last_access: BTreeMap<i64, u64>,
    hits: u64,
    misses: u64,
    loads: u64,
    load_failures: u64,
    evictions: u64,
    oversize_bypasses: u64,
    inflight: usize,
}

/// 进程内的模型投影存储。生成侧只写它，读侧（历史快照查询、按需删除、落库适配器）
/// 只读它。持久模式下 `current_max_bytes` 使它成为按根淘汰的有界增量 overlay；未收到
/// durable ACK 的候选根不会被淘汰，确保失败后仍可重试发布。
#[derive(Debug)]
pub struct ModelMemoryStore {
    scopes: RwLock<HashMap<ScopeKey, ScopeState>>,
    current_max_bytes: Option<usize>,
    cache: Mutex<CacheControl>,
}

impl Default for ModelMemoryStore {
    fn default() -> Self {
        Self {
            scopes: RwLock::new(HashMap::new()),
            current_max_bytes: None,
            cache: Mutex::new(CacheControl::default()),
        }
    }
}

static STORE: OnceLock<ModelMemoryStore> = OnceLock::new();

impl ModelMemoryStore {
    /// A restored projection has no complete deletion history. Until an exact
    /// room ACK, compare against the complete local membership baseline.
    pub(crate) fn room_baseline_known(&self, dbnum: u32) -> bool {
        self.read()
            .get(&ScopeKey::Current)
            .is_some_and(|state| state.room_baselines.contains(&dbnum))
    }
    pub(crate) fn room_members_intersecting(
        &self,
        dbnum: u32,
        bounds: &[[[f32; 3]; 2]],
    ) -> BTreeSet<i64> {
        let scopes = self.read();
        let Some(tree) = scopes
            .get(&ScopeKey::Current)
            .and_then(|state| state.room_spatial.get(&dbnum))
        else {
            return BTreeSet::new();
        };
        bounds
            .iter()
            .flat_map(|bounds| {
                tree.locate_intersecting_bounds(&parry3d::bounding_volume::Aabb::new(
                    bounds[0].into(),
                    bounds[1].into(),
                ))
                .map(|row| row.refno.0 as i64)
                .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn room_member_records(
        &self,
        dbnum: u32,
        members: &BTreeSet<i64>,
    ) -> Vec<ProjectedGeometry> {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return Vec::new();
        };
        members
            .iter()
            .flat_map(|member| state.by_source.get(&(dbnum, *member)).into_iter().flatten())
            .filter_map(|id| state.geometries.get(id).cloned())
            .collect()
    }
    /// Ref0 is not a dbnum. Resolve requested sources against the resident
    /// per-db indexes rather than guessing ownership or querying database geometry.
    pub(crate) fn room_records_for_members(
        &self,
        members: &BTreeSet<i64>,
    ) -> Vec<ProjectedGeometry> {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return Vec::new();
        };
        state
            .commit_epochs
            .keys()
            .flat_map(|dbnum| {
                members.iter().flat_map(|member| {
                    state
                        .by_source
                        .get(&(*dbnum, *member))
                        .into_iter()
                        .flatten()
                })
            })
            .filter_map(|id| state.geometries.get(id).cloned())
            .collect()
    }
    pub(crate) fn room_snapshot(
        &self,
        dbnum: u32,
        full: bool,
    ) -> (
        u64,
        String,
        BTreeMap<i64, GeometryChange>,
        Vec<ProjectedGeometry>,
    ) {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return (0, hex::encode([0u8; 32]), BTreeMap::new(), Vec::new());
        };
        let mut changes = state.room_changes.get(&dbnum).cloned().unwrap_or_default();
        if full {
            for ((db, member), _) in state.by_source.range((dbnum, i64::MIN)..=(dbnum, i64::MAX)) {
                if *db == dbnum {
                    changes.entry(*member).or_default();
                }
            }
        }
        let mut records = Vec::new();
        for (member, change) in &mut changes {
            let rows = state
                .by_source
                .get(&(dbnum, *member))
                .into_iter()
                .flatten()
                .filter_map(|id| state.geometries.get(id));
            for row in rows {
                change.after.push(row.world_bounds);
                records.push(row.clone());
            }
        }
        (
            state.commit_epochs.get(&dbnum).copied().unwrap_or(0),
            hex::encode(state.model_digests.get(&dbnum).copied().unwrap_or_default()),
            changes,
            records,
        )
    }

    pub(crate) fn ack_rooms(&self, dbnum: u32, epoch: u64) -> bool {
        let mut scopes = self.write();
        let Some(state) = scopes.get_mut(&ScopeKey::Current) else {
            return epoch == 0;
        };
        if state.commit_epochs.get(&dbnum).copied().unwrap_or(0) != epoch {
            return false;
        }
        state.room_changes.remove(&dbnum);
        state.room_baselines.insert(dbnum);
        true
    }
    pub fn global() -> &'static Self {
        STORE.get_or_init(|| {
            if crate::options::store_mode().is_durable() {
                Self::with_cache_limit(crate::options::model_cache_max_bytes())
            } else {
                Self::default()
            }
        })
    }

    pub fn with_cache_limit(max_bytes: usize) -> Self {
        Self {
            scopes: RwLock::new(HashMap::new()),
            current_max_bytes: Some(max_bytes.max(1)),
            cache: Mutex::new(CacheControl::default()),
        }
    }

    fn cache_control(&self) -> std::sync::MutexGuard<'_, CacheControl> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn touch_root(&self, root: RefNo) {
        if self.current_max_bytes.is_none() {
            return;
        }
        // Keep the resident check and access update ordered with eviction.
        let scopes = self.read();
        if !scopes.get(&ScopeKey::Current).is_some_and(|state| {
            state.receipts.contains_key(&packed_refno(root))
        }) {
            return;
        }
        let mut cache = self.cache_control();
        cache.clock = cache.clock.wrapping_add(1);
        let clock = cache.clock;
        cache.last_access.insert(packed_refno(root), clock);
    }

    pub(crate) fn note_cache_lookup(&self, hit: bool) {
        let mut cache = self.cache_control();
        if hit {
            cache.hits += 1;
        } else {
            cache.misses += 1;
        }
    }

    pub(crate) fn note_cache_load_started(&self) {
        let mut cache = self.cache_control();
        cache.inflight += 1;
    }

    pub(crate) fn note_cache_load_finished(&self, success: bool) {
        let mut cache = self.cache_control();
        cache.inflight = cache.inflight.saturating_sub(1);
        if success {
            cache.loads += 1;
        } else {
            cache.load_failures += 1;
        }
    }

    pub fn cache_stats(&self) -> ModelCacheStats {
        let scopes = self.read();
        let current = scopes.get(&ScopeKey::Current);
        let cache = self.cache_control();
        let (pending_roots, durable_roots) = current.map_or((0, 0), |state| {
            state
                .receipts
                .iter()
                .fold((0, 0), |(pending, durable), (root, receipt)| {
                    let caught_up = state
                        .publication_acks
                        .get(root)
                        .is_some_and(|ack| ack.matches(receipt));
                    if caught_up {
                        (pending, durable + 1)
                    } else {
                        (pending + 1, durable)
                    }
                })
        });
        ModelCacheStats {
            mode: crate::options::model_cache_mode().as_str(),
            max_bytes: self.current_max_bytes,
            resident_bytes: current.map_or(0, ScopeState::total_resident_bytes),
            resident_roots: current.map_or(0, |state| state.receipts.len()),
            hits: cache.hits,
            misses: cache.misses,
            loads: cache.loads,
            load_failures: cache.load_failures,
            evictions: cache.evictions,
            oversize_bypasses: cache.oversize_bypasses,
            inflight: cache.inflight,
            pending_roots,
            durable_roots,
            room_spatial_rebuild_count: current.map_or(0, |state| {
                state.room_spatial_rebuild_count
            }),
            room_spatial_rows_rebuilt: current
                .map_or(0, |state| state.room_spatial_rows_rebuilt),
            room_spatial_rebuild_duration_ms: current.map_or(0, |state| {
                state.room_spatial_rebuild_duration_ms
            }),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, HashMap<ScopeKey, ScopeState>> {
        self.scopes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<ScopeKey, ScopeState>> {
        self.scopes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 一次提交一批改动。整批在同一把写锁下落地——一个 cohort 是一个原子边界，
    /// 与旧发布事务的 `BEGIN … COMMIT` 同一条纪律。
    pub fn commit(&self, scope: ProjectionScope<'_>, commits: Vec<ProjectionCommit>) {
        if commits.iter().all(ProjectionCommit::is_empty) {
            return;
        }
        let mut scopes = self.write();
        let state = scopes.entry(scope.key()).or_default();
        // All roots describe one target snapshot. During reparenting the old
        // root removes the same identity that the new root upserts. Retire the
        // prior snapshot first, independent of root completion/order.
        for commit in &commits {
            state.invalidate_publication_acks_for(commit);
            state.apply_removals(commit);
        }
        for commit in commits {
            state.apply_upserts_and_receipt(commit);
        }
        state.flush_room_spatial();
    }

    /// Atomically admit and commit a current-overlay cohort. Exact durable roots
    /// may be evicted first; pending roots are never eviction candidates.
    pub fn try_commit_current(
        &self,
        commits: Vec<ProjectionCommit>,
    ) -> Result<(), OverlayBackpressure> {
        let Some(max_bytes) = self.current_max_bytes else {
            self.commit(ProjectionScope::Current, commits);
            return Ok(());
        };
        if commits.iter().all(ProjectionCommit::is_empty) {
            return Ok(());
        }

        let protected = commits
            .iter()
            .filter_map(|commit| commit.receipt.as_ref().map(|(root, _)| packed_refno(*root)))
            .collect::<BTreeSet<_>>();
        let mut scopes = self.write();
        let state = scopes.entry(ScopeKey::Current).or_default();
        let mut cache = self.cache_control();

        loop {
            let mut replaced = BTreeSet::new();
            for commit in &commits {
                replaced.extend(commit.removals.iter().cloned());
                for branch in &commit.tube_branch_resets {
                    if let Some(ids) = state.by_tube_branch.get(&packed_refno(*branch)) {
                        replaced.extend(ids.iter().cloned());
                    }
                }
                replaced.extend(commit.upserts.iter().map(|row| row.record_id.clone()));
            }
            let removed_bytes = replaced
                .iter()
                .filter_map(|id| state.geometry_bytes.get(id))
                .sum::<usize>();
            // A cohort has final-write semantics.  Counting every intermediate
            // upsert can reject a batch whose installed state is within budget.
            let final_upserts = commits
                .iter()
                .flat_map(|commit| &commit.upserts)
                .map(|row| (row.record_id.as_str(), row))
                .collect::<BTreeMap<_, _>>();
            let added_bytes = final_upserts
                .values()
                .map(|row| serde_json::to_vec(row).map_or(0, |bytes| bytes.len()))
                .sum::<usize>();
            let final_receipts = commits
                .iter()
                .filter_map(|commit| commit.receipt.as_ref())
                .map(|(root, receipt)| (packed_refno(*root), (*root, receipt)))
                .collect::<BTreeMap<_, _>>();
            let removed_receipt_bytes = final_receipts
                .keys()
                .filter_map(|root| {
                    state.receipts.get(root).map(|receipt| {
                        receipt_resident_bytes(
                            RefNo::new((*root >> 32) as u32, *root as u32),
                            receipt,
                        )
                    })
                })
                .sum::<usize>();
            let added_receipt_bytes = final_receipts
                .values()
                .map(|(root, receipt)| receipt_resident_bytes(*root, receipt))
                .sum::<usize>();
            let projected_bytes = state
                .total_resident_bytes()
                .saturating_sub(removed_bytes)
                .saturating_sub(removed_receipt_bytes)
                .saturating_add(added_bytes)
                .saturating_add(added_receipt_bytes);
            if projected_bytes <= max_bytes {
                break;
            }

            let candidate = state
                .publication_acks
                .iter()
                .filter(|(root, ack)| {
                    !protected.contains(root)
                        && state
                            .receipts
                            .get(root)
                            .is_some_and(|receipt| ack.matches(receipt))
                })
                .map(|(root, _)| *root)
                .min_by_key(|root| cache.last_access.get(root).copied().unwrap_or(0));
            let Some(candidate) = candidate else {
                cache.oversize_bypasses += 1;
                return Err(OverlayBackpressure {
                    projected_bytes,
                    max_bytes,
                });
            };

            state.publication_acks.remove(&candidate);
            state.receipts.remove(&candidate);
            state.invalidated_roots.remove(&candidate);
            cache.last_access.remove(&candidate);
            let root = RefNo::new((candidate >> 32) as u32, candidate as u32);
            for id in state.record_ids_under(root) {
                let shared = state.geometries.get(&id).is_some_and(|record| {
                    state
                        .receipts
                        .keys()
                        .any(|other| record.ancestors.contains(other))
                });
                if !shared {
                    state.evict(&id);
                }
            }
            cache.evictions += 1;
        }

        for commit in &commits {
            state.invalidate_publication_acks_for(commit);
            state.apply_removals(commit);
        }
        for commit in commits {
            state.apply_upserts_and_receipt(commit);
        }
        state.flush_room_spatial();
        Ok(())
    }

    pub fn current_counts(&self) -> (usize, usize) {
        self.read()
            .get(&ProjectionScope::Current.key())
            .map(|state| (state.geometries.len(), state.receipts.len()))
            .unwrap_or_default()
    }

    /// 当前投影里某个库的提交纪元，配合 [`Self::restore_dbnum`] 用。
    pub fn commit_epoch(&self, dbnum: u32) -> u64 {
        self.read()
            .get(&ProjectionScope::Current.key())
            .and_then(|state| state.commit_epochs.get(&dbnum).copied())
            .unwrap_or(0)
    }

    /// Restore a complete published database image without executing generation.
    /// The replacement is atomic to all readers of the process projection.
    ///
    /// `expected_epoch` 是读库之前 [`Self::commit_epoch`] 的取值。读库要翻很多页、页与页
    /// 之间全是 await，而「只写内存」的按需生成同时还在往同一个库提交（`model_read_route`
    /// R2）。纪元变了就说明这份镜像已经旧了：**不换**，返回 `false` 让调用方重读——照换
    /// 的话那些只在内存里的记录会被无声抹掉，库里也没有它们。
    #[must_use = "换不进去要重读镜像，不能当成已恢复"]
    #[cfg(test)]
    pub(crate) fn restore_dbnum(
        &self,
        dbnum: u32,
        records: Vec<ProjectedGeometry>,
        receipts: Vec<(RefNo, RootReceipt)>,
        expected_epoch: u64,
    ) -> bool {
        let mut scopes = self.write();
        let state = scopes.entry(ProjectionScope::Current.key()).or_default();
        if state.commit_epochs.get(&dbnum).copied().unwrap_or(0) != expected_epoch {
            return false;
        }
        state.room_baselines.remove(&dbnum);
        let old = state
            .geometries
            .values()
            .filter(|record| record.dbnum == dbnum)
            .map(|record| record.record_id.clone())
            .collect::<Vec<_>>();
        for id in old {
            state.remove(&id);
        }
        state.receipts.retain(|_, receipt| {
            receipt
                .model_target
                .as_ref()
                .is_none_or(|target| target.design.dbnum != dbnum)
        });
        state
            .invalidated_roots
            .retain(|packed| RefNo::new((*packed >> 32) as u32, *packed as u32).dbno() != dbnum);
        state.publication_acks.retain(|_, ack| ack.dbnum != dbnum);
        state.published_sesno.remove(&dbnum);
        for record in records {
            state.insert(record);
        }
        for (root, receipt) in receipts {
            let session = state.published_sesno.entry(dbnum).or_default();
            *session = (*session).max(receipt.source_sesno);
            state.invalidated_roots.remove(&packed_refno(root));
            state.receipts.insert(packed_refno(root), receipt);
        }
        state.flush_room_spatial();
        true
    }

    /// Install one complete durable root without replacing any other resident root.
    /// The caller must have validated the receipt/count and must pass the dbnum epoch
    /// observed before its database read. A concurrent generation commit wins and
    /// makes this load retry instead of allowing an older disk image to overwrite it.
    #[must_use = "a stale root load must be retried"]
    #[cfg(test)]
    fn restore_root(
        &self,
        root: RefNo,
        records: Vec<ProjectedGeometry>,
        receipt: RootReceipt,
        expected_epoch: u64,
    ) -> bool {
        let dbnum = root.dbno();
        let packed = packed_refno(root);
        let mut scopes = self.write();
        let state = scopes.entry(ScopeKey::Current).or_default();
        if state.commit_epochs.get(&dbnum).copied().unwrap_or(0) != expected_epoch {
            return false;
        }

        let loaded_ids = records
            .iter()
            .map(|record| record.record_id.clone())
            .collect::<BTreeSet<_>>();
        let stale = state
            .record_ids_under(root)
            .into_iter()
            .filter(|id| !loaded_ids.contains(id))
            .collect::<Vec<_>>();
        for id in stale {
            let shared = state.geometries.get(&id).is_some_and(|record| {
                state
                    .receipts
                    .keys()
                    .any(|other| *other != packed && record.ancestors.contains(other))
            });
            if !shared {
                state.evict(&id);
            }
        }
        for record in records {
            state.insert(record);
        }
        state.room_baselines.remove(&dbnum);
        state.invalidated_roots.remove(&packed);
        state.receipts.insert(packed, receipt);
        state.flush_room_spatial();
        drop(scopes);
        self.touch_root(root);
        true
    }

    fn enforce_current_cache_limit(&self, protected: RefNo) {
        let Some(max_bytes) = self.current_max_bytes else {
            return;
        };
        let protected = packed_refno(protected);
        let mut scopes = self.write();
        let Some(state) = scopes.get_mut(&ScopeKey::Current) else {
            return;
        };
        let mut cache = self.cache_control();
        let protected_bytes = state
            .record_ids_under(RefNo::new((protected >> 32) as u32, protected as u32))
            .into_iter()
            .filter_map(|id| state.geometry_bytes.get(&id))
            .sum::<usize>()
            .saturating_add(state.receipts.get(&protected).map_or(0, |receipt| {
                receipt_resident_bytes(
                    RefNo::new((protected >> 32) as u32, protected as u32),
                    receipt,
                )
            }));
        if protected_bytes > max_bytes {
            cache.oversize_bypasses += 1;
        }
        while state.total_resident_bytes() > max_bytes {
            let Some(candidate) = state
                .publication_acks
                .keys()
                .copied()
                .filter(|root| *root != protected || protected_bytes > max_bytes)
                .min_by_key(|root| cache.last_access.get(root).copied().unwrap_or(0))
            else {
                break;
            };
            state.publication_acks.remove(&candidate);
            state.receipts.remove(&candidate);
            state.invalidated_roots.remove(&candidate);
            cache.last_access.remove(&candidate);
            let root = RefNo::new((candidate >> 32) as u32, candidate as u32);
            let ids = state.record_ids_under(root);
            for id in ids {
                let shared = state.geometries.get(&id).is_some_and(|record| {
                    state
                        .receipts
                        .keys()
                        .any(|other| record.ancestors.contains(other))
                });
                if !shared {
                    state.evict(&id);
                }
            }
            cache.evictions += 1;
        }
        state.flush_room_spatial();
    }

    /// Drop one resident cache entry because the durable point lookup says it no
    /// longer exists. Shared nested-root records remain owned by their other
    /// resident receipts. This is cache state, not a generated removal delta.
    pub(crate) fn drop_cached_root(&self, root: RefNo) {
        let packed = packed_refno(root);
        let mut scopes = self.write();
        let Some(state) = scopes.get_mut(&ScopeKey::Current) else {
            return;
        };
        state.receipts.remove(&packed);
        state.publication_acks.remove(&packed);
        state.invalidated_roots.remove(&packed);
        for id in state.record_ids_under(root) {
            let shared = state.geometries.get(&id).is_some_and(|record| {
                state
                    .receipts
                    .keys()
                    .any(|other| record.ancestors.contains(other))
            });
            if !shared {
                state.evict(&id);
            }
        }
        state.flush_room_spatial();
        drop(scopes);
        self.cache_control().last_access.remove(&packed);
    }

    /// A whole-dbnum reinitialize invalidates the process projection immediately.
    /// The epoch bump also prevents an in-flight database snapshot from restoring
    /// the pre-wipe image after this point.
    pub fn clear_dbnum(&self, dbnum: u32) {
        let mut scopes = self.write();
        let state = scopes.entry(ProjectionScope::Current.key()).or_default();
        state.room_baselines.remove(&dbnum);
        let old = state
            .geometries
            .values()
            .filter(|record| record.dbnum == dbnum)
            .map(|record| record.record_id.clone())
            .collect::<Vec<_>>();
        for id in old {
            state.remove(&id);
        }
        state.receipts.retain(|_, receipt| {
            // A targetless receipt is legacy/ambiguous: after any whole-dbnum
            // wipe it cannot prove ownership or freshness, so fail closed and
            // make that root regenerate as well.
            receipt
                .model_target
                .as_ref()
                .is_some_and(|target| target.design.dbnum != dbnum)
        });
        state
            .invalidated_roots
            .retain(|packed| RefNo::new((*packed >> 32) as u32, *packed as u32).dbno() != dbnum);
        state.publication_acks.retain(|_, ack| ack.dbnum != dbnum);
        state.published_sesno.remove(&dbnum);
        state.room_changes.remove(&dbnum);
        state.model_digests.remove(&dbnum);
        state.room_spatial.remove(&dbnum);
        state
            .room_spatial_dirty
            .retain(|(owner, _)| *owner != dbnum);
        state.bump_epoch(dbnum);
    }

    /// 一个生成根名下、指定库的已投影几何身份。旧 `existing_geometry_ids` 的内存版。
    pub fn geometry_ids_of_root(
        &self,
        scope: ProjectionScope<'_>,
        dbnum: u32,
        root: RefNo,
    ) -> Vec<GeometryId> {
        let scopes = self.read();
        let Some(state) = scopes.get(&scope.key()) else {
            return Vec::new();
        };
        state
            .record_ids_under(root)
            .into_iter()
            .filter_map(|record_id| state.geometries.get(&record_id))
            .filter(|record| record.dbnum == dbnum)
            .map(|record| record.geometry_id.clone())
            .collect()
    }

    /// 在同一读锁下复制整根回执与几何，避免发布或淘汰把两个版本拼在一起。
    pub fn root_snapshot(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
    ) -> Option<(RootReceipt, Vec<ProjectedGeometry>)> {
        let snapshot = {
            let scopes = self.read();
            let state = scopes.get(&scope.key())?;
            let receipt = state.receipts.get(&packed_refno(root))?.clone();
            let records = state
                .record_ids_under(root)
                .into_iter()
                .filter_map(|id| state.geometries.get(&id).cloned())
                .collect();
            (receipt, records)
        };
        if matches!(scope, ProjectionScope::Current) {
            self.touch_root(root);
        }
        Some(snapshot)
    }

    /// 一个生成根名下的全部记录。按需删除路径用它。
    pub fn records_of_root(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
    ) -> Vec<ProjectedGeometry> {
        let current = matches!(scope, ProjectionScope::Current);
        let records = {
            let scopes = self.read();
            let Some(state) = scopes.get(&scope.key()) else {
                return Vec::new();
            };
            state
                .record_ids_under(root)
                .into_iter()
                .filter_map(|record_id| state.geometries.get(&record_id).cloned())
                .collect()
        };
        if current {
            self.touch_root(root);
        }
        records
    }

    /// Approximate serialized bytes retained for one complete root overlay.
    /// Shared records are counted once for this root's admission estimate.
    pub(crate) fn resident_bytes_of_root(&self, root: RefNo) -> usize {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return 0;
        };
        state
            .record_ids_under(root)
            .into_iter()
            .filter_map(|id| state.geometry_bytes.get(&id))
            .sum::<usize>()
            .saturating_add(
                state
                    .receipts
                    .get(&packed_refno(root))
                    .map_or(0, |receipt| receipt_resident_bytes(root, receipt)),
            )
    }

    /// 整个投影域的记录，按 record_id 排序。历史快照查询与落库适配器用它。
    pub fn records(&self, scope: ProjectionScope<'_>) -> Vec<ProjectedGeometry> {
        let scopes = self.read();
        scopes
            .get(&scope.key())
            .map(|state| state.geometries.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Immutable file view and its version from one read lock. Room consumers
    /// check this epoch again before committing their results.
    pub(crate) fn snapshot_dbnum(&self, dbnum: u32) -> (u64, Vec<ProjectedGeometry>) {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return (0, Vec::new());
        };
        (
            state.commit_epochs.get(&dbnum).copied().unwrap_or(0),
            state
                .geometries
                .values()
                .filter(|row| row.dbnum == dbnum)
                .cloned()
                .collect(),
        )
    }

    /// 这个投影域里盖过章的生成根。落库适配器按它逐根对账。
    ///
    /// 用回执而不是记录的祖先链：一根投影完（哪怕这一轮一条几何都没变）就有回执，而
    /// 祖先链只在有记录时才说得出根，空根会整个消失。
    pub fn roots(&self, scope: ProjectionScope<'_>) -> Vec<RefNo> {
        let scopes = self.read();
        scopes
            .get(&scope.key())
            .map(|state| {
                state
                    .receipts
                    .keys()
                    .chain(state.invalidated_roots.iter())
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|packed| RefNo::new((packed >> 32) as u32, packed as u32))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Called only after source absence and output deletion have succeeded.
    pub fn forget_retired_root(&self, scope: ProjectionScope<'_>, root: RefNo) {
        if let Some(state) = self.write().get_mut(&scope.key()) {
            state.invalidated_roots.remove(&packed_refno(root));
            state.receipts.remove(&packed_refno(root));
            state.publication_acks.remove(&packed_refno(root));
        }
    }

    pub fn receipt(&self, scope: ProjectionScope<'_>, root: RefNo) -> Option<RootReceipt> {
        let current = matches!(scope, ProjectionScope::Current);
        let receipt = {
            let scopes = self.read();
            scopes
                .get(&scope.key())?
                .receipts
                .get(&packed_refno(root))
                .cloned()
        };
        if current {
            self.touch_root(root);
        }
        receipt
    }

    pub fn publication_ack(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
    ) -> Option<RootPublicationAck> {
        let scopes = self.read();
        scopes
            .get(&scope.key())?
            .publication_acks
            .get(&packed_refno(root))
            .cloned()
    }

    /// 只在对应 process-store 根事务已经成功提交后调用。
    ///
    /// 返回 `false` 表示写盘期间内存候选已经前移；这个旧 ACK 不得推进 durable
    /// 水位，也不得使新候选获得可淘汰资格。
    #[must_use]
    pub fn record_publication_ack(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
        ack: RootPublicationAck,
    ) -> bool {
        let current = matches!(scope, ProjectionScope::Current);
        let durable = ack.medium.is_durable();
        let accepted = {
            let mut scopes = self.write();
            let state = scopes.entry(scope.key()).or_default();
            let Some(receipt) = state.receipts.get(&packed_refno(root)) else {
                return false;
            };
            if !ack.matches(receipt) {
                return false;
            }
            state.publication_acks.insert(packed_refno(root), ack);
            true
        };
        if accepted && current && durable {
            self.touch_root(root);
            self.enforce_current_cache_limit(root);
        }
        accepted
    }

    /// Whether this root has a memory overlay newer or different from the exact
    /// content identity last confirmed by the durable store.
    pub fn root_needs_persist(&self, root: RefNo) -> bool {
        let scopes = self.read();
        let Some(state) = scopes.get(&ScopeKey::Current) else {
            return false;
        };
        let Some(receipt) = state.receipts.get(&packed_refno(root)) else {
            return false;
        };
        !state
            .publication_acks
            .get(&packed_refno(root))
            .is_some_and(|ack| ack.matches(receipt))
    }

    /// Current-memory roots of one database whose exact receipt has not yet
    /// been acknowledged by the durable store. This is the write-behind
    /// pipeline's reconciliation set; it never reads or hydrates RocksDB.
    pub fn pending_roots_of_dbnum(&self, dbnum: u32) -> Vec<RefNo> {
        self.roots(ProjectionScope::Current)
            .into_iter()
            .filter(|root| root.dbno() == dbnum && self.root_needs_persist(*root))
            .collect()
    }

    pub fn invalidate_publication_ack(&self, scope: ProjectionScope<'_>, root: RefNo) {
        if let Some(state) = self.write().get_mut(&scope.key()) {
            state.publication_acks.remove(&packed_refno(root));
        }
    }

    /// Remove only the publication marker for a root while retaining its
    /// generated records.  A caller uses this when an external durability gate
    /// (the mirror spool) fails after projection commit: later `ensure` calls
    /// must retry publication instead of treating the in-memory rows as done.
    pub fn invalidate_receipt(&self, scope: ProjectionScope<'_>, root: RefNo) {
        if let Some(state) = self.write().get_mut(&scope.key()) {
            if state.receipts.contains_key(&packed_refno(root)) {
                state.invalidated_roots.insert(packed_refno(root));
            }
            state.receipts.remove(&packed_refno(root));
            state.publication_acks.remove(&packed_refno(root));
        }
    }

    /// Invalidate every publication marker in a projection scope without
    /// discarding the records that are still safe to display.  A source
    /// snapshot swap uses this as a cheap global fence: the next `ensure`
    /// must compare against and publish from the new epoch before it can call
    /// the projection current again.
    pub fn invalidate_all_receipts(&self, scope: ProjectionScope<'_>) -> usize {
        let mut scopes = self.write();
        let Some(state) = scopes.get_mut(&scope.key()) else {
            return 0;
        };
        let invalidated = state.receipts.len();
        state
            .invalidated_roots
            .extend(state.receipts.keys().copied());
        state.receipts.clear();
        state.publication_acks.clear();
        invalidated
    }

    /// 只作废**一个库**的发布回执与 ACK，记录照旧可显示（lazy-startup L4：文件变化只让对应
    /// dbnum 失效，母计划不变量 7）。根归哪个库由它自己的 refno 位解码定（不变量 1）；
    /// 别的库一个回执都不碰——8000 保存了，7998 的根仍然是「已发布」。
    pub fn invalidate_receipts_of_dbnum(&self, scope: ProjectionScope<'_>, dbnum: u32) -> usize {
        let mut scopes = self.write();
        let Some(state) = scopes.get_mut(&scope.key()) else {
            return 0;
        };
        let belongs = |packed: &i64| {
            crate::data_interface::refno_route::refno_dbnum_of(RefNo::new(
                (*packed >> 32) as u32,
                *packed as u32,
            )) == Some(dbnum)
        };
        let before = state.receipts.len();
        state
            .invalidated_roots
            .extend(state.receipts.keys().filter(|key| belongs(key)).copied());
        state.receipts.retain(|packed, _| !belongs(packed));
        state.publication_acks.retain(|packed, _| !belongs(packed));
        before - state.receipts.len()
    }

    /// 某个库已投影过的最高源会话。旧 `persisted_e3d_session` 的内存版。
    pub fn published_sesno(&self, scope: ProjectionScope<'_>, dbnum: u32) -> Option<u32> {
        let scopes = self.read();
        scopes
            .get(&scope.key())?
            .published_sesno
            .get(&dbnum)
            .copied()
    }

    /// 一个根的投影是否完整：记录数与回执相符，且每条记录的 mesh 文件都还在盘上。
    ///
    /// mesh 文件仍然写盘（内容寻址，不是库），所以这一条与旧
    /// `published_root_projection_complete` 检查的是同一件事：投影里点名的网格必须真的存在，
    /// 否则这根要退回全量重算。
    pub fn root_projection_complete(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
        mesh_dir: &Path,
    ) -> bool {
        let scopes = self.read();
        let Some(state) = scopes.get(&scope.key()) else {
            return false;
        };
        let Some(receipt) = state.receipts.get(&packed_refno(root)) else {
            return false;
        };
        let record_ids = state.record_ids_under(root);
        if record_ids.len() != receipt.geometry_count {
            return false;
        }
        record_ids.into_iter().all(|record_id| {
            state
                .geometries
                .get(&record_id)
                .is_some_and(|record| mesh_dir.join(format!("{}.mesh", record.mesh_id)).is_file())
        })
    }

    /// 丢掉一整个投影域（历史快照过期即丢）。共享的 mesh 文件不动——它们是内容寻址的，
    /// 由可达性 GC 收，不跟着某一个快照走。
    pub fn drop_scope(&self, scope: ProjectionScope<'_>) {
        self.write().remove(&scope.key());
    }

    #[cfg(test)]
    pub(crate) fn reset(&self) {
        self.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(record_id: &str, source: RefNo, root: RefNo, dbnum: u32) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: record_id.to_string(),
            geometry_id: GeometryId::Element {
                refno: source.to_string(),
            },
            source_refno: source,
            dbnum,
            source_sesno: 42,
            noun: "EQUI".into(),
            mesh_id: format!("mesh_{record_id}"),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0; 3], [1.0; 3]],
            ancestors: vec![packed_refno(source), packed_refno(root)],
            shape: ProjectedShape::Baked,
            tube: None,
        }
    }

    fn tube_record(record_id: &str, branch: RefNo, root: RefNo) -> ProjectedGeometry {
        let mut record = record(record_id, branch, root, 7997);
        record.geometry_id = GeometryId::ImpliedTube {
            container_refno: branch.to_string(),
            route_ordinal: 0,
            from_refno: "7997/1".into(),
            to_refno: "7997/2".into(),
        };
        record.tube = Some(ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(7997, 1),
            to: RefNo::new(7997, 2),
            invalid: false,
        });
        record
    }

    fn store() -> ModelMemoryStore {
        ModelMemoryStore::default()
    }

    #[test]
    fn cold_receipt_lookups_do_not_allocate_cache_metadata() {
        let store = ModelMemoryStore::with_cache_limit(4096);
        store.commit(ProjectionScope::Current, vec![ProjectionCommit {
            receipt: Some((RefNo::new(7997, 10_001), RootReceipt {
                source_sesno: 42, model_target: None, manifest_hash: "resident".into(),
                geometry_count: 0, published_via: default_published_via(),
            })),
            ..Default::default()
        }]);
        let before = store.cache_control().last_access.len();
        for id in 0..10_000 {
            assert!(store.receipt(ProjectionScope::Current, RefNo::new(7997, id)).is_none());
        }
        assert_eq!(store.cache_control().last_access.len(), before);
    }

    #[test]
    fn concurrent_root_snapshots_never_mix_receipts_and_geometry() {
        let store = store();
        let root = RefNo::new(7997, 1);
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|threads| {
            threads.spawn(|| {
                barrier.wait();
                for version in 0..2_000 {
                    let count = version % 2 + 1;
                    store.commit(ProjectionScope::Current, vec![ProjectionCommit {
                        removals: vec!["one".into(), "two".into()],
                        upserts: ["one", "two"].into_iter().take(count).map(|id| {
                            let mut row = record(id, root, root, 7997);
                            row.mesh_id = version.to_string();
                            row
                        }).collect(),
                        receipt: Some((root, RootReceipt {
                            source_sesno: 42, model_target: None,
                            manifest_hash: version.to_string(), geometry_count: count,
                            published_via: default_published_via(),
                        })),
                        ..Default::default()
                    }]);
                }
            });
            barrier.wait();
            for _ in 0..2_000 {
                if let Some((receipt, records)) = store.root_snapshot(ProjectionScope::Current, root) {
                    assert_eq!(records.len(), receipt.geometry_count);
                    assert!(records.iter().all(|row| row.mesh_id == receipt.manifest_hash));
                }
                std::thread::yield_now();
            }
        });
    }

    #[test]
    fn root_snapshot_keeps_receipt_and_records_from_one_commit() {
        let store = store();
        let root = RefNo::new(7997, 1);
        let publish = |manifest: &str| store.commit(ProjectionScope::Current, vec![ProjectionCommit {
            upserts: vec![record("one", root, root, 7997)],
            receipt: Some((root, RootReceipt {
                source_sesno: 42, model_target: None, manifest_hash: manifest.into(),
                geometry_count: 1, published_via: default_published_via(),
            })),
            ..Default::default()
        }]);
        publish("old");
        let (receipt, records) = store.root_snapshot(ProjectionScope::Current, root).unwrap();
        publish("new");
        assert_eq!(receipt.manifest_hash, "old");
        assert_eq!(records.len(), receipt.geometry_count);
        assert_eq!(store.root_snapshot(ProjectionScope::Current, root).unwrap().0.manifest_hash, "new");
    }

    /// lazy-startup L4：一个库保存了，只作废**它自己**的回执与 ACK（含第二个 extent 的根——
    /// 归属按位解码，不按 word0），别的库的根仍是「已发布」；记录本身不动。
    #[test]
    fn invalidating_one_dbnum_leaves_other_dbnums_receipts_alone() {
        let store = store();
        let receipt = |sesno: u32| RootReceipt {
            source_sesno: sesno,
            model_target: None,
            manifest_hash: "manifest".into(),
            geometry_count: 1,
            published_via: default_published_via(),
        };
        let root_8000 = RefNo::new(8_000, 100);
        let root_8000_second_extent = RefNo::new(8_000 + 0x2000, 7);
        let root_7998 = RefNo::new(7_998, 100);
        store.commit(
            ProjectionScope::Current,
            vec![
                ProjectionCommit {
                    upserts: vec![record("a", RefNo::new(8_000, 1), root_8000, 8_000)],
                    receipt: Some((root_8000, receipt(37))),
                    ..Default::default()
                },
                ProjectionCommit {
                    receipt: Some((root_8000_second_extent, receipt(37))),
                    ..Default::default()
                },
                ProjectionCommit {
                    upserts: vec![record("b", RefNo::new(7_998, 1), root_7998, 7_998)],
                    receipt: Some((root_7998, receipt(5))),
                    ..Default::default()
                },
            ],
        );
        let ack = |dbnum: u32, sesno: u32| RootPublicationAck {
            dbnum,
            source_sesno: sesno,
            geometry_count: 1,
            manifest_hash: "manifest".into(),
            model_target: None,
            room_membership: false,
            room_generation: None,
            medium: crate::fast_model::model_db_adapter::PersistMedium::EmbeddedMem,
        };
        store.record_publication_ack(ProjectionScope::Current, root_8000, ack(8_000, 37));
        store.record_publication_ack(ProjectionScope::Current, root_7998, ack(7_998, 5));

        assert_eq!(
            store.invalidate_receipts_of_dbnum(ProjectionScope::Current, 8_000),
            2,
            "两个 extent 的根都归 8000"
        );
        assert!(store.receipt(ProjectionScope::Current, root_8000).is_none());
        assert!(
            store
                .receipt(ProjectionScope::Current, root_8000_second_extent)
                .is_none()
        );
        assert!(
            store
                .publication_ack(ProjectionScope::Current, root_8000)
                .is_none()
        );
        let roots = store.roots(ProjectionScope::Current);
        assert_eq!(roots.len(), 3);
        assert!(roots.contains(&root_8000));
        assert!(roots.contains(&root_8000_second_extent));
        assert!(roots.contains(&root_7998));
        store.forget_retired_root(ProjectionScope::Current, root_8000_second_extent);
        assert!(
            !store
                .roots(ProjectionScope::Current)
                .contains(&root_8000_second_extent)
        );
        assert!(store.roots(ProjectionScope::Current).contains(&root_8000));
        assert!(
            store.receipt(ProjectionScope::Current, root_7998).is_some(),
            "7998 一个回执不动"
        );
        assert!(
            store
                .publication_ack(ProjectionScope::Current, root_7998)
                .is_some()
        );
        assert_eq!(
            store
                .records_of_root(ProjectionScope::Current, root_8000)
                .len(),
            1,
            "记录照旧可显示，只有回执作废"
        );
        assert_eq!(
            store.invalidate_receipts_of_dbnum(ProjectionScope::Current, 8_000),
            0,
            "第二次没有可作废的"
        );
        assert_eq!(
            store.invalidate_receipts_of_dbnum(ProjectionScope::Current, 5_052),
            0,
            "没发布过根的库作废 0 条，不 panic"
        );
    }

    #[test]
    fn complete_publication_ack_is_invalidated_before_candidate_geometry_changes() {
        let store = store();
        let root = RefNo::new(24381, 1);
        let geometry = record("a", RefNo::new(24381, 7), root, 7997);
        let receipt = RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: "manifest".into(),
            geometry_count: 1,
            published_via: default_published_via(),
        };
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![geometry.clone()],
                receipt: Some((root, receipt.clone())),
                ..Default::default()
            }],
        );
        let ack = RootPublicationAck {
            dbnum: 7997,
            source_sesno: 42,
            geometry_count: 1,
            manifest_hash: "manifest".into(),
            model_target: None,
            room_membership: true,
            room_generation: Some("rooms-v1".into()),
            medium: crate::fast_model::model_db_adapter::PersistMedium::EmbeddedMem,
        };
        store.record_publication_ack(ProjectionScope::Current, root, ack.clone());
        assert_eq!(
            store.publication_ack(ProjectionScope::Current, root),
            Some(ack.clone())
        );

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![geometry],
                receipt: Some((root, receipt)),
                ..Default::default()
            }],
        );
        assert!(
            store
                .publication_ack(ProjectionScope::Current, root)
                .is_none(),
            "a regenerated candidate must be re-published before it is complete"
        );

        store.record_publication_ack(ProjectionScope::Current, root, ack);
        store.clear_dbnum(7997);
        assert!(
            store
                .publication_ack(ProjectionScope::Current, root)
                .is_none()
        );
    }

    #[test]
    fn room_input_adapter_uses_resident_sources_not_ref0_as_dbnum() {
        let store = store();
        let source = RefNo::new(24381, 7);
        let parent = RefNo::new(24381, 1);
        let mut geometry = record("room_member", source, parent, 7997);
        geometry.world_transform = DMat4::from_translation(glam::DVec3::new(3.0, 4.0, 5.0));
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![geometry.clone()],
                ..Default::default()
            }],
        );
        let rows = store.room_records_for_members(&BTreeSet::from([packed_refno(source)]));
        assert_eq!(rows, vec![geometry.clone()]);
        let inst = rows[0].room_inst();
        assert!(
            inst.has_neg,
            "baked memory records must match durable booled_id records"
        );
        assert_eq!(
            inst.refno,
            crate::data_interface::generation_root::refno_from_e3d(source)
        );
        assert_eq!(
            inst.owner,
            crate::data_interface::generation_root::refno_from_e3d(parent)
        );
        assert_eq!(inst.world_trans.translation, glam::Vec3::new(3.0, 4.0, 5.0));
        assert_eq!(inst.insts[0].geo_hash, geometry.mesh_id);
        assert!(
            store
                .room_records_for_members(&BTreeSet::from([packed_refno(parent)]))
                .is_empty()
        );
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![geometry.record_id],
                ..Default::default()
            }],
        );
        assert!(
            store
                .room_records_for_members(&BTreeSet::from([packed_refno(source)]))
                .is_empty()
        );
    }

    #[test]
    fn canonical_room_input_is_not_reported_as_a_boolean_mesh() {
        let source = RefNo::new(24381, 8);
        let root = RefNo::new(24381, 1);
        let mut geometry = record("canonical_room_member", source, root, 7997);
        geometry.shape = ProjectedShape::Canonical {
            key: PrimitiveMeshKey::CylinderV3 { segments: 32 },
            local_transform: DMat4::IDENTITY,
            local_bounds: [[-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]],
        };

        assert!(!geometry.room_inst().has_neg);
    }

    #[test]
    fn restored_room_baseline_requires_new_ack_and_is_dbnum_scoped() {
        let store = store();
        let root = RefNo::new(24381, 1);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("a", root, root, 7997), record("b", root, root, 1112)],
                ..Default::default()
            }],
        );
        assert!(!store.room_baseline_known(7997));
        assert!(store.ack_rooms(7997, store.commit_epoch(7997)));
        assert!(store.ack_rooms(1112, store.commit_epoch(1112)));
        assert!(store.room_baseline_known(7997));
        let epoch = store.commit_epoch(7997);
        assert!(!store.restore_dbnum(7997, vec![], vec![], epoch + 1));
        assert!(
            store.room_baseline_known(7997),
            "rejected restore preserves baseline"
        );
        assert!(store.restore_dbnum(7997, vec![], vec![], epoch));
        assert!(
            !store.room_baseline_known(7997),
            "restored empty model may have deleted prior members"
        );
        assert!(
            store.room_baseline_known(1112),
            "other database baseline stays valid"
        );
        assert!(!store.ack_rooms(7997, epoch));
        assert!(!store.room_baseline_known(7997));
        assert!(store.ack_rooms(7997, store.commit_epoch(7997)));
        assert!(store.room_baseline_known(7997));
    }

    #[test]
    fn clearing_one_dbnum_drops_only_its_projection_and_advances_the_epoch() {
        let store = store();
        let root_a = RefNo::new(24381, 1);
        let root_b = RefNo::new(24382, 1);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    record("a", root_a, root_a, 7997),
                    record("b", root_b, root_b, 1112),
                ],
                receipt: Some((
                    root_a,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "legacy".into(),
                        geometry_count: 1,
                        published_via: default_published_via(),
                    },
                )),
                ..Default::default()
            }],
        );
        assert!(store.receipt(ProjectionScope::Current, root_a).is_some());
        let epoch = store.commit_epoch(7997);

        store.clear_dbnum(7997);

        assert!(store.commit_epoch(7997) > epoch);
        assert!(
            store
                .records_of_root(ProjectionScope::Current, root_a)
                .is_empty()
        );
        assert!(store.receipt(ProjectionScope::Current, root_a).is_none());
        assert!(!store.roots(ProjectionScope::Current).contains(&root_a));
        assert_eq!(
            store
                .records_of_root(ProjectionScope::Current, root_b)
                .len(),
            1
        );
    }

    #[test]
    fn room_changes_preserve_old_bounds_and_newer_obligations() {
        let store = store();
        let root = RefNo::new(24381, 1);
        let member = RefNo::new(24381, 2);
        let original = record("a", member, root, 7997);
        let put = |row| {
            store.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: vec![row],
                    ..Default::default()
                }],
            )
        };
        put(original.clone());
        let (baseline, digest, _, _) = store.room_snapshot(7997, true);
        assert!(store.ack_rooms(7997, baseline));
        let mut moved = original.clone();
        moved.world_bounds = [[10.0; 3], [11.0; 3]];
        put(moved.clone());
        let (epoch, moved_digest, changes, records) = store.room_snapshot(7997, false);
        assert_eq!(records.len(), 1);
        assert_ne!(digest, moved_digest);
        assert_eq!(
            changes[&packed_refno(member)].before,
            vec![original.world_bounds]
        );
        assert_eq!(
            changes[&packed_refno(member)].after,
            vec![moved.world_bounds]
        );
        assert!(
            store
                .room_members_intersecting(7997, &[original.world_bounds])
                .is_empty()
        );
        assert!(
            store
                .room_members_intersecting(7997, &[moved.world_bounds])
                .contains(&packed_refno(member))
        );
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec!["a".into()],
                ..Default::default()
            }],
        );
        assert!(
            !store.ack_rooms(7997, epoch),
            "new deletion survives old ACK"
        );
        let (_, _, deleted, records) = store.room_snapshot(7997, false);
        assert!(records.is_empty());
        assert_eq!(
            deleted[&packed_refno(member)].before,
            vec![original.world_bounds]
        );
        assert!(deleted[&packed_refno(member)].after.is_empty());
        put(original);
        assert_eq!(
            store.room_snapshot(7997, false).1,
            digest,
            "content digest is history independent"
        );
    }

    #[test]
    fn room_spatial_accepts_large_incremental_pipe_like_cohorts() {
        let store = store();
        let root = RefNo::new(24384, 1);
        let make = |index: u32| {
            let member = RefNo::new(24384, 100_000 + index);
            let angle = index as f32 * 0.173;
            let radius = 8_000.0 + (index % 53) as f32 * 97.0;
            let x = radius * angle.cos();
            let y = radius * angle.sin();
            let z = (index % 17) as f32 * 250.0;
            let mut row = record(&format!("pipe-{index}"), member, root, 8000);
            row.noun = if index % 2 == 0 { "FTUB" } else { "BEND" }.into();
            row.world_bounds = [[x, y, z], [x + 50.0, y + 100.0, z + 30.0]];
            row
        };
        for range in [0..441, 441..800, 800..1_150, 1_150..1_512] {
            store.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: range.map(make).collect(),
                    ..Default::default()
                }],
            );
        }
        assert_eq!(
            store
                .room_members_intersecting(8000, &[[[-20_000.0; 3], [20_000.0; 3]]])
                .len(),
            1_512
        );
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: (0..100).map(|index| format!("pipe-{index}")).collect(),
                ..Default::default()
            }],
        );
        let remaining = store.room_members_intersecting(8000, &[[[-20_000.0; 3], [20_000.0; 3]]]);
        assert_eq!(remaining.len(), 1_412);
        assert!((0..100).all(|index| {
            !remaining.contains(&packed_refno(RefNo::new(24384, 100_000 + index)))
        }));
        let stats = store.cache_stats();
        assert_eq!(stats.room_spatial_rebuild_count, 5);
        assert_eq!(stats.room_spatial_rows_rebuilt, 5_315);
    }

    #[test]
    fn replacing_one_record_moves_the_spatial_member_without_leaving_old_bounds() {
        let store = store();
        let root = RefNo::new(7997, 100);
        let old_member = RefNo::new(7997, 10);
        let new_member = RefNo::new(7997, 20);
        let mut original = record("same", old_member, root, 7997);
        original.world_bounds = [[0.0; 3], [1.0; 3]];
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![original],
                ..Default::default()
            }],
        );

        let mut moved = record("same", new_member, root, 7997);
        moved.world_bounds = [[10.0; 3], [11.0; 3]];
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![moved],
                ..Default::default()
            }],
        );

        assert!(
            store
                .room_members_intersecting(7997, &[[[0.0; 3], [1.0; 3]]])
                .is_empty()
        );
        assert_eq!(
            store.room_members_intersecting(7997, &[[[10.0; 3], [11.0; 3]]]),
            BTreeSet::from([packed_refno(new_member)])
        );
    }

    /// 祖先索引就是旧 `anc CONTAINS <packed>` 的形态：按根能取回子树的全部记录，
    /// 库号照旧参与筛选。
    #[test]
    fn tube_resets_preserve_unrelated_large_projection() {
        let mut state = ScopeState::default();
        let root = RefNo::new(24381, 1);
        for n in 0..50_000 {
            state.insert(record(
                &format!("solid_{n}"),
                RefNo::new(24381, n + 2),
                root,
                7997,
            ));
        }
        let branches = (0..2_000)
            .map(|n| RefNo::new(24381, 100_000 + n))
            .collect::<Vec<_>>();
        for (n, branch) in branches.iter().enumerate() {
            state.insert(tube_record(&format!("tube_{n}"), *branch, root));
        }
        let started = std::time::Instant::now();
        state.apply(ProjectionCommit {
            tube_branch_resets: branches,
            ..Default::default()
        });
        println!(
            "tube-reset-benchmark: 50000 unrelated geometries, 2000 branches, elapsed_us={}",
            started.elapsed().as_micros()
        );
        assert_eq!(state.geometries.len(), 50_000);
        assert!(state.geometries.values().all(|r| r.tube.is_none()));
    }

    #[test]
    fn tube_resets_follow_source_after_replacement_even_without_ancestor() {
        let mut state = ScopeState::default();
        let a = RefNo::new(24381, 10);
        let b = RefNo::new(24381, 20);
        let root = RefNo::new(24381, 1);
        let mut tube = tube_record("moved", a, root);
        tube.ancestors.clear();
        state.insert(tube.clone());
        tube.source_refno = b;
        state.insert(tube);
        state.insert(tube_record("solid_now", a, root));
        state.insert(record("solid_now", a, root, 7997));
        state.apply(ProjectionCommit {
            tube_branch_resets: vec![a],
            ..Default::default()
        });
        assert!(state.geometries.contains_key("moved"));
        assert!(state.geometries.contains_key("solid_now"));
        state.apply(ProjectionCommit {
            tube_branch_resets: vec![b],
            ..Default::default()
        });
        assert!(!state.geometries.contains_key("moved"));
        assert!(state.geometries.contains_key("solid_now"));
    }

    #[test]
    fn records_are_reachable_by_root_and_filtered_by_dbnum() {
        let store = store();
        let root = RefNo::new(7997, 100);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    record("a", RefNo::new(7997, 1), root, 7997),
                    record("b", RefNo::new(7997, 2), root, 7997),
                    record("c", RefNo::new(7997, 3), root, 8000),
                ],
                ..ProjectionCommit::default()
            }],
        );

        assert_eq!(
            store
                .geometry_ids_of_root(ProjectionScope::Current, 7997, root)
                .len(),
            2
        );
        assert_eq!(
            store.records_of_root(ProjectionScope::Current, root).len(),
            3
        );
        assert!(
            store
                .geometry_ids_of_root(ProjectionScope::Current, 7997, RefNo::new(7997, 999))
                .is_empty()
        );
    }

    /// 读库镜像的那几秒里，「只写内存」的按需生成往同一个库提交了一条——旧镜像不许换
    /// 进来。照换的话那条记录内存里没了、库里也没有（它本来就不落库），无声消失。
    #[test]
    fn a_stale_database_image_is_refused_after_a_concurrent_memory_commit() {
        let store = store();
        let root = RefNo::new(7997, 100);
        // 库里已提交的那份镜像（读库时看到的）。
        let epoch = store.commit_epoch(7997);
        let image = vec![record("a", RefNo::new(7997, 1), root, 7997)];

        // 读库翻页期间，按需生成把 "b" 提交进内存投影。
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("b", RefNo::new(7997, 2), root, 7997)],
                ..ProjectionCommit::default()
            }],
        );
        assert_ne!(store.commit_epoch(7997), epoch, "提交必须推进纪元");

        assert!(
            !store.restore_dbnum(7997, image.clone(), vec![], epoch),
            "纪元已变，旧镜像不许换进来"
        );
        assert_eq!(
            store.records_of_root(ProjectionScope::Current, root).len(),
            1,
            "被拒的恢复不许动投影，按需生成那条得还在"
        );

        // 别的库的提交与这个库无关，不该把它的恢复挡下来。
        let epoch = store.commit_epoch(7997);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record(
                    "c",
                    RefNo::new(8000, 3),
                    RefNo::new(8000, 100),
                    8000,
                )],
                ..ProjectionCommit::default()
            }],
        );
        assert!(
            store.restore_dbnum(7997, image, vec![], epoch),
            "8000 的提交不该挡住 7997 的恢复"
        );
        let ids = store
            .records_of_root(ProjectionScope::Current, root)
            .into_iter()
            .map(|record| record.record_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["a".to_string()], "换进来之后就是库里那一份");
    }

    /// 删掉一条记录，祖先索引跟着收干净——留着空桶等于按根查还会捞出已删的 id。
    #[test]
    fn removal_clears_the_ancestor_index_too() {
        let store = store();
        let root = RefNo::new(7997, 100);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("a", RefNo::new(7997, 1), root, 7997)],
                ..ProjectionCommit::default()
            }],
        );
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec!["a".into()],
                ..ProjectionCommit::default()
            }],
        );
        assert!(
            store
                .records_of_root(ProjectionScope::Current, root)
                .is_empty()
        );
        assert!(store.records(ProjectionScope::Current).is_empty());
    }

    #[test]
    fn reparent_cohort_preserves_new_owner_in_either_root_order() {
        let old_root = RefNo::new(7997, 100);
        let new_root = RefNo::new(7997, 200);
        let member = RefNo::new(7997, 10);
        for destination_first in [false, true] {
            let store = store();
            store.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: vec![
                        record("moved", member, old_root, 7997),
                        tube_record("tube", member, old_root),
                    ],
                    ..Default::default()
                }],
            );
            let source = ProjectionCommit {
                removals: vec!["moved".into()],
                tube_branch_resets: vec![member],
                ..Default::default()
            };
            let destination = ProjectionCommit {
                tube_branch_resets: vec![member],
                upserts: vec![
                    record("moved", member, new_root, 7997),
                    tube_record("tube", member, new_root),
                ],
                ..Default::default()
            };
            let commits = if destination_first {
                vec![destination, source]
            } else {
                vec![source, destination]
            };
            store.commit(ProjectionScope::Current, commits);
            assert_eq!(
                store.room_members_intersecting(7997, &[[[0.0; 3], [1.0; 3]]]),
                BTreeSet::from([packed_refno(member)]),
                "destination_first={destination_first}"
            );
            assert!(
                store
                    .records_of_root(ProjectionScope::Current, old_root)
                    .is_empty()
            );
            assert_eq!(
                store
                    .records_of_root(ProjectionScope::Current, new_root)
                    .len(),
                2,
                "destination_first={destination_first}"
            );
            assert_eq!(store.records(ProjectionScope::Current).len(), 2);
        }
    }

    /// 分支重置只清这个分支的管身，别的分支与同来源的普通元素行留着——旧
    /// `DELETE pe:⟨branch⟩->tubi_relate` 就是这个范围。
    #[test]
    fn a_branch_reset_clears_only_that_branch_tubes() {
        let store = store();
        let root = RefNo::new(7997, 100);
        let branch = RefNo::new(7997, 10);
        let other = RefNo::new(7997, 20);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    tube_record("derived_a", branch, root),
                    tube_record("derived_b", branch, root),
                    tube_record("derived_c", other, root),
                    record("7997_10", branch, root, 7997),
                ],
                ..ProjectionCommit::default()
            }],
        );

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                tube_branch_resets: vec![branch],
                upserts: vec![tube_record("derived_d", branch, root)],
                ..ProjectionCommit::default()
            }],
        );

        let ids = store
            .records(ProjectionScope::Current)
            .into_iter()
            .map(|record| record.record_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["7997_10", "derived_c", "derived_d"]);
    }

    /// 投影域之间互不可见：历史快照丢掉不碰当前投影。
    #[test]
    fn scopes_are_isolated_and_droppable() {
        let store = store();
        let root = RefNo::new(7997, 100);
        let upsert = |record_id: &str| ProjectionCommit {
            upserts: vec![record(record_id, RefNo::new(7997, 1), root, 7997)],
            ..ProjectionCommit::default()
        };
        store.commit(ProjectionScope::Current, vec![upsert("live")]);
        store.commit(
            ProjectionScope::Historical("7997_100@24"),
            vec![upsert("snap")],
        );

        assert_eq!(store.records(ProjectionScope::Current).len(), 1);
        assert_eq!(
            store
                .records(ProjectionScope::Historical("7997_100@24"))
                .len(),
            1
        );

        store.drop_scope(ProjectionScope::Historical("7997_100@24"));
        assert!(
            store
                .records(ProjectionScope::Historical("7997_100@24"))
                .is_empty()
        );
        assert_eq!(store.records(ProjectionScope::Current).len(), 1);
    }

    /// 已投影会话单调：删记录不把水位拉低，否则一次清理就能让陈旧会话重新盖上来。
    #[test]
    fn the_published_session_watermark_only_rises() {
        let store = store();
        let root = RefNo::new(7997, 100);
        let mut newer = record("a", RefNo::new(7997, 1), root, 7997);
        newer.source_sesno = 43;
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![newer],
                ..ProjectionCommit::default()
            }],
        );
        assert_eq!(
            store.published_sesno(ProjectionScope::Current, 7997),
            Some(43)
        );

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec!["a".into()],
                ..ProjectionCommit::default()
            }],
        );
        assert_eq!(
            store.published_sesno(ProjectionScope::Current, 7997),
            Some(43)
        );
        assert_eq!(store.published_sesno(ProjectionScope::Current, 8000), None);
    }

    #[test]
    fn snapshot_fence_invalidates_receipts_but_keeps_projected_records() {
        let store = store();
        let root = RefNo::new(7997, 100);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("a", RefNo::new(7997, 1), root, 7997)],
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "manifest".into(),
                        geometry_count: 1,
                        published_via: "regen".into(),
                    },
                )),
                ..ProjectionCommit::default()
            }],
        );

        assert_eq!(store.invalidate_all_receipts(ProjectionScope::Current), 1);
        assert!(store.receipt(ProjectionScope::Current, root).is_none());
        assert_eq!(store.records(ProjectionScope::Current).len(), 1);
        assert_eq!(store.invalidate_all_receipts(ProjectionScope::Current), 0);
    }

    /// 回执数目对不上、或者点名的 mesh 文件不在盘上，这根就不算投影完整——
    /// 与旧 `published_root_projection_complete` 同一判据。
    #[test]
    fn a_root_is_complete_only_with_matching_counts_and_present_meshes() {
        let store = store();
        let root = RefNo::new(7997, 100);
        let mesh_dir = std::env::temp_dir().join(format!(
            "model-memory-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&mesh_dir).unwrap();
        let mesh = mesh_dir.join("mesh_a.mesh");
        std::fs::write(&mesh, b"fixture").unwrap();

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("a", RefNo::new(7997, 1), root, 7997)],
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "manifest".into(),
                        geometry_count: 1,
                        published_via: "regen".into(),
                    },
                )),
                ..ProjectionCommit::default()
            }],
        );
        assert!(store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir));

        std::fs::remove_file(&mesh).unwrap();
        assert!(
            !store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
            "少一个 mesh 文件就得退回全量重算"
        );

        std::fs::write(&mesh, b"fixture").unwrap();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![record("b", RefNo::new(7997, 2), root, 7997)],
                ..ProjectionCommit::default()
            }],
        );
        assert!(
            !store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
            "记录数与回执对不上同样不算完整"
        );
        std::fs::remove_dir_all(mesh_dir).unwrap();
    }

    #[test]
    fn the_byte_limit_evicts_only_durably_acked_roots() {
        let store = ModelMemoryStore::with_cache_limit(1);
        let root_a = RefNo::new(7997, 100);
        let root_b = RefNo::new(7997, 200);
        let receipt = |name: &str| RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: name.into(),
            geometry_count: 1,
            published_via: "regen".into(),
        };
        store.commit(
            ProjectionScope::Current,
            vec![
                ProjectionCommit {
                    upserts: vec![record("a", RefNo::new(7997, 1), root_a, 7997)],
                    receipt: Some((root_a, receipt("a"))),
                    ..Default::default()
                },
                ProjectionCommit {
                    upserts: vec![record("b", RefNo::new(7997, 2), root_b, 7997)],
                    receipt: Some((root_b, receipt("b"))),
                    ..Default::default()
                },
            ],
        );
        let ack = |root: RefNo| RootPublicationAck {
            dbnum: root.dbno(),
            source_sesno: 42,
            geometry_count: 1,
            manifest_hash: if root == root_a { "a" } else { "b" }.into(),
            model_target: None,
            room_membership: false,
            room_generation: None,
            medium: crate::fast_model::model_db_adapter::PersistMedium::RocksDb,
        };

        store.record_publication_ack(ProjectionScope::Current, root_a, ack(root_a));
        assert!(store.receipt(ProjectionScope::Current, root_a).is_none());
        assert!(
            store.receipt(ProjectionScope::Current, root_b).is_some(),
            "an unacknowledged candidate must survive cache pressure"
        );
        assert_eq!(store.cache_stats().evictions, 1);

        store.record_publication_ack(ProjectionScope::Current, root_b, ack(root_b));
        assert!(store.receipt(ProjectionScope::Current, root_b).is_none());
        let stats = store.cache_stats();
        assert_eq!(stats.evictions, 2);
        assert_eq!(stats.resident_bytes, 0);
        assert_eq!(stats.oversize_bypasses, 2);
    }

    #[test]
    fn hard_limit_rejects_pending_cohort_before_installing_it() {
        let root = RefNo::new(7997, 100);
        let row = record("too-large", RefNo::new(7997, 1), root, 7997);
        let store = ModelMemoryStore::with_cache_limit(1);
        let error = store
            .try_commit_current(vec![ProjectionCommit {
                upserts: vec![row],
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "large".into(),
                        geometry_count: 1,
                        published_via: "regen".into(),
                    },
                )),
                ..Default::default()
            }])
            .expect_err("a pending root may not exceed the hard limit");
        assert_eq!(error.max_bytes, 1);
        assert!(error.projected_bytes > error.max_bytes);
        assert_eq!(store.current_counts(), (0, 0));
        assert_eq!(store.commit_epoch(7997), 0);
    }

    #[test]
    fn hard_limit_charges_nonzero_residency_for_empty_roots() {
        let store = ModelMemoryStore::with_cache_limit(1);
        let root = RefNo::new(7997, 101);
        let error = store
            .try_commit_current(vec![ProjectionCommit {
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "empty".into(),
                        geometry_count: 0,
                        published_via: "regen".into(),
                    },
                )),
                ..Default::default()
            }])
            .expect_err("an empty root still consumes receipt and identity memory");
        assert!(error.projected_bytes > 1);
        assert_eq!(store.current_counts(), (0, 0));
        assert_eq!(store.commit_epoch(7997), 0);
    }

    #[test]
    fn admission_counts_only_the_final_upsert_for_a_record_id() {
        let root = RefNo::new(7997, 102);
        let first = record("same", RefNo::new(7997, 1), root, 7997);
        let mut last = first.clone();
        last.noun = "LAST".into();
        let receipt = RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: "final-write".into(),
            geometry_count: 1,
            published_via: "regen".into(),
        };
        let exact_limit =
            serde_json::to_vec(&last).unwrap().len() + receipt_resident_bytes(root, &receipt);
        let store = ModelMemoryStore::with_cache_limit(exact_limit);
        store
            .try_commit_current(vec![ProjectionCommit {
                upserts: vec![first, last],
                receipt: Some((root, receipt)),
                ..Default::default()
            }])
            .expect("final-write accounting must not double-charge duplicate ids");
        assert_eq!(store.current_counts(), (1, 1));
        assert!(store.cache_stats().resident_bytes <= exact_limit);
    }

    #[test]
    fn hard_limit_evicts_a_clean_root_to_admit_a_new_pending_root() {
        let root_a = RefNo::new(7997, 100);
        let root_b = RefNo::new(7997, 200);
        let row_a = record("a", RefNo::new(7997, 1), root_a, 7997);
        let row_b = record("b", RefNo::new(7997, 2), root_b, 7997);
        let receipt = |manifest: &str| RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: manifest.into(),
            geometry_count: 1,
            published_via: "regen".into(),
        };
        let max_bytes = (serde_json::to_vec(&row_a).unwrap().len()
            + receipt_resident_bytes(root_a, &receipt("a")))
        .max(
            serde_json::to_vec(&row_b).unwrap().len()
                + receipt_resident_bytes(root_b, &receipt("b")),
        );
        let store = ModelMemoryStore::with_cache_limit(max_bytes);
        store
            .try_commit_current(vec![ProjectionCommit {
                upserts: vec![row_a],
                receipt: Some((root_a, receipt("a"))),
                ..Default::default()
            }])
            .unwrap();
        assert!(store.record_publication_ack(
            ProjectionScope::Current,
            root_a,
            RootPublicationAck {
                dbnum: 7997,
                source_sesno: 42,
                geometry_count: 1,
                manifest_hash: "a".into(),
                model_target: None,
                room_membership: false,
                room_generation: None,
                medium: crate::fast_model::model_db_adapter::PersistMedium::RocksDb,
            },
        ));
        assert_eq!(
            store.room_members_intersecting(7997, &[[[0.0; 3], [1.0; 3]]]),
            BTreeSet::from([packed_refno(RefNo::new(7997, 1))])
        );

        store
            .try_commit_current(vec![ProjectionCommit {
                upserts: vec![row_b],
                receipt: Some((root_b, receipt("b"))),
                ..Default::default()
            }])
            .unwrap();
        assert!(store.receipt(ProjectionScope::Current, root_a).is_none());
        assert!(store.receipt(ProjectionScope::Current, root_b).is_some());
        assert!(store.root_needs_persist(root_b));
        assert_eq!(store.cache_stats().evictions, 1);
        assert_eq!(
            store.room_members_intersecting(7997, &[[[0.0; 3], [1.0; 3]]]),
            BTreeSet::from([packed_refno(RefNo::new(7997, 2))])
        );
    }

    #[test]
    fn an_old_same_session_ack_cannot_advance_the_durable_watermark() {
        let store = ModelMemoryStore::with_cache_limit(1024);
        let root = RefNo::new(7997, 300);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "new-content".into(),
                        geometry_count: 0,
                        published_via: "regen".into(),
                    },
                )),
                ..Default::default()
            }],
        );
        let accepted = store.record_publication_ack(
            ProjectionScope::Current,
            root,
            RootPublicationAck {
                dbnum: root.dbno(),
                source_sesno: 42,
                geometry_count: 0,
                manifest_hash: "old-content".into(),
                model_target: None,
                room_membership: false,
                room_generation: None,
                medium: crate::fast_model::model_db_adapter::PersistMedium::RocksDb,
            },
        );
        assert!(!accepted);
        assert!(store.root_needs_persist(root));
        assert_eq!(store.pending_roots_of_dbnum(root.dbno()), vec![root]);
        let stats = store.cache_stats();
        assert_eq!((stats.pending_roots, stats.durable_roots), (1, 0));

        assert!(store.record_publication_ack(
            ProjectionScope::Current,
            root,
            RootPublicationAck {
                dbnum: root.dbno(),
                source_sesno: 42,
                geometry_count: 0,
                manifest_hash: "new-content".into(),
                model_target: None,
                room_membership: false,
                room_generation: None,
                medium: crate::fast_model::model_db_adapter::PersistMedium::RocksDb,
            },
        ));
        assert!(store.pending_roots_of_dbnum(root.dbno()).is_empty());
    }
}
