use std::collections::HashMap;
use std::default::default;
use std::io::Read;
use std::mem::take;
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
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use aios_core::tool::math_tool;
use anyhow::anyhow;
use bevy::log::error;
use bevy::prelude::Transform;
use dashmap::{DashMap, DashSet};
use futures::future::ok;
use glam::{Mat3, Vec3};
use nalgebra::Point3;
use opencascade::{DsShape, Shape};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Isometry, Vector};
use tokio::sync::{Mutex, RwLock};
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn::geo::create_profile_geos;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::save_arangodb_doc;
use crate::consts::AQL_PDMS_ELES_COLLECTION;

/// 生成基本体的几何数据
pub async fn cache_prim_geos(
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
                };
                let mut geo_insts = vec![];
                let mut geo_hash = None;
                let mut item_trans = Transform::IDENTITY;

                let attr = mgr.get_attr(refno).await.unwrap_or_default();
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
                if let Some(brep_obj) = attr.create_brep_shape(limit_size) {
                    if brep_obj.check_valid() {
                        item_trans = brep_obj.get_trans();
                        if item_trans.is_nan() { continue; }
                        geo_param = brep_obj
                            .convert_to_geo_param()
                            .unwrap_or(PdmsGeoParam::Unknown);
                        let r = cached_mesh_mgr.gen_pdms_mesh(brep_obj, replace_mesh);
                        geo_hash = Some(r);
                    }
                }
                let Some(geo_hash) = geo_hash else {
                    continue;
                };
                let visible = attr.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                let tr = &item_trans;
                let Some(mut geo_aabb) = cached_mesh_mgr.get_bbox(&geo_hash) else {
                    continue;
                };
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
                    });
                }
                *processed_cnt.lock().await -= 1;
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


