//! 一个 dbnum 初始化的编排（ADR-077）：**解析 ∥ 生成 ∥ 两条写入 lane → 排空 → 核对 → 一笔收尾**。
//!
//! ```text
//!              ┌─ 解析 spawn_blocking ──► 元素批 SQL ──► [属性 lane: flume(100) → 单写者] ─┐
//! initialize ──┤                                                                          ├─► drain ×2 → 核对 → task_end（一笔）
//!              └─ produce_slices ──► mpsc(1) ──► 渲染(Initial + 房间边) ──► [模型 lane: flume(2) → 单写者] ─┘
//!                                                 └─ 盒子攒着 → 排空成功后收敛一次
//! ```
//!
//! 谁拥有什么：
//! * **写者归这里。** 两条 lane（`persist_writer::Lane`）在这一层起、在这一层排空——「先排空再清库」只有能看见
//!   两条 lane 的这一层做得对（ADR-077 D8）。解析那一半（`baseline_ingest::stream_frozen_into`）与生成那一半
//!   （`E3dModelService::produce_slices` + `project_slice_for_initialization` + `model_initial_persist::render_slice`）
//!   只往 lane 里流，不碰写者、不清库、不推水位。
//! * **投影不参与。** 初始化的模型行不进 `ModelMemoryStore`、不登记 `ModelWriteBehind`（ADR-076 D11 / ADR-077 D5）；
//!   根回执**不落库**（ADR-079：`gen_root` 退役）——两 lane 排空成功后逐根成败进进程内账本
//!   （`model_root_ledger`），空间树的盒子在内存里攒着、同一刻收敛一次。
//! * **水位一个写者。** `applied_sesno` + `model_sesno` + 写入标记 `finished` + Ref0 统计在收尾那**一笔**
//!   （`task_end::TaskEnd::data`，ADR-077 D7）。这一笔之前库里有行没水位，崩了就是写入标记 `writing` →
//!   下次整库重来（ADR-073 D3）。
//!
//! 失败两档（ADR-077 D8）：
//! * **lane 失败**（解析器 / 闭包出错、任一写者有失败事务）→ 两条 lane 排空 → `wipe_dbnum_for_reinit` → `Err`。
//!   数据半边一出错就用 `try_join` 把模型半边掐掉（生产端在下一次 hand-off 看到 receiver 关了自己停）。
//! * **模型半边自己的问题**（服务打不开、根枚举失败、单根死信、房间计算器冻不出来）**不是** lane 失败：
//!   属性行照落、`applied_sesno` 照盖、写入标记照收，`model_sesno` 不盖，回执里 `Partial` + 逐根 `Failed`
//!   （ADR-065 D4 / ADR-073 W6）。

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use aios_core::options::DbOption;
use anyhow::Context;
use e3d_io::refno::RefNo;

use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::generation_root::GenerationRoot;
use crate::data_interface::manual_update::{ModelUnitResult, UnitGenStatus, UnitKind};
use crate::data_interface::persist_writer::{
    Lane, MODEL_LANE_DEPTH, OnWriteError, QUEUE_CAPACITY, WriteJob, WriterOutcome,
};
use crate::data_interface::task_end::{
    RoomGenerationStamp, TaskDataOutcome, TaskEnd, TaskModelOutcome,
};
use crate::versioned_db::baseline_ingest::{self, StreamedBaseline};

/// 模型半边跑不跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelHalf {
    /// DESI 库随初始化一起生成整库模型、直写 RocksDB（dbnum 任务那条路）。
    Generate,
    /// 只落属性面（SYS meta / CATA、规格工具那条路：模型留给启动核对按凭证缺口排 ModelOnly）。
    Skip,
}

/// 一次初始化要的全部输入。文件身份已由调用方在冻结点复核过。
pub struct InitializationRequest<'a> {
    pub project: &'a str,
    pub dbnum: u32,
    pub db_type: &'a str,
    pub file_path: &'a Path,
    pub file_name: &'a str,
    /// 叶子 extract 旁的母库（ADR-028）：叶子缺号时整库再摄入一遍补父层。
    pub extract_parent: Option<&'a Path>,
    /// 本轮复核过的文件最新会话 = 初始化冻结的目标 `T`。
    pub file_latest_sesno: i32,
    /// `T` 那一版的保存时刻（水位行的 `applied_sesno_time` / `model_sesno_time`）；拿不到就不写。
    pub target_time: Option<String>,
    pub model: ModelHalf,
}

