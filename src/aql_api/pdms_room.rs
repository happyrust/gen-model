use std::collections::{HashMap, HashSet};
use aios_core::pdms_types::{PdmsElement, RefU64, UdaMajorType};
use bb8_arangodb::arangors_lite::AqlQuery;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Pool, Row};
use crate::api::attr::get_site_major_from_uda;
use crate::aql_api::children::{query_ancestor_name_of_type_aql, query_deep_children_refnos_fuzzy};
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_ROOM_EDGES_COLLECTION, AQL_ROOM_ELES_COLLECTION};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::graph_db::pdms_arango::*;
use aios_core::pdms_types::*;
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use itertools::Itertools;
use nalgebra::Point3;
use parry3d::math::Vector;
use parry3d::query::{Ray, RayCast};
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use crate::rvm::data_api::query_rvm_geo_instance_aql;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomData {
    pub refno: RefU64,
    pub name: String,
    pub aabb: Option<Aabb>,
    pub target_refnos: Vec<RefU64>,
}

///Room元素
#[derive(Debug, Serialize, Deserialize)]
pub struct RoomElement {
    #[serde(serialize_with = "ser_refno_as_key_str")]
    #[serde(deserialize_with = "de_refno_from_key_str")]
    #[serde(rename = "_key")]
    pub refno: RefU64,
    ///room名称
    pub name: String,
    ///room的aabb
    pub aabb: Option<Aabb>,
    ///room的panels
    pub panels: Vec<RoomPanelElement>,
}

//提前缓存，经常需要使用到的
///房间panel的信息, panel 的owner就是房间节点
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoomPanelElement {
    #[serde(serialize_with = "ser_refno_as_key_str")]
    #[serde(deserialize_with = "de_refno_from_key_str")]
    pub refno: RefU64,
    ///对应的aabb
    pub aabb: Aabb,
    ///对应的几何体
    pub inst_geo: EleInstGeo,
    ///对应的方位
    pub transform: Transform,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoomEdge {
    pub _key: String,
    pub _from: String,
    pub _to: String,
    pub major: UdaMajorType,
}

/// 房间对应的信息
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomInfo {
    // 厂房
    pub factory: String,
    // 层位
    pub leave: i32,
    // 房间名
    pub room_name: String,
}

/// 将房间信息保存到图数据库
impl AiosDBManager {
    pub(crate) async fn save_room_info_to_arangodb(&self,
                                                   room_map: HashMap<RefU64, (Aabb, Vec<RefU64>)>,
                                                   room_panels_map: HashMap<RefU64, Vec<RoomPanelElement>>) -> anyhow::Result<bool> {
        let mut room_eles = vec![];
        let mut room_edges_json = vec![];
        for (refno, (aabb, target_refnos)) in room_map {
            let Ok(frmw) = self.get_ancestor_refno_of_type_data(refno, "FRMW") else {
                continue;
            };
            let mut frmw_name = self.get_name(frmw).await?.to_string();
            let name = frmw_name.split('-').last().unwrap_or_default().to_string();
            dbg!(&name);
            let panels_info = room_panels_map.get(&refno).cloned().unwrap_or_default();
            room_eles.push(RoomElement {
                refno,
                name,
                aabb: Some(aabb),
                panels: panels_info,
            });
            for target_refno in target_refnos {
                // 获取 target_refno 属于哪个专业
                let Ok(site) = self.get_ancestor_refno_of_type_data(target_refno, "SITE") else {
                    continue;
                };
                let mut major = UdaMajorType::NULL;
                if let Some((_, pool)) = self.get_project_pool_by_refno(site).await {
                    if let Some(major_uda) = get_site_major_from_uda(site, &pool).await {
                        major = major_uda;
                    }
                }
                let hash = refno.hash_with_another_refno(target_refno);
                room_edges_json.push(RoomEdge {
                    _key: hash.to_string(),
                    _from: format!("room_eles/{}", refno.to_url_refno()),
                    _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", target_refno.to_url_refno()),
                    major,
                })
            }
        }
        let database = self.get_arango_db().await?;
        let replace = self.db_option.replace_dbs;
        let room_eles_json = serde_json::to_value(&room_eles)?;
        save_arangodb_doc(room_eles_json, "room_eles", &database, replace).await?;
        let room_edges_json = serde_json::to_value(&room_edges_json)?;
        save_arangodb_doc(room_edges_json, "room_edges", &database, replace).await?;
        Ok(true)
    }
}

/// 获取所有需要计算的房间号
pub async fn query_all_need_compute_room_refno(dbno: &Vec<i32>,
                                               room_type: &str,
                                               filter_name: Option<&str>,
                                               pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut refnos = vec![];
    let sql = gen_query_all_need_compute_room_refno_sql(dbno, room_type, filter_name);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for result in results {
        refnos.push((RefU64(result.get::<i64, _>("ID") as u64), result.get::<String, _>("NAME")));
    }
    Ok(refnos)
}

/// 传入参考号 返回该参考号所在的房间
pub async fn query_room_name_from_refno_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<String>> {
    let refno = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    With @@pdms_eles,@@room_edges,@@room_eles
    for v,e in 1 inbound @id @@room_edges
         return v.name
    ")
        .bind_var("id", refno)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION)
        .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION);
    let result = database.aql_query::<String>(aql).await?;
    if !result.is_empty() {
        Ok(Some(result[0].to_string()))
    } else {
        Ok(None)
    }
}

