//! Durable, per-target model work queued before the incremental watermark.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::occ_generate::AabbChange;
use crate::fast_model::room_model;

pub const TABLE: &str = "model_update_pending";
pub const ATTEMPT_TABLE: &str = "increment_update_attempt";
/// 判不了的在册面板登记表。
///
/// 曾经它是项目级**屏障**：只要有一块面板缺几何，全库房间重算整个停摆（本项目现场
/// 一度是 2 块面板压住 2580 个房间目标，而那两块的修复根早已进死信，屏障永远解不开）。
/// 现在它只是一份**缺陷清单**——替换范围的排除由元素分支按面板逐块处理，这里只负责
/// 记账、驱动修复、以及在清单变化时说一声。
const ROOM_PANEL_DEFECTS: &str = "room_panel_coverage_barrier:current";
const QUERY_CHUNK: usize = 500;
// 空闲轮一页的上界（ADR-011 2026-08-09 修订）。页内 fresh 根合并成**一次**
// `generate_roots` 调用（ADR-012）：解析 → 实例 → 网格的启动开销按页付而不是按根付；
// 页与页之间让位，新入队的数据批次最多等一页。此前是 1——每个根独占一轮空闲轮，
// 138 个修复根的积压要连刷十几分钟，每轮还各付一遍房间映射 / 面板索引 / 空间树写盘。
const DRAIN_PAGE_SIZE: usize = 16;

/// Retry ceiling per work item (same policy as `side_effect_pending`). A job
/// that keeps failing stays in the table as an inspectable dead letter instead
/// of burning a generator run every watcher cycle forever; it revives
/// automatically because [`render_upsert`] resets `attempts` whenever a newer
/// session touches the same target.
///
/// Public because the manual run enforces the same ceiling: reading the table
/// without it is how you INSPECT a dead letter, not how you re-run one.
pub const MAX_ATTEMPTS: u32 = 5;

/// Short-lived recovery record written before any PE mutation. A retry reuses
/// this exact range and pre-update model plan instead of recomputing ownership
/// from a possibly partially-applied database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncrementUpdateAttempt {
    pub dbnum: u32,
    pub db_type: String,
    pub file_path: String,
    pub start_sesno: i32,
    pub end_sesno: i32,
    pub plan: ModelUpdatePlan,
}

#[derive(Debug, Deserialize)]
struct AttemptRow {
    dbnum: u32,
    db_type: String,
    file_path: String,
    start_sesno: i32,
    end_sesno: i32,
    plan_json: String,
}

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

/// One durable room row addressed by the same `(action, target)` identity as
/// [`record_id_of`].  A post-commit drain carries these keys across the RocksDB
/// boundary instead of discovering work by scanning the global pending table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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
    for chunk in plan.work_items.chunks(QUERY_CHUNK) {
        SUL_DB
            .query(
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
            .map_err(|error| {
                anyhow::anyhow!("persist model work batch statement failed: {error}")
            })?;
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

/// Refresh targets omitted by a derived-geometry root's final node set, then force one
/// idempotent room invalidation for each target that currently has an AABB.
pub(crate) async fn refresh_post_regen_aabbs(refnos: &[RefnoEnum]) -> anyhow::Result<usize> {
    if refnos.is_empty() {
        return Ok(0);
    }
    crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos_incremental(refnos, true)
        .await?;
    let changes = crate::fast_model::occ_generate::existing_geometric_aabb_changes(refnos).await?;
    if let Some(context) = crate::data_interface::staging::active_staging_writes() {
        context.defer_room_changes(&changes).await;
    } else {
        enqueue_room_recalc(&changes).await?;
    }
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
fn source_time_for<'a>(
    item: &ModelWorkItem,
    window_end_sesno: i32,
    window_end_time: Option<&'a str>,
) -> Option<&'a str> {
    if item.source_end_sesno == 0 || item.source_end_sesno != window_end_sesno {
        return None;
    }
    window_end_time
}

/// `source_end_sesno_time` 是**那条来源保存的写入时刻**（RFC3339），待重试卡上的
/// `来源保存 08-05 18:24`（plant-ui ADR-0019 Q7）。与水位那一列同一把尺子、同一种
/// 写法：跟着 `source_end_sesno` 的单调条件走，读不到就整条子句都不写。
fn render_upsert(item: &ModelWorkItem, source_end_sesno_time: Option<&str>) -> String {
    let id = record_id(item);
    let db_type = escape_surql_str(&item.db_type);
    let target = escape_surql_str(&item.target_refno);
    let noun = escape_surql_str(&item.noun);
    let end_sesno = item.source_end_sesno;
    let dbnum = item.dbnum;

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
        vec![
            "attempts = 0".to_string(),
            "last_error = NONE".to_string(),
            "status = 'pending'".to_string(),
        ]
    } else {
        vec![
            format!(
                "attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END"
            ),
            format!(
                "last_error = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END"
            ),
            format!(
                "status = IF {end_sesno} > (source_end_sesno?:0) THEN 'pending' \
                 ELSE status?:'pending' END"
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
    // 复活子句读的是 `source_end_sesno` 的**旧值**，所以必须排在它被覆盖之前。
    clauses.push(format!(
        "source_end_sesno = math::max([source_end_sesno?:0, {end_sesno}])"
    ));
    clauses.push("revision = (revision?:0) + 1".to_string());
    clauses.push("updated_at = time::now()".to_string());

    format!("UPSERT {id} SET {};", clauses.join(", "))
}

pub async fn load_attempt(dbnum: u32) -> anyhow::Result<Option<IncrementUpdateAttempt>> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT dbnum, db_type, file_path, start_sesno, end_sesno, plan_json \
             FROM {ATTEMPT_TABLE}:{dbnum};"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("load increment attempt dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("load increment attempt dbnum={dbnum} statement failed: {error}")
        })?;
    let rows: Vec<AttemptRow> = response.take(0).map_err(|error| {
        anyhow::anyhow!("decode increment attempt dbnum={dbnum} failed: {error}")
    })?;
    rows.into_iter()
        .next()
        .map(|row| {
            let plan = serde_json::from_str(&row.plan_json).map_err(|error| {
                anyhow::anyhow!("decode increment attempt plan dbnum={dbnum} failed: {error}")
            })?;
            Ok(IncrementUpdateAttempt {
                dbnum: row.dbnum,
                db_type: row.db_type,
                file_path: row.file_path,
                start_sesno: row.start_sesno,
                end_sesno: row.end_sesno,
                plan,
            })
        })
        .transpose()
}