/// 一次初始化交回任务的东西。
#[derive(Debug, Clone, Default)]
pub struct InitializationOutcome {
    /// 库里这一 dbnum 的 `pe` 行数（含 WORL），与 2026-09-14 之前 `BaselineOutcome::elements` 同口径。
    pub elements: usize,
    /// 合法空库（ADR-021 空基线凭据）。
    pub confirmed_empty: bool,
    /// 逐根回执（模型半边跑了才有）：`Generated` / `Failed`。
    pub units: Vec<ModelUnitResult>,
    /// 模型半边的结论；`None` = 没跑模型（Skip / 非 DESI / 阶段开关关着）。
    pub model: Option<TaskModelOutcome>,
    pub warnings: Vec<String>,
    /// 两条 lane 的账。
    pub attribute_writer: WriterOutcome,
    pub model_writer: WriterOutcome,
    /// 收尾那一笔盖上了 `model_sesno`。
    pub model_stamped: bool,
}

/// 模型半边的产物（内存里的，写者那半在 lane 的账上）。
#[derive(Default)]
struct ModelHalfReport {
    /// 没跑（Skip / 非 DESI / 阶段关着）；`Some(理由)` 进 warnings。
    skipped: Option<String>,
    units: Vec<ModelUnitResult>,
    /// 交给模型 lane 的事务数与记录数。
    transactions: usize,
    records: usize,
    room_edges: usize,
    /// 排空成功后进空间树的盒子与它们的来源。
    tree_boxes: Vec<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>,
    refreshed: Vec<aios_core::RefnoEnum>,
    /// 本次生成照的目录版本（CATA dbnum → 会话，取最旧）：整库都成时随 `model_sesno` 盖成库级
    /// 目录落款（ADR-079）。
    catalogue_sessions: std::collections::BTreeMap<u32, u32>,
    /// 本次逐根算房间用的那台计算器的代号（ADR-073 D14）：初始化生成的是这个库的**每一个**根，
    /// 它们的边都照这一代算，整库都成时随 `model_sesno` 盖成 `model_room_generation`。
    /// `None` = 房间开关关着（`frozen_room_calculator` 回 `Ok(None)`），收尾把库上那一格清空。
    room_generation: Option<String>,
    /// 模型半边自己的错（服务打不开、枚举失败、`Required` 报错…）：不是 lane 失败，进 warnings。
    failure: Option<String>,
}

impl ModelHalfReport {
    fn failed(reason: String) -> Self {
        Self {
            failure: Some(reason),
            ..Self::default()
        }
    }

    fn skipped(reason: String) -> Self {
        Self {
            skipped: Some(reason),
            ..Self::default()
        }
    }

    /// 收尾那一笔要写的模型结论：跑了才有；跑了而一根没成 / 半边报错 = `all_roots_ok: false`。
    /// 整库都成才连库级目录落款一起盖（ADR-079）：初始化生成的是这个库的**每一个**根，它们照的
    /// 目录版本就是整个库的目录版本。durable 侧的房间就绪承诺（ADR-073 D14）同一条纪律、同一个
    /// 理由：每个根都是这一代算的，所以初始化说得了「整库的边对齐到这一代」。
    fn outcome(&self) -> Option<TaskModelOutcome> {
        if self.skipped.is_some() {
            return None;
        }
        let dead_roots = self
            .units
            .iter()
            .filter(|unit| unit.status == UnitGenStatus::Failed)
            .count();
        let all_roots_ok = self.failure.is_none() && dead_roots == 0;
        Some(TaskModelOutcome {
            all_roots_ok,
            dead_roots,
            catalogue_sessions: all_roots_ok.then(|| self.catalogue_sessions.clone()),
            room_generation: match (all_roots_ok, &self.room_generation) {
                (false, _) => RoomGenerationStamp::Untouched,
                (true, Some(generation)) => RoomGenerationStamp::Aligned(generation.clone()),
                (true, None) => RoomGenerationStamp::Cleared,
            },
        })
    }

    /// 两 lane 排空成功之后，把逐根成败记进进程内账本（ADR-079：`gen_root` 回执 / 死信留痕的替身）。
    /// 排在收尾那一笔之前：账本是本进程的工作记录，收尾盖不盖章都要记。
    fn record_in_ledger(&self, dbnum: u32, source_end_sesno: i32) {
        use crate::data_interface::model_root_ledger as ledger;
        for unit in &self.units {
            match unit.status {
                UnitGenStatus::Generated => {
                    ledger::record_generated_str(dbnum, &unit.root_refno, &unit.noun, source_end_sesno)
                }
                _ => ledger::record_failed_str(
                    dbnum,
                    &unit.root_refno,
                    &unit.noun,
                    source_end_sesno,
                    unit.message.as_deref().unwrap_or("initialization root failed"),
                ),
            }
        }
    }
}

