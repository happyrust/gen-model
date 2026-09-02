//! Core3D `PartialUpdateDesiMgr` 的粒度与去重规则，写成一份可执行的参考模型。
//!
//! **这不是生产路径。** 它不连库、不读我们的 noun 名单、不产生工作项——只按 core 的
//! 两个 noun 位（significant / primitive）在一棵抽象元素树上推演，产出 core 会往它那条
//! 待办队列里追加的记录序列。存在的理由只有一个：**给生产实现当契约**。
//!
//! 为什么先落契约再动生产代码：逆向结论是从指令流读出来的，读错过一次
//! （`AncestorDeletes` 命中已标记祖先时是否终止上行，方向整个反了），而那一条正好是
//! 计划 T3.3 要照抄的。规则写成能跑的代码、配上用例，下一次读错就会红，不会一路带到生产。
//!
//! 规则编号（R…）与用例编号（C…）对应：
//! - `docs/specs/core3d-partial-update-conformance.md`
//! - `docs/specs/core3d-partial-update-test-cases.md`
//!
//! 取证：`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`
//!
//! **不建模的部分**：`m_granularityMode ≠ 0` 那条负实体上卷分支（证据 §6.1 证明恒不可达）
//! 与它专用的 `SearchMode::Negative`。照着死代码建模只会引诱下一个人去实现它。

use std::collections::HashSet;

use aios_core::RefnoEnum;

/// core 眼里一个元素的全部信息：两个 noun 描述符位。
///
/// 两个位都从 `DB_Noun::getField(id, &out)` 的**出参**取，`getField` 自己的返回值
/// （"这个 noun 登没登记这个字段"）被丢弃——所以按 core 的口径**字段未登记 = 该位为假**
/// （R0-1）。`primitive` 是 `0xA103E ∨ 0xBBD5ADC` 的或值，而这个或式**跨版本不稳定**
/// （R0-2）：快照要分开存两位，合成只发生在这里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NounBits {
    /// `0x5657A0A` / `90536458`
    pub significant: bool,
    /// `0xA103E` / `659518` ∨ `0xBBD5ADC` / `196958940`
    pub primitive: bool,
}

/// 参考模型读元素树的最小接口。
pub trait ElementTree {
    fn owner(&self, elem: RefnoEnum) -> Option<RefnoEnum>;

    /// 直接成员，顺序与 `DB_Element::members()` 一致。
    fn members(&self, elem: RefnoEnum) -> Vec<RefnoEnum>;

    fn bits(&self, elem: RefnoEnum) -> NounBits;
}

/// 视图 ID 清单（`PDMS_Idlist2`）在模型里的最小形状，`AbsentPrimitives` 的判据。
pub trait IdList {
    /// `PDMS_Idlist2 +0x18`。
    ///
    /// **为假时 core 把整棵子树的 primitive 全判为缺失**（R23）——而 `Update` 的 pass 2
    /// 刚把这个块画完。这条语义还没在 live 进程上确认（用例 C3-4），模型照 core 的指令流
    /// 实现，不替它打圆场。
    fn is_active(&self) -> bool;

    fn contains(&self, elem: RefnoEnum) -> bool;
}

/// `PartialUpdateDesiMgr::ModelState`。0/1/3 由外部入口给，2/4/5 是内部态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelState {
    Changed,
    New,
    /// `AncestorDeletes` 打的标记。三遍都不消费它，但它有两个间接消费者：
    /// `IsPending(_, Changed)` 的最后一步，以及 `EraseModel(DB_Ref&)` 的策略切换（R24）。
    AncestorDelete,
    Deleted,
    /// 被重画的 significant 块内、自己也 significant 的后代。只擦不画。
    MemberOfChangedSignificant,
    /// `AbsentPrimitives` 挑出来的孤儿图元。在 pass 3 单独擦。
    AbsentPrimitive,
}

impl ModelState {
    /// `AncestorDeletes` 动作、`AbsentPrimitives` 不动作的那一组。
    fn is_delete_class(self) -> bool {
        matches!(self, Self::Deleted | Self::MemberOfChangedSignificant)
    }
}

/// 待办队列里的一条。core 那边是 24 字节：`DB_Ref`(12) + 两个句柄字 + `state`(4)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub elem: RefnoEnum,
    pub state: ModelState,
}

