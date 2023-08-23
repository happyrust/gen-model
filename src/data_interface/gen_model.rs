use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::aql_api::children::query_children_order_aql;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn::geo::create_profile_geos;
use crate::data_interface::db_manager::GeoEnum;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::save_arangodb_doc;
use crate::graph_db::pdms_inst_arango::*;
use crate::graph_db::pdms_mesh_arango::{save_mesh_to_arango_db, save_mesh_to_local_db};
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use crate::consts::*;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
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
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::{Mat4, Vec3};
use nalgebra::Point3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    prim_refnos: &[RefU64],
    sjus_map_arc: Arc<DashMap<RefU64, (Vec3, f32)>>,
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
        let sjus_map_clone = sjus_map_arc.clone();
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

                let attr = mgr_clone.get_attr_from_localdb(refno).unwrap_or_default();
                let mut geos_info = EleGeosInfo {
                    refno,
                    visible: true,
                    generic_type: mgr_clone.get_generic_type(refno),
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
                let limit_size: Option<f32> = if GENRAL_NEG_NOUN_NAMES.contains(&attr.get_type()) {
                    if let Some(parent_inst) = shape_insts_data
                        .inst_info_map
                        .get(&attr.get_owner().unwrap_or_default())
                    {
                        parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let brep_shape = if attr.get_type() == "POHE" {
                    let pgo_refnos = mgr_clone
                        .get_children_from_localdb(refno)
                        .unwrap_or_default();
                    let mut polygons = vec![];
                    for pgo_refno in pgo_refnos {
                        let mut verts = vec![];
                        let v_att = mgr_clone.get_children_attrs(pgo_refno).unwrap_or_default();
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
                let mut geo_aabb = if !replace_mesh && let Ok(aabb) = mgr_clone.get_mesh_aabb_from_localdb(geo_hash) {
                    if let Ok(mesh) = mgr_clone.get_mesh_from_localdb(geo_hash) {
                        cached_mesh_mgr.insert(geo_hash, PlantGeoData {
                            geo_hash,
                            mesh: Some(mesh),
                            aabb: Some(aabb),
                        });
                    }
                    aabb
                } else {
                    let tmp_tol = if attr.is_neg() {
                        tol_ratio
                    } else {
                        tol_ratio
                    };
                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(brep_shape, replace_mesh, tmp_tol) else {
                        continue;
                    };
                    aabb
                };
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                let tr = &item_trans;
                let ele_aabb = aabb_apply_transform(&geo_aabb, &tr);
                let inst_geo = EleInstGeo {
                    geo_hash,
                    refno,
                    owner_pos_refno: Default::default(),
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
                            inst_key: refno.to_url_refno(),
                            refno,
                            insts: geo_insts,
                            aabb: Some(geo_aabb),
                            type_name: attr.get_type().to_string(),
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
    let mut is_debug = false;
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
                let Some(refno_basic) = mgr.get_refno_basic(loop_refno) else {
                    continue;
                };
                let parent_basic = mgr.get_owner_ref_basic(loop_refno).unwrap();
                let target_type = parent_basic.get_type();
                let cur_type = refno_basic.get_type();
                let parent_refno = refno_basic.get_owner();
                let mut parent_att = mgr.get_attr_from_localdb(parent_refno).unwrap_or_default();
                // let grand_refno = parent_att.get_owner().unwrap_or_default();

                let Ok(Some(mut trans_origin)) = mgr.get_world_transform(loop_refno).await else {
                    continue;
                };
                //判断父节点是否有SJUS，需要调整位置
                if cur_type == "PLOO" && let Some(sjus_adjust) = sjus_map_clone.get(&parent_refno) {
                    let offset = trans_origin.rotation.mul_vec3(sjus_adjust.value().0);
                    trans_origin.translation += offset;
                    // dbg!(offset);
                    // dbg!(trans_origin.translation);
                }
                // else if let Some(sjus_adjust) = sjus_map_clone.get(&grand_refno){
                // let grand_trans = mgr.get_world_transform(grand_refno).await.unwrap_or_default().unwrap_or_default();
                // trans_origin.translation += grand_trans.rotation * sjus_adjust.value().0;
                // dbg!(trans_origin.translation);
                // }
                println!(
                    "正在处理loops类型的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    parent_refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let mut loop_verts: Vec<Vec3> = vec![];
                let mut fradius_vec: Vec<f32> = vec![];

                if let Ok(children_atts) = mgr.get_children_attrs(loop_refno) {
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

                let mut sibling_refnos = mgr
                    .get_children_from_localdb(parent_refno)
                    .unwrap_or_default();
                let cur_sibling_index = sibling_refnos
                    .iter()
                    .filter(|&x| mgr.get_type_name(*x).as_str() == "PLOO")
                    .position(|x| *x == loop_refno)
                    .unwrap_or_default();
                // dbg!(&sibling_refnos);
                let mut geos_info = EleGeosInfo {
                    refno: parent_refno,
                    cata_hash: None,
                    visible: true,
                    world_transform: trans_origin,
                    generic_type: mgr.get_generic_type(parent_refno),
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
                // dbg!(&target_type);
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
                                geo_aabb = if !replace_mesh && let Ok(aabb) = mgr.get_mesh_aabb_from_localdb(geo_hash) {
                                    if let Ok(mesh) = mgr.get_mesh_from_localdb(geo_hash) {
                                        cached_mesh_mgr.insert(geo_hash, PlantGeoData {
                                            geo_hash,
                                            mesh: Some(mesh),
                                            aabb: Some(aabb),
                                        });
                                    }
                                    Some(aabb)
                                } else {
                                    let tmp_tol = if parent_att.is_neg() {
                                        tol_ratio.map(|x| x * 2.0)
                                    } else {
                                        tol_ratio
                                    };
                                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(revo, replace_mesh, tmp_tol) else {
                                        continue;
                                    };
                                    Some(aabb)
                                };
                            }
                        }
                    }
                    //todo 关于justline，可能需要jusline的信息才能判断中心点
                    "AEXTR" | "NXTR" | "EXTR" | "PANE" | "FLOOR" | "SCREED" | "GWALL" => {
                        let loop_attr = mgr.get_attr_from_localdb(loop_refno).unwrap_or_default();
                        //不是第一个loop，需要取第一个的loop的height
                        let mut height = if cur_sibling_index > 0 {
                            mgr.get_attr_from_localdb(sibling_refnos[0]).unwrap_or_default()
                                .get_f32("HEIG").unwrap_or_default()
                        } else {
                            loop_attr
                                .get_f32("HEIG")
                                .unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default())
                        };
                        if height < f32::EPSILON {
                            println!("{}： 的height太小为: {}", parent_refno, height);
                            continue;
                        }
                        if loop_attr.get_type() == "NXTR" {
                            if let Some(parent_inst) = shape_insts_data
                                .get_inst_info(loop_attr.get_owner().unwrap_or_default()) {
                                if let Some(h) =
                                    parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0) {
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
                        geo_aabb = if !replace_mesh && let Ok(aabb) = mgr.get_mesh_aabb_from_localdb(geo_hash) {
                            // dbg!("Found in local mesh");
                            Some(aabb)
                        } else {
                            let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(extrusion, replace_mesh, tol_ratio) else {
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
                    owner_pos_refno: Default::default(),
                    pts: Default::default(),
                    aabb: Some(geo_aabb),
                    transform: tr,
                    visible,
                    is_tubi: false,
                    geo_param: geo_param.clone(),
                    geo_type: if parent_att.is_neg()
                        || cur_sibling_index >= 1
                    {
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
                        type_name: parent_att.get_type().to_string(),
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

///获取单个元件的模型数据
pub async fn gen_cata_single_geoms(
    mgr: Arc<AiosDBManager>,
    design_refno: RefU64,
    brep_shape_map: &CateBrepShapeMap,
    refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>,
    scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
) -> anyhow::Result<RefU64> {
    let cur_ele = mgr
        .get_refno_basic(design_refno)
        .ok_or(anyhow::anyhow!("Element不存在"))?;
    let type_name = cur_ele.get_type();
    let owner = mgr.get_owner_ref_basic(design_refno);
    if owner.is_none() {
        return Ok(RefU64::default());
    }
    let desi_att = mgr.get_attr_from_localdb(design_refno)?;
    let geoms_info = resolve_desi_comp(Some(mgr.as_ref()), design_refno, None, scom_info_map, None)
        .await
        .unwrap_or_default();
    if type_name == "SCTN" || type_name == "STWALL" ||
        type_name == "GENSEC" || type_name == "WALL" {
        create_profile_geos(
            design_refno,
            &desi_att,
            &geoms_info,
            &brep_shape_map,
            mgr.as_ref(),
        ).await?;
        return Ok(geoms_info.refno);
    } else {
        let CateGeomsInfo {
            refno,
            geometries,
            n_geometries,
            axis_map,
        } = geoms_info;
        for (i, geom) in geometries.into_iter().enumerate() {
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                brep_shape_map
                    .entry(design_refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
            }
        }
        for (i, geom) in n_geometries.into_iter().enumerate() {
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
    scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    //branch 下按顺序的清单
    branch_map: Arc<DashMap<RefU64, Vec<PdmsElement>>>,
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
    if !all_unique_keys.is_empty() {
        for i in 0..batch_chunks_cnt as usize {
            let mgr_clone = mgr.clone();
            let instance_mgr = main_instance_mgr.clone();
            let all_unique_keys = all_unique_keys.clone();
            let processed_cnt = processed_cnt.clone();
            let scom_info_map = scom_info_map.clone();
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
                    let Some(cata_hash) = all_unique_keys[j].clone() else {
                        continue;
                    };
                    if cata_hash == "0" {
                        continue;
                    }
                    let target_cata = target_cata_map.get(&cata_hash).unwrap();
                    let mut cached_mesh_mgr = mgr_clone.cached_mesh_mgr.write().await;
                    let mut shape_insts_data = instance_mgr.write().await;
                    let mut target_geo_data_option = None;
                    let mut process_refno = None;
                    //reuse代表是否重用，如果
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
                        let current_att = mgr_clone
                            .get_attr_from_localdb(ele_refno)
                            .unwrap_or_default();
                        let mut refno_ptset_map = DashMap::new();
                        let cur_type = current_att.get_type();

                        let Ok(cat_refno) = gen_cata_single_geoms(
                            mgr_clone.clone(),
                            ele_refno,
                            &brep_shapes_map,
                            &refno_ptset_map,
                            &scom_info_map,
                        )
                            .await
                            else {
                                continue;
                            };
                        let mut is_reuse_unit = false;
                        ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                        for (ele_refno, shapes) in brep_shapes_map {
                            let Ok(Some(mut origin_trans)) = mgr_clone.get_world_transform(ele_refno).await
                                else {
                                    continue;
                                };

                            let Ok(ele_att) = mgr_clone.get_attr_from_localdb(ele_refno) else {
                                continue;
                            };

                            if let Some(sjus) = ele_att.get_str("SJUS") {
                                let parent = ele_att.get_owner().unwrap_or_default();
                                if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                    let height = sjus_adjust.value().1;
                                    let off_z = cal_sjus_value(sjus, height);
                                    let parent_trans = mgr_clone.get_world_transform(parent).await.unwrap_or_default().unwrap_or_default();

                                    origin_trans.translation.z = parent_trans.translation.z;
                                    // origin_trans.translation = origin_trans.translation + parent_trans.rotation.mul_vec3(sjus_adjust.value().0)
                                    //     + origin_trans.rotation.mul_vec3(Vec3::new(0.0, 0.0, off_z));
                                    origin_trans.translation = origin_trans.translation + sjus_adjust.value().0
                                        + Vec3::new(0.0, 0.0, off_z);
                                }
                            }

                            let cat_attmap =
                                mgr_clone.get_attr(cat_refno).await.unwrap_or_default();
                            // dbg!(&cat_attmap);
                            let gmse_refno = cat_attmap.get_foreign_refno("GMRE").unwrap_or(
                                cat_attmap.get_foreign_refno("GSTR").unwrap_or_default(),
                            );

                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            let pos_neg_map = if gmse_refno.is_valid() {
                                mgr_clone
                                    .query_refnos_has_pos_neg_map(&[gmse_refno])
                                    .await
                                    .unwrap_or_default()
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
                                generic_type: mgr_clone.get_generic_type(ele_refno),
                                aabb: None,
                                world_transform: origin_trans,
                                flow_pt_indexs: if !ele_att.contains_attr_name("ARRI") {
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
                            // let mut cata_aabb: Option<Aabb> = None;
                            //将负实体和正实体统计出来
                            let mut merged_cata_aabb: Option<Aabb> = None;
                            let mut n_merged_cata_aabb: Option<Aabb> = None;
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
                                if !visible || !brep_shape.check_valid() {
                                    continue;
                                }
                                let mut trans = brep_shape.get_trans();
                                let is_neg = neg_own_pos_map.contains_key(&refno);
                                let geo_hash = brep_shape.hash_unit_mesh_params();
                                let mut geo_aabb = if !replace_mesh && let Ok(aabb) = mgr_clone.get_mesh_aabb_from_localdb(geo_hash) {
                                    if let Ok(mesh) = mgr_clone.get_mesh_from_localdb(geo_hash) {
                                        cached_mesh_mgr.insert(geo_hash, PlantGeoData {
                                            geo_hash,
                                            mesh: Some(mesh),
                                            aabb: Some(aabb),
                                        });
                                    }
                                    aabb
                                } else {
                                    let mut tol = if is_neg {
                                        tol_ratio.unwrap_or(1.0) * 50.0
                                    } else {
                                        tol_ratio.unwrap_or(1.0)
                                    };
                                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(brep_shape.clone(), replace_mesh, Some(tol)) else {
                                        continue;
                                    };
                                    aabb
                                };
                                // dbg!(geo_aabb);
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
                                        .compute_matrix();
                                    let Some(aabb) = cached_mesh_mgr.get_aabb(geo_hash) else {
                                        continue;
                                    };
                                    //稍微扩张一点
                                    if is_neg {
                                        let mut center: Vec3 = aabb.center().into();
                                        let t_mat = Mat4::from_translation(center);
                                        let mut s = 1.001;
                                        // let s_mat = Mat4::from_scale(Vec3::new(1.0, 1.0, s));
                                        let s_mat = Mat4::from_scale(Vec3::splat(s));
                                        let inv_t_mat = Mat4::from_translation(-center);
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
                                    owner_pos_refno: neg_own_pos_map
                                        .get(&refno)
                                        .cloned()
                                        .unwrap_or_default(),
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
                                    // dbg!(&geos_info);
                                    aabb =
                                        Aabb::new(Point3::new(0., 0., 0.), Point3::new(0., 0., 0.));
                                }
                            }

                            //保存ngmr的信息
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
                            if geo_insts.len() > 0 {
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
                                            owner_pos_refno: Default::default(),
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
                            //只有一个，现在不采用branch的方式去生成了
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
                        // dbg!(&target_geo_data);
                        continue;
                    }
                    //如果已经有了，需要生成transform和bbox那些
                    for ele_refno in target_cata.group_refnos.clone() {
                        if Some(ele_refno) == process_refno {
                            continue;
                        }
                        println!(
                            "正在处理同类元件库的模型当前参考号：{}",
                            ele_refno.to_refno_string(),
                        );
                        let Ok(Some(mut origin_trans)) = mgr_clone.get_world_transform(ele_refno).await else {
                            continue;
                        };

                        let Some(ref_basic) = mgr_clone.get_refno_basic(ele_refno) else {
                            continue;
                        };
                        let mut flow_pt_indexs = vec![];
                        let Some(own_ref_basic) = mgr_clone.get_refno_basic(ref_basic.owner) else {
                            continue;
                        };

                        let attr = mgr_clone
                            .get_attr_from_localdb(ele_refno)
                            .unwrap_or_default();
                        if let Some(sjus) = attr.get_str("SJUS") {
                            let parent = ref_basic.owner;
                            if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                let height = sjus_adjust.value().1;
                                let off_z = cal_sjus_value(sjus, height);
                                // let parent_trans = mgr_clone.get_world_transform(parent).await.unwrap_or_default().unwrap_or_default();
                                origin_trans.translation +=  sjus_adjust.value().0 +
                                    origin_trans.rotation * Vec3::new(0.0, 0.0, off_z);
                            }
                        }

                        if CATA_HAS_TUBI_GEO_NAMES.contains(&own_ref_basic.get_type()) {
                            flow_pt_indexs = vec![
                                attr.get_i32("ARRI").unwrap_or(-1),
                                attr.get_i32("LEAV").unwrap_or(-1),
                            ];
                        }
                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type: mgr_clone.get_generic_type(ele_refno),
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
                            //将ngmr的mesh加载到内存，方便后续处理负实体
                            // if let Ok(aabb) = mgr_clone.get_mesh_aabb_from_localdb(geo_hash) {
                            //     if let Ok(mesh) = mgr_clone.get_mesh_from_localdb(geo_hash) {
                            //         cached_mesh_mgr.insert(geo_hash, PlantGeoData {
                            //             geo_hash,
                            //             mesh: Some(mesh),
                            //             aabb: Some(aabb),
                            //         });
                            //     }
                            // }
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
        let Ok(children_refnos) = mgr.get_children_from_localdb(branch_refno) else {
            continue;
        };
        // dbg!(&children_refnos);
        let mut children = vec![];
        //排一下顺序，后面这个element也是要存在本地
        children_refnos.into_iter().for_each(|x| {
            for c in b.value() {
                //同时过滤掉ATTA
                if c.refno == x && c.get_type_name() != "ATTA" {
                    children.push(c);
                }
            }
        });
        // dbg!(&children);
        let Ok(branch_att) = mgr.get_attr_from_localdb(branch_refno) else {
            continue;
        };
        // dbg!(&branch_att);
        //可能只有branch 元素需要做一遍求解
        let Ok(Some(branch_transform)) = mgr.get_world_transform(branch_refno).await else {
            continue;
        };
        let htube_pt = branch_transform.transform_point(branch_att.get_vec3("HPOS").unwrap());
        let hdir = branch_transform
            .transform_point(branch_att.get_vec3("HDIR").unwrap())
            .normalize_or_zero();
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());

        let is_hang = branch_att.get_type() == "HANG";
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();
        // dbg!(h_ref);

        let bran_name = branch_att.get_name().0.to_string();
        let tubi_att = mgr.get_attr_from_localdb(h_ref).unwrap_or_default();
        let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        let mut tubi_size = query_tubi_size(
            &mgr,
            branch_refno,
            tubi_cat_ref,
            is_hang,
            &scom_info_map,
            None,
        )
            .await?;
        // dbg!(&tubi_size);
        let tubi_geo_hash = if matches!(tubi_size, TubiSize::BoreSize(_)) {
            TUBI_GEO_HASH
        } else {
            BOXI_GEO_HASH
        };

        let mut href_type = "".to_string();
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
                                generic_type: mgr.get_generic_type(branch_refno),
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
                            att_type: branch_att.get_type().to_string(),
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
            let Ok(Some(ele_transform)) = mgr.get_world_transform(refno).await else {
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
            if axis_map.contains_key(&arrive) {
                let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                let dir = axis_map[&arrive].dir;
                let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                if a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                    current_tubing.end_pt = a_pos;
                    current_tubing.desire_arrive_dir = a_dir;
                    if current_tubing.is_dir_ok() {
                        if let Some(t) = current_tubing.get_transform() {
                            // dbg!(current_tubing.leave_refno);
                            inst_tubi_map.insert(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(current_tubing.leave_refno),
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
                        #[cfg(debug_assertions)]
                        dbg!(&current_tubing);
                        println!("{} 的直段方向有问题", refno.to_refno_string());
                    }
                }
            }
            if axis_map.contains_key(&leave) {
                let dir = axis_map[&leave].dir;
                let ref_dir = axis_map[&leave].ref_dir;
                let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                let l_ref_dir = world_trans.transform_vec3(ref_dir).normalize_or_zero();
                let l_pos = world_trans.transform_point(axis_map[&leave].pt);
                // let lstube_cat_ref = refno_lstube_map
                //     .get(&refno)
                //     .map(|x| *x.value())
                //     .unwrap_or_default();
                let att_map = mgr.get_attr_from_localdb(refno).unwrap_or_default();
                let lstube_ref = att_map.get_foreign_refno("LSTU").unwrap_or_default();
                let lstube_cat_ref = mgr
                    .get_attr_from_localdb(lstube_ref)
                    .unwrap_or_default()
                    .get_foreign_refno("CATR")
                    .unwrap_or_default();
                // let bore = lstube_bores_map
                //     .get(&lstube)
                //     .map(|x| *x.value())
                //     .unwrap_or_default();
                // current_tubing.tubi_size = TubiSize::BoreSize(bore);
                // dbg!((refno, lstube_cat_ref));
                current_tubing.tubi_size = query_tubi_size(
                    &mgr,
                    refno,
                    lstube_cat_ref,
                    is_hang,
                    &scom_info_map,
                    Some(axis_map),
                )
                    .await?;
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
                        if let Some(t) = current_tubing.get_transform() {
                            inst_tubi_map.insert(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(current_tubing.leave_refno),
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
        // dbg!(&inst_tubi_map);
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
    // dbg!(&tubi_result.len());
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

///生成几何体数据
pub async fn gen_geos_data(mut mgr: Arc<AiosDBManager>) -> anyhow::Result<bool> {
    let time = Instant::now();
    let db_option = &mgr.db_option;
    let project = &mgr.db_option.project_name;
    let mdb = &mgr.db_option.mdb_name;
    let mut db_nos = mgr.db_option.manual_db_nums.clone().unwrap_or_default();

    if db_nos.is_empty() {
        let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
        let pool = AiosDBManager::get_db_pool(&url, project).await?;
        db_nos = query_db_nums_of_mdb(mdb, &mgr.db_option.module, &pool).await?;
        db_nos.sort();
        println!("当前mdb的所有dbnos: {:?}", db_nos);
    }

    let adb = mgr.get_arango_db().await?;
    // dbg!(&db_nos);
    let scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let replace_mesh = mgr.db_option.replace_mesh;

    for db_no in db_nos {
        println!("开始处理db: {db_no}");
        let d_types = &mgr.db_option.debug_refno_types;
        let not_debug = db_option.debug_refno_types.is_empty();
        let mut run_cache_cata = d_types.iter().any(|x| x == "CATA");
        let mut run_cache_loop = d_types.iter().any(|x| x == "LOOP");
        let mut run_cache_prim = d_types.iter().any(|x| x == "PRIM");

        let mut shape_insts_data = ShapeInstancesData::default();
        let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
        let unit_box_aabb = Aabb::new(Point3::new(-0.5, -0.5, -0.5), Point3::new(0.5, 0.5, 0.5));
        shape_insts_data.insert_geos_data(
            TUBI_GEO_HASH.to_string(),
            EleInstGeosData {
                inst_key: TUBI_GEO_HASH.to_string(),
                refno: Default::default(),
                insts: vec![EleInstGeo {
                    geo_hash: TUBI_GEO_HASH,
                    refno: Default::default(),
                    owner_pos_refno: Default::default(),
                    geo_param: PdmsGeoParam::PrimSCylinder(SCylinder::default()),
                    pts: vec![],
                    aabb: Some(unit_cyli_aabb),
                    transform: Default::default(),
                    visible: true,
                    is_tubi: true,
                    geo_type: GeoBasicType::Tubi,
                }],
                aabb: Some(unit_cyli_aabb),
                type_name: "TUBI".to_string(),
                ptset_map: Default::default(),
            },
        );
        shape_insts_data.insert_geos_data(
            BOXI_GEO_HASH.to_string(),
            EleInstGeosData {
                inst_key: BOXI_GEO_HASH.to_string(),
                refno: Default::default(),
                insts: vec![EleInstGeo {
                    geo_hash: BOXI_GEO_HASH,
                    refno: Default::default(),
                    owner_pos_refno: Default::default(),
                    geo_param: PdmsGeoParam::PrimBox(SBox::default()),
                    pts: vec![],
                    aabb: Some(unit_box_aabb),
                    transform: Default::default(),
                    visible: true,
                    is_tubi: true,
                    geo_type: GeoBasicType::Tubi,
                }],
                aabb: Some(unit_box_aabb),
                type_name: "BOXI".to_string(),
                ptset_map: Default::default(),
            },
        );

        let instance_mgr = Arc::new(RwLock::new(shape_insts_data));

        let target_dbnos = [db_no];
        let root_refnos = mgr.get_gen_model_root_refnos(&target_dbnos).await?;
        // dbg!(&root_refnos);
        if root_refnos.is_empty() {
            println!("输入的调试参考号或者db号不正确");
            continue;
        }

        //提前缓存好，
        let target_ploo_refnos = mgr
            .get_gen_model_target_refnos(GeoEnum::PLOO, &target_dbnos, false)
            .await?;
        let loop_sjus_map = DashMap::new();
        target_ploo_refnos.iter().for_each(|r| {
            let Ok(loop_att) = mgr.get_attr_from_localdb(*r) else {
                return;
            };
            let owner = loop_att.get_owner().unwrap_or_default();
            let mut height = loop_att
                .get_f32("HEIG")
                .unwrap_or(loop_att.get_f32("HEIG").unwrap_or_default());
            let sjus = loop_att.get_str("SJUS").unwrap_or_default();
            let off_z = cal_sjus_value(sjus, height);
            //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
            loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
        });

        let loop_sjus_map_arc = Arc::new(loop_sjus_map);
        //元件库的模型计算
        //求出有多少个是一样的模型
        let target_cata_refnos = mgr
            .get_gen_model_target_refnos(GeoEnum::CATA_BRAN_AND_HANGER_REUSE, &target_dbnos, false)
            .await?;
        println!("使用管道或者支吊架元件库数量: {}", target_cata_refnos.len());
        //查询出branch 和 branch 下的子节点
        let mut branch_refnos_map = DashMap::new();
        let mut refno_lstube_map = DashMap::new();
        let mut lstube_bores_map = DashMap::new();
        let mut bran_comp_eles = vec![];
        for refno in &target_cata_refnos {
            let att = mgr.get_attr_from_localdb(*refno).unwrap_or_default();

            let children = query_children_order_aql(&adb, *refno).await?;
            if children.is_empty() && !CATA_HAS_TUBI_GEO_NAMES.contains(&att.get_type()) {
                continue;
            }
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            //求出元件对应的outside bore
            branch_refnos_map.insert(*refno, children);
        }

        let lstube_refnos = mgr
            .query_foreign_refnos(&bran_comp_eles, &[&["LSRO", "LSTU"]], &["CATR"], &[], 2)
            .await?;
        for c in 0..bran_comp_eles.len() {
            refno_lstube_map.insert(bran_comp_eles[c], lstube_refnos[c]);
        }
        let lstube_set = lstube_refnos
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter();
        for l in lstube_set {
            let Ok(att) = mgr.get_attr_from_localdb(l) else {
                continue;
            };
            let params = att.get_f64_vec("PARA").unwrap_or_default();
            let gtype = att.get_as_string("GTYP").unwrap_or_default();
            if params.len() >= 2 {
                // let type_name = db1_dehash(params[2] as u32);
                // dbg!(type_name);
                let bore = params[if gtype.as_str() == "TUBE" { 1 } else { 0 }] as f32;
                lstube_bores_map.insert(l, bore);
            }
        }
        // dbg!(&lstube_bores_map);
        let target_bran_reuse_cata_map = mgr
            .get_gen_model_map_by_cata_hash(
                GeoEnum::CATA_BRAN_AND_HANGER_REUSE,
                &target_dbnos,
                true,
                false,
            )
            .await?;
        let target_single_reuse_cata_map = mgr
            .get_gen_model_map_by_cata_hash(GeoEnum::CATA_SINGLE_REUSE, &target_dbnos, false, false)
            .await?;
        let target_single_cata_map = mgr
            .get_gen_model_map_by_cata_hash(
                GeoEnum::CATA_WITHOUT_REUSE,
                &target_dbnos,
                false,
                false,
            )
            .await?;
        #[cfg(debug_assertions)]
        {
            dbg!(&target_bran_reuse_cata_map.len());
            dbg!(target_bran_reuse_cata_map
                .iter()
                .map(|x| (x.key().clone(), x.value().group_refnos.clone()))
                .collect::<Vec<_>>());
            dbg!(target_single_reuse_cata_map.len());
            dbg!(&target_single_cata_map.len());
        }

        let mut has_run_cata = false;
        if run_cache_cata {
            let mut handles = vec![];
            //bran，hanger下需要重用的模型
            if !target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty() {
                let scom_info_map_clone = scom_info_map.clone();
                let mgr_clone = mgr.clone();
                let instance_mgr_clone = instance_mgr.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
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

            ///需要重用的类型
            if !target_single_reuse_cata_map.is_empty() {
                let scom_info_map_clone = scom_info_map.clone();
                let mgr_clone = mgr.clone();
                let instance_mgr_clone = instance_mgr.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
                        Arc::new(target_single_reuse_cata_map),
                        Arc::new(Default::default()),
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
                let scom_info_map_clone = scom_info_map.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let instance_mgr_clone = instance_mgr.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
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

        let mut has_geom_refnos = vec![];
        for root_refno in root_refnos.clone() {
            let refnos = mgr.query_refnos_has_geos(root_refno).await?;
            has_geom_refnos.extend_from_slice(&refnos);
        }
        dbg!(has_geom_refnos.len());
        if !has_geom_refnos.is_empty() {
            let target_loop_refnos = mgr
                .get_gen_model_target_refnos(GeoEnum::LOOP_AND_PLOO, &target_dbnos, false)
                .await?;
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

            let target_prim_refnos = mgr
                .get_gen_model_target_refnos(GeoEnum::PRIM, &target_dbnos, false)
                .await?;
            println!("使用基本体数量: {}", target_prim_refnos.len());
            if run_cache_prim && !target_prim_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let handle = tokio::spawn(async move {
                    gen_prim_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        target_prim_refnos.as_slice(),
                        sjus_map_clone,
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
            // dbg!(&has_pos_neg_map);
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
                    has_pos_neg_map.into_iter().for_each(
                        |(comp_refno, (mut pos_refnos, origin_neg_refnos))| {
                            println!("正在处理: {} 下的负实体", comp_refno);

                            let Ok(children_refnos) = mgr.get_children_from_localdb(comp_refno)
                                else {
                                    return;
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
                            let mut inst_info_result_map_clone =
                                compound_inst_info_result_map.clone();
                            let mut inst_geos_result_map_clone =
                                compound_inst_geos_result_map.clone();

                            let mut batch_manifolds = vec![];
                            //没有正实体的情况，直接跳过
                            // if neg_refnos.is_empty() {
                            //     return;
                            // }
                            pos_refnos.push(comp_refno);
                            // dbg!(&pos_refnos);
                            let Some(w_trans) =
                                trans_map.get(&comp_refno).map(|x| x.value().clone())
                                else {
                                    return;
                                };
                            // dbg!(w_trans);
                            let mut total_refnos = vec![comp_refno];
                            total_refnos.extend_from_slice(&neg_refnos);
                            let inverse_mat = w_trans.compute_matrix().inverse();

                            let origin_aabb =
                                { inst_data.get_info(&comp_refno).map(|x| x.aabb).flatten() };

                            let mut neg_refnos = vec![];
                            let mut found_non_manifold = false;
                            //如果数量比较少，直接用慢的csg方法
                            let mut use_csg = total_refnos.len() < 20;
                            use_csg = false;
                            for (index, t_refno) in total_refnos.into_iter().enumerate() {
                                let geos_info_tmp = {
                                    inst_data
                                        .get_info(&t_refno)
                                        .or(inst_data.get_ngmr_info(&t_refno))
                                        .cloned()
                                };
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
                                    let Some(mesh) = mesh_mgr_clone.get_mesh(geo_inst.geo_hash)
                                        else {
                                            continue;
                                        };
                                    let Some(aabb) = mesh_mgr_clone.get_aabb(geo_inst.geo_hash)
                                        else {
                                            continue;
                                        };
                                    let geo_mat = geos_info.world_transform;
                                    let ele_mat = inverse_mat * geo_mat.compute_matrix();
                                    let mut local_mat =
                                        ele_mat * geo_inst.transform.compute_matrix();

                                    //如果是第一个正实体，需要生成模型计算
                                    //如果是负实体，需要生成模型计算
                                    let is_neg =
                                        !pos_refnos.contains(&t_refno) || geo_inst.is_neg();
                                    if t_refno == comp_refno || is_neg {
                                        if pos_refnos.contains(&t_refno) {
                                            pos_aabb = aabb;
                                        } else {
                                            neg_refnos.push(t_refno);
                                        }
                                        if is_neg {
                                            geo_inst.owner_pos_refno = pos_refno;
                                            //根据类型来考虑是否需要扩大负实体
                                            let mut center: Vec3 = aabb.center().into();
                                            let t_mat = Mat4::from_translation(center);
                                            let mut s = 1.01;
                                            let s_mat = if matches!(
                                                geo_inst.geo_param,
                                                PdmsGeoParam::PrimRevolution(_)
                                            ) {
                                                //如果是旋转体，xy方向都适当放大一点
                                                if aabb.contains(&pos_aabb) {
                                                    s = 1.03;
                                                }
                                                Mat4::from_scale(Vec3::new(1.0, s, s))
                                            } else {
                                                Mat4::from_scale(Vec3::new(1.0, 1.0, s))
                                            };
                                            let inv_t_mat = Mat4::from_translation(-center);
                                            local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                                        }

                                        #[cfg(debug_assertions)]
                                        {
                                            dbg!(t_refno);
                                            dbg!(mesh.vertices.len());
                                        }

                                        let manifold: ManifoldRust = (mesh, &local_mat).into();
                                        // let manifold: ManifoldRust = (mesh, &Mat4::IDENTITY).into();
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
                            use_csg = false;
                            // ----- 基本体的负实体运算  ----- //
                            let mut plant_geo_data = {
                                if batch_manifolds.len() < 2 {
                                    return;
                                }
                                let mut src_manifold = batch_manifolds.remove(0);
                                let final_manifold =
                                    src_manifold.batch_boolean_subtract(&batch_manifolds);
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
                                owner_pos_refno: Default::default(),
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
                                generic_type: mgr.get_generic_type(comp_refno),
                                aabb: origin_aabb.clone(),
                                world_transform: w_trans,
                                //cata hash 用作唯一的标识符就行，后面需要变名称
                                cata_hash: Some(inst_key.to_string()),
                                flow_pt_indexs: vec![],
                                geo_type: GeoBasicType::Compound,
                            };
                            // dbg!(&comp_geos_info);
                            inst_info_result_map_clone.insert(comp_refno, comp_geos_info);
                            let comp_type = mgr
                                .get_refno_basic(comp_refno)
                                .unwrap()
                                .get_type()
                                .to_string();
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
                        },
                    );

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
            if !shape_insts_data.ngmr_inst_info_map.is_empty() {
                let mut boolean_ngmr_map = HashMap::new();
                ///查找是否是某些参考号的子节点
                for (&refno, geos_info) in &shape_insts_data.ngmr_inst_info_map {
                    // dbg!(refno);
                    if let Some(parent) = mgr.get_ancestor_refno_till_type(
                        refno,
                        TOTAL_CONTAIN_NGMR_GEO_NAEMS.as_slice(),
                    ) {
                        // dbg!(parent);
                        let Some(parent_geos_info) = shape_insts_data.get_inst_info(parent) else {
                            continue;
                        };
                        let Some(geos_data) = shape_insts_data.get_inst_geos_data(parent_geos_info)
                            else {
                                continue;
                            };
                        boolean_ngmr_map
                            .entry(parent)
                            .or_insert_with(|| Vec::new())
                            .push(refno);
                    }
                }
                //开始进行ngmr 的 boolean操作
                println!("开始处理ngmr的负实体模型");
                for (parent, refnos) in boolean_ngmr_map {
                    //更新ngmr的owner
                    {
                        for ngmr_ele_refno in refnos.clone() {
                            // dbg!(ngmr_ele_refno);
                            if let Some(inst_geos_data) =
                                shape_insts_data.get_inst_geos_data_mut_by_refno(ngmr_ele_refno)
                            {
                                for ele in inst_geos_data.insts.iter_mut() {
                                    if ele.geo_type == GeoBasicType::CateCrossNeg {
                                        ele.owner_pos_refno = parent;
                                        ele.refno = ngmr_ele_refno;
                                        // dbg!((ele.refno, ele.owner_pos_refno));
                                    }
                                }
                            }
                        }
                    }

                    //这里优先取compound的数据参与计算，如果没有再使用原生的info数据
                    let Some(parent_geos_info) = shape_insts_data
                        .get_compound_info(parent)
                        .or(shape_insts_data.get_inst_info(parent))
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
                    let mut p_inst = parent_geos_data.insts[0].clone();
                    let parent_matrix_inverse =
                        parent_geos_info.world_transform.compute_matrix().inverse();
                    let mut pos_monifolds = vec![];
                    for p_inst in parent_geos_data.insts.clone() {
                        let Some(parent_mesh) = mesh_mgr.get_mesh(p_inst.geo_hash) else {
                            continue;
                        };
                        let mat4 = p_inst.transform.compute_matrix();
                        let mut tmp_manifold: ManifoldRust = (parent_mesh, &mat4).into();
                        pos_monifolds.push(tmp_manifold);
                    }
                    let mut parent_manifold = ManifoldRust::batch_boolean(&pos_monifolds, 0);

                    #[cfg(debug_assertions)]
                    dbg!(parent_manifold.num_tri());
                    let mut neg_ms = vec![];
                    for refno in refnos {
                        let Some(geos_info) = shape_insts_data.get_ngmr_info(&refno) else {
                            continue;
                        };
                        let Some(geos_data) = shape_insts_data.get_inst_geos_data(geos_info) else {
                            continue;
                        };
                        let relative_mat =
                            parent_matrix_inverse * geos_info.world_transform.compute_matrix();
                        for g in &geos_data.insts {
                            let local_mat = relative_mat * g.transform.compute_matrix();
                            let Some(mesh) = mesh_mgr.get_mesh(g.geo_hash) else {
                                continue;
                            };
                            let Some(aabb) = mesh_mgr.get_aabb(g.geo_hash) else {
                                continue;
                            };
                            //根据类型来考虑是否需要扩大负实体
                            let mut center: Vec3 = aabb.center().into();
                            let t_mat = Mat4::from_translation(center);
                            let s = 1.005;
                            // let s_mat = Mat4::from_scale(Vec3::new(1.0, 1.0, s));
                            let s_mat = Mat4::from_scale(Vec3::splat(s));
                            let inv_t_mat = Mat4::from_translation(-center);
                            let final_mat = local_mat * t_mat * s_mat * inv_t_mat;

                            let mut neg_manifold: ManifoldRust = (mesh, &final_mat).into();
                            // dbg!(refno);
                            // dbg!(neg_manifold.num_tri());
                            if neg_manifold.num_tri() != 0 {
                                neg_ms.push(neg_manifold);
                            }
                        }
                    }
                    let mut final_manifold = parent_manifold.batch_boolean_subtract(&neg_ms);
                    #[cfg(debug_assertions)]
                    dbg!(final_manifold.num_tri());
                    //相当于更新
                    let mut new_geos_info = parent_geos_info.clone();
                    //如果和ngmr发生相减后， 没有复用了
                    // new_geos_info.cata_hash = Some(parent.to_url_refno());
                    new_geos_info.update_to_compound(Some(parent.to_url_refno().as_str()));
                    let geo_hash = new_geos_info.get_inst_key_u64();
                    p_inst.geo_hash = geo_hash;
                    p_inst.transform = Transform::IDENTITY;
                    let mut mesh: PlantMesh = (final_manifold.clone()).into();
                    for f in neg_ms {
                        // #[cfg(target_os = "macos"|| target_os = "linux")]
                        // f.destroy();
                    }
                    // final_manifold.destroy();
                    let mut new_geos_data = parent_geos_data.clone();
                    new_geos_data.insts = vec![p_inst];
                    new_geos_data.inst_key = geo_hash.to_string();
                    // new_geos_data.aabb = d.aabb;

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
            save_instance_to_graph_db(&mgr, &inst_data).await?;
        }

        {
            let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
            dbg!(mesh_mgr.len());
            save_mesh_to_arango_db(&mgr, &mut mesh_mgr, replace_mesh).await?;
        }

        println!("{db_no} 生成完毕。");
    }

    println!("生成所有模型时间: {}ms", time.elapsed().as_millis());
    Ok(true)
}

async fn query_tubi_size(
    mgr: &AiosDBManager,
    refno: RefU64,
    tubi_cat_ref: RefU64,
    is_hang: bool,
    scom_info_map: &Arc<RwLock<HashMap<RefU64, ScomInfo>>>,
    axis_map: Option<&BTreeMap<i32, CateAxisParam>>,
) -> anyhow::Result<TubiSize> {
    // if let Ok(tubi_att) = mgr.get_attr_from_localdb(tubi_ref)
    {
        // dbg!(&tubi_att);
        // let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        //只是为了获得外径而已
        let tubi_geoms_info = resolve_desi_comp(
            Some(mgr),
            refno,
            Some(tubi_cat_ref),
            &scom_info_map,
            axis_map,
        )
            .await
            .unwrap_or_default();
        let mut has_tube_geom = false;
        for geom in &tubi_geoms_info.geometries {
            if let TubeImplied(d) = geom {
                return Ok(TubiSize::BoreSize(d.diameter));
            } else if let BoxImplied(d) = geom {
                return Ok(TubiSize::BoxSize((d.width, d.height)));
            }
        }

        if !has_tube_geom {
            if let Ok(cat_att) = mgr.get_attr_from_localdb(tubi_cat_ref) {
                let params = cat_att.get_f64_vec("PARA").unwrap_or_default();
                if params.len() >= 2 {
                    let tubi_bore = params[if is_hang { 0 } else { 1 }] as f32;
                    return Ok(TubiSize::BoreSize(tubi_bore));
                }
            };
        }
    }
    return Ok(TubiSize::None);
}
