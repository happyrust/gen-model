use std::collections::HashMap;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use arangors_lite::AqlQuery;
use bevy_transform::prelude::Transform;
use glam::Vec3;
use parry2d::utils::Array1;
use crate::aql_api::children::query_ancestor_till_types_aql;
use crate::aql_api::pdms_room::query_room_name_from_refnos_aql;
use crate::consts::{AQL_FOREIGN_EDGES_COLLECTION, AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION};
use crate::data_center_api::data_api::{get_refno_desc, get_refno_desi_desc, get_refno_latest_version, get_refno_paras};
use crate::data_center_api::electric_major::sctn::EleNodeWithSpreName;
use crate::data_center_api::electric_major::stru::get_dq_jldatu_fixing_data;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

// 圆板类
pub async fn get_dq_fixing_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<DataCenterInstance>> {
    let database = aios_mgr.get_arango_db().await?;
    let mut result = Vec::new();
    let fixings = query_dq_circular_plate(refnos, &database).await.unwrap_or(vec![]);
    let fixing_refnos = fixings.iter().map(|x| x.refno).collect::<Vec<RefU64>>();
    let room_name = query_room_name_from_refnos_aql(fixing_refnos, &database).await.unwrap_or(vec![]);
    let room_map = room_name
        .into_iter()
        .map(|x| (x.refno, x.room_name))
        .collect::<HashMap<RefU64, String>>();

    for fixing in fixings {
        let mut fixing_attrs = Vec::new();
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PART1".to_string(),
            value: AttrValue::AttrString(fixing.refno.to_refno_str()).into(),
        });
        let owner_refno = aios_mgr.get_ancestor_refno_till_type(fixing.refno, &vec!["STRU"]);
        // 往上找到STRU的NAME
        let mut owner_name = "".to_string();
        if let Some(owner_refno) = owner_refno {
            let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
            owner_name = owner_attr.get_name().unwrap_or("".to_string());
        }
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PART2".to_string(),
            value: AttrValue::AttrString(owner_name).into(),
        });
        let transform = aios_mgr.get_world_transform(fixing.refno).unwrap_or(None).unwrap_or(Transform::default());
        let pos = transform.translation;
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PART4".to_string(),
            value: AttrValue::AttrVec3(pos).into(),
        });
        let attr = aios_mgr.get_attr(fixing.refno).await.unwrap_or_default();
        let ori = attr.get_vec3("ORI").unwrap_or(Vec3::ZERO);
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PART5".to_string(),
            value: AttrValue::AttrVec3(ori).into(),
        });

        let spre_name = fixing.spre_name.clone();
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTD4".to_string(),
            value: AttrValue::AttrString(spre_name.clone()).into(),
        });
        let room_name = room_map.get(&fixing.refno).map_or("".to_string(), |x| x.to_string());
        fixing_attrs.push(DataCenterAttr {
            attribute_model_code: "PARTD14".to_string(),
            value: AttrValue::AttrString(room_name).into(),
        });
        let mut object_code = "".to_string();
        match spre_name {
            s if s.contains("JT3") => {
                get_dq_finxing_jt_3(&fixing, aios_mgr, &database, &mut fixing_attrs, false).await;
                object_code = "PARTDK".to_string();
            }
            s if s.contains("JT4") => {
                get_dq_finxing_jt_3(&fixing, aios_mgr, &database, &mut fixing_attrs, true).await;
                object_code = "PARTDK".to_string();
            }
            s if s.contains("C1") => {
                get_dq_finxing_c1(&s, &mut fixing_attrs);
                object_code = "PARTDJ".to_string()
            }
            s if s.contains("C2") => {
                get_dq_finxing_c2(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDE".to_string()
            }
            s if s.contains("MGB") => {
                get_dq_finxing_mgb(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDH".to_string()
            }
            s => {
                get_dq_jldatu_fixing_data(fixing.refno, &s, &mut fixing_attrs, aios_mgr);
                object_code = "PARTDG".to_string()
            }
        }
        result.push(DataCenterInstance {
            object_model_code: object_code,
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: fixing.refno.to_refno_str(),
            version: get_refno_latest_version(),
            attributes: fixing_attrs,
        });
    }
    Ok(result)
}

