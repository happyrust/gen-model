//! 全局几何并发闸（specs/023，ADR-041 第 3 条）。
//!
//! 生成路径此前的并行形态是「根内并行、根间串行」，且每处 fan-out 的宽度都是
//! 写死的（`manifold_bool` 固定切 16 块、`mesh_generate`/`pdms_inst`/`gen_model`
//! 同类）：这层代码写的时候假设自己独占进程，彼此不知道对方存在。两层各限各的
//! 会得到一个没人算得清的乘积（8 个根 × 16 路 = 128 个 task 同抢 CPU 且全砸同
//! 一个 kv-mem 实例），调参时无法归因。
//!
//! 本模块是**唯一**的额度权威：根级与根内 fan-out 共用同一份额度，它同时就是
//! 对 fork 服务器和 kv-mem 的限流阀门。额度也是唯一的性能旋钮兼回滚开关——
//! 配置 `geometry_workers = 1` 即退化为串行执行（specs/023 第 2 条的回滚路径）。
//!
//! # 用法纪律（防死锁，NON-NEGOTIABLE）
//!
//! 许可只准罩住**同步 CPU 段**：拿着许可期间不得 await 任何同样要过闸的子任务。
//! 编排层（`gen_model` 的分类 stage 任务这种「spawn 子任务再 join」的壳）一律不
//! 拿许可——否则额度 = 1 时外层攥着唯一一张许可等内层，内层永远等不到，直接死锁。
//! 落到写法上：
//!
//! - CPU 段用 [`run_gated_cpu`]，只收同步闭包，闭包里写不出 `.await`，上面那条
//!   死锁写法在类型层就不成立（ADR-052 第 1 条）；
//! - fan-out 宽度用 [`resident_workers`]（`min(额度, 件数)` 个常驻 worker 从共享
//!   队列动态领取），不是静态均分，也不是「每件一个 task 挤信号量」；
//! - 编排层继续用 `spawn_with_staged_io`，不碰闸;
//! - 顺序循环里的分批（SQL 写回攒批这种不并发的 chunk）只用 [`GeometryGate::chunk_size`]
//!   派生批宽，**不拿许可**；
//! - [`spawn_gated_leaf`] 已退役，只剩 specs/033 T011～T018 待迁的七处调用点。
//!
//! # 计量（specs/033 T002）
//!
//! 闸自己记四个量：在飞、在等、累计等待时长、累计**持有**时长。前三个原来就有，
//! 持有时长是新的——没有它就算不出利用率，也就无从判断「额度开到 8 到底兑现了几路」。
//!
//! 持有时长在**读的那一刻**结算，而不是只在归还时结算：一个 BRAN 布尔能攥着许可
//! 跑几分钟，若只在 `Drop` 里累加，快照落在这种长段中间时正被占着的那几张许可读数
//! 是 0，利用率被系统性低估——T004 的基线采集与 T013 的 0.7 验收线恰好都要在 CPU
//! 密集段内取样，读小了会把达标的配置误判成不达标。账本因此记两笔：已归还许可的
//! 持有之和，与在飞许可各自的取得时刻之和；读数时用「在飞张数 × 当下 − 取得时刻
//! 之和」把在飞那截补出来。
//!
//! 读数有一条现在必须说清的边界：**这一版的 `active` 是混读的**。走
//! [`run_gated_cpu`] 的那部分许可只罩同步 CPU 段，持有时长就是 CPU 忙的时长；
//! 走退役的 [`spawn_gated_leaf`] 的那七处仍罩整个 `Future`（数据库查询、跨 `.await`
//! 的锁、同步文件写都在里面），持有时长只读作「许可被占住多久」。两者要到
//! specs/033 T011～T018 把叶子迁完之后才全都重合；在那之前采到的利用率不得整体
//! 当成 CPU 利用率写进 A/B 结论。
//!
//! # 第二个域：`/model/records` 冷读的读连接（2026-11-11）
//!
//! `src/fast_model/` 里的并发宽度只在本模块定。除了几何额度，这里还定一个：冷读另开几条只读连接
//! （[`configured_read_connections`]，环境变量 [`READ_CONNECTIONS_ENV`]），以及按连接数发放的借出额度
//! （[`ReadLeaseGate`]）。`model_read_pool` 只管连接本身——建、借、还、坏了收掉——宽度与额度都从这里拿，
//! 它自己不建信号量、不读那个环境变量（[`tests::no_hardcoded_fanout_width_survives_in_fast_model`] 与
//! [`tests::the_width_domains_in_this_module_are_named`] 钉住）。
//!
//! 它与几何额度是**两个域**，不共用一份，原因有三：
//!
//! - 罩的东西相反。几何许可只准罩同步 CPU 段，拿着它不许 `.await`；一份读连接额度罩的恰恰是「一句冷读从
//!   发出到收完」这段等库的时间。把冷读塞进几何额度，就是 ADR-052 背景里那个容量倒置：许可被等库的任务占满，
//!   CPU 空着。
//! - 量的东西不同。几何额度是本进程同时用几个核；读连接数是库那一头同时算几句冷读（SurrealDB 把同一条连接上的
//!   请求放在一个任务里轮着跑，一条连接只用得上一个核）。
//! - 两者不相乘。ADR-041 第 3 条要消灭的是「外层 N 路 × 内层 M 路」那种没人算得清的乘积；这两个域谁也不在
//!   另一个里面——借着读连接的读不派过闸的子任务，CPU 段里不读库——对库的压力是相加：几何那一侧的读写仍走
//!   共享连接，冷读最多再添「读连接数」句。
//!
//! 再添第三个域之前先想清楚它为什么不能归进这两个；添了要在上面那条测试里记名。

use serde::Serialize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 几何并发闸：额度即同时执行的叶子任务数上限。
pub struct GeometryGate {
    quota: usize,
    semaphore: Arc<Semaphore>,
    metrics: Arc<GateMetrics>,
}

