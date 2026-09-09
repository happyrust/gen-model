//! 文件侧的房间结构触发——ADR-010 D12 / ADR-040 §4 那三条规则在 direct 窗口规划器上的落点。
//!
//! 房间归属有两类触发源：几何（AABB 变更集 / 定向重写目标，随 e3d 发布事务排
//! `RoomRecalc*`）与**结构**——房间节点改名、面板换属主。后者不改任何几何，几何路
//! 永远点不着它，所以要单独看账本。规则原样照搬老 PE 规划器
//! （`model_update_plan::collect_room_structural_triggers`）：
//!
//! 1. `FRMW` / `SBFR` 的 NAME 变了，且**新旧任一**命中 `room_key_word` → 名下全部 PANE
//!    （子 + 孙两层，与房间拓扑同口径）整间重算；改进房间与改出房间都要算，关键字未配置不触发。
//! 2. `PANE` 换了属主 → 它自己整间重算（新旧两间房都经它的整间分支收敛）。
//! 3. `project_hd` 下 `CWALL` / `CFLOOR` 换了属主 → 其直接 PANE 子元素整间重算。
//!
//! 与老规划器的差别只在数据源：老规划器读 SurrealDB 的 `pe` / `pe_owner`，这里全部读
//! S / T 两端的 `DbSet`（名字、属主、成员）——direct 模式下 `pe` 可能根本没有这一窗的行
//! （读透形态压根不摄入）。账本只做零 I/O 粗筛：没有房间类 noun 变更的窗口一次文件读都
//! 不发生。面板枚举读不到就记进 `unreadable`，规划器据此把 `room_build:main` 标成
//! `rebuild_required`（ADR-040 §5 的持久兜底），不掐窗口。
//!
//! 没有开关：这是恢复一条已接受的保证（2026-09-07 审核 F-A，共识 d-272），不是新能力。

use std::collections::BTreeSet;
use std::sync::Arc;

use aios_core::RefnoEnum;
use e3d_io::db_element::DbSet;
use e3d_model::increment::UpdatePlan;
use e3d_model::ledger::ChangeKind;

use crate::data_interface::generation_root::{
    SubtreeElement, refno_from_e3d, subtree_element_from_set,
};
use crate::fast_model::room_topology::{RoomHierarchy, collect_room_groups};

/// 账本里一条房间类 noun 的变更，已经把两端读好，剩下的判定是纯函数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoomStructuralChange {
    /// 房间节点（FRMW / SBFR）的记录变了；两端 NAME 原文（读不到的一端为 `None`）。
    RoomRecordChanged {
        room: RefnoEnum,
        old_name: Option<String>,
        new_name: Option<String>,
    },
    /// PANE 换了属主。
    PanelReparented { panel: RefnoEnum },
    /// CWALL / CFLOOR 换了属主（只在 `project_hd` 语义下有意义）。
    ContainerReparented { container: RefnoEnum },
}

/// 一窗的房间结构触发：分类结果 + 折出的面板目标 + 账。挂在 `WindowRootSources` 上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoomStructuralTriggers {
    /// 账本里落在房间类 noun 上、且是「修改」一族的条目数（粗筛命中数，零 I/O 得到）。
    pub candidates: usize,
    /// 规则 1 命中的房间节点。
    pub renamed_rooms: BTreeSet<RefnoEnum>,
    /// 规则 2 命中的面板。
    pub moved_panels: BTreeSet<RefnoEnum>,
    /// 规则 3 命中的容器。
    pub moved_panel_containers: BTreeSet<RefnoEnum>,
    /// 最终要排 `RoomRecalcPanel` 的面板（三条规则的并集，去重）。
    pub panel_targets: BTreeSet<RefnoEnum>,
    /// 读两端 / 枚举面板时挂掉的地方。非空 = 目标集不完备，规划器要标全量重建。
    pub unreadable: Vec<String>,
}

fn normalized(noun: &str) -> String {
    noun.trim().to_ascii_uppercase()
}

fn is_room_noun(noun: &str) -> bool {
    matches!(normalized(noun).as_str(), "FRMW" | "SBFR")
}

fn is_panel_container_noun(noun: &str) -> bool {
    if cfg!(feature = "project_hd") {
        matches!(normalized(noun).as_str(), "CWALL" | "CFLOOR")
    } else {
        false
    }
}

/// 名字命中关键字：与 `room_topology::room_panel_groups` / SQL 侧 `'{kw}' in NAME` 同口径，
/// 空关键字不匹配任何名字。
fn name_hits_keyword(name: &str, keywords: &[String]) -> bool {
    keywords
        .iter()
        .any(|keyword| !keyword.is_empty() && name.contains(keyword.as_str()))
}

