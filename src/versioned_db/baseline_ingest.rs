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
//! | 执行 | [`persist_writer`]（落盘写者：有界队列 → 单写者，含期限与乐观冲突退避重试） |
//! | CATA 依赖 | [`cata_dependency`]（seed 流式攒、文件收尾一次闭包、同一条 lane） |
//! | 统计 | **没有**：Ref0 统计已整体退役（ADR-071 D6），dbnum 归属由 refno 直接算出（`refno_route::refno_dbnum`） |
//!
//! 形状：`spawn_blocking` 里流式收集 → 每块渲成若干笔事务（每条队列消息 = 一笔事务 SQL）→
//! 属性 lane（[`QUEUE_CAPACITY`] 有界队列 → **单写者**；SurrealDB 2.1 乐观事务下同一面多写者在共享表 /
//! 索引键上持续互撞，7997 真库曾耗尽重试预算）。解析与写入重叠：5525 实测写者才是瓶颈（排空 7.4s > 解析 4.25s）。
//!
//! 两个入口（ADR-077）：
//! * [`stream_frozen_into`]——**只往给定的属性 lane 流**：不落写入标记、不起写者、不排空、不清库。
//!   写者的拥有者是能看见两条 lane 的那一层（`data_interface::dbnum_initialize`），否则「先排空再清库」
//!   在双 lane 下立不住。
//! * [`ingest_file`]——单 lane 版：自己起属性 lane、自己排空、自己清库，给不带模型半边的调用方
//!   （手动 live 用例）用；语义与 2026-09-14 之前一致。
//!
//! 纪律：
//! * **不动水位**。`applied_sesno` 的收口是调用方的事，本模块只回「写了多少」。
//! * **任何失败先排空再清库再 Err**（解析 / 写者 / 闭包）：`fast_delete::wipe_dbnum_for_reinit`
//!   把这个 dbnum 的行清干净，下一轮从 `applied_sesno = 0` 重来，不留半写的库。
//! * **`Fresh` 只插不删、已有行赢**：父层补缺（`replace_dbs = false`）把 extract 母库整库再摄入进同一个
//!   dbnum 时，叶子 extract 里正在改的版本不许被母库的旧版本覆盖。
//! * 冻结源反哈希出的 `dbnum / db_type` 必须等于调用方给的（[`check_identity`]），不等一行都不落。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aios_core::RefU64;
use aios_core::options::DbOption;
use anyhow::Context;

use crate::data_interface::e3d_collector::{BaselineChunk, BaselineStats, stream_baseline};
use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::uda_keys::UdaKeyMap;
use crate::versioned_db::baseline::{render_baseline_batches, source_for_file};

/// 队列、单写者与它们的纪律都在 [`persist_writer`] 里，模型 lane 用的是同一份。
pub(crate) use crate::data_interface::persist_writer::{
    Lane, OnWriteError, QUEUE_CAPACITY, WriteJob, WriterOutcome,
};
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
    /// S0 取证（plan 2026-09-15）：解析 / 渲染拆项与送进 lane 的 SQL 字节。
    pub resolve_ms: u128,
    pub render_ms: u128,
    pub sql_bytes: usize,
    pub parse_requests: usize,
    pub parsed: usize,
    /// S2：解析缓存命中 / 未命中的元素数。
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// S3（D6）：这一版已经落过库、因此没有再送进 lane 的元素数。
    pub skipped_persisted: usize,
}

/// 一个冻结源流进属性 lane 之后的账——解析那一半；写者那一半在 lane 的 [`WriterOutcome`] 上。
#[derive(Debug, Clone)]
pub(crate) struct StreamedBaseline {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    pub target_sesno: u32,
    pub stats: BaselineStats,
    /// CATA 依赖闭包的账；DESI 之外为 `None`。
    pub dependency: Option<DependencySummary>,
    /// `UdaKeyMap` 查不到的键（有就记一句告警）。
    uda_misses_warning: Option<String>,
}

