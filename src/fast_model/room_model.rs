use crate::fast_model::process_meshes_update_db;
use aios_core::room::room::{load_aabb_tree, GLOBAL_AABB_TREE};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::test::test_surreal::init_test_surreal;
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use bevy_transform::components::Transform;
use bevy_transform::TransformPoint;
use dashmap::DashSet;
use glam::Vec3;
use itertools::Itertools;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Isometry;
use parry3d::math::{Point, Real};
use parry3d::query::PointQuery;
use parry3d::shape::{TriMesh, TriMeshFlags};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[tokio::test]
pub async fn test_cal_rooms() -> anyhow::Result<()> {
    init_test_surreal().await;
    let refno = "24381/58346".into();
    process_meshes_update_db(None, (&["24381/34303".into(), refno]))
        .await
        .unwrap();
    load_aabb_tree().await.unwrap();
    build_room_relations().await.unwrap();
    let within_refnos = query_room_refnos(refno, &HashSet::new(), 0.1)
        .await
        .unwrap();
    dbg!(&within_refnos);
    Ok(())
}

//TODO need figure out
#[tokio::test]
pub async fn test_cal_distance() -> anyhow::Result<()> {
    init_test_surreal().await;
    let panel_refno = "24381/34303".into();
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno])
        .await
        .unwrap_or_default();
    // dbg!(&geom_insts);
    if geom_insts.is_empty() {
        return Ok(());
    }

    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            let Ok(mesh) =
                PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", inst.geo_hash))
            else {
                continue;
            };
            let Some(mut tri_mesh) = mesh
                .get_tri_mesh_with_flag(inst.transform.compute_matrix(), TriMeshFlags::ORIENTED)
            else {
                continue;
            };
            dbg!(tri_mesh.indices().len());
            dbg!(tri_mesh.vertices().len());

            dbg!(tri_mesh.local_aabb());

            let point = Vec3::new(8495.01953125, -8.15999984741211, 0.0);
            dbg!(tri_mesh.local_aabb().contains_local_point(&point.into()));
            dbg!(tri_mesh.contains_local_point(&point.into()));

            let mat = (geom_inst.world_trans * inst.transform).compute_matrix();
        }
    }
    return Ok(());
}

pub async fn build_room_relations() -> anyhow::Result<()> {
    let room_panel_map = build_room_panels_relate().await.unwrap();
    let exclude_panel_refnos = room_panel_map
        .iter()
        .map(|(_, _, panel_refnos)| panel_refnos.clone())
        .flatten()
        .collect::<HashSet<_>>();
    // dbg!(exclude_panel_refnos.len());
    for (room_refno, room_num, panel_refnos) in room_panel_map {
        for panel_refno in panel_refnos {
            let refnos = query_room_refnos(panel_refno, &exclude_panel_refnos, 0.1)
                .await
                .unwrap();
            if !refnos.is_empty() {
                // dbg!(refnos.len());
                save_room_relate(panel_refno, &refnos, &room_num)
                    .await
                    .unwrap();
            }
        }
    }
    Ok(())
}

async fn save_room_relate(
    panel_refno: RefU64,
    within_refnos: &HashSet<RefU64>,
    room_num: &str,
) -> anyhow::Result<()> {
    let mut final_sql = "".to_string();
    for refno in within_refnos {
        let sql = format!(
            "relate {}->room_relate->{} set room_num='{}';",
            panel_refno.to_pe_key(),
            refno.to_pe_key(),
            room_num
        );
        final_sql.push_str(&sql);
    }
    // dbg!(&final_sql);
    SUL_DB.query(&final_sql).await?;
    Ok(())
}

async fn build_room_panels_relate() -> anyhow::Result<Vec<(RefU64, String, Vec<RefU64>)>> {
    //属于room的panel
    let sql = r#"
        select value [meta::id(id), array::last(string::split(NAME, '-')),  REFNO<-pe_owner<-pe<-pe_owner<-pe[?noun='PANE'].id] from FRMW where "-RM" in NAME
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let result: Vec<(RefU64, String, Vec<RefU64>)> = response.take(0).unwrap();
    // dbg!(&result);
    let mut sql_string = String::new();
    for (room_refno, room_num, panel_refnos) in &result {
        let sql = format!(
            "relate {}->room_panel_relate->[{}] set room_num='{}';",
            room_refno.to_pe_key(),
            panel_refnos.iter().map(|x| x.to_pe_key()).join(","),
            room_num
        );
        sql_string.push_str(&sql);
    }
    SUL_DB.query(sql_string).await?;

    Ok(result)
}

