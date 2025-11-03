use aios_core::tool::direction_parse::*;
use aios_core::tool::math_tool::*;
use aios_core::*;
use dashmap::DashMap;

use crate::cata::resolve_helper::{parse_ori_str_to_quat, parse_str_axis_to_vec3};
use crate::data_interface::tidb_manager::AiosDBManager;

//AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X
#[test]
fn parse_axis() {
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context: DashMap<String, String> = DashMap::new();
    context.insert("DESP4".into(), "800.0".into());
    context.insert("DESP5".into(), "300.0".into());
    context.insert("DESP10".into(), "200.0".into());
    context.insert("DESP11".into(), "0.0".into());
    let cata_context = CataContext {
        context,
        is_tubi: false,
    };
    // context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let str = "AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X";
    let r = parse_str_axis_to_vec3(str, &cata_context);
    dbg!(r);
    //AXIS -Y ( ATANT ( 0 - DESP[10 ] - ( DESP[4 ] - DESP[5 ] ) / 2 , 0 - DESP[11 ] ) ) -X
    let str = "AXIS -Y (ATANT((DESP[10]-(DESP[4]-DESP[5])/2),(0-DESP[11]))) X";
    let r = parse_str_axis_to_vec3(str, &cata_context);
    dbg!(r);
}

#[test]
fn parse_ori() {
    let str = "Y is W and Z is U";
    let mut context: DashMap<String, String> = DashMap::new();
    let cata_context = CataContext {
        context,
        is_tubi: false,
    };
    let ori = parse_ori_str_to_quat(str, &cata_context);
    dbg!(ori);
}

fn test_parse_dir() -> anyhow::Result<()> {
    // let axis_str = "Y27.041-X";
    // let axis_str = "Y";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    // let axis_str = "-Y";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    // let axis_str = "-Y30X";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    // let axis_str = "Y30-X";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    // let axis_str = "-Y30-X";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    // let axis_str = "-X30-Y";
    // let mut addition_axis = parse_expr_to_dir(axis_str).unwrap_or_default();
    // let mut mat3 = cal_mat3_by_zdir(addition_axis);
    // dbg!(to_pdms_ori_str(&mat3));

    return Ok(());
}