pub async fn prepare_attempt(attempt: &IncrementUpdateAttempt) -> anyhow::Result<()> {
    let plan_json = escape_surql_str(&serde_json::to_string(&attempt.plan)?);
    let db_type = escape_surql_str(&attempt.db_type);
    let file_path = escape_surql_str(&attempt.file_path);
    let sql = format!(
        "UPSERT {ATTEMPT_TABLE}:{dbnum} SET dbnum = {dbnum}, \
         db_type = '{db_type}', file_path = '{file_path}', \
         start_sesno = {start_sesno}, end_sesno = {end_sesno}, \
         plan_json = '{plan_json}', status = 'prepared', \
         created_at = created_at?:time::now(), updated_at = time::now();",
        dbnum = attempt.dbnum,
        start_sesno = attempt.start_sesno,
        end_sesno = attempt.end_sesno,
    );
    SUL_DB
        .query(sql)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "prepare increment attempt dbnum={} failed: {error}",
                attempt.dbnum
            )
        })?
        .check()
        .map_err(|error| {
            anyhow::anyhow!(
                "prepare increment attempt dbnum={} statement failed: {error}",
                attempt.dbnum
            )
        })?;
    Ok(())
}

/// The monotonic watermark advance for one `dbnum`. Rendered in one place so
/// the window and baseline transactions cannot drift apart.
///
/// `end_sesno_time` 是右端那条保存在 E3D 里的**写入时刻**（RFC3339，ADR-020 那把尺子）。
/// 存它的唯一理由是回退阻断卡（plant-ui ADR-0019 Q6）：文件被换回旧版本之后，
/// `applied_sesno` 那一页在当前文件里读不到了，它的写入时刻现读不出来——水位推进的
/// 这一刻是唯一能顺手存下来的时机。
///
/// 两条硬规矩：
///
/// 1. **时刻跟着序号那条单调条件走。** 序号是 `math::max`，刻意不回退；时刻若无条件
///    赋值，一个 `end_sesno` 低于存量水位的批次会让序号不动、时刻却退回去，而阻断卡
///    恰好靠这一对说话。条件子句必须排在 `applied_sesno` 被覆盖**之前**——SurrealDB
///    的 `SET` 顺序求值，排在后面读到的就是刚写完的新值（同 `render_upsert` 的复活子句）。
/// 2. **拿不到时刻就整条子句都不写**（不是写 `NONE`），让旧行与读不到时刻的新行走同一
///    条降级路径：界面说「应用时刻无记录」，**绝不拿挂钟 `applied_at` 兜底**——回退本来
///    就是时间倒挂场景，两把尺子混用最容易骗人。
///
/// 存的是 RFC3339 **字符串**而不是 `type::datetime`，与本表其余时间列（`applied_at` /
/// `scanned_at` / `file_modified_at`）不同：那些没人读回 Rust，而这一列要原样露给界面。
/// 走 datetime 读回来会被规范化成 UTC，同一张阻断卡上「文件端」（现读，带 +08:00）与
/// 「已应用端」就会差八个小时——同一条保存两种说法，正是这轮改造要消灭的东西。
pub(crate) fn render_watermark_advance(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
) -> String {
    let time_clause = end_sesno_time
        .map(|time| {
            format!(
                "applied_sesno_time = IF {end_sesno} >= (applied_sesno ?: 0) \
                 THEN '{}' ELSE applied_sesno_time END, ",
                escape_surql_str(time)
            )
        })
        .unwrap_or_default();
    format!(
        "UPSERT dbnum_watermark:{dbnum} SET dbnum = {dbnum}, \
         {time_clause}\
         applied_sesno = math::max([applied_sesno?:0, {end_sesno}]), \
         sesno = math::max([sesno?:0, {end_sesno}]), \
         applied_at = time::now(), updated_at = time::now();"
    )
}

/// 窗口语句批的分块大小。与主数据落库的 `TX_CHUNK` 同一纪律（`increment_pipeline::
/// persist_latest_main_data`）：整窗口单事务撑爆 SurrealDB ws 通道是已记录事故。
pub(crate) const FINALIZE_WINDOW_TX_CHUNK: usize = 500;

