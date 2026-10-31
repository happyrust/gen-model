//! IncrementPipeline — deep module for narrow incremental persist + watermark.
//!
//! Interface: `apply(ranges_map) -> IncrResult`
//! Does NOT own model refresh or MQTT sync (callers consume `IncrResult`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aios_core::data_center::DataCenterRecordOperate;
use aios_core::pdms_types::*;
use aios_core::{RefnoEnum, SUL_DB, clear_all_caches_batch};
use anyhow::Context as _;
use indexmap::IndexMap;

use crate::data_interface::db_file_header::DbFileHeader;
use crate::data_interface::e3d_collector::RetiredTypeRow;
use crate::data_interface::ele_ops::{
    EleOperationData, EleOperationDetail, ModifiedElement, PersistElementChange, UdaKeyMap,
    terminal_persist_changes,
};
use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::sesno_range::COLD_START_DB_TYPES;

const DATACENTER_VERSION: &str = "datacenter_version";
/// 交付删除记录表（plan `docs/plans/2026-09-22-datacenter-delete-records-dev-plan.md` D1）：
/// 交付单元被删 / 改位号时，记「这个 refno 曾发生过删除或改名事件」。
/// 主键只取 refno 值：`datacenter_delete:⟨ref0_ref1⟩`；`INSERT IGNORE` 让首次事件赢，
/// 写者只有增量随窗语句，消费与后续处理交给第三方。
pub(crate) const DATACENTER_DELETE: &str = "datacenter_delete";

/// datacenter 上溯用的 `pe` 行视图：noun / owner / name。overlay（本窗净态）与持久层
/// 窗口前态都渲成这一形状，`effective_view` 逐字段 overlay 优先合成。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PeChainView {
    pub noun: Option<String>,
    pub owner: Option<RefU64>,
    /// `NAME`（含首位 `/`）；没名字的元素为 `None`。
    pub name: Option<String>,
}

