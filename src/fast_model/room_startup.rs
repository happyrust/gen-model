//! 房间启动对比与应用——常驻房间模型 P3 的后半截（共识 d-42；
//! `docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md` §4）。
//!
//! 用户口径「启动时扫描全部房间数据，与库里已有的房间信息对版本，有增量就重算变了的房间、
//! 删旧关系写新关系」四步里，前两步在 `room_resident`（扫描 = `initialize_at_startup`，
//! 对版本 = `plan_startup` 纯函数），后两步在这里：
//!
//! - [`startup_room_reconcile`]：编排。止血口 / 空树两道门照旧最强 → 持 `lock_spatial_serial`
//!   → `ensure_current` 取此刻的模型 → 读参照 → 需要时按参照令牌从分片合成旧视图 →
//!   `plan_startup` → 按臂处置。全量降为显式入口（无参照 / 同键不等 / 参照不可读 / 持久
//!   `rebuild_required` / `AIOS_ROOM_FULL_REBUILD=1`），**交回 `lib.rs`** 在 `startup_autorun`
//!   门后跑——对比本身不受它（对比是定向的，成本随变更量走）。
//! - [`apply_startup_plan`]：应用。删 → 重算 → 改列，每块面板 / 每间房一个事务，单块失败不拦其余。
//! - [`settle_full_rebuild`]：写点 ②——显式全量全部成功之后写参照。
//! - [`record_startup_compare`] / [`startup_health`]：回执，进程内一份给 `/health`。
//!
//! 判据（别人不照做算错；源码钉在本文件与 `room_resident.rs` 的测试里）：
//! 1. 删 / 写只经 `room_model::recalc_panel_membership_for_consumption`（面板不在新视图里，它自己
//!    落进「已不在册」分支清边），改列只经 `room_resident::render_renumber_statements`。本文件的
//!    生产代码不另写一条删除或插入语句。
//! 2. 参照只在「边 == 这份拓扑」成立的瞬间写：对比应用全部成功、或显式全量全部成功。`NoGeometry`
//!    的面板没算，也挡参照——它们的边与新拓扑不一致（多半来自未监听的库，Q20 (a)），下次启动再比。
//! 3. 对比与应用全程持 `lock_spatial_serial`、不带 `RoomWriteGuard`（与 `build_room_relations`
//!    同口径，不与任务的 revision 争）。
//! 4. 本文件不认识 `watch_scope`（源码钉 `room_files_never_consult_the_watch_scope`）：限定域圈的
//!    是数据批次，房间是参照系。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;
use aios_core::options::DbOption;
use serde_json::{Value, json};

use crate::fast_model::room_resident::{
    self, ResidentForm, ResidentManifest, ResidentReference, ResidentRoomModel, RoomRenumber,
    StartupRoomPlan, TopologyDiff,
};
use crate::fast_model::room_topology::RoomHierarchy;

type Db = surrealdb::Surreal<surrealdb::engine::any::Any>;

// ---------------------------------------------------------------------------
// 应用（§4.4）
// ---------------------------------------------------------------------------

/// diff 里三类各有多少——回执里「删 N / 重算 M / 改列 K 间 P 块」的分母。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PlannedCounts {
    pub removed: usize,
    pub recalc: usize,
    pub renumbered_rooms: usize,
    pub renumbered_panels: usize,
}

impl PlannedCounts {
    pub(crate) fn of(diff: &TopologyDiff) -> Self {
        Self {
            removed: diff.removed_panels.len(),
            recalc: diff.recalc_panels.len(),
            renumbered_rooms: diff.renumbered_rooms.len(),
            renumbered_panels: diff
                .renumbered_rooms
                .iter()
                .map(|room| room.panels.len())
                .sum(),
        }
    }
}

/// [`apply_startup_plan`] 的回执。三类各自计数；没算的（无几何 / 不可用）与失败的逐条留着——
/// 参照能不能写看 `unusable` 与 `failures` 是不是都空（`no_geometry` 不挡，D-A (a)）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StartupApplyOutcome {
    pub planned: PlannedCounts,
    /// 旧 \ 新：归属边已清空的面板数。
    pub removed: usize,
    /// 边已按新视图重写的面板数。
    pub recalculated: usize,
    /// 在册却**一条几何行都没有**（从未生成）、这次没算的面板，按**文件** dbnum 分组（与
    /// `ResidentStats::missing_panels_by_dbnum` 同口径）。边不动。**不挡参照**
    /// （共识 D-A (a)，2026-09-08）：没有几何就没有边可陈旧，几何到来时随该库任务的房间段
    /// 整库重算（ADR-072 D4）。
    pub no_geometry: BTreeMap<u32, Vec<RefnoEnum>>,
    /// 在册、有几何行、却有一行世界包围盒不可用的面板（`ResidentRoomModel::unusable_panels`），
    /// 按文件 dbnum 分组。这块面板的几何不可信、存量边可能已陈旧：**挡参照**，等几何修好后
    /// 随该库任务的房间段重算。
    pub unusable: BTreeMap<u32, Vec<RefnoEnum>>,
    /// 逐块 / 逐间失败（覆盖率闸门、网格、写库）。边不动。
    pub failures: Vec<String>,
    /// **这一轮没能按新拓扑算过、且存量边可能因此陈旧的面板**——不可用的、删失败的、重算失败的、
    /// 改列失败那间房里的面板，一视同仁（M2）：它们的边与新拓扑不一致。这一摊是**进程内的尾巴**：
    /// 它挡住参照（[`StartupApplyOutcome::all_settled`]），参照不前进，下次启动同一份 diff 再比一次；
    /// 边本身由该库任务的房间段整库重算收敛（ADR-072 D4，房间随任务算），不再另排 durable 待办行
    /// （第二条异步账随 ADR-072 R4 退役）。从未生成的（`no_geometry`）不在这一摊（D-A (a)）。
    pub unsettled: BTreeSet<RefnoEnum>,
    pub renumbered_rooms: usize,
    pub renumbered_panels: usize,
    pub elapsed_ms: u128,
}

impl StartupApplyOutcome {
    pub(crate) fn no_geometry_count(&self) -> usize {
        self.no_geometry.values().map(Vec::len).sum()
    }

    pub(crate) fn unusable_count(&self) -> usize {
        self.unusable.values().map(Vec::len).sum()
    }

    /// 参照能不能写：有一块**可能有陈旧边**却没算成的都不行——参照说的是「边与这份拓扑一致」。
    /// 从未生成的面板（`no_geometry`）没有边，不在此列（D-A (a)）。
    pub(crate) fn all_settled(&self) -> bool {
        self.failures.is_empty() && self.unusable.is_empty()
    }

