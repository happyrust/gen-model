//! 进程内模型根并发控制。数据库队列仍由 ADR-011 的单协调器消费；这里仅决定一个
//! execution group 的后半程允许同时推进多少根，不创建新的线程池、Rayon 池或信号量。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::options::ModelConcurrencyMode;

const EVALUATION_WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct Window {
    started_at: Instant,
    completed: u64,
    failed: u64,
    pressured: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            completed: 0,
            failed: 0,
            pressured: false,
        }
    }
}

static EFFECTIVE: AtomicUsize = AtomicUsize::new(1);
static COMPLETED_TOTAL: AtomicU64 = AtomicU64::new(0);
static FAILED_TOTAL: AtomicU64 = AtomicU64::new(0);
static SHAPE_PRODUCER_BLOCKED_MICROS: AtomicU64 = AtomicU64::new(0);
static SHAPE_SQL_BYTES: AtomicU64 = AtomicU64::new(0);
static SHAPE_INSTANCES: AtomicU64 = AtomicU64::new(0);
static BASELINE_WRITE_P95_MICROS: AtomicU64 = AtomicU64::new(0);
static BASELINE_RSS_BYTES: AtomicU64 = AtomicU64::new(0);
static WINDOW: OnceLock<Mutex<Window>> = OnceLock::new();
static SURREAL_LATENCY: OnceLock<Mutex<SurrealLatencyWindow>> = OnceLock::new();

#[derive(Debug, Default)]
struct SurrealLatencyWindow {
    reads: VecDeque<u64>,
    writes: VecDeque<u64>,
    retries: u64,
}

fn surreal_latency() -> &'static Mutex<SurrealLatencyWindow> {
    SURREAL_LATENCY.get_or_init(|| Mutex::new(SurrealLatencyWindow::default()))
}

fn push_latency(samples: &mut VecDeque<u64>, micros: u64) {
    const MAX_SAMPLES: usize = 512;
    if samples.len() == MAX_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(micros);
}

pub(crate) fn record_surreal_read(elapsed: Duration) {
    let mut latency = surreal_latency()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    push_latency(
        &mut latency.reads,
        elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
    );
}

pub(crate) fn record_surreal_write(elapsed: Duration, retried: bool) {
    let mut latency = surreal_latency()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    push_latency(
        &mut latency.writes,
        elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
    );
    latency.retries += u64::from(retried);
}

fn percentile(samples: &VecDeque<u64>, percentile: usize) -> Option<u64> {
    let mut values = samples.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    (!values.is_empty()).then(|| {
        let index = (values.len() - 1).saturating_mul(percentile) / 100;
        values[index]
    })
}

fn window() -> &'static Mutex<Window> {
    WINDOW.get_or_init(|| Mutex::new(Window::default()))
}

fn exceeds_k1_baseline(
    current: usize,
    sample: u64,
    baseline: u64,
    numerator: u64,
    denominator: u64,
) -> bool {
    current > 1 && baseline > 0 && sample > baseline.saturating_mul(numerator) / denominator.max(1)
}

pub(crate) fn effective_root_inflight() -> usize {
    match crate::options::model_concurrency_mode() {
        ModelConcurrencyMode::Legacy => 1,
        ModelConcurrencyMode::Bounded => crate::options::model_root_inflight_max(),
        ModelConcurrencyMode::Adaptive => EFFECTIVE
            .load(Ordering::Relaxed)
            .clamp(1, crate::options::model_root_inflight_max()),
    }
}

/// 每个 execution group 结束时结算一次。具体资源探针可把 `pressured` 置真；错误本身
/// 也视作压力，避免失败风暴中继续放大并发。30秒内不抖动额度。
pub(crate) fn record_window(completed: usize, failed: usize, pressured: bool) {
    COMPLETED_TOTAL.fetch_add(completed as u64, Ordering::Relaxed);
    FAILED_TOTAL.fetch_add(failed as u64, Ordering::Relaxed);
    if crate::options::model_concurrency_mode() != ModelConcurrencyMode::Adaptive {
        return;
    }
    let current_write_p95 = {
        let latency = surreal_latency()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        percentile(&latency.writes, 95).unwrap_or(0)
    };
    let current = effective_root_inflight();
    // K=1 的整个观测期都是基线采样，而不是把进程刚启动时的第一个低水位样本冻结
    // 成“峰值”。否则 RSS 从 27MiB 正常涨到模型稳态的 90MiB 就会被误判为压力，
    // adaptive 永远没有机会升到 K=2。
    if current == 1 && current_write_p95 > 0 {
        BASELINE_WRITE_P95_MICROS.fetch_max(current_write_p95, Ordering::Relaxed);
    }
    let write_baseline = BASELINE_WRITE_P95_MICROS.load(Ordering::Relaxed);
    let write_pressure = exceeds_k1_baseline(current, current_write_p95, write_baseline, 2, 1);
    let rss = process_metrics().1.unwrap_or(0);
    if current == 1 && rss > 0 {
        BASELINE_RSS_BYTES.fetch_max(rss, Ordering::Relaxed);
    }
    let rss_baseline = BASELINE_RSS_BYTES.load(Ordering::Relaxed);
    let rss_pressure = exceeds_k1_baseline(current, rss, rss_baseline, 5, 4);
    let mut state = window()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.completed += completed as u64;
    state.failed += failed as u64;
    state.pressured |= pressured || failed > 0 || write_pressure || rss_pressure;
    if state.started_at.elapsed() < EVALUATION_WINDOW {
        return;
    }
    let next = next_effective(
        current,
        state.pressured,
        state.completed,
        crate::options::model_root_inflight_max(),
    );
    EFFECTIVE.store(next, Ordering::Relaxed);
    *state = Window::default();
}

