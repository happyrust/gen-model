//! 实时历史几何查询的取消标志与 DFS 停止检查点（设计 §1.2 断开取消、§1.3 实时计算源）。
//!
//! - [`CancelToken`]：一只共享的取消标志。handler 建一只，克隆进 `spawn_blocking` 闭包；
//!   超时（Requirement 6.9）或请求断开（6.14）时置位，阻塞线程在下一个检查点退出。
//! - [`CancelOnDrop`]：handler future 被 hyper 丢弃（连接关闭）时，guard 的 `Drop` 置位 token。
//! - [`HistoryHooks`]：e3d-model DFS 的钩子，每弹出一个元素问一次：先看取消，再看元素上限（6.8）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use e3d_model::pipeline::{GenerationHooks, Stop};

/// 共享的取消标志：克隆得到同一只标志，任何一份 [`cancel`](Self::cancel) 后所有克隆都看到已取消。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 置位。幂等。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// 底层标志：给只认 `&AtomicBool` 的检查点（`project_historical_records` 的 bake 循环）。
    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
}

/// 离开作用域时置位 token。handler 持有它：正常返回时计算已结束，置位无害；
/// future 被中途丢弃（连接断开）时，阻塞线程借此在下一个检查点停下。
#[derive(Debug)]
pub struct CancelOnDrop(pub CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// 实时历史计算的 DFS 钩子：取消优先，其次元素上限。
///
/// e3d-model 在处理第 `visited + 1` 个元素之前调用 `should_stop(visited)`，所以
/// `visited >= max_elements` 时子树元素数必然超过 `max_elements`——与 Legacy 路
/// 「生成完再判 `report.visited > MAX_HISTORY_ELEMENTS`」是同一条边界，只是中途就停。
#[derive(Debug, Clone)]
pub struct HistoryHooks {
    cancel: CancelToken,
    max_elements: usize,
}

impl HistoryHooks {
    pub fn new(cancel: CancelToken, max_elements: usize) -> Self {
        Self {
            cancel,
            max_elements,
        }
    }
}

impl GenerationHooks for HistoryHooks {
    fn should_stop(&self, visited: usize) -> Option<Stop> {
        if self.cancel.is_cancelled() {
            Some(Stop::Cancelled)
        } else if visited >= self.max_elements {
            Some(Stop::LimitExceeded)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_on_drop_sets_every_clone_of_the_token() {
        let token = CancelToken::new();
        let observer = token.clone();
        {
            let _guard = CancelOnDrop(token);
            assert!(!observer.is_cancelled(), "guard alive: not cancelled yet");
        }
        assert!(observer.is_cancelled(), "guard dropped: token set");
        assert!(observer.flag().load(Ordering::Acquire));
    }

    #[test]
    fn cancel_is_idempotent_and_shared() {
        let token = CancelToken::new();
        let other = token.clone();
        other.cancel();
        other.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn hooks_pass_until_the_limit_would_be_exceeded() {
        let hooks = HistoryHooks::new(CancelToken::new(), 3);
        // 处理第 1..=3 个元素之前：visited = 0..=2，不停。
        for visited in 0..3 {
            assert_eq!(hooks.should_stop(visited), None, "visited={visited}");
        }
        // 要处理第 4 个：子树已超过 3 个。
        assert_eq!(hooks.should_stop(3), Some(Stop::LimitExceeded));
    }

    #[test]
    fn hooks_report_cancellation_before_the_limit() {
        let token = CancelToken::new();
        let hooks = HistoryHooks::new(token.clone(), 3);
        assert_eq!(hooks.should_stop(0), None);
        token.cancel();
        assert_eq!(hooks.should_stop(0), Some(Stop::Cancelled));
        // 取消与超限同时成立时报取消：调用方已不要结果，不该回 422。
        assert_eq!(hooks.should_stop(10), Some(Stop::Cancelled));
    }

    #[test]
    fn hooks_stop_a_real_cancelled_token_through_drop() {
        let token = CancelToken::new();
        let hooks = HistoryHooks::new(token.clone(), usize::MAX);
        drop(CancelOnDrop(token));
        assert_eq!(hooks.should_stop(0), Some(Stop::Cancelled));
    }
}
