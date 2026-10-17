//! 空间查询的「房间」维度（plant3d-web ADR 0067；计划
//! `plant3d-web/docs/plans/2026-09-20-spatial-query-room-and-discipline-filter-plan.md` §4.1 / §4.2）：
//!
//! - `GET /api/v1/spatial/rooms`（[`list_rooms`]）：在册房间清单 + 房间体制此刻的状态，给抽屉画可搜索下拉；
//! - `nearby` / `nearby/refnos` 的 `rooms=`（[`resolve_room_scope`]）：把一批候选折成「谁属于所选房间」的成员集，
//!   过滤在服务端分页前做（`spatial_query::filter_candidates`），`total_count` / 分组 / 分页 / 全集因此一致。
//!
//! 「属于」按**任一归属**（Q2）：构件挂在所选房间任一面板上的边即命中；材料表取的「主归属」不在这里用。
//! 归属的取数与 `e3d.room.lookup`（`query_service::room`）同一条判据分两路：
//! - **读透形态**（`data_face().is_read_through()`）：库上不落 `room_relate` 边，用常驻房间模型 + 候选的内存投影记录现算
//!   （`MemoryRoomCalculator::calculate_for_root`，只读投影快照、**不取空间串行锁**——这是一次探索性查询，不替单个构件做
//!   可承诺的答复）；没有投影记录的候选（树从快照恢复而本进程没再 ensure 过的根）判不出归属，计入 `unresolved`、从结果剔除。
//! - **摄入 / 落盘形态**：直接读 `SUL_DB` 的 `room_relate` 边（`in ∈ 所选面板`）。
//!
//! 归属不是当前代 / 没算过的候选**照常答**（Q13）：`room_status` 说清来源与缺口，`warnings` 各加一句，不 409。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use aios_core::{RefU64, RefnoEnum};
use serde::Serialize;

use crate::fast_model::room_model::MemoryRoomCalculator;
use crate::fast_model::room_resident::{self, ResidentLifecycle, ResidentRoomModel};
use crate::fast_model::spatial_query::{ElementAttributes, SpatialQueryError};

/// `/spatial/rooms` 与 `room_status` 里 `status` 的字面值。
pub const STATUS_READY: &str = "ready";
pub const STATUS_DEGRADED: &str = "degraded";
pub const STATUS_INITIALIZING: &str = "initializing";
pub const STATUS_DISABLED: &str = "disabled";
pub const STATUS_UNSUPPORTED: &str = "unsupported";
pub const STATUS_FAILED: &str = "failed";

/// `room_status.source`：归属从哪来。
pub const SOURCE_MEMORY: &str = "memory";
pub const SOURCE_DURABLE: &str = "durable";

/// 房间体制此刻的状态：`status` 字面值、一句原因、能拿到就给常驻模型。
pub struct RoomsAvailability {
    pub status: &'static str,
    pub reason: Option<String>,
    pub model: Option<Arc<ResidentRoomModel>>,
}

/// 从总开关 + 常驻模型生命周期折出状态（`/health.features.room` 同一套输入）。
/// `Ready` / `Degraded` 都给清单——Degraded 的模型照常算房间（ADR-073 D10），只是有缺口要说。
pub fn rooms_availability() -> RoomsAvailability {
    if !crate::options::room_membership() {
        return RoomsAvailability {
            status: STATUS_DISABLED,
            reason: Some("room_membership=false：服务端未开启房间归属计算".to_string()),
            model: None,
        };
    }
    if !crate::options::spatial_compute() {
        return RoomsAvailability {
            status: STATUS_DISABLED,
            reason: Some(format!(
                "{}=0：空间派生计算关闭，房间模型不装载",
                crate::options::SPATIAL_COMPUTE_ENV
            )),
            model: None,
        };
    }
    let (status, reason) = match room_resident::lifecycle() {
        ResidentLifecycle::Disabled { source } => (STATUS_DISABLED, Some(format!("房间模型未启用：{source}"))),
        ResidentLifecycle::Loading { phase, .. } => {
            (STATUS_INITIALIZING, Some(format!("房间模型正在装载（{phase}），稍后重试")))
        }
        ResidentLifecycle::Unsupported { reason } => (STATUS_UNSUPPORTED, Some(reason)),
        ResidentLifecycle::Blocked { problems } => (
            STATUS_FAILED,
            Some(problems.first().cloned().unwrap_or_else(|| "房间模型被阻断".to_string())),
        ),
        ResidentLifecycle::Ready { .. } => (STATUS_READY, None),
        ResidentLifecycle::Degraded { problems, .. } => (STATUS_DEGRADED, problems.first().cloned()),
    };
    if status != STATUS_READY && status != STATUS_DEGRADED {
        return RoomsAvailability {
            status,
            reason,
            model: None,
        };
    }
    match room_resident::current_for_calculation() {
        Ok(model) => RoomsAvailability {
            status,
            reason,
            model: Some(model),
        },
        Err(error) => RoomsAvailability {
            status: STATUS_FAILED,
            reason: Some(format!("房间模型取不到：{error:#}")),
            model: None,
        },
    }
}