/// 评估窗结算的额度裁决：压力减半（下限 1）且优先于进展；有进展加一（封顶 `max`）；
/// 一个窗口既无压力也无完成则原地不动。抽成纯函数是为了让这三条各自有一条会红的
/// 测试——控制器直接影响现场吞吐，不能只靠现场发现回归。
fn next_effective(current: usize, pressured: bool, completed: u64, max: usize) -> usize {
    if pressured {
        (current / 2).max(1)
    } else if completed > 0 {
        (current + 1).min(max)
    } else {
        current
    }
}

pub(crate) fn record_shape_run(blocked_micros: u64, sql_bytes: usize, instances: usize) {
    SHAPE_PRODUCER_BLOCKED_MICROS.fetch_add(blocked_micros, Ordering::Relaxed);
    SHAPE_SQL_BYTES.fetch_add(sql_bytes as u64, Ordering::Relaxed);
    SHAPE_INSTANCES.fetch_add(instances as u64, Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelConcurrencySnapshot {
    pub mode: ModelConcurrencyMode,
    pub execution_group_size: usize,
    pub effective_root_inflight: usize,
    pub root_inflight_max: usize,
    pub completed_total: u64,
    pub failed_total: u64,
    pub shape_queue_depth: usize,
    pub shape_producer_blocked_micros: u64,
    pub shape_sql_bytes: u64,
    pub shape_instances_written: u64,
    pub surreal_read_p50_micros: Option<u64>,
    pub surreal_read_p95_micros: Option<u64>,
    pub surreal_read_p99_micros: Option<u64>,
    pub surreal_write_p50_micros: Option<u64>,
    pub surreal_write_p95_micros: Option<u64>,
    pub surreal_write_p99_micros: Option<u64>,
    pub surreal_retries: u64,
    pub process_cpu_percent: Option<f32>,
    pub process_rss_bytes: Option<u64>,
}

pub fn snapshot() -> ModelConcurrencySnapshot {
    let latency = surreal_latency()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (process_cpu_percent, process_rss_bytes) = process_metrics();
    ModelConcurrencySnapshot {
        mode: crate::options::model_concurrency_mode(),
        execution_group_size: crate::options::model_regen_execution_group(),
        effective_root_inflight: effective_root_inflight(),
        root_inflight_max: crate::options::model_root_inflight_max(),
        completed_total: COMPLETED_TOTAL.load(Ordering::Relaxed),
        failed_total: FAILED_TOTAL.load(Ordering::Relaxed),
        // 单 writer 在 execution group 边界已排空；运行中精确深度由 flume 通道自身
        // 控制，健康快照这里报告最后已确认的边界值。
        shape_queue_depth: 0,
        shape_producer_blocked_micros: SHAPE_PRODUCER_BLOCKED_MICROS.load(Ordering::Relaxed),
        shape_sql_bytes: SHAPE_SQL_BYTES.load(Ordering::Relaxed),
        shape_instances_written: SHAPE_INSTANCES.load(Ordering::Relaxed),
        surreal_read_p50_micros: percentile(&latency.reads, 50),
        surreal_read_p95_micros: percentile(&latency.reads, 95),
        surreal_read_p99_micros: percentile(&latency.reads, 99),
        surreal_write_p50_micros: percentile(&latency.writes, 50),
        surreal_write_p95_micros: percentile(&latency.writes, 95),
        surreal_write_p99_micros: percentile(&latency.writes, 99),
        surreal_retries: latency.retries,
        process_cpu_percent,
        process_rss_bytes,
    }
}

fn process_metrics() -> (Option<f32>, Option<u64>) {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();
    let system = SYSTEM.get_or_init(|| Mutex::new(System::new()));
    let mut system = system
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pid = Pid::from_u32(std::process::id());
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or((None, None), |process| {
        (Some(process.cpu_usage()), Some(process.memory()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_halves_and_progress_adds_one() {
        assert_eq!(next_effective(7, true, 12, 8), 3, "压力减半");
        assert_eq!(next_effective(1, true, 0, 8), 1, "减半有下限 1");
        assert_eq!(next_effective(3, false, 5, 4), 4, "有进展加一");
        assert_eq!(next_effective(4, false, 5, 4), 4, "加一封顶 max");
        assert_eq!(next_effective(2, false, 0, 8), 2, "空窗不抖动");
        assert_eq!(next_effective(2, true, 9, 8), 1, "压力优先于进展");
    }

    #[test]
    fn k1_samples_extend_the_baseline_instead_of_pressuring_the_first_ramp() {
        assert!(!exceeds_k1_baseline(1, 90, 27, 5, 4));
        assert!(exceeds_k1_baseline(2, 126, 100, 5, 4));
        assert!(!exceeds_k1_baseline(2, 125, 100, 5, 4));
    }
}
