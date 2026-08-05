//! Durable, per-target model work queued before the incremental watermark.

use std::collections::{BTreeSet, HashSet};
use std::str::FromStr;

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use serde::{Deserialize, Serialize};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::occ_generate::AabbChange;
use crate::fast_model::room_model;

pub const TABLE: &str = "model_update_pending";
pub const ATTEMPT_TABLE: &str = "increment_update_attempt";
const QUERY_CHUNK: usize = 500;
// ponytail: one bounded idle page may still delay a new batch; lower this if the
// measured generation latency exceeds the queue SLA.
const DRAIN_PAGE_SIZE: usize = 64;

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

/// Persist the exact model work before advancing `applied_sesno`.
pub async fn enqueue_plan(plan: &ModelUpdatePlan) -> anyhow::Result<()> {
    for chunk in plan.work_items.chunks(QUERY_CHUNK) {
        SUL_DB
            .query(
                chunk
                    .iter()
                    .map(render_upsert)
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
fn room_recalc_item(change: &AabbChange) -> ModelWorkItem {
    ModelWorkItem {
        dbnum: 0,
        db_type: "DESI".to_string(),
        source_end_sesno: 0,
        action: if change.noun == "PANE" {
            ModelWorkAction::RoomRecalcPanel
        } else {
            ModelWorkAction::RoomRecalcElement
        },
        target_refno: change.refno.to_pdms_str(),
        noun: change.noun.clone(),
    }
}

/// 包围盒真的变了 → 排一次房间归属重算。
///
/// 只接受**变更集**：同一轮里同一个目标只需要一行，因此先按目标折叠再落库——队列行
/// 的 id 本来就幂等，重复入队只是白跑一趟往返。
///
/// `gen_spatial_tree` 关着时一条都不排。那个开关同时管着全量重建与空间树对账：关着
/// 意味着 `build_room_relations` 从不运行、树也从不与库对账，此时跑增量不只是徒劳
/// ——元素分支是「先删该构件的所有入边再写回」，而候选面板取自那棵没人维护的树，
/// 捞不到候选就只剩下那条 DELETE，等于把上一次全量建出来的边悄悄抹掉。
pub async fn enqueue_room_recalc(
    db_option: &aios_core::options::DbOption,
    changes: &[AabbChange],
) -> anyhow::Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    if !db_option.gen_spatial_tree {
        warn_spatial_tree_disabled_once();
        return Ok(());
    }
    let mut items: std::collections::BTreeMap<String, ModelWorkItem> =
        std::collections::BTreeMap::new();
    for change in changes {
        let item = room_recalc_item(change);
        items.insert(item.target_refno.clone(), item);
    }
    enqueue_plan(&ModelUpdatePlan {
        work_items: items.into_values().collect(),
        ..Default::default()
    })
    .await
}

static SPATIAL_TREE_DISABLED_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// `gen_spatial_tree` 关着时一条房间任务都不排——按进程说一次。
///
/// 这是配置层面的事实，不必每批包围盒变更各说一遍。但**必须说**：这条路上队列里从此
/// 不会出现任何 `room_recalc` 行，`room_round` 每轮早退、泳道空着，现场能看到的只有
/// 「模型动了、房间号不动」，没有任何东西解释为什么。
fn warn_spatial_tree_disabled_once() {
    if !SPATIAL_TREE_DISABLED_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        println!(
            "gen_spatial_tree 关闭：包围盒变更不排房间重算任务，room_relate 与材料表房间号\
             将停在上一次全量重建的结果"
        );
    }
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

fn render_upsert(item: &ModelWorkItem) -> String {
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
    let revival_clauses = if revives_unconditionally(item) {
        vec!["attempts = 0".to_string(), "last_error = NONE".to_string()]
    } else {
        vec![
            format!("attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END"),
            format!(
                "last_error = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END"
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
    // 复活子句读的是 `source_end_sesno` 的**旧值**，所以必须排在它被覆盖之前。
    clauses.push(format!(
        "source_end_sesno = math::max([source_end_sesno?:0, {end_sesno}])"
    ));
    clauses.push("revision = (revision?:0) + 1".to_string());
    clauses.push("status = 'pending'".to_string());
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
fn render_watermark_advance(dbnum: u32, end_sesno: i32) -> String {
    format!(
        "UPSERT dbnum_watermark:{dbnum} SET dbnum = {dbnum}, \
         applied_sesno = math::max([applied_sesno?:0, {end_sesno}]), \
         sesno = math::max([sesno?:0, {end_sesno}]), \
         applied_at = time::now(), updated_at = time::now();"
    )
}

/// Render the single transaction that closes a window: first the caller's
/// `window_statements` (side effects that must share this watermark's fate),
/// then the durable model work, the watermark advance and the recovery-record
/// removal.
fn render_finalize_transaction(
    dbnum: u32,
    end_sesno: i32,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> String {
    let mut statements = window_statements.to_vec();
    statements.extend(plan.work_items.iter().map(render_upsert));
    statements.push(render_watermark_advance(dbnum, end_sesno));
    statements.push(format!("DELETE {ATTEMPT_TABLE}:{dbnum};"));
    format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        statements.join("\n")
    )
}

/// Render the transaction that closes a freshly parsed baseline.
///
/// Same collar as [`render_finalize_transaction`] minus the recovery-record
/// removal: a baseline is not a replayable window, so it never has an
/// `increment_update_attempt` row, and deleting one here could only discard
/// another path's crash-recovery state.
fn render_baseline_transaction(dbnum: u32, end_sesno: i32, plan: &ModelUpdatePlan) -> String {
    let mut statements: Vec<String> = plan.work_items.iter().map(render_upsert).collect();
    statements.push(render_watermark_advance(dbnum, end_sesno));
    format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        statements.join("\n")
    )
}

/// Atomically establish durable model work, advance the authoritative
/// watermark, and remove the recovery record.
///
/// `window_statements` carries writes that must not outlive a rolled-back
/// window nor be lost under an advancing watermark — currently this window's
/// `datacenter_version` status updates. Committing those separately would let a
/// delivery-status write fail while the watermark still moved past it, and no
/// later window would ever repair the miss.
pub async fn finalize_attempt(
    dbnum: u32,
    end_sesno: i32,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<()> {
    SUL_DB
        .query(render_finalize_transaction(
            dbnum,
            end_sesno,
            plan,
            window_statements,
        ))
        .await
        .map_err(|error| {
            anyhow::anyhow!("finalize increment attempt dbnum={dbnum} failed: {error}")
        })?
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
    plan: &ModelUpdatePlan,
) -> anyhow::Result<()> {
    SUL_DB
        .query(render_baseline_transaction(dbnum, end_sesno, plan))
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

    let target = &item.target_refno;
    SUL_DB
        .query(render_delete_work(item))
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
        .query(render_upsert(&item))
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
        .map_err(|error| anyhow::anyhow!("mark model work {root_refno} statement failed: {error}"))?;
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
            crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(
                mgr,
                &[item.target_refno.clone()],
            )
            .await
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
        // 单件执行路径：自己加载一次房间映射。批量消费走 [`drain_rooms`]，它按轮加载
        // 一次并在整轮复用——房间映射是一次房间类型表全表扫描加逐行图遍历，几十个任务
        // 各扫一遍是承受不起的。
        ModelWorkAction::RoomRecalcElement | ModelWorkAction::RoomRecalcPanel => {
            let rooms = room_model::load_room_panel_map(&mgr.db_option).await?;
            let coverage = room_model::survey_panel_tree_coverage(&rooms).await?;
            run_room_task(&mgr.db_option, &rooms, coverage, item.action, refno)
                .await
                .map(|_| ())
        }
    }
}

/// 执行一个房间重算任务，返回本次写入了归属边的构件集合。
async fn run_room_task(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    coverage: room_model::PanelTreeCoverage,
    action: ModelWorkAction,
    target: RefnoEnum,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    match action {
        ModelWorkAction::RoomRecalcPanel => {
            room_model::recalc_panel_membership(db_option, rooms, target).await
        }
        ModelWorkAction::RoomRecalcElement => {
            room_model::recalc_element_membership(db_option, rooms, coverage, target).await?;
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
async fn run_one(mgr: &AiosDBManager, job: &PendingModelWork, report: &mut DrainReport) {
    let root_lock = (job.action == ModelWorkAction::RegenRoot).then(|| {
        crate::data_interface::manual_update::generation_root_lock(&job.target_refno)
    });
    let _root_guard = match &root_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let outcome = match execute_item(mgr, job).await {
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
    let limit = limit.map(|value| format!(" LIMIT {value}")).unwrap_or_default();
    format!(
        "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
         AND (attempts?:0) < {MAX_ATTEMPTS} {action_filter} \
         ORDER BY updated_at ASC{limit};"
    )
}

/// Only never-failed, parseable roots share a batch. `generate_roots` is all
/// or nothing, so re-admitting a root that already failed would fail the
/// whole batch again on every later drain and re-pay the per-root fallback
/// for every healthy neighbour queued alongside it.
pub(crate) fn root_joins_regen_batch(attempts: u32, target_refno: &str) -> bool {
    attempts == 0 && RefU64::from_str(target_refno).is_ok()
}

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
    let (batchable, singles): (Vec<PendingModelWork>, Vec<PendingModelWork>) =
        regen_jobs.into_iter().partition(joins_regen_batch);

    let mut report = DrainReport::default();

    if !batchable.is_empty() {
        let mut roots: Vec<String> = Vec::with_capacity(batchable.len());
        for job in &batchable {
            if !roots.contains(&job.target_refno) {
                roots.push(job.target_refno.clone());
            }
        }
        let mut lock_roots = roots.clone();
        lock_roots.sort_unstable();
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
                let settlements = batchable
                    .iter()
                    .map(|job| (job.target_refno.clone(), job.revision))
                    .collect::<Vec<_>>();
                match clear_regen_work_batch(&settlements).await {
                    Ok(()) => report.done += batchable.len(),
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
                        for job in &batchable {
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
                for job in &batchable {
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
const DATA_ACTION_FILTER: &str =
    "AND action IN ['transform', 'delete_cleanup', 'cascade_expand', 'regen_root']";
const ROOM_ACTION_FILTER: &str = "AND action IN ['room_recalc_panel', 'room_recalc_element']";

fn data_phase_is_clear(succeeded: bool, has_more: bool) -> bool {
    succeeded && !has_more
}

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    // 三个阶段的先后是硬约束，不是习惯：
    // 1. 非 regen 先跑——`cascade_expand` 会反过来入队 regen 工作；
    // 2. regen 次之——房间归属要读几何与包围盒，在重生成之前算出来的结果本身就是错的；
    // 3. 房间最后（ADR-010 §7）。
    let mut done = drain_non_regen(mgr).await?;
    done += drain_where(mgr, REGEN_ACTION_FILTER, None).await?;
    done += drain_rooms(&mgr.db_option).await?;
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

/// 第三阶段：房间归属重算。
///
/// 不复用 [`drain_where`] 的原因有两个。其一，房间映射要按轮加载一次而不是按任务；
/// 其二，整间分支必须先于元素分支跑完，才能把它已经写过的构件从元素任务里摘掉
/// （ADR-010 §8 的冲突规则）——这两条 `drain_where` 的通用循环都表达不了。
///
/// 队列行级别的去重不需要在内存里再做一遍：房间任务的 record id 已经是
/// `{action}_{target}`（不带 dbnum），同一个目标天然只占一行。
///
/// 取 `DbOption` 而不是 `AiosDBManager`：这一阶段只用得到配置，收窄参数也让合成夹具
/// 能用它自己的房间关键字驱动整个阶段——`init_form_config()` 读的是项目配置，夹具那间
/// `/ZZ-R-K100` 在默认关键字下根本匹配不到。
pub async fn drain_rooms(db_option: &aios_core::options::DbOption) -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(render_drain_select(ROOM_ACTION_FILTER, None))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("load pending room work statement failed: {error}"))?;
    let jobs: Vec<PendingModelWork> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending room work failed: {error}"))?;
    if jobs.is_empty() {
        return Ok(0);
    }

    let (panels, elements): (Vec<PendingModelWork>, Vec<PendingModelWork>) = jobs
        .into_iter()
        .partition(|job| job.action == ModelWorkAction::RoomRecalcPanel);

    let rooms = room_model::load_room_panel_map(db_option).await?;
    // 元素分支的候选面板只能从空间树上取，量一次覆盖率整轮复用（见
    // [`room_model::PanelTreeCoverage`]）。树里一块在册面板都没有时元素分支会拒绝把
    // 「捞不到候选」当成「不在任何房间里」，宁可失败重试也不静默清掉存量归属边。
    let coverage = room_model::survey_panel_tree_coverage(&rooms).await?;
    if !coverage.can_trust_empty_candidates() {
        println!(
            "空间树上一块在册面板都没有（在册 {} 块）：本轮元素任务不会改写房间归属，\
             等空间树与库对账重建后重试",
            coverage.registered
        );
    }
    let mut report = DrainReport::default();
    let mut claimed_members: HashSet<RefnoEnum> = HashSet::new();
    let mut claimed_panels: HashSet<RefnoEnum> = HashSet::new();

    for job in &panels {
        match run_room_job(db_option, &rooms, coverage, job).await {
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

    // 吸收的封闭性输入（ADR-010 §8，2026-07-28 修订）只为真正的候选加载一次；
    // 加载失败不放大成整轮失败，但**一个都不吸收**——封闭性未知时把元素任务照跑
    // 一遍只是多花一次网格判定，错吸收却会把陈旧边永久留在库里。
    let absorb_candidates: Vec<RefnoEnum> = elements
        .iter()
        .filter_map(|job| RefU64::from_str(&job.target_refno).ok())
        .map(RefnoEnum::from)
        .filter(|refno| claimed_members.contains(refno))
        .collect();
    let closure_inputs = if absorb_candidates.is_empty() {
        None
    } else {
        match load_absorption_closure_inputs(&rooms, &absorb_candidates).await {
            Ok(inputs) => Some(inputs),
            Err(error) => {
                println!("吸收封闭性输入加载失败，本轮不吸收任何元素任务: {error:#}");
                None
            }
        }
    };

    for job in &elements {
        // 整间分支刚把它写进某块面板的成员里，且它的旧归属面板与当前候选面板**全部**
        // 落在本轮已重算面板集合之内时，元素任务才是重复劳动。封闭性不成立就照跑：
        // 只有元素分支那条「删全部入边」能清掉本轮没重算的面板指向它的陈旧边、
        // 写上它新进入的本轮外面板的边——构件与新面板同轮搬迁而旧面板不在本轮时，
        // 无条件吸收会把旧面板的边永久留在库里。
        let absorbed = RefU64::from_str(&job.target_refno)
            .ok()
            .map(RefnoEnum::from)
            .is_some_and(|refno| {
                claimed_members.contains(&refno)
                    && closure_inputs
                        .as_ref()
                        .is_some_and(|inputs| absorption_verdict(inputs, refno, &claimed_panels))
            });
        let outcome = if absorbed {
            delete_work(job).await
        } else {
            match run_room_job(db_option, &rooms, coverage, job).await {
                Ok(_) => delete_work(job).await,
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) => report.done += 1,
            Err(error) => record_failure(job, &error, &mut report).await,
        }
    }

    if !report.failures.is_empty() {
        anyhow::bail!(
            "{} pending room task(s) failed after {} completed: {}",
            report.failures.len(),
            report.done,
            report.failures.join("; ")
        );
    }
    Ok(report.done)
}

/// 同轮吸收的封闭性输入：候选元素的现存归属边与当前空间树候选面板。
#[derive(Debug, Default)]
struct AbsorptionClosureInputs {
    /// 元素 → 现存 `room_relate` 入边的面板集合。没有旧边的元素不在映射里（等价空集）。
    old_edge_panels: std::collections::HashMap<RefnoEnum, HashSet<RefnoEnum>>,
    /// 元素 → 当前包围盒在空间树上命中的**在册** PANE 集合。树里没有该元素条目时
    /// 不在映射里——候选未知，吸收判定必须让路。
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

/// 为本轮吸收候选批量加载封闭性输入：一条 SELECT 查旧边，一次树遍历取候选面板。
async fn load_absorption_closure_inputs(
    rooms: &room_model::RoomPanelMap,
    elements: &[RefnoEnum],
) -> anyhow::Result<AbsorptionClosureInputs> {
    use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};

    let mut inputs = AbsorptionClosureInputs::default();

    let keys = elements
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    #[derive(serde::Deserialize)]
    struct EdgeRow {
        panel: RefnoEnum,
        element: RefnoEnum,
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT in.id AS panel, out.id AS element FROM room_relate WHERE out IN [{keys}];"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("查询吸收候选的现存归属边失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("查询吸收候选的现存归属边语句失败: {error}"))?;
    for row in response
        .take::<Vec<EdgeRow>>(0)
        .map_err(|error| anyhow::anyhow!("解析吸收候选的现存归属边失败: {error}"))?
    {
        inputs
            .old_edge_panels
            .entry(row.element)
            .or_default()
            .insert(row.panel);
    }

    // 候选面板用元素在树上的包围盒条目求交——树与库由同一次刷新维护（记录先落、
    // 指针后落、写入全部成功才动树），此处与元素分支读到的是同一份现状。
    load_aabb_tree().await?;
    let tree = GLOBAL_AABB_TREE.read().await;
    let wanted: std::collections::HashMap<RefU64, RefnoEnum> = elements
        .iter()
        .map(|refno| (refno.refno(), *refno))
        .collect();
    let mut element_aabbs: Vec<(RefnoEnum, parry3d::bounding_volume::Aabb)> = Vec::new();
    for bbox in tree.tree.iter() {
        if let Some(&element) = wanted.get(&bbox.refno) {
            element_aabbs.push((element, bbox.aabb));
        }
    }
    for (element, aabb) in element_aabbs {
        let candidates: HashSet<RefnoEnum> = tree
            .locate_intersecting_bounds(&aabb)
            .filter(|bbox| bbox.noun == "PANE")
            .map(|bbox| RefnoEnum::from(bbox.refno))
            .filter(|panel| rooms.room_num_of(*panel).is_some())
            .collect();
        inputs.candidate_panels.insert(element, candidates);
    }
    Ok(inputs)
}

async fn run_room_job(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    coverage: room_model::PanelTreeCoverage,
    job: &PendingModelWork,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&job.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid pending refno {}", job.target_refno))?,
    );
    run_room_task(db_option, rooms, coverage, job.action, refno).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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
        assert!(sql.starts_with("UPDATE"), "复活不是入队，不得 UPSERT: {sql}");
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
            !settlement_arm.contains("record_failure(")
                && !settlement_arm.contains("mark_failed("),
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
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            revision: 7,
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
            render_upsert(&item).contains("revision = (revision?:0) + 1"),
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
        let sql = render_upsert(&ModelWorkItem {
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
        assert_eq!(item.source_end_sesno, 0, "跨库会话号不可比，派生任务不认领会话");
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
        stale_old.candidate_panels.insert(element, [panel(10)].into());
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
        let sql = render_upsert(&room_item(ModelWorkAction::RoomRecalcPanel, 24381, 42));
        assert!(sql.contains("attempts = 0"), "{sql}");
        assert!(sql.contains("last_error = NONE"), "{sql}");
        assert!(
            !sql.contains("IF 42 > (source_end_sesno?:0)"),
            "房间任务不应按会话号决定是否复活: {sql}"
        );
        // dbnum / source_end_sesno 降为字段，只记最后一次触发来源。
        assert!(sql.contains("dbnum = math::max([dbnum?:0, 24381])"), "{sql}");
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

        let sql = render_upsert(&derived);
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
        let sql = render_upsert(&derived);
        assert!(
            sql.contains("dbnum = dbnum?:0"),
            "不认领的入队必须保留行上已存的库号: {sql}"
        );

        // 认领了库号的常规入队照写本次来源，行为不变。
        let claiming = render_upsert(&ModelWorkItem {
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
        let sql = render_upsert(&ModelWorkItem {
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

    /// 每一种 action 都必须被某个 drain 阶段消费，且只被一个消费。
    ///
    /// 漏掉一种，那种任务入队之后就永远躺在表里，不报错也不执行；被两个阶段同时选中，
    /// 则会在同一轮里跑两遍。新增 action 时下面的 `match` 会编译失败，逼调用方明确
    /// 它归哪个阶段。
    #[test]
    fn every_action_is_consumed_by_exactly_one_drain_phase() {
        const ALL_ACTIONS: [ModelWorkAction; 6] = [
            ModelWorkAction::RegenRoot,
            ModelWorkAction::Transform,
            ModelWorkAction::DeleteCleanup,
            ModelWorkAction::CascadeExpand,
            ModelWorkAction::RoomRecalcElement,
            ModelWorkAction::RoomRecalcPanel,
        ];
        let declared_phase = |action: ModelWorkAction| match action {
            ModelWorkAction::RegenRoot => REGEN_ACTION_FILTER,
            ModelWorkAction::Transform
            | ModelWorkAction::DeleteCleanup
            | ModelWorkAction::CascadeExpand => NON_REGEN_ACTION_FILTER,
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

    /// `gen_spatial_tree` 关着是「一条房间任务都不排」，这件事必须留下痕迹。
    ///
    /// 静默返回 `Ok(())` 时现场只看得到「模型动了、房间号不动」：队列里没有 room_recalc
    /// 行、房间轮每轮早退、泳道空着，没有任何一处解释原因。
    #[test]
    fn a_disabled_spatial_tree_says_so_before_dropping_room_work() {
        let source = include_str!("model_update_pending.rs");
        let body = source
            .split_once("pub async fn enqueue_room_recalc(")
            .expect("enqueue_room_recalc 必须存在")
            .1
            .split_once("\nstatic SPATIAL_TREE_DISABLED_WARNED")
            .expect("入队函数之后是告警开关")
            .0;
        let gate_at = body
            .find("if !db_option.gen_spatial_tree {")
            .expect("开关必须单独成一个分支，不能与 changes.is_empty() 并在一起——\
                     没有变更要排本来就无话可说");
        let warn_at = body
            .find("warn_spatial_tree_disabled_once()")
            .expect("关着时必须留下痕迹");
        assert!(gate_at < warn_at, "{body}");
    }

    #[test]
    fn regen_waits_for_a_successful_and_empty_non_regen_phase() {
        assert!(data_phase_is_clear(true, false));
        assert!(!data_phase_is_clear(true, true), "the 65th item keeps the barrier closed");
        assert!(!data_phase_is_clear(false, false), "a failed page keeps the barrier closed");
    }

    #[test]
    fn only_fresh_parseable_roots_join_the_regen_batch() {
        let fresh = PendingModelWork {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            revision: 1,
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

    #[test]
    fn finalization_is_one_transaction_with_delivery_status_work_watermark_and_cleanup() {
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
        let sql = render_finalize_transaction(8191, 42, &plan, &[delivery_status.to_string()]);
        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.contains("UPSERT model_update_pending:regen_root_16777216_5"));
        assert!(sql.contains("applied_sesno = math::max([applied_sesno?:0, 42])"));
        assert!(sql.contains("DELETE increment_update_attempt:8191"));
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");

        // A delivery-status write must share the watermark's fate, so it belongs
        // inside this transaction and ahead of the advance that publishes it.
        let status_at = sql
            .find(delivery_status)
            .unwrap_or_else(|| panic!("delivery status must ride the finalize transaction: {sql}"));
        let watermark_at = sql
            .find("UPSERT dbnum_watermark:8191")
            .unwrap_or_else(|| panic!("{sql}"));
        assert!(status_at < watermark_at, "{sql}");
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

        let sql = render_baseline_transaction(7997, 76, &plan);
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

        finalize_attempt(DBNUM, 42, &plan, &[])
            .await
            .expect("first finalize");
        assert_eq!(load_attempt(DBNUM).await.expect("attempt removed"), None);

        // Replay the post-crash finalization: stable work id + max watermark
        // must keep exactly one task and the same applied sesno.
        finalize_attempt(DBNUM, 42, &plan, &[])
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
        finalize_attempt(DBNUM, 52, &plan, &[])
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
        finalize_attempt(DBNUM, END_SESNO, &plan, &[])
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
            finalize_attempt(DBNUM, 42, &plan, &delivery)
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