/// 数据半边的产物：叶子那一份 + 父层补缺那一份（有的话）。
struct DataHalfReport {
    leaf: StreamedBaseline,
    /// 父层补缺摄入了母库（`Fresh`，已有行赢）；补过之后 `parsed_count` 不再能与 pe 行数对账。
    parent_filled: bool,
}

/// 跑一次初始化。返回 `Err` = 库里已经清干净（或清不干净也已经报出来），下一轮从 0 重来。
pub(crate) async fn initialize(
    db_option: &DbOption,
    request: InitializationRequest<'_>,
) -> anyhow::Result<InitializationOutcome> {
    let dbnum = request.dbnum;
    let target_sesno = u32::try_from(request.file_latest_sesno)
        .with_context(|| format!("dbnum={dbnum} 目标会话 {} 不合法", request.file_latest_sesno))?;
    let started = Instant::now();
    let frozen = baseline_ingest::frozen_source_for(
        request.file_path,
        dbnum,
        request.db_type,
        Some(target_sesno),
    )?;
    // 写入标记 `writing{0, T}`（ADR-072 D1）：从这里起到收尾那一笔之间崩，下次启动整库重来。
    baseline_ingest::mark_initialization_writing(dbnum, frozen.target_sesno()).await?;

    // 从这里起有写者在跑：之后的任何失败都要先排空、再清库。
    crate::data_interface::batch_worker::set_active_task_stage("ingest_and_generate");
    let attributes = Lane::spawn("element batch", QUEUE_CAPACITY, OnWriteError::Continue);
    let model_lane = Lane::spawn("model slice", MODEL_LANE_DEPTH, OnWriteError::StopWriting);

    let data = data_half(
        db_option,
        request.project,
        &frozen,
        request.file_path,
        request.extract_parent,
        attributes.sender(),
    );
    let model = model_half(
        request.model,
        dbnum,
        request.db_type,
        &frozen,
        model_lane.sender(),
    );
    // 数据半边一错就掐掉模型半边（`try_join` 丢掉另一个 future → 生产端下一次 hand-off 自己停）；
    // 模型半边自己的错不是 lane 失败，包成 `Ok` 让数据半边跑完。
    let halves = futures::future::try_join(data, async { Ok::<_, anyhow::Error>(model.await) }).await;

    crate::data_interface::batch_worker::set_active_task_stage("drain_lanes");
    let (attribute_writer, model_writer) =
        futures::future::join(attributes.drain(), model_lane.drain()).await;
    // 解析任务是 spawn 出去的，攒下的属性解析失败在这里落库；排在裁决之前——解析失败恰恰是最需要留痕的那一次。
    if let Err(error) = crate::data_interface::parse_error::flush().await {
        log::warn!("{error:#}");
    }

    // ── 裁决：lane 失败 → 清库 → Err ────────────────────────────────────────────────
    let (data, model) = match halves {
        Ok((data, model)) => (data, model),
        Err(error) => {
            return wipe_and_bail(dbnum, request.file_name, format!("{error:#}")).await;
        }
    };
    let writer_failures = [
        attribute_writer.failure("attribute lane"),
        model_writer.failure("model lane"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !writer_failures.is_empty() {
        return wipe_and_bail(dbnum, request.file_name, writer_failures.join("；")).await;
    }
    data.leaf.acknowledge(started);
    // 模型 lane 排空成功 = 交出去的每一笔都在库里了：逐根成败进进程内账本（ADR-079）。
    model.record_in_ledger(dbnum, request.file_latest_sesno);

    // ── 核对（ADR-077 D11）：属性面照旧数两次 `pe`；模型面不数库，任务手里的清单就是证据 ────
    crate::data_interface::batch_worker::set_active_task_stage("verify");
    let (pe_count, root_count) = baseline_counts(dbnum).await?;
    let parsed_count = (!data.parent_filled).then_some(data.leaf.stats.elements);
    let confirmed_empty = crate::data_interface::manual_update::baseline_parse_confirmed_empty(
        parsed_count,
        pe_count,
        root_count,
    );
    if !confirmed_empty {
        anyhow::ensure!(
            pe_count > 0,
            "dbnum={dbnum} 基线解析完成后仍没有 PE 数据"
        );
        if let Some(parsed_count) = parsed_count {
            anyhow::ensure!(
                crate::data_interface::manual_update::baseline_parse_matches(
                    pe_count, root_count, parsed_count
                ),
                "dbnum={dbnum} 基线不完整: PE={pe_count} WORL={root_count} 本次成功解析={parsed_count}; 不推进 applied_sesno"
            );
        }
    }

    let mut warnings = Vec::new();
    if let Some(reason) = &model.skipped {
        warnings.push(format!("dbnum={dbnum} 初始化未生成模型：{reason}"));
    }
    if let Some(reason) = &model.failure {
        warnings.push(format!("dbnum={dbnum} 初始化模型半边失败（属性面已落库，模型水位不盖）：{reason}"));
    }

    // ── 空间树：盒子在内存里攒着，两 lane 都排空成功了才进树（ADR-077 D5 (b)）──────────────
    if !model.tree_boxes.is_empty() || !model.refreshed.is_empty() {
        let synced = crate::fast_model::aabb_tree::converge_tree_after_persist(
            &model.refreshed,
            model.tree_boxes.clone(),
            &[],
        )
        .await;
        log::info!("dbnum={dbnum} 初始化后空间树收敛：{synced} 条进树");
    }

    // ── 收尾一笔（ADR-077 D7）：Ref0 统计 + applied + 写入标记 finished + model ──────────────
    crate::data_interface::batch_worker::set_active_task_stage("task_end");
    let info = {
        let source = crate::data_interface::ref0_snapshot::SnapshotSource {
            frozen: Some(frozen.clone()),
            path: request.file_path.to_path_buf(),
            file_name: request.file_name.to_owned(),
            db_type: request.db_type.to_owned(),
        };
        match tokio::task::spawn_blocking(move || {
            crate::data_interface::ref0_snapshot::snapshot_at(&source, target_sesno)
        })
        .await
        .map_err(|error| anyhow::anyhow!("Ref0 统计快照任务异常: {error}"))
        .and_then(|snapshot| snapshot)
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                warnings.push(format!(
                    "Ref0 统计快照未取到（数据行与水位不受影响，下次任务再序列化） dbnum={dbnum} sesno={target_sesno}: {error:#}"
                ));
                None
            }
        }
    };
    let model_outcome = model.outcome();
    let end = TaskEnd {
        dbnum,
        target_sesno: request.file_latest_sesno,
        target_time: request.target_time.clone(),
        data: Some(TaskDataOutcome { confirmed_empty }),
        model: model_outcome.clone(),
        info,
        closes_task: true,
    };
    let receipt = crate::data_interface::task_end::commit_task_end_on(&aios_core::SUL_DB, &end)
        .await
        .with_context(|| format!("dbnum={dbnum} 初始化收尾一笔未落（写入标记仍是 writing，下次启动整库重来）"))?;
    // 整库房间重算的欠账（审核 P1-2）随 `Aligned` 一并销掉：这一库的每一根都是这台计算器算的，
    // 上一窗欠下的那次整库重算已经没有意义。没盖 `Aligned`（有根失败 / 开关关着）就留着。
    if model_outcome.as_ref().is_some_and(|outcome| {
        matches!(outcome.room_generation, RoomGenerationStamp::Aligned(_))
    }) && let Err(error) = crate::data_interface::room_recompute::discharge(dbnum).await
    {
        warnings.push(format!(
            "dbnum={dbnum} 整库房间重算欠账未销（初始化已把每一根对齐；下一个任务会多跑一次整库重算）: {error:#}"
        ));
    }

    println!(
        "dbnum={dbnum} 初始化完成 耗时={}ms pe={pe_count} worl={root_count} 属性 lane: tx={} bytes={} 冲突重试={} 超时={}；模型 lane: tx={} bytes={} 冲突重试={} 超时={} 根={} 失败根={} 记录={} 房间边={} model_sesno={}",
        started.elapsed().as_millis(),
        attribute_writer.transactions,
        attribute_writer.bytes,
        attribute_writer.conflict_retries,
        attribute_writer.timeouts,
        model_writer.transactions,
        model_writer.bytes,
        model_writer.conflict_retries,
        model_writer.timeouts,
        model.units.len(),
        model
            .units
            .iter()
            .filter(|unit| unit.status == UnitGenStatus::Failed)
            .count(),
        model.records,
        model.room_edges,
        receipt
            .model_sesno
            .map(|sesno| sesno.to_string())
            .unwrap_or_else(|| "未盖".to_owned()),
    );

    Ok(InitializationOutcome {
        elements: pe_count,
        confirmed_empty,
        units: model.units,
        model: model_outcome,
        warnings,
        attribute_writer,
        model_writer,
        model_stamped: receipt.model_sesno.is_some(),
    })
}

