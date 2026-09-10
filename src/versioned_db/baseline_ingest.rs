//! 基线摄入：**一个库文件** → 当前 SurrealDB（`Fresh` 整库写）。
//!
//! 出处：`docs/plans/2026-09-07-retire-sync-total-async-threaded-plan.md`（承接 2026-09-04 退役清单
//! §3.2 路 a / B8）。这里是 legacy `sync_total_async_threaded` 壳退役后基线唯一的编排层，
//! 它自己**不含任何数据面能力**——五个部件全是与增量路共用的那一份：
//!
//! | 部件 | 出处 |
//! |---|---|
//! | 收集 | [`e3d_collector::stream_baseline`]（与 `collect_net_window` 同一套枚举 / 读取 / 失败闸） |
//! | 渲染 | [`baseline::render_baseline_batches`] → `element_batch`（`Fresh`，事务不带统计观测） |
//! | 执行 | [`pe_batch::execute_on`] → `surreal_retry`（乐观冲突指数退避重试） |
//! | CATA 依赖 | [`cata_dependency`]（seed 流式攒、文件收尾一次闭包、同一条写者队列） |
//! | 统计 | **不在这里**：`dbnum_info_table` 由 dbnum 任务在数据批次应用之后从目标索引序列化（`ref0_snapshot` → `task_end`，ADR-071 D6），基线与增量同一笔 |
//!
//! 形状：`spawn_blocking` 里流式收集 → 每块渲成若干笔事务（每条队列消息 = 一笔事务 SQL）→
//! 有界队列（[`QUEUE_CAPACITY`]）→ **单写者**（SurrealDB 2.1 乐观事务下多写者在共享表 / 索引键上
//! 持续互撞，7997 真库曾耗尽重试预算）。解析与写入重叠：5525 实测写者才是瓶颈（排空 7.4s > 解析 4.25s）。
//!
//! 纪律：
//! * **不动水位**。`applied_sesno` 与模型计划的收口是调用方（`manual_update::initialize_dbnum_baseline`
//!   → `increment_tail::finalize_baseline`）的事，本模块只回「写了多少」。
//! * **任何失败先清库再 Err**（解析 / 写者 / 闭包 / 统计对不上）：`fast_delete::wipe_dbnum_for_reinit`
//!   把这个 dbnum 的行清干净，下一轮从 `applied_sesno = 0` 重来，不留半写的库。
//! * **`Fresh` 只插不删、已有行赢**：父层补缺（`replace_dbs = false`）把 extract 母库整库再摄入进同一个
//!   dbnum 时，叶子 extract 里正在改的版本不许被母库的旧版本覆盖。
//! * 冻结源反哈希出的 `dbnum / db_type` 必须等于调用方给的（[`check_identity`]），不等一行都不落。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aios_core::options::DbOption;
use aios_core::{RefU64, SUL_DB};
use anyhow::Context;
use futures::StreamExt;

use crate::data_interface::e3d_collector::{BaselineChunk, BaselineStats, stream_baseline};
use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::uda_keys::UdaKeyMap;
use crate::versioned_db::baseline::{render_baseline_batches, source_for_file};

/// 有界队列容量：解析最多领先写者这么多笔事务。
pub(crate) const QUEUE_CAPACITY: usize = 100;
/// 写者每次从队列取出的事务数。必须小于 [`QUEUE_CAPACITY`]：单写者 `chunks(N)` 要等到 N 条才
/// 交出，而生产者在 [`QUEUE_CAPACITY`] 条时阻塞——N ≥ 容量就是一个确定性的死锁。
pub(crate) const WRITE_WINDOW: usize = 20;
/// `sync_chunk_size` 没写时一块的元素数（与 legacy 同值：对绝大多数库等于整文件一块）。
const DEFAULT_CHUNK_ELEMENTS: usize = 10_0000;

/// 一个库文件的基线摄入回执。
#[derive(Debug, Clone)]
pub struct BaselineIngest {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    /// 交给写者的元素数，**不含 WORL**（`manual_update::baseline_parse_matches` 拿它对
    /// `pe 行数 − WORL 行数`）。
    pub elements: usize,
    pub stats: BaselineStats,
    /// CATA 依赖闭包的账；DESI 之外为 `None`。
    pub dependency: Option<DependencySummary>,
}

