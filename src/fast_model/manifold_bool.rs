//! Manifold 布尔运算模块
//!
//! 本模块提供基于 Manifold 库的几何体布尔运算功能。
//! 所有布尔运算操作均使用 Manifold 库实现，不再依赖 OpenCASCADE。

use crate::fast_model::{debug_model, debug_model_debug, debug_model_trace};
use crate::{db_err, deser_err, log_err, query_err};
use aios_core::SurrealQueryExt;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{
    CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo, ParamNegInfo,
    query_cata_neg_boolean_groups, query_geom_mesh_data, query_manifold_boolean_operations,
    query_simple_cata_negative_bool,
};
use aios_core::{
    RecordId, RefnoEnum, SUL_DB, gen_bytes_hash, get_inst_relate_keys, init_test_surreal,
    utils::RecordIdExt,
};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use glam::DMat4;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// 从文件加载网格数据
///
/// # 参数
///
/// * `id` - 网格文件的ID
///
/// # 返回值
///
/// 返回 `anyhow::Result<PlantMesh>` 表示加载是否成功以及加载的网格数据
#[inline]
fn load_mesh(id: &str) -> anyhow::Result<PlantMesh> {
    let mesh = PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", id))?;
    Ok(mesh)
}

/// 从文件加载流形数据
///
/// # 参数
///
/// * `dir` - 模型文件目录路径
/// * `id` - 网格文件的ID
/// * `mat` - 变换矩阵
/// * `more_precision` - 是否需要更高精度
///
/// # 返回值
///
/// 返回 `anyhow::Result<ManifoldRust>` 表示加载是否成功以及加载的流形数据
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

/// 处理元件库有负实体的布尔运算（使用 Manifold）
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
pub async fn apply_cata_neg_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    // Query catalog negative boolean groups using the extracted method
    let params = query_cata_neg_boolean_groups(refnos, replace_exist)
        .await
        .map_err(|e| {
            let msg = format!("{:?}", e);
            query_err!("apply_cata_neg_boolean_manifold 查询失败", msg)(e)
        })?;

    if params.is_empty() {
        return Ok(());
    }

    let mut tasks = Vec::new();
    let chunk = (params.len() / 16).max(1);
    // let chunk = params.len();
    // dbg!(&params);
    for chunk in params.chunks(chunk) {
        let group: Vec<CataNegGroup> = chunk.iter().cloned().collect();
        let dir_clone = dir.clone();
        let task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            for g in group {
                let pes = g
                    .boolean_group
                    .iter()
                    .flatten()
                    .map(|x: &RefnoEnum| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",");
                // dbg!(g.refno);
                let sql = format!(
                    r#"
                    select out as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
                    from {}->inst_relate->inst_info->geo_relate
                    where !out.bad and geom_refno in [{}]  and out.aabb!=none and out.param!=none"#,
                    g.refno.to_pe_key(),
                    pes
                );
                // println!("geom sql is {}", &sql);
                // 使用 JsonValue 作为中间类型，然后手动反序列化
                let gms = SUL_DB.query_take::<Vec<GmGeoData>>(&sql, 0).await?;
                // .map_err(|e| anyhow!("query_take failed: {e}; sql: {sql}"))?;
                // dbg!(&gms);

                let mut update_sql = String::new();
                for bg in g.boolean_group {
                    let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id.to_raw(),
                        ));
                        continue;
                    };

                    debug_model_debug!("正在负实体计算的mesh hash: {}", &pos.id.to_mesh_id());

                    let Ok(mut pos_manifold) = load_manifold(
                        &dir_clone,
                        &pos.id.to_mesh_id(),
                        pos.trans.0.to_matrix().as_dmat4(),
                        false,
                    ) else {
                        println!("布尔运算失败: 无法加载正实体 manifold, refno: {}", &g.refno);
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id.to_raw(),
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
                        let m = neg_geo.trans.0.to_matrix().as_dmat4();
                        if let Ok(manifold) =
                            load_manifold(&dir_clone, &neg_geo.id.to_mesh_id(), m, true)
                        {
                            neg_manifolds.push(manifold);
                        }
                    }
                    //没有负实体也要加上为_b后缀，表示已经进行过分析计算了。
                    // if !neg_manifolds.is_empty()
                    {
                        let new_id = g.refno.hash_with_another_refno(bg[0]);
                        let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
                        let mesh = PlantMesh::from(&final_manifold);
                        //保存到文件到dir下
                        if mesh
                            .ser_to_file(&dir_clone.join(format!("{}.mesh", new_id)))
                            .is_ok()
                        {
                            update_sql.push_str(&format!(
                                "create inst_geo:⟨{}⟩ set meshed = true, aabb = {};",
                                new_id,
                                &pos.aabb_id.to_raw()
                            ));
                            // 有索引的关系，所以geom_refno需要点变化
                            let relate_sql = format!(
                                "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:⟨{}⟩, geo_type='Pos', trans=trans:⟨0⟩, visible = true;",
                                &g.inst_info_id.to_raw(),
                                new_id,
                                format!("{}_b", bg[0]),
                            );
                            // println!("cate neg relate sql is {}", &relate_sql);
                            update_sql.push_str(relate_sql.as_str());
                            update_sql.push_str(&format!(
                                "update {}<-inst_relate set booled=true;",
                                &g.inst_info_id.to_raw(),
                            ));
                            // dbg!(&update_sql);
                        }
                    }
                }
                if !update_sql.is_empty() {
                    SUL_DB
                        .query(update_sql.clone())
                        .await
                        .map_err(|e| anyhow!("update failed: {e}; sql: {update_sql}"))?;
                }
            }
            Ok(())
        });
        tasks.push(task);
    }
    // dbg!(tasks.len());
    // 传播 JoinError 与任务内部的 anyhow::Error
    let results = futures::future::try_join_all(tasks).await?;
    for r in results {
        r?;
    }
    debug_model!("元件库的负实体计算{:?}完成", refnos);
    Ok(())
}

