use std::collections::{HashMap, HashSet};
use aios_core::{GeomInstQuery, RefU64, SUL_DB};
use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::test::test_surreal::init_test_surreal;
use itertools::Itertools;
use parry3d::math::Point;
use parry3d::math::Isometry;
use parry3d::query::PointQuery;
use parry3d::shape::TriMesh;
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
            let tri_mesh: TriMesh = mesh.get_tri_mesh((geom_inst.world_trans * inst.transform).compute_matrix());
            let mut contains_query = GLOBAL_AABB_TREE.read().await.locate_intersecting_bounds(&geom_inst.world_aabb).collect::<Vec<_>>();
            let mut need_check_refnos = vec![];
            contains_query.retain(|(refno, bbox)| {
                //filter the wrong aabb
                if *refno == panel_refno || (bbox.mins[0] > 1000000.0) {
                    return false;
                }
                // dbg!(&bbox);
                let out_bound = bbox.vertices().iter().any(|x| !tri_mesh.contains_point(&Isometry::identity(), &x));
                if out_bound {
                    need_check_refnos.push(*refno);
                }
                !out_bound
            });
            //for test
            // dbg!(tri_mesh.contains_point(&Isometry::identity(), &Point::new(0.0, 0.0, 0.0) ));
            // if !contains_query.is_empty() {
            //     dbg!(&contains_query);
            // }
            within_refnos.extend(contains_query.iter().map(|(x, _)| x));
            if !need_check_refnos.is_empty() {
                dbg!(&need_check_refnos);
            }
            // dbg!(&within_refnos);

            //intersect 的需要额外判断
            // let intersect_query = GLOBAL_AABB_TREE.read().await.locate_intersecting_bounds(&geom_inst.world_aabb).collect::<Vec<_>>();
            // // dbg!(&intersect_query);
            // //过滤掉在contains_query里的
            // let bb_intersect = intersect_query.into_iter()
            //     .map(|b| b.0)
            //     .filter(|x| !refnos.contains(x))
            //     .collect::<Vec<_>>();

            // if !bb_intersect.is_empty() {
            //     dbg!(&bb_intersect);
            // }

        }
    }

    Ok(within_refnos)
}