//! e3d-io backed model-tree reads for the Web API.
//!
//! This module deliberately does not query `pe`, `pe_owner`, or any other
//! SurrealDB element table.  MDB membership comes from the SYS file, and every
//! displayed node is decoded from the selected DESI database by [`DirectStore`].
//!
//! 库级元数据来自文件头；OWNER、TYPE 和成员仅按请求点查。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use aios_core::pdms_types::EleTreeNode;
use aios_core::{RefU64, RefnoEnum};
use anyhow::Context;

use super::direct_index::DbIndexes;
use super::direct_store::{DirectStore, DirectStoreError};
use super::generation_root::{
    GenerationNode, GenerationRoot, SubtreeElement, enumerate_generation_roots_in_subtree,
    resolve_element_generation_root,
};
use super::group_membership::{
    GroupIndex, GroupMembers, GroupScope, ResolvedMember, SecondaryParent, UnresolvedSlot,
    expand_group_with_rules_on, is_group_noun, resolved_members_with_rules,
};
use super::mdb_membership::MdbMembership;
use super::mdb_read_store::{MdbReadProvider, MdbReadStore};
use super::scosel_eval::ScoselScope;

/// One element database of the frozen tree view: its identity and the session it is
/// pinned to, read from the file header through the shared read view. No element count —
/// there is no census behind this service.
#[derive(Debug, Clone)]
pub struct TreeDatabase {
    pub dbnum: u32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
}

/// Frozen database metadata; no per-element census.
pub struct TreeMetadata {
    databases: Vec<TreeDatabase>,
    worlds: HashMap<u32, RefU64>,
    ref0s: HashMap<u32, Vec<u32>>,
    problems: Vec<String>,
}
impl TreeMetadata {
    fn open(membership: &MdbMembership, store: &MdbReadStore) -> anyhow::Result<Self> {
        let mut result = Self {
            databases: Vec::new(),
            worlds: HashMap::new(),
            ref0s: HashMap::new(),
            problems: membership.problems().to_vec(),
        };
        for db in membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            let metadata = store.metadata(db.dbnum)?;
            result.worlds.insert(
                db.dbnum,
                RefU64::from_two_nums(metadata.world.word0, metadata.world.word1),
            );
            result.ref0s.insert(db.dbnum, metadata.extent_ref0s);
            result.databases.push(TreeDatabase {
                dbnum: db.dbnum,
                db_type: metadata.db_type,
                file: metadata.source.file,
                sesno: metadata.source.sesno,
            });
        }
        Ok(result)
    }
    pub fn databases(&self) -> &[TreeDatabase] {
        &self.databases
    }
    pub fn ref0s_of(&self, dbnum: u32) -> Vec<u32> {
        self.ref0s.get(&dbnum).cloned().unwrap_or_default()
    }
    /// 这个库的 WORLD 元素；库不在 MDB 里或没有文件时 `None`。
    pub fn world_of(&self, dbnum: u32) -> Option<RefU64> {
        self.worlds.get(&dbnum).copied()
    }
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
    pub fn session_fingerprint(&self) -> Vec<(u32, std::path::PathBuf, u32)> {
        self.databases
            .iter()
            .map(|db| (db.dbnum, db.file.clone(), db.sesno))
            .collect()
    }
    pub fn epoch(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.session_fingerprint().hash(&mut hash);
        hash.finish()
    }
}

/// One process-local, immutable view of the current MDB model tree.
///
/// Each DESI file is opened once at the latest session visible when first read.
/// `DirectStore` then freezes that session for the lifetime of this service, so
/// roots, children, and ancestor requests cannot accidentally mix sessions.
pub struct DirectTreeService {
    project: String,
    mdb: String,
    membership: Arc<MdbMembership>,
    store: Arc<MdbReadStore>,
    reader: Arc<MdbReadProvider>,
    metadata: std::sync::Mutex<Option<Arc<TreeMetadata>>>,
    /// `children()` 的整层记忆。成员表与树行都钉在本服务那只冻结会话上——文件
    /// 一动 `is_current()` 就否、provider 换一整只服务，所以这张表与服务同寿命
    /// 即正确，不需要也不允许跨服务携带。
    children_memo: std::sync::Mutex<HashMap<RefU64, Arc<Vec<EleTreeNode>>>>,
    /// `resolved_children()` 的整层记忆，与 `children_memo` 分开存、同一寿命理由。
    resolved_memo: std::sync::Mutex<HashMap<RefU64, Arc<ResolvedChildren>>>,
    /// 分组二级收录的反查表（[`GroupIndex`]），首次要用时建；钩在冻结会话上，换会话即换服务。
    group_index: std::sync::Mutex<Option<Arc<GroupIndex>>>,
    /// 规则组 `SCOSEL` 求值域（MDB 全量跨库），首次要用时建，理由同 `group_index`：
    /// 逐库索引钉在本服务那只冻结会话上，会话一换 provider 就换整只服务，所以与服务同寿命即正确。
    scosel_scope: std::sync::Mutex<Option<Arc<MdbScoselScope>>>,
}

impl DirectTreeService {
    /// Resolve the MDB from its SYS file and build the service on the current
    /// sessions of its files.  No element row is read from SurrealDB.
    ///
    /// Freeze exact sessions from headers without enumerating elements.
    pub fn open(project: &str, mdb: &str) -> anyhow::Result<Self> {
        let reader = MdbReadProvider::shared(project, mdb)?;
        let store = reader.view()?;
        Ok(Self {
            project: project.to_string(),
            mdb: format!("/{}", mdb.trim_start_matches('/')),
            membership: reader.membership.clone(),
            store,
            reader,
            metadata: std::sync::Mutex::new(None),
            children_memo: std::sync::Mutex::new(HashMap::new()),
            resolved_memo: std::sync::Mutex::new(HashMap::new()),
            group_index: std::sync::Mutex::new(None),
            scosel_scope: std::sync::Mutex::new(None),
        })
    }

