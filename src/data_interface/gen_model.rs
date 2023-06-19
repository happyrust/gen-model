use std::collections::{HashMap, HashSet};
use std::default::default;
use std::io::Read;
use std::mem::take;
use std::ptr::replace;
use std::sync::Arc;
use std::time::Instant;
use aios_core::consts::CYLI_HASH;
use aios_core::options::DbOption;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::parsed_data::geo_params_data::CateGeoParam::TubeImplied;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::cylinder::{LCylinder, SCylinder};
use aios_core::prim_geo::TUBI_GEO_HASH;
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdge};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::math_tool;
use anyhow::anyhow;
use bevy::log::{error, info};
use bevy::prelude::Transform;
use dashmap::{DashMap, DashSet};
use futures::future::ok;
use glam::{Mat3, Vec3};
use nalgebra::Point3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Isometry, Vector};
use rayon::iter::IntoParallelIterator;
use tokio::sync::{Mutex, RwLock};
use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::aql_api::children::query_children_order_aql;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn::geo::create_profile_geos;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::save_arangodb_doc;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::data_interface::db_manager::GeoEnum;
use crate::graph_db::pdms_inst_arango::save_instance_to_graph_db;
use crate::graph_db::pdms_mesh_arango::{save_mesh_to_arango_db, save_mesh_to_local_db};
use rayon::iter::ParallelIterator;

