//! GPWL / GPSET 分组的写侧编排（M5 / G4 的 gen-model 一侧）：描述要写什么、写前校验、按库拆成执行
//! 阶段，不碰库。
//!
//! 落点是 e3d-io 的分组写入口（`insert_gpwl` / `insert_gpset` / `insert_gpset_member` /
//! `set_gpset_rules`，见 `docs/plans/2026-11-08-gpwl-gpset-m5-write-side.md` §2、§8），由后续的执行器
//! 接上。先全量校验、再写库：[`validate_ops`] 有一条不过就整批不写；放行的批次经 [`plan_stages`]
//! 按库拆成 [`Stage`]。拒绝原因与读侧 [`UnresolvedReason`](super::group_membership::UnresolvedReason)
//! 同一套口径：放过的批次写下去，读侧不会多出坏槽。

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use aios_core::RefU64;

use super::direct_store::DirectStoreError;
use super::direct_tree::TreeStore;
use super::group_membership::{
    GROUP_ITEM_NOUN, GROUP_SET_NOUN, GROUP_WORLD_NOUN, ResolvedMember, is_group_noun,
    resolved_members_on,
};
use super::refno_route::refno_dbnum;

/// 每个库的根；GPWL 建在它下面。
const WORLD_NOUN: &str = "WORL";

/// 容器：库里已有的元素，或本批第几条 `CreateWorld` / `CreateSet` 的产物。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerRef {
    Existing(RefU64),
    /// `ops[index]` 必须是排在前面的 `CreateWorld` 或 `CreateSet`；由 [`validate_ops`] 与
    /// [`plan_stages`] 核对。
    InBatch(usize),
}

/// 成员在 GPSET 成员表里的位置，与 e3d-io 的 `MemberPosition` 一一对应；`After` / `Before` 的锚点
/// 是成员表里的 GPITEM，不是 target。默认追加到末尾，与 e3d-io `insert` 的落点一致。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MemberPosition {
    #[default]
    Last,
    First,
    After(RefU64),
    Before(RefU64),
}

/// 一条待写的分组成员变更。落点由 e3d-io 的 Transaction 执行，这里只描述意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupWriteOp {
    /// `world`（WORL）下建 GPWL。
    CreateWorld { world: RefU64, name: String },
    /// GPWL 下建 GPSET；GPSET 落在它的 GPWL 所在的库。
    CreateSet { gpwl: ContainerRef, name: String },
    /// 建 GPITEM + 写 GPREF，按 `at` 放进成员表。`target` 必须是库里已有的设计元素。
    AddMember {
        gpset: ContainerRef,
        target: RefU64,
        at: MemberPosition,
    },
    /// 写 SCOSEL。e3d-io 的 `set_gpset_rules` 经 `RecordDraft::set_int_array_hunk` 原样写成 type-20
    /// 二进制 hunk：没有文本中转，也没有 0x3FFF 的计数上限；不要改走收文本的 `Transaction::set_hunk`。
    /// **空 `Vec` 写的是 len=0 的 hunk，不是删属性**——读侧「SCOSEL 是空数组」与「没有 SCOSEL」
    /// 是两件事，清规则要走删属性，不在这条路上。
    SetRules {
        gpset: ContainerRef,
        scosel: Vec<i32>,
    },
}

/// 写前校验拒掉一条变更的原因；`target_missing` / `db_not_in_mdb` 与读侧坏槽同名同义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// `AddMember` 的 target 库在、元素不在（含空引用）：写下去就是读侧的 `target_missing` 坏槽。
    TargetMissing,
    /// target 的库不在本 MDB（或没有文件）：写下去就是读侧的 `db_not_in_mdb` 坏槽。
    DbNotInMdb,
    /// 容器种类不对：`CreateWorld` 的 world 不是 WORL，`CreateSet` 的 gpwl 不是 GPWL，`AddMember` /
    /// `SetRules` 的 gpset 不是 GPSET。已有容器不存在或库不在 MDB、`InBatch` 指的不是排在前面的
    /// 对应那种 `Create*`，都算。
    NotAGroup,
    /// 同一 GPSET 下同一 target 在本批里第二次出现。已在现有成员里的不算，见
    /// [`SkipReason::AlreadyMember`]。
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