/// 文件收尾那一次 CATA 依赖闭包的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencySummary {
    pub seeds: usize,
    pub rows: usize,
    pub catalogue_dbnums: usize,
    pub missing: usize,
}

/// 写者任务的收尾账：失败次数与前几条样例（错误文本进日志与回执）。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WriterOutcome {
    pub errors: usize,
    pub samples: Vec<String>,
}

impl WriterOutcome {
    fn record(&mut self, context: &str, error: anyhow::Error) {
        self.errors += 1;
        if self.samples.len() < 3 {
            self.samples.push(format!("{context}: {error}"));
        }
    }
}

/// 冻结源反哈希出的库身份必须与调用方点名的一致。
///
/// legacy 壳按目录扫、以文件为准，调用方给的 dbnum 只用来从 `HashMap` 里取结果——文件名与
/// 文件头不一致时静默落进另一个 dbnum。单文件入口把它变成硬校验：不等就一行都不落。
pub(crate) fn check_identity(
    frozen_dbnum: i32,
    frozen_db_type: &str,
    dbnum: u32,
    db_type: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        u32::try_from(frozen_dbnum).is_ok_and(|actual| actual == dbnum),
        "基线文件身份不符：文件头 dbnum={frozen_dbnum}，调用方点名 dbnum={dbnum}；不落任何行"
    );
    anyhow::ensure!(
        frozen_db_type.eq_ignore_ascii_case(db_type),
        "基线文件身份不符：文件头 db_type={frozen_db_type}，调用方点名 db_type={db_type}；不落任何行"
    );
    Ok(())
}