/// lane 失败的出口：清库、报错。清库自己失败也要 Err——两个原因并成一句。
async fn wipe_and_bail<T>(dbnum: u32, file_name: &str, summary: String) -> anyhow::Result<T> {
    log::error!("initialization failed: file={file_name} dbnum={dbnum}: {summary}");
    println!("{file_name}: 初始化失败，清库后重来（dbnum={dbnum}）: {summary}");
    baseline_ingest::wipe_after_failure(dbnum).await?;
    anyhow::bail!("dbnum={dbnum} 初始化失败: {summary}")
}

/// 属性半边：叶子文件流进属性 lane；叶子缺号时母库整库再流一遍（`Fresh`，已有行赢）。
async fn data_half(
    db_option: &DbOption,
    project: &str,
    frozen: &FrozenSource,
    file_path: &Path,
    extract_parent: Option<&Path>,
    sender: flume::Sender<WriteJob>,
) -> anyhow::Result<DataHalfReport> {
    let leaf = baseline_ingest::stream_frozen_into(db_option, project, frozen, &sender).await?;
    let dbnum = leaf.dbnum;
    let parent = extract_parent
        .map(Path::to_path_buf)
        .or_else(|| crate::data_interface::extract_family::parent_path_of(file_path))
        .filter(|path| path.is_file() && path != file_path);
    let mut parent_filled = false;
    if let Some(parent) = parent {
        match crate::data_interface::extract_family::parent_gap_refno_count(file_path, &parent) {
            Ok(0) => {}
            Ok(gap) => {
                println!(
                    "dbnum={dbnum} 父层有 {gap} 个叶子缺号，补解析 {}",
                    parent.display()
                );
                // 母库整库再摄入进同一个 dbnum：`Fresh` 已有行赢，叶子 extract 里正在改的版本不会被
                // 母库的旧版本覆盖；目标会话取母库最新。
                let parent_frozen = baseline_ingest::frozen_source_for(
                    &parent,
                    dbnum,
                    &leaf.db_type,
                    None,
                )?;
                baseline_ingest::stream_frozen_into(db_option, project, &parent_frozen, &sender)
                    .await
                    .with_context(|| {
                        format!(
                            "dbnum={dbnum} 父层补解析失败 {}；不推进 applied_sesno",
                            parent.display()
                        )
                    })?;
                parent_filled = true;
            }
            Err(error) => anyhow::bail!(
                "dbnum={dbnum} 父层缺号探测失败 {}: {error:#}",
                parent.display()
            ),
        }
    }
    drop(sender);
    Ok(DataHalfReport { leaf, parent_filled })
}