    pub fn metadata(&self) -> anyhow::Result<Arc<TreeMetadata>> {
        let mut slot = self
            .metadata
            .lock()
            .map_err(|_| anyhow::anyhow!("tree metadata lock poisoned"))?;
        if let Some(metadata) = slot.as_ref() {
            return Ok(metadata.clone());
        }
        let metadata = Arc::new(TreeMetadata::open(&self.membership, &self.store)?);
        *slot = Some(metadata.clone());
        Ok(metadata)
    }

    pub(crate) fn is_current(&self) -> bool {
        self.reader
            .view()
            .is_ok_and(|view| Arc::ptr_eq(&self.store, &view))
    }

    pub(crate) fn source_metadata(
        &self,
        dbnum: u32,
    ) -> anyhow::Result<e3d_io::db_element::DbMetadata> {
        Ok(self.store.metadata(dbnum)?)
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    /// SITE rows in MDB CURD order, then in each WORL's stored member order.
    ///
    /// Enumerating every SITE explicitly needs each library WORLD; ancestors/children do not.
    pub fn roots(&self) -> anyhow::Result<Vec<EleTreeNode>> {
        let metadata = self.metadata()?;

        let mut out = Vec::new();
        for db in self.membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            for world in metadata.worlds.get(&db.dbnum).copied() {
                for (order, child) in self.store.members(world)?.into_iter().enumerate() {
                    let node = self.node(child, Some(world), order)?;
                    if node.noun.eq_ignore_ascii_case("SITE") {
                        out.push(node);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Direct members in the exact order stored in the element record.
    ///
    /// 整层结果记进 `children_memo`：同一冻结会话下成员表不变，重复展开、祖先
    /// 侧查（`node()` 经属主拿兄弟）与 `subtree_refnos` 的 BFS 都不必重付。
    /// 失败的不记——下一次调用重读。
    pub fn children(&self, parent: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        if let Some(hit) = self
            .children_memo
            .lock()
            .map_err(|_| anyhow::anyhow!("tree children memo lock poisoned"))?
            .get(&parent)
        {
            return Ok((**hit).clone());
        }
        let nodes = self
            .store
            .members(parent)?
            .into_iter()
            .enumerate()
            .map(|(order, child)| self.node(child, Some(parent), order))
            .collect::<anyhow::Result<Vec<EleTreeNode>>>()?;
        self.children_memo
            .lock()
            .map_err(|_| anyhow::anyhow!("tree children memo lock poisoned"))?
            .insert(parent, Arc::new(nodes.clone()));
        Ok(nodes)
    }

    /// Root plus its complete member subtree, read from e3d-io in stored order.
    /// This is the direct-mode scope source for on-demand model generation; it
    /// avoids rebuilding the same hierarchy through SurrealDB before every
    /// display request.
    pub fn subtree_refnos(&self, root: RefU64) -> anyhow::Result<Vec<RefnoEnum>> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(current) = stack.pop() {
            if !seen.insert(current) {
                continue;
            }
            out.push(RefnoEnum::from(current));
            let members = self.store.members(current)?;
            stack.extend(members.into_iter().rev());
        }
        Ok(out)
    }

    /// Resolve every generation root in a subtree directly from stored member
    /// order and noun attributes. Delivery units win; outside a delivery unit,
    /// Core3D-significant nodes are the fallback generation roots.
    ///
    /// The traversal itself lives in `generation_root::enumerate_generation_roots_in_subtree`
    /// and is shared with the increment planner's e3d-io enumeration (ADR-056 N7);
    /// this service only supplies the `DirectStore` reads.
    pub fn generation_roots_in_subtree(
        &self,
        root: RefU64,
        unit_types: &[String],
    ) -> anyhow::Result<Vec<GenerationRoot>> {
        let requested = RefnoEnum::from(root);
        if let Some(ancestor) = self.generation_root_of(root, unit_types)?
            && ancestor.root != requested
        {
            return Ok(vec![ancestor]);
        }
        generation_roots_in_subtree_on(self.store.as_ref(), root, unit_types)
    }

    /// Resolve the requested element's owning generation root by OWNER point reads.
    /// Unlike subtree enumeration, a container never selects its first descendant.
    pub fn generation_root_of(
        &self,
        root: RefU64,
        unit_types: &[String],
    ) -> anyhow::Result<Option<GenerationRoot>> {
        self.store.element_snapshot(root)?;
        let requested = RefnoEnum::from(root);
        let nodes = self
            .ancestors(root)?
            .into_iter()
            .map(|refno| {
                let attrs = self.store.named_attmap(refno.refno())?;
                let owner = attrs
                    .get_refu64("OWNER")
                    .filter(|owner| owner.0 != 0 && *owner != refno.refno())
                    .map(RefnoEnum::from);
                Ok((
                    refno,
                    GenerationNode {
                        owner,
                        noun: attrs.get_as_string("TYPE").unwrap_or_default(),
                        name: attrs.get_as_string("NAME").unwrap_or_default(),
                    },
                ))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        Ok(resolve_element_generation_root(
            requested,
            unit_types,
            |refno| nodes.get(&refno).cloned(),
        ))
    }

    /// Self -> OWNER -> ... chain, matching Plant UI's existing contract.
    ///
    /// 每一级从钉住会话点查 OWNER，跨库沿同一服务的路由目录读取。
    pub fn ancestors(&self, start: RefU64) -> anyhow::Result<Vec<RefnoEnum>> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = start;
        while current.0 != 0 {
            anyhow::ensure!(seen.insert(current), "OWNER cycle at {current}");
            anyhow::ensure!(chain.len() < 4096, "OWNER depth exceeded at {current}");
            let attrs = self.store.named_attmap(current)?;
            chain.push(RefnoEnum::from(current));
            match attrs.get_refu64("OWNER") {
                Some(owner) if owner.0 != 0 => current = owner,
                _ => break,
            }
        }
        Ok(chain)
    }

    /// RefNo 原生库号，并验证当前路由目录是否包含该库。
    pub fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
        self.store.dbnum_of(refno).ok().map(|dbnum| dbnum as u32)
    }

    /// 本服务钉住的只读视图，供需要 [`TreeStore`] 的写前校验用（`group_write::validate_ops`）。
    ///
    /// 校验读的是**写之前**的库状态，所以必须与这个服务同一只冻结会话——另开一个
    /// `MdbReadStore` 会看到别的 sesno，校验结论与即将写的库对不上。`Arc` 克隆不复制
    /// 数据，调用方可以放心搬进 `spawn_blocking`。
    ///
    /// 写侧不在这里发生：这个视图只读，`Transaction` 由 `group_exec` 自己开。
    pub fn tree_store(&self) -> Arc<MdbReadStore> {
        self.store.clone()
    }

    /// 这个元素的 noun（`TYPE`），与 [`Self::children`] 节点上的 `noun` 同一来源。
    ///
    /// 名称索引里没有类型，搜索结果要带 noun 只能回记录读一次；调用方按页调，别对
    /// 全集调。
    pub fn noun_of(&self, refno: RefU64) -> anyhow::Result<String> {
        let attrs = self.store.named_attmap(refno)?;
        Ok(attrs
            .get_as_string("TYPE")
            .unwrap_or_default()
            .trim()
            .to_string())
    }

    /// Search the NAME indexes of every DESI/ISOD member in this snapshot.
    pub fn search_names(&self, query: &str) -> anyhow::Result<Vec<(String, RefnoEnum, u32)>> {
        let mut out = Vec::new();
        for db in self.membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            out.extend(
                self.store
                    .indexes(db.dbnum as i32)?
                    .search_names(query)
                    .into_iter()
                    .map(|(name, refno)| (name, RefnoEnum::from(refno), db.dbnum)),
            );
        }
        out.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.to_pdms_str().cmp(&right.1.to_pdms_str()))
        });
        Ok(out)
    }

