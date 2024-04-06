use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::SUL_DB;
use crate::fast_model::GeoTransQuery;

pub async fn apply_insts_boolean_manifold(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    let sql = r#"
        select
             in as refno,
             world_trans.d as wt,
             aabb.d as aabb,
            (select value [meta::id(out), trans.d] from out->geo_relate) as ts,
            (select value [in, world_trans.d, (select value [meta::id(out), geo_type, trans.d] from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"])]
        from array::flatten(neg_refnos->inst_relate)) as neg_ts from inst_relate where !bad_bool and !booled and neg_refnos!=none and aabb.d!=none
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let boolean_query: Vec<GeoTransQuery> = response.take(0)?;
    dbg!(boolean_query.len());

    let mut tasks = Vec::new();
    let chunk = (boolean_query.len() / 16).max(1);
    for chunk in boolean_query.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let task = tokio::spawn(async move {
            let mut update_sql = String::new();
            for mut b in group {
                let Some((pos_id, pos_t)) = b.ts.pop() else {
                    continue;
                };
                //没有实体的情况，下次就不要再继续计算布尔运算了
                let Ok(mesh) = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", pos_id)) else {
                    update_sql.push_str(&format!(
                        "update inst_relate set bad_bool=true where in=pe:{};",
                        b.refno
                    ));
                    continue;
                };
                // dbg!(b.refno);
                let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();
                let pos_matrix = pos_t.compute_matrix().as_dmat4();
                let pos_manifold: ManifoldRust = (&mesh, &pos_matrix).into();
                dbg!(pos_manifold.num_tri());


                let mut neg_manifolds = vec![];
                for (refno, neg_t, negs) in b.neg_ts.into_iter() {
                    for (neg_id, geo_type, t) in negs {
                        if let Ok(mesh) = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", neg_id)) {
                            let m = inverse_mat
                                * neg_t.compute_matrix().as_dmat4()
                                * t.compute_matrix().as_dmat4();

                            neg_manifolds.push((&mesh, &m).into());
                        }
                    }
                }
                // dbg!(neg_shapes.len());
                if !neg_manifolds.is_empty() {
                    let mut success = false;
                    let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
                    let mesh = PlantMesh::from(&final_manifold);
                    mesh.export_obj(false, &format!("{}.obj", b.refno));
                    //保存到文件到dir下
                    if mesh
                        .ser_to_file(&dir_clone.join(format!("{}.mesh", b.refno)))
                        .is_ok() {
                        update_sql.push_str(&format!(
                            "update inst_relate set booled=true where in=pe:{};",
                            b.refno
                        ));
                        success = true;
                    }

                    if !success {
                        update_sql.push_str(&format!(
                            "update inst_relate set bad_bool=true where in=pe:{};",
                            b.refno
                        ));
                    }
                }
                // dbg!(&update_sql);
            }
            SUL_DB.query(update_sql).await;
        });
        tasks.push(task);
    }
    // dbg!(tasks.len());
    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }
    Ok(())
}
