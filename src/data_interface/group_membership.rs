//! GPWL / GPSET 分组的二级成员解析（E3D secondary hierarchy 的读侧）。
//!
//! 存储视图与解析视图分层：GPSET 成员链上挂的是 GPITEM 元素记录，真身在 GPITEM 的
//! `GPREF`（可跨库）；GPWL 的成员就是 GPSET。这里只产出解析结果，不改任何存储视图的
//! 口径——`children` / `subtree_refnos` / 共用的生成根遍历照旧按存储成员走。
//! 取证见 `docs/plans/2026-11-08-gpwl-gpset-group-nodes.md` §0，接口契约见同目录 `-dev-plan.md` §2。
//!
//! **规则组（G3）**：APS 的 29 个 GPSET 全部是存储成员为 0 的 `SCOSEL` 规则组，存储成员
//! 这条路解出空表。 [`resolved_members_with_rules`] 在存储成员之后按规则求值补上命中成员，
//! 来源是 [`ResolvedMember::Rule`]——它没有 GPITEM，`via()` 是 `None`。规则求值本身在
//! `scosel_eval`（纯函数），这里只负责「读 `SCOSEL` → decode → evaluate → 去重追加」。
//! 坏槽走 [`UnresolvedReason::ScoselEvalFailed`] / [`UnresolvedReason::ScoselUnsupported`]，
//! 与 GPREF 链的三条原因码分开，既有语义一个不复用（`library/scosel-semantics.md` §9）。

use std::collections::{HashMap, HashSet};

use aios_core::RefU64;
use e3d_io::record::explicit::ExplicitValue;

use super::direct_store::DirectStoreError;
use super::direct_tree::TreeStore;
use super::scosel_eval::{ScoselScope, evaluate};

pub const GROUP_WORLD_NOUN: &str = "GPWL";
pub const GROUP_SET_NOUN: &str = "GPSET";
pub const GROUP_ITEM_NOUN: &str = "GPITEM";

/// GPWL / GPSET：容器自己不出几何（NWEI / GWEI / RWEI 恒 0），成员要经解析才是设计元素。
pub fn is_group_noun(noun: &str) -> bool {
    let noun = noun.trim();
    noun.eq_ignore_ascii_case(GROUP_WORLD_NOUN) || noun.eq_ignore_ascii_case(GROUP_SET_NOUN)
}

/// 分组的一个解析成员。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedMember {
    /// 成员链上的元素本身就是成员（GPWL 下的 GPSET 等）。
    Owned(RefU64),
    /// GPITEM 经 `GPREF` 解引用得到的元素；它的属主是自己的主链，不是这个分组。
    Reference { via: RefU64, target: RefU64 },
    /// 规则组 `SCOSEL` 求值命中的元素。它**没有** GPITEM：分组里根本没写这个成员，
    /// 是规则把它算了进来，所以 `via()` 是 `None`（不是「丢了 via」，是本来就没有）。
    Rule { element: RefU64 },
}

impl ResolvedMember {
    pub fn element(&self) -> RefU64 {
        match *self {
            Self::Owned(element) => element,
            Self::Reference { target, .. } => target,
            Self::Rule { element } => element,
        }
    }

    pub fn via(&self) -> Option<RefU64> {
        match *self {
            Self::Owned(_) => None,
            Self::Reference { via, .. } => Some(via),
            // 规则成员没有经由的 GPITEM。契约里这一格必须是 `null` 而不是省略
            // （`AGENTS.md` 实现约束 11），所以调用方拿 `None` 要照常发键。
            Self::Rule { .. } => None,
        }
    }

    /// 这个成员是规则命中的（而不是存储成员链上写的）。契约的 `rule` 字段由它决定。
    pub fn is_rule(&self) -> bool {
        matches!(*self, Self::Rule { .. })
    }
}

/// 解析不出真身的槽为什么解析不出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// GPITEM 没有 `GPREF`，或是空引用。
    GprefMissing,
    /// `GPREF` 指向的库不在本 MDB（或没有文件）。
    DbNotInMdb,
    /// 库在，元素不在。
    TargetMissing,
    /// 规则组的 `SCOSEL` 字节解不出来（`ScoselError`），整条规则没参与求值。
    ///
    /// 与 GPREF 链的三条原因码分家：`SCOSEL` 坏掉时槽上没有 `via` 也没有 `target`，
    /// 它描述的是「这条规则没算」，不是「某个成员指针断了」。
    ScoselEvalFailed,
    /// 规则解出来了，但求值器有部分形状认不出（`EvalOutcome.unsupported` 非空）。
    ///
    /// 命中到的合法成员**仍然返回**（部分成功）——这一码只说「还缺一块」，不是「全丢」。
    ScoselUnsupported,
    /// 规则完整解出、求值器也没有认不出的形状，但**一个成员都没命中**。
    ///
    /// 为什么单独一码（`validation-contract.md` VAL-LIVE-003）：APS 的 `/AREA01_HVAC`、
    /// `/AREA03_HVAC`、`/AREA03_SUPP` 三条规则在当前语料里命中 0 个。若这里只回空表、不出槽，
    /// 前端看到的就是「0 行 + 无原因」——与「这条规则坏了」无法区分，正是本 mission 要消灭的
    /// 静默空转。`scosel_eval` 的注释说「空 members + 空 unsupported 是真正的没命中，可以
    /// 安全地当成空分组」，那是**求值器**的语义（它答「命中了谁」）；上层要把「没命中」
    /// 这件事说出来，才不是静默。实测 27 条规则组里正好 3 条走这条码。
    ScoselNoMatch,
    /// 分组既没有存储成员、也没有规则（或规则为空），展开出来什么都没有。
    ///
    /// 为什么也要一条码：VAL-LIVE-003 的判定是「29 个 GPSET 里无一个是 0 节点且 0 unresolved」。
    /// APS 的 `/ACCEPT-GROUP`（`2013286764_3`）就是这种组——0 个 GPITEM、0 字 `SCOSEL`。
    /// 它不属于「规则组」，但同样会让前端点开是空白且无任何提示，所以按「空分组」点名。
    /// **开关关着时不会出现**（前两层判据在它之前就返回），所以「开关关 = 逐字不变」不受影响。
    GroupEmpty,
}

