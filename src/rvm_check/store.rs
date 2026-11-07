//! 比对结果入库：`rvm_check_run`（一次比对一行）与 `rvm_check_mismatch`（一条差异一行）。
//!
//! 页面单根（[`super::execute`]）与批量命令（[`super::batch`]）跑完都落这两张表，之后「某个根历次比对
//! 怎么变的」「这一批哪些原因种类最多」「某个 noun 一直在差什么」都是一句 `SELECT`，不必再翻产物目录里的
//! `compare.json` / `summary.json`。表是 SCHEMALESS 的，字段随 [`RunRow`] / [`MismatchRow`] 走；
//! 首次用到时 `DEFINE … IF NOT EXISTS` 建表建索引（与 `dbnum_state::INCREMENT_STATE_SCHEMA` 同一纪律）。
//!
//! 一行差异带三样东西：`category`（比对器的分类：off / kind_differ / only_baseline …）、`reason_kind`
//! （归因后的原因种类，见 [`super::report::reason`]）、`reason`（带数值的一句话）。某一步没跑完的
//! （生成超时、E3D 没导出、比对 panic……）也落一行，`category = stage_failure`，`reason_kind` 由
//! [`classify_failure`] 从阶段与报错文本里认（`generate_timeout` / `e3d_export_no_file` / …）。
//!
//! 写法：`UPSERT` 运行行（同一 `run_id` 重跑覆盖）、按 `run_id` 删旧差异行再分批 `INSERT`——重跑或续跑
//! 不会留下两份。落库失败只记日志，产物目录里的 JSON 仍是完整的一份。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aios_core::SUL_DB;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::sync::OnceCell;

use super::report::{RvmCounts, RvmMismatch, reason};

pub const RUN_TABLE: &str = "rvm_check_run";
pub const MISMATCH_TABLE: &str = "rvm_check_mismatch";

/// 一次 `INSERT` 带多少条差异行：7,576 件 off 的 SITE 一条语句塞不下，也不该塞。
const INSERT_CHUNK: usize = 500;
/// 失败文本落库前截到这么长（与 `model_scope_regen` 的 `last_error` 同口径）。
const MESSAGE_LIMIT: usize = 2000;

pub const SCHEMA: &str = r#"
DEFINE TABLE IF NOT EXISTS rvm_check_run SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_refno ON rvm_check_run FIELDS refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_dbnum ON rvm_check_run FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_verdict ON rvm_check_run FIELDS verdict;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_batch ON rvm_check_run FIELDS batch_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_finished ON rvm_check_run FIELDS finished_unix_ms;
DEFINE TABLE IF NOT EXISTS rvm_check_mismatch SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_run ON rvm_check_mismatch FIELDS run_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_refno ON rvm_check_mismatch FIELDS refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_dbnum ON rvm_check_mismatch FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_batch ON rvm_check_mismatch FIELDS batch_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_kind ON rvm_check_mismatch FIELDS reason_kind;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_category ON rvm_check_mismatch FIELDS category;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_noun ON rvm_check_mismatch FIELDS noun;
"#;

static SCHEMA_READY: OnceCell<()> = OnceCell::const_new();

/// 建表建索引（幂等）。
pub async fn ensure_schema_on(db: &Surreal<Any>) -> Result<()> {
    db.query(SCHEMA)
        .await
        .context("rvm_check 结果表 DDL")?
        .check()
        .context("rvm_check 结果表 DDL 语句")?;
    Ok(())
}

/// 进程内只真跑一次；失败不记，下次再试。
pub async fn ensure_schema() -> Result<()> {
    SCHEMA_READY
        .get_or_try_init(|| ensure_schema_on(&SUL_DB))
        .await?;
    Ok(())
}

/// 各阶段耗时（秒）。页面走毫秒计时，批量走秒计时，入库统一成秒。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StageSeconds {
    #[serde(default)]
    pub resolve: f64,
    #[serde(default)]
    pub generate: f64,
    #[serde(default)]
    pub e3d_export: f64,
    #[serde(default)]
    pub compare: f64,
}

