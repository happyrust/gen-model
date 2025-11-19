use super::models::NounCategory;
use aios_core::pdms_types::{
    GNERAL_LOOP_OWNER_NOUN_NAMES, GNERAL_PRIM_NOUN_NAMES, USE_CATE_NOUN_NAMES,
};
use std::collections::HashSet;

/// Full Noun 模式下的 Noun 列表聚合结果
#[derive(Debug, Clone)]
pub struct FullNounCollection {
    /// 按类别分组的 Noun 列表
    pub cate_nouns: Vec<&'static str>,
    pub loop_owner_nouns: Vec<&'static str>,
    pub prim_nouns: Vec<&'static str>,
    /// 所有 Noun 的去重集合（用于快速查找）
    pub all_nouns: HashSet<&'static str>,
}

impl FullNounCollection {
    /// 聚合并去重所有 Noun 列表（兼容版本）
    ///
    /// 从 pdms_types 中的常量收集：
    /// - USE_CATE_NOUN_NAMES
    /// - GNERAL_LOOP_OWNER_NOUN_NAMES
    /// - GNERAL_PRIM_NOUN_NAMES
    ///
    /// 可选的 extra_nouns 用于扩展（调试或特殊场景）
    pub fn collect(extra_nouns: Option<&[&'static str]>) -> Self {
        Self::collect_with_config(extra_nouns, None)
    }

    /// 聚合并去重 Noun 列表，支持配置过滤
    ///
    /// 根据 config 过滤启用的 noun 类别和具体 noun
    pub fn collect_with_config(
        extra_nouns: Option<&[&'static str]>,
        config: Option<&super::config::FullNounConfig>,
    ) -> Self {
        // 收集 cate nouns（仅在类别内部去重，不做跨类别互斥）
        let mut cate_nouns = Vec::new();
        for &noun in USE_CATE_NOUN_NAMES.iter() {
            // 应用配置过滤
            if let Some(config) = config {
                if !config.should_process_noun(noun, "cate") {
                    continue;
                }
            }

            if !cate_nouns.contains(&noun) {
                cate_nouns.push(noun);
            }
        }

        // 收集 loop owner nouns
        let mut loop_owner_nouns = Vec::new();
        for &noun in GNERAL_LOOP_OWNER_NOUN_NAMES.iter() {
            // 应用配置过滤
            if let Some(config) = config {
                if !config.should_process_noun(noun, "loop") {
                    continue;
                }
            }

            if !loop_owner_nouns.contains(&noun) {
                loop_owner_nouns.push(noun);
            }
        }

        // 收集 prim nouns
        let mut prim_nouns = Vec::new();
        for &noun in GNERAL_PRIM_NOUN_NAMES.iter() {
            // 应用配置过滤
            if let Some(config) = config {
                if !config.should_process_noun(noun, "prim") {
                    continue;
                }
            }

            if !prim_nouns.contains(&noun) {
                prim_nouns.push(noun);
            }
        }

        // 添加额外的 nouns（如果提供）
        if let Some(extras) = extra_nouns {
            for &noun in extras {
                // 应用配置过滤
                if let Some(config) = config {
                    if !config.should_process_noun(noun, "cate") {
                        continue;
                    }
                }

                // 简单策略：额外的 noun 默认归入 cate 类别
                // 实际使用时可以根据需要调整
                if !cate_nouns.contains(&noun)
                    && !loop_owner_nouns.contains(&noun)
                    && !prim_nouns.contains(&noun)
                {
                    cate_nouns.push(noun);
                }
            }
        }

        // 汇总所有 noun，构建去重集合（允许同一个 noun 同时属于多个类别）
        let mut all_nouns = HashSet::new();
        for &noun in cate_nouns
            .iter()
            .chain(loop_owner_nouns.iter())
            .chain(prim_nouns.iter())
        {
            all_nouns.insert(noun);
        }
        if let Some(extras) = extra_nouns {
            for &noun in extras {
                all_nouns.insert(noun);
            }
        }

        // 如果有配置，打印过滤信息
        if let Some(config) = config {
            if !config.enabled_categories.is_empty() && !config.excluded_nouns.is_empty() {
                println!(
                    "🔍 Noun 过滤: 启用 {:?}, 排除 {:?}",
                    config.enabled_categories, config.excluded_nouns
                );
            } else if !config.enabled_categories.is_empty() {
                println!("🔍 Noun 过滤: 启用 {:?}", config.enabled_categories);
            } else if !config.excluded_nouns.is_empty() {
                println!("🔍 Noun 过滤: 排除 {:?}", config.excluded_nouns);
            }
        }

        Self {
            cate_nouns,
            loop_owner_nouns,
            prim_nouns,
            all_nouns,
        }
    }

    /// 根据 Noun 名称判断其类别
    pub fn get_category(&self, noun: &str) -> Option<NounCategory> {
        if self.cate_nouns.contains(&noun) {
            Some(NounCategory::Cate)
        } else if self.loop_owner_nouns.contains(&noun) {
            Some(NounCategory::LoopOwner)
        } else if self.prim_nouns.contains(&noun) {
            Some(NounCategory::Prim)
        } else {
            None
        }
    }

    /// 获取所有 Noun 的总数
    pub fn total_count(&self) -> usize {
        self.all_nouns.len()
    }

    /// 获取指定类别的 Noun 列表
    pub fn get_nouns_by_category(&self, category: NounCategory) -> &[&'static str] {
        match category {
            NounCategory::Cate => &self.cate_nouns,
            NounCategory::LoopOwner => &self.loop_owner_nouns,
            NounCategory::Prim => &self.prim_nouns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_nouns() {
        let collection = FullNounCollection::collect(None);

        // 所有类别中的 noun 都应该出现在 all_nouns 中
        for &noun in collection
            .cate_nouns
            .iter()
            .chain(collection.loop_owner_nouns.iter())
            .chain(collection.prim_nouns.iter())
        {
            assert!(collection.all_nouns.contains(noun));
        }

        // all_nouns 去重后的数量不大于各类别总和
        let total_in_lists = collection.cate_nouns.len()
            + collection.loop_owner_nouns.len()
            + collection.prim_nouns.len();
        assert!(collection.all_nouns.len() <= total_in_lists);
    }

    #[test]
    fn test_get_category() {
        let collection = FullNounCollection::collect(None);

        // 测试已知的noun
        if let Some(&first_cate) = collection.cate_nouns.first() {
            assert_eq!(
                collection.get_category(first_cate),
                Some(NounCategory::Cate)
            );
        }

        // 测试不存在的noun
        assert_eq!(collection.get_category("NONEXISTENT"), None);
    }

    #[test]
    fn test_extra_nouns() {
        let extras = vec!["CUSTOM1", "CUSTOM2"];
        let collection = FullNounCollection::collect(Some(&extras));

        assert!(collection.all_nouns.contains("CUSTOM1"));
        assert!(collection.all_nouns.contains("CUSTOM2"));
    }
}
