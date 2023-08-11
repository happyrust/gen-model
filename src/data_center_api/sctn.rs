use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::{EleTreeNode, RefU64};
use crate::api::element::query_children;
use crate::aql_api::children::{query_ancestor_till_type_aql, query_ancestor_till_types_aql, query_children_eles, query_refnos_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::data_center_api::data_api::{get_ori_angle_str, get_refno_desc, get_refno_desi_desc, get_refno_latest_version, get_refno_paras, get_refno_world_poss_pose};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气支吊架 型钢数据
pub async fn get_dq_support_sctn_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager, sctn_types: Vec<String>) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    // 圆盘的数据
    // 1516为 sctn 1907为 gensec,sctn
    // let mut select_type = if aios_mgr.db_option.project_code == "1516" { vec!["SCTN"] } else { vec!["GENSEC","SCTN"] };
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, sctn_types).await {
        for child in children {
            let Ok(implicit_attr) = aios_mgr.get_implicit_attr(child.refno, Some(vec!["GTYP"])).await else { continue; };
            let Some(gtype) = implicit_attr.get_str("GTYP") else { continue; };
            let mut stru_desc = None;
            match gtype {
                "BOX" => {
                    let attr = get_dq_support_sctn_gtype_box_data(
                        &child, aios_mgr).await.unwrap_or((vec![], "".to_string()));
                    result.push(DataCenterInstance {
                        object_model_code: "PARTDA".to_string(),
                        project_code: aios_mgr.db_option.project_code.to_string(),
                        instance_code: child.name,
                        version: get_refno_latest_version(),
                        attributes: attr.0,
                    });
                }
                "BEAM" => {
                    let attr = get_dq_support_sctn_gtype_beam_data(
                        &child, aios_mgr).await.unwrap_or((vec![], "".to_string()));
                    result.push(DataCenterInstance {
                        object_model_code: "PARTDB".to_string(),
                        project_code: aios_mgr.db_option.project_code.to_string(),
                        instance_code: child.name,
                        version: get_refno_latest_version(),
                        attributes: attr.0,
                    });
                }
                _ => {}
            }
            // 圆板类
            if child.noun == "GENSEC" {
                let Ok(fixings) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                             vec!["FIXING".to_string()]).await else { continue; };
                for fixing in fixings {
                    let mut fixing_attrs = Vec::new();
                    let Some(spre_name) = query_foreign_name_aql(fixing.refno,
                                                                 vec!["SPRE", "SPRE"], &database).await? else { continue; };
                    match spre_name {
                        s if s.contains("JT3") => {
                            let desc = get_refno_desc(fixing.refno, &aios_mgr).await.unwrap_or("".to_string());
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTD15".to_string(),
                                value: desc,
                            });
                            let paras = get_refno_paras(fixing.refno, &aios_mgr).await.unwrap_or(Vec::new());
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK1".to_string(),
                                value: AttrValue::AttrString(format!("{}X{}", paras.get(0).unwrap_or(&0.0)
                                                                     , paras.get(1).unwrap_or(&0.0))).into(),
                            });
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK2".to_string(),
                                value: AttrValue::AttrFloat(*(paras.get(2).unwrap_or(&0.0)) as f32).into(),
                            });
                            let stru = query_ancestor_till_types_aql(&database, fixing.refno, vec!["STRU"]).await?;
                            if let Some(stru) = stru {
                                let desc = get_refno_desi_desc(stru.refno, &aios_mgr).await.unwrap_or("".to_string());
                                stru_desc = Some(desc);
                            }
                            if let Some(stru_desc) = &stru_desc {
                                match stru_desc {
                                    s if s.contains("S1-150") => {
                                        fixing_attrs.push(DataCenterAttr {
                                            attribute_model_code: "PARTDK3".to_string(),
                                            value: AttrValue::AttrFloat(2.0 * *paras.get(3).unwrap_or(&0.0) as f32).into(),
                                        });
                                    }
                                    s if s.contains("S1-151") => {
                                        fixing_attrs.push(DataCenterAttr {
                                            attribute_model_code: "PARTDK4".to_string(),
                                            value: AttrValue::AttrFloat(*paras.get(4).unwrap_or(&0.0) as f32).into(),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                        s if s.contains("JT4") => {
                            let desc = get_refno_desc(fixing.refno, &aios_mgr).await.unwrap_or("".to_string());
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTD15".to_string(),
                                value: desc,
                            });
                            let paras = get_refno_paras(fixing.refno, &aios_mgr).await.unwrap_or(Vec::new());
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK1".to_string(),
                                value: AttrValue::AttrFloat(*paras.get(6).unwrap_or(&0.0) as f32).into(),
                            });
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK2".to_string(),
                                value: AttrValue::AttrFloat((*paras.get(7).unwrap_or(&0.0)) as f32).into(),
                            });
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK3".to_string(),
                                value: AttrValue::AttrFloat((*paras.get(1).unwrap_or(&0.0)) as f32).into(),
                            });
                        }
                        _ => { continue; }
                    }
                    result.push(DataCenterInstance {
                        object_model_code: "PARTDK".to_string(),
                        project_code: aios_mgr.db_option.project_code.to_string(),
                        instance_code: fixing.refno.to_refno_str(),
                        version: get_refno_latest_version(),
                        attributes: fixing_attrs,
                    });
                }
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

