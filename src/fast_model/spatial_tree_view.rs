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
//!
//! **直段**（`tubes=1`，plant3d-web 方案 B，2026-09-22；计划 `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md`）：
//! 隐式管身不进空间树（ADR-072 D4），所以折树拆成两段（[`draft_tree`] → [`TreeDraft::tube_roots`] → [`TreeDraft::finish`]），中间按树里出现的
//! BRAN 单元读根名下的管身记录（`spatial_tree_tubes`），直段按与管件同一把尺过范围（`measure_box_in_range`）、按两端管件的房归房、挂到单元下
//! （[`TreeTube`]），独立 `tube_count` / `total_tube_count`、放置数计入 `leaf_count`；缺省关，关着时响应与旧版逐字节相同。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use aios_core::RefU64;
use serde::Serialize;

use crate::data_interface::generation_root::{is_coarse_hierarchy_noun, is_delivery_unit_noun};
use crate::fast_model::spatial_discipline::Lineage;
use crate::fast_model::spatial_query::{
    BoxJson, CANDIDATE_CAP, COVERAGE, Center, CenterJson, ElementAttributes, Filtered, Hit,
    MAX_RADIUS_MM, RawSpatialQuery, Scan, SortBy, SpatialQueryParams, filter_candidates,
    keyword_matches, measure_box_in_range, parse_f32, response_warnings, sort_hits,
};
use crate::fast_model::spatial_rooms::{RoomScope, RoomStatusJson};
use crate::fast_model::spatial_state::SpatialTreeState;
use crate::fast_model::spatial_tree_tubes::TubeRecord;

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

/// 直段读根预算（计划 `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.6，D1 (b)）：一次树响应最多为多少个
/// BRAN 单元补直段——按最近成员距离取前这么多根，超出置 `tubes_truncated` 并加一句 warning。R432 3 m 球 17 根 / 整房 28 根远在
/// 预算内；100 m 多房那档 BRAN 单元以百计，落盘形态下每根一句冷读，不许把树路由拖成秒级。
pub const DEFAULT_TUBE_ROOT_CAP: usize = 500;

/// 名下有隐式管身的单元类型：管道与风管的分支都是 BRAN；HANG / SUPPO / EQUI 没有直段，不为它们读根。
const TUBE_UNIT_NOUNS: &[&str] = &["BRAN"];

/// 树路由的额外选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeOptions {
    pub leaf_cap: usize,
    pub only: Option<LeafSelector>,
    /// `tubes=1`（计划 §2.1；D6 缺省**关**）：BRAN 单元下补直段行（[`TreeTube`]）与各层 `tube_count`。关着时响应与不认识
    /// 这个参数的旧版**逐字节相同**——新键全是 `Option` + `skip_serializing_if`。
    pub tubes: bool,
    /// 见 [`DEFAULT_TUBE_ROOT_CAP`]。
    pub tube_root_cap: usize,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            leaf_cap: DEFAULT_LEAF_CAP,
            only: None,
            tubes: false,
            tube_root_cap: DEFAULT_TUBE_ROOT_CAP,
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

/// 一段直管（`tubes=1`；计划 §2.1）。身份是 `(所属单元 refno, from, to, ordinal)` 四元组（D3）；`from` / `to` 是两端管件的 `a_b`
/// ——容器头 / 尾那一段的一端是 BRAN 自己。直段没有 refno，不计入任何 `count`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeTube {
    /// 同 `(from, to)` 内的序号（`route_ordinal`），无关两段常为 0。
    pub ordinal: u32,
    pub from: String,
    pub to: String,
    /// 两端的 noun（大写）；命中里有就用命中的，没有回记录读一次，读不出为空串。
    pub from_noun: String,
    pub to_noun: String,
    /// 到中心（点 / 目标盒 / 走廊）的最小表面距离，mm——与管件同一把尺（`spatial_query::measure_box_in_range`）。
    pub distance: f32,
    /// 直管长度，mm（存下的世界变换 z 轴列的模）。
    pub length: f32,
    /// 世界盒（与 `nearby` 结果的 `aabb` 同形），给前端定位飞行用。
    pub aabb: BoxJson,
    /// 标记为无效的直管（`is_invalid_tubi`），前端画告警色。
    pub invalid: bool,
    /// 只在该直段放到 ≥ 2 间所选房间下时出现（同 [`TreeLeaf::shared_rooms`]）。
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
    /// `tubes=1` 时才出现：该单元下落在范围内、属于该房间的直段放置数（**不计入** `count`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube_count: Option<usize>,
    /// 最近成员的距离（只看管件）。
    pub min_distance: f32,
    /// 叶子省略时不出现（`leaves_inline=false` 且不是 `unit=` 点到的那一个）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<TreeLeaf>>,
    /// `tubes=1` 时与 `elements` 同一套内联规则；按距离、再 `(from, to, ordinal)` 排。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tubes: Option<Vec<TreeTube>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeUnitType {
    pub noun: String,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube_count: Option<usize>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube_count: Option<usize>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube_count: Option<usize>,
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
    /// `tubes=1` 时才出现：全树按身份键去重的直段数（跨房直段只算一次，同 `total_count` 口径）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tube_count: Option<usize>,
    /// `tubes=1` 时才出现：BRAN 单元数超过读根预算（[`DEFAULT_TUBE_ROOT_CAP`]），只为最近的那些补了直段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tubes_truncated: Option<bool>,
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
    /// 直段放置（`TreeDraft::finish` 里 `tube_hits` 的下标），`tubes=1` 才有；出响应前按距离排。
    tubes: Vec<usize>,
}

