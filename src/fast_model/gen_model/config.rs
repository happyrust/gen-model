use std::num::NonZeroUsize;
use aios_core::options::DbOption;
use super::errors::{FullNounError, Result};

/// 类型安全的并发配置
///
/// 保证并发数始终在有效范围内（2-8）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Concurrency(NonZeroUsize);

impl Concurrency {
    /// 最小并发数
    pub const MIN: usize = 2;

    /// 最大并发数
    pub const MAX: usize = 8;

    /// 默认并发数
    pub const DEFAULT: usize = 4;

    /// 创建新的并发配置
    ///
    /// # Arguments
    /// * `n` - 并发数，会自动限制在 MIN-MAX 范围内
    ///
    /// # Errors
    /// * 如果 n 为 0，返回 InvalidConcurrency 错误
    ///
    /// # Examples
    /// ```
    /// let concurrency = Concurrency::new(6)?; // Ok(6)
    /// let concurrency = Concurrency::new(10)?; // Ok(8) - 自动限制
    /// let concurrency = Concurrency::new(0)?; // Err - 无效值
    /// ```
    pub fn new(n: usize) -> Result<Self> {
        if n == 0 {
            return Err(FullNounError::InvalidConcurrency(n, Self::MIN, Self::MAX));
        }

        let clamped = n.clamp(Self::MIN, Self::MAX);

        // 如果值被修正，发出警告
        if clamped != n {
            log::warn!(
                "并发数 {} 超出范围，已自动调整为 {}（范围：{}-{}）",
                n, clamped, Self::MIN, Self::MAX
            );
        }

        // SAFETY: clamped 范围是 [MIN, MAX]，MIN >= 2，所以不可能为 0
        Ok(Self(unsafe { NonZeroUsize::new_unchecked(clamped) }))
    }

    /// 创建默认并发配置
    pub fn default() -> Self {
        // SAFETY: DEFAULT = 4，不为 0
        Self(unsafe { NonZeroUsize::new_unchecked(Self::DEFAULT) })
    }

    /// 获取并发数值
    pub fn get(&self) -> usize {
        self.0.get()
    }
}

impl Default for Concurrency {
    fn default() -> Self {
        Self::default()
    }
}

/// 类型安全的批次大小配置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchSize(NonZeroUsize);

impl BatchSize {
    /// 最小批次大小
    pub const MIN: usize = 10;

    /// 最大批次大小
    pub const MAX: usize = 1000;

    /// 默认批次大小
    pub const DEFAULT: usize = 100;

    /// 创建新的批次大小配置
    pub fn new(n: usize) -> Result<Self> {
        if n == 0 {
            return Err(FullNounError::InvalidBatchSize(n));
        }

        let clamped = n.clamp(Self::MIN, Self::MAX);

        if clamped != n {
            log::warn!(
                "批次大小 {} 超出范围，已自动调整为 {}（范围：{}-{}）",
                n, clamped, Self::MIN, Self::MAX
            );
        }

        Ok(Self(unsafe { NonZeroUsize::new_unchecked(clamped) }))
    }

    /// 创建默认批次大小
    pub fn default() -> Self {
        Self(unsafe { NonZeroUsize::new_unchecked(Self::DEFAULT) })
    }

    /// 获取批次大小值
    pub fn get(&self) -> usize {
        self.0.get()
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self::default()
    }
}

/// Full Noun 模式的统一配置
///
/// 封装所有 Full Noun 相关配置，提供类型安全和验证
#[derive(Debug, Clone)]
pub struct FullNounConfig {
    /// 是否启用 Full Noun 模式
    pub enabled: bool,

    /// 并发处理的 Noun 数量
    pub concurrency: Concurrency,

    /// 每批次处理的 refno 数量
    pub batch_size: BatchSize,

    /// 是否验证 SJUS map（建议启用）
    pub validate_sjus_map: bool,