/// 闸的计量。挂在闸实例上而不是模块级 static：闸本身是进程单例，而单测里的临时闸
/// 各记各的，读数不会被同一个测试二进制里并行跑的其他用例污染。
#[derive(Debug)]
struct GateMetrics {
    /// 观测起点 = 闸建立的时刻。利用率的分母来自它，因此必须与计数同生共死。
    epoch: Instant,
    waiting: AtomicUsize,
    wait_micros: AtomicU64,
    hold: Mutex<HoldLedger>,
}

/// 「持有」的账本。在飞张数与在飞许可的取得时刻是**一对**耦合读数：补在飞那截要拿
/// 张数乘当下、再减去取得时刻之和，两个数各自用原子量读会撕（读到张数已加一而时刻
/// 和还没加，就补出一整段凭空的持有）。所以放进同一把 std `Mutex`：取还许可的频次
/// 就是几何叶子任务的频次（一件几毫秒到几分钟），这把锁不可能成为热点，锁内只有几
/// 条整数运算、绝不跨 `.await`。
#[derive(Debug, Default)]
struct HoldLedger {
    /// 当下攥着许可的任务数。
    active: usize,
    /// 已归还的那些许可，持有时长之和。
    settled_micros: u64,
    /// 在飞许可各自的取得时刻（相对 `epoch` 的微秒偏移）之和。
    in_flight_offset_sum: u64,
}

impl HoldLedger {
    /// 截至 `now_micros`（同为相对 `epoch` 的偏移）的累计持有时长，含在飞未结算部分。
    ///
    /// 调用方必须在**同一次持锁内**取 `now_micros`：取得时刻是先落账本、后被读到的，
    /// 锁内取时刻才能保证每张在飞许可的取得时刻都早于它，`in_flight` 那一项不会算成
    /// 负数被 saturating 抹平成 0（那正是本函数要修的低估形态）。
    fn held_micros_at(&self, now_micros: u64) -> u64 {
        let in_flight = (self.active as u64)
            .saturating_mul(now_micros)
            .saturating_sub(self.in_flight_offset_sum);
        self.settled_micros.saturating_add(in_flight)
    }
}

impl GateMetrics {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            waiting: AtomicUsize::new(0),
            wait_micros: AtomicU64::new(0),
            hold: Mutex::new(HoldLedger::default()),
        }
    }

    /// 账本锁。中毒（持锁线程 panic）时读回内层数据接着记账：读数不准是小事，让几何
    /// 管线因为一个计数器 panic 才是大事。
    fn ledger(&self) -> MutexGuard<'_, HoldLedger> {
        self.hold
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn saturating_micros(elapsed: Duration) -> u64 {
    elapsed.as_micros().min(u128::from(u64::MAX)) as u64
}

impl GeometryGate {
    pub(crate) fn with_quota(quota: usize) -> Self {
        let quota = quota.max(1);
        Self {
            quota,
            semaphore: Arc::new(Semaphore::new(quota)),
            metrics: Arc::new(GateMetrics::new()),
        }
    }

    /// 配置定死的额度（进程内不变）。
    pub fn quota(&self) -> usize {
        self.quota
    }

    /// 叶子 fan-out 的分块宽度：把 `len` 件工作均分成**不超过额度**份。
    ///
    /// 额度 = 1 时返回 `len`（单块 = 串行），对齐 specs/023 第 2 条的回滚语义；
    /// `len = 0` 时返回 1——`chunks(0)` 会 panic，空输入在调用点本来就直接返回。
    pub fn chunk_size(&self, len: usize) -> usize {
        len.div_ceil(self.quota).max(1)
    }

    /// 取一张许可（RAII，drop 即归还）。**模块私有**（ADR-052 第 2 条 /
    /// specs/033 T007）：调用方拿不到它，也就无法自行把许可的作用域扩大到
    /// CPU 段之外。对外只有 [`run_gated_cpu`] 一个口子。
    ///
    /// 等待时长在拿到许可的那一刻结算，持有时长从那一刻起算：排队的时间既不算
    /// 在飞，也不算持有——否则额度 1 的串行执行会算出超过 100% 的利用率。
    async fn acquire(&self) -> GeometryPermit {
        self.metrics.waiting.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("geometry gate semaphore is never closed");
        self.metrics.waiting.fetch_sub(1, Ordering::Relaxed);
        self.metrics
            .wait_micros
            .fetch_add(saturating_micros(started.elapsed()), Ordering::Relaxed);
        let acquired_offset = saturating_micros(self.metrics.epoch.elapsed());
        {
            let mut ledger = self.metrics.ledger();
            ledger.active += 1;
            ledger.in_flight_offset_sum =
                ledger.in_flight_offset_sum.saturating_add(acquired_offset);
        }
        GeometryPermit {
            _permit: permit,
            metrics: self.metrics.clone(),
            acquired_offset,
        }
    }

    /// 当下这一刻的读数。挂在实例上而不是只有全局自由函数版本，单测里的临时闸才读
    /// 得到自己的账（读全局闸会被同一个测试二进制里并行跑的其他用例污染）。
    pub(crate) fn snapshot(&self) -> GeometryConcurrencySnapshot {
        let ledger = self.metrics.ledger();
        let observed_micros = saturating_micros(self.metrics.epoch.elapsed());
        GeometryConcurrencySnapshot {
            quota: self.quota,
            active: ledger.active,
            waiting: self.metrics.waiting.load(Ordering::Relaxed),
            permit_wait_micros: self.metrics.wait_micros.load(Ordering::Relaxed),
            active_permit_micros: ledger.held_micros_at(observed_micros),
            observed_micros,
        }
    }
}