#[derive(Default)]
struct SpecAcc {
    /// 单元类型 noun → 单元 refno → 成员。
    units: BTreeMap<String, BTreeMap<RefU64, UnitAcc>>,
    /// 其他构件：noun → 成员。
    others: BTreeMap<String, Vec<usize>>,
}

/// 折树的第一段产物（计划 §2.2）：过滤 / 排 / 去重 / 单元 / 折入 `per_room` 都做完了，只差直段与出响应。
/// 中间那一步（读直段）可能要 `await`（冷读库），所以拆开：`draft_tree` 与 [`TreeDraft::finish`] 各在一段阻塞线程上跑，
/// [`TreeDraft::tube_roots`] 说要读哪些根。`tubes` 关着时 [`assemble_tree`] 直接 `finish(None)`，行为与拆分前逐字节相同。
pub struct TreeDraft {
    state: SpatialTreeState,
    center: Center,
    candidate_count: usize,
    truncated_candidates: bool,
    params: SpatialQueryParams,
    options: TreeOptions,
    /// 过滤、按距离排、按 refno 去重后的命中。
    hits: Vec<Hit>,
    /// 过滤前树上取到的全部候选 refno：直段归房时区分「两端在范围里却不是任一所选房间的成员 = 属于别处」与
    /// 「两端根本不在范围里 = 解不出」（[`TreeDraft::place_tubes`]）。
    candidate_refnos: HashSet<RefU64>,
    /// 每条命中放到了几间房。
    shared: Vec<usize>,
    /// 管件放置数（跨房构件每间房各算一次）。
    leaf_count: usize,
    per_room: Vec<BTreeMap<i64, SpecAcc>>,
}

/// `tubes=1` 要读的根（[`TreeDraft::tube_roots`]）：树里出现的 BRAN 单元，去重、按最近成员距离排、截到 `tube_root_cap`。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TubeRoots {
    pub roots: Vec<RefU64>,
    /// 截断前的 BRAN 单元数。
    pub total: usize,
    pub truncated: bool,
}

/// 读回来的直段（[`TreeDraft::finish`] 的入参）：`by_root` 是 [`TubeRoots::roots`] 里每根名下的直段（没有就是空表），
/// `truncated` / `roots_total` 原样来自 [`TubeRoots`]，出响应时进 `tubes_truncated` 与 warning。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TubeSupplement {
    pub by_root: HashMap<RefU64, Vec<TubeRecord>>,
    pub roots_total: usize,
    pub truncated: bool,
}

/// 一段进了范围的直段：记录 + 距离 + 两端 noun + 它放到了哪几间房（下标，升序）。
struct TubeHit {
    record: TubeRecord,
    distance: f32,
    from_noun: String,
    to_noun: String,
    rooms: Vec<usize>,
}

/// 锁外的后半程第一段：与 `nearby` 同一条过滤链 → 按距离排、按 refno 去重 → 每条命中解「属于哪几间房」「属于哪个单元」→ 折入
/// 房间 → 专业 → 单元类型 → 单元 / 其他构件的累加器。`scope` 是 `rooms=` 解出来的成员集（树路由要求必给）。
pub fn draft_tree(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    scope: &RoomScope,
    options: &TreeOptions,
) -> TreeDraft {
    let candidate_count = scan.candidates.len();
    let candidate_refnos: HashSet<RefU64> = if options.tubes {
        scan.candidates
            .iter()
            .map(|candidate| candidate.entry.refno)
            .collect()
    } else {
        HashSet::new()
    };
    let Filtered { mut hits, .. } =
        filter_candidates(scan.candidates, params, &scan.center, attrs, params.rooms.as_ref().map(|_| scope));
    sort_hits(&mut hits, SortBy::Distance);
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.refno));

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

    TreeDraft {
        state: scan.state,
        center: scan.center,
        candidate_count,
        truncated_candidates: scan.truncated_candidates,
        params: params.clone(),
        options: options.clone(),
        hits,
        candidate_refnos,
        shared,
        leaf_count,
        per_room,
    }
}

/// 一段式入口（`tubes` 关着 / 单测）：`draft_tree` + `finish(None)`。
pub fn assemble_tree(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    scope: &RoomScope,
    unit_types: &[String],
    options: &TreeOptions,
) -> TreeResponse {
    draft_tree(scan, params, attrs, scope, options).finish(attrs, scope, unit_types, None)
}

impl TreeDraft {
    /// 每个 BRAN 单元出现在哪几间房、各在哪个专业下（第一个碰到的 `spec_value`）；`(距离, refno)` 用来排读根顺序。
    fn tube_unit_slots(&self) -> BTreeMap<RefU64, Vec<(usize, i64)>> {
        let mut slots: BTreeMap<RefU64, Vec<(usize, i64)>> = BTreeMap::new();
        for (room_index, specs) in self.per_room.iter().enumerate() {
            for (spec_value, acc) in specs {
                for (noun, units) in &acc.units {
                    if !TUBE_UNIT_NOUNS.contains(&noun.as_str()) {
                        continue;
                    }
                    for refno in units.keys() {
                        let rooms = slots.entry(*refno).or_default();
                        if !rooms.iter().any(|(room, _)| *room == room_index) {
                            rooms.push((room_index, *spec_value));
                        }
                    }
                }
            }
        }
        slots
    }

