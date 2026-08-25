//! Authority-scoped resident cache for committed catalogue definitions.
//!
//! RocksDB remains authoritative. Staging reads always bypass this cache.

use aios_core::RefnoEnum;
use aios_core::pdms_data::ScomInfo;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tokio::sync::{RwLock, watch};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AuthorityId {
    runtime_id: u64,
    diagnostic_hash: u64,
}

impl AuthorityId {
    fn new(runtime_id: u64, diagnostic_hash: u64) -> Self {
        Self {
            runtime_id,
            diagnostic_hash,
        }
    }

    #[cfg(test)]
    fn test(runtime_id: u64) -> Self {
        Self::new(runtime_id, runtime_id.wrapping_mul(31))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CataReadScope {
    Committed(AuthorityId),
    StagedPage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CataLoadError {
    Database(Arc<str>),
    CatalogueDefect(Arc<str>),
    Superseded,
    ShuttingDown,
}

impl std::fmt::Display for CataLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "database: {message}"),
            Self::CatalogueDefect(message) => write!(formatter, "catalogue defect: {message}"),
            Self::Superseded => formatter.write_str("catalogue load superseded by publication"),
            Self::ShuttingDown => formatter.write_str("catalogue cache is shutting down"),
        }
    }
}

impl std::error::Error for CataLoadError {}

#[derive(Clone)]
pub struct LoadedScomInfo {
    pub info: Arc<ScomInfo>,
    pub dependencies: Arc<[RefnoEnum]>,
    pub estimated_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CataInvalidation {
    #[default]
    None,
    Selective(Vec<RefnoEnum>),
    FullAuthority,
}

#[derive(Clone, Copy, Debug)]
pub struct CataCacheLimits {
    pub max_bytes: u64,
    pub max_entries: usize,
    pub max_entry_bytes: u64,
}

impl Default for CataCacheLimits {
    fn default() -> Self {
        Self {
            max_bytes: 33_554_432,
            max_entries: 16_384,
            max_entry_bytes: 4_194_304,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CataCacheSnapshot {
    pub hits: u64,
    pub miss_leaders: u64,
    pub waiters: u64,
    pub admission_bypass: u64,
    pub load_success: u64,
    pub load_db_error: u64,
    pub load_defect: u64,
    pub superseded: u64,
    pub resident_entries: usize,
    pub resident_bytes: u64,
    pub resident_high_water_bytes: u64,
    pub active_flights: usize,
    pub dependency_edges: usize,
    pub evictions: u64,
    pub selective_invalidations: u64,
    pub full_invalidations: u64,
    pub oversized_rejections: u64,
    pub load_latency_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    authority: AuthorityId,
    scom: RefnoEnum,
}

#[derive(Clone)]
struct CacheEntry {
    value: LoadedScomInfo,
    access_tick: u64,
}

type FlightResult = Result<LoadedScomInfo, CataLoadError>;

struct Flight {
    epoch: u64,
    receiver: watch::Receiver<Option<FlightResult>>,
}

#[derive(Default)]
struct Counters {
    hits: u64,
    miss_leaders: u64,
    waiters: u64,
    admission_bypass: u64,
    load_success: u64,
    load_db_error: u64,
    load_defect: u64,
    superseded: u64,
    evictions: u64,
    selective_invalidations: u64,
    full_invalidations: u64,
    oversized_rejections: u64,
    load_latency_ms: u64,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<CacheKey, CacheEntry>,
    flights: HashMap<CacheKey, Flight>,
    reverse_dependencies: HashMap<(AuthorityId, RefnoEnum), HashSet<CacheKey>>,
    resident_bytes: u64,
    resident_high_water_bytes: u64,
    tick: u64,
    counters: Counters,
}

struct AuthorityState {
    epoch: AtomicU64,
    publication: RwLock<()>,
}

impl Default for AuthorityState {
    fn default() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            publication: RwLock::new(()),
        }
    }
}

pub struct CataCache {
    limits: CataCacheLimits,
    state: Mutex<CacheState>,
    authorities: Mutex<HashMap<AuthorityId, Arc<AuthorityState>>>,
}

impl CataCache {
    pub fn new(limits: CataCacheLimits) -> Self {
        Self {
            limits,
            state: Mutex::new(CacheState::default()),
            authorities: Mutex::new(HashMap::new()),
        }
    }