/// 生成基本体的几何数据
pub async fn gen_prim_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    db_option: &DbOption,
    prim_refnos: &[RefU64],
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let batch_size = mgr.db_option.gen_model_batch_size;
    let prim_cnt = prim_refnos.len();
    if prim_cnt == 0 { return Ok(true); }
    let batch_chunks_cnt = prim_cnt / batch_size + 1;
    let mut handles = vec![];
    let all_refnos = Arc::new(prim_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(prim_cnt));
    let replace_mesh = db_option.replace_mesh;
    let tol_ratio = db_option.mesh_tol_ratio;
    for i in 0..batch_chunks_cnt as usize {
        let mgr = mgr.clone();
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
                let mut cached_mesh_mgr = mgr.cached_mesh_mgr.write().await;
                let mut shape_insts_data = instance_mgr.write().await;
                let refno = all_refnos[j];
                println!(
                    "正在处理基本体的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let Ok(Some(trans_origin)) = mgr
                    .get_world_transform(refno)
                    .await else {
                    continue;
                };
                let mut geos_info = EleGeosInfo {
                    refno,
                    visible: true,
                    generic_type: mgr.get_generic_type(refno),
                    aabb: None,
                    world_transform: trans_origin,
                    cata_hash: None,
                    flow_pt_indexs: vec![],
                    geo_type: Default::default(),
                };
                let mut geo_insts = vec![];
                let mut item_trans = Transform::IDENTITY;

                let attr = mgr.get_attr_from_localdb(refno).unwrap_or_default();
                let mut geo_param = PdmsGeoParam::Unknown;
                //需要限制负实体的大小，太大，导致负运算失败
                let limit_size: Option<f32> = if GENRAL_NEG_NOUN_NAMES.contains(&attr.get_type()) {
                    if let Some(parent_inst) = shape_insts_data.inst_info_map.get(&attr.get_owner().unwrap_or_default()) {
                        parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0)
                    } else {
                        None
                    }
                } else {
                    None
                };
                // let mut geo_aabb = None;
                let Some(brep_shape) = attr.create_brep_shape(limit_size) else {
                    continue;
                };
                if !brep_shape.check_valid() {
                    continue;
                }

                item_trans = brep_shape.get_trans();
                if item_trans.is_nan() { continue; }
                geo_param = brep_shape
                    .convert_to_geo_param()
                    .unwrap_or(PdmsGeoParam::Unknown);
                let geo_hash = brep_shape.hash_unit_mesh_params();
                let mut geo_aabb = if !replace_mesh && let Ok(aabb) = mgr.get_mesh_aabb_from_localdb(geo_hash) {
                    aabb
                } else {
                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(brep_shape, replace_mesh, tol_ratio) else {
                        continue;
                    };
                    aabb
                };
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                let tr = &item_trans;
                // geo_aabb =
                // let Some(mut geo_aabb) = cached_mesh_mgr.get_bbox(&geo_hash) else {
                //     continue;
                // };
                let ele_aabb = aabb_apply_transform(&geo_aabb, &tr);
                let inst_geo = EleInstGeo {
                    geo_hash,
                    refno,
                    pts: Default::default(),
                    aabb: Some(geo_aabb),
                    transform: *tr,
                    geo_param,
                    visible,
                    is_tubi: false,
                    geo_type: if attr.is_neg() { GeoBasicType::Neg } else { GeoBasicType::Pos },
                };
                geo_insts.push(inst_geo);
                geos_info.aabb = Some(
                    ele_aabb.transform_by(&Isometry {
                        rotation: trans_origin.rotation.into(),
                        translation: trans_origin.translation.into(),
                    }),
                );
                if geo_insts.len() > 0 {
                    shape_insts_data.insert_info(refno, geos_info);
                    shape_insts_data.insert_geos_data(*refno, EleInstGeosData {
                        inst_key: *refno,
                        refno,
                        insts: geo_insts,
                        aabb: Some(geo_aabb),
                        type_name: attr.get_type().to_string(),
                        ptset_map: Default::default(),
                        reuse_unit: true,
                    });
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


pub async fn gen_loop_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    db_option: &DbOption,
    loop_refnos: &[RefU64],
) -> anyhow::Result<bool> {
    let t = Instant::now();
    let batch_size = mgr.db_option.gen_model_batch_size;
    let mut is_debug = false;
    let loop_cnt = loop_refnos.len();
    if loop_cnt == 0 { return Ok(true); }
    //处理loop elements
    let batch_chunks_cnt = loop_cnt / batch_size + 1;
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
        let handle = tokio::spawn(async move {
            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > loop_cnt as usize {
                end_idx = loop_cnt as usize;
            }
            for j in start_idx..end_idx {
                let mut cached_mesh_mgr = mgr.cached_mesh_mgr.write().await;
                let mut shape_insts_data = instance_mgr.write().await;
                let loop_refno = all_loop_refnos[j];
                let Ok(Some(trans_origin)) = mgr
                    .get_world_transform(loop_refno)
                    .await else {
                    continue;
                };
                let Some(refno_basic) = mgr.get_refno_basic(loop_refno) else {
                    continue;
                };
                let parent_basic = mgr.get_owner_ref_basic(loop_refno).unwrap();
                let target_type = parent_basic.get_type();
                let parent_refno = refno_basic.get_owner();
                println!(
                    "正在处理loops类型的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    parent_refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                *processed_cnt.lock().await -= 1;
                let mut geos_info = EleGeosInfo {
                    refno: parent_refno,
                    cata_hash: None,
                    visible: true,
                    world_transform: trans_origin,
                    generic_type: mgr.get_generic_type(parent_refno),
                    aabb: None,
                    flow_pt_indexs: vec![],
                    geo_type: Default::default(),
                };
                let mut loop_verts: Vec<Vec3> = vec![];
                let mut fradius_vec: Vec<f32> = vec![];

                if let Ok(children_refs) = mgr.get_children_refs(loop_refno).await {
                    for x in children_refs {
                        if let Ok(a) = mgr.get_implicit_attr(x, Some(vec!["POS", "FRAD"])).await {
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
                }
                if loop_verts.is_empty() { continue; }
                let mut parent_att = mgr.get_attr_from_localdb(parent_refno).unwrap_or_default();
                let mut geo_hash = 0;
                let mut geo_aabb = None;
                let mut item_trans = Transform::IDENTITY;
                let mut geo_param = PdmsGeoParam::Unknown;
                // dbg!(&target_type);
                match target_type {
                    "NREV" | "REVO" => {
                        let angle = parent_att.get_f32("ANGL").unwrap_or_default();
                        if angle >= f32::EPSILON {
                            let revo = Box::new(Revolution {
                                verts: loop_verts,
                                fradius_vec,
                                angle,
                                ..Default::default()
                            });
                            if revo.check_valid() {
                                // dbg!(&revo);
                                item_trans = revo.get_trans();
                                geo_param = revo
                                    .convert_to_geo_param()
                                    .unwrap_or(PdmsGeoParam::Unknown);
                                geo_hash = revo.hash_unit_mesh_params();
                                geo_aabb = if !replace_mesh && let Ok(aabb) = mgr.get_mesh_aabb_from_localdb(geo_hash) {
                                    Some(aabb)
                                } else {
                                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(revo, replace_mesh, tol_ratio) else {
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
                        let mut height = loop_attr
                            .get_f32("HEIG")
                            .unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
                        if height < f32::EPSILON {
                            println!("{}： 的height太小为: {}", parent_refno, height);
                            continue;
                        }
                        let i: usize = 0;
                        //fix 1516 的情况  =24381/36952，当为DBOT的时候，会变成DISH
                        let sjus = loop_attr.get_str("SJUS").unwrap_or_default();
                        //开始处理有偏移的情况
                        {
                            if loop_attr.get_type() == "NXTR" {
                                if let Some(parent_inst) = shape_insts_data.get_inst_info(loop_attr.get_owner().unwrap_or_default()) {
                                    if let Some(h) = parent_inst.aabb.map(|x| x.bounding_sphere().radius * 2.0) {
                                        height = height.min(h);
                                        // dbg!(height);
                                        println!("Height 太长，裁剪为: {}", height);
                                    }
                                }
                            };
                            // dbg!(height);
                            let extrusion = Box::new(Extrusion {
                                verts: loop_verts,
                                height,
                                fradius_vec,
                                ..Default::default()
                            });
                            // dbg!(&extrusion);
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
                        };
                        let off_z = if sjus == "UTOP" || sjus == "DTOP" {
                            -height
                        } else if sjus == "UCEN" || sjus == "DCEN" {
                            -height / 2.0
                        } else {
                            0.0
                        };
                        item_trans.translation =
                            item_trans.translation + Vec3::new(0.0, 0.0, off_z);
                    }
                    _ => {}
                }
                let Some(mut geo_aabb) = geo_aabb else {
                    println!("LOOP 有问题：{} ", loop_refno.to_refno_string());
                    continue;
                };

                let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                if item_trans.is_nan() { continue; }
                let tr: Transform = item_trans;
                let ele_aabb = aabb_apply_transform(&geo_aabb, &tr);
                let geom_inst = EleInstGeo {
                    geo_hash,
                    refno: parent_refno,
                    pts: Default::default(),
                    aabb: Some(geo_aabb),
                    transform: tr,
                    visible,
                    is_tubi: false,
                    geo_param,
                    geo_type: if parent_att.is_neg() { GeoBasicType::Neg } else { GeoBasicType::Pos },
                };
                geos_info.aabb = Some(aabb_apply_transform(&ele_aabb, &trans_origin));
                shape_insts_data.insert_info(parent_refno, geos_info);
                shape_insts_data.insert_geos_data(*parent_refno, EleInstGeosData {
                    inst_key: *parent_refno,
                    refno: parent_refno,
                    insts: vec![geom_inst],
                    aabb: Some(ele_aabb),
                    type_name: parent_att.get_type().to_string(),
                    ptset_map: Default::default(),
                    reuse_unit: false,
                });
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
    let cur_ele = mgr.get_refno_basic(design_refno).ok_or(anyhow!("Element不存在"))?;
    let type_name = cur_ele.get_type();
    let owner = mgr.get_owner_ref_basic(design_refno);
    if owner.is_none() {
        return Ok(RefU64::default());
    }
    let desi_att = mgr.get_attr_from_localdb(design_refno)?;
    let geoms_info = resolve_desi_comp(Some(mgr.as_ref()), design_refno, None, scom_info_map)
        .await
        .unwrap_or_default();
    // dbg!(geoms.geometries.len());
    if type_name == "SCTN"
        || type_name == "STWALL"
        || type_name == "GENSEC"
        || type_name == "WALL"
    {
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
            axis_map,
        } = geoms_info;
        for (i, geom) in geometries.into_iter().enumerate() {
            // dbg!((i, &geom));
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
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

/// 生成元件库的branch型几何体
pub async fn gen_cata_geos(
    mgr: Arc<AiosDBManager>,
    main_instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>>,
    db_option: &DbOption,
    target_cata_map: Arc<DashMap<u64, CataHashRefnoKV>>,
    //branch 下按顺序的清单
    branch_map: Arc<DashMap<RefU64, Vec<PdmsElement>>>,
    refno_lstube_map: Arc<DashMap<RefU64, RefU64>>,
    lstube_bores_map: Arc<DashMap<RefU64, f32>>,
) -> anyhow::Result<bool> {
    let batch_size = mgr.db_option.gen_model_batch_size;
    let t = Instant::now();
    let unique_cata_cnt = target_cata_map.len();
    let batch_chunks_cnt = unique_cata_cnt / batch_size + 1;
    println!("使用元件库的unique模型总数：{unique_cata_cnt}, 分块数量: {batch_chunks_cnt}");
    let mut handles = vec![];
    let processed_cnt = Arc::new(Mutex::new(unique_cata_cnt));
    let mut tubi_aqls = Arc::new(DashMap::new());
    let replace_mesh = db_option.replace_mesh;
    let tol_ratio = db_option.mesh_tol_ratio;

    // dbg!(&target_cata_map);
    let all_unique_keys = Arc::new(target_cata_map.iter().map(|x| x.cata_hash).collect::<Vec<_>>());
    if !all_unique_keys.is_empty() {
        for i in 0..batch_chunks_cnt as usize {
            let mgr = mgr.clone();
            let instance_mgr = main_instance_mgr.clone();
            let all_unique_keys = all_unique_keys.clone();
            let processed_cnt = processed_cnt.clone();
            let scom_info_map = scom_info_map.clone();
            let target_cata_map = target_cata_map.clone();

            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > unique_cata_cnt as usize {
                    end_idx = unique_cata_cnt as usize;
                }
                println!("当前范围: {start_idx} ~ {end_idx}");
                for j in start_idx..end_idx {
                    let Some(cata_hash) = all_unique_keys[j] else {
                        continue;
                    };
                    if cata_hash == 0 { continue; }
                    let target_cata = target_cata_map.get(&cata_hash).unwrap();
                    let mut cached_mesh_mgr = mgr.cached_mesh_mgr.write().await;
                    let mut shape_insts_data = instance_mgr.write().await;
                    let mut target_geo_data_option = None;
                    let mut process_refno = None;
                    //reuse代表是否重用，如果
                    if replace_mesh || target_cata.exist_geo.is_none() {
                        //如果没有已有的，需要生成
                        let refno = target_cata.group_refnos[0];
                        process_refno = Some(refno);
                        println!(
                            "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                            j,
                            refno.to_refno_string(),
                            processed_cnt.lock().await.to_owned()
                        );
                        *processed_cnt.lock().await -= 1;
                        //在这里直接处理完所有需要处理的transform
                        let brep_shapes_map = CateBrepShapeMap::new();
                        let current_att = mgr.get_attr_from_localdb(refno).unwrap_or_default();
                        let mut refno_ptset_map = DashMap::new();
                        let cur_type = current_att.get_type();

                        let Ok(cat_refno) = gen_cata_single_geoms(
                            mgr.clone(),
                            refno,
                            &brep_shapes_map,
                            &refno_ptset_map,
                            &scom_info_map,
                        ).await else {
                            continue;
                        };
                        let mut is_reuse_unit = false;
                        ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                        for (ele_refno, shapes) in brep_shapes_map {
                            let Ok(Some(mut o)) = mgr
                                .get_world_transform(ele_refno)
                                .await else {
                                continue;
                            };
                            let Ok(ele_att) = mgr.get_attr_from_localdb(ele_refno) else {
                                continue;
                            };

                            let is_scaled_reuse = SCALED_REUSE_GEO_NAMES.contains(&ele_att.get_type());
                            // let Ok(Some(gmse_refno)) = mgr.query_foreign_refno(ele_refno,
                            //                                                    &[&["SPRE", "CATR"]], &["GMRE", "GSTR"],
                            //                                                    &[]).await else {
                            //     continue;
                            // };
                            // dbg!(gmse_refno);
                            // dbg!(ele_refno);
                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            // let pos_neg_map = mgr.query_refnos_has_pos_neg_map(gmse_refno).await.unwrap_or_default();
                            // let pos_neg_map: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> = HashMap::new();
                            // let has_neg = !pos_neg_map.is_empty();
                            // let mut neg_refnos = pos_neg_map.values().map(|(_, neg)| neg).flatten().cloned().collect::<Vec<_>>();
                            // let mut pos_refnos = pos_neg_map.values().map(|(pos, _)| pos).flatten().cloned().collect::<Vec<_>>();
                            let mut geos_info = EleGeosInfo {
                                refno: ele_refno,
                                cata_hash: Some(cata_hash),
                                visible: true,
                                generic_type: mgr.get_generic_type(ele_refno),
                                aabb: None,
                                world_transform: o,
                                flow_pt_indexs: if !ele_att.contains_attr_name("ARRI") { vec![] } else {
                                    vec![
                                        ele_att.get_i32("ARRI").unwrap_or(-1),
                                        ele_att.get_i32("LEAV").unwrap_or(-1),
                                    ]
                                },
                                geo_type: Default::default(),
                            };

                            let mut geo_insts = vec![];
                            let mut cata_aabb: Option<Aabb> = None;
                            //将负实体和正实体统计出来
                            let mut merged_cata_aabb: Option<Aabb> = None;
                            for shape in shapes {
                                let CateBrepShape {
                                    refno,
                                    brep_shape,
                                    transform,
                                    visible,
                                    is_tubi,
                                    pts,
                                    ..
                                } = shape;
                                if !visible || !brep_shape.check_valid() {
                                    continue;
                                }
                                let mut trans = brep_shape.get_trans();
                                if is_scaled_reuse {
                                    if brep_shape.is_reuse_unit() {
                                        let attr = mgr.get_attr_from_localdb(ele_refno).unwrap_or_default();
                                        let poss = attr.get_vec3("POSS").unwrap_or_default();
                                        let pose = attr.get_vec3("POSE").unwrap_or_default();
                                        let v = (pose - poss).length();
                                        if v < f32::EPSILON {
                                            continue;
                                        }
                                        geos_info.world_transform.scale = Vec3::new(1.0, 1.0, v);
                                        trans.scale = Vec3::ONE;
                                        is_reuse_unit = true;
                                    }
                                }
                                let geo_hash = brep_shape.hash_unit_mesh_params();
                                let mut geo_aabb = if !replace_mesh && let Ok(aabb) = mgr.get_mesh_aabb_from_localdb(geo_hash) {
                                    aabb
                                } else {
                                    let Some((_, aabb)) = cached_mesh_mgr.gen_plant_data(brep_shape.clone(), replace_mesh, tol_ratio) else {
                                        continue;
                                    };
                                    aabb
                                };
                                // dbg!(geo_hash);
                                let rot = transform.rotation;
                                let translation =
                                    transform.translation + transform.rotation * trans.translation;
                                let scale = trans.scale;
                                let tmp_aabb = geo_aabb.scaled(&trans.scale.into());
                                let transformed_aabb = tmp_aabb.transform_by(&Isometry {
                                    rotation: rot.into(),
                                    translation: translation.into(),
                                });

                                if let Some(mut cata_aabb) = cata_aabb {
                                    cata_aabb.merge(&transformed_aabb);
                                } else {
                                    cata_aabb = Some(transformed_aabb);
                                }
                                if cata_aabb.is_some() {
                                    if let Some(mut a) = merged_cata_aabb {
                                        a.merge(cata_aabb.as_ref().unwrap());
                                    } else {
                                        merged_cata_aabb = cata_aabb;
                                    }
                                }
                                let transform = Transform {
                                    translation,
                                    rotation: rot,
                                    scale,
                                };
                                if transform.is_nan() { continue; }
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
                                    geo_type: GeoBasicType::Pos,
                                };
                                geo_insts.push(geom_inst);
                            }
                            //需要变换成世界坐标系下的aabb
                            if let Some(a) = merged_cata_aabb {
                                geos_info.aabb = Some(aabb_apply_transform(&a, &geos_info.world_transform));
                            }

                            if let Some(mut aabb) = &mut geos_info.aabb {
                                if aabb.mins.x.is_infinite() {
                                    dbg!(&geos_info);
                                    aabb = Aabb::new(Point3::new(0., 0., 0.), Point3::new(0., 0., 0.));
                                }
                            }
                            if geo_insts.len() > 0 {
                                // dbg!(&geos_info);
                                let inst_key = geos_info.get_inst_key();
                                shape_insts_data.insert_info(ele_refno, geos_info);
                                let d = EleInstGeosData {
                                    inst_key,
                                    refno: cat_refno,
                                    insts: geo_insts,
                                    aabb: merged_cata_aabb,
                                    type_name: cur_type.to_string(),
                                    ptset_map: refno_ptset_map
                                        .remove(&ele_refno)
                                        .map(|x| x.1)
                                        .unwrap_or_default(),
                                    reuse_unit: is_reuse_unit,
                                };
                                target_geo_data_option = Some(d.clone());
                                shape_insts_data.insert_geos_data(inst_key, d);
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
                    if target_geo_data.aabb.is_none() { continue; }

                    //如果已经有了，需要生成transform和bbox那些
                    for ele_refno in target_cata.group_refnos.clone() {
                        if Some(ele_refno) == process_refno {
                            // continue;
                        }
                        println!(
                            "正在处理同类元件库的模型当前参考号：{}",
                            ele_refno.to_refno_string(),
                        );
                        let Ok(Some(mut o)) = mgr
                            .get_world_transform(ele_refno)
                            .await else {
                            continue;
                        };

                        let Some(ref_basic) = mgr.get_refno_basic(ele_refno) else {
                            continue;
                        };
                        let is_scaled_reuse = SCALED_REUSE_GEO_NAMES.contains(&ref_basic.get_type());
                        if is_scaled_reuse && target_geo_data.reuse_unit {
                            let attr = mgr.get_attr_from_localdb(ele_refno).unwrap_or_default();
                            let poss = attr.get_vec3("POSS").unwrap_or_default();
                            let pose = attr.get_vec3("POSE").unwrap_or_default();
                            let v = (pose - poss).length();
                            o.scale = Vec3::new(1.0, 1.0, v);
                        }

                        let mut flow_pt_indexs = vec![];
                        let Some(own_ref_basic) = mgr.get_refno_basic(ref_basic.owner) else {
                            continue;
                        };

                        if CATA_HAS_TUBI_GEO_NAMES.contains(&own_ref_basic.get_type()) {
                            let attr = mgr.get_attr_from_localdb(ele_refno).unwrap_or_default();
                            flow_pt_indexs = vec![
                                attr.get_i32("ARRI").unwrap_or(-1),
                                attr.get_i32("LEAV").unwrap_or(-1),
                            ];
                        }

                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            cata_hash: Some(cata_hash),
                            visible: true,
                            generic_type: mgr.get_generic_type(ele_refno),
                            aabb: Some(aabb_apply_transform(target_geo_data.aabb.as_ref().unwrap(), &o)),
                            world_transform: o,
                            flow_pt_indexs,
                            geo_type: Default::default(),
                        };
                        let inst_key = geos_info.get_inst_key();
                        shape_insts_data.insert_info(ele_refno, geos_info);
                        shape_insts_data.insert_geos_data(inst_key, target_geo_data.clone());
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
    }
    futures::future::join_all(take(&mut handles)).await;

    //先暂时维持和之前一样
    //还是把tubi 抽出来，创造一个tubi geos

    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    //直段需要插入一个单位的cylinder

    let mut inst_tubi_map = HashMap::new();
    //todo 重用直段的生成
    for b in branch_map.iter() {
        let shape_insts_data = main_instance_mgr.read().await;
        let branch_refno = *b.key();
        dbg!(branch_refno);
        let Ok(children_refnos) = mgr.get_children_from_localdb(branch_refno) else{
            continue;
        };
        // dbg!(&children_refnos);
        let mut children = vec![];
        //排一下顺序，后面这个element也是要存在本地
        children_refnos.into_iter().for_each(|x|{
            for c in b.value() {
                //同时过滤掉ATTA
                if c.refno == x && c.get_type_name() != "ATTA" {
                    children.push(c);
                }
            }
        });
        // dbg!(&children);
        let Ok(group_att) = mgr.get_attr_from_localdb(branch_refno) else {
            continue;
        };
        //可能只有branch 元素需要做一遍求解
        let Ok(Some(group_transform)) = mgr
            .get_world_transform(branch_refno)
            .await else {
            continue;
        };
        let htube_pt = group_transform.transform_point(
            group_att
                .get_vec3("HPOS")
                .ok_or(anyhow!("HPOS not exist".to_string()))?,
        );
        let hdir = group_transform
            .transform_point(
                group_att
                    .get_vec3("HDIR")
                    .ok_or(anyhow!("HDIR not exist".to_string()))?,
            )
            .normalize_or_zero();
        let bran_ttube_pt = group_transform.transform_point(
            group_att
                .get_vec3("TPOS")
                .ok_or(anyhow!("TPOS not exist".to_string()))?,
        );

        let is_hang = group_att.get_type() == "HANG";
        let h_ref = group_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();
        // dbg!(h_ref);

        let bran_name = group_att.get_name().0.to_string();
        let mut bore = 0.0f32;
        let mut href_type = "".to_string();
        //todo 头节点的处理
        if let Ok(h_att) = mgr.get_attr_from_localdb(h_ref) {
            href_type = h_att.get_type().to_string();
            let h_cat_ref = h_att.get_foreign_refno("CATR").unwrap_or_default();
            //只是为了获得外径而已
            let tubi_geoms_info =
                resolve_desi_comp(Some(mgr.as_ref()), branch_refno, Some(h_cat_ref), &scom_info_map)
                    .await
                    .unwrap_or_default();
            let mut has_tube_geom = false;
            for tubi_geom in &tubi_geoms_info.geometries {
                if let TubeImplied(d) = tubi_geom {
                    bore = d.diameter;
                    has_tube_geom = true;
                    break;
                }
            }

            if !has_tube_geom {
                if let Ok(h_cat_att) = mgr.get_attr_from_localdb(h_cat_ref) {
                    let params = h_cat_att.get_f64_vec("PARA").unwrap_or_default();
                    if params.len() >= 2 {
                        bore = params[if is_hang { 0 } else { 1 }] as f32;
                    }
                };
            }
        }

        let tref = group_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let mut current_tubing = PdmsTubing {
            leave_refno: branch_refno,
            arrive_refno: tref,
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            desire_arrive_dir: Default::default(),
            bore,
        };


        // 整个 bran 就一个 tubi, 没有children的情况
        // 需要求解出 leave bore
        if children.len() == 0 {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
            {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                // current_tubing.finished = true;
                //需要检查href的方位
                current_tubing.desire_arrive_dir = -current_tubing.get_dir();
                //检查一下方向是否一致，不一致的，不显示，或者加标记位
                if current_tubing.is_dir_ok() {
                    if let Some(t) = current_tubing.get_transform() {
                        inst_tubi_map
                            .insert(branch_refno, EleGeosInfo {
                                refno: branch_refno,
                                cata_hash: Some(TUBI_GEO_HASH),
                                visible: true,
                                generic_type: mgr.get_generic_type(branch_refno),
                                aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                world_transform: t,
                                flow_pt_indexs: vec![],
                                geo_type: Default::default(),
                            });
                        // 将 tubi 数据保存到图数据库
                        let key = h_ref.hash_with_another_refno(tref);
                        tubi_aqls.entry(key).or_insert(TubiEdge {
                            _key: key.to_string(),
                            _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.leave_refno.to_url_refno()),
                            _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.arrive_refno.to_url_refno()),
                            start_pt: current_tubing.start_pt,
                            end_pt: current_tubing.end_pt,
                            att_type: group_att.get_type().to_string(),
                            extra_type: "".to_string(),
                            bore,
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
            //
            // shape_insts_data
            println!(
                "正在处理直段{}: {}",
                cur_type,
                refno.to_refno_string()
            );
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
                    if current_tubing.is_dir_ok(){
                        if let Some(t) = current_tubing.get_transform() {
                            // dbg!(current_tubing.leave_refno);
                            inst_tubi_map
                                .insert(current_tubing.leave_refno, EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(TUBI_GEO_HASH),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(current_tubing.leave_refno),
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                    geo_type: Default::default(),
                                });
                            let key = current_tubing.leave_refno.hash_with_another_refno(current_tubing.arrive_refno);
                            tubi_aqls.entry(key).or_insert(TubiEdge {
                                _key: key.to_string(),
                                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.leave_refno.to_url_refno()),
                                _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.arrive_refno.to_url_refno()),
                                start_pt: current_tubing.start_pt,
                                end_pt: current_tubing.end_pt,
                                att_type: ele.noun.clone(),
                                extra_type: "".to_string(),
                                bore: current_tubing.bore,
                                bran_name: bran_name.clone(),
                            });
                        }
                    }  else {
                        // dbg!(&axis_map);
                        // dbg!(axis_map[&arrive].pt);
                        // let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                        // dbg!(a_pos);
                        dbg!(&current_tubing);
                        println!("{} 的直段方向有问题", refno.to_refno_string());
                    }
                }
            }
            if axis_map.contains_key(&leave) {
                let dir = axis_map[&leave].dir;
                let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                let l_pos = world_trans.transform_point(axis_map[&leave].pt);
                let lstube = refno_lstube_map.get(&refno).map(|x| *x.value()).unwrap_or_default();
                let bore = lstube_bores_map.get(&lstube).map(|x| *x.value()).unwrap_or_default();
                current_tubing.bore = bore;
                current_tubing.start_pt = l_pos;
                current_tubing.desire_leave_dir = l_dir;
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
                            inst_tubi_map
                                .insert(current_tubing.leave_refno, EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(TUBI_GEO_HASH),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(current_tubing.leave_refno),
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                    geo_type: Default::default(),
                                });
                            let key = current_tubing.leave_refno.hash_with_another_refno(current_tubing.arrive_refno);
                            tubi_aqls.entry(key).or_insert(TubiEdge {
                                _key: key.to_string(),
                                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.leave_refno.to_url_refno()),
                                _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", current_tubing.arrive_refno.to_url_refno()),
                                start_pt: current_tubing.start_pt,
                                end_pt: current_tubing.end_pt,
                                att_type: ele.noun.clone(),
                                extra_type: "".to_string(),
                                bore: current_tubing.bore,
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
    dbg!(&tubi_result.len());
    if !tubi_result.is_empty() {
        let conn = mgr.get_arango_db().await?;
        let json = serde_json::to_value(tubi_result).unwrap_or_default();
        save_arangodb_doc(json, "tubi_edges", &conn, mgr.db_option.replace_dbs)
            .await?;
    }
    println!(
        "处理元件库几何体: {} 花费时间: {} ms",
        unique_cata_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

// 需要区分project，不同project的mesh，是不同的
pub async fn gen_geos_data(
    mut mgr: Arc<AiosDBManager>,
    db_option: DbOption,
) -> anyhow::Result<bool> {
    let time = Instant::now();
    let project = &db_option.project_name;
    let mdb = &db_option.mdb_name;
    let mut db_nos = db_option.manual_db_nums.clone().unwrap_or_default();

    // let s_refno = RefU64::from_two_nums(24381, 100675);
    // let children = mgr.get_children_from_localdb(s_refno);
    // dbg!(children);
    //
    // let s_refno = RefU64::from_two_nums(17496, 143555);
    // let att = mgr.get_attr_from_localdb(s_refno);
    // dbg!(att);
    // let plin_param = mgr.query_pline(s_refno, "OBOW").await?;
    // dbg!(plin_param);
    // let transform = mgr.get_world_transform(s_refno).await?.unwrap();
    // dbg!(transform);

    // let s_refno = RefU64::from_two_nums(17496, 161309);
    // let att = mgr.get_attr_from_localdb(s_refno);
    // dbg!(att);
    // // let plin_param = mgr.query_pline(s_refno, "OBOW").await?;
    // // dbg!(plin_param);
    // let transform = mgr.get_world_transform(s_refno).await?.unwrap();
    // dbg!(transform);

    // return Ok(true);

    if db_nos.is_empty() {
        let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
        let pool = AiosDBManager::get_db_pool(&url, project).await?;
        db_nos = query_db_nums_of_mdb(mdb, &db_option.module, &pool).await?;
        db_nos.sort();
        info!("当前mdb的所有dbnos: {:?}", db_nos);
    }
    // std::fs::create_dir_all("./assets/mesh").unwrap();
    // std::fs::create_dir_all("./assets/instance").unwrap();

    let adb = mgr.get_arango_db().await?;

    dbg!(&db_nos);
    let scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>> = Arc::new(RwLock::new(HashMap::new()));
    let replace_mesh = db_option.replace_mesh;

    for db_no in db_nos {
        println!("开始处理db: {db_no}");
        let d_types = &db_option.debug_refno_types;
        let not_debug = db_option.debug_refno_types.is_empty();
        let mut run_cache_cata = d_types.iter().any(|x| x == "CATA");
        let mut run_cache_loop = d_types.iter().any(|x| x == "LOOP");
        let mut run_cache_prim = d_types.iter().any(|x| x == "PRIM");

        let mut shape_insts_data = ShapeInstancesData::default();
        let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
        shape_insts_data.insert_geos_data(TUBI_GEO_HASH, EleInstGeosData {
            inst_key: TUBI_GEO_HASH,
            refno: Default::default(),
            insts: vec![EleInstGeo {
                geo_hash: TUBI_GEO_HASH,
                refno: Default::default(),
                geo_param: PdmsGeoParam::PrimSCylinder(SCylinder::default()),
                pts: vec![],
                aabb: Some(unit_cyli_aabb),
                transform: Default::default(),
                visible: true,
                is_tubi: true,
                geo_type: GeoBasicType::Pos,
            }],
            aabb: Some(unit_cyli_aabb),
            type_name: "TUBI".to_string(),
            ptset_map: Default::default(),
            reuse_unit: true,
        });
        let instance_mgr = Arc::new(RwLock::new(shape_insts_data));

        let instance_mgr_clone = instance_mgr.clone();

        let db_option_clone = db_option.clone();
        let mgr_clone = mgr.clone();
        let mgr_clone_new = mgr.clone();

        let target_dbnos = [db_no];
        let root_refnos = mgr.get_gen_model_root_refnos(&target_dbnos).await?;
        dbg!(&root_refnos);
        if root_refnos.is_empty() {
            println!("输入的调试参考号或者db号不正确");
            continue;
        }


        //元件库的模型计算
        //求出有多少个是一样的模型
        let target_cata_refnos = mgr.get_gen_model_target_refnos(GeoEnum::CATA_BRAN_AND_HANGER_REUSE, &target_dbnos, false).await?;
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
            // if children.is_empty() { continue; }
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            //求出元件对应的outside bore
            branch_refnos_map.insert(*refno, children);
        }
        // dbg!(&branch_refnos_map);

        let lstube_refnos = mgr.query_foreign_refnos(&bran_comp_eles,
                                                     &[&["LSRO", "LSTU"]], &["CATR"],
                                                     &[], 2).await?;
        // dbg!(&bran_comp_eles);
        // dbg!(&lstube_refnos);
        for c in 0..bran_comp_eles.len() {
            refno_lstube_map.insert(bran_comp_eles[c], lstube_refnos[c]);
        }
        let lstube_set = lstube_refnos.into_iter()
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
        let target_bran_reuse_cata_map = mgr.get_gen_model_map_by_cata_hash(GeoEnum::CATA_BRAN_AND_HANGER_REUSE, &target_dbnos, true, false).await?;
        let target_single_reuse_cata_map = mgr.get_gen_model_map_by_cata_hash(GeoEnum::CATA_SINGLE_REUSE, &target_dbnos, false, false).await?;
        let target_single_cata_map = mgr.get_gen_model_map_by_cata_hash(GeoEnum::CATA_WITHOUT_REUSE, &target_dbnos, false, false).await?;
        dbg!(&target_bran_reuse_cata_map.len());
        // dbg!(&target_bran_reuse_cata_map);
        // dbg!(target_single_reuse_cata_map.iter().map(|x| x.value().group_refnos.clone()).collect::<Vec<_>>());
        dbg!(target_single_reuse_cata_map.len());
        dbg!(&target_single_cata_map.len());

        let mut has_run_cata = false;
        if run_cache_cata {
            let mut handles = vec![];
            //bran，hanger下需要重用的模型
            if !target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty() {
                let scom_info_map_clone = scom_info_map.clone();
                let mgr_clone = mgr.clone();
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
                        &db_option_clone,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        Arc::new(refno_lstube_map),
                        Arc::new(lstube_bores_map),
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
                let db_option_clone = db_option.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
                        &db_option_clone,
                        Arc::new(target_single_reuse_cata_map),
                        Arc::new(Default::default()),
                        Arc::new(Default::default()),
                        Arc::new(Default::default()),
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
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let handle = tokio::spawn(async move {
                    gen_cata_geos(
                        mgr_clone,
                        instance_mgr_clone,
                        scom_info_map_clone,
                        &db_option_clone,
                        Arc::new(target_single_cata_map),
                        Arc::new(Default::default()),
                        Arc::new(Default::default()),
                        Arc::new(Default::default()),
                    )
                        .await
                        .unwrap();
                });
                has_run_cata = true;
                handles.push(handle);
            }

            futures::future::join_all(handles).await;
            if has_run_cata {
                let mesh_mgr = mgr.cached_mesh_mgr.read().await;
                let inst_data = instance_mgr.read().await;
                println!("当前db下的元件库生成统计：");
                dbg!(mesh_mgr.len());
                dbg!(inst_data.inst_info_map.len());
                // dbg!(&inst_data.inst_info_map);
                dbg!(inst_data.inst_tubi_map.len());
                save_instance_to_graph_db(&mgr, &inst_data).await?;
                save_mesh_to_local_db(&mgr, &mesh_mgr, replace_mesh).expect("Save mesh to local db failed.");
                save_mesh_to_arango_db(&mgr, &mesh_mgr, replace_mesh).await?;
            }
            // mgr.cached_mesh_mgr.write().await.clear();
            instance_mgr.write().await.clear();
        }

        let mut has_geom_refnos = vec![];
        for root_refno in root_refnos.clone() {
            let refnos = mgr.query_refnos_has_geos(root_refno).await?;
            has_geom_refnos.extend_from_slice(&refnos);
        }
        dbg!(has_geom_refnos.len());
        if !has_geom_refnos.is_empty() {
            let target_loop_refnos = mgr.get_gen_model_target_refnos(GeoEnum::LOOP, &target_dbnos, false).await?;
            println!("使用LOOP的数量: {}", target_loop_refnos.len());
            if run_cache_loop && !target_loop_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    gen_loop_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        &target_loop_refnos,
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            let target_prim_refnos = mgr.get_gen_model_target_refnos(GeoEnum::PRIM, &target_dbnos, false).await?;
            println!("使用基本体数量: {}", target_prim_refnos.len());
            if run_cache_prim && !target_prim_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    gen_prim_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        target_prim_refnos.as_slice(),
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            println!("开始处理负实体计算");
            //todo 优化负实体的计算, use monifold 库
            // dbg!(&root_refnos);
            let has_pos_neg_map = mgr.query_refnos_has_pos_neg_map(&root_refnos).await.unwrap_or_default();
            // dbg!(&has_pos_neg_map);
            dbg!(has_pos_neg_map.len());

            if db_option.apply_boolean_operation && !has_pos_neg_map.is_empty() {
                let now = Instant::now();
                let mut trans_map = DashMap::new();
                let mut mesh_result_map: Arc<DashMap<u64, PlantGeoData>> = Arc::new(DashMap::new());
                let mut inst_info_result_map = Arc::new(DashMap::new());
                let mut inst_geos_result_map = Arc::new(DashMap::new());
                {
                    let inst_data = Arc::new(instance_mgr.read().await);
                    let mesh_mgr = Arc::new(mgr.cached_mesh_mgr.read().await);
                    for comp_refno in has_pos_neg_map.keys().cloned() {
                        let trans = mgr.get_world_transform(comp_refno).await.unwrap_or_default().unwrap_or_default();
                        trans_map.insert(comp_refno, trans);
                    }
                    has_pos_neg_map.into_par_iter().for_each(|(comp_refno, (mut pos_refnos, neg_refnos))| {
                        println!("正在处理: {} 下的负实体", comp_refno);
                        let inst_data_clone = inst_data.clone();
                        let mut mesh_mgr_clone = mesh_mgr.clone();
                        let trans_map_clone = trans_map.clone();
                        let mut mesh_result_map_clone = mesh_result_map.clone();
                        let mut inst_info_result_map_clone = inst_info_result_map.clone();
                        let mut inst_geos_result_map_clone = inst_geos_result_map.clone();

                        let mut pos_meshes = vec![];
                        let mut neg_meshes = vec![];
                        // let mut w_aabb: Option<Aabb> = None;
                        //没有正实体的情况，直接跳过
                        if neg_refnos.is_empty() { return; }
                        pos_refnos.push(comp_refno);
                        let Some(w_trans) = trans_map.get(&comp_refno).map(|x| x.value().clone()) else {
                            return;
                        };
                        // dbg!(w_trans);
                        let mut total_refnos = pos_refnos.clone();
                        total_refnos.extend_from_slice(&neg_refnos);
                        let inverse_mat = w_trans.compute_matrix().inverse();

                        let Some(origin_comp_geos_info) = inst_data.get_info(&comp_refno) else {
                            return;
                        };

                        let mut neg_refnos = vec![];
                        for t_refno in total_refnos {
                            let Some(geos_info) = inst_data.get_info(&t_refno) else {
                                continue;
                            };
                            // dbg!(geos_info);
                            // if let Some(mut w_aabb) = w_aabb {
                            //     w_aabb.merge(&geos_info.aabb.unwrap());
                            // } else {
                            //     w_aabb = geos_info.aabb;
                            // }
                            // dbg!(t_refno);
                            let Some(inst_geos) = inst_data.get_inst_geos_data(geos_info) else {
                                continue;
                            };
                            // dbg!(t_refno);
                            for geo_inst in &inst_geos.insts {
                                let geo_refno = geo_inst.refno;
                                // dbg!(geo_refno);
                                let Some(mesh) = mesh_mgr_clone.get_mesh(geo_inst.geo_hash) else {
                                    continue;
                                };
                                let geo_mat = geos_info.world_transform;
                                let ele_mat = inverse_mat * geo_mat.compute_matrix();
                                let local_mat = ele_mat * geo_inst.transform.compute_matrix();
                                let csg_mesh = mesh.into_csg_mesh(&local_mat);
                                if pos_refnos.contains(&t_refno) {
                                    pos_meshes.push(csg_mesh)
                                } else {
                                    neg_meshes.push(csg_mesh);
                                    neg_refnos.push(t_refno);
                                }
                            }
                        }
                        let geo_hash = *comp_refno;
                        if pos_meshes.is_empty() { return; }
                        let mut final_mesh = pos_meshes.pop().unwrap();
                        // for pos_mesh in pos_meshes {
                        //     final_mesh = final_mesh + pos_mesh;
                        // }
                        for (i, neg_mesh) in neg_meshes.into_iter().enumerate() {
                            // let tmp_mesh = final_mesh.clone() - neg_mesh;
                            // if tmp_mesh.triangles.is_empty() {
                            //     println!("需要执行的负实体: {} 建模有问题", neg_refnos[i]);
                            //     break;
                            // }
                            final_mesh = final_mesh - neg_mesh;
                        }
                        let mut plant_geo_data = PlantGeoData::from(final_mesh);
                        plant_geo_data.geo_hash = geo_hash;
                        mesh_result_map_clone.insert(geo_hash, plant_geo_data);
                        let geom_inst = EleInstGeo {
                            geo_hash,
                            refno: comp_refno,
                            pts: vec![],
                            aabb: None,
                            transform: Transform::IDENTITY,
                            geo_param: PdmsGeoParam::CompoundShape,
                            visible: true,
                            is_tubi: false,
                            geo_type: GeoBasicType::Compound,
                        };


                        let mut comp_geos_info = EleGeosInfo {
                            refno: comp_refno,
                            visible: true,
                            generic_type: mgr.get_generic_type(comp_refno),
                            aabb: origin_comp_geos_info.aabb.clone(),
                            world_transform: w_trans,
                            cata_hash: None,
                            flow_pt_indexs: vec![],
                            geo_type: GeoBasicType::Compound,
                        };
                        // dbg!(&comp_geos_info);
                        inst_info_result_map_clone.insert(comp_refno, comp_geos_info);
                        let comp_type = mgr.get_refno_basic(comp_refno).unwrap().get_type().to_string();
                        inst_geos_result_map_clone.insert(*comp_refno, EleInstGeosData {
                            inst_key: *comp_refno,
                            refno: comp_refno,
                            insts: vec![geom_inst],
                            aabb: origin_comp_geos_info.aabb.clone(),
                            type_name: comp_type,
                            ptset_map: Default::default(),
                            reuse_unit: false,
                        });
                    });

                    println!("布尔运算实体耗时 {} ms", now.elapsed().as_millis());
                }

                {
                    let mut inst_data = instance_mgr.write().await;
                    dbg!(inst_geos_result_map.len());
                    let inst_geos_result_map_inner = Arc::try_unwrap(inst_geos_result_map).unwrap();
                    for (k, v) in inst_geos_result_map_inner {
                        inst_data.insert_geos_data(k, v);
                    }
                    let inst_info_result_map_inner = Arc::try_unwrap(inst_info_result_map).unwrap();
                    for (k, v) in inst_info_result_map_inner {
                        inst_data.insert_info(k, v);
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
        }else{
            println!("当前节点下面没有要继续生成的基本体几何节点");
        }

        {
            let inst_data = instance_mgr.read().await;
            println!("当前db下的基本体生成统计：");
            dbg!(inst_data.inst_geos_map.len());
            save_instance_to_graph_db(&mgr, &inst_data).await?;
        }

        {
            let mesh_mgr = mgr.cached_mesh_mgr.read().await;
            dbg!(mesh_mgr.len());
            save_mesh_to_arango_db(&mgr, &mesh_mgr, replace_mesh).await?;
            save_mesh_to_local_db(&mgr, &mesh_mgr, replace_mesh).expect("Save mesh to local db failed.");
        }

        println!("{db_no} 生成完毕。");
    }



    println!("生成所有模型时间: {}ms", time.elapsed().as_millis());
    Ok(true)
}

async fn process_csg_boolean_operations(has_geom_refno: RefU64, mgr: Arc<AiosDBManager>,
                                        instance_mgr: Arc<RwLock<ShapeInstancesData>>) -> anyhow::Result<bool> {
    let pos_neg_map = mgr.query_refnos_has_pos_neg_map(&[has_geom_refno]).await.unwrap_or_default();
    // dbg!(&pos_neg_map);
    let has_neg = !pos_neg_map.is_empty();
    // dbg!(has_neg);
    //如果有负实体，直接合在一起，不需要再拆分
    //有点太慢了，todo 改用manifold 库试试
    for (comp_refno, (pos_refnos, neg_refnos)) in pos_neg_map {
        // dbg!(comp_refno);
        println!("正在处理: {} 下的负实体", comp_refno);
        let mut pos_meshes = vec![];
        let mut neg_meshes = vec![];
        let mut w_aabb: Option<Aabb> = None;
        //没有正实体的情况，直接跳过
        if pos_refnos.is_empty() { continue; }
        let Ok(Some(w_trans)) = mgr.get_world_transform(comp_refno).await else {
            continue;
        };
        let mut total_refnos = pos_refnos.clone();
        total_refnos.extend_from_slice(&neg_refnos);
        // dbg!(&total_refnos);
        // dbg!(&pos_refnos);
        let inverse_mat = w_trans.compute_matrix().inverse();
        {
            let inst_data = instance_mgr.read().await;
            let mesh_mgr = mgr.cached_mesh_mgr.read().await;
            for t_refno in total_refnos {
                let Some(geos_info) = inst_data.get_info(&t_refno) else {
                    continue;
                };
                // dbg!(geos_info);
                if let Some(mut w_aabb) = w_aabb {
                    w_aabb.merge(&geos_info.aabb.unwrap());
                } else {
                    w_aabb = geos_info.aabb;
                }
                let Some(inst_geos) = inst_data.get_inst_geos(geos_info) else {
                    continue;
                };
                for geo_inst in inst_geos {
                    let geo_refno = geo_inst.refno;
                    // dbg!(geo_refno);
                    let Some(mesh) = mesh_mgr.get_mesh(geo_inst.geo_hash) else {
                        // dbg!(geo_inst);
                        continue;
                    };
                    let Ok(Some(geo_mat)) = mgr.get_world_transform(geo_refno).await else {
                        continue;
                    };
                    let ele_mat = inverse_mat * geo_mat.compute_matrix();
                    let local_mat = ele_mat * geo_inst.transform.compute_matrix();
                    let csg_mesh = mesh.into_csg_mesh(&local_mat);
                    if pos_refnos.contains(&t_refno) {
                        pos_meshes.push(csg_mesh)
                    } else {
                        neg_meshes.push(csg_mesh);
                    }
                }
            }
        }
        let geo_hash = *comp_refno;
        // let mut inst_data = instance_mgr.write().await;
        // let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
        if pos_meshes.is_empty() { return Ok(false); }
        let mut final_mesh = pos_meshes.pop().unwrap();
        for pos_mesh in pos_meshes {
            final_mesh = final_mesh + pos_mesh;
        }
        for neg_mesh in neg_meshes {
            final_mesh = final_mesh - neg_mesh;
        }
        // mesh_mgr.insert(geo_hash, final_mesh.into());
        let geom_inst = EleInstGeo {
            geo_hash,
            refno: comp_refno,
            pts: vec![],
            aabb: None,
            transform: Transform::IDENTITY,
            geo_param: PdmsGeoParam::CompoundShape,
            visible: true,
            is_tubi: false,
            geo_type: GeoBasicType::Compound,
        };


        let mut geos_info = EleGeosInfo {
            refno: comp_refno,
            visible: true,
            generic_type: mgr.get_generic_type(comp_refno),
            aabb: w_aabb,
            world_transform: w_trans,
            cata_hash: None,
            flow_pt_indexs: vec![],
            geo_type: Default::default(),
        };
        // dbg!(&geos_info);
        // inst_data.insert_info(comp_refno, geos_info);
        let comp_type = mgr.get_refno_basic(comp_refno).unwrap().get_type().to_string();
        // inst_data.insert_geos_data(*comp_refno, EleInstGeosData{
        //     inst_key: *comp_refno,
        //     refno: comp_refno,
        //     insts: vec![geom_inst],
        //     aabb: None,
        //     type_name: comp_type,
        //     ptset_map: Default::default(),
        //     flow_pt_indexs: vec![],
        // });
    }

    return Ok(true);
}

async fn process_occ_boolean_operations(has_geom_refno: RefU64, mgr: Arc<AiosDBManager>, instance_mgr: Arc<RwLock<ShapeInstancesData>>) -> anyhow::Result<bool> {
    // let pos_neg_map = mgr.query_refnos_has_pos_neg_map(has_geom_refno).await.unwrap_or_default();
    // // dbg!(&pos_neg_map);
    // let has_neg = !pos_neg_map.is_empty();
    // // dbg!(has_neg);
    // //如果有负实体，直接合在一起，不需要再拆分
    // //有点太慢了，todo 改用manifold 库试试
    // for (comp_refno, (pos_refnos, neg_refnos)) in pos_neg_map {
    //     dbg!(comp_refno);
    //     let mut pos_shapes = vec![];
    //     let mut neg_shapes = vec![];
    //     let mut w_aabb: Option<Aabb> = None;
    //     //没有正实体的情况，直接跳过
    //     if pos_refnos.is_empty() { continue; }
    //     let Ok(Some(w_trans)) = mgr.get_world_transform(comp_refno).await else {
    //         continue;
    //     };
    //     let mut total_refnos = pos_refnos.clone();
    //     total_refnos.extend_from_slice(&neg_refnos);
    //     // dbg!(&total_refnos);
    //     dbg!(&pos_refnos);
    //     let inverse_mat = w_trans.compute_matrix().inverse();
    //     {
    //         let inst_data = instance_mgr.read().await;
    //         let mesh_mgr = mgr.cached_mesh_mgr.read().await;
    //         let mut neg_need_offset = false;
    //         'outer: for t_refno in total_refnos {
    //             let Some(geos_info) = inst_data.get_info(&t_refno) else {
    //                 continue;
    //             };
    //             // dbg!(geos_info);
    //             if let Some(mut w_aabb) = w_aabb {
    //                 w_aabb.merge(&geos_info.aabb.unwrap());
    //             } else {
    //                 w_aabb = geos_info.aabb;
    //             }
    //             for geo_inst in &geos_info.geo_basics {
    //                 let geo_refno = geo_inst.refno;
    //                 // dbg!(geo_refno);
    //                 let Some(occ_shape) = mesh_mgr.get_occ_shape(geo_inst.geo_hash) else {
    //                     dbg!(geo_inst);
    //                     continue;
    //                 };
    //                 // dbg!("Get shape");
    //                 let Ok(Some(geo_mat)) = mgr.get_world_transform(geo_refno).await else {
    //                     continue;
    //                 };
    //                 let ele_mat = inverse_mat * geo_mat.compute_matrix();
    //
    //                 // dbg!(ele_mat.to_scale_rotation_translation());
    //                 let local_mat = ele_mat * geo_inst.transform.compute_matrix();
    //                 // dbg!(&local_mat);
    //                 //如果scale都是一样的，只需要用transform
    //                 let (s, r, t) = local_mat.to_scale_rotation_translation();
    //                 let is_scale_same = abs_diff_eq!(s.max_element(), s.min_element(), epsilon=0.01);
    //                 // dbg!(is_scale_same);
    //                 let shape = if is_scale_same {
    //                     occ_shape.transform(&local_mat.as_dmat4()).unwrap()
    //                 } else {
    //                     occ_shape.g_transform(&local_mat.as_dmat4()).unwrap()
    //                 };
    //
    //                 if pos_refnos.contains(&t_refno) {
    //                     // neg_need_offset = matches!(geo_inst.geo_param, PrimExtrusion(_));
    //                     // dbg!(neg_need_offset);
    //                     pos_shapes.push(shape)
    //                 } else {
    //                     // if geo_refno == RefU64::from_two_nums(24381, 35205) {
    //                     //     continue;
    //                     // }
    //                     dbg!(t_refno);
    //                     //说明，这里特殊处理一下，如果被切割的是 extrusion,，需要将负实体扩张一下，不然生成的不对
    //                     // if neg_need_offset {
    //                     //     let cut_shape = shape.offset(1.0).expect("Offset shape error.");
    //                     //     neg_shapes.push(cut_shape);
    //                     // } else {
    //                     neg_shapes.push(shape);
    //                     // }
    //                 }
    //             }
    //         }
    //     }
    //     let geo_hash = *comp_refno;
    //     let mut inst_data = instance_mgr.write().await;
    //     let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
    //     // dbg!(pos_shapes.len());
    //     // dbg!(neg_shapes.len());
    //     let mut final_shape = None;
    //     // if let Ok(mut pos_compound_shape) = OCCShape::fuse_shapes(&pos_shapes)
    //     //     && let Ok(neg_compound_shape) = OCCShape::fuse_shapes(&neg_shapes)
    //     // {
    //     //     println!("Cut by merged.");
    //     //     if let Ok(s) = pos_compound_shape.cut(&neg_compound_shape, 1.0) {
    //     //         final_shape = Some(s);
    //     //     }
    //     // }
    //     if final_shape.is_none() {
    //         if let Ok(mut pos_compound_shape) = OCCShape::fuse_shapes(&pos_shapes) {
    //             println!("Cut by merged failed, so by each one.");
    //             for neg_shape in &neg_shapes {
    //                 pos_compound_shape = pos_compound_shape.cut(neg_shape, 1.0).unwrap();
    //             }
    //             final_shape = Some(pos_compound_shape);
    //         }
    //     }
    //
    //     if let Some(s) = final_shape {
    //         let size = w_aabb.unwrap().bounding_sphere().radius as f64;
    //         dbg!(size);
    //         let mesh: PlantMesh = s.mesh(0.01 * size).unwrap().into();
    //         mesh_mgr.insert(geo_hash, mesh);
    //     } else {
    //         println!("Cut 失败.");
    //     }
    //
    //     let geom_inst = EleInstGeo {
    //         geo_hash,
    //         refno: comp_refno,
    //         pts: vec![],
    //         aabb: None,
    //         transform: Transform::IDENTITY,
    //         geo_param: PdmsGeoParam::CompoundShape,
    //         visible: true,
    //         is_tubi: false,
    //     };
    //
    //     let mut geos_info = EleGeosInfo {
    //         refno: comp_refno,
    //         geo_basics: vec![geom_inst],
    //         visible: true,
    //         generic_type: mgr.get_generic_type(comp_refno),
    //         aabb: w_aabb,
    //         world_transform: w_trans,
    //         ptset_map: default(),
    //         flow_pt_indexs: default(),
    //     };
    //     // dbg!(&geos_info);
    //     inst_data.insert(comp_refno, geos_info);
    // }
    //
    return Ok(true);
}
