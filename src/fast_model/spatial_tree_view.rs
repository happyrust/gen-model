//! 房间层级树（plant3d-web ADR 0068；计划
//! `plant3d-web/docs/plans/2026-09-20-spatial-room-hierarchy-tree-plan.md` §4.1 / §4.2）：
//!
//! - `GET /api/v1/spatial/nearby/tree`（[`crate::fast_model::spatial_query::nearby_tree`]）：同 `nearby` 的一次查询，
//!   `rooms=` 必填，命中不分页、按 refno 去重后折成 **房间 → 专业 → 最小交付单元类型 → 单元 → 构件**，
//!   不属于任何单元的构件归专业下的「其他构件」按 noun 分；
//! - `GET /api/v1/spatial/rooms/{refno}/tree`（[`room_tree_params`]）：以房间自身的包围盒为范围、`rooms=` 它自己的同一棵树，
//!   给模型树 ROOM 标签页浏览一间房。
//!
//! 两层新信息都在服务端补：**构件属于哪几间所选房间**来自 `rooms=` 解算时顺手保留的 `RoomScope::member_rooms`
//! （ADR 0067 的边本来就有 `面板`，只是此前没留）；**构件属于哪个最小交付单元**沿属主链找首个
//! `delivery_unit_types` 里的 noun（[`unit_root_of_shared`]），逐级记忆同 `spatial_discipline`——链上每个走过的元素都记下答案，
//! 换代（会话指纹变了）整体清空，锁只罩查表 / 写回、不罩记录读。
//!
//! 计数按 refno 去重：一个构件在一间房下只出现一次；横跨多间所选房间的构件在每间房下都出现（Q3，任一归属），
//! `total_count` 只算一次，叶子上带 `shared_rooms`。叶子随树内联（refno / noun / 距离，不带名字），叶子放置数超过
//! [`DEFAULT_LEAF_CAP`] 时整体省略、客户端按 `unit=` / `other_noun=` 只取一组（[`LeafSelector`]）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use aios_core::RefU64;
use serde::Serialize;

use crate::data_interface::generation_root::{is_coarse_hierarchy_noun, is_delivery_unit_noun};
use crate::fast_model::spatial_discipline::Lineage;
use crate::fast_model::spatial_query::{
    CANDIDATE_CAP, COVERAGE, CenterJson, ElementAttributes, Filtered, MAX_RADIUS_MM,
    RawSpatialQuery, Scan, SortBy, SpatialQueryParams, filter_candidates, parse_f32,
    response_warnings, sort_hits,
};
use crate::fast_model::spatial_rooms::{RoomScope, RoomStatusJson};

/// 叶子放置数（跨房构件每间房各算一次）的内联上限；超过就省略全部 `elements`。
pub const DEFAULT_LEAF_CAP: usize = 5000;

/// 属主链最多爬多少级（与 `spatial_discipline` 同）。
const MAX_DEPTH: usize = 4096;

// ---------------------------------------------------------------------------
// 最小交付单元根
// ---------------------------------------------------------------------------

/// 一个构件所属的最小交付单元：属主链上（含自身）首个 noun 在 `delivery_unit_types` 里的元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRoot {
    pub refno: RefU64,
    /// 大写。
    pub noun: String,
    /// 单元的 `NAME`，没有为空串。
    pub name: String,
}

/// 沿 OWNER 往上找单元（或到顶 / 到 SITE·ZONE·WORL / 读不出 / 成环 / 超深）。`memoised` 是「这一级有没有现成答案」的查表，
/// 命中就截断；回 `(这次走过、还没有答案的路径, 单元)`，路径上的元素都该记成这个答案。
fn walk_to_unit(
    lineage: &dyn Lineage,
    unit_types: &[String],
    refno: RefU64,
    mut memoised: impl FnMut(RefU64) -> Option<Option<UnitRoot>>,
) -> (Vec<RefU64>, Option<UnitRoot>) {
    let mut path: Vec<RefU64> = Vec::new();
    let mut current = refno;
    let answer = loop {
        if let Some(known) = memoised(current) {
            break known;
        }
        if path.len() >= MAX_DEPTH || path.contains(&current) {
            break None;
        }
        path.push(current);
        let Some(step) = lineage.lineage_step(current) else {
            break None;
        };
        if is_delivery_unit_noun(&step.noun, unit_types) {
            break Some(UnitRoot {
                refno: current,
                noun: step.noun.trim().to_ascii_uppercase(),
                name: step.name,
            });
        }
        if is_coarse_hierarchy_noun(&step.noun) {
            break None;
        }
        match step.owner {
            Some(owner) if owner.0 != 0 && owner != current => current = owner,
            _ => break None,
        }
    };
    (path, answer)
}

/// 一棵属主链上的单元记忆：`refno → 单元（或 None = 不属任何单元）`，链上每个走过的元素都记。
#[derive(Debug, Default)]
pub struct UnitMemo {
    values: HashMap<RefU64, Option<UnitRoot>>,
}