/// `Members` 的收集模式。缺 `Negative`（挂在死代码上，见模块头）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Significant,
    Primitive,
}

/// 从 `elem` 自己开始的 owner 链，**含自身**。
///
/// core 没有深度上限也没有环检测，坏数据在它那边会死循环；这里的 `seen` 是模型的防御，
/// 目的是让环变成一个可断言的结果而不是挂住测试进程。
fn self_and_ancestors(tree: &impl ElementTree, elem: RefnoEnum) -> Vec<RefnoEnum> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(elem);
    while let Some(node) = current {
        if !seen.insert(node) {
            break;
        }
        chain.push(node);
        current = tree.owner(node);
    }
    chain
}

/// R14 —— `SignificantOwner`：**含自身**的向上攀爬，终止条件是 significant 位。
///
/// 与 [`super::generation_root::resolve_element_generation_root`] 三点不同：从元素自己
/// 开始判、判据是位不是 SITE/ZONE/WORL、**没有深度上限**。loop 容器不需要特例——
/// 它本来就不 significant，自然被跨过。
///
/// 攀到顶返回 `None`。core 在这里会把那个无效引用照样 push 进队列，模型不 push：
/// 无效引用在我们这边没有对应物，而且它在 core 的 pass 1 里也会因为
/// `EraseModel` 与 `Exists` 双双失败被就地清掉（R28），可观测结果一致。
pub fn significant_owner(tree: &impl ElementTree, elem: RefnoEnum) -> Option<RefnoEnum> {
    self_and_ancestors(tree, elem)
        .into_iter()
        .find(|node| tree.bits(*node).significant)
}

/// R11 —— `Members(e, mode)`：显式栈的 LIFO 遍历，种子是 `e` 的直接成员。
///
/// 两个模式的差别全在"下潜"这一栏，收集规则反而是次要的：
/// - `Significant`：**只穿过 significant 成员**。非 significant 的子节点会挡住它下面的
///   significant 孙节点——这不是遍历实现的副作用，是判据本身（`0x1047E37E` / `0x1047E381`
///   一起跳过收集与下潜两件事）。
/// - `Primitive`：对所有成员下潜，走整棵子树。
///
/// 把前者写成"全子树找 significant"，块内成员清理就会多删一批行。
pub fn members(tree: &impl ElementTree, elem: RefnoEnum, mode: SearchMode) -> Vec<RefnoEnum> {
    let mut out = Vec::new();
    let mut stack = tree.members(elem);
    let mut seen = HashSet::new();

    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        let bits = tree.bits(current);
        match mode {
            SearchMode::Significant => {
                if !bits.significant {
                    continue;
                }
                out.push(current);
            }
            SearchMode::Primitive => {
                if bits.primitive {
                    out.push(current);
                }
            }
        }
        stack.extend(tree.members(current));
    }
    out
}

/// `IsPresent`：队列线性扫描 `rec.state == state && rec.elem == elem`。
fn is_present(queue: &[Record], elem: RefnoEnum, state: ModelState) -> bool {
    queue
        .iter()
        .any(|record| record.state == state && record.elem == elem)
}

/// `IsDecendantPresent`：队列里存在一条该 state 的记录，沿它自己的 owner 链能走到 `elem`
/// ——即该记录是 `elem` 的子孙**或就是 `elem` 自己**。
fn is_descendant_present(
    tree: &impl ElementTree,
    queue: &[Record],
    elem: RefnoEnum,
    state: ModelState,
) -> bool {
    queue
        .iter()
        .filter(|record| record.state == state)
        .any(|record| self_and_ancestors(tree, record.elem).contains(&elem))
}

