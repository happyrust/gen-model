use std::collections::BTreeMap;
use crate::cata::resolve_helper::{parse_ori_str_to_quat, parse_str_axis_to_vec3};
use crate::data_interface::tidb_manager::AiosDBManager;

//AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X
#[test]
fn parse_axis() {
    // let str = "X ( 45 )  Y ( 35 ) Z";
    //-X (DESIGN PARAM 14 ) -Y
    let mut context = BTreeMap::new();
    context.insert("DESP4".into(), "800.0".into());
    context.insert("DESP5".into(), "300.0".into());
    context.insert("DESP10".into(), "200.0".into());
    context.insert("DESP11".into(), "0.0".into());
    // context.insert("RPRO_CPAR".into(), "DESIGN PARAM 14".into());
    let str = "AXIS -Y ( ATAN ( ( DESP[2 ] / 2 + DESP[10 ] ) / ( DESP[3 ] / 2 - DESP[11 ] ) ) ) X";
    let r = parse_str_axis_to_vec3::<AiosDBManager>(str, &context, None);
    dbg!(r);
    //AXIS -Y ( ATANT ( 0 - DESP[10 ] - ( DESP[4 ] - DESP[5 ] ) / 2 , 0 - DESP[11 ] ) ) -X
    let str = "AXIS -Y (ATANT((DESP[10]-(DESP[4]-DESP[5])/2),(0-DESP[11]))) X";
    let r = parse_str_axis_to_vec3::<AiosDBManager>(str, &context, None);
    dbg!(r);
}


#[test]
fn parse_ori(){
    let str = "Y is W and Z is U";
    let mut context = BTreeMap::new();
    let ori = parse_ori_str_to_quat::<AiosDBManager>(str, &context, None);
    dbg!(ori);
}

