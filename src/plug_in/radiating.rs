use std::collections::HashMap;
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::{AttrVal, RefU64};
use aios_core::prim_geo::tubing::TubiSize;
use arangors_lite::AqlQuery;
use bitvec::macros::internal::funty::Floating;
use glam::Vec3;
use crate::aql_api::children::query_children_order_aql;
use crate::aql_api::tubi::query_tubi_from_bran;
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_PDMS_INST_GEO_COLLECTION, AQL_PDMS_INST_INFO_COLLECTION};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use serde::{Serialize, Deserialize};
use aios_core::pdms_types::ser_refno_as_str;
use aios_core::pdms_types::de_refno_from_key_str;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct HeatDissipationData {
    #[serde(deserialize_with = "de_refno_from_key_str")]
    #[serde(serialize_with = "ser_refno_as_str")]
    pub refno: RefU64,
    pub att_type: String,
    pub bore: f32,
    pub length: f32,
}

/// 返回散热量信息
pub async fn get_heat_dissipation_data(bran_refno: RefU64, database: &ArDatabase, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<HeatDissipationData>> {
    let mut length_map = Vec::new();
    let bran_children = query_children_order_aql(database, bran_refno).await?;
    // 查询tubi的数据,收集改bran下的不同外径的尺寸
    let mut bore_size = Vec::new();
    let tubis = query_tubi_from_bran(bran_refno, database).await?;
    for tubi in &tubis {
        // 只考虑工艺管道
        match &tubi.tubi_size {
            TubiSize::BoreSize(data) => {
                let Some(from_refno) = RefU64::from_arangodb_refno_str(&tubi._from) else { continue; };
                length_map.push(HeatDissipationData {
                    refno: from_refno,
                    att_type: "TUBI".to_string(),
                    bore: *data,
                    length: tubi.start_pt.distance(tubi.end_pt),
                });
                if !bore_size.contains(data) {
                    bore_size.push(*data);
                }
            }
            _ => { continue; }
        }
    }
    // 查询点集,计算每个元件的长度
    let points = query_bran_point_map(bran_refno, database).await?
        .into_iter().map(|point| (point.refno, point)).collect::<HashMap<_, _>>();
    // 方便变径取bore值，每个redu bore_idx +1 ，就取bore_size的下一个值
    let mut bore_idx = 0;
    let points_len = points.len();
    for (idx, element) in bran_children.into_iter().enumerate() {
        if element.noun.as_str() == "ATTA" { continue; };
        let Some(point) = points.get(&element.refno) else { continue; };
        match point.att_type.as_str() {
            "ELBO" | "VALV" => {
                let Ok(attr) = aios_mgr.get_attr(point.refno).await else { continue; };
                let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") else { continue; };
                let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") else { continue; };
                let Some(arrive_point) = point.ptset_map.get(arrive) else { continue; };
                let Some(leave_point) = point.ptset_map.get(leave) else { continue; };
                // arrive 到 0 0 0 的距离
                let arrive_distance = arrive_point.pt.distance(Vec3::ZERO);
                // leave 到 0 0 0 的距离
                let leave_distance = leave_point.pt.distance(Vec3::ZERO);
                // 如果没有tubi就去arrive的 pbore
                let bore = if bore_size.is_empty() && bore_idx >= bore_size.len() { arrive_point.pbore } else { bore_size[bore_idx] };
                let length = arrive_distance + leave_distance;
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore,
                    length,
                });
            }
            "TEE" => {
                if point.ptset_map.len() > 3 { continue; }
                // 三通默认 1 2 3点就是三通的三个点
                let Some(first_point) = point.ptset_map.get(&1) else { continue; };
                let Some(second_point) = point.ptset_map.get(&2) else { continue; };
                let Some(third_point) = point.ptset_map.get(&3) else { continue; };
                let first_length = first_point.pt.distance(Vec3::ZERO);
                let second_length = second_point.pt.distance(Vec3::ZERO);
                let third_length = third_point.pt.distance(Vec3::ZERO);
                let bore = if bore_size.is_empty() && bore_idx >= bore_size.len() { first_point.pbore } else { bore_size[bore_idx] };
                let length = first_length + second_length + third_length;
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore,
                    length,
                });
            }
            "REDU" => {
                let Ok(attr) = aios_mgr.get_attr(point.refno).await else { continue; };
                let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") else { continue; };
                let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") else { continue; };
                let Some(arrive_point) = point.ptset_map.get(arrive) else { continue; };
                let Some(leave_point) = point.ptset_map.get(leave) else { continue; };
                bore_idx += 1;
                // redu 为 bran最后一个元素时 取 leave_point的 pbore
                let mut bore = if (bore_size.is_empty() && bore_idx >= bore_size.len()) || idx == points_len - 1 {
                    leave_point.pbore
                } else {
                    bore_size[bore_idx]
                };
                let length = arrive_point.pt.distance(leave_point.pt);
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore,
                    length,
                });
            }
            _ => {
                let Ok(attr) = aios_mgr.get_attr(point.refno).await else { continue; };
                let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") else { continue; };
                let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") else { continue; };
                let Some(arrive_point) = point.ptset_map.get(arrive) else { continue; };
                let Some(leave_point) = point.ptset_map.get(leave) else { continue; };
                let bore = if bore_size.is_empty() && bore_idx >= bore_size.len() { leave_point.pbore } else { bore_size[bore_idx] };
                let length = arrive_point.pt.distance(leave_point.pt);
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore,
                    length,
                });
            }
        }
    }
    // dbg!(&length_map);
    // 计算整个bran的面积
    // let mut area = 0.0;
    // for length_data in length_map {
    //     area += length_data.bore * f32::PI * length_data.length
    // }
    Ok(length_map)
}

async fn query_bran_point_map(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<InstPointMap>> {
    let id = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, bran_refno.to_url_refno());
    let aql = AqlQuery::new("
    with @@pdms_eles,@@pdms_edges,@@pdms_inst_infos,@@pdms_inst_geos
    for v in 1 inbound @id @@pdms_edges
        filter v.noun != 'ATTA'
        let cata_hash = document(@@pdms_inst_infos,v._key)
        let hash = cata_hash.cata_hash == null ? cata_hash._key : cata_hash.cata_hash
        let geo = document(@@pdms_inst_geos,hash)
        filter geo != null
        return {
        'refno': v._key,
        'att_type': v.noun,
        'ptset_map': geo.ptset_map
        }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION)
        .bind_var("id", id);
    let result = database.aql_query::<InstPointMap>(aql).await?;
    Ok(result)
}

#[tokio::test]
async fn test_get_heat_dissipation_data() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let bran_refno = RefU64::from_refno_str("24383/66521").unwrap();
    get_heat_dissipation_data(bran_refno, &database, &aios_mgr).await?;
    Ok(())
}