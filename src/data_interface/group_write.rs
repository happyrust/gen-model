//! GPWL / GPSET 分组的写侧编排（M5 / G4 的 gen-model 一侧）：描述要写什么、写前校验，不碰库。
//!
//! 落点是 e3d-io 的分组写入口（`insert_gpwl` / `insert_gpset` / `insert_gpset_member` /
//! `set_gpset_rules`，见 `docs/plans/2026-11-08-gpwl-gpset-m5-write-side.md` §2），由后续的适配函数
//! 接上；这里只用 [`GroupWriteOp`] 描述意图，用 [`validate_ops`] 在写之前对着 MDB 现状把关。
//! 拒绝原因与读侧 [`UnresolvedReason`](super::group_membership::UnresolvedReason) 同一套口径：
//! 放过的批次写下去，读侧不会多出坏槽。

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use aios_core::RefU64;

use super::direct_store::DirectStoreError;
use super::direct_tree::TreeStore;
use super::group_membership::{
    GROUP_ITEM_NOUN, GROUP_SET_NOUN, GROUP_WORLD_NOUN, ResolvedMember, is_group_noun,
    resolved_members_on,
};

/// 一条待写的分组成员变更。落点由 e3d-io 的 Transaction 执行，这里只描述意图。
///
/// `gpwl` / `gpset` 只能指库里已有的元素：本批里新建的 GPWL / GPSET 写之前还没有 refno。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupWriteOp {
    /// WORL 下建 GPWL。
    CreateWorld { name: String },
    /// GPWL 下建 GPSET。
    CreateSet { gpwl: RefU64, name: String },
    /// 建 GPITEM + 写 GPREF。
    AddMember { gpset: RefU64, target: RefU64 },
    /// 写 SCOSEL。
    SetRules { gpset: RefU64, scosel: Vec<i32> },
}

/// 写前校验拒掉一条变更的原因；`target_missing` / `db_not_in_mdb` 与读侧坏槽同名同义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// `AddMember` 的 target 库在、元素不在（含空引用）：写下去就是读侧的 `target_missing` 坏槽。
    TargetMissing,
    /// target 的库不在本 MDB（或没有文件）：写下去就是读侧的 `db_not_in_mdb` 坏槽。
    DbNotInMdb,
    /// `CreateSet` 的 gpwl 不是 GPWL，`AddMember` / `SetRules` 的 gpset 不是 GPSET（含不存在）。
    NotAGroup,
    /// 同一 GPSET 下同一 target 第二次出现，或已在现有成员里：读侧去重只会留一个。
    DuplicateMember,
    /// target 本身是 GPWL / GPSET / GPITEM：分组不收分组容器当显式成员。
    NotAnElement,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetMissing => "target_missing",
            Self::DbNotInMdb => "db_not_in_mdb",
            Self::NotAGroup => "not_a_group",
            Self::DuplicateMember => "duplicate_member",
            Self::NotAnElement => "not_an_element",
        }
    }
}

/// 被拒的一条；`index` 是它在 `ops` 里的下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejection {
    pub index: usize,
    pub reason: RejectReason,
}

/// [`validate_ops`] 的结论：要么整批可以交给 e3d-io，要么逐条列出原因、整批不写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedBatch {
    /// 全部通过，按原顺序交给 e3d-io。
    Ready(Vec<GroupWriteOp>),
    /// 至少一条没过；没被拒的几条也不写，免得只写进去一半。
    Rejected(Vec<Rejection>),
}

/// 写前校验：目标存在、同组不重复、分组 noun 对得上。返回可以安全交给 e3d-io 的批次，
/// 或逐条原因。校验不过的不写——悬空 GPREF 会让读侧落到 target_missing 坏槽。
///
/// 每条只报第一道没过的检查，顺序是：分组 noun → target 的库 → target 存在 → target noun → 重复。
/// 重复按 GPSET 现有的解析成员（口径同读侧 `resolved_members_on`）加上本批前面已放过的
/// `AddMember` 算。读库出错（不是「库不在 / 元素不在」这两种）不是拒绝原因，照读侧整体上抛。
pub fn validate_ops(store: &dyn TreeStore, ops: &[GroupWriteOp]) -> anyhow::Result<ValidatedBatch> {
    let mut members = HashMap::new();
    let mut rejected = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let reason = match op {
            GroupWriteOp::CreateWorld { .. } => None,
            GroupWriteOp::CreateSet { gpwl, .. } => {
                container_rejection(store, *gpwl, GROUP_WORLD_NOUN)?
            }
            GroupWriteOp::SetRules { gpset, .. } => {
                container_rejection(store, *gpset, GROUP_SET_NOUN)?
            }
            GroupWriteOp::AddMember { gpset, target } => {
                member_rejection(store, &mut members, *gpset, *target)?
            }
        };
        if let Some(reason) = reason {
            rejected.push(Rejection { index, reason });
        }
    }
    Ok(if rejected.is_empty() {
        ValidatedBatch::Ready(ops.to_vec())
    } else {
        ValidatedBatch::Rejected(rejected)
    })
}