/// 获取电气支吊架 型钢数据 gtype 为 box
async fn get_dq_support_sctn_gtype_box_data(refno: &EleTreeNode, aios_mgr: &AiosDBManager) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr).await.unwrap_or("".to_string());
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], &database).await?.unwrap_or("".to_string());
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();
    if let Some(spre_name_split_last) = spre_name_split.last() {
        let spre_name_split_last_split = spre_name_split_last.split("X").collect::<Vec<_>>();
        if spre_name_split_last_split.len() >= 3 {
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDA26".to_string(),
                value: AttrValue::AttrString(format!("{}X{}", spre_name_split_last_split[0], spre_name_split_last_split[1])).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDA27".to_string(),
                value: AttrValue::AttrString(spre_name_split[2].to_string()).into(),
            });
        }
    }
    if let Ok(Some((poss, pose))) = get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await {
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDA31".to_string(),
            value: AttrValue::AttrVec3(pose).into(),
        });
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDA32".to_string(),
            value: AttrValue::AttrVec3(poss).into(),
        });
        let distance = pose.distance(poss);
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDA28".to_string(),
            value: AttrValue::AttrFloat(distance).into(),
        });
    }
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA29".to_string(),
        value: AttrValue::AttrString("全焊透".to_string()).into(),
    });
    let ori_str = get_ori_angle_str(refno.refno, aios_mgr).await.unwrap_or("".to_string());
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA33".to_string(),
        value: AttrValue::AttrString(ori_str).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA34".to_string(),
        value: AttrValue::AttrString("100".to_string()).into(),
    });
    Ok((attr, desc))
}

/// 获取电气支吊架 型钢数据 gtype 为 beam
async fn get_dq_support_sctn_gtype_beam_data(refno: &EleTreeNode, aios_mgr: &AiosDBManager) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr).await.unwrap_or("".to_string());
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    let children = query_children_eles(&database, refno.refno).await?;
    let mut fitt = None;
    for child in children {
        if child.noun == "FITT" {
            fitt = Some(child.refno);
            break;
        }
    }
    let fitt_spre_name = if let Some(fitt) = fitt {
        Some(query_foreign_name_aql(fitt, vec!["SPRE", "SPRE"], &database).await
            .unwrap_or(None).unwrap_or("".to_string()))
    } else {
        None
    };
    match fitt_spre_name.clone() {
        Some(s) if s.contains("Z2") => {
            attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("异型钢/异型钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
        }
        Some(s) if s.contains("Z3") => {
            attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("方钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("槽钢/方钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("槽钢".to_string()).into(),
            });
        }
        None => {
            attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("槽钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("槽钢/槽钢".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("槽钢/槽钢".to_string()).into(),
            });
        }
        _ => {}
    }

    if let Ok(Some((poss, pose))) = get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await {
        let distance = poss.distance(pose);
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB27".to_string(),
            value: AttrValue::AttrFloat(distance).into(),
        });
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB36".to_string(),
            value: AttrValue::AttrVec3(poss).into(),
        });
    }

    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], &database).await?.unwrap_or("".to_string());
    let spre_name_split_last = spre_name.split("-").collect::<Vec<_>>().last().unwrap_or(&"").to_string();
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB32".to_string(),
        value: AttrValue::AttrString(spre_name_split_last.to_string()).into(),
    });
    let fitt_spre_name_split = if let Some(fitt_spre_name) = fitt_spre_name {
        fitt_spre_name.split("-").collect::<Vec<_>>().last().unwrap_or(&"").to_string()
    } else {
        spre_name_split_last
    };
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB33".to_string(),
        value: AttrValue::AttrString(fitt_spre_name_split.to_string()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB35".to_string(),
        value: AttrValue::AttrString("连续角焊".to_string()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB39".to_string(),
        value: AttrValue::AttrString("A4-80".to_string()).into(),
    });
    Ok((attr, desc))
}


#[tokio::test]
async fn test_query_around_owner_within_radius() {
    let mgr = AiosDBManager::init_form_config().await.unwrap();
    let refno = RefU64::from_refno_str("24383/96911").unwrap();
    let result = mgr.query_around_owner_within_radius(refno,true,None,true,vec!["BRAN"]).await.unwrap();
    dbg!(&result);
}