/// 模型半边：从文件枚举交付单元根 → 并行生成 → 逐片渲成事务文本 → 模型 lane。
///
/// 这里的每一个 `Err` 都被折成 [`ModelHalfReport::failure`]（不是 lane 失败）；只有写者的账才决定清不清库。
async fn model_half(
    mode: ModelHalf,
    dbnum: u32,
    db_type: &str,
    frozen: &FrozenSource,
    sender: flume::Sender<WriteJob>,
) -> ModelHalfReport {
    if mode == ModelHalf::Skip {
        return ModelHalfReport::skipped("调用方只要属性面（模型留给启动核对）".to_owned());
    }
    if !db_type.eq_ignore_ascii_case("DESI") {
        return ModelHalfReport::skipped(format!("{db_type} 库没有模型"));
    }
    if !crate::options::model_incremental() {
        return ModelHalfReport::skipped(
            "model_incremental=false，本窗模型工作不执行，模型阶段开启后由恢复任务按凭证缺口重排".to_owned(),
        );
    }
    if let Err(error) = crate::data_interface::initialization_phase::require_model_generation() {
        return ModelHalfReport::failed(format!("模型生成门未开: {error}"));
    }
    let report = match model_half_inner(dbnum, frozen, sender).await {
        Ok(report) => report,
        Err(error) => ModelHalfReport::failed(format!("{error:#}")),
    };
    // 跑完即弃：`initialization_roots` / `produce_slices` 用的生成输入（`DbSet` + 池级元素缓存 + 整库属主表）
    // 留在进程级 `GENERATION_INPUT_CACHE` 里，键是本次冻结源的摘要——之后没有人再按这个键来取，
    // 却要占到进程退出（7997 上元素缓存一项约 720 MB）。切片手里的 `Arc` 此刻都已消费完，这里放掉的就是最后一份。
    let forgotten = crate::fast_model::e3d_model_service::forget_generation_input(dbnum);
    log::info!("dbnum={dbnum} 初始化模型半边结束，释放生成输入缓存 {forgotten} 条");
    report
}

