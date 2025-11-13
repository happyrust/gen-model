use std::sync::Arc;
use aios_core::{RefnoEnum, rs_surreal::query_tubi_insts_by_brans};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化数据库连接
    aios_core::init_surreal_from_config().await?;
    
    println!("🔍 调试 TUBI 数据查询");
    println!("====================");
    
    // 测试一些已知的 refno
    let test_refnos = vec![
        RefnoEnum::from_str("21491_18946")?,
        RefnoEnum::from_str("21491_18947")?,
        RefnoEnum::from_str("21491_18948")?,
    ];
    
    println!("📋 测试 refnos: {:?}", test_refnos);
    
    // 查询 tubi 数据
    let tubi_insts = query_tubi_insts_by_brans(&test_refnos).await?;
    println!("📊 查询结果: 找到 {} 个 tubi 实例", tubi_insts.len());
    
    if !tubi_insts.is_empty() {
        println!("✅ TUBI 数据示例:");
        for (i, tubi) in tubi_insts.iter().take(3).enumerate() {
            println!("  [{}] refno: {}, geo_hash: {}", i, tubi.refno, tubi.geo_hash);
        }
    } else {
        println!("❌ 未找到任何 TUBI 数据");
        
        // 尝试查询所有可能的 tubi 数据
        println!("\n🔍 尝试查询所有 tubi 数据...");
        let all_refnos = vec![
            RefnoEnum::from_str("21491_18946")?,
            RefnoEnum::from_str("21491_18957")?,
            RefnoEnum::from_str("21491_18959")?,
            RefnoEnum::from_str("21491_18962")?,
        ];
        
        let all_tubi = query_tubi_insts_by_brans(&all_refnos).await?;
        println!("📊 扩展查询结果: 找到 {} 个 tubi 实例", all_tubi.len());
    }
    
    // 检查数据库连接状态
    println!("\n🔧 数据库连接状态:");
    if let Ok(db) = aios_core::SUL_DB.get() {
        println!("✅ 数据库连接正常");
    } else {
        println!("❌ 数据库连接失败");
    }
    
    Ok(())
}