impl PeChainView {
    #[cfg(test)]
    pub(crate) fn new(noun: Option<&str>, owner: Option<RefU64>) -> Self {
        Self {
            noun: noun.map(str::to_owned),
            owner,
            name: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn named(noun: Option<&str>, owner: Option<RefU64>, name: &str) -> Self {
        Self {
            noun: noun.map(str::to_owned),
            owner,
            name: Some(name.to_owned()),
        }
    }
}

/// Meta / config DB types: persist + watermark only; no geometry model refresh.
/// Same set as cold-start eligibility ([`COLD_START_DB_TYPES`]).
pub const SYS_META_DB_TYPES: &[&str] = COLD_START_DB_TYPES;

/// 一个冻结窗口的完整收集结果。
///
/// `range_eles` 是净操作，不足以表达空保存或窗口内自抵消的会话；
/// `session_sesnos` 因此直接来自文件会话页映射，是批次保存清单的唯一事实源。
///
/// 没有「口径」字段：ADR-031 之后收集只有净窗口一种，恒为同一取值的字段是假选择。
/// 口径与全部容忍计数由 `warnings` 首条（[`net_caliber_warning`]）自报。
#[derive(Debug, Clone)]
pub struct CollectedWindow {
    pub range_eles: BTreeMap<u32, Vec<EleOperationData>>,
    pub persist_changes: Vec<PersistElementChange>,
    pub session_sesnos: Vec<u32>,
    pub warnings: Vec<String>,
    /// ADR-036 成员关系仲裁补出的删除数。e3d-io 收集器不做成员仲裁（索引差分本身即终态），
    /// 恒为 0；字段留着是回执契约的一部分。
    pub membership_deleted: usize,
    /// 收集时冻住的文件世代与目标会话；执行前必须用它复核目标 root。
    pub frozen: Option<FrozenSource>,
    /// refno 复用（窗口内换了 noun）的条目：终稿已按 `Add` 进 `range_eles`，旧 noun 行
    /// 随元素块的删除块一起清（`element_batch`，plan 2026-09-06 §3 D2）。
    pub retired_type_rows: Vec<RetiredTypeRow>,
}

/// 逐会话实体回放入口（`collect_changes`，old-pdms-io 底座）2026-09-04 已随 old-pdms-io 一起删除，
/// 不再有 feature 门——任何构建里都不存在。
///
/// ```compile_fail
/// use aios_database::data_interface::increment_pipeline::IncrementPipeline;
/// let _ = IncrementPipeline::collect_changes;
/// ```
pub struct LegacySessionReplayUnavailableInProduction;

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
    /// 冻结窗口内实际存在的完整会话清单；包含空保存与净结果自抵消的会话。
    pub session_sesnos: Vec<u32>,
    /// Full delta payload (MySQL / classified refresh). Kept for callers that need detail.
    pub range_eles: BTreeMap<u32, Vec<EleOperationData>>,
    /// The model work this window established, with the delivery-unit rollup
    /// behind it. Callers that need to act on the affected units (the manual
    /// run's per-unit reporting) read it here instead of resolving the rollup a
    /// second time — it can only be resolved before the window persists anyway.
    pub model_plan: crate::data_interface::model_update_plan::ModelUpdatePlan,
    /// 尾事务之后那次凭证前移的账（ADR-056 P2-2；plant-ui 计划 G3）。只有窗口规划器
    /// （direct DESI）给了名单或退化原因时才有；老规划器与 CATA / SYST 窗口是 `None`，
    /// 回执上整格不出现。
    pub credential_advance:
        Option<crate::data_interface::window_root_plan::CredentialAdvanceReceipt>,
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

/// 直写窗口每块事务的语句数。
///
/// 原实现把整窗口拼成「单个事务」，大型系统库（如 amssys 冷启动 168 会话 ~4000+ 元素）
/// 会撑爆 SurrealDB ws 通道上限，报「receiving from an empty and closed channel」而整体
/// 失败。改为按本常量分块、每块自身原子提交：配合幂等 UPSERT 与「失败不推进水位、按
/// 同一窗口重试」，重试仍从可收敛状态开始，不会半写卡死。
pub(crate) const PERSIST_TX_CHUNK: usize = 500;

/// 把窗口语句按 `chunk` 条一块包成独立事务，语句顺序不变——跨块引用与单事务同样是
/// 前向依赖。生产直写（[`IncrementPipeline::persist_latest_main_data`]）与崩溃重放对拍
/// （`direct_window_replay_parity`）共用这一份分块，对拍里的「中途 kill」就是在这些块的
/// 边界上停：块内原子，所以块边界是崩溃能落在的全部位置。
pub(crate) fn persist_transaction_batches(statements: &[String], chunk: usize) -> Vec<String> {
    statements
        .chunks(chunk.max(1))
        .filter_map(wrap_in_transaction)
        .collect()
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
    cata_dependency: Duration,
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
             collect={}ms plan={}ms cata_dependency={}ms persist={}ms cache={}ms rev_index={}ms \
             datacenter={}ms finalize={}ms",
            self.collect.as_millis(),
            self.plan.as_millis(),
            self.cata_dependency.as_millis(),
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

/// 交入窗口的采信判定（纯函数）：调用方交出的预收集结果只有与本次要应用的区间
/// **完全一致**才复用；错位区间（重叠、错一格、方向反了）与没交都回退自行收集。
/// 部分重叠也拒绝——增量窗口是按「水位+1..=文件最新」整体应用的，掐头去尾的
/// 子集会让折叠与影响判定漏看会话。崩溃后的重跑同样走这条路：区间由回退后的水位
/// 重新算出来，没有「持久化的旧计划」这一档了（ADR-072 D1）。
fn accept_handed_in_window(
    precollected: Option<(RangeInclusive<i32>, CollectedWindow)>,
    requested_range: &RangeInclusive<i32>,
) -> Option<CollectedWindow> {
    match precollected {
        Some((range, collected)) if &range == requested_range => Some(collected),
        _ => None,
    }
}

/// 从文件权威会话页映射中截取冻结窗口，天然升序、去重。
///
/// 不能看 `range_eles.keys()`：一次 Save Work 即便没有元素变化、或多次操作在净窗口
/// 内互相抵消，它的会话页仍然存在，也必须进入批次回执。
fn session_sesnos_in_range(
    sesno_pgno_map: &BTreeMap<i32, u32>,
    sesno_range: &RangeInclusive<i32>,
) -> Vec<u32> {
    sesno_pgno_map
        .range(sesno_range.clone())
        .filter_map(|(&sesno, _)| u32::try_from(sesno).ok())
        .collect()
}

/// 净口径标注——第一条 warning 的固定形状（预览断言与证据文档都认这个前缀）。
///
/// 全部容忍形状的计数**必须在这里露面**：`unchanged_rewrites` / `unparseable_finals` /
/// 类型改变（refno 复用）只活在 `NetWindowOutcome` 里，而它在 `collect_window` 拼完这条
/// warning 之后即被拆散——不并进回执，批次路径上「跳过」就是静默的
/// （spec-003 FR-8：任何一种容忍都不许静默；2026-08-18 审核 P2）。
///
/// 底座换成 e3d-io 之后（2026-09-03）成员补删 / 幽灵新增两项恒为 0——两根索引差分本身即
/// 终态，没有仲裁这一步；字段保留，让证据文档里新旧两代 warning 能逐字段对读。
fn net_caliber_warning(
    added: usize,
    deleted: usize,
    modified: usize,
    outcome: &crate::data_interface::e3d_collector::NetWindowOutcome,
) -> String {
    format!(
        "收集口径：净窗口（会话索引差分，ADR-031/036 单一口径；e3d-io 底座）——added={added} deleted={deleted} \
         modified={modified} 成员补删=0 幽灵新增丢弃=0 原样重写跳过={} 终稿不可解析={} 类型改变={} \
         索引候选={}（ins/mod/del={}/{}/{}）基会话={} 读页={} 差分 {}ms",
        outcome.unchanged_rewrites,
        outcome.unparseable_finals,
        outcome.retired_type_rows.len(),
        outcome.stats.candidates,
        outcome.stats.inserted,
        outcome.stats.modified,
        outcome.stats.deleted,
        outcome
            .stats
            .base_sesno
            .map(|sesno| sesno.to_string())
            .unwrap_or_else(|| "无（全量）".into()),
        outcome.stats.pages_read,
        outcome.stats.elapsed_ms
    )
}

/// 净窗口是终态而不是事件流：同一 refno 只能有一种终态操作。
///
/// 成员补删会覆盖索引差分的旧操作；这道出口断言防止以后的合并改动
/// 把同一元素同时交给落库和模型影响分类。
fn ensure_unique_terminal_operations(
    window: &BTreeMap<u32, Vec<EleOperationData>>,
) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    for operation in window.values().flatten() {
        anyhow::ensure!(
            seen.insert(operation.refno),
            "净窗口终态重复: refno={}",
            operation.refno
        );
    }
    Ok(())
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

/// 一次 `Modified` 里交付单元自身 `NAME` 的 (旧值, 新值)。只看 `modified_attrs ∪
/// modified_explicit_attrs`（`normalize_attribute_name == "NAME"`）：`NAME` 从 `added_*`
/// 出现说明以前没名字，没有「改前的位号」可记，回 `None`。值不是文本型也回 `None`。
fn delivery_unit_name_change(modified: &ModifiedElement) -> Option<(String, String)> {
    use crate::data_interface::model_impact::normalize_attribute_name;
    fn text(value: &aios_core::NamedAttrValue) -> Option<&str> {
        use aios_core::NamedAttrValue;
        match value {
            NamedAttrValue::StringType(s)
            | NamedAttrValue::ElementType(s)
            | NamedAttrValue::WordType(s) => Some(s.as_str()),
            _ => None,
        }
    }
    modified
        .modified_attrs
        .iter()
        .chain(&modified.modified_explicit_attrs)
        .find(|(attr, _)| normalize_attribute_name(attr) == "NAME")
        .and_then(|(_, (old, new))| Some((text(old)?.to_owned(), text(new)?.to_owned())))
}

/// 一条交付删除记录的渲染输入（plan 2026-09-22 D3）。
pub(crate) struct DatacenterDeleteRecord<'a> {
    pub refno: RefU64,
    /// 要在中台删掉的那个位号（`NAME` 原文，含首位 `/`）。
    pub name: &'a str,
    pub noun: &'a str,
    pub zone: Option<RefU64>,
    pub dbnum: u32,
    pub sesno: u32,
    pub sesno_time: Option<&'a str>,
    /// `deleted` | `renamed`
    pub reason: &'static str,
}

/// `INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:⟨ref0_ref1⟩, … };`——ID
/// 只取 refno 的值，不把 `pe` 记录链接或 `name` 放进 ID。同一 refno 后续再删除、再改名或整窗
/// 重放都忽略，事件表只保留“曾经发生过”的首次事实。字段全是字面量且不带 `time::now()`；位号里的
/// `\` 与 `'` 仍按 SurrealQL 字串规则转义。
pub(crate) fn render_datacenter_delete_insert_ignore(
    record: &DatacenterDeleteRecord<'_>,
) -> String {
    use crate::data_interface::dbnum_state::escape_surql_str;
    let pe = record.refno.to_pe_key();
    let name = escape_surql_str(record.name);
    let zone = record
        .zone
        .map(|zone| zone.to_pe_key())
        .unwrap_or_else(|| "NONE".into());
    let sesno_time = record
        .sesno_time
        .map(|time| format!("'{}'", escape_surql_str(time)))
        .unwrap_or_else(|| "NONE".into());
    format!(
        "INSERT IGNORE INTO {DATACENTER_DELETE} {{ id: {id}, refno: {pe}, name: '{name}', \
         type: '{noun}', zone_refno: {zone}, dbnum: {dbnum}, sesno: {sesno}, \
         sesno_time: {sesno_time}, reason: '{reason}' }};",
        id = record.refno.to_table_key(DATACENTER_DELETE),
        noun = escape_surql_str(record.noun),
        dbnum = record.dbnum,
        sesno = record.sesno,
        reason = record.reason,
    )
}

/// 预览回执用（plan 2026-09-22 T4）：一批随窗语句里有几条**交付删除记录**。只认
/// [`render_datacenter_delete_insert_ignore`] 出的形状。这里只报告本窗口尝试写入的事件数；同一
/// refno 已经存在时数据库会忽略，事件表行数不会增加。
pub(crate) fn count_datacenter_delete_statements(statements: &[String]) -> usize {
    let prefix = format!("INSERT IGNORE INTO {DATACENTER_DELETE} ");
    statements
        .iter()
        .filter(|statement| statement.starts_with(&prefix))
        .count()
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

/// 测试钩：`AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY="<sesno>:<ms>"`（或 `"*:<ms>"`）——窗口右端等于 `<sesno>` 时，在冻结源复核前
/// 睡 `<ms>` 毫秒。生产环境不设此变量，这里只是一次 `env::var`。只拉长时间、不改任何判据：复核本身还是
/// [`verify_frozen_source`] 那一道门，夹具在这段时间里写进下一个会话，门就该像真机上那样回「anchor 已改变」。
async fn delay_before_frozen_verify(end_sesno: u32) {
    let Ok(spec) = std::env::var("AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY") else {
        return;
    };
    let Some((which, millis)) = spec.split_once(':') else {
        return;
    };
    let Ok(millis) = millis.trim().parse::<u64>() else {
        return;
    };
    let which = which.trim();
    let applies = which == "*" || which.parse::<u32>().ok() == Some(end_sesno);
    if applies && millis > 0 {
        println!(
            "[测试钩] 窗口右端 {end_sesno}：冻结源复核前按 AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY 等 {millis} ms"
        );
        tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    }
}

/// 冻结源复核（commit-generation gate）：收集时冻住的那个文件世代，持久化之前必须还是它。
///
/// **这不是模型侧的优化。** 纯数据窗口（零模型候选）同样必须验：收集与持久化之间文件被
/// 原子替换或回拨，同号会话在新文件里是另一件事，写进去的就是另一个工程的数据。所以
/// `apply_one` 无条件调它，与计划出自哪个规划器无关——这是 spec 035 T407 拆出来要**保留**
/// 的那一半（另一半见 [`reconcile_plan_final_presence`]）。
///
/// 返回已验证的冻结源，供存活复核复用同一次打开。
fn verify_frozen_source(frozen: &FrozenSource, end_sesno: i32) -> anyhow::Result<FrozenSource> {
    anyhow::ensure!(end_sesno >= 0, "模型计划复核会话非法: {end_sesno}");
    anyhow::ensure!(
        frozen.target_sesno() == end_sesno as u32,
        "模型计划复核 target={} 与冻结 token target={} 不一致",
        end_sesno,
        frozen.target_sesno()
    );
    frozen
        .reopen_verified()
        .map_err(|error| anyhow::anyhow!("重开 Save Work 冻结快照失败: {error:#}"))
}

/// 计划目标 T 端存活复核——**只给老规划器**（spec 035 T412 / T407 的另一半）。
///
/// 老规划器的根是从 Surreal `pe_owner` 图上溯来的，而那张图是窗口**之前**的状态，
/// 所以要回文件问一句「这些根在 T 还在不在」。
///
/// 窗口规划器（`ModelUpdatePlan::window_planned`）的计划**不走这里**：它的根来自
/// `enumerate_generation_roots(DbSet@T)`，本身就是 T 端存活证明。再拿 old-pdms-io 复核
/// 一遍不但多余，还是个硬失败向量——ams7999 45→46 上
/// `contains_refnos_at(46, {24383/101198})` 报
/// `IndexPageData.noun == 0xCC47DF`（0830 坐实的「`0x18` 是 `free_dwords` 不是条目数」
/// 那一族解码 bug），而 `24383/101198` 正是 e3d-io 在 `DbSet@46` 上刚读到的新建 STRU 子树。
/// 整窗 `Err` → 批次 Failed → attempt 行已落 → replay 分支按固定区间重放同一份计划 →
/// 同一页再报错 → **该库水位永久卡死**，只能手删 attempt 行。
fn reconcile_plan_final_presence(
    frozen: &FrozenSource,
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
    let candidates = candidates.into_iter().collect::<BTreeSet<_>>();
    if candidates.is_empty() {
        return Ok(retain_finally_live_design_refnos(plan, |_| false));
    }
    anyhow::ensure!(
        end_sesno >= 0 && frozen.target_sesno() == end_sesno as u32,
        "存活复核 target={end_sesno} 与冻结源目标会话 {} 不一致",
        frozen.target_sesno()
    );
    let live = crate::data_interface::e3d_collector::contains_refnos_at(frozen, &candidates)
        .map_err(|error| anyhow::anyhow!("读取模型计划冻结目标 root 失败: {error:#}"))?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    Ok(reconcile_plan_with_live_set(plan, &live))
}

impl IncrementPipeline {
    pub fn new() -> Self {
        Self
    }

    /// 收集一个增量窗口的操作流——预览与执行体共用的**唯一**入口（ADR-011 同
    /// 谓词纪律）。
    ///
    /// 口径只有一种：净窗口（会话索引双根差分 + 两端版本合成，
    /// [`crate::data_interface::e3d_collector::collect_net_window`]，e3d-io 底座），与 core.dll
    /// `DB_IndexTableCompare` 同思想。ADR-031 之后**没有分支**——逐会话回放
    /// （`collect_changes`，old-pdms-io 底座）先退出生产路径，2026-09-04 随 old-pdms-io 整体删除，
    /// 连诊断门都不再有。对拍的判据是 E3D TTY 录下的会话链，不是另一套解析器。
    ///
    /// 返回完整 [`CollectedWindow`]；第一条 warning 固定为口径标注 + 全部容忍计数，
    /// `session_sesnos` 则始终从冻结区间内的会话页映射提取，不能从操作 key 反推。
    pub fn collect_window(
        path: &std::path::Path,
        sesno_range: RangeInclusive<i32>,
    ) -> anyhow::Result<CollectedWindow> {
        Self::collect_window_for_candidate(path, sesno_range, None)
    }

    pub(crate) fn collect_window_for_candidate(
        path: &std::path::Path,
        sesno_range: RangeInclusive<i32>,
        frozen: Option<&FrozenSource>,
    ) -> anyhow::Result<CollectedWindow> {
        let target = u32::try_from(*sesno_range.end())
            .map_err(|_| anyhow::anyhow!("dabacon 窗口终点非法: {}", sesno_range.end()))?;
        let frozen = if let Some(token) = frozen {
            anyhow::ensure!(
                token.matches_path(path),
                "候选路径 {} 与冻结 token 路径 {} 不一致",
                path.display(),
                token.path().display()
            );
            token.reopen_verified_at(target)
        } else {
            FrozenSource::capture_at(path, target)
        }
        .map_err(|e| anyhow::anyhow!("打开 dabacon 冻结源失败: {e}"))?;
        // 会话页清单与操作流共用**同一个**冻结源：`session_chain` 先验文件身份再读链，
        // 双开会允许同路径在两次读取之间被替换，拼出 A 文件的会话清单 + B 文件的操作流。
        let session_sesnos = crate::data_interface::db_file_header::session_sesnos_in_range(
            &frozen.session_chain()?,
            &sesno_range,
        );
        let outcome = crate::data_interface::e3d_collector::collect_net_window(
            &frozen,
            sesno_range,
            frozen.db_type(),
        )?;
        ensure_unique_terminal_operations(&outcome.window)?;
        let (mut added, mut deleted, mut modified) = (0usize, 0usize, 0usize);
        for operation in outcome.window.values().flatten() {
            match &operation.detail {
                EleOperationDetail::Add(_) => added += 1,
                EleOperationDetail::Deleted => deleted += 1,
                EleOperationDetail::Modified(_) => modified += 1,
                EleOperationDetail::None => {}
            }
        }
        let mut warnings = vec![net_caliber_warning(added, deleted, modified, &outcome)];
        warnings.extend(outcome.warnings);
        Ok(CollectedWindow {
            frozen: Some(outcome.frozen),
            persist_changes: terminal_persist_changes(&outcome.window, &outcome.tombstones),
            range_eles: outcome.window,
            session_sesnos,
            warnings,
            membership_deleted: 0,
            retired_type_rows: outcome.retired_type_rows,
        })
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
        increment_ranges_map: IndexMap<PathBuf, (DbFileHeader, RangeInclusive<i32>, String)>,
    ) -> IncrResult {
        self.apply_with_precollected(increment_ranges_map, IndexMap::new())
            .await
    }

    /// 同 [`Self::apply`]，但允许调用方交出**已经收集好**的增量窗口，避免同一个文件
    /// 被完整解析两次。
    ///
    /// 背景：`manual_update::execute_one_dbnum` 为了算 `changed_elements` 和 DESI 的
    /// 单元归并，会先 `collect_window` 一次；随后 `apply_one` 在 fresh 分支里又收集
    /// 了一遍同一文件、同一窗口。非 DESI 库（SYST/CATA/DICT）尤其亏——第一趟的整份
    /// 结果只被用来算两个标量。实测 dbnum=250206 单趟就要 5 分多钟。
    ///
    /// 交入的结果**仅在恰好覆盖本次要应用的区间时**才被采信：崩溃重放走的是持久化
    /// 的固定区间，可能与 `requested_range` 不同，那种情况永远重新收集。
    pub async fn apply_with_precollected(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbFileHeader, RangeInclusive<i32>, String)>,
        mut precollected: IndexMap<PathBuf, (RangeInclusive<i32>, CollectedWindow)>,
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

            let (outcome, warnings) = self
                .apply_one(&path, &basic_info, sesno_range, &db_type, handed_in)
                .await;
            result.warnings.extend(warnings);
            match outcome {
                Ok(success) => result.successes.push(success),
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
        basic_info: &DbFileHeader,
        requested_range: RangeInclusive<i32>,
        db_type: &str,
        precollected: Option<(RangeInclusive<i32>, CollectedWindow)>,
    ) -> (anyhow::Result<IncrFileSuccess>, Vec<String>) {
        let mut warnings = Vec::new();
        let outcome = async {
        let mut timings = StageTimings::default();
        let dbnum = basic_info.dbnum as u32;
        let path_text = path.to_string_lossy().into_owned();

        // 崩溃恢复不在这一层了（ADR-072 D1）：从前这里先读 `increment_update_attempt`，
        // 见到未收口的记录就**复用旧计划**原样重放那一段。现在写入标记
        // （`write_state::mark_writing`）说「上一次死在写库中途」，任务开工前 / 启动水化段
        // 先把该库回退到 S，再从头重算 S→T——半应用的库不会被拿来当规划输入，所以
        // 这里只认调用方交下来的那个区间。
        let _ = &path_text;
        // 采信判定见 `accept_handed_in_window`；复用时 `timings.collect`
        // 自然为 0，收集成本记在调用方那一侧。
        let collected = match accept_handed_in_window(precollected, &requested_range) {
            Some(collected) => collected,
            None => {
                let start = Instant::now();
                let collected = Self::collect_window(path, requested_range.clone())?;
                timings.collect += start.elapsed();
                collected
            }
        };
        // 口径 warning 必须先于任何可能失败的计划 I/O 进入旁路；
        // `apply_one` 即使随后返回 Err，也会把这份 warnings 交给调用方。
        warnings.extend(collected.warnings.iter().cloned());

        // Direct DESI uses the same file-pinned S→T root planner as the
        // live e3d-model parity test.  Other modes retain the established
        // CATA temporarily keeps its isolated ref_rev cascade planner.
        let base_sesno = requested_range.start().saturating_sub(1) as u32;
        let mut model_plan = StageTimings::measure(
            &mut timings.plan,
            crate::data_interface::window_root_plan::build_effective_model_update_plan(
                dbnum,
                db_type,
                base_sesno,
                *requested_range.end() as u32,
                &collected.range_eles,
            ),
        )
        .await?;
        let sesno_range = requested_range;
        let collected = Some(collected);
        let start_sesno = *sesno_range.start();
        let end_sesno = *sesno_range.end();

        println!(
            "IncrementPipeline: {:?}, db_type={}, sesno range: {:?}",
            path, db_type, &sesno_range
        );
        // 测试钩（只在设了 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` 时生效）：把「收集已冻结、尚未复核」这一段拉长，
        // 让 L2 回放夹具能确定性地在这里塞进下一个会话（INC-H01「保存中途被看见」：真机上是 E3D 在规划期间写下一会话）。
        delay_before_frozen_verify(end_sesno as u32).await;

        let collected = collected.expect("the window is collected before the plan is built");
        let frozen = collected.frozen.ok_or_else(|| {
            anyhow::anyhow!("增量窗口缺冻结源 FrozenSource，拒绝持久化和推进水位")
        })?;
        let session_sesnos = collected.session_sesnos;
        let persist_changes = collected.persist_changes;
        let range_eles = collected.range_eles;
        let retired_type_rows = collected.retired_type_rows;
        // 整块在一个同步作用域里跑完：复核与存活点查都是文件 I/O，不跨 `.await` 持有引擎。
        let removed_plan_refnos = {
            // 冻结源复核无条件跑：纯数据窗口也不许在收集与持久化之间被换掉文件。
            let verified = verify_frozen_source(&frozen, end_sesno)?;
            // 存活复核只给老规划器。窗口规划器的根是 `DbSet@T` 枚举出来的，自身即 T 端
            // 存活证明；再问一遍是多余的，而且曾把整窗打死（T412）。
            if model_plan.window_planned {
                0
            } else {
                reconcile_plan_final_presence(&verified, end_sesno, &mut model_plan)?
            }
        };
        if removed_plan_refnos > 0 {
            warnings.push(format!(
                "dbnum={dbnum}: 从持久模型计划收敛 {removed_plan_refnos} 个 Save Work 后不存在的设计目标"
            ));
        }
        let mut cache_refnos = Self::collect_cache_invalidation_refnos(&range_eles);
        // 生成根级失效（ADR-010 残余关闭）：`QUERY_DEEP_CHILDREN_REFNOS` 按子树根
        // 为键，「变更元素 + 属主」的失效集够不着深层后代之上的高层根，同根下一次
        // 重生成会拿旧成员表静默漏算。计划层刚算出生成根，失效按根补齐。
        cache_refnos.extend(model_plan.regen_root_refnos());
        warnings.extend(model_plan.warnings.iter().cloned());

        // CATA 依赖块排在 DESI 块与水位**之前**（plan 2026-09-06 §3 D6 (a) / Q3）：本窗口写回的 DESI 元素
        // 所依赖的目录链元素先入库，缺依赖的窗口不算落地。闭包解析失败（locator / 文件 I/O）拦水位，
        // 与 DESI 块同纪律；「库不在 MDB / 元素不在」只进 warning。非 DESI 窗口（SYS / DICT）没有目录链。
        let cata_rows = if db_type.eq_ignore_ascii_case("DESI") {
            let seeds = crate::data_interface::cata_dependency::seeds_from_elements(
                persist_changes.iter().filter_map(|change| match &change.state {
                    crate::data_interface::ele_ops::PersistElementState::Add(data)
                    | crate::data_interface::ele_ops::PersistElementState::Modified(data) => {
                        Some(data)
                    }
                    crate::data_interface::ele_ops::PersistElementState::Deleted { .. } => None,
                }),
            );
            if seeds.is_empty() {
                None
            } else {
                let project = aios_core::get_db_option().project_name.clone();
                let rows = StageTimings::measure(
                    &mut timings.cata_dependency,
                    crate::data_interface::cata_dependency::resolve_dependency_rows(
                        &project, dbnum, &seeds,
                    ),
                )
                .await?;
                if rows.manifest.missing > 0 {
                    warnings.push(format!(
                        "dbnum={dbnum}: CATA 依赖闭包缺失 {} 项，详情随窗口工件保留",
                        rows.manifest.missing
                    ));
                }
                Some(rows)
            }
        } else {
            None
        };

        // 只保留最新数据：仅写入 pe 主数据（最新状态），不再写 sessions / element_changes 历史表
        //
        // Cache invalidation must run after every attempted persist, including a
        // partially failed batch: earlier Surreal statements may already have
        // changed data even though the watermark must remain unchanged.
        // 结构数据不在 Task 热路径直写；确定性内容随窗口工件落盘，提交证明出现后由
        // 单项目串行 sink 投影。这里的 persist 槽只统计工件组装/落盘，见下方。
        // 失效名单随后还要装进工件：lane 落完结构行会按同一份名单再失效一次
        // （`IncrementWindowArtifact::cache_refnos` 说了为什么）。
        let invalidated =
            StageTimings::measure(&mut timings.cache, Self::invalidate_caches(&cache_refnos)).await;
        if invalidated > 0 {
            println!(
                "IncrementPipeline: invalidated {invalidated} PE/attribute cache entries \
                 and world-transform caches"
            );
        }

        // ADR-003 B1-emit / ADR-063：`ref_rev` **只在摄入形态维护，且只服务 UI**。
        //
        // 它曾是「关联模型也要更新」的权威来源，所以维护失败要进持久补偿队列、补偿也落不
        // 下就不推进水位。ADR-063 把这个口径降了：反向查询的权威源是**文件里的引用表**
        // （e3d-io `referrers_of`），选根一行都不读 `ref_rev`。留下的维护只为属性面板那类
        // UI 便利，所以失败仍进补偿队列（口径不变，别让 UI 悄悄看到半张表），但读透形态
        // 整个不跑——那一档本来就没有第二份事实要维护。
        //
        // 跳过之后**不许**有人把「索引没维护」读成「没有引用者」：`fetch_ref_rev_edges`
        // 在同一形态下直接报错，调用方走既有的 `cascade_deferred` 退化路（见 ADR-063 D3）。
        let reverse_index_statements = if crate::options::data_face().is_read_through() {
            Vec::new()
        } else {
            crate::data_interface::manual_update::build_reverse_index_statements(&range_eles)
        };

        // 水位推进要顺手存下右端那条保存的写入时刻（plant-ui ADR-0019 Q6）：文件被换回
        // 旧版本之后这一页就读不到了，这一刻是唯一能存下来的时机。一页会话页，每批一次。
        let end_sesno_time =
            crate::data_interface::manual_update::session_time_rfc3339("", path, end_sesno);
        // 交付删除记录的 `sesno_time` 取变化所在那一版的写入时刻（plan 2026-09-22 D3，不用
        // `time::now()`，重放读到的还是同一页）。
        let session_time_of = |sesno: u32| {
            i32::try_from(sesno).ok().and_then(|sesno| {
                crate::data_interface::manual_update::session_time_rfc3339("", path, sesno)
            })
        };

        // Resolve-then-render（W3）：这个计时槽现在只含**窗口前持久态的点查**
        // （overlay 兜不住的祖先 noun/owner/name），产出的语句是固定目标 id 的纯
        // UPDATE / UPSERT——收口事务里不再有任何 datacenter 上溯 I/O，也不再依赖持久层
        // 灌了 fn::find_ancestor_types。
        let mut window_statements = StageTimings::measure(
            &mut timings.datacenter,
            Self::datacenter_statements(&range_eles, db_type, dbnum, &session_time_of),
        )
        .await?;
        // OWNER 搬迁的定点 anc 重算先于水位提交、失败即拦住水位
        // （窗口重放时随窗口语句批重算，幂等收敛）。
        window_statements.extend(Self::anc_repair_statements_for_window(&range_eles, db_type));
        // refno 复用（窗口内换了 noun）的旧 noun 表行不再单独排进尾事务：它随元素块的删除块
        // 一起清（`element_batch::items_from_changes` 的 `old_noun`，plan 2026-09-06 §3 D2）。

        let project = aios_core::get_db_option().project_name.clone();
        let from_sesno = u32::try_from(start_sesno.saturating_sub(1))?;
        let target_sesno = u32::try_from(end_sesno)?;
        // 本窗的结构行（pe / ATT_ / CATA 依赖 / `ref_rev` / 随窗语句）先在内存里装成一份窗口产物，
        // 再**同步**落库——内存 → 语句 → 库，一步到位（ADR-071 D1 / D11）：没有 journal、没有
        // lane、没有 ACK。行先落、水位最后（ADR-071 不变量 S3）：落库失败就在这里返回，水位不动、
        // attempt 记录仍在，整个固定区间重放收敛（落库幂等，`landing_the_same_window_twice_converges`）。
        let artifact = crate::data_interface::window_landing::IncrementWindowArtifact::from_window(
            &project,
            dbnum,
            db_type,
            from_sesno,
            target_sesno,
            &persist_changes,
            &retired_type_rows,
            cata_rows.as_ref(),
            reverse_index_statements,
            window_statements,
            cache_refnos.iter().copied(),
        );
        let landing = StageTimings::measure(
            &mut timings.persist,
            crate::data_interface::window_landing::land_increment_window(
                &aios_core::SUL_DB,
                u64::from(target_sesno),
                &artifact,
            ),
        )
        .await
        .with_context(|| {
            format!(
                "dbnum={dbnum} 窗口 {from_sesno}..={target_sesno} 结构行落库失败，水位不动、\
                 attempt 记录保留，整窗重放收敛"
            )
        })?;
        warnings.extend(landing.warnings.iter().cloned());
        println!(
            "[data-window] dbnum={dbnum} 窗口 {from_sesno}..={target_sesno} 结构行落库：元素 {}（{} 笔事务）、\
             CATA 依赖 {} 行（{} 笔）、ref_rev {} 句、随窗 {} 句",
            landing.elements,
            landing.element_transactions,
            landing.cata_elements,
            landing.cata_transactions,
            landing.reverse_index_statements,
            landing.window_statements
        );

        // Finalization publishes this window's delivery-status updates as ordered
        // chunked batches, then one tail transaction establishes durable model
        // work, advances the watermark and removes the short-lived recovery
        // record. If any step fails, the watermark is untouched, the attempt
        // remains, and the whole fixed range is safe to replay (the idempotent
        // fixed-target updates converge on re-application).
        // Recheck the stable file generation immediately before publishing the
        // watermark tail. Same-file append is accepted; atomic path replacement
        // is not.
        frozen
            .reopen_verified()
            .map_err(|error| anyhow::anyhow!("提交前冻结源身份复核失败: {error:#}"))?;

        // The tail receives the complete durable plan, including RegenRoot: the
        // model phase runs after the watermark (ADR-025 §7 / ADR-056 D1), so
        // stripping roots here would advance the data watermark while
        // permanently losing the only durable request to generate them.
        StageTimings::measure(
            &mut timings.finalize,
            crate::data_interface::increment_tail::finalize_attempt(
                dbnum,
                end_sesno,
                end_sesno_time.as_deref(),
                &model_plan,
                &[],
            ),
        )
        .await?;
        // 前移那一格只剩回执材料（`DataBatchResult.credential_advance`，ADR-079）：名单上的根
        // 由任务收尾盖到 T 的 `model_sesno` 承诺，库里没有行要改；名单为空且规划器没退化时整格没有。
        let credential_advance =
            crate::data_interface::window_root_plan::CredentialAdvanceReceipt::from_plan(
                &model_plan,
            );

        // 水位之后不再有 CATA 落库的补偿任务（ADR-064 Q2，2026-09-07 退役 D8-A 的
        // `enqueue_cata_ref_rev`）：本窗口 DESI 元素的目录链依赖已由 `persist_cata_dependencies`
        // 排在 DESI 块与水位**之前**落进 `pe`（`dependency = true`），`ref_rev` 的 DESI→CATA 边由
        // 上面的 reverse_index 步维护——补偿队列剩下的只是「按根子树的全出向引用再解一遍闭包、
        // 水位之后逐元素重写目录行」，第二份判据、第二个写者，还会抹掉 `dependency` 标记。

        timings.report(
            dbnum,
            db_type,
            range_eles.values().map(|ops| ops.len()).sum::<usize>(),
        );

        let changed_refnos = Self::changed_refnos(&range_eles);

        Ok(IncrFileSuccess {
                path: path.clone(),
                dbnum,
                start_sesno,
                end_sesno,
                db_type: db_type.to_string(),
                changed_refnos,
                session_sesnos,
                range_eles,
                model_plan,
                credential_advance,
            })
        }
        .await;
        (outcome, warnings)
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
    /// 两个调用点：Task 在 journal 之前（`apply_one`），lane 在结构行落定之后
    /// （`window_landing::land_increment_window`，名单随工件走）。
    pub(crate) async fn invalidate_caches(refnos: impl IntoIterator<Item = &RefnoEnum>) -> usize {
        let refnos: Vec<RefnoEnum> = refnos.into_iter().copied().collect();
        if refnos.is_empty() {
            return 0;
        }
        clear_all_caches_batch(&refnos).await;
        refnos.len()
    }

    /// Persist ONLY the latest main data (pe + attributes) for this delta.
    ///
    /// Deliberately skips the history/version tables (`sessions` /
    /// `element_changes`): we keep only the latest state, no historical
    /// versions.
    ///
    /// 写法只有一种（plan 2026-09-06 §3 D1/D2，`element_batch`）：前置块清 Add / 墓碑自己的旧出向边，
    /// 然后每块「先按 id 删整组表示，再每表一条 `INSERT IGNORE`」——被删元素落成墓碑 `pe` 行
    /// （`deleted = true`，noun / owner 保留），换了 noun 的元素旧表行随删除块一起清。
    /// 幂等：重放撞上上一轮写了一半的块，删除块把它清掉、插入块重建，终态收敛。
    ///
    /// Any batch write failure is propagated (ADR-001): the caller must NOT
    /// advance the watermark unless the whole batch persisted. Swallowing errors
    /// here would let `applied_sesno` run ahead of the data actually stored.
    ///
    /// 返回值是回执 warnings：UDA 键查不到的、墓碑退化的，都不拦水位。
    async fn persist_latest_main_data(
        changes: &[PersistElementChange],
        retired: &[RetiredTypeRow],
        dbnum: i32,
    ) -> anyhow::Result<Vec<String>> {
        // `ATT_UDA` 的 `u` 链接要把 UKEY 换成 `UDA:<refno>`：每窗口读一次 `UDA` 表，
        // 查不到的键随返回值进回执 `warnings`（不静默丢，也不拦水位——元素其它属性照常落库）。
        let uda_keys = UdaKeyMap::load().await?;
        let (batches, report) = Self::terminal_persist_batches(changes, retired, dbnum, &uda_keys)?;
        for tx_sql in &batches {
            // `.check()`：把事务内被取消/失败的语句错误上浮为 Err。原实现只 map_err
            // 传输错误、未 check 语句级错误，事务被取消时仍可能返回 Ok → 水位误推进。
            crate::data_interface::pe_batch::execute_on(&SUL_DB, tx_sql).await?;
        }

        println!(
            "增量主数据落库完成，共 {} 条（批量先删后插，{} 笔事务，最大一笔 {} 字节，墓碑 {}；仅最新状态，不写历史）",
            report.elements, report.transactions, report.max_transaction_bytes, report.tombstones
        );
        let mut warnings = Vec::new();
        warnings.extend(uda_keys.misses_warning(dbnum as u32));
        warnings.extend(report.warning(dbnum as u32));
        Ok(warnings)
    }

    /// 整窗口的事务 SQL（含前置块与 pe 统计观测）与账——渲染在 `element_batch`，这里只是
    /// 增量路的入口，`window_repair` 与崩溃重放对拍也从这里拿，保证三处发的是同一批语句。
    pub(crate) fn terminal_persist_batches(
        changes: &[PersistElementChange],
        retired: &[RetiredTypeRow],
        dbnum: i32,
        uda_keys: &UdaKeyMap,
    ) -> anyhow::Result<(
        Vec<String>,
        crate::data_interface::element_batch::ElementBatchReport,
    )> {
        crate::data_interface::element_batch::render_transactions(
            changes,
            retired,
            dbnum,
            uda_keys,
            crate::data_interface::element_batch::WriteMode::Replace,
        )
    }

    /// 渲染差分操作的兼容语句，供尚未迁移的诊断与测试载体使用。
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
        // An Add may reuse a numeric refno from an older, now-absent element
        // generation. Clear all incoming and outgoing ownership edges for every final Add
        // before rendering any operation: the current owner edge is commonly
        // installed by the owner's child-list update, whose refno may sort before
        // or after the child. Embedding this cleanup in the child's own statement
        // could therefore erase the freshly installed edge. The scoped id-range
        // delete also removes child slots left by the old generation when the new
        // element is a leaf. These deletes are in
        // the same staged data transaction as the Add and are replay-safe.
        for write in planned
            .iter()
            .filter(|write| matches!(&write.op.detail, EleOperationDetail::Add(_)))
        {
            let pe = format!("pe:{}", write.op.refno);
            // `WHERE in = ...` makes SurrealDB 2.1 scan the full relation table during
            // DELETE (105k+ model trees already exceed the 120s staging deadline). The
            // graph target follows the same `in` side through the relation adjacency
            // index and remains a deterministic ReplaySafe target.
            statements.push(format!("DELETE {pe}->pe_owner;"));
            statements.push(format!("DELETE pe_owner:[{pe}, NONE]..=[{pe}, ..];"));
        }
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
    ) -> HashMap<RefU64, PeChainView> {
        use crate::data_interface::cata_closure::is_valid_ref0;
        use crate::data_interface::model_impact::{added_owner, owner_change};
        let mut states: HashMap<RefU64, PeChainView> = HashMap::new();
        for op in data.values().flatten() {
            let entry = states.entry(op.refno).or_default();
            match &op.detail {
                EleOperationDetail::Add(added) => {
                    let noun = op.get_noun_type();
                    if !noun.is_empty() {
                        entry.noun = Some(noun);
                    }
                    if let Some(owner) = added_owner(op) {
                        entry.owner = Some(owner.refno());
                    }
                    if !added.name.is_empty() {
                        entry.name = Some(added.name.clone());
                    }
                }
                EleOperationDetail::Modified(modified) => {
                    if !modified.noun.is_empty() {
                        entry.noun = Some(modified.noun.clone());
                    }
                    if is_valid_ref0(modified.current_data.owner.get_0()) {
                        entry.owner = Some(modified.current_data.owner);
                    } else if let (_, Some(new_owner)) = owner_change(op) {
                        entry.owner = Some(new_owner.refno());
                    }
                    if !modified.current_data.name.is_empty() {
                        entry.name = Some(modified.current_data.name.clone());
                    } else if let Some((_, new_name)) = delivery_unit_name_change(modified) {
                        entry.name = Some(new_name);
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
    ///
    /// 同一趟还渲染**交付删除记录**（[`DATACENTER_DELETE`]，plan 2026-09-22 D4）：交付单元
    /// **自身**被删 → 记删除前的位号；交付单元自身 `NAME` 变 → 记改前的位号。固定目标
    /// `INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:⟨refno⟩, … }`，同一 refno
    /// 只保留首次事件；写前须确认 `datacenter_version:{refno}` 已存在（最新 issue #1）；
    /// `zone_refno` 沿 owner 链上溯到最近 `ZONE`（D5）。
    /// 下属管件的增删改名不记（issue 原文「不包含其下属管件」），照旧只让单元记 Modify。
    /// 每条记录的 `sesno` 是变化所在的会话，`sesno_time` 取**同一版**的写入时刻（`session_time`，
    /// 读不到给 `None`）：一窗常跨好几个会话，窗口末端那一版的时刻对不上记录上的 `sesno`。
    async fn resolve_datacenter_statements_with<F, Fut, S>(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
        delivery_unit_types: &[String],
        dbnum: u32,
        session_time: S,
        published_refnos: &HashSet<RefU64>,
        mut load_pe: F,
    ) -> anyhow::Result<Vec<String>>
    where
        F: FnMut(Vec<RefU64>) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<HashMap<RefU64, PeChainView>>>,
        S: Fn(u32) -> Option<String>,
    {
        /// 防御性走链上限（owner 环 / 数据损坏的最后一道闸）。
        const ROLLUP_WALK_CAP: usize = 64;

        /// 逐 refno 的效果视图（noun / owner / name）：overlay 字段优先，缺的问持久层
        /// （懒加载 + 记忆化；`cache` 里 `None` = 行不存在，问过了）。
        async fn effective_view<F, Fut>(
            overlay: &HashMap<RefU64, PeChainView>,
            cache: &mut HashMap<RefU64, Option<PeChainView>>,
            load_pe: &mut F,
            refno: RefU64,
        ) -> anyhow::Result<PeChainView>
        where
            F: FnMut(Vec<RefU64>) -> Fut,
            Fut: std::future::Future<Output = anyhow::Result<HashMap<RefU64, PeChainView>>>,
        {
            use crate::data_interface::cata_closure::is_valid_ref0;
            let overlay_entry = overlay.get(&refno).cloned().unwrap_or_default();
            if overlay_entry.noun.is_some() && overlay_entry.owner.is_some() {
                return Ok(overlay_entry);
            }
            if !cache.contains_key(&refno) {
                let fetched = load_pe(vec![refno]).await?;
                cache.insert(refno, fetched.get(&refno).cloned());
            }
            let stored = cache.get(&refno).cloned().flatten().unwrap_or_default();
            Ok(PeChainView {
                noun: overlay_entry.noun.or(stored.noun),
                owner: overlay_entry
                    .owner
                    .or(stored.owner.filter(|owner| is_valid_ref0(owner.get_0()))),
                name: overlay_entry.name.or(stored.name),
            })
        }

        /// 从 `start`（含自身）沿 owner 链上溯到最近的 `ZONE`（D5）。链断 / 到顶 → `None`。
        async fn nearest_zone<F, Fut>(
            overlay: &HashMap<RefU64, PeChainView>,
            cache: &mut HashMap<RefU64, Option<PeChainView>>,
            load_pe: &mut F,
            start: Option<RefU64>,
        ) -> anyhow::Result<Option<RefU64>>
        where
            F: FnMut(Vec<RefU64>) -> Fut,
            Fut: std::future::Future<Output = anyhow::Result<HashMap<RefU64, PeChainView>>>,
        {
            let mut current = start;
            for _ in 0..ROLLUP_WALK_CAP {
                let Some(refno) = current else {
                    return Ok(None);
                };
                let view = effective_view(overlay, cache, load_pe, refno).await?;
                match view.noun.as_deref() {
                    None => return Ok(None), // 链断：行不在
                    Some("ZONE") => return Ok(Some(refno)),
                    Some(_) => current = view.owner,
                }
            }
            Ok(None)
        }

        let mut unit = delivery_unit_types.to_vec();
        unit.push("ZONE".into());
        let is_delivery_unit = |noun: &str| delivery_unit_types.iter().any(|unit| unit == noun);
        let overlay = Self::window_net_states(data);
        // 持久层窗口前态缓存：None = 行不存在（问过了）。
        let mut persistent: HashMap<RefU64, Option<PeChainView>> = HashMap::new();
        // 会话 → 写入时刻，只问真落记录的会话，一个会话问一次。
        let mut session_times: HashMap<u32, Option<String>> = HashMap::new();

        let mut statements = Vec::new();
        // Modify 状态归组（plan 2026-09-06 §1.1 #7 / P3）：同状态、无别的字段的目标攒成一条多目标
        // `UPDATE a, b, c SET status = 'Modify'`（fork 兼容性见 `fork_surreal_compat::dual_multi_target_update_*`）。
        // Delete 带各自的 `belong_zone`，仍逐条。
        let mut modify_targets: Vec<String> = Vec::new();
        for ops in data.values() {
            for d in ops {
                match &d.detail {
                    EleOperationDetail::Deleted => {
                        // Deleted 的 overlay 为空，这里读到的就是窗口前态（墓碑保留的
                        // noun / owner / name）——artifact 先渲后落，持久层此刻还是 S 侧。
                        let view = effective_view(&overlay, &mut persistent, &mut load_pe, d.refno)
                            .await?;
                        let belong_zone = match (view.noun.as_deref(), view.owner) {
                            // BRAN 的归属 ZONE 隔一层 PIPE（与老形态的
                            // `$pe.owner.owner` 同义）。
                            (Some("BRAN"), Some(owner)) => {
                                effective_view(&overlay, &mut persistent, &mut load_pe, owner)
                                    .await?
                                    .owner
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

                        // D4 删除分支：交付单元自身被删 → 记删除前的位号。
                        if let (Some(noun), Some(name)) =
                            (view.noun.as_deref(), view.name.as_deref())
                        {
                            if is_delivery_unit(noun)
                                && !name.is_empty()
                                && published_refnos.contains(&d.refno)
                            {
                                let zone = nearest_zone(
                                    &overlay,
                                    &mut persistent,
                                    &mut load_pe,
                                    view.owner,
                                )
                                .await?;
                                let sesno_time = session_times
                                    .entry(d.sesno)
                                    .or_insert_with(|| session_time(d.sesno))
                                    .clone();
                                statements.push(render_datacenter_delete_insert_ignore(
                                    &DatacenterDeleteRecord {
                                        refno: d.refno,
                                        name,
                                        noun,
                                        zone,
                                        dbnum,
                                        sesno: d.sesno,
                                        sesno_time: sesno_time.as_deref(),
                                        reason: "deleted",
                                    },
                                ));
                            }
                        }
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
                                let view = effective_view(
                                    &overlay,
                                    &mut persistent,
                                    &mut load_pe,
                                    current,
                                )
                                .await?;
                                let Some(noun) = view.noun else {
                                    break; // 链断：行不在（老形态同样解析不出）
                                };
                                if unit.iter().any(|unit_noun| unit_noun == &noun) {
                                    target = Some(current);
                                    break;
                                }
                                let Some(owner) = view.owner else {
                                    break; // 到顶（WORL 之上）仍没有单元层
                                };
                                current = owner;
                            }
                            target
                        };
                        match target {
                            Some(target) => {
                                let key = target.to_table_key(DATACENTER_VERSION);
                                if !modify_targets.contains(&key) {
                                    modify_targets.push(key);
                                }
                            }
                            None => println!(
                                "datacenter 上溯：{} 解不出单元层归属（链断或到顶），\
                                 无交付记录可标，跳过",
                                d.refno.to_pe_key()
                            ),
                        }

                        // D4 改名分支：交付单元自身的 NAME 变了 → 记改前的位号。
                        // 老值不存在（NAME 是本窗新出现的）不记：中台没有按空位号发布过的东西。
                        if is_delivery_unit(&modify_data.noun)
                            && published_refnos.contains(&d.refno)
                        {
                            if let Some((old_name, new_name)) =
                                delivery_unit_name_change(modify_data)
                            {
                                if !old_name.is_empty() && old_name != new_name {
                                    let view = effective_view(
                                        &overlay,
                                        &mut persistent,
                                        &mut load_pe,
                                        d.refno,
                                    )
                                    .await?;
                                    let zone = nearest_zone(
                                        &overlay,
                                        &mut persistent,
                                        &mut load_pe,
                                        view.owner,
                                    )
                                    .await?;
                                    let sesno_time = session_times
                                        .entry(d.sesno)
                                        .or_insert_with(|| session_time(d.sesno))
                                        .clone();
                                    statements.push(render_datacenter_delete_insert_ignore(
                                        &DatacenterDeleteRecord {
                                            refno: d.refno,
                                            name: &old_name,
                                            noun: &modify_data.noun,
                                            zone,
                                            dbnum,
                                            sesno: d.sesno,
                                            sesno_time: sesno_time.as_deref(),
                                            reason: "renamed",
                                        },
                                    ));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // 一条多目标 UPDATE 最多带这么多记录：几千个目标一条语句是几百 KB 的报文，按 500 切、仍是批量。
        const DATACENTER_TARGETS_PER_UPDATE: usize = 500;
        let mut statements = dedup_statements_keep_last(statements);
        for chunk in modify_targets.chunks(DATACENTER_TARGETS_PER_UPDATE) {
            statements.push(format!(
                "update {} set status = '{:?}';",
                chunk.join(", "),
                DataCenterRecordOperate::Modify
            ));
        }
        Ok(statements)
    }

    /// 窗口前持久态的 pe 点查（noun + owner + name），显式 `SUL_DB` 不经读路由——
    /// 与锁域解析同一纪律；软删行仍在场，Deleted 分支读到的正是删除前的归属与位号。
    async fn load_pe_chain_view_from_persistent(
        refnos: Vec<RefU64>,
    ) -> anyhow::Result<HashMap<RefU64, PeChainView>> {
        #[derive(serde::Deserialize)]
        struct PeChainRow {
            id: RefnoEnum,
            #[serde(default)]
            noun: Option<String>,
            #[serde(default)]
            owner: Option<RefnoEnum>,
            #[serde(default)]
            name: Option<String>,
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
                    "SELECT id, noun, owner, name FROM [{keys}] WHERE record::exists(id);"
                ))
                .await?
                .check()?;
            let rows: Vec<PeChainRow> = response.take(0)?;
            for row in rows {
                out.insert(
                    row.id.refno(),
                    PeChainView {
                        noun: row.noun,
                        owner: row.owner.map(|owner| owner.refno()),
                        name: row.name.filter(|name| !name.is_empty()),
                    },
                );
            }
        }
        Ok(out)
    }

    /// 最新 issue #1 的插入门：只记录已经发布到中台的交付单元。`datacenter_version` 与事件表
    /// 使用同一个 refno 值作 ID，因此对窗口内变更 refno 做一轮分块显式点查即可。
    async fn load_published_datacenter_refnos(
        refnos: Vec<RefU64>,
    ) -> anyhow::Result<HashSet<RefU64>> {
        Self::load_published_datacenter_refnos_on(&SUL_DB, refnos).await
    }

    async fn load_published_datacenter_refnos_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        refnos: Vec<RefU64>,
    ) -> anyhow::Result<HashSet<RefU64>> {
        let mut out = HashSet::new();
        for chunk in refnos.chunks(200) {
            let keys = chunk
                .iter()
                .map(|refno| refno.to_table_key(DATACENTER_VERSION))
                .collect::<Vec<_>>()
                .join(",");
            let mut response = db
                .query(format!(
                    "SELECT VALUE id FROM [{keys}] WHERE record::exists(id);"
                ))
                .await?
                .check()?;
            for id in response.take::<Vec<RefnoEnum>>(0)? {
                out.insert(id.refno());
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
    /// (`increment_tail::FinalizeRender` has the full argument).
    async fn datacenter_statements<S>(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
        db_type: &str,
        dbnum: u32,
        session_time: S,
    ) -> anyhow::Result<Vec<String>>
    where
        S: Fn(u32) -> Option<String>,
    {
        if db_type != "DESI" {
            return Ok(Vec::new());
        }

        let delivery_unit_types =
            crate::data_interface::generation_root::configured_delivery_unit_types();
        let changed_refnos = data
            .values()
            .flatten()
            .map(|operation| operation.refno)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let published_refnos = Self::load_published_datacenter_refnos(changed_refnos).await?;
        Self::resolve_datacenter_statements_with(
            data,
            &delivery_unit_types,
            dbnum,
            session_time,
            &published_refnos,
            Self::load_pe_chain_view_from_persistent,
        )
        .await
    }

    /// 预览回执的 `datacenter_delete_rows`（plan 2026-09-22 T4）：这一窗会落几条交付删除记录。
    /// 走与执行体**同一条**渲染路（[`Self::datacenter_statements`]，含持久层窗口前态点查），
    /// 数出来的就是执行会写的——只计数、不改任何判定；非 DESI 窗口恒为 0。会话时刻一律不读：
    /// 它只是记录上的一个字段，不影响条数。
    pub(crate) async fn count_datacenter_delete_records(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
        db_type: &str,
        dbnum: u32,
    ) -> anyhow::Result<usize> {
        let statements = Self::datacenter_statements(data, db_type, dbnum, |_| None).await?;
        Ok(count_datacenter_delete_statements(&statements))
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
    /// 才轮到直写失效。
    #[test]
    fn cache_invalidation_extends_to_the_plans_regen_roots() {
        let source = include_str!("increment_pipeline.rs");
        let collect_at = source
            .find("Self::collect_cache_invalidation_refnos(&range_eles)")
            .expect("元素级失效集必须存在");
        let extend_at = source
            .find("cache_refnos.extend(model_plan.regen_root_refnos())")
            .expect("失效集必须并入生成根");
        let invalidate_at = source
            .find("Self::invalidate_caches(&cache_refnos)")
            .expect("Task 侧的失效必须存在");
        assert!(
            collect_at < extend_at && extend_at < invalidate_at,
            "顺序必须是 collect → extend(regen roots) → 直写失效"
        );
    }

    /// `apply_one` 的函数体（到 `fn anc_repair_statements_for_window(` 为止）。
    fn apply_one_body() -> &'static str {
        include_str!("increment_pipeline.rs")
            .split_once("async fn apply_one(")
            .expect("apply_one must exist")
            .1
            .split_once("fn anc_repair_statements_for_window(")
            .expect("anc_repair_statements_for_window must follow apply_one")
            .0
    }

    /// ADR-056 P1（spec 035 成功标准 1）：`apply_one` 只剩一条路——函数体里不得再出现任何
    /// `staging::` 引用，也不得再按窗口模式丢弃持久化的恢复记录。
    ///
    /// 2026-09-10 起这条路是 **任务内同步序列化**（ADR-071 D1 / D11，共识 d-285 / d-288 作废了
    /// (a) 的 lane）：Task 解闭包、渲染、失效缓存、装成内存里的窗口产物，随即**同步**落库，再走
    /// 尾事务。顺序纪律：闭包解析（失败拦水位）→ 缓存失效 → `ref_rev` 渲染 → 随窗语句渲染 →
    /// 窗口产物 → `land_increment_window`（结构行落库）→ 冻结源复核 → `finalize_attempt`
    /// （尾事务，水位）。行先落、水位最后，中间没有 journal / accept / complete。
    #[test]
    fn apply_one_has_no_staging_fork() {
        let body = apply_one_body();
        assert!(
            !body.contains("staging::"),
            "apply_one 不得再引用 staging（ADR-056 P1）"
        );
        assert!(
            !body.contains("prepared.filter(") && !body.contains("should_rebuild_stale"),
            "直写路径从不丢弃一份持久化的恢复记录"
        );
        let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("缺 {needle}"));
        // CATA 闭包解析仍在 Task（plan 2026-09-06 §3 D6 (a)：解析失败拦水位的口径保留），
        // 行本身随窗口产物一起落。
        let cata = at("cata_dependency::resolve_dependency_rows(");
        let invalidate = at("Self::invalidate_caches(&cache_refnos)");
        let reverse_index = at("build_reverse_index_statements(&range_eles)");
        let datacenter =
            at("Self::datacenter_statements(&range_eles, db_type, dbnum, &session_time_of)");
        let artifact = at("IncrementWindowArtifact::from_window(");
        let landed = at("land_increment_window(");
        let recheck = at("reopen_verified()");
        let finalize = at("increment_tail::finalize_attempt(");
        assert!(
            cata < invalidate
                && invalidate < reverse_index
                && reverse_index < datacenter
                && datacenter < artifact
                && artifact < landed
                && landed < recheck
                && recheck < finalize,
            "同步序列化顺序纪律：闭包 → 失效 → ref_rev 渲染 → 随窗渲染 → 窗口产物 → 落库 → \
             复核 → finalize"
        );
    }

    /// ADR-071 不变量 S3 / S5 的源码钉：数据窗口的结构行由 Task **自己同步**落库，落库口只有
    /// `land_increment_window` 一处、排在尾事务之前；Task 与库之间没有 journal / lane / ACK
    /// （`increment_local_runtime` / `store_backup_bundle` / `accept` / `complete` 一个都不许回来
    /// ——回来一个，就是把同步序列化偷换回 (a) 的异步账本）。老的逐段直写口
    /// （`persist_latest_main_data` / `maintain_reverse_index` …）也不许回来：它们的渲染器已在
    /// 落库入口里复用，再写一遍就是双写。尾事务的随窗语句形参保持为空——它们已随窗口产物落了。
    #[test]
    fn the_data_window_lands_its_structured_rows_synchronously_before_the_tail() {
        let body = apply_one_body();
        for forbidden in [
            "increment_local_runtime(",
            "store_backup_bundle(",
            "runtime.accept(",
            "runtime.complete(",
            "LocalWindow",
            "LocalCompletion",
            "observe_database_applied(",
            "persist_latest_main_data(",
            "persist_cata_dependencies(",
            "persist_resolved_rows",
            "maintain_reverse_index(",
            "terminal_persist_batches(",
            "execute_on(",
            "enqueue_ref_rev",
        ] {
            assert!(
                !body.contains(forbidden),
                "apply_one 不得再有异步账本 / 第二个结构行写点：{forbidden}"
            );
        }
        assert_eq!(
            body.matches("land_increment_window(").count(),
            1,
            "结构行落库口只有一处: {body}"
        );
        let landed = body.find("land_increment_window(").expect("落库口");
        let finalize = body
            .find("increment_tail::finalize_attempt(")
            .expect("尾事务还在");
        assert!(landed < finalize, "行先落、水位最后（ADR-071 S3）");
        let tail = body
            .split_once("increment_tail::finalize_attempt(")
            .expect("尾事务还在")
            .1
            .split_once(".await?")
            .expect("尾事务要 await")
            .0;
        assert!(
            tail.contains("&[],"),
            "尾事务不再携带随窗语句（它们已随窗口产物同步落库）: {tail}"
        );
    }

    /// 直写路径的持久化语句（主数据 + 反向索引）与水位是两回事：一条 Deleted 渲染出
    /// **恰好两条**语句（`pe` 软删 + `ref_rev` 清理），打在生产 schema 的一次性实例上之后
    /// `dbnum_watermark` 仍为空——水位只属于 `finalize_attempt` 的尾事务。
    /// （前身 `staged_parse_keeps_one_journal_and_does_not_finalize` 借 kv-mem 暂存窗口当
    /// 载体断言 journal 条数；ADR-056 P1 后 journal 没有对象，改成直起实例断言直写渲染。）
    #[tokio::test(flavor = "multi_thread")]
    async fn persist_statements_never_touch_the_watermark() {
        use crate::data_interface::table_parity::{apply_all, fresh_mem_db, init_schema_on};

        let db = fresh_mem_db("aios", "persist_no_watermark")
            .await
            .expect("mem boots");
        init_schema_on(&db).await.expect("production schema");
        db.query("UPSERT pe:⟨7996_10⟩ SET noun = 'PIPE', dbnum = 7996")
            .await
            .expect("seed pe row")
            .check()
            .expect("seeded");

        let mut range = BTreeMap::new();
        range.insert(
            1,
            vec![EleOperationData::new(
                RefU64((7996_u64 << 32) | 10),
                1,
                EleOperationDetail::Deleted,
            )],
        );
        let statements = IncrementPipeline::render_persist_statements(&range, 7996)
            .into_iter()
            .chain(crate::data_interface::manual_update::build_reverse_index_statements(&range))
            .collect::<Vec<_>>();
        assert_eq!(
            statements.len(),
            2,
            "主数据软删 + ref_rev 清理: {statements:?}"
        );
        apply_all(&db, &statements).await.expect("direct persist");

        let mut response = db
            .query("SELECT * FROM dbnum_watermark")
            .await
            .expect("query watermark");
        let rows: surrealdb::Value = response.take(0).expect("take watermark");
        assert_eq!(
            serde_json::to_string(&rows).expect("serialize"),
            "{\"Array\":[]}",
            "持久化阶段不得提前推进水位"
        );
        let mut response = db
            .query("RETURN pe:⟨7996_10⟩.deleted")
            .await
            .expect("query tombstone");
        assert_eq!(
            response.take::<Option<bool>>(0).expect("take deleted"),
            Some(true),
            "Deleted 渲染成软删 UPDATE，而不是硬删"
        );
    }

    /// 删除 / 修改目标的主数据语句都是 `UPDATE pe:…`：SurrealDB 2.x 的 `UPDATE` 命不中记录
    /// 就是空操作，所以持久层**缺行**时它们不会凭空造出半行 `pe`——直写路径上这一条同样
    /// 重要（老收集器幻删 / 漏增时 Modified 打在不存在的行上不得留下残迹，F8）。
    /// （前身 `the_window_cannot_see_the_ownership_of_deleted_or_modified_targets` 在暂存世界里
    /// 断言同一性质并推到「暂存态选根为 None」；暂存半边没有对象，只留下这一半。）
    #[tokio::test(flavor = "multi_thread")]
    async fn deleted_and_modified_targets_never_materialise_pe_rows() {
        use crate::data_interface::table_parity::{apply_all, fresh_mem_db, init_schema_on};

        let db = fresh_mem_db("aios", "no_pe_materialisation")
            .await
            .expect("mem boots");
        init_schema_on(&db).await.expect("production schema");

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

        let statements = IncrementPipeline::render_persist_statements(&range, 7995);
        for refno in [deleted, modified] {
            let pe = RefnoEnum::from(refno).to_pe_key();
            assert!(
                !statements
                    .iter()
                    .any(|sql| sql.contains(&format!("UPSERT {pe}"))
                        || sql.contains(&format!("CREATE {pe}"))),
                "删除 / 修改目标只能是 UPDATE，不得 UPSERT/CREATE 出 {pe}: {statements:?}"
            );
        }
        apply_all(&db, &statements)
            .await
            .expect("direct persist on an empty instance");

        let mut response = db
            .query("SELECT VALUE <string>record::id(id) FROM pe")
            .await
            .expect("list pe rows");
        let rows: Vec<String> = response.take(0).expect("take pe ids");
        assert!(rows.is_empty(), "持久层缺行时不得凭空长出 pe 行: {rows:?}");
    }

    /// ADR-003 B1-emit 的 `ref_rev` 维护在 lane 形态下的去向（plan 2026-09-09 §1 #5）：
    /// 渲染留 Task（`build_reverse_index_statements`，仍先问读透形态，ADR-063 D2），语句
    /// 原样进工件由 lane 执行；`side_effect_pending` 的 `enqueue_ref_rev` 补偿分支在这条路
    /// 上退役——lane 从同一份工件顺序重放，本来就是那条补偿要提供的保证。前身
    /// `reverse_index_failure_requires_durable_recovery_before_finalize` 钉的是补偿分支存在，
    /// 现在钉相反的一面。
    #[test]
    fn reverse_index_rides_the_window_artifact_without_a_compensation_queue() {
        let body = apply_one_body();
        let guarded = body
            .find("if crate::options::data_face().is_read_through() {")
            .expect("反向索引渲染必须先问形态（ADR-063 D2）");
        let rendered = body
            .find("build_reverse_index_statements(&range_eles)")
            .expect("渲染还在 Task");
        assert!(guarded < rendered, "读透形态必须在渲染之前就跳过");
        let artifact = body
            .split_once("IncrementWindowArtifact::from_window(")
            .expect("工件组装还在")
            .1
            .split_once(");")
            .expect("组装有结尾")
            .0;
        assert!(
            artifact.contains("reverse_index_statements,"),
            "渲染好的 ref_rev 语句必须原样装进工件: {artifact}"
        );
        assert!(
            !body.contains("enqueue_ref_rev") && !body.contains("side_effect_pending"),
            "补偿队列在 lane 形态下退役，不许回到 apply_one"
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

    // `prepared_attempt_rejects_a_file_that_no_longer_covers_fixed_range` 与
    // `prepared_attempt_accepts_equivalent_windows_path_spelling` 随 `validate_prepared_attempt`
    // 一起退役（ADR-072 D1）：崩溃后不再拿一份持久化的旧区间去对当前文件，而是回退到 S、
    // 按当前文件重解窗口。「文件被换回旧版本」那一档由冻结点的回退检出 + 整库重建接手。

    /// IU-S3-04：交入窗口只有与请求区间**完全一致**才被采信，其余一律回退
    /// 自行收集。掐头/去尾/整体错位的窗口混进来，折叠与影响判定就会漏看会话。
    #[test]
    fn handed_in_window_is_only_accepted_on_an_exact_range_match() {
        let collected = || CollectedWindow {
            frozen: None,
            range_eles: BTreeMap::from([(25u32, Vec::<EleOperationData>::new())]),
            persist_changes: Vec::new(),
            session_sesnos: vec![25, 26],
            warnings: vec!["收集口径：净窗口（Net）".to_string()],
            membership_deleted: 0,
            retired_type_rows: Vec::new(),
        };

        assert!(
            accept_handed_in_window(Some((25..=26, collected())), &(25..=26)).is_some(),
            "区间逐位相等必须复用（这正是双跑修复省下的那一遍解析）"
        );
        // 右端多一格 / 左端少一格 / 完全错开 / 反向，全部拒绝。
        assert!(accept_handed_in_window(Some((25..=27, collected())), &(25..=26)).is_none());
        assert!(accept_handed_in_window(Some((24..=26, collected())), &(25..=26)).is_none());
        assert!(accept_handed_in_window(Some((27..=30, collected())), &(25..=26)).is_none());
        #[allow(clippy::reversed_empty_ranges)]
        {
            assert!(accept_handed_in_window(Some((26..=25, collected())), &(25..=26)).is_none());
        }
        // 没交就是没交。
        assert!(accept_handed_in_window(None, &(25..=26)).is_none());
    }

    /// 空保存、自抵消和稀疏会话都由会话页映射决定，不能被操作 key 抹掉。
    #[test]
    fn session_manifest_comes_from_frozen_session_pages_not_operations() {
        let pages = BTreeMap::from([(1, 11), (3, 13), (6, 16), (9, 19)]);
        assert_eq!(session_sesnos_in_range(&pages, &(2..=8)), vec![3, 6]);

        let collected = CollectedWindow {
            frozen: None,
            // 会话 3 是空保存，会话 6 的操作在净窗口内自抵消：最终操作集为空。
            range_eles: BTreeMap::new(),
            persist_changes: Vec::new(),
            session_sesnos: session_sesnos_in_range(&pages, &(2..=8)),
            warnings: vec!["收集口径：净窗口（Net）".to_string()],
            membership_deleted: 0,
            retired_type_rows: Vec::new(),
        };
        assert!(collected.range_eles.is_empty());
        assert_eq!(collected.session_sesnos, vec![3, 6]);
        assert!(collected.warnings[0].contains("净窗口"));
    }

    /// 净口径标注必须携带全部容忍形状的计数。`unchanged_rewrites` / `unparseable_finals` /
    /// 类型改变只活在 `NetWindowOutcome` 里，而它在 `collect_window` 之后即被
    /// 拆散——从这条 warning 里删掉它们，批次回执上「跳过」就退回静默
    /// （spec-003 FR-8：任何一种容忍都不许静默；2026-08-18 审核 P2）。
    #[test]
    fn the_net_caliber_warning_carries_the_tolerated_shape_counts() {
        use crate::data_interface::e3d_collector::{NetWindowOutcome, RetiredTypeRow, WindowStats};

        let outcome = NetWindowOutcome {
            frozen: FrozenSource::synthetic("D:/proj/ams8000_0001", 8000, "DESI", 46),
            window: BTreeMap::new(),
            warnings: Vec::new(),
            unchanged_rewrites: 4,
            unparseable_finals: 3,
            ignored_finals: Vec::new(),
            retired_type_rows: vec![RetiredTypeRow {
                refno: RefU64::from_two_nums(24383, 10),
                old_noun: "BOX".into(),
                new_noun: "NOZZ".into(),
            }],
            tombstones: Default::default(),
            stats: WindowStats {
                base_sesno: Some(45),
                target_sesno: 46,
                candidates: 26,
                inserted: 22,
                deleted: 1,
                modified: 3,
                pages_read: 40,
                elapsed_ms: 12,
            },
        };

        let line = net_caliber_warning(7, 8, 9, &outcome);

        assert!(
            line.starts_with("收集口径：净窗口"),
            "第一条 warning 的口径前缀不许变（预览断言认它）: {line}"
        );
        for needle in [
            "added=7",
            "deleted=8",
            "modified=9",
            "成员补删=0",
            "幽灵新增丢弃=0",
            "原样重写跳过=4",
            "终稿不可解析=3",
            "类型改变=1",
            "索引候选=26（ins/mod/del=22/3/1）",
            "基会话=45",
            "读页=40",
            "差分 12ms",
        ] {
            assert!(line.contains(needle), "{needle} 不在口径标注里: {line}");
        }
    }

    #[test]
    #[ignore = "manual live: needs AvevaMarineSample ams8000_0001"]
    fn live_ams8000_ses236_membership_delete_matches_expected_net() {
        let path =
            std::path::Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001");
        let collected = IncrementPipeline::collect_window(path, 236..=236)
            .expect("collect exact live window 236");
        let (mut added, mut modified, mut deleted) = (0, 0, 0);
        for operation in collected.range_eles.values().flatten() {
            match operation.detail {
                EleOperationDetail::Add(_) => added += 1,
                EleOperationDetail::Modified(_) => modified += 1,
                EleOperationDetail::Deleted => deleted += 1,
                EleOperationDetail::None => {}
            }
        }
        assert_eq!((added, modified, deleted), (0, 1, 1));
        // e3d-io 底座没有成员仲裁补删：那条删除必须由索引差分本身给出。
        assert_eq!(collected.membership_deleted, 0);
        assert!(collected.range_eles.values().flatten().any(|operation| {
            operation.refno == RefU64::from_two_nums(24384, 26201)
                && matches!(operation.detail, EleOperationDetail::Deleted)
        }));
    }

    /// 收集入口不许再长出第二个口径（ADR-031）：`collect_window` 只走净窗口——
    /// 没有开关、没有分支、不碰 legacy 回放。会话页清单仍与操作流共用**同一个**
    /// 冻结源：双开会允许同路径在两次读取之间被替换，拼出 A 文件的会话
    /// 清单 + B 文件的操作流。
    #[test]
    fn the_collector_has_no_caliber_branch() {
        let source = include_str!("increment_pipeline.rs");
        let body = source
            .split_once("pub(crate) fn collect_window_for_candidate(")
            .expect("collect_window_for_candidate")
            .1
            .split_once("pub async fn apply(")
            .expect("apply follows collector")
            .0;

        assert!(
            body.contains("e3d_collector::collect_net_window(") && !body.contains("pdms_io::"),
            "唯一口径必须是 e3d-io 净窗口收集器: {body}"
        );
        assert!(
            body.contains("&frozen.session_chain()?"),
            "会话页清单必须来自同一个冻结源: {body}"
        );
        assert!(
            body.contains("token.reopen_verified_at(target)")
                && body.contains("FrozenSource::capture_at(path, target)"),
            "候选冻结 token 与普通预览都必须汇入同一个冻结源收集器: {body}"
        );
        assert_eq!(
            body.matches("FrozenSource::capture(").count(),
            0,
            "收集入口必须选择 capture_at/reopen_verified_at，不能绕回无目标冻结的 capture: {body}"
        );
        // 三个曾经的口径符号：开关、legacy 收集器、口径枚举。任一回到这个函数体，
        // 「同谓词」就重新退回约定而不是结构。
        for banned in ["net_window_collection", "collect_changes", "CollectionMode"] {
            assert!(
                !body.contains(banned),
                "`{banned}` 回到收集入口就是第二个口径决定点（ADR-031）: {body}"
            );
        }
        assert!(!source.contains(concat!("fn collect_session", "_sesnos(")));
    }

    /// 收集口径是故障回执的一部分：fresh 收集后要先保存 warning 再做模型计划，
    /// apply_with_precollected 也必须在匹配成功/失败之前接住 apply_one 的旁路。
    #[test]
    fn the_caliber_warning_survives_a_later_apply_error() {
        let source = include_str!("increment_pipeline.rs");
        let apply = source
            .split_once("pub async fn apply_with_precollected(")
            .expect("apply_with_precollected")
            .1
            .split_once("async fn apply_one(")
            .expect("apply_one follows")
            .0;
        let call = apply.find("let (outcome, warnings)").expect("双通道返回");
        let forward = apply
            .find("result.warnings.extend(warnings)")
            .expect("先转交 warnings");
        let branch = apply.find("match outcome").expect("再分成功/失败");
        assert!(call < forward && forward < branch, "{apply}");

        let one = source
            .split_once("async fn apply_one(")
            .expect("apply_one")
            .1
            .split_once("async fn invalidate_caches(")
            .expect("invalidate follows")
            .0;
        let collect_warning = one
            .find("warnings.extend(collected.warnings.iter().cloned())")
            .expect("收集后立即保存口径");
        let plan = one
            .find("build_effective_model_update_plan(")
            .expect("模型计划可能失败");
        assert!(
            collect_warning < plan,
            "口径 warning 必须先于计划 I/O: {one}"
        );
    }

    /// IU-S3-03 的接班人（ADR-072 D1）：`apply_one` 里**只有一条**窗口来源。
    ///
    /// 从前这里有两条：见到未收口的 `increment_update_attempt` 就复用它的固定区间与
    /// 持久化计划「原样重放」，否则才走交入 / 自行收集。那条分支连同恢复记录一起退役了
    /// ——崩溃由写入标记回退（`write_state::rollback_to_watermark`）+ 从头重算 S→T 接手，
    /// 半应用的库根本不会被当成规划输入。钉的是「第二条来源不许回来」。
    #[test]
    fn the_window_has_exactly_one_source_and_no_replay_branch() {
        let source = include_str!("increment_pipeline.rs");
        let body = source
            .split_once(concat!("async fn ", "apply_one("))
            .expect("apply_one must exist")
            .1
            .split_once(concat!("async fn ", "invalidate_caches("))
            .expect("invalidate_caches follows apply_one in this file")
            .0;

        for retired in [
            "load_attempt(",
            "prepare_attempt(",
            "validate_prepared_attempt(",
            "attempt.plan",
            "recovery_frozen",
        ] {
            assert!(
                !body.contains(retired),
                "恢复记录那条重放分支已退（ADR-072 D1），不许回来: {retired}\n{body}"
            );
        }
        // 采信点恰好一处——多出第二处就说明有人绕开了统一判定。
        assert_eq!(
            body.matches(concat!("accept_handed_in_window", "("))
                .count(),
            1,
            "采信判定必须只有一个入口"
        );
        // 窗口收集也只有这一处：交入落空才自行收集。
        assert_eq!(
            body.matches("Self::collect_window(path, requested_range.clone())")
                .count(),
            1,
            "窗口收集只有一个回退口: {body}"
        );
    }

    /// IU-S8-05 / IU-S12-02（L1 源码钉）：缓存失效要跟着**结构行真正改变的那一刻**走。
    /// 前身 `cache_invalidation_survives_a_partially_failed_persist` 钉的是「直写部分失败也要
    /// 失效」。现在结构行由 Task 同步落库（`land_increment_window`），两处都要失效：Task 在装
    /// 窗口产物之前失效一次（名单同时装进产物），落库入口在结构行全部落定之后按同一份名单再失效
    /// 一次——落库中途失败时前一次失效已经生效，不留陈旧缓存。任一处退掉即红。
    #[test]
    fn cache_invalidation_runs_before_the_landing_and_again_after_the_rows_land() {
        let body = apply_one_body();
        let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("缺 {needle}"));
        let invalidate = at("Self::invalidate_caches(&cache_refnos)");
        let listed = at("cache_refnos.iter().copied(),");
        let landed = at("land_increment_window(");
        assert!(
            invalidate < listed && listed < landed,
            "Task 侧：失效 → 名单进窗口产物 → 才落库: {body}"
        );

        // 那个文件是 CRLF，别拿 "\n}\n" 当函数结尾切；测试模块头是稳的。
        let landing = include_str!("window_landing/mod.rs")
            .split_once("pub async fn land_increment_window(")
            .expect("落库入口还在")
            .1
            .split_once("#[cfg(test)]")
            .expect("落库入口之后是测试模块")
            .0;
        let last_write = landing
            .find("report.window_statements = window.window_statements.len();")
            .expect("随窗语句是最后一步结构写");
        let landing_invalidate = landing
            .find("IncrementPipeline::invalidate_caches(&window.cache_refnos())")
            .expect("落完必须按产物名单再失效一次");
        assert!(
            last_write < landing_invalidate,
            "落库侧：结构行全部落定之后才失效: {landing}"
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

    /// T18 记录项（ADR-031，非门）：release 下按协议计时完整收集。
    /// 1 warmup + 5 次，报 median / min / p95；冷启动另报；两类窗口。
    /// 计时对象刻意是生产入口 `collect_window`（含打开文件），不是内层合成器。
    /// 2026-09-04 起只剩这一臂：old-pdms-io 回放的对照计时随 `collect_changes` 一起删了。
    #[test]
    #[ignore = "manual live: release timing for the single-caliber collector"]
    fn live_ams8000_single_caliber_release_timing() {
        use std::path::PathBuf;

        let path = std::env::var_os("AIOS_AMS8000_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let testbed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("python/testbed/projects/AvevaMarineSample/ams000/ams8000_0001");
                if testbed.is_file() {
                    testbed
                } else {
                    PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001")
                }
            });
        assert!(path.is_file(), "找不到计时文件: {}", path.display());

        let latest = DbFileHeader::read(&path)
            .expect("read header")
            .latest_sesno() as i32;
        let windows = [
            ("high-retouch", (latest / 2).max(1)..=latest),
            ("add-floor", 1..=latest),
        ];

        fn stats(samples: &[u128]) -> (u128, u128, u128) {
            let mut sorted = samples.to_vec();
            sorted.sort_unstable();
            let n = sorted.len();
            let median = sorted[n / 2];
            let min = sorted[0];
            let p95_idx = ((n as f64) * 0.95).ceil() as usize - 1;
            (median, min, sorted[p95_idx.min(n - 1)])
        }

        println!(
            "[T18] file={} sha-skipped latest={latest} cfg={}",
            path.display(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        for (name, window) in windows {
            let mut net_samples = Vec::new();
            let cold_net = Instant::now();
            let net0 = IncrementPipeline::collect_window(&path, window.clone()).expect("net cold");
            let cold_net_ms = cold_net.elapsed().as_millis();

            for _ in 0..5 {
                let started = Instant::now();
                let _ = IncrementPipeline::collect_window(&path, window.clone()).expect("net warm");
                net_samples.push(started.elapsed().as_millis());
            }

            let net_ops = net0.range_eles.values().flatten().count();
            let (n_med, n_min, n_p95) = stats(&net_samples);
            println!(
                "[T18] {name} {window:?} sessions~{} net_ops={net_ops} cold_net={cold_net_ms}ms \
                 net median/min/p95={n_med}/{n_min}/{n_p95}ms",
                *window.end() - *window.start() + 1
            );
        }
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

    #[test]
    fn net_window_rejects_duplicate_terminal_operations() {
        let window = BTreeMap::from([(1, vec![op(1, 1, blank())]), (2, vec![op(1, 2, blank())])]);
        let error = ensure_unique_terminal_operations(&window).expect_err("duplicate must block");
        assert!(error.to_string().contains("净窗口终态重复"));
    }

    fn window(ops: Vec<EleOperationData>) -> BTreeMap<u32, Vec<EleOperationData>> {
        let mut map: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
        for op in ops {
            map.entry(op.sesno).or_default().push(op);
        }
        map
    }

    #[test]
    fn added_refno_clears_every_previous_owner_edge_before_upsert() {
        let added = refno(30);
        let owner = refno(20);
        let mut element = crate::data_interface::ele_data::EleData {
            refno: added,
            owner,
            ..Default::default()
        };
        element
            .whole_attmap
            .attmap
            .insert("TYPE".into(), text("FTUB"));
        element
            .whole_attmap
            .attmap
            .insert("DBNUM".into(), NamedAttrValue::IntegerType(7997));

        let statements = IncrementPipeline::render_persist_statements(
            &window(vec![EleOperationData::new(
                added,
                47,
                EleOperationDetail::Add(element),
            )]),
            7997,
        );
        let cleanup = "DELETE pe:7997_30->pe_owner;";
        assert_eq!(statements.first().map(String::as_str), Some(cleanup));
        let children_cleanup = "DELETE pe_owner:[pe:7997_30, NONE]..=[pe:7997_30, ..];";
        assert_eq!(
            statements.get(1).map(String::as_str),
            Some(children_cleanup)
        );
        let sql = statements.join("\n");
        assert_eq!(
            sql.matches(cleanup).count(),
            1,
            "one Add needs exactly one replay-safe incoming-edge cleanup: {sql}"
        );
        assert!(
            sql.find(cleanup) < sql.find(children_cleanup)
                && sql.find(children_cleanup) < sql.find("UPSERT pe:7997_30"),
            "the old generation's owner and child edges must be gone before the current PE is installed: {sql}"
        );
    }

    /// 尾事务拿到的是**完整**的窗口前计划（含 RegenRoot）：模型阶段在水位之后跑
    /// （ADR-025 §7 / ADR-056 D1），在这里剥掉根就等于推进了数据水位却永久丢掉了
    /// 唯一一份「请生成它」的持久请求。
    #[test]
    fn the_finalize_tail_receives_the_full_pre_persist_plan() {
        let source = include_str!("increment_pipeline.rs");
        let block = source
            .split_once("// The tail receives the complete durable plan")
            .expect("finalize tail must document the durable plan")
            .1
            .split_once("timings.report(")
            .expect("timing report must follow the finalize tail")
            .0;
        assert!(
            block.contains("increment_tail::finalize_attempt(") && block.contains("&model_plan,"),
            "the full pre-persist plan must enter the finalize tail: {block}"
        );
        assert!(
            !block.contains("ModelWorkAction::RegenRoot") && !block.contains("work_items.retain"),
            "RegenRoot may only be removed after successful generation, never at finalize: {block}"
        );
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

    /// 冻结源复核是 commit-generation gate，不是模型侧优化：它必须**无条件**跑在
    /// `apply_one` 里，纯数据窗口（零模型候选）与窗口规划器的计划都不例外。
    ///
    /// T412 把「计划目标 T 端存活复核」摘到 `window_planned` 后面之后，这条钉子改盯
    /// 顺序：`verify_frozen_source` 要出现在第一处 `window_planned` 分支**之前**，
    /// 否则身份门就跟着规划器一起被跳过了。
    #[test]
    fn every_window_verifies_the_frozen_source_before_any_planner_branch() {
        let source = include_str!("increment_pipeline.rs");
        let one = source
            .split_once("    async fn apply_one(")
            .expect("apply_one")
            .1
            .split_once("\n    async fn ")
            .expect("apply_one boundary")
            .0;
        let verify = one
            .find("verify_frozen_source(&frozen, end_sesno)")
            .expect("stable generation gate");
        let branch = one
            .find("model_plan.window_planned")
            .expect("planner branch");
        assert!(
            verify < branch,
            "冻结源复核必须先于任何规划器分支，纯数据窗口也要验: {one}"
        );

        // 门本体仍在被保留的那一半里，没有随存活复核一起被摘掉。
        let gate = source
            .split_once("fn verify_frozen_source(")
            .expect("verify_frozen_source")
            .1
            .split_once("\nfn ")
            .expect("verify_frozen_source boundary")
            .0;
        assert!(
            gate.contains(".reopen_verified()")
                && gate.contains("target_sesno() == end_sesno as u32"),
            "身份门要同时验文件世代与 target 会话: {gate}"
        );
    }

    /// T412：窗口规划器的计划不得再走 old-pdms-io 的 `contains_refnos_at`。
    ///
    /// 它的根来自 `enumerate_generation_roots(DbSet@T)`，自身即 T 端存活证明；老读法
    /// 在 ams7999 45→46 这类窗口上会对新建子树报 `IndexPageData.noun == 0xCC47DF`，
    /// 把整窗打成 Failed 并让水位永久卡死。
    #[test]
    fn window_planned_plans_skip_the_legacy_liveness_recheck() {
        let source = include_str!("increment_pipeline.rs");
        let one = source
            .split_once("    async fn apply_one(")
            .expect("apply_one")
            .1
            .split_once("\n    async fn ")
            .expect("apply_one boundary")
            .0;
        let branch = one
            .find("if model_plan.window_planned")
            .expect("存活复核必须挂在 window_planned 后面");
        let call = one
            .find("reconcile_plan_final_presence(&verified")
            .expect("存活复核调用点");
        assert!(
            branch < call,
            "`reconcile_plan_final_presence` 只能在老规划器分支里调: {one}"
        );
        assert_eq!(
            one.matches("reconcile_plan_final_presence(").count(),
            1,
            "存活复核不得有第二个调用点绕过 window_planned: {one}"
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
}

#[cfg(test)]
mod datacenter_tests {
    use super::*;

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

    type ChainMap = std::collections::HashMap<RefU64, PeChainView>;

    fn refu(n: u64) -> RefU64 {
        RefU64((7997u64 << 32) | n)
    }

    /// 窗口前态一行：`(noun, owner)`，没名字。
    fn row(noun: &str, owner: Option<RefU64>) -> PeChainView {
        PeChainView::new(Some(noun), owner)
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
        let published_refnos = ops.iter().map(|operation| operation.refno).collect();
        render_with_published(ops, chain, published_refnos).await
    }

    async fn render_with_published(
        ops: Vec<EleOperationData>,
        chain: ChainMap,
        published_refnos: HashSet<RefU64>,
    ) -> Vec<String> {
        let unit_types = crate::data_interface::generation_root::resolve_delivery_unit_types(&[]);
        IncrementPipeline::resolve_datacenter_statements_with(
            &BTreeMap::from([(1u32, ops)]),
            &unit_types,
            7997,
            |_| None,
            &published_refnos,
            chain_loader(chain),
        )
        .await
        .expect("resolve datacenter statements")
    }

    /// 走链兜底用的窗口前持久态：1 → 6(PIPE) → 5(ZONE) → 4(SITE)，旁支 2 → 5。
    fn pre_window_chain() -> ChainMap {
        ChainMap::from([
            (refu(1), row("FTUB", Some(refu(6)))),
            (refu(2), row("DAMP", Some(refu(5)))),
            (refu(6), row("PIPE", Some(refu(5)))),
            (refu(5), row("ZONE", Some(refu(4)))),
            (refu(4), row("SITE", None)),
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

        // Delete 逐条（各带 belong_zone），两个 Modify 目标归成一条多目标 UPDATE（P3）。
        assert_eq!(statements.len(), 2, "{statements:?}");
        for statement in &statements {
            assert!(statement.ends_with(';'), "{statement}");
        }
        let modify = statements
            .iter()
            .find(|s| s.contains("'Modify'"))
            .expect("Modify 归组语句");
        assert!(
            modify.starts_with(
                "update datacenter_version:7997_1, datacenter_version:7997_5 set status = 'Modify';"
            ) || modify.starts_with(
                "update datacenter_version:7997_5, datacenter_version:7997_1 set status = 'Modify';"
            ),
            "{modify}"
        );
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
        chain.insert(refu(7), row("ZONE", Some(refu(4))));

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
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
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
        chain.insert(refu(8), row("SITE", None));
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

        // 一条 Delete + 一条归组的 Modify（FTUB 上溯到 ZONE 7997_5、BRAN 7997_10）；SITE 自身修改跳过。
        assert_eq!(
            statements.len(),
            2,
            "SITE 自身修改解不出单元层归属，必须跳过而不是渲出 NONE 目标: {statements:?}"
        );
        let modify = statements
            .iter()
            .find(|s| s.contains("'Modify'"))
            .expect("Modify 归组语句");
        assert!(
            modify.contains("datacenter_version:7997_5")
                && modify.contains("datacenter_version:7997_10"),
            "{modify}"
        );
        assert!(
            !modify.contains("7997_8"),
            "SITE 自身不得进目标名单: {modify}"
        );
        for statement in &statements {
            assert!(
                !statement.contains("fn::") && !statement.contains("$pe"),
                "收口语句必须是固定目标 id 的纯 UPDATE（D5 回退即红）: {statement}"
            );
        }
    }

    // ---- 交付删除记录（plan 2026-09-22-datacenter-delete-records-dev-plan §3.1）----

    fn text(s: &str) -> aios_core::NamedAttrValue {
        aios_core::NamedAttrValue::StringType(s.to_string())
    }

    /// 交付单元 `refno` 自身改位号 `old → new`（显式属性那一格，与 fold 之后的形状一致），
    /// 窗口净态的 owner 为 `owner`。
    fn renamed(refno: RefU64, noun: &str, owner: RefU64, old: &str, new: &str) -> EleOperationData {
        let mut op = modified(noun);
        op.refno = refno;
        op.sesno = 12;
        if let EleOperationDetail::Modified(m) = &mut op.detail {
            m.current_data.owner = owner;
            m.current_data.name = new.to_string();
            m.modified_explicit_attrs
                .insert("NAME".into(), (text(old), text(new)));
        }
        op
    }

    fn delete_rows(statements: &[String]) -> Vec<&String> {
        statements
            .iter()
            .filter(|s| s.contains("datacenter_delete:"))
            .collect()
    }

    /// 改位号：一条 `INSERT IGNORE`，ID 只取 refno 值，原有 Modify 照旧；语句过
    /// `replay_safe` 白名单（显式 id、无随机 / 时间函数）。
    #[tokio::test(flavor = "multi_thread")]
    async fn renaming_a_delivery_unit_records_the_old_name_for_deletion() {
        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        let statements = render_with(
            vec![renamed(refu(3), "BRAN", refu(6), "/A-OLD", "/A-NEW")],
            chain,
        )
        .await;

        assert_eq!(statements.len(), 2, "{statements:?}");
        let delete = delete_rows(&statements).remove(0);
        assert_eq!(
            delete,
            "INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:7997_3, refno: pe:7997_3, \
             name: '/A-OLD', type: 'BRAN', zone_refno: pe:7997_5, dbnum: 7997, sesno: 12, \
             sesno_time: NONE, reason: 'renamed' };"
        );
        crate::data_interface::staging::replay_safe::validate_statement(delete)
            .expect("交付删除记录带显式 id，必须过重放安全白名单");
        assert!(
            statements
                .iter()
                .any(|s| s == "update datacenter_version:7997_3 set status = 'Modify';"),
            "改位号的单元照旧标 Modify，新数据按正常提资走: {statements:?}"
        );
    }

    /// 最新 issue #1：只有 `datacenter_version:{refno}` 已存在的单元才进事件表。未发布单元的
    /// Modify / Delete 状态语句沿用既有逻辑，但不得生成 `datacenter_delete` 写入。
    #[tokio::test(flavor = "multi_thread")]
    async fn unpublished_delivery_units_do_not_record_delete_events() {
        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        let statements = render_with_published(
            vec![renamed(refu(3), "BRAN", refu(6), "/A-OLD", "/A-NEW")],
            chain,
            HashSet::new(),
        )
        .await;

        assert!(delete_rows(&statements).is_empty(), "{statements:?}");
        assert_eq!(
            statements,
            vec!["update datacenter_version:7997_3 set status = 'Modify';".to_string()]
        );
    }

    /// 发布存在性门按与事件表相同的 refno ID 点查；表尚不存在时为空，存在时只回命中的行。
    #[tokio::test(flavor = "multi_thread")]
    async fn published_event_gate_reads_datacenter_version_by_refno_id() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("datacenter_delete_gate")
            .use_db("refno_ids")
            .await
            .expect("use db");
        let candidates = vec![refu(3), refu(4), refu(21)];
        assert!(
            IncrementPipeline::load_published_datacenter_refnos_on(&db, candidates.clone())
                .await
                .expect("missing table is empty")
                .is_empty()
        );

        db.query(
            "INSERT IGNORE INTO datacenter_version [\
             { id: datacenter_version:7997_3 },\
             { id: datacenter_version:7997_21 }];",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seed versions");
        assert_eq!(
            IncrementPipeline::load_published_datacenter_refnos_on(&db, candidates)
                .await
                .expect("load published"),
            HashSet::from([refu(3), refu(21)])
        );
    }

    /// 下属管件改名、`NAME` 从无到有、位号没变（只是 `NAME` 出现在别的改动里）：都不记。
    #[tokio::test(flavor = "multi_thread")]
    async fn component_renames_and_first_names_do_not_record_deletions() {
        // ELBO(11) 在 BRAN(3) 之下改名：只让 BRAN 记 Modify。
        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        chain.insert(refu(11), row("ELBO", Some(refu(3))));
        let component = render_with(
            vec![renamed(refu(11), "ELBO", refu(3), "/E-OLD", "/E-NEW")],
            chain.clone(),
        )
        .await;
        assert!(delete_rows(&component).is_empty(), "{component:?}");
        assert_eq!(
            component,
            vec!["update datacenter_version:7997_3 set status = 'Modify';".to_string()]
        );

        // NAME 本窗才出现（added_explicit_attrs）：以前没名字，没有改前位号。
        let mut first_name = modified("BRAN");
        first_name.refno = refu(3);
        if let EleOperationDetail::Modified(m) = &mut first_name.detail {
            m.current_data.owner = refu(6);
            m.added_explicit_attrs.insert("NAME".into(), text("/A-NEW"));
        }
        let first = render_with(vec![first_name], chain.clone()).await;
        assert!(delete_rows(&first).is_empty(), "{first:?}");

        // (old, new) 相同（别处写回同值）：不记。
        let same = render_with(vec![renamed(refu(3), "BRAN", refu(6), "/A", "/A")], chain).await;
        assert!(delete_rows(&same).is_empty(), "{same:?}");
    }

    /// 删除：位号与 noun 来自窗口前态（墓碑），`zone_refno` 沿 owner 链上溯到最近 ZONE——
    /// SUPPO → STRU → ZONE 隔两层也解得到（D5 与 `belong_zone` 那套「其余取 owner」的差别）；
    /// 原有 `status = 'Delete', belong_zone = …` 一字不改。
    #[tokio::test(flavor = "multi_thread")]
    async fn deleting_a_delivery_unit_records_its_name_and_nearest_zone() {
        let mut chain = pre_window_chain();
        chain.insert(refu(20), row("STRU", Some(refu(5))));
        chain.insert(
            refu(21),
            PeChainView::named(Some("SUPPO"), Some(refu(20)), "/SUPP-001"),
        );
        let mut op = EleOperationData::new(refu(21), 1, EleOperationDetail::Deleted);
        op.sesno = 30;
        let statements = render_with(vec![op], chain).await;

        assert_eq!(statements.len(), 2, "{statements:?}");
        assert_eq!(
            statements[0],
            "update datacenter_version:7997_21 set status = 'Delete', belong_zone = pe:7997_20;",
            "belong_zone 那半句保持老口径（D9）"
        );
        assert_eq!(
            statements[1],
            "INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:7997_21, refno: pe:7997_21, \
             name: '/SUPP-001', type: 'SUPPO', zone_refno: pe:7997_5, dbnum: 7997, sesno: 30, \
             sesno_time: NONE, reason: 'deleted' };"
        );
        crate::data_interface::staging::replay_safe::validate_statement(&statements[1])
            .expect("带显式 id 的 INSERT IGNORE 过白名单");
    }

    /// 删的不是交付单元（DAMP）、或单元没名字：只有原来的 Delete 那一条。
    #[tokio::test(flavor = "multi_thread")]
    async fn deleting_components_or_unnamed_units_records_nothing_extra() {
        let component = render_with(
            vec![EleOperationData::new(
                refu(2),
                1,
                EleOperationDetail::Deleted,
            )],
            pre_window_chain(),
        )
        .await;
        assert_eq!(component.len(), 1, "{component:?}");
        assert!(delete_rows(&component).is_empty());

        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        let unnamed = render_with(
            vec![EleOperationData::new(
                refu(3),
                1,
                EleOperationDetail::Deleted,
            )],
            chain,
        )
        .await;
        assert_eq!(unnamed.len(), 1, "{unnamed:?}");
        assert!(delete_rows(&unnamed).is_empty());
    }

    /// 链断（owner 行不在）：`zone_refno: NONE`，语句仍出；位号里的 `'` / `\` 转义后仍过白名单。
    #[tokio::test(flavor = "multi_thread")]
    async fn orphan_units_record_none_zone_and_names_are_escaped() {
        let statements = render_with(
            vec![renamed(refu(9), "EQUI", refu(99), "/E'Q\\1", "/E-2")],
            ChainMap::new(),
        )
        .await;
        let delete = delete_rows(&statements).remove(0);
        assert!(
            delete.contains("zone_refno: NONE") && delete.contains("'/E\\'Q\\\\1'"),
            "{delete}"
        );
        crate::data_interface::staging::replay_safe::validate_statement(delete)
            .expect("转义后的位号仍是带显式 id 的 INSERT IGNORE");
    }

    /// 同一批语句渲两遍逐字相同（整窗重放收敛）；`sesno_time` 是记录那一版（`renamed` 的 12）的时刻。
    #[tokio::test(flavor = "multi_thread")]
    async fn delete_records_render_deterministically_and_carry_the_session_time() {
        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        let ops = || vec![renamed(refu(3), "BRAN", refu(6), "/A-OLD", "/A-NEW")];
        async fn render(ops: Vec<EleOperationData>, chain: ChainMap) -> Vec<String> {
            let unit_types =
                crate::data_interface::generation_root::resolve_delivery_unit_types(&[]);
            IncrementPipeline::resolve_datacenter_statements_with(
                &BTreeMap::from([(1u32, ops)]),
                &unit_types,
                7997,
                |sesno| (sesno == 12).then(|| "2026-09-22T10:00:00+08:00".to_string()),
                &HashSet::from([refu(3)]),
                chain_loader(chain),
            )
            .await
            .expect("resolve")
        }
        let first = render(ops(), chain.clone()).await;
        let second = render(ops(), chain).await;
        assert_eq!(first, second);
        assert!(
            delete_rows(&first)[0].contains("sesno_time: '2026-09-22T10:00:00+08:00'"),
            "{first:?}"
        );
    }

    /// 一窗跨几个会话时，每条记录的 `sesno_time` 跟着它自己的 `sesno` 走，不是窗口末端那一版；
    /// 读不到时刻的那一版落 NONE；同一个会话只问一次。
    #[tokio::test(flavor = "multi_thread")]
    async fn delete_records_carry_the_time_of_their_own_session() {
        let mut chain = pre_window_chain();
        let mut in_session = |refno: u64, old: &str, sesno: u32| {
            chain.insert(refu(refno), row("BRAN", Some(refu(6))));
            let mut op = renamed(refu(refno), "BRAN", refu(6), old, "/NEW");
            op.sesno = sesno;
            op
        };
        let window = BTreeMap::from([
            (12u32, vec![in_session(3, "/A-OLD", 12)]),
            (
                13,
                vec![in_session(7, "/B-OLD", 13), in_session(8, "/C-OLD", 13)],
            ),
            (14, vec![in_session(10, "/D-OLD", 14)]),
        ]);
        let asked = std::sync::Mutex::new(Vec::new());
        let statements = IncrementPipeline::resolve_datacenter_statements_with(
            &window,
            &crate::data_interface::generation_root::resolve_delivery_unit_types(&[]),
            7997,
            |sesno| {
                asked.lock().unwrap().push(sesno);
                (sesno != 14).then(|| format!("2026-09-22T10:00:{sesno:02}+08:00"))
            },
            &HashSet::from([refu(3), refu(7), refu(8), refu(10)]),
            chain_loader(chain),
        )
        .await
        .expect("resolve");
        let rows = delete_rows(&statements);
        let row_of = |old: &str| {
            rows.iter()
                .find(|row| row.contains(&format!("name: '{old}'")))
                .unwrap_or_else(|| panic!("no delete record for {old}: {rows:?}"))
                .to_string()
        };
        for (old, tail) in [
            (
                "/A-OLD",
                "sesno: 12, sesno_time: '2026-09-22T10:00:12+08:00'",
            ),
            (
                "/B-OLD",
                "sesno: 13, sesno_time: '2026-09-22T10:00:13+08:00'",
            ),
            (
                "/C-OLD",
                "sesno: 13, sesno_time: '2026-09-22T10:00:13+08:00'",
            ),
            ("/D-OLD", "sesno: 14, sesno_time: NONE"),
        ] {
            let row = row_of(old);
            assert!(row.contains(tail), "{row}");
        }
        assert_eq!(*asked.lock().unwrap(), vec![12, 13, 14], "一个会话只问一次");
    }

    /// 预览回执的计数与渲染同口径（T4）：单元改名一条 + 单元被删一条 + 下属管件改名零条 = 2；
    /// `datacenter_version` 的 Modify / Delete 状态语句不算进来。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_preview_count_matches_the_rendered_delete_records() {
        let mut chain = pre_window_chain();
        chain.insert(refu(3), row("BRAN", Some(refu(6))));
        chain.insert(refu(11), row("ELBO", Some(refu(3))));
        chain.insert(refu(20), row("STRU", Some(refu(5))));
        chain.insert(
            refu(21),
            PeChainView::named(Some("SUPPO"), Some(refu(20)), "/SUPP-001"),
        );
        let statements = render_with(
            vec![
                renamed(refu(3), "BRAN", refu(6), "/A-OLD", "/A-NEW"),
                renamed(refu(11), "ELBO", refu(3), "/E-OLD", "/E-NEW"),
                EleOperationData::new(refu(21), 1, EleOperationDetail::Deleted),
            ],
            chain,
        )
        .await;
        assert_eq!(
            count_datacenter_delete_statements(&statements),
            2,
            "{statements:?}"
        );
        assert_eq!(
            count_datacenter_delete_statements(&statements),
            delete_rows(&statements).len()
        );
        assert!(
            statements.len() > 2,
            "datacenter_version 的状态语句照旧在同一批里: {statements:?}"
        );
        assert_eq!(count_datacenter_delete_statements(&[]), 0);
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
            7997,
            |_| None,
            &HashSet::from([refu(1), refu(3)]),
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
                        #[serde(default)]
                        name: Option<String>,
                    }
                    let keys = refnos
                        .iter()
                        .map(|refno| refno.to_pe_key())
                        .collect::<Vec<_>>()
                        .join(",");
                    let mut response = db
                        .query(format!(
                            "SELECT id, noun, owner, name FROM [{keys}] WHERE record::exists(id);"
                        ))
                        .await?
                        .check()?;
                    let rows: Vec<Row> = response.take(0)?;
                    Ok(rows
                        .into_iter()
                        .map(|row| {
                            (
                                row.id.refno(),
                                PeChainView {
                                    noun: row.noun,
                                    owner: row.owner.map(|o| o.refno()),
                                    name: row.name,
                                },
                            )
                        })
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
        crate::test_support::init_test_db()
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
        use crate::data_interface::ele_data::EleData;
        use aios_core::NamedAttrValue;

        crate::test_support::init_test_db()
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
            .query("SELECT VALUE id FROM pe:4000000003_10<-pe_owner;")
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
}
