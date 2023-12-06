use std::collections::HashMap;
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
use aios_core::pdms_types::*;
use arangors_lite::AqlQuery;
use bevy_transform::prelude::Transform;
use glam::Vec3;
use crate::arangodb::ArDatabase;
use serde::{Serialize, Deserialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use crate::aql_api::pdms_room::query_room_name_from_refnos_aql;
use crate::consts::{AQL_FOREIGN_EDGES_COLLECTION, AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION};
use crate::data_center_api::electric_major::fixing::get_dq_fixing_data;

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
    // 查询所有需要的房间号
    let refnos = children.iter().map(|child| child.refno).collect::<Vec<RefU64>>();
    let room_map = query_room_name_from_refnos_aql(refnos, &database).await?;
    let room_map = room_map.into_iter()
        .map(|x| (x.refno, x.room_name))
        .collect::<HashMap<RefU64, String>>();

    for child in children.clone() {
        let Ok(attr) = aios_mgr.get_attr(child.refno).await else { continue; };
        let Some(gtype) = attr.get_str("GTYP") else { continue; };
        match gtype {
            "BOX" => {
                let attr = get_dq_support_sctn_gtype_box_data(&child, &room_map, aios_mgr)
                    .await
                    .unwrap_or((vec![], "".to_string()));
                if !attr.0.is_empty() {
                    result.push(DataCenterInstance {
                        object_model_code: "PARTDA".to_string(),
                        project_code: aios_mgr.db_option.project_code.to_string(),
                        instance_code: child.name,
                        version: get_refno_latest_version(),
                        attributes: attr.0,
                    });
                }
            }
            "BEAM" => {
                let attr = get_dq_support_sctn_gtype_beam_data(&child, &room_map, aios_mgr)
                    .await
                    .unwrap_or((vec![], "".to_string()));
                if !attr.0.is_empty() {
                    result.push(DataCenterInstance {
                        object_model_code: "PARTDB".to_string(),
                        project_code: aios_mgr.db_option.project_code.to_string(),
                        instance_code: child.name,
                        version: get_refno_latest_version(),
                        attributes: attr.0,
                    });
                }
            }
            _ => {
                let spre_attr = aios_mgr.get_foreign_attrmap(child.refno, "SPRE").unwrap_or_default();
                let spre_name = spre_attr.get_name().unwrap_or("".to_string());
                if spre_name.contains("S10") {
                    let r = get_dq_support_sctn_spre_s10_data(&child, &room_map, &spre_name, aios_mgr).await.unwrap_or((vec![], "".to_string()));
                    if !r.0.is_empty() {
                        result.push(DataCenterInstance {
                            object_model_code: "PARTDD".to_string(),
                            project_code: aios_mgr.db_option.project_code.to_string(),
                            instance_code: child.name,
                            version: get_refno_latest_version(),
                            attributes: r.0,
                        });
                    }
                } else if spre_name.contains("S11") {
                    let r = get_dq_support_sctn_spre_s11_data(&child, &room_map, &spre_name, aios_mgr).await.unwrap_or((vec![], "".to_string()));
                    if !r.0.is_empty() {
                        result.push(DataCenterInstance {
                            object_model_code: "PARTDC".to_string(),
                            project_code: aios_mgr.db_option.project_code.to_string(),
                            instance_code: child.name,
                            version: get_refno_latest_version(),
                            attributes: r.0,
                        });
                    }
                }
            }
        }
    }
    // 圆板类
    let children = children.iter().map(|child| child.refno).collect::<Vec<_>>();
    let fixings = get_dq_fixing_data(children, aios_mgr).await.unwrap_or(vec![]);
    for fixing in fixings {
        result.push(fixing);
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
    room_map: &HashMap<RefU64, String>,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut data_center_attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_string()).into(),
    });
    let mut stru_desc = "".to_string();
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(refno.refno, &vec!["STRU"]);
    // 往上找到STRU的NAME
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        if let Some(desc) = owner_attr.get_str("DESC") {
            stru_desc = desc.to_string();
        }
        owner_name = owner_attr.get_name().unwrap_or("".to_string());
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(owner_name).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("方钢".to_string()).into(),
    });

    let transform = aios_mgr.get_world_transform_or_default(refno.refno).await;


    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART5".to_string(),
        value: AttrValue::AttrVec3(ori).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD2".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("方钢".to_string()).into(),
    });
    let spre_attr = aios_mgr.get_foreign_attrmap(refno.refno, "SPRE").unwrap_or_default();
    let spre_name = spre_attr.get_name().unwrap_or("".to_string());
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();

    let spre_name_first = spre_name_split.first().unwrap_or(&"").to_string();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name_first).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("mm".to_string()).into(),
    });
    let room_code = room_map.get(&refno.refno).map_or("".to_string(), |x| x.to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD14".to_string(),
        value: AttrValue::AttrString(room_code).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    let catr_attr = aios_mgr.get_cat_attmap(refno.refno).unwrap_or_default();
    // PARA1xPARA1xPARA2
    let paras = catr_attr.get_f32_vec("PARA").unwrap_or(vec![]);
    let para_0 = paras.get(0).map_or(0.0, |x| *x);
    let para_1 = paras.get(1).map_or(0.0, |x| *x);
    let para_2 = paras.get(2).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA26".to_string(),
        value: AttrValue::AttrString(format!("{}X{}X{}", para_0, para_1, para_2)).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA27".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
    let mut poss = Vec3::ZERO;
    let mut pose = Vec3::ZERO;
    let mut distance = 0.0;
    if let Ok(Some((poss_q, pose_q))) =
        get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await
    {
        poss = poss_q;
        pose = pose_q;
        distance = pose.distance(poss);
    }
    // 判断STRU>DESC是否包含floor来分辨吊架还是支架，吊架取方钢的高点，支架取方钢低点
    let mut pos = Vec3::ZERO;
    // 支架
    if stru_desc.to_lowercase().contains("floor") {
        if poss.z < pose.z {
            let pos = transform.transform_point(poss);
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART4".to_string(),
                value: AttrValue::AttrVec3(pos).into(),
            });
        } else {
            let pos = transform.transform_point(pose);
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART4".to_string(),
                value: AttrValue::AttrVec3(pos).into(),
            });
        }
    } else {
        if poss.z > pose.z {
            let pos = transform.transform_point(poss);
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART4".to_string(),
                value: AttrValue::AttrVec3(pos).into(),
            });
        } else {
            let pos = transform.transform_point(pose);
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART4".to_string(),
                value: AttrValue::AttrVec3(pos).into(),
            });
        }
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrFloat(distance).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA28".to_string(),
        value: AttrValue::AttrFloat(distance).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA29".to_string(),
        value: AttrValue::AttrString("全焊透".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA30".to_string(),
        value: AttrValue::AttrString("".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA31".to_string(),
        value: AttrValue::AttrVec3(pose).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA32".to_string(),
        value: AttrValue::AttrVec3(poss).into(),
    });
    let ori_str = get_ori_angle_str(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA33".to_string(),
        value: AttrValue::AttrString(ori_str).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDA34".to_string(),
        value: AttrValue::AttrString("100mm".to_string()).into(),
    });
    Ok((data_center_attr, desc))
}

