//! 落盘写者（ADR-077 D2 / D9 / D10）：**前台流着解析 / 生成，后台按 lane 单写者逐笔把事务文本写进库**。
//!
//! 出处是基线摄入那条写者队列（2026-09-07 起 `versioned_db::baseline_ingest`，2026-09-14 抬成
//! `sql_write_queue`）。这里把它泛化成两条 lane 共用的一份：属性 lane 照旧收元素批事务；模型 lane
//! 收初始化渲好的模型切片事务。两条 lane 是同一套机器的两个实例，**不同 lane 并发、同 lane 串行**。
//!
//! 三条纪律，缺一条就会在真库上咬人：
//!
//! * **一条 lane 一个写者。** SurrealDB 2.1 是乐观事务，同一面多写者在共享表 / 索引键上持续互撞——
//!   7997 真库上曾经把重试预算耗尽（模型面 4 / 8 写者实测更慢，见 zone-partitioned plan）。并行度要
//!   加在解析 / 生成那一头，不在写这一头；两面的键集合不相交，才允许两条 lane 并发。
//! * **队列有界。** 无界队列不阻塞，代价是解析能把整库攒进内存。属性 lane 上界 [`QUEUE_CAPACITY`]
//!   笔事务；模型 lane 上界 [`MODEL_LANE_DEPTH`] 片。
//! * **写者逐笔取、逐笔写，不攒窗。** 旧写者用 `chunks(WRITE_WINDOW)` 攒够 20 笔才动手，于是要靠
//!   `WRITE_WINDOW < QUEUE_CAPACITY` 防死锁；深度 2 的模型 lane 在那个形状下必死锁。写者本来就是
//!   一笔一笔发库，攒窗没有任何收益，去掉之后那条死锁纪律随之消失。
//!
//! 失败不中断消费：**队列必须被排空**，否则生产者卡在 `send` 上永远收不了尾。要不要在第一笔
//! 失败之后继续往库里写，由 [`OnWriteError`] 决定——账都记着，由调用方最后裁决。
//!
//! 每一笔都带期限（[`transaction_timeout`]）：WS router 会把 pending response 留到重连，一次挂起
//! 就是一条 lane 永远收不了尾（2026-09-06 run-ad 16:38）。超时按可重试处理再试一次，仍超就记账。

use std::time::{Duration, Instant};

use surrealdb::{Surreal, engine::any::Any};

/// 属性 lane 的有界队列容量：解析最多领先写者这么多笔事务。
pub(crate) const QUEUE_CAPACITY: usize = 100;
/// 模型 lane 的有界队列深度（按片计）：一片在库里写、一片渲好等着、生成侧在攒第三片（ADR-077 D9）。
pub(crate) const MODEL_LANE_DEPTH: usize = 2;

/// 单笔事务的基础期限；随字节数往上加，见 [`transaction_timeout`]。
const TRANSACTION_BASE_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSACTION_TIMEOUT_CAP: Duration = Duration::from_secs(180);
/// 冲突重试预算，与 `surreal_retry::execute_surreal_checked_on` 同值。
const CONFLICT_ATTEMPTS: usize = 16;
/// 超时之后再试几次。超时不是冲突：库那头可能已经在做，重试是赌它这次能回来，赌一次就够。
const TIMEOUT_RETRIES: usize = 1;

/// 一笔事务的期限：30 s 是小事务的口径，每 MiB SQL 加 8 s，上限 180 s（`publication_timeout_for` 的口径）。
pub(crate) fn transaction_timeout(sql_bytes: usize) -> Duration {
    let per_mib = Duration::from_secs(8 * (sql_bytes / (1024 * 1024)) as u64);
    (TRANSACTION_BASE_TIMEOUT + per_mib).min(TRANSACTION_TIMEOUT_CAP)
}

/// 交给写者的一笔：已经包好 `BEGIN … COMMIT` 的事务文本，加一个只给失败样例用的标签。
///
/// 写者不认识标签的内容：元素批写 `element batch #k`，模型切片写 `model slice roots=[a,b,…]`，
/// 失败时它原样进 [`WriterOutcome::samples`]，人看日志就知道是谁的错。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJob {
    pub sql: String,
    pub label: String,
}

impl WriteJob {
    pub(crate) fn new(sql: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            label: label.into(),
        }
    }
}

