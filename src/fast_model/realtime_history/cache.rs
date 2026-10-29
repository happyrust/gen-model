//! 实时历史几何的结果缓存（design §1.2 第 4、8 步，Requirement 6.17、7.2–7.5）。
//!
//! 键 `(dbnum, 单元 refno, 解析后 sesno, 源文件集指纹)`，值是序列化好的 200 响应（体字节 + 头部），
//! 命中时原样回放，所以命中与未命中逐字节一致（7.3）。
//!
//! - 容量 0–1024，0 = 停用（不存、不命中）；满时先淘汰最久未使用的一条再写（7.2）。
//! - 只写 HTTP 200：错误、超时、断开的结果不进表（6.17、7.2）。
//! - 指纹一变整表清空：表里只留当前指纹的条目，条目数自然只算当前指纹（7.4）。
//!   算到一半指纹变了的结果（键带旧指纹）也不写，免得旧条目回到表里。
//! - 命中数在 [`RealtimeResultCache::get`] 命中时 +1；未命中数由调用方在拿到并发名额后
//!   调 [`RealtimeResultCache::record_miss`] +1（忙拒绝两者都不计，7.1）。两者只增不减（7.5）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

/// 容量上限（Requirement 7.2）。
pub const REALTIME_CACHE_MAX_CAPACITY: usize = 1024;

/// 唯一写入的状态码。
pub const CACHEABLE_STATUS: u16 = 200;

/// `(dbnum, unit, sesno, fingerprint)`；`unit` 是 refno 的 `(ref0, ref1)`，
/// `fingerprint` 是 `source_set_fingerprint` 的 64 位 hex。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RealtimeCacheKey {
    pub dbnum: u32,
    pub unit: (u32, u32),
    pub sesno: u32,
    pub fingerprint: String,
}

/// 一份已序列化的 200 History_Geometry_Response：体字节与要回的头部。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedHistoryResponse {
    pub sesno: u32,
    pub session_time: Option<String>,
    pub geometry_key: String,
    /// `ETag`、`X-Sesno`、`X-Session-Time`、`Cache-Control`，按此顺序。
    pub headers: Vec<(&'static str, String)>,
    /// 已序列化的 JSON 体。
    pub body: Vec<u8>,
}

impl CachedHistoryResponse {
    /// 由解析后会话与序列化好的体拼出响应；头部在这里一次定好，命中与未命中共用。
    pub fn new(
        sesno: u32,
        session_time: Option<String>,
        geometry_key: String,
        body: Vec<u8>,
    ) -> Self {
        let headers = vec![
            ("ETag", format!("\"{geometry_key}\"")),
            ("X-Sesno", sesno.to_string()),
            // 会话无时间时值为空字符串（design §1.2 响应头）。
            ("X-Session-Time", session_time.clone().unwrap_or_default()),
            ("Cache-Control", "no-cache".to_string()),
        ];
        Self {
            sesno,
            session_time,
            geometry_key,
            headers,
            body,
        }
    }
}

/// `/health.model_cache.realtime_history` 的缓存四项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RealtimeCacheStats {
    /// 当前指纹下的条目数。
    pub entries: usize,
    /// 生效容量（0 = 停用）。
    pub capacity: usize,
    pub hits: u64,
    pub misses: u64,
}

struct Slot<V> {
    value: Arc<V>,
    last_used: u64,
}

struct State<V> {
    entries: HashMap<RealtimeCacheKey, Slot<V>>,
    /// 最近一次见到的指纹；表里所有条目都属于它。
    current_fingerprint: Option<String>,
    next_tick: u64,
}

impl<V> State<V> {
    /// 指纹与当前不同就整表清空并换成新指纹。返回被清掉的值，调用方在锁外 drop。
    fn adopt_fingerprint(&mut self, fingerprint: &str) -> Vec<Arc<V>> {
        if self.current_fingerprint.as_deref() == Some(fingerprint) {
            return Vec::new();
        }
        self.current_fingerprint = Some(fingerprint.to_string());
        self.entries.drain().map(|(_, slot)| slot.value).collect()
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }
}