    /// 是否在验证失败时严格报错（false 则只警告）
    pub strict_validation: bool,

    /// 启用的 noun 类别/名称列表，空表示启用所有
    pub enabled_categories: Vec<String>,

    /// 禁用的 noun 列表
    pub excluded_nouns: Vec<String>,
}

impl FullNounConfig {
    /// 从 DbOption 创建配置
    ///
    /// # Arguments
    /// * `opt` - 数据库配置选项
    ///
    /// # Errors
    /// * 如果并发数或批次大小无效
    ///
    /// 注意：由于 DbOption 在 aios-core 中可能没有 full_noun_* 字段，
    /// 这个函数用于兼容性。实际使用时建议使用 from_db_option_ext。
    pub fn from_db_option(_opt: &DbOption) -> Result<Self> {
        // 使用默认配置，因为标准 DbOption 可能没有这些字段
        Ok(Self::default())
    }

    /// 从 DbOptionExt 创建配置（推荐）
    ///
    /// DbOptionExt 在 src/options.rs 中定义，包含 Full Noun 相关字段
    pub fn from_db_option_ext(opt: &crate::options::DbOptionExt) -> Result<Self> {
        let concurrency = Concurrency::new(opt.get_full_noun_concurrency())?;
        let batch_size = BatchSize::new(opt.get_full_noun_batch_size())?;

        Ok(Self {
            enabled: opt.full_noun_mode,
            concurrency,
            batch_size,
            validate_sjus_map: true,  // 默认启用验证
            strict_validation: false,  // 默认只警告，不报错
            enabled_categories: opt.full_noun_enabled_categories.clone(),
            excluded_nouns: opt.full_noun_excluded_nouns.clone(),
        })
    }

    /// 创建默认配置
    pub fn default() -> Self {
        Self {
            enabled: false,
            concurrency: Concurrency::default(),
            batch_size: BatchSize::default(),
            validate_sjus_map: true,
            strict_validation: false,
            enabled_categories: Vec::new(),
            excluded_nouns: Vec::new(),
        }
    }

