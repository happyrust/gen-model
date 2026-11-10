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
//!
//! 批量命令一行是一个比对单元：`unit` 记它是哪一种（`root` 是 WORLD 下的 SITE / APPLDW，`--unit zone`
//! 时是 `zone` / `fallback`），`root_refno` / `root_noun` / `root_name` 记它所在的上级根——单元行是它的
//! 上级 SITE / APPLDW，根行是它自己。所以「某个 SITE 下的全部 ZONE」是 `root_refno = …` 一个条件，
//! [`rollup_on`] 再把一批按上级根收成一行。差异行同样带 `root_refno`。
//!
//! 批量命令不等整批跑完：[`StoreWorker`] 在自己的线程里连一次库，一个单元跑完就落一个；续跑时上次
//! 没落进去的单元（中途断了、当时库连不上）按库里已有的 `run_id` 补上。
//!
//! ## 构件级结果：`rvm_check_element`
//!
//! 上面两张表到单元为止（差异表只有出问题的件，也没有构件自己的 refno）。第三张表一个带几何的构件一行
//! （[`ElementRow`]，由 [`super::element::ElementCheck`] 摊出来），**通过的也有**，记录 id 由 refno 得出：
//!
//! | 构件 | 记录 id 的键 |
//! |---|---|
//! | 有自己 refno 的 | `<w0>_<w1>`（`rvm_check_element:⟨24383_1234⟩`，与 `pe` 的键同一写法） |
//! | 隐式管身一类的派生件 | `<属主 w0>_<属主 w1>:TUBI:<管号>`；只在基准侧的那根是 `…:TUBI:E<E3D 管号>` |
//! | 并成一件的容器管身整组 / 组里 E3D 多画的记录 | `<属主>:TUBES` / `<属主>:TUBES:R<序号>` |
//! | refno 没解出来的 | `<单元 w0>_<单元 w1>~<判档>~<显示名>`，`refno_resolved = false` |
//!
//! 同一个构件再次被核对时覆盖旧行，只留**最后完成**的那一次（按 `finished_unix_ms`：补入库的老结果不会
//! 盖掉更新的行）；`run_id` / `batch_id` 指回 `rvm_check_run` 里那一次。一个单元重新核对后，它名下比这次
//! 更早、这次没再出现的行（构件删了、挪走了）随手清掉（[`StaleSweep`]）。某一步没跑完、没有比对结果的
//! 运行不写这张表、也不清——上一次的结果留着。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aios_core::SUL_DB;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::sync::OnceCell;

use super::element::{self, ElementCheck, grade};
use super::report::{RvmCounts, RvmMismatch, reason};

pub const RUN_TABLE: &str = "rvm_check_run";
pub const MISMATCH_TABLE: &str = "rvm_check_mismatch";
pub const ELEMENT_TABLE: &str = "rvm_check_element";

/// 一次 `INSERT` 带多少条差异行：7,576 件 off 的 SITE 一条语句塞不下，也不该塞。
const INSERT_CHUNK: usize = 500;
/// 一笔事务带多少条构件行。整根 8 万件分 80 笔；草稿库上实测首次写约 1.1 万行 / 秒、覆盖旧行约 0.4 万行 / 秒。
const ELEMENT_CHUNK: usize = 1000;
/// 排队等入库的构件行上限：超了就让交结果的一方等一等（[`StoreWorker::submit`]），父进程里攒着的行数有界
/// ——最多是这个数再加一个单元（单元不拆：整根 8 万件自己就超线，它进去之后下一个等它写完）。
/// 按 ZONE 边跑边写碰不到这条线（一个单元百来行，写得比生成快）；续跑补入库会碰到：跳过的单元是一口气
/// 从作业目录读出来交的，一千多个单元十几万行，不拦就全在内存里排着。
const PENDING_ELEMENT_ROWS_LIMIT: usize = 50_000;
/// 失败文本落库前截到这么长（与 `model_scope_regen` 的 `last_error` 同口径）。
const MESSAGE_LIMIT: usize = 2000;

pub const SCHEMA: &str = r#"
DEFINE TABLE IF NOT EXISTS rvm_check_run SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_refno ON rvm_check_run FIELDS refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_dbnum ON rvm_check_run FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_verdict ON rvm_check_run FIELDS verdict;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_batch ON rvm_check_run FIELDS batch_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_finished ON rvm_check_run FIELDS finished_unix_ms;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_run_root ON rvm_check_run FIELDS root_refno;
DEFINE TABLE IF NOT EXISTS rvm_check_mismatch SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_run ON rvm_check_mismatch FIELDS run_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_refno ON rvm_check_mismatch FIELDS refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_dbnum ON rvm_check_mismatch FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_batch ON rvm_check_mismatch FIELDS batch_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_kind ON rvm_check_mismatch FIELDS reason_kind;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_category ON rvm_check_mismatch FIELDS category;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_noun ON rvm_check_mismatch FIELDS noun;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_mismatch_root ON rvm_check_mismatch FIELDS root_refno;
DEFINE TABLE IF NOT EXISTS rvm_check_element SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_refno ON rvm_check_element FIELDS refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_unit ON rvm_check_element FIELDS unit_refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_root ON rvm_check_element FIELDS root_refno;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_run ON rvm_check_element FIELDS run_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_batch ON rvm_check_element FIELDS batch_id;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_dbnum ON rvm_check_element FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_noun ON rvm_check_element FIELDS noun;
DEFINE INDEX IF NOT EXISTS idx_rvm_check_element_grade ON rvm_check_element FIELDS grade;
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
    /// 比对单元的种类：`root`（WORLD 下的 SITE / APPLDW）/ `zone` / `fallback`。
    /// 页面单根没有单元的概念，不写。
    pub unit: Option<String>,
    /// 所在上级根的 refno（`a/b`）：单元行是它的上级 SITE / APPLDW，根行是它自己。
    /// 页面单根，以及加这一格之前写进来的行没有它，查询时把那一行的 `refno` 当上级根。
    pub root_refno: Option<String>,
    pub root_noun: Option<String>,
    pub root_name: Option<String>,
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
    /// 这条差异所在运行的上级根（同 [`RunRow::root_refno`]）。
    pub root_refno: Option<String>,
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

/// `rvm_check_element` 的一行：一个带几何的构件最近一次核对的结果。没有值的格不写（库里就是没有这一格）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ElementRow {
    /// 记录 id 的键（库里是 `rvm_check_element:⟨键⟩`）：由 refno 得出，见模块文档的表。查询应答里叫 `key`。
    pub id: String,
    /// 构件自己的 refno（`a/b`）；派生件是属主的。没解出来就没有这一格。
    pub refno: Option<String>,
    /// refno 解出来没有。假的那些行键里不含 refno，按 `unit_refno` / `name` 找。
    pub refno_resolved: bool,
    /// 派生件：没有自己的库记录（隐式管身一类），记在属主的 refno 名下。
    pub derived: bool,
    /// `implied_tube` / `hang_axis_tube` / `container_tubes` / `container_record`。
    pub derived_kind: Option<String>,
    /// 派生件在属主名下的序号（管号、容器组里的记录号；整组为 0）。
    pub derived_ordinal: Option<usize>,
    /// 隐式管身两端的构件 refno（上游 / 下游）。
    pub tube_from: Option<String>,
    pub tube_to: Option<String>,
    /// 同一次比对里按 refno 得出的键已经被另一件占了（基准里按名字解出的 refno 与某个生成件相同），
    /// 这一行改用带判档后缀的键。只有这种行才有这一格。
    pub key_shared: Option<bool>,
    /// 库号。
    pub dbnum: Option<u32>,
    pub noun: String,
    /// 构件在 RVM 里的显示名。
    pub name: String,
    /// 配对用的主键；配上的件才有。
    pub pair_key: Option<String>,
    /// 所在的核对单元（`a/b`）：按 ZONE 跑是那个 ZONE / 兜底单元，整根跑是 SITE / APPLDW。
    pub unit_refno: String,
    /// `zone` / `fallback` / `root`；页面单根落不进 WORLD 下任何一棵树时是 `node`。
    pub unit_kind: String,
    /// 所在的上级根（SITE / APPLDW）。
    pub root_refno: Option<String>,
    /// 这一行是只核对单元里一块时写的：`node`（只比那个节点自己）/ `subtree`，以及那个节点的 refno。
    pub scope_kind: Option<String>,
    pub scope_refno: Option<String>,
    /// 写下这一行的那次运行（`rvm_check_run.run_id`）与它的批次。
    pub run_id: String,
    pub batch_id: Option<String>,
    /// `page` / `batch`。
    pub source: String,
    /// 判档：exact / chord / reference_artifact / film / off / only_baseline / only_generated /
    /// unkeyable_* / centreline_only / container_*（[`super::element::grade`]）。
    pub grade: String,
    /// 这一档算不算通过（包围盒在容差档内；只画中心线的不进判据，也算）。
    pub passed: bool,
    /// 两侧图元种类是否同一套；没配上的件没有。
    pub kind_match: Option<bool>,
    /// `ok` / `info` / `warning` / `error`。
    pub severity: String,
    /// 逐轴 |生成 − 基准| 的最大值（mm）。
    pub max_axis_delta_mm: Option<f64>,
    /// `reference_artifact` / `film` 两档才有：滤掉断开薄片 / 贴体薄膜之后的差。
    pub significant_max_axis_delta_mm: Option<f64>,
    pub baseline_kinds: Vec<String>,
    pub generated_kinds: Vec<String>,
    /// 两侧的包围盒 `[xmin, ymin, zmin, xmax, ymax, zmax]`（mm），在 `aabb_frame` 那一帧里
    /// （`local` 元素局部系 / `world`）。
    pub baseline_aabb: Option<[f64; 6]>,
    pub generated_aabb: Option<[f64; 6]>,
    pub aabb_frame: Option<String>,
    /// 有问题时的原因种类与带数值的说明，与 `rvm_check_mismatch` 里这一件的那一行相同。
    pub reason_kind: Option<String>,
    pub reason: Option<String>,
    pub finished_at: String,
    pub finished_unix_ms: i64,
}

/// 这次只核对了单元里的一块（批量的 `--scope`，或页面上比的是单元之下的一个节点）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementScope {
    /// `node` / `subtree`。
    pub kind: String,
    /// 选中节点的 refno（`a/b`）。
    pub refno: String,
}

/// 一次比对的构件级结果，连同它们记在哪个单元名下。只有比对出了结果才有（[`RunFacts::elements`]）。
#[derive(Clone, Debug)]
pub struct ElementFacts {
    pub checks: Vec<ElementCheck>,
    /// 所在的核对单元（[`ElementRow::unit_refno`]）。
    pub unit_refno: String,
    pub unit_kind: String,
    pub root_refno: Option<String>,
    /// 整个单元都核对了为 `None`。
    pub scope: Option<ElementScope>,
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
    /// 比对单元的种类（[`RunRow::unit`]）；页面单根给 `None`。
    pub unit: Option<String>,
    /// 所在的上级根（[`RunRow::root_refno`]）；页面单根给 `None`。
    pub root_refno: Option<String>,
    pub root_noun: Option<String>,
    pub root_name: Option<String>,
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
    /// 构件级结果（落 `rvm_check_element`）。比对没出结果（某一步没跑完）、或作业目录里没有那份结果
    /// （加这张表之前的老产物）时为 `None`：不写、也不清那张表。
    pub elements: Option<ElementFacts>,
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
        root_refno: facts.root_refno.clone(),
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
        row.reason = reason::CENTRELINE_ONLY_TEXT.to_string();
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
        unit: facts.unit.clone(),
        root_refno: facts.root_refno.clone(),
        root_noun: facts.root_noun.clone(),
        root_name: facts.root_name.clone(),
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

/// 一次比对落库的账。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// 差异行数（不含只记账的 `info` 行）。
    pub mismatch_total: usize,
    /// 写进差异表的行数（含 `info` 行）。
    pub rows: usize,
    /// 写进构件表的行数。
    pub elements: usize,
    /// 从构件表清掉的陈旧行数。
    pub stale_elements: usize,
    /// 这个单元在库里已经有更晚的一次核对（[`newer_run_on`]），这次（补入库的老结果）没有动构件表。
    pub elements_superseded: bool,
    /// 写库用了多久（毫秒）：三张表合计，以及其中构件表那一段（查有没有更晚的运行、写行、清陈旧行）。
    pub millis: u64,
    pub element_millis: u64,
}

/// 摊行 + 落库一步到位（三张表）；失败只回 Err，调用方决定记日志还是忽略。
pub async fn persist_facts(facts: &RunFacts) -> Result<Stored> {
    ensure_schema().await?;
    persist_facts_on(&SUL_DB, facts).await
}