/// `rvm_check_run` 的一行。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RunRow {
    /// 页面：任务号（`rvm-check-…`）；批量：`<batch_id>/<tag>/<w0_w1>`。
    pub run_id: String,
    /// `page` / `batch`。
    pub source: String,
    pub batch_id: Option<String>,
    /// 服务身份里的项目名（`DbOption.project_name`）。
    pub project: String,
    pub e3d_project: String,
    pub e3d_mdb: String,
    /// `a/b`。
    pub refno: String,
    pub dbnum: Option<u32>,
    pub db_type: Option<String>,
    pub sesno: Option<u32>,
    pub noun: Option<String>,
    pub name: Option<String>,
    /// `pass` / `kind_differ` / `fail`。
    pub verdict: String,
    /// 更细的终态：页面与 verdict 相同；批量是 `empty` / `no-baseline` / `gen-failed` / `commit-guard` …。
    pub status: String,
    /// 停在或走完的阶段。
    pub stage: String,
    pub message: String,
    pub counts: RvmCounts,
    /// 差异行数（不含只记账的 `info` 行）。
    pub mismatch_total: usize,
    pub mismatch_by_kind: BTreeMap<String, usize>,
    pub mismatch_by_category: BTreeMap<String, usize>,
    pub centreline_only_baseline: usize,
    pub e3d_export_by: Option<String>,
    pub seconds: StageSeconds,
    pub gen_peak_private_mb: Option<u64>,
    pub artifact_dir: Option<String>,
    pub files: Vec<String>,
    /// 生成侧 `report.json` 的账本计数（跳过 / 待证据 / 只画中心线 / 不认识的 noun …），
    /// 解释「为什么生成侧没有这一件」。读不到就是 `null`。
    pub gen_ledger: Option<Value>,
    pub started_at: Option<String>,
    pub finished_at: String,
    pub finished_unix_ms: i64,
}

/// `rvm_check_mismatch` 的一行。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MismatchRow {
    pub run_id: String,
    pub source: String,
    pub batch_id: Option<String>,
    pub project: String,
    pub refno: String,
    pub dbnum: Option<u32>,
    pub seq: usize,
    /// 比对器的分类，或 `stage_failure` / `centreline_only`。
    pub category: String,
    /// 原因种类（[`reason`] 的常量，或 [`classify_failure`] 的取值）。
    pub reason_kind: String,
    pub reason: String,
    /// `error` / `warning` / `info`。
    pub severity: String,
    pub name: String,
    pub noun: String,
    pub max_axis_delta_mm: Option<f64>,
    pub baseline_kinds: Vec<String>,
    pub generated_kinds: Vec<String>,
    pub baseline_aabb: Option<[f64; 6]>,
    pub generated_aabb: Option<[f64; 6]>,
    pub finished_at: String,
    pub finished_unix_ms: i64,
}

/// 一次比对要入库的全部事实；页面与批量各自摊成这一份，再由 [`build_rows`] 变成两张表的行。
#[derive(Clone, Debug)]
pub struct RunFacts {
    pub run_id: String,
    pub source: &'static str,
    pub batch_id: Option<String>,
    pub project: String,
    pub e3d_project: String,
    pub e3d_mdb: String,
    pub refno: String,
    pub dbnum: Option<u32>,
    pub db_type: Option<String>,
    pub sesno: Option<u32>,
    pub noun: Option<String>,
    pub name: Option<String>,
    pub verdict: String,
    pub status: String,
    pub stage: String,
    pub message: String,
    pub counts: RvmCounts,
    /// **不截断**的差异清单。
    pub mismatches: Vec<RvmMismatch>,
    pub centreline_only_baseline: Vec<String>,
    pub e3d_export_by: Option<String>,
    pub seconds: StageSeconds,
    pub gen_peak_private_mb: Option<u64>,
    pub artifact_dir: Option<PathBuf>,
    pub files: Vec<String>,
    pub started_at: Option<String>,
    pub finished_at: chrono::DateTime<chrono::Local>,
}

pub const SOURCE_PAGE: &str = "page";
pub const SOURCE_BATCH: &str = "batch";
pub const CATEGORY_STAGE_FAILURE: &str = "stage_failure";

/// 某一步没跑完时的原因种类：从阶段名与报错文本里认。
pub fn classify_failure(stage: &str, message: &str) -> &'static str {
    let text = message.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    match stage {
        "resolve" => {
            if has(&["不在当前 mdb", "无法解析"]) {
                "resolve_refno_invalid"
            } else if has(&["模型树"]) {
                "tree_not_ready"
            } else {
                "resolve_failed"
            }
        }
        "generate_ours" | "generate" | "gen-failed" => {
            if has(&["超时", "timeout"]) {
                "generate_timeout"
            } else if has(&["panic"]) {
                "generate_panic"
            } else if has(&["账没平"]) {
                "generate_unbalanced"
            } else if has(&["没有写出"]) {
                "generate_no_rvm"
            } else if has(&["attlib", "schema"]) {
                "generate_schema_missing"
            } else if has(&["可提交内存", "commit"]) {
                "generate_commit_guard"
            } else {
                "generate_failed"
            }
        }
        "commit-guard" => "generate_commit_guard",
        "timeout" => "generate_timeout",
        "no-baseline" => "baseline_missing",
        "e3d_export" | "export" => {
            if has(&["启动脚本"]) {
                "e3d_launcher_missing"
            } else if has(&["没有启动"]) {
                "e3d_not_started"
            } else if has(&["没有落出文件", "没落出"]) {
                "e3d_export_no_file"
            } else if has(&["timeout=true", "超时"]) {
                "e3d_export_timeout"
            } else {
                "e3d_export_failed"
            }
        }
        "compare" | "compare-failed" => {
            if has(&["panic"]) {
                "compare_panic"
            } else if has(&["bbox 帧认不出来"]) {
                "compare_unknown_bbox_frame"
            } else if has(&["主键重复"]) {
                "compare_duplicate_key"
            } else if has(&["e3d 名称重复"]) {
                "compare_duplicate_name"
            } else {
                "compare_failed"
            }
        }
        _ => "run_failed",
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(limit).collect()
}

