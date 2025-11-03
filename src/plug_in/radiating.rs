use std::collections::{HashMap, HashSet};
use std::io::Read;
use aios_core::{AttrVal, get_uda_info};
use aios_core::options::DbOption;
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::*;
use aios_core::prim_geo::tubing::TubiSize;
use arangors_lite::AqlQuery;
use bitvec::macros::internal::funty::Floating;
use glam::Vec3;
use crate::aql_api::children::{query_children_eles, query_children_order_aql, query_children_refnos, query_children_with_name_aql, query_refnos_from_names_fulltext, query_room_belong_site_name};
use crate::aql_api::tubi::query_tubi_from_bran;
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_ELES_COLLECTION, AQL_PDMS_INST_GEO_COLLECTION, AQL_PDMS_INST_INFO_COLLECTION};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use serde::{Serialize, Deserialize};
use aios_core::pdms_types::ser_refno_as_str;
use aios_core::pdms_types::de_refno_from_key_str;
use dashmap::DashMap;
use nom::Parser;
use once_cell::sync::OnceCell;
use crate::api::element::query_id_from_name;
use crate::aql_api::pdms_element::query_id_from_names_aql;
use crate::aql_api::pdms_room::{get_room_code_from_attr, query_bran_through_rooms_aql, query_room_code_from_owner, query_room_codes_from_owners, query_room_name_from_refno_aql};
use crate::aql_api::PdmsRoomNameAql;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct HeatDissipationData {
    #[serde(deserialize_with = "de_refno_from_key_str")]
    #[serde(serialize_with = "ser_refno_as_str")]
    pub refno: RefU64,
    pub att_type: String,
    pub bore: f32,
    pub length: f32,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct GetPipeHeatDissipationRequest {
    pub pipe: String,
    pub temp: f32,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct GetPipeHeatDissipationResponse {
    pub pipe: String,
    pub temp: f32,
    pub bran: String,
    pub room: String,
    pub heat: f32,
}

/// 获取 pipe下的bran中，每个bran的散热量
pub async fn get_pipe_heat_dissipation(requests: Vec<GetPipeHeatDissipationRequest>, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<GetPipeHeatDissipationResponse>> {
    let database = aios_mgr.get_arango_db().await?;
    let request = requests
        .into_iter()
        .map(|r| (format!("/{}", r.pipe), r.temp))
        .collect::<HashMap<String, f32>>();
    let names = request.keys().map(|name| name.clone()).collect::<Vec<_>>();
    // 通过pipe name 查询到pipe的所有参考后
    // let pipe_refnos = query_id_from_names_aql(names, Some("PIPE"), &database).await?;
    let pipe_refnos = query_refnos_from_names_fulltext(names, &database).await?;
    let mut result = Vec::new();
    for (_name, pipe_ele) in pipe_refnos {
    // for pipe_ele in pipe_refnos {
        let Some(temp) = request.get(&pipe_ele.name) else { continue; };
        let Ok(bran_refnos) = query_children_with_name_aql(&database, pipe_ele.refno).await else { continue; };
        // 一次查询 pipe下所有bran穿过的房间
        let bran = bran_refnos.iter().map(|r| r.0).collect::<Vec<RefU64>>();
        let Ok(room_map) = query_room_codes_from_owners(bran, &database).await else { continue; };
        let room_map = room_map.into_iter()
            .filter(|x| RefU64::from_str(&x.refno).is_some())
            .map(|x| (RefU64::from_str(&x.refno).unwrap(), x))
            .collect::<HashMap<RefU64, PdmsRoomNameAql>>();
        // 查询不到房间号的，通过uda来查询,并判断他是否为反应堆厂房
        let mut uda_room_map = HashMap::new();
        let without_room_bran_refnos = bran_refnos.clone().into_iter().filter(|x| room_map.contains_key(&x.0)).collect::<Vec<_>>();
        for refno in without_room_bran_refnos {
            let Ok(room_name) = get_room_code_from_attr(refno.0, &aios_mgr).await else { continue; };
            if room_name.is_empty() { continue; };
            uda_room_map.entry(refno.0).or_insert(room_name);
        }
        let uda_room_refnos = uda_room_map.values().map(|x| x.to_string()).collect::<HashSet<String>>()
            .into_iter().collect::<Vec<String>>();
        // 找到房间号在哪个site下面
        let uda_room_site_map = query_room_belong_site_name(uda_room_refnos, &database).await?;
        let uda_room_site_map = uda_room_site_map.into_iter()
            .map(|x| (x.name, x.owner_name))
            .collect::<HashMap<String, String>>();
        // 计算每个bran的散热量
        for (bran, bran_name) in bran_refnos {
            let room_code = room_map.get(&bran);
            let room_code = if room_code.is_some() {
                room_code.unwrap().clone()
            } else {
                if let Some(room) = uda_room_map.get(&bran) {
                    if let Some(site) = uda_room_site_map.get(room) {
                        PdmsRoomNameAql {
                            refno: bran.to_string(),
                            room_name: room.to_string(),
                            b_rs: site.contains("RS"),
                        }
                    } else {
                        PdmsRoomNameAql {
                            refno: bran.to_string(),
                            room_name: room.to_string(),
                            b_rs: false,
                        }
                    }
                } else {
                    PdmsRoomNameAql::default()
                }
            };
            let area = get_heat_dissipation_data(bran, &database, aios_mgr).await?;
            let heat = get_heat_dissipation_table(*temp, area, room_code.b_rs) as f32 / 1000.0;
            result.push(GetPipeHeatDissipationResponse {
                pipe: if pipe_ele.name.starts_with("/") { pipe_ele.name[1..].to_string() } else { pipe_ele.name.clone() },
                temp: *temp,
                bran: if bran_name.starts_with("/") { bran_name[1..].to_string() } else { bran_name },
                room: room_code.room_name,
                heat,
            });
        }
    }
    Ok(result)
}

/// 返回整个bran的散热面积
pub async fn get_heat_dissipation_data(bran_refno: RefU64, database: &ArDatabase, aios_mgr: &AiosDBManager) -> anyhow::Result<f32> {
    let mut length_map = Vec::new();
    let bran_children = query_children_order_aql(database, bran_refno).await?;
    // 查询tubi的数据,收集改bran下的不同外径的尺寸
    let mut bore_size = Vec::new();
    let tubis = query_tubi_from_bran(bran_refno, database).await?;
    // 查询保温层厚度
    let iparas = aios_mgr.query_ipara_from_bran(bran_refno).await?;
    let ipara = *iparas.get(0).unwrap_or(&0.0) as f32;
    for tubi in &tubis {
        // 只考虑工艺管道
        match &tubi.tubi_size {
            TubiSize::BoreSize(data) => {
                let Some(from_refno) = RefU64::from_str(&tubi._from) else { continue; };
                length_map.push(HeatDissipationData {
                    refno: from_refno,
                    att_type: "TUBI".to_string(),
                    bore: *data + ipara,
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
            "ELBO" | "BEND" | "VALV" => {
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
                let bore = if bore_size.is_empty() || bore_idx >= bore_size.len() { arrive_point.pbore } else { bore_size[bore_idx] };
                let length = arrive_distance + leave_distance;
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore: bore + ipara,
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
                let bore = if bore_size.is_empty() || bore_idx >= bore_size.len() { first_point.pbore } else { bore_size[bore_idx] };
                let length = first_length + second_length + third_length;
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore: bore + ipara,
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
                let mut bore = if bore_size.is_empty() || bore_idx >= bore_size.len() || idx == points_len - 1 {
                    leave_point.pbore
                } else {
                    bore_size[bore_idx]
                };
                let length = arrive_point.pt.distance(leave_point.pt);
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore: bore + ipara,
                    length,
                });
            }
            _ => {
                let Ok(attr) = aios_mgr.get_attr(point.refno).await else { continue; };
                let Some(AttrVal::IntegerType(arrive)) = attr.get_val("ARRI") else { continue; };
                let Some(AttrVal::IntegerType(leave)) = attr.get_val("LEAV") else { continue; };
                let Some(arrive_point) = point.ptset_map.get(arrive) else { continue; };
                let Some(leave_point) = point.ptset_map.get(leave) else { continue; };
                let bore = if bore_size.is_empty() || bore_idx >= bore_size.len() { leave_point.pbore } else { bore_size[bore_idx] };
                let length = arrive_point.pt.distance(leave_point.pt);
                length_map.push(HeatDissipationData {
                    refno: point.refno,
                    att_type: point.att_type.clone(),
                    bore: bore + ipara,
                    length,
                });
            }
        }
    }
    // 计算整个bran的面积
    let mut area = 0.0;
    let mut total_length = 0.0;
    for length_data in length_map {
        total_length += length_data.length;
        area += length_data.bore * f32::PI * length_data.length
    }
    Ok(area)
}

/// 查询bran下面所有元件的点集(除去atta)
async fn query_bran_point_map(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<InstPointMap>> {
    let id = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, bran_refno.to_string());
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

/// 根据温度和面积计算散热量(提供的表格)
fn get_heat_dissipation_table(heat: f32, area: f32, b_reactor: bool) -> f64 {
    // 换算成 m²
    let area = area as f64 / 1000000.0;
    if b_reactor {
        if heat >= 60.0 && heat < 120.0 {
            return area * 58.0;
        } else if heat >= 120.0 && heat < 350.0 {
            return area * 87.0;
        }
    } else {
        if heat >= 60.0 && heat <= 200.0 {
            return area * 93.0;
        } else if heat > 200.0 {
            return area * 139.0;
        }
    }
    0.0
}

fn get_outside_diameter_from_bore(bore: f32) -> Option<f32> {
    None
}

#[tokio::test]
async fn test_get_heat_dissipation_data() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let bran_refno = RefU64::from_str("24383/66521").unwrap();
    get_heat_dissipation_data(bran_refno, &database, &aios_mgr).await?;
    Ok(())
}

#[test]
fn test_() {
    let result = get_uda_info().clone();
    dbg!(&result.0);
}

// #[tokio::test]
// async fn test_query_hole_model_data_by_key() -> anyhow::Result<()> {
//     let aios_mgr = AiosDBManager::init_form_config().await?;
//     let database = aios_mgr.get_arango_db().await?;
//     let keys = vec!["bca176a3-a8cf-4e1f-b21e-50ac7f56ab5d11".to_string(),"bca176a3-a8cf-4e1f-b21e-50ac7f56ab5d13".to_string()];
//     if let Ok(Some(result)) = query_hole_model_data_by_key(&database,keys).await{
//         dbg!(&result);
//     }
//     Ok(())
// }
//
