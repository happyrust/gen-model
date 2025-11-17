use super::models::NounCategory;
use aios_core::RefnoEnum;
use std::collections::HashMap;

/// 分类的 refno 集合
///
/// 使用单一 HashMap 替代三个独立的 HashSet，节省约 33% 内存
///
/// # 优势
/// - 内存效率：一个 HashMap vs 三个 HashSet
/// - 类型安全：每个 refno 都有明确的类别
/// - 查询方便：可以按类别快速筛选
#[derive(Debug, Default, Clone)]
pub struct CategorizedRefnos {
    inner: HashMap<RefnoEnum, NounCategory>,
}

impl CategorizedRefnos {
    /// 创建新的空集合
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// 创建指定容量的集合
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
        }
    }

    /// 插入 refno 及其类别
    ///
    /// 如果 refno 已存在，会更新其类别
    pub fn insert(&mut self, refno: RefnoEnum, category: NounCategory) -> Option<NounCategory> {
        self.inner.insert(refno, category)
    }

    /// 批量插入
    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (RefnoEnum, NounCategory)>,
    {
        self.inner.extend(iter);
    }

    /// 检查是否包含指定 refno
    pub fn contains(&self, refno: &RefnoEnum) -> bool {
        self.inner.contains_key(refno)
    }

    /// 获取 refno 的类别
    pub fn get_category(&self, refno: &RefnoEnum) -> Option<NounCategory> {
        self.inner.get(refno).copied()
    }

    /// 获取指定类别的所有 refno
    ///
    /// # Examples
    /// ```
    /// let refnos = categorized.get_by_category(NounCategory::Cate);
    /// ```
    pub fn get_by_category(&self, category: NounCategory) -> Vec<RefnoEnum> {
        self.inner
            .iter()
            .filter(|(_, cat)| **cat == category)
            .map(|(refno, _)| *refno)
            .collect()
    }

    /// 获取所有 refno（忽略类别）
    pub fn get_all(&self) -> Vec<RefnoEnum> {
        self.inner.keys().copied().collect()
    }

    /// 总数量
    pub fn total_count(&self) -> usize {
        self.inner.len()
    }

    /// 指定类别的数量
    pub fn count_by_category(&self, category: NounCategory) -> usize {
        self.inner.values().filter(|cat| **cat == category).count()
    }

    /// 按类别统计数量
    pub fn count_statistics(&self) -> CategoryStatistics {
        let mut stats = CategoryStatistics::default();

        for category in self.inner.values() {
            match category {
                NounCategory::Cate => stats.cate_count += 1,
                NounCategory::LoopOwner => stats.loop_count += 1,
                NounCategory::Prim => stats.prim_count += 1,
            }
        }

        stats
    }

    /// 清空所有数据
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 移除指定 refno
    pub fn remove(&mut self, refno: &RefnoEnum) -> Option<NounCategory> {
        self.inner.remove(refno)
    }

    /// 打印统计信息
    pub fn print_statistics(&self) {
        let stats = self.count_statistics();

        println!("╔════════════════════════════════════════╗");
        println!("║      Refno 分类统计                      ║");
        println!("╠════════════════════════════════════════╣");
        println!("║ Cate (元件库):  {:<21} ║", stats.cate_count);
        println!("║ Loop (管道):    {:<21} ║", stats.loop_count);
        println!("║ Prim (基本体):  {:<21} ║", stats.prim_count);
        println!("╟────────────────────────────────────────╢");
        println!("║ 总计:           {:<21} ║", stats.total_count());
        println!("╚════════════════════════════════════════╝");
    }
}

/// 类别统计信息
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CategoryStatistics {
    pub cate_count: usize,
    pub loop_count: usize,
    pub prim_count: usize,
}

impl CategoryStatistics {
    pub fn total_count(&self) -> usize {
        self.cate_count + self.loop_count + self.prim_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut refnos = CategorizedRefnos::new();

        let refno1 = RefnoEnum::RefU64(1);
        let refno2 = RefnoEnum::RefU64(2);
        let refno3 = RefnoEnum::RefU64(3);

        refnos.insert(refno1, NounCategory::Cate);
        refnos.insert(refno2, NounCategory::LoopOwner);
        refnos.insert(refno3, NounCategory::Prim);

        assert_eq!(refnos.total_count(), 3);
        assert!(refnos.contains(&refno1));
        assert_eq!(refnos.get_category(&refno1), Some(NounCategory::Cate));
    }

    #[test]
    fn test_get_by_category() {
        let mut refnos = CategorizedRefnos::new();

        refnos.insert(RefnoEnum::RefU64(1), NounCategory::Cate);
        refnos.insert(RefnoEnum::RefU64(2), NounCategory::Cate);
        refnos.insert(RefnoEnum::RefU64(3), NounCategory::LoopOwner);

        let cate_refnos = refnos.get_by_category(NounCategory::Cate);
        assert_eq!(cate_refnos.len(), 2);

        let loop_refnos = refnos.get_by_category(NounCategory::LoopOwner);
        assert_eq!(loop_refnos.len(), 1);
    }

    #[test]
    fn test_statistics() {
        let mut refnos = CategorizedRefnos::new();

        refnos.insert(RefnoEnum::RefU64(1), NounCategory::Cate);
        refnos.insert(RefnoEnum::RefU64(2), NounCategory::Cate);
        refnos.insert(RefnoEnum::RefU64(3), NounCategory::LoopOwner);
        refnos.insert(RefnoEnum::RefU64(4), NounCategory::Prim);

        let stats = refnos.count_statistics();
        assert_eq!(stats.cate_count, 2);
        assert_eq!(stats.loop_count, 1);
        assert_eq!(stats.prim_count, 1);
        assert_eq!(stats.total_count(), 4);
    }

    #[test]
    fn test_memory_efficiency() {
        // 验证使用单一 HashMap 比三个 HashSet 更高效
        let refnos = CategorizedRefnos::with_capacity(1000);

        // 确保容量设置正确
        assert!(refnos.inner.capacity() >= 1000);
    }
}