/// 一次收口的两段渲染产物（2026-08-10 审核 P1）。
///
/// `window_batches`：本窗口的 datacenter 交付状态与 anc 定点重算语句，按
/// [`FINALIZE_WINDOW_TX_CHUNK`] 分块、每块各自包装成独立事务，**先于尾事务**按
/// 原序执行。它们曾整段塞进尾事务：正确性要求其实只有「不得在水位推进之后丢失」，
/// 而语句数 ∝ 窗口内的操作数——宽 DESI 窗口会把尾事务推到当年撑爆 ws 通道的量级，
/// 且尾事务确定性失败 = 写回无限重试 + 重启重放同一窗口再失败的跨重启活锁。
/// 拆块之后不变量依然成立：任何一块失败都发生在水位推进**之前**——水位不动、
/// 恢复记录还在，整窗口按同一区间重放，幂等的固定目标 UPDATE 重复应用只是空转。
///
/// `tail`：收口尾事务体（未包装）——durable 模型工作、空间意图、revision 收口、
/// 水位推进、attempts 清除、恢复记录删除。行数有界（∝ 工作项数），保持单个原子
/// 事务：这一段才是「要么全部成立、要么整体重放」的收口本体。
#[derive(Debug, Clone)]
pub(crate) struct FinalizeRender {
    /// 已各自包装成 `BEGIN…COMMIT` 的窗口语句批，按原序执行。
    pub window_batches: Vec<String>,
    /// 尾事务体（未包装），由执行方套上唯一的事务包装。
    pub tail: String,
}

fn render_window_statement_batches(window_statements: &[String]) -> Vec<String> {
    window_statements
        .chunks(FINALIZE_WINDOW_TX_CHUNK)
        .map(|chunk| {
            format!(
                "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
                chunk.join("\n")
            )
        })
        .collect()
}

/// ADR-017 T1.3：窗口收口的渲染（窗口语句批 + 尾事务体）。
///
/// 暂存路径由 `StagedExecutor::commit_to` 在 journal 重放之后、尾事务之前逐批
/// 执行 `window_batches`；直写路径由 [`finalize_attempt_on`] 同序执行——两条
/// 路径共用同一份渲染，收口内容不可能漂移。顺序：窗口语句批（datacenter 交付
/// 状态 + anc 重算，commit-time 语义）→ 尾事务（durable 模型工作 → 水位推进 →
/// 恢复记录删除）。收口条件（水位单调、revision 判真）全部在持久层事务内判定。
pub(crate) fn render_finalize_tail(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> FinalizeRender {
    render_finalize_tail_with_effects(
        dbnum,
        end_sesno,
        end_sesno_time,
        plan,
        window_statements,
        &[],
        &[],
        &[],
    )
    .expect("empty finalize effects are valid")
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

pub(crate) fn render_finalize_tail_with_effects(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
    refresh_refnos: &[String],
    remove_refnos: &[String],
    settled_regen: &[(String, u64)],
) -> anyhow::Result<FinalizeRender> {
    let mut statements: Vec<String> = plan
        .work_items
        .iter()
        .map(|item| render_upsert(item, source_time_for(item, end_sesno, end_sesno_time)))
        .collect();
    if !refresh_refnos.is_empty() || !remove_refnos.is_empty() {
        statements.push(
            crate::data_interface::side_effect_pending::SideEffectCompensator::render_spatial_reconcile_upsert(
                dbnum,
                end_sesno,
                refresh_refnos,
                remove_refnos,
            )?,
        );
        // 空间版本号与意图、水位同一事务递增：启动侧拿 sidecar 与它比相等来决定
        // 树文件还能不能信（load_project_tree_verified）。只有携带空间意图的尾
        // 事务才 bump——没动树的提交不该作废别人的文件。
        statements.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    }
    statements.extend(settled_regen.iter().map(|(root, revision)| {
        render_delete_revision(ModelWorkAction::RegenRoot, root, *revision)
    }));
    statements.push(render_watermark_advance(dbnum, end_sesno, end_sesno_time));
    statements.push(crate::data_interface::staging::attempts::render_clear_window_attempts(dbnum));
    statements.push(format!("DELETE {ATTEMPT_TABLE}:{dbnum};"));
    Ok(FinalizeRender {
        window_batches: render_window_statement_batches(window_statements),
        tail: statements.join("\n"),
    })
}

/// Render the transaction that closes a freshly parsed baseline.
///
/// Same collar as [`render_finalize_transaction`] minus the recovery-record
/// removal: a baseline is not a replayable window, so it never has an
/// `increment_update_attempt` row, and deleting one here could only discard
/// another path's crash-recovery state.
fn render_baseline_transaction(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
) -> String {
    let mut statements: Vec<String> = plan
        .work_items
        .iter()
        .map(|item| render_upsert(item, source_time_for(item, end_sesno, end_sesno_time)))
        .collect();
    statements.push(render_watermark_advance(dbnum, end_sesno, end_sesno_time));
    format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        statements.join("\n")
    )
}

/// Establish durable model work, advance the authoritative watermark, and
/// remove the recovery record — the last three atomically.
///
/// `window_statements` carries writes that must never be lost under an
/// advancing watermark — this window's `datacenter_version` status updates and
/// the OWNER-move `anc` repairs. They execute as ordered [`FinalizeRender::
/// window_batches`] **before** the tail transaction: any batch failure returns
/// before the watermark moves, so the whole fixed range replays and the
/// idempotent fixed-target UPDATEs converge. Executing them after (or without
/// gating) the watermark was the historical bug this ordering exists to
/// prevent — a lost status write was unrepairable because no later window
/// revisits an element that did not change again.
pub async fn finalize_attempt(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<()> {
    finalize_attempt_on(
        &SUL_DB,
        dbnum,
        end_sesno,
        end_sesno_time,
        plan,
        window_statements,
    )
    .await
}

pub(crate) async fn finalize_attempt_on(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<()> {
    let render = render_finalize_tail(dbnum, end_sesno, end_sesno_time, plan, window_statements);
    for (index, batch) in render.window_batches.iter().enumerate() {
        db.query(batch)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize increment attempt dbnum={dbnum} window batch {index} failed: {error}"
                )
            })?
            .check()
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize increment attempt dbnum={dbnum} window batch {index} \
                     statement failed: {error}"
                )
            })?;
    }
    db.query(format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render.tail
    ))
    .await
    .map_err(|error| anyhow::anyhow!("finalize increment attempt dbnum={dbnum} failed: {error}"))?
    .check()
    .map_err(|error| {
        anyhow::anyhow!("finalize increment attempt dbnum={dbnum} statement failed: {error}")
    })?;
    Ok(())
}