    fn node(
        &self,
        refno: RefU64,
        expected_owner: Option<RefU64>,
        order: usize,
    ) -> anyhow::Result<EleTreeNode> {
        // 树行四格（TYPE / NAME / OWNER / 成员数）全在已解析记录里：`tree_node_parts`
        // 一份 `parsed` 取齐，跳过整张属性表与描述符抽取——那才是展开一层的主要开销。
        // 与 `named_attmap` 三键同值（P2-1 对拍见 `generation_root.rs`）。
        let parts = self.store.tree_node_parts(refno)?;
        anyhow::ensure!(!parts.noun.is_empty(), "{refno} 的 direct TYPE 为空");
        let mut name = parts.name.trim().to_string();
        if name.is_empty() {
            name.clone_from(&parts.noun);
        }
        let owner = parts.owner;
        if let Some(expected) = expected_owner {
            anyhow::ensure!(
                owner == expected,
                "{refno} 的成员表 OWNER={expected}，元素记录 OWNER={owner}"
            );
        }
        let children_count = parts.members.len();
        Ok(EleTreeNode {
            refno: RefnoEnum::from(refno),
            noun: parts.noun,
            name,
            owner: RefnoEnum::from(owner),
            order: u16::try_from(order).unwrap_or(u16::MAX),
            children_count: u16::try_from(children_count).unwrap_or(u16::MAX),
            op: Default::default(),
            mod_cnt: None,
            children_updated: None,
            status_code: None,
        })
    }
}

/// `SCOSEL` 求值域：MDB 声明范围内**所有**有文件的元素库的索引，跨库聚合。
///
/// 为什么必须跨库（实测，`library/scosel-semantics.md` §3、`library/g3-rule-members.md` §5）：
/// APS 装 27 个规则组的库 `aps250204` 里 **0 个 ZONE**，2622 个 ZONE 分布在 68 个库。
/// 只在规则组所在库求值，`/AREA01_PIPE` 必得 0。
///
/// 求值域按 MDB 收窄（`membership.element_databases()` 里 `path.is_some()` 的库），不是全盘
/// 全扫：全盘会多出 MDB 未声明库里的同名副本（实测 `/AREA01_PIPE` 多 `2013287044_56`），
/// 那部分**不属于**求值域（契约 VAL-LIVE-001 的口径说明）。
///
/// 建不成的库（文件打不开 / 索引失败）不静默：进 `problems` 并 `log::warn!`，逐库点名。
/// 那条规则可能因此少命中，但至少能查到是谁缺的。
pub struct MdbScoselScope {
    /// 参与求值的库索引，按 MDB 声明序（命中集合本身与序无关，这里只为可复现的日志）。
    indexes: Vec<(u32, Arc<DbIndexes>)>,
    /// 建不成的库：`(dbnum, 原因)`。非空表示求值域缺了一块。
    problems: Vec<(u32, String)>,
}

impl MdbScoselScope {
    fn build(membership: &MdbMembership, store: &MdbReadStore) -> anyhow::Result<Self> {
        let mut indexes = Vec::new();
        let mut problems = Vec::new();
        for db in membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            match store.indexes(db.dbnum as i32) {
                Ok(index) => indexes.push((db.dbnum, index)),
                Err(error) => {
                    log::warn!(
                        "SCOSEL 求值域缺少库 {}（{}）：{error}",
                        db.dbnum,
                        db.name
                    );
                    problems.push((db.dbnum, error.to_string()));
                }
            }
        }
        Ok(Self { indexes, problems })
    }

    /// 参与求值的库号，按声明序——真库测试据此证明「域里有哪些库」。
    pub fn dbnums(&self) -> Vec<u32> {
        self.indexes.iter().map(|(dbnum, _)| *dbnum).collect()
    }

    /// 某一个库的索引；不在求值域里（没文件 / 建索引失败 / 不是元素库）回 `None`。
    ///
    /// 给真库测试与诊断用：逐库统计（例如「规则组所在的 `aps250204` 里 0 个 ZONE」）
    /// 只能按库问，跨库聚合的 trait 方法答不了这个问题。
    pub fn index_of(&self, dbnum: u32) -> Option<&DbIndexes> {
        self.indexes
            .iter()
            .find(|(db, _)| *db == dbnum)
            .map(|(_, index)| index.as_ref())
    }

    /// 建不成的库与原因；空表表示求值域完整。
    pub fn problems(&self) -> &[(u32, String)] {
        &self.problems
    }
}