impl UnresolvedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GprefMissing => "gpref_missing",
            Self::DbNotInMdb => "db_not_in_mdb",
            Self::TargetMissing => "target_missing",
            Self::ScoselEvalFailed => "scosel_eval_failed",
            Self::ScoselUnsupported => "scosel_unsupported",
            Self::ScoselNoMatch => "scosel_no_match",
            Self::GroupEmpty => "group_empty",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnresolvedSlot {
    pub via: RefU64,
    pub target: Option<RefU64>,
    pub reason: UnresolvedReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupMembers {
    pub members: Vec<ResolvedMember>,
    pub unresolved: Vec<UnresolvedSlot>,
}

/// 按存储原序解析 `parent` 的成员：GPITEM 换成 `GPREF` 指向的元素，其余原样。
///
/// 对任何元素都成立——成员里没有 GPITEM 时结果就是存储成员，调用方不必先判 noun。
/// 同一真身被多个 GPITEM 引用只留第一次。`GPREF` 跨库靠 store 按 refno 自动路由；
/// 坏槽（空引用 / 库不在 MDB / 元素不在）进 `unresolved`，其余读错整体上抛，不静默漏成员。
pub fn resolved_members_on(store: &dyn TreeStore, parent: RefU64) -> anyhow::Result<GroupMembers> {
    let mut out = GroupMembers::default();
    let mut seen = HashSet::new();
    for member in store.members(parent)? {
        let attrs = store.named_attmap(member)?;
        let noun = attrs.get_as_string("TYPE").unwrap_or_default();
        if !noun.trim().eq_ignore_ascii_case(GROUP_ITEM_NOUN) {
            if seen.insert(member) {
                out.members.push(ResolvedMember::Owned(member));
            }
            continue;
        }
        let Some(target) = attrs.get_refu64("GPREF").filter(|target| target.0 != 0) else {
            out.unresolved.push(UnresolvedSlot {
                via: member,
                target: None,
                reason: UnresolvedReason::GprefMissing,
            });
            continue;
        };
        // 存在性用成员表探：比整张属性表（描述符抽取）便宜，库不在 / 元素不在的错误形状相同。
        match store.members(target) {
            Ok(_) => {
                if seen.insert(target) {
                    out.members.push(ResolvedMember::Reference {
                        via: member,
                        target,
                    });
                }
            }
            Err(error) => {
                let reason = match error {
                    DirectStoreError::DbNotInMdb { .. }
                    | DirectStoreError::NoFileForDbnum { .. } => UnresolvedReason::DbNotInMdb,
                    DirectStoreError::NoSuchElement { .. }
                    | DirectStoreError::InvalidRefNo { .. } => UnresolvedReason::TargetMissing,
                    other => {
                        return Err(anyhow::Error::from(other)
                            .context(format!("读 GPITEM {member} 的 GPREF 目标 {target} 失败")));
                    }
                };
                out.unresolved.push(UnresolvedSlot {
                    via: member,
                    target: Some(target),
                    reason,
                });
            }
        }
    }
    Ok(out)
}

/// 规则组 `SCOSEL` 的属性名（`db1_hash("SCOSEL")` = 175760542，见
/// `library/scosel-semantics.md` §4）。真库里它是 type-20 hunk，读侧走
/// [`TreeStore::explicit_int_array`] 取解好的 int 数组。
pub const GROUP_RULE_ATTRIBUTE: &str = "SCOSEL";

/// 按存储原序解析 `parent` 的成员，**再按规则求值追加命中成员**（G3）。
///
/// 三层判据依次收窄，任一层不成立就与 [`resolved_members_on`] **逐字相同**：
///
/// 1. `scope` 是 `None`——调用方没有求值域，规则无从算起（这是 wiring 之前的老路径）；
/// 2. 开关 `group_scosel` 关——默认关，关着时所有响应与 HEAD 逐字相同；
/// 3. `parent` 上没有非空 `SCOSEL`——普通分组、或显式写了个空规则组。
///
/// 三层都过了才读规则：`decode` 失败记 [`UnresolvedReason::ScoselEvalFailed`]，求值有
/// 认不出的形状记 [`UnresolvedReason::ScoselUnsupported`] 但**已算出的成员照常返回**
/// （部分成功）。两条原因码与 GPREF 链的三条彻底分开——坏的是规则，不是成员指针。
///
/// 存储成员在前（原序、`via` 有值），规则命中在后（按求值顺序、`via` 是 `None`）。
/// `seen` 去重且**存储成员优先**：同一个元素既写在 GPITEM 里又被规则命中时，保留存储
/// 那条路径（它有 `via`，前端据此显示「引用」角标）。
pub fn resolved_members_with_rules(
    store: &dyn TreeStore,
    parent: RefU64,
    scope: Option<&dyn ScoselScope>,
) -> anyhow::Result<GroupMembers> {
    let mut out = resolved_members_on(store, parent)?;
    // 判据 1 / 2：没有求值域、或开关关着，一个字节都不多读。
    let Some(scope) = scope else {
        return Ok(out);
    };
    if !crate::options::group_scosel() {
        return Ok(out);
    }
    // 判据 3：没有 SCOSEL 就不是规则组。空数组（len=0 的 hunk）同样是「没有规则」——
    // `group_write.rs` 的 SetRules 注释说明空 Vec 写的是 len=0 的 hunk，读侧「空数组」与
    // 「没这个属性」都要当成没有规则，否则清完规则的组会变成一条坏槽。
    let Some(words) = store.explicit_int_array(parent, GROUP_RULE_ATTRIBUTE)? else {
        note_empty_group(store, parent, &mut out)?;
        return Ok(out);
    };
    if words.is_empty() {
        note_empty_group(store, parent, &mut out)?;
        return Ok(out);
    }
    let decoded = match super::scosel::decode(&words) {
        Ok(decoded) => decoded,
        Err(error) => {
            // 字节坏了：整条规则都没参与求值。槽挂在分组自己身上（`via` = parent）——
            // 坏的是这个分组的规则，不是某个 GPITEM。
            log::warn!("GPSET {parent} 的 SCOSEL 解不出来：{error}");
            out.unresolved.push(UnresolvedSlot {
                via: parent,
                target: None,
                reason: UnresolvedReason::ScoselEvalFailed,
            });
            return Ok(out);
        }
    };
    let mut seen: HashSet<RefU64> = out.members.iter().map(ResolvedMember::element).collect();
    let mut unsupported: Vec<String> = Vec::new();
    // 本次规则一共算出几个成员（去重后）。用来判「规则完整解出但一个都没命中」——
    // 那种情况必须出一条 `scosel_no_match`，否则前端看到的就是「0 行 + 无原因」的静默空转。
    let mut rule_hits = 0usize;
    for rule in &decoded.rules {
        let outcome = evaluate(rule, scope);
        for issue in &outcome.unsupported {
            unsupported.push(format!("{:?}: {}", issue.kind, issue.detail));
        }
        for refno in outcome.members {
            if seen.insert(refno) {
                rule_hits += 1;
                out.members.push(ResolvedMember::Rule { element: refno });
            }
        }
    }
    if !unsupported.is_empty() {
        // 一条槽聚合本次所有认不出的形状：`UnresolvedSlot` 没有 detail 字段，逐条开槽
        // 会把前端列表撑成噪声。合法命中已经在 `members` 里了（部分成功）。
        log::warn!(
            "GPSET {parent} 的 SCOSEL 有 {} 处求值器认不出的形状：{}",
            unsupported.len(),
            unsupported.join("；")
        );
        out.unresolved.push(UnresolvedSlot {
            via: parent,
            target: None,
            reason: UnresolvedReason::ScoselUnsupported,
        });
    }
    // 「规则完整解出、没有认不出的形状，但一个成员都没命中」——不能什么都不说。
    //
    // 判据同时要求 `unsupported` 为空：有认不出的形状时已经出了 `scosel_unsupported`，
    // 再叠一条 no_match 会让同一条规则出两个槽，前端只能猜哪个才是原因。
    // 存储成员有没有不影响这条判定——它说的是「规则这条来源没命中」，与 GPITEM 那条无关。
    if rule_hits == 0 && unsupported.is_empty() {
        log::warn!(
            "GPSET {parent} 的 SCOSEL 完整解出（{} 条规则）但命中 0 个元素——规则本身没有坏，\
             是当前语料里没有 DESC 等于这些文本的元素",
            decoded.rules.len()
        );
        out.unresolved.push(UnresolvedSlot {
            via: parent,
            target: None,
            reason: UnresolvedReason::ScoselNoMatch,
        });
    }
    Ok(out)
}

/// 分组既没有存储成员、也没有规则（或规则为空）时，把「这个分组是空的」记成一条槽。
///
/// 只在**真的什么都没解析出来**时记：已经出了坏槽（`gpref_missing` / `db_not_in_mdb` /
/// `target_missing`）的分组不算静默——那三条本身就是「为什么没有成员」的答案，再叠一条
/// `group_empty` 会让同一次解析出两个互相矛盾的原因码（`VAL-MEMBER-008` 钉住既有三码的语义）。
///
/// 开关关着时走不到这里：`resolved_members_with_rules` 的前两层判据在调用它之前就返回了，
/// 所以「开关关 = 逐字不变」不受影响。
fn note_empty_group(
    store: &dyn TreeStore,
    parent: RefU64,
    out: &mut GroupMembers,
) -> anyhow::Result<()> {
    if !out.members.is_empty() || !out.unresolved.is_empty() {
        return Ok(());
    }
    // 复核一遍存储成员：调用方可能已经解析过，但这里要的是「这条分组真的空」这个事实。
    if !store.members(parent)?.is_empty() {
        return Ok(());
    }
    log::warn!("分组 {parent} 既没有存储成员、也没有规则，展开出来是空的");
    out.unresolved.push(UnresolvedSlot {
        via: parent,
        target: None,
        reason: UnresolvedReason::GroupEmpty,
    });
    Ok(())
}

/// 某个元素被哪个分组经哪个 GPITEM 收录（`element ← GPITEM.GPREF ← group`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondaryParent {
    pub group: RefU64,
    pub group_noun: String,
    pub via: RefU64,
}

