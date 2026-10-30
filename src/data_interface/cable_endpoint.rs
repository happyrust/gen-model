//! C8 —— direct 窗口规划器的电缆端点 / 路由反向级联（core `change/EVALAT` 规则 F / E 同形）。同库版 2026-09-07，
//! 跨库版 2026-10-01（GitHub #35，见下面「跨库版」一节）。
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
//! - **本库的表只答本库的引用者**（表存在引用方的库里）。语料里 `CABLE.STAREF` / `ENDREF` 的 196 条跨库边
//!   （电缆在电缆库、`ELCONN` 在电气库）在本库表里查不到——跨库版（下一节）逐个开 MDB 其余设计成员查它们自己的表；
//! - 表在哪一端：只查 T。引用者的记录和表都在引用方那边，被引用的 `EQUI` 删了表项还在（电缆仍指着它），
//!   电缆自己删了则走删除路径，不归这里；
//! - 账本 noun 零 I/O 粗筛：没有 `EQUI` / `ELCONN` / `CWBRAN` 改动的窗口连引擎都不开；
//! - 引用者 → 根：沿 T 端 owner 索引向上（含自身），链上**每一个**落在 `roots_T` 里的根都算
//!   （与 C2 / 祖先闭包同口径，复用 [`roots_on_owner_chain`]）；
//! - 开关 `model_cable_endpoint_cascade`（`DbOption.toml` / `AIOS_MODEL_CABLE_ENDPOINT_CASCADE`），
//!   **默认开**（2026-10-17 起，GitHub #35 Phase C，见 `docs/plans/2026-10-17-cable-endpoint-cascade-default-on-checklist.md`）；
//!   关着时不开引擎、不查表，`touched` 一字不改。
//!
//! 2026-10-17（GitHub #35 Phase B + C）：`vendor/e3d-model` 的 `EXTGEO` 电缆路由与跨桥架圆角实体构建已对拍落地，
//! 且规则 E 补齐了桥架附着点（`HATTA` / `TATTA` / `RATTA` / `RNODE`）直接触发与桥架子路径输入（`RPATH` / `POINTR` /
//! 桥架件）沿 T 端 `owners` 上卷所属 `CWBRAN` 根、跳过 `Disposition::NoModel` 元数据门放行条目。
//!
//! **跨库版（GitHub #35，2026-10-01）**：同一批键到 MDB **其余**设计成员各自的 `STAREF` / `ENDREF` 反向表点查
//! （[`read_foreign_referrers`]）——core 的 `DB_PseudoBACKRE` 也是 `DB_MDB::dbs` 逐库迭代，与 P4 逐库并集同一个形状，
//! 成员名单也是同一份（`E3dModelService::design_sources()`）。命中的引用者（`CABLE` / 它的 `EXTGEO` 段）在那个库里，
//! 本窗的根集装不下：记进 [`CableEndpointCascade::foreign`]，计划带着交给跨库交接
//! （[`crate::data_interface::cross_db_handoff`]），落地之后在那个库的文件最新会话上折根、排一条只带名单的 `ModelOnly`。
//! 打不开的成员库点名、**不判退化**：电缆几何落地前几何上不显，为它整窗重算全部根不值；几何落地翻默认时再收紧。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aios_core::RefnoEnum;
use e3d_attlib::db1_hash;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use e3d_model::increment::UpdatePlan;

use crate::data_interface::generation_root::{GenerationRoot, refno_from_e3d, refno_to_e3d};
use crate::data_interface::reverse_reference::SourceDb;
use crate::data_interface::structural_connection::roots_on_owner_chain;

/// 回执里逐库点名的跨库引用者上限，多出来的只报个数。
const FOREIGN_NAMED_LIMIT: usize = 12;

/// 要点查的两张反向表：core `ELCXRF` = `BACKRE{ENDREF, STAREF}`，`LALCAB::takeCables` 同两个。
pub const REFERENCE_ATTRIBUTES: [&str; 2] = ["STAREF", "ENDREF"];

/// 变更元素落在 core 的哪条规则里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// 规则 F：`EQUI` / `ELCONN` 变了 → `ELCXRF` = 引用它的电缆端点。
    Endpoint,
    /// 规则 E：`CWBRAN`、桥架附着点（`HATTA` / `TATTA` / `RATTA` / `RNODE`）或所属 `CWBRAN`
    /// 子路径（`RPATH` / `POINTR` / 桥架件）变了 → `CLIST` = 路过它的电缆（查路由节点被哪段 `EXTGEO` 引用）。
    Route,
}

/// noun → 规则。别的 noun 给 `None` = 不直接查（桥架子路径输入走 [`is_branch_route_input`] 上卷到所属 `CWBRAN`）。
pub fn rule_for(noun: &str) -> Option<Rule> {
    match noun.trim().to_ascii_uppercase().as_str() {
        "EQUI" | "ELCONN" => Some(Rule::Endpoint),
        "CWBRAN" | "HATTA" | "TATTA" | "RATTA" | "RNODE" => Some(Rule::Route),
        _ => None,
    }
}