async fn model_half_inner(
    dbnum: u32,
    frozen: &FrozenSource,
    sender: flume::Sender<WriteJob>,
) -> anyhow::Result<ModelHalfReport> {
    use crate::fast_model::e3d_model_service::E3dModelService;

    let service = E3dModelService::from_frozen_source(frozen).await?;
    // 根从文件枚举（ADR-077 D6）：模型面开工不等属性面。
    let roots = service.initialization_roots(dbnum)?;
    // 模型增量类型限定是消费门：名单外的根照常存在、只是本进程不生成它（不算失败、不留回执）。
    let (admitted, withheld): (Vec<GenerationRoot>, Vec<GenerationRoot>) = roots
        .into_iter()
        .partition(|root| crate::data_interface::model_type_scope::admits(&root.noun));
    if !withheld.is_empty() {
        println!(
            "dbnum={dbnum} 初始化：{} 个根按 model_incremental_types 暂扣不生成",
            withheld.len()
        );
    }
    let nouns = admitted
        .iter()
        .map(|root| {
            (
                crate::data_interface::generation_root::refno_to_e3d(root.root),
                root.noun.clone(),
            )
        })
        .collect::<HashMap<RefNo, String>>();
    let mut report = ModelHalfReport::default();
    if admitted.is_empty() {
        println!("dbnum={dbnum} 初始化：没有交付单元根可生成");
        return Ok(report);
    }
    // 冻计算器之前先把常驻房间模型对齐到此刻的文件版本（ADR-073 D12）：初始化的库多半就是刚出现 /
    // 刚前进的那个文件，它自己的房间与面板要进常驻才算得到。
    crate::fast_model::model_db_adapter::refresh_room_reference_before_task(dbnum).await;
    // 房间计算器任务开工冻一次（ADR-073 D1）；冻不出来 = 本批一根都不落（房间失败 = 该根失败）。
    let rooms = match crate::fast_model::model_db_adapter::frozen_room_calculator() {
        Ok(rooms) => rooms,
        Err(error) => {
            let reason = format!("room calculator unavailable for this task: {error:#}");
            for (root, noun) in &nouns {
                report.units.push(failed_unit(dbnum, *root, noun, &reason));
            }
            report.failure = Some(reason);
            return Ok(report);
        }
    };
    let root_strings = admitted
        .iter()
        .map(|root| root.root.to_pdms_str())
        .collect::<Vec<_>>();
    println!(
        "dbnum={dbnum} 初始化模型生成开始：交付单元根={}（并行生成 → 切片 → 模型 lane 直写）",
        root_strings.len()
    );
    let failure_policy = crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
    let cap = crate::fast_model::model_initial_persist::default_transaction_cap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let produce = async {
        let outcome = service
            .produce_slices(dbnum, &root_strings, failure_policy, &tx)
            .await;
        drop(tx);
        outcome
    };
    let consume = async {
        let mut report = ModelHalfReport::default();
        while let Some(batch) = rx.recv().await {
            let projection = service.project_slice_for_initialization(batch, failure_policy)?;
            let commits = projection
                .commits
                .into_iter()
                .map(|(root, commit, _)| (root, commit))
                .collect::<Vec<_>>();
            // 这一片的根照哪一版目录算出来的（回执里的 `ModelTarget.catalogue`）：整库初始化完、
            // 收尾盖 `model_sesno` 时一并盖成库级目录落款（ADR-079，取最旧那一版）。
            for (_, commit) in &commits {
                if let Some(target) = commit
                    .receipt
                    .as_ref()
                    .and_then(|(_, receipt)| receipt.model_target.as_ref())
                {
                    for file in &target.catalogue {
                        report
                            .catalogue_sessions
                            .entry(file.dbnum)
                            .and_modify(|kept| *kept = (*kept).min(file.session))
                            .or_insert(file.session);
                    }
                }
            }
            let rendered = crate::fast_model::model_initial_persist::render_slice(
                dbnum,
                &commits,
                rooms.as_ref(),
                cap,
            );
            for transaction in rendered.transactions {
                let label = transaction.label();
                for root in &transaction.roots {
                    report.units.push(generated_unit(dbnum, *root, &nouns));
                }
                report.transactions += 1;
                report.records += transaction.records;
                report.room_edges += transaction.room_edges;
                report.tree_boxes.extend(transaction.tree_boxes);
                report.refreshed.extend(transaction.refreshed);
                sender
                    .send_async(WriteJob::new(transaction.sql, label))
                    .await
                    .map_err(|_| anyhow::anyhow!("model lane closed before the slice was queued"))?;
            }
            for (root, reason) in projection
                .failures
                .into_iter()
                .chain(rendered.dead_letters)
            {
                log::error!("dbnum={dbnum} 初始化根 {root} 失败：{reason}");
                let noun = nouns.get(&root).cloned().unwrap_or_default();
                // 死信不再往库里写痕（`gen_root` 退役，ADR-079）：失败只进进程内账本
                // （收尾时随单元清单一起记），`model_sesno` 不盖就是它在库里的全部说法。
                report.units.push(failed_unit(dbnum, root, &noun, &reason));
            }
        }
        Ok::<_, anyhow::Error>(report)
    };
    let (produced, consumed) = futures::future::join(produce, consume).await;
    let mut report = consumed?;
    produced?;
    // 这一库的每个根都是这台计算器算的房间（ADR-073 D14）：整库都成时收尾那一笔把代号盖进
    // `model_room_generation`，冷重启后 `e3d.room.lookup` 靠它替没有 ACK 的根说话。
    report.room_generation = rooms
        .as_ref()
        .map(|calculator| calculator.resident_generation().to_owned());
    // 枚举到了、却既没成也没败的根（生产端提前停了）——如实记失败，别让它们静默消失。
    let seen = report
        .units
        .iter()
        .map(|unit| unit.root_refno.clone())
        .collect::<std::collections::HashSet<_>>();
    for (root, noun) in &nouns {
        let key = format!("{}/{}", root.word0, root.word1);
        if !seen.contains(&key) {
            report
                .units
                .push(failed_unit(dbnum, *root, noun, "root was neither generated nor reported"));
        }
    }
    drop(sender);
    Ok(report)
}