/// 第一笔失败之后怎么办。两档都**照常把队列排空**，区别只在还发不发库。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnWriteError {
    /// 接着写后面的事务。属性 lane 用它：一笔坏批不该让其余几万行也进不去，
    /// 失败最后由整库清空重来。
    Continue,
    /// 后面的事务只丢不发。模型 lane 用它：一笔失败整库反正要清，继续往库里写只是白花时间。
    StopWriting,
}

/// 写者任务的收尾账：失败次数与前几条样例（错误文本进日志与回执），以及这条 lane 的体量与重试。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WriterOutcome {
    pub errors: usize,
    pub samples: Vec<String>,
    /// 成功提交的事务数。
    pub transactions: usize,
    /// 成功提交的事务文本字节数合计。
    pub bytes: usize,
    /// 乐观事务冲突后重试的次数（ADR-077 D3 那道门的尺子）。
    pub conflict_retries: usize,
    /// 撞上期限的次数。
    pub timeouts: usize,
    /// 写者从起到**最后一笔写完**的墙钟毫秒数（一笔没写过就是到排空为止）。
    ///
    /// 不算队列空着等 sender 关掉的那一截：`Lane` 自己那份 sender 要到 `drain` 才掉，而 `drain` 等的是
    /// 两半都收工——按「到排空」算，模型 lane 的账会被数据半边的收工时刻顶上去（7997 实测：模型半边
    /// 58 s 就写完了，账上却是 110 s）。
    pub elapsed_ms: u64,
}

impl WriterOutcome {
    pub(crate) fn record(&mut self, context: &str, error: anyhow::Error) {
        self.errors += 1;
        if self.samples.len() < 3 {
            // `{:#}`：把标签与库的原话整条链打出来，只打最外层就只剩标签。
            self.samples.push(format!("{context}: {error:#}"));
        }
    }

    /// 把账折成一句话；`None` = 干净。
    pub(crate) fn failure(&self, what: &str) -> Option<String> {
        (self.errors > 0).then(|| {
            format!(
                "{what} SurrealDB write failed {} time(s); samples: {}",
                self.errors,
                self.samples.join("; ")
            )
        })
    }

    /// 排空时打的那一行账（ADR-077 后果 6）。
    pub(crate) fn summary(&self, lane: &str) -> String {
        format!(
            "persist lane drained: lane={lane} transactions={} bytes={} elapsed_ms={} conflict_retries={} timeouts={} errors={}",
            self.transactions,
            self.bytes,
            self.elapsed_ms,
            self.conflict_retries,
            self.timeouts,
            self.errors
        )
    }
}

/// 一笔失败按哪一档处理。
enum Attempt {
    Done,
    Conflict(anyhow::Error),
    TimedOut,
    Failed(anyhow::Error),
}

