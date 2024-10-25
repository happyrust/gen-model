use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use crate::fast_model::{utils, EXIST_MESH_GEO_HASHES};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::{PlantMesh, RsVec3};
use aios_core::tool::float_tool::{dvec4_round_3, f64_round};
use aios_core::{
    gen_bytes_hash, get_inst_relate_keys, query_deep_neg_inst_refnos,
    query_deep_visible_inst_refnos, RefnoEnum, SUL_DB,
};
use aios_core::{get_db_option, init_test_surreal};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::DMat4;
use itertools::Itertools;
use opencascade::primitives::IntoShape;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use parse_pdms_db::parse::round_f32;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use surrealdb::sql::Thing;

///生成小的几何体
#[tokio::test]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    process_meshes_update_db_deep_default((&["17496/171559".into(), "24381/35844".into()]))
        .await
        .unwrap();
    Ok(())
}

///生成模型的部分，update aabb
pub async fn gen_meshes_in_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    let replace_exist = option
        .as_ref()
        .map(|x| x.is_replace_mesh())
        .unwrap_or(false);
    // let time = std::time::Instant::now();
    let dir = option
        .as_ref()
        .map(|x| x.get_meshes_path())
        .unwrap_or("assets/meshes".into());
    for chunk in refnos.chunks(100) {
        // 生成模型文件
        gen_inst_meshes(chunk, replace_exist, dir.clone())
            .await
            .unwrap();
        // println!(
        //     "gen_inst_meshes finished: {} ms",
        //     time.elapsed().as_millis()
        // );
        // let time = std::time::Instant::now();
        update_inst_relate_aabbs_by_refnos(chunk, replace_exist)
            .await
            .unwrap();
        // println!(
        //     "update_inst_relate_aabbs finished: {} ms",
        //     time.elapsed().as_millis()
        // );
    }
    Ok(())
}

///执行布尔运算的部分
pub async fn booleans_meshes_in_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    for chunk in refnos.chunks(100) {
        let dir = option
            .as_ref()
            .map(|x| x.get_meshes_path())
            .unwrap_or("assets/meshes".into());
        let replace_exist = option
            .as_ref()
            .map(|x| x.is_replace_mesh())
            .unwrap_or(false);
        let time = std::time::Instant::now();
        //生成元件库内部几何体的负实体运算
        apply_cata_neg_boolean_manifold(chunk, replace_exist, dir.clone())
            .await
            .unwrap();
        apply_insts_boolean_manifold(chunk, replace_exist, dir.clone()).await?;
        //有一些布尔运算要精确计算，不然会有薄片出现
        //生成负实体的布尔运算
        // apply_insts_boolean_occ(&refnos, replace_exist, dir.clone()).await?;
        // println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());
    }
    Ok(())
}

pub async fn process_meshes_update_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    let replace_exist = option
        .as_ref()
        .map(|x| x.is_replace_mesh())
        .unwrap_or(false);
    let time = std::time::Instant::now();
    let dir = option
        .as_ref()
        .map(|x| x.get_meshes_path())
        .unwrap_or("assets/meshes".into());
    // dbg!(&target_refnos);
    // 生成模型文件
    gen_inst_meshes(&refnos, replace_exist, dir.clone())
        .await
        .unwrap();
    println!(
        "gen_inst_meshes finished: {} ms",
        time.elapsed().as_millis()
    );
    let time = std::time::Instant::now();
    update_inst_relate_aabbs_by_refnos(&refnos, replace_exist)
        .await
        .unwrap();
    println!(
        "update_inst_relate_aabbs finished: {} ms",
        time.elapsed().as_millis()
    );

    let time = std::time::Instant::now();
    //生成元件库内部几何体的负实体运算
    apply_cata_neg_boolean_manifold(&refnos, replace_exist, dir.clone())
        .await
        .unwrap();
    apply_insts_boolean_manifold(&refnos, replace_exist, dir.clone()).await?;
    //有一些布尔运算要精确计算，不然会有薄片出现
    //生成负实体的布尔运算
    // apply_insts_boolean_occ(&refnos, replace_exist, dir.clone()).await?;
    // println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());

    Ok(())
}

