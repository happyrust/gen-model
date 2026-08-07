//! StagedExecutor：暂存执行 + 语句日志 + 分块写回（ADR-017 §3/§4，开发方案 T0.2）。
//!
//! 一个提交单元持有一个执行器：窗口计算期间所有持久层写语句改从这里过——
//! 按 [`ExecMode`] 决定「在暂存库生效」与「进语句日志」两件事各自做不做；
//! 写回 = 把日志按原序、按 [`TX_CHUNK`] 分块事务重放到持久层，尾事务收口由
//! 调用方渲染（水位推进、attempts 清除、pending 收口——T1.3）。
//!
//! 恢复语义（ADR-017 §4）：journal 只活在内存。写回失败且进程存活 → 同一份
//! journal 整体重试（语句 ReplaySafe ⇒ 幂等收敛）；进程崩溃 → journal 消失，
//! 唯一路径是按水位整窗口重算。两条路径由构造互斥。

use std::sync::Arc;

use anyhow::{Context, bail};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::replay_safe;
use super::resources::{ResourceBand, ResourceGauge};
use crate::data_interface::increment_pipeline::wrap_in_transaction;
use crate::surreal_retry::execute_surreal_checked_on;

/// 写回分块大小。整窗口单事务撑爆 ws 通道是已记录事故（amssys 冷启动 4000+
/// 元素，`increment_pipeline.rs`），沿用同一分块纪律。
pub const TX_CHUNK: usize = 500;

/// 一次 `execute` 调用的路由模式（ADR-017 §3 读路由四则的写侧对偶）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    /// 暂存执行 + 进日志：窗口内后续读要看见、写回也要落持久层（常规写）。
    Both,
    /// 只暂存不进日志：暂存世界的工作集操作（预载拷入、兜底解析产物等），
    /// 持久层由别的机制（或根本不需要）落地。
    StagingOnly,
    /// 不暂存只进日志：commit-time-only 语句（全局扫描 / 修补），写回时按
    /// 原始位置对持久层执行。若其写集与暂存读集相交，调用方必须另以
    /// 工作集范围的语句在暂存库执行（T2.5 审计表的职责）。
    CommitOnly,
}

/// 日志条目：一次 `execute` 调用的 SQL 文本与模式，按执行顺序排列。
#[derive(Clone, Debug)]
pub struct JournalEntry {
    pub sql: String,
    pub mode: ExecMode,
}

/// 一个提交单元的暂存执行器。
///
/// 句柄约定：`staging` 已 `use_ns`/`use_db` 到该单元的 staging database
/// （`staging_{dbnum}_{window_id}`，T0.3 的生命周期模块负责建库与初始化）。
pub(super) struct StagedExecutor {
    staging: Surreal<Any>,
    label: String,
    journal: Vec<JournalEntry>,
    gauge: Option<Arc<ResourceGauge>>,
}

impl StagedExecutor {
    pub fn new(staging: Surreal<Any>, label: impl Into<String>) -> Self {
        Self {
            staging,
            label: label.into(),
            journal: Vec::new(),
            gauge: None,
        }
    }

    /// 接上资源面板（T0.3）：每次成功执行记账，`Abandon` 档位拒绝继续摄入。
    pub fn with_gauge(mut self, gauge: Arc<ResourceGauge>) -> Self {
        self.gauge = Some(gauge);
        self
    }

    /// 该单元的暂存库句柄（读路由上下文、预载等共用）。
    pub fn staging_db(&self) -> &Surreal<Any> {
        &self.staging
    }

    /// 暂存库名（`staging_{dbnum}_{window_id}`）。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 有序语句日志。
    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    /// 执行一段 SQL。进日志的模式（`Both` / `CommitOnly`）先过 ReplaySafe
    /// validator——不合规语句既不进暂存也不进日志，在源头拒绝（T0.5）。
    /// 暂存执行逐语句 `check()`：暂存世界的静默错误同样是错模型（F2）。
    pub async fn execute(&mut self, sql: impl Into<String>, mode: ExecMode) -> anyhow::Result<()> {
        let sql = sql.into();
        if matches!(mode, ExecMode::Both | ExecMode::CommitOnly) {
            replay_safe::validate_statement(&sql)?;
        }
        self.execute_validated(sql, mode).await
    }

    pub(super) async fn execute_scoped_delete(
        &mut self,
        sql: impl Into<String>,
    ) -> anyhow::Result<()> {
        let sql = sql.into();
        replay_safe::validate_scoped_delete_transaction(&sql)?;
        self.execute_validated(sql, ExecMode::Both).await
    }