/// Atomically establish a freshly parsed `dbnum`'s model work and its watermark.
///
/// A baseline full-parse writes element data but no geometry, and every later
/// incremental window only regenerates the roots that window itself touched. So
/// a watermark that advances without its generation work leaves the database
/// permanently modelless — nothing revisits a root that never changes again.
/// Binding the two into one transaction makes that state unreachable: either
/// the baseline is both applied and scheduled for generation, or it replays.
pub async fn finalize_baseline(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
) -> anyhow::Result<()> {
    SUL_DB
        .query(render_baseline_transaction(
            dbnum,
            end_sesno,
            end_sesno_time,
            plan,
        ))
        .await
        .map_err(|error| anyhow::anyhow!("finalize baseline dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("finalize baseline dbnum={dbnum} statement failed: {error}")
        })?;
    Ok(())
}

#[cfg(test)]
static FAIL_DELETES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Make the next `count` queue-row deletes fail, to exercise the drain's failure
/// isolation without having to take SurrealDB down mid-round.
#[cfg(test)]
fn fail_deletes_for_test(count: usize) {
    FAIL_DELETES.store(count, std::sync::atomic::Ordering::SeqCst);
}

/// 收口语句一律按 `(action, target_refno)` 谓词寻址，而不是按重新算出来的 record id。
///
/// 算 id 的写法要求「入队时算的 id」与「收口时算的 id」永远一致。它们曾经不一致过
/// （见 `record_id_of`：dbnum 位置上有人传 Ref0），后果是 `DELETE` 静默命中零行——
/// 任务清不掉、每一轮都重跑一次完整生成，而日志里一切正常。谓词寻址让收口只依赖
/// 行里实际存着的字段，顺带把历史遗留的 `{dbnum}_` 前缀旧行一并收敛掉。
fn settle_predicate(action: ModelWorkAction, target_refno: &str, revision: u64) -> String {
    format!(
        "action = '{}' AND target_refno = '{}' AND (revision?:0) = {revision}",
        action.as_str(),
        escape_surql_str(target_refno)
    )
}

fn render_delete_revision(action: ModelWorkAction, target_refno: &str, revision: u64) -> String {
    format!(
        "DELETE {TABLE} WHERE {};",
        settle_predicate(action, target_refno, revision)
    )
}