/// R17–R20 —— `IsPending`：命中即把本次变化**整个丢弃**。
///
/// **三个 state 三套判法，不是一套。** 共用的只有第一步：把键归一化到块
/// （`IsPrimitive(e) ? e : SignificantOwner(e)`，R20）。之后：
/// - `Changed` 沿 owner 链找 New 或 Changed，再看子孙有没有排着 New，最后看自己有没有
///   被祖先删除标记打过；
/// - `New` 只沿链找 New，不看标记也不看子孙；
/// - `Deleted` **完全不上行**，只看键自己的 Deleted / MemberOfChangedSignificant。
pub fn is_pending(
    tree: &impl ElementTree,
    queue: &[Record],
    elem: RefnoEnum,
    state: ModelState,
) -> bool {
    let key = if tree.bits(elem).primitive {
        Some(elem)
    } else {
        significant_owner(tree, elem)
    };
    let Some(key) = key else {
        return false;
    };

    match state {
        ModelState::Changed => {
            let chain = self_and_ancestors(tree, key);
            if chain.iter().any(|current| {
                is_present(queue, *current, ModelState::New)
                    || is_present(queue, *current, ModelState::Changed)
            }) {
                return true;
            }
            is_descendant_present(tree, queue, key, ModelState::New)
                || is_present(queue, key, ModelState::AncestorDelete)
        }
        ModelState::New => self_and_ancestors(tree, key)
            .iter()
            .any(|current| is_present(queue, *current, ModelState::New)),
        ModelState::Deleted => {
            is_present(queue, key, ModelState::Deleted)
                || is_present(queue, key, ModelState::MemberOfChangedSignificant)
        }
        _ => false,
    }
}

/// R21 —— `AncestorDeletes`：删除类状态下，给整条 owner 链上每个合格祖先打 state-2 标记。
///
/// 判据是 `IsPrimitive(anc) ∨ IsSignificant(anc)`，两个都假就跳过这一级继续往上。
///
/// **命中"祖先已被标记"只跳过该级的 push，上行链照走到顶。** 证据文档第一版把这条写成
/// "整条上行终止"，方向是反的：`0x1047C14F` 的 `jnz` 去的 `0x1047C1F4` 是 `mov al, 1`
/// 而不是 return，转一圈回到 `0x1047C19D` 继续取 owner。按错的那版实现，删除路径的标记
/// 会少一大半，R24 的第二个消费者也跟着大面积失效。用例 C1-7 钉的就是这一条。
pub fn ancestor_deletes(
    tree: &impl ElementTree,
    queue: &mut Vec<Record>,
    elem: RefnoEnum,
    state: ModelState,
) {
    if !state.is_delete_class() {
        return;
    }
    for ancestor in self_and_ancestors(tree, elem).into_iter().skip(1) {
        let bits = tree.bits(ancestor);
        if !(bits.primitive || bits.significant) {
            continue;
        }
        if !is_present(queue, ancestor, ModelState::AncestorDelete) {
            queue.push(Record {
                elem: ancestor,
                state: ModelState::AncestorDelete,
            });
        }
    }
}

/// R22 —— `AbsentPrimitives`：重画一个块之前，把块内"模型里有、当前 ID 清单里没有"的
/// 图元行挑出来交给 pass 3 擦掉。只在非删除类状态下动作。
///
/// 用的是 `SearchMode::Primitive`，**整棵子树**，不是直接成员。
pub fn absent_primitives(
    tree: &impl ElementTree,
    id_list: &impl IdList,
    queue: &mut Vec<Record>,
    elem: RefnoEnum,
    state: ModelState,
) {
    if state.is_delete_class() {
        return;
    }
    for primitive in members(tree, elem, SearchMode::Primitive) {
        if id_list.is_active() && id_list.contains(primitive) {
            continue;
        }
        queue.push(Record {
            elem: primitive,
            state: ModelState::AbsentPrimitive,
        });
    }
}

