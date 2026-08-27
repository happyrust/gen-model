//! tokio 调度延迟采样（specs/033 T003）。
//!
//! ADR-052 第 3 条要把几何 CPU 段挪出 tokio worker，理由是「三角化和布尔占住 worker，
//! shape receiver、SurrealDB response、watcher、`/health` 与 timer 被挤掉调度」。这句话
//! 在改动前无法证伪——没有任何一个数说得清调度被挤到了什么程度，只有「/health 感觉
//! 有点卡」这种印象。本模块补的就是这个数。
//!
//! 手法是最朴素的那种：一个只睡觉的任务，量自己「睡 100ms」实际睡了多久，超出的部分
//! 就是它排队等 worker 的时间。每一轮独立计时、不追赶进度，所以一次 3 秒的卡顿产生
//! 一个 3 秒的样本，而不是三十个越来越小的样本把分位数冲淡。
//!
//! 读法与边界：
//!
//! - p50 是常态排队，p99 是最坏那一次被压了多久；`max_micros` 是进程期最坏值，
//!   它不随窗口滚走——现场最要紧的那一次卡顿往往发生在几十分钟前。
//! - tokio 的定时器本身有约 1ms 粒度，空载读数不是 0 而是零点几毫秒，别把它当回归。
//! - 它测的是**整个 runtime** 的拥挤程度，不区分是谁占的。要归因到几何，得跟
//!   `geometry_concurrency` 的在飞数与持有时长对着看。
//! - 没在采样和延迟为 0 是两回事：`sampling = false` 时分位数一律 `None`，
//!   不报 0。`model_concurrency` 那个恒为 0 的 `shape_queue_depth` 就是反面例子。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

/// 采样周期。100ms 在「看得见亚秒级卡顿」与「自己不成为负载」之间。
const SAMPLE_PERIOD: Duration = Duration::from_millis(100);
/// 保留的样本数，约 51 秒的滚动窗口——够覆盖一个 execution group 的 CPU 密集段。
const MAX_SAMPLES: usize = 512;

static WINDOW: OnceLock<Mutex<LagWindow>> = OnceLock::new();
static SAMPLER_STARTED: OnceLock<()> = OnceLock::new();

#[derive(Debug, Default)]
struct LagWindow {
    samples: VecDeque<u64>,
    observed_total: u64,
    max_micros: u64,
}

impl LagWindow {
    fn record(&mut self, lag: Duration) {
        let micros = lag.as_micros().min(u128::from(u64::MAX)) as u64;
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(micros);
        self.observed_total = self.observed_total.saturating_add(1);
        self.max_micros = self.max_micros.max(micros);
    }

    fn render(&self, sampling: bool) -> RuntimeLagSnapshot {
        // 分位数与 `model_concurrency` 共用同一份实现：两个 /health 区块的 p95 必须
        // 是同一种 p95，否则拿它们对照就是在比两把不同的尺子。
        let percentile = crate::data_interface::model_concurrency::percentile;
        RuntimeLagSnapshot {
            sampling,
            sample_period_ms: SAMPLE_PERIOD.as_millis() as u64,
            samples: self.samples.len(),
            observed_total: self.observed_total,
            p50_micros: percentile(&self.samples, 50),
            p95_micros: percentile(&self.samples, 95),
            p99_micros: percentile(&self.samples, 99),
            max_micros: (self.observed_total > 0).then_some(self.max_micros),
        }
    }
}

fn window() -> &'static Mutex<LagWindow> {
    WINDOW.get_or_init(|| Mutex::new(LagWindow::default()))
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RuntimeLagSnapshot {
    /// 采样任务是否起来了。false 时下面的分位数一律为空——「没在测」不许长得像
    /// 「测出来是 0」。
    pub sampling: bool,
    pub sample_period_ms: u64,
    /// 当前滚动窗口里的样本数。
    pub samples: usize,
    /// 进程期累计采样次数。它与 `samples` 的差就是已经滚出窗口的部分。
    pub observed_total: u64,
    pub p50_micros: Option<u64>,
    pub p95_micros: Option<u64>,
    pub p99_micros: Option<u64>,
    /// 进程期最坏一次，不随窗口滚走。
    pub max_micros: Option<u64>,
}

