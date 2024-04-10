use crate::fast_model::utils;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::{PlantMesh, RsVec3};
use aios_core::test::test_surreal::init_test_surreal;
use aios_core::{gen_bytes_hash, RefU64, SUL_DB};
use bevy_transform::prelude::Transform;
use itertools::Itertools;
use opencascade::primitives::IntoShape;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use aios_core::tool::float_tool::{dvec4_round_3, f64_round};
use glam::DMat4;
use parse_pdms_db::parse::round_f32;
use crate::fast_model::manifold_bool::{apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold};

///生成小的几何体
#[tokio::test]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    process_meshes_update_db(Some(&["17496/171559".into(), "24381/35844".into()]))
        .await
        .unwrap();
    Ok(())
}

pub async fn process_meshes_update_db(refnos: Option<&[RefU64]>) -> anyhow::Result<()> {
    let time = std::time::Instant::now();
    gen_inst_meshes(None).await.unwrap();
    println!("gen_inst_meshes finished: {} ms", time.elapsed().as_millis());
    let time = std::time::Instant::now();
    update_inst_relate_aabbs().await.unwrap();
    println!("update_inst_relate_aabbs finished: {} ms", time.elapsed().as_millis());
    // apply_cata_neg_boolean_occ(None).await.unwrap();
    apply_cata_neg_boolean_manifold(None).await.unwrap();
    let time = std::time::Instant::now();
    apply_insts_boolean_manifold(None).await.unwrap();
    apply_insts_boolean_occ(None).await.unwrap();
    println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryGeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

pub async fn gen_inst_meshes(dir: Option<PathBuf>) -> anyhow::Result<()> {
    //首先查询 inst_geo
    //需要扫描当前的文件夹，是否有已经生成的几何体
    //使用page 去扫描？
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }

    const PAGE_NUM: usize = 100;
    let mut i = 0;
    let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();

    let sql = r#"select value id from inst_geo where !meshed && !bad"#.to_string();
    let mut response = SUL_DB.query(sql).await.unwrap();
    let inst_geo_ids: Vec<String> = response.take(0).unwrap();

    for chunk in inst_geo_ids.chunks(PAGE_NUM) {
        let ids = chunk.join(",");
        let mut response = SUL_DB.query(&format!("select meta::id(id) as id, param from {}", ids)).await?;
        let result: Vec<QueryGeoParam> = response.take(0)?;
        if result.is_empty() {
            break;
        }
        i += 1;
        // dbg!(&result.len());
        for g in result {
            //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
            // dbg!(&g);
            match g.param.gen_occ_shape() {
                Ok(shape) => {
                    let mut aabb = Aabb::new_invalid();
                    for edge in shape.edges() {
                        for point in edge.approximation_segments_custom(1.0, 1.0) {
                            aabb.take_point(nalgebra::Point3::new(
                                point.x as f32,
                                point.y as f32,
                                point.z as f32,
                            ));
                        }
                    }
                    let tol = aabb.half_extents().magnitude() as f64 * 0.005;
                    shapes_map.insert(g.id, (shape, tol));
                }
                Err(e) => {
                    // dbg!("{} error: {e}", g.id);
                }
            }
        }
        let mut update_sql = "".to_string();
        let mut aabb_map: HashMap<u64, String> = HashMap::new();
        let mut pts_json_map = HashMap::new();
        for (id, (s, tol)) in &shapes_map {
            let mut m_tol = *tol;
            let mut success = false;
            // #[cfg(debug_assertions)]
            // s.write_step(format!("{}.step", id)).unwrap();
            match PlantMesh::gen_occ_mesh(s, m_tol) {
                Ok(mesh) => {
                    // dbg!((id, m_tol, mesh.vertices.len()));
                    //保存到文件到dir下
                    if mesh.ser_to_file(&dir.join(format!("{}.mesh", id))).is_ok() {
                        let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                        let mut pt_hashes = HashSet::new();
                        for edge in s.edges() {
                            for point in edge.approximation_segments_custom(1.0, 1.0) {
                                // dbg!(point);
                                let pts_hash = RsVec3(point.as_vec3()).gen_hash();
                                pt_hashes.insert(format!("vec3:⟨{}⟩", pts_hash));
                                if !pts_json_map.contains_key(&pts_hash) {
                                    pts_json_map.insert(pts_hash, serde_json::to_string(&point).unwrap());
                                }
                            }
                        }
                        update_sql.push_str(&format!(
                            "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩, pts=[{}];",
                            id, aabb_hash, pt_hashes.into_iter().join(","),
                        ));
                        aabb_map
                            .entry(aabb_hash)
                            .or_insert(serde_json::to_string(&mesh.aabb).unwrap());
                        success = true;
                    }
                }
                Err(e) => {
                    println!("{} mesh error: {e}", id);
                }
            }
            if !success {
                //有问题的模型，就不需要每次都重复生成了
                update_sql.push_str(&format!("update inst_geo:⟨{}⟩ set bad = true;", id));
            }
        }
        if !update_sql.is_empty() {
            //执行SUL_DB update,使用chunk 保存
            SUL_DB.query(update_sql).await.unwrap();
        }
        utils::save_pts_to_surreal(&pts_json_map).await?;
        //更新aabb数据到数据库
        utils::save_aabb_to_surreal(&aabb_map).await?;
    }

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: RefU64,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    pub world_trans: Transform,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    pub trans: Transform,
    pub aabb: Aabb,
}