async fn attempt_once(db: &Surreal<Any>, job: &WriteJob, context: &str) -> Attempt {
    let request = async {
        let mut response = db
            .query(job.sql.clone())
            .await
            .map_err(|error| anyhow::anyhow!("{context} transport failed: {error}"))?;
        // 回滚掉的事务会给出错语句之前的每一条都报一句笼统的「事务失败」，只看第一条永远看不到
        // 真正的原因：全部摘出来、按语句序排好。
        let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
        errors.sort_by_key(|(index, _)| *index);
        if !errors.is_empty() {
            anyhow::bail!(
                "{context} statements failed: {}",
                errors
                    .into_iter()
                    .map(|(index, error)| format!("statement {index}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        Ok::<(), anyhow::Error>(())
    };
    match tokio::time::timeout(transaction_timeout(job.sql.len()), request).await {
        Ok(Ok(())) => Attempt::Done,
        Ok(Err(error))
            if crate::surreal_retry::is_retryable_surreal_write_error(&error.to_string()) =>
        {
            Attempt::Conflict(error)
        }
        Ok(Err(error)) => Attempt::Failed(error),
        Err(_) => Attempt::TimedOut,
    }
}

/// 执行一笔：冲突按指数退避重试、超时再试一次，其余错误原样返回。账记在 `outcome` 上。
///
/// **要求事务文本幂等。** 冲突时整笔一行都没落、原样重发是安全的；超时那一档库那头可能已经提交，
/// 重发靠 `INSERT IGNORE` / `UPSERT` 收敛——两条 lane 渲的行都是这个形状。
async fn execute_job(
    db: &Surreal<Any>,
    job: &WriteJob,
    context: &str,
    outcome: &mut WriterOutcome,
) -> anyhow::Result<()> {
    let mut conflicts = 0usize;
    let mut timeouts = 0usize;
    loop {
        match attempt_once(db, job, context).await {
            Attempt::Done => return Ok(()),
            Attempt::Conflict(error) => {
                conflicts += 1;
                if conflicts >= CONFLICT_ATTEMPTS {
                    return Err(error.context(format!("{}: conflict retries exhausted", job.label)));
                }
                outcome.conflict_retries += 1;
                tokio::time::sleep(crate::surreal_retry::conflict_retry_backoff(conflicts)).await;
            }
            Attempt::TimedOut => {
                timeouts += 1;
                outcome.timeouts += 1;
                if timeouts > TIMEOUT_RETRIES {
                    anyhow::bail!(
                        "{context} timed out {timeouts} time(s) after {:?} each ({} bytes): {}",
                        transaction_timeout(job.sql.len()),
                        job.sql.len(),
                        job.label
                    );
                }
            }
            Attempt::Failed(error) => return Err(error.context(job.label.clone())),
        }
    }
}

/// 起一个单写者：逐笔从队列取事务、逐笔执行（含期限与冲突重试），直到队列关闭并排空。
///
/// 库由调用方给（测试对一次性 `mem://` 实例用）。过去还有一个取本进程 `SUL_DB` 的包装，
/// 它没有任何调用方——写者要写哪个库是任务自己的事，不该由这里默认成进程那一个。
pub(crate) fn spawn_writer_on(
    db: Surreal<Any>,
    receiver: flume::Receiver<WriteJob>,
    context: &'static str,
    on_error: OnWriteError,
) -> tokio::task::JoinHandle<WriterOutcome> {
    tokio::task::spawn(async move {
        let started = Instant::now();
        let mut last_write_done: Option<Duration> = None;
        let mut outcome = WriterOutcome::default();
        while let Ok(job) = receiver.recv_async().await {
            if outcome.errors > 0 && on_error == OnWriteError::StopWriting {
                continue;
            }
            match execute_job(&db, &job, context, &mut outcome).await {
                Ok(()) => {
                    outcome.transactions += 1;
                    outcome.bytes += job.sql.len();
                }
                Err(error) => outcome.record(context, error),
            }
            last_write_done = Some(started.elapsed());
        }
        outcome.elapsed_ms = last_write_done
            .unwrap_or_else(|| started.elapsed())
            .as_millis() as u64;
        outcome
    })
}

/// 收尾写入管线：先关闭 sender、等写者把队列排空，再把账交出去。
///
/// 顺序是关键。前台失败时如果提前返回，已经渲好的数据会连同没人消费的通道一起被丢弃，
/// 而写者可能还在往库里写——清库要等它停下来才算得上「清」。
pub(crate) async fn drain_writer(
    sender: flume::Sender<WriteJob>,
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

/// 一条写入 lane：一条有界 channel + 一个单写者。属性 / 模型各一条，不同 lane 并发、同 lane 串行。
pub struct Lane {
    name: &'static str,
    sender: Option<flume::Sender<WriteJob>>,
    writer: tokio::task::JoinHandle<WriterOutcome>,
}

impl Lane {
    /// 在 `db` 上起一条 lane。库由调用方显式给——`Lane` 自己不设默认库。
    pub(crate) fn spawn_on(
        db: Surreal<Any>,
        name: &'static str,
        capacity: usize,
        on_error: OnWriteError,
    ) -> Self {
        let (sender, receiver) = flume::bounded::<WriteJob>(capacity.max(1));
        Self {
            name,
            sender: Some(sender),
            writer: spawn_writer_on(db, receiver, name, on_error),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// 给生产者的那一头。`drain` 之后再调是编程错误。
    pub(crate) fn sender(&self) -> flume::Sender<WriteJob> {
        self.sender
            .clone()
            .expect("a persist lane hands out senders only before it is drained")
    }

    /// 关掉自己那一份 sender、等写者排空、交账。生产者手里 clone 出去的 sender 也得先掉，
    /// 否则这里等不到队列关闭——调用方要在 `drain` 之前把它们都 drop 掉。
    pub(crate) async fn drain(mut self) -> WriterOutcome {
        let sender = self
            .sender
            .take()
            .expect("a persist lane is drained exactly once");
        let outcome = drain_writer(sender, self.writer).await;
        log::info!("{}", outcome.summary(self.name));
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("persist_writer").use_db(label).await.unwrap();
        db
    }

    fn pe_job(dbnum: u32, n: usize) -> WriteJob {
        WriteJob::new(
            format!(
                "BEGIN TRANSACTION; INSERT IGNORE INTO pe [{{ id: pe:{dbnum}_{n}, dbnum: {dbnum} }}] RETURN NONE; COMMIT TRANSACTION;"
            ),
            format!("element batch #{n}"),
        )
    }

    #[test]
    fn a_clean_run_reports_nothing() {
        assert_eq!(WriterOutcome::default().failure("increment"), None);
    }

    #[test]
    fn the_account_keeps_at_most_three_samples() {
        let mut outcome = WriterOutcome::default();
        for index in 0..5 {
            outcome.record("element batch", anyhow::anyhow!("boom {index}"));
        }
        assert_eq!(outcome.errors, 5);
        assert_eq!(outcome.samples.len(), 3);
        let failure = outcome.failure("increment").expect("failure summary");
        assert!(failure.contains("failed 5 time(s)"), "{failure}");
    }

    /// 期限随体量走：小事务 30 s，每 MiB 加 8 s，封顶 180 s。
    #[test]
    fn the_transaction_timeout_grows_with_bytes_and_caps() {
        assert_eq!(transaction_timeout(0), Duration::from_secs(30));
        assert_eq!(
            transaction_timeout(2 * 1024 * 1024),
            Duration::from_secs(46)
        );
        assert_eq!(
            transaction_timeout(64 * 1024 * 1024),
            Duration::from_secs(180)
        );
    }

    /// 解析失败时也要先排空写者：不等待就返回的实现会把已解析的数据连同通道一起丢掉，
    /// 而清库会在写者还在写的时候发生。
    #[tokio::test]
    async fn drain_writer_waits_for_the_writer_to_consume_everything() {
        const MESSAGES: usize = 8;
        let (sender, receiver) = flume::bounded::<WriteJob>(MESSAGES);
        for i in 0..MESSAGES {
            sender.send(pe_job(1, i)).unwrap();
        }
        let consumed = Arc::new(AtomicUsize::new(0));
        let writer_consumed = consumed.clone();
        let writer = tokio::task::spawn(async move {
            // 先歇一会儿：不等待 writer 就返回的实现，此刻计数一定还是 0。
            tokio::time::sleep(Duration::from_millis(80)).await;
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
        let (sender, receiver) = flume::bounded::<WriteJob>(1);
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

    /// 一条 lane 把交给它的每一笔都落进库，账上的笔数与字节数对得上。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_lane_lands_every_job_and_keeps_the_account() {
        let db = database("lane").await;
        let lane = Lane::spawn_on(db.clone(), "element batch", 4, OnWriteError::Continue);
        let sender = lane.sender();
        let mut bytes = 0usize;
        for n in 0..10 {
            let job = pe_job(7, n);
            bytes += job.sql.len();
            sender.send_async(job).await.unwrap();
        }
        drop(sender);
        let outcome = lane.drain().await;
        assert_eq!(outcome.errors, 0, "{:?}", outcome.samples);
        assert_eq!(outcome.transactions, 10);
        assert_eq!(outcome.bytes, bytes);
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = db
            .query("SELECT count() AS n FROM pe WHERE dbnum = 7 GROUP ALL;")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(rows[0].n, 10);
    }

    /// 账上的耗时到**最后一笔写完**为止：写完之后 sender 还开着（另一半没收工）那一截空等不算。
    /// 按「到排空」算的实现在这里会报 ≥ 400 ms。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_lane_account_stops_the_clock_at_the_last_write_not_at_drain() {
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let db = database("idle_tail").await;
        let spawned = Instant::now();
        let lane = Lane::spawn_on(db.clone(), "model slice", 2, OnWriteError::StopWriting);
        let sender = lane.sender();
        sender.send_async(pe_job(9, 0)).await.unwrap();
        sender.send_async(pe_job(9, 1)).await.unwrap();
        // 等两笔真落地，再让通道空转 400 ms——模拟模型半边早已写完、数据半边还没收工。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let rows: Vec<Count> = db
                .query("SELECT count() AS n FROM pe WHERE dbnum = 9 GROUP ALL;")
                .await
                .unwrap()
                .take(0)
                .unwrap();
            if rows.first().is_some_and(|row| row.n == 2) {
                break;
            }
            assert!(Instant::now() < deadline, "两笔没有在 5 s 内落地");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(sender);
        let outcome = lane.drain().await;
        let total_ms = spawned.elapsed().as_millis() as u64;
        assert_eq!(outcome.transactions, 2, "{:?}", outcome.samples);
        assert!(
            outcome.elapsed_ms + 300 <= total_ms,
            "耗时不该把空等的尾巴算进去: elapsed_ms={} 起到排空={total_ms}",
            outcome.elapsed_ms
        );
    }

    /// 两条 lane 并发往同一个库写不同的表，行都在、互不干扰（ADR-077 D2 / D3 的 mem 版）。
    #[tokio::test(flavor = "multi_thread")]
    async fn two_lanes_write_the_same_database_concurrently() {
        let db = database("two_lanes").await;
        let attributes = Lane::spawn_on(db.clone(), "element batch", 4, OnWriteError::Continue);
        let model = Lane::spawn_on(db.clone(), "model slice", 2, OnWriteError::StopWriting);
        let attribute_sender = attributes.sender();
        let model_sender = model.sender();
        let producers = tokio::join!(
            async move {
                for n in 0..20 {
                    attribute_sender.send_async(pe_job(9, n)).await.unwrap();
                }
            },
            async move {
                for n in 0..20 {
                    model_sender
                        .send_async(WriteJob::new(
                            format!(
                                "BEGIN TRANSACTION; INSERT RELATION IGNORE INTO inst_relate [{{ id: inst_relate:{n}, in: pe:9_{n}, out: inst_info:{n}, dbnum: 9 }}] RETURN NONE; COMMIT TRANSACTION;"
                            ),
                            format!("model slice roots=[9/{n}]"),
                        ))
                        .await
                        .unwrap();
                }
            }
        );
        let _ = producers;
        let (attributes, model) = tokio::join!(attributes.drain(), model.drain());
        assert_eq!(attributes.errors, 0, "{:?}", attributes.samples);
        assert_eq!(model.errors, 0, "{:?}", model.samples);
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let mut response = db
            .query("SELECT count() AS n FROM pe WHERE dbnum = 9 GROUP ALL; SELECT count() AS n FROM inst_relate WHERE dbnum = 9 GROUP ALL;")
            .await
            .unwrap();
        let pe: Vec<Count> = response.take(0).unwrap();
        let relate: Vec<Count> = response.take(1).unwrap();
        assert_eq!(pe[0].n, 20);
        assert_eq!(relate[0].n, 20);
    }

    /// 语句错误不是冲突：不重试、记账、按 `OnWriteError` 决定后面的还发不发。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_statement_error_is_recorded_and_stop_writing_skips_the_rest() {
        let db = database("stop").await;
        let lane = Lane::spawn_on(db.clone(), "model slice", 2, OnWriteError::StopWriting);
        let sender = lane.sender();
        sender
            .send_async(WriteJob::new(
                "BEGIN TRANSACTION; INSERT IGNORE INTO pe [{ id: pe:3_0, dbnum: 3 }] RETURN NONE; THROW 'bad slice'; COMMIT TRANSACTION;",
                "model slice roots=[1/1]",
            ))
            .await
            .unwrap();
        sender.send_async(pe_job(3, 1)).await.unwrap();
        drop(sender);
        let outcome = lane.drain().await;
        assert_eq!(outcome.errors, 1, "{:?}", outcome.samples);
        assert!(
            outcome.samples[0].contains("roots=[1/1]") && outcome.samples[0].contains("bad slice"),
            "失败样例要带标签与原因: {:?}",
            outcome.samples
        );
        assert_eq!(outcome.transactions, 0, "StopWriting 之后的那一笔不许再发");
        assert_eq!(outcome.conflict_retries, 0);
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        // 回滚：THROW 之前那一行也不在；StopWriting：后面那一笔没发。
        let rows: Vec<Count> = db
            .query("SELECT count() AS n FROM pe WHERE dbnum = 3 GROUP ALL;")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(rows.is_empty() || rows[0].n == 0, "{}", rows.len());
    }

    /// 同一笔在 `Continue` 档下：坏的记账，后面的照写。
    #[tokio::test(flavor = "multi_thread")]
    async fn continue_keeps_writing_after_a_failure() {
        let db = database("continue").await;
        let lane = Lane::spawn_on(db.clone(), "element batch", 2, OnWriteError::Continue);
        let sender = lane.sender();
        sender
            .send_async(WriteJob::new(
                "BEGIN TRANSACTION; THROW 'bad batch'; COMMIT TRANSACTION;",
                "element batch #0",
            ))
            .await
            .unwrap();
        sender.send_async(pe_job(4, 1)).await.unwrap();
        drop(sender);
        let outcome = lane.drain().await;
        assert_eq!(outcome.errors, 1);
        assert_eq!(outcome.transactions, 1);
    }
}
