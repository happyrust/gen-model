//! `/model/records` 冷读用的只读连接（A6 第二步，2026-11-11；spec §4.5.2）。
//!
//! # 为什么要另开连接
//!
//! 本进程到 SurrealDB 只有 `aios_core::SUL_DB` 一条 WebSocket 连接。SurrealDB 服务端把**同一条连接**上的请求放在
//! 同一个任务里轮着跑（它的 `rpc/websocket.rs`：读循环把每条消息的处理 push 进一只 `FuturesUnordered`、就地 poll），
//! 所以一条连接上同时发多少句都只用得上一个核。前端按 ZONE 6 路并发取记录，到了库这一头仍是一句一句地算。
//!
//! 2026-11-11 真库实测（ams7333 的 SITE `23717/2`：531 个 ZONE、79,409 行，6 个 ZONE 同时在途，照本模块的语句形状发）：
//!
//! | 连接的用法 | 取完整个 SITE |
//! |---|---|
//! | 全挤在一条连接上（原来的做法） | 7.9 s |
//! | 4 条连接，一条同一时刻只给一个请求 | 2.4–2.8 s |
//! | 6 条连接 | 1.6–1.7 s |
//!
//! 「一条连接、6 句同时发」与「一条连接、6 句依次发」一样慢（2.29–2.38 s 对 2.24–2.36 s，240 个 ZONE），
//! 而「6 条连接各发一句」是 0.51–0.54 s——卡的就是连接，不是库的算力。
//!
//! # 这里做什么
//!
//! 另开几条连接，**只**给 `/model/records` 的冷读用。一条连接同一时刻只借给一个读（[`Lease`]），所以库里同时在算的
//! 冷读语句数不会超过连接数——原来的做法没有这个上限，前端并发几路库里就同时摊开几句。
//!
//! - 连接数：环境变量 [`CONNECTIONS_ENV`]，缺省 [`DEFAULT_CONNECTIONS`]，上限 [`MAX_CONNECTIONS`]；`0` = 不开，
//!   一切照旧走 `SUL_DB`（退回原来做法的开关）。开几条、以及按连接数发放的借出额度都在
//!   [`concurrency`](super::concurrency) 里定（`src/fast_model/` 的并发宽度只在那里定，见它的模块文档末节）；
//!   本模块只管连接本身。
//! - 只在持久层是外部那台库（`store_mode = rocksdb`）时开：内嵌 kv-mem 没有第二条连接可言，另开一条连到的是另一只空库。
//! - 只读：连接建好之后只做 `use` + `signin`，不灌函数、不修数据；借出去的连接只会被传给
//!   [`model_record_reader`] 的两个读函数，那里只有 `SELECT`。增量批次的写入仍然全部走 `SUL_DB`，不经过这里——
//!   冷读挪走以后，写入也不必再和大读挤同一条连接。
//! - 出了岔子就退回 `SUL_DB`：开不出来、等连接等太久、在读连接上读失败，都照原来的路再读一遍，
//!   所以请求拿到的结果不会比没有这个模块时差。连着失败几次就整池收掉，过一会儿再开。
//!
//! 第一次有冷读要用时才建连接，不动启动序列。
//!
//! [`model_record_reader`]: super::model_record_reader

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;
use surrealdb::opt::auth::Root;
use surrealdb::{Surreal, engine::any::Any};

use super::concurrency::{
    ReadLeaseGate, ReadLeasePermit, configured_read_connections as configured_connections,
};
use super::model_memory_store::{ModelMemoryStore, ProjectionScope};
use super::model_record_reader::{self as reader, ModelRecord, RecordWindow};

/// 读连接开几条的环境变量、缺省值、上限与取值规则。定义在 [`concurrency`](super::concurrency)（并发宽度只在那里定），
/// 这里照原来的名字转出来。
pub use super::concurrency::{
    DEFAULT_READ_CONNECTIONS as DEFAULT_CONNECTIONS, MAX_READ_CONNECTIONS as MAX_CONNECTIONS,
    READ_CONNECTIONS_ENV as CONNECTIONS_ENV, read_connections_from as connections_from,
};

