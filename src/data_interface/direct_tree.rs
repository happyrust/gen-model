//! e3d-io backed model-tree reads for the Web API.
//!
//! This module deliberately does not query `pe`, `pe_owner`, or any other
//! SurrealDB element table.  MDB membership comes from the SYS file, and every
//! displayed node is decoded from the selected DESI database by [`DirectStore`].
//!
//! 骨架（哪个 ref0 属于哪个库、谁是谁的属主、每个元素是什么 noun）来自进程级的
//! [`MdbMemoryStore`]，启动时预热好；本模块只在需要**属性**（NAME、成员表）时才
//! 落到 [`DirectStore`]。此前这三样是本模块自己在第一次树请求时逐库
//! `indexed_refnos()` 现扫的，而且 owner 链每上一级要抽一整条记录。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aios_core::pdms_types::EleTreeNode;
use aios_core::{RefU64, RefnoEnum};
use anyhow::Context;

use super::direct_store::{DbPin, DirectSchema, DirectStore};
use super::generation_root::{
    GenerationNode, GenerationRoot, SubtreeElement, enumerate_generation_roots_in_subtree,
    resolve_element_generation_root,
};
use super::mdb_membership::{ISOD_STYP, MdbMembership};
use super::mdb_memory_store::MdbMemoryStore;
use super::mdb_route_directory::MdbRouteDirectory;

/// One process-local, immutable view of the current MDB model tree.
///
/// Each DESI file is opened once at the latest session visible when first read.
/// `DirectStore` then freezes that session for the lifetime of this service, so
/// roots, children, and ancestor requests cannot accidentally mix sessions.
pub struct DirectTreeService {
    project: String,
    mdb: String,
    membership: Arc<MdbMembership>,
    store: Arc<DirectStore>,
    skeleton: Arc<MdbMemoryStore>,
    skeleton_fingerprint: Vec<(u32, std::path::PathBuf, u32)>,
}

impl DirectTreeService {
    /// Resolve the MDB from its SYS file and take the e3d-io ref0 locator from
    /// the process-level skeleton.  No element row is read from SurrealDB.
    ///
    /// 骨架预热过就是一次 `Arc` 克隆；没预热过（预热失败、或探针 / 单测这类不走
    /// 启动链的进程）由 `ensure` 现建一份，代价就是此前每次 open 都要付的那一遍扫描。
    pub fn open(project: &str, mdb: &str) -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        let membership = super::mdb_membership::get(project, mdb)
            .map(Ok)
            .unwrap_or_else(|| {
                super::mdb_membership::resolve(option, project, mdb).map(Arc::new)
            })?;
        let skeleton = super::mdb_memory_store::ensure(project, mdb)?;
        let schema = Arc::new(DirectSchema::open_from_env()?);
        // 定位器是 dbnum → 文件的路由目录，直接从 CURD 成员清单造（切片 W6）：refno 归哪个库由
        // `DirectStore` 位解码，不再从骨架抄 `ref0 → dbnum`。骨架在这里只剩 WORL 枚举
        // （`roots()` 的 `refnos_of_noun`）与 noun / owner 查询在用——那是母计划 3B / 3C 的活。
        let directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
        anyhow::ensure!(
            directory.element_databases().next().is_some(),
            "direct tree 在 MDB {} 的 CURD 里没有路由出任何 DESI / ISOD 库文件",
            membership.mdb()
        );
        let store = Arc::new(DirectStore::new(schema, Arc::new(directory)));

        let mut pinned = 0usize;
        for db in membership.element_databases() {
            let Some(file) = db.path.clone() else {
                continue;
            };
            store.pin(DbPin {
                dbnum: db.dbnum as i32,
                db_type: if db.styp == ISOD_STYP { "ISOD" } else { "DESI" }.to_string(),
                file,
                // Direct tree is compared with the current E3D TTY view.  None
                // resolves the file's latest session once, then freezes it.
                sesno: None,
            });
            pinned += 1;
        }
        anyhow::ensure!(pinned > 0, "MDB {} 没有可读的 DESI 文件", membership.mdb());

