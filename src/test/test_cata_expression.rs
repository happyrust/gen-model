use aios_core::*;
use aios_core::tiny_expr::expr_eval::interp;
use dashmap::DashMap;
use regex::Regex;

use crate::test::test_helper::get_test_ams_db_manager;
use crate::expression_fix::{ExpressionFixer, eval_pdms_expression, eval_attrib_expression};

///测试带小数的表达式, gitee:
#[test]
fn test_parse_param_with_point_digit() {
    let input_exp = "( ( ( -  DESI[1.1]/2 ) - DESI[0.2] ) )";
    let mut context: DashMap<String, String> = DashMap::new();
    context.insert("DESI1".into(), "30.0".into());
    context.insert("DESI0".into(), "40.0".into());
    let cata_context = CataContext { context, is_tubi: false };
    let r = eval_str_to_f64(input_exp, &cata_context, true, "DIST");
    dbg!(&r);
    assert_eq!(r.unwrap(), -55.0);
}

#[test]
fn test_parse_design_param() {
    let input_exp = "-0.5 TIMES  DESIGN PARAM 1";
    let mut context: DashMap<String, String> = DashMap::new();
    context.insert("DESP1".into(), "30.0".into());
    let cata_context = CataContext { context, is_tubi: false };
    let r = eval_str_to_f64(input_exp, &cata_context, true, "DIST");
    dbg!(&r);
    assert_eq!(r.unwrap(), -15.0);
}

///测试带小数的表达式
#[test]
fn test_parse_param_with_of_operator() {
    let input_exp = "LBOR OF PREV";
    let input_exp = "LBOR OF 24381/88991";
    let mut context: DashMap<String, String> = DashMap::new();
    let interface = get_test_ams_db_manager();
    let cata_context = CataContext { context, is_tubi: false };
    // 是提前准备，还是在使用的时候去获取
    let r = eval_str_to_f64(input_exp, &cata_context, true, "DIST");
    dbg!(&r);
    assert_eq!(r.unwrap(), 850.0);
}

#[test]
fn parse_3_axis() {
    //
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context: DashMap<String, String> = DashMap::new();
    context.insert("DESI14".into(), "30.0".into());
    context.insert("DESI13".into(), "30.0".into());
    context.insert("DDANGLE".into(), "45.0".into());
    context.insert("PARAM 2".into(), "30.0".into());
    context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let cata_context = CataContext { context, is_tubi: false };
    let str = "X ( RPRO_CPAR )  Y ( DESIGN PARAM 13 ) Z";
    // let str = "X ( DESIGN PARAM 14 )  Y ";
    let str = "X (60.0)  Y ";
    let str = "X ( 45 )  Y ( 35 ) Z";
    let str = "TANF PARAM 2 DDANGLE";
    let r = eval_str_to_f64(str, &cata_context, true, "DIST");
    dbg!(r);
}

//[(.*[^-])([-?X|Y|Z])]?
#[test]
fn test_parse_dir() {
    let re = Regex::new(r"(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])(.*[^-])(-?[X|Y|Z])").unwrap();
    let target = "-X (DESIGN PARAM 14 ) -Y";
    // let target = "-X";
    let target = target.trim();
    let target = "-X ( DESIGN PARAM 14 ) -Y ( DESIGN PARAM 19 ) -Z";

    // let re = Regex::new(r"(DESIGN?\s+)?([I|C|O)]?PARAM?)\s*(\d+)").unwrap();
    // let input_exp = "DESIGN PARAM 1";
    // dbg!(caps.into_iter().len());
    for cap in re.captures_iter(&target) {
        dbg!(cap.len());
        // dbg!(&cap[0]);
        dbg!(&cap[1]);
        dbg!(&cap[2]);
        dbg!(&cap[3]);
        dbg!(&cap[4]);
        dbg!(&cap[5]);
        // dbg!(&cap[4]);
        // println!("{} {} {} {}", &cap[1], &cap[2], &cap[3], &cap[4]);
    }
}