async fn get_dq_finxing_jt_3(fixing: &EleNodeWithSpreName,
                             aios_mgr: &AiosDBManager,
                             database: &ArDatabase,
                             mut fixing_attrs: &mut Vec<DataCenterAttr>, b_jt_4: bool) {
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("圆板".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("圆板".to_string()).into(),
    });
    // 往上找到GENSEC判断Gtype是BOX：2，BEAM：1
    let owner_refno = aios_mgr.get_ancestor_refno_till_type(fixing.refno, &vec!["GENSEC"]);
    let mut owner_name = "".to_string();
    if let Some(owner_refno) = owner_refno {
        let owner_attr = aios_mgr.get_attr(owner_refno).await.unwrap_or_default();
        owner_name = owner_attr.get_str("GTYP").map_or("".to_string(), |x| x.to_string());
    }
    match owner_name.as_str() {
        "BEAM" => {
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("1".to_string()).into(),
            });
        }
        "BOX" => {
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("2".to_string()).into(),
            });
        }
        _ => {
            fixing_attrs.push(DataCenterAttr {
                attribute_model_code: "PARTD6".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
        }
    }
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let desc = get_refno_desc(fixing.refno, &aios_mgr)
        .await
        .unwrap_or("".to_string());
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: desc,
    });
    let mut stru_desc = None;
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
        database,
        fixing.refno,
        vec!["STRU"],
    ).await.unwrap_or(None);
    if let Some(stru) = stru {
        let desc = get_refno_desi_desc(stru.refno, &aios_mgr)
            .await
            .unwrap_or("".to_string());
        stru_desc = Some(desc);
    }
    let mut partdk_3 = 0.0;
    let mut partdk_4 = 0.0;
    //JT3先区分S1-150取2*para4，JT4取para2
    //JT3先区分S1-151取para5
    if b_jt_4 {
        partdk_3 = *paras.get(1).unwrap_or(&0.0) as f32;
    } else {
        if let Some(stru_desc) = &stru_desc {
            match stru_desc {
                s if s.contains("S1-150") => {
                    partdk_3 = 2.0 * *paras.get(3).unwrap_or(&0.0) as f32;
                }
                s if s.contains("S1-151") => {
                    partdk_4 = *paras.get(4).unwrap_or(&0.0) as f32;
                }
                _ => {}
            }
        }
    }
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDK3".to_string(),
        value: AttrValue::AttrFloat(partdk_3).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDK4".to_string(),
        value: AttrValue::AttrFloat(
            partdk_4
        ).into(),
    });
}

fn get_dq_finxing_c1(spre_name: &str,
                     mut fixing_attrs: &mut Vec<DataCenterAttr>) {
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("欧姆卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString("SCTN".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
}

fn get_dq_finxing_c2(refno: RefU64,
                     spre_name: &str,
                     mut fixing_attrs: &mut Vec<DataCenterAttr>,
                     aios_mgr: &AiosDBManager) {
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("管卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("管卡".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
    let paras = get_refno_paras(refno, aios_mgr).unwrap_or(vec![]);
    let para_10 = paras.get(9).map_or(0.0, |x| *x);
    let para_8 = paras.get(7).map_or(0.0, |x| *x);
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDE26".to_string(),
        value: AttrValue::AttrFloat(para_10 as f32).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDE27".to_string(),
        value: AttrValue::AttrFloat(para_8 as f32).into(),
    });
}

fn get_dq_finxing_mgb(refno: RefU64, spre_name: &str,
                      mut fixing_attrs: &mut Vec<DataCenterAttr>, aios_mgr: &AiosDBManager) {
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD6".to_string(),
        value: AttrValue::AttrString("1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("Q355B".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let spre_last = spre_name.split("-").collect::<Vec<&str>>().last().map_or("".to_string(), |x| x.to_string());
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: spre_last,
    });
    let paras = get_refno_paras(refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = paras.get(0).map_or(0.0, |x| *x);
    let para_2 = paras.get(1).map_or(0.0, |x| *x);
    let para_3 = paras.get(2).map_or(0.0, |x| *x);
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDH26".to_string(),
        value: AttrValue::AttrString(format!("{}X{}", para_1, para_2)).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDH27".to_string(),
        value: AttrValue::AttrFloat(para_3 as f32).into(),
    });
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