/// 闸的许可凭证；持有期间占一份额度。模块私有，见 [`GeometryGate::acquire`]。
struct GeometryPermit {
    _permit: OwnedSemaphorePermit,
    metrics: Arc<GateMetrics>,
    /// 取得时刻，相对闸的 `epoch`。存偏移而不是 `Instant`：结算与快照补在飞都要拿它
    /// 跟同一根时间轴上的「当下」做差，两处用同一个量纲才不会各算各的。
    acquired_offset: u64,
}

impl Drop for GeometryPermit {
    fn drop(&mut self) {
        let released_offset = saturating_micros(self.metrics.epoch.elapsed());
        let mut ledger = self.metrics.ledger();
        ledger.active = ledger.active.saturating_sub(1);
        ledger.in_flight_offset_sum = ledger
            .in_flight_offset_sum
            .saturating_sub(self.acquired_offset);
        ledger.settled_micros = ledger
            .settled_micros
            .saturating_add(released_offset.saturating_sub(self.acquired_offset));
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct GeometryConcurrencySnapshot {
    pub quota: usize,
    pub active: usize,
    pub waiting: usize,
    pub permit_wait_micros: u64,
    /// 累计许可持有时长。配合 `observed_micros` 求利用率，见
    /// [`GeometryConcurrencySnapshot::utilization_since`]。
    pub active_permit_micros: u64,
    /// 闸建立至今的墙钟。利用率的分母，单独暴露是为了让调用方能对两次快照做差，
    /// 算某一段（而不是进程一辈子）的利用率。
    pub observed_micros: u64,
}

impl GeometryConcurrencySnapshot {
    /// 本快照与 `before` 之间那一段的闸利用率：持有时长增量 ÷（额度 × 墙钟增量）。
    ///
    /// 只给区间值、不给「进程至今」的平均值，是因为后者会把等死信、等 AABB 收敛这类
    /// 大段空闲摊进分母——现场恰好就有这种形态（模型本体 258s，之后干等 22 分钟），
    /// 平均下来是个既真实又毫无意义的小数。墙钟增量为 0 时返回 `None`，不编一个 0。
    pub fn utilization_since(&self, before: &Self) -> Option<f64> {
        gate_utilization(
            self.active_permit_micros
                .saturating_sub(before.active_permit_micros),
            self.quota,
            self.observed_micros.saturating_sub(before.observed_micros),
        )
    }
}

/// 利用率本体。不夹到 1.0：大于 1 只可能是计量本身错了（等待被算进了持有），
/// 那种时候要的是看见它，不是把它藏成 100%。
fn gate_utilization(held_micros: u64, quota: usize, wall_micros: u64) -> Option<f64> {
    let capacity = (quota as u64).checked_mul(wall_micros)?;
    (capacity > 0).then(|| held_micros as f64 / capacity as f64)
}

pub fn snapshot() -> GeometryConcurrencySnapshot {
    geometry_gate().snapshot()
}

static GATE: OnceLock<GeometryGate> = OnceLock::new();

/// 把配置探测结果落成生效额度：未配置 → 物理核数（最小 1），配置非法 → Err。
///
/// 拆成纯函数是为了能在单测里各走一遍两种结局；默认取物理核数而不是逻辑核数，
/// 是 ADR-041 第 3 条的原文口径——超线程对这类三角化/布尔的浮点密集负载没有
/// 额外吞吐，按逻辑核数开闸只会加剧 kv-mem 争用。
fn resolve_geometry_quota(configured: Result<Option<usize>, String>) -> Result<usize, String> {
    Ok(configured?.unwrap_or_else(|| num_cpus::get_physical().max(1)))
}

/// 启动时校验并定死几何并发闸额度（specs/023 兼容性条款）。
///
/// `geometry_workers` 写了非法值必须**启动失败**而不是静默回退默认值——额度
/// 默认取物理核数，默认行为本身就随机器变，静默回退会让「我明明配了 1」的
/// 回滚操作无声失效。二进制路径（`run_cli`）与 Python 路径（`full_init`）都
/// 必须在任何模型/房间写入之前调用它。
///
/// 成功时同时把全局闸初始化成校验过的额度，保证后续 [`geometry_gate`] 拿到的
/// 就是这里裁决的值。
pub fn validate_geometry_concurrency_config() -> Result<usize, String> {
    let quota = resolve_geometry_quota(crate::options::configured_geometry_workers())?;
    let gate = GATE.get_or_init(|| GeometryGate::with_quota(quota));
    if gate.quota() != quota {
        // OnceLock 已被更早的取值占住且额度不同——只可能是有人在校验前就动了闸。
        return Err(format!(
            "几何并发闸已按额度 {} 初始化，与本次校验出的 {quota} 不符：\
             validate_geometry_concurrency_config 必须先于一切生成路径调用",
            gate.quota()
        ));
    }
    Ok(quota)
}

/// 全局几何并发闸。
///
/// 正常启动序里 [`validate_geometry_concurrency_config`] 已经把它定死；这里的
/// 惰性初始化只为不走启动序的路径（单测、探针 bin）兜底。兜底路径上配置非法
/// 直接 panic——这不是可继续的状态，静默回退默认值正是 specs/023 明令禁止的。
pub fn geometry_gate() -> &'static GeometryGate {
    GATE.get_or_init(|| {
        let quota = resolve_geometry_quota(crate::options::configured_geometry_workers())
            .unwrap_or_else(|error| panic!("几何并发闸配置非法：{error}"));
        GeometryGate::with_quota(quota)
    })
}

/// **已退役（ADR-052 第 1 条 / specs/033 T007），新代码一律用 [`run_gated_cpu`]。**
///
/// 它罩的是整个 `Future`：数据库查询、跨 `.await` 的锁、同步文件写都在一张许可
/// 之内，于是许可可以被「等库 / 等锁」的任务全部占住而 CPU 空着（ADR-052 背景
/// 第 1、2 条那个容量倒置），而且「攥着许可去 await 另一个过闸子任务」这条会
/// 死锁的写法在类型层挡不住。现存七处调用点保留一个发布周期（specs/033 T011～
/// T018 逐个迁移），由
/// [`tests::the_retired_future_wide_gate_gains_no_new_call_sites`] 钉住不再新增。
pub(crate) fn spawn_gated_leaf<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
        let _permit = geometry_gate().acquire().await;
        future.await
    })
}

