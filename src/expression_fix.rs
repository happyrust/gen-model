use aios_core::{CataContext, eval_str_to_f64};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use regex::Regex;

/// 表达式修复器 - 专门处理ATTRIB关键字和相关表达式问题
pub struct ExpressionFixer;

impl ExpressionFixer {
    /// 预处理ATTRIB表达式，将ATTRIB关键字转换为可解析的变量名
    pub fn preprocess_attrib_expression(expr: &str) -> String {
        // 处理 ATTRIB PARA[数字] 格式，支持空格
        let attrib_para_regex = Regex::new(r"ATTRIB\s+PARA\s*\[\s*(\d+)\s*\]").unwrap();
        let mut processed = attrib_para_regex.replace_all(expr, "PARA$1").to_string();

        // 处理 ATTRIB 属性名 格式
        let attrib_regex = Regex::new(r"ATTRIB\s+([A-Z][A-Z0-9_]*)").unwrap();
        processed = attrib_regex.replace_all(&processed, "$1").to_string();

        // 清理多余空格和格式化
        processed = processed
            .replace("  ", " ")
            .replace("( ", "(")
            .replace(" )", ")")
            .replace(" ,", ",")
            .replace(", ", ",")
            .trim()
            .to_string();

        processed
    }

    /// 增强的表达式求值函数，支持ATTRIB关键字
    pub fn eval_expression_with_attrib_support(
        expr: &str,
        context: &CataContext,
        unit: &str,
    ) -> Result<f64> {
        // 第一步：预处理ATTRIB关键字
        let processed_expr = Self::preprocess_attrib_expression(expr);

        // 第二步：验证处理后的表达式
        if processed_expr.is_empty() {
            return Err(anyhow!("表达式为空"));
        }

        // 第三步：调用原有的表达式解析
        match eval_str_to_f64(&processed_expr, context, unit) {
            Ok(result) => Ok(result),
            Err(e) => {
                // 提供详细的错误信息
                let error_analysis = Self::analyze_expression_error(&e, expr, &processed_expr);
                Err(anyhow!(
                    "表达式解析失败:\n原始表达式: {}\n处理后表达式: {}\n错误: {}\n建议: {}",
                    expr,
                    processed_expr,
                    e,
                    error_analysis.join("; ")
                ))
            }
        }
    }

    /// 分析表达式错误并提供解决建议
    fn analyze_expression_error(
        error: &anyhow::Error,
        original_expr: &str,
        processed_expr: &str,
    ) -> Vec<String> {
        let error_msg = error.to_string().to_lowercase();
        let mut suggestions = Vec::new();

        // 检查ATTRIB相关问题
        if original_expr.contains("ATTRIB") {
            suggestions.push("ATTRIB关键字已预处理，检查属性名是否在上下文中定义".to_string());

            if original_expr.contains("PARA[") {
                suggestions.push("PARA数组索引已转换，确认索引值是否正确".to_string());
            }
        }

        // 检查函数相关问题
        if error_msg.contains("min") || error_msg.contains("max") {
            suggestions.push("MIN/MAX函数需要两个参数，检查参数数量".to_string());
        }

        // 检查语法问题
        if error_msg.contains("parse") || error_msg.contains("syntax") {
            suggestions.push("检查括号是否配对".to_string());
            suggestions.push("验证操作符和函数名拼写".to_string());
        }

        // 检查变量问题
        if error_msg.contains("variable") || error_msg.contains("undefined") {
            suggestions.push("检查所有变量是否在上下文中定义".to_string());
        }

        // 通用建议
        if suggestions.is_empty() {
            suggestions.push("检查表达式语法格式".to_string());
            suggestions.push("验证所有变量和函数名".to_string());
        }

        suggestions
    }

    /// 创建测试上下文，用于验证表达式修复
    pub fn create_test_context() -> CataContext {
        let mut context = DashMap::new();

        // 添加常用的测试属性
        context.insert("HEIG".to_string(), "100.0".to_string());
        context.insert("PARA0".to_string(), "10.0".to_string());
        context.insert("PARA1".to_string(), "20.0".to_string());
        context.insert("PARA2".to_string(), "30.0".to_string());
        context.insert("PARA3".to_string(), "40.0".to_string());
        context.insert("PARA4".to_string(), "50.0".to_string());
        context.insert("PARA5".to_string(), "60.0".to_string());

        // 添加其他常用属性
        context.insert("WIDT".to_string(), "200.0".to_string());
        context.insert("LENG".to_string(), "300.0".to_string());
        context.insert("RADI".to_string(), "25.0".to_string());

        CataContext {
            context,
            is_tubi: false,
            ..Default::default()
        }
    }
}

/// 评估PDMS表达式 - 支持ATTRIB关键字和数组索引语法
pub fn eval_pdms_expression(expr: &str, context: &CataContext) -> Result<f64> {
    ExpressionFixer::eval_expression_with_attrib_support(expr, context, "DIST")
}

/// 评估ATTRIB表达式 - 专门处理ATTRIB关键字
pub fn eval_attrib_expression(expr: &str, context: &CataContext, unit: &str) -> Result<f64> {
    ExpressionFixer::eval_expression_with_attrib_support(expr, context, unit)
}

/// 评估增强表达式 - 通用的增强表达式评估器
pub fn eval_enhanced_expression(expr: &str, context: &CataContext, unit: &str) -> Result<f64> {
    ExpressionFixer::eval_expression_with_attrib_support(expr, context, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_attrib_expression() {
        // 测试ATTRIB PARA[数字]格式
        let expr1 = "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )";
        let processed1 = ExpressionFixer::preprocess_attrib_expression(expr1);
        assert_eq!(processed1, "(MIN (HEIG,PARA3))");

        // 测试简单ATTRIB格式
        let expr2 = "ATTRIB WIDT + ATTRIB LENG";
        let processed2 = ExpressionFixer::preprocess_attrib_expression(expr2);
        assert_eq!(processed2, "WIDT + LENG");

        // 测试复杂表达式
        let expr3 = "MAX(ATTRIB PARA[0], ATTRIB PARA[1] * 2)";
        let processed3 = ExpressionFixer::preprocess_attrib_expression(expr3);
        assert_eq!(processed3, "MAX(PARA0,PARA1 * 2)");
    }

    #[test]
    fn test_eval_expression_with_attrib_support() {
        let context = ExpressionFixer::create_test_context();

        // 测试原始问题表达式
        let expr = "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )";
        let result = ExpressionFixer::eval_expression_with_attrib_support(expr, &context, "DIST");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 40.0); // MIN(100.0, 40.0) = 40.0
    }

    #[test]
    fn test_max_function() {
        let context = ExpressionFixer::create_test_context();

        let expr = "MAX(ATTRIB PARA[1], ATTRIB PARA[2])";
        let result = eval_pdms_expression(expr, &context);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 30.0); // MAX(20.0, 30.0) = 30.0
    }
}