/// `/spatial/rooms` 清单里的一间房。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomListItem {
    /// `a_b`。
    pub refno: String,
    pub room_num: String,
    /// 房间元素的 `NAME`；读不出为 `null`。
    pub name: Option<String>,
    /// 答不出为 `null`。
    pub dbnum: Option<u32>,
    pub panel_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomsResponse {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 常驻房间模型这一代的代号（`ResidentRoomModel::input_version`）；模型不在为 `null`。
    pub definition_version: Option<String>,
    pub rooms: Vec<RoomListItem>,
}

/// 在册房间按 `room_num`、refno 排的清单（纯函数部分，单测直接喂 `RoomPanels`）。
pub fn room_list_items(
    rooms: &[crate::fast_model::room_model::RoomPanels],
    attrs: &dyn ElementAttributes,
) -> Vec<RoomListItem> {
    let mut items = rooms
        .iter()
        .map(|room| {
            let refno = room.room.refno();
            RoomListItem {
                refno: refno.to_string(),
                room_num: room.room_num.clone(),
                name: attrs.name_of(refno),
                dbnum: attrs.dbnum_of(refno),
                panel_count: room.panels.len(),
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a.room_num.cmp(&b.room_num).then_with(|| a.refno.cmp(&b.refno)));
    items
}

/// `GET /api/v1/spatial/rooms`。
pub fn list_rooms(attrs: &dyn ElementAttributes) -> RoomsResponse {
    let availability = rooms_availability();
    let Some(model) = availability.model else {
        return RoomsResponse {
            status: availability.status,
            reason: availability.reason,
            definition_version: None,
            rooms: Vec::new(),
        };
    };
    RoomsResponse {
        status: availability.status,
        reason: availability.reason,
        definition_version: Some(model.input_version()),
        rooms: room_list_items(&model.map.rooms, attrs),
    }
}

/// 请求里点到的一间在册房间。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomRef {
    /// `a_b`。
    pub refno: String,
    pub room_num: String,
}

/// 一次查询里「所选房间的成员是谁」的答案，交给 `filter_candidates` 做集合过滤。
#[derive(Debug, Clone, PartialEq)]
pub struct RoomScope {
    pub rooms: Vec<RoomRef>,
    /// 属于任一所选房间的构件（任一归属，Q2）。
    pub members: HashSet<RefU64>,
    /// 成员 → 它属于 `rooms` 里的哪几间（下标，升序去重）。`nearby` 不看它；房间层级树（ADR 0068，
    /// `spatial_tree_view`）按它把构件挂到每一间所属的房间下。两条解算路径都顺手填：读透形态由
    /// `面板 → 构件` 边经「面板 → 房间」映射得到，落盘形态由 `room_relate` 的 `in`（面板）得到。
    pub member_rooms: HashMap<RefU64, Vec<usize>>,
    /// 候选里判不出归属的条数（读透形态没有投影记录 / 单条记录算不出来）；已从结果剔除。
    pub unresolved: usize,
    pub source: &'static str,
    pub definition_version: Option<String>,
    /// 落盘形态：候选所在库的房间边是否都对齐到当前代；读透形态 `None`。
    pub library_alignment_current: Option<bool>,
}

impl RoomScope {
    pub fn contains(&self, refno: RefU64) -> bool {
        self.members.contains(&refno)
    }

    /// 要并进响应 `warnings` 的话（没有就空）。
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.unresolved > 0 {
            out.push(format!(
                "房间过滤：{} 个候选没有内存投影记录、判不出房间归属，已从结果剔除（先显示它们再查会被纳入）",
                self.unresolved
            ));
        }
        if self.library_alignment_current == Some(false) {
            out.push("房间归属不是当前代：所在库的房间边还没对齐到当前房间模型，结果可能过期".to_string());
        }
        out
    }

    pub fn status_json(&self) -> RoomStatusJson {
        RoomStatusJson {
            rooms: self.rooms.clone(),
            source: self.source,
            matched: self.members.len(),
            unresolved: self.unresolved,
            definition_version: self.definition_version.clone(),
            library_alignment_current: self.library_alignment_current,
        }
    }
}