/// 放行、但执行时可以跳过的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// target 已是这个 GPSET 的解析成员（口径同读侧 `resolved_members_on`）。e3d-io 的
    /// `insert_gpset_member` 对它只回已有的 GPITEM，重跑同一批变更不会失败。
    AlreadyMember,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyMember => "already_member",
        }
    }
}

/// 放行但可以跳过的一条；`index` 是它在 `ops` 里的下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    pub index: usize,
    pub reason: SkipReason,
}

/// [`validate_ops`] 的结论：要么整批放行，要么逐条列出原因、整批不写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedBatch {
    /// 全部通过：`ops` 原样、原顺序（下标不变，`InBatch` 仍然对得上）；`skipped` 是执行时可以
    /// 跳过的几条。
    Ready {
        ops: Vec<GroupWriteOp>,
        skipped: Vec<Skipped>,
    },
    /// 至少一条没过：一条都不写。
    Rejected(Vec<Rejection>),
}

/// 写前校验：容器种类对得上、目标存在、同组不重复。返回整批放行（带可以跳过的标记），或逐条原因。
/// 校验不过的不写——悬空 GPREF 会让读侧落到 target_missing 坏槽。
///
/// 在写库之前全量做完：`ContainerRef::Existing` 与 target 读库查；`ContainerRef::InBatch` 只核对下标
/// 指向的那条种类——它的产物由 `insert_gpwl` / `insert_gpset` 建出，noun 必然对，不必等它建完。
/// 每条只报第一道没过的检查：容器 → target 的库 → target 存在 → target noun → 本批内重复。新组
/// （`InBatch` 指向的 `CreateSet`）本来是空的，只查本批内重复；已有的组再对一遍现有解析成员，
/// target 已在其中的放行并标成跳过。读库出错（不是「库不在 / 元素不在」）不是拒绝原因，照读侧
/// 整体上抛。
pub fn validate_ops(store: &dyn TreeStore, ops: &[GroupWriteOp]) -> anyhow::Result<ValidatedBatch> {
    let mut stored_members = HashMap::new();
    let mut added = HashSet::new();
    let mut skipped = Vec::new();
    let mut rejected = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let verdict = match op {
            GroupWriteOp::CreateWorld { world, .. } => container_verdict(
                store,
                ops,
                index,
                ContainerRef::Existing(*world),
                WORLD_NOUN,
            )?,
            GroupWriteOp::CreateSet { gpwl, .. } => {
                container_verdict(store, ops, index, *gpwl, GROUP_WORLD_NOUN)?
            }
            GroupWriteOp::SetRules { gpset, .. } => {
                container_verdict(store, ops, index, *gpset, GROUP_SET_NOUN)?
            }
            GroupWriteOp::AddMember { gpset, target, .. } => member_verdict(
                store,
                ops,
                index,
                &mut stored_members,
                &mut added,
                *gpset,
                *target,
            )?,
        };
        match verdict {
            Verdict::Pass => {}
            Verdict::Skip(reason) => skipped.push(Skipped { index, reason }),
            Verdict::Reject(reason) => rejected.push(Rejection { index, reason }),
        }
    }
    Ok(if rejected.is_empty() {
        ValidatedBatch::Ready {
            ops: ops.to_vec(),
            skipped,
        }
    } else {
        ValidatedBatch::Rejected(rejected)
    })
}

enum Verdict {
    Pass,
    Skip(SkipReason),
    Reject(RejectReason),
}

fn container_verdict(
    store: &dyn TreeStore,
    ops: &[GroupWriteOp],
    index: usize,
    container: ContainerRef,
    noun: &str,
) -> anyhow::Result<Verdict> {
    Ok(if container_is(store, ops, index, container, noun)? {
        Verdict::Pass
    } else {
        Verdict::Reject(RejectReason::NotAGroup)
    })
}

