//! C2 —— direct 窗口规划器的结构连接正向级联（core `change/EVALAT` 规则 K → `ADSTCH` 同形）。
//!
//! 窗口 S→T 里的变更元素若是持连接属性的结构 noun，**正向**读它身上的连接属性
//! （`SCTN` / `GENSEC` / `STWALL` / `WALL` 的 `JOIS` / `JOIE`；`PJOI` / `SJOI` / `SUBJ` / `FIXING`
//! 的 `CREF`；`CMPF` 的 `CFRA`），逐个到对端读回指属性（`CREF` ↔ `JOIS` / `JOIE`、`CFRA` ↔ `PFRE`），
//! **等于自己才算一条边**；再把对端 owner 链上落在 `roots_T` 里的生成根并进 `touched`，于是
//! `regen = touched ∩ roots_T` 会带上对端的根。只正向读、不建反向表——与 core 一样。
//!
//! 为什么要这一步：`touched` 只按祖先闭包算（「根不在闭包里 = 子树里没有任何一件受影响的东西」），
//! 而结构连接是**跨子树**的引用：改一个 `SJOI`，它 `CREF` 指的 `SCTN` 常常在另一个 `FRMW` / `SBFR`
//! 之下，那一根的几何（关节负体、端部切割）要跟着重算，闭包却看不见它。真库普查
//! （`docs/evidence/2026-09-07-cross-root-reference-census.md`）：486 库 21,161 条回指成立的跨根边，
//! 10 个库 12 个「只改一端」的窗口。
//!
//! 边界与护栏（计划 `2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C2、共识 d-144）：
//! - 读属性走 [`DbElement::get_ref`]（noun 模板解隐式区）——`JOIS` / `JOIE` / `CREF` 都在定长槽位，
//!   只读显式尾块一条都读不到；
//! - `STRU` 门：core 规则 K 先 `CGETEL(STRU)`，src 的 owner 链上没有 `STRU` 就不做 `ADSTCH`。语料里
//!   缺口边 100% 在 `STRU` 下，这里照样加上；owner 链读挂时保守放行；
//! - 读哪一端：`Deleted` 读 S（T 上已经没有它），`TypeChanged` 两端都读，其余读 T；
//! - 对端 → 根：沿 T 端 owner 索引向上（含自身），链上**每一个**落在 `roots_T` 里的根都算——与闭包
//!   「按祖先封闭」同一口径（嵌套显著根 STRU / FRMW / SBFR 各管各的几何，都要重算）；
//! - 开关 `model_structural_connection_cascade`（`DbOption.toml` /
//!   `AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE`），**默认关**；关着时一次文件读都不发生，`touched` 一字不改。
//!
//! **跨库对端（GitHub #33，2026-09-22 探针 `live_probe_cross_db_structural_connection_edges`）：读得到、上卷不到。**
//! 生产的 `DbSet@T` 由 `E3dModelService::build_set` 造，MDB 全部 DESI 都注册（窗口那个库钉 T、其余钉当前 pin），
//! 所以对端落在别的 DESI 库时 `backref_holds` 照样读到、回指照样核对——AvevaPlantSample 98 个 DESI 库里 12 条跨库边
//! （`FIXING.CREF ↔ GENSEC.JOIE`，250104↔250112 / 250108↔250110 / 250558↔250564 / 250560↔250562）12/12 `Some(true)`。
//! 但 [`StructuralConnectionCascade::collect`] 拿到的 `owners` 与 `roots_T` 是**窗口那一个文件**的索引与根集
//! （`WindowRootSources::collect` 只扫 `file`），对端的根（对端库自己的索引能找到，12/12）**一个都进不了 `touched`**。
//! 要覆盖它得有跨 dbnum 的工作交接——对端的根属于另一个库的任务，本窗的计划装不下它。**2026-10-01 落地（#34 前置 2）**：
//! 形态没有照房间反查的停放槽（ADR-073 D13 U3 b `RoomRoots`，那是按「代」计欠账才要的），而是走目录级联同一条路——
//! 计划带着跨库对端（`ModelUpdatePlan::cross_db_elements`），窗口落地之后 [`crate::data_interface::cross_db_handoff`]
//! 在对端库的文件最新会话上折根，给那个库排一条只带名单的 `ModelOnly`。
//!
//! **跨库的账单独记（2026-09-30，#34 前置 1）**：会级联的边里，对端库号（[`RefNo::dbno`]；不比 `word0`——同一个库
//! 换 extent `word0` 就变）与源不同的，不再拿本窗的 owner 索引去查、也不再混进 `far_end_without_root`（那一格只剩
//! 「对端已删 / 无根族」），记进 [`StructuralConnectionCascade::far_end_cross_db`]，并由
//! [`StructuralConnectionCascade::summary_line`] 逐条点名，随窗口回执 `warnings` 出去。只记账、不上卷：`touched` /
//! `regen` 与拆账前逐字相同。
//!
//! **几何上显了一半（2026-09-28 起）**：e3d-model 第 4 阶段的 GENSEC 一支已落地（`d510253` 起 GENSEC 两端扣 `JOIS` /
//! `JOIE` 关节 FIXING 名下 TMPL 的负体，v0.1.31 `d3355a8` 收口），同时 e3d-model 自己的 `plan_update` 补了同库关节级联
//! （`increment::joint_consumer_of`：FIXING 进计划集 → 指着它的 GENSEC 一并重建；`b820a4a` 的 `joint_fixings_of` 管反方向）。
//! 所以同库 FIXING ↔ GENSEC 这一类，开关关着对端根也已在 `regen` 里（`aps250110_0001` 29→30 两态都是 258 根，见
//! `tests::live`）。**SCTN 那一支 2026-10-23 落地**（e3d-model `28d0402`，计划 §3 C2 ★0907）：带目录正体、标架有样本的
//! SJOI 自成单元，法兰端板的负体切梁端，`plan_update` 的同库关节级联也推广到 SJOI ↔ SCTN——`aps7200_0001` 3→4
//! （94 个 `SJOI` 单独改）开关关着也是 8 根（此前关 6 / 开 8）。**主关节 2026-11-12 落地**：带目录正体的 `PJOI` ↔ 认领它的 SCTN
//! （柱被端板削短、主关节的原点落在梁线上）同样由 `plan_update` 的同库关节级联接住，挪 PNOD 也带上它名下的主关节
//! ——见 `fast_model::unit_publish_cascade_target_tests` 的 aps7166 测试。仍只有本开关补得上的是 `SUBJ` → SCTN、
//! 不带目录正体的 `PJOI`、标架没有样本的 SJOI（连接梁转了 ±90°、连的就是宿主自己）与 `CMPF.CFRA`。跨库那一格
//! 两边都盖不住：
//! e3d-model 对 `CREF` 指到别的库的关节只记一条 note（`JointConsumer::OtherDatabase`）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aios_core::RefnoEnum;
use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::increment::UpdatePlan;
use e3d_model::ledger::ChangeKind;

use crate::data_interface::generation_root::{GenerationRoot, refno_from_e3d, refno_to_e3d};

/// owner 链上溯的步数上限（与 `window_root_plan` 里其它链走法同值）。
const OWNER_CHAIN_LIMIT: usize = 256;

/// 回执里逐条点名的跨库边上限（与删除覆盖账 `delete_coverage_gap_warning` 同值），多出来的只报条数。
const CROSS_DB_NAMED_LIMIT: usize = 12;

/// 持连接属性的结构 noun → 要正向读的属性。
///
/// 表是 C1 普查给的语料边界（55,503 条结构连接边的持属性端只有这九个 noun）加 core `ADSTCH`
/// 的三组（证据 `2026-09-06-ida-core3d-change-evalat-cascade.md` §3）。别的 noun 给空表 = 不读。
pub fn forward_attributes(noun: &str) -> &'static [&'static str] {
    match noun.trim().to_ascii_uppercase().as_str() {
        "SCTN" | "GENSEC" | "STWALL" | "WALL" => &["JOIS", "JOIE"],
        "PJOI" | "SJOI" | "SUBJ" | "FIXING" => &["CREF"],
        "CMPF" => &["CFRA"],
        _ => &[],
    }
}

/// 正向属性 → 对端必须回指的属性（`ADSTCH` 的双向核对）。
pub fn back_pointers(attr: &str) -> &'static [&'static str] {
    match attr.trim().to_ascii_uppercase().as_str() {
        "CREF" => &["JOIS", "JOIE"],
        "JOIS" | "JOIE" => &["CREF"],
        "CFRA" => &["PFRE"],
        "PFRE" => &["CFRA"],
        _ => &[],
    }
}

/// 读边时看的是窗口的哪一端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowSide {
    Base,
    Target,
}

/// 从一个变更元素正向读出来的一条连接边——I/O 的产物；判定只看它的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionEdge {
    pub side: WindowSide,
    pub source: RefNo,
    pub source_noun: String,
    pub attribute: &'static str,
    pub target: RefNo,
    /// 对端的回指属性里有一个等于 `source`。`Some(false)` = 对端在、但没有指回来
    /// （含对端 noun 根本没有回指属性）；`None` = 对端不存在或读挂。
    pub backref: Option<bool>,
    /// `source` 的 owner 链上有 `STRU`（core 规则 K 的 `CGETEL(STRU)`）。`None` = 链读挂。
    pub under_stru: Option<bool>,
}

impl ConnectionEdge {
    /// core 会不会为这条边做 `ADSTCH`：回指成立，且 `STRU` 门放行（链读挂保守放行）。
    pub fn cascades(&self) -> bool {
        self.backref == Some(true) && self.under_stru != Some(false)
    }
}

/// 读边的产物与账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionEdges {
    pub edges: Vec<ConnectionEdge>,
    /// 账本 noun 落在结构表里、真去读了的 (元素, 端) 数。
    pub sources_read: usize,
    /// 读 source 本身就挂了的 (元素, 端) 数。
    pub unreadable_sources: usize,
}

/// I/O：从账本挑结构 noun 的变更元素，正向读连接属性，到对端核对回指。
///
/// 账本 noun 先做零 I/O 粗筛，所以没有结构改动的窗口一次文件读都不发生；`TypeChanged`
/// 两端 noun 不同，交给元素自报。
pub fn read_connection_edges(
    plan: &UpdatePlan,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
) -> ConnectionEdges {
    let mut out = ConnectionEdges::default();
    let mut seen = BTreeSet::new();
    for entry in plan.ledger.entries() {
        let sides: &[WindowSide] = match entry.kind {
            ChangeKind::Deleted => &[WindowSide::Base],
            ChangeKind::TypeChanged => &[WindowSide::Base, WindowSide::Target],
            _ => &[WindowSide::Target],
        };
        if entry.kind != ChangeKind::TypeChanged && forward_attributes(&entry.noun).is_empty() {
            continue;
        }
        for &side in sides {
            if !seen.insert((entry.refno.word0, entry.refno.word1, side)) {
                continue;
            }
            let set = match side {
                WindowSide::Base => base,
                WindowSide::Target => target,
            };
            match read_edges_of(set, entry.refno, side, &mut out.edges) {
                Ok(()) => out.sources_read += 1,
                Err(_) => out.unreadable_sources += 1,
            }
        }
    }
    out
}

