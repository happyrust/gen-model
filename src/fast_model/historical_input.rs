//! 只给历史用的生成输入缓存（实时历史几何查询，Requirement 7.5）。
//!
//! 当前模型那份输入走 `e3d_model_service::generation_input`（ADR-084，每库一条、按库数设上限）；
//! 历史查询来回拖动时要的是**同一库的相邻会话**，若共用那张表，每换一个 sesno 就把当前模型的输入替掉。
//! 所以另起一张表：键 `(dbnum, sesno, identity)`，容量 `model_history_input_max`（0–64，缺省 4，
//! 0 = 不缓存、每次现建），按最近使用淘汰。同键并发构建用 `DashMap<Key, Arc<OnceCell<V>>>`
//! 合并成一次：后到的调用方阻塞在同一个 `OnceCell` 上，等第一个建完直接拿结果。
//!
//! 值类型是泛型：生产里是 `GenerationInput`（由实时计算源实例化），单测不用开真文件。
//! 值应是廉价可克隆的句柄（`Arc` 包着的 `DbSet` / 索引）；被淘汰的那份由还握着它的调用方继续用完。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use dashmap::DashMap;
use once_cell::sync::OnceCell;
use serde::Serialize;

/// `(dbnum, sesno, identity)`：identity 与当前模型输入缓存同义（冻结源摘要或库文件规范路径）。
pub type HistoricalInputKey = (u32, u32, String);

/// `/health.model_cache.realtime_history.input_cache` 的两项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HistoricalInputStats {
    /// 此刻留着几份已建好的输入（建到一半的不算）。
    pub entries: usize,
    /// 生效的容量（0 = 不缓存）。
    pub capacity: usize,
}

/// 已建好条目的最近使用序号。只在这把锁里改，`entries.len()` 就是条目数。
#[derive(Default)]
struct Recency {
    entries: HashMap<HistoricalInputKey, u64>,
    next_tick: u64,
}

pub struct HistoricalInputLru<V> {
    capacity: usize,
    cells: DashMap<HistoricalInputKey, Arc<OnceCell<V>>>,
    recency: Mutex<Recency>,
}