fn member_verdict(
    store: &dyn TreeStore,
    ops: &[GroupWriteOp],
    index: usize,
    stored_members: &mut HashMap<RefU64, HashSet<RefU64>>,
    added: &mut HashSet<(ContainerRef, RefU64)>,
    gpset: ContainerRef,
    target: RefU64,
) -> anyhow::Result<Verdict> {
    if !container_is(store, ops, index, gpset, GROUP_SET_NOUN)? {
        return Ok(Verdict::Reject(RejectReason::NotAGroup));
    }
    let noun = match probe_noun(store, target)? {
        Ok(noun) => noun,
        Err(reason) => return Ok(Verdict::Reject(reason)),
    };
    if is_group_noun(&noun) || noun == GROUP_ITEM_NOUN {
        return Ok(Verdict::Reject(RejectReason::NotAnElement));
    }
    if !added.insert((gpset, target)) {
        return Ok(Verdict::Reject(RejectReason::DuplicateMember));
    }
    let ContainerRef::Existing(gpset) = gpset else {
        return Ok(Verdict::Pass);
    };
    let members = match stored_members.entry(gpset) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => entry.insert(
            resolved_members_on(store, gpset)?
                .members
                .iter()
                .map(ResolvedMember::element)
                .collect(),
        ),
    };
    Ok(if members.contains(&target) {
        Verdict::Skip(SkipReason::AlreadyMember)
    } else {
        Verdict::Pass
    })
}

/// `container` 是不是 `noun` 这种容器：已有的读库核对，本批产物只核对下标指向的那条。
fn container_is(
    store: &dyn TreeStore,
    ops: &[GroupWriteOp],
    index: usize,
    container: ContainerRef,
    noun: &str,
) -> anyhow::Result<bool> {
    Ok(match container {
        ContainerRef::Existing(refno) => probe_noun(store, refno)?.is_ok_and(|found| found == noun),
        ContainerRef::InBatch(k) => produced_noun(ops, index, k) == Some(noun),
    })
}

/// `ops[index]` 经 `InBatch(k)` 引用的那条会建出什么；`k` 不在 `index` 前面、或那条不建容器，给 `None`。
fn produced_noun(ops: &[GroupWriteOp], index: usize, k: usize) -> Option<&'static str> {
    match ops.get(k) {
        Some(GroupWriteOp::CreateWorld { .. }) if k < index => Some(GROUP_WORLD_NOUN),
        Some(GroupWriteOp::CreateSet { .. }) if k < index => Some(GROUP_SET_NOUN),
        _ => None,
    }
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

/// 一个库的写入计划。执行器一个库开一个 `Transaction`：按序跑 `creates`、记下每条建出来的 refno，
/// 再跑 `fills`（[`Slot::Created`] 用记下的 refno 回填），最后提交一次。
///
/// 跨库没有两阶段提交：各 `Stage` 按顺序逐库提交，某个库失败就停下、不再提交后面的库，回报已提交的
/// 库号、失败的库号与错误，已提交的不回滚。所以「整批不写」只在 [`validate_ops`] 这一步和同一个库
/// 里成立，跨库可能停在中间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub dbnum: u32,
    pub creates: Vec<Create>,
    pub fills: Vec<Fill>,
}

/// 第一段：建容器。`index` 是它在原批次里的下标，执行器回报结果时用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Create {
    World {
        index: usize,
        world: RefU64,
        name: String,
    },
    Set {
        index: usize,
        gpwl: Slot,
        name: String,
    },
}

/// 第二段：往 GPSET 里写成员或规则。`index` 同 [`Create`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    AddMember {
        index: usize,
        gpset: Slot,
        target: RefU64,
        at: MemberPosition,
    },
    SetRules {
        index: usize,
        gpset: Slot,
        scosel: Vec<i32>,
    },
}

/// 执行时的容器：库里已有的，或本 [`Stage`] 前面第几个 `creates` 的产物（执行器回填）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Existing(RefU64),
    Created(usize),
}