/// 获取电气支吊架 型钢数据 gtype 为 beam
async fn get_dq_support_sctn_gtype_beam_data(
    refno: &EleTreeNode,
    room_map: &HashMap<RefU64, String>,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let database = aios_mgr.get_arango_db().await?;
    let mut data_center_attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_string()).into(),
    });
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(refno.refno, &vec!["STRU"]);
    // 往上找到STRU的NAME
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        owner_name = owner_attr.get_name().unwrap_or("".to_string());
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(owner_name).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("方钢".to_string()).into(),
    });

    let transform = aios_mgr.get_world_transform_or_default(refno.refno).await;
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
        attribute_model_code: "PARTD2".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("方钢".to_string()).into(),
    });
    let spre_attr = aios_mgr.get_foreign_attrmap(refno.refno, "SPRE").unwrap_or_default();
    let spre_name = spre_attr.get_name().unwrap_or("".to_string());
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();

    let spre_name_first = spre_name_split.first().unwrap_or(&"").to_string();
    let spre_name_last = spre_name_split.last().unwrap_or(&"").to_string();
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name_first).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let room_code = room_map.get(&refno.refno).map_or("".to_string(), |x| x.to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD14".to_string(),
        value: AttrValue::AttrString(room_code).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
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
        let spre_attr = aios_mgr.get_foreign_attrmap(fitt, "SPRE").unwrap_or_default();
        Some(spre_attr.get_name().unwrap_or("".to_string()))
    } else {
        None
    };
    match fitt_spre_name.clone() {
        Some(s) if s.contains("Z2") => {
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART26".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("异型钢/异型钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
        }
        Some(s) if s.contains("Z3") => {
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("方钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART26".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("槽钢/方钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("槽钢".to_string()).into(),
            });
        }
        _ => {
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART3".to_string(),
                value: AttrValue::AttrString("槽钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PART26".to_string(),
                value: AttrValue::AttrString("异型钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB29".to_string(),
                value: AttrValue::AttrString("槽钢/槽钢".to_string()).into(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDB30".to_string(),
                value: AttrValue::AttrString("槽钢/槽钢".to_string()).into(),
            });
        }
    }

    let mut poss = Vec3::ZERO;
    let mut pose = Vec3::ZERO;
    if let Ok(Some((poss_q, pose_q))) =
        get_refno_world_poss_pose(refno.refno, &refno.noun, &database, aios_mgr).await
    {
        poss = poss_q;
        pose = pose_q;
    }

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB27".to_string(),
        value: AttrValue::AttrString(spre_name_last.clone()).into(),
    });


    let fitt_spre_name_split = if let Some(fitt_spre_name) = fitt_spre_name {
        fitt_spre_name
            .split("-")
            .collect::<Vec<_>>()
            .last()
            .unwrap_or(&"")
            .to_string()
    } else {
        spre_name_last
    };
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB33".to_string(),
        value: AttrValue::AttrString(fitt_spre_name_split.to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB35".to_string(),
        value: AttrValue::AttrString("连续角焊".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB36".to_string(),
        value: AttrValue::AttrVec3(poss).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB37".to_string(),
        value: AttrValue::AttrVec3(pose).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB38".to_string(),
        value: AttrValue::AttrString("".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDB39".to_string(),
        value: AttrValue::AttrString("不锈钢".to_string()).into(),
    });
    let bran_refno = aios_mgr
        .query_around_owner_within_radius(refno.refno, true, None, true, &["BRAN"])
        .await
        .unwrap_or(vec![]);
    let mut ftub = None;
    if !bran_refno.is_empty() {
        let bran_refno = bran_refno[0];
        let bran_attr = aios_mgr.get_attr(bran_refno).await.unwrap_or_default();
        let bran_name = bran_attr.get_name().unwrap_or("".to_string());

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
                data_center_attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDB40".to_string(),
                    value: AttrValue::AttrString(spacing).into(),
                });
                let connection_method = get_tray_connection_method(paras[0] as f32);
                data_center_attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDB41".to_string(),
                    value: AttrValue::AttrString(connection_method).into(),
                });
                data_center_attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDB42".to_string(),
                    value: AttrValue::AttrString(bran_name).into(),
                });
            }
        }
    }
    if bran_refno.is_empty() || ftub.is_none() {
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB40".to_string(),
            value: AttrValue::AttrString("".to_string()).into(),
        });
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB41".to_string(),
            value: AttrValue::AttrString("".to_string()).into(),
        });
        data_center_attr.push(DataCenterAttr {
            attribute_model_code: "PARTDB42".to_string(),
            value: AttrValue::AttrString("".to_string()).into(),
        });
    }
    Ok((data_center_attr, desc))
}