/// 落一次比对的全部行。**构件行先写、运行行后写**：构件行写到一半失败时这次运行在库里还没有，续跑会把它
/// 整个补上；反过来的话运行行已经在、构件行却不全，续跑看不出来。
pub async fn persist_facts_on(db: &Surreal<Any>, facts: &RunFacts) -> Result<Stored> {
    let (run, rows) = build_rows(facts);
    let elements = facts
        .elements
        .clone()
        .map(|elements| ElementWrite::new(facts, elements));
    persist_rows_on(db, &run, &rows, elements).await
}

/// 构件行落库时要知道的归属：记在哪个单元、哪个上级根名下，重新核对后清哪一片。
#[derive(Clone, Debug)]
struct ElementTarget {
    unit: String,
    root: Option<String>,
    sweep: StaleSweep,
}

impl ElementTarget {
    fn of(elements: &ElementFacts) -> Self {
        Self {
            unit: elements.unit_refno.clone(),
            root: elements.root_refno.clone(),
            sweep: StaleSweep::of(elements),
        }
    }
}

/// 一次比对要写的构件行（按块现摊，[`ElementRowStream`]），连同它们的归属。
struct ElementWrite {
    target: ElementTarget,
    rows: ElementRowStream,
}

impl ElementWrite {
    /// `facts` 给运行那一层的格（`run_id`、批次、库号、完成时间），`elements` 整份搬进来。
    fn new(facts: &RunFacts, elements: ElementFacts) -> Self {
        Self {
            target: ElementTarget::of(&elements),
            rows: ElementRowStream::new(facts, elements),
        }
    }
}

async fn persist_rows_on(
    db: &Surreal<Any>,
    run: &RunRow,
    rows: &[MismatchRow],
    elements: Option<ElementWrite>,
) -> Result<Stored> {
    let mut stored = Stored {
        mismatch_total: run.mismatch_total,
        rows: rows.len(),
        ..Stored::default()
    };
    let started = std::time::Instant::now();
    let millis =
        |from: std::time::Instant| u64::try_from(from.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Some(ElementWrite {
        target,
        rows: element_rows,
    }) = elements
    {
        // 这个单元已经有更晚的一次核对在库里：这次是补入库的老结果，构件表不动——它多出来的那些件
        // 是后来那次已经清掉（或压根没有）的，写回去就成了陈旧行。运行行与差异行照常补。
        if newer_run_on(db, &target, run.finished_unix_ms)
            .await?
            .is_some()
        {
            stored.elements_superseded = true;
        } else {
            (stored.elements, stored.stale_elements) = persist_elements_on(
                db,
                &run.run_id,
                run.finished_unix_ms,
                element_rows,
                &target.sweep,
            )
            .await?;
        }
        stored.element_millis = millis(started);
    }
    persist_on(db, run, rows).await?;
    stored.millis = millis(started);
    Ok(stored)
}

/// 比对出了结果的终态（批量的 `status`；页面单根的 `status` 就是三色）。
const COMPARED_STATUSES: [&str; 4] = ["pass", "empty", "kind_differ", "fail"];

/// 这个单元（或包着它的整根）在库里有没有比 `finished_unix_ms` 更晚、比对出了结果的运行；有就给它的
/// `run_id`。查的是 `rvm_check_run`（按 `refno` 走索引，一个单元没几行），不扫构件表。
///
/// 运行行不记 `--scope`，所以更晚的那次即使只核对了单元里的一块也算——宁可少写，不把陈旧行写回去。
async fn newer_run_on(
    db: &Surreal<Any>,
    target: &ElementTarget,
    finished_unix_ms: i64,
) -> Result<Option<String>> {
    let root = target
        .root
        .clone()
        .filter(|root| *root != target.unit)
        .unwrap_or_default();
    let mut response = db
        .query(format!(
            "SELECT VALUE run_id FROM {RUN_TABLE} WHERE refno = $unit \
               AND finished_unix_ms > $ms AND status IN $compared LIMIT 1; \
             SELECT VALUE run_id FROM {RUN_TABLE} WHERE refno = $root AND unit = 'root' \
               AND finished_unix_ms > $ms AND status IN $compared LIMIT 1;"
        ))
        .bind(("unit", target.unit.clone()))
        .bind(("root", root))
        .bind(("ms", finished_unix_ms))
        .bind(("compared", COMPARED_STATUSES.to_vec()))
        .await
        .context("查这个单元有没有更晚的运行")?
        .check()?;
    let unit: Vec<String> = response.take(0).context("读更晚的运行")?;
    let root: Vec<String> = response.take(1).context("读更晚的整根运行")?;
    Ok(unit.into_iter().chain(root).next())
}

/// `a/b` → 记录 id 里的写法 `a_b`（与 `pe` 的键同形）。
fn refno_key(refno: &str) -> String {
    refno.trim().trim_start_matches('=').replace('/', "_")
}

/// refno 没解出来的构件，键里带的显示名最多这么长；再长就截断、补上全名的哈希。
const KEY_NAME_LIMIT: usize = 160;

/// 稳定的 64 位哈希（FNV-1a）：只给过长的显示名做后缀，同一个名字每次得同一个键。
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// 一个构件按它的身份得出的键（还没处理同一次比对里的撞键）。见模块文档的表。
fn element_key(check: &ElementCheck, unit_key: &str) -> String {
    let Some(refno) = check.refno.as_deref() else {
        let name = check.name.trim();
        return if name.chars().count() <= KEY_NAME_LIMIT {
            format!("{unit_key}~{}~{name}", check.grade)
        } else {
            let head: String = name.chars().take(KEY_NAME_LIMIT).collect();
            format!("{unit_key}~{}~{head}#{:016x}", check.grade, fnv1a(name))
        };
    };
    let base = refno_key(refno);
    match &check.derived {
        None => base,
        Some(derived) => match derived.kind.as_str() {
            element::DERIVED_CONTAINER_TUBES => format!("{base}:TUBES"),
            element::DERIVED_CONTAINER_RECORD => format!("{base}:TUBES:R{}", derived.ordinal),
            // 只在基准侧的那根管身用的是 E3D 的管号，另起一格，免得与我们自己编号的那根撞键。
            _ if grade::baseline_side(&check.grade) => {
                format!("{base}:TUBI:E{}", derived.ordinal)
            }
            _ => format!("{base}:TUBI:{}", derived.ordinal),
        },
    }
}

/// 每个构件在 `rvm_check_element` 里的键，与 `elements.checks` 同序；第二项是「这个键是撞了以后换的」。
///
/// 键先发给生成侧有的构件，再发给只在基准侧的：基准里按名字解出的 refno 万一与某个生成件相同
/// （两侧名字对不上、其实是同一个元素），refno 那个键留给生成件，基准那一行用带判档后缀的键并标
/// `key_shared`——两行都留，谁也不盖谁。
fn element_keys(elements: &ElementFacts) -> Vec<(String, bool)> {
    let unit_key = refno_key(&elements.unit_refno);
    let mut keys: Vec<(String, bool)> = vec![(String::new(), false); elements.checks.len()];
    let mut used: HashSet<String> = HashSet::with_capacity(elements.checks.len());
    for baseline_pass in [false, true] {
        for (at, check) in elements.checks.iter().enumerate() {
            if grade::baseline_side(&check.grade) != baseline_pass {
                continue;
            }
            let natural = element_key(check, &unit_key);
            keys[at] = if used.insert(natural.clone()) {
                (natural, false)
            } else {
                let stem = format!("{natural}~{}", check.grade);
                let mut key = stem.clone();
                let mut copy = 2usize;
                while !used.insert(key.clone()) {
                    key = format!("{stem}~{copy}");
                    copy += 1;
                }
                (key, true)
            };
        }
    }
    keys
}

/// 一次运行里每一行构件都相同的那几格。
struct ElementRowShared {
    dbnum: Option<u32>,
    unit_refno: String,
    unit_kind: String,
    root_refno: Option<String>,
    scope_kind: Option<String>,
    scope_refno: Option<String>,
    run_id: String,
    batch_id: Option<String>,
    source: String,
    finished_at: String,
    finished_unix_ms: i64,
}

/// 把构件级结果一块一块摊成行。键要看过全体才发得出来（同一次比对里谁撞键），所以先一次发完；行按块
/// 现摊（字符串直接搬过去，不复制），摊过的原始结果随即放掉——整根 8 万件不必「结果一份、行又一份」
/// 同时留在内存里。
struct ElementRowStream {
    shared: ElementRowShared,
    keys: std::vec::IntoIter<(String, bool)>,
    checks: std::vec::IntoIter<ElementCheck>,
}

impl ElementRowStream {
    fn new(facts: &RunFacts, elements: ElementFacts) -> Self {
        let keys = element_keys(&elements);
        Self {
            shared: ElementRowShared {
                dbnum: facts.dbnum,
                unit_refno: elements.unit_refno,
                unit_kind: elements.unit_kind,
                root_refno: elements.root_refno,
                scope_kind: elements.scope.as_ref().map(|scope| scope.kind.clone()),
                scope_refno: elements.scope.map(|scope| scope.refno),
                run_id: facts.run_id.clone(),
                batch_id: facts.batch_id.clone(),
                source: facts.source.to_string(),
                finished_at: facts.finished_at.to_rfc3339(),
                finished_unix_ms: facts.finished_at.timestamp_millis(),
            },
            keys: keys.into_iter(),
            checks: elements.checks.into_iter(),
        }
    }

    /// 下一块，最多 `size` 行；摊完了给空的。
    fn next_chunk(&mut self, size: usize) -> Vec<ElementRow> {
        let shared = &self.shared;
        self.checks
            .by_ref()
            .zip(self.keys.by_ref())
            .take(size)
            .map(|(check, (key, key_shared))| element_row(shared, check, key, key_shared))
            .collect()
    }
}

fn element_row(
    shared: &ElementRowShared,
    check: ElementCheck,
    key: String,
    key_shared: bool,
) -> ElementRow {
    let passed = check.passed();
    let reason =
        (!check.reason.is_empty()).then(|| truncate_chars(&check.reason, MESSAGE_LIMIT));
    let (derived_kind, derived_ordinal, tube_from, tube_to) = match check.derived {
        Some(derived) => (
            Some(derived.kind),
            Some(derived.ordinal),
            derived.from,
            derived.to,
        ),
        None => (None, None, None, None),
    };
    ElementRow {
        id: key,
        refno_resolved: check.refno.is_some(),
        refno: check.refno,
        derived: derived_kind.is_some(),
        derived_kind,
        derived_ordinal,
        tube_from,
        tube_to,
        key_shared: key_shared.then_some(true),
        dbnum: shared.dbnum,
        noun: check.noun,
        name: check.name,
        pair_key: check.pair_key,
        unit_refno: shared.unit_refno.clone(),
        unit_kind: shared.unit_kind.clone(),
        root_refno: shared.root_refno.clone(),
        scope_kind: shared.scope_kind.clone(),
        scope_refno: shared.scope_refno.clone(),
        run_id: shared.run_id.clone(),
        batch_id: shared.batch_id.clone(),
        source: shared.source.clone(),
        grade: check.grade,
        passed,
        kind_match: check.kind_match,
        severity: check.severity,
        max_axis_delta_mm: check.max_axis_delta_mm,
        significant_max_axis_delta_mm: check.significant_max_axis_delta_mm,
        baseline_kinds: check.baseline_kinds,
        generated_kinds: check.generated_kinds,
        baseline_aabb: check.baseline_aabb,
        generated_aabb: check.generated_aabb,
        aabb_frame: check.aabb_frame,
        reason_kind: (!check.reason_kind.is_empty()).then_some(check.reason_kind),
        reason,
        finished_at: shared.finished_at.clone(),
        finished_unix_ms: shared.finished_unix_ms,
    }
}

/// 把一次比对的构件级结果一次全摊成 `rvm_check_element` 的行；没有构件级结果就是空的。
/// 落库不走这里（[`ElementRowStream`] 按块摊），这是给人看一整份用的。
pub fn build_element_rows(facts: &RunFacts) -> Vec<ElementRow> {
    match facts.elements.clone() {
        Some(elements) => ElementRowStream::new(facts, elements).next_chunk(usize::MAX),
        None => Vec::new(),
    }
}

/// 一个单元重新核对后，要把哪一片里比这次更早的构件行清掉——这次没再出现的那些：构件删了、挪到别处了。
/// 只清**更早**的：补入库的老结果不会删掉后来的行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaleSweep {
    /// 整个单元（ZONE / 兜底单元）核对过：记在它名下的旧行。
    Unit(String),
    /// 整根核对过：这个根下的全部旧行，包括此前按 ZONE、按范围写下的。
    Root(String),
    /// 只核对了单元里的一块：只清同一范围上次留下的，单元里别处的行不动。
    Scope { unit: String, scope: ElementScope },
}