// 传入参考号集合 返回该参考号所在的房间
pub async fn query_room_name_from_refnos_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsNodeBelongRoomName>> {
    let refnos = refnos.into_iter().map(|refno| format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno())).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    With @@pdms_eles,@@room_edges
    for id in @refnos
    for v,e in 1 inbound id @@room_edges
         return {
            'refno': v._key,
            'room_name': v.name
         }
    ").bind_var("refnos", refnos)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION);
    let result = database.aql_query::<PdmsNodeBelongRoomName>(aql).await;
    match result {
        Ok(data) => {
            Ok(data)
        }
        Err(_) => {
            Ok(vec![])
        }
    }
}

/// 获取节点连接的两边的房间
pub async fn query_node_connect_rooms(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<(String, String)>> {
    todo!()
}

/// 获取该参考号属于哪个房间 room_name_type : 存放房间名的类型
pub async fn query_room_info_from_refno(refno: RefU64, room_name_type: &str, database: &ArDatabase) -> anyhow::Result<Option<String>> {
    let refno = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    With @@pdms_eles,@@room_edges
    let refno = (for v,e in 1 inbound @id @@room_edges
                return v._key )[0]
    return refno").bind_var("id", refno)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION);
    let result = database.aql_query::<String>(aql).await;
    return match result {
        Ok(r) => {
            if !r.is_empty() {
                let room_refno = RefU64::from_url_refno(&r[0]);
                if room_refno.is_none() { return Ok(None); }
                let room_refno = room_refno.unwrap();
                let room_name = query_ancestor_name_of_type_aql(database, room_refno, room_name_type).await?;
                if room_name.is_none() { return Ok(None); }
                let room_name = room_name.unwrap();
                Ok(Some(room_name))
            } else {
                Ok(None)
            }
        }
        Err(_) => {
            Ok(None)
        }
    };
}

fn gen_query_all_need_compute_room_refno_sql(dbnos: &Vec<i32>, room_type: &str, filter_name: Option<&str>) -> String {
    let mut sql = String::new();

    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}'", room_type));

    if !dbnos.is_empty() {
        let mut dbno_str = String::new();
        for dbno in dbnos {
            dbno_str.push_str(&format!("{} ,", dbno.to_string()));
        }

        dbno_str.remove(dbno_str.len() - 1);
        sql.push_str(&format!("AND NUMBDB IN ({})", dbno_str))
    }

    if filter_name.is_some() {
        sql.push_str(&format!("AND NAME LIKE '%{}%'", filter_name.unwrap()))
    }
    sql
}

/// 查找房间下的所有元件的参考号
pub async fn query_room_refnos_aql(refno: RefU64, filter_major: Option<UdaMajorType>, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if filter_major.is_none() {
        AqlQuery::new("
        With @@pdms_eles,@@pdms_edges
        for e in 0..10 inbound @key @@pdms_edges
            // filter e.noun == 'PANE'
            for v in 1 outbound CONCAT('room_eles/',e._key) room_edges
                filter v != null
                return v._key
        ")
            .bind_var("key", key)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_ROOM_EDGES_COLLECTION)
    } else {
        let filter_data = filter_major.unwrap().to_major_str();
        AqlQuery::new("
        With @@pdms_eles,@@room_edges
        for v,e in 1 outbound @key @@room_edges
            filter v != null
            filter filter_major == e.major
            return v._key
        ").bind_var("key", key)
            .bind_var("filter_major", filter_data)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION)
    };
    let result: Vec<String> = database.aql_query(aql).await?;
    Ok(convert_refno_vec_from_vec_string(result))
}

