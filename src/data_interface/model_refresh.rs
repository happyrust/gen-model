//! ModelRefreshPolicy — 增量落库后的模型重生成执行端。
//!
//! 这里只负责「执行」，不负责「决策」：生成根由调用方选定，本模块把它们喂给
//! `E3dModelService`（[`ModelRefreshPolicy::generate_roots`]），另外提供按
//! `pe.deleted` 状态清理被删元素旧几何的入口
//! （[`ModelRefreshPolicy::cleanup_deleted_by_pe_state`]）。
//!
//! 三条生产链路各自选完根后汇入这里：自动路径 `IncrementPipeline::apply` →
//! `build_model_update_plan` → `model_update_pending::drain`；手动路径
//! `manual_update::generate_unit_model`；补偿路径 `side_effect_pending::drain`。
//! 「变更元素 → 生成根」的归一策略只有一份，在 `generation_root.rs`。

use aios_core::RefnoEnum;
use aios_core::pdms_types::*;
use anyhow::Context;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::e3d_model_service::E3dModelService;

#[derive(Debug, Clone)]
pub(crate) struct RootGenerationFailure {
    pub root: String,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TargetedGenerationReport {
    pub completed: Vec<String>,
    pub failures: Vec<RootGenerationFailure>,
    pub room_edges_written: usize,
    pub rows_upserted: usize,
    pub commits: usize,
    pub max_transaction_bytes: usize,
}

/// 把逐根失败并回调用方给的根序。持久层允许同批部分成功，因此不能因为 failures 非空
/// 就抹掉已经成功的根；调用方会把“不在 completed 也不在 failures”的根视为编排缺陷。
fn report_from_failures(
    roots: &[String],
    failures: Vec<RootGenerationFailure>,
) -> TargetedGenerationReport {
    let failed = failures
        .iter()
        .map(|failure| failure.root.as_str())
        .collect::<std::collections::HashSet<_>>();
    let completed = roots
        .iter()
        .filter(|root| !failed.contains(root.as_str()))
        .cloned()
        .collect();
    TargetedGenerationReport {
        completed,
        failures,
        ..Default::default()
    }
}

#[cfg(test)]
static FAIL_GENERATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn fail_generations_for_test(count: usize) {
    FAIL_GENERATIONS.store(count, std::sync::atomic::Ordering::SeqCst);
}

/// Deep module: one interface, swappable refresh adapters.
pub struct ModelRefreshPolicy;

impl ModelRefreshPolicy {
    /// Apply one already-frozen source session window through e3d-model's
    /// incremental API. The data pipeline owns window selection; this method
    /// only validates it and persists the resulting GeometryId delta.
    ///
    /// ADR-056 P2-1 改造对象：暂存窗口退役后它失去生产调用点（直写路径一直是
    /// `run_unit_worklist(…, None, …)` → 根级 `generate_roots`，D3）。P2 用它的
    /// `collect_window` + `plan_update` 半边做选根与凭证前移，`execute_plan` 半边
    /// （单元级落库，D3 已否）届时摘除；在那之前不删。
    #[allow(dead_code)]
    pub(crate) async fn apply_window(
        dbnum: u32,
        start_sesno: i32,
        end_sesno: i32,
    ) -> anyhow::Result<()> {
        let target_sesno = u32::try_from(end_sesno).context("invalid target session")?;
        let first_sesno = u32::try_from(start_sesno).context("invalid start session")?;
        let base_sesno = first_sesno.saturating_sub(1);
        // D1（ADR-056）：根级失败进 `model_update_pending` 重试账，不再有窗口级
        // `Required` 阻断——这就是今天直写路径一直在用的值。
        let failure_policy =
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
        crate::data_interface::batch_worker::set_active_task_stage("model_increment");
        let report = E3dModelService::from_current()
            .await?
            .apply_window(dbnum, base_sesno, target_sesno, failure_policy)
            .await?;
        if report.failed == 0 {
            Ok(())
        } else {
            anyhow::bail!(
                "e3d-model incremental generation reported {} failed geometry unit(s)",
                report.failed
            )
        }
    }