/// 等一条空闲连接最多等多久。平时等的是别的读（几十到几百毫秒）；等到这个数说明连接都卡住了，这次改走 `SUL_DB`。
const LEASE_WAIT_LIMIT: Duration = Duration::from_secs(30);

/// 建一池连接最多用多久。库不在的时候不让第一个请求陪着干等。
const OPEN_LIMIT: Duration = Duration::from_secs(10);

/// 连着失败这么多次就把整池收掉。
const FAILURES_BEFORE_RETIRE: u32 = 3;

/// 池子收掉（或开不出来）之后隔多久再试。
const REOPEN_AFTER: Duration = Duration::from_secs(60);

/// 每条读连接的请求通道容量。一条连接同一时刻只有一个读在用，留一点余量给 SDK 自己的保活。
const CHANNEL_CAPACITY: usize = 64;

/// 持久层是不是外部那台库。进程启动后读一次（介质在运行中不会变）。
fn store_is_external() -> bool {
    static EXTERNAL: OnceLock<bool> = OnceLock::new();
    *EXTERNAL.get_or_init(|| crate::options::store_mode().is_durable())
}

/// 该不该开读连接：要了至少一条，而且持久层是外部那台库。
fn pool_allowed(wanted: usize, external_store: bool) -> bool {
    wanted > 0 && external_store
}

/// 一池读连接。
struct Pool {
    connections: Vec<Surreal<Any>>,
    /// 没借出去的连接的下标。
    free: StdMutex<Vec<usize>>,
    /// 借出额度，宽度 = 连接数：拿到一份就一定拿得到一条空闲连接。
    leases: ReadLeaseGate,
    failures_in_a_row: AtomicU32,
}

impl Pool {
    fn from_connections(connections: Vec<Surreal<Any>>) -> Arc<Self> {
        let size = connections.len();
        Arc::new(Self {
            connections,
            free: StdMutex::new((0..size).collect()),
            leases: ReadLeaseGate::with_width(size),
            failures_in_a_row: AtomicU32::new(0),
        })
    }

    /// 连上配置里那台库，开 `size` 条。任何一条连不上就整池不要——半池连接只会让人以为开关生效了。
    async fn open(size: usize) -> anyhow::Result<Arc<Self>> {
        let connecting = futures::future::try_join_all((0..size).map(|_| open_connection()));
        let connections = tokio::time::timeout(OPEN_LIMIT, connecting)
            .await
            .map_err(|_| anyhow::anyhow!("{} 秒内没连上", OPEN_LIMIT.as_secs()))??;
        Ok(Self::from_connections(connections))
    }

    fn size(&self) -> usize {
        self.connections.len()
    }

    /// 借一条连接；都借出去了就等，最多等 `limit`。`None` = 等到头了。
    async fn lease(self: &Arc<Self>, limit: Duration) -> Option<Lease> {
        let permit = self.leases.acquire(limit).await?;
        let index = self.free.lock().ok()?.pop()?;
        Some(Lease {
            pool: self.clone(),
            index,
            _permit: permit,
        })
    }
}

/// 一条只读连接：与 `aios_core::init_surreal` 同一套连接参数（同一个地址、同样开 AST 载荷、同一个 ns / db、同一个账号），
/// 但不做它后面那两件**写库**的事（灌函数、修 `DB.STYP`）——函数与数据在库里，谁连上去都看得见。
pub(super) async fn open_connection() -> anyhow::Result<Surreal<Any>> {
    let option = aios_core::get_db_option();
    let db: Surreal<Any> = Surreal::init();
    let config = surrealdb::opt::Config::default().ast_payload();
    db.connect((option.get_version_db_conn_str(), config))
        .with_capacity(CHANNEL_CAPACITY)
        .await?;
    db.use_ns(&option.surreal_ns)
        .use_db(&option.project_name)
        .await?;
    db.signin(Root {
        username: &option.v_user,
        password: &option.v_password,
    })
    .await?;
    Ok(db)
}

/// 借出去的一条读连接。丢掉就还回去。
struct Lease {
    pool: Arc<Pool>,
    index: usize,
    /// 字段在 `Drop::drop` 之后才析构：连接先回空闲表，额度后放——下一个拿到额度的一定拿得到连接。
    _permit: ReadLeasePermit,
}

impl Lease {
    fn db(&self) -> &Surreal<Any> {
        &self.pool.connections[self.index]
    }