#[test]
fn test_rpro() {
    use regex::Captures;
    let s = "RPRO_TLEN";
    // let rpro_regex = Regex::new(r"RPRO\s*([A-Z]+[0-9]*)").unwrap();
    // let mut new_exp = rpro_regex.replace_all(&new_exp, "");
    // dbg!(new_exp);

    let re = Regex::new(r"([A-Z]+[0-9]*)(\s*\[(\d+)\])?").unwrap();
    for caps in re.captures_iter(s) {
        dbg!(&caps[0]);
    }

    let re = Regex::new(r"(RPRO)\s+(\S+)").unwrap();
    let result = re.replace(s, |caps: &Captures| format!("{}_{}", &caps[1], &caps[2]));
    dbg!(result);
}

#[test]
fn test_math_exp() {
    let expr = "MAX ( ( ( - 31 ) + 60 ), 29.2 )";
    let mut context: DashMap<String, String> = DashMap::new();
    let cata_context = CataContext { context, is_tubi: false };
    dbg!(eval_str_to_f64(expr, &cata_context, true, "DIST")).expect("TODO: panic message");
}

/// 测试ATTRIB表达式修复功能
#[test]
fn test_attrib_expression_fix() {
    // 创建测试上下文
    let context = ExpressionFixer::create_test_context();

    // 测试原始问题表达式: ( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )
    let problematic_expr = "( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )";
    println!("测试表达式: {}", problematic_expr);

    // 使用修复器处理表达式
    let result = ExpressionFixer::eval_expression_with_attrib_support(
        problematic_expr,
        &context,
        true,
        "DIST"
    );

    match result {
        Ok(value) => {
            println!("✅ 表达式修复成功！结果: {}", value);
            // MIN(100.0, 40.0) = 40.0
            assert_eq!(value, 40.0);
        }
        Err(e) => {
            println!("❌ 表达式修复失败: {}", e);
            panic!("表达式修复应该成功");
        }
    }
}

/// 测试各种ATTRIB表达式格式
#[test]
fn test_various_attrib_expressions() {
    let context = ExpressionFixer::create_test_context();

    // 测试用例集合
    let test_cases = vec![
        ("ATTRIB HEIG", 100.0, "简单ATTRIB属性"),
        ("ATTRIB PARA[0]", 10.0, "ATTRIB数组索引[0]"),
        ("ATTRIB PARA[3 ]", 40.0, "ATTRIB数组索引带空格"),
        ("MIN(ATTRIB HEIG, ATTRIB WIDT)", 100.0, "MIN函数与ATTRIB"),
        ("MAX(ATTRIB PARA[1], ATTRIB PARA[2])", 30.0, "MAX函数与ATTRIB数组"),
        ("ATTRIB HEIG + ATTRIB PARA[1]", 120.0, "ATTRIB加法运算"),
        ("(ATTRIB WIDT - ATTRIB HEIG) / 2", 50.0, "ATTRIB复合运算"),
    ];

    for (expr, expected, description) in test_cases {
        println!("\n测试: {} - {}", description, expr);

        match eval_pdms_expression(expr, &context) {
            Ok(result) => {
                println!("✅ 结果: {} (期望: {})", result, expected);
                assert_eq!(result, expected, "表达式 '{}' 的结果不匹配", expr);
            }
            Err(e) => {
                println!("❌ 失败: {}", e);
                panic!("表达式 '{}' 应该成功解析", expr);
            }
        }
    }
}

/// 测试表达式预处理功能
#[test]
fn test_expression_preprocessing() {
    let test_cases = vec![
        ("( MIN ( ATTRIB HEIG,ATTRIB PARA[3 ] ) )", "(MIN(HEIG,PARA3))"),
        ("ATTRIB WIDT + ATTRIB LENG", "WIDT + LENG"),
        ("MAX(ATTRIB PARA[0], ATTRIB PARA[1] * 2)", "MAX(PARA0,PARA1 * 2)"),
        ("ATTRIB HEIG / ( ATTRIB PARA[2] + 10 )", "HEIG / (PARA2 + 10)"),
    ];

    for (input, expected) in test_cases {
        let processed = ExpressionFixer::preprocess_attrib_expression(input);
        println!("输入: {} -> 输出: {}", input, processed);
        assert_eq!(processed, expected, "预处理结果不匹配");
    }
}

#[test]
fn test_interp() {
    let input_str =
        "((0.5*500*TAN(/2)+(500+2)*TAN(3/2)*COS(3))/2-((-(500/2+2)*TAN(3/2)+2*COS((90-3)))/2)";
    let result = interp(&input_str.to_lowercase()).unwrap();
    dbg!(&result);
}