/// 对多个实例进行布尔运算（使用 Manifold）
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示布尔运算是否成功
pub async fn apply_insts_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    for refno in refnos {
        apply_insts_boolean_manifold_single(*refno, replace_exist, dir.clone()).await?;
    }
    Ok(())
}

/// 对单个实例进行布尔运算（使用 Manifold）
///
/// # 参数
///
/// * `refno` - 参考号
/// * `replace_exist` - 是否替换已存在的布尔运算结果
/// * `dir` - 模型文件目录路径
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示布尔运算是否成功
pub async fn apply_insts_boolean_manifold_single(
    refno: RefnoEnum,
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    // Query manifold boolean operations data using the extracted method
    match query_manifold_boolean_operations(refno).await {
        Ok(boolean_query) => {
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
                            let pos_mesh_id = pos_id.to_mesh_id();
                            debug_model_debug!("正在负实体计算的mesh hash: {}", &pos_mesh_id);
                            if let Ok(manifold) = load_manifold(
                                &dir_clone,
                                &pos_mesh_id,
                                pos_t.0.to_matrix().as_dmat4(),
                                false,
                            ) {
                                pos_manifolds.push(manifold);
                            }
                        }
                        //没有实体的情况，下次就不要再继续计算布尔运算了
                        let inst_relate_id = b.refno.to_table_key("inst_relate");
                        if pos_manifolds.is_empty() {
                            println!("布尔运算失败: 没有找到正实体 manifold, refno: {}", &b.refno);
                            update_sql.push_str(&format!(
                                "update {} set bad_bool=true;",
                                &inst_relate_id
                            ));
                            continue;
                        };
                        let inverse_mat = b.wt.0.to_matrix().as_dmat4().inverse();
                        let mut pos_manifold = ManifoldRust::batch_boolean(&pos_manifolds, 0);
                        if pos_manifold.num_tri() == 0 {
                            println!(
                                "布尔运算失败: 正实体 manifold 没有三角形, refno: {}",
                                &b.refno
                            );
                            update_sql.push_str(&format!(
                                "update {} set bad_bool=true;",
                                &inst_relate_id
                            ));
                            continue;
                        };
                        #[cfg(feature = "debug_model")]
                        {
                            // let pos_mesh = PlantMesh::from(&pos_manifold);
                            // pos_mesh.export_obj(false, "pos_t.obj").unwrap();
                        }

                        let mut neg_manifolds = vec![];
                        for (neg_refno, mut neg_t, negs) in b.neg_ts.into_iter() {
                            for NegInfo {
                                id, trans, aabb, ..
                            } in negs
                            {
                                let Some(mut neg_aabb) = aabb else {
                                    continue;
                                };
                                let m = inverse_mat
                                    * neg_t.0.to_matrix().as_dmat4()
                                    * trans.0.to_matrix().as_dmat4();
                                if let Ok(manifold) =
                                    load_manifold(&dir_clone, &id.to_mesh_id(), m, true)
                                {
                                    #[cfg(feature = "debug_model")]
                                    {
                                        let neg_mesh = PlantMesh::from(&manifold);
                                        // neg_mesh
                                        //     .export_obj(
                                        //         false,
                                        //         &format!("{}_t.obj", neg_refno),
                                        //     )
                                        //     .unwrap();
                                    }
                                    neg_manifolds.push(manifold);
                                }
                            }
                        }

                        if !neg_manifolds.is_empty() {
                            let mut success = false;
                            let final_manifold =
                                pos_manifold.batch_boolean_subtract(&neg_manifolds);
                            let mesh = PlantMesh::from(&final_manifold);
                            // 生成mesh_id: 如果是当前版本(sesno==0)用refno，否则用refno_sesno
                            let mesh_id = if b.sesno == 0 {
                                b.refno.to_string()
                            } else {
                                format!("{}_{}", b.refno, b.sesno)
                            };
                            // dbg!(&mesh_id);
                            //保存到文件到dir下
                            if mesh
                                .ser_to_file(&dir_clone.join(format!("{}.mesh", mesh_id)))
                                .is_ok()
                            {
                                update_sql.push_str(&format!(
                                    "update {} set booled_id='{}';",
                                    &inst_relate_id, mesh_id
                                ));
                                success = true;
                            }

                            if !success {
                                println!("布尔运算失败: 无法保存结果 mesh, refno: {}", &b.refno);
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
            // Error handling is already done in the query method
            return Err(e);
        }
    }
    debug_model!("design的负实体计算{}完成", refno);
    Ok(())
}

#[tokio::test]
async fn test_boolean_refno_parse_error() {
    init_test_surreal().await;

    let refno: RefnoEnum = "17496_172792".into();
    let path: PathBuf = "assets/meshes".into();
    apply_insts_boolean_manifold_single(refno, false, path)
        .await
        .unwrap();
}