fn read_edges_of(
    set: &Arc<DbSet>,
    refno: RefNo,
    side: WindowSide,
    edges: &mut Vec<ConnectionEdge>,
) -> Result<(), DbElementError> {
    let element = set.element(refno);
    if !element.exists()? {
        return Ok(());
    }
    let noun = element.element_type()?;
    let attributes = forward_attributes(&noun);
    if attributes.is_empty() {
        return Ok(());
    }
    let under_stru = under_stru(&element);
    for &attribute in attributes {
        if !element.has_attribute(attribute)? {
            continue;
        }
        let Some(target) = element.get_ref(attribute)? else {
            continue;
        };
        let backref = backref_holds(set, target, attribute, refno);
        edges.push(ConnectionEdge {
            side,
            source: refno,
            source_noun: noun.clone(),
            attribute,
            target,
            backref,
            under_stru,
        });
    }
    Ok(())
}

/// 对端的回指属性里有没有一个等于 `source`。`None` = 对端不存在或读挂。
fn backref_holds(set: &Arc<DbSet>, target: RefNo, attribute: &str, source: RefNo) -> Option<bool> {
    let element = set.element(target);
    if !element.exists().ok()? {
        return None;
    }
    for &back in back_pointers(attribute) {
        if !element.has_attribute(back).ok()? {
            continue;
        }
        if element.get_ref(back).ok()? == Some(source) {
            return Some(true);
        }
    }
    Some(false)
}

/// `element` 的 owner 链上有没有 `STRU`。`None` = 链读挂或成环。
fn under_stru(element: &DbElement) -> Option<bool> {
    let mut seen = BTreeSet::new();
    seen.insert((element.refno().word0, element.refno().word1));
    let mut cursor = element.owner().ok()?;
    for _ in 0..OWNER_CHAIN_LIMIT {
        let Some(owner) = cursor else {
            return Some(false);
        };
        if !seen.insert((owner.refno().word0, owner.refno().word1)) {
            return None;
        }
        if owner.element_type().ok()?.eq_ignore_ascii_case("STRU") {
            return Some(true);
        }
        cursor = owner.owner().ok()?;
    }
    None
}

/// 纯函数：`start` 的 owner 链上（含自身）落在 `roots` 里的全部根，自下而上。
///
/// 与祖先闭包同一口径——一个元素受影响，它上面每一个嵌套的显著根都要重算。`start` 不在
/// `owners` 里（T 端已删）给空。
pub fn roots_on_owner_chain(
    owners: &BTreeMap<(u32, u32), RefNo>,
    roots: &BTreeSet<(u32, u32)>,
    start: RefNo,
) -> Vec<RefNo> {
    let mut out = Vec::new();
    let mut current = (start.word0, start.word1);
    let mut seen = BTreeSet::new();
    for _ in 0..OWNER_CHAIN_LIMIT {
        if !seen.insert(current) {
            break;
        }
        if roots.contains(&current) {
            out.push(RefNo::new(current.0, current.1));
        }
        let Some(owner) = owners.get(&current) else {
            break;
        };
        let owner_key = (owner.word0, owner.word1);
        if !owner.is_valid() || owner_key == current {
            break;
        }
        current = owner_key;
    }
    out
}

/// 一窗的结构连接级联：账 + 要并进 `touched` 的对端根。挂在 `WindowRootSources` 上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructuralConnectionCascade {
    /// 开关态。`false` = 一次文件读都没发生，[`Self::expand_touched`] 不动 `touched`。
    pub enabled: bool,
    pub sources_read: usize,
    pub unreadable_sources: usize,
    pub edges: usize,
    pub backref_ok: usize,
    pub backref_mismatch: usize,
    pub backref_unreadable: usize,
    /// 回指成立但 src 不在 `STRU` 下（core 规则 K 不触发）。
    pub gated_by_stru: usize,
    /// 会级联的**同库**边里，对端在 T 端 owner 链上找不到任何生成根（对端已删 / 无根族）。
    /// 跨库的对端不在这一格，见 [`Self::far_end_cross_db`]。
    pub far_end_without_root: usize,
    /// 会级联的边里，对端落在**别的库**（[`RefNo::dbno`] 与源不同）：它的根属于另一个库的任务，本窗的计划装不下，
    /// 本窗不上卷（#34 前置 1）；计划带着它们（`ModelUpdatePlan::cross_db_elements`），落地之后交给那个库的任务
    /// （[`crate::data_interface::cross_db_handoff`]，#34 前置 2，2026-10-01）。按出现序，[`Self::summary_line`] 逐条点名。
    pub far_end_cross_db: Vec<CrossDbFarEnd>,
    /// 要并进 `touched` 的对端根（都在 `roots_T` 里）。
    pub far_end_roots: BTreeSet<RefnoEnum>,
}

/// 一条会级联、但对端在别的库的结构连接边（[`StructuralConnectionCascade::far_end_cross_db`] 的一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDbFarEnd {
    pub source: RefNo,
    pub source_noun: String,
    pub attribute: &'static str,
    pub target: RefNo,
}

impl std::fmt::Display for CrossDbFarEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}.{} -> {} (dbnum {})",
            self.source_noun,
            self.source,
            self.attribute,
            self.target,
            self.target.dbno()
        )
    }
}

impl StructuralConnectionCascade {
    /// 开关关：零 I/O、零改动。
    pub fn off() -> Self {
        Self::default()
    }

    /// 纯函数：从读出的边折出对端根集。`root_chain(对端)` 给对端 owner 链上的根
    /// （生产传 [`roots_on_owner_chain`]，测试可以手搓）。
    pub fn from_edges(
        edges: &ConnectionEdges,
        mut root_chain: impl FnMut(RefNo) -> Vec<RefNo>,
    ) -> Self {
        let mut out = Self {
            enabled: true,
            sources_read: edges.sources_read,
            unreadable_sources: edges.unreadable_sources,
            edges: edges.edges.len(),
            ..Self::default()
        };
        for edge in &edges.edges {
            match edge.backref {
                Some(true) => out.backref_ok += 1,
                Some(false) => out.backref_mismatch += 1,
                None => out.backref_unreadable += 1,
            }
            if edge.backref == Some(true) && edge.under_stru == Some(false) {
                out.gated_by_stru += 1;
            }
            if !edge.cascades() {
                continue;
            }
            // 跨库对端：本窗的 owner 索引与 `roots_T` 只有窗口那一个库，查了也是空（#33），所以不查，单独记账。
            // 比库号不比 `word0`：同一个库的元素换了 extent，`word0` 就不同。
            if edge.target.dbno() != edge.source.dbno() {
                out.far_end_cross_db.push(CrossDbFarEnd {
                    source: edge.source,
                    source_noun: edge.source_noun.clone(),
                    attribute: edge.attribute,
                    target: edge.target,
                });
                continue;
            }
            let roots = root_chain(edge.target);
            if roots.is_empty() {
                out.far_end_without_root += 1;
            }
            out.far_end_roots
                .extend(roots.into_iter().map(refno_from_e3d));
        }
        out
    }

    /// I/O 版：读边，再按 T 端 owner 索引与 `roots_T` 把对端映射成根。
    pub fn collect(
        plan: &UpdatePlan,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        owners: &BTreeMap<(u32, u32), RefNo>,
        roots_t: &[GenerationRoot],
    ) -> Self {
        let edges = read_connection_edges(plan, base, target);
        let root_keys: BTreeSet<(u32, u32)> = roots_t
            .iter()
            .map(|root| {
                let refno = refno_to_e3d(root.root);
                (refno.word0, refno.word1)
            })
            .collect();
        Self::from_edges(&edges, |far_end| {
            roots_on_owner_chain(owners, &root_keys, far_end)
        })
    }

    /// 把对端根并进 `touched`，回新增的个数。开关关 = 不动。
    pub fn expand_touched(&self, touched: &mut BTreeSet<RefnoEnum>) -> usize {
        if !self.enabled {
            return 0;
        }
        self.far_end_roots
            .iter()
            .filter(|root| touched.insert(**root))
            .count()
    }