/// 响应里的 `room_status`（只在给了 `rooms=` 时出现）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomStatusJson {
    pub rooms: Vec<RoomRef>,
    pub source: &'static str,
    /// 所选房间在本次候选里命中的成员数（过滤前口径）。
    pub matched: usize,
    pub unresolved: usize,
    pub definition_version: Option<String>,
    pub library_alignment_current: Option<bool>,
}

/// 面板 → 它围着 `rooms` 里的哪几间（下标）。一块面板通常只属于一间房，共用墙可能两边都算。
pub(crate) type PanelRooms = HashMap<RefnoEnum, Vec<usize>>;

/// 成员集 + 成员 → 房间下标 + 判不出归属的条数：两条解算路径的共同产物。
pub(crate) struct Members {
    pub set: HashSet<RefU64>,
    pub rooms_of: HashMap<RefU64, Vec<usize>>,
    pub unresolved: usize,
}

fn push_room_index(rooms_of: &mut HashMap<RefU64, Vec<usize>>, member: RefU64, room_index: usize) {
    let slots = rooms_of.entry(member).or_default();
    if let Err(position) = slots.binary_search(&room_index) {
        slots.insert(position, room_index);
    }
}

/// 请求点到的房间 → 在册房间与它们的面板（面板 → 房间下标）。不在册的 refno → 400。
fn select_rooms(
    model: &ResidentRoomModel,
    requested: &BTreeSet<RefU64>,
) -> Result<(Vec<RoomRef>, PanelRooms), SpatialQueryError> {
    let mut rooms = Vec::with_capacity(requested.len());
    let mut panels: PanelRooms = HashMap::new();
    for refno in requested {
        let Some(room) = model.map.rooms.iter().find(|room| room.room.refno() == *refno) else {
            return Err(SpatialQueryError::BadRequest(format!(
                "rooms 里的 {} 不是在册房间（名字不含房间关键词或房间号不合规），可先 GET /api/v1/spatial/rooms 看清单",
                RefnoEnum::from(*refno).to_pdms_str()
            )));
        };
        let room_index = rooms.len();
        rooms.push(RoomRef {
            refno: refno.to_string(),
            room_num: room.room_num.clone(),
        });
        for panel in &room.panels {
            let slots = panels.entry(*panel).or_default();
            if !slots.contains(&room_index) {
                slots.push(room_index);
            }
        }
    }
    Ok((rooms, panels))
}

fn dedup(candidates: &[RefU64]) -> Vec<RefU64> {
    let mut seen = HashSet::new();
    candidates
        .iter()
        .copied()
        .filter(|refno| seen.insert(*refno))
        .collect()
}

/// 读透形态：候选的投影记录 → 常驻模型算归属（记录从全局 `ModelMemoryStore` 取，算法在 [`members_from_records`]）。
fn resolve_members_in_memory(
    model: Arc<ResidentRoomModel>,
    panels: &PanelRooms,
    candidates: &[RefU64],
) -> anyhow::Result<Members> {
    let packed = candidates
        .iter()
        .map(|refno| refno.0 as i64)
        .collect::<BTreeSet<i64>>();
    let records =
        crate::fast_model::model_memory_store::ModelMemoryStore::global().room_records_for_members(&packed);
    members_from_records(model, crate::options::meshes_dir(), panels, candidates, &records)
}

