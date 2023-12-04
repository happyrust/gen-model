use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::*;
use aios_core::pdms_user::RefnoMajor;
use bevy_transform::prelude::Transform;
use nalgebra::sup;
use parry3d::utils::hashmap::HashMap;
use regex::Regex;
use crate::aql_api::children::{get_uda_type_refnos_from_select_refnos, query_ancestor_name_of_type_aql, query_children_eles, query_refnos_belong_major, query_refnos_travel_children_with_type_aql, query_travel_children_with_type_aql};
use crate::aql_api::pdms_room::{query_room_name_from_owner_aql, query_room_name_from_refno_aql};
use crate::data_center_api::data_api::{get_refno_desc, get_refno_desi_desc, get_refno_latest_version, get_refnos_major_map, take_off_name_first_char};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气支吊架信息
pub async fn get_dq_support_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                     vec!["STRU".to_string()]).await {
        let refnos = children.iter().map(|child| child.refno).collect::<Vec<RefU64>>();
        let major_map = get_refnos_major_map(refnos, &database).await.unwrap_or_default();
        for stru in children {
            let mut attr = Vec::new();
            let Ok(stru_attr) = aios_mgr.get_attr(stru.refno).await else { continue; };
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB1".to_string(),
                value: AttrValue::AttrString(stru.name.to_string()).into(),
            });

            let bran_refno = aios_mgr
                .query_around_owner_within_radius(stru.refno, true, None, true, &["BRAN"])
                .await
                .unwrap_or(vec![]);
            let bran_name = if !bran_refno.is_empty() {
                let bran_name = aios_mgr.get_name(bran_refno[0]).await.unwrap_or("".to_string());
                if bran_name.starts_with("/") { bran_name[1..].to_string() } else { bran_name }
            } else {
                "".to_string()
            };
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB2".to_string(),
                value: AttrValue::AttrString(bran_name).into(),
            });

            let room_code = query_room_name_from_refno_aql(stru.refno, &database).await?;
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB3".to_string(),
                value: AttrValue::AttrString(room_code.unwrap_or("".to_string())).into(),
            });

            let frmw = query_children_eles(&database, stru.refno).await?;
            if !frmw.is_empty() {
                let desc = get_refno_desi_desc(frmw[0].refno, aios_mgr).await.unwrap_or("".to_string());
                attr.push(DataCenterAttr {
                    attribute_model_code: "ERECAB4".to_string(),
                    value: AttrValue::AttrString(desc).into(),
                });
            }
            let file_code = stru_attr.get_str(":WJBM").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB5".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let file_code = stru_attr.get_str(":NBBM").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB6".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let file_code = stru_attr.get_str(":ZD_GCDH").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB7".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let file_code = stru_attr.get_str(":ZD_JZBH").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB8".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let file_code = stru_attr.get_str(":ZD_ZXHM").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB9".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let file_code = stru_attr.get_str(":ZD_ZXMC").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB10".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            let major = major_map.get(&stru.refno).map_or(RefnoMajor::default(), |x| x.clone());
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB11".to_string(),
                value: AttrValue::AttrString(major.major.to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB12".to_string(),
                value: AttrValue::AttrString(major.major_classify.to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB13".to_string(),
                value: AttrValue::AttrString("QA2".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB14".to_string(),
                value: AttrValue::AttrString("F-SC1".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB15".to_string(),
                value: AttrValue::AttrString("NA".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB16".to_string(),
                value: AttrValue::AttrString("抗震I级".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB17".to_string(),
                value: AttrValue::AttrString("NA".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB18".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            let room_name = query_room_name_from_refno_aql(stru.refno, &database).await?.unwrap_or("".to_string());
            attr.push(DataCenterAttr {
                attribute_model_code: "ROOM2".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            let zone_name = query_ancestor_name_of_type_aql(&database, stru.refno, "ZONE").await
                .unwrap_or(None).unwrap_or("".to_string());
            let zone_code = if zone_name.contains("MSUP") {
                "全焊透".to_string()
            } else if zone_name.contains("LSUP") {
                "角焊".to_string()
            } else {
                "".to_string()
            };
            // S2，判断STRU>DESC是否包含floor来分辨吊架还是支架，吊架取方钢的高点，支架取方钢低点。S1找圆板中心点取Z轴最高的点算
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB22".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            // 判断 MSUP：全焊透，LSUP：角焊
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB24".to_string(),
                value: AttrValue::AttrString(zone_code).into(),
            });
            let desc = get_refno_desi_desc(stru.refno, aios_mgr).await.unwrap_or("".to_string());
            let split_desc = desc.clone().split("-").map(|desc| desc.to_string()).collect::<Vec<_>>();
            // 计算所属房间顶底标高
            let support_type = if desc.starts_with("S2") && desc.contains("FLOOR") { "支架".to_string() } else { "吊架".to_string() };
            let panel = aios_mgr.query_own_room_panel_elevations(stru.refno, None).await.unwrap_or_default();
            if !panel.is_empty() {
                for (_panel, (min, max)) in panel {
                    if support_type == "支架".to_string() {
                        attr.push(DataCenterAttr {
                            attribute_model_code: "ERECAB41".to_string(),
                            value: AttrValue::AttrFloat(max).into(),
                        });
                    } else {
                        attr.push(DataCenterAttr {
                            attribute_model_code: "ERECAB41".to_string(),
                            value: AttrValue::AttrFloat(min).into(),
                        });
                    }
                    break;
                }
            } else {
                attr.push(DataCenterAttr {
                    attribute_model_code: "ERECAB41".to_string(),
                    value: AttrValue::AttrFloat(0.0).into(),
                });
            }
            // 支吊架的类型
            let support_type = split_desc.first().unwrap_or(&"".to_string()).to_string();
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB42".to_string(),

                value: AttrValue::AttrString("0.07".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB43".to_string(),
                value: AttrValue::AttrString("0.1".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB44".to_string(),
                value: AttrValue::AttrString("40.0".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB45".to_string(),
                value: AttrValue::AttrString(support_type).into(),
            });
            // 托臂个数
            let gensec = query_travel_children_with_type_aql(&database, stru.refno, "GENSEC").await.unwrap_or(vec![]);
            let mut gensec_count = 0;
            for g in gensec {
                if g.name.contains("BAR") {
                    gensec_count += 1;
                }
            }
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB46".to_string(),
                value: AttrValue::AttrInt(gensec_count).into(),
            });
            // STRU>DESC中不包含S1-S17就不是典型支架
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB47".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            // 设计阶段
            let file_code = stru_attr.get_str(":3D_SJJD").unwrap_or("");
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB48".to_string(),
                value: AttrValue::AttrString(file_code.to_string()).into(),
            });
            // attr.push(DataCenterAttr {
            //     attribute_model_code: "ERECAB49".to_string(),
            //     value: AttrValue::AttrFloatArray(vec![0.0, 0.0]).into(),
            // });
            result.push(DataCenterInstance {
                object_model_code: "ERECAB".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: stru.name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}


/// 仪表管道支吊架类
///
/// SITE name contains(INSTHB)>ZONE name contains(SUPP) > :SUPP
pub async fn query_dq_erecad_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let database = aios_mgr.get_arango_db().await?;
    let mut result = Vec::new();
    // 查询自定义类型 :SUPP
    let supps = get_uda_type_refnos_from_select_refnos(refnos, "SUPP", "ZONE", aios_mgr).await.unwrap_or(vec![]);
    // 通过 owner返回房间号（只返回一个）
    let supp_refnos = supps.iter().map(|s| s.refno).collect::<Vec<RefU64>>();
    let room_name_map = query_room_name_from_owner_aql(supp_refnos, &database).await?;
    let room_name_map = room_name_map.into_iter()
        .map(|r| (r.refno, r.room_name))
        .collect::<HashMap<RefU64, String>>();

    for supp in supps {
        let mut data_center_attr = Vec::new();
        let Ok(attr) = aios_mgr.get_attr(supp.refno).await else { continue; };
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD1".to_string(),
            value: supp.name.clone(),
        });
        // :ZD_BZBM，取,前面的
        let standard_num = attr.get_str(":ZD_BZBM").map_or("".to_string(), |x| x.to_string());
        let split = standard_num.split(",").collect::<Vec<_>>().first().map_or("".to_string(), |x| x.to_string());
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD2".to_string(),
            value: split.clone(),
        });
        let room_name = room_name_map.get(&supp.refno).map_or("".to_string(), |x| x.to_string());
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD3".to_string(),
            value: room_name,
        });
        let transform = aios_mgr.get_world_transform(supp.refno).await?.unwrap_or_default();
        let pos = transform.translation;
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD4".to_string(),
            value: AttrValue::AttrVec3(pos).into(),
        });

        let mut b_anti_seismic = "".to_string();
        if split == "1SA".to_string() {
            b_anti_seismic = "抗震".to_string();
        } else if split == "1S".to_string() {
            b_anti_seismic = "非抗震".to_string();
        }
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD5".to_string(),
            value: AttrValue::AttrString(b_anti_seismic).into(),
        });
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECAD6".to_string(),
            value: AttrValue::AttrString(standard_num).into(),
        });

        result.push(DataCenterInstance {
            object_model_code: "ERECAD".to_string(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: supp.name,
            version: get_refno_latest_version(),
            attributes: data_center_attr,
        });
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

pub async fn query_dq_erecc_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let database = aios_mgr.get_arango_db().await?;
    let mut result = Vec::new();
    // 查询自定义类型 :SUPP
    let supps = get_uda_type_refnos_from_select_refnos(refnos, "SUPP", "ZONE", aios_mgr).await.unwrap_or(vec![]);
    // 通过 owner返回房间号（只返回一个）
    let supp_refnos = supps.iter().map(|s| s.refno).collect::<Vec<RefU64>>();
    let room_name_map = query_room_name_from_owner_aql(supp_refnos, &database).await?;
    let room_name_map = room_name_map.into_iter()
        .map(|r| (r.refno, r.room_name))
        .collect::<HashMap<RefU64, String>>();

    for supp in supps {
        let mut data_center_attr = Vec::new();
        let Ok(attr) = aios_mgr.get_attr(supp.refno).await else { continue; };
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECC1".to_string(),
            value: supp.name.clone(),
        });
        // :ZD_BZBM
        let standard_num = attr.get_str(":ZD_BZBM").map_or("".to_string(), |x| x.to_string());
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECC2".to_string(),
            value: standard_num,
        });
        let room_name = room_name_map.get(&supp.refno).map_or("".to_string(), |x| x.to_string());
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECC3".to_string(),
            value: room_name,
        });
        let equi_num = attr.get_str(":ZD_YBSB").map_or("".to_string(), |x| x.to_string());
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "ERECC4".to_string(),
            value: equi_num,
        });

        result.push(DataCenterInstance {
            object_model_code: "ERECC".to_string(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: supp.name,
            version: get_refno_latest_version(),
            attributes: data_center_attr,
        });
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

/// 判断字段串是否包含 S1到 S17的任意一种
fn contains_s1_to_s17(input: &str) -> bool {
    let regex_str = r"S(1[0-7]|0?[1-9])[^0-9]+";
    let regex = Regex::new(regex_str).unwrap();
    regex.is_match(input)
}

#[test]
fn test_contains_s1_to_s17() {
    let input = "S17-";
    dbg!(contains_s1_to_s17(input));
}

#[tokio::test]
async fn test_get_dq_support_data() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let refnos = vec![RefU64::from_str("24383/86099").unwrap()];
    let result = get_dq_support_data(refnos, &aios_mgr).await?;
    let mut file = std::fs::File::create("data_center_test/ERECAB.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}