    /// 参照没写的原因；全部成功时 `None`。
    pub(crate) fn blocking_reason(&self) -> Option<String> {
        if self.all_settled() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.unusable.is_empty() {
            parts.push(format!(
                "{} 块在册面板几何不可用没算，按库 {{{}}}",
                self.unusable_count(),
                self.unusable
                    .iter()
                    .map(|(dbnum, panels)| format!("{dbnum}: {}", panels.len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.failures.is_empty() {
            parts.push(format!(
                "{} 处失败 [{}]",
                self.failures.len(),
                self.failures.join("; ")
            ));
        }
        Some(parts.join("；"))
    }

    /// 回执那一段：`删 N / 重算 M（成功 a · 无几何 b · 不可用 u · 失败 c）/ 改列 K 间 P 块`；
    /// 有尾巴时末尾多一句「没算成的 n 块留作尾巴」。
    pub(crate) fn summary(&self) -> String {
        let recalc_failed = self
            .planned
            .recalc
            .saturating_sub(self.recalculated + self.no_geometry_count() + self.unusable_count());
        let tail = if self.unsettled.is_empty() {
            String::new()
        } else {
            format!(
                " / 没算成的 {} 块留作尾巴（挡参照，等该库任务的房间段重算）",
                self.unsettled.len()
            )
        };
        format!(
            "删 {}/{} / 重算 {}（成功 {} · 无几何 {} · 不可用 {} · 失败 {}）/ 改列 {}/{} 间 {}/{} 块{tail}",
            self.removed,
            self.planned.removed,
            self.planned.recalc,
            self.recalculated,
            self.no_geometry_count(),
            self.unusable_count(),
            recalc_failed,
            self.renumbered_rooms,
            self.planned.renumbered_rooms,
            self.renumbered_panels,
            self.planned.renumbered_panels,
        )
    }
}

/// [`split_recalc_by_geometry`] 的三摊。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecalcSplit {
    /// 常驻模型里有可用几何：这次算。
    pub to_recalc: Vec<RefnoEnum>,
    /// 一条几何行都没有（从未生成），按文件 dbnum 分组：不算、不挡参照（D-A (a)）。
    pub no_geometry: BTreeMap<u32, Vec<RefnoEnum>>,
    /// 有行却有一行包围盒不可用，按文件 dbnum 分组：不算、挡参照。
    pub unusable: BTreeMap<u32, Vec<RefnoEnum>>,
}

/// 重算集按「常驻模型里有没有可用几何」分**三**摊（纯函数）。没几何的这次不算、边不动、进回执
/// 点名（Q20 (a)）——`recalc_panel_membership_for_consumption` 读的是同一份几何（两种数据面
/// 都是实时行，ADR-072 D4），去了也只会 `NoGeometry` 上抛；在这里先分开，回执里
/// 「无几何」与「失败」才是两句话，不必从错误文本里猜。
///
/// 没几何再分一刀（D-A (a)）：**从未生成**（一条行都没有）与**有行却不可用**（包围盒坏，
/// `ResidentRoomModel::unusable_panels`）。前者没有边可陈旧，不挡参照；后者的存量边可能已经
/// 陈旧，挡参照。两者对归属计算一样都是「判不了就不写」。
pub(crate) fn split_recalc_by_geometry(
    model: &ResidentRoomModel,
    panels: &BTreeSet<RefnoEnum>,
) -> RecalcSplit {
    let mut split = RecalcSplit::default();
    for panel in panels {
        if model.panel(*panel).is_some() {
            split.to_recalc.push(*panel);
        } else if model.unusable_panels.contains(panel) {
            split
                .unusable
                .entry(model.file_of_panel(*panel))
                .or_default()
                .push(*panel);
        } else {
            split
                .no_geometry
                .entry(model.file_of_panel(*panel))
                .or_default()
                .push(*panel);
        }
    }
    split
}

/// 按 diff 删 / 重算 / 改列（§4.4）。**调用方已持 `lock_spatial_serial`**。
///
/// 三类按序处理，每块面板 / 每间房一个事务（与 `build_room_relations` 同一纪律：单块失败
/// 不拦其余，失败逐条收集）：
///
/// 1. 删（`removed_panels`，从旧视图枚举）：对每块面板调 `recalc_panel_membership_for_consumption`；
///    面板不在新视图里，它自然落进「面板已不在册」分支——清它全部出边与 `room_panel_relate`，
///    日志「面板 X 已不在册，清空其房间归属」照旧打。不另写一条删除路径。
/// 2. 重算（`recalc_panels`）：同一个函数，面板在新视图里 → 覆盖率闸门（fail-closed）→ 成员
///    判定 → 先清后写（带新 `room_num`；面板从 A 间挪到 B 间时 A→板的旧边在这一步掉）。
///    无几何 / 不可用的先分出去不算（[`split_recalc_by_geometry`]）；闸门 / 网格 / 写库失败进
///    `failures`。
/// 3. 改列（`renumbered_rooms`）：`render_renumber_statements` 一间房一个事务，边 id 不动。
///
/// 顺序删 → 重算 → 改列只为日志好读；三类按面板 / 房间分键、互不重叠，换序不影响结果。
///
/// **没算成、且存量边可能因此陈旧的那一摊留作尾巴**（M2）：三类里失败的、以及几何不可用那一摊，
/// 收进 `unsettled`。这一摊只有一个落点——进程内回执（[`record_startup_compare`]）：它挡住参照
/// （写点 ①），参照不前进，下次启动同一份 diff 再比一次；边本身由该库任务的房间段整库重算收敛
/// （ADR-072 D4，房间随任务算）。此前它还会折成 durable 待办行排进第二条异步账，那条账随
/// ADR-072 R4 整体退役，这里**不再入队、不写库**。**从未生成**的面板（`no_geometry`）不在这一摊
/// （D-A (a)，2026-09-08）：它没有边可陈旧，几何到来时同样随任务算。
pub(crate) async fn apply_startup_plan(
    db_option: &DbOption,
    model: &ResidentRoomModel,
    diff: &TopologyDiff,
) -> StartupApplyOutcome {
    use crate::fast_model::room_model::recalc_panel_membership_for_consumption;

    let started = Instant::now();
    let mut outcome = StartupApplyOutcome {
        planned: PlannedCounts::of(diff),
        ..Default::default()
    };
    for panel in &diff.removed_panels {
        match recalc_panel_membership_for_consumption(db_option, &model.map, *panel, Some(model))
            .await
        {
            Ok(_) => outcome.removed += 1,
            Err(error) => {
                outcome
                    .failures
                    .push(format!("{panel} 清空归属失败: {error:#}"));
                outcome.unsettled.insert(*panel);
            }
        }
    }
    let RecalcSplit {
        to_recalc,
        no_geometry,
        unusable,
    } = split_recalc_by_geometry(model, &diff.recalc_panels);
    // 几何不可用与重算失败一视同仁：两者都是「这块面板有几何、边却没有按新拓扑算过」。
    // 从未生成的那一摊（`no_geometry`，未监听库的常态，Q20 (a)）只进回执点名——它没有边可
    // 陈旧，不挡参照（D-A (a)）。
    outcome
        .unsettled
        .extend(unusable.values().flatten().copied());
    outcome.no_geometry = no_geometry;
    outcome.unusable = unusable;
    for panel in to_recalc {
        match recalc_panel_membership_for_consumption(db_option, &model.map, panel, Some(model))
            .await
        {
            Ok(_) => outcome.recalculated += 1,
            Err(error) => {
                outcome
                    .failures
                    .push(format!("{panel} 重算归属失败: {error:#}"));
                outcome.unsettled.insert(panel);
            }
        }
    }
    for renumber in &diff.renumbered_rooms {
        match apply_renumber_on(&aios_core::SUL_DB, renumber).await {
            Ok(()) => {
                outcome.renumbered_rooms += 1;
                outcome.renumbered_panels += renumber.panels.len();
            }
            Err(error) => {
                outcome.failures.push(format!(
                    "房间 {} {}→{} 改列失败: {error:#}",
                    renumber.room, renumber.old_num, renumber.new_num
                ));
                // 改列失败那间房的面板改成整间重算：比只补一次 `UPDATE room_num` 多算，
                // 但改列本来就是重算的省略式，多算的那一遍收敛到同一批边（宁可多算一次）。
                outcome.unsettled.extend(renumber.panels.iter().copied());
            }
        }
    }
    // 尾巴不入队：`unsettled` 非空时 `all_settled()` 为假，参照不写；进程内那道闸
    // （[`startup_tail_of`]）就是这条线的守卫，边由该库任务的房间段整库重算收敛。
    outcome.elapsed_ms = started.elapsed().as_millis();
    outcome
}

/// 一间房的改列，一个事务：交集里每块面板的 `room_relate` 出边与 `room_panel_relate` 入边只改
/// `room_num`，边 id 不动、不重算。语句只来自 `render_renumber_statements`。`db` 显式传入，
/// `mem://` 门直接喂。
pub(crate) async fn apply_renumber_on(db: &Db, renumber: &RoomRenumber) -> anyhow::Result<()> {
    let statements = room_resident::render_renumber_statements(renumber);
    if statements.is_empty() {
        return Ok(());
    }
    db.query(format!(
        "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
        statements.join(" ")
    ))
    .await?
    .check()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 持久重建要求（臂表里的两行：`rebuild_required == true` / 对比标脏）
// ---------------------------------------------------------------------------

/// `room_build:main` 上的持久重建要求。读法与 `room_model::RoomBuildRecord` 同一条纪律：
/// 字段缺失或 `NONE` 都按「不要求」读——老二进制盖的章没有这两格。
#[derive(Debug, Default, serde::Deserialize)]
struct RebuildRequirement {
    #[serde(default)]
    rebuild_required: Option<bool>,
    #[serde(default)]
    rebuild_reason: Option<String>,
}

/// `Some(理由)` = 有持久重建要求（结构枚举失败 / 上次对比标的），只有成功的全量清它。
pub(crate) async fn read_rebuild_required_on(db: &Db) -> anyhow::Result<Option<String>> {
    let mut response = db
        .query("SELECT rebuild_required, rebuild_reason FROM room_build:main;")
        .await?
        .check()?;
    let rows: Vec<RebuildRequirement> = response.take(0)?;
    Ok(rows
        .into_iter()
        .next()
        .filter(|row| row.rebuild_required.unwrap_or(false))
        .map(|row| {
            row.rebuild_reason
                .unwrap_or_else(|| "房间结构枚举失败，需要全量重建".to_string())
        }))
}

/// 对比判出「库被动过」（同键不等）或「参照读不回来」时把理由落成持久要求：全量这一臂若被
/// `startup_autorun=false` 挡住，下次启动照样知道要跑。语句是 `room_model` 的同一条。
pub(crate) async fn mark_rebuild_required_on(db: &Db, reason: &str) -> anyhow::Result<()> {
    db.query(crate::fast_model::room_model::render_room_rebuild_required(
        Some(reason),
    ))
    .await?
    .check()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 回执：进程内一份给 /health（`record_startup_compare`，与 `startup_report` 同一存法）
// ---------------------------------------------------------------------------

/// 本进程最近一次启动房间对比的结局（`/health.room_build.resident.startup_compare`）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct StartupCompareReport {
    pub form: String,
    /// 计划摘要（`StartupRoomPlan::summary`），或没比的原因（`跳过：…`）。
    pub plan: String,
    pub removed: usize,
    pub recalculated: usize,
    /// 从未生成、这次没算的面板数：**不挡参照**（D-A (a)），只是点名。
    pub no_geometry: usize,
    /// 有几何却不可用、这次没算的面板数：挡参照。
    pub unusable: usize,
    pub failed: usize,
    pub renumbered_rooms: usize,
    pub renumbered_panels: usize,
    pub reference_written: bool,
    /// **本次启动对比给这个进程留下的尾巴**（M2 的进程内闸）。`None` = 没留尾巴：这一路的
    /// 归属边确实按当前拓扑算过（`Unchanged` 零写入，或 `Apply` 全部成功），启动侧之外的参照
    /// 写点可以放行。`Some(理由)` = 有没算成的目标、或者压根没比过（跳过 / 显式全量还没收尾），
    /// 拒写。
    ///
    /// 这一格是进程内的，谁也滤不掉；它只活在本进程——但重启会重新做一次对比（参照没写、
    /// 现状与参照的键仍不等 → 同一份 diff → 同一摊尾巴），所以「重启丢标志」不构成漏洞：
    /// 缺省值（没有回执 = 还没比过）本身就是拒写。尾巴不再另有 durable 待办行那道守卫
    /// （第二条异步账随 ADR-072 R4 退役）：边由该库任务的房间段整库重算收敛。
    pub unsettled: Option<String>,
    /// 参照没写的原因 / 补充说明。
    pub note: Option<String>,
    /// 走了显式全量臂时的理由；全量本体在 `lib.rs`，成败看 `room_build` 的章与
    /// `reference_written`（[`settle_full_rebuild`] 回填）。
    pub full_rebuild: Option<String>,
    pub at: String,
    pub elapsed_ms: u128,
}

impl StartupCompareReport {
    /// 缺省是**有尾巴**：只有明确走到「边 == 这份拓扑」的臂才把它清成 `None`。新增一条臂
    /// 而忘了表态时，默认落在保守那一边（拒写，下次启动多比一次）。
    fn new(form: ResidentForm, plan: String, elapsed_ms: u128) -> Self {
        Self {
            form: form.as_str().to_string(),
            plan: plan.clone(),
            removed: 0,
            recalculated: 0,
            no_geometry: 0,
            unusable: 0,
            failed: 0,
            renumbered_rooms: 0,
            renumbered_panels: 0,
            reference_written: false,
            unsettled: Some(plan),
            note: None,
            full_rebuild: None,
            at: chrono::Local::now().to_rfc3339(),
            elapsed_ms,
        }
    }

    fn with_apply(mut self, outcome: &StartupApplyOutcome) -> Self {
        self.removed = outcome.removed;
        self.recalculated = outcome.recalculated;
        self.no_geometry = outcome.no_geometry_count();
        self.unusable = outcome.unusable_count();
        self.failed = outcome.failures.len();
        self.renumbered_rooms = outcome.renumbered_rooms;
        self.renumbered_panels = outcome.renumbered_panels;
        // 参照写失败不算尾巴：边确实按这份拓扑算过了，只是那一行没落库——下一次参照写点正是它的重试。
        self.unsettled = outcome.blocking_reason();
        self
    }
}

/// 启动侧之外的参照写点要过的门（M2，纯函数）：本进程的启动对比留没留下尾巴。
///
/// `None` = 放行。`Some(理由)` = 拒写参照，两种：
/// 1. **没有回执** —— 本进程还没做过启动房间对比。这不是理论情形：worker
///    （`ensure_batch_worker`）起在 `run_cli` 的房间段**之前**，它的空闲轮完全可能先于房间段
///    跑完一轮。那时启动 diff 一块面板都还没应用，参照却会被推进到当前拓扑，停机期间
///    未监听库的全部变化就此静默丢失。
/// 2. **回执说有尾巴** —— 见 [`StartupCompareReport::unsettled`]。
pub(crate) fn startup_tail_of(report: Option<&StartupCompareReport>) -> Option<String> {
    match report {
        None => Some(
            "本进程还没做过启动房间对比（worker 的空闲轮起在房间段之前）：\
             启动 diff 尚未应用，此刻写参照等于宣称停机期间什么都没变"
                .to_string(),
        ),
        Some(report) => report.unsettled.clone(),
    }
}

/// [`startup_tail_of`] 的进程内取数版，供启动侧之外的参照写点判门。
pub(crate) fn startup_compare_tail() -> Option<String> {
    startup_tail_of(startup_compare_report().as_ref())
}

fn compare_report_slot() -> &'static RwLock<Option<StartupCompareReport>> {
    static REPORT: OnceLock<RwLock<Option<StartupCompareReport>>> = OnceLock::new();
    REPORT.get_or_init(|| RwLock::new(None))
}

/// 最近一次启动对比的结局；还没跑过就是 `None`。
pub(crate) fn startup_compare_report() -> Option<StartupCompareReport> {
    compare_report_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub(crate) fn record_startup_compare(report: StartupCompareReport) {
    *compare_report_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(report);
}

/// 显式全量收尾后回填「参照写了没」。没有在途回执（全量不是本模块判的）就不动。
///
/// 全量成功并写上参照 = 边就是按这份拓扑算的，尾巴一并清掉（其它参照写点从此可以放行）；没写成
/// （身份换了代 / 参照写失败 / 模型建不起来）照旧留着尾巴。全量被 `startup_autorun` 挡住或
/// 自己失败时，这个函数**根本不会被调**，`full()` 立的那句尾巴原样留着——正是要的。
fn note_full_rebuild_settled(reference_written: bool, note: Option<String>) {
    if let Some(report) = compare_report_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        report.reference_written = reference_written;
        if reference_written {
            report.unsettled = None;
        } else {
            report.unsettled =
                Some(note.clone().unwrap_or_else(|| {
                    "显式全量收尾没能写上参照，本进程按「有尾巴」处置".to_string()
                }));
        }
        report.note = note;
    }
}

/// 显式全量收尾时把「从未生成、这次没算」的面板数回填进回执的 `no_geometry` 一格（只点名，
/// 不动尾巴——D-A (a)）。没有在途回执就不动。
fn note_full_rebuild_geometry(without_geometry: usize) {
    if let Some(report) = compare_report_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        report.no_geometry = without_geometry;
    }
}

const REFERENCE_HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// `/health.room_build.resident` 多的两格（§4.7）：`reference`（读库，2 s 上限，与
/// `room_build_health_from` 同形）与 `startup_compare`（进程内）。
pub(crate) async fn startup_health() -> serde_json::Map<String, Value> {
    let reference = reference_health_from(
        REFERENCE_HEALTH_TIMEOUT,
        room_resident::read_resident_reference_on(&aios_core::SUL_DB),
    )
    .await;
    let mut map = serde_json::Map::new();
    map.insert("reference".to_string(), reference);
    map.insert(
        "startup_compare".to_string(),
        serde_json::to_value(startup_compare_report()).unwrap_or(Value::Null),
    );
    map
}

async fn reference_health_from<F>(timeout: Duration, read: F) -> Value
where
    F: std::future::Future<Output = anyhow::Result<Option<ResidentReference>>>,
{
    match tokio::time::timeout(timeout, read).await {
        Ok(Ok(Some(reference))) => reference_health_value(&reference),
        Ok(Ok(None)) => Value::Null,
        Ok(Err(error)) => json!({ "error": format!("读取启动参照失败: {error:#}") }),
        Err(_) => json!({
            "error": format!("读取启动参照超过 {} 秒", timeout.as_secs_f64())
        }),
    }
}

/// 参照那一格：令牌只给个数——整串令牌是 JSON 里嵌 JSON，`/health` 上没人读。
pub(crate) fn reference_health_value(reference: &ResidentReference) -> Value {
    json!({
        "form": reference.form,
        "key": reference.key,
        "digest": reference.digest,
        "tokens": reference.tokens.len(),
        "rooms": reference.rooms,
        "registered_panels": reference.registered_panels,
        "settled_at": reference.settled_at,
    })
}

// ---------------------------------------------------------------------------
// 编排（§4.2 / §4.3 / §4.6）
// ---------------------------------------------------------------------------

/// 房间段的裁决：对比已把房间段处置完，还是要 `lib.rs` 跑一次显式全量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupRoomVerdict {
    /// 到此为止：零写入 / diff 已应用（成败都进回执）/ 没清单或模型建不起来而止步 / 止血口或
    /// 空树跳过。
    Settled,
    /// 五条显式入口之一：`lib.rs` 在 `startup_autorun` 门后跑 `build_room_relations`，成功后调
    /// [`settle_full_rebuild`]。`expected` 是决策时模型的身份（拿不到模型时 `None`），收尾写参照
    /// 前要核对它没变。
    FullRebuild {
        reason: String,
        expected: Option<ResidentManifest>,
    },
}

/// 计划 → 处置（纯函数，臂表 §4.3 的落点）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanDisposition<'a> {
    /// 零写入，参照不动（D2：不刷 `settled_at`）。
    ZeroWrite,
    /// 按 diff 删 / 重算 / 改列；空 diff 也走这一臂（零写入 + 写参照）。
    ApplyDiff(&'a TopologyDiff),
    /// 显式全量。`mark_rebuild_required` 为真时先把理由落进 `room_build:main.rebuild_required`：
    /// 同键不等（库被动过）与参照不可读都不是「多算一次」能了的事，全量被 `startup_autorun`
    /// 挡住也要留下持久要求。
    FullRebuild {
        reason: String,
        mark_rebuild_required: bool,
    },
}

pub(crate) fn dispose(plan: &StartupRoomPlan) -> PlanDisposition<'_> {
    match plan {
        StartupRoomPlan::NoReference => PlanDisposition::FullRebuild {
            reason: "无参照（开关第一次打开 / 库刚建 / 形态换了）".to_string(),
            mark_rebuild_required: false,
        },
        StartupRoomPlan::Unchanged => PlanDisposition::ZeroWrite,
        StartupRoomPlan::Apply(diff) => PlanDisposition::ApplyDiff(diff),
        StartupRoomPlan::IntegrityMismatch { .. } | StartupRoomPlan::ReferenceUnreadable(_) => {
            PlanDisposition::FullRebuild {
                reason: plan.summary(),
                mark_rebuild_required: true,
            }
        }
    }
}

/// 旧视图只在会走到 diff 那一臂时才去读分片：形态同、摘要不等、键不等。别的臂零 I/O。
pub(crate) fn needs_old_view(reference: &ResidentReference, current: &ResidentRoomModel) -> bool {
    reference.form() == Some(current.manifest.form)
        && reference.digest != current.digest
        && reference.key != current.manifest.key
}

/// 一次性运维入口：`AIOS_ROOM_FULL_REBUILD=1`（`1/true/yes/on`，大小写不敏感）。
pub(crate) fn full_rebuild_requested_by_env() -> bool {
    std::env::var(crate::ROOM_FULL_REBUILD_ENV)
        .ok()
        .is_some_and(|raw| flag_is_on(&raw))
}

fn flag_is_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn short(text: &str) -> String {
    text.chars().take(12).collect()
}

/// 启动日志那一行的前半句：`{form} 参照 {key}@{digest}（{settled_at}）→ 现状 {key}@{digest}`。
fn describe_compare(
    form: ResidentForm,
    reference: Option<&ResidentReference>,
    current: &ResidentRoomModel,
) -> String {
    let reference = match reference {
        Some(reference) => format!(
            "参照 {}@{}（{}）",
            short(&reference.key),
            short(&reference.digest),
            reference.settled_at
        ),
        None => "无参照".to_string(),
    };
    format!(
        "{} {reference} → 现状 {}@{}",
        form.as_str(),
        short(&current.manifest.key),
        short(&current.digest)
    )
}

fn skip(form: ResidentForm, reason: String, started: Instant) -> StartupRoomVerdict {
    println!("[房间启动对比] 跳过（{reason}）：不动边、不写参照");
    record_startup_compare(StartupCompareReport::new(
        form,
        format!("跳过：{reason}"),
        started.elapsed().as_millis(),
    ));
    StartupRoomVerdict::Settled
}

fn full(
    form: ResidentForm,
    reason: String,
    expected: Option<ResidentManifest>,
    started: Instant,
) -> StartupRoomVerdict {
    let mut report = StartupCompareReport::new(form, reason.clone(), started.elapsed().as_millis());
    report.full_rebuild = Some(reason.clone());
    record_startup_compare(report);
    StartupRoomVerdict::FullRebuild { reason, expected }
}

/// 启动段的房间对比（§4.2 / §4.3）。`room_resident_model` 开着时 `lib.rs` 拿它替掉
/// `skip_startup_room_build` → `build_room_relations` 那一臂；外层三道门（`startup_data_ready` /
/// `spatial_compute` / `model_incremental`）照旧在 `lib.rs` 判。
///
/// 门：`AIOS_SKIP_STARTUP_ROOM_BUILD` 最强（对比与全量一起跳，运维止血口语义不变）；空间树
/// `ReadyEmpty` 照旧跳过并盖平凡章。`startup_autorun` **不**门对比——只门返回的
/// [`StartupRoomVerdict::FullRebuild`] 那一臂，在 `lib.rs` 判。这里也不读 `room_build` 的空间凭据
/// 对账（§4.6）：对比接管了「空间状态变了、房间没跟上」里拓扑那一半，几何那一半归该库任务的
/// 房间段（ADR-072 D4）；每次成功全量照旧盖章，只是章不再决定跑不跑。
///
/// 进锁后先 `ensure_current`：初始化装的那份模型对应初始化那一刻的清单，到房间段时被监听文件的
/// 首轮可能已发布、清单已变，身份键不同就整份重建（键没变是常路、零 I/O）。
pub(crate) async fn startup_room_reconcile(db_option: &DbOption) -> StartupRoomVerdict {
    let started = Instant::now();
    let face = crate::options::data_face();
    let form = ResidentForm::for_face(face);

    if std::env::var(crate::SKIP_STARTUP_ROOM_BUILD_ENV).is_ok() {
        return skip(
            form,
            format!("{} 已设置", crate::SKIP_STARTUP_ROOM_BUILD_ENV),
            started,
        );
    }
    if let Some(reason) =
        crate::vacuous_room_build_reason(crate::fast_model::spatial_state::current_state())
    {
        // 平凡也盖章：凭据没有比较对象的话 /health 会一直说「没有成功的全量房间重建凭据」。
        if let Err(error) = crate::fast_model::room_model::stamp_vacuous_room_build().await {
            println!("[房间启动对比] 空世界的凭据没盖上（下次启动会再对账一次）: {error:#}");
        }
        return skip(form, reason, started);
    }
    let Some(hierarchy) = RoomHierarchy::configured() else {
        return skip(
            form,
            "房间子系统需要 project_hd 或 project_hh 特性".to_string(),
            started,
        );
    };
    #[cfg(any(feature = "project_hd", feature = "project_hh"))]
    let match_room_fn = crate::fast_model::room_model::configured_match_room_fn();
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let match_room_fn = |_: &str| false;
    let room_key_words = db_option.get_room_key_word();

    // 换代、读参照、diff、写边、写参照落在同一段串行区：文件发布不会插在中间把清单换掉；
    // worker 的空闲轮与这里串行，对同一块面板各算一次也是幂等的。
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let mesh_dir = db_option.get_meshes_path();
    let model =
        room_resident::ensure_current(face, hierarchy, &room_key_words, match_room_fn, &mesh_dir)
            .await
            .map(|outcome| outcome.map(|outcome| outcome.model));
    let expected = model
        .as_ref()
        .ok()
        .and_then(|model| model.as_ref().map(|model| model.manifest.clone()));

    // 显式入口 ①：运维一次性口子，对比一概不看。
    if full_rebuild_requested_by_env() {
        return full(
            form,
            format!("{}=1", crate::ROOM_FULL_REBUILD_ENV),
            expected,
            started,
        );
    }
    // 显式入口 ②：持久要求（结构枚举失败 / 上次对比标的），只有成功的全量清它。
    match read_rebuild_required_on(&aios_core::SUL_DB).await {
        Ok(Some(reason)) => {
            return full(form, format!("持久重建要求：{reason}"), expected, started);
        }
        Ok(None) => {}
        Err(error) => {
            println!("[房间启动对比] 读 rebuild_required 失败（按没有要求继续对比）: {error:#}")
        }
    }
    let model = match model {
        Ok(Some(model)) => model,
        Ok(None) => {
            return skip(form, "摄入侧没有发布清单：无从对比".to_string(), started);
        }
        Err(error) => {
            eprintln!("[房间启动对比] 模型建不起来，本轮止步、存量边不动: {error:#}");
            return skip(form, format!("模型建不起来: {error:#}"), started);
        }
    };

    let reference = match room_resident::read_resident_reference_on(&aios_core::SUL_DB).await {
        Ok(reference) => reference,
        // 读不到参照与没有参照同判（今天的凭据对账也是「读不到一律跑」）：多跑一次全量远好过
        // 让一次查询故障静默变成房间永不收敛。
        Err(error) => {
            return full(
                form,
                format!("读启动参照失败（{error:#}）"),
                Some(model.manifest.clone()),
                started,
            );
        }
    };
    let old_view = match reference.as_ref() {
        Some(reference) if needs_old_view(reference, &model) => Some(
            room_resident::load_reference_map_on(
                &aios_core::SUL_DB,
                reference,
                hierarchy,
                &room_key_words,
                match_room_fn,
            )
            .await
            .map(|(_, map)| map)
            .map_err(|error| format!("{error:#}")),
        ),
        _ => None,
    };
    let plan = room_resident::plan_startup(
        reference.as_ref(),
        &model,
        old_view
            .as_ref()
            .map(|view| view.as_ref().map_err(Clone::clone)),
    );
    let header = describe_compare(form, reference.as_ref(), &model);
    match dispose(&plan) {
        PlanDisposition::ZeroWrite => {
            println!(
                "[房间启动对比] {header}：{}；参照不动；耗时 {} ms",
                plan.summary(),
                started.elapsed().as_millis()
            );
            let mut report =
                StartupCompareReport::new(form, plan.summary(), started.elapsed().as_millis());
            report.note = Some("参照摘要 == 现状，参照不动（D2）".to_string());
            // 零写入不是「没比」而是「比过、一样」：参照说的那份拓扑就是现状，边与它一致。
            // 这一臂没有尾巴，其它参照写点从此可以放行（它们此后只是把 `settled_at` 往前推）。
            report.unsettled = None;
            record_startup_compare(report);
            StartupRoomVerdict::Settled
        }
        PlanDisposition::FullRebuild {
            reason,
            mark_rebuild_required,
        } => {
            if mark_rebuild_required
                && let Err(error) = mark_rebuild_required_on(&aios_core::SUL_DB, &reason).await
            {
                println!(
                    "[房间启动对比] rebuild_required 没写进 room_build:main（本次照样走显式全量）: {error:#}"
                );
            }
            println!("[房间启动对比] {header}：{reason} → 显式全量");
            full(form, reason, Some(model.manifest.clone()), started)
        }
        PlanDisposition::ApplyDiff(diff) => {
            let outcome = apply_startup_plan(db_option, &model, diff).await;
            // 写点 ①：全部成功才写参照（`NoGeometry` 也挡）。
            let (reference_written, note) = if outcome.all_settled() {
                match room_resident::persist_reference(&model).await {
                    Ok(_) => (true, None),
                    Err(error) => (false, Some(format!("参照写失败: {error:#}"))),
                }
            } else {
                (false, outcome.blocking_reason())
            };
            let verdict = if reference_written {
                "参照已更新".to_string()
            } else {
                format!("参照未更新（{}）", note.as_deref().unwrap_or("未知"))
            };
            println!(
                "[房间启动对比] {header}：{}；{}；{verdict}；耗时 {} ms",
                plan.summary(),
                outcome.summary(),
                started.elapsed().as_millis()
            );
            let mut report =
                StartupCompareReport::new(form, plan.summary(), started.elapsed().as_millis())
                    .with_apply(&outcome);
            report.reference_written = reference_written;
            report.note = note;
            record_startup_compare(report);
            StartupRoomVerdict::Settled
        }
    }
}

/// 写点 ②（§4.5）：显式全量 `build_room_relations_with_view` 返回 `Ok(outcome)` 之后调。持
/// `lock_spatial_serial` 取此刻的模型；它的身份得与决策时那份一致（`expected`）——全量在锁外跑了
/// 十几秒，期间若清单换了代，全量依据的拓扑与此刻模型说的就不是同一份，不写（下次启动
/// `NoReference` → 再全量一次，多算不漏算）。决策时没拿到模型（`expected = None`）同理不写。
///
/// `outcome.without_geometry`（从未生成的面板）**不挡**这里的参照（D-A (a)），与写点 ① 对
/// `no_geometry` 同一口径——有行却不可用的面板在全量里是 `failures`，整轮 `Err`，根本到不了这里。
/// 它只随回执点名，`/health.startup_compare.no_geometry` 照记。
pub(crate) async fn settle_full_rebuild(
    db_option: &DbOption,
    expected: Option<&ResidentManifest>,
    outcome: &crate::fast_model::room_model::FullRebuildOutcome,
) -> bool {
    if !outcome.without_geometry.is_empty() {
        println!(
            "[房间全量] {} 块在册面板从未生成几何、这次没算（不挡参照：没有边可陈旧，几何到来时由发布事务补算）",
            outcome.without_geometry.len()
        );
    }
    note_full_rebuild_geometry(outcome.without_geometry.len());
    let Some(expected) = expected else {
        let note = "决策时模型建不起来，无从核对全量依据的拓扑，参照不写".to_string();
        println!("[房间全量] 参照未更新（{note}）");
        note_full_rebuild_settled(false, Some(note));
        return false;
    };
    let Some(hierarchy) = RoomHierarchy::configured() else {
        return false;
    };
    #[cfg(any(feature = "project_hd", feature = "project_hh"))]
    let match_room_fn = crate::fast_model::room_model::configured_match_room_fn();
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let match_room_fn = |_: &str| false;
    let room_key_words = db_option.get_room_key_word();
    let mesh_dir = db_option.get_meshes_path();
    let face = crate::options::data_face();

    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let (reference_written, note) = match room_resident::ensure_current(
        face,
        hierarchy,
        &room_key_words,
        match_room_fn,
        &mesh_dir,
    )
    .await
    {
        Ok(Some(outcome)) if outcome.model.manifest == *expected => {
            match room_resident::persist_reference(&outcome.model).await {
                Ok(reference) => {
                    println!(
                        "[房间全量] 参照已更新：{} {}@{}（{} 间房 / {} 块在册面板 / {} 个令牌）",
                        reference.form,
                        short(&reference.key),
                        short(&reference.digest),
                        reference.rooms,
                        reference.registered_panels,
                        reference.tokens.len()
                    );
                    (true, None)
                }
                Err(error) => (false, Some(format!("参照写失败: {error:#}"))),
            }
        }
        Ok(Some(outcome)) => (
            false,
            Some(format!(
                "全量期间清单换了代（{} → {}），参照不写，下次启动再比",
                short(&expected.key),
                short(&outcome.model.manifest.key)
            )),
        ),
        Ok(None) => (false, Some("摄入侧没有发布清单，参照不写".to_string())),
        Err(error) => (false, Some(format!("模型建不起来，参照不写: {error:#}"))),
    };
    if let Some(note) = &note {
        println!("[房间全量] 参照未更新（{note}）");
    }
    note_full_rebuild_settled(reference_written, note);
    reference_written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::match_room_name_hd;
    use crate::fast_model::room_resident::{MeshCache, registered_map};
    use crate::fast_model::room_topology::RoomGroup;
    use aios_core::{GeomInstQuery, RefU64};
    use bevy_transform::components::Transform;
    use glam::Vec3;
    use parry3d::bounding_volume::Aabb;
    use parry3d::math::Point;
    use std::sync::Arc;

    fn r(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(24384, id))
    }

