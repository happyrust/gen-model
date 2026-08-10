//! Fast, Ref0-range based removal of one DBNUM's persisted data.

use std::time::Instant;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;

use aios_core::SUL_DB;

const RANGE_END: &str = "9999999999";
const RANGE_TABLES: &[&str] = &[
    "pe",
    "inst_relate",
    "tubi_relate",
    "room_relate",
    "ref_rev",
    "geo_relate",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FastDeleteDbnumResult {
    pub dbnum: u32,
    pub ref0s: Vec<String>,
    pub pe_rows: usize,
    pub noun_tables: usize,
    pub range_statements: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Ref0Row {
    prefix: String,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct NounRow {
    noun: String,
}

#[derive(Debug, Deserialize)]
struct CountRow {
    count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneAboveWatermarkPreview {
    pub dbnum: u32,
    pub target_watermark: i32,
    pub applied_sesno: i32,
    pub pe_rows: usize,
    pub sample_refnos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneAboveWatermarkResult {
    #[serde(flatten)]
    pub preview: PruneAboveWatermarkPreview,
    pub deleted_pe_rows: usize,
    pub rebuilt_info_rows: usize,
    pub remaining_rows_above_watermark: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct WatermarkRow {
    applied_sesno: Option<i32>,
    file_name: Option<String>,
    db_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PrunePeRow {
    id: Thing,
    noun: String,
}

fn valid_table_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn range_of(table: &str, ref0: &str) -> String {
    format!("{table}:{ref0}_0..{ref0}_{RANGE_END}")
}

fn pe_key(id: &Thing) -> anyhow::Result<String> {
    let rendered = id.to_string();
    let key = rendered
        .strip_prefix("pe:")
        .filter(|key| {
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'_')
        })
        .with_context(|| format!("unexpected PE record id: {rendered}"))?;
    Ok(key.to_owned())
}

async fn inspect_prune_rows(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<(WatermarkRow, Vec<PrunePeRow>)> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    if target_watermark < 0 {
        bail!("target watermark must be non-negative");
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT applied_sesno, file_name, db_type FROM ONLY dbnum_watermark:{dbnum};\n\
             SELECT id, noun FROM pe WHERE dbnum = {dbnum} AND <int>sesno > {target_watermark} ORDER BY id;"
        ))
        .await
        .context("inspect rows above watermark")?
        .check()
        .context("inspect rows above watermark statement")?;
    let state = response
        .take::<Option<WatermarkRow>>(0)
        .context("decode dbnum watermark")?
        .with_context(|| format!("dbnum {dbnum} has no registered watermark"))?;
    let applied = state.applied_sesno.unwrap_or_default();
    if target_watermark > applied {
        bail!(
            "target watermark {target_watermark} is above applied_sesno {applied} for dbnum {dbnum}"
        );
    }
    let rows = response
        .take::<Vec<PrunePeRow>>(1)
        .context("decode rows above watermark")?;
    Ok((state, rows))
}

pub async fn preview_prune_above_watermark(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<PruneAboveWatermarkPreview> {
    let (state, rows) = inspect_prune_rows(dbnum, target_watermark).await?;
    let mut sample_refnos = rows
        .iter()
        .take(20)
        .map(|row| pe_key(&row.id))
        .collect::<anyhow::Result<Vec<_>>>()?;
    sample_refnos.sort_unstable();
    Ok(PruneAboveWatermarkPreview {
        dbnum,
        target_watermark,
        applied_sesno: state.applied_sesno.unwrap_or_default(),
        pe_rows: rows.len(),
        sample_refnos,
    })
}

/// Remove partial/latest-state rows whose per-element session is newer than
/// the selected authoritative watermark, then lower the durable watermark and
/// rebuild the legacy Ref0 statistics from the surviving PE snapshot.
///
/// This is a residue cleanup operation, not a historical-version restore: PE
/// stores one latest row per refno. The normal watcher replays the now-pending
/// sessions after this operation.
pub async fn prune_above_watermark(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<PruneAboveWatermarkResult> {
    let started = Instant::now();
    let _commit_guard = crate::data_interface::batch_worker::STAGED_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;
    let (state, rows) = inspect_prune_rows(dbnum, target_watermark).await?;
    let applied_sesno = state.applied_sesno.unwrap_or_default();
    let mut statements = Vec::with_capacity(rows.len() * 6 + 4);
    let mut sample_refnos = Vec::new();
    for row in &rows {
        if !valid_table_name(&row.noun) {
            bail!("invalid noun table name for dbnum {dbnum}: {}", row.noun);
        }
        let key = pe_key(&row.id)?;
        if sample_refnos.len() < 20 {
            sample_refnos.push(key.clone());
        }
        statements.push(format!(
            "DELETE array::flatten(SELECT VALUE ->pe_owner FROM pe:{key});"
        ));
        statements.push(format!(
            "DELETE array::flatten(SELECT VALUE <-pe_owner FROM pe:{key});"
        ));
        for table in RANGE_TABLES.iter().filter(|table| **table != "pe") {
            statements.push(format!("DELETE {table}:{key};"));
        }
        statements.push(format!("DELETE {}:{key};", row.noun));
        statements.push(format!("DELETE pe:{key};"));
    }
    statements.push(format!(
        "DELETE model_update_pending WHERE dbnum = {dbnum} AND <int>source_end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "DELETE increment_update_attempt WHERE dbnum = {dbnum} AND <int>end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "DELETE incr_side_effect_pending WHERE dbnum = {dbnum} AND <int>end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {target_watermark}, sesno = {target_watermark};"
    ));
    execute_phase("prune rows above watermark", &statements).await?;

    // A zero-row cleanup already proved the PE postcondition while both write
    // gates were held. Avoid two full PE scans in that common diagnostic case.
    let (rebuilt_info_rows, remaining_rows_above_watermark) = if rows.is_empty() {
        (0, 0)
    } else {
        let rebuilt = crate::versioned_db::database::rebuild_dbnum_info_from_pe(
            dbnum,
            state.file_name.as_deref().unwrap_or_default(),
            state.db_type.as_deref().unwrap_or_default(),
        )
        .await?;
        let remaining = preview_prune_above_watermark(dbnum, target_watermark)
            .await?
            .pe_rows;
        (rebuilt, remaining)
    };
    if remaining_rows_above_watermark != 0 {
        bail!(
            "dbnum {dbnum} prune incomplete: {remaining_rows_above_watermark} PE rows remain above watermark {target_watermark}"
        );
    }
    sample_refnos.sort_unstable();
    Ok(PruneAboveWatermarkResult {
        preview: PruneAboveWatermarkPreview {
            dbnum,
            target_watermark,
            applied_sesno,
            pe_rows: rows.len(),
            sample_refnos,
        },
        deleted_pe_rows: rows.len(),
        rebuilt_info_rows,
        remaining_rows_above_watermark,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Render separate checked phases. A giant optimistic transaction conflicts
/// with the watcher's periodic observation write on large DBNUMs. Metadata is
/// deliberately last, so a failed data phase never advertises an initialized
/// database as cleanly deleted.
fn render_delete_phases(
    dbnum: u32,
    ref0s: &[String],
    noun_tables: &[String],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut relations = Vec::new();
    let mut ranges = Vec::new();
    for ref0 in ref0s {
        let pe_range = range_of("pe", ref0);
        relations.push(format!(
            "DELETE array::flatten(SELECT VALUE ->pe_owner FROM {pe_range});"
        ));
        relations.push(format!(
            "DELETE array::flatten(SELECT VALUE <-pe_owner FROM {pe_range});"
        ));
        for table in RANGE_TABLES.iter().filter(|table| **table != "pe") {
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        for table in noun_tables {
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        ranges.push(format!("DELETE {pe_range};"));
    }
    let metadata = vec![
        format!("DELETE model_update_pending WHERE dbnum = {dbnum};"),
        format!("DELETE increment_update_attempt WHERE dbnum = {dbnum};"),
        format!("DELETE incr_side_effect_pending WHERE dbnum = {dbnum};"),
        format!("DELETE dbnum_info_table WHERE dbnum = {dbnum};"),
        format!("DELETE dbnum_watermark:{dbnum};"),
    ];
    (relations, ranges, metadata)
}

async fn execute_phase(label: &str, statements: &[String]) -> anyhow::Result<()> {
    if statements.is_empty() {
        return Ok(());
    }
    SUL_DB
        .query(statements.join("\n"))
        .await
        .with_context(|| format!("{label} transport failed"))?
        .check()
        .with_context(|| format!("{label} statement failed"))?;
    Ok(())
}

/// Delete all persisted rows owned by `dbnum` using Ref0 record-id ranges.
///
/// The HTTP caller stops dispatch first. These locks additionally serialize
/// against staged commit and scan-observation writes for internal callers.
pub async fn delete_dbnum_fast(dbnum: u32) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let started = Instant::now();
    let _commit_guard = crate::data_interface::batch_worker::STAGED_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;

    let mut response = SUL_DB
        .query(format!(
            "SELECT string::split(<string>id, '_')[0] AS prefix, count() AS count \
             FROM pe WHERE dbnum = {dbnum} GROUP BY prefix;\n\
             SELECT noun FROM pe WHERE dbnum = {dbnum} GROUP BY noun;"
        ))
        .await
        .context("inspect dbnum rows for fast delete")?
        .check()
        .context("inspect dbnum rows for fast delete statement")?;
    let prefix_rows = response
        .take::<Vec<Ref0Row>>(0)
        .context("decode Ref0 groups")?;
    let noun_rows = response
        .take::<Vec<NounRow>>(1)
        .context("decode noun groups")?;

    let pe_rows = prefix_rows.iter().map(|row| row.count).sum();
    let mut ref0s = prefix_rows
        .into_iter()
        .map(|row| {
            row.prefix
                .strip_prefix("pe:")
                .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_owned)
                .with_context(|| {
                    format!("unexpected PE id prefix for dbnum {dbnum}: {}", row.prefix)
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ref0s.sort_unstable();
    ref0s.dedup();

    let mut noun_tables = noun_rows
        .into_iter()
        .map(|row| row.noun)
        .collect::<Vec<_>>();
    noun_tables.sort_unstable();
    noun_tables.dedup();
    if let Some(invalid) = noun_tables.iter().find(|name| !valid_table_name(name)) {
        bail!("invalid noun table name for dbnum {dbnum}: {invalid}");
    }

    let (relations, ranges, metadata) = render_delete_phases(dbnum, &ref0s, &noun_tables);
    execute_phase("delete owner relations", &relations).await?;
    execute_phase("delete Ref0 ranges", &ranges).await?;
    execute_phase("delete dbnum metadata", &metadata).await?;

    let mut verify = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL;"
        ))
        .await
        .context("verify fast delete")?
        .check()
        .context("verify fast delete statement")?;
    let remaining = verify
        .take::<Vec<CountRow>>(0)
        .context("decode fast delete verification")?
        .first()
        .map(|row| row.count)
        .unwrap_or_default();
    if remaining != 0 {
        bail!("dbnum {dbnum} fast delete incomplete: {remaining} PE rows remain");
    }

    Ok(FastDeleteDbnumResult {
        dbnum,
        ref0s,
        pe_rows,
        noun_tables: noun_tables.len(),
        range_statements: ranges.len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ref0_ranges_and_keeps_metadata_last() {
        let (relations, ranges, metadata) =
            render_delete_phases(7997, &["24381".into()], &["EQUI".into(), "PANE".into()]);
        assert!(relations.iter().any(|sql| sql.contains("->pe_owner")));
        assert!(relations.iter().any(|sql| sql.contains("<-pe_owner")));
        assert!(ranges.contains(&"DELETE EQUI:24381_0..24381_9999999999;".into()));
        assert!(ranges.contains(&"DELETE inst_relate:24381_0..24381_9999999999;".into()));
        assert_eq!(
            ranges.last().unwrap(),
            "DELETE pe:24381_0..24381_9999999999;"
        );
        assert_eq!(metadata.last().unwrap(), "DELETE dbnum_watermark:7997;");
    }

    #[test]
    fn rejects_dynamic_table_name_injection() {
        assert!(valid_table_name("STWALL"));
        assert!(valid_table_name("TYPE_2"));
        assert!(!valid_table_name("PANE; DELETE pe"));
        assert!(!valid_table_name("lower"));
    }
}
