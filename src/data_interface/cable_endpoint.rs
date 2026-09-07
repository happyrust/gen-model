//! C8（同库版）—— direct 窗口规划器的电缆端点 / 路由反向级联（core `change/EVALAT` 规则 F / E 同形）。
//!
//! 窗口 S→T 里的变更元素若是 `EQUI` / `ELCONN`（规则 F：`GATREF(ELCXRF)`）或 `CWBRAN`（规则 E：
//! `GATREF(CLIST)`），到 T 端文件**自带的反向引用表**里点查「`STAREF` / `ENDREF` 指向它的元素」——
//! 规则 F 查变更元素自己，规则 E 查 `CWBRAN` 自己加它在 T 端索引里的整棵子树（路由节点 `HATTA` / `TATTA` /
//! `RNODE` / `RATTA` 散在一到两层下，路由段 `EXTGEO` 的 `STAREF` / `ENDREF` 指的就是它们）；引用者（`CABLE`
//! 自身或它的 `EXTGEO` 段）owner 链上落在 `roots_T` 里的生成根并进 `touched`，于是
//! `regen = touched ∩ roots_T` 会带上电缆的根。
//!
//! 与 core 同形（证据 `docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §8.6）：`ELCXRF`
//! 是 core.dll 伪属性 `DB_PseudoELCXRF_DESI` = `BACKRE{ENDREF, STAREF}` → `DB_PseudoBACKRE` 逐库
//! `DB_RefTableIterator`；`CLIST` 是 Core3D `CAB_DB_BranchCableList_pseudo` → 成员节点的 `LALCAB` →
//! `takeCables` 同一张表同两个属性。这里点查的正是 e3d-io 读出的那张表
//! （`ReadOnlyEngine::referrers_of`，`docs/evidence/2026-09-03-dabacon-named-tables-and-reference-tables.md`）。
//!
//! 边界与护栏（计划 `2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C8、共识 d-168）：
//! - **只查本库**：一张反向表只答它自己那个库的引用者（表存在引用方的库里）。语料里 `CABLE.STAREF` /
//!   `ENDREF` 的 196 条跨库边（电缆在电缆库、`ELCONN` 在电气库）这里查不到——那是跨库版的活，
//!   要 MDB 逐库并（core `DB_MDB::dbs` 也是逐库迭代）；
//! - 表在哪一端：只查 T。引用者的记录和表都在引用方那边，被引用的 `EQUI` 删了表项还在（电缆仍指着它），
//!   电缆自己删了则走删除路径，不归这里；
//! - 账本 noun 零 I/O 粗筛：没有 `EQUI` / `ELCONN` / `CWBRAN` 改动的窗口连引擎都不开；
//! - 引用者 → 根：沿 T 端 owner 索引向上（含自身），链上**每一个**落在 `roots_T` 里的根都算
//!   （与 C2 / 祖先闭包同口径，复用 [`roots_on_owner_chain`]）；
//! - 开关 `model_cable_endpoint_cascade`（`DbOption.toml` / `AIOS_MODEL_CABLE_ENDPOINT_CASCADE`），
//!   **默认关**；关着时不开引擎、不查表，`touched` 一字不改。
//!
//! 几何上今天不显：电缆几何没按路由落地（`CABLE` 现走 cata_model，e3d-model 的 `EXTGEO` 段未接），
//! 拉进来的电缆根重算出来的还是今天那份；先接线，电缆几何一落地这里就是正确性项（计划 §3 C8）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aios_core::RefnoEnum;
use e3d_attlib::db1_hash;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use e3d_model::increment::UpdatePlan;

use crate::data_interface::generation_root::{GenerationRoot, refno_from_e3d, refno_to_e3d};
use crate::data_interface::structural_connection::roots_on_owner_chain;

/// 要点查的两张反向表：core `ELCXRF` = `BACKRE{ENDREF, STAREF}`，`LALCAB::takeCables` 同两个。
pub const REFERENCE_ATTRIBUTES: [&str; 2] = ["STAREF", "ENDREF"];