/// `container` 得是 `noun` 这种分组；不存在、库不在 MDB 也算不是。
fn container_rejection(
    store: &dyn TreeStore,
    container: RefU64,
    noun: &str,
) -> anyhow::Result<Option<RejectReason>> {
    Ok(match probe_noun(store, container)? {
        Ok(found) if found == noun => None,
        _ => Some(RejectReason::NotAGroup),
    })
}

fn member_rejection(
    store: &dyn TreeStore,
    members: &mut HashMap<RefU64, HashSet<RefU64>>,
    gpset: RefU64,
    target: RefU64,
) -> anyhow::Result<Option<RejectReason>> {
    if let Some(reason) = container_rejection(store, gpset, GROUP_SET_NOUN)? {
        return Ok(Some(reason));
    }
    let noun = match probe_noun(store, target)? {
        Ok(noun) => noun,
        Err(reason) => return Ok(Some(reason)),
    };
    if is_group_noun(&noun) || noun == GROUP_ITEM_NOUN {
        return Ok(Some(RejectReason::NotAnElement));
    }
    let seen = match members.entry(gpset) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => entry.insert(
            resolved_members_on(store, gpset)?
                .members
                .iter()
                .map(ResolvedMember::element)
                .collect(),
        ),
    };
    Ok((!seen.insert(target)).then_some(RejectReason::DuplicateMember))
}

