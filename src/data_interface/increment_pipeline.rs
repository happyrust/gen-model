//! IncrementPipeline — deep module for narrow incremental persist + watermark.
//!
//! Interface: `apply(ranges_map) -> IncrResult`
//! Does NOT own model refresh or MQTT sync (callers consume `IncrResult`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aios_core::data_center::DataCenterRecordOperate;
use aios_core::pdms_types::*;
use aios_core::{RefnoEnum, SUL_DB, clear_all_caches_batch};
use indexmap::IndexMap;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement, PdmsIO};

use crate::data_interface::sesno_range::COLD_START_DB_TYPES;

const DATACENTER_VERSION: &str = "datacenter_version";

/// Meta / config DB types: persist + watermark only; no geometry model refresh.
/// Same set as cold-start eligibility ([`COLD_START_DB_TYPES`]).
pub const SYS_META_DB_TYPES: &[&str] = COLD_START_DB_TYPES;

/// One file that completed Surreal persist + watermark advance.
#[derive(Debug, Clone)]
pub struct IncrFileSuccess {
    pub path: PathBuf,
    pub dbnum: u32,
    pub start_sesno: i32,
    pub end_sesno: i32,
    /// PDMS db type (`SYST` / `DESI` / …) for downstream side-effects.
    pub db_type: String,
    /// Changed element refnos for downstream model refresh. Deduped, and free
    /// of the `None` operations that carry no change at all — every consumer
    /// resolves a generation root per entry, so a refno repeated once per
    /// session in the window was paying for that lookup once per session.
    pub changed_refnos: Vec<RefU64>,
    /// Full delta payload (MySQL / classified refresh). Kept for callers that need detail.
    pub range_eles: BTreeMap<u32, Vec<EleOperationData>>,
    /// The model work this window established, with the delivery-unit rollup
    /// behind it. Callers that need to act on the affected units (the manual
    /// run's per-unit reporting) read it here instead of resolving the rollup a
    /// second time — it can only be resolved before the window persists anyway.
    pub model_plan: crate::data_interface::model_update_plan::ModelUpdatePlan,
}

/// One file that failed before watermark advance.
#[derive(Debug, Clone)]
pub struct IncrFileError {
    pub path: PathBuf,
    pub error: String,
}

/// Result of [`IncrementPipeline::apply`]. Per-file isolation: failures do not stop siblings.
#[derive(Debug, Default, Clone)]
pub struct IncrResult {
    pub successes: Vec<IncrFileSuccess>,
    pub errors: Vec<IncrFileError>,
    /// Non-fatal side-channel issues (MySQL skipped here; datacenter warnings, etc.).
    pub warnings: Vec<String>,
}

impl IncrResult {
    pub fn all_changed_refnos(&self) -> Vec<RefU64> {
        self.successes
            .iter()
            .flat_map(|s| s.changed_refnos.iter().copied())
            .collect()
    }

    /// Refnos from successes that are not SYS meta DBs (eligible for mesh refresh).
    pub fn geometry_changed_refnos(&self) -> Vec<RefU64> {
        self.successes
            .iter()
            .filter(|s| !SYS_META_DB_TYPES.contains(&s.db_type.as_str()))
            .flat_map(|s| s.changed_refnos.iter().copied())
            .collect()
    }

    pub fn had_work(&self) -> bool {
        !self.successes.is_empty()
    }
}

/// Wrap the given SurrealQL statements into one atomic transaction: that batch
/// lands whole or not at all. A per-file window is split across several such
/// batches (see `persist_latest_main_data`), so the window itself is NOT
/// all-or-nothing — ADR-001 holds because a failed batch never advances the
/// applied watermark and the same window replays idempotently. Returns `None`
/// when there is nothing to run. Statements keep the original `;\n` separator
/// (SurrealDB tolerates the resulting empty statements).
pub(crate) fn wrap_in_transaction(statements: &[String]) -> Option<String> {
    if statements.is_empty() {
        return None;
    }
    Some(format!(
        "BEGIN TRANSACTION;\n{};\nCOMMIT TRANSACTION;",
        statements.join(";\n")
    ))
}

/// Where one key's value came from last, while folding a run of `Modified` ops.
///
/// `to_modify_surql` renders `added` and `modified` identically (the new value)
/// and `deleted` as `NULL`, so the bucket a key ends in decides its final value.
enum LastWrite<V> {
    Added(V),
    Modified((V, V)),
    Deleted(V),
}

/// Per-key last-writer-wins fold of one attribute namespace over a run of ops.
///
/// Unioning the three delta maps would be wrong: a key deleted in one session and
/// re-added in a later one would land in both `added` and `deleted`, and
/// `to_modify_surql` applies `deleted` last — silently turning the value back into
/// `NULL`. Replaying by session and keeping only the last bucket per key is what
/// makes a folded statement equivalent to the sequence it replaces.
fn fold_attr_namespace<'a, K, V, I>(
    per_op: I,
) -> (
    std::collections::HashMap<K, V>,
    std::collections::HashMap<K, (V, V)>,
    std::collections::HashMap<K, V>,
)
where
    K: Eq + std::hash::Hash + Clone + 'a,
    V: Clone + 'a,
    I: Iterator<
        Item = (
            &'a std::collections::HashMap<K, V>,
            &'a std::collections::HashMap<K, (V, V)>,
            &'a std::collections::HashMap<K, V>,
        ),
    >,
{
    use std::collections::HashMap;

    let mut last: HashMap<K, LastWrite<V>> = HashMap::new();
    for (added, modified, deleted) in per_op {
        for (k, v) in added {
            last.insert(k.clone(), LastWrite::Added(v.clone()));
        }
        for (k, v) in modified {
            last.insert(k.clone(), LastWrite::Modified(v.clone()));
        }
        for (k, v) in deleted {
            last.insert(k.clone(), LastWrite::Deleted(v.clone()));
        }
    }

    let mut added = HashMap::new();
    let mut modified = HashMap::new();
    let mut deleted = HashMap::new();
    for (k, write) in last {
        match write {
            LastWrite::Added(v) => {
                added.insert(k, v);
            }
            LastWrite::Modified(v) => {
                modified.insert(k, v);
            }
            LastWrite::Deleted(v) => {
                deleted.insert(k, v);
            }
        }
    }
    (added, modified, deleted)
}

/// Merge a run of consecutive `Modified` ops on one refno into a single op.
///
/// `current_data` and `noun` come from the newest op (it already carries the full
/// post-state); the delta maps are folded per key; `children_changed` keeps the
/// oldest `old` and the newest `new` so the pair still spans the whole run — only
/// the `new` side is rendered, and the `DELETE … <-pe_owner` + re-`INSERT` it
/// drives is a full replace, so the newest child list is the correct one.
fn fold_modified_run(run: &[&ModifiedElement]) -> Option<ModifiedElement> {
    let last = run.last()?;
    if run.len() == 1 {
        return Some((*last).clone());
    }

    let mut folded = (*last).clone();

    let (added, modified, deleted) = fold_attr_namespace(
        run.iter()
            .map(|e| (&e.added_attrs, &e.modified_attrs, &e.deleted_attrs)),
    );
    folded.added_attrs = added;
    folded.modified_attrs = modified;
    folded.deleted_attrs = deleted;

    let (added, modified, deleted) = fold_attr_namespace(run.iter().map(|e| {
        (
            &e.added_explicit_attrs,
            &e.modified_explicit_attrs,
            &e.deleted_explicit_attrs,
        )
    }));
    folded.added_explicit_attrs = added;
    folded.modified_explicit_attrs = modified;
    folded.deleted_explicit_attrs = deleted;

    let (added, modified, deleted) = fold_attr_namespace(run.iter().map(|e| {
        (
            &e.added_uda_attrs,
            &e.modified_uda_attrs,
            &e.deleted_uda_attrs,
        )
    }));
    folded.added_uda_attrs = added;
    folded.modified_uda_attrs = modified;
    folded.deleted_uda_attrs = deleted;

    let oldest = run
        .iter()
        .find_map(|e| e.children_changed.as_ref().map(|(old, _)| old.clone()));
    let newest = run
        .iter()
        .rev()
        .find_map(|e| e.children_changed.as_ref().map(|(_, new)| new.clone()));
    folded.children_changed = match (oldest, newest) {
        (Some(old), Some(new)) => Some((old, new)),
        _ => None,
    };

    Some(folded)
}

/// One operation to render, after the window has been folded.
struct PlannedWrite<'a> {
    sesno: u32,
    op: &'a EleOperationData,
    /// `Some` when this position stands in for a whole run of `Modified` ops.
    folded: Option<ModifiedElement>,
}

/// Collapse a window so each refno is written once per run of consecutive
/// `Modified` operations.
///
/// This module keeps only the latest state (no `sessions` / `element_changes`
/// history), so replaying every intermediate session write is pure overhead: a
/// refno modified N times in one window emitted N `UPSERT … MERGE` + N `UPDATE pe`
/// pairs that the last one overwrote anyway. Measured on the amssys cold-start
/// window (169 sessions, 4635 operations over 2148 refnos) this removes ~31% of
/// the statements and a matching share of the ~17 MB of SurrealQL.
///
/// Deliberately conservative:
/// * only a *consecutive* run of `Modified` on one refno collapses, so the
///   create-then-tombstone ordering of `Add` / `Deleted` is untouched;
/// * the merged statement is emitted at the position of the run's LAST operation,
///   so the global statement order is preserved. No generated statement ever reads
///   another record (every value is a literal), so dropping intermediate writes
///   cannot change what a later statement sees.
fn fold_window(range_eles: &BTreeMap<u32, Vec<EleOperationData>>) -> Vec<PlannedWrite<'_>> {
    use std::collections::HashMap;

    let mut flat: Vec<PlannedWrite<'_>> = Vec::new();
    for (&sesno, elements) in range_eles {
        for op in elements {
            flat.push(PlannedWrite {
                sesno,
                op,
                folded: None,
            });
        }
    }

    let mut positions: HashMap<RefU64, Vec<usize>> = HashMap::new();
    for (i, planned) in flat.iter().enumerate() {
        positions.entry(planned.op.refno).or_default().push(i);
    }

    // Collect the runs before folding: folding writes back into `flat`, so it can
    // no longer hold the immutable borrow that run detection needs.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for idxs in positions.values() {
        let mut run: Vec<usize> = Vec::new();
        for &i in idxs {
            if matches!(flat[i].op.detail, EleOperationDetail::Modified(_)) {
                run.push(i);
            } else if run.len() > 1 {
                runs.push(std::mem::take(&mut run));
            } else {
                run.clear();
            }
        }
        if run.len() > 1 {
            runs.push(run);
        }
    }

    let mut dropped = vec![false; flat.len()];
    for run in runs {
        let folded = {
            let members: Vec<&ModifiedElement> = run
                .iter()
                .filter_map(|&i| match &flat[i].op.detail {
                    EleOperationDetail::Modified(m) => Some(m),
                    _ => None,
                })
                .collect();
            fold_modified_run(&members)
        };
        let (&last_idx, earlier) = run.split_last().expect("a run holds at least two entries");
        for &i in earlier {
            dropped[i] = true;
        }
        flat[last_idx].folded = folded;
    }

    flat.into_iter()
        .zip(dropped)
        .filter_map(|(planned, drop)| (!drop).then_some(planned))
        .collect()
}

/// Wall time of each stage of one file's window, reported as a single line so a
/// slow stage can be attributed without attaching a profiler to a live run.
#[derive(Debug, Default, Clone, Copy)]
struct StageTimings {
    collect: Duration,
    plan: Duration,
    persist: Duration,
    cache: Duration,
    reverse_index: Duration,
    datacenter: Duration,
    finalize: Duration,
}

impl StageTimings {
    async fn measure<T>(slot: &mut Duration, work: impl Future<Output = T>) -> T {
        let start = Instant::now();
        let value = work.await;
        *slot += start.elapsed();
        value
    }

    fn report(&self, dbnum: u32, db_type: &str, elements: usize) {
        println!(
            "IncrementPipeline 阶段耗时 dbnum={dbnum} db_type={db_type} 元素={elements}: \
             collect={}ms plan={}ms persist={}ms cache={}ms rev_index={}ms \
             datacenter={}ms finalize={}ms",
            self.collect.as_millis(),
            self.plan.as_millis(),
            self.persist.as_millis(),
            self.cache.as_millis(),
            self.reverse_index.as_millis(),
            self.datacenter.as_millis(),
            self.finalize.as_millis(),
        );
    }
}

/// Independent deep module: collect delta → Surreal persist → datacenter meta → watermark by dbnum.
#[derive(Debug, Default, Clone)]
pub struct IncrementPipeline;