///刷新inst_relate 的 aabb
pub async fn update_inst_relate_aabbs() -> anyhow::Result<()> {
    const PAGE_NUM: usize = 300;
    let mut i = 0;
    let sql = format!(r#"select value id from inst_relate where aabb = none"#);
    let mut response = SUL_DB.query(sql).await.unwrap();
    let inst_relate_ids: Vec<String> = response.take(0).unwrap();
    for chunk in inst_relate_ids.chunks(PAGE_NUM) {
        let mut aabb_map: HashMap<u64, String> = HashMap::new();
        let ids = chunk.join(",");
        let sql = format!(r#"select in as id, world_trans.d as world_trans,
            (select out.aabb.d as aabb, trans.d as trans from out->geo_relate where out.aabb.d != none)
            as geo_aabbs from [{}]"#, ids);
        let mut response = SUL_DB.query(sql).await.unwrap();
        let result: Vec<QueryAabbParam> = response.take(0).unwrap();
        dbg!(result.len());
        i += 1;

        let mut update_sql = String::new();
        for r in result {
            let mut aabb = Aabb::new_invalid();
            // if r.id == "24383_66722".into() {
            //     dbg!(&r);
            // }
            // dbg!(r.id);
            for g in r.geo_aabbs {
                let t = r.world_trans * g.trans;
                let tmp_aabb = g.aabb.scaled(&t.scale.into());
                let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                    rotation: t.rotation.into(),
                    translation: t.translation.into(),
                });
                aabb.merge(&tmp_aabb);
            }
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
            aabb_map
                .entry(aabb_hash)
                .or_insert(serde_json::to_string(&aabb).unwrap());
            let sql = format!(
                "update inst_relate set aabb = aabb:⟨{}⟩ where in=pe:{};",
                aabb_hash,
                r.id.to_string()
            );
            update_sql.push_str(&sql);
        }
        SUL_DB.query(&update_sql).await.unwrap();
        utils::save_aabb_to_surreal(&aabb_map).await.unwrap();
    }
    Ok(())
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NegInfo {
    pub id: String,
    pub geo_type: String,
    pub para_type: String,
    pub trans: Transform,
    pub aabb: Option<Aabb>
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeoTransQuery {
    pub refno: RefU64,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(String, Transform)>,
    pub neg_ts: Vec<(RefU64, Transform, Vec<NegInfo>)>,
}

#[inline]
fn round_dmat4(m: DMat4) -> DMat4 {
    DMat4 {
        x_axis: dvec4_round_3(m.x_axis),
        y_axis: dvec4_round_3(m.y_axis),
        z_axis: dvec4_round_3(m.z_axis),
        w_axis: dvec4_round_3(m.w_axis),
    }
}

//需要带入，缩小范围
///执行bool 运算
pub async fn apply_insts_boolean_occ(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }
    //避免重复执行布尔运算
    //"Neg",
    let sql = r#"
     select meta::id(id) as id, param from
         array::group(select value array::group([array::group(neg_refnos->inst_relate->inst_info->geo_relate[where !bad and
         geo_type in ["Neg", "CataCrossNeg"]]->inst_geo),
         ->inst_info->geo_relate->inst_geo[?!bad]]) from inst_relate where neg_refnos!=none and !bad_bool and !booled) where param!=none;
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let params: Vec<GeoParam> = response.take(0)?;
    // dbg!(&params.len());
    if params.is_empty() {
        return Ok(());
    }
    let mut shapes_map: HashMap<String, OccSharedShape> = HashMap::new();
    //然后执行bool 运算
    for g in params {
        //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
        if let Ok(shape) = g.param.gen_occ_shape() {
            shapes_map.insert(g.id, shape);
        }
    }
    //没有需要执行的布尔运算
    if shapes_map.is_empty() {
        return Ok(());
    }
    let shapes_map_arc = Arc::new(shapes_map);

    // and in.noun not in ["FLOOR"]
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    let sql = r#"
        select
             in as refno,
             world_trans.d as wt,
             aabb.d as aabb,
            (select value [meta::id(out), trans.d] from out->geo_relate) as ts,
           (select value [in, world_trans.d, (select meta::id(out) as id, geo_type, trans.d as trans,
             out.aabb.d as aabb, object::keys(out.param)[0] as para_type
            from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"])]
            from array::flatten(neg_refnos->inst_relate)) as neg_ts
        from inst_relate where !bad_bool and !booled
            and neg_refnos!=none and aabb.d!=none
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let boolean_query: Vec<GeoTransQuery> = response.take(0)?;
    dbg!(boolean_query.len());
    if boolean_query.is_empty(){
        return Ok(());
    }

    let mut tasks = Vec::new();
    // let chunk = (boolean_query.len() / 16).max(1);
    let chunk = boolean_query.len();
    for chunk in boolean_query.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let shapes_map_clone = shapes_map_arc.clone();
        let task = tokio::spawn(async move {
            let mut update_sql = String::new();
            for mut b in group {
                if b.ts.is_empty() {
                    continue;
                }
                let Some((pos_id, pos_t)) = b.ts.pop() else {
                    continue;
                };
                //没有实体的情况，下次就不要再继续计算布尔运算了
                let Some(mut pos_shape) = shapes_map_clone.get(&pos_id).map(|x|x.clone()) else {
                    update_sql.push_str(&format!(
                        "update inst_relate set bad_bool=true where in=pe:{};",
                        b.refno
                    ));
                    continue;
                };
                let pos_matrix = pos_t.compute_matrix().as_dmat4();
                // dbg!(pos_matrix);
                let Ok(mut pos_shape) = pos_shape.transformed(&pos_matrix) else {
                    update_sql.push_str(&format!(
                        "update inst_relate set bad_bool=true where in=pe:{};",
                        b.refno
                    ));
                    continue;
                };

                for (id, t) in b.ts.iter(){
                    dbg!(id);
                    if let Some(shape) = shapes_map_clone.get(id) {
                        if let Ok(s) = shape.transformed(&t.compute_matrix().as_dmat4()){
                            pos_shape = pos_shape.union(&s.0).shape.into();
                        }
                    }
                }
                // dbg!(b.refno);
                let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();

                #[cfg(debug_assertions)]
                pos_shape.write_step(format!("{}.step", "pos")).unwrap();
                // dbg!(b.neg_ts.len());
                let mut neg_shapes = vec![];
                let mut cross_neg_shapes = vec![];
                for (refno, neg_t, negs) in b.neg_ts.into_iter() {
                    // if refno != "25688/45323".into() {
                    //     continue;
                    // }
                    // for (neg_id, geo_type, t) in negs {
                    for NegInfo{ id, geo_type, para_type, trans, aabb } in negs {
                        if aabb.is_none() {
                            // dbg!(&id);
                            continue;
                        }
                        if let Some(neg_shape) = shapes_map_clone.get(&id) {
                            let m = round_dmat4(inverse_mat
                                * neg_t.compute_matrix().as_dmat4()
                                * trans.compute_matrix().as_dmat4());
                            // dbg!(m);
                            // dbg!(refno);
                            if let Ok(t_neg_shape) = neg_shape.0.transformed_by_gmat(&m) {
                                // t_neg_shape.write_step(format!("{}.step", &neg_id)).unwrap();
                                if geo_type == "Neg" {
                                    // dbg!(refno);
                                    neg_shapes.push(t_neg_shape);
                                } else {
                                    cross_neg_shapes.push(t_neg_shape);
                                }
                            }
                        }
                    }
                }
                // dbg!((neg_shapes.len(), cross_neg_shapes.len()));
                if !neg_shapes.is_empty() || !cross_neg_shapes.is_empty() {
                    let mut success = false;
                    if let Ok(pos_shape) = pos_shape.subtract_shapes(&neg_shapes, false) {
                        if let Ok(final_shape) = pos_shape.subtract_shapes(&cross_neg_shapes, true) {
                            let tol = b.aabb.half_extents().magnitude() * 0.01;
                            #[cfg(debug_assertions)]
                            {
                                final_shape.write_step(format!("{}.step", b.refno)).unwrap();
                                // final_shape.write_stl_with_tolerance(format!("{}.stl", b.refno), tol as _).unwrap();
                            }
                            // dbg!(tol);
                            if let Ok(mesh) = PlantMesh::gen_occ_mesh(&final_shape, tol as _) {
                                //保存到文件到dir下
                                if mesh
                                    .ser_to_file(&dir_clone.join(format!("{}.mesh", b.refno)))
                                    .is_ok()
                                {
                                    update_sql.push_str(&format!(
                                        "update inst_relate set booled=true where in=pe:{};",
                                        b.refno
                                    ));
                                    success = true;
                                }
                            }
                        }
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CataNegGroup {
    pub refno: RefU64,
    pub inst_info_id: String,
    pub boolean_group: Vec<Vec<RefU64>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GmGeoData {
    pub id: String,
    pub geom_refno: RefU64,
    pub trans: Transform,
    pub param: PdmsGeoParam,
    //暂时aabb 不变
    pub aabb_id: String,
}

//处理元件库有负实体的布尔运算
pub async fn apply_cata_neg_boolean_occ(dir: Option<PathBuf>) -> anyhow::Result<()> {
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
    // dbg!(params.len());
    // dbg!(&params);
    if params.is_empty() {
        return Ok(());
    }

    let mut tasks = Vec::new();
    // let chunk = (params.len() / 16).max(1);
    let chunk = params.len();
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
                    // dbg!(pos);
                    let Ok(Ok(mut pos_shape)) = pos
                        .param
                        .gen_occ_shape()
                        .map(|x| x.transformed(&pos.trans.compute_matrix().as_dmat4())) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };
                    // pos_shape
                    //     .write_step(format!("{}.step", "pos"))
                    //     .unwrap();

                    let mut neg_shapes = vec![];
                    for &neg in bg.iter().skip(1) {
                        // dbg!(neg);
                        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                            continue;
                        };
                        // dbg!(neg_geo.trans.compute_matrix().as_dmat4());
                        let Ok(neg_shape) = neg_geo
                            .param
                            .gen_occ_shape() else {
                            continue;
                        };
                        if let Ok(t_neg_shape) = neg_shape.0.transformed_by_gmat(&neg_geo.trans.compute_matrix().as_dmat4()) {
                            // #[cfg(debug_assertions)]
                            // t_neg_shape.write_step(format!("{}.step", neg)).unwrap();
                            neg_shapes.push(t_neg_shape);
                        }
                    }
                    if !neg_shapes.is_empty() {
                        // for neg_shape in neg_shapes {
                        let new_id = g.refno.hash_with_another_refno(bg[0]);
                        if let Ok(pos_shape) = pos_shape.subtract_shapes(&neg_shapes, true) {
                            let mut aabb = Aabb::new_invalid();
                            for edge in pos_shape.edges() {
                                for point in edge.approximation_segments_custom(1.0, 1.0) {
                                    aabb.take_point(nalgebra::Point3::new(
                                        point.x as f32,
                                        point.y as f32,
                                        point.z as f32,
                                    ));
                                }
                            }
                            let tol = aabb.half_extents().magnitude() as f64 * 0.01;
                            // dbg!(tol);
                            // #[cfg(debug_assertions)]
                            // pos_shape
                            //     .write_step(format!("{}.step", "final"))
                            //     .unwrap();
                            let mut success = false;
                            if let Ok(mesh) = PlantMesh::gen_occ_mesh(&pos_shape, tol as _) {
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
                                    success = true;
                                }
                            }

                            if !success {
                                update_sql.push_str(&format!(
                                    "update {}<-inst_relate set bad_bool=true;",
                                    &g.inst_info_id,
                                ));
                            }
                        }
                    }
                    // pos_shape
                    //     .write_step(format!("{}.step", "final"))
                    //     .unwrap();
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
