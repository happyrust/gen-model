use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use aios_core::penetration::{PenetrationData, PenetrationVec};
use nalgebra::{Unit, Vector2, Vector3};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::interface::PdmsDataInterface;

//得到贯穿件详细信息
pub async fn get_penetration_detail_by_refno(aios_mgr: &AiosDBManager, refno_vec: &mut Vec<(RefU64, RefU64)>) -> anyhow::Result<PenetrationVec> {
    let mut hole_data_vec = PenetrationVec::default();
    for i in refno_vec {
        let mut data = PenetrationData::default();
        if let Ok(Some(translation)) = aios_mgr.get_world_transform(i.0.clone()).await {
            data.position = translation.translation;
        }
        //X偏移角
        get_x_deviation_angle(&mut data);
        //壳内房间号和壳外房间号
        get_room_number(&mut data);
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            data.owner_refno = i.1.clone();
            data.refno = i.0.clone();
            data.name = attr.get_name().to_string();
            if attr.get_name().to_string().contains("ZZZ") {
                hole_data_vec.data.push(data);
            }
        }
    }
    return Ok(hole_data_vec);
}

///得到壳内房间号和壳外房间号
fn get_room_number(data: &mut PenetrationData) {
//暂时默认壳内房间号
    data.inner_room_num = "101".to_string();
    data.outer_room_num = "102".to_string();
}

///得到贯穿件X轴偏移角度
pub fn get_x_deviation_angle(mut data: &mut PenetrationData) {
    let x: f32 = data.position.x;
    let y: f32 = data.position.y;
    let mut angle = 0.0;
    //y>0,取其补角；y<0,取其相反数
    if y > 0.0 {
        angle = 360.0 - y.atan2(x).to_degrees();
    } else {
        angle = -y.atan2(x).to_degrees();
    }
    //四舍五入成整数
    let angle_i32 = angle.round() as i32;
    data.x_deviation_angle = angle_i32.to_string();
}


#[test]
pub fn test_x_deviation_angle() {
    let x: f32 = 21815.76;
    let y: f32 = 11599.72;
    let mut angle = 0.0;
    if y > 0.0 {
        angle = 360.0 - y.atan2(x).to_degrees();
    } else {
        angle = -y.atan2(x).to_degrees();
    }
    let angle_i32 = angle.round() as i32;
    dbg!(angle_i32);
}

