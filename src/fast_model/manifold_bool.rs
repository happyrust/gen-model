use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::SUL_DB;
use glam::DMat4;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use crate::fast_model::{CataNegGroup, GeoTransQuery, GmGeoData, NegInfo};

#[inline]
fn load_mesh(id: &str) -> anyhow::Result<PlantMesh> {
    let mesh = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", id))?;
    Ok(mesh)
}

#[inline]
fn load_manifold(id: &str, mat: DMat4) -> anyhow::Result<ManifoldRust> {
    let mesh = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", id))?;
    let manifold: ManifoldRust = (&mesh, &mat).into();
    Ok(manifold)
}

pub async fn apply_insts_boolean_manifold(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    //暂时只处理FLOOR的情况
    //and in.noun in ["FLOOR"]
    let sql = r#"
        select
             in as refno,
             world_trans.d as wt,
             aabb.d as aabb,
            (select value [meta::id(out), trans.d] from out->geo_relate) as ts,
            (select value [in, world_trans.d, (select meta::id(out) as id, geo_type, trans.d as trans,
             out.aabb.d as aabb, object::keys(out.param)[0] as para_type
            from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"])]
        from array::flatten(neg_refnos->inst_relate)) as neg_ts from inst_relate where !bad_bool
        and !booled and neg_refnos!=none and aabb.d!=none
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let boolean_query: Vec<GeoTransQuery> = response.take(0)?;
    dbg!(boolean_query.len());

    let mut tasks = Vec::new();
    let chunk = (boolean_query.len() / 16).max(1);
    //排除有NREV的情况，因为NREV的布尔计算不是很准，还要判断这个NREV的包围盒和实体的包围盒是否差不多大
    for chunk in boolean_query.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let task = tokio::spawn(async move {
            let mut update_sql = String::new();
            for mut b in group {
                let mut pos_manifolds = vec![];
                for (pos_id, pos_t) in b.ts.iter() {
                    if let Ok(manifold) = load_manifold(pos_id, pos_t.compute_matrix().as_dmat4()) {
                        pos_manifolds.push(manifold);
                    }
                }
                let pos_aabb = b.aabb;
                let z_len = pos_aabb.extents().z as f64;
                //没有实体的情况，下次就不要再继续计算布尔运算了
                if pos_manifolds.is_empty() {
                    update_sql.push_str(&format!(
                        "update inst_relate set bad_bool=true where in=pe:{};",
                        b.refno
                    ));
                    continue;
                };
                // dbg!(b.refno);
                let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();
                let mut pos_manifold = ManifoldRust::batch_boolean(&pos_manifolds, 0);
                // dbg!(pos_manifold.num_tri());
                if pos_manifold.num_tri() == 0 {
                    update_sql.push_str(&format!(
                        "update inst_relate set bad_bool=true where in=pe:{};",
                        b.refno
                    ));
                    continue;
                };

                let mut neg_manifolds = vec![];
                for (refno, mut neg_t, negs) in b.neg_ts.into_iter() {
                    for NegInfo { id, geo_type, para_type, trans, aabb } in negs {
                        let Some(mut neg_aabb) = aabb else {
                            continue;
                        };
                        if para_type == "PrimRevolution" || para_type == "PrimRTorus"{
                            // dbg!("Found NREV, if aabb is similar, need use scale x, y");
                            dbg!("Found NREV, NRTO, use occ");
                            // //如果选装的点在包围盒里，就需要放大？？
                            // neg_t.scale.x *= 1.01;
                            // neg_t.scale.y *= 1.01;
                            //交给OCC处理
                            return;
                        }
                        //看类型给偏差？todo 解决误差的问题
                        if para_type == "PrimExtrusion" || para_type.contains("Cylinder") || para_type == "PrimBox"{
                            // neg_t.translation.z -= 0.0005 * neg_t.scale.z;
                            if neg_aabb.extents().z == 0.0 {
                                continue;
                            }
                            let d = (z_len + 1.0) / z_len;
                            // let scale_z = (d / neg_aabb.extents().z as f64).min(1.02);
                            let scale_z= d.min(1.02);
                            // dbg!(scale_z);
                            // neg_t.scale.z *= (scale_z as f32);
                            neg_t.scale.z *= scale_z as f32;
                        }
                        let m = inverse_mat
                            * neg_t.compute_matrix().as_dmat4()
                            * trans.compute_matrix().as_dmat4();
                        if let Ok(manifold) = load_manifold(&id, m) {
                            neg_manifolds.push(manifold);
                        }
                    }
                }
                // dbg!(neg_shapes.len());
                if !neg_manifolds.is_empty() {
                    let mut success = false;
                    let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
                    let mesh = PlantMesh::from(&final_manifold);
                    #[cfg(debug_assertions)]
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
            match SUL_DB.query(update_sql).await {
                Ok(_) => {}
                Err(e) => {
                    dbg!(e);
                }
            }
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

//处理元件库有负实体的布尔运算
pub async fn apply_cata_neg_boolean_manifold(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }

    let sql = r#"
        select in as refno, (->inst_info)[0] as inst_info_id, (select value array::flatten([geom_refno, cata_neg])
        from ->inst_info->geo_relate where visible and !out.bad and cata_neg!=none) as boolean_group from inst_relate where (->inst_info)[0]!=none and has_cata_neg and !bad_bool and !booled
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let mut params: Vec<CataNegGroup> = response.take(0)?;
    if params.is_empty() {
        return Ok(());
    }

    let mut tasks = Vec::new();
    let chunk = (params.len() / 16).max(1);
    // let chunk = params.len();
    for chunk in params.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let task = tokio::spawn(async move {
            for g in group {
                let pes = g
                    .boolean_group
                    .iter()
                    .flatten()
                    .map(|x| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",");
                // dbg!(g.refno);
                let sql = format!(
                    r#"
                    select meta::id(out) as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
                    from {}->inst_relate->inst_info->geo_relate
                    where visible and !out.bad and geom_refno in [{}]  and out.aabb!=none and out.param!=none"#,
                    g.refno.to_pe_key(),
                    pes
                );
                // dbg!(&sql);
                let Ok(mut resp) = SUL_DB.query(&sql).await else {
                    continue;
                };
                // let gms: Vec<GmGeoData> = resp.take(0).unwrap();
                let Ok(gms) = resp.take::<Vec<GmGeoData>>(0) else {
                    dbg!(&sql);
                    continue;
                };
                // dbg!(&gms);

                let mut update_sql = String::new();
                for bg in g.boolean_group {
                    let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };

                    let Ok(mut pos_manifold) = load_manifold(&pos.id, pos.trans.compute_matrix().as_dmat4()) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };

                    let mut neg_manifolds = vec![];
                    for &neg in bg.iter().skip(1) {
                        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                            continue;
                        };
                        let m = neg_geo.trans.compute_matrix().as_dmat4();
                        if let Ok(manifold) = load_manifold(&neg_geo.id, m) {
                            neg_manifolds.push(manifold);
                        }
                    }
                    if !neg_manifolds.is_empty() {
                        // for neg_shape in neg_shapes {
                        let new_id = g.refno.hash_with_another_refno(bg[0]);
                        let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
                        let mesh = PlantMesh::from(&final_manifold);
                        #[cfg(debug_assertions)]
                        mesh.export_obj(false, &format!("{}.obj", g.refno));
                        //保存到文件到dir下
                        if mesh
                            .ser_to_file(&dir_clone.join(format!("{}.mesh", new_id)))
                            .is_ok()
                        {
                            update_sql.push_str(&format!(
                                "create inst_geo:⟨{}⟩ set meshed = true, aabb = {}, visible = true;",
                                new_id, &pos.aabb_id
                            ));
                            // 有索引的关系，所以geom_refno需要点变化
                            update_sql.push_str(&format!(
                                "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:{}, geo_type='Pos', trans=trans:⟨0⟩;",
                                &g.inst_info_id,
                                new_id,
                                format!("{}_b", bg[0]),
                            ));
                            update_sql.push_str(&format!(
                                "update {}<-inst_relate set booled=true;",
                                &g.inst_info_id,
                            ));
                        }
                    }
                }
                if !update_sql.is_empty() {
                    SUL_DB.query(update_sql).await.unwrap();
                }
            }
        });
        tasks.push(task);
    }
    dbg!(tasks.len());
    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }

    Ok(())
}