    fn group(room: u32, name: &str, panels: &[u32]) -> RoomGroup {
        RoomGroup {
            room: r(room),
            name: name.into(),
            panels: panels.iter().map(|id| r(*id)).collect(),
        }
    }

    fn inst(panel: u32) -> GeomInstQuery {
        GeomInstQuery {
            refno: r(panel),
            old_refno: None,
            owner: r(1),
            world_aabb: Aabb::new(Point::new(0.0, 0.0, 0.0), Point::new(1.0, 1.0, 1.0)),
            world_trans: Transform::from_translation(Vec3::ZERO),
            insts: vec![aios_core::ModelHashInst {
                geo_hash: "mesh".into(),
                ..Default::default()
            }],
            has_neg: false,
            generic: "PANE".into(),
            pts: None,
            date: None,
        }
    }

    fn model(
        form: ResidentForm,
        key: &str,
        groups: Vec<RoomGroup>,
        insts: Vec<GeomInstQuery>,
    ) -> ResidentRoomModel {
        let keywords = vec!["-RM".to_string()];
        let map = registered_map(&groups, &keywords, match_room_name_hd);
        ResidentRoomModel::assemble(
            ResidentManifest {
                form,
                key: key.into(),
            },
            groups,
            map,
            insts,
            Arc::<MeshCache>::default(),
        )
    }

