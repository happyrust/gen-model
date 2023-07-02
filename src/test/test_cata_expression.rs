use std::collections::BTreeMap;
use aios_core::pdms_types::{AttrMap, AttrVal, RefU64};
use aios_core::tiny_expr::expr_eval::interp;
use regex::Regex;
use crate::cata::resolve_helper::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::test::test_helper::get_test_ams_db_manager;

///测试带小数的表达式, gitee:
#[test]
fn test_parse_param_with_point_digit() {
    let input_exp = "( ( ( -  DESI[1.1]/2 ) - DESI[0.2] ) )";
    let mut context = BTreeMap::new();
    context.insert("DESI1".into(), "30.0".into());
    context.insert("DESI0".into(), "40.0".into());
    let r = eval_str_to_f64::<AiosDBManager>(input_exp, &context, None, true);
    dbg!(&r);
    assert_eq!(r.unwrap(), -55.0);
}

#[test]
fn test_parse_design_param() {
    let input_exp = "-0.5 TIMES  DESIGN PARAM 1";
    let mut context = BTreeMap::new();
    context.insert("DESI1".into(), "30.0".into());
    let r = eval_str_to_f64::<AiosDBManager>(input_exp, &context, None, true);
    dbg!(&r);
    assert_eq!(r.unwrap(), -15.0);
}

///测试带小数的表达式
#[test]
fn test_parse_param_with_of_operator() {
    let input_exp = "LBOR OF PREV";
    let input_exp = "LBOR OF 24381/88991";
    let mut context = BTreeMap::new();
    let interface = get_test_ams_db_manager();
    // 是提前准备，还是在使用的时候去获取
    let r = eval_str_to_f64::<AiosDBManager>(input_exp, &context, Some(&interface), true);
    dbg!(&r);
    assert_eq!(r.unwrap(), 850.0);
}


#[test]
fn parse_3_axis() {
    //
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context = BTreeMap::new();
    context.insert("DESI14".into(), "30.0".into());
    context.insert("DESI13".into(), "30.0".into());
    context.insert("DDANGLE".into(), "45.0".into());
    context.insert("PARAM 2".into(), "30.0".into());
    context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let str = "X ( RPRO_CPAR )  Y ( DESIGN PARAM 13 ) Z";
    // let str = "X ( DESIGN PARAM 14 )  Y ";
    let str = "X (60.0)  Y ";
    let str = "X ( 45 )  Y ( 35 ) Z";
    let str = "TANF PARAM 2 DDANGLE";
    let r = eval_str_to_f64::<AiosDBManager>(str, &context, None, true);
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
    let result = re.replace(s, |caps: &Captures| {
        format!("{}_{}", &caps[1], &caps[2])
    });
    dbg!(result);
}

#[test]
fn test_math_exp() {
    let expr = "MAX ( ( ( - 31 ) + 60 ), 29.2 )";
    let context = BTreeMap::new();
    dbg!(eval_str_to_f64::<AiosDBManager>(expr, &context, None, true)).expect("TODO: panic message");
}

//todo fix
#[test]
fn test_interp() {
    let input_str = "((0.5*500*TAN(/2)+(500+2)*TAN(3/2)*COS(3))/2-((-(500/2+2)*TAN(3/2)+2*COS((90-3)))/2)";
    let result = interp(&input_str.to_lowercase()).unwrap();
    dbg!(&result);
}