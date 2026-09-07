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
//! 几何上今天不显：e3d-model 第 4 阶段（SCTN 关节负体）未接，对端重算出来的梁还是毛坯；先接线，
//! 第 4 阶段一落地这里就是正确性项（计划 §3 C2 ★0907）。

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
    /// 会级联的边里，对端在 T 端 owner 链上找不到任何生成根（对端已删 / 无根族）。
    pub far_end_without_root: usize,
    /// 要并进 `touched` 的对端根（都在 `roots_T` 里）。
    pub far_end_roots: BTreeSet<RefnoEnum>,
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
        format!(
            "dbnum={dbnum}: structural-connection cascade on — sources_read={} \
             unreadable_sources={} edges={} backref_ok={} backref_mismatch={} \
             backref_unreadable={} gated_by_stru={} far_end_without_root={} far_end_roots={}",
            self.sources_read,
            self.unreadable_sources,
            self.edges,
            self.backref_ok,
            self.backref_mismatch,
            self.backref_unreadable,
            self.gated_by_stru,
            self.far_end_without_root,
            self.far_end_roots.len()
        )
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
        assert!(cascade.far_end_roots.is_empty());
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
            line.contains("cascade on") && line.contains("far_end_roots=1"),
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
        /// 开关开：回指成立的边 ≥ 普查数、对端根 ∈ `regen`、且 `regen` 严格多于关着时；
        /// 开关关：对端根一个都不进。只读文件：`AIOS_APS_PROJECT_DIR`（默认
        /// `D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000`）+ `AIOS_E3D_TEMPLATE_DIR`。
        #[test]
        #[ignore = "manual live: needs the AvevaPlantSample DESI files and the E3D template directory"]
        fn live_aps_one_ended_windows_pull_the_far_end_roots_into_regen() {
            let dir =
                PathBuf::from(std::env::var("AIOS_APS_PROJECT_DIR").unwrap_or_else(|_| {
                    r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000".into()
                }));
            let schema = DirectSchema::open_from_env().expect("E3D template directory");
            let table = [
                ("aps7200_0001", 3u32, 4u32, "SJOI", 94usize),
                ("aps250110_0001", 29, 30, "FIXING", 193),
            ];
            for (name, base_sesno, target_sesno, noun, census_edges) in table {
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
                assert!(off.far_end_roots.is_empty(), "{name}: 开关关不该读边");
                let off_set: BTreeSet<RefnoEnum> = regen_off.iter().copied().collect();
                assert!(
                    regen_on
                        .iter()
                        .all(|root| off_set.contains(root) || on.far_end_roots.contains(root)),
                    "{name}: 开关开只能多出对端根"
                );
                assert!(
                    regen_on.len() > regen_off.len(),
                    "{name}: 这是「只改一端」的窗口，开关开必须多出至少一根（on={} off={}）",
                    regen_on.len(),
                    regen_off.len()
                );
            }
        }

        /// 九个钉死窗口（`window_root_plan::tests::live_ams8000_pinned_windows_land_in_the_expected_buckets`
        /// 的表）开关两态 `regen` 逐字相同——它们没有跨根的结构连接改动。只读文件：
        /// `AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
        #[test]
        #[ignore = "manual live: needs the real ams8000 DESI file and the E3D template directory"]
        fn live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off() {
            let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
                |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
            ));
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
    }
}
