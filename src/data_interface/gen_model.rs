use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::aql_api::children::query_children_order_aql;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn::geo::create_profile_geos;
use crate::consts::*;
use crate::data_interface::db_manager::GeoEnum;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::save_arangodb_doc;
use crate::graph_db::pdms_inst_arango::*;
use crate::graph_db::pdms_mesh_arango::{save_mesh_data, save_mesh_to_local_db};
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::ScomInfo;
use aios_core::pe::SPdmsElement;
use aios_core::prim_geo::category::{convert_to_brep_shapes, CateBrepShape};
use aios_core::prim_geo::cylinder::SCylinder;
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::prim_geo::polyhedron::{Polygon, Polyhedron};
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::sbox::SBox;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdge, TubiSize};
use aios_core::prim_geo::*;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{pdms_types::*, RefU64};
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::{DMat4, Mat4, Vec3};
use glam::{DVec3, Mat3};
use nalgebra::Point3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::mem::take;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// 生成模型暂时现在用的本地缓存，这样生成的速度会加快，如果增量更新过来的数据
/// 先保存到sled，然后调用增量更新的

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    prim_refnos: &[RefU64],
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let db_option = &mgr.db_option;
    let batch_size = db_option.gen_model_batch_size;
    let prim_cnt = prim_refnos.len();
    if prim_cnt == 0 {
        return Ok(true);
    }
    let batch_chunks_cnt = prim_cnt / batch_size + 1;
    let mut handles = vec![];
    let all_refnos = Arc::new(prim_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(prim_cnt));
    let replace_mesh = db_option.replace_mesh;
    let tol_ratio = db_option.mesh_tol_ratio;
    for i in 0..batch_chunks_cnt as usize {
        let mgr_clone = mgr.clone();
        let instance_mgr = instance_mgr.clone();

        let all_refnos = all_refnos.clone();
        let processed_cnt = processed_cnt.clone();
        let handle = tokio::spawn(async move {
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > prim_cnt as usize {
                end_idx = prim_cnt as usize;
            }
            for j in start_idx..end_idx {
                let mut cached_mesh_mgr = mgr_clone.cached_mesh_mgr.write().await;
                let mut shape_insts_data = instance_mgr.write().await;
                let refno = all_refnos[j];
                println!(
                    "正在处理基本体的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let Ok(Some(mut trans_origin)) = mgr_clone.get_world_transform(refno).await else {
                    continue;
                };
                let mut geo_insts = vec![];
                let mut item_trans = Transform::IDENTITY;

                // let attr = mgr_clone.aios_core::get_named_attmap(refno).await.unwrap_or_default();
                let attr = aios_core::get_named_attmap(refno).await.unwrap_or_default();
                let mut geos_info = EleGeosInfo {
                    refno,
                    visible: true,
                    generic_type: mgr_clone.get_generic_type(refno).await,
                    aabb: None,
                    world_transform: trans_origin,
                    cata_hash: None,
                    flow_pt_indexs: vec![],
                    geo_type: if attr.is_neg() {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                };
                let mut geo_param = PdmsGeoParam::Unknown;
                //需要限制负实体的大小，太大，导致负运算失败
                let limit_size: Option<f32> = if GENRAL_NEG_NOUN_NAMES
                    .contains(&attr.get_type_str())
                {
                    if let Some(parent_inst) = shape_insts_data.inst_info_map.get(&attr.get_owner())
                    {
                        parent_inst
                            .aabb
                            .map(|x| x.bounding_sphere().radius * 2000.0)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let brep_shape = if attr.get_type_str() == "POHE" {
                    // let pgo_refnos = mgr_clone
                    //     .get_children_from_localdb(refno)
                    //     .unwrap_or_default();
                    let pgo_refnos = aios_core::get_children_refnos(refno)
                        .await
                        .unwrap_or_default();
                    let mut polygons = vec![];
                    for pgo_refno in pgo_refnos {
                        let mut verts = vec![];
                        // let v_att = mgr_clone.get_children_attrs(pgo_refno).unwrap_or_default();
                        //todo 改成只获取需要的数据
                        //使用macro 也可以
                        let v_att = aios_core::get_children_named_attmaps(pgo_refno)
                            .await
                            .unwrap_or_default();
                        for v in v_att {
                            verts.push(v.get_position().unwrap_or_default());
                        }
                        polygons.push(Polygon { verts });
                    }
                    let obj: Box<dyn BrepShapeTrait> = Box::new(Polyhedron { polygons });
                    Some(obj)
                } else {
                    attr.create_brep_shape(limit_size)
                };
                let Some(brep_shape) = brep_shape else {
                    continue;
                };
                if !brep_shape.check_valid() {
                    continue;
                }

                item_trans = brep_shape.get_trans();
                if item_trans.is_nan() {
                    continue;
                }
                geo_param = brep_shape
                    .convert_to_geo_param()
                    .unwrap_or(PdmsGeoParam::Unknown);
                let geo_hash = brep_shape.hash_unit_mesh_params();
                // dbg!(geo_hash);
                let geo_aabb = {
                    let tmp_tol = if attr.is_neg() { tol_ratio } else { tol_ratio };
                    let Some((_, aabb)) =
                        cached_mesh_mgr.gen_plant_data(brep_shape, replace_mesh, tmp_tol)
                    else {
                        continue;
                    };
                    aabb
                };
                // dbg!(&attr);
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                // dbg!(visible);
                geos_info.visible = visible;
                let tr = &item_trans;
                let ele_aabb = aabb_apply_transform(&geo_aabb, &tr);
                let inst_geo = EleInstGeo {
                    geo_hash,
                    refno,
                    owner_pos_refnos: Default::default(),
                    pts: Default::default(),
                    aabb: Some(geo_aabb),
                    transform: *tr,
                    geo_param,
                    visible,
                    is_tubi: false,
                    geo_type: if attr.is_neg() {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                };
                geo_insts.push(inst_geo);
                geos_info.aabb = Some(ele_aabb.transform_by(&Isometry {
                    rotation: trans_origin.rotation.into(),
                    translation: trans_origin.translation.into(),
                }));
                if geo_insts.len() > 0 {
                    shape_insts_data.insert_info(refno, geos_info);
                    shape_insts_data.insert_geos_data(
                        refno.to_url_refno(),
                        EleInstGeosData {
                            inst_key: refno.to_string(),
                            refno,
                            insts: geo_insts,
                            aabb: Some(ele_aabb),
                            type_name: attr.get_type_str().to_string(),
                            ptset_map: Default::default(),
                        },
                    );
                }
            }
        });
        handles.push(handle);
        if !db_option.multi_threads {
            if !handles.is_empty() {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
    }
    futures::future::join_all(take(&mut handles)).await;
    println!(
        "处理常规基本几何体: {} 花费时间: {} ms",
        prim_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

///处理带有loop的元件
pub async fn gen_loop_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    loop_refnos: &[RefU64],
    sjus_map_arc: Arc<DashMap<RefU64, (Vec3, f32)>>,
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let db_option = &mgr.db_option;
    let mut batch_size = mgr.db_option.gen_model_batch_size;
    let loop_cnt = loop_refnos.len();
    if loop_cnt == 0 {
        return Ok(true);
    }
    //处理loop elements
    let mut batch_chunks_cnt = (loop_cnt / batch_size + 1);
    let mut handles = vec![];
    let all_refnos = Arc::new(loop_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(loop_cnt));
    let replace_mesh = db_option.replace_mesh;
    let tol_ratio = db_option.mesh_tol_ratio;
    for i in 0..batch_chunks_cnt as usize {
        let mgr = mgr.clone();
        let instance_mgr = instance_mgr.clone();
        let all_loop_refnos = all_refnos.clone();
        let processed_cnt = processed_cnt.clone();
        let sjus_map_clone = sjus_map_arc.clone();
        let handle = tokio::spawn(async move {
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > loop_cnt as usize {
                end_idx = loop_cnt as usize;
            }
            let mut cached_mesh_mgr = mgr.cached_mesh_mgr.write().await;
            let mut shape_insts_data = instance_mgr.write().await;
            for j in start_idx..end_idx {
                let loop_refno = all_loop_refnos[j];
                let Ok(Some(ce_pe)) = aios_core::get_pe(loop_refno).await else {
                    continue;
                };
                // let parent_basic = mgr.get_owner_ref_basic(loop_refno).unwrap();
                let parent_refno = ce_pe.get_owner();
                let Ok(Some(owner_pe)) = aios_core::get_pe(parent_refno).await else {
                    continue;
                };
                //todo get bacsic type
                let target_type = owner_pe.get_type_str();
                let cur_type = ce_pe.get_type_str();
                let mut parent_att = aios_core::get_named_attmap(parent_refno)
                    .await
                    .unwrap_or_default();
                let Ok(Some(mut trans_origin)) = mgr.get_world_transform(loop_refno).await else {
                    continue;
                };
                //判断父节点是否有SJUS，需要调整位置
                if cur_type == "PLOO"
                    && let Some(sjus_adjust) = sjus_map_clone.get(&parent_refno)
                {
                    let offset = trans_origin.rotation.mul_vec3(sjus_adjust.value().0);
                    trans_origin.translation += offset;
                }
                println!(
                    "正在处理loops类型的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    parent_refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let mut loop_verts: Vec<Vec3> = vec![];
                let mut fradius_vec: Vec<f32> = vec![];

                if let Ok(children_atts) = aios_core::get_children_named_attmaps(loop_refno).await {
                    for a in children_atts {
                        let pt = a.get_position().unwrap_or_default();
                        if loop_verts.len() > 0 {
                            if pt.distance(*loop_verts.last().unwrap()) > f32::EPSILON {
                                loop_verts.push(pt);
                                fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
                            }
                        } else {
                            loop_verts.push(pt);
                            fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
                        }
                    }
                }
                if loop_verts.is_empty() {
                    continue;
                }

                let mut children_attmaps = aios_core::get_children_named_attmaps(parent_refno)
                    .await
                    .unwrap_or_default();
                let cur_sibling_index = children_attmaps
                    .iter()
                    .filter(|&x| x.get_type_str() == "PLOO")
                    .position(|x| x.get_refno().unwrap_or_default() == loop_refno)
                    .unwrap_or_default();
                let mut geos_info = EleGeosInfo {
                    refno: parent_refno,
                    cata_hash: None,
                    visible: true,
                    world_transform: trans_origin,
                    generic_type: mgr.get_generic_type(parent_refno).await,
                    aabb: None,
                    flow_pt_indexs: vec![],
                    geo_type: if parent_att.is_neg() {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                };
                let mut geo_hash = 0;
                let mut geo_aabb = None;
                let mut item_trans = Transform::IDENTITY;
                let mut geo_param = PdmsGeoParam::Unknown;
                match target_type {
                    "NREV" | "REVO" => {
                        let angle = parent_att.get_f32("ANGL").unwrap_or_default();
                        if angle.abs() >= f32::EPSILON {
                            let revo = Box::new(Revolution {
                                verts: loop_verts,
                                fradius_vec,
                                angle,
                                ..Default::default()
                            });
                            if revo.check_valid() {
                                // dbg!(&revo);
                                item_trans = revo.get_trans();
                                geo_param =
                                    revo.convert_to_geo_param().unwrap_or(PdmsGeoParam::Unknown);
                                geo_hash = revo.hash_unit_mesh_params();
                                geo_aabb = {
                                    let tmp_tol = if parent_att.is_neg() {
                                        tol_ratio.map(|x| x * 2.0)
                                    } else {
                                        tol_ratio
                                    };
                                    let Some((_, aabb)) =
                                        cached_mesh_mgr.gen_plant_data(revo, replace_mesh, tmp_tol)
                                    else {
                                        continue;
                                    };
                                    Some(aabb)
                                };
                            }
                        }
                    }
                    //todo 关于justline，可能需要jusline的信息才能判断中心点
                    "AEXTR" | "NXTR" | "EXTR" | "PANE" | "FLOOR" | "SCREED" | "GWALL" => {
                        let loop_attr = aios_core::get_named_attmap(loop_refno)
                            .await
                            .unwrap_or_default();
                        //不是第一个loop，需要取第一个的loop的height
                        let mut height = if cur_sibling_index > 0 {
                            aios_core::get_named_attmap(children_attmaps[0].get_refno_or_default())
                                .await
                                .unwrap_or_default()
                                .get_f32("HEIG")
                                .unwrap_or_default()
                        } else {
                            loop_attr
                                .get_f32("HEIG")
                                .unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default())
                        };
                        if height < f32::EPSILON {
                            println!("{}： 的height太小为: {}", parent_refno, height);
                            continue;
                        }
                        if loop_attr.get_type_str() == "NXTR" {
                            if let Some(parent_inst) =
                                shape_insts_data.get_inst_info(loop_attr.get_owner())
                            {
                                if let Some(h) =
                                    parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0)
                                {
                                    height = height.min(h);
                                    // dbg!(height);
                                    println!("Height 太长，裁剪为: {}", height);
                                }
                            }
                        };
                        let extrusion = Box::new(Extrusion {
                            verts: loop_verts,
                            height,
                            fradius_vec,
                            ..Default::default()
                        });
                        geo_param = extrusion
                            .convert_to_geo_param()
                            .unwrap_or(PdmsGeoParam::Unknown);
                        item_trans = extrusion.get_trans();

                        geo_hash = extrusion.hash_unit_mesh_params();
                        geo_aabb = {
                            let Some((_, aabb)) =
                                cached_mesh_mgr.gen_plant_data(extrusion, replace_mesh, tol_ratio)
                            else {
                                continue;
                            };
                            Some(aabb)
                        };
                    }
                    _ => {}
                }
                let Some(mut geo_aabb) = geo_aabb else {
                    println!("LOOP 有问题：{} ", loop_refno.to_refno_string());
                    continue;
                };
                let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                if item_trans.is_nan() {
                    continue;
                }
                let tr: Transform = item_trans;
                let ele_aabb = aabb_apply_transform(&geo_aabb, &tr);
                //需要判断多个PLOO、LOOP的情况，第二个开始都是负实体
                let geom_inst = EleInstGeo {
                    geo_hash,
                    refno: parent_refno,
                    owner_pos_refnos: Default::default(),
                    pts: Default::default(),
                    aabb: Some(geo_aabb),
                    transform: tr,
                    visible,
                    is_tubi: false,
                    geo_param: geo_param.clone(),
                    geo_type: if parent_att.is_neg() || cur_sibling_index >= 1 {
                        GeoBasicType::Neg
                    } else {
                        GeoBasicType::Pos
                    },
                };
                geos_info.aabb = Some(aabb_apply_transform(&ele_aabb, &trans_origin));
                shape_insts_data.insert_info(parent_refno, geos_info.clone());
                shape_insts_data.insert_geos_data(
                    parent_refno.to_url_refno(),
                    EleInstGeosData {
                        inst_key: parent_refno.to_url_refno(),
                        refno: parent_refno,
                        insts: vec![geom_inst.clone()],
                        aabb: Some(ele_aabb),
                        type_name: parent_att.get_type_str().to_string(),
                        ptset_map: Default::default(),
                    },
                );
            }
        });

        handles.push(handle);
        if !db_option.multi_threads {
            if !handles.is_empty() {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
    }
    futures::future::join_all(take(&mut handles)).await;
    println!(
        "处理loops几何体: {} 花费时间: {} ms",
        loop_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

///生成mesh的逻辑单独拿出来
pub fn gen_mesh() -> anyhow::Result<()> {
    Ok(())
}

///获取单个元件的模型数据
pub async fn gen_cata_single_geoms(
    mgr: Arc<AiosDBManager>,
    design_refno: RefU64,
    brep_shape_map: &CateBrepShapeMap,
    refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>,
) -> anyhow::Result<RefU64> {
    let desi_att = aios_core::get_named_attmap(design_refno).await?;
    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        return Ok(RefU64::default());
    }
    let geoms_info = mgr.resolve_desi_comp(design_refno, None, None).await?;
    if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" || type_name == "WALL"
    {
        create_profile_geos(
            design_refno,
            &desi_att,
            &geoms_info,
            &brep_shape_map,
            mgr.as_ref(),
        )
        .await?;
        return Ok(geoms_info.refno);
    } else {
        let CateGeomsInfo {
            refno,
            geometries,
            n_geometries,
            axis_map,
        } = geoms_info;
        for geom in geometries {
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                brep_shape_map
                    .entry(design_refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
            }
        }
        for geom in n_geometries {
            if let Some(mut cate_shape) = convert_to_brep_shapes(&geom) {
                cate_shape.is_ngmr = true;
                brep_shape_map
                    .entry(design_refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
            }
        }
        refno_ptset_map.insert(design_refno, axis_map);
        return Ok(refno);
    }
}

///针对aabb，应用transform
#[inline]
fn aabb_apply_transform(aabb: &Aabb, t: &Transform) -> Aabb {
    let a = aabb.scaled(&t.scale.into());
    let transformed_aabb = a.transform_by(&Isometry {
        rotation: t.rotation.into(),
        translation: t.translation.into(),
    });
    transformed_aabb
}

///计算对齐偏移值
#[inline]
fn cal_sjus_value(sjus: &str, height: f32) -> f32 {
    let off_z = if sjus == "UTOP" || sjus == "DTOP" || sjus == "TOP" {
        height
    } else if sjus == "UCEN" || sjus == "DCEN" || sjus == "CENT" {
        height / 2.0
    } else {
        0.0
    };
    off_z
}

/// 生成元件库的branch型几何体
pub async fn gen_cata_geos(
    mgr: Arc<AiosDBManager>,
    main_instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<DashMap<RefU64, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefU64, (Vec3, f32)>>,
) -> anyhow::Result<bool> {
    let batch_size = mgr.db_option.gen_model_batch_size;
    let t = Instant::now();
    let unique_cata_cnt = target_cata_map.len();
    let batch_chunks_cnt = unique_cata_cnt / batch_size + 1;
    println!("使用元件库的unique模型总数：{unique_cata_cnt}, 分块数量: {batch_chunks_cnt}");
    let mut handles = vec![];
    let processed_cnt = Arc::new(Mutex::new(unique_cata_cnt));
    let mut tubi_aqls = Arc::new(DashMap::new());
    let replace_mesh = mgr.db_option.replace_mesh;
    let tol_ratio = mgr.db_option.mesh_tol_ratio;
    let multi_threads = mgr.db_option.multi_threads;

    let all_unique_keys = Arc::new(
        target_cata_map
            .iter()
            .map(|x| x.cata_hash.clone())
            .collect::<Vec<_>>(),
    );
    dbg!(&all_unique_keys.len());
    if !all_unique_keys.is_empty() {
        for i in 0..batch_chunks_cnt as usize {
            let mgr_clone = mgr.clone();
            let instance_mgr = main_instance_mgr.clone();
            let all_unique_keys = all_unique_keys.clone();
            let processed_cnt = processed_cnt.clone();
            let target_cata_map = target_cata_map.clone();
            let sjus_map_clone = sjus_map_arc.clone();

            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > unique_cata_cnt as usize {
                    end_idx = unique_cata_cnt as usize;
                }
                println!("当前范围: {start_idx} ~ {end_idx}");
                for j in start_idx..end_idx {
                    let cata_hash = all_unique_keys[j].clone();
                    if cata_hash == "0" {
                        continue;
                    }
                    let target_cata = target_cata_map.get(&cata_hash).unwrap();
                    let mut cached_mesh_mgr = mgr_clone.cached_mesh_mgr.write().await;
                    let mut shape_insts_data = instance_mgr.write().await;
                    let mut target_geo_data_option = None;
                    let mut process_refno = None;
                    if replace_mesh || target_cata.exist_geo.is_none() {
                        //如果没有已有的，需要生成
                        let ele_refno = target_cata.group_refnos[0];
                        process_refno = Some(ele_refno);
                        println!(
                            "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                            j,
                            ele_refno.to_refno_string(),
                            processed_cnt.lock().await.to_owned()
                        );
                        *processed_cnt.lock().await -= 1;
                        //在这里直接处理完所有需要处理的transform
                        let brep_shapes_map = CateBrepShapeMap::new();
                        let current_att = aios_core::get_named_attmap(ele_refno)
                            .await
                            .unwrap_or_default();
                        let mut refno_ptset_map = DashMap::new();
                        let cur_type = current_att.get_type_str();

                        let r = gen_cata_single_geoms(
                            mgr_clone.clone(),
                            ele_refno,
                            &brep_shapes_map,
                            &refno_ptset_map,
                        )
                        .await;
                        let cat_refno = match r {
                            Ok(cat_refno) => cat_refno,
                            Err(e) => {
                                println!("生成元件库模型失败: {:?}", e);
                                continue;
                            }
                        };
                        // dbg!(&brep_shapes_map);
                        ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                        for (ele_refno, shapes) in brep_shapes_map {
                            let Ok(Some(mut origin_trans)) =
                                mgr_clone.get_world_transform(ele_refno).await
                            else {
                                continue;
                            };
                            // dbg!((ele_refno, origin_trans));
                            let Ok(ele_att) = aios_core::get_named_attmap(ele_refno).await else {
                                continue;
                            };

                            if let Some(sjus) = ele_att.get_str("SJUS") {
                                let parent = ele_att.get_owner();
                                if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                    let height = sjus_adjust.value().1;
                                    let off_z = cal_sjus_value(sjus, height);
                                    let parent_trans = mgr_clone
                                        .get_world_transform(parent)
                                        .await
                                        .unwrap_or_default()
                                        .unwrap_or_default();

                                    origin_trans.translation.z = parent_trans.translation.z;
                                    origin_trans.translation = origin_trans.translation
                                        + sjus_adjust.value().0
                                        + Vec3::new(0.0, 0.0, off_z);
                                }
                            }

                            let Ok(Some(gmse_refno)) = aios_core::query_single_by_paths(
                                cat_refno,
                                &["->GMRE", "->GSTR"],
                                &["refno"],
                            )
                            .await
                            .map(|x| x.get_refno_lossy()) else {
                                continue;
                            };
                            dbg!(gmse_refno);

                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            let pos_neg_map: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> =
                                if gmse_refno.is_valid() {
                                    // mgr_clone
                                    //     .query_refnos_has_pos_neg_map(&[gmse_refno])
                                    //     .await
                                    //     .unwrap_or_default()
                                    HashMap::new()
                                } else {
                                    HashMap::new()
                                };
                            let mut neg_own_pos_map: HashMap<RefU64, RefU64> = pos_neg_map
                                .iter()
                                .map(|(k, (poss, negs))| negs.iter().map(|x| (*x, *k)))
                                .flatten()
                                // .cloned()
                                .collect();
                            //如果有负实体，需要合在一起
                            let mut geos_info = EleGeosInfo {
                                refno: ele_refno,
                                cata_hash: Some(cata_hash.clone()),
                                visible: true,
                                generic_type: mgr_clone.get_generic_type(ele_refno).await,
                                aabb: None,
                                world_transform: origin_trans,
                                flow_pt_indexs: if !ele_att.contains_key("ARRI") {
                                    vec![]
                                } else {
                                    vec![
                                        ele_att.get_i32("ARRI").unwrap_or(-1),
                                        ele_att.get_i32("LEAV").unwrap_or(-1),
                                    ]
                                },
                                geo_type: Default::default(),
                            };

                            let mut manifold_map: HashMap<RefU64, ManifoldRust> = HashMap::new();
                            let mut geo_insts = vec![];
                            let mut ngmr_geo_insts = vec![];
                            //将负实体和正实体统计出来
                            let mut merged_cata_aabb: Option<Aabb> = None;
                            let mut n_merged_cata_aabb: Option<Aabb> = None;
                            // dbg!(ele_refno);
                            //直接将所有的几何体组合起来
                            for shape in shapes {
                                let CateBrepShape {
                                    refno,
                                    brep_shape,
                                    transform,
                                    visible,
                                    is_tubi,
                                    pts,
                                    is_ngmr,
                                    ..
                                } = shape;
                                if !brep_shape.check_valid() {
                                    continue;
                                }
                                let mut trans = brep_shape.get_trans();
                                let is_neg = neg_own_pos_map.contains_key(&refno);
                                let geo_hash = brep_shape.hash_unit_mesh_params();
                                let mut geo_aabb = {
                                    let mut tol = if is_neg {
                                        tol_ratio.unwrap_or(1.0) * 50.0
                                    } else {
                                        tol_ratio.unwrap_or(1.0)
                                    };
                                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(
                                        brep_shape.clone(),
                                        replace_mesh,
                                        Some(tol),
                                    ) else {
                                        continue;
                                    };
                                    aabb
                                };
                                let rot = transform.rotation;
                                let translation =
                                    transform.translation + transform.rotation * trans.translation;
                                let scale = trans.scale;
                                let tmp_aabb = geo_aabb.scaled(&trans.scale.into());
                                let transformed_aabb = tmp_aabb.transform_by(&Isometry {
                                    rotation: rot.into(),
                                    translation: translation.into(),
                                });

                                if let Some(mesh) = cached_mesh_mgr.get_mesh(geo_hash) {
                                    let mut local_mat = Transform {
                                        translation,
                                        rotation: rot,
                                        scale,
                                    }
                                    .compute_matrix()
                                    .as_dmat4();
                                    let Some(aabb) = cached_mesh_mgr.get_aabb(geo_hash) else {
                                        continue;
                                    };
                                    //稍微扩张一点
                                    if is_neg {
                                        let center: Vec3 = aabb.center().into();
                                        let mut center = center.as_dvec3();
                                        let t_mat = DMat4::from_translation(center);
                                        let mut s = 1.001;
                                        let s_mat = DMat4::from_scale(DVec3::splat(s));
                                        let inv_t_mat = DMat4::from_translation(-center);
                                        local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                                    }
                                    let new_mesh = mesh.transform_by(&(local_mat));
                                    manifold_map.insert(refno, new_mesh.into());
                                };
                                if is_ngmr {
                                    if let Some(a) = &mut n_merged_cata_aabb {
                                        a.merge(&&transformed_aabb);
                                    } else {
                                        n_merged_cata_aabb = Some(transformed_aabb);
                                    }
                                } else {
                                    if let Some(a) = &mut merged_cata_aabb {
                                        a.merge(&transformed_aabb);
                                    } else {
                                        merged_cata_aabb = Some(transformed_aabb);
                                    }
                                }
                                let transform = Transform {
                                    translation,
                                    rotation: rot,
                                    scale,
                                };
                                if transform.is_nan() {
                                    continue;
                                }
                                let geom_inst = EleInstGeo {
                                    geo_hash,
                                    refno,
                                    pts,
                                    aabb: Some(geo_aabb),
                                    transform,
                                    geo_param: brep_shape
                                        .convert_to_geo_param()
                                        .unwrap_or(PdmsGeoParam::Unknown),
                                    visible,
                                    is_tubi,
                                    geo_type: if is_ngmr {
                                        GeoBasicType::CateCrossNeg
                                    } else if is_neg {
                                        GeoBasicType::CateNeg
                                    } else {
                                        GeoBasicType::Pos
                                    },
                                    owner_pos_refnos: if is_ngmr {
                                        Default::default()
                                    } else {
                                        neg_own_pos_map
                                            .get(&refno)
                                            .cloned()
                                            .map(|x| [x].into())
                                            .unwrap_or_default()
                                    },
                                };
                                if is_ngmr {
                                    ngmr_geo_insts.push(geom_inst.clone());
                                }
                                geo_insts.push(geom_inst);
                                // break;
                            }
                            // dbg!(csg_map.len());
                            if merged_cata_aabb.is_none() {
                                merged_cata_aabb = n_merged_cata_aabb;
                            }
                            //需要变换成世界坐标系下的aabb
                            if let Some(a) = merged_cata_aabb {
                                geos_info.aabb =
                                    Some(aabb_apply_transform(&a, &geos_info.world_transform));
                            }

                            if let Some(mut aabb) = &mut geos_info.aabb {
                                if aabb.mins.x.is_infinite() {
                                    aabb =
                                        Aabb::new(Point3::new(0., 0., 0.), Point3::new(0., 0., 0.));
                                }
                            }

                            //保存ngmr的信息
                            // dbg!(&ngmr_geo_insts);
                            if ngmr_geo_insts.len() > 0 {
                                let mut n_geos_info = geos_info.clone();
                                n_geos_info.update_to_ngmr(None);
                                let mut inst_key = n_geos_info.get_inst_key();
                                let n_origin = EleInstGeosData {
                                    inst_key: inst_key.clone(),
                                    refno: cat_refno,
                                    insts: ngmr_geo_insts,
                                    aabb: n_merged_cata_aabb,
                                    type_name: cur_type.to_string(),
                                    ptset_map: Default::default(),
                                };
                                shape_insts_data.insert_ngmr_info(ele_refno, n_geos_info);
                                shape_insts_data.insert_geos_data(inst_key, n_origin);
                            }

                            //将负实体的运算结果，存在另外一个collection
                            // ----- 处理的是元件库实体内的负实体运算  ----- //
                            {
                                let mut inst_key = geos_info.get_inst_key();
                                let mut origin = EleInstGeosData {
                                    inst_key,
                                    refno: cat_refno,
                                    insts: geo_insts.clone(),
                                    aabb: merged_cata_aabb,
                                    type_name: cur_type.to_string(),
                                    ptset_map: refno_ptset_map
                                        .remove(&ele_refno)
                                        .map(|x| x.1)
                                        .unwrap_or_default(),
                                };
                                shape_insts_data.insert_info(ele_refno, geos_info.clone());
                                target_geo_data_option = Some(origin.clone());
                                shape_insts_data
                                    .insert_geos_data(geos_info.get_inst_key(), origin.clone());

                                //在这里执行负实体的运算
                                let mut final_geo_insts = geo_insts;
                                let mut final_compounds_map = HashMap::new();
                                let mut total_manifolds = vec![];

                                for (&k, (_, neg_vec)) in &pos_neg_map {
                                    if let Some(src_manifold) = manifold_map.get(&k) {
                                        let mut neg_ms = vec![];
                                        for neg in neg_vec {
                                            if let Some(m) = manifold_map.get(neg) {
                                                neg_ms.push(m.clone());
                                            } else {
                                                continue;
                                            }
                                        }
                                        total_manifolds.extend_from_slice(&neg_ms);
                                        //元件库实体内的负实体运算
                                        let final_manifold =
                                            src_manifold.batch_boolean_subtract(&neg_ms);
                                        final_compounds_map.insert(k, final_manifold);
                                        final_geo_insts.retain(|x| x.refno != k);
                                    } else {
                                        continue;
                                    }
                                }
                                //元件库内部的负实体计算
                                if !final_compounds_map.is_empty() {
                                    final_geo_insts.retain(|x| x.geo_type == GeoBasicType::Pos);
                                    let mut compound_geos_info = geos_info;
                                    compound_geos_info.update_to_compound(None);
                                    let inst_key = compound_geos_info.get_inst_key();
                                    for (k, v) in final_compounds_map {
                                        //组合成新的hash
                                        let geo_hash =
                                            hash_two_str(&inst_key.to_string(), &k.to_url_refno());
                                        let mesh: PlantMesh = (v.clone()).into();
                                        let aabb = mesh.cal_aabb();
                                        cached_mesh_mgr.insert(
                                            geo_hash,
                                            PlantGeoData {
                                                geo_hash,
                                                mesh: Some(mesh),
                                                aabb,
                                            },
                                        );
                                        // result_manifold.destroy();
                                        let compound_geom_inst = EleInstGeo {
                                            geo_hash,
                                            refno: k,
                                            owner_pos_refnos: Default::default(),
                                            pts: vec![],
                                            aabb,
                                            transform: Transform::IDENTITY,
                                            geo_param: PdmsGeoParam::CompoundShape,
                                            visible: true,
                                            is_tubi: false,
                                            geo_type: GeoBasicType::Compound,
                                        };
                                        final_geo_insts.push(compound_geom_inst);
                                    }
                                    //compound 需要建立和 inst info 里的关系，是由inst info 合并出来的，所以需要做个edges的处理
                                    let compound_geos_data = EleInstGeosData {
                                        inst_key: inst_key.clone(),
                                        refno: cat_refno,
                                        insts: final_geo_insts,
                                        aabb: origin.aabb.clone(),
                                        type_name: origin.type_name.clone(),
                                        ptset_map: origin.ptset_map.clone(),
                                    };
                                    shape_insts_data.insert_geos_data(inst_key, compound_geos_data);
                                    shape_insts_data
                                        .insert_compound_info(ele_refno, compound_geos_info);
                                }

                                for t in total_manifolds {
                                    // t.destroy();
                                }
                            }
                            break;
                        }
                    } else {
                        target_geo_data_option = target_cata.exist_geo.clone();
                    }

                    //排除一些特殊情况
                    let Some(target_geo_data) = target_geo_data_option else {
                        continue;
                    };
                    if target_geo_data.aabb.is_none() {
                        continue;
                    }
                    for ele_refno in target_cata.group_refnos.clone() {
                        if Some(ele_refno) == process_refno {
                            continue;
                        }
                        println!(
                            "正在处理同类元件库的模型当前参考号：{}",
                            ele_refno.to_refno_string(),
                        );
                        let Ok(Some(mut origin_trans)) =
                            mgr_clone.get_world_transform(ele_refno).await
                        else {
                            continue;
                        };

                        let mut flow_pt_indexs = vec![];
                        let attr = aios_core::get_named_attmap(ele_refno)
                            .await
                            .unwrap_or_default();
                        if let Some(sjus) = attr.get_str("SJUS") {
                            let parent = attr.get_owner();
                            if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                let height = sjus_adjust.value().1;
                                let off_z = cal_sjus_value(sjus, height);
                                origin_trans.translation += sjus_adjust.value().0
                                    + origin_trans.rotation * Vec3::new(0.0, 0.0, off_z);
                            }
                        }

                        //todo 检查使用jusl的模型，需要调正确

                        // if CATA_HAS_TUBI_GEO_NAMES.contains(&attr.get_type_str()) {
                        flow_pt_indexs = vec![
                            attr.get_i32("ARRI").unwrap_or(-1),
                            attr.get_i32("LEAV").unwrap_or(-1),
                        ];
                        // }
                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type: mgr_clone.get_generic_type(ele_refno).await,
                            aabb: Some(aabb_apply_transform(
                                target_geo_data.aabb.as_ref().unwrap(),
                                &origin_trans,
                            )),
                            world_transform: origin_trans,
                            flow_pt_indexs,
                            geo_type: Default::default(),
                        };
                        shape_insts_data.insert_info(ele_refno, geos_info.clone());
                        //如果有负实体，需要特殊处理
                        if target_geo_data.has_cata_neg() {
                            let mut compound_geos_info = geos_info.clone();
                            compound_geos_info.update_to_compound(None);
                            //为了不覆盖，这里需要动一下
                            shape_insts_data.insert_compound_info(ele_refno, compound_geos_info);
                        }

                        //如果有ngmr负实体，需要特殊处理
                        if target_geo_data.has_ngmr() {
                            let mut n_geos_info = geos_info.clone();
                            //更新为ngmr类型
                            n_geos_info.update_to_ngmr(None);
                            let geo_hash = n_geos_info.get_inst_key_u64();
                            //todo 将ngmr的mesh加载到内存，方便后续处理负实体
                            shape_insts_data.insert_ngmr_info(ele_refno, n_geos_info);
                        }
                    }
                }
            });
            handles.push(handle);
            if !multi_threads {
                if !handles.is_empty() {
                    futures::future::join_all(take(&mut handles)).await;
                }
            }
        }
    }
    futures::future::join_all(take(&mut handles)).await;

    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    //直段需要插入一个单位的cylinder
    let mut inst_tubi_map = HashMap::new();

    for b in branch_map.iter() {
        let shape_insts_data = main_instance_mgr.read().await;
        let branch_refno = *b.key();
        // dbg!(branch_refno);
        let Ok(children) = aios_core::get_children_pes(branch_refno).await else {
            continue;
        };
        let Ok(branch_att) = aios_core::get_named_attmap(branch_refno).await else {
            continue;
        };
        // dbg!(&children);
        //可能只有branch 元素需要做一遍求解
        let Ok(Some(branch_transform)) = mgr.get_world_transform(branch_refno).await else {
            continue;
        };
        // dbg!(&branch_att);
        let htube_pt = branch_transform.transform_point(branch_att.get_vec3("HPOS").unwrap());
        let hdir = branch_transform
            .transform_vec3(branch_att.get_vec3("HDIR").unwrap())
            .normalize_or_zero();
        // dbg!(to_pdms_vec_str(&hdir));
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());
        // dbg!(bran_ttube_pt);

        let is_hang = branch_att.get_type_str() == "HANG";
        // dbg!(&branch_att);
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();

        let bran_name = branch_att.get_name_or_default();
        let tubi_att = aios_core::get_named_attmap(h_ref).await.unwrap_or_default();
        // dbg!(&tubi_att);
        let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        // dbg!(&tubi_cat_ref);
        let mut tubi_size =
            query_tubi_size(&mgr, branch_refno, tubi_cat_ref, is_hang, None).await?;
        dbg!(&tubi_size);
        //todo 其实这里应该待定比较好
        let mut tubi_geo_hash = if matches!(tubi_size, TubiSize::BoxSize(_)) {
            BOXI_GEO_HASH
        } else {
            TUBI_GEO_HASH
        };

        let tref = branch_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let mut current_tubing = PdmsTubing {
            leave_refno: branch_refno,
            arrive_refno: tref,
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            leave_ref_dir: None,
            desire_arrive_dir: Default::default(),
            tubi_size,
        };

        // 需要求解出 leave bore
        if children.len() == 0 {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                //需要检查href的方位
                current_tubing.desire_arrive_dir = -current_tubing.get_dir();
                //检查一下方向是否一致，不一致的，不显示，或者加标记位
                if current_tubing.is_dir_ok() {
                    if let Some(t) = current_tubing.get_transform() {
                        inst_tubi_map.insert(
                            branch_refno,
                            EleGeosInfo {
                                refno: branch_refno,
                                cata_hash: Some(tubi_geo_hash.to_string()),
                                visible: true,
                                generic_type: mgr.get_generic_type(branch_refno).await,
                                aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                world_transform: t,
                                flow_pt_indexs: vec![],
                                geo_type: Default::default(),
                            },
                        );
                        // 将 tubi 数据保存到图数据库
                        let key = h_ref.hash_with_another_refno(tref);
                        tubi_aqls.entry(key).or_insert(TubiEdge {
                            _key: key.to_string(),
                            _from: format!(
                                "{AQL_PDMS_ELES_COLLECTION}/{}",
                                current_tubing.leave_refno.to_url_refno()
                            ),
                            _to: format!(
                                "{AQL_PDMS_ELES_COLLECTION}/{}",
                                current_tubing.arrive_refno.to_url_refno()
                            ),
                            start_pt: current_tubing.start_pt,
                            end_pt: current_tubing.end_pt,
                            att_type: branch_att.get_type_str().to_string(),
                            extra_type: "".to_string(),
                            tubi_size,
                            bran_name: bran_name.clone(),
                        });
                    }
                } else {
                    println!("{} 的直段方向有问题", branch_refno.to_refno_string());
                }
            }
            continue;
        }

        let last_child = children.last().unwrap().clone();
        //不包含atta的元件集合
        let mut bran_comp_vec = vec![];
        //第一遍完成后，然后生成tubing
        for ele in children {
            let refno = ele.refno;
            let cur_type = ele.noun.as_str();
            let Some(inst_info) = shape_insts_data.get_inst_info(refno) else {
                dbg!(refno);
                continue;
            };
            let Some(inst_geos_data) = shape_insts_data.get_inst_geos_data(inst_info) else {
                dbg!(inst_info);
                continue;
            };
            println!("正在处理直段{}: {}", cur_type, refno.to_refno_string());
            let world_trans = inst_info.world_transform;
            let axis_map = &inst_geos_data.ptset_map;
            let arrive = inst_info.flow_pt_indexs[0];
            let leave = inst_info.flow_pt_indexs[1];
            //有隐含管段
            // dbg!(axis_map);
            bran_comp_vec.push(refno);
            current_tubing.arrive_refno = refno;
            //ATTA不产生直段
            if cur_type != "ATTA" && axis_map.contains_key(&arrive) {
                let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                let dir = axis_map[&arrive].dir;
                // dbg!(quat_to_pdms_ori_xyz_str(&world_trans.rotation));
                let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                if a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                    current_tubing.end_pt = a_pos;
                    current_tubing.desire_arrive_dir = a_dir;
                    if current_tubing.is_dir_ok() {
                        // dbg!(&current_tubing);
                        if let Some(t) = current_tubing.get_transform() {
                            inst_tubi_map.insert(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: mgr
                                        .get_generic_type(current_tubing.leave_refno)
                                        .await,
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                    geo_type: GeoBasicType::Tubi,
                                },
                            );
                            let key = current_tubing
                                .leave_refno
                                .hash_with_another_refno(current_tubing.arrive_refno);
                            tubi_aqls.entry(key).or_insert(TubiEdge {
                                _key: key.to_string(),
                                _from: format!(
                                    "{AQL_PDMS_ELES_COLLECTION}/{}",
                                    current_tubing.leave_refno.to_url_refno()
                                ),
                                _to: format!(
                                    "{AQL_PDMS_ELES_COLLECTION}/{}",
                                    current_tubing.arrive_refno.to_url_refno()
                                ),
                                start_pt: current_tubing.start_pt,
                                end_pt: current_tubing.end_pt,
                                att_type: ele.noun.clone(),
                                extra_type: "".to_string(),
                                tubi_size: current_tubing.tubi_size,
                                bran_name: bran_name.clone(),
                            });
                        }
                    } else {
                        //#[cfg(debug_assertions)]
                        dbg!(&current_tubing);
                        dbg!(to_pdms_vec_str(&current_tubing.desire_arrive_dir));
                        dbg!(to_pdms_vec_str(&current_tubing.desire_leave_dir));
                        println!("{} 的直段方向有问题", refno.to_refno_string());
                    }
                }
            }
            if axis_map.contains_key(&leave) {
                let dir = axis_map[&leave].dir;
                let ref_dir = axis_map[&leave].ref_dir;
                let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                let l_ref_dir = world_trans.transform_vec3(ref_dir).normalize_or_zero();

                if cur_type == "ATTA" {
                    current_tubing.desire_leave_dir = l_dir;
                    current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                        Some(l_ref_dir)
                    } else {
                        None
                    };
                    // #[cfg(debug_assertions)]
                    // dbg!((refno, to_pdms_vec_str(&current_tubing.desire_leave_dir)));
                    continue;
                }

                let l_pos = world_trans.transform_point(axis_map[&leave].pt);
                let att_map = aios_core::get_named_attmap(refno).await.unwrap_or_default();
                let lstube_ref = att_map.get_foreign_refno("LSTU").unwrap_or_default();
                let lstube_cat_ref = aios_core::get_named_attmap(lstube_ref)
                    .await
                    .unwrap_or_default()
                    .get_foreign_refno("CATR")
                    .unwrap_or_default();

                current_tubing.tubi_size =
                    query_tubi_size(&mgr, refno, lstube_cat_ref, is_hang, Some(axis_map)).await?;
                tubi_geo_hash = if matches!(current_tubing.tubi_size, TubiSize::BoxSize(_)) {
                    BOXI_GEO_HASH
                } else {
                    TUBI_GEO_HASH
                };
                current_tubing.start_pt = l_pos;
                current_tubing.desire_leave_dir = l_dir;
                current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                    Some(l_ref_dir)
                } else {
                    None
                };
            }
            current_tubing.leave_refno = refno;
            //有隐含管段
            //最后一段的管道
            if refno == last_child.refno {
                if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                    //检查是否有一端是世界坐标原点
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
                    //todo 需要取得连接到的，tref的点对应的arrive方向
                    current_tubing.desire_arrive_dir = -current_tubing.desire_leave_dir;
                    if current_tubing.is_dir_ok() {
                        // dbg!(&current_tubing);
                        if let Some(t) = current_tubing.get_transform() {
                            inst_tubi_map.insert(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: mgr
                                        .get_generic_type(current_tubing.leave_refno)
                                        .await,
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                    geo_type: GeoBasicType::Tubi,
                                },
                            );
                            let key = current_tubing
                                .leave_refno
                                .hash_with_another_refno(current_tubing.arrive_refno);
                            tubi_aqls.entry(key).or_insert(TubiEdge {
                                _key: key.to_string(),
                                _from: format!(
                                    "{AQL_PDMS_ELES_COLLECTION}/{}",
                                    current_tubing.leave_refno.to_url_refno()
                                ),
                                _to: format!(
                                    "{AQL_PDMS_ELES_COLLECTION}/{}",
                                    current_tubing.arrive_refno.to_url_refno()
                                ),
                                start_pt: current_tubing.start_pt,
                                end_pt: current_tubing.end_pt,
                                att_type: ele.noun.clone(),
                                extra_type: "".to_string(),
                                tubi_size: current_tubing.tubi_size,
                                bran_name: bran_name.clone(),
                            });
                        }
                    } else {
                        dbg!(current_tubing.desire_arrive_dir);
                        println!("{} 的直段方向有问题", refno.to_refno_string());
                    }
                }
            }
        }
    }

    if !inst_tubi_map.is_empty() {
        let mut main = main_instance_mgr.write().await;
        for (k, v) in inst_tubi_map {
            main.insert_tubi(k, v);
        }
        println!("模型生成完毕,正在保存直段到图数据库");
    }

    let tubi_result = Arc::try_unwrap(tubi_aqls)
        .unwrap()
        .into_iter()
        .map(|x| x.1)
        .collect::<Vec<_>>();
    if !tubi_result.is_empty() {
        let conn = mgr.get_arango_db().await?;
        let json = serde_json::to_value(tubi_result).unwrap_or_default();
        save_arangodb_doc(json, "tubi_edges", &conn, mgr.db_option.replace_dbs).await?;
    }
    println!(
        "处理元件库几何体: {} 花费时间: {} ms",
        unique_cata_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use aios_core::consts::NGMR_OWN_TYPES;
use aios_core::tool::math_tool::*;
use aios_core::SUL_DB;
use num_enum::IntoPrimitive;
use num_enum::TryFromPrimitive;
use std::convert::TryFrom;

#[derive(Debug, Default, IntoPrimitive, Eq, PartialEq, TryFromPrimitive, Copy, Clone)]
#[repr(i32)]
pub enum NgmrRemovedType {
    #[default]
    AsDefault = -1,
    Nothing = 0,
    Attached = 1,
    Owner = 2,
    Item = 3,
    AttachedAndOwner = 4,
    AttachedAndItem = 5,
    OwnerAndItem = 6,
    All = 7,
}

///石家庄 push -> 到北京
///北京 push -> 到石家庄

///暂时还是依赖sled，后面可以考虑surrealdb
///将数据同步到本地数据库
pub async fn sync_to_localdb(mut mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    Ok(())
}

/// 这个要不要做生成分页的批量处理
///生成几何体数据
pub async fn gen_all_geos_data(
    mut mgr: Arc<AiosDBManager>,
    incr_update_log: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    let time = Instant::now();
    //根据需要拉入数据到本地数据库也可以
    let is_incr_update = incr_update_log.is_some();
    if is_incr_update && incr_update_log.as_ref().unwrap().count() == 0 {
        return Ok(false);
    }
    let db_option = &mgr.db_option;
    let project = &mgr.db_option.project_name;
    let mdb = &mgr.db_option.mdb_name;
    let mut debug_root_refnos = mgr
        .db_option
        .debug_root_refnos
        .as_ref()
        .map(|x| {
            x.iter()
                .map(|x| RefU64::from_str(x).unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let is_debug = debug_root_refnos.len() > 0;
    let mut db_nos = mgr.db_option.manual_db_nums.clone().unwrap_or_default();

    // if !is_incr_update && db_nos.is_empty() {
    //     let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
    //     let pool = AiosDBManager::get_db_pool(&url, project).await?;
    //     //todo 通过图数据库查询db_nos
    //     db_nos = query_db_nums_of_mdb(mdb, &mgr.db_option.module, &pool).await?;
    //     db_nos.sort();
    //     println!("当前mdb的所有dbnos: {:?}", db_nos);
    // }
    // dbg!(&db_nos);
    let replace_mesh = mgr.db_option.replace_mesh;
    if is_incr_update || is_debug {
        //处理增量更新，不需要使用db_nos
        db_nos = vec![0];
    }

    for db_no in db_nos {
        if is_incr_update {
            println!(
                "开始处理更新模型数量: {}",
                incr_update_log.as_ref().unwrap().count()
            );
        } else if is_debug {
            println!("开始调试模型: {:?}", &debug_root_refnos);
        } else {
            println!("开始处理db: {db_no}");
        }
        let d_types = &mgr.db_option.debug_refno_types;
        let mut run_cache_cata = d_types.iter().any(|x| x == "CATA");
        let mut run_cache_loop = d_types.iter().any(|x| x == "LOOP");
        let mut run_cache_prim = d_types.iter().any(|x| x == "PRIM");

        let shape_insts_data = ShapeInstancesData::default();
        let instance_mgr = Arc::new(RwLock::new(shape_insts_data));

        let target_dbnos = [db_no];
        let mut root_refnos = vec![];
        // dbg!(&root_refnos);
        if !is_incr_update && !is_debug {
            root_refnos = mgr.get_gen_model_root_refnos(&target_dbnos).await?;
            println!("输入的调试参考号或者db号不正确");
            continue;
        }

        //提前缓存ploo
        let loop_sjus_map = DashMap::new();
        if !is_incr_update {
            //todo 区别，一个是从db nums 里获取，一个是从增量更新数据，debug数据里获取
            let target_ploo_refnos = mgr
                .get_gen_model_target_refnos(GeoEnum::PLOO, &target_dbnos, false)
                .await?;
            for r in target_ploo_refnos {
                let Ok(loop_att) = aios_core::get_named_attmap(r).await else {
                    continue;
                };
                let owner = loop_att.get_owner();
                let mut height = loop_att
                    .get_f32("HEIG")
                    .unwrap_or(loop_att.get_f32("HEIG").unwrap_or_default());
                let sjus = loop_att.get_str("SJUS").unwrap_or_default();
                let off_z = cal_sjus_value(sjus, height);
                //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
                //插入方向和偏移距离
                loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
            }
        }

        let loop_sjus_map_arc = Arc::new(loop_sjus_map);
        //元件库的模型计算
        {
            let target_bran_hanger_refnos = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .bran_hanger_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else if is_debug {
                let r =
                    aios_core::query_refnos_deep_children(&debug_root_refnos, &["BRAN", "HANG"])
                        .await?;
                debug_root_refnos.retain_mut(|x| !r.contains(x));
                dbg!(&r);
                r.into_iter().collect()
            } else {
                mgr.get_gen_model_target_refnos(
                    GeoEnum::CATA_BRAN_AND_HANGER_REUSE,
                    &target_dbnos,
                    false,
                )
                .await?
            };
            println!(
                "使用管道或者支吊架元件库数量: {}",
                target_bran_hanger_refnos.len()
            );
            //查询出branch 和 branch 下的子节点
            let mut branch_refnos_map = DashMap::new();
            let mut bran_comp_eles = vec![];
            for &refno in &target_bran_hanger_refnos {
                let children = aios_core::get_children_pes(refno).await.unwrap_or_default();
                bran_comp_eles.extend(children.iter().map(|x| x.refno));
                //求出元件对应的outside bore
                branch_refnos_map.insert(refno, children);
            }
            ///获取重用的信息
            dbg!(&target_bran_hanger_refnos);
            let target_bran_reuse_cata_map = if is_incr_update || is_debug {
                let map = aios_core::query_group_by_cata_hash(&target_bran_hanger_refnos)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            CataHashRefnoKV {
                                cata_hash: k,
                                exist_geo: None,
                                group_refnos: v,
                            },
                        )
                    })
                    .collect();
                // dbg!(&map);
                map
                // DashMap<String, CataHashRefnoKV>
            } else {
                mgr.get_gen_model_map_by_cata_hash(
                    GeoEnum::CATA_BRAN_AND_HANGER_REUSE,
                    &target_dbnos,
                    true,
                    false,
                )
                .await?
            };

            let target_single_cata_map = if is_incr_update {
                let cata_map = DashMap::new();
                let cata_refnos = &incr_update_log.as_ref().unwrap().basic_cata_refnos;
                //直接使用group的办法，按cata_hash 进行分组
                for &r in cata_refnos {
                    let Ok(Some(att)) = aios_core::get_pe(r).await else {
                        continue;
                    };
                    cata_map.insert(
                        att.cata_hash.clone(),
                        CataHashRefnoKV {
                            cata_hash: att.cata_hash,
                            exist_geo: None,
                            group_refnos: vec![r],
                        },
                    );
                }
                cata_map
            } else if is_debug {
                //需要使用层级展开，graph的方式去查询
                let mut response = SUL_DB
                    .query(format!(
                        "select value refno from [{}] where owner.noun in ['BRAN', 'HANG'] or noun in $nouns ",
                        debug_root_refnos
                            .iter()
                            .map(|x| x.to_pe_key())
                            .collect::<Vec<_>>()
                            .join(",")
                    ))
                    .bind(("nouns", CATA_WITHOUT_REUSE_GEO_NAMES))
                    .await
                    .unwrap();
                let bran_children_refnos: Vec<RefU64> = response.take(0)?;
                dbg!(&bran_children_refnos);
                let mut cata_refnos = aios_core::query_refnos_deep_children(
                    &debug_root_refnos,
                    &CATA_WITHOUT_REUSE_GEO_NAMES,
                )
                .await?;
                dbg!(&cata_refnos);
                cata_refnos.extend(bran_children_refnos);
                let cata_map = DashMap::new();
                //直接使用group的办法，按cata_hash 进行分组
                for r in cata_refnos {
                    let Ok(Some(att)) = aios_core::get_pe(r).await else {
                        continue;
                    };
                    cata_map.insert(
                        att.cata_hash.clone(),
                        CataHashRefnoKV {
                            cata_hash: att.cata_hash,
                            exist_geo: None,
                            group_refnos: vec![r],
                        },
                    );
                }
                // dbg!(&cata_map);
                cata_map
            } else {
                //相当于在group
                mgr.get_gen_model_map_by_cata_hash(
                    GeoEnum::CATA_WITHOUT_REUSE,
                    &target_dbnos,
                    false,
                    false,
                )
                .await?
            };
            #[cfg(debug_assertions)]
            {
                dbg!(&target_bran_reuse_cata_map.len());
                dbg!(&target_single_cata_map.len());
            }

            let mut has_run_cata = false;
            if run_cache_cata {
                let mut handles = vec![];
                //bran，hanger下需要重用的模型
                if !target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty() {
                    let mgr_clone = mgr.clone();
                    let instance_mgr_clone = instance_mgr.clone();
                    let sjus_map_clone = loop_sjus_map_arc.clone();
                    {
                        instance_mgr_clone.write().await.fill_basic_shapes();
                    }
                    let handle = tokio::spawn(async move {
                        gen_cata_geos(
                            mgr_clone,
                            instance_mgr_clone,
                            Arc::new(target_bran_reuse_cata_map),
                            Arc::new(branch_refnos_map),
                            sjus_map_clone,
                        )
                        .await
                        .unwrap();
                    });
                    has_run_cata = true;
                    handles.push(handle);
                }

                //不能重用的类型
                if !target_single_cata_map.is_empty() {
                    let mgr_clone = mgr.clone();
                    let sjus_map_clone = loop_sjus_map_arc.clone();
                    let instance_mgr_clone = instance_mgr.clone();
                    let handle = tokio::spawn(async move {
                        gen_cata_geos(
                            mgr_clone,
                            instance_mgr_clone,
                            Arc::new(target_single_cata_map),
                            Arc::new(Default::default()),
                            sjus_map_clone,
                        )
                        .await
                        .unwrap();
                    });
                    has_run_cata = true;
                    handles.push(handle);
                }

                futures::future::join_all(handles).await;
                if has_run_cata {
                    let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
                    let shape_insts_data = instance_mgr.read().await;
                    println!("当前db下的元件库生成统计：");
                    dbg!(mesh_mgr.len());
                    dbg!(shape_insts_data.inst_info_map.len());
                    // dbg!(&inst_data.inst_info_map);
                    dbg!(shape_insts_data.inst_tubi_map.len());
                }
            }
        }

        let mut has_geom_refnos: Vec<RefU64> = vec![];
        if !is_incr_update {
            // for root_refno in root_refnos.clone() {
            //     let refnos = mgr.query_refnos_has_geos(root_refno).await?;
            //     has_geom_refnos.extend_from_slice(&refnos);
            // }
            dbg!(has_geom_refnos.len());
        }

        if !has_geom_refnos.is_empty() || is_incr_update {
            let target_loop_refnos = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .loop_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else {
                mgr.get_gen_model_target_refnos(GeoEnum::LOOP_AND_PLOO, &target_dbnos, false)
                    .await?
            };
            println!("使用LOOP的数量: {}", target_loop_refnos.len());
            if run_cache_loop && !target_loop_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let mgr_clone = mgr.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let handle = tokio::spawn(async move {
                    gen_loop_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &target_loop_refnos,
                        sjus_map_clone,
                    )
                    .await
                    .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            ///基本体模型的生成
            let target_prim_refnos = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .prim_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else {
                mgr.get_gen_model_target_refnos(GeoEnum::PRIM, &target_dbnos, false)
                    .await?
            };
            println!("使用基本体数量: {}", target_prim_refnos.len());
            if run_cache_prim && !target_prim_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    gen_prim_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        target_prim_refnos.as_slice(),
                    )
                    .await
                    .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            println!("开始处理负实体计算");
            let has_pos_neg_map = mgr
                .query_refnos_has_pos_neg_map(&root_refnos)
                .await
                .unwrap_or_default();
            dbg!(has_pos_neg_map.len());
            //负实体的结果不需要保存到本地
            {
                let mesh_mgr = mgr.cached_mesh_mgr.read().await;
                save_mesh_to_local_db(&mgr, &mesh_mgr, replace_mesh)
                    .expect("Save mesh to local db failed.");
            }

            //总的负实体计算
            if db_option.apply_boolean_operation && !has_pos_neg_map.is_empty() {
                let now = Instant::now();
                let mut trans_map = DashMap::new();
                let mut mesh_result_map: Arc<DashMap<u64, PlantGeoData>> = Arc::new(DashMap::new());
                let mut compound_inst_info_result_map = Arc::new(DashMap::new());
                let mut compound_inst_geos_result_map = Arc::new(DashMap::new());
                {
                    let mut inst_data = instance_mgr.write().await;
                    let mut mesh_mgr = Arc::new(mgr.cached_mesh_mgr.read().await);
                    for comp_refno in has_pos_neg_map.keys().cloned() {
                        let trans = mgr
                            .get_world_transform(comp_refno)
                            .await
                            .unwrap_or_default()
                            .unwrap_or_default();
                        trans_map.insert(comp_refno, trans);
                    }
                    // has_pos_neg_map.into_iter().for_each(
                    //     |(comp_refno, (mut pos_refnos, origin_neg_refnos))| {
                    for (comp_refno, (mut pos_refnos, origin_neg_refnos)) in has_pos_neg_map {
                        println!("正在处理: {} 下的负实体", comp_refno);

                        // let Ok(children_refnos) = mgr.get_children_from_localdb(comp_refno)
                        let Ok(children_refnos) = aios_core::get_children_refnos(comp_refno).await
                        else {
                            continue;
                        };
                        let mut neg_refnos = vec![];
                        children_refnos.iter().for_each(|x| {
                            for c in &origin_neg_refnos {
                                if c == x {
                                    neg_refnos.push(*c);
                                }
                            }
                        });

                        let mut mesh_mgr_clone = mesh_mgr.clone();
                        let mut mesh_result_map_clone = mesh_result_map.clone();
                        let mut inst_info_result_map_clone = compound_inst_info_result_map.clone();
                        let mut inst_geos_result_map_clone = compound_inst_geos_result_map.clone();

                        let mut batch_manifolds = vec![];
                        //没有正实体的情况，直接跳过
                        // if neg_refnos.is_empty() {
                        //     return;
                        // }
                        pos_refnos.push(comp_refno);
                        // dbg!(&pos_refnos);
                        let Some(w_trans) = trans_map.get(&comp_refno).map(|x| x.value().clone())
                        else {
                            continue;
                        };
                        // #[cfg(debug_assertions)]
                        // {
                        //     dbg!(w_trans);
                        //     dbg!(quat_to_pdms_ori_str(&w_trans.rotation));
                        // }
                        // dbg!(w_trans);
                        let mut total_refnos = vec![comp_refno];
                        total_refnos.extend_from_slice(&neg_refnos);
                        let inverse_mat = w_trans.compute_matrix().as_dmat4().inverse();

                        let origin_aabb =
                            { inst_data.get_info(&comp_refno).map(|x| x.aabb).flatten() };

                        let mut neg_refnos = vec![];
                        let mut found_non_manifold = false;

                        for (index, t_refno) in total_refnos.into_iter().enumerate() {
                            let geos_info_tmp = { inst_data.get_info(&t_refno).cloned() };
                            let Some(geos_info) = geos_info_tmp else {
                                continue;
                            };
                            let Some(inst_geos) = inst_data.get_inst_geos_data_mut(&geos_info)
                            else {
                                continue;
                            };
                            let mut pos_aabb = Aabb::new_invalid();
                            let pos_refno = pos_refnos[0];
                            for geo_inst in &mut inst_geos.insts {
                                let Some(mesh) = mesh_mgr_clone.get_mesh(geo_inst.geo_hash) else {
                                    continue;
                                };
                                let Some(aabb) = mesh_mgr_clone.get_aabb(geo_inst.geo_hash) else {
                                    continue;
                                };
                                let world_geo_mat = geos_info.world_transform;
                                #[cfg(debug_assertions)]
                                {
                                    dbg!(world_geo_mat);
                                    dbg!(quat_to_pdms_ori_str(&world_geo_mat.rotation));
                                }
                                let ele_mat =
                                    inverse_mat * world_geo_mat.compute_matrix().as_dmat4();
                                let mut local_mat =
                                    ele_mat * geo_inst.transform.compute_matrix().as_dmat4();

                                #[cfg(debug_assertions)]
                                {
                                    dbg!(ele_mat);
                                    dbg!(&geo_inst);
                                    dbg!(local_mat);
                                    dbg!(to_pdms_ori_str(&Mat3::from_mat4(local_mat.as_mat4())));
                                }

                                //如果是第一个正实体，需要生成模型计算
                                //如果是负实体，需要生成模型计算
                                let is_neg = !pos_refnos.contains(&t_refno) || geo_inst.is_neg();
                                if t_refno == comp_refno || is_neg {
                                    if pos_refnos.contains(&t_refno) {
                                        pos_aabb = aabb;
                                    } else {
                                        neg_refnos.push(t_refno);
                                    }
                                    if is_neg {
                                        geo_inst.owner_pos_refnos = [pos_refno].into();
                                        //根据类型来考虑是否需要扩大负实体
                                        let mut center: Vec3 = aabb.center().into();
                                        let t_mat = DMat4::from_translation(center.as_dvec3());
                                        let mut s = 1.0;
                                        if matches!(
                                            geo_inst.geo_param,
                                            PdmsGeoParam::PrimRevolution(_)
                                        ) {
                                            //如果是旋转体，xy方向都适当放大一点, 旋切的情况处理，因为精度不一样，倒是负实体切割问题
                                            if aabb.contains(&pos_aabb) {
                                                s = 1.03;
                                            }
                                            let s_mat = DMat4::from_scale(DVec3::new(1.0, s, s));
                                            let inv_t_mat =
                                                DMat4::from_translation((-center).as_dvec3());
                                            local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                                        } else {
                                            let s_mat =
                                                DMat4::from_scale(DVec3::new(1.003, 1.003, 1.003));
                                            let inv_t_mat =
                                                DMat4::from_translation((-center).as_dvec3());
                                            local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                                        };
                                    }

                                    #[cfg(debug_assertions)]
                                    {
                                        dbg!(t_refno);
                                        dbg!(mesh.vertices.len());
                                    }

                                    let manifold: ManifoldRust = (mesh, &local_mat).into();
                                    if manifold.num_tri() == 0 {
                                        println!("Found non manifold {}", t_refno);
                                        found_non_manifold = true;
                                    } else {
                                        if is_neg {
                                            batch_manifolds.push(manifold);
                                        } else {
                                            //正实体放在最前面
                                            batch_manifolds.insert(0, manifold);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(debug_assertions)]
                        dbg!(&neg_refnos);
                        let geo_hash = *comp_refno;
                        #[cfg(debug_assertions)]
                        dbg!(batch_manifolds.len());
                        // ----- 基本体的负实体运算  ----- //
                        let mut plant_geo_data = {
                            if batch_manifolds.len() < 2 {
                                continue;
                            }
                            let final_manifold =
                                // if cfg!(debug_assertions) {
                                //     ManifoldRust::batch_boolean(&batch_manifolds, 0)
                                // } else
                                {
                                    let mut src_manifold = batch_manifolds.remove(0);
                                    src_manifold.batch_boolean_subtract(&batch_manifolds)
                                };
                            dbg!(final_manifold.num_tri());
                            let final_mesh: PlantMesh = final_manifold.clone().into();
                            for m in batch_manifolds {
                                // #[cfg(target_os = "macos" || target_os = "linux")]
                                // m.destroy();
                            }
                            PlantGeoData {
                                geo_hash,
                                mesh: Some(final_mesh),
                                aabb: origin_aabb.clone(),
                            }
                        };
                        mesh_result_map_clone.insert(geo_hash, plant_geo_data);
                        let geom_inst = EleInstGeo {
                            geo_hash,
                            refno: comp_refno,
                            owner_pos_refnos: Default::default(),
                            pts: vec![],
                            aabb: None,
                            transform: Transform::IDENTITY,
                            geo_param: PdmsGeoParam::CompoundShape,
                            visible: true,
                            is_tubi: false,
                            geo_type: GeoBasicType::Compound,
                        };

                        let inst_key = hash_two_str(&comp_refno.to_url_refno(), "compound");
                        let mut comp_geos_info = EleGeosInfo {
                            refno: comp_refno,
                            visible: true,
                            generic_type: mgr.get_generic_type(comp_refno).await,
                            aabb: origin_aabb.clone(),
                            world_transform: w_trans,
                            //cata hash 用作唯一的标识符就行，后面需要变名称
                            cata_hash: Some(inst_key.to_string()),
                            flow_pt_indexs: vec![],
                            geo_type: GeoBasicType::Compound,
                        };
                        inst_info_result_map_clone.insert(comp_refno, comp_geos_info);
                        let comp_type = mgr.get_type_name(comp_refno).await;

                        inst_geos_result_map_clone.insert(
                            inst_key.to_string(),
                            EleInstGeosData {
                                inst_key: inst_key.to_string(),
                                refno: comp_refno,
                                insts: vec![geom_inst],
                                aabb: origin_aabb.clone(),
                                type_name: comp_type,
                                ptset_map: Default::default(),
                            },
                        );
                    }
                    // );

                    println!("布尔运算实体耗时 {} ms", now.elapsed().as_millis());
                }

                {
                    let mut inst_data = instance_mgr.write().await;
                    // dbg!(compound_inst_geos_result_map.len());
                    let inst_geos_result_map_inner =
                        Arc::try_unwrap(compound_inst_geos_result_map).unwrap();
                    for (k, v) in inst_geos_result_map_inner {
                        inst_data.insert_geos_data(k, v);
                    }
                    let inst_info_result_map_inner =
                        Arc::try_unwrap(compound_inst_info_result_map).unwrap();
                    for (k, v) in inst_info_result_map_inner {
                        //排除重复的情况
                        inst_data.insert_compound_info(k, v);
                    }

                    let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
                    let mesh_result_map_inner = Arc::try_unwrap(mesh_result_map).unwrap();
                    for (k, v) in mesh_result_map_inner {
                        mesh_mgr.insert(k, v);
                    }
                }
            } else {
                println!("当前节点下面没有需要参与负实体计算的几何体");
            }
        } else {
            println!("当前节点下面没有要继续生成的基本体几何节点");
        }

        {
            let mut shape_insts_data = instance_mgr.write().await;
            let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
            //处理有NGMR的情况，首选需要过滤出来
            println!(
                "开始处理ngmr的负实体, 总数: {}",
                shape_insts_data.ngmr_inst_info_map.len()
            );
            if !shape_insts_data.ngmr_inst_info_map.is_empty() {
                let mut boolean_ngmr_map = HashMap::new();
                ///查找是否是某些参考号的子节点
                for (refno, geos_info) in shape_insts_data.ngmr_inst_info_map.clone() {
                    let Some(geos_data) = shape_insts_data.get_inst_geos_data_mut(&geos_info)
                    else {
                        continue;
                    };
                    let att = aios_core::get_named_attmap(refno).await.unwrap_or_default();
                    let c_ref = att.get_foreign_refno("CREF");
                    #[cfg(debug_assertions)]
                    dbg!(c_ref);
                    //todo fix
                    // let o_ref = mgr.traverse_ancestor(refno, |r| async {
                    //     let type_name = mgr.get_type_name(r).await;
                    //     r != refno && NGMR_OWN_TYPES.contains(&type_name.as_str())
                    // });
                    let o_ref = None;
                    #[cfg(debug_assertions)]
                    dbg!(o_ref);
                    let mut own_pos_map = HashMap::new();
                    for g in &mut geos_data.insts {
                        let ngmr_geo_refno = g.refno;
                        let geo_att = aios_core::get_named_attmap(ngmr_geo_refno)
                            .await
                            .unwrap_or_default();
                        // dbg!(&geo_att);
                        let geo_refno = g.refno;
                        let removed_type =
                            NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1))
                                .unwrap_or_default();
                        #[cfg(debug_assertions)]
                        dbg!(removed_type);
                        match removed_type {
                            // NgmrRemovedType::AsDefault => {}
                            NgmrRemovedType::Nothing => {}
                            NgmrRemovedType::Attached => {
                                if let Some(x) = c_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                            }
                            NgmrRemovedType::AsDefault | NgmrRemovedType::Owner => {
                                if let Some(x) = o_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                            }
                            NgmrRemovedType::Item => {
                                g.owner_pos_refnos.insert(refno);
                                boolean_ngmr_map
                                    .entry(refno)
                                    .or_insert_with(|| BTreeMap::new())
                                    .entry(refno)
                                    .or_insert_with(|| BTreeSet::new())
                                    .insert(geo_refno);
                            }
                            NgmrRemovedType::AttachedAndOwner => {
                                if let Some(x) = c_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                                if let Some(x) = o_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                            }
                            NgmrRemovedType::AttachedAndItem => {
                                if let Some(x) = c_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                                g.owner_pos_refnos.insert(refno);
                                boolean_ngmr_map
                                    .entry(refno)
                                    .or_insert_with(|| BTreeMap::new())
                                    .entry(refno)
                                    .or_insert_with(|| BTreeSet::new())
                                    .insert(geo_refno);
                            }
                            NgmrRemovedType::OwnerAndItem => {
                                if let Some(x) = o_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                                g.owner_pos_refnos.insert(refno);
                                boolean_ngmr_map
                                    .entry(refno)
                                    .or_insert_with(|| BTreeMap::new())
                                    .entry(refno)
                                    .or_insert_with(|| BTreeSet::new())
                                    .insert(geo_refno);
                            }
                            //几个种类都支持
                            NgmrRemovedType::All => {
                                if let Some(x) = o_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }
                                if let Some(x) = c_ref {
                                    g.owner_pos_refnos.insert(x);
                                    boolean_ngmr_map
                                        .entry(x)
                                        .or_insert_with(|| BTreeMap::new())
                                        .entry(refno)
                                        .or_insert_with(|| BTreeSet::new())
                                        .insert(geo_refno);
                                }

                                g.owner_pos_refnos.insert(refno);
                                boolean_ngmr_map
                                    .entry(refno)
                                    .or_insert_with(|| BTreeMap::new())
                                    .entry(refno)
                                    .or_insert_with(|| BTreeSet::new())
                                    .insert(geo_refno);
                            }
                        }
                        own_pos_map.insert(g.refno, g.owner_pos_refnos.clone());
                    }

                    //对应的原本的inst geos 也要更新

                    for (refno, inst_geos) in &mut shape_insts_data.inst_geos_map {
                        for inst_geo in &mut inst_geos.insts {
                            if let Some(r) = own_pos_map.get(&inst_geo.refno) {
                                // dbg!(r);
                                inst_geo.owner_pos_refnos = r.clone();
                            }
                        }
                    }

                    #[cfg(debug_assertions)]
                    {
                        // dbg!(&boolean_ngmr_map);
                    }
                }

                println!("开始处理ngmr的负实体模型");
                for (parent, ngmr_map) in boolean_ngmr_map {
                    let Some(parent_geos_info) = shape_insts_data.get_final_inst_info(parent)
                    else {
                        continue;
                    };
                    let Some(parent_geos_data) =
                        shape_insts_data.get_inst_geos_data(parent_geos_info)
                    else {
                        continue;
                    };
                    if parent_geos_data.insts.is_empty() {
                        continue;
                    }
                    let parent_matrix_inverse = parent_geos_info
                        .world_transform
                        .compute_matrix()
                        .as_dmat4()
                        .inverse();
                    let mut pos_monifolds = vec![];
                    for p_inst in parent_geos_data.insts.clone() {
                        //过滤掉ngmr的类型，否则会有重复
                        if p_inst.geo_type != GeoBasicType::Pos
                            && p_inst.geo_type != GeoBasicType::Compound
                        {
                            continue;
                        }
                        let Some(parent_mesh) = mesh_mgr.get_mesh(p_inst.geo_hash) else {
                            continue;
                        };
                        let dmat4 = p_inst.transform.compute_matrix().as_dmat4();
                        let mut tmp_manifold: ManifoldRust = (parent_mesh, &dmat4).into();
                        pos_monifolds.push(tmp_manifold);
                    }
                    let mut parent_manifold = ManifoldRust::batch_boolean(&pos_monifolds, 0);

                    #[cfg(debug_assertions)]
                    dbg!(parent_manifold.num_tri());
                    let mut neg_ms = vec![];
                    for (refno, geo_refnos) in ngmr_map {
                        let Some(geos_info) = shape_insts_data.get_ngmr_info(&refno) else {
                            continue;
                        };
                        let Some(geos_data) = shape_insts_data.get_inst_geos_data(geos_info) else {
                            continue;
                        };

                        let relative_mat = parent_matrix_inverse
                            * geos_info.world_transform.compute_matrix().as_dmat4();
                        // dbg!(refno);
                        for g in &geos_data.insts {
                            if !g.visible || !geo_refnos.contains(&g.refno) {
                                dbg!(g.refno);
                                continue;
                            }

                            let mut local_mat = g.transform.compute_matrix().as_dmat4();
                            let Some(mesh) = mesh_mgr.get_mesh(g.geo_hash) else {
                                continue;
                            };
                            let Some(aabb) = mesh_mgr.get_aabb(g.geo_hash) else {
                                continue;
                            };
                            {
                                let center: Vec3 = aabb.center().into();
                                let mut center = center.as_dvec3();
                                let t_mat = DMat4::from_translation(center);
                                let mut s = 1.001;
                                let s_mat = DMat4::from_scale(DVec3::splat(s));
                                let inv_t_mat = DMat4::from_translation(-center);
                                local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                            }
                            local_mat = relative_mat * local_mat;
                            #[cfg(debug_assertions)]
                            {
                                if refno.get_1() == 209883 {
                                    mesh.export_obj(false, &format!("{}.obj", g.refno))
                                        .expect("TODO: panic message");
                                }
                            }
                            //todo 负实体，默认去增加一下数据的精度值，而不是盲目缩放，比如round 1， round 2
                            let mut neg_manifold: ManifoldRust = (mesh, &local_mat).into();
                            #[cfg(debug_assertions)]
                            {
                                dbg!(neg_manifold.num_tri());
                            }
                            if neg_manifold.num_tri() != 0 {
                                neg_ms.push(neg_manifold);
                            }
                        }
                    }
                    //开始进行ngmr 的 boolean操作
                    let mut final_manifold = parent_manifold.batch_boolean_subtract(&neg_ms);
                    #[cfg(debug_assertions)]
                    dbg!(final_manifold.num_tri());
                    //相当于更新
                    let mut new_geos_info = parent_geos_info.clone();
                    //如果和ngmr发生相减后， 没有复用了
                    new_geos_info.update_to_compound(Some(parent.to_url_refno().as_str()));
                    let mut mesh: PlantMesh = (final_manifold.clone()).into();
                    let geo_hash = new_geos_info.get_inst_key_u64();
                    let new_inst = EleInstGeo {
                        geo_hash,
                        refno: parent,
                        owner_pos_refnos: Default::default(),
                        geo_param: PdmsGeoParam::CompoundShape,
                        pts: vec![],
                        aabb: mesh.cal_aabb(),
                        transform: Transform::IDENTITY,
                        visible: true,
                        is_tubi: false,
                        geo_type: GeoBasicType::Compound,
                    };

                    for f in neg_ms {
                        // #[cfg(target_os = "macos"|| target_os = "linux")]
                        // f.destroy();
                    }
                    // final_manifold.destroy();
                    let mut new_geos_data = parent_geos_data.clone();
                    new_geos_data.insts = vec![new_inst];
                    new_geos_data.inst_key = geo_hash.to_string();
                    // dbg!(&new_geos_data);

                    mesh_mgr.insert(
                        geo_hash,
                        PlantGeoData {
                            geo_hash,
                            mesh: Some(mesh),
                            aabb: new_geos_data.aabb,
                        },
                    );
                    shape_insts_data.insert_geos_data(geo_hash.to_string(), new_geos_data);
                    shape_insts_data.insert_compound_info(parent, new_geos_info);
                }
            }
        }

        {
            let inst_data = instance_mgr.read().await;
            println!("当前db下的基本体生成统计：");
            dbg!(inst_data.inst_geos_map.len());
            // dbg!(&inst_data.inst_geos_map);
            save_mesh_instance_data(&mgr, &inst_data).await?;
        }

        {
            let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
            dbg!(mesh_mgr.len());
            save_mesh_data(&mgr, &mut mesh_mgr, replace_mesh).await?;
        }

        println!("{db_no} 生成完毕。");
    }

    println!("生成所有模型时间: {}ms", time.elapsed().as_millis());
    Ok(true)
}

pub async fn query_tubi_size(
    mgr: &AiosDBManager,
    refno: RefU64,
    tubi_cat_ref: RefU64,
    is_hang: bool,
    axis_map: Option<&BTreeMap<i32, CateAxisParam>>,
) -> anyhow::Result<TubiSize> {
    //只是为了获得外径而已
    let tubi_geoms_info = resolve_desi_comp(Some(mgr), refno, Some(tubi_cat_ref), axis_map)
        .await
        .unwrap_or_default();
    for geom in &tubi_geoms_info.geometries {
        if let BoxImplied(d) = geom {
            return Ok(TubiSize::BoxSize((d.width, d.height)));
        } else if let TubeImplied(d) = geom {
            return Ok(TubiSize::BoreSize(d.diameter));
        }
    }
    // use default
    {
        if let Ok(cat_att) = aios_core::get_named_attmap(tubi_cat_ref).await {
            let params = cat_att.get_f32_vec("PARA").unwrap_or_default();
            if params.len() >= 2 {
                let tubi_bore = params[if is_hang { 0 } else { 1 }] as f32;
                return Ok(TubiSize::BoreSize(tubi_bore));
            }
        };
    }
    return Ok(TubiSize::None);
}