/// 把懒建的 scope 槽转成 trait 对象引用（`Option<&Arc<T>>` → `Option<&dyn ScoselScope>`）。
///
/// 成员解析只要 `&dyn ScoselScope`，而服务手里存的是具体类型（建索引要它的 `problems()`
/// 与 `dbnums()`）。四个消费者共用这一处转换，省得每处写一遍 `as_ref() as &dyn`。
fn scope_ref(scope: &Option<Arc<MdbScoselScope>>) -> Option<&dyn ScoselScope> {
    scope.as_ref().map(|scope| scope.as_ref() as &dyn ScoselScope)
}

impl ScoselScope for MdbScoselScope {
    fn refnos_with_attr_text(&self, attr_hash: u32, text: &str) -> HashSet<RefU64> {
        self.indexes
            .iter()
            .flat_map(|(_, index)| index.refnos_of_attr_text(attr_hash, text))
            .collect()
    }

    fn refnos_of_noun(&self, noun_hash: u32) -> HashSet<RefU64> {
        self.indexes
            .iter()
            .flat_map(|(_, index)| index.refnos_of_noun_hash(noun_hash, None))
            .collect()
    }
}

/// `DirectStore` adapter of the shared generation-root traversal: `TYPE` / `NAME`
/// come from the direct attmap, members from the element record in stored order.
///
/// Free function so a single pinned store (no MDB, no `DirectTreeService`) can
/// be enumerated too — that is how the parity test in `generation_root.rs`
/// compares this read path with the e3d-io `DbSet` read path.
pub trait TreeStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError>;
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError>;

    /// 读一个元素的**显式 int 数组属性**（`SCOSEL` 就是这种），按属性名。
    ///
    /// 单独开一条通道，是因为 `named_attmap` 交的是 schema 投影后的兼容视图：`SCOSEL`
    /// 在 GPSET 的模板里没有声明，投影把它当「schema 外的自然值」塞进去，形状随库而变；
    /// 而规则组要的是**记录里的原字**（`scosel::decode` 对字节敏感）。所以这里直接取
    /// `explicit_attributes`，按 `db1_hash(attr)` 找那一项。
    ///
    /// 默认 `Ok(None)`：只走树 / 成员两条路的实现（`group_membership` 的测试替身、
    /// 写侧的只读视图）不需要为它多写一份。真实现见 `DirectStore` / `MdbReadStore`。
    fn explicit_int_array(
        &self,
        refno: RefU64,
        attr: &str,
    ) -> Result<Option<Vec<i32>>, DirectStoreError> {
        let _ = (refno, attr);
        Ok(None)
    }
}
impl TreeStore for DirectStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> {
        DirectStore::named_attmap(self, refno)
    }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        DirectStore::members(self, refno)
    }
    fn explicit_int_array(
        &self,
        refno: RefU64,
        attr: &str,
    ) -> Result<Option<Vec<i32>>, DirectStoreError> {
        DirectStore::explicit_int_array(self, refno, attr)
    }
}
impl TreeStore for MdbReadStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> {
        MdbReadStore::named_attmap(self, refno)
    }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        MdbReadStore::members(self, refno)
    }
    fn explicit_int_array(
        &self,
        refno: RefU64,
        attr: &str,
    ) -> Result<Option<Vec<i32>>, DirectStoreError> {
        MdbReadStore::explicit_int_array(self, refno, attr)
    }
}

pub fn generation_roots_in_subtree_on(
    store: &dyn TreeStore,
    root: RefU64,
    unit_types: &[String],
) -> anyhow::Result<Vec<GenerationRoot>> {
    enumerate_generation_roots_in_subtree(RefnoEnum::from(root), unit_types, |refno| {
        let refno = refno.refno();
        let attrs = store.named_attmap(refno)?;
        Ok(SubtreeElement {
            noun: attrs.get_as_string("TYPE").unwrap_or_default(),
            name: attrs.get_as_string("NAME").unwrap_or_default(),
            members: store
                .members(refno)?
                .into_iter()
                .map(RefnoEnum::from)
                .collect(),
        })
    })
}

/// Lazy wrapper so enabling the HTTP server does not scan project files until
/// a direct-tree endpoint is actually requested.
#[derive(Clone)]
pub struct DirectTreeProvider {
    project: String,
    mdb: String,
    service: Arc<tokio::sync::RwLock<Option<Arc<DirectTreeService>>>>,
    reload: Arc<tokio::sync::Mutex<()>>,
}

impl DirectTreeProvider {
    pub async fn cache_stats(&self) -> (Vec<i32>, usize) {
        self.service
            .read()
            .await
            .as_ref()
            .map(|service| {
                (
                    service.store.opened_dbnums(),
                    service.store.cached_elements(),
                )
            })
            .unwrap_or_default()
    }

