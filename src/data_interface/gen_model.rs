use std::collections::HashMap;
use std::default::default;
use std::mem::take;
use std::sync::Arc;
use aios_core::options::DbOption;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::parsed_data::geo_params_data::CateGeoParam::TubeImplied;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdge};
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use anyhow::anyhow;
use bevy::log::error;
use bevy::prelude::Transform;
use cached::instant::Instant;
use dashmap::{DashMap, DashSet};
use glam::Vec3;
use nalgebra::Point3;
use opencascade::DsShape;
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
                let trans_origin = mgr
                    .get_world_transform(refno)
                    .await
                    .unwrap_or_default()
                    .unwrap_or_default();
                let mut geos_info = EleGeosInfo {
                    refno,
                    visible: true,
                    generic_type: mgr.get_generic_type(refno),
                    aabb: None,
                    world_transform: trans_origin,
                    cata_hash: None,
                };

                // let mut geo_edges = vec![];
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
                        // dbg!(&brep_obj);
                        item_trans = brep_obj.get_trans();
                        geo_param = brep_obj
                            .convert_to_geo_param()
                            .unwrap_or(PdmsGeoParam::Unknown);
                        let r = cached_mesh_mgr.gen_pdms_mesh(brep_obj, replace_mesh);
                        geo_hash = Some(r);
                    }
                }
                if let Some(geo_hash) = geo_hash {
                    let visible = attr.is_visible_by_level(None).unwrap_or(true);
                    geos_info.visible = visible;
                    let tr = &item_trans;
                    let Some(mut aabb) = cached_mesh_mgr.get_bbox(&geo_hash) else {
                        continue;
                    };
                    aabb = aabb.scaled(&Vector::new(tr.scale.x, tr.scale.y, tr.scale.z));
                    let ele_aabb = aabb.transform_by(&Isometry {
                        rotation: tr.rotation.into(),
                        translation: tr.translation.into(),
                    });
                    let inst_geo = EleInstGeo {
                        geo_hash,
                        refno,
                        pts: Default::default(),
                        aabb: Some(aabb),
                        transform: *tr,
                        geo_param,
                        visible,
                        is_tubi: false,
                        geo_type: if attr.is_neg() { GeoBasicType::Neg } else { GeoBasicType::Pos },
                    };
                    // geo_edges.push(GeoEdge{
                    //     refno,
                    //     geo_type: if attr.is_neg() { GeoBasicType::Neg } else { GeoBasicType::Pos },
                    //     geo_hash,
                    //     cata_hash: None,
                    // });
                    geo_insts.push(inst_geo);
                    // shape_insts_data.insert_geo(inst_geo.geo_hash, inst_geo);
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
                            aabb: None,
                            ptset_map: Default::default(),
                            flow_pt_indexs: vec![],
                        });
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
                let trans_origin = mgr
                    .get_world_transform(refno)
                    .await
                    .unwrap_or_default()
                    .unwrap_or_default();
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
                };
                let mut geo_insts = vec![];
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
                    let tr: Transform = item_trans;
                    if let Some(mut aabb) = cached_mesh_mgr.get_bbox(&geo_hash) {
                        aabb = aabb.scaled(&Vector::new(tr.scale.x, tr.scale.y, tr.scale.z));
                        let ele_aabb = aabb.transform_by(&Isometry {
                            rotation: tr.rotation.into(),
                            translation: tr.translation.into(),
                        });
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
                        geo_insts.push(geom_inst);
                        geos_info.aabb = Some(
                            ele_aabb.transform_by(&Isometry {
                                rotation: trans_origin.rotation.into(),
                                translation: trans_origin.translation.into(),
                            }),
                        );
                    } else {
                        error!("LOOP 有问题：{} ", refno.to_refno_string());
                    }
                }
                //todo 插入两个是为了都能找到PLOO对应的构件
                // dbg!(&geo_insts);
                if !geo_insts.is_empty() {
                    // inst_map.insert(target_refno, geos_info.clone());
                    // inst_map.insert(refno, geos_info);
                    shape_insts_data.insert_info(target_refno, geos_info.clone());
                    shape_insts_data.insert_info(refno, geos_info);
                    shape_insts_data.insert_geos_data(*refno, EleInstGeosData {
                        inst_key: *refno,
                        refno,
                        insts: geo_insts,
                        aabb: None,
                        ptset_map: Default::default(),
                        flow_pt_indexs: vec![],
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
) -> anyhow::Result<bool> {
    let cur_ele = mgr.get_refno_basic(design_refno).ok_or(anyhow!("Element不存在"))?;
    let type_name = cur_ele.get_type();
    let owner = mgr.get_owner_ref_basic(design_refno);
    if owner.is_none() {
        return Ok(false);
    }
    let desi_att = mgr.get_attr(design_refno).await?;
    let geoms = resolve_desi_comp(design_refno, None, Some(mgr.as_ref()), scom_info_map)
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
            &geoms,
            &brep_shape_map,
            mgr.as_ref(),
        ).await?;
    } else {
        let CateGeomsInfo {
            geometries,
            axis_map,
        } = geoms;
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
    }
    Ok(true)
}

