//! CATA 依赖闭包的进程级解析缓存（`docs/plans/2026-09-15-cata-dependency-closure-cache-dev-plan.md` D1–D3）。
//!
//! 缓存的是目录元素的**原始事实**——完整属性表 + 成员 + 出向边 + owner / noun（[`CachedCataElement`]），
//! 不是闭包结果：扩边规则（容器白名单 / 排除库 / owner 链 / 只收 CATA）仍由 `CataClosureResolver::resolve()`
//! 按**本次** cfg 施加，四个调用点 cfg 的差别不会污染缓存。
//!
//! - 键 `(project, dbnum, refno)`；每库一份身份戳 [`CatalogueIdentity`]（文件指纹 + 目录会话号），
//!   戳变了整库条目**立刻**作废（D2：戳相等才命中，不做「更早 sesno 兼容」）。
//! - 条目 + 字节双上限，LRU（tick 序 `BTreeMap`，O(log n)），字节是**估算**（[`approx_bytes`]），上限管的是量级。
//! - 任一上限为 `0` = 关：`get` 直接 miss、`insert` 不写；调用方先看 [`ClosureCache::enabled`] 再决定要不要碰锁。
//!
//! RocksDB 仍是唯一事实，这里省的只是重复劳动。**不是** 2026-09-07（ADR-064 Q2）删掉的 `CataDepCache`——那是
//! 惰性兜底路上「哪些 refno 已入库」的存在性缓存；落库出口仍只有 `cata_dependency` 一个。
//! S2 把它接进 `resolve()`；S4 用 `options.rs` 的 `cata_dependency_cache_max_entries / _max_bytes` 换掉 [`CacheLimits::default`]。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use once_cell::sync::Lazy;

use super::cata_closure::ParsedCataEle;

/// 一个目录元素解析一次得到的全部东西：BFS 扩边要的（`parsed`）+ 渲染依赖行要的（`att`）。
#[derive(Debug, Clone)]
pub(crate) struct CachedCataElement {
    pub parsed: ParsedCataEle,
    pub att: NamedAttrMap,
    /// 入表时估一次（[`approx_bytes`]），淘汰按它记账。
    pub approx_bytes: usize,
}

impl CachedCataElement {
    pub(crate) fn new(parsed: ParsedCataEle, att: NamedAttrMap) -> Self {
        let approx_bytes = approx_bytes(&parsed, &att);
        Self {
            parsed,
            att,
            approx_bytes,
        }
    }
}

/// 一个目录库此刻的文件身份：`locator_fingerprint`（size:mtime，含 extract 父文件）+ 文件头的目标会话号。
/// 两者都对上才算同一份文件；任一变了该库缓存整体作废。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogueIdentity {
    pub fingerprint: String,
    pub target_sesno: u32,
}

/// 上限；任一为 `0` 即关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CacheLimits {
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl CacheLimits {
    /// 关掉（`0`）。
    pub(crate) const OFF: CacheLimits = CacheLimits {
        max_entries: 0,
        max_bytes: 0,
    };

    pub(crate) fn enabled(&self) -> bool {
        self.max_entries > 0 && self.max_bytes > 0
    }
}

