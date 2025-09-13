// PDMS 时间数据缓存演示程序

use aios_database::fast_model::aabb_cache::{AabbCache, PdmsTimeExtractor, RefnoTimeData, SesnoTimeMapping};
use aios_core::types::RefU64;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🕐 PDMS 时间数据缓存演示");
    println!("{}", "=".repeat(60));
    
    // 创建缓存
    let cache_path = "assets/pdms_time_cache_demo.redb";
    let cache = AabbCache::open_with_path(cache_path)?;
    println!("✅ 缓存已创建: {}", cache_path);
    
    // 演示数据库 1112 的时间数据
    demo_database_1112(&cache).await?;
    
    // 演示某个 refno 的历史记录
    demo_refno_history(&cache).await?;
    
    // 演示时间范围查询
    demo_time_range_query(&cache).await?;
    
    println!("🎉 演示完成！");
    println!("💡 提示: 缓存文件保存在 {}", cache_path);
    
    Ok(())
}

async fn demo_database_1112(cache: &AabbCache) -> anyhow::Result<()> {
    println!("\n📊 数据库 1112 演示");
    println!("{}", "-".repeat(40));

    // 首先检查缓存中是否有真实的参考号
    let existing_refnos = cache.get_all_refnos().unwrap_or_default();

    let demo_refnos = if existing_refnos.is_empty() {
        println!("⚠️  缓存中没有找到现有的参考号，使用演示数据");
        // 如果没有真实数据，创建一些演示数据并存储到缓存中
        let demo_data = vec![
            (RefU64(1112_00001), "PIPE", "主管道"),
            (RefU64(1112_00002), "ELBO", "弯头"),
            (RefU64(1112_00003), "TEE", "三通"),
            (RefU64(1112_00004), "VALVE", "阀门"),
            (RefU64(1112_00005), "FLANGE", "法兰"),
        ];

        // 为演示数据创建 AABB 并存储到缓存
        for (i, (refno, element_type, _desc)) in demo_data.iter().enumerate() {
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

        demo_data
    } else {
        println!("✅ 找到 {} 个现有的参考号", existing_refnos.len());
        // 使用真实的参考号，取前5个
        existing_refnos.into_iter().take(5)
            .map(|refno| (refno, "UNKNOWN", "真实数据"))
            .collect()
    };
    
    let base_time = 1640995200; // 2022-01-01 00:00:00 UTC
    
    // 为每个 refno 创建多个版本的时间记录
    for (i, (refno, element_type, desc)) in demo_refnos.iter().enumerate() {
        let sessions = vec![100, 150, 200];
        
        for (j, session) in sessions.iter().enumerate() {
            let timestamp = base_time + (i * 600) as u64 + (j * 300) as u64;
            
            // 存储 sesno 时间映射
            let mapping = SesnoTimeMapping {
                dbnum: 1112,
                sesno: *session,
                timestamp,
                description: Some(format!("Session {} for {}", session, element_type)),
            };
            cache.put_sesno_time_mapping(&mapping)?;
            
            // 存储 refno 时间数据
            let time_data = RefnoTimeData {
                refno_value: refno.0,
                session: *session,
                dbnum: 1112,
                created_at: timestamp,
                updated_at: timestamp,
                sesno_timestamp: timestamp,
                author: Some("pdms_engineer".to_string()),
                description: Some(format!("{} {} - version {}", element_type, desc, j + 1)),
            };
            cache.put_refno_time_data(&time_data)?;
        }
    }
    
    println!("✅ 已存储 {} 个元素的时间数据", demo_refnos.len());
    
    // 显示最新的几个 refno 信息
    println!("\n🔍 最新的 refno 时间信息:");
    for (refno, element_type, desc) in demo_refnos.iter().take(3) {
        if let Some(time_data) = cache.get_refno_time_data(*refno, 200) {
            let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(time_data.sesno_timestamp);
            println!("  📌 RefNo: {}, 类型: {}, 描述: {}", refno.to_string(), element_type, desc);
            println!("     🕒 最新时间: {:?}", datetime);
            println!("     👤 作者: {:?}", time_data.author);
            println!("     📝 说明: {:?}", time_data.description);
            println!();
        }
    }
    
    Ok(())
}

async fn demo_refno_history(cache: &AabbCache) -> anyhow::Result<()> {
    println!("📜 RefNo 历史记录演示");
    println!("{}", "-".repeat(40));

    // 获取一个真实的参考号进行演示
    let all_refnos = cache.get_all_refnos().unwrap_or_default();
    let target_refno = if let Some(refno) = all_refnos.first() {
        *refno
    } else {
        RefU64(1112_00001) // 如果没有真实数据，使用演示数据
    };

    println!("🎯 查询 RefNo {} 的历史记录:", target_refno.to_string());
    
    let history = cache.get_refno_time_history(target_refno);
    
    if history.is_empty() {
        println!("  ❌ 未找到历史记录");
        return Ok(());
    }
    
    for (i, record) in history.iter().enumerate() {
        let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(record.sesno_timestamp);
        println!("  {}. Session: {}, 时间: {:?}", 
            i + 1, record.session, datetime);
        println!("     📝 描述: {:?}", record.description);
        println!("     👤 作者: {:?}", record.author);
        
        // 显示与上一版本的时间差
        if i > 0 {
            let prev_time = history[i-1].sesno_timestamp;
            let time_diff = record.sesno_timestamp - prev_time;
            println!("     ⏱️  距上次修改: {} 秒", time_diff);
        }
        println!();
    }
    
    println!("📊 总共找到 {} 个历史版本", history.len());
    
    Ok(())
}

async fn demo_time_range_query(cache: &AabbCache) -> anyhow::Result<()> {
    println!("⏰ 时间范围查询演示");
    println!("{}", "-".repeat(40));
    
    let base_time = 1640995200;
    let start_time = base_time;
    let end_time = base_time + 1800; // 30分钟内
    
    let start_datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(start_time);
    let end_datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(end_time);
    
    println!("🔍 查询时间范围: {:?} 到 {:?}", start_datetime, end_datetime);
    
    let refnos_in_range = cache.query_refnos_by_time_range(start_time, end_time);
    
    if refnos_in_range.is_empty() {
        println!("  ❌ 指定时间范围内未找到 refno");
        return Ok(());
    }
    
    println!("📊 找到 {} 个 refno:", refnos_in_range.len());
    
    for refno in &refnos_in_range {
        println!("  🎯 RefNo: {}", refno.0);
        
        // 获取该 refno 在时间范围内的所有记录
        let history = cache.get_refno_time_history(*refno);
        let records_in_range: Vec<_> = history.iter()
            .filter(|record| record.sesno_timestamp >= start_time && record.sesno_timestamp <= end_time)
            .collect();
            
        for record in records_in_range {
            let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(record.sesno_timestamp);
            println!("     📅 Session {}: {:?}", record.session, datetime);
        }
        println!();
    }
    
    Ok(())
}
