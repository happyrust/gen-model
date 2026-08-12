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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectKind {
    SystDerived,
    /// 某个窗口的反向引用索引没维护上，需要按引用者定点重建（ADR-003）。
    RefRevMaintain,
    /// 水位提交后必须完成的空间树刷新/删除与文件持久化。
    SpatialReconcile,
}

impl SideEffectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SystDerived => "syst_derived",
            Self::RefRevMaintain => "ref_rev_maintain",
            Self::SpatialReconcile => "spatial_reconcile",
        }
    }
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

    async fn upsert_pending(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        changed_refnos: &[RefU64],
    ) -> anyhow::Result<()> {
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
    pub async fn reconcile_spatial_pending(_mgr: &AiosDBManager) -> anyhow::Result<usize> {
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
            // 只在树真的动过时落盘（脏位由 remove/refresh 两个变更入口维护）。
            // 无条件全量序列化有一个销毁性的边界：树加载失败以空树启动的进程若在
            // 无变更收敛轮里照样序列化，会把**空树**写过 `accel_tree_{project}.bin`，
            // 覆盖上一次攒下的全量成果。脏位门控同时省掉无变更收敛轮的整树序列化。
            crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty()
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

        for job in &jobs {
            crate::surreal_retry::retry_sul_db_transport("确认空间收敛任务完成", || {
                Self::mark_done(SideEffectKind::SpatialReconcile, job.dbnum, job.end_sesno)
            })
            .await?;
        }
        Ok(jobs.len())
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
                    let referrers: Vec<RefnoEnum> = job
                        .changed_refnos
                        .iter()
                        .filter_map(|refno| refno.parse::<RefU64>().ok())
                        .map(RefnoEnum::from)
                        .collect();
                    crate::data_interface::manual_update::repair_reverse_index_for(&referrers).await
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

    // `complete_syst_jobs` / `fail_syst_jobs` 随 `execute_incr_update` 退役：
    // 合流后 SYST 派生只走本补偿队列（enqueue_syst → drain 逐作业 mark_done /
    // mark_failed），不再有「先同步跑一遍、成了再回头销行」的旁路。
}

#[cfg(test)]
mod tests {
    use super::{SideEffectCompensator, SideEffectKind};

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
        assert_eq!(stalled["stalled"], true, "重试打满必须报 stalled: {stalled}");

        let retrying = SideEffectCompensator::render_spatial_reconcile_status(&[job(1, None)]);
        assert_eq!(retrying["stalled"], false, "预算未打满不算 stalled: {retrying}");

        let degraded =
            SideEffectCompensator::spatial_reconcile_error_status(&anyhow::anyhow!("boom"));
        let degraded_object = degraded.as_object().expect("降级形状必须是对象");
        assert_eq!(degraded_object.len(), keys.len(), "降级分支不许缩键: {degraded}");
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

    /// 收敛只许在树真的动过时落盘。
    ///
    /// 无条件 `persist_aabb_tree()` 的销毁性边界：树加载失败以空树启动的进程里，
    /// 删除清理照样寄存 remove 意图、尾事务照写 spatial 行——收敛轮对空树摘除
    /// 零条之后，会把**空树**序列化覆盖 `accel_tree_{project}.bin`，销毁上一次
    /// 攒下的全量成果。脏位门控的 `persist_aabb_tree_if_dirty` 在两个变更入口
    /// （摘除 > 0、刷新换过条目）之外不落盘，正好挡住这条路。真实落盘写 cwd
    /// 文件，单测不能实跑，只能钉源码。
    #[test]
    fn reconcile_persists_only_a_mutated_tree() {
        let source = include_str!("side_effect_pending.rs");
        let body = source
            .split_once("pub async fn reconcile_spatial_pending(")
            .expect("reconcile_spatial_pending must exist")
            .1
            .split_once("pub async fn has_pending_spatial_work(")
            .expect("reconcile 之后是 pending 探针")
            .0;
        assert!(
            body.contains("persist_aabb_tree_if_dirty("),
            "收敛必须走脏位门控的落盘: {body}"
        );
        assert!(
            !body.contains(concat!("persist_aabb_tree", "()")),
            "无条件全量落盘会让空树覆盖项目树文件: {body}"
        );
    }
}