/// 二级成员的反查表：元素 → 收录它的分组。
///
/// 从各库 WORLD 出发只走分组层级（WORL → GPWL → GPSET → GPITEM，嵌套分组一并展开），
/// 读的是 WORLD 直接成员、分组本身与各成员的 TYPE，不进设计层级，建表代价与分组规模同阶。
#[derive(Debug, Default)]
pub struct GroupIndex {
    by_member: HashMap<RefU64, Vec<SecondaryParent>>,
}

impl GroupIndex {
    /// 不含规则成员的建表：等价于 [`Self::build_with_scope`] 传 `None`。
    ///
    /// 写侧校验（`group_write`）与既有调用方走这条，形状与 G3 引入前逐字相同。
    pub fn build(store: &dyn TreeStore, worlds: &[RefU64]) -> anyhow::Result<Self> {
        Self::build_with_scope(store, worlds, None)
    }

    /// 含规则成员的建表：`scope` 给了且 `group_scosel` 开着时，规则组按 `SCOSEL` 求值补成员。
    ///
    /// 规则成员的 `via()` 是 `None`，本来就不会进这张反查表（它没有经由的 GPITEM），所以
    /// 传 scope 的实际作用是**嵌套规则组继续往下展开**（`is_group_noun` 分支要按解析成员走），
    /// 而不是给规则成员自己造一条二级收录。`scope = None` 时与 [`Self::build`] 逐字相同
    /// （`resolved_members_with_rules` 的第一层判据）。
    pub fn build_with_scope(
        store: &dyn TreeStore,
        worlds: &[RefU64],
        scope: Option<&dyn ScoselScope>,
    ) -> anyhow::Result<Self> {
        let mut index = Self::default();
        let mut stack = Vec::new();
        for &world in worlds {
            for top in store.members(world)? {
                let noun = noun_of(store, top)?;
                if noun == GROUP_WORLD_NOUN {
                    stack.push((top, noun));
                }
            }
        }
        let mut visited = HashSet::new();
        while let Some((group, group_noun)) = stack.pop() {
            if !visited.insert(group) {
                continue;
            }
            for member in resolved_members_with_rules(store, group, scope)?.members {
                let element = member.element();
                if let Some(via) = member.via() {
                    index
                        .by_member
                        .entry(element)
                        .or_default()
                        .push(SecondaryParent {
                            group,
                            group_noun: group_noun.clone(),
                            via,
                        });
                }
                let noun = noun_of(store, element)?;
                if is_group_noun(&noun) {
                    stack.push((element, noun));
                }
            }
        }
        Ok(index)
    }