impl StaleSweep {
    pub fn of(elements: &ElementFacts) -> Self {
        match &elements.scope {
            Some(scope) => Self::Scope {
                unit: elements.unit_refno.clone(),
                scope: scope.clone(),
            },
            None if elements.unit_kind == "root" => Self::Root(elements.unit_refno.clone()),
            None => Self::Unit(elements.unit_refno.clone()),
        }
    }
}

/// 落一次比对的构件行，返回 `(写的行数, 清掉的陈旧行数)`。
///
/// 每块一笔事务：先把这一块的键里**不比这次新**的旧行删掉，再 `INSERT IGNORE`——库里那一行比这次还新
/// （这次是补入库的老结果）时删不掉、也插不进，留着新的。全部写完再按 [`StaleSweep`] 清陈旧行。
/// 同一次运行重写一遍结果相同。
async fn persist_elements_on(
    db: &Surreal<Any>,
    run_id: &str,
    finished_unix_ms: i64,
    mut rows: ElementRowStream,
    sweep: &StaleSweep,
) -> Result<(usize, usize)> {
    let mut written = 0usize;
    loop {
        let chunk = rows.next_chunk(ELEMENT_CHUNK);
        if chunk.is_empty() {
            break;
        }
        written += chunk.len();
        let keys: Vec<String> = chunk.iter().map(|row| row.id.clone()).collect();
        db.query(format!(
            "BEGIN TRANSACTION; \
             LET $ids = $keys.map(|$key| type::thing('{ELEMENT_TABLE}', $key)); \
             DELETE $ids WHERE finished_unix_ms <= $ms RETURN NONE; \
             INSERT IGNORE INTO {ELEMENT_TABLE} $rows RETURN NONE; \
             COMMIT TRANSACTION;"
        ))
        .bind(("keys", keys))
        .bind(("ms", finished_unix_ms))
        .bind(("rows", chunk))
        .await
        .with_context(|| format!("写 {ELEMENT_TABLE} {run_id}"))?
        .check()
        .with_context(|| format!("写 {ELEMENT_TABLE} {run_id} 的语句"))?;
    }
    let (condition, binds): (&str, Vec<(&'static str, String)>) = match sweep {
        StaleSweep::Unit(unit) => ("unit_refno = $unit", vec![("unit", unit.clone())]),
        StaleSweep::Root(root) => ("root_refno = $unit", vec![("unit", root.clone())]),
        StaleSweep::Scope { unit, scope } => (
            "unit_refno = $unit AND scope_kind = $scope_kind AND scope_refno = $scope_refno",
            vec![
                ("unit", unit.clone()),
                ("scope_kind", scope.kind.clone()),
                ("scope_refno", scope.refno.clone()),
            ],
        ),
    };
    let mut query = db
        .query(format!(
            "LET $stale = (SELECT VALUE id FROM {ELEMENT_TABLE} WHERE {condition} AND finished_unix_ms < $ms); \
             DELETE $stale RETURN NONE; \
             RETURN array::len($stale);"
        ))
        .bind(("ms", finished_unix_ms));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query
        .await
        .with_context(|| format!("清 {ELEMENT_TABLE} 的陈旧行 {run_id}"))?
        .check()
        .with_context(|| format!("清 {ELEMENT_TABLE} 的陈旧行 {run_id} 的语句"))?;
    let stale: Option<usize> = response
        .take(2)
        .with_context(|| format!("读 {ELEMENT_TABLE} 清掉的行数 {run_id}"))?;
    Ok((written, stale.unwrap_or(0)))
}

/// 连库最多等多久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// 一份事实落库最多等多久：库卡住时只丢这一份，不让整批跟着挂。
const PERSIST_TIMEOUT: Duration = Duration::from_secs(120);

/// 带构件行的那一份多给些时间：每千行加 3 秒（整根 8 万件共 6 分钟；草稿库上覆盖 8 万行实测 21 秒）。
fn persist_timeout(element_rows: usize) -> Duration {
    PERSIST_TIMEOUT + Duration::from_secs(element_rows.div_ceil(1000) as u64 * 3)
}

/// 批量命令与命令行查询都不在服务进程里，各自连一次 `DbOption.toml` 指的那台库。
/// `store_mode` 是 mem 时库只在服务进程里，这里够不着，直接回 Err 让调用方说明。
fn ensure_store_reachable() -> Result<()> {
    anyhow::ensure!(
        crate::options::store_mode().is_durable(),
        "store_mode 是 mem（进程内嵌库），库只在服务进程里，这里连不上；以 AIOS_STORE_MODE=rocksdb 指到真在跑的 SurrealDB 再跑"
    );
    Ok(())
}

/// 入库用的连接：走旁路入口 [`crate::attach_store_connected`]（与改成逐个落库之前同一条路，
/// 连上时会照例重载 `resource/surreal` 下的函数定义），再确保两张表在。
#[cfg(feature = "ws")]
async fn connect_store() -> Result<()> {
    ensure_store_reachable()?;
    tokio::time::timeout(CONNECT_TIMEOUT, crate::attach_store_connected())
        .await
        .with_context(|| format!("连 SurrealDB 超时（{} s）", CONNECT_TIMEOUT.as_secs()))?
        .context("连 SurrealDB")?;
    ensure_schema().await
}

/// 只读查询用的连接：连上、选库、登录，别的什么都不做——不重载函数定义、不修 `DB.STYP`、不建表建索引
/// （[`connect_store`] 这三样都会做）。查询只用内置语句，所以在哪个目录下跑、服务开没开着，
/// 都不会动库里的东西；库里还没有这两张表时查出来是空的。
#[cfg(feature = "ws")]
async fn connect_store_read_only() -> Result<()> {
    ensure_store_reachable()?;
    let db_option = crate::options::get_db_option_ext().inner;
    let endpoint = db_option.get_version_db_conn_str();
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        aios_core::connect_surdb(
            &endpoint,
            &db_option.surreal_ns,
            &db_option.project_name,
            &db_option.v_user,
            &db_option.v_password,
        ),
    )
    .await
    .with_context(|| format!("连 SurrealDB 超时（{} s）", CONNECT_TIMEOUT.as_secs()))?
    .with_context(|| format!("连 SurrealDB（{endpoint}）"))
}

#[cfg(not(feature = "ws"))]
async fn connect_store() -> Result<()> {
    anyhow::bail!("这个构建没有 ws 特性，连不上 SurrealDB")
}

#[cfg(not(feature = "ws"))]
async fn connect_store_read_only() -> Result<()> {
    connect_store().await
}

/// 这一批已经落进库的 `run_id`。续跑时拿它判断哪些单元上次没落进去。
pub async fn stored_run_ids_on(db: &Surreal<Any>, batch_id: &str) -> Result<HashSet<String>> {
    let mut response = db
        .query(format!(
            "SELECT VALUE run_id FROM {RUN_TABLE} WHERE batch_id = $batch_id;"
        ))
        .bind(("batch_id", batch_id.to_string()))
        .await
        .context("查这一批已入库的 run_id")?
        .check()?;
    let ids: Vec<String> = response.take(0).context("读这一批已入库的 run_id")?;
    Ok(ids.into_iter().collect())
}

/// 入库线程收尾时交回的账。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoreOutcome {
    /// 落进库的运行数。
    pub runs: usize,
    /// 连带写进去的差异行数（含只记账的 `info` 行）。
    pub rows: usize,
    /// 连带写进去的构件行数，以及重新核对时清掉的陈旧构件行数。
    pub elements: usize,
    pub stale_elements: usize,
    /// 库里已经有更晚的一次核对、所以没动构件表的运行数（补入库的老结果）。
    pub elements_superseded: usize,
    /// 落进去的那些运行写库合计用了多久（毫秒），以及其中构件表那一段。入库线程与生成并行，
    /// 这不是整批多出来的墙钟。
    pub millis: u64,
    pub element_millis: u64,
    /// 没落进去的运行：`(run_id, 原因)`。
    pub failures: Vec<(String, String)>,
    /// 连库这一步就没成：一行都没写。
    pub connect_error: Option<String>,
}

impl StoreOutcome {
    /// 全落进去是 `Ok((运行数, 行数))`；连不上、或有哪一份没落进去，说清差在哪。
    pub fn into_result(self) -> std::result::Result<(usize, usize), String> {
        if let Some(error) = self.connect_error {
            return Err(error);
        }
        let Some((run_id, reason)) = self.failures.first() else {
            return Ok((self.runs, self.rows));
        };
        Err(format!(
            "{} 个落进去了，{} 个没落进去（第一个 {run_id}：{reason}）",
            self.runs,
            self.failures.len()
        ))
    }
}

/// 排队的构件行数减去刚处理完的那一份（不许减成负的：测试里直接往信箱塞的那些没记过账）。
fn release_pending(pending: &AtomicUsize, rows: usize) {
    let _ = pending.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(rows))
    });
}

/// 把陆续送来的事实逐份落库，送完为止。哪一份失败或超时只记进 [`StoreOutcome::failures`]，不拦后面的。
/// `pending` 是还在排队的构件行数（[`StoreWorker::submit`] 记的账），处理完一份放一份。
async fn persist_stream(
    db: &Surreal<Any>,
    inbox: &mut tokio::sync::mpsc::UnboundedReceiver<RunFacts>,
    pending: &AtomicUsize,
) -> StoreOutcome {
    let mut outcome = StoreOutcome::default();
    while let Some(mut facts) = inbox.recv().await {
        let (run, rows) = build_rows(&facts);
        // 构件级结果整份搬进去，写的时候按块摊成行、摊过的随即放掉。
        let elements = facts.elements.take();
        let queued = elements
            .as_ref()
            .map_or(0, |elements| elements.checks.len());
        let elements = elements.map(|elements| ElementWrite::new(&facts, elements));
        drop(facts);
        let limit = persist_timeout(queued);
        let written =
            tokio::time::timeout(limit, persist_rows_on(db, &run, &rows, elements)).await;
        release_pending(pending, queued);
        let failure = match written {
            Ok(Ok(stored)) => {
                outcome.runs += 1;
                outcome.rows += stored.rows;
                outcome.elements += stored.elements;
                outcome.stale_elements += stored.stale_elements;
                outcome.elements_superseded += usize::from(stored.elements_superseded);
                outcome.millis += stored.millis;
                outcome.element_millis += stored.element_millis;
                continue;
            }
            Ok(Err(error)) => format!("{error:#}"),
            Err(_) => format!("写库超过 {} 秒没有回应", limit.as_secs()),
        };
        eprintln!("警告：{} 没有入库：{failure}", run.run_id);
        outcome.failures.push((run.run_id, failure));
    }
    outcome
}

/// 批量命令的入库线程：连一次库，一个单元跑完就落一个，不等整批结束。
///
/// 批量命令是同步代码、不在 tokio 里，所以线程自己起一个当前线程 runtime。事实从各个工人线程送进来，
/// 收的一侧是异步的：等下一份的时候 runtime 仍在转，库连接的后台任务不会被饿死。
pub struct StoreWorker {
    facts: tokio::sync::mpsc::UnboundedSender<RunFacts>,
    thread: std::thread::JoinHandle<StoreOutcome>,
    /// 已经交出去、还没写完的构件行数。
    pending_rows: Arc<AtomicUsize>,
}

impl StoreWorker {
    /// 起线程并等它连上库。第二项是这一批已经在库里的 `run_id`（[`stored_run_ids_on`]）；连不上时是原因，
    /// 返回的工人照样能收事实（收了就丢），调用方不必分两条路写。
    pub fn start(batch_id: &str) -> (Self, std::result::Result<HashSet<String>, String>) {
        let (facts, inbox) = tokio::sync::mpsc::unbounded_channel();
        let (ready, readiness) = std::sync::mpsc::channel();
        let batch_id = batch_id.to_string();
        let pending_rows = Arc::new(AtomicUsize::new(0));
        let pending = pending_rows.clone();
        let thread = std::thread::spawn(move || store_thread(&batch_id, inbox, ready, &pending));
        let stored = readiness
            .recv()
            .unwrap_or_else(|_| Err("入库线程没连上库就退出了".to_string()));
        (
            Self {
                facts,
                thread,
                pending_rows,
            },
            stored,
        )
    }

