//! 暂存资源三级状态机（ADR-017 §2 / 开发方案 T0.3、R3）。
//!
//! 长窗口（冻结吸收放大区间）+ CATA 闭包 + 生成产物会持续推高暂存内存与
//! journal 体量，而暂存活在进程堆里——不允许走到 OOM。治理分三档：
//! 告警阈值只告警；更高阈值拒绝吸收扩窗（后继排队行保持独立窗口）；
//! 极限阈值废弃暂存并转入资源阻断告警（数据无损：水位没动，窗口重算）。
//!
//! 计量口径：暂存执行的 SQL 字节 + journal 字节计入同一配额，行数一并观测。
//! 这是**摄入量代理**而不是精确堆占用——mem 引擎不暴露每库字节数，代理量
//! 单调、便宜、与真实占用同数量级，足以在 OOM 之前拉响三级动作。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 资源档位，按严重度排序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceBand {
    /// 正常。
    Normal,
    /// 告警：观测与日志，动作照旧。
    Warn,
    /// 拒绝吸收扩窗：本窗口继续，但重扫抬高上界的会话改走后继独立窗口。
    RefuseAbsorb,
    /// 废弃暂存：立即停止摄入，窗口转资源阻断告警（上层负责废弃与重算）。
    Abandon,
}

/// 三级阈值（对「暂存 SQL 字节 + journal 字节」的合计配额）。
#[derive(Clone, Copy, Debug)]
pub struct ResourceThresholds {
    pub warn_bytes: u64,
    pub refuse_absorb_bytes: u64,
    pub abandon_bytes: u64,
    pub warn_rows: u64,
    pub refuse_absorb_rows: u64,
    pub abandon_rows: u64,
}

impl Default for ResourceThresholds {
    /// 规模前提（ADR-017 结果/约束）：目标项目 .rdb < 5GB、内存预算 2–3 倍。
    /// 单窗口摄入配额取其零头；真实项目的调参入口留给 T4.3（配置 + /health）。
    fn default() -> Self {
        Self {
            warn_bytes: env_u64("AIOS_STAGING_WARN_BYTES", 512 * 1024 * 1024),
            refuse_absorb_bytes: env_u64("AIOS_STAGING_REFUSE_ABSORB_BYTES", 1024 * 1024 * 1024),
            abandon_bytes: env_u64("AIOS_STAGING_ABANDON_BYTES", 2 * 1024 * 1024 * 1024),
            warn_rows: env_u64("AIOS_STAGING_WARN_ROWS", 1_000_000),
            refuse_absorb_rows: env_u64("AIOS_STAGING_REFUSE_ABSORB_ROWS", 2_000_000),
            abandon_rows: env_u64("AIOS_STAGING_ABANDON_ROWS", 4_000_000),
        }
    }
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

/// 一个提交单元的资源观测面板。执行器在每次成功执行后记账；
/// 生命周期（吸收判定）与上层状态机按 [`ResourceGauge::band`] 行动。
#[derive(Debug)]
pub struct ResourceGauge {
    thresholds: ResourceThresholds,
    staged_sql_bytes: AtomicU64,
    journal_bytes: AtomicU64,
    journal_entries: AtomicU64,
    staged_statements: AtomicU64,
    estimated_write_rows: AtomicU64,
}

impl ResourceGauge {
    pub fn new(thresholds: ResourceThresholds) -> Arc<Self> {
        Arc::new(Self {
            thresholds,
            staged_sql_bytes: AtomicU64::new(0),
            journal_bytes: AtomicU64::new(0),
            journal_entries: AtomicU64::new(0),
            staged_statements: AtomicU64::new(0),
            estimated_write_rows: AtomicU64::new(0),
        })
    }

    pub fn with_defaults() -> Arc<Self> {
        Self::new(ResourceThresholds::default())
    }