pub fn snapshot() -> RuntimeLagSnapshot {
    let sampling = SAMPLER_STARTED.get().is_some();
    window()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .render(sampling)
}

/// 起采样任务。幂等：`run_app` 转手调 `run_cli`，两条入口都会喊一声。
///
/// 必须在 tokio runtime 里调用。任务本身不持有任何锁跨 `.await`，也不碰数据库——
/// 它要量的正是调度，自己再去排队等别的资源就测不准了。
pub fn spawn_sampler() {
    if SAMPLER_STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let started = tokio::time::Instant::now();
            tokio::time::sleep(SAMPLE_PERIOD).await;
            // 超出周期的部分才是排队。每轮独立计时，不补追落后的轮次：卡顿 3 秒
            // 应当留下一个 3 秒的样本，而不是三十个把 p50 拉回正常的小样本。
            let lag = started.elapsed().saturating_sub(SAMPLE_PERIOD);
            window()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record(lag);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个样本都没有时，分位数与最坏值都是 `None`——不是 0。这条是 specs/033 FR-6
    /// 的同一条纪律：读数缺席要看得出来。
    #[test]
    fn an_idle_window_reports_nothing_rather_than_zero() {
        let empty = LagWindow::default();

        let not_started = empty.render(false);
        assert!(!not_started.sampling);
        assert_eq!(not_started.samples, 0);
        assert_eq!(not_started.observed_total, 0);
        assert_eq!(not_started.p50_micros, None);
        assert_eq!(not_started.max_micros, None, "没测过不等于最坏是 0");

        let started_but_no_sample_yet = empty.render(true);
        assert!(started_but_no_sample_yet.sampling);
        assert_eq!(started_but_no_sample_yet.p99_micros, None);
    }

    /// 窗口滚动只丢样本，不丢账：累计次数继续涨，进程期最坏值不被后来的平静冲掉。
    /// 现场那次最狠的卡顿通常发生在几十分钟前，只留滚动窗口等于没留。
    #[test]
    fn the_window_rolls_but_the_worst_sample_survives() {
        let mut window = LagWindow::default();
        window.record(Duration::from_millis(900));
        for _ in 0..MAX_SAMPLES {
            window.record(Duration::from_micros(200));
        }

        let rendered = window.render(true);
        assert_eq!(rendered.samples, MAX_SAMPLES, "窗口不许无限长");
        assert_eq!(
            rendered.observed_total,
            MAX_SAMPLES as u64 + 1,
            "滚出去的样本仍然要计数"
        );
        assert_eq!(
            rendered.p50_micros,
            Some(200),
            "900ms 那条已滚出窗口，分位数只反映当下"
        );
        assert_eq!(
            rendered.max_micros,
            Some(900_000),
            "进程期最坏值不随窗口滚走"
        );
    }

    /// 分位数按样本值排序，不看到达顺序，与 `model_concurrency` 同一实现（最近邻、
    /// 不插值）；单样本时三个分位数都等于它。
    ///
    /// 顺带钉住一件容易被误读的事：样本少的时候 p99 撑不起来——五个样本的
    /// `(n-1) × 99 / 100` 仍然落在第四个，最坏的那次进不了分位数。这正是
    /// `max_micros` 必须单独存在的理由，别拿 p99 当最坏值看。
    #[test]
    fn percentiles_come_from_the_samples_not_from_the_arrival_order() {
        let mut window = LagWindow::default();
        for micros in [50_u64, 5_000, 100, 200, 150] {
            window.record(Duration::from_micros(micros));
        }
        let rendered = window.render(true);
        assert_eq!(rendered.p50_micros, Some(150));
        assert_eq!(rendered.p95_micros, Some(200));
        assert_eq!(rendered.p99_micros, Some(200), "五个样本撑不起 p99");
        assert_eq!(rendered.max_micros, Some(5_000), "最坏值只在 max_micros 里");
        assert_eq!(rendered.samples, 5);

        let mut single = LagWindow::default();
        single.record(Duration::from_micros(7));
        let rendered = single.render(true);
        assert_eq!(rendered.p50_micros, Some(7));
        assert_eq!(rendered.p95_micros, Some(7));
        assert_eq!(rendered.p99_micros, Some(7));
    }
}
