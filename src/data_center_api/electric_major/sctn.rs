use std::io::Write;
use crate::api::element::query_children;
use crate::aql_api::children::{
    query_ancestor_till_type_aql, query_ancestor_till_types_aql, query_children_eles,
    query_children_order_aql, query_refnos_travel_children_with_type_aql,
};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::data_center_api::data_api::{
    get_ori_angle_str, get_pspec_code, get_refno_desc, get_refno_desi_desc,
    get_refno_latest_version, get_refno_paras, get_refno_world_poss_pose,
};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::{EleTreeNode, RefU64};
use arangors_lite::AqlQuery;
use crate::arangodb::ArDatabase;
use serde::{Serialize, Deserialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use crate::consts::{AQL_FOREIGN_EDGES_COLLECTION, AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION};

/// 获取电气支吊架 型钢数据
pub async fn get_dq_support_sctn_data(
    refnos: Vec<RefU64>,
    aios_mgr: &AiosDBManager,
    sctn_types: Vec<String>,
) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    let Ok(children) =
        query_refnos_travel_children_with_type_aql(&database, &refnos, sctn_types).await
        else { return Ok(DataCenterProject::default()); };
    for child in children.clone() {
        let Ok(attr) = aios_mgr.get_attr(child.refno).await else { continue; };
        let Some(gtype) = attr.get_str("GTYP") else {
            continue;
        };
        match gtype {
            "BOX" => {
                let attr = get_dq_support_sctn_gtype_box_data(&child, aios_mgr)
                    .await
                    .unwrap_or((vec![], "".to_string()));
                result.push(DataCenterInstance {
                    object_model_code: "PARTDA".to_string(),
                    project_code: aios_mgr.db_option.project_code.to_string(),
                    instance_code: child.name,
                    version: get_refno_latest_version(),
                    attributes: attr.0,
                });
            }
            "BEAM" => {
                let attr = get_dq_support_sctn_gtype_beam_data(&child, aios_mgr)
                    .await
                    .unwrap_or((vec![], "".to_string()));
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
    }
    // 圆板类
    let children = children.iter().map(|child| child.refno).collect::<Vec<_>>();
    let fixings = query_dq_circular_plate(children, &database).await.unwrap_or(vec![]);
    for fixing in fixings {
        let mut fixing_attrs = Vec::new();
        let spre_name = fixing.spre_name;
        match spre_name {
            s if s.contains("JT3") => {
                let mut stru_desc = None;
                let desc = get_refno_desc(fixing.refno, &aios_mgr)
                    .await
                    .unwrap_or("".to_string());
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTD15".to_string(),
                    value: desc,
                });
                let paras = get_refno_paras(fixing.refno, &aios_mgr)
                    .unwrap_or(Vec::new());
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTDK1".to_string(),
                    value: AttrValue::AttrString(format!(
                        "{}X{}",
                        paras.get(0).unwrap_or(&0.0),
                        paras.get(1).unwrap_or(&0.0)
                    ))
                        .into(),
                });
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTDK2".to_string(),
                    value: AttrValue::AttrFloat(*(paras.get(2).unwrap_or(&0.0)) as f32)
                        .into(),
                });
                let stru = query_ancestor_till_types_aql(
                    &database,
                    fixing.refno,
                    vec!["STRU"],
                ).await.unwrap_or(None);
                if let Some(stru) = stru {
                    let desc = get_refno_desi_desc(stru.refno, &aios_mgr)
                        .await
                        .unwrap_or("".to_string());
                    stru_desc = Some(desc);
                }
                if let Some(stru_desc) = &stru_desc {
                    match stru_desc {
                        s if s.contains("S1-150") => {
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK3".to_string(),
                                value: AttrValue::AttrFloat(
                                    2.0 * *paras.get(3).unwrap_or(&0.0) as f32,
                                ).into(),
                            });
                        }
                        s if s.contains("S1-151") => {
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK4".to_string(),
                                value: AttrValue::AttrFloat(
                                    *paras.get(4).unwrap_or(&0.0) as f32,
                                ).into(),
                            });
                        }
                        _ => {
                            fixing_attrs.push(DataCenterAttr {
                                attribute_model_code: "PARTDK4".to_string(),
                                value: AttrValue::AttrFloat(0.0).into(),
                            });
                        }
                    }
                }
            }
            s if s.contains("JT4") => {
                let desc = get_refno_desc(fixing.refno, &aios_mgr)
                    .await
                    .unwrap_or("".to_string());
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTD15".to_string(),
                    value: desc,
                });
                let paras = get_refno_paras(fixing.refno, &aios_mgr)
                    .unwrap_or(Vec::new());
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTDK1".to_string(),
                    value: AttrValue::AttrFloat(*paras.get(6).unwrap_or(&0.0) as f32)
                        .into(),
                });
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTDK2".to_string(),
                    value: AttrValue::AttrFloat((*paras.get(7).unwrap_or(&0.0)) as f32)
                        .into(),
                });
                fixing_attrs.push(DataCenterAttr {
                    attribute_model_code: "PARTDK3".to_string(),
                    value: AttrValue::AttrFloat((*paras.get(1).unwrap_or(&0.0)) as f32)
                        .into(),
                });
            }
            _ => {
                continue;
            }
        }
        result.push(DataCenterInstance {
            object_model_code: "PARTDK".to_string(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: fixing.refno.to_refno_str(),
            version: get_refno_latest_version(),
            attributes: fixing_attrs,
        });
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

/// 获取电气支吊架 型钢数据 gtype 为 box
async fn get_dq_support_sctn_gtype_box_data(
    refno: &EleTreeNode,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], &database)
        .await?
        .unwrap_or("".to_string());
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();
    if let Some(spre_name_split_last) = spre_name_split.last() {
        let spre_name_split_last_split = spre_name_split_last.split("X").collect::<Vec<_>>();
        if spre_name_split_last_split.len() >= 3 {
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDA26".to_string(),
                value: AttrValue::AttrString(format!(
                    "{}X{}",
                    spre_name_split_last_split[0], spre_name_split_last_split[1]
                ))
                    .into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "PARTDA27".to_string(),
                value: AttrValue::AttrString(spre_name_split[2].to_string()).into(),
            });
        }
    }
    if let Ok(Some((poss, pose))) =
        get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await
    {
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
    let ori_str = get_ori_angle_str(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
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
async fn get_dq_support_sctn_gtype_beam_data(
    refno: &EleTreeNode,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
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
        Some(
            query_foreign_name_aql(fitt, vec!["SPRE", "SPRE"], &database)
                .await
                .unwrap_or(None)
                .unwrap_or("".to_string()),
        )
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

    if let Ok(Some((poss, pose))) =
        get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await
    {
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

    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], &database)
        .await?
        .unwrap_or("".to_string());
    let spre_name_split_last = spre_name
        .split("-")
        .collect::<Vec<_>>()
        .last()
        .unwrap_or(&"")
        .to_string();
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB32".to_string(),
        value: AttrValue::AttrString(spre_name_split_last.to_string()).into(),
    });
    let fitt_spre_name_split = if let Some(fitt_spre_name) = fitt_spre_name {
        fitt_spre_name
            .split("-")
            .collect::<Vec<_>>()
            .last()
            .unwrap_or(&"")
            .to_string()
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
        attribute_model_code: "PARTDB38".to_string(),
        value: AttrValue::AttrString("M12".to_string()).into(),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB39".to_string(),
        value: AttrValue::AttrString("A4-80".to_string()).into(),
    });
    let bran_refno = aios_mgr
        .query_around_owner_within_radius(refno.refno, true, None, true, &["BRAN"])
        .await
        .unwrap_or(vec![]);
    let mut ftub = None;
    if !bran_refno.is_empty() {
        let bran_refno = bran_refno[0];
        let children = query_children_order_aql(&database, bran_refno).await?;
        // 找到第一个 ftub
        for child in children {
            if child.noun == "FTUB" {
                ftub = Some(child.refno);
                break;
            }
        }
        if ftub.is_some() {
            let paras = get_refno_paras(ftub.unwrap(), aios_mgr)
                .unwrap_or(vec![]);
            if !paras.is_empty() {
                // let bolt = get_tray_bolt_specifications(paras[0] as f32);
                let pspec = get_pspec_code(bran_refno, &database)
                    .await
                    .unwrap_or("".to_string());
                let spacing = get_tray_bolt_spacing(&pspec, paras[0] as f32);
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDB40".to_string(),
                    value: AttrValue::AttrString(spacing).into(),
                });
                let connection_method = get_tray_connection_method(paras[0] as f32);
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDB41".to_string(),
                    value: AttrValue::AttrString(connection_method).into(),
                });
            }
        }
    }
    if bran_refno.is_empty() || ftub.is_none() {
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB40".to_string(),
            value: AttrValue::AttrString("".to_string()).into(),
        });
        attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB41".to_string(),
            value: AttrValue::AttrString("".to_string()).into(),
        });
    }
    Ok((attr, desc))
}