impl UnitMemo {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已经记过多少个元素（单测钉「只读一次」用）。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn unit_root_of(
        &mut self,
        lineage: &dyn Lineage,
        unit_types: &[String],
        refno: RefU64,
    ) -> Option<UnitRoot> {
        let (path, answer) = walk_to_unit(lineage, unit_types, refno, |current| {
            self.values.get(&current).cloned()
        });
        for visited in path {
            self.values.insert(visited, answer.clone());
        }
        answer
    }
}

/// 进程内共享的记忆 + 它对应的换代键；换代整体清空。类型表随记忆一起换代。
struct SharedUnitMemo {
    epoch: u64,
    unit_types: Arc<[String]>,
    values: HashMap<RefU64, Option<UnitRoot>>,
}

type SharedSlot = Mutex<Option<SharedUnitMemo>>;

fn shared_slot() -> &'static SharedSlot {
    static SLOT: OnceLock<SharedSlot> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 锁内的一小段：换代检查 + 对记忆做一件纯内存的事。**不在这里读记录。**
fn with_shared<R>(
    slot: &SharedSlot,
    epoch: u64,
    load_types: fn() -> Vec<String>,
    f: impl FnOnce(&mut SharedUnitMemo) -> R,
) -> R {
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let stale = guard.as_ref().is_none_or(|shared| shared.epoch != epoch);
    if stale {
        *guard = Some(SharedUnitMemo {
            epoch,
            unit_types: load_types().into(),
            values: HashMap::new(),
        });
    }
    f(guard
        .as_mut()
        .expect("shared unit memo was just initialised"))
}

fn unit_root_of_in(
    slot: &SharedSlot,
    load_types: fn() -> Vec<String>,
    lineage: &dyn Lineage,
    epoch: u64,
    refno: RefU64,
) -> Option<UnitRoot> {
    let unit_types = with_shared(slot, epoch, load_types, |shared| shared.unit_types.clone());
    let (path, answer) = walk_to_unit(lineage, &unit_types, refno, |current| {
        with_shared(slot, epoch, load_types, |shared| {
            shared.values.get(&current).cloned()
        })
    });
    if !path.is_empty() {
        with_shared(slot, epoch, load_types, |shared| {
            for visited in path {
                shared.values.insert(visited, answer.clone());
            }
        });
    }
    answer
}

/// 生产入口：按 `epoch`（`DirectTreeService` 的会话指纹）取共享记忆答某元素所属的最小交付单元。
/// 类型表 = `configured_delivery_unit_types()`（`/health.delivery_unit_types` 那张）。
pub fn unit_root_of_shared(lineage: &dyn Lineage, epoch: u64, refno: RefU64) -> Option<UnitRoot> {
    unit_root_of_in(
        shared_slot(),
        crate::data_interface::generation_root::configured_delivery_unit_types,
        lineage,
        epoch,
        refno,
    )
}

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

/// 叶子超上限之后「只取这一组」的选择器：`unit=<单元 refno>` 或 `other_noun=<NOUN>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafSelector {
    Unit(RefU64),
    OtherNoun(String),
}

impl LeafSelector {
    /// 两个都没给 = 不选；两个都给 400。
    pub fn parse(unit: Option<&str>, other_noun: Option<&str>) -> Result<Option<Self>, String> {
        let unit = unit.map(str::trim).filter(|s| !s.is_empty());
        let other_noun = other_noun.map(str::trim).filter(|s| !s.is_empty());
        match (unit, other_noun) {
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err("unit 与 other_noun 二选一，不能同时给".to_string()),
            (Some(text), None) => RefU64::from_str(text)
                .ok()
                .filter(RefU64::is_valid)
                .map(|refno| Some(Self::Unit(refno)))
                .ok_or_else(|| format!("unit 格式非法（应为单元 refno，a/b 或 a_b）: {text}")),
            (None, Some(noun)) => Ok(Some(Self::OtherNoun(noun.to_ascii_uppercase()))),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Unit(refno) => format!("unit:{refno}"),
            Self::OtherNoun(noun) => format!("other_noun:{noun}"),
        }
    }
}

/// 树路由的额外选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeOptions {
    pub leaf_cap: usize,
    pub only: Option<LeafSelector>,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            leaf_cap: DEFAULT_LEAF_CAP,
            only: None,
        }
    }
}