    pub fn parents_of(&self, element: RefU64) -> &[SecondaryParent] {
        self.by_member
            .get(&element)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// 一个分组管到的设计元素：生成 / 记录按分组请求时的作用范围。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupScope {
    /// 非分组的解析成员，按前序（存储原序）首次出现排列、去重。
    pub members: Vec<RefU64>,
    /// 一路展开遇到的坏槽；同一个分组只展开一次，它的坏槽也只计一次。
    pub unresolved: Vec<UnresolvedSlot>,
}

/// 把 `group` 展开到设计元素：嵌套的 GPWL / GPSET（含经 `GPREF` 引用来的）按前序展开，每个分组只展开
/// 一次，引用成环也会停；非分组成员原样收下，同一元素经多条路径收录只留第一次。分组本身不进
/// `members`——它不出几何，交给生成 / 记录的是它管的元素。
pub fn expand_group_on(store: &dyn TreeStore, group: RefU64) -> anyhow::Result<GroupScope> {
    expand_group_with_rules_on(store, group, None)
}

/// [`expand_group_on`] 的规则感知版：每一层成员都走 [`resolved_members_with_rules`]，
/// 于是规则组在嵌套展开时也补出 `SCOSEL` 命中的成员。
///
/// `scope = None`（或开关关、或该层没有非空 `SCOSEL`）时与 [`expand_group_on`] 逐字相同——
/// 判据全在 [`resolved_members_with_rules`] 里，这里不再重复一遍。
pub fn expand_group_with_rules_on(
    store: &dyn TreeStore,
    group: RefU64,
    scope: Option<&dyn ScoselScope>,
) -> anyhow::Result<GroupScope> {
    let mut scope_out = GroupScope::default();
    let mut expanded = HashSet::from([group]);
    let mut seen = HashSet::new();
    let top = resolved_members_with_rules(store, group, scope)?;
    scope_out.unresolved.extend(top.unresolved);
    let mut stack = vec![top.members.into_iter()];
    while let Some(level) = stack.last_mut() {
        let Some(member) = level.next() else {
            stack.pop();
            continue;
        };
        let element = member.element();
        if is_group_noun(&noun_of(store, element)?) {
            if expanded.insert(element) {
                let nested = resolved_members_with_rules(store, element, scope)?;
                scope_out.unresolved.extend(nested.unresolved);
                stack.push(nested.members.into_iter());
            }
        } else if seen.insert(element) {
            scope_out.members.push(element);
        }
    }
    Ok(scope_out)
}

fn noun_of(store: &dyn TreeStore, refno: RefU64) -> anyhow::Result<String> {
    Ok(store
        .named_attmap(refno)?
        .get_as_string("TYPE")
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase())
}

/// 内存里的树（[`TreeStore`] 的测试替身）：每个元素只有 TYPE、成员和（GPITEM 才有的）GPREF。
/// 本模块的单测与 `web_service::handlers` 的分组契约快照共用。
#[cfg(test)]
pub(crate) mod fake_store {
    use std::collections::{HashMap, HashSet};

    use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

    use super::{GROUP_ITEM_NOUN, GROUP_RULE_ATTRIBUTE};
    use crate::data_interface::direct_store::DirectStoreError;
    use crate::data_interface::direct_tree::TreeStore;

    #[derive(Default)]
    pub(crate) struct FakeStore {
        elements: HashMap<RefU64, (&'static str, Vec<RefU64>, Option<RefU64>)>,
        /// 显式 int 数组属性（只有 `SCOSEL` 用得上），按属性名存原字。
        int_arrays: HashMap<(RefU64, String), Vec<i32>>,
        /// 这些高位字的库不在 MDB，指过去回 `DbNotInMdb`；其余缺失回 `NoSuchElement`。
        pub(crate) foreign: HashSet<u32>,
    }

    impl FakeStore {
        pub(crate) fn add(&mut self, refno: RefU64, noun: &'static str, members: &[RefU64]) {
            self.elements.insert(refno, (noun, members.to_vec(), None));
        }

        pub(crate) fn item(&mut self, refno: RefU64, gpref: Option<RefU64>) {
            self.elements
                .insert(refno, (GROUP_ITEM_NOUN, Vec::new(), gpref));
        }

        /// 给元素挂一段 `SCOSEL` 原字（夹具里的规则组用）。
        pub(crate) fn scosel(&mut self, refno: RefU64, words: &[i32]) {
            self.int_arrays
                .insert((refno, GROUP_RULE_ATTRIBUTE.to_string()), words.to_vec());
        }

        fn missing(&self, refno: RefU64) -> DirectStoreError {
            let high = (refno.0 >> 32) as u32;
            if self.foreign.contains(&high) {
                DirectStoreError::DbNotInMdb {
                    dbnum: high as i32,
                    refno: refno.to_string(),
                }
            } else {
                DirectStoreError::NoSuchElement {
                    refno: refno.to_string(),
                    dbnum: high as i32,
                    sesno: None,
                }
            }
        }
    }

    impl TreeStore for FakeStore {
        fn named_attmap(&self, refno: RefU64) -> Result<NamedAttrMap, DirectStoreError> {
            let (noun, _, gpref) = self
                .elements
                .get(&refno)
                .ok_or_else(|| self.missing(refno))?;
            let mut attrs = NamedAttrMap::default();
            attrs.map.insert(
                "TYPE".into(),
                NamedAttrValue::StringType((*noun).to_string()),
            );
            if let Some(target) = gpref {
                attrs
                    .map
                    .insert("GPREF".into(), NamedAttrValue::RefU64Type(*target));
            }
            Ok(attrs)
        }

        fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
            self.elements
                .get(&refno)
                .map(|(_, members, _)| members.clone())
                .ok_or_else(|| self.missing(refno))
        }