    fn authority_state(&self, authority: AuthorityId) -> Arc<AuthorityState> {
        self.authorities
            .lock()
            .expect("CATA authority lock poisoned")
            .entry(authority)
            .or_insert_with(|| Arc::new(AuthorityState::default()))
            .clone()
    }

    pub async fn get_or_load<F, Fut>(
        self: &Arc<Self>,
        scope: CataReadScope,
        scom: RefnoEnum,
        loader: F,
    ) -> Result<Arc<ScomInfo>, CataLoadError>
    where
        F: Fn() -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<LoadedScomInfo, CataLoadError>> + Send + 'static,
    {
        let CataReadScope::Committed(authority) = scope else {
            return loader().await.map(|loaded| loaded.info);
        };
        if self.limits.max_bytes == 0 || self.limits.max_entries == 0 {
            self.state
                .lock()
                .expect("CATA cache lock poisoned")
                .counters
                .admission_bypass += 1;
            return loader().await.map(|loaded| loaded.info);
        }

        loop {
            let key = CacheKey { authority, scom };
            let authority_state = self.authority_state(authority);
            let epoch = authority_state.epoch.load(Ordering::Acquire);
            let mut receiver = {
                let mut state = self.state.lock().expect("CATA cache lock poisoned");
                state.tick = state.tick.wrapping_add(1);
                let tick = state.tick;
                if let Some(entry) = state.entries.get_mut(&key) {
                    entry.access_tick = tick;
                    let info = entry.value.info.clone();
                    state.counters.hits += 1;
                    return Ok(info);
                }
                if let Some(flight) = state.flights.get(&key) {
                    if flight.epoch == epoch {
                        let receiver = flight.receiver.clone();
                        state.counters.waiters += 1;
                        receiver
                    } else {
                        state.flights.remove(&key);
                        self.start_flight(
                            key,
                            epoch,
                            authority_state.clone(),
                            loader.clone(),
                            &mut state,
                        )
                    }
                } else {
                    self.start_flight(
                        key,
                        epoch,
                        authority_state.clone(),
                        loader.clone(),
                        &mut state,
                    )
                }
            };

            loop {
                if let Some(result) = receiver.borrow().clone() {
                    match result {
                        Err(CataLoadError::Superseded) => break,
                        Ok(loaded) => return Ok(loaded.info),
                        Err(error) => return Err(error),
                    }
                }
                if receiver.changed().await.is_err() {
                    return Err(CataLoadError::ShuttingDown);
                }
            }
        }
    }

    fn start_flight<F, Fut>(
        self: &Arc<Self>,
        key: CacheKey,
        epoch: u64,
        authority_state: Arc<AuthorityState>,
        loader: F,
        state: &mut CacheState,
    ) -> watch::Receiver<Option<FlightResult>>
    where
        F: Fn() -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<LoadedScomInfo, CataLoadError>> + Send + 'static,
    {
        let (sender, receiver) = watch::channel(None);
        state.flights.insert(
            key,
            Flight {
                epoch,
                receiver: receiver.clone(),
            },
        );
        state.counters.miss_leaders += 1;
        let cache = self.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut result = loader().await;
            let _publication = authority_state.publication.read().await;
            if authority_state.epoch.load(Ordering::Acquire) != epoch {
                result = Err(CataLoadError::Superseded);
            }
            let mut state = cache.state.lock().expect("CATA cache lock poisoned");
            state.counters.load_latency_ms = state
                .counters
                .load_latency_ms
                .saturating_add(started.elapsed().as_millis() as u64);
            if state
                .flights
                .get(&key)
                .is_some_and(|flight| flight.epoch == epoch)
            {
                state.flights.remove(&key);
            }
            match &result {
                Ok(loaded) => {
                    state.counters.load_success += 1;
                    cache.admit(key, loaded.clone(), &mut state);
                }
                Err(CataLoadError::Database(_)) => state.counters.load_db_error += 1,
                Err(CataLoadError::CatalogueDefect(_)) => state.counters.load_defect += 1,
                Err(CataLoadError::Superseded) => state.counters.superseded += 1,
                Err(CataLoadError::ShuttingDown) => {}
            }
            drop(state);
            let _ = sender.send(Some(result));
        });
        receiver
    }

