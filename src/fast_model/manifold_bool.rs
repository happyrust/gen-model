use crate::fast_model::{CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo};
use aios_core::csg::manifold::ManifoldRust;
use aios_core::error::{init_deserialize_error, init_query_error};
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{get_inst_relate_keys, init_test_surreal, RefU64, SUL_DB};
use anyhow::anyhow;
use glam::DMat4;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use bevy_transform::prelude::Transform;

#[inline]
fn load_mesh(id: &str) -> anyhow::Result<PlantMesh> {
    let mesh = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", id))?;
    Ok(mesh)
}

#[inline]
fn load_manifold(
    dir: &PathBuf,
    id: &str,
    mat: DMat4,
    more_precision: bool,
) -> anyhow::Result<ManifoldRust> {
    let mesh = PlantMesh::des_mesh_file(&dir.join(format!("{}.mesh", id)))?;
    let manifold = ManifoldRust::convert_to_manifold(mesh, mat, more_precision);
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
            from {inst_keys} where in.id != none and (->inst_info)[0]!=none and has_cata_neg "#
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
    // dbg!(&params);
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
                    select record::id(out) as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
                    from {}->inst_relate->inst_info->geo_relate
                    where !out.bad and geom_refno in [{}]  and out.aabb!=none and out.param!=none"#,
                    g.refno.to_pe_key(),
                    pes
                );
                // println!("geom sql is {}", &sql);
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

                    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                    println!("正在负实体计算的mesh hash: {}", &pos.id);

                    let Ok(mut pos_manifold) = load_manifold(
                        &dir_clone,
                        &pos.id,
                        pos.trans.compute_matrix().as_dmat4(),
                        false,
                    ) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };

                    // dbg!(&update_sql);
                    let mut neg_manifolds = vec![];
                    //负实体的精度要比正实体大
                    for &neg in bg.iter().skip(1) {
                        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                            continue;
                        };
                        let m = neg_geo.trans.compute_matrix().as_dmat4();
                        if let Ok(manifold) = load_manifold(&dir_clone, &neg_geo.id, m, true) {
                            neg_manifolds.push(manifold);
                        }
                    }
                    //没有负实体也要加上为_b后缀，表示已经进行过分析计算了。
                    // if !neg_manifolds.is_empty()
                    {
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
                                "create inst_geo:⟨{}⟩ set meshed = true, aabb = {};",
                                new_id, &pos.aabb_id
                            ));
                            // 有索引的关系，所以geom_refno需要点变化
                            let relate_sql = format!(
                                "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:⟨{}⟩, geo_type='Pos', trans=trans:⟨0⟩, visible = true;",
                                &g.inst_info_id,
                                new_id,
                                format!("{}_b", bg[0]),
                            );
                            // println!("cate neg relate sql is {}", &relate_sql);
                            update_sql.push_str(relate_sql.as_str());
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
    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
    println!("元件库的负实体计算{:?}完成", refnos);
    Ok(())
}

pub async fn apply_insts_boolean_manifold(
    refnos: &[RefU64],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    for refno in refnos {
        apply_insts_boolean_manifold_single(*refno, replace_exist, dir.clone()).await?;
    }
    Ok(())
}