    /// `tubes=1` 时要读直段的根（计划 §2.2 / §2.6）：树里出现的 BRAN 单元，去重、按最近成员距离（再 refno）排，
    /// 截到 `tube_root_cap`。关着时为空。
    pub fn tube_roots(&self) -> TubeRoots {
        if !self.options.tubes {
            return TubeRoots::default();
        }
        let mut nearest: HashMap<RefU64, f32> = HashMap::new();
        for specs in &self.per_room {
            for acc in specs.values() {
                for (noun, units) in &acc.units {
                    if !TUBE_UNIT_NOUNS.contains(&noun.as_str()) {
                        continue;
                    }
                    for (refno, unit) in units {
                        let distance = unit
                            .members
                            .first()
                            .map(|first| self.hits[*first].distance)
                            .unwrap_or(f32::INFINITY);
                        let slot = nearest.entry(*refno).or_insert(distance);
                        *slot = slot.min(distance);
                    }
                }
            }
        }
        let mut roots = nearest.into_iter().collect::<Vec<_>>();
        roots.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let total = roots.len();
        let truncated = total > self.options.tube_root_cap;
        roots.truncate(self.options.tube_root_cap);
        TubeRoots {
            roots: roots.into_iter().map(|(refno, _)| refno).collect(),
            total,
            truncated,
        }
    }

    /// 直段过范围 / 过滤 / 归房 / 挂单元（计划 §2.3 / §2.4）。返回放置了的直段，并把下标压进各单元的 `tubes`。
    fn place_tubes(
        &mut self,
        attrs: &dyn ElementAttributes,
        scope: &RoomScope,
        supplement: &TubeSupplement,
    ) -> Vec<TubeHit> {
        let unit_slots = self.tube_unit_slots();
        let exclude_subtree = !self.params.include_self && self.center.target_box.is_some();
        let nouns_allow_tubes = self.params.nouns.is_empty() || self.params.nouns.contains("TUBI");
        let hit_nouns: HashMap<RefU64, String> = self
            .hits
            .iter()
            .map(|hit| (hit.refno, hit.noun.trim().to_ascii_uppercase()))
            .collect();
        let mut noun_memo: HashMap<RefU64, String> = HashMap::new();
        let mut noun_of = |refno: RefU64, bran: RefU64, bran_noun: &str| -> String {
            if refno == bran {
                return bran_noun.to_string();
            }
            if let Some(noun) = hit_nouns.get(&refno) {
                return noun.clone();
            }
            noun_memo
                .entry(refno)
                .or_insert_with(|| attrs.noun_of(refno).unwrap_or_default())
                .clone()
        };

        let mut tube_hits: Vec<TubeHit> = Vec::new();
        if !nouns_allow_tubes {
            return tube_hits;
        }
        let mut brans = supplement.by_root.keys().copied().collect::<Vec<_>>();
        brans.sort();
        for bran in brans {
            let Some(slots) = unit_slots.get(&bran) else {
                continue;
            };
            if exclude_subtree && self.center.subtree.contains(&bran) {
                continue;
            }
            let bran_noun = slots
                .first()
                .and_then(|(room, spec)| {
                    self.per_room[*room][spec]
                        .units
                        .iter()
                        .find(|(_, units)| units.contains_key(&bran))
                        .map(|(noun, _)| noun.clone())
                })
                .unwrap_or_else(|| "BRAN".to_string());
            for record in &supplement.by_root[&bran] {
                if let Some(keyword) = &self.params.keyword
                    && !keyword_matches(keyword, bran, "TUBI")
                {
                    continue;
                }
                let Some(distance) = measure_box_in_range(&self.center, &self.params, &record.aabb) else {
                    continue;
                };
                // D4 (i)：两端管件任一属该房就挂到该房；两端都不是任一所选房间的成员时再分两档——在范围里（是候选）却不是成员
                // = 已经判过、属于别处，它旁边的管件正是这样被 `rooms=` 滤掉的，直段跟着走；根本不在范围里 = 解不出，按单元所在的房兜底。
                let mut ends: Vec<usize> = Vec::new();
                for end in [record.from, record.to] {
                    if let Some(rooms) = scope.member_rooms.get(&end) {
                        ends.extend(rooms.iter().copied());
                    }
                }
                ends.sort_unstable();
                ends.dedup();
                if ends.is_empty()
                    && [record.from, record.to]
                        .iter()
                        .any(|end| self.candidate_refnos.contains(end) && !scope.contains(*end))
                {
                    continue;
                }
                let placed: Vec<(usize, i64)> = if ends.is_empty() {
                    slots.clone()
                } else {
                    slots
                        .iter()
                        .copied()
                        .filter(|(room, _)| ends.binary_search(room).is_ok())
                        .collect()
                };
                if placed.is_empty() {
                    continue;
                }
                let tube_index = tube_hits.len();
                for (room, spec) in &placed {
                    if let Some(acc) = self
                        .per_room
                        .get_mut(*room)
                        .and_then(|specs| specs.get_mut(spec))
                        .and_then(|acc| acc.units.get_mut(&bran_noun))
                        .and_then(|units| units.get_mut(&bran))
                    {
                        acc.tubes.push(tube_index);
                        self.leaf_count += 1;
                    }
                }
                tube_hits.push(TubeHit {
                    from_noun: noun_of(record.from, bran, &bran_noun),
                    to_noun: noun_of(record.to, bran, &bran_noun),
                    record: record.clone(),
                    distance,
                    rooms: placed.into_iter().map(|(room, _)| room).collect(),
                });
            }
        }
        for specs in &mut self.per_room {
            for acc in specs.values_mut() {
                for units in acc.units.values_mut() {
                    for unit in units.values_mut() {
                        unit.tubes.sort_by(|a, b| {
                            let (a, b) = (&tube_hits[*a], &tube_hits[*b]);
                            a.distance
                                .total_cmp(&b.distance)
                                .then_with(|| a.record.key().cmp(&b.record.key()))
                        });
                    }
                }
            }
        }
        tube_hits
    }