/// 纯算法部分：`records` 是候选的投影记录（可以不全——没有记录的候选计入 `unresolved`），`panels` 是所选房间的面板
/// （面板 → 房间下标）。整批算不出来（某条记录盒子不可用之类）就退到逐条算，算不出的那几条计入 `unresolved`——一条坏记录
/// 不该让整次查询 500。
pub(crate) fn members_from_records(
    model: Arc<ResidentRoomModel>,
    mesh_dir: std::path::PathBuf,
    panels: &PanelRooms,
    candidates: &[RefU64],
    records: &[crate::fast_model::model_memory_store::ProjectedGeometry],
) -> anyhow::Result<Members> {
    let present = records
        .iter()
        .map(|record| record.source_refno)
        .map(|source| crate::data_interface::generation_root::refno_from_e3d(source).refno())
        .collect::<HashSet<_>>();
    let mut unresolved = candidates.iter().filter(|refno| !present.contains(refno)).count();
    if records.is_empty() {
        return Ok(Members {
            set: HashSet::new(),
            rooms_of: HashMap::new(),
            unresolved,
        });
    }
    let calculator = MemoryRoomCalculator::new(model, mesh_dir)?;
    let edges = match calculator.calculate_for_root(records) {
        Ok(edges) => edges,
        Err(_) => {
            let mut edges = Vec::new();
            let mut failed = HashSet::new();
            for record in records {
                match calculator.calculate_for_root(std::slice::from_ref(record)) {
                    Ok(mut some) => edges.append(&mut some),
                    Err(_) => {
                        failed.insert(
                            crate::data_interface::generation_root::refno_from_e3d(record.source_refno).refno(),
                        );
                    }
                }
            }
            unresolved += failed.len();
            edges
        }
    };
    let mut set = HashSet::new();
    let mut rooms_of = HashMap::new();
    for edge in edges {
        let Some(room_indices) = panels.get(&edge.panel) else {
            continue;
        };
        let member = edge.member.refno.refno();
        set.insert(member);
        for room_index in room_indices {
            push_room_index(&mut rooms_of, member, *room_index);
        }
    }
    Ok(Members {
        set,
        rooms_of,
        unresolved,
    })
}

fn parse_record_id(id: &str) -> Option<RefU64> {
    id.trim_matches(|c| c == '⟨' || c == '⟩' || c == '`').parse::<RefU64>().ok()
}

/// 摄入 / 落盘形态：`room_relate` 边里 `in ∈ 所选面板` 的 `out`，连同 `in` 一起取，好知道每个成员挂在哪几间房下。
async fn resolve_members_durable(panels: &PanelRooms) -> anyhow::Result<Members> {
    if panels.is_empty() {
        return Ok(Members {
            set: HashSet::new(),
            rooms_of: HashMap::new(),
            unresolved: 0,
        });
    }
    let mut keys = panels.keys().map(|panel| panel.to_pe_key()).collect::<Vec<_>>();
    keys.sort();
    let sql = format!(
        "SELECT record::id(in) AS panel, record::id(out) AS member FROM room_relate WHERE in IN [{}];",
        keys.join(", ")
    );
    #[derive(serde::Deserialize)]
    struct Row {
        panel: String,
        member: String,
    }
    let mut response = aios_core::SUL_DB.query(sql).await?.check()?;
    let rows: Vec<Row> = response.take(0)?;
    let mut set = HashSet::new();
    let mut rooms_of = HashMap::new();
    for row in &rows {
        let Some(member) = parse_record_id(&row.member) else {
            continue;
        };
        set.insert(member);
        let Some(room_indices) = parse_record_id(&row.panel)
            .and_then(|panel| panels.get(&RefnoEnum::from(panel)))
        else {
            continue;
        };
        for room_index in room_indices {
            push_room_index(&mut rooms_of, member, *room_index);
        }
    }
    Ok(Members {
        set,
        rooms_of,
        unresolved: 0,
    })
}