pub async fn process_meshes_update_db_deep_default(refnos: &[RefnoEnum]) -> anyhow::Result<()> {
    let dboption = get_db_option();
    process_meshes_update_db_deep(&dboption, refnos).await
}

pub async fn process_meshes_update_db_deep(
    dboption: &DbOption,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if !refnos.is_empty() {
        let dir = dboption.get_meshes_path();
        let replace_exist = dboption.is_replace_mesh();
        // dbg!(refnos.len());
        println!("更新模型结点数量: {}", refnos.len());
        let time = std::time::Instant::now();
        for &refno in refnos {
            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
            println!("更新模型结点: {}", refno);
            let mut target_visible_refnos = vec![];
            let mut update_refnos = query_deep_visible_inst_refnos(refno)
                .await
                .unwrap_or_default();
            target_visible_refnos.extend(update_refnos.clone());
            dbg!(&target_visible_refnos);

            let neg_refnos = query_deep_neg_inst_refnos(refno).await.unwrap_or_default();
            update_refnos.extend(neg_refnos);

            // #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
            println!("实际需要更新模型结点数量: {}", update_refnos.len());

            if update_refnos.is_empty() {
                continue;
            }

            //缩小范围
            if dboption.gen_mesh {
                // dbg!(&target_refnos);
                // 生成模型文件
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                let time = std::time::Instant::now();
                gen_inst_meshes(&update_refnos, replace_exist, dir.clone())
                    .await
                    .unwrap();
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                println!(
                    "gen_inst_meshes finished: {} ms",
                    time.elapsed().as_millis()
                );
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                let time = std::time::Instant::now();
                // 更新aabb 到inst relate，geo relate
                update_inst_relate_aabbs_by_refnos(&update_refnos, replace_exist)
                    .await
                    .unwrap();
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                println!(
                    "update_inst_relate_aabbs finished: {} ms",
                    time.elapsed().as_millis()
                );
            }

            if target_visible_refnos.is_empty() {
                continue;
            }

            if dboption.apply_boolean_operation {
                // apply_cata_neg_boolean_occ(None).await.unwrap();
                // dbg!(target_visible_refnos.len());
                let time = std::time::Instant::now();
                //生成元件库内部几何体的负实体运算
                apply_cata_neg_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                apply_insts_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                //有一些布尔运算要精确计算，不然会有薄片出现
                //生成负实体的布尔运算
                // apply_insts_boolean_occ(&target_visible_refnos, replace_exist, dir.clone()).await?;
            }
        }
        println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryGeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

///生成meshes
pub async fn gen_inst_meshes(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    const PAGE_NUM: usize = 100;
    let mut i = 0;
    let inst_keys = get_inst_relate_keys(refnos);
    let sql = if replace_exist {
        format!(
            r#"array::group(select value (select value [out, ($parent<-neg_relate)[0] != none] from out->geo_relate where  !out.bad)
            from {inst_keys})"#
        )
    } else {
        format!(
            r#"array::group(select value (select value [out, ($parent<-neg_relate)[0] != none] from out->geo_relate where out.aabb.d=none and !out.meshed and !out.bad)
            from {inst_keys})"#
        )
    };
    //out.aabb.d == none and
    // println!("sql is {}", &sql);
    let mut response = SUL_DB.query(sql).await.unwrap();
    let mut inst_geo_ids: Vec<(Option<Thing>, bool)> = response.take(0).unwrap();
    //todo 排除已经生成了的模型
    // let mut update_geos_by_meshes = HashSet::default();
    inst_geo_ids.retain(|(x, y)| {
        if let Some(t) = x {
            if replace_exist {
                true
            } else {
                if EXIST_MESH_GEO_HASHES.contains_key(&t.id.to_raw()) {
                    // update_geos_by_meshes.insert(t.id.to_raw());
                    false
                } else {
                    true
                }
            }
        } else {
            false
        }
    });
    if inst_geo_ids.is_empty() {
        return Ok(());
    }
    let thing_has_neg_map = inst_geo_ids
        .iter()
        .map(|(x, y)| (x.as_ref().unwrap().id.to_raw(), *y))
        .collect::<HashMap<_, _>>();
    let thing_has_neg_map_arc = Arc::new(thing_has_neg_map);
    // dbg!(&thing_map);
    let mut tasks = vec![];
    //: DashMap<u64, String>
    let aabb_map = Arc::new(DashMap::new());
    let pts_json_map = Arc::new(DashMap::new());
    for (idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
        let ids = chunk
            .into_iter()
            .map(|(x, _)| x.as_ref().unwrap().to_string())
            .join(",");
        let thing_neg_map = thing_has_neg_map_arc.clone();
        let dir = dir.clone();
        let aabb_map = aabb_map.clone();
        let pts_json_map = pts_json_map.clone();
        let task = tokio::spawn(async move {
            let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();
            let sql = format!(
                "select <string> record::id(id) as id, param from [{}] where param != NONE",
                ids
            );
            // println!("sql is {}", &sql);
            match SUL_DB.query(&sql).await {
                Ok(mut response) => {
                    let r = response.take::<Vec<QueryGeoParam>>(0);
                    if let Err(e) = &r {
                        init_deserialize_error(
                            "Vec<QueryGeoParam>",
                            e,
                            &sql,
                            &std::panic::Location::caller().to_string(),
                        );
                        return;
                    }
                    let result: Vec<QueryGeoParam> = r.unwrap();
                    if result.is_empty() {
                        return;
                    }
                    i += 1;
                    // dbg!(&result);
                    for g in result {
                        //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
                        #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                        println!("gen mesh param: {}", &g.id);
                        //检查是否是PrimPolyhedron
                        let is_polyhedron = match &g.param {
                            PdmsGeoParam::PrimPolyhedron(_) => true,
                            _ => false,
                        };
                        match g.param.gen_occ_shape() {
                            Ok(shape) => {
                                let mut aabb = Aabb::new_invalid();
                                for edge in shape.edges() {
                                    for point in edge.approximation_segments_custom(2.0, 2.0) {
                                        aabb.take_point(nalgebra::Point3::new(
                                            point.x as f32,
                                            point.y as f32,
                                            point.z as f32,
                                        ));
                                    }
                                }
                                //如果作为负实体，需要缩小一些范围？如果作为负实体的母体，需要把精度提高一些
                                //如果是作为负实体可以稍微降一些？
                                let mut coeff = 0.005;
                                // dbg!(&g.id);
                                if thing_neg_map.get(&g.id).copied().unwrap_or(false) {
                                    match g.param {
                                        PdmsGeoParam::PrimExtrusion(_)
                                        | PdmsGeoParam::PrimRevolution(_) => {
                                            coeff /= 10.0;
                                            // dbg!(&coeff);
                                        }
                                        _ => {
                                            coeff /= 5.0;
                                        }
                                    };
                                }

                                let mut tol = if is_polyhedron {
                                    0.01
                                } else {
                                    (aabb.half_extents().magnitude() as f64 * coeff).min(50.0)
                                };
                                // dbg!(tol);
                                shapes_map.insert(g.id, (shape, tol));
                            }
                            Err(e) => {
                                #[cfg(feature = "log_error")]
                                {
                                    let failed_refnos =
                                        aios_core::query_refnos_by_geo_hash(&g.id).await.unwrap();
                                    println!("{:?} mesh error: {}", failed_refnos, e.to_string());
                                }
                            }
                        }
                    }
                    let mut update_sql = "".to_string();

                    for (id, (s, tol)) in &shapes_map {
                        let mut m_tol = *tol;
                        // dbg!(m_tol);
                        let mut success = false;
                        // #[cfg(feature = "debug_model")]
                        // s.write_step(format!("{}.step", id)).unwrap();
                        #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                        println!("gen mesh hash: {}", id);
                        match PlantMesh::gen_occ_mesh(s, m_tol) {
                            Ok(mesh) => {
                                if mesh.aabb.is_none() {
                                    continue;
                                }
                                #[cfg(feature = "debug_model")]
                                mesh.export_obj(false, &format!("{}.obj", id));
                                // dbg!((id, m_tol, mesh.vertices.len()));
                                //保存到文件到dir下
                                if mesh.ser_to_file(&dir.join(format!("{}.mesh", id))).is_ok() {
                                    #[cfg(feature = "debug_model")]
                                    mesh.export_obj(false, &format!("{}.obj", id));
                                    let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                                    let mut pt_hashes = HashSet::new();
                                    for edge in s.edges() {
                                        //TODO edge 这里取中点就可以了
                                        // for point in edge.approximation_segments_custom(1.0, 1.0) {
                                        for point in [edge.start_point(), edge.end_point()] {
                                            let pts_hash = RsVec3(point.as_vec3()).gen_hash();
                                            pt_hashes.insert(format!("vec3:⟨{}⟩", pts_hash));
                                            if !pts_json_map.contains_key(&pts_hash) {
                                                pts_json_map.insert(
                                                    pts_hash,
                                                    serde_json::to_string(&point).unwrap(),
                                                );
                                            }
                                        }
                                    }
                                    update_sql.push_str(&format!(
                                        "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩, pts=[{}];",
                                        id,
                                        aabb_hash,
                                        pt_hashes.into_iter().join(","),
                                    ));
                                    aabb_map
                                        .entry(aabb_hash.to_string())
                                        .or_insert(mesh.aabb.unwrap());
                                    success = true;
                                }
                            }
                            //显示哪些模型可能会受影响
                            Err(e) => {
                                #[cfg(any(
                                    feature = "debug_model",
                                    feature = "debug_model_no_obj"
                                ))]
                                {
                                    let failed_refnos =
                                        aios_core::query_refnos_by_geo_hash(id).await.unwrap();
                                    println!("{:?} mesh error: {}", failed_refnos, e.to_string());
                                }
                            }
                        }
                        if !success {
                            //有问题的模型，就不需要每次都重复生成了
                            update_sql.push_str(&format!("update inst_geo:⟨{}⟩ set bad=true;", id));
                        }
                    }
                    if !update_sql.is_empty() {
                        //执行SUL_DB update,使用chunk 保存
                        if let Err(_) = SUL_DB.query(&update_sql).await {
                            init_save_database_error(
                                &update_sql,
                                &std::panic::Location::caller().to_string(),
                            );
                        }
                    }
                }
                Err(e) => {
                    init_query_error(&sql, e, &std::panic::Location::caller().to_string());
                }
            }
        });
        tasks.push(task);
    }

    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }

    for (id, _) in inst_geo_ids {
        if let Some(id) = id {
            let h = id.to_raw();
            if !EXIST_MESH_GEO_HASHES.contains_key(&h) {
                if let Some(aabb) = aabb_map.get(&h) {
                    EXIST_MESH_GEO_HASHES.insert(h, *aabb);
                }
            }
        }
    }

    utils::save_pts_to_surreal(&pts_json_map).await;
    utils::save_aabb_to_surreal(&aabb_map).await;

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: Thing,
    pub refno: RefnoEnum,
    pub noun: String,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    pub world_trans: Transform,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    pub trans: Transform,
    pub aabb: Aabb,
}

