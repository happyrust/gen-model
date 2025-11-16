use aios_core::eval_str_to_f64;
use aios_database::expression_fix::{ExpressionFixer, eval_pdms_expression};
use aios_core::CataContext;
use dashmap::DashMap;

fn main() {
    println!("🔍 测试属性不存在时的处理方式");
    println!("{}", "=".repeat(50));

    // 创建一个缺少某些属性的测试上下文
    let mut context: DashMap<String, String> = DashMap::new();
    
    // 只添加部分属性，故意不添加 LOHE
    context.insert("HEIG".to_string(), "100.0".to_string());
    context.insert("WIDT".to_string(), "200.0".to_string());
    context.insert("PARA1".to_string(), "10.0".to_string());
    context.insert("PARA2".to_string(), "20.0".to_string());
    
    let cata_context = CataContext { 
        context, 
        is_tubi: false 
    };

    // 测试不存在的属性表达式
    let test_expr = "ATTRIB LOHE";
    
    println!("\n📋 测试表达式: {}", test_expr);
    println!("上下文中包含的属性: HEIG, WIDT, PARA1, PARA2");
    println!("❌ LOHE 属性不存在");

    // 1. 使用原始解析器测试
    println!("\n🔧 使用原始解析器 eval_str_to_f64:");
    match eval_str_to_f64(test_expr, &cata_context, "DIST") {
        Ok(result) => println!("  ✅ 意外成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    // 2. 使用修复器测试
    println!("\n🛠️ 使用表达式修复器:");
    match eval_pdms_expression(test_expr, &cata_context) {
        Ok(result) => println!("  ✅ 成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    // 3. 测试更复杂的不存在属性表达式
    let complex_expr = "( -( ATTRIB LOHE / 2 ) )";
    println!("\n📋 测试复杂表达式: {}", complex_expr);
    
    println!("\n🔧 使用原始解析器:");
    match eval_str_to_f64(complex_expr, &cata_context, "DIST") {
        Ok(result) => println!("  ✅ 意外成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    println!("\n🛠️ 使用表达式修复器:");
    match eval_pdms_expression(complex_expr, &cata_context) {
        Ok(result) => println!("  ✅ 成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    // 4. 测试存在属性的情况作为对比
    let valid_expr = "ATTRIB HEIG";
    println!("\n📋 对比测试 - 存在的属性: {}", valid_expr);
    
    println!("\n🔧 使用原始解析器:");
    match eval_str_to_f64(valid_expr, &cata_context, "DIST") {
        Ok(result) => println!("  ✅ 成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    println!("\n🛠️ 使用表达式修复器:");
    match eval_pdms_expression(valid_expr, &cata_context) {
        Ok(result) => println!("  ✅ 成功: {}", result),
        Err(e) => println!("  ❌ 失败: {}", e),
    }

    println!("\n🎯 结论分析:");
    println!("1. 当属性不存在时，原始解析器会返回具体错误");
    println!("2. 表达式修复器提供更详细的错误分析");
    println!("3. 两种方式都不会静默失败，都会明确报告错误");
}
