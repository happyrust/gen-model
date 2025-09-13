// 查询真实参考号的工具程序

use aios_database::fast_model::aabb_cache::{AabbCache, RefnoTimeData, SesnoTimeMapping};
use aios_core::types::RefU64;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🔍 真实参考号查询工具");
    println!("{}", "=".repeat(60));
    
    // 尝试打开现有的缓存文件
    let cache_paths = vec![
        "assets/aabb_cache.redb",
        "assets/pdms_time_cache_demo.redb",
        "cache.redb",
        "aabb_cache.redb",
    ];
    
    let mut cache_found = false;
    let mut cache = None;
    
    for path in &cache_paths {
        if std::path::Path::new(path).exists() {
            match AabbCache::open_with_path(path) {
                Ok(c) => {
                    println!("✅ 找到缓存文件: {}", path);
                    cache = Some(c);
                    cache_found = true;
                    break;
                }
                Err(e) => {
                    println!("⚠️  无法打开缓存文件 {}: {}", path, e);
                }
            }
        }
    }
    
    if !cache_found {
        println!("❌ 未找到任何缓存文件，创建新的演示缓存");
        cache = Some(create_demo_cache().await?);
    }
    
    let cache = cache.unwrap();
    
    // 显示缓存统计信息
    show_cache_stats(&cache).await?;
    
    // 查询所有参考号
    query_all_refnos(&cache).await?;
    
    // 按数据库查询参考号
    query_refnos_by_database(&cache).await?;
    
    // 查询最新的参考号
    query_latest_refnos(&cache).await?;
    
    // 如果有时间数据，显示时间信息
    query_time_data(&cache).await?;
    
    println!("🎉 查询完成！");
    
    Ok(())
}

async fn create_demo_cache() -> anyhow::Result<AabbCache> {
    let cache = AabbCache::open_with_path("assets/real_refnos_demo.redb")?;
    
    // 创建一些真实格式的演示数据
    let demo_refnos = vec![
        (RefU64(1112_86525), "PIPE"),
        (RefU64(1112_86526), "ELBO"),
        (RefU64(1112_86527), "TEE"),
        (RefU64(1516_12345), "VALVE"),
        (RefU64(1516_12346), "FLANGE"),
        (RefU64(2000_00001), "PUMP"),
    ];
    
    for (i, (refno, element_type)) in demo_refnos.iter().enumerate() {
        use parry3d::bounding_volume::Aabb;
        use glam::Vec3;
        use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
        
        let aabb = Aabb::new(
            Vec3::new(i as f32 * 10.0, 0.0, 0.0).into(),
            Vec3::new(i as f32 * 10.0 + 5.0, 5.0, 5.0).into()
        );
        let bbox = RStarBoundingBox::new(aabb, (*refno).into(), element_type.to_string());
        cache.put_ref_bbox(&bbox)?;
    }
    
    println!("✅ 创建了 {} 个演示参考号", demo_refnos.len());
    
    Ok(cache)
}

async fn show_cache_stats(cache: &AabbCache) -> anyhow::Result<()> {
    println!("\n📈 缓存统计信息");
    println!("{}", "-".repeat(40));
    
    let stats = cache.get_cache_stats()?;
    println!("📦 主表记录数: {}", stats.ref_bbox_count);
    println!("🕒 版本化记录数: {}", stats.versioned_count);
    println!("⏰ 时间数据记录数: {}", stats.time_data_count);
    println!("🔗 Sesno 映射记录数: {}", stats.sesno_mapping_count);
    
    Ok(())
}

async fn query_all_refnos(cache: &AabbCache) -> anyhow::Result<()> {
    println!("\n🎯 所有参考号");
    println!("{}", "-".repeat(40));
    
    let all_refnos = cache.get_all_refnos()?;
    
    if all_refnos.is_empty() {
        println!("❌ 缓存中没有找到任何参考号");
        return Ok(());
    }
    
    println!("找到 {} 个参考号:", all_refnos.len());
    for (i, refno) in all_refnos.iter().enumerate() {
        println!("  {}. RefNo: {}", i + 1, refno.to_string());
        
        // 尝试获取对应的 AABB 数据
        if let Some(bbox) = cache.get_ref_bbox(*refno) {
            println!("     📦 类型: {}", bbox.noun);
            println!("     📏 边界: [{:.2}, {:.2}, {:.2}] - [{:.2}, {:.2}, {:.2}]",
                bbox.aabb.mins.x, bbox.aabb.mins.y, bbox.aabb.mins.z,
                bbox.aabb.maxs.x, bbox.aabb.maxs.y, bbox.aabb.maxs.z);
        }
        
        if i >= 9 { // 只显示前10个
            println!("  ... 还有 {} 个参考号", all_refnos.len() - 10);
            break;
        }
    }
    
    Ok(())
}

async fn query_refnos_by_database(cache: &AabbCache) -> anyhow::Result<()> {
    println!("\n🏢 按数据库查询参考号");
    println!("{}", "-".repeat(40));
    
    let databases = vec![1112, 1516, 2000, 24383];
    
    for dbnum in databases {
        let refnos = cache.get_refnos_by_dbnum(dbnum)?;
        
        if !refnos.is_empty() {
            println!("数据库 {} 的参考号 ({} 个):", dbnum, refnos.len());
            for refno in refnos.iter().take(5) {
                println!("  🎯 RefNo: {}", refno.to_string());
            }
            if refnos.len() > 5 {
                println!("  ... 还有 {} 个", refnos.len() - 5);
            }
            println!();
        }
    }
    
    Ok(())
}

async fn query_latest_refnos(cache: &AabbCache) -> anyhow::Result<()> {
    println!("🔝 最新的参考号");
    println!("{}", "-".repeat(40));
    
    let latest_refnos = cache.get_latest_refnos(5)?;
    
    if latest_refnos.is_empty() {
        println!("❌ 没有找到参考号");
        return Ok(());
    }
    
    println!("最新的 {} 个参考号:", latest_refnos.len());
    for (i, refno) in latest_refnos.iter().enumerate() {
        println!("  {}. RefNo: {}", i + 1, refno.to_string());
        
        // 显示对应的元素类型
        if let Some(bbox) = cache.get_ref_bbox(*refno) {
            println!("     📦 类型: {}", bbox.noun);
        }
    }
    
    Ok(())
}

async fn query_time_data(cache: &AabbCache) -> anyhow::Result<()> {
    println!("\n⏰ 时间数据查询");
    println!("{}", "-".repeat(40));
    
    let stats = cache.get_cache_stats()?;
    
    if stats.time_data_count == 0 {
        println!("❌ 缓存中没有时间数据");
        return Ok(());
    }
    
    println!("找到 {} 条时间数据记录", stats.time_data_count);
    
    // 尝试查询一些参考号的时间历史
    let all_refnos = cache.get_all_refnos()?;
    
    for refno in all_refnos.iter().take(3) {
        let history = cache.get_refno_time_history(*refno);
        
        if !history.is_empty() {
            println!("\n📜 RefNo {} 的时间历史:", refno.to_string());
            for record in history {
                let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(record.sesno_timestamp);
                println!("  🕒 Session {}: {:?}", record.session, datetime);
                if let Some(desc) = record.description {
                    println!("     📝 {}", desc);
                }
            }
        }
    }
    
    Ok(())
}
