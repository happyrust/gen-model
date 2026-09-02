//! SideEffectCompensator — durable retry for post-watermark side effects.
//!
//! PE persist advances [`crate::data_interface::increment_pipeline::IncrementPipeline`]
//! watermarks and must not roll back. Model refresh / SYST derived sync can fail
//! afterward; this module records those jobs in Surreal and retries on drain.
//!
use aios_core::SUL_DB;
use aios_core::pdms_types::*;
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::increment_pipeline::IncrResult;
use crate::data_interface::tidb_manager::AiosDBManager;

const TABLE: &str = "incr_side_effect_pending";
const MAX_ATTEMPTS: u32 = 5;

fn parse_ref_rev_payload(changed_refnos: &[String]) -> anyhow::Result<Vec<RefnoEnum>> {
    anyhow::ensure!(
        !changed_refnos.is_empty(),
        "ref_rev_maintain changed_refnos 为空"
    );
    changed_refnos
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let value = raw.trim();
            anyhow::ensure!(!value.is_empty(), "changed_refnos[{index}] 为空");
            let parsed = value.parse::<RefU64>().map_err(|error| {
                anyhow::anyhow!("changed_refnos[{index}]={value:?} 非法: {error}")
            })?;
            anyhow::ensure!(parsed.0 != 0, "changed_refnos[{index}] 不得是零 refno");
            Ok(RefnoEnum::from(parsed))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectKind {
    SystDerived,
    /// 某个窗口的反向引用索引没维护上，需要按引用者定点重建（ADR-003）。
    RefRevMaintain,
    /// 水位提交后必须完成的空间树刷新/删除与文件持久化。
    SpatialReconcile,
    /// 某个 DESI 窗口触及的生成根，其 CATA 引用闭包要解析进 Surreal——**只**为
    /// `ref_rev` 反向索引与 UI 目录属性服务（ADR-056 D8-A）。模型面经 e3d-io 从文件读
    /// CATA，不等这一步；水位也不等。前身是 `prepare_required_dependencies` 那道
    /// 「必需依赖」门，随 kv-mem 暂存窗口一起退役。
    CataRefRev,
}

impl SideEffectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SystDerived => "syst_derived",
            Self::RefRevMaintain => "ref_rev_maintain",
            Self::SpatialReconcile => "spatial_reconcile",
            Self::CataRefRev => "cata_ref_rev",
        }
    }
}