    pub fn record_staged(&self, sql_bytes: usize) {
        self.staged_sql_bytes
            .fetch_add(sql_bytes as u64, Ordering::Relaxed);
        self.staged_statements.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_journal(&self, sql_bytes: usize) {
        self.journal_bytes
            .fetch_add(sql_bytes as u64, Ordering::Relaxed);
        self.journal_entries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_write_rows(&self, rows: u64) {
        self.estimated_write_rows.fetch_add(rows, Ordering::Relaxed);
    }

    /// 合计摄入量（同一配额：暂存 SQL 字节 + journal 字节）。
    pub fn total_bytes(&self) -> u64 {
        self.staged_sql_bytes.load(Ordering::Relaxed) + self.journal_bytes.load(Ordering::Relaxed)
    }

    pub fn band(&self) -> ResourceBand {
        self.projected_band(0, 0)
    }

    pub fn projected_band(&self, additional_bytes: u64, additional_rows: u64) -> ResourceBand {
        let total = self.total_bytes().saturating_add(additional_bytes);
        let rows = self
            .estimated_write_rows
            .load(Ordering::Relaxed)
            .saturating_add(additional_rows);
        if total >= self.thresholds.abandon_bytes || rows >= self.thresholds.abandon_rows {
            ResourceBand::Abandon
        } else if total >= self.thresholds.refuse_absorb_bytes
            || rows >= self.thresholds.refuse_absorb_rows
        {
            ResourceBand::RefuseAbsorb
        } else if total >= self.thresholds.warn_bytes || rows >= self.thresholds.warn_rows {
            ResourceBand::Warn
        } else {
            ResourceBand::Normal
        }
    }

    /// /health 与任务面板用的快照（T4.3 接线）。
    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            band: self.band(),
            staged_sql_bytes: self.staged_sql_bytes.load(Ordering::Relaxed),
            journal_bytes: self.journal_bytes.load(Ordering::Relaxed),
            journal_entries: self.journal_entries.load(Ordering::Relaxed),
            staged_statements: self.staged_statements.load(Ordering::Relaxed),
            estimated_write_rows: self.estimated_write_rows.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResourceSnapshot {
    pub band: ResourceBand,
    pub staged_sql_bytes: u64,
    pub journal_bytes: u64,
    pub journal_entries: u64,
    pub staged_statements: u64,
    pub estimated_write_rows: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Arc<ResourceGauge> {
        ResourceGauge::new(ResourceThresholds {
            warn_bytes: 10,
            refuse_absorb_bytes: 20,
            abandon_bytes: 30,
            warn_rows: 100,
            refuse_absorb_rows: 200,
            abandon_rows: 300,
        })
    }

    #[test]
    fn bands_escalate_with_combined_intake() {
        let gauge = tiny();
        assert_eq!(gauge.band(), ResourceBand::Normal);

        gauge.record_staged(6); // 暂存 6 字节
        assert_eq!(gauge.band(), ResourceBand::Normal);

        gauge.record_journal(5); // 合计 11 ≥ 10 → 告警
        assert_eq!(gauge.band(), ResourceBand::Warn);

        gauge.record_staged(9); // 合计 20 ≥ 20 → 拒绝吸收
        assert_eq!(gauge.band(), ResourceBand::RefuseAbsorb);

        gauge.record_journal(10); // 合计 30 ≥ 30 → 废弃暂存
        assert_eq!(gauge.band(), ResourceBand::Abandon);

        let snap = gauge.snapshot();
        assert_eq!(snap.staged_sql_bytes, 15);
        assert_eq!(snap.journal_bytes, 15);
        assert_eq!(snap.staged_statements, 2);
        assert_eq!(snap.journal_entries, 2);
        assert_eq!(snap.estimated_write_rows, 0);
    }

    #[test]
    fn projected_rows_escalate_before_recording() {
        let gauge = tiny();
        assert_eq!(gauge.projected_band(0, 300), ResourceBand::Abandon);
        assert_eq!(gauge.band(), ResourceBand::Normal);
    }
}
