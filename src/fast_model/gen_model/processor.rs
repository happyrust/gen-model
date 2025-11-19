use super::context::NounProcessContext;
use crate::fast_model::query_provider::{count_noun_all_db, query_noun_page_all_db};
use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 通用 Noun 处理器
///
/// 统一了 process_cate_nouns, process_loop_nouns, process_prim_nouns 的重复逻辑
/// 消除了90%的代码冗余
pub struct NounProcessor {
    pub ctx: NounProcessContext,
    pub category_name: &'static str,
    /// 调试模式：限制每种 Noun 的实例数量（None 表示不限制）
    pub debug_limit_per_noun: Option<usize>,
}

impl NounProcessor {
    pub fn new(
        ctx: NounProcessContext,
        category_name: &'static str,
        debug_limit_per_noun: Option<usize>,
    ) -> Self {
        Self {
            ctx,
            category_name,
            debug_limit_per_noun,
        }
    }

    /// 处理一批 Nouns，使用提供的页面处理函数
    ///
    /// # Arguments
    /// * `nouns` - 要处理的 Noun 列表
    /// * `refno_sink` - 收集所有处理过的 refno
    /// * `page_processor` - 处理单页 refno 的函数
    ///
    /// # Generic Parameters
    /// * `F` - 页面处理函数类型
    /// * `Fut` - 异步Future类型
    pub async fn process_nouns<F, Fut>(
        &self,
        nouns: &[&'static str],
        refno_sink: Arc<RwLock<HashSet<RefnoEnum>>>,
        page_processor: F,
    ) -> Result<()>
    where
        F: Fn(Vec<RefnoEnum>) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        if nouns.is_empty() {
            println!(
                "[gen_full_noun_geos] {} nouns: 空列表，跳过",
                self.category_name
            );
            return Ok(());
        }

        let mut total_instances = 0usize;

        for &noun in nouns.iter() {
            // 统计当前 noun 的总数
            let mut total = count_noun_all_db(noun)
                .await
                .map_err(|e| anyhow!("统计 {} noun {} 失败: {}", self.category_name, noun, e))?
                as usize;

            if total == 0 {
                println!(
                    "[gen_full_noun_geos] {} noun {}: 无实例",
                    self.category_name, noun
                );
                continue;
            }

            // 调试限制：根据配置限制每种 noun 的实例数量
            if let Some(limit) = self.debug_limit_per_noun {
                if total > limit {
                    println!(
                        "[gen_full_noun_geos] 🔍 调试模式：限制 {} noun {} 数量从 {} 个到 {} 个",
                        self.category_name, noun, total, limit
                    );
                    total = limit;
                }
            }

            let page_size = self.ctx.batch_size.max(1);
            println!(
                "[gen_full_noun_geos] {} noun {}: 共 {} 个实例，分页大小 {}",
                self.category_name, noun, total, page_size
            );

            // 分页处理
            let mut processed = 0usize;
            while processed < total {
                // 本页最多处理 remaining 个，避免超过调试上限
                let remaining = total - processed;
                let current_page_size = page_size.min(remaining.max(1));

                // 查询当前页
                let refnos = query_noun_page_all_db(noun, processed, current_page_size)
                    .await
                    .map_err(|e| {
                        anyhow!("分页查询 {} noun {} 失败: {}", self.category_name, noun, e)
                    })?;

                if refnos.is_empty() {
                    break;
                }

                // 收集 refno 到 sink
                {
                    let mut sink = refno_sink.write().await;
                    sink.extend(refnos.iter().copied());
                }

                // 日志输出
                let page_index = processed / page_size + 1;
                println!(
                    "[gen_full_noun_geos] {} noun {}: 处理第 {} 页 ({} ~ {})",
                    self.category_name,
                    noun,
                    page_index,
                    processed + 1,
                    processed + refnos.len()
                );

                // 调用具体的页面处理函数
                let batch_len = refnos.len();
                page_processor(refnos).await?;

                processed += batch_len;
            }

            total_instances += total;
        }

        if total_instances == 0 {
            println!("[gen_full_noun_geos] {} nouns: 无实例", self.category_name);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::options::DbOption;

    #[tokio::test]
    async fn test_empty_nouns() {
        let ctx = NounProcessContext::new(Arc::new(DbOption::default()), 100, 4);
        let processor = NounProcessor::new(ctx, "test", None);
        let sink = Arc::new(RwLock::new(HashSet::new()));

        let result = processor
            .process_nouns(&[], sink.clone(), |_refnos| async { Ok(()) })
            .await;

        assert!(result.is_ok());
        assert_eq!(sink.read().await.len(), 0);
    }
}