pub async fn cache_loop_geos(
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
    for i in 0..batch_chunks_cnt as usize {
        let mgr = mgr.clone();
        let instance_mgr = instance_mgr.clone();
        let all_refnos = all_refnos.clone();
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
                let refno = all_refnos[j];
                println!(
                    "正在处理loops的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                    j,
                    refno.to_refno_string(),
                    processed_cnt.lock().await.to_owned()
                );
                let Ok(Some(trans_origin)) = mgr
                    .get_world_transform(refno)
                    .await else {
                    continue;
                };
                *processed_cnt.lock().await -= 1;
                let Some(refno_basic) = mgr.get_refno_basic(refno) else {
                    continue;
                };
                let parent_basic = mgr.get_owner_ref_basic(refno).unwrap();
                let target_type = parent_basic.get_type();
                let parent_refno = refno_basic.get_owner();
                let mut geos_info = EleGeosInfo {
                    refno,
                    cata_hash: None,
                    visible: true,
                    world_transform: trans_origin,
                    generic_type: mgr.get_generic_type(parent_refno),
                    aabb: None,
                    flow_pt_indexs: vec![],
                };
                let mut target_refno = parent_refno;
                let mut loop_verts: Vec<Vec3> = vec![];
                let mut fradius_vec: Vec<f32> = vec![];

                if let Ok(children_refs) = mgr.get_children_refs(refno).await {
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
                let mut parent_att = mgr.get_attr(parent_refno).await.unwrap_or_default();
                let mut geo_hash = None;
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
                                geo_hash =
                                    Some(cached_mesh_mgr.gen_pdms_mesh(revo, replace_mesh));
                            }
                        }
                    }
                    //todo 关于justline，可能需要jusline的信息才能判断中心点
                    "AEXTR" | "NXTR" | "EXTR" | "PANE" | "FLOOR" | "SCREED" | "GWALL" => {
                        let attr = mgr.get_attr(refno).await.unwrap_or_default();
                        target_refno = parent_refno;
                        let mut height = attr
                            .get_f32("HEIG")
                            .unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
                        let i: usize = 0;
                        //fix 1516 的情况  =24381/36952，当为DBOT的时候，会变成DISH
                        let sjus = attr.get_str("SJUS").unwrap_or_default();
                        //开始处理有偏移的情况
                        {
                            if attr.get_type() == "NXTR" {
                                if let Some(parent_inst) = shape_insts_data.get_inst_info(attr.get_owner().unwrap_or_default()) {
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
                            let r = cached_mesh_mgr.gen_pdms_mesh(extrusion, replace_mesh);
                            geo_hash = Some(r);
                            // }
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

                if let Some(geo_hash) = geo_hash {
                    let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                    geos_info.visible = visible;
                    if item_trans.is_nan() { continue; }
                    let tr: Transform = item_trans;
                    if let Some(mut aabb) = cached_mesh_mgr.get_bbox(&geo_hash) {
                        let ele_aabb = aabb_apply_transform(&aabb, &tr);
                        let geom_inst = EleInstGeo {
                            geo_hash,
                            refno,
                            pts: Default::default(),
                            aabb: Some(aabb),
                            transform: tr,
                            visible,
                            is_tubi: false,
                            geo_param,
                            geo_type: if parent_att.is_neg() { GeoBasicType::Neg } else { GeoBasicType::Pos },
                        };
                        // geo_insts.push(geom_inst);
                        geos_info.aabb = Some(aabb_apply_transform(&ele_aabb, &trans_origin));

                        shape_insts_data.insert_info(target_refno, geos_info.clone());
                        shape_insts_data.insert_info(refno, geos_info);
                        shape_insts_data.insert_geos_data(*refno, EleInstGeosData {
                            inst_key: *refno,
                            refno,
                            insts: vec![geom_inst],
                            aabb: Some(ele_aabb),
                            type_name: parent_att.get_type().to_string(),
                            ptset_map: Default::default(),
                        });
                    } else {
                        error!("LOOP 有问题：{} ", refno.to_refno_string());
                    }
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
    let desi_att = mgr.get_attr(design_refno).await?;
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
pub async fn cache_cata_geos(
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
    // dbg!(has_cata_cnt);
    if unique_cata_cnt == 0 { return Ok(true); }
    let batch_chunks_cnt = unique_cata_cnt / batch_size + 1;
    println!("使用元件库的unique模型总数：{unique_cata_cnt}, 分块数量: {batch_chunks_cnt}");
    let mut handles = vec![];
    // let all_refnos = Arc::new(cata_refnos.to_vec());
    let processed_cnt = Arc::new(Mutex::new(unique_cata_cnt));
    // let mut tubi_aqls = Arc::new(DashMap::new());
    let replace_mesh = db_option.replace_mesh;

    let all_unique_keys = Arc::new(target_cata_map.iter().map(|x| x.cata_hash).collect::<Vec<_>>());
    for i in 0..batch_chunks_cnt as usize {
        let mgr = mgr.clone();
        let instance_mgr = main_instance_mgr.clone();
        let all_unique_keys = all_unique_keys.clone();
        let processed_cnt = processed_cnt.clone();
        // let mut tubi_aqls_clone = tubi_aqls.clone();
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
                if replace_mesh || target_cata.exist_geo.is_none() {
                    //如果没有已有的，需要生成
                    let refno = target_cata.group_refnos[0];
                    println!(
                        "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                        j,
                        refno.to_refno_string(),
                        processed_cnt.lock().await.to_owned()
                    );
                    //在这里直接处理完所有需要处理的transform
                    let brep_shapes_map = CateBrepShapeMap::new();
                    let current_att = mgr.get_attr(refno).await.unwrap_or_default();
                    let mut refno_ptset_map = DashMap::new();
                    let cur_type = current_att.get_type();

                    let Ok(cat_refno) = gen_cata_single_geoms(
                        mgr.clone(),
                        refno,
                        &brep_shapes_map,
                        &refno_ptset_map,
                        &scom_info_map,
                    ).await else{
                        continue;
                    };
                    ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                    for (ele_refno, shapes) in brep_shapes_map {
                        let Ok(Some(o)) = mgr
                            .get_world_transform(ele_refno)
                            .await else {
                            continue;
                        };
                        let Ok(ele_att) = mgr.get_attr(ele_refno).await else {
                            continue;
                        };
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
                            flow_pt_indexs: vec![
                                ele_att.get_i32("ARRI").unwrap_or(-1),
                                ele_att.get_i32("LEAV").unwrap_or(-1),
                            ],
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
                            let trans = brep_shape.get_trans();
                            // dbg!(&brep_shape);
                            let geo_hash =
                                cached_mesh_mgr.gen_pdms_mesh(brep_shape.clone(), replace_mesh);
                            // dbg!(geo_hash);
                            let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash);
                            // dbg!(&bbox);
                            if bbox.is_none() {
                                continue;
                            }
                            let mut geo_aabb = bbox.unwrap();

                            // dbg!(&transform);
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
                            geos_info.aabb = Some(
                                a.transform_by(&Isometry {
                                    rotation: o.rotation.into(),
                                    translation: o.translation.into(),
                                }),
                            );
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
                    println!(
                        "正在处理同类元件库的模型当前参考号：{}",
                        ele_refno.to_refno_string(),
                    );
                    let Ok(Some(o)) = mgr
                        .get_world_transform(ele_refno)
                        .await else {
                        continue;
                    };

                    let flow = mgr.get_attr(ele_refno).await.unwrap_or_default();

                    let mut geos_info = EleGeosInfo {
                        refno: ele_refno,
                        cata_hash: Some(cata_hash),
                        visible: true,
                        generic_type: mgr.get_generic_type(ele_refno),
                        aabb: Some(aabb_apply_transform(target_geo_data.aabb.as_ref().unwrap(), &o)),
                        world_transform: o,
                        flow_pt_indexs: if !flow.contains_attr_name("ARRI") { vec![] } else{ vec![
                            flow.get_i32("ARRI").unwrap_or(-1),
                            flow.get_i32("LEAV").unwrap_or(-1),
                        ]},
                    };
                    //需要变换成世界坐标系下的aabb
                    // if let Some(a) = target_geo_data.aabb {
                    //     geos_info.aabb = Some(
                    //         a.transform_by(&Isometry {
                    //             rotation: o.rotation.into(),
                    //             translation: o.translation.into(),
                    //         }),
                    //     );
                    // }
                    let inst_key = geos_info.get_inst_key();
                    shape_insts_data.insert_info(ele_refno, geos_info);
                    shape_insts_data.insert_geos_data(inst_key, target_geo_data.clone());
                }

                *processed_cnt.lock().await -= 1;
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

    //先暂时维持和之前一样
    //还是把tubi 抽出来，创造一个tubi geos

    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    //直段需要插入一个单位的cylinder

    let mut inst_tubi_map = HashMap::new();
    //todo 重用直段的生成
    for b in branch_map.iter() {
        let shape_insts_data = main_instance_mgr.read().await;
        let branch_refno = *b.key();
        let children = b.value();
        let group_att = mgr.get_attr(branch_refno).await?;
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
        if let Ok(h_att) = mgr.get_attr(h_ref).await {
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
                let h_cat_att = mgr.get_attr(h_cat_ref).await?;
                let params = h_cat_att.get_f64_vec("PARA").unwrap_or_default();
                if params.len() >= 2 {
                    bore = params[if is_hang { 0 } else { 1 }] as f32;
                }
            }
            // dbg!(bore);
        }

        // let lstube = refno_lstube_map.get(&refno).map(|x| *x.value()).unwrap_or_default();
        // let bore = lstube_bores_map.get(&lstube).map(|x| *x.value()).unwrap_or_default();
        // dbg!(hbore);

        let mut current_tubing = PdmsTubing {
            refno: branch_refno,
            next_refno: Default::default(),
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            desire_arrive_dir: Default::default(),
            // _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", branch_refno.to_url_refno()),
            // _to: Default::default(),
            bore,
        };

        // 整个 bran 就一个 tubi, 没有children的情况
        // 需要求解出 leave bore
        if children.len() == 0 {
            // if !current_tubing.finished
            //     && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
            // {
            //     current_tubing.end_pt = bran_ttube_pt;
            //     current_tubing.finished = true;
            //     //需要检查href的方位
            //     current_tubing.desire_arrive_dir = -current_tubing.get_dir();
            //     //检查一下方向是否一致，不一致的，不显示，或者加标记位
            //     if current_tubing.is_dir_ok() {
            //         if let Some(t) = current_tubing.get_transform() {
            //             inst_tubi_map
            //                 .insert(branch_refno, EleGeosInfo {
            //                     refno: branch_refno,
            //                     cata_hash: Some(TUBI_GEO_HASH),
            //                     visible: true,
            //                     generic_type: mgr.get_generic_type(branch_refno),
            //                     aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
            //                     world_transform: t,
            //                 });
            //         }
            //     } else {
            //         error!("{} 的直段方向有问题", branch_refno.to_refno_string());
            //     }
            // }
            // // 将 tubi 数据保存到图数据库
            // let tref = group_att
            //     .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            //     .unwrap_or_default();
            // let key = h_ref.hash_with_another_refno(tref);
            // tubi_aqls.entry(key).or_insert(TubiEdge {
            //     _key: key.to_string(),
            //     _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", h_ref.to_url_refno()),
            //     _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", tref.to_url_refno()),
            //     start_pt: current_tubing.start_pt,
            //     end_pt: current_tubing.end_pt,
            //     att_type: group_att.get_type().to_string(),
            //     extra_type: "".to_string(),
            //     bore,
            //     bran_name: bran_name.clone(),
            // });
            // continue;
        }

        let last_child = children.last().unwrap().clone();
        //不包含atta的元件集合
        let mut bran_comp_vec = vec![];
        //第一遍完成后，然后生成tubing
        for ele in children {
            let refno = ele.refno;
            let cur_type = ele.noun.as_str();
            if cur_type == "ATTA" {
                continue;
            }
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
            if axis_map.contains_key(&arrive) {
                let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                let dir = axis_map[&arrive].dir;
                let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                if a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                    current_tubing.refno = refno;
                    current_tubing.end_pt = a_pos;
                    current_tubing.desire_arrive_dir = a_dir;
                    if current_tubing.is_dir_ok() {
                        if let Some(t) = current_tubing.get_transform() {
                            inst_tubi_map
                                .insert(refno, EleGeosInfo {
                                    refno,
                                    cata_hash: Some(TUBI_GEO_HASH),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(refno),
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                });
                        }
                    } else {
                        // dbg!(&axis_map);
                        // dbg!(axis_map[&arrive].pt);
                        // let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                        // dbg!(a_pos);
                        dbg!(&current_tubing);
                        println!("{} 的直段方向有问题", refno.to_refno_string());
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
            }
            //有隐含管段
            //最后一段的管道
            if refno == last_child.refno {
                if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                    //检查是否有一端是世界坐标原点
                    current_tubing.refno = refno;
                    current_tubing.end_pt = bran_ttube_pt;
                    //todo 需要取得连接到的，tref的点对应的arrive方向
                    current_tubing.desire_arrive_dir = -current_tubing.desire_leave_dir;

                    if current_tubing.is_dir_ok() {
                        let last_component_refno = *bran_comp_vec.last().unwrap();
                        if let Some(t) = current_tubing.get_transform() {
                            inst_tubi_map
                                .insert(last_component_refno, EleGeosInfo {
                                    refno: last_component_refno,
                                    cata_hash: Some(TUBI_GEO_HASH),
                                    visible: true,
                                    generic_type: mgr.get_generic_type(last_component_refno),
                                    aabb: Some(aabb_apply_transform(&unit_cyli_aabb, &t)),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
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

    // dbg!(time.el)

    {
        let mut main = main_instance_mgr.write().await;
        // let m = Arc::try_unwrap(local_inst_mgr).unwrap();
        // let m = local_inst_mgr.
        // let mut local_inst_mgr = m.into_inner();
        // let l = local_inst_mgr.read().await;
        // main.merge_ref(&l);
        for (k, v) in inst_tubi_map {
            main.insert_tubi(k, v);
        }
        // dbg!(shape_insts_data.inst_tubi_map.len());
    }


    println!("模型生成完毕,正在保存直段到图数据库");
    // let tubi_result = Arc::try_unwrap(tubi_aqls)
    //     .unwrap()
    //     .into_iter()
    //     .map(|x| x.1)
    //     .collect::<Vec<_>>();
    // // dbg!(&tubi_result.len());
    // if !tubi_result.is_empty() {
    //     let conn = mgr.get_arango_db().await?;
    //     let json = serde_json::to_value(tubi_result).unwrap_or_default();
    //     save_arangodb_doc(json, "tubi_edges", &conn, mgr.db_option.replace_dbs)
    //         .await?;
    // }
    println!(
        "处理元件库几何体: {} 花费时间: {} ms",
        unique_cata_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}