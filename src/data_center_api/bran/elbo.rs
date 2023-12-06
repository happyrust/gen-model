use std::collections::BTreeMap;
use aios_core::AttrMap;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString};
use aios_core::pdms_types::*;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::Vec3;
use crate::api::attr::query_explicit_attr;
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql};
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_bran_itema_attr, get_ispec_from_attr, get_material_pressure_code, get_refno_desc, get_refno_latest_version, get_refno_paras, get_rtext_from_attr, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_data_center_elbo_attr(refno: PdmsElement, bran_name: &str, room_code: String,
                                       database: &ArDatabase, aios_mgr: &AiosDBManager) -> DataCenterInstance {
    let need_query_material_code = vec![("ITEMA11".to_string(), "Code".to_string()),
                                        ("ITEMA12".to_string(), "Name".to_string()),
                                        ("ITEMA13".to_string(), "Make".to_string()),
                                        ("ITEMA14".to_string(), "Mat".to_string()),
                                        ("ITEMA15".to_string(), "MatSpec".to_string()),
                                        ("ITEMA16".to_string(), "Spec".to_string()),
                                        ("ITEMA17".to_string(), "RCCM".to_string()),
                                        ("ITEMA18".to_string(), "QAGrade".to_string()),
                                        ("ITEMA19".to_string(), "Weight".to_string()),
                                        ("ITEMAD5".to_string(), "Diameter".to_string()),
                                        ("ITEMAD7".to_string(), "Link".to_string())];
    let mut result = Vec::new();
    // 重复的取值
    get_bran_itema_attr(refno.clone(), bran_name, room_code,database, aios_mgr, &mut result).await;

    let spre_attr = aios_mgr.get_foreign_attrmap(refno.refno, "SPRE").unwrap_or_default();
    let spre_name = spre_attr.get_name().unwrap_or("".to_string());
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
    get_material_pressure_code("ITEMAD3", "ITEMAD4", "ITEMAD6", &mut result, &material_map);
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let radius = get_elbo_radius(&attr, aios_mgr).await.unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMAD1".to_string(),
        value: AttrString(radius).into(),
    });
    let angle = attr.get_f32("ANGLE").unwrap_or(90.0);
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMAD2".to_string(),
        value: AttrFloat(angle).into(),
    });
    DataCenterInstance {
        object_model_code: "ITEMAD".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name,
        version: get_refno_latest_version(),
        attributes: result,
    }
}

/// 手动获取部分数据中台 布置专业
pub async fn get_data_center_attr_handle(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<BTreeMap<String, DataCenterAttr>> {
    let mut map = BTreeMap::new();
    let ispec = get_ispec_from_attr(attr, aios_mgr).await?;
    map.entry("ITEMA21".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMA21".to_string(),
        value: ispec,
    });
    let rtext = get_rtext_from_attr(attr, aios_mgr).await?;
    map.entry("ITEMA24".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMA24".to_string(),
        value: rtext,
    });
    let radius = get_elbo_radius(attr, aios_mgr).await?;
    map.entry("ITEMAD1".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMAD1".to_string(),
        value: radius,
    });
    Ok(map)
}

/// 获取 elbo 的弯曲半径 都默认为 para 2
async fn get_elbo_radius(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let Some(refno) = attr.get_refno() else { return Ok("".to_string()); };
    let database = aios_mgr.get_arango_db().await?;
    let catr = query_foreign_refno_aql(&database, refno, &vec!["SPRE", "CATR"]).await?;
    if let Some(catr) = catr {
        let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok("".to_string()); };
        let catr_explicit = query_explicit_attr(catr, &pool).await?;
        if let Some(para) = catr_explicit.get_f32_vec("PARA") {
            if para.len() > 1 {
                return Ok(para[1].to_string());
            }
        }
    }
    Ok("".to_string())
}

/// 电气专业 type = elbo and spre contain LED/LEU
pub async fn get_dq_elbo_spre_data(pe: &PdmsElement, bran_name: &str, spre_name: &str,
                                   room_name: &str, ftub_paras: &Vec<f64>, angle: f32,
                                   aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterInstance> {
    let mut data_center_attr = Vec::new();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(pe.refno.to_string()).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(bran_name.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("垂直可调连接板".to_string()).into(),
    });
    let transform = aios_mgr.get_world_transform_or_default(pe.refno).await;
    let pos = transform.translation;
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART4".to_string(),
        value: AttrValue::AttrVec3(pos).into(),
    });
    let attr = aios_mgr.get_attr(pe.refno).await.unwrap_or_default();
    let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART5".to_string(),
        value: AttrValue::AttrVec3(ori).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE1".to_string(),
        value: AttrValue::AttrString("垂直可调连接板".to_string()).into(),
    });
    // data_center_attr.push(DataCenterAttr {
    //     attribute_model_code: "PARTE2".to_string(),
    //     value: AttrValue::AttrString("TEE".to_string()).into(),
    // });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTE3".to_string(),
        value: AttrValue::AttrString("垂直可调连接板".to_string()).into(),
    });
    let desc = get_refno_desc(pe.refno, aios_mgr)
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
    let para = get_refno_paras(pe.refno, aios_mgr).unwrap_or(vec![]);
    let para_3 = para.get(2).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTED25".to_string(),
        value: AttrValue::AttrFloat(para_3 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTED28".to_string(),
        value: AttrValue::AttrFloat(angle).into(),
    });
    Ok(DataCenterInstance {
        object_model_code: "PARTED".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: pe.name.to_string(),
        version: get_refno_latest_version(),
        attributes: data_center_attr,
    })
}