    async fn execute_validated(&mut self, sql: String, mode: ExecMode) -> anyhow::Result<()> {
        let estimated_rows = replay_safe::estimate_write_rows(&sql)?;
        if let Some(gauge) = &self.gauge {
            let additional_bytes = match mode {
                ExecMode::Both => (sql.len() as u64).saturating_mul(2),
                ExecMode::StagingOnly | ExecMode::CommitOnly => sql.len() as u64,
            };
            let projected = gauge.projected_band(additional_bytes, estimated_rows);
            if projected == ResourceBand::Abandon {
                bail!(
                    "[{}] 当前语句将使暂存资源到达废弃档位（已有 {} 字节，预计新增 {} 字节/{} 行），停止摄入",
                    self.label,
                    gauge.total_bytes(),
                    additional_bytes,
                    estimated_rows,
                );
            }
            if gauge.band() < ResourceBand::Warn && projected >= ResourceBand::Warn {
                eprintln!("[{}] 暂存资源进入 {projected:?} 档位", self.label);
            }
        }
        match mode {
            ExecMode::Both => {
                self.run_on_staging(&sql).await?;
                if let Some(gauge) = &self.gauge {
                    gauge.record_staged(sql.len());
                    gauge.record_journal(sql.len());
                    gauge.record_write_rows(estimated_rows);
                }
                self.journal.push(JournalEntry { sql, mode });
            }
            ExecMode::StagingOnly => {
                self.run_on_staging(&sql).await?;
                if let Some(gauge) = &self.gauge {
                    gauge.record_staged(sql.len());
                    gauge.record_write_rows(estimated_rows);
                }
            }
            ExecMode::CommitOnly => {
                if let Some(gauge) = &self.gauge {
                    gauge.record_journal(sql.len());
                    gauge.record_write_rows(estimated_rows);
                }
                self.journal.push(JournalEntry { sql, mode });
            }
        }
        Ok(())
    }

    async fn run_on_staging(&self, sql: &str) -> anyhow::Result<()> {
        let response = self
            .staging
            .query(sql)
            .await
            .with_context(|| format!("[{}] 暂存执行传输失败", self.label))?;
        if let Err(error) = response.check() {
            bail!("[{}] 暂存执行语句失败: {error}", self.label);
        }
        Ok(())
    }

    /// 写回：日志按原序分块事务重放到 `target`，随后执行调用方渲染的尾事务。
    ///
    /// 任何一块失败即上抛——日志与暂存原样保留，调用方可整体重试（T4.1 的
    /// 退避与「写回滞留」告警在上层）。尾事务自身单独一个事务，収口条件
    /// （水位、revision 判真）在持久层判定。
    pub async fn commit_to(
        &self,
        target: &Surreal<Any>,
        tail_transaction: Option<&str>,
    ) -> anyhow::Result<()> {
        self.replay_journal_to(target, TX_CHUNK, None).await?;
        if let Some(tail) = tail_transaction {
            let tail_tx = wrap_in_transaction(&[tail.to_string()]).expect("非空尾事务必然可包装");
            execute_surreal_checked_on(target, &tail_tx, &format!("[{}] 写回尾事务", self.label))
                .await?;
        }
        Ok(())
    }

    /// 写回到生产持久层（`SUL_DB`）。
    pub async fn commit(&self, tail_transaction: Option<&str>) -> anyhow::Result<()> {
        self.commit_to(&aios_core::SUL_DB, tail_transaction).await
    }

