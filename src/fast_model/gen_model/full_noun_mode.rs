use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use aios_core::options::DbOption;
use aios_core::pdms_types::{
    GNERAL_LOOP_OWNER_NOUN_NAMES, GNERAL_PRIM_NOUN_NAMES, USE_CATE_NOUN_NAMES,
};
use aios_core::pe::SPdmsElement;
use dashmap::DashMap;
use glam::Vec3;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::fast_model::{cata_model, query_provider};
use super::cate_processor::process_cate_refno_page;
use super::categorized_refnos::CategorizedRefnos;
use super::config::FullNounConfig;
use super::context::NounProcessContext;
use super::errors::{FullNounError, Result};
use super::loop_processor::process_loop_refno_page;
use super::prim_processor::process_prim_refno_page;
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

    println!("🚀 启动 Full Noun 模式（入口 Noun 深度查询版本）");
    config.print_info();

    // 🔥 读取 manual_db_nums 配置（用于过滤数据库）
    let dbnums: Vec<u32> = db_option.manual_db_nums.clone().unwrap_or_default();
    if !dbnums.is_empty() {
        println!("🗂️  数据库过滤: 仅查询 dbnum = {:?}", dbnums);
    } else {
        println!("🗂️  数据库过滤: 查询所有数据库（未设置 manual_db_nums）");
    }

    let has_explicit_entry_nouns = config
        .enabled_categories
        .iter()
        .any(|cat| {
            let lower = cat.to_lowercase();
            !matches!(lower.as_str(), "cate" | "loop" | "prim")
        });

    let entry_nouns: Vec<String> = if has_explicit_entry_nouns {
        config
            .enabled_categories
            .iter()
            .filter(|cat| {
                let lower = cat.to_lowercase();
                !matches!(lower.as_str(), "cate" | "loop" | "prim")
            })
            .cloned()
            .collect()
    } else {
        let mut set = HashSet::new();
        for &noun in GNERAL_LOOP_OWNER_NOUN_NAMES
            .iter()
            .chain(GNERAL_PRIM_NOUN_NAMES.iter())
            .chain(USE_CATE_NOUN_NAMES.iter())
        {
            set.insert(noun.to_string());
        }
        set.into_iter().collect()
    };

    if entry_nouns.is_empty() {
        println!("[gen_full_noun_geos] 入口 Noun 列表为空，直接返回");
        return Ok(CategorizedRefnos::new());
    }

    println!("📌 入口 Noun 列表: {:?}", entry_nouns);

    let mut all_roots: HashSet<RefnoEnum> = HashSet::new();
    let mut loop_refnos: HashSet<RefnoEnum> = HashSet::new();
    let mut prim_refnos: HashSet<RefnoEnum> = HashSet::new();
    let mut cate_refnos: HashSet<RefnoEnum> = HashSet::new();
    // BRAN/HANG 根节点单独收集，避免混入普通 CATE 流程
    let mut bran_hanger_roots: HashSet<RefnoEnum> = HashSet::new();

    for entry in &entry_nouns {
        let noun_upper = entry.to_uppercase();
        let noun_str = noun_upper.as_str();
        let nouns_slice = [noun_str];

        // 🔥 使用 manual_db_nums 过滤（如果配置了）
        let mut refnos = aios_core::mdb::query_type_refnos_by_dbnums(&nouns_slice, &dbnums)
            .await
            .map_err(|e| {
                FullNounError::DatabaseError(format!(
                    "query_type_refnos_by_dbnums({}, dbnums={:?}) failed: {}",
                    noun_str, dbnums, e
                ))
            })?;

        if let Some(limit) = config.debug_limit_per_noun {
            if refnos.len() > limit {
                println!(
                    "[gen_full_noun_geos] 入口 noun {}: 调试模式限制实例数量从 {} 到 {}",
                    noun_str,
                    refnos.len(),
                    limit
                );
                refnos.truncate(limit);
            }
        }

        if refnos.is_empty() {
            println!(
                "[gen_full_noun_geos] 入口 noun {}: 未找到实例，跳过",
                noun_str
            );
            continue;
        }

        all_roots.extend(refnos.iter().copied());

        // 🔥 BRAN/HANG 作为特殊类型单独处理（用于 Tubing 生成）
        if noun_str == "BRAN" || noun_str == "HANG" {
            bran_hanger_roots.extend(refnos.iter().copied());
            println!(
                "[gen_full_noun_geos] {} 作为 BRAN/HANG 特殊处理（Tubing）",
                noun_str
            );
        }

        if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun_str) {
            loop_refnos.extend(refnos.iter().copied());
        }
        if GNERAL_PRIM_NOUN_NAMES.contains(&noun_str) {
            prim_refnos.extend(refnos.iter().copied());
        }
        if USE_CATE_NOUN_NAMES.contains(&noun_str) {
            cate_refnos.extend(refnos.iter().copied());
        }
    }

    if all_roots.is_empty() {
        println!(
            "[gen_full_noun_geos] 入口 Noun {:?}: 未找到任何实例，直接返回",
            entry_nouns
        );
        return Ok(CategorizedRefnos::new());
    }

    println!(
        "[gen_full_noun_geos] 入口 Noun {:?}: 根节点总数 {}",
        entry_nouns,
        all_roots.len()
    );

    let roots_vec: Vec<RefnoEnum> = all_roots.iter().copied().collect();

    let loop_descendants = aios_core::collect_descendant_filter_ids(
        &roots_vec,
        &GNERAL_LOOP_OWNER_NOUN_NAMES,
        None,
    )
    .await
    .map_err(|e| {
        FullNounError::DatabaseError(format!(
            "collect_descendant_filter_ids(loop) failed: {}",
            e
        ))
    })?;
    loop_refnos.extend(loop_descendants);

    let prim_descendants = aios_core::collect_descendant_filter_ids(
        &roots_vec,
        &GNERAL_PRIM_NOUN_NAMES,
        None,
    )
    .await
    .map_err(|e| {
        FullNounError::DatabaseError(format!(
            "collect_descendant_filter_ids(prim) failed: {}",
            e
        ))
    })?;
    prim_refnos.extend(prim_descendants);

    let cate_descendants = aios_core::collect_descendant_filter_ids(
        &roots_vec,
        &USE_CATE_NOUN_NAMES,
        None,
    )
    .await
    .map_err(|e| {
        FullNounError::DatabaseError(format!(
            "collect_descendant_filter_ids(cate) failed: {}",
            e
        ))
    })?;
    cate_refnos.extend(cate_descendants);

    println!(
        " 深度查询结果：Loop={}，Prim={}，Cate={}",
        loop_refnos.len(),
        prim_refnos.len(),
        cate_refnos.len()
    );

    let loop_sjus_map_arc = Arc::new(DashMap::new());
    validate_sjus_map(&loop_sjus_map_arc, config)?;

    let ctx = NounProcessContext::new(
        db_option.clone(),
        config.batch_size.get(),
        config.concurrency.get(),
    );

    let mut categorized = CategorizedRefnos::new();

    println!("📍 [1/3] 处理 LOOP Refno 集合...");
    let loop_start = Instant::now();
    let loop_vec: Vec<RefnoEnum> = loop_refnos.iter().copied().collect();
    {
        let ranges = ctx.bounded_chunks(loop_vec.len());
        for (page_index, (start, end)) in ranges.into_iter().enumerate() {
            let slice = &loop_vec[start..end];
            println!(
                "[gen_full_noun_geos] loop: 处理第 {} 页 ({} ~ {})",
                page_index + 1,
                start + 1,
                end
            );
            process_loop_refno_page(&ctx, loop_sjus_map_arc.clone(), sender.clone(), slice)
                .await
                .map_err(|e| {
                    FullNounError::GeometryGenerationFailed(
                        "loop".to_string(),
                        e.to_string(),
                    )
                })?;
        }
    }
    let loop_duration = loop_start.elapsed();
    println!("⏱️  LOOP processing took {} ms", loop_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        loop_count = loop_vec.len(),
        duration_ms = loop_duration.as_millis() as u64,
        "LOOP Noun processing completed (entry-based)"
    );

    println!("📍 [2/3] 处理 PRIM Refno 集合...");
    let prim_start = Instant::now();
    let prim_vec: Vec<RefnoEnum> = prim_refnos.iter().copied().collect();
    {
        let ranges = ctx.bounded_chunks(prim_vec.len());
        for (page_index, (start, end)) in ranges.into_iter().enumerate() {
            let slice = &prim_vec[start..end];
            println!(
                "[gen_full_noun_geos] prim: 处理第 {} 页 ({} ~ {})",
                page_index + 1,
                start + 1,
                end
            );
            process_prim_refno_page(&ctx, sender.clone(), slice)
                .await
                .map_err(|e| {
                    FullNounError::GeometryGenerationFailed(
                        "prim".to_string(),
                        e.to_string(),
                    )
                })?;
        }
    }
    let prim_duration = prim_start.elapsed();
    println!("⏱️  PRIM processing took {} ms", prim_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        prim_count = prim_vec.len(),
        duration_ms = prim_duration.as_millis() as u64,
        "PRIM Noun processing completed (entry-based)"
    );

    println!("📍 [3/3] 处理 CATE Refno 集合 (不含 BRAN/HANG Tubing)...");
    let cate_start = Instant::now();
    let cate_vec: Vec<RefnoEnum> = cate_refnos.iter().copied().collect();
    {
        let ranges = ctx.bounded_chunks(cate_vec.len());
        for (page_index, (start, end)) in ranges.into_iter().enumerate() {
            let slice = &cate_vec[start..end];
            println!(
                "[gen_full_noun_geos] cate: 处理第 {} 页 ({} ~ {})",
                page_index + 1,
                start + 1,
                end
            );
            process_cate_refno_page(&ctx, loop_sjus_map_arc.clone(), sender.clone(), slice)
                .await
                .map_err(|e| {
                    FullNounError::GeometryGenerationFailed(
                        "cate".to_string(),
                        e.to_string(),
                    )
                })?;
        }
    }
    let cate_duration = cate_start.elapsed();
    println!("⏱️  CATE processing took {} ms", cate_duration.as_millis());

    #[cfg(feature = "profile")]
    info!(
        cate_count = cate_vec.len(),
        duration_ms = cate_duration.as_millis() as u64,
        "CATE Noun processing completed (entry-based)"
    );

    // 📍 [4/3] 专门处理 BRAN/HANG Tubing（需要 branch_map 支持）
    let bran_start = Instant::now();
    let bran_roots: Vec<RefnoEnum> = bran_hanger_roots.iter().copied().collect();
    if !bran_roots.is_empty() {
        println!(
            "📍 [4/3] 处理 BRAN/HANG 根节点集合 (count={})...",
            bran_roots.len()
        );

        // 参考旧版实现：每批处理少量 BRAN/HANG，避免单次 SQL 过大
        let batch_size = 4usize;
        let total_bran = bran_roots.len();
        let chunks: Vec<&[RefnoEnum]> = bran_roots.chunks(batch_size).collect();
        let total_batches = chunks.len();

        for (batch_idx, chunk) in chunks.into_iter().enumerate() {
            let batch_num = batch_idx + 1;
            println!(
                "[gen_full_noun_geos] 处理 BRAN/HANG 批次 {}/{} ({}~{} 个)",
                batch_num,
                total_batches,
                batch_idx * batch_size + 1,
                (batch_idx * batch_size + chunk.len()).min(total_bran)
            );

            // 1. 查询当前批次的子元素
            let branch_refnos_map: DashMap<RefnoEnum, Vec<SPdmsElement>> = DashMap::new();

            for &refno in chunk {
                match aios_core::collect_children_elements(refno, &[]).await {
                    Ok(children) => {
                        if !children.is_empty() {
                            branch_refnos_map.insert(refno, children);
                        }
                    }
                    Err(e) => {
                        println!(
                            "[gen_full_noun_geos] 查询 BRAN/HANG 子元素失败 (refno={}): {}",
                            refno, e
                        );
                    }
                }
            }

            if branch_refnos_map.is_empty() {
                println!(
                    "[gen_full_noun_geos] 批次 {}: 未找到任何 BRAN/HANG 子元素，跳过 Tubing 生成",
                    batch_num
                );
                continue;
            }

            // 2. 查询当前批次的元件库分组（按 cata_hash）
            let target_bran_reuse_cata_map = match aios_core::query_group_by_cata_hash(chunk).await
            {
                Ok(map) => map,
                Err(e) => {
                    println!(
                        "[gen_full_noun_geos] 批次 {}: 查询 BRAN/HANG 元件库分组失败: {}",
                        batch_num, e
                    );
                    DashMap::new()
                }
            };

            // 3. 调用 cata_model::gen_cata_geos 生成 Tubing 几何体
            if let Err(e) = cata_model::gen_cata_geos(
                db_option.clone(),
                Arc::new(target_bran_reuse_cata_map),
                Arc::new(branch_refnos_map),
                loop_sjus_map_arc.clone(),
                sender.clone(),
            )
            .await
            {
                println!(
                    "[gen_full_noun_geos] 批次 {}: BRAN/HANG Tubing 几何生成失败: {}",
                    batch_num, e
                );
            } else {
                println!(
                    "[gen_full_noun_geos] 批次 {}/{} BRAN/HANG Tubing 生成完成",
                    batch_num, total_batches
                );
            }
        }
    } else {
        println!("[gen_full_noun_geos] BRAN/HANG 根节点集合为空，跳过 Tubing 生成");
    }
    let bran_duration = bran_start.elapsed();
    println!(
        "⏱️  BRAN/HANG Tubing processing took {} ms",
        bran_duration.as_millis()
    );

    // 将 BRAN/HANG 根节点也归类为 Cate，便于后续 mesh 深度遍历
    let bran_vec: Vec<RefnoEnum> = bran_hanger_roots.iter().copied().collect();

    for r in &cate_vec {
        categorized.insert(*r, super::models::NounCategory::Cate);
    }
    for r in &bran_vec {
        categorized.insert(*r, super::models::NounCategory::Cate);
    }
    for r in &loop_vec {
        categorized.insert(*r, super::models::NounCategory::LoopOwner);
    }
    for r in &prim_vec {
        categorized.insert(*r, super::models::NounCategory::Prim);
    }

    let total_duration = total_start.elapsed();
    println!("✅ Full Noun 处理完成（入口 Noun 深度查询版本）");
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
        "   ├─ CATE: {} ms ({:.1}%)",
        cate_duration.as_millis(),
        cate_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
    );
    println!(
        "   └─ BRAN/HANG Tubing: {} ms ({:.1}%)",
        bran_duration.as_millis(),
        bran_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
    );

    categorized.print_statistics();

    #[cfg(feature = "profile")]
    info!(
        total_duration_ms = total_duration.as_millis() as u64,
        loop_ms = loop_duration.as_millis() as u64,
        prim_ms = prim_duration.as_millis() as u64,
        cate_ms = cate_duration.as_millis() as u64,
        "Full Noun generation completed with performance metrics (entry-based)"
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
