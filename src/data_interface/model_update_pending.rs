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

fn render_finalize_transaction(dbnum: u32, end_sesno: i32, plan: &ModelUpdatePlan) -> String {
    let mut statements = plan
        .work_items
        .iter()
        .map(render_upsert)
        .collect::<Vec<_>>();
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
pub async fn finalize_attempt(
    dbnum: u32,
    end_sesno: i32,
    plan: &ModelUpdatePlan,
) -> anyhow::Result<()> {
    SUL_DB
        .query(render_finalize_transaction(dbnum, end_sesno, plan))
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

async fn delete_work(item: &PendingModelWork) -> anyhow::Result<()> {
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

/// Drain pending work independently. Failures remain durable and are retried on
/// a later watcher/manual invocation, even when there is no new session.
async fn drain_where(mgr: &AiosDBManager, action_filter: &str) -> anyhow::Result<usize> {
    let sql = format!(
        "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
         {action_filter} ORDER BY updated_at ASC LIMIT 50;"
    );
    let mut response =
        SUL_DB.query(sql).await?.check().map_err(|error| {
            anyhow::anyhow!("load pending model work statement failed: {error}")
        })?;
    let jobs: Vec<PendingModelWork> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode pending model work failed: {error}"))?;
    let mut done = 0;
    for job in jobs {
        match execute_item(mgr, &job).await {
            Ok(()) => {
                delete_work(&job).await?;
                done += 1;
            }
            Err(error) => {
                let _ = mark_failed(&job, &format!("{error:#}")).await;
            }
        }
    }
    Ok(done)
}

pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    drain_where(mgr, "").await
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

    #[test]
    fn finalization_is_one_transaction_with_work_watermark_and_attempt_cleanup() {
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

        let sql = render_finalize_transaction(8191, 42, &plan);
        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.contains("UPSERT model_update_pending:8191_regen_root_16777216_5"));
        assert!(sql.contains("applied_sesno = math::max([applied_sesno?:0, 42])"));
        assert!(sql.contains("DELETE increment_update_attempt:8191"));
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
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
}