/// 进程内 LRU。值类型泛型：生产里是 [`CachedHistoryResponse`]，参考模型测试可换成别的。
pub struct RealtimeResultCache<V = CachedHistoryResponse> {
    capacity: usize,
    state: Mutex<State<V>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<V> RealtimeResultCache<V> {
    /// 容量夹到 0–1024。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.min(REALTIME_CACHE_MAX_CAPACITY),
            state: Mutex::new(State {
                entries: HashMap::new(),
                current_fingerprint: None,
                next_tick: 0,
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// 容量取配置 `model_history_realtime_cache`（0–1024，缺省 32）。
    pub fn with_configured_capacity() -> Self {
        Self::new(crate::options::model_history_realtime_cache())
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 查一次。键的指纹与当前不同时先整表清空（旧指纹条目从此不再命中）。
    /// 命中刷新最近使用并把命中数 +1；未命中不计数（见 [`Self::record_miss`]）。
    pub fn get(&self, key: &RealtimeCacheKey) -> Option<Arc<V>> {
        if self.capacity == 0 {
            return None;
        }
        let (found, cleared) = {
            let mut state = self.lock();
            let cleared = state.adopt_fingerprint(&key.fingerprint);
            let tick = state.tick();
            let found = state.entries.get_mut(key).map(|slot| {
                slot.last_used = tick;
                slot.value.clone()
            });
            (found, cleared)
        };
        drop(cleared);
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// 未命中且拿到并发名额、开始计算时调用：未命中数 +1。
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// 写入一次计算结果。只有 `status == 200`、容量 > 0、键的指纹仍是当前指纹（或表还没见过指纹）
    /// 时才写；满时先淘汰最久未使用的一条。返回是否写入。
    pub fn insert(&self, key: RealtimeCacheKey, status: u16, value: Arc<V>) -> bool {
        if status != CACHEABLE_STATUS || self.capacity == 0 {
            return false;
        }
        let evicted = {
            let mut state = self.lock();
            match state.current_fingerprint.as_deref() {
                None => state.current_fingerprint = Some(key.fingerprint.clone()),
                // 计算期间指纹已变：这份结果属于旧源文件，不写。
                Some(current) if current != key.fingerprint => return false,
                Some(_) => {}
            }
            let mut evicted = Vec::new();
            if !state.entries.contains_key(&key) {
                while state.entries.len() >= self.capacity {
                    let Some(oldest) = state
                        .entries
                        .iter()
                        .min_by_key(|(_, slot)| slot.last_used)
                        .map(|(k, _)| k.clone())
                    else {
                        break;
                    };
                    if let Some(slot) = state.entries.remove(&oldest) {
                        evicted.push(slot.value);
                    }
                }
            }
            let last_used = state.tick();
            if let Some(old) = state.entries.insert(key, Slot { value, last_used }) {
                evicted.push(old.value);
            }
            evicted
        };
        drop(evicted);
        true
    }

    /// 条目数、容量、命中数、未命中数（给 `/health`）。
    pub fn stats(&self) -> RealtimeCacheStats {
        RealtimeCacheStats {
            entries: self.lock().entries.len(),
            capacity: self.capacity,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<V>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FP_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn key(sesno: u32, fingerprint: &str) -> RealtimeCacheKey {
        RealtimeCacheKey {
            dbnum: 1112,
            unit: (24384, 23257),
            sesno,
            fingerprint: fingerprint.to_string(),
        }
    }

    fn response(sesno: u32) -> Arc<CachedHistoryResponse> {
        Arc::new(CachedHistoryResponse::new(
            sesno,
            Some("2024-03-01T08:00:00+08:00".to_string()),
            format!("g:{sesno:032x}"),
            format!("{{\"sesno\":{sesno}}}").into_bytes(),
        ))
    }

    #[test]
    fn hit_replays_identical_body_and_headers() {
        let cache = RealtimeResultCache::new(4);
        assert!(cache.get(&key(10, FP_A)).is_none());
        let stored = response(10);
        assert!(cache.insert(key(10, FP_A), 200, stored.clone()));
        let hit = cache.get(&key(10, FP_A)).expect("hit");
        assert_eq!(hit.body, stored.body);
        assert_eq!(hit.headers, stored.headers);
        assert_eq!(hit.headers[0], ("ETag", format!("\"g:{:032x}\"", 10)));
        assert_eq!(hit.headers[1], ("X-Sesno", "10".to_string()));
    }

    #[test]
    fn session_without_time_has_empty_header() {
        let r = CachedHistoryResponse::new(3, None, "g:00".to_string(), Vec::new());
        assert_eq!(r.headers[2], ("X-Session-Time", String::new()));
        assert_eq!(r.headers[3], ("Cache-Control", "no-cache".to_string()));
    }

    #[test]
    fn only_200_is_stored() {
        let cache = RealtimeResultCache::new(4);
        for status in [404, 413, 422, 503, 504] {
            assert!(!cache.insert(key(10, FP_A), status, response(10)));
        }
        assert!(cache.get(&key(10, FP_A)).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn evicts_least_recently_used_and_never_exceeds_capacity() {
        let cache = RealtimeResultCache::new(2);
        cache.insert(key(1, FP_A), 200, response(1));
        cache.insert(key(2, FP_A), 200, response(2));
        // 命中 1，最久未使用变成 2。
        assert!(cache.get(&key(1, FP_A)).is_some());
        cache.insert(key(3, FP_A), 200, response(3));
        assert_eq!(cache.stats().entries, 2);
        assert!(cache.get(&key(2, FP_A)).is_none(), "2 evicted");
        assert!(cache.get(&key(1, FP_A)).is_some());
        assert!(cache.get(&key(3, FP_A)).is_some());
        // 同键重写不淘汰别的条目。
        cache.insert(key(3, FP_A), 200, response(3));
        assert_eq!(cache.stats().entries, 2);
        assert!(cache.get(&key(1, FP_A)).is_some());
    }

    #[test]
    fn fingerprint_change_clears_table_and_rejects_stale_writes() {
        let cache = RealtimeResultCache::new(4);
        cache.insert(key(1, FP_A), 200, response(1));
        cache.insert(key(2, FP_A), 200, response(2));
        assert_eq!(cache.stats().entries, 2);
        // 指纹变化后的第一次查询：旧条目全部失效，不再计入条目数。
        assert!(cache.get(&key(1, FP_B)).is_none());
        assert_eq!(cache.stats().entries, 0);
        // 旧指纹下算完的结果不写回。
        assert!(!cache.insert(key(2, FP_A), 200, response(2)));
        assert_eq!(cache.stats().entries, 0);
        assert!(cache.insert(key(1, FP_B), 200, response(1)));
        assert!(cache.get(&key(1, FP_B)).is_some());
        // 指纹改回 A 时同样整表清空，旧 A 条目早已不在。
        assert!(cache.get(&key(1, FP_A)).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn capacity_zero_disables_cache() {
        let cache = RealtimeResultCache::new(0);
        assert!(!cache.insert(key(1, FP_A), 200, response(1)));
        assert!(cache.get(&key(1, FP_A)).is_none());
        assert_eq!(
            cache.stats(),
            RealtimeCacheStats {
                entries: 0,
                capacity: 0,
                hits: 0,
                misses: 0
            }
        );
    }

    #[test]
    fn capacity_is_clamped_to_1024() {
        let cache: RealtimeResultCache = RealtimeResultCache::new(5000);
        assert_eq!(cache.capacity(), REALTIME_CACHE_MAX_CAPACITY);
    }

    #[test]
    fn hits_and_misses_only_grow() {
        let cache = RealtimeResultCache::new(2);
        assert!(cache.get(&key(1, FP_A)).is_none());
        cache.record_miss();
        cache.insert(key(1, FP_A), 200, response(1));
        cache.get(&key(1, FP_A));
        cache.get(&key(1, FP_A));
        // 指纹变化清表不回退计数。
        cache.get(&key(1, FP_B));
        cache.record_miss();
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses), (2, 2));
        assert_eq!(stats.entries, 0);
    }
}
