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
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

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

/// 一个生成根上一次投影留下的回执，用来判断「目标没变，不必重算」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootReceipt {
    pub source_sesno: u32,
    pub model_target: Option<ModelTarget>,
    pub manifest_hash: String,
    pub geometry_count: usize,
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

/// 把 refno 打包成旧 `anc` 数组里那个 i64。
pub fn packed_refno(refno: RefNo) -> i64 {
    (((refno.word0 as u64) << 32) | refno.word1 as u64) as i64
}

#[derive(Debug, Default)]
struct ScopeState {
    /// record_id → 记录。
    geometries: BTreeMap<String, ProjectedGeometry>,
    /// 打包祖先 → 挂在它下面的 record_id。旧 `anc CONTAINS <packed>` 的索引形态。
    by_ancestor: BTreeMap<i64, BTreeSet<String>>,
    /// 管段来源 → 管身记录。重置管段不应遍历其它根的全部几何。
    by_tube_branch: BTreeMap<i64, BTreeSet<String>>,
    /// 生成根 → 回执。
    receipts: BTreeMap<i64, RootReceipt>,
    /// 每个库投影过的最高源会话。单调，只涨不落——它护的是「旧会话不许盖新会话」。
    published_sesno: BTreeMap<u32, u32>,
}

impl ScopeState {
    fn insert(&mut self, record: ProjectedGeometry) {
        self.remove(&record.record_id);
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
        self.geometries.insert(record.record_id.clone(), record);
    }

    fn remove(&mut self, record_id: &str) -> Option<ProjectedGeometry> {
        let record = self.geometries.remove(record_id)?;
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
            self.receipts.insert(packed_refno(root), receipt);
        }
    }

    #[cfg(test)]
    fn apply(&mut self, commit: ProjectionCommit) {
        self.apply_removals(&commit);
        self.apply_upserts_and_receipt(commit);
    }
}

/// 进程内的模型投影存储。生成侧只写它，读侧（历史快照查询、按需删除、落库适配器）
/// 只读它。
#[derive(Debug, Default)]
pub struct ModelMemoryStore {
    scopes: RwLock<HashMap<ScopeKey, ScopeState>>,
}

static STORE: OnceLock<ModelMemoryStore> = OnceLock::new();

impl ModelMemoryStore {
    pub fn global() -> &'static Self {
        STORE.get_or_init(Self::default)
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
            state.apply_removals(commit);
        }
        for commit in commits {
            state.apply_upserts_and_receipt(commit);
        }
    }

    pub fn current_counts(&self) -> (usize, usize) {
        self.read()
            .get(&ProjectionScope::Current.key())
            .map(|state| (state.geometries.len(), state.receipts.len()))
            .unwrap_or_default()
    }

    /// Restore a complete published database image without executing generation.
    /// The replacement is atomic to all readers of the process projection.
    pub fn restore_dbnum(
        &self,
        dbnum: u32,
        records: Vec<ProjectedGeometry>,
        receipts: Vec<(RefNo, RootReceipt)>,
    ) {
        let mut scopes = self.write();
        let state = scopes.entry(ProjectionScope::Current.key()).or_default();
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
        state.published_sesno.remove(&dbnum);
        for record in records {
            state.insert(record);
        }
        for (root, receipt) in receipts {
            let session = state.published_sesno.entry(dbnum).or_default();
            *session = (*session).max(receipt.source_sesno);
            state.receipts.insert(packed_refno(root), receipt);
        }
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

    /// 一个生成根名下的全部记录，按库分组。按需删除路径用它。
    pub fn records_of_root(
        &self,
        scope: ProjectionScope<'_>,
        root: RefNo,
    ) -> Vec<ProjectedGeometry> {
        let scopes = self.read();
        let Some(state) = scopes.get(&scope.key()) else {
            return Vec::new();
        };
        state
            .record_ids_under(root)
            .into_iter()
            .filter_map(|record_id| state.geometries.get(&record_id).cloned())
            .collect()
    }

    /// 整个投影域的记录，按 record_id 排序。历史快照查询与落库适配器用它。
    pub fn records(&self, scope: ProjectionScope<'_>) -> Vec<ProjectedGeometry> {
        let scopes = self.read();
        scopes
            .get(&scope.key())
            .map(|state| state.geometries.values().cloned().collect())
            .unwrap_or_default()
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
                    .map(|&packed| RefNo::new((packed >> 32) as u32, packed as u32))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn receipt(&self, scope: ProjectionScope<'_>, root: RefNo) -> Option<RootReceipt> {
        let scopes = self.read();
        scopes
            .get(&scope.key())?
            .receipts
            .get(&packed_refno(root))
            .cloned()
    }

    /// Remove only the publication marker for a root while retaining its
    /// generated records.  A caller uses this when an external durability gate
    /// (the mirror spool) fails after projection commit: later `ensure` calls
    /// must retry publication instead of treating the in-memory rows as done.
    pub fn invalidate_receipt(&self, scope: ProjectionScope<'_>, root: RefNo) {
        if let Some(state) = self.write().get_mut(&scope.key()) {
            state.receipts.remove(&packed_refno(root));
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
        state.receipts.clear();
        invalidated
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
}
