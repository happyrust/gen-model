//! 第 5 步：实时计算并发闸（design §1.2 第 5 步，Requirement 6.17、7.1、7.5）。
//!
//! 未命中结果缓存的请求先在这里取一个名额；名额满时立即拒绝（503 `HISTORY_BUSY` +
//! `Retry-After`），不排队、不改变进行中的计算数。取到的 [`OwnedSemaphorePermit`]
//! 要随计算闭包一起移进 `spawn_blocking`：handler 超时或客户端断开后，阻塞线程还会
//! 跑到下一个取消检查点才退出，名额在那时（permit 被 drop）才归还，`in_flight`
//! 因此反映真正还在占 CPU 的计算数。

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// `Retry-After` 的取值范围（秒，Requirement 7.1）。
pub const RETRY_AFTER_MIN_SECS: u64 = 1;
pub const RETRY_AFTER_MAX_SECS: u64 = 5;

/// 名额已满：调用方据此回 503 `HISTORY_BUSY`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateBusy {
    /// 拒绝时观察到的进行中计算数。
    pub in_flight: usize,
    /// 并发上限。
    pub limit: usize,
    /// `Retry-After` 响应头的秒数，落在 [1, 5]。
    pub retry_after_secs: u64,
}

/// `Retry-After = clamp(1 + in_flight / limit, 1, 5)` 秒（整数除法）。
pub fn retry_after_secs(in_flight: usize, limit: usize) -> u64 {
    let limit = limit.max(1);
    let secs = 1u64.saturating_add((in_flight / limit) as u64);
    secs.clamp(RETRY_AFTER_MIN_SECS, RETRY_AFTER_MAX_SECS)
}

/// 实时历史计算的并发闸。
#[derive(Debug, Clone)]
pub struct RealtimeGate {
    semaphore: Arc<Semaphore>,
    limit: usize,
}