async fn get_dq_support_sctn_spre_s10_data(
    refno: &EleTreeNode,
    room_map: &HashMap<RefU64, String>,
    spre_name: &str,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let mut data_center_attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_string()).into(),
    });
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(refno.refno, &vec!["STRU"]);
    // 往上找到STRU的NAME
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        owner_name = owner_attr.get_name().unwrap_or("".to_string());
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(owner_name).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("Z形铁".to_string()).into(),
    });

    let transform = aios_mgr.get_world_transform_or_default(refno.refno).await;
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
        attribute_model_code: "PARTD2".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("Z形铁".to_string()).into(),
    });
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();
    let spre_name_first = spre_name_split.first().unwrap_or(&"").to_string();

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name_first).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let room_code = room_map.get(&refno.refno).map_or("".to_string(), |x| x.to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD14".to_string(),
        value: AttrValue::AttrString(room_code).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    let catr_attr = aios_mgr.get_cat_attmap(refno.refno).unwrap_or_default();
    // PARA1xPARA1xPARA2
    let paras = catr_attr.get_f32_vec("PARA").unwrap_or(vec![]);
    let para_0 = paras.get(0).map_or(0.0, |x| *x);
    let para_1 = paras.get(1).map_or(0.0, |x| *x);
    let para_2 = paras.get(2).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDD26".to_string(),
        value: AttrValue::AttrString(format!("{}X{}X{}", para_0, para_1, para_2)).into(),
    });
    Ok((data_center_attr, desc))
}