/// 把放行的批次按库拆成 [`Stage`]：库按它在批次里第一次出现的顺序排，库内保持批次原序。库号由
/// refno 位解码（[`refno_dbnum`]）：已有容器取它自己的库，`CreateWorld` 取 world 的库，本批建的容器
/// 取它父容器的库。子节点总落在父节点的库里，`CreateSet` 没有另指库的字段，跨库建子节点在这个
/// 形状里表达不出来，所以每个 `Stage` 自给自足。
///
/// 纯函数，不碰库。输入应当已经过 [`validate_ops`]：`ValidatedBatch::Ready` 里标成跳过的 `AddMember`
/// 也照常进 `fills`（e3d-io 对它回 `Existing`），执行器也可以按 `index` 直接跳过。`InBatch` 指错、
/// 或容器 refno 解不出库号时，回第一条出错的 [`Rejection`]（`not_a_group`）。
pub fn plan_stages(ops: &[GroupWriteOp]) -> Result<Vec<Stage>, Rejection> {
    let mut stages = Vec::new();
    // 本批建容器的那几条：落在第几个 Stage、是它的第几个 create
    let mut created = HashMap::new();
    for (index, op) in ops.iter().enumerate() {
        match op {
            GroupWriteOp::CreateWorld { world, name } => {
                let stage = stage_of(&mut stages, index, *world)?;
                created.insert(index, (stage, stages[stage].creates.len()));
                stages[stage].creates.push(Create::World {
                    index,
                    world: *world,
                    name: name.clone(),
                });
            }
            GroupWriteOp::CreateSet { gpwl, name } => {
                let (stage, gpwl) =
                    slot_of(&mut stages, &created, ops, index, *gpwl, GROUP_WORLD_NOUN)?;
                created.insert(index, (stage, stages[stage].creates.len()));
                stages[stage].creates.push(Create::Set {
                    index,
                    gpwl,
                    name: name.clone(),
                });
            }
            GroupWriteOp::AddMember { gpset, target, at } => {
                let (stage, gpset) =
                    slot_of(&mut stages, &created, ops, index, *gpset, GROUP_SET_NOUN)?;
                stages[stage].fills.push(Fill::AddMember {
                    index,
                    gpset,
                    target: *target,
                    at: *at,
                });
            }
            GroupWriteOp::SetRules { gpset, scosel } => {
                let (stage, gpset) =
                    slot_of(&mut stages, &created, ops, index, *gpset, GROUP_SET_NOUN)?;
                stages[stage].fills.push(Fill::SetRules {
                    index,
                    gpset,
                    scosel: scosel.clone(),
                });
            }
        }
    }
    Ok(stages)
}

/// `container` 落在哪个 Stage、执行时怎么拿到它。
fn slot_of(
    stages: &mut Vec<Stage>,
    created: &HashMap<usize, (usize, usize)>,
    ops: &[GroupWriteOp],
    index: usize,
    container: ContainerRef,
    noun: &str,
) -> Result<(usize, Slot), Rejection> {
    match container {
        ContainerRef::Existing(refno) => {
            Ok((stage_of(stages, index, refno)?, Slot::Existing(refno)))
        }
        ContainerRef::InBatch(k) => match created.get(&k) {
            Some(&(stage, create)) if produced_noun(ops, index, k) == Some(noun) => {
                Ok((stage, Slot::Created(create)))
            }
            _ => Err(Rejection {
                index,
                reason: RejectReason::NotAGroup,
            }),
        },
    }
}