/// 三条失败路合成一句；`None` = 全部干净。
///
/// 顺序就是发生顺序：解析失败时闭包根本没跑；写者的账无论如何都要读——它可能在解析成功后
/// 才失败，也可能与解析失败同时出现（队列满、写者退出让 sink 报错）。
pub(crate) fn failure_summary(
    parser: Option<&anyhow::Error>,
    dependency: Option<&anyhow::Error>,
    writer: &WriterOutcome,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(error) = parser {
        parts.push(format!("基线收集失败: {error:#}"));
    }
    if let Some(error) = dependency {
        parts.push(format!("CATA 依赖闭包失败: {error:#}"));
    }
    if writer.errors > 0 {
        parts.push(format!(
            "baseline SurrealDB write failed {} time(s); samples: {}",
            writer.errors,
            writer.samples.join("; ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("；"))
}

/// 把一个库文件按 `Fresh` 整库写进当前 SurrealDB。
///
/// * `target_sesno`：`Some` = 冻结在这一会话（叶子基线传 `file_latest_sesno`，与本轮文件身份复核
///   同一个会话）；`None` = 文件最新（父层补缺）。
/// * 冻结源优先复用数据批次 worker 手里的活动文件轮次（`active_frozen_source`），与
///   `source_for_file` 的规则一致。
///
/// 不推进水位；失败先清库（见模块注释）。
pub async fn ingest_file(
    db_option: &DbOption,
    project: &str,
    path: &Path,
    dbnum: u32,
    db_type: &str,
    target_sesno: Option<u32>,
) -> anyhow::Result<BaselineIngest> {
    let active = crate::data_interface::batch_worker::active_frozen_source()?;
    let frozen = source_for_file(path, target_sesno, active.as_ref())
        .with_context(|| format!("打开基线冻结源失败 {}", path.display()))?;
    check_identity(frozen.dbnum(), frozen.db_type(), dbnum, db_type)?;
    ingest_frozen(db_option, project, &frozen).await
}

/// [`ingest_file`] 的摄入主体：身份复核过之后，dbnum / db_type / 目标会话一律以冻结源为准。
///
/// 不单独对外：`ingest_file` 是唯一入口，点名与文件头必须先对上（[`check_identity`]）。
/// 2026-09-07 之前 `bootstrap_sys_metadata` 直接从这里进来——它按目录扫、手里没有点名的身份，
/// 于是文件头就是身份；那条空库引导已随 `bin/sync_sys_only` 退役。
async fn ingest_frozen(
    db_option: &DbOption,
    project: &str,
    frozen: &FrozenSource,
) -> anyhow::Result<BaselineIngest> {
    anyhow::ensure!(
        db_option.is_save_db(),
        "save_db = false 下不做基线：基线的唯一产出就是库里的行"
    );
    let dbnum = u32::try_from(frozen.dbnum())
        .map_err(|_| anyhow::anyhow!("基线文件 dbnum 为负: {}", frozen.dbnum()))?;
    let db_type = frozen.db_type().to_owned();
    let file_name = frozen
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("数据库文件名不是 UTF-8: {}", frozen.path().display()))?
        .to_owned();
    let target_sesno = frozen.target_sesno();
    // 空库不进基线是原义（legacy 在这里 `continue`，调用方再报「未返回目标文件结果」）。
    anyhow::ensure!(
        target_sesno > 0,
        "dbnum={dbnum} 文件 {file_name} 没有会话，基线不执行"
    );
    let started = Instant::now();
    println!(
        "开始基线摄入 {project} dbnum={dbnum} db_type={db_type} file={file_name} target_sesno={target_sesno}"
    );

    // 老库摘 legacy 事件；`execute_on` 里的 `require_no_event_on` 是硬闸，这一步让老库能过闸。
    crate::versioned_db::database::retire_dbnum_event().await?;
    // `ATT_UDA` 的 `u` 链接要把 UKEY 换成 `UDA:<refno>`；DICT 相先于 DESI 相，定义此时已在库里。
    // 查不到的键记账、随文件回执打一行，不静默丢。
    let uda_keys = Arc::new(UdaKeyMap::load().await.with_context(|| {
        format!("baseline UDA key table failed: file={file_name} dbnum={dbnum}")
    })?);
    let chunk_size = db_option
        .sync_chunk_size
        .map(|size| size as usize)
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_CHUNK_ELEMENTS);
    let is_design = db_type.eq_ignore_ascii_case("DESI");

    // 从这里起有写者在跑：之后的任何失败都要先排空、再清库。
    let (sender, receiver) = flume::bounded::<String>(QUEUE_CAPACITY);
    let writer = spawn_writer(receiver);

    // CATA 依赖 seed：流式收集时顺手攒本库每个 DESI 元素的目录链引用，文件收尾一次闭包，
    // 经同一条写者队列入库（单写者不变），排在调用方的 `finalize_baseline` 之前。
    let dependency_seeds = Arc::new(Mutex::new(BTreeSet::<RefU64>::new()));
    let parser = {
        let frozen = frozen.clone();
        let db_type = db_type.clone();
        let sender = sender.clone();
        let uda_keys = uda_keys.clone();
        let dependency_seeds = dependency_seeds.clone();
        // 文件 I/O + 解码是同步的 CPU 活，放阻塞池；有界队列的同步 `send` 只阻塞这条阻塞线程，
        // 写者任务照常在运行时上消费。
        tokio::task::spawn_blocking(move || {
            stream_baseline(&frozen, &db_type, chunk_size, |chunk| {
                dispatch_chunk(
                    &chunk,
                    dbnum,
                    is_design,
                    &uda_keys,
                    &dependency_seeds,
                    &sender,
                )
            })
        })
        .await
    };
    let parser: anyhow::Result<BaselineStats> = match parser {
        Ok(Ok(stats)) => Ok(stats),
        Ok(Err(error)) => Err(error),
        Err(join_error) => Err(anyhow::anyhow!("baseline task panicked: {join_error}")),
    };

    // 闭包缺失（库不在 MDB / 元素不在）只出声；解析器本身出错按基线失败处理（缺依赖的基线
    // 不算落地，与稳态窗口同纪律）。解析已失败就不跑——半个文件的 seed 没有意义。
    let seeds = std::mem::take(
        &mut *dependency_seeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let dependency: anyhow::Result<Option<DependencySummary>> = match &parser {
        Ok(_) if is_design && !seeds.is_empty() => {
            persist_dependency_closure(project, dbnum, &file_name, &seeds, &uda_keys, &sender)
                .await
                .map(Some)
        }
        _ => Ok(None),
    };

    let writer = drain_writer(sender, writer).await;
    // 解析任务是 spawn 出去的，攒下的属性解析失败在这里落库（见 `note_attrs_failure`）。
    // 排在裁决之前：解析失败恰恰是最需要留痕的那一次。
    if let Err(error) = crate::data_interface::parse_error::flush().await {
        log::warn!("{error:#}");
    }

    if let Some(summary) =
        failure_summary(parser.as_ref().err(), dependency.as_ref().err(), &writer)
    {
        log::error!("baseline failed: file={file_name} dbnum={dbnum}: {summary}");
        println!("{file_name}: 基线失败，清库后重来（dbnum={dbnum}）: {summary}");
        wipe_after_failure(dbnum).await?;
        anyhow::bail!("dbnum={dbnum} 基线摄入失败: {summary}");
    }
    let stats = parser.expect("failure_summary 为空即解析成功");
    let dependency = dependency.expect("failure_summary 为空即闭包成功");

    // 新路没有「属性残、拓扑在」的中间态（Q3 甲）：跑到这里就是这一文件的完整属性都解出来了，
    // 销掉旧进程可能留下的 ATTRS 账。
    crate::data_interface::parse_error::note_attrs_success(&file_name);
    if let Some(warning) = uda_keys.misses_warning(dbnum) {
        log::warn!("{file_name}: {warning}");
        println!("{file_name}: {warning}");
    }
    if stats.ignored > 0 {
        println!(
            "{file_name}: {} 条字典无属性 schema 的系统记录跳过（已记进解析失败账本）",
            stats.ignored
        );
    }
    println!(
        "解析任务完成, 耗时: {} s, 总数量: {}（索引 {} 条，WORL {} 个，{} 块，读页 {}）",
        started.elapsed().as_secs_f32(),
        stats.elements,
        stats.indexed,
        stats.worlds,
        stats.chunks,
        stats.pages_read
    );

    // 统计不在这里收尾（ADR-071 D6，2026-09-10）：`dbnum_info_table` 由 dbnum 任务在数据批次应用
    // 之后从目标索引序列化，基线与增量同一笔（`batch_worker` 的 `ref0_snapshot` → `task_end`）。
    // 此前这里 `rebuild_on` 一次再核 `pe_count == info_count`，那是逐笔观测块时代的对账。

    Ok(BaselineIngest {
        dbnum,
        db_type,
        file_name,
        elements: stats.elements,
        stats,
        dependency,
    })
}

/// 一块基线元素 → 若干笔事务 → 队列。队列满 / 写者退出以 `Err` 冒出 sink，让 `stream_baseline`
/// 中止并归因到本库；这里不许 panic（panic 绕过失败裁决，会留下一个半写的库）。
fn dispatch_chunk(
    chunk: &BaselineChunk,
    dbnum: u32,
    is_design: bool,
    uda_keys: &UdaKeyMap,
    dependency_seeds: &Mutex<BTreeSet<RefU64>>,
    sender: &flume::Sender<String>,
) -> anyhow::Result<()> {
    for ignored in &chunk.ignored {
        crate::data_interface::parse_error::note_element_failure(
            &ignored.refno.to_string(),
            &ignored.reason,
        );
    }
    if is_design {
        let seeds = crate::data_interface::cata_dependency::seeds_from_elements(
            chunk.elements.iter().map(|(_, element)| element),
        );
        dependency_seeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(seeds);
    }
    // 与增量路同一个渲染器（`element_batch`，Fresh）：每笔事务就是一条消息，切块按 `tx_limits`。
    let (transactions, _report) = render_baseline_batches(chunk, dbnum as i32, uda_keys)?;
    for sql in transactions {
        sender
            .send(sql)
            .map_err(|error| anyhow::anyhow!("baseline dispatch failed: {error}"))?;
    }
    Ok(())
}

/// 文件收尾的 CATA 依赖闭包：本库 DESI 元素依赖的目录链元素经同一条写者队列 `Fresh` 入库，
/// 每个目录库再补一行 `cata_dependency_state`（不是水位）。
async fn persist_dependency_closure(
    project: &str,
    dbnum: u32,
    file_name: &str,
    seeds: &BTreeSet<RefU64>,
    uda_keys: &UdaKeyMap,
    sender: &flume::Sender<String>,
) -> anyhow::Result<DependencySummary> {
    use crate::data_interface::cata_dependency;
    let rows = cata_dependency::resolve_dependency_rows(project, dbnum, seeds).await?;
    let batches = cata_dependency::render_dependency_transactions(
        &rows,
        uda_keys,
        crate::data_interface::element_batch::WriteMode::Fresh,
    )?;
    for (cata_dbnum, sqls) in &batches {
        let elements = rows.by_dbnum.get(cata_dbnum).map(Vec::len).unwrap_or(0);
        let sesno = rows
            .by_dbnum
            .get(cata_dbnum)
            .and_then(|elements| elements.first())
            .map(|(sesno, _)| *sesno)
            .unwrap_or(0);
        for sql in sqls {
            sender
                .send_async(sql.clone())
                .await
                .map_err(|error| anyhow::anyhow!("dependency dispatch failed: {error}"))?;
        }
        sender
            .send_async(cata_dependency::render_dependency_state(
                *cata_dbnum,
                sesno,
                elements,
            ))
            .await
            .map_err(|error| anyhow::anyhow!("dependency state dispatch failed: {error}"))?;
    }
    let summary = DependencySummary {
        seeds: seeds.len(),
        rows: rows.elements(),
        catalogue_dbnums: rows.by_dbnum.len(),
        missing: rows.manifest.missing,
    };
    println!(
        "{file_name}: CATA 依赖闭包 seeds={} → {} 行 / {} 库，缺失 {}",
        summary.seeds, summary.rows, summary.catalogue_dbnums, summary.missing
    );
    if summary.missing > 0 {
        log::warn!(
            "{file_name}: CATA 依赖闭包缺失 {} 个 refno（库不在 MDB / 元素不在），样例 {:?}",
            summary.missing,
            rows.manifest
                .missing_samples
                .iter()
                .take(5)
                .collect::<Vec<_>>()
        );
    }
    Ok(summary)
}

/// 单写者：按 [`WRITE_WINDOW`] 条一窗从队列取事务，逐笔 `execute_on`（含冲突重试）。
/// 一笔失败只记账、继续消费——队列必须被排空，否则生产者卡在 `send` 上永远收不了尾；
/// 账在 [`drain_writer`] 之后由 [`failure_summary`] 裁决。
fn spawn_writer(receiver: flume::Receiver<String>) -> tokio::task::JoinHandle<WriterOutcome> {
    tokio::task::spawn(async move {
        let mut stream = receiver.into_stream().chunks(WRITE_WINDOW);
        let mut outcome = WriterOutcome::default();
        while let Some(window) = stream.next().await {
            for sql in window {
                if let Err(error) =
                    crate::data_interface::pe_batch::execute_on(&SUL_DB, &sql).await
                {
                    outcome.record("element batch", error);
                }
            }
        }
        outcome
    })
}

/// 收尾写入管线：先关闭 sender、等写者把队列排空，再把账交出去。
///
/// 顺序是关键。解析失败时如果提前返回，已经解析好的数据会连同没人消费的通道一起被丢弃，
/// 而写者可能还在往库里写——清库要等它停下来才算得上「清」。
pub(crate) async fn drain_writer(
    sender: flume::Sender<String>,
    writer: tokio::task::JoinHandle<WriterOutcome>,
) -> WriterOutcome {
    drop(sender);
    match writer.await {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut outcome = WriterOutcome::default();
            outcome.record("writer task join failed", anyhow::anyhow!("{error}"));
            outcome
        }
    }
}

/// 失败收尾：清掉这个 dbnum 的全部行（水位清值不删行，下一轮按首次导入重来）。
async fn wipe_after_failure(dbnum: u32) -> anyhow::Result<()> {
    crate::data_interface::fast_delete::wipe_dbnum_for_reinit(dbnum)
        .await
        .map(|wiped| {
            println!("dbnum={dbnum} 基线失败清库完成：PE={}", wiped.pe_rows);
        })
        .with_context(|| format!("baseline chunk 失败后的清库未完成: dbnum={dbnum}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn writer_window_is_smaller_than_bounded_queue() {
        assert!(WRITE_WINDOW < QUEUE_CAPACITY);
    }

    /// I3：文件头身份与调用方点名不符就不落行。
    #[test]
    fn identity_mismatch_refuses_the_file() {
        check_identity(7999, "DESI", 7999, "DESI").unwrap();
        check_identity(7999, "desi", 7999, "DESI").unwrap();
        let wrong_dbnum = check_identity(7998, "DESI", 7999, "DESI").unwrap_err();
        assert!(
            wrong_dbnum.to_string().contains("dbnum=7998"),
            "{wrong_dbnum}"
        );
        let wrong_type = check_identity(7999, "CATA", 7999, "DESI").unwrap_err();
        assert!(
            wrong_type.to_string().contains("db_type=CATA"),
            "{wrong_type}"
        );
        assert!(check_identity(-1, "DESI", 7999, "DESI").is_err());
    }

    /// I4：三条失败路各自都要进裁决，写者的账不因解析成功而被忽略。
    #[test]
    fn failure_summary_covers_parser_dependency_and_writer() {
        let clean = WriterOutcome::default();
        assert_eq!(failure_summary(None, None, &clean), None);

        let parse = anyhow::anyhow!("parse boom");
        let summary = failure_summary(Some(&parse), None, &clean).unwrap();
        assert!(summary.contains("parse boom"), "{summary}");

        let dependency = anyhow::anyhow!("closure boom");
        let summary = failure_summary(None, Some(&dependency), &clean).unwrap();
        assert!(summary.contains("closure boom"), "{summary}");

        let mut writer = WriterOutcome::default();
        writer.record("element batch", anyhow::anyhow!("conflict on pe"));
        writer.record("element batch", anyhow::anyhow!("conflict on pe"));
        let summary = failure_summary(None, None, &writer).unwrap();
        assert!(
            summary.contains("2 time(s)") && summary.contains("conflict on pe"),
            "{summary}"
        );

        let summary = failure_summary(Some(&parse), None, &writer).unwrap();
        assert!(
            summary.contains("parse boom") && summary.contains("2 time(s)"),
            "{summary}"
        );
    }

    #[test]
    fn writer_outcome_keeps_only_three_samples() {
        let mut writer = WriterOutcome::default();
        for i in 0..5 {
            writer.record("element batch", anyhow::anyhow!("error {i}"));
        }
        assert_eq!(writer.errors, 5);
        assert_eq!(writer.samples.len(), 3);
    }

    /// 解析失败时也要先排空写者：不等待就返回的实现会把已解析的数据连同通道一起丢掉，
    /// 而清库会在写者还在写的时候发生。
    #[tokio::test]
    async fn drain_writer_waits_for_the_writer_to_consume_everything() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        const MESSAGES: usize = 8;
        let (sender, receiver) = flume::bounded::<String>(MESSAGES);
        for i in 0..MESSAGES {
            sender
                .send(format!(
                    "BEGIN TRANSACTION; INSERT IGNORE INTO pe [{{ id: pe:1_{i} }}] RETURN NONE; COMMIT TRANSACTION;"
                ))
                .unwrap();
        }
        let consumed = Arc::new(AtomicUsize::new(0));
        let writer_consumed = consumed.clone();
        let writer = tokio::task::spawn(async move {
            // 先歇一会儿：不等待 writer 就返回的实现，此刻计数一定还是 0。
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            let mut seen = 0usize;
            while receiver.recv_async().await.is_ok() {
                seen += 1;
            }
            writer_consumed.store(seen, Ordering::SeqCst);
            WriterOutcome::default()
        });

        let outcome = drain_writer(sender, writer).await;
        assert_eq!(outcome, WriterOutcome::default());
        assert_eq!(consumed.load(Ordering::SeqCst), MESSAGES, "写者必须被排空");
    }

    #[tokio::test]
    async fn drain_writer_reports_a_panicked_writer_as_a_failure() {
        let (sender, receiver) = flume::bounded::<String>(1);
        let writer = tokio::task::spawn(async move {
            drop(receiver);
            panic!("writer died");
        });
        let outcome = drain_writer(sender, writer).await;
        assert_eq!(outcome.errors, 1);
        assert!(
            outcome.samples[0].contains("writer task join failed"),
            "{:?}",
            outcome.samples
        );
    }

    fn crate_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// 代码行里提到 `needle`：注释行不算，字符串字面量里的（前一个字符是 `"`，如别处源码钉
    /// 的禁用词表）也不算——与 `data_interface::legacy_parse_pdms_db_guard::code_mentions_any` 同口径。
    fn code_lines_mentioning<'a>(source: &'a str, needle: &str) -> Vec<&'a str> {
        source
            .lines()
            .filter(|line| {
                let code = line.trim_start();
                !code.starts_with("//")
                    && code
                        .match_indices(needle)
                        .any(|(at, _)| !code[..at].ends_with('"'))
            })
            .collect()
    }

    /// I1：基线元素行只从本模块出去——生产代码里 `stream_baseline(` 的调用点只有这里
    /// （定义在 `e3d_collector`）。第二个调用点就是第二条基线编排。
    #[test]
    fn the_baseline_collector_has_exactly_one_production_caller() {
        let src = crate_root().join("src");
        let mut callers = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(&src)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/");
                if relative == "data_interface/e3d_collector.rs"
                    || relative == "versioned_db/baseline_ingest.rs"
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read source");
                if !code_lines_mentioning(&source, "stream_baseline(").is_empty() {
                    callers.push(relative);
                }
            }
        }
        assert!(
            callers.is_empty(),
            "基线收集器多出了调用点（第二条基线编排）：{callers:?}"
        );
    }

    /// I2 / I5：本模块不带 legacy 全量同步的任何形状；统计也不在这里收尾（ADR-071 D6：
    /// 基线与增量同一笔任务收尾快照，`rebuild_on` / `pe_count == info_count` 都不许回来）。
    #[test]
    fn the_ingest_entry_carries_no_legacy_sync_shape() {
        let source = include_str!("baseline_ingest.rs");
        let production = source.split_once("#[cfg(test)]").expect("tests follow").0;
        for forbidden in [
            "total_sync",
            "included_db_files",
            "manual_db_nums",
            "replace_dbs",
            "DashSet",
            "feature = \"sql\"",
            "initialize_on(",
            "rebuild_on(",
            "dbnum_stat_totals(",
            "settle_statistics(",
        ] {
            assert!(
                code_lines_mentioning(production, forbidden).is_empty(),
                "基线摄入入口不许带 legacy 形状 `{forbidden}`"
            );
        }
    }

    /// I4：清库只发生在写者排空之后，且排在成功回执之前的每条失败路上。
    #[test]
    fn cleanup_happens_after_the_writer_drained_and_before_success() {
        let source = include_str!("baseline_ingest.rs");
        let body = source
            .split_once("async fn ingest_frozen(")
            .expect("ingest_frozen")
            .1
            .split_once("fn dispatch_chunk(")
            .expect("dispatch follows")
            .0;
        let drain = body
            .find("drain_writer(sender, writer).await")
            .expect("writer drain");
        let judge = body.find("failure_summary(").expect("failure judgement");
        let wipe = body
            .find("wipe_after_failure(dbnum).await?")
            .expect("failure cleanup");
        let success = body.find("Ok(BaselineIngest {").expect("success receipt");
        assert!(
            drain < judge && judge < wipe && wipe < success,
            "顺序必须是 排空写者 → 裁决 → 清库 → 成功回执（统计收尾已随 ADR-071 D6 退出这里）"
        );
        assert_eq!(
            body.matches("wipe_after_failure(dbnum).await?").count(),
            1,
            "裁决失败那条路要清库；统计收尾那条已经没有了"
        );
        assert_eq!(
            body.matches(".expect(").count(),
            2,
            "摄入主体只允许在裁决之后解包两次（解析 / 闭包结果），别的地方不许 panic"
        );
    }
}