    /// 交一份事实去落库，不等它写完。线程已经退出时这一份丢掉，账在 [`Self::finish`] 里。
    ///
    /// 排队的构件行超过 [`PENDING_ELEMENT_ROWS_LIMIT`] 时在这里等库写完一些再交：生成比入库快的时候
    /// （几个整根前后脚跑完）父进程里攒着的行数有界，不随单元数涨。按 ZONE 跑一个单元百来行，碰不到这条线。
    pub fn submit(&self, facts: RunFacts) {
        let rows = facts
            .elements
            .as_ref()
            .map_or(0, |elements| elements.checks.len());
        while rows > 0
            && self.pending_rows.load(Ordering::Relaxed) > PENDING_ELEMENT_ROWS_LIMIT
            && !self.facts.is_closed()
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.pending_rows.fetch_add(rows, Ordering::Relaxed);
        if self.facts.send(facts).is_err() {
            release_pending(&self.pending_rows, rows);
        }
    }

    /// 等已交的都写完，交回这一批的账。
    pub fn finish(self) -> StoreOutcome {
        drop(self.facts);
        self.thread.join().unwrap_or_else(|_| StoreOutcome {
            connect_error: Some("入库线程 panic".to_string()),
            ..StoreOutcome::default()
        })
    }
}

fn store_thread(
    batch_id: &str,
    mut inbox: tokio::sync::mpsc::UnboundedReceiver<RunFacts>,
    ready: std::sync::mpsc::Sender<std::result::Result<HashSet<String>, String>>,
    pending: &AtomicUsize,
) -> StoreOutcome {
    let failed = |error: String| StoreOutcome {
        connect_error: Some(error),
        ..StoreOutcome::default()
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let message = format!("起入库用的 runtime 失败：{error}");
            let _ = ready.send(Err(message.clone()));
            return failed(message);
        }
    };
    runtime.block_on(async {
        let connected = async {
            connect_store().await?;
            stored_run_ids_on(&SUL_DB, batch_id).await
        }
        .await;
        match connected {
            Ok(stored) => {
                let _ = ready.send(Ok(stored));
                persist_stream(&SUL_DB, &mut inbox, pending).await
            }
            Err(error) => {
                let message = format!("{error:#}");
                let _ = ready.send(Err(message.clone()));
                // 没连上：送来的照收照丢，送的一侧不必知道。
                while let Some(facts) = inbox.recv().await {
                    release_pending(
                        pending,
                        facts
                            .elements
                            .as_ref()
                            .map_or(0, |elements| elements.checks.len()),
                    );
                }
                failed(message)
            }
        }
    })
}

/// `rvm_check_run` 的查询条件；没给的不限。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RunFilter {
    pub refno: Option<String>,
    /// 上级根（SITE / APPLDW）：它自己那一行，加上它下面的全部单元（[`ROOT_SCOPE`]）。
    pub root_refno: Option<String>,
    /// 单元种类：`root` / `zone` / `fallback`。加这一格之前写进来的行没有它，筛不出来。
    pub unit: Option<String>,
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
    /// 上级根（SITE / APPLDW）下全部运行的差异（[`ROOT_SCOPE`]）。
    pub root_refno: Option<String>,
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

/// [`where_clause`] 的特殊算子：按上级根筛。没有 `root_refno` 的行（页面单根，以及加这一格之前写进来的
/// 批量行——那时一行就是一个根）把自己的 `refno` 当上级根，所以按 SITE 查历史不会漏掉老记录。
const ROOT_SCOPE: &str = "root-scope";