/// I/O：从账本挑房间类 noun 的变更，房间节点补读两端 NAME。
///
/// 只看「修改」一族（`AttributesModified` / `OpaqueRecordChange` / `Reparented`）：新建的房间
/// 与面板会经几何生成 → 几何触发；删除走 `DeleteCleanup` 的房间边清理（ADR-010 §4 删除例外）。
/// `TypeChanged` 两端不是同一个 noun，按删后重建处置，也不在这里。
pub(crate) fn read_room_structural_changes(
    plan: &UpdatePlan,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
) -> (Vec<RoomStructuralChange>, usize, Vec<String>) {
    let mut changes = Vec::new();
    let mut candidates = 0usize;
    let mut unreadable = Vec::new();
    let mut seen_rooms = BTreeSet::new();
    let mut seen_reparented = BTreeSet::new();
    for entry in plan.ledger.entries() {
        let noun = normalized(&entry.noun);
        let refno = refno_from_e3d(entry.refno);
        match entry.kind {
            ChangeKind::AttributesModified | ChangeKind::OpaqueRecordChange
                if is_room_noun(&noun) =>
            {
                candidates += 1;
                if !seen_rooms.insert(refno) {
                    continue;
                }
                let mut read = |set: &Arc<DbSet>, side: &str| -> Option<String> {
                    match subtree_element_from_set(set, refno) {
                        Ok(element) => Some(element.name),
                        Err(error) => {
                            unreadable
                                .push(format!("读 {refno} 在 {side} 端的 NAME 失败: {error:#}"));
                            None
                        }
                    }
                };
                let old_name = read(base, "S");
                let new_name = read(target, "T");
                changes.push(RoomStructuralChange::RoomRecordChanged {
                    room: refno,
                    old_name,
                    new_name,
                });
            }
            ChangeKind::Reparented if noun == "PANE" => {
                candidates += 1;
                if seen_reparented.insert(refno) {
                    changes.push(RoomStructuralChange::PanelReparented { panel: refno });
                }
            }
            ChangeKind::Reparented if is_panel_container_noun(&noun) => {
                candidates += 1;
                if seen_reparented.insert(refno) {
                    changes.push(RoomStructuralChange::ContainerReparented { container: refno });
                }
            }
            _ => {}
        }
    }
    (changes, candidates, unreadable)
}

impl RoomStructuralTriggers {
    /// 纯函数：三条规则的分类。`lookup` 只在枚举面板时被调（读 T 端），
    /// 生产传 [`subtree_element_from_set`] 的 T 端闭包，测试可以手搓一张表。
    pub(crate) fn classify(
        changes: &[RoomStructuralChange],
        candidates: usize,
        room_key_words: &[String],
        hierarchy: Option<RoomHierarchy>,
        mut lookup: impl FnMut(RefnoEnum) -> anyhow::Result<SubtreeElement>,
    ) -> Self {
        let mut out = Self {
            candidates,
            ..Self::default()
        };
        for change in changes {
            match change {
                RoomStructuralChange::RoomRecordChanged {
                    room,
                    old_name,
                    new_name,
                } => {
                    // 名字没变（别的属性变了）就不是房间语义的变更；一端读不到按「变了」
                    // 保守处理——那一端的名字是不是房间名判不了，宁多算不漏算。
                    let name_changed = match (old_name, new_name) {
                        (Some(old), Some(new)) => old != new,
                        _ => true,
                    };
                    let hits = old_name
                        .as_deref()
                        .is_some_and(|name| name_hits_keyword(name, room_key_words))
                        || new_name
                            .as_deref()
                            .is_some_and(|name| name_hits_keyword(name, room_key_words));
                    if name_changed && hits {
                        out.renamed_rooms.insert(*room);
                    }
                }
                RoomStructuralChange::PanelReparented { panel } => {
                    out.moved_panels.insert(*panel);
                }
                RoomStructuralChange::ContainerReparented { container } => {
                    out.moved_panel_containers.insert(*container);
                }
            }
        }

        out.panel_targets.extend(out.moved_panels.iter().copied());

        // 规则 1 的面板枚举：与房间拓扑同一段遍历（子 + 孙 / 直接子，按层级口径）。
        if !out.renamed_rooms.is_empty() {
            match hierarchy {
                Some(hierarchy) => {
                    let rooms: Vec<RefnoEnum> = out.renamed_rooms.iter().copied().collect();
                    match collect_room_groups(&rooms, hierarchy, &mut lookup) {
                        Ok(groups) => {
                            for group in groups
                                .into_iter()
                                .filter(|group| out.renamed_rooms.contains(&group.room))
                            {
                                out.panel_targets.extend(group.panels);
                            }
                        }
                        Err(error) => out
                            .unreadable
                            .push(format!("房间改名的面板枚举失败: {error:#}")),
                    }
                }
                None => out.unreadable.push(
                    "房间改名命中，但本构建没有房间层级口径（project_hd / project_hh 都没开）"
                        .into(),
                ),
            }
        }

        // 规则 3 的面板枚举：容器的直接 PANE 子元素。
        for container in out.moved_panel_containers.clone() {
            let members = match lookup(container) {
                Ok(element) => element.members,
                Err(error) => {
                    out.unreadable.push(format!(
                        "CWALL/CFLOOR {container} 搬迁的 PANE 枚举失败: {error:#}"
                    ));
                    continue;
                }
            };
            for member in members {
                match lookup(member) {
                    Ok(child) if normalized(&child.noun) == "PANE" => {
                        out.panel_targets.insert(member);
                    }
                    Ok(_) => {}
                    Err(error) => out
                        .unreadable
                        .push(format!("读 {container} 的成员 {member} 失败: {error:#}")),
                }
            }
        }
        out
    }