    /// 回执里的一行观测。
    pub fn summary_line(&self, dbnum: u32) -> String {
        if !self.enabled {
            return format!(
                "dbnum={dbnum}: structural-connection cascade off — touched unchanged \
                 (model_structural_connection_cascade=false)"
            );
        }
        let mut line = format!(
            "dbnum={dbnum}: structural-connection cascade on — sources_read={} \
             unreadable_sources={} edges={} backref_ok={} backref_mismatch={} \
             backref_unreadable={} gated_by_stru={} far_end_without_root={} \
             far_end_cross_db={} far_end_roots={}",
            self.sources_read,
            self.unreadable_sources,
            self.edges,
            self.backref_ok,
            self.backref_mismatch,
            self.backref_unreadable,
            self.gated_by_stru,
            self.far_end_without_root,
            self.far_end_cross_db.len(),
            self.far_end_roots.len()
        );
        if !self.far_end_cross_db.is_empty() {
            let named: Vec<String> = self
                .far_end_cross_db
                .iter()
                .take(CROSS_DB_NAMED_LIMIT)
                .map(ToString::to_string)
                .collect();
            let more = self
                .far_end_cross_db
                .len()
                .saturating_sub(CROSS_DB_NAMED_LIMIT);
            line.push_str(&format!(
                "; cross-db far ends not rolled up here — their roots belong to another dbnum's task \
                 and are handed off to it after the window lands (GitHub #34): [{}]{}",
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

    const DB: u32 = 24384;

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

    /// SCTN A(=11) 在根 R1(=1) 下，节点 J(=21) 在根 R2(=2) 下；A.JOIS → J。
    fn edge(backref: Option<bool>, under_stru: Option<bool>) -> ConnectionEdges {
        ConnectionEdges {
            edges: vec![ConnectionEdge {
                side: WindowSide::Target,
                source: r(11),
                source_noun: "SCTN".into(),
                attribute: "JOIS",
                target: r(21),
                backref,
                under_stru,
            }],
            sources_read: 1,
            unreadable_sources: 0,
        }
    }

    /// 元素 → 根链：11 在 1 下，21 在 2 下，31 在 1 下（与 A 同根），41 无根。
    fn chain(far_end: RefNo) -> Vec<RefNo> {
        match far_end.word1 {
            11 | 31 => vec![r(1)],
            21 => vec![r(2)],
            _ => vec![],
        }
    }

    /// 改 A：闭包只碰到 root(A)；把级联并进 `touched` 后走生产同一段 `plan_window_roots`。
    fn regen_with(cascade: &StructuralConnectionCascade) -> Vec<RefnoEnum> {
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
        let roots_t = vec![root(1, "FRMW"), root(2, "FRMW")];
        plan_window_roots(&roots_t, &[], &impact)
            .regen
            .iter()
            .map(|root| root.root)
            .collect()
    }

    /// 契约第一态：回指成立 → `regen ⊇ {root(A), root(J)}`。
    #[test]
    fn a_backref_that_holds_pulls_the_far_end_root_into_regen() {
        let cascade = StructuralConnectionCascade::from_edges(&edge(Some(true), Some(true)), chain);
        assert_eq!(cascade.backref_ok, 1);
        assert_eq!(cascade.far_end_roots, set(&[2]));
        assert_eq!(regen_with(&cascade), vec![e(1), e(2)]);
    }

    /// 契约第二态：J 的 `CREF` 指着别人（回指不成立）→ 只有 `root(A)`。
    /// 这一条钉住「`ADSTCH` 是双向核对，不是单向引用」（计划 §6 风险 1）。
    #[test]
    fn a_backref_that_does_not_hold_adds_nothing() {
        let cascade =
            StructuralConnectionCascade::from_edges(&edge(Some(false), Some(true)), chain);
        assert_eq!(cascade.backref_mismatch, 1);
        assert!(cascade.far_end_roots.is_empty());
        assert_eq!(regen_with(&cascade), vec![e(1)]);
    }

    /// 契约第三态：开关关 → 只有 `root(A)`，而且 `touched` 一字不改。
    #[test]
    fn the_switch_off_leaves_touched_untouched() {
        let cascade = StructuralConnectionCascade::off();
        assert!(!cascade.enabled);
        let mut touched = set(&[1]);
        assert_eq!(cascade.expand_touched(&mut touched), 0);
        assert_eq!(touched, set(&[1]));
        assert_eq!(regen_with(&cascade), vec![e(1)]);
    }

    /// 对端读不出（不存在 / 读挂）不算成立——宁漏勿错的方向是「不多算」，因为多算的根会
    /// 白重算，而对端本就不存在时也没有几何要更新。
    #[test]
    fn an_unreadable_far_end_is_not_a_backref() {
        let cascade = StructuralConnectionCascade::from_edges(&edge(None, Some(true)), chain);
        assert_eq!(cascade.backref_unreadable, 1);
        assert!(cascade.far_end_roots.is_empty());
    }

    /// `STRU` 门：src 不在 `STRU` 下 → core 规则 K 不触发；链读挂则保守放行。
    #[test]
    fn the_stru_gate_blocks_only_a_source_known_to_be_outside_stru() {
        let gated = StructuralConnectionCascade::from_edges(&edge(Some(true), Some(false)), chain);
        assert_eq!(gated.gated_by_stru, 1);
        assert!(gated.far_end_roots.is_empty());

        let unknown = StructuralConnectionCascade::from_edges(&edge(Some(true), None), chain);
        assert_eq!(unknown.gated_by_stru, 0);
        assert_eq!(unknown.far_end_roots, set(&[2]));
    }

    /// 对端与 src 同根 → 并进去也只是原来那一个根；对端无根 → 记账、不加根。
    #[test]
    fn a_far_end_under_the_same_root_or_under_no_root_adds_no_new_root() {
        let mut same_root = edge(Some(true), Some(true));
        same_root.edges[0].target = r(31);
        let cascade = StructuralConnectionCascade::from_edges(&same_root, chain);
        assert_eq!(cascade.far_end_roots, set(&[1]));
        assert_eq!(regen_with(&cascade), vec![e(1)]);

        let mut no_root = edge(Some(true), Some(true));
        no_root.edges[0].target = r(41);
        let cascade = StructuralConnectionCascade::from_edges(&no_root, chain);
        assert_eq!(cascade.far_end_without_root, 1);
        assert!(cascade.far_end_cross_db.is_empty());
        assert!(cascade.far_end_roots.is_empty());
    }

    /// 跨库对端（#34 前置 1）：对端库号与源不同 → 记进 `far_end_cross_db` 并在回执里点名，不拿本窗的根链去查、
    /// 不混进 `far_end_without_root`，`regen` 与没有这条边时逐字相同。
    ///
    /// 对端的 `word1` 故意取 21：同库时 21 在根 2 下（`chain`），查了就会把别的库里恰好同号的元素错并成根 2。
    #[test]
    fn a_cross_db_far_end_is_counted_and_named_but_not_rolled_up() {
        // word0 24384 → dbno 8000（源 A 所在的库），24383 → dbno 7999。
        let other = RefNo::new(24383, 21);
        assert_eq!((r(11).dbno(), other.dbno()), (8000, 7999));
        let mut cross = edge(Some(true), Some(true));
        cross.edges[0].target = other;
        let mut consulted = Vec::new();
        let cascade = StructuralConnectionCascade::from_edges(&cross, |far_end| {
            consulted.push(far_end);
            chain(far_end)
        });
        assert!(
            consulted.is_empty(),
            "跨库对端不该拿本窗的根链去查：{consulted:?}"
        );
        assert_eq!(cascade.backref_ok, 1);
        assert_eq!(cascade.far_end_without_root, 0);
        assert_eq!(
            cascade.far_end_cross_db,
            vec![CrossDbFarEnd {
                source: r(11),
                source_noun: "SCTN".into(),
                attribute: "JOIS",
                target: other,
            }]
        );
        assert!(cascade.far_end_roots.is_empty());
        assert_eq!(regen_with(&cascade), vec![e(1)]);
        let line = cascade.summary_line(8000);
        assert!(
            line.contains("far_end_cross_db=1")
                && line.contains("SCTN 24384/11.JOIS -> 24383/21 (dbnum 7999)"),
            "{line}"
        );

        // 同一个库换了 extent（word0 + 0x2000，库号不变）仍是同库：照旧查根链。
        let next_extent = RefNo::new(24384 + 0x2000, 21);
        assert_eq!(next_extent.dbno(), 8000);
        let mut same_db = edge(Some(true), Some(true));
        same_db.edges[0].target = next_extent;
        let cascade = StructuralConnectionCascade::from_edges(&same_db, |_| vec![r(2)]);
        assert!(cascade.far_end_cross_db.is_empty());
        assert_eq!(cascade.far_end_roots, set(&[2]));

        // 点名封顶 12 条，多出来的只报条数。
        let many = ConnectionEdges {
            edges: (0..13)
                .map(|i| ConnectionEdge {
                    target: RefNo::new(24383, 100 + i),
                    ..cross.edges[0].clone()
                })
                .collect(),
            sources_read: 13,
            unreadable_sources: 0,
        };
        let line = StructuralConnectionCascade::from_edges(&many, chain).summary_line(8000);
        assert!(
            line.contains("far_end_cross_db=13") && line.ends_with("(+1 more)"),
            "{line}"
        );
        assert_eq!(line.matches(" -> ").count(), 12, "{line}");
    }

    /// 对端上面每一个嵌套根都算（与闭包「按祖先封闭」同口径）：SBFR ⊂ FRMW ⊂ STRU 三层都是根。
    #[test]
    fn every_nested_root_above_the_far_end_is_collected() {
        let owners: BTreeMap<(u32, u32), RefNo> = [
            ((DB, 100), r(100)), // STRU：owner 是自己 = 顶
            ((DB, 110), r(100)), // FRMW
            ((DB, 111), r(110)), // SBFR
            ((DB, 112), r(111)), // SCTN
            ((DB, 200), r(200)), // 别的顶
        ]
        .into_iter()
        .collect();
        let roots: BTreeSet<(u32, u32)> = [(DB, 100), (DB, 110), (DB, 111), (DB, 200)]
            .into_iter()
            .collect();
        assert_eq!(
            roots_on_owner_chain(&owners, &roots, r(112)),
            vec![r(111), r(110), r(100)]
        );
        assert_eq!(
            roots_on_owner_chain(&owners, &roots, r(111)),
            vec![r(111), r(110), r(100)]
        );
        // T 端已删（不在索引里）→ 空。
        assert!(roots_on_owner_chain(&owners, &roots, r(999)).is_empty());
    }

    /// 表就是 C1 给的九个 noun，一个不多；回指表两两对称。
    #[test]
    fn the_noun_table_is_exactly_the_c1_boundary_and_back_pointers_are_symmetric() {
        for noun in ["SCTN", "GENSEC", "STWALL", "WALL"] {
            assert_eq!(forward_attributes(noun), &["JOIS", "JOIE"], "{noun}");
        }
        for noun in ["PJOI", "SJOI", "SUBJ", "FIXING"] {
            assert_eq!(forward_attributes(noun), &["CREF"], "{noun}");
        }
        assert_eq!(forward_attributes("CMPF"), &["CFRA"]);
        assert_eq!(
            forward_attributes(" sctn "),
            &["JOIS", "JOIE"],
            "大小写与空白不敏感"
        );
        for noun in ["PANE", "BPANEL", "BRAN", "EQUI", "STRU", "FRMW", ""] {
            assert!(forward_attributes(noun).is_empty(), "{noun} 不持连接属性");
        }
        for forward in ["CREF", "JOIS", "JOIE", "CFRA", "PFRE"] {
            for &back in back_pointers(forward) {
                assert!(
                    back_pointers(back).contains(&forward),
                    "{forward} → {back} 应当能指回来"
                );
            }
        }
        assert!(back_pointers("HREF").is_empty());
    }

    #[test]
    fn the_summary_line_tells_the_switch_state_apart() {
        assert!(
            StructuralConnectionCascade::off()
                .summary_line(8000)
                .contains("cascade off")
        );
        let on = StructuralConnectionCascade::from_edges(&edge(Some(true), Some(true)), chain);
        let line = on.summary_line(8000);
        assert!(
            line.contains("cascade on")
                && line.contains("far_end_cross_db=0")
                && line.contains("far_end_roots=1")
                && !line.contains("cross-db far ends"),
            "{line}"
        );
    }

    // ---- 真库（手动）：C1 找到的两个「只改持属性一端」的窗口 + 九个钉死窗口两态逐字相同 ----

    #[cfg(test)]
    mod live {
        use std::path::{Path, PathBuf};

        use e3d_io::db_element::{DbFilePin, template_file_for};

        use super::*;
        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::{
            enumerate_generation_roots, resolve_delivery_unit_types,
        };
        use crate::data_interface::window_root_plan::{PersistedRoot, WindowRootSources};
        use crate::fast_model::e3d_model_service::scan_index;

        /// 语料缺失就跳过，存在就必须真跑。
        ///
        /// 这些用例读本机安装的 E3D 样例库（DESI），CI 上没有。以前靠 `#[ignore]`
        /// 挡住，结果是**任何机器上都永远不跑**——包括装了语料的开发机。改成
        /// 「文件不在 → 打一行 skip 后 return；文件在 → 继续，失败就是真失败」。
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

        /// 一窗：两态各算一次 regen（roots_S = base 端枚举，即「S 时刻每个根都已发布」）。
        fn regen_both_ways(
            schema: &DirectSchema,
            file: &Path,
            base_sesno: u32,
            target_sesno: u32,
        ) -> (
            (StructuralConnectionCascade, Vec<RefnoEnum>),
            (StructuralConnectionCascade, Vec<RefnoEnum>),
        ) {
            let unit_types = resolve_delivery_unit_types(&[]);
            let (base, target) = (
                open(schema, file, base_sesno),
                open(schema, file, target_sesno),
            );
            let base_index = scan_index(file, Some(base_sesno)).expect("scan base index");
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
                let _switch = crate::options::StructuralConnectionCascadeOverride::set(on);
                let sources = WindowRootSources::collect(
                    file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                )
                .expect("collect");
                assert!(
                    sources.closure.is_complete(),
                    "{:?}",
                    sources.closure.unreadable
                );
                let impact = sources.impact(&roots_s);
                let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                let regen: Vec<RefnoEnum> = plan.regen.iter().map(|root| root.root).collect();
                (sources.structural_connection, regen)
            };
            (run(true), run(false))
        }

        /// C1 §2.3 推荐的两个验收窗口：`aps7200_0001` 3→4（94 个 `SJOI` 单独改，`CREF` 指的
        /// `SCTN` 不在同一根下）、`aps250110_0001` 29→30（193 个 `FIXING` 单独改 → `GENSEC`）。
        /// 开关开：回指成立的边 ≥ 普查数、对端根 ∈ `regen`、只多出对端根、两窗都没有跨库对端；开关关：一条边都不读。
        ///
        /// 开关关着对端根进不进 `regen`（表的最后一列）：两窗现在都钉成「关着已全覆盖」——e3d-model 的同库关节级联
        /// 哪天掉了，这一窗先红。
        /// - FIXING → GENSEC（aps250110）：e3d-model `plan_update` 自己的同库关节级联（`increment::joint_consumer_of`，
        ///   `d510253`）把指着变更 FIXING 的 GENSEC 放进计划集、成了闭包种子（09-07 关 251 / 开 258，09-30 起两态都是 258）；
        /// - `SJOI` → SCTN（aps7200）：2026-10-23 起同一条级联推广到 SJOI ↔ SCTN（e3d-model `28d0402`），关 6 / 开 8 →
        ///   两态都是 8。这里的 `DbSet` 没挂目录库，读不出目录链的 SJOI 按目录件算（`category::is_catalogue_joint` 的
        ///   读挂口径）。
        ///
        /// 只读文件：`AIOS_APS_PROJECT_DIR`（默认 `D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000`）+
        /// `AIOS_E3D_TEMPLATE_DIR`。
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_aps_one_ended_windows_pull_the_far_end_roots_into_regen() {
            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            if !corpus_ready(
                "aps 的两个单端窗口",
                &[
                    dir.join("aps7200_0001").as_path(),
                    dir.join("aps250110_0001").as_path(),
                ],
            ) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            // (库, S, T, 持属性端 noun, 普查的回指成立边数, 开关关着对端根是否已全在 regen 里)
            let table = [
                ("aps7200_0001", 3u32, 4u32, "SJOI", 94usize, true),
                ("aps250110_0001", 29, 30, "FIXING", 193, true),
            ];
            for (name, base_sesno, target_sesno, noun, census_edges, covered_when_off) in table {
                let file = dir.join(name);
                let ((on, regen_on), (off, regen_off)) =
                    regen_both_ways(&schema, &file, base_sesno, target_sesno);
                println!(
                    "{name} {base_sesno}→{target_sesno}: on={} | regen_on={} regen_off={}",
                    on.summary_line(0),
                    regen_on.len(),
                    regen_off.len()
                );
                assert!(on.enabled && !off.enabled);
                assert!(
                    on.backref_ok >= census_edges,
                    "{name}: 普查数出 {census_edges} 条 {noun} 回指成立的边，这里只读到 {}",
                    on.backref_ok
                );
                assert!(!on.far_end_roots.is_empty(), "{name}: 对端根集不该为空");
                for root in &on.far_end_roots {
                    assert!(
                        regen_on.contains(root),
                        "{name}: 对端根 {} 该在 regen 里",
                        root.to_pdms_str()
                    );
                }
                assert!(
                    on.far_end_cross_db.is_empty(),
                    "{name}: 这一窗的边都在本库，跨库账该是空的：{:?}",
                    on.far_end_cross_db
                );
                assert!(off.far_end_roots.is_empty(), "{name}: 开关关不该读边");
                let off_set: BTreeSet<RefnoEnum> = regen_off.iter().copied().collect();
                assert!(
                    regen_on
                        .iter()
                        .all(|root| off_set.contains(root) || on.far_end_roots.contains(root)),
                    "{name}: 开关开只能多出对端根"
                );
                if covered_when_off {
                    let missing: Vec<String> = on
                        .far_end_roots
                        .iter()
                        .filter(|root| !off_set.contains(root))
                        .map(|root| root.to_pdms_str())
                        .collect();
                    assert!(
                        missing.is_empty(),
                        "{name}: e3d-model 的同库关节级联该让开关关着也盖住全部对端根，缺 {missing:?}"
                    );
                    let on_set: BTreeSet<RefnoEnum> = regen_on.iter().copied().collect();
                    assert_eq!(
                        on_set,
                        off_set,
                        "{name}: 对端根关着已全在，开关开不该再多出根（on={} off={}）",
                        regen_on.len(),
                        regen_off.len()
                    );
                } else {
                    assert!(
                        regen_on.len() > regen_off.len(),
                        "{name}: 这是「只改一端」的窗口，开关开必须多出至少一根（on={} off={}）",
                        regen_on.len(),
                        regen_off.len()
                    );
                }
            }
        }

        /// 探针（GitHub #33）：**对端落在另一个 DESI 库时，这条级联读得到吗、上卷得到吗？**
        ///
        /// 扫一个项目目录里全部 DESI 库的最新会话，找持连接属性的结构 noun 里 `get_ref` 指向
        /// **别的库号**的边（`RefNo::dbno()` 不等），对每条：
        /// - 「读到 / 读不到」：在一份**MDB 同款**的 `DbSet`（全部 DESI 都 `add_db`，与生产 `build_set` 同形）
        ///   上跑 `backref_holds`——`Some(true)` = 读到且回指成立；`None` = 对端所在库没注册 / 读挂；
        /// - 「生产口径的上卷」：`roots_on_owner_chain(源库的 owner 索引, 源库的 roots_T, 对端)`——这就是
        ///   `WindowRootSources::collect` 里传的那两样（都只有窗口那一个文件），对端在别的库时 `owners.get`
        ///   第一步就是 `None`，预期**空**；
        /// - 「拆账后的回执」（2026-09-30，#34 前置 1）：同一批边交给 [`StructuralConnectionCascade::from_edges`]，
        ///   会级联的全记进 `far_end_cross_db`、`far_end_without_root` 一条不占，本窗根链一次都不查；
        /// - 「对端自己库里的根」：同一函数换成**对端库**的 owner 索引与根集，看它本来该落到哪些根。
        ///
        /// 语料里一条跨库边都没有也是结论（打印出来，测试仍过）。有的话钉住今天的形状：生产口径一根都
        /// 上卷不到、而对端库自己能找到根——这就是 #33 要回答的「读得到、上卷不到」。只读文件：
        /// `AIOS_APS_PROJECT_DIR`（默认 `D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000`）+ `AIOS_E3D_TEMPLATE_DIR`。
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_probe_cross_db_structural_connection_edges() {
            use crate::data_interface::db_file_header::DbFileHeader;

            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            if !corpus_ready("aps000 的跨库结构连接边", &[dir.as_path()]) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);

            // 目录里的 DESI 库：按文件头认，不按名字猜。
            let mut desi: Vec<(u32, PathBuf, u32)> = std::fs::read_dir(&dir)
                .expect("read project dir")
                .filter_map(|entry| entry.ok().map(|e| e.path()))
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
            println!(
                "[probe #33] {} 个 DESI 库：{:?}",
                desi.len(),
                desi.iter().map(|d| d.0).collect::<Vec<_>>()
            );

            // MDB 同款的一份 DbSet：全部 DESI 钉最新（生产 build_set 只把窗口那个库钉在 T，其余钉当前 pin）。
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

            // 每个库：最新会话的索引（owner 表）+ 生产同口径的根集。
            struct Lib {
                owners: Arc<BTreeMap<(u32, u32), RefNo>>,
                roots: BTreeSet<(u32, u32)>,
                root_list: Vec<GenerationRoot>,
                elements: Vec<RefNo>,
            }
            let mut libs: BTreeMap<u32, Lib> = BTreeMap::new();
            for (dbnum, path, sesno) in &desi {
                let index = scan_index(path, Some(*sesno)).expect("scan index");
                let root_list = enumerate_generation_roots(&all, &index.roots, &unit_types)
                    .expect("enumerate roots");
                let roots = root_list
                    .iter()
                    .map(|root| {
                        let refno = refno_to_e3d(root.root);
                        (refno.word0, refno.word1)
                    })
                    .collect();
                // `SourceIndex::roots` 只是顶层元素；全库元素是 owner 表的键。
                let elements = index
                    .owners
                    .keys()
                    .map(|&(word0, word1)| RefNo::new(word0, word1))
                    .collect();
                libs.insert(
                    *dbnum,
                    Lib {
                        owners: index.owners.clone(),
                        roots,
                        root_list,
                        elements,
                    },
                );
            }

            // 正向读每个结构 noun 的连接属性，只留指向别的库的边。
            struct Edge {
                source: RefNo,
                noun: String,
                attribute: &'static str,
                target: RefNo,
            }
            let mut cross: Vec<Edge> = Vec::new();
            let mut same_db = 0usize;
            let mut sources_seen = 0usize;
            for (dbnum, lib) in &libs {
                for refno in &lib.elements {
                    let element = all.element(*refno);
                    let Ok(noun) = element.element_type() else {
                        continue;
                    };
                    let attributes = forward_attributes(&noun);
                    if attributes.is_empty() {
                        continue;
                    }
                    sources_seen += 1;
                    for &attribute in attributes {
                        if !element.has_attribute(attribute).unwrap_or(false) {
                            continue;
                        }
                        let Ok(Some(target)) = element.get_ref(attribute) else {
                            continue;
                        };
                        if !target.is_valid() {
                            continue;
                        }
                        if target.dbno() == *dbnum {
                            same_db += 1;
                        } else {
                            cross.push(Edge {
                                source: *refno,
                                noun: noun.clone(),
                                attribute,
                                target,
                            });
                        }
                    }
                }
            }
            let mut by_pair: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            for edge in &cross {
                *by_pair
                    .entry((edge.source.dbno(), edge.target.dbno()))
                    .or_default() += 1;
            }
            println!(
                "[probe #33] 结构 noun 元素 {sources_seen} 个；同库边 {same_db} 条；跨库边 {} 条，按 (源库 → 目标库)：{by_pair:?}",
                cross.len()
            );
            if cross.is_empty() {
                println!(
                    "[probe #33] 语料里没有跨库的结构连接边——「读得到 / 上卷不到」在这份语料上无从判别，结论只能从代码形状说"
                );
                return;
            }

            // 逐条：读到没有 / 生产口径上卷到什么 / 对端自己库里该是什么根。
            let mut readable = 0usize;
            let mut backref_ok = 0usize;
            let mut production_found = 0usize;
            let mut far_end_lib_found = 0usize;
            let mut target_lib_missing = 0usize;
            for (i, edge) in cross.iter().enumerate() {
                let backref = backref_holds(&all, edge.target, edge.attribute, edge.source);
                if backref.is_some() {
                    readable += 1;
                }
                if backref == Some(true) {
                    backref_ok += 1;
                }
                let src_lib = &libs[&edge.source.dbno()];
                let production = roots_on_owner_chain(&src_lib.owners, &src_lib.roots, edge.target);
                if !production.is_empty() {
                    production_found += 1;
                }
                let far_end = match libs.get(&edge.target.dbno()) {
                    Some(lib) => roots_on_owner_chain(&lib.owners, &lib.roots, edge.target),
                    None => {
                        target_lib_missing += 1;
                        Vec::new()
                    }
                };
                if !far_end.is_empty() {
                    far_end_lib_found += 1;
                }
                if i < 20 {
                    println!(
                        "[probe #33] {} {} .{} → {} | backref={backref:?} | 生产口径上卷={:?} | 对端库自己的根={:?}",
                        edge.noun,
                        edge.source,
                        edge.attribute,
                        edge.target,
                        production.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                        far_end.iter().map(|r| r.to_string()).collect::<Vec<_>>()
                    );
                }
            }
            println!(
                "[probe #33] 跨库边 {} 条：读到 {readable}（回指成立 {backref_ok}）；生产口径上卷到根 {production_found}；\
                 对端库自己能找到根 {far_end_lib_found}；目标库不在目录里 {target_lib_missing}",
                cross.len()
            );

            // 钉今天的形状：读得到（MDB 同款 DbSet 注册了对端库），生产口径一根都上卷不到。
            assert_eq!(
                readable + target_lib_missing,
                cross.len(),
                "对端库在 DbSet 里就该读得到——读不到的那些不是「库没注册」就是数据问题，逐条看上面的打印"
            );
            assert_eq!(
                production_found, 0,
                "生产口径（源库 owner 索引 + 源库 roots_T）对跨库对端本该一根都找不到；找到了说明形状变了，更新 #33 的结论"
            );

            // 拆账后的回执（#34 前置 1）：同一批跨库边按生产同一段 `from_edges` 记账。
            let edges = ConnectionEdges {
                edges: cross
                    .iter()
                    .map(|edge| ConnectionEdge {
                        side: WindowSide::Target,
                        source: edge.source,
                        source_noun: edge.noun.clone(),
                        attribute: edge.attribute,
                        target: edge.target,
                        backref: backref_holds(&all, edge.target, edge.attribute, edge.source),
                        under_stru: under_stru(&all.element(edge.source)),
                    })
                    .collect(),
                sources_read: cross.len(),
                unreadable_sources: 0,
            };
            let cascading = edges.edges.iter().filter(|edge| edge.cascades()).count();
            let mut consulted = 0usize;
            let accounted = StructuralConnectionCascade::from_edges(&edges, |_| {
                consulted += 1;
                Vec::new()
            });
            println!(
                "[probe #33] 拆账后（{cascading} 条会级联）：{}",
                accounted.summary_line(0)
            );
            assert_eq!(consulted, 0, "跨库对端不该拿本窗的根链去查");
            assert_eq!(accounted.far_end_cross_db.len(), cascading);
            assert_eq!(accounted.far_end_without_root, 0);
            assert!(accounted.far_end_roots.is_empty());

            // 跨库交接（#34 前置 2，2026-10-01）：同一批会级联的对端按库分组（计划上的 `cross_db_elements` 同一段），
            // 在对端库自己的索引与根集上用交接同一段折根——每一个都得折得出根，而且与上面逐条算的「对端库自己的根」
            // 逐字相同。
            use crate::data_interface::cross_db_handoff::{
                elements_by_dbnum, fold_elements_to_roots, parse_refno,
            };
            let handoff = elements_by_dbnum(
                accounted
                    .far_end_cross_db
                    .iter()
                    .map(|far_end| far_end.target),
            );
            let mut handed_roots = 0usize;
            for (dbnum, refnos) in &handoff {
                let Some(lib) = libs.get(dbnum) else {
                    println!(
                        "[probe #34] dbnum {dbnum} 不在目录里，交接会在 latest_source_version 那一步点名"
                    );
                    continue;
                };
                let elements: Vec<RefNo> =
                    refnos.iter().filter_map(|text| parse_refno(text)).collect();
                assert_eq!(elements.len(), refnos.len(), "名单拼得回 refno");
                let folded = fold_elements_to_roots(&lib.owners, &lib.root_list, &elements);
                println!(
                    "[probe #34] 交给 dbnum {dbnum}：{} 个对端 → {} 个根 {:?}；折不出 {:?}",
                    elements.len(),
                    folded.roots.len(),
                    folded
                        .roots
                        .iter()
                        .map(|root| format!("{} {}", root.refno, root.noun))
                        .collect::<Vec<_>>(),
                    folded.unresolved
                );
                assert!(
                    folded.unresolved.is_empty(),
                    "对端库自己能找到根（#33 量过 12/12），交接也必须折得出：{:?}",
                    folded.unresolved
                );
                let expected: BTreeSet<String> = elements
                    .iter()
                    .flat_map(|element| roots_on_owner_chain(&lib.owners, &lib.roots, *element))
                    .map(|root| refno_from_e3d(root).to_pdms_str())
                    .collect();
                let got: BTreeSet<String> =
                    folded.roots.iter().map(|root| root.refno.clone()).collect();
                assert_eq!(got, expected, "交接折根与同库对端同一口径");
                handed_roots += folded.roots.len();
            }
            println!(
                "[probe #34] 跨库交接：{} 个对端库、{handed_roots} 个根",
                handoff.len()
            );
            assert!(
                cascading == 0 || handed_roots > 0,
                "有会级联的跨库边就该交得出根"
            );
        }

        /// 九个钉死窗口（`window_root_plan::tests::live_ams8000_pinned_windows_land_in_the_expected_buckets`
        /// 的表）开关两态 `regen` 逐字相同——它们没有跨根的结构连接改动。只读文件：
        /// `AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
        ///
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off() {
            let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
                |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
            ));
            if !corpus_ready("ams8000 的九个钉死窗口（结构连接）", &[file.as_path()]) {
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
            for (base_sesno, target_sesno) in windows {
                let ((on, regen_on), (_, regen_off)) =
                    regen_both_ways(&schema, &file, base_sesno, target_sesno);
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

        // ---- GitHub #34 复核（2026-11-11）：关节 ↔ 梁两个方向 ----
        //
        // e3d-model 这两个月把关节负体接上了：GENSEC 扣 FIXING 的（09-28），SCTN 扣 SJOI / PJOI 的（10-15 起；
        // 11-10 `036e89d` 主关节 PJOI 的朝向照 Core3D 现算，三本库 48 根杆的切口跟着变）。「关节变了，被它切的梁
        // 要不要重算」因此是正确性问题，两个方向都要有真库的证据。自然窗口有的用自然窗口；语料里没有「只改 PJOI」
        // 的一步，那一类用假定源——与 C8 `live_zdj7200_…`、P4 `live_every_posref_table_…` 同一手法。

        const APS_PROJECT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000";
        const AMS_PROJECT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";

        fn project_dir(env: &str, default: &str) -> PathBuf {
            PathBuf::from(std::env::var(env).unwrap_or_else(|_| default.into()))
        }

        /// 一个库在某个会话上的静态现场：`DbSet`、属主索引、生产同口径的根集。
        struct Site {
            set: Arc<DbSet>,
            owners: Arc<BTreeMap<(u32, u32), RefNo>>,
            roots_t: Vec<GenerationRoot>,
            root_keys: BTreeSet<(u32, u32)>,
        }

        fn site(schema: &DirectSchema, file: &Path, sesno: u32) -> Site {
            let set = open(schema, file, sesno);
            let index = scan_index(file, Some(sesno)).expect("scan index");
            let roots_t =
                enumerate_generation_roots(&set, &index.roots, &resolve_delivery_unit_types(&[]))
                    .expect("enumerate roots");
            let root_keys = roots_t
                .iter()
                .map(|root| {
                    let refno = refno_to_e3d(root.root);
                    (refno.word0, refno.word1)
                })
                .collect();
            Site {
                set,
                owners: index.owners.clone(),
                roots_t,
                root_keys,
            }
        }

        /// `element` 属主链上的根，自下而上（第一个就是最近的根）。
        fn root_chain(site: &Site, element: RefNo) -> Vec<RefnoEnum> {
            roots_on_owner_chain(&site.owners, &site.root_keys, element)
                .into_iter()
                .map(refno_from_e3d)
                .collect()
        }

        /// 库里全部会级联的结构连接边（回指成立、过 `STRU` 门），用生产同一段 `read_edges_of` 读。
        /// 对端在别的库的边也留着（这份 `DbSet` 只挂了一个库，回指读不到，调用方单独点名）。
        fn cascading_edges(site: &Site) -> Vec<ConnectionEdge> {
            let mut edges = Vec::new();
            for &(word0, word1) in site.owners.keys() {
                // 读挂的元素不是这条门要量的（生产把它记进 `unreadable_sources`）。
                let _ = read_edges_of(
                    &site.set,
                    RefNo::new(word0, word1),
                    WindowSide::Target,
                    &mut edges,
                );
            }
            edges.retain(|edge| edge.cascades() || edge.target.dbno() != edge.source.dbno());
            edges
        }

        /// 假定 `source` 这一件在一窗里改了（账本里只有它一条），按生产同一段折出两态各自的 `regen`。
        ///
        /// 闭包直接命中的根 = 它属主链上的全部根（祖先闭包对单个种子就是它的祖先）；开关开着再并上 C2 的对端根——
        /// 与 `WindowRootSources::collect` 同一个开关分支、与 `impact` 同一次 `expand_touched`。`touched` 按构造全在
        /// `roots_T` 里，所以它就是 `regen = touched ∩ roots_T`。返回 `(开着的账, regen 开, regen 关)`。
        fn regen_if_changed(
            site: &Site,
            source: RefNo,
            noun: &str,
        ) -> (
            StructuralConnectionCascade,
            BTreeSet<RefnoEnum>,
            BTreeSet<RefnoEnum>,
        ) {
            let mut ledger = e3d_model::ledger::ChangeLedger::default();
            ledger.record(
                source,
                &e3d_model::element_diff::ElementDiff {
                    refno: source.to_string(),
                    noun: noun.into(),
                    type_changed: None,
                    owner_changed: None,
                    members: None,
                    attributes: vec!["POS".into()],
                    opaque: false,
                },
            );
            ledger.dispose(source, e3d_model::ledger::Disposition::RolledUp);
            let plan = UpdatePlan {
                regenerate: BTreeSet::new(),
                remove: BTreeSet::new(),
                regenerate_derived: BTreeSet::new(),
                derived_stale: BTreeSet::new(),
                ledger,
                full_rebuild: None,
                report: e3d_model::increment::IncrementReport::default(),
            };
            let direct: BTreeSet<RefnoEnum> = root_chain(site, source).into_iter().collect();
            let run = |on: bool| {
                let _switch = crate::options::StructuralConnectionCascadeOverride::set(on);
                let cascade = if crate::options::model_structural_connection_cascade() {
                    StructuralConnectionCascade::collect(
                        &plan,
                        &site.set,
                        &site.set,
                        &site.owners,
                        &site.roots_t,
                    )
                } else {
                    StructuralConnectionCascade::off()
                };
                let mut touched = direct.clone();
                cascade.expand_touched(&mut touched);
                (cascade, touched)
            };
            let (on, regen_on) = run(true);
            let (off, regen_off) = run(false);
            assert!(on.enabled && !off.enabled);
            (on, regen_on, regen_off)
        }

        /// 一类边（`持属性端.属性->对端`）在一个库里的账。
        #[derive(Debug, Default, Clone, PartialEq, Eq)]
        struct EdgeAccount {
            /// 会级联的同库边数。
            edges: usize,
            /// 开关关着时，对端**最近的根**不在 `regen` 里的边数——嵌套根各管各的几何，这些对端不会重算。
            uncovered_when_off: usize,
            /// 其中连对端属主链上任何一个根都不在 `regen` 里的（外层根整棵重算也罩不住）。
            disjoint_when_off: usize,
            /// 下面四格只打印、不进表（量「开关开着一次改动多重算多少」）：逐边多出来的根数之和 / 单条边最多；
            /// 关着时 `regen` 的根名下的元素数之和 / 多出来的根名下的元素数之和（元素按最近的根归属）。
            added_roots: usize,
            added_roots_max: usize,
            elements_off: usize,
            elements_added: usize,
        }

        /// 假定源，两个方向一起走：库里每一条会级联的同库结构连接边，假定持属性的那一端单独改了。
        /// 持属性端是关节（`PJOI` / `SJOI` / `SUBJ` / `FIXING` 的 `CREF`）就是「关节变了 → 被它切的梁」，
        /// 是梁（`SCTN` / `GENSEC` / `STWALL` / `WALL` 的 `JOIS` / `JOIE`）就是「梁变了 → 它的关节」。
        ///
        /// 契约（每条边都断言）：开关开，对端属主链上的**每一个**根都在 `regen` 里；开关关，`regen` 恰好是变更元素
        /// 自己链上的根；开着只多出对端根。
        ///
        /// 账（打印，再与下面的表逐格相等）：开关关着时对端最近的根不在 `regen` 里的边数——这些边只有本开关接得住
        /// （e3d-model `plan_update` 自己的同库关节级联只管 FIXING ↔ GENSEC 与带目录正体的 SJOI ↔ SCTN，而且它在
        /// 单元一层，这里量的是根这一层）。四本库各代表一种形状：
        /// - `aps7200_0001`：钢结构，关节与梁常在不同的根下——`PJOI` / `SJOI` 两族都有只靠开关的边；
        /// - `aps7324_0001`：MDS 支架（`/MDS/FT54…` 那 48 根杆所在的库之一）——关节与被切的杆同在一个 FRMW 根下，
        ///   关着也整根重算；
        /// - `ams251183_0001`：正式后端所在工程（AMS）里跨根边最多的库；
        /// - `ams8000_0001`：正式后端监听的库，1244 条边全部同根。
        ///
        /// 会话钉死（2026-11-11 各库的最新会话）：这些库还在被人用，钉住了以后多出来的会话不动这张表。
        ///
        /// 只读文件：`AIOS_APS_PROJECT_DIR` / `AIOS_AMS_PROJECT_DIR`（缺省本机样例工程）+ `AIOS_E3D_TEMPLATE_DIR`。
        /// 语料不在本机时跳过（见 [`corpus_ready`]）。
        #[test]
        fn live_a_changed_joint_pulls_its_beam_into_regen_and_a_changed_beam_pulls_its_joints() {
            let aps = project_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
            let ams = project_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
            // (库文件, 钉住的会话)
            let files = [
                (aps.join("aps7200_0001"), 11u32),
                (aps.join("aps7324_0001"), 1366),
                (aps.join("aps7350_0001"), 211),
                (ams.join("ams251183_0001"), 15),
                (ams.join("ams8000_0001"), 1143),
            ];
            let paths: Vec<&Path> = files.iter().map(|(file, _)| file.as_path()).collect();
            if !corpus_ready("关节 ↔ 梁的假定源", &paths) {
                return;
            }
            // (库, 边的类别, 会级联的同库边数, 开关关着对端最近的根不在 regen 里的, 其中根链无交集的)
            // 2026-11-11 本机实测；第四列与同日 `cross_root_reference_census` 的「回指成立 ∧ 正向最近根不覆盖 ∧
            // 在 STRU 下」逐格相同。
            let expected: &[(&str, &str, usize, usize, usize)] = &[
                ("aps7200_0001", "PJOI.CREF->SCTN", 22, 20, 0),
                ("aps7200_0001", "PJOI.CREF->STWALL", 18, 0, 0),
                ("aps7200_0001", "PJOI.CREF->WALL", 2, 0, 0),
                ("aps7200_0001", "SCTN.JOIE->PJOI", 1, 0, 0),
                ("aps7200_0001", "SCTN.JOIE->SJOI", 104, 64, 8),
                ("aps7200_0001", "SCTN.JOIS->PJOI", 21, 18, 0),
                ("aps7200_0001", "SCTN.JOIS->SJOI", 96, 66, 10),
                ("aps7200_0001", "SJOI.CREF->SCTN", 200, 130, 18),
                ("aps7200_0001", "STWALL.JOIE->PJOI", 9, 0, 0),
                ("aps7200_0001", "STWALL.JOIS->PJOI", 9, 0, 0),
                ("aps7200_0001", "WALL.JOIE->PJOI", 1, 0, 0),
                ("aps7200_0001", "WALL.JOIS->PJOI", 1, 0, 0),
                ("aps7324_0001", "FIXING.CREF->GENSEC", 322, 0, 0),
                ("aps7324_0001", "GENSEC.JOIE->FIXING", 181, 0, 0),
                ("aps7324_0001", "GENSEC.JOIS->FIXING", 141, 0, 0),
                ("aps7324_0001", "PJOI.CREF->SCTN", 60, 0, 0),
                ("aps7324_0001", "SCTN.JOIE->PJOI", 60, 0, 0),
                ("aps7324_0001", "SCTN.JOIE->SJOI", 91, 0, 0),
                ("aps7324_0001", "SCTN.JOIS->SJOI", 144, 0, 0),
                ("aps7324_0001", "SJOI.CREF->SCTN", 235, 0, 0),
                ("aps7350_0001", "FIXING.CREF->GENSEC", 326, 0, 0),
                ("aps7350_0001", "GENSEC.JOIE->FIXING", 181, 0, 0),
                ("aps7350_0001", "GENSEC.JOIS->FIXING", 145, 0, 0),
                ("aps7350_0001", "PJOI.CREF->SCTN", 60, 0, 0),
                ("aps7350_0001", "SCTN.JOIE->PJOI", 60, 0, 0),
                ("aps7350_0001", "SCTN.JOIE->SJOI", 91, 0, 0),
                ("aps7350_0001", "SCTN.JOIS->SJOI", 142, 0, 0),
                ("aps7350_0001", "SJOI.CREF->SCTN", 233, 0, 0),
                ("ams251183_0001", "PJOI.CREF->SCTN", 449, 4, 0),
                ("ams251183_0001", "SCTN.JOIE->PJOI", 349, 135, 0),
                ("ams251183_0001", "SCTN.JOIE->SJOI", 2, 0, 0),
                ("ams251183_0001", "SCTN.JOIS->PJOI", 100, 75, 0),
                ("ams251183_0001", "SCTN.JOIS->SJOI", 4, 0, 0),
                ("ams251183_0001", "SJOI.CREF->SCTN", 6, 0, 0),
                ("ams8000_0001", "PJOI.CREF->SCTN", 248, 0, 0),
                ("ams8000_0001", "SCTN.JOIE->PJOI", 124, 0, 0),
                ("ams8000_0001", "SCTN.JOIS->PJOI", 124, 0, 0),
                ("ams8000_0001", "SCTN.JOIS->SUBJ", 374, 0, 0),
                ("ams8000_0001", "SUBJ.CREF->SCTN", 374, 0, 0),
            ];
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let mut measured: Vec<(String, String, EdgeAccount)> = Vec::new();
            for (file, sesno) in &files {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let site = site(&schema, file, *sesno);
                let edges = cascading_edges(&site);
                // 每个根名下的元素数（按最近的根归属，嵌套根不重复计）：量多出来的根有多大。
                let mut population: BTreeMap<RefnoEnum, usize> = BTreeMap::new();
                for &(word0, word1) in site.owners.keys() {
                    if let Some(nearest) = root_chain(&site, RefNo::new(word0, word1))
                        .into_iter()
                        .next()
                    {
                        *population.entry(nearest).or_default() += 1;
                    }
                }
                let weigh = |roots: &BTreeSet<RefnoEnum>| -> usize {
                    roots
                        .iter()
                        .map(|root| population.get(root).copied().unwrap_or(0))
                        .sum()
                };
                let mut accounts: BTreeMap<String, EdgeAccount> = BTreeMap::new();
                let mut cross_db = 0usize;
                for edge in &edges {
                    if edge.target.dbno() != edge.source.dbno() {
                        cross_db += 1;
                        println!(
                            "[#34]   跨库：{} {}.{} -> {}（dbnum {}）",
                            edge.source_noun,
                            edge.source,
                            edge.attribute,
                            edge.target,
                            edge.target.dbno()
                        );
                        continue;
                    }
                    let far_noun = site
                        .set
                        .element(edge.target)
                        .element_type()
                        .unwrap_or_else(|_| "?".into());
                    let label = format!("{}.{}->{far_noun}", edge.source_noun, edge.attribute);
                    let (on, regen_on, regen_off) =
                        regen_if_changed(&site, edge.source, &edge.source_noun);
                    let own: BTreeSet<RefnoEnum> =
                        root_chain(&site, edge.source).into_iter().collect();
                    let far = root_chain(&site, edge.target);
                    assert_eq!(
                        regen_off, own,
                        "{name} {label} {}: 开关关着 regen 只该有变更元素自己链上的根",
                        edge.source
                    );
                    assert!(
                        on.backref_ok >= 1,
                        "{name} {label} {}: 开着该读到这条边",
                        edge.source
                    );
                    for root in &far {
                        assert!(
                            regen_on.contains(root),
                            "{name} {label} {} -> {}: 对端链上的根 {} 该在 regen 里",
                            edge.source,
                            edge.target,
                            root.to_pdms_str()
                        );
                    }
                    assert!(
                        regen_on
                            .iter()
                            .all(|root| regen_off.contains(root) || on.far_end_roots.contains(root)),
                        "{name} {label} {}: 开关开只能多出对端根",
                        edge.source
                    );
                    let uncovered = far
                        .first()
                        .is_some_and(|nearest| !regen_off.contains(nearest));
                    let disjoint =
                        !far.is_empty() && far.iter().all(|root| !regen_off.contains(root));
                    let added: BTreeSet<RefnoEnum> =
                        regen_on.difference(&regen_off).copied().collect();
                    let account = accounts.entry(label.clone()).or_default();
                    account.edges += 1;
                    account.uncovered_when_off += usize::from(uncovered);
                    account.disjoint_when_off += usize::from(disjoint);
                    account.added_roots += added.len();
                    account.added_roots_max = account.added_roots_max.max(added.len());
                    account.elements_off += weigh(&regen_off);
                    account.elements_added += weigh(&added);
                    // 每一类各点名头两条只有开关接得住的边，真机验证时好挑目标。
                    if (uncovered && account.uncovered_when_off <= 2)
                        || (disjoint && account.disjoint_when_off <= 2)
                    {
                        println!(
                            "[#34]   例：{name} {label} {} -> {}{}：变更端的根 {:?}，对端的根 {:?}",
                            edge.source,
                            edge.target,
                            if disjoint {
                                "（根链无交集）"
                            } else {
                                ""
                            },
                            own.iter()
                                .map(|root| root.to_pdms_str())
                                .collect::<Vec<_>>(),
                            far.iter()
                                .map(|root| root.to_pdms_str())
                                .collect::<Vec<_>>()
                        );
                    }
                }
                println!(
                    "[#34] {name}@{sesno}: 元素 {} 根 {} 会级联的边 {}（另有跨库 {cross_db} 条不在这条门里）",
                    site.owners.len(),
                    site.roots_t.len(),
                    edges.len() - cross_db
                );
                for (label, account) in &accounts {
                    println!(
                        "[#34]   (\"{name}\", \"{label}\", {}, {}, {}),",
                        account.edges, account.uncovered_when_off, account.disjoint_when_off
                    );
                    println!(
                        "[#34]   量：{name} {label}: 逐边假定单独改，开着多出的根合计 {}（单条边最多 {}）；\
                         名下元素 关着 {} / 多出 {}",
                        account.added_roots,
                        account.added_roots_max,
                        account.elements_off,
                        account.elements_added
                    );
                    measured.push((name.clone(), label.clone(), account.clone()));
                }
            }
            let measured: Vec<(&str, &str, usize, usize, usize)> = measured
                .iter()
                .map(|(name, label, account)| {
                    (
                        name.as_str(),
                        label.as_str(),
                        account.edges,
                        account.uncovered_when_off,
                        account.disjoint_when_off,
                    )
                })
                .collect();
            assert_eq!(
                measured, expected,
                "关节 ↔ 梁的账变了：库文件多了会话，或根枚举 / 结构表的口径动了——核一遍再改表"
            );
        }

        /// 这一窗（开关关着）的根规划是否退化。`Some(原因)` = 闭包不可信、全部持久根重排——那样的窗两态必然相同，
        /// 证明不了「关着也盖得住对端」。
        fn window_degraded(
            schema: &DirectSchema,
            file: &Path,
            base_sesno: u32,
            target_sesno: u32,
        ) -> Option<String> {
            let unit_types = resolve_delivery_unit_types(&[]);
            let (base, target) = (
                open(schema, file, base_sesno),
                open(schema, file, target_sesno),
            );
            let base_index = scan_index(file, Some(base_sesno)).expect("scan base index");
            let roots_s: Vec<PersistedRoot> =
                enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                    .expect("enumerate base roots")
                    .into_iter()
                    .map(|root| PersistedRoot {
                        refno: root.root,
                        noun: root.noun,
                    })
                    .collect();
            let _switch = crate::options::StructuralConnectionCascadeOverride::set(false);
            let sources = WindowRootSources::collect(
                file,
                base_sesno,
                target_sesno,
                &base,
                &target,
                &unit_types,
                &[],
            )
            .expect("collect");
            sources.impact(&roots_s).degraded_reason()
        }

        /// 自然窗口，反方向：**梁**那一端单独改了，关节的根要进 `regen`。
        ///
        /// 语料（AMS + APS 两个工程，2026-11-11 的 `cross_root_reference_census`）里这种形状、又没有退化的窗只有
        /// `aps250209_0001` 15→16 与 16→17：`GENSEC.JOIS` / `JOIE` → FIXING，跨根单端边 86 / 35 条。另一个方向
        /// （FIXING 单独改）没退化的窗就是 [`live_aps_one_ended_windows_pull_the_far_end_roots_into_regen`] 里的
        /// `aps250110_0001` 29→30；`aps250108_0001` 15→16、`aps250112_0001` 18→19 也是 FIXING 单独改，但整窗退化
        /// （候选数超过索引键的 30%，全部持久根重排），两态必然相同，证明不了什么，不收。
        ///
        /// 契约与那一条相同：开关开，回指成立的边不少于普查数、对端根全在 `regen`、只多出对端根、没有跨库对端；
        /// 开关关，一条边都不读。这两窗开关关着对端根也已经全在 `regen` 里（两态集合相等）——要如实读：它们各自重排
        /// 373 根，而库里一共 382 根，几乎是全库；梁改了、关节没改的小窗口语料里没有，逐边的证据在假定源那一条
        /// （[`live_a_changed_joint_pulls_its_beam_into_regen_and_a_changed_beam_pulls_its_joints`]）。
        #[test]
        fn live_aps_windows_where_only_the_beam_changed_pull_the_joint_roots_into_regen() {
            let dir = project_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
            let file = dir.join("aps250209_0001");
            if !corpus_ready("aps250209 的梁端单独改动窗口", &[file.as_path()]) {
                return;
            }
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            // (S, T, 普查的「只改 GENSEC 一端」的跨根边数)
            let table = [(15u32, 16u32, 86usize), (16, 17, 35)];
            for (base_sesno, target_sesno, census_edges) in table {
                let label = format!("aps250209_0001 {base_sesno}→{target_sesno}");
                assert_eq!(
                    window_degraded(&schema, &file, base_sesno, target_sesno),
                    None,
                    "{label}: 这一窗不该退化——退化的窗两态必然相同，放在这里就成了空转"
                );
                let ((on, regen_on), (off, regen_off)) =
                    regen_both_ways(&schema, &file, base_sesno, target_sesno);
                let on_set: BTreeSet<RefnoEnum> = regen_on.iter().copied().collect();
                let off_set: BTreeSet<RefnoEnum> = regen_off.iter().copied().collect();
                println!(
                    "[#34] {label}（单独改 GENSEC）: {} | regen_on={} regen_off={}",
                    on.summary_line(0),
                    regen_on.len(),
                    regen_off.len()
                );
                assert!(on.enabled && !off.enabled);
                assert!(
                    on.backref_ok >= census_edges,
                    "{label}: 普查数出 {census_edges} 条只改 GENSEC 一端的跨根边，这里只读到 {}",
                    on.backref_ok
                );
                assert!(!on.far_end_roots.is_empty(), "{label}: 对端根集不该为空");
                for root in &on.far_end_roots {
                    assert!(
                        on_set.contains(root),
                        "{label}: 关节的根 {} 该在 regen 里",
                        root.to_pdms_str()
                    );
                }
                assert!(
                    on.far_end_cross_db.is_empty(),
                    "{label}: 这一窗的边都在本库：{:?}",
                    on.far_end_cross_db
                );
                assert!(off.far_end_roots.is_empty(), "{label}: 开关关不该读边");
                assert_eq!(
                    on_set, off_set,
                    "{label}: 开关关着关节的根也已全在 regen 里，两态集合该相等"
                );
            }
        }
        /// 量法（手动）：开关打开之后一次增量的重算范围放大多少。
        ///
        /// 逐库走会话链的最后 `AIOS_C2_MEASURE_STEPS_PER_DB` 步（缺省 12，0 = 整条链），每一步两态各跑一遍生产同款的
        /// `WindowRootSources::collect` → `impact` → `plan_window_roots`，记：`regen` 根数两态、多出来的根、这些根名下
        /// 的元素数（按最近的根归属，嵌套根不重复计）、读了多少条边、规划耗时两态。库的名单缺省是正式后端监听的五个库
        /// 加两个工程里结构连接边最多的那些；`AIOS_C2_MEASURE_FILES`（`;` 分隔的文件路径）可整体替换。
        ///
        /// 只读文件、不碰库、不下数值断言（放大多少是语料的性质）；只断言 `regen_off ⊆ regen_on` 与「多出来的全是
        /// 对端根」。结果打印到 stdout（`--nocapture`）。
        #[test]
        #[ignore = "manual live measurement: walks real DESI session chains twice; needs the E3D sample projects and template directory"]
        fn live_measure_how_much_the_cascade_widens_regen_on_real_windows() {
            use std::time::Instant;

            use e3d_io::ReadOnlyEngine;
            use e3d_io::session::SessionSelector;

            let aps = project_dir("AIOS_APS_PROJECT_DIR", APS_PROJECT_DIR);
            let ams = project_dir("AIOS_AMS_PROJECT_DIR", AMS_PROJECT_DIR);
            let files: Vec<PathBuf> = match std::env::var("AIOS_C2_MEASURE_FILES") {
                Ok(value) => value.split(';').map(PathBuf::from).collect(),
                Err(_) => [
                    // 正式后端监听的五个库。
                    "ams7997_0001",
                    "ams7998_0001",
                    "ams8000_0001",
                    "ams251095_0001",
                    "ams251137_0001",
                    // AMS 里结构连接边多的库。
                    "ams7999_0001",
                    "ams7324_0001",
                    "ams251183_0001",
                    "ams251181_0001",
                    "ams251180_0001",
                    "ams251173_0001",
                    "ams251172_0001",
                    "ams251182_0001",
                ]
                .iter()
                .map(|name| ams.join(name))
                .chain(
                    [
                        "aps7200_0001",
                        "aps7166_0001",
                        "aps7324_0001",
                        "aps250110_0001",
                        "aps250108_0001",
                        "aps250112_0001",
                        "aps250209_0001",
                    ]
                    .iter()
                    .map(|name| aps.join(name)),
                )
                .collect(),
            };
            let per_db: usize = std::env::var("AIOS_C2_MEASURE_STEPS_PER_DB")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(12);
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let unit_types = resolve_delivery_unit_types(&[]);

            #[derive(Default)]
            struct Totals {
                windows: usize,
                widened: usize,
                regen_off: usize,
                regen_on: usize,
                elements_off: usize,
                elements_added: usize,
                edges: usize,
                cross_db: usize,
                millis_off: u128,
                millis_on: u128,
            }
            let mut grand = Totals::default();
            for file in &files {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !file.is_file() {
                    println!("[#34 量] skip {name}: 文件不在");
                    continue;
                }
                let steps: Vec<(u32, u32)> = {
                    let Ok(mut engine) = ReadOnlyEngine::open(file) else {
                        println!("[#34 量] skip {name}: 打不开");
                        continue;
                    };
                    let mut sesnos: Vec<u32> = match engine.sessions() {
                        Ok(sessions) => sessions.iter().map(|session| session.session_id).collect(),
                        Err(error) => {
                            println!("[#34 量] skip {name}: 会话链读不出：{error}");
                            continue;
                        }
                    };
                    sesnos.sort_unstable();
                    sesnos.dedup();
                    if sesnos.len() < 2 {
                        println!("[#34 量] skip {name}: 只有 {} 个会话", sesnos.len());
                        continue;
                    }
                    match engine
                        .diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)
                    {
                        Ok(steps) => {
                            let all: Vec<(u32, u32)> = steps
                                .iter()
                                .map(|step| (step.from.sesno, step.to.sesno))
                                .collect();
                            let skip = if per_db == 0 {
                                0
                            } else {
                                all.len().saturating_sub(per_db)
                            };
                            all.into_iter().skip(skip).collect()
                        }
                        Err(error) => {
                            println!("[#34 量] skip {name}: diff_steps 失败：{error}");
                            continue;
                        }
                    }
                };
                let mut totals = Totals::default();
                for (base_sesno, target_sesno) in steps {
                    let (base, target) = (
                        open(&schema, file, base_sesno),
                        open(&schema, file, target_sesno),
                    );
                    let roots_s: Vec<PersistedRoot> = match scan_index(file, Some(base_sesno))
                        .and_then(|index| {
                            enumerate_generation_roots(&base, &index.roots, &unit_types)
                        }) {
                        Ok(roots) => roots
                            .into_iter()
                            .map(|root| PersistedRoot {
                                refno: root.root,
                                noun: root.noun,
                            })
                            .collect(),
                        Err(error) => {
                            println!(
                                "[#34 量] skip {name} {base_sesno}→{target_sesno}: S 端根枚举失败：{error:#}"
                            );
                            continue;
                        }
                    };
                    let run = |on: bool| {
                        let _switch = crate::options::StructuralConnectionCascadeOverride::set(on);
                        let started = Instant::now();
                        let sources = WindowRootSources::collect(
                            file,
                            base_sesno,
                            target_sesno,
                            &base,
                            &target,
                            &unit_types,
                            &[],
                        )?;
                        let impact = sources.impact(&roots_s);
                        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                        let millis = started.elapsed().as_millis();
                        let regen: BTreeSet<RefnoEnum> =
                            plan.regen.iter().map(|root| root.root).collect();
                        anyhow::Ok((sources, regen, plan.degraded, millis))
                    };
                    let (on, off) = match (run(true), run(false)) {
                        (Ok(on), Ok(off)) => (on, off),
                        (Err(error), _) | (_, Err(error)) => {
                            println!(
                                "[#34 量] skip {name} {base_sesno}→{target_sesno}: 规划失败：{error:#}"
                            );
                            continue;
                        }
                    };
                    let (sources_on, regen_on, degraded_on, millis_on) = on;
                    let (_, regen_off, _, millis_off) = off;
                    let cascade = &sources_on.structural_connection;
                    assert!(
                        regen_off.is_subset(&regen_on),
                        "{name} {base_sesno}→{target_sesno}: 级联只该往 touched 里加"
                    );
                    let added: Vec<RefnoEnum> = regen_on.difference(&regen_off).copied().collect();
                    assert!(
                        added
                            .iter()
                            .all(|root| cascade.far_end_roots.contains(root)),
                        "{name} {base_sesno}→{target_sesno}: 多出来的根必须都是对端根"
                    );
                    totals.windows += 1;
                    totals.regen_off += regen_off.len();
                    totals.regen_on += regen_on.len();
                    totals.edges += cascade.edges;
                    totals.cross_db += cascade.far_end_cross_db.len();
                    totals.millis_off += millis_off;
                    totals.millis_on += millis_on;
                    if added.is_empty() && cascade.edges == 0 {
                        continue;
                    }
                    // 元素数按最近的根归属（嵌套根不重复计）；只在读到边的窗上算，免得每窗多扫一遍索引。
                    let (elements_off, elements_added) = match scan_index(file, Some(target_sesno))
                    {
                        Ok(index) => {
                            let root_keys: BTreeSet<(u32, u32)> = sources_on
                                .roots_t
                                .iter()
                                .map(|root| {
                                    let refno = refno_to_e3d(root.root);
                                    (refno.word0, refno.word1)
                                })
                                .collect();
                            let (mut under_off, mut under_added) = (0usize, 0usize);
                            for &(word0, word1) in index.owners.keys() {
                                let nearest = roots_on_owner_chain(
                                    &index.owners,
                                    &root_keys,
                                    RefNo::new(word0, word1),
                                )
                                .into_iter()
                                .next()
                                .map(refno_from_e3d);
                                match nearest {
                                    Some(root) if regen_off.contains(&root) => under_off += 1,
                                    Some(root) if regen_on.contains(&root) => under_added += 1,
                                    _ => {}
                                }
                            }
                            (under_off, under_added)
                        }
                        Err(_) => (0, 0),
                    };
                    totals.elements_off += elements_off;
                    totals.elements_added += elements_added;
                    if !added.is_empty() {
                        totals.widened += 1;
                    }
                    println!(
                        "[#34 量] {name} {base_sesno}→{target_sesno}: regen 关 {} / 开 {}（+{}）；名下元素 关 {elements_off} / \
                         多 {elements_added}；边 {} 回指成立 {} 对端根 {} 跨库 {}；规划 关 {millis_off}ms / 开 {millis_on}ms{}",
                        regen_off.len(),
                        regen_on.len(),
                        added.len(),
                        cascade.edges,
                        cascade.backref_ok,
                        cascade.far_end_roots.len(),
                        cascade.far_end_cross_db.len(),
                        degraded_on
                            .map(|reason| format!("；这一窗退化：{reason}"))
                            .unwrap_or_default()
                    );
                }
                println!(
                    "[#34 量] == {name}: {} 窗，其中 {} 窗多出根；regen 合计 关 {} / 开 {}；读到边的窗上名下元素 关 {} / 多 {}；\
                     边 {} 跨库 {}；规划合计 关 {}ms / 开 {}ms",
                    totals.windows,
                    totals.widened,
                    totals.regen_off,
                    totals.regen_on,
                    totals.elements_off,
                    totals.elements_added,
                    totals.edges,
                    totals.cross_db,
                    totals.millis_off,
                    totals.millis_on
                );
                grand.windows += totals.windows;
                grand.widened += totals.widened;
                grand.regen_off += totals.regen_off;
                grand.regen_on += totals.regen_on;
                grand.elements_off += totals.elements_off;
                grand.elements_added += totals.elements_added;
                grand.edges += totals.edges;
                grand.cross_db += totals.cross_db;
                grand.millis_off += totals.millis_off;
                grand.millis_on += totals.millis_on;
            }
            println!(
                "[#34 量] ==== 合计 {} 窗，其中 {} 窗多出根；regen 关 {} / 开 {}（{:+.2}%）；读到边的窗上名下元素 关 {} / 多 {}；\
                 边 {} 跨库 {}；规划 关 {}ms / 开 {}ms",
                grand.windows,
                grand.widened,
                grand.regen_off,
                grand.regen_on,
                if grand.regen_off == 0 {
                    0.0
                } else {
                    (grand.regen_on as f64 / grand.regen_off as f64 - 1.0) * 100.0
                },
                grand.elements_off,
                grand.elements_added,
                grand.edges,
                grand.cross_db,
                grand.millis_off,
                grand.millis_on
            );
            assert!(grand.windows > 0, "一窗都没量到——目录或模板目录不对");
        }
    }
}