impl RealtimeGate {
    /// 以给定上限建闸；上限小于 1 时按 1。
    pub fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
            limit,
        }
    }

    /// 上限取 [`crate::options::model_history_realtime_concurrency`]。
    pub fn from_options() -> Self {
        Self::new(crate::options::model_history_realtime_concurrency())
    }

    /// 进程内共享的那一道闸（路由与 `/health` 共用）。
    pub fn global() -> &'static RealtimeGate {
        static GATE: OnceLock<RealtimeGate> = OnceLock::new();
        GATE.get_or_init(Self::from_options)
    }

    /// 并发上限。
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// 进行中的计算数：`limit − available_permits()`。
    pub fn in_flight(&self) -> usize {
        self.limit
            .saturating_sub(self.semaphore.available_permits())
    }

    /// 不等待地取一个名额。满了返回 [`GateBusy`]，不改变 `in_flight`。
    ///
    /// 返回的 permit 是 owned 的，可以移进阻塞线程；drop 时归还名额。
    pub fn try_acquire(&self) -> Result<OwnedSemaphorePermit, GateBusy> {
        match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(_) => {
                let in_flight = self.in_flight();
                Err(GateBusy {
                    in_flight,
                    limit: self.limit,
                    retry_after_secs: retry_after_secs(in_flight, self.limit),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn retry_after_formula_is_clamped_to_1_through_5() {
        assert_eq!(retry_after_secs(0, 4), 1);
        assert_eq!(retry_after_secs(3, 4), 1);
        assert_eq!(retry_after_secs(4, 4), 2);
        assert_eq!(retry_after_secs(8, 4), 3);
        assert_eq!(retry_after_secs(1000, 4), 5);
        assert_eq!(retry_after_secs(usize::MAX, 1), 5);
        // 上限 0 不会除零，按 1 算。
        assert_eq!(retry_after_secs(1, 0), 2);
    }

    #[test]
    fn rejects_when_full_with_retry_after_and_keeps_in_flight() {
        let gate = RealtimeGate::new(2);
        assert_eq!(gate.limit(), 2);
        assert_eq!(gate.in_flight(), 0);

        let a = gate.try_acquire().expect("first permit");
        let b = gate.try_acquire().expect("second permit");
        assert_eq!(gate.in_flight(), 2);

        let busy = gate.try_acquire().expect_err("gate full");
        assert_eq!(
            busy,
            GateBusy {
                in_flight: 2,
                limit: 2,
                retry_after_secs: 2,
            }
        );
        // 拒绝不改变进行中的计算数。
        assert_eq!(gate.in_flight(), 2);

        drop(a);
        drop(b);
    }

    #[test]
    fn dropping_permit_lowers_in_flight_and_frees_a_slot() {
        let gate = RealtimeGate::new(1);
        let permit = gate.try_acquire().expect("permit");
        assert_eq!(gate.in_flight(), 1);
        assert!(gate.try_acquire().is_err());

        drop(permit);
        assert_eq!(gate.in_flight(), 0);
        let again = gate.try_acquire().expect("slot freed after drop");
        assert_eq!(gate.in_flight(), 1);
        drop(again);
        assert_eq!(gate.in_flight(), 0);
    }

    #[test]
    fn zero_limit_is_treated_as_one() {
        let gate = RealtimeGate::new(0);
        assert_eq!(gate.limit(), 1);
        let _p = gate.try_acquire().expect("one slot");
        assert!(gate.try_acquire().is_err());
    }

    #[tokio::test]
    async fn permit_moved_into_blocking_thread_is_released_when_work_stops() {
        let gate = RealtimeGate::new(1);
        let permit = gate.try_acquire().expect("permit");

        // 模拟 handler 已经返回（例如超时），计算闭包仍在阻塞线程上跑。
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            stop_rx.recv().expect("stop signal");
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(gate.in_flight(), 1, "still held while work runs");
        let busy = gate.try_acquire().expect_err("still full");
        assert_eq!(busy.retry_after_secs, 2);

        stop_tx.send(()).expect("send stop");
        handle.await.expect("blocking task joined");
        assert_eq!(gate.in_flight(), 0, "released once work stopped");
        assert!(gate.try_acquire().is_ok());
    }

    #[test]
    fn permit_can_move_across_std_threads() {
        let gate = RealtimeGate::new(3);
        let permits: Vec<_> = (0..3).map(|_| gate.try_acquire().unwrap()).collect();
        assert_eq!(gate.in_flight(), 3);

        let handles: Vec<_> = permits
            .into_iter()
            .map(|p| std::thread::spawn(move || drop(p)))
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(gate.in_flight(), 0);
    }

    // Feature: model-version-timeline-scrub, Property 37: 并发闸与无快照副作用（后端）。
    //
    // 闸的语义部分在这里做属性验证：任意交错的「取名额 / 放名额 / 满时再来一次」序列，
    // 进行中计算数任一时刻不超过上限、忙拒绝不改变进行中计算数、每个名额恰好释放一次。
    // 「整个序列前后 `HistoricalModelStore::stats().snapshots` 与任务表条数不变」需要真库
    // 与真计算器，由 W4 集成脚本 `realtime-history-smoke.ps1` 覆盖（见 tasks.md 第 8 组）。
    mod prop_gate_model {
        use proptest::prelude::*;

        use super::super::*;

        /// 一条操作：取名额、放掉某个已持有的名额、或在满时尝试取。
        #[derive(Debug, Clone)]
        enum Op {
            Acquire,
            Release(usize),
            TryWhileFull,
        }

        fn ops() -> impl Strategy<Value = Vec<Op>> {
            prop::collection::vec(
                prop_oneof![
                    4 => Just(Op::Acquire),
                    4 => (0usize..8).prop_map(Op::Release),
                    2 => Just(Op::TryWhileFull),
                ],
                0..30,
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn interleaved_acquire_release_never_exceeds_the_limit(
                limit in 1usize..=64,
                operations in ops(),
            ) {
                let gate = RealtimeGate::new(limit);
                let mut held: Vec<OwnedSemaphorePermit> = Vec::new();
                let mut acquired_total = 0usize;

                for (step, op) in operations.iter().enumerate() {
                    match op {
                        Op::Acquire | Op::TryWhileFull => {
                            let full = held.len() >= limit;
                            match gate.try_acquire() {
                                Ok(permit) => {
                                    prop_assert!(!full, "step {}: acquired while full", step);
                                    held.push(permit);
                                    acquired_total += 1;
                                }
                                Err(busy) => {
                                    prop_assert!(full, "step {}: rejected below the limit", step);
                                    // 忙拒绝不改变进行中计算数，Retry-After 落在 [1, 5]。
                                    prop_assert_eq!(busy.in_flight, held.len(), "step {}", step);
                                    prop_assert_eq!(busy.limit, limit);
                                    prop_assert!(
                                        (RETRY_AFTER_MIN_SECS..=RETRY_AFTER_MAX_SECS)
                                            .contains(&busy.retry_after_secs),
                                        "step {}: retry_after = {}",
                                        step,
                                        busy.retry_after_secs
                                    );
                                }
                            }
                        }
                        Op::Release(index) => {
                            if !held.is_empty() {
                                let index = index % held.len();
                                held.remove(index);
                                // 名额恰好释放一次：in_flight 跟着持有数走。
                                prop_assert_eq!(gate.in_flight(), held.len(), "step {}", step);
                            }
                        }
                    }
                    prop_assert_eq!(gate.in_flight(), held.len(), "step {}", step);
                    prop_assert!(gate.in_flight() <= limit, "step {}: over the limit", step);
                }

                // 全部放掉后回到零，且名额总数守恒。
                let released = held.len();
                drop(held);
                prop_assert_eq!(gate.in_flight(), 0);
                prop_assert!(acquired_total >= released);
            }

            /// 上限 0 按 1 处理，永远只放一个名额进来。
            #[test]
            fn zero_limit_degrades_to_one_slot(attempts in 1usize..8) {
                let gate = RealtimeGate::new(0);
                prop_assert_eq!(gate.limit(), 1);
                let mut held = Vec::new();
                for _ in 0..attempts {
                    match gate.try_acquire() {
                        Ok(permit) => held.push(permit),
                        Err(busy) => prop_assert_eq!(busy.in_flight, 1),
                    }
                    prop_assert!(gate.in_flight() <= 1);
                }
                prop_assert_eq!(held.len(), 1);
            }
        }
    }
}
