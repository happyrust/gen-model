//! 解析侧共用的那条写入队列：**前台流着解析 / 渲染，后台单写者按批序列化进库**。
//!
//! 出处是基线摄入（`versioned_db::baseline_ingest`）——那条路 2026-09-07 就是这个形状，实测
//! 解析与写入真的重叠（5525 库：写者排空 7.4 s > 解析 4.25 s，写者才是瓶颈）。这里把它从
//! 基线里抬出来，让增量那一路用同一份，而不是各写一遍。
//!
//! 三条纪律，缺一条就会在真库上咬人：
//!
//! * **单写者。** SurrealDB 2.1 是乐观事务，多写者在共享表 / 索引键上持续互撞——7997 真库上
//!   曾经把重试预算耗尽。并行度要加在解析那一头，不在写这一头。
//! * **队列有界。** 无界队列不阻塞，代价是解析能把整库攒进内存。上界是
//!   [`QUEUE_CAPACITY`]：解析最多领先写者这么多笔事务。
//! * **[`WRITE_WINDOW`] 必须小于 [`QUEUE_CAPACITY`]。** 写者 `chunks(N)` 要等够 N 条才交出一窗，
//!   而生产者在队列满时阻塞；N ≥ 容量就是一个确定性的死锁。
//!
//! 失败不中断消费：**队列必须被排空**，否则生产者卡在 `send` 上永远收不了尾。要不要在第一笔
//! 失败之后继续往库里写，由 [`OnWriteError`] 决定——账都记着，由调用方最后裁决。

use futures::StreamExt;

use aios_core::SUL_DB;

/// 有界队列容量：解析最多领先写者这么多笔事务。
pub(crate) const QUEUE_CAPACITY: usize = 100;
/// 写者每次从队列取出的事务数。必须小于 [`QUEUE_CAPACITY`]（见模块注释第三条）。
pub(crate) const WRITE_WINDOW: usize = 20;

/// 第一笔失败之后怎么办。两档都**照常把队列排空**，区别只在还发不发库。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnWriteError {
    /// 接着写后面的事务。整库基线用它：一笔坏批不该让其余几万行也进不去，
    /// 失败最后由 `wipe_after_failure` 整库清掉重来。
    Continue,
    /// 后面的事务只丢不发。增量用它：窗口是有序的先删后插，前一笔没落地时继续往下写
    /// 只会在库里堆更多对不上的中间态，而这一窗本来就要整窗重来。
    StopWriting,
}

/// 写者任务的收尾账：失败次数与前几条样例（错误文本进日志与回执）。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WriterOutcome {
    pub errors: usize,
    pub samples: Vec<String>,
}

impl WriterOutcome {
    pub(crate) fn record(&mut self, context: &str, error: anyhow::Error) {
        self.errors += 1;
        if self.samples.len() < 3 {
            self.samples.push(format!("{context}: {error}"));
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
}

/// 起一个单写者：按 [`WRITE_WINDOW`] 条一窗从队列取事务，逐笔 `execute_on`（含冲突重试）。
pub(crate) fn spawn_writer(
    receiver: flume::Receiver<String>,
    context: &'static str,
    on_error: OnWriteError,
) -> tokio::task::JoinHandle<WriterOutcome> {
    tokio::task::spawn(async move {
        let mut stream = receiver.into_stream().chunks(WRITE_WINDOW);
        let mut outcome = WriterOutcome::default();
        while let Some(window) = stream.next().await {
            for sql in window {
                if outcome.errors > 0 && on_error == OnWriteError::StopWriting {
                    continue;
                }
                if let Err(error) =
                    crate::data_interface::pe_batch::execute_on(&SUL_DB, &sql).await
                {
                    outcome.record(context, error);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 死锁那条纪律是个算术事实，钉住它比写一段注释可靠。
    #[test]
    fn the_write_window_is_smaller_than_the_queue() {
        assert!(WRITE_WINDOW < QUEUE_CAPACITY);
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
}