async fn gen_cata_auto_tubi_geoms(
    mgr: Arc<AiosDBManager>,
    branch_refno: RefU64,
    group_att: &AttrMap,
    brep_shape_map: &CateBrepShapeMap,
    refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>,
    tubi_aqls: &mut Arc<DashMap<u64, TubiEdge>>,
    scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
) -> anyhow::Result<bool> {
    let group_transform = mgr
        .get_world_transform(branch_refno)
        .await?
        .unwrap_or_default();
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
    let hconnect = group_att.get_as_string("HCON").unwrap_or_default();
    let bran_name = group_att.get_name().0.to_string();
    let mut has_tubi = true;
    let mut bore = 0.0f32;
    let mut href_type = "".to_string();
    //头节点的处理
    if let Ok(h_att) = mgr.get_attr(h_ref).await {
        href_type = h_att.get_type().to_string();
        let h_cat_ref = h_att.get_foreign_refno("CATR").unwrap_or_default();
        let tubi_geoms_info =
            resolve_desi_comp(branch_refno, Some(h_cat_ref), Some(mgr.as_ref()), scom_info_map)
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
    }
    let mut current_tubing = PdmsTubing {
        refno: branch_refno,
        start_pt: htube_pt,
        end_pt: Vec3::ZERO,
        desire_leave_dir: hdir,
        desire_arrive_dir: Default::default(),
        _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", h_ref.to_url_refno()),
        _to: Default::default(),
        bore,
        finished: false,
    };
    let children = mgr
        .get_children_refs(branch_refno)
        .await
        .unwrap_or_default();

    // 整个 bran 就一个 tubi
    if children.len() == 0 {
        if !current_tubing.finished
            && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
        {
            current_tubing.end_pt = bran_ttube_pt;
            current_tubing.finished = true;
            //需要检查href的方位
            current_tubing.desire_arrive_dir = -current_tubing.get_dir();
            //检查一下方向是否一致，不一致的，不显示，或者加标记位
            if current_tubing.is_dir_ok() {
                let shape = current_tubing.convert_to_shape();
                brep_shape_map
                    .entry(branch_refno)
                    .or_insert(Vec::new())
                    .push(shape);
            } else {
                error!("{} 的直段方向有问题", branch_refno.to_refno_string());
            }
        }
        // 将 tubi 数据保存到图数据库
        let tref = group_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let key = h_ref.hash_with_another_refno(tref);
        tubi_aqls.entry(key).or_insert(TubiEdge {
            _key: key.to_string(),
            _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", h_ref.to_url_refno()),
            _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", tref.to_url_refno()),
            start_pt: current_tubing.start_pt,
            end_pt: current_tubing.end_pt,
            att_type: group_att.get_type().to_string(),
            extra_type: "".to_string(),
            bore,
            bran_name: bran_name.to_string(),
        });
        return Ok(true);
    }
    // 将 bran 保存到 tubi_edges 中
    // 保存bran 和 第一个子节点的连接关系
    if let Some(first_refno) = children.0.first() {
        let first_attr = mgr.get_attr(*first_refno).await?;
        let geoms = resolve_desi_comp(*first_refno, None,
                                      Some(mgr.as_ref()), &scom_info_map).await;
        dbg!(geoms.is_ok());
        if let Ok(geoms) = geoms {
            dbg!(geoms.geometries.len());
            if let Some(arrive) = first_attr.get_i32("ARRI") {
                if geoms.axis_map.contains_key(&arrive) {
                    let first_world_trans = mgr
                        .get_world_transform(*first_refno)
                        .await?
                        .unwrap_or_default();
                    let a_pos = first_world_trans.transform_point(geoms.axis_map[&arrive].pt);
                    let key = branch_refno.hash_with_another_refno(*first_refno);
                    let att_type = first_attr.get_type();
                    let mut extra_type = "".to_string();
                    if att_type == "ATTA" {
                        let attype = first_attr.get_str("ATTY").unwrap_or("");
                        extra_type = attype.to_string();
                    }
                    tubi_aqls.entry(key).or_insert(TubiEdge {
                        _key: key.to_string(),
                        _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", branch_refno.to_url_refno()),
                        _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", first_refno.to_url_refno()),
                        start_pt: htube_pt,
                        end_pt: a_pos,
                        att_type: att_type.to_string(),
                        extra_type,
                        bore,
                        bran_name: bran_name.to_string(),
                    });
                }
            }
        }
    }
    // 保存元件之间的距离
    for (idx, refno) in children.clone().into_iter().enumerate() {
        let mut edge = TubiEdge::default();
        edge._from = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        if idx >= children.len() - 1 {
            break;
        }
        let to_refno = children[idx + 1];
        let key = refno.hash_with_another_refno(to_refno);
        edge._key = key.to_string();
        edge._to = format!("{AQL_PDMS_ELES_COLLECTION}/{}", to_refno.to_url_refno());
        edge.bran_name = bran_name.to_string();

        let Ok(attr) = mgr.get_attr(refno).await else {
            continue;
        };
        let Ok(to_attr) = mgr.get_attr(to_refno).await else {
            continue;
        };
        let att_type = to_attr.get_type();
        edge.att_type = att_type.to_string();
        // 单独存 atta 的 attype
        if att_type == "ATTA" {
            let attype = to_attr.get_str("ATTY").unwrap_or("");
            edge.extra_type = attype.to_string();
        }

        let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();

        let Ok(mut geoms) = resolve_desi_comp(refno, None, Some(mgr.as_ref()), scom_info_map).await else {
            continue;
        };
        let Ok(mut to_geoms) = resolve_desi_comp(to_refno, None, Some(mgr.as_ref()), scom_info_map).await else {
            continue;
        };
        let to_world_trans = mgr.get_world_transform(to_refno).await?.unwrap_or_default();
        if let Some(arrive) = to_attr.get_i32("ARRI") {
            if to_geoms.axis_map.contains_key(&arrive) {
                let p = to_geoms.axis_map[&arrive].pt;
                let a_pos = to_world_trans.transform_point(p);
                edge.end_pt = a_pos;
            } else {
                //need debug
                // dbg!(&to_refno);
                // dbg!(&arrive);
            }
        }
        if let Some(lstube) = attr.get_foreign_refno(if is_hang { "LSRO" } else { "LSTU" }) {
            if let Ok(lstube_att) = mgr.get_attr(lstube).await {
                let lstube_cat_refno =
                    lstube_att.get_foreign_refno("CATR").unwrap_or_default();
                let tubi_geoms_info = resolve_desi_comp(
                    refno,
                    Some(lstube_cat_refno),
                    Some(mgr.as_ref()),
                    scom_info_map,
                )
                    .await
                    .unwrap_or_default();
                let mut has_tube_geom = false;
                for tubi_geom in &tubi_geoms_info.geometries {
                    if let TubeImplied(d) = tubi_geom {
                        edge.bore = d.diameter;
                        has_tube_geom = true;
                        break;
                    }
                }
                if !has_tube_geom {
                    let lstube_cat_att = mgr.get_attr(lstube_cat_refno).await?;
                    let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                    if params.len() >= 2 {
                        edge.bore = params[if is_hang { 0 } else { 1 }] as f32;
                    }
                }
            }
        }
        if let Some(leave) = attr.get_i32("LEAV") {
            if geoms.axis_map.contains_key(&leave) {
                let p = geoms.axis_map[&leave].pt;
                let l_pos = world_trans.transform_point(p);
                edge.start_pt = l_pos;
            }
        }
        if !edge._key.is_empty() {
            // bran children 之间的关系
            tubi_aqls.entry(key).or_insert(edge);
        }
    }
    // 保存最后一个元件
    if let Some(last_refno) = children.0.last() {
        let last_attr = mgr.get_attr(*last_refno).await?;
        let last_geoms = resolve_desi_comp(*last_refno, None, Some(mgr.as_ref()), scom_info_map).await;
        if let Ok(last_geoms) = last_geoms {
            let last_world_trans = mgr
                .get_world_transform(*last_refno)
                .await?
                .unwrap_or_default();
            if let Some(leave) = last_attr.get_i32("LEAV") {
                if last_geoms.axis_map.contains_key(&leave) {
                    let tref = group_att.get_foreign_refno("TREF").unwrap_or(RefU64(0));
                    let tref_attr = mgr.get_attr(tref).await?;
                    let p = last_geoms.axis_map[&leave].pt;
                    let l_pos = last_world_trans.transform_point(p);
                    let key = last_refno.hash_with_another_refno(tref);
                    let att_type = tref_attr.get_type();
                    let mut extra_type = "".to_string();
                    if att_type == "ATTA" {
                        let attype = tref_attr.get_str("ATTY").unwrap_or("");
                        extra_type = attype.to_string();
                    }
                    tubi_aqls.entry(key).or_insert(TubiEdge {
                        _key: key.to_string(),
                        _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", last_refno.to_url_refno()),
                        _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", tref.to_url_refno()),
                        start_pt: l_pos,
                        end_pt: bran_ttube_pt,
                        att_type: att_type.to_string(),
                        extra_type,
                        bore,
                        bran_name: bran_name.to_string(),
                    });
                }
            }
        }
    }

    let last_child = children.last().unwrap().clone();
    //不包含atta的元件集合
    let mut bran_comp_vec = vec![];
    //第一遍完成后，然后生成tubing
    for refno in children {
        let Ok(attr) = mgr.get_attr(refno).await else {
            continue;
        };
        println!(
            "正在处理元件{}: {}",
            attr.get_type(),
            refno.to_refno_string()
        );
        let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();
        let mut geoms = resolve_desi_comp(refno, None, Some(mgr.as_ref()), scom_info_map).await;
        if geoms.is_err() {
            error!("{:?}", geoms.err().unwrap());
            continue;
        }
        let mut geoms = geoms.unwrap();
        //有隐含管段
        if has_tubi && (!attr.is_type("ATTA") && !attr.is_type("WELD")) {
            bran_comp_vec.push(attr.get_refno().unwrap());
            if let Some(arrive) = attr.get_i32("ARRI") {
                if geoms.axis_map.contains_key(&arrive) {
                    let a_pos = world_trans.transform_point(geoms.axis_map[&arrive].pt);
                    let dir = geoms.axis_map[&arrive].dir;
                    let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                    let arrive_refno = geoms.axis_map[&arrive].refno;
                    if !current_tubing.finished
                        && a_pos.distance(current_tubing.start_pt) > TUBI_TOL
                    {
                        current_tubing.refno = refno;
                        current_tubing.end_pt = a_pos;
                        current_tubing.desire_arrive_dir = a_dir;
                        current_tubing.finished = true;
                        if current_tubing.is_dir_ok() {
                            let brep_shape = current_tubing.convert_to_shape();
                            brep_shape_map
                                .entry(refno)
                                .or_insert(Vec::new())
                                .push(brep_shape);
                        } else {
                            error!("{} 的直段方向有问题", refno.to_refno_string());
                        }
                    }
                }
            }
            if let Some(lstube) = attr.get_foreign_refno(if is_hang { "LSRO" } else { "LSTU" })
            {
                if let Ok(lstube_att) = mgr.get_attr(lstube).await {
                    let lstube_cat_refno =
                        lstube_att.get_foreign_refno("CATR").unwrap_or_default();
                    //todo check how to get the bore value
                    let tubi_geoms_info = resolve_desi_comp(
                        refno,
                        Some(lstube_cat_refno),
                        Some(mgr.as_ref()),
                        scom_info_map,
                    )
                        .await
                        .unwrap_or_default();
                    let mut has_tube_geom = false;
                    for tubi_geom in &tubi_geoms_info.geometries {
                        if let TubeImplied(d) = tubi_geom {
                            current_tubing.bore = d.diameter;
                            has_tube_geom = true;
                            break;
                        }
                    }
                    if !has_tube_geom {
                        let lstube_cat_att = mgr.get_attr(lstube_cat_refno).await?;
                        let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                        if params.len() >= 2 {
                            current_tubing.bore = params[if is_hang { 0 } else { 1 }] as f32;
                        }
                    }
                }
            }
            if let Some(leave) = attr.get_i32("LEAV") {
                if geoms.axis_map.contains_key(&leave) {
                    let dir = geoms.axis_map[&leave].dir;
                    let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                    let l_pos = world_trans.transform_point(geoms.axis_map[&leave].pt);
                    current_tubing.start_pt = l_pos;
                    current_tubing.desire_leave_dir = l_dir;
                    current_tubing.finished = false;
                }
            }
        }
        //管件的生成
        let CateGeomsInfo {
            geometries,
            axis_map,
        } = geoms;
        for (i, geom) in geometries.into_iter().enumerate() {
            // dbg!((i, &geom));
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                brep_shape_map
                    .entry(refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
            }
        }
        refno_ptset_map.insert(refno, axis_map);
        //有隐含管段
        if has_tubi {
            //最后一段的管道
            if refno == last_child {
                if !current_tubing.finished
                    && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
                {
                    //检查是否有一端是世界坐标原点
                    current_tubing.refno = refno;
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.finished = true;
                    //todo 需要取得连接到的，tref的点对应的arrive方向
                    current_tubing.desire_arrive_dir = -current_tubing.desire_leave_dir;

                    if current_tubing.is_dir_ok()
                    {
                        // dbg!("Last tube");
                        let shape = current_tubing.convert_to_shape();
                        let last_component_refno = *bran_comp_vec.last().unwrap();
                        brep_shape_map
                            .entry(last_component_refno)
                            .or_insert(Vec::new())
                            .push(shape);
                    } else {
                        dbg!(current_tubing.desire_arrive_dir);
                        error!("{} 的直段方向有问题", refno.to_refno_string());
                    }
                }
            }
        }
    }
    Ok(true)
}