/// `refno` 所在库的 Stage 下标，还没有就在末尾开一个。
fn stage_of(stages: &mut Vec<Stage>, index: usize, refno: RefU64) -> Result<usize, Rejection> {
    let dbnum = refno_dbnum(refno).ok_or(Rejection {
        index,
        reason: RejectReason::NotAGroup,
    })?;
    if let Some(at) = stages.iter().position(|stage| stage.dbnum == dbnum) {
        return Ok(at);
    }
    stages.push(Stage {
        dbnum,
        creates: Vec::new(),
        fills: Vec::new(),
    });
    Ok(stages.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::ContainerRef::{Existing, InBatch};
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

    fn world(world: RefU64, name: &str) -> GroupWriteOp {
        GroupWriteOp::CreateWorld {
            world,
            name: name.into(),
        }
    }

    fn set(gpwl: ContainerRef, name: &str) -> GroupWriteOp {
        GroupWriteOp::CreateSet {
            gpwl,
            name: name.into(),
        }
    }

    fn add(gpset: ContainerRef, target: RefU64) -> GroupWriteOp {
        GroupWriteOp::AddMember {
            gpset,
            target,
            at: MemberPosition::default(),
        }
    }

    fn rules(gpset: ContainerRef) -> GroupWriteOp {
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
            ValidatedBatch::Ready { .. } => panic!("整批都放过了，预期有被拒的"),
        }
    }

    #[test]
    fn a_batch_that_passes_every_check_is_handed_over_whole_and_in_order() {
        let ops = vec![
            world(r(2, 1), "/GPWL-NEW"),
            set(InBatch(0), "/GPSET-NEW"),
            add(InBatch(1), r(1, 3)),
            set(Existing(r(2, 10)), "/GPSET-RULES"),
            rules(InBatch(3)),
            add(Existing(r(2, 11)), r(1, 4)),
            GroupWriteOp::AddMember {
                gpset: Existing(r(2, 14)),
                target: r(1, 5),
                at: MemberPosition::First,
            },
        ];
        assert_eq!(
            validate_ops(&sample(), &ops).unwrap(),
            ValidatedBatch::Ready {
                ops,
                skipped: vec![]
            }
        );
    }

    #[test]
    fn a_target_whose_db_is_in_the_mdb_but_element_is_not_is_target_missing() {
        assert_eq!(
            rejected(&[
                add(Existing(r(2, 11)), r(1, 99)),
                add(Existing(r(2, 11)), r(0, 0))
            ]),
            [(0, "target_missing"), (1, "target_missing")]
        );
    }

    #[test]
    fn a_target_in_a_db_outside_the_mdb_is_db_not_in_mdb() {
        assert_eq!(
            rejected(&[add(Existing(r(2, 11)), r(9, 1))]),
            [(0, "db_not_in_mdb")]
        );
    }

    #[test]
    fn each_op_needs_its_own_kind_of_existing_container() {
        let ops = [
            world(r(2, 10), "/ON-A-GPWL"),
            world(r(1, 99), "/ON-NOTHING"),
            set(Existing(r(2, 11)), "/UNDER-A-GPSET"),
            set(Existing(r(1, 1)), "/UNDER-A-WORL"),
            add(Existing(r(2, 10)), r(1, 4)),
            add(Existing(r(2, 99)), r(1, 4)),
            // 容器先于 target 判
            add(Existing(r(2, 10)), r(9, 1)),
            rules(Existing(r(1, 3))),
        ];
        assert_eq!(
            rejected(&ops),
            (0..ops.len())
                .map(|index| (index, "not_a_group"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_in_batch_container_must_be_an_earlier_creation_of_the_right_kind() {
        let ops = [
            world(r(2, 1), "/W"),
            set(InBatch(0), "/S"),
            add(InBatch(0), r(1, 4)),
            set(InBatch(1), "/S-UNDER-S"),
            rules(InBatch(2)),
            add(InBatch(6), r(1, 4)),
            set(InBatch(6), "/S-SELF"),
            add(InBatch(99), r(1, 4)),
        ];
        assert_eq!(
            rejected(&ops),
            [2, 3, 4, 5, 6, 7].map(|index| (index, "not_a_group"))
        );
        assert_eq!(
            plan_stages(&ops),
            Err(Rejection {
                index: 2,
                reason: RejectReason::NotAGroup
            })
        );
    }

    #[test]
    fn a_target_repeated_under_one_set_in_the_batch_is_a_duplicate() {
        let ops = [
            add(Existing(r(2, 11)), r(1, 4)),
            add(Existing(r(2, 11)), r(1, 4)),
            add(Existing(r(2, 14)), r(1, 4)),
            add(Existing(r(2, 11)), r(1, 3)),
            add(Existing(r(2, 11)), r(1, 3)),
        ];
        assert_eq!(
            rejected(&ops),
            [(1, "duplicate_member"), (4, "duplicate_member")]
        );
    }

    #[test]
    fn a_target_already_in_an_existing_set_is_let_through_and_marked_skipped() {
        // GPITEM 2/12 已经收了 ZONE 1/3
        let ops = vec![
            add(Existing(r(2, 11)), r(1, 3)),
            add(Existing(r(2, 11)), r(1, 4)),
        ];
        let skipped = vec![Skipped {
            index: 0,
            reason: SkipReason::AlreadyMember,
        }];
        assert_eq!(
            validate_ops(&sample(), &ops).unwrap(),
            ValidatedBatch::Ready { ops, skipped }
        );
        assert_eq!(SkipReason::AlreadyMember.as_str(), "already_member");
    }

    /// 已在组里的 target 在同一批里写两遍：第一条标跳过、第二条按本批内重复拒掉，整批不写。
    /// 不改成「两条都跳过」——同批写两遍是调用方的错，静默放过会把问题藏到读侧。
    #[test]
    fn a_repeat_of_an_already_present_target_still_counts_as_a_batch_duplicate() {
        let ops = [
            add(Existing(r(2, 11)), r(1, 3)),
            add(Existing(r(2, 11)), r(1, 3)),
        ];
        assert_eq!(rejected(&ops), [(1, "duplicate_member")]);
    }

    #[test]
    fn members_of_a_set_created_in_the_batch_are_only_checked_against_the_batch() {
        let ops = [
            set(Existing(r(2, 10)), "/S"),
            add(InBatch(0), r(1, 3)),
            add(InBatch(0), r(1, 3)),
            add(InBatch(0), r(1, 99)),
        ];
        assert_eq!(
            rejected(&ops),
            [(2, "duplicate_member"), (3, "target_missing")]
        );
    }

    #[test]
    fn group_containers_cannot_be_explicit_members() {
        let ops = [
            add(Existing(r(2, 14)), r(2, 10)),
            add(Existing(r(2, 14)), r(2, 11)),
            add(Existing(r(2, 14)), r(2, 12)),
            add(Existing(r(2, 14)), r(2, 14)),
        ];
        assert_eq!(
            rejected(&ops),
            [0, 1, 2, 3].map(|index| (index, "not_an_element"))
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

    #[test]
    fn containers_are_created_first_and_back_filled_into_the_fills_that_follow() {
        let ops = [
            world(r(2, 1), "/GPWL-NEW"),
            set(InBatch(0), "/GPSET-ITEMS"),
            add(InBatch(1), r(1, 3)),
            set(InBatch(0), "/GPSET-RULES"),
            rules(InBatch(3)),
            add(Existing(r(2, 11)), r(1, 4)),
        ];
        assert_eq!(
            plan_stages(&ops),
            Ok(vec![Stage {
                dbnum: 2,
                creates: vec![
                    Create::World {
                        index: 0,
                        world: r(2, 1),
                        name: "/GPWL-NEW".into(),
                    },
                    Create::Set {
                        index: 1,
                        gpwl: Slot::Created(0),
                        name: "/GPSET-ITEMS".into(),
                    },
                    Create::Set {
                        index: 3,
                        gpwl: Slot::Created(0),
                        name: "/GPSET-RULES".into(),
                    },
                ],
                fills: vec![
                    Fill::AddMember {
                        index: 2,
                        gpset: Slot::Created(1),
                        target: r(1, 3),
                        at: MemberPosition::Last,
                    },
                    Fill::SetRules {
                        index: 4,
                        gpset: Slot::Created(2),
                        scosel: encode(&Scosel::default()),
                    },
                    Fill::AddMember {
                        index: 5,
                        gpset: Slot::Existing(r(2, 11)),
                        target: r(1, 4),
                        at: MemberPosition::Last,
                    },
                ],
            }])
        );
    }

    #[test]
    fn every_db_gets_one_stage_in_the_order_it_first_appears() {
        let ops = [
            add(Existing(r(3, 11)), r(1, 4)),
            set(Existing(r(2, 10)), "/S"),
            add(InBatch(1), r(1, 5)),
            rules(Existing(r(3, 11))),
        ];
        assert_eq!(
            plan_stages(&ops),
            Ok(vec![
                Stage {
                    dbnum: 3,
                    creates: vec![],
                    fills: vec![
                        Fill::AddMember {
                            index: 0,
                            gpset: Slot::Existing(r(3, 11)),
                            target: r(1, 4),
                            at: MemberPosition::Last,
                        },
                        Fill::SetRules {
                            index: 3,
                            gpset: Slot::Existing(r(3, 11)),
                            scosel: encode(&Scosel::default()),
                        },
                    ],
                },
                Stage {
                    dbnum: 2,
                    creates: vec![Create::Set {
                        index: 1,
                        gpwl: Slot::Existing(r(2, 10)),
                        name: "/S".into(),
                    }],
                    fills: vec![Fill::AddMember {
                        index: 2,
                        gpset: Slot::Created(0),
                        target: r(1, 5),
                        at: MemberPosition::Last,
                    }],
                },
            ])
        );
    }
}