    /// 按 `chunk_size` 分块重放日志；`max_chunks` 限制本次重放的块数
    /// （测试「随机中断写回再重放」的收敛性用）。返回本次执行的块数。
    pub(crate) async fn replay_journal_to(
        &self,
        target: &Surreal<Any>,
        chunk_size: usize,
        max_chunks: Option<usize>,
    ) -> anyhow::Result<usize> {
        let mut batches = Vec::new();
        let mut plain = Vec::new();
        let flush_plain = |plain: &mut Vec<String>, batches: &mut Vec<String>| {
            if let Some(sql) = wrap_in_transaction(plain) {
                batches.push(sql);
                plain.clear();
            }
        };
        for entry in &self.journal {
            if replay_safe::is_explicit_transaction(&entry.sql) {
                flush_plain(&mut plain, &mut batches);
                batches.push(entry.sql.clone());
            } else {
                plain.push(entry.sql.clone());
                if plain.len() == chunk_size.max(1) {
                    flush_plain(&mut plain, &mut batches);
                }
            }
        }
        flush_plain(&mut plain, &mut batches);

        let mut replayed = 0usize;
        for (index, sql) in batches.into_iter().enumerate() {
            if max_chunks.is_some_and(|max| index >= max) {
                break;
            }
            execute_surreal_checked_on(target, &sql, &format!("[{}] 写回块 {index}", self.label))
                .await?;
            replayed += 1;
        }
        Ok(replayed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;
    use surrealdb::engine::any::connect;

    /// 每个用例独立的 mem 实例 + 命名 staging database（仿 T0.3 的命名约定）。
    async fn staging_handle(db: &str) -> Surreal<Any> {
        let handle = connect("mem://").await.expect("mem boots");
        handle
            .use_ns("staging")
            .use_db(db)
            .await
            .expect("use staging db");
        handle
    }

    /// 独立 mem 实例扮演持久层（写回目标）。
    async fn persistent_handle() -> Surreal<Any> {
        let handle = connect("mem://").await.expect("mem boots");
        handle
            .use_ns("main")
            .use_db("main")
            .await
            .expect("use main db");
        handle
    }

    async fn select_values(db: &Surreal<Any>, sql: &str) -> String {
        let mut response = db.query(sql).await.expect("query");
        let value: surrealdb::Value = response.take(0).expect("take");
        serde_json::to_string(&value).expect("serialize")
    }

    /// T0.2 验收：三种模式对「暂存生效」「进日志」的路由各自正确，
    /// 日志保持原始顺序（CommitOnly 语句按原始位置参与写回）。
    #[tokio::test(flavor = "multi_thread")]
    async fn execute_modes_route_between_staging_and_journal() {
        let staging = staging_handle("staging_7997_1").await;
        let mut executor = StagedExecutor::new(staging.clone(), "staging_7997_1");

        executor
            .execute("UPSERT pe:a SET noun = 'ZONE'", ExecMode::Both)
            .await
            .expect("Both");
        executor
            .execute(
                "UPSERT pe:preloaded SET noun = 'SITE'",
                ExecMode::StagingOnly,
            )
            .await
            .expect("StagingOnly");
        executor
            .execute(
                "UPDATE inst_relate SET zone_refno = pe:a WHERE zone_refno = NONE",
                ExecMode::CommitOnly,
            )
            .await
            .expect("CommitOnly");

        // 暂存世界：Both 与 StagingOnly 可见，CommitOnly 未执行。
        let staged = select_values(&staging, "SELECT VALUE id FROM pe ORDER BY id").await;
        assert!(
            staged.contains("\"a\"") && staged.contains("preloaded"),
            "{staged}"
        );

        // 日志：Both 与 CommitOnly 在场且保持原始顺序，StagingOnly 缺席。
        let journal = executor.journal();
        assert_eq!(journal.len(), 2);
        assert_eq!(journal[0].mode, ExecMode::Both);
        assert!(journal[0].sql.contains("pe:a"));
        assert_eq!(journal[1].mode, ExecMode::CommitOnly);
        assert!(journal[1].sql.contains("zone_refno"));
    }

    /// T0.5 验收（源头拒绝）：不合规语句既不进暂存也不进日志。
    #[tokio::test(flavor = "multi_thread")]
    async fn validator_rejects_at_the_door_leaving_no_trace() {
        let staging = staging_handle("staging_7997_2").await;
        let mut executor = StagedExecutor::new(staging.clone(), "staging_7997_2");

        let error = executor
            .execute("CREATE pe SET noun = 'PIPE'", ExecMode::Both)
            .await
            .expect_err("裸表 CREATE 必须被拒");
        assert!(error.to_string().contains("ReplaySafe"), "{error}");

        assert!(executor.journal().is_empty());
        let staged = select_values(&staging, "SELECT * FROM pe").await;
        assert_eq!(staged, "{\"Array\":[]}", "暂存库不得有痕迹: {staged}");

        // 暂存执行失败的语句同样不进日志（先执行后入账）。
        let error = executor
            .execute("UPSERT pe:x SET n = math::nonexistent(1)", ExecMode::Both)
            .await
            .expect_err("暂存执行失败必须上抛");
        assert!(!error.to_string().is_empty());
        assert!(executor.journal().is_empty(), "失败语句不得入账");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn projected_abandon_rejects_before_staging_or_accounting() {
        let staging = staging_handle("staging_7997_resource").await;
        let gauge = ResourceGauge::new(ResourceThresholds {
            warn_bytes: 8,
            refuse_absorb_bytes: 16,
            abandon_bytes: 24,
            warn_rows: 100,
            refuse_absorb_rows: 200,
            abandon_rows: 300,
        });
        let mut executor =
            StagedExecutor::new(staging.clone(), "staging_7997_resource").with_gauge(gauge.clone());

        executor
            .execute("UPSERT pe:a SET noun = 'LONG_PIPE_NAME'", ExecMode::Both)
            .await
            .expect_err("预计越过 abandon 的当前语句必须在执行前拒绝");

        assert!(executor.journal().is_empty());
        assert_eq!(gauge.total_bytes(), 0);
        assert_eq!(
            select_values(&staging, "SELECT * FROM pe").await,
            "{\"Array\":[]}"
        );
    }

    /// 写回：按原序分块重放 + 尾事务收口，终态与语句语义一致。
    #[tokio::test(flavor = "multi_thread")]
    async fn commit_replays_in_order_with_chunking_and_tail_transaction() {
        let staging = staging_handle("staging_7997_3").await;
        let mut executor = StagedExecutor::new(staging, "staging_7997_3");

        // 5 条语句、块大小 2 → 3 块；后写覆盖先写依赖顺序正确。
        for (index, value) in [1i64, 2, 3].iter().enumerate() {
            executor
                .execute(
                    format!("UPSERT counter:c SET v = {value}, step = {index}"),
                    ExecMode::Both,
                )
                .await
                .expect("both");
        }
        executor
            .execute("UPSERT row:r1 SET tag = 'first'", ExecMode::Both)
            .await
            .expect("both");
        executor
            .execute(
                "UPDATE row:r1 SET tag = 'patched' WHERE tag = 'first'",
                ExecMode::CommitOnly,
            )
            .await
            .expect("commit-only");

        let target = persistent_handle().await;
        let chunks = executor
            .replay_journal_to(&target, 2, None)
            .await
            .expect("replay");
        assert_eq!(chunks, 3, "5 条语句按 2 分块应是 3 块");

        // 尾事务单独收口。
        executor
            .commit_to(
                &target,
                Some("UPSERT dbnum_watermark:7997 SET applied_sesno = 42"),
            )
            .await
            .expect("commit with tail");

        let counter = select_values(&target, "SELECT VALUE v FROM counter").await;
        assert_eq!(
            counter, "{\"Array\":[{\"Number\":{\"Int\":3}}]}",
            "最后写入胜出"
        );
        let tag = select_values(&target, "SELECT VALUE tag FROM row").await;
        assert_eq!(
            tag, "{\"Array\":[{\"Strand\":\"patched\"}]}",
            "CommitOnly 语句按原始位置生效"
        );
        let watermark =
            select_values(&target, "SELECT VALUE applied_sesno FROM dbnum_watermark").await;
        assert_eq!(
            watermark, "{\"Array\":[{\"Number\":{\"Int\":42}}]}",
            "尾事务已收口"
        );
    }

    /// T0.5 收敛测试：写回中断后拿同一份 journal 整体重放，终态与一次成功
    /// 写回完全一致（ReplaySafe ⇒ 幂等）。
    #[tokio::test(flavor = "multi_thread")]
    async fn interrupted_replay_then_full_retry_converges() {
        let staging = staging_handle("staging_7997_4").await;
        let mut executor = StagedExecutor::new(staging, "staging_7997_4");

        for i in 0..6 {
            executor
                .execute(
                    format!("UPSERT item:i{i} SET v = {i}, latest = {}", i * 10),
                    ExecMode::Both,
                )
                .await
                .expect("both");
        }

        // 中断路径：只重放第一块（2 条），随后整体重试。
        let interrupted = persistent_handle().await;
        let replayed = executor
            .replay_journal_to(&interrupted, 2, Some(1))
            .await
            .expect("partial replay");
        assert_eq!(replayed, 1);
        executor
            .commit_to(&interrupted, None)
            .await
            .expect("full retry after interruption");

        // 对照路径：一次成功写回。
        let clean = persistent_handle().await;
        executor
            .commit_to(&clean, None)
            .await
            .expect("clean commit");

        let a = select_values(&interrupted, "SELECT * FROM item ORDER BY id").await;
        let b = select_values(&clean, "SELECT * FROM item ORDER BY id").await;
        assert_eq!(a, b, "中断重试后的终态必须与一次成功写回一致");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn explicit_transaction_replays_without_nested_transaction() {
        let staging = staging_handle("staging_7997_tx").await;
        let mut executor = StagedExecutor::new(staging, "staging_7997_tx");
        executor
            .execute(
                "BEGIN; UPSERT pe:a SET noun = 'PIPE'; UPSERT pe:b SET noun = 'EQUI'; COMMIT;",
                ExecMode::Both,
            )
            .await
            .expect("stage transaction");

        let target = persistent_handle().await;
        executor
            .commit_to(&target, None)
            .await
            .expect("replay transaction");
        let rows = select_values(&target, "SELECT VALUE id FROM pe ORDER BY id").await;
        assert!(rows.contains("a") && rows.contains("b"), "{rows}");
    }
}