impl StreamedBaseline {
    /// 裁决通过之后的收尾：销掉旧进程可能留下的 ATTRS 账、打解析回执。
    ///
    /// 新路没有「属性残、拓扑在」的中间态（Q3 甲）：走到这里就是这一文件的完整属性都解出来并落了库。
    pub(crate) fn acknowledge(&self, started: Instant) {
        crate::data_interface::parse_error::note_attrs_success(&self.file_name);
        if let Some(warning) = &self.uda_misses_warning {
            log::warn!("{}: {warning}", self.file_name);
            println!("{}: {warning}", self.file_name);
        }
        if self.stats.ignored > 0 {
            println!(
                "{}: {} 条字典无属性 schema 的系统记录跳过（已记进解析失败账本）",
                self.file_name, self.stats.ignored
            );
        }
        println!(
            "解析任务完成, 耗时: {} s, 总数量: {}（索引 {} 条，WORL {} 个，{} 块，读页 {}）",
            started.elapsed().as_secs_f32(),
            self.stats.elements,
            self.stats.indexed,
            self.stats.worlds,
            self.stats.chunks,
            self.stats.pages_read
        );
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

/// 按路径冻结一个库文件并核对身份。
///
/// * `target_sesno`：`Some` = 冻结在这一会话（叶子基线传 `file_latest_sesno`，与本轮文件身份复核
///   同一个会话）；`None` = 文件最新（父层补缺）。
/// * 冻结源优先复用数据批次 worker 手里的活动文件轮次（`active_frozen_source`），与
///   `source_for_file` 的规则一致。
pub(crate) fn frozen_source_for(
    path: &Path,
    dbnum: u32,
    db_type: &str,
    target_sesno: Option<u32>,
) -> anyhow::Result<FrozenSource> {
    let active = crate::data_interface::batch_worker::active_frozen_source()?;
    let frozen = source_for_file(path, target_sesno, active.as_ref())
        .with_context(|| format!("打开基线冻结源失败 {}", path.display()))?;
    check_identity(frozen.dbnum(), frozen.db_type(), dbnum, db_type)?;
    Ok(frozen)
}

/// 把一个库文件按 `Fresh` 整库写进当前 SurrealDB——**单 lane 版**：自己起属性 lane、自己排空、
/// 自己清库。不推进水位。给不带模型半边的调用方用；双 lane 初始化走
/// `data_interface::dbnum_initialize`（它自己拿 [`stream_frozen_into`]）。
///
/// 写入标记、属性 lane 与解析失败落库写 `db`；失败清库仍走本进程的库。
pub async fn ingest_file(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    db_option: &DbOption,
    project: &str,
    path: &Path,
    dbnum: u32,
    db_type: &str,
    target_sesno: Option<u32>,
) -> anyhow::Result<BaselineIngest> {
    let frozen = frozen_source_for(path, dbnum, db_type, target_sesno)?;
    let started = Instant::now();
    // 写入标记 `writing{0, T}`（ADR-072 D1 / 计划 §4.2-1）：基线的起点恒为 0——库里没有
    // 「S 版本」可退，崩在中途的回退就是整库 wipe 再重排（D2 第 1 条）。
    mark_initialization_writing_on(db, dbnum, frozen.target_sesno()).await?;

    // 从这里起有写者在跑：之后的任何失败都要先排空、再清库。
    let lane = Lane::spawn_on(
        db.clone(),
        "element batch",
        QUEUE_CAPACITY,
        OnWriteError::Continue,
    );
    let sender = lane.sender();
    let streamed = stream_frozen_into(db_option, project, &frozen, &sender).await;
    drop(sender);
    let writer = lane.drain().await;
    // 解析任务是 spawn 出去的，攒下的属性解析失败在这里落库（见 `note_attrs_failure`）。
    // 排在裁决之前：解析失败恰恰是最需要留痕的那一次。
    if let Err(error) = crate::data_interface::parse_error::flush_on(db).await {
        log::warn!("{error:#}");
    }
    let streamed = match judge_streamed(streamed, &writer) {
        Ok(streamed) => streamed,
        Err(summary) => {
            let file_name = file_name_of(&frozen);
            log::error!("baseline failed: file={file_name} dbnum={dbnum}: {summary}");
            println!("{file_name}: 基线失败，清库后重来（dbnum={dbnum}）: {summary}");
            wipe_after_failure(dbnum).await?;
            anyhow::bail!("dbnum={dbnum} 基线摄入失败: {summary}");
        }
    };
    streamed.acknowledge(started);
    Ok(BaselineIngest {
        dbnum: streamed.dbnum,
        db_type: streamed.db_type,
        file_name: streamed.file_name,
        elements: streamed.stats.elements,
        stats: streamed.stats,
        dependency: streamed.dependency,
    })
}

/// 基线 / 初始化开工的写入标记 `writing{0, T}`（ADR-072 D1），落在 `db` 那个库。
///
/// 数据批次 worker 在冻结点已经落过一次标记，这里**重写**是为了把 S 与 intent 修正成初始化那一档：
/// 走到基线说明冻结点重解窗口判了「按首次导入」，worker 那一刻按队列行算出来的 S 可能不是 0
/// （回退检出的 `Reinitialize` 行尤其如此）。标记只有一行，重写即纠正。
pub(crate) async fn mark_initialization_writing_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<()> {
    crate::data_interface::write_state::mark_writing_on(
        db,
        dbnum,
        0,
        target_sesno as i32,
        crate::data_interface::write_state::WriteIntent::Initialize,
    )
    .await
    .with_context(|| format!("基线写入标记未落 dbnum={dbnum}：没有标记就没有崩溃出路"))
}

/// 解析结果 × 写者的账 → 干净就交出解析账，否则合成一句失败原因。
pub(crate) fn judge_streamed(
    streamed: anyhow::Result<StreamedBaseline>,
    writer: &WriterOutcome,
) -> Result<StreamedBaseline, String> {
    match streamed {
        Ok(streamed) => match failure_summary(None, None, writer) {
            None => Ok(streamed),
            Some(summary) => Err(summary),
        },
        Err(error) => {
            // 解析器与闭包的错误在 [`stream_frozen_into`] 里已经各自带了前缀，这里只并上写者的账。
            Err(failure_summary(Some(&error), None, writer).expect("an error is a failure"))
        }
    }
}

fn file_name_of(frozen: &FrozenSource) -> String {
    frozen
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<non-utf8>")
        .to_owned()
}

/// 把一个冻结源的元素批与 CATA 依赖闭包**流进给定的属性 lane**。
///
/// 不落写入标记、不起写者、不排空、不清库、不推进水位——这些都归拥有 lane 的那一层。
/// 身份复核过之后，dbnum / db_type / 目标会话一律以冻结源为准。返回 `Err` = 解析器或闭包出错
/// （各自带前缀）；已经送进 lane 的事务照常会被写者消费，调用方排空之后自己裁决、自己清库。
pub(crate) async fn stream_frozen_into(
    db_option: &DbOption,
    project: &str,
    frozen: &FrozenSource,
    sender: &flume::Sender<WriteJob>,
) -> anyhow::Result<StreamedBaseline> {
    stream_frozen_into_on(&aios_core::SUL_DB, db_option, project, frozen, sender).await
}

/// 同 [`stream_frozen_into`]：摘 legacy 事件走 `db` 那个库；UDA 键表仍读本进程的库。
pub(crate) async fn stream_frozen_into_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    db_option: &DbOption,
    project: &str,
    frozen: &FrozenSource,
    sender: &flume::Sender<WriteJob>,
) -> anyhow::Result<StreamedBaseline> {
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
    println!(
        "开始基线摄入 {project} dbnum={dbnum} db_type={db_type} file={file_name} target_sesno={target_sesno}"
    );

    // 老库摘 legacy 事件；`execute_on` 里的 `require_no_event_on` 是硬闸，这一步让老库能过闸。
    crate::data_interface::pe_batch::retire_event_on(db).await?;
    // `ATT_UDA` 的 `u` 链接要把 UKEY 换成 `UDA:<refno>`；DICT 相先于 DESI 相，定义此时已在库里。
    // 查不到的键记账、随文件回执打一行，不静默丢。
    let uda_keys = Arc::new(UdaKeyMap::load_on(db).await.with_context(|| {
        format!("baseline UDA key table failed: file={file_name} dbnum={dbnum}")
    })?);
    let chunk_size = db_option
        .sync_chunk_size
        .map(|size| size as usize)
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_CHUNK_ELEMENTS);
    let is_design = db_type.eq_ignore_ascii_case("DESI");

