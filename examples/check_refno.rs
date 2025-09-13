// 检查数据库中是否存在指定参考号的工具

use aios_core::options::DbOption;
use aios_core::pdms_types::{RefnoEnum, RefU64};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::data_interface::interface::PdmsDataInterface;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🔍 检查参考号数据库存在性");
    println!("{}", "=".repeat(40));
    
    // 目标参考号
    let target_refno = "24383/92720";
    println!("🎯 目标参考号: {}", target_refno);
    
    // 创建数据库管理器
    let db_option = DbOption::default();
    let db_manager = AiosDBManager::init(&db_option).await?;
    
    println!("📊 数据库连接状态: ✅");
    
    // 转换参考号格式
    let refno_enum = RefnoEnum::from(target_refno);
    println!("🔄 转换后的参考号: {:?}", refno_enum);
    
    // 查询参考号是否存在
    println!("\n🔍 查询参考号存在性...");
    
    // 方法1: 尝试获取类型名称
    let refno_u64: RefU64 = refno_enum.clone().into();
    let type_name = db_manager.get_type_name(refno_u64).await;
    if !type_name.is_empty() {
        println!("✅ 找到类型名称: {}", type_name);
    } else {
        println!("❌ 未找到类型名称");
    }

    // 方法2: 尝试获取属性数据
    match db_manager.get_attr(refno_u64).await {
        Ok(attr_data) => {
            println!("✅ 找到属性数据:");
            println!("   属性数量: {}", attr_data.len());
        }
        Err(e) => {
            println!("❌ 未找到属性数据: {}", e);
        }
    }
    
    // 方法3: 查询几何数据
    println!("\n🔍 查询几何数据...");

    // 尝试查询不同类型的数据
    let types_to_check = vec!["PRIM", "LOOP", "BRAN", "HANGER", "SITE", "ZONE"];

    for type_name in &types_to_check {
        println!("🔍 检查类型: {}", type_name);

        // 这里可以添加具体的查询逻辑
        // 由于我们没有直接的查询方法，我们先跳过
        println!("   ⏭️  跳过（需要具体实现）");
    }
    
    // 方法4: 查询可用的参考号示例
    println!("\n📋 查询可用的参考号示例...");

    // 尝试查询一些已知存在的参考号
    let sample_refnos = vec![
        "17496/254421",
        "17496/266621",
        "1/1",
        "1/2",
        "1/3",
    ];

    println!("🔍 检查示例参考号:");
    for sample_refno in sample_refnos {
        let sample_enum = RefnoEnum::from(sample_refno);
        let sample_u64: RefU64 = sample_enum.into();
        let type_name = db_manager.get_type_name(sample_u64).await;
        if !type_name.is_empty() {
            println!("   ✅ {}: 类型 {}", sample_refno, type_name);
        } else {
            println!("   ❌ {}: 不存在", sample_refno);
        }
    }
    
    // 建议
    println!("\n💡 建议:");
    println!("1. 确认参考号 {} 在数据库中是否存在", target_refno);
    println!("2. 检查数据库连接和项目配置");
    println!("3. 使用上面显示为 ✅ 的参考号进行测试");
    println!("4. 确认几何数据是否已生成");
    
    Ok(())
}