    pub fn new(project: impl Into<String>, mdb: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            mdb: mdb.into(),
            service: Arc::new(tokio::sync::RwLock::new(None)),
            reload: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn get(&self) -> anyhow::Result<Arc<DirectTreeService>> {
        if let Some(service) = self.service.read().await.clone()
            && service.is_current()
        {
            return Ok(service);
        }
        let _reload = self.reload.lock().await;
        if let Some(service) = self.service.read().await.clone()
            && service.is_current()
        {
            return Ok(service);
        }
        let project = self.project.clone();
        let mdb = self.mdb.clone();
        let service = tokio::task::spawn_blocking(move || DirectTreeService::open(&project, &mdb))
            .await
            .context("direct tree 初始化任务异常结束")?
            .map(Arc::new)?;
        *self.service.write().await = Some(service.clone());
        Ok(service)
    }
}

impl DirectTreeService {
    /// 这个元素的 `NAME`（去掉首尾空白）；没有名字、或读不出记录都回 `None`。
    ///
    /// 与 [`Self::noun_of`] 同一来源、同一代价（回元素记录读一次），所以同样**只按页调**：
    /// `/api/v1/spatial/nearby` 只给本页（≤ 1000 条）补名字（spec §4.13；计划 §5-2 按 (a)）。
    pub fn name_of(&self, refno: RefU64) -> Option<String> {
        let attrs = self.store.named_attmap(refno).ok()?;
        let name = attrs.get_as_string("NAME")?.trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// 属主链上的一级：这个元素的 `TYPE` / `NAME` 与 `OWNER`，一次记录读（与 [`Self::name_of`] 同代价）。
    /// 空间查询派生「所属专业」沿它往上爬到 SITE（`fast_model::spatial_discipline`），逐级记忆后每个元素只读一次。
    /// 读不出记录回 `None`；`OWNER` 为 0 / 指向自己按到顶处理（`owner: None`）。
    pub fn lineage_step(
        &self,
        refno: RefU64,
    ) -> Option<crate::fast_model::spatial_discipline::LineageStep> {
        let attrs = self.store.named_attmap(refno).ok()?;
        let owner = attrs
            .get_refu64("OWNER")
            .filter(|owner| owner.0 != 0 && *owner != refno);
        Some(crate::fast_model::spatial_discipline::LineageStep {
            noun: attrs
                .get_as_string("TYPE")
                .unwrap_or_default()
                .trim()
                .to_string(),
            name: attrs
                .get_as_string("NAME")
                .unwrap_or_default()
                .trim()
                .to_string(),
            owner,
        })
    }

    /// 这份服务钉住的会话指纹的哈希：`spatial_discipline` 的共享记忆按它换代（会话前进 = 属主链可能变了）。
    /// 元数据读不出时回 0——记忆键退化成常数，语义是「不换代」，宁可多命中一次记忆也别每次查询都清空。
    pub fn lineage_epoch(&self) -> u64 {
        self.metadata()
            .map(|metadata| metadata.epoch())
            .unwrap_or(0)
    }
}

/// 解析视图里的一行：树行本身，加上它经由哪个 GPITEM 解出（`None` = 成员链上的元素本身），
/// 以及这一行是不是 `SCOSEL` 规则算出来的成员。
///
/// 为什么需要 `rule`：`via == None` 有两种完全不同的行——主链子节点（分组里没写成员，就是
/// 存储层级本身）与规则成员（分组里也没写这个成员，是规则把它算进来的）。只看 `via` 分不开，
/// 而响应契约要求前者**三个键都不出现**、后者必须是 `via: null` + `secondary: true` +
/// `rule: true`（`validation-contract.md` VAL-CONTRACT-001/003）。
#[derive(Debug, Clone)]
pub struct ResolvedChild {
    pub node: EleTreeNode,
    pub via: Option<RefU64>,
    /// 由 `ResolvedMember::Rule` 解出的成员。主链子节点与 GPITEM 解析成员都是 `false`。
    pub rule: bool,
}

/// [`DirectTreeService::resolved_children`] 的一层：解析出的行，与解析不出的 GPITEM 槽。
#[derive(Debug, Clone, Default)]
pub struct ResolvedChildren {
    pub nodes: Vec<ResolvedChild>,
    pub unresolved: Vec<UnresolvedSlot>,
}

/// [`DirectTreeService::group_generation_scope`] 的结果：分组管到的设计元素、它们去重后的生成根（按成员
/// 先后）、展开时遇到的坏槽。
#[derive(Debug, Clone, Default)]
pub struct GroupGenerationScope {
    pub members: Vec<RefU64>,
    pub roots: Vec<GenerationRoot>,
    pub unresolved: Vec<UnresolvedSlot>,
}

impl DirectTreeService {
    /// 解析视图的一层：分组（GPWL / GPSET）父节点的 GPITEM 换成 `GPREF` 指向的设计元素；
    /// 其余父节点与 [`Self::children`] 逐行相同。存储视图的树 / 子树 / 生成根遍历不经过这里。
    ///
    /// 解析成员的 `owner` 是它自己主链上的属主而不是这个分组，所以建行不校验属主；分组行的
    /// `children_count` 换成它解析后的成员数，展开箭头才与下一层对得上。
    pub fn resolved_children(&self, parent: RefU64) -> anyhow::Result<Arc<ResolvedChildren>> {
        if let Some(layer) = self
            .resolved_memo
            .lock()
            .map_err(|_| anyhow::anyhow!("resolved children memo lock poisoned"))?
            .get(&parent)
            .cloned()
        {
            return Ok(layer);
        }
        let layer = Arc::new(self.read_resolved_children(parent)?);
        self.resolved_memo
            .lock()
            .map_err(|_| anyhow::anyhow!("resolved children memo lock poisoned"))?
            .insert(parent, layer.clone());
        Ok(layer)
    }

