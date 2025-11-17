use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use aios_core::options::DbOption;
use dashmap::DashMap;
use glam::Vec3;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::cate_processor::process_cate_refno_page;
use super::categorized_refnos::CategorizedRefnos;
use super::config::FullNounConfig;
use super::context::NounProcessContext;
use super::errors::{FullNounError, Result};
use super::loop_processor::process_loop_refno_page;
use super::noun_collection::FullNounCollection;
use super::prim_processor::process_prim_refno_page;
use super::processor::NounProcessor;

// Performance profiling support
#[cfg(feature = "profile")]
use tracing::{info, instrument};

/// 验证 SJUS map 是否完整
///
/// 根据配置决定是否警告或报错
pub fn validate_sjus_map(
    sjus_map: &DashMap<RefnoEnum, (Vec3, f32)>,
    config: &FullNounConfig,
) -> Result<()> {
    if config.validate_sjus_map && sjus_map.is_empty() {
        let warning = "⚠️ SJUS map 为空，几何体生成可能产生不正确的结果";

        if config.strict_validation {
            log::error!("{}", warning);
            return Err(FullNounError::EmptySjusMap);
        } else {
            log::warn!("{}", warning);
            log::warn!("  提示：如果这是预期行为，可以在配置中禁用 validate_sjus_map");
        }
    }
    Ok(())
}

