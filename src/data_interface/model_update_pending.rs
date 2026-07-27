//! Durable, per-target model work queued before the incremental watermark.

use std::collections::HashSet;
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
}

/// 队列行的 id。同一个 (dbnum, action, target) 只占一行，重复入队即幂等更新。
///
/// 房间任务是唯一的例外，它的 id **不带 dbnum**（ADR-010 §7）：一块面板天然跨库，
/// 带上 dbnum 会让同一间房在一轮里排出多行、于是被重算多遍；更糟的是失败之后它只能
/// 等同一个 dbnum 的新会话来复活，而真正触发它的其它库永远够不着——那正是审计里 B6
/// 的放大版。dbnum 与 `source_end_sesno` 因此降为字段，只记最后一次触发来源。
fn record_id(item: &ModelWorkItem) -> String {
    let action = item.action.as_str();
    let target = item.target_refno.replace('/', "_");
    if item.action.is_room_recalc() {
        return format!("{TABLE}:{action}_{target}");
    }
    format!("{TABLE}:{}_{action}_{target}", item.dbnum)
}

/// Persist the exact model work before advancing `applied_sesno`.
pub async fn enqueue_plan(plan: &ModelUpdatePlan) -> anyhow::Result<()> {
    for item in &plan.work_items {
        upsert(item).await?;
    }
    Ok(())
}