    fn read_resolved_children(&self, parent: RefU64) -> anyhow::Result<ResolvedChildren> {
        if !is_group_noun(&self.noun_of(parent)?) {
            let nodes = self
                .children(parent)?
                .into_iter()
                // 非分组父节点的行就是主链子节点本身：没有经由的 GPITEM，也不是规则算出来的。
                .map(|node| ResolvedChild {
                    node,
                    via: None,
                    rule: false,
                })
                .collect();
            return Ok(ResolvedChildren {
                nodes,
                unresolved: Vec::new(),
            });
        }
        // 规则组的成员要按 `SCOSEL` 求值补上（G3）。`resolved_scope()` 在开关关着时给
        // `None`，于是这条路径与 G3 引入前逐字相同——这是契约快照「开关关」那一份的底座。
        let scope = self.resolved_scope()?;
        let GroupMembers {
            members,
            unresolved,
        } = resolved_members_with_rules(self.store.as_ref(), parent, scope_ref(&scope))?;
        let mut nodes = Vec::with_capacity(members.len());
        for (order, member) in members.into_iter().enumerate() {
            let expected_owner = match member {
                ResolvedMember::Owned(_) => Some(parent),
                // 规则成员与 GPREF 引用成员一样：它的属主是自己主链上的属主，不是这个分组。
                // 这里只做「不校验属主」的判定；`rule` 字段的发出由响应层（handlers）决定。
                ResolvedMember::Reference { .. } | ResolvedMember::Rule { .. } => None,
            };
            let mut node = self.node(member.element(), expected_owner, order)?;
            if is_group_noun(&node.noun) {
                let count = resolved_members_with_rules(
                    self.store.as_ref(),
                    member.element(),
                    scope_ref(&scope),
                )?
                .members
                .len();
                node.children_count = u16::try_from(count).unwrap_or(u16::MAX);
            }
            nodes.push(ResolvedChild {
                node,
                via: member.via(),
                // 规则成员的行要在响应里发 `rule: true`；其余两种都是 `false`。
                // 判定走 `ResolvedMember::is_rule()`，不在这里再 match 一次变体。
                rule: member.is_rule(),
            });
        }
        Ok(ResolvedChildren { nodes, unresolved })
    }

    /// 收录 `refno` 的分组（经哪个 GPITEM）；不在任何分组里回空表。
    ///
    /// 反查表首次调用时从各库 WORLD 扫一遍分组层级建好，之后与服务同寿命。
    pub fn secondary_parents(&self, refno: RefU64) -> anyhow::Result<Vec<SecondaryParent>> {
        Ok(self.group_index()?.parents_of(refno).to_vec())
    }

    /// `SCOSEL` 求值域（MDB 全量跨库），懒建、与冻结会话同寿命。
    ///
    /// 与 [`Self::group_index`] 同一套理由与形状（`Mutex<Option<Arc<_>>>`）：逐库索引钉在
    /// 本服务那只冻结会话上；文件一动 `is_current()` 就否、provider 换整只服务，于是
    /// scope 也随服务一起换——不跨服务携带，也不需要失效逻辑。
    ///
    /// 代价是一次建索引（APS 59 个有文件的元素库，实测 1.3 秒），所以只在该用的地方调：
    /// 开关关着时调用方不该碰它（`resolved_members_with_rules` 的第二层判据在 scope
    /// 已给的情况下才起作用，接线点因此要先看开关——见 [`Self::resolved_scope`]）。
    pub fn scosel_scope(&self) -> anyhow::Result<Arc<MdbScoselScope>> {
        let mut slot = self
            .scosel_scope
            .lock()
            .map_err(|_| anyhow::anyhow!("scosel scope lock poisoned"))?;
        if let Some(scope) = slot.as_ref() {
            return Ok(scope.clone());
        }
        let scope = Arc::new(MdbScoselScope::build(&self.membership, &self.store)?);
        if !scope.problems().is_empty() {
            log::warn!(
                "SCOSEL 求值域不完整：{} 个库建索引失败 {:?}",
                scope.problems().len(),
                scope.problems()
            );
        }
        *slot = Some(scope.clone());
        Ok(scope)
    }

    /// 交给成员解析的 scope：开关关着时 `None`（一个字节都不多读、一次索引都不建）。
    ///
    /// 开关判据放在这里而不是 `resolved_members_with_rules` 里再判一次，是因为建索引
    /// 有真实代价（APS 1.3 秒）：关着时连 scope 都不该建。`resolved_members_with_rules`
    /// 内部仍有同一道判据（它不假定调用方看过开关），两层都不成立才逐字不变。
    fn resolved_scope(&self) -> anyhow::Result<Option<Arc<MdbScoselScope>>> {
        if !crate::options::group_scosel() {
            return Ok(None);
        }
        Ok(Some(self.scosel_scope()?))
    }

    fn group_index(&self) -> anyhow::Result<Arc<GroupIndex>> {
        let mut slot = self
            .group_index
            .lock()
            .map_err(|_| anyhow::anyhow!("group index lock poisoned"))?;
        if let Some(index) = slot.as_ref() {
            return Ok(index.clone());
        }
        let metadata = self.metadata()?;
        let mut worlds = Vec::new();
        for db in self.membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            worlds.extend(metadata.worlds.get(&db.dbnum).copied());
        }
        // 反查表也要认规则成员：嵌套规则组（规则命中一个分组）要按解析成员继续往下展开。
        // 规则成员自己没有 `via`，不进这张表；`scope = None` 时与 G3 引入前逐字相同。
        let scope = self.resolved_scope()?;
        let index = Arc::new(GroupIndex::build_with_scope(
            self.store.as_ref(),
            &worlds,
            scope_ref(&scope),
        )?);
        *slot = Some(index.clone());
        Ok(index)
    }

    /// `root` 是分组（GPWL / GPSET）时展开到的设计元素（口径见 [`expand_group_on`]）；不是分组回 `None`。
    pub fn group_scope(&self, root: RefU64) -> anyhow::Result<Option<GroupScope>> {
        if !is_group_noun(&self.noun_of(root)?) {
            return Ok(None);
        }
        expand_group_with_rules_on(self.store.as_ref(), root, scope_ref(&self.resolved_scope()?))
            .map(Some)
    }

    /// 分组的生成范围：成员逐个走 [`Self::generation_roots_in_subtree`]，按生成根去重、保留成员先后。
    ///
    /// 分组只登记成员、不拥有模型：存储视图下它的子树里只有 GPITEM，直接对它解根什么也解不出。
    /// 成员全都解析不出或分组是空的，`roots` 就是空表——调用方按空范围回答，不再退回单根生成。
    /// `root` 不是分组回 `None`。
    pub fn group_generation_scope(
        &self,
        root: RefU64,
        unit_types: &[String],
    ) -> anyhow::Result<Option<GroupGenerationScope>> {
        let Some(GroupScope {
            members,
            unresolved,
        }) = self.group_scope(root)?
        else {
            return Ok(None);
        };
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        for &member in &members {
            for generation_root in self.generation_roots_in_subtree(member, unit_types)? {
                if seen.insert(generation_root.root) {
                    roots.push(generation_root);
                }
            }
        }
        Ok(Some(GroupGenerationScope {
            members,
            roots,
            unresolved,
        }))
    }