    fn groups() -> Vec<RoomGroup> {
        vec![
            group(5, "/1RX-RM03-R301", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
        ]
    }

    /// 臂表 §4.3 的落点：只有 `Unchanged` 零写入；`Apply` 交 diff；无参照走全量但不标脏；
    /// 同键不等与参照不可读走全量**且**先落持久要求。
    #[test]
    fn dispose_maps_every_plan_arm_and_marks_rebuild_required_only_for_tamper_and_unreadable() {
        assert_eq!(
            dispose(&StartupRoomPlan::Unchanged),
            PlanDisposition::ZeroWrite
        );
        let diff = TopologyDiff::default();
        assert_eq!(
            dispose(&StartupRoomPlan::Apply(diff.clone())),
            PlanDisposition::ApplyDiff(&diff)
        );
        match dispose(&StartupRoomPlan::NoReference) {
            PlanDisposition::FullRebuild {
                reason,
                mark_rebuild_required,
            } => {
                assert!(reason.contains("无参照"), "{reason}");
                assert!(!mark_rebuild_required, "无参照不是库坏了，不标脏");
            }
            other => panic!("{other:?}"),
        }
        for plan in [
            StartupRoomPlan::IntegrityMismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
            },
            StartupRoomPlan::ReferenceUnreadable("checksum".into()),
        ] {
            match dispose(&plan) {
                PlanDisposition::FullRebuild {
                    reason,
                    mark_rebuild_required,
                } => {
                    assert_eq!(reason, plan.summary());
                    assert!(mark_rebuild_required, "{plan:?} 要留下持久要求");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// 旧视图只在会 diff 时读：形态同、摘要不等、键不等三者缺一都不读分片。
    #[test]
    fn the_old_view_is_only_loaded_when_the_plan_will_diff() {
        let current = model(ResidentForm::Published, "m2", groups(), vec![]);
        let mut reference = ResidentReference::for_model(&current);
        assert!(
            !needs_old_view(&reference, &current),
            "摘要等：Unchanged，不读"
        );
        reference.digest = "changed".into();
        assert!(
            !needs_old_view(&reference, &current),
            "同键不等：IntegrityMismatch，不读"
        );
        reference.key = "m1".into();
        assert!(
            needs_old_view(&reference, &current),
            "键不等摘要不等：要 diff"
        );
        reference.form = "latest".into();
        assert!(
            !needs_old_view(&reference, &current),
            "形态不同：NoReference，不读"
        );
    }

    /// 重算集按模型里有没有几何分**三**摊：从未生成的按**文件** dbnum 分组、不进重算、不挡参照；
    /// 有行却不可用的另成一摊（挡参照）；有几何的进重算（D-A (a)）。
    #[test]
    fn recalc_panels_without_geometry_are_set_aside_by_file_dbnum() {
        let mut bad = inst(9);
        bad.world_aabb = Aabb::new(Point::new(f32::NAN, 0.0, 0.0), Point::new(1.0, 1.0, 1.0));
        let mut current = model(ResidentForm::Published, "m2", groups(), vec![inst(7), bad]);
        current.room_files = BTreeMap::from([(r(5), 7997u32), (r(6), 8000u32)]);
        let panels = BTreeSet::from([r(7), r(8), r(9)]);
        let split = split_recalc_by_geometry(&current, &panels);
        assert_eq!(
            split.to_recalc.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([r(7)])
        );
        assert_eq!(
            split.no_geometry,
            BTreeMap::from([(7997u32, vec![r(8)])]),
            "8 一条行都没有：从未生成"
        );
        assert_eq!(
            split.unusable,
            BTreeMap::from([(8000u32, vec![r(9)])]),
            "9 有行、包围盒坏：不可用，按它房间所属的文件 8000 分组"
        );
    }

    /// 回执：几何不可用与失败挡参照、理由点名；从未生成的（`no_geometry`）**不挡**（D-A (a)）；
    /// 全部成功才 `all_settled`。
    #[test]
    fn apply_outcome_blocks_the_reference_on_missing_geometry_or_failures_and_names_them() {
        let mut outcome = StartupApplyOutcome {
            planned: PlannedCounts {
                removed: 1,
                recalc: 4,
                renumbered_rooms: 1,
                renumbered_panels: 2,
            },
            removed: 1,
            recalculated: 2,
            renumbered_rooms: 1,
            renumbered_panels: 2,
            no_geometry: BTreeMap::from([(7997u32, vec![r(8)])]),
            unusable: BTreeMap::from([(8000u32, vec![r(9)])]),
            ..Default::default()
        };
        assert!(!outcome.all_settled(), "几何不可用的那一块挡参照");
        let reason = outcome.blocking_reason().unwrap();
        assert!(
            reason.contains("1 块在册面板几何不可用") && reason.contains("8000: 1"),
            "{reason}"
        );
        assert!(
            !reason.contains("7997"),
            "从未生成的那一块不是拒写的理由: {reason}"
        );
        assert_eq!(
            outcome.summary(),
            "删 1/1 / 重算 4（成功 2 · 无几何 1 · 不可用 1 · 失败 0）/ 改列 1/1 间 2/2 块"
        );

        // 只剩从未生成的那一块：不挡参照（它没有边可陈旧，几何到来时由发布事务补算）。
        outcome.unusable.clear();
        outcome.recalculated = 3;
        assert!(
            outcome.all_settled() && outcome.blocking_reason().is_none(),
            "no_geometry 不挡参照（D-A (a)）"
        );
        outcome.no_geometry.clear();
        outcome.recalculated = 4;
        assert!(outcome.all_settled() && outcome.blocking_reason().is_none());

        // 有尾巴时回执末尾多一句「留作尾巴多少块」——尾巴只在进程内，不再有入队计数。
        outcome.unusable = BTreeMap::from([(8000u32, vec![r(9)])]);
        outcome.recalculated = 3;
        outcome.unsettled = BTreeSet::from([r(9)]);
        assert_eq!(
            outcome.summary(),
            "删 1/1 / 重算 4（成功 3 · 无几何 0 · 不可用 1 · 失败 0）/ 改列 1/1 间 2/2 块 \
             / 没算成的 1 块留作尾巴（挡参照，等该库任务的房间段重算）"
        );
        outcome.unusable.clear();
        outcome.unsettled.clear();
        outcome.recalculated = 4;

        outcome
            .failures
            .push("pe:24384_9 重算归属失败: 覆盖率不够".into());
        assert!(!outcome.all_settled());
        assert!(
            outcome.blocking_reason().unwrap().contains("覆盖率不够"),
            "{:?}",
            outcome.blocking_reason()
        );
    }

    /// `mem://` 门：改列一间房一个事务，只改交集面板两种边的 `room_num`，边 id 与别的边原样。
    #[tokio::test]
    async fn renumber_changes_only_the_room_num_of_the_intersection_in_one_transaction() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_startup").use_db("renumber").await.unwrap();
        db.query(
            "CREATE pe:24384_1 SET noun = 'FRMW'; CREATE pe:24384_2 SET noun = 'FRMW'; \
             CREATE pe:24384_7 SET noun = 'PANE'; CREATE pe:24384_8 SET noun = 'PANE'; CREATE pe:24384_11 SET noun = 'PANE'; \
             CREATE pe:24384_5 SET noun = 'EQUI'; CREATE pe:24384_6 SET noun = 'BOX'; CREATE pe:24384_9 SET noun = 'EQUI'; CREATE pe:24384_12 SET noun = 'EQUI'; \
             RELATE pe:24384_1->room_panel_relate->pe:24384_7 SET room_num = 'R301'; \
             RELATE pe:24384_1->room_panel_relate->pe:24384_8 SET room_num = 'R301'; \
             RELATE pe:24384_2->room_panel_relate->pe:24384_11 SET room_num = 'R302'; \
             RELATE pe:24384_7->room_relate->pe:24384_5 SET room_num = 'R301'; \
             RELATE pe:24384_7->room_relate->pe:24384_6 SET room_num = 'R301'; \
             RELATE pe:24384_8->room_relate->pe:24384_9 SET room_num = 'R301'; \
             RELATE pe:24384_11->room_relate->pe:24384_12 SET room_num = 'R302';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        async fn edge_ids(db: &Db) -> Vec<String> {
            let mut ids: Vec<String> = db
                .query("SELECT VALUE <string>id FROM room_relate;")
                .await
                .unwrap()
                .check()
                .unwrap()
                .take(0)
                .unwrap();
            ids.sort();
            ids
        }
        let ids_before = edge_ids(&db).await;

        apply_renumber_on(
            &db,
            &RoomRenumber {
                room: r(1),
                old_num: "R301".into(),
                new_num: "R309".into(),
                panels: vec![r(7)],
            },
        )
        .await
        .unwrap();

        async fn room_nums(db: &Db, sql: &str) -> Vec<String> {
            let mut response = db.query(sql).await.unwrap().check().unwrap();
            let mut nums: Vec<String> = response.take(0).unwrap();
            nums.sort();
            nums
        }
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_7->room_relate;").await,
            vec!["R309", "R309"]
        );
        assert_eq!(
            room_nums(
                &db,
                "SELECT VALUE room_num FROM pe:24384_7<-room_panel_relate;"
            )
            .await,
            vec!["R309"]
        );
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_8->room_relate;").await,
            vec!["R301"],
            "不在交集里的面板原样"
        );
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_11->room_relate;").await,
            vec!["R302"],
            "别的房间原样"
        );
        let ids_after = edge_ids(&db).await;
        assert_eq!(ids_before, ids_after, "改列不增删边、不动边 id");
        assert_eq!(ids_after.len(), 4);
    }