fn generated_unit(dbnum: u32, root: RefNo, nouns: &HashMap<RefNo, String>) -> ModelUnitResult {
    ModelUnitResult {
        dbnum,
        root_refno: format!("{}/{}", root.word0, root.word1),
        noun: nouns.get(&root).cloned().unwrap_or_default(),
        status: UnitGenStatus::Generated,
        attempts: 0,
        message: None,
        old_owner: None,
        new_owner: None,
        kind: UnitKind::Regen,
        transform_fallback_reason: None,
        unit_fallback_reason: None,
    }
}

fn failed_unit(dbnum: u32, root: RefNo, noun: &str, reason: &str) -> ModelUnitResult {
    ModelUnitResult {
        dbnum,
        root_refno: format!("{}/{}", root.word0, root.word1),
        noun: noun.to_owned(),
        status: UnitGenStatus::Failed,
        attempts: 1,
        message: Some(reason.to_owned()),
        old_owner: None,
        new_owner: None,
        kind: UnitKind::Regen,
        transform_fallback_reason: None,
        unit_fallback_reason: None,
    }
}

/// pe 行数与 WORL 根数（`pe_dbnum_noun` 整键索引，见 `common.surql` 注释）。
pub(crate) async fn baseline_counts(dbnum: u32) -> anyhow::Result<(usize, usize)> {
    #[derive(serde::Deserialize)]
    struct CountRow {
        count: usize,
    }
    async fn scalar_count(sql: String) -> anyhow::Result<usize> {
        let rows = aios_core::SUL_DB
            .query(sql)
            .await?
            .check()?
            .take::<Vec<CountRow>>(0)?;
        Ok(rows.first().map(|row| row.count).unwrap_or_default())
    }
    let pe_count = scalar_count(format!(
        "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL"
    ))
    .await?;
    let root_count = scalar_count(format!(
        "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} AND noun = 'WORL' GROUP ALL"
    ))
    .await?;
    Ok((pe_count, root_count))
}