    fn admit(&self, key: CacheKey, value: LoadedScomInfo, state: &mut CacheState) {
        if value.estimated_bytes > self.limits.max_entry_bytes
            || value.estimated_bytes > self.limits.max_bytes
        {
            state.counters.oversized_rejections += 1;
            return;
        }
        self.remove_entry(key, state);
        state.tick = state.tick.wrapping_add(1);
        let tick = state.tick;
        state.resident_bytes = state.resident_bytes.saturating_add(value.estimated_bytes);
        state.resident_high_water_bytes = state.resident_high_water_bytes.max(state.resident_bytes);
        for dependency in value.dependencies.iter().copied() {
            state
                .reverse_dependencies
                .entry((key.authority, dependency))
                .or_default()
                .insert(key);
        }
        state.entries.insert(
            key,
            CacheEntry {
                value,
                access_tick: tick,
            },
        );
        self.evict_to_low_water(state);
    }

    fn remove_entry(&self, key: CacheKey, state: &mut CacheState) {
        let Some(entry) = state.entries.remove(&key) else {
            return;
        };
        state.resident_bytes = state
            .resident_bytes
            .saturating_sub(entry.value.estimated_bytes);
        for dependency in entry.value.dependencies.iter().copied() {
            let reverse_key = (key.authority, dependency);
            if let Some(keys) = state.reverse_dependencies.get_mut(&reverse_key) {
                keys.remove(&key);
                if keys.is_empty() {
                    state.reverse_dependencies.remove(&reverse_key);
                }
            }
        }
    }

    fn evict_to_low_water(&self, state: &mut CacheState) {
        if state.entries.len() <= self.limits.max_entries
            && state.resident_bytes <= self.limits.max_bytes
        {
            return;
        }
        let target_entries = self.limits.max_entries.saturating_mul(9) / 10;
        let target_bytes = self.limits.max_bytes.saturating_mul(9) / 10;
        let mut candidates = state
            .entries
            .iter()
            .map(|(key, entry)| (*key, entry.access_tick))
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(_, tick)| *tick);
        for (key, _) in candidates {
            if state.entries.len() <= target_entries && state.resident_bytes <= target_bytes {
                break;
            }
            self.remove_entry(key, state);
            state.counters.evictions += 1;
        }
    }

    pub async fn publish_invalidation(
        &self,
        authority: AuthorityId,
        invalidation: CataInvalidation,
    ) {
        if invalidation == CataInvalidation::None {
            return;
        }
        let authority_state = self.authority_state(authority);
        let _publication = authority_state.publication.write().await;
        self.apply_invalidation(authority, invalidation, &authority_state);
    }

    /// Hold the publication writer across the authoritative commit and cache
    /// publication, so no loader can publish a pre-commit read in between.
    pub async fn commit_and_publish<F, T, E>(
        &self,
        authority: AuthorityId,
        invalidation: CataInvalidation,
        commit: F,
    ) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let authority_state = self.authority_state(authority);
        let _publication = authority_state.publication.write().await;
        let committed = commit.await?;
        self.apply_invalidation(authority, invalidation, &authority_state);
        Ok(committed)
    }

    fn apply_invalidation(
        &self,
        authority: AuthorityId,
        invalidation: CataInvalidation,
        authority_state: &AuthorityState,
    ) {
        if invalidation == CataInvalidation::None {
            return;
        }
        let mut state = self.state.lock().expect("CATA cache lock poisoned");
        match invalidation {
            CataInvalidation::None => return,
            CataInvalidation::Selective(refnos) => {
                let mut keys = HashSet::new();
                for refno in refnos {
                    if let Some(dependents) = state.reverse_dependencies.get(&(authority, refno)) {
                        keys.extend(dependents.iter().copied());
                    }
                }
                for key in keys {
                    self.remove_entry(key, &mut state);
                }
                state.counters.selective_invalidations += 1;
            }
            CataInvalidation::FullAuthority => {
                let keys = state
                    .entries
                    .keys()
                    .filter(|key| key.authority == authority)
                    .copied()
                    .collect::<Vec<_>>();
                for key in keys {
                    self.remove_entry(key, &mut state);
                }
                state.counters.full_invalidations += 1;
            }
        }
        authority_state.epoch.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> CataCacheSnapshot {
        let state = self.state.lock().expect("CATA cache lock poisoned");
        CataCacheSnapshot {
            hits: state.counters.hits,
            miss_leaders: state.counters.miss_leaders,
            waiters: state.counters.waiters,
            admission_bypass: state.counters.admission_bypass,
            load_success: state.counters.load_success,
            load_db_error: state.counters.load_db_error,
            load_defect: state.counters.load_defect,
            superseded: state.counters.superseded,
            resident_entries: state.entries.len(),
            resident_bytes: state.resident_bytes,
            resident_high_water_bytes: state.resident_high_water_bytes,
            active_flights: state.flights.len(),
            dependency_edges: state.reverse_dependencies.values().map(HashSet::len).sum(),
            evictions: state.counters.evictions,
            selective_invalidations: state.counters.selective_invalidations,
            full_invalidations: state.counters.full_invalidations,
            oversized_rejections: state.counters.oversized_rejections,
            load_latency_ms: state.counters.load_latency_ms,
        }
    }
}

