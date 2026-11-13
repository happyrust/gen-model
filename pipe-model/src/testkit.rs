//! 测试工具：内存 `PipeCatalog`/`SpecCatalog` mock。
//!
//! 只在 `#[cfg(test)]` 编译。编辑段 refno 从 1_000_000 起递增分配
//! （与摄入行号段隔离语义的测试版）。

use crate::catalog::PipeCatalog;
use crate::choice::QuestionAnswers;
use crate::context::{Context, Flow};
use crate::spec_resolver::SpecCatalog;
use aios_core::{NamedAttrValue, RefU64};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

/// 内存 mock：数据面最小读集。
#[derive(Default)]
pub struct MockCatalog {
    pub nouns: HashMap<RefU64, String>,
    pub names: HashMap<RefU64, String>,
    pub owners: HashMap<RefU64, RefU64>,
    pub members: HashMap<RefU64, Vec<RefU64>>,
    pub attrs: HashMap<RefU64, BTreeMap<String, NamedAttrValue>>,
    pub writable: HashMap<RefU64, bool>,
    counter: AtomicU64,
}

impl MockCatalog {
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(100),
            ..Default::default()
        }
    }

    pub fn zone(&mut self, refno: RefU64, name: &str) -> &mut Self {
        self.nouns.insert(refno, "ZONE".into());
        self.names.insert(refno, name.into());
        self.writable.insert(refno, true);
        self
    }

    pub fn pipe(&mut self, refno: RefU64, name: &str, spec: RefU64, bore: f32) -> &mut Self {
        self.nouns.insert(refno, "PIPE".into());
        self.names.insert(refno, name.into());
        self.writable.insert(refno, true);
        let mut attrs = BTreeMap::new();
        attrs.insert("PSPE".into(), NamedAttrValue::RefU64Type(spec));
        attrs.insert("BORE".into(), NamedAttrValue::F32Type(bore));
        self.attrs.insert(refno, attrs);
        self
    }

    /// BRAN（挂在 pipe 下，带 PSPE 继承）。
    pub fn branch(&mut self, refno: RefU64, name: &str, pipe: RefU64, spec: RefU64) -> &mut Self {
        self.nouns.insert(refno, "BRAN".into());
        self.names.insert(refno, name.into());
        self.owners.insert(refno, pipe);
        self.writable.insert(refno, true);
        self.members.entry(pipe).or_default().push(refno);
        let mut attrs = BTreeMap::new();
        attrs.insert("PSPE".into(), NamedAttrValue::RefU64Type(spec));
        self.attrs.insert(refno, attrs);
        self
    }
}

impl PipeCatalog for MockCatalog {
    fn noun_of(&self, r: RefU64) -> Option<String> {
        self.nouns.get(&r).cloned()
    }
    fn name_of(&self, r: RefU64) -> Option<String> {
        self.names.get(&r).cloned()
    }
    fn owner_of(&self, r: RefU64) -> Option<RefU64> {
        self.owners.get(&r).copied()
    }
    fn members_of(&self, r: RefU64) -> Vec<RefU64> {
        self.members.get(&r).cloned().unwrap_or_default()
    }
    fn attr(&self, r: RefU64, n: &str) -> Option<NamedAttrValue> {
        self.attrs.get(&r).and_then(|m| m.get(n)).cloned()
    }
    fn attmap_of(&self, r: RefU64) -> Option<aios_core::NamedAttrMap> {
        self.attrs
            .get(&r)
            .map(|m| aios_core::NamedAttrMap { map: m.clone() })
    }
    fn name_exists(&self, n: &str) -> bool {
        self.names.values().any(|v| v == n)
    }
    fn refno_by_name(&self, n: &str) -> Option<RefU64> {
        self.names
            .iter()
            .find(|(_, v)| v.as_str() == n)
            .map(|(k, _)| *k)
    }
    fn writable(&self, r: RefU64) -> bool {
        *self.writable.get(&r).unwrap_or(&false)
    }
    fn alloc_refno(&self, _under: RefU64) -> RefU64 {
        RefU64(1_000_000 + self.counter.fetch_add(1, Ordering::SeqCst))
    }
}

/// 规格库 mock：直查表 + SELE 链。
///
/// `table` 命中即直查返回；`sele_next` 记录 SELE 节点 → 后继节点（测试 SELE
/// 回落链时挂 `sele → spco` 边并放进 `spco_set`）。
#[derive(Default)]
pub struct MockSpec {
    pub table: HashMap<(RefU64, String), RefU64>,
    pub roots: HashMap<RefU64, RefU64>,
    pub sele_next: HashMap<RefU64, RefU64>,
    pub spco_set: std::collections::HashSet<RefU64>,
    /// SPCO → 答卷（`derive_answers` 的测试数据源）。
    pub answers_map: HashMap<RefU64, QuestionAnswers>,
}

impl SpecCatalog for MockSpec {
    fn root_selector(&self, spref: RefU64) -> Option<RefU64> {
        self.roots.get(&spref).copied()
    }
    fn table_lookup(
        &self,
        spref: RefU64,
        comp_type: &str,
        _qa: &QuestionAnswers,
    ) -> Option<RefU64> {
        self.table.get(&(spref, comp_type.to_string())).copied()
    }
    fn sele_descend(&self, selector: RefU64, _qa: &QuestionAnswers) -> Option<RefU64> {
        self.sele_next.get(&selector).copied()
    }
    fn is_spco(&self, node: RefU64) -> bool {
        self.spco_set.contains(&node)
    }
    fn derive_answers(&self, spco: RefU64) -> Option<QuestionAnswers> {
        self.answers_map.get(&spco).cloned()
    }
}

pub fn ctx() -> Context {
    Context {
        element: RefU64(1),
        dbnum: 7001,
        flow: Flow::Forwards,
        end: None,
    }
}
