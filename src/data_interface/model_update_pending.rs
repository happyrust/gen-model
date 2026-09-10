//! Per-process, per-target model work queued before the incremental watermark.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::initialization_phase::{StartupCoverage, StartupReconcile};
use crate::data_interface::model_coverage::{
    published_catalogue_sessions_on, stale_catalogue_dbnums,
};
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::aabb_refresh::AabbChange;
use crate::fast_model::room_model;

pub const TABLE: &str = "model_update_pending";
const ROOM_TARGET_VERSION_TABLE: &str = "room_target_version";
/// 判不了的在册面板登记表。
///
/// 曾经它是项目级**屏障**：只要有一块面板缺几何，全库房间重算整个停摆（本项目现场
/// 一度是 2 块面板压住 2580 个房间目标，而那两块的修复根早已进死信，屏障永远解不开）。
/// 现在它只是一份**缺陷清单**——替换范围的排除由元素分支按面板逐块处理，这里只负责
/// 记账、驱动修复、以及在清单变化时说一声。
const ROOM_PANEL_DEFECTS: &str = "room_panel_coverage_barrier:current";
const QUERY_CHUNK: usize = 500;
// 空闲轮一页的读取上界。页内逐根生成并在每根前后重查数据门：批量调用曾让新数据
// 最多等待整页（现场 16 根超过 200 秒），而初始化门中途关闭又会把调度让位记成失败。
// 读取仍分页，昂贵生成则以单根为不可抢占的最小单位。
const DRAIN_PAGE_SIZE: usize = 16;
/// 自动初始化/空闲轮每页认领的 regen 根数。与底层 `gen_geos_data`
/// 的 100 根分块对齐：保留批量预载收益，也不再把万级根和锁塞进一条任务。
const REGEN_DRAIN_PAGE_SIZE: usize = 100;
/// Regen 全部收口后，后置 AABB 任务同样有界消费。
const POST_REGEN_AABB_PAGE_SIZE: usize = 256;
const MODEL_DRAIN_DETAIL_SAMPLE_SIZE: usize = 10;

#[derive(Debug, Deserialize)]
struct PendingRowCount {
    count: u64,
}

/// Remove data-derived work left by an earlier service process before any startup
/// producer or consumer is allowed to run.
///
/// `model_update_pending` is a scheduling ledger for the current process. The
/// authoritative restart boundary is the atomic kv-mem data/model writeback to
/// RocksDB, so replaying rows from a previous process would mix two snapshots.
/// Counting and deleting share one ordered SurrealDB request; at this point the
/// process-instance lock is held and no watcher/worker has started yet.
///
/// **房间任务不在此列**（ADR-050 修订 2026-09-03）。清的是 `DATA_ACTION_FILTER` 那五种，
/// 它们全都能由启动复核（[`reconcile_model_coverage_at_startup`]）从凭证重新排出来；
/// `room_recalc_*` 由 ADR-010 §7 的房间轮自己收敛，唯一的补偿是启动整库房间重建，而
/// 那一步在 direct / env 禁用 / `autorun=false` / `model_incremental=false` 下都不跑——
/// 这些形态下把房间行一起删掉就是真丢工作，且没有任何东西会报。与
/// [`discard_pending_for_full_rebuild`] 同一口径。
pub async fn clear_stale_at_process_start() -> anyhow::Result<u64> {
    clear_stale_at_process_start_on(&SUL_DB).await
}

async fn clear_stale_at_process_start_on(db: &Surreal<Any>) -> anyhow::Result<u64> {
    clear_stale_with_protected_on(
        db,
        &crate::data_interface::task_registry::TaskRegistry::global().running_data_batch_dbnums(),
    )
    .await
}

pub(crate) async fn clear_stale_with_protected_on(
    db: &Surreal<Any>,
    protected: &[u32],
) -> anyhow::Result<u64> {
    let init_exclusion = render_initialization_exclusion(protected);
    let mut response = db
        .query(format!(
            "SELECT count() AS count FROM {TABLE} WHERE init_run_id IS NONE {DATA_ACTION_FILTER}{init_exclusion} GROUP ALL; \
             DELETE {TABLE} WHERE init_run_id IS NONE {DATA_ACTION_FILTER}{init_exclusion}; \
             SELECT count() AS count FROM {TABLE} WHERE true {ROOM_ACTION_FILTER} GROUP ALL;"
        ))
        .await?
        .check()?;
    let counts: Vec<PendingRowCount> = response.take(0)?;
    let rooms: Vec<PendingRowCount> = response.take(2)?;
    let kept_rooms = rooms.first().map_or(0, |row| row.count);
    if kept_rooms > 0 {
        println!(
            "启动清理保留 {kept_rooms} 行上一进程的房间任务（room_recalc_*）：\
             它们不从凭证补偿，由房间轮自行收敛"
        );
    }
    Ok(counts.first().map_or(0, |row| row.count))
}

/// 整库重建开工前作废这个库排着的增量工作，返回丢弃的行数。
///
/// 排在队列里的每一条都是照**重建之前**那份模型算出来的。重建要把这个库的根整个
/// 重画一遍，这些行于是既没用又有害：`transform` 与 `delete_cleanup` 排在 regen
/// 之前，会先拿旧窗口的结论去改一遍马上就要被替换的行；`cascade_expand` 会派生出
/// 一批同样过期的 regen 工作。core 的 `Refresh(当前 VIEW)` 在同一个位置把整条队列
/// 丢掉（`m_queue.end = m_queue.begin`），理由一模一样——视图自己要重画了。
///
/// **房间任务不在此列。** 清的是 `DATA_ACTION_FILTER` 那五种；
/// `room_recalc_*` 由 ADR-010 §7 的房间轮自己收敛，重建不会替它们重新入队，
/// 一起删掉就是真丢工作。
///
/// `dbnum = 0` 的行同样留着：按需生成（[`ensure_regen_pending`]）落的行认领不了来源库，
/// 它是人当场点的，不属于任何一次窗口的陈旧结论。
pub async fn discard_pending_for_full_rebuild(dbnum: u32) -> anyhow::Result<u64> {
    discard_pending_for_full_rebuild_on(&SUL_DB, dbnum).await
}

async fn discard_pending_for_full_rebuild_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<u64> {
    let mut response = db
        .query(format!(
            "SELECT count() AS count FROM {TABLE} \
             WHERE dbnum = {dbnum} {DATA_ACTION_FILTER} GROUP ALL; \
             DELETE {TABLE} WHERE dbnum = {dbnum} {DATA_ACTION_FILTER};"
        ))
        .await?
        .check()?;
    let counts: Vec<PendingRowCount> = response.take(0)?;
    Ok(counts.first().map_or(0, |row| row.count))
}

/// Retry ceiling per work item (same policy as `side_effect_pending`). A job
/// that keeps failing stays in the table as an inspectable dead letter instead
/// of burning a generator run every watcher cycle forever; it revives
/// automatically because [`render_upsert`] resets `attempts` whenever a newer
/// session touches the same target.
///
/// Public because the manual run enforces the same ceiling: reading the table
/// without it is how you INSPECT a dead letter, not how you re-run one.
pub const MAX_ATTEMPTS: u32 = 5;
const MODEL_WORK_LEASE: &str = "10m";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingModelWork {
    pub dbnum: u32,
    pub db_type: String,
    pub source_end_sesno: i32,
    /// 来源那条保存的写入时刻（RFC3339）。旧行、以及不认领会话号的行
    /// （房间任务、反向级联派生根）都是 `None`。
    #[serde(default)]
    pub source_end_sesno_time: Option<String>,
    pub action: ModelWorkAction,
    pub target_refno: String,
    #[serde(default)]
    pub noun: String,
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub revision: u64,
    /// 房间面板缺几何时，本生成根必须补出的 PANE。普通增量生成根为空。
    #[serde(default)]
    pub required_panels: Vec<String>,
}

/// One phase's durable model-work posture, exposed by `/health`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ModelPendingPhaseStatus {
    pub retryable: usize,
    pub dead_letters: usize,
}

/// One terminal row shown in the bounded dead-letter sample.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPendingBlockingSample {
    pub action: String,
    pub target_refno: String,
    #[serde(default)]
    pub noun: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub updated_at: serde_json::Value,
}

/// Single-round-trip snapshot of the durable model/room work table.
///
/// `data_phase.dead_letters` is the model-gate blocker. Room rows are reported
/// separately because ADR-025 keeps Room behind Model, but operators still need
/// to see the work accumulating on the downstream side of that barrier.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelPendingStatus {
    pub retryable: usize,
    pub dead_letters: usize,
    pub data_phase: ModelPendingPhaseStatus,
    pub room_phase: ModelPendingPhaseStatus,
    pub by_action: BTreeMap<String, ModelPendingPhaseStatus>,
    pub blocking_samples: Vec<ModelPendingBlockingSample>,
    /// 被模型类型限定（`model_incremental_types`）暂扣、本进程不消费的活口行数。
    /// 上面的 `retryable` / `data_phase` **不含**它们——那两个数说的是「本进程还会
    /// 去做的活」，与 worker 的 `has_pending_work` 同一谓词；这一格单独摆出来，
    /// 「名单外的根为什么一直不生成」在 `/health` 上就有答案，不必翻启动日志。
    /// 没限定时恒为 0。
    pub held_by_model_incremental_types: usize,
    pub error: Option<String>,
}

impl ModelPendingStatus {
    pub fn error(error: impl std::fmt::Display) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::default()
        }
    }

    pub fn has_data_dead_letters(&self) -> bool {
        self.data_phase.dead_letters > 0
    }

    pub fn has_room_dead_letters(&self) -> bool {
        self.room_phase.dead_letters > 0
    }

    pub fn data_blocking_samples(&self) -> impl Iterator<Item = &ModelPendingBlockingSample> {
        self.blocking_samples.iter().filter(|sample| {
            ModelWorkAction::parse(&sample.action).is_none_or(|action| !action.is_room_recalc())
        })
    }
}

/// One durable room row addressed by the same `(action, target)` identity as
/// [`record_id_of`].  A post-commit drain carries these keys across the RocksDB
/// boundary instead of discovering work by scanning the global pending table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub(crate) struct RoomWorkKey {
    pub action: ModelWorkAction,
    pub target_refno: String,
}

/// Exact room work published by one committed increment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoomDrainScope {
    keys: BTreeSet<RoomWorkKey>,
}

impl RoomDrainScope {
    pub(crate) fn from_plan(plan: &ModelUpdatePlan) -> Self {
        Self {
            keys: plan
                .work_items
                .iter()
                .filter(|item| item.action.is_room_recalc())
                .map(|item| RoomWorkKey {
                    action: item.action,
                    target_refno: item.target_refno.clone(),
                })
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    /// Resolve the globally deduplicated room rows touched by one file round.
    /// The association is separate from row identity because the same target
    /// can be invalidated by more than one database file.
    pub(crate) async fn for_file_round(dbnum: u32, end_sesno: i32) -> anyhow::Result<Self> {
        let round = escape_surql_str(&format!("{dbnum}:{end_sesno}"));
        let mut response = SUL_DB
            .query(format!(
                "SELECT action, target_refno FROM {TABLE} WHERE file_rounds CONTAINS '{round}' \
                 AND action IN ['room_recalc_panel', 'room_recalc_element'];"
            ))
            .await?
            .check()?;
        let rows: Vec<RoomWorkKey> = response.take(0)?;
        Ok(Self {
            keys: rows.into_iter().collect(),
        })
    }

    fn keys_for(&self, action: ModelWorkAction) -> Vec<RoomWorkKey> {
        self.keys
            .iter()
            .filter(|key| key.action == action)
            .cloned()
            .collect()
    }

    fn pages_for(&self, action: ModelWorkAction, page_size: usize) -> Vec<Vec<RoomWorkKey>> {
        let keys = self.keys_for(action);
        keys.chunks(page_size.max(1)).map(<[_]>::to_vec).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelRepairGroup {
    root_refno: String,
    noun: String,
    required_panels: Vec<String>,
}

/// 队列行的 id。同一个 `(action, target)` 只占一行，重复入队即幂等更新（ADR-015）。
///
/// `dbnum` **不参与寻址**。项目内 `target_refno` 的 Ref0 唯一归属一个 dbnum，所以把它
/// 拼进 id 不增加任何区分度，却要求每个入队方都算出同一个 dbnum——而它们并没有：
/// 反向级联派生根（`derived_regen_item`）与按需生成（`on_demand_model`）拿的是
/// `RefU64::get_0()`，那是 Ref0 不是 dbnum（`cata_closure` 专门有 `dbnum_of_ref0` 做这层
/// 反查）。于是 `24381/100677` 会同时存在 `7997_regen_root_…`（DESI 窗口排的）与
/// `24381_regen_root_…`（级联排的）两行：同一个根整整重生成两遍，而按需生成那条路径
/// 读写的始终是另一行，真正的 pending 永远收不掉。
///
/// dbnum 与 `source_end_sesno` 因此都只是字段，记最后一次触发来源。房间任务本来就
/// 已经这样寻址（ADR-010 §7），现在所有动作统一。
pub(crate) fn record_id_of(action: ModelWorkAction, target_refno: &str) -> String {
    let action_name = action.as_str();
    let target = target_refno.replace('/', "_");
    format!("{TABLE}:{action_name}_{target}")
}

fn record_id(item: &ModelWorkItem) -> String {
    record_id_of(item.action, &item.target_refno)
}

/// 把一组缺失 PANE 幂等地附着到它们的生成根任务上。
///
/// `required_panels CONTAINSALL ...` 必须在更新数组之前求值；SurrealDB 的 SET 子句按
/// 顺序执行。相同缺口重复探测时保留 revision/attempts/status，只有出现新缺口时才
/// 复活任务并递增 revision，既保护并发收口，也避免房间轮每十分钟制造一次新生成。
fn render_missing_panel_repair_upsert(group: &PanelRepairGroup) -> String {
    let id = record_id_of(ModelWorkAction::RegenRoot, &group.root_refno);
    let root = escape_surql_str(&group.root_refno);
    let noun = escape_surql_str(&group.noun);
    let panels = group
        .required_panels
        .iter()
        .map(|panel| format!("'{}'", escape_surql_str(panel)))
        .collect::<Vec<_>>()
        .join(", ");
    let required = format!("[{panels}]");
    let already_known = format!("(required_panels?:[]) CONTAINSALL {required}");
    format!(
        "UPSERT {id} SET \
         dbnum = dbnum?:0, db_type = 'DESI', action = 'regen_root', \
         target_refno = '{root}', noun = '{noun}', \
         attempts = IF {already_known} THEN attempts?:0 ELSE 0 END, \
         last_error = IF {already_known} THEN last_error ELSE NONE END, \
         source_end_sesno = source_end_sesno?:0, \
         revision = IF {already_known} THEN revision?:0 ELSE (revision?:0) + 1 END, \
         status = IF {already_known} THEN status?:'pending' ELSE 'pending' END, \
         updated_at = IF {already_known} THEN updated_at?:time::now() ELSE time::now() END, \
         required_panels = array::union(required_panels?:[], {required});"
    )
}

fn render_set_room_panel_defects(groups: &[PanelRepairGroup]) -> String {
    let panels = groups
        .iter()
        .flat_map(|group| group.required_panels.iter())
        .map(|panel| format!("'{}'", escape_surql_str(panel)))
        .collect::<Vec<_>>()
        .join(", ");
    let roots = groups
        .iter()
        .map(|group| format!("'{}'", escape_surql_str(&group.root_refno)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "UPSERT {ROOM_PANEL_DEFECTS} SET status = 'repairing', \
         missing_panels = [{panels}], repair_roots = [{roots}], updated_at = time::now();"
    )
}

fn render_clear_room_panel_defects() -> String {
    format!("DELETE {ROOM_PANEL_DEFECTS};")
}

/// 当前登记在案的缺陷面板，已排序——只用来判断这一轮要不要再说一遍。
///
/// 读不出来时当作「和上次不一样」：宁可多打一行，也别让一次查询抖动把真正的缺陷
/// 变化吞掉。
async fn read_room_panel_defects() -> Vec<String> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        missing_panels: Vec<String>,
    }
    let query = format!("SELECT missing_panels FROM {ROOM_PANEL_DEFECTS};");
    let Ok(mut response) = SUL_DB.query(query).await.and_then(|r| r.check()) else {
        return Vec::new();
    };
    let mut panels = response
        .take::<Vec<Row>>(0)
        .map(|rows| {
            rows.into_iter()
                .next()
                .map(|row| row.missing_panels)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    panels.sort_unstable();
    panels
}

async fn clear_room_panel_defects() -> anyhow::Result<()> {
    SUL_DB
        .query(render_clear_room_panel_defects())
        .await?
        .check()?;
    Ok(())
}

/// 登记这一轮判不了的在册面板，并把它们的生成根推进 durable 修复队列。
///
/// 这里**不**再阻断任何房间重算：元素分支已改为在 DELETE 上按面板让开
/// （`room_model::render_element_relate_write`），一块缺几何的面板只影响它自己的
/// 那几条边。所以本函数的职责退化为「记账 + 尝试修」，失败也只是记一笔。
///
/// 只在缺陷集合**变化时**打印。它每 30 秒被空闲轮碰一次，逐轮打印会把一个静态
/// 事实刷成噪音，真正该被看见的「又多了一块」反而淹掉。
async fn record_room_panel_defects(registered_rooms: usize, missing: &[RefnoEnum]) {
    let previous = read_room_panel_defects().await;
    let mut current = missing
        .iter()
        .map(RefnoEnum::to_pdms_str)
        .collect::<Vec<_>>();
    current.sort_unstable();
    let changed = previous != current;

    if changed {
        println!(
            "[房间缺陷] {registered_rooms} 间在册房间的面板里有 {} 块没有可用几何（{}）：\
             指向它们的存量归属边本轮不改写，其余房间目标照常重算",
            current.len(),
            current
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    match enqueue_missing_panel_repairs(missing).await {
        Ok(repair) if changed => println!(
            "[房间缺陷] 已登记并归并为 {} 个带几何后置条件的生成根（{} 块面板）",
            repair.roots, repair.panels
        ),
        Ok(_) => {}
        Err(error) => {
            if changed {
                println!("[房间缺陷] 缺失面板模型补偿入队失败: {error:#}");
            }
        }
    }
}

fn group_missing_panel_repairs(
    resolved: Vec<(
        RefnoEnum,
        crate::data_interface::generation_root::GenerationRoot,
    )>,
) -> Vec<PanelRepairGroup> {
    let mut groups = BTreeMap::<String, PanelRepairGroup>::new();
    for (panel, root) in resolved {
        let root_refno = root.root.to_pdms_str();
        groups
            .entry(root_refno.clone())
            .or_insert_with(|| PanelRepairGroup {
                root_refno,
                noun: root.noun,
                required_panels: Vec::new(),
            })
            .required_panels
            .push(panel.to_pdms_str());
    }
    groups
        .into_values()
        .map(|mut group| {
            group.required_panels.sort_unstable();
            group.required_panels.dedup();
            group
        })
        .collect()
}

#[derive(Debug, Default)]
struct PanelRepairEnqueueReport {
    roots: usize,
    panels: usize,
}

/// 把缺几何的在册面板归一到生成根，并以带后置条件的 durable 工作入队。
async fn enqueue_missing_panel_repairs(
    missing_panels: &[RefnoEnum],
) -> anyhow::Result<PanelRepairEnqueueReport> {
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let resolved =
        crate::data_interface::generation_root::resolve_generation_roots_with_targets_on(
            &SUL_DB,
            missing_panels,
            &unit_types,
        )
        .await?;
    if resolved.len() != missing_panels.len() {
        let resolved_panels = resolved
            .iter()
            .map(|(panel, _)| *panel)
            .collect::<HashSet<_>>();
        let unresolved = missing_panels
            .iter()
            .filter(|panel| !resolved_panels.contains(panel))
            .take(8)
            .map(RefnoEnum::to_pdms_str)
            .collect::<Vec<_>>();
        anyhow::bail!(
            "{} 块缺失面板中有 {} 块无法解析生成根（例如 {}），未登记为房间面板缺陷",
            missing_panels.len(),
            missing_panels.len().saturating_sub(resolved.len()),
            unresolved.join(", ")
        );
    }
    let groups = group_missing_panel_repairs(resolved);
    let panels = groups.iter().map(|group| group.required_panels.len()).sum();
    for chunk in groups.chunks(QUERY_CHUNK) {
        SUL_DB
            .query(
                chunk
                    .iter()
                    .map(render_missing_panel_repair_upsert)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .await
            .map_err(|error| anyhow::anyhow!("persist missing panel repairs failed: {error}"))?
            .check()
            .map_err(|error| {
                anyhow::anyhow!("persist missing panel repair statements failed: {error}")
            })?;
    }
    SUL_DB
        .query(render_set_room_panel_defects(&groups))
        .await
        .map_err(|error| anyhow::anyhow!("persist room panel defects failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("persist room panel defect statement failed: {error}"))?;
    Ok(PanelRepairEnqueueReport {
        roots: groups.len(),
        panels,
    })
}

/// Persist the exact model work before advancing `applied_sesno`.
pub async fn enqueue_plan(plan: &ModelUpdatePlan) -> anyhow::Result<()> {
    enqueue_plan_on(&SUL_DB, plan).await
}

/// [`enqueue_plan`] 的显式 `db` 版，`mem://` 门直接喂（入队的行形与生产逐字同源）。
pub(crate) async fn enqueue_plan_on(
    db: &Surreal<Any>,
    plan: &ModelUpdatePlan,
) -> anyhow::Result<()> {
    for chunk in plan.work_items.chunks(QUERY_CHUNK) {
        db.query(
            chunk
                .iter()
                // 这条路只入队工作项，手上没有窗口右端的时刻——来源时刻由收口
                // 事务那一份 upsert 补（同一行、同一条单调条件，不会互相覆盖）。
                .map(|item| render_upsert(item, None))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .await
        .map_err(|error| anyhow::anyhow!("persist model work batch failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("persist model work batch statement failed: {error}"))?;
    }
    Ok(())
}

/// Translate legacy changed-refno jobs into stable root work. Legacy rows do
/// not retain operations, so this is deliberately a conservative regen-only
/// bridge; new rows always use the exact pre-persist plan.
#[cfg(test)]
async fn enqueue_legacy_changed_refnos(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    refnos: &[aios_core::RefU64],
) -> anyhow::Result<()> {
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let mut plan = ModelUpdatePlan::default();
    let mut seen = std::collections::BTreeSet::new();
    for &legacy_refno in refnos {
        let refno = RefnoEnum::from(legacy_refno);
        let Some(root) =
            crate::data_interface::generation_root::resolve_live_element_generation_root(
                refno,
                &unit_types,
            )
            .await?
        else {
            continue;
        };
        if seen.insert(root.root.to_pdms_str()) {
            plan.work_items.push(ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::RegenRoot,
                target_refno: root.root.to_pdms_str(),
                noun: root.noun,
            });
        }
    }
    enqueue_plan(&plan).await
}

/// 一个包围盒变更对应的房间重算任务（ADR-010 §2/§4）。
///
/// `dbnum` / `source_end_sesno` 对房间任务只是来源记录，不参与寻址也不参与复活判定：
/// 行 id 不带 dbnum，复活由每次入队递增的 revision 驱动。两者都取 0——这一层在几何刷新里，既不知道自己
/// 属于哪次会话，也没有 refno 所属库的反查结果。曾经填 `refno().get_0()`，那是 Ref0
/// 不是 dbnum（见 `record_id_of`），而 Ref0 有可能撞上另一个库真实的 dbnum，把这行
/// 误挂到别的库名下；宁可留空也不填一个看着像真的假值。
fn room_recalc_item_with_source(
    refno: RefnoEnum,
    noun: &str,
    dbnum: u32,
    end_sesno: i32,
) -> ModelWorkItem {
    ModelWorkItem {
        dbnum,
        db_type: "DESI".to_string(),
        source_end_sesno: end_sesno,
        action: if noun == "PANE" {
            ModelWorkAction::RoomRecalcPanel
        } else {
            ModelWorkAction::RoomRecalcElement
        },
        target_refno: refno.to_pdms_str(),
        noun: noun.to_string(),
    }
}

fn room_recalc_item(change: &AabbChange) -> ModelWorkItem {
    room_recalc_item_with_source(change.refno, &change.noun, 0, 0)
}

fn room_recalc_items(changes: &[AabbChange]) -> Vec<ModelWorkItem> {
    let mut items = std::collections::BTreeMap::new();
    for change in changes {
        let item = room_recalc_item(change);
        items.insert(item.target_refno.clone(), item);
    }
    items.into_values().collect()
}

/// Render room work for a transaction that also publishes the new AABB pointer.
/// The caller owns the transaction wrapper; exposing only the statements keeps
/// direct and staged enqueue semantics on the same `(action, target)` renderer.
pub(crate) fn render_room_recalc_upserts(changes: &[AabbChange]) -> String {
    room_recalc_items(changes)
        .iter()
        // 房间任务不认领来源保存（同一块面板被不同库轮流触发），来源段整个不摆。
        .map(|item| render_upsert(item, None))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 包围盒真的变了 → 排一次房间归属重算。
///
/// 只接受**变更集**：同一轮里同一个目标只需要一行，因此先按目标折叠再落库——队列行
/// 的 id 本来就幂等，重复入队只是白跑一趟往返。没有变更时本来就无话可说。
pub async fn enqueue_room_recalc(changes: &[AabbChange]) -> anyhow::Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    enqueue_plan(&ModelUpdatePlan {
        work_items: room_recalc_items(changes),
        ..Default::default()
    })
    .await
}

/// 启动对比里**没算成**的面板 → `RoomRecalcPanel` durable 目标（常驻房间模型 M2；
/// 调用方是 `room_startup::apply_startup_plan`）。返回入队了几块。
///
/// 为什么要有这条路：写点 ③（[`settle_resident_reference`]）放行的前提只有「本轮没失败」
/// 与「房间表里连死信都不剩」。启动对比里失败或无几何的面板此前**既不入队、也不标脏**——
/// 它们不在房间表里，也不产生 drain 失败，于是那两条前提恒真，启动之后任意一轮干净的 drain
/// 都会把参照推进到当前拓扑，而参照的含义是「库里的归属边就是按这份拓扑算出来的」。下次启动
/// `plan_startup` 拿它做 diff，就把那几块从没算成的面板判成「没变」，陈旧的边永久留下且无日志。
///
/// 形状与几何触发那条路（[`room_recalc_item`]）一字不差，特别是 **`dbnum` / `source_end_sesno`
/// 都取 0**，两条理由：
///
/// 1. 启动对比不是一次保存窗口，手上没有「来源库 + 来源会话」——照 [`room_recalc_item`] 的
///    注释，宁可留空也不填一个看着像真的假值（行 id 本来就不带 dbnum，寻址只认目标）。
/// 2. **正确性**：drain 与 [`room_work_is_drained`] 共用 [`render_automatic_scope_filter`]
///    的范围谓词 `(dbnum?:0) = 0 OR (dbnum?:0) IN [watch_dbnums]`。填一个真实库号的话，
///    未监听库的面板——恰恰是「无几何」那一摊的主力（Q20 (a)）——入队之后既没人消费，
///    也不被清空判定看见：行在库里躺着，参照照写不误，等于白入队。0 让它对两侧都可见。
pub(crate) async fn enqueue_room_panel_recalcs(
    panels: &BTreeSet<RefnoEnum>,
) -> anyhow::Result<usize> {
    enqueue_room_panel_recalcs_on(&SUL_DB, panels).await
}

/// [`enqueue_room_panel_recalcs`] 的显式 `db` 版，`mem://` 门直接喂。
pub(crate) async fn enqueue_room_panel_recalcs_on(
    db: &Surreal<Any>,
    panels: &BTreeSet<RefnoEnum>,
) -> anyhow::Result<usize> {
    let items = room_panel_recalc_items(panels);
    if items.is_empty() {
        return Ok(0);
    }
    let count = items.len();
    enqueue_plan_on(
        db,
        &ModelUpdatePlan {
            work_items: items,
            ..Default::default()
        },
    )
    .await?;
    Ok(count)
}

/// 纯函数：一摊面板 → `RoomRecalcPanel` 工作项。与
/// `window_root_plan::append_room_structural_work` 折面板目标的落法同形，只是不认领来源。
pub(crate) fn room_panel_recalc_items(panels: &BTreeSet<RefnoEnum>) -> Vec<ModelWorkItem> {
    panels
        .iter()
        .map(|panel| room_recalc_item_with_source(*panel, "PANE", 0, 0))
        .collect()
}

/// Refresh targets omitted by a derived-geometry root's final node set, then force one
/// idempotent room invalidation for each target that currently has an AABB.
pub(crate) async fn refresh_post_regen_aabbs(refnos: &[RefnoEnum]) -> anyhow::Result<usize> {
    if refnos.is_empty() {
        return Ok(0);
    }
    crate::fast_model::aabb_refresh::update_inst_relate_aabbs_by_refnos_incremental(refnos, true)
        .await?;
    let changes = crate::fast_model::aabb_refresh::existing_geometric_aabb_changes(refnos).await?;
    enqueue_room_recalc(&changes).await?;
    Ok(changes.len())
}

/// 这次入队要不要**无条件**把死信复活（清零 `attempts` / `last_error`）。
///
/// 两类任务的会话号不能拿来比大小，因此不能用「来了更新的会话」当复活理由：
///
/// * **房间重算**——行 id 不带 dbnum，同一块面板会被不同库的会话轮流触发，
///   跨库比 sesno 毫无意义（一个库的 500 会永久压住另一个库的 80）。而它的入队
///   条件本身就是「AABB 真的变了」，每一次入队都是一个全新的重算理由。
/// * **不认领会话号的任务**（`source_end_sesno == 0`）——反向级联派生根
///   （[`derived_regen_item`]）就是这一类：跨库会话号不可比，所以它如实留空。
///   而 `0 > 0` 恒假，按会话号比的话它失败到 [`MAX_ATTEMPTS`] 之后就再也不会被
///   [`render_drain_select`] 取到，**即便后续每一次目录改动都在重新把它推进队列**
///   ——每次 upsert 只是把 `revision` 加一，任务永久躺平。房间任务过去为这个道理
///   单独开了口子，派生根有同样的性质却没赶上。
fn revives_unconditionally(item: &ModelWorkItem) -> bool {
    item.action.is_room_recalc() || item.source_end_sesno == 0
}

/// 窗口右端那条保存的时刻只属于**认领了这个右端**的任务（同 T2 的端点守卫）。
///
/// 一份收口里的任务并非都来自同一条保存：房间任务与反向级联派生根
/// （[`revives_unconditionally`] 那两类）`source_end_sesno` 是 0，如实不认领来源；
/// 号对不上就不贴时刻，宁可让待重试卡的来源段整个不摆，也不能把 A 的时刻标在 B 上。
pub(crate) fn source_time_for<'a>(
    item: &ModelWorkItem,
    window_end_sesno: i32,
    window_end_time: Option<&'a str>,
) -> Option<&'a str> {
    if item.source_end_sesno == 0 || item.source_end_sesno != window_end_sesno {
        return None;
    }
    window_end_time
}

/// A positive DESI save is the smallest durable publication cohort available at
/// this layer. Every root produced by that window must publish together; work
/// without a comparable source target stays independently schedulable.
fn publication_cohort(dbnum: u32, source_end_sesno: i32) -> Option<String> {
    (dbnum > 0 && source_end_sesno > 0).then(|| format!("{dbnum}:{source_end_sesno}"))
}

fn pending_publication_cohort(job: &PendingModelWork) -> Option<String> {
    (job.action == ModelWorkAction::RegenRoot)
        .then(|| publication_cohort(job.dbnum, job.source_end_sesno))
        .flatten()
}

/// `source_end_sesno_time` 是**那条来源保存的写入时刻**（RFC3339），待重试卡上的
/// `来源保存 08-05 18:24`（plant-ui ADR-0019 Q7）。与水位那一列同一把尺子、同一种
/// 写法：跟着 `source_end_sesno` 的单调条件走，读不到就整条子句都不写。
pub(crate) fn render_upsert(item: &ModelWorkItem, source_end_sesno_time: Option<&str>) -> String {
    let id = record_id(item);
    let db_type = escape_surql_str(&item.db_type);
    let target = escape_surql_str(&item.target_refno);
    let root_id = escape_surql_str(&item.target_refno.replace('/', "_"));
    let noun = escape_surql_str(&item.noun);
    let end_sesno = item.source_end_sesno;
    let dbnum = item.dbnum;
    let active_room_round = item
        .action
        .is_room_recalc()
        .then(|| {
            crate::data_interface::batch_worker::active_data_window()
                .map(|(dbnum, end)| format!("{dbnum}:{end}"))
        })
        .flatten();
    let queued_status = if active_room_round.is_some() {
        "collecting"
    } else {
        "pending"
    };

    // 死信复活的判据：本次触发是否比这一行已知的来源更新。
    //
    // 常规任务按会话号比——同库内 sesno 单调，「来了更新的会话」就是重试的正当理由。
    // 不能这么比的那两类见 [`revives_unconditionally`]。
    // status 与 attempts / last_error 走同一个复活判据：没复活就保持原状态
    // （新行缺省 'pending'）。此前 status 无条件写 'pending'，死信行（attempts
    // 已到上限）被一次旧会话的 upsert 摸过之后，面板上是 pending、drain 却永远
    // 不取——状态在撒谎。drain 的候选集本来就按 attempts 挡，这里只修观感与
    // 一致性，不改消费语义。
    let revival_clauses = if revives_unconditionally(item) {
        let mut clauses = vec![
            "last_error = NONE".to_string(),
            format!("status = '{queued_status}'"),
        ];
        if !item.action.is_room_recalc() {
            clauses.push("attempts = 0".to_string());
        }
        clauses
    } else {
        vec![
            format!(
                "attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END"
            ),
            format!(
                "last_error = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END"
            ),
            format!(
                "status = IF {end_sesno} > (source_end_sesno?:0) THEN '{queued_status}' \
                 ELSE status?:'{queued_status}' END"
            ),
        ]
    };
    // dbnum 字段的合并策略与复活无关，别把两件事绑在一个判断上：房间任务的行不带
    // dbnum、会被不同库轮流触发，所以只升不降；其余照写本次来源——但本次入队
    // **不认领**来源库时（dbnum == 0：反向级联派生根、按需生成）不得把行上已存的
    // 真实库号抹掉。抹掉的后果不是丢失而是延迟：那个根从「本库批次工作单」掉进
    // 空闲轮 `drain_data_phases`，而 0 覆盖真值没有任何信息增益。
    let dbnum_clause = if item.action.is_room_recalc() {
        format!("dbnum = math::max([dbnum?:0, {dbnum}])")
    } else if dbnum == 0 {
        "dbnum = dbnum?:0".to_string()
    } else {
        format!("dbnum = {dbnum}")
    };

    let mut clauses = vec![
        dbnum_clause,
        format!("db_type = '{db_type}'"),
        format!("action = '{}'", item.action.as_str()),
        format!("target_refno = '{target}'"),
        format!("noun = '{noun}'"),
    ];
    if let Some(round) = active_room_round {
        clauses.push(format!(
            "file_rounds = array::union(file_rounds?:[], ['{}'])",
            escape_surql_str(&round)
        ));
    }
    if item.action == ModelWorkAction::RegenRoot
        && let Some(cohort) = publication_cohort(dbnum, end_sesno)
    {
        clauses.push(format!(
            "cohort_id = IF {end_sesno} >= (source_end_sesno?:0) \
             THEN '{cohort}' ELSE cohort_id END"
        ));
    }
    clauses.extend(revival_clauses);
    // 时刻跟着序号那条单调写入走：本次来源没有比行上已知的更新时，时刻不许退回去
    // （与水位那一列同一个坑）。它读的同样是 `source_end_sesno` 的**旧值**，所以
    // 和复活子句一样排在覆盖之前。
    if let Some(time) = source_end_sesno_time {
        clauses.push(format!(
            "source_end_sesno_time = IF {end_sesno} >= (source_end_sesno?:0) \
             THEN '{}' ELSE source_end_sesno_time END",
            escape_surql_str(time)
        ));
    }
    if item.action == ModelWorkAction::RegenRoot {
        // A revision identifies an immutable source target, not the number of
        // duplicate notifications for it. Replaying the same target is idempotent.
        clauses.push(format!(
            "revision = IF $previous_revision = NONE OR {end_sesno} > ($previous_target?:-1) \
             THEN ($previous_revision?:0) + 1 ELSE $previous_revision END"
        ));
    } else {
        clauses.push("revision = (revision?:0) + 1".to_string());
    }
    // Revision and revival clauses read the old source target, so the monotonic
    // source assignment must remain after both of them.
    clauses.push(format!(
        "source_end_sesno = math::max([source_end_sesno?:0, {end_sesno}])"
    ));
    clauses.push("updated_at = time::now()".to_string());

    let pending_upsert = if item.action == ModelWorkAction::RegenRoot {
        format!(
            "LET $previous = (SELECT * FROM {id} LIMIT 1)[0];\n\
             LET $root_previous = (SELECT * FROM type::thing('gen_root', '{root_id}') LIMIT 1)[0];\n\
             LET $previous_revision = math::max([$previous.revision?:0, $root_previous.desired_revision?:0]);\n\
             LET $previous_target = math::max([$previous.source_end_sesno?:-1, $root_previous.desired_target.source_end_sesno?:-1]);\n\
             UPSERT {id} SET {};",
            clauses.join(", ")
        )
    } else {
        format!("UPSERT {id} SET {};", clauses.join(", "))
    };
    if item.action == ModelWorkAction::DeleteCleanup && dbnum > 0 && end_sesno > 0 {
        // A committed deletion replaces older generation intent for this root.
        // Keep the cleanup job as the durable obligation; an obsolete failed
        // regen must not survive it as a permanent dead letter. A later recreate
        // (or an unversioned manual request) cannot be cancelled by this window.
        let regen_id = record_id_of(ModelWorkAction::RegenRoot, &item.target_refno);
        return format!(
            "{pending_upsert}\nDELETE {regen_id} WHERE dbnum = {dbnum} \
             AND action = 'regen_root' AND (source_end_sesno?:0) > 0 \
             AND source_end_sesno <= {end_sesno};"
        );
    }
    if item.action != ModelWorkAction::RegenRoot {
        return pending_upsert;
    }

    let desired_target = if source_end_sesno_time.is_some() {
        "{dbnum: $desired.dbnum?:0, source_end_sesno: $desired.source_end_sesno?:0, source_end_sesno_time: $desired.source_end_sesno_time}"
    } else {
        "{dbnum: $desired.dbnum?:0, source_end_sesno: $desired.source_end_sesno?:0}"
    };
    // 顶层 `dbnum` 必须在这里写：它是 `gen_root` 行**唯一**的库归属列，启动的凭证下限
    // （`startup_coverage_floors`）、覆盖清单与 `/dbnums` 的模型凭证下限全按 `WHERE dbnum != NONE`
    // 取数。历史上只有 `fn::sync_gen_roots` 写它，而这条路创建的行一直没有——2026-09-03 实测
    // ams 现场 2245 行 `gen_root` 里 2244 行 `dbnum = NONE`，库归属只藏在 `desired_target.dbnum`
    // 这个嵌套字段里。`fn::sync_gen_roots` 随 ADR-058 D4 退出启动路径之后，不在这里补，
    // 新根就永远归属不明，而按 dbnum 取数的每一处都会静默少算（不报错，只是看不见）。
    format!(
        "{pending_upsert}\n\
         LET $desired = (SELECT * FROM ONLY {id});\n\
         UPSERT type::thing('gen_root', '{root_id}') SET\n\
           dbnum = {dbnum},\n\
           desired_revision = $desired.revision,\n\
           desired_target = {desired_target},\n\
           publication_status = IF (published_revision?:0) > 0 THEN 'stale' ELSE 'pending' END,\n\
           last_error = NONE, updated_at = time::now();"
    )
}

/// Make AABB-derived room work part of the same durable plan that advances the watermark.
pub(crate) fn merge_room_recalc_changes(
    plan: &mut ModelUpdatePlan,
    dbnum: u32,
    end_sesno: i32,
    changes: &HashMap<RefnoEnum, String>,
) {
    // 暂存链上房间目标真正变成 durable pending 行的唯一入口（窗口收口与崩溃重放
    // 检查点都经这里），所以房间增量的开关也钉在这里，而不是更早的
    // `defer_room_changes`——包围盒变了这件事照旧记进窗口意图，只是不落成队列行。
    if !crate::options::room_incremental() {
        return;
    }
    let mut existing = plan
        .work_items
        .iter()
        .map(|item| (item.action, item.target_refno.clone()))
        .collect::<BTreeSet<_>>();
    let mut ordered = changes.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(refno, _)| **refno);
    for (&refno, noun) in ordered {
        let item = room_recalc_item_with_source(refno, noun, dbnum, end_sesno);
        if existing.insert((item.action, item.target_refno.clone())) {
            plan.work_items.push(item);
        }
    }
}

#[cfg(test)]
static FAIL_DELETES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Make the next `count` queue-row deletes fail, to exercise the drain's failure
/// isolation without having to take SurrealDB down mid-round.
#[cfg(test)]
fn fail_deletes_for_test(count: usize) {
    FAIL_DELETES.store(count, std::sync::atomic::Ordering::SeqCst);
}

/// 收口的**校验**谓词：`(action, target_refno, revision)` 必须与行上存的一致。
///
/// 它曾经也是**寻址**方式（`DELETE {TABLE} WHERE 谓词`）：算 id 的写法要求「入队时算的 id」与
/// 「收口时算的 id」永远一致，而它们曾经不一致过（见 `record_id_of`：dbnum 位置上有人传 Ref0），
/// 后果是 `DELETE` 静默命中零行——任务清不掉、每一轮都重跑一次完整生成。那次之后
/// `record_id_of` 把 dbnum 从 id 里拿掉了，入参只剩 action / target_refno，与这里校验的正是同两格，
/// 再算不出两个不同的 id；于是寻址回到 record id（见 [`settle_target`]，2026-09-06 性能实测），
/// 谓词留下来只做校验。
fn settle_predicate(action: ModelWorkAction, target_refno: &str, revision: u64) -> String {
    format!(
        "action = '{}' AND target_refno = '{}' AND (revision?:0) = {revision}",
        action.as_str(),
        escape_surql_str(target_refno)
    )
}

/// 收口寻址：**按 record id 取行，谓词只做校验**——`<id> WHERE action = … AND target_refno = … AND revision = …`。
///
/// 2026-09-06 rocksdb 7997 实测（`output/rocksdb-7997-perf-20260906/`）：`model_update_pending` 没有索引，
/// 上面那条谓词在事务内每条全表扫 18k 行 140 ms，一根收口两条（SELECT + DELETE）、16 根一笔 4.5–5.8 s，
/// 占模型消化每页时间的七成；换成按 id 取行同一笔 0.05 s。id 与入队方是同一个 [`record_id_of`]
/// （入参只有 action / target_refno，与谓词校验的正是同两格），算不出两个不同的 id；历史上
/// `{dbnum}_` 前缀的旧行由 [`clear_stale_at_process_start`] 每次启动整表清掉，不再需要谓词兜底。
/// 谓词留在 WHERE 里是为了 revision：更高 revision 顶替过的行 `$work` 取空、不盖章不删行。
fn settle_target(action: ModelWorkAction, target_refno: &str, revision: u64) -> String {
    format!(
        "{} WHERE {}",
        record_id_of(action, target_refno),
        settle_predicate(action, target_refno, revision)
    )
}

pub(crate) fn render_delete_revision(
    action: ModelWorkAction,
    target_refno: &str,
    revision: u64,
) -> String {
    format!("DELETE {};", settle_target(action, target_refno, revision))
}

fn render_delete_work(item: &PendingModelWork) -> String {
    render_delete_revision(item.action, &item.target_refno, item.revision)
}

fn room_target_version_id(action: ModelWorkAction, target_refno: &str) -> String {
    format!(
        "{}_{}",
        action.as_str(),
        target_refno.replace(['/', ':'], "_")
    )
}

/// Settle a room task and retain proof of the published view that produced its
/// visible result. A newer revision makes `$work` empty, so an old calculation
/// cannot remove or stamp the replacement task.
fn render_complete_room_revision(item: &PendingModelWork, published_version: u64) -> String {
    room_model::render_room_write(&[], Some(&room_write_guard(item, published_version)))
}

fn room_write_guard(item: &PendingModelWork, published_version: u64) -> room_model::RoomWriteGuard {
    debug_assert!(item.action.is_room_recalc());
    let target_row = settle_target(item.action, &item.target_refno, item.revision);
    let target = escape_surql_str(&item.target_refno);
    let version_id = escape_surql_str(&room_target_version_id(item.action, &item.target_refno));
    room_model::RoomWriteGuard {
        select_work: format!("(SELECT * FROM {target_row} LIMIT 1)[0]"),
        settle_statements: vec![
            format!(
                "UPSERT type::thing('{ROOM_TARGET_VERSION_TABLE}', '{version_id}') SET \
                 action = '{}', target_refno = '{target}', \
                 computed_publish_version = math::max([computed_publish_version?:0, {published_version}]), \
                 completed_revision = {}, updated_at = time::now()",
                item.action.as_str(),
                item.revision,
            ),
            format!("DELETE {target_row}"),
        ],
    }
}

fn render_complete_regen_revision(root_refno: &str, revision: u64) -> String {
    let target_row = settle_target(ModelWorkAction::RegenRoot, root_refno, revision);
    let root_id = escape_surql_str(&root_refno.replace('/', "_"));
    format!(
        "LET $work = (SELECT * FROM {target_row} LIMIT 1)[0];\n\
         IF $work != NONE {{\n\
           UPDATE type::thing('gen_root', '{root_id}') SET\n\
             status = 'Generated',\n\
             publication_status = 'ready',\n\
             desired_revision = $work.revision,\n\
             published_revision = $work.revision,\n\
             published_target = {{dbnum: $work.dbnum?:0, source_end_sesno: $work.source_end_sesno?:0, source_end_sesno_time: $work.source_end_sesno_time}},\n\
             source_end_sesno = $work.source_end_sesno?:0,\n\
             source_end_sesno_time = $work.source_end_sesno_time,\n\
             last_error = NONE,\n\
             updated_at = time::now();\n\
           DELETE {target_row};\n\
         }};"
    )
}

async fn delete_work(item: &PendingModelWork) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_DELETES
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |remaining| remaining.checked_sub(1),
        )
        .is_ok()
    {
        anyhow::bail!("injected queue cleanup failure");
    }

    delete_work_on(&SUL_DB, item).await
}

async fn delete_work_on(db: &Surreal<Any>, item: &PendingModelWork) -> anyhow::Result<()> {
    let target = &item.target_refno;
    db.query(render_delete_work(item))
        .await
        .map_err(|error| anyhow::anyhow!("delete completed model work {target} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("delete completed model work {target} statement failed: {error}")
        })?;
    Ok(())
}

async fn complete_room_work(item: &PendingModelWork, published_version: u64) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_DELETES
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |remaining| remaining.checked_sub(1),
        )
        .is_ok()
    {
        anyhow::bail!("injected queue cleanup failure");
    }

    let target = &item.target_refno;
    SUL_DB
        .query(render_complete_room_revision(item, published_version))
        .await
        .map_err(|error| anyhow::anyhow!("complete room work {target} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("complete room work {target} statement failed: {error}")
        })?;
    Ok(())
}

fn render_mark_failed_revision(
    action: ModelWorkAction,
    target_refno: &str,
    revision: u64,
    error: &str,
) -> String {
    let error = escape_surql_str(error);
    let attempts = if action.is_room_recalc() {
        String::new()
    } else {
        "attempts = (attempts?:0) + 1, ".to_string()
    };
    // `UPDATE <id> SET … WHERE <谓词>`：与收口同一套按 id 寻址 + 谓词校验（见 `settle_target`）。
    let pending = format!(
        "UPDATE {} SET status = 'failed', {attempts}\
         last_error = '{error}', updated_at = time::now() \
         WHERE {};",
        record_id_of(action, target_refno),
        settle_predicate(action, target_refno, revision)
    );
    if action != ModelWorkAction::RegenRoot {
        return pending;
    }
    let root_id = escape_surql_str(&target_refno.replace('/', "_"));
    format!(
        "{pending}\n\
         UPDATE type::thing('gen_root', '{root_id}') SET\n\
           publication_status = IF (published_revision?:0) > 0 THEN 'stale' ELSE 'failed' END,\n\
           last_error = '{error}', updated_at = time::now()\n\
           WHERE desired_revision = {revision};"
    )
}

fn render_mark_failed(item: &PendingModelWork, error: &str) -> String {
    render_mark_failed_revision(item.action, &item.target_refno, item.revision, error)
}

async fn mark_failed(item: &PendingModelWork, error: &str) -> anyhow::Result<()> {
    let target = &item.target_refno;
    SUL_DB
        .query(render_mark_failed(item, error))
        .await
        .map_err(|query_error| anyhow::anyhow!("mark model work {target} failed: {query_error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("mark model work {target} statement failed: {error}"))?;
    Ok(())
}

/// 确保 `(regen_root, root)` 存在一行 durable pending，返回它的收口令牌（spec §4.5）。
///
/// 按需生成（ensure）在真正跑生成**之前**调它：曾经那条路只读现有行，表里本来没有
/// 这个根时 `expected_revision` 是 `None`、收口是 no-op——一次纯按需生成在进程中途
/// 崩溃后不留任何持久痕迹，没有 drain 会把它捡回来，只能靠人再点一次。先落行之后：
/// 崩溃 → 行还在（status = pending），空闲轮 `drain_data_phases` 接手；成功 → 按本次
/// revision 收口，期间若有新触发把 revision 又推高，行留给 drain，不误删新工作。
///
/// 走与所有入队方相同的 [`render_upsert`]：不认领会话号（`source_end_sesno = 0`，
/// 人在主动要求生成，无条件复活死信正是想要的语义）、不认领来源库（`dbnum = 0`，
/// 这一层没有 Ref0→dbnum 的反查结果，见 [`derived_regen_item`]）。
pub async fn ensure_regen_pending(root_refno: &str, noun: &str) -> anyhow::Result<u64> {
    let item = ModelWorkItem {
        dbnum: 0,
        db_type: "DESI".to_string(),
        source_end_sesno: 0,
        action: ModelWorkAction::RegenRoot,
        target_refno: root_refno.to_string(),
        noun: noun.to_string(),
    };
    SUL_DB
        // 不认领会话号的行本来就没有来源保存可说，来源段整个不摆。
        .query(render_upsert(&item, None))
        .await
        .map_err(|error| anyhow::anyhow!("persist ensure pending {root_refno} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("persist ensure pending {root_refno} statement failed: {error}")
        })?;
    current_regen_revision(root_refno)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ensure 落 pending 之后读不到行: {root_refno}"))
}

/// 生成根在当前投影下的生成源版本（ADR-054）：所在库由 MDB 的 DESI 文件按索引点查得出，
/// 时点是该文件此刻的最新会话。**不经 `pe`、不经 `dbnum_watermark`**——零解析项目也要能
/// 点看即生成，而文件比库新的那段时间用户要的是文件那一版。
async fn root_model_source(
    root_refno: &str,
) -> anyhow::Result<crate::data_interface::model_source::SourceVersion> {
    let root = crate::fast_model::e3d_model_service::parse_refno(root_refno)?;
    tokio::task::spawn_blocking(move || {
        crate::data_interface::model_source::root_source_version(root)
    })
    .await
    .map_err(|error| anyhow::anyhow!("resolve model source {root_refno} task failed: {error}"))?
    .map_err(|error| error.context(format!("生成根 {root_refno} 无法判定生成源版本")))
}

/// 为按需生成落一条认领**当前生成源版本**（文件最新会话，ADR-054）的 durable 工作。
///
/// 旧的 [`ensure_regen_pending`] 是人工强制重试语义，故意写 `source_end_sesno=0`；
/// 自动按需显示不能沿用它，否则生成完成后凭证仍是 0，下一次显示永远判成过期。
pub async fn ensure_regen_pending_current(root_refno: &str, noun: &str) -> anyhow::Result<u64> {
    let version = root_model_source(root_refno).await?;
    let dbnum = version.dbnum;
    let item = ModelWorkItem {
        dbnum,
        db_type: "DESI".to_string(),
        source_end_sesno: version.sesno as i32,
        action: ModelWorkAction::RegenRoot,
        target_refno: root_refno.to_string(),
        noun: noun.to_string(),
    };
    let root_id = escape_surql_str(&root_refno.replace('/', "_"));
    let noun = escape_surql_str(noun);
    let transaction = format!(
        "BEGIN TRANSACTION;\n\
         UPSERT type::thing('gen_root', '{root_id}') SET \
           pe = type::thing('pe', '{root_id}'), noun = '{noun}', dbnum = {dbnum};\n\
         {}\n\
         COMMIT TRANSACTION;",
        render_upsert(&item, version.session_time.as_deref())
    );
    SUL_DB
        .query(transaction)
        .await
        .map_err(|error| {
            anyhow::anyhow!("persist current ensure pending {root_refno} failed: {error}")
        })?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("persist current ensure pending {root_refno} statement failed: {error}")
        })?;
    current_regen_revision(root_refno)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ensure 落当前水位 pending 之后读不到行: {root_refno}"))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelCoverageReport {
    pub dbnum: u32,
    pub expected_roots: usize,
    pub completed_roots: usize,
    pub enqueued_roots: usize,
    pub source_end_sesno: i32,
    pub source_end_sesno_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenRootCoverageRow {
    pe: surrealdb::sql::Thing,
    noun: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    source_end_sesno: Option<i32>,
    #[serde(default)]
    source_end_sesno_time: Option<String>,
}

/// 凭证是否覆盖要求的会话号（ADR-054 实施约束 4：单调，不判等值；时刻只记不比）。
fn gen_root_credential_is_current(row: &GenRootCoverageRow, source_end_sesno: i32) -> bool {
    matches!(
        row.status.as_deref(),
        Some("Generated" | "AlreadyAvailable" | "NoRenderableGeometry")
    ) && crate::data_interface::model_source::credential_covers(
        row.source_end_sesno,
        source_end_sesno,
    )
}

/// 用权威 `fn::sync_gen_roots` 重算当前根覆盖，并把当前水位快照下缺少完成凭证的根
/// 投入同一个 durable 模型队列。`force_all` 只影响是否重跑，既不删旧模型，也不改水位。
pub async fn sync_and_seed_model_coverage(
    dbnum: u32,
    force_all: bool,
) -> anyhow::Result<ModelCoverageReport> {
    if !crate::data_interface::watch_scope::admits(dbnum) {
        anyhow::bail!(crate::data_interface::watch_scope::excluded_reason(dbnum));
    }
    let started = Instant::now();
    let sync_result = SUL_DB
        .query(format!("RETURN fn::sync_gen_roots({dbnum});"))
        .await;
    crate::data_interface::model_concurrency::record_surreal_write(started.elapsed(), false);
    sync_result
        .map_err(|error| anyhow::anyhow!("sync gen_root dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("sync gen_root dbnum={dbnum} statement failed: {error}")
        })?;

    let state = crate::data_interface::dbnum_state::DbnumState::read(dbnum)
        .await?
        .ok_or_else(|| anyhow::anyhow!("dbnum={dbnum} has no applied watermark"))?;
    let started = Instant::now();
    let load_result = SUL_DB
        .query(format!(
            "SELECT pe, noun, subtree, status, source_end_sesno, source_end_sesno_time \
             FROM gen_root WHERE dbnum = {dbnum} ORDER BY subtree DESC;"
        ))
        .await;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let mut response = load_result
        .map_err(|error| anyhow::anyhow!("load gen_root dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("load gen_root dbnum={dbnum} statement failed: {error}")
        })?;
    let rows: Vec<GenRootCoverageRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode gen_root dbnum={dbnum} failed: {error}"))?;
    // 管线路径的「要求时点」是窗口右端 = 已应用水位（ADR-054 Q2：显式时点照用）；
    // 判据单调，所以 ensure 已按更新的文件会话生成过的根在这里算完成，不重排。
    let completed_roots = rows
        .iter()
        .filter(|row| gen_root_credential_is_current(row, state.applied_sesno))
        .count();
    let mut statements = Vec::new();
    let mut enqueued_roots = 0usize;
    for row in &rows {
        if !force_all && gen_root_credential_is_current(row, state.applied_sesno) {
            continue;
        }
        // 队列字段一律用斜杠拼法。`RefnoEnum` 的 `Display` 是下划线 `A_B`，与其余入队
        // 方（`to_pdms_str()` 的 `A/B`）不同，而 `record_id_of` 把斜杠折成下划线，两种
        // 拼法落在**同一行**上：不会长出重复行，但字段值变成「谁最后写谁说了算」。
        // 拿字符串精确查这个字段的地方（`current_regen_revision`、`retry_pending_unit`）
        // 于是会命中零行、返回 `Ok(None)`，与「本来就没有这行」无从区分。
        let root = crate::data_interface::helper::pe_thing_to_refno(row.pe.clone())?.to_pdms_str();
        let item = ModelWorkItem {
            dbnum,
            db_type: "DESI".to_string(),
            source_end_sesno: state.applied_sesno,
            action: ModelWorkAction::RegenRoot,
            target_refno: root.clone(),
            noun: row.noun.clone(),
        };
        let mut statement = render_upsert(&item, state.applied_sesno_time.as_deref());
        if force_all {
            statement.push_str(&format!(
                "\nUPDATE {TABLE} SET attempts = 0, status = 'pending', last_error = NONE \
                 WHERE action = 'regen_root' AND target_refno = '{}';",
                escape_surql_str(&root)
            ));
        }
        statements.push(statement);
        enqueued_roots += 1;
    }
    for chunk in statements.chunks(QUERY_CHUNK) {
        let started = Instant::now();
        let seed_result = SUL_DB
            .query(format!(
                "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
                chunk.join("\n")
            ))
            .await;
        crate::data_interface::model_concurrency::record_surreal_write(started.elapsed(), false);
        seed_result
            .map_err(|error| anyhow::anyhow!("seed model coverage dbnum={dbnum} failed: {error}"))?
            .check()
            .map_err(|error| {
                anyhow::anyhow!("seed model coverage dbnum={dbnum} statement failed: {error}")
            })?;
    }
    Ok(ModelCoverageReport {
        dbnum,
        expected_roots: rows.len(),
        completed_roots,
        enqueued_roots,
        source_end_sesno: state.applied_sesno,
        source_end_sesno_time: state.applied_sesno_time,
    })
}

/// 取该生成根当前的收口令牌。存量表里同一个根可能还留着一条旧 id 的行，取较大的
/// revision：收口只清掉这一版，另一版留给 drain 正常消化，绝不会误删更新的工作。
pub async fn current_regen_revision(root_refno: &str) -> anyhow::Result<Option<u64>> {
    let action = ModelWorkAction::RegenRoot.as_str();
    let target = escape_surql_str(root_refno);
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE revision?:0 FROM {TABLE} \
             WHERE action = '{action}' AND target_refno = '{target}';"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("load model work revision {root_refno} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("load model work revision {root_refno} statement failed: {error}")
        })?;
    let revisions: Vec<u64> = response.take(0).map_err(|error| {
        anyhow::anyhow!("decode model work revision {root_refno} failed: {error}")
    })?;
    Ok(revisions.into_iter().max())
}

async fn clear_regen_work_revision(root_refno: &str, revision: u64) -> anyhow::Result<()> {
    SUL_DB
        .query(format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            render_complete_regen_revision(root_refno, revision)
        ))
        .await
        .map_err(|error| {
            anyhow::anyhow!("delete completed model work {root_refno} failed: {error}")
        })?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("delete completed model work {root_refno} statement failed: {error}")
        })?;
    Ok(())
}

fn render_clear_regen_transactions(items: &[(String, u64)]) -> Vec<String> {
    items
        .chunks(QUERY_CHUNK)
        .map(|chunk| {
            let deletes = chunk
                .iter()
                .map(|(root_refno, revision)| render_complete_regen_revision(root_refno, *revision))
                .collect::<Vec<_>>()
                .join("\n");
            format!("BEGIN TRANSACTION;\n{deletes}\nCOMMIT TRANSACTION;")
        })
        .collect()
}

pub(crate) async fn clear_regen_work_batch(items: &[(String, u64)]) -> anyhow::Result<()> {
    for transaction in render_clear_regen_transactions(items) {
        let started = Instant::now();
        let settle_result = SUL_DB.query(transaction).await;
        crate::data_interface::model_concurrency::record_surreal_write(started.elapsed(), false);
        settle_result
            .map_err(|error| anyhow::anyhow!("delete completed model work batch failed: {error}"))?
            .check()
            .map_err(|error| {
                anyhow::anyhow!("delete completed model work batch statement failed: {error}")
            })?;
    }
    Ok(())
}

async fn mark_regen_revision_failed(
    root_refno: &str,
    revision: u64,
    error: &str,
) -> anyhow::Result<()> {
    SUL_DB
        .query(render_mark_failed_revision(
            ModelWorkAction::RegenRoot,
            root_refno,
            revision,
            error,
        ))
        .await
        .map_err(|query_error| {
            anyhow::anyhow!("mark model work {root_refno} failed: {query_error}")
        })?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("mark model work {root_refno} statement failed: {error}")
        })?;
    Ok(())
}

pub async fn settle_regen_work(
    root_refno: &str,
    expected_revision: Option<u64>,
    generation_error: Option<&str>,
) -> anyhow::Result<()> {
    let Some(revision) = expected_revision else {
        return Ok(());
    };
    match generation_error {
        Some(error) => mark_regen_revision_failed(root_refno, revision, error).await,
        None => clear_regen_work_revision(root_refno, revision).await,
    }
}

/// 人工复活一行待重试任务的 UPDATE（spec §4.6.1，纯渲染）。
///
/// 只允许操作**已存在**的 `(action, target_refno)`——这个端点是「复活」不是「入队」，
/// 入队有自己的窗口与级联语义，不能从这里绕。原子地 `revision += 1`（旧收口令牌全部
/// 作废，正在跑的那次成功后删不掉这行，留给 drain——与并发触发的既有语义一致）、
/// `attempts = 0`、清 `last_error`，下一轮 drain 重新取到它。
fn render_retry_pending_unit(action: ModelWorkAction, target_refno: &str) -> String {
    format!(
        "UPDATE {TABLE} SET revision = (revision?:0) + 1, attempts = 0, \
         last_error = NONE, status = 'pending', updated_at = time::now() \
         WHERE action = '{}' AND target_refno = '{}' RETURN AFTER;",
        action.as_str(),
        escape_surql_str(target_refno)
    )
}

/// 人工复活一行待重试任务（死信的唯一 HTTP 出口，spec §4.6.1）。
///
/// 自动路径的复活（[`render_upsert`] 按会话号 / 无条件两种判据）覆盖不到「认领了
/// 会话号、又没有更新会话到来」的死信——[`render_drain_select`] 的 attempts 上限
/// 把它们永远挡在外面，此前除了直接改库没有第二条路。
///
/// 返回 `None` 表示表里没有这行（HTTP 层回 404）。同一谓词命中多行时（历史遗留的
/// `{dbnum}_` 前缀旧行），全部复活并返回 revision 最大的那行作回执。
pub async fn retry_pending_unit(
    action: ModelWorkAction,
    target_refno: &str,
) -> anyhow::Result<Option<PendingModelWork>> {
    let mut response = SUL_DB
        .query(render_retry_pending_unit(action, target_refno))
        .await
        .map_err(|error| anyhow::anyhow!("revive pending unit {target_refno} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("revive pending unit {target_refno} statement failed: {error}")
        })?;
    let rows: Vec<PendingModelWork> = response.take(0).map_err(|error| {
        anyhow::anyhow!("decode revived pending unit {target_refno} failed: {error}")
    })?;
    Ok(rows.into_iter().max_by_key(|row| row.revision))
}

/// Regeneration work for one root a reverse cascade discovered (pure).
///
/// The derived root is NOT booked against the seed's catalogue `dbnum`: filing a
/// design root there meant a dead letter could only ever be revived by a new
/// CATALOGUE session, while the design sessions that actually need it
/// regenerated could never reach it. `expand_live_reverse_cascade` drops every
/// referrer whose **real** `pe.dbnum` belongs to a non-design database — it used
/// to compare `RefU64::get_0()` (a Ref0, not a dbnum) against that set, which
/// both let catalogue intermediates through and silently dropped design
/// referrers whose Ref0 happened to collide. So what arrives here is a design
/// root, and a referrer whose dbnum cannot be resolved is kept rather than lost.
///
/// 但这里也**不能**填 `root.refno().get_0()`——那是 Ref0，不是 dbnum（见
/// `record_id_of`）。自从行 id 不再带 dbnum，这个字段只剩下路由与追踪用途，填 0
/// 表示「来源库未解析」：这一层没有 Ref0→dbnum 的反查结果，而一个看着像真 dbnum
/// 的 Ref0 会把这行误挂到别的库名下、被那个库的批次工作单捞走。留 0 之后它由空闲轮
/// 的 `drain_data_phases` 统一消化，下一次真正的 DESI 窗口再 upsert 时会补上真值。
///
/// `source_end_sesno` is 0 rather than the seed's: session numbers are
/// per-database, so a catalogue sesno of 500 sitting next to design sessions in
/// the 80s would block revival outright. 0 claims no session, which lets the
/// next real session on the design db reset the attempt count as intended.
fn derived_regen_item(
    root: crate::data_interface::generation_root::GenerationRoot,
) -> ModelWorkItem {
    ModelWorkItem {
        dbnum: 0,
        db_type: "DESI".to_string(),
        source_end_sesno: 0,
        action: ModelWorkAction::RegenRoot,
        target_refno: root.root.to_pdms_str(),
        noun: root.noun,
    }
}

async fn execute_item(mgr: &AiosDBManager, item: &PendingModelWork) -> anyhow::Result<()> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&item.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid pending refno {}", item.target_refno))?,
    );
    if let Some((round_dbnum, round_end)) =
        crate::data_interface::batch_worker::active_data_window()
        && matches!(
            item.action,
            ModelWorkAction::Transform
                | ModelWorkAction::DeleteCleanup
                | ModelWorkAction::PostRegenAabb
        )
    {
        let statement = crate::data_interface::side_effect_pending::SideEffectCompensator::
            render_deferred_file_model_action(
                round_dbnum,
                round_end,
                crate::data_interface::side_effect_pending::DeferredFileModelAction {
                    action: item.action.as_str().to_string(),
                    target_refno: item.target_refno.clone(),
                    noun: item.noun.clone(),
                },
            )?;
        SUL_DB.query(statement).await?.check()?;
        return Ok(());
    }
    match item.action {
        ModelWorkAction::RegenRoot => {
            if item.required_panels.is_empty() {
                crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(
                    mgr,
                    &[item.target_refno.clone()],
                )
                .await?;
            } else {
                // 修复排队后房间拓扑仍可能变化。先确认至少还有一块在册 PANE 需要这个根，
                // 避免合法删除了面板/生成根之后，先生成一个已不存在的根而把修复任务打进死信。
                let rooms = room_model::load_room_panel_map(&mgr.db_option).await?;
                let registered = registered_required_panels(&rooms, &item.required_panels)?;
                if !registered.is_empty() {
                    crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(
                        mgr,
                        &[item.target_refno.clone()],
                    )
                    .await?;
                }
                verify_required_panel_geometry(mgr, &item.required_panels).await?;
            }
            Ok(())
        }
        ModelWorkAction::Transform => mgr.update_world_transforms(&HashSet::from([refno])).await,
        ModelWorkAction::DeleteCleanup => {
            // The cleanup abstraction must retire both publication faces. In read-through
            // mode the authoritative Plant UI projection lives in ModelMemoryStore; deleting
            // only the Surreal mirror leaves `/model/records` serving a ghost subtree until
            // process restart. `delete_model_subtree` also writes the mirror tombstone before
            // atomically removing the in-memory records and invalidating affected receipts.
            mgr.delete_model_subtree(refno).await?;
            if item.noun.trim().is_empty() {
                let key = item.target_refno.replace('/', "_");
                SUL_DB
                    .query(format!(
                        "DELETE gen_root:{key} WHERE pe = NONE AND noun = NONE;"
                    ))
                    .await?
                    .check()?;
            }
            Ok(())
        }
        ModelWorkAction::CascadeExpand => {
            let roots =
                crate::data_interface::manual_update::expand_live_reverse_cascade(refno).await?;
            enqueue_plan(&ModelUpdatePlan {
                work_items: roots.into_iter().map(derived_regen_item).collect(),
                ..Default::default()
            })
            .await
        }
        ModelWorkAction::PostRegenAabb => refresh_post_regen_aabbs(&[refno]).await.map(|_| ()),
        // 单件执行路径：自己加载一次房间映射。批量消费走 [`drain_rooms`]，它按轮加载
        // 一次并在整轮复用——房间映射是一次房间类型表全表扫描加逐行图遍历，几十个任务
        // 各扫一遍是承受不起的。
        ModelWorkAction::RoomRecalcElement | ModelWorkAction::RoomRecalcPanel => {
            // 常驻模型在这条路上恒不参与（`None`）：换代点的前提是调用方已持
            // `lock_spatial_serial`，而单件执行路径不在那段串行区里，取模型就是拿一份没人
            // 保证还对得上的拓扑改写存量边。批量消费（`drain_rooms_selected`）才是接常驻
            // 的地方，这里照旧每次各查一次拓扑与几何。
            let rooms = room_model::load_room_panel_map(&mgr.db_option).await?;
            let panels =
                room_model::load_consumption_panel_index(&mgr.db_option, &rooms, None).await?;
            // 整间任务用不到构件的旧归属快照，别为它多发一条查询。
            let history = if matches!(item.action, ModelWorkAction::RoomRecalcElement) {
                room_model::ElementRoomHistory::load(&[refno]).await?
            } else {
                room_model::ElementRoomHistory::default()
            };
            match item.action {
                ModelWorkAction::RoomRecalcPanel => {
                    room_model::recalc_panel_membership_for_consumption(
                        &mgr.db_option,
                        &rooms,
                        refno,
                        None,
                    )
                    .await
                    .map(|_| ())
                }
                ModelWorkAction::RoomRecalcElement => {
                    room_model::recalc_element_membership(&rooms, &panels, &history, refno).await
                }
                _ => unreachable!("room arm only receives room actions"),
            }
        }
    }
}

pub(crate) async fn execute_deferred_file_model_action(
    mgr: &AiosDBManager,
    action: &crate::data_interface::side_effect_pending::DeferredFileModelAction,
) -> anyhow::Result<()> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&action.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid deferred refno {}", action.target_refno))?,
    );
    match action.action.as_str() {
        "transform" => mgr.update_world_transforms(&HashSet::from([refno])).await,
        "delete_cleanup" => {
            mgr.delete_model_subtree(refno).await?;
            if action.noun.trim().is_empty() {
                let key = action.target_refno.replace('/', "_");
                SUL_DB
                    .query(format!(
                        "DELETE gen_root:{key} WHERE pe = NONE AND noun = NONE;"
                    ))
                    .await?
                    .check()?;
            }
            Ok(())
        }
        "post_regen_aabb" => refresh_post_regen_aabbs(&[refno]).await.map(|_| ()),
        other => anyhow::bail!("unknown deferred file model action {other}"),
    }
}

fn missing_required_panels(required: &[String], available: &HashSet<String>) -> Vec<String> {
    required
        .iter()
        .filter(|panel| !available.contains(panel.as_str()))
        .cloned()
        .collect()
}

fn registered_required_panels(
    rooms: &room_model::RoomPanelMap,
    required: &[String],
) -> anyhow::Result<Vec<RefnoEnum>> {
    required
        .iter()
        .map(|panel| {
            RefU64::from_str(panel)
                .map(RefnoEnum::from)
                .map_err(|_| anyhow::anyhow!("invalid required panel refno {panel}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .map(|panels| {
            panels
                .into_iter()
                .filter(|panel| rooms.room_num_of(*panel).is_some())
                .collect()
        })
}

/// 面板补偿不是“生成调用返回 Ok”就算成功：房间计算真正依赖的是每块 PANE 都能从
/// `inst_relate` 读到有效 AABB 与 world transform。后置条件不满足时保留同一生成根
/// pending 并累计 attempts，最终进入可观测死信，而不是十分钟后重新造一条新任务。
async fn verify_required_panel_geometry(
    mgr: &AiosDBManager,
    required: &[String],
) -> anyhow::Result<()> {
    let rooms = room_model::load_room_panel_map(&mgr.db_option).await?;
    // 拓扑可能在补偿排队后继续变化。已经移出合规房间的旧 PANE 不再是房间覆盖条件，
    // 否则一个合法删除会把生成根推入永远无法满足的死信。
    let required_now = registered_required_panels(&rooms, required)?;
    let available = crate::data_interface::staging::query_valid_insts(&required_now)
        .await?
        .into_iter()
        .map(|inst| inst.refno.to_pdms_str())
        .collect::<HashSet<_>>();
    let required_now = required_now
        .iter()
        .map(RefnoEnum::to_pdms_str)
        .collect::<Vec<_>>();
    let missing = missing_required_panels(&required_now, &available);
    if !missing.is_empty() {
        anyhow::bail!(
            "生成根执行完成后仍有 {} 块必需房间面板缺少有效 inst_relate/AABB/world_trans: {}",
            missing.len(),
            missing
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // 局部根满足后再对一次全局缺陷清单：全齐了就销账，没齐就把最新缺口记上。
    let panel_index = room_model::load_panel_index(&mgr.db_option, &rooms).await?;
    if panel_index.ensure_complete().is_ok() {
        clear_room_panel_defects().await?;
        println!("[房间缺陷] 在册面板几何已完整，缺陷登记已销账");
    } else {
        // 修复波次执行期间可能又有 PANE 新增或丢失几何。把最新缺口并入 durable
        // 根任务；若恰好落到当前根，revision 会递增，使下面的旧令牌收口命中零行。
        // 这样原波次全部结束后仍有工作能够最终把清单清空。
        record_room_panel_defects(rooms.rooms.len(), panel_index.missing_panels()).await;
    }
    Ok(())
}

/// 一页修复根的验收结论：通过的收口令牌 + 未通过的逐条失败（下标指回入参）。
struct RepairVerifyPage {
    passed: Vec<(String, u64)>,
    failed: Vec<(usize, String)>,
}

/// 修复根的整页合并验收（ADR-011 2026-08-09 修订）。
///
/// 语义与单件路径 [`verify_required_panel_geometry`] 逐字对齐，但房间映射、有效
/// 实例查询、在册面板索引与屏障维护**整页只做一次**——单件路径每个根各付一遍
/// 这四样，正是空闲轮「一轮几秒、百余个修复根连刷十几分钟」的来源。
///
/// 屏障维护先于收口（与 `run_one` 的 execute→delete 顺序同构）：
/// [`enqueue_missing_panel_repairs`] 若把本页某个根的 revision 推高，随后按旧令牌
/// 的收口就命中零行，那行留给下一轮——不误删新工作。
async fn verify_repair_jobs_page(
    mgr: &AiosDBManager,
    rooms: &room_model::RoomPanelMap,
    jobs: &[&PendingModelWork],
) -> anyhow::Result<RepairVerifyPage> {
    let mut page = RepairVerifyPage {
        passed: Vec::new(),
        failed: Vec::new(),
    };
    let mut per_job: Vec<(usize, Vec<RefnoEnum>)> = Vec::with_capacity(jobs.len());
    let mut union: Vec<RefnoEnum> = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        match registered_required_panels(rooms, &job.required_panels) {
            Ok(registered) => {
                union.extend(registered.iter().copied());
                per_job.push((index, registered));
            }
            Err(error) => page.failed.push((index, format!("{error:#}"))),
        }
    }
    union.sort_unstable();
    union.dedup();
    let available = crate::data_interface::staging::query_valid_insts(&union)
        .await?
        .into_iter()
        .map(|inst| inst.refno.to_pdms_str())
        .collect::<HashSet<_>>();

    let panel_index = room_model::load_panel_index(&mgr.db_option, rooms).await?;
    if panel_index.ensure_complete().is_ok() {
        clear_room_panel_defects().await?;
        println!("[房间缺陷] 在册面板几何已完整，缺陷登记已销账");
    } else {
        record_room_panel_defects(rooms.rooms.len(), panel_index.missing_panels()).await;
    }

    for (index, registered) in per_job {
        let job = jobs[index];
        let required_now = registered
            .iter()
            .map(RefnoEnum::to_pdms_str)
            .collect::<Vec<_>>();
        let missing = missing_required_panels(&required_now, &available);
        if missing.is_empty() {
            page.passed.push((job.target_refno.clone(), job.revision));
        } else {
            page.failed.push((
                index,
                format!(
                    "生成根执行完成后仍有 {} 块必需房间面板缺少有效 inst_relate/AABB/world_trans: {}",
                    missing.len(),
                    missing
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    Ok(page)
}

/// 执行一个房间重算任务，返回本次写入了归属边的构件集合。
async fn run_room_task(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panels: &room_model::PanelIndex,
    history: &room_model::ElementRoomHistory,
    job: &PendingModelWork,
    target: RefnoEnum,
    published_version: u64,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    #[cfg(test)]
    if FAIL_DELETES
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |remaining| remaining.checked_sub(1),
        )
        .is_ok()
    {
        anyhow::bail!("injected queue cleanup failure");
    }

    let guard = room_write_guard(job, published_version);
    match job.action {
        ModelWorkAction::RoomRecalcPanel => {
            // 面板侧几何与元素分支同源：两条分支都从这一轮那份索引取，不可能一个走常驻、
            // 一个走库内查询（`PanelIndex::resident`）。
            room_model::recalc_panel_membership_guarded(
                db_option,
                rooms,
                target,
                &guard,
                panels.resident(),
            )
            .await
        }
        ModelWorkAction::RoomRecalcElement => {
            room_model::recalc_element_membership_guarded(rooms, panels, history, target, &guard)
                .await?;
            Ok(HashSet::new())
        }
        other => anyhow::bail!("{} 不是房间任务", other.as_str()),
    }
}

/// 一轮 drain 的产出：完成数、逐条失败原因，以及失败牵涉到的 `dbnum`。
///
/// 失败的 `dbnum` 要单独带出来，是因为非 regen 积压是**全局**的：批次执行前那次
/// `drain_non_regen` 会扫掉所有库的位姿/删除/级联工作。只报一个「这轮有失败」的
/// 布尔值，调用方就分不清失败的是自己这一批还是隔壁库，只能一律按前置失败阻断
/// 自己的模型生成——一条坏行于是停掉全线。
#[derive(Debug, Default)]
pub struct DrainReport {
    /// Number of durable rows requested by the caller's selection.
    pub requested: usize,
    /// Number of live rows actually loaded from the durable queue.
    pub loaded: usize,
    pub done: usize,
    pub failures: Vec<String>,
    pub failed_dbnums: BTreeSet<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelDrainYieldReason {
    EpochChanged,
    ModelGateClosed,
    DataQueued,
    InitializationNotReady,
    RootLockBusy,
}

impl ModelDrainYieldReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EpochChanged => "epoch_changed",
            Self::ModelGateClosed => "model_gate_closed",
            Self::DataQueued => "data_queued",
            Self::InitializationNotReady => "initialization_not_ready",
            Self::RootLockBusy => "root_lock_busy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDrainDisposition {
    Completed {
        done: usize,
    },
    YieldedForData {
        done: usize,
        claimed_epoch: u64,
        current_epoch: u64,
        reason: ModelDrainYieldReason,
    },
    Failed {
        done: usize,
        message: String,
    },
    /// 本页所有 regen 根都正被其他生成者持有。任务行原样保留，不消耗重试。
    DeferredForLock {
        done: usize,
        busy: usize,
        sample: Option<String>,
    },
}

impl ModelDrainDisposition {
    pub const fn done(&self) -> usize {
        match self {
            Self::Completed { done }
            | Self::YieldedForData { done, .. }
            | Self::Failed { done, .. }
            | Self::DeferredForLock { done, .. } => *done,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct ModelDrainTelemetry {
    total_yields: u64,
    last_yield_reason: Option<&'static str>,
    last_claimed_epoch: Option<u64>,
    last_current_epoch: Option<u64>,
    last_root_duration_ms: Option<u64>,
    /// 数据在单根生成期间到达时，它最多等了这一根的执行时长；认领前已排队则为 0。
    last_data_wait_ms: Option<u64>,
    /// 调度让位不得消费重试；单列出来让 health/live 直接断言。
    attempts_consumed_on_yield: u64,
    /// 最近一次模型页实际写入 pending 行的 attempts 增量。正常完成和让位为 0，
    /// 真实失败且 `mark_failed` 成功时按根累计。
    last_attempts_delta: u64,
    last_stage: Option<&'static str>,
    last_page_claimed: usize,
    last_page_completed: usize,
    last_remaining: usize,
    last_deferred_lock_count: usize,
    last_deferred_lock_sample: Option<String>,
    /// 连续几页「认领了整页、一个都没收口、待办总数纹丝不动」。
    ///
    /// 上面那三个数一直都在，却没有任何人拿它们做判断：2026-08-27 现场连着 118 页
    /// `page_claimed=100 / page_completed=0 / remaining=1462`，每一页的任务状态都是
    /// `succeeded`，`/health` 一路报 `ok`。这个计数就是那三个数的消费者。
    /// 墙钟不在这里重复存一份——每页的 `created_at` 在 `/api/v1/tasks` 里。
    starved_pages: u32,
}

static MODEL_DRAIN_TELEMETRY: OnceLock<Mutex<ModelDrainTelemetry>> = OnceLock::new();

fn model_drain_telemetry() -> MutexGuard<'static, ModelDrainTelemetry> {
    MODEL_DRAIN_TELEMETRY
        .get_or_init(|| Mutex::new(ModelDrainTelemetry::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn model_drain_telemetry_snapshot() -> serde_json::Value {
    serde_json::to_value(model_drain_telemetry().clone()).unwrap_or_default()
}

/// 连续多少页「整页认领、零收口、待办不动」就算停滞。
///
/// 3 页 ≈ 80 秒（现场每页约 26 秒）：够长，不会被一次寻常的根锁竞争或 epoch 让位
/// 误触；够短，不至于像 2026-08-27 那样烧掉 50 分钟还得靠人去翻日志才看出来。
pub const MODEL_DRAIN_STARVED_PAGES_ALERT: u32 = 3;

/// 页级停滞是否已经够格阻断。`/health` 用它推 `blocking_conditions`，
/// 启动等待循环用它决定还要不要按 300 秒的节奏闭嘴。
pub fn model_drain_page_starved() -> bool {
    model_drain_telemetry().starved_pages >= MODEL_DRAIN_STARVED_PAGES_ALERT
}

/// 当前连续停滞页数，给日志点名用。
pub fn model_drain_starved_pages() -> u32 {
    model_drain_telemetry().starved_pages
}

/// 上一页是不是白跑了：整页认领、零收口、一个根锁都没挡着，而待办总数到这一页还是
/// 原样。
///
/// 四个条件缺一不可。只看 `completed == 0` 会把「本页全被根锁挡住」也算进来（那是
/// 正常让位，行没动过是对的）；只看待办不变会把「收掉几个、又新排进来几个」误判成
/// 停滞。根锁那一条必须**显式**挡掉，光靠前两条挡不住：整页 lock-busy 走
/// [`DrainPageProgress::settle_deferred_for_lock`] 早退，telemetry 就停在「认领 N /
/// 收口 0 / 待办不变」，与真正的停滞逐字段同形。而 specs/033 的前提正是单个布尔能
/// 跑几分钟——队列里只剩一两个根、恰好都被按需生成占着时，三页 80 秒轻松跨过去，
/// 那是让位不是收不掉。早退之前 `report("lock_deferred", …)` 已经把 deferred 数写
/// 进 telemetry，判据拿得到它。
fn page_was_starved(
    previous_claimed: usize,
    previous_completed: usize,
    previous_deferred_lock_count: usize,
    previous_remaining: usize,
    queue_total: usize,
) -> bool {
    previous_claimed > 0
        && previous_completed == 0
        && previous_deferred_lock_count == 0
        && previous_remaining == queue_total
}

/// 队列见底：连续停滞计数就地清零。
///
/// 计数只在认领新一页时结算，而队列一空 drain 就在「取到零行」那里直接返回、再也
/// 不认领——最后一页若被计为停滞，读数会一直冻在那儿，`/health` degraded 到进程
/// 重启为止。这条路不是假想：未回报的根撞满 [`MAX_ATTEMPTS`] 进死信之后队列正好
/// 会空，`settle_unechoed_roots` 与页级停滞告警于是串成一条谁也清不掉的 degraded。
/// 死信自己有 `blocking_conditions` 会喊，停滞这一条到此为止。
fn clear_page_starvation() {
    model_drain_telemetry().starved_pages = 0;
}

impl DrainReport {
    fn record(&mut self, dbnum: u32, message: String) {
        self.failed_dbnums.insert(dbnum);
        self.failures.push(message);
    }

    /// 这一轮的失败是否够格阻断 `dbnum` 这一批的后续模型生成。
    ///
    /// `dbnum = 0` 是「来源库未知」的入队（见 [`record_id_of`]）：牵连范围无从判断，
    /// 按阻断处理。
    pub fn blocks(&self, dbnum: u32) -> bool {
        self.failed_dbnums.contains(&dbnum) || self.failed_dbnums.contains(&0)
    }

    /// 折回调用方原来的 `Result<usize>` 口径。
    fn into_result(self) -> anyhow::Result<usize> {
        if !self.failures.is_empty() {
            let done = self.done;
            anyhow::bail!(
                "{} pending model task(s) failed after {done} completed: {}",
                self.failures.len(),
                self.failures.join("; ")
            );
        }
        Ok(self.done)
    }
}

/// Record a durable failure for one job and collect it for the drain summary.
///
/// Clearing the queue row counts the same as the work itself: a target whose row
/// can never be removed keeps climbing towards [`MAX_ATTEMPTS`] instead of
/// re-running a full generation every watcher cycle forever.
async fn record_failure(
    job: &PendingModelWork,
    error: &anyhow::Error,
    report: &mut DrainReport,
) -> bool {
    let message = format!("{error:#}");
    if let Err(mark_error) = mark_failed(job, &message).await {
        report.record(
            job.dbnum,
            format!(
                "{} {}: {message}; mark failed: {mark_error:#}",
                job.action.as_str(),
                job.target_refno
            ),
        );
        false
    } else {
        report.record(
            job.dbnum,
            format!("{} {}: {message}", job.action.as_str(), job.target_refno),
        );
        true
    }
}

/// Run one job on its own, recording a durable failure rather than aborting the
/// drain, so a single broken target cannot stall the rest of the queue.
///
/// This is infallible on purpose. Returning `Err` here — as the queue-row delete
/// used to — aborted the whole round on one flaky `DELETE`, so every other
/// `dbnum` queued behind it was skipped and the target that had just generated
/// successfully paid for a second full `gen_all_geos_data` on the next round.
/// 跑一件活，**panic 与 Err 走同一条记账路径**。
///
/// panic 过去是漏网的那一类：`execute_item` 里炸开会一路展开出 drain、出空闲轮，
/// 被 `batch_worker::isolate_panic` 在最外面接住。于是这一行的 `last_error` 没写、
/// `attempts` 没涨、[`MAX_ATTEMPTS`] 那道死信门永远轮不到它，而下一个 `IDLE_WAKE`
/// 又把同一件活原样重演一遍——现场 2026-08-08 的日志里，同一句
/// `range end index 172 out of range for slice of length 168` 就这么每 30 秒刷一次、
/// 刷了 46 次。
///
/// panic 只是这件活失败的一种形式，账要记在它自己那一行上：错误文本进
/// `last_error`，次数进 `attempts`，连撞 [`MAX_ATTEMPTS`] 就和别的失败一样变成
/// 可查的死信，而不是变成一个没人负责的循环。
async fn execute_item_isolated(mgr: &AiosDBManager, job: &PendingModelWork) -> anyhow::Result<()> {
    match crate::data_interface::batch_worker::isolate_panic(execute_item(mgr, job)).await {
        Ok(result) => result,
        Err(reason) => anyhow::bail!("panic: {reason}"),
    }
}

/// 房间任务同理：panic 记进这一行的 `last_error`，不再展开出整轮 drain。
async fn run_room_job_isolated(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panels: &room_model::PanelIndex,
    history: &room_model::ElementRoomHistory,
    job: &PendingModelWork,
    published_version: u64,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    match crate::data_interface::batch_worker::isolate_panic(run_room_job(
        db_option,
        rooms,
        panels,
        history,
        job,
        published_version,
    ))
    .await
    {
        Ok(result) => result,
        Err(reason) => anyhow::bail!("panic: {reason}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOneOutcome {
    Completed,
    Failed,
    YieldedForData,
}

fn is_initialization_not_ready(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<crate::data_interface::initialization_phase::InitializationNotReady>()
    })
}

async fn run_one(
    mgr: &AiosDBManager,
    job: &PendingModelWork,
    report: &mut DrainReport,
) -> RunOneOutcome {
    let root_lock = (job.action == ModelWorkAction::RegenRoot)
        .then(|| crate::data_interface::manual_update::generation_root_lock(&job.target_refno));
    let _root_guard = match &root_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let outcome = match execute_item_isolated(mgr, job).await {
        Ok(()) => delete_work(job).await,
        Err(error) if is_initialization_not_ready(&error) => {
            return RunOneOutcome::YieldedForData;
        }
        Err(error) => Err(error),
    };
    match outcome {
        Ok(()) => {
            report.done += 1;
            RunOneOutcome::Completed
        }
        Err(error) => {
            if record_failure(job, &error, report).await {
                model_drain_telemetry().last_attempts_delta += 1;
            }
            RunOneOutcome::Failed
        }
    }
}

/// Render the drain SELECT. Work at or above [`MAX_ATTEMPTS`] stays in the
/// table as a dead letter: the automatic watcher never picks it up again,
/// while manual preview/retry reads the table without this cap and remains
/// the way to inspect or revive it.
/// Global automatic consumption is bounded by the effective watch scope.
///
/// Keep this as a SQL predicate rather than filtering decoded rows: excluded
/// durable work must never be claimed, executed, or have its retry counter
/// touched by this process.
///
/// `dbnum = 0` means **「不属于任何库」**，不是「0 号库」，所以它落在每个数字域
/// 之外而不是域外——两条产出路径是故意这么写的，各自都有注释与断言钉着：
/// [`room_recalc_item`]（这一层拿不到 Ref0→dbnum 的反查结果，填 Ref0 会把行误挂到
/// 某个恰好同号的库名下）与 [`ensure_regen_pending`]。把它们按数字域排除，等于让
/// 一个带 `--watch-dbnum` 的进程**永远消费不掉自己刚排出来的房间任务**。
///
/// 2026-08-27 现场就是这样：模型阶段收工后，`count_room_targets`（没有这道过滤）
/// 每轮报 2553 个目标，而 [`load_room_jobs`]（有这道过滤）一行都取不出来，于是
/// 「完成 0、失败 0」以每秒一轮的节奏刷了 7697 轮、一小时二十五分钟，`/health`
/// 全程 `ok`。过滤两侧对「哪些行算数」的答案必须是同一个。
///
/// 排除 legacy 行不再是这里的职责：ADR-050 之后 [`clear_stale_at_process_start`]
/// 在任何生产者/消费者启动前整表清空，表里剩下的每一行都是本进程自己写的。
fn render_automatic_scope_filter(dbnums: &[u32]) -> String {
    if dbnums.is_empty() {
        String::new()
    } else {
        let members = dbnums
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!(" AND ((dbnum?:0) = 0 OR (dbnum?:0) IN [{members}])")
    }
}

/// 模型增量类型限定（`model_incremental_types`，见 [`super::model_type_scope`]）的 SQL 谓词。
///
/// 与 [`render_automatic_scope_filter`] 同一条纪律：留在 SQL 里而不是过滤解码后的行——
/// 名单外的 durable 行不得被本进程认领、执行或碰 attempts；计数与加载两侧也必须给出同
/// 一个答案（否则空闲轮会像 2026-08-27 那样自己喂自己空转）。
///
/// 只圈 `regen_root` / `delete_cleanup`（[`super::model_type_scope::GATED_ACTIONS`]）：
/// `transform`（老规划器的便宜路径）、`cascade_expand`（CATA 元素，展开后的派生根另有
/// 自己的 regen 行）、`post_regen_aabb` 与房间任务不看名单。**noun 为空的行放行**——
/// 老规划器的 Transform / DeleteCleanup 行不带 noun，挡不准的不挡。名单已在配置层
/// 归一成大写，行上的 noun 再在 SQL 里 uppercase 一次，别让一处小写写法把根静默扣住。
fn render_automatic_type_filter(nouns: &[String]) -> String {
    if nouns.is_empty() {
        return String::new();
    }
    format!(
        " AND (action NOT IN [{gated}] OR (noun?:'') = '' OR \
         string::uppercase(noun?:'') IN [{members}])",
        gated = render_gated_actions(),
        members = render_noun_members(nouns)
    )
}

/// [`render_automatic_type_filter`] 的补集：被类型限定**暂扣**的行。只给 `/health` 的
/// `held_by_model_incremental_types` 计数用——它与放行谓词必须严格互补，否则一行会
/// 同时出现在「待消费」与「被暂扣」两边，或者两边都不见。
fn render_automatic_type_holdback(nouns: &[String]) -> Option<String> {
    (!nouns.is_empty()).then(|| {
        format!(
            " AND action IN [{gated}] AND (noun?:'') != '' AND \
             string::uppercase(noun?:'') NOT IN [{members}]",
            gated = render_gated_actions(),
            members = render_noun_members(nouns)
        )
    })
}

fn render_gated_actions() -> String {
    crate::data_interface::model_type_scope::GATED_ACTIONS
        .iter()
        .map(|action| format!("'{}'", action.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_noun_members(nouns: &[String]) -> String {
    nouns
        .iter()
        .map(|noun| format!("'{}'", escape_surql_str(noun)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_drain_select_for_scope(
    action_filter: &str,
    limit: Option<usize>,
    dbnums: &[u32],
    nouns: &[String],
) -> String {
    render_drain_select_for_scope_with_active_cohorts(
        action_filter,
        limit,
        dbnums,
        nouns,
        &format!(
            "(SELECT VALUE cohort_id FROM {TABLE} WHERE status = 'running' \
             AND lease_until >= time::now() AND cohort_id IS NOT NONE)"
        ),
    )
}

/// A retained local manifest protects its entire database even if a crash
/// interrupted the batched SQL reservation. Room work must still converge.
pub(crate) fn render_initialization_exclusion(dbnums: &[u32]) -> String {
    if dbnums.is_empty() {
        return String::new();
    }
    format!(
        " AND (dbnum IS NONE OR dbnum NOT IN {dbnums:?} OR action IN ['room_recalc_panel','room_recalc_element'])"
    )
}

fn render_drain_select_for_scope_with_active_cohorts(
    action_filter: &str,
    limit: Option<usize>,
    dbnums: &[u32],
    nouns: &[String],
    active_cohorts: &str,
) -> String {
    let limit = limit
        .map(|value| format!(" LIMIT {value}"))
        .unwrap_or_default();
    let scope_filter = render_automatic_scope_filter(dbnums);
    let type_filter = render_automatic_type_filter(nouns);
    let init_exclusion = render_initialization_exclusion(
        &crate::data_interface::task_registry::TaskRegistry::global().running_data_batch_dbnums(),
    );
    format!(
        "SELECT *, source_end_sesno_time IS NONE AS legacy_source_time, \
         updated_at IS NONE AS legacy_timestamp FROM {TABLE} \
         WHERE (status IN ['pending', 'failed'] OR \
          (status = 'running' AND (lease_until IS NONE OR lease_until < time::now()))) \
         AND init_run_id IS NONE \
         AND (cohort_id IS NONE OR cohort_id NOT IN {active_cohorts}) \
         AND (action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS}) {action_filter}{scope_filter}{type_filter}{init_exclusion} \
         ORDER BY legacy_source_time ASC, source_end_sesno_time DESC, \
         legacy_timestamp ASC, updated_at DESC, id ASC{limit};"
    )
}

fn render_drain_claim_for_scope(
    action_filter: &str,
    limit: Option<usize>,
    dbnums: &[u32],
    nouns: &[String],
    lease_owner: &str,
) -> String {
    let select = render_drain_select_for_scope_with_active_cohorts(
        action_filter,
        limit,
        dbnums,
        nouns,
        "$active_cohorts",
    );
    let select = select.trim_end_matches(';');
    format!(
        "LET $active_cohorts = SELECT VALUE cohort_id FROM {TABLE} \
         WHERE status='running' AND lease_until >= time::now() AND cohort_id IS NOT NONE;\n\
         UPDATE ({select}) SET status='running', lease_owner='{}', \
         lease_until=time::now() + {MODEL_WORK_LEASE} RETURN AFTER;",
        escape_surql_str(lease_owner)
    )
}

fn render_cohort_mate_claim_for_scope(
    action_filter: &str,
    dbnums: &[u32],
    nouns: &[String],
    lease_owner: &str,
    cohort_ids: &[String],
) -> Option<String> {
    if cohort_ids.is_empty() {
        return None;
    }
    let cohorts = cohort_ids
        .iter()
        .map(|cohort| format!("'{}'", escape_surql_str(cohort)))
        .collect::<Vec<_>>()
        .join(", ");
    let scope_filter = render_automatic_scope_filter(dbnums);
    let type_filter = render_automatic_type_filter(nouns);
    let init_exclusion = render_initialization_exclusion(
        &crate::data_interface::task_registry::TaskRegistry::global().running_data_batch_dbnums(),
    );
    Some(format!(
        "UPDATE {TABLE} SET status='running', lease_owner='{}', \
         lease_until=time::now() + {MODEL_WORK_LEASE} \
         WHERE cohort_id IN [{cohorts}] AND init_run_id IS NONE \
         AND (status IN ['pending', 'failed'] OR \
          (status = 'running' AND (lease_until IS NONE OR lease_until < time::now()))) \
         AND (action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS}) {action_filter}{scope_filter}{type_filter}{init_exclusion} RETURN AFTER;",
        escape_surql_str(lease_owner)
    ))
}

async fn claim_page_with_cohorts_on(
    db: &Surreal<Any>,
    action_filter: &str,
    limit: Option<usize>,
    dbnums: &[u32],
    nouns: &[String],
    lease_owner: &str,
) -> anyhow::Result<Vec<PendingModelWork>> {
    let mut response = db
        .query(render_drain_claim_for_scope(
            action_filter,
            limit,
            dbnums,
            nouns,
            lease_owner,
        ))
        .await?
        .check()?;
    let mut jobs: Vec<PendingModelWork> = response.take(1)?;
    let cohort_ids = jobs
        .iter()
        .filter_map(pending_publication_cohort)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(sql) =
        render_cohort_mate_claim_for_scope(action_filter, dbnums, nouns, lease_owner, &cohort_ids)
    {
        let mut response = db.query(sql).await?.check()?;
        let mates: Vec<PendingModelWork> = response.take(0)?;
        let already_claimed = jobs
            .iter()
            .map(|job| (job.action.as_str(), job.target_refno.clone()))
            .collect::<HashSet<_>>();
        jobs.extend(mates.into_iter().filter(|job| {
            !already_claimed.contains(&(job.action.as_str(), job.target_refno.clone()))
        }));
    }
    Ok(jobs)
}

fn drain_lease_owner() -> &'static str {
    static OWNER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OWNER.get_or_init(|| {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    })
}

fn render_drain_select(action_filter: &str, limit: Option<usize>) -> String {
    render_drain_select_for_scope(
        action_filter,
        limit,
        &crate::data_interface::watch_scope::dbnums(),
        &crate::data_interface::model_type_scope::nouns(),
    )
}

fn render_pending_count_query(action_filter: &str, dbnums: &[u32], nouns: &[String]) -> String {
    let scope_filter = render_automatic_scope_filter(dbnums);
    let type_filter = render_automatic_type_filter(nouns);
    format!(
        "SELECT count() AS count FROM {TABLE} \
         WHERE status IN ['pending', 'failed', 'running'] \
         AND (action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS}) {action_filter}{scope_filter}{type_filter} GROUP ALL;"
    )
}

async fn count_pending_work(action_filter: &str) -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(render_pending_count_query(
            action_filter,
            &crate::data_interface::watch_scope::dbnums(),
            &crate::data_interface::model_type_scope::nouns(),
        ))
        .await?
        .check()?;
    let rows: Vec<PendingRowCount> = response.take(0)?;
    Ok(rows.first().map_or(0, |row| row.count as usize))
}

fn model_drain_yield_reason(claimed_epoch: u64) -> Option<(ModelDrainYieldReason, u64)> {
    let coordinator =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    let snapshot = coordinator.snapshot();
    if snapshot.epoch_id != claimed_epoch {
        return Some((ModelDrainYieldReason::EpochChanged, snapshot.epoch_id));
    }
    if !coordinator.model_generation_allowed() {
        return Some((ModelDrainYieldReason::ModelGateClosed, snapshot.epoch_id));
    }
    if data_batch_waiting() {
        return Some((ModelDrainYieldReason::DataQueued, snapshot.epoch_id));
    }
    None
}

fn data_batch_waiting() -> bool {
    crate::data_interface::batch_scheduler::BatchScheduler::global()
        .snapshot()
        .iter()
        .any(|row| matches!(row.state, "queued" | "running"))
}

fn model_drain_detail(
    jobs: &[PendingModelWork],
    epoch_id: u64,
    queue_total: usize,
) -> serde_json::Value {
    serde_json::json!({
        "epoch_id": epoch_id,
        "queue_total": queue_total,
        "page_claimed": jobs.len(),
        "page_completed": 0,
        "remaining": queue_total,
        "stage": "claimed",
        "execution_group_size": crate::options::model_regen_execution_group(),
        "effective_root_inflight": crate::data_interface::model_concurrency::effective_root_inflight(),
        "root_inflight_max": crate::options::model_root_inflight_max(),
        "roots": jobs.iter().take(MODEL_DRAIN_DETAIL_SAMPLE_SIZE).map(|job| serde_json::json!({
            "dbnum": job.dbnum,
            "source_end_sesno": job.source_end_sesno,
            "target_refno": job.target_refno,
            "action": job.action.as_str(),
            "revision": job.revision,
        })).collect::<Vec<_>>(),
        "root_samples_truncated": jobs.len().saturating_sub(MODEL_DRAIN_DETAIL_SAMPLE_SIZE),
    })
}

/// 一页模型消化在任务注册表与 telemetry 里的进度簿记。
///
/// 认领时定死的三样（task_id / queue_total / page_claimed）钉在结构体里；之后的
/// 每个报告点只说会变的三件事——子阶段、已结算数、被根锁挡住的数量。此前这是
/// 一个 7 参自由函数，四个调用点每次都把不变的三样重抄一遍。
struct DrainPageProgress {
    task_id: String,
    queue_total: usize,
    page_claimed: usize,
}

impl DrainPageProgress {
    /// 认领登记：插任务行、置 "claimed" 阶段、telemetry 打底（attempts 增量清零）。
    fn claim(
        mgr: &AiosDBManager,
        jobs: &[PendingModelWork],
        claimed_epoch: u64,
        queue_total: usize,
    ) -> Self {
        let task_id = crate::data_interface::task_registry::TaskRegistry::new_task_id("model");
        let registry = crate::data_interface::task_registry::TaskRegistry::global();
        registry.insert_running_model_drain(
            &task_id,
            &mgr.db_option.project_name,
            jobs.len() as u32,
            model_drain_detail(jobs, claimed_epoch, queue_total),
        );
        registry.set_stage(&task_id, "claimed");
        {
            let mut telemetry = model_drain_telemetry();
            // 认领新一页之前，telemetry 里还留着上一页的终值——正好用来判断上一页是不是
            // 白跑了。一页一次，不多不少（`report` 一页会调好几次，放那里会重复计数）。
            let previous_page_starved = page_was_starved(
                telemetry.last_page_claimed,
                telemetry.last_page_completed,
                telemetry.last_deferred_lock_count,
                telemetry.last_remaining,
                queue_total,
            );
            if previous_page_starved {
                telemetry.starved_pages = telemetry.starved_pages.saturating_add(1);
            } else {
                telemetry.starved_pages = 0;
            }
            telemetry.last_attempts_delta = 0;
            telemetry.last_stage = Some("claimed");
            telemetry.last_page_claimed = jobs.len();
            telemetry.last_page_completed = 0;
            telemetry.last_remaining = queue_total;
            telemetry.last_deferred_lock_count = 0;
            telemetry.last_deferred_lock_sample = None;
        }
        Self {
            task_id,
            queue_total,
            page_claimed: jobs.len(),
        }
    }

    /// 子阶段推进。任务行 detail 缺失（已被淘汰）时整个跳过，与旧行为一致。
    fn report(
        &self,
        stage: &'static str,
        page_completed: usize,
        deferred_lock_count: usize,
        deferred_lock_sample: Option<&str>,
    ) {
        let registry = crate::data_interface::task_registry::TaskRegistry::global();
        let Some(mut detail) = registry.get(&self.task_id).and_then(|entry| entry.detail) else {
            return;
        };
        let Some(object) = detail.as_object_mut() else {
            return;
        };
        object.insert("queue_total".into(), self.queue_total.into());
        object.insert("page_claimed".into(), self.page_claimed.into());
        object.insert("page_completed".into(), page_completed.into());
        object.insert(
            "remaining".into(),
            self.queue_total.saturating_sub(page_completed).into(),
        );
        object.insert("stage".into(), stage.into());
        object.insert("deferred_lock_count".into(), deferred_lock_count.into());
        if let Some(sample) = deferred_lock_sample {
            object.insert("deferred_lock_sample".into(), sample.into());
        }
        registry.set_detail(&self.task_id, detail);
        registry.set_stage(&self.task_id, stage);

        let mut telemetry = model_drain_telemetry();
        telemetry.last_stage = Some(stage);
        telemetry.last_page_claimed = self.page_claimed;
        telemetry.last_page_completed = page_completed;
        telemetry.last_remaining = self.queue_total.saturating_sub(page_completed);
        telemetry.last_deferred_lock_count = deferred_lock_count;
        telemetry.last_deferred_lock_sample = deferred_lock_sample.map(str::to_string);
    }

    fn bump_done(&self) {
        crate::data_interface::task_registry::TaskRegistry::global().bump_units_done(&self.task_id);
    }

    /// 整页全被根锁挡住的收口：任务收 Yielded(RootLockBusy)，行原样保留、attempts 不动。
    fn settle_deferred_for_lock(
        &self,
        report: &DrainReport,
        busy: usize,
        sample: Option<String>,
        claimed_epoch: u64,
    ) -> ModelDrainDisposition {
        let current_epoch =
            crate::data_interface::initialization_phase::InitializationCoordinator::global()
                .snapshot()
                .epoch_id;
        finish_model_drain_task(
            &self.task_id,
            crate::data_interface::task_registry::TaskState::Yielded,
            report,
            busy,
            claimed_epoch,
            current_epoch,
            Some(ModelDrainYieldReason::RootLockBusy),
        );
        ModelDrainDisposition::DeferredForLock {
            done: 0,
            busy,
            sample,
        }
    }

    /// 页面走到头的统一收口：成功 / 带锁让位的成功 / 失败，detail 与任务终态一次报齐。
    fn settle(
        &self,
        report: &DrainReport,
        deferred_lock_count: usize,
        deferred_lock_sample: Option<&str>,
        claimed_epoch: u64,
    ) -> ModelDrainDisposition {
        let current_epoch =
            crate::data_interface::initialization_phase::InitializationCoordinator::global()
                .snapshot()
                .epoch_id;
        if report.failures.is_empty() {
            self.report(
                if deferred_lock_count > 0 {
                    "completed_with_deferred_lock"
                } else {
                    "completed"
                },
                report.done,
                deferred_lock_count,
                deferred_lock_sample,
            );
            finish_model_drain_task(
                &self.task_id,
                if deferred_lock_count > 0 {
                    crate::data_interface::task_registry::TaskState::Yielded
                } else {
                    crate::data_interface::task_registry::TaskState::Succeeded
                },
                report,
                deferred_lock_count,
                claimed_epoch,
                current_epoch,
                (deferred_lock_count > 0).then_some(ModelDrainYieldReason::RootLockBusy),
            );
            ModelDrainDisposition::Completed { done: report.done }
        } else {
            let message = model_drain_failure_message(report);
            self.report(
                "failed",
                report.done,
                deferred_lock_count,
                deferred_lock_sample,
            );
            let state = if report.done == 0 {
                crate::data_interface::task_registry::TaskState::Failed
            } else {
                crate::data_interface::task_registry::TaskState::Partial
            };
            finish_model_drain_task(
                &self.task_id,
                state,
                report,
                0,
                claimed_epoch,
                current_epoch,
                None,
            );
            ModelDrainDisposition::Failed {
                done: report.done,
                message,
            }
        }
    }
}

fn finish_model_drain_task(
    task_id: &str,
    state: crate::data_interface::task_registry::TaskState,
    report: &DrainReport,
    unstarted: usize,
    claimed_epoch: u64,
    current_epoch: u64,
    yield_reason: Option<ModelDrainYieldReason>,
) {
    crate::data_interface::task_registry::TaskRegistry::global().finish(
        task_id,
        state,
        serde_json::json!({
            "completed": report.done,
            "failed": report.failures.len(),
            "unstarted": unstarted,
            "claimed_epoch": claimed_epoch,
            "current_epoch": current_epoch,
            "yield_reason": yield_reason.map(ModelDrainYieldReason::as_str),
            "failures": report.failures,
        }),
    );
}

fn model_drain_failure_message(report: &DrainReport) -> String {
    format!(
        "{} pending model task(s) failed after {} completed: {}",
        report.failures.len(),
        report.done,
        report.failures.join("; ")
    )
}

fn cooperative_yield_task_state(
    report: &DrainReport,
) -> crate::data_interface::task_registry::TaskState {
    if report.failures.is_empty() {
        crate::data_interface::task_registry::TaskState::Yielded
    } else if report.done == 0 {
        crate::data_interface::task_registry::TaskState::Failed
    } else {
        crate::data_interface::task_registry::TaskState::Partial
    }
}

fn finish_cooperative_yield(
    task_id: &str,
    report: &DrainReport,
    unstarted: usize,
    claimed_epoch: u64,
    current_epoch: u64,
    reason: ModelDrainYieldReason,
    data_wait_ms: Option<u64>,
) -> ModelDrainDisposition {
    finish_model_drain_task(
        task_id,
        cooperative_yield_task_state(report),
        report,
        unstarted,
        claimed_epoch,
        current_epoch,
        Some(reason),
    );
    let mut telemetry = model_drain_telemetry();
    telemetry.total_yields += 1;
    telemetry.last_yield_reason = Some(reason.as_str());
    telemetry.last_claimed_epoch = Some(claimed_epoch);
    telemetry.last_current_epoch = Some(current_epoch);
    if reason == ModelDrainYieldReason::DataQueued {
        telemetry.last_data_wait_ms = data_wait_ms;
    }
    debug_assert_eq!(telemetry.attempts_consumed_on_yield, 0);
    drop(telemetry);

    if report.failures.is_empty() {
        ModelDrainDisposition::YieldedForData {
            done: report.done,
            claimed_epoch,
            current_epoch,
            reason,
        }
    } else {
        ModelDrainDisposition::Failed {
            done: report.done,
            message: model_drain_failure_message(report),
        }
    }
}

/// 空闲模型页的可抢占消费。持久行仍按原查询顺序认领；页内能进批的 regen 根一次
/// 生成（[`run_regen_batch`]），其余的逐根执行、根与根之间可让位。初始化门关闭是
/// 调度结果，不写 `last_error`，也不烧掉 attempts。
/// 一页处理完，把本进程认领了却没收口的行放回 `pending`。
///
/// 一条 UPDATE 一个往返在 2026-09-06 rocksdb 7997 实测里是整个模型消化的最大单项：cohort 认领
/// 把 18,675 行一起标成 running，页因 EpochChanged 在 100–270 根后结束，然后逐行释放 18k 次
/// ——每次库侧 0.2 ms、往返 ~10 ms，**每页 3 分钟**（`output/rocksdb-7997-perf-20260906/REPORT.md` §3.1）。
/// 现在按 [`RELEASE_STATEMENTS_PER_ROUND_TRIP`] 条语句拼成一个请求发，逐行谓词（status / revision /
/// lease_owner）一个字不变：别人的租约、更高的 revision 照旧碰不到（`yielded_claims_release_without_touching_new_revisions_or_other_owners`）。
async fn release_unsettled_claims_on(
    db: &Surreal<Any>,
    jobs: &[PendingModelWork],
    owner: &str,
) -> anyhow::Result<()> {
    for batch in render_release_unsettled_claims(jobs, owner) {
        db.query(batch).await?.check()?;
    }
    Ok(())
}

/// 一个请求里放多少条释放语句。每条都是按 record id 的单行 UPDATE，库侧毫秒以下；
/// 500 与 [`QUERY_CHUNK`] 同档，一页 18k 行 ≈ 37 个往返。
const RELEASE_STATEMENTS_PER_ROUND_TRIP: usize = QUERY_CHUNK;

fn render_release_unsettled_claims(jobs: &[PendingModelWork], owner: &str) -> Vec<String> {
    let owner = escape_surql_str(owner);
    jobs.chunks(RELEASE_STATEMENTS_PER_ROUND_TRIP)
        .map(|chunk| {
            chunk
                .iter()
                .map(|job| {
                    format!(
                        "UPDATE {} SET status='pending', lease_owner=NONE, lease_until=NONE \
                         WHERE status='running' AND revision={} AND lease_owner='{owner}';",
                        record_id_of(job.action, &job.target_refno),
                        job.revision
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

async fn drain_where_cooperative(
    mgr: &AiosDBManager,
    action_filter: &str,
    limit: Option<usize>,
) -> anyhow::Result<ModelDrainDisposition> {
    let jobs = claim_page_with_cohorts_on(
        &SUL_DB,
        action_filter,
        limit,
        &crate::data_interface::watch_scope::dbnums(),
        &crate::data_interface::model_type_scope::nouns(),
        drain_lease_owner(),
    )
    .await
    .map_err(|error| anyhow::anyhow!("claim pending model work failed: {error}"))?;
    if jobs.is_empty() {
        // 队列见底，这一轮没有页可认领——停滞计数只在认领时结算，不在这儿清就再也
        // 没机会清了（见 `clear_page_starvation`）。
        clear_page_starvation();
        return Ok(ModelDrainDisposition::Completed { done: 0 });
    }
    let claimed_jobs = jobs.clone();
    let result = drain_claimed_jobs_cooperative(mgr, action_filter, jobs).await;
    // Yield, busy roots and unstarted execution groups must be immediately
    // claimable. Completed/failed or replaced revisions are not touched.
    release_unsettled_claims_on(&SUL_DB, &claimed_jobs, drain_lease_owner()).await?;
    result
}

async fn drain_claimed_jobs_cooperative(
    mgr: &AiosDBManager,
    action_filter: &str,
    jobs: Vec<PendingModelWork>,
) -> anyhow::Result<ModelDrainDisposition> {
    let queue_total = count_pending_work(action_filter).await?;

    let claimed_epoch =
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .epoch_id;
    let progress = DrainPageProgress::claim(mgr, &jobs, claimed_epoch, queue_total);

    let mut report = DrainReport {
        requested: jobs.len(),
        loaded: jobs.len(),
        ..Default::default()
    };

    // 页内合批。[`REGEN_DRAIN_PAGE_SIZE`] 的意义就是摊薄一次生成的启动开销；
    // 这条空闲路径过去逐根调用 `generate_roots`，那笔开销按根重付——页大小的代价
    // 付了，合批本身没接上。
    //
    // 代价是批内不能中途让位：整批要么一起结算、要么一起回退逐根。所以只在批前
    // 查一次 epoch，批后的让位检查由下面的逐根循环接着做。
    let (batchable, sequential): (Vec<PendingModelWork>, Vec<PendingModelWork>) = jobs
        .into_iter()
        .partition(|job| job.action == ModelWorkAction::RegenRoot && joins_regen_batch(job));
    let mut remaining = Vec::with_capacity(sequential.len());
    let mut deferred_lock_count = 0usize;
    let mut deferred_lock_sample = None;
    if batchable.is_empty() {
        remaining.extend(sequential);
    } else {
        if let Some((reason, current_epoch)) = model_drain_yield_reason(claimed_epoch) {
            return Ok(finish_cooperative_yield(
                &progress.task_id,
                &report,
                batchable.len() + sequential.len(),
                claimed_epoch,
                current_epoch,
                reason,
                Some(0),
            ));
        }
        let batched = batchable.len();
        let settled_before = report.done;
        let started = Instant::now();
        progress.report("lock_wait", report.done, 0, None);
        let batch = run_regen_batch(
            mgr,
            batchable,
            &mut report,
            RegenLockPolicy::DeferBusy,
            Some(&progress.task_id),
        )
        .await;
        let batch_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        // 摊薄值：让 `last_root_duration_ms` 在合批前后保持「每根」的量纲。
        model_drain_telemetry().last_root_duration_ms = Some(batch_ms / batched.max(1) as u64);
        for _ in 0..report.done.saturating_sub(settled_before) {
            progress.bump_done();
        }
        remaining.extend(batch.fallback);
        remaining.extend(sequential);
        deferred_lock_count = batch.deferred_lock_count;
        deferred_lock_sample = batch.deferred_locks.first().cloned();
        progress.report(
            if deferred_lock_count > 0 {
                "lock_deferred"
            } else {
                "settling"
            },
            report.done,
            deferred_lock_count,
            deferred_lock_sample.as_deref(),
        );
        if deferred_lock_count > 0 && report.done == 0 && remaining.is_empty() {
            return Ok(progress.settle_deferred_for_lock(
                &report,
                deferred_lock_count,
                deferred_lock_sample,
                claimed_epoch,
            ));
        }
    }

    for (index, job) in remaining.iter().enumerate() {
        if let Some((reason, current_epoch)) = model_drain_yield_reason(claimed_epoch) {
            let unstarted = remaining.len() - index;
            return Ok(finish_cooperative_yield(
                &progress.task_id,
                &report,
                unstarted,
                claimed_epoch,
                current_epoch,
                reason,
                Some(0),
            ));
        }

        let started = Instant::now();
        let outcome = run_one(mgr, job, &mut report).await;
        let root_duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        model_drain_telemetry().last_root_duration_ms = Some(root_duration_ms);
        match outcome {
            RunOneOutcome::Completed | RunOneOutcome::Failed => progress.bump_done(),
            RunOneOutcome::YieldedForData => {
                let current_epoch =
                    crate::data_interface::initialization_phase::InitializationCoordinator::global(
                    )
                    .snapshot()
                    .epoch_id;
                let reason = ModelDrainYieldReason::InitializationNotReady;
                return Ok(finish_cooperative_yield(
                    &progress.task_id,
                    &report,
                    remaining.len() - index,
                    claimed_epoch,
                    current_epoch,
                    reason,
                    None,
                ));
            }
        }

        if let Some((reason, current_epoch)) = model_drain_yield_reason(claimed_epoch) {
            let unstarted = remaining.len() - index - 1;
            return Ok(finish_cooperative_yield(
                &progress.task_id,
                &report,
                unstarted,
                claimed_epoch,
                current_epoch,
                reason,
                Some(root_duration_ms),
            ));
        }
    }

    Ok(progress.settle(
        &report,
        deferred_lock_count,
        deferred_lock_sample.as_deref(),
        claimed_epoch,
    ))
}

fn render_scoped_room_select(keys: &[RoomWorkKey]) -> String {
    let ids = keys
        .iter()
        .map(|key| record_id_of(key.action, &key.target_refno))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT * FROM [{ids}] WHERE record::exists(id) \
         AND status IN ['pending', 'failed'] \
         ORDER BY updated_at ASC;"
    )
}

/// Only never-failed, parseable roots share a batch. `generate_roots` is all
/// or nothing, so re-admitting a root that already failed would fail the
/// whole batch again on every later drain and re-pay the per-root fallback
/// for every healthy neighbour queued alongside it.
pub(crate) fn root_joins_regen_batch(attempts: u32, target_refno: &str) -> bool {
    attempts == 0 && RefU64::from_str(target_refno).is_ok()
}

/// 修复根（带 `required_panels`）同样进合批（ADR-011 2026-08-09 修订）：生成合并
/// 成一次调用，面板后置验收由 [`verify_repair_jobs_page`] 整页做一次、逐根定夺。
fn joins_regen_batch(job: &PendingModelWork) -> bool {
    root_joins_regen_batch(job.attempts, &job.target_refno)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegenLockPolicy {
    /// 显式/人工处理保留“等到完成”的历史语义。
    Wait,
    /// 自动空闲轮不被一个已占用根拖住整页。
    DeferBusy,
}

#[derive(Debug, Default)]
struct RegenBatchOutcome {
    fallback: Vec<PendingModelWork>,
    deferred_lock_count: usize,
    deferred_locks: Vec<String>,
}

async fn acquire_regen_page_locks(
    roots: &[String],
    policy: RegenLockPolicy,
) -> (Vec<tokio::sync::OwnedMutexGuard<()>>, HashSet<String>) {
    let mut guards = Vec::with_capacity(roots.len());
    let mut busy = HashSet::new();
    for root in roots {
        let lock = crate::data_interface::manual_update::generation_root_lock(root);
        match policy {
            RegenLockPolicy::Wait => guards.push(lock.lock_owned().await),
            RegenLockPolicy::DeferBusy => match lock.try_lock_owned() {
                Ok(guard) => guards.push(guard),
                Err(_) => {
                    busy.insert(root.clone());
                }
            },
        }
    }
    (guards, busy)
}

/// Drain pending work independently. Failures remain durable and are retried on
/// a later watcher/manual invocation, even when there is no new session.
async fn drain_where(
    mgr: &AiosDBManager,
    action_filter: &str,
    limit: Option<usize>,
) -> anyhow::Result<usize> {
    drain_where_report(mgr, action_filter, limit)
        .await?
        .into_result()
}

/// [`drain_where`] 的本体。`Err` 只留给「这一轮根本没跑起来」（读表 / 解码失败）；
/// 逐条任务的失败进 [`DrainReport`]，由调用方决定牵连范围。
async fn drain_where_report(
    mgr: &AiosDBManager,
    action_filter: &str,
    limit: Option<usize>,
) -> anyhow::Result<DrainReport> {
    let mut response = SUL_DB
        .query(render_drain_select(action_filter, limit))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("load pending model work statement failed: {error}"))?;
    let jobs: Vec<PendingModelWork> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending model work failed: {error}"))?;

    // The generator accepts the whole root set in a single pass, so running it
    // once per queued root repeats the entire parse → instances → mesh/boolean
    // setup for every root. Fresh regen work therefore goes out as one batch,
    // falling back to per-root runs when that batch fails so the broken target
    // is pinpointed and marked durably; retried or unparsable roots run alone
    // (see [`joins_regen_batch`]).
    let (regen_jobs, other_jobs): (Vec<PendingModelWork>, Vec<PendingModelWork>) = jobs
        .into_iter()
        .partition(|job| job.action == ModelWorkAction::RegenRoot);
    let (batchable, singles): (Vec<PendingModelWork>, Vec<PendingModelWork>) =
        regen_jobs.into_iter().partition(joins_regen_batch);

    let mut report = DrainReport::default();
    let batch = run_regen_batch(mgr, batchable, &mut report, RegenLockPolicy::Wait, None).await;

    for job in batch
        .fallback
        .iter()
        .chain(singles.iter())
        .chain(other_jobs.iter())
    {
        run_one(mgr, job, &mut report).await;
    }

    Ok(report)
}

/// 一页 regen 根合批生成：生成一次、验收一次、收口一次。
///
/// `generate_roots` 收的本来就是根数组，下游 `gen_geos_data` 还按 100 根一块预取
/// PLOO、用 `FuturesUnordered` 做流水重叠。逐根调用会把整条「依赖预载 → cata 闭包
/// → 生成 → prune → mesh」按根重启一遍，块内每条批量查询也都退化成 batch-of-1：
/// 现场量到每根 1.04 秒、每页 16.6 秒，而进程只吃 11.8% 单核——时间几乎全花在
/// 串行往返上，不是几何计算上。
///
/// 返回仍需逐根执行的作业：房间映射预检读失败的修复根，以及合批失败后整页回退的
/// 根。逐根怎么跑由调用方定——[`drain_where_report`] 一口气跑完，
/// [`drain_where_cooperative`] 每根之间还要查一次 epoch。
fn regen_execution_groups(
    batchable: Vec<PendingModelWork>,
    group_size: usize,
) -> Vec<Vec<PendingModelWork>> {
    let mut cohorts = BTreeMap::<String, Vec<PendingModelWork>>::new();
    let mut independent = Vec::new();
    for job in batchable {
        if let Some(cohort) = pending_publication_cohort(&job) {
            cohorts.entry(cohort).or_default().push(job);
        } else {
            independent.push(job);
        }
    }
    let group_size = group_size.max(1);
    let mut groups = Vec::new();
    for mut jobs in cohorts.into_values().chain(std::iter::once(independent)) {
        while !jobs.is_empty() {
            let remainder = jobs.split_off(jobs.len().min(group_size));
            groups.push(std::mem::replace(&mut jobs, remainder));
        }
    }
    groups
}

fn is_atomic_publication_cohort(jobs: &[PendingModelWork]) -> bool {
    jobs.first()
        .and_then(pending_publication_cohort)
        .is_some_and(|cohort| {
            jobs.iter()
                .all(|job| pending_publication_cohort(job).as_deref() == Some(cohort.as_str()))
        })
}

fn should_defer_publication_cohort(jobs: &[PendingModelWork], has_busy_root: bool) -> bool {
    has_busy_root && is_atomic_publication_cohort(jobs)
}

async fn run_regen_batch(
    mgr: &AiosDBManager,
    batchable: Vec<PendingModelWork>,
    report: &mut DrainReport,
    lock_policy: RegenLockPolicy,
    task_id: Option<&str>,
) -> RegenBatchOutcome {
    let group_size = crate::options::model_regen_execution_group();
    let mut outcome = RegenBatchOutcome::default();
    let claimed_epoch =
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .epoch_id;
    for group in regen_execution_groups(batchable, group_size) {
        let group_outcome = run_regen_group(mgr, group, report, lock_policy, task_id).await;
        outcome.fallback.extend(group_outcome.fallback);
        outcome.deferred_lock_count += group_outcome.deferred_lock_count;
        for root in group_outcome.deferred_locks {
            if outcome.deferred_locks.len() < MODEL_DRAIN_DETAIL_SAMPLE_SIZE {
                outcome.deferred_locks.push(root);
            }
        }
        // execution group 是最小不可抢占单元；数据工作、epoch 或模型门变化后不再接纳
        // 下一组。未开始的 durable 行保持原样，attempts 不增加。
        if model_drain_yield_reason(claimed_epoch).is_some() {
            break;
        }
    }
    outcome
}

/// 生成器回报的根与队列行里的 `target_refno` 是同一个身份的两种拼法：队列存 E3D 的
/// `24381/100817`，而报告里那一串经 `RefnoEnum` 走了一趟，`Display` 打出来是
/// `24381_100817`。直接比字符串**一次都不会相等**——2026-08-27 现场整页 100 个根既
/// 进不了收口集合、也配不上失败名单：队列行原样留着、`attempts` 永远是 0、
/// `MAX_ATTEMPTS` 永远够不着、死信永远是空的，1462 个根空转 50 分钟，而每一轮任务
/// 都报 `succeeded`。两边都过这一道再比。
///
/// 解析不出来的原样返回，**不能** `unwrap_or_default()`：那会把所有坏值折成同一个
/// 零值，两个不同的坏根就被认成同一个，比不归一更糟。
pub(crate) fn root_identity_key(refno: &str) -> String {
    RefU64::from_str(refno).map_or_else(|_| refno.to_string(), |parsed| parsed.to_string())
}

/// 本组里认领了数据窗口的那些库，各取它见过的最高窗口右端（ADR-060 D5）。
///
/// 只收 `RegenRoot` 且**同时**认领了库与会话号的行——那正是 [`publication_cohort`] 的
/// 成立条件。房间任务、反向级联派生根、人工强制重试三类如实写 `dbnum = 0` /
/// `source_end_sesno = 0`，它们不属于任何数据窗口，也就不该推动库级的追平承诺。
/// 时刻跟着最高那一条走，号变了就换时刻，永远不会拿 A 的时刻配 B 的号。
fn settled_window_cohorts(
    plain_jobs: &[PendingModelWork],
    repair_jobs: &[PendingModelWork],
) -> Vec<(u32, i32, Option<String>)> {
    let mut highest: BTreeMap<u32, (i32, Option<String>)> = BTreeMap::new();
    for job in plain_jobs.iter().chain(repair_jobs.iter()) {
        if publication_cohort(job.dbnum, job.source_end_sesno).is_none()
            || job.action != ModelWorkAction::RegenRoot
        {
            continue;
        }
        let slot = highest.entry(job.dbnum).or_insert((0, None));
        if job.source_end_sesno > slot.0 {
            *slot = (job.source_end_sesno, job.source_end_sesno_time.clone());
        }
    }
    highest
        .into_iter()
        .map(|(dbnum, (sesno, time))| (dbnum, sesno, time))
        .collect()
}

/// 认领了却没有任何去向的根：生成没报错，可它既不在完成名单里也不在失败名单里，
/// 于是进不了收口集合，队列行原样留到下一轮，再被同样地认领、同样地丢掉。
///
/// 这与 2026-07-30 审计 C2 那条「收口失败不是生成失败，attempts 不涨」是**两回事**。
/// C2 防的是 flaky 的 DELETE：根已经进了收口集合，只是删的那一下抽风，下一轮重试就好，
/// 给它记次数会把一整批健康的根冤枉进死信。这里根本没有 DELETE 可言——根压根没进得了
/// 收口集合，重跑一万次结果都一样。确定性的收不掉必须计次：撞满 [`MAX_ATTEMPTS`] 就
/// 进死信，让 `/health` 的 `blocking_conditions` 把它喊出来，而不是无声烧 CPU。
async fn settle_unechoed_roots(
    plain_jobs: &[PendingModelWork],
    repair_jobs: &[PendingModelWork],
    disposed: &HashSet<String>,
    report: &mut DrainReport,
) {
    let unechoed = plain_jobs
        .iter()
        .chain(repair_jobs.iter())
        .filter(|job| !disposed.contains(&root_identity_key(&job.target_refno)))
        .collect::<Vec<_>>();
    if unechoed.is_empty() {
        return;
    }
    let samples = unechoed
        .iter()
        .take(MODEL_DRAIN_DETAIL_SAMPLE_SIZE)
        .map(|job| format!("{}({})", job.target_refno, job.noun))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "本组 {} 个根生成未报错，但生成器既没回报完成也没回报失败，队列行无法收口；\
         按失败计次，连撞 {MAX_ATTEMPTS} 次后进死信（示例: {samples}）",
        unechoed.len()
    );
    for job in unechoed {
        record_failure(
            job,
            &anyhow::anyhow!(
                "generator echoed neither success nor failure for this root, \
                 so there was nothing to settle"
            ),
            report,
        )
        .await;
    }
}

/// 一个不可抢占的实例生成小组。根锁只在本小组进入执行前获取，不再为整页100根
/// 长时间占锁；小组之间自然形成数据优先级/epoch 的让位边界。
async fn run_regen_group(
    mgr: &AiosDBManager,
    batchable: Vec<PendingModelWork>,
    report: &mut DrainReport,
    lock_policy: RegenLockPolicy,
    task_id: Option<&str>,
) -> RegenBatchOutcome {
    let mut outcome = RegenBatchOutcome::default();
    if !batchable.is_empty() {
        // 自动页不等忙根：一个按需生成正在用的根不能拖住同页其他根。
        // 手动“处理到完成”路径仍等锁，不改它的返回契约。
        let mut lock_roots = batchable
            .iter()
            .map(|job| job.target_refno.clone())
            .collect::<Vec<_>>();
        lock_roots.sort_unstable();
        lock_roots.dedup();
        let (root_guards, busy_roots) = acquire_regen_page_locks(&lock_roots, lock_policy).await;
        let defer_whole_cohort =
            should_defer_publication_cohort(&batchable, !busy_roots.is_empty());
        let (batchable, deferred): (Vec<_>, Vec<_>) = batchable
            .into_iter()
            .partition(|job| !defer_whole_cohort && !busy_roots.contains(&job.target_refno));
        outcome.deferred_lock_count = deferred.len();
        outcome.deferred_locks = deferred
            .into_iter()
            .map(|job| job.target_refno)
            .take(MODEL_DRAIN_DETAIL_SAMPLE_SIZE)
            .collect();
        if batchable.is_empty() {
            return outcome;
        }
        if let Some(task_id) = task_id {
            crate::data_interface::task_registry::TaskRegistry::global()
                .set_stage(task_id, "dependency_preload");
            model_drain_telemetry().last_stage = Some("dependency_preload");
        }

        // 修复根（带 required_panels）也进合批（ADR-011 2026-08-09 修订）：生成一次、
        // 整页验收一次。生成名单先过在册预检——面板全部出册的修复根跳过生成
        // （拓扑合法变化，根可能已删除，硬生成会拖垮整批），但仍随本页验收与收口。
        let (mut repair_jobs, plain_jobs): (Vec<PendingModelWork>, Vec<PendingModelWork>) =
            batchable
                .into_iter()
                .partition(|job| !job.required_panels.is_empty());
        let mut skip_generation: HashSet<String> = HashSet::new();
        let mut repair_rooms = None;
        if !repair_jobs.is_empty() {
            match room_model::load_room_panel_map(&mgr.db_option).await {
                Ok(rooms) => {
                    for job in &repair_jobs {
                        if let Ok(registered) =
                            registered_required_panels(&rooms, &job.required_panels)
                            && registered.is_empty()
                        {
                            skip_generation.insert(job.target_refno.clone());
                        }
                    }
                    repair_rooms = Some(rooms);
                }
                Err(error) => {
                    // 预检读不到房间映射：本页修复根退回单件路径（各付各的加载），
                    // 平根照常合批——一次读失败不该把整页拖成逐件全灭。
                    println!(
                        "修复根预检读取房间映射失败，本页 {} 个修复根退回单件执行: {error:#}",
                        repair_jobs.len()
                    );
                    outcome.fallback.extend(std::mem::take(&mut repair_jobs));
                }
            }
        }

        let mut roots: Vec<String> = Vec::new();
        for job in plain_jobs.iter().chain(repair_jobs.iter()) {
            if skip_generation.contains(&job.target_refno) {
                continue;
            }
            if !roots.contains(&job.target_refno) {
                roots.push(job.target_refno.clone());
            }
        }
        if let Some(task_id) = task_id {
            crate::data_interface::task_registry::TaskRegistry::global()
                .set_stage(task_id, "model_generate");
            model_drain_telemetry().last_stage = Some("model_generate");
        }
        let batch_result =
            crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots_report(
                mgr, &roots,
            )
            .await;
        match batch_result {
            Ok(generation) => {
                if let Some(task_id) = task_id {
                    crate::data_interface::task_registry::TaskRegistry::global()
                        .set_stage(task_id, "settling");
                    model_drain_telemetry().last_stage = Some("settling");
                }
                // 报告侧一律按身份键收，队列侧查之前也过同一道（见 [`root_identity_key`]）。
                // `settlements` 本身仍存队列行自己的写法——它要拿去寻址那一行，不是拿去比。
                let completed = generation
                    .completed
                    .iter()
                    .map(|root| root_identity_key(root))
                    .collect::<HashSet<_>>();
                let mut settlements = plain_jobs
                    .iter()
                    .filter(|job| {
                        skip_generation.contains(&job.target_refno)
                            || completed.contains(&root_identity_key(&job.target_refno))
                    })
                    .map(|job| (job.target_refno.clone(), job.revision))
                    .collect::<Vec<_>>();
                // 本组每个根的去向都必须落进这个集合。留在集合外的根既没收口、也没记
                // 失败、也没被锁挡住——它无声落地，下一轮被原样认领，再无声落地一次。
                // 2026-08-27 现场就是这样：1462 个根空转 50 分钟，零日志、零 attempts、
                // 零死信，而每一轮任务都报 succeeded。
                let mut disposed: HashSet<String> = HashSet::new();
                for failure in generation.failures {
                    let root = root_identity_key(&failure.root);
                    if let Some(job) = plain_jobs
                        .iter()
                        .chain(repair_jobs.iter())
                        .find(|job| root_identity_key(&job.target_refno) == root)
                    {
                        record_failure(job, &anyhow::anyhow!(failure.error), report).await;
                    }
                    disposed.insert(root);
                }
                let completed_repair_jobs = repair_jobs
                    .iter()
                    .filter(|job| {
                        skip_generation.contains(&job.target_refno)
                            || completed.contains(&root_identity_key(&job.target_refno))
                    })
                    .collect::<Vec<_>>();
                if !completed_repair_jobs.is_empty() {
                    let job_refs = completed_repair_jobs;
                    match verify_repair_jobs_page(
                        mgr,
                        repair_rooms
                            .as_ref()
                            .expect("repair jobs retain their page room map"),
                        &job_refs,
                    )
                    .await
                    {
                        Ok(page) => {
                            settlements.extend(page.passed);
                            for (index, message) in page.failed {
                                record_failure(
                                    job_refs[index],
                                    &anyhow::anyhow!("{message}"),
                                    report,
                                )
                                .await;
                                disposed.insert(root_identity_key(&job_refs[index].target_refno));
                            }
                        }
                        Err(error) => {
                            // 验收基础设施失败（读房间映射 / 实例 / 面板索引 / 屏障）：
                            // 这页修复根刚刚全部生成成功，唯一没做完的是验收。与下面
                            // 批量收口失败同一纪律（2026-07-30 审计 C2）：行原样留在
                            // 表里（attempts 不涨），下一轮 drain 重跑幂等生成再验一次。
                            let message = format!(
                                "repair verification failed for {} generated root(s), \
                                 rows stay pending for the next drain: {error:#}",
                                job_refs.len()
                            );
                            for job in &job_refs {
                                report.failed_dbnums.insert(job.dbnum);
                                disposed.insert(root_identity_key(&job.target_refno));
                            }
                            report.failures.push(message);
                        }
                    }
                }
                match clear_regen_work_batch(&settlements).await {
                    Ok(()) => report.done += settlements.len(),
                    Err(error) => {
                        // 收口失败不是生成失败（2026-07-30 审计 C2）：这批根刚刚全部
                        // 生成成功，唯一没做完的是把队列行删掉。给它们逐根 mark_failed
                        // 会各涨一次 attempts——一条 flaky 的 DELETE 连撞 MAX_ATTEMPTS
                        // 次，一整批健康的根就全进死信，而生成明明一次都没失败过。
                        // 行留在表里不动（attempts 仍是 0），下一轮 drain 会重新取到
                        // 它们、重跑一遍幂等生成、再试一次收口；batch_worker 那条同构
                        // 路径（`settlement_failed`）也是这个口径。
                        let message = format!(
                            "batch settlement failed for {} generated root(s), \
                             rows stay pending for the next drain: {error:#}",
                            settlements.len()
                        );
                        for job in plain_jobs.iter().chain(repair_jobs.iter()).filter(|job| {
                            settlements.iter().any(|(root, revision)| {
                                root == &job.target_refno && *revision == job.revision
                            })
                        }) {
                            report.failed_dbnums.insert(job.dbnum);
                        }
                        report.failures.push(message);
                    }
                }
                for (root, _) in &settlements {
                    disposed.insert(root_identity_key(root));
                }
                settle_unechoed_roots(&plain_jobs, &repair_jobs, &disposed, report).await;
                // 兼容 drain 只结算根凭证和旧工作行，不再推进库级完成水位。
                // 发布、空间和房间是否完成只有所属 dbnum Task 知道，水位由其收尾盖章。
            }
            Err(error) => {
                // The per-root fallback acquires the same locks one by one, so
                // 这一页的锁必须先在这里放掉，再把根交回调用方逐根重试。
                drop(root_guards);
                println!(
                    "批量重生成 {} 个根失败，回退逐根重试以定位问题根: {error:#}",
                    roots.len()
                );
                outcome.fallback.extend(plain_jobs);
                outcome.fallback.extend(repair_jobs);
            }
        }
    }

    outcome
}

// 三个阶段的 action 白名单。合起来必须正好覆盖 `ModelWorkAction` 的全部取值：漏掉
// 一种，那种任务入了队就永远不会被消费，而且没有任何报错——它只是静静躺在表里。
// `every_action_is_consumed_by_exactly_one_drain_phase` 守着这条。
const NON_REGEN_ACTION_FILTER: &str =
    "AND action IN ['transform', 'delete_cleanup', 'cascade_expand']";
const REGEN_ACTION_FILTER: &str = "AND action = 'regen_root'";
const POST_REGEN_AABB_ACTION_FILTER: &str = "AND action = 'post_regen_aabb'";
const DATA_ACTION_FILTER: &str = "AND action IN ['transform', 'delete_cleanup', 'cascade_expand', 'regen_root', 'post_regen_aabb']";
const ROOM_ACTION_FILTER: &str = "AND action IN ['room_recalc_panel', 'room_recalc_element']";
const ROOM_PANEL_ACTION_FILTER: &str = "AND action = 'room_recalc_panel'";
const ROOM_ELEMENT_ACTION_FILTER: &str = "AND action = 'room_recalc_element'";
/// 元素侧一轮最多消化多少个。
///
/// 比数据阶段的 [`DRAIN_PAGE_SIZE`] 大：一轮房间的固定开销是两次全量查询（在册房间
/// 映射 + 在册面板几何），页太小的话每页都要重付一遍。
const ROOM_DRAIN_PAGE_SIZE: usize = 256;
/// 面板侧一轮最多消化多少块。
///
/// 比元素页小得多：一个元素任务只是拿本构件的盒子在在册面板上筛一遍，一块面板却要
/// 载入自己的网格、把落在包围盒里的构件逐个做点包含判定，再整间先清后写。这条路上
/// 没有让位检查——一块面板从开始算到收口不可中断——所以页大小就是这一轮的时长上限。
const ROOM_PANEL_PAGE_SIZE: usize = 32;

fn data_phase_is_clear(succeeded: bool, has_more: bool) -> bool {
    succeeded && !has_more
}

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .model_generation_allowed()
    {
        return Ok(0);
    }
    // 四个阶段的先后是硬约束，不是习惯：
    // 1. 非 regen 先跑——`cascade_expand` 会反过来入队 regen 工作；
    // 2. regen 次之——房间归属要读几何与包围盒，在重生成之前算出来的结果本身就是错的；
    // 3. 被改判的原始位姿目标补刷 AABB；
    // 4. 房间最后（ADR-010 §7）。
    let mut done = drain_non_regen(mgr).await?;
    done += drain_where(mgr, REGEN_ACTION_FILTER, None).await?;
    done += drain_where(mgr, POST_REGEN_AABB_ACTION_FILTER, None).await?;
    done += drain_rooms(&mgr.db_option).await?.into_result()?;
    Ok(done)
}

pub async fn drain_non_regen(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .model_generation_allowed()
    {
        return Ok(0);
    }
    drain_where(mgr, NON_REGEN_ACTION_FILTER, None).await
}

/// 与 [`drain_non_regen`] 同一轮工作，但把失败牵涉到的 `dbnum` 一起带出来。
///
/// 批次执行前的那次前置消化用它：非 regen 积压是全局的，只有**本批这个库**的
/// 前置失败才该拦下本批的模型生成。
pub async fn drain_non_regen_report(mgr: &AiosDBManager) -> anyhow::Result<DrainReport> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .model_generation_allowed()
    {
        return Ok(DrainReport::default());
    }
    drain_where_report(mgr, NON_REGEN_ACTION_FILTER, None).await
}

pub async fn drain_post_regen_aabb_report(
    mgr: &AiosDBManager,
    dbnum: u32,
) -> anyhow::Result<DrainReport> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .model_generation_allowed()
    {
        return Ok(DrainReport::default());
    }
    drain_where_report(
        mgr,
        &format!("{POST_REGEN_AABB_ACTION_FILTER} AND dbnum = {dbnum}"),
        None,
    )
    .await
}

/// 前两个阶段（非 regen → regen），不含房间。
///
/// 数据批次 worker 的空闲轮用它消化积压：房间收敛按 ADR-011 §8 在队列跑空时
/// 单独收一轮（包成 `room_recalc` 任务），不跟在积压消化后面顺手带走——那样
/// 房间轮就没有自己的任务行了。
pub async fn drain_data_phases_disposition(
    mgr: &AiosDBManager,
) -> anyhow::Result<ModelDrainDisposition> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .model_generation_allowed()
    {
        return Ok(ModelDrainDisposition::Completed { done: 0 });
    }
    // 每段先用 [`has_pending_work`] 探一下（LIMIT 1、不排序、1ms 量级）再决定要不要
    // 付 drain 那条查询的钱。drain 的 ORDER BY 里有两个当场算出来的 `IS NONE` 派生
    // 列，天生不可索引：`model_update_pending` 上没有索引，取 16 行要全表扫排一遍，
    // 现场量到 260ms。而今天 `transform/delete_cleanup/cascade_expand` 与
    // `post_regen_aabb` 两段的积压长期是 0——不探就是每个空闲轮白付两次全表排序。
    let mut done = 0usize;
    if has_pending_work(NON_REGEN_ACTION_FILTER).await? {
        let non_regen =
            drain_where_cooperative(mgr, NON_REGEN_ACTION_FILTER, Some(DRAIN_PAGE_SIZE)).await?;
        done += non_regen.done();
        match non_regen {
            ModelDrainDisposition::YieldedForData { .. }
            | ModelDrainDisposition::Failed { .. }
            | ModelDrainDisposition::DeferredForLock { .. } => {
                return Ok(non_regen);
            }
            ModelDrainDisposition::Completed { .. } => {}
        }
        // 这一页消化完还有剩：regen 的前置屏障保持关闭，下一轮接着收。
        if has_pending_work(NON_REGEN_ACTION_FILTER).await? {
            return Ok(ModelDrainDisposition::Completed { done });
        }
    }

    if has_claimable_work(REGEN_ACTION_FILTER).await? {
        let regen = drain_where_cooperative(
            mgr,
            REGEN_ACTION_FILTER,
            Some(crate::options::model_regen_claim_page()),
        )
        .await?;
        done += regen.done();
        match regen {
            ModelDrainDisposition::YieldedForData {
                claimed_epoch,
                current_epoch,
                reason,
                ..
            } => {
                return Ok(ModelDrainDisposition::YieldedForData {
                    done,
                    claimed_epoch,
                    current_epoch,
                    reason,
                });
            }
            ModelDrainDisposition::Failed { message, .. } => {
                return Ok(ModelDrainDisposition::Failed { done, message });
            }
            ModelDrainDisposition::DeferredForLock { busy, sample, .. } => {
                return Ok(ModelDrainDisposition::DeferredForLock { done, busy, sample });
            }
            ModelDrainDisposition::Completed { .. } => {}
        }
    }
    // Include locally owned initialization tasks in the AABB barrier, while
    // excluding them from ordinary claims above.
    if has_pending_work(REGEN_ACTION_FILTER).await? {
        return Ok(ModelDrainDisposition::Completed { done });
    }

    if !crate::options::spatial_compute() {
        return Ok(ModelDrainDisposition::Completed { done });
    }

    if !has_pending_work(POST_REGEN_AABB_ACTION_FILTER).await? {
        return Ok(ModelDrainDisposition::Completed { done });
    }
    let aabb = drain_where_cooperative(
        mgr,
        POST_REGEN_AABB_ACTION_FILTER,
        Some(crate::options::model_post_regen_aabb_page()),
    )
    .await?;
    done += aabb.done();
    Ok(match aabb {
        ModelDrainDisposition::Completed { .. } => ModelDrainDisposition::Completed { done },
        ModelDrainDisposition::YieldedForData {
            claimed_epoch,
            current_epoch,
            reason,
            ..
        } => ModelDrainDisposition::YieldedForData {
            done,
            claimed_epoch,
            current_epoch,
            reason,
        },
        ModelDrainDisposition::Failed { message, .. } => {
            ModelDrainDisposition::Failed { done, message }
        }
        ModelDrainDisposition::DeferredForLock { busy, sample, .. } => {
            ModelDrainDisposition::DeferredForLock { done, busy, sample }
        }
    })
}

/// Compatibility wrapper for Python/debug callers that only need the completed count.
pub async fn drain_data_phases(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    match drain_data_phases_disposition(mgr).await? {
        ModelDrainDisposition::Completed { done }
        | ModelDrainDisposition::YieldedForData { done, .. }
        | ModelDrainDisposition::DeferredForLock { done, .. } => Ok(done),
        ModelDrainDisposition::Failed { message, .. } => anyhow::bail!(message),
    }
}

fn render_has_pending_work_query(
    action_filter: &str,
    attempt_operator: &str,
    dbnums: &[u32],
    nouns: &[String],
) -> String {
    let scope_filter = render_automatic_scope_filter(dbnums);
    let type_filter = render_automatic_type_filter(nouns);
    let attempt_filter = if attempt_operator == "<" {
        format!(
            "(action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS})"
        )
    } else {
        format!(
            "(action NOT IN ['room_recalc_panel', 'room_recalc_element'] AND (attempts?:0) >= {MAX_ATTEMPTS})"
        )
    };
    let statuses = if attempt_operator == "<" {
        "['pending', 'failed', 'running']"
    } else {
        "['pending', 'failed']"
    };
    format!(
        "RETURN array::len((SELECT VALUE id FROM {TABLE} \
         WHERE status IN {statuses} AND {attempt_filter} \
         {action_filter}{scope_filter}{type_filter} LIMIT 1)) > 0;"
    )
}

async fn has_pending_work(action_filter: &str) -> anyhow::Result<bool> {
    let mut response = SUL_DB
        .query(render_has_pending_work_query(
            action_filter,
            "<",
            &crate::data_interface::watch_scope::dbnums(),
            &crate::data_interface::model_type_scope::nouns(),
        ))
        .await?
        .check()?;
    Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
}

async fn has_claimable_work(action_filter: &str) -> anyhow::Result<bool> {
    has_pending_work(&format!(
        "{action_filter} AND init_run_id IS NONE{}",
        render_initialization_exclusion(
            &crate::data_interface::task_registry::TaskRegistry::global()
                .running_data_batch_dbnums()
        )
    ))
    .await
}

/// Scheduling probe only; completion barriers must use has_pending_data_work.
pub async fn has_claimable_data_work() -> anyhow::Result<bool> {
    has_claimable_work(DATA_ACTION_FILTER).await
}

pub async fn has_pending_data_work() -> anyhow::Result<bool> {
    has_pending_work(DATA_ACTION_FILTER).await
}

#[derive(Debug, Clone, Deserialize)]
struct ModelPendingActionCount {
    action: String,
    c: usize,
}

fn phase_for_action(action: &str) -> bool {
    ModelWorkAction::parse(action).is_some_and(ModelWorkAction::is_room_recalc)
}

fn add_model_pending_counts(
    status: &mut ModelPendingStatus,
    rows: impl IntoIterator<Item = ModelPendingActionCount>,
    dead: bool,
) {
    for row in rows {
        let action = status.by_action.entry(row.action.clone()).or_default();
        let phase = if phase_for_action(&row.action) {
            &mut status.room_phase
        } else {
            // Unknown actions are deliberately conservative: an unrecognised row
            // must keep the model gate closed rather than disappear from health.
            &mut status.data_phase
        };
        if dead {
            status.dead_letters += row.c;
            phase.dead_letters += row.c;
            action.dead_letters += row.c;
        } else {
            status.retryable += row.c;
            phase.retryable += row.c;
            action.retryable += row.c;
        }
    }
}

fn build_model_pending_status(
    retryable: Vec<ModelPendingActionCount>,
    dead: Vec<ModelPendingActionCount>,
    mut blocking_samples: Vec<ModelPendingBlockingSample>,
) -> ModelPendingStatus {
    let mut status = ModelPendingStatus::default();
    add_model_pending_counts(&mut status, retryable, false);
    add_model_pending_counts(&mut status, dead, true);
    blocking_samples.truncate(10);
    status.blocking_samples = blocking_samples;
    status
}

/// Read all model/room pending counts and a bounded terminal sample in one
/// database round trip. This is shared by the idle worker and `/health`, so the
/// console and API cannot disagree about whether the model gate is blocked.
/// 类型限定生效时多出第四条语句：被暂扣的行数。名单为空时**不渲染**这条——
/// 三条语句的形状与引入前逐字相同，解码侧按名单是否为空决定要不要取第四格。
fn render_model_pending_status_query_for_scope(dbnums: &[u32], nouns: &[String]) -> String {
    let scope_filter = render_automatic_scope_filter(dbnums);
    let type_filter = render_automatic_type_filter(nouns);
    let held = render_automatic_type_holdback(nouns)
        .map(|holdback| {
            format!(
                "SELECT count() AS count FROM {TABLE} \
                 WHERE status IN ['pending', 'failed', 'running'] AND (action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS})\
                 {holdback}{scope_filter} GROUP ALL;"
            )
        })
        .unwrap_or_default();
    format!(
        "SELECT action, count() AS c FROM {TABLE} \
             WHERE status IN ['pending', 'failed', 'running'] AND (action IN ['room_recalc_panel', 'room_recalc_element'] OR (attempts?:0) < {MAX_ATTEMPTS}){scope_filter}{type_filter} \
             GROUP BY action;\
             SELECT action, count() AS c FROM {TABLE} \
             WHERE status IN ['pending', 'failed'] AND action NOT IN ['room_recalc_panel', 'room_recalc_element'] AND (attempts?:0) >= {MAX_ATTEMPTS}{scope_filter}{type_filter} \
             GROUP BY action;\
             SELECT id, action, target_refno, noun, attempts, last_error, revision, updated_at \
             FROM {TABLE} WHERE status IN ['pending', 'failed'] \
             AND action NOT IN ['room_recalc_panel', 'room_recalc_element'] AND (attempts?:0) >= {MAX_ATTEMPTS}{scope_filter}{type_filter} \
             ORDER BY updated_at DESC, id ASC LIMIT 10;{held}"
    )
}

fn render_model_pending_status_query(nouns: &[String]) -> String {
    render_model_pending_status_query_for_scope(
        &crate::data_interface::watch_scope::dbnums(),
        nouns,
    )
}

pub async fn model_pending_status() -> anyhow::Result<ModelPendingStatus> {
    let nouns = crate::data_interface::model_type_scope::nouns();
    let mut response = SUL_DB
        .query(render_model_pending_status_query(&nouns))
        .await
        .map_err(|error| anyhow::anyhow!("read model pending status failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("read model pending status statement failed: {error}"))?;
    let retryable = response
        .take::<Vec<ModelPendingActionCount>>(0)
        .map_err(|error| {
            anyhow::anyhow!("decode retryable model pending counts failed: {error}")
        })?;
    let dead = response
        .take::<Vec<ModelPendingActionCount>>(1)
        .map_err(|error| anyhow::anyhow!("decode model dead-letter counts failed: {error}"))?;
    let samples = response
        .take::<Vec<ModelPendingBlockingSample>>(2)
        .map_err(|error| anyhow::anyhow!("decode model dead-letter samples failed: {error}"))?;
    let held = if nouns.is_empty() {
        0
    } else {
        response
            .take::<Vec<PendingRowCount>>(3)
            .map_err(|error| {
                anyhow::anyhow!("decode model rows held by model_incremental_types failed: {error}")
            })?
            .first()
            .map_or(0, |row| row.count as usize)
    };
    let mut status = build_model_pending_status(retryable, dead, samples);
    status.held_by_model_incremental_types = held;
    Ok(status)
}

/// 待重算房间目标的分项计数（ADR-011 §10：随 `room_recalc` 任务详情带出）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct RoomTargetCounts {
    /// 还活着的整间任务数（PANE / 房间节点）。
    pub panels: usize,
    /// 还活着的元素任务数。
    pub elements: usize,
    /// 已达重试上限的死信数——自动路径不会再碰它们，只有界面能把它们暴露出来。
    pub dead_letters: usize,
    /// 项目级覆盖屏障暂缓的房间目标数。
    ///
    /// 屏障已经撤掉（缺几何的面板改为按块让开替换范围，不再冻结全库），所以这个数
    /// 恒为 0。字段保留是因为它进了 `room_recalc` 任务详情的 JSON，删掉会让既有看板
    /// 少一个键；留着并说明白，比让消费者去猜一个消失的字段强。
    pub blocked: usize,
}

impl RoomTargetCounts {
    /// 本轮 drain 会处理的目标总数（死信不算）。
    pub fn live(&self) -> usize {
        self.panels + self.elements
    }
}

/// 这个计数是空闲轮「要不要收一轮房间」的唯一依据，所以它必须跟
/// [`load_room_jobs`] 圈同一批行——两侧口径一旦分家，就是一个自己喂自己的空转环：
/// 计数器说还有活、加载器一行都取不出来、本轮完成 0，下一轮再来一遍。
fn render_room_target_counts_query(dbnums: &[u32]) -> String {
    let scope_filter = render_automatic_scope_filter(dbnums);
    format!(
        "SELECT action, count() AS c FROM {TABLE} WHERE status IN ['pending', 'failed'] \
         {ROOM_ACTION_FILTER}{scope_filter} GROUP BY action; RETURN [];"
    )
}

/// 统计待重算房间目标，供空闲轮决定要不要收房间并给 `room_recalc` 任务当
/// total 与详情。
pub async fn count_room_targets() -> anyhow::Result<RoomTargetCounts> {
    #[derive(serde::Deserialize)]
    struct ActionRow {
        action: String,
        c: usize,
    }
    #[derive(serde::Deserialize)]
    struct CountRow {
        c: usize,
    }
    let mut response = SUL_DB
        .query(render_room_target_counts_query(
            &crate::data_interface::watch_scope::dbnums(),
        ))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("count pending room work statement failed: {error}"))?;
    let live: Vec<ActionRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending room count failed: {error}"))?;
    let dead: Vec<CountRow> = response
        .take(1)
        .map_err(|error| anyhow::anyhow!("decode dead room count failed: {error}"))?;

    let mut counts = RoomTargetCounts {
        dead_letters: dead.first().map(|r| r.c).unwrap_or(0),
        ..Default::default()
    };
    for row in live {
        match row.action.as_str() {
            "room_recalc_panel" => counts.panels = row.c,
            "room_recalc_element" => counts.elements = row.c,
            other => anyhow::bail!("房间目标计数遇到未知 action: {other}"),
        }
    }
    Ok(counts)
}

/// Global idle-room drain. It keeps the historical backlog behavior while the
/// post-commit worker uses [`drain_rooms_scoped`] to address only its own rows.
pub async fn drain_rooms(db_option: &aios_core::options::DbOption) -> anyhow::Result<DrainReport> {
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .postprocess_allowed()
    {
        return Ok(DrainReport::default());
    }
    let mut recovery_failures = Vec::new();
    match crate::data_interface::side_effect_pending::SideEffectCompensator::
        published_file_rounds_awaiting_rooms()
    .await
    {
        Ok(rounds) => {
            for round in rounds {
                let recovered = async {
                    crate::fast_model::room_publication::clear_deferred_room_edges(
                        &round.room_clear_refnos,
                    )
                    .await?;
                    let scope = promote_room_file_round(round.dbnum, round.end_sesno).await?;
                    let report = drain_rooms_scoped(db_option, &scope).await?;
                    anyhow::ensure!(
                        report.failures.is_empty(),
                        "房间恢复仍有 {} 个目标失败",
                        report.failures.len()
                    );
                    crate::data_interface::side_effect_pending::SideEffectCompensator::
                        mark_file_rooms_released(round.dbnum, round.end_sesno)
                    .await?;
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if let Err(error) = recovered {
                    recovery_failures.push(format!(
                        "恢复文件房间发布 dbnum={} sesno={} 失败: {error:#}",
                        round.dbnum, round.end_sesno
                    ));
                }
            }
        }
        Err(error) => recovery_failures.push(format!("读取待恢复文件房间发布失败: {error:#}")),
    }
    let mut report = drain_rooms_selected(db_option, None).await?;
    report.failures.extend(recovery_failures);
    Ok(report)
}

/// Drain exactly the durable room rows published by one committed increment.
/// An empty scope is deliberately connection-free, so batches without room
/// changes never load the room map, panel geometry, or the pending table.
pub(crate) async fn drain_rooms_scoped(
    db_option: &aios_core::options::DbOption,
    scope: &RoomDrainScope,
) -> anyhow::Result<DrainReport> {
    if scope.is_empty() {
        return Ok(DrainReport::default());
    }
    drain_rooms_selected(db_option, Some(scope)).await
}

/// Release the room rows collected by a completed file round. Rows belonging
/// to incomplete files remain `collecting` and are invisible to both scoped and
/// global drains.
pub(crate) async fn promote_room_file_round(
    dbnum: u32,
    end_sesno: i32,
) -> anyhow::Result<RoomDrainScope> {
    let round = escape_surql_str(&format!("{dbnum}:{end_sesno}"));
    let published_version = crate::data_interface::side_effect_pending::SideEffectCompensator::
        published_spatial_version()
    .await?;
    SUL_DB
        .query(format!(
            "UPDATE {TABLE} SET status = 'pending', \
             required_publish_version = math::max([required_publish_version?:0, {published_version}]), \
             updated_at = time::now() \
             WHERE status = 'collecting' AND file_rounds CONTAINS '{round}';"
        ))
        .await?
        .check()?;
    RoomDrainScope::for_file_round(dbnum, end_sesno).await
}

async fn drain_rooms_selected(
    db_option: &aios_core::options::DbOption,
    scope: Option<&RoomDrainScope>,
) -> anyhow::Result<DrainReport> {
    // 状态机门禁（一致性闭环方案 §6）：房间重算的整间分支候选取自
    // GLOBAL_AABB_TREE，树不在可消费状态就消费任务 = 拿不可信的树改写归属。
    // 被拒时错误带 SPATIAL_TREE_NOT_READY 码，durable 行原样保留，
    // 状态收敛后由空闲轮/scoped 重试收走。
    crate::fast_model::spatial_state::ensure_spatial_ready()?;
    // Upgrade/startup may have durable room work before the next data window.
    // Establish the immutable geometry baseline here as well; if an old
    // collecting round makes the live rows ambiguous, the baseline helper
    // fails closed and leaves every task untouched.
    //
    // 读透形态没有数据批次，也就永远不会有文件轮次来更新这份基线：建了也只是把全部
    // 设计库的实时行冻成一份再也不动的 `room_published_inst`，房间任务随后拿它算新几何。
    // 所以读透不建基线，几何视图整轮退回实时行（`room_model::consumption_view`，审核 F-B）。
    let read_through = crate::options::data_face().is_read_through();
    if !read_through {
        crate::data_interface::side_effect_pending::SideEffectCompensator::
            ensure_published_file_baseline(0, 0)
        .await?;
    }
    // Pin the spatial tree, published-file manifest and model rows as one
    // consumer view. Spatial publication uses this same serial lock, so a room
    // calculation cannot straddle two published versions; a newly enqueued
    // revision remains durable and is handled after this guard is released.
    let _published_view_guard = crate::fast_model::spatial_state::lock_spatial_serial().await;
    crate::fast_model::spatial_state::ensure_spatial_ready()?;
    let published_version =
        crate::data_interface::side_effect_pending::SideEffectCompensator::
            published_spatial_version()
        .await?;
    // 换代点（常驻房间模型 P2）：把这一路的模型对齐到此刻那一组文件版本——**串行锁之内、
    // 读任务之前**。身份键没变是常路（一次探针查询，直接复用上一轮那份 `Arc`）；键变了才
    // 整份重建。换代、读任务、写归属边落在同一段串行区里，算到一半模型被换掉就是拿旧拓扑
    // 写新边，而且两边都不报错。开关关着时是 `None`，下面每一处都走今天的读法。
    let round = ensure_resident_rooms(db_option).await?;
    let resident = round.as_ref().map(|round| round.model.clone());
    // Panels are always complete-before-elements. The scoped path selects exact
    // record ids; element records themselves are not accumulated across pages.
    //
    // 全局路径的面板侧同样分页（[`ROOM_PANEL_PAGE_SIZE`]）。此前这里传 `None`：表里
    // 有多少块面板任务就一轮全拉进来逐块整间重算，中途不分页、不查 epoch、不让位。
    // 而面板任务是跟着 regen 一起产的——现场 regen 每消化 55 个根就多 4 块面板，
    // 积压跑完那一刻第一轮房间要一口气吃掉上千块。页大小在这条路上就是单轮时长上限。
    let panels = match scope {
        Some(scope) => {
            load_scoped_room_jobs(&scope.keys_for(ModelWorkAction::RoomRecalcPanel)).await?
        }
        None => load_room_jobs(ROOM_PANEL_ACTION_FILTER, Some(ROOM_PANEL_PAGE_SIZE)).await?,
    };
    // 满页 = 后面还有面板。元素侧本轮整页让开，保持「面板先于元素」这条既有次序：
    // 同轮吸收（ADR-010 §8）只对本轮已重算的面板成立，面板还没收完就跑元素，等于
    // 让本该被吸收的元素任务各自先清后写一遍，下一轮那块面板再整间重写一遍。两条
    // 分支共用判定与边 id，收敛结果一样，但白跑一遍。
    let panel_page_full = scope.is_none() && panels.len() >= ROOM_PANEL_PAGE_SIZE;
    // 护栏（主计划 §3 P4）：换代那一轮拓扑真的变了（diff 非空），而这一轮的面板目标里一块
    // 都没落在 diff 上 → 账本触发（F-A，`room_structural_trigger.rs`）漏了这一形状。常驻模型的
    // diff **不产目标**（原则 3），它只是唯一能出声的第二道。放在 `loaded == 0` 那道早退
    // **之前**：一个目标都没有恰恰是最该出声的形状。
    if let Some(round) = round.as_ref()
        && let Some(diff) = round.diff.as_ref()
    {
        report_generation_swap(&round.model, diff, &panels, panel_page_full);
    }
    let global_elements = if scope.is_none() && !panel_page_full {
        load_room_jobs(ROOM_ELEMENT_ACTION_FILTER, Some(ROOM_DRAIN_PAGE_SIZE)).await?
    } else {
        Vec::new()
    };
    let loaded = panels.len() + global_elements.len();
    let requested = scope.map_or(loaded, RoomDrainScope::len);
    let scoped_element_keys = scope
        .map(|scope| scope.keys_for(ModelWorkAction::RoomRecalcElement))
        .unwrap_or_default();
    if loaded == 0 && scoped_element_keys.is_empty() {
        return Ok(DrainReport {
            requested,
            loaded,
            ..Default::default()
        });
    }

    // 拓扑从常驻模型取——它就是「过了关键字与命名校验的在册视图」，与
    // `load_room_panel_map` 的产物同形（`room_resident::registered_map`）。每轮各读一次
    // 拓扑在 ams7997 上是 6–7 s / 文件；常驻之后只有换代那一轮付。
    let rooms = match resident.as_ref() {
        Some(model) => model.map.clone(),
        None => room_model::load_room_panel_map(db_option).await?,
    };
    // Panel geometry is loaded once for the complete scoped drain and reused by
    // every 256-element history page. The view (published rows vs live rows)
    // follows the data face, the same choice the panel branch makes.
    // 面板几何同源：`resident` 就是刚才那次换代交回来的那份，索引把它收在身上，整间分支
    // 从 `panel_index.resident()` 取同一份（`run_room_task`）。开关关着是 `None`，两条分支
    // 一起回到今天的库内查询。三角网从此经 `MeshCache`（键 = geo_hash + 世界矩阵位模式）
    // 跨代复用，不再每轮按面板各读一次 `.mesh`。
    let panel_index =
        room_model::load_consumption_panel_index(db_option, &rooms, resident.clone()).await?;
    let mut report = DrainReport {
        requested,
        loaded,
        ..Default::default()
    };
    // 判不了的面板只记账、不阻断：替换范围的排除交给元素分支按面板处理
    // （`room_model::render_element_relate_write` 的 `in NOT IN`）。此前这里是整轮
    // 早退，于是一块缺几何的面板就能让全库房间重算无限期停摆。
    let missing = panel_index.missing_panels();
    if missing.is_empty() {
        if let Err(error) = clear_room_panel_defects().await {
            println!("[房间缺陷] 面板几何已完整但缺陷登记没销掉: {error:#}");
        }
    } else {
        record_room_panel_defects(rooms.rooms.len(), missing).await;
    }

    let empty_history = room_model::ElementRoomHistory::default();
    let mut claimed_members: HashSet<RefnoEnum> = HashSet::new();
    let mut claimed_panels: HashSet<RefnoEnum> = HashSet::new();
    for job in &panels {
        match run_room_job_isolated(
            db_option,
            &rooms,
            &panel_index,
            &empty_history,
            job,
            published_version,
        )
        .await
        {
            Ok(members) => {
                claimed_members.extend(members);
                if let Ok(refno) = RefU64::from_str(&job.target_refno) {
                    claimed_panels.insert(RefnoEnum::from(refno));
                }
                report.done += 1;
            }
            Err(error) => {
                let _ = record_failure(job, &error, &mut report).await;
            }
        }
        // The global idle drain is lower priority than a newly discovered data
        // window.  A room page can take minutes on a large backlog, so yielding
        // only at the page boundary leaves an eligible batch stranded while the
        // sole dispatcher is busy here.  Scoped post-commit drains must finish
        // their own file round and therefore do not take this exit.
        if scope.is_none() && data_batch_waiting() {
            return Ok(report);
        }
    }

    if let Some(scope) = scope {
        for page in scope.pages_for(ModelWorkAction::RoomRecalcElement, ROOM_DRAIN_PAGE_SIZE) {
            let elements = load_scoped_room_jobs(&page).await?;
            report.loaded += elements.len();
            drain_room_element_page(
                db_option,
                &rooms,
                &panel_index,
                &claimed_members,
                &claimed_panels,
                &elements,
                published_version,
                false,
                &mut report,
            )
            .await;
        }
    } else if !global_elements.is_empty() {
        drain_room_element_page(
            db_option,
            &rooms,
            &panel_index,
            &claimed_members,
            &claimed_panels,
            &global_elements,
            published_version,
            true,
            &mut report,
        )
        .await;
    }
    // 写点 ③（S4）：仍在串行锁之内，参照说的就是刚才那份清单。
    if let Some(model) = resident.as_ref() {
        settle_resident_reference(model, &report).await;
    }
    Ok(report)
}

/// 一次换代交回来的东西：这一轮归属要用的那份模型，外加**真换了代**那一次与上一代的拓扑差。
///
/// `diff` 是 P4 护栏唯一的输入（`EnsureOutcome.diff`）。身份键没变的常路是 `None`——那时
/// 拓扑压根没动，没有护栏可言；第一代（进程里第一次建模）也是 `None`，没有上一代可比。
struct ResidentRound {
    model: std::sync::Arc<crate::fast_model::room_resident::ResidentRoomModel>,
    diff: Option<crate::fast_model::room_resident::TopologyDiff>,
}

/// 开关开着时把常驻房间模型对齐到此刻的文件版本，交回这一轮归属要用的那份。
///
/// **调用方必须已持 `lock_spatial_serial`**（源码钉
/// `the_resident_generation_swap_happens_inside_the_spatial_serial_lock`）。
///
/// `None` 有三种，都回到今天的读法（每轮各查一次拓扑与几何）：开关关着、这个构建没有
/// 房间层级、摄入侧还没有发布清单（升级基线未建）。建不起来才是 `Err`——那时按原则 2
/// 止步，不拿半份拓扑改写存量边。
///
/// `EnsureOutcome.diff` 此前在这里被直接丢掉；P4 之后它随 [`ResidentRound`] 一起交出去，
/// 护栏拿它对本轮的面板目标。
async fn ensure_resident_rooms(
    db_option: &aios_core::options::DbOption,
) -> anyhow::Result<Option<ResidentRound>> {
    if !crate::options::room_resident_model() {
        return Ok(None);
    }
    let Some(hierarchy) = crate::fast_model::room_topology::RoomHierarchy::configured() else {
        return Ok(None);
    };
    #[cfg(any(feature = "project_hd", feature = "project_hh"))]
    let match_room_fn = room_model::configured_match_room_fn();
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let match_room_fn = |_: &str| false;
    let room_key_words = db_option.get_room_key_word();
    let mesh_dir = db_option.get_meshes_path();
    let outcome = crate::fast_model::room_resident::ensure_current(
        crate::options::data_face(),
        hierarchy,
        &room_key_words,
        match_room_fn,
        &mesh_dir,
    )
    .await?;
    Ok(outcome.map(|outcome| ResidentRound {
        model: outcome.model,
        diff: outcome.diff,
    }))
}

/// 护栏与「参照被启动对比的尾巴挡住」两句话的出声间隔。与
/// `batch_worker::MODEL_DEAD_LETTER_REPEAT_SECS` 同一个数、同一条纪律：空闲轮每 30 秒来
/// 一趟，每趟复述同一句就是把日志刷成噪音；可完全不说又正是这类问题最难查的地方。
const ROOM_ANNOUNCE_REPEAT_SECS: i64 = 300;

/// 限流判据（纯函数）：理由变了、或者距上次出声已经到期。
fn announcement_is_due(
    last: &(Option<String>, i64),
    fingerprint: &str,
    now: i64,
    repeat_secs: i64,
) -> bool {
    last.0.as_deref() != Some(fingerprint) || now.saturating_sub(last.1) >= repeat_secs
}

/// 取一次出声名额：够格就把 `(理由, 时刻)` 写回槽位并回 `true`。
fn claim_announcement(slot: &Mutex<(Option<String>, i64)>, fingerprint: &str) -> bool {
    let now = chrono::Local::now().timestamp();
    let mut last = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !announcement_is_due(&last, fingerprint, now, ROOM_ANNOUNCE_REPEAT_SECS) {
        return false;
    }
    *last = (Some(fingerprint.to_string()), now);
    true
}

static RESIDENT_GUARD_ANNOUNCEMENT: Mutex<(Option<String>, i64)> = Mutex::new((None, 0));
static REFERENCE_TAIL_ANNOUNCEMENT: Mutex<(Option<String>, i64)> = Mutex::new((None, 0));

/// P4 护栏的判据（纯函数）：这一轮的面板目标碰没碰到 diff。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SwapObservation {
    /// diff 碰到的面板数（[`TopologyDiff::touched_panels`]）。
    touched: usize,
    /// 本轮面板目标里落在 diff 上的块数。
    matched: usize,
    /// 护栏响不响。
    fired: bool,
    /// 响了是那一句；判不了时是「为什么判不了」；一切正常是 `None`。
    note: Option<String>,
}

/// 拓扑变了而这一轮一块面板目标都没碰到它 → 账本触发漏了（主计划 §3 P4，Q8 (a)）。
///
/// 两种**不出声**的形状要分清，别把它们折进「护栏没响」一句里：
///
/// * `diff` 为空——换了代但拓扑没动（某个文件的版本前进了、房间与面板没变）。本来就无事。
/// * 面板页满了（[`ROOM_PANEL_PAGE_SIZE`]）——后面还有页，本轮没命中说明不了任何事。
///   判不了就不判（判据不完整时出声就是误报），但把原因写进 `/health` 那一格。
fn observe_generation_swap(
    diff: &crate::fast_model::room_resident::TopologyDiff,
    round_targets: &BTreeSet<RefnoEnum>,
    panel_page_full: bool,
) -> SwapObservation {
    if diff.is_empty() {
        return SwapObservation {
            touched: 0,
            matched: 0,
            fired: false,
            note: None,
        };
    }
    let touched = diff.touched_panels();
    let matched = round_targets
        .iter()
        .filter(|panel| touched.contains(panel))
        .count();
    if matched > 0 {
        return SwapObservation {
            touched: touched.len(),
            matched,
            fired: false,
            note: None,
        };
    }
    if panel_page_full {
        return SwapObservation {
            touched: touched.len(),
            matched: 0,
            fired: false,
            note: Some(format!(
                "本轮面板页已满（{ROOM_PANEL_PAGE_SIZE} 块，后面还有页），命中判不了，本次不出声"
            )),
        };
    }
    let sample: Vec<String> = touched.iter().take(5).map(RefnoEnum::to_pdms_str).collect();
    SwapObservation {
        touched: touched.len(),
        matched: 0,
        fired: true,
        note: Some(format!(
            "常驻拓扑换代后 {}，而本轮 {} 个面板目标里一块都不在这 {} 块受影响的面板上\
             （例：{}{}）——账本结构触发（F-A，room_structural_trigger.rs）可能漏了这一形状。\
             本笔只出声、不标脏：标脏会把下次启动拖进全量重建，而护栏本身还没在真库上验过。\
             下次启动的对比会把这些面板算进 diff",
            diff.summary(),
            round_targets.len(),
            touched.len(),
            sample.join(", "),
            if touched.len() > sample.len() {
                " …"
            } else {
                ""
            }
        )),
    }
}

/// 换代那一轮的出声与观测：判据是 [`observe_generation_swap`]，出声限流，结论进
/// `/health.room_build.resident.last_swap`。
fn report_generation_swap(
    model: &crate::fast_model::room_resident::ResidentRoomModel,
    diff: &crate::fast_model::room_resident::TopologyDiff,
    panels: &[PendingModelWork],
    panel_page_full: bool,
) {
    let round_targets: BTreeSet<RefnoEnum> = panels
        .iter()
        .filter_map(|job| RefU64::from_str(&job.target_refno).ok())
        .map(RefnoEnum::from)
        .collect();
    let observation = observe_generation_swap(diff, &round_targets, panel_page_full);
    if observation.fired
        && let Some(line) = observation.note.as_deref()
        && claim_announcement(&RESIDENT_GUARD_ANNOUNCEMENT, &diff.summary())
    {
        println!("[常驻房间模型护栏] {line}");
    }
    crate::fast_model::room_resident::record_generation_swap(
        crate::fast_model::room_resident::GenerationSwapReport {
            form: model.manifest.form.as_str().to_string(),
            key: model.manifest.key.chars().take(12).collect(),
            diff: Some(diff.summary()),
            touched_panels: observation.touched,
            matched_targets: observation.matched,
            round_targets: round_targets.len(),
            guard_fired: observation.fired,
            note: observation.note,
            at: chrono::Local::now().to_rfc3339(),
        },
    );
}

/// 写点 ③（常驻房间模型 S4）：一轮 drain 全成、房间表里一个活口都不剩、且启动对比没留
/// 尾巴时，把这一份拓扑钉成参照。
///
/// 参照的含义是「库里的归属边就是按这份拓扑算出来的」，下次启动拿它做增量对比。三条前提
/// 缺一不可：
///
/// 1. 本轮**没有失败**；
/// 2. 此刻**没有任何未消费的房间目标**（含死信）——少了它就可能有一块面板的边还停在上一份
///    拓扑上，而参照却说它是新的，下次启动的 diff 把这块面板算成「没变」，陈旧的边永久留下；
/// 3. **本进程的启动对比没留下尾巴**（M2）。前两条都只看得见「房间表」与「本轮」：启动对比
///    里失败或无几何的面板此前既不入队、也不标脏，于是两条前提恒真，任意一轮干净的 drain
///    都会把参照推进到当前拓扑。现在那一摊会被 `apply_startup_plan` 折成 `RoomRecalcPanel`
///    （第 2 条就挡得住），这一条是它的**互补**：入队本身失败、或者行被范围谓词滤掉、
///    又或者对比压根还没跑（worker 起在房间段之前）时，只剩它。
///
/// 写失败只影响下次启动（退回 `NoReference` → 全量一次，多算不漏算），所以只出声、
/// 不让本轮 drain 失败。
async fn settle_resident_reference(
    model: &crate::fast_model::room_resident::ResidentRoomModel,
    report: &DrainReport,
) {
    if !report.failures.is_empty() {
        return;
    }
    if let Some(tail) = crate::fast_model::room_startup::startup_compare_tail() {
        if claim_announcement(&REFERENCE_TAIL_ANNOUNCEMENT, &tail) {
            println!("[房间参照] 参照不写——启动对比这一轮留了尾巴：{tail}");
        }
        return;
    }
    match room_work_is_drained().await {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            println!("[房间参照] 无法确认房间目标是否已清空，参照不写: {error:#}");
            return;
        }
    }
    match crate::fast_model::room_resident::persist_reference(model).await {
        Ok(reference) => println!(
            "[房间参照] 一轮 drain 全成、房间目标已清空：参照已更新 {} {}（{} 间房 / {} 块在册面板 / {} 个令牌）",
            reference.form,
            reference.key.chars().take(12).collect::<String>(),
            reference.rooms,
            reference.registered_panels,
            reference.tokens.len()
        ),
        Err(error) => println!("[房间参照] 参照未更新: {error:#}"),
    }
}

/// 房间表里还有没有活口——**含死信**。
///
/// `has_pending_work` 那一支只看还会被重试的行；撞满 [`MAX_ATTEMPTS`] 的死信行同样是
/// 「这个目标没算成」，对参照而言两者没有区别。
async fn room_work_is_drained() -> anyhow::Result<bool> {
    if has_pending_work(ROOM_ACTION_FILTER).await? {
        return Ok(false);
    }
    let mut response = SUL_DB
        .query(render_has_pending_work_query(
            ROOM_ACTION_FILTER,
            ">=",
            &crate::data_interface::watch_scope::dbnums(),
            &crate::data_interface::model_type_scope::nouns(),
        ))
        .await?
        .check()?;
    Ok(!response.take::<Option<bool>>(0)?.unwrap_or(true))
}

pub(crate) async fn has_room_work() -> anyhow::Result<bool> {
    room_work_is_drained().await.map(|drained| !drained)
}

/// 项目级全量房间计算已经重新写过全部当前面板的成员边后，删除在计算开始前遗留的
/// 房间任务。计算期间产生的新 revision 通过 `updated_at > cutoff` 保留给增量路径。
pub(crate) async fn clear_room_work_covered_by_full_rebuild(cutoff: &str) -> anyhow::Result<u64> {
    clear_room_work_covered_by_full_rebuild_on(&SUL_DB, cutoff).await
}

async fn clear_room_work_covered_by_full_rebuild_on(
    db: &Surreal<Any>,
    cutoff: &str,
) -> anyhow::Result<u64> {
    let cutoff = escape_surql_str(cutoff);
    let predicate = format!(
        "action IN ['room_recalc_element', 'room_recalc_panel'] AND updated_at <= d'{cutoff}'"
    );
    let mut response = db
        .query(format!(
            "SELECT count() AS count FROM {TABLE} WHERE {predicate} GROUP ALL; \
             DELETE {TABLE} WHERE {predicate} RETURN NONE;"
        ))
        .await?
        .check()?;
    Ok(response
        .take::<Vec<PendingRowCount>>(0)?
        .first()
        .map_or(0, |row| row.count))
}

async fn drain_room_element_page(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panel_index: &room_model::PanelIndex,
    claimed_members: &HashSet<RefnoEnum>,
    claimed_panels: &HashSet<RefnoEnum>,
    elements: &[PendingModelWork],
    published_version: u64,
    yield_for_data: bool,
    report: &mut DrainReport,
) {
    let element_refnos: Vec<RefnoEnum> = elements
        .iter()
        .filter_map(|job| RefU64::from_str(&job.target_refno).ok())
        .map(RefnoEnum::from)
        .collect();
    let history = match room_model::ElementRoomHistory::load(&element_refnos).await {
        Ok(history) => Some(history),
        Err(error) => {
            println!(
                "构件现存归属快照加载失败，本页不吸收任何元素任务（归属变化日志会把旧房间\
                 显示成「无房间」）: {error:#}"
            );
            None
        }
    };
    let empty_history = room_model::ElementRoomHistory::default();
    let history_ref = history.as_ref().unwrap_or(&empty_history);
    let absorb_candidates: Vec<RefnoEnum> = element_refnos
        .iter()
        .copied()
        .filter(|refno| claimed_members.contains(refno))
        .collect();
    let closure_inputs = match history.as_ref() {
        None => None,
        Some(_) if absorb_candidates.is_empty() => None,
        Some(history) => {
            match load_absorption_closure_inputs(panel_index, history, &absorb_candidates).await {
                Ok(inputs) => Some(inputs),
                Err(error) => {
                    println!("吸收封闭性输入加载失败，本页不吸收任何元素任务: {error:#}");
                    None
                }
            }
        }
    };

    for job in elements {
        let absorbed = RefU64::from_str(&job.target_refno)
            .ok()
            .map(RefnoEnum::from)
            .is_some_and(|refno| {
                claimed_members.contains(&refno)
                    && closure_inputs
                        .as_ref()
                        .is_some_and(|inputs| absorption_verdict(inputs, refno, claimed_panels))
            });
        let outcome = if absorbed {
            complete_room_work(job, published_version).await
        } else {
            match run_room_job_isolated(
                db_option,
                rooms,
                panel_index,
                history_ref,
                job,
                published_version,
            )
            .await
            {
                Ok(_) => Ok(()),
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) => report.done += 1,
            Err(error) => {
                let _ = record_failure(job, &error, report).await;
            }
        }
        if yield_for_data && data_batch_waiting() {
            return;
        }
    }
}

async fn load_room_jobs(
    action_filter: &str,
    limit: Option<usize>,
) -> anyhow::Result<Vec<PendingModelWork>> {
    let mut response = SUL_DB
        .query(render_drain_select(action_filter, limit))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("load pending room work statement failed: {error}"))?;
    response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending room work failed: {error}"))
}

async fn load_scoped_room_jobs(keys: &[RoomWorkKey]) -> anyhow::Result<Vec<PendingModelWork>> {
    load_scoped_room_jobs_on(&SUL_DB, keys).await
}

async fn load_scoped_room_jobs_on(
    db: &Surreal<Any>,
    keys: &[RoomWorkKey],
) -> anyhow::Result<Vec<PendingModelWork>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    for chunk in keys.chunks(QUERY_CHUNK) {
        let mut response = db
            .query(render_scoped_room_select(chunk))
            .await?
            .check()
            .map_err(|error| anyhow::anyhow!("load scoped room work statement failed: {error}"))?;
        let mut page: Vec<PendingModelWork> = response
            .take(0)
            .map_err(|error| anyhow::anyhow!("decode scoped room work failed: {error}"))?;
        jobs.append(&mut page);
    }
    jobs.sort_by(|left, right| left.target_refno.cmp(&right.target_refno));
    Ok(jobs)
}

/// 同轮吸收的封闭性输入：候选元素的现存归属边与当前空间树候选面板。
#[derive(Debug, Default)]
struct AbsorptionClosureInputs {
    /// 元素 → 现存 `room_relate` 入边的面板集合。没有旧边的元素不在映射里（等价空集）。
    old_edge_panels: std::collections::HashMap<RefnoEnum, HashSet<RefnoEnum>>,
    /// 元素 → 当前世界包围盒与在册面板（库内几何，[`room_model::PanelIndex`]）相交的
    /// PANE 集合。与元素分支 `recalc_element_membership` 的候选**同源**，二者不可分叉。
    /// 查不到实例或包围盒不可用的构件不在映射里——候选未知，吸收判定必须让路。
    candidate_panels: std::collections::HashMap<RefnoEnum, HashSet<RefnoEnum>>,
}

/// 吸收的封闭性判据（纯函数）。
///
/// 整间分支只重写了本轮 claimed 面板的出边；元素分支才会「删该构件全部入边再写回」。
/// 只有当该构件的旧归属面板与当前候选面板都落在 claimed 集合里，跳过元素任务才
/// 不会丢删陈旧边（旧面板不在本轮）或漏写新边（新面板不在本轮）。
fn absorption_is_closed(
    old_edge_panels: &HashSet<RefnoEnum>,
    candidate_panels: &HashSet<RefnoEnum>,
    claimed_panels: &HashSet<RefnoEnum>,
) -> bool {
    old_edge_panels.is_subset(claimed_panels) && candidate_panels.is_subset(claimed_panels)
}

/// 一个候选元素的吸收裁决：旧边缺省为空集（没有旧边不阻碍吸收），候选集缺失
/// 视为未知、一律不吸收。
fn absorption_verdict(
    inputs: &AbsorptionClosureInputs,
    element: RefnoEnum,
    claimed_panels: &HashSet<RefnoEnum>,
) -> bool {
    let no_old_edges = HashSet::new();
    let old = inputs
        .old_edge_panels
        .get(&element)
        .unwrap_or(&no_old_edges);
    inputs
        .candidate_panels
        .get(&element)
        .is_some_and(|candidates| absorption_is_closed(old, candidates, claimed_panels))
}

/// 为本轮吸收候选整理封闭性输入：旧边取自整页快照，候选面板走库内面板几何。
///
/// 旧边**不再自己发查询**：本轮开头的 [`room_model::ElementRoomHistory`] 已经把整页元素
/// 的 `room_relate` 入边查回来了，元素分支的归属变化日志读的也是它。同一份边问两遍
/// 除了多一次往返，还留下了两份可能分叉的读法。
///
/// 候选面板**不经过空间树**：元素分支（`recalc_element_membership`）2026-08-05 已改从
/// 本轮加载的在册面板几何（[`room_model::PanelIndex`]）选候选，这里预测它会碰哪些面板的
/// 逻辑必须同源。留在树上的话，树缺在册 PANE 条目时（issue #7 的典型态）会拿到空候选、
/// 错误吸收，把元素分支本会写的边永久跳过——正是那类静默漏分配。
async fn load_absorption_closure_inputs(
    panels: &room_model::PanelIndex,
    history: &room_model::ElementRoomHistory,
    elements: &[RefnoEnum],
) -> anyhow::Result<AbsorptionClosureInputs> {
    let mut inputs = AbsorptionClosureInputs::default();

    for &element in elements {
        let old_panels = history.panels_of(element);
        // 没有旧边的元素**不**插入映射：`absorption_verdict` 把缺项读成空集，
        // 而插入一个空集在语义上与之等价，留空更贴近「这条边本来就不存在」。
        if !old_panels.is_empty() {
            inputs.old_edge_panels.insert(element, old_panels);
        }
    }

    // 候选面板与元素分支同源：库内面板几何（PanelIndex）+ 库内构件世界包围盒。
    inputs.candidate_panels = room_model::element_candidate_panels(panels, elements).await?;
    Ok(inputs)
}

async fn run_room_job(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panels: &room_model::PanelIndex,
    history: &room_model::ElementRoomHistory,
    job: &PendingModelWork,
    published_version: u64,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&job.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid pending refno {}", job.target_refno))?,
    );
    run_room_task(
        db_option,
        rooms,
        panels,
        history,
        job,
        refno,
        published_version,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    // S5a 搬走的三块（收口尾事务与恢复记录、模型水位、启动复核）连同它们的源码钉还留在
    // 本文件的测试里；再导出拆掉之后改由这里直接引。测试搬家另算一笔。
    use crate::data_interface::increment_tail::*;
    // S7 搬去 `model_coverage.rs` 的只读取数，测试同上先留在这里直接引。
    use crate::data_interface::model_coverage::{
        backfill_gen_root_dbnum_on, coverage_tracked_dbnums_on, startup_coverage_floors_on,
    };
    use crate::data_interface::model_watermark::*;
    use crate::data_interface::startup_reconcile::*;
    use anyhow::Context as _;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// ADR-050（2026-09-03 修订）: rows from a previous process are not recovery input.
    /// The startup barrier removes the five data actions for every dbnum (not just the
    /// watch scope, and `dbnum = 0` on-demand rows too), and keeps `room_recalc_*`:
    /// data work is re-derived from `gen_root` credentials by the startup reconcile,
    /// room work has no such compensation outside the full room rebuild.
    #[tokio::test]
    async fn startup_cleanup_removes_previous_data_work_and_keeps_room_work() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("startup_cleanup").await.unwrap();
        db.query(
            "CREATE model_update_pending:transform SET dbnum = 8000, action = 'transform';\
             CREATE model_update_pending:delete SET dbnum = 8000, action = 'delete_cleanup';\
             CREATE model_update_pending:cascade SET dbnum = 8000, action = 'cascade_expand';\
             CREATE model_update_pending:regen SET dbnum = 1112, action = 'regen_root';\
             CREATE model_update_pending:aabb SET dbnum = 1112, action = 'post_regen_aabb';\
             CREATE model_update_pending:on_demand SET dbnum = 0, action = 'regen_root';\
             CREATE model_update_pending:room_element SET dbnum = 8000, action = 'room_recalc_element';\
             CREATE model_update_pending:room_panel SET dbnum = 1112, action = 'room_recalc_panel';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert_eq!(
            clear_stale_at_process_start_on(&db).await.unwrap(),
            6,
            "数据五种不分库全清，dbnum=0 的按需行也清"
        );

        let mut response = db
            .query("SELECT VALUE record::id(id) FROM model_update_pending;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut survivors: Vec<String> = response.take(0).unwrap();
        survivors.sort();
        assert_eq!(
            survivors,
            vec!["room_element".to_string(), "room_panel".to_string()],
            "房间任务原样留下，由房间轮收敛"
        );
    }

    #[tokio::test]
    async fn initialization_ownership_survives_cleanup_and_blocks_only_completion() {
        use surrealdb::engine::any::connect;
        let db = connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("owned_initialization")
            .await
            .unwrap();
        db.query("CREATE model_update_pending:owned SET action='regen_root', status='pending', attempts=0, init_run_id='run'; CREATE model_update_pending:ordinary SET action='regen_root', status='pending', attempts=0;")
            .await.unwrap().check().unwrap();
        assert_eq!(clear_stale_at_process_start_on(&db).await.unwrap(), 1);
        let mut barrier = db
            .query(render_has_pending_work_query(
                REGEN_ACTION_FILTER,
                "<",
                &[],
                &[],
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(barrier.take::<Option<bool>>(0).unwrap(), Some(true));
        let mut claim = db
            .query(render_has_pending_work_query(
                &format!("{REGEN_ACTION_FILTER} AND init_run_id IS NONE"),
                "<",
                &[],
                &[],
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(claim.take::<Option<bool>>(0).unwrap(), Some(false));
    }

    /// 启动清理与整库重建作废用的是同一份数据动作清单：任何一边改了口径，另一边
    /// 必须一起改，否则「什么算可从凭证补偿的工作」会有两个答案。
    #[test]
    fn startup_cleanup_uses_the_same_data_action_filter_as_full_rebuild_discard() {
        let source = include_str!("model_update_pending.rs");
        let cleanup = source
            .split_once("async fn clear_stale_at_process_start_on(")
            .expect("startup cleanup exists")
            .1
            .split_once("/// 整库重建开工前作废")
            .expect("cleanup ends before the rebuild discard")
            .0;
        assert!(
            cleanup.contains(
                "DELETE {TABLE} WHERE init_run_id IS NONE {DATA_ACTION_FILTER}{init_exclusion};"
            ),
            "启动清理只删数据五种: {cleanup}"
        );
        assert!(
            !cleanup.contains("DELETE {TABLE};"),
            "回到整表 DELETE 即红——房间任务没有补偿路径: {cleanup}"
        );
    }

    /// C2-4 / R6 —— 整库重建作废这个库排着的增量，但只作废数据五种。
    ///
    /// 三类必须活下来，各自的理由不同：房间任务重建不会替它们重排；别的库跟这次
    /// 重建无关；`dbnum = 0` 是按需生成当场落的行，不是任何窗口的陈旧结论。
    #[tokio::test]
    async fn full_rebuild_drops_pending_incremental_work() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("full_rebuild_discard")
            .await
            .unwrap();
        db.query(
            "CREATE model_update_pending:stale_transform SET dbnum = 8000, action = 'transform';\
             CREATE model_update_pending:stale_delete SET dbnum = 8000, action = 'delete_cleanup';\
             CREATE model_update_pending:stale_cascade SET dbnum = 8000, action = 'cascade_expand';\
             CREATE model_update_pending:stale_regen SET dbnum = 8000, action = 'regen_root';\
             CREATE model_update_pending:stale_aabb SET dbnum = 8000, action = 'post_regen_aabb';\
             CREATE model_update_pending:room_element SET dbnum = 8000, action = 'room_recalc_element';\
             CREATE model_update_pending:room_panel SET dbnum = 8000, action = 'room_recalc_panel';\
             CREATE model_update_pending:other_db SET dbnum = 1112, action = 'transform';\
             CREATE model_update_pending:on_demand SET dbnum = 0, action = 'regen_root';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert_eq!(
            discard_pending_for_full_rebuild_on(&db, 8000)
                .await
                .unwrap(),
            5,
            "数据五种全清"
        );

        let mut response = db
            .query("SELECT VALUE record::id(id) FROM model_update_pending;")
            .await
            .unwrap()
            .check()
            .unwrap();
        // 排序留在 Rust 这边：`SELECT VALUE` 投出的是裸值，投影里已经没有 `id` 这个
        // 字段了，跟一句 `ORDER BY id` 会被 SurrealDB 直接拒掉（Missing order idiom）。
        let mut survivors: Vec<String> = response.take(0).unwrap();
        survivors.sort();
        assert_eq!(
            survivors,
            vec!["on_demand", "other_db", "room_element", "room_panel"]
        );
    }

    /// Cleanup ordering is a behavioral invariant: moving the call into
    /// `run_cli` would already be too late because spatial state is loaded in
    /// `run_app` first. Keep it after connection and before every startup stage.
    #[test]
    fn startup_cleanup_precedes_spatial_watcher_worker_and_model_startup() {
        let source = include_str!("../lib.rs");
        let run_app = source.find("pub async fn run_app").unwrap();
        let source = &source[run_app..];
        // 锚点从 `match init_surreal().await` 换成连库那一句（2026-09-04 store_mode
        // 三分支）：连库不再只有一条路，三档介质的分派收在 `ensure_store_connected` 里。
        // 这条测试守的是**顺序**，不是某一条连接语句长什么样。
        let connected = source.find("ensure_store_connected(&db_option").unwrap();
        let cleanup = source
            .find("model_update_pending::clear_stale_at_process_start()")
            .unwrap();
        let spatial = source.find("load_project_tree_verified().await").unwrap();
        let run_cli = source.find("run_cli(db_option).await").unwrap();

        assert!(
            connected < cleanup,
            "cleanup ran before the connection attempt"
        );
        assert!(
            cleanup < spatial,
            "spatial/model state loaded before cleanup"
        );
        assert!(
            cleanup < run_cli,
            "watcher/worker startup became reachable before cleanup"
        );
    }

    /// ADR-048: an explicit watch scope is an execution boundary for the
    /// durable queue, not merely a filter on new filesystem observations.
    #[test]
    fn automatic_durable_drain_is_bounded_by_watch_dbnums() {
        let scope = [8000];
        let predicate = "AND ((dbnum?:0) = 0 OR (dbnum?:0) IN [8000])";

        let drain = render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &scope, &[]);
        assert!(
            drain.contains(predicate),
            "drain escaped watch scope: {drain}"
        );

        let retryable = render_has_pending_work_query(DATA_ACTION_FILTER, "<", &scope, &[]);
        assert!(
            retryable.contains(predicate),
            "readiness escaped watch scope: {retryable}"
        );

        let dead = render_has_pending_work_query(DATA_ACTION_FILTER, ">=", &scope, &[]);
        assert!(
            dead.contains(predicate),
            "dead-letter gate escaped watch scope: {dead}"
        );

        let status = render_model_pending_status_query_for_scope(&scope, &[]);
        assert_eq!(
            status.matches(predicate).count(),
            3,
            "all health/status statements must share the watch predicate: {status}"
        );

        // 排除仍然要真的排除：域外的库不能因为放行无主行而一起被放进来。
        assert!(
            drain.contains("(dbnum?:0) IN [8000]"),
            "numeric scope must still exclude other dbnums: {drain}"
        );
    }

    /// `dbnum = 0` 是「不属于任何库」，不是「0 号库」——它必须落在每个数字域**之内**。
    ///
    /// 房间重算与 `ensure_regen_pending` 都故意不认领来源库（各自有注释与断言钉着），
    /// 于是按数字域排除它们，等于让带 `--watch-dbnum` 的进程永远消费不掉自己刚排出来
    /// 的活。2026-08-27 现场：模型阶段收工后房间轮以每秒一轮的节奏空转了 7697 轮，
    /// 每轮「完成 0、失败 0（开跑前 2553 个目标）」，而 `/health` 全程 `ok`。
    #[test]
    fn dbnum_less_work_is_inside_every_declared_scope() {
        let scope = [8000];
        for sql in [
            render_drain_select_for_scope(ROOM_ACTION_FILTER, Some(256), &scope, &[]),
            render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &scope, &[]),
            render_room_target_counts_query(&scope),
            render_has_pending_work_query(ROOM_ACTION_FILTER, "<", &scope, &[]),
            render_model_pending_status_query_for_scope(&scope, &[]),
        ] {
            assert!(
                sql.contains("(dbnum?:0) = 0 OR"),
                "无主行被数字域挡在外面，这一行就永远没人消费: {sql}"
            );
        }
    }

    /// 空闲轮问的那个数与它随后能取到的行必须出自同一个谓词。
    ///
    /// 分家的代价是一个自己喂自己的环：`count_room_targets` 说 2553 个目标、
    /// `load_room_jobs` 一行都取不出来、本轮完成 0，于是下一轮再问一次。它不报错、
    /// 不计次、不进死信，`/health` 也看不见——只有 CPU 知道。
    #[test]
    fn the_room_counter_and_the_room_loader_share_one_predicate() {
        let scope = [8000];
        let counts = render_room_target_counts_query(&scope);
        let loader = render_drain_select_for_scope(ROOM_ACTION_FILTER, Some(256), &scope, &[]);
        let predicate = render_automatic_scope_filter(&scope);

        assert!(
            !predicate.is_empty(),
            "这条断言只在声明了域时才有意义: {predicate}"
        );
        assert_eq!(
            counts.matches(predicate.as_str()).count(),
            1,
            "房间活口计数必须带域: {counts}"
        );
        assert!(
            loader.contains(predicate.as_str()),
            "加载器必须与计数器同域: {loader}"
        );
        assert!(
            !counts.contains("attempts")
                && loader.contains("action IN ['room_recalc_panel', 'room_recalc_element'] OR"),
            "房间目标无次数上限: {counts}"
        );
    }

    /// No declared scope is byte-for-byte compatible with the historical
    /// unrestricted selection. This prevents an empty `watch_dbnums` from
    /// turning into `IN []` and silently stopping every deployment.
    #[test]
    fn empty_watch_scope_keeps_the_durable_drain_unrestricted() {
        assert_eq!(render_automatic_scope_filter(&[]), "");
        for sql in [
            render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &[], &[]),
            render_has_pending_work_query(DATA_ACTION_FILTER, "<", &[], &[]),
            render_model_pending_status_query_for_scope(&[], &[]),
        ] {
            assert!(
                !sql.contains("dbnum?:0"),
                "empty scope narrowed work: {sql}"
            );
        }
    }

    /// 模型类型限定（`model_incremental_types`）与监听限定同款：它是 durable 队列的
    /// **执行边界**，不是过滤解码后的行。谓词必须进 drain / claim / cohort mate / 计数 /
    /// 就绪探测 / `/health` 每一条语句，两侧对「哪些行算数」的答案必须是同一个。
    #[test]
    fn automatic_durable_drain_is_bounded_by_model_incremental_types() {
        let nouns = vec!["BRAN".to_string(), "EQUI".to_string()];
        let predicate = render_automatic_type_filter(&nouns);
        assert_eq!(
            predicate,
            " AND (action NOT IN ['regen_root', 'delete_cleanup'] OR (noun?:'') = '' OR \
             string::uppercase(noun?:'') IN ['BRAN', 'EQUI'])"
        );

        let drain = render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &[], &nouns);
        assert!(
            drain.contains(&predicate),
            "drain escaped type scope: {drain}"
        );
        let claim = render_drain_claim_for_scope(REGEN_ACTION_FILTER, Some(16), &[], &nouns, "w");
        assert!(
            claim.contains(&predicate),
            "claim escaped type scope: {claim}"
        );
        assert!(
            claim.starts_with("LET $active_cohorts = SELECT VALUE cohort_id"),
            "{claim}"
        );
        assert!(
            claim.contains("cohort_id NOT IN $active_cohorts"),
            "{claim}"
        );
        assert_eq!(
            claim
                .matches("SELECT VALUE cohort_id FROM model_update_pending")
                .count(),
            1,
            "claim must materialize active cohorts once instead of using an unsupported correlated subquery: {claim}"
        );
        let mates = render_cohort_mate_claim_for_scope(
            REGEN_ACTION_FILTER,
            &[],
            &nouns,
            "w",
            &["8000:12".to_string()],
        )
        .expect("cohort mates are claimed when a cohort was opened");
        assert!(
            mates.contains(&predicate),
            "cohort mates escaped type scope: {mates}"
        );
        let count = render_pending_count_query(REGEN_ACTION_FILTER, &[], &nouns);
        assert!(
            count.contains(&predicate),
            "queue_total escaped type scope: {count}"
        );
        let retryable = render_has_pending_work_query(DATA_ACTION_FILTER, "<", &[], &nouns);
        assert!(
            retryable.contains(&predicate),
            "readiness escaped type scope: {retryable}"
        );
        let dead = render_has_pending_work_query(DATA_ACTION_FILTER, ">=", &[], &nouns);
        assert!(
            dead.contains(&predicate),
            "dead-letter gate escaped type scope: {dead}"
        );

        // 监听限定与类型限定同时在场时两个谓词都得在：库域之内、类型之外的行同样不消费。
        let both = render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &[8000], &nouns);
        assert!(
            both.contains("(dbnum?:0) IN [8000]") && both.contains(&predicate),
            "{both}"
        );
    }

    /// `/health` 那条查询：三条既有语句都带放行谓词，且**多出一条**互补的暂扣计数——
    /// 一行只能落在「待消费」与「被暂扣」之一，`retryable` 与 worker 的就绪探测同口径。
    #[test]
    fn health_status_reports_rows_held_by_model_incremental_types_separately() {
        let nouns = vec!["BRAN".to_string()];
        let status = render_model_pending_status_query_for_scope(&[], &nouns);
        let admit = render_automatic_type_filter(&nouns);
        let hold = render_automatic_type_holdback(&nouns).expect("a scope has a complement");
        assert_eq!(
            status.matches(admit.as_str()).count(),
            3,
            "all health/status statements must share the type predicate: {status}"
        );
        assert_eq!(
            status.matches(hold.as_str()).count(),
            1,
            "held rows are counted by exactly one extra statement: {status}"
        );
        assert_eq!(status.matches(';').count(), 4, "{status}");
        assert_eq!(
            hold,
            " AND action IN ['regen_root', 'delete_cleanup'] AND (noun?:'') != '' AND \
             string::uppercase(noun?:'') NOT IN ['BRAN']"
        );
    }

    /// 没限定时每一条语句都与引入前逐字相同，`/health` 也不多出那条暂扣计数。
    #[test]
    fn empty_type_scope_keeps_the_durable_drain_unrestricted() {
        assert_eq!(render_automatic_type_filter(&[]), "");
        assert_eq!(render_automatic_type_holdback(&[]), None);
        for sql in [
            render_drain_select_for_scope(REGEN_ACTION_FILTER, Some(16), &[], &[]),
            render_drain_claim_for_scope(REGEN_ACTION_FILTER, Some(16), &[], &[], "w"),
            render_pending_count_query(REGEN_ACTION_FILTER, &[], &[]),
            render_has_pending_work_query(DATA_ACTION_FILTER, "<", &[], &[]),
            render_model_pending_status_query_for_scope(&[], &[]),
        ] {
            assert!(
                !sql.contains("string::uppercase(noun"),
                "empty type scope narrowed work: {sql}"
            );
        }
        assert_eq!(
            render_model_pending_status_query_for_scope(&[], &[])
                .matches(';')
                .count(),
            3
        );
    }

    /// 谓词真跑一遍：名单外的 regen 根取不回来、名单内的取得回来，noun 为空的
    /// Transform 行与房间行不受名单影响，`/health` 的暂扣计数与放行计数互补。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_type_scoped_process_leaves_other_model_types_untouched_in_the_queue() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem surreal");
        db.use_ns("type_scope")
            .use_db("type_scope")
            .await
            .expect("select fixture database");
        let item = |action: ModelWorkAction, target: &str, noun: &str| ModelWorkItem {
            dbnum: 8000,
            db_type: "DESI".into(),
            source_end_sesno: 12,
            action,
            target_refno: target.into(),
            noun: noun.into(),
        };
        let items = [
            item(ModelWorkAction::RegenRoot, "24384/1", "BRAN"),
            item(ModelWorkAction::RegenRoot, "24384/2", "HANG"),
            item(ModelWorkAction::RegenRoot, "24384/3", "bran"),
            item(ModelWorkAction::DeleteCleanup, "24384/4", "SUPPO"),
            item(ModelWorkAction::DeleteCleanup, "24384/5", ""),
            item(ModelWorkAction::Transform, "24384/6", ""),
            item(ModelWorkAction::RoomRecalcElement, "24384/7", "HANG"),
        ];
        db.query(
            items
                .iter()
                .map(render_upsert_no_time)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .await
        .expect("seed rows")
        .check()
        .expect("seed statements");

        let nouns = vec!["BRAN".to_string()];
        let claimed = claim_page_with_cohorts_on(&db, "", None, &[], &nouns, "type-worker")
            .await
            .expect("claim within type scope");
        let mut targets = claimed
            .iter()
            .map(|job| job.target_refno.as_str())
            .collect::<Vec<_>>();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec!["24384/1", "24384/3", "24384/5", "24384/6", "24384/7"],
            "只有名单外且 noun 已知的 regen/delete 行留在队列里: {claimed:#?}"
        );

        let mut response = db
            .query(render_model_pending_status_query_for_scope(&[], &nouns))
            .await
            .expect("status query")
            .check()
            .expect("status statements");
        let held: Vec<PendingRowCount> = response.take(3).expect("held count");
        assert_eq!(
            held.first().map(|row| row.count),
            Some(2),
            "HANG regen + SUPPO delete"
        );
    }

    /// 多数用例只关心复活 / 覆盖那几条子句，与来源保存时刻无关。
    fn render_upsert_no_time(item: &ModelWorkItem) -> String {
        render_upsert(item, None)
    }

    /// 前两条断言的是 SQL 长什么样，这一条把它真的跑一遍：无主行取得回来，
    /// 域外的库仍然取不回来，而且计数器与加载器给出同一个数。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_scoped_process_still_consumes_its_own_dbnum_less_room_work() {
        use surrealdb::engine::any::connect;

        #[derive(Debug, Deserialize)]
        struct CountedAction {
            action: String,
            c: usize,
        }

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("dbnum_less_scope")
            .await
            .expect("select fixture db");

        // 房间任务：故意不认领来源库（`room_recalc_item` 写死 dbnum = 0）。
        let room = room_recalc_item(&AabbChange {
            refno: RefnoEnum::from(RefU64((24384u64 << 32) | 22403)),
            noun: "FTUB".into(),
        });
        assert_eq!(room.dbnum, 0, "这条用例的前提就是房间任务无主");
        // 域外的库：必须继续被挡住。
        let stranger = ModelWorkItem {
            dbnum: 9999,
            db_type: "DESI".into(),
            source_end_sesno: 0,
            action: ModelWorkAction::RoomRecalcElement,
            target_refno: "24999/1".into(),
            noun: "EQUI".into(),
        };
        db.query(format!(
            "{}\n{}",
            render_upsert_no_time(&room),
            render_upsert_no_time(&stranger)
        ))
        .await
        .expect("seed rows")
        .check()
        .expect("seed statements");

        let scope = [8000];
        let mut response = db
            .query(render_drain_select_for_scope(
                ROOM_ACTION_FILTER,
                Some(256),
                &scope,
                &[],
            ))
            .await
            .expect("load room jobs")
            .check()
            .expect("load statement");
        let loaded: Vec<PendingModelWork> = response.take(0).expect("decode room jobs");
        assert_eq!(
            loaded
                .iter()
                .map(|job| &job.target_refno)
                .collect::<Vec<_>>(),
            vec!["24384/22403"],
            "无主行必须取得回来，域外的库必须取不回来"
        );

        let mut response = db
            .query(render_room_target_counts_query(&scope))
            .await
            .expect("count room targets")
            .check()
            .expect("count statement");
        let counted: Vec<CountedAction> = response.take(0).expect("decode room counts");
        let live = counted.iter().map(|row| row.c).sum::<usize>();
        assert_eq!(
            live,
            loaded.len(),
            "计数器与加载器不同口径就是那个每秒一轮的空转环: {:?}",
            counted.iter().map(|row| &row.action).collect::<Vec<_>>()
        );
    }

    /// 缺失面板触发的模型补偿以“根 + 必需面板集合”为持久事实：同一缺口重复探测
    /// 不应不断推高 revision；只有发现新的缺失面板才产生一个新收口版本。
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_panel_repair_upsert_is_idempotent_and_revision_safe() {
        use surrealdb::engine::any::connect;

        #[derive(Debug, Deserialize)]
        struct RepairRow {
            revision: u64,
            status: String,
            attempts: u32,
            required_panels: Vec<String>,
        }

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("missing_panel_repair")
            .await
            .expect("select fixture db");
        let mut group = PanelRepairGroup {
            root_refno: "4000000001/10".into(),
            noun: "CWALL".into(),
            required_panels: vec!["4000000001/11".into()],
        };

        for _ in 0..2 {
            db.query(render_missing_panel_repair_upsert(&group))
                .await
                .expect("upsert repair")
                .check()
                .expect("repair statement");
        }
        let mut response = db
            .query("SELECT revision, status, attempts, required_panels FROM model_update_pending;")
            .await
            .expect("read repair")
            .check()
            .expect("read statement");
        let rows: Vec<RepairRow> = response.take(0).expect("decode repair");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].revision, 1, "相同缺口不得重复触发生成");
        assert_eq!(rows[0].status, "pending");
        assert_eq!(rows[0].attempts, 0);
        assert_eq!(rows[0].required_panels, ["4000000001/11"]);

        group.required_panels.push("4000000001/12".into());
        db.query(render_missing_panel_repair_upsert(&group))
            .await
            .expect("extend repair")
            .check()
            .expect("extend statement");
        let mut response = db
            .query("SELECT revision, status, attempts, required_panels FROM model_update_pending;")
            .await
            .expect("read extended repair")
            .check()
            .expect("read extended statement");
        let rows: Vec<RepairRow> = response.take(0).expect("decode extended repair");
        assert_eq!(rows[0].revision, 2, "新缺口必须保护正在执行的旧 revision");
        assert_eq!(rows[0].required_panels, ["4000000001/11", "4000000001/12"]);
    }

    #[test]
    fn panel_repair_regen_is_verified_before_settlement() {
        let job = PendingModelWork {
            dbnum: 0,
            db_type: "DESI".into(),
            source_end_sesno: 0,
            source_end_sesno_time: None,
            action: ModelWorkAction::RegenRoot,
            target_refno: "4000000001/10".into(),
            noun: "CWALL".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            revision: 1,
            required_panels: vec!["4000000001/11".into(), "4000000001/12".into()],
        };

        assert!(
            joins_regen_batch(&job),
            "修复根照样进合批（ADR-011 2026-08-09 修订）：生成一次、整页验收一次"
        );
        assert_eq!(
            missing_required_panels(
                &job.required_panels,
                &HashSet::from(["4000000001/11".to_string()])
            ),
            ["4000000001/12"]
        );

        // 验收必须先于收口：批量成功路径上 verify_repair_jobs_page 在
        // clear_regen_work_batch 之前，屏障刷新推高的 revision 才能让旧令牌收口
        // 命中零行（不误删新工作）。
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_where_report(")
            .expect("drain_where_report 必须存在")
            .1
            .split_once("\n// 三个阶段的 action 白名单")
            .expect("drain_where_report 之后是阶段白名单")
            .0;
        let verify_at = body
            .find("verify_repair_jobs_page(")
            .expect("修复根整页验收必须存在");
        let settle_at = body
            .find("clear_regen_work_batch(")
            .expect("批量收口必须存在");
        assert!(verify_at < settle_at, "修复根验收必须先于收口: {body}");
    }

    #[test]
    fn removed_panels_are_filtered_before_the_repair_root_is_generated() {
        let registered = RefnoEnum::from("4000000001/11");
        let stale = RefnoEnum::from("4000000001/12");
        let rooms = room_model::RoomPanelMap {
            rooms: vec![room_model::RoomPanels {
                room: RefnoEnum::from("4000000001/1"),
                room_num: "R100".into(),
                panels: vec![registered],
            }],
            all_panels: HashSet::from([registered, stale]),
        };

        assert_eq!(
            registered_required_panels(&rooms, &[registered.to_pdms_str(), stale.to_pdms_str()])
                .expect("valid required panel refnos"),
            [registered]
        );
    }

    /// 缺陷面板要一直被记账和驱动修复，但**不得**再让整轮 drain 早退。
    ///
    /// 早退曾经是这里的处置，代价是 2 块缺几何的面板把 2580 个房间目标冻了 5 个多
    /// 小时——而它们的修复根早就撞满 `MAX_ATTEMPTS` 进了死信，屏障永远解不开。现在
    /// 排除替换范围由元素分支按面板做，drain 只管记账、修、然后继续往下跑。
    #[test]
    fn panel_defects_are_recorded_without_halting_the_room_drain() {
        let source = include_str!("model_update_pending.rs");
        let verify = source
            .split_once("async fn verify_required_panel_geometry(")
            .expect("verification helper")
            .1
            .split_once("/// 一页修复根的验收结论")
            .expect("verification helper end")
            .0;
        let room_drain = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain")
            .1
            .split_once("let empty_history")
            .expect("drain 的缺陷分支必须在元素历史加载之前结束")
            .0;

        assert!(
            verify.contains("record_room_panel_defects("),
            "完成旧修复根时必须把全局新缺口并入 durable 队列"
        );
        assert!(
            room_drain.contains("record_room_panel_defects("),
            "drain 仍必须登记缺陷面板并驱动修复: {room_drain}"
        );
        assert!(
            !room_drain.contains("return Ok(report);"),
            "缺陷面板不得让整轮 drain 早退: {room_drain}"
        );
    }

    /// 空间状态机门禁（一致性闭环方案 §6）：房间消费在加载任何任务之前必须过
    /// `ensure_spatial_ready`——树不可信时消费任务 = 拿错树改写归属；被拒的
    /// durable 行保留待重试。空 scope 的连接自由早退不受影响（在门之前）。
    #[test]
    fn room_drain_is_gated_by_the_spatial_state_machine() {
        let source = include_str!("model_update_pending.rs");
        let head = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("let panels = ")
            .expect("面板任务加载必须存在")
            .0;
        assert!(
            head.contains("ensure_spatial_ready()"),
            "消费任何房间任务之前必须过状态机门: {head}"
        );
        let lock = head
            .find("lock_spatial_serial().await")
            .expect("房间计算必须固定一个已发布版本");
        let version = head
            .find("published_spatial_version()")
            .expect("房间计算必须记录所消费的发布版本");
        assert!(
            lock < version,
            "先持发布串行锁，再读取版本和派生输入: {head}"
        );
        // 空 scope 早退在门之前：无房间变更的批次不受门禁牵连。
        let scoped = source
            .split_once("pub(crate) async fn drain_rooms_scoped(")
            .expect("scoped drain must exist")
            .1
            .split_once("async fn drain_rooms_selected(")
            .expect("selected follows")
            .0;
        assert!(
            scoped.contains("if scope.is_empty()"),
            "空 scope 必须在进门前早退: {scoped}"
        );
    }

    /// 换代点（常驻房间模型 P2）必须落在串行锁**之内**、读任务**之前**。
    ///
    /// 锁外换代 = 算到一半模型被替掉，拿旧拓扑写新边，两边都不报错。读完任务再换代
    /// 同样不行：那一批任务是按上一份拓扑派生的，配上新拓扑就是张冠李戴。
    #[test]
    fn the_resident_generation_swap_happens_inside_the_spatial_serial_lock() {
        let source = include_str!("model_update_pending.rs");
        let head = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("let panels = ")
            .expect("面板任务加载必须存在")
            .0;
        let lock = head
            .find("lock_spatial_serial().await")
            .expect("房间计算必须固定一个已发布版本");
        let swap = head
            .find("ensure_resident_rooms(db_option)")
            .expect("换代点必须在读任务之前调");
        assert!(lock < swap, "换代必须在串行锁之内: {head}");
    }

    /// 开关开着时拓扑从常驻模型取；关着时**一字不动**地走今天的读法。
    ///
    /// 常驻那份 `map` 与 `load_room_panel_map` 的产物同形（都出自
    /// `room_resident::registered_map`），换的只是「从哪拿」，不是「怎么算」。
    #[test]
    fn the_room_drain_takes_its_topology_from_the_resident_model_when_it_is_on() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("async fn ensure_resident_rooms(")
            .expect("room drain boundary")
            .0;
        assert!(
            body.contains("Some(model) => model.map.clone()"),
            "开着时拓扑必须从常驻模型取: {body}"
        );
        assert!(
            body.contains("None => room_model::load_room_panel_map(db_option).await?"),
            "关着时必须回到今天的读法: {body}"
        );
        let ensure = source
            .split_once("async fn ensure_resident_rooms(")
            .expect("换代点入口")
            .1
            .split_once("\n}\n")
            .expect("入口结束")
            .0;
        assert!(
            ensure.contains("crate::options::room_resident_model()"),
            "换代点必须受开关门控: {ensure}"
        );
        assert!(
            ensure.contains("crate::options::data_face()"),
            "取哪一路模型由数据面决定，不许在这里点名形态: {ensure}"
        );
        // P4：`EnsureOutcome.diff` 是护栏唯一的输入，不许在这里丢掉（此前这一行是
        // `Ok(outcome.map(|outcome| outcome.model))`，diff 直接落地）。
        assert!(
            ensure.contains("diff: outcome.diff"),
            "换代的 diff 必须交给调用方（P4 护栏读它）: {ensure}"
        );
        assert!(
            body.contains("report_generation_swap(&round.model, diff, &panels, panel_page_full)"),
            "护栏必须在这一轮的面板任务上判: {body}"
        );
        let guard_at = body
            .find("report_generation_swap(")
            .expect("护栏必须在 drain 里调");
        let early_return_at = body
            .find("if loaded == 0 && scoped_element_keys.is_empty()")
            .expect("空轮早退必须在");
        assert!(
            guard_at < early_return_at,
            "一个目标都没有恰恰最该出声：护栏排在空轮早退之前: {body}"
        );
    }

    /// P4 护栏的判据（主计划 §3 P4）：拓扑变了而这一轮一块面板目标都没碰到它 → 出声。
    ///
    /// 三条边界一并钉住：diff 为空（换了代但拓扑没动）不出声；有命中不出声；面板页满时
    /// 判据不完整，不出声但要留下原因——那不是「护栏没响」，是「判不了」。
    #[test]
    fn the_guard_only_fires_when_a_real_topology_change_meets_zero_panel_targets() {
        use crate::fast_model::room_resident::TopologyDiff;

        let panel = |id: u32| RefnoEnum::from(RefU64::from_two_nums(24384, id));
        let diff = TopologyDiff {
            recalc_panels: BTreeSet::from([panel(7), panel(8)]),
            ..Default::default()
        };

        let quiet = observe_generation_swap(&TopologyDiff::default(), &BTreeSet::new(), false);
        assert_eq!(
            quiet,
            SwapObservation {
                touched: 0,
                matched: 0,
                fired: false,
                note: None
            },
            "换了代但拓扑没动：本来就无事"
        );

        let hit = observe_generation_swap(&diff, &BTreeSet::from([panel(8), panel(99)]), false);
        assert_eq!((hit.touched, hit.matched, hit.fired), (2, 1, false));
        assert_eq!(hit.note, None, "有命中就是账本触发干了活");

        let missed = observe_generation_swap(&diff, &BTreeSet::from([panel(99)]), false);
        assert_eq!((missed.touched, missed.matched, missed.fired), (2, 0, true));
        let line = missed.note.expect("响了要有那一句");
        assert!(line.contains("recalc=2"), "{line}");
        assert!(
            line.contains("24384/7") && line.contains("24384/8"),
            "{line}"
        );
        assert!(line.contains("F-A"), "护栏要点名它抓的是谁漏了: {line}");
        assert!(
            line.contains("不标脏"),
            "只出声不标脏要写在那一句里: {line}"
        );

        let empty_round = observe_generation_swap(&diff, &BTreeSet::new(), false);
        assert!(
            empty_round.fired,
            "一个面板目标都没有正是护栏要抓的形状: {empty_round:?}"
        );

        let full_page = observe_generation_swap(&diff, &BTreeSet::from([panel(99)]), true);
        assert!(
            !full_page.fired,
            "面板页满了后面还有页，命中判不了：判不了就不判"
        );
        assert!(
            full_page
                .note
                .expect("判不了也要写清为什么")
                .contains("判不了"),
            "「判不了」不等于「护栏没响」，两句话不合成一句"
        );
    }

    /// 限流（与 `batch_worker` 的两处同一条纪律）：理由变了立刻说；同一句话没到期就闭嘴；
    /// 到期再说一次。
    #[test]
    fn the_guard_announcement_is_throttled_by_reason_and_by_clock() {
        let mut slot = (None, 0i64);
        assert!(
            announcement_is_due(&slot, "removed=1", 1_000, ROOM_ANNOUNCE_REPEAT_SECS),
            "第一次总要说"
        );
        slot = (Some("removed=1".to_string()), 1_000);
        assert!(
            !announcement_is_due(&slot, "removed=1", 1_299, ROOM_ANNOUNCE_REPEAT_SECS),
            "同一句话、没到期：闭嘴"
        );
        assert!(
            announcement_is_due(&slot, "removed=2", 1_001, ROOM_ANNOUNCE_REPEAT_SECS),
            "理由变了：立刻说，不等表"
        );
        assert!(
            announcement_is_due(&slot, "removed=1", 1_300, ROOM_ANNOUNCE_REPEAT_SECS),
            "同一句话到期了：再说一次"
        );
        // 槽位版走的是同一个判据，只是把时刻记回去。
        static SLOT: Mutex<(Option<String>, i64)> = Mutex::new((None, 0));
        assert!(claim_announcement(&SLOT, "第一次"));
        assert!(
            !claim_announcement(&SLOT, "第一次"),
            "同一句立刻复述要被吃掉"
        );
        assert!(claim_announcement(&SLOT, "换了个理由"));
    }

    /// M2 (a)：启动对比没算成的面板折成 `RoomRecalcPanel`，形状与几何触发那条路一字不差，
    /// 特别是 `dbnum` / `source_end_sesno` 都不认领。
    ///
    /// `dbnum = 0` 不是偷懒：drain 与 [`room_work_is_drained`] 共用的范围谓词是
    /// `(dbnum?:0) = 0 OR (dbnum?:0) IN [watch_dbnums]`，填真实库号会让未监听库的面板
    /// （「无几何」那一摊的主力）入队之后既没人消费、也不被清空判定看见。
    #[test]
    fn startup_repair_targets_are_room_panel_recalcs_that_claim_no_source_database() {
        let panel = |id: u32| RefnoEnum::from(RefU64::from_two_nums(24384, id));
        assert!(room_panel_recalc_items(&BTreeSet::new()).is_empty());

        let items = room_panel_recalc_items(&BTreeSet::from([panel(8), panel(7)]));
        assert_eq!(items.len(), 2);
        assert_eq!(
            items
                .iter()
                .map(|item| item.target_refno.as_str())
                .collect::<Vec<_>>(),
            vec!["24384/7", "24384/8"],
            "BTreeSet 定序，入队幂等"
        );
        for item in &items {
            assert_eq!(item.action, ModelWorkAction::RoomRecalcPanel);
            assert_eq!(item.noun, "PANE");
            assert_eq!(item.db_type, "DESI");
            assert_eq!(
                (item.dbnum, item.source_end_sesno),
                (0, 0),
                "启动对比不是一次保存窗口，来源库与会话都不认领"
            );
        }
        // 与几何触发那条路同源：同一个目标渲染出同一行 id，重复入队只是复活它。
        assert_eq!(
            record_id(&items[0]),
            record_id(&room_recalc_item(&AabbChange {
                refno: panel(7),
                noun: "PANE".into(),
            }))
        );
    }

    /// M2 (a) 的 `mem://` 门：入队的行**真的**落成房间表里一个待消费目标，且被
    /// [`render_has_pending_work_query`]（`room_work_is_drained` 的两条查询）看见——
    /// 这就是写点 ③ 第二条前提被挡住的机制。空集不写任何一行。
    #[tokio::test]
    async fn requeued_startup_panels_are_visible_to_the_room_queue_drained_probe() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("startup_repair_targets")
            .await
            .expect("select fixture db");

        async fn room_queue_is_drained(db: &Surreal<Any>) -> bool {
            for operator in ["<", ">="] {
                let mut response = db
                    .query(render_has_pending_work_query(
                        ROOM_ACTION_FILTER,
                        operator,
                        &[],
                        &[],
                    ))
                    .await
                    .expect("probe runs")
                    .check()
                    .expect("probe checks");
                if response
                    .take::<Option<bool>>(0)
                    .expect("probe decodes")
                    .unwrap_or(true)
                {
                    return false;
                }
            }
            true
        }

        assert!(room_queue_is_drained(&db).await, "空表 = 已清空");
        assert_eq!(
            enqueue_room_panel_recalcs_on(&db, &BTreeSet::new())
                .await
                .expect("空集不写库"),
            0
        );
        assert!(room_queue_is_drained(&db).await, "空集之后还是空表");

        let panel = RefnoEnum::from(RefU64::from_two_nums(24384, 7));
        assert_eq!(
            enqueue_room_panel_recalcs_on(&db, &BTreeSet::from([panel]))
                .await
                .expect("入队"),
            1
        );
        assert!(
            !room_queue_is_drained(&db).await,
            "没算成的面板在队列里 = 写点 ③ 的第二条前提为假"
        );

        let mut rows = db
            .query(format!(
                "SELECT VALUE {{ status: status, action: action, dbnum: dbnum?:0, \
                 attempts: attempts?:0, target_refno: target_refno }} FROM {TABLE};"
            ))
            .await
            .expect("read back")
            .check()
            .expect("read back checks");
        let rows: Vec<serde_json::Value> = rows.take(0).expect("decode rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["status"], "pending", "{rows:?}");
        assert_eq!(rows[0]["action"], "room_recalc_panel", "{rows:?}");
        assert_eq!(rows[0]["dbnum"], 0, "{rows:?}");
        assert_eq!(rows[0]["attempts"], 0, "{rows:?}");
        assert_eq!(rows[0]["target_refno"], "24384/7", "{rows:?}");

        // 死信也算没消费：撞满 MAX_ATTEMPTS 之后队列依然「没清空」。
        db.query(format!(
            "UPDATE {TABLE} SET status = 'failed', attempts = {MAX_ATTEMPTS};"
        ))
        .await
        .expect("bury")
        .check()
        .expect("bury checks");
        assert!(
            !room_queue_is_drained(&db).await,
            "死信同样是「这个目标没算成」"
        );

        // 再入队一次是幂等的：同一个目标同一行，只是复活它。
        assert_eq!(
            enqueue_room_panel_recalcs_on(&db, &BTreeSet::from([panel]))
                .await
                .expect("重复入队"),
            1
        );
        let mut count = db
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {TABLE}));"
            ))
            .await
            .expect("count")
            .check()
            .expect("count checks");
        assert_eq!(count.take::<Option<i64>>(0).expect("decode"), Some(1));
    }

    /// 写点 ③（S4 + M2）：一轮 drain 之后写参照，三条前提缺一不可——「本轮无失败」、
    /// 「房间表清空」、「启动对比没留尾巴」。
    ///
    /// 少了第二条，参照就会在还有目标没消费时宣称「边 == 这份拓扑」——下次启动的 diff
    /// 把那块面板算成没变，陈旧的边永久留下，静默且无日志。死信也算没消费：撞满
    /// `MAX_ATTEMPTS` 不会让那条边变正确。少了第三条，前两条对**启动对比**留下的那一摊
    /// 恒真（那些面板既不在房间表里、也不产生本轮失败），洞照旧。
    #[test]
    fn the_third_reference_write_needs_a_clean_round_and_an_empty_room_queue() {
        let source = include_str!("model_update_pending.rs");
        let drain = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("async fn ensure_resident_rooms(")
            .expect("room drain boundary")
            .0;
        let pages = drain
            .rfind("drain_room_element_page(")
            .expect("元素页必须存在");
        let write = drain
            .find("settle_resident_reference(model, &report)")
            .expect("写点 ③ 必须存在");
        assert!(pages < write, "参照要在元素页都跑完之后才写: {drain}");

        let settle = source
            .split_once("async fn settle_resident_reference(")
            .expect("写点 ③ 入口")
            .1
            .split_once("\n}\n")
            .expect("入口结束")
            .0;
        assert!(
            settle.contains("report.failures.is_empty()"),
            "本轮有失败就不许推进参照: {settle}"
        );
        assert!(
            settle.contains("room_work_is_drained()"),
            "还有未消费的房间目标就不许推进参照: {settle}"
        );
        let tail_at = settle
            .find("room_startup::startup_compare_tail()")
            .expect("启动对比留了尾巴就不许推进参照");
        let drained_at = settle
            .find("room_work_is_drained().await")
            .expect("清空判定必须在");
        assert!(
            tail_at < drained_at,
            "进程内那道闸是零 I/O 的，排在两条查询之前: {settle}"
        );

        let drained = source
            .split_once("async fn room_work_is_drained(")
            .expect("清空判定")
            .1
            .split_once("\n}\n")
            .expect("判定结束")
            .0;
        assert!(
            drained.contains("has_pending_work(ROOM_ACTION_FILTER)"),
            "会被重试的行要算: {drained}"
        );
        assert!(
            drained.contains("\">=\""),
            "死信同样是「这个目标没算成」，一样要算: {drained}"
        );
    }

    #[test]
    fn room_drain_reads_the_committed_room_geometry_view() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("async fn drain_room_element_page(")
            .expect("room drain boundary")
            .0;
        // 视图由数据面决定（`room_model::consumption_view`）：ingest 读发布视图，读透退回
        // 实时行（审核 F-B）。这里不许再直接点名任何一个视图。
        assert!(
            body.contains("load_consumption_panel_index(db_option, &rooms, resident.clone())"),
            "增量房间消费必须经 consumption 入口加载面板（视图随数据面，面板几何随换代那份）: {body}"
        );
        assert!(
            !body.contains("load_panel_index(db_option, &rooms")
                && !body.contains("load_published_panel_index("),
            "增量房间消费不得自己钉死 Live 或 Published 视图: {body}"
        );
        // 读透没有文件轮次，不建（也不需要）发布输入基线。
        let baseline_at = body
            .find("ensure_published_file_baseline(0, 0)")
            .expect("ingest 形态仍要建基线");
        let face_at = body.find("data_face()").expect("基线与视图都要问数据面");
        assert!(face_at < baseline_at, "建基线之前先问数据面: {body}");
    }

    #[test]
    fn missing_panels_share_one_repair_per_generation_root() {
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};

        let root = GenerationRoot {
            root: RefnoEnum::from("4000000001/10"),
            noun: "CWALL".into(),
            name: "/WALL".into(),
            kind: GenerationRootKind::Normal,
        };
        let groups = group_missing_panel_repairs(vec![
            (RefnoEnum::from("4000000001/12"), root.clone()),
            (RefnoEnum::from("4000000001/11"), root),
        ]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].root_refno, "4000000001/10");
        assert_eq!(
            groups[0].required_panels,
            ["4000000001/11", "4000000001/12"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn incomplete_panel_index_is_recorded_as_a_durable_defect_list() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("room_panel_defects")
            .await
            .expect("select fixture db");
        let groups = vec![PanelRepairGroup {
            root_refno: "4000000001/10".into(),
            noun: "CWALL".into(),
            required_panels: vec!["4000000001/11".into(), "4000000001/12".into()],
        }];

        db.query(render_set_room_panel_defects(&groups))
            .await
            .expect("set defects")
            .check()
            .expect("set defects statement");
        let mut response = db
            .query("SELECT status, missing_panels, repair_roots FROM room_panel_coverage_barrier:current;")
            .await
            .expect("read defects")
            .check()
            .expect("read defects statement");
        let rows: Vec<serde_json::Value> = response.take(0).expect("decode defects");
        assert_eq!(rows[0]["status"], "repairing");
        assert_eq!(rows[0]["missing_panels"].as_array().unwrap().len(), 2);
        assert_eq!(
            rows[0]["repair_roots"],
            serde_json::json!(["4000000001/10"])
        );

        db.query(render_clear_room_panel_defects())
            .await
            .expect("clear defects")
            .check()
            .expect("clear defects statement");
        let mut response = db
            .query("RETURN record::exists(room_panel_coverage_barrier:current);")
            .await
            .expect("read cleared defects")
            .check()
            .expect("read cleared statement");
        assert_eq!(response.take::<Option<bool>>(0).unwrap(), Some(false));
    }

    /// 一件活 panic 了，账要记在它自己那一行上，而不是展开出去变成空闲轮的事。
    ///
    /// 漏这一层的代价在现场量过：`range end index 172 out of range for slice of
    /// length 168` 每 30 秒一次、连刷 46 次，因为 panic 一路展开到空闲轮的
    /// `isolate_panic` 才被接住——那一行的 `last_error` 始终是空的，`attempts` 始终
    /// 是 0，`MAX_ATTEMPTS` 那道死信门永远轮不到它。
    #[test]
    fn a_panicking_job_lands_in_its_own_error_ledger() {
        let source = include_str!("model_update_pending.rs");
        let run_one = source
            .split_once("async fn run_one(")
            .expect("run_one must exist")
            .1
            .split_once("fn render_drain_select")
            .expect("run_one must end before render_drain_select")
            .0;

        assert!(
            run_one.contains("execute_item_isolated(mgr, job)"),
            "run_one 必须走隔离版执行，否则 panic 绕开 record_failure: {run_one}"
        );
        assert!(
            !run_one.contains("execute_item(mgr, job)"),
            "裸 execute_item 会让 panic 展开出整轮 drain: {run_one}"
        );

        // 两条房间路径（面板与元素）也都必须走隔离版。切片而不是全文搜索：
        // 断言字符串自己也含这个名字，全文计数会把测试本身数进去。
        let room_drains = source
            .split_once("async fn drain_rooms_selected(")
            .expect("drain_rooms_selected must exist")
            .1
            .split_once("async fn load_room_jobs(")
            .expect("两个房间执行点都在 load_room_jobs 之前")
            .0;
        assert_eq!(
            room_drains.matches("run_room_job").count(),
            room_drains.matches("run_room_job_isolated").count(),
            "房间执行点里不许有裸调用: {room_drains}"
        );
        assert_eq!(
            room_drains.matches("run_room_job_isolated").count(),
            2,
            "面板与元素两条路径都要走隔离版: {room_drains}"
        );
    }

    #[test]
    fn pending_regeneration_holds_the_shared_root_lock_through_settlement() {
        let source = include_str!("model_update_pending.rs");
        let run_one = source
            .split_once("async fn run_one(")
            .expect("run_one must exist")
            .1
            .split_once("fn render_drain_select")
            .expect("run_one must end before render_drain_select")
            .0;
        let batch = source
            .split_once("async fn run_regen_batch(")
            .expect("batch regeneration helper must exist")
            .1
            .split_once("const NON_REGEN_ACTION_FILTER")
            .expect("batch regeneration helper must end before the phase whitelist")
            .0;
        let page_locks = source
            .split_once("async fn acquire_regen_page_locks(")
            .expect("page lock helper must exist")
            .1
            .split_once("async fn run_regen_batch(")
            .expect("page lock helper must end before batch regeneration")
            .0;

        assert!(run_one.contains("generation_root_lock"), "{run_one}");
        assert!(page_locks.contains("generation_root_lock"), "{page_locks}");
        assert!(
            run_one.find("lock().await") < run_one.find("delete_work(job).await"),
            "single-root lock must cover queue settlement"
        );
        assert!(
            batch.find("let (root_guards, busy_roots)")
                < batch.find("clear_regen_work_batch(&settlements).await"),
            "batch locks must cover queue settlement"
        );
        assert!(
            batch.find("clear_regen_work_batch(&settlements).await")
                < batch.find("drop(root_guards)"),
            "batch lock guards may only be dropped after the settlement path"
        );
    }

    /// 人工复活的三件事必须原子地发生在同一条语句里（spec §4.6.1）：
    /// `revision + 1`（作废旧收口令牌）、`attempts = 0`（重新进 drain 候选集）、
    /// 清 `last_error`。且它只 UPDATE 不 UPSERT——复活不是入队，表里没有的行
    /// 不能从这里凭空造出来。
    #[test]
    fn a_manual_retry_revives_in_one_atomic_statement() {
        let sql = render_retry_pending_unit(ModelWorkAction::RegenRoot, "24381/100677");
        assert!(
            sql.starts_with("UPDATE"),
            "复活不是入队，不得 UPSERT: {sql}"
        );
        assert!(sql.contains("revision = (revision?:0) + 1"), "{sql}");
        assert!(sql.contains("attempts = 0"), "{sql}");
        assert!(sql.contains("last_error = NONE"), "{sql}");
        assert!(sql.contains("status = 'pending'"), "{sql}");
        assert!(
            sql.contains("WHERE action = 'regen_root' AND target_refno = '24381/100677'"),
            "必须按 (action, target) 寻址既有行: {sql}"
        );
        assert!(sql.contains("RETURN AFTER"), "回执要带复活后的行: {sql}");
    }

    /// 收口失败不是生成失败（2026-07-30 审计 C2）。
    ///
    /// 批量生成成功之后 `clear_regen_work_batch` 挂掉，曾经的处置是给批里每个根
    /// `record_failure`（→ mark_failed → attempts + 1）：一条 flaky 的 DELETE 连撞
    /// [`MAX_ATTEMPTS`] 次，一整批**生成从没失败过**的健康根就全进死信——而死信只有
    /// 人工才能复活。正确口径与 `batch_worker` 的同构路径一致：行留在表里不动，
    /// 下一轮 drain 重跑幂等生成、再试一次收口。
    #[test]
    fn batch_settlement_failure_never_marks_generated_roots_failed() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_where(")
            .expect("drain_where 必须存在")
            .1
            .split_once("const NON_REGEN_ACTION_FILTER")
            .expect("drain_where 必须在阶段白名单之前结束")
            .0;
        // 收边用批量失败回退分支的注释当锚点（本文件是 CRLF，不能按 "\n...}" 找）。
        // 这段截出来的正是「生成成功、收口失败」那条 arm。
        let settlement_arm = body
            .split_once("match clear_regen_work_batch(&settlements).await")
            .expect("批量收口分支必须存在")
            .1
            .split_once("Err(error) => {")
            .expect("收口失败分支必须存在")
            .1
            .split_once("The per-root fallback")
            .expect("收口失败分支之后是批量失败回退分支")
            .0;
        // 按调用点形态（带左括号）断言，注释里提到这两个名字不算数。
        assert!(
            !settlement_arm.contains("record_failure(") && !settlement_arm.contains("mark_failed("),
            "收口失败分支不得动行状态（不涨 attempts、不写 failed）: {settlement_arm}"
        );
        assert!(
            settlement_arm.contains("failures.push"),
            "收口失败仍要进 drain 汇总，让这一轮如实报错: {settlement_arm}"
        );
    }

    /// 认领了却没有任何去向的根必须被点名、被计次（2026-08-27 现场）。
    ///
    /// 那天 1462 个根连着 118 页 `done=0/total=100`，每页都报 `succeeded`：生成不报错、
    /// 收口集合里没有它们、`record_failure` 一次都没调过，于是 `attempts` 永远是 0、
    /// `MAX_ATTEMPTS` 永远够不着、死信永远是空的，日志里一个字都没有。这条钉住三件事：
    /// 点名要带样本、计次要走 `record_failure`、话里要说清撞满就进死信。
    #[test]
    fn unechoed_roots_are_named_and_counted_towards_the_dead_letter_ceiling() {
        let source = include_str!("model_update_pending.rs");
        // 定义在文件里排在测试模块之前，所以 `split_once` 命中的是定义而不是这里的
        // 字面量——签名因此不许带泛型参数，否则这个锚点会滑到测试自己身上。
        let body = source
            .split_once("async fn settle_unechoed_roots(")
            .expect("未回报根的处置函数必须存在")
            .1
            .split_once("async fn run_regen_group(")
            .expect("它必须在 run_regen_group 之前结束")
            .0;

        assert!(
            body.contains("record_failure("),
            "确定性的收不掉必须计次，否则 MAX_ATTEMPTS 永远够不着: {body}"
        );
        assert!(
            body.contains("MODEL_DRAIN_DETAIL_SAMPLE_SIZE"),
            "点名必须带有界样本，只报个数等于没报: {body}"
        );
        assert!(
            body.contains("println!"),
            "这条路此前一行日志都没有，必须出声: {body}"
        );
        assert!(
            body.contains("MAX_ATTEMPTS"),
            "话要说清撞满几次进死信: {body}"
        );
    }

    /// 收口失败的根**不得**被当成「未回报」二次处置。
    ///
    /// 两条路的处置正相反：收口失败按审计 C2 不涨 attempts，未回报必须涨。要是
    /// `settlements` 没有先并进 `disposed`，一次 flaky 的 DELETE 就会让整批健康的根
    /// 从 C2 那条路掉进这条路、白白吃一次 attempts——C2 修的那个 bug 就原样回来了。
    #[test]
    fn settlement_failures_are_disposed_before_the_unechoed_sweep() {
        let source = include_str!("model_update_pending.rs");
        let group = source
            .split_once("async fn run_regen_group(")
            .expect("run_regen_group 必须存在")
            .1
            .split_once("\n// 三个阶段的 action 白名单")
            .expect("run_regen_group 必须在阶段白名单之前结束")
            .0;

        let folded = group
            .find("for (root, _) in &settlements")
            .expect("settlements 必须并进 disposed");
        let swept = group
            .find("settle_unechoed_roots(")
            .expect("未回报清扫必须发生");
        assert!(
            folded < swept,
            "settlements 要先并进 disposed，再做未回报清扫: {group}"
        );
        assert!(
            group.find("match clear_regen_work_batch(&settlements).await") < Some(folded),
            "并入发生在收口之后，Ok/Err 两条 arm 都要覆盖: {group}"
        );
    }

    // ---- 模型水位护栏（ADR-060）----

    #[tokio::test]
    async fn task_tail_rejects_empty_roots_without_authoritative_completion() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("task_tail")
            .use_db("empty_unverified")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=0; \
                  CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET \
                  dbnum=8000, end_sesno=12, status='done', room_released=true;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let proof = crate::data_interface::side_effect_pending::SideEffectCompensator::
            completed_file_round_on(&db, 8000, 12).await.unwrap();
        let result = complete_model_task_on(
            &db,
            8000,
            ModelTaskCompletion {
                target_sesno: 12,
                file_round: proof,
                inputs: None,
                model: crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                },
                roots: 0,
            },
        )
        .await;
        assert!(
            result.is_err(),
            "missing root inventory must not finish a task: {result:?}"
        );
    }

    /// 有根没成不是错误，是「被根拦住」这个现场（四个变体不许合并）；收据一笔不落。
    #[tokio::test]
    async fn task_tail_reports_unfinished_roots_as_held_not_as_an_error() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("task_tail")
            .use_db("held_by_roots")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=0; \
                  CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET \
                  dbnum=8000, end_sesno=12, status='done', room_released=true;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let proof = crate::data_interface::side_effect_pending::SideEffectCompensator::
            completed_file_round_on(&db, 8000, 12).await.unwrap();
        let outcome = complete_model_task_on(
            &db,
            8000,
            ModelTaskCompletion {
                target_sesno: 12,
                file_round: proof,
                inputs: None,
                model: crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: false,
                    dead_roots: 2,
                },
                roots: 5,
            },
        )
        .await
        .expect("unfinished roots are an outcome, not an error");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::HeldByRoots {
                total: 2,
                blockers: Vec::new(),
            }
        );
        let completion: Option<String> = db
            .query("RETURN incr_side_effect_pending:spatial_reconcile_8000_12.model_completion;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert!(completion.is_none(), "被根拦住时收据不落");
    }

    #[tokio::test]
    async fn task_tail_rejects_a_file_round_proof_for_another_target() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("task_tail").use_db("stage_gate").await.unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET kind='spatial_reconcile', \
             dbnum=8000, end_sesno=12, status='done', room_released=false;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let proof = crate::data_interface::side_effect_pending::SideEffectCompensator::
            mark_file_rooms_released_on(&db, 8000, 12)
            .await
            .unwrap();

        let error = complete_model_task_on(
            &db,
            8001,
            ModelTaskCompletion {
                target_sesno: 12,
                file_round: proof.clone(),
                inputs: None,
                model: crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                },
                roots: 0,
            },
        )
        .await
        .expect_err("another dbnum proof must not stamp");
        assert!(error.to_string().contains("dbnum"), "{error:#}");

        let error = complete_model_task_on(
            &db,
            8000,
            ModelTaskCompletion {
                target_sesno: 13,
                file_round: proof,
                inputs: None,
                model: crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                },
                roots: 0,
            },
        )
        .await
        .expect_err("another target proof must not stamp");
        assert!(error.to_string().contains("target"), "{error:#}");
    }

    /// 收尾不得再回查它已经知道的事（ADR-071 D3）。7997 实测那一问 5.5 s，
    /// 换来的只是任务手里已有的三件事实。
    #[test]
    fn the_task_tail_asks_its_own_units_instead_of_probing_gen_root() {
        // 模型水位那一段自 S5a 起住在 `model_watermark.rs`（本文件只再导出它）；
        // 钉的判据没变，换的只是读哪个文件。
        let source = include_str!("model_watermark.rs");
        let tail = source
            .split_once("pub(crate) async fn complete_model_task_on(")
            .expect("任务收尾必须存在")
            .1
            .split_once("\n/// 拦路根抽样上限")
            .expect("收尾必须在拦路根抽样常量之前结束")
            .0;
        assert!(
            !tail.contains("read_model_watermark_probe("),
            "收尾不得再跑探针: {tail}"
        );
        assert!(
            tail.contains("completion.model.all_roots_ok"),
            "根的成败要由任务自己的单元清单给出: {tail}"
        );
        assert!(
            !tail.contains("commit_task_end_on("),
            "收据这一步不盖水位——那是房间之后收尾那一笔的活: {tail}"
        );
    }

    /// 一条根都没有的空模型库，`roots == 0` 是任务自己数出来的，不是探针反推的。
    #[test]
    fn an_empty_root_inventory_comes_from_the_task_not_from_the_database() {
        let source = include_str!("model_watermark.rs");
        let tail = source
            .split_once("pub(crate) async fn complete_model_task_on(")
            .expect("任务收尾必须存在")
            .1
            .split_once("\n/// 拦路根抽样上限")
            .expect("收尾必须在拦路根抽样常量之前结束")
            .0;
        assert!(
            tail.contains("let empty = completion.roots == 0;"),
            "{tail}"
        );
        assert!(!tail.contains("probe.has_roots"), "{tail}");
        assert!(!tail.contains("probe.settled_floor"), "{tail}");
    }

    fn regen_group_body() -> &'static str {
        include_str!("model_update_pending.rs")
            .split_once("async fn run_regen_group(")
            .expect("run_regen_group 必须存在")
            .1
            .split_once("\n// 三个阶段的 action 白名单")
            .expect("run_regen_group 必须在阶段白名单之前结束")
            .0
    }

    /// Compatibility drain settles roots only; the enclosing Task owns the library stamp.
    #[test]
    fn the_compatibility_regen_drain_never_stamps_the_model_watermark() {
        let group = regen_group_body();
        assert!(group.contains("clear_regen_work_batch(&settlements)"));
        assert!(!group.contains("advance_model_watermark("));
        assert!(!group.contains("stamp_model_watermark_on("));
    }

    // 收口尾事务自 S5a 起住在 `increment_tail.rs`（本文件只再导出它）；钉的判据没变，
    // 换的只是读哪个文件。
    fn finalize_attempt_body() -> &'static str {
        include_str!("increment_tail.rs")
            .split_once("pub(crate) async fn finalize_attempt_on(")
            .expect("finalize_attempt_on 必须存在")
            .1
            .split_once("\n/// Establish a freshly parsed")
            .expect("finalize_attempt_on 必须在 finalize_baseline 的文档之前结束")
            .0
    }

    /// G1-bis（ADR-060 D5 的 2026-09-05 修订）：零 regen 的窗口靠**第二触发点**盖章。
    ///
    /// 它必须排在尾事务提交与凭证前移**之后**——前者是 M2 的前提（`applied_sesno` 先到 T），
    /// 后者是候选值能等于 T 的前提（未触到的根先前移，安定下限才到 T）；且只在没有
    /// `RegenRoot` 时跑——有的话被触到的根还停在旧凭证，探针只能读出旧值，白跑，
    /// 收口之后 cohort 那一处会盖。反过来（先盖再前移）就是拿旧下限盖一个没意义的章。
    #[test]
    fn finalize_attempt_advances_root_credentials_but_not_the_library_watermark() {
        let body = finalize_attempt_body();
        let committed = body.find("COMMIT TRANSACTION").expect("尾事务必须存在");
        let advanced = body
            .find("advance_root_credentials_on(")
            .expect("凭证前移必须存在");
        assert!(committed < advanced);
        assert!(!body.contains("stamp_model_watermark_on("));
    }

    /// 同一格在启动退化窗口上：一项工作都没排、凭证刚前移的库，`reconcile_startup_window`
    /// 自己盖章——启动对账排在开窗之前、这一轮已经跑过，不盖就要等再下一次启动（N2）。
    #[test]
    fn a_startup_window_never_stamps_the_model_watermark_itself() {
        let body = include_str!("window_root_plan.rs")
            .split_once("pub async fn reconcile_startup_window(")
            .expect("reconcile_startup_window 必须存在")
            .1
            .split_once("#[cfg(test)]")
            .expect("测试模块在函数之后")
            .0;
        let advanced = body
            .find("advance_root_credentials(")
            .expect("启动窗口的凭证前移必须存在");
        assert!(advanced > 0);
        assert!(!body.contains("advance_model_watermark("));
        assert!(!body.contains("stamp_model_watermark_on("));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn delete_cleanup_completion_leaves_the_stamp_to_the_task_tail() {
        let db = reconcile_fixture(
            "delete_cleanup_watermark",
            "CREATE gen_root:24381_1 SET dbnum=8000, status='Generated', publication_status='ready', \
             desired_revision=1, published_revision=1, source_end_sesno=20; \
             CREATE gen_root:24381_2 SET dbnum=8000, status='Generated', publication_status='ready', \
             desired_revision=1, published_revision=1, source_end_sesno=20; \
             CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=20, model_sesno=20;",
        ).await;
        let time = "2026-09-06T12:00:00+00:00";
        let plan = ModelUpdatePlan {
            credential_advance: vec!["24381/1".into()],
            work_items: vec![
                ModelWorkItem {
                    dbnum: 8000,
                    db_type: "DESI".into(),
                    source_end_sesno: 26,
                    action: ModelWorkAction::DeleteCleanup,
                    target_refno: "24381/2".into(),
                    noun: "BRAN".into(),
                },
                ModelWorkItem {
                    dbnum: 8000,
                    db_type: "DESI".into(),
                    source_end_sesno: 26,
                    action: ModelWorkAction::DeleteCleanup,
                    target_refno: "24381/3".into(),
                    noun: "BRAN".into(),
                },
            ],
            ..Default::default()
        };
        finalize_attempt_on(&db, 8000, 26, Some(time), &plan, &[])
            .await
            .unwrap();
        assert_eq!(
            stamped_watermark(&db).await.0,
            20,
            "the old root still limits the floor until cleanup executes"
        );
        let jobs: Vec<PendingModelWork> = db
            .query("SELECT * FROM model_update_pending ORDER BY target_refno;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(jobs.len(), 2);
        // Completion receives control only after the model deletion succeeds.
        db.query("DELETE gen_root:24381_2;")
            .await
            .unwrap()
            .check()
            .unwrap();
        delete_work_on(&db, &jobs[0]).await.unwrap();
        assert_eq!(
            stamped_watermark(&db).await.0,
            20,
            "another cleanup is still pending"
        );
        delete_work_on(&db, &jobs[1]).await.unwrap();
        assert_eq!(
            stamped_watermark(&db).await,
            (20, None),
            "delete cleanup settles root work but cannot certify publication and rooms"
        );
    }

    /// 真引擎上走一遍 G1：一窗 20→26 只前移凭证、零 regen，收口之后两枚水位都在 26，
    /// 时刻跟着号一起贴上——不必等下次启动。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_zero_regen_window_leaves_the_library_watermark_to_the_task_tail() {
        let db = reconcile_fixture(
            "model_watermark_zero_regen_window",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 20, model_sesno = 20;",
        )
        .await;
        let time = "2026-09-05T01:00:00+08:00";
        let plan = ModelUpdatePlan {
            credential_advance: vec!["24381/1".into(), "24381/2".into()],
            ..Default::default()
        };
        finalize_attempt_on(&db, 8000, 26, Some(time), &plan, &[])
            .await
            .expect("finalize");
        assert_eq!(
            stamped_watermark(&db).await,
            (20, None),
            "根凭证前移不能替 Task 证明发布、空间与房间都已完成"
        );
    }

    /// 反例：有 `RegenRoot` 的窗口不在收口处盖——被触到的根还停在 20，此刻盖只能盖出 20；
    /// 它的章由 cohort 收口之后那一处负责。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_window_with_regen_work_leaves_the_stamp_to_the_cohort_settlement() {
        let db = reconcile_fixture(
            "model_watermark_regen_window_defers",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 20, model_sesno = 20;",
        )
        .await;
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: 8000,
                db_type: "DESI".into(),
                source_end_sesno: 26,
                action: ModelWorkAction::RegenRoot,
                target_refno: "24381/1".into(),
                noun: "BRAN".into(),
            }],
            credential_advance: vec!["24381/2".into()],
            ..Default::default()
        };
        finalize_attempt_on(&db, 8000, 26, Some("2026-09-05T01:00:00+08:00"), &plan, &[])
            .await
            .expect("finalize");
        assert_eq!(
            stamped_watermark(&db).await,
            (20, None),
            "根 24381/1 还没重生成，收口处不盖；等 cohort 收口"
        );
    }

    /// G5 / M7：这两枚水位是仪表，不是闸门。
    ///
    /// 用户 2026-09-04 原话：「我的目的就是要保证数据解析的内容保存和模型生成的保存
    /// 能够异步。」装表不许改电路——数据写回不看模型水位，模型生成不等数据水位。
    /// 一旦哪一侧开始读对面那一枚，两条链就重新耦合上了，而且不会有任何报错。
    #[test]
    fn the_two_watermarks_are_instruments_not_gates() {
        let persist = include_str!("increment_pipeline.rs");
        assert!(
            !persist.contains("model_sesno"),
            "数据写回路径不得读模型水位（M7）"
        );

        let generation = include_str!("../fast_model/e3d_model_service.rs");
        let code_touching_the_data_watermark = generation
            .lines()
            .filter(|line| line.contains("applied_sesno"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>();
        assert!(
            code_touching_the_data_watermark.is_empty(),
            "模型生成不得以数据水位为前置（M7 / ADR-054）: {code_touching_the_data_watermark:?}"
        );

        // 盖章那一段读 `applied_sesno` 是对的（M2 封顶），但它在收口之后、不在生成路上。
        let group = regen_group_body();
        assert!(
            !group.contains("applied_sesno"),
            "生成这一组的过程本身不得看数据水位: {group}"
        );
    }

    /// G3（ADR-060 D5）：不认领数据窗口的工作，一律不触发盖章。
    ///
    /// 房间任务与反向级联派生根如实写 `dbnum = 0` / `source_end_sesno = 0`
    /// （跨库会话号不可比），人工强制重试同样不认领。让它们推动库级的追平承诺，
    /// 会造出「半个库追平了」的假象。判据与 [`publication_cohort`] 是同一个，
    /// 不另立一套。
    #[test]
    fn only_work_that_claims_a_data_window_can_move_the_model_watermark() {
        let job = |dbnum, sesno, action, time: Option<&str>| PendingModelWork {
            dbnum,
            db_type: "DESI".into(),
            source_end_sesno: sesno,
            source_end_sesno_time: time.map(str::to_string),
            action,
            target_refno: "24381/1".into(),
            noun: "BRAN".into(),
            status: "running".into(),
            attempts: 0,
            last_error: None,
            revision: 1,
            required_panels: Vec::new(),
        };

        let ignored = vec![
            // 房间任务：两个字段都不认领。
            job(0, 0, ModelWorkAction::RoomRecalcElement, None),
            // 人工强制重试 / 派生根：认领不了会话号。
            job(8000, 0, ModelWorkAction::RegenRoot, None),
            // CATA 播种那一类：认领不了库。
            job(0, 42, ModelWorkAction::RegenRoot, None),
            // 便宜路径不是发布同期的成员。
            job(8000, 42, ModelWorkAction::Transform, None),
        ];
        assert!(
            settled_window_cohorts(&ignored, &[]).is_empty(),
            "不认领数据窗口的工作不得触发模型水位盖章"
        );

        let claimed = vec![
            job(
                8000,
                25,
                ModelWorkAction::RegenRoot,
                Some("2026-09-02T10:00:00+08:00"),
            ),
            job(
                8000,
                26,
                ModelWorkAction::RegenRoot,
                Some("2026-09-04T09:00:00+08:00"),
            ),
        ];
        assert_eq!(
            settled_window_cohorts(&claimed, &ignored),
            vec![(8000, 26, Some("2026-09-04T09:00:00+08:00".to_string()))],
            "同一库取最高那一窗，时刻必须跟着那一条走——号和时刻永远配对"
        );
    }

    /// G6：时刻跟着序号那条单调条件走，且必须排在覆盖之前。
    ///
    /// 与 [`render_watermark_advance`] 一模一样的坑：`SET` 顺序求值，时刻子句排在
    /// `model_sesno` 被 `math::max` 覆盖之后，读到的就是刚写完的新值，条件恒真，
    /// 一个落后的批次会把时刻退回去。
    #[test]
    fn the_model_watermark_time_follows_the_same_monotonic_clause_as_the_number() {
        let sql =
            render_model_watermark_advance(8000, 26, Some("2026-09-04T09:00:00+08:00"), 2, 26);
        assert!(
            sql.contains("model_sesno = math::max([model_sesno?:0, 26])"),
            "水位必须单调: {sql}"
        );
        let time_clause = sql
            .find("model_sesno_time = IF 26 >= (model_sesno ?: 0)")
            .expect("时刻必须带单调条件");
        let overwrite = sql.find("model_sesno = math::max").expect("覆盖必须存在");
        assert!(
            time_clause < overwrite,
            "时刻子句读的是旧值，必须排在覆盖之前: {sql}"
        );
        assert!(
            sql.contains("WHERE (applied_sesno ?: 0) >= 26"),
            "M2 的第二道闸必须在语句里: {sql}"
        );
        assert!(
            sql.contains("model_dead_roots = 2") && sql.contains("model_roots_observed_at = 26"),
            "四格计数与它的观察点同一句落库: {sql}"
        );

        let without_time = render_model_watermark_advance(8000, 26, None, 0, 26);
        assert!(
            !without_time.contains("model_sesno_time"),
            "读不到时刻就整条子句都不写，不许写 NONE 把旧值抹掉: {without_time}"
        );
    }

    /// G2 / M2：模型跑到数据前面时只是不盖章，绝不改水位。
    ///
    /// 这一格在两条链真异步之后是常态（ADR-060 M7）：模型按文件最新生成，数据窗口
    /// 还没提交到那一版。承诺的是「到 N 为止一致」，数据没到 N 这句话就不成立。
    #[tokio::test]
    async fn the_model_watermark_never_outruns_the_data_watermark() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_watermark_ahead")
            .await
            .unwrap();
        db.query(
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 30;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 20;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let outcome = advance_model_watermark_on(&db, 8000, 30, Some("2026-09-04T09:00:00+08:00"))
            .await
            .expect("probe runs");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::AheadOfData {
                candidate: 30,
                applied: 20
            }
        );

        let mut response = db
            .query("RETURN dbnum_watermark:8000.model_sesno;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let stamped: Option<i32> = response.take(0).unwrap();
        assert_eq!(stamped, None, "数据没到那一版，一个字都不许写");
    }

    /// ADR-065：失败根必须把严格完成水位钉在上一版。
    ///
    /// `http_422` 是「这个 noun 与数据永远解析不出合法生成根」的永久判定，死信是
    /// 重试撞满 [`MAX_ATTEMPTS`]。两者都永远不会变成安定行——算进拦路，这个库的灯
    /// 从此永红，而且现场看不出是被哪一个根卡的。它们进「已放弃」那一格，照旧露出来。
    #[tokio::test]
    async fn an_abandoned_root_holds_the_strict_model_watermark() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_watermark_dead")
            .await
            .unwrap();
        db.query(format!(
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'http_422';\
             CREATE gen_root:24381_3 SET dbnum = 8000, status = 'failed', source_end_sesno = 0;\
             CREATE {TABLE}:dead SET dbnum = 8000, db_type = 'DESI', source_end_sesno = 26, \
             action = 'regen_root', target_refno = '24381/3', noun = 'BRAN', \
             status = 'failed', attempts = {MAX_ATTEMPTS}, revision = 1;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 0, \
             model_sesno_time = NONE, model_dead_roots = 0;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let outcome = advance_model_watermark_on(&db, 8000, 26, Some("2026-09-04T09:00:00+08:00"))
            .await
            .expect("probe runs");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::HeldByRoots {
                total: 2,
                blockers: Vec::new(),
            },
            "已放弃根仍未生成完整，严格完成水位不得越过"
        );

        #[derive(serde::Deserialize)]
        struct Stamped {
            model_sesno: i32,
            model_sesno_time: Option<String>,
            model_dead_roots: u32,
        }
        let mut response = db
            .query(
                "SELECT model_sesno, model_sesno_time, model_dead_roots \
                 FROM ONLY dbnum_watermark:8000;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let row: Option<Stamped> = response.take(0).unwrap();
        let row = row.expect("水位行还在");
        assert_eq!(row.model_sesno, 0);
        assert_eq!(row.model_sesno_time, None);
        assert_eq!(row.model_dead_roots, 0, "阻挡路径不伪造一次水位写入");
    }

    /// 还在赶的根（既不是死信也不是 `http_422`）照旧拦住水位——G4 放宽的只有已放弃那一格。
    #[tokio::test]
    async fn a_root_that_is_still_catching_up_holds_the_model_watermark() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_watermark_held")
            .await
            .unwrap();
        db.query(
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'failed', source_end_sesno = 0;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let outcome = advance_model_watermark_on(&db, 8000, 26, None)
            .await
            .expect("probe runs");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::HeldByRoots {
                total: 1,
                blockers: vec!["24381_2".to_string()]
            }
        );
    }

    /// A root conservatively marked stale after an uncertain publication ACK
    /// is not completion evidence, even when its old geometry and session
    /// credential are still present.  The startup/model-only recovery task must
    /// regenerate or reconcile it before either the model or room watermark can
    /// advance.
    #[tokio::test]
    async fn a_stale_publication_receipt_holds_the_model_watermark() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_watermark_stale_receipt")
            .await
            .unwrap();
        db.query(
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', \
               publication_status = 'ready', desired_revision = 1, published_revision = 1, \
               source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'Generated', \
               publication_status = 'stale', desired_revision = 2, published_revision = 1, \
               source_end_sesno = 20;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 20;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let outcome = advance_model_watermark_on(&db, 8000, 26, None)
            .await
            .expect("probe runs");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::HeldByRoots {
                total: 1,
                blockers: vec!["24381_2".to_string()],
            }
        );
    }

    /// 承诺的上界是**数据水位**，不是安定凭证的下限（共识 d-427，2026-09-10 翻面）。
    ///
    /// 翻面之前这里钉的是 `min(根凭证)`。那条定义要求「抬一个库级承诺必须先把每一根
    /// 凭证各抬一遍」，担这活的 ADR-056 P2-2 凭证前移退役之后，min 就永远停在最老那根：
    /// 现场 7997 数据到了 168、六行凭证还是 165，`model_sesno` 死在 165。
    ///
    /// 换来的保守不在这里，在 `blockers` 那道闸——它直接数未安定根，不经过凭证。
    /// 所以本用例两拍：**都安定** → 承诺到数据那一版；**有一根没安定** → 一步不动。
    #[tokio::test]
    async fn the_stamp_is_the_data_watermark_once_no_root_is_still_chasing() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_watermark_floor")
            .await
            .unwrap();
        db.query(
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'Generated', source_end_sesno = 20;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let outcome = advance_model_watermark_on(&db, 8000, 26, Some("2026-09-04T09:00:00+08:00"))
            .await
            .expect("probe runs");
        assert_eq!(
            outcome,
            ModelWatermarkOutcome::Advanced { sesno: 26 },
            "两根都安定，整库就承诺追平到数据这一版——凭证停在 20 的那根不再拖住它"
        );

        let mut response = db
            .query("RETURN dbnum_watermark:8000.model_sesno_time;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let time: Option<String> = response.take(0).unwrap();
        assert_eq!(
            time,
            Some("2026-09-04T09:00:00+08:00".to_string()),
            "水位落在 26、时刻描述的也是 26，号对上了才贴"
        );

        // 第二拍：把其中一根打回未安定，那道闸必须替 min 接下保守这一半。
        db.query("UPDATE gen_root:24381_2 SET publication_status = 'stale';")
            .await
            .unwrap()
            .check()
            .unwrap();
        let held = advance_model_watermark_on(&db, 8000, 26, Some("2026-09-04T09:00:00+08:00"))
            .await
            .expect("probe runs");
        assert!(
            matches!(held, ModelWatermarkOutcome::HeldByRoots { total: 1, .. }),
            "还有根在赶就一步不动，得到的却是 {held:?}"
        );
    }

    // ---- 双向对账护栏（ADR-060 D6/D7）----

    async fn reconcile_fixture(name: &'static str, seed: &str) -> surrealdb::Surreal<Any> {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db(name).await.unwrap();
        db.query(seed.to_string()).await.unwrap().check().unwrap();
        db
    }

    async fn stamped_watermark(db: &Surreal<Any>) -> (i32, Option<String>) {
        #[derive(serde::Deserialize)]
        struct Row {
            #[serde(default)]
            model_sesno: i32,
            #[serde(default)]
            model_sesno_time: Option<String>,
        }
        let mut response = db
            .query(
                "SELECT model_sesno ?: 0 AS model_sesno, model_sesno_time \
                 FROM ONLY dbnum_watermark:8000;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let row: Option<Row> = response.take(0).unwrap();
        let row = row.expect("水位行还在");
        (row.model_sesno, row.model_sesno_time)
    }

    /// 对账写的是 `=` 不是 `math::max`，而且改了就把时刻抹掉。
    ///
    /// 合并成 `math::max` 的话，「降回去」那一半静默失效——而失效的表现是一枚永远
    /// 偏高的水位，没有任何东西会报错。时刻描述的是**旧的**那个号，升降都对不上。
    #[test]
    fn the_reconcile_writes_absolutely_and_drops_the_stale_timestamp() {
        let sql = render_model_watermark_reconcile(8000, 20, 2);
        assert!(
            sql.contains("model_sesno = 20") && !sql.contains("math::max"),
            "对账必须能往下写，不许套单调: {sql}"
        );
        assert!(
            sql.contains("model_sesno_time = NONE"),
            "号变了，行上那个时刻描述的就是别人，必须抹掉: {sql}"
        );
        assert!(
            !sql.contains("WHERE"),
            "对账是更正，不该再被应用水位的守卫挡住——支撑值本身已经被它封过顶: {sql}"
        );
    }

    /// D6 下调：水位比证据高就降回去。
    #[tokio::test]
    async fn the_reconcile_lowers_a_watermark_that_lost_its_backing() {
        let db = reconcile_fixture(
            "model_watermark_lower",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'Generated', source_end_sesno = 20;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 26, \
             model_sesno_time = '2026-09-04T09:00:00+08:00';",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::LoweredUnbacked { from: 26, to: 20 }
        );
        assert_eq!(stamped_watermark(&db).await, (20, None));
    }

    /// 普通对账发现凭证高于完成水位时只报告，不代替 Task 补章。
    #[tokio::test]
    async fn the_reconcile_reports_but_does_not_raise_a_missed_stamp() {
        let db = reconcile_fixture(
            "model_watermark_raise",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 0;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::AwaitingTaskCompletion {
                recorded: 0,
                supported: 26
            }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    /// 上调同样受 M2 封顶：证据再高，也不许越过数据水位。
    #[tokio::test]
    async fn the_reconcile_still_caps_at_the_data_watermark() {
        let db = reconcile_fixture(
            "model_watermark_cap",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 40;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 0;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::AwaitingTaskCompletion {
                recorded: 0,
                supported: 26
            },
            "模型追到 40 了，但只承诺到数据到过的那一版"
        );
    }

    /// 兼容历史库也必须经过恢复 Task，普通对账不再凭投影存在直接上调。
    #[tokio::test]
    async fn legacy_inst_projection_waits_for_a_completion_task() {
        let db = reconcile_fixture(
            "legacy_inst_model_watermark",
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 26;\
             CREATE inst_relate:24381_1 SET in = pe:24381_1;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::AwaitingTaskCompletion {
                recorded: 0,
                supported: 26
            }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    #[tokio::test]
    async fn legacy_tubi_projection_with_dbnum_waits_for_a_completion_task() {
        let db = reconcile_fixture(
            "legacy_tubi_model_watermark",
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 26;\
             CREATE tubi_relate:24381_1 SET dbnum = 8000;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::AwaitingTaskCompletion {
                recorded: 0,
                supported: 26
            }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    /// 兼容接管（ADR-060 D10）：`dbnum_state` 的播种把模型水位填成数据水位之后，对账必须
    /// 认它——旧管线的投影就是 `supported()` 的支撑，不下调、不清零，也不再是恢复 Task 的对象。
    #[tokio::test]
    async fn a_seeded_legacy_projection_library_reconciles_as_aligned_and_is_adopted() {
        let db = reconcile_fixture(
            "legacy_adopted_model_watermark",
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 26;\
             CREATE inst_relate:24381_1 SET in = pe:24381_1, dbnum = 8000;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, \
               model_sesno = 26, room_sesno = 26;",
        )
        .await;

        let probe = read_model_watermark_probe(&db, 8000).await.unwrap();
        assert!(probe.legacy_adopted(), "{probe:?}");
        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::Aligned { sesno: 26 }
        );
        assert_eq!(stamped_watermark(&db).await, (26, None));
    }

    /// 接管只认「无根 + 有属性 + 有投影 + 水位已追平」这一格；差任何一条都不是接管——
    /// 差了就回到各自原有的处置（恢复 Task / 幽灵清零 / 正常窗口）。
    #[test]
    fn legacy_adoption_requires_every_one_of_its_four_conditions() {
        let adopted = ModelWatermarkProbe {
            blockers: Vec::new(),
            abandoned: 0,
            settled_floor: None,
            applied_sesno: 26,
            recorded: 26,
            has_roots: false,
            has_attributes: true,
            has_model_projection: true,
            completed_sesno: None,
            completion_present: false,
        };
        assert!(adopted.legacy_adopted());
        // 新管线已经写过根：走凭证那一套，不是接管。
        assert!(
            !ModelWatermarkProbe {
                has_roots: true,
                ..adopted.clone()
            }
            .legacy_adopted()
        );
        // 没有属性：数据面都不在，无从接管。
        assert!(
            !ModelWatermarkProbe {
                has_attributes: false,
                ..adopted.clone()
            }
            .legacy_adopted()
        );
        // 没有投影：数据在模型无，播种记的是 0，照旧由模型面生成。
        assert!(
            !ModelWatermarkProbe {
                has_model_projection: false,
                ..adopted.clone()
            }
            .legacy_adopted()
        );
        // 水位没播上（0）或数据已经前移：都不是接管，前者等 Task，后者照常开窗。
        assert!(
            !ModelWatermarkProbe {
                recorded: 0,
                ..adopted.clone()
            }
            .legacy_adopted()
        );
        assert!(
            !ModelWatermarkProbe {
                applied_sesno: 30,
                ..adopted.clone()
            }
            .legacy_adopted()
        );
    }

    #[tokio::test]
    async fn attributes_without_a_model_projection_do_not_seed_model_watermark() {
        let db = reconcile_fixture(
            "attributes_only_model_watermark",
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 26;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::Aligned { sesno: 0 }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    #[tokio::test]
    async fn model_projection_without_attributes_does_not_seed_model_watermark() {
        let db = reconcile_fixture(
            "model_only_model_watermark",
            "CREATE inst_relate:24381_1 SET dbnum = 8000;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::Aligned { sesno: 0 }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    /// D7 幽灵模型水位：水位说追平过，`gen_root` 与模型投影却都没有。
    ///
    /// 它与「失去支撑」共用同一个下调动作，但**措辞必须分开**：这一种意味着模型面整个
    /// 没了（按需重新生成即可），那一种意味着水位记高了。混成一句，现场就分不出该不该
    /// 去查模型产物。
    #[tokio::test]
    async fn a_ghost_model_watermark_is_cleared_and_named() {
        let db = reconcile_fixture(
            "model_watermark_ghost",
            "CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 26, \
             model_sesno_time = '2026-09-04T09:00:00+08:00';",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::ClearedGhost { from: 26 }
        );
        assert_eq!(stamped_watermark(&db).await, (0, None));
    }

    /// ADR-065 在启动对账同样成立：失败根使既有追平水位失去完整性证据。
    ///
    /// 这一格比热路径那一格更凶——热路径漏一次只是晚点盖章，对账要是把已放弃根算进
    /// 「还在赶」，`supported` 恒为 0，**每一次启动都会把刚追平的水位抹掉**，而现场看到
    /// 的只是一枚永远为 0 的水位。
    #[tokio::test]
    async fn an_abandoned_root_lowers_unbacked_watermark_at_reconcile() {
        let db = reconcile_fixture(
            "model_watermark_reconcile_dead",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'http_422';\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 26, \
             model_sesno_time = '2026-09-04T09:00:00+08:00';",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::LoweredUnbacked { from: 26, to: 0 },
            "失败根没有完整模型证据，对账必须撤销错误水位"
        );
        let (sesno, time) = stamped_watermark(&db).await;
        assert_eq!(sesno, 0);
        assert_eq!(time, None);
    }

    /// 还在赶的根让支撑值归 0——对账降水位是对的：这个库整体没追平到任何一版。
    #[tokio::test]
    async fn a_root_still_catching_up_makes_the_reconcile_zero_the_watermark() {
        let db = reconcile_fixture(
            "model_watermark_reconcile_held",
            "CREATE gen_root:24381_1 SET dbnum = 8000, status = 'Generated', source_end_sesno = 26;\
             CREATE gen_root:24381_2 SET dbnum = 8000, status = 'failed', source_end_sesno = 0;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26, model_sesno = 26;",
        )
        .await;

        assert_eq!(
            reconcile_model_watermark_on(&db, 8000).await.unwrap(),
            ModelWatermarkReconcile::LoweredUnbacked { from: 26, to: 0 }
        );
    }

    /// 生成器的回报与队列行必须先归一到同一个根键再比。
    ///
    /// 这条就是 2026-08-27 现场的成因：队列存 E3D 的 `24381/100817`，而报告那一串经
    /// `RefnoEnum` 走了一趟、`Display` 打成 `24381_100817`，字符串直接比一次都不会
    /// 相等。整页根既进不了收口集合也配不上失败名单，队列行原样留着被无限重认领，
    /// 而 `attempts` 一次都不涨。改回直接比字符串即红。
    #[test]
    fn the_generator_report_and_the_queue_row_agree_on_one_root_key() {
        assert_eq!(
            root_identity_key("24381/100817"),
            root_identity_key("24381_100817"),
            "同一个根的两种拼法必须归一"
        );
        assert_eq!(
            root_identity_key("=24381/100817"),
            root_identity_key("24381_100817"),
            "带 = 前缀的 RefU64 写法也是同一个根"
        );
        assert_ne!(
            root_identity_key("24381/100817"),
            root_identity_key("24381/100818"),
            "不同的根不许被归一成同一个"
        );
        // 解析不出来的原样留着。`unwrap_or_default()` 会把所有坏值折成同一个零值，
        // 两个不同的坏根于是被认成同一个——那比不归一更糟。
        assert_eq!(root_identity_key("not-a-refno"), "not-a-refno");
        assert_ne!(root_identity_key("bad-a"), root_identity_key("bad-b"));

        // 调用点守卫：查 `completed` / `disposed` 之前必须过这一道。少一处就是少一批
        // 收不掉的根，而它安静得跟正常跑完一模一样。
        let source = include_str!("model_update_pending.rs");
        let group = source
            .split_once("async fn run_regen_group(")
            .expect("run_regen_group 必须存在")
            .1
            .split_once("\n// 三个阶段的 action 白名单")
            .expect("run_regen_group 必须在阶段白名单之前结束")
            .0;
        assert!(
            !group.contains("completed.contains(&job.target_refno)"),
            "队列行的写法不能直接拿去查生成器的回报: {group}"
        );
        assert!(
            !group.contains("disposed.insert(job.target_refno.clone())"),
            "进 disposed 的也得是身份键: {group}"
        );
    }

    /// 页级停滞的判据：四个条件缺一不可。
    #[test]
    fn page_starvation_needs_a_claimed_page_zero_settled_and_a_frozen_backlog() {
        // 现场形态：认领 100、收口 0、没有根锁挡着、待办 1462 一动不动。
        assert!(page_was_starved(100, 0, 0, 1462, 1462));
        // 收掉了一些 → 不是停滞。
        assert!(!page_was_starved(100, 16, 0, 1462, 1446));
        // 待办变了（收掉几个又排进来几个）→ 队列在动，不是停滞。
        assert!(!page_was_starved(100, 0, 0, 1462, 1400));
        // 上一页压根没认领到东西（首页、或队列本来就空）→ 无从判断，不算。
        assert!(!page_was_starved(0, 0, 0, 0, 0));
    }

    /// 整页被根锁挡住是让位，不是停滞——而它与停滞在前三个数上逐字段同形。
    ///
    /// `settle_deferred_for_lock` 早退时 telemetry 停在「认领 N / 收口 0 / 待办不变」，
    /// 光靠那三个数分不出来。specs/033 的前提就是单个布尔能跑几分钟：队列里只剩
    /// 一两个根、恰好都被按需生成占着时，三页 80 秒轻松跨过去，`/health` 就会为一次
    /// 正常让位报 degraded。去掉 deferred 那一条判据，这里立刻转红。
    #[test]
    fn a_page_fully_deferred_by_root_locks_is_yielding_not_starving() {
        assert!(
            !page_was_starved(2, 0, 2, 1462, 1462),
            "整页 lock-busy 是让位：行没动过是对的，不该计进停滞"
        );
        assert!(
            !page_was_starved(100, 0, 1, 1462, 1462),
            "只要有根被锁挡着，这一页就没资格被判成「收不掉」"
        );
    }

    /// 队列见底必须清零，否则停滞告警清不掉。
    ///
    /// 计数只在 `DrainPageProgress::claim` 里结算，而 drain 取到零行就直接返回、
    /// 再也不认领：最后一页若被计为停滞，`/health` 会 degraded 到进程重启。这条路
    /// 由新加的两件事自己铺出来——未回报的根撞满 `MAX_ATTEMPTS` 进死信之后队列正好
    /// 会空。纯函数钉不住「有没有人在那条早退路径上清」，按仓内先例用源码断言。
    #[test]
    fn an_empty_queue_clears_the_starvation_counter_before_returning() {
        let source = include_str!("model_update_pending.rs");
        // 早退分支排在测试模块之前，`split_once` 命中的是它而不是这里的字面量。
        let early_return = source
            .split_once("if jobs.is_empty() {")
            .expect("取到零行的早退分支必须在")
            .1
            .split_once("ModelDrainDisposition::Completed { done: 0 }")
            .expect("它以 Completed{done:0} 收尾")
            .0;
        assert!(
            early_return.contains("clear_page_starvation()"),
            "队列见底要清零，否则告警冻在最后一页上: {early_return}"
        );
    }

    /// 停滞阈值要够长到躲开一次寻常的锁竞争，又够短到不至于烧掉半小时。
    #[test]
    fn the_starvation_threshold_stays_in_the_useful_band() {
        assert!(
            (2..=10).contains(&MODEL_DRAIN_STARVED_PAGES_ALERT),
            "3 页约 80 秒；调到 1 会被一次让位误触，调到几十页就又回到人肉发现了"
        );
    }

    #[test]
    fn settlement_only_mutates_the_queue_revision_that_was_executed() {
        let work = PendingModelWork {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            source_end_sesno_time: Some("2026-08-05T18:24:00+08:00".into()),
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            revision: 7,
            required_panels: Vec::new(),
        };
        let item = ModelWorkItem {
            dbnum: work.dbnum,
            db_type: work.db_type.clone(),
            source_end_sesno: work.source_end_sesno,
            action: work.action,
            target_refno: work.target_refno.clone(),
            noun: work.noun.clone(),
        };

        assert!(
            render_upsert_no_time(&item)
                .contains("revision = IF $previous_revision = NONE OR 42 > ($previous_target?:-1)"),
            "only a new immutable target may create a settlement revision"
        );
        let expected = "WHERE action = 'regen_root' AND target_refno = '16777216/5' \
                        AND (revision?:0) = 7";
        assert!(
            render_delete_work(&work).contains(expected),
            "old success must not delete a newer trigger: {}",
            render_delete_work(&work)
        );
        assert!(
            render_mark_failed(&work, "boom").contains(expected),
            "old failure must not overwrite a newer trigger: {}",
            render_mark_failed(&work, "boom")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn regen_root_target_is_idempotent_and_stale_completion_cannot_publish_it() {
        use surrealdb::engine::any::connect;

        #[derive(Debug, Deserialize)]
        struct PendingRevision {
            revision: u64,
            cohort_id: String,
        }
        #[derive(Debug, Deserialize)]
        struct RootRevision {
            desired_revision: u64,
            #[serde(default)]
            published_revision: u64,
            publication_status: String,
        }

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("root_revision_v2")
            .await
            .expect("select fixture db");
        let item = |sesno: i32| ModelWorkItem {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: sesno,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
        };

        db.query(format!(
            "{}\n{}",
            render_upsert_no_time(&item(42)),
            render_upsert_no_time(&item(42))
        ))
        .await
        .expect("duplicate enqueue transport")
        .check()
        .expect("duplicate enqueue statements");
        let mut response = db
            .query("SELECT revision, cohort_id FROM model_update_pending WHERE target_refno = '16777216/5';")
            .await
            .expect("read pending")
            .check()
            .expect("read pending statement");
        let rows: Vec<PendingRevision> = response.take(0).expect("decode pending");
        assert_eq!(rows[0].revision, 1, "same target must be idempotent");
        assert_eq!(rows[0].cohort_id, "8191:42");

        db.query(render_complete_regen_revision("16777216/5", 1))
            .await
            .expect("publish revision one")
            .check()
            .expect("publish revision one statements");
        db.query(render_upsert_no_time(&item(43)))
            .await
            .expect("enqueue next target")
            .check()
            .expect("enqueue next target statements");
        db.query(render_complete_regen_revision("16777216/5", 1))
            .await
            .expect("stale completion transport")
            .check()
            .expect("stale completion statement");

        let mut response = db
            .query("SELECT desired_revision, published_revision, publication_status FROM ONLY type::thing('gen_root', '16777216_5');")
            .await
            .expect("read root state")
            .check()
            .expect("read root state statement");
        let root: Option<RootRevision> = response.take(0).expect("decode root state");
        let root = root.expect("root state exists");
        assert_eq!((root.desired_revision, root.published_revision), (2, 1));
        assert_eq!(root.publication_status, "stale");

        db.query(render_mark_failed_revision(
            ModelWorkAction::RegenRoot,
            "16777216/5",
            2,
            "candidate failed",
        ))
        .await
        .expect("mark failed transport")
        .check()
        .expect("mark failed statements");
        let mut response = db
            .query("SELECT publication_status FROM ONLY type::thing('gen_root', '16777216_5');")
            .await
            .expect("read stale state")
            .check()
            .expect("read stale state statement");
        #[derive(Deserialize)]
        struct StatusRow {
            publication_status: String,
        }
        let row: Option<StatusRow> = response.take(0).expect("decode stale state");
        assert_eq!(row.expect("stale state exists").publication_status, "stale");
    }

    /// 收口不能靠「再算一遍 record id」。存量表里同一个根还留着旧格式的行
    /// （`{dbnum}_regen_root_…`），按 id 寻址会命中零行——任务清不掉、每一轮重跑一次
    /// 完整生成，而日志里一切正常。谓词寻址只依赖行里实际存着的字段。
    /// 入队方只准用一种拼法写 `target_refno`。
    ///
    /// 同一个根有两种字符串形态：`to_pdms_str()` 的斜杠 `A/B`，与 `Display` 的下划线
    /// `A_B`。`record_id_of` 会把斜杠折成下划线，所以两种拼法**落在同一行上**——不会
    /// 长出重复行，但字段值变成「谁最后 upsert 谁说了算」。而
    /// [`current_regen_revision`] 与 [`retry_pending_unit`] 是拿字符串精确查这个字段
    /// 的：拼法一换，查询命中零行、返回 `Ok(None)`，与「本来就没有这行」完全无从
    /// 区分。`staged_settlement_revision` 拿到 `None` 就跳过收口，那个根在提交后的
    /// 空闲轮里被原样重生成一遍，而日志里只有 `Err` 那一支会出声。
    ///
    /// `sync_and_seed_model_coverage` 曾经是唯一的例外（`pe_thing_to_refno(..).to_string()`）。
    #[test]
    fn every_enqueue_path_spells_target_refno_with_the_slash_form() {
        // 先钉住前提：两种拼法真的不同。这条塌了说明上游改了 Display 或 to_pdms_str，
        // 本测试连同它守的那个不变量都要重看。
        let refno = RefnoEnum::from(RefU64::from_two_nums(24381, 100677));
        assert_eq!(refno.to_pdms_str(), "24381/100677");
        assert_eq!(refno.to_string(), "24381_100677");
        assert_eq!(
            record_id_of(ModelWorkAction::RegenRoot, &refno.to_pdms_str()),
            record_id_of(ModelWorkAction::RegenRoot, &refno.to_string()),
            "两种拼法折成同一个 record id——所以它们抢的是同一行，不是各占一行"
        );

        let source = include_str!("model_update_pending.rs");
        let seeded = source
            .split_once("pub async fn sync_and_seed_model_coverage(")
            .expect("覆盖回填必须存在")
            .1
            .split_once("const STARTUP_CATALOGUE_WINDOW_RECORD_CAP")
            .expect(
                "它在目录窗口那几个常量之前结束（model_coverage_current 已搬去 model_coverage.rs）",
            )
            .0;
        assert!(
            seeded.contains("pe_thing_to_refno(row.pe.clone())?.to_pdms_str()"),
            "覆盖回填写进 target_refno 的必须是斜杠拼法: {seeded}"
        );
        assert!(
            !seeded.contains("pe_thing_to_refno(row.pe.clone())?.to_string()"),
            "退回 Display 拼法即红：那会让这一批行与其余入队方各说各话: {seeded}"
        );
    }

    /// 收口按 record id 取行（O(1)，`model_update_pending` 没有索引，谓词全表扫在事务内每条 140 ms），
    /// 谓词留作校验：id 必须来自入队方同一个 `record_id_of`，字段必须与行上存的一致，revision 更高
    /// 的顶替行不许被旧收口删掉。
    #[test]
    fn settlement_addresses_the_row_by_its_id_and_guards_it_by_its_fields() {
        let sql = render_delete_revision(ModelWorkAction::RegenRoot, "24381/100677", 3);
        assert_eq!(
            sql,
            "DELETE model_update_pending:regen_root_24381_100677 WHERE action = 'regen_root' \
             AND target_refno = '24381/100677' AND (revision?:0) = 3;"
        );
        assert!(
            sql.starts_with(&format!(
                "DELETE {}",
                record_id_of(ModelWorkAction::RegenRoot, "24381/100677")
            )),
            "settlement must address the same record id the enqueue side writes: {sql}"
        );
        assert!(
            !sql.contains(&format!("DELETE {TABLE} WHERE")),
            "settlement must not fall back to a full-table predicate scan: {sql}"
        );
        let settle = render_complete_regen_revision("24381/100677", 3);
        assert!(
            settle.contains("SELECT * FROM model_update_pending:regen_root_24381_100677 WHERE action = 'regen_root'"),
            "{settle}"
        );
        assert!(
            settle.contains(
                "DELETE model_update_pending:regen_root_24381_100677 WHERE action = 'regen_root'"
            ),
            "{settle}"
        );
        let failed =
            render_mark_failed_revision(ModelWorkAction::RegenRoot, "24381/100677", 3, "boom");
        assert!(
            failed.starts_with(
                "UPDATE model_update_pending:regen_root_24381_100677 SET status = 'failed'"
            ),
            "{failed}"
        );
    }

    #[test]
    fn batch_settlement_is_revision_safe_and_bounded() {
        let items = (0..501)
            .map(|index| (format!("16777216/{}", index + 1), index as u64 + 1))
            .collect::<Vec<_>>();
        let transactions = render_clear_regen_transactions(&items);

        assert_eq!(transactions.len(), 2);
        assert!(transactions.iter().all(|sql| {
            sql.starts_with("BEGIN TRANSACTION;") && sql.ends_with("COMMIT TRANSACTION;")
        }));
        assert!(
            transactions[0].contains(
                "DELETE model_update_pending:regen_root_16777216_1 WHERE action = 'regen_root' \
                 AND target_refno = '16777216/1' AND (revision?:0) = 1;"
            ),
            "{}",
            transactions[0]
        );
        assert!(
            transactions[1].contains(
                "DELETE model_update_pending:regen_root_16777216_501 WHERE action = 'regen_root' \
                 AND target_refno = '16777216/501' AND (revision?:0) = 501;"
            ),
            "{}",
            transactions[1]
        );
    }

    /// 页尾释放按 500 条一个请求，逐行谓词（status / revision / lease_owner）原样保留：
    /// 18k 行从 18k 个往返变成 37 个。
    #[test]
    fn releasing_unsettled_claims_is_batched_per_round_trip_and_keeps_the_row_guards() {
        let job = |index: u64| PendingModelWork {
            dbnum: 7997,
            db_type: "DESI".into(),
            source_end_sesno: 106,
            source_end_sesno_time: None,
            action: ModelWorkAction::RegenRoot,
            target_refno: format!("24381/{index}"),
            noun: "BRAN".into(),
            status: "running".into(),
            attempts: 0,
            last_error: None,
            revision: index,
            required_panels: Vec::new(),
        };
        let jobs = (1..=1001).map(job).collect::<Vec<_>>();
        let batches = render_release_unsettled_claims(&jobs, "owner-a");
        assert_eq!(
            batches.len(),
            3,
            "1001 rows must take 3 round trips at 500 per request"
        );
        assert_eq!(
            batches[0].matches("UPDATE model_update_pending:").count(),
            500
        );
        assert_eq!(
            batches[2].matches("UPDATE model_update_pending:").count(),
            1
        );
        assert!(
            batches[0].contains(
                "UPDATE model_update_pending:regen_root_24381_7 SET status='pending', lease_owner=NONE, \
                 lease_until=NONE WHERE status='running' AND revision=7 AND lease_owner='owner-a';"
            ),
            "{}",
            batches[0]
        );
        assert!(render_release_unsettled_claims(&[], "owner-a").is_empty());
    }

    /// ADR-015：任务身份是 `(action, target_refno)`，`dbnum` 不参与寻址。
    ///
    /// 这条断言的反面正是它要防的事故：`24381/100677` 在 DESI 窗口下 dbnum 是 7997，
    /// 而反向级联与按需生成传的是 Ref0（24381）。id 里只要带 dbnum，同一个根就会分裂
    /// 成两行——重生成两遍，且按需生成那侧永远收不掉真正的 pending。
    #[test]
    fn record_id_ignores_dbnum_so_one_root_can_never_split_into_two_rows() {
        let item = |dbnum| ModelWorkItem {
            dbnum,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "24381/100677".into(),
            noun: "EQUI".into(),
        };
        assert_eq!(
            record_id(&item(7997)),
            "model_update_pending:regen_root_24381_100677"
        );
        assert_eq!(record_id(&item(7997)), record_id(&item(24381)));
        assert_eq!(record_id(&item(7997)), record_id(&item(0)));
    }

    /// B5（2026-07-26 审计 round2）：SurrealDB 对 `UPSERT … SET a = …, b = …` 顺序求值，
    /// 后面的子句读得到前面刚写的值。`attempts` / `last_error` 的复活条件读的是
    /// `source_end_sesno?:0` 的**旧值**，因此这两个子句必须写在 `source_end_sesno = …`
    /// 赋值**之前**——顺序反了，死信将永远不被新会话复活，且无任何报错。此处把书写
    /// 顺序钉成断言，防止一次字段排序整理静默毁掉复活语义。
    #[test]
    fn revival_clauses_run_before_the_watermark_field_they_read() {
        let sql = render_upsert_no_time(&ModelWorkItem {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
        });
        let attempts_at = sql
            .find("attempts = IF")
            .unwrap_or_else(|| panic!("attempts revival clause missing: {sql}"));
        let last_error_at = sql
            .find("last_error = IF")
            .unwrap_or_else(|| panic!("last_error revival clause missing: {sql}"));
        let status_at = sql
            .find("status = IF")
            .unwrap_or_else(|| panic!("status revival clause missing: {sql}"));
        let sesno_write_at = sql
            .find("source_end_sesno = math::max")
            .unwrap_or_else(|| panic!("source_end_sesno write missing: {sql}"));
        assert!(
            attempts_at < sesno_write_at,
            "attempts revival must be evaluated before source_end_sesno is overwritten: {sql}"
        );
        assert!(
            last_error_at < sesno_write_at,
            "last_error reset must be evaluated before source_end_sesno is overwritten: {sql}"
        );
        assert!(
            status_at < sesno_write_at,
            "status revival must be evaluated before source_end_sesno is overwritten: {sql}"
        );
    }

    /// 2026-08-10 审核 P2-1：status 与 attempts / last_error 同一个复活判据。
    ///
    /// 此前 status 无条件写 'pending'：一条死信（attempts 已到上限）被旧会话的
    /// upsert 摸过之后，面板上显示 pending、drain 却永远不取——状态在撒谎。
    /// 现在：旧会话保持原状态，新会话才连同 attempts 一起复活。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stale_trigger_keeps_a_dead_letters_status_and_a_newer_one_revives_it() {
        use surrealdb::engine::any::connect;

        let item = |sesno: i32| ModelWorkItem {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: sesno,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
        };
        // 字符串层面：非无条件复活的任务，status 必须是条件子句。
        let sql = render_upsert_no_time(&item(42));
        assert!(
            sql.contains(
                "status = IF 42 > (source_end_sesno?:0) THEN 'pending' ELSE status?:'pending' END"
            ),
            "{sql}"
        );

        // 持久层层面：入队 → 打成死信 → 旧会话摸一把 → 状态不变；新会话 → 复活。
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("upsert_status")
            .use_db("main")
            .await
            .expect("use db");
        #[derive(serde::Deserialize)]
        struct Row {
            status: String,
            attempts: u32,
        }
        let read = |db: surrealdb::Surreal<surrealdb::engine::any::Any>| async move {
            let mut response = db
                .query(format!(
                    "SELECT status, attempts FROM {TABLE} WHERE target_refno = '16777216/5';"
                ))
                .await
                .expect("read transport")
                .check()
                .expect("read");
            let rows: Vec<Row> = response.take(0).expect("decode");
            let row = rows.into_iter().next().expect("row exists");
            (row.status, row.attempts)
        };

        db.query(render_upsert_no_time(&item(42)))
            .await
            .expect("enqueue transport")
            .check()
            .expect("enqueue");
        db.query(format!(
            "UPDATE {TABLE} SET status = 'failed', attempts = {MAX_ATTEMPTS} \
             WHERE target_refno = '16777216/5';"
        ))
        .await
        .expect("dead-letter transport")
        .check()
        .expect("dead-letter");

        db.query(render_upsert_no_time(&item(41)))
            .await
            .expect("stale transport")
            .check()
            .expect("stale upsert");
        assert_eq!(
            read(db.clone()).await,
            ("failed".to_string(), MAX_ATTEMPTS),
            "旧会话不构成复活理由，状态与 attempts 都不许动"
        );

        db.query(render_upsert_no_time(&item(43)))
            .await
            .expect("newer transport")
            .check()
            .expect("newer upsert");
        assert_eq!(
            read(db.clone()).await,
            ("pending".to_string(), 0),
            "新会话把死信整体复活"
        );
    }

    /// T5（plant-ui ADR-0019 Q7）：来源保存时刻与 `source_end_sesno` 同生共死。
    ///
    /// 三条：① 时刻子句读的是旧值，必须排在覆盖之前（与上面的复活子句同一个道理）；
    /// ② 单调条件与序号那条一致，来源没变新时不许把时刻换成更早的；
    /// ③ **端点对不上就不贴**——一份收口里的任务并非都来自同一条保存。
    #[test]
    fn the_source_save_time_is_written_with_its_own_sesno_and_only_for_that_endpoint() {
        let claiming = ModelWorkItem {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
        };

        let sql = render_upsert(&claiming, Some("2026-08-05T18:24:00+08:00"));
        let time_at = sql
            .find("source_end_sesno_time = IF 42 >= (source_end_sesno?:0)")
            .unwrap_or_else(|| panic!("时刻子句必须带单调条件: {sql}"));
        let sesno_write_at = sql
            .find("source_end_sesno = math::max")
            .unwrap_or_else(|| panic!("source_end_sesno write missing: {sql}"));
        assert!(
            time_at < sesno_write_at,
            "时刻子句读的是旧值，必须排在覆盖之前: {sql}"
        );
        assert!(
            sql.contains("THEN '2026-08-05T18:24:00+08:00' ELSE"),
            "{sql}"
        );

        // 读不到时刻 → 整条子句都不写，旧行与新行走同一条降级路径（来源段不摆）。
        assert!(
            !render_upsert_no_time(&claiming).contains("source_end_sesno_time"),
            "没有时刻时不该出现这条子句"
        );

        // 端点守卫：本次收口的右端是 43，这一行认领的却是 42——不许把 43 的时刻贴上去。
        assert_eq!(
            source_time_for(&claiming, 43, Some("2026-08-07T14:10:00+08:00")),
            None,
            "号对不上就不贴时刻"
        );
        assert_eq!(
            source_time_for(&claiming, 42, Some("2026-08-05T18:24:00+08:00")),
            Some("2026-08-05T18:24:00+08:00")
        );

        // 不认领会话号的行（房间任务 / 派生根）永远拿不到时刻，即便右端恰好是 0。
        let room = room_item(ModelWorkAction::RoomRecalcPanel, 24381, 0);
        assert_eq!(room.source_end_sesno, 0, "前提：房间任务不认领会话号");
        assert_eq!(
            source_time_for(&room, 0, Some("2026-08-07T14:10:00+08:00")),
            None,
            "不认领来源的行不许被 0 == 0 误伤"
        );
    }

    /// 同一个坑的第二处（plant-ui ADR-0019 Q6）：水位的时刻列若无条件赋值，一个
    /// `end_sesno` 低于存量水位的批次会让**序号不动、时刻却退回去**，而回退阻断卡
    /// 恰好靠这一对说话。时刻必须跟着序号那条单调条件走，且读的是 `applied_sesno`
    /// 的**旧值**——因此子句要排在它被覆盖之前，与上面那条复活子句同一个道理。
    #[test]
    fn the_applied_time_rides_the_same_monotonic_condition_as_the_watermark() {
        let sql = render_watermark_advance(8000, 1031, Some("2026-08-07T14:10:00+08:00"));
        let time_at = sql
            .find("applied_sesno_time = IF 1031 >= (applied_sesno ?: 0)")
            .unwrap_or_else(|| panic!("时刻子句必须带单调条件: {sql}"));
        let sesno_write_at = sql
            .find("applied_sesno = math::max")
            .unwrap_or_else(|| panic!("watermark write missing: {sql}"));
        assert!(
            time_at < sesno_write_at,
            "时刻子句读的是 applied_sesno 的旧值，必须排在它被覆盖之前: {sql}"
        );
        // 存字符串而不是 datetime：读回来不能被规范化成 UTC，否则同一张阻断卡上
        // 「文件端」（现读，带本地时区）与「已应用端」会差八个小时。
        assert!(
            sql.contains("THEN '2026-08-07T14:10:00+08:00' ELSE"),
            "{sql}"
        );
        assert!(!sql.contains("type::datetime"), "{sql}");

        // 读不到时刻 → 整条子句都不写（不是写 NONE）：旧行与拿不到时刻的新行走
        // 同一条降级路径，界面说「应用时刻无记录」。
        let without_time = render_watermark_advance(8000, 1031, None);
        assert!(
            !without_time.contains("applied_sesno_time"),
            "{without_time}"
        );
        assert!(
            without_time.contains("applied_sesno = math::max([applied_sesno?:0, 1031])"),
            "{without_time}"
        );
    }

    /// 落库那一半：序号与时刻同生共死，且旧行读出来是缺席而不是报错。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_batch_below_the_watermark_moves_neither_the_sesno_nor_its_time() {
        use surrealdb::engine::any::connect;

        #[derive(Debug, Deserialize)]
        struct WatermarkRow {
            applied_sesno: i32,
            #[serde(default)]
            applied_sesno_time: Option<String>,
        }

        async fn row_of(
            db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        ) -> (i32, Option<String>) {
            let mut response = db
                .query("SELECT applied_sesno, applied_sesno_time FROM dbnum_watermark:8000;")
                .await
                .expect("read watermark")
                .check()
                .expect("read statement");
            let rows: Vec<WatermarkRow> = response.take(0).expect("decode watermark");
            let row = rows.into_iter().next().expect("watermark row exists");
            (row.applied_sesno, row.applied_sesno_time)
        }

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("watermark_applied_time")
            .await
            .expect("select fixture db");

        // ① 旧行：本字段引入之前写的那种，读出来必须是缺席而不是解码失败。
        db.query(render_watermark_advance(8000, 1024, None))
            .await
            .expect("legacy advance")
            .check()
            .expect("legacy statement");
        assert_eq!(row_of(&db).await, (1024, None));

        // ② 正常推进：序号与时刻一起落库。
        db.query(render_watermark_advance(
            8000,
            1031,
            Some("2026-08-07T14:10:00+08:00"),
        ))
        .await
        .expect("advance")
        .check()
        .expect("advance statement");
        assert_eq!(
            row_of(&db).await,
            (1031, Some("2026-08-07T14:10:00+08:00".into()))
        );

        // ③ 一个右端低于存量水位的批次（崩溃重放会真的产生它）：两个都不许动。
        db.query(render_watermark_advance(
            8000,
            1029,
            Some("2026-08-06T09:15:00+08:00"),
        ))
        .await
        .expect("stale advance")
        .check()
        .expect("stale statement");
        assert_eq!(
            row_of(&db).await,
            (1031, Some("2026-08-07T14:10:00+08:00".into())),
            "序号没退，时刻更不许退——阻断卡靠这一对说话"
        );
    }

    /// B6：反向级联派生出来的根**不记在种子所在的目录库**账上——那样它的死信只能等
    /// 下一次目录库会话来复活，而真正需要它重生成的设计库会话永远够不着它。会话号同理：
    /// 跨库比大小没有意义，所以派生任务不认领任何会话号。
    ///
    /// 也不能拿 `refno().get_0()` 冒充设计库号：`24381/100677` 的 dbnum 是 7997，24381
    /// 只是 Ref0。填一个看着像真 dbnum 的 Ref0，最坏情况是撞上另一个库、被那个库的批次
    /// 工作单捞走。这一层没有 Ref0→dbnum 的反查结果，就如实留空。
    #[test]
    fn a_cascade_derived_root_claims_neither_a_database_nor_a_session() {
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};

        let item = derived_regen_item(GenerationRoot {
            root: RefnoEnum::from(RefU64((24381u64 << 32) | 100677)),
            noun: "EQUI".into(),
            name: "/PUMP-01".into(),
            kind: GenerationRootKind::DeliveryUnit,
        });

        assert_eq!(item.dbnum, 0, "Ref0 不是 dbnum，来源库未解析就留空");
        assert_eq!(item.db_type, "DESI");
        assert_eq!(item.action, ModelWorkAction::RegenRoot);
        assert_eq!(item.target_refno, "24381/100677");
        assert_eq!(
            item.source_end_sesno, 0,
            "跨库会话号不可比，派生任务不认领会话"
        );
    }

    fn room_item(action: ModelWorkAction, dbnum: u32, end_sesno: i32) -> ModelWorkItem {
        ModelWorkItem {
            dbnum,
            db_type: "DESI".into(),
            source_end_sesno: end_sesno,
            action,
            target_refno: "24381/34303".into(),
            noun: "PANE".into(),
        }
    }

    #[test]
    fn scoped_room_drain_addresses_only_the_current_plan_targets() {
        let panel = room_item(ModelWorkAction::RoomRecalcPanel, 7999, 90);
        let mut element = room_item(ModelWorkAction::RoomRecalcElement, 7999, 90);
        element.target_refno = "24381/100677".into();
        element.noun = "EQUI".into();
        let mut unrelated = room_item(ModelWorkAction::RegenRoot, 7999, 90);
        unrelated.target_refno = "24381/999999".into();
        let plan = ModelUpdatePlan {
            work_items: vec![panel.clone(), element.clone(), panel, unrelated],
            ..Default::default()
        };

        let scope = RoomDrainScope::from_plan(&plan);
        assert_eq!(scope.len(), 2, "重复目标要去重，非房间任务要排除");

        let panel_sql =
            render_scoped_room_select(&scope.keys_for(ModelWorkAction::RoomRecalcPanel));
        assert!(
            panel_sql.contains("model_update_pending:room_recalc_panel_24381_34303"),
            "{panel_sql}"
        );
        assert!(!panel_sql.contains("24381_100677"), "{panel_sql}");
        assert!(!panel_sql.contains("24381_999999"), "{panel_sql}");
        assert!(panel_sql.contains("record::exists(id)"), "{panel_sql}");

        let element_sql =
            render_scoped_room_select(&scope.keys_for(ModelWorkAction::RoomRecalcElement));
        assert!(
            element_sql.contains("model_update_pending:room_recalc_element_24381_100677"),
            "{element_sql}"
        );
        assert!(!element_sql.contains("24381_34303"), "{element_sql}");
    }

    #[tokio::test]
    async fn room_failures_remain_selectable_without_attempt_limits() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("room_no_attempt_limit")
            .await
            .unwrap();
        let item = room_item(ModelWorkAction::RoomRecalcElement, 0, 0);
        db.query(render_upsert_no_time(&item))
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query(format!("UPDATE {} SET attempts=999;", record_id(&item)))
            .await
            .unwrap()
            .check()
            .unwrap();
        let failure =
            render_mark_failed_revision(item.action, &item.target_refno, 1, "fixture failure");
        assert!(!failure.contains("attempts"));
        db.query(failure).await.unwrap().check().unwrap();
        let keys = vec![RoomWorkKey {
            action: item.action,
            target_refno: item.target_refno,
        }];
        assert_eq!(load_scoped_room_jobs_on(&db, &keys).await.unwrap().len(), 1);
        let mut result = db
            .query(render_model_pending_status_query_for_scope(&[], &[]))
            .await
            .unwrap()
            .check()
            .unwrap();
        let live: Vec<ModelPendingActionCount> = result.take(0).unwrap();
        let dead: Vec<ModelPendingActionCount> = result.take(1).unwrap();
        assert_eq!(live[0].c, 1);
        assert!(dead.is_empty());
    }

    #[tokio::test]
    async fn full_room_rebuild_clears_only_older_room_work() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("full_room_settlement")
            .await
            .unwrap();
        db.query(
            "CREATE model_update_pending:old_element SET action='room_recalc_element', updated_at=d'2026-09-09T00:00:00Z';
             CREATE model_update_pending:new_element SET action='room_recalc_element', updated_at=d'2026-09-09T00:00:02Z';
             CREATE model_update_pending:old_panel SET action='room_recalc_panel', updated_at=d'2026-09-09T00:00:00Z';
             CREATE model_update_pending:new_panel SET action='room_recalc_panel', updated_at=d'2026-09-09T00:00:02Z';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let cleared = clear_room_work_covered_by_full_rebuild_on(&db, "2026-09-09T00:00:01Z")
            .await
            .unwrap();
        assert_eq!(cleared, 2);
        let mut result = db
            .query("SELECT VALUE id FROM model_update_pending ORDER BY id;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let ids: Vec<surrealdb::sql::Thing> = result.take(0).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().any(|id| id.id.to_string() == "new_element"));
        assert!(ids.iter().any(|id| id.id.to_string() == "new_panel"));
    }

    #[tokio::test]
    async fn scoped_room_selection_isolated_backlog_and_revision_safe_on_surreal() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("scoped_room_selection")
            .await
            .expect("select fixture db");
        let mut current = room_item(ModelWorkAction::RoomRecalcElement, 7999, 90);
        current.target_refno = "24381/100677".into();
        current.noun = "EQUI".into();
        let mut backlog = current.clone();
        backlog.source_end_sesno = 80;
        backlog.target_refno = "24381/999999".into();
        db.query(format!(
            "{}\n{}",
            render_upsert_no_time(&current),
            render_upsert_no_time(&backlog)
        ))
        .await
        .expect("seed pending rows")
        .check()
        .expect("seed statements");

        let plan = ModelUpdatePlan {
            work_items: vec![current.clone()],
            ..Default::default()
        };
        let keys = RoomDrainScope::from_plan(&plan).keys_for(ModelWorkAction::RoomRecalcElement);
        let selected = load_scoped_room_jobs_on(&db, &keys)
            .await
            .expect("select exact current-task row");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].target_refno, current.target_refno);

        let mut newer = current.clone();
        newer.source_end_sesno = 91;
        db.query(render_upsert_no_time(&newer))
            .await
            .expect("publish newer revision")
            .check()
            .expect("publish statement");
        delete_work_on(&db, &selected[0])
            .await
            .expect("old settlement is a successful no-op");

        let mut response = db
            .query(format!("SELECT * FROM {} ORDER BY target_refno;", TABLE))
            .await
            .expect("read remaining pending")
            .check()
            .expect("read statement");
        let remaining: Vec<PendingModelWork> = response.take(0).expect("decode pending");
        assert_eq!(remaining.len(), 2, "历史 backlog 与新 revision 都必须保留");
        let current_row = remaining
            .iter()
            .find(|row| row.target_refno == current.target_refno)
            .expect("new revision survives old delete");
        assert_eq!(current_row.revision, selected[0].revision + 1);
        assert!(
            remaining
                .iter()
                .any(|row| row.target_refno == backlog.target_refno),
            "当前 scope 不得消费历史 backlog"
        );

        // Room completion also keeps a durable version proof. A stale revision
        // must neither remove the replacement task nor advance that proof.
        db.query(render_complete_room_revision(&selected[0], 11))
            .await
            .expect("stale room completion transport")
            .check()
            .expect("stale room completion statement");
        let current_row = remaining
            .iter()
            .find(|row| row.target_refno == current.target_refno)
            .expect("new revision remains available")
            .clone();
        db.query(render_complete_room_revision(&current_row, 12))
            .await
            .expect("current room completion transport")
            .check()
            .expect("current room completion statement");

        #[derive(Deserialize)]
        struct CompletedVersion {
            computed_publish_version: u64,
            completed_revision: u64,
        }
        let mut response = db
            .query(format!(
                "SELECT computed_publish_version, completed_revision FROM ONLY \
                 type::thing('{ROOM_TARGET_VERSION_TABLE}', '{}');",
                room_target_version_id(current_row.action, &current_row.target_refno)
            ))
            .await
            .expect("read room completion version")
            .check()
            .expect("read room completion version statement");
        let completed: Option<CompletedVersion> = response.take(0).expect("decode completion");
        let completed = completed.expect("current revision writes version proof");
        assert_eq!(completed.computed_publish_version, 12);
        assert_eq!(completed.completed_revision, current_row.revision);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn room_edges_version_and_revision_settle_in_one_guarded_transaction() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("atomic_room_publish")
            .await
            .expect("select fixture db");
        let item = ModelWorkItem {
            dbnum: 0,
            db_type: "DESI".into(),
            source_end_sesno: 0,
            action: ModelWorkAction::RoomRecalcPanel,
            target_refno: "24381/10".into(),
            noun: "PANE".into(),
        };
        db.query(format!(
            "CREATE pe:panel; CREATE pe:old_member; CREATE pe:new_member;\n{}",
            render_upsert_no_time(&item)
        ))
        .await
        .expect("seed first revision")
        .check()
        .expect("seed first revision statements");
        let mut response = db
            .query(format!("SELECT * FROM {TABLE};"))
            .await
            .expect("read revision one")
            .check()
            .expect("read revision one statement");
        let stale: PendingModelWork = response
            .take::<Vec<PendingModelWork>>(0)
            .expect("decode revision one")
            .remove(0);

        db.query(format!(
            "RELATE pe:panel->room_relate:panel_old_member->pe:old_member \
             SET room_num = 'R1';\n{}",
            render_upsert_no_time(&item)
        ))
        .await
        .expect("seed old edge and revision two")
        .check()
        .expect("seed old edge and revision two statements");
        let mutations = vec![
            "DELETE pe:panel->room_relate".to_string(),
            "INSERT RELATION INTO room_relate [{ id: room_relate:panel_new_member, \
             in: pe:panel, out: pe:new_member, room_num: 'R2' }]"
                .to_string(),
        ];

        db.query(room_model::render_room_write(
            &mutations,
            Some(&room_write_guard(&stale, 11)),
        ))
        .await
        .expect("execute stale guarded write")
        .check()
        .expect("stale guarded write statement");
        let mut response = db
            .query("SELECT VALUE record::id(out) FROM pe:panel->room_relate;")
            .await
            .expect("read edge after stale write")
            .check()
            .expect("read edge after stale statement");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("decode old edge"),
            ["old_member"],
            "stale revision must not touch room edges"
        );

        let mut response = db
            .query(format!("SELECT * FROM {TABLE};"))
            .await
            .expect("read current revision")
            .check()
            .expect("read current revision statement");
        let current: PendingModelWork = response
            .take::<Vec<PendingModelWork>>(0)
            .expect("decode current revision")
            .remove(0);
        db.query(room_model::render_room_write(
            &mutations,
            Some(&room_write_guard(&current, 12)),
        ))
        .await
        .expect("execute current guarded write")
        .check()
        .expect("current guarded write statement");
        let mut response = db
            .query(format!(
                "SELECT VALUE record::id(out) FROM pe:panel->room_relate;\n\
                 RETURN record::exists({TABLE}:room_recalc_panel_24381_10);\n\
                 SELECT computed_publish_version FROM ONLY type::thing(\
                   '{ROOM_TARGET_VERSION_TABLE}', '{}');",
                room_target_version_id(current.action, &current.target_refno)
            ))
            .await
            .expect("read committed room publication")
            .check()
            .expect("read committed room publication statements");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("decode new edge"),
            ["new_member"]
        );
        assert_eq!(response.take::<Option<bool>>(1).unwrap(), Some(false));
        assert_eq!(
            response
                .take::<Option<serde_json::Value>>(2)
                .unwrap()
                .expect("version proof exists")["computed_publish_version"],
            12
        );
    }

    #[test]
    fn scoped_element_targets_are_paginated_at_the_room_page_size() {
        let plan = ModelUpdatePlan {
            work_items: (0..(ROOM_DRAIN_PAGE_SIZE + 1))
                .map(|seq| ModelWorkItem {
                    dbnum: 7999,
                    db_type: "DESI".into(),
                    source_end_sesno: 90,
                    action: ModelWorkAction::RoomRecalcElement,
                    target_refno: format!("24381/{}", 100000 + seq),
                    noun: "EQUI".into(),
                })
                .collect(),
            ..Default::default()
        };

        let pages = RoomDrainScope::from_plan(&plan)
            .pages_for(ModelWorkAction::RoomRecalcElement, ROOM_DRAIN_PAGE_SIZE);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].len(), ROOM_DRAIN_PAGE_SIZE);
        assert_eq!(pages[1].len(), 1);

        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_rooms_selected(")
            .expect("scoped drain must exist")
            .1
            .split_once("async fn drain_room_element_page(")
            .expect("element page boundary")
            .0;
        assert!(
            !body.contains("elements.extend"),
            "scoped 元素记录不得跨页累积: {body}"
        );
        let page_at = body
            .find("for page in scope.pages_for")
            .expect("scope must iterate pages");
        let load_at = body[page_at..]
            .find("load_scoped_room_jobs(&page)")
            .map(|at| page_at + at)
            .expect("each page must load exact pending rows");
        let drain_at = body[load_at..]
            .find("drain_room_element_page(")
            .map(|at| load_at + at)
            .expect("each loaded page must drain immediately");
        assert!(page_at < load_at && load_at < drain_at, "{body}");
    }

    #[test]
    fn global_room_pages_yield_to_new_data_without_weakening_scoped_drains() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_rooms_selected(")
            .expect("room drain must exist")
            .1
            .split_once("#[cfg(test)]")
            .map_or_else(|| source.to_string(), |(body, _)| body.to_string());

        assert!(
            body.contains("if scope.is_none() && data_batch_waiting()"),
            "global panel work must yield after a completed job"
        );
        assert!(
            body.split_whitespace()
                .collect::<String>()
                .contains("published_version,false,"),
            "scoped post-commit element drains must remain atomic"
        );
        assert!(
            body.split_whitespace()
                .collect::<String>()
                .contains("published_version,true,"),
            "global element pages must enable data preemption"
        );
        assert!(
            body.contains("if yield_for_data && data_batch_waiting()"),
            "global element work must yield between jobs"
        );
    }

    #[tokio::test]
    async fn empty_scoped_room_drain_is_a_connection_free_noop() {
        let report = drain_rooms_scoped(
            &aios_core::options::DbOption::default(),
            &RoomDrainScope::default(),
        )
        .await
        .expect("empty scope must not touch SUL_DB");

        assert_eq!(report.requested, 0);
        assert_eq!(report.loaded, 0);
        assert_eq!(report.done, 0);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn task_scoped_room_drain_does_not_wait_for_the_initialization_run() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("pub(crate) async fn drain_rooms_scoped(")
            .expect("scoped room drain exists")
            .1
            .split_once("pub(crate) async fn promote_room_file_round(")
            .expect("promotion follows scoped room drain")
            .0;
        assert!(
            !body.contains("postprocess_allowed"),
            "a dbnum Task must finish its own room work during initialization"
        );
    }

    /// ADR-010 §7：房间任务的行不带 dbnum。一块面板天然跨库，带上 dbnum 会让同一间房
    /// 在一轮里排出多行、被重算多遍，失败后又只能等同一个库的新会话来复活，而真正
    /// 触发它的那些库永远够不着它（B6 的放大版）。
    #[test]
    fn a_room_task_is_addressed_by_target_alone_across_databases() {
        let from_one_db = record_id(&room_item(ModelWorkAction::RoomRecalcPanel, 24381, 42));
        let from_another = record_id(&room_item(ModelWorkAction::RoomRecalcPanel, 24384, 7));
        assert_eq!(from_one_db, from_another);
        assert_eq!(
            from_one_db,
            "model_update_pending:room_recalc_panel_24381_34303"
        );

        // 元素分支与整间分支是两种任务，同一个目标上不能挤成一行。
        assert_ne!(
            from_one_db,
            record_id(&room_item(ModelWorkAction::RoomRecalcElement, 24381, 42))
        );

        // ADR-015 之后其余任务同样不按库分行。
        let regen = ModelWorkItem {
            action: ModelWorkAction::RegenRoot,
            ..room_item(ModelWorkAction::RegenRoot, 24381, 42)
        };
        assert_eq!(
            record_id(&regen),
            "model_update_pending:regen_root_24381_34303"
        );
    }

    /// 触发源的分流（ADR-010 §2）：PANE 自己一动，整间房的成员全变，元素级表达不了，
    /// 必须整块面板重算；其余元素只重算自己的归属。
    #[test]
    fn a_moved_panel_routes_to_the_whole_room_branch() {
        let change = |refno: u64, noun: &str| AabbChange {
            refno: RefnoEnum::from(RefU64((24381u64 << 32) | refno)),
            noun: noun.into(),
        };

        let panel = room_recalc_item(&change(34303, "PANE"));
        assert_eq!(panel.action, ModelWorkAction::RoomRecalcPanel);
        assert_eq!(panel.target_refno, "24381/34303");
        // 来源库与会话号都不认领：这一层拿不到 Ref0→dbnum 的反查结果，填 Ref0 会把这行
        // 误挂到某个恰好同号的库名下。
        assert_eq!(panel.dbnum, 0);
        assert_eq!(panel.source_end_sesno, 0);

        assert_eq!(
            room_recalc_item(&change(100677, "EQUI")).action,
            ModelWorkAction::RoomRecalcElement
        );
    }

    #[test]
    fn direct_aabb_transaction_reuses_the_durable_room_upsert() {
        let panel = AabbChange {
            refno: RefnoEnum::from(RefU64((24381u64 << 32) | 34303)),
            noun: "PANE".into(),
        };
        let element = AabbChange {
            refno: RefnoEnum::from(RefU64((24381u64 << 32) | 100677)),
            noun: "EQUI".into(),
        };
        let sql = render_room_recalc_upserts(&[panel.clone(), element, panel]);

        assert_eq!(
            sql.matches("UPSERT model_update_pending:room_recalc_panel_24381_34303")
                .count(),
            1,
            "同一 chunk 的重复触发只应发布一行: {sql}"
        );
        assert!(
            sql.contains("UPSERT model_update_pending:room_recalc_element_24381_100677"),
            "{sql}"
        );
        assert!(sql.contains("revision = (revision?:0) + 1"), "{sql}");
        assert!(
            !sql.contains("BEGIN TRANSACTION"),
            "事务由 AABB 指针调用方统一包装: {sql}"
        );
    }

    /// 面板覆盖率要按「缺了几块」报，不能只在「一块都没有」时才出声。
    ///
    /// 147 块在册面板里只有 12 块有几何（issue #7 审核实测）同样是异常，而全 0 判据
    /// 对它一声不响——落在那 135 块里的构件每一轮都被收敛成「不属于任何房间」，现场
    /// 只看得到房间号消失。
    #[test]
    fn the_room_round_reports_partial_panel_coverage() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("pub async fn drain_rooms(")
            .expect("drain_rooms 必须存在")
            .1
            .split_once("\nasync fn load_room_jobs(")
            .expect("drain_rooms 之后是 load_room_jobs")
            .0;

        assert!(
            body.contains("missing_panels()"),
            "覆盖率必须按缺失面板数报: {body}"
        );
        assert!(
            !body.contains("usable_panels() == 0"),
            "只在一块都没有时才出声，等于放过 12/147 那种状态: {body}"
        );
    }

    /// 面板侧也必须分页，而且满页时元素侧要整页让开。
    ///
    /// 面板任务与 regen 是一起产的（现场每消化 55 个根多 4 块面板），而这条路上一块
    /// 面板从载网格到整间收口不可中断、中途不查 epoch。不分页的话，积压跑完那一轮
    /// 要一口气吃掉上千块，单轮时长没有上限。
    #[test]
    fn the_global_room_round_pages_its_panels_too() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_rooms_selected(")
            .expect("drain_rooms_selected 必须存在")
            .1
            .split_once("async fn drain_room_element_page(")
            .expect("drain_rooms_selected 之后是元素分页执行体")
            .0;

        assert!(
            body.contains("load_room_jobs(ROOM_PANEL_ACTION_FILTER, Some(ROOM_PANEL_PAGE_SIZE))"),
            "全局面板侧必须带页大小，`None` 就是没有单轮时长上限: {body}"
        );
        let page_full = body
            .find("let panel_page_full")
            .expect("满页判定必须存在——元素侧靠它决定要不要让开");
        let elements = body
            .find("load_room_jobs(ROOM_ELEMENT_ACTION_FILTER")
            .expect("元素侧仍要分页加载");
        assert!(
            page_full < elements,
            "满页判定必须先于元素加载，否则面板没收完就跑元素、同轮吸收白让: {body}"
        );
        assert!(
            body.contains("!panel_page_full"),
            "元素侧必须受满页判定门控: {body}"
        );
    }

    /// 同轮吸收的封闭性（ADR-010 §8，2026-07-28 修订）：旧边或候选任何一个越出本轮
    /// claimed 面板集合，元素任务都必须照跑——错吸收会把本轮没重算的面板指向该构件的
    /// 陈旧边永久留在库里，或漏写它新进入的本轮外面板的边。
    #[test]
    fn absorption_requires_old_edges_and_candidates_inside_the_claimed_set() {
        let panel = |seq: u64| RefnoEnum::from(RefU64((4000000001u64 << 32) | seq));
        let element = panel(20);
        let claimed: HashSet<RefnoEnum> = [panel(10)].into();

        // 旧边与候选都在 claimed 里：吸收成立。
        let mut inputs = AbsorptionClosureInputs::default();
        inputs.old_edge_panels.insert(element, [panel(10)].into());
        inputs.candidate_panels.insert(element, [panel(10)].into());
        assert!(absorption_verdict(&inputs, element, &claimed));

        // 旧边指向本轮没重算的面板：只有元素分支能清它，不得吸收。
        let mut stale_old = AbsorptionClosureInputs::default();
        stale_old
            .old_edge_panels
            .insert(element, [panel(11)].into());
        stale_old
            .candidate_panels
            .insert(element, [panel(10)].into());
        assert!(!absorption_verdict(&stale_old, element, &claimed));

        // 候选里有本轮没重算的面板：它的新边只有元素分支会写，不得吸收。
        let mut outside_candidate = AbsorptionClosureInputs::default();
        outside_candidate
            .old_edge_panels
            .insert(element, [panel(10)].into());
        outside_candidate
            .candidate_panels
            .insert(element, [panel(10), panel(11)].into());
        assert!(!absorption_verdict(&outside_candidate, element, &claimed));

        // 没有旧边（映射缺位 = 空集）不阻碍吸收；候选缺位 = 封闭性未知，一律不吸收。
        let mut no_old_edges = AbsorptionClosureInputs::default();
        no_old_edges
            .candidate_panels
            .insert(element, [panel(10)].into());
        assert!(absorption_verdict(&no_old_edges, element, &claimed));
        assert!(!absorption_verdict(
            &AbsorptionClosureInputs::default(),
            element,
            &claimed
        ));
    }

    /// 房间任务的死信无条件复活，而不是按会话号比。
    ///
    /// 常规任务的判据「来了更新的会话」在这里不成立：行不带 dbnum，同一块面板被不同库
    /// 轮流触发，跨库比 sesno 只会让一个库的 500 永久压住另一个库的 80。而房间任务的
    /// 入队条件本身就是「AABB 真的变了」，每一次入队都是全新的重算理由。
    #[test]
    fn a_room_task_revives_on_any_new_trigger_not_on_a_newer_session() {
        let sql = render_upsert_no_time(&room_item(ModelWorkAction::RoomRecalcPanel, 24381, 42));
        assert!(!sql.contains("attempts ="), "{sql}");
        assert!(sql.contains("last_error = NONE"), "{sql}");
        assert!(
            !sql.contains("IF 42 > (source_end_sesno?:0)"),
            "房间任务不应按会话号决定是否复活: {sql}"
        );
        // dbnum / source_end_sesno 降为字段，只记最后一次触发来源。
        assert!(
            sql.contains("dbnum = math::max([dbnum?:0, 24381])"),
            "{sql}"
        );
        assert!(
            sql.contains("source_end_sesno = math::max([source_end_sesno?:0, 42])"),
            "{sql}"
        );
    }

    /// 不认领会话号的任务必须无条件复活，否则它一旦判死就永远醒不过来。
    ///
    /// 派生根的 `source_end_sesno` 是 0（跨库会话号不可比，如实留空），而按会话号
    /// 比的复活判据是 `0 > 0` —— 恒假。于是它失败到 MAX_ATTEMPTS 之后，后续每一次
    /// 目录改动重新把它推进队列时都只是 `revision + 1`，`attempts` 纹丝不动，
    /// `drain` 的 `attempts < MAX_ATTEMPTS` 永远把它挡在外面：构件停在旧几何，
    /// 队列里躺着一行谁也不会去执行的任务。
    #[test]
    fn a_task_that_claims_no_session_revives_on_every_enqueue() {
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};

        let derived = derived_regen_item(GenerationRoot {
            root: RefnoEnum::from(RefU64((24381u64 << 32) | 100677)),
            noun: "EQUI".into(),
            name: "/PUMP-01".into(),
            kind: GenerationRootKind::DeliveryUnit,
        });
        assert_eq!(derived.source_end_sesno, 0, "前提：派生根不认领会话号");

        let sql = render_upsert_no_time(&derived);
        assert!(sql.contains("attempts = 0"), "{sql}");
        assert!(sql.contains("last_error = NONE"), "{sql}");
        assert!(
            !sql.contains("attempts = IF"),
            "不认领会话号的任务不能按会话号决定是否复活（0 > 0 恒假）: {sql}"
        );
    }

    /// 不认领来源库的入队（dbnum == 0：派生根、按需生成）不得抹掉行上已存的真实
    /// 库号。抹掉的后果是延迟：DESI 窗口曾把真 dbnum 写上去，这个根本属于「本库
    /// 批次工作单」；被 0 覆盖之后它只能等空闲轮的 `drain_data_phases`。
    #[test]
    fn an_enqueue_that_claims_no_dbnum_keeps_the_stored_one() {
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};

        let derived = derived_regen_item(GenerationRoot {
            root: RefnoEnum::from(RefU64((24381u64 << 32) | 100677)),
            noun: "EQUI".into(),
            name: "/PUMP-01".into(),
            kind: GenerationRootKind::DeliveryUnit,
        });
        assert_eq!(derived.dbnum, 0, "前提：派生根不认领来源库");
        let sql = render_upsert_no_time(&derived);
        assert!(
            sql.contains("dbnum = dbnum?:0"),
            "不认领的入队必须保留行上已存的库号: {sql}"
        );

        // 认领了库号的常规入队照写本次来源，行为不变。
        let claiming = render_upsert_no_time(&ModelWorkItem {
            dbnum: 7997,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "24381/100677".into(),
            noun: "EQUI".into(),
        });
        assert!(claiming.contains("dbnum = 7997"), "{claiming}");
    }

    /// 反过来：认领了会话号的常规任务仍按会话号比，旧会话不构成复活理由。
    #[test]
    fn a_task_that_claims_a_session_still_revives_only_on_a_newer_one() {
        let sql = render_upsert_no_time(&ModelWorkItem {
            dbnum: 7997,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "24381/100677".into(),
            noun: "EQUI".into(),
        });
        assert!(
            sql.contains("attempts = IF 42 > (source_end_sesno?:0)"),
            "{sql}"
        );
        assert!(
            sql.contains("last_error = IF 42 > (source_end_sesno?:0)"),
            "{sql}"
        );
        assert!(
            !sql.contains("attempts = 0,"),
            "常规任务不该无条件复活: {sql}"
        );
    }

    /// 同轮吸收的封闭性检查不许再碰空间树。
    ///
    /// 元素分支的候选 2026-08-05 已从空间树改成库内面板几何（`PanelIndex`）；预测元素
    /// 分支会碰哪些面板的封闭性检查必须同源。留在树上的话，树缺在册 PANE 条目时
    /// （issue #7 的典型态）会拿到空候选、错误吸收，把元素分支本会写的边永久跳过。
    #[test]
    fn the_absorption_closure_does_not_depend_on_the_spatial_tree() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn load_absorption_closure_inputs(")
            .expect("load_absorption_closure_inputs 必须存在")
            .1
            .split_once("\nasync fn run_room_job(")
            .expect("封闭性输入之后是 run_room_job")
            .0;

        assert!(
            !body.contains("GLOBAL_AABB_TREE") && !body.contains("load_aabb_tree"),
            "吸收封闭性的候选面板必须来自 PanelIndex，不能回到空间树: {body}"
        );
        assert!(
            body.contains("element_candidate_panels"),
            "候选必须与元素分支同源，走库内面板几何: {body}"
        );
    }

    /// 每一种 action 都必须被某个 drain 阶段消费，且只被一个消费。
    ///
    /// 漏掉一种，那种任务入队之后就永远躺在表里，不报错也不执行；被两个阶段同时选中，
    /// 则会在同一轮里跑两遍。新增 action 时下面的 `match` 会编译失败，逼调用方明确
    /// 它归哪个阶段。
    #[test]
    fn every_action_is_consumed_by_exactly_one_drain_phase() {
        const ALL_ACTIONS: [ModelWorkAction; 7] = [
            ModelWorkAction::RegenRoot,
            ModelWorkAction::Transform,
            ModelWorkAction::DeleteCleanup,
            ModelWorkAction::CascadeExpand,
            ModelWorkAction::PostRegenAabb,
            ModelWorkAction::RoomRecalcElement,
            ModelWorkAction::RoomRecalcPanel,
        ];
        let declared_phase = |action: ModelWorkAction| match action {
            ModelWorkAction::RegenRoot => REGEN_ACTION_FILTER,
            ModelWorkAction::Transform
            | ModelWorkAction::DeleteCleanup
            | ModelWorkAction::CascadeExpand => NON_REGEN_ACTION_FILTER,
            ModelWorkAction::PostRegenAabb => POST_REGEN_AABB_ACTION_FILTER,
            ModelWorkAction::RoomRecalcElement | ModelWorkAction::RoomRecalcPanel => {
                ROOM_ACTION_FILTER
            }
        };

        for action in ALL_ACTIONS {
            let quoted = format!("'{}'", action.as_str());
            let declared = declared_phase(action);
            assert!(
                declared.contains(&quoted),
                "{quoted} 不在它声明的阶段白名单里: {declared}"
            );
            for other in [
                NON_REGEN_ACTION_FILTER,
                REGEN_ACTION_FILTER,
                POST_REGEN_AABB_ACTION_FILTER,
                ROOM_ACTION_FILTER,
            ] {
                assert!(
                    other == declared || !other.contains(&quoted),
                    "{quoted} 同时落在两个阶段里: {other}"
                );
            }
        }
    }

    #[test]
    fn drain_select_leaves_dead_letters_in_the_table() {
        assert_eq!(
            DRAIN_PAGE_SIZE, 16,
            "非 regen 阶段的空闲消化按有界页让位（页间可插入新批次）"
        );
        assert_eq!(
            REGEN_DRAIN_PAGE_SIZE, 100,
            "初始化 regen 页与底层几何生成的 100 根分块对齐"
        );
        assert_eq!(POST_REGEN_AABB_PAGE_SIZE, 256);
        let sql = render_drain_select("AND action = 'regen_root'", Some(DRAIN_PAGE_SIZE));
        assert!(
            sql.contains(&format!("(attempts?:0) < {MAX_ATTEMPTS}")),
            "{sql}"
        );
        assert!(sql.contains("status IN ['pending', 'failed']"), "{sql}");
        assert!(sql.contains("AND action = 'regen_root'"), "{sql}");
        assert!(
            sql.contains("source_end_sesno_time IS NONE AS legacy_source_time"),
            "current saves need an explicit priority bucket: {sql}"
        );
        assert!(
            sql.contains("updated_at IS NONE AS legacy_timestamp"),
            "legacy rows need an explicit sort bucket: {sql}"
        );
        assert!(
            sql.contains("ORDER BY legacy_source_time ASC, source_end_sesno_time DESC, legacy_timestamp ASC, updated_at DESC, id ASC"),
            "work from the newest actual save must run before old and legacy backlog, with deterministic tie-breaking: {sql}"
        );
        assert!(
            sql.contains(&format!("LIMIT {DRAIN_PAGE_SIZE}")),
            "one idle drain must be bounded so newly queued batches get another turn: {sql}"
        );
        assert!(
            !render_drain_select("AND action = 'regen_root'", None).contains("LIMIT"),
            "explicit/manual drain keeps its drain-until-complete contract"
        );
    }

    #[test]
    fn automatic_regen_and_aabb_drains_are_bounded_and_barriered() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("pub async fn drain_data_phases_disposition(")
            .expect("automatic data-phase drain exists")
            .1
            .split_once("pub async fn drain_data_phases(")
            .expect("compatibility wrapper follows the drain")
            .0;

        assert!(
            body.contains("Some(crate::options::model_regen_claim_page())"),
            "automatic regen must never use an unbounded SELECT: {body}"
        );
        assert!(
            body.contains("Some(crate::options::model_post_regen_aabb_page())"),
            "automatic post-regen AABB must be bounded: {body}"
        );
        let regen_drain = body
            .find("Some(crate::options::model_regen_claim_page())")
            .expect("bounded regen call");
        let second_probe = body[regen_drain..]
            .find("has_pending_work(REGEN_ACTION_FILTER)")
            .map(|offset| regen_drain + offset)
            .expect("regen must be re-probed after one page");
        let aabb_drain = body
            .find("Some(crate::options::model_post_regen_aabb_page())")
            .expect("bounded AABB call");
        assert!(
            regen_drain < second_probe && second_probe < aabb_drain,
            "AABB cannot cross a non-empty regen barrier: {body}"
        );
    }

    /// ADR-054 实施约束 4：凭证判据是单调的。**改回 `== 水位` 会让「107 覆盖 106」那一句红。**
    /// 等值判据会把 ensure 按文件最新会话生成过的根，在管线按窗口右端复核时判成过期。
    #[test]
    fn generation_root_completion_covers_the_required_session_monotonically() {
        let mut row = GenRootCoverageRow {
            pe: surrealdb::sql::Thing::from(("pe", "24381_1")),
            noun: "EQUI".into(),
            status: Some("Generated".into()),
            source_end_sesno: Some(106),
            source_end_sesno_time: Some("2026-08-14T05:21:19+00:00".into()),
        };
        assert!(gen_root_credential_is_current(&row, 106));
        row.source_end_sesno = Some(107);
        assert!(
            gen_root_credential_is_current(&row, 106),
            "按更新的文件会话生成过的根覆盖更旧的窗口右端"
        );
        row.source_end_sesno = Some(105);
        assert!(!gen_root_credential_is_current(&row, 106));
        // 人工强制重试行的 0 是「未认领」，不是第 0 版。
        row.source_end_sesno = Some(0);
        assert!(!gen_root_credential_is_current(&row, 0));
        row.source_end_sesno = None;
        assert!(!gen_root_credential_is_current(&row, 106));
        row.source_end_sesno = Some(106);
        row.status = Some("Failed".into());
        assert!(!gen_root_credential_is_current(&row, 106));
    }

    /// 目录落款真跑一遍聚合语句：三个根、两个 CATA 库、同一个库两个不同会话，
    /// 取回来必须是**每库一条、会话取最旧**；没有目录段的根不能把它带崩。
    #[tokio::test]
    async fn published_catalogue_sessions_take_the_oldest_recorded_session_per_db() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("published_catalogue")
            .await
            .expect("select fixture db");
        db.query(
            "CREATE gen_root:a SET dbnum = 8000, published_model_target = { \
               catalogue: [ \
                 { dbnum: 7000, db_type: 'CATA', file: '/cat7000', session: 12, digest: 'd1' }, \
                 { dbnum: 7001, db_type: 'CATA', file: '/cat7001', session: 3, digest: 'd2' } \
               ] };\
             CREATE gen_root:b SET dbnum = 8000, published_model_target = { \
               catalogue: [ \
                 { dbnum: 7000, db_type: 'CATA', file: '/cat7000', session: 9, digest: 'd3' } \
               ] };\
             CREATE gen_root:c SET dbnum = 1112, status = 'Generated';",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let recorded = published_catalogue_sessions_on(&db)
            .await
            .expect("read catalogue stamps");
        assert_eq!(
            recorded,
            BTreeMap::from([(7000, 9), (7001, 3)]),
            "每库一条、取最旧的那一版：问的是「有没有根落在旧目录上」"
        );
    }

    /// 一条目录落款都没有时是空表，不是错——零解析 / 从没发布过的库就长这样。
    #[tokio::test]
    async fn published_catalogue_sessions_are_empty_when_nothing_was_published() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("published_catalogue_empty")
            .await
            .expect("select fixture db");
        db.query("CREATE gen_root:a SET dbnum = 8000, status = 'Generated';")
            .await
            .expect("seed")
            .check()
            .expect("seed statements");

        assert!(
            published_catalogue_sessions_on(&db)
                .await
                .expect("read catalogue stamps")
                .is_empty()
        );
    }

    /// 「已确认过期」与「不知道」必须分开数：文件读不到的库进 unreadable 那一栏，
    /// 一个都不许混进过期名单——混进去就是拿未知冒充结论。
    #[test]
    fn a_catalogue_that_cannot_be_read_is_unknown_not_stale() {
        let recorded = BTreeMap::from([(7000, 9), (7001, 3), (7002, 40), (7003, 7)]);
        let (stale, unreadable) = stale_catalogue_dbnums(&recorded, |dbnum| match dbnum {
            7000 => Some(12), // 前移了 → 过期
            7001 => Some(3),  // 追平
            7002 => None,     // 不在当前 MDB → 未知
            _ => Some(5),     // 文件比记录旧 → 不算过期（回退由数据面管）
        });
        assert_eq!(stale, vec![7000]);
        assert_eq!(unreadable, 1);
    }

    /// 目录那一格不许染指 `result`。`result` 说的是「这次复核跑成没跑成」，目录前移是
    /// 在案的能力缺口（2026-09-04 启动增量审核 #4）——混进去，每一次有目录变更的启动
    /// 都会被报成失败态，人就再也分不清哪次是真出岔子。
    #[test]
    fn the_catalogue_flag_is_named_separately_and_never_downgrades_the_result() {
        // 这棵树 CRLF / LF 混用，`include_str!` 原样带回来；探针按 LF 写，先归一。
        // 启动复核自 S5a 起住在 `startup_reconcile.rs`，`inner` 与取重的纯函数都在那里。
        let source = include_str!("startup_reconcile.rs").replace("\r\n", "\n");
        let inner = source
            .split_once("async fn reconcile_model_coverage_at_startup_inner()")
            .expect("inner exists")
            .1;
        // P3 起这一位 = 「前移了且反查没能精确回答」，两个出口都从同一个 `is_degraded()` 取值。
        assert!(
            inner.contains(".is_some_and(|cascade| cascade.is_degraded());"),
            "目录那一位要从反查回执的 is_degraded() 折出来"
        );
        assert!(
            !inner.contains("catalogue_cascade_degraded: !stale_catalogue.is_empty()"),
            "P3 之后「目录前移」本身不再是退化：精确命中只排引用根的启动这一位为假"
        );
        assert!(
            inner.contains(
                "coverage.reconcile.catalogue_cascade_degraded = catalogue_cascade_degraded;"
            ) && inner.contains("        catalogue_cascade_degraded,\n"),
            "两个出口都要带上目录那一位"
        );
        // 取重 2026-09-08 起是纯函数 `startup_reconcile_result(window_failed, unreadable)`（09-04 审核 #7）：
        // 入参里既没有目录那一格、也没有 `not_in_mdb`，函数体里也不许出现。
        assert!(
            inner.contains("result: startup_reconcile_result(window_failed, unreadable),"),
            "`result` 必须由那个纯函数给出"
        );
        let arms = source
            .split_once("fn startup_reconcile_result(window_failed: usize, unreadable: usize)")
            .expect("取重的纯函数存在")
            .1;
        let arms = &arms[..arms.find("\n}\n").expect("函数体收尾")];
        assert!(
            !arms.contains("catalogue") && !arms.contains("not_in_mdb"),
            "`result` 的取重不许看目录那一格、也不看不在 MDB 的历史行: {arms}"
        );
        for probe in ["if window_failed > 0 {", "} else if unreadable > 0 {"] {
            assert!(arms.contains(probe), "取重顺序不变：{probe}");
        }
    }

    /// 未限定域启动核对的清单来自 gen_root 的 GROUP BY；`dbnum` 缺失的历史行核对
    /// 不了归属，必须被跳过而不是 panic 或伪装成 dbnum=0。
    #[tokio::test]
    async fn coverage_tracked_dbnums_lists_each_dbnum_once_and_skips_unowned_rows() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("coverage_tracked").await.unwrap();
        db.query(
            "CREATE gen_root:a SET dbnum = 8000, status = 'Generated';\
             CREATE gen_root:b SET dbnum = 8000, status = 'Pending';\
             CREATE gen_root:c SET dbnum = 1112, status = 'Generated';\
             CREATE gen_root:d SET status = 'Generated';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let tracked = coverage_tracked_dbnums_on(&db).await.unwrap();
        assert_eq!(tracked, vec![1112, 8000]);
    }

    /// 一级过滤的取数（ADR-058 D5）真的跑一遍。`include_str!` 护栏钉得住调用顺序，
    /// 钉不住这两条聚合本身——而它们一旦算错，落后的库会被静默跳过，谁都不会报错。
    ///
    /// 三件事一起验：安定行取**最小**凭证当下限、未安定行（失败态与 `source_end_sesno = 0`
    /// 的未认领行）不进下限但要计数、`dbnum` 缺失的历史行整行跳过。
    #[tokio::test]
    async fn startup_floors_take_the_settled_minimum_and_count_the_unsettled_separately() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("startup_floors").await.unwrap();
        db.query(
            "CREATE gen_root:a SET dbnum = 8000, status = 'Generated', source_end_sesno = 106;\
             CREATE gen_root:b SET dbnum = 8000, status = 'AlreadyAvailable', source_end_sesno = 100;\
             CREATE gen_root:c SET dbnum = 8000, status = 'Failed', source_end_sesno = 120;\
             CREATE gen_root:d SET dbnum = 8000, status = 'Generated', source_end_sesno = 0;\
             CREATE gen_root:e SET dbnum = 1112, status = 'NoRenderableGeometry', source_end_sesno = 45;\
             CREATE gen_root:f SET dbnum = 7998, status = 'Pending';\
             CREATE gen_root:g SET status = 'Generated', source_end_sesno = 9;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let floors = startup_coverage_floors_on(&db).await.unwrap();

        assert_eq!(
            floors.keys().copied().collect::<Vec<_>>(),
            vec![1112, 7998, 8000]
        );
        let ams = floors[&8000];
        assert_eq!(
            ams.settled_floor,
            Some(100),
            "下限取安定行的最小值，失败态那条 120 不算数"
        );
        assert_eq!(ams.settled_roots, 2);
        assert_eq!(
            ams.unsettled_roots, 2,
            "Failed 与未认领的 source_end_sesno = 0 都算未安定"
        );

        let single = floors[&1112];
        assert_eq!(single.settled_floor, Some(45));
        assert_eq!(single.unsettled_roots, 0);

        // 一条安定行都没有的库：下限是 None（不是 0），一级过滤据此判它必须开窗。
        let never = floors[&7998];
        assert_eq!(never.settled_floor, None);
        assert_eq!(never.settled_roots, 0);
        assert_eq!(never.unsettled_roots, 1);
    }

    /// `gen_root` 行的库归属只有顶层 `dbnum` 一处真值。建行时不写，按库取数的每一处
    /// （启动凭证下限、覆盖清单、`/dbnums` 的模型凭证下限）都会静默少算这一行。
    #[test]
    fn a_regen_upsert_stamps_the_gen_root_row_with_its_dbnum() {
        let statement = render_upsert_no_time(&ModelWorkItem {
            dbnum: 8000,
            db_type: "DESI".into(),
            source_end_sesno: 276,
            action: ModelWorkAction::RegenRoot,
            target_refno: "24384/1004".into(),
            noun: "BRAN".into(),
        });
        let gen_root = statement
            .split_once("UPSERT type::thing('gen_root'")
            .expect("regen 必须同时落 gen_root 行")
            .1;
        assert!(
            gen_root.contains("dbnum = 8000"),
            "顶层 dbnum 必须在建行时就写: {gen_root}"
        );
    }

    /// 回填只动「只有嵌套归属」的历史行：已有顶层归属的不碰，两处都没有的也不碰
    /// （没有可信的归属可补，硬写就是编一个）。跑第二遍必须一行都不再命中。
    #[tokio::test]
    async fn the_dbnum_backfill_fixes_only_the_nested_only_rows_and_is_idempotent() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("gen_root_backfill").await.unwrap();
        db.query(
            "CREATE gen_root:nested SET desired_target = { dbnum: 8000, source_end_sesno: 271 };\
             CREATE gen_root:already SET dbnum = 1112, \
                 desired_target = { dbnum: 1112, source_end_sesno: 45 };\
             CREATE gen_root:orphan SET desired_revision = 1;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert_eq!(backfill_gen_root_dbnum_on(&db).await.unwrap(), 1);
        assert_eq!(
            backfill_gen_root_dbnum_on(&db).await.unwrap(),
            0,
            "补完就不该再命中"
        );

        #[derive(Debug, Deserialize)]
        struct Row {
            #[allow(dead_code)]
            id: surrealdb::sql::Thing,
            #[serde(default)]
            dbnum: Option<u32>,
        }
        // SurrealDB 2.1 要求 ORDER BY 的字段也出现在投影里（同 `gen_root` 覆盖查询那条）。
        let mut response = db
            .query("SELECT id, dbnum FROM gen_root ORDER BY id;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<Row> = response.take(0).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.dbnum).collect::<Vec<_>>(),
            vec![Some(1112), Some(8000), None],
            "already / nested / orphan 三行分别是：不动、补上、仍然没有"
        );
    }

    // 启动复核自 S5a 起住在 `startup_reconcile.rs`（本文件只再导出它）。判据没变，换的只是
    // 读哪个文件：那个模块从这个入口一直到文件末尾就是原来那一段，不再需要下界标记。
    fn startup_reconcile_body() -> &'static str {
        include_str!("startup_reconcile.rs")
            .split_once("pub async fn reconcile_model_coverage_at_startup()")
            .expect("startup reconcile exists")
            .1
    }

    /// ADR-050 的补偿路径必须两头都接上：
    /// 1. 覆盖核对**不得**只在监听限定域下运行——启动清空是无条件的，补偿也得是；
    /// 2. 核对失败**不得**中止启动（`?` 上抛就是把一个库的配置错变成整个服务的崩溃
    ///    循环），失败按「覆盖未确认」处理并当面播报。
    #[test]
    fn startup_coverage_reconcile_covers_unscoped_and_never_aborts_startup() {
        let body = startup_reconcile_body();
        assert!(
            !body.contains(".await?"),
            "启动覆盖核对不许把错误上抛中止启动，逐库告警后继续: {body}"
        );
        assert!(
            body.contains("startup_coverage_floors()"),
            "一级过滤必须一次问完全部库的凭证下限: {body}"
        );
        assert!(
            body.contains("BatchIntent::ModelOnly"),
            "发现落后必须排 ModelOnly Task，不能只报数: {body}"
        );

        let startup = include_str!("../lib.rs");
        let call = startup
            .find("reconcile_model_coverage_at_startup().await")
            .expect("启动序列必须接入覆盖核对");
        assert!(
            !startup.contains("startup_data_ready && crate::data_interface::watch_scope::active()"),
            "覆盖核对不得只挂在监听限定域上（ADR-050 的清空是无条件的，补偿也得是）"
        );
        // 调用必须在 run_cli 顶层：4 空格缩进意味着它不在任何 `if` 块内，compensation
        // 才真正无条件（2026-09-03 之前它被 `startup_data_ready` 把门，`autorun=false`
        // 且无 watcher 的形态下一次都不跑，与本测试第一半直接矛盾）。
        assert!(
            startup.contains(
                "\n    crate::data_interface::startup_reconcile::reconcile_model_coverage_at_startup().await;\n"
            ),
            "覆盖核对必须在 run_cli 顶层无条件执行，不在任何 if 块内"
        );
        // 顶层缩进这一条同时排除了两道历史门：任何 `if !direct_read_mode` /
        // `if startup_data_ready` 包裹都会把调用行压到 8 空格。核对读的是文件与
        // `gen_root`，不需要任何数据摄入，direct 正是它的主场（ADR-058 D8 / 审核 A1）。
        let call_line = startup[..call]
            .rfind('\n')
            .map(|line| &startup[line + 1..call]);
        assert_eq!(
            call_line,
            Some("    crate::data_interface::startup_reconcile::"),
            "覆盖核对调用必须顶格在 run_cli 函数体层级"
        );
    }

    #[test]
    fn startup_reconcile_always_settles_the_coordinator() {
        let body = startup_reconcile_body();
        let run = body
            .find("reconcile_model_coverage_at_startup_inner().await;")
            .expect("outer shell must run the reconcile");
        let settle = body
            .find("coordinator.settle_startup_reconcile();")
            .expect("outer shell must settle the coordinator");
        let inner = body
            .find("async fn reconcile_model_coverage_at_startup_inner()")
            .expect("inner reconcile exists");
        assert!(
            run < settle && settle < inner,
            "settlement must stay in the outer shell so every inner return reaches it: {body}"
        );
        assert!(
            body[run..inner].contains("BatchScheduler::global().wake();"),
            "settling startup reconcile must wake the worker immediately: {body}"
        );
    }

    /// Startup recovery uses the applied watermark as its frozen completion target.
    ///
    /// `fn::sync_gen_roots` → `fn::gen_root_cover` 第一句就是
    /// `select value id from pe where dbnum = …`（ADR-056 N7 点名的那一处），
    /// 而 `applied_sesno` 只承诺数据落库、不是生成时点（ADR-054）。两者任一回到这条
    /// 函数里，启动就又变回旧架构，而且不会有任何东西报错。
    ///
    /// **2026-09-04 补一句边界**：ADR-060 D6 的模型水位对账**确实**要读 `applied_sesno`
    /// （它是那枚水位的封顶，M2），但它读的是「模型追平到哪一版数据」这件事，与本条
    /// 守的「拿什么当生成时点」无关。两者不许混：对账整体委派给
    /// [`reconcile_model_watermarks`]，这条函数体里一个 `applied_sesno` 都不许出现，
    /// 谁想图省事把那一句读回内联进来，本测试就红。
    #[test]
    fn startup_reconcile_uses_strict_watermarks_without_pe_backed_coverage() {
        let body = startup_reconcile_body();
        assert!(
            !body.contains("sync_gen_roots") && !body.contains("sync_and_seed_model_coverage"),
            "启动路径不得再经 pe 打头的覆盖物化（ADR-058 D4 / ADR-056 约束 9）: {body}"
        );
        assert!(
            body.contains("probe.recorded < state.applied_sesno"),
            "启动必须按严格库水位缺口排恢复: {body}"
        );
        assert!(
            body.contains("latest_source_version(dbnum)"),
            "右端必须取文件最新会话: {body}"
        );
    }

    /// ADR-060 D10：兼容接管的库要在取文件最新、开窗之前被认出并跳过，且必须出声、进汇总行。
    #[test]
    fn startup_reconcile_skips_legacy_adopted_libraries_before_opening_a_window() {
        let body = startup_reconcile_body();
        let adopted = body
            .find("probe.legacy_adopted()")
            .expect("接管判定必须挂进启动");
        let lookup = body.find("source_file_lookup(dbnum)").expect("右端查找");
        let window = body
            .find("BatchIntent::ModelOnly")
            .expect("二级恢复 Task 必须存在");
        assert!(
            adopted < lookup && lookup < window,
            "接管判定要排在取文件最新与开窗之前: {body}"
        );
        assert!(
            body.contains("reason=legacy_adopted"),
            "接管必须逐库出声: {body}"
        );
        assert!(
            body.contains("兼容接管 {legacy_adopted} 库"),
            "接管要进核对汇总行: {body}"
        );
    }

    /// 初始化发布 run 已退役（ADR-071 S5，d-300）：启动核对不再问「这个库有没有保留的 run」
    /// 或「能不能接管它」，恢复 Task 只看镜像里的凭证下限与文件最新。回来一个即红。
    #[test]
    fn startup_reconcile_no_longer_consults_a_retained_publish_run() {
        let body = startup_reconcile_body();
        for retired in [
            "can_adopt_retained_initialization(",
            "blocks_dbnum(",
            "initialization_source(",
        ] {
            assert!(
                !body.contains(retired),
                "启动核对不得再问发布 run：{retired}"
            );
        }
        assert!(
            body.contains("BatchIntent::ModelOnly"),
            "二级恢复 Task 仍然存在: {body}"
        );
    }

    /// ADR-060 D6/D7：对账必须挂进启动，且覆盖面要包含**没有凭证的库**。
    ///
    /// 无凭证库包含两类：既无 `gen_root` 也无模型投影的幽灵水位，以及已有属性和模型投影、
    /// 但历史版本还没有水位追踪的兼容库。`floors` 是一条 `GROUP BY gen_root`，两类都不会
    /// 出现在里面；只对 `floors` 对账就会同时漏掉清理与兼容初始化。
    #[test]
    fn startup_reconcile_checks_the_model_watermark_including_the_libraries_without_credentials() {
        let body = startup_reconcile_body();
        let call = body
            .find("reconcile_model_watermarks(")
            .expect("模型水位对账必须挂进启动");
        assert!(
            body.contains("let candidate_dbnums = mdb_design_dbnums.clone();")
                && body[call..].contains("candidate_dbnums.iter().copied()"),
            "对账覆盖面必须并上没有凭证的库，否则幽灵模型水位永远查不出来: {body}"
        );
        let window = body
            .find("BatchIntent::ModelOnly")
            .expect("二级恢复 Task 必须存在");
        assert!(
            call < window,
            "对账只读既有属性、模型投影、gen_root 与水位行，排在开窗之前，别和选根搅在一起: {body}"
        );
    }

    /// ADR-058 实施约束 2：两级必须真的是两级。
    ///
    /// 一级是一次聚合取数，二级才开窗；「不落后就跳过」这一步必须排在开窗之前，否则
    /// 448 库全都要开一次 `collect_window`——而那正是这次重构要省掉的那笔钱。
    #[test]
    fn startup_reconcile_opens_a_window_only_for_the_libraries_that_lag() {
        let body = startup_reconcile_body();
        let floors = body
            .find("startup_coverage_floors()")
            .expect("一级取数必须存在");
        let skip = body.find("if !behind {").expect("一级过滤的跳过必须存在");
        let window = body
            .find("BatchIntent::ModelOnly")
            .expect("二级恢复 Task 必须存在");
        assert!(
            floors < skip && skip < window,
            "顺序必须是 一次取下限 → 不落后就跳过 → 才开窗: {body}"
        );
    }

    /// 09-04 审核 #6：「哪些库还没有凭证记录」的分母是 **MDB 声明的 DESI**，不是数据面的
    /// 登记表。登记表里有 SYST / DICT / CATA 与换过 MDB 之后的历史行，拿它当分母会把没有
    /// 模型面的库报成「无凭证」——而这一栏是 `/health.startup` 上给人看的。
    #[test]
    fn the_no_credential_column_is_measured_against_the_mdb_not_the_registry() {
        let body = startup_reconcile_body()
            .split_once("async fn dbnums_without_credentials(")
            .expect("无凭证库那一栏必须存在")
            .1
            .split_once("enum StartupTargetLookup")
            .expect("其后是右端查找的三态")
            .0;
        assert!(
            !body.contains("list_registered"),
            "分母不许再是 DbnumState 登记表: {body}"
        );
        assert!(
            body.contains("mdb_design_dbnums"),
            "分母必须来自 model_source::mdb_design_dbnums（与右端「文件最新」同一份权威）: {body}"
        );
        assert!(
            body.contains("watch_scope::admits("),
            "监听限定域照样套在这一栏上: {body}"
        );
    }

    /// 09-04 审核 #7：「不在当前 MDB」与「取不到文件最新」是两栏。
    ///
    /// 前者是换过 MDB 之后留下的历史 `gen_root` 行，没有文件可核，只能点名；后者才是
    /// 「这一轮没确认」。混成一栏的后果是 `gen_root` 里只要留着一行历史库，
    /// `/health.startup.reconcile.result` 就一辈子出不来 `ok`。
    #[test]
    fn startup_reconcile_names_libraries_outside_the_mdb_separately_from_unreadable() {
        // 取重是纯函数：不在 MDB 的库数根本不是入参。
        assert_eq!(startup_reconcile_result(0, 0), "ok");
        assert_eq!(startup_reconcile_result(0, 3), "unreadable");
        assert_eq!(startup_reconcile_result(1, 3), "window_failed");
        assert_eq!(startup_reconcile_result(2, 0), "window_failed");

        let body = startup_reconcile_body();
        let lookup = body
            .find("source_file_lookup(dbnum)")
            .expect("右端之前必须先问「在不在当前 MDB」");
        let latest = body
            .find("latest_source_version(dbnum)")
            .expect("右端仍取文件最新会话");
        assert!(lookup < latest, "先分栏再取右端: {body}");
        assert!(
            body.contains("Ok(StartupTargetLookup::NotInMdb) =>")
                && body.contains("not_in_mdb.push(dbnum)"),
            "不在 MDB 的库要单独计数: {body}"
        );
        assert!(
            body.contains("result: startup_reconcile_result(window_failed, unreadable)"),
            "result 只由开窗失败与未确认取重: {body}"
        );
        assert!(
            body.contains("not_in_mdb: not_in_mdb.len()"),
            "那一栏要进 StartupReconcile 给 /health 看: {body}"
        );
        let unreadable_default =
            crate::data_interface::initialization_phase::StartupReconcile::default();
        assert_eq!(unreadable_default.not_in_mdb, 0);
        assert_eq!(unreadable_default.result, "pending");
    }

    #[test]
    fn startup_recovery_uses_the_root_credential_floor_after_watermark_correction() {
        assert_eq!(model_recovery_base(0, Some(155), false), 155);
        assert_eq!(model_recovery_base(120, Some(155), false), 155);
        assert_eq!(model_recovery_base(120, None, false), 120);
        assert_eq!(
            model_recovery_base(120, Some(155), true),
            0,
            "a genuinely invalid source-bound completion receipt still requires full revalidation"
        );
    }

    #[test]
    fn coverage_query_selects_the_ordering_field_required_by_surreal() {
        let body = include_str!("model_update_pending.rs")
            .split_once("pub async fn sync_and_seed_model_coverage(")
            .expect("coverage seeding entrypoint")
            .1
            .split_once("const STARTUP_CATALOGUE_WINDOW_RECORD_CAP")
            .expect("coverage query boundary")
            .0;
        assert!(
            body.contains(
                "SELECT pe, noun, subtree, status, source_end_sesno, source_end_sesno_time"
            ),
            "SurrealDB 2.1 requires ORDER BY subtree to also appear in the SELECT projection"
        );
        assert!(body.contains("ORDER BY subtree DESC"));
    }

    #[test]
    fn successful_regen_writes_the_completion_credential_before_deleting_pending() {
        let sql = render_complete_regen_revision("24381/100817", 7);
        let credential = sql.find("UPDATE type::thing('gen_root'").unwrap();
        let deletion = sql.find("DELETE model_update_pending").unwrap();
        assert!(
            credential < deletion,
            "completion credential must precede settlement: {sql}"
        );
        assert!(sql.contains("source_end_sesno = $work.source_end_sesno?:0"));
        assert!(sql.contains("(revision?:0) = 7"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn completion_credential_and_pending_settlement_commit_together_on_surreal() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("regen_credential")
            .await
            .expect("select fixture db");
        // 行 id 必须是入队方写的那一个（`record_id_of`）：收口按 id 取行，谓词只做校验。
        db.query(
            "CREATE model_update_pending:regen_root_24381_100817 SET action='regen_root', \
             target_refno='24381/100817', revision=7, source_end_sesno=106, \
             source_end_sesno_time='2026-08-14T05:21:19+00:00'; \
             CREATE gen_root:24381_100817 SET status='Pending';",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");
        db.query(format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            render_complete_regen_revision("24381/100817", 7)
        ))
        .await
        .expect("settle")
        .check()
        .expect("settlement SQL parses");
        let mut response = db
            .query(
                "SELECT status, source_end_sesno, source_end_sesno_time FROM gen_root:24381_100817; \
                 RETURN array::len(SELECT VALUE id FROM model_update_pending);",
            )
            .await
            .expect("verify")
            .check()
            .expect("verify statements");
        let roots: Vec<serde_json::Value> = response.take(0).expect("root row");
        let pending: Option<usize> = response.take(1).expect("pending count");
        assert_eq!(roots[0]["status"], "Generated");
        assert_eq!(roots[0]["source_end_sesno"], 106);
        assert_eq!(pending, Some(0));
    }

    #[test]
    fn model_drain_task_detail_keeps_only_ten_root_samples() {
        let jobs = (0..101)
            .map(|index| PendingModelWork {
                dbnum: 7997,
                db_type: "DESI".into(),
                action: ModelWorkAction::RegenRoot,
                target_refno: format!("24381/{index}"),
                noun: "EQUI".into(),
                source_end_sesno: 106,
                source_end_sesno_time: None,
                status: "pending".into(),
                attempts: 0,
                last_error: None,
                revision: 1,
                required_panels: Vec::new(),
            })
            .collect::<Vec<_>>();
        let detail = model_drain_detail(&jobs, 3, 18_674);
        assert_eq!(detail["queue_total"], 18_674);
        assert_eq!(detail["page_claimed"], 101);
        assert_eq!(detail["roots"].as_array().unwrap().len(), 10);
        assert_eq!(detail["root_samples_truncated"], 91);
        assert!(serde_json::to_vec(&detail).unwrap().len() < 256 * 1024);
    }

    #[test]
    fn maximum_tasks_response_stays_below_256_kib_with_bounded_model_pages() {
        let registry = crate::data_interface::task_registry::TaskRegistry::default();
        let jobs = (0..REGEN_DRAIN_PAGE_SIZE)
            .map(|index| PendingModelWork {
                dbnum: 7997,
                db_type: "DESI".into(),
                action: ModelWorkAction::RegenRoot,
                target_refno: format!("24381/{index}"),
                noun: "EQUI".into(),
                source_end_sesno: 106,
                source_end_sesno_time: None,
                status: "pending".into(),
                attempts: 0,
                last_error: None,
                revision: 1,
                required_panels: Vec::new(),
            })
            .collect::<Vec<_>>();
        for page in 0..160 {
            registry.insert_running_model_drain(
                &format!("model-size-{page:03}"),
                "AvevaMarineSample",
                REGEN_DRAIN_PAGE_SIZE as u32,
                model_drain_detail(&jobs, 3, 18_674usize.saturating_sub(page * 100)),
            );
        }

        let response = serde_json::json!({ "tasks": registry.list(None, None, 160) });
        let encoded = serde_json::to_vec(&response).unwrap();
        println!("maximum /api/v1/tasks response: {} bytes", encoded.len());
        assert!(
            encoded.len() < 256 * 1024,
            "maximum /api/v1/tasks response grew to {} bytes",
            encoded.len()
        );
        let handler = include_str!("../web_service/handlers.rs");
        assert!(
            handler.contains("query.limit.unwrap_or(50).min(160)"),
            "HTTP 入口必须使用已验证的 160 条上限"
        );
    }

    #[tokio::test]
    async fn automatic_regen_page_defers_busy_roots_without_blocking_free_roots() {
        let busy_root = "7997/900001".to_string();
        let free_root = "7997/900002".to_string();
        let held = crate::data_interface::manual_update::generation_root_lock(&busy_root)
            .lock_owned()
            .await;

        let (guards, busy) =
            acquire_regen_page_locks(&[busy_root.clone(), free_root], RegenLockPolicy::DeferBusy)
                .await;
        assert_eq!(guards.len(), 1, "free roots remain runnable");
        assert_eq!(busy, HashSet::from([busy_root.clone()]));

        drop(guards);
        drop(held);
        let (guards, busy) =
            acquire_regen_page_locks(&[busy_root], RegenLockPolicy::DeferBusy).await;
        assert_eq!(guards.len(), 1);
        assert!(busy.is_empty());
    }

    #[tokio::test]
    async fn one_hundred_and_one_regen_rows_are_selected_as_two_pages_before_aabb() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("bounded_regen_pages")
            .await
            .unwrap();
        let mut seed = String::new();
        for index in 0..101 {
            seed.push_str(&format!(
                "CREATE model_update_pending:regen_{index} SET dbnum=7997, db_type='DESI', \
                 source_end_sesno=106, action='regen_root', target_refno='24381/{index}', \
                 noun='EQUI', status='pending', attempts=0, revision=1, updated_at=time::now();"
            ));
        }
        seed.push_str(
            "CREATE model_update_pending:aabb SET dbnum=7997, db_type='DESI', \
             source_end_sesno=106, action='post_regen_aabb', target_refno='24381/500000', \
             noun='EQUI', status='pending', attempts=0, revision=1, updated_at=time::now();",
        );
        db.query(seed).await.unwrap().check().unwrap();

        let mut first = db
            .query(render_drain_select_for_scope(
                REGEN_ACTION_FILTER,
                Some(REGEN_DRAIN_PAGE_SIZE),
                &[7997],
                &[],
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let first_page: Vec<PendingModelWork> = first.take(0).unwrap();
        assert_eq!(first_page.len(), 100);

        let targets = first_page
            .iter()
            .map(|job| format!("'{}'", job.target_refno))
            .collect::<Vec<_>>()
            .join(",");
        db.query(format!(
            "DELETE model_update_pending WHERE action='regen_root' AND target_refno IN [{targets}]"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let mut second = db
            .query(render_drain_select_for_scope(
                REGEN_ACTION_FILTER,
                Some(REGEN_DRAIN_PAGE_SIZE),
                &[7997],
                &[],
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let second_page: Vec<PendingModelWork> = second.take(0).unwrap();
        assert_eq!(second_page.len(), 1);

        let mut aabb = db
            .query(render_drain_select_for_scope(
                POST_REGEN_AABB_ACTION_FILTER,
                Some(POST_REGEN_AABB_PAGE_SIZE),
                &[7997],
                &[],
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let aabb_page: Vec<PendingModelWork> = aabb.take(0).unwrap();
        assert_eq!(aabb_page.len(), 1);
    }

    #[tokio::test]
    async fn one_cohort_is_claimed_whole_even_when_it_crosses_the_page_limit() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("cohort_claim").await.unwrap();
        db.query(
            "CREATE model_update_pending:first SET dbnum=7997, db_type='DESI', \
             source_end_sesno=106, cohort_id='7997:106', action='regen_root', \
             target_refno='24381/1', noun='BRAN', status='pending', attempts=0, revision=1; \
             CREATE model_update_pending:second SET dbnum=7997, db_type='DESI', \
             source_end_sesno=106, cohort_id='7997:106', action='regen_root', \
             target_refno='24381/2', noun='BRAN', status='pending', attempts=0, revision=1; \
             CREATE model_update_pending:other SET dbnum=7997, db_type='DESI', \
             source_end_sesno=105, cohort_id='7997:105', action='regen_root', \
             target_refno='24381/3', noun='BRAN', status='pending', attempts=0, revision=1;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let jobs = claim_page_with_cohorts_on(
            &db,
            REGEN_ACTION_FILTER,
            Some(1),
            &[7997],
            &[],
            "test-worker",
        )
        .await
        .unwrap();
        assert_eq!(
            jobs.len(),
            2,
            "the page boundary must expand to the whole cohort"
        );
        assert!(jobs.iter().all(|job| job.source_end_sesno == 106));
    }

    #[tokio::test]
    async fn yielded_claims_release_without_touching_new_revisions_or_other_owners() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("yield_release").await.unwrap();
        for suffix in 1..=3 {
            db.query(format!("CREATE model_update_pending:regen_root_24381_{suffix} SET dbnum=7997, db_type='DESI', source_end_sesno=106, action='regen_root', target_refno='24381/{suffix}', noun='BRAN', status='pending', attempts=0, revision=1;")).await.unwrap().check().unwrap();
        }
        let jobs = claim_page_with_cohorts_on(&db, "", Some(3), &[7997], &[], "owner-a")
            .await
            .unwrap();
        assert_eq!(jobs.len(), 3);
        db.query("UPDATE model_update_pending:regen_root_24381_2 SET lease_owner='owner-b'; UPDATE model_update_pending:regen_root_24381_3 SET revision=2;").await.unwrap().check().unwrap();
        release_unsettled_claims_on(&db, &jobs, "owner-a")
            .await
            .unwrap();
        let mut response = db.query("SELECT VALUE [status, attempts] FROM model_update_pending:regen_root_24381_1; SELECT VALUE status FROM [model_update_pending:regen_root_24381_2, model_update_pending:regen_root_24381_3];").await.unwrap().check().unwrap();
        let first: Vec<(String, u32)> = response.take(0).unwrap();
        let protected: Vec<String> = response.take(1).unwrap();
        assert_eq!(first, vec![("pending".into(), 0)]);
        assert_eq!(protected, vec!["running", "running"]);
    }

    #[test]
    fn execution_groups_bound_large_cohorts() {
        let job = |target: &str, sesno| PendingModelWork {
            dbnum: 7997,
            db_type: "DESI".into(),
            source_end_sesno: sesno,
            source_end_sesno_time: None,
            action: ModelWorkAction::RegenRoot,
            target_refno: target.into(),
            noun: "BRAN".into(),
            status: "running".into(),
            attempts: 0,
            last_error: None,
            revision: 1,
            required_panels: Vec::new(),
        };
        let groups = regen_execution_groups(
            vec![
                job("24381/1", 106),
                job("24381/9", 105),
                job("24381/2", 106),
            ],
            1,
        );
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|group| group.len() == 1));
        let cohort = groups
            .iter()
            .find(|group| group[0].source_end_sesno == 106)
            .expect("bounded cohort group");
        assert!(
            should_defer_publication_cohort(cohort, true),
            "one busy root must defer its bounded publication group"
        );
    }

    /// 空闲页也要合批，不能一根一根地调生成。
    ///
    /// 这条路径过去逐根调 `generate_roots(&[one])`：[`DRAIN_PAGE_SIZE`] 付了页大小的
    /// 代价，摊薄却一次都没发生。现场量到每页 16.6 秒、每根 1.04 秒且九页几乎一模
    /// 一样，而进程只吃 11.8% 单核——那是 16 次流水线启动的串行往返，不是几何算力。
    #[test]
    fn the_idle_page_generates_its_regen_roots_in_one_batch() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("async fn drain_where_cooperative(")
            .expect("cooperative drain must exist")
            .1
            .split_once("fn render_scoped_room_select(")
            .expect("cooperative drain must end before the scoped room select")
            .0;

        let batch_at = body
            .find("run_regen_batch(")
            .expect("空闲页必须走合批，逐根调用会把生成启动开销按根重付一遍");
        let loop_at = body
            .find("for (index, job) in remaining")
            .expect("合批之外的根仍要逐根跑");
        assert!(
            batch_at < loop_at,
            "合批要先于逐根循环：回退下来的根才能接着享受根间让位检查: {body}"
        );
    }

    /// 每一段都要先探再取。
    ///
    /// drain 那条查询的 `ORDER BY` 里有两个当场算出来的 `IS NONE` 派生列，天生不可
    /// 索引，而 `model_update_pending` 上一个索引都没有：取 16 行要把全表扫排一遍，
    /// 现场 1.9 万行量到 260ms。`has_pending_work` 是 LIMIT 1 不排序的 1ms 查询。
    /// 今天非 regen 段与 post_regen_aabb 段的积压长期是 0，不探就是每个空闲轮白付
    /// 两次全表排序。
    #[test]
    fn each_data_phase_probes_before_it_drains() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("pub async fn drain_data_phases_disposition(")
            .expect("data phase drain must exist")
            .1
            .split_once("pub async fn drain_data_phases(")
            .expect("disposition 之后是只要计数的兼容包装")
            .0;
        let compact = body.split_whitespace().collect::<String>();

        for filter in [
            "NON_REGEN_ACTION_FILTER",
            "REGEN_ACTION_FILTER",
            "POST_REGEN_AABB_ACTION_FILTER",
        ] {
            let probe = compact
                .find(&format!(
                    "{}({filter})",
                    if filter == "REGEN_ACTION_FILTER" {
                        "has_claimable_work"
                    } else {
                        "has_pending_work"
                    }
                ))
                .unwrap_or_else(|| panic!("{filter} 段缺少探测: {body}"));
            let drain = compact
                .find(&format!("drain_where_cooperative(mgr,{filter},"))
                .unwrap_or_else(|| panic!("{filter} 段缺少消费: {body}"));
            assert!(probe < drain, "{filter} 段必须先探再取: {body}");
        }
    }

    #[test]
    fn regen_waits_for_a_successful_and_empty_non_regen_phase() {
        assert!(data_phase_is_clear(true, false));
        assert!(
            !data_phase_is_clear(true, true),
            "the 65th item keeps the barrier closed"
        );
        assert!(
            !data_phase_is_clear(false, false),
            "a failed page keeps the barrier closed"
        );
    }

    #[test]
    fn only_fresh_parseable_roots_join_the_regen_batch() {
        let fresh = PendingModelWork {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            source_end_sesno_time: None,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            revision: 1,
            required_panels: Vec::new(),
        };
        assert!(joins_regen_batch(&fresh));

        // A root that failed before must run alone: putting it back into the
        // batch would fail the whole batch again on every drain.
        let retried = PendingModelWork {
            attempts: 1,
            ..fresh.clone()
        };
        assert!(!joins_regen_batch(&retried));

        let unparsable = PendingModelWork {
            target_refno: "not-a-refno".into(),
            ..fresh
        };
        assert!(!joins_regen_batch(&unparsable));
    }

    /// 2026-08-10 审核 P1：收口拆成「窗口语句批（分块、先行）+ 原子尾事务」。
    ///
    /// 交付状态写不许在水位推进之后丢失——保障方式从「同一个事务」改为「先于
    /// 尾事务执行、失败即拦住水位」：任何一批失败都发生在水位推进之前，整窗口
    /// 按同一区间重放、幂等收敛。尾事务保持原子：durable 工作 + 水位 + 清理。
    #[test]
    fn finalization_batches_window_statements_before_an_atomic_tail() {
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: 8191,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::RegenRoot,
                target_refno: "16777216/5".into(),
                noun: "BRAN".into(),
            }],
            ..Default::default()
        };

        let delivery_status = "update datacenter_version:16777216_5 set status = 'Modify';";
        let render = render_finalize_tail(
            8191,
            42,
            Some("2026-08-05T18:24:00+08:00"),
            &plan,
            &[delivery_status.to_string()],
        );

        // 窗口语句批：自成事务、按序先行。
        assert_eq!(
            render.window_batches.len(),
            1,
            "{:?}",
            render.window_batches
        );
        let batch = &render.window_batches[0];
        assert!(batch.starts_with("BEGIN TRANSACTION;\n"), "{batch}");
        assert!(batch.contains(delivery_status), "{batch}");
        assert!(batch.ends_with("COMMIT TRANSACTION;"), "{batch}");

        // 尾事务体：durable 工作 + 水位，且不再夹带窗口语句。
        let tail = &render.tail;
        assert!(
            tail.contains("UPSERT model_update_pending:regen_root_16777216_5"),
            "{tail}"
        );
        assert!(
            tail.contains("applied_sesno = math::max([applied_sesno?:0, 42])"),
            "{tail}"
        );
        // ADR-072 D1 的源码钉：数据尾事务不碰任何写入标记——恢复记录那一行退役了，
        // 新的 `dbnum_write_state` 由任务收尾那一笔盖成 `finished`（W2）。
        assert!(
            !tail.contains("DELETE increment_update_attempt:8191"),
            "尾事务不得再删恢复记录: {tail}"
        );
        assert!(
            !tail.contains("dbnum_write_state"),
            "尾事务不得碰写入标记: {tail}"
        );
        assert!(
            !tail.contains(delivery_status),
            "窗口语句不得再进尾事务（体积 ∝ 操作数，是撑爆 ws 通道的老路）: {tail}"
        );
    }

    #[test]
    fn degraded_room_plan_marks_rebuild_before_advancing_watermark() {
        let plan = ModelUpdatePlan {
            room_rebuild_required: true,
            room_rebuild_reason: Some("dbnum=8000: panel query failed at C:\\fixture".into()),
            ..Default::default()
        };
        let render = render_finalize_tail(8000, 240, None, &plan, &[]);
        let marker = render
            .tail
            .find("UPSERT room_build:main SET")
            .expect("durable room rebuild marker");
        assert!(
            render.tail[marker..].contains("rebuild_required = true"),
            "room_build 尾事务必须置位全量重建要求: {}",
            render.tail
        );
        let watermark = render
            .tail
            .find("UPSERT dbnum_watermark:8000")
            .expect("watermark tail");
        assert!(
            marker < watermark,
            "恢复标记必须和水位同尾事务且先于水位: {}",
            render.tail
        );
        assert!(
            render.tail.contains("C:\\\\fixture"),
            "Windows 路径必须转义: {}",
            render.tail
        );
    }

    /// 窗口语句批按 [`FINALIZE_WINDOW_TX_CHUNK`] 分块：块内原序、块间原序，
    /// 空窗口不产批。
    #[test]
    fn window_statement_batches_chunk_and_preserve_order() {
        let statements = (0..FINALIZE_WINDOW_TX_CHUNK + 1)
            .map(|index| format!("update datacenter_version:x_{index} set status = 'Modify';"))
            .collect::<Vec<_>>();
        let render = render_finalize_tail(8191, 42, None, &ModelUpdatePlan::default(), &statements);
        assert_eq!(render.window_batches.len(), 2, "501 条按 500 分块应是 2 批");
        assert!(
            render.window_batches[0].contains("x_0 ")
                && render.window_batches[0]
                    .contains(&format!("x_{} ", FINALIZE_WINDOW_TX_CHUNK - 1)),
            "第一批装满一个块"
        );
        assert!(
            render.window_batches[1].contains(&format!("x_{FINALIZE_WINDOW_TX_CHUNK} ")),
            "溢出的语句进第二批"
        );

        let empty = render_finalize_tail(8191, 42, None, &ModelUpdatePlan::default(), &[]);
        assert!(empty.window_batches.is_empty(), "空窗口不产批");
    }

    /// 拆块安全性的持久层验证：窗口语句批失败 → 尾事务不执行 → 水位不推进、
    /// 写入标记保留；成功路径推进水位，而标记**照旧留在 `writing`**——收它的是任务收尾
    /// 那一笔（ADR-072 W2）。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_window_batch_gates_the_watermark_and_keeps_the_write_marker() {
        use crate::data_interface::write_state::{WriteIntent, render_mark_writing};
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("finalize_split")
            .use_db("gate")
            .await
            .expect("use db");
        db.query(render_mark_writing(8191, 41, 42, WriteIntent::ApplyWindow))
            .await
            .expect("seed write marker transport")
            .check()
            .expect("seed write marker");

        let error = finalize_attempt_on(
            &db,
            8191,
            42,
            None,
            &ModelUpdatePlan::default(),
            &["UPDATE datacenter_version:x SET status = math::nonexistent(1);".to_string()],
        )
        .await
        .expect_err("坏窗口语句批必须让收口失败");
        assert!(format!("{error:#}").contains("window batch 0"), "{error:#}");

        let read = "SELECT VALUE applied_sesno FROM dbnum_watermark:8191;\
                    SELECT VALUE status FROM dbnum_write_state:8191;";
        let mut response = db.query(read).await.expect("read transport").check().expect("read");
        let watermark: Option<i32> = response.take(0).expect("watermark");
        let marker: Option<String> = response.take(1).expect("write marker");
        assert_eq!(watermark, None, "窗口语句批失败后水位不得推进");
        assert_eq!(
            marker.as_deref(),
            Some("writing"),
            "写入标记必须原样保留，等待回退重跑"
        );

        // 成功路径：窗口语句应用、水位推进、标记仍是 writing。
        finalize_attempt_on(
            &db,
            8191,
            42,
            None,
            &ModelUpdatePlan::default(),
            &["UPDATE datacenter_version:x SET status = 'Modify';".to_string()],
        )
        .await
        .expect("healthy finalize");
        let mut response = db.query(read).await.expect("read transport").check().expect("read");
        let watermark: Option<i32> = response.take(0).expect("watermark");
        let marker: Option<String> = response.take(1).expect("write marker");
        assert_eq!(watermark, Some(42));
        assert_eq!(
            marker.as_deref(),
            Some("writing"),
            "数据尾事务不收标记：任务还要写模型 / 房间"
        );
    }

    #[test]
    fn staged_tail_persists_spatial_intent_and_revision_guarded_settlement_before_watermark() {
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: 8191,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::RoomRecalcPanel,
                target_refno: "16777216/2".into(),
                noun: "PANE".into(),
            }],
            ..Default::default()
        };
        let sql = render_finalize_tail_with_effects(
            8191,
            42,
            None,
            &plan,
            &[],
            &["16777216/2".to_string()],
            &["16777216/3".to_string()],
            &[("16777216/5".to_string(), 7)],
        )
        .expect("staged finalize tail")
        .tail;

        let spatial = sql
            .find("spatial_reconcile_8191_42")
            .expect("spatial intent");
        let room = sql
            .find("model_update_pending:room_recalc_panel_16777216_2")
            .expect("room pending must ride the same tail");
        let epoch = sql
            .find("UPSERT spatial_epoch:current")
            .expect("epoch bump must ride the same tail");
        let settlement = sql
            .find("action = 'regen_root' AND target_refno = '16777216/5' AND (revision?:0) = 7")
            .expect("revision-guarded settlement");
        let watermark = sql.find("UPSERT dbnum_watermark:8191").expect("watermark");
        assert!(
            room < spatial,
            "房间意图要与空间意图、水位同事务提交: {sql}"
        );
        assert!(spatial < watermark, "{sql}");
        assert!(
            epoch < watermark,
            "空间版本号必须与意图、水位同一事务且先于水位: {sql}"
        );
        assert!(settlement < watermark, "{sql}");
    }

    #[tokio::test]
    async fn committed_spatial_intent_survives_discarding_the_window_database() {
        use crate::data_interface::staging::ResourceThresholds;
        use crate::data_interface::staging::StagedFinalize;
        use crate::data_interface::staging::lifecycle::create_window_on;
        use surrealdb::engine::any::connect;

        let persistent = connect("mem://").await.expect("persistent mem target");
        persistent
            .use_ns("spatial_finalize")
            .use_db("persistent")
            .await
            .expect("select persistent target");
        let instance = connect("mem://").await.expect("window mem target");
        let window = create_window_on(&instance, 8191, 2, 42, ResourceThresholds::default())
            .await
            .expect("create window");
        let context = window.write_context();
        context
            .defer_spatial_refresh(&[RefnoEnum::from(
                "16777216/2".parse::<RefU64>().expect("refresh refno"),
            )])
            .await;
        context
            .register_finalize(StagedFinalize {
                dbnum: 8191,
                start_sesno: 2,
                end_sesno: 42,
                end_sesno_time: None,
                plan: ModelUpdatePlan::default(),
                window_statements: vec![],
                cache_refnos: vec![],
            })
            .await
            .expect("register finalize");
        window
            .commit_registered_to(&persistent)
            .await
            .expect("commit window");

        drop(context);
        window.drop_database().await.expect("discard window");
        let mut response = persistent
            .query(
                "SELECT VALUE status FROM incr_side_effect_pending:spatial_reconcile_8191_42;\
                 SELECT VALUE applied_sesno FROM dbnum_watermark:8191;",
            )
            .await
            .expect("read persistent result")
            .check()
            .expect("read statements");
        let pending: Vec<String> = response.take(0).expect("pending row");
        let watermark: Option<i32> = response.take(1).expect("watermark row");
        assert_eq!(pending, ["pending"], "spatial intent must be durable");
        assert_eq!(watermark, Some(42));
    }

    /// 没动树的提交不得作废别人的树文件：无空间意图的尾事务不递增版本号。
    #[test]
    fn tail_without_spatial_effects_does_not_bump_the_epoch() {
        let sql = render_finalize_tail(8191, 42, None, &ModelUpdatePlan::default(), &[]).tail;
        assert!(
            !sql.contains("spatial_epoch"),
            "无空间意图时不得 bump: {sql}"
        );
    }

    fn room_change_fixture() -> HashMap<RefnoEnum, String> {
        HashMap::from([
            (
                RefnoEnum::from("16777216/2".parse::<RefU64>().unwrap()),
                "PANE".to_string(),
            ),
            (
                RefnoEnum::from("16777216/3".parse::<RefU64>().unwrap()),
                "EQUI".to_string(),
            ),
        ])
    }

    #[test]
    fn aabb_room_changes_are_part_of_the_finalize_plan_before_room_settlement() {
        let _room = crate::options::RoomIncrementalOverride::set(true);
        let mut plan = ModelUpdatePlan::default();
        let changes = room_change_fixture();
        merge_room_recalc_changes(&mut plan, 8191, 42, &changes);
        merge_room_recalc_changes(&mut plan, 8191, 42, &changes);

        assert_eq!(plan.work_items.len(), 2);
        assert!(plan.work_items.iter().any(|item| {
            item.action == ModelWorkAction::RoomRecalcPanel && item.target_refno == "16777216/2"
        }));
        assert!(plan.work_items.iter().any(|item| {
            item.action == ModelWorkAction::RoomRecalcElement && item.target_refno == "16777216/3"
        }));
        assert!(
            plan.work_items
                .iter()
                .all(|item| item.dbnum == 8191 && item.source_end_sesno == 42)
        );
    }

    /// 房间增量关掉之后，收口计划里一条房间行都不许出现。
    ///
    /// 钉在这一层而不是更上游：暂存链上房间目标变成 durable pending 行只有这一个
    /// 入口，它漏了，开关就只剩「不消费」那半边——表照样攒，开关一开当场涌出一
    /// 整批积压。
    #[test]
    fn a_disabled_room_increment_contributes_nothing_to_the_finalize_plan() {
        let _room = crate::options::RoomIncrementalOverride::set(false);
        let mut plan = ModelUpdatePlan::default();
        merge_room_recalc_changes(&mut plan, 8191, 42, &room_change_fixture());
        assert!(plan.work_items.is_empty(), "{:?}", plan.work_items);
    }

    /// A baseline that advanced its watermark without durable generation work
    /// would leave the dbnum modelless forever. Work batches must therefore be
    /// complete before the watermark tail. 基线的写入标记（`writing{0,T}`）同样不由这一笔收。
    #[test]
    fn baseline_transaction_pairs_generation_work_with_the_watermark() {
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: 7997,
                db_type: "DESI".into(),
                source_end_sesno: 76,
                action: ModelWorkAction::RegenRoot,
                target_refno: "24381/2".into(),
                noun: "SITE".into(),
            }],
            ..Default::default()
        };

        let render =
            render_baseline_finalize(7997, 76, Some("2026-08-01T09:12:00+08:00"), &plan, false);
        assert_eq!(render.window_batches.len(), 1);
        let work = &render.window_batches[0];
        assert!(work.starts_with("BEGIN TRANSACTION;\n"), "{work}");
        assert!(work.ends_with("COMMIT TRANSACTION;"), "{work}");
        assert!(
            work.contains("UPSERT model_update_pending:regen_root_24381_2"),
            "baseline generation work missing: {work}"
        );
        assert!(!work.contains("applied_sesno = math::max"), "{work}");
        assert!(
            render
                .tail
                .contains("applied_sesno = math::max([applied_sesno?:0, 76])"),
            "baseline watermark advance missing: {}",
            render.tail
        );
        assert!(
            render
                .tail
                .contains("confirmed_empty_baseline_sesno = NONE"),
            "非空基线必须清除空基线凭据: {}",
            render.tail
        );
        assert!(!render.tail.contains(ATTEMPT_TABLE), "{}", render.tail);
        assert!(
            !render.tail.contains("dbnum_write_state"),
            "{}",
            render.tail
        );

        let empty = render_baseline_finalize(7997, 76, None, &ModelUpdatePlan::default(), true);
        assert!(empty.window_batches.is_empty());
        assert!(
            empty.tail.contains("confirmed_empty_baseline_sesno = 76"),
            "合法空基线必须与同一水位原子记录凭据: {}",
            empty.tail
        );
    }

    #[test]
    fn baseline_generation_work_is_split_into_bounded_batches() {
        let plan = ModelUpdatePlan {
            work_items: (0..=FINALIZE_WINDOW_TX_CHUNK)
                .map(|index| ModelWorkItem {
                    dbnum: 7997,
                    db_type: "DESI".into(),
                    source_end_sesno: 76,
                    action: ModelWorkAction::RegenRoot,
                    target_refno: format!("24381/{index}"),
                    noun: "SITE".into(),
                })
                .collect(),
            ..Default::default()
        };

        let render = render_baseline_finalize(7997, 76, None, &plan, false);
        assert_eq!(render.window_batches.len(), 2);
        assert_eq!(
            render.window_batches[0]
                .matches("UPSERT model_update_pending:")
                .count(),
            500
        );
        assert_eq!(
            render.window_batches[1]
                .matches("UPSERT model_update_pending:")
                .count(),
            1
        );
        assert!(!render.tail.contains("UPSERT model_update_pending:"));
    }

    // `prepared_attempt_round_trips_the_fixed_range_and_model_plan` 随
    // `IncrementUpdateAttempt` 一起退役（ADR-072 D1）：崩溃后不再复用持久化的旧计划，
    // 写入标记只存 `status / start_sesno / end_sesno / intent`，序列化对拍无对象可拍。

    #[tokio::test]
    #[ignore = "manual live: verifies durable recovery state in configured Surreal"]
    async fn live_finalize_is_crash_safe_and_idempotent() {
        const DBNUM: u32 = 4_294_967_000;
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::Transform,
                target_refno: "4294967000/1".into(),
                noun: String::new(),
            }],
            warnings: vec!["crash recovery fixture".into()],
            ..Default::default()
        };
        let work_id = record_id(&plan.work_items[0]);
        let cleanup = format!(
            "DELETE dbnum_write_state:{DBNUM}; DELETE dbnum_watermark:{DBNUM}; DELETE {work_id};"
        );

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean recovery fixture")
            .check()
            .expect("pre-clean statements");

        crate::data_interface::write_state::mark_writing(
            DBNUM,
            40,
            42,
            crate::data_interface::write_state::WriteIntent::ApplyWindow,
        )
        .await
        .expect("mark writing");

        finalize_attempt(DBNUM, 42, None, &plan, &[])
            .await
            .expect("first finalize");
        assert!(
            crate::data_interface::write_state::load(DBNUM)
                .await
                .expect("load write marker")
                .expect("marker row survives the tail")
                .is_writing(),
            "数据尾事务不收标记（ADR-072 W2）"
        );

        // Replay the post-crash finalization: stable work id + max watermark
        // must keep exactly one task and the same applied sesno.
        finalize_attempt(DBNUM, 42, None, &plan, &[])
            .await
            .expect("idempotent finalize replay");
        let mut response = SUL_DB
            .query(format!("SELECT * FROM {work_id};"))
            .await
            .expect("query pending work")
            .check()
            .expect("pending work statement");
        let work: Vec<PendingModelWork> = response.take(0).expect("decode pending work");
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].source_end_sesno, 42);

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE applied_sesno FROM dbnum_watermark:{DBNUM};"
            ))
            .await
            .expect("query watermark")
            .check()
            .expect("watermark statement");
        let watermarks: Vec<i32> = response.take(0).expect("decode watermark");
        assert_eq!(watermarks, vec![42]);

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup recovery fixture")
            .check()
            .expect("cleanup statements");
    }

    /// OS kill 之后写入标记必须还在——它是崩溃恢复唯一的入口（ADR-072 W1）。
    #[tokio::test]
    #[ignore = "manual live: kills a helper process after the write marker lands"]
    async fn live_os_kill_preserves_the_write_marker() {
        use crate::data_interface::write_state::{WriteIntent, load, mark_writing};

        const DBNUM: u32 = 4_294_966_999;
        const HELPER_ENV: &str = "AIOS_OS_KILL_ATTEMPT_HELPER";
        const READY: &str = "AIOS_OS_KILL_ATTEMPT_READY";
        const TEST_NAME: &str =
            "data_interface::model_update_pending::tests::live_os_kill_preserves_the_write_marker";

        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: 52,
                action: ModelWorkAction::Transform,
                target_refno: "4294966999/1".into(),
                noun: String::new(),
            }],
            warnings: vec!["os-kill recovery fixture".into()],
            ..Default::default()
        };

        if std::env::var_os(HELPER_ENV).is_some() {
            aios_core::init_test_surreal()
                .await
                .expect("helper connect surreal");
            mark_writing(DBNUM, 50, 52, WriteIntent::ApplyWindow)
                .await
                .expect("helper marks writing");
            println!("{READY}");
            std::io::stdout().flush().expect("flush ready marker");
            loop {
                std::thread::park();
            }
        }

        let work_id = record_id(&plan.work_items[0]);
        let cleanup = format!(
            "DELETE dbnum_write_state:{DBNUM}; DELETE dbnum_watermark:{DBNUM}; DELETE {work_id};"
        );
        aios_core::init_test_surreal()
            .await
            .expect("parent connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean os-kill fixture")
            .check()
            .expect("pre-clean statements");

        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", TEST_NAME, "--ignored", "--nocapture"])
            .env(HELPER_ENV, "1")
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn prepare helper");
        let stdout = child.stdout.take().expect("capture helper stdout");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(Ok(line)) if line == READY => break,
                Ok(Ok(_)) => continue,
                Ok(Err(error)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("read helper output: {error}");
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("helper did not report a durable write marker: {error}");
                }
            }
        }
        child.kill().expect("terminate helper process");
        assert!(
            !child.wait().expect("wait for killed helper").success(),
            "helper must be terminated, not exit normally"
        );

        let marker = load(DBNUM)
            .await
            .expect("load after OS kill")
            .expect("写入标记必须跨进程死亡活下来");
        assert!(marker.is_writing());
        assert_eq!((marker.start_sesno, marker.end_sesno), (50, 52));
        assert_eq!(marker.intent, WriteIntent::ApplyWindow);

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup os-kill fixture")
            .check()
            .expect("cleanup statements");
    }

    #[tokio::test]
    #[ignore = "manual live: verifies one drain consumes more than the old 50-row cap"]
    async fn live_non_regen_drain_consumes_the_whole_queue() {
        const DBNUM: u32 = 4_000_000_020;
        let cleanup = format!("DELETE {TABLE} WHERE dbnum = {DBNUM};");
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean queue fixture")
            .check()
            .expect("pre-clean statements");

        let work_items = (1..=51)
            .map(|index| ModelWorkItem {
                dbnum: DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::DeleteCleanup,
                target_refno: format!("{DBNUM}/{index}"),
                noun: String::new(),
            })
            .collect();
        enqueue_plan(&ModelUpdatePlan {
            work_items,
            ..Default::default()
        })
        .await
        .expect("enqueue queue fixture");

        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        assert_eq!(
            drain_non_regen(&manager)
                .await
                .expect("drain queue fixture"),
            51
        );

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE dbnum = {DBNUM};"
            ))
            .await
            .expect("query remaining fixture")
            .check()
            .expect("query remaining statement");
        let remaining: Vec<surrealdb::sql::Thing> =
            response.take(0).expect("decode remaining fixture");
        assert!(remaining.is_empty(), "{remaining:?}");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup queue fixture")
            .check()
            .expect("cleanup statements");
    }

    /// A durable delete task must use the model service, not clean only the SQL mirror.
    /// The latter leaves read-through `ModelMemoryStore` records visible to Plant UI.
    #[test]
    fn delete_cleanup_retires_the_read_through_projection() {
        let source = include_str!("model_update_pending.rs");
        let execute = source
            .split_once("async fn execute_item(")
            .expect("execute_item exists")
            .1
            .split_once("fn missing_required_panels(")
            .expect("execute_item end exists")
            .0;
        let delete = execute
            .split_once("ModelWorkAction::DeleteCleanup =>")
            .expect("delete cleanup arm exists")
            .1
            .split_once("ModelWorkAction::CascadeExpand =>")
            .expect("delete cleanup arm end exists")
            .0;

        assert!(delete.contains("mgr.delete_model_subtree(refno).await?"));
        assert!(
            !delete.contains("delete_inst_relate_subtree"),
            "queue cleanup must not bypass the read-through projection"
        );
    }

    /// A failed queue-row delete must not abort the round. Before the fix the
    /// `?` on `delete_work` returned early, so every task queued behind the
    /// flaky one was skipped for that whole drain.
    #[tokio::test]
    #[ignore = "manual live: verifies one bad queue delete does not stall the drain"]
    async fn live_failed_queue_cleanup_does_not_stall_the_rest() {
        const DBNUM: u32 = 4_000_000_024;
        let cleanup = format!("DELETE {TABLE} WHERE dbnum = {DBNUM};");
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean isolation fixture")
            .check()
            .expect("pre-clean statements");

        let work_items = (1..=3)
            .map(|index| ModelWorkItem {
                dbnum: DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::DeleteCleanup,
                target_refno: format!("{DBNUM}/{index}"),
                noun: String::new(),
            })
            .collect();
        enqueue_plan(&ModelUpdatePlan {
            work_items,
            ..Default::default()
        })
        .await
        .expect("enqueue isolation fixture");

        // Only the first row processed fails to clear; the other two must still run.
        fail_deletes_for_test(1);
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let error = drain_non_regen(&manager)
            .await
            .expect_err("the failed cleanup must still be reported");
        assert!(
            error.to_string().contains("injected queue cleanup failure"),
            "{error:#}"
        );
        assert!(
            error.to_string().contains("2 completed"),
            "the other two tasks must have run in the same round: {error:#}"
        );

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE [target_refno, status, attempts] FROM {TABLE} \
                 WHERE dbnum = {DBNUM};"
            ))
            .await
            .expect("query isolation fixture")
            .check()
            .expect("query isolation statement");
        let remaining: Vec<serde_json::Value> = response.take(0).expect("decode isolation fixture");
        assert_eq!(remaining.len(), 1, "{remaining:?}");
        assert_eq!(remaining[0][1], serde_json::json!("failed"));
        assert_eq!(remaining[0][2], serde_json::json!(1));

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup isolation fixture")
            .check()
            .expect("cleanup statements");
    }

    #[tokio::test]
    #[ignore = "manual live: verifies failed generation remains durable in configured Surreal"]
    async fn live_generation_failure_keeps_pending_and_watermark() {
        const DBNUM: u32 = 4_000_000_021;
        const END_SESNO: i32 = 42;
        let plan = ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: END_SESNO,
                action: ModelWorkAction::RegenRoot,
                target_refno: format!("{DBNUM}/1"),
                noun: "BRAN".into(),
            }],
            ..Default::default()
        };
        let work_id = record_id(&plan.work_items[0]);
        let cleanup =
            format!("DELETE {TABLE} WHERE dbnum = {DBNUM}; DELETE dbnum_watermark:{DBNUM};");

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean failure fixture")
            .check()
            .expect("pre-clean statements");
        finalize_attempt(DBNUM, END_SESNO, None, &plan, &[])
            .await
            .expect("persist work and watermark");

        // Fresh regen work first runs as one batch, then falls back to one
        // root after a batch error. Fail both calls to exercise durable retry.
        crate::data_interface::model_refresh::fail_generations_for_test(2);
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let error = drain(&manager)
            .await
            .expect_err("injected generation failure must fail the drain");
        assert!(
            error
                .to_string()
                .contains("injected model generation failure"),
            "{error:#}"
        );

        let mut response = SUL_DB
            .query(format!(
                "RETURN [
                    (SELECT VALUE status FROM {work_id})[0],
                    (SELECT VALUE attempts FROM {work_id})[0],
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:{DBNUM})[0]
                ];"
            ))
            .await
            .expect("query failed work")
            .check()
            .expect("query failed work statement");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode failed work");
        assert_eq!(
            state,
            vec![
                serde_json::json!("failed"),
                serde_json::json!(1),
                serde_json::json!(END_SESNO),
            ]
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup failure fixture")
            .check()
            .expect("cleanup statements");
    }

    async fn assert_live_delivery_unit_regenerates(job_dbnum: u32, root: &str, noun: &str) {
        // 状态机门禁下 drain 途中的房间副作用要求空间树就绪；与
        // live_incomplete_room_panels 同款前置，顺带避免「空树算出空成员集」
        // 被写回 live 库的破坏面。
        crate::fast_model::aabb_tree::rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before delivery-unit drain");

        let root = RefU64::from_str(root).expect("valid delivery-unit fixture refno");
        let cleanup = format!("DELETE {TABLE} WHERE dbnum = {job_dbnum};");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean delivery-unit fixture")
            .check()
            .expect("pre-clean statements");

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE noun FROM {};",
                RefnoEnum::from(root).to_pe_key()
            ))
            .await
            .expect("query delivery-unit noun")
            .check()
            .expect("query delivery-unit noun statement");
        let actual: Option<String> = response.take(0).expect("decode delivery-unit noun");
        assert_eq!(actual.as_deref(), Some(noun));

        enqueue_legacy_changed_refnos(job_dbnum, 42, "DESI", &[root])
            .await
            .expect("enqueue delivery unit");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        assert_eq!(drain(&manager).await.expect("regenerate delivery unit"), 1);

        let subtree =
            crate::data_interface::helper::collect_pe_subtree_refnos(&[RefnoEnum::from(root)])
                .await
                .expect("collect generated delivery-unit subtree");
        let pe_keys = subtree
            .iter()
            .map(|refno| refno.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "array::flatten(SELECT VALUE IF record::exists(type::thing('inst_relate', \
                 record::id(id))) {{ [id] }} ELSE {{ [] }} FROM [{pe_keys}]);"
            ))
            .await
            .expect("query generated delivery-unit instances")
            .check()
            .expect("query generated delivery-unit instances statement");
        let generated: Vec<surrealdb::sql::Thing> = response
            .take(0)
            .expect("decode generated delivery-unit instances");
        assert!(
            !generated.is_empty(),
            "{noun} subtree has no generated model"
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup delivery-unit fixture")
            .check()
            .expect("cleanup statements");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates one existing BRAN in configured Surreal"]
    async fn live_bran_pending_is_actually_regenerated() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        assert_live_delivery_unit_regenerates(4_000_000_024, "24381/100817", "BRAN").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: probes incomplete room panel coverage and persists targeted repairs"]
    async fn live_incomplete_room_panels_enqueue_targeted_repairs() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        // 状态机门禁下房间消费要求空间树就绪；重建同时避免旧行为里「空树算出
        // 空成员集」被写回 live 库的破坏面。
        crate::fast_model::aabb_tree::rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before room drain");

        // 缺陷面板不再阻断整轮，所以这里不再断言 `done == 0`：其余目标本就该跑完。
        // 要钉的是「缺陷被登记下来，且修复根真的进了 durable 队列」。
        drain_rooms(aios_core::get_db_option())
            .await
            .expect("room coverage probe must complete");

        let mut response = SUL_DB
            .query(format!(
                "RETURN record::exists({ROOM_PANEL_DEFECTS}); \
                 RETURN array::len(SELECT VALUE id FROM {TABLE} \
                    WHERE action = 'regen_root' AND array::len(required_panels?:[]) > 0);"
            ))
            .await
            .expect("query repair facts")
            .check()
            .expect("repair fact statements");
        assert_eq!(response.take::<Option<bool>>(0).unwrap(), Some(true));
        assert!(
            response
                .take::<Option<usize>>(1)
                .unwrap()
                .unwrap_or_default()
                > 0
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates one existing HANG in configured Surreal"]
    async fn live_hang_pending_is_actually_regenerated() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        assert_live_delivery_unit_regenerates(4_000_000_025, "24381/177947", "HANG").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates one existing SUPPO in configured Surreal"]
    async fn live_suppo_pending_is_actually_regenerated() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        assert_live_delivery_unit_regenerates(4_000_000_026, "24384/25725", "SUPPO").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates one existing ZONE-owned EQUI in configured Surreal"]
    async fn live_zone_owned_equi_pending_is_actually_regenerated() {
        const JOB_DBNUM: u32 = 4_000_000_022;
        const ROOT: &str = "24381/100677";
        let root = RefU64::from_str(ROOT).expect("valid EQUI fixture refno");
        let cleanup = format!("DELETE {TABLE} WHERE dbnum = {JOB_DBNUM};");

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        // 同 assert_live_delivery_unit_regenerates 的前置：房间副作用过空间门。
        crate::fast_model::aabb_tree::rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before EQUI drain");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean EQUI fixture")
            .check()
            .expect("pre-clean statements");

        let mut response = SUL_DB
            .query(format!(
                "RETURN [
                    (SELECT VALUE noun FROM {})[0],
                    (SELECT VALUE owner.noun FROM {})[0]
                ];",
                RefnoEnum::from(root).to_pe_key(),
                RefnoEnum::from(root).to_pe_key(),
            ))
            .await
            .expect("query EQUI ownership")
            .check()
            .expect("query EQUI ownership statement");
        let nouns: Vec<String> = response.take(0).expect("decode EQUI ownership");
        assert_eq!(nouns, vec!["EQUI", "ZONE"]);

        enqueue_legacy_changed_refnos(JOB_DBNUM, 42, "DESI", &[root])
            .await
            .expect("enqueue ZONE-owned EQUI");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        assert_eq!(
            drain(&manager).await.expect("regenerate ZONE-owned EQUI"),
            1
        );

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE dbnum = {JOB_DBNUM};"
            ))
            .await
            .expect("query EQUI regeneration result")
            .check()
            .expect("query EQUI regeneration statement");
        let remaining: Vec<surrealdb::sql::Thing> =
            response.take(0).expect("decode remaining EQUI work");
        assert!(remaining.is_empty(), "{remaining:?}");

        let subtree =
            crate::data_interface::helper::collect_pe_subtree_refnos(&[RefnoEnum::from(root)])
                .await
                .expect("collect generated EQUI subtree");
        let pe_keys = subtree
            .iter()
            .map(|refno| refno.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "array::flatten(SELECT VALUE IF record::exists(type::thing('inst_relate', \
                 record::id(id))) {{ [id] }} ELSE {{ [] }} FROM [{pe_keys}]);"
            ))
            .await
            .expect("query generated EQUI subtree instances")
            .check()
            .expect("query generated EQUI subtree instances statement");
        let generated: Vec<surrealdb::sql::Thing> =
            response.take(0).expect("decode generated EQUI instances");
        assert!(!generated.is_empty(), "EQUI subtree has no generated model");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup EQUI fixture")
            .check()
            .expect("cleanup statements");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates 67 BRAN roots for the shared SPCO fixture"]
    async fn live_shared_spco_cascade_regenerates_every_consumer() {
        const JOB_DBNUM: u32 = 4_000_000_023;
        const SPCO: &str = "23274/295504";
        let cleanup = format!("DELETE {TABLE} WHERE dbnum = {JOB_DBNUM};");

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        // 同 assert_live_delivery_unit_regenerates 的前置：房间副作用过空间门。
        crate::fast_model::aabb_tree::rebuild_tree_from_pointers()
            .await
            .expect("rebuild spatial tree before SPCO cascade drain");
        // 级联展开读 ref_rev（重建产物，不是基线副产品）——自足重建；随后用
        // 同一个展开器算出本店切面的期望根数，drain 口径不再钉死（旧 1+67）。
        crate::data_interface::manual_update::rebuild_reverse_index()
            .await
            .expect("rebuild reverse index before cascade");
        let expected_roots = crate::data_interface::manual_update::expand_live_reverse_cascade(
            RefnoEnum::from(SPCO),
        )
        .await
        .expect("pre-expand shared SPCO cascade")
        .len();
        assert!(expected_roots > 0, "shared SPCO expanded to zero roots");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean SPCO fixture")
            .check()
            .expect("pre-clean statements");
        enqueue_plan(&ModelUpdatePlan {
            work_items: vec![ModelWorkItem {
                dbnum: JOB_DBNUM,
                db_type: "DESI".into(),
                source_end_sesno: 42,
                action: ModelWorkAction::CascadeExpand,
                target_refno: SPCO.into(),
                noun: "SPCO".into(),
            }],
            ..Default::default()
        })
        .await
        .expect("enqueue shared SPCO cascade");

        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        assert_eq!(
            drain(&manager).await.expect("drain shared SPCO cascade"),
            1 + expected_roots,
            "one cascade task plus every BRAN root must complete in one drain"
        );

        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE dbnum = {JOB_DBNUM}; \
                 SELECT VALUE REFNO FROM DAMP WHERE SPRE = pe:23274_295504;"
            ))
            .await
            .expect("query shared SPCO result")
            .check()
            .expect("query shared SPCO result statements");
        let remaining: Vec<surrealdb::sql::Thing> =
            response.take(0).expect("decode remaining SPCO work");
        assert!(remaining.is_empty(), "{remaining:?}");
        let consumers: Vec<RefnoEnum> = response.take(1).expect("decode SPCO consumers");
        // 消费者总数随店切面走（旧 72 钉的是当年切面），结构断言 + env 钉，
        // 与 live_rebuild_ref_rev_covers_shared_spco_consumers 同款拆层。
        assert!(
            consumers.len() >= 2,
            "shared SPCO must have multiple live consumers, got {}",
            consumers.len()
        );
        if let Ok(expected) = std::env::var("AIOS_EXPECT_SPCO_CONSUMERS") {
            let expected: usize = expected
                .parse()
                .expect("AIOS_EXPECT_SPCO_CONSUMERS must be a number");
            assert_eq!(
                consumers.len(),
                expected,
                "shared-SPCO consumer count drifted"
            );
        }

        let pe_keys = consumers
            .iter()
            .map(|refno| refno.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "array::flatten(SELECT VALUE IF record::exists(type::thing('inst_relate', \
                 record::id(id))) {{ [id] }} ELSE {{ [] }} FROM [{pe_keys}]);"
            ))
            .await
            .expect("query shared SPCO consumer models")
            .check()
            .expect("query shared SPCO consumer model statement");
        let generated: Vec<surrealdb::sql::Thing> = response
            .take(0)
            .expect("decode shared SPCO consumer models");
        assert_eq!(
            generated.len(),
            consumers.len(),
            "not every shared consumer regenerated"
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup SPCO fixture")
            .check()
            .expect("cleanup statements");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: validates a 5k delivery + 5k work finalize over configured websocket"]
    async fn live_finalize_capacity_is_atomic_and_idempotent() {
        const DBNUM: u32 = 4_000_000_024;
        const COUNT: usize = 5_000;
        const FIXTURE: &str = "codex_finalize_capacity";
        let cleanup = format!(
            "DELETE {TABLE} WHERE dbnum = {DBNUM}; \
             DELETE dbnum_watermark:{DBNUM}; \
             DELETE dbnum_write_state:{DBNUM}; \
             DELETE datacenter_version WHERE capacity_fixture = '{FIXTURE}';"
        );

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean finalize capacity fixture")
            .check()
            .expect("valid pre-clean statements");

        let plan = ModelUpdatePlan {
            work_items: (0..COUNT)
                .map(|index| ModelWorkItem {
                    dbnum: DBNUM,
                    db_type: "DESI".into(),
                    source_end_sesno: 42,
                    action: ModelWorkAction::RegenRoot,
                    target_refno: format!("{DBNUM}/{}", index + 1),
                    noun: "BRAN".into(),
                })
                .collect(),
            ..Default::default()
        };
        let delivery = (0..COUNT)
            .map(|index| {
                format!(
                    "UPSERT datacenter_version:capacity_{index} SET \
                     status = 'Modify', capacity_fixture = '{FIXTURE}';"
                )
            })
            .collect::<Vec<_>>();
        for _ in 0..2 {
            crate::data_interface::write_state::mark_writing(
                DBNUM,
                41,
                42,
                crate::data_interface::write_state::WriteIntent::ApplyWindow,
            )
            .await
            .expect("mark capacity fixture writing");
            finalize_attempt(DBNUM, 42, None, &plan, &delivery)
                .await
                .expect("finalize 5k delivery + 5k model work");
        }

        let mut response = SUL_DB
            .query(format!(
                "RETURN [
                    count(SELECT * FROM {TABLE} WHERE dbnum = {DBNUM}),
                    math::min(SELECT VALUE revision FROM {TABLE} WHERE dbnum = {DBNUM}),
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:{DBNUM})[0],
                    dbnum_write_state:{DBNUM}.status = 'writing',
                    count(SELECT * FROM datacenter_version
                          WHERE capacity_fixture = '{FIXTURE}')
                ];"
            ))
            .await
            .expect("query finalize capacity state")
            .check()
            .expect("valid capacity state query");
        let state: Vec<serde_json::Value> =
            response.take(0).expect("decode finalize capacity state");
        assert_eq!(
            state,
            vec![
                serde_json::json!(COUNT),
                serde_json::json!(2),
                serde_json::json!(42),
                serde_json::json!(true),
                serde_json::json!(COUNT),
            ]
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup finalize capacity fixture")
            .check()
            .expect("valid cleanup statements");
    }

    #[test]
    fn initialization_gate_precedes_every_background_model_pending_claim() {
        let source = include_str!("model_update_pending.rs");
        for marker in [
            "pub async fn drain(",
            "pub async fn drain_non_regen(",
            "pub async fn drain_non_regen_report(",
            "pub async fn drain_post_regen_aabb_report(",
            "pub async fn drain_data_phases_disposition(",
        ] {
            let body = source
                .split_once(marker)
                .unwrap_or_else(|| panic!("missing {marker}"))
                .1;
            let gate = body
                .find("model_generation_allowed()")
                .unwrap_or_else(|| panic!("{marker} lacks initialization gate"));
            let claim = body
                .find("drain_")
                .or_else(|| body.find("drain_where"))
                .unwrap_or(usize::MAX);
            assert!(
                gate < claim,
                "{marker} may claim/increment attempts before the gate"
            );
        }
        for marker in ["pub async fn drain_rooms("] {
            let body = source
                .split_once(marker)
                .unwrap_or_else(|| panic!("missing {marker}"))
                .1;
            let gate = body
                .find("postprocess_allowed()")
                .unwrap_or_else(|| panic!("{marker} lacks postprocess gate"));
            let claim = body.find("drain_rooms_selected").unwrap_or(usize::MAX);
            assert!(gate < claim, "{marker} may claim room work before the gate");
        }
    }

    #[test]
    fn initialization_not_ready_is_a_yield_not_a_retryable_model_failure() {
        let error = anyhow::Error::new(
            crate::data_interface::initialization_phase::InitializationNotReady {
                snapshot: crate::data_interface::initialization_phase::InitializationSnapshot {
                    epoch_id: 9,
                    status: "design_running",
                    current_phase: Some("design"),
                    data_ready: false,
                    model_ready: false,
                    model_in_flight: false,
                    model_phase_open: false,
                    startup_reconcile_pending: false,
                    startup_rooms_pending: false,
                    startup: None,
                    startup_data_wait_degraded: None,
                    phases: Default::default(),
                    blockers: Vec::new(),
                    blocked_by: Vec::new(),
                    shadowed: Vec::new(),
                },
            },
        )
        .context("model generation gate closed");

        assert!(is_initialization_not_ready(&error));
        assert!(!is_initialization_not_ready(&anyhow::anyhow!(
            "mesh generation failed"
        )));

        let source = include_str!("model_update_pending.rs");
        let yield_body = source
            .split_once("fn finish_cooperative_yield(")
            .expect("cooperative yield settlement exists")
            .1
            .split_once("/// 空闲模型页")
            .expect("cooperative yield settlement has a bounded body")
            .0;
        assert!(!yield_body.contains("mark_failed("));
        assert!(!yield_body.contains("record_failure("));
        assert!(!yield_body.contains("last_attempts_delta +="));

        let run_one_body = source
            .split_once("async fn run_one(")
            .expect("single-root runner exists")
            .1
            .split_once("/// Render the drain SELECT")
            .expect("single-root runner has a bounded body")
            .0;
        let not_ready = run_one_body
            .find("Err(error) if is_initialization_not_ready(&error)")
            .expect("initialization-not-ready has an explicit match arm");
        let yielded = run_one_body[not_ready..]
            .find("return RunOneOutcome::YieldedForData")
            .expect("initialization-not-ready returns yield")
            + not_ready;
        let failure_ledger = run_one_body
            .find("record_failure(job, &error, report)")
            .expect("real errors still reach the durable failure ledger");
        assert!(not_ready < yielded && yielded < failure_ledger);
    }

    #[test]
    fn a_real_root_failure_stays_failed_when_the_remaining_page_yields() {
        let clean = DrainReport {
            done: 2,
            ..Default::default()
        };
        assert_eq!(
            cooperative_yield_task_state(&clean),
            crate::data_interface::task_registry::TaskState::Yielded
        );

        let mut failed = DrainReport {
            done: 0,
            ..Default::default()
        };
        failed.record(7999, "regen 1/2: broken mesh".to_owned());
        assert_eq!(
            cooperative_yield_task_state(&failed),
            crate::data_interface::task_registry::TaskState::Failed
        );
        assert!(model_drain_failure_message(&failed).contains("broken mesh"));

        failed.done = 1;
        assert_eq!(
            cooperative_yield_task_state(&failed),
            crate::data_interface::task_registry::TaskState::Partial
        );
    }

    #[test]
    fn yielded_disposition_preserves_completed_count_and_reason() {
        let disposition = ModelDrainDisposition::YieldedForData {
            done: 3,
            claimed_epoch: 7,
            current_epoch: 8,
            reason: ModelDrainYieldReason::EpochChanged,
        };
        assert_eq!(disposition.done(), 3);
        let ModelDrainDisposition::YieldedForData { reason, .. } = disposition else {
            panic!("expected yielded disposition")
        };
        assert_eq!(reason.as_str(), "epoch_changed");
    }

    /// 死信不再扣模型门（ADR-071 D9 / S4 后半第一刀：空闲轮的 `has_dead_work` 已删），所以
    /// `has_dead_data_work` 探针也没了——留一条钉，别让它以「死信 = 未完成的初始化工作」
    /// 的旧口径回来。
    #[test]
    fn dead_letters_no_longer_gate_the_model_phase() {
        let source = include_str!("model_update_pending.rs");
        let production = source
            .split_once("\n#[cfg(test)]")
            .expect("test boundary")
            .0;
        assert!(
            !production.contains("fn has_dead_data_work("),
            "死信探针随「死信扣模型门」一起退役，不得回来"
        );
    }

    #[test]
    fn model_pending_status_splits_retryable_and_dead_work_by_phase() {
        let query = render_model_pending_status_query(&[]);
        assert!(
            query.contains("(attempts?:0) < 5"),
            "attempts=4 is retryable"
        );
        assert!(
            query.matches("(attempts?:0) >= 5").count() >= 2,
            "attempts=5 is terminal"
        );
        assert!(query.contains("LIMIT 10"), "blocking samples stay bounded");
        let samples = (0..12)
            .map(|index| ModelPendingBlockingSample {
                action: if index == 0 {
                    "regen_root".into()
                } else {
                    "room_recalc_element".into()
                },
                target_refno: format!("1/{index}"),
                attempts: MAX_ATTEMPTS,
                ..Default::default()
            })
            .collect();
        let status = build_model_pending_status(
            vec![
                ModelPendingActionCount {
                    action: "regen_root".into(),
                    c: 4,
                },
                ModelPendingActionCount {
                    action: "room_recalc_panel".into(),
                    c: 36,
                },
            ],
            vec![
                ModelPendingActionCount {
                    action: "regen_root".into(),
                    c: 1,
                },
                ModelPendingActionCount {
                    action: "room_recalc_element".into(),
                    c: 2,
                },
            ],
            samples,
        );

        assert_eq!(status.retryable, 40);
        assert_eq!(status.dead_letters, 3);
        assert_eq!(status.data_phase.retryable, 4);
        assert_eq!(status.data_phase.dead_letters, 1);
        assert_eq!(status.room_phase.retryable, 36);
        assert_eq!(status.room_phase.dead_letters, 2);
        assert_eq!(
            status.blocking_samples.len(),
            10,
            "health sample is bounded"
        );
        assert!(status.has_data_dead_letters());
        assert!(status.has_room_dead_letters());
        assert_eq!(status.data_blocking_samples().count(), 1);
    }

    /// 三条语句得真能在 SurrealDB 上跑一遍。
    ///
    /// 2026-08-21 现场：`ORDER BY updated_at DESC, id ASC` 而投影里没有 `id`，
    /// SurrealDB 报 `Missing order idiom \`id\` in statement selection` 并拒掉整批。
    /// 后果不是少一段样本——`/health` 的 `model_update_pending` 恒为错误、
    /// `degraded_sections` 常驻一条、顶层 `status` 被钉在 `degraded`，把刚做好的
    /// 「只有死信才降级」反过来盖掉了。纯字符串断言看不出这一类，得真连一次。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_pending_status_query_round_trips_on_surreal() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_pending_status")
            .await
            .expect("select fixture db");
        db.query(format!(
            "UPSERT {TABLE}:live SET action = 'regen_root', target_refno = '24381/38436', \
             noun = 'FRMW', status = 'failed', attempts = 1, revision = 2, updated_at = time::now();\
             UPSERT {TABLE}:terminal SET action = 'regen_root', target_refno = '24381/38614', \
             noun = 'FRMW', status = 'failed', attempts = {MAX_ATTEMPTS}, \
             last_error = 'invalid BREP shape', revision = 2, updated_at = time::now();"
        ))
        .await
        .expect("seed pending rows")
        .check()
        .expect("seed statements");

        let mut response = db
            .query(render_model_pending_status_query(&[]))
            .await
            .expect("status query")
            .check()
            .expect("status statements parse");
        let retryable: Vec<ModelPendingActionCount> =
            response.take(0).expect("decode retryable counts");
        let dead: Vec<ModelPendingActionCount> = response.take(1).expect("decode dead counts");
        let samples: Vec<ModelPendingBlockingSample> = response.take(2).expect("decode samples");

        let status = build_model_pending_status(retryable, dead, samples);
        assert_eq!(status.retryable, 1);
        assert_eq!(status.dead_letters, 1);
        assert_eq!(status.blocking_samples.len(), 1, "{status:?}");
        assert_eq!(status.blocking_samples[0].target_refno, "24381/38614");
        assert_eq!(status.blocking_samples[0].attempts, MAX_ATTEMPTS);

        // 上面那几条断言只有在引擎真会拒绝旧写法时才说明问题：把 `id` 从投影里去掉
        // 必须报错，否则这条回归是空的。
        let stale = render_model_pending_status_query(&[]).replace(
            "SELECT id, action, target_refno",
            "SELECT action, target_refno",
        );
        let rejected = db.query(stale).await.and_then(|response| response.check());
        assert!(
            rejected.is_err(),
            "ORDER BY 的 idiom 不在投影里就该被引擎拒绝: {rejected:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn durable_claim_excludes_a_second_worker_until_the_lease_expires() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("model_pending_claim")
            .await
            .expect("select fixture db");
        db.query(format!(
            "CREATE {TABLE}:one SET dbnum=8000, db_type='DESI', source_end_sesno=42, \
             cohort_id='8000:42', \
             action='regen_root', target_refno='1/2', noun='BRAN', status='pending', \
             attempts=0, revision=1, updated_at=time::now(); \
             CREATE {TABLE}:two SET dbnum=8000, db_type='DESI', source_end_sesno=42, \
             cohort_id='8000:42', \
             action='regen_root', target_refno='1/3', noun='BRAN', status='pending', \
             attempts=0, revision=1, updated_at=time::now();"
        ))
        .await
        .expect("seed claim row")
        .check()
        .expect("valid seed");

        let mut first = db
            .query(render_drain_claim_for_scope(
                "",
                Some(1),
                &[],
                &[],
                "worker-a",
            ))
            .await
            .expect("first claim")
            .check()
            .expect("valid first claim");
        let claimed: Vec<PendingModelWork> = first.take(1).expect("decode first claim");
        assert_eq!(claimed.len(), 1);

        let mut second = db
            .query(render_drain_claim_for_scope(
                "",
                Some(1),
                &[],
                &[],
                "worker-b",
            ))
            .await
            .expect("second claim")
            .check()
            .expect("valid second claim");
        let duplicate: Vec<PendingModelWork> = second.take(1).expect("decode second claim");
        assert!(
            duplicate.is_empty(),
            "a live lease on one member must reserve the whole cohort"
        );

        db.query(format!(
            "UPDATE {TABLE} SET lease_until=time::now() - 1s WHERE status='running';"
        ))
        .await
        .expect("expire lease")
        .check()
        .expect("valid lease expiry");
        let mut reclaimed = db
            .query(render_drain_claim_for_scope(
                "",
                Some(1),
                &[],
                &[],
                "worker-b",
            ))
            .await
            .expect("reclaim")
            .check()
            .expect("valid reclaim");
        let reclaimed: Vec<PendingModelWork> = reclaimed.take(1).expect("decode reclaim");
        assert_eq!(reclaimed.len(), 1);
    }

    #[test]
    fn unknown_pending_actions_conservatively_block_the_data_phase() {
        let status = build_model_pending_status(
            Vec::new(),
            vec![ModelPendingActionCount {
                action: "future_action".into(),
                c: 1,
            }],
            vec![ModelPendingBlockingSample {
                action: "future_action".into(),
                target_refno: "9/9".into(),
                attempts: MAX_ATTEMPTS,
                ..Default::default()
            }],
        );
        assert_eq!(status.data_phase.dead_letters, 1);
        assert_eq!(status.room_phase.dead_letters, 0);
        assert_eq!(status.data_blocking_samples().count(), 1);
    }
}