    /// [`Self::generation_roots_in_subtree`] 的分组感知版：分组换成它成员的生成根，其余逐字相同。
    pub fn resolved_generation_roots_in_subtree(
        &self,
        root: RefU64,
        unit_types: &[String],
    ) -> anyhow::Result<Vec<GenerationRoot>> {
        match self.group_generation_scope(root, unit_types)? {
            Some(scope) => Ok(scope.roots),
            None => self.generation_roots_in_subtree(root, unit_types),
        }
    }
}

#[cfg(test)]
mod lazy_live_tests {
    use super::*;

    impl DirectTreeService {
        /// Test seam: the shared read view this service is pinned to, so the cross-service
        /// sharing proof in `element_attributes` can compare `Arc` identity instead of statistics.
        /// Lives under `#[cfg(test)]` here (not on the production impl) because the source guards
        /// cut production code at the first `#[cfg(test)]` marker.
        pub(crate) fn read_view(&self) -> Arc<MdbReadStore> {
            self.store.clone()
        }
    }

    #[test]
    #[ignore = "requires local AMS source files"]
    fn ams_tree_first_read_does_not_scan_elements() {
        e3d_io::read_stats::reset();
        let service = DirectTreeService::open("AvevaMarineSample", "ALL").unwrap();
        let roots = service.roots().unwrap();
        assert!(!roots.is_empty());
        let chain = service
            .ancestors(RefU64::from_two_nums(24384, 23257))
            .unwrap();
        assert!(chain.len() > 1);
        let stats = e3d_io::read_stats::snapshot();
        assert_eq!(stats.scan_calls, 0, "{stats:?}");
        assert_eq!(stats.scanned_elements, 0);
        eprintln!(
            "AMS roots={} ancestors={} costs={}",
            roots.len(),
            chain.len(),
            stats.summary()
        );
    }

    // ── G3 接线：SCOSEL 求值域 ─────────────────────────────────────────────────

    /// VAL-WIRE-001：`scosel_scope()` 首次调用建、之后复用（`Arc::ptr_eq` 为真）。
    ///
    /// 形状与 `group_index` 同款（`Mutex<Option<Arc<T>>>`）：逐库索引钉在这只冻结会话上，
    /// 文件一动 provider 就换整只服务，所以 scope 与服务同寿命即正确，不需要失效逻辑。
    #[test]
    #[ignore = "requires local APS source files"]
    fn the_scosel_scope_is_built_once_and_shared_for_the_service_lifetime() {
        let service = DirectTreeService::open("AvevaPlantSample", "ALL").unwrap();
        let first = service.scosel_scope().unwrap();
        let second = service.scosel_scope().unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "第二次调用必须复用同一个 Arc——重建一次索引是 1.3 秒，不能每次请求都付"
        );
        // 求值域必须覆盖 MDB 里所有有文件的元素库：缺的库要么在域里，要么在 problems 里点名。
        let element_databases = service
            .membership
            .element_databases()
            .filter(|db| db.path.is_some())
            .count();
        assert_eq!(
            first.dbnums().len() + first.problems().len(),
            element_databases,
            "有文件的元素库 {} 个，域 {} 个 + 建不成 {} 个",
            element_databases,
            first.dbnums().len(),
            first.problems().len()
        );
        assert!(first.problems().is_empty(), "{:?}", first.problems());
        eprintln!(
            "SCOSEL 求值域：{} 个库（MDB 声明 {} 个有文件的元素库）",
            first.dbnums().len(),
            element_databases
        );
    }

    /// VAL-WIRE-002：求值域是 MDB 全量**跨库**，不是规则组所在的单个库。
    ///
    /// 实测口径（`library/scosel-semantics.md` §3）：APS 装 27 个规则组的库 `aps250204`
    /// 里 **0 个 ZONE**。按 MDB 声明范围收窄后实测 ZONE 共 1088 个、分布在 36 个库
    /// （全盘全扫的 2622 个 / 68 个库包含 MDB 未声明库，那不是求值域）。只在规则组那一库
    /// 求值，`/AREA01_PIPE` 必然回 0——所以「命中非空」本身就是跨库求值的证明。
    ///
    /// 域外副本必须显式报告（AGENTS.md 实现约束 9）：全盘全扫会命中 MDB 未声明库里的
    /// 同名 `2013287044_56`，它不属于求值域，这里把它点出来而不是静默丢弃。
    #[test]
    #[ignore = "requires local APS source files"]
    fn the_scosel_scope_evaluates_across_databases_not_inside_the_rule_group_database() {
        let service = DirectTreeService::open("AvevaPlantSample", "ALL").unwrap();
        let scope = service.scosel_scope().unwrap();
        let zone_hash = e3d_attlib::db1_hash("ZONE");
        let desc_hash = e3d_attlib::db1_hash("DESC");

        // 装规则组的库：文件名叫 aps250204 的那一个（27 个规则组在它里面）。
        let group_db = service
            .membership
            .element_databases()
            .filter(|db| db.path.is_some())
            .find(|db| {
                db.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("aps250204"))
            })
            .expect("APS 应有装规则组的库 aps250204");
        let zones_in_group_db = scope
            .index_of(group_db.dbnum)
            .map(|index| index.refnos_of_noun_hash(zone_hash, None).len())
            .unwrap_or(0);
        assert_eq!(
            zones_in_group_db, 0,
            "库 {} 里 0 个 ZONE——只在单库求值必然为 0，这是「必须跨库」的实证",
            group_db.dbnum
        );

