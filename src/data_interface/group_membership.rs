//! GPWL / GPSET 分组的二级成员解析（E3D secondary hierarchy 的读侧）。
//!
//! 存储视图与解析视图分层：GPSET 成员链上挂的是 GPITEM 元素记录，真身在 GPITEM 的
//! `GPREF`（可跨库）；GPWL 的成员就是 GPSET。这里只产出解析结果，不改任何存储视图的
//! 口径——`children` / `subtree_refnos` / 共用的生成根遍历照旧按存储成员走。
//! 取证见 `docs/plans/2026-11-08-gpwl-gpset-group-nodes.md` §0，接口契约见同目录 `-dev-plan.md` §2。

use std::collections::{HashMap, HashSet};

use aios_core::RefU64;

use super::direct_store::DirectStoreError;
use super::direct_tree::TreeStore;

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
}

impl ResolvedMember {
    pub fn element(&self) -> RefU64 {
        match *self {
            Self::Owned(element) => element,
            Self::Reference { target, .. } => target,
        }
    }

    pub fn via(&self) -> Option<RefU64> {
        match *self {
            Self::Owned(_) => None,
            Self::Reference { via, .. } => Some(via),
        }
    }
}

/// 解析不出真身的 GPITEM 槽为什么解析不出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// GPITEM 没有 `GPREF`，或是空引用。
    GprefMissing,
    /// `GPREF` 指向的库不在本 MDB（或没有文件）。
    DbNotInMdb,
    /// 库在，元素不在。
    TargetMissing,
}

impl UnresolvedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GprefMissing => "gpref_missing",
            Self::DbNotInMdb => "db_not_in_mdb",
            Self::TargetMissing => "target_missing",
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
    pub fn build(store: &dyn TreeStore, worlds: &[RefU64]) -> anyhow::Result<Self> {
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
            for member in resolved_members_on(store, group)?.members {
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
    let mut scope = GroupScope::default();
    let mut expanded = HashSet::from([group]);
    let mut seen = HashSet::new();
    let top = resolved_members_on(store, group)?;
    scope.unresolved.extend(top.unresolved);
    let mut stack = vec![top.members.into_iter()];
    while let Some(level) = stack.last_mut() {
        let Some(member) = level.next() else {
            stack.pop();
            continue;
        };
        let element = member.element();
        if is_group_noun(&noun_of(store, element)?) {
            if expanded.insert(element) {
                let nested = resolved_members_on(store, element)?;
                scope.unresolved.extend(nested.unresolved);
                stack.push(nested.members.into_iter());
            }
        } else if seen.insert(element) {
            scope.members.push(element);
        }
    }
    Ok(scope)
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

    use super::GROUP_ITEM_NOUN;
    use crate::data_interface::direct_store::DirectStoreError;
    use crate::data_interface::direct_tree::TreeStore;

    #[derive(Default)]
    pub(crate) struct FakeStore {
        elements: HashMap<RefU64, (&'static str, Vec<RefU64>, Option<RefU64>)>,
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
}
