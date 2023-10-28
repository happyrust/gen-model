use std::collections::HashMap;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::*;
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::{ PdmsElement};
use aios_core::types::*;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::Vec3;
use regex::Regex;
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

/// 获取电气bend数据
pub async fn get_dq_bend_data(refno: &PdmsElement, bran_name: &str, spre_name: &str, room_name: &str, ftub_paras: &Vec<f64>,
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
        value: AttrValue::AttrString("成品弯通".to_string()).into(),
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
        value: AttrValue::AttrString("成品弯通".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("成品弯通".to_string()).into(),
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
    let para = get_refno_paras(refno.refno, aios_mgr).unwrap_or(vec![]);
    let para_11 = para.get(10).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB26".to_string(),
        value: AttrValue::AttrFloat(para_11 as f32).into(),
    });
    let number = capture_number_with_char(spre_name).unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB27".to_string(),
        value: AttrValue::AttrString(number).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB28".to_string(),
        value: AttrValue::AttrString(bran_pspec.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB29".to_string(),
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
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB30".to_string(),
        value: AttrValue::AttrVec3(arrive_pos).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEB31".to_string(),
        value: AttrValue::AttrVec3(leave_pos).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTEB".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}

/// 获取工艺专业bend数据
pub async fn get_gy_bend_data(refno: &PdmsElement, bran_name: &str, room_code: String,
                              database: &ArDatabase, aios_mgr: &AiosDBManager) -> DataCenterInstance {
    let mut result = Vec::new();
    let spre_attr = aios_mgr.get_foreign_attrmap(refno.refno, "SPRE").unwrap_or_default();
    let spre_name = spre_attr.get_name().unwrap_or("".to_string());
    let bran_spre_material_code = get_spre_material_code(&spre_name).unwrap_or("".to_string());
    let need_query_material_code = vec![("ITEMA11".to_string(), "Code".to_string()),
                                        ("ITEMA12".to_string(), "Name".to_string()),
                                        ("ITEMA13".to_string(), "Make".to_string()),
                                        ("ITEMA14".to_string(), "Mat".to_string()),
                                        ("ITEMA15".to_string(), "MatSpec".to_string()),
                                        ("ITEMA16".to_string(), "Spec".to_string()),
                                        ("ITEMA17".to_string(), "RCCM".to_string()),
                                        ("ITEMA18".to_string(), "QAGrade".to_string()),
                                        ("ITEMAA2".to_string(), "Weight".to_string()),
                                        ("ITEMAA5".to_string(), "Diameter".to_string()),
                                        ("ITEMAA7".to_string(), "Link".to_string())];
    result.push(DataCenterAttr {
        attribute_model_code: "ITEM1".to_string(),
        value: AttrString(refno.refno.to_refno_string()).into(),
    });
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEMA1".to_string(),
        value: AttrString(refno.name.clone()).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMA2".to_string(),
        value: AttrString(refno.noun.clone()).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMA3".to_string(),
        value: AttrString(bran_name.to_string()).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMA4".to_string(),
        value: AttrString("".to_string()).into(),
    };
    result.push(item_4);
    let world_position = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or_default();
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMA5".to_string(),
        value: AttrVec3(world_position.translation).into(),
    };
    result.push(item_5);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMA8".to_string(),
        value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
    };
    result.push(item_8);

    let material_map = if let Ok(puhua_pool) = aios_mgr.get_puhua_pool().await {
        let query_code = need_query_material_code.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
        let material_map = get_material_map_from_code(&bran_spre_material_code, query_code, &puhua_pool).await;
        material_map
    } else {
        DashMap::default()
    };
    for (item_code, material_code) in &need_query_material_code {
        let material = if material_map.contains_key(material_code) {
            material_map.get(material_code).unwrap().value().clone()
        } else {
            "".to_string()
        };
        result.push(DataCenterAttr {
            attribute_model_code: item_code.to_string(),
            value: material,
        });
    }

    // 单位 kg
    let weight_unit: f32 = if material_map.contains_key("Weight") {
        material_map.get("Weight").unwrap().value().clone().parse().unwrap_or(0.0)
    } else {
        0.0
    };
    // let weight = length * weight_unit / 1000.0;
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMAA2".to_string(),
        value: AttrFloat(weight_unit).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA20".to_string(),
        value: AttrString(room_code).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();

    let ispec = get_ispec_from_attr(&attr, &aios_mgr).await.unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA21".to_string(),
        value: AttrString(ispec).into(),
    });
    let tspe = query_foreign_name_aql(refno.refno, vec!["TSPE", "TSPE"], database).await.unwrap_or(None).unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA22".to_string(),
        value: AttrString(tspe).into(),
    });
    let r_text = get_rtext_from_attr(&attr, aios_mgr).await.unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA24".to_string(),
        value: AttrString(r_text).into(),
    });
    get_material_pressure_code("ITEMAA3", "ITEMAA4", "ITEMAA6", &mut result, &material_map);
    DataCenterInstance {
        object_model_code: "ITEMAA".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.clone(),
        version: get_refno_latest_version(),
        attributes: result,
    }
}

/// 获取电气bend数据 angle != 45 || 90
pub async fn get_dq_bend_angle_data(refno: &PdmsElement, bran_name: &str, spre_name: &str,
                                    room_name: &str, ftub_paras: &Vec<f64>, angle: f32,
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
        value: AttrValue::AttrString("水平可调连接板".to_string()).into(),
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
        value: AttrValue::AttrString("水平可调连接板".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("水平可调连接板".to_string()).into(),
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
    let para_3 = para.get(2).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEC25".to_string(),
        value: AttrValue::AttrFloat(para_3 as f32).into(),
    });
    let ftub_para_1 = ftub_paras.get(0).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEC27".to_string(),
        value: AttrValue::AttrFloat(ftub_para_1 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEC28".to_string(),
        value: AttrValue::AttrFloat(angle).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTEC".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}


/// 给一个字符串，判断这个字符串中是否带有数字加: 例如（“hello45:”），如果有，则返回这个数字
fn capture_number_with_char(input: &str) -> Option<String> {
    let Ok(regex) = Regex::new(r"(\d+):") else { return None; };
    // 在输入字符串中查找匹配
    if let Some(captures) = regex.captures(input) {
        // 提取匹配的部分
        if let Some(number_match) = captures.get(1) {
            return Some(number_match.as_str().to_string());
        }
    }
    None
}

#[test]
fn test_capture_number_with_char() {
    let input = "45:";
    let number = capture_number_with_char(input);
    dbg!(number);
}