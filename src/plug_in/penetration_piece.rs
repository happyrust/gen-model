use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use aios_core::penetration::{PenetrationData, PenetrationVec};
use nalgebra::{Unit, Vector2, Vector3};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::interface::PdmsDataInterface;
pub async fn get_penetration_detail_by_refno(aios_mgr: &AiosDBManager, refno_vec: &mut Vec<(RefU64, RefU64)>) -> anyhow::Result<PenetrationVec> {
    let mut hole_data_vec = PenetrationVec::default();
    for i in refno_vec {
        let mut data = PenetrationData::default();
        if let Ok(Some(translation)) = aios_mgr.get_world_transform(i.0.clone()).await {
            data.position = translation.translation;
        }
        get_x_deviation_angle(&mut data);

        //暂时默认壳内房间号
        data.inner_room_num = "101".to_string();
        data.outer_room_num = "102".to_string();

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
///得到贯穿件X轴偏移角度
pub fn get_x_deviation_angle(mut data: &mut PenetrationData) {
//得到偏移角度
    let v1 = Vector3::new(data.position.x, data.position.y,0.);
    let v2 = Vector3::new(1., 0.,0.);
    let unit_v1 = Unit::new_normalize(v1);
    let unit_v2 = Unit::new_normalize(v2);
    let dot_product = unit_v2.dot(&unit_v1);
    //获得角度
    let angle_in_radians = 360.0 as f32 -dot_product.acos().to_degrees();
    data.x_deviation_angle = angle_in_radians.to_string();
}