        fn explicit_int_array(
            &self,
            refno: RefU64,
            attr: &str,
        ) -> Result<Option<Vec<i32>>, DirectStoreError> {
            // 元素本身不存在时要报错（与真实现一致：先定位元素，再找属性），
            // 元素在但没这个属性才回 `None`。
            if !self.elements.contains_key(&refno) {
                return Err(self.missing(refno));
            }
            Ok(self.int_arrays.get(&(refno, attr.to_string())).cloned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake_store::FakeStore;
    use super::*;

    fn r(high: u32, low: u32) -> RefU64 {
        RefU64::from_two_nums(high, low)
    }

    /// 设计库 1：WORL 1/1 ─ SITE 1/2 ─ ZONE 1/3 ─ BRAN 1/4。
    /// 分组库 2：WORL 2/1 ─ GPWL 2/10 ─ GPSET 2/11 ─ GPITEM 2/12（→ ZONE 1/3）、GPITEM 2/13（→ BRAN 1/4）。
    fn sample() -> FakeStore {
        let mut store = FakeStore::default();
        store.add(r(1, 1), "WORL", &[r(1, 2)]);
        store.add(r(1, 2), "SITE", &[r(1, 3)]);
        store.add(r(1, 3), "ZONE", &[r(1, 4)]);
        store.add(r(1, 4), "BRAN", &[]);
        store.add(r(2, 1), "WORL", &[r(2, 10)]);
        store.add(r(2, 10), "GPWL", &[r(2, 11)]);
        store.add(r(2, 11), "GPSET", &[r(2, 12), r(2, 13)]);
        store.item(r(2, 12), Some(r(1, 3)));
        store.item(r(2, 13), Some(r(1, 4)));
        store
    }

    #[test]
    fn gpset_items_resolve_to_cross_db_gpref_targets_in_stored_order() {
        let resolved = resolved_members_on(&sample(), r(2, 11)).unwrap();
        assert_eq!(
            resolved.members,
            vec![
                ResolvedMember::Reference {
                    via: r(2, 12),
                    target: r(1, 3)
                },
                ResolvedMember::Reference {
                    via: r(2, 13),
                    target: r(1, 4)
                },
            ]
        );
        assert!(resolved.unresolved.is_empty());
    }

    #[test]
    fn non_item_members_pass_through_unchanged() {
        let store = sample();
        let gpwl = resolved_members_on(&store, r(2, 10)).unwrap();
        assert_eq!(gpwl.members, vec![ResolvedMember::Owned(r(2, 11))]);
        let site = resolved_members_on(&store, r(1, 2)).unwrap();
        assert_eq!(site.members, vec![ResolvedMember::Owned(r(1, 3))]);
        assert!(site.unresolved.is_empty());
    }

    #[test]
    fn bad_slots_are_reported_instead_of_dropped() {
        let mut store = sample();
        store.foreign.insert(9);
        store.add(r(2, 11), "GPSET", &[r(2, 12), r(2, 20), r(2, 21), r(2, 22)]);
        store.item(r(2, 20), None);
        store.item(r(2, 21), Some(r(9, 5)));
        store.item(r(2, 22), Some(r(1, 999)));
        let resolved = resolved_members_on(&store, r(2, 11)).unwrap();
        assert_eq!(
            resolved.members,
            vec![ResolvedMember::Reference {
                via: r(2, 12),
                target: r(1, 3)
            }]
        );
        assert_eq!(
            resolved.unresolved,
            vec![
                UnresolvedSlot {
                    via: r(2, 20),
                    target: None,
                    reason: UnresolvedReason::GprefMissing
                },
                UnresolvedSlot {
                    via: r(2, 21),
                    target: Some(r(9, 5)),
                    reason: UnresolvedReason::DbNotInMdb
                },
                UnresolvedSlot {
                    via: r(2, 22),
                    target: Some(r(1, 999)),
                    reason: UnresolvedReason::TargetMissing
                },
            ]
        );
    }

    #[test]
    fn the_same_target_is_listed_once() {
        let mut store = sample();
        store.add(r(2, 11), "GPSET", &[r(2, 12), r(2, 14)]);
        store.item(r(2, 14), Some(r(1, 3)));
        let resolved = resolved_members_on(&store, r(2, 11)).unwrap();
        assert_eq!(
            resolved.members,
            vec![ResolvedMember::Reference {
                via: r(2, 12),
                target: r(1, 3)
            }]
        );
    }

    #[test]
    fn index_maps_targets_back_to_their_groups_through_nested_sets() {
        let mut store = sample();
        // 嵌套：GPSET 2/11 再收录 GPSET 2/30，后者收录 SITE 1/2。
        store.add(r(2, 11), "GPSET", &[r(2, 12), r(2, 13), r(2, 15)]);
        store.item(r(2, 15), Some(r(2, 30)));
        store.add(r(2, 30), "GPSET", &[r(2, 31)]);
        store.item(r(2, 31), Some(r(1, 2)));
        let index = GroupIndex::build(&store, &[r(1, 1), r(2, 1)]).unwrap();
        let parent = |group: RefU64, via: RefU64| SecondaryParent {
            group,
            group_noun: GROUP_SET_NOUN.to_string(),
            via,
        };
        assert_eq!(index.parents_of(r(1, 3)), &[parent(r(2, 11), r(2, 12))]);
        assert_eq!(index.parents_of(r(1, 4)), &[parent(r(2, 11), r(2, 13))]);
        assert_eq!(index.parents_of(r(2, 30)), &[parent(r(2, 11), r(2, 15))]);
        assert_eq!(index.parents_of(r(1, 2)), &[parent(r(2, 30), r(2, 31))]);
        assert!(
            index.parents_of(r(2, 11)).is_empty(),
            "GPWL → GPSET 是主链，不算二级收录"
        );
        assert!(index.parents_of(r(1, 1)).is_empty());
    }

    #[test]
    fn expanding_a_gpwl_walks_its_sets_down_to_design_elements() {
        let scope = expand_group_on(&sample(), r(2, 10)).unwrap();
        assert_eq!(scope.members, vec![r(1, 3), r(1, 4)]);
        assert!(scope.unresolved.is_empty());
    }

    #[test]
    fn nested_and_cyclic_groups_expand_once_in_preorder() {
        let mut store = sample();
        // GPSET 2/11 → ZONE 1/3、GPSET 2/30、BRAN 1/4；GPSET 2/30 → SITE 1/2、BRAN 1/4、回指 GPSET 2/11。
        store.add(r(2, 11), "GPSET", &[r(2, 12), r(2, 15), r(2, 13)]);
        store.item(r(2, 15), Some(r(2, 30)));
        store.add(r(2, 30), "GPSET", &[r(2, 31), r(2, 32), r(2, 33)]);
        store.item(r(2, 31), Some(r(1, 2)));
        store.item(r(2, 32), Some(r(1, 4)));
        store.item(r(2, 33), Some(r(2, 11)));
        let scope = expand_group_on(&store, r(2, 10)).unwrap();
        assert_eq!(scope.members, vec![r(1, 3), r(1, 2), r(1, 4)]);
        assert!(scope.unresolved.is_empty());
    }

    #[test]
    fn a_group_of_bad_slots_expands_to_nothing_but_keeps_the_slots() {
        let mut store = sample();
        store.foreign.insert(9);
        store.add(r(2, 11), "GPSET", &[r(2, 20), r(2, 21)]);
        store.item(r(2, 20), None);
        store.item(r(2, 21), Some(r(9, 5)));
        let scope = expand_group_on(&store, r(2, 10)).unwrap();
        assert!(scope.members.is_empty());
        assert_eq!(
            scope
                .unresolved
                .iter()
                .map(|slot| slot.reason)
                .collect::<Vec<_>>(),
            vec![UnresolvedReason::GprefMissing, UnresolvedReason::DbNotInMdb]
        );

        store.add(r(2, 11), "GPSET", &[]);
        assert_eq!(
            expand_group_on(&store, r(2, 10)).unwrap(),
            GroupScope::default()
        );
    }

    // ── G3：规则成员 ──────────────────────────────────────────────────────────

    /// 夹具求值域：`DESC` 文本表 + noun 表，口径与 `scosel_eval` 的 `FixtureScope` 一致。
    #[derive(Default)]
    struct RuleScope {
        texts: HashMap<(u32, String), HashSet<RefU64>>,
        nouns: HashMap<u32, HashSet<RefU64>>,
    }

    impl RuleScope {
        fn with_text(mut self, text: &str, refnos: &[RefU64]) -> Self {
            self.texts.insert(
                (
                    super::super::scosel_eval::desc_attribute_hash(),
                    text.to_string(),
                ),
                refnos.iter().copied().collect(),
            );
            self
        }

        fn with_noun(mut self, noun: u32, refnos: &[RefU64]) -> Self {
            self.nouns.insert(noun, refnos.iter().copied().collect());
            self
        }
    }

    impl ScoselScope for RuleScope {
        fn refnos_with_attr_text(&self, attr_hash: u32, text: &str) -> HashSet<RefU64> {
            self.texts
                .get(&(attr_hash, text.to_string()))
                .cloned()
                .unwrap_or_default()
        }

        fn refnos_of_noun(&self, noun_hash: u32) -> HashSet<RefU64> {
            self.nouns.get(&noun_hash).cloned().unwrap_or_default()
        }
    }

    const ZONE_HASH: u32 = 640_493;

    /// 一条 `ALL ZONE WITH ( ATTRIB DESC OF ZONE EQ '<text>' )` 的 SCOSEL 原字，
    /// 逐字照 `library/scosel-semantics.md` §2 的 35 字真实样本，只换文本。
    fn zone_rule_words(text: &str) -> Vec<i32> {
        use super::super::scosel::{
            OP_ATTR_QUALIFIER, OP_ATTRIBUTE_ID, OP_CLASS, OP_FILTER, OP_RULE_GROUP, OP_STR_EQ,
            OP_TEXT,
        };
        let chars: Vec<i32> = text.chars().map(|c| c as i32).collect();
        let mut words = vec![
            0,
            OP_ATTRIBUTE_ID,
            1,
            OP_CLASS,
            3,
            OP_RULE_GROUP,
            ZONE_HASH as i32,
            OP_FILTER,
            0,
            1,
            // 106,4,<DESC>,1,120,0
            106,
            4,
            super::super::scosel_eval::desc_attribute_hash() as i32,
            1,
            120,
            0,
            // 1602,4,2,<ZONE>,6,1701
            OP_ATTR_QUALIFIER,
            4,
            2,
            ZONE_HASH as i32,
            6,
            1701,
            OP_TEXT,
        ];
        words.push(chars.len() as i32);
        words.extend(chars);
        words.push(OP_STR_EQ);
        // `100015` 的 len **含自身**：`len = 1(len) + 1(前导 1) + 表达式字数`。
        // 表达式从 `filter_at + 3` 起（`filter_at` 是 `100015` 自己），所以表达式字数
        // = `words.len() - (filter_at + 3)`。
        let filter_at = 7;
        words[filter_at + 1] = (2 + (words.len() - filter_at - 3)) as i32;
        words[0] = words.len() as i32;
        words
    }

    /// VAL-MEMBER-001：`Rule` 变体的两个访问器；两个既有变体行为不变。
    #[test]
    fn a_rule_member_returns_its_element_and_carries_no_via() {
        let element = r(1, 3);
        let rule = ResolvedMember::Rule { element };
        assert_eq!(
            rule.element(),
            element,
            "规则成员的 element() 必须是那个元素"
        );
        assert_eq!(
            rule.via(),
            None,
            "规则成员没有经由的 GPITEM，via() 必须是 None"
        );
        assert!(rule.is_rule());

        // 既有两个变体不变。
        let owned = ResolvedMember::Owned(r(2, 11));
        assert_eq!(owned.element(), r(2, 11));
        assert_eq!(owned.via(), None);
        assert!(!owned.is_rule());

        let reference = ResolvedMember::Reference {
            via: r(2, 12),
            target: r(1, 3),
        };
        assert_eq!(reference.element(), r(1, 3));
        assert_eq!(reference.via(), Some(r(2, 12)));
        assert!(!reference.is_rule());
    }

    /// VAL-MEMBER-002：`scope = None` 时与 `resolved_members_on` 逐字相同。
    #[test]
    fn without_a_scope_the_result_is_word_for_word_the_stored_members() {
        let store = sample();
        for parent in [r(2, 10), r(2, 11), r(1, 2), r(1, 3)] {
            assert_eq!(
                resolved_members_with_rules(&store, parent, None).unwrap(),
                resolved_members_on(&store, parent).unwrap(),
                "{parent} 在 scope=None 时必须与存储成员逐字相同"
            );
        }
    }

    /// VAL-MEMBER-003：开关关着时，即使有非空 SCOSEL、即使给了 scope，也不追加规则成员。
    #[test]
    fn a_closed_switch_appends_no_rule_member_even_with_a_non_empty_scosel() {
        let mut store = sample();
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _off = crate::options::GroupScoselOverride::set(false);
        assert_eq!(
            resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap(),
            resolved_members_with_rules(&store, r(2, 11), None).unwrap(),
            "开关关着时结果必须与 scope=None 相同"
        );
    }

    /// VAL-MEMBER-004：GPSET 没有 SCOSEL 时，即使开关开、即使给了 scope，也不追加。
    #[test]
    fn a_set_without_scosel_gets_no_rule_member_even_with_the_switch_on() {
        let store = sample();
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        assert_eq!(
            resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap(),
            resolved_members_on(&store, r(2, 11)).unwrap(),
            "没有 SCOSEL 的分组不该凭空多出规则成员"
        );
    }

    /// 空数组（len=0 的 hunk）等同于「没有规则」，不是一条坏槽。
    #[test]
    fn an_empty_scosel_counts_as_no_rule_at_all() {
        let mut store = sample();
        store.scosel(r(2, 11), &[]);
        let scope = RuleScope::default().with_text("AREA01PIPE", &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(resolved, resolved_members_on(&store, r(2, 11)).unwrap());
        assert!(resolved.unresolved.is_empty(), "空规则组不该产生坏槽");
    }

    /// VAL-MEMBER-005：存储成员在前（原序、via 有值），规则命中在后；重复的元素只留存储那条。
    #[test]
    fn stored_members_come_first_and_rule_hits_are_appended_without_duplicates() {
        let mut store = sample();
        // 规则命中 BRAN 1/4（已在存储成员里，经 GPITEM 2/13）与 ZONE 1/3（不在存储成员里）。
        // 夹具改成：存储成员只留 GPITEM 2/13 → BRAN 1/4。
        store.add(r(2, 11), "GPSET", &[r(2, 13)]);
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3), r(1, 4)])
            .with_noun(ZONE_HASH, &[r(1, 3), r(1, 4)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(
            resolved.members,
            vec![
                ResolvedMember::Reference {
                    via: r(2, 13),
                    target: r(1, 4)
                },
                ResolvedMember::Rule { element: r(1, 3) },
            ],
            "存储成员在前（via 有值），规则命中在后；BRAN 1/4 已在存储里，不再作为规则成员重复出现"
        );
        assert!(resolved.unresolved.is_empty());
    }

    /// VAL-MEMBER-006：`SCOSEL` 字节解不出来 → `scosel_eval_failed`，且不混进既有三码。
    #[test]
    fn a_broken_scosel_reports_scosel_eval_failed_and_nothing_else() {
        let mut store = sample();
        store.scosel(r(2, 11), &[9, 9, 9]);
        let scope = RuleScope::default();
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        // 存储成员照常解析（坏的是规则，不是 GPREF 链）。
        assert_eq!(
            resolved.members,
            resolved_members_on(&store, r(2, 11)).unwrap().members
        );
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["scosel_eval_failed"],
            "坏 SCOSEL 只能出一条 scosel_eval_failed，不能混进 gpref_missing / db_not_in_mdb / target_missing"
        );
    }

    /// VAL-MEMBER-007：`unsupported` 非空 → `scosel_unsupported`，且已算出的合法成员仍然返回。
    #[test]
    fn an_unsupported_rule_reports_scosel_unsupported_and_keeps_the_good_hits() {
        let mut store = sample();
        // 存储成员只留 GPITEM 2/13（→ BRAN 1/4），让第一条规则算出的 ZONE 1/3 不在存储成员里，
        // 这样「部分成功」的成员一定是规则路径带进来的。
        store.add(r(2, 11), "GPSET", &[r(2, 13)]);
        // 两条规则：第一条认得出（ZONE + DESC），第二条属性 hash 不是 DESC（NAME）。
        let good = zone_rule_words("AREA01PIPE");
        let mut bad = zone_rule_words("WHATEVER");
        // 把 `106,4,<DESC>` 的 hash 换成 NAME（639374）。
        let desc_at = bad
            .iter()
            .position(|word| *word == super::super::scosel_eval::desc_attribute_hash() as i32)
            .expect("夹具里应有 DESC hash");
        bad[desc_at] = e3d_attlib::db1_hash("NAME") as i32;
        // 第二条规则自成一段：用段外的老形状 `100007,<attr>,100015,...`——decode 两种形状
        // 都认（`scosel.rs` 的 `OP_RULE_GROUP` 分支），而 class 段里的规则组标记只够配第一条。
        let mut words = good.clone();
        words[0] = 0;
        words.extend_from_slice(&[super::super::scosel::OP_RULE_GROUP, ZONE_HASH as i32]);
        words.extend_from_slice(&bad[7..]);
        words[0] = words.len() as i32;
        store.scosel(r(2, 11), &words);

        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert!(
            resolved
                .members
                .contains(&ResolvedMember::Rule { element: r(1, 3) }),
            "第一条规则算出的合法成员必须仍然返回（部分成功）：{:?}",
            resolved.members
        );
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["scosel_unsupported"],
            "认不出的形状只能出 scosel_unsupported"
        );
    }