/// 生成元件库的branch型几何体
pub async fn cache_cata_geos(
    mgr: Arc<AiosDBManager>,
    instance_mgr: Arc<RwLock<ShapeInstancesData>>,
    scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>>,
    db_option: &DbOption,
    target_cata_map: Arc<DashMap<u64, CataHashRefnoKV>>,
    //将branch的情况也放在这里处理
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

    //只需要处理参数不一样的一些元件
    //将不重复的构件先生成


    for i in 0..batch_chunks_cnt as usize {
        let mgr = mgr.clone();
        let instance_mgr = instance_mgr.clone();
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
                let cata_hash = all_unique_keys[j];
                let target_cata = target_cata_map.get(&cata_hash).unwrap();
                let mut cached_mesh_mgr = mgr.cached_mesh_mgr.write().await;
                let mut shape_insts_data = instance_mgr.write().await;
                let mut target_geo_data = None;
                if target_cata.exist_geo.is_none()  {
                    //如果没有已有的，需要生成
                    // target_refno = Some(target_cata.group_refnos[0]);
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

                    gen_cata_single_geoms(
                        mgr.clone(),
                        refno,
                        &brep_shapes_map,
                        &refno_ptset_map,
                        &scom_info_map,
                    )
                        .await
                        .unwrap_or_default();
                    ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                    for (ele_refno, shapes) in brep_shapes_map {
                        let o = mgr
                            .get_world_transform(ele_refno)
                            .await
                            .unwrap_or_default()
                            .unwrap_or_default();
                        let ele_att = mgr.get_attr(ele_refno).await.unwrap_or_default();
                        let Ok(Some(gmse_refno)) = mgr.query_foreign_refno(ele_refno,
                                                                           &[&["SPRE", "CATR"]], &["GMRE", "GSTR"],
                                                                           &[]).await else {
                            continue;
                        };
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

                        };

                        // let mut shapes_map = HashMap::new();
                        let mut geo_insts = vec![];
                        // let mut ele_aabb: Option<Aabb> = None;
                        let mut cata_aabb: Option<Aabb> = None;
                        // let mut tubi_aabb: Option<Aabb> = None;
                        let mut has_tubi = false;
                        //将负实体和正实体统计出来
                        let mut merged_cata_aabb: Option<Aabb> = None;
                        // dbg!(shapes.len());
                        // dbg!(ele_refno);
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
                            // if is_tubi {
                            //     has_tubi = true;
                            //     if let Some(mut tubi_aabb) = tubi_aabb {
                            //         tubi_aabb.merge(&transformed_aabb);
                            //     } else {
                            //         tubi_aabb = Some(transformed_aabb);
                            //     }
                            // } else {
                            if let Some(mut cata_aabb) = cata_aabb {
                                cata_aabb.merge(&transformed_aabb);
                            } else {
                                cata_aabb = Some(transformed_aabb);
                            }
                            // }

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

                            // if pos_refnos.contains(&refno) || neg_refnos.contains(&refno) {
                            //     if let Some(o) = cached_mesh_mgr.get_occ_shape(geo_hash) {
                            //         shapes_map.insert(refno, o.g_transform(&transform.compute_matrix().as_dmat4()).unwrap());
                            //     } else {
                            //         dbg!(&brep_shape);
                            //         dbg!(refno);
                            //     }
                            // } else {  //不属于分组里面的，不需要理会
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
                        // }

                        //有负实体的是一个整体，需要合并处理
                        // if has_neg {
                        //     for (comp_ref, (pos, negs)) in pos_neg_map {
                        //         let mut final_shape = None;
                        //         for r in &pos {
                        //             let Some(shape) = shapes_map.get(r).cloned() else {
                        //                 dbg!(r);
                        //                 continue;
                        //             };
                        //             if final_shape.is_none() {
                        //                 final_shape = Some(shape);
                        //             } else {
                        //                 final_shape = final_shape.map(|x| x.fuse(&shape, 1.0).expect("occ shape Fuse 出错"));
                        //             }
                        //         }
                        //         for r in &negs {
                        //             let Some(shape) = shapes_map.get(r).cloned() else {
                        //                 dbg!(r);
                        //                 continue;
                        //             };
                        //             if final_shape.is_none() {
                        //                 final_shape = Some(shape);
                        //             } else {
                        //                 dbg!(r);
                        //                 final_shape = final_shape.map(|x| x.cut(&shape, 1.0).expect("occ shape cut 出错"));
                        //             }
                        //         }
                        //
                        //         let geo_hash = *comp_ref;
                        //         if let Some(s) = final_shape {
                        //             let size = w_aabb.unwrap().bounding_sphere().radius as f64;
                        //             dbg!(size);
                        //             let mesh: PlantMesh = s.mesh(0.008 * size).unwrap().into();
                        //             cached_mesh_mgr.meshes.insert(geo_hash, mesh);
                        //         }
                        //
                        //         let geom_inst = EleGeoInstance {
                        //             geo_hash,
                        //             refno: comp_ref,
                        //             pts: vec![],
                        //             aabb: w_aabb,
                        //             transform: Transform::IDENTITY,
                        //             geo_param: PdmsGeoParam::CompoundShape,
                        //             visible: true,
                        //             is_tubi: false,
                        //             is_neg: false,
                        //         };
                        //         // dbg!(&geom_inst);
                        //         // dbg!(cached_mesh_mgr.meshes.keys());
                        //         geo_insts.push(geom_inst);
                        //     }
                        // }
                        //

                        //需要变换成世界坐标系下的aabb
                        if let Some(a) = merged_cata_aabb {
                            geos_info.aabb = Some(
                                a.transform_by(&Isometry {
                                    rotation: o.rotation.into(),
                                    translation: o.translation.into(),
                                }),
                            );
                        }


                        //todo 暂时不合并直段的包围盒
                        if has_tubi {
                            //geos_info.aabb.as_mut().unwrap().merge(&tubi_aabb);
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
                                refno: ele_refno,
                                insts: geo_insts,
                                aabb: merged_cata_aabb,
                                ptset_map: refno_ptset_map
                                    .remove(&ele_refno)
                                    .map(|x| x.1)
                                    .unwrap_or_default(),
                                flow_pt_indexs: vec![
                                    ele_att.get_i32("ARRI").unwrap_or(-1),
                                    ele_att.get_i32("LEAV").unwrap_or(-1),
                                ],
                            };
                            target_geo_data = Some(d.clone());
                            shape_insts_data.insert_geos_data(inst_key, d);
                        }
                        //只有一个，现在不采用branch的方式去生成了
                        break;
                    }
                }else{
                    target_geo_data = target_cata.exist_geo.clone();
                }

                //排除一些特殊情况
                let Some(target_geo_data) = target_geo_data else{
                    continue;
                };
                if target_geo_data.aabb.is_none() { continue; }

                //如果已经有了，需要生成transform和bbox那些
                for ele_refno in target_cata.group_refnos.clone() {
                    println!(
                        "正在处理同类元件库的模型当前参考号：{}",
                        ele_refno.to_refno_string(),
                    );
                    let o = mgr
                        .get_world_transform(ele_refno)
                        .await
                        .unwrap_or_default()
                        .unwrap_or_default();
                    let mut geos_info = EleGeosInfo {
                        refno: ele_refno,
                        cata_hash: Some(cata_hash),
                        visible: true,
                        generic_type: mgr.get_generic_type(ele_refno),
                        aabb: None,
                        world_transform: o,
                    };
                    //需要变换成世界坐标系下的aabb
                    if let Some(a) = target_geo_data.aabb {
                        geos_info.aabb = Some(
                            a.transform_by(&Isometry {
                                rotation: o.rotation.into(),
                                translation: o.translation.into(),
                            }),
                        );
                    }
                    shape_insts_data.insert_info(ele_refno, geos_info);
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
    println!("模型生成完毕,正在保存管道到图数据库");
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