/// 查找房间集合下的所有元件的参考号
pub async fn query_rooms_refnos_aql(rooms: Vec<String>, database: &ArDatabase) -> anyhow::Result<Vec<RoomNodes>> {
    let aql = AqlQuery::new("
    With @@room_eles
    for room in @@room_eles
    filter room.name in @rooms
    for v,e in 1 outbound room._id @@room_edges
         return {
            'refno': v._key,
            'room_name': room.name,
         }
    ")
        .bind_var("rooms", rooms)
        .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION)
        .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION);
    let result = database.aql_query::<PdmsNodeBelongRoomName>(aql).await;
    match result {
        Ok(datas) => {
            let mut result_map = HashMap::new();
            for data in datas {
                result_map.entry(data.room_name).or_insert_with(Vec::new).push(data.refno.to_refno_string());
            }
            Ok(result_map.into_iter().map(|data| RoomNodes {
                room_name: data.0,
                nodes: data.1,
            }).collect())
        }
        Err(e) => {
            Ok(vec![])
        }
    }
}

/// 查找房间下的所有元件的 pdms_element
pub async fn query_room_pdms_elements_aql(refno: RefU64, filter_major: Option<UdaMajorType>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = if filter_major.is_none() {
        AqlQuery::new("
        With @@pdms_eles,@@pdms_edges,@@room_eles,@@room_edges
        for e in 0..10 inbound @key @@pdms_edges
            filter e.noun == 'PANE'
            for v in 1 outbound CONCAT('room_eles/',e._key) @@room_edges
                filter v != null
                return { refno:v._key , owner:v.owner , name:v.name,noun:v.noun,version:0,children_count:0 }
        ").bind_var("key", key)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
            .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION)
            .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION)
    } else {
        let filter_data = filter_major.unwrap().to_major_str();
        AqlQuery::new("
        With @@pdms_eles,@@pdms_edges,@@room_eles,@@room_edges
        for p in 0..10 inbound @key @@pdms_edges
            filter p.noun == 'PANE'
            for v,e in 1 outbound CONCAT('room_eles/',p._key) room_edges
                filter v != null
                filter @filter_major == e.major
                return { _key:v._key , owner:v.owner , name:v.name,noun:v.noun,version:0,children_count:0 }
        ").bind_var("key", key)
            .bind_var("filter_major", filter_data)
            .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
            .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION)
            .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION)
    };
    let results: Vec<PdmsElement> = database.aql_query(aql).await.unwrap();
    Ok(results)
}

/// 通过命名规则获取房间名
pub fn get_room_name_split(name: &str) -> Option<RoomInfo> {
    let room_split = name.split('-').collect::<Vec<_>>();
    if room_split.len() < 3 { return None; }
    let factory = room_split[0].replace("/", "");
    let leave = room_split[1].replace("RM", "").parse().unwrap_or(0);
    let room_name = room_split[2].to_string();
    Some(RoomInfo {
        factory,
        leave,
        room_name,
    })
}

pub async fn query_refno_belong_rooms(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    let mut set = HashSet::new();
    let mut r = Vec::new();
    let id = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    With @@pdms_eles,@@pdms_edges,@@room_eles,@@room_edges
    let elements = ( for v in 0..100 inbound @id @@pdms_edges
                    filter v!= null
                    return v._id )
    let room_refnos = (for element in elements
                        for v in 1 inbound element @@room_edges
                        filter v!= null
                        return v._key )
    for room_refno in room_refnos
        let id = document('pdms_eles',room_refno)._id
        for v in 0..10 outbound id pdms_edges
            filter v!= null
            filter v.noun == 'FRMW'
            return { _key:v._key , owner:0 , name:v.name,noun:v.noun,version:0,children_count:1 }")
        .bind_var("id", id)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@room_eles", AQL_ROOM_ELES_COLLECTION)
        .bind_var("@room_edges", AQL_ROOM_EDGES_COLLECTION);
    let results: Vec<PdmsElement> = database.aql_query(aql).await?;
    for result in results {
        let refno = result.refno;
        if set.contains(&refno) { continue; }
        set.insert(refno);
        r.push(PdmsElement {
            refno,
            owner: result.owner,
            name: result.name,
            noun: result.noun,
            version: 0,
            children_count: 1,
        })
    }
    Ok(r)
}

