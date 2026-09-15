//! 数据面事务切块上限（`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §3 D3）。
//!
//! 此前 `dbnum_stats` 借的是 `staging::executor` 的三个常量（64 KiB / 250 行 / 100 元素）——那是
//! 暂存期 `mem://` 的口径，而且寄居在 spec 035 P3 要删的目录里。模型侧在同一台 ws + rocksdb 上
//! 「渲染 > 2 MiB 劈半、单块 ≤ 8 MiB」已被 7997 真库验证（模型侧后来收成「一根一笔、超上限
//! 就整根失败」，那套切块退役了，但这个量级的结论照旧成立），
//! ADR-017 记录的事故是整窗口**单事务**（几十 MiB），不是 2 MiB 级。数据面按同一量级重定：
//!
//! | 常量 | 默认 | `DbOption.toml` 键 |
//! |---|---|---|
//! | 一笔事务的 SQL 字节上限 | 2 MiB | `data_tx_max_bytes` |
//! | 一笔事务的写行上限 | 5000 | `data_tx_max_write_rows` |
//! | 一块的元素数上限 | 1000 | `data_tx_elements` |
//!
//! 三个都是**上限**：超过任一项就把块劈半（`element_batch` / `pe_batch::batches` 已有的形状）。
//! `/health` 报生效值。Q6 的取舍：先 2 MiB，P0 实测单笔超 3 s 再降。

/// 一笔数据面事务的 SQL 字节上限（默认 2 MiB）。
pub const DATA_TX_MAX_BYTES_DEFAULT: usize = 2 * 1024 * 1024;
/// 一笔数据面事务的写行上限（默认 5000）。
pub const DATA_TX_MAX_WRITE_ROWS_DEFAULT: u64 = 5000;
/// 一块的元素数上限（默认 1000）。
pub const DATA_TX_ELEMENTS_DEFAULT: usize = 1000;

/// 生效的三个上限；配置里没写就是默认值，写了 0 或负数按默认处理（0 元素一块是死循环）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DataTxLimits {
    pub max_bytes: usize,
    pub max_write_rows: u64,
    pub elements: usize,
}

impl Default for DataTxLimits {
    fn default() -> Self {
        Self {
            max_bytes: DATA_TX_MAX_BYTES_DEFAULT,
            max_write_rows: DATA_TX_MAX_WRITE_ROWS_DEFAULT,
            elements: DATA_TX_ELEMENTS_DEFAULT,
        }
    }
}

impl DataTxLimits {
    /// 从配置扩展字段合成；`None` / 0 一律回默认。
    pub fn from_config(
        max_bytes: Option<u64>,
        max_write_rows: Option<u64>,
        elements: Option<u64>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            max_bytes: max_bytes
                .filter(|value| *value > 0)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(defaults.max_bytes),
            max_write_rows: max_write_rows
                .filter(|value| *value > 0)
                .unwrap_or(defaults.max_write_rows),
            elements: elements
                .filter(|value| *value > 0)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(defaults.elements),
        }
    }
}

/// 本进程生效的上限（读 `DbOption.toml` 的三个扩展键）。
pub fn data_tx_limits() -> DataTxLimits {
    let (max_bytes, max_write_rows, elements) = crate::options::data_tx_limits_config();
    DataTxLimits::from_config(max_bytes, max_write_rows, elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_rocksdb_caliber_not_the_staging_one() {
        let limits = DataTxLimits::default();
        assert_eq!(limits.max_bytes, 2 * 1024 * 1024);
        assert_eq!(limits.max_write_rows, 5000);
        assert_eq!(limits.elements, 1000);
    }

    #[test]
    fn zero_and_missing_fall_back_to_defaults() {
        assert_eq!(
            DataTxLimits::from_config(None, None, None),
            DataTxLimits::default()
        );
        assert_eq!(
            DataTxLimits::from_config(Some(0), Some(0), Some(0)),
            DataTxLimits::default()
        );
        let custom = DataTxLimits::from_config(Some(1024), Some(7), Some(3));
        assert_eq!(
            (custom.max_bytes, custom.max_write_rows, custom.elements),
            (1024, 7, 3)
        );
    }
}