static AUTHORITY_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CURRENT_AUTHORITY: OnceLock<Mutex<Option<AuthorityId>>> = OnceLock::new();
static GLOBAL_CACHE: OnceLock<Arc<CataCache>> = OnceLock::new();

fn diagnostic_hash() -> u64 {
    let option = crate::options::get_db_option_ext();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{option:?}").hash(&mut hasher);
    hasher.finish()
}

pub fn current_authority() -> AuthorityId {
    let mut current = CURRENT_AUTHORITY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("current CATA authority lock poisoned");
    *current.get_or_insert_with(|| {
        AuthorityId::new(
            AUTHORITY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            diagnostic_hash(),
        )
    })
}

pub async fn reset_authority() -> AuthorityId {
    let old = CURRENT_AUTHORITY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("current CATA authority lock poisoned")
        .as_ref()
        .copied();
    if let Some(old) = old {
        global_cache()
            .publish_invalidation(old, CataInvalidation::FullAuthority)
            .await;
    }
    let new = AuthorityId::new(
        AUTHORITY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        diagnostic_hash(),
    );
    *CURRENT_AUTHORITY
        .get()
        .expect("authority initialized")
        .lock()
        .expect("current CATA authority lock poisoned") = Some(new);
    new
}

pub fn active_read_scope() -> CataReadScope {
    if aios_core::staging::active_staging_reads().is_some() {
        CataReadScope::StagedPage
    } else {
        CataReadScope::Committed(current_authority())
    }
}