    /// Execute explicit, already-planned generation roots. The pending-work
    /// consumer owns root selection; this method owns only generator setup.
    pub(crate) async fn generate_roots(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<()> {
        let report = Self::generate_roots_report(mgr, roots).await?;
        if report.failures.is_empty() {
            Ok(())
        } else {
            let examples = report
                .failures
                .iter()
                .take(3)
                .map(|failure| format!("{}: {}", failure.root, failure.error))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(
                "e3d-model generation failed for {} root(s): {examples}",
                report.failures.len()
            )
        }
    }

    pub(crate) async fn generate_roots_report(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<TargetedGenerationReport> {
        Self::generate_roots_report_on_source(mgr, roots, None, None).await
    }

    /// 与 [`Self::generate_roots_report`] 同一条执行/持久化流水线，但设计文件严格钉在
    /// 调用方已经冻结的身份与会话。整库即时任务用它保证「枚举根的文件」与
    /// 「真正生成模型的文件」是同一份；不得在后台 worker 里退回 `from_current()`。
    pub(crate) async fn generate_roots_report_from_frozen_source(
        mgr: &AiosDBManager,
        roots: &[String],
        source: &crate::data_interface::frozen_source::FrozenSource,
        room_generation: Option<&str>,
    ) -> anyhow::Result<TargetedGenerationReport> {
        Self::generate_roots_report_on_source(mgr, roots, Some(source), room_generation).await
    }

    async fn generate_roots_report_on_source(
        _mgr: &AiosDBManager,
        roots: &[String],
        explicit_source: Option<&crate::data_interface::frozen_source::FrozenSource>,
        expected_room_generation: Option<&str>,
    ) -> anyhow::Result<TargetedGenerationReport> {
        if roots.is_empty() {
            return Ok(TargetedGenerationReport::default());
        }
        let dbnum = E3dModelService::dbnum_for_roots(roots).await?;
        // memory 读源作用域（plan 2026-09-06 §8 R2）不碰 durable 权威库，也不等 ingest
        // 初始化写者；发布目标另判：ingest fallback 只留投影，read-through 的 kv-mem
        // 是本进程正式 process store，仍走逐根模型行 + 房间边 + 回执事务。
        let memory_only = crate::data_interface::model_read_route::memory_only_for(dbnum);
        let publication_policy =
            crate::data_interface::model_read_route::publication_policy(memory_only);
        if !memory_only {
            crate::data_interface::initialization_phase::require_model_generation()?;
        }
        #[cfg(test)]
        if FAIL_GENERATIONS
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            anyhow::bail!("injected model generation failure");
        }
        // D1（ADR-056）：模型失败不阻断水位，根级失败进重试账；`Required` 只属于
        // 已退役的暂存窗口。
        let failure_policy =
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
        crate::data_interface::batch_worker::set_active_task_stage("model_generate");
        // ADR-071 D1：没有初始化 run、不等 ACK、不等别的写者——同一 dbnum 只有本任务一个写者，
        // 生成完就地把行写进库。冻结源在手（任务窗口内）就钉它那一版文件，否则取文件最新。
        let mut diagnostic = crate::process_diagnostics::Operation::begin("model.source.begin",
            format!("dbnum={dbnum} roots={roots:?} frozen={}", explicit_source.is_some()));
        let service = match explicit_source {
            Some(source) => {
                anyhow::ensure!(
                    source.dbnum() as u32 == dbnum,
                    "explicit frozen source dbnum {} cannot generate roots owned by dbnum {dbnum}",
                    source.dbnum()
                );
                E3dModelService::from_frozen_source(source).await?
            }
            None => match crate::data_interface::batch_worker::active_frozen_source()? {
                Some(source) => {
                    anyhow::ensure!(
                        source.dbnum() as u32 == dbnum,
                        "active file round dbnum {} cannot generate roots owned by dbnum {dbnum}",
                        source.dbnum()
                    );
                    E3dModelService::from_frozen_source(&source).await?
                }
                None => E3dModelService::from_current().await?,
            },
        };
        diagnostic.stage("model.geometry.begin", format!("dbnum={dbnum} roots={roots:?}"));
        let persisted = service
            .generate_and_persist_roots(dbnum, roots, failure_policy)
            .await.map_err(|error| {
                diagnostic.finish(format!("geometry_failed dbnum={dbnum}: {error:#}"));
                error
            })?;
        diagnostic.stage("model.geometry.done", format!("dbnum={dbnum} failed={} failures={:?}", persisted.failed, persisted.failed_roots));
        let generation_failures = persisted
            .failed_roots
            .iter()
            .map(|(root, error)| RootGenerationFailure {
                root: format!("{}/{}", root.word0, root.word1),
                error: error.clone(),
            })
            .collect::<Vec<_>>();
        let generation_failed_roots = generation_failures
            .iter()
            .map(|failure| failure.root.clone())
            .collect::<std::collections::HashSet<_>>();
        if persisted.failed == 0 || !generation_failures.is_empty() {
            // 读透形态（`data_face = read-through`）：新生成的每根在**发布模型行之前**先把
            // 子树（DFS 访问名单 ∪ 祖先链）的数据面终态写透进 `pe` / `ATT_*` / `pe_owner`
            // ——plant-ui 拿到 ensure 回执后读模型行时 `in.owner` 已经在。生成服务只报名单
            // （ADR-057：模型面不认识库），读文件用的 `DbSet` 与生成钉同一版会话。
            // durable spool 是发布门：落盘失败时保留内存记录供连续展示，但清掉根回执，
            // 让后续 ensure 必须重试发布。
            let read_through_set = if crate::options::data_face().is_read_through()
                && crate::options::store_mode().is_durable()
                && !persisted.visited_by_root.is_empty()
            {
                match Self::open_read_through_source(&service, dbnum, &persisted) {
                    Ok(opened) => Some(opened),
                    Err(error) => {
                        log::error!(
                            "read_through_degraded: dbnum {dbnum} 的读透源打不开，本批 {} 根不能建立 durable mirror job：{error:#}",
                            roots.len()
                        );
                        return Ok(TargetedGenerationReport {
                            completed: Vec::new(),
                            failures: roots
                                .iter()
                                .map(|root| RootGenerationFailure {
                                    root: root.clone(),
                                    error: format!("durable mirror source unavailable: {error:#}"),
                                })
                                .collect(),
                            ..Default::default()
                        });
                    }
                }
            } else {
                None
            };
            // CATA 依赖块（plan 2026-09-06 §3 D6 (c)）：摄入 + 落库的形态下，每根发布前把它子树
            // （e3d-model DFS `visited`）依赖的目录链元素 Fresh 入库——服务 ADR-056 N7 下 `pe` 零行的库
            // 走按需 ensure 时属性面板也有目录属性可读。独立于模型发布事务，失败只 warning、不拦发布
            // （ADR-056 D1 的对偶：依赖块也不拦模型）。memory 路由不摄入属性/目录依赖；
            // read-through process store 只接模型行、房间边与根回执。
            let dependency_source = if !memory_only
                && !crate::options::data_face().is_read_through()
                && crate::options::store_mode().is_durable()
                && !persisted.visited_by_root.is_empty()
            {
                match Self::open_read_through_source(&service, dbnum, &persisted) {
                    Ok((set, _)) => Some(set),
                    Err(error) => {
                        log::warn!(
                            "cata_dependency_skipped: dbnum {dbnum} 的设计文件打不开，本批 {} 根的目录依赖不入库：{error:#}",
                            roots.len()
                        );
                        None
                    }
                }
            } else {
                None
            };
            // 生成只写进程内投影（ADR-057：模型面不认识库），落库是这一步。ADR-071 D2 × ADR-073 D1：
            // 本批全部根的行**跨根合块**、一块一笔事务同步写进库（`persist_roots_batched`，d-932 保留
            // 512 根一组），每根的房间归属边紧跟它自己的行、同一块同一事务、模型在前；不登记进文件轮次
            // 等别人刷、不等 ACK；落不进去的那些根算失败——投影在内存里是对的，但 Plant UI 看不见的
            // 东西不能算完成。
            //
            // 房间计算器在这里**冻一次**（ADR-073 D1 硬约束：任务期间不换代），整批根共用。
            // 开关关闭时得到 None：模型照常持久化、不写新房间边；开关开启但常驻模型不可用
            // （在建 / 降级）时才把本批每根记成「房间算不出来」，下一次任务重生成。
            let rooms = if publication_policy.uses_process_store() {
                crate::fast_model::model_db_adapter::frozen_room_calculator().and_then(|rooms| {
                    if let Some(expected) = expected_room_generation {
                        let actual = rooms
                            .as_ref()
                            .map(|calculator| calculator.resident_generation());
                        anyhow::ensure!(
                            actual == Some(expected),
                            "room resident generation changed before model publication: expected {expected}, actual {actual:?}"
                        );
                    }
                    Ok(rooms)
                })
            } else {
                Ok(None)
            };
            let mut failures = generation_failures;
            let mut room_edges_written = 0usize;
            let mut rows_upserted = 0usize;
            let mut commits = 0usize;
            let mut max_transaction_bytes = 0usize;
            let mut publishable: Vec<(String, e3d_io::refno::RefNo)> = Vec::new();
            for root in roots {
                if generation_failed_roots.contains(root) {
                    continue;
                }
                let refno = crate::fast_model::e3d_model_service::parse_refno(root)?;
                if let (Some(set), Some(visited)) = (
                    dependency_source.as_ref(),
                    persisted.visited_by_root.get(&refno),
                ) {
                    let project = aios_core::get_db_option().project_name.clone();
                    match crate::data_interface::cata_dependency::persist_for_design_subtree(
                        &project, dbnum, set, visited,
                    )
                    .await
                    {
                        Ok(report) => {
                            if let Some(warning) = report.warning(dbnum) {
                                log::warn!("root {root}: {warning}");
                            }
                        }
                        Err(error) => log::warn!(
                            "cata_dependency_degraded: root {root} 的目录依赖入库失败（不拦发布）：{error:#}"
                        ),
                    }
                }
                if let Some((set, source)) = read_through_set.as_ref() {
                    if let Some(visited) = persisted.visited_by_root.get(&refno) {
                        if let Err(error) =
                            Self::write_through_root(set, source, refno, visited).await
                        {
                            crate::fast_model::model_memory_store::ModelMemoryStore::global()
                                .invalidate_receipt(
                                    crate::fast_model::model_memory_store::ProjectionScope::Current,
                                    refno,
                                );
                            failures.push(RootGenerationFailure {
                                root: root.clone(),
                                error: format!("durable mirror hand-off failed: {error:#}"),
                            });
                            continue;
                        }
                    } else {
                        crate::fast_model::model_memory_store::ModelMemoryStore::global()
                            .invalidate_receipt(
                                crate::fast_model::model_memory_store::ProjectionScope::Current,
                                refno,
                            );
                        failures.push(RootGenerationFailure {
                            root: root.clone(),
                            error: "generation report omitted the root's frozen source visit set"
                                .into(),
                        });
                        continue;
                    }
                    if let Err(error) = Self::spool_model_root(source, refno) {
                        crate::fast_model::model_memory_store::ModelMemoryStore::global()
                            .invalidate_receipt(
                                crate::fast_model::model_memory_store::ProjectionScope::Current,
                                refno,
                            );
                        failures.push(RootGenerationFailure {
                            root: root.clone(),
                            error: format!("durable model mirror hand-off failed: {error:#}"),
                        });
                        continue;
                    }
                }
                // ingest 初始化在飞时仍只留投影；read-through 的 kv-mem 是正式 process store，
                // 必须继续走逐根模型行 + 房间边 + 回执事务。
                if !publication_policy.uses_process_store() {
                    continue;
                }
                if !crate::options::data_face().is_read_through()
                    || !crate::options::store_mode().is_durable()
                {
                    publishable.push((root.clone(), refno));
                }
            }
            if !publishable.is_empty() {
                if let Some((round_dbnum, _)) =
                    crate::data_interface::batch_worker::active_data_window()
                {
                    anyhow::ensure!(
                        round_dbnum == dbnum,
                        "file round {round_dbnum} cannot publish model roots for dbnum {dbnum}"
                    );
                }
                let refnos = publishable
                    .iter()
                    .map(|(_, refno)| *refno)
                    .collect::<Vec<_>>();
                let started = std::time::Instant::now();
                diagnostic.stage("model.publish.begin", format!("dbnum={dbnum} roots={refnos:?}"));
                let batched = match &rooms {
                    Ok(rooms) => {
                        crate::fast_model::model_db_adapter::persist_roots_batched(
                            crate::fast_model::model_db_adapter::PersistTarget::process(),
                            crate::fast_model::model_memory_store::ModelMemoryStore::global(),
                            dbnum,
                            &refnos,
                            rooms.as_ref(),
                        )
                        .await?
                    }
                    Err(error) => {
                        // 房间失败 = 该根失败（ADR-073 D1）：计算器冻不出来，本批一根都不落库，
                        // 投影留在内存里、回执不盖，下一次任务重生成。
                        let text = format!("room calculator unavailable for this task: {error:#}");
                        crate::fast_model::model_db_adapter::BatchedPersist {
                            failed: refnos.iter().map(|refno| (*refno, text.clone())).collect(),
                            ..Default::default()
                        }
                    }
                };
                diagnostic.stage("model.publish.done", format!("dbnum={dbnum} failed={:?} elapsed_ms={}", batched.failed, started.elapsed().as_millis()));
                if let Some(rows) = batched.rows.as_ref() {
                    room_edges_written = batched.room_edges;
                    rows_upserted = rows.upserted;
                    commits = rows.commits;
                    max_transaction_bytes = batched.max_transaction_bytes;
                    println!(
                        "[model-persist] dbnum={dbnum} roots={} groups={} rows={} removed={} receipts={} room_edges={} commits={} max_transaction_bytes={} failed_roots={} elapsed_ms={}",
                        refnos.len(),
                        batched.groups,
                        rows.upserted,
                        rows.removed,
                        rows.receipts,
                        batched.room_edges,
                        rows.commits,
                        batched.max_transaction_bytes,
                        batched.failed.len(),
                        started.elapsed().as_millis()
                    );
                }
                let mut failed_by_refno = std::collections::BTreeMap::new();
                for (refno, error) in batched.failed {
                    failed_by_refno.insert(
                        crate::fast_model::model_memory_store::packed_refno(refno),
                        error,
                    );
                }
                for (root, refno) in &publishable {
                    if let Some(error) = failed_by_refno
                        .get(&crate::fast_model::model_memory_store::packed_refno(*refno))
                    {
                        failures.push(RootGenerationFailure {
                            root: root.clone(),
                            error: error.clone(),
                        });
                    }
                }
            }
            let mut report = report_from_failures(roots, failures);
            report.room_edges_written = room_edges_written;
            report.rows_upserted = rows_upserted;
            report.commits = commits;
            report.max_transaction_bytes = max_transaction_bytes;
            diagnostic.finish(format!("dbnum={dbnum} completed={:?} failures={:?} rows={} commits={} room_edges={}",
                report.completed, report.failures, report.rows_upserted, report.commits, report.room_edges_written));
            Ok(report)
        } else {
            diagnostic.finish(format!("dbnum={dbnum} geometry_failed={} roots={roots:?}", persisted.failed));
            Ok(TargetedGenerationReport {
                completed: Vec::new(),
                failures: roots
                    .iter()
                    .map(|root| RootGenerationFailure {
                        root: root.clone(),
                        error: format!(
                            "e3d-model reported {} failed geometry unit(s)",
                            persisted.failed
                        ),
                    })
                    .collect(),
                ..Default::default()
            })
        }
    }

    /// 读透写行要读的那份文件：与生成同一个 dbnum、同一版会话（`E3dPersistReport::source_sesno`）。
    /// 打开走 `e3d_collector::open_pinned_set`——基线读行用的就是它，两边渲出的行要逐字段相同，
    /// 先从同一个开法起。会话缺了不猜：拿文件最新顶上会让 `pe.sesno` 与模型凭证说的不是同一版文件。
    fn open_read_through_source(
        service: &E3dModelService,
        dbnum: u32,
        persisted: &crate::fast_model::e3d_mesh_store::E3dPersistReport,
    ) -> anyhow::Result<(
        std::sync::Arc<e3d_io::db_element::DbSet>,
        crate::data_interface::read_through::ReadThroughSource,
    )> {
        let sesno = persisted
            .source_sesno
            .context("generation report carries no source session; cannot write through")?;
        let (file, db_type) = service.source_pin(dbnum)?;
        let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()?;
        let set =
            crate::data_interface::e3d_collector::open_pinned_set(&schema, &file, &db_type, sesno)?;
        let source = crate::data_interface::read_through::ReadThroughSource { dbnum, file, sesno };
        Ok((set, source))
    }

    /// 一根的写透：`visited ∪ 祖先链` → durable mirror spool。文件属性读取本身不受
    /// spool 失败影响，但模型根只有在这份交接落盘之后才能取得发布回执。
    async fn write_through_root(
        set: &std::sync::Arc<e3d_io::db_element::DbSet>,
        source: &crate::data_interface::read_through::ReadThroughSource,
        root: e3d_io::refno::RefNo,
        visited: &[e3d_io::refno::RefNo],
    ) -> anyhow::Result<()> {
        use crate::data_interface::read_through;
        let refnos = match read_through::subtree_with_ancestors(set, root, visited) {
            Ok(refnos) => refnos,
            Err(error) => {
                log::error!(
                    "read_through_degraded: root {root} 的祖先链上溯失败，本根未写透：{error:#}"
                );
                return Err(error);
            }
        };
        match read_through::spool_write_through(set, source, &refnos) {
            Ok(receipt) => {
                log::info!(
                    "读透写透：根 {root} 写入 {} 个元素的数据面行（名单 {}，祖先在内；{} 块语句，{} ms，dbnum {} 会话 {}）",
                    receipt.written,
                    receipt.requested,
                    receipt.statements,
                    receipt.elapsed_ms,
                    receipt.dbnum,
                    receipt.sesno
                );
                if !receipt.missing.is_empty() {
                    log::warn!(
                        "读透写透：根 {root} 有 {} 个元素不在会话 {} 的索引里，未写：{}",
                        receipt.missing.len(),
                        receipt.sesno,
                        receipt
                            .missing
                            .iter()
                            .take(8)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if receipt.foreign > 0 {
                    log::warn!(
                        "读透写透：根 {root} 的名单里有 {} 个不属于 dbnum {} 的元素，按 ADR-058 S3 不写",
                        receipt.foreign,
                        receipt.dbnum
                    );
                }
                for warning in &receipt.warnings {
                    log::warn!("{warning}");
                }
            }
            Err(error) => {
                log::error!("read_through_degraded: root {root} 持久镜像任务落盘失败：{error:#}");
                return Err(error);
            }
        }
        Ok(())
    }

    /// Freeze the model rows produced for this root into the same durable spool
    /// as the element mirror.  The queued SQL is a full idempotent replacement;
    /// replay never reads the current source files or the current memory store.
    fn spool_model_root(
        source: &crate::data_interface::read_through::ReadThroughSource,
        root: e3d_io::refno::RefNo,
    ) -> anyhow::Result<()> {
        let option = aios_core::get_db_option();
        let transaction = crate::fast_model::model_db_adapter::render_spooled_root(
            crate::fast_model::model_memory_store::ModelMemoryStore::global(),
            root,
        )?;
        let spool = crate::data_interface::mirror_spool::MirrorSpool::for_process(
            &option.surreal_ns,
            &option.project_name,
            &option.mdb_name,
        )?;
        let snapshot_epoch =
            crate::data_interface::mdb_memory_store::get(&option.project_name, &option.mdb_name)
                .map(|snapshot| snapshot.epoch())
                .unwrap_or_else(|| u64::from(source.sesno));
        spool.persist(
            snapshot_epoch,
            vec![(source.dbnum, source.sesno)],
            format!("model:{root}"),
            vec![transaction],
        )?;
        Ok(())
    }

    /// F1/F3（T304）补偿路径专用：补偿只有 `changed_refnos`（不含操作类型），故按**当前
    /// `pe.deleted` 状态**反推被删元素并清理其旧几何（含子树）。删除态在 pe 持久，
    /// 可靠可重放。幂等；失败上抛（进补偿重试）。
    pub async fn cleanup_deleted_by_pe_state(refnos: &[RefU64]) -> anyhow::Result<()> {
        if refnos.is_empty() {
            return Ok(());
        }
        let mut deleted: Vec<RefnoEnum> = Vec::new();
        for chunk in refnos.chunks(200) {
            let keys = chunk
                .iter()
                .map(|r| RefnoEnum::from(*r).to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT VALUE id FROM [{keys}] WHERE deleted = true;");
            let mut response = aios_core::SUL_DB.query(sql).await?.check()?;
            for value in response.take::<Vec<surrealdb::sql::Thing>>(0)? {
                deleted.push(crate::data_interface::helper::pe_thing_to_refno(value)?);
            }
        }
        if deleted.is_empty() {
            return Ok(());
        }
        println!(
            "ModelRefreshPolicy/compensate: 清理 {} 个已删除元素的旧几何（含子树）",
            deleted.len()
        );
        crate::data_interface::helper::delete_inst_relate_subtree(&deleted, 300).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_root_does_not_erase_successful_peers_from_the_report() {
        let roots = vec!["24383/10".into(), "24383/20".into(), "24383/30".into()];
        let report = report_from_failures(
            &roots,
            vec![RootGenerationFailure {
                root: "24383/20".into(),
                error: "room membership failed".into(),
            }],
        );
        assert_eq!(report.completed, vec!["24383/10", "24383/30"]);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].root, "24383/20");
    }

    /// ADR-056 D8-A（spec 035 T121）：CATA 必需依赖门 `prepare_required_dependencies`
    /// 与它的停滞看门狗整个退场——模型面经 `E3dDbResolver` 从文件读 CATA，
    /// `cata_closure` 入 Surreal 只服务 `ref_rev` / UI（补偿队列，T126）。
    /// 任何一处把「CATA 行必须先落库」重新立成模型或水位的前置，这里就红。
    #[test]
    fn the_cata_dependency_gate_is_gone() {
        let source = include_str!("model_refresh.rs");
        let production = source
            .split_once("mod tests {")
            .expect("test module must follow production code")
            .0;
        for needle in [
            "fn prepare_required_dependencies",
            "await_required_dependency",
            "DEPENDENCY_STALL_TIMEOUT",
            "GeometryFailurePolicy::Required",
            "active_staging_writes",
        ] {
            assert!(
                !production.contains(needle),
                "model_refresh.rs 生产代码不得再含 `{needle}`（ADR-056 D1 / D8-A）"
            );
        }
    }

    fn generate_roots_report_body(source: &str) -> &str {
        source
            .split_once("pub(crate) async fn generate_roots_report(")
            .expect("generate_roots_report")
            .1
            .split_once("fn open_read_through_source(")
            .expect("generate_roots_report end")
            .0
    }

    /// 读透形态下，一根的写透排在它的模型行发布之前（计划 §3.2 第 5 条）：plant-ui 拿到
    /// ensure 回执后去读 `inst_relate.in.owner`，那时 `pe` 行必须已经在。反过来写就会有一段
    /// 「模型行在、pe 行还没到」的窗口，而隐式直管那条查询在这个窗口里整批解不出来。
    #[test]
    fn write_through_precedes_root_publication() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let body = generate_roots_report_body(&source);
        let write_through = body
            .find("Self::write_through_root(")
            .expect("generate_roots_report 必须在读透形态下调 write_through_root");
        let publish = body
            .find("model_db_adapter::persist_roots_batched(")
            .expect("generate_roots_report 必须调 persist_roots_batched");
        assert!(
            write_through < publish,
            "写透必须排在 persist_roots_batched 之前"
        );
        assert!(
            body.contains("crate::options::data_face().is_read_through()"),
            "写透只在读透形态下发生，判据必须是 data_face()"
        );
    }

    /// ADR-071 D1 / D2 / 实施约束 2：模型行由本任务**同步、跨根合块**写进库——不等初始化 run 的
    /// ACK、不等别的写者、不把根登记进文件轮次让别处稍后刷、不再一根一笔 `persist_root`。
    /// 任何一处把这些形状接回来，这里就红。
    #[test]
    fn model_rows_are_persisted_in_task_without_acks_or_deferred_publication() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let body = generate_roots_report_body(&source);
        for needle in [
            "wait_for_task_root_acks",
            "wait_for_dbnum(",
            "render_file_model_publish_root",
            "initialization_source",
            "model_db_adapter::persist_root(",
        ] {
            assert!(
                !body.contains(needle),
                "generate_roots_report 不得再含 `{needle}`（ADR-071）"
            );
        }
        assert_eq!(
            body.matches("model_db_adapter::persist_roots_batched(")
                .count(),
            1,
            "整批根只走一次跨根合块落库"
        );
    }

    #[test]
    fn read_through_process_store_does_not_skip_model_and_room_publication() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let body = generate_roots_report_body(&source);
        assert!(
            body.contains("model_read_route::publication_policy(memory_only)"),
            "read/write routing must be split before deciding whether to publish"
        );
        assert!(
            !body.contains("if memory_only {\n                    continue;\n                }"),
            "memory-routed read-through uses the volatile process store; it must not skip publication"
        );
        assert!(
            body.contains("model_db_adapter::persist_roots_batched("),
            "the volatile process store must reuse the same per-root model+room transaction"
        );
    }

