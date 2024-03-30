use std::collections::{HashMap, HashSet};
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::test::test_surreal::init_test_surreal;
use bevy_transform::components::Transform;
use dashmap::DashSet;
use glam::Vec3;
use itertools::Itertools;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;
use parry3d::math::Isometry;
use parry3d::query::PointQuery;
use parry3d::shape::TriMesh;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use crate::fast_model::process_gen_meshes;

#[tokio::test]
pub async fn test_cal_rooms() -> anyhow::Result<()> {
    init_test_surreal().await;
    let refno = "17496/200186".into();
    process_gen_meshes(Some(&["24383/66662".into(), "17496/200186".into()]))
        .await
        .unwrap();
    load_aabb_tree().await.unwrap();
    build_room_relations().await.unwrap();
    let within_refnos = query_room_refnos(refno).await.unwrap();
    dbg!(&within_refnos);
    Ok(())
}

pub async fn build_room_relations() -> anyhow::Result<()> {
    let room_panel_map = build_room_panels_relate().await.unwrap();
    for (room_refno, room_num, panel_refnos) in room_panel_map {
        for panel_refno in panel_refnos {
            let refnos = query_room_refnos(panel_refno)
                .await
                .unwrap();
            if !refnos.is_empty() {
                dbg!(refnos.len());
                save_room_relate(panel_refno, &refnos, &room_num).await.unwrap();
            }
        }
    }
    Ok(())
}

async fn save_room_relate(panel_refno: RefU64, within_refnos: &HashSet<RefU64>, room_num: &str) -> anyhow::Result<()> {
    let mut final_sql = "".to_string();
    for refno in within_refnos {
        let sql = format!(
            "relate {}->room_relate->{} set room_num='{}';",
            panel_refno.to_pe_key(), refno.to_pe_key(), room_num
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
    for (room_refno, room_num, panel_refnos) in &result{
        let sql = format!(
            "relate {}->room_panel_relate->[{}] set room_num='{}';",
            room_refno.to_pe_key(), panel_refnos.iter().map(|x| x.to_pe_key()).join(","),
            room_num
        );
        sql_string.push_str(&sql);
    }
    SUL_DB.query(sql_string).await?;

    Ok(result)
}



pub async fn query_room_refnos(panel_refno: RefU64) -> anyhow::Result<HashSet<RefU64>> {

    //查询到aabb直接完全在这个房间里的mesh里，就不用做点的检查
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno]).await.unwrap_or_default();
    if geom_insts.is_empty() {
        return Ok(Default::default());
    }
    // println!("geom insts len: {}", geom_insts.len());

    let mut within_refnos = HashSet::new();
    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            let Ok(mesh) = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", inst.geo_hash)) else {
                continue;
            };
            let mut tri_mesh: TriMesh = mesh.get_tri_mesh((geom_inst.world_trans * inst.transform).compute_matrix());
            let mut contains_query = GLOBAL_AABB_TREE.read().await.locate_intersecting_bounds(&geom_inst.world_aabb).collect::<Vec<_>>();
            let mut need_check_refnos = vec![];
            contains_query.retain(|(refno, bbox)| {
                //filter the wrong aabb
                if *refno == panel_refno || (bbox.mins[0] > 1000000.0) {
                    return false;
                }
                // dbg!(&bbox);
                let contains: Vec<bool> =
                    bbox.vertices().iter().map(|x| tri_mesh.contains_point(&Isometry::identity(), &x)).collect::<Vec<_>>();
                //每一个点都在mesh里面
                if contains.iter().all(|&x| x){
                    return true;
                }else {
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
            if !need_check_refnos.is_empty() {
                // dbg!(panel_refno);
                // dbg!(&within_refnos);
                // dbg!(&need_check_refnos);
                //首先判断，如果是包围盒完全不在里面，直接跳过
                //继续的点检查可能会比较耗时，后续应该加开关，让用户判断是否需要继续做检查
                let pes = need_check_refnos.iter().map(|x| x.to_pe_key()).join(",");
                let mut repsonse = SUL_DB.query(format!(
                    r#"select
                         in.id as refno, world_trans.d as world_trans,
                         (select value [trans.d, ->inst_geo.pts.d] from ->inst_info->geo_relate) as pts_group
                       from array::flatten([{}]->inst_relate)
                    "#,
                    pes)).await?;
                let geom_pts: Vec<GeomPtsQuery> = repsonse.take(0)?;
                // dbg!(&geom_pts);
                let mut intersect_set = DashSet::new();
                geom_pts.par_iter().for_each(|g| {
                    if g.pts_group.par_iter().find_any(|(trans, pts)|{
                        let pt_trans = g.world_trans * (*trans);
                        pts.par_iter().find_any(|&pt|
                            tri_mesh.contains_point(&Isometry::identity(), &pt_trans.transform_point(*pt).into())
                        ).is_some()
                    }).is_some() {
                        // dbg!(g.refno);
                        intersect_set.insert(g.refno);
                    }
                });
                if !intersect_set.is_empty() {
                    println!("found intersect room panel {}, the are refnos: {}", panel_refno,
                             &intersect_set.iter().map(|x| x.to_string()).join(","));
                }
                within_refnos.extend(intersect_set);
                // dbg!(&within_refnos);
            }
        }
    }

    Ok(within_refnos)
}