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
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            //找到name中包含“ZZZ”的元素
            if attr.get_name_or_default().contains("ZZZ") {
                let mut data = PenetrationData::default();
                if let Ok(Some(translation)) = aios_mgr.get_world_transform(i.0.clone()).await {
                    //获得位置
                    data.position = translation.translation;
                }
                //获得父亲结点refno
                data.owner_refno = i.1.clone();
                //获得自身refno
                data.refno = i.0.clone();
                //获得name
                data.name = attr.get_name_or_default();
                //获得x偏移角度
                get_x_deviation_angle(&mut data);
                //获得高差
                let height_difference = aios_mgr.query_eles_keypts_and_aabb_as_whole(&[i.0.clone()], true).await;
                if let Ok(height_diff) = height_difference {
                    if height_diff.is_some() {
                        if height_diff.as_ref().unwrap().0.len() > 2 {
                            let z1 = height_diff.as_ref().unwrap().0[0].z;
                            let z2 = height_diff.as_ref().unwrap().0[1].z;
                            data.height_difference = (z1 - z2).abs();
                        }
                    }
                }
                //获得房间号
                let rooms_number = aios_mgr.query_through_element_room_nums(&[i.0.clone()], None).await;
                if let Ok(rooms_number) = rooms_number {
                    for (key, value) in rooms_number {
                        //获得内房间号
                        data.inner_room_num = value.0;
                        //获得外房间号
                        data.outer_room_num = value.1;
                    }
                }

                hole_data_vec.data.push(data);
            }
        }
    }
    return Ok(hole_data_vec);
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