/// Full Noun 模式下生成所有几何体（优化版本）
///
/// # 主要改进
/// 1. ✅ 顺序执行：LOOP -> PRIM -> CATE（确保依赖关系正确）
/// 2. ✅ 批量并发：每个类别内部使用批量并发处理
/// 3. ✅ 内存优化：使用 CategorizedRefnos 替代三个 HashSet
/// 4. ✅ 数据验证：检查 SJUS map 完整性
/// 5. ✅ 类型安全：使用 FullNounConfig 和错误类型
///
/// # 执行顺序
/// 必须按照 LOOP -> PRIM -> CATE 顺序执行，因为 CATE 依赖 LOOP 生成的 SJUS 数据
#[cfg_attr(feature = "profile", instrument(skip(db_option, config, sender)))]
pub async fn gen_full_noun_geos_optimized(
    db_option: Arc<DbOption>,
    config: &FullNounConfig,
    sender: flume::Sender<ShapeInstancesData>,
) -> Result<CategorizedRefnos> {
    let total_start = Instant::now();

    println!("🚀 启动 Full Noun 模式（优化版本）");
    config.print_info();

    // 收集所有 Noun（应用配置过滤）
    let collection = FullNounCollection::collect_with_config(None, Some(config));
    println!(
        "📋 收集到 {} 个 Noun 类型（Cate: {}, Loop: {}, Prim: {}）",
        collection.total_count(),
        collection.cate_nouns.len(),
        collection.loop_owner_nouns.len(),
        collection.prim_nouns.len()
    );

    // 创建共享的 SJUS map（空的，需要验证）
    let loop_sjus_map_arc = Arc::new(DashMap::new());

    // 验证 SJUS map
    validate_sjus_map(&loop_sjus_map_arc, config)?;

    // 创建处理上下文
    let ctx = NounProcessContext::new(
        db_option.clone(),
        config.batch_size.get(),
        config.concurrency.get(),
    );

    // 创建三个独立的收集器（用于并发处理）
    let cate_sink = Arc::new(RwLock::new(HashSet::new()));
    let loop_sink = Arc::new(RwLock::new(HashSet::new()));
    let prim_sink = Arc::new(RwLock::new(HashSet::new()));

    // ⚡ 顺序执行：LOOP -> PRIM -> CATE（内部批量并发）
    println!("⚡ 开始顺序处理三个 Noun 类别（LOOP -> PRIM -> CATE）...");

    // 1️⃣ 先执行 LOOP 处理
    println!("📍 [1/3] 处理 LOOP Nouns...");
    let loop_start = Instant::now();
    let loop_result = {
        let processor = NounProcessor::new(ctx.clone(), "loop");
        let loop_nouns = collection.loop_owner_nouns.clone();
        let loop_sjus_map = loop_sjus_map_arc.clone();
        let loop_sender = sender.clone();

        processor
            .process_nouns(&loop_nouns, loop_sink.clone(), |refnos| {
                let ctx_clone = processor.ctx.clone();
                let sjus_map_clone = loop_sjus_map.clone();
                let sender_clone = loop_sender.clone();
                async move {
                    process_loop_refno_page(&ctx_clone, sjus_map_clone, sender_clone, &refnos)
                        .await
                        .map_err(Into::into)
                }
            })
            .await?;
        loop_sink
    };
    let loop_duration = loop_start.elapsed();
    println!("⏱️  LOOP processing took {} ms", loop_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        loop_count = collection.loop_owner_nouns.len(),
        duration_ms = loop_duration.as_millis() as u64,
        "LOOP Noun processing completed"
    );

    // 2️⃣ 再执行 PRIM 处理
    println!("📍 [2/3] 处理 PRIM Nouns...");
    let prim_start = Instant::now();
    let prim_result = {
        let processor = NounProcessor::new(ctx.clone(), "prim");
        let prim_nouns = collection.prim_nouns.clone();
        let prim_sender = sender.clone();

        processor
            .process_nouns(&prim_nouns, prim_sink.clone(), |refnos| {
                let ctx_clone = processor.ctx.clone();
                let sender_clone = prim_sender.clone();
                async move {
                    process_prim_refno_page(&ctx_clone, sender_clone, &refnos)
                        .await
                        .map_err(Into::into)
                }
            })
            .await?;
        prim_sink
    };
    let prim_duration = prim_start.elapsed();
    println!("⏱️  PRIM processing took {} ms", prim_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        prim_count = collection.prim_nouns.len(),
        duration_ms = prim_duration.as_millis() as u64,
        "PRIM Noun processing completed"
    );

    // 3️⃣ 最后执行 CATE 处理（包含 BRAN/HANG）
    println!("📍 [3/3] 处理 CATE Nouns (包含 BRAN/HANG)...");
    let cate_start = Instant::now();
    let cate_result = {
        let processor = NounProcessor::new(ctx.clone(), "cate");
        let cate_nouns = collection.cate_nouns.clone();
        let cate_sjus_map = loop_sjus_map_arc.clone();
        let cate_sender = sender;

        processor
            .process_nouns(&cate_nouns, cate_sink.clone(), |refnos| {
                let ctx_clone = processor.ctx.clone();
                let sjus_map_clone = cate_sjus_map.clone();
                let sender_clone = cate_sender.clone();
                async move {
                    process_cate_refno_page(&ctx_clone, sjus_map_clone, sender_clone, &refnos)
                        .await
                        .map_err(Into::into)
                }
            })
            .await?;
        cate_sink
    };
    let cate_duration = cate_start.elapsed();
    println!("⏱️  CATE processing took {} ms", cate_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        cate_count = collection.cate_nouns.len(),
        duration_ms = cate_duration.as_millis() as u64,
        "CATE Noun processing completed (includes BRAN/HANG)"
    );

    // 提取结果
    let loop_refnos = loop_result;
    let prim_refnos = prim_result;
    let cate_refnos = cate_result;

    // 合并到统一的分类存储（内存优化）
    let mut categorized = CategorizedRefnos::new();

    {
        let cate_set = cate_refnos.read().await;
        categorized.extend(
            cate_set
                .iter()
                .map(|r| (*r, super::models::NounCategory::Cate)),
        );
    }

    {
        let loop_set = loop_refnos.read().await;
        categorized.extend(
            loop_set
                .iter()
                .map(|r| (*r, super::models::NounCategory::LoopOwner)),
        );
    }

    {
        let prim_set = prim_refnos.read().await;
        categorized.extend(
            prim_set
                .iter()
                .map(|r| (*r, super::models::NounCategory::Prim)),
        );
    }

    let total_duration = total_start.elapsed();
    println!("✅ Full Noun 处理完成");
    println!(
        "⏱️  Total Full Noun processing: {} ms",
        total_duration.as_millis()
    );
    println!(
        "   ├─ LOOP: {} ms ({:.1}%)",
        loop_duration.as_millis(),
        loop_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
    );
    println!(
        "   ├─ PRIM: {} ms ({:.1}%)",
        prim_duration.as_millis(),
        prim_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
    );
    println!(
        "   └─ CATE: {} ms ({:.1}%)",
        cate_duration.as_millis(),
        cate_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
    );

    categorized.print_statistics();

    #[cfg(feature = "profile")]
    info!(
        total_duration_ms = total_duration.as_millis() as u64,
        loop_ms = loop_duration.as_millis() as u64,
        prim_ms = prim_duration.as_millis() as u64,
        cate_ms = cate_duration.as_millis() as u64,
        total_nouns = collection.total_count(),
        "Full Noun generation completed with performance metrics"
    );

    Ok(categorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_sjus_map_empty_warning() {
        let sjus_map = DashMap::new();
        let config = FullNounConfig::default();

        // 默认配置下，空 map 会警告但不报错
        let result = validate_sjus_map(&sjus_map, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_sjus_map_empty_strict() {
        let sjus_map = DashMap::new();
        let config = FullNounConfig::default().with_strict_validation(true);

        // 严格模式下，空 map 会报错
        let result = validate_sjus_map(&sjus_map, &config);
        assert!(result.is_err());

        if let Err(FullNounError::EmptySjusMap) = result {
            // 正确
        } else {
            panic!("Expected EmptySjusMap error");
        }
    }

    #[test]
    fn test_validate_sjus_map_with_data() {
        let sjus_map = DashMap::new();
        sjus_map.insert(RefnoEnum::RefU64(1), (Vec3::ZERO, 1.0));

        let config = FullNounConfig::default().with_strict_validation(true);

        // 有数据时不应报错
        let result = validate_sjus_map(&sjus_map, &config);
        assert!(result.is_ok());
    }
}