/// 私有的几何 CPU 池：线程数 = 闸额度。
///
/// **不用全局 Rayon 池**（ADR-052 第 3 条）：那个池被 `room_model` / `db_model` 的
/// `par_iter` 共用，宽度不归闸管，接进来就是第二套并发口径——正是 ADR-041 第 3 条
/// 消灭掉的形态。
static CPU_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();

/// 几何 worker 的线程栈（GitHub #78）：e3d-model 求值、manifold 布尔与三角化全在这条线上，rayon 默认只给
/// 2MB（或 `RUST_MIN_STACK`）——仓内脚本都靠 `RUST_MIN_STACK=128MB` 兜底，发布包没有。与 tokio 运行时同取
/// 64MB：栈是保留地址空间、按页提交，多留不花实际内存。
pub(crate) const GEOMETRY_THREAD_STACK: usize = 64 * 1024 * 1024;

fn cpu_pool() -> &'static rayon::ThreadPool {
    CPU_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(geometry_gate().quota())
            .stack_size(GEOMETRY_THREAD_STACK)
            .thread_name(|index| format!("geometry-cpu-{index}"))
            .build()
            .expect("private geometry CPU pool")
    })
}

/// 几何 CPU 段的执行域（ADR-052 第 1、3 条 / specs/033 T006）。
///
/// 只收**同步闭包**。闭包里写不出 `.await`，于是「攥着许可去 await 另一个过闸
/// 子任务」这条真正会死锁的写法在类型层就不成立——那是 [`spawn_gated_leaf`] 罩
/// 整个 `Future` 时挡不住的一条。等库、等锁一律留在 async 侧，不占额度。
///
/// 计算离开 tokio worker，跑进上面那个私有池：三角化与布尔不再把 shape receiver、
/// SurrealDB 响应、watcher、`/health` 与 timer 挤掉调度。许可在派发**前**取得、
/// 在 CPU worker 线程上归还（`permit` 随闭包搬过去），所以它罩住的正好是这一段
/// CPU——池宽与额度相等，派发排队时间为零。
///
/// 闭包 panic 在这里就地接住转成错误：rayon 的默认处置是**整个进程 abort**，
/// 一根坏构件不该把服务打死。接住之后许可照常归还，闸不会被漏掉的一张堵死。
pub(crate) async fn run_gated_cpu<T, F>(job: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = geometry_gate().acquire().await;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    cpu_pool().spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("geometry CPU segment panicked")));
        // 许可跟着 CPU 段走：在 worker 线程上归还，不留在等结果的 async 外壳里。
        drop(permit);
        let _ = sender.send(outcome);
    });
    receiver
        .await
        .map_err(|_| anyhow::anyhow!("geometry CPU worker vanished"))?
}

/// 一次 fan-out 派多少个常驻 worker：`min(额度, 件数)`（ADR-052 第 4 条）。
///
/// 动态领取而不是静态均分——`chunk_size` 只保证块数不超过额度、不保证接近额度
/// （额度 16、17 件只切出 9 块），而几何负载天然长尾，静态分区一旦把几个重 BRAN
/// 分进同一块，其余 worker 早已空转。也**不得**改成「每件一个 task 再去挤信号量」。
///
/// 件数为 0 时返回 0：没有活就不派 worker，也就不占额度。
pub(crate) fn resident_workers(jobs: usize) -> usize {
    geometry_gate().quota().min(jobs)
}

// ---- 第二个域：`/model/records` 冷读的读连接（模块文档末节） ----

/// 冷读的读连接开几条。`0` = 不开（照旧只用共享连接）。
pub const READ_CONNECTIONS_ENV: &str = "AIOS_MODEL_READ_CONNECTIONS";

/// 缺省的读连接数。保守取 4：实测 4 条已经把 8 万件的 SITE 从 7.9 s 压到 2.4–2.8 s；再加到 6 条是 1.6–1.7 s，
/// 要那 0.8 s 就把环境变量设成 6。多开连接不抬库的内存（1 / 2 / 3 / 4 / 6 条量下来工作集都在 ±6 MB 内），
/// 抬的是库同一时刻用几个核。
pub const DEFAULT_READ_CONNECTIONS: usize = 4;

/// 读连接数的上限：认到这里为止，写得再大也按它开。
pub const MAX_READ_CONNECTIONS: usize = 16;

/// 读连接数的取值规则：没设、空白、认不出的值 → 缺省；`0` → 不开；超过上限按上限。
pub fn read_connections_from(raw: Option<&str>) -> usize {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        None => DEFAULT_READ_CONNECTIONS,
        Some(raw) => match raw.parse::<usize>() {
            Ok(wanted) => wanted.min(MAX_READ_CONNECTIONS),
            Err(_) => DEFAULT_READ_CONNECTIONS,
        },
    }
}

/// 本进程要开几条读连接。进程启动后读一次。
pub(crate) fn configured_read_connections() -> usize {
    static WANTED: OnceLock<usize> = OnceLock::new();
    *WANTED
        .get_or_init(|| read_connections_from(std::env::var(READ_CONNECTIONS_ENV).ok().as_deref()))
}

/// 读连接的借出额度：宽度 = 读连接数，一份额度对一条连接，一条连接同一时刻只借给一个读。
///
/// 与 [`GeometryGate`] 故意是两个类型（为什么是两个域见模块文档末节）：这份额度持有期间必然跨 `.await`
/// （等库），几何许可不许；它也没有 `chunk_size` / `resident_workers` 那一套——冷读不 fan-out，来一个读借一份。
pub(crate) struct ReadLeaseGate {
    semaphore: Arc<Semaphore>,
}