/// `WHERE a = $a AND b = $b` 这一段；返回子句与要绑的参数。
fn where_clause(
    terms: Vec<(&'static str, &'static str, Option<Value>)>,
) -> (String, Vec<(&'static str, Value)>) {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    for (column, operator, value) in terms {
        if let Some(value) = value {
            parts.push(if operator == ROOT_SCOPE {
                format!("({column} = ${column} OR (!{column} AND refno = ${column}))")
            } else {
                format!("{column} {operator} ${column}")
            });
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

fn run_where(filter: &RunFilter) -> (String, Vec<(&'static str, Value)>) {
    where_clause(vec![
        (
            "refno",
            "=",
            filter
                .refno
                .as_deref()
                .map(normalize_refno)
                .map(Value::from),
        ),
        (
            "root_refno",
            ROOT_SCOPE,
            filter
                .root_refno
                .as_deref()
                .map(normalize_refno)
                .map(Value::from),
        ),
        ("unit", "=", filter.unit.clone().map(Value::from)),
        ("dbnum", "=", filter.dbnum.map(Value::from)),
        ("verdict", "=", filter.verdict.clone().map(Value::from)),
        ("status", "=", filter.status.clone().map(Value::from)),
        ("source", "=", filter.source.clone().map(Value::from)),
        ("batch_id", "=", filter.batch_id.clone().map(Value::from)),
        ("project", "=", filter.project.clone().map(Value::from)),
        ("finished_unix_ms", "<", filter.before.map(Value::from)),
    ])
}

/// 最近的比对运行，`finished_unix_ms` 倒序。
pub async fn list_runs_on(db: &Surreal<Any>, filter: &RunFilter) -> Result<Vec<Value>> {
    let (clause, binds) = run_where(filter);
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
        (
            "root_refno",
            ROOT_SCOPE,
            filter
                .root_refno
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

/// `rvm_check_element` 的查询条件；没给的不限。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ElementFilter {
    /// 构件的 refno：它自己那一行，加上记在它名下的派生件（隐式管身）。
    pub refno: Option<String>,
    /// 所在的核对单元（ZONE / 兜底单元；整根跑的是 SITE / APPLDW）。
    pub unit_refno: Option<String>,
    /// 所在的上级根（SITE / APPLDW）。
    pub root_refno: Option<String>,
    pub dbnum: Option<u32>,
    pub batch_id: Option<String>,
    pub run_id: Option<String>,
    pub source: Option<String>,
    pub noun: Option<String>,
    /// 判档：exact / chord / reference_artifact / film / off / only_baseline / only_generated / …。
    pub grade: Option<String>,
    /// `ok` / `info` / `warning` / `error`。
    pub severity: Option<String>,
    pub passed: Option<bool>,
    pub kind_match: Option<bool>,
    /// 只看派生件 / 只看有自己 refno 的。
    pub derived: Option<bool>,
    /// 只看 refno 解出来的 / 没解出来的。
    pub refno_resolved: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn element_where(filter: &ElementFilter) -> (String, Vec<(&'static str, Value)>) {
    let refno = |raw: &Option<String>| raw.as_deref().map(normalize_refno).map(Value::from);
    where_clause(vec![
        ("refno", "=", refno(&filter.refno)),
        ("unit_refno", "=", refno(&filter.unit_refno)),
        ("root_refno", "=", refno(&filter.root_refno)),
        ("run_id", "=", filter.run_id.clone().map(Value::from)),
        ("batch_id", "=", filter.batch_id.clone().map(Value::from)),
        ("grade", "=", filter.grade.clone().map(Value::from)),
        ("noun", "=", filter.noun.clone().map(Value::from)),
        ("dbnum", "=", filter.dbnum.map(Value::from)),
        ("source", "=", filter.source.clone().map(Value::from)),
        ("severity", "=", filter.severity.clone().map(Value::from)),
        ("passed", "=", filter.passed.map(Value::from)),
        ("kind_match", "=", filter.kind_match.map(Value::from)),
        ("derived", "=", filter.derived.map(Value::from)),
        (
            "refno_resolved",
            "=",
            filter.refno_resolved.map(Value::from),
        ),
    ])
}

/// 构件行，按记录 id（也就是 refno 的键）的顺序：同一组条件翻页时顺序不变。每行多一格 `key`
/// （记录 id 的键）。第二项是这一页之后还有没有。
///
/// 不按完成时间排：一个整根八万行，排序要把命中的行全读一遍；按 id 顺序可以读到够数就停。
pub async fn list_elements_on(
    db: &Surreal<Any>,
    filter: &ElementFilter,
) -> Result<(Vec<Value>, bool)> {
    let (clause, binds) = element_where(filter);
    let limit = clamp_limit(filter.limit, 500);
    let offset = filter.offset.unwrap_or(0);
    let mut query = db.query(format!(
        "SELECT *, record::id(id) AS key OMIT id FROM {ELEMENT_TABLE}{clause} LIMIT {} START {offset};",
        limit + 1
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("查 rvm_check_element")?.check()?;
    let mut rows = response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_element 行")?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    Ok((rows, more))
}

/// [`element_grades_on`] 从库里读回来的一组计数。
#[derive(Deserialize)]
struct GradeTally {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    unit_refno: Option<String>,
    #[serde(default)]
    root_refno: Option<String>,
    #[serde(default)]
    grade: String,
    #[serde(default)]
    kind_match: Option<bool>,
    #[serde(default)]
    passed: bool,
    #[serde(default)]
    n: u64,
}

/// 一组构件行按判档数出来的账：`counts` 与 `rvm_check_run.counts` 同一个形状，可以逐格对账。
#[derive(Default)]
struct GradeSum {
    rows: u64,
    passed: u64,
    grades: BTreeMap<String, u64>,
    counts: RvmCounts,
}

impl GradeSum {
    fn add(&mut self, tally: &GradeTally) {
        self.rows += tally.n;
        if tally.passed {
            self.passed += tally.n;
        }
        *self.grades.entry(tally.grade.clone()).or_default() += tally.n;
        let counts = &mut self.counts;
        let paired = match tally.grade.as_str() {
            grade::EXACT => Some(&mut counts.exact),
            grade::CHORD => Some(&mut counts.chord),
            grade::REFERENCE_ARTIFACT => Some(&mut counts.reference_artifact),
            grade::FILM => Some(&mut counts.film),
            grade::OFF => Some(&mut counts.off),
            _ => None,
        };
        if let Some(slot) = paired {
            *slot += tally.n;
            counts.both += tally.n;
            match tally.kind_match {
                Some(true) => counts.kind_match += tally.n,
                Some(false) => counts.kind_differ += tally.n,
                None => {}
            }
            return;
        }
        match tally.grade.as_str() {
            grade::ONLY_BASELINE => counts.only_baseline += tally.n,
            grade::ONLY_GENERATED => counts.only_generated += tally.n,
            grade::UNKEYABLE_BASELINE => counts.unkeyable_baseline += tally.n,
            grade::UNKEYABLE_GENERATED => counts.unkeyable_generated += tally.n,
            grade::CENTRELINE_ONLY => counts.centreline_only_baseline += tally.n,
            grade::CONTAINER_OFF => counts.container_off += tally.n,
            grade::CONTAINER_ONLY_BASELINE => counts.container_only_baseline += tally.n,
            grade::CONTAINER_ONLY_GENERATED => counts.container_only_generated += tally.n,
            _ => {}
        }
    }

    fn json(&self) -> Value {
        serde_json::json!({
            "rows": self.rows,
            "passed": self.passed,
            "not_passed": self.rows - self.passed,
            "grades": self.grades,
            "counts": self.counts,
        })
    }
}

/// 构件行按判档计数，按「哪次运行写的、记在哪个单元名下」分组。
///
/// 应答：`{count, rows, passed, not_passed, grades, counts, units: [...]}`。顶层是命中的全部行的合计；
/// `units` 每组一行（`run_id`、`unit_refno`、`root_refno` 加同样五格）。`counts` 是把各档条数摆回
/// `rvm_check_run.counts` 的形状——刚跑完的一个单元，它这一组的 `counts` 应当与它运行行里的 `counts`
/// 逐格相同（之后别的运行重新核对了其中的构件，行就归到新的那次名下了）。
pub async fn element_grades_on(db: &Surreal<Any>, filter: &ElementFilter) -> Result<Value> {
    let (clause, binds) = element_where(filter);
    let mut query = db.query(format!(
        "SELECT run_id, unit_refno, root_refno, grade, kind_match, passed, count() AS n \
         FROM {ELEMENT_TABLE}{clause} GROUP BY run_id, unit_refno, root_refno, grade, kind_match, passed;"
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("汇总 rvm_check_element")?.check()?;
    let tallies: Vec<GradeTally> = response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_element 汇总")?
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<_, _>>()
        .context("rvm_check_element 的汇总行读不成计数")?;
    let mut total = GradeSum::default();
    let mut units: BTreeMap<(String, String), (Option<String>, GradeSum)> = BTreeMap::new();
    for tally in &tallies {
        total.add(tally);
        let unit = units
            .entry((
                tally.run_id.clone().unwrap_or_default(),
                tally.unit_refno.clone().unwrap_or_default(),
            ))
            .or_default();
        if unit.0.is_none() {
            unit.0.clone_from(&tally.root_refno);
        }
        unit.1.add(tally);
    }
    let units: Vec<Value> = units
        .into_iter()
        .map(|((run_id, unit_refno), (root_refno, sum))| {
            let mut unit = sum.json();
            unit["run_id"] = Value::from(run_id);
            unit["unit_refno"] = Value::from(unit_refno);
            unit["root_refno"] = Value::from(root_refno);
            unit
        })
        .collect();
    let mut answer = total.json();
    answer["count"] = Value::from(units.len());
    answer["units"] = Value::from(units);
    Ok(answer)
}

/// [`rollup_on`] 一次最多读多少行运行记录。九个库按 ZONE 跑一批约 2,500 行；超了就只汇总最新的
/// 这么多行，并在应答里标 `truncated`。
const ROLLUP_SCAN_LIMIT: usize = 20_000;
/// 一行汇总里最多列多少个没过的运行；总数另记在 `not_clean_runs`。
const ROLLUP_NOT_CLEAN_LIMIT: usize = 50;

/// [`rollup_on`] 从 `rvm_check_run` 读回来的一行。
#[derive(Deserialize)]
struct RollupSource {
    #[serde(default)]
    batch_id: Option<String>,
    #[serde(default)]
    dbnum: Option<u32>,
    refno: String,
    #[serde(default)]
    noun: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    root_refno: Option<String>,
    #[serde(default)]
    root_noun: Option<String>,
    #[serde(default)]
    root_name: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    counts: RvmCounts,
    #[serde(default)]
    mismatch_total: usize,
    #[serde(default)]
    seconds: StageSeconds,
    #[serde(default)]
    gen_peak_private_mb: Option<u64>,
    #[serde(default)]
    finished_unix_ms: i64,
    #[serde(default)]
    finished_at: String,
}

/// 一个上级根在一批里的合计。
#[derive(Default)]
struct RootRollup {
    root_noun: Option<String>,
    root_name: Option<String>,
    runs: usize,
    units: BTreeMap<String, usize>,
    status: BTreeMap<String, usize>,
    counts: RvmCounts,
    mismatch_total: usize,
    seconds: StageSeconds,
    gen_peak_private_mb: Option<u64>,
    finished_unix_ms: i64,
    finished_at: String,
    not_clean_runs: usize,
    not_clean: Vec<Value>,
}

/// 批量结果按上级根汇总：每批、每个库、每个上级根（SITE / APPLDW）、每种跑法一行。
///
/// `mode` 是跑法：`root` 是把整个根当一个单元比的那一行，`zone` 是按 ZONE 拆开比的各单元之和（兜底单元
/// 也算在里面，`units` 里分开计数）。同一批里两种跑法都有时各出一行，不相加——两行的 `counts` 正好
/// 用来核对「各 ZONE 之和等于整根」。
///
/// 只汇总批量行（页面单根各是各的一次比对，没有批）。应答：`{count, scanned, truncated, roots: [...]}`，
/// 最新的批在前；每行带单元数、各终态个数、计数合计、差异行数、各阶段耗时合计、生成峰值的最大值，以及
/// 没过的运行（最多 [`ROLLUP_NOT_CLEAN_LIMIT`] 个）。
pub async fn rollup_on(db: &Surreal<Any>, filter: &RunFilter) -> Result<Value> {
    let (clause, binds) = run_where(&RunFilter {
        source: Some(SOURCE_BATCH.to_string()),
        ..filter.clone()
    });
    let scan = ROLLUP_SCAN_LIMIT + 1;
    let mut query = db.query(format!(
        "SELECT batch_id, dbnum, refno, noun, name, unit, root_refno, root_noun, root_name, status, \
         verdict, counts, mismatch_total, seconds, gen_peak_private_mb, finished_unix_ms, finished_at \
         FROM {RUN_TABLE}{clause} ORDER BY finished_unix_ms DESC LIMIT {scan};"
    ));
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut response = query.await.context("汇总 rvm_check_run")?.check()?;
    let mut rows = response
        .take::<Vec<Value>>(0)
        .context("读 rvm_check_run 汇总")?;
    let truncated = rows.len() > ROLLUP_SCAN_LIMIT;
    rows.truncate(ROLLUP_SCAN_LIMIT);
    let scanned = rows.len();

    let mut groups: BTreeMap<(String, Option<u32>, String, &'static str), RootRollup> =
        BTreeMap::new();
    let mut batch_latest: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let row: RollupSource =
            serde_json::from_value(row).context("rvm_check_run 的行读不成汇总要的字段")?;
        let batch_id = row.batch_id.unwrap_or_default();
        let unit = row.unit.unwrap_or_else(|| "root".to_string());
        let mode = if unit == "root" { "root" } else { "zone" };
        let root = row.root_refno.unwrap_or_else(|| row.refno.clone());
        let latest = batch_latest.entry(batch_id.clone()).or_insert(i64::MIN);
        *latest = (*latest).max(row.finished_unix_ms);

        let group = groups.entry((batch_id, row.dbnum, root, mode)).or_default();
        // 根行没有单独的上级根：它的 noun / 名字就是上级根的。
        let (root_noun, root_name) = if mode == "root" {
            (
                row.root_noun.or(row.noun.clone()),
                row.root_name.or(row.name.clone()),
            )
        } else {
            (row.root_noun, row.root_name)
        };
        if group.root_noun.is_none() {
            group.root_noun = root_noun;
        }
        if group.root_name.is_none() {
            group.root_name = root_name;
        }
        group.runs += 1;
        *group.units.entry(unit).or_default() += 1;
        *group.status.entry(row.status.clone()).or_default() += 1;
        group.counts.add(&row.counts);
        group.mismatch_total += row.mismatch_total;
        group.seconds.resolve += row.seconds.resolve;
        group.seconds.generate += row.seconds.generate;
        group.seconds.e3d_export += row.seconds.e3d_export;
        group.seconds.compare += row.seconds.compare;
        group.gen_peak_private_mb = group.gen_peak_private_mb.max(row.gen_peak_private_mb);
        if row.finished_unix_ms >= group.finished_unix_ms || group.finished_at.is_empty() {
            group.finished_unix_ms = row.finished_unix_ms;
            group.finished_at = row.finished_at;
        }
        if row.verdict != super::report::VERDICT_PASS {
            group.not_clean_runs += 1;
            if group.not_clean.len() < ROLLUP_NOT_CLEAN_LIMIT {
                group.not_clean.push(serde_json::json!({
                    "refno": row.refno,
                    "noun": row.noun,
                    "name": row.name,
                    "status": row.status,
                }));
            }
        }
    }

    let mut ordered: Vec<_> = groups.into_iter().collect();
    // 最新的批在前；批内按库号、上级根、跑法（`BTreeMap` 的键序）。
    ordered.sort_by_key(|((batch_id, ..), _)| {
        std::cmp::Reverse(batch_latest.get(batch_id).copied().unwrap_or(i64::MIN))
    });
    let round = |seconds: f64| (seconds * 10.0).round() / 10.0;
    let roots: Vec<Value> = ordered
        .into_iter()
        .map(|((batch_id, dbnum, root, mode), group)| {
            serde_json::json!({
                "batch_id": batch_id,
                "dbnum": dbnum,
                "root_refno": root,
                "root_noun": group.root_noun,
                "root_name": group.root_name,
                "mode": mode,
                "runs": group.runs,
                "units": group.units,
                "status": group.status,
                "clean": group.not_clean_runs == 0,
                "not_clean_runs": group.not_clean_runs,
                "not_clean": group.not_clean,
                "counts": group.counts,
                "mismatch_total": group.mismatch_total,
                "seconds": {
                    "resolve": round(group.seconds.resolve),
                    "generate": round(group.seconds.generate),
                    "e3d_export": round(group.seconds.e3d_export),
                    "compare": round(group.seconds.compare),
                },
                "gen_peak_private_mb": group.gen_peak_private_mb,
                "finished_at": group.finished_at,
                "finished_unix_ms": group.finished_unix_ms,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "count": roots.len(),
        "scanned": scanned,
        "truncated": truncated,
        "roots": roots,
    }))
}

/// 一次查询与它的条件；应答体见 [`answer_on`]。
#[derive(Clone, Debug)]
pub enum Query {
    /// 运行记录（一个根 / 单元一行），最新在前。
    Runs(RunFilter),
    /// 按上级根汇总（[`rollup_on`]）。
    Rollup(RunFilter),
    /// 差异明细。
    Mismatches(MismatchFilter),
    /// 差异按原因种类计数，多的在前。
    Reasons(MismatchFilter),
    /// 构件级结果：一个带几何的构件一行，通过的也有（[`list_elements_on`]）。
    Elements(ElementFilter),
    /// 构件级结果按判档计数，按运行与单元分组（[`element_grades_on`]）。
    ElementGrades(ElementFilter),
}

/// 一次查询的应答体。HTTP 接口（`/api/v1/rvm-check/history|rollup|mismatches|reasons|elements|element-grades`）
/// 与命令行 `rvm-check-query` 给的是同一份形状。
pub async fn answer_on(db: &Surreal<Any>, query: &Query) -> Result<Value> {
    Ok(match query {
        Query::Elements(filter) => {
            let (rows, more) = list_elements_on(db, filter).await?;
            serde_json::json!({ "count": rows.len(), "more": more, "rows": rows })
        }
        Query::ElementGrades(filter) => element_grades_on(db, filter).await?,
        Query::Runs(filter) => {
            let runs = list_runs_on(db, filter).await?;
            serde_json::json!({ "count": runs.len(), "runs": runs })
        }
        Query::Rollup(filter) => rollup_on(db, filter).await?,
        Query::Mismatches(filter) => {
            let rows = list_mismatches_on(db, filter).await?;
            serde_json::json!({ "count": rows.len(), "rows": rows })
        }
        Query::Reasons(filter) => {
            let kinds = reason_summary_on(db, filter).await?;
            serde_json::json!({ "count": kinds.len(), "kinds": kinds })
        }
    })
}

/// [`answer_on`] 的 `SUL_DB` 版（服务进程里，库已经连上）：先确保表在。
pub async fn answer(query: &Query) -> Result<Value> {
    ensure_schema().await?;
    answer_on(&SUL_DB, query).await
}

/// 命令行查询（同步代码、不在 tokio 里，服务不必开着）：自己连一次库再查，只读
/// （[`connect_store_read_only`]）。
pub fn query_blocking(query: &Query) -> Result<Value> {
    super::batch::ensure_db_option_present()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("起查询用的 runtime")?;
    runtime.block_on(async {
        connect_store_read_only().await?;
        answer_on(&SUL_DB, query).await
    })
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
            unit: None,
            root_refno: None,
            root_noun: None,
            root_name: None,
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
            elements: None,
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

    /// 一个 ZONE 单元的事实：自己是 `refno`，上级根是 `root`（SITE `/SITE-A`）。
    fn zone_facts(
        batch: &str,
        refno: &str,
        root: &str,
        verdict: &str,
        mismatches: Vec<RvmMismatch>,
    ) -> RunFacts {
        let run_id = format!("{batch}/ams7999/{}", refno.replace('/', "_"));
        let mut run = facts(&run_id, refno, verdict, mismatches);
        run.batch_id = Some(batch.to_string());
        run.noun = Some("ZONE".to_string());
        run.name = Some(format!("/Z-{refno}"));
        run.unit = Some("zone".to_string());
        run.root_refno = Some(root.to_string());
        run.root_noun = Some("SITE".to_string());
        run.root_name = Some("/SITE-A".to_string());
        run.centreline_only_baseline.clear();
        run
    }

    async fn put(db: &Surreal<Any>, run: &RunFacts) {
        let (row, rows) = build_rows(run);
        persist_on(db, &row, &rows).await.unwrap();
    }

    /// 三批：`b-old` 是加上级根字段之前的老行（整根，没有 `unit` / `root_refno`），`b-r` 是整根的新行，
    /// `b-z` 是按 ZONE 拆开的四个单元（根 1/2 下两个 ZONE 加一个兜底，根 1/3 下一个 ZONE）；另有一条页面单根。
    async fn seed_roots_and_units(db: &Surreal<Any>) {
        let at = chrono::Local::now();
        let mut old = facts(
            "b-old/ams7999/1_2",
            "1/2",
            VERDICT_FAIL,
            vec![mismatch("off", reason::AABB_OFF, "/OLD", "BOX")],
        );
        old.batch_id = Some("b-old".to_string());
        old.finished_at = at - chrono::Duration::seconds(60);
        put(db, &old).await;

        let mut root = facts("b-r/ams7999/1_2", "1/2", "pass", Vec::new());
        root.batch_id = Some("b-r".to_string());
        root.unit = Some("root".to_string());
        root.root_refno = Some("1/2".to_string());
        root.root_noun = Some("SITE".to_string());
        root.root_name = Some("/SITE-A".to_string());
        root.finished_at = at - chrono::Duration::seconds(30);
        put(db, &root).await;

        let failing = zone_facts(
            "b-z",
            "5/1",
            "1/2",
            VERDICT_FAIL,
            vec![mismatch("off", reason::AABB_SHIFTED, "/P1", "PIPE")],
        );
        put(db, &failing).await;
        put(db, &zone_facts("b-z", "5/2", "1/2", "pass", Vec::new())).await;
        let mut fallback = zone_facts("b-z", "5/3", "1/2", "pass", Vec::new());
        fallback.unit = Some("fallback".to_string());
        fallback.noun = Some("STRU".to_string());
        fallback.status = "empty".to_string();
        put(db, &fallback).await;
        put(db, &zone_facts("b-z", "6/1", "1/3", "pass", Vec::new())).await;

        let mut page = facts("rvm-check-1", "1/2", "pass", Vec::new());
        page.source = SOURCE_PAGE;
        page.batch_id = None;
        put(db, &page).await;
    }

    /// 按上级根查：它下面的单元行、它自己的根行都在；没有 `root_refno` 的老行与页面单根把自己的
    /// refno 当上级根。差异行同样按上级根查得到。别的根不混进来。
    #[tokio::test]
    async fn runs_and_mismatches_are_found_by_their_root() {
        let db = database("by-root").await;
        seed_roots_and_units(&db).await;

        let under = list_runs_on(
            &db,
            &RunFilter {
                root_refno: Some("=1/2".to_string()),
                ..RunFilter::default()
            },
        )
        .await
        .unwrap();
        let mut ids: Vec<&str> = under
            .iter()
            .map(|run| run["run_id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            [
                "b-old/ams7999/1_2",
                "b-r/ams7999/1_2",
                "b-z/ams7999/5_1",
                "b-z/ams7999/5_2",
                "b-z/ams7999/5_3",
                "rvm-check-1",
            ]
        );

        let zones = list_runs_on(
            &db,
            &RunFilter {
                root_refno: Some("1_2".to_string()),
                unit: Some("zone".to_string()),
                ..RunFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(zones.len(), 2, "{zones:?}");
        assert!(
            zones
                .iter()
                .all(|run| run["root_refno"] == "1/2" && run["root_name"] == "/SITE-A")
        );

        let errors = list_mismatches_on(
            &db,
            &MismatchFilter {
                root_refno: Some("1/2".to_string()),
                severity: Some(reason::ERROR.to_string()),
                ..MismatchFilter::default()
            },
        )
        .await
        .unwrap();
        let mut names: Vec<&str> = errors
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["/OLD", "/P1"]);
        let unit_row = errors.iter().find(|row| row["name"] == "/P1").unwrap();
        assert_eq!(unit_row["refno"], "5/1", "差异行的 refno 仍是它所在的单元");
        assert_eq!(unit_row["root_refno"], "1/2");
    }

    /// 汇总：一批里一个上级根一行，单元数、终态、计数、差异行数都是各单元之和，没过的单元列出来。
    /// 整根那一行与按 ZONE 拆开的各行分开汇总（`mode`），不相加；页面单根不进汇总；最新的批在前。
    #[tokio::test]
    async fn rollup_sums_units_per_root_and_keeps_the_two_modes_apart() {
        let db = database("rollup").await;
        seed_roots_and_units(&db).await;

        let all = rollup_on(&db, &RunFilter::default()).await.unwrap();
        assert_eq!(all["scanned"], 6, "六条批量行，页面单根不算：{all}");
        assert_eq!(all["truncated"], false);
        let roots = all["roots"].as_array().unwrap();
        let keys: Vec<(&str, &str, &str)> = roots
            .iter()
            .map(|root| {
                (
                    root["batch_id"].as_str().unwrap(),
                    root["root_refno"].as_str().unwrap(),
                    root["mode"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("b-z", "1/2", "zone"),
                ("b-z", "1/3", "zone"),
                ("b-r", "1/2", "root"),
                ("b-old", "1/2", "root"),
            ]
        );

        let zones = &roots[0];
        assert_eq!(zones["root_noun"], "SITE");
        assert_eq!(zones["root_name"], "/SITE-A");
        assert_eq!(zones["dbnum"], 7999);
        assert_eq!(zones["runs"], 3);
        assert_eq!(zones["units"]["zone"], 2);
        assert_eq!(zones["units"]["fallback"], 1);
        assert_eq!(zones["status"]["fail"], 1);
        assert_eq!(zones["status"]["pass"], 1);
        assert_eq!(zones["status"]["empty"], 1);
        assert_eq!(zones["clean"], false);
        assert_eq!(zones["not_clean_runs"], 1);
        assert_eq!(zones["not_clean"][0]["refno"], "5/1");
        assert_eq!(zones["not_clean"][0]["status"], "fail");
        assert_eq!(zones["counts"]["both"], 30);
        assert_eq!(zones["counts"]["exact"], 24);
        assert_eq!(zones["counts"]["off"], 1);
        assert_eq!(zones["mismatch_total"], 1);
        assert_eq!(zones["seconds"]["generate"], 7.5);
        assert_eq!(zones["gen_peak_private_mb"], 900);

        assert_eq!(roots[1]["runs"], 1);
        assert_eq!(roots[1]["clean"], true);
        // 整根的行：上级根的 noun / 名字就是它自己的；老行没有 `unit`，算整根。
        assert_eq!(roots[2]["units"]["root"], 1);
        assert_eq!(roots[2]["root_name"], "/SITE-A");
        assert_eq!(roots[2]["counts"]["both"], 10);
        assert_eq!(roots[3]["units"]["root"], 1);
        assert_eq!(roots[3]["root_noun"], "SITE");
        assert_eq!(roots[3]["clean"], false);

        // 命令行与 HTTP 走的是同一个入口，条件照常收窄。
        let one = answer_on(
            &db,
            &Query::Rollup(RunFilter {
                batch_id: Some("b-z".to_string()),
                root_refno: Some("1/3".to_string()),
                ..RunFilter::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(one["count"], 1);
        assert_eq!(one["roots"][0]["root_refno"], "1/3");
        let runs = answer_on(
            &db,
            &Query::Runs(RunFilter {
                batch_id: Some("b-z".to_string()),
                ..RunFilter::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(runs["count"], 4);
        assert_eq!(runs["runs"].as_array().unwrap().len(), 4);
    }

    /// 入库线程的主体：送来几份落几份，送完即止；落进去的 `run_id` 按批读得回来（续跑靠它判断补不补）。
    #[tokio::test]
    async fn the_stream_persists_what_it_is_sent_and_stored_ids_read_back_per_batch() {
        let db = database("stream").await;
        assert!(stored_run_ids_on(&db, "b-z").await.unwrap().is_empty());

        let (facts, mut inbox) = tokio::sync::mpsc::unbounded_channel();
        let failing = zone_facts(
            "b-z",
            "5/1",
            "1/2",
            VERDICT_FAIL,
            vec![mismatch("off", reason::AABB_SHIFTED, "/P1", "PIPE")],
        );
        facts.send(failing).unwrap();
        facts
            .send(zone_facts("b-z", "5/2", "1/2", "pass", Vec::new()))
            .unwrap();
        facts
            .send(zone_facts("b-other", "5/9", "1/2", "pass", Vec::new()))
            .unwrap();
        drop(facts);

        let outcome = untimed(persist_stream(&db, &mut inbox, &AtomicUsize::new(0)).await);
        assert_eq!(
            outcome,
            StoreOutcome {
                runs: 3,
                rows: 1,
                ..StoreOutcome::default()
            }
        );
        assert_eq!(outcome.into_result(), Ok((3, 1)));
        assert_eq!(
            stored_run_ids_on(&db, "b-z").await.unwrap(),
            HashSet::from(["b-z/ams7999/5_1".to_string(), "b-z/ams7999/5_2".to_string()])
        );
    }

    /// 命令行查询不建表：库里还没有这几张表（从没跑过对拍）时，各种查询都回空，不报错。
    #[tokio::test]
    async fn queries_on_a_store_without_the_tables_come_back_empty() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("rvm_check_store")
            .use_db("no-tables")
            .await
            .unwrap();
        for query in [
            Query::Runs(RunFilter::default()),
            Query::Rollup(RunFilter::default()),
            Query::Mismatches(MismatchFilter::default()),
            Query::Reasons(MismatchFilter::default()),
            Query::Elements(ElementFilter::default()),
            Query::ElementGrades(ElementFilter::default()),
        ] {
            let answer = answer_on(&db, &query).await.unwrap();
            assert_eq!(answer["count"], 0, "{query:?} → {answer}");
        }
    }

    // ------------------------------------------------------------------ 构件级结果

    use crate::rvm_check::element::{
        DERIVED_CONTAINER_RECORD, DERIVED_CONTAINER_TUBES, Derived, SEVERITY_OK,
    };

    /// 一条构件级结果：通过的档给 `ok`，其余给 `error` 加一句原因。
    fn element(refno: Option<&str>, grade: &str, name: &str) -> ElementCheck {
        let passed = grade::passed(grade);
        let paired = matches!(grade, "exact" | "chord" | "reference_artifact" | "film" | "off");
        ElementCheck {
            refno: refno.map(str::to_string),
            derived: None,
            noun: "ELBO".to_string(),
            name: name.to_string(),
            pair_key: paired.then(|| format!("ELBO {name}")),
            grade: grade.to_string(),
            kind_match: paired.then_some(true),
            max_axis_delta_mm: paired.then_some(0.01),
            significant_max_axis_delta_mm: None,
            baseline_kinds: vec!["Cylinder".to_string()],
            generated_kinds: vec!["Cylinder".to_string()],
            baseline_aabb: paired.then_some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            generated_aabb: paired.then_some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            aabb_frame: paired.then(|| "local".to_string()),
            reason_kind: if passed {
                String::new()
            } else {
                reason::AABB_OFF.to_string()
            },
            reason: if passed {
                String::new()
            } else {
                format!("{name} 超差")
            },
            severity: if passed { SEVERITY_OK } else { reason::ERROR }.to_string(),
        }
    }

    /// 记在属主名下的派生件。
    fn derived(owner: &str, kind: &str, ordinal: usize, grade: &str) -> ElementCheck {
        ElementCheck {
            derived: Some(Derived {
                kind: kind.to_string(),
                ordinal,
                from: Some("7/100".to_string()),
                to: None,
            }),
            noun: "TUBI".to_string(),
            ..element(Some(owner), grade, &format!("TUBI {ordinal} of BRAN /B"))
        }
    }

    /// 一个 ZONE 单元（`unit`，上级根 `root`）在 `at` 完成的一次核对，带构件级结果。
    fn unit_run(
        batch: &str,
        unit: &str,
        root: &str,
        at: chrono::DateTime<chrono::Local>,
        checks: Vec<ElementCheck>,
    ) -> RunFacts {
        let mut run = zone_facts(batch, unit, root, "pass", Vec::new());
        run.finished_at = at;
        run.elements = Some(ElementFacts {
            checks,
            unit_refno: unit.to_string(),
            unit_kind: "zone".to_string(),
            root_refno: Some(root.to_string()),
            scope: None,
        });
        run
    }

    async fn elements_where(db: &Surreal<Any>, filter: ElementFilter) -> Vec<Value> {
        list_elements_on(db, &filter).await.unwrap().0
    }

    /// 库里全部构件行的 `(键, 判档, 所在单元)`，按键排。
    async fn element_table(db: &Surreal<Any>) -> Vec<(String, String, String)> {
        let mut rows: Vec<(String, String, String)> = elements_where(
            db,
            ElementFilter {
                limit: Some(5000),
                ..ElementFilter::default()
            },
        )
        .await
        .iter()
        .map(|row| {
            (
                row["key"].as_str().unwrap().to_string(),
                row["grade"].as_str().unwrap().to_string(),
                row["unit_refno"].as_str().unwrap().to_string(),
            )
        })
        .collect();
        rows.sort();
        rows
    }

    fn triple(key: &str, grade: &str, unit: &str) -> (String, String, String) {
        (key.to_string(), grade.to_string(), unit.to_string())
    }

    /// 记录 id 的键由 refno 得出：有自己 refno 的就是 `a_b`；派生件记在属主名下带序号，只在基准侧的管身
    /// 另起一格；refno 没解出来的用「单元 + 判档 + 显示名」，名字太长就截断加哈希——都是确定的，重跑得同一个键。
    /// 基准里按名字解出的 refno 与某个生成件相同时，refno 那个键留给生成件，基准那一行换带后缀的键。
    #[test]
    fn element_rows_are_keyed_by_refno_with_deterministic_fallbacks() {
        let long_name = format!("BOX 1 of EQUIPMENT /{}", "X".repeat(300));
        let checks = vec![
            element(Some("24383/1234"), "exact", "/A"),
            derived("1/20", "implied_tube", 2, "exact"),
            derived("1/20", "implied_tube", 2, "only_baseline"),
            derived("1/20", DERIVED_CONTAINER_TUBES, 0, "off"),
            derived("1/20", DERIVED_CONTAINER_RECORD, 3, "container_only_baseline"),
            element(None, "only_baseline", "BOX 2 of EQUIPMENT /E1"),
            element(Some("24383/1234"), "unkeyable_baseline", "/A-OLD-NAME"),
            element(None, "only_generated", &long_name),
            derived("1/21", "hang_axis_tube", 1, "chord"),
        ];
        let run = unit_run("b-z", "5/1", "1/2", chrono::Local::now(), checks);
        let rows = build_element_rows(&run);
        let keys: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(keys[0], "24383_1234");
        assert_eq!(keys[1], "1_20:TUBI:2");
        assert_eq!(keys[2], "1_20:TUBI:E2", "E3D 的管号另起一格");
        assert_eq!(keys[3], "1_20:TUBES");
        assert_eq!(keys[4], "1_20:TUBES:R3");
        assert_eq!(keys[5], "5_1~only_baseline~BOX 2 of EQUIPMENT /E1");
        assert_eq!(keys[6], "24383_1234~unkeyable_baseline");
        assert!(
            keys[7].starts_with("5_1~only_generated~BOX 1 of EQUIPMENT /XXXX") && keys[7].contains('#'),
            "{}",
            keys[7]
        );
        assert!(keys[7].chars().count() < 220, "{}", keys[7].chars().count());
        assert_eq!(keys[8], "1_21:TUBI:1");
        assert_eq!(build_element_rows(&run)[7].id, keys[7], "同一个名字每次得同一个键");

        assert_eq!(rows[6].key_shared, Some(true));
        assert!(rows.iter().enumerate().all(|(at, row)| at == 6 || row.key_shared.is_none()));
        assert_eq!(rows[0].refno.as_deref(), Some("24383/1234"));
        assert!(rows[0].refno_resolved && !rows[0].derived && rows[0].passed);
        assert!(rows[1].derived);
        assert_eq!(rows[1].derived_kind.as_deref(), Some("implied_tube"));
        assert_eq!(rows[1].derived_ordinal, Some(2));
        assert_eq!(rows[1].tube_from.as_deref(), Some("7/100"));
        assert_eq!(rows[1].refno.as_deref(), Some("1/20"), "派生件记在属主名下");
        assert!(!rows[5].refno_resolved && rows[5].refno.is_none() && !rows[5].passed);
        assert_eq!(rows[5].reason_kind.as_deref(), Some(reason::AABB_OFF));
        assert_eq!(rows[0].reason_kind, None, "通过的件没有原因");
        for row in &rows {
            assert_eq!(row.unit_refno, "5/1");
            assert_eq!(row.unit_kind, "zone");
            assert_eq!(row.root_refno.as_deref(), Some("1/2"));
            assert_eq!(row.run_id, "b-z/ams7999/5_1");
            assert_eq!(row.batch_id.as_deref(), Some("b-z"));
            assert_eq!(row.dbnum, Some(7999));
            assert_eq!(row.source, SOURCE_BATCH);
            assert_eq!(row.scope_kind, None);
        }
        let ids: HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(ids.len(), rows.len(), "一次比对里键不重复");

        // 没有构件级结果（某一步没跑完）：一行都不出。
        let failed = zone_facts("b-z", "5/1", "1/2", VERDICT_FAIL, Vec::new());
        assert!(build_element_rows(&failed).is_empty());
    }

    /// 一个单元核对完，它下面每个带几何的构件各有一行（通过的也有），写两遍不翻倍；按记录 id、按 refno、
    /// 按单元 / 判档 / 是否通过 / noun / 批次 / 库号都查得到，能翻页；没有值的格不存。
    #[tokio::test]
    async fn element_rows_land_once_per_component_and_are_queryable() {
        let db = database("elements").await;
        let run = unit_run(
            "b-z",
            "5/1",
            "1/2",
            chrono::Local::now(),
            vec![
                element(Some("7/1"), "exact", "/A"),
                element(Some("7/2"), "off", "/B"),
                derived("9/1", "implied_tube", 1, "exact"),
                derived("9/1", "implied_tube", 2, "chord"),
                element(None, "only_baseline", "/GONE"),
            ],
        );
        let stored = persist_facts_on(&db, &run).await.unwrap();
        assert_eq!((stored.elements, stored.stale_elements), (5, 0));
        assert!(!stored.elements_superseded);
        let again = persist_facts_on(&db, &run).await.unwrap();
        assert_eq!((again.elements, again.stale_elements), (5, 0), "同一次运行重写：不清自己");
        ensure_schema_on(&db).await.unwrap();
        assert_eq!(
            element_table(&db).await,
            [
                triple("5_1~only_baseline~/GONE", "only_baseline", "5/1"),
                triple("7_1", "exact", "5/1"),
                triple("7_2", "off", "5/1"),
                triple("9_1:TUBI:1", "exact", "5/1"),
                triple("9_1:TUBI:2", "chord", "5/1"),
            ]
        );

        // 记录 id 就是 refno 的键：直接按 id 取。
        let mut response = db
            .query("SELECT * OMIT id FROM type::thing('rvm_check_element', '7_2');")
            .await
            .unwrap();
        let by_id: Vec<Value> = response.take(0).unwrap();
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0]["refno"], "7/2");
        assert_eq!(by_id[0]["reason"], "/B 超差");

        // 按 refno：有自己 refno 的一行；属主的 refno 取到记在它名下的派生件。
        let own = elements_where(
            &db,
            ElementFilter {
                refno: Some("=7/1".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(own.len(), 1, "{own:?}");
        let row = &own[0];
        assert_eq!(row["key"], "7_1");
        assert!(row.get("id").is_none(), "不吐记录 id：{row}");
        assert_eq!(row["refno"], "7/1");
        assert_eq!(row["refno_resolved"], true);
        assert_eq!(row["derived"], false);
        assert_eq!(row["dbnum"], 7999);
        assert_eq!(row["noun"], "ELBO");
        assert_eq!(row["name"], "/A");
        assert_eq!(row["pair_key"], "ELBO /A");
        assert_eq!(row["unit_refno"], "5/1");
        assert_eq!(row["unit_kind"], "zone");
        assert_eq!(row["root_refno"], "1/2");
        assert_eq!(row["run_id"], "b-z/ams7999/5_1");
        assert_eq!(row["batch_id"], "b-z");
        assert_eq!(row["source"], "batch");
        assert_eq!(row["grade"], "exact");
        assert_eq!(row["passed"], true);
        assert_eq!(row["kind_match"], true);
        assert_eq!(row["severity"], "ok");
        assert_eq!(row["max_axis_delta_mm"], 0.01);
        assert_eq!(row["baseline_kinds"][0], "Cylinder");
        assert_eq!(row["generated_aabb"][3], 1.0);
        assert_eq!(row["aabb_frame"], "local");
        assert!(row["finished_at"].as_str().is_some_and(|at| at.contains('T')));
        assert_eq!(
            row["finished_unix_ms"],
            run.finished_at.timestamp_millis()
        );
        for absent in [
            "reason",
            "reason_kind",
            "derived_kind",
            "scope_kind",
            "key_shared",
            "significant_max_axis_delta_mm",
        ] {
            assert!(row.get(absent).is_none(), "{absent} 没有值就不存：{row}");
        }
        let tubes = elements_where(
            &db,
            ElementFilter {
                refno: Some("9_1".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(tubes.len(), 2);
        assert!(tubes.iter().all(|row| row["derived"] == true
            && row["derived_kind"] == "implied_tube"
            && row["tube_from"] == "7/100"));
        assert_eq!(tubes[1]["derived_ordinal"], 2);

        // 没通过的：超差的那件带原因；refno 没解出来的那件没有 `refno` 这一格。
        let failed = elements_where(
            &db,
            ElementFilter {
                unit_refno: Some("5/1".to_string()),
                passed: Some(false),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(failed.len(), 2, "{failed:?}");
        let unresolved = elements_where(
            &db,
            ElementFilter {
                refno_resolved: Some(false),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].get("refno").is_none());
        assert_eq!(unresolved[0]["name"], "/GONE");
        assert_eq!(unresolved[0]["reason_kind"], reason::AABB_OFF);

        for (filter, expected) in [
            (
                ElementFilter {
                    grade: Some("chord".to_string()),
                    ..ElementFilter::default()
                },
                1,
            ),
            (
                ElementFilter {
                    noun: Some("TUBI".to_string()),
                    root_refno: Some("1_2".to_string()),
                    ..ElementFilter::default()
                },
                2,
            ),
            (
                ElementFilter {
                    batch_id: Some("b-z".to_string()),
                    dbnum: Some(7999),
                    derived: Some(false),
                    ..ElementFilter::default()
                },
                3,
            ),
            (
                ElementFilter {
                    run_id: Some("b-z/ams7999/5_1".to_string()),
                    severity: Some("error".to_string()),
                    ..ElementFilter::default()
                },
                2,
            ),
            (
                ElementFilter {
                    dbnum: Some(1),
                    ..ElementFilter::default()
                },
                0,
            ),
        ] {
            assert_eq!(
                elements_where(&db, filter.clone()).await.len(),
                expected,
                "{filter:?}"
            );
        }

        // 翻页：命令行与 HTTP 走的同一个入口，应答 `{count, more, rows}`；两页接起来不重不漏。
        let page = |limit: usize, offset: usize| ElementFilter {
            unit_refno: Some("5/1".to_string()),
            limit: Some(limit),
            offset: Some(offset),
            ..ElementFilter::default()
        };
        let first = answer_on(&db, &Query::Elements(page(3, 0))).await.unwrap();
        assert_eq!(first["count"], 3);
        assert_eq!(first["more"], true);
        let second = answer_on(&db, &Query::Elements(page(3, 3))).await.unwrap();
        assert_eq!(second["count"], 2);
        assert_eq!(second["more"], false);
        let mut seen: Vec<&str> = first["rows"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["rows"].as_array().unwrap())
            .map(|row| row["key"].as_str().unwrap())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 5);

        // 按判档计数：一个单元一组，`counts` 摆回运行行的形状。
        let grades = answer_on(
            &db,
            &Query::ElementGrades(ElementFilter {
                batch_id: Some("b-z".to_string()),
                ..ElementFilter::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(grades["count"], 1);
        assert_eq!(grades["rows"], 5);
        assert_eq!(grades["passed"], 3);
        assert_eq!(grades["not_passed"], 2);
        assert_eq!(grades["grades"]["exact"], 2);
        let unit = &grades["units"][0];
        assert_eq!(unit["run_id"], "b-z/ams7999/5_1");
        assert_eq!(unit["unit_refno"], "5/1");
        assert_eq!(unit["root_refno"], "1/2");
        assert_eq!(unit["rows"], 5);
        assert_eq!(unit["counts"]["both"], 4);
        assert_eq!(unit["counts"]["exact"], 2);
        assert_eq!(unit["counts"]["chord"], 1);
        assert_eq!(unit["counts"]["off"], 1);
        assert_eq!(unit["counts"]["only_baseline"], 1);
        assert_eq!(unit["counts"]["kind_match"], 4);
    }

    /// 重新核对：同一个构件覆盖旧行、只留最新一次（`run_id` / `batch_id` 跟着换）；上一次留下、这次已经
    /// 不在这个单元下面的行清掉，别的单元的行不动。构件挪到另一个单元时跟着走，不被原单元清掉。
    /// 整根核对一遍，这个根下此前按 ZONE 写的行也一并清。
    #[tokio::test]
    async fn a_recheck_overwrites_in_place_and_sweeps_what_left_the_unit() {
        let db = database("recheck").await;
        let t0 = chrono::Local::now();
        let at = |seconds: i64| t0 + chrono::Duration::seconds(seconds);
        let abc = vec![
            element(Some("7/1"), "exact", "/A"),
            element(Some("7/2"), "exact", "/B"),
            element(Some("7/3"), "exact", "/C"),
        ];
        put_all(&db, &unit_run("b-1", "5/1", "1/2", at(0), abc)).await;
        put_all(
            &db,
            &unit_run("b-1", "5/2", "1/2", at(1), vec![element(Some("7/4"), "exact", "/D")]),
        )
        .await;

        // 5/1 重新核对：A 这次超差，B 还在，C 没了。
        let stored = put_all(
            &db,
            &unit_run(
                "b-2",
                "5/1",
                "1/2",
                at(5),
                vec![
                    element(Some("7/1"), "off", "/A"),
                    element(Some("7/2"), "exact", "/B"),
                ],
            ),
        )
        .await;
        assert_eq!((stored.elements, stored.stale_elements), (2, 1));
        assert_eq!(
            element_table(&db).await,
            [
                triple("7_1", "off", "5/1"),
                triple("7_2", "exact", "5/1"),
                triple("7_4", "exact", "5/2"),
            ]
        );
        let a = elements_where(
            &db,
            ElementFilter {
                refno: Some("7/1".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(a[0]["run_id"], "b-2/ams7999/5_1");
        assert_eq!(a[0]["batch_id"], "b-2");
        assert_eq!(a[0]["reason"], "/A 超差");
        assert_eq!(a[0]["finished_unix_ms"], at(5).timestamp_millis());

        // B 挪到了 5/2：5/2 核对时行跟着换单元；随后 5/1 再核对（只剩 A）不会把它清掉。
        let stored = put_all(
            &db,
            &unit_run(
                "b-3",
                "5/2",
                "1/2",
                at(6),
                vec![
                    element(Some("7/4"), "exact", "/D"),
                    element(Some("7/2"), "exact", "/B"),
                ],
            ),
        )
        .await;
        assert_eq!(stored.stale_elements, 0);
        let stored = put_all(
            &db,
            &unit_run("b-3", "5/1", "1/2", at(7), vec![element(Some("7/1"), "exact", "/A")]),
        )
        .await;
        assert_eq!(stored.stale_elements, 0, "B 已经记在 5/2 名下");
        assert_eq!(
            element_table(&db).await,
            [
                triple("7_1", "exact", "5/1"),
                triple("7_2", "exact", "5/2"),
                triple("7_4", "exact", "5/2"),
            ]
        );

        // 核对通过但两侧都空（empty）：这个单元名下一行不剩。
        let stored = put_all(&db, &unit_run("b-4", "5/2", "1/2", at(8), Vec::new())).await;
        assert_eq!((stored.elements, stored.stale_elements), (0, 2));

        // 整根核对：根 1/2 下只剩 A 与新出现的 X；此前按 ZONE 写的行只要这次没再出现就清。
        put_all(
            &db,
            &unit_run("b-4", "5/3", "1/2", at(9), vec![element(Some("7/7"), "exact", "/Z")]),
        )
        .await;
        put_all(
            &db,
            &unit_run("b-4", "6/1", "1/3", at(9), vec![element(Some("8/1"), "exact", "/OTHER")]),
        )
        .await;
        let mut whole = unit_run(
            "b-root",
            "1/2",
            "1/2",
            at(20),
            vec![
                element(Some("7/1"), "exact", "/A"),
                element(Some("7/9"), "exact", "/X"),
            ],
        );
        whole.unit = Some("root".to_string());
        whole.elements.as_mut().unwrap().unit_kind = "root".to_string();
        let stored = put_all(&db, &whole).await;
        assert_eq!((stored.elements, stored.stale_elements), (2, 1), "5/3 留下的 /Z");
        assert_eq!(
            element_table(&db).await,
            [
                triple("7_1", "exact", "1/2"),
                triple("7_9", "exact", "1/2"),
                triple("8_1", "exact", "6/1"),
            ],
            "别的根不动"
        );
    }

    async fn put_all(db: &Surreal<Any>, run: &RunFacts) -> Stored {
        persist_facts_on(db, run).await.unwrap()
    }

    /// 入库线程交回的账去掉用时（每次跑都不一样），其余逐格比。
    fn untimed(mut outcome: StoreOutcome) -> StoreOutcome {
        outcome.millis = 0;
        outcome.element_millis = 0;
        outcome
    }

    /// 只核对单元里的一块（`--scope`、页面上比单元之下的节点）：范围之外的行不动，范围里的行带上范围；
    /// 同一范围再核对时只清这个范围上次留下的；之后整个单元核对一遍，范围留下的也一并清。
    #[tokio::test]
    async fn a_scoped_recheck_only_sweeps_what_the_same_scope_left() {
        let db = database("scoped").await;
        let t0 = chrono::Local::now();
        let at = |seconds: i64| t0 + chrono::Duration::seconds(seconds);
        let scoped = |batch: &str, seconds: i64, checks: Vec<ElementCheck>| {
            let mut run = unit_run(batch, "5/1", "1/2", at(seconds), checks);
            run.elements.as_mut().unwrap().scope = Some(ElementScope {
                kind: "subtree".to_string(),
                refno: "8/1".to_string(),
            });
            run
        };
        put_all(
            &db,
            &unit_run(
                "b-1",
                "5/1",
                "1/2",
                at(0),
                vec![
                    element(Some("7/1"), "exact", "/A"),
                    element(Some("7/2"), "exact", "/B"),
                    element(Some("7/3"), "exact", "/C"),
                ],
            ),
        )
        .await;

        let stored = put_all(
            &db,
            &scoped(
                "b-s1",
                5,
                vec![
                    element(Some("7/2"), "off", "/B"),
                    element(Some("7/5"), "exact", "/E"),
                ],
            ),
        )
        .await;
        assert_eq!((stored.elements, stored.stale_elements), (2, 0), "A、C 在范围外，不清");
        let b = elements_where(
            &db,
            ElementFilter {
                refno: Some("7/2".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(b[0]["grade"], "off");
        assert_eq!(b[0]["scope_kind"], "subtree");
        assert_eq!(b[0]["scope_refno"], "8/1");
        assert_eq!(b[0]["unit_refno"], "5/1", "仍记在所属单元名下");

        // 同一范围再核对：这次只有 B，上次这个范围留下的 E 清掉；范围外的 A、C 仍在。
        let stored = put_all(
            &db,
            &scoped("b-s2", 10, vec![element(Some("7/2"), "exact", "/B")]),
        )
        .await;
        assert_eq!(stored.stale_elements, 1);
        assert_eq!(
            element_table(&db).await,
            [
                triple("7_1", "exact", "5/1"),
                triple("7_2", "exact", "5/1"),
                triple("7_3", "exact", "5/1"),
            ]
        );

        // 整个单元核对一遍（这次没有 C）：范围写下的 B 被整单元的结果覆盖，范围标记随之消失。
        let stored = put_all(
            &db,
            &unit_run(
                "b-2",
                "5/1",
                "1/2",
                at(20),
                vec![
                    element(Some("7/1"), "exact", "/A"),
                    element(Some("7/2"), "exact", "/B"),
                ],
            ),
        )
        .await;
        assert_eq!(stored.stale_elements, 1);
        let b = elements_where(
            &db,
            ElementFilter {
                refno: Some("7/2".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert!(b[0].get("scope_kind").is_none(), "{}", b[0]);
        assert_eq!(b[0]["batch_id"], "b-2");
    }

    /// 只留**最后完成**的那一次：补入库的老结果不盖更新的行、不清更新的行，也不把后来已经清掉的件写回去。
    /// 某一步没跑完的运行（没有构件级结果）不动这张表。
    #[tokio::test]
    async fn an_older_result_never_clobbers_newer_rows() {
        let db = database("older").await;
        let t0 = chrono::Local::now();
        let at = |seconds: i64| t0 + chrono::Duration::seconds(seconds);
        put_all(
            &db,
            &unit_run("b-new", "5/1", "1/2", at(100), vec![element(Some("7/1"), "exact", "/A")]),
        )
        .await;

        // 更早的一批后来才补入库：这个单元已经有更晚的核对，构件表不动；运行行照补。
        let stored = put_all(
            &db,
            &unit_run(
                "b-old",
                "5/1",
                "1/2",
                at(0),
                vec![
                    element(Some("7/1"), "off", "/A"),
                    element(Some("7/3"), "exact", "/C"),
                ],
            ),
        )
        .await;
        assert!(stored.elements_superseded);
        assert_eq!((stored.elements, stored.stale_elements), (0, 0));
        assert_eq!(element_table(&db).await, [triple("7_1", "exact", "5/1")]);
        assert_eq!(
            stored_run_ids_on(&db, "b-old").await.unwrap().len(),
            1,
            "运行行照常补进去"
        );

        // 页面上后来单比了这个单元下面的一个节点（运行行记的是那个节点，不是单元）：A 的行比下面这次新。
        let mut page = unit_run("x", "5/1", "1/2", at(200), vec![element(Some("7/1"), "chord", "/A")]);
        page.run_id = "rvm-check-page-1".to_string();
        page.source = SOURCE_PAGE;
        page.batch_id = None;
        page.refno = "7/1".to_string();
        page.unit = None;
        page.root_refno = None;
        page.elements.as_mut().unwrap().scope = Some(ElementScope {
            kind: "subtree".to_string(),
            refno: "7/1".to_string(),
        });
        put_all(&db, &page).await;
        // 一批介于两者之间的整单元结果补进来：A 留着页面那次的（更新），它带来的 B 是 A 之外的最新信息，照写。
        let stored = put_all(
            &db,
            &unit_run(
                "b-mid",
                "5/1",
                "1/2",
                at(150),
                vec![
                    element(Some("7/1"), "off", "/A"),
                    element(Some("7/2"), "exact", "/B"),
                ],
            ),
        )
        .await;
        assert!(!stored.elements_superseded);
        assert_eq!(stored.stale_elements, 0, "比这次新的行不清");
        let rows = elements_where(
            &db,
            ElementFilter {
                unit_refno: Some("5/1".to_string()),
                ..ElementFilter::default()
            },
        )
        .await;
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|row| row["key"] == "7_1").unwrap();
        assert_eq!(a["grade"], "chord", "页面那次更新，留着");
        assert_eq!(a["run_id"], "rvm-check-page-1");
        assert_eq!(a["source"], "page");
        let b = rows.iter().find(|row| row["key"] == "7_2").unwrap();
        assert_eq!(b["batch_id"], "b-mid");

        // 没跑完的运行：不写、不清。
        let mut failed = zone_facts("b-fail", "5/1", "1/2", VERDICT_FAIL, Vec::new());
        failed.finished_at = at(300);
        failed.status = "gen-failed".to_string();
        failed.stage = "generate_ours".to_string();
        let stored = put_all(&db, &failed).await;
        assert_eq!((stored.elements, stored.stale_elements), (0, 0));
        assert_eq!(element_table(&db).await.len(), 2);
    }

    /// 入库线程带着构件行走：写了几行、清了几行都记在账上，排队的行数处理完就放掉。
    #[tokio::test]
    async fn the_stream_writes_element_rows_and_releases_the_queue() {
        let db = database("stream-elements").await;
        let t0 = chrono::Local::now();
        let (facts, mut inbox) = tokio::sync::mpsc::unbounded_channel();
        let pending = AtomicUsize::new(0);
        for (seconds, checks) in [
            (
                0,
                vec![
                    element(Some("7/1"), "exact", "/A"),
                    element(Some("7/2"), "exact", "/B"),
                ],
            ),
            (5, vec![element(Some("7/1"), "exact", "/A")]),
        ] {
            let run = unit_run(
                &format!("b-{seconds}"),
                "5/1",
                "1/2",
                t0 + chrono::Duration::seconds(seconds),
                checks,
            );
            pending.fetch_add(run.elements.as_ref().unwrap().checks.len(), Ordering::Relaxed);
            facts.send(run).unwrap();
        }
        facts
            .send(zone_facts("b-9", "5/9", "1/2", "pass", Vec::new()))
            .unwrap();
        drop(facts);
        assert_eq!(pending.load(Ordering::Relaxed), 3);

        let outcome = untimed(persist_stream(&db, &mut inbox, &pending).await);
        assert_eq!(
            outcome,
            StoreOutcome {
                runs: 3,
                elements: 3,
                stale_elements: 1,
                ..StoreOutcome::default()
            }
        );
        assert_eq!(pending.load(Ordering::Relaxed), 0);
        assert_eq!(element_table(&db).await, [triple("7_1", "exact", "5/1")]);
    }

    #[test]
    fn the_store_outcome_says_what_did_not_land() {
        let unreachable = StoreOutcome {
            connect_error: Some("连不上".to_string()),
            ..StoreOutcome::default()
        };
        assert_eq!(unreachable.into_result(), Err("连不上".to_string()));
        let partial = StoreOutcome {
            runs: 2,
            rows: 5,
            failures: vec![("b/x".to_string(), "超时".to_string())],
            ..StoreOutcome::default()
        };
        let message = partial.into_result().unwrap_err();
        assert!(
            message.contains("2 个落进去了")
                && message.contains("1 个没落进去")
                && message.contains("b/x"),
            "{message}"
        );
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
