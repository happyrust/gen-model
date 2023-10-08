use std::collections::HashMap;
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::{AttrVal, PdmsElement, RefU64};
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::Vec3;
use crate::api::element::{query_ele_node, query_name};
use crate::aql_api::attr_map::query_refnos_point_map_aql;
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_bran_itema_attr, get_refno_desc, get_refno_latest_version, get_refno_paras, get_refnos_arrive_leave_info, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

pub async fn get_data_center_tee_attr(refno: PdmsElement, bran_name: &str, database: &ArDatabase, aios_mgr: &AiosDBManager) -> DataCenterInstance {
    let need_query_material_code = vec![("ITEMA11".to_string(), "Code".to_string()),
                                        ("ITEMA12".to_string(), "Name".to_string()), ("ITEMA13".to_string(), "Make".to_string()),
                                        ("ITEMA14".to_string(), "Mat".to_string()),
                                        ("ITEMA15".to_string(), "MatSpec".to_string()),
                                        ("ITEMA16".to_string(), "Spec".to_string()),
                                        ("ITEMA17".to_string(), "RCCM".to_string()),
                                        ("ITEMA18".to_string(), "QAGrade".to_string()), ];
    let mut result = Vec::new();
    // 重复的取值
    get_bran_itema_attr(refno.clone(), bran_name, database, aios_mgr, &mut result).await;

    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], database).await.unwrap_or(None).unwrap_or("".to_string());
    let material_code = get_spre_material_code(&spre_name).unwrap_or("".to_string());
    let material_map = if let Ok(puhua_pool) = aios_mgr.get_puhua_pool().await {
        let query_code = need_query_material_code.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
        let material_map = get_material_map_from_code(&material_code, query_code, &puhua_pool).await;
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
    DataCenterInstance {
        object_model_code: "ITEMAB".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name,
        version: get_refno_latest_version(),
        attributes: result,
    }
}

/// 获取电气三通数据
pub async fn get_dq_tee_data(refno: &PdmsElement, bran_name: &str, spre_name: &str, room_name: &str, ftub_paras: &Vec<f64>,
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
        value: AttrValue::AttrString("三通".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE2".to_string(),
        value: AttrValue::AttrString("TEE".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("三通".to_string()).into(),
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
    let para_6 = para.get(5).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA26".to_string(),
        value: AttrValue::AttrFloat(para_6 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA27".to_string(),
        value: AttrValue::AttrString(bran_pspec.to_string()).into(),
    });
    let mut arrive_pos = Vec3::ZERO;
    let mut leave_pos = Vec3::ZERO;
    let mut other_pos = Vec3::ZERO;
    let mut other_pbore = 0.0;
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
                for i in 1..4 {
                    if *arrive == i || *leave == i {
                        continue;
                    }
                    if points.ptset_map.contains_key(&i) {
                        let other_point = points.ptset_map.get(&i).unwrap();
                        other_pos = transform.transform_point(other_point.pt);
                        other_pbore = other_point.pbore;
                    }
                }
            }
        }
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA28".to_string(),
        value: AttrValue::AttrFloat(other_pbore).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA29".to_string(),
        value: AttrValue::AttrBool(b_cover).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA30".to_string(),
        value: AttrValue::AttrVec3(arrive_pos).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA31".to_string(),
        value: AttrValue::AttrVec3(leave_pos).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTEA32".to_string(),
        value: AttrValue::AttrVec3(other_pos).into(),
    });
    Ok(DataCenterInstance{
        object_model_code: "PARTEA".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}


#[tokio::test]
async fn test_get_data_center_tee_attr() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let tee_refno = RefU64::from_refno_str("24383/66752").unwrap();
    let pool = aios_mgr.get_project_pool_by_refno(tee_refno).await.unwrap();
    let tee_node = query_ele_node(tee_refno, &pool.1).await.unwrap();
    let owner_name = query_name(tee_node.owner, &pool.1).await.unwrap();

    let result = get_data_center_tee_attr(tee_node.into(), &owner_name, &database, &aios_mgr).await;
    let mut file = std::fs::File::create("tee.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}