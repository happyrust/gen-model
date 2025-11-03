use aios_core::eval_str_to_f64;
use aios_database::expression_fix::{ExpressionFixer, eval_pdms_expression};

fn main() {
    println!("🔧 表达式修复器演示程序");
    println!("{}", "=".repeat(50));

    // 创建测试上下文
    let context = ExpressionFixer::create_test_context();

    // 原始问题表达式
    let problematic_expr = "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )";

    println!("\n📋 原始问题:");
    println!("表达式: {}", problematic_expr);

    // 尝试使用原始方法解析（会失败）
    println!("\n❌ 使用原始解析器:");
    match eval_str_to_f64(problematic_expr, &context, "DIST") {
        Ok(result) => println!("意外成功: {}", result),
        Err(e) => println!("预期失败: {}", e),
    }

    // 使用修复器解析
    println!("\n✅ 使用表达式修复器:");
    match ExpressionFixer::eval_expression_with_attrib_support(problematic_expr, &context, "DIST") {
        Ok(result) => {
            println!("修复成功! 结果: {}", result);
            println!("解释: MIN(HEIG=100.0, PARA3=40.0) = 40.0");
        }
        Err(e) => println!("修复失败: {}", e),
    }

    // 展示预处理过程
    println!("\n🔄 预处理过程:");
    let processed = ExpressionFixer::preprocess_attrib_expression(problematic_expr);
    println!("原始: {}", problematic_expr);
    println!("处理后: {}", processed);

    // 测试更多表达式
    println!("\n📊 更多测试用例:");
    let test_cases = vec![
        "ATTRIB HEIG",
        "ATTRIB PARA[0]",
        "ATTRIB PARA[3 ]",
        "MAX(ATTRIB WIDT, ATTRIB LENG)",
        "ATTRIB HEIG + ATTRIB PARA[1] * 2",
        "(ATTRIB WIDT - ATTRIB HEIG) / ATTRIB PARA[2]",
    ];

    for expr in test_cases {
        println!("\n测试: {}", expr);
        match eval_pdms_expression(expr, &context) {
            Ok(result) => println!("  ✅ 结果: {}", result),
            Err(e) => println!("  ❌ 错误: {}", e),
        }
    }

    println!("\n🎉 演示完成!");
}