/// 变更元素落在 core 的哪条规则里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// 规则 F：`EQUI` / `ELCONN` 变了 → `ELCXRF` = 引用它的电缆端点。
    Endpoint,
    /// 规则 E：`CWBRAN` 变了 → `CLIST` = 路过它的电缆（查它路由节点被谁引用）。
    Route,
}

/// noun → 规则。别的 noun 给 `None` = 不查。
pub fn rule_for(noun: &str) -> Option<Rule> {
    match noun.trim().to_ascii_uppercase().as_str() {
        "EQUI" | "ELCONN" => Some(Rule::Endpoint),
        "CWBRAN" => Some(Rule::Route),
        _ => None,
    }
}

/// 账本里落在规则表里的一个变更元素——零 I/O 粗筛的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeSource {
    pub refno: RefNo,
    pub noun: String,
    pub rule: Rule,
}

/// 从账本挑出要查的变更元素（按 refno 去重，不分变更类型）。
pub fn sources_from_plan(plan: &UpdatePlan) -> Vec<CascadeSource> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for entry in plan.ledger.entries() {
        let Some(rule) = rule_for(&entry.noun) else {
            continue;
        };
        if !seen.insert((entry.refno.word0, entry.refno.word1)) {
            continue;
        }
        out.push(CascadeSource {
            refno: entry.refno,
            noun: entry.noun.clone(),
            rule,
        });
    }
    out
}

/// 子树遍历的深度上限（`CWBRAN` 下最多两三层：`CTSTRA` → `RNODE` → `RATTA`；超过就是成环 / 坏数据）。
const SUBTREE_DEPTH_LIMIT: usize = 16;