pub async fn query_room_refnos(
    panel_refno: RefU64,
    exclude_refnos: &HashSet<RefU64>,
    inside_tol: f32,
) -> anyhow::Result<HashSet<RefU64>> {
    //查询到aabb直接完全在这个房间里的mesh里，就不用做点的检查
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno])
        .await
        .unwrap_or_default();
    // dbg!(&geom_insts);
    if geom_insts.is_empty() {
        return Ok(Default::default());
    }

    let mut within_refnos = HashSet::new();
    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            let Ok(mesh) =
                PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", inst.geo_hash))
            else {
                continue;
            };
            let Some(mut tri_mesh) = mesh.get_tri_mesh_with_flag(
                (geom_inst.world_trans * inst.transform).compute_matrix(),
                TriMeshFlags::ORIENTED,
            ) else {
                continue;
            };
            let mut contains_query = GLOBAL_AABB_TREE
                .read()
                .await
                .locate_intersecting_bounds(&geom_inst.world_aabb)
                .collect::<Vec<_>>();
            let mut need_check_refnos = vec![];
            contains_query.retain(|(refno, bbox)| {
                //filter the wrong aabb
                if exclude_refnos.contains(refno) || (bbox.mins[0] > 1000000.0) {
                    return false;
                }
                // dbg!(&bbox);
                let contains: Vec<bool> = bbox
                    .vertices()
                    .iter()
                    .map(|x| tri_mesh.contains_point(&Isometry::identity(), &x))
                    .collect::<Vec<_>>();
                //每一个点都在mesh里面
                if contains.iter().all(|&x| x) {
                    return true;
                } else {
                    //只要有一个点在mesh里面，就需要继续检查是否真的相交
                    if contains.iter().any(|&x| x) {
                        need_check_refnos.push(*refno);
                    }
                    return false;
                }
            });
            //for test
            // dbg!(tri_mesh.contains_point(&Isometry::identity(), &Point::new(0.0, 0.0, 0.0) ));
            // if !contains_query.is_empty() {
            //     dbg!(&contains_query);
            // }
            within_refnos.extend(contains_query.iter().map(|(x, _)| x));
            //todo 先暂时不检查相交的情况
            // if !need_check_refnos.is_empty() {
            //     // dbg!(panel_refno);
            //     // dbg!(&within_refnos);
            //     // dbg!(&need_check_refnos);
            //     //首先判断，如果是包围盒完全不在里面，直接跳过
            //     //继续的点检查可能会比较耗时，后续应该加开关，让用户判断是否需要继续做检查
            //     let pes = need_check_refnos.iter().map(|x| x.to_pe_key()).join(",");
            //     let mut repsonse = SUL_DB.query(format!(
            //         r#"select
            //              in.id as refno, world_trans.d as world_trans, aabb.d as world_aabb,
            //              (select value [trans.d, ->inst_geo[?pts!=none].pts[?d!=none].d] from ->inst_info->geo_relate) as pts_group
            //            from array::flatten([{}]->inst_relate)  where !booled
            //         "#,
            //         pes)).await?;
            //     let geom_pts: Vec<GeomPtsQuery> = repsonse.take(0)?;
            //     // dbg!(&geom_pts);
            //     let mut intersect_set = DashSet::new();
            //     geom_pts.par_iter().for_each(|g| {
            //         if g.pts_group
            //             .par_iter()
            //             .find_any(|(trans, o_pts)| {
            //                 if let Some(pts) = o_pts {
            //                     let pt_trans = g.world_trans * (*trans);
            //                     pts.par_iter()
            //                         .find_any(|&pt| {
            //                             tri_mesh.contains_point(
            //                                 &Isometry::identity(),
            //                                 &pt_trans.transform_point(*pt).into(),
            //                             )
            //                         })
            //                         .is_some()
            //                 } else {
            //                     false
            //                 }
            //             })
            //             .is_some()
            //         {
            //             // dbg!(g.refno);
            //             intersect_set.insert(g.refno);
            //         }
            //     });
            //     if !intersect_set.is_empty() {
            //         println!(
            //             "found intersect room panel {}, refnos: {}",
            //             panel_refno,
            //             &intersect_set.iter().map(|x| x.to_string()).join(",")
            //         );
            //     }
            //     within_refnos.extend(intersect_set);
            //     // dbg!(&within_refnos);
            // }
            //
        }
    }

    Ok(within_refnos)
}