    /// I/O 版：读账本与两端，再按 T 端枚举面板。
    pub(crate) fn collect(
        plan: &UpdatePlan,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        room_key_words: &[String],
    ) -> Self {
        let (changes, candidates, unreadable) = read_room_structural_changes(plan, base, target);
        let mut out = Self::classify(
            &changes,
            candidates,
            room_key_words,
            RoomHierarchy::configured(),
            |refno| subtree_element_from_set(target, refno),
        );
        out.unreadable.extend(unreadable);
        out
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.panel_targets.is_empty() && self.unreadable.is_empty()
    }

    /// 回执里的一行观测。`candidates > 0` 而 `panel_targets = 0` 就是护栏要看的那一格。
    pub(crate) fn summary_line(&self, dbnum: u32) -> String {
        let mut line = format!(
            "dbnum={dbnum}: room structural triggers — candidates={} renamed_rooms={} \
             moved_panels={} moved_panel_containers={} panel_targets={} unreadable={}",
            self.candidates,
            self.renamed_rooms.len(),
            self.moved_panels.len(),
            self.moved_panel_containers.len(),
            self.panel_targets.len(),
            self.unreadable.len()
        );
        if self.candidates > 0 && self.panel_targets.is_empty() && self.unreadable.is_empty() {
            line.push_str(
                "; 房间类 noun 有变更却一块面板目标都没折出来（名字没变 / 关键字未命中 / 名下无 PANE），请核对",
            );
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn refno(word1: u32) -> RefnoEnum {
        RefnoEnum::from(format!("24384/{word1}").as_str())
    }

    fn element(noun: &str, name: &str, members: &[RefnoEnum]) -> SubtreeElement {
        SubtreeElement {
            noun: noun.to_string(),
            name: name.to_string(),
            members: members.to_vec(),
        }
    }

    /// FRMW(1) → CWALL(2) → PANE(3), PANE(4)；FRMW(1) → PANE(5)（直接子）；
    /// CFLOOR(6) → PANE(7)；ZONE(8) → EQUI(9)。
    fn tree() -> HashMap<RefnoEnum, SubtreeElement> {
        HashMap::from([
            (
                refno(1),
                element("FRMW", "/1RX-RM03-R301", &[refno(2), refno(5)]),
            ),
            (refno(2), element("CWALL", "/W1", &[refno(3), refno(4)])),
            (refno(3), element("PANE", "/P3", &[])),
            (refno(4), element("PANE", "/P4", &[])),
            (refno(5), element("PANE", "/P5", &[])),
            (refno(6), element("CFLOOR", "/F1", &[refno(7)])),
            (refno(7), element("PANE", "/P7", &[])),
            (refno(8), element("ZONE", "/Z", &[refno(9)])),
            (refno(9), element("EQUI", "/E", &[])),
        ])
    }

    fn lookup(
        tree: &HashMap<RefnoEnum, SubtreeElement>,
    ) -> impl FnMut(RefnoEnum) -> anyhow::Result<SubtreeElement> + '_ {
        move |refno| {
            tree.get(&refno)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("{refno} 不在夹具里"))
        }
    }

    fn keywords() -> Vec<String> {
        vec!["-RM".to_string()]
    }