/// 一个变更元素要在表里点查的键：规则 F 是它自己；规则 E 是 `CWBRAN` 自己加它**整棵子树**
/// （`HATTA` / `TATTA` 是直接成员，`RNODE` / `RATTA` 常挂在下一层——core `CAB_DB_BranchCableList_pseudo`
/// 收的是子树里这四种 noun，索引里没有 noun，就把子树全查；不是路由节点的键在表里本来就是空）。
pub fn query_points(source: &CascadeSource, children: impl Fn(RefNo) -> Vec<RefNo>) -> Vec<RefNo> {
    let mut out = vec![source.refno];
    if source.rule != Rule::Route {
        return out;
    }
    let mut seen: BTreeSet<(u32, u32)> = [(source.refno.word0, source.refno.word1)]
        .into_iter()
        .collect();
    let mut frontier = vec![source.refno];
    for _ in 0..SUBTREE_DEPTH_LIMIT {
        let mut next = Vec::new();
        for parent in &frontier {
            for child in children(*parent) {
                if seen.insert((child.word0, child.word1)) {
                    out.push(child);
                    next.push(child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// 一条反向边——表的产物；判定只看它的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferrerEdge {
    pub source: RefNo,
    pub rule: Rule,
    /// 表里被查的键（规则 F = `source`；规则 E = `source` 或它子树里的元素）。
    pub target: RefNo,
    pub attribute: &'static str,
    /// `referrer.attribute == target` 的那个元素（`CABLE` 或它的 `EXTGEO` 段）。
    pub referrer: RefNo,
}

/// 点查的产物与账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferrerEdges {
    pub edges: Vec<ReferrerEdge>,
    pub sources: usize,
    /// (键, 属性) 点查次数。
    pub queries: usize,
    /// 表读挂的点查次数（表在、读不动；表不在算空，不算挂）。
    pub query_failures: usize,
    /// T 端会话里 `STAREF` / `ENDREF` 两张表各在不在。
    pub tables_present: BTreeSet<&'static str>,
}

/// T 端索引的直接成员表（子树遍历用），只在窗口里真有 `CWBRAN` 改动时才建（一次 O(索引)）。
fn children_index(owners: &BTreeMap<(u32, u32), RefNo>) -> BTreeMap<(u32, u32), Vec<RefNo>> {
    let mut out: BTreeMap<(u32, u32), Vec<RefNo>> = BTreeMap::new();
    for (&(word0, word1), owner) in owners {
        let key = (owner.word0, owner.word1);
        if !owner.is_valid() || key == (word0, word1) {
            continue;
        }
        out.entry(key).or_default().push(RefNo::new(word0, word1));
    }
    out
}

/// I/O：对每个变更元素的每个键，点查 T 端文件的 `STAREF` / `ENDREF` 反向表。
pub fn read_referrer_edges(
    sources: &[CascadeSource],
    engine: &mut ReadOnlyEngine,
    owners: &BTreeMap<(u32, u32), RefNo>,
) -> ReferrerEdges {
    let mut out = ReferrerEdges {
        sources: sources.len(),
        ..ReferrerEdges::default()
    };
    if sources.is_empty() {
        return out;
    }
    let table_hashes: BTreeSet<u32> = engine
        .tables()
        .iter()
        .map(|table| table.name_hash)
        .collect();
    for attribute in REFERENCE_ATTRIBUTES {
        if table_hashes.contains(&db1_hash(attribute)) {
            out.tables_present.insert(attribute);
        }
    }
    let children = if sources.iter().any(|source| source.rule == Rule::Route) {
        children_index(owners)
    } else {
        BTreeMap::new()
    };
    for source in sources {
        let points = query_points(source, |parent| {
            children
                .get(&(parent.word0, parent.word1))
                .cloned()
                .unwrap_or_default()
        });
        for target in points {
            for attribute in REFERENCE_ATTRIBUTES {
                if !out.tables_present.contains(&attribute) {
                    continue;
                }
                out.queries += 1;
                match engine.referrers_of(db1_hash(attribute), target) {
                    Ok(referrers) => {
                        for referrer in referrers {
                            if !referrer.is_valid() {
                                continue;
                            }
                            out.edges.push(ReferrerEdge {
                                source: source.refno,
                                rule: source.rule,
                                target,
                                attribute,
                                referrer,
                            });
                        }
                    }
                    Err(_) => out.query_failures += 1,
                }
            }
        }
    }
    out
}

/// 一窗的电缆端点 / 路由级联：账 + 要并进 `touched` 的引用者根。挂在 `WindowRootSources` 上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CableEndpointCascade {
    /// 开关态。`false` = 没开引擎、没查表，[`Self::expand_touched`] 不动 `touched`。
    pub enabled: bool,
    /// T 端引擎开不了（同一个文件 `scan_index` 刚开过，几乎不会发生；发生了就记账、不级联）。
    pub engine_unavailable: bool,
    pub sources: usize,
    pub endpoint_sources: usize,
    pub route_sources: usize,
    pub queries: usize,
    pub query_failures: usize,
    /// T 端会话里 `STAREF` / `ENDREF` 两张表在几张（0–2）。
    pub tables_present: usize,
    pub edges: usize,
    /// 引用者在 T 端 owner 链上找不到任何生成根（悬空表项 / 无根族）的边数。
    pub referrer_without_root: usize,
    /// 要并进 `touched` 的引用者根（都在 `roots_T` 里）。
    pub referrer_roots: BTreeSet<RefnoEnum>,
}

impl CableEndpointCascade {
    /// 开关关：零 I/O、零改动。
    pub fn off() -> Self {
        Self::default()
    }

    /// 纯函数：从点查出的边折出引用者根集。`root_chain(引用者)` 给它 owner 链上的根
    /// （生产传 [`roots_on_owner_chain`]，测试可以手搓）。
    pub fn from_edges(
        sources: &[CascadeSource],
        edges: &ReferrerEdges,
        mut root_chain: impl FnMut(RefNo) -> Vec<RefNo>,
    ) -> Self {
        let mut out = Self {
            enabled: true,
            sources: edges.sources,
            endpoint_sources: sources
                .iter()
                .filter(|source| source.rule == Rule::Endpoint)
                .count(),
            route_sources: sources
                .iter()
                .filter(|source| source.rule == Rule::Route)
                .count(),
            queries: edges.queries,
            query_failures: edges.query_failures,
            tables_present: edges.tables_present.len(),
            edges: edges.edges.len(),
            ..Self::default()
        };
        for edge in &edges.edges {
            let roots = root_chain(edge.referrer);
            if roots.is_empty() {
                out.referrer_without_root += 1;
            }
            out.referrer_roots
                .extend(roots.into_iter().map(refno_from_e3d));
        }
        out
    }

    /// I/O 版（给定变更元素）：开 T 端引擎点查，再按 T 端 owner 索引与 `roots_T` 把引用者映射成根。
    pub fn from_sources(
        sources: &[CascadeSource],
        file: &Path,
        target_sesno: u32,
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
    ) -> Self {
        if sources.is_empty() {
            return Self {
                enabled: true,
                ..Self::default()
            };
        }
        let mut engine = match ReadOnlyEngine::open_at(file, target_sesno) {
            Ok(engine) => engine,
            Err(_) => {
                return Self {
                    enabled: true,
                    engine_unavailable: true,
                    sources: sources.len(),
                    ..Self::default()
                };
            }
        };
        let edges = read_referrer_edges(sources, &mut engine, owners);
        let root_keys: BTreeSet<(u32, u32)> = roots_t
            .iter()
            .map(|root| {
                let refno = refno_to_e3d(root.root);
                (refno.word0, refno.word1)
            })
            .collect();
        Self::from_edges(sources, &edges, |referrer| {
            roots_on_owner_chain(owners, &root_keys, referrer)
        })
    }

    /// I/O 版（从账本粗筛）。
    pub fn collect(
        plan: &UpdatePlan,
        file: &Path,
        target_sesno: u32,
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
    ) -> Self {
        let sources = sources_from_plan(plan);
        Self::from_sources(&sources, file, target_sesno, owners, roots_t)
    }

    /// 把引用者根并进 `touched`，回新增的个数。开关关 = 不动。
    pub fn expand_touched(&self, touched: &mut BTreeSet<RefnoEnum>) -> usize {
        if !self.enabled {
            return 0;
        }
        self.referrer_roots
            .iter()
            .filter(|root| touched.insert(**root))
            .count()
    }

    /// 回执里的一行观测。
    pub fn summary_line(&self, dbnum: u32) -> String {
        if !self.enabled {
            return format!(
                "dbnum={dbnum}: cable-endpoint cascade off — touched unchanged \
                 (model_cable_endpoint_cascade=false)"
            );
        }
        format!(
            "dbnum={dbnum}: cable-endpoint cascade on (same-db) — sources={} (endpoint={} route={}) \
             tables_present={} queries={} query_failures={} engine_unavailable={} edges={} \
             referrer_without_root={} referrer_roots={}",
            self.sources,
            self.endpoint_sources,
            self.route_sources,
            self.tables_present,
            self.queries,
            self.query_failures,
            self.engine_unavailable,
            self.edges,
            self.referrer_without_root,
            self.referrer_roots.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::GenerationRootKind;
    use crate::data_interface::window_root_plan::{WindowImpact, plan_window_roots};
    use aios_core::RefU64;

    const DB: u32 = 23584;

    fn r(id: u32) -> RefNo {
        RefNo::new(DB, id)
    }

    fn e(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(DB, id))
    }

    fn set(ids: &[u32]) -> BTreeSet<RefnoEnum> {
        ids.iter().map(|id| e(*id)).collect()
    }

    fn root(id: u32, noun: &str) -> GenerationRoot {
        GenerationRoot {
            root: e(id),
            noun: noun.into(),
            name: format!("/N{id}"),
            kind: GenerationRootKind::Normal,
        }
    }

    fn equi(id: u32) -> CascadeSource {
        CascadeSource {
            refno: r(id),
            noun: "EQUI".into(),
            rule: Rule::Endpoint,
        }
    }

    fn cwbran(id: u32) -> CascadeSource {
        CascadeSource {
            refno: r(id),
            noun: "CWBRAN".into(),
            rule: Rule::Route,
        }
    }

    /// EQUI Q(=1，自己是根) 被电缆 C(=2，自己是根) 的 `STAREF` 引用。
    fn endpoint_edges(referrers: &[u32]) -> ReferrerEdges {
        ReferrerEdges {
            edges: referrers
                .iter()
                .map(|id| ReferrerEdge {
                    source: r(1),
                    rule: Rule::Endpoint,
                    target: r(1),
                    attribute: "STAREF",
                    referrer: r(*id),
                })
                .collect(),
            sources: 1,
            queries: 2,
            query_failures: 0,
            tables_present: ["STAREF", "ENDREF"].into_iter().collect(),
        }
    }

    /// 元素 → 根链：1 是 EQUI 根自己；2 是 CABLE 根自己；21 是 CABLE 2 下的 EXTGEO；
    /// 31 在 EQUI 根 1 下（与变更元素同根）；41 无根。
    fn chain(referrer: RefNo) -> Vec<RefNo> {
        match referrer.word1 {
            1 | 31 => vec![r(1)],
            2 | 21 => vec![r(2)],
            _ => vec![],
        }
    }

    /// 改 Q：闭包只碰到 root(Q)；把级联并进 `touched` 后走生产同一段 `plan_window_roots`。
    fn regen_with(cascade: &CableEndpointCascade) -> Vec<RefnoEnum> {
        let mut touched = set(&[1]);
        cascade.expand_touched(&mut touched);
        let impact = WindowImpact {
            touched,
            closure_complete: true,
            cascade_degraded: None,
            unresolved: 0,
            candidates: 1,
            index_keys: 1000,
        };
        let roots_t = vec![root(1, "EQUI"), root(2, "CABLE")];
        plan_window_roots(&roots_t, &[], &impact)
            .regen
            .iter()
            .map(|root| root.root)
            .collect()
    }

    /// 契约第一态：表里有引用者 → `regen ⊇ {root(Q), root(C)}`。
    #[test]
    fn a_referrer_in_the_table_pulls_the_cable_root_into_regen() {
        let cascade = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[2]), chain);
        assert_eq!(cascade.edges, 1);
        assert_eq!(cascade.endpoint_sources, 1);
        assert_eq!(cascade.referrer_roots, set(&[2]));
        assert_eq!(regen_with(&cascade), vec![e(1), e(2)]);
    }

    /// 契约第二态：表里没有引用者（没人接在这台设备上）→ 只有 `root(Q)`。
    #[test]
    fn no_referrer_adds_nothing() {
        let cascade = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[]), chain);
        assert_eq!(cascade.queries, 2);
        assert!(cascade.referrer_roots.is_empty());
        assert_eq!(regen_with(&cascade), vec![e(1)]);
    }

    /// 契约第三态：开关关 → 只有 `root(Q)`，而且 `touched` 一字不改。
    #[test]
    fn the_switch_off_leaves_touched_untouched() {
        let cascade = CableEndpointCascade::off();
        assert!(!cascade.enabled);
        let mut touched = set(&[1]);
        assert_eq!(cascade.expand_touched(&mut touched), 0);
        assert_eq!(touched, set(&[1]));
        assert_eq!(regen_with(&cascade), vec![e(1)]);
    }

    /// 引用者是 `EXTGEO` 段 → 沿 owner 链归到它的 `CABLE` 根；引用者与变更元素同根 → 还是那一个根；
    /// 引用者无根（悬空表项）→ 记账、不加根。
    #[test]
    fn a_referrer_is_mapped_through_its_owner_chain() {
        let via_segment =
            CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[21]), chain);
        assert_eq!(via_segment.referrer_roots, set(&[2]));

        let same_root = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[31]), chain);
        assert_eq!(same_root.referrer_roots, set(&[1]));
        assert_eq!(regen_with(&same_root), vec![e(1)]);

        let dangling = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[41]), chain);
        assert_eq!(dangling.referrer_without_root, 1);
        assert!(dangling.referrer_roots.is_empty());
    }

    /// 规则 E 的键 = `CWBRAN` 自己加整棵子树（`RATTA` 挂在 `RNODE` 下那一层也要到）；规则 F 只有自己；
    /// 成环 / 重复不转圈。
    #[test]
    fn route_sources_query_their_whole_subtree() {
        let members = |parent: RefNo| -> Vec<RefNo> {
            match parent.word1 {
                100 => vec![r(101), r(102)],
                101 => vec![r(103)],
                103 => vec![r(100)], // 坏数据：指回顶
                _ => vec![],
            }
        };
        assert_eq!(
            query_points(&cwbran(100), members),
            vec![r(100), r(101), r(102), r(103)]
        );
        assert_eq!(query_points(&equi(100), members), vec![r(100)]);

        // 成员表：只收直接成员，顶（owner 是自己 / 无效）不进任何人的名下。
        let owners: BTreeMap<(u32, u32), RefNo> = [
            ((DB, 100), r(100)),
            ((DB, 101), r(100)),
            ((DB, 102), r(100)),
            ((DB, 103), r(101)),
        ]
        .into_iter()
        .collect();
        let children = children_index(&owners);
        assert_eq!(children.get(&(DB, 100)), Some(&vec![r(101), r(102)]));
        assert_eq!(children.get(&(DB, 101)), Some(&vec![r(103)]));
        assert!(!children.contains_key(&(DB, 103)));
    }

    /// 规则表：三个 noun，一个不多；大小写与空白不敏感。
    #[test]
    fn the_noun_table_is_exactly_the_two_core_rules() {
        assert_eq!(rule_for("EQUI"), Some(Rule::Endpoint));
        assert_eq!(rule_for("ELCONN"), Some(Rule::Endpoint));
        assert_eq!(rule_for(" cwbran "), Some(Rule::Route));
        for noun in [
            "CABLE", "EXTGEO", "HATTA", "SUBE", "CWAY", "SCTN", "BRAN", "",
        ] {
            assert_eq!(rule_for(noun), None, "{noun}");
        }
        assert_eq!(REFERENCE_ATTRIBUTES, ["STAREF", "ENDREF"]);
    }

    #[test]
    fn the_summary_line_tells_the_switch_state_apart() {
        assert!(
            CableEndpointCascade::off()
                .summary_line(7200)
                .contains("cascade off")
        );
        let on = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[2]), chain);
        let line = on.summary_line(7200);
        assert!(
            line.contains("cascade on") && line.contains("referrer_roots=1"),
            "{line}"
        );
    }

    // ---- 真库（手动）：zdj7200 的六根电缆 + 现有窗口两态逐字相同 ----

    #[cfg(test)]
    mod live {
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
        use e3d_io::session::SessionSelector;

        use super::*;
        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::{
            enumerate_generation_roots, resolve_delivery_unit_types,
        };
        use crate::data_interface::window_root_plan::{PersistedRoot, WindowRootSources};
        use crate::fast_model::e3d_model_service::scan_index;

        fn zdj7200() -> PathBuf {
            PathBuf::from(
                std::env::var("AIOS_ZDJ7200_FILE")
                    .unwrap_or_else(|_| r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000\zdj7200_0001".into()),
            )
        }

        fn open(schema: &DirectSchema, file: &Path, sesno: u32) -> Arc<DbSet> {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: file.to_path_buf(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin DESI");
            set
        }

        /// zdj7200_0001 最新会话：六根 `CABLE` 的 `STAREF` / `ENDREF` 指着同库的 `EQUI`（普查
        /// `CABLE.STAREF/ENDREF -> EQUI` 36 条 = 这 6 根 × 3 份拷贝），语料里没有「只改那台 EQUI」
        /// 的自然窗口（2→3 时电缆还没建，4→5 两端一起建），所以这里**假定** `EQUI` 变了：
        /// 规则 F 查表必须把接在它上面的电缆根全部拉进来；规则 E 假定一条 `CWBRAN` 变了，路过它
        /// 的电缆根必须进来；两条都只能拉进 `CABLE` 根。只读文件：`AIOS_ZDJ7200_FILE`
        /// （默认 zdj7200_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
        #[test]
        #[ignore = "manual live: needs the ZDJ zdj7200_0001 DESI file and the E3D template directory"]
        fn live_zdj7200_an_equi_or_cwbran_source_pulls_its_cable_roots_in() {
            let file = zdj7200();
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let sesno = ReadOnlyEngine::open(&file)
                .expect("open zdj7200")
                .session()
                .sesno;
            let index = scan_index(&file, Some(sesno)).expect("scan index");
            let set = open(&schema, &file, sesno);
            let roots_t = enumerate_generation_roots(&set, &index.roots, &unit_types)
                .expect("enumerate roots");
            let root_noun: BTreeMap<RefnoEnum, String> = roots_t
                .iter()
                .map(|root| (root.root, root.noun.clone()))
                .collect();

            // 规则 F：找一台被电缆 STAREF 引用的 EQUI（不写死 refno，从表里反推）。
            let cables: Vec<RefNo> = roots_t
                .iter()
                .filter(|root| root.noun.eq_ignore_ascii_case("CABLE"))
                .map(|root| refno_to_e3d(root.root))
                .collect();
            assert_eq!(cables.len(), 6, "zdj7200 该有六根电缆：{cables:?}");
            let mut equi_targets: BTreeMap<(u32, u32), BTreeSet<(u32, u32)>> = BTreeMap::new();
            for cable in &cables {
                let element = set.element(*cable);
                for attribute in REFERENCE_ATTRIBUTES {
                    if let Ok(Some(target)) = element.get_ref(attribute) {
                        equi_targets
                            .entry((target.word0, target.word1))
                            .or_default()
                            .insert((cable.word0, cable.word1));
                    }
                }
            }
            let (equi, expected_cables) = equi_targets
                .iter()
                .max_by_key(|(_, cables)| cables.len())
                .map(|(equi, cables)| (RefNo::new(equi.0, equi.1), cables.clone()))
                .expect("电缆总得接在什么上");
            assert_eq!(
                set.element(equi).element_type().expect("noun"),
                "EQUI",
                "六根电缆的端点在这套语料里是 EQUI"
            );
            let source = CascadeSource {
                refno: equi,
                noun: "EQUI".into(),
                rule: Rule::Endpoint,
            };
            let cascade = CableEndpointCascade::from_sources(
                &[source],
                &file,
                sesno,
                &index.owners,
                &roots_t,
            );
            println!(
                "EQUI {}/{}: {}",
                equi.word0,
                equi.word1,
                cascade.summary_line(7200)
            );
            assert_eq!(
                cascade.tables_present, 2,
                "zdj7200 有 STAREF 与 ENDREF 两张表"
            );
            assert_eq!(cascade.query_failures, 0);
            assert!(!cascade.engine_unavailable);
            let expected: BTreeSet<RefnoEnum> = expected_cables
                .iter()
                .map(|cable| refno_from_e3d(RefNo::new(cable.0, cable.1)))
                .collect();
            assert_eq!(
                cascade.referrer_roots, expected,
                "接在 EQUI 上的电缆根必须一根不多一根不少"
            );
            assert!(expected.len() >= 5, "普查里那台 EQUI 接着 5 根电缆");

            // 规则 E：CWBRAN 变了 → 路过它的电缆根（EXTGEO.STAREF/ENDREF 指着它的路由节点）。
            let branches: Vec<CascadeSource> = roots_t
                .iter()
                .filter(|root| root.noun.eq_ignore_ascii_case("CWBRAN"))
                .map(|root| CascadeSource {
                    refno: refno_to_e3d(root.root),
                    noun: "CWBRAN".into(),
                    rule: Rule::Route,
                })
                .collect();
            assert!(!branches.is_empty(), "zdj7200 有 CWBRAN");
            let route = CableEndpointCascade::from_sources(
                &branches,
                &file,
                sesno,
                &index.owners,
                &roots_t,
            );
            println!("CWBRAN×{}: {}", branches.len(), route.summary_line(7200));
            assert!(
                route.edges > 0,
                "EXTGEO.STAREF/ENDREF 指着这些分支的路由节点"
            );
            assert!(!route.referrer_roots.is_empty());
            for root in &route.referrer_roots {
                assert_eq!(
                    root_noun.get(root).map(String::as_str),
                    Some("CABLE"),
                    "规则 E 只该拉进电缆根：{}",
                    root.to_pdms_str()
                );
            }
            let cable_set: BTreeSet<RefnoEnum> =
                cables.iter().map(|c| refno_from_e3d(*c)).collect();
            assert!(route.referrer_roots.is_subset(&cable_set));
        }

        /// zdj7200_0001 会话链上每一步，以及 C2 的验收窗口 `aps7200_0001` 3→4：开关两态 `regen`
        /// 逐字相同——语料里没有「只改 EQUI / CWBRAN 一端」的自然窗口（2→3 电缆还没建；4→5
        /// 电缆与端点一起建，电缆根本来就在 `touched` 里）。只读文件：`AIOS_ZDJ7200_FILE`、
        /// `AIOS_APS_PROJECT_DIR` + `AIOS_E3D_TEMPLATE_DIR`。
        #[test]
        #[ignore = "manual live: needs the ZDJ / AvevaPlantSample DESI files and the E3D template directory"]
        fn live_existing_windows_are_identical_with_the_cascade_on_and_off() {
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let aps =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            let mut windows: Vec<(PathBuf, u32, u32)> = vec![(aps.join("aps7200_0001"), 3, 4)];
            let zdj = zdj7200();
            let mut engine = ReadOnlyEngine::open(&zdj).expect("open zdj7200");
            let mut sesnos: Vec<u32> = engine
                .sessions()
                .expect("sessions")
                .iter()
                .map(|session| session.session_id)
                .collect();
            sesnos.sort_unstable();
            sesnos.dedup();
            let steps = engine
                .diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)
                .expect("diff steps");
            for step in &steps {
                windows.push((zdj.clone(), step.from.sesno, step.to.sesno));
            }
            drop(engine);
            for (file, base_sesno, target_sesno) in windows {
                let (base, target) = (
                    open(&schema, &file, base_sesno),
                    open(&schema, &file, target_sesno),
                );
                let base_index = scan_index(&file, Some(base_sesno)).expect("scan base index");
                let roots_s: Vec<PersistedRoot> =
                    enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                        .expect("enumerate base roots")
                        .into_iter()
                        .map(|root| PersistedRoot {
                            refno: root.root,
                            noun: root.noun,
                        })
                        .collect();
                let run = |on: bool| {
                    let _switch = crate::options::CableEndpointCascadeOverride::set(on);
                    let sources = WindowRootSources::collect(
                        &file,
                        base_sesno,
                        target_sesno,
                        &base,
                        &target,
                        &unit_types,
                        &[],
                    )
                    .expect("collect");
                    let impact = sources.impact(&roots_s);
                    let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                    let regen: Vec<RefnoEnum> = plan.regen.iter().map(|root| root.root).collect();
                    (sources.cable_endpoint, regen)
                };
                let ((on, regen_on), (off, regen_off)) = (run(true), run(false));
                println!(
                    "{} {base_sesno}→{target_sesno}: {} | regen_on={} regen_off={}",
                    file.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    on.summary_line(7200),
                    regen_on.len(),
                    regen_off.len()
                );
                assert!(on.enabled && !off.enabled);
                assert_eq!(
                    regen_on,
                    regen_off,
                    "{} {base_sesno}→{target_sesno}: 两态 regen 必须逐字相同",
                    file.display()
                );
            }
        }
    }
}