#[serde_as]
#[derive(Clone, Default, Serialize, Deserialize)]
struct EleNodeWithSpreName {
    #[serde_as(as = "DisplayFromStr")]
    pub refno: RefU64,
    pub noun: String,
    pub name: String,
    #[serde_as(as = "DisplayFromStr")]
    pub owner: RefU64,
    pub spre_name: String,
}

/// 获取电气圆板的数据
async fn query_dq_circular_plate(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<EleNodeWithSpreName>> {
    let id = refnos.into_iter()
        .map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno()))
        .collect::<Vec<_>>();
    let aql = AqlQuery::new("
        with @@pdms_edges,@@pdms_eles,@@foreign_edges
        for refno in @id
        let node = document(refno)
        filter node != null
        filter node.noun == 'GENSEC'
        FOR z in 0..100 INBOUND node._id @@pdms_edges
        filter z.noun == 'FIXING'
        filter z != null
        let foreign = (
        for v, e, p in 1..2 outbound z._id @@foreign_edges
            filter p.edges[0].foreign_type == 'SPRE'
            filter e.foreign_type == 'SPRE'
            return v.name
        )
        filter foreign[0] != null
        return {
            'refno':z._key,
            'owner':z.owner,
            'name':z.name,
            'noun':z.noun,
            'spre_name': foreign[0]
        }
      ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@foreign_edges", AQL_FOREIGN_EDGES_COLLECTION)
        .bind_var("id", id);
    let result = database.aql_query::<EleNodeWithSpreName>(aql).await.unwrap();
    Ok(result)
}