impl ReadLeaseGate {
    /// `width` 由调用方按**实际开出来**的连接数给：要开几条是 [`configured_read_connections`] 定的，开不满就整池
    /// 不要，所以两者相等。宽度 0 的闸一份也借不出。
    pub(crate) fn with_width(width: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(width)),
        }
    }

    /// 当下还借得出几份。
    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// 借一份（RAII，drop 即归还）；都借出去了就等，最多等 `limit`。`None` = 等到头了。
    pub(crate) async fn acquire(&self, limit: Duration) -> Option<ReadLeasePermit> {
        let permit = tokio::time::timeout(limit, self.semaphore.clone().acquire_owned())
            .await
            .ok()?
            .ok()?;
        Some(ReadLeasePermit { _permit: permit })
    }
}

/// 一份读连接额度；持有期间占一条读连接。
pub(crate) struct ReadLeasePermit {
    _permit: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 额度 = 1 时分块宽度 = 全长：单块单任务，就是改动前的串行。
    /// GitHub #78：几何 worker 跑 e3d-model 求值、布尔与三角化，栈不能是 rayon 默认的 2MB——发布包不设
    /// `RUST_MIN_STACK`。真建一次池，在 worker 线程上量一下可用栈。
    #[test]
    fn the_geometry_pool_runs_on_large_stacks() {
        assert!(GEOMETRY_THREAD_STACK >= 64 * 1024 * 1024);
        let source = include_str!("concurrency.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the tests")
            .0;
        let pool = source
            .split_once("fn cpu_pool()")
            .expect("cpu_pool")
            .1
            .split_once("\n}\n")
            .or_else(|| {
                source
                    .split_once("fn cpu_pool()")
                    .unwrap()
                    .1
                    .split_once("\n}\r\n")
            })
            .expect("cpu_pool end")
            .0;
        assert!(
            pool.contains(".stack_size(GEOMETRY_THREAD_STACK)"),
            "几何私有池要显式给栈大小: {pool}"
        );
        // 2MB 栈上放不下 8MB 的局部数组；64MB 的 worker 放得下。
        let probe = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .stack_size(GEOMETRY_THREAD_STACK)
            .build()
            .unwrap();
        let sum = probe.install(|| {
            let block = std::hint::black_box([1u8; 8 * 1024 * 1024]);
            block.iter().map(|&b| b as u64).sum::<u64>()
        });
        assert_eq!(sum, 8 * 1024 * 1024);
    }

    #[test]
    fn quota_one_yields_a_single_chunk() {
        let gate = GeometryGate::with_quota(1);
        assert_eq!(gate.chunk_size(0), 1, "chunks(0) 会 panic，空输入也要给 1");
        assert_eq!(gate.chunk_size(1), 1);
        assert_eq!(gate.chunk_size(1000), 1000);
    }

    /// 分块数不超过额度，且不会因为除法取整多出一块（旧写法 `len/16` 会切出
    /// 第 17 块尾巴，块数本身就不是宽度）。
    #[test]
    fn chunk_size_never_exceeds_the_quota_in_chunk_count() {
        for quota in [2_usize, 3, 8, 16] {
            let gate = GeometryGate::with_quota(quota);
            for len in [1_usize, 5, 16, 17, 100, 1001] {
                let size = gate.chunk_size(len);
                let chunk_count = len.div_ceil(size);
                assert!(
                    chunk_count <= quota,
                    "len={len} quota={quota} size={size} 切出了 {chunk_count} 块"
                );
            }
        }
    }

    /// 额度 = 0 是防御性下限：闸自己夹到 1，不许出现拿不到许可的死闸。
    #[test]
    fn a_zero_quota_is_clamped_to_serial_not_dead() {
        let gate = GeometryGate::with_quota(0);
        assert_eq!(gate.quota(), 1);
    }

    /// 未配置回落物理核数且至少为 1；配置读取错误原样上浮，不静默回退。
    #[test]
    fn resolve_defaults_to_physical_cores_and_propagates_probe_errors() {
        assert!(resolve_geometry_quota(Ok(None)).expect("默认值必须可用") >= 1);
        assert_eq!(resolve_geometry_quota(Ok(Some(3))), Ok(3));
        assert_eq!(
            resolve_geometry_quota(Err("geometry_workers 非法".into())),
            Err("geometry_workers 非法".into())
        );
    }