    /// `mem://` 门：持久重建要求的读写——没有行 / `false` / `NONE` 都是「不要求」；标脏后读回理由，
    /// 且不碰 `resident_reference` 那一格。
    #[tokio::test]
    async fn rebuild_requirement_roundtrips_and_leaves_the_reference_alone() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_startup").use_db("rebuild").await.unwrap();
        assert_eq!(read_rebuild_required_on(&db).await.unwrap(), None, "没有行");

        let current = model(ResidentForm::Published, "m1", groups(), vec![]);
        let reference = ResidentReference::for_model(&current);
        room_resident::write_resident_reference_on(&db, &reference)
            .await
            .unwrap();
        assert_eq!(
            read_rebuild_required_on(&db).await.unwrap(),
            None,
            "行在、两格不在（老章 / 只写过参照）= 不要求"
        );
        db.query("UPSERT room_build:main SET rebuild_required = false;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(read_rebuild_required_on(&db).await.unwrap(), None);

        mark_rebuild_required_on(&db, "同一组文件版本拓扑摘要却不等（aaaa ≠ bbbb）")
            .await
            .unwrap();
        assert_eq!(
            read_rebuild_required_on(&db).await.unwrap().as_deref(),
            Some("同一组文件版本拓扑摘要却不等（aaaa ≠ bbbb）")
        );
        assert_eq!(
            room_resident::read_resident_reference_on(&db)
                .await
                .unwrap(),
            Some(reference),
            "标脏不碰参照"
        );
    }