    /// VAL-LIVE-003 的单元侧底座：规则完整解出、求值器没有认不出的形状、但命中 0 个成员时，
    /// 必须出一条 `scosel_no_match`——不能回「0 成员 + 0 槽」的静默空转。
    ///
    /// APS 实测正好三条走这条码（`/AREA01_HVAC`、`/AREA03_HVAC`、`/AREA03_SUPP`），
    /// 见 `library/g3-rule-members.md` §5 与 E3D `Q ALL` 的 `None`。
    #[test]
    fn a_rule_that_matches_nothing_reports_scosel_no_match() {
        let mut store = sample();
        // 存储成员清零：这条码只说「规则这条来源没命中」，别被 GPITEM 成员的成败干扰。
        store.add(r(2, 11), "GPSET", &[]);
        store.scosel(r(2, 11), &zone_rule_words("AREA01HVAC"));
        // 求值域里没有任何 DESC == AREA01HVAC 的元素（真库语料就是这样）。
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert!(resolved.members.is_empty(), "{:?}", resolved.members);
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["scosel_no_match"],
            "规则没坏、只是没命中：必须出 scosel_no_match，不能静默"
        );

        // 反向：命中非空时不许出这条码。
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(resolved.members.len(), 1, "{:?}", resolved.members);
        assert!(resolved.unresolved.is_empty(), "{:?}", resolved.unresolved);

