use aios_core::options::DbOption;
use std::sync::Arc;

/// Noun 处理上下文，包含所有处理过程需要的配置信息
#[derive(Clone)]
pub struct NounProcessContext {
    pub db_option: Arc<DbOption>,
    pub batch_size: usize,
    pub batch_concurrency: usize,
}

impl NounProcessContext {
    /// 创建新的处理上下文
    ///
    /// # Arguments
    /// * `db_option` - 数据库配置
    /// * `batch_size` - 每批次处理的数量
    /// * `batch_concurrency` - 批次处理的并发数（自动限制最小为1）
    pub fn new(db_option: Arc<DbOption>, batch_size: usize, batch_concurrency: usize) -> Self {
        Self {
            db_option,
            batch_size,
            batch_concurrency: batch_concurrency.max(1),
        }
    }

    /// 根据总数计算分批范围
    ///
    /// 返回 (start, end) 范围列表，用于分页查询
    ///
    /// # Example
    /// ```
    /// let ctx = NounProcessContext::new(db_option, 100, 4);
    /// let ranges = ctx.bounded_chunks(350);
    /// // 返回: [(0, 100), (100, 200), (200, 300), (300, 350)]
    /// ```
    pub fn bounded_chunks(&self, total: usize) -> Vec<(usize, usize)> {
        if total == 0 {
            return vec![];
        }

        let chunk = self.batch_size.max(1);
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < total {
            let end = (start + chunk).min(total);
            ranges.push((start, end));
            start = end;
        }
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_chunks() {
        let ctx = NounProcessContext {
            db_option: Arc::new(DbOption::default()),
            batch_size: 100,
            batch_concurrency: 4,
        };

        // 测试正常情况
        let ranges = ctx.bounded_chunks(350);
        assert_eq!(ranges, vec![(0, 100), (100, 200), (200, 300), (300, 350)]);

        // 测试空情况
        assert_eq!(ctx.bounded_chunks(0), vec![]);

        // 测试小于batch_size的情况
        assert_eq!(ctx.bounded_chunks(50), vec![(0, 50)]);
    }

    #[test]
    fn test_batch_concurrency_minimum() {
        let ctx = NounProcessContext::new(Arc::new(DbOption::default()), 100, 0);
        assert_eq!(ctx.batch_concurrency, 1); // 自动修正为最小值1
    }
}