/// 桥架分支路径输入 noun：自身不直接被 `EXTGEO.STAREF` / `ENDREF` 指向，但改动会改写所属
/// `CWBRAN` 子路径与附着点世界坐标，需沿 T 端 owner 链上卷到所属 `CWBRAN` 按规则 E 点查整棵分支子树。
pub fn is_branch_route_input(noun: &str) -> bool {
    let upper = noun.trim().to_ascii_uppercase();
    matches!(upper.as_str(), "RPATH" | "POINTR") || e3d_model::is_cable_tray_element(&upper)
}

/// 账本里落在规则表里的一个变更元素——零 I/O 粗筛的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeSource {
    pub refno: RefNo,
    pub noun: String,
    pub rule: Rule,
}

fn no_model_keys(plan: &UpdatePlan) -> BTreeSet<(u32, u32)> {
    plan.ledger
        .dispositions()
        .iter()
        .filter(|(_, d)| *d == e3d_model::ledger::Disposition::NoModel)
        .map(|(k, _)| *k)
        .collect()
}

/// 从账本挑出要查的变更元素（按 refno 去重，跳过 `NoModel`）。
pub fn sources_from_plan(plan: &UpdatePlan) -> Vec<CascadeSource> {
    let no_model = no_model_keys(plan);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for entry in plan.ledger.entries() {
        // `NoModel` 是元数据门放行的候选（只动了 DCHC=0 且生成器不读的字段，像 aps250514 3→4
        // 两只 CWBRAN 只改 HCREF / TCREF），源自己都不重算，更不该沿引用边把别的根拉进 regen。
        let key = (entry.refno.word0, entry.refno.word1);
        if no_model.contains(&key) {
            continue;
        }
        let Some(rule) = rule_for(&entry.noun) else {
            continue;
        };
        if !seen.insert(key) {
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

/// 从账本挑出要查的变更元素，并将落在 `CWBRAN` 子树内的路径输入（`RPATH` / `POINTR` / 桥架件）
/// 沿 T 端 `owners` 上卷到所属 `CWBRAN` 根（按规则 E 展开整棵分支子树查 `STAREF` / `ENDREF`）。
pub fn sources_from_plan_with_owners(
    plan: &UpdatePlan,
    owners: &BTreeMap<(u32, u32), RefNo>,
    roots_t: &[GenerationRoot],
) -> Vec<CascadeSource> {
    let no_model = no_model_keys(plan);
    let cwbran_roots: BTreeSet<(u32, u32)> = roots_t
        .iter()
        .filter(|root| root.noun.trim().eq_ignore_ascii_case("CWBRAN"))
        .map(|root| {
            let refno = refno_to_e3d(root.root);
            (refno.word0, refno.word1)
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for entry in plan.ledger.entries() {
        let key = (entry.refno.word0, entry.refno.word1);
        if no_model.contains(&key) {
            continue;
        }
        if let Some(rule) = rule_for(&entry.noun) {
            if seen.insert(key) {
                out.push(CascadeSource {
                    refno: entry.refno,
                    noun: entry.noun.clone(),
                    rule,
                });
            }
            continue;
        }
        if !cwbran_roots.is_empty() && is_branch_route_input(&entry.noun) {
            for branch in roots_on_owner_chain(owners, &cwbran_roots, entry.refno) {
                if seen.insert((branch.word0, branch.word1)) {
                    out.push(CascadeSource {
                        refno: branch,
                        noun: "CWBRAN".to_string(),
                        rule: Rule::Route,
                    });
                }
            }
        }
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

/// 跨库版点查的账（[`CableEndpointCascade::foreign`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignReferrers {
    /// 真点查过的成员库数（开得了、两张表至少有一张）。
    pub members_queried: usize,
    /// (键, 属性) 点查次数，各成员相加。
    pub queries: usize,
    /// 表在、读挂了的点查次数。
    pub query_failures: usize,
    /// 打不开的成员库 → 原因。**不是**「这个库没有引用者」，是未知；回执点名。
    pub unreadable_members: BTreeMap<u32, String>,
    /// 引用者所在库 → 引用者（`CABLE` 或它的 `EXTGEO` 段），去重。
    pub referrers: BTreeMap<u32, BTreeSet<(u32, u32)>>,
}

impl ForeignReferrers {
    /// 跨库引用者总数。
    pub fn referrer_count(&self) -> usize {
        self.referrers.values().map(BTreeSet::len).sum()
    }
}

/// I/O（跨库版，GitHub #35）：同一批变更元素的同一批键（规则 E 的子树取自**本库** T 端 owner 索引——路由节点在本库），
/// 到 MDB 其余设计成员各自的 `STAREF` / `ENDREF` 反向表点查。一个成员打不开只记账不中断；本库与非设计库跳过。
pub fn read_foreign_referrers(
    sources: &[CascadeSource],
    owners: &BTreeMap<(u32, u32), RefNo>,
    own_dbnum: u32,
    members: &[SourceDb],
) -> ForeignReferrers {
    let mut out = ForeignReferrers::default();
    if sources.is_empty() {
        return out;
    }
    for member in members
        .iter()
        .filter(|member| member.db_type.eq_ignore_ascii_case("DESI") && member.dbnum != own_dbnum)
    {
        let opened = match member.sesno {
            Some(sesno) => ReadOnlyEngine::open_at(&member.file, sesno),
            None => ReadOnlyEngine::open(&member.file),
        };
        let mut engine = match opened {
            Ok(engine) => engine,
            Err(error) => {
                out.unreadable_members
                    .insert(member.dbnum, format!("{error:#}"));
                continue;
            }
        };
        let edges = read_referrer_edges(sources, &mut engine, owners);
        if edges.tables_present.is_empty() {
            continue;
        }
        out.members_queried += 1;
        out.queries += edges.queries;
        out.query_failures += edges.query_failures;
        for edge in edges.edges {
            out.referrers
                .entry(edge.referrer.dbno())
                .or_default()
                .insert((edge.referrer.word0, edge.referrer.word1));
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
    /// 跨库版（GitHub #35）：MDB 其余设计成员表里的引用者。本窗不上卷，随计划交给跨库交接。
    pub foreign: ForeignReferrers,
}

impl CableEndpointCascade {
    /// 跨库版（GitHub #35）：在同库版的结果上补 MDB 其余设计成员的点查（[`read_foreign_referrers`]）。
    /// 开关关（`off()`）或成员名单空 = 原样返回，一个成员库都不开。
    pub fn with_foreign_members(
        mut self,
        plan: &UpdatePlan,
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
        own_dbnum: u32,
        members: &[SourceDb],
    ) -> Self {
        if !self.enabled || members.is_empty() {
            return self;
        }
        let sources = sources_from_plan_with_owners(plan, owners, roots_t);
        self.foreign = read_foreign_referrers(&sources, owners, own_dbnum, members);
        self
    }

    /// 跨库引用者（跨库交接的输入，[`crate::data_interface::cross_db_handoff::elements_by_dbnum`]）。
    pub fn foreign_referrer_elements(&self) -> impl Iterator<Item = RefNo> + '_ {
        self.foreign
            .referrers
            .values()
            .flatten()
            .map(|&(word0, word1)| RefNo::new(word0, word1))
    }
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

    /// I/O 版（从账本粗筛 + `CWBRAN` 子路径输入上卷）。
    pub fn collect(
        plan: &UpdatePlan,
        file: &Path,
        target_sesno: u32,
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
    ) -> Self {
        let sources = sources_from_plan_with_owners(plan, owners, roots_t);
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
        let mut line = format!(
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
        );
        let foreign = &self.foreign;
        if foreign.members_queried > 0
            || !foreign.unreadable_members.is_empty()
            || !foreign.referrers.is_empty()
        {
            let named: Vec<String> = foreign
                .referrers
                .iter()
                .flat_map(|(db, keys)| {
                    keys.iter().map(move |&(word0, word1)| {
                        format!("{} (dbnum {db})", RefNo::new(word0, word1))
                    })
                })
                .take(FOREIGN_NAMED_LIMIT)
                .collect();
            let more = foreign.referrer_count().saturating_sub(FOREIGN_NAMED_LIMIT);
            line.push_str(&format!(
                "; cross-db (GitHub #35) — members_queried={} queries={} query_failures={} \
                 unreadable_members={:?} referrers={} handed off to their own dbnum's task: [{}]{}",
                foreign.members_queried,
                foreign.queries,
                foreign.query_failures,
                foreign.unreadable_members.keys().collect::<Vec<_>>(),
                foreign.referrer_count(),
                named.join(", "),
                if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                }
            ));
        }
        line
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
            cache_stamp: 0,
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

    /// 规则表：规则 F（EQUI / ELCONN）与规则 E（CWBRAN + 4 种附着点 HATTA / TATTA / RATTA / RNODE）；
    /// 桥架子路径输入（RPATH / POINTR / 桥架件）由 `is_branch_route_input` 识别并沿 owner 链上卷到所属 CWBRAN。
    #[test]
    fn the_noun_table_is_exactly_the_two_core_rules() {
        assert_eq!(rule_for("EQUI"), Some(Rule::Endpoint));
        assert_eq!(rule_for("ELCONN"), Some(Rule::Endpoint));
        for route_noun in [" cwbran ", "HATTA", "TATTA", "RATTA", "RNODE"] {
            assert_eq!(rule_for(route_noun), Some(Rule::Route), "{route_noun}");
        }
        for noun in ["CABLE", "EXTGEO", "SUBE", "CWAY", "SCTN", "BRAN", ""] {
            assert_eq!(rule_for(noun), None, "{noun}");
        }
        assert!(is_branch_route_input("RPATH"));
        assert!(is_branch_route_input(" pointr "));
        assert!(is_branch_route_input("CTSTRA"));
        assert!(!is_branch_route_input("EXTGEO"));
        assert!(!is_branch_route_input("CABLE"));
        assert_eq!(REFERENCE_ATTRIBUTES, ["STAREF", "ENDREF"]);
    }

    /// 桥架子路径输入（`RPATH` / `POINTR`）落在 `CWBRAN` 子树内时，上卷到所属 `CWBRAN` 作为 `Rule::Route` 源；
    /// 落在 `CABLE` / `HANDRA` 下的 `POINTR` 或被元数据门判为 `NoModel` 的条目不触发路由级联。
    #[test]
    fn rpath_and_pointr_under_cwbran_fold_to_owning_cwbran_route_source() {
        use e3d_model::element_diff::ElementDiff;
        use e3d_model::increment::{IncrementReport, UpdatePlan};
        use e3d_model::ledger::{ChangeLedger, Disposition};

        let mod_diff = |refno: RefNo, noun: &str| ElementDiff {
            refno: refno.to_string(),
            noun: noun.into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: vec!["POS".into()],
            opaque: false,
        };

        // 100 = CWBRAN 根 -> 101 = RPATH -> 102 = POINTR；103 = HATTA（同在 CWBRAN 下）
        // 200 = CABLE 根 -> 201 = RPATH -> 202 = POINTR
        // 300 = CWBRAN 根（只动了 NoModel 字段）
        let owners: BTreeMap<(u32, u32), RefNo> = [
            ((DB, 100), r(100)),
            ((DB, 101), r(100)),
            ((DB, 102), r(101)),
            ((DB, 103), r(100)),
            ((DB, 200), r(200)),
            ((DB, 201), r(200)),
            ((DB, 202), r(201)),
            ((DB, 300), r(300)),
            ((DB, 301), r(300)),
        ]
        .into_iter()
        .collect();
        let roots_t = vec![root(100, "CWBRAN"), root(200, "CABLE"), root(300, "CWBRAN")];

        let mut ledger = ChangeLedger::default();
        // CWBRAN 100 下的 POINTR 102 改了位置（RolledUp）
        ledger.record(r(102), &mod_diff(r(102), "POINTR"));
        ledger.dispose(r(102), Disposition::RolledUp);
        // CABLE 200 下的 POINTR 202 改了位置（RolledUp）：不属于 CWBRAN，不应触发桥架级联
        ledger.record(r(202), &mod_diff(r(202), "POINTR"));
        ledger.dispose(r(202), Disposition::RolledUp);
        // CWBRAN 300 及其 RPATH 301 只改了 NoModel 元数据：不应触发级联
        ledger.record(r(300), &mod_diff(r(300), "CWBRAN"));
        ledger.dispose(r(300), Disposition::NoModel);
        ledger.record(r(301), &mod_diff(r(301), "RPATH"));
        ledger.dispose(r(301), Disposition::NoModel);

        let plan = UpdatePlan {
            regenerate: BTreeSet::new(),
            remove: BTreeSet::new(),
            regenerate_derived: BTreeSet::new(),
            derived_stale: BTreeSet::new(),
            ledger,
            full_rebuild: None,
            report: IncrementReport::default(),
        };

        let sources = sources_from_plan_with_owners(&plan, &owners, &roots_t);
        assert_eq!(sources, vec![cwbran(100)]);

        let children = children_index(&owners);
        let points = query_points(&sources[0], |p| {
            children
                .get(&(p.word0, p.word1))
                .cloned()
                .unwrap_or_default()
        });
        assert!(
            points.contains(&r(103)),
            "上卷到 CWBRAN 100 后，query_points 必须覆盖同分支下的附着点 HATTA 103: {points:?}"
        );
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
        assert!(
            !line.contains("cross-db"),
            "没查过别的库就不出跨库那一段: {line}"
        );
    }

    /// 低 13 位放不下的库号（APS 的 2501xx）在第一个 extent 上的 `word0`：高半截挪到 26..30 位。
    fn word0_of(dbno: u32) -> u32 {
        (dbno & 0x1FFF) | ((dbno & 0x3E000) << 13) | (1 << 13)
    }

    /// 跨库版（GitHub #35）能纯内存钉的两态：没有要查的键 → 一个成员库都不开；成员库打不开 → 点名
    /// （是「未知」，不是「没有引用者」）；本库与非设计库不开。第一态（别的库表里有引用者 → 交给那个库）在
    /// `tests::live::live_aps_a_cross_db_endpoint_finds_its_cable_in_the_other_library` 真库上。
    #[test]
    fn the_cross_db_lookup_opens_only_the_other_design_members_and_names_the_unreadable_ones() {
        assert_eq!(r(1).dbno(), 7200, "测试库号");
        let member = |dbnum: u32, db_type: &str| SourceDb {
            dbnum,
            db_type: db_type.into(),
            file: std::path::PathBuf::from(format!(r"Z:\gm35\{dbnum}_0001")),
            sesno: None,
        };
        let members = [
            member(7200, "DESI"),
            member(250152, "DESI"),
            member(7351, "CATA"),
        ];
        assert_eq!(
            read_foreign_referrers(&[], &BTreeMap::new(), 7200, &members),
            ForeignReferrers::default(),
            "没有要查的键，一个成员库都不开"
        );
        let foreign = read_foreign_referrers(&[equi(1)], &BTreeMap::new(), 7200, &members);
        assert_eq!(
            foreign
                .unreadable_members
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![250152],
            "只开别的设计成员：本库与 CATA 不开"
        );
        assert_eq!(foreign.members_queried, 0);
        assert!(foreign.referrers.is_empty());
        assert_eq!(foreign.referrer_count(), 0);
    }

    /// 跨库引用者只记账、不并进本窗：`touched` 照旧，回执点名（含打不开的成员），跨库交接拿到按库分好的名单。
    #[test]
    fn cross_db_referrers_are_named_and_handed_off_instead_of_rolled_up() {
        let cable = RefNo::new(word0_of(250176), 55);
        let segment = RefNo::new(word0_of(250176), 61);
        assert_eq!(cable.dbno(), 250176);
        let mut cascade = CableEndpointCascade::from_edges(&[equi(1)], &endpoint_edges(&[]), chain);
        cascade.foreign = ForeignReferrers {
            members_queried: 2,
            queries: 4,
            query_failures: 0,
            unreadable_members: BTreeMap::from([(250164, "locked".to_string())]),
            referrers: BTreeMap::from([(
                250176,
                BTreeSet::from([(cable.word0, cable.word1), (segment.word0, segment.word1)]),
            )]),
        };
        let mut touched = set(&[1]);
        assert_eq!(
            cascade.expand_touched(&mut touched),
            0,
            "跨库引用者不并进本窗"
        );
        assert_eq!(regen_with(&cascade), vec![e(1)]);
        let line = cascade.summary_line(7200);
        assert!(
            line.contains("cross-db (GitHub #35)")
                && line.contains("referrers=2")
                && line.contains("(dbnum 250176)")
                && line.contains("250164"),
            "{line}"
        );
        assert_eq!(
            crate::data_interface::cross_db_handoff::elements_by_dbnum(
                cascade.foreign_referrer_elements()
            ),
            BTreeMap::from([(250176, vec![cable.to_string(), segment.to_string()])])
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

        /// 语料缺失就跳过，存在就必须真跑。
        ///
        /// 这些用例读的是本机安装的 E3D 样例库（DESI），CI 上没有。以前靠 `#[ignore]`
        /// 挡住，结果是**任何机器上都永远不跑**——包括装了语料的开发机。改成
        /// 「文件不在 → 打一行 skip 后 return；文件在 → 继续，失败就是真失败」，
        /// 于是开发机上它们进了默认回归，CI 上也不会红。
        ///
        /// 返回 `false` 表示应当跳过（调用方直接 `return`）。
        fn corpus_ready(what: &str, paths: &[&Path]) -> bool {
            for path in paths {
                if !path.exists() {
                    eprintln!(
                        "skip {what}: 语料不在本机（{}）——装了 E3D 样例库的机器上这条会真跑",
                        path.display()
                    );
                    return false;
                }
            }
            true
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
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_zdj7200_an_equi_or_cwbran_source_pulls_its_cable_roots_in() {
            let file = zdj7200();
            if !corpus_ready("zdj7200 的电缆根", &[file.as_path()]) {
                return;
            }
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
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_existing_windows_are_identical_with_the_cascade_on_and_off() {
            let aps =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            let zdj = zdj7200();
            if !corpus_ready(
                "zdj7200 / aps000 的两态逐字对拍",
                &[zdj.as_path(), aps.join("aps7200_0001").as_path()],
            ) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let mut windows: Vec<(PathBuf, u32, u32)> = vec![
                (aps.join("aps7200_0001"), 3, 4),
                (aps.join("aps250514_0001"), 3, 4),
            ];
            for name in ["aps250176_0001", "aps250179_0001"] {
                let path = aps.join(name);
                if let Ok(mut eng) = ReadOnlyEngine::open(&path) {
                    if let Ok(sessions) = eng.sessions() {
                        let mut sesnos: Vec<u32> = sessions.iter().map(|s| s.session_id).collect();
                        sesnos.sort_unstable();
                        sesnos.dedup();
                        if !sesnos.is_empty() {
                            if let Ok(steps) = eng.diff_steps(
                                SessionSelector::Exact(sesnos[0]),
                                SessionSelector::Latest,
                            ) {
                                for step in &steps {
                                    windows.push((path.clone(), step.from.sesno, step.to.sesno));
                                }
                            }
                        }
                    }
                }
            }
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

        /// 跨库版（GitHub #35）真库：AvevaPlantSample 里电缆在电缆库、端点 `EQUI` / `ELCONN` 在别的库（C6 普查
        /// `CABLE.STAREF` / `ENDREF` 跨库 196 条，端点库 250152/154/156/160/162/164 及副本）。语料里没有「只改端点」的
        /// 自然窗口，与同库版同一手法：从电缆正向读出跨库端点边，**假定**那台设备变了，用生产同一段
        /// [`read_foreign_referrers`] 到其余设计库的反向表去查——必须查回那根电缆（或它的 `EXTGEO` 段），再用跨库交接
        /// 同一段 [`crate::data_interface::cross_db_handoff::fold_elements_to_roots`] 在电缆库的 T 端索引上折回 `CABLE` 根。
        /// 只读文件：`AIOS_APS_PROJECT_DIR`（默认 `D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000`）+ `AIOS_E3D_TEMPLATE_DIR`。
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_aps_a_cross_db_endpoint_finds_its_cable_in_the_other_library() {
            use crate::data_interface::db_file_header::DbFileHeader;

            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            if !corpus_ready("aps000 的跨库电缆端点", &[dir.as_path()]) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);
            let mut desi: Vec<(u32, PathBuf, u32)> = std::fs::read_dir(&dir)
                .expect("read project dir")
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.is_file())
                .filter_map(|path| {
                    let header = DbFileHeader::read(&path).ok()?;
                    header
                        .db_type
                        .eq_ignore_ascii_case("DESI")
                        .then(|| (header.dbnum as u32, path, header.latest_sesno()))
                })
                .collect();
            desi.sort();
            desi.dedup_by_key(|(dbnum, _, _)| *dbnum);
            assert!(!desi.is_empty(), "{} 下没有 DESI 库文件", dir.display());

            // MDB 同款：全部 DESI 钉最新放进一份 DbSet，正向读得到跨库的端点。
            let all = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            for (_, path, sesno) in &desi {
                all.add_db(DbFilePin {
                    file: path.clone(),
                    template: template_file_for(schema.template_dir(), "DESI")
                        .expect("DESI template"),
                    db_type: Some("DESI".into()),
                    sesno: Some(*sesno),
                })
                .expect("pin DESI");
            }
            struct Lib {
                owners: Arc<BTreeMap<(u32, u32), RefNo>>,
                roots: Vec<GenerationRoot>,
            }
            let mut libs: BTreeMap<u32, Lib> = BTreeMap::new();
            for (dbnum, path, sesno) in &desi {
                let index = scan_index(path, Some(*sesno)).expect("scan index");
                let roots = enumerate_generation_roots(&all, &index.roots, &unit_types)
                    .expect("enumerate roots");
                libs.insert(
                    *dbnum,
                    Lib {
                        owners: index.owners.clone(),
                        roots,
                    },
                );
            }

            // 电缆根正向读 STAREF / ENDREF，留指向别的库、且那个库在目录里的端点边。
            let mut edges: Vec<(RefNo, &'static str, RefNo)> = Vec::new();
            for (dbnum, lib) in &libs {
                for root in lib
                    .roots
                    .iter()
                    .filter(|root| root.noun.eq_ignore_ascii_case("CABLE"))
                {
                    let cable = refno_to_e3d(root.root);
                    let element = all.element(cable);
                    for attribute in REFERENCE_ATTRIBUTES {
                        let Ok(Some(target)) = element.get_ref(attribute) else {
                            continue;
                        };
                        if target.is_valid()
                            && target.dbno() != *dbnum
                            && libs.contains_key(&target.dbno())
                        {
                            edges.push((cable, attribute, target));
                        }
                    }
                }
            }
            println!(
                "[#35] {} 个 DESI 库；跨库电缆端点边 {} 条（目标库在目录里的）",
                desi.len(),
                edges.len()
            );
            assert!(
                !edges.is_empty(),
                "C6 普查说 APS 有跨库端点边——一条都没找到，先看语料是否换过"
            );

            let members: Vec<SourceDb> = desi
                .iter()
                .map(|(dbnum, path, sesno)| SourceDb {
                    dbnum: *dbnum,
                    db_type: "DESI".into(),
                    file: path.clone(),
                    sesno: Some(*sesno),
                })
                .collect();
            let mut checked = 0usize;
            for (cable, attribute, target) in edges.iter().take(8) {
                let own = target.dbno();
                let noun = all.element(*target).element_type().unwrap_or_default();
                let Some(rule) = rule_for(&noun) else {
                    println!(
                        "[#35] skip {cable}.{attribute} -> {target}: 端点 noun `{noun}` 不在规则 F 里"
                    );
                    continue;
                };
                let source = CascadeSource {
                    refno: *target,
                    noun: noun.clone(),
                    rule,
                };
                let foreign = read_foreign_referrers(&[source], &libs[&own].owners, own, &members);
                let cable_db = cable.dbno();
                let found: Vec<RefNo> = foreign
                    .referrers
                    .get(&cable_db)
                    .map(|keys| {
                        keys.iter()
                            .map(|&(word0, word1)| RefNo::new(word0, word1))
                            .collect()
                    })
                    .unwrap_or_default();
                let folded = crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                    &libs[&cable_db].owners,
                    &libs[&cable_db].roots,
                    &found,
                );
                println!(
                    "[#35] 假定 {noun} {target}（dbnum {own}）变了：members_queried={} queries={} \
                     unreadable={:?} 引用者 {:?} → 折回电缆库 {cable_db} 的根 {:?}",
                    foreign.members_queried,
                    foreign.queries,
                    foreign.unreadable_members.keys().collect::<Vec<_>>(),
                    foreign.referrers,
                    folded.roots
                );
                assert!(
                    foreign.unreadable_members.is_empty(),
                    "目录里的设计库都该开得了：{:?}",
                    foreign.unreadable_members
                );
                assert!(
                    !foreign.referrers.contains_key(&own),
                    "本库的表不在跨库版里查"
                );
                assert!(
                    folded
                        .roots
                        .iter()
                        .any(|root| root.refno == refno_from_e3d(*cable).to_pdms_str()),
                    "{cable}.{attribute} -> {target}：跨库点查必须查回这根电缆的根，实得 {:?}",
                    folded.roots
                );
                checked += 1;
            }
            assert!(checked > 0, "前 8 条跨库端点边的端点都不在规则 F 里");

            // 规则 E（#35 Phase C）：EXTGEO 段正向读 STAREF / ENDREF，收集指向其它库桥架附着点
            // （HATTA / TATTA / RATTA / RNODE）的跨库路由边（aps250179 → aps250176 共 152 条）。
            let mut route_edges: Vec<(RefNo, RefNo, &'static str, RefNo)> = Vec::new();
            for (dbnum, lib) in &libs {
                for root in lib
                    .roots
                    .iter()
                    .filter(|root| root.noun.eq_ignore_ascii_case("CABLE"))
                {
                    let cable = refno_to_e3d(root.root);
                    let Ok(cable_members) = all.element(cable).members() else {
                        continue;
                    };
                    for member in cable_members {
                        if !member
                            .element_type()
                            .is_ok_and(|t| t.eq_ignore_ascii_case("RPATH"))
                        {
                            continue;
                        }
                        let Ok(rpath_members) = member.members() else {
                            continue;
                        };
                        for seg in rpath_members {
                            if !seg
                                .element_type()
                                .is_ok_and(|t| t.eq_ignore_ascii_case("EXTGEO"))
                            {
                                continue;
                            }
                            let extgeo = seg.refno();
                            for attribute in REFERENCE_ATTRIBUTES {
                                let Ok(Some(target)) = seg.get_ref(attribute) else {
                                    continue;
                                };
                                if target.is_valid()
                                    && target.dbno() != *dbnum
                                    && libs.contains_key(&target.dbno())
                                {
                                    route_edges.push((cable, extgeo, attribute, target));
                                }
                            }
                        }
                    }
                }
            }
            println!(
                "[#35] 跨库 EXTGEO 路由边 {} 条（目标库在目录里的）",
                route_edges.len()
            );
            assert!(
                !route_edges.is_empty(),
                "C6 普查说 APS 有 152 条跨库 EXTGEO 路由边——一条都没找到"
            );

            let mut route_checked = 0usize;
            for (cable, extgeo, attribute, target) in route_edges.iter().take(8) {
                let own = target.dbno();
                let noun = all.element(*target).element_type().unwrap_or_default();
                let rule = rule_for(&noun).unwrap_or_else(|| {
                    panic!("{extgeo}.{attribute} -> {target}：附着点 noun `{noun}` 必须在规则 E 里")
                });
                assert_eq!(rule, Rule::Route);

                // (1) 附着点自身（HATTA / TATTA / RATTA / RNODE）改动 → 跨库点查命中 EXTGEO 并折回 CABLE 根
                let source = CascadeSource {
                    refno: *target,
                    noun: noun.clone(),
                    rule,
                };
                let foreign = read_foreign_referrers(&[source], &libs[&own].owners, own, &members);
                let cable_db = cable.dbno();
                let found: Vec<RefNo> = foreign
                    .referrers
                    .get(&cable_db)
                    .map(|keys| {
                        keys.iter()
                            .map(|&(word0, word1)| RefNo::new(word0, word1))
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    found.contains(extgeo),
                    "假定附着点 {noun} {target} 改动：跨库点查必须直接命中 {extgeo}，实得 {found:?}"
                );
                let folded = crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                    &libs[&cable_db].owners,
                    &libs[&cable_db].roots,
                    &found,
                );
                assert!(
                    folded
                        .roots
                        .iter()
                        .any(|root| root.refno == refno_from_e3d(*cable).to_pdms_str()),
                    "{extgeo}.{attribute} -> {target}：EXTGEO 必须沿 owner 链折回电缆根 {cable}，实得 {:?}",
                    folded.roots
                );

                // (2) 所属 CWBRAN 下的 RPATH 改动 → sources_from_plan_with_owners 上卷到 CWBRAN →
                //     query_points 展开整棵 CWBRAN 子树 → 跨库点查同样命中 EXTGEO 并折回 CABLE 根
                let cwbran_keys: BTreeSet<(u32, u32)> = libs[&own]
                    .roots
                    .iter()
                    .filter(|r| r.noun.eq_ignore_ascii_case("CWBRAN"))
                    .map(|r| {
                        let er = refno_to_e3d(r.root);
                        (er.word0, er.word1)
                    })
                    .collect();
                let cwbran_owners = roots_on_owner_chain(&libs[&own].owners, &cwbran_keys, *target);
                assert!(
                    !cwbran_owners.is_empty(),
                    "附着点 {target} 在 dbnum {own} 必须挂在某个 CWBRAN 根名下"
                );
                let cwbran = cwbran_owners[0];
                let rpath = all
                    .element(cwbran)
                    .members()
                    .expect("cwbran members")
                    .into_iter()
                    .find(|m| {
                        m.element_type()
                            .is_ok_and(|t| t.eq_ignore_ascii_case("RPATH"))
                    })
                    .expect("CWBRAN 必有 RPATH")
                    .refno();
                let mut ledger = e3d_model::ledger::ChangeLedger::default();
                ledger.record(
                    rpath,
                    &e3d_model::element_diff::ElementDiff {
                        refno: rpath.to_string(),
                        noun: "RPATH".into(),
                        type_changed: None,
                        owner_changed: None,
                        members: None,
                        attributes: vec!["POS".into()],
                        opaque: false,
                    },
                );
                ledger.dispose(rpath, e3d_model::ledger::Disposition::RolledUp);
                let plan = e3d_model::increment::UpdatePlan {
                    regenerate: BTreeSet::new(),
                    remove: BTreeSet::new(),
                    regenerate_derived: BTreeSet::new(),
                    derived_stale: BTreeSet::new(),
                    ledger,
                    full_rebuild: None,
                    report: e3d_model::increment::IncrementReport::default(),
                };
                let branch_sources =
                    sources_from_plan_with_owners(&plan, &libs[&own].owners, &libs[&own].roots);
                assert_eq!(
                    branch_sources,
                    vec![CascadeSource {
                        refno: cwbran,
                        noun: "CWBRAN".into(),
                        rule: Rule::Route,
                    }]
                );
                let branch_foreign =
                    read_foreign_referrers(&branch_sources, &libs[&own].owners, own, &members);
                let branch_found: Vec<RefNo> = branch_foreign
                    .referrers
                    .get(&cable_db)
                    .map(|keys| {
                        keys.iter()
                            .map(|&(word0, word1)| RefNo::new(word0, word1))
                            .collect()
                    })
                    .unwrap_or_default();
                let branch_folded = crate::data_interface::cross_db_handoff::fold_elements_to_roots(
                    &libs[&cable_db].owners,
                    &libs[&cable_db].roots,
                    &branch_found,
                );
                println!(
                    "[#35] 假定 CWBRAN {cwbran} 的 RPATH {rpath}（dbnum {own}）变了：queries={}                      引用者数={} → 折回电缆库 {cable_db} 的根数={}",
                    branch_foreign.queries,
                    branch_found.len(),
                    branch_folded.roots.len()
                );
                assert!(
                    branch_folded
                        .roots
                        .iter()
                        .any(|root| root.refno == refno_from_e3d(*cable).to_pdms_str()),
                    "RPATH {rpath} (under CWBRAN {cwbran}) 改动后，跨库级联必须查回电缆根 {cable}"
                );
                route_checked += 1;
            }
            assert!(route_checked > 0, "跨库 EXTGEO 路由边至少应核验 1 条");
        }

        /// 九个钉死窗口（`window_root_plan::tests::live_ams8000_pinned_windows_land_in_the_expected_buckets`
        /// 的表）开关两态 `regen` 逐字相同——它们没有跨根的电缆端点或桥架路由改动。只读文件：
        /// `AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off() {
            let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
                |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
            ));
            if !corpus_ready("ams8000 的九个钉死窗口", &[file.as_path()]) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let windows = [
                (255u32, 256u32),
                (195, 196),
                (45, 46),
                (24, 26),
                (266, 267),
                (267, 268),
                (268, 269),
                (269, 270),
                (270, 271),
            ];
            let unit_types = resolve_delivery_unit_types(&[]);
            for (base_sesno, target_sesno) in windows {
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
                let ((on, regen_on), (_, regen_off)) = (run(true), run(false));
                println!(
                    "ams8000 {base_sesno}→{target_sesno}: {} | regen_on={} regen_off={}",
                    on.summary_line(8000),
                    regen_on.len(),
                    regen_off.len()
                );
                assert_eq!(
                    regen_on, regen_off,
                    "ams8000 {base_sesno}→{target_sesno}: 两态 regen 必须逐字相同"
                );
            }
        }
    }
}