    /// `/health` 的参照格：三种降级形状各自可辨，成功形状只给令牌个数。
    #[tokio::test]
    async fn reference_health_distinguishes_missing_error_and_timeout() {
        let current = model(ResidentForm::Published, "m1", groups(), vec![]);
        let reference = ResidentReference::for_model(&current);
        let ok = reference_health_from(Duration::from_secs(1), async {
            Ok(Some(reference.clone()))
        })
        .await;
        assert_eq!(ok["form"], "published");
        assert_eq!(ok["key"], "m1");
        assert_eq!(ok["tokens"], 0);
        assert_eq!(ok["rooms"], 2);
        assert_eq!(ok["registered_panels"], 3);
        assert_eq!(ok["settled_at"], reference.settled_at);

        let none = reference_health_from(Duration::from_secs(1), async { Ok(None) }).await;
        assert_eq!(none, Value::Null);

        let error = reference_health_from(Duration::from_secs(1), async {
            Err(anyhow::anyhow!("boom"))
        })
        .await;
        assert!(error["error"].as_str().unwrap().contains("boom"));

        let slow = reference_health_from(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(None)
        })
        .await;
        assert!(slow["error"].as_str().unwrap().contains("超过"));
    }

    /// M2 (b) 的闸（纯函数）：启动侧之外的参照写点只在「本进程比过、且没留尾巴」时放行。
    ///
    /// 没有回执那一条不是形式主义——worker（`ensure_batch_worker`）起在 `run_cli` 的房间段
    /// **之前**，它的空闲轮完全可能先于房间段跑完一轮；那时启动 diff 一块面板都没应用，
    /// 参照却会被推进到当前拓扑，停机期间未监听库的全部变化就此静默丢失。
    /// **重启不构成漏洞**：标志丢了 = 回到「没有回执」= 拒写，缺省就在保守那一边。
    #[test]
    fn the_third_write_point_is_gated_by_the_startup_compare_tail() {
        assert!(
            startup_tail_of(None)
                .expect("还没比过就不许写")
                .contains("还没做过启动房间对比"),
            "没有回执时要说清为什么拒写"
        );

        let clean = StartupApplyOutcome {
            planned: PlannedCounts {
                recalc: 2,
                ..Default::default()
            },
            recalculated: 2,
            ..Default::default()
        };
        let settled =
            StartupCompareReport::new(ResidentForm::Published, "对比".into(), 1).with_apply(&clean);
        assert_eq!(
            startup_tail_of(Some(&settled)),
            None,
            "一块没落下：参照写点照旧放行"
        );

        // 只有从未生成的那一块没算：不是尾巴（D-A (a)），参照写点照旧放行。
        let never_generated = StartupApplyOutcome {
            planned: PlannedCounts {
                recalc: 2,
                ..Default::default()
            },
            recalculated: 1,
            no_geometry: BTreeMap::from([(7997u32, vec![r(8)])]),
            ..Default::default()
        };
        let passed = StartupCompareReport::new(ResidentForm::Published, "对比".into(), 1)
            .with_apply(&never_generated);
        assert_eq!(
            startup_tail_of(Some(&passed)),
            None,
            "从未生成的面板没有边可陈旧，不算尾巴"
        );
        assert_eq!(passed.no_geometry, 1, "但回执照样点名");

        let with_tail = StartupApplyOutcome {
            planned: PlannedCounts {
                recalc: 2,
                ..Default::default()
            },
            recalculated: 1,
            unusable: BTreeMap::from([(8000u32, vec![r(9)])]),
            unsettled: BTreeSet::from([r(9)]),
            ..Default::default()
        };
        let held = StartupCompareReport::new(ResidentForm::Published, "对比".into(), 1)
            .with_apply(&with_tail);
        let reason = startup_tail_of(Some(&held)).expect("有尾巴就不许写");
        assert!(
            reason.contains("几何不可用") && reason.contains("8000"),
            "拒写的理由要点名是哪一摊: {reason}"
        );
        assert_eq!(held.unusable, 1);

        // 跳过与显式全量两条臂：缺省就是「有尾巴」，没人表态它也不会放行。
        let skipped =
            StartupCompareReport::new(ResidentForm::Published, "跳过：空间树已验证为空".into(), 0);
        assert!(
            startup_tail_of(Some(&skipped))
                .expect("没比过就不许写")
                .contains("跳过"),
            "跳过那一臂照样拦"
        );
    }

    /// 回执槽：记一条、读回来；显式全量收尾只回填三格（含尾巴）。
    #[test]
    fn compare_report_records_and_full_rebuild_settlement_only_touches_two_fields() {
        let outcome = StartupApplyOutcome {
            planned: PlannedCounts {
                removed: 2,
                recalc: 1,
                ..Default::default()
            },
            removed: 2,
            recalculated: 1,
            ..Default::default()
        };
        let report =
            StartupCompareReport::new(ResidentForm::Latest, "对比".into(), 7).with_apply(&outcome);
        record_startup_compare(report.clone());
        assert_eq!(startup_compare_report(), Some(report.clone()));
        assert_eq!(
            (report.removed, report.recalculated, report.failed),
            (2, 1, 0)
        );

        // 显式全量还没收尾：进程内那道闸拦着（`full()` 立的那句尾巴）。
        let pending_full = StartupCompareReport::new(
            ResidentForm::Latest,
            "持久重建要求：房间结构枚举失败".into(),
            1,
        );
        record_startup_compare(pending_full);
        assert!(
            startup_compare_tail().is_some(),
            "全量没跑完就写参照 = 宣称边已按新拓扑算过"
        );
        note_full_rebuild_settled(false, Some("全量期间清单换了代".into()));
        assert_eq!(
            startup_compare_tail().as_deref(),
            Some("全量期间清单换了代"),
            "全量收尾没写成参照：尾巴留着"
        );
        note_full_rebuild_settled(true, None);
        assert_eq!(
            startup_compare_tail(),
            None,
            "全量全成并写上参照：边就是按这份拓扑算的，闸放行"
        );

        record_startup_compare(report.clone());
        note_full_rebuild_settled(true, Some("x".into()));
        let settled = startup_compare_report().unwrap();
        assert!(settled.reference_written);
        assert_eq!(settled.note.as_deref(), Some("x"));
        assert_eq!(
            (
                settled.form.as_str(),
                settled.plan.as_str(),
                settled.removed
            ),
            ("latest", "对比", 2)
        );
    }

    #[test]
    fn the_full_rebuild_flag_accepts_the_usual_spellings_only() {
        for on in ["1", "true", "YES", " on "] {
            assert!(flag_is_on(on), "{on}");
        }
        for off in ["0", "false", "", "maybe"] {
            assert!(!flag_is_on(off), "{off}");
        }
    }

    /// 生产段（`#[cfg(test)]` 之前）的源码，行尾已归一成 `\n`。
    ///
    /// 归一这一步不是洁癖：`core.autocrlf = true` 下这份文件在工作树里是 CRLF，而下面几条钉
    /// 用的模式串（`"#[cfg(test)]\nmod tests {"`、`fn_body` 的 `"\n}\n"`）按 `\n` 写。不归一
    /// 就整片落空，三条钉会以「tests follow production code」齐红——看起来像生产代码退了，
    /// 其实只是行尾变了。`git diff` 还因为 autocrlf 什么都不显示。
    fn production_source() -> &'static str {
        static SOURCE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            let normalized = include_str!("room_startup.rs").replace("\r\n", "\n");
            normalized
                .split_once("#[cfg(test)]\nmod tests {")
                .expect("tests follow production code")
                .0
                .to_string()
        });
        &SOURCE
    }

    fn code_lines(text: &str) -> String {
        text.lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fn_body<'a>(source: &'a str, head: &str) -> &'a str {
        source
            .split_once(head)
            .unwrap_or_else(|| panic!("{head} must exist"))
            .1
            .split_once("\n}\n")
            .expect("fn body must close")
            .0
    }

    /// 源码钉（§4.8）：删与写只经 `recalc_panel_membership_for_consumption`，改列只经
    /// `render_renumber_statements`——本文件的生产代码不含自己的删除 / 插入语句；应用臂不含全量。
    #[test]
    fn the_startup_file_writes_edges_only_through_the_shared_membership_and_renumber_paths() {
        let code = code_lines(production_source());
        for forbidden in ["DELETE", "INSERT RELATION", "RELATE "] {
            assert!(
                !code.contains(forbidden),
                "room_startup.rs 的生产代码不许自己写 {forbidden}: 删与写只经 recalc_panel_membership_for_consumption"
            );
        }
        let apply = fn_body(
            production_source(),
            "pub(crate) async fn apply_startup_plan(",
        );
        assert!(
            apply.contains(
                "recalc_panel_membership_for_consumption(db_option, &model.map, *panel, Some(model))"
            ) && apply.contains(
                "recalc_panel_membership_for_consumption(db_option, &model.map, panel, Some(model))"
            ),
            "删与重算都走同一个函数，且面板几何取自这次装好的那份常驻模型（P2-b）: {apply}"
        );
        assert!(
            apply.contains("apply_renumber_on(&aios_core::SUL_DB, renumber)"),
            "改列走 render_renumber_statements 的事务包装: {apply}"
        );
        let renumber = fn_body(
            production_source(),
            "pub(crate) async fn apply_renumber_on(",
        );
        assert!(
            renumber.contains("render_renumber_statements(renumber)"),
            "{renumber}"
        );
        for forbidden in ["build_room_relations", "RoomWriteGuard", "startup_autorun"] {
            assert!(
                !apply.contains(forbidden),
                "应用臂不含全量、不带写守卫、不看冷启动开关（{forbidden}）: {apply}"
            );
        }
    }

    /// 源码钉（M2 (a)，ADR-072 R5 修订）：应用臂里**每一条**没算成的路都要把面板收进 `unsettled`
    /// ——它是进程内那道闸唯一的输入；而这一摊**不再入队**：第二条异步账（房间待办行）随
    /// ADR-072 R4 退役，边由该库任务的房间段整库重算收敛，应用臂不写任何待办行、不自己拼 SQL。
    ///
    /// 「几何不可用」与「重算失败」一视同仁：两者都是「这块面板有几何、边却没有按新拓扑算过」。
    /// **从未生成**的那一摊（`no_geometry`）不进（D-A (a)）：它没有边可陈旧。
    #[test]
    fn every_unsettled_panel_of_the_startup_apply_is_a_tail_and_none_is_enqueued() {
        let apply = fn_body(
            production_source(),
            "pub(crate) async fn apply_startup_plan(",
        );
        // 删失败 / 重算失败：逐块进 unsettled。
        assert_eq!(
            apply.matches("outcome.unsettled.insert(").count(),
            2,
            "删失败与重算失败两条路各收一次: {apply}"
        );
        // 几何不可用那一摊（`split_recalc_by_geometry` 分出来的）整摊进；从未生成的那一摊不进。
        assert!(
            apply.contains("unusable.values().flatten().copied()"),
            "几何不可用与重算失败一视同仁: {apply}"
        );
        assert!(
            !apply.contains("no_geometry.values()"),
            "从未生成的面板不算尾巴（D-A (a)）: {apply}"
        );
        // 改列失败那间房的面板整间重算（改列本来就是重算的省略式，多算不漏算）。
        assert!(
            apply.contains("outcome.unsettled.extend(renumber.panels.iter().copied())"),
            "改列失败同样算尾巴: {apply}"
        );
        // 尾巴不入队、不写库：房间随任务算（ADR-072 D4 / 附录 C.1）。
        for retired in [
            "enqueue_room_panel_recalcs(",
            "enqueue_room_recalc(",
            concat!("model_update_", "pending"),
            "RoomRecalcPanel",
            ".query(",
        ] {
            assert!(
                !apply.contains(retired),
                "应用臂不再排房间待办行（{retired}）: {apply}"
            );
        }
    }

    /// 源码钉（M2 (b)）：进程内那道闸的缺省是「有尾巴」，只有两处明确表态才清成 `None`
    /// ——`Unchanged` 零写入那一臂，与显式全量写上参照那一处。
    ///
    /// 新增一条臂而忘了表态时，默认落在保守那一边（拒写，下次启动多比一次）。
    #[test]
    fn the_startup_tail_defaults_to_blocked_and_is_cleared_at_exactly_two_places() {
        let code = code_lines(production_source());
        let clears: Vec<usize> = code
            .match_indices("unsettled = None")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(clears.len(), 2, "只有两处清尾巴: {clears:?}");
        let new_body = fn_body(
            production_source(),
            "    fn new(form: ResidentForm, plan: String, elapsed_ms: u128) -> Self {",
        );
        assert!(
            new_body.contains("unsettled: Some(plan)"),
            "回执的缺省是「有尾巴」: {new_body}"
        );
        let reconcile = code_lines(fn_body(
            production_source(),
            "pub(crate) async fn startup_room_reconcile(",
        ));
        let zero_write_arm = reconcile
            .split_once("PlanDisposition::ZeroWrite =>")
            .expect("零写入那一臂必须在")
            .1
            .split_once("PlanDisposition::FullRebuild {")
            .expect("下一臂是显式全量")
            .0;
        assert!(
            zero_write_arm.contains("report.unsettled = None"),
            "零写入那一臂要清尾巴（参照说的那份拓扑就是现状）: {zero_write_arm}"
        );
        let settle = fn_body(production_source(), "fn note_full_rebuild_settled(");
        assert!(
            settle.contains("report.unsettled = None") && settle.contains("if reference_written {"),
            "全量只有**写上参照**那一支清尾巴: {settle}"
        );
        let with_apply = fn_body(
            production_source(),
            "    fn with_apply(mut self, outcome: &StartupApplyOutcome) -> Self {",
        );
        assert!(
            with_apply.contains("self.unsettled = outcome.blocking_reason()"),
            "应用臂的尾巴就是「参照为什么没写」那一句: {with_apply}"
        );
    }

    /// 源码钉：编排进锁后才换代（`the_generation_swap_happens_inside_the_spatial_serial_lock` 的
    /// 启动侧），不读 `room_build` 的空间凭据对账（§4.6），不看 `startup_autorun`（只门 `lib.rs`
    /// 的全量臂），不带写守卫。
    #[test]
    fn the_reconcile_holds_the_serial_lock_before_swapping_and_never_asks_the_stamp() {
        let body = code_lines(fn_body(
            production_source(),
            "pub(crate) async fn startup_room_reconcile(",
        ));
        let lock = body
            .find("lock_spatial_serial().await")
            .expect("对比要持串行锁");
        let ensure = body.find("ensure_current(").expect("对比要先换代");
        let reference = body
            .find("read_resident_reference_on(")
            .expect("对比要读参照");
        let plan = body.find("plan_startup(").expect("对比要出计划");
        assert!(
            lock < ensure && ensure < reference && reference < plan,
            "顺序是 锁 → 换代 → 读参照 → 计划: {body}"
        );
        for forbidden in [
            "reconcile_startup_room_build",
            "startup_autorun",
            "RoomWriteGuard",
            "build_room_relations",
            "is_auto_work_armed",
        ] {
            assert!(
                !body.contains(forbidden),
                "编排不做这件事（{forbidden}）: {body}"
            );
        }
        let env_at = body
            .find("SKIP_STARTUP_ROOM_BUILD_ENV")
            .expect("止血口最强");
        let vacuous_at = body
            .find("vacuous_room_build_reason(")
            .expect("空树照旧跳过");
        assert!(
            env_at < vacuous_at && vacuous_at < lock,
            "两道门在锁之前: {body}"
        );
        assert!(
            body.contains("stamp_vacuous_room_build().await"),
            "空树照旧盖平凡章: {body}"
        );
    }

    /// 源码钉：`persist_reference(` 的生产调用点只有两处（写点 ① 对比应用全成、写点 ② 显式全量
    /// 全成），每处紧邻「全部成功」判定。这里数的是**启动侧**的写点；启动侧之外的写点各有
    /// 自己的钉，过的门是 [`startup_compare_tail`]。
    #[test]
    fn the_reference_is_written_at_exactly_two_places_each_behind_a_success_check() {
        let code = code_lines(production_source());
        let calls: Vec<usize> = code
            .match_indices("persist_reference(")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(calls.len(), 2, "写点只有两处: {calls:?}");
        let guard_before = |at: usize, needle: &str| {
            let window = &code[at.saturating_sub(400)..at];
            window.contains(needle)
        };
        assert!(
            guard_before(calls[0], "outcome.all_settled()"),
            "写点 ① 前面是 all_settled: {}",
            &code[calls[0].saturating_sub(400)..calls[0]]
        );
        assert!(
            guard_before(calls[1], "outcome.model.manifest == *expected"),
            "写点 ② 前面核对身份没变: {}",
            &code[calls[1].saturating_sub(400)..calls[1]]
        );
        let settle = fn_body(
            production_source(),
            "pub(crate) async fn settle_full_rebuild(",
        );
        assert!(
            settle.contains("lock_spatial_serial().await"),
            "写点 ② 也在串行锁里取模型: {settle}"
        );
    }
}