/// 生成侧 `report.json` 的账本计数：只取能说明「为什么没生成 / 生成成什么样」的那几格。
pub fn gen_ledger_summary(report_json: &Path) -> Option<Value> {
    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    let size = std::fs::metadata(report_json).ok()?.len();
    if size == 0 || size > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(report_json).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    fn size_of(value: Option<&Value>) -> u64 {
        match value {
            None | Some(Value::Null) => 0,
            Some(Value::Number(number)) => number.as_u64().unwrap_or(0),
            Some(Value::Array(items)) => items.len() as u64,
            Some(Value::Object(map)) => map.values().map(|v| size_of(Some(v))).sum(),
            Some(_) => 0,
        }
    }
    fn by_key(value: Option<&Value>) -> Value {
        match value {
            Some(Value::Object(map)) => Value::Object(
                map.iter()
                    .map(|(key, v)| (key.clone(), Value::from(size_of(Some(v)))))
                    .filter(|(_, v)| v.as_u64() != Some(0))
                    .collect(),
            ),
            other => Value::from(size_of(other)),
        }
    }
    let mut out = serde_json::Map::new();
    for key in ["visited", "generated"] {
        out.insert(key.to_string(), Value::from(size_of(value.get(key))));
    }
    for key in [
        "skipped",
        "failed",
        "catalogue_chain_broken",
        "negatives_skipped",
        "joint_cuts_explained",
    ] {
        out.insert(key.to_string(), Value::from(size_of(value.get(key))));
    }
    for key in [
        "evidence_pending",
        "centreline_only",
        "proven_non_graphic",
        "unknown_nouns",
        "not_wired",
        "unsupported",
        "catalog_pending",
        "orphans",
    ] {
        out.insert(key.to_string(), by_key(value.get(key)));
    }
    Some(Value::Object(out))
}