///刷新inst_relate 的 aabb
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()> {
    const CHUNK: usize = 100;
    // dbg!(refnos);
    let aabb_map = DashMap::new();
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let mut rstar_objs = Vec::new();
        let inst_keys = get_inst_relate_keys(chunk);
        let mut sql = format!(
            r#"select id, in as refno, world_trans.d as world_trans, in.noun as noun,
            (select out.aabb.d as aabb, trans.d as trans from out->geo_relate where out.aabb.d != none and trans.d != none)
            as geo_aabbs from {inst_keys} where world_trans.d != none"#,
        );
        //替换所有的aabb
        if !replace_exist {
            sql.push_str(" and aabb=none");
        }
        let mut response = SUL_DB.query(sql).await.unwrap();
        let Ok(result) = response.take::<Vec<QueryAabbParam>>(0) else {
            continue;
        };
        let mut update_sql = String::new();
        for r in result {
            // dbg!(&r);
            let mut aabb = Aabb::new_invalid();
            for g in r.geo_aabbs {
                let t = r.world_trans * g.trans;
                let tmp_aabb = g.aabb.scaled(&t.scale.into());
                let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                    rotation: t.rotation.into(),
                    translation: t.translation.into(),
                });
                aabb.merge(&tmp_aabb);
            }
            // dbg!(aabb.extents().magnitude());
            if aabb.extents().magnitude().is_nan() || aabb.extents().magnitude().is_infinite() {
                #[cfg(feature = "debug_model")]
                dbg!("Found nan aabb");
                continue;
            }
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb).to_string();
            aabb_map.entry(aabb_hash.clone()).or_insert(aabb);
            rstar_objs.push(RStarBoundingBox::new(aabb, r.refno, r.noun));
            let sql = format!(
                "update {} set aabb = aabb:⟨{}⟩;",
                r.refno.to_inst_relate_key(),
                aabb_hash,
            );
            //todo 如果没有transform，直接按None处理，都是默认Transform::IDENTITY
            // dbg!(&sql);
            update_sql.push_str(&sql);
        }
        if !update_sql.is_empty() {
            // dbg!(&update_sql);
            SUL_DB.query(&update_sql).await.unwrap();
        }
        //更新Rstar
        GLOBAL_AABB_TREE.write().await.update_aabbs(rstar_objs);
    }
    utils::save_aabb_to_surreal(&aabb_map).await;

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
    #[serde(default)]
    pub para_type: String,
    pub trans: Transform,
    pub aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ManiGeoTransQuery {
    pub refno: RefnoEnum,
    pub sesno: u32,
    pub noun: String,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(String, Transform)>,
    pub neg_ts: Vec<(RefnoEnum, Transform, Vec<NegInfo>)>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParamNegInfo {
    // pub id: String,
    pub param: PdmsGeoParam,
    pub geo_type: String,
    pub para_type: String,
    pub trans: Transform,
    pub aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OccGeoTransQuery {
    pub refno: RefnoEnum,
    pub noun: String,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(PdmsGeoParam, Transform)>,
    pub neg_ts: Vec<(RefnoEnum, Transform, Vec<ParamNegInfo>)>,
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

pub async fn apply_insts_boolean_occ(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    let mut sql = format!(
        r#"
            select
                in as refno,
                in.noun as noun,
                world_trans.d as wt,
                aabb.d as aabb,
                (select value [out.param, trans.d] from out->geo_relate) as ts,
                (select value [in, world_trans.d, (select out.param as param, geo_type, trans.d as trans,
                out.aabb.d as aabb, object::keys(out.param)[0] as para_type
                from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"] and trans.d != NONE )]
            from array::flatten(in<-neg_relate.in->inst_relate) ) as neg_ts from {} where in.id != none and !bad_bool
            and (in<-neg_relate)[0] != none and aabb.d!=none
        "#,
        inst_keys
    );
    // if !replace_exist
    {
        sql.push_str(" and !booled");
    }
    match SUL_DB.query(&sql).await {
        Ok(mut response) => {
            match response.take::<Vec<OccGeoTransQuery>>(0) {
                Ok(boolean_query) => {
                    // #[cfg(debug_assertions)]
                    // println!("occ inst boolean len: {}", boolean_query.len());
                    // dbg!(boolean_query.len());
                    // dbg!(&boolean_query);
                    if boolean_query.is_empty() {
                        return Ok(());
                    }

                    let chunk = (boolean_query.len() / 16).max(1);
                    for chunk in boolean_query.chunks(chunk) {
                        let group = chunk.to_vec();
                        let dir_clone = dir.clone();
                        // let shapes_map_clone = shapes_map_arc.clone();
                        // let task = tokio::spawn(async move {
                        let mut update_sql = String::new();
                        for mut b in group {
                            if b.ts.is_empty() {
                                continue;
                            }
                            let Some((pos_param, pos_t)) = b.ts.pop() else {
                                continue;
                            };
                            let inst_relate_id = b.refno.to_table_key("inst_relate");
                            //没有实体的情况，下次就不要再继续计算布尔运算了
                            let Ok(mut pos_shape) = pos_param.gen_occ_shape() else {
                                update_sql.push_str(&format!(
                                    "update {} set bad_bool=true;",
                                    &inst_relate_id
                                ));
                                continue;
                            };
                            let pos_matrix = pos_t.compute_matrix().as_dmat4();
                            // dbg!(pos_matrix);
                            let Ok(mut pos_shape) = pos_shape.transformed(&pos_matrix) else {
                                update_sql.push_str(&format!(
                                    "update {} set bad_bool=true;",
                                    &inst_relate_id
                                ));
                                continue;
                            };

                            for (param, t) in b.ts.iter() {
                                // dbg!(id);
                                if let Ok(shape) = param.gen_occ_shape() {
                                    if let Ok(s) = shape.transformed(&t.compute_matrix().as_dmat4())
                                    {
                                        pos_shape = pos_shape.union(&s.0).shape.into();
                                    }
                                }
                            }
                            // dbg!(b.refno);
                            let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();

                            #[cfg(feature = "debug_model")]
                            pos_shape.write_step(format!("{}.step", "pos")).unwrap();
                            // dbg!(b.neg_ts.len());
                            let mut neg_shapes = vec![];
                            let mut cross_neg_shapes = vec![];
                            for (refno, neg_t, negs) in b.neg_ts.into_iter() {
                                for ParamNegInfo {
                                    param,
                                    geo_type,
                                    para_type,
                                    trans,
                                    aabb,
                                } in negs
                                {
                                    if aabb.is_none() {
                                        // dbg!(&id);
                                        continue;
                                    }
                                    if let Ok(neg_shape) = param.gen_occ_shape() {
                                        let m = round_dmat4(
                                            inverse_mat
                                                * neg_t.compute_matrix().as_dmat4()
                                                * trans.compute_matrix().as_dmat4(),
                                        );
                                        // dbg!(m);
                                        // dbg!(refno);
                                        if let Ok(t_neg_shape) = neg_shape.0.transformed_by_gmat(&m)
                                        {
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
                                let inst_relate_id = b.refno.to_table_key("inst_relate");
                                if let Ok(pos_shape) = pos_shape.subtract_shapes(&neg_shapes, false)
                                {
                                    if let Ok(final_shape) =
                                        pos_shape.subtract_shapes(&cross_neg_shapes, true)
                                    {
                                        let tol = b.aabb.half_extents().magnitude() * 0.01;
                                        #[cfg(feature = "debug_model")]
                                        {
                                            final_shape
                                                .write_step(format!("{}.step", b.refno))
                                                .unwrap();
                                        }
                                        if let Ok(mesh) =
                                            PlantMesh::gen_occ_mesh(&final_shape, tol as _)
                                        {
                                            //保存到文件到dir下
                                            if mesh
                                                .ser_to_file(
                                                    &dir_clone.join(format!("{}.mesh", b.refno)),
                                                )
                                                .is_ok()
                                            {
                                                update_sql.push_str(&format!(
                                                    "update {} set booled=true;",
                                                    &inst_relate_id
                                                ));
                                                success = true;
                                            }
                                        }
                                    }
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
                            let r = SUL_DB.query(&update_sql).await;
                            if let Err(_e) = r {
                                init_save_database_error(
                                    &update_sql,
                                    &std::panic::Location::caller().to_string(),
                                );
                            }
                        }
                        // });
                        // tasks.push(task);
                    }
                }
                Err(e) => {
                    init_deserialize_error(
                        "Vec<OccGeoTransQuery>",
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CataNegGroup {
    pub refno: RefnoEnum,
    pub inst_info_id: Thing,
    pub boolean_group: Vec<Vec<RefnoEnum>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GmGeoData {
    pub id: String,
    pub geom_refno: RefnoEnum,
    pub trans: Transform,
    pub param: PdmsGeoParam,
    //暂时aabb 不变
    pub aabb_id: Thing,
}

//处理元件库有负实体的布尔运算
pub async fn apply_cata_neg_boolean_occ(dir: PathBuf) -> anyhow::Result<()> {
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
                    select record::id(out) as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
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
                        .map(|x| x.transformed(&pos.trans.compute_matrix().as_dmat4()))
                    else {
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
                        let Ok(neg_shape) = neg_geo.param.gen_occ_shape() else {
                            continue;
                        };
                        if let Ok(t_neg_shape) = neg_shape
                            .0
                            .transformed_by_gmat(&neg_geo.trans.compute_matrix().as_dmat4())
                        {
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