    /// 这次读成了。
    fn succeeded(self) {
        self.pool.failures_in_a_row.store(0, Ordering::Relaxed);
    }

    /// 这次读在读连接上失败了。连着失败够多次就把整池收掉。
    fn failed(self, error: &anyhow::Error) {
        let failures = self.pool.failures_in_a_row.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!(
            "[model-read-pool] 读连接上的冷读失败（连续第 {failures} 次），这次改走共享连接重读: {error:#}"
        );
        if failures >= FAILURES_BEFORE_RETIRE {
            retire(&self.pool, "读连接连着失败");
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut free) = self.pool.free.lock() {
            free.push(self.index);
        }
    }
}

/// 池子的现状。
#[derive(Default)]
struct Gate {
    pool: Option<Arc<Pool>>,
    /// 在这之前不再尝试开池（上次开不出来，或者刚收掉）。
    closed_until: Option<Instant>,
}

enum GateView {
    Open(Arc<Pool>),
    /// 还没开过，或者歇够了：可以开。
    Empty,
    /// 歇着：这段时间一律走 `SUL_DB`。
    Closed,
}

impl Gate {
    fn view(&self, now: Instant) -> GateView {
        match (&self.pool, self.closed_until) {
            (Some(pool), _) => GateView::Open(pool.clone()),
            (None, Some(until)) if now < until => GateView::Closed,
            (None, _) => GateView::Empty,
        }
    }

    fn install(&mut self, pool: Arc<Pool>) {
        self.pool = Some(pool);
        self.closed_until = None;
    }

    fn close(&mut self, now: Instant) {
        self.pool = None;
        self.closed_until = Some(now + REOPEN_AFTER);
    }

    /// 收掉 `pool`——只在它还是现役那一池时才收（别人已经换过一池就不动）。回是否真收了。
    fn retire(&mut self, pool: &Arc<Pool>, now: Instant) -> bool {
        let current = self
            .pool
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, pool));
        if current {
            self.close(now);
        }
        current
    }
}

fn gate() -> &'static StdMutex<Gate> {
    static GATE: OnceLock<StdMutex<Gate>> = OnceLock::new();
    GATE.get_or_init(|| StdMutex::new(Gate::default()))
}

/// 建池这一步一次只让一个请求做，别的等它。
fn opening() -> &'static tokio::sync::Mutex<()> {
    static OPENING: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    OPENING.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn gate_view() -> GateView {
    match gate().lock() {
        Ok(gate) => gate.view(Instant::now()),
        Err(_) => GateView::Closed,
    }
}

fn retire(pool: &Arc<Pool>, why: &str) {
    let retired = gate()
        .lock()
        .is_ok_and(|mut gate| gate.retire(pool, Instant::now()));
    if retired {
        eprintln!(
            "[model-read-pool] {why}，收掉这 {} 条读连接；{} 秒内冷读走共享连接，之后重开",
            pool.size(),
            REOPEN_AFTER.as_secs()
        );
    }
}

/// 现役的那一池；没有就建。`None` = 这次不用读连接（没开、歇着、建不出来）。
async fn current_pool() -> Option<Arc<Pool>> {
    let wanted = configured_connections();
    if !pool_allowed(wanted, store_is_external()) {
        return None;
    }
    match gate_view() {
        GateView::Open(pool) => return Some(pool),
        GateView::Closed => return None,
        GateView::Empty => {}
    }
    let _opening = opening().lock().await;
    // 等锁的工夫别人可能已经建好了，或者刚试过没建成。
    match gate_view() {
        GateView::Open(pool) => return Some(pool),
        GateView::Closed => return None,
        GateView::Empty => {}
    }
    match Pool::open(wanted).await {
        Ok(pool) => {
            println!(
                "[model-read-pool] /model/records 的冷读另开了 {} 条只读连接（{CONNECTIONS_ENV}，缺省 {DEFAULT_CONNECTIONS}，0 = 不开）",
                pool.size()
            );
            if let Ok(mut gate) = gate().lock() {
                gate.install(pool.clone());
            }
            Some(pool)
        }
        Err(error) => {
            eprintln!(
                "[model-read-pool] 读连接没开出来，冷读照旧走共享连接，{} 秒后再试: {error:#}",
                REOPEN_AFTER.as_secs()
            );
            if let Ok(mut gate) = gate().lock() {
                gate.close(Instant::now());
            }
            None
        }
    }
}

