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

use super::direct_store::{DirectStore, DirectStoreError};
use super::generation_root::{
    GenerationNode, GenerationRoot, SubtreeElement, enumerate_generation_roots_in_subtree,
    resolve_element_generation_root,
};
use super::group_membership::{
    GroupIndex, GroupMembers, ResolvedMember, SecondaryParent, UnresolvedSlot, is_group_noun,
    resolved_members_on,
};
use super::mdb_membership::MdbMembership;
use super::mdb_read_store::{MdbReadProvider, MdbReadStore};

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

/// `DirectStore` adapter of the shared generation-root traversal: `TYPE` / `NAME`
/// come from the direct attmap, members from the element record in stored order.
///
/// Free function so a single pinned store (no MDB, no `DirectTreeService`) can
/// be enumerated too — that is how the parity test in `generation_root.rs`
/// compares this read path with the e3d-io `DbSet` read path.
pub trait TreeStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError>;
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError>;
}
impl TreeStore for DirectStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> {
        DirectStore::named_attmap(self, refno)
    }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        DirectStore::members(self, refno)
    }
}
impl TreeStore for MdbReadStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> {
        MdbReadStore::named_attmap(self, refno)
    }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        MdbReadStore::members(self, refno)
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

/// 解析视图里的一行：树行本身，加上它经由哪个 GPITEM 解出（`None` = 成员链上的元素本身）。
#[derive(Debug, Clone)]
pub struct ResolvedChild {
    pub node: EleTreeNode,
    pub via: Option<RefU64>,
}

/// [`DirectTreeService::resolved_children`] 的一层：解析出的行，与解析不出的 GPITEM 槽。
#[derive(Debug, Clone, Default)]
pub struct ResolvedChildren {
    pub nodes: Vec<ResolvedChild>,
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
                .map(|node| ResolvedChild { node, via: None })
                .collect();
            return Ok(ResolvedChildren {
                nodes,
                unresolved: Vec::new(),
            });
        }
        let GroupMembers {
            members,
            unresolved,
        } = resolved_members_on(self.store.as_ref(), parent)?;
        let mut nodes = Vec::with_capacity(members.len());
        for (order, member) in members.into_iter().enumerate() {
            let expected_owner = match member {
                ResolvedMember::Owned(_) => Some(parent),
                ResolvedMember::Reference { .. } => None,
            };
            let mut node = self.node(member.element(), expected_owner, order)?;
            if is_group_noun(&node.noun) {
                let count = resolved_members_on(self.store.as_ref(), member.element())?
                    .members
                    .len();
                node.children_count = u16::try_from(count).unwrap_or(u16::MAX);
            }
            nodes.push(ResolvedChild {
                node,
                via: member.via(),
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
        let index = Arc::new(GroupIndex::build(self.store.as_ref(), &worlds)?);
        *slot = Some(index.clone());
        Ok(index)
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
}