        // 反向：有认不出的形状时只出 `scosel_unsupported`，不叠一条 no_match
        // （同一规则两个槽，前端只能猜哪个是原因）。
        let mut bad = zone_rule_words("AREA01PIPE");
        let desc_at = bad
            .iter()
            .position(|word| *word == super::super::scosel_eval::desc_attribute_hash() as i32)
            .expect("夹具里应有 DESC hash");
        bad[desc_at] = e3d_attlib::db1_hash("NAME") as i32;
        store.scosel(r(2, 11), &bad);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["scosel_unsupported"],
            "认不出的形状已经出了槽，不再叠 no_match"
        );
    }

    /// 空分组也要点名：既没有存储成员、也没有规则时，出 `group_empty`。
    ///
    /// APS 的 `/ACCEPT-GROUP`（`2013286764_3`）就是这种组——0 个 GPITEM、0 字 `SCOSEL`。
    /// 它是 VAL-LIVE-003「无一个是 0 节点且 0 unresolved」在真库上的最后一个缺口。
    #[test]
    fn a_group_with_neither_members_nor_rules_reports_group_empty() {
        let mut store = sample();
        store.add(r(2, 11), "GPSET", &[]);
        let scope = RuleScope::default();
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert!(resolved.members.is_empty());
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["group_empty"],
            "空分组必须出 group_empty，不能 0 节点 + 0 槽"
        );

