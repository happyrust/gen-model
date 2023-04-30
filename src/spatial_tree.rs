use crate::aql_api::pdms_room::*;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::db_number::DbNumMgr;
use aios_core::pdms_types::UdaMajorType::{E, T, V};
use aios_core::pdms_types::*;
use bevy::utils::default;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Point};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use aios_core::options::DbOption;

pub fn tri_tri_intersection() -> bool {
    true
}

///投影到平面上的房间，去计算是否二维有相交
pub async fn compute_rooms_by_projection(
    room_refno: Vec<RefU64>,
    all_insts_mgr: HashMap<u32, ShapeInstancesMgr>,
    collider_shape_mgr: CachedColliderShapeMgr,
    db_option: &DbOption,
) -> anyhow::Result<HashMap<RefU64, (Aabb, Vec<RefU64>)>> {
    // let mut room_info_map = HashMap::new();
    // let mut file = fs::File::open("assets/mesh/mesh.bin")?;
    // let mut data = vec![];
    // file.read_to_end(&mut data)?;
    // let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
    //
    // let instance_dir_path = "assets/instance";
    // let mut file = fs::File::open("accel.spa")?;
    // let mut buf = vec![];
    // file.read_to_end(&mut buf)?;
    // let rtree = bincode::deserialize::<AccelerationTree>(&buf)?;
    // //生成 TriMesh 的 shape
    // let dbno = db_option.arch_db_nums.clone().unwrap_or_default().clone();
    // // let room_infos = vec![(RefU64::from_two_nums(24381, 35031), "R330".to_string())];
    // let dbno_mgr =
    //     DbNumMgr::load_file(&format!("{instance_dir_path}/dbno_mgr.num")).unwrap_or_default();
    // for target_refno in room_refno {
    //     if let Some(dbno) = dbno_mgr.get_dbno(target_refno) {
    //         if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
    //             {
    //                 let all_refnos = inst_mgr.level_shape_mgr.get(&target_refno).unwrap();
    //                 for room_refno in all_refnos.value().clone().into_iter() {
    //                     if room_info_map.contains_key(&room_refno) {
    //                         continue;
    //                     }
    //                     let ele_geos_info = inst_mgr.get_inst_data(room_refno);
    //                     let ele_refno = *ele_geos_info.key();
    //                     let room_colliders =
    //                         collider_shape_mgr.get_collider(ele_refno, inst_mgr, &mesh_mgr);
    //                     if let Some(target_abb) = ele_geos_info.aabb {
    //                         let mut withing_room_refnos =
    //                             vec![RefU64::from_refno_str("24381/109830").unwrap()];
    //                         let mut removed_refnos = vec![];
    //                         withing_room_refnos.retain(|x| {
    //                             //直接判断点集，可以快速过滤一些构件
    //                             if let Some(dbno) = dbno_mgr.get_dbno(*x) {
    //                                 if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
    //                                     let ele_geos_info = inst_mgr.get_inst_data(*x);
    //                                     let mut has_checked = false;
    //                                     let tr = ele_geos_info.get_transform();
    //                                     for pt_kv in &ele_geos_info.ptset_map {
    //                                         let p = tr.transform_point(pt_kv.1.pt);
    //                                         for rc in &room_colliders {
    //                                             if rc.as_ref().contains_point(
    //                                                 &Isometry::identity(),
    //                                                 &Point::new(p.x, p.y, p.z),
    //                                             ) {
    //                                                 return true;
    //                                             };
    //                                         }
    //                                         has_checked = true;
    //                                         let checking_colliders = collider_shape_mgr
    //                                             .get_collider(*x, inst_mgr, &mesh_mgr);
    //                                         for rc in &room_colliders {
    //                                             for cc in &checking_colliders {
    //                                                 let target_pt = if let Some(tri_mesh) =
    //                                                     cc.as_ref().as_trimesh()
    //                                                 {
    //                                                     tri_mesh.triangle(0).local_aabb().center()
    //                                                 } else {
    //                                                     cc.compute_local_aabb().center()
    //                                                 };
    //                                                 if rc.as_ref().contains_point(
    //                                                     &Isometry::identity(),
    //                                                     &target_pt,
    //                                                 ) {
    //                                                     return true;
    //                                                 }
    //                                                 let r = parry3d::query::intersection_test(
    //                                                     &Isometry::identity(),
    //                                                     rc.as_ref(),
    //                                                     &Isometry::identity(),
    //                                                     cc.as_ref(),
    //                                                 )
    //                                                 .unwrap();
    //                                                 if r {
    //                                                     return true;
    //                                                 }
    //                                             }
    //                                         }
    //                                     }
    //                                 }
    //                             }
    //                             removed_refnos.push(*x);
    //                             println!("removed {} refno ;", removed_refnos.len());
    //                             false
    //                         });
    //                         let mut file = fs::File::create("removed_refnos.data").unwrap();
    //                         let serialized = bincode::serialize(&removed_refnos).unwrap();
    //                         file.write_all(serialized.as_slice()).unwrap();
    //                         dbg!(removed_refnos.len());
    //                         dbg!(&withing_room_refnos.len());
    //                         room_info_map
    //                             .entry(room_refno)
    //                             .or_insert((target_abb, withing_room_refnos));
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
    // Ok(room_info_map)
    Ok(Default::default())
}