async fn get_dq_support_sctn_spre_s11_data(
    refno: &EleTreeNode,
    room_map: &HashMap<RefU64, String>,
    spre_name: &str,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(Vec<DataCenterAttr>, String)> {
    let mut data_center_attr = Vec::new();
    let desc = get_refno_desc(refno.refno, aios_mgr)
        .await
        .unwrap_or("".to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART1".to_string(),
        value: AttrValue::AttrString(refno.refno.to_string()).into(),
    });
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(refno.refno, &vec!["STRU"]);
    // 往上找到STRU的NAME
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        owner_name = owner_attr.get_name().unwrap_or("".to_string());
    }
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART2".to_string(),
        value: AttrValue::AttrString(owner_name).into(),
    });

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("固定桥".to_string()).into(),
    });

    let transform = aios_mgr.get_world_transform_or_default(refno.refno).await;
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
        attribute_model_code: "PARTD2".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("固定桥".to_string()).into(),
    });
    let spre_name_split = spre_name.split("-").collect::<Vec<_>>();
    let spre_name_first = spre_name_split.first().unwrap_or(&"").to_string();

    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name_first).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let room_code = room_map.get(&refno.refno).map_or("".to_string(), |x| x.to_string());
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD14".to_string(),
        value: AttrValue::AttrString(room_code).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: AttrValue::AttrString(desc.clone()).into(),
    });
    // PARA1xPARA1xPARA2
    let paras = get_refno_paras(refno.refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = paras.get(0).map_or(0.0, |x| *x);
    let para_3 = paras.get(2).map_or(0.0, |x| *x);
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDC26".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
    data_center_attr.push(DataCenterAttr {
        attribute_model_code: "PARTDC27".to_string(),
        value: AttrValue::AttrFloat(para_3 as f32).into(),
    });
    Ok((data_center_attr, desc))
}

#[serde_as]
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EleNodeWithSpreName {
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
        .map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_string()))
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
    let refno = RefU64::from_str("24383/96911").unwrap();
    let result = mgr
        .query_around_owner_within_radius(refno, true, None, true, &["BRAN"])
        .await
        .unwrap();
    dbg!(&result);
}

#[tokio::test]
async fn test_get_dq_support_sctn_data() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let refnos = vec![RefU64::from_str("24383/86099").unwrap()];
    let result = get_dq_support_sctn_data(refnos, &aios_mgr, vec![]).await?;
    let mut file = std::fs::File::create("data_center_test/PARTDA_PARTDB_PARTDK.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}

#[test]
fn test_checked_add() {
    let a: u8 = u8::MAX - 1;
    let b = a.checked_add(1);
    dbg!(&b);
}