/// 候选所在库的房间边是否都对齐到当前代（`e3d.room.lookup` 的第 2 层判据）；只在落盘形态下作数。
async fn library_alignment_current(dbnums: &BTreeSet<u32>) -> Option<bool> {
    if !crate::options::store_mode().is_durable() {
        return None;
    }
    let mut all_current = true;
    for dbnum in dbnums {
        let generation = match crate::data_interface::dbnum_state::DbnumState::read(*dbnum).await {
            // `model_room_generation` 只在整库重算全过后才回盖本代，自带证据——不叠加
            // `model_sesno > 0`（issue #56：不跑模型管线的节点该格恒 0，但房间边是真的算齐了）。
            Ok(Some(state)) => state.model_room_generation,
            _ => None,
        };
        if !crate::query_service::library_room_alignment_is_current(generation.as_deref()) {
            all_current = false;
        }
    }
    Some(all_current)
}

/// 只核「房间体制可用 + 请求的房间都在册」，不解成员：树路由在扫树**之前**先过这一道，好让「不在册 → 400」
/// 排在「中心构件没有盒 → 404」前面（`rooms/{refno}/tree` 的房间既是范围又是过滤，不在册的房间自然也没有盒，
/// 先扫会答成 404）。`nearby` 不走这里——它的错误顺序不动。
pub fn ensure_rooms_registered(requested: &BTreeSet<RefU64>) -> Result<(), SpatialQueryError> {
    let availability = rooms_availability();
    let Some(model) = availability.model else {
        return Err(SpatialQueryError::RoomsUnavailable {
            status: availability.status,
            reason: availability.reason,
        });
    };
    select_rooms(&model, requested).map(|_| ())
}

pub async fn resolve_tree_room_scope(
    requested: Option<&BTreeSet<RefU64>>,
    candidates: &[RefU64],
    attrs: &dyn ElementAttributes,
) -> Result<RoomScope, SpatialQueryError> {
    if let Some(rooms) = requested {
        return resolve_room_scope(rooms, candidates, attrs).await;
    }
    let availability = rooms_availability();
    let mut scope = if let Some(model) = availability.model {
        let rooms = model.map.rooms.iter().map(|room| room.room.refno()).collect();
        resolve_room_scope(&rooms, candidates, attrs).await?
    } else {
        RoomScope {
            rooms: Vec::new(),
            members: HashSet::new(),
            member_rooms: HashMap::new(),
            unresolved: dedup(candidates).len(),
            source: "unavailable",
            definition_version: None,
            library_alignment_current: None,
        }
    };
    let unassigned = scope.rooms.len();
    for refno in dedup(candidates) {
        let rooms = scope.member_rooms.entry(refno).or_default();
        if rooms.is_empty() {
            rooms.push(unassigned);
        }
        scope.members.insert(refno);
    }
    scope.rooms.push(RoomRef {
        refno: "unassigned".to_string(),
        room_num: "未归属房间".to_string(),
    });
    Ok(scope)
}

/// `rooms=` 的解算入口：`requested` 是参数里的房间 refno，`candidates` 是这次扫出来的候选 refno（可重复）。
pub async fn resolve_room_scope(
    requested: &BTreeSet<RefU64>,
    candidates: &[RefU64],
    attrs: &dyn ElementAttributes,
) -> Result<RoomScope, SpatialQueryError> {
    let availability = rooms_availability();
    let Some(model) = availability.model else {
        return Err(SpatialQueryError::RoomsUnavailable {
            status: availability.status,
            reason: availability.reason,
        });
    };
    let (rooms, panels) = select_rooms(&model, requested)?;
    let definition_version = Some(model.input_version());
    let candidates = dedup(candidates);
    if candidates.is_empty() {
        return Ok(RoomScope {
            rooms,
            members: HashSet::new(),
            member_rooms: HashMap::new(),
            unresolved: 0,
            source: if crate::options::data_face().is_read_through() {
                SOURCE_MEMORY
            } else {
                SOURCE_DURABLE
            },
            definition_version,
            library_alignment_current: None,
        });
    }
    if crate::options::data_face().is_read_through() {
        let panels_for_task = panels.clone();
        let members = tokio::task::spawn_blocking(move || {
            resolve_members_in_memory(model, &panels_for_task, &candidates)
        })
        .await
        .map_err(|join| SpatialQueryError::Internal(anyhow::anyhow!("room scope task aborted: {join}")))?
        .map_err(SpatialQueryError::Internal)?;
        return Ok(RoomScope {
            rooms,
            members: members.set,
            member_rooms: members.rooms_of,
            unresolved: members.unresolved,
            source: SOURCE_MEMORY,
            definition_version,
            library_alignment_current: None,
        });
    }
    let members = resolve_members_durable(&panels)
        .await
        .map_err(SpatialQueryError::Internal)?;
    let dbnums = candidates
        .iter()
        .filter_map(|refno| attrs.dbnum_of(*refno))
        .collect::<BTreeSet<u32>>();
    Ok(RoomScope {
        rooms,
        members: members.set,
        member_rooms: members.rooms_of,
        unresolved: 0,
        source: SOURCE_DURABLE,
        definition_version,
        library_alignment_current: library_alignment_current(&dbnums).await,
    })
}

