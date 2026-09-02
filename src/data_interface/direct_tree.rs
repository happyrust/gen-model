//! e3d-io backed model-tree reads for the Web API.
//!
//! This module deliberately does not query `pe`, `pe_owner`, or any other
//! SurrealDB element table.  MDB membership comes from the SYS file, and every
//! displayed node is decoded from the selected DESI database by [`DirectStore`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aios_core::pdms_types::EleTreeNode;
use aios_core::{RefU64, RefnoEnum};
use anyhow::Context;

use super::cata_closure::InMemoryCataLocator;
use super::direct_store::{DbPin, DirectSchema, DirectStore};
use super::generation_root::{
    GenerationRoot, SubtreeElement, enumerate_generation_roots_in_subtree,
};
use super::mdb_membership::{DESI_STYP, MdbMembership};

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
}

impl DirectTreeService {
    /// Resolve the MDB from its SYS file and build the e3d-io ref0 locator from
    /// the configured project directory.  No element row is read from SurrealDB.
    pub fn open(project: &str, mdb: &str) -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        let membership = super::mdb_membership::get(project, mdb)
            .map(Ok)
            .unwrap_or_else(|| {
                super::mdb_membership::resolve(option, project, mdb).map(Arc::new)
            })?;
        // Build the affiliation map from the same e3d-io live indexes that the
        // tree reads.  The generic directory locator uses the legacy pdms-io
        // scanner and also walks unrelated project databases; tree direct mode
        // must neither depend on that parser nor scan outside the MDB's CURD.
        let mut affiliations = HashMap::new();
        let mut files = HashMap::new();
        for db in membership.of_type(DESI_STYP) {
            let Some(path) = db.path.clone() else {
                continue;
            };
            let mut engine = e3d_io::ReadOnlyEngine::open(&path)
                .with_context(|| format!("e3d-io 打开 DESI {} ({})", db.dbnum, path.display()))?;
            for (refno, _, _) in engine.indexed_refnos()? {
                let ref0 = refno.word0;
                if let Some(previous) = affiliations.insert(ref0, db.dbnum) {
                    anyhow::ensure!(
                        previous == db.dbnum,
                        "MDB {} 的 ref0 {} 同时属于 DESI {} 和 {}",
                        membership.mdb(),
                        ref0,
                        previous,
                        db.dbnum
                    );
                }
            }
            files.insert(db.dbnum, ("DESI".to_string(), project.to_string(), path));
        }
        let locator = Arc::new(InMemoryCataLocator::from_parts(affiliations, files));
        anyhow::ensure!(
            locator.ref0_count() > 0,
            "direct tree 在 MDB {} 的 DESI CURD 中没有建立出任何 ref0 定位记录",
            membership.mdb()
        );
        let schema = Arc::new(DirectSchema::open_from_env()?);
        let store = Arc::new(DirectStore::new(schema, locator));

        let mut pinned = 0usize;
        for db in membership.of_type(DESI_STYP) {
            let Some(file) = db.path.clone() else {
                continue;
            };
            store.pin(DbPin {
                dbnum: db.dbnum as i32,
                db_type: "DESI".to_string(),
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
        })
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    pub fn dbnum_of(&self, refno: RefU64) -> anyhow::Result<i32> {
        self.store.dbnum_of(refno).map_err(Into::into)
    }

    /// SITE rows in MDB CURD order, then in each WORL's stored member order.
    pub fn roots(&self) -> anyhow::Result<Vec<EleTreeNode>> {
        let mut out = Vec::new();
        for db in self.membership.of_type(DESI_STYP) {
            if db.path.is_none() {
                continue;
            }
            let dbnum = db.dbnum as i32;
            let indexes = self.store.indexes(dbnum)?;
            for world in indexes.refnos_of_noun("WORL", Some(true)) {
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
        generation_roots_in_subtree_on(&self.store, root, unit_types)
    }

    /// Self -> OWNER -> ... chain, matching Plant UI's existing contract.
    pub fn ancestors(&self, start: RefU64) -> anyhow::Result<Vec<RefnoEnum>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut current = start;
        for _ in 0..256 {
            anyhow::ensure!(seen.insert(current), "OWNER 链成环于 {current}");
            out.push(RefnoEnum::from(current));
            let attrs = self.store.named_attmap(current)?;
            let Some(owner) = attrs.get_refu64("OWNER").filter(|owner| owner.0 != 0) else {
                return Ok(out);
            };
            current = owner;
        }
        anyhow::bail!("OWNER 链超过 256 层，起点 {start}")
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
    service: Arc<tokio::sync::OnceCell<Arc<DirectTreeService>>>,
}

impl DirectTreeProvider {
    pub fn new(project: impl Into<String>, mdb: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            mdb: mdb.into(),
            service: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    pub async fn get(&self) -> anyhow::Result<Arc<DirectTreeService>> {
        let project = self.project.clone();
        let mdb = self.mdb.clone();
        self.service
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || DirectTreeService::open(&project, &mdb))
                    .await
                    .context("direct tree 初始化任务异常结束")?
                    .map(Arc::new)
            })
            .await
            .cloned()
    }
}