/// 精算房间计算
/// （1）调用投影算法，投影到2d，进行计算
pub async fn recompute_spatial_tree(
    room_refno: Vec<RefU64>,
    all_insts_mgr: HashMap<u32, ShapeInstancesMgr>,
    collider_shape_mgr: CachedColliderShapeMgr,
    db_option: &DbOption,
) -> anyhow::Result<HashMap<RefU64, (Aabb, Vec<RefU64>)>> {
    // let mut room_info_map = HashMap::new();
    // let mut file = fs::File::open("assets/mesh/mesh.bin")?;
    // let mut data = vec![];
    // file.read_to_end(&mut data)?;
    // let mesh_mgr = bincode::deserialize::<CachedMeshesMgr>(&data)?;
    //
    // let instance_dir_path = "assets/instance";
    // let mut file = fs::File::open("assets/accel.spa")?;
    // let mut buf = vec![];
    // file.read_to_end(&mut buf)?;
    // let rtree = bincode::deserialize::<AccelerationTree>(&buf)?;
    // //生成 TriMesh 的 shape
    // let dbno = db_option.arch_db_nums.clone().unwrap_or_default().clone();
    // // let room_infos = vec![(RefU64::from_two_nums(24381, 35031), "R330".to_string())];
    // let dbno_mgr =
    //     DbNumMgr::load_file(&format!("{instance_dir_path}/dbno_mgr.num")).unwrap_or_default();
    // for target_refno in room_refno {
    //     if let Some(dbno) = dbno_mgr.get_dbno(target_refno) {
    //         if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
    //             if inst_mgr.level_shape_mgr.contains_key(&target_refno) {
    //                 let all_refnos = inst_mgr.level_shape_mgr.get(&target_refno).unwrap();
    //                 for room_refno in all_refnos.value().clone().into_iter() {
    //                     if room_info_map.contains_key(&room_refno) {
    //                         continue;
    //                     }
    //                     let ele_geos_info = inst_mgr.get_inst_data(room_refno);
    //                     {
    //                         //filter None aabb
    //                         let ele_refno = *ele_geos_info.key();
    //                         let room_colliders =
    //                             collider_shape_mgr.get_collider(ele_refno, inst_mgr, &mesh_mgr);
    //                         if let Some(target_abb) = ele_geos_info.aabb {
    //                             let mut withing_room_refnos = rtree
    //                                 .locate_intersecting_bounds(&target_abb)
    //                                 .collect::<Vec<_>>();
    //                             // dbg!(&withing_room_refnos.len());
    //                             // let mut withing_room_refnos = vec![RefU64::from_refno_str("24383/68087").unwrap()];
    //                             // let mut removed_refnos = vec![];
    //                             // withing_room_refnos.retain(|x| {
    //                             //     //直接判断点集，可以快速过滤一些构件
    //                             //     if let Some(dbno) = dbno_mgr.get_dbno(*x) {
    //                             //         if let Some(inst_mgr) = all_insts_mgr.get(&dbno) {
    //                             //             let ele_geos_info = inst_mgr.get_inst_data(*x);
    //                             //             let mut has_checked = false;
    //                             //             {
    //                             //                 let tr = ele_geos_info.get_transform();
    //                             //                 for pt_kv in &ele_geos_info.ptset_map {
    //                             //                     let p = tr.transform_point(pt_kv.1.pt);
    //                             //                     for rc in &room_colliders {
    //                             //                         if rc.as_ref().contains_point(&Isometry::identity(), &Point::new(p.x, p.y, p.z)) {
    //                             //                             return true;
    //                             //                         };
    //                             //                     }
    //                             //                     has_checked = true;
    //                             //                     let checking_colliders = collider_shape_mgr.get_collider(*x, inst_mgr, &mesh_mgr);
    //                             //                     for rc in &room_colliders {
    //                             //                         for cc in &checking_colliders {
    //                             //                             let target_pt = if let Some(tri_mesh) = cc.as_ref().as_trimesh() {
    //                             //                                 tri_mesh.triangle(0).local_aabb().center()
    //                             //                             } else {
    //                             //                                 cc.compute_local_aabb().center()
    //                             //                             };
    //                             //                             if rc.as_ref().contains_point(&Isometry::identity(), &target_pt) {
    //                             //                                 return true;
    //                             //                             }
    //                             //                             let r = parry3d::query::intersection_test(&Isometry::identity(), rc.as_ref(),
    //                             //                                                                       &Isometry::identity(), cc.as_ref()).unwrap();
    //                             //                             if r {
    //                             //                                 return true;
    //                             //                             }
    //                             //                         }
    //                             //                     }
    //                             //                 }
    //                             //             }
    //                             //         }
    //                             //     }
    //                             //     removed_refnos.push(*x);
    //                             //     println!("removed {} refno ;", removed_refnos.len());
    //                             //     false
    //                             // });
    //                             // let mut file = fs::File::create("removed_refnos.data").unwrap();
    //                             // let serialized = bincode::serialize(&removed_refnos).unwrap();
    //                             // file.write_all(serialized.as_slice()).unwrap();
    //                             // dbg!(removed_refnos.len());
    //                             // dbg!(&withing_room_refnos.len());
    //                             room_info_map
    //                                 .entry(room_refno)
    //                                 .or_insert((target_abb, withing_room_refnos));
    //                         }
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
    // Ok(room_info_map)
    Ok(Default::default())
}

#[tokio::test]
async fn test_save_spatial_tree_to_db() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    // pane 的 refno
    let test_room_refno = RefU64::from_two_nums(24381, 35033);

    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let room_contains_refno = vec![
        RefU64::from_two_nums(24384, 3088),
        RefU64::from_two_nums(24384, 3090),
        RefU64::from_two_nums(24381, 71799),
        RefU64::from_two_nums(24384, 3130),
    ];
    let not_contains_refno = vec![
        RefU64::from_two_nums(24381, 35286),
        RefU64::from_two_nums(17496, 106149),
        RefU64::from_two_nums(24381, 110151),
        RefU64::from_two_nums(24381, 101720),
        RefU64::from_two_nums(17496, 100004),
    ];
    let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(E), &database).await?;
    let compute_contains_refno = query_room_refnos_aql(test_room_refno, Some(T), &database).await?;
    // let compute_contains_refno = query_room_refnos_aql(test_room_refno, None,&database).await?;
    // dbg!(&compute_contains_refno.len());
    // for compute_refno in compute_contains_refno {
    //     if not_contains_refno.contains(&compute_refno) {
    //         dbg!(&compute_refno);
    //     }
    // }
    Ok(())
}