pub async fn apply_insts_boolean_manifold_single(
    refno: RefU64,
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    //排除不在这个范围内的ngrm geom refno
    //这里需要截断传进来的参考号数量
    let mut sql = format!(
        r#"
        select
                in as refno,
                in.noun as noun,
                world_trans.d as wt,
                aabb.d as aabb,
                (select value [record::id(out), trans.d] from out->geo_relate where geo_type in ["Compound", "Pos"] and trans.d != NONE ) as ts,
                (select value [in, world_trans.d,
                    (select record::id(out) as id, geo_type, trans.d as trans, out.aabb.d as aabb
                    from array::flatten(out->geo_relate) where trans.d != NONE and ( geo_type=="Neg" or (geo_type=="CataCrossNeg"
                        and geom_refno in (select value ngmr from pe:{refno}<-ngmr_relate) ) ))]
                        from array::flatten([array::flatten(in<-neg_relate.in->inst_relate), array::flatten(in<-ngmr_relate.in->inst_relate)]) where world_trans.d!=none
                ) as neg_ts
             from inst_relate:{refno} where in.id != none and !bad_bool and ((in<-neg_relate)[0] != none or in<-ngmr_relate[0] != none) and aabb.d != NONE
        "#
    );
    if !replace_exist {
        sql.push_str(" and !booled");
    }
    match SUL_DB.query(&sql).await {
        Ok(mut response) => {
            match response.take::<Vec<ManiGeoTransQuery>>(0) {
                Ok(boolean_query) => {
                    // dbg!(&boolean_query);
                    let chunk = (boolean_query.len() / 16).max(1);
                    //排除有NREV的情况，因为NREV的布尔计算不是很准，还要判断这个NREV的包围盒和实体的包围盒是否差不多大
                    for chunk in boolean_query.chunks(chunk) {
                        let group = chunk.to_vec();
                        let dir_clone = dir.clone();
                        {
                            let mut update_sql = String::new();
                            for mut b in group {
                                let mut pos_manifolds = vec![];
                                for (pos_id, pos_t) in b.ts.iter() {
                                    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                                    println!("正在负实体计算的mesh hash: {}", &pos_id);
                                    if let Ok(manifold) = load_manifold(
                                        &dir_clone,
                                        pos_id,
                                        pos_t.compute_matrix().as_dmat4(),
                                        false,
                                    ) {
                                        pos_manifolds.push(manifold);
                                    }
                                }
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
                                for (neg_refno, mut neg_t, negs) in b.neg_ts.into_iter() {
                                    for NegInfo {
                                        id, trans, aabb, ..
                                    } in negs
                                    {
                                        let Some(mut neg_aabb) = aabb else {
                                            continue;
                                        };
                                        let m = inverse_mat
                                            * neg_t.compute_matrix().as_dmat4()
                                            * trans.compute_matrix().as_dmat4();
                                        if let Ok(manifold) =
                                            load_manifold(&dir_clone, &id, m, true)
                                        {
                                            #[cfg(feature = "debug_model")]
                                            {
                                                let neg_mesh = PlantMesh::from(&manifold);
                                                neg_mesh
                                                    .export_obj(
                                                        false,
                                                        &format!("{}_t.obj", neg_refno),
                                                    )
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
                                    #[cfg(feature = "debug_model")]
                                    dbg!(final_manifold.num_tri());
                                    let mesh = PlantMesh::from(&final_manifold);
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
    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
    println!("design的负实体计算{}完成", refno);
    Ok(())
}


#[tokio::test]
async fn test_json_refno_parse_error() {
    init_test_surreal().await;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub(crate) struct TestJson {
        pub refno: RefU64,
        // pub noun: String,
        // pub wt: Transform,
        // pub aabb: Aabb,
        // pub ts: Vec<(String, Transform)>,
        pub neg_ts: Vec<(Transform, Vec<NegInfo>)>,
    }

    let test_json = r#"
        {
            "refno": { "tb": "pe", "id": { "String": "17496_172792" } },
             "neg_ts": [
      [
        [
          {
            "rotation": [0.47776905, 0.5212838, 0.5212838, 0.47776905],
            "scale": [1.0, 1.0, 1.0],
            "translation": [-4751.5884, 10621.164, 4649.75]
          }
        ,

          {
            "aabb": {
              "maxs": [0.5, 0.49786708, 1.0],
              "mins": [-0.5, -0.49786708, 0.0]
            },
            "geo_type": "Neg",
            "id": "2",
            "trans": {
              "rotation": [0.0, 0.0, 0.0, 1.0],
              "scale": [17.0, 17.0, 16.0],
              "translation": [0.0, 0.0, -8.0]
            }
          }
        ]
      ],
      [
        [
          {
            "rotation": [0.47776905, 0.5212838, 0.5212838, 0.47776905],
            "scale": [1.0, 1.0, 1.0],
            "translation": [-4736.3726, 10446.827, 4649.75]
          }
        ,

          {
            "aabb": {
              "maxs": [0.5, 0.49786708, 1.0],
              "mins": [-0.5, -0.49786708, 0.0]
            },
            "geo_type": "Neg",
            "id": "2",
            "trans": {
              "rotation": [0.0, 0.0, 0.0, 1.0],
              "scale": [17.0, 17.0, 16.0],
              "translation": [0.0, 0.0, -8.0]
            }
          }
        ]
      ]
    ]
        }
    "#;

    let result = serde_json::from_str::<TestJson>(test_json);
    dbg!(result);

    // let refno:RefU64 = "17496_172792".into();
    // let path: PathBuf = "assets/meshes".into();
    // apply_insts_boolean_manifold_single(refno, false, path).await.unwrap();
}

#[tokio::test]
async fn test_boolean_refno_parse_error() {
    init_test_surreal().await;

    let refno:RefU64 = "17496_172792".into();
    let path: PathBuf = "assets/meshes".into();
    apply_insts_boolean_manifold_single(refno, false, path).await.unwrap();
}