/// 把一次比对摊成两张表的行。
pub fn build_rows(facts: &RunFacts) -> (RunRow, Vec<MismatchRow>) {
    let finished_at = facts.finished_at.to_rfc3339();
    let finished_unix_ms = facts.finished_at.timestamp_millis();
    let mut rows: Vec<MismatchRow> = Vec::with_capacity(facts.mismatches.len() + 1);
    let base = |seq: usize| MismatchRow {
        run_id: facts.run_id.clone(),
        source: facts.source.to_string(),
        batch_id: facts.batch_id.clone(),
        project: facts.project.clone(),
        refno: facts.refno.clone(),
        dbnum: facts.dbnum,
        seq,
        category: String::new(),
        reason_kind: String::new(),
        reason: String::new(),
        severity: reason::ERROR.to_string(),
        name: String::new(),
        noun: String::new(),
        max_axis_delta_mm: None,
        baseline_kinds: Vec::new(),
        generated_kinds: Vec::new(),
        baseline_aabb: None,
        generated_aabb: None,
        finished_at: finished_at.clone(),
        finished_unix_ms,
    };

    // 某一步没跑完：一行 stage_failure。比对跑完但判红的不算（它的差异在清单里）。
    if facts.verdict != super::report::VERDICT_PASS
        && facts.verdict != super::report::VERDICT_KIND
        && facts.mismatches.is_empty()
        && facts.stage != "compare"
        && !facts.message.is_empty()
    {
        let mut row = base(0);
        row.category = CATEGORY_STAGE_FAILURE.to_string();
        row.reason_kind = classify_failure(&facts.stage, &facts.message).to_string();
        row.reason = truncate_chars(&facts.message, MESSAGE_LIMIT);
        row.name = facts.refno.clone();
        row.noun = facts.noun.clone().unwrap_or_default();
        rows.push(row);
    } else if facts.verdict == super::report::VERDICT_FAIL
        && facts.mismatches.is_empty()
        && facts.counts.both == 0
        && !facts.message.is_empty()
    {
        // 比对跑完却一件都没配上：主键口径对不上，也要留一行能查到的原因。
        let mut row = base(0);
        row.category = CATEGORY_STAGE_FAILURE.to_string();
        row.reason_kind = if facts.stage == "compare" {
            "compare_nothing_paired"
        } else {
            classify_failure(&facts.stage, &facts.message)
        }
        .to_string();
        row.reason = truncate_chars(&facts.message, MESSAGE_LIMIT);
        row.name = facts.refno.clone();
        rows.push(row);
    }
    for mismatch in &facts.mismatches {
        let mut row = base(rows.len());
        row.category = mismatch.category.clone();
        row.reason_kind = if mismatch.reason_kind.is_empty() {
            mismatch.category.clone()
        } else {
            mismatch.reason_kind.clone()
        };
        row.reason = truncate_chars(&mismatch.reason, MESSAGE_LIMIT);
        row.severity = if mismatch.severity.is_empty() {
            reason::ERROR.to_string()
        } else {
            mismatch.severity.clone()
        };
        row.name = mismatch.name.clone();
        row.noun = mismatch.noun.clone();
        row.max_axis_delta_mm = mismatch.max_axis_delta_mm;
        row.baseline_kinds = mismatch.baseline_kinds.clone();
        row.generated_kinds = mismatch.generated_kinds.clone();
        row.baseline_aabb = mismatch.baseline_aabb;
        row.generated_aabb = mismatch.generated_aabb;
        rows.push(row);
    }
    let mismatch_total = rows.len();
    for name in &facts.centreline_only_baseline {
        let mut row = base(rows.len());
        row.category = reason::CENTRELINE_ONLY.to_string();
        row.reason_kind = reason::CENTRELINE_ONLY.to_string();
        row.reason = "E3D 只画了中心线、没画实体，不进判据（我们不画线）".to_string();
        row.severity = reason::INFO.to_string();
        row.name = name.clone();
        row.noun = name
            .trim()
            .split_whitespace()
            .next()
            .filter(|head| !head.starts_with('/'))
            .map(|head| head.chars().take(4).collect())
            .unwrap_or_default();
        rows.push(row);
    }

    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows.iter().take(mismatch_total) {
        *by_kind.entry(row.reason_kind.clone()).or_default() += 1;
        *by_category.entry(row.category.clone()).or_default() += 1;
    }
    let gen_ledger = facts
        .artifact_dir
        .as_deref()
        .map(|dir| dir.join("ours").join("report.json"))
        .and_then(|path| gen_ledger_summary(&path));
    let run = RunRow {
        run_id: facts.run_id.clone(),
        source: facts.source.to_string(),
        batch_id: facts.batch_id.clone(),
        project: facts.project.clone(),
        e3d_project: facts.e3d_project.clone(),
        e3d_mdb: facts.e3d_mdb.clone(),
        refno: facts.refno.clone(),
        dbnum: facts.dbnum,
        db_type: facts.db_type.clone(),
        sesno: facts.sesno,
        noun: facts.noun.clone(),
        name: facts.name.clone(),
        verdict: facts.verdict.clone(),
        status: facts.status.clone(),
        stage: facts.stage.clone(),
        message: truncate_chars(&facts.message, MESSAGE_LIMIT),
        counts: facts.counts.clone(),
        mismatch_total,
        mismatch_by_kind: by_kind,
        mismatch_by_category: by_category,
        centreline_only_baseline: facts.centreline_only_baseline.len(),
        e3d_export_by: facts.e3d_export_by.clone(),
        seconds: facts.seconds.clone(),
        gen_peak_private_mb: facts.gen_peak_private_mb,
        artifact_dir: facts
            .artifact_dir
            .as_ref()
            .map(|dir| dir.display().to_string()),
        files: facts.files.clone(),
        gen_ledger,
        started_at: facts.started_at.clone(),
        finished_at,
        finished_unix_ms,
    };
    (run, rows)
}

/// 落一次比对：运行行 `UPSERT`，差异行先按 `run_id` 删旧再分批插。
pub async fn persist_on(db: &Surreal<Any>, run: &RunRow, rows: &[MismatchRow]) -> Result<()> {
    db.query(format!(
        "UPSERT type::thing('{RUN_TABLE}', $run_id) CONTENT $run; \
         DELETE {MISMATCH_TABLE} WHERE run_id = $run_id;"
    ))
    .bind(("run_id", run.run_id.clone()))
    .bind(("run", run.clone()))
    .await
    .with_context(|| format!("写 {RUN_TABLE} {}", run.run_id))?
    .check()
    .with_context(|| format!("写 {RUN_TABLE} {} 的语句", run.run_id))?;
    for chunk in rows.chunks(INSERT_CHUNK) {
        db.query(format!("INSERT INTO {MISMATCH_TABLE} $rows;"))
            .bind(("rows", chunk.to_vec()))
            .await
            .with_context(|| format!("写 {MISMATCH_TABLE} {}", run.run_id))?
            .check()
            .with_context(|| format!("写 {MISMATCH_TABLE} {} 的语句", run.run_id))?;
    }
    Ok(())
}