/// [`SideEffectKind::CataRefRev`] 一次 drain 的收口口径（纯函数，供 drain 与单测共用）：
/// `missing > 0` **不是失败**——缺的是目录清单里没有 / 定位不到的元件，重试也不会长出来，
/// 记 warning 让人看见即可；只有 `preload_cata_for_roots` 本身报错才走 `mark_failed` 重试。
fn cata_ref_rev_summary(parsed: usize, missing: usize) -> (bool, String) {
    (
        missing > 0,
        format!("[cata_ref_rev] 目录闭包解析完成: parsed={parsed} missing={missing}"),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingJob {
    pub id: Thing,
    pub kind: String,
    pub dbnum: u32,
    pub end_sesno: i32,
    pub db_type: String,
    #[serde(default)]
    pub changed_refnos: Vec<String>,
    #[serde(default)]
    pub refresh_refnos: Vec<String>,
    #[serde(default)]
    pub remove_refnos: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Independent module: enqueue / complete / drain side-effect jobs.
#[derive(Debug, Default, Clone)]
pub struct SideEffectCompensator;

impl SideEffectCompensator {
    fn record_id(kind: SideEffectKind, dbnum: u32, end_sesno: i32) -> String {
        format!("{}:{}_{}_{}", TABLE, kind.as_str(), dbnum, end_sesno)
    }

    /// Render one durable post-commit spatial intent for the window tail transaction.
    pub(crate) fn render_spatial_reconcile_upsert(
        dbnum: u32,
        end_sesno: i32,
        refresh_refnos: &[String],
        remove_refnos: &[String],
    ) -> anyhow::Result<String> {
        let refresh = refresh_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let remove = remove_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(refno) = refresh.intersection(&remove).next() {
            anyhow::bail!("空间任务 refno {refno} 同时 refresh/remove");
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let refresh_json = serde_json::to_string(&refresh.into_iter().collect::<Vec<_>>())?;
        let remove_json = serde_json::to_string(&remove.into_iter().collect::<Vec<_>>())?;
        Ok(format!(
            "UPSERT {id} SET kind = 'spatial_reconcile', dbnum = {dbnum}, \
             end_sesno = {end_sesno}, db_type = 'DESI', changed_refnos = [], \
             refresh_refnos = {refresh_json}, remove_refnos = {remove_json}, \
             status = 'pending', attempts = attempts?:0, last_error = NONE, \
             updated_at = time::now();"
        ))
    }

    /// After PE+watermark success: enqueue only legacy non-model side effects.
    /// Model work is now persisted by `IncrementPipeline` before its watermark.
    ///
    /// One row per SYST file. A single row keyed by the FIRST success's `dbnum`
    /// paired with the LARGEST `end_sesno` across all of them used to stand in
    /// for the batch — with two SYST databases in one batch that pair described
    /// neither file, so the row could not be traced back to what produced it.
    pub async fn enqueue_from_incr(_mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        for success in Self::syst_successes(incr) {
            Self::upsert_pending(
                SideEffectKind::SystDerived,
                success.dbnum,
                success.end_sesno,
                &success.db_type,
                &[],
            )
            .await?;
        }
        Ok(())
    }

    fn syst_successes(
        incr: &IncrResult,
    ) -> impl Iterator<Item = &crate::data_interface::increment_pipeline::IncrFileSuccess> {
        incr.successes.iter().filter(|s| s.db_type == "SYST")
    }

    /// 单个 SYST 批次落库后登记一条派生同步任务（数据批次 worker 用）。
    ///
    /// 与 [`Self::enqueue_from_incr`] 是同一张表、同一种行 id——worker 一次只
    /// 执行一个文件，没有 `IncrResult` 可给，就按批次直接记。
    pub async fn enqueue_syst(dbnum: u32, end_sesno: i32, db_type: &str) -> anyhow::Result<()> {
        Self::upsert_pending(SideEffectKind::SystDerived, dbnum, end_sesno, db_type, &[]).await
    }

    /// 反向引用索引没维护上 → 记一条定点重建任务。
    ///
    /// 按 ADR-003，`ref_rev` 是「关联模型也要更新」的权威来源：缺一条边就是某个设计
    /// 实例静默不重生成。这件事过去只留一句 warning，而那句话既没人读、也没有任何东西
    /// 会自动触发重建，只能等有人手工跑全量。既然补偿队列就在旁边，让它走同一条重试
    /// 通道、同一个 `MAX_ATTEMPTS`。
    ///
    /// 带上引用者名单而不是会话区间：重建从库里的当前状态算（PE 主数据早于本步落库），
    /// 不必回头再解析一遍文件。
    pub async fn enqueue_ref_rev(
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        referrers: &[RefU64],
    ) -> anyhow::Result<()> {
        if referrers.is_empty() {
            return Ok(());
        }
        Self::upsert_pending(
            SideEffectKind::RefRevMaintain,
            dbnum,
            end_sesno,
            db_type,
            referrers,
        )
        .await
    }

    /// 数据窗口提交后登记一条「CATA 闭包解析进 Surreal」任务（ADR-056 D8-A，spec 035 T126）。
    ///
    /// 挂在 `apply_one` 的 `finalize_attempt` 之后：水位已经推进，这里**不拦模型、不拦水位**，
    /// 只是让 `ref_rev` 反向索引与 UI 目录属性迟一拍追上。与 [`Self::enqueue_ref_rev`]
    /// 同一张表、同一条 `MAX_ATTEMPTS` 重试通道；行 id 带 `end_sesno`，下一个窗口是新的一行，
    /// 上一窗口的死信不会挡住它。`AIOS_CATA_CLOSURE_MODE=off` 或没有生成根时不入队。
    pub async fn enqueue_cata_ref_rev(
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        roots: &[RefU64],
    ) -> anyhow::Result<()> {
        if roots.is_empty() || !crate::data_interface::cata_closure::cata_closure_enabled() {
            return Ok(());
        }
        Self::upsert_pending(SideEffectKind::CataRefRev, dbnum, end_sesno, db_type, roots).await
    }

    /// 入队语句的纯渲染，返回 `(记录 id, SurrealQL)`。
    ///
    /// `attempts = attempts?:0` 是这条语句的要害。同一个 `(kind, dbnum, end_sesno)`
    /// 会被重复 UPSERT——两个入队点按写法路由二选一，失败重试还会再来——保住已有的
    /// 重试计数才让 [`MAX_ATTEMPTS`] 有意义；清零的话一条永远失败的作业每来一个批次
    /// 就复活一次，死信这一档永远立不起来。
    fn render_upsert_pending(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        changed_refnos: &[RefU64],
    ) -> (String, String) {
        let id = Self::record_id(kind, dbnum, end_sesno);
        let refno_json = serde_json::to_string(
            &changed_refnos
                .iter()
                .map(|r| r.to_pdms_str())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into());
        let db_type = escape_surql_str(db_type);
        let sql = format!(
            "UPSERT {id} SET \
             kind = '{}', dbnum = {dbnum}, end_sesno = {end_sesno}, \
             db_type = '{db_type}', changed_refnos = {refno_json}, \
             status = 'pending', attempts = attempts?:0, \
             updated_at = time::now();",
            kind.as_str(),
        );
        (id, sql)
    }

    async fn upsert_pending(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        changed_refnos: &[RefU64],
    ) -> anyhow::Result<()> {
        let (id, sql) =
            Self::render_upsert_pending(kind, dbnum, end_sesno, db_type, changed_refnos);
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("enqueue {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("enqueue {id} statement failed: {e}"))?;
        println!("SideEffectCompensator: enqueued {id}");
        Ok(())
    }

    pub async fn mark_done(kind: SideEffectKind, dbnum: u32, end_sesno: i32) -> anyhow::Result<()> {
        let id = Self::record_id(kind, dbnum, end_sesno);
        let sql = format!(
            "UPDATE {id} SET status = 'done', last_error = NONE, updated_at = time::now();"
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("mark_done {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("mark_done {id} statement failed: {e}"))?;
        Ok(())
    }

    pub async fn mark_failed(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        err: &str,
    ) -> anyhow::Result<()> {
        let id = Self::record_id(kind, dbnum, end_sesno);
        // 错误信息常含 Windows 路径的反斜杠，需与单引号一起转义，否则会破坏
        // SurrealQL 字符串字面量导致本次 mark_failed 失败（attempts 不自增、
        // last_error 不落库）。复用 dbnum_state 的统一转义。
        let escaped = escape_surql_str(err);
        let sql = format!(
            "UPDATE {id} SET status = 'failed', attempts = (attempts?:0) + 1, \
             last_error = '{escaped}', updated_at = time::now();"
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("mark_failed {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("mark_failed {id} statement failed: {e}"))?;
        Ok(())
    }

    /// Complete every committed spatial intent before the worker admits another data batch.
    /// Spatial jobs deliberately ignore [`MAX_ATTEMPTS`]: abandoning one would publish a
    /// watermark whose global spatial state can never catch up.
    ///
    /// 公开入口负责取空间串行锁（锁序 `DATA_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    /// → GLOBAL_AABB_TREE`：worker 提交路径与派发门先持前者再进来，Python
    /// `spatial.reconcile` 直接进来——这把锁把此前不设防的 Python 并发收敛也串行化了）。
    pub async fn reconcile_spatial_pending(_mgr: &AiosDBManager) -> anyhow::Result<usize> {
        let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
        Self::reconcile_spatial_pending_locked().await
    }

    /// 已持空间串行锁的收敛主体（启动装载的立即重放、revalidator 复检等持锁方用）。
    pub(crate) async fn reconcile_spatial_pending_locked() -> anyhow::Result<usize> {
        let query = format!(
            "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
             AND status IN ['pending', 'failed'] ORDER BY updated_at ASC;"
        );
        let jobs: Vec<PendingJob> =
            crate::surreal_retry::retry_sul_db_transport("读取待收敛空间任务", || {
                let query = query.clone();
                async move {
                    let mut response = SUL_DB.query(query).await?.check()?;
                    Ok(response.take(0)?)
                }
            })
            .await?;
        if jobs.is_empty() {
            Self::promote_state_after_replay().await;
            return Ok(0);
        }

        let outcome = async {
            let mut deferred =
                crate::data_interface::staging::write_context::DeferredSpatialMutations::default();
            for job in &jobs {
                for raw in &job.refresh_refnos {
                    let refno = raw
                        .parse::<RefU64>()
                        .map(RefnoEnum::from)
                        .map_err(|_| anyhow::anyhow!("invalid spatial refresh refno {raw}"))?;
                    deferred.remove.remove(&refno);
                    deferred.refresh.insert(refno);
                }
                for raw in &job.remove_refnos {
                    let refno = raw
                        .parse::<RefU64>()
                        .map(RefnoEnum::from)
                        .map_err(|_| anyhow::anyhow!("invalid spatial remove refno {raw}"))?;
                    deferred.refresh.remove(&refno);
                    deferred.remove.insert(refno);
                }
            }
            crate::fast_model::aabb_tree::apply_deferred_spatial_mutations(deferred).await?;
            // 崩溃窗口 ②（一致性闭环方案 §8）：树已更新、快照未发布。pending 行
            // 还在（销账在发布之后），重启按 ReplayRequired 重放收敛。
            crate::fast_model::spatial_state::failpoint("spatial_after_tree_sync");
            // 只在树真的动过时落盘（脏位由 remove/refresh 两个变更入口维护），
            // 且落盘走发布门：树内容不可信的状态（DegradedBlocked 等）不许把
            // 残缺内容写过好文件——旧的「空树覆盖项目树文件」销毁性边界现在由
            // 状态门 + 脏位双重挡住。脏位门控同时省掉无变更收敛轮的整树序列化。
            crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty_locked()
                .await
                .map(|_| ())
        }
        .await;
        if let Err(error) = outcome {
            let message = format!("{error:#}");
            for job in &jobs {
                let _ = Self::mark_failed(
                    SideEffectKind::SpatialReconcile,
                    job.dbnum,
                    job.end_sesno,
                    &message,
                )
                .await;
            }
            return Err(error);
        }

        // 崩溃窗口 ④（一致性闭环方案 §8）：快照已发布、pending 未销账。重启时
        // 指纹相等 + pending 在场 → ReplayRequired，重放幂等追认后销账。
        crate::fast_model::spatial_state::failpoint("spatial_after_publish_before_ack");
        for job in &jobs {
            crate::surreal_retry::retry_sul_db_transport("确认空间收敛任务完成", || {
                Self::mark_done(SideEffectKind::SpatialReconcile, job.dbnum, job.end_sesno)
            })
            .await?;
        }
        Self::promote_state_after_replay().await;
        Ok(jobs.len())
    }

    /// 重放收敛成功后的状态晋升（方案 D5）：启动把状态置为 `ReplayRequired` 时，
    /// 一次成功的收敛意味着「树已追平全部已提交意图」，按树条目数晋升
    /// Ready / ReadyEmpty。其他状态不动——重建/复检各自管理自己的迁移，
    /// 常态 Ready 下的收敛也无需反复改写状态。
    async fn promote_state_after_replay() {
        use crate::fast_model::spatial_state::{self, SpatialTreeState};
        if spatial_state::current_state() == SpatialTreeState::ReplayRequired {
            let entries = aios_core::room::room::GLOBAL_AABB_TREE.read().await.size();
            spatial_state::set_ready_by_entries(entries);
            println!("空间意图重放收敛完成，空间树进入可消费状态（{entries} 条）");
        }
    }

    /// 还有没有已提交、但空间树尚未收敛的意图。
    ///
    /// 「空间树是不是陈旧的」这件事只有库里说了算：进程内的失败标志跨不过重启，
    /// 而未收敛的意图恰恰是崩溃后最该被认出来的那一类。
    pub async fn has_pending_spatial_work() -> anyhow::Result<bool> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status IN ['pending', 'failed'] LIMIT 1;"
            ))
            .await?
            .check()?;
        Ok(!response.take::<Vec<Thing>>(0)?.is_empty())
    }

    /// 待收敛空间意图的条数（/health `spatial_tree.pending` 的同源镜像；
    /// 权威口径仍是 `spatial_reconcile.pending`，两处同一查询谓词）。
    pub async fn count_pending_spatial_work() -> anyhow::Result<usize> {
        #[derive(Deserialize)]
        struct Row {
            count: i64,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT count() FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status IN ['pending', 'failed'] GROUP ALL;"
            ))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<Row>>(0)?
            .first()
            .map(|row| row.count.max(0) as usize)
            .unwrap_or(0))
    }

    pub async fn spatial_reconcile_status() -> anyhow::Result<serde_json::Value> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status IN ['pending', 'failed'] ORDER BY updated_at DESC;"
            ))
            .await?
            .check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        Ok(Self::render_spatial_reconcile_status(&jobs))
    }

    /// /health `spatial_reconcile` 的纯渲染半边。
    ///
    /// 四个键（pending / retries / last_error / stalled）是对外承诺（五缺陷方案
    /// W1.4）：运维按键取值，缺一个都是破坏性修改。形状由单测钉住，读库只负责
    /// 供货 `jobs`（按 updated_at DESC，`last_error` 因此取的是最近一条）。
    pub(crate) fn render_spatial_reconcile_status(jobs: &[PendingJob]) -> serde_json::Value {
        serde_json::json!({
            "pending": jobs.len(),
            "retries": jobs.iter().map(|job| job.attempts as u64).sum::<u64>(),
            "last_error": jobs.iter().find_map(|job| job.last_error.as_deref()),
            "stalled": jobs.iter().any(|job| job.attempts >= MAX_ATTEMPTS),
        })
    }

    /// 读库失败时 /health 的降级形状：与成功形状**同键**，`stalled` 保守置真。
    ///
    /// 这个降级此前手搓在 handler 里，与成功形状只靠肉眼保持一致——现在同源于
    /// 本模块，形状测试把两个分支一起钉住。
    pub fn spatial_reconcile_error_status(error: &anyhow::Error) -> serde_json::Value {
        serde_json::json!({
            "pending": 0,
            "retries": 0,
            "last_error": format!("读取空间收敛状态失败: {error:#}"),
            "stalled": true,
        })
    }

    /// 可 drain 的两类副作用（SystDerived / RefRevMaintain）的队列状态
    /// （/health `side_effect_pending`，P2-4）。
    ///
    /// 与 spatial 分开：spatial 无视 [`MAX_ATTEMPTS`]、由 [`Self::reconcile_spatial_pending`]
    /// 单独收敛，永远不会成死信；这两类走 [`Self::drain`]，attempts 到顶就被 drain 的
    /// 上限（[`MAX_ATTEMPTS`]）挡在候选集之外成为死信。此前 /health 只报 spatial 四键，
    /// 这两类死信在接口上无处可见、也没有复活出口（P2-4）。
    pub async fn side_effect_status() -> anyhow::Result<serde_json::Value> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind != 'spatial_reconcile' \
                 AND status IN ['pending', 'failed'] ORDER BY updated_at DESC;"
            ))
            .await?
            .check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        Ok(Self::render_side_effect_status(&jobs))
    }

    /// Dead non-spatial side effects are terminal for automatic retry but not
    /// successful initialization.  The model-ready barrier uses this probe so
    /// exhausted SYST/ref-reversal work remains visible as unsettled.
    pub async fn has_dead_work() -> anyhow::Result<bool> {
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {TABLE} \
                 WHERE kind != 'spatial_reconcile' AND status IN ['pending', 'failed'] \
                 AND (attempts?:0) >= {MAX_ATTEMPTS} LIMIT 1)) > 0;"
            ))
            .await?
            .check()?;
        Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
    }

    /// /health `side_effect_pending` 的纯渲染半边（形状由单测钉住）。
    ///
    /// `pending` = 仍在重试预算内（drain 会取）；`dead_letters` = attempts 到顶
    /// （drain 取不到，需 [`Self::revive_dead_letters`] 人工复活）；`by_kind` 给出
    /// 两类可 drain 副作用各自的计数。供货 `jobs` 按 updated_at DESC，`last_error`
    /// 因此取最近一条。`stalled` 只在出现死信时立起。
    pub(crate) fn render_side_effect_status(jobs: &[PendingJob]) -> serde_json::Value {
        let kind_counts = |kind: &str| -> serde_json::Value {
            let dead = jobs
                .iter()
                .filter(|job| job.kind == kind && job.attempts >= MAX_ATTEMPTS)
                .count();
            let pending = jobs
                .iter()
                .filter(|job| job.kind == kind && job.attempts < MAX_ATTEMPTS)
                .count();
            serde_json::json!({ "pending": pending, "dead_letters": dead })
        };
        let dead_letters = jobs
            .iter()
            .filter(|job| job.attempts >= MAX_ATTEMPTS)
            .count();
        let pending = jobs.len() - dead_letters;
        serde_json::json!({
            "pending": pending,
            "dead_letters": dead_letters,
            "retries": jobs.iter().map(|job| job.attempts as u64).sum::<u64>(),
            "by_kind": {
                "syst_derived": kind_counts(SideEffectKind::SystDerived.as_str()),
                "ref_rev_maintain": kind_counts(SideEffectKind::RefRevMaintain.as_str()),
                "cata_ref_rev": kind_counts(SideEffectKind::CataRefRev.as_str()),
            },
            "last_error": jobs.iter().find_map(|job| job.last_error.as_deref()),
            "stalled": dead_letters > 0,
        })
    }

    /// 读库失败时 /health 的降级形状：与成功形状**同键**，`stalled` 保守置真。
    ///
    /// 与 spatial 同一纪律（形状测试把成功与降级两个分支一起钉住），不在 handler
    /// 里手搓 JSON。
    pub fn side_effect_error_status(error: &anyhow::Error) -> serde_json::Value {
        serde_json::json!({
            "pending": 0,
            "dead_letters": 0,
            "retries": 0,
            "by_kind": {
                "syst_derived": { "pending": 0, "dead_letters": 0 },
                "ref_rev_maintain": { "pending": 0, "dead_letters": 0 },
                "cata_ref_rev": { "pending": 0, "dead_letters": 0 },
            },
            "last_error": format!("读取副作用补偿状态失败: {error:#}"),
            "stalled": true,
        })
    }

    async fn mark_abandoned(id: &Thing, reason: &str) -> anyhow::Result<()> {
        let sql = format!(
            "UPDATE {id} SET status = 'abandoned', last_error = '{}', updated_at = time::now();",
            escape_surql_str(reason)
        );
        SUL_DB.query(sql).await?.check()?;
        Ok(())
    }

    /// Replay pending/failed jobs (attempts < MAX_ATTEMPTS). Safe to call at init.
    ///
    /// Every job runs on its own: a failure — including a failure to write the
    /// job's own bookkeeping — is collected and reported at the end rather than
    /// aborting the round. Propagating it from inside the loop meant one flaky
    /// `UPDATE` skipped every job queued behind it, which is the same defect
    /// `model_update_pending::run_one` was rewritten to avoid. Returning `Err`
    /// once the round is over also matches that queue's contract, so a job that
    /// keeps failing surfaces in the caller's warnings instead of only in stdout.
    pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
        let sql = format!(
            "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
             AND kind != 'spatial_reconcile' \
             AND (attempts?:0) < {MAX_ATTEMPTS} ORDER BY updated_at ASC;"
        );
        let mut response = SUL_DB.query(sql).await?.check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        if jobs.is_empty() {
            return Ok(0);
        }

        println!(
            "SideEffectCompensator: draining {} pending side-effect job(s)",
            jobs.len()
        );
        let mut done = 0usize;
        let mut failures: Vec<String> = Vec::new();

        for job in jobs {
            let kind = match job.kind.as_str() {
                "syst_derived" => SideEffectKind::SystDerived,
                "ref_rev_maintain" => SideEffectKind::RefRevMaintain,
                "cata_ref_rev" => SideEffectKind::CataRefRev,
                other => {
                    let reason = format!("unsupported legacy side-effect kind: {other}");
                    if let Err(error) = Self::mark_abandoned(&job.id, &reason).await {
                        failures.push(format!("abandon {} failed: {error:#}", job.id));
                    } else {
                        println!("SideEffectCompensator: abandoned {} ({other})", job.id);
                    }
                    continue;
                }
            };

            let result = match kind {
                SideEffectKind::RefRevMaintain => {
                    match parse_ref_rev_payload(&job.changed_refnos) {
                        Ok(referrers) => {
                            crate::data_interface::manual_update::repair_reverse_index_for(
                                &referrers,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                }
                // 连不上 AiosDBMgr 是这个作业自己的失败，不是整轮的失败：
                // 这里必须留在 async 块里，`?` 一旦逃出去就会掐掉后面所有作业。
                SideEffectKind::SystDerived => {
                    async {
                        let aios_mgr =
                            aios_core::aios_db_mgr::aios_mgr::AiosDBMgr::init_from_db_option()
                                .await?;
                        crate::team_data::sync_team_data(&aios_mgr).await
                    }
                    .await
                }
                SideEffectKind::SpatialReconcile => {
                    unreachable!("spatial jobs are drained before dequeue")
                }
                // 只维护 ref_rev / UI 目录属性：`missing > 0` 记 warning 仍算完成
                // （见 `cata_ref_rev_summary`），只有解析本身报错才进重试。
                SideEffectKind::CataRefRev => match parse_ref_rev_payload(&job.changed_refnos) {
                    Ok(roots) => {
                        let roots = roots.iter().map(|root| root.refno()).collect::<Vec<_>>();
                        crate::data_interface::cata_closure::preload_cata_for_roots(
                            &mgr.db_option.project_name,
                            &roots,
                            Some(crate::data_interface::cata_closure::DependencyCacheContext {
                                source_dbnum: job.dbnum,
                                effective_end_sesno: job.end_sesno,
                            }),
                        )
                        .await
                        .map(|outcome| {
                            let (warn, summary) =
                                cata_ref_rev_summary(outcome.parsed, outcome.missing);
                            if warn {
                                log::warn!(
                                    "{summary} dbnum={} sesno={}（缺失不重试，只影响 ref_rev / UI 目录属性）",
                                    job.dbnum,
                                    job.end_sesno
                                );
                            }
                            println!("{summary}");
                        })
                    }
                    Err(error) => Err(error),
                },
            };

            let outcome = match result {
                Ok(()) => Self::mark_done(kind, job.dbnum, job.end_sesno).await,
                Err(error) => Err(error),
            };

            match outcome {
                Ok(()) => {
                    done += 1;
                    println!(
                        "SideEffectCompensator: done {:?} dbnum={} sesno={}",
                        kind, job.dbnum, job.end_sesno
                    );
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    println!(
                        "SideEffectCompensator: retry failed {:?} dbnum={}: {msg}",
                        kind, job.dbnum
                    );
                    let _ = Self::mark_failed(kind, job.dbnum, job.end_sesno, &msg).await;
                    failures.push(format!("{:?} dbnum={}: {msg}", kind, job.dbnum));
                }
            }
        }

        if !failures.is_empty() {
            anyhow::bail!(
                "{} side-effect job(s) failed after {done} completed: {}",
                failures.len(),
                failures.join("; ")
            );
        }
        Ok(done)
    }

    /// 人工复活 SystDerived / RefRevMaintain 死信（attempts 到顶被 [`Self::drain`]
    /// 的上限挡在候选集之外）——队列三出路（ADR-011 队列纪律）里的「可复活」那一条。
    ///
    /// spatial 无视上限、永远不会死信，故不在此复活。复活 = attempts 清零 + 清
    /// last_error + 回到 `pending`，下一轮 drain 重新取到；**唤醒 worker 由调用方负责**
    /// （HTTP 端点里 `BatchScheduler::wake()`——复活绕过入队通道，worker 的 Notify
    /// 没人碰过，不叫醒它这些行要等兜底轮询）。返回复活的行数。
    pub async fn revive_dead_letters() -> anyhow::Result<usize> {
        let mut response = SUL_DB
            .query(Self::render_revive_dead_letters())
            .await
            .map_err(|e| anyhow::anyhow!("revive side-effect dead letters failed: {e}"))?
            .check()
            .map_err(|e| {
                anyhow::anyhow!("revive side-effect dead letters statement failed: {e}")
            })?;
        let revived: Vec<PendingJob> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("decode revived side-effect dead letters failed: {e}"))?;
        Ok(revived.len())
    }

    /// 复活死信的 UPDATE（纯渲染）：只碰**可 drain 的两类**到顶死信（attempts >=
    /// [`MAX_ATTEMPTS`] 且 status ∈ pending/failed），spatial 不动（它无视上限、
    /// 自有重放语义）。
    fn render_revive_dead_letters() -> String {
        format!(
            "UPDATE {TABLE} SET attempts = 0, last_error = NONE, status = 'pending', \
             updated_at = time::now() WHERE kind != 'spatial_reconcile' \
             AND status IN ['pending', 'failed'] AND (attempts?:0) >= {MAX_ATTEMPTS} \
             RETURN AFTER;"
        )
    }

    /// 清扫已完成的非空间副作用行——队列三出路里的「可收口」终态清理。
    ///
    /// SystDerived / RefRevMaintain 成功后 [`Self::mark_done`] 置 `status = 'done'`
    /// 却从不删行，日积月累。由空闲轮每轮调一次（幂等：删完再删是空操作）。
    /// spatial 的 done 行**不在此清扫**——它的重放/销账语义（崩溃窗口 ②/④，pending
    /// 在场即 ReplayRequired）自成一套，误删会掩盖重启该重放的意图。返回删除行数。
    pub async fn sweep_done() -> anyhow::Result<usize> {
        let mut response = SUL_DB
            .query(Self::render_sweep_done())
            .await
            .map_err(|e| anyhow::anyhow!("sweep done side-effect rows failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("sweep done side-effect rows statement failed: {e}"))?;
        let removed: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("decode swept side-effect rows failed: {e}"))?;
        Ok(removed.len())
    }

    /// 清扫 done 行的 DELETE（纯渲染）：非空间、终态；`RETURN BEFORE` 回删掉的行以计数。
    fn render_sweep_done() -> String {
        format!(
            "DELETE {TABLE} WHERE kind != 'spatial_reconcile' AND status = 'done' RETURN BEFORE;"
        )
    }

    // `complete_syst_jobs` / `fail_syst_jobs` 随 `execute_incr_update` 退役：
    // 合流后 SYST 派生只走本补偿队列（enqueue_syst → drain 逐作业 mark_done /
    // mark_failed），不再有「先同步跑一遍、成了再回头销行」的旁路。
}

#[cfg(test)]
mod tests {
    use super::{SideEffectCompensator, SideEffectKind, parse_ref_rev_payload};

    #[test]
    fn ref_rev_payload_is_nonempty_and_all_or_nothing() {
        assert!(parse_ref_rev_payload(&[]).is_err());
        assert!(parse_ref_rev_payload(&["".into()]).is_err());
        assert!(parse_ref_rev_payload(&["24381/1".into(), "broken".into()]).is_err());
        assert_eq!(
            parse_ref_rev_payload(&["24381/1".into(), "24381/2".into()])
                .expect("valid payload")
                .len(),
            2
        );
    }

    /// SYST 派生入队：行 id 只由 `(kind, dbnum, end_sesno)` 决定，重复入队保住
    /// 已有 attempts，一个窗口右端一行。
    ///
    /// 这三条各自挡着一种失效。id 不稳定，两个入队点（直写路径与暂存提交尾）就
    /// 记成两行、TEAM 同步白跑一遍；attempts 被清零，一条永远失败的派生作业每来
    /// 一个批次复活一次，`MAX_ATTEMPTS` 与 /health 的死信计数一起失效；右端不进
    /// id，一次初始化里的多个 SYST 窗口会互相覆盖，只剩最后一个留下账。
    #[test]
    fn syst_enqueue_is_idempotent_per_window_and_keeps_its_retry_budget() {
        let (id, sql) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::SystDerived,
            8191,
            169,
            "SYST",
            &[],
        );
        assert_eq!(id, "incr_side_effect_pending:syst_derived_8191_169");
        assert!(sql.starts_with(&format!("UPSERT {id} SET")), "{sql}");
        assert!(
            sql.contains("attempts = attempts?:0"),
            "重复入队必须保住重试计数: {sql}"
        );
        assert!(sql.contains("status = 'pending'"), "{sql}");
        assert!(
            sql.contains("changed_refnos = []"),
            "SYST 派生按批次记账，不带引用者名单: {sql}"
        );

        let (next_window, _) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::SystDerived,
            8191,
            170,
            "SYST",
            &[],
        );
        assert_ne!(id, next_window, "换一个窗口右端就是另一行");
    }

    #[test]
    fn spatial_reconcile_row_is_deterministic_and_keeps_final_net_mutation() {
        let sql = SideEffectCompensator::render_spatial_reconcile_upsert(
            24381,
            42,
            &["16777216/1".to_string(), "16777216/2".to_string()],
            &["16777216/3".to_string()],
        )
        .expect("spatial reconcile SQL");

        assert!(sql.contains("incr_side_effect_pending:spatial_reconcile_24381_42"));
        assert!(sql.contains("kind = 'spatial_reconcile'"));
        assert!(sql.contains("refresh_refnos = [\"16777216/1\",\"16777216/2\"]"));
        assert!(sql.contains("remove_refnos = [\"16777216/3\"]"));
        assert_eq!(
            SideEffectKind::SpatialReconcile.as_str(),
            "spatial_reconcile"
        );
    }

    /// ADR-056 D8-A / spec 035 T126，出路一：入队行走同一张表、同一条 `MAX_ATTEMPTS`
    /// 通道——`attempts = attempts?:0` 保住计数，id 带 `end_sesno` 让下一个窗口是新的一行
    /// （上一窗口的死信挡不住它），payload 是生成根。
    #[test]
    fn cata_ref_rev_upsert_carries_roots_and_keeps_the_retry_budget() {
        use super::TABLE;
        use aios_core::pdms_types::RefU64;

        let roots = [RefU64((8000u64 << 32) | 10), RefU64((8000u64 << 32) | 11)];
        let (id, sql) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::CataRefRev,
            8000,
            26,
            "DESI",
            &roots,
        );
        assert_eq!(id, format!("{TABLE}:cata_ref_rev_8000_26"));
        assert!(sql.contains("kind = 'cata_ref_rev'"), "{sql}");
        assert!(sql.contains("attempts = attempts?:0"), "{sql}");
        assert!(
            sql.contains("\"8000/10\"") && sql.contains("\"8000/11\""),
            "{sql}"
        );
        let (next_window, _) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::CataRefRev,
            8000,
            27,
            "DESI",
            &roots,
        );
        assert_ne!(id, next_window, "新窗口必须是新的一行");
    }

    /// 出路二：drain 的候选过滤只挡 spatial 与到顶死信，`cata_ref_rev` 在候选集里且有
    /// 自己的派发臂——否则这一行会被当成「unsupported legacy kind」直接 abandoned。
    #[test]
    fn drain_dispatches_cata_ref_rev_instead_of_abandoning_it() {
        let source = include_str!("side_effect_pending.rs");
        let drain = source
            .split_once("pub async fn drain(")
            .expect("drain must exist")
            .1
            .split_once("pub async fn revive_dead_letters(")
            .expect("revive must follow drain")
            .0;
        assert!(drain.contains("kind != 'spatial_reconcile'"), "{drain}");
        assert!(!drain.contains("kind != 'cata_ref_rev'"), "{drain}");
        assert!(
            drain.contains("\"cata_ref_rev\" => SideEffectKind::CataRefRev"),
            "drain 必须认得 cata_ref_rev"
        );
        assert!(
            drain.contains("SideEffectKind::CataRefRev => match parse_ref_rev_payload"),
            "cata_ref_rev 必须有自己的派发臂"
        );
    }

    /// 出路三：`missing > 0` 只是 warning，作业仍按成功收口（不会进 `mark_failed`、不会
    /// 成死信）——缺的元件重试也不会长出来，D8-A 说它不拦模型、不拦水位。
    #[test]
    fn cata_ref_rev_missing_is_a_warning_not_a_failure() {
        let (warn, summary) = super::cata_ref_rev_summary(12, 0);
        assert!(!warn);
        assert!(summary.contains("parsed=12 missing=0"), "{summary}");
        let (warn, summary) = super::cata_ref_rev_summary(10, 2);
        assert!(warn, "缺失要响");
        assert!(summary.contains("missing=2"), "{summary}");
        let status = SideEffectCompensator::render_side_effect_status(&[]);
        assert!(
            status["by_kind"]["cata_ref_rev"].is_object(),
            "/health by_kind 必须报 cata_ref_rev：{status}"
        );
    }

    #[test]
    fn spatial_reconcile_rejects_conflicting_refno() {
        let duplicate = "16777216/1".to_string();
        let error = SideEffectCompensator::render_spatial_reconcile_upsert(
            24381,
            42,
            std::slice::from_ref(&duplicate),
            std::slice::from_ref(&duplicate),
        )
        .expect_err("same refno cannot be refreshed and removed");
        assert!(error.to_string().contains("同时 refresh/remove"));
    }

    /// /health `spatial_reconcile` 的四键契约（台账缺口 G-02，W1.4 验收）。
    ///
    /// 成功与读库降级两个分支必须同键；`stalled` 只在重试预算打满时立起；
    /// `last_error` 取最近一条（查询按 updated_at DESC 供货，渲染取第一个非空）。
    #[test]
    fn spatial_reconcile_status_keeps_its_four_key_shape_in_both_branches() {
        use super::{MAX_ATTEMPTS, PendingJob, TABLE};

        let keys = ["pending", "retries", "last_error", "stalled"];
        let empty = SideEffectCompensator::render_spatial_reconcile_status(&[]);
        let object = empty.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {empty}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {empty}");
        }
        assert_eq!(empty["pending"], 0);
        assert_eq!(empty["retries"], 0);
        assert_eq!(empty["last_error"], serde_json::Value::Null);
        assert_eq!(empty["stalled"], false);

        let job = |attempts: u32, last_error: Option<&str>| PendingJob {
            id: surrealdb::sql::Thing::from((TABLE, "spatial_reconcile_8000_26")),
            kind: "spatial_reconcile".into(),
            dbnum: 8000,
            end_sesno: 26,
            db_type: "DESI".into(),
            changed_refnos: vec![],
            refresh_refnos: vec![],
            remove_refnos: vec![],
            status: "failed".into(),
            attempts,
            last_error: last_error.map(str::to_owned),
        };
        let stalled = SideEffectCompensator::render_spatial_reconcile_status(&[
            job(2, Some("空间树落盘失败")),
            job(MAX_ATTEMPTS, None),
        ]);
        assert_eq!(stalled["pending"], 2);
        assert_eq!(stalled["retries"], u64::from(2 + MAX_ATTEMPTS));
        assert_eq!(stalled["last_error"], "空间树落盘失败");
        assert_eq!(
            stalled["stalled"], true,
            "重试打满必须报 stalled: {stalled}"
        );

        let retrying = SideEffectCompensator::render_spatial_reconcile_status(&[job(1, None)]);
        assert_eq!(
            retrying["stalled"], false,
            "预算未打满不算 stalled: {retrying}"
        );

        let degraded =
            SideEffectCompensator::spatial_reconcile_error_status(&anyhow::anyhow!("boom"));
        let degraded_object = degraded.as_object().expect("降级形状必须是对象");
        assert_eq!(
            degraded_object.len(),
            keys.len(),
            "降级分支不许缩键: {degraded}"
        );
        for key in keys {
            assert!(degraded_object.contains_key(key), "缺键 {key}: {degraded}");
        }
        assert_eq!(degraded["stalled"], true);
        assert!(
            degraded["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("boom")
        );
    }

    /// 收敛只许在树真的动过时落盘，且全程持空间串行锁。
    ///
    /// 无条件 `persist_aabb_tree()` 的销毁性边界：树加载失败以空树启动的进程里，
    /// 删除清理照样寄存 remove 意图、尾事务照写 spatial 行——收敛轮对空树摘除
    /// 零条之后，会把**空树**序列化覆盖 `accel_tree_{project}.bin`，销毁上一次
    /// 攒下的全量成果。脏位门控 + 快照发布门（状态机）双重挡住这条路。真实落盘
    /// 写 cwd 文件，单测不能实跑，只能钉源码。
    #[test]
    fn reconcile_persists_only_a_mutated_tree() {
        let source = include_str!("side_effect_pending.rs");
        let wrapper = source
            .split_once("pub async fn reconcile_spatial_pending(")
            .expect("reconcile_spatial_pending must exist")
            .1
            .split_once("pub(crate) async fn reconcile_spatial_pending_locked(")
            .expect("locked variant must follow")
            .0;
        assert!(
            wrapper.contains("lock_spatial_serial().await"),
            "公开收敛入口必须先取空间串行锁: {wrapper}"
        );
        let body = source
            .split_once("pub(crate) async fn reconcile_spatial_pending_locked(")
            .expect("reconcile body must exist")
            .1
            .split_once("pub async fn has_pending_spatial_work(")
            .expect("reconcile 之后是 pending 探针")
            .0;
        assert!(
            body.contains("persist_aabb_tree_if_dirty_locked("),
            "收敛必须走脏位门控的持锁落盘: {body}"
        );
        assert!(
            !body.contains(concat!("persist_aabb_tree", "()")),
            "无条件全量落盘会让空树覆盖项目树文件: {body}"
        );
        // 状态晋升必须发生在销账（mark_done）之后：pending 还没销就把消费者放行，
        // 门禁窗口内的房间轮会拿「已知陈旧」的树改写归属。
        let done_at = body.rfind("mark_done").expect("必须销账");
        let promote_at = body
            .rfind("promote_state_after_replay()")
            .expect("成功路径必须晋升状态");
        assert!(done_at < promote_at, "状态晋升必须在销账之后: {body}");
    }

    /// P2-4：/health `side_effect_pending` 必须报出 SystDerived / RefRevMaintain 的
    /// 死信计数，且成功与读库降级两个分支同键。
    ///
    /// 此前 /health 只报 spatial 四键，这两类走 drain 的副作用一旦 attempts 到顶就
    /// 被 drain 上限挡在候选集外成死信，接口上无处可见。这条钉住新增的可观测口径：
    /// 回退（删 dead_letters/by_kind 键、或把 spatial 混进来）即红。
    #[test]
    fn side_effect_status_exposes_dead_letters_and_keeps_its_shape() {
        use super::{MAX_ATTEMPTS, PendingJob, TABLE};

        let keys = [
            "pending",
            "dead_letters",
            "retries",
            "by_kind",
            "last_error",
            "stalled",
        ];
        let empty = SideEffectCompensator::render_side_effect_status(&[]);
        let object = empty.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {empty}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {empty}");
        }
        assert_eq!(empty["dead_letters"], 0);
        assert_eq!(empty["stalled"], false);

        let job = |kind: &str, attempts: u32, last_error: Option<&str>| PendingJob {
            id: surrealdb::sql::Thing::from((TABLE, format!("{kind}_8000_26").as_str())),
            kind: kind.into(),
            dbnum: 8000,
            end_sesno: 26,
            db_type: "SYST".into(),
            changed_refnos: vec![],
            refresh_refnos: vec![],
            remove_refnos: vec![],
            status: "failed".into(),
            attempts,
            last_error: last_error.map(str::to_owned),
        };
        // syst_derived 到顶 = 死信；ref_rev_maintain 还在预算内 = pending。
        let status = SideEffectCompensator::render_side_effect_status(&[
            job("syst_derived", MAX_ATTEMPTS, Some("SYST 同步失败")),
            job("ref_rev_maintain", 2, None),
        ]);
        assert_eq!(
            status["dead_letters"], 1,
            "到顶的 syst_derived 必须计死信: {status}"
        );
        assert_eq!(status["pending"], 1, "预算内的行仍是 pending: {status}");
        assert_eq!(status["retries"], u64::from(MAX_ATTEMPTS + 2));
        assert_eq!(status["stalled"], true, "有死信必须 stalled: {status}");
        assert_eq!(status["last_error"], "SYST 同步失败");
        assert_eq!(status["by_kind"]["syst_derived"]["dead_letters"], 1);
        assert_eq!(status["by_kind"]["syst_derived"]["pending"], 0);
        assert_eq!(status["by_kind"]["ref_rev_maintain"]["dead_letters"], 0);
        assert_eq!(status["by_kind"]["ref_rev_maintain"]["pending"], 1);

        // 读库降级分支必须同键，stalled 保守置真。
        let degraded = SideEffectCompensator::side_effect_error_status(&anyhow::anyhow!("boom"));
        let degraded_object = degraded.as_object().expect("降级形状必须是对象");
        assert_eq!(
            degraded_object.len(),
            keys.len(),
            "降级分支不许缩键: {degraded}"
        );
        for key in keys {
            assert!(degraded_object.contains_key(key), "缺键 {key}: {degraded}");
        }
        assert_eq!(degraded["stalled"], true);
        assert!(
            degraded["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("boom")
        );
    }

    #[test]
    fn side_effect_dead_letters_remain_unsettled_for_initialization() {
        let source = include_str!("side_effect_pending.rs");
        let body = source
            .split_once("pub async fn has_dead_work()")
            .expect("dead-work probe exists")
            .1
            .split_once("/// /health")
            .expect("dead-work probe end exists")
            .0;
        assert!(body.contains("kind != 'spatial_reconcile'"));
        assert!(body.contains("(attempts?:0) >= {MAX_ATTEMPTS}"));
    }

    /// P2-4「可复活」：复活只碰**可 drain 两类**的到顶死信，把 attempts 清零回到
    /// pending，并放过 spatial（它无视上限、自有重放语义）。
    #[test]
    fn reviving_dead_letters_resets_attempts_and_spares_spatial() {
        use super::MAX_ATTEMPTS;
        let sql = SideEffectCompensator::render_revive_dead_letters();
        assert!(sql.contains("attempts = 0"), "复活必须清零 attempts: {sql}");
        assert!(
            sql.contains("status = 'pending'"),
            "复活必须回到 pending: {sql}"
        );
        assert!(
            sql.contains("last_error = NONE"),
            "复活必须清 last_error: {sql}"
        );
        assert!(
            sql.contains("kind != 'spatial_reconcile'"),
            "spatial 不走上限、不在复活范围: {sql}"
        );
        assert!(
            sql.contains(&format!("(attempts?:0) >= {MAX_ATTEMPTS}")),
            "只复活到顶死信，不误动仍在重试预算内的行: {sql}"
        );
    }

    /// P2-4「可收口」：done 行清扫只删非空间的终态行，且幂等。
    ///
    /// spatial 的 done 行不在此清扫——pending 在场是它崩溃重放的信号，混进来会误删。
    #[test]
    fn sweeping_done_removes_only_terminal_non_spatial_rows() {
        let sql = SideEffectCompensator::render_sweep_done();
        assert!(sql.starts_with("DELETE "), "必须是删除语句: {sql}");
        assert!(sql.contains("status = 'done'"), "只删终态行: {sql}");
        assert!(
            sql.contains("kind != 'spatial_reconcile'"),
            "空间收敛的 done 行有自己的重放/销账语义，不在此清扫: {sql}"
        );
    }
}