    #[test]
    fn renaming_into_or_out_of_a_room_name_recalculates_every_panel_beneath() {
        let tree = tree();
        // 改进房间：普通框架名 → 房间名。
        let into = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/1RX-FRAME".into()),
                new_name: Some("/1RX-RM03-R301".into()),
            }],
            1,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert_eq!(into.renamed_rooms, BTreeSet::from([refno(1)]));
        assert_eq!(
            into.panel_targets,
            BTreeSet::from([refno(3), refno(4), refno(5)]),
            "子 + 孙两层的 PANE 都要进目标集"
        );
        assert!(into.unreadable.is_empty());

        // 改出房间：房间名 → 普通框架名，旧边该清，同样要算。
        let out = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/1RX-RM03-R301".into()),
                new_name: Some("/1RX-FRAME".into()),
            }],
            1,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert_eq!(out.panel_targets, into.panel_targets);
    }

    #[test]
    fn a_room_record_change_without_a_name_change_or_keyword_hit_is_not_a_trigger() {
        let tree = tree();
        // 名字没变（别的属性变了）。
        let same = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/1RX-RM03-R301".into()),
                new_name: Some("/1RX-RM03-R301".into()),
            }],
            1,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert!(same.renamed_rooms.is_empty() && same.panel_targets.is_empty());
        assert!(
            same.summary_line(8000).contains("请核对"),
            "{}",
            same.summary_line(8000)
        );

        // 名字变了但两头都不是房间名。
        let plain = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/A".into()),
                new_name: Some("/B".into()),
            }],
            1,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert!(plain.panel_targets.is_empty());

        // 关键字未配置：判不了房间性，不触发（与老规划器同口径）。
        let no_keyword = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/A".into()),
                new_name: Some("/1RX-RM03-R301".into()),
            }],
            1,
            &[],
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert!(no_keyword.panel_targets.is_empty());
        let empty_keyword = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/A".into()),
                new_name: Some("/1RX-RM03-R301".into()),
            }],
            1,
            &[String::new()],
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert!(
            empty_keyword.panel_targets.is_empty(),
            "空关键字不匹配任何名字"
        );
    }

    #[test]
    fn an_unreadable_side_is_treated_as_changed_not_ignored() {
        let tree = tree();
        let out = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: None,
                new_name: Some("/1RX-RM03-R301".into()),
            }],
            1,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert_eq!(
            out.panel_targets,
            BTreeSet::from([refno(3), refno(4), refno(5)])
        );
    }

    #[test]
    fn a_reparented_panel_targets_itself_and_a_reparented_container_targets_its_direct_panes() {
        let tree = tree();
        let out = RoomStructuralTriggers::classify(
            &[
                RoomStructuralChange::PanelReparented { panel: refno(3) },
                RoomStructuralChange::ContainerReparented {
                    container: refno(6),
                },
                // 同一块面板重复上报只算一次。
                RoomStructuralChange::PanelReparented { panel: refno(3) },
            ],
            3,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert_eq!(out.moved_panels, BTreeSet::from([refno(3)]));
        assert_eq!(out.moved_panel_containers, BTreeSet::from([refno(6)]));
        assert_eq!(out.panel_targets, BTreeSet::from([refno(3), refno(7)]));
        assert!(out.unreadable.is_empty());
    }

    #[test]
    fn enumeration_failures_are_recorded_instead_of_silently_shrinking_the_target_set() {
        let tree = tree();
        let out = RoomStructuralTriggers::classify(
            &[
                RoomStructuralChange::RoomRecordChanged {
                    room: refno(42),
                    old_name: Some("/A-RM".into()),
                    new_name: Some("/B-RM".into()),
                },
                RoomStructuralChange::ContainerReparented {
                    container: refno(43),
                },
                RoomStructuralChange::PanelReparented { panel: refno(3) },
            ],
            3,
            &keywords(),
            Some(RoomHierarchy::Hd),
            lookup(&tree),
        );
        assert_eq!(
            out.panel_targets,
            BTreeSet::from([refno(3)]),
            "读得到的那条照常进"
        );
        assert_eq!(out.unreadable.len(), 2, "{:?}", out.unreadable);
        assert!(!out.is_empty());
    }

    #[test]
    fn no_hierarchy_means_no_room_subsystem_and_is_reported_not_swallowed() {
        let tree = tree();
        let out = RoomStructuralTriggers::classify(
            &[RoomStructuralChange::RoomRecordChanged {
                room: refno(1),
                old_name: Some("/A".into()),
                new_name: Some("/A-RM".into()),
            }],
            1,
            &keywords(),
            None,
            lookup(&tree),
        );
        assert!(out.panel_targets.is_empty());
        assert_eq!(out.unreadable.len(), 1);
    }

    #[test]
    fn a_window_without_room_class_changes_is_empty_and_quiet() {
        let out = RoomStructuralTriggers::classify(
            &[],
            0,
            &keywords(),
            Some(RoomHierarchy::Hd),
            |refno| anyhow::bail!("不该读 {refno}"),
        );
        assert!(out.is_empty());
        assert!(!out.summary_line(8000).contains("请核对"));
    }
}