        // 空 SCOSEL 也是空分组（不是一条坏槽）。
        store.scosel(r(2, 11), &[]);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["group_empty"]
        );

        // 已有坏槽的分组不再叠 group_empty（同一次解析两个原因码会自相矛盾）。
        let mut bad = sample();
        bad.add(r(2, 11), "GPSET", &[r(2, 20)]);
        bad.item(r(2, 20), None);
        let resolved = resolved_members_with_rules(&bad, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect::<Vec<_>>(),
            vec!["gpref_missing"],
            "坏 GPREF 槽本身就是「为什么没有成员」，不再叠 group_empty"
        );

        // 开关关着时一个槽都不多（「开关关 = 逐字不变」）。
        // 注意：`GroupScoselOverride::set` 是进程级互斥量，两个守卫同时在栈上会自锁——
        // 先把上一个守卫落掉（作用域结束）再取下一个。
        drop(_on);
        let _off = crate::options::GroupScoselOverride::set(false);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert!(resolved.unresolved.is_empty(), "{:?}", resolved.unresolved);
    }

    /// VAL-MEMBER-008：三个既有原因码仍然只表示 GPREF 链的三种失败，语义没被复用。
    #[test]
    fn the_three_gpref_reason_codes_keep_their_meaning() {
        assert_eq!(UnresolvedReason::GprefMissing.as_str(), "gpref_missing");
        assert_eq!(UnresolvedReason::DbNotInMdb.as_str(), "db_not_in_mdb");
        assert_eq!(UnresolvedReason::TargetMissing.as_str(), "target_missing");
        assert_eq!(
            UnresolvedReason::ScoselEvalFailed.as_str(),
            "scosel_eval_failed"
        );
        assert_eq!(
            UnresolvedReason::ScoselUnsupported.as_str(),
            "scosel_unsupported"
        );

        // 开关开着 + 有 scope，坏 GPREF 槽仍然出原三条码，一条规则码都不多。
        let mut store = sample();
        store.foreign.insert(9);
        store.add(r(2, 11), "GPSET", &[r(2, 20), r(2, 21), r(2, 22)]);
        store.item(r(2, 20), None);
        store.item(r(2, 21), Some(r(9, 5)));
        store.item(r(2, 22), Some(r(1, 999)));
        let scope = RuleScope::default();
        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(
            resolved
                .unresolved
                .iter()
                .map(|slot| slot.reason)
                .collect::<Vec<_>>(),
            vec![
                UnresolvedReason::GprefMissing,
                UnresolvedReason::DbNotInMdb,
                UnresolvedReason::TargetMissing,
            ]
        );
    }

    /// 接线：`expand_group_with_rules_on` 在 `scope = None` 时必须与 `expand_group_on`
    /// 逐字相同——这是「开关关时响应逐字不变」在生成 / 记录这条路上的底座。
    #[test]
    fn expanding_without_a_scope_matches_the_stored_only_expansion() {
        let store = sample();
        assert_eq!(
            expand_group_with_rules_on(&store, r(2, 10), None).unwrap(),
            expand_group_on(&store, r(2, 10)).unwrap()
        );
    }

    /// 接线：规则组的成员要能进 `GroupScope.members`，否则 `/model/generate` 与
    /// `/model/records` 对规则组仍然是空的（G3 的核心修复点）。
    #[test]
    fn a_rule_group_expands_to_its_rule_hits_when_the_scope_is_given() {
        let mut store = sample();
        // GPSET 2/11 的存储成员清零，只留规则：命中 ZONE 1/3。
        store.add(r(2, 11), "GPSET", &[]);
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let expanded = expand_group_with_rules_on(&store, r(2, 10), Some(&scope)).unwrap();
        assert_eq!(
            expanded.members,
            vec![r(1, 3)],
            "规则命中的 ZONE 必须进 GroupScope.members，生成与记录才有范围"
        );
        assert!(expanded.unresolved.is_empty());
    }

    /// 接线：开关关着时即使给了 scope，展开结果也与 `scope = None` 逐字相同。
    #[test]
    fn a_closed_switch_leaves_the_expansion_byte_identical() {
        let mut store = sample();
        store.add(r(2, 11), "GPSET", &[]);
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _off = crate::options::GroupScoselOverride::set(false);
        assert_eq!(
            expand_group_with_rules_on(&store, r(2, 10), Some(&scope)).unwrap(),
            expand_group_on(&store, r(2, 10)).unwrap(),
            "开关关着时展开必须与存储视图逐字相同"
        );
    }

    /// 接线：`GroupIndex::build_with_scope(None)` 与 `build` 逐字相同（反查表不得因
    /// G3 接线而变形状）。
    #[test]
    fn the_group_index_without_a_scope_matches_the_plain_build() {
        let store = sample();
        let worlds = [r(1, 1), r(2, 1)];
        let plain = GroupIndex::build(&store, &worlds).unwrap();
        let scoped = GroupIndex::build_with_scope(&store, &worlds, None).unwrap();
        for element in [r(1, 2), r(1, 3), r(1, 4), r(2, 11)] {
            assert_eq!(plain.parents_of(element), scoped.parents_of(element), "{element}");
        }
    }

    /// 接线：规则成员自己没有 `via`，所以不进反查表——给了 scope 也不会给它凭空造一条
    /// 「被哪个 GPITEM 收录」的记录（那会让前端显示一个不存在的引用角标）。
    #[test]
    fn rule_members_carry_no_secondary_parent() {
        let mut store = sample();
        store.add(r(2, 11), "GPSET", &[]);
        store.scosel(r(2, 11), &zone_rule_words("AREA01PIPE"));
        let scope = RuleScope::default()
            .with_text("AREA01PIPE", &[r(1, 3)])
            .with_noun(ZONE_HASH, &[r(1, 3)]);
        let _on = crate::options::GroupScoselOverride::set(true);
        let index = GroupIndex::build_with_scope(&store, &[r(1, 1), r(2, 1)], Some(&scope)).unwrap();
        assert!(
            index.parents_of(r(1, 3)).is_empty(),
            "规则成员没有经由的 GPITEM，反查表不该给它编一条"
        );
    }
}