impl<V: Clone> HistoricalInputLru<V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cells: DashMap::new(),
            recency: Mutex::new(Recency::default()),
        }
    }

    /// 容量取配置 `model_history_input_max`（启动时已夹到 0–64）。
    pub fn with_configured_capacity() -> Self {
        Self::new(crate::options::model_history_input_max())
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 命中直接返回并刷新最近使用；未命中调 `build` 建一份放进表里，超出容量时淘汰最久没用的一条。
    ///
    /// 同键并发只建一次：别的调用方阻塞等结果（`build` 本身不可中断，设计 §1.2）。
    /// `build` 失败不留条目，错误原样返回，下一次调用重建。容量 0 时每次都调 `build`、不存。
    pub fn get_or_build<E>(
        &self,
        key: HistoricalInputKey,
        build: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        if self.capacity == 0 {
            return build();
        }
        // 先把 cell 抄出来再放掉表的分片锁：构建可能要数秒，不能拿着分片锁建。
        let cell = self
            .cells
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        match cell.get_or_try_init(build) {
            Ok(value) => {
                let value = value.clone();
                let evicted = self.touch_and_evict(&key, &cell);
                // 被淘汰的值（`DbSet` 析构要关文件）在锁外 drop。
                drop(evicted);
                Ok(value)
            }
            Err(error) => {
                // 只摘掉自己这只还空着的 cell：别人此间已建成或换了新 cell 都不动。
                self.cells.remove_if(&key, |_, current| {
                    Arc::ptr_eq(current, &cell) && current.get().is_none()
                });
                Err(error)
            }
        }
    }

    /// 条数与容量（给 `/health`）。
    pub fn stats(&self) -> HistoricalInputStats {
        HistoricalInputStats {
            entries: self.lock_recency().entries.len(),
            capacity: self.capacity,
        }
    }

    fn lock_recency(&self) -> std::sync::MutexGuard<'_, Recency> {
        self.recency.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 记这一键刚被用到，再按容量淘汰别的键。返回被摘掉的 cell，调用方在锁外 drop。
    fn touch_and_evict(
        &self,
        key: &HistoricalInputKey,
        cell: &Arc<OnceCell<V>>,
    ) -> Vec<Arc<OnceCell<V>>> {
        let mut recency = self.lock_recency();
        // 建完到这里之间这只 cell 可能已被别的线程淘汰：那就不再记它，免得条目数里多一条表里没有的键。
        let still_cached = self
            .cells
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current.value(), cell));
        if !still_cached {
            return Vec::new();
        }
        recency.next_tick += 1;
        let tick = recency.next_tick;
        recency.entries.insert(key.clone(), tick);
        let mut evicted = Vec::new();
        while recency.entries.len() > self.capacity {
            let Some(oldest) = recency
                .entries
                .iter()
                .filter(|(candidate, _)| *candidate != key)
                .min_by_key(|(_, last_used)| **last_used)
                .map(|(candidate, _)| candidate.clone())
            else {
                break;
            };
            recency.entries.remove(&oldest);
            if let Some((_, removed)) = self.cells.remove(&oldest) {
                evicted.push(removed);
            }
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn key(dbnum: u32, sesno: u32) -> HistoricalInputKey {
        (dbnum, sesno, "identity".to_string())
    }

    /// 取一次，返回值与这次是否真的调了 `build`。
    fn fetch(lru: &HistoricalInputLru<String>, k: HistoricalInputKey) -> (String, bool) {
        let built = std::cell::Cell::new(false);
        let label = format!("{}/{}", k.0, k.1);
        let value = lru
            .get_or_build::<()>(k, || {
                built.set(true);
                Ok(label)
            })
            .unwrap();
        (value, built.get())
    }

    #[test]
    fn evicts_least_recently_used_and_hits_refresh_recency() {
        let lru = HistoricalInputLru::new(2);
        assert_eq!(fetch(&lru, key(1, 10)), ("1/10".to_string(), true));
        assert_eq!(fetch(&lru, key(1, 11)), ("1/11".to_string(), true));
        // 命中 1/10，它变成最近用过的；装 1/12 时出的应是 1/11。
        assert_eq!(fetch(&lru, key(1, 10)), ("1/10".to_string(), false));
        assert_eq!(fetch(&lru, key(1, 12)), ("1/12".to_string(), true));
        assert_eq!(
            lru.stats(),
            HistoricalInputStats {
                entries: 2,
                capacity: 2
            }
        );
        assert!(!fetch(&lru, key(1, 10)).1, "1/10 still cached");
        assert!(!fetch(&lru, key(1, 12)).1, "1/12 still cached");
        assert!(fetch(&lru, key(1, 11)).1, "1/11 was evicted and is rebuilt");
    }

    #[test]
    fn identity_is_part_of_the_key() {
        let lru = HistoricalInputLru::new(4);
        assert!(fetch(&lru, (1, 10, "a".to_string())).1);
        assert!(fetch(&lru, (1, 10, "b".to_string())).1);
        assert!(!fetch(&lru, (1, 10, "a".to_string())).1);
        assert_eq!(lru.stats().entries, 2);
    }

    #[test]
    fn capacity_zero_builds_every_time_and_stores_nothing() {
        let lru = HistoricalInputLru::new(0);
        assert!(fetch(&lru, key(1, 10)).1);
        assert!(fetch(&lru, key(1, 10)).1);
        assert_eq!(
            lru.stats(),
            HistoricalInputStats {
                entries: 0,
                capacity: 0
            }
        );
    }

    #[test]
    fn failed_build_is_not_cached() {
        let lru: HistoricalInputLru<String> = HistoricalInputLru::new(2);
        let failed = lru.get_or_build(key(1, 10), || Err("scan failed"));
        assert_eq!(failed, Err("scan failed"));
        assert_eq!(lru.stats().entries, 0);
        assert!(fetch(&lru, key(1, 10)).1, "retry builds again");
        assert_eq!(lru.stats().entries, 1);
    }

    #[test]
    fn concurrent_requests_for_one_key_build_once() {
        const THREADS: usize = 8;
        let lru = Arc::new(HistoricalInputLru::new(4));
        let builds = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let (lru, builds, barrier) = (lru.clone(), builds.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    lru.get_or_build::<()>(key(7, 3), || {
                        builds.fetch_add(1, Ordering::SeqCst);
                        // 让其余线程都赶上这只还空着的 cell。
                        thread::sleep(Duration::from_millis(100));
                        Ok(Arc::new(42u32))
                    })
                    .unwrap()
                })
            })
            .collect();
        let values: Vec<Arc<u32>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert!(
            values.iter().all(|v| Arc::ptr_eq(v, &values[0])),
            "all callers share one value"
        );
        assert_eq!(
            lru.stats(),
            HistoricalInputStats {
                entries: 1,
                capacity: 4
            }
        );
    }

    #[test]
    fn stats_count_entries_up_to_capacity() {
        let lru = HistoricalInputLru::new(3);
        assert_eq!(
            lru.stats(),
            HistoricalInputStats {
                entries: 0,
                capacity: 3
            }
        );
        for sesno in 1..=5 {
            fetch(&lru, key(2, sesno));
            assert_eq!(lru.stats().entries, (sesno as usize).min(3));
        }
        assert_eq!(lru.capacity(), 3);
    }
}
