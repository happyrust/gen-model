use crate::fast_model::{CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo};
use aios_core::csg::manifold::ManifoldRust;
use aios_core::error::{init_deserialize_error, init_query_error};
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{get_inst_relate_keys, RefU64, SUL_DB};
use anyhow::anyhow;
use glam::DMat4;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[inline]
fn load_mesh(id: &str) -> anyhow::Result<PlantMesh> {
    let mesh = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", id))?;
    Ok(mesh)
}

#[inline]
fn load_manifold(dir: &PathBuf, id: &str, mat: DMat4) -> anyhow::Result<ManifoldRust> {
    let mesh = PlantMesh::des_mesh_file(&dir.join(format!("{}.mesh", id)))?;
    let manifold: ManifoldRust = (&mesh, &mat).into();
    Ok(manifold)
}

//处理元件库有负实体的布尔运算
pub async fn apply_cata_neg_boolean_manifold(
    refnos: &[RefU64],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);

    let mut sql = format!(
        r#" select in as refno, (->inst_info)[0] as inst_info_id, (select value array::flatten([geom_refno, cata_neg])
            from ->inst_info->geo_relate where visible and !out.bad and cata_neg!=none) as boolean_group
            from {inst_keys} where (->inst_info)[0]!=none and has_cata_neg "#
    );

    if !replace_exist {
        sql.push_str("and !bad_bool and !booled");
    }

    // println!("sql is {}", &sql);
    let mut response = SUL_DB.query(sql).await?;
    let mut params: Vec<CataNegGroup> = response.take(0)?;
    // dbg!(&params);
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
                //
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

                    let Ok(mut pos_manifold) =
                        load_manifold(&dir_clone, &pos.id, pos.trans.compute_matrix().as_dmat4())
                    else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };

                    // dbg!(&update_sql);
                    let mut neg_manifolds = vec![];
                    for &neg in bg.iter().skip(1) {
                        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                            continue;
                        };
                        let m = neg_geo.trans.compute_matrix().as_dmat4();
                        if let Ok(manifold) = load_manifold(&dir_clone, &neg_geo.id, m) {
                            neg_manifolds.push(manifold);
                        }
                    }
                    if !neg_manifolds.is_empty() {
                        let new_id = g.refno.hash_with_another_refno(bg[0]);
                        let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
                        let mesh = PlantMesh::from(&final_manifold);
                        #[cfg(feature = "debug_model")]
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
                                "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:⟨{}⟩, geo_type='Pos', trans=trans:⟨0⟩;",
                                &g.inst_info_id,
                                new_id,
                                format!("{}_b", bg[0]),
                            ));
                            update_sql.push_str(&format!(
                                "update {}<-inst_relate set booled=true;",
                                &g.inst_info_id,
                            ));
                            // dbg!(&update_sql);
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
    // dbg!(tasks.len());
    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }

    Ok(())
}