        let mut zone_total = 0usize;
        let mut zone_databases = 0usize;
        for dbnum in scope.dbnums() {
            let count = scope
                .index_of(dbnum)
                .expect("dbnums 与 index_of 必须自洽")
                .refnos_of_noun_hash(zone_hash, None)
                .len();
            if count > 0 {
                zone_databases += 1;
                zone_total += count;
            }
        }
        eprintln!(
            "ZONE 共 {zone_total} 个，分布在 {zone_databases} 个库；规则组所在库 {} 里 0 个",
            group_db.dbnum
        );
        assert!(
            zone_databases > 1,
            "ZONE 必须跨多个库（实测 68 个），只在一个库里说明域没收全"
        );

        let mut hits: Vec<RefU64> = scope
            .refnos_with_attr_text(desc_hash, "AREA01PIPE")
            .into_iter()
            .collect();
        hits.sort_unstable_by_key(|refno| refno.0);
        assert!(
            !hits.is_empty(),
            "跨库求值 /AREA01_PIPE 必须命中非空（单库求值会是 0）"
        );
        eprintln!(
            "DESC == AREA01PIPE 在求值域内命中 {} 个：{:?}",
            hits.len(),
            hits
        );

        // 域外副本：全盘全扫会多出的那一个，它所在的库不在 MDB 声明范围里。
        let outside = RefU64::from_two_nums(2013287044, 56);
        // 库号走路由目录问，不要拿 refno 的高位字当库号——那只是 refno 的编码，不是 dbnum。
        let outside_dbnum = service.dbnum_of(outside);
        assert!(
            !hits.contains(&outside),
            "MDB 未声明库里的同名副本 {outside} 不得进求值域"
        );
        eprintln!(
            "域外副本 {outside} 属于库 {outside_dbnum:?}，该库在求值域里？{}",
            outside_dbnum.is_some_and(|dbnum| scope.index_of(dbnum).is_some())
        );
        assert!(
            outside_dbnum.is_none_or(|dbnum| scope.index_of(dbnum).is_none()),
            "域外副本所在的库不该在求值域里（{outside_dbnum:?}）"
        );
    }

    /// 接线：三个消费者（`read_resolved_children` / `expand_group_on` / `GroupIndex::build`）
    /// 在开关开时都拿到规则成员。
    ///
    /// **进程级开关的并发陷阱**：`GroupScoselOverride` 是进程级静态量。若本测试与要求
    /// 「规则组解析出 0 行」的既有真库测试（`aps_group_worlds_are_navigable_in_the_resolved_view`）
    /// 在同一进程里并发跑，两边都会随机失败。所以这条测试**默认跳过**：只有显式设
    /// `AIOS_RUN_SCOPE_WIRING_LIVE=1` 才真跑（跳过时打印原因，不静默）。
    ///
    /// 取证命令（单条过滤 + 单线程，两条要求互斥的测试都覆盖到）：
    ///
    /// ```text
    /// $env:AIOS_RUN_SCOPE_WIRING_LIVE = "1"
    /// cargo test --lib data_interface::direct_tree::lazy_live_tests::the_three_consumers \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    #[test]
    #[ignore = "requires local APS source files; set AIOS_RUN_SCOPE_WIRING_LIVE=1 (process-wide switch override)"]
    fn the_three_consumers_see_rule_members_when_the_switch_is_on() {
        if std::env::var("AIOS_RUN_SCOPE_WIRING_LIVE").ok().as_deref() != Some("1") {
            eprintln!(
                "跳过 the_three_consumers_see_rule_members_when_the_switch_is_on：\
                 未设 AIOS_RUN_SCOPE_WIRING_LIVE=1。本条要用进程级开关覆盖，\
                 与要求「规则组解析出 0 行」的既有真库测试并发会互相干扰。"
            );
            return;
        }
        let _on = crate::options::GroupScoselOverride::set(true);
        let service = DirectTreeService::open("AvevaPlantSample", "ALL").unwrap();
        // `/AREA01_PIPE` = 2013286748_2（`library/g3-rule-members.md` §5）。
        let gpset = RefU64::from_two_nums(2013286748, 2);

        let layer = service.resolved_children(gpset).unwrap();
        assert!(
            !layer.nodes.is_empty(),
            "/AREA01_PIPE 的解析视图必须非空（接线前它是 0 行）"
        );
        assert!(
            layer
                .nodes
                .iter()
                .all(|row| row.node.noun.eq_ignore_ascii_case("ZONE")),
            "{:?}",
            layer
                .nodes
                .iter()
                .map(|row| (&row.node.noun, row.via))
                .collect::<Vec<_>>()
        );
        assert!(
            layer.nodes.iter().all(|row| row.via.is_none()),
            "规则成员没有经由的 GPITEM，via 必须是 None"
        );
        assert!(
            layer.nodes.iter().all(|row| row.rule),
            "这些行全是 SCOSEL 规则算出来的，`rule` 必须是 true——否则响应层发不出 `rule: true`"
        );

        let group_scope = service
            .group_scope(gpset)
            .unwrap()
            .expect("GPSET 是分组，group_scope 必须给 Some");
        assert!(
            !group_scope.members.is_empty(),
            "生成 / 记录的范围必须非空（这条路的消费者是 expand_group_with_rules_on）"
        );

        // 反查表那一路（`GroupIndex::build_with_scope`）：规则成员不该凭空多出一条二级收录。
        for row in &layer.nodes {
            assert!(
                service
                    .secondary_parents(row.node.refno.refno())
                    .unwrap()
                    .is_empty(),
                "规则成员没有经由的 GPITEM，反查表不该给它编一条：{}",
                row.node.refno
            );
        }
        eprintln!(
            "/AREA01_PIPE：resolved={} 个 ZONE，group_scope.members={} 个，unresolved={:?}",
            layer.nodes.len(),
            group_scope.members.len(),
            layer.unresolved
        );
    }
}