    #[test]
    fn one_bad_generation_root_does_not_charge_every_sibling_in_the_dbnum() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let body = generate_roots_report_body(&source);
        assert!(
            body.contains("let generation_failures = persisted") && body.contains(".failed_roots")
        );
        assert!(body.contains("generation_failed_roots.contains(root)"));
        assert!(
            body.contains("persisted.failed == 0 || !generation_failures.is_empty()"),
            "identified bad roots must leave successful siblings on the publication path"
        );
    }

    #[test]
    fn explicit_frozen_source_precedes_task_context_and_current_files() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let body = generate_roots_report_body(&source);
        let explicit = body
            .find("match explicit_source")
            .expect("source selection must start with the explicit task snapshot");
        let task_context = body
            .find("active_frozen_source()")
            .expect("normal data tasks still consume their task-local source");
        let current = body
            .find("E3dModelService::from_current()")
            .expect("ordinary on-demand calls retain the current-source fallback");
        assert!(explicit < task_context && task_context < current);
        assert!(
            body.contains("E3dModelService::from_frozen_source(source).await?"),
            "the explicit source must be passed to the generator unchanged"
        );
    }

    /// 镜像数据库不是前置，但 durable spool 是发布门：源打不开、祖先链失败或
    /// spool 落盘失败都必须让这根失败，并撤掉提前写入内存的发布回执。
    #[test]
    fn durable_spool_failure_blocks_root_publication() {
        let source = include_str!("model_refresh.rs").replace("\r\n", "\n");
        let production = source.split_once("mod tests {").expect("tests").0;
        let helper = production
            .split_once("async fn write_through_root(")
            .expect("write_through_root")
            .1
            .split_once("pub async fn cleanup_deleted_by_pe_state(")
            .expect("write_through_root end")
            .0;
        assert!(helper.contains("anyhow::Result<()>"));
        assert!(helper.contains("return Err(error)"));
        assert!(helper.contains("read_through_degraded"), "失败要有名字");
        let body = generate_roots_report_body(&source);
        assert!(body.contains("invalidate_receipt("));
        assert!(body.contains("durable model mirror hand-off failed"));
        let opening = body
            .split_once("Self::open_read_through_source(")
            .expect("open_read_through_source call")
            .1;
        assert!(
            opening
                .trim_start()
                .starts_with("&service, dbnum, &persisted) {"),
            "读透源打开失败要走 match 分支记日志，不许 `?`：{}",
            &opening[..opening.len().min(80)]
        );
    }

    /// F1 · T106（live）：按 `pe.deleted` 反推的清理必须扫净整棵子树，且**只**动被删的那棵。
    ///
    /// 用 `4000000001/…` 保留段造图：`/10` 与其子 `/11` 都是软删墓碑并各自挂着
    /// `inst_relate → inst_info → geo_relate → inst_geo`；`/20` 是同批未删除的兄弟，
    /// 代表「同 ZONE 其它交付单元」。把三者一起传进去，验证删除集只按 `deleted = true`
    /// 过滤，且子树遍历能跨过 `pe_owner` 找到 `/11`。
    ///
    /// `inst_geo` **按设计幸存**：它是内容寻址的共享节点，级联删除的引用计数守卫
    /// 数不到还有谁指着那块几何，跟着删是跨生成根的数据损坏（见
    /// `render_cascade_delete` 的理由段）；回收归全库引用计数的后台 sweep
    /// （`live_inst_info_without_geo_relate_is_reclaimed` 钉的那条路径）。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let root = RefU64((4000000001u64 << 32) | 10);
        let sibling = RefU64((4000000001u64 << 32) | 20);
        let cleanup = "delete pe:4000000001_10, pe:4000000001_11, pe:4000000001_20, \
            inst_relate:4000000001_10, inst_relate:4000000001_11, inst_relate:4000000001_20, \
            inst_info:zz_f1_10, inst_info:zz_f1_11, inst_info:zz_f1_20, \
            geo_relate:zz_f1_10, geo_relate:zz_f1_11, geo_relate:zz_f1_20, \
            inst_geo:zz_f1_10, inst_geo:zz_f1_11, inst_geo:zz_f1_20;";
        let setup = format!(
            "{cleanup}
            create pe:4000000001_10 set deleted = true;
            create pe:4000000001_11 set deleted = true;
            create pe:4000000001_20 set deleted = false;
            relate pe:4000000001_11->pe_owner->pe:4000000001_10;
            create inst_info:zz_f1_10;
            create inst_info:zz_f1_11;
            create inst_info:zz_f1_20;
            create inst_geo:zz_f1_10;
            create inst_geo:zz_f1_11;
            create inst_geo:zz_f1_20;
            relate pe:4000000001_10->inst_relate:4000000001_10->inst_info:zz_f1_10;
            relate pe:4000000001_11->inst_relate:4000000001_11->inst_info:zz_f1_11;
            relate pe:4000000001_20->inst_relate:4000000001_20->inst_info:zz_f1_20;
            relate inst_info:zz_f1_10->geo_relate:zz_f1_10->inst_geo:zz_f1_10;
            relate inst_info:zz_f1_11->geo_relate:zz_f1_11->inst_geo:zz_f1_11;
            relate inst_info:zz_f1_20->geo_relate:zz_f1_20->inst_geo:zz_f1_20;"
        );
        aios_core::SUL_DB
            .query(setup)
            .await
            .expect("create deleted subtree fixture")
            .check()
            .expect("valid setup");

        ModelRefreshPolicy::cleanup_deleted_by_pe_state(&[root, sibling])
            .await
            .expect("cleanup deleted geometry by pe state");

        let mut response = aios_core::SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000001_10').id != none,
                    type::thing('inst_relate', '4000000001_11').id != none,
                    inst_info:zz_f1_10.id != none,
                    inst_info:zz_f1_11.id != none,
                    inst_geo:zz_f1_10.id != none,
                    inst_geo:zz_f1_11.id != none,
                    type::thing('inst_relate', '4000000001_20').id != none,
                    inst_info:zz_f1_20.id != none,
                    inst_geo:zz_f1_20.id != none
                ];",
            )
            .await
            .expect("query state after cleanup")
            .check()
            .expect("valid post-cleanup query");
        let state = response.take::<Vec<bool>>(0).expect("decode post state");

        aios_core::SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(
            state,
            vec![false, false, false, false, true, true, true, true, true],
            "被删子树的 inst_relate/inst_info 应清空（inst_geo 是共享内容寻址节点，\
             按设计留给后台 sweep 回收），未删除的兄弟必须原样保留"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates AIOS_GEOM_COVERAGE_ROOTS against configured SurrealDB"]
    async fn live_generate_roots_with_coverage_audit() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let roots = std::env::var("AIOS_GEOM_COVERAGE_ROOTS")
            .expect("set AIOS_GEOM_COVERAGE_ROOTS")
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(!roots.is_empty(), "no coverage roots");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        ModelRefreshPolicy::generate_roots(&manager, &roots)
            .await
            .expect("generate coverage roots");

        if let Ok(preserved) = std::env::var("AIOS_GEOM_PRESERVE_REFS") {
            for value in preserved
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                let refno = RefnoEnum::from(value);
                let sql = format!("return {}.id != none", refno.to_inst_relate_key());
                let mut response = aios_core::SUL_DB
                    .query(sql)
                    .await
                    .expect("query preserved model");
                assert!(
                    response
                        .take::<Option<bool>>(0)
                        .expect("decode preserved model")
                        .unwrap_or(false),
                    "regenerating {:?} deleted the untouched shared model {value}",
                    roots
                );
            }
        }
    }
}