fn same_attempt_file_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }

    #[cfg(windows)]
    {
        let normalize = |path: &str| path.replace('/', "\\").to_ascii_lowercase();
        normalize(left) == normalize(right)
    }

    #[cfg(not(windows))]
    {
        match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

fn validate_prepared_attempt(
    attempt: &crate::data_interface::model_update_pending::IncrementUpdateAttempt,
    db_type: &str,
    file_path: &str,
    current_file_latest_sesno: i32,
) -> anyhow::Result<()> {
    if attempt.db_type != db_type || !same_attempt_file_path(&attempt.file_path, file_path) {
        anyhow::bail!(
            "unfinished increment attempt dbnum={} belongs to type={} path={}, \
             current type={db_type} path={file_path}",
            attempt.dbnum,
            attempt.db_type,
            attempt.file_path
        );
    }
    if attempt.end_sesno > current_file_latest_sesno {
        anyhow::bail!(
            "unfinished increment attempt dbnum={} requires sesno {}..={}, \
             but current file only covers through {current_file_latest_sesno}; \
             file rollback/replacement is blocked",
            attempt.dbnum,
            attempt.start_sesno,
            attempt.end_sesno,
        );
    }
    Ok(())
}

/// 过时的暂存恢复记录要不要并入新会话、按全区间重建计划（而不是原样重放）？
///
/// 文件已经走在恢复记录前面（`attempt_end_sesno < requested_end_sesno`）时，暂存
/// 模式下必须重建。不并的代价是死循环——窗口停在 25、文件已到 26，25 里还活着的
/// 元素被 26 删掉之后，祖先解析必然断在它身上，每次重试都在重演同一幕；现场
/// dbnum=8000 就这么卡了三轮，直到人手工删掉记录才过去。
///
/// **重建安全不是因为「持久层一个字都没落」**——写回并非单事务：
/// `staging::executor::StagedExecutor::commit_to` 先按 `TX_CHUNK` 分块重放
/// journal（每块各自一个事务），之后才跑尾事务，所以崩在某一块之后、尾事务之前
/// 会留下半提交行。安全的真正来源是写回计划 T4.1
/// （`docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md`）：journal
/// 只活在内存，进程一崩它就没了，唯一恢复路径是整窗口重算，重算的 regen 删除集
/// 覆盖先前的半提交行、幂等收敛。
///
/// 「整窗口」这件事是结构上成立的，不是巧合：请求区间左端取 `applied_sesno + 1`，
/// 而水位推进与恢复记录的删除同在一条尾事务里（`render_finalize_tail_with_effects`）。
/// 记录还在 ⇒ 水位一定没动过 ⇒ 重建出来的区间必然从老记录那个 `start_sesno` 起步，
/// 半提交的那几个会话一个都跑不掉。
///
/// 原样重放在直写模式下是对的：那条路上 PE 块可能已经写了一半而水位故意没动，
/// 而它没有 journal、也没有「整窗口重算」这条退路，只能照先前备好的计划走。
///
/// 判据取「**这一次**是不是跑在暂存窗口里」（`in_staged_window`），而不是进程级的
/// increment_mode：基线（start_sesno == 1）即便进程是 staged 也走直写，问的是进程
/// 就会答错。
///
/// 恢复记录**超前**于文件（回退/换文件）不归它管：返回 false 落回重放分支，由
/// [`validate_prepared_attempt`] 拒绝并给出人话诊断。
fn should_rebuild_stale_staged_attempt(
    attempt_end_sesno: Option<i32>,
    requested_end_sesno: i32,
    in_staged_window: bool,
) -> bool {
    in_staged_window && attempt_end_sesno.is_some_and(|end| end < requested_end_sesno)
}

/// 交入窗口的采信判定（纯函数）：调用方交出的预收集结果只有与本次要应用的区间
/// **完全一致**才复用；错位区间（重叠、错一格、方向反了）与没交都回退自行收集。
/// 部分重叠也拒绝——增量窗口是按「水位+1..=文件最新」整体应用的，掐头去尾的
/// 子集会让折叠与影响判定漏看会话。崩溃重放分支根本不询问本函数：它按持久化的
/// 固定区间重新收集（见 `apply_one` 的 prepared 分支）。
fn accept_handed_in_window(
    precollected: Option<(RangeInclusive<i32>, BTreeMap<u32, Vec<EleOperationData>>)>,
    requested_range: &RangeInclusive<i32>,
) -> Option<BTreeMap<u32, Vec<EleOperationData>>> {
    match precollected {
        Some((range, eles)) if &range == requested_range => Some(eles),
        _ => None,
    }
}

/// 启动自检：收口事务依赖的 SurrealDB 自定义函数在不在。
///
/// 历史背景：datacenter 语句曾渲出 `fn::find_ancestor_types(...)` 在收口事务里
/// 现场上溯，而 A3 把它并进了收口——「可失败的副作用」变成「水位推进的必要
/// 条件」，函数漏灌 = 每个 DESI 窗口收口必败（issue #16）。W3/W4 之后 journal
/// 已纯数据化、datacenter 语句是固定目标 UPDATE；收口里剩余的 `fn::` 硬依赖只有
/// OWNER 搬迁重算的 `fn::anc_u64`，探针已对准它
/// （W6 审计：`docs/2026-08-07_journal-fn-dependency-audit.md`；P3 退役
/// `zone_refno` 后 `fn::find_ancestor_type` 已彻底离开收口链）。
///
/// 这些函数定义在 `resource/surreal/common.surql`；启动序列会从 **exe 工作
/// 目录**的 `resource/surreal/` 整目录灌一遍（`define_common_functions`，日志
/// 「载入surreal …」），所以函数在不在，取决于部署包带的脚本版本与启动时的工作
/// 目录。脚本旧了/目录不对，函数照样缺，而缺失时收口的错误信息长成
/// `finalize increment attempt dbnum=… statement failed: …`，排查的人会先去翻
/// 水位和模型队列。
///
/// `aios_core` 会在旧版部署脚本缺少 `fn::anc_u64` 时从二进制内置定义补装；这里紧接
/// 加载阶段验证这个最终硬依赖。缺失或探针本身失败都阻止启动，避免服务看似健康、
/// 第一个 DESI 窗口却在收口阶段确定性失败。
pub async fn selfcheck_surreal_functions() -> anyhow::Result<()> {
    match desi_finalize_preflight().await {
        FinalizePreflight::Ready => Ok(()),
        FinalizePreflight::Missing(reason) | FinalizePreflight::Unverified(reason) => {
            anyhow::bail!(
                "SurrealDB 公共函数启动自检未通过：{reason}。服务未启动，避免 DESI 批次\
                 在收口阶段失败或 applied_sesno 停滞"
            )
        }
    }
}

/// DESI 收口预检的裁决。
pub enum FinalizePreflight {
    /// 硬前置在场，放行。
    Ready,
    /// 硬前置确定性缺失（函数没灌进当前库或被人删了）。批次应当即刻失败——
    /// 拖到写回阶段只会先白跑整个窗口（房间预载、目录闭包、模型重生成），
    /// 再一头扎进无限重试。
    Missing(String),
    /// 探针自己没跑成（连接一类的临时故障）。不据此定罪：照常执行，
    /// 写回路径的重试自己会兜住真正的持久层故障。
    Unverified(String),
}

/// 批次执行前探一次 DESI 收口/写回的 `fn::` 前置（与
/// [`selfcheck_surreal_functions`] 同一根探针）。
///
/// issue #16 的教训：前置缺失时写回对持久层确定性失败，`retry_until_recovered`
/// 无限重试而控制台一个字都不说——水位不动、重启重放同一区间、模型永远不更新。
/// 预检把「确定性缺失」提前到开窗之前，换成一条带修法的人话终态。
///
/// 探针对象随 W6 审计（`docs/2026-08-07_journal-fn-dependency-audit.md`）对准
/// **剩余的收口硬依赖**：W3/W4 之后 journal 已纯数据化、datacenter 语句是固定
/// 目标 UPDATE，收口里唯一还对持久层求值 `fn::` 的是 OWNER 搬迁的
/// anc 定点重算（`render_anc_repair_statements`）——用的是 `fn::anc_u64`，
/// 定义在 `resource/surreal/common.surql`；旧版脚本（没有 P1 新增的
/// `fn::anc_u64`）现在会被正确拒绝，而不是等到含搬迁的窗口在写回里无限重试。
/// （`fn::find_ancestor_type` 曾同为硬前置，P3 退役 `zone_refno` 后离开收口链，
/// 探针随之收窄。）
pub async fn desi_finalize_preflight() -> FinalizePreflight {
    const PROBE: &str = "RETURN fn::anc_u64(type::thing('pe','__finalize_preflight__'));";
    match aios_core::SUL_DB.query(PROBE).await {
        Err(error) => FinalizePreflight::Unverified(format!("探针查询未送达持久层（{error}）")),
        Ok(response) => match response.check() {
            Ok(_) => FinalizePreflight::Ready,
            Err(error) => FinalizePreflight::Missing(format!(
                "调用 fn::anc_u64 失败（{error}）。它是收口事务里 OWNER 搬迁重算的\
                 硬前置，定义在 resource/surreal/common.surql\
                 ——脚本没灌进当前库或版本太旧（缺 P1 新增的 fn::anc_u64）"
            )),
        },
    }
}

/// 逐条语句的保尾去重：同一条语句被渲染多次时只保留**最后一次出现**，其余全是
/// 纯重复（2026-08-10 审核 P1）。
///
/// datacenter 语句按窗口内的**原始 op** 逐条渲染：一个元素在窗口里被改 N 次就
/// 渲染 N 条一模一样的 `update datacenter_version:… set status = 'Modify';`——
/// 主数据那侧有 `fold_window` 折叠，这一侧没有，于是收口体积 ∝ 操作数而不是
/// ∝ 元素数。这些语句都是自含的单行赋值：删掉一条较早的重复，不改变它那一行
/// 的终值（终值由保留下来的最后一次出现决定），也碰不到别的行。
///
/// **必须保尾而不是保头**：同一目标行上可能交错出现两种不同语句（改 → 删 →
/// 重建再改，渲成 M、D、M），保头会把最后那条 M 去掉、终态错成 Delete；保尾
/// 得到 D、M，与逐条重放同一个终态。
fn dedup_statements_keep_last(statements: Vec<String>) -> Vec<String> {
    let mut remaining: HashMap<&str, usize> = HashMap::new();
    for statement in &statements {
        *remaining.entry(statement.as_str()).or_default() += 1;
    }
    let keep = statements
        .iter()
        .map(|statement| {
            let count = remaining
                .get_mut(statement.as_str())
                .expect("counted above");
            *count -= 1;
            *count == 0
        })
        .collect::<Vec<_>>();
    statements
        .into_iter()
        .zip(keep)
        .filter_map(|(statement, keep)| keep.then_some(statement))
        .collect()
}

/// PDMS session logs may contain provisional `Add` operations whose records are
/// no longer present when Save Work publishes the final file. Treating those
/// entries as live creates phantom PE rows and makes file-backed ancestor
/// preload chase refnos which the final record index cannot resolve.
fn retain_finally_live_adds(
    range_eles: &mut BTreeMap<u32, Vec<EleOperationData>>,
    mut is_live: impl FnMut(RefU64) -> bool,
) -> usize {
    let before = range_eles.values().map(Vec::len).sum::<usize>();
    for ops in range_eles.values_mut() {
        ops.retain(|op| !matches!(&op.detail, EleOperationDetail::Add(_)) || is_live(op.refno));
    }
    before - range_eles.values().map(Vec::len).sum::<usize>()
}

/// A PDMS owner move can leave a `Deleted` entry in the session stream even
/// though the same refno is present in the Save Work final index.  Persisting
/// that provisional delete marks the live element deleted and loses its final
/// OWNER.  Replace only those exact-window, finally-live deletes with a full
/// final-record upsert; genuine deletes (absent from the final index) remain
/// untouched.
fn restore_finally_live_deletes(
    range_eles: &mut BTreeMap<u32, Vec<EleOperationData>>,
    final_elements: &HashMap<RefU64, parse_pdms_db::parse::EleData>,
) -> usize {
    let mut restored = 0;
    for operations in range_eles.values_mut() {
        for operation in operations {
            if !matches!(operation.detail, EleOperationDetail::Deleted) {
                continue;
            }
            let Some(element) = final_elements.get(&operation.refno) else {
                continue;
            };
            operation.detail = EleOperationDetail::Add(element.clone());
            restored += 1;
        }
    }
    restored
}

/// `PagedDbSession::read_raw_records` returns the physical record, which may
/// start with page padding/continuation words.  The element parser expects the
/// declared implicit-length word.  Keep this boundary check equivalent to the
/// paged parser instead of handing it an unbounded file tail.
fn final_record_payload(raw: &[u8]) -> anyhow::Result<&[u8]> {
    let mut prefix = 0usize;
    while prefix + 4 <= raw.len() {
        let word = &raw[prefix..prefix + 4];
        if word == [0, 0, 0, 0] || word == [0, 0, 0, 7] {
            prefix += 4;
        } else {
            break;
        }
    }
    anyhow::ensure!(raw.len().saturating_sub(prefix) >= 24, "元素记录长度不足");
    let impl_words = i32::from_be_bytes(
        raw[prefix..prefix + 4]
            .try_into()
            .map_err(|_| anyhow::anyhow!("读取 impl_len 失败"))?,
    );
    anyhow::ensure!(impl_words > 0, "impl_len 非法: {impl_words}");
    anyhow::ensure!(
        (impl_words as usize).saturating_mul(4) <= raw.len() - prefix,
        "impl_len 超出记录边界: words={impl_words}, bytes={}",
        raw.len() - prefix
    );
    Ok(&raw[prefix..])
}

fn retain_finally_live_design_refnos(
    plan: &mut crate::data_interface::model_update_plan::ModelUpdatePlan,
    mut is_live: impl FnMut(RefU64) -> bool,
) -> usize {
    let before = plan.design_refnos.len();
    plan.design_refnos.retain(|raw| {
        let refno = RefnoEnum::from(raw.as_str());
        refno.is_valid() && is_live(refno.refno())
    });
    before - plan.design_refnos.len()
}

fn reconcile_plan_with_live_set(
    plan: &mut crate::data_interface::model_update_plan::ModelUpdatePlan,
    live: &std::collections::HashSet<RefU64>,
) -> usize {
    use crate::data_interface::model_update_plan::ModelWorkAction;

    let mut removed = retain_finally_live_design_refnos(plan, |refno| live.contains(&refno));
    for unit in &mut plan.units {
        if !unit.will_generate {
            continue;
        }
        let refno = RefnoEnum::from(unit.root_refno.as_str());
        if !refno.is_valid() || !live.contains(&refno.refno()) {
            unit.will_generate = false;
            removed += 1;
        }
    }
    let before = plan.work_items.len();
    plan.work_items.retain(|item| {
        if !matches!(
            item.action,
            ModelWorkAction::RegenRoot | ModelWorkAction::Transform
        ) {
            return true;
        }
        let refno = RefnoEnum::from(item.target_refno.as_str());
        refno.is_valid() && live.contains(&refno.refno())
    });
    removed + before - plan.work_items.len()
}

fn reconcile_plan_final_presence(
    path: &std::path::Path,
    end_sesno: i32,
    plan: &mut crate::data_interface::model_update_plan::ModelUpdatePlan,
) -> anyhow::Result<usize> {
    use crate::data_interface::model_update_plan::ModelWorkAction;

    let mut candidates = plan
        .design_refnos
        .iter()
        .map(|raw| RefnoEnum::from(raw.as_str()))
        .filter(|refno| refno.is_valid())
        .map(|refno| refno.refno())
        .collect::<Vec<_>>();
    candidates.extend(
        plan.units
            .iter()
            .filter(|unit| unit.will_generate)
            .map(|unit| RefnoEnum::from(unit.root_refno.as_str()))
            .filter(|refno| refno.is_valid())
            .map(|refno| refno.refno()),
    );
    candidates.extend(
        plan.work_items
            .iter()
            .filter(|item| {
                matches!(
                    item.action,
                    ModelWorkAction::RegenRoot | ModelWorkAction::Transform
                )
            })
            .map(|item| RefnoEnum::from(item.target_refno.as_str()))
            .filter(|refno| refno.is_valid())
            .map(|refno| refno.refno()),
    );
    candidates.sort_unstable();
    candidates.dedup();
    if candidates.is_empty() {
        return Ok(retain_finally_live_design_refnos(plan, |_| false));
    }
    let mut final_file = parse_pdms_db::paged::PagedDbSession::open(path)
        .map_err(|error| anyhow::anyhow!("打开 Save Work 最终页式索引失败: {error:#}"))?;
    if final_file.snapshot().sesno != end_sesno as u32 {
        return Ok(0);
    }
    let live = final_file
        .read_raw_records(&candidates)
        .map_err(|error| anyhow::anyhow!("读取模型计划最终记录存在性失败: {error:#}"))?
        .into_keys()
        .collect::<std::collections::HashSet<_>>();
    Ok(reconcile_plan_with_live_set(plan, &live))
}

impl IncrementPipeline {
    pub fn new() -> Self {
        Self
    }

    /// 把解析产物写进活动窗口，并把同一批主数据/ref_rev语句收入唯一 journal。
    /// 此阶段刻意不生成收口语句：水位与 pending 只能在模型生成也成功后提交。
    pub(crate) async fn stage_parsed_window(
        window: &mut crate::data_interface::staging::ActiveStagedWindow,
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        dbnum: u32,
    ) -> anyhow::Result<usize> {
        use crate::data_interface::staging::ExecMode;

        window.activate().await?;
        let statements = Self::render_persist_statements(range_eles, dbnum as i32)
            .into_iter()
            .chain(crate::data_interface::manual_update::build_reverse_index_statements(range_eles))
            .collect::<Vec<_>>();
        for sql in &statements {
            window.execute(sql, ExecMode::Both).await?;
        }
        Ok(statements.len())
    }

    /// Side-effect-free change collection for one file over a sesno range.
    ///
    /// Opens the E3D file and returns the per-`sesno` element operations WITHOUT
    /// persisting anything (no `pe` writes, no datacenter meta, no watermark
    /// advance). Shared by the apply path ([`Self::apply_one`]) and the read-only
    /// manual-update preview so the two cannot diverge.
    pub fn collect_changes(
        path: &std::path::Path,
        sesno_range: RangeInclusive<i32>,
    ) -> anyhow::Result<BTreeMap<u32, Vec<EleOperationData>>> {
        let fixed_end_sesno = *sesno_range.end();
        let mut io = PdmsIO::new("", path.to_path_buf(), true);
        io.open()
            .map_err(|e| anyhow::anyhow!("打开 PDMS IO 失败: {}", e))?;
        let mut range_eles = io.collect_increment_eles(Some(sesno_range))?;
        let add_refnos = range_eles
            .values()
            .flatten()
            .filter(|op| matches!(&op.detail, EleOperationDetail::Add(_)))
            .map(|op| op.refno)
            .collect::<Vec<_>>();
        let deleted_refnos = range_eles
            .values()
            .flatten()
            .filter(|op| matches!(&op.detail, EleOperationDetail::Deleted))
            .map(|op| op.refno)
            .collect::<Vec<_>>();
        let mut final_candidates = add_refnos.clone();
        final_candidates.extend(deleted_refnos.iter().copied());
        final_candidates.sort_unstable();
        final_candidates.dedup();

        let final_state = if final_candidates.is_empty() {
            None
        } else {
            let authoritative_sesno = io
                .get_latest_sesno()
                .map_err(|error| anyhow::anyhow!("读取 Save Work 最终会话号失败: {error:#}"))?;
            if authoritative_sesno != fixed_end_sesno as u32 {
                println!(
                    "增量最终文件对账跳过：窗口末 sesno={fixed_end_sesno}，最终文件 sesno={authoritative_sesno}，path={}",
                    path.display()
                );
                None
            } else {
                let mut final_file =
                    parse_pdms_db::paged::PagedDbSession::open(path).map_err(|error| {
                        anyhow::anyhow!("打开 Save Work 最终页式索引失败: {error:#}")
                    })?;
                let final_snapshot_sesno = final_file.snapshot().sesno;
                if final_snapshot_sesno == fixed_end_sesno as u32 {
                    let final_records =
                        final_file
                            .read_raw_records(&final_candidates)
                            .map_err(|error| {
                                anyhow::anyhow!("读取 Save Work 最终记录存在性失败: {error:#}")
                            })?;
                    let live_refnos = final_records.keys().copied().collect::<HashSet<_>>();
                    let mut final_deleted_elements = HashMap::new();
                    for refno in &deleted_refnos {
                        let Some(raw) = final_records.get(refno) else {
                            continue;
                        };
                        let payload = final_record_payload(raw).map_err(|error| {
                            anyhow::anyhow!(
                                "读取 Save Work 最终存活记录 {} 边界失败: {error:#}",
                                RefnoEnum::from(*refno).to_pdms_str()
                            )
                        })?;
                        let element =
                            parse_pdms_db::parse::parse_raw_ele_data(payload).map_err(|error| {
                                anyhow::anyhow!(
                                    "解析 Save Work 最终存活记录 {} 失败: {error:#}",
                                    RefnoEnum::from(*refno).to_pdms_str()
                                )
                            })?;
                        anyhow::ensure!(
                            element.refno == *refno,
                            "Save Work 最终记录 refno 不匹配: 期望 {}, 实际 {}",
                            RefnoEnum::from(*refno).to_pdms_str(),
                            RefnoEnum::from(element.refno).to_pdms_str()
                        );
                        final_deleted_elements.insert(*refno, element);
                    }
                    Some((live_refnos, final_deleted_elements))
                } else {
                    println!(
                        "增量最终页式索引失配，回退 legacy 最终索引：窗口末 sesno={fixed_end_sesno}，paged sesno={final_snapshot_sesno}，path={}",
                        path.display()
                    );
                    let live_refnos = final_candidates
                        .iter()
                        .copied()
                        .filter(|refno| io.search_latest_refno(*refno, None).is_some())
                        .collect::<HashSet<_>>();
                    let mut final_deleted_elements = HashMap::new();
                    for refno in deleted_refnos.iter().copied() {
                        if !live_refnos.contains(&refno) {
                            continue;
                        }
                        let element = io.auto_get_raw_element(refno).map_err(|error| {
                            anyhow::anyhow!(
                                "legacy 解析 Save Work 最终存活记录 {} 失败: {error:#}",
                                RefnoEnum::from(refno).to_pdms_str()
                            )
                        })?;
                        final_deleted_elements.insert(refno, element);
                    }
                    Some((live_refnos, final_deleted_elements))
                }
            }
        };

        if let Some((live_refnos, final_deleted_elements)) = final_state {
            let removed =
                retain_finally_live_adds(&mut range_eles, |refno| live_refnos.contains(&refno));
            if removed > 0 {
                println!(
                    "增量窗口剔除 {removed} 条 Save Work 后无最终记录的临时 Add: {}",
                    path.display()
                );
            }

            let restored = restore_finally_live_deletes(&mut range_eles, &final_deleted_elements);
            if restored > 0 {
                println!(
                    "增量窗口恢复 {restored} 条 Save Work 后仍存活的临时 Deleted: {}",
                    path.display()
                );
            }
        }
        Ok(range_eles)
    }

    /// Apply incremental updates for the given sesno ranges.
    ///
    /// Map value: `(basic_info, sesno_range, db_type)`.
    ///
    /// - Skips copy files whose name contains `-`
    /// - On per-file failure: records error, continues
    /// - Watermark advances only after Surreal persist succeeds for that file
    /// - Watermark key is **dbnum** (dedicated `dbnum_watermark:{dbnum}` record)
    pub async fn apply(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>, String)>,
    ) -> IncrResult {
        self.apply_with_precollected(increment_ranges_map, IndexMap::new())
            .await
    }

    /// 同 [`Self::apply`]，但允许调用方交出**已经收集好**的增量窗口，避免同一个文件
    /// 被完整解析两次。
    ///
    /// 背景：`manual_update::execute_one_dbnum` 为了算 `changed_elements` 和 DESI 的
    /// 单元归并，会先 `collect_changes` 一次；随后 `apply_one` 在 fresh 分支里又收集
    /// 了一遍同一文件、同一窗口。非 DESI 库（SYST/CATA/DICT）尤其亏——第一趟的整份
    /// 结果只被用来算两个标量。实测 dbnum=250206 单趟就要 5 分多钟。
    ///
    /// 交入的结果**仅在恰好覆盖本次要应用的区间时**才被采信：崩溃重放走的是持久化
    /// 的固定区间，可能与 `requested_range` 不同，那种情况永远重新收集。
    pub async fn apply_with_precollected(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>, String)>,
        mut precollected: IndexMap<
            PathBuf,
            (RangeInclusive<i32>, BTreeMap<u32, Vec<EleOperationData>>),
        >,
    ) -> IncrResult {
        let mut result = IncrResult::default();

        for (path, (basic_info, sesno_range, db_type)) in increment_ranges_map {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();

            if !crate::data_interface::increment_manager::is_pdms_db_file_name(file_name) {
                result
                    .warnings
                    .push(format!("skip copy file: {}", path.display()));
                continue;
            }

            let handed_in = precollected.shift_remove(&path);

            match self
                .apply_one(&path, &basic_info, sesno_range, &db_type, handed_in)
                .await
            {
                Ok((success, warnings)) => {
                    result.warnings.extend(warnings);
                    result.successes.push(success);
                }
                Err(e) => {
                    result.errors.push(IncrFileError {
                        path,
                        error: e.to_string(),
                    });
                }
            }
        }

        result
    }

    async fn apply_one(
        &self,
        path: &PathBuf,
        basic_info: &DbPageBasicInfo,
        requested_range: RangeInclusive<i32>,
        db_type: &str,
        precollected: Option<(RangeInclusive<i32>, BTreeMap<u32, Vec<EleOperationData>>)>,
    ) -> anyhow::Result<(IncrFileSuccess, Vec<String>)> {
        let mut warnings = Vec::new();
        let mut timings = StageTimings::default();
        let dbnum = basic_info.pdms_header.db_num as u32;
        let path_text = path.to_string_lossy().into_owned();

        // A crash may leave PE chunks partially applied while the watermark is
        // intentionally unchanged. In that case the pre-update OWNER graph is
        // no longer trustworthy, so reuse the durable fixed range + model plan
        // prepared before the first write.
        let prepared = crate::data_interface::model_update_pending::load_attempt(dbnum).await?;
        // 文件已经走在这条恢复记录前面时，暂存模式下要**并掉新会话重建计划**，而
        // 不是原样重放；判据与它为什么安全见 `should_rebuild_stale_staged_attempt`。
        //
        // 判据问的是「**这一次**是不是跑在暂存窗口里」，而不是进程级的
        // increment_mode：基线（start_sesno == 1）即便进程是 staged 也走直写，
        // 问的是进程就会答错。
        let merge_newer_sessions = should_rebuild_stale_staged_attempt(
            prepared.as_ref().map(|attempt| attempt.end_sesno),
            *requested_range.end(),
            crate::data_interface::staging::active_staging_writes().is_some(),
        );
        if merge_newer_sessions {
            let attempt = prepared.as_ref().expect("guarded above");
            warnings.push(format!(
                "dbnum={dbnum}: 恢复记录停在 {}..={}，文件已到 {}；本次按整窗口重算并入新会话，\
                 上一轮写回若留下半提交行由本次的删除集覆盖",
                attempt.start_sesno,
                attempt.end_sesno,
                *requested_range.end()
            ));
        }
        let prepared = prepared.filter(|_| !merge_newer_sessions);
        let (sesno_range, mut model_plan, collected) = if let Some(attempt) = prepared {
            validate_prepared_attempt(&attempt, db_type, &path_text, *requested_range.end())?;
            warnings.push(format!(
                "dbnum={dbnum}: replay unfinished range {}..={} after an interrupted persist",
                attempt.start_sesno, attempt.end_sesno
            ));
            (attempt.start_sesno..=attempt.end_sesno, attempt.plan, None)
        } else {
            // 采信判定见 `accept_handed_in_window`；复用时 `timings.collect`
            // 自然为 0，收集成本记在调用方那一侧。
            let range_eles = match accept_handed_in_window(precollected, &requested_range) {
                Some(eles) => eles,
                None => {
                    let start = Instant::now();
                    let eles = Self::collect_changes(path, requested_range.clone())?;
                    timings.collect += start.elapsed();
                    eles
                }
            };

            let end_sesno = *requested_range.end();
            let model_plan = StageTimings::measure(
                &mut timings.plan,
                crate::data_interface::model_update_plan::build_model_update_plan(
                    dbnum,
                    end_sesno,
                    db_type,
                    &range_eles,
                ),
            )
            .await?;
            StageTimings::measure(
                &mut timings.plan,
                crate::data_interface::model_update_pending::prepare_attempt(
                    &crate::data_interface::model_update_pending::IncrementUpdateAttempt {
                        dbnum,
                        db_type: db_type.to_string(),
                        file_path: path_text,
                        start_sesno: *requested_range.start(),
                        end_sesno,
                        plan: model_plan.clone(),
                    },
                ),
            )
            .await?;
            (requested_range, model_plan, Some(range_eles))
        };
        let start_sesno = *sesno_range.start();
        let end_sesno = *sesno_range.end();

        println!(
            "IncrementPipeline: {:?}, db_type={}, sesno range: {:?}",
            path, db_type, &sesno_range
        );

        // Recovery recollects the durable fixed range; a fresh attempt reuses
        // the collection that produced its pre-update model plan.
        let range_eles = match collected {
            Some(range_eles) => range_eles,
            None => {
                let start = Instant::now();
                let range_eles = Self::collect_changes(path, sesno_range)?;
                timings.collect += start.elapsed();
                range_eles
            }
        };
        let removed_plan_refnos = reconcile_plan_final_presence(path, end_sesno, &mut model_plan)?;
        if removed_plan_refnos > 0 {
            warnings.push(format!(
                "dbnum={dbnum}: 从持久模型计划收敛 {removed_plan_refnos} 个 Save Work 后不存在的设计目标"
            ));
        }
        let mut cache_refnos = Self::collect_cache_invalidation_refnos(&range_eles);
        // 生成根级失效（ADR-010 残余关闭）：`QUERY_DEEP_CHILDREN_REFNOS` 按子树根
        // 为键，「变更元素 + 属主」的失效集够不着深层后代之上的高层根，同根下一次
        // 重生成会拿旧成员表静默漏算。计划层刚算出生成根，失效按根补齐；暂存路径
        // 同一份集合随提交 / 废弃时机清（`commit_registered_to` / `drop_database`）。
        cache_refnos.extend(model_plan.regen_root_refnos());
        warnings.extend(model_plan.warnings.iter().cloned());
        let staged = crate::data_interface::staging::active_staging_writes();
        let staged_cache_refnos = staged
            .is_some()
            .then(|| cache_refnos.iter().copied().collect::<Vec<_>>());

        // 只保留最新数据：仅写入 pe 主数据（最新状态），不再写 sessions / element_changes 历史表
        //
        // Cache invalidation must run after every attempted persist, including a
        // partially failed batch: earlier Surreal statements may already have
        // changed data even though the watermark must remain unchanged.
        let persist_result = if let Some(context) = staged.as_ref() {
            let statements = Self::render_persist_statements(&range_eles, dbnum as i32)
                .into_iter()
                .chain(
                    crate::data_interface::manual_update::build_reverse_index_statements(
                        &range_eles,
                    ),
                )
                .collect::<Vec<_>>();
            StageTimings::measure(&mut timings.persist, async {
                for sql in statements {
                    context
                        .execute(sql, crate::data_interface::staging::ExecMode::Both)
                        .await?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
        } else {
            StageTimings::measure(
                &mut timings.persist,
                Self::persist_latest_main_data(&range_eles, dbnum as i32),
            )
            .await
        };
        let invalidated = if staged.is_some() {
            0
        } else {
            StageTimings::measure(&mut timings.cache, Self::invalidate_caches(cache_refnos)).await
        };
        if invalidated > 0 {
            println!(
                "IncrementPipeline: invalidated {invalidated} PE/attribute cache entries \
                 and world-transform caches"
            );
        }
        persist_result?;

        // ADR-003 B1-emit: 维护失败可进持久补偿队列；补偿也落不下时不推进水位。
        //
        // 失败不能只留一句 warning。`ref_rev` 是「关联模型也要更新」的权威来源，缺一条边
        // 就是某个设计实例静默不重生成；而「靠后续触及 / 全量重建自愈」里没有任何一步是
        // 自动发生的——那条边可能到下一次有人手工跑全量重建为止都不存在。所以把这批引用者
        // 记进持久补偿队列，走与其它副作用同一条重试通道。
        if staged.is_none()
            && let Err(e) = StageTimings::measure(
                &mut timings.reverse_index,
                Self::maintain_reverse_index(&range_eles),
            )
            .await
        {
            warnings.push(format!(
                "reverse-index maintain (non-fatal) {}: {}",
                path.display(),
                e
            ));
            let referrers = Self::changed_refnos(&range_eles);
            crate::data_interface::side_effect_pending::SideEffectCompensator::enqueue_ref_rev(
                dbnum, end_sesno, db_type, &referrers,
            )
            .await?;
        }

        // Resolve-then-render（W3）：这个计时槽现在只含**窗口前持久态的点查**
        // （overlay 兜不住的祖先 noun/owner），产出的语句是固定目标 id 的纯
        // UPDATE——收口事务里不再有任何 datacenter 上溯 I/O，也不再依赖持久层
        // 灌了 fn::find_ancestor_types。
        let mut window_statements = StageTimings::measure(
            &mut timings.datacenter,
            Self::datacenter_statements(&range_eles, db_type),
        )
        .await?;
        // OWNER 搬迁的定点 anc 重算先于水位提交、失败即拦住水位
        // （窗口重放时随窗口语句批重算，幂等收敛）。
        window_statements.extend(Self::anc_repair_statements_for_window(&range_eles, db_type));

        // Finalization publishes this window's delivery-status updates as ordered
        // chunked batches, then one tail transaction establishes durable model
        // work, advances the watermark and removes the short-lived recovery
        // record. If any step fails, the watermark is untouched, the attempt
        // remains, and the whole fixed range is safe to replay (the idempotent
        // fixed-target updates converge on re-application).
        // 水位推进要顺手存下右端那条保存的写入时刻（plant-ui ADR-0019 Q6）：文件被换回
        // 旧版本之后这一页就读不到了，这一刻是唯一能存下来的时机。一页会话页，每批一次。
        let end_sesno_time =
            crate::data_interface::manual_update::session_time_rfc3339("", path, end_sesno);

        if staged.is_some() {
            let mut finalize_plan = model_plan.clone();
            finalize_plan.work_items.retain(|item| {
                item.action != crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot
            });
            StageTimings::measure(
                &mut timings.finalize,
                crate::data_interface::staging::register_staged_finalize(
                    crate::data_interface::staging::StagedFinalize {
                        dbnum,
                        start_sesno,
                        end_sesno,
                        end_sesno_time,
                        plan: finalize_plan,
                        window_statements,
                        cache_refnos: staged_cache_refnos.unwrap_or_default(),
                    },
                ),
            )
            .await?;
        } else {
            StageTimings::measure(
                &mut timings.finalize,
                crate::data_interface::model_update_pending::finalize_attempt(
                    dbnum,
                    end_sesno,
                    end_sesno_time.as_deref(),
                    &model_plan,
                    &window_statements,
                ),
            )
            .await?;
        }

        timings.report(
            dbnum,
            db_type,
            range_eles.values().map(|ops| ops.len()).sum::<usize>(),
        );

        let changed_refnos = Self::changed_refnos(&range_eles);

        Ok((
            IncrFileSuccess {
                path: path.clone(),
                dbnum,
                start_sesno,
                end_sesno,
                db_type: db_type.to_string(),
                changed_refnos,
                range_eles,
                model_plan,
            },
            warnings,
        ))
    }

    /// 本窗口的 OWNER 搬迁 anc 定点重算语句（随收口窗口语句批提交）。
    ///
    /// **只对 DESI 窗口渲染**（与 [`Self::datacenter_statements`] 同门，
    /// 2026-08-07 审核 P2）：`anc` 物化的是设计元素祖先链，CATA/SYST 元素的
    /// refno 不会出现在任何 anc 里——对它们渲染出的每条 UPDATE 都是收口事务里
    /// 的一次空转子查询扫描（目录重组一次搬上千元素时把收口拖慢一个量级）；
    /// 且这些语句对 `fn::anc_u64` 的硬依赖只受 DESI 批次预检
    /// （[`desi_finalize_preflight`]）保护，非 DESI 窗口渲染它们等于把
    /// 「函数缺失不炸」押在「空集不求值」这个引擎细节上。
    fn anc_repair_statements_for_window(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        db_type: &str,
    ) -> Vec<String> {
        if db_type != "DESI" {
            return Vec::new();
        }
        let moved = Self::moved_refnos(range_eles);
        if moved.is_empty() {
            return Vec::new();
        }
        println!(
            "IncrementPipeline: {} 个元素发生 OWNER 搬迁，提交尾重算其子树 anc",
            moved.len()
        );
        Self::render_anc_repair_statements(&moved)
    }

    /// 本窗口里发生 OWNER 搬迁的元素（`ChangeBucket::Moved` 口径）。
    ///
    /// 交付单元自己搬家会走重生成、实例行随之整体重写；这份名单主要治「单元层级
    /// 之上的容器搬家」（PIPE/ZONE 改挂 OWNER）——那种变更不产生任何模型工作项，
    /// 子树 inst_relate/tubi_relate 行上物化的 `anc` 却已陈旧。
    /// 名单不按 noun 过滤：单元根多修一次是幂等空转，漏修是静默陈旧。
    fn moved_refnos(range_eles: &BTreeMap<u32, Vec<EleOperationData>>) -> Vec<RefU64> {
        use crate::data_interface::model_impact::{ChangeBucket, user_change_buckets};
        let mut seen = HashSet::new();
        range_eles
            .values()
            .flatten()
            .flat_map(user_change_buckets)
            .filter(|(bucket, _)| *bucket == ChangeBucket::Moved)
            .map(|(_, refno)| refno.refno())
            .filter(|refno| seen.insert(*refno))
            .collect()
    }

    /// 单条 CONTAINSANY 语句最多携带的搬家元素数（语句体积上界 ~几 KB）。
    const ANC_REPAIR_CHUNK: usize = 200;

    /// 渲染搬家元素子树的 `anc` 定点重算语句（随收口窗口语句批
    /// 提交、先于水位；层级查询优化方案 P1 的搬家维护，P3 起不再连带
    /// `zone_refno`——列已退役）。
    ///
    /// `anc CONTAINSANY [搬家元素…]`：搬家把整棵子树一起带走，受影响行的 anc
    /// 无论新旧算法都含着搬家元素，所以这一个条件恰好圈出全部受影响行；重算
    /// 发生在数据落库之后，走的是提交后的活 owner 链。
    ///
    /// 整批搬家元素合并成每表**一条**语句（按 [`Self::ANC_REPAIR_CHUNK`] 分块）：
    /// 逐元素渲染的老形态是每个搬家元素两次 `WHERE anc CONTAINS` 子查询扫描，
    /// 一次容器大搬移（上千元素）就是收口路径上的数千次扫描；CONTAINSANY 是同
    /// 一批行的并集，且同一行只重算一次——重算本身幂等，语义不变（2026-08-10
    /// 审核 P1）。
    fn render_anc_repair_statements(moved: &[RefU64]) -> Vec<String> {
        moved
            .chunks(Self::ANC_REPAIR_CHUNK)
            .flat_map(|chunk| {
                let list = chunk
                    .iter()
                    .map(|refno| refno.0.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                [
                    format!(
                        "UPDATE (SELECT VALUE id FROM inst_relate WHERE anc CONTAINSANY [{list}]) SET \
                         anc = fn::anc_u64(in) RETURN NONE;"
                    ),
                    format!(
                        "UPDATE (SELECT VALUE id FROM tubi_relate WHERE anc CONTAINSANY [{list}]) SET \
                         anc = fn::anc_u64(in) RETURN NONE;"
                    ),
                ]
            })
            .collect()
    }

    /// 本窗口里真正动过的 refno，按首次出现去重。
    ///
    /// 与 [`crate::data_interface::manual_update::build_reverse_index_statements`] 同一口径
    /// （跳过 `None` 操作），所以它既是 `IncrFileSuccess.changed_refnos`，也正好是反向索引
    /// 维护碰过的那批引用者——修复任务要重建的就是这些。
    fn changed_refnos(range_eles: &BTreeMap<u32, Vec<EleOperationData>>) -> Vec<RefU64> {
        let mut seen = HashSet::new();
        range_eles
            .values()
            .flatten()
            .filter(|op| !matches!(op.detail, EleOperationDetail::None))
            .map(|op| op.refno)
            .filter(|refno| seen.insert(*refno))
            .collect()
    }

    /// Collect the cache keys whose database-backed values may change.
    ///
    /// Besides each changed element, include its current owner and both sides of
    /// an explicit OWNER move. This invalidates parent hierarchy/attribute reads
    /// used by the subsequent model refresh. The global world-transform caches
    /// are cleared by [`aios_core::clear_all_caches`] as part of each invalidation.
    fn collect_cache_invalidation_refnos(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> HashSet<RefnoEnum> {
        use crate::data_interface::model_impact::changed_owner_refnos;

        let mut refnos = HashSet::new();
        for operation in range_eles.values().flatten() {
            if matches!(&operation.detail, EleOperationDetail::None) {
                continue;
            }

            let changed = RefnoEnum::from(operation.refno);
            if changed.is_valid() {
                refnos.insert(changed);
            }

            refnos.extend(changed_owner_refnos(operation));

            let current_owner = match &operation.detail {
                EleOperationDetail::Add(element) => Some(RefnoEnum::from(element.owner)),
                EleOperationDetail::Modified(element) => {
                    Some(RefnoEnum::from(element.current_data.owner))
                }
                EleOperationDetail::Deleted | EleOperationDetail::None => None,
            };
            if let Some(owner) = current_owner.filter(|owner| owner.is_valid()) {
                refnos.insert(owner);
            }
        }
        refnos
    }

    /// Clear database-backed aios-core caches before any post-persist consumer
    /// (model refresh, transform update, preview, etc.) can read stale values.
    ///
    /// Invalidating per refno would re-clear the global world-transform caches
    /// and re-take every cache lock once per element, so a wide window paid for
    /// the same wholesale clear thousands of times.
    async fn invalidate_caches(refnos: HashSet<RefnoEnum>) -> usize {
        if refnos.is_empty() {
            return 0;
        }
        let refnos: Vec<RefnoEnum> = refnos.into_iter().collect();
        clear_all_caches_batch(&refnos).await;
        refnos.len()
    }

    /// Persist ONLY the latest main data (pe + attributes) for this delta.
    ///
    /// Deliberately skips the history/version tables (`sessions` /
    /// `element_changes`): we keep only the latest state, no historical
    /// versions. Mirrors step 5 of the old `update_elements_to_database`,
    /// batching `EleOperationData::to_surql` in groups of 100.
    ///
    /// Any batch write failure is propagated (ADR-001): the caller must NOT
    /// advance the watermark unless the whole batch persisted. Swallowing errors
    /// here would let `applied_sesno` run ahead of the data actually stored.
    async fn persist_latest_main_data(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        dbnum: i32,
    ) -> anyhow::Result<()> {
        // 收集本文件本窗口的全部落库语句，随后分块提交（提交策略见下方 TX_CHUNK 处）。
        // ADR-001「失败批次不推进水位、按同一窗口重试」的安全性并不依赖「整窗口一个
        // 事务」，而是靠 Add 改用幂等 UPSERT：重试撞上上一轮已写入的记录也能覆盖收敛，
        // 不会出现「半写 + 重试反复撞已存在记录失败 → dbnum 水位卡死」。
        let statements = Self::render_persist_statements(range_eles, dbnum);
        let total = statements.len();
        // 分块事务提交：原实现把整窗口拼成「单个事务」，大型系统库（如 amssys 冷启动
        // 168 会话 ~4000+ 元素）会撑爆 SurrealDB ws 通道上限，报「receiving from an
        // empty and closed channel」而整体失败。改为按 TX_CHUNK 条语句一块、每块自身
        // 原子提交：配合幂等 UPSERT 与「失败不推进水位、按同一窗口重试」，重试仍从可
        // 收敛状态开始，不会半写卡死。语句顺序保持不变，跨块引用与单事务同样是前向依赖。
        const TX_CHUNK: usize = 500;
        for chunk in statements.chunks(TX_CHUNK) {
            if let Some(tx_sql) = wrap_in_transaction(chunk) {
                // `.check()`：把事务内被取消/失败的语句错误上浮为 Err。原实现只 map_err
                // 传输错误、未 check 语句级错误，事务被取消时仍可能返回 Ok → 水位误推进。
                SUL_DB
                    .query(&tx_sql)
                    .await
                    .map_err(|e| anyhow::anyhow!("增量主数据落库失败(事务提交): {e}"))?
                    .check()
                    .map_err(|e| anyhow::anyhow!("增量主数据落库失败(事务内语句): {e}"))?;
            }
        }

        println!(
            "增量主数据落库完成，共 {total} 条（分块事务提交 chunk={TX_CHUNK}，仅最新状态，不写历史）"
        );
        Ok(())
    }

    /// 渲染本窗口的全部落库语句（折叠后）。纯函数：直写路径
    /// （[`Self::persist_latest_main_data`]）与暂存路径
    /// （[`Self::apply_window_staged`]）共用同一份渲染，两边不可能漂移。
    ///
    /// 窗口内同一 refno 被连续改 N 次就写 N 次，而本模块只保留最新状态，中间态
    /// 全部会被最后一次覆盖。折叠掉它们（见 `fold_window`）既减语句数也减 SQL
    /// 体积；被折掉的位置一定是 Modified（必然渲染出语句），折叠量以日志报出。
    pub(crate) fn render_persist_statements(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        dbnum: i32,
    ) -> Vec<String> {
        let raw_ops: usize = range_eles.values().map(|v| v.len()).sum();
        let planned = fold_window(range_eles);

        let mut statements: Vec<String> = Vec::new();
        for write in &planned {
            let id = write.op.refno.to_string();
            let surql = match &write.folded {
                Some(folded) => folded.to_modify_surql(&id, write.sesno),
                None => write.op.to_surql(&id, dbnum, write.sesno),
            };
            if !surql.is_empty() {
                statements.push(surql);
            }
        }

        let folded_away = raw_ops.saturating_sub(planned.len());
        if folded_away > 0 {
            println!(
                "增量窗口折叠：合并同 refno 的连续 Modified，省下 {folded_away} 条语句（实际落库 {} 条）",
                statements.len()
            );
        }
        statements
    }

    /// 本窗口每个 refno 的**净态**（noun / owner，按会话升序后写覆盖先写）——
    /// datacenter 上溯的 overlay 层：窗口里改过的元素以窗口终态为准，没改过的
    /// 落到持久层窗口前态（单写者下两层合成 == 主数据重放后的持久层状态，
    /// 也就是老的 commit-time `fn::find_ancestor_types` 现场上溯看到的世界）。
    fn window_net_states(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> HashMap<RefU64, (Option<String>, Option<RefU64>)> {
        use crate::data_interface::cata_closure::is_valid_ref0;
        use crate::data_interface::model_impact::{added_owner, owner_change};
        let mut states: HashMap<RefU64, (Option<String>, Option<RefU64>)> = HashMap::new();
        for op in data.values().flatten() {
            let entry = states.entry(op.refno).or_default();
            match &op.detail {
                EleOperationDetail::Add(_) => {
                    let noun = op.get_noun_type();
                    if !noun.is_empty() {
                        entry.0 = Some(noun);
                    }
                    if let Some(owner) = added_owner(op) {
                        entry.1 = Some(owner.refno());
                    }
                }
                EleOperationDetail::Modified(modified) => {
                    if !modified.noun.is_empty() {
                        entry.0 = Some(modified.noun.clone());
                    }
                    if is_valid_ref0(modified.current_data.owner.get_0()) {
                        entry.1 = Some(modified.current_data.owner);
                    } else if let (_, Some(new_owner)) = owner_change(op) {
                        entry.1 = Some(new_owner.refno());
                    }
                }
                // Deleted 不带任何新态：它的 noun/owner 就是窗口前持久态
                // （软删不改行内容，与老形态 commit-time 读 $pe.noun/$pe.owner
                // 看到的是同一份）。
                EleOperationDetail::Deleted | EleOperationDetail::None => {}
            }
        }
        states
    }

    /// Resolve-then-render this window's `datacenter_version` status updates
    /// （W3，决议 D5）：上溯在**渲染时**用 Rust 侧走链完成——overlay（本窗口
    /// 净态）优先、持久层窗口前态兜底（`load_pe`，与锁域解析同一
    /// `mutation_roots_resolve_against_the_pre_window_persistent_state` 纪律）——
    /// 产出**固定目标 id 的纯 UPDATE**。收口事务从此不再依赖持久层的
    /// `fn::find_ancestor_types` 现场求值（issue #16 的故障面），也不再受
    /// `fn::ancestor` 9 跳展开预算限制（Rust 走链上限 64）。
    ///
    /// `UPDATE` only touches delivery records that already exist, so an element
    /// that was never published to the data centre is a silent no-op — the
    /// statements are safe to emit for every changed element. Each one carries
    /// its own `;` so a batch can be concatenated verbatim.
    ///
    /// 上溯解不出目标（链断 / 没有单元层祖先，如 SITE 自身的属性修改）时**跳过**
    /// 该语句：老形态里这是 `$pe = NONE` 塞进 `type::thing` 的未定义行为角落，
    /// 现在是显式的「无交付记录可标，无事发生」。
    async fn resolve_datacenter_statements_with<F, Fut>(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
        delivery_unit_types: &[String],
        mut load_pe: F,
    ) -> anyhow::Result<Vec<String>>
    where
        F: FnMut(Vec<RefU64>) -> Fut,
        Fut: std::future::Future<
                Output = anyhow::Result<HashMap<RefU64, (Option<String>, Option<RefU64>)>>,
            >,
    {
        /// 防御性走链上限（owner 环 / 数据损坏的最后一道闸）。
        const ROLLUP_WALK_CAP: usize = 64;

        /// 逐 refno 的效果视图（noun, owner）：overlay 字段优先，缺的问持久层
        /// （懒加载 + 记忆化；`cache` 里 `None` = 行不存在，问过了）。
        async fn effective_view<F, Fut>(
            overlay: &HashMap<RefU64, (Option<String>, Option<RefU64>)>,
            cache: &mut HashMap<RefU64, Option<(Option<String>, Option<RefU64>)>>,
            load_pe: &mut F,
            refno: RefU64,
        ) -> anyhow::Result<(Option<String>, Option<RefU64>)>
        where
            F: FnMut(Vec<RefU64>) -> Fut,
            Fut: std::future::Future<
                    Output = anyhow::Result<HashMap<RefU64, (Option<String>, Option<RefU64>)>>,
                >,
        {
            use crate::data_interface::cata_closure::is_valid_ref0;
            let overlay_entry = overlay.get(&refno).cloned().unwrap_or((None, None));
            if overlay_entry.0.is_some() && overlay_entry.1.is_some() {
                return Ok(overlay_entry);
            }
            if !cache.contains_key(&refno) {
                let fetched = load_pe(vec![refno]).await?;
                cache.insert(refno, fetched.get(&refno).cloned());
            }
            let stored = cache.get(&refno).cloned().flatten().unwrap_or((None, None));
            Ok((
                overlay_entry.0.or(stored.0),
                overlay_entry
                    .1
                    .or(stored.1.filter(|owner| is_valid_ref0(owner.get_0()))),
            ))
        }

        let mut unit = delivery_unit_types.to_vec();
        unit.push("ZONE".into());
        let overlay = Self::window_net_states(data);
        // 持久层窗口前态缓存：None = 行不存在（问过了）。
        let mut persistent: HashMap<RefU64, Option<(Option<String>, Option<RefU64>)>> =
            HashMap::new();

        let mut statements = Vec::new();
        for ops in data.values() {
            for d in ops {
                match &d.detail {
                    EleOperationDetail::Deleted => {
                        let (noun, owner) =
                            effective_view(&overlay, &mut persistent, &mut load_pe, d.refno)
                                .await?;
                        let belong_zone = match (noun.as_deref(), owner) {
                            // BRAN 的归属 ZONE 隔一层 PIPE（与老形态的
                            // `$pe.owner.owner` 同义）。
                            (Some("BRAN"), Some(owner)) => {
                                effective_view(&overlay, &mut persistent, &mut load_pe, owner)
                                    .await?
                                    .1
                            }
                            (_, owner) => owner,
                        };
                        let belong_zone = belong_zone
                            .map(|refno| refno.to_pe_key())
                            .unwrap_or_else(|| "NONE".into());
                        statements.push(format!(
                            "update {} set status = '{:?}', belong_zone = {};",
                            d.refno.to_table_key(DATACENTER_VERSION),
                            DataCenterRecordOperate::Delete,
                            belong_zone
                        ));
                    }
                    EleOperationDetail::Modified(modify_data) => {
                        let target = if unit.iter().any(|noun| noun == &modify_data.noun) {
                            Some(d.refno)
                        } else {
                            // 最近的单元层（含 ZONE）自身或祖先——与
                            // `fn::find_ancestor_types(pe, [...])[0]` 同义，但走
                            // overlay+持久层的 Rust 链，不吃 9 跳静默截断。
                            let mut current = d.refno;
                            let mut target = None;
                            for _ in 0..ROLLUP_WALK_CAP {
                                let (noun, owner) = effective_view(
                                    &overlay,
                                    &mut persistent,
                                    &mut load_pe,
                                    current,
                                )
                                .await?;
                                let Some(noun) = noun else {
                                    break; // 链断：行不在（老形态同样解析不出）
                                };
                                if unit.iter().any(|unit_noun| unit_noun == &noun) {
                                    target = Some(current);
                                    break;
                                }
                                let Some(owner) = owner else {
                                    break; // 到顶（WORL 之上）仍没有单元层
                                };
                                current = owner;
                            }
                            target
                        };
                        match target {
                            Some(target) => statements.push(format!(
                                "update {} set status = '{:?}';",
                                target.to_table_key(DATACENTER_VERSION),
                                DataCenterRecordOperate::Modify
                            )),
                            None => println!(
                                "datacenter 上溯：{} 解不出单元层归属（链断或到顶），\
                                 无交付记录可标，跳过",
                                d.refno.to_pe_key()
                            ),
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(dedup_statements_keep_last(statements))
    }

    /// 窗口前持久态的 pe 点查（noun + owner），显式 `SUL_DB` 不经读路由——
    /// 与锁域解析同一纪律；软删行仍在场，Deleted 分支读到的正是删除前的归属。
    async fn load_pe_noun_owner_from_persistent(
        refnos: Vec<RefU64>,
    ) -> anyhow::Result<HashMap<RefU64, (Option<String>, Option<RefU64>)>> {
        #[derive(serde::Deserialize)]
        struct PeChainRow {
            id: RefnoEnum,
            #[serde(default)]
            noun: Option<String>,
            #[serde(default)]
            owner: Option<RefnoEnum>,
        }
        let mut out = HashMap::new();
        for chunk in refnos.chunks(200) {
            let keys = chunk
                .iter()
                .map(|refno| refno.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let mut response = SUL_DB
                .query(format!(
                    "SELECT id, noun, owner FROM [{keys}] WHERE record::exists(id);"
                ))
                .await?
                .check()?;
            let rows: Vec<PeChainRow> = response.take(0)?;
            for row in rows {
                out.insert(
                    row.id.refno(),
                    (row.noun, row.owner.map(|owner| owner.refno())),
                );
            }
        }
        Ok(out)
    }

    /// The statements marking this window's delivery records Modify / Delete in
    /// `datacenter_version`, empty for a window that cannot have any.
    ///
    /// Delivery records are published design elements, and the
    /// `unit`/`belong_zone` rollup semantics only exist in the DESI hierarchy —
    /// SYS meta DBs hold project structure (MDB/DB/CURD/TEAM) and CATA holds
    /// catalogue definitions, so none of their elements can ever match a
    /// delivery record. Skipping every non-DESI window keeps cold starts and
    /// catalogue imports from paying for thousands of guaranteed no-op UPDATEs.
    ///
    /// These statements used to be executed right here, in chunks, outside any
    /// transaction, with the error downgraded to a caller warning. A failed
    /// status write was then lost for good: the watermark still advanced past
    /// the one window that carried it, and no later window revisits an element
    /// that did not change again. They are now handed to `finalize_attempt` /
    /// the staged commit, which execute them as ordered batches **before** the
    /// watermark-advancing tail transaction — a failure keeps the watermark
    /// unmoved so the window replays and the idempotent updates converge
    /// (`model_update_pending::FinalizeRender` has the full argument).
    async fn datacenter_statements(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
        db_type: &str,
    ) -> anyhow::Result<Vec<String>> {
        if db_type != "DESI" {
            return Ok(Vec::new());
        }

        let delivery_unit_types =
            crate::data_interface::generation_root::configured_delivery_unit_types();
        Self::resolve_datacenter_statements_with(
            data,
            &delivery_unit_types,
            Self::load_pe_noun_owner_from_persistent,
        )
        .await
    }

    /// ADR-003 B1-emit: maintain the reverse-reference index (`ref_rev`) for this
    /// window. A direct failure is recoverable through the durable side-effect
    /// queue; if that recovery record cannot be persisted, the caller returns
    /// before finalization so the watermark does not publish an unrecoverable
    /// stale index. This write is not part of the main-data transaction.
    /// Statements are rendered by the pure
    /// [`crate::data_interface::manual_update::build_reverse_index_statements`].
    async fn maintain_reverse_index(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> anyhow::Result<()> {
        let statements =
            crate::data_interface::manual_update::build_reverse_index_statements(range_eles);
        if statements.is_empty() {
            return Ok(());
        }
        const CHUNK: usize = 500;
        for chunk in statements.chunks(CHUNK) {
            let sql = chunk.join("\n");
            SUL_DB
                .query(&sql)
                .await
                .map_err(|e| anyhow::anyhow!("反向引用索引维护失败(非致命): {e}"))?
                .check()
                .map_err(|e| anyhow::anyhow!("反向引用索引语句失败(非致命): {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    /// 失效集必须在「元素 + 属主」之外并入计划层算出的生成根（ADR-010 残余）：
    /// `QUERY_DEEP_CHILDREN_REFNOS` 按子树根为键，漏掉根键的失效等于同根下一次
    /// 重生成拿旧成员表静默漏算。钉住书写顺序：collect → extend(regen roots) →
    /// 才轮到暂存快照捕获与直写失效。
    #[test]
    fn cache_invalidation_extends_to_the_plans_regen_roots() {
        let source = include_str!("increment_pipeline.rs");
        let collect_at = source
            .find("Self::collect_cache_invalidation_refnos(&range_eles)")
            .expect("元素级失效集必须存在");
        let extend_at = source
            .find("cache_refnos.extend(model_plan.regen_root_refnos())")
            .expect("失效集必须并入生成根");
        let staged_capture_at = source
            .find("let staged_cache_refnos")
            .expect("暂存路径的失效快照必须存在");
        assert!(
            collect_at < extend_at && extend_at < staged_capture_at,
            "顺序必须是 collect → extend(regen roots) → 暂存快照捕获"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_parse_keeps_one_journal_and_does_not_finalize() {
        use crate::data_interface::staging::{
            ExecMode, ResourceThresholds, lifecycle::create_window_on,
        };
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("mem boots");
        let mut window = create_window_on(&instance, 7996, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");
        window
            .execute(
                "UPSERT pe:⟨7996_10⟩ SET noun = 'PIPE'",
                ExecMode::StagingOnly,
            )
            .await
            .expect("seed staging only");

        let mut range = BTreeMap::new();
        range.insert(
            1,
            vec![EleOperationData::new(
                RefU64((7996_u64 << 32) | 10),
                1,
                EleOperationDetail::Deleted,
            )],
        );

        let staged = IncrementPipeline::stage_parsed_window(&mut window, &range, 7996)
            .await
            .expect("stage parsed window");
        assert_eq!(staged, 2, "主数据删除 + ref_rev 清理");
        assert_eq!(window.journal().await.len(), 2, "必须沿用同一 journal");

        let db = window.staging_db().clone();
        let mut response = db
            .query("SELECT * FROM dbnum_watermark")
            .await
            .expect("query watermark");
        let rows: surrealdb::Value = response.take(0).expect("take watermark");
        assert_eq!(
            serde_json::to_string(&rows).expect("serialize"),
            "{\"Array\":[]}",
            "解析阶段不得提前推进水位"
        );

        window.drop_database().await.expect("cleanup");
    }

    /// 解析阶段过后，暂存里**依然没有**删除/修改目标的 `pe` 行。
    ///
    /// 两类操作渲染出来的主数据语句都是 `UPDATE pe:…`，而 SurrealDB 2.x 的 `UPDATE`
    /// 命不中记录就是空操作；暂存库起点是空的，这两类目标的 `pe` 行只存在于持久层。
    /// 于是任何在窗口上下文里解析生成根的调用都只会拿到 `None`——「一次性持有全部
    /// 生成根锁」在暂存世界里会静默退化成一把都不持有。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_window_cannot_see_the_ownership_of_deleted_or_modified_targets() {
        use crate::data_interface::staging::{ResourceThresholds, lifecycle::create_window_on};
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("mem boots");
        let mut window = create_window_on(&instance, 7995, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");

        let deleted = RefU64((7995_u64 << 32) | 10);
        let modified = RefU64((7995_u64 << 32) | 11);
        let mut range = BTreeMap::new();
        range.insert(
            1,
            vec![
                EleOperationData::new(deleted, 1, EleOperationDetail::Deleted),
                EleOperationData::new(
                    modified,
                    1,
                    EleOperationDetail::Modified(ModifiedElement {
                        current_data: Default::default(),
                        added_attrs: Default::default(),
                        deleted_attrs: Default::default(),
                        modified_attrs: Default::default(),
                        added_explicit_attrs: Default::default(),
                        deleted_explicit_attrs: Default::default(),
                        modified_explicit_attrs: Default::default(),
                        added_uda_attrs: Default::default(),
                        deleted_uda_attrs: Default::default(),
                        modified_uda_attrs: Default::default(),
                        noun: "DAMP".to_string(),
                        children_changed: None,
                    }),
                ),
            ],
        );

        IncrementPipeline::stage_parsed_window(&mut window, &range, 7995)
            .await
            .expect("stage parsed window");

        let unit_types = vec!["BRAN".to_string()];
        for refno in [deleted, modified] {
            let refno = RefnoEnum::from(refno);
            let pe = window
                .scope(aios_core::get_pe(refno))
                .await
                .expect("staged read");
            assert!(pe.is_none(), "暂存里不该凭空出现 {refno} 的 pe 行: {pe:?}");
            let root = window
                .scope(
                    crate::data_interface::generation_root::resolve_live_element_generation_root(
                        refno,
                        &unit_types,
                    ),
                )
                .await
                .expect("staged root resolution");
            assert!(
                root.is_none(),
                "暂存态解析不出 {refno} 的生成根，锁范围会静默漏掉它: {root:?}"
            );
        }

        window.drop_database().await.expect("cleanup");
    }

    #[test]
    fn reverse_index_failure_requires_durable_recovery_before_finalize() {
        let source = include_str!("increment_pipeline.rs");
        let recovery = source
            .split_once("Self::maintain_reverse_index(&range_eles)")
            .expect("reverse-index maintenance must exist")
            .1
            .split_once("let datacenter_statements")
            .expect("recovery must finish before finalization")
            .0;

        assert!(recovery.contains("enqueue_ref_rev"), "{recovery}");
        assert!(
            !recovery.contains("if let Err(enqueue_error)"),
            "recovery enqueue failure must escape apply_one instead of becoming a warning"
        );
        assert!(
            recovery.contains("await?"),
            "durable recovery must succeed before finalization"
        );
    }

    #[test]
    fn cache_targets_are_deduped_and_none_operations_are_skipped() {
        let changed = RefU64((1_u64 << 32) | 42);
        let ignored = RefU64((1_u64 << 32) | 99);
        let mut range_eles = BTreeMap::new();
        range_eles.insert(
            1,
            vec![
                EleOperationData::new(changed, 1, EleOperationDetail::Deleted),
                EleOperationData::new(changed, 1, EleOperationDetail::Deleted),
                EleOperationData::new(ignored, 1, EleOperationDetail::None),
            ],
        );

        let targets = IncrementPipeline::collect_cache_invalidation_refnos(&range_eles);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains(&RefnoEnum::from(changed)));
        assert!(!targets.contains(&RefnoEnum::from(ignored)));
    }

    #[test]
    fn wrap_in_transaction_is_atomic_or_none() {
        assert_eq!(wrap_in_transaction(&[]), None);

        let sql = wrap_in_transaction(&[
            "UPSERT a:1 CONTENT {}".to_string(),
            "UPDATE pe:1 SET x = 1".to_string(),
        ])
        .expect("non-empty statements must wrap");

        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with(";\nCOMMIT TRANSACTION;"), "{sql}");
        // Both statements are inside the same transaction body.
        assert!(
            sql.contains("UPSERT a:1 CONTENT {};\nUPDATE pe:1 SET x = 1"),
            "{sql}"
        );
    }

    #[test]
    fn prepared_attempt_rejects_a_file_that_no_longer_covers_fixed_range() {
        let attempt = crate::data_interface::model_update_pending::IncrementUpdateAttempt {
            dbnum: 8191,
            db_type: "DESI".to_string(),
            file_path: "D:/project/desi".to_string(),
            start_sesno: 40,
            end_sesno: 42,
            plan: Default::default(),
        };
        let error = validate_prepared_attempt(&attempt, "DESI", "D:/project/desi", 41)
            .expect_err("rollback must be rejected");
        assert!(error.to_string().contains("only covers through 41"));
        validate_prepared_attempt(&attempt, "DESI", "D:/project/desi", 42)
            .expect("complete fixed range is replayable");
    }

    #[cfg(windows)]
    #[test]
    fn prepared_attempt_accepts_equivalent_windows_path_spelling() {
        let attempt = crate::data_interface::model_update_pending::IncrementUpdateAttempt {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file_path: "D:\\AVEVA\\Projects\\E3D3.1\\AvevaMarineSample\\ams000\\ams8000_0001"
                .to_string(),
            start_sesno: 196,
            end_sesno: 196,
            plan: Default::default(),
        };

        validate_prepared_attempt(
            &attempt,
            "DESI",
            "d:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001",
            196,
        )
        .expect("separator and drive-letter casing must not change file identity");
    }

    /// 暂存窗口里碰上落后于文件的恢复记录：重建，不重放。
    ///
    /// 这是 dbnum=8000 那一幕的唯一出口——照旧重放的话，窗口停在 25 而 26 已经把 25
    /// 里还活着的元素删掉，祖先解析每一轮都断在同一个元素上，永不自愈。
    #[test]
    fn a_stale_staged_attempt_is_rebuilt_into_the_newer_sessions() {
        assert!(should_rebuild_stale_staged_attempt(Some(25), 26, true));
    }

    /// 其余三种情形一律不许丢弃这份持久化计划。
    #[test]
    fn nothing_else_discards_a_prepared_attempt() {
        // 直写模式：PE 块可能已写了一半而水位故意没动，更新前的 OWNER 图不再可信，
        // 而这条路上没有 journal、也没有整窗口重算那条退路。
        assert!(!should_rebuild_stale_staged_attempt(Some(25), 26, false));
        // 记录与文件持平：本来就是原样重放那条路。
        assert!(!should_rebuild_stale_staged_attempt(Some(26), 26, true));
        // 记录超前于文件（回退 / 换文件）：落回重放分支，让
        // `validate_prepared_attempt` 出人话诊断，不在这里悄悄吞掉。
        assert!(!should_rebuild_stale_staged_attempt(Some(27), 26, true));
        // 压根没有恢复记录：这是一次全新的窗口。
        assert!(!should_rebuild_stale_staged_attempt(None, 26, true));
    }

    /// IU-S3-04：交入窗口只有与请求区间**完全一致**才被采信，其余一律回退
    /// 自行收集。掐头/去尾/整体错位的窗口混进来，折叠与影响判定就会漏看会话。
    #[test]
    fn handed_in_window_is_only_accepted_on_an_exact_range_match() {
        let eles = || BTreeMap::from([(25u32, Vec::<EleOperationData>::new())]);

        assert!(
            accept_handed_in_window(Some((25..=26, eles())), &(25..=26)).is_some(),
            "区间逐位相等必须复用（这正是双跑修复省下的那一遍解析）"
        );
        // 右端多一格 / 左端少一格 / 完全错开 / 反向，全部拒绝。
        assert!(accept_handed_in_window(Some((25..=27, eles())), &(25..=26)).is_none());
        assert!(accept_handed_in_window(Some((24..=26, eles())), &(25..=26)).is_none());
        assert!(accept_handed_in_window(Some((27..=30, eles())), &(25..=26)).is_none());
        #[allow(clippy::reversed_empty_ranges)]
        {
            assert!(accept_handed_in_window(Some((26..=25, eles())), &(25..=26)).is_none());
        }
        // 没交就是没交。
        assert!(accept_handed_in_window(None, &(25..=26)).is_none());
    }

    /// IU-S3-03（L1 源码钉）：崩溃重放分支**永远重新收集**——prepared 命中时
    /// 三元组的 collected 位恒为 None，交入窗口只在 prepared 落空的 fresh 分支
    /// 被询问。把交入结果喂给重放分支，就是把「按持久化固定区间重放」偷换成
    /// 「按调用方本次窗口重放」，`validate_prepared_attempt` 挡的正是这种错位。
    /// 回退即红。
    #[test]
    fn crash_replay_never_consumes_the_handed_in_window() {
        let source = include_str!("increment_pipeline.rs");
        let body = source
            .split_once(concat!("async fn ", "apply_one("))
            .expect("apply_one must exist")
            .1
            .split_once(concat!("async fn ", "invalidate_caches("))
            .expect("invalidate_caches follows apply_one in this file")
            .0;

        // prepared 分支的产物：固定区间 + 持久化计划 + **None**（不带收集结果）。
        assert!(
            body.contains(concat!("attempt.plan, ", "None)")),
            "重放分支必须以 None 进入收集阶段: {body}"
        );
        // 交入窗口的唯一询问点在 prepared 校验之后（即 fresh 分支里）。
        let replay_at = body
            .find(concat!("validate_prepared_attempt", "(&attempt"))
            .expect("重放校验缺失");
        let handed_at = body
            .find(concat!(
                "accept_handed_in_window",
                "(precollected, &requested_range)"
            ))
            .expect("交入窗口采信点缺失");
        assert!(
            replay_at < handed_at,
            "交入窗口只能在 prepared 落空之后询问: {body}"
        );
        // 反空转：采信点恰好一处——多出第二处就说明有人绕开了统一判定。
        assert_eq!(
            body.matches(concat!("accept_handed_in_window", "(")).count(),
            1,
            "采信判定必须只有一个入口"
        );
    }

    /// IU-S8-05 / IU-S12-02（L1 源码钉）：直写路径的缓存失效必须在**每次尝试
    /// 落库之后**执行——包括部分失败的批次。persist 的结果被捕获成
    /// `persist_result`、失效跑完才 `?` 上抛：部分失败时前面的 Surreal 语句可能
    /// 已经改了数据而水位保持不动，此刻不清缓存，preview / 模型刷新这些后继者
    /// 读到的就是旧值。回退成落库当场 `?` 直接上抛即红。
    #[test]
    fn cache_invalidation_survives_a_partially_failed_persist() {
        let source = include_str!("increment_pipeline.rs");
        let body = source
            .split_once(concat!("async fn ", "apply_one("))
            .expect("apply_one must exist")
            .1
            .split_once(concat!("async fn ", "invalidate_caches("))
            .expect("invalidate_caches follows apply_one")
            .0;
        let capture_at = body
            .find("let persist_result = ")
            .expect("persist 结果必须被捕获而不是当场上抛");
        let invalidate_at = body
            .find("Self::invalidate_caches(cache_refnos)")
            .expect("直写路径必须执行缓存失效");
        let propagate_at = body
            .find("persist_result?")
            .expect("persist 失败最终要上抛（水位不推进）");
        assert!(
            capture_at < invalidate_at && invalidate_at < propagate_at,
            "顺序必须是 捕获 persist 结果 → 缓存失效 → 才上抛失败: {body}"
        );
    }

    /// IU-S0-05 的 warning 半边（名字白名单本身由 increment_manager 的真实
    /// 正反例表钉住）：名字不合 AVEVA 形态的文件在 apply 入口被跳过时必须留下
    /// warning 再 continue——无声跳过表现为「无错也无果」，正是 apply_file
    /// 透出 warnings（2026-08-12）要消灭的那类现象。
    #[test]
    fn skipped_copy_files_leave_a_warning_behind() {
        let source = include_str!("increment_pipeline.rs");
        let body = source
            .split_once(concat!("pub async fn ", "apply_with_precollected("))
            .expect("apply_with_precollected must exist")
            .1
            .split_once(concat!("async fn ", "apply_one("))
            .expect("apply_one follows")
            .0;
        let gate_at = body
            .find("is_pdms_db_file_name(file_name)")
            .expect("入口必须有名字白名单门");
        let warn_at = body.find("skip copy file").expect("跳过必须留 warning");
        let continue_at = body[gate_at..]
            .find("continue")
            .map(|at| gate_at + at)
            .expect("跳过分支必须 continue 而不是漏进 apply_one");
        assert!(
            gate_at < warn_at && warn_at < continue_at,
            "顺序必须是 白名单门 → warnings.push → continue: {body}"
        );
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use aios_core::NamedAttrValue;
    use std::collections::HashMap;

    fn refno(id: u64) -> RefU64 {
        RefU64((7997u64 << 32) | id)
    }

    fn text(v: &str) -> NamedAttrValue {
        NamedAttrValue::StringType(v.to_string())
    }

    fn blank() -> ModifiedElement {
        ModifiedElement {
            current_data: Default::default(),
            added_attrs: Default::default(),
            deleted_attrs: Default::default(),
            modified_attrs: Default::default(),
            added_explicit_attrs: Default::default(),
            deleted_explicit_attrs: Default::default(),
            modified_explicit_attrs: Default::default(),
            added_uda_attrs: Default::default(),
            deleted_uda_attrs: Default::default(),
            modified_uda_attrs: Default::default(),
            noun: "DAMP".to_string(),
            children_changed: None,
        }
    }

    fn op(id: u64, sesno: u32, element: ModifiedElement) -> EleOperationData {
        EleOperationData::new(refno(id), sesno, EleOperationDetail::Modified(element))
    }

    fn window(ops: Vec<EleOperationData>) -> BTreeMap<u32, Vec<EleOperationData>> {
        let mut map: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
        for op in ops {
            map.entry(op.sesno).or_default().push(op);
        }
        map
    }

    fn folded_at<'a>(planned: &'a [PlannedWrite<'a>], id: u64) -> &'a ModifiedElement {
        planned
            .iter()
            .find(|w| w.op.refno == refno(id))
            .and_then(|w| w.folded.as_ref())
            .expect("expected a folded run for this refno")
    }

    #[test]
    fn a_run_of_modified_collapses_onto_its_last_session() {
        let mut second = blank();
        second.added_attrs.insert("XLEN".into(), text("2"));

        let w = window(vec![op(1, 1, blank()), op(1, 2, second), op(1, 3, blank())]);
        let planned = fold_window(&w);

        assert_eq!(planned.len(), 1);
        // The merged write lands where the newest operation was, so `pe.sesno`
        // still ends at the latest session that touched the element.
        assert_eq!(planned[0].sesno, 3);
    }

    /// The case a naive union of the three delta maps gets wrong: the key would
    /// stay in `deleted_attrs`, which `to_modify_surql` applies last, silently
    /// turning a live value back into `NULL`.
    #[test]
    fn a_key_deleted_then_re_added_keeps_the_newer_value() {
        let mut first = blank();
        first.deleted_attrs.insert("XLEN".into(), text("old"));
        let mut second = blank();
        second.added_attrs.insert("XLEN".into(), text("new"));

        let w = window(vec![op(1, 1, first), op(1, 2, second)]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);

        assert_eq!(folded.added_attrs.get("XLEN"), Some(&text("new")));
        assert!(folded.deleted_attrs.is_empty());
    }

    #[test]
    fn a_key_added_then_deleted_ends_up_cleared() {
        let mut first = blank();
        first.added_attrs.insert("XLEN".into(), text("v"));
        let mut second = blank();
        second.deleted_attrs.insert("XLEN".into(), text("v"));

        let w = window(vec![op(1, 1, first), op(1, 2, second)]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);

        assert!(folded.added_attrs.is_empty());
        assert!(folded.deleted_attrs.contains_key("XLEN"));
    }

    #[test]
    fn later_sessions_win_per_key_across_every_namespace() {
        let mut first = blank();
        first
            .modified_attrs
            .insert("POS".into(), (text("a"), text("b")));
        first
            .added_explicit_attrs
            .insert("NAME".into(), text("/OLD"));
        first.added_uda_attrs.insert(7, text("u1"));

        let mut second = blank();
        second
            .modified_attrs
            .insert("POS".into(), (text("b"), text("c")));
        second
            .modified_explicit_attrs
            .insert("NAME".into(), (text("/OLD"), text("/NEW")));
        second
            .modified_uda_attrs
            .insert(7, (text("u1"), text("u2")));

        let w = window(vec![op(1, 1, first), op(1, 2, second)]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);

        assert_eq!(
            folded.modified_attrs.get("POS"),
            Some(&(text("b"), text("c")))
        );
        // NAME moved from `added` to `modified`, so it must no longer sit in both.
        assert!(folded.added_explicit_attrs.is_empty());
        assert_eq!(
            folded.modified_explicit_attrs.get("NAME"),
            Some(&(text("/OLD"), text("/NEW")))
        );
        assert!(folded.added_uda_attrs.is_empty());
        assert_eq!(
            folded.modified_uda_attrs.get(&7),
            Some(&(text("u1"), text("u2")))
        );
    }

    /// `Add` creates the record and `Deleted` lays the tombstone; folding across
    /// them would drop one of the two, so a run may only span `Modified`.
    #[test]
    fn add_and_deleted_break_a_run_and_are_never_merged() {
        let w = window(vec![
            EleOperationData::new(refno(1), 1, EleOperationDetail::Add(Default::default())),
            op(1, 2, blank()),
            op(1, 3, blank()),
            EleOperationData::new(refno(1), 4, EleOperationDetail::Deleted),
            op(1, 5, blank()),
        ]);
        let planned = fold_window(&w);

        // Add, one merged Modified, Deleted, and the trailing lone Modified.
        assert_eq!(planned.len(), 4);
        let kinds: Vec<&str> = planned.iter().map(|w| w.op.get_op_type()).collect();
        assert_eq!(kinds, vec!["新增", "修改", "删除", "修改"]);
        assert!(planned[1].folded.is_some(), "the run should be merged");
        assert!(planned[3].folded.is_none(), "a lone op needs no merge");
    }

    #[test]
    fn phantom_adds_absent_from_the_post_save_file_are_removed() {
        let mut changes = window(vec![
            EleOperationData::new(refno(1), 20, EleOperationDetail::Add(Default::default())),
            EleOperationData::new(refno(2), 20, EleOperationDetail::Add(Default::default())),
            op(3, 20, blank()),
        ]);

        let removed = retain_finally_live_adds(&mut changes, |candidate| candidate == refno(2));

        assert_eq!(removed, 1);
        let ops = changes.get(&20).expect("session remains present");
        assert!(!ops.iter().any(|op| op.refno == refno(1)));
        assert!(ops.iter().any(|op| op.refno == refno(2)));
        assert!(ops.iter().any(|op| op.refno == refno(3)));
    }

    #[test]
    fn finally_live_deleted_record_is_replaced_with_final_state_upsert() {
        let child = refno(11);
        let final_owner = refno(22);
        let mut final_element = parse_pdms_db::parse::EleData {
            refno: child,
            owner: final_owner,
            noun: 123,
            ..Default::default()
        };
        final_element
            .whole_attmap
            .attmap
            .insert("OWNER".into(), NamedAttrValue::RefU64Type(final_owner));
        final_element
            .whole_attmap
            .attmap
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(child));
        final_element
            .whole_attmap
            .attmap
            .insert("TYPE".into(), NamedAttrValue::StringType("BOX".into()));
        let mut changes = window(vec![EleOperationData::new(
            child,
            47,
            EleOperationDetail::Deleted,
        )]);
        let final_elements = HashMap::from([(child, final_element)]);

        assert_eq!(
            restore_finally_live_deletes(&mut changes, &final_elements),
            1
        );
        let operation = &changes[&47][0];
        let EleOperationDetail::Add(restored) = &operation.detail else {
            panic!("finally-live delete must become a full final-state upsert");
        };
        assert_eq!(restored.refno, child);
        assert_eq!(restored.owner, final_owner);
        let sql = operation.to_surql("7997_11", 7997, 47);
        assert!(
            sql.contains("\"deleted\": false"),
            "final-state upsert must clear a previous tombstone: {sql}"
        );
    }

    #[test]
    fn finally_absent_deleted_record_remains_a_true_delete() {
        let child = refno(11);
        let mut changes = window(vec![EleOperationData::new(
            child,
            47,
            EleOperationDetail::Deleted,
        )]);

        assert_eq!(
            restore_finally_live_deletes(&mut changes, &HashMap::new()),
            0
        );
        assert!(matches!(
            changes[&47][0].detail,
            EleOperationDetail::Deleted
        ));
    }

    #[test]
    fn final_record_payload_strips_page_markers_and_checks_declared_boundary() {
        let mut raw = vec![0, 0, 0, 7, 0, 0, 0, 0];
        raw.extend_from_slice(&6i32.to_be_bytes());
        raw.extend_from_slice(&[0; 20]);
        assert_eq!(final_record_payload(&raw).unwrap(), &raw[8..]);

        let mut truncated = vec![0, 0, 0, 8];
        truncated.extend_from_slice(&[0; 20]);
        assert!(final_record_payload(&truncated).is_err());
    }

    #[test]
    fn crash_replay_drops_phantom_design_refnos_from_the_durable_plan() {
        let mut plan = crate::data_interface::model_update_plan::ModelUpdatePlan {
            design_refnos: vec![
                RefnoEnum::from(refno(1)).to_pdms_str(),
                RefnoEnum::from(refno(2)).to_pdms_str(),
            ],
            ..Default::default()
        };

        let removed =
            retain_finally_live_design_refnos(&mut plan, |candidate| candidate == refno(2));

        assert_eq!(removed, 1);
        assert_eq!(
            plan.design_refnos,
            [RefnoEnum::from(refno(2)).to_pdms_str()]
        );
    }

    #[test]
    fn crash_replay_disables_phantom_generation_units() {
        let mut plan = crate::data_interface::model_update_plan::ModelUpdatePlan {
            units: vec![
                crate::data_interface::manual_update::DeliveryUnitSummary {
                    root_refno: RefnoEnum::from(refno(1)).to_pdms_str(),
                    will_generate: true,
                    ..Default::default()
                },
                crate::data_interface::manual_update::DeliveryUnitSummary {
                    root_refno: RefnoEnum::from(refno(2)).to_pdms_str(),
                    will_generate: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let live = std::collections::HashSet::from([refno(2)]);

        let removed = reconcile_plan_with_live_set(&mut plan, &live);

        assert_eq!(removed, 1);
        assert!(!plan.units[0].will_generate);
        assert!(plan.units[1].will_generate);
    }

    #[test]
    fn unrelated_refnos_keep_their_relative_order() {
        let w = window(vec![
            op(1, 1, blank()),
            op(2, 1, blank()),
            op(1, 2, blank()),
            op(2, 2, blank()),
        ]);
        let planned = fold_window(&w);

        assert_eq!(planned.len(), 2);
        // Both runs collapse onto session 2, preserving the order they appeared in.
        assert_eq!(planned[0].op.refno, refno(1));
        assert_eq!(planned[1].op.refno, refno(2));
    }

    /// The rebuilt `pe_owner` edges are a full replace, so the newest child list
    /// is the only correct one to render.
    #[test]
    fn the_newest_child_list_wins() {
        let mut first = blank();
        first.children_changed = Some((vec![refno(10)].into(), vec![refno(11)].into()));
        let mut second = blank();
        second.children_changed = Some((vec![refno(11)].into(), vec![refno(12)].into()));

        let w = window(vec![op(1, 1, first), op(1, 2, second)]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);

        let (old, new) = folded.children_changed.as_ref().expect("children changed");
        assert_eq!(old.to_vec(), vec![refno(10)]);
        assert_eq!(new.to_vec(), vec![refno(12)]);
    }

    #[test]
    fn a_window_without_repeats_is_left_alone() {
        let w = window(vec![
            op(1, 1, blank()),
            op(2, 1, blank()),
            op(3, 2, blank()),
        ]);
        let planned = fold_window(&w);

        assert_eq!(planned.len(), 3);
        assert!(planned.iter().all(|w| w.folded.is_none()));
    }

    /// Folding must not change how many statement groups one write renders to.
    #[test]
    fn a_merged_write_still_renders_one_statement_group() {
        let mut first = blank();
        first.added_attrs.insert("XLEN".into(), text("1"));
        let mut second = blank();
        second.added_attrs.insert("YLEN".into(), text("2"));

        let w = window(vec![op(1, 1, first), op(1, 2, second)]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);
        let sql = folded.to_modify_surql("7997_1", 2);

        assert_eq!(sql.matches("UPSERT DAMP:7997_1 MERGE").count(), 1, "{sql}");
        assert_eq!(sql.matches("UPDATE pe:7997_1 SET").count(), 1, "{sql}");
        assert!(sql.contains("XLEN"), "{sql}");
        assert!(sql.contains("YLEN"), "{sql}");
    }

    /// A merged statement must reproduce the state the replayed sequence left
    /// behind, key for key.
    #[test]
    fn merging_is_equivalent_to_replaying_the_sequence() {
        let mut first = blank();
        first.added_attrs.insert("A".into(), text("1"));
        first.added_attrs.insert("B".into(), text("1"));
        let mut second = blank();
        second.deleted_attrs.insert("A".into(), text("1"));
        let mut third = blank();
        third.added_attrs.insert("A".into(), text("3"));
        third
            .modified_attrs
            .insert("B".into(), (text("1"), text("3")));

        let w = window(vec![
            op(1, 1, first.clone()),
            op(1, 2, second.clone()),
            op(1, 3, third.clone()),
        ]);
        let planned = fold_window(&w);
        let folded = folded_at(&planned, 1);

        // Replay the same sequence the way `to_modify_surql` resolves each op.
        let mut replayed: HashMap<String, Option<NamedAttrValue>> = HashMap::new();
        for element in [&first, &second, &third] {
            for (k, v) in &element.added_attrs {
                replayed.insert(k.clone(), Some(v.clone()));
            }
            for (k, (_, v)) in &element.modified_attrs {
                replayed.insert(k.clone(), Some(v.clone()));
            }
            for k in element.deleted_attrs.keys() {
                replayed.insert(k.clone(), None);
            }
        }

        let mut merged: HashMap<String, Option<NamedAttrValue>> = HashMap::new();
        for (k, v) in &folded.added_attrs {
            merged.insert(k.clone(), Some(v.clone()));
        }
        for (k, (_, v)) in &folded.modified_attrs {
            merged.insert(k.clone(), Some(v.clone()));
        }
        for k in folded.deleted_attrs.keys() {
            merged.insert(k.clone(), None);
        }

        assert_eq!(merged, replayed);
    }

    /// The state one refno ends in after a window, modelled independently of how
    /// [`fold_window`] detects runs: an `Add` replaces the record wholesale, a
    /// `Deleted` lays a tombstone without clearing attributes, and a `Modified`
    /// applies its three delta maps in the order `to_modify_surql` resolves them.
    #[derive(Default, Clone, PartialEq, Debug)]
    struct FinalState {
        attrs: std::collections::BTreeMap<String, Option<NamedAttrValue>>,
        explicit: std::collections::BTreeMap<String, Option<NamedAttrValue>>,
        uda: std::collections::BTreeMap<i32, Option<NamedAttrValue>>,
        children: Option<Vec<RefU64>>,
        tombstoned: bool,
        recreated: usize,
    }

    impl FinalState {
        fn apply(&mut self, detail: &EleOperationDetail) {
            match detail {
                EleOperationDetail::Add(_) => {
                    let recreated = self.recreated + 1;
                    *self = FinalState::default();
                    self.recreated = recreated;
                }
                EleOperationDetail::Deleted => self.tombstoned = true,
                EleOperationDetail::None => {}
                EleOperationDetail::Modified(m) => {
                    for (k, v) in &m.added_attrs {
                        self.attrs.insert(k.clone(), Some(v.clone()));
                    }
                    for (k, (_, v)) in &m.modified_attrs {
                        self.attrs.insert(k.clone(), Some(v.clone()));
                    }
                    for k in m.deleted_attrs.keys() {
                        self.attrs.insert(k.clone(), None);
                    }
                    for (k, v) in &m.added_explicit_attrs {
                        self.explicit.insert(k.clone(), Some(v.clone()));
                    }
                    for (k, (_, v)) in &m.modified_explicit_attrs {
                        self.explicit.insert(k.clone(), Some(v.clone()));
                    }
                    for k in m.deleted_explicit_attrs.keys() {
                        self.explicit.insert(k.clone(), None);
                    }
                    for (k, v) in &m.added_uda_attrs {
                        self.uda.insert(*k, Some(v.clone()));
                    }
                    for (k, (_, v)) in &m.modified_uda_attrs {
                        self.uda.insert(*k, Some(v.clone()));
                    }
                    for k in m.deleted_uda_attrs.keys() {
                        self.uda.insert(*k, None);
                    }
                    if let Some((_, new)) = &m.children_changed {
                        self.children = Some(new.to_vec());
                    }
                }
            }
        }
    }

    /// Folding is only safe if a real window ends in exactly the state its
    /// unfolded replay would. Ten hand-built cases cannot cover the attribute
    /// sequences a 169-session cold start actually produces, so this replays both
    /// forms over a real E3D file and compares them refno by refno.
    ///
    /// Needs only the file — no SurrealDB. Run it in release; a debug build parses
    /// the same window roughly 90x slower.
    ///
    /// ```text
    /// $env:AIOS_FOLD_TEST_FILE = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys"
    /// cargo test --release --lib -- folding_a_real_window_preserves_final_state --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "manual: folds a real E3D window and checks it against an unfolded replay"]
    fn folding_a_real_window_preserves_final_state() {
        use std::collections::BTreeMap as Map;

        let file = std::env::var("AIOS_FOLD_TEST_FILE").expect("set AIOS_FOLD_TEST_FILE");
        let to: i32 = std::env::var("AIOS_FOLD_TEST_TO")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(169);

        let range_eles = IncrementPipeline::collect_changes(std::path::Path::new(&file), 1..=to)
            .expect("collect changes");

        let mut before: Map<RefU64, FinalState> = Map::new();
        let mut raw_ops = 0usize;
        for elements in range_eles.values() {
            for op in elements {
                raw_ops += 1;
                before.entry(op.refno).or_default().apply(&op.detail);
            }
        }

        let planned = fold_window(&range_eles);
        let mut after: Map<RefU64, FinalState> = Map::new();
        for write in &planned {
            let entry = after.entry(write.op.refno).or_default();
            match &write.folded {
                Some(folded) => entry.apply(&EleOperationDetail::Modified(folded.clone())),
                None => entry.apply(&write.op.detail),
            }
        }

        println!(
            "折叠 {} 个操作 → {} 个（省 {:.1}%），覆盖 {} 个 refno",
            raw_ops,
            planned.len(),
            (raw_ops - planned.len()) as f64 / raw_ops.max(1) as f64 * 100.0,
            before.len()
        );

        assert_eq!(
            before.len(),
            after.len(),
            "a refno went missing while folding"
        );
        for (refno, expected) in &before {
            assert_eq!(
                after.get(refno),
                Some(expected),
                "refno {refno:?} ends in a different state after folding"
            );
        }
        assert!(planned.len() < raw_ops, "this window folded nothing");
    }
}

#[cfg(test)]
mod bench_tests {
    use super::*;
    use surrealdb::Surreal;
    use surrealdb::engine::any::Any;
    use surrealdb::opt::auth::Root;

    /// Exactly what `persist_latest_main_data` used to emit, before folding.
    fn render_unfolded(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        dbnum: i32,
    ) -> Vec<String> {
        let mut statements = Vec::new();
        for (&sesno, elements) in range_eles {
            for element in elements {
                let surql = element.to_surql(&element.refno.to_string(), dbnum, sesno);
                if !surql.is_empty() {
                    statements.push(surql);
                }
            }
        }
        statements
    }

    /// What it emits now.
    fn render_folded(range_eles: &BTreeMap<u32, Vec<EleOperationData>>, dbnum: i32) -> Vec<String> {
        let mut statements = Vec::new();
        for write in fold_window(range_eles) {
            let id = write.op.refno.to_string();
            let surql = match &write.folded {
                Some(folded) => folded.to_modify_surql(&id, write.sesno),
                None => write.op.to_surql(&id, dbnum, write.sesno),
            };
            if !surql.is_empty() {
                statements.push(surql);
            }
        }
        statements
    }

    async fn throwaway(endpoint: &str, db_name: &str) -> Surreal<Any> {
        let db: Surreal<Any> = Surreal::init();
        db.connect(endpoint)
            .with_capacity(1000)
            .await
            .expect("connect throwaway instance");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("sign in");
        db.use_ns("bench").use_db(db_name).await.expect("use ns/db");
        db
    }

    /// Replay the statements the way the pipeline does: 500 per atomic
    /// transaction, awaited in order.
    async fn replay(db: &Surreal<Any>, statements: &[String]) -> Duration {
        const TX_CHUNK: usize = 500;
        let start = Instant::now();
        for chunk in statements.chunks(TX_CHUNK) {
            if let Some(tx_sql) = wrap_in_transaction(chunk) {
                db.query(&tx_sql)
                    .await
                    .expect("bench transport")
                    .check()
                    .expect("bench statements");
            }
        }
        start.elapsed()
    }

    /// What folding is actually worth at the database, measured by replaying one
    /// real window in both forms.
    ///
    /// Deliberately NOT `SUL_DB`: that global points at the configured working
    /// project and a benchmark must never write there. This opens its own client
    /// against a throwaway instance and refuses to run against the configured port.
    ///
    /// ```text
    /// bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8099 memory
    /// $env:AIOS_FOLD_TEST_FILE = "…\ams000\amssys"
    /// cargo test --release --lib -- persist_ab_on_a_throwaway_instance --ignored --nocapture
    /// ```
    ///
    /// An empty in-memory instance has no existing rows and no indexes, so read
    /// the ratio between the two runs, not the absolute milliseconds.
    #[tokio::test]
    #[ignore = "manual bench: needs a throwaway SurrealDB on 127.0.0.1:8099"]
    async fn persist_ab_on_a_throwaway_instance() {
        let file = std::env::var("AIOS_FOLD_TEST_FILE").expect("set AIOS_FOLD_TEST_FILE");
        let endpoint = std::env::var("AIOS_BENCH_ENDPOINT")
            .unwrap_or_else(|_| "ws://127.0.0.1:8099".to_string());
        assert!(
            !endpoint.contains(":8009"),
            "refusing to benchmark against the configured working database"
        );
        let to: i32 = std::env::var("AIOS_FOLD_TEST_TO")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(169);
        let dbnum: i32 = std::env::var("AIOS_FOLD_TEST_DBNUM")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8191);

        let range_eles = IncrementPipeline::collect_changes(std::path::Path::new(&file), 1..=to)
            .expect("collect changes");

        let unfolded = render_unfolded(&range_eles, dbnum);
        let folded = render_folded(&range_eles, dbnum);
        let bytes =
            |s: &[String]| s.iter().map(String::len).sum::<usize>() as f64 / 1024.0 / 1024.0;

        println!(
            "未折叠 {} 条 / {:.2} MB   折叠后 {} 条 / {:.2} MB",
            unfolded.len(),
            bytes(&unfolded),
            folded.len(),
            bytes(&folded)
        );

        // Alternate the two forms so a warm-up or ordering effect shows up as a
        // gap between the two rounds rather than hiding inside one of them.
        for round in 1..=2 {
            let db = throwaway(&endpoint, &format!("unfolded_{round}")).await;
            let a = replay(&db, &unfolded).await;
            let db = throwaway(&endpoint, &format!("folded_{round}")).await;
            let b = replay(&db, &folded).await;
            println!(
                "第 {round} 轮: 未折叠 {}ms  折叠后 {}ms  省 {:.1}%",
                a.as_millis(),
                b.as_millis(),
                (a.as_secs_f64() - b.as_secs_f64()) / a.as_secs_f64() * 100.0
            );
        }
    }
}

#[cfg(test)]
mod datacenter_tests {
    use super::*;
    use pdms_io::io::ModifiedElement;

    fn modified(noun: &str) -> EleOperationData {
        EleOperationData::new(
            RefU64((7997u64 << 32) | 1),
            1,
            EleOperationDetail::Modified(ModifiedElement {
                current_data: Default::default(),
                added_attrs: Default::default(),
                deleted_attrs: Default::default(),
                modified_attrs: Default::default(),
                added_explicit_attrs: Default::default(),
                deleted_explicit_attrs: Default::default(),
                modified_explicit_attrs: Default::default(),
                added_uda_attrs: Default::default(),
                deleted_uda_attrs: Default::default(),
                modified_uda_attrs: Default::default(),
                noun: noun.to_string(),
                children_changed: None,
            }),
        )
    }

    type ChainMap = std::collections::HashMap<RefU64, (Option<String>, Option<RefU64>)>;

    fn refu(n: u64) -> RefU64 {
        RefU64((7997u64 << 32) | n)
    }

    fn chain_loader(
        map: ChainMap,
    ) -> impl FnMut(Vec<RefU64>) -> std::future::Ready<anyhow::Result<ChainMap>> {
        move |refnos| {
            std::future::ready(Ok(refnos
                .into_iter()
                .filter_map(|refno| map.get(&refno).cloned().map(|entry| (refno, entry)))
                .collect()))
        }
    }

    async fn render_with(ops: Vec<EleOperationData>, chain: ChainMap) -> Vec<String> {
        let unit_types = crate::data_interface::generation_root::resolve_delivery_unit_types(&[]);
        IncrementPipeline::resolve_datacenter_statements_with(
            &BTreeMap::from([(1u32, ops)]),
            &unit_types,
            chain_loader(chain),
        )
        .await
        .expect("resolve datacenter statements")
    }

    /// 走链兜底用的窗口前持久态：1 → 6(PIPE) → 5(ZONE) → 4(SITE)，旁支 2 → 5。
    fn pre_window_chain() -> ChainMap {
        ChainMap::from([
            (refu(1), (Some("FTUB".into()), Some(refu(6)))),
            (refu(2), (Some("DAMP".into()), Some(refu(5)))),
            (refu(6), (Some("PIPE".into()), Some(refu(5)))),
            (refu(5), (Some("ZONE".into()), Some(refu(4)))),
            (refu(4), (Some("SITE".into()), None)),
        ])
    }

    /// Chunks are concatenated verbatim, so an unterminated statement would
    /// silently merge into its neighbour.
    #[tokio::test(flavor = "multi_thread")]
    async fn every_statement_is_self_terminated_so_a_chunk_can_be_concatenated() {
        let statements = render_with(
            vec![
                modified("BRAN"),
                modified("DAMP"),
                EleOperationData::new(refu(2), 1, EleOperationDetail::Deleted),
            ],
            pre_window_chain(),
        )
        .await;

        assert_eq!(statements.len(), 3);
        for statement in &statements {
            assert!(statement.ends_with(';'), "{statement}");
        }
    }

    /// W3（D5）：单元层名词直接标自己；其余名词的上溯在**渲染时**解出固定目标 id
    /// ——产物里再无 `fn::find_ancestor_types` / `$pe` 现场求值（回退即红）。
    #[tokio::test(flavor = "multi_thread")]
    async fn delivery_unit_nouns_update_directly_and_others_roll_up_to_an_ancestor() {
        for noun in ["BRAN", "HANG", "SUPPO", "EQUI"] {
            let unit = render_with(vec![modified(noun)], ChainMap::new())
                .await
                .remove(0);
            assert_eq!(
                unit, "update datacenter_version:7997_1 set status = 'Modify';",
                "{noun}"
            );
        }

        // FTUB(1) → PIPE(6) → ZONE(5)：解到最近的单元层（ZONE），渲染成固定目标。
        let nested = render_with(vec![modified("FTUB")], pre_window_chain())
            .await
            .remove(0);
        assert_eq!(
            nested,
            "update datacenter_version:7997_5 set status = 'Modify';"
        );
    }

    /// 上溯优先吃**窗口净态**（overlay）：元素在本窗口搬了家，rollup 必须按新
    /// OWNER 解——与老形态 commit-time 对重放后持久层求值同一语义。
    #[tokio::test(flavor = "multi_thread")]
    async fn rollup_follows_the_windows_own_owner_moves() {
        let mut op = modified("FTUB");
        if let EleOperationDetail::Modified(m) = &mut op.detail {
            // 窗口把 1 从 PIPE(6) 搬到 ZONE(7)——净态 owner 以 current_data 为准。
            m.current_data.owner = refu(7);
        }
        let mut chain = pre_window_chain();
        chain.insert(refu(7), (Some("ZONE".into()), Some(refu(4))));

        let statement = render_with(vec![op], chain).await.remove(0);
        assert_eq!(
            statement, "update datacenter_version:7997_7 set status = 'Modify';",
            "rollup 必须走窗口内的新 OWNER，而不是窗口前持久态"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deletions_record_the_owning_zone_and_no_ops_render_nothing() {
        // 非 BRAN：belong_zone = owner。
        let deleted = render_with(
            vec![EleOperationData::new(
                refu(2),
                1,
                EleOperationDetail::Deleted,
            )],
            pre_window_chain(),
        )
        .await
        .remove(0);
        assert_eq!(
            deleted,
            "update datacenter_version:7997_2 set status = 'Delete', belong_zone = pe:7997_5;"
        );

        // BRAN：归属 ZONE 隔一层 PIPE（老形态的 $pe.owner.owner）。
        let mut chain = pre_window_chain();
        chain.insert(refu(3), (Some("BRAN".into()), Some(refu(6))));
        let deleted_bran = render_with(
            vec![EleOperationData::new(
                refu(3),
                1,
                EleOperationDetail::Deleted,
            )],
            chain,
        )
        .await
        .remove(0);
        assert_eq!(
            deleted_bran,
            "update datacenter_version:7997_3 set status = 'Delete', belong_zone = pe:7997_5;"
        );

        // 链解不出（行不在持久层也不在窗口里）：belong_zone 显式 NONE。
        let orphan = render_with(
            vec![EleOperationData::new(
                refu(9),
                1,
                EleOperationDetail::Deleted,
            )],
            ChainMap::new(),
        )
        .await
        .remove(0);
        assert_eq!(
            orphan,
            "update datacenter_version:7997_9 set status = 'Delete', belong_zone = NONE;"
        );

        let noop = render_with(
            vec![EleOperationData::new(refu(3), 1, EleOperationDetail::None)],
            ChainMap::new(),
        )
        .await;
        assert!(noop.is_empty(), "{noop:?}");
    }

    /// 「回退即红」源码钉（W3）：渲染产物是纯数据 UPDATE——出现任何
    /// `fn::` 调用或 `$pe` 现场求值即红。上溯解不出单元层归属的元素（SITE 自身
    /// 的属性修改）不渲染语句——老形态那是 `$pe = NONE` 塞进 `type::thing` 的
    /// 未定义角落。
    #[tokio::test(flavor = "multi_thread")]
    async fn resolved_statements_carry_no_server_side_walks() {
        let mut chain = pre_window_chain();
        chain.insert(refu(8), (Some("SITE".into()), None));
        let statements = render_with(
            vec![
                modified("FTUB"),
                // BRAN 放在独立 refno 上：`modified()` 的缺省 refno 与 FTUB 撞在
                // 一起时，两个 op 会渲出两条一模一样的语句——老断言把那对纯重复
                // 数成 3，恰是保尾去重要消灭的形态（2026-08-10 审核 P1）。
                {
                    let mut op = modified("BRAN");
                    op.refno = refu(10);
                    op
                },
                EleOperationData::new(refu(2), 1, EleOperationDetail::Deleted),
                EleOperationData::new(refu(3), 1, EleOperationDetail::None),
                {
                    let mut op = modified("SITE");
                    op.refno = refu(8);
                    op
                },
            ],
            chain,
        )
        .await;

        assert_eq!(
            statements.len(),
            3,
            "SITE 自身修改解不出单元层归属，必须跳过而不是渲出 NONE 目标: {statements:?}"
        );
        for statement in &statements {
            assert!(
                !statement.contains("fn::") && !statement.contains("$pe"),
                "收口语句必须是固定目标 id 的纯 UPDATE（D5 回退即红）: {statement}"
            );
        }
    }

    /// 语义等价对拍：固定目标 UPDATE 与老形态的服务端现场上溯，落在**同一批**
    /// `datacenter_version` 行上、写出同一份终态。两个 mem 库同种同一棵 pe 链与
    /// 交付记录，一边跑老模板（`fn::find_ancestor_types` + `type::thing`），
    /// 一边跑新渲染，逐行对比。
    #[tokio::test(flavor = "multi_thread")]
    async fn fixed_target_updates_hit_the_rows_the_server_side_walk_hit() {
        use surrealdb::engine::any::connect;

        async fn seeded_db(name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
            let db = connect("mem://").await.expect("mem boots");
            db.use_ns("dc_parity").use_db(name).await.expect("use db");
            crate::data_interface::staging::lifecycle::init_staging_schema(&db)
                .await
                .expect("schema + fn definitions");
            db.query(
                "UPSERT pe:7997_4 CONTENT { noun: 'SITE' };\
                 UPSERT pe:7997_5 CONTENT { noun: 'ZONE', owner: pe:7997_4 };\
                 UPSERT pe:7997_6 CONTENT { noun: 'PIPE', owner: pe:7997_5 };\
                 UPSERT pe:7997_1 CONTENT { noun: 'FTUB', owner: pe:7997_6 };\
                 UPSERT pe:7997_3 CONTENT { noun: 'BRAN', owner: pe:7997_6 };\
                 UPSERT datacenter_version:7997_5 CONTENT { status: 'Publish' };\
                 UPSERT datacenter_version:7997_3 CONTENT { status: 'Publish' };",
            )
            .await
            .expect("seed transport")
            .check()
            .expect("seeded");
            db
        }

        // 老形态（W3 之前的模板，原样手抄）。
        let legacy = seeded_db("legacy").await;
        legacy
            .query(
                "let $pe = fn::find_ancestor_types(pe:7997_1,['BRAN','HANG','SUPPO','EQUI','ZONE'])[0];\
                 update type::thing('datacenter_version',$pe) set status = 'Modify';",
            )
            .await
            .expect("legacy modify transport")
            .check()
            .expect("legacy modify");
        legacy
            .query(
                "let $pe = pe:7997_3;\
                 let $belong_zone = if $pe.noun == 'BRAN' { $pe.owner.owner } else { $pe.owner };\
                 update type::thing('datacenter_version',$pe) set status = 'Delete',belong_zone = $belong_zone;",
            )
            .await
            .expect("legacy delete transport")
            .check()
            .expect("legacy delete");

        // 新形态：同一窗口 ops 经 resolve-then-render，loader 直读同种子的库
        // （模拟窗口前持久态点查）。
        let resolved = seeded_db("resolved").await;
        let loader_db = resolved.clone();
        let statements = IncrementPipeline::resolve_datacenter_statements_with(
            &BTreeMap::from([(
                1u32,
                vec![
                    modified("FTUB"),
                    EleOperationData::new(refu(3), 1, EleOperationDetail::Deleted),
                ],
            )]),
            &crate::data_interface::generation_root::resolve_delivery_unit_types(&[]),
            move |refnos| {
                let db = loader_db.clone();
                async move {
                    #[derive(serde::Deserialize)]
                    struct Row {
                        id: aios_core::RefnoEnum,
                        #[serde(default)]
                        noun: Option<String>,
                        #[serde(default)]
                        owner: Option<aios_core::RefnoEnum>,
                    }
                    let keys = refnos
                        .iter()
                        .map(|refno| refno.to_pe_key())
                        .collect::<Vec<_>>()
                        .join(",");
                    let mut response = db
                        .query(format!(
                            "SELECT id, noun, owner FROM [{keys}] WHERE record::exists(id);"
                        ))
                        .await?
                        .check()?;
                    let rows: Vec<Row> = response.take(0)?;
                    Ok(rows
                        .into_iter()
                        .map(|row| (row.id.refno(), (row.noun, row.owner.map(|o| o.refno()))))
                        .collect())
                }
            },
        )
        .await
        .expect("resolve");
        for statement in &statements {
            resolved
                .query(statement)
                .await
                .expect("resolved transport")
                .check()
                .expect("resolved statement");
        }

        let snapshot = |db: surrealdb::Surreal<surrealdb::engine::any::Any>| async move {
            let mut response = db
                .query("SELECT * FROM datacenter_version ORDER BY id;")
                .await
                .expect("snapshot transport")
                .check()
                .expect("snapshot");
            let rows: surrealdb::Value = response.take(0).expect("rows");
            serde_json::to_string(&rows).expect("serialize")
        };
        assert_eq!(
            snapshot(legacy).await,
            snapshot(resolved).await,
            "固定目标 UPDATE 必须与服务端现场上溯落出同一份 datacenter_version 终态"
        );
    }

    /// 层级查询优化 P1 的搬家维护：OWNER 变化的元素（Moved 桶）必须渲出
    /// inst_relate / tubi_relate 各一条 anc 定点重算语句进 finalize
    /// 事务；普通属性变化、新建、删除不产生重算（它们各自的重生成路径已自洽）。
    /// P3 之后重算不再带 `zone_refno`（列已退役）——回退即红。
    #[test]
    fn owner_moves_render_anc_repair_statements_and_others_do_not() {
        use aios_core::NamedAttrValue;

        let mut op = modified("PIPE");
        if let EleOperationDetail::Modified(m) = &mut op.detail {
            m.modified_attrs.insert(
                "OWNER".into(),
                (
                    NamedAttrValue::RefU64Type(RefU64((7997u64 << 32) | 10)),
                    NamedAttrValue::RefU64Type(RefU64((7997u64 << 32) | 20)),
                ),
            );
        }
        let moved_elem = RefU64((7997u64 << 32) | 1);
        // 同一元素出现两次（两个 sesno 都搬）也只修一次。
        let range = BTreeMap::from([(1u32, vec![op.clone()]), (2u32, vec![op])]);
        let moved = IncrementPipeline::moved_refnos(&range);
        assert_eq!(moved, vec![moved_elem]);

        let statements = IncrementPipeline::render_anc_repair_statements(&moved);
        assert_eq!(statements.len(), 2, "{statements:?}");
        for (statement, table) in statements.iter().zip(["inst_relate", "tubi_relate"]) {
            assert!(statement.contains(table), "{statement}");
            assert!(
                statement.contains(&format!("anc CONTAINSANY [{}]", moved_elem.0)),
                "受影响行由旧 anc 含搬家元素这一个条件圈出: {statement}"
            );
            assert!(
                statement.contains("anc = fn::anc_u64(in)"),
                "anc 必须按提交后的活 owner 链重算: {statement}"
            );
            assert!(
                !statement.contains("zone_refno"),
                "zone_refno 已退役，重算语句不得再写它（P3 回退即红）: {statement}"
            );
            assert!(
                statement.ends_with(';'),
                "chunk 拼接依赖自终止: {statement}"
            );
        }

        // 非搬家操作：普通属性变化 / 新建 / 删除 → 无修补语句。
        let range = BTreeMap::from([(
            1u32,
            vec![
                modified("DAMP"),
                EleOperationData::new(RefU64((7997u64 << 32) | 2), 1, EleOperationDetail::Deleted),
            ],
        )]);
        assert!(IncrementPipeline::moved_refnos(&range).is_empty());
        assert!(IncrementPipeline::render_anc_repair_statements(&[]).is_empty());
    }

    /// 2026-08-10 审核 P1：整批搬家元素必须合并进 CONTAINSANY——逐元素渲染是
    /// 每个元素两次子查询扫描，容器大搬移一次上千元素就是收口路径上的数千次
    /// 扫描。分块上界之内每表恰好一条语句；越界按块翻倍。
    #[test]
    fn a_batch_of_moves_merges_into_one_containsany_statement_per_table() {
        let moved = (1..=3)
            .map(|n| RefU64((7997u64 << 32) | n))
            .collect::<Vec<_>>();
        let statements = IncrementPipeline::render_anc_repair_statements(&moved);
        assert_eq!(
            statements.len(),
            2,
            "块内的搬家元素合并为每表一条: {statements:?}"
        );
        let list = moved
            .iter()
            .map(|refno| refno.0.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for statement in &statements {
            assert!(
                statement.contains(&format!("anc CONTAINSANY [{list}]")),
                "{statement}"
            );
        }

        let oversized = (1..=(IncrementPipeline::ANC_REPAIR_CHUNK as u64 + 1))
            .map(|n| RefU64((7997u64 << 32) | n))
            .collect::<Vec<_>>();
        assert_eq!(
            IncrementPipeline::render_anc_repair_statements(&oversized).len(),
            4,
            "超过分块上界后按块翻倍（语句体积有界）"
        );
    }

    /// 2026-08-10 审核 P1：同一元素在窗口里被改 N 次，datacenter 语句只留一条
    /// ——收口体积必须 ∝ 元素数而不是 ∝ 操作数（主数据那侧的 fold 早已如此）。
    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_modifies_of_one_target_render_one_statement() {
        let statements = render_with(
            vec![modified("BRAN"), modified("BRAN"), modified("BRAN")],
            ChainMap::new(),
        )
        .await;
        assert_eq!(
            statements,
            vec!["update datacenter_version:7997_1 set status = 'Modify';".to_string()],
            "重复触发同一目标的语句是纯重复，必须收敛成一条"
        );
    }

    /// 去重必须**保尾**：改 → 删 → 重建再改（M、D、M）里保头会把最后那条 M
    /// 丢掉、终态错成 Delete；保尾得到 D、M，与逐条重放同一个终态。
    #[test]
    fn dedup_keeps_the_last_occurrence_so_replay_order_survives() {
        let modify = "update datacenter_version:7997_1 set status = 'Modify';".to_string();
        let delete = "update datacenter_version:7997_1 set status = 'Delete', belong_zone = NONE;"
            .to_string();

        let deduped =
            dedup_statements_keep_last(vec![modify.clone(), delete.clone(), modify.clone()]);
        assert_eq!(deduped, vec![delete, modify], "最后写入者必须活下来");

        assert!(dedup_statements_keep_last(Vec::new()).is_empty());
        let distinct = vec!["a;".to_string(), "b;".to_string()];
        assert_eq!(
            dedup_statements_keep_last(distinct.clone()),
            distinct,
            "无重复时原序原样"
        );
    }

    /// P2（2026-08-07 审核）：anc 修复只对 DESI 窗口渲染——CATA/SYST/DICT 元素的
    /// refno 不会出现在任何 anc 里，渲出来的 UPDATE 全是收口事务里的空转子查询
    /// 扫描（目录重组一次搬上千元素时把收口拖慢一个量级），且其 `fn::anc_u64`
    /// 依赖只受 DESI 预检保护。回退（去掉 db_type 门）即红。
    #[test]
    fn anc_repair_is_rendered_for_desi_windows_only() {
        use aios_core::NamedAttrValue;

        let mut op = modified("PIPE");
        if let EleOperationDetail::Modified(m) = &mut op.detail {
            m.modified_attrs.insert(
                "OWNER".into(),
                (
                    NamedAttrValue::RefU64Type(RefU64((7997u64 << 32) | 10)),
                    NamedAttrValue::RefU64Type(RefU64((7997u64 << 32) | 20)),
                ),
            );
        }
        let range = BTreeMap::from([(1u32, vec![op])]);

        assert_eq!(
            IncrementPipeline::anc_repair_statements_for_window(&range, "DESI").len(),
            2,
            "DESI 窗口的搬迁必须渲出 inst_relate/tubi_relate 各一条重算"
        );
        for db_type in ["CATA", "SYST", "DICT"] {
            assert!(
                IncrementPipeline::anc_repair_statements_for_window(&range, db_type).is_empty(),
                "{db_type} 窗口不得渲染 anc 修复语句（anc 只含 DESI 链）"
            );
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::data_interface::tidb_manager::AiosDBManager;

    /// Manual: requires local Surreal `ws://127.0.0.1:8009` + E3D project files.
    /// Example: lower `dbnum_watermark:8191` then
    /// `cargo test -p aios-database force_init_watcher_incr_once -- --ignored --nocapture`
    ///
    /// 合流后 `init_watcher` 只重扫入队（ADR-011 §4），要真把增量应用掉还得
    /// 跑一遍与 worker 相同的消费循环。
    #[tokio::test]
    #[ignore = "manual live incr against local Surreal/E3D"]
    async fn force_init_watcher_incr_once() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let mgr = std::sync::Arc::new(AiosDBManager::init_form_config().await.expect("init mgr"));
        mgr.init_watcher().await.expect("init_watcher");
        let ran = crate::data_interface::batch_worker::drain_queue_until_empty(&mgr).await;
        println!("consumed {ran} queued batch task(s)");
    }

    /// F4 · T403（live）：同一窗口重放 `Add` 的 `pe_owner` 写必须收敛。
    ///
    /// 落库按 `TX_CHUNK` 分块提交、整窗口非单事务，早块提交后块失败时 ADR-001 要求按
    /// 同一 sesno 窗口重放。裸 `INSERT RELATION` 会撞上已存在的复合 id `[pe:{id}, i]`
    /// 反复报错、把该 dbnum 的水位卡死；语句先删本元素入向边才收敛。
    ///
    /// 这里只回放渲染结果中的 `pe_owner` 语句：F4 的主张就落在这两句上，剥掉 pe / noun
    /// 主记录的 UPSERT 可以让断言不依赖属性载荷的完整度。语句取自真实的
    /// `to_surql` 输出而非手写，避免测试和实现各写一份 SQL 而漂移。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_add_pe_owner_replay_is_idempotent() {
        use aios_core::NamedAttrValue;
        use parse_pdms_db::parse::EleData;
        use pdms_io::io::EleOperationDetail;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let mut ele = EleData::default();
        ele.whole_attmap.attmap.map.insert(
            "TYPE".to_string(),
            NamedAttrValue::StringType("BOX".to_string()),
        );
        // DBNUM 只影响 pe / noun 主记录的 JSON（本例过滤掉不回放），取常规值即可；
        // 4000000003 这类保留段号超出 i32，不能直接放进 IntegerType。
        ele.whole_attmap
            .attmap
            .map
            .insert("DBNUM".to_string(), NamedAttrValue::IntegerType(7997));
        ele.children = RefU64Vec(vec![
            RefU64((4000000003u64 << 32) | 11),
            RefU64((4000000003u64 << 32) | 12),
        ]);

        let rendered = EleOperationDetail::Add(ele).to_surql("4000000003_10", 7997, 7);
        let relate_sql = rendered
            .lines()
            .filter(|line| line.contains("pe_owner"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            relate_sql.contains("DELETE") && relate_sql.contains("INSERT RELATION"),
            "渲染结果里应同时有 DELETE 与 INSERT RELATION:\n{rendered}"
        );

        let cleanup = "delete pe:4000000003_10, pe:4000000003_11, pe:4000000003_12;";
        let setup = format!(
            "{cleanup}
            create pe:4000000003_10;
            create pe:4000000003_11;
            create pe:4000000003_12;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create replay fixture")
            .check()
            .expect("valid setup");

        // 第一次：正常写入。第二次：模拟同窗口重放——必须同样成功。
        for attempt in 1..=2 {
            SUL_DB
                .query(&relate_sql)
                .await
                .unwrap_or_else(|e| panic!("第 {attempt} 次执行关系语句失败: {e}"))
                .check()
                .unwrap_or_else(|e| panic!("第 {attempt} 次关系语句返回错误: {e}"));
        }

        let mut response = SUL_DB
            .query("SELECT VALUE id FROM pe_owner WHERE out = pe:4000000003_10;")
            .await
            .expect("count relations")
            .check()
            .expect("valid count query");
        let count = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .expect("decode relation ids")
            .len();

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup replay fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(count, 2, "重放后 children 关系应恰好各一条，不得重复累积");
    }

    /// Real-file E2E: load the backup baseline, apply the current file's real
    /// FTUB + transient Add→Deleted window, skip the net-zero model work, then
    /// generate every affected root. The E3D source files are read-only.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: requires isolated Surreal on :8009 and local AMS files"]
    async fn live_real_ftub_delete_move_and_reorder() {
        use crate::data_interface::dbnum_state::{DbnumState, FileObservation};
        use crate::data_interface::generation_root::{
            configured_delivery_unit_types, resolve_live_element_generation_root,
        };
        use crate::data_interface::manual_update::load_pending_model_units;
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::model_update_pending::{drain, drain_non_regen};
        use crate::data_interface::model_update_plan::ModelWorkAction;
        use crate::versioned_db::database::sync_total_async_threaded;
        use aios_core::NamedAttrValue;
        use dashmap::DashSet;
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};

        let current = PathBuf::from(std::env::var("AIOS_FTUB_CURRENT_FILE").unwrap_or_else(|_| {
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
        }));
        let backup = PathBuf::from(std::env::var("AIOS_FTUB_BASELINE_FILE").unwrap_or_else(|_| {
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001.codex-before-move-20260724"
                .into()
        }));
        assert!(
            current.is_file(),
            "missing current fixture: {}",
            current.display()
        );
        assert!(
            backup.is_file(),
            "missing baseline fixture: {}",
            backup.display()
        );

        aios_core::init_test_surreal()
            .await
            .expect("connect isolated surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init current-project manager");

        let fixture_project = "AiosFtubIncrementFixture";
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let fixture_root = std::env::temp_dir().join(format!(
            "aios-ftub-increment-{}-{unique}",
            std::process::id()
        ));
        let fixture_db_dir = fixture_root
            .join(fixture_project)
            .join(format!("{fixture_project}000"));
        std::fs::create_dir_all(&fixture_db_dir).expect("create baseline fixture directory");
        std::fs::copy(&backup, fixture_db_dir.join("ams8000_0001"))
            .expect("copy read-only baseline fixture");

        let mut baseline_options = manager.db_option.clone();
        baseline_options.project_path = fixture_root.to_string_lossy().into_owned();
        baseline_options.included_projects = vec![fixture_project.into()];
        baseline_options.project_dirs = None;
        baseline_options.total_sync = true;
        baseline_options.replace_dbs = false;
        baseline_options.included_db_files = Some(vec!["ams8000_0001".into()]);
        baseline_options.manual_db_nums = Some(vec![8000]);
        baseline_options.gen_model = false;
        baseline_options.gen_mesh = false;

        let parsed = sync_total_async_threaded(
            &baseline_options,
            fixture_project,
            Arc::new(DashSet::new()),
            &["DESI"],
            100,
        )
        .await
        .expect("load sesno-15 baseline");
        assert!(
            parsed.get(&8000).copied().unwrap_or_default() > 0,
            "baseline dbnum=8000 must contain elements"
        );

        let branch = "24384/22402";
        let deleted = RefnoEnum::from("24384/30939");
        let baseline_root =
            resolve_live_element_generation_root(deleted, &configured_delivery_unit_types())
                .await
                .expect("inspect element before applying its tombstone")
                .expect("baseline FTUB must resolve to its owning BRAN");
        assert_eq!(
            (
                baseline_root.root.to_pdms_str(),
                baseline_root.noun.as_str()
            ),
            (branch.to_string(), "BRAN"),
            "an Add→Deleted window only cancels when the element did not exist at baseline"
        );
        let mut baseline_io = PdmsIO::new("", backup.clone(), true);
        baseline_io.open().expect("open baseline file");
        let baseline_sesno = i32::try_from(baseline_io.get_latest_sesno().expect("baseline sesno"))
            .expect("baseline sesno fits i32");
        let mut current_io = PdmsIO::new("", current.clone(), true);
        current_io.open().expect("open current file");
        let current_sesno = i32::try_from(current_io.get_latest_sesno().expect("current sesno"))
            .expect("current sesno fits i32");
        let basic_info = current_io.get_page_basic_info().expect("current header");
        assert_eq!(basic_info.pdms_header.db_num, 8000);
        assert!(
            current_sesno > baseline_sesno,
            "fixture must contain a real incremental window"
        );

        let current_meta = std::fs::metadata(&current).expect("current file metadata");
        // 夹具专用的强写：这个 live 用例故意重放更旧的基线，走正门会被判成回退
        // 而只写观察值，前置身份就摆不进去。
        DbnumState::force_scan_identity_for_test(&FileObservation {
            dbnum: 8000,
            project: manager.db_option.project_name.clone(),
            db_type: "DESI".into(),
            file_name: current
                .file_name()
                .expect("current file name")
                .to_string_lossy()
                .into_owned(),
            file_path: current.to_string_lossy().into_owned(),
            file_size: current_meta.len(),
            file_latest_sesno: current_sesno,
            file_modified_at: None,
        })
        .await
        .expect("record current-file identity");
        // This ignored live test deliberately replays an older baseline in the
        // isolated :8009 database. Production watermarks remain monotonic; only
        // the fixture is rewound so a previous interrupted test is rerunnable.
        SUL_DB
            .query(format!(
                "UPDATE dbnum_watermark:8000 SET applied_sesno = {baseline_sesno}, \
                 sesno = {baseline_sesno}; \
                 DELETE increment_update_attempt:8000; \
                 DELETE model_update_pending WHERE dbnum = 8000;"
            ))
            .await
            .expect("rewind isolated fixture watermark")
            .check()
            .expect("valid fixture rewind");
        let recovery_range = (baseline_sesno + 1)..=current_sesno;
        let recovery_changes = IncrementPipeline::collect_changes(&current, recovery_range.clone())
            .expect("collect fixed recovery range");
        let recovery_plan = crate::data_interface::model_update_plan::build_model_update_plan(
            8000,
            current_sesno,
            "DESI",
            &recovery_changes,
        )
        .await
        .expect("build recovery model plan");
        crate::data_interface::model_update_pending::prepare_attempt(
            &crate::data_interface::model_update_pending::IncrementUpdateAttempt {
                dbnum: 8000,
                db_type: "DESI".into(),
                file_path: current.to_string_lossy().into_owned(),
                start_sesno: baseline_sesno + 1,
                end_sesno: current_sesno,
                plan: recovery_plan,
            },
        )
        .await
        .expect("simulate crash after durable prepare and before PE writes");
        let ranges = IndexMap::from([(
            current.clone(),
            (basic_info.clone(), recovery_range.clone(), "DESI".into()),
        )]);
        let result = IncrementPipeline::new().apply(ranges).await;
        assert!(
            result.errors.is_empty(),
            "increment failed: {:?}",
            result.errors
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("replay unfinished range")),
            "apply must recover the durable pre-write attempt: {:?}",
            result.warnings
        );
        assert_eq!(result.successes.len(), 1);
        let mut response = SUL_DB
            .query("RETURN count(SELECT * FROM model_update_pending WHERE dbnum = 8000);")
            .await
            .expect("count pending before replay")
            .check()
            .expect("valid pending-count query");
        let pending_before_replay = response
            .take::<Option<usize>>(0)
            .expect("decode pending count")
            .unwrap_or_default();
        let replay = IncrementPipeline::new()
            .apply(IndexMap::from([(
                current.clone(),
                (basic_info, recovery_range, "DESI".into()),
            )]))
            .await;
        assert!(
            replay.errors.is_empty(),
            "same-range replay failed: {:?}",
            replay.errors
        );
        assert_eq!(replay.successes.len(), 1);
        let mut response = SUL_DB
            .query("RETURN count(SELECT * FROM model_update_pending WHERE dbnum = 8000);")
            .await
            .expect("count pending after replay")
            .check()
            .expect("valid replay pending-count query");
        assert_eq!(
            response
                .take::<Option<usize>>(0)
                .expect("decode replay pending count")
                .unwrap_or_default(),
            pending_before_replay,
            "replaying one fixed session range must not duplicate durable model work"
        );
        let transient = result.successes[0]
            .range_eles
            .values()
            .flatten()
            .filter(|op| op.refno == deleted.refno())
            .collect::<Vec<_>>();
        assert!(
            transient
                .iter()
                .any(|op| matches!(&op.detail, EleOperationDetail::Add(_)))
                && transient
                    .iter()
                    .any(|op| matches!(&op.detail, EleOperationDetail::Deleted)),
            "real fixture must retain the Add→Deleted sequence: {transient:?}"
        );
        assert_eq!(
            DbnumState::applied_sesno(8000)
                .await
                .expect("read final watermark"),
            current_sesno
        );

        let fitting = "24384/22403";
        let pending = load_pending_model_units()
            .await
            .expect("load generation roots");
        assert!(
            pending
                .iter()
                .any(|job| job.dbnum == 8000 && job.root_refno == branch),
            "FTUB/member changes must schedule owning BRAN {branch}: {pending:?}"
        );
        assert!(
            !pending
                .iter()
                .any(|job| job.dbnum == 8000 && job.root_refno == fitting),
            "FTUB is a component, never a delivery-unit generation root"
        );
        let mut response = SUL_DB
            .query(format!(
                "RETURN count(SELECT * FROM model_update_pending \
                 WHERE action = 'delete_cleanup' AND target_refno = '{}');",
                deleted.to_pdms_str()
            ))
            .await
            .expect("query transient-delete work")
            .check()
            .expect("valid transient-delete work query");
        assert_eq!(
            response
                .take::<Option<usize>>(0)
                .expect("decode transient-delete work")
                .unwrap_or_default(),
            1,
            "an element that existed at baseline must retain delete cleanup despite Add→Deleted"
        );
        assert!(
            drain_non_regen(&manager)
                .await
                .expect("execute non-regeneration work")
                >= 1,
            "real window must execute the FTUB transform work"
        );
        let mut response = SUL_DB
            .query(format!(
                "RETURN {}.id != none;",
                deleted.to_inst_relate_key()
            ))
            .await
            .expect("query transient deleted model state")
            .check()
            .expect("valid transient deleted-model query");
        assert!(
            !response
                .take::<Option<bool>>(0)
                .expect("decode transient deleted-model query")
                .unwrap_or(false),
            "same-window Add→Deleted must not materialize a model"
        );

        let mut roots = pending
            .iter()
            .filter(|job| job.dbnum == 8000)
            .map(|job| job.root_refno.clone())
            .collect::<Vec<_>>();
        roots.sort_unstable();
        roots.dedup();
        ModelRefreshPolicy::generate_roots(&manager, &roots)
            .await
            .expect("generate every affected root");
        // BRAN is the delivery/generation root; its FTUB remains the concrete
        // component that owns the generated instance relation.
        let key = RefnoEnum::from(fitting).to_inst_relate_key();
        let mut response = SUL_DB
            .query(format!("RETURN {key}.id != none;"))
            .await
            .expect("query generated fitting")
            .check()
            .expect("valid generated-fitting query");
        assert!(
            response
                .take::<Option<bool>>(0)
                .expect("decode generated-fitting query")
                .unwrap_or(false),
            "owning BRAN generation must materialize its FTUB component geometry"
        );
        let mut response = SUL_DB
            .query(format!(
                "RETURN {}.id != none;",
                deleted.to_inst_relate_key()
            ))
            .await
            .expect("query deleted model after root regeneration")
            .check()
            .expect("valid post-regeneration deleted-model query");
        assert!(
            !response
                .take::<Option<bool>>(0)
                .expect("decode post-regeneration deleted-model query")
                .unwrap_or(false),
            "regenerating the surviving BRAN must not create the transient deleted element"
        );

        // Synthetic OWNER edit over real PE/CATA data: plan before persist, then
        // move the real FTUB from BRAN A to BRAN B and regenerate both sides.
        let old_branch = RefnoEnum::from(branch);
        let new_branch = RefnoEnum::from("24384/22404");
        let fitting_refno = RefnoEnum::from(fitting);
        SUL_DB
            .query(format!(
                "DELETE pe_owner WHERE in = {} AND out != {};",
                fitting_refno.to_pe_key(),
                old_branch.to_pe_key()
            ))
            .await
            .expect("remove stale owner edge from an interrupted prior run")
            .check()
            .expect("valid stale-owner cleanup");
        IncrementPipeline::invalidate_caches(HashSet::from([
            fitting_refno,
            old_branch,
            new_branch,
        ]))
        .await;
        let mut move_op = result.successes[0]
            .range_eles
            .values()
            .flatten()
            .find(|op| {
                op.refno == fitting_refno.refno()
                    && matches!(&op.detail, EleOperationDetail::Modified(_))
            })
            .expect("real FTUB modification")
            .clone();
        let mut source_fitting_op = move_op.clone();
        let EleOperationDetail::Modified(source_modified) = &mut source_fitting_op.detail else {
            unreachable!("filtered above")
        };
        source_modified.modified_explicit_attrs.insert(
            "OWNER".into(),
            (
                NamedAttrValue::RefU64Type(new_branch.refno()),
                NamedAttrValue::RefU64Type(old_branch.refno()),
            ),
        );
        let EleOperationDetail::Modified(modified) = &mut move_op.detail else {
            unreachable!("filtered above")
        };
        modified.current_data.owner = new_branch.refno();
        modified.modified_explicit_attrs.insert(
            "OWNER".into(),
            (
                NamedAttrValue::RefU64Type(old_branch.refno()),
                NamedAttrValue::RefU64Type(new_branch.refno()),
            ),
        );
        let old_children = RefU64Vec(
            aios_core::get_children_pes(old_branch)
                .await
                .expect("load old BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect(),
        );
        let new_children = RefU64Vec(
            aios_core::get_children_pes(new_branch)
                .await
                .expect("load new BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect(),
        );
        let original_old_children = old_children.clone();
        let original_new_children = new_children.clone();
        let mut old_children_after = old_children.clone();
        old_children_after
            .0
            .retain(|child| *child != fitting_refno.refno());
        let mut new_children_after = new_children.clone();
        if !new_children_after.0.contains(&fitting_refno.refno()) {
            new_children_after.0.push(fitting_refno.refno());
        }
        let parent_op = |root: RefnoEnum, sesno: u32, before: RefU64Vec, after: RefU64Vec| {
            EleOperationData::new(
                root.refno(),
                sesno,
                EleOperationDetail::Modified(ModifiedElement {
                    current_data: Default::default(),
                    added_attrs: Default::default(),
                    deleted_attrs: Default::default(),
                    modified_attrs: Default::default(),
                    added_explicit_attrs: Default::default(),
                    deleted_explicit_attrs: Default::default(),
                    modified_explicit_attrs: Default::default(),
                    added_uda_attrs: Default::default(),
                    deleted_uda_attrs: Default::default(),
                    modified_uda_attrs: Default::default(),
                    noun: "BRAN".into(),
                    children_changed: Some((before, after)),
                }),
            )
        };
        let move_sesno = u32::try_from(current_sesno + 1).expect("synthetic sesno fits u32");
        let move_range = BTreeMap::from([(
            move_sesno,
            vec![
                move_op,
                parent_op(old_branch, move_sesno, old_children, old_children_after),
                parent_op(new_branch, move_sesno, new_children, new_children_after),
            ],
        )]);
        let move_plan = crate::data_interface::model_update_plan::build_model_update_plan(
            8000,
            current_sesno + 1,
            "DESI",
            &move_range,
        )
        .await
        .expect("build move model plan");
        let mut move_roots = move_plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RegenRoot)
            .map(|item| item.target_refno.clone())
            .collect::<Vec<_>>();
        move_roots.sort_unstable();
        move_roots.dedup();
        let mut expected_move_roots = vec![old_branch.to_pdms_str(), new_branch.to_pdms_str()];
        expected_move_roots.sort_unstable();
        assert_eq!(
            move_roots, expected_move_roots,
            "cross-BRAN OWNER move must regenerate both membership sides"
        );

        IncrementPipeline::persist_latest_main_data(&move_range, 8000)
            .await
            .expect("persist synthetic OWNER move");
        IncrementPipeline::invalidate_caches(HashSet::from([
            fitting_refno,
            old_branch,
            new_branch,
        ]))
        .await;
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE out FROM pe_owner WHERE in = {};",
                fitting_refno.to_pe_key()
            ))
            .await
            .expect("query moved FTUB owner")
            .check()
            .expect("valid moved-owner query");
        assert_eq!(
            response
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("decode moved owner"),
            vec![new_branch.to_pe_key().parse().expect("new owner thing")]
        );

        ModelRefreshPolicy::generate_roots(&manager, &move_roots)
            .await
            .expect("regenerate both sides of OWNER move");
        for root in [old_branch, new_branch] {
            let subtree = crate::data_interface::helper::collect_pe_subtree_refnos(&[root])
                .await
                .expect("collect regenerated branch subtree");
            let pe_keys = subtree
                .iter()
                .map(RefnoEnum::to_pe_key)
                .collect::<Vec<_>>()
                .join(",");
            let mut response = SUL_DB
                .query(format!(
                    "SELECT VALUE id FROM inst_relate WHERE in IN [{pe_keys}];"
                ))
                .await
                .expect("query regenerated branch models")
                .check()
                .expect("valid regenerated-branch query");
            assert!(
                !response
                    .take::<Vec<surrealdb::sql::Thing>>(0)
                    .expect("decode regenerated branch models")
                    .is_empty(),
                "both old and new BRAN must contain generated model instances after move"
            );
        }
        let mut response = SUL_DB
            .query(format!(
                "RETURN {}.id != none;",
                fitting_refno.to_inst_relate_key()
            ))
            .await
            .expect("query moved FTUB model")
            .check()
            .expect("valid moved-FTUB query");
        assert!(
            response
                .take::<Option<bool>>(0)
                .expect("decode moved-FTUB model")
                .unwrap_or(false),
            "receiving BRAN generation must materialize the moved FTUB"
        );

        // Same members, different order: persist indexed pe_owner edges and
        // regenerate only the affected receiving BRAN.
        let reorder_before = RefU64Vec(
            aios_core::get_children_pes(new_branch)
                .await
                .expect("load moved BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect(),
        );
        assert!(
            reorder_before.0.len() >= 2,
            "real receiving BRAN needs at least two children for reorder coverage"
        );
        let mut reorder_after = reorder_before.clone();
        reorder_after.0.swap(0, 1);
        let reorder_sesno = u32::try_from(current_sesno + 2).expect("reorder sesno fits u32");
        let reorder_range = BTreeMap::from([(
            reorder_sesno,
            vec![parent_op(
                new_branch,
                reorder_sesno,
                reorder_before.clone(),
                reorder_after.clone(),
            )],
        )]);
        // A malformed ref_rev endpoint exercises the production read/decode
        // failure path without a test-only injection switch.
        const BAD_REFERRER: &str = "pe:codex_bad_ref_rev";
        SUL_DB
            .query(format!(
                "DELETE ref_rev WHERE in = {BAD_REFERRER}; \
                 RELATE {BAD_REFERRER}->ref_rev->{};",
                new_branch.to_pe_key()
            ))
            .await
            .expect("inject malformed reverse edge")
            .check()
            .expect("valid malformed-edge fixture");
        let reorder_plan = crate::data_interface::model_update_plan::build_model_update_plan(
            8000,
            current_sesno + 2,
            "DESI",
            &reorder_range,
        )
        .await
        .expect("build reorder model plan");
        let reorder_roots = reorder_plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RegenRoot)
            .map(|item| item.target_refno.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            reorder_roots,
            vec![new_branch.to_pdms_str()],
            "same-set reorder must regenerate only its owning BRAN"
        );
        assert!(
            reorder_plan
                .work_items
                .iter()
                .any(|item| item.action == ModelWorkAction::CascadeExpand
                    && item.target_refno == new_branch.to_pdms_str()),
            "reverse-index failure must persist a deferred cascade seed: {reorder_plan:?}"
        );
        assert!(
            reorder_plan
                .warnings
                .iter()
                .any(|warning| warning.contains("reverse-reference lookup failed")),
            "reverse-index failure must remain visible: {:?}",
            reorder_plan.warnings
        );

        IncrementPipeline::persist_latest_main_data(&reorder_range, 8000)
            .await
            .expect("persist BRAN child reorder");
        crate::data_interface::model_update_pending::finalize_attempt(
            8000,
            current_sesno + 2,
            None,
            &reorder_plan,
            &[],
        )
        .await
        .expect("persist reorder work and advance watermark");
        SUL_DB
            .query(format!("DELETE ref_rev WHERE in = {BAD_REFERRER};"))
            .await
            .expect("repair reverse index")
            .check()
            .expect("valid reverse-index repair");
        IncrementPipeline::invalidate_caches(HashSet::from([new_branch])).await;
        #[derive(serde::Deserialize)]
        struct OrderedChild {
            child: surrealdb::sql::Thing,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT id, in AS child FROM pe_owner WHERE out = {} ORDER BY id;",
                new_branch.to_pe_key()
            ))
            .await
            .expect("query persisted child order")
            .check()
            .expect("valid child-order query");
        let persisted_order = response
            .take::<Vec<OrderedChild>>(0)
            .expect("decode persisted child order");
        let persisted_order = persisted_order
            .into_iter()
            .map(|row| row.child)
            .collect::<Vec<_>>();
        let expected_order = reorder_after
            .0
            .iter()
            .map(|child| {
                RefnoEnum::from(*child)
                    .to_pe_key()
                    .parse()
                    .expect("child thing")
            })
            .collect::<Vec<surrealdb::sql::Thing>>();
        assert_eq!(
            persisted_order, expected_order,
            "pe_owner compound ids must retain the new child order"
        );
        drain(&manager)
            .await
            .expect("recover deferred cascade and regenerate reordered BRAN");
        assert!(
            load_pending_model_units()
                .await
                .expect("load recovered pending work")
                .is_empty(),
            "repaired reverse index must let deferred cascade and root work converge"
        );
        let reordered_subtree =
            crate::data_interface::helper::collect_pe_subtree_refnos(&[new_branch])
                .await
                .expect("collect reordered branch subtree");
        let reordered_pe_keys = reordered_subtree
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM inst_relate WHERE in IN [{reordered_pe_keys}];"
            ))
            .await
            .expect("query reordered branch models")
            .check()
            .expect("valid reordered-model query");
        assert!(
            !response
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("decode reordered branch models")
                .is_empty(),
            "reordered BRAN must still have generated model instances"
        );

        // Restore the shared :8009 ProjAMS state to the actual file. The MOVE
        // and ORDER cases above are synthetic and must not leak sesno+1/+2 or
        // altered ownership into later manual-preview tests.
        std::fs::copy(&current, fixture_db_dir.join("ams8000_0001"))
            .expect("replace fixture with current source file");
        let restored = sync_total_async_threaded(
            &baseline_options,
            fixture_project,
            Arc::new(DashSet::new()),
            &["DESI"],
            100,
        )
        .await
        .expect("restore current ProjAMS dbnum=8000");
        assert!(
            restored.get(&8000).copied().unwrap_or_default() > 0,
            "restored dbnum=8000 must contain elements"
        );
        SUL_DB
            .query(format!(
                "UPDATE dbnum_watermark:8000 SET applied_sesno = {current_sesno}, \
                 sesno = {current_sesno}, file_latest_sesno = {current_sesno}; \
                 DELETE increment_update_attempt:8000; \
                 DELETE model_update_pending WHERE dbnum = 8000;"
            ))
            .await
            .expect("restore isolated fixture watermark")
            .check()
            .expect("valid fixture restore");
        let restored_old_children = RefU64Vec(
            aios_core::get_children_pes(old_branch)
                .await
                .expect("read synthetic old BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect(),
        );
        let restored_new_children = RefU64Vec(
            aios_core::get_children_pes(new_branch)
                .await
                .expect("read synthetic new BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect(),
        );
        let restore_sesno = u32::try_from(current_sesno).expect("current sesno fits u32");
        IncrementPipeline::persist_latest_main_data(
            &BTreeMap::from([(
                restore_sesno,
                vec![
                    source_fitting_op,
                    parent_op(
                        old_branch,
                        restore_sesno,
                        restored_old_children,
                        original_old_children.clone(),
                    ),
                    parent_op(
                        new_branch,
                        restore_sesno,
                        restored_new_children,
                        original_new_children.clone(),
                    ),
                ],
            )]),
            8000,
        )
        .await
        .expect("restore synthetic OWNER and child-order edits");
        SUL_DB
            .query(format!(
                "DELETE pe_owner WHERE in = {} AND out != {};",
                fitting_refno.to_pe_key(),
                old_branch.to_pe_key()
            ))
            .await
            .expect("remove synthetic owner edge")
            .check()
            .expect("valid synthetic-owner cleanup");
        IncrementPipeline::invalidate_caches(HashSet::from([
            fitting_refno,
            old_branch,
            new_branch,
        ]))
        .await;
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE out FROM pe_owner WHERE in = {};",
                fitting_refno.to_pe_key()
            ))
            .await
            .expect("query restored fitting owner")
            .check()
            .expect("valid restored-owner query");
        assert_eq!(
            response
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("decode restored fitting owner"),
            vec![old_branch.to_pe_key().parse().expect("old owner thing")],
            "restore exactly one FTUB owner from source"
        );
        assert_eq!(
            aios_core::get_children_pes(old_branch)
                .await
                .expect("read restored old BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect::<Vec<_>>(),
            original_old_children.0,
            "restore old BRAN membership and order"
        );
        assert_eq!(
            aios_core::get_children_pes(new_branch)
                .await
                .expect("read restored new BRAN children")
                .into_iter()
                .map(|child| child.refno.refno())
                .collect::<Vec<_>>(),
            original_new_children.0,
            "restore new BRAN membership and order"
        );
        ModelRefreshPolicy::generate_roots(
            &manager,
            &[old_branch.to_pdms_str(), new_branch.to_pdms_str()],
        )
        .await
        .expect("restore affected BRAN models");

        std::fs::remove_dir_all(&fixture_root).expect("remove temporary baseline fixture");
    }

    /// D-03（live）：ProjAMS 上一次**真实** E3D 删除会话。
    ///
    /// 六个变化桶里 Deleted 是唯一还没有真实文件会话的一个。上面那个 FTUB 用例走的是
    /// 备份基线对当前文件的瞬态 Add→Deleted，而 FTUB 是伪类型、BRAN 内的组件，覆盖不到
    /// 「元素自身带模型被删」的清理路径。这里的 VTWA 24381/107146 自带 `inst_relate`
    /// 与 `SPRE`，且属于 `INLINE` 变化等价类——此前没有任何用例覆盖过该类。
    ///
    /// 会话由 `scripts/e3d/projams_incr_delete_apply.mac` 产生。删除不可逆、refno 不会
    /// 被重新发放，回滚只能靠删除前的文件备份
    /// `ams7997_0001.codex-before-d03-delete-20260727`。
    /// 执行前基线见 `docs/evidence/2026-07-27-d03-delete-session-baseline.md`。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: needs the real delete session from scripts/e3d/projams_incr_delete_apply.mac"]
    async fn live_real_delete_session_cleans_up_model_and_regenerates_branch() {
        use crate::data_interface::generation_root::{
            configured_delivery_unit_types, resolve_live_element_generation_root,
        };
        use crate::data_interface::manual_update::load_pending_model_units;
        use crate::data_interface::model_update_pending::drain;
        use crate::data_interface::model_update_plan::{ModelWorkAction, build_model_update_plan};

        let design_file = PathBuf::from(std::env::var("AIOS_PROJAMS_DELETE_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001".into(),
        ));
        assert!(
            design_file.is_file(),
            "missing design file: {}",
            design_file.display()
        );
        let sesno: i32 = std::env::var("AIOS_PROJAMS_DELETE_SESNO")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(84);

        aios_core::init_test_surreal()
            .await
            .expect("connect isolated surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");

        let deleted = RefnoEnum::from("24381/107146");
        let branch = RefnoEnum::from("24381/107104");

        let mut response = SUL_DB
            .query("SELECT VALUE applied_sesno FROM dbnum_watermark:7997;")
            .await
            .expect("read watermark before the delete window")
            .check()
            .expect("valid watermark query");
        let applied_before = response
            .take::<Vec<i32>>(0)
            .expect("decode watermark")
            .first()
            .copied()
            .unwrap_or_default();
        assert!(
            applied_before < sesno,
            "window {sesno} is already applied (applied_sesno={applied_before}); \
             restore the pre-delete file backup and reload before rerunning"
        );

        // 墓碑落库后元素链路就查不到了，生成根必须先解析。
        let root = resolve_live_element_generation_root(deleted, &configured_delivery_unit_types())
            .await
            .expect("inspect VTWA before applying its tombstone")
            .expect("VTWA must resolve to its owning BRAN");
        assert_eq!(
            (root.root.to_pdms_str(), root.noun.as_str()),
            (branch.to_pdms_str(), "BRAN")
        );
        let children_before = aios_core::get_children_pes(branch)
            .await
            .expect("read BRAN children before the delete")
            .len();

        let changes = IncrementPipeline::collect_changes(&design_file, sesno..=sesno)
            .expect("collect the real delete session");
        let operations = changes
            .get(&u32::try_from(sesno).expect("sesno fits u32"))
            .expect("the delete session must exist in the file");
        assert!(
            operations.iter().any(|operation| {
                RefnoEnum::from(operation.refno) == deleted
                    && matches!(operation.detail, EleOperationDetail::Deleted)
            }),
            "sesno {sesno} must carry the VTWA tombstone: {operations:?}"
        );

        let plan = build_model_update_plan(7997, sesno, "DESI", &changes)
            .await
            .expect("build delete model plan");
        let actions = plan
            .work_items
            .iter()
            .map(|item| (item.action, item.target_refno.as_str()))
            .collect::<Vec<_>>();
        assert!(
            actions
                .iter()
                .any(|(action, _)| *action == ModelWorkAction::DeleteCleanup),
            "a real tombstone must plan model cleanup: {actions:?}"
        );
        assert!(
            actions.contains(&(ModelWorkAction::RegenRoot, branch.to_pdms_str().as_str())),
            "the owning BRAN must be replanned after losing a component: {actions:?}"
        );

        let mut io = PdmsIO::new("", design_file.clone(), true);
        io.open().expect("open design file");
        let basic_info = io.get_page_basic_info().expect("design file header");
        assert_eq!(basic_info.pdms_header.db_num, 7997);

        let result = IncrementPipeline::new()
            .apply(IndexMap::from([(
                design_file.clone(),
                (basic_info.clone(), sesno..=sesno, "DESI".into()),
            )]))
            .await;
        assert!(
            result.errors.is_empty(),
            "delete window failed: {:?}",
            result.errors
        );
        drain(&manager)
            .await
            .expect("consume the model work of the delete window");

        let mut response = SUL_DB
            .query(
                "RETURN [
                    (SELECT VALUE deleted FROM pe:24381_107146)[0],
                    inst_relate:24381_107146.id != none,
                    count(SELECT * FROM ref_rev
                          WHERE in = pe:24381_107146 OR out = pe:24381_107146),
                    count(SELECT * FROM pe_owner WHERE in = pe:24381_107146),
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:7997)[0]
                ];",
            )
            .await
            .expect("query the state left by the delete window")
            .check()
            .expect("valid cleanup query");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode cleanup state");
        assert!(
            state[0].is_null() || state[0] == serde_json::json!(true),
            "the tombstoned VTWA must be gone or soft-deleted: {state:?}"
        );
        assert_eq!(
            state[1],
            serde_json::json!(false),
            "inst_relate must not outlive its element: {state:?}"
        );
        assert_eq!(
            state[2],
            serde_json::json!(0),
            "no ref_rev edge may keep the deleted refno as an endpoint: {state:?}"
        );
        assert_eq!(
            state[3],
            serde_json::json!(0),
            "the pe_owner edge to the BRAN must be removed: {state:?}"
        );
        assert!(
            state[4]
                .as_i64()
                .is_some_and(|applied| applied >= sesno as i64),
            "the watermark must advance past the delete window: {state:?}"
        );
        assert_eq!(
            aios_core::get_children_pes(branch)
                .await
                .expect("read BRAN children after the delete")
                .len(),
            children_before - 1,
            "the BRAN must lose exactly the deleted component"
        );

        // 同区间重放。`apply` 是不带水位闸门的底层原语：崩溃恢复本就要求它能把一个
        // 已部分落库的固定区间原样重跑，而两个生产调用方的区间都是从水位算出来的，
        // 谁都不会拿已应用的窗口来调它。所以可断言的是收敛，不是「重放不排活」——
        // 重放会按同样的键重新建立同一批工作，这与 `live_real_ftub_delete_move_and_
        // reorder` 里「重放固定区间不得重复建立模型工作」是同一条约定。
        let replay = IncrementPipeline::new()
            .apply(IndexMap::from([(
                design_file,
                (basic_info, sesno..=sesno, "DESI".into()),
            )]))
            .await;
        assert!(
            replay.errors.is_empty(),
            "same-range replay failed: {:?}",
            replay.errors
        );
        let mut response = SUL_DB
            .query(
                "SELECT action, target_refno FROM model_update_pending
                 WHERE dbnum = 7997 ORDER BY action;",
            )
            .await
            .expect("read the work replanned by the replay")
            .check()
            .expect("valid replanned-work query");
        let replanned: Vec<serde_json::Value> = response.take(0).expect("decode replanned work");
        assert_eq!(
            replanned,
            vec![
                serde_json::json!({
                    "action": "delete_cleanup",
                    "target_refno": "24381/107146",
                }),
                serde_json::json!({
                    "action": "regen_root",
                    "target_refno": "24381/107104",
                }),
            ],
            "the replay must re-establish exactly the same keyed work, never duplicates: \
             {replanned:?}"
        );

        drain(&manager)
            .await
            .expect("consume the work replanned by the replay");
        assert!(
            load_pending_model_units()
                .await
                .expect("read pending model units after the replay drain")
                .is_empty(),
            "the replayed window must drain back to an empty queue"
        );

        // 收敛：墓碑没被复活，水位没再动，BRAN 也没多出或少掉子件。
        let mut response = SUL_DB
            .query(
                "RETURN [
                    (SELECT VALUE deleted FROM pe:24381_107146)[0],
                    inst_relate:24381_107146.id != none,
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:7997)[0]
                ];",
            )
            .await
            .expect("query the state left by the replay")
            .check()
            .expect("valid convergence query");
        let converged: Vec<serde_json::Value> = response.take(0).expect("decode converged state");
        assert!(
            converged[0].is_null() || converged[0] == serde_json::json!(true),
            "the replay must not resurrect the tombstoned VTWA: {converged:?}"
        );
        assert_eq!(
            converged[1],
            serde_json::json!(false),
            "the replay must not recreate inst_relate for a deleted element: {converged:?}"
        );
        assert_eq!(
            converged[2],
            serde_json::json!(sesno),
            "the replay must leave the watermark where the first pass put it: {converged:?}"
        );
        assert_eq!(
            aios_core::get_children_pes(branch)
                .await
                .expect("read BRAN children after the replay")
                .len(),
            children_before - 1,
            "the BRAN child count must not drift across a replay"
        );
    }
}
