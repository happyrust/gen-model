//! Durable, per-target model work queued before the incremental watermark.

use std::collections::HashSet;
use std::str::FromStr;

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use serde::{Deserialize, Serialize};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::tidb_manager::AiosDBManager;

pub const TABLE: &str = "model_update_pending";
pub const ATTEMPT_TABLE: &str = "increment_update_attempt";

/// Retry ceiling per work item (same policy as `side_effect_pending`). A job
/// that keeps failing stays in the table as an inspectable dead letter instead
/// of burning a generator run every watcher cycle forever; it revives
/// automatically because [`render_upsert`] resets `attempts` whenever a newer
/// session touches the same target.
const MAX_ATTEMPTS: u32 = 5;

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

fn record_id(item: &ModelWorkItem) -> String {
    format!(
        "{TABLE}:{}_{}_{}",
        item.dbnum,
        item.action.as_str(),
        item.target_refno.replace('/', "_")
    )
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

pub async fn enqueue_legacy_root(
    dbnum: u32,
    end_sesno: i32,
    root_refno: &str,
    noun: &str,
) -> anyhow::Result<()> {
    enqueue_plan(&ModelUpdatePlan {
        work_items: vec![ModelWorkItem {
            dbnum,
            db_type: "DESI".into(),
            source_end_sesno: end_sesno,
            action: ModelWorkAction::RegenRoot,
            target_refno: root_refno.to_string(),
            noun: noun.to_string(),
        }],
        warnings: Vec::new(),
    })
    .await
}

fn render_upsert(item: &ModelWorkItem) -> String {
    let id = record_id(item);
    let db_type = escape_surql_str(&item.db_type);
    let target = escape_surql_str(&item.target_refno);
    let noun = escape_surql_str(&item.noun);
    let end_sesno = item.source_end_sesno;
    format!(
        "UPSERT {id} SET \
         dbnum = {dbnum}, db_type = '{db_type}', action = '{action}', \
         target_refno = '{target}', noun = '{noun}', \
         attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END, \
         last_error = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END, \
         source_end_sesno = math::max([source_end_sesno?:0, {end_sesno}]), \
         status = 'pending', updated_at = time::now();",
        dbnum = item.dbnum,
        action = item.action.as_str(),
    )
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
    statements.push(format!(
        "UPSERT dbnum_watermark:{dbnum} SET dbnum = {dbnum}, \
         applied_sesno = math::max([applied_sesno?:0, {end_sesno}]), \
         sesno = math::max([sesno?:0, {end_sesno}]), \
         applied_at = time::now(), updated_at = time::now();"
    ));
    statements.push(format!("DELETE {ATTEMPT_TABLE}:{dbnum};"));
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
            let work_items = roots
                .into_iter()
                .map(|root| ModelWorkItem {
                    dbnum: item.dbnum,
                    db_type: item.db_type.clone(),
                    source_end_sesno: item.source_end_sesno,
                    action: ModelWorkAction::RegenRoot,
                    target_refno: root.root.to_pdms_str(),
                    noun: root.noun,
                })
                .collect();
            enqueue_plan(&ModelUpdatePlan {
                work_items,
                warnings: Vec::new(),
            })
            .await
        }
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

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    // Cascade expansion enqueues regen work, so consume non-regen work first
    // and load regen work only after those roots are durable.
    let non_regen = drain_non_regen(mgr).await;
    let regen = drain_where(mgr, "AND action = 'regen_root'").await;
    match (non_regen, regen) {
        (Ok(non_regen), Ok(regen)) => Ok(non_regen + regen),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(non_regen), Err(regen)) => anyhow::bail!(
            "non-regen pending tasks failed: {non_regen:#}; regen pending tasks failed: {regen:#}"
        ),
    }
}

pub async fn drain_non_regen(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    drain_where(
        mgr,
        "AND action IN ['transform', 'delete_cleanup', 'cascade_expand']",
    )
    .await
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
            warnings: Vec::new(),
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
            warnings: Vec::new(),
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
            warnings: Vec::new(),
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
            warnings: Vec::new(),
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
            warnings: Vec::new(),
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