    // CATA 依赖 seed：流式收集时顺手攒本库每个 DESI 元素的目录链引用，文件收尾一次闭包，
    // 经同一条 lane 入库（单写者不变），排在调用方的收尾之前。
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
    let stats = match parser {
        Ok(Ok(stats)) => stats,
        Ok(Err(error)) => return Err(error.context("基线收集失败")),
        Err(join_error) => anyhow::bail!("基线收集失败: baseline task panicked: {join_error}"),
    };

    // 闭包缺失（库不在 MDB / 元素不在）只出声；解析器本身出错按基线失败处理（缺依赖的基线
    // 不算落地，与稳态窗口同纪律）。
    let seeds = std::mem::take(
        &mut *dependency_seeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let dependency = if is_design && !seeds.is_empty() {
        Some(
            persist_dependency_closure(
                db, db_option, project, dbnum, &file_name, &seeds, &uda_keys, sender,
            )
            .await
            .context("CATA 依赖闭包失败")?,
        )
    } else {
        None
    };

    Ok(StreamedBaseline {
        dbnum,
        db_type,
        file_name,
        target_sesno,
        uda_misses_warning: uda_keys.misses_warning(dbnum),
        stats,
        dependency,
    })
}

/// 一块基线元素 → 若干笔事务 → lane。队列满 / 写者退出以 `Err` 冒出 sink，让 `stream_baseline`
/// 中止并归因到本库；这里不许 panic（panic 绕过失败裁决，会留下一个半写的库）。
fn dispatch_chunk(
    chunk: &BaselineChunk,
    dbnum: u32,
    is_design: bool,
    uda_keys: &UdaKeyMap,
    dependency_seeds: &Mutex<BTreeSet<RefU64>>,
    sender: &flume::Sender<WriteJob>,
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
    let first = chunk
        .elements
        .first()
        .map(|(_, element)| element.refno.to_string())
        .unwrap_or_default();
    for (index, sql) in transactions.into_iter().enumerate() {
        sender
            .send(WriteJob::new(
                sql,
                format!("element batch dbnum={dbnum} chunk-first={first} tx#{index}"),
            ))
            .map_err(|error| anyhow::anyhow!("baseline dispatch failed: {error}"))?;
    }
    Ok(())
}

/// 文件收尾的 CATA 依赖闭包：本库 DESI 元素依赖的目录链元素经同一条 lane `Fresh` 入库，
/// 每个目录库再补一行 `cata_dependency_state`（不是水位）。
async fn persist_dependency_closure(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    db_option: &DbOption,
    project: &str,
    dbnum: u32,
    file_name: &str,
    seeds: &BTreeSet<RefU64>,
    uda_keys: &UdaKeyMap,
    sender: &flume::Sender<WriteJob>,
) -> anyhow::Result<DependencySummary> {
    use crate::data_interface::cata_dependency;
    let rows =
        cata_dependency::resolve_dependency_rows_on(db, db_option, project, dbnum, seeds).await?;
    let render_started = std::time::Instant::now();
    // D6：只查不记（plan Q3 第一阶段）——这条 lane 是异步单写者，成功要等 drain，
    // 记不了「写成功之后」那一刻；已经被同步出口落过的行照样不必再发一遍。
    let batches = cata_dependency::render_dependency_transactions_unpersisted(
        &rows,
        uda_keys,
        crate::data_interface::element_batch::WriteMode::Fresh,
    )?;
    let render_ms = render_started.elapsed().as_millis();
    let sql_bytes: usize = batches
        .by_dbnum
        .iter()
        .flat_map(|(_, batch)| batch.transactions.iter())
        .map(String::len)
        .sum();
    for (cata_dbnum, batch) in &batches.by_dbnum {
        for (index, sql) in batch.transactions.iter().enumerate() {
            sender
                .send_async(WriteJob::new(
                    sql.clone(),
                    format!("cata dependency dbnum={dbnum} cata={cata_dbnum} tx#{index}"),
                ))
                .await
                .map_err(|error| anyhow::anyhow!("dependency dispatch failed: {error}"))?;
        }
        sender
            .send_async(WriteJob::new(
                cata_dependency::render_dependency_state(
                    *cata_dbnum,
                    batch.sesno,
                    batch.refnos.len(),
                ),
                format!("cata dependency state dbnum={dbnum} cata={cata_dbnum}"),
            ))
            .await
            .map_err(|error| anyhow::anyhow!("dependency state dispatch failed: {error}"))?;
    }
    let summary = DependencySummary {
        seeds: seeds.len(),
        rows: rows.elements(),
        catalogue_dbnums: rows.by_dbnum.len(),
        missing: rows.manifest.missing,
        resolve_ms: rows.account.resolve_ms,
        render_ms,
        sql_bytes,
        parse_requests: rows.account.closure.parse_requests,
        parsed: rows.account.closure.parsed,
        cache_hits: rows.account.closure.cache_hits,
        cache_misses: rows.account.closure.cache_misses,
        skipped_persisted: batches.skipped,
    };
    println!(
        "{file_name}: CATA 依赖闭包 seeds={} → {} 行 / {} 库，缺失 {}（resolve {} ms · render {} ms · SQL {} KB · parse {} 次 / {} 成功 · 缓存 {} 命中 / {} 未命中 · 已落库跳过 {}）",
        summary.seeds,
        summary.rows,
        summary.catalogue_dbnums,
        summary.missing,
        summary.resolve_ms,
        summary.render_ms,
        summary.sql_bytes / 1024,
        summary.parse_requests,
        summary.parsed,
        summary.cache_hits,
        summary.cache_misses,
        summary.skipped_persisted,
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

/// 失败收尾：清掉这个 dbnum 的全部行（水位清值不删行，下一轮按首次导入重来）。
pub(crate) async fn wipe_after_failure(dbnum: u32) -> anyhow::Result<()> {
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

    /// 裁决：解析成功 + 写者干净 = 通过；写者有账 / 解析出错 = 一句合成的失败原因。
    #[test]
    fn judging_a_stream_folds_parser_and_writer_into_one_verdict() {
        fn streamed() -> StreamedBaseline {
            StreamedBaseline {
                dbnum: 7999,
                db_type: "DESI".into(),
                file_name: "ams7999_0001".into(),
                target_sesno: 30,
                stats: BaselineStats::default(),
                dependency: None,
                uda_misses_warning: None,
            }
        }
        let clean = WriterOutcome::default();
        assert!(judge_streamed(Ok(streamed()), &clean).is_ok());

        let mut writer = WriterOutcome::default();
        writer.record("element batch", anyhow::anyhow!("statement failed"));
        let summary = judge_streamed(Ok(streamed()), &writer).unwrap_err();
        assert!(summary.contains("1 time(s)"), "{summary}");

        let summary = judge_streamed(Err(anyhow::anyhow!("boom").context("基线收集失败")), &clean)
            .unwrap_err();
        assert!(summary.contains("基线收集失败"), "{summary}");
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

    /// 流那一半不拥有写者：它不落写入标记、不起 lane、不排空、不清库、不推水位（ADR-077 D8：
    /// 「先排空再清库」只有拥有两条 lane 的那一层做得对）。
    #[test]
    fn the_streaming_half_owns_no_writer_and_no_cleanup() {
        let source = include_str!("baseline_ingest.rs");
        let body = source
            .split_once("pub(crate) async fn stream_frozen_into(")
            .expect("stream_frozen_into")
            .1
            .split_once("fn dispatch_chunk(")
            .expect("dispatch follows")
            .0;
        for forbidden in [
            "Lane::spawn",
            "spawn_writer",
            "drain(",
            "wipe_after_failure",
            "mark_writing",
            "mark_initialization_writing",
            "finalize_baseline",
            "render_watermark_advance(",
        ] {
            assert!(
                code_lines_mentioning(body, forbidden).is_empty(),
                "stream_frozen_into 不许 `{forbidden}`：写者与清库归拥有两条 lane 的编排层"
            );
        }
    }

    /// I4（单 lane 版）：清库只发生在写者排空之后，且排在成功回执之前。
    #[test]
    fn cleanup_happens_after_the_writer_drained_and_before_success() {
        let source = include_str!("baseline_ingest.rs");
        let body = source
            .split_once("pub async fn ingest_file(")
            .expect("ingest_file")
            .1
            .split_once("pub(crate) async fn mark_initialization_writing_on(")
            .expect("marker helper follows")
            .0;
        let drain = body.find("lane.drain().await").expect("writer drain");
        let judge = body.find("judge_streamed(").expect("failure judgement");
        let wipe = body
            .find("wipe_after_failure(dbnum).await?")
            .expect("failure cleanup");
        let success = body.find("Ok(BaselineIngest {").expect("success receipt");
        assert!(
            drain < judge && judge < wipe && wipe < success,
            "顺序必须是 排空写者 → 裁决 → 清库 → 成功回执"
        );
        assert_eq!(
            body.matches("wipe_after_failure(dbnum).await?").count(),
            1,
            "裁决失败那条路要清库，且只有这一处"
        );
    }

    /// 初始化写入标记 `writing{0, T}` 落在给它的库里。
    #[tokio::test]
    async fn the_initialization_marker_lands_in_the_given_database() {
        use crate::data_interface::write_state::{WriteIntent, load_on};
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("baseline_ingest").use_db("marker").await.unwrap();

        mark_initialization_writing_on(&db, 7997, 43).await.unwrap();

        let marker = load_on(&db, 7997)
            .await
            .unwrap()
            .expect("写入标记必须落在给定的库里");
        assert!(marker.is_writing(), "{marker:?}");
        assert_eq!(
            (marker.start_sesno, marker.end_sesno, marker.intent),
            (0, 43, WriteIntent::Initialize)
        );
    }

    /// 流那一半摘 legacy 事件摘的是给它的库。拼出来的冻结源不指向真实文件，所以这一趟本身
    /// 必然失败——这里只看事件有没有从给定的库里摘掉。
    #[tokio::test]
    async fn the_streaming_half_retires_the_legacy_event_on_the_given_database() {
        use crate::data_interface::pe_batch::require_no_event_on;
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("baseline_ingest").use_db("retire").await.unwrap();
        db.query("DEFINE EVENT update_dbnum_event ON pe WHEN true THEN { RETURN NONE; };")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(require_no_event_on(&db).await.is_err());
        let frozen = FrozenSource::synthetic("ams8000", 8000, "DESI", 42);
        let (sender, _receiver) = flume::unbounded();

        let streamed =
            stream_frozen_into_on(&db, &DbOption::default(), "AMS", &frozen, &sender).await;

        assert!(streamed.is_err(), "拼出来的冻结源不该流得通");
        require_no_event_on(&db).await.unwrap();
    }

    /// UDA 键表从给它的库读：本进程的库在单测里没连，读它会停在「UDA 键表失败」；读给定的库
    /// 则走到冻结源本身（拼出来的，不指向真实文件）才失败。
    #[tokio::test]
    async fn the_streaming_half_reads_uda_keys_from_the_given_database() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("baseline_ingest").use_db("uda").await.unwrap();
        let frozen = FrozenSource::synthetic("ams8000", 8000, "DESI", 42);
        let (sender, _receiver) = flume::unbounded();

        let Err(error) =
            stream_frozen_into_on(&db, &DbOption::default(), "AMS", &frozen, &sender).await
        else {
            panic!("拼出来的冻结源不该流得通");
        };
        let message = format!("{error:#}");
        assert!(
            !message.contains("baseline UDA key table failed"),
            "{message}"
        );
        assert!(message.contains("基线收集失败"), "{message}");
    }
}