/// 返回贯穿件穿过的两个房间号
pub async fn query_through_element_room_nums(mgr: &AiosDBManager, refnos: &[RefU64]) -> anyhow::Result<HashMap<RefU64, (String, String)>> {
    let mut own_panels_map = query_through_element_room_panels(mgr, refnos).await?;
    Ok(
        own_panels_map.iter().map(|(refno, (p0, p1))| {
            let room0 = mgr.get_owner(*p0);
            let room1 = mgr.get_owner(*p1);

            let room0_num = mgr.room_info_map.get(&room0).map(|x| x.name.clone()).unwrap_or_default();
            let room1_num = mgr.room_info_map.get(&room1).map(|x| x.name.clone()).unwrap_or_default();
            (*refno, (room0_num, room1_num))
        }).collect()
    )
}

/// 返回贯穿件穿过的两个房间panels
pub async fn query_through_element_room_panels(mgr: &AiosDBManager, refnos: &[RefU64]) -> anyhow::Result<HashMap<RefU64, (RefU64, RefU64)>> {
    let through_children_map: HashMap<RefU64, Vec<RefU64>> = refnos.into_iter()
        .map(|x|
            (*x, (mgr.get_children_from_localdb(*x).unwrap_or_default()).0)
        ).collect();
    let mut res_map = HashMap::new();
    for (through_refno, children) in through_children_map {
        // dbg!(&children);
        if let Ok(mut result) = query_ele_own_room_panels(mgr, &children, Some(through_refno)).await{
            for (k,mut v) in result {
                let r1 = v.pop().unwrap_or_default();
                let r0 = v.pop().unwrap_or_default();
                res_map.insert(k, (r0, r1));
            }
        }
    }
    Ok(res_map)
}

/// 返回元件所属的房间panels
pub async fn query_ele_own_room_panels(mgr: &AiosDBManager, refnos: &[RefU64], as_whole_to: Option<RefU64>) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>> {
    let room_panels_tree = mgr.room_panels_rtree.as_ref().ok_or(anyhow!("房间空间树未生成。"))?;
    //先用包围盒去查询和哪些房间的aabb相交
    let database = mgr.get_arango_db().await?;
    let inst_data = query_insts_shape_data(&database, refnos, &[GeoBasicType::Pos]).await?;
    let is_as_whole = as_whole_to.is_some();
    if inst_data.inst_info_map.is_empty() { return Ok(Default::default()); }
    let mut own_panels_map = HashMap::new();
    let mut whole_key_points = vec![];
    let mut whole_aabb = Aabb::new_invalid();
    if is_as_whole {
        for (&refno, info) in &inst_data.inst_info_map {
            let Some(inst_geos) = inst_data.get_inst_geos(info) else {
                continue;
            };
            let key_points = inst_geos.iter()
                .map(|x| x.geo_param.key_points().into_iter().map(|v| x.transform.transform_point(v)))
                .flatten()
                .map(|x| info.world_transform.transform_point(x))
                .collect::<Vec<_>>();
            whole_key_points.extend_from_slice(&key_points);
            whole_aabb.merge(&info.aabb.unwrap());
        }
        dbg!(&whole_key_points);
        dbg!(&whole_aabb);
        let intersect_room_panels = room_panels_tree.locate_intersecting_bounds(&whole_aabb).collect::<Vec<_>>();
        dbg!(&intersect_room_panels);
        let mut geo_hashes = HashSet::new();
        let mut panel_infos = vec![];
        for (panel_refno, _) in &intersect_room_panels {
            if let Some(panel_info) = mgr.room_panel_info_map.get(panel_refno) {
                geo_hashes.insert(panel_info.inst_geo.geo_hash);
                panel_infos.push(panel_info);
            }
        }
        mgr.cache_plant_meshes(&geo_hashes, false).await?;
        let mut target_panels = vec![];
        for panel_info in panel_infos {
            let Ok(Some(room_panel_mesh)) = mgr.get_plant_mesh(panel_info.inst_geo.geo_hash).await else {
                continue;
            };
            let t = panel_info.transform * panel_info.inst_geo.transform;
            let collider_mesh = room_panel_mesh.get_tri_mesh(t.compute_matrix());
            for key_point in &whole_key_points {
                let contain_point = match collider_mesh.cast_local_ray_and_get_normal(
                    &Ray::new(Point3::from_slice(&key_point.to_array()), Vector::new(0.0, 0.0, 1.0)),
                    100000.0,
                    false,
                ) {
                    Some(intersection) => {
                        collider_mesh.is_backface(intersection.feature)
                    }
                    None => false,
                };
                if contain_point {
                    target_panels.push(panel_info.refno);
                    break;
                }
            }
        }
        own_panels_map.insert(as_whole_to.unwrap(), target_panels);
        return Ok(own_panels_map);
    }

    for (&refno, info) in &inst_data.inst_info_map {
        let Some(inst_geos) = inst_data.get_inst_geos(info) else {
            continue;
        };
        // dbg!(inst_geos.iter().map(|x| &x.geo_param));
        let key_points = inst_geos.iter()
            .map(|x| x.geo_param.key_points().into_iter().map(|v| x.transform.transform_point(v)))
            .flatten()
            .map(|x| info.world_transform.transform_point(x))
            .collect::<Vec<_>>();
        let mut intersect_room_panels = vec![];
        // dbg!(&key_points);
        // dbg!(info.aabb);
        let Some(mut ele_aabb) = info.aabb else {
            continue;
        };
        intersect_room_panels = room_panels_tree.locate_intersecting_bounds(&ele_aabb).collect::<Vec<_>>();
        dbg!(&intersect_room_panels);
        let mut geo_hashes = HashSet::new();
        let mut panel_infos = vec![];
        for (panel_refno, _) in &intersect_room_panels {
            if let Some(panel_info) = mgr.room_panel_info_map.get(panel_refno) {
                geo_hashes.insert(panel_info.inst_geo.geo_hash);
                panel_infos.push(panel_info);
            }
        }
        let plant_mesh = query_pdms_mesh_aql(&database, geo_hashes.iter()).await?;
        dbg!(plant_mesh.meshes.len());
        let mut target_panels = vec![];
        for panel_info in panel_infos {
            let Some(room_panel_mesh) = plant_mesh.get_mesh(panel_info.inst_geo.geo_hash) else {
                continue;
            };
            let t = panel_info.transform * panel_info.inst_geo.transform;
            let collider_mesh = room_panel_mesh.get_tri_mesh(t.compute_matrix());
            for key_point in &key_points {
                let contain_point = match collider_mesh.cast_local_ray_and_get_normal(
                    &Ray::new(Point3::from_slice(&key_point.to_array()), Vector::new(0.0, 0.0, 1.0)),
                    100000.0,
                    false,
                ) {
                    Some(intersection) => {
                        collider_mesh.is_backface(intersection.feature)
                    }
                    None => false,
                };
                if contain_point {
                    target_panels.push(panel_info.refno);
                    break;
                }
            }
        }

        let target_refno = as_whole_to.unwrap_or(refno);
        own_panels_map.insert(target_refno, target_panels);
    }

    Ok(own_panels_map)
}