        Ok(Self {
            project: project.to_string(),
            mdb: format!("/{}", mdb.trim_start_matches('/')),
            membership,
            store,
            skeleton_fingerprint: skeleton.session_fingerprint(),
            skeleton,
        })
    }

    /// 这次服务背后的骨架。
    pub fn skeleton(&self) -> anyhow::Result<Arc<MdbMemoryStore>> {
        Ok(self.skeleton.clone())
    }

    fn is_current(&self) -> bool {
        super::mdb_memory_store::get(&self.project, &self.mdb)
            .is_some_and(|current| current.session_fingerprint() == self.skeleton_fingerprint)
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    /// SITE rows in MDB CURD order, then in each WORL's stored member order.
    ///
    /// WORL 从骨架查，不再触发 [`DirectStore::indexes`]——那是每库一次全树扫描加
    /// 描述符抽取，而这里只要「哪些元素是 WORL」。骨架看不到「成员表非空」这一格
    /// （头扫描不解析成员表），少这一格结果不变：成员表空的 WORL 下面一个 SITE 都
    /// 没有，过不过滤都贡献零行。两边都按 refno 升序交，顺序也不变。
    pub fn roots(&self) -> anyhow::Result<Vec<EleTreeNode>> {
        let skeleton = self.skeleton()?;
        let mut out = Vec::new();
        for db in self.membership.element_databases() {
            if db.path.is_none() {
                continue;
            }
            let dbnum = db.dbnum as i32;
            for world in skeleton.refnos_of_noun(db.dbnum, "WORL") {
                for (order, child) in self.store.members_in(dbnum, world)?.into_iter().enumerate() {
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
    pub fn children(&self, parent: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        self.store
            .members(parent)?
            .into_iter()
            .enumerate()
            .map(|(order, child)| self.node(child, Some(parent), order))
            .collect()
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
        // Give the HTTP layer the same typed absence signal as direct attribute reads.
        // `MdbMemoryStore::ancestors` carries a diagnostic-rich anyhow message, but that
        // erased the category and `/model/ensure` exposed a deleted element as HTTP 500.
        if !self.skeleton.contains(root) {
            let dbnum = self
                .skeleton
                .dbnum_of_ref0(root.get_0())
                .map(|value| value as i32)
                .unwrap_or_default();
            return Err(super::direct_store::DirectStoreError::NoSuchElement {
                refno: RefnoEnum::from(root).to_string(),
                dbnum,
                sesno: None,
            }
            .into());
        }
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
        if let Some(ancestor) = resolve_element_generation_root(requested, unit_types, |refno| {
            nodes.get(&refno).cloned()
        }) && ancestor.root != requested
        {
            return Ok(vec![ancestor]);
        }
        generation_roots_in_subtree_on(&self.store, root, unit_types)
    }

    /// Self -> OWNER -> ... chain, matching Plant UI's existing contract.
    ///
    /// 走骨架里的属主表，全程零解析。此前每上一级要抽一整条记录再转一张属性表，
    /// 只为读一个 `OWNER`——而这条链是 `generation_roots_in_subtree` 每次按需生成
    /// 都要先走一遍的东西。
    pub fn ancestors(&self, start: RefU64) -> anyhow::Result<Vec<RefnoEnum>> {
        Ok(self
            .skeleton()?
            .ancestors(start)?
            .into_iter()
            .map(RefnoEnum::from)
            .collect())
    }

    /// 这个元素归属的库号（骨架的 `ref0 → dbnum`）。`None` = 它的 Ref0 不在本 MDB 里。
    ///
    /// 给 HTTP 层往树节点上外包 `dbnum` 用：refno 前半段只是 Ref0，浏览器端拿它当库号
    /// 分桶会分错，库号只有这里答得出。
    pub fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
        self.skeleton.dbnum_of(refno)
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
        let attrs = self.store.named_attmap(refno)?;
        let noun = attrs
            .get_as_string("TYPE")
            .unwrap_or_default()
            .trim()
            .to_string();
        anyhow::ensure!(!noun.is_empty(), "{refno} 的 direct TYPE 为空");
        let mut name = attrs
            .get_as_string("NAME")
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            name.clone_from(&noun);
        }
        let owner = attrs.get_refu64("OWNER").unwrap_or_default();
        if let Some(expected) = expected_owner {
            anyhow::ensure!(
                owner == expected,
                "{refno} 的成员表 OWNER={expected}，元素记录 OWNER={owner}"
            );
        }
        let children_count = self.store.members(refno)?.len();
        Ok(EleTreeNode {
            refno: RefnoEnum::from(refno),
            noun,
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
pub fn generation_roots_in_subtree_on(
    store: &DirectStore,
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
    pub fn new(project: impl Into<String>, mdb: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            mdb: mdb.into(),
            service: Arc::new(tokio::sync::RwLock::new(None)),
            reload: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn get(&self) -> anyhow::Result<Arc<DirectTreeService>> {
        if let Some(reason) = super::mdb_memory_store::stale_reason(&self.project, &self.mdb) {
            anyhow::bail!(
                "MDB_SNAPSHOT_STALE: latest file session could not be installed: {reason}"
            );
        }
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