    /// T09（specs/023）：`src/fast_model/` 内不得再出现写死的并发宽度，新增
    /// fan-out 必须过闸。三条禁令，按目录扫描而不是逐文件 include_str，新文件
    /// 自动入网：
    ///
    /// 1. 本模块之外不得出现本地信号量（`Semaphore::new`）——那是第二套并发
    ///    口径，正是 ADR-041 第 3 条消灭的形态（cata_model 曾有一套 CPU*2 夹
    ///    4..16 的私有信号量）。
    /// 2. 本模块之外不得自行探测核数（`available_parallelism` / `num_cpus`）——
    ///    宽度决策只属于闸。
    /// 3. 含 fan-out（有 spawn 或 [`run_gated_cpu`] 调用）的文件里不得出现
    ///    `.len() / <字面量>` 的分块宽度与 `batch_chunks_cnt = <字面量>`：分块宽度
    ///    一律 `geometry_gate().chunk_size()`，worker 数一律 [`resident_workers`]。
    ///    `indices.len() / <字面量>` 单独放行——那是「每个三角形三个索引」的网格
    ///    算术，不是 fan-out 宽度。此前它靠「那些文件不 spawn」侥幸躲开，现在
    ///    生成服务自己也过闸了，例外必须写明。
    #[test]
    fn no_hardcoded_fanout_width_survives_in_fast_model() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fast_model");
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&dir).expect("read src/fast_model") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("utf8 file name")
                .to_string();
            if name == "concurrency.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            scanned += 1;

            assert!(
                !source.contains("Semaphore::new("),
                "{name}: 本地信号量是第二套并发口径，额度只能来自几何并发闸"
            );
            assert!(
                !source.contains("available_parallelism") && !source.contains("num_cpus::"),
                "{name}: 并发宽度不得自行探测核数，额度只能来自几何并发闸"
            );

            let spawns = source.contains("spawn_gated_leaf(")
                || source.contains("run_gated_cpu(")
                || source.contains("spawn_with_staged_io(")
                || source.contains("tokio::spawn(");
            if !spawns {
                continue;
            }
            for (idx, line) in source.lines().enumerate() {
                if let Some(rest) = line.split(".len() / ").nth(1) {
                    let literal: String = rest.chars().take_while(char::is_ascii_digit).collect();
                    assert!(
                        literal.is_empty() || line.contains("indices.len() / "),
                        "{name}:{}: 写死的分块除数：`{}`。宽度用 geometry_gate().chunk_size()",
                        idx + 1,
                        line.trim()
                    );
                }
                if let Some(rest) = line.split("batch_chunks_cnt = ").nth(1) {
                    assert!(
                        !rest.starts_with(|c: char| c.is_ascii_digit()),
                        "{name}:{}: 写死的分块数：`{}`。宽度用 geometry_gate().chunk_size()",
                        idx + 1,
                        line.trim()
                    );
                }
                assert!(
                    !line.trim_start().starts_with("let batch_size = if "),
                    "{name}:{}: 按输入规模写死的 fan-out 分块：`{}`。块宽用 geometry_gate().chunk_size()",
                    idx + 1,
                    line.trim()
                );
            }
        }
        assert!(scanned >= 20, "扫描面塌了：只看到 {scanned} 个文件");
    }

    /// 上面那条护栏只认得出「别的文件里有信号量 / 探测核数」。这一条补另一半：本模块里的并发域是**点名的两个**
    /// ——几何额度（[`GeometryGate`]）与读连接的借出额度（[`ReadLeaseGate`]）——再添一个要在这里记名、在模块文档里
    /// 说清它为什么不能归进已有的域，不然「宽度只在本模块定」就成了「信号量都堆到本模块来」。
    /// 读连接池那一侧：额度与宽度都从本模块拿，它自己不读环境变量定宽度。
    #[test]
    fn the_width_domains_in_this_module_are_named() {
        let production = |source: &'static str| {
            source
                .split_once("\n#[cfg(test)]")
                .expect("production code precedes the tests")
                .0
        };
        let source = production(include_str!("concurrency.rs"));
        assert_eq!(
            source.matches("Semaphore::new(").count(),
            2,
            "本模块的并发域只有点名的两个"
        );
        assert!(source.contains("Semaphore::new(quota)"), "几何额度");
        assert!(source.contains("Semaphore::new(width)"), "读连接的借出额度");
        assert_eq!(
            source.matches("std::env::var(").count(),
            1,
            "本模块只直接读一个环境变量：读连接数（几何额度走 options）"
        );
        assert!(source.contains("std::env::var(READ_CONNECTIONS_ENV)"));

        let pool = production(include_str!("model_read_pool.rs"));
        assert!(
            pool.contains("ReadLeaseGate::with_width(")
                && pool.contains("configured_read_connections"),
            "读连接池的额度与宽度都从本模块拿"
        );
        assert!(
            !pool.contains("std::env::var("),
            "读连接池不自己读环境变量定宽度"
        );
    }

    /// 读连接数的取值规则（第二个域的宽度）。
    #[test]
    fn the_read_connection_count_defaults_conservatively_and_zero_switches_the_pool_off() {
        assert_eq!(read_connections_from(None), DEFAULT_READ_CONNECTIONS);
        assert_eq!(read_connections_from(Some("")), DEFAULT_READ_CONNECTIONS);
        assert_eq!(read_connections_from(Some("  ")), DEFAULT_READ_CONNECTIONS);
        assert_eq!(
            read_connections_from(Some("0")),
            0,
            "0 = 不开，照旧只用共享连接"
        );
        assert_eq!(read_connections_from(Some(" 6 ")), 6);
        assert_eq!(read_connections_from(Some("1")), 1);
        assert_eq!(
            read_connections_from(Some("999")),
            MAX_READ_CONNECTIONS,
            "写得再大也只开到上限"
        );
        // 认不出的值不当成 0：写错了就悄悄关掉提速，比按缺省开更难发现。
        assert_eq!(read_connections_from(Some("six")), DEFAULT_READ_CONNECTIONS);
        assert_eq!(read_connections_from(Some("-1")), DEFAULT_READ_CONNECTIONS);
        assert_eq!(DEFAULT_READ_CONNECTIONS, 4);
    }

    /// 读连接额度：宽度几份就借得出几份；借光了后来的等，等到头回 `None`；一还就借到；宽度 0 一份也借不出。
    #[tokio::test]
    async fn the_read_lease_gate_lends_exactly_its_width_and_makes_the_rest_wait() {
        let gate = Arc::new(ReadLeaseGate::with_width(2));
        assert_eq!(gate.available(), 2);
        let first = gate
            .acquire(Duration::from_millis(50))
            .await
            .expect("第一份");
        let second = gate
            .acquire(Duration::from_millis(50))
            .await
            .expect("第二份");
        assert_eq!(gate.available(), 0);
        assert!(
            gate.acquire(Duration::from_millis(30)).await.is_none(),
            "借光了：等到头也借不到第三份"
        );

        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.acquire(Duration::from_secs(5)).await.is_some() })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "额度还没还，后来的读在等");
        drop(first);
        assert!(waiter.await.unwrap(), "一还就借到");
        drop(second);
        assert_eq!(gate.available(), 2, "用完全数归还");

        assert!(
            ReadLeaseGate::with_width(0)
                .acquire(Duration::from_millis(10))
                .await
                .is_none(),
            "宽度 0 的闸一份也借不出"
        );
    }

    /// specs/033 T007：`spawn_gated_leaf` 已退役，剩下的调用点是一份点名清单
    /// （T011～T018 逐个迁走）。**新代码用 [`run_gated_cpu`]**：新增一处调用即红。
    ///
    /// 用清单而不是 `#[deprecated]`，是因为后者会在这七处各喷一条警告，逼着六个
    /// 不属于本轮的文件各挂一个 `#[allow(deprecated)]`——那既弄脏了别人的文件，
    /// 也把「无新警告」这条纪律作废掉。清单挡的是同一件事，而且点得出名字。
    #[test]
    fn the_retired_future_wide_gate_gains_no_new_call_sites() {
        // 计数含各文件自己那条源码形状断言里的字面量，删调用点时两处一起改。
        let expected = [
            ("cata_model.rs", 1_usize),
            ("gen_model.rs", 2),
            ("loop_model.rs", 1),
            ("manifold_bool.rs", 2),
            ("mesh_generate.rs", 2),
            ("prim_model.rs", 1),
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fast_model");
        for entry in std::fs::read_dir(&dir).expect("read src/fast_model") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("utf8 file name")
                .to_string();
            if name == "concurrency.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            let found = source.matches("concurrency::spawn_gated_leaf(").count();
            let allowed = expected
                .iter()
                .find_map(|(file, count)| (*file == name).then_some(*count))
                .unwrap_or(0);
            assert_eq!(
                found, allowed,
                "{name}: spawn_gated_leaf 是退役接口，调用点只出不进；新 fan-out 用 run_gated_cpu"
            );
        }
    }

    /// specs/033 T006：额度就是同时在算的 CPU 段数。派 4 倍额度的活下去，观察到的
    /// 并发峰值不得超过额度——闸真的在限流，而不是只发凭证。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_cpu_domain_never_runs_more_segments_than_the_quota() {
        let quota = geometry_gate().quota();
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs = (0..quota * 4).map(|_| {
            let (in_flight, peak) = (in_flight.clone(), peak.clone());
            run_gated_cpu(move || {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        });
        futures::future::try_join_all(jobs).await.expect("cpu jobs");
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= quota, "并发峰值 {peak} 超过额度 {quota}");
    }

    /// ADR-052 第 3 条：CPU 段离开 tokio worker，跑在私有池里。跑在 tokio worker 上
    /// 就会把 shape receiver / SurrealDB 响应 / watcher / timer 挤掉调度。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cpu_segment_leaves_the_tokio_worker_for_the_private_pool() {
        let thread = run_gated_cpu(|| {
            Ok(std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string())
        })
        .await
        .expect("cpu job");
        assert!(
            thread.starts_with("geometry-cpu-"),
            "CPU 段跑在了 {thread:?} 上，不是私有几何池"
        );
    }

    /// 闭包 panic 必须**就地接住**：rayon 的默认处置是整个进程 abort，一根坏构件
    /// 不该把服务打死（去掉 `catch_unwind` 这条会直接崩测试进程，不是转红）。接住
    /// 之后许可要归还，否则额度被永久吃掉一张，额度 1 时下一段直接挂死。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_cpu_segment_is_caught_and_returns_its_permit() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let error = run_gated_cpu(|| -> anyhow::Result<()> { panic!("geometry blew up") }).await;
        std::panic::set_hook(previous);

        let error = error.expect_err("panic 必须变成错误");
        assert!(error.to_string().contains("panicked"), "{error}");
        for _ in 0..geometry_gate().quota() + 1 {
            run_gated_cpu(|| Ok(()))
                .await
                .expect("闸没有被漏掉的许可堵住");
        }
    }

    /// ADR-052 第 4 条：fan-out 宽度是 `min(额度, 件数)`；没有活就不派 worker。
    #[test]
    fn resident_worker_count_is_the_quota_capped_by_the_work() {
        let quota = geometry_gate().quota();
        assert_eq!(resident_workers(0), 0, "没有活就不占额度");
        assert_eq!(resident_workers(1), 1);
        assert_eq!(resident_workers(quota), quota);
        assert_eq!(
            resident_workers(quota * 10),
            quota,
            "件数再多也不许超过额度——那正是「每件一个 task 挤信号量」的形态"
        );
    }

    /// specs/023 第 2 条：额度 = 1 时任意多个叶子任务实际串行执行——在飞数的
    /// 峰值恰为 1。额度 = 2 时峰值不超过 2（闸真的在限流，而不是只发凭证）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_gate_caps_leaf_tasks_at_its_quota() {
        for (quota, expected_peak) in [(1_usize, 1_usize), (2, 2)] {
            let gate = Arc::new(GeometryGate::with_quota(quota));
            let in_flight = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));

            let mut handles = Vec::new();
            for _ in 0..8 {
                let gate = gate.clone();
                let in_flight = in_flight.clone();
                let peak = peak.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = gate.acquire().await;
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                }));
            }
            for handle in handles {
                handle.await.expect("leaf task joins");
            }

            let peak = peak.load(Ordering::SeqCst);
            assert!(
                peak <= expected_peak,
                "quota={quota} 时在飞峰值 {peak} 超过额度"
            );
            if quota == 1 {
                assert_eq!(peak, 1, "额度 1 必须真的串行");
            }
        }
    }

    /// specs/033 T002：利用率是「持有 ÷（额度 × 墙钟）」，且分母为 0 时不编数。
    /// 超过 1 不夹回去——那是计量出错的信号，藏起来等于把 bug 变成 100% 健康。
    #[test]
    fn gate_utilization_is_a_ratio_and_refuses_to_invent_one() {
        assert_eq!(gate_utilization(800, 8, 1_000), Some(0.1));
        assert_eq!(gate_utilization(8_000, 8, 1_000), Some(1.0));
        assert_eq!(gate_utilization(0, 8, 1_000), Some(0.0));
        assert_eq!(
            gate_utilization(500, 8, 0),
            None,
            "墙钟为 0 时没有利用率可言"
        );
        assert_eq!(
            gate_utilization(2_000, 1, 1_000),
            Some(2.0),
            "大于 1 必须原样露出来"
        );
    }

    /// specs/033 T002：区间利用率对两次快照做差，不受进程早先那些空闲段影响；
    /// 计数器反向（只可能是快照传反了）按 0 处理，不产生负利用率。
    #[test]
    fn utilization_is_taken_between_two_snapshots_not_over_the_whole_process() {
        let before = GeometryConcurrencySnapshot {
            quota: 4,
            active: 0,
            waiting: 0,
            permit_wait_micros: 0,
            active_permit_micros: 1_000_000,
            observed_micros: 9_000_000,
        };
        let after = GeometryConcurrencySnapshot {
            active_permit_micros: 1_000_000 + 3_200_000,
            observed_micros: 9_000_000 + 1_000_000,
            ..before
        };
        assert_eq!(after.utilization_since(&before), Some(0.8));
        assert_eq!(
            before.utilization_since(&after),
            None,
            "墙钟增量被 saturating 夹成 0，不许倒着算出个数来"
        );
    }

    /// specs/033 T002：排队的时间既不算在飞也不算持有。额度 1 时两个任务串行各持
    /// 30ms，第二个要等第一个——若等待被计进持有，持有时长就会超过闸自己的墙钟。
    /// 这条上界是结构性的，不依赖机器快慢。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn held_time_counts_the_permit_not_the_queue() {
        let gate = Arc::new(GeometryGate::with_quota(1));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let gate = gate.clone();
            handles.push(tokio::spawn(async move {
                let _permit = gate.acquire().await;
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }));
        }
        for handle in handles {
            handle.await.expect("leaf task joins");
        }

        let snapshot = gate.snapshot();
        assert!(
            snapshot.active_permit_micros >= 55_000,
            "两段真持有没记全：{}us",
            snapshot.active_permit_micros
        );
        assert!(
            snapshot.active_permit_micros <= snapshot.observed_micros,
            "额度 1 时持有 {}us 不可能超过墙钟 {}us",
            snapshot.active_permit_micros,
            snapshot.observed_micros
        );
        assert!(
            snapshot.permit_wait_micros >= 20_000,
            "第二个任务确实排过队：{}us",
            snapshot.permit_wait_micros
        );
        assert_eq!(snapshot.active, 0, "许可全部归还后在飞必须归零");
        assert_eq!(snapshot.waiting, 0);
    }

    /// specs/033 T002：快照落在长持有段**中间**时，正被攥着的许可也要算数。只在
    /// `Drop` 里结算的旧写法在这里读回 0——单个 BRAN 布尔能占住许可跑几分钟，T004
    /// 的基线快照大概率就落在这种段里，读 0 就是把利用率系统性压低。
    ///
    /// 顺带钉住归还侧的对账：闸空着时累计持有不许再涨（在飞张数没减），下一张许可
    /// 的在飞增量仍要准（上一张的取得时刻没从和里减掉，就会一直吃掉后来者的读数）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn held_time_includes_the_permit_that_is_still_in_flight() {
        let gate = Arc::new(GeometryGate::with_quota(2));

        let permit = gate.acquire().await;
        let before = gate.snapshot();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let during = gate.snapshot();
        assert_eq!(during.active, 1, "许可还攥在手里");
        let in_flight = during
            .active_permit_micros
            .saturating_sub(before.active_permit_micros);
        assert!(
            in_flight >= 35_000,
            "在飞的这 40ms 没被算进持有：{in_flight}us"
        );

        drop(permit);
        let settled = gate.snapshot();
        assert_eq!(settled.active, 0);
        assert!(
            settled.active_permit_micros >= during.active_permit_micros,
            "归还只是把在飞那截转成已结算，累计不许倒退：{} < {}",
            settled.active_permit_micros,
            during.active_permit_micros
        );

        tokio::time::sleep(Duration::from_millis(30)).await;
        let idle = gate.snapshot();
        assert_eq!(
            idle.active_permit_micros, settled.active_permit_micros,
            "闸空着的 30ms 里没人持有，累计持有不许涨"
        );

        let _second = gate.acquire().await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        let again = gate.snapshot();
        let second_in_flight = again
            .active_permit_micros
            .saturating_sub(idle.active_permit_micros);
        assert!(
            second_in_flight >= 25_000,
            "第二张许可的在飞时长被上一张的残留取得时刻吃掉了：{second_in_flight}us"
        );
    }

    /// specs/033 T004/T013 的采样形态：额度 1、一段 CPU 密集布尔从窗口头占到窗口尾，
    /// 区间利用率必须读作满载。旧写法在这一段读 0.0，0.7 的验收线会把达标的配置判成
    /// 不达标。这里的 1.0 是恒等式不是掐表：两次快照的持有增量与墙钟增量由同一个
    /// 「当下」算出，同一张许可全程在飞，两者逐微秒相等。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn utilization_reads_full_when_one_hold_spans_the_whole_window() {
        let gate = Arc::new(GeometryGate::with_quota(1));
        let _permit = gate.acquire().await;

        let before = gate.snapshot();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let after = gate.snapshot();

        assert_eq!(
            after.utilization_since(&before),
            Some(1.0),
            "整段被同一张许可占满，利用率就该是 1.0：持有 {}us 墙钟 {}us",
            after
                .active_permit_micros
                .saturating_sub(before.active_permit_micros),
            after.observed_micros.saturating_sub(before.observed_micros)
        );
    }
}