pub async fn apply_insts_boolean_manifold(
    refnos: &[RefU64],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);
    // let mut remain_refnos = vec![];
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    let mut sql = format!(
        r#" select
                in as refno,
                in.noun as noun,
                world_trans.d as wt,
                aabb.d as aabb,
                (select value [meta::id(out), trans.d] from out->geo_relate) as ts,
                (select value [in, world_trans.d, (select meta::id(out) as id, geo_type, trans.d as trans,
                out.aabb.d as aabb, object::keys(out.param)[0] as para_type
                from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"] and out.param != NONE)]
            from array::flatten(in<-neg_relate.in->inst_relate) ) as neg_ts from {} where !bad_bool
            and (in<-neg_relate)[0] != none and aabb.d != NONE
        "#,
        inst_keys
    );
    if !replace_exist {
        sql.push_str(" and !booled");
    }
    match SUL_DB.query(&sql).await {
        Ok(mut response) => {
            match response.take::<Vec<ManiGeoTransQuery>>(0) {
                Ok(boolean_query) => {
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
                                    if let Ok(manifold) =
                                        load_manifold(&dir_clone, pos_id, pos_t.compute_matrix().as_dmat4())
                                    {
                                        pos_manifolds.push(manifold);
                                    }
                                }
                                let pos_aabb = b.aabb;
                                let pos_extent = pos_aabb.extents();
                                //没有实体的情况，下次就不要再继续计算布尔运算了
                                let inst_relate_id = b.refno.to_table_key("inst_relate");
                                if pos_manifolds.is_empty() {
                                    update_sql.push_str(&format!(
                                        "update {} set bad_bool=true;",
                                        &inst_relate_id
                                    ));
                                    continue;
                                };
                                let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();
                                let mut pos_manifold =
                                    ManifoldRust::batch_boolean(&pos_manifolds, 0);
                                if pos_manifold.num_tri() == 0 {
                                    update_sql.push_str(&format!(
                                        "update {} set bad_bool=true;",
                                        &inst_relate_id
                                    ));
                                    continue;
                                };
                                #[cfg(feature = "debug_model")]
                                {
                                    let pos_mesh = PlantMesh::from(&pos_manifold);
                                    pos_mesh.export_obj(false, "pos_t.obj").unwrap();
                                }

                                let mut neg_manifolds = vec![];
                                let mut found_need_occ = false;
                                for (refno, mut neg_t, negs) in b.neg_ts.into_iter() {
                                    for NegInfo {
                                        id,
                                        geo_type,
                                        para_type,
                                        trans,
                                        aabb,
                                    } in negs
                                    {
                                        // dbg!(&b.noun);
                                        let Some(mut neg_aabb) = aabb else {
                                            continue;
                                        };
                                        // 什么情况下该使用OCC的布尔运算？
                                        // dbg!((refno, neg_aabb.extents().xy() , pos_aabb.extents().xy()));
                                        let neg_max = neg_aabb.extents().xy().max();
                                        let pos_max = pos_aabb.extents().xy().max();
                                        //一个模糊的条件，如果aabb的尺寸比较接近，最好就应该移交给OCC去处理！！
                                        //这里暂时扩大一下xy的缩放，这样缩放会导致切割的会不太准确
                                        if para_type == "PrimRevolution"
                                            || para_type == "PrimRTorus"
                                        {
                                            if neg_max / pos_max > 0.9 {
                                                found_need_occ = true;
                                                continue;
                                                // let scale_xy = 1.02;
                                                // neg_t.scale.x *= scale_xy;
                                                // neg_t.scale.y *= scale_xy;
                                            }
                                        }

                                        //如果有关键点在包围盒上了，就需要做缩放
                                        // let intersect = pos_aabb.intersects(&neg_aabb);
                                        // if para_type == "PrimRevolution" || para_type == "PrimRTorus" {
                                        //     //如果选装的点在包围盒里，就需要放大？？
                                        //     let m = pos_extent.x.max(pos_extent.y) as f64;
                                        //     let d = (m + 1.0) / m;
                                        //     let scale_xy = d.clamp(1.01, 1.02) as f32;
                                        //     //
                                        //     let scale_xy = 1.02;
                                        //     let nxy_max = neg_aabb.extents().xy().max();
                                        //     let pxy_max = pos_aabb.extents().xy().max();
                                        //     let sim_dist = (nxy_max - pxy_max).abs();
                                        //     dbg!((neg_aabb.extents().xy() , pos_aabb.extents().xy()));
                                        //     dbg!(sim_dist/pxy_max);
                                        //     if sim_dist < 10.0 /*|| b.noun == "FLOOR"*/ {
                                        //         // dbg!((neg_aabb.extents().xy() - pos_aabb.extents().xy()));
                                        //         //交给occ 去处理
                                        //         // return;
                                        //         neg_t.scale.x *= scale_xy;
                                        //         neg_t.scale.y *= scale_xy;
                                        //         dbg!(scale_xy);
                                        //     }
                                        // }
                                        //看类型给偏差？todo 解决误差的问题
                                        //如果AABB 比较接近的情况下，又有旋转体

                                        // dbg!(&b.noun);
                                        if b.noun == "FLOOR"
                                            || b.noun.contains("WALL")
                                            || b.noun == "GENSEC"
                                            || b.noun == "PANE"
                                        {
                                            if neg_aabb.extents().z == 0.0 {
                                                continue;
                                            }
                                            let d =
                                                (pos_extent.z as f64 + 1.0) / pos_extent.z as f64;
                                            let scale_z = d.min(1.02);
                                            neg_t.scale.z *= scale_z as f32;
                                        }

                                        let m = inverse_mat
                                            * neg_t.compute_matrix().as_dmat4()
                                            * trans.compute_matrix().as_dmat4();
                                        if let Ok(manifold) = load_manifold(&dir_clone, &id, m) {
                                            #[cfg(feature = "debug_model")]
                                            {
                                                let neg_mesh = PlantMesh::from(&manifold);
                                                neg_mesh
                                                    .export_obj(false, &format!("{}_t.obj", &id))
                                                    .unwrap();
                                            }
                                            neg_manifolds.push(manifold);
                                        }
                                    }
                                }
                                // dbg!(found_need_occ);
                                //直接交给OCC去处理精确的计算
                                if found_need_occ {
                                    continue;
                                }

                                if !neg_manifolds.is_empty() {
                                    let mut success = false;
                                    let final_manifold =
                                        pos_manifold.batch_boolean_subtract(&neg_manifolds);
                                    let mesh = PlantMesh::from(&final_manifold);
                                    // dbg!(neg_manifolds.len());
                                    #[cfg(feature = "debug_model")]
                                    mesh.export_obj(false, &format!("{}.obj", b.refno));
                                    //保存到文件到dir下
                                    if mesh
                                        .ser_to_file(&dir_clone.join(format!("{}.mesh", b.refno)))
                                        .is_ok()
                                    {
                                        update_sql.push_str(&format!(
                                            "update {} set booled=true;",
                                            &inst_relate_id
                                        ));
                                        success = true;
                                    }

                                    if !success {
                                        update_sql.push_str(&format!(
                                            "update {} set bad_bool=true;",
                                            &inst_relate_id
                                        ));
                                    }
                                }
                                // dbg!(&update_sql);
                            }
                            if !update_sql.is_empty() {
                                match SUL_DB.query(update_sql).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(e);
                                    }
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
                }
                Err(e) => {
                    init_deserialize_error(
                        "Vec<ManiGeoTransQuery>",
                        &e,
                        &sql,
                        &std::panic::Location::caller().to_string(),
                    );
                    return Err(anyhow!(e.to_string()));
                }
            }
        }
        Err(e) => {
            init_query_error(&sql, &e, &std::panic::Location::caller().to_string());
            return Err(anyhow!(e.to_string()));
        }
    }
    Ok(())
}