/// `rooms/{refno}/tree` 的参数：以房间为目标（`Target::Refno`，它的并集盒就是范围）、`shape=cube`、`rooms=` 它自己、
/// `include_self=false`（房间自己的面板不算成员）。`radius` 在这里是外扩量，缺省 0 = 只要与房间盒相交；`refno` / `x,y,z` /
/// `source_mode` / `rooms` 不接受——范围就是这间房。其余过滤（`nouns / keyword / include_negative / dbnums / spec_values`）透传。
pub fn room_tree_params(room: RefU64, raw: &RawSpatialQuery) -> Result<SpatialQueryParams, String> {
    let given = |value: &Option<String>| value.as_deref().is_some_and(|s| !s.trim().is_empty());
    if given(&raw.refno) || given(&raw.x) || given(&raw.y) || given(&raw.z) {
        return Err("rooms/{refno}/tree 以房间自身为范围，不接受 refno / x,y,z".to_string());
    }
    if given(&raw.rooms) {
        return Err("rooms/{refno}/tree 的房间就是路径里那一间，不接受 rooms=".to_string());
    }
    if given(&raw.source_mode) {
        return Err("rooms/{refno}/tree 不接受 source_mode".to_string());
    }
    let margin = match raw
        .radius
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => 0.0,
        Some(text) => {
            let value = parse_f32("radius", text)?;
            if !(0.0..=MAX_RADIUS_MM).contains(&value) {
                return Err(format!(
                    "radius（房间盒外扩量）必须在 [0, {MAX_RADIUS_MM}] mm 之内（收到 {text}）"
                ));
            }
            value
        }
    };
    let mut adjusted = raw.clone();
    adjusted.refno = Some(room.to_string());
    adjusted.x = None;
    adjusted.y = None;
    adjusted.z = None;
    adjusted.source_mode = None;
    // `parse` 要求 radius > 0；这里先给个占位、解析完再写回真正的外扩量（0 也合法）。
    adjusted.radius = Some("1".to_string());
    adjusted.shape = Some("cube".to_string());
    adjusted.rooms = Some(room.to_string());
    adjusted.include_self = Some("false".to_string());
    let mut params = SpatialQueryParams::parse(&adjusted)?;
    params.radius = margin;
    Ok(params)
}

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeLeaf {
    /// `a_b`。
    pub refno: String,
    /// 大写。
    pub noun: String,
    /// 到中心（点或目标盒）的最小表面距离，mm。
    pub distance: f32,
    /// 只在该构件属于 ≥ 2 间所选房间时出现（Q3）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_rooms: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeUnit {
    pub refno: String,
    pub noun: String,
    /// 单元的 `NAME`；没有为 `null`。
    pub name: Option<String>,
    pub count: usize,
    /// 最近成员的距离。
    pub min_distance: f32,
    /// 叶子省略时不出现（`leaves_inline=false` 且不是 `unit=` 点到的那一个）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<TreeLeaf>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeUnitType {
    pub noun: String,
    pub count: usize,
    pub units: Vec<TreeUnit>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeOtherNoun {
    pub noun: String,
    pub count: usize,
    pub min_distance: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<TreeLeaf>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeOthers {
    pub count: usize,
    /// 按计数降序、同计数按 noun。
    pub by_noun: Vec<TreeOtherNoun>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeSpec {
    pub spec_value: i64,
    pub count: usize,
    /// 按 `delivery_unit_types` 配置表顺序。
    pub unit_types: Vec<TreeUnitType>,
    pub others: TreeOthers,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeRoom {
    pub refno: String,
    pub room_num: String,
    pub name: Option<String>,
    pub count: usize,
    /// `spec_value` 升序（0「其他」在前，同 `sort=spec_distance`）。
    pub specs: Vec<TreeSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeResponse {
    pub center: CenterJson,
    pub radius: f32,
    pub shape: &'static str,
    /// 全树按 refno 去重的构件数（跨房构件只算一次）。
    pub total_count: usize,
    pub candidate_count: usize,
    pub truncated_candidates: bool,
    pub candidate_cap: usize,
    /// `true` = 所有 `elements` 都在；`false` = 省略了（超上限，或只按 `unit=` / `other_noun=` 内联了一组）。
    pub leaves_inline: bool,
    pub leaf_cap: usize,
    /// 叶子放置数（跨房构件每间房各算一次）。
    pub leaf_count: usize,
    /// 只按选择器内联了一组时是哪一组（`unit:a_b` / `other_noun:PANE`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inlined: Option<String>,
    pub delivery_unit_types: Vec<String>,
    /// 按 `room_num`、refno 排。
    pub rooms: Vec<TreeRoom>,
    pub room_status: RoomStatusJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub spatial_state: &'static str,
    pub coverage: &'static str,
}

#[derive(Default)]
struct UnitAcc {
    root: Option<UnitRoot>,
    /// 命中下标，按距离序（命中已排好序、按序压入）。
    members: Vec<usize>,
}

#[derive(Default)]
struct SpecAcc {
    /// 单元类型 noun → 单元 refno → 成员。
    units: BTreeMap<String, BTreeMap<RefU64, UnitAcc>>,
    /// 其他构件：noun → 成员。
    others: BTreeMap<String, Vec<usize>>,
}

/// 锁外的后半程：与 `nearby` 同一条过滤链 → 按距离排、按 refno 去重 → 每条命中解「属于哪几间房」「属于哪个单元」→ 折树。
/// `scope` 是 `rooms=` 解出来的成员集（树路由要求必给）。
pub fn assemble_tree(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    scope: &RoomScope,
    unit_types: &[String],
    options: &TreeOptions,
) -> TreeResponse {
    let candidate_count = scan.candidates.len();
    let Filtered { mut hits, .. } =
        filter_candidates(scan.candidates, params, &scan.center, attrs, Some(scope));
    sort_hits(&mut hits, SortBy::Distance);
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.refno));
    let total_count = hits.len();

    let room_count = scope.rooms.len();
    let all_rooms: Vec<usize> = (0..room_count).collect();
    let mut per_room: Vec<BTreeMap<i64, SpecAcc>> =
        (0..room_count).map(|_| BTreeMap::new()).collect();
    let mut shared = vec![1usize; hits.len()];
    let mut leaf_count = 0usize;
    for (index, hit) in hits.iter().enumerate() {
        let rooms = scope
            .member_rooms
            .get(&hit.refno)
            .map(Vec::as_slice)
            .filter(|rooms| !rooms.is_empty())
            .unwrap_or(&all_rooms);
        shared[index] = rooms.len();
        let unit = attrs.unit_root_of(hit.refno);
        for room_index in rooms {
            let Some(specs) = per_room.get_mut(*room_index) else {
                continue;
            };
            let spec = specs.entry(hit.spec_value).or_default();
            match &unit {
                Some(root) => {
                    let acc = spec
                        .units
                        .entry(root.noun.clone())
                        .or_default()
                        .entry(root.refno)
                        .or_default();
                    if acc.root.is_none() {
                        acc.root = Some(root.clone());
                    }
                    acc.members.push(index);
                }
                None => spec
                    .others
                    .entry(hit.noun.trim().to_ascii_uppercase())
                    .or_default()
                    .push(index),
            }
            leaf_count += 1;
        }
    }

    let inline_all = options.only.is_none() && leaf_count <= options.leaf_cap;
    let inline_for = |selected: bool| inline_all || (options.only.is_some() && selected);
    let leaf = |index: usize| TreeLeaf {
        refno: hits[index].refno.to_string(),
        noun: hits[index].noun.trim().to_ascii_uppercase(),
        distance: hits[index].distance,
        shared_rooms: (shared[index] > 1).then_some(shared[index]),
    };
    let type_rank = |noun: &str| {
        unit_types
            .iter()
            .position(|candidate| candidate == noun)
            .unwrap_or(unit_types.len())
    };

    let mut order: Vec<usize> = (0..room_count).collect();
    order.sort_by(|a, b| {
        scope.rooms[*a]
            .room_num
            .cmp(&scope.rooms[*b].room_num)
            .then_with(|| scope.rooms[*a].refno.cmp(&scope.rooms[*b].refno))
    });
    let mut rooms_out = Vec::with_capacity(room_count);
    for room_index in order {
        let room_ref = &scope.rooms[room_index];
        let mut specs_out = Vec::new();
        let mut room_total = 0usize;
        for (spec_value, acc) in &per_room[room_index] {
            let mut unit_types_out = acc
                .units
                .iter()
                .map(|(noun, units)| {
                    let mut units_out = units
                        .iter()
                        .map(|(refno, unit)| {
                            let selected =
                                matches!(&options.only, Some(LeafSelector::Unit(picked)) if picked == refno);
                            TreeUnit {
                                refno: refno.to_string(),
                                noun: noun.clone(),
                                name: unit
                                    .root
                                    .as_ref()
                                    .map(|root| root.name.clone())
                                    .filter(|name| !name.is_empty()),
                                count: unit.members.len(),
                                min_distance: unit
                                    .members
                                    .first()
                                    .map(|first| hits[*first].distance)
                                    .unwrap_or(0.0),
                                elements: inline_for(selected)
                                    .then(|| unit.members.iter().map(|member| leaf(*member)).collect()),
                            }
                        })
                        .collect::<Vec<_>>();
                    units_out.sort_by(|a, b| {
                        a.min_distance
                            .total_cmp(&b.min_distance)
                            .then_with(|| a.refno.cmp(&b.refno))
                    });
                    TreeUnitType {
                        noun: noun.clone(),
                        count: units_out.iter().map(|unit| unit.count).sum(),
                        units: units_out,
                    }
                })
                .collect::<Vec<_>>();
            unit_types_out.sort_by(|a, b| {
                type_rank(&a.noun)
                    .cmp(&type_rank(&b.noun))
                    .then_with(|| a.noun.cmp(&b.noun))
            });
            let mut by_noun = acc
                .others
                .iter()
                .map(|(noun, members)| {
                    let selected =
                        matches!(&options.only, Some(LeafSelector::OtherNoun(picked)) if picked == noun);
                    TreeOtherNoun {
                        noun: noun.clone(),
                        count: members.len(),
                        min_distance: members.first().map(|first| hits[*first].distance).unwrap_or(0.0),
                        elements: inline_for(selected)
                            .then(|| members.iter().map(|member| leaf(*member)).collect()),
                    }
                })
                .collect::<Vec<_>>();
            by_noun.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.noun.cmp(&b.noun)));
            let others_count = by_noun.iter().map(|group| group.count).sum::<usize>();
            let spec_count = unit_types_out
                .iter()
                .map(|group| group.count)
                .sum::<usize>()
                + others_count;
            room_total += spec_count;
            specs_out.push(TreeSpec {
                spec_value: *spec_value,
                count: spec_count,
                unit_types: unit_types_out,
                others: TreeOthers {
                    count: others_count,
                    by_noun,
                },
            });
        }
        rooms_out.push(TreeRoom {
            refno: room_ref.refno.clone(),
            room_num: room_ref.room_num.clone(),
            name: RefU64::from_str(&room_ref.refno)
                .ok()
                .and_then(|refno| attrs.name_of(refno)),
            count: room_total,
            specs: specs_out,
        });
    }

    TreeResponse {
        center: CenterJson::from(&scan.center),
        radius: params.radius,
        shape: params.shape.as_str(),
        total_count,
        candidate_count,
        truncated_candidates: scan.truncated_candidates,
        candidate_cap: CANDIDATE_CAP,
        leaves_inline: inline_all,
        leaf_cap: options.leaf_cap,
        leaf_count,
        inlined: options.only.as_ref().map(LeafSelector::label),
        delivery_unit_types: unit_types.to_vec(),
        rooms: rooms_out,
        room_status: scope.status_json(),
        warnings: response_warnings(&scan.center, Some(scope)),
        spatial_state: scan.state.as_str(),
        coverage: COVERAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::spatial_discipline::LineageStep;
    use std::cell::Cell;

    /// 本文件的用例独占 `4000000015/*` 这一组 refno。
    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_015, seq)
    }

    fn types() -> Vec<String> {
        ["BRAN", "HANG", "SUPPO", "EQUI"].map(String::from).to_vec()
    }

    /// 查表属主链：`(refno, noun, name, owner)`，读一次记一次数。
    struct TableLineage {
        steps: HashMap<RefU64, LineageStep>,
        reads: Cell<usize>,
    }

    impl TableLineage {
        fn new(rows: &[(u32, &str, &str, Option<u32>)]) -> Self {
            Self {
                steps: rows
                    .iter()
                    .map(|(seq, noun, name, owner)| {
                        (
                            refno(*seq),
                            LineageStep {
                                noun: noun.to_string(),
                                name: name.to_string(),
                                owner: owner.map(refno),
                            },
                        )
                    })
                    .collect(),
                reads: Cell::new(0),
            }
        }
    }

    impl Lineage for TableLineage {
        fn lineage_step(&self, refno: RefU64) -> Option<LineageStep> {
            self.reads.set(self.reads.get() + 1);
            self.steps.get(&refno).cloned()
        }
    }

    /// TUBI 100 → BRAN 10 → PIPE 1 → ZONE 0；ELBO 101 同枝；PANE 200 → FRMW 20 → STRU 2 → ZONE 0；
    /// 201 读不出记录；EQUI 300 自己就是单元。
    fn lineage() -> TableLineage {
        TableLineage::new(&[
            (0, "ZONE", "/Z", None),
            (1, "PIPE", "/P", Some(0)),
            (10, "BRAN", "/P-B1", Some(1)),
            (100, "TUBI", "", Some(10)),
            (101, "ELBO", "", Some(10)),
            (2, "STRU", "/S", Some(0)),
            (20, "FRMW", "/S-F1", Some(2)),
            (200, "PANE", "", Some(20)),
            (300, "EQUI", "/E1", Some(0)),
        ])
    }

    #[test]
    fn unit_root_is_the_nearest_delivery_unit_up_the_owner_chain_or_none() {
        let lineage = lineage();
        let mut memo = UnitMemo::new();
        let unit = memo
            .unit_root_of(&lineage, &types(), refno(100))
            .expect("TUBI sits under a BRAN");
        assert_eq!(
            (unit.refno, unit.noun.as_str(), unit.name.as_str()),
            (refno(10), "BRAN", "/P-B1")
        );
        assert_eq!(lineage.reads.get(), 2, "TUBI + BRAN 两次读，PIPE 不用再爬");

        assert_eq!(
            memo.unit_root_of(&lineage, &types(), refno(200)),
            None,
            "PANE 一路到 ZONE 都不是单元"
        );
        assert_eq!(
            lineage.reads.get(),
            2 + 4,
            "PANE / FRMW / STRU / ZONE 各读一次"
        );
        assert_eq!(
            memo.unit_root_of(&lineage, &types(), refno(201)),
            None,
            "读不出记录 = 无单元"
        );

        let equi = memo
            .unit_root_of(&lineage, &types(), refno(300))
            .expect("EQUI is its own unit");
        assert_eq!(equi.refno, refno(300));
        assert_eq!(equi.name, "/E1");
    }

    #[test]
    fn unit_memo_truncates_at_any_known_ancestor_and_remembers_the_whole_path() {
        let lineage = lineage();
        let mut memo = UnitMemo::new();
        memo.unit_root_of(&lineage, &types(), refno(100));
        let before = lineage.reads.get();
        let sibling = memo
            .unit_root_of(&lineage, &types(), refno(101))
            .expect("ELBO under the same BRAN");
        assert_eq!(sibling.refno, refno(10));
        assert_eq!(lineage.reads.get(), before + 1, "只读 ELBO 自己，BRAN 已记");
        assert_eq!(memo.len(), 3, "TUBI / BRAN / ELBO 都记了");

        memo.unit_root_of(&lineage, &types(), refno(200));
        let before = lineage.reads.get();
        assert_eq!(memo.unit_root_of(&lineage, &types(), refno(20)), None);
        assert_eq!(lineage.reads.get(), before, "FRMW 已记为无单元，一次都不读");
    }

    #[test]
    fn shared_memo_resets_when_the_epoch_changes_and_keeps_the_lock_off_record_reads() {
        let slot: SharedSlot = Mutex::new(None);
        let lineage = lineage();
        let unit = unit_root_of_in(&slot, types, &lineage, 1, refno(100)).expect("BRAN");
        assert_eq!(unit.refno, refno(10));
        let reads = lineage.reads.get();
        assert_eq!(
            unit_root_of_in(&slot, types, &lineage, 1, refno(101)).map(|u| u.refno),
            Some(refno(10))
        );
        assert_eq!(lineage.reads.get(), reads + 1, "同代：兄弟只读自己");
        assert!(slot.try_lock().is_ok(), "调用结束时锁不在手上");

        let reads = lineage.reads.get();
        assert_eq!(
            unit_root_of_in(&slot, types, &lineage, 2, refno(101)).map(|u| u.refno),
            Some(refno(10))
        );
        assert_eq!(
            lineage.reads.get(),
            reads + 2,
            "换代整体清空：ELBO + BRAN 重读"
        );
    }

    #[test]
    fn leaf_selector_parses_unit_or_other_noun_but_not_both() {
        assert_eq!(LeafSelector::parse(None, None), Ok(None));
        assert_eq!(
            LeafSelector::parse(Some(" 4000000015/10 "), None),
            Ok(Some(LeafSelector::Unit(refno(10))))
        );
        assert_eq!(
            LeafSelector::parse(None, Some("pane")),
            Ok(Some(LeafSelector::OtherNoun("PANE".into())))
        );
        assert!(LeafSelector::parse(Some("abc"), None).is_err());
        assert!(LeafSelector::parse(Some("1_1"), Some("PANE")).is_err());
        assert_eq!(
            LeafSelector::Unit(refno(10)).label(),
            format!("unit:{}", refno(10))
        );
    }

    // ---- 折树 ----

    use crate::fast_model::model_memory_store::ModelMemoryStore;
    use crate::fast_model::spatial_query::scan_tree;
    use crate::fast_model::spatial_rooms::{RoomRef, room_scope_with_member_rooms};
    use crate::fast_model::spatial_state::SpatialTreeState;
    use aios_core::RefnoEnum;
    use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
    use parry3d::bounding_volume::Aabb;

    fn entry(seq: u32, noun: &str, min: [f32; 3], max: [f32; 3]) -> RStarBoundingBox {
        RStarBoundingBox::new(
            Aabb::new(min.into(), max.into()),
            RefnoEnum::from(refno(seq)),
            noun.to_string(),
        )
    }

    /// 查表属性：专业、单元、名字。
    #[derive(Default)]
    struct TableAttrs {
        specs: HashMap<RefU64, i64>,
        units: HashMap<RefU64, UnitRoot>,
        names: HashMap<RefU64, String>,
    }

    impl ElementAttributes for TableAttrs {
        fn dbnum_of(&self, _refno: RefU64) -> Option<u32> {
            Some(4_000_000_015)
        }
        fn name_of(&self, refno: RefU64) -> Option<String> {
            self.names.get(&refno).cloned()
        }
        fn spec_value_of(&self, refno: RefU64) -> i64 {
            self.specs.get(&refno).copied().unwrap_or(0)
        }
        fn unit_root_of(&self, refno: RefU64) -> Option<UnitRoot> {
            self.units.get(&refno).cloned()
        }
    }

    fn unit(seq: u32, noun: &str, name: &str) -> UnitRoot {
        UnitRoot {
            refno: refno(seq),
            noun: noun.into(),
            name: name.into(),
        }
    }

    /// 1 TUBI / 2 ELBO 挂 BRAN 10，3 NOZZ 挂 EQUI 20，4 PANE 无单元（树上两条条目），6 TUBI 挂 BRAN 11 且跨两间房，
    /// 7 FLOOR 无单元、只在第二间房。房间 51「R2」在前（下标 0）、52「R1」在后（下标 1）——输出按房间号排要反过来。
    fn fixture() -> (AccelerationTree, TableAttrs, RoomScope) {
        let tree = AccelerationTree::load(vec![
            entry(1, "TUBI", [1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            entry(2, "ELBO", [2.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
            entry(3, "NOZZ", [3.0, 0.0, 0.0], [4.0, 1.0, 1.0]),
            entry(4, "PANE", [4.0, 0.0, 0.0], [5.0, 1.0, 1.0]),
            entry(4, "PANE", [6.0, 0.0, 0.0], [7.0, 1.0, 1.0]),
            entry(6, "tubi", [5.0, 0.0, 0.0], [6.0, 1.0, 1.0]),
            entry(7, "FLOOR", [7.0, 0.0, 0.0], [8.0, 1.0, 1.0]),
        ]);
        let attrs = TableAttrs {
            specs: [1, 2, 3, 6]
                .into_iter()
                .map(|seq| (refno(seq), 1))
                .collect(),
            units: HashMap::from([
                (refno(1), unit(10, "BRAN", "/B10")),
                (refno(2), unit(10, "BRAN", "/B10")),
                (refno(3), unit(20, "EQUI", "/E20")),
                (refno(6), unit(11, "BRAN", "")),
            ]),
            names: HashMap::from([(refno(51), "/R2-RM".to_string())]),
        };
        let scope = room_scope_with_member_rooms(
            vec![
                RoomRef {
                    refno: refno(51).to_string(),
                    room_num: "R2".into(),
                },
                RoomRef {
                    refno: refno(52).to_string(),
                    room_num: "R1".into(),
                },
            ],
            [
                (refno(1), vec![0]),
                (refno(2), vec![0]),
                (refno(3), vec![0]),
                (refno(4), vec![0]),
                (refno(6), vec![0, 1]),
                (refno(7), vec![1]),
            ],
            0,
        );
        (tree, attrs, scope)
    }

    fn origin_params(rooms: &str) -> SpatialQueryParams {
        SpatialQueryParams::parse(&RawSpatialQuery {
            x: Some("0".into()),
            y: Some("0".into()),
            z: Some("0".into()),
            radius: Some("100".into()),
            rooms: Some(rooms.into()),
            ..RawSpatialQuery::default()
        })
        .expect("params")
    }

    fn tree_of(options: &TreeOptions) -> TreeResponse {
        let (tree, attrs, scope) = fixture();
        let params = origin_params(&format!("{},{}", refno(51), refno(52)));
        let scan = scan_tree(
            &tree,
            SpatialTreeState::Ready,
            &params,
            None,
            &ModelMemoryStore::default(),
        )
        .expect("scan");
        assemble_tree(scan, &params, &attrs, &scope, &types(), options)
    }

    fn leaf_refnos(elements: &Option<Vec<TreeLeaf>>) -> Vec<String> {
        elements
            .as_ref()
            .map(|leaves| leaves.iter().map(|leaf| leaf.refno.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn tree_folds_hits_into_room_spec_unit_type_unit_and_others_with_dedup_and_shared_rooms() {
        let tree = tree_of(&TreeOptions::default());
        assert_eq!(tree.total_count, 6, "refno 4 的两条条目只算一条");
        assert_eq!(tree.candidate_count, 7);
        assert_eq!(tree.leaf_count, 7, "6 跨两房、各算一次放置");
        assert!(tree.leaves_inline);
        assert_eq!(tree.inlined, None);
        assert_eq!(tree.delivery_unit_types, types());
        assert_eq!(tree.room_status.matched, 6);

        assert_eq!(
            tree.rooms
                .iter()
                .map(|room| room.room_num.as_str())
                .collect::<Vec<_>>(),
            ["R1", "R2"],
            "按房间号排，不按请求下标"
        );
        let r1 = &tree.rooms[0];
        assert_eq!(r1.refno, refno(52).to_string());
        assert_eq!((r1.name.as_deref(), r1.count), (None, 2));
        assert_eq!(
            r1.specs
                .iter()
                .map(|spec| (spec.spec_value, spec.count))
                .collect::<Vec<_>>(),
            [(0, 1), (1, 1)]
        );
        assert_eq!(r1.specs[0].others.by_noun[0].noun, "FLOOR");
        assert_eq!(
            leaf_refnos(&r1.specs[0].others.by_noun[0].elements),
            [refno(7).to_string()]
        );
        let shared = &r1.specs[1].unit_types[0].units[0];
        assert_eq!(shared.refno, refno(11).to_string());
        assert_eq!(
            (shared.noun.as_str(), shared.name.as_deref()),
            ("BRAN", None),
            "空名字给 null"
        );
        assert_eq!(
            shared.elements.as_ref().unwrap()[0].shared_rooms,
            Some(2),
            "跨两房的构件带 shared_rooms"
        );

        let r2 = &tree.rooms[1];
        assert_eq!((r2.name.as_deref(), r2.count), (Some("/R2-RM"), 5));
        assert_eq!(
            r2.specs
                .iter()
                .map(|spec| (spec.spec_value, spec.count))
                .collect::<Vec<_>>(),
            [(0, 1), (1, 4)],
            "0「其他」在前"
        );
        let others = &r2.specs[0].others;
        assert_eq!(
            (
                others.count,
                others.by_noun[0].noun.as_str(),
                others.by_noun[0].count
            ),
            (1, "PANE", 1)
        );
        let pane = &others.by_noun[0].elements.as_ref().unwrap()[0];
        assert_eq!(
            (pane.distance, pane.shared_rooms),
            (4.0, None),
            "去重保留距离最近的那条"
        );
        let spec1 = &r2.specs[1];
        assert_eq!(
            spec1
                .unit_types
                .iter()
                .map(|group| (group.noun.as_str(), group.count))
                .collect::<Vec<_>>(),
            [("BRAN", 3), ("EQUI", 1)],
            "单元类型按配置表顺序"
        );
        let bran_units = &spec1.unit_types[0].units;
        assert_eq!(
            bran_units
                .iter()
                .map(|u| (u.refno.clone(), u.count, u.min_distance))
                .collect::<Vec<_>>(),
            [
                (refno(10).to_string(), 2, 1.0),
                (refno(11).to_string(), 1, 5.0)
            ],
            "单元按最近成员距离"
        );
        assert_eq!(bran_units[0].name.as_deref(), Some("/B10"));
        assert_eq!(
            leaf_refnos(&bran_units[0].elements),
            [refno(1).to_string(), refno(2).to_string()],
            "构件按距离"
        );
        assert_eq!(spec1.unit_types[1].units[0].name.as_deref(), Some("/E20"));
        assert_eq!(spec1.others.count, 0);
        assert!(spec1.others.by_noun.is_empty());
    }

    #[test]
    fn leaf_cap_omits_every_element_and_a_selector_inlines_just_one_group() {
        let capped = tree_of(&TreeOptions {
            leaf_cap: 3,
            only: None,
        });
        assert!(!capped.leaves_inline);
        assert_eq!((capped.leaf_count, capped.leaf_cap), (7, 3));
        for room in &capped.rooms {
            for spec in &room.specs {
                assert!(
                    spec.unit_types
                        .iter()
                        .all(|group| group.units.iter().all(|u| u.elements.is_none()))
                );
                assert!(
                    spec.others
                        .by_noun
                        .iter()
                        .all(|group| group.elements.is_none())
                );
            }
        }
        assert_eq!(capped.total_count, 6, "计数不受省略影响");

        let one_unit = tree_of(&TreeOptions {
            leaf_cap: 3,
            only: Some(LeafSelector::Unit(refno(10))),
        });
        assert!(!one_unit.leaves_inline);
        assert_eq!(
            one_unit.inlined.as_deref(),
            Some(format!("unit:{}", refno(10)).as_str())
        );
        let r2 = &one_unit.rooms[1];
        let bran_units = &r2.specs[1].unit_types[0].units;
        assert_eq!(
            leaf_refnos(&bran_units[0].elements).len(),
            2,
            "点到的单元带叶子"
        );
        assert!(bran_units[1].elements.is_none(), "别的单元不带");
        assert!(r2.specs[0].others.by_noun[0].elements.is_none());

        let one_noun = tree_of(&TreeOptions {
            leaf_cap: 3,
            only: Some(LeafSelector::OtherNoun("PANE".into())),
        });
        assert_eq!(
            leaf_refnos(&one_noun.rooms[1].specs[0].others.by_noun[0].elements),
            [refno(4).to_string()]
        );
        assert!(
            one_noun.rooms[0].specs[0].others.by_noun[0]
                .elements
                .is_none(),
            "FLOOR 组不带"
        );
        assert!(
            one_noun.rooms[1].specs[1].unit_types[0].units[0]
                .elements
                .is_none()
        );

        let serialised = serde_json::to_value(&capped).expect("json");
        let first_unit = &serialised["rooms"][1]["specs"][1]["unit_types"][0]["units"][0];
        assert!(first_unit.get("elements").is_none(), "省略的叶子键不出现");
        assert!(serialised.get("inlined").is_none());
    }

    #[test]
    fn room_tree_params_take_the_room_as_target_scope_and_cube_with_zero_margin() {
        let room = refno(5);
        let params = room_tree_params(room, &RawSpatialQuery::default()).expect("defaults");
        assert_eq!(
            params.target,
            crate::fast_model::spatial_query::Target::Refno(room)
        );
        assert_eq!(params.shape, crate::fast_model::spatial_query::Shape::Cube);
        assert_eq!(params.radius, 0.0, "缺省不外扩");
        assert!(!params.include_self, "房间自己的面板不算成员");
        assert_eq!(params.rooms, Some(std::collections::BTreeSet::from([room])));

        let raw = RawSpatialQuery {
            radius: Some("250".into()),
            nouns: Some("pane,tubi".into()),
            spec_values: Some("3".into()),
            ..RawSpatialQuery::default()
        };
        let params = room_tree_params(room, &raw).expect("margin + filters");
        assert_eq!(params.radius, 250.0);
        assert_eq!(
            params.nouns,
            std::collections::BTreeSet::from(["PANE".to_string(), "TUBI".to_string()])
        );
        assert_eq!(
            params.spec_values,
            Some(std::collections::BTreeSet::from([3]))
        );

        for (key, raw) in [
            (
                "x",
                RawSpatialQuery {
                    x: Some("1".into()),
                    ..RawSpatialQuery::default()
                },
            ),
            (
                "refno",
                RawSpatialQuery {
                    refno: Some("1_1".into()),
                    ..RawSpatialQuery::default()
                },
            ),
            (
                "rooms",
                RawSpatialQuery {
                    rooms: Some("1_1".into()),
                    ..RawSpatialQuery::default()
                },
            ),
            (
                "source_mode",
                RawSpatialQuery {
                    source_mode: Some("aabb".into()),
                    ..RawSpatialQuery::default()
                },
            ),
            (
                "radius",
                RawSpatialQuery {
                    radius: Some("-1".into()),
                    ..RawSpatialQuery::default()
                },
            ),
        ] {
            assert!(room_tree_params(room, &raw).is_err(), "{key} 应被拒");
        }
    }
}