/// 获取电气托盘托臂耦合螺栓规格表
fn get_tray_bolt_specifications(para_1: f32) -> String {
    match para_1 {
        600.0 | 500.0 | 300.0 => "M12".to_string(),
        200.0 | 100.0 | 50.0 => "M8".to_string(),
        _ => "".to_string(),
    }
}

// 获取托臂托盘螺栓连接方式对照表
fn get_tray_connection_method(para_1: f32) -> String {
    match para_1 {
        600.0 | 500.0 | 300.0 | 200.0 => "双螺栓".to_string(),
        100.0 | 50.0 => "单螺栓".to_string(),
        _ => "".to_string(),
    }
}

// 获取托臂托盘耦合螺栓间距距距离
fn get_tray_bolt_spacing(tray_type: &str, para1: f32) -> String {
    match tray_type {
        "梯架" => match para1 {
            600.0 => "540mm".to_string(),
            500.0 => "440mm".to_string(),
            300.0 => "240mm".to_string(),
            _ => "".to_string(),
        },
        "实底" => match para1 {
            600.0 => "500mm".to_string(),
            500.0 => "400mm".to_string(),
            300.0 => "200mm".to_string(),
            200.0 => "100mm".to_string(),
            _ => "".to_string(),
        },
        "带孔托盘" => match para1 {
            200.0 => "100mm".to_string(),
            _ => "".to_string(),
        },
        _ => "".to_string(),
    }
}

#[tokio::test]
async fn test_query_around_owner_within_radius() {
    let mgr = AiosDBManager::init_form_config().await.unwrap();
    let refno = RefU64::from_refno_str("24383/96911").unwrap();
    let result = mgr
        .query_around_owner_within_radius(refno, true, None, true, &["BRAN"])
        .await
        .unwrap();
    dbg!(&result);
}

#[tokio::test]
async fn test_get_dq_support_sctn_data() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let refnos = vec![RefU64::from_refno_str("24383/86099").unwrap()];
    let result = get_dq_support_sctn_data(refnos, &aios_mgr, vec![]).await?;
    let mut file = std::fs::File::create("data_center_test/PARTDA_PARTDB_PARTDK.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}