#[cfg(test)]
mod tests {
    /// 代码行里提到 `needle`：注释行不算。
    fn code_lines_mentioning<'a>(source: &'a str, needle: &str) -> Vec<&'a str> {
        source
            .lines()
            .filter(|line| {
                let code = line.trim_start();
                !code.starts_with("//") && code.contains(needle)
            })
            .collect()
    }

    fn production() -> &'static str {
        include_str!("dbnum_initialize.rs")
            .split_once("#[cfg(test)]")
            .expect("tests follow")
            .0
    }

    /// ADR-077 D5：初始化编排不碰投影、不登记 write-behind；D6：不读 `pe` 枚举根。
    #[test]
    fn the_orchestration_touches_neither_the_projection_nor_the_pe_table_for_roots() {
        for forbidden in [
            "ModelMemoryStore",
            "ModelWriteBehind",
            "model_write_behind::",
            "load_baseline_nodes",
            "baseline_model_plan",
            "enqueue_model_watermark_write_behind",
            "finalize_baseline(",
        ] {
            assert!(
                code_lines_mentioning(production(), forbidden).is_empty(),
                "初始化编排不许出现 `{forbidden}`"
            );
        }
    }

    /// ADR-077 D7 / D8：先两 lane 都 `drain`，再裁决清库，再核对，最后才是收尾那一笔；
    /// 收尾一笔同时带 `data`、`closes_task: true`。
    #[test]
    fn drains_precede_the_verdict_and_the_single_closing_transaction() {
        let body = production()
            .split_once("pub(crate) async fn initialize(")
            .expect("initialize")
            .1
            .split_once("async fn wipe_and_bail<")
            .expect("wipe helper follows")
            .0;
        let drain = body
            .find("futures::future::join(attributes.drain(), model_lane.drain())")
            .expect("both lanes drain together");
        let wipe = body.find("wipe_and_bail(").expect("lane failure wipes");
        let counts = body.find("baseline_counts(dbnum)").expect("verification counts");
        let converge = body
            .find("converge_tree_after_persist(")
            .expect("tree convergence after the drains");
        let tail = body.find("commit_task_end_on(").expect("closing transaction");
        assert!(
            drain < wipe && wipe < counts && counts < converge && converge < tail,
            "顺序必须是 排空两 lane → 裁决清库 → 核对 → 空间树 → 收尾一笔"
        );
        assert_eq!(
            body.matches("commit_task_end_on(").count(),
            1,
            "收尾只有一笔"
        );
        assert!(
            body.contains("data: Some(TaskDataOutcome { confirmed_empty })")
                && body.contains("closes_task: true"),
            "收尾那一笔同时盖 applied、收写入标记"
        );
        assert!(
            body.find("mark_initialization_writing(").unwrap() < body.find("Lane::spawn(").unwrap(),
            "写入标记落在任何写者起来之前（ADR-072 W1）"
        );
    }

    /// 跑完即弃：模型半边一结束就把本 dbnum 的生成输入（`DbSet` + 元素缓存 + 属主表）从进程级缓存里放掉。
    /// 初始化用冻结源摘要当键，之后没人再取；不放就要占到进程退出，多库长驻按 dbnum 累加。
    #[test]
    fn the_model_half_forgets_its_generation_input_when_it_is_done() {
        let body = production()
            .split_once("async fn model_half(")
            .expect("model_half")
            .1
            .split_once("async fn model_half_inner(")
            .expect("inner follows")
            .0;
        let inner = body.find("model_half_inner(dbnum, frozen, sender)").expect("runs the inner half");
        let forget = body
            .find("forget_generation_input(dbnum)")
            .expect("model_half 结束时必须释放生成输入缓存");
        assert!(inner < forget, "先跑完模型半边，再放掉它的生成输入");
        assert!(
            code_lines_mentioning(
                production()
                    .split_once("async fn model_half_inner(")
                    .expect("inner")
                    .1,
                "forget_generation_input"
            )
            .is_empty(),
            "放缓存只在 model_half 一处，inner 里的切片还握着 Arc"
        );
    }

    /// 流那两半不拥有写者：`stream_frozen_into` / `produce_slices` 只往 sender 里送。
    #[test]
    fn the_halves_only_feed_the_lanes() {
        let source = production();
        for half in ["async fn data_half(", "async fn model_half_inner("] {
            // 函数体到第一行顶格 `}` 为止。按行走而不是找 `"\n}\n"`：编辑器把文件存成 CRLF
            // 时后者找不到，这条钉就会在文件一个字没改的情况下失败。
            let body = source
                .split_once(half)
                .expect(half)
                .1
                .lines()
                .take_while(|line| *line != "}")
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!body.is_empty(), "{half} 的函数体为空");
            for forbidden in ["Lane::spawn", "drain(", "wipe_after_failure(", "commit_task_end_on("] {
                assert!(
                    code_lines_mentioning(&body, forbidden).is_empty(),
                    "{half} 不许 `{forbidden}`"
                );
            }
        }
    }
}
