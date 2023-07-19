use std::collections::HashMap;
use aios_core::data_center::AttrValue::{AttrIntArray, AttrMap, AttrString};
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use regex::Regex;
use crate::api::attr::query_explicit_attr;
use crate::aql_api::children::{query_children_eles, query_refnos_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql};
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::data_api::{get_dq_material_code, get_refno_desc, get_refno_desp, get_refno_paras};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

/// 获取 管段元数据
pub fn get_data_center_bran_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let segma_1 = vec![1, 2, 3, 4];
    let segma_2 = "TEST".to_string();
    let segma_3 = "安装基准点".to_string();
    let segma_4 = "TEST".to_string();
    let segma_5 = "TEST_TEST_TEST_TEST".to_string();
    let segma_6 = "TEST_TEST_TEST_TEST".to_string();
    let segma_7 = "TEST_TEST_TEST_TEST".to_string();
    let mut map = HashMap::new();
    map.insert("流向1".to_string(), vec!["支吊架编号1".to_string(), "支吊架编号2".to_string()]);
    let segma_8 = map;
    let segma_9 = "TEST".to_string();
    let segma_10 = "TEST_TEST_TEST_TEST".to_string();

    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA1".to_string(),
        value: AttrIntArray(segma_1).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA2".to_string(),
        value: AttrString(segma_2).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA3".to_string(),
        value: AttrString(segma_3).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA4".to_string(),
        value: AttrString(segma_4).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA5".to_string(),
        value: AttrString(segma_5).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA6".to_string(),
        value: AttrString(segma_6).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA7".to_string(),
        value: AttrString(segma_7).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA8".to_string(),
        value: AttrMap(segma_8).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA9".to_string(),
        value: AttrString(segma_9).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA10".to_string(),
        value: AttrString(segma_10).into(),
    });
    result
}