/// 一次读走的是哪条路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadVia {
    /// 根在进程内投影里，没碰库。
    Memory,
    /// `SUL_DB`：没开读连接，或者读连接这次用不上。
    Shared,
    /// 读连接；带着这一池的连接数。
    Pool(usize),
}

/// 一次读的去向与等连接的时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadReport {
    pub via: ReadVia,
    pub waited: Duration,
}

/// 一个请求里若干次读的汇总，给 `Server-Timing` 的 `conn` 那一格用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadTally {
    pool_reads: u32,
    shared_reads: u32,
    pool_size: usize,
    waited: Duration,
}

impl ReadTally {
    pub fn add(&mut self, report: ReadReport) {
        match report.via {
            ReadVia::Memory => {}
            ReadVia::Shared => self.shared_reads += 1,
            ReadVia::Pool(size) => {
                self.pool_reads += 1;
                self.pool_size = size;
            }
        }
        self.waited += report.waited;
    }

    /// 等读连接一共等了多久。
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// 读库走的是哪条路：`pool-<连接数>` / `shared` / `mixed`（两条路都走过，多半是读连接失败后重读）/
    /// `none`（这个请求没读库）。
    pub fn label(&self) -> String {
        match (self.pool_reads > 0, self.shared_reads > 0) {
            (true, false) => format!("pool-{}", self.pool_size),
            (false, true) => "shared".to_string(),
            (true, true) => "mixed".to_string(),
            (false, false) => "none".to_string(),
        }
    }
}

fn root_is_resident(root: RefnoEnum) -> bool {
    let packed = root.refno().0 as u64;
    let root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    ModelMemoryStore::global()
        .receipt(ProjectionScope::Current, root)
        .is_some()
}

/// 给 `root` 的这次读备一条连接。投影里有回执的根不碰库，不占连接。
async fn lease_for(root: RefnoEnum) -> (Option<Lease>, ReadReport) {
    if root_is_resident(root) {
        return (
            None,
            ReadReport {
                via: ReadVia::Memory,
                waited: Duration::ZERO,
            },
        );
    }
    let shared = |waited| ReadReport {
        via: ReadVia::Shared,
        waited,
    };
    let Some(pool) = current_pool().await else {
        return (None, shared(Duration::ZERO));
    };
    let started = Instant::now();
    match pool.lease(LEASE_WAIT_LIMIT).await {
        Some(lease) => {
            let report = ReadReport {
                via: ReadVia::Pool(pool.size()),
                waited: started.elapsed(),
            };
            (Some(lease), report)
        }
        None => {
            eprintln!(
                "[model-read-pool] 等了 {} 秒没等到空闲的读连接，这次改走共享连接",
                LEASE_WAIT_LIMIT.as_secs()
            );
            let failures = pool.failures_in_a_row.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= FAILURES_BEFORE_RETIRE {
                retire(&pool, "读连接一直借不出来");
            }
            (None, shared(started.elapsed()))
        }
    }
}

/// [`model_record_reader::model_records_of_root`] 的取连接版：冷读优先走读连接，读连接用不上或读失败就走 `SUL_DB`。
///
/// [`model_record_reader::model_records_of_root`]: super::model_record_reader::model_records_of_root
pub async fn model_records_of_root(
    root: RefnoEnum,
) -> anyhow::Result<(Vec<ModelRecord>, ReadReport)> {
    let (lease, mut report) = lease_for(root).await;
    if let Some(lease) = lease {
        match reader::model_records_of_root(lease.db(), root).await {
            Ok(records) => {
                lease.succeeded();
                return Ok((records, report));
            }
            Err(error) => {
                lease.failed(&error);
                report.via = ReadVia::Shared;
            }
        }
    }
    let records = reader::model_records_of_root(&aios_core::SUL_DB, root).await?;
    Ok((records, report))
}