pub fn global_cache() -> &'static Arc<CataCache> {
    GLOBAL_CACHE.get_or_init(|| Arc::new(CataCache::new(crate::options::cata_cache_limits())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn loaded(marker: &str, bytes: u64, dependencies: Vec<RefnoEnum>) -> LoadedScomInfo {
        let mut info = ScomInfo::default();
        info.gtype = marker.into();
        LoadedScomInfo {
            info: Arc::new(info),
            dependencies: dependencies.into(),
            estimated_bytes: bytes,
        }
    }

    #[test]
    fn legacy_scom_cache_symbols_are_absent_from_main_paths() {
        assert!(!include_str!("resolve.rs").contains(concat!("SCOM_INFO_", "MAP")));
        assert!(!include_str!("../defines.rs").contains(concat!("CACHED_SCOM_INFO_", "MAP")));
    }

    #[tokio::test]
    async fn one_hundred_waiters_share_one_loader() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let authority = AuthorityId::test(1);
        let key = RefnoEnum::from("1/2");
        let tasks = (0..100).map(|_| {
            let cache = cache.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, move || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            Ok(loaded("shared", 64, vec![key]))
                        }
                    })
                    .await
                    .unwrap()
            })
        });
        for result in futures::future::join_all(tasks).await {
            assert_eq!(result.unwrap().gtype, "shared");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.snapshot().miss_leaders, 1);
    }

    #[tokio::test]
    async fn staged_reads_and_database_errors_are_not_admitted() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let key = RefnoEnum::from("1/3");
        for _ in 0..2 {
            let calls = calls.clone();
            cache
                .get_or_load(CataReadScope::StagedPage, key, move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(loaded("staged", 64, vec![key]))
                    }
                })
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.snapshot().resident_entries, 0);

        let authority = AuthorityId::test(2);
        for _ in 0..2 {
            assert!(
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, || async {
                        Err(CataLoadError::Database("fixture".into()))
                    })
                    .await
                    .is_err()
            );
        }
        assert_eq!(cache.snapshot().miss_leaders, 2);
    }

    #[tokio::test]
    async fn selective_invalidation_keeps_unrelated_scom() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let authority = AuthorityId::test(3);
        let first = RefnoEnum::from("1/10");
        let second = RefnoEnum::from("1/20");
        let dep_first = RefnoEnum::from("1/11");
        let dep_second = RefnoEnum::from("1/21");
        for (key, dep, marker) in [(first, dep_first, "a"), (second, dep_second, "b")] {
            cache
                .get_or_load(
                    CataReadScope::Committed(authority),
                    key,
                    move || async move { Ok(loaded(marker, 64, vec![dep])) },
                )
                .await
                .unwrap();
        }
        cache
            .publish_invalidation(authority, CataInvalidation::Selective(vec![dep_first]))
            .await;
        assert_eq!(cache.snapshot().resident_entries, 1);
        let value = cache
            .get_or_load(CataReadScope::Committed(authority), second, || async {
                panic!("unrelated entry should remain resident")
            })
            .await
            .unwrap();
        assert_eq!(value.gtype, "b");
    }

    #[tokio::test]
    async fn capacity_evicts_to_ninety_percent_and_held_arc_survives() {
        let cache = Arc::new(CataCache::new(CataCacheLimits {
            max_bytes: 100,
            max_entries: 10,
            max_entry_bytes: 100,
        }));
        let authority = AuthorityId::test(4);
        let held = cache
            .get_or_load(
                CataReadScope::Committed(authority),
                "1/1".into(),
                || async { Ok(loaded("held", 60, vec![])) },
            )
            .await
            .unwrap();
        cache
            .get_or_load(
                CataReadScope::Committed(authority),
                "1/2".into(),
                || async { Ok(loaded("new", 60, vec![])) },
            )
            .await
            .unwrap();
        assert!(cache.snapshot().resident_bytes <= 90);
        assert_eq!(held.gtype, "held");
    }

    #[tokio::test]
    async fn authorities_do_not_share_entries_or_flights() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let key = RefnoEnum::from("1/30");
        for authority in [AuthorityId::test(30), AuthorityId::test(31)] {
            let calls = calls.clone();
            cache
                .get_or_load(CataReadScope::Committed(authority), key, move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(loaded("isolated", 64, vec![key]))
                    }
                })
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.snapshot().resident_entries, 2);
    }

    #[tokio::test]
    async fn cancelling_the_leader_does_not_cancel_the_shared_loader() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let authority = AuthorityId::test(40);
        let key = RefnoEnum::from("1/40");
        let leader = {
            let cache = cache.clone();
            let calls = calls.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, move || {
                        let calls = calls.clone();
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.notify_one();
                            release.notified().await;
                            Ok(loaded("survived", 64, vec![key]))
                        }
                    })
                    .await
            })
        };
        started.notified().await;
        leader.abort();
        let waiter = {
            let cache = cache.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, || async {
                        panic!("the existing flight must be reused")
                    })
                    .await
                    .unwrap()
            })
        };
        release.notify_waiters();
        assert_eq!(waiter.await.unwrap().gtype, "survived");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn publication_supersedes_an_old_loader_and_retries() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Notify::new());
        let second_started = Arc::new(tokio::sync::Notify::new());
        let second_release = Arc::new(tokio::sync::Notify::new());
        let authority = AuthorityId::test(50);
        let key = RefnoEnum::from("1/50");
        let first_waiter = {
            let cache = cache.clone();
            let calls = calls.clone();
            let first_started = first_started.clone();
            let first_release = first_release.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, move || {
                        let call = calls.fetch_add(1, Ordering::SeqCst);
                        let first_started = first_started.clone();
                        let first_release = first_release.clone();
                        async move {
                            assert_eq!(call, 0, "the first waiter must not start another loader");
                            first_started.notify_one();
                            first_release.notified().await;
                            Ok(loaded("old", 64, vec![key]))
                        }
                    })
                    .await
                    .unwrap()
            })
        };
        first_started.notified().await;
        cache
            .publish_invalidation(authority, CataInvalidation::FullAuthority)
            .await;

        let second_waiter = {
            let cache = cache.clone();
            let calls = calls.clone();
            let second_started = second_started.clone();
            let second_release = second_release.clone();
            tokio::spawn(async move {
                cache
                    .get_or_load(CataReadScope::Committed(authority), key, move || {
                        let calls = calls.clone();
                        let second_started = second_started.clone();
                        let second_release = second_release.clone();
                        async move {
                            assert_eq!(calls.fetch_add(1, Ordering::SeqCst), 1);
                            second_started.notify_one();
                            second_release.notified().await;
                            Ok(loaded("new", 64, vec![key]))
                        }
                    })
                    .await
                    .unwrap()
            })
        };
        second_started.notified().await;
        assert_eq!(cache.snapshot().active_flights, 1);

        first_release.notify_waiters();
        tokio::task::yield_now().await;
        assert_eq!(
            cache.snapshot().active_flights,
            1,
            "an obsolete loader must not remove the replacement flight"
        );
        second_release.notify_waiters();

        assert_eq!(first_waiter.await.unwrap().gtype, "new");
        assert_eq!(second_waiter.await.unwrap().gtype, "new");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.snapshot().superseded, 1);
    }

    #[tokio::test]
    async fn zero_bytes_is_a_no_resident_cache_rollback() {
        let cache = Arc::new(CataCache::new(CataCacheLimits {
            max_bytes: 0,
            ..CataCacheLimits::default()
        }));
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let calls = calls.clone();
            cache
                .get_or_load(
                    CataReadScope::Committed(AuthorityId::test(60)),
                    "1/60".into(),
                    move || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(loaded("off", 64, vec![]))
                        }
                    },
                )
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.snapshot().resident_entries, 0);
        assert_eq!(cache.snapshot().admission_bypass, 2);
    }

    #[tokio::test]
    async fn failed_commit_preserves_the_old_generation() {
        let cache = Arc::new(CataCache::new(CataCacheLimits::default()));
        let authority = AuthorityId::test(70);
        let key = RefnoEnum::from("1/70");
        cache
            .get_or_load(
                CataReadScope::Committed(authority),
                key,
                move || async move { Ok(loaded("old", 64, vec![key])) },
            )
            .await
            .unwrap();
        let result: Result<(), &str> = cache
            .commit_and_publish(authority, CataInvalidation::FullAuthority, async {
                Err("commit failed")
            })
            .await;
        assert_eq!(result, Err("commit failed"));
        let value = cache
            .get_or_load(CataReadScope::Committed(authority), key, || async {
                panic!("failed commit must retain the old cache generation")
            })
            .await
            .unwrap();
        assert_eq!(value.gtype, "old");
        assert_eq!(cache.snapshot().full_invalidations, 0);
    }
}