/// R10 / R12 / R13 / R15 —— `GranularityExpansion`：把一次变化摊成队列记录。
///
/// 三条活的规则：
/// - **significant 元素变了 → 整块重画，块内 significant 后代的模型行被逐个抹掉。**
///   "块级颗粒"的确切含义不是"重画子树里每一个"，是"重画块，顺手删掉块内那些自己也曾
///   单独成块的行"。
/// - **既不 significant 又不 primitive → 什么都不做**，连 `AncestorDeletes` 都不做。
///   我们生产路径的 `Unknown → Regen` 保守兜底在 core 里没有对应物；那是我们有意多做。
/// - **primitive 元素变了 → 上卷到 `SignificantOwner` 重画**，删除除外（删除 push 自己）。
///
/// 块内成员的 `AncestorDeletes` / `AbsentPrimitives` 收的是**原始 state**，不是成员被
/// push 进去的那个 3/4。
pub fn granularity_expansion(
    tree: &impl ElementTree,
    id_list: &impl IdList,
    queue: &mut Vec<Record>,
    elem: RefnoEnum,
    state: ModelState,
) {
    if tree.bits(elem).significant {
        queue.push(Record { elem, state });
        ancestor_deletes(tree, queue, elem, state);
        absent_primitives(tree, id_list, queue, elem, state);

        let member_state = if state == ModelState::Deleted {
            ModelState::Deleted
        } else {
            ModelState::MemberOfChangedSignificant
        };
        for member in members(tree, elem, SearchMode::Significant) {
            queue.push(Record {
                elem: member,
                state: member_state,
            });
            ancestor_deletes(tree, queue, member, state);
            absent_primitives(tree, id_list, queue, member, state);
        }
        return;
    }

    if !tree.bits(elem).primitive {
        return;
    }

    if state == ModelState::Deleted {
        queue.push(Record {
            elem,
            state: ModelState::Deleted,
        });
    } else if let Some(owner) = significant_owner(tree, elem) {
        queue.push(Record { elem: owner, state });
        absent_primitives(tree, id_list, queue, owner, state);
    }

    ancestor_deletes(tree, queue, elem, state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::pdms_types::RefU64;
    use std::collections::HashMap;

    fn r(id: u32) -> RefnoEnum {
        RefU64::from_two_nums(24381, id).into()
    }

    #[derive(Default)]
    struct TestTree {
        owners: HashMap<RefnoEnum, RefnoEnum>,
        children: HashMap<RefnoEnum, Vec<RefnoEnum>>,
        bits: HashMap<RefnoEnum, NounBits>,
    }

    impl TestTree {
        /// `(id, owner, significant, primitive)`；`owner` 为 `None` 表示到顶。
        fn build(rows: &[(u32, Option<u32>, bool, bool)]) -> Self {
            let mut tree = TestTree::default();
            for (id, owner, significant, primitive) in rows {
                let node = r(*id);
                tree.bits.insert(
                    node,
                    NounBits {
                        significant: *significant,
                        primitive: *primitive,
                    },
                );
                if let Some(owner) = owner {
                    tree.owners.insert(node, r(*owner));
                    tree.children.entry(r(*owner)).or_default().push(node);
                }
            }
            tree
        }
    }

    impl ElementTree for TestTree {
        fn owner(&self, elem: RefnoEnum) -> Option<RefnoEnum> {
            self.owners.get(&elem).copied()
        }

        fn members(&self, elem: RefnoEnum) -> Vec<RefnoEnum> {
            self.children.get(&elem).cloned().unwrap_or_default()
        }

        fn bits(&self, elem: RefnoEnum) -> NounBits {
            self.bits.get(&elem).copied().unwrap_or_default()
        }
    }

    struct TestIdList {
        active: bool,
        drawn: HashSet<RefnoEnum>,
    }

    impl TestIdList {
        fn active(ids: &[u32]) -> Self {
            TestIdList {
                active: true,
                drawn: ids.iter().map(|id| r(*id)).collect(),
            }
        }

        /// 什么都画不了的清单——`AbsentPrimitives` 会把所有 primitive 判为缺失（R23）。
        fn inactive() -> Self {
            TestIdList {
                active: false,
                drawn: HashSet::new(),
            }
        }
    }

    impl IdList for TestIdList {
        fn is_active(&self) -> bool {
            self.active
        }

        fn contains(&self, elem: RefnoEnum) -> bool {
            self.drawn.contains(&elem)
        }
    }

    fn sorted(mut refnos: Vec<RefnoEnum>) -> Vec<RefnoEnum> {
        refnos.sort();
        refnos
    }

    fn marked(queue: &[Record], state: ModelState) -> Vec<RefnoEnum> {
        sorted(
            queue
                .iter()
                .filter(|record| record.state == state)
                .map(|record| record.elem)
                .collect(),
        )
    }

    /// C1-4 / R11 —— 非 significant 的子节点**挡住**它下面的 significant 孙节点。
    ///
    /// ```text
    /// A(sig) ├── B(sig)  ── D(sig)
    ///        └── C(!sig) ── E(sig)
    /// ```
    /// `E` 不在结果里：mode 0 对非 significant 成员既不收集也不下潜。把这条写成
    /// "全子树找 significant"，块内成员清理（T3.2）就会多删 E 的行。
    #[test]
    fn c1_4_significant_member_walk_stops_at_non_significant_nodes() {
        let tree = TestTree::build(&[
            (1, None, true, false),
            (2, Some(1), true, false),
            (3, Some(1), false, false),
            (4, Some(2), true, false),
            (5, Some(3), true, false),
        ]);

        assert_eq!(
            sorted(members(&tree, r(1), SearchMode::Significant)),
            vec![r(2), r(4)]
        );
    }

    /// C1-5 / R11 —— mode 1 对所有成员下潜，走整棵子树。与 C1-4 成对：同一棵树，
    /// 差别只在"下潜"那一栏。缺失图元回收（T3.1）用的是这一个。
    #[test]
    fn c1_5_primitive_member_walk_covers_the_whole_subtree() {
        let tree = TestTree::build(&[
            (1, None, true, false),
            (2, Some(1), true, false),
            (3, Some(1), false, false),
            (4, Some(2), false, true),
            (5, Some(3), false, true),
        ]);

        assert_eq!(
            sorted(members(&tree, r(1), SearchMode::Primitive)),
            vec![r(4), r(5)]
        );
    }

    /// C1-7 / R21 —— 祖先标记打满整条链，中间那个两位都假的祖先只被跳过、不终止上行。
    ///
    /// ```text
    /// P(prim) → Q(两位都假) → S(sig) → T(prim) → ROOT
    /// ```
    /// **反例断言在最后一行**：按证据文档第一版（"命中即整条终止"）实现出来是 `{S}`。
    #[test]
    fn c1_7_ancestor_delete_marks_every_qualifying_ancestor_to_the_top() {
        let tree = TestTree::build(&[
            (10, None, false, false),
            (11, Some(10), false, true),
            (12, Some(11), true, false),
            (13, Some(12), false, false),
            (14, Some(13), false, true),
        ]);

        let mut queue = Vec::new();
        ancestor_deletes(&tree, &mut queue, r(14), ModelState::Deleted);

        assert_eq!(
            marked(&queue, ModelState::AncestorDelete),
            vec![r(11), r(12)]
        );
        assert_ne!(
            marked(&queue, ModelState::AncestorDelete),
            vec![r(12)],
            "命中已标记祖先只跳过该级 push，不终止上行"
        );
    }

    /// C1-8 / R21 —— 已标记的祖先不重复入队，但**它上面的一级照样被检查**。
    #[test]
    fn c1_8_already_marked_ancestor_is_skipped_not_terminating() {
        let tree = TestTree::build(&[
            (10, None, false, false),
            (11, Some(10), true, false),
            (12, Some(11), true, false),
            (13, Some(12), false, true),
            (14, Some(12), false, true),
        ]);

        let mut queue = vec![Record {
            elem: r(12),
            state: ModelState::AncestorDelete,
        }];
        ancestor_deletes(&tree, &mut queue, r(13), ModelState::Deleted);

        assert_eq!(
            marked(&queue, ModelState::AncestorDelete),
            vec![r(11), r(12)],
            "12 不重复入队，11 是这一趟新标的"
        );

        // 第二个后代再来一次：两级都已标记，队列不再增长。
        let before = queue.len();
        ancestor_deletes(&tree, &mut queue, r(14), ModelState::Deleted);
        assert_eq!(queue.len(), before);
    }

    /// R21 —— 非删除类状态完全不动作。
    #[test]
    fn ancestor_deletes_only_fires_on_delete_class_states() {
        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), false, true)]);

        for state in [
            ModelState::Changed,
            ModelState::New,
            ModelState::AbsentPrimitive,
        ] {
            let mut queue = Vec::new();
            ancestor_deletes(&tree, &mut queue, r(11), state);
            assert!(queue.is_empty(), "{state:?} 不该打祖先标记");
        }
    }

    /// C1-9 / R17–R19 —— 三个 state 三套判法。同一个队列、同一个元素，三种答案。
    #[test]
    fn c1_9_is_pending_uses_a_different_rule_per_state() {
        // NOZZ(11) 是 primitive，挂在 significant 的 EQUI(10) 下。
        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), false, true)]);
        let queue = vec![Record {
            elem: r(10),
            state: ModelState::Changed,
        }];

        assert!(
            is_pending(&tree, &queue, r(11), ModelState::Changed),
            "Changed 沿 owner 链找到 EQUI 的 Changed"
        );
        assert!(
            !is_pending(&tree, &queue, r(11), ModelState::New),
            "New 只找 New，队列里没有"
        );
        assert!(
            !is_pending(&tree, &queue, r(11), ModelState::Deleted),
            "Deleted 完全不上行，只看 NOZZ 自己的 3/4"
        );
    }

    /// R17 —— `Changed` 的后两步：子孙排着 New，以及自己被祖先删除标记打过。
    #[test]
    fn changed_also_yields_to_a_queued_descendant_new_and_to_an_ancestor_delete_mark() {
        let tree = TestTree::build(&[
            (10, None, true, false),
            (11, Some(10), true, false),
            (12, Some(11), false, true),
        ]);

        let descendant_new = vec![Record {
            elem: r(11),
            state: ModelState::New,
        }];
        assert!(is_pending(
            &tree,
            &descendant_new,
            r(10),
            ModelState::Changed
        ));

        let ancestor_mark = vec![Record {
            elem: r(10),
            state: ModelState::AncestorDelete,
        }];
        assert!(is_pending(
            &tree,
            &ancestor_mark,
            r(10),
            ModelState::Changed
        ));
        assert!(
            !is_pending(&tree, &ancestor_mark, r(10), ModelState::New),
            "只有 Changed 看 state 2"
        );
    }

    /// C1-10 / R20 —— 去重键先归一化到块：非 primitive 的元素按它的 `SignificantOwner` 判。
    ///
    /// 两条路径殊途同归（都返回真），但**中间量不同**——所以断言键本身。
    #[test]
    fn c1_10_dedup_key_normalises_non_primitives_to_their_significant_owner() {
        let tree = TestTree::build(&[
            (10, None, true, false),
            (11, Some(10), false, false),
            (12, Some(10), false, true),
        ]);
        let queue = vec![Record {
            elem: r(10),
            state: ModelState::Changed,
        }];

        assert_eq!(significant_owner(&tree, r(11)), Some(r(10)));
        assert!(is_pending(&tree, &queue, r(11), ModelState::Changed));

        // primitive 的那个键是自己，但沿链上行同样命中。
        assert!(tree.bits(r(12)).primitive);
        assert!(is_pending(&tree, &queue, r(12), ModelState::Changed));
    }

    /// R14 —— 含自身、无深度上限；攀到顶返回 `None`。
    #[test]
    fn significant_owner_includes_self_and_has_no_depth_limit() {
        let mut rows = vec![(0u32, None, false, false)];
        rows.extend((1..=64u32).map(|id| (id, Some(id - 1), false, true)));
        let deep = TestTree::build(&rows);
        assert_eq!(
            significant_owner(&deep, r(64)),
            None,
            "整条链都不 significant，攀到顶"
        );

        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), true, true)]);
        assert_eq!(significant_owner(&tree, r(11)), Some(r(11)), "从自己开始判");
    }

    /// R10 —— significant 变化：自己入队，块内 significant 后代拿 state 4。
    #[test]
    fn granularity_expansion_pushes_the_block_and_erases_inner_significant_members() {
        let tree = TestTree::build(&[
            (10, None, true, false),
            (11, Some(10), true, false),
            (12, Some(11), false, true),
        ]);
        let id_list = TestIdList::active(&[12]);

        let mut queue = Vec::new();
        granularity_expansion(&tree, &id_list, &mut queue, r(10), ModelState::Changed);

        assert_eq!(
            queue,
            vec![
                Record {
                    elem: r(10),
                    state: ModelState::Changed
                },
                Record {
                    elem: r(11),
                    state: ModelState::MemberOfChangedSignificant
                },
            ]
        );
    }

    /// R10 —— 删除时块内成员拿的是 3 而不是 4。
    #[test]
    fn deleting_a_significant_block_pushes_members_as_deleted() {
        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), true, false)]);
        let id_list = TestIdList::active(&[]);

        let mut queue = Vec::new();
        granularity_expansion(&tree, &id_list, &mut queue, r(10), ModelState::Deleted);

        assert_eq!(marked(&queue, ModelState::Deleted), vec![r(10), r(11)]);
        assert!(marked(&queue, ModelState::MemberOfChangedSignificant).is_empty());
    }

    /// C1-6 / R13 —— 两个位都假：core 什么都不做，连祖先标记都不打。
    ///
    /// 我们生产路径在这里是 `Unknown → Regen` 的保守兜底。这个测试钉的是"我们知道
    /// core 会丢，而我们有意多做"——哪天要改成丢弃，它就是那次改动的入口。
    #[test]
    fn c1_6_core_discards_elements_that_are_neither_significant_nor_primitive() {
        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), false, false)]);
        let id_list = TestIdList::active(&[]);

        for state in [ModelState::Changed, ModelState::New, ModelState::Deleted] {
            let mut queue = Vec::new();
            granularity_expansion(&tree, &id_list, &mut queue, r(11), state);
            assert!(queue.is_empty(), "{state:?} 也该被丢弃");
        }
    }

    /// R12 / R15 / R21 —— primitive 非删除上卷到块，删除则 push 自己**并顺带标记它的块**。
    ///
    /// 删除那一支容易漏掉尾部的 `AncestorDeletes`：`(11, Deleted)` 之后 10 会拿到一条
    /// state-2 标记，正是它让后续同块的删除被 `IsPending` 吸收、并让图元擦除切到
    /// `FromCandidateModel`（R24）。
    #[test]
    fn primitive_rolls_up_to_its_block_except_on_delete() {
        let tree = TestTree::build(&[(10, None, true, false), (11, Some(10), false, true)]);
        let id_list = TestIdList::active(&[11]);

        let mut changed = Vec::new();
        granularity_expansion(&tree, &id_list, &mut changed, r(11), ModelState::Changed);
        assert_eq!(
            changed,
            vec![Record {
                elem: r(10),
                state: ModelState::Changed
            }],
            "非删除不打祖先标记，只上卷"
        );

        let mut deleted = Vec::new();
        granularity_expansion(&tree, &id_list, &mut deleted, r(11), ModelState::Deleted);
        assert_eq!(
            deleted,
            vec![
                Record {
                    elem: r(11),
                    state: ModelState::Deleted
                },
                Record {
                    elem: r(10),
                    state: ModelState::AncestorDelete
                },
            ]
        );
    }

    /// R22 —— 只有不在 ID 清单里的图元被挑出来。
    #[test]
    fn absent_primitives_only_reclaims_what_the_id_list_no_longer_has() {
        let tree = TestTree::build(&[
            (10, None, true, false),
            (11, Some(10), false, true),
            (12, Some(10), false, true),
        ]);
        let id_list = TestIdList::active(&[11]);

        let mut queue = Vec::new();
        absent_primitives(&tree, &id_list, &mut queue, r(10), ModelState::Changed);
        assert_eq!(marked(&queue, ModelState::AbsentPrimitive), vec![r(12)]);

        // 删除类状态不动作。
        let mut on_delete = Vec::new();
        absent_primitives(&tree, &id_list, &mut on_delete, r(10), ModelState::Deleted);
        assert!(on_delete.is_empty());
    }

    /// R23 —— ID 清单不活跃时**整棵子树的 primitive 全判为缺失**。
    ///
    /// 这是 core 的实际行为，不是我们想要的行为：pass 2 刚把这个块画完，pass 3 就把
    /// 里头的图元逐个擦掉。移植 T3.1 之前必须在 live 进程上确认 `PDMS_Idlist2 +0x18`
    /// 到底什么时候为假（用例 C3-4）。这个测试的作用是**让这条边界有名有姓**，
    /// 而不是等它在生产里出现。
    #[test]
    fn c3_4_inactive_id_list_marks_every_primitive_absent() {
        let tree = TestTree::build(&[
            (10, None, true, false),
            (11, Some(10), false, true),
            (12, Some(10), false, true),
        ]);

        let mut queue = Vec::new();
        absent_primitives(
            &tree,
            &TestIdList::inactive(),
            &mut queue,
            r(10),
            ModelState::Changed,
        );
        assert_eq!(
            marked(&queue, ModelState::AbsentPrimitive),
            vec![r(11), r(12)]
        );
    }
}