/// [`persist_on`] 的 `SUL_DB` 版：先确保表在。
pub async fn persist(run: &RunRow, rows: &[MismatchRow]) -> Result<()> {
    ensure_schema().await?;
    persist_on(&SUL_DB, run, rows).await
}

/// 摊行 + 落库一步到位；失败只回 Err，调用方决定记日志还是忽略。
pub async fn persist_facts(facts: &RunFacts) -> Result<(usize, usize)> {
    let (run, rows) = build_rows(facts);
    persist(&run, &rows).await?;
    Ok((run.mismatch_total, rows.len()))
}

/// 批量命令（同步代码、不在 tokio 里）用：自己起一个当前线程 runtime，连上 `DbOption.toml` 指的那台库，
/// 逐根落库。返回 (根数, 行数)。`store_mode` 是 mem 时没有可写的持久库，直接回 Err 让调用方说明。
#[cfg(feature = "ws")]
pub fn persist_batch_blocking(facts: &[RunFacts]) -> Result<(usize, usize)> {
    use std::time::Duration;
    anyhow::ensure!(
        crate::options::store_mode().is_durable(),
        "store_mode 是 mem（进程内嵌库），没有可写的持久库；以 AIOS_STORE_MODE=rocksdb 指到真在跑的 SurrealDB 再跑"
    );
    if facts.is_empty() {
        return Ok((0, 0));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("起入库用的 runtime")?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(60), crate::attach_store_connected())
            .await
            .context("连 SurrealDB 超时（60 s）")?
            .context("连 SurrealDB")?;
        ensure_schema().await?;
        let mut rows_total = 0usize;
        for fact in facts {
            let (run, rows) = build_rows(fact);
            persist_on(&SUL_DB, &run, &rows).await?;
            rows_total += rows.len();
        }
        Ok((facts.len(), rows_total))
    })
}

#[cfg(not(feature = "ws"))]
pub fn persist_batch_blocking(_facts: &[RunFacts]) -> Result<(usize, usize)> {
    anyhow::bail!("这个构建没有 ws 特性，连不上 SurrealDB")
}

/// `rvm_check_run` 的查询条件；没给的不限。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RunFilter {
    pub refno: Option<String>,
    pub dbnum: Option<u32>,
    pub verdict: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub batch_id: Option<String>,
    pub project: Option<String>,
    /// 只看 `finished_unix_ms` 早于它的（往回翻页）。
    pub before: Option<i64>,
    pub limit: Option<usize>,
}

/// `rvm_check_mismatch` 的查询条件；没给的不限。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MismatchFilter {
    pub run_id: Option<String>,
    pub refno: Option<String>,
    pub dbnum: Option<u32>,
    pub batch_id: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub reason_kind: Option<String>,
    pub noun: Option<String>,
    pub severity: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

const MAX_LIMIT: usize = 5000;

fn clamp_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, MAX_LIMIT)
}

/// `WHERE a = $a AND b = $b` 这一段；返回子句与要绑的参数。
fn where_clause(
    terms: Vec<(&'static str, &'static str, Option<Value>)>,
) -> (String, Vec<(&'static str, Value)>) {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    for (column, operator, value) in terms {
        if let Some(value) = value {
            parts.push(format!("{column} {operator} ${column}"));
            binds.push((column, value));
        }
    }
    let clause = if parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", parts.join(" AND "))
    };
    (clause, binds)
}

fn normalize_refno(raw: &str) -> String {
    raw.trim().trim_start_matches('=').replace('_', "/")
}