    /// 第二段：直段（`tubes=1` 且给了 `supplement`）→ 出响应。`supplement` 为 `None` 时 `tubes` 开着也只是各层 `tube_count = 0`。
    pub fn finish(
        mut self,
        attrs: &dyn ElementAttributes,
        scope: &RoomScope,
        unit_types: &[String],
        supplement: Option<TubeSupplement>,
    ) -> TreeResponse {
        let tubes_on = self.options.tubes;
        let supplement = supplement.unwrap_or_default();
        let tube_hits = if tubes_on {
            self.place_tubes(attrs, scope, &supplement)
        } else {
            Vec::new()
        };
        let total_tube_count = tube_hits.len();
        let Self {
            state,
            center,
            candidate_count,
            truncated_candidates,
            params,
            options,
            hits,
            candidate_refnos: _,
            shared,
            leaf_count,
            per_room,
        } = self;
        let total_count = hits.len();
        let room_count = scope.rooms.len();

        let inline_all = options.only.is_none() && leaf_count <= options.leaf_cap;
        let inline_for = |selected: bool| inline_all || (options.only.is_some() && selected);
        let leaf = |index: usize| TreeLeaf {
            refno: hits[index].refno.to_string(),
            noun: hits[index].noun.trim().to_ascii_uppercase(),
            distance: hits[index].distance,
            shared_rooms: (shared[index] > 1).then_some(shared[index]),
        };
        let tube = |index: usize| {
            let hit = &tube_hits[index];
            TreeTube {
                ordinal: hit.record.ordinal,
                from: hit.record.from.to_string(),
                to: hit.record.to.to_string(),
                from_noun: hit.from_noun.clone(),
                to_noun: hit.to_noun.clone(),
                distance: hit.distance,
                length: hit.record.length,
                aabb: BoxJson::from(&hit.record.aabb),
                invalid: hit.record.invalid,
                shared_rooms: (hit.rooms.len() > 1).then_some(hit.rooms.len()),
            }
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
            let mut room_tubes = 0usize;
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
                                    tube_count: tubes_on.then_some(unit.tubes.len()),
                                    min_distance: unit
                                        .members
                                        .first()
                                        .map(|first| hits[*first].distance)
                                        .unwrap_or(0.0),
                                    elements: inline_for(selected)
                                        .then(|| unit.members.iter().map(|member| leaf(*member)).collect()),
                                    tubes: (tubes_on && inline_for(selected))
                                        .then(|| unit.tubes.iter().map(|index| tube(*index)).collect()),
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
                            tube_count: tubes_on.then(|| {
                                units_out
                                    .iter()
                                    .map(|unit| unit.tube_count.unwrap_or(0))
                                    .sum()
                            }),
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
                let spec_tubes = unit_types_out
                    .iter()
                    .map(|group| group.tube_count.unwrap_or(0))
                    .sum::<usize>();
                room_total += spec_count;
                room_tubes += spec_tubes;
                specs_out.push(TreeSpec {
                    spec_value: *spec_value,
                    count: spec_count,
                    tube_count: tubes_on.then_some(spec_tubes),
                    unit_types: unit_types_out,
                    others: TreeOthers {
                        count: others_count,
                        by_noun,
                    },
                });
            }
            if params.rooms.is_none() && room_total == 0 && room_tubes == 0 {
                continue;
            }
            rooms_out.push(TreeRoom {
                refno: room_ref.refno.clone(),
                room_num: room_ref.room_num.clone(),
                name: RefU64::from_str(&room_ref.refno)
                    .ok()
                    .and_then(|refno| attrs.name_of(refno)),
                count: room_total,
                tube_count: tubes_on.then_some(room_tubes),
                specs: specs_out,
            });
        }

        let mut warnings = response_warnings(&center, params.rooms.as_ref().map(|_| scope)).unwrap_or_default();
        if params.rooms.is_none() {
            // 「未归属房间」这一组本身就是正常归宿：房间模型只覆盖有面板的房间，
            // 场地、支架、跨房间的管道都不属于任何一间房。这类构件照旧挂进
            // 「未归属房间」，但不再产出警告——条数在 `room_status.unresolved` 里。
            if scope.source == "unavailable" {
                warnings.push("房间归属服务暂不可用，命中构件保留在「未归属房间」中".to_string());
            }
            if scope.library_alignment_current == Some(false) {
                warnings.push("房间归属不是当前代，分组可能过期；未归属构件仍保留".to_string());
            }
        }
        if tubes_on && supplement.truncated {
            warnings.push(format!(
                "直段：BRAN 单元 {} 个超过读根预算 {}，只为最近的 {} 个补了直段（tubes_truncated）",
                supplement.roots_total,
                options.tube_root_cap,
                supplement.by_root.len()
            ));
        }