fn render_delete_work(item: &PendingModelWork) -> String {
    render_delete_revision(item.action, &item.target_refno, item.revision)
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

fn render_mark_failed_revision(
    action: ModelWorkAction,
    target_refno: &str,
    revision: u64,
    error: &str,
) -> String {
    let error = escape_surql_str(error);
    format!(
        "UPDATE {TABLE} SET status = 'failed', attempts = (attempts?:0) + 1, \
         last_error = '{error}', updated_at = time::now() \
         WHERE {};",
        settle_predicate(action, target_refno, revision)
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
        .query(render_delete_revision(
            ModelWorkAction::RegenRoot,
            root_refno,
            revision,
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
                .map(|(root_refno, revision)| {
                    render_delete_revision(ModelWorkAction::RegenRoot, root_refno, *revision)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("BEGIN TRANSACTION;\n{deletes}\nCOMMIT TRANSACTION;")
        })
        .collect()
}

pub(crate) async fn clear_regen_work_batch(items: &[(String, u64)]) -> anyhow::Result<()> {
    for transaction in render_clear_regen_transactions(items) {
        SUL_DB
            .query(transaction)
            .await
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
            crate::data_interface::helper::delete_inst_relate_subtree(&[refno], 300).await
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
            let rooms = room_model::load_room_panel_map(&mgr.db_option).await?;
            let panels = room_model::load_panel_index(&mgr.db_option, &rooms).await?;
            // 整间任务用不到构件的旧归属快照，别为它多发一条查询。
            let history = if matches!(item.action, ModelWorkAction::RoomRecalcElement) {
                room_model::ElementRoomHistory::load(&[refno]).await?
            } else {
                room_model::ElementRoomHistory::default()
            };
            run_room_task(
                &mgr.db_option,
                &rooms,
                &panels,
                &history,
                item.action,
                refno,
            )
            .await
            .map(|_| ())
        }
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

#[derive(Debug, Default)]
pub(crate) struct StagedNonRegenReport {
    pub derived_roots: Vec<crate::data_interface::generation_root::GenerationRoot>,
    pub succeeded_plan_items: BTreeSet<(ModelWorkAction, String)>,
    pub failures: Vec<String>,
}

/// Execute this window's prerequisites without touching the durable pending queue.
pub(crate) async fn run_staged_non_regen_work(
    mgr: &AiosDBManager,
    plan_items: &[ModelWorkItem],
) -> StagedNonRegenReport {
    let mut report = StagedNonRegenReport::default();
    for action in [
        ModelWorkAction::Transform,
        ModelWorkAction::DeleteCleanup,
        ModelWorkAction::CascadeExpand,
    ] {
        for item in plan_items.iter().filter(|item| item.action == action) {
            let refno = match RefU64::from_str(&item.target_refno).map(RefnoEnum::from) {
                Ok(refno) => refno,
                Err(_) => {
                    report.failures.push(format!(
                        "{} 目标 {} 无效",
                        action.as_str(),
                        item.target_refno
                    ));
                    continue;
                }
            };
            let outcome = match action {
                ModelWorkAction::Transform => {
                    mgr.update_world_transforms(&HashSet::from([refno])).await
                }
                ModelWorkAction::DeleteCleanup => {
                    crate::data_interface::helper::delete_inst_relate_subtree(&[refno], 300).await
                }
                ModelWorkAction::CascadeExpand => {
                    crate::data_interface::manual_update::expand_staged_reverse_cascade(refno)
                        .await
                        .map(|roots| report.derived_roots.extend(roots))
                }
                _ => unreachable!(),
            };
            match outcome {
                Ok(()) => {
                    report
                        .succeeded_plan_items
                        .insert((action, item.target_refno.clone()));
                }
                Err(error) => report.failures.push(format!(
                    "{} 目标 {} 暂存执行失败: {error:#}",
                    action.as_str(),
                    item.target_refno
                )),
            }
        }
    }
    report.derived_roots.sort_by_key(|root| root.root);
    report.derived_roots.dedup_by_key(|root| root.root);
    report
}

/// 执行一个房间重算任务，返回本次写入了归属边的构件集合。
async fn run_room_task(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panels: &room_model::PanelIndex,
    history: &room_model::ElementRoomHistory,
    action: ModelWorkAction,
    target: RefnoEnum,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    match action {
        ModelWorkAction::RoomRecalcPanel => {
            room_model::recalc_panel_membership(db_option, rooms, target).await
        }
        ModelWorkAction::RoomRecalcElement => {
            room_model::recalc_element_membership(rooms, panels, history, target).await?;
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
async fn record_failure(job: &PendingModelWork, error: &anyhow::Error, report: &mut DrainReport) {
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
    } else {
        report.record(
            job.dbnum,
            format!("{} {}: {message}", job.action.as_str(), job.target_refno),
        );
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
) -> anyhow::Result<HashSet<RefnoEnum>> {
    match crate::data_interface::batch_worker::isolate_panic(run_room_job(
        db_option, rooms, panels, history, job,
    ))
    .await
    {
        Ok(result) => result,
        Err(reason) => anyhow::bail!("panic: {reason}"),
    }
}

async fn run_one(mgr: &AiosDBManager, job: &PendingModelWork, report: &mut DrainReport) {
    let root_lock = (job.action == ModelWorkAction::RegenRoot)
        .then(|| crate::data_interface::manual_update::generation_root_lock(&job.target_refno));
    let _root_guard = match &root_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let outcome = match execute_item_isolated(mgr, job).await {
        Ok(()) => delete_work(job).await,
        Err(error) => Err(error),
    };
    match outcome {
        Ok(()) => report.done += 1,
        Err(error) => record_failure(job, &error, report).await,
    }
}

/// Render the drain SELECT. Work at or above [`MAX_ATTEMPTS`] stays in the
/// table as a dead letter: the automatic watcher never picks it up again,
/// while manual preview/retry reads the table without this cap and remains
/// the way to inspect or revive it.
fn render_drain_select(action_filter: &str, limit: Option<usize>) -> String {
    let limit = limit
        .map(|value| format!(" LIMIT {value}"))
        .unwrap_or_default();
    format!(
        "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
         AND (attempts?:0) < {MAX_ATTEMPTS} {action_filter} \
         ORDER BY updated_at ASC{limit};"
    )
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
         AND (attempts?:0) < {MAX_ATTEMPTS} ORDER BY updated_at ASC;"
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
    let (batchable, mut singles): (Vec<PendingModelWork>, Vec<PendingModelWork>) =
        regen_jobs.into_iter().partition(joins_regen_batch);

    let mut report = DrainReport::default();

    if !batchable.is_empty() {
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
                    singles.extend(std::mem::take(&mut repair_jobs));
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
        // 锁覆盖本页全部根（含跳过生成的修复根：验收与收口也在锁内，与 run_one 同构）。
        let mut lock_roots: Vec<String> = plain_jobs
            .iter()
            .chain(repair_jobs.iter())
            .map(|job| job.target_refno.clone())
            .collect();
        lock_roots.sort_unstable();
        lock_roots.dedup();
        let locks = lock_roots
            .iter()
            .map(|root| crate::data_interface::manual_update::generation_root_lock(root))
            .collect::<Vec<_>>();
        let mut _root_guards = Vec::with_capacity(locks.len());
        for lock in &locks {
            _root_guards.push(lock.lock().await);
        }
        let batch_result =
            crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(mgr, &roots)
                .await;
        match batch_result {
            Ok(()) => {
                let mut settlements = plain_jobs
                    .iter()
                    .map(|job| (job.target_refno.clone(), job.revision))
                    .collect::<Vec<_>>();
                if !repair_jobs.is_empty() {
                    let job_refs = repair_jobs.iter().collect::<Vec<_>>();
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
                                    &mut report,
                                )
                                .await;
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
                                repair_jobs.len()
                            );
                            for job in &repair_jobs {
                                report.failed_dbnums.insert(job.dbnum);
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
                        for job in plain_jobs.iter().chain(repair_jobs.iter()) {
                            report.failed_dbnums.insert(job.dbnum);
                        }
                        report.failures.push(message);
                    }
                }
            }
            Err(error) => {
                // The per-root fallback acquires the same locks one by one.
                drop(_root_guards);
                drop(locks);
                println!(
                    "批量重生成 {} 个根失败，回退逐根重试以定位问题根: {error:#}",
                    roots.len()
                );
                for job in plain_jobs.iter().chain(repair_jobs.iter()) {
                    run_one(mgr, job, &mut report).await;
                }
            }
        }
    }

    for job in singles.iter().chain(other_jobs.iter()) {
        run_one(mgr, job, &mut report).await;
    }

    Ok(report)
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

fn data_phase_is_clear(succeeded: bool, has_more: bool) -> bool {
    succeeded && !has_more
}

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
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
    drain_where(mgr, NON_REGEN_ACTION_FILTER, None).await
}

/// 与 [`drain_non_regen`] 同一轮工作，但把失败牵涉到的 `dbnum` 一起带出来。
///
/// 批次执行前的那次前置消化用它：非 regen 积压是全局的，只有**本批这个库**的
/// 前置失败才该拦下本批的模型生成。
pub async fn drain_non_regen_report(mgr: &AiosDBManager) -> anyhow::Result<DrainReport> {
    drain_where_report(mgr, NON_REGEN_ACTION_FILTER, None).await
}

pub async fn drain_post_regen_aabb_report(
    mgr: &AiosDBManager,
    dbnum: u32,
) -> anyhow::Result<DrainReport> {
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
pub async fn drain_data_phases(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    let non_regen = drain_where(mgr, NON_REGEN_ACTION_FILTER, Some(DRAIN_PAGE_SIZE)).await;
    let has_more = if non_regen.is_ok() {
        has_pending_work(NON_REGEN_ACTION_FILTER).await?
    } else {
        false
    };
    if !data_phase_is_clear(non_regen.is_ok(), has_more) {
        return non_regen;
    }

    let mut done = non_regen?;
    done += drain_where(mgr, REGEN_ACTION_FILTER, Some(DRAIN_PAGE_SIZE)).await?;
    done += drain_where(mgr, POST_REGEN_AABB_ACTION_FILTER, Some(DRAIN_PAGE_SIZE)).await?;
    Ok(done)
}

async fn has_pending_work(action_filter: &str) -> anyhow::Result<bool> {
    let mut response = SUL_DB
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM {TABLE} \
             WHERE status IN ['pending', 'failed'] AND (attempts?:0) < {MAX_ATTEMPTS} \
             {action_filter} LIMIT 1)) > 0;"
        ))
        .await?
        .check()?;
    Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
}

pub async fn has_pending_data_work() -> anyhow::Result<bool> {
    has_pending_work(DATA_ACTION_FILTER).await
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
        .query(format!(
            "SELECT action, count() AS c FROM {TABLE} WHERE status IN ['pending', 'failed'] \
             AND (attempts?:0) < {MAX_ATTEMPTS} {ROOM_ACTION_FILTER} GROUP BY action;\
             SELECT count() AS c FROM {TABLE} WHERE (attempts?:0) >= {MAX_ATTEMPTS} \
             {ROOM_ACTION_FILTER} GROUP ALL;"
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
    drain_rooms_selected(db_option, None).await
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

async fn drain_rooms_selected(
    db_option: &aios_core::options::DbOption,
    scope: Option<&RoomDrainScope>,
) -> anyhow::Result<DrainReport> {
    // 状态机门禁（一致性闭环方案 §6）：房间重算的整间分支候选取自
    // GLOBAL_AABB_TREE，树不在可消费状态就消费任务 = 拿不可信的树改写归属。
    // 被拒时错误带 SPATIAL_TREE_NOT_READY 码，durable 行原样保留，
    // 状态收敛后由空闲轮/scoped 重试收走。
    crate::fast_model::spatial_state::ensure_spatial_ready()?;
    // Panels are always complete-before-elements. The scoped path selects exact
    // record ids; element records themselves are not accumulated across pages.
    let panels = match scope {
        Some(scope) => {
            load_scoped_room_jobs(&scope.keys_for(ModelWorkAction::RoomRecalcPanel)).await?
        }
        None => load_room_jobs(ROOM_PANEL_ACTION_FILTER, None).await?,
    };
    let global_elements = if scope.is_none() {
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

    let rooms = room_model::load_room_panel_map(db_option).await?;
    // Panel geometry is loaded once for the complete scoped drain and reused by
    // every 256-element history page.
    let panel_index = room_model::load_panel_index(db_option, &rooms).await?;
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
        match run_room_job_isolated(db_option, &rooms, &panel_index, &empty_history, job).await {
            Ok(members) => {
                claimed_members.extend(members);
                if let Ok(refno) = RefU64::from_str(&job.target_refno) {
                    claimed_panels.insert(RefnoEnum::from(refno));
                }
                match delete_work(job).await {
                    Ok(()) => report.done += 1,
                    Err(error) => record_failure(job, &error, &mut report).await,
                }
            }
            Err(error) => record_failure(job, &error, &mut report).await,
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
            &mut report,
        )
        .await;
    }
    Ok(report)
}

async fn drain_room_element_page(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    panel_index: &room_model::PanelIndex,
    claimed_members: &HashSet<RefnoEnum>,
    claimed_panels: &HashSet<RefnoEnum>,
    elements: &[PendingModelWork],
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
            delete_work(job).await
        } else {
            match run_room_job_isolated(db_option, rooms, panel_index, history_ref, job).await {
                Ok(_) => delete_work(job).await,
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) => report.done += 1,
            Err(error) => record_failure(job, &error, report).await,
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
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&job.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid pending refno {}", job.target_refno))?,
    );
    run_room_task(db_option, rooms, panels, history, job.action, refno).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// 多数用例只关心复活 / 覆盖那几条子句，与来源保存时刻无关。
    fn render_upsert_no_time(item: &ModelWorkItem) -> String {
        render_upsert(item, None)
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
            .split_once("if !batchable.is_empty()")
            .expect("batch regeneration branch must exist")
            .1
            .split_once("for job in singles")
            .expect("batch regeneration branch must end before singles")
            .0;

        assert!(run_one.contains("generation_root_lock"), "{run_one}");
        assert!(batch.contains("generation_root_lock"), "{batch}");
        assert!(
            run_one.find("lock().await") < run_one.find("delete_work(job).await"),
            "single-root lock must cover queue settlement"
        );
        assert!(
            batch.find("lock().await") < batch.find("clear_regen_work_batch(&settlements).await"),
            "batch locks must cover queue settlement"
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
            render_upsert_no_time(&item).contains("revision = (revision?:0) + 1"),
            "every trigger must create a new settlement revision"
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

    /// 收口不能靠「再算一遍 record id」。存量表里同一个根还留着旧格式的行
    /// （`{dbnum}_regen_root_…`），按 id 寻址会命中零行——任务清不掉、每一轮重跑一次
    /// 完整生成，而日志里一切正常。谓词寻址只依赖行里实际存着的字段。
    #[test]
    fn settlement_addresses_the_row_by_its_fields_not_by_a_recomputed_id() {
        let sql = render_delete_revision(ModelWorkAction::RegenRoot, "24381/100677", 3);
        assert_eq!(
            sql,
            "DELETE model_update_pending WHERE action = 'regen_root' \
             AND target_refno = '24381/100677' AND (revision?:0) = 3;"
        );
        assert!(
            !sql.contains("model_update_pending:"),
            "settlement must not address a record id: {sql}"
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
                "DELETE model_update_pending WHERE action = 'regen_root' \
                 AND target_refno = '16777216/1' AND (revision?:0) = 1;"
            ),
            "{}",
            transactions[0]
        );
        assert!(
            transactions[1].contains(
                "DELETE model_update_pending WHERE action = 'regen_root' \
                 AND target_refno = '16777216/501' AND (revision?:0) = 501;"
            ),
            "{}",
            transactions[1]
        );
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
        assert!(sql.contains("attempts = 0"), "{sql}");
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
            "空闲消化按有界页让位（页间可插入新批次），页内合批生成摊薄启动开销（ADR-011 2026-08-09 修订）"
        );
        let sql = render_drain_select("AND action = 'regen_root'", Some(DRAIN_PAGE_SIZE));
        assert!(
            sql.contains(&format!("(attempts?:0) < {MAX_ATTEMPTS}")),
            "{sql}"
        );
        assert!(sql.contains("status IN ['pending', 'failed']"), "{sql}");
        assert!(sql.contains("AND action = 'regen_root'"), "{sql}");
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

        // 尾事务体：durable 工作 + 水位 + 恢复记录清理，且不再夹带窗口语句。
        let tail = &render.tail;
        assert!(
            tail.contains("UPSERT model_update_pending:regen_root_16777216_5"),
            "{tail}"
        );
        assert!(
            tail.contains("applied_sesno = math::max([applied_sesno?:0, 42])"),
            "{tail}"
        );
        assert!(
            tail.contains("DELETE increment_update_attempt:8191"),
            "{tail}"
        );
        assert!(
            !tail.contains(delivery_status),
            "窗口语句不得再进尾事务（体积 ∝ 操作数，是撑爆 ws 通道的老路）: {tail}"
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
    /// 恢复记录保留；成功路径则三样各就各位。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_window_batch_gates_the_watermark_and_keeps_the_attempt() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("finalize_split")
            .use_db("gate")
            .await
            .expect("use db");
        db.query(format!(
            "UPSERT {ATTEMPT_TABLE}:8191 SET dbnum = 8191, status = 'prepared';"
        ))
        .await
        .expect("seed attempt transport")
        .check()
        .expect("seed attempt");

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

        let mut response = db
            .query(format!(
                "SELECT VALUE applied_sesno FROM dbnum_watermark:8191;\
                 SELECT VALUE status FROM {ATTEMPT_TABLE}:8191;"
            ))
            .await
            .expect("read transport")
            .check()
            .expect("read");
        let watermark: Option<i32> = response.take(0).expect("watermark");
        let attempt: Option<String> = response.take(1).expect("attempt");
        assert_eq!(watermark, None, "窗口语句批失败后水位不得推进");
        assert_eq!(
            attempt.as_deref(),
            Some("prepared"),
            "恢复记录必须原样保留，等待整窗口重放"
        );

        // 成功路径：窗口语句应用、水位推进、恢复记录删除。
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
        let mut response = db
            .query(format!(
                "SELECT VALUE applied_sesno FROM dbnum_watermark:8191;\
                 SELECT VALUE id FROM {ATTEMPT_TABLE}:8191;"
            ))
            .await
            .expect("read transport")
            .check()
            .expect("read");
        let watermark: Option<i32> = response.take(0).expect("watermark");
        let attempt_rows: Vec<surrealdb::RecordId> = response.take(1).expect("attempt rows");
        assert_eq!(watermark, Some(42));
        assert!(attempt_rows.is_empty(), "收口成功后恢复记录必须删除");
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

    /// A baseline that advanced its watermark without queueing generation work
    /// would leave the dbnum modelless forever, so the two must share one
    /// transaction. It must NOT drop an `increment_update_attempt` row: a
    /// baseline never owns one, and another path's recovery record is not its
    /// to discard.
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

        let sql = render_baseline_transaction(7997, 76, Some("2026-08-01T09:12:00+08:00"), &plan);
        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
        let work_at = sql
            .find("UPSERT model_update_pending:regen_root_24381_2")
            .unwrap_or_else(|| panic!("baseline generation work missing: {sql}"));
        let watermark_at = sql
            .find("applied_sesno = math::max([applied_sesno?:0, 76])")
            .unwrap_or_else(|| panic!("baseline watermark advance missing: {sql}"));
        assert!(work_at < watermark_at, "{sql}");
        assert!(!sql.contains(ATTEMPT_TABLE), "{sql}");
    }

    #[test]
    fn prepared_attempt_round_trips_the_fixed_range_and_model_plan() {
        let attempt = IncrementUpdateAttempt {
            dbnum: 8191,
            db_type: "DESI".into(),
            file_path: "D:/project/desi".into(),
            start_sesno: 40,
            end_sesno: 42,
            plan: ModelUpdatePlan {
                work_items: vec![ModelWorkItem {
                    dbnum: 8191,
                    db_type: "DESI".into(),
                    source_end_sesno: 42,
                    action: ModelWorkAction::Transform,
                    target_refno: "16777216/9".into(),
                    noun: String::new(),
                }],
                warnings: vec!["kept across restart".into()],
                ..Default::default()
            },
        };

        let json = serde_json::to_string(&attempt).expect("serialize attempt");
        let restored: IncrementUpdateAttempt =
            serde_json::from_str(&json).expect("deserialize attempt");
        assert_eq!(restored, attempt);
    }

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
        let attempt = IncrementUpdateAttempt {
            dbnum: DBNUM,
            db_type: "DESI".into(),
            file_path: "D:/fixture/desi".into(),
            start_sesno: 40,
            end_sesno: 42,
            plan: plan.clone(),
        };
        let work_id = record_id(&plan.work_items[0]);
        let cleanup = format!(
            "DELETE {ATTEMPT_TABLE}:{DBNUM}; DELETE dbnum_watermark:{DBNUM}; DELETE {work_id};"
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

        prepare_attempt(&attempt).await.expect("prepare attempt");
        assert_eq!(
            load_attempt(DBNUM).await.expect("load attempt"),
            Some(attempt)
        );

        finalize_attempt(DBNUM, 42, None, &plan, &[])
            .await
            .expect("first finalize");
        assert_eq!(load_attempt(DBNUM).await.expect("attempt removed"), None);

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

    #[tokio::test]
    #[ignore = "manual live: kills a helper process after durable prepare"]
    async fn live_os_kill_preserves_prepared_attempt() {
        const DBNUM: u32 = 4_294_966_999;
        const HELPER_ENV: &str = "AIOS_OS_KILL_ATTEMPT_HELPER";
        const READY: &str = "AIOS_OS_KILL_ATTEMPT_READY";
        const TEST_NAME: &str =
            "data_interface::model_update_pending::tests::live_os_kill_preserves_prepared_attempt";

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
        let attempt = IncrementUpdateAttempt {
            dbnum: DBNUM,
            db_type: "DESI".into(),
            file_path: "D:/fixture/os-kill-desi".into(),
            start_sesno: 50,
            end_sesno: 52,
            plan: plan.clone(),
        };

        if std::env::var_os(HELPER_ENV).is_some() {
            aios_core::init_test_surreal()
                .await
                .expect("helper connect surreal");
            prepare_attempt(&attempt)
                .await
                .expect("helper prepare attempt");
            println!("{READY}");
            std::io::stdout().flush().expect("flush ready marker");
            loop {
                std::thread::park();
            }
        }

        let work_id = record_id(&plan.work_items[0]);
        let cleanup = format!(
            "DELETE {ATTEMPT_TABLE}:{DBNUM}; DELETE dbnum_watermark:{DBNUM}; DELETE {work_id};"
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
                    panic!("helper did not report durable prepare: {error}");
                }
            }
        }
        child.kill().expect("terminate helper process");
        assert!(
            !child.wait().expect("wait for killed helper").success(),
            "helper must be terminated, not exit normally"
        );

        assert_eq!(
            load_attempt(DBNUM).await.expect("load after OS kill"),
            Some(attempt)
        );
        finalize_attempt(DBNUM, 52, None, &plan, &[])
            .await
            .expect("recover killed attempt");
        assert_eq!(load_attempt(DBNUM).await.expect("attempt removed"), None);

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
            68,
            "one cascade task plus 67 BRAN roots must complete in one drain"
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
        assert_eq!(consumers.len(), 72, "shared SPCO fixture changed");

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
        assert_eq!(generated.len(), 72, "not every shared consumer regenerated");

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
             DELETE {ATTEMPT_TABLE}:{DBNUM}; \
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
        let attempt = IncrementUpdateAttempt {
            dbnum: DBNUM,
            db_type: "DESI".into(),
            file_path: "capacity-fixture".into(),
            start_sesno: 42,
            end_sesno: 42,
            plan: plan.clone(),
        };

        for _ in 0..2 {
            prepare_attempt(&attempt)
                .await
                .expect("prepare capacity attempt");
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
                    count(SELECT * FROM {ATTEMPT_TABLE}:{DBNUM}) = 0,
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
}