/// 最近的比对运行，`finished_unix_ms` 倒序。
pub async fn list_runs_on(db: &Surreal<Any>, filter: &RunFilter) -> Result<Vec<Value>> {
    let (clause, binds) = where_clause(vec![
        (
            "refno",
            "=",
            filter
                .refno
                .as_deref()
                .map(normalize_refno)
                .map(Value::from),
        ),
        ("dbnum", "=", filter.dbnum.map(Value::from)),
        ("verdict", "=", filter.verdict.clone().map(Value::from)),
        ("status", "=", filter.status.clone().map(Value::from)),
        ("source", "=", filter.source.clone().map(Value::from)),
        ("batch_id", "=", filter.batch_id.clone().map(Value::from)),
        ("project", "=", filter.project.clone().map(Value::from)),
        ("finished_unix_ms", "<", filter.before.map(Value::from)),
    ]);
    let limit = clamp_limit(filter.limit, 50);
    let mut query = db.query(format!(
        "SELECT * OMIT id FROM {RUN_TABLE}{clause} ORDER BY finished_unix_ms DESC LIMIT {limit};"
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("查 rvm_check_run")?.check()?;
    response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_run 行")
}

/// 差异行，按 `run_id`、`seq` 排。
pub async fn list_mismatches_on(db: &Surreal<Any>, filter: &MismatchFilter) -> Result<Vec<Value>> {
    let (clause, binds) = mismatch_where(filter);
    let limit = clamp_limit(filter.limit, 500);
    let offset = filter.offset.unwrap_or(0);
    let mut query = db.query(format!(
        "SELECT * OMIT id FROM {MISMATCH_TABLE}{clause} ORDER BY finished_unix_ms DESC, seq ASC LIMIT {limit} START {offset};"
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("查 rvm_check_mismatch")?.check()?;
    response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_mismatch 行")
}

/// 按原因种类汇总：`[{reason_kind, category, severity, n}]`，多的在前。
pub async fn reason_summary_on(db: &Surreal<Any>, filter: &MismatchFilter) -> Result<Vec<Value>> {
    let (clause, binds) = mismatch_where(filter);
    let mut query = db.query(format!(
        "SELECT reason_kind, category, severity, count() AS n FROM {MISMATCH_TABLE}{clause} \
         GROUP BY reason_kind, category, severity;"
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("汇总 rvm_check_mismatch")?.check()?;
    let mut rows = response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_mismatch 汇总")?;
    rows.sort_by(|a, b| {
        b.get("n")
            .and_then(Value::as_u64)
            .cmp(&a.get("n").and_then(Value::as_u64))
    });
    Ok(rows)
}

fn mismatch_where(filter: &MismatchFilter) -> (String, Vec<(&'static str, Value)>) {
    where_clause(vec![
        ("run_id", "=", filter.run_id.clone().map(Value::from)),
        (
            "refno",
            "=",
            filter
                .refno
                .as_deref()
                .map(normalize_refno)
                .map(Value::from),
        ),
        ("dbnum", "=", filter.dbnum.map(Value::from)),
        ("batch_id", "=", filter.batch_id.clone().map(Value::from)),
        ("source", "=", filter.source.clone().map(Value::from)),
        ("category", "=", filter.category.clone().map(Value::from)),
        (
            "reason_kind",
            "=",
            filter.reason_kind.clone().map(Value::from),
        ),
        ("noun", "=", filter.noun.clone().map(Value::from)),
        ("severity", "=", filter.severity.clone().map(Value::from)),
    ])
}

pub async fn list_runs(filter: &RunFilter) -> Result<Vec<Value>> {
    ensure_schema().await?;
    list_runs_on(&SUL_DB, filter).await
}

pub async fn list_mismatches(filter: &MismatchFilter) -> Result<Vec<Value>> {
    ensure_schema().await?;
    list_mismatches_on(&SUL_DB, filter).await
}

pub async fn reason_summary(filter: &MismatchFilter) -> Result<Vec<Value>> {
    ensure_schema().await?;
    reason_summary_on(&SUL_DB, filter).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rvm_check::report::VERDICT_FAIL;

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("rvm_check_store").use_db(label).await.unwrap();
        ensure_schema_on(&db).await.unwrap();
        db
    }

    fn mismatch(category: &str, reason_kind: &str, name: &str, noun: &str) -> RvmMismatch {
        RvmMismatch {
            category: category.to_string(),
            name: name.to_string(),
            noun: noun.to_string(),
            max_axis_delta_mm: Some(12.5),
            baseline_kinds: vec!["Box".to_string()],
            generated_kinds: vec!["Box".to_string()],
            baseline_aabb: Some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            generated_aabb: Some([5.0, 0.0, 0.0, 6.0, 1.0, 1.0]),
            reason_kind: reason_kind.to_string(),
            reason: format!("{name} 偏了 'x' \\ y"),
            severity: if category == "kind_differ" {
                reason::WARNING
            } else {
                reason::ERROR
            }
            .to_string(),
        }
    }

    fn facts(run_id: &str, refno: &str, verdict: &str, mismatches: Vec<RvmMismatch>) -> RunFacts {
        RunFacts {
            run_id: run_id.to_string(),
            source: SOURCE_BATCH,
            batch_id: Some("b-1".to_string()),
            project: "AvevaMarineSample".to_string(),
            e3d_project: "AMS".to_string(),
            e3d_mdb: "/ALL".to_string(),
            refno: refno.to_string(),
            dbnum: Some(7999),
            db_type: Some("DESI".to_string()),
            sesno: Some(12),
            noun: Some("SITE".to_string()),
            name: Some("/SITE-A".to_string()),
            verdict: verdict.to_string(),
            status: verdict.to_string(),
            stage: "compare".to_string(),
            message: "msg".to_string(),
            counts: RvmCounts {
                both: 10,
                exact: 8,
                off: mismatches.len() as u64,
                ..RvmCounts::default()
            },
            mismatches,
            centreline_only_baseline: vec!["SNOD 1 of SCTN /S".to_string()],
            e3d_export_by: Some("refno".to_string()),
            seconds: StageSeconds {
                generate: 2.5,
                compare: 0.1,
                ..StageSeconds::default()
            },
            gen_peak_private_mb: Some(900),
            artifact_dir: None,
            files: vec!["compare.json".to_string()],
            started_at: None,
            finished_at: chrono::Local::now(),
        }
    }

    #[test]
    fn rows_carry_reason_kinds_and_a_stage_failure_gets_its_own_row() {
        let run = facts(
            "b-1/ams7999/1_2",
            "1/2",
            VERDICT_FAIL,
            vec![
                mismatch("off", reason::AABB_SHIFTED, "SJOI 1 /S", "SJOI"),
                mismatch("off", reason::AABB_SHIFTED, "SJOI 2 /S", "SJOI"),
                mismatch("kind_differ", reason::FACET_VS_ANALYTIC, "/NUT", "CONE"),
            ],
        );
        let (row, rows) = build_rows(&run);
        assert_eq!(row.mismatch_total, 3);
        assert_eq!(row.centreline_only_baseline, 1);
        assert_eq!(rows.len(), 4, "三条差异 + 一条中心线记账");
        assert_eq!(row.mismatch_by_kind[reason::AABB_SHIFTED], 2);
        assert_eq!(row.mismatch_by_kind[reason::FACET_VS_ANALYTIC], 1);
        assert_eq!(row.mismatch_by_category["off"], 2);
        assert_eq!(rows[3].severity, reason::INFO);
        assert_eq!(rows[3].reason_kind, reason::CENTRELINE_ONLY);
        assert_eq!(rows[3].noun, "SNOD");
        assert!(rows.iter().enumerate().all(|(i, r)| r.seq == i));

        let mut failed = facts("page-1", "1/3", VERDICT_FAIL, Vec::new());
        failed.stage = "generate_ours".to_string();
        failed.message = "生成超时（1200 秒）：…".to_string();
        failed.centreline_only_baseline.clear();
        let (row, rows) = build_rows(&failed);
        assert_eq!(row.mismatch_total, 1);
        assert_eq!(rows[0].category, CATEGORY_STAGE_FAILURE);
        assert_eq!(rows[0].reason_kind, "generate_timeout");
        assert_eq!(row.mismatch_by_kind["generate_timeout"], 1);
    }

    #[test]
    fn failure_kinds_follow_stage_and_message() {
        assert_eq!(
            classify_failure("generate_ours", "生成时 panic：x"),
            "generate_panic"
        );
        assert_eq!(
            classify_failure("generate_ours", "生成的账没平：…"),
            "generate_unbalanced"
        );
        assert_eq!(
            classify_failure("gen-failed", "生成子进程退出码 1"),
            "generate_failed"
        );
        assert_eq!(
            classify_failure("commit-guard", "可提交内存余量低于 1.5 GB"),
            "generate_commit_guard"
        );
        assert_eq!(
            classify_failure("e3d_export", "按 refno 导出没有落出文件（exit=…）"),
            "e3d_export_no_file"
        );
        assert_eq!(
            classify_failure("e3d_export", "找不到 E3D 启动脚本"),
            "e3d_launcher_missing"
        );
        assert_eq!(
            classify_failure("compare", "比对时 panic：x"),
            "compare_panic"
        );
        assert_eq!(
            classify_failure("compare", "x 里有 3 条记录的 bbox 帧认不出来"),
            "compare_unknown_bbox_frame"
        );
        assert_eq!(
            classify_failure("no-baseline", "基准目录里没有 …"),
            "baseline_missing"
        );
        assert_eq!(
            classify_failure("resolve", "节点 1/2 不在当前 MDB"),
            "resolve_refno_invalid"
        );
    }

    /// 写两次同一 `run_id` 不留两份；按 refno / 原因种类 / 运行号都查得到；汇总按种类计数。
    #[tokio::test]
    async fn persisting_twice_is_idempotent_and_rows_are_queryable() {
        let db = database("roundtrip").await;
        let run = facts(
            "b-1/ams7999/1_2",
            "1/2",
            VERDICT_FAIL,
            vec![
                mismatch("off", reason::AABB_SHIFTED, "SJOI 1 /S", "SJOI"),
                mismatch("off", reason::AABB_AXES_SWAPPED, "PCLA 1 /P", "PCLA"),
                mismatch("kind_differ", reason::FACET_VS_ANALYTIC, "/NUT", "CONE"),
            ],
        );
        let (row, rows) = build_rows(&run);
        persist_on(&db, &row, &rows).await.unwrap();
        persist_on(&db, &row, &rows).await.unwrap();
        // 幂等也包括表定义：再跑一遍 DDL 不许清行。
        ensure_schema_on(&db).await.unwrap();

        let runs = list_runs_on(
            &db,
            &RunFilter {
                refno: Some("=1/2".to_string()),
                ..RunFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0]["run_id"], "b-1/ams7999/1_2");
        assert_eq!(runs[0]["verdict"], VERDICT_FAIL);
        assert_eq!(runs[0]["mismatch_total"], 3);
        assert_eq!(runs[0]["counts"]["both"], 10);
        assert_eq!(runs[0]["mismatch_by_kind"][reason::AABB_SHIFTED], 1);
        assert!(runs[0].get("id").is_none(), "不吐记录 id：{:?}", runs[0]);

        let all = list_mismatches_on(
            &db,
            &MismatchFilter {
                run_id: Some("b-1/ams7999/1_2".to_string()),
                ..MismatchFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 4, "三条差异 + 一条中心线，重写不翻倍：{all:?}");
        assert_eq!(all[0]["seq"], 0);
        assert_eq!(all[0]["reason_kind"], reason::AABB_SHIFTED);
        assert_eq!(all[0]["reason"], "SJOI 1 /S 偏了 'x' \\ y");
        assert_eq!(all[0]["baseline_aabb"][3], 1.0);

        let swapped = list_mismatches_on(
            &db,
            &MismatchFilter {
                dbnum: Some(7999),
                reason_kind: Some(reason::AABB_AXES_SWAPPED.to_string()),
                ..MismatchFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(swapped.len(), 1);
        assert_eq!(swapped[0]["name"], "PCLA 1 /P");

        let errors_only = list_mismatches_on(
            &db,
            &MismatchFilter {
                severity: Some(reason::ERROR.to_string()),
                ..MismatchFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(errors_only.len(), 2);

        let summary = reason_summary_on(
            &db,
            &MismatchFilter {
                batch_id: Some("b-1".to_string()),
                ..MismatchFilter::default()
            },
        )
        .await
        .unwrap();
        let kinds: BTreeMap<String, u64> = summary
            .iter()
            .map(|row| {
                (
                    row["reason_kind"].as_str().unwrap().to_string(),
                    row["n"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(kinds[reason::AABB_SHIFTED], 1);
        assert_eq!(kinds[reason::AABB_AXES_SWAPPED], 1);
        assert_eq!(kinds[reason::FACET_VS_ANALYTIC], 1);
        assert_eq!(kinds[reason::CENTRELINE_ONLY], 1);

        // 第二次运行同一个根：历史按时间倒序，最新在前，两条都在。
        let mut again = facts("b-2/ams7999/1_2", "1/2", "pass", Vec::new());
        again.batch_id = Some("b-2".to_string());
        again.finished_at = run.finished_at + chrono::Duration::seconds(5);
        let (row2, rows2) = build_rows(&again);
        persist_on(&db, &row2, &rows2).await.unwrap();
        let history = list_runs_on(
            &db,
            &RunFilter {
                dbnum: Some(7999),
                ..RunFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["run_id"], "b-2/ams7999/1_2");
        assert_eq!(history[1]["run_id"], "b-1/ams7999/1_2");
        let only_pass = list_runs_on(
            &db,
            &RunFilter {
                verdict: Some("pass".to_string()),
                ..RunFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(only_pass.len(), 1);
    }

    #[test]
    fn gen_ledger_summary_counts_lists_and_maps() {
        let dir = std::env::temp_dir().join(format!("rvm-check-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "visited": 100, "generated": 80,
                "skipped": [{"refno": "1/2"}, {"refno": "1/3"}],
                "failed": [],
                "evidence_pending": {"ATTA": 5, "SCTN": [1, 2]},
                "centreline_only": {"SNOD": 7},
                "unknown_nouns": {},
                "not_wired": {"PNOD": 3}
            })
            .to_string(),
        )
        .unwrap();
        let ledger = gen_ledger_summary(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(ledger["visited"], 100);
        assert_eq!(ledger["skipped"], 2);
        assert_eq!(ledger["failed"], 0);
        assert_eq!(ledger["evidence_pending"]["ATTA"], 5);
        assert_eq!(ledger["evidence_pending"]["SCTN"], 2);
        assert_eq!(ledger["centreline_only"]["SNOD"], 7);
        assert_eq!(ledger["not_wired"]["PNOD"], 3);
        assert!(ledger["unknown_nouns"].as_object().unwrap().is_empty());
    }
}