        let mut room_status = scope.status_json();
        if params.rooms.is_none() {
            room_status.rooms = rooms_out.iter()
                .filter(|room| room.refno != "unassigned")
                .map(|room| crate::fast_model::spatial_rooms::RoomRef {
                    refno: room.refno.clone(),
                    room_num: room.room_num.clone(),
                }).collect();
            room_status.matched = total_count;
        }
        TreeResponse {
            center: CenterJson::from(&center),
            radius: params.radius,
            shape: params.shape.as_str(),
            total_count,
            total_tube_count: tubes_on.then_some(total_tube_count),
            tubes_truncated: tubes_on.then_some(supplement.truncated),
            candidate_count,
            truncated_candidates,
            candidate_cap: CANDIDATE_CAP,
            leaves_inline: inline_all,
            leaf_cap: options.leaf_cap,
            leaf_count,
            inlined: options.only.as_ref().map(LeafSelector::label),
            delivery_unit_types: unit_types.to_vec(),
            rooms: rooms_out,
            room_status,
            warnings: (!warnings.is_empty()).then_some(warnings),
            spatial_state: state.as_str(),
            coverage: COVERAGE,
        }
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

    /// 查表属性：专业、单元、名字、noun（只给不在命中里的直段两端用，读一次记一次数）。
    #[derive(Default)]
    struct TableAttrs {
        specs: HashMap<RefU64, i64>,
        units: HashMap<RefU64, UnitRoot>,
        names: HashMap<RefU64, String>,
        nouns: HashMap<RefU64, String>,
        noun_reads: std::sync::atomic::AtomicUsize,
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
        fn noun_of(&self, refno: RefU64) -> Option<String> {
            self.noun_reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.nouns.get(&refno).cloned()
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
    /// 7 FLOOR 无单元、只在第二间房；5 GWALL 在范围里却不是任一所选房间的成员（被 `rooms=` 滤掉的那一类）。
    /// 房间 51「R2」在前（下标 0）、52「R1」在后（下标 1）——输出按房间号排要反过来。
    fn fixture() -> (AccelerationTree, TableAttrs, RoomScope) {
        let tree = AccelerationTree::load(vec![
            entry(1, "TUBI", [1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            entry(2, "ELBO", [2.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
            entry(3, "NOZZ", [3.0, 0.0, 0.0], [4.0, 1.0, 1.0]),
            entry(4, "PANE", [4.0, 0.0, 0.0], [5.0, 1.0, 1.0]),
            entry(4, "PANE", [6.0, 0.0, 0.0], [7.0, 1.0, 1.0]),
            entry(5, "GWALL", [1.0, 5.0, 0.0], [2.0, 6.0, 1.0]),
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
            nouns: HashMap::from([(refno(8), "VALV".to_string())]),
            noun_reads: Default::default(),
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
        assert_eq!(tree.total_count, 6, "refno 4 的两条条目只算一条；5 不是房间成员");
        assert_eq!(tree.candidate_count, 8);
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
            ..TreeOptions::default()
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
            ..TreeOptions::default()
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
            ..TreeOptions::default()
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

    // ---- 直段（tubes=1，方案 B）----

    fn tube_record(bran: u32, from: u32, to: u32, ordinal: u32, min: [f32; 3], max: [f32; 3]) -> TubeRecord {
        TubeRecord {
            bran: refno(bran),
            ordinal,
            from: refno(from),
            to: refno(to),
            aabb: Aabb::new(min.into(), max.into()),
            length: (max[0] - min[0]) * 1000.0,
            invalid: ordinal == 1,
        }
    }

    /// BRAN 10 的六段（a 两端都是房 0 的成员；b 在范围外；c 从容器头到 1；d 到一个不在命中里的 8；e 两端都不在范围里、解不出；
    /// h 一端是 5——在范围里却不是任一所选房间的成员 = 属于别处）、BRAN 11 的两段（f 一端是跨两房的 6；g 一端是只在房 1 的 7）、
    /// BRAN 12（不在树上）的一段。
    fn supplement() -> TubeSupplement {
        TubeSupplement {
            by_root: HashMap::from([
                (
                    refno(10),
                    vec![
                        tube_record(10, 1, 2, 0, [1.5, 0.0, 0.0], [2.5, 1.0, 1.0]),
                        tube_record(10, 2, 1, 0, [500.0, 0.0, 0.0], [600.0, 1.0, 1.0]),
                        tube_record(10, 10, 1, 0, [0.5, 0.0, 0.0], [1.0, 1.0, 1.0]),
                        tube_record(10, 2, 8, 1, [2.5, 0.0, 0.0], [3.0, 1.0, 1.0]),
                        tube_record(10, 8, 9, 0, [3.0, 0.0, 0.0], [3.5, 1.0, 1.0]),
                        tube_record(10, 5, 9, 0, [1.0, 4.0, 0.0], [2.0, 5.0, 1.0]),
                    ],
                ),
                (
                    refno(11),
                    vec![
                        tube_record(11, 6, 11, 0, [5.5, 0.0, 0.0], [6.0, 1.0, 1.0]),
                        tube_record(11, 7, 13, 0, [6.0, 0.0, 0.0], [6.5, 1.0, 1.0]),
                    ],
                ),
                (
                    refno(12),
                    vec![tube_record(12, 1, 2, 0, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])],
                ),
            ]),
            roots_total: 2,
            truncated: false,
        }
    }

    fn tubes_on(options: TreeOptions) -> TreeOptions {
        TreeOptions {
            tubes: true,
            ..options
        }
    }

    fn tube_tree_with(
        options: &TreeOptions,
        params: &SpatialQueryParams,
        supplement: TubeSupplement,
    ) -> (TreeResponse, TableAttrs) {
        let (tree, attrs, scope) = fixture();
        let scan = scan_tree(
            &tree,
            SpatialTreeState::Ready,
            params,
            None,
            &ModelMemoryStore::default(),
        )
        .expect("scan");
        let draft = draft_tree(scan, params, &attrs, &scope, options);
        let response = draft.finish(&attrs, &scope, &types(), Some(supplement));
        (response, attrs)
    }

    fn tube_tree_of(options: &TreeOptions) -> (TreeResponse, TableAttrs) {
        let params = origin_params(&format!("{},{}", refno(51), refno(52)));
        tube_tree_with(options, &params, supplement())
    }

    fn tube_ends(tubes: &Option<Vec<TreeTube>>) -> Vec<(String, String, u32)> {
        tubes
            .as_ref()
            .map(|tubes| {
                tubes
                    .iter()
                    .map(|tube| (tube.from.clone(), tube.to.clone(), tube.ordinal))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn walk_json(value: &serde_json::Value, visit: &mut dyn FnMut(&str)) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    visit(key);
                    walk_json(value, visit);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk_json(item, visit)),
            _ => {}
        }
    }

    /// D6：`tubes` 关着时（缺省）响应里**不出现**任何直段键——旧客户端 / 金样看到的字节与拆分前相同；
    /// 拆成两段的 `draft_tree` + `finish(None)` 与一段式 `assemble_tree` 同一个答案。
    #[test]
    fn tubes_off_keeps_the_response_free_of_every_tube_key() {
        let (tree, attrs, scope) = fixture();
        let params = origin_params(&format!("{},{}", refno(51), refno(52)));
        let one_shot = assemble_tree(
            scan_tree(&tree, SpatialTreeState::Ready, &params, None, &ModelMemoryStore::default()).expect("scan"),
            &params,
            &attrs,
            &scope,
            &types(),
            &TreeOptions::default(),
        );
        let two_step = draft_tree(
            scan_tree(&tree, SpatialTreeState::Ready, &params, None, &ModelMemoryStore::default()).expect("scan"),
            &params,
            &attrs,
            &scope,
            &TreeOptions::default(),
        )
        .finish(&attrs, &scope, &types(), Some(supplement()));
        assert_eq!(one_shot, two_step, "关着时给了直段也不用");
        let json = serde_json::to_string(&one_shot).expect("json");
        for key in ["tube_count", "tubes", "total_tube_count", "tubes_truncated", "from_noun"] {
            assert!(!json.contains(&format!("\"{key}\"")), "{key} 不该出现: {json}");
        }
        assert_eq!(one_shot.leaf_count, 7, "直段不算进 leaf_count");
    }

    /// `tubes=1`：范围内的直段按两端管件的房挂到 BRAN 单元下（D4 (i)；两端都解不出的按单元所在的房兜底），
    /// 逐层 `tube_count`、跨房只算一次的 `total_tube_count`、直段放置数计入 `leaf_count`（D7）；`count` 一个不变。
    #[test]
    fn tubes_on_hangs_segments_under_bran_units_by_their_end_rooms_and_counts_them_apart() {
        let (tree, attrs) = tube_tree_of(&tubes_on(TreeOptions::default()));
        assert_eq!(tree.total_count, 6, "构件计数不变");
        assert_eq!(
            tree.total_tube_count,
            Some(6),
            "b 在范围外、h 一端是范围里的非成员（属于别处）、BRAN 12 不在树上；f 跨两房只算一次"
        );
        assert_eq!(tree.tubes_truncated, Some(false));
        assert_eq!(tree.leaf_count, 7 + 7, "7 个构件放置 + 7 个直段放置（f 两间房各一次）");
        assert!(tree.leaves_inline);
        assert!(tree.warnings.is_none());

        let r1 = &tree.rooms[0];
        assert_eq!((r1.room_num.as_str(), r1.count, r1.tube_count), ("R1", 2, Some(2)));
        assert_eq!(r1.specs[0].tube_count, Some(0), "「其他」专业没有直段");
        assert_eq!(r1.specs[1].tube_count, Some(2));
        let bran11_in_r1 = &r1.specs[1].unit_types[0].units[0];
        assert_eq!((bran11_in_r1.refno.as_str(), bran11_in_r1.count, bran11_in_r1.tube_count),
            (refno(11).to_string().as_str(), 1, Some(2)));
        assert_eq!(
            tube_ends(&bran11_in_r1.tubes),
            [
                (refno(6).to_string(), refno(11).to_string(), 0),
                (refno(7).to_string(), refno(13).to_string(), 0),
            ],
            "f 经 6 跨两房、g 经 7 只在房 1"
        );
        let f = &bran11_in_r1.tubes.as_ref().unwrap()[0];
        assert_eq!((f.shared_rooms, f.to_noun.as_str()), (Some(2), "BRAN"), "到容器尾那一端的 noun 是 BRAN 自己");
        assert_eq!(f.distance, 5.5);
        assert_eq!(f.length, 500.0);
        assert_eq!(f.aabb.min, [5.5, 0.0, 0.0]);
        let g = &bran11_in_r1.tubes.as_ref().unwrap()[1];
        assert_eq!((g.shared_rooms, g.from_noun.as_str(), g.to_noun.as_str()), (None, "FLOOR", ""));

        let r2 = &tree.rooms[1];
        assert_eq!((r2.room_num.as_str(), r2.count, r2.tube_count), ("R2", 5, Some(5)));
        let spec1 = &r2.specs[1];
        assert_eq!(spec1.tube_count, Some(5));
        assert_eq!(
            spec1.unit_types.iter().map(|g| (g.noun.as_str(), g.count, g.tube_count)).collect::<Vec<_>>(),
            [("BRAN", 3, Some(5)), ("EQUI", 1, Some(0))]
        );
        let bran10 = &spec1.unit_types[0].units[0];
        assert_eq!((bran10.refno.as_str(), bran10.count, bran10.tube_count, bran10.min_distance),
            (refno(10).to_string().as_str(), 2, Some(4), 1.0));
        assert_eq!(
            tube_ends(&bran10.tubes),
            [
                (refno(10).to_string(), refno(1).to_string(), 0),
                (refno(1).to_string(), refno(2).to_string(), 0),
                (refno(2).to_string(), refno(8).to_string(), 1),
                (refno(8).to_string(), refno(9).to_string(), 0),
            ],
            "按距离排；b 在范围外不来；e 两端都解不出按单元的房兜底"
        );
        let tubes = bran10.tubes.as_ref().unwrap();
        assert_eq!(
            tubes.iter().map(|t| (t.from_noun.as_str(), t.to_noun.as_str(), t.invalid)).collect::<Vec<_>>(),
            [("BRAN", "TUBI", false), ("TUBI", "ELBO", false), ("ELBO", "VALV", true), ("VALV", "", false)],
            "两端 noun：容器 = BRAN、命中里的用命中、不在命中里的读一次、读不出为空"
        );
        assert_eq!(
            attrs.noun_reads.load(std::sync::atomic::Ordering::Relaxed),
            3,
            "8 读一次记住、9 / 13 各一次"
        );
        assert!(tubes.iter().all(|t| t.shared_rooms.is_none()));
        let bran11_in_r2 = &spec1.unit_types[0].units[1];
        assert_eq!((bran11_in_r2.tube_count, tube_ends(&bran11_in_r2.tubes).len()), (Some(1), 1), "g 不在房 0");

        let json = serde_json::to_value(&tree).expect("json");
        let mut keys = HashSet::new();
        walk_json(&json, &mut |key| {
            keys.insert(key.to_string());
        });
        for key in ["tube_count", "tubes", "total_tube_count", "tubes_truncated", "from_noun", "length", "aabb"] {
            assert!(keys.contains(key), "{key} 该出现");
        }
        assert_eq!(json["rooms"][1]["specs"][1]["unit_types"][1]["units"][0]["tube_count"], 0, "EQUI 单元也带 tube_count=0");
    }

    /// D7：直段放置数与叶子共用 `leaf_cap`——超了就连 `tubes` 一起省略；`unit=` 点到的单元连直段一起内联。
    #[test]
    fn tube_rows_share_the_leaf_cap_and_the_unit_selector() {
        let (capped, _) = tube_tree_of(&tubes_on(TreeOptions {
            leaf_cap: 10,
            ..TreeOptions::default()
        }));
        assert!(!capped.leaves_inline, "7 + 7 > 10");
        assert_eq!((capped.leaf_count, capped.total_tube_count), (14, Some(6)), "计数不受省略影响");
        for room in &capped.rooms {
            for spec in &room.specs {
                for group in &spec.unit_types {
                    assert!(group.units.iter().all(|u| u.elements.is_none() && u.tubes.is_none()));
                    assert!(group.units.iter().all(|u| u.tube_count.is_some()), "计数照给");
                }
            }
        }
        let (one_unit, _) = tube_tree_of(&tubes_on(TreeOptions {
            leaf_cap: 10,
            only: Some(LeafSelector::Unit(refno(10))),
            ..TreeOptions::default()
        }));
        let bran_units = &one_unit.rooms[1].specs[1].unit_types[0].units;
        assert_eq!(leaf_refnos(&bran_units[0].elements).len(), 2);
        assert_eq!(tube_ends(&bran_units[0].tubes).len(), 4, "点到的单元连直段一起带");
        assert!(bran_units[1].elements.is_none() && bran_units[1].tubes.is_none(), "别的单元不带");

        let (loose, _) = tube_tree_of(&tubes_on(TreeOptions {
            leaf_cap: 14,
            ..TreeOptions::default()
        }));
        assert!(loose.leaves_inline, "恰好到上限仍内联");
    }

    /// 过滤链里直段能走的几格：`nouns=` 点名了类型却没点 TUBI → 直段不出；点了 TUBI 照出；`keyword=` 用容器 BRAN + TUBI 走同一个谓词。
    #[test]
    fn tubes_follow_the_nouns_and_keyword_filters_like_elements() {
        let rooms = format!("{},{}", refno(51), refno(52));
        let with = |extra: &[(&str, &str)]| {
            let mut raw = RawSpatialQuery {
                x: Some("0".into()),
                y: Some("0".into()),
                z: Some("0".into()),
                radius: Some("100".into()),
                rooms: Some(rooms.clone()),
                ..RawSpatialQuery::default()
            };
            for (key, value) in extra {
                match *key {
                    "nouns" => raw.nouns = Some((*value).into()),
                    "keyword" => raw.keyword = Some((*value).into()),
                    other => panic!("{other}"),
                }
            }
            let params = SpatialQueryParams::parse(&raw).expect("params");
            tube_tree_with(&tubes_on(TreeOptions::default()), &params, supplement()).0
        };
        let elbo_only = with(&[("nouns", "elbo")]);
        assert_eq!((elbo_only.total_count, elbo_only.total_tube_count), (1, Some(0)), "BRAN 10 在树上（经 ELBO 2），直段不出");
        let elbo_and_tubi = with(&[("nouns", "elbo,tubi")]);
        assert_eq!((elbo_and_tubi.total_count, elbo_and_tubi.total_tube_count), (3, Some(6)));
        let keyword_elbo = with(&[("keyword", "elbo")]);
        assert_eq!((keyword_elbo.total_count, keyword_elbo.total_tube_count), (1, Some(0)), "TUBI / BRAN refno 都不含 ELBO");
        let keyword_tubi = with(&[("keyword", "tubi")]);
        assert_eq!((keyword_tubi.total_count, keyword_tubi.total_tube_count), (2, Some(6)));
        let keyword_bran11 = with(&[("keyword", &refno(11).to_string())]);
        assert_eq!(keyword_bran11.total_count, 0, "命中里没有含 BRAN 11 refno 的，树是空的，直段没地方挂");
        assert_eq!(keyword_bran11.total_tube_count, Some(0));
    }

    /// D1 (b) 的预算：`tube_roots()` 按最近成员距离排、截到 `tube_root_cap`，超了 `tubes_truncated=true` + warning；
    /// 没读的根 `tube_count = 0`。关着时不要任何根。
    #[test]
    fn tube_roots_are_the_nearest_bran_units_up_to_the_budget() {
        let (tree, attrs, scope) = fixture();
        let params = origin_params(&format!("{},{}", refno(51), refno(52)));
        let scan = || scan_tree(&tree, SpatialTreeState::Ready, &params, None, &ModelMemoryStore::default()).expect("scan");
        assert_eq!(
            draft_tree(scan(), &params, &attrs, &scope, &TreeOptions::default()).tube_roots(),
            TubeRoots::default(),
            "关着不读"
        );
        let all = draft_tree(scan(), &params, &attrs, &scope, &tubes_on(TreeOptions::default())).tube_roots();
        assert_eq!(
            all,
            TubeRoots {
                roots: vec![refno(10), refno(11)],
                total: 2,
                truncated: false
            },
            "BRAN 10 最近成员 1.0、BRAN 11 是 5.0；EQUI 20 不读"
        );
        let budget = tubes_on(TreeOptions {
            tube_root_cap: 1,
            ..TreeOptions::default()
        });
        let draft = draft_tree(scan(), &params, &attrs, &scope, &budget);
        let roots = draft.tube_roots();
        assert_eq!((roots.roots.clone(), roots.total, roots.truncated), (vec![refno(10)], 2, true));
        let mut only_read = supplement();
        only_read.by_root.retain(|root, _| roots.roots.contains(root));
        only_read.roots_total = roots.total;
        only_read.truncated = roots.truncated;
        let response = draft.finish(&attrs, &scope, &types(), Some(only_read));
        assert_eq!(response.tubes_truncated, Some(true));
        assert_eq!(response.total_tube_count, Some(4));
        assert!(
            response.warnings.as_ref().is_some_and(|w| w.iter().any(|line| line.contains("预算 1") && line.contains("2 个"))),
            "{:?}",
            response.warnings
        );
        let bran11 = &response.rooms[0].specs[1].unit_types[0].units[0];
        assert_eq!((bran11.refno.as_str(), bran11.tube_count), (refno(11).to_string().as_str(), Some(0)), "没读的根计 0");
    }

    /// `include_self=false` 的 refno 目标：目标子树里的 BRAN 是「自己」，它的直段不列（成员被剔、单元也不在树上是常态；
    /// 这里目标就是 BRAN 自己在树上的盒、成员不在投影里，单元仍出现——直段照样按容器剔）。
    #[test]
    fn a_target_brans_own_tubes_are_dropped_with_include_self_false() {
        let (mut tree_entries, attrs, scope) = fixture();
        let mut entries = tree_entries.tree.iter().cloned().collect::<Vec<_>>();
        entries.push(entry(10, "BRAN", [1.0, 0.0, 0.0], [3.0, 1.0, 1.0]));
        tree_entries = AccelerationTree::load(entries);
        let query = |include_self: &str| {
            SpatialQueryParams::parse(&RawSpatialQuery {
                refno: Some(refno(10).to_string()),
                radius: Some("100".into()),
                rooms: Some(format!("{},{}", refno(51), refno(52))),
                include_self: Some(include_self.into()),
                ..RawSpatialQuery::default()
            })
            .expect("params")
        };
        let run = |params: &SpatialQueryParams| {
            let scan = scan_tree(&tree_entries, SpatialTreeState::Ready, params, None, &ModelMemoryStore::default()).expect("scan");
            draft_tree(scan, params, &attrs, &scope, &tubes_on(TreeOptions::default()))
                .finish(&attrs, &scope, &types(), Some(supplement()))
        };
        let excluded = run(&query("false"));
        let bran10 = &excluded.rooms[1].specs[1].unit_types[0].units[0];
        assert_eq!((bran10.refno.as_str(), bran10.count, bran10.tube_count), (refno(10).to_string().as_str(), 2, Some(0)), "目标 BRAN 自己的直段不列");
        assert_eq!(excluded.total_tube_count, Some(2), "BRAN 11 的 f / g 照列");
        let included = run(&query("true"));
        assert_eq!(included.total_tube_count, Some(2 + 4), "include_self=true 才连它自己的直段一起列");
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