/// `refno` 的 noun（大写）。库不在 MDB、元素不在（含空引用）按读侧坏槽的口径归成原因，其余读错上抛。
fn probe_noun(
    store: &dyn TreeStore,
    refno: RefU64,
) -> anyhow::Result<Result<String, RejectReason>> {
    if refno.0 == 0 {
        return Ok(Err(RejectReason::TargetMissing));
    }
    match store.named_attmap(refno) {
        Ok(attrs) => Ok(Ok(attrs
            .get_as_string("TYPE")
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase())),
        Err(DirectStoreError::DbNotInMdb { .. } | DirectStoreError::NoFileForDbnum { .. }) => {
            Ok(Err(RejectReason::DbNotInMdb))
        }
        Err(DirectStoreError::NoSuchElement { .. } | DirectStoreError::InvalidRefNo { .. }) => {
            Ok(Err(RejectReason::TargetMissing))
        }
        Err(other) => Err(anyhow::Error::from(other).context(format!("读 {refno} 的 TYPE 失败"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::group_membership::UnresolvedReason;
    use crate::data_interface::group_membership::fake_store::FakeStore;
    use crate::data_interface::scosel::{Scosel, encode};

    fn r(high: u32, low: u32) -> RefU64 {
        RefU64::from_two_nums(high, low)
    }

    /// 设计库 1：WORL 1/1 ─ SITE 1/2 ─ ZONE 1/3 ─ BRAN 1/4、EQUI 1/5。
    /// 分组库 2：WORL 2/1 ─ GPWL 2/10 ─ GPSET 2/11（GPITEM 2/12 → ZONE 1/3）、GPSET 2/14（空）。
    /// 库 9 不在 MDB。
    fn sample() -> FakeStore {
        let mut store = FakeStore::default();
        store.add(r(1, 1), "WORL", &[r(1, 2)]);
        store.add(r(1, 2), "SITE", &[r(1, 3)]);
        store.add(r(1, 3), "ZONE", &[r(1, 4), r(1, 5)]);
        store.add(r(1, 4), "BRAN", &[]);
        store.add(r(1, 5), "EQUI", &[]);
        store.add(r(2, 1), "WORL", &[r(2, 10)]);
        store.add(r(2, 10), "GPWL", &[r(2, 11), r(2, 14)]);
        store.add(r(2, 11), "GPSET", &[r(2, 12)]);
        store.item(r(2, 12), Some(r(1, 3)));
        store.add(r(2, 14), "GPSET", &[]);
        store.foreign.insert(9);
        store
    }

    fn add(gpset: RefU64, target: RefU64) -> GroupWriteOp {
        GroupWriteOp::AddMember { gpset, target }
    }

    fn rules(gpset: RefU64) -> GroupWriteOp {
        GroupWriteOp::SetRules {
            gpset,
            scosel: encode(&Scosel::default()),
        }
    }

    /// 被拒的那几条：（下标，原因码）。整批放过就直接判失败。
    fn rejected(ops: &[GroupWriteOp]) -> Vec<(usize, &'static str)> {
        match validate_ops(&sample(), ops).unwrap() {
            ValidatedBatch::Rejected(rejections) => rejections
                .iter()
                .map(|rejection| (rejection.index, rejection.reason.as_str()))
                .collect(),
            ValidatedBatch::Ready(_) => panic!("整批都放过了，预期有被拒的"),
        }
    }

    #[test]
    fn a_batch_that_passes_every_check_is_handed_over_whole_and_in_order() {
        let ops = vec![
            GroupWriteOp::CreateWorld {
                name: "/GPWL-NEW".into(),
            },
            GroupWriteOp::CreateSet {
                gpwl: r(2, 10),
                name: "/GPSET-NEW".into(),
            },
            add(r(2, 11), r(1, 4)),
            add(r(2, 11), r(1, 5)),
            add(r(2, 14), r(1, 3)),
            rules(r(2, 14)),
        ];
        assert_eq!(
            validate_ops(&sample(), &ops).unwrap(),
            ValidatedBatch::Ready(ops)
        );
    }

    #[test]
    fn a_target_whose_db_is_in_the_mdb_but_element_is_not_is_target_missing() {
        assert_eq!(
            rejected(&[add(r(2, 11), r(1, 99)), add(r(2, 11), r(0, 0))]),
            [(0, "target_missing"), (1, "target_missing")]
        );
    }

    #[test]
    fn a_target_in_a_db_outside_the_mdb_is_db_not_in_mdb() {
        assert_eq!(rejected(&[add(r(2, 11), r(9, 1))]), [(0, "db_not_in_mdb")]);
    }

    #[test]
    fn sets_go_only_under_a_gpwl_and_members_or_rules_only_into_a_gpset() {
        let ops = [
            GroupWriteOp::CreateSet {
                gpwl: r(2, 11),
                name: "/UNDER-A-GPSET".into(),
            },
            GroupWriteOp::CreateSet {
                gpwl: r(1, 1),
                name: "/UNDER-A-WORL".into(),
            },
            add(r(2, 10), r(1, 4)),
            add(r(2, 99), r(1, 4)),
            add(r(2, 10), r(9, 1)),
            rules(r(1, 3)),
        ];
        assert_eq!(
            rejected(&ops),
            [
                (0, "not_a_group"),
                (1, "not_a_group"),
                (2, "not_a_group"),
                (3, "not_a_group"),
                (4, "not_a_group"),
                (5, "not_a_group"),
            ]
        );
    }

    #[test]
    fn a_target_already_in_the_set_or_repeated_in_the_batch_is_a_duplicate() {
        let ops = [
            // GPITEM 2/12 已经收了 ZONE 1/3
            add(r(2, 11), r(1, 3)),
            add(r(2, 11), r(1, 4)),
            add(r(2, 11), r(1, 4)),
            // 换一个 GPSET 不算重复
            add(r(2, 14), r(1, 4)),
        ];
        assert_eq!(
            rejected(&ops),
            [(0, "duplicate_member"), (2, "duplicate_member")]
        );
    }

    #[test]
    fn group_containers_cannot_be_explicit_members() {
        let ops = [
            add(r(2, 14), r(2, 10)),
            add(r(2, 14), r(2, 11)),
            add(r(2, 14), r(2, 12)),
            add(r(2, 14), r(2, 14)),
        ];
        assert_eq!(
            rejected(&ops),
            [
                (0, "not_an_element"),
                (1, "not_an_element"),
                (2, "not_an_element"),
                (3, "not_an_element"),
            ]
        );
    }

    #[test]
    fn the_shared_codes_mean_what_the_read_side_bad_slots_mean() {
        assert_eq!(
            RejectReason::TargetMissing.as_str(),
            UnresolvedReason::TargetMissing.as_str()
        );
        assert_eq!(
            RejectReason::DbNotInMdb.as_str(),
            UnresolvedReason::DbNotInMdb.as_str()
        );
    }
}
