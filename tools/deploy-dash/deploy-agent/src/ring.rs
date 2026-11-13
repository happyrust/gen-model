//! 带行号的环形缓冲：日志（每来源 5,000 行）、提示（200 条）、任务输出共用。
//!
//! 游标语义（PROTOCOL.md「游标」一节）：行号从 1 起单调递增；`after = n` 要的是 `n` 之后的行；
//! `next` = 这次给到的最后一行（没有新行时等于 `after`）；`after` 比最老的一行还老 → `reset`，从最老的给起；
//! `after` 比最新的还新（代理重启过、编号从头来了）→ 也是 `reset`；不给 `after` → 给最后 `max` 行（`reset`）。

use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Ring<T> {
    items: VecDeque<(u64, T)>,
    cap: usize,
    next_n: u64,
}

/// 一次查询的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window<T> {
    pub from: u64,
    pub next: u64,
    pub reset: bool,
    pub dropped: u64,
    pub more: bool,
    pub oldest: u64,
    pub newest: u64,
    pub items: Vec<(u64, T)>,
}

impl<T: Clone> Ring<T> {
    pub fn new(cap: usize) -> Self {
        Self { items: VecDeque::with_capacity(cap.min(1024)), cap: cap.max(1), next_n: 1 }
    }

    /// 放一行，返回它的行号。满了挤掉最老的。
    pub fn push(&mut self, item: T) -> u64 {
        let n = self.next_n;
        self.next_n += 1;
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back((n, item));
        n
    }

    pub fn oldest(&self) -> u64 {
        self.items.front().map(|(n, _)| *n).unwrap_or(0)
    }

    /// 最新一行的行号（一行都没进过是 0）。
    pub fn newest(&self) -> u64 {
        self.next_n - 1
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn query(&self, after: Option<u64>, max: usize) -> Window<T> {
        let max = max.max(1);
        let oldest = self.oldest();
        let newest = self.newest();
        let (start, reset, dropped) = match after {
            None => (self.items.len().saturating_sub(max), true, 0),
            Some(a) if a > newest => (0, true, 0),
            Some(a) if !self.items.is_empty() && a + 1 < oldest => (0, true, oldest - a - 1),
            Some(a) => (self.items.iter().position(|(n, _)| *n > a).unwrap_or(self.items.len()), false, 0),
        };
        let items: Vec<(u64, T)> = self.items.iter().skip(start).take(max).cloned().collect();
        let last = items.last().map(|(n, _)| *n);
        let next = match (last, after) {
            (Some(n), _) => n,
            (None, Some(a)) if !reset => a,
            (None, _) => newest,
        };
        Window {
            from: items.first().map(|(n, _)| *n).unwrap_or(0),
            next,
            reset,
            dropped,
            more: last.is_some_and(|n| n < newest),
            oldest,
            newest,
            items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_with(cap: usize, count: u64) -> Ring<u64> {
        let mut r = Ring::new(cap);
        for i in 1..=count {
            assert_eq!(r.push(i * 10), i);
        }
        r
    }

    #[test]
    fn after_returns_only_newer_lines_and_next_is_the_last_one() {
        let r = ring_with(5000, 12);
        let w = r.query(Some(9), 800);
        assert_eq!(w.items.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![10, 11, 12]);
        assert_eq!((w.from, w.next, w.reset, w.more, w.dropped), (10, 12, false, false, 0));
        let w = r.query(Some(12), 800);
        assert!(w.items.is_empty());
        assert_eq!((w.from, w.next, w.reset), (0, 12, false), "追平了：next 原样回来，不 reset");
    }

    #[test]
    fn max_limits_and_more_says_there_is_more() {
        let r = ring_with(5000, 100);
        let w = r.query(Some(0), 30);
        assert_eq!((w.from, w.next, w.more, w.reset), (1, 30, true, false));
        let w = r.query(Some(w.next), 800);
        assert_eq!((w.from, w.next, w.more), (31, 100, false));
    }

    #[test]
    fn ring_keeps_only_cap_lines_and_old_cursor_resets() {
        let r = ring_with(5000, 5003);
        assert_eq!(r.len(), 5000);
        assert_eq!((r.oldest(), r.newest()), (4, 5003));
        // 客户端手里到第 1 行，2、3 两行已被挤掉
        let w = r.query(Some(1), 800);
        assert!(w.reset);
        assert_eq!(w.dropped, 2);
        assert_eq!(w.from, 4);
        // 刚好接得上（手里到第 3 行）不算 reset
        let w = r.query(Some(3), 800);
        assert!(!w.reset);
        assert_eq!(w.from, 4);
    }

    #[test]
    fn cursor_from_the_future_means_agent_restarted() {
        let r = ring_with(5000, 3);
        let w = r.query(Some(12_990), 800);
        assert!(w.reset);
        assert_eq!((w.from, w.next), (1, 3));
    }

    #[test]
    fn no_cursor_gives_the_tail() {
        let r = ring_with(5000, 1000);
        let w = r.query(None, 800);
        assert!(w.reset);
        assert_eq!((w.from, w.next, w.items.len()), (201, 1000, 800));
        let empty: Ring<u64> = Ring::new(10);
        let w = empty.query(None, 800);
        assert_eq!((w.from, w.next, w.reset, w.items.len()), (0, 0, true, 0));
        let w = empty.query(Some(0), 800);
        assert_eq!((w.next, w.reset), (0, false));
    }
}