// 返回贯穿件 穿过的两个房间号 ， tuple.0：距离核岛中心 世界坐标 0，0，0 最近的点
// pub async fn query_through_element_rooms_old(mgr: &AiosDBManager, refnos: &[RefU64]) -> anyhow::Result<Vec<(String, String)>> {
// //获得refno的mesh数据, 然后找到最近和最远的两个顶点，沿着远点到贯穿件的方向
// let database = mgr.get_arango_db().await?;
// let inst_data = query_insts_shape_data(&database, refnos).await?;
//
// // dbg!(&instances);
// for (_, info) in &inst_data.inst_info_map {
//     let Some(inst_geos) = inst_data.get_inst_geos(info) else {
//         continue;
//     };
//     let refno = info.refno;
//     let Some(w_trans) = mgr.get_world_transform(refno).await? else {
//         continue;
//     };
//     //z 方向可以不考虑，去算最近最远点
//     let w_pos = w_trans.translation;
//
//     dbg!(inst_geos.len());
//     let target_geos = inst_geos.iter()
//         .filter(|x| x.geo_type == GeoBasicType::Compound ||
//             x.geo_type == GeoBasicType::Pos);
//     let hashes = target_geos
//         .map(|x| x.geo_hash)
//         .collect::<Vec<_>>();
//     let ele_mesh_mgr = query_pdms_mesh_aql(&database, hashes.iter()).await.unwrap_or_default();
//     for (&hash, geo) in hashes.iter().zip(inst_geos) {
//         let t = info.get_geo_world_transform(geo);
//         if let Some(ele_mesh) = ele_mesh_mgr.get_mesh(hash) {
//             dbg!(ele_mesh.vertices.len());
//         }
//     }
//     // let r = self.calculate_room(info, inst_geos, rtree).await?;
//     // final_within_room_refnos.extend_from_slice(&r);
// }
//
// //获取所有的room的包围盒
//
// //获得room的所有mesh数据
//
//
// Ok(vec!(("R532".to_string(), "R320".to_string())))
// }