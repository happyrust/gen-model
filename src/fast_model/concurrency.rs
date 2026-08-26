//! 全局几何并发闸（specs/023，ADR-041 第 3 条）。
//!
//! 生成路径此前的并行形态是「根内并行、根间串行」，且每处 fan-out 的宽度都是
//! 写死的（`manifold_bool` 固定切 16 块、`occ_generate`/`pdms_inst`/`gen_model`
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
//! 许可只准由**叶子工作任务**持有：一个任务拿着许可期间，不得 await 任何同样
//! 要过闸的子任务。编排层（`gen_model` 的分类 stage 任务这种「spawn 子任务再
//! join」的壳）一律不拿许可——否则额度 = 1 时外层攥着唯一一张许可等内层，内层
//! 永远等不到，直接死锁。落到写法上：
//!
//! - 叶子 fan-out 用 [`spawn_gated_leaf`]，一个任务一张许可，任务结束自动归还；
//! - 编排层继续用 `spawn_with_staged_io`，不碰闸;
//! - 顺序循环里的分批（SQL 写回攒批这种不并发的 chunk）只用 [`GeometryGate::chunk_size`]
//!   派生批宽，**不拿许可**。

use serde::Serialize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 几何并发闸：额度即同时执行的叶子任务数上限。
pub struct GeometryGate {
    quota: usize,
    semaphore: Arc<Semaphore>,
}

impl GeometryGate {
    pub(crate) fn with_quota(quota: usize) -> Self {
        let quota = quota.max(1);
        Self {
            quota,
            semaphore: Arc::new(Semaphore::new(quota)),
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

    /// 取一张许可（RAII，drop 即归还）。只准叶子任务调用，见模块级纪律。
    pub async fn acquire(&self) -> GeometryPermit {
        WAITING.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("geometry gate semaphore is never closed");
        WAITING.fetch_sub(1, Ordering::Relaxed);
        WAIT_MICROS.fetch_add(
            started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
        ACTIVE.fetch_add(1, Ordering::Relaxed);
        GeometryPermit { _permit: permit }
    }
}

/// 闸的许可凭证；持有期间占一份额度。
pub struct GeometryPermit {
    _permit: OwnedSemaphorePermit,
}

impl Drop for GeometryPermit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static WAITING: AtomicUsize = AtomicUsize::new(0);
static WAIT_MICROS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Serialize)]
pub struct GeometryConcurrencySnapshot {
    pub quota: usize,
    pub active: usize,
    pub waiting: usize,
    pub permit_wait_micros: u64,
}

pub fn snapshot() -> GeometryConcurrencySnapshot {
    GeometryConcurrencySnapshot {
        quota: geometry_gate().quota(),
        active: ACTIVE.load(Ordering::Relaxed),
        waiting: WAITING.load(Ordering::Relaxed),
        permit_wait_micros: WAIT_MICROS.load(Ordering::Relaxed),
    }
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

/// 叶子工作任务的统一入口：继承暂存读写上下文，并对全局闸取一张许可。
///
/// 这是生成路径 fan-out 的**唯一**并发形态（T09 的源码形状断言钉的就是这个名
/// 字）；持许可期间不得 await 另一个 gated 任务，见模块级纪律。
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 额度 = 1 时分块宽度 = 全长：单块单任务，就是改动前的串行。
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
    /// 3. 含 fan-out（有 spawn 调用）的文件里不得出现 `.len() / <字面量>` 的
    ///    分块宽度与 `batch_chunks_cnt = <字面量>`：分块宽度一律
    ///    `geometry_gate().chunk_size()`。纯网格文件里的 `indices.len() / 3`
    ///    这类索引算术不在扫描范围（它们不 spawn）。
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
                || source.contains("spawn_with_staged_io(")
                || source.contains("tokio::spawn(");
            if !spawns {
                continue;
            }
            for (idx, line) in source.lines().enumerate() {
                if let Some(rest) = line.split(".len() / ").nth(1) {
                    let literal: String = rest.chars().take_while(char::is_ascii_digit).collect();
                    assert!(
                        literal.is_empty(),
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
}