/// 单测 / 纯函数用：不经房间体制，直接由「所选房间 + 已知成员边」拼一个 scope。每个成员都记成属于第一间房
/// （`rooms` 为空则不记）；要指定成员挂在哪几间房用 [`room_scope_with_member_rooms`]。
pub fn room_scope_from_members(
    rooms: Vec<RoomRef>,
    members: impl IntoIterator<Item = RefU64>,
    unresolved: usize,
) -> RoomScope {
    let first_room = (!rooms.is_empty()).then_some(0usize);
    let members = members.into_iter().collect::<HashSet<_>>();
    let member_rooms = first_room
        .map(|index| members.iter().map(|member| (*member, vec![index])).collect())
        .unwrap_or_default();
    RoomScope {
        rooms,
        members,
        member_rooms,
        unresolved,
        source: SOURCE_MEMORY,
        definition_version: None,
        library_alignment_current: None,
    }
}

/// 单测 / 纯函数用：成员带上「属于 `rooms` 里的哪几间（下标）」。
pub fn room_scope_with_member_rooms(
    rooms: Vec<RoomRef>,
    member_rooms: impl IntoIterator<Item = (RefU64, Vec<usize>)>,
    unresolved: usize,
) -> RoomScope {
    let mut members = HashSet::new();
    let mut rooms_of: HashMap<RefU64, Vec<usize>> = HashMap::new();
    for (member, indices) in member_rooms {
        members.insert(member);
        for index in indices {
            push_room_index(&mut rooms_of, member, index);
        }
    }
    RoomScope {
        rooms,
        members,
        member_rooms: rooms_of,
        unresolved,
        source: SOURCE_MEMORY,
        definition_version: None,
        library_alignment_current: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::{RoomPanelMap, RoomPanels};

    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_013, seq)
    }

    #[derive(Default)]
    struct TableAttrs {
        dbnums: HashMap<RefU64, u32>,
        names: HashMap<RefU64, String>,
    }

    impl ElementAttributes for TableAttrs {
        fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
            self.dbnums.get(&refno).copied()
        }
        fn name_of(&self, refno: RefU64) -> Option<String> {
            self.names.get(&refno).cloned()
        }
    }

    fn rooms() -> Vec<RoomPanels> {
        vec![
            RoomPanels {
                room: RefnoEnum::from(refno(2)),
                room_num: "B202".into(),
                panels: vec![RefnoEnum::from(refno(20)), RefnoEnum::from(refno(21))],
            },
            RoomPanels {
                room: RefnoEnum::from(refno(1)),
                room_num: "A101".into(),
                panels: vec![RefnoEnum::from(refno(10))],
            },
            RoomPanels {
                room: RefnoEnum::from(refno(3)),
                room_num: "A101".into(),
                panels: vec![],
            },
        ]
    }

    #[test]
    fn room_list_is_sorted_by_room_num_then_refno_and_carries_name_dbnum_panel_count() {
        let attrs = TableAttrs {
            dbnums: HashMap::from([(refno(1), 4_000_000_013)]),
            names: HashMap::from([(refno(1), "/A101-RM".to_string())]),
        };
        let items = room_list_items(&rooms(), &attrs);
        assert_eq!(
            items.iter().map(|item| item.room_num.as_str()).collect::<Vec<_>>(),
            ["A101", "A101", "B202"]
        );
        assert_eq!(items[0].refno, refno(1).to_string());
        assert_eq!(items[0].name.as_deref(), Some("/A101-RM"));
        assert_eq!(items[0].dbnum, Some(4_000_000_013));
        assert_eq!(items[0].panel_count, 1);
        assert_eq!(items[1].refno, refno(3).to_string(), "同房间号按 refno 兜底");
        assert_eq!((items[1].name.as_deref(), items[1].dbnum), (None, None), "答不出就是 null");
        assert_eq!(items[2].panel_count, 2);
    }

    #[test]
    fn selecting_rooms_unions_their_panels_and_rejects_unknown_refnos() {
        let model = ResidentRoomModel::from_projection(
            crate::fast_model::room_resident::ResidentManifest {
                form: crate::fast_model::room_resident::ResidentForm::Latest,
                key: "test".into(),
            },
            vec![],
            RoomPanelMap {
                rooms: rooms(),
                all_panels: HashSet::new(),
            },
            &[],
            Arc::<crate::fast_model::room_resident::MeshCache>::default(),
        );
        let (picked, panels) =
            select_rooms(&model, &BTreeSet::from([refno(1), refno(2)])).expect("both rooms are registered");
        assert_eq!(
            picked.iter().map(|room| room.room_num.as_str()).collect::<Vec<_>>(),
            ["A101", "B202"],
            "按请求里的 refno 序（BTreeSet）"
        );
        assert_eq!(panels.len(), 3);
        assert_eq!(panels[&RefnoEnum::from(refno(10))], vec![0], "A101 的面板指回第 0 间");
        assert_eq!(panels[&RefnoEnum::from(refno(21))], vec![1], "B202 的面板指回第 1 间");
        let error = select_rooms(&model, &BTreeSet::from([refno(99)])).expect_err("unregistered room");
        assert!(matches!(error, SpatialQueryError::BadRequest(message) if message.contains("不是在册房间")));
    }

    #[test]
    fn scope_warnings_name_the_unresolved_count_and_stale_alignment() {
        let mut scope = room_scope_from_members(
            vec![RoomRef {
                refno: refno(1).to_string(),
                room_num: "A101".into(),
            }],
            [refno(100), refno(101)],
            3,
        );
        assert!(scope.contains(refno(100)));
        assert!(!scope.contains(refno(102)));
        let warnings = scope.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("3 个候选"));
        scope.library_alignment_current = Some(false);
        assert_eq!(scope.warnings().len(), 2);
        let status = scope.status_json();
        assert_eq!((status.matched, status.unresolved), (2, 3));
        assert_eq!(status.rooms[0].room_num, "A101");
        scope.unresolved = 0;
        scope.library_alignment_current = Some(true);
        assert!(scope.warnings().is_empty());
    }

    /// 读透路径的合成用例：一块 1000³ 的在册面板（房间 R1），三条候选——一条整个在里面、一条整个在外面、一条没有投影记录。
    /// 期望：里面那条是成员、外面那条不是、没记录那条计入 `unresolved`；另一间房（面板不含这一块）的面板集一个都不命中。
    #[test]
    fn memory_path_marks_members_inside_the_room_and_counts_candidates_without_records() {
        use crate::fast_model::model_memory_store::{ProjectedGeometry, ProjectedShape};
        use crate::fast_model::room_model::box_mesh_for_test;
        use crate::fast_model::room_resident::{MeshCache, ResidentForm, ResidentManifest};
        use glam::Vec3;

        let dir = tempfile::tempdir().unwrap();
        box_mesh_for_test(Vec3::ZERO, Vec3::splat(1000.))
            .ser_to_file(&dir.path().join("panel.mesh"))
            .unwrap();
        box_mesh_for_test(Vec3::ZERO, Vec3::splat(1.))
            .ser_to_file(&dir.path().join("unit.mesh"))
            .unwrap();

        let record = |id: u32, mesh: &str, min: f32, max: f32, scale: f32| ProjectedGeometry {
            record_id: format!("r{id}"),
            geometry_id: e3d_model::elmodl::GeometryId::Element {
                refno: e3d_io::refno::RefNo::new(4_000_000_013, id).to_string(),
            },
            source_refno: e3d_io::refno::RefNo::new(4_000_000_013, id),
            dbnum: 4_000_000_013,
            source_sesno: 1,
            noun: "EQUI".into(),
            mesh_id: mesh.into(),
            world_transform: glam::DMat4::from_scale_rotation_translation(
                glam::DVec3::splat(scale as f64),
                glam::DQuat::IDENTITY,
                glam::DVec3::splat(min as f64),
            ),
            world_bounds: [[min; 3], [max; 3]],
            ancestors: vec![],
            shape: ProjectedShape::Baked,
            tube: None,
        };
        let panel = RefnoEnum::from(refno(10));
        let map = RoomPanelMap {
            rooms: vec![
                RoomPanels {
                    room: RefnoEnum::from(refno(1)),
                    room_num: "R1".into(),
                    panels: vec![panel],
                },
                RoomPanels {
                    room: RefnoEnum::from(refno(2)),
                    room_num: "R2".into(),
                    panels: vec![RefnoEnum::from(refno(11))],
                },
            ],
            all_panels: HashSet::from([panel, RefnoEnum::from(refno(11))]),
        };
        let panel_record = record(10, "panel", 0., 1000., 1.);
        let model = Arc::new(ResidentRoomModel::from_projection(
            ResidentManifest {
                form: ResidentForm::Latest,
                key: "test".into(),
            },
            vec![],
            map,
            &[panel_record],
            Arc::<MeshCache>::default(),
        ));

        // 30：200..400 整个在面板里；31：5000..5200 整个在外面；32：没有记录。
        let inside = record(30, "unit", 200., 400., 200.);
        let outside = record(31, "unit", 5000., 5200., 200.);
        let candidates = [refno(30), refno(31), refno(32), refno(30)];
        let members = members_from_records(
            model.clone(),
            dir.path().to_path_buf(),
            &HashMap::from([(panel, vec![0usize])]),
            &candidates,
            &[inside.clone(), outside.clone()],
        )
        .expect("room calculation");
        assert_eq!(members.set, HashSet::from([refno(30)]));
        assert_eq!(members.rooms_of, HashMap::from([(refno(30), vec![0])]), "成员经面板指回它所属的房间下标");
        assert_eq!(members.unresolved, 1, "只有 32 没有记录；重复给的 30 有记录、不算（调用方本就先去重）");

        // 另一间房的面板集：同一批记录一个都不命中。
        let other = members_from_records(
            model,
            dir.path().to_path_buf(),
            &HashMap::from([(RefnoEnum::from(refno(11)), vec![1usize])]),
            &[refno(30), refno(31)],
            &[inside, outside],
        )
        .expect("room calculation");
        assert!(other.set.is_empty());
        assert!(other.rooms_of.is_empty());
        assert_eq!(other.unresolved, 0);
    }

    #[test]
    fn scope_helpers_record_which_rooms_each_member_belongs_to() {
        let rooms = vec![
            RoomRef {
                refno: refno(1).to_string(),
                room_num: "A101".into(),
            },
            RoomRef {
                refno: refno(2).to_string(),
                room_num: "B202".into(),
            },
        ];
        let scope = room_scope_from_members(rooms.clone(), [refno(100)], 0);
        assert_eq!(scope.member_rooms[&refno(100)], vec![0], "缺省记成第一间");

        let scope = room_scope_with_member_rooms(
            rooms,
            [(refno(100), vec![1, 0, 1]), (refno(101), vec![1])],
            0,
        );
        assert_eq!(scope.members, HashSet::from([refno(100), refno(101)]));
        assert_eq!(scope.member_rooms[&refno(100)], vec![0, 1], "下标升序去重");
        assert_eq!(scope.member_rooms[&refno(101)], vec![1]);
        assert_eq!(scope.status_json().matched, 2);
    }

    #[test]
    fn durable_ids_parse_with_or_without_surreal_brackets() {
        for text in ["4000000013_5", "⟨4000000013_5⟩", "`4000000013_5`"] {
            let parsed = text
                .trim_matches(|c| c == '⟨' || c == '⟩' || c == '`')
                .parse::<RefU64>()
                .expect("refno parses");
            assert_eq!(parsed, refno(5));
        }
    }
}