/// 给排水专业 获取 bran和pipe的name
pub async fn get_sg_pipe_bran_name(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                     vec!["PIPE".to_string()]).await {
        for pipe in children {
            let brans = query_children_eles(&database, pipe.refno).await?;
            for bran in brans {
                if bran.noun != "BRAN" { continue; };
                let mut attr = Vec::new();
                attr.push(DataCenterAttr {
                    attribute_model_code: "SEGMA1".to_string(),
                    value: bran.name.clone(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "SEGMA2".to_string(),
                    value: pipe.name.clone(),
                });
                result.push(DataCenterInstance {
                    object_model_code: "SEGMA".to_string(),
                    project_code: "1516".to_string(),
                    instance_code: bran.name,
                    version: "A版".to_string(),
                    attributes: attr,
                })
            }
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

/// 获取电气专业的 bran 下面的元件信息
pub async fn get_dq_bran_data(refnos: &[RefU64], aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    let regex = Regex::new(r"\d.*:\d")?; // 判断字符串是否包含有多个数字加一个:
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                     vec!["BRAN".to_string()]).await {
        for bran in children {
            let bran_children = query_children_eles(&database, bran.refno).await?;
            let room_name = query_room_name_from_refno_aql(bran.refno, &database).await?.unwrap_or("".to_string());
            let pspe_name = query_foreign_name_aql(bran.refno, vec!["PSPE", "PSPE"], &database).await?;
            let mut kind = "".to_string();
            if let Some(pspe_name) = pspe_name {
                match pspe_name {
                    s if s.contains("Ladder") => { kind = "梯架".to_string() }
                    s if s.contains("Ventilated") => { kind = "带孔托盘".to_string() }
                    s if s.contains("Trough") => { kind = "实底托盘".to_string() }
                    s if s.contains("Riser") => { kind = "竖梯".to_string() }
                    s if s.contains("Divider") => { kind = "分隔板".to_string() }
                    _ => {}
                }
            }

            let mut tray_width = "".to_string();
            let mut tray_height = "".to_string();
            let mut bridge_dir = "".to_string();
            let mut b_climbing = false;
            let mut b_wheel = false;
            // 找到bran下的第一个ftub
            for child in &bran_children {
                if child.noun == "ATTA" { continue; }
                if !bridge_dir.is_empty() {
                    let spre_name = query_foreign_name_aql(child.refno, vec!["SPRE", "SPRE"], &database).await?;
                    if let Some(spre_name) = spre_name {
                        if spre_name.contains("Riser") || spre_name.contains("RDivider") {
                            bridge_dir = "竖向".to_string();
                        } else {
                            bridge_dir = "水平".to_string();
                        }
                    }
                }

                if child.noun == "FTUB" {
                    let mut paras = get_refno_paras(child.refno, aios_mgr).await?;
                    tray_width = paras.get(0).unwrap_or(&0.0).to_string();
                    tray_height = paras.get(1).unwrap_or(&0.0).to_string();
                    break;
                }
                if !b_climbing {
                    if child.noun == "ELBO" {
                        b_climbing = true;
                    }
                }
                if !b_wheel {
                    if child.noun == "BEND" {
                        b_wheel = true;
                    }
                }
            }

            let bran_name_split = bran.name.split("-").collect::<Vec<_>>();
            let mut color = "".to_string();
            let mut b_paint = false;
            if bran_name_split.len() >= 2 {
                color = bran_name_split[1].to_string();
                if color == "CO" { b_paint = true }
            }
            let desc = get_refno_desc(bran.refno, aios_mgr).await?;
            let b_partition = if desc.is_empty() { false } else { true };

            let mut erecb_attr = Vec::new();
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB1".to_string(),
                value: AttrValue::AttrString(bran.name.clone()).into(),
            });

            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB3".to_string(),
                value: AttrValue::AttrString(room_name.clone()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB25".to_string(),
                value: AttrValue::AttrBool(b_climbing).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB27".to_string(),
                value: AttrValue::AttrBool(b_wheel).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB21".to_string(),
                value: AttrValue::AttrString(format!("{}mm{}", tray_width, kind)).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB31".to_string(),
                value: AttrValue::AttrString(kind.clone()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB32".to_string(),
                value: AttrValue::AttrString(color.to_string()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB33".to_string(),
                value: AttrValue::AttrBool(b_paint).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB34".to_string(),
                value: AttrValue::AttrBool(b_partition).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB35".to_string(),
                value: AttrValue::AttrString(bridge_dir.to_string()).into(),
            });
            result.push(DataCenterInstance {
                object_model_code: "ERECB".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: bran.name,
                version: "A版".to_string(),
                attributes: erecb_attr,
            });

            for child in bran_children {
                let spre_name = query_foreign_name_aql(child.refno, vec!["SPRE", "SPRE"], &database).await?.unwrap_or_default();
                let mut object_code = None;
                match child.noun.as_str() {
                    "FTUB" => { if spre_name.contains("RISER") { object_code = Some("PARTEH") } else { object_code = Some("PARTEF") } }
                    "TEE" => { object_code = Some("PARTEA") }
                    "CROS" => { object_code = Some("PARTEJ") }
                    "BEND" => { if regex.is_match(&spre_name) { object_code = Some("PARTEB") } }
                    _ => {}
                }
                if object_code.is_none() { continue; };

                let mut attr = Vec::new();

                let world_transform = aios_mgr.get_world_transform(child.refno).await?.unwrap_or_default();
                attr.push(DataCenterAttr {
                    attribute_model_code: "PART4".to_string(),
                    value: AttrValue::AttrVec3(world_transform.translation).into(),
                });

                let stander_num = get_refno_desc(child.refno, aios_mgr).await.unwrap_or_default();
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTE4".to_string(),
                    value: AttrValue::AttrString(stander_num.to_string()).into(),
                });
                let material_map = get_dq_material_code(&spre_name,
                                                        &stander_num, &vec!["ItemCode".to_string(), "Unit".to_string()], aios_mgr).await.unwrap_or_default();
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTE5".to_string(),
                    value: AttrValue::AttrString(material_map.get("ItemCode").unwrap_or(&"".to_string()).to_string()).into(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTE12".to_string(),
                    value: AttrValue::AttrString(material_map.get("Unit").unwrap_or(&"".to_string()).to_string()).into(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTE15".to_string(),
                    value: AttrValue::AttrString(tray_width.to_string()).into(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTE16".to_string(),
                    value: AttrValue::AttrString(tray_height.to_string()).into(),
                });

                // let desp = get_refno_desp(child.refno,aios_mgr).await.unwrap_or_default();
                // let mut b_cover = if desp.len() < 4 { false } else { if desp[3] == 1.0 { true } else { false } };
                // attr.push(DataCenterAttr {
                //     attribute_model_code: "PARTEF29".to_string(),
                //     value: AttrValue::AttrBool(b_cover).into(),
                // });

                result.push(DataCenterInstance {
                    object_model_code: object_code.unwrap().to_string(),
                    project_code: aios_mgr.db_option.project_code.to_string(),
                    instance_code: child.name.to_string(),
                    version: "A版".to_string(),
                    attributes: attr,
                });
            }
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}