/// Translate legacy changed-refno jobs into stable root work. Legacy rows do
/// not retain operations, so this is deliberately a conservative regen-only
/// bridge; new rows always use the exact pre-persist plan.
pub async fn enqueue_legacy_changed_refnos(
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
/// 行 id 不带 dbnum，复活是无条件的。dbnum 跟着 refno 走（与反向级联派生根同一口径），
/// 会话号取 0——两个触发点都在几何刷新那一层，本来就不知道自己属于哪次会话。
fn room_recalc_item(change: &AabbChange) -> ModelWorkItem {
    ModelWorkItem {
        dbnum: change.refno.refno().get_0(),
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
    if !db_option.gen_spatial_tree || changes.is_empty() {
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
    // 房间任务不行：它的行不带 dbnum，同一块面板会被不同库的会话轮流触发，跨库比 sesno
    // 毫无意义（一个库的 500 会永久压住另一个库的 80）。而房间任务的入队条件本身就是
    // 「AABB 真的变了」——每一次入队都是一个全新的重算理由，所以无条件复活。
    let (dbnum_clause, revival_clauses) = if item.action.is_room_recalc() {
        (
            format!("dbnum = math::max([dbnum?:0, {dbnum}])"),
            vec!["attempts = 0".to_string(), "last_error = NONE".to_string()],
        )
    } else {
        (
            format!("dbnum = {dbnum}"),
            vec![
                format!(
                    "attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END"
                ),
                format!(
                    "last_error = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END"
                ),
            ],
        )
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
    clauses.push("status = 'pending'".to_string());
    clauses.push("updated_at = time::now()".to_string());

    format!("UPSERT {id} SET {};", clauses.join(", "))
}

async fn upsert(item: &ModelWorkItem) -> anyhow::Result<()> {
    let id = record_id(item);
    SUL_DB
        .query(render_upsert(item))
        .await
        .map_err(|error| anyhow::anyhow!("persist model work {id} failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("persist model work {id} statement failed: {error}"))?;
    Ok(())
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

    let work = ModelWorkItem {
        dbnum: item.dbnum,
        db_type: item.db_type.clone(),
        source_end_sesno: item.source_end_sesno,
        action: item.action,
        target_refno: item.target_refno.clone(),
        noun: item.noun.clone(),
    };
    let id = record_id(&work);
    SUL_DB
        .query(format!("DELETE {id};"))
        .await
        .map_err(|error| anyhow::anyhow!("delete completed model work {id} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("delete completed model work {id} statement failed: {error}")
        })?;
    Ok(())
}

async fn mark_failed(item: &PendingModelWork, error: &str) -> anyhow::Result<()> {
    let work = ModelWorkItem {
        dbnum: item.dbnum,
        db_type: item.db_type.clone(),
        source_end_sesno: item.source_end_sesno,
        action: item.action,
        target_refno: item.target_refno.clone(),
        noun: item.noun.clone(),
    };
    let id = record_id(&work);
    let error = escape_surql_str(error);
    SUL_DB
        .query(format!(
            "UPDATE {id} SET status = 'failed', attempts = (attempts?:0) + 1, \
             last_error = '{error}', updated_at = time::now();"
        ))
        .await
        .map_err(|query_error| anyhow::anyhow!("mark model work {id} failed: {query_error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("mark model work {id} statement failed: {error}"))?;
    Ok(())
}

pub async fn clear_regen_work(dbnum: u32, root_refno: &str) -> anyhow::Result<()> {
    let work = ModelWorkItem {
        dbnum,
        db_type: String::new(),
        source_end_sesno: 0,
        action: ModelWorkAction::RegenRoot,
        target_refno: root_refno.to_string(),
        noun: String::new(),
    };
    let id = record_id(&work);
    SUL_DB
        .query(format!("DELETE {id};"))
        .await
        .map_err(|error| anyhow::anyhow!("delete completed model work {id} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("delete completed model work {id} statement failed: {error}")
        })?;
    Ok(())
}

pub async fn mark_regen_failed(dbnum: u32, root_refno: &str, error: &str) -> anyhow::Result<()> {
    let item = PendingModelWork {
        dbnum,
        db_type: String::new(),
        source_end_sesno: 0,
        action: ModelWorkAction::RegenRoot,
        target_refno: root_refno.to_string(),
        noun: String::new(),
        status: String::new(),
        attempts: 0,
        last_error: None,
    };
    mark_failed(&item, error).await
}

/// Regeneration work for one root a reverse cascade discovered (pure).
///
/// The derived root is booked against ITS OWN database, not the seed's. Filing
/// a design root under the catalogue `dbnum` that triggered it meant a dead
/// letter could only ever be revived by a new CATALOGUE session, while the
/// design sessions that actually need it regenerated could never reach it.
/// `expand_live_reverse_cascade` already drops every non-design referrer, so
/// what arrives here is a design root.
///
/// `source_end_sesno` is 0 rather than the seed's: session numbers are
/// per-database, so a catalogue sesno of 500 sitting next to design sessions in
/// the 80s would block revival outright. 0 claims no session, which lets the
/// next real session on the design db reset the attempt count as intended.
fn derived_regen_item(
    root: crate::data_interface::generation_root::GenerationRoot,
) -> ModelWorkItem {
    ModelWorkItem {
        dbnum: root.root.refno().get_0(),
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
            run_room_task(&mgr.db_option, &rooms, item.action, refno)
                .await
                .map(|_| ())
        }
    }
}

/// 执行一个房间重算任务，返回本次写入了归属边的构件集合。
async fn run_room_task(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    action: ModelWorkAction,
    target: RefnoEnum,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    match action {
        ModelWorkAction::RoomRecalcPanel => {
            room_model::recalc_panel_membership(db_option, rooms, target).await
        }
        ModelWorkAction::RoomRecalcElement => {
            room_model::recalc_element_membership(db_option, rooms, target).await?;
            Ok(HashSet::new())
        }
        other => anyhow::bail!("{} 不是房间任务", other.as_str()),
    }
}

/// Record a durable failure for one job and collect it for the drain summary.
///
/// Clearing the queue row counts the same as the work itself: a target whose row
/// can never be removed keeps climbing towards [`MAX_ATTEMPTS`] instead of
/// re-running a full generation every watcher cycle forever.
async fn record_failure(job: &PendingModelWork, error: &anyhow::Error, failures: &mut Vec<String>) {
    let message = format!("{error:#}");
    if let Err(mark_error) = mark_failed(job, &message).await {
        failures.push(format!(
            "{} {}: {message}; mark failed: {mark_error:#}",
            job.action.as_str(),
            job.target_refno
        ));
    } else {
        failures.push(format!(
            "{} {}: {message}",
            job.action.as_str(),
            job.target_refno
        ));
    }
}

/// Run one job on its own, recording a durable failure rather than aborting the
/// drain, so a single broken target cannot stall the rest of the queue.
///
/// This is infallible on purpose. Returning `Err` here — as the queue-row delete
/// used to — aborted the whole round on one flaky `DELETE`, so every other
/// `dbnum` queued behind it was skipped and the target that had just generated
/// successfully paid for a second full `gen_all_geos_data` on the next round.
async fn run_one(
    mgr: &AiosDBManager,
    job: &PendingModelWork,
    done: &mut usize,
    failures: &mut Vec<String>,
) {
    let outcome = match execute_item(mgr, job).await {
        Ok(()) => delete_work(job).await,
        Err(error) => Err(error),
    };
    match outcome {
        Ok(()) => *done += 1,
        Err(error) => record_failure(job, &error, failures).await,
    }
}

/// Render the drain SELECT. Work at or above [`MAX_ATTEMPTS`] stays in the
/// table as a dead letter: the automatic watcher never picks it up again,
/// while manual preview/retry reads the table without this cap and remains
/// the way to inspect or revive it.
fn render_drain_select(action_filter: &str) -> String {
    format!(
        "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
         AND (attempts?:0) < {MAX_ATTEMPTS} {action_filter} ORDER BY updated_at ASC;"
    )
}

/// Only never-failed, parseable roots share a batch. `generate_roots` is all
/// or nothing, so re-admitting a root that already failed would fail the
/// whole batch again on every later drain and re-pay the per-root fallback
/// for every healthy neighbour queued alongside it.
fn joins_regen_batch(job: &PendingModelWork) -> bool {
    job.attempts == 0 && RefU64::from_str(&job.target_refno).is_ok()
}

/// Drain pending work independently. Failures remain durable and are retried on
/// a later watcher/manual invocation, even when there is no new session.
async fn drain_where(mgr: &AiosDBManager, action_filter: &str) -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(render_drain_select(action_filter))
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

    let mut done = 0;
    let mut failures = Vec::new();

    if !batchable.is_empty() {
        let mut roots: Vec<String> = Vec::with_capacity(batchable.len());
        for job in &batchable {
            if !roots.contains(&job.target_refno) {
                roots.push(job.target_refno.clone());
            }
        }
        match crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(mgr, &roots)
            .await
        {
            Ok(()) => {
                for job in &batchable {
                    match delete_work(job).await {
                        Ok(()) => done += 1,
                        Err(error) => record_failure(job, &error, &mut failures).await,
                    }
                }
            }
            Err(error) => {
                println!(
                    "批量重生成 {} 个根失败，回退逐根重试以定位问题根: {error:#}",
                    roots.len()
                );
                for job in &batchable {
                    run_one(mgr, job, &mut done, &mut failures).await;
                }
            }
        }
    }

    for job in singles.iter().chain(other_jobs.iter()) {
        run_one(mgr, job, &mut done, &mut failures).await;
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} pending model task(s) failed after {done} completed: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(done)
}

// 三个阶段的 action 白名单。合起来必须正好覆盖 `ModelWorkAction` 的全部取值：漏掉
// 一种，那种任务入了队就永远不会被消费，而且没有任何报错——它只是静静躺在表里。
// `every_action_is_consumed_by_exactly_one_drain_phase` 守着这条。
const NON_REGEN_ACTION_FILTER: &str =
    "AND action IN ['transform', 'delete_cleanup', 'cascade_expand']";
const REGEN_ACTION_FILTER: &str = "AND action = 'regen_root'";
const ROOM_ACTION_FILTER: &str = "AND action IN ['room_recalc_panel', 'room_recalc_element']";

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    // 三个阶段的先后是硬约束，不是习惯：
    // 1. 非 regen 先跑——`cascade_expand` 会反过来入队 regen 工作；
    // 2. regen 次之——房间归属要读几何与包围盒，在重生成之前算出来的结果本身就是错的；
    // 3. 房间最后（ADR-010 §7）。
    let phases = [
        ("non-regen", drain_non_regen(mgr).await),
        ("regen", drain_where(mgr, REGEN_ACTION_FILTER).await),
        ("room recalc", drain_rooms(&mgr.db_option).await),
    ];

    let mut done = 0;
    let mut failures = Vec::new();
    for (phase, outcome) in phases {
        match outcome {
            Ok(count) => done += count,
            Err(error) => failures.push(format!("{phase} pending tasks failed: {error:#}")),
        }
    }
    if !failures.is_empty() {
        anyhow::bail!("{}", failures.join("; "));
    }
    Ok(done)
}

pub async fn drain_non_regen(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    drain_where(mgr, NON_REGEN_ACTION_FILTER).await
}

/// 前两个阶段（非 regen → regen），不含房间。
///
/// 数据批次 worker 的空闲轮用它消化积压：房间收敛按 ADR-011 §8 在队列跑空时
/// 单独收一轮（包成 `room_recalc` 任务），不跟在积压消化后面顺手带走——那样
/// 房间轮就没有自己的任务行了。
pub async fn drain_data_phases(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    let phases = [
        ("non-regen", drain_non_regen(mgr).await),
        ("regen", drain_where(mgr, REGEN_ACTION_FILTER).await),
    ];
    let mut done = 0;
    let mut failures = Vec::new();
    for (phase, outcome) in phases {
        match outcome {
            Ok(count) => done += count,
            Err(error) => failures.push(format!("{phase} pending tasks failed: {error:#}")),
        }
    }
    if !failures.is_empty() {
        anyhow::bail!("{}", failures.join("; "));
    }
    Ok(done)
}

/// 还活着（未到重试上限）的待重算房间目标数，供空闲轮决定要不要收房间
/// 并给 `room_recalc` 任务当 total。
pub async fn count_live_room_targets() -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT count() AS c FROM {TABLE} WHERE status IN ['pending', 'failed'] \
             AND (attempts?:0) < {MAX_ATTEMPTS} {ROOM_ACTION_FILTER} GROUP ALL;"
        ))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("count pending room work statement failed: {error}"))?;
    #[derive(serde::Deserialize)]
    struct CountRow {
        c: usize,
    }
    let rows: Vec<CountRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending room count failed: {error}"))?;
    Ok(rows.first().map(|r| r.c).unwrap_or(0))
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
        .query(render_drain_select(ROOM_ACTION_FILTER))
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
    let mut done = 0;
    let mut failures = Vec::new();
    let mut claimed: HashSet<RefnoEnum> = HashSet::new();

    for job in &panels {
        match run_room_job(db_option, &rooms, job).await {
            Ok(members) => {
                claimed.extend(members);
                match delete_work(job).await {
                    Ok(()) => done += 1,
                    Err(error) => record_failure(job, &error, &mut failures).await,
                }
            }
            Err(error) => record_failure(job, &error, &mut failures).await,
        }
    }

    for job in &elements {
        // 整间分支刚刚把这个构件写进某块面板的成员里，它的元素任务就是重复劳动：
        // 两条分支共用判定与边 id，再跑一遍只会得到同一批边。
        let absorbed = RefU64::from_str(&job.target_refno)
            .is_ok_and(|refno| claimed.contains(&RefnoEnum::from(refno)));
        let outcome = if absorbed {
            delete_work(job).await
        } else {
            match run_room_job(db_option, &rooms, job).await {
                Ok(_) => delete_work(job).await,
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) => done += 1,
            Err(error) => record_failure(job, &error, &mut failures).await,
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} pending room task(s) failed after {done} completed: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(done)
}

async fn run_room_job(
    db_option: &aios_core::options::DbOption,
    rooms: &room_model::RoomPanelMap,
    job: &PendingModelWork,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let refno = RefnoEnum::from(
        RefU64::from_str(&job.target_refno)
            .map_err(|_| anyhow::anyhow!("invalid pending refno {}", job.target_refno))?,
    );
    run_room_task(db_option, rooms, job.action, refno).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn record_id_is_stable_per_dbnum_action_and_target() {
        let item = ModelWorkItem {
            dbnum: 8191,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RegenRoot,
            target_refno: "16777216/5".into(),
            noun: "BRAN".into(),
        };
        assert_eq!(
            record_id(&item),
            "model_update_pending:8191_regen_root_16777216_5"
        );
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

    /// B6：反向级联派生出来的根记在**它自己的**设计库账上。继承种子的 dbnum 时，
    /// 一个目录库触发的设计根会被记在目录库下，于是它的死信只能等下一次目录库会话
    /// 来复活——而真正需要它重生成的设计库会话永远够不着它。会话号同理：跨库比大小
    /// 没有意义，所以派生任务不认领任何会话号。
    #[test]
    fn a_cascade_derived_root_is_booked_against_its_own_design_db() {
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};

        let item = derived_regen_item(GenerationRoot {
            root: RefnoEnum::from(RefU64((24381u64 << 32) | 100677)),
            noun: "EQUI".into(),
            name: "/PUMP-01".into(),
            kind: GenerationRootKind::DeliveryUnit,
        });

        assert_eq!(item.dbnum, 24381, "派生根应记在设计库，而不是种子所在的目录库");
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

        // 其余任务照旧按库分行。
        let regen = ModelWorkItem {
            action: ModelWorkAction::RegenRoot,
            ..room_item(ModelWorkAction::RegenRoot, 24381, 42)
        };
        assert_eq!(
            record_id(&regen),
            "model_update_pending:24381_regen_root_24381_34303"
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
        // dbnum 跟着 refno 走，而不是跟着触发它的那个库；会话号不认领。
        assert_eq!(panel.dbnum, 24381);
        assert_eq!(panel.source_end_sesno, 0);

        assert_eq!(
            room_recalc_item(&change(100677, "EQUI")).action,
            ModelWorkAction::RoomRecalcElement
        );
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
        let sql = render_drain_select("AND action = 'regen_root'");
        assert!(
            sql.contains(&format!("(attempts?:0) < {MAX_ATTEMPTS}")),
            "{sql}"
        );
        assert!(sql.contains("status IN ['pending', 'failed']"), "{sql}");
        assert!(sql.contains("AND action = 'regen_root'"), "{sql}");
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
        assert!(sql.contains("UPSERT model_update_pending:8191_regen_root_16777216_5"));
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
            .find("UPSERT model_update_pending:7997_regen_root_24381_2")
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
}