    /// 构建器模式：设置是否启用
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// 构建器模式：设置并发数
    pub fn with_concurrency(mut self, concurrency: Concurrency) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// 构建器模式：设置批次大小
    pub fn with_batch_size(mut self, batch_size: BatchSize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// 构建器模式：设置严格验证
    pub fn with_strict_validation(mut self, strict: bool) -> Self {
        self.strict_validation = strict;
        self
    }

    /// 构建器模式：设置启用的类别
    pub fn with_enabled_categories(mut self, categories: Vec<String>) -> Self {
        self.enabled_categories = categories;
        self
    }

    /// 构建器模式：设置排除的 noun
    pub fn with_excluded_nouns(mut self, nouns: Vec<String>) -> Self {
        self.excluded_nouns = nouns;
        self
    }

    /// 检查 noun 类别是否启用
    pub fn is_category_enabled(&self, category: &str) -> bool {
        self.enabled_categories.is_empty()
            || self.enabled_categories.iter()
                .any(|cat| cat == category || cat.to_lowercase() == category.to_lowercase())
    }

    /// 检查具体 noun 是否被排除
    pub fn is_noun_excluded(&self, noun: &str) -> bool {
        self.excluded_nouns.iter()
            .any(|excluded| excluded == noun || excluded.to_lowercase() == noun.to_lowercase())
    }

    /// 检查具体 noun 是否应该处理
    /// 综合考虑类别启用和 noun 排除
    pub fn should_process_noun(&self, noun: &str, category: &str) -> bool {
        // 如果被明确排除，则不处理
        if self.is_noun_excluded(noun) {
            return false;
        }

        // 如果启用了具体 noun 名称，优先检查
        let has_explicit_nouns = self.enabled_categories.iter()
            .any(|cat| !matches!(cat.to_lowercase().as_str(), "cate" | "loop" | "prim"));

        if has_explicit_nouns {
            // 如果有具体的 noun 名称，则检查 noun 是否在列表中
            return self.enabled_categories.iter()
                .any(|cat| cat == noun || cat.to_lowercase() == noun.to_lowercase());
        }

        // 否则检查类别是否启用
        self.is_category_enabled(category)
    }

    /// 打印配置信息
    pub fn print_info(&self) {
        println!("╔════════════════════════════════════════╗");
        println!("║    Full Noun 模式配置                    ║");
        println!("╠════════════════════════════════════════╣");
        println!("║ 启用状态: {:<28} ║", if self.enabled { "✅ 已启用" } else { "❌ 未启用" });
        println!("║ 并发 Noun 数: {:<24} ║", self.concurrency.get());
        println!("║ 批次大小: {:<28} ║", self.batch_size.get());
        println!("║ SJUS 验证: {:<27} ║", if self.validate_sjus_map { "✅ 启用" } else { "❌ 禁用" });
        println!("║ 严格模式: {:<28} ║", if self.strict_validation { "✅ 启用" } else { "❌ 禁用" });
        
        if !self.enabled_categories.is_empty() {
            println!("╠════════════════════════════════════════╣");
            println!("║ 启用类别: {:<27} ║", self.enabled_categories.join(", "));
        }
        
        if !self.excluded_nouns.is_empty() {
            println!("╠════════════════════════════════════════╣");
            println!("║ 排除 Noun: {:<26} ║", self.excluded_nouns.join(", "));
        }
        
        println!("╚════════════════════════════════════════╝");
    }
}

impl Default for FullNounConfig {
    fn default() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrency_valid_range() {
        let c1 = Concurrency::new(4).unwrap();
        assert_eq!(c1.get(), 4);

        let c2 = Concurrency::new(2).unwrap();
        assert_eq!(c2.get(), 2);

        let c3 = Concurrency::new(8).unwrap();
        assert_eq!(c3.get(), 8);
    }

    #[test]
    fn test_concurrency_clamping() {
        // 超出最大值
        let c1 = Concurrency::new(100).unwrap();
        assert_eq!(c1.get(), Concurrency::MAX);

        // 低于最小值但不为 0
        let c2 = Concurrency::new(1).unwrap();
        assert_eq!(c2.get(), Concurrency::MIN);
    }

    #[test]
    fn test_concurrency_zero_error() {
        let result = Concurrency::new(0);
        assert!(result.is_err());

        if let Err(FullNounError::InvalidConcurrency(val, min, max)) = result {
            assert_eq!(val, 0);
            assert_eq!(min, Concurrency::MIN);
            assert_eq!(max, Concurrency::MAX);
        } else {
            panic!("Expected InvalidConcurrency error");
        }
    }

    #[test]
    fn test_batch_size() {
        let b1 = BatchSize::new(100).unwrap();
        assert_eq!(b1.get(), 100);

        let b2 = BatchSize::new(0);
        assert!(b2.is_err());

        let b3 = BatchSize::new(5000).unwrap();
        assert_eq!(b3.get(), BatchSize::MAX);
    }

    #[test]
    fn test_config_builder() {
        let config = FullNounConfig::default()
            .with_enabled(true)
            .with_concurrency(Concurrency::new(6).unwrap())
            .with_strict_validation(true);

        assert!(config.enabled);
        assert_eq!(config.concurrency.get(), 6);
        assert!(config.strict_validation);
    }

    #[test]
    fn test_config_from_db_option() {
        let mut db_opt = DbOption::default();
        db_opt.full_noun_mode = true;
        db_opt.full_noun_max_concurrent_nouns = 6;
        db_opt.full_noun_batch_size = 200;

        let config = FullNounConfig::from_db_option(&db_opt).unwrap();

        assert!(config.enabled);
        assert_eq!(config.concurrency.get(), 6);
        assert_eq!(config.batch_size.get(), 200);
    }
}