/// [`model_record_reader::model_records_window`] 的取连接版，规则同 [`model_records_of_root`]。
///
/// 读连接上失败后重读是安全的：窗口的续读进度只在一页读成之后才往前挪，失败的那一页不留痕。
///
/// [`model_record_reader::model_records_window`]: super::model_record_reader::model_records_window
pub async fn model_records_window(
    root: RefnoEnum,
    skip: usize,
    limit: usize,
) -> anyhow::Result<(RecordWindow, ReadReport)> {
    let (lease, mut report) = lease_for(root).await;
    if let Some(lease) = lease {
        match reader::model_records_window(lease.db(), root, skip, limit).await {
            Ok(window) => {
                lease.succeeded();
                return Ok((window, report));
            }
            Err(error) => {
                lease.failed(&error);
                report.via = ReadVia::Shared;
            }
        }
    }
    let window = reader::model_records_window(&aios_core::SUL_DB, root, skip, limit).await?;
    Ok((window, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连接数的取值规则（缺省 4、0 = 不开、上限 16）随宽度的决定搬到了 `concurrency`，细的那条测试在那边
    /// （`the_read_connection_count_defaults_conservatively_and_zero_switches_the_pool_off`）。这里只确认照原名
    /// 转出来的还是那几个。
    #[test]
    fn the_connection_count_rules_are_the_ones_decided_in_concurrency() {
        assert_eq!(CONNECTIONS_ENV, "AIOS_MODEL_READ_CONNECTIONS");
        assert_eq!((DEFAULT_CONNECTIONS, MAX_CONNECTIONS), (4, 16));
        assert_eq!(connections_from(None), DEFAULT_CONNECTIONS);
        assert_eq!(connections_from(Some("0")), 0, "0 = 不开，照旧只用共享连接");
        assert_eq!(connections_from(Some("999")), MAX_CONNECTIONS);
    }

    #[test]
    fn the_pool_only_opens_against_the_external_store() {
        assert!(pool_allowed(4, true));
        assert!(!pool_allowed(0, true), "开关关着");
        // 内嵌 kv-mem：另开一条连接连到的是另一只空库，读回来全是空。
        assert!(!pool_allowed(4, false));
    }

    async fn memory_pool(size: usize) -> Arc<Pool> {
        let mut connections = Vec::with_capacity(size);
        for _ in 0..size {
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("test").use_db("read_pool").await.unwrap();
            connections.push(db);
        }
        Pool::from_connections(connections)
    }

    #[tokio::test]
    async fn a_connection_is_lent_to_one_read_at_a_time_and_comes_back_when_dropped() {
        let pool = memory_pool(2).await;
        let first = pool.lease(Duration::from_millis(50)).await.expect("第一条");
        let second = pool.lease(Duration::from_millis(50)).await.expect("第二条");
        assert_ne!(first.index, second.index, "两个读不共用一条连接");
        assert!(
            pool.lease(Duration::from_millis(50)).await.is_none(),
            "都借出去了：等到头也借不到第三条"
        );

        // 借到的连接是通的。
        let mut response = first.db().query("RETURN 1;").await.unwrap().check().unwrap();
        let one: Option<i64> = response.take(0).unwrap();
        assert_eq!(one, Some(1));

        let returned = first.index;
        drop(first);
        let third = pool
            .lease(Duration::from_millis(50))
            .await
            .expect("还回来的那条");
        assert_eq!(third.index, returned);
        drop(second);
        drop(third);
        assert_eq!(pool.free.lock().unwrap().len(), 2, "用完全数归还");
        assert_eq!(pool.leases.available(), 2);
    }

    #[tokio::test]
    async fn a_waiting_read_gets_the_connection_as_soon_as_it_is_returned() {
        let pool = memory_pool(1).await;
        let held = pool.lease(Duration::from_millis(50)).await.expect("唯一一条");
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.lease(Duration::from_secs(5)).await.is_some() })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "连接还没还，后来的读在等");
        drop(held);
        assert!(waiter.await.unwrap(), "一还就借到");
    }

    #[tokio::test]
    async fn repeated_failures_retire_the_pool_and_a_success_clears_the_count() {
        let pool = memory_pool(1).await;
        let mut gate = Gate::default();
        let now = Instant::now();
        assert!(matches!(gate.view(now), GateView::Empty));
        gate.install(pool.clone());
        assert!(matches!(gate.view(now), GateView::Open(_)));

        // 失败两次、成功一次：计数清零，池子还在。
        for _ in 0..2 {
            pool.failures_in_a_row.fetch_add(1, Ordering::Relaxed);
        }
        pool.lease(Duration::from_millis(50))
            .await
            .expect("连接")
            .succeeded();
        assert_eq!(pool.failures_in_a_row.load(Ordering::Relaxed), 0);

        // 收掉之后歇一阵：这段时间既不用它也不重开，歇够了才允许重开。
        assert!(gate.retire(&pool, now));
        assert!(matches!(gate.view(now), GateView::Closed));
        assert!(matches!(
            gate.view(now + REOPEN_AFTER - Duration::from_secs(1)),
            GateView::Closed
        ));
        assert!(matches!(gate.view(now + REOPEN_AFTER), GateView::Empty));

        // 已经换过一池：迟到的「收掉旧池」不动新池。
        let newer = memory_pool(1).await;
        gate.install(newer);
        assert!(!gate.retire(&pool, now));
        assert!(matches!(gate.view(now), GateView::Open(_)));
    }

    #[test]
    fn the_tally_names_which_connection_served_the_request() {
        let read = |via| ReadReport {
            via,
            waited: Duration::from_millis(2),
        };
        let mut none = ReadTally::default();
        none.add(read(ReadVia::Memory));
        assert_eq!(none.label(), "none", "只读了投影，没碰库");

        let mut pooled = ReadTally::default();
        pooled.add(read(ReadVia::Pool(4)));
        pooled.add(read(ReadVia::Pool(4)));
        assert_eq!(pooled.label(), "pool-4");
        assert_eq!(pooled.waited(), Duration::from_millis(4));

        let mut shared = ReadTally::default();
        shared.add(read(ReadVia::Shared));
        assert_eq!(shared.label(), "shared");

        let mut mixed = pooled;
        mixed.add(read(ReadVia::Shared));
        assert_eq!(mixed.label(), "mixed");
    }

    /// 真库只读对拍（`--ignored`）：同一批根，一条连接依次读一遍当基准，再用读连接池并发读一遍，逐根把记录序列化成
    /// JSON 逐字比；顺手报三种用法的墙钟（一条连接依次 / 一条连接并发 / 读连接池并发）。只发 `SELECT`。
    ///
    /// 环境变量：`AIOS_READ_POOL_PROBE_ROOTS` = 一行一个根（`a/b` 或 `a_b`）的文件；`AIOS_READ_POOL_PROBE_SIZE` = 连接数
    /// （缺省 [`DEFAULT_CONNECTIONS`]）；`AIOS_READ_POOL_PROBE_INFLIGHT` = 同时在途几个根（缺省 6，即前端的并发数）。
    /// 连的是 `DbOption.toml`（或 `DB_OPTION_FILE`）里那台库；连接参数不打印。
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "requires the configured SurrealDB; read-only"]
    async fn live_probe_pooled_cold_reads_match_the_single_connection() {
        let Ok(roots_file) = std::env::var("AIOS_READ_POOL_PROBE_ROOTS") else {
            eprintln!("AIOS_READ_POOL_PROBE_ROOTS 没设，跳过");
            return;
        };
        let size = connections_from(std::env::var("AIOS_READ_POOL_PROBE_SIZE").ok().as_deref());
        let inflight = std::env::var("AIOS_READ_POOL_PROBE_INFLIGHT")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(6);
        let roots: Vec<RefnoEnum> = std::fs::read_to_string(&roots_file)
            .expect("根清单")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| RefnoEnum::from(line.replace('/', "_").as_str()))
            .collect();
        assert!(!roots.is_empty(), "根清单是空的");

        async fn read_json(db: &Surreal<Any>, root: RefnoEnum) -> (usize, String) {
            let records = reader::model_records_of_root(db, root)
                .await
                .unwrap_or_else(|error| panic!("读 {root} 失败: {error:#}"));
            (records.len(), serde_json::to_string(&records).unwrap())
        }

        // 基准：一条连接，一根一根读。
        let single = Arc::new(open_connection().await.expect("连库"));
        let started = Instant::now();
        let mut baseline = Vec::with_capacity(roots.len());
        for root in &roots {
            baseline.push(read_json(&single, *root).await);
        }
        let sequential = started.elapsed();
        let rows: usize = baseline.iter().map(|(rows, _)| rows).sum();

        // 原来的做法：同一条连接上并发。
        let started = Instant::now();
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let roots = Arc::new(roots);
        let mut tasks = Vec::new();
        for _ in 0..inflight {
            let (single, next, roots) = (single.clone(), next.clone(), roots.clone());
            tasks.push(tokio::spawn(async move {
                let mut out = Vec::new();
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(root) = roots.get(at) else { break };
                    out.push((at, read_json(&single, *root).await));
                }
                out
            }));
        }
        let mut shared_answers = vec![None; roots.len()];
        for task in tasks {
            for (at, answer) in task.await.unwrap() {
                shared_answers[at] = Some(answer);
            }
        }
        let shared = started.elapsed();

        // 读连接池并发。
        let pool = Pool::open(size).await.expect("开读连接");
        let started = Instant::now();
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..inflight {
            let (pool, next, roots) = (pool.clone(), next.clone(), roots.clone());
            tasks.push(tokio::spawn(async move {
                let mut out = Vec::new();
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(root) = roots.get(at) else { break };
                    let lease = pool.lease(LEASE_WAIT_LIMIT).await.expect("借连接");
                    out.push((at, read_json(lease.db(), *root).await));
                }
                out
            }));
        }
        let mut pooled_answers = vec![None; roots.len()];
        for task in tasks {
            for (at, answer) in task.await.unwrap() {
                pooled_answers[at] = Some(answer);
            }
        }
        let pooled = started.elapsed();

        let mut different = 0usize;
        for (at, expected) in baseline.iter().enumerate() {
            for (way, answers) in [("一条连接并发", &shared_answers), ("读连接池", &pooled_answers)] {
                let answer = answers[at].as_ref().expect("每个根都读过");
                if answer != expected {
                    different += 1;
                    eprintln!(
                        "{} 的记录与基准不同（{way}）：基准 {} 条，这次 {} 条",
                        roots[at], expected.0, answer.0
                    );
                }
            }
        }
        println!(
            "read-pool-probe roots={} rows={rows} inflight={inflight} pool={size} \
             sequential_ms={} shared_ms={} pooled_ms={} speedup_vs_shared={:.2} different={different}",
            roots.len(),
            sequential.as_millis(),
            shared.as_millis(),
            pooled.as_millis(),
            shared.as_secs_f64() / pooled.as_secs_f64().max(1e-9),
        );
        assert_eq!(pool.free.lock().unwrap().len(), size, "连接全数归还");
        assert_eq!(different, 0, "读连接池读回的记录必须与一条连接依次读的逐字相同");

        // 再走一遍进程级入口——`/model/records` 的 handler 调的就是这两个函数：池子由它自己按配置建，
        // 读走的是读连接（不是共享连接；测试进程里共享连接根本没连，退回去就会报错），内容仍与基准逐字相同。
        const DURABLE_PAGE: usize = 5000; // handler 给单根请求的 limit 上限
        let wanted = configured_connections();
        for (at, root) in roots.iter().enumerate().take(8) {
            let (records, report) = model_records_of_root(*root).await.expect("进程级入口（整根）");
            assert_eq!(report.via, ReadVia::Pool(wanted), "整根这一路要走读连接: {report:?}");
            assert_eq!(serde_json::to_string(&records).unwrap(), baseline[at].1, "{root}");

            let (window, report) = model_records_window(*root, 0, DURABLE_PAGE)
                .await
                .expect("进程级入口（按页）");
            assert_eq!(report.via, ReadVia::Pool(wanted), "按页这一路要走读连接: {report:?}");
            if baseline[at].0 <= DURABLE_PAGE {
                assert!(!window.more, "{root}: 一页装得下的根不该还有下一页");
                assert_eq!(serde_json::to_string(&window.rows).unwrap(), baseline[at].1, "{root}");
            } else {
                assert_eq!(window.rows.len(), DURABLE_PAGE, "{root}");
                assert!(window.more, "{root}");
            }
        }
        println!(
            "read-pool-probe global-entry roots={} via=pool-{wanted} same=true",
            roots.len().min(8)
        );
    }
}
