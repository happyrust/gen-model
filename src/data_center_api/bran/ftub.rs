use std::collections::HashMap;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::{AttrVal, PdmsElement, RefU64};
use bevy_transform::prelude::Transform;
use glam::Vec3;
use regex::Regex;
use crate::data_center_api::data_api::{get_refno_desc, get_refno_latest_version, get_refno_paras};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气ftub数据
pub async fn get_dq_ftub_data(refno: &PdmsElement, bran_name: &str, spre_name: &str, room_name: &str,
                              bran_pspec: &str, b_cover: bool, point_map: &HashMap<RefU64, InstPointMap>,
                              aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterInstance> {
    let mut data_center_attr = Vec::new();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_refno_str()).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(bran_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("直段".to_string()).into(),
    });
    let transform = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or(Transform::default());
    let pos = transform.translation;
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART4".to_string(),
        value: AttrValue::AttrVec3(pos).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART5".to_string(),
        value: AttrValue::AttrVec3(ori).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE1".to_string(),
        value: AttrValue::AttrString("直段".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("直段".to_string()).into(),
    });
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE4".to_string(),
        value: AttrValue::AttrString(desc).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE5".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE7".to_string(),
        value: AttrValue::AttrInt(1).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE11".to_string(),
        value: AttrValue::AttrString("Q235B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE14".to_string(),
        value: AttrValue::AttrString(room_name.to_string()).into(),
    });
    let para = get_refno_paras(refno.refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = para.get(0).map_or(0.0, |x| *x);
    let para_2 = para.get(1).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE15".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE16".to_string(),
        value: AttrValue::AttrFloat(para_2 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF27".to_string(),
        value: AttrValue::AttrFloat(para_2 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF28".to_string(),
        value: AttrValue::AttrString(bran_pspec.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF29".to_string(),
        value: AttrValue::AttrBool(b_cover).into(),
    });
    let mut arrive_pos = Vec3::ZERO;
    let mut leave_pos = Vec3::ZERO;
    if let Some(points) = point_map.get(&refno.refno) {
        if let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") {
            if let Some(point_info) = points.ptset_map.get(arrive) {
                let arrive_point = transform.transform_point(point_info.pt);
                arrive_pos = arrive_point;
            }
            if let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") {
                if let Some(point_info) = points.ptset_map.get(leave) {
                    let leave_point = transform.transform_point(point_info.pt);
                    leave_pos = leave_point;
                }
            }
        }
    }
    let distance = arrive_pos.distance(leave_pos);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF25".to_string(),
        value: AttrValue::AttrFloat(distance).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF30".to_string(),
        value: AttrValue::AttrVec3(arrive_pos).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEF31".to_string(),
        value: AttrValue::AttrVec3(leave_pos).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTEF".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}

/// 获取电气ftub数据 spre_name 包含 RDivider
pub async fn get_dq_ftub_contains_rdivider_data(refno: &PdmsElement, bran_name: &str, spre_name: &str,
                                                room_name: &str, ftub_paras: &Vec<f64>,
                                                point_map: &HashMap<RefU64, InstPointMap>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterInstance> {
    let mut data_center_attr = Vec::new();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_refno_str()).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(bran_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("分隔板".to_string()).into(),
    });
    let transform = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or(Transform::default());
    let pos = transform.translation;
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART4".to_string(),
        value: AttrValue::AttrVec3(pos).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART5".to_string(),
        value: AttrValue::AttrVec3(ori).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE1".to_string(),
        value: AttrValue::AttrString("分隔板".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("分隔板".to_string()).into(),
    });
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE4".to_string(),
        value: AttrValue::AttrString(desc).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE5".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE7".to_string(),
        value: AttrValue::AttrInt(1).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE11".to_string(),
        value: AttrValue::AttrString("Q235B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE14".to_string(),
        value: AttrValue::AttrString(room_name.to_string()).into(),
    });
    // let para = get_refno_paras(refno.refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = ftub_paras.get(0).map_or(0.0, |x| *x);
    let para_2 = ftub_paras.get(1).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE15".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE16".to_string(),
        value: AttrValue::AttrFloat(para_2 as f32).into(),
    });
    let mut arrive_pos = Vec3::ZERO;
    let mut leave_pos = Vec3::ZERO;
    if let Some(points) = point_map.get(&refno.refno) {
        if let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") {
            if let Some(point_info) = points.ptset_map.get(arrive) {
                let arrive_point = transform.transform_point(point_info.pt);
                arrive_pos = arrive_point;
            }
            if let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") {
                if let Some(point_info) = points.ptset_map.get(leave) {
                    let leave_point = transform.transform_point(point_info.pt);
                    leave_pos = leave_point;
                }
            }
        }
    }
    let distance = arrive_pos.distance(leave_pos);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEG25".to_string(),
        value: AttrValue::AttrFloat(distance).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTEG".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}

/// 获取电气ftub数据 spre_name 包含 RDivider
pub async fn get_dq_ftub_contains_riser_data(refno: &PdmsElement, bran_name: &str, spre_name: &str,
                                                room_name: &str, ftub_paras: &Vec<f64>,
                                                aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterInstance> {
    let mut data_center_attr = Vec::new();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_refno_str()).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(bran_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("竖梯".to_string()).into(),
    });
    let transform = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or(Transform::default());
    let pos = transform.translation;
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART4".to_string(),
        value: AttrValue::AttrVec3(pos).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART5".to_string(),
        value: AttrValue::AttrVec3(ori).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE1".to_string(),
        value: AttrValue::AttrString("竖梯".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("竖梯".to_string()).into(),
    });
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE4".to_string(),
        value: AttrValue::AttrString(desc).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE5".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE7".to_string(),
        value: AttrValue::AttrInt(1).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE11".to_string(),
        value: AttrValue::AttrString("Q235B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE14".to_string(),
        value: AttrValue::AttrString(room_name.to_string()).into(),
    });
    // let para = get_refno_paras(refno.refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = ftub_paras.get(0).map_or(0.0, |x| *x);
    let para_2 = ftub_paras.get(1).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE15".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE16".to_string(),
        value: AttrValue::AttrFloat(para_2 as f32).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTEH".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}