impl Default for CacheLimits {
    /// plan D8 / Q1（暂定，S4 接 `options.rs`）：三库闭包并集 ≈ 29,000 个元素、每行 SQL ≈ 1 KB，
    /// 200,000 条 / 256 MiB 是「整份 AMS 目录 × 6」的量级。
    fn default() -> Self {
        Self {
            max_entries: 200_000,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

/// 计数（进程累计）+ 当前占用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    /// 超上限按 LRU 淘汰的条目数。
    pub evicted: u64,
    /// 因库身份变化整库作废的条目数。
    pub retired: u64,
    pub entries: usize,
    pub bytes: usize,
}

type Key = (u32, u32, RefU64); // (project_id, dbnum, refno)

struct Entry {
    value: Arc<CachedCataElement>,
    tick: u64,
}

/// 进程级解析缓存本体；通过 [`closure_cache`] 拿全局那一份，或 [`ClosureCache::new`] 建一份给单测。
pub(crate) struct ClosureCache {
    limits: CacheLimits,
    project_ids: HashMap<String, u32>,
    /// `(project_id, dbnum)` → 该库入表时的身份。
    identities: HashMap<(u32, u32), CatalogueIdentity>,
    entries: HashMap<Key, Entry>,
    /// 最近使用序：tick → key（tick 单调递增，命中时换一个新 tick）。
    lru: BTreeMap<u64, Key>,
    next_tick: u64,
    bytes: usize,
    stats: CacheStats,
}

impl ClosureCache {
    pub(crate) fn new(limits: CacheLimits) -> Self {
        Self {
            limits,
            project_ids: HashMap::new(),
            identities: HashMap::new(),
            entries: HashMap::new(),
            lru: BTreeMap::new(),
            next_tick: 0,
            bytes: 0,
            stats: CacheStats::default(),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.limits.enabled()
    }

    pub(crate) fn limits(&self) -> CacheLimits {
        self.limits
    }

    pub(crate) fn stats(&self) -> CacheStats {
        let mut stats = self.stats.clone();
        stats.entries = self.entries.len();
        stats.bytes = self.bytes;
        stats
    }

    fn project_id(&mut self, project: &str) -> u32 {
        if let Some(id) = self.project_ids.get(project) {
            return *id;
        }
        let id = self.project_ids.len() as u32;
        self.project_ids.insert(project.to_string(), id);
        id
    }

    /// 一次 resolve 碰到一个库时先调：身份与缓存里记的不一致（或第一次见）就整库作废、记下新身份。
    /// 之后对该库的 `get` / `insert` 都以这份身份为准。关着时什么都不做。
    pub(crate) fn begin_catalogue(
        &mut self,
        project: &str,
        dbnum: u32,
        identity: &CatalogueIdentity,
    ) {
        if !self.enabled() {
            return;
        }
        let pid = self.project_id(project);
        match self.identities.get(&(pid, dbnum)) {
            Some(known) if known == identity => {}
            Some(_) => {
                self.retire_catalogue(pid, dbnum);
                self.identities.insert((pid, dbnum), identity.clone());
            }
            None => {
                self.identities.insert((pid, dbnum), identity.clone());
            }
        }
    }

    /// 命中刷最近使用序。没经 [`Self::begin_catalogue`] 登记身份的库一律 miss（不知道它是哪一版文件）。
    pub(crate) fn get(
        &mut self,
        project: &str,
        dbnum: u32,
        refno: RefU64,
    ) -> Option<Arc<CachedCataElement>> {
        if !self.enabled() {
            return None;
        }
        let pid = self.project_id(project);
        if !self.identities.contains_key(&(pid, dbnum)) {
            self.stats.misses += 1;
            return None;
        }
        let key = (pid, dbnum, refno);
        let tick = self.bump_tick();
        match self.entries.get_mut(&key) {
            Some(entry) => {
                self.lru.remove(&entry.tick);
                entry.tick = tick;
                self.lru.insert(tick, key);
                self.stats.hits += 1;
                Some(entry.value.clone())
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// 写入（同键覆盖）；库没登记身份就不写；超任一上限从最旧淘汰到线内。
    /// 单条比字节上限还大的元素不进缓存（进了也立刻被淘汰，白搭）。
    pub(crate) fn insert(&mut self, project: &str, dbnum: u32, value: CachedCataElement) {
        if !self.enabled() {
            return;
        }
        let pid = self.project_id(project);
        if !self.identities.contains_key(&(pid, dbnum)) {
            debug_assert!(false, "insert before begin_catalogue: dbnum={dbnum}");
            return;
        }
        if value.approx_bytes > self.limits.max_bytes {
            return;
        }
        let key = (pid, dbnum, value.parsed.refno);
        if let Some(old) = self.entries.remove(&key) {
            self.lru.remove(&old.tick);
            self.bytes -= old.value.approx_bytes;
        }
        let tick = self.bump_tick();
        self.bytes += value.approx_bytes;
        self.entries.insert(
            key,
            Entry {
                value: Arc::new(value),
                tick,
            },
        );
        self.lru.insert(tick, key);
        self.stats.inserts += 1;
        self.evict_to_limits();
    }

    /// 全清（测试 / 换库）。身份也忘掉。
    pub(crate) fn clear(&mut self) {
        self.identities.clear();
        self.entries.clear();
        self.lru.clear();
        self.bytes = 0;
    }

    fn bump_tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    fn evict_to_limits(&mut self) {
        while self.entries.len() > self.limits.max_entries || self.bytes > self.limits.max_bytes {
            let Some((&tick, &key)) = self.lru.iter().next() else {
                break;
            };
            self.lru.remove(&tick);
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.value.approx_bytes;
                self.stats.evicted += 1;
            }
        }
    }

    fn retire_catalogue(&mut self, pid: u32, dbnum: u32) {
        let doomed: Vec<Key> = self
            .entries
            .keys()
            .filter(|(p, d, _)| *p == pid && *d == dbnum)
            .copied()
            .collect();
        for key in doomed {
            if let Some(entry) = self.entries.remove(&key) {
                self.lru.remove(&entry.tick);
                self.bytes -= entry.value.approx_bytes;
                self.stats.retired += 1;
            }
        }
    }
}

/// 一个条目大约占多少字节：键 + 属性表（键名 + 值的堆部分）+ 边表。**是估算**——上限管的是量级，
/// S5 用峰值 RSS 增量对一次，偏差 > 2× 再修。
pub(crate) fn approx_bytes(parsed: &ParsedCataEle, att: &NamedAttrMap) -> usize {
    const ENTRY_OVERHEAD: usize = 96; // Key + Entry + Arc 头 + HashMap / BTreeMap 槽位，粗算
    const MAP_NODE_OVERHEAD: usize = 32; // BTreeMap 节点里一格 (String, NamedAttrValue) 的固定开销
    let mut total = ENTRY_OVERHEAD
        + std::mem::size_of::<ParsedCataEle>()
        + parsed.noun_name.len()
        + (parsed.outbound.len() + parsed.children.len()) * std::mem::size_of::<RefU64>();
    for (name, value) in &att.map {
        total += MAP_NODE_OVERHEAD + name.len() + std::mem::size_of::<NamedAttrValue>();
        total += match value {
            NamedAttrValue::StringType(s)
            | NamedAttrValue::ElementType(s)
            | NamedAttrValue::WordType(s) => s.len(),
            NamedAttrValue::StringArrayType(items) => items
                .iter()
                .map(|s| s.len() + std::mem::size_of::<String>())
                .sum(),
            NamedAttrValue::F32VecType(items) => items.len() * std::mem::size_of::<f32>(),
            NamedAttrValue::IntArrayType(items) => items.len() * std::mem::size_of::<i32>(),
            NamedAttrValue::BoolArrayType(items) => items.len(),
            NamedAttrValue::RefU64Array(items) => items.len() * 16,
            _ => 0,
        };
    }
    total
}

static CLOSURE_CACHE: Lazy<Mutex<ClosureCache>> =
    Lazy::new(|| Mutex::new(ClosureCache::new(CacheLimits::default())));

/// 全局那一份。锁内只做查表 / 写表，解析在锁外。
pub(crate) fn closure_cache() -> MutexGuard<'static, ClosureCache> {
    CLOSURE_CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refu(dbnum: u64, n: u64) -> RefU64 {
        RefU64((dbnum << 32) | n)
    }

    fn identity(fp: &str, sesno: u32) -> CatalogueIdentity {
        CatalogueIdentity {
            fingerprint: fp.to_string(),
            target_sesno: sesno,
        }
    }

    fn element(dbnum: u32, n: u64, padding: usize) -> CachedCataElement {
        let refno = refu(dbnum as u64, n);
        let mut att = NamedAttrMap::default();
        att.map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        att.map
            .insert("TYPE".into(), NamedAttrValue::StringType("SCOM".into()));
        att.map.insert(
            "NAME".into(),
            NamedAttrValue::StringType(format!("/SCOM-{n}-{}", "x".repeat(padding))),
        );
        let parsed = ParsedCataEle {
            refno,
            owner: refu(dbnum as u64, 0),
            noun: 1,
            noun_name: "SCOM".into(),
            outbound: vec![refu(dbnum as u64, 900 + n)],
            children: Vec::new(),
        };
        CachedCataElement::new(parsed, att)
    }

    #[test]
    fn the_cache_keeps_one_entry_per_project_dbnum_refno() {
        let mut cache = ClosureCache::new(CacheLimits::default());
        cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
        cache.insert("ams", 5052, element(5052, 1, 0));
        cache.insert("ams", 5052, element(5052, 1, 8)); // 同键覆盖
        cache.insert("ams", 5052, element(5052, 2, 0));
        let stats = cache.stats();
        assert_eq!(stats.entries, 2, "同键只存一份");
        assert_eq!(stats.inserts, 3);
        let hit = cache.get("ams", 5052, refu(5052, 1)).expect("命中");
        assert!(
            hit.att.map.get("NAME").is_some_and(
                |v| matches!(v, NamedAttrValue::StringType(s) if s.ends_with("xxxxxxxx"))
            ),
            "覆盖后拿到的是后写的那份"
        );
        assert_eq!(stats.bytes, cache.stats().bytes, "命中不改字节账");
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn a_different_project_dbnum_fingerprint_or_sesno_is_a_miss() {
        let mut cache = ClosureCache::new(CacheLimits::default());
        cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
        cache.insert("ams", 5052, element(5052, 1, 0));
        let refno = refu(5052, 1);
        assert!(cache.get("ams", 5052, refno).is_some());
        assert!(
            cache.get("aps", 5052, refno).is_none(),
            "别的项目不共享（也没登记身份）"
        );
        assert!(cache.get("ams", 5053, refno).is_none(), "别的库不共享");
        // 指纹变了：整库作废，同键 miss。
        cache.begin_catalogue("ams", 5052, &identity("2:2", 189));
        assert!(
            cache.get("ams", 5052, refno).is_none(),
            "文件指纹变了就 miss"
        );
        cache.insert("ams", 5052, element(5052, 1, 0));
        assert!(cache.get("ams", 5052, refno).is_some());
        // 只有会话号变了也一样。
        cache.begin_catalogue("ams", 5052, &identity("2:2", 190));
        assert!(
            cache.get("ams", 5052, refno).is_none(),
            "目录会话号变了就 miss"
        );
        assert_eq!(cache.stats().retired, 2);
    }

    #[test]
    fn eviction_is_lru_and_honours_both_the_entry_and_the_byte_limit() {
        // 条目上限 3。
        let mut cache = ClosureCache::new(CacheLimits {
            max_entries: 3,
            max_bytes: usize::MAX / 2,
        });
        cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
        for n in 1..=3 {
            cache.insert("ams", 5052, element(5052, n, 0));
        }
        assert!(
            cache.get("ams", 5052, refu(5052, 1)).is_some(),
            "刷新 1 的使用序"
        );
        cache.insert("ams", 5052, element(5052, 4, 0));
        assert!(
            cache.get("ams", 5052, refu(5052, 2)).is_none(),
            "最旧的 2 被淘汰"
        );
        assert!(
            cache.get("ams", 5052, refu(5052, 1)).is_some(),
            "刚用过的 1 留着"
        );
        assert!(cache.get("ams", 5052, refu(5052, 4)).is_some());
        assert_eq!(cache.stats().entries, 3);
        assert_eq!(cache.stats().evicted, 1);

        // 字节上限：装得下两条小的，第三条进来挤掉最旧的；单条超上限的根本不进。
        let one = element(5052, 1, 0).approx_bytes;
        let mut cache = ClosureCache::new(CacheLimits {
            max_entries: 100,
            max_bytes: one * 2 + one / 2,
        });
        cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
        cache.insert("ams", 5052, element(5052, 1, 0));
        cache.insert("ams", 5052, element(5052, 2, 0));
        assert_eq!(cache.stats().entries, 2);
        cache.insert("ams", 5052, element(5052, 3, 0));
        assert_eq!(cache.stats().entries, 2, "按字节淘汰到线内");
        assert!(
            cache.get("ams", 5052, refu(5052, 1)).is_none(),
            "最旧的 1 被淘汰"
        );
        assert!(cache.stats().bytes <= one * 2 + one / 2);
        cache.insert("ams", 5052, element(5052, 9, 4096));
        assert!(
            cache.get("ams", 5052, refu(5052, 9)).is_none(),
            "单条比字节上限大的不进缓存"
        );
        assert_eq!(cache.stats().entries, 2, "也不挤掉别人");
    }

    #[test]
    fn a_changed_catalogue_identity_retires_the_whole_dbnum_but_not_its_neighbours() {
        let mut cache = ClosureCache::new(CacheLimits::default());
        cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
        cache.begin_catalogue("ams", 7002, &identity("7:7", 98));
        for n in 1..=5 {
            cache.insert("ams", 5052, element(5052, n, 0));
        }
        cache.insert("ams", 7002, element(7002, 1, 0));
        let before = cache.stats().bytes;
        cache.begin_catalogue("ams", 5052, &identity("1:2", 189));
        let stats = cache.stats();
        assert_eq!(stats.retired, 5, "5052 五条整库作废");
        assert_eq!(stats.entries, 1, "7002 那条不受影响");
        assert!(stats.bytes < before && stats.bytes > 0, "字节账跟着减");
        assert!(cache.get("ams", 7002, refu(7002, 1)).is_some());
        // 同一身份再登记一次不作废。
        cache.begin_catalogue("ams", 7002, &identity("7:7", 98));
        assert_eq!(cache.stats().retired, 5);
    }

    #[test]
    fn zero_limits_switch_the_cache_off() {
        for limits in [
            CacheLimits::OFF,
            CacheLimits {
                max_entries: 0,
                max_bytes: 1024,
            },
            CacheLimits {
                max_entries: 10,
                max_bytes: 0,
            },
        ] {
            let mut cache = ClosureCache::new(limits);
            assert!(!cache.enabled(), "{limits:?}");
            cache.begin_catalogue("ams", 5052, &identity("1:1", 189));
            cache.insert("ams", 5052, element(5052, 1, 0));
            assert!(cache.get("ams", 5052, refu(5052, 1)).is_none());
            let stats = cache.stats();
            assert_eq!(stats.entries, 0);
            assert_eq!(stats.bytes, 0);
            assert_eq!(stats.inserts, 0);
            assert_eq!(stats.hits + stats.misses, 0, "关着连账都不记");
        }
        assert!(ClosureCache::new(CacheLimits::default()).enabled());
    }

    #[test]
    fn approx_bytes_grows_with_attribute_payload_and_edges() {
        let small = element(5052, 1, 0);
        let padded = element(5052, 1, 1000);
        assert!(
            padded.approx_bytes >= small.approx_bytes + 1000,
            "字符串负载要算进去: {} vs {}",
            padded.approx_bytes,
            small.approx_bytes
        );
        let mut many_edges = element(5052, 1, 0);
        many_edges.parsed.outbound = (0..100).map(|n| refu(5052, 1000 + n)).collect();
        let recounted = approx_bytes(&many_edges.parsed, &many_edges.att);
        assert!(
            recounted >= small.approx_bytes + 99 * std::mem::size_of::<RefU64>(),
            "边表要算进去: {recounted} vs {}",
            small.approx_bytes
        );
        assert!(
            small.approx_bytes < 2048,
            "三个属性的元素不该估到 2 KB 以上: {}",
            small.approx_bytes
        );
    }
}
