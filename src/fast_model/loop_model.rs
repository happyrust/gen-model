use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use aios_core::pdms_types::{EleGeosInfo, EleInstGeo, EleInstGeosData, GeoBasicType, ShapeInstancesData};
use aios_core::RefU64;
use dashmap::DashMap;
use glam::Vec3;
use std::time::Instant;
use bevy_transform::components::Transform;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::{Extrusion, Revolution};
use std::mem::take;
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::shared;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use crate::consts::*;

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
                // if loop_refno.get_1() == 171403{
                //     continue;
                // }
                let Ok(Some(ce_pe)) = aios_core::get_pe(loop_refno).await else {
                    continue;
                };
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
                // println!(
                //     "正在处理loops类型的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                //     j,
                //     parent_refno.to_string(),
                //     processed_cnt.lock().await.to_owned()
                // );
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
                //处理相邻的情况，第一个loop是正实体，后面的为负实体
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
                    cata_refno: None,
                    ptset_map: Default::default(),
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
                    println!("LOOP 有问题：{} ", loop_refno.to_string());
                    continue;
                };
                let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                geos_info.visible = visible;
                if item_trans.is_nan() {
                    continue;
                }
                let tr: Transform = item_trans;
                let ele_aabb = shared::aabb_apply_transform(&geo_aabb, &tr);
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
                geos_info.aabb = Some(shared::aabb_apply_transform(&ele_aabb, &trans_origin));
                shape_insts_data.insert_info(parent_refno, geos_info.clone());
                shape_insts_data.insert_geos_data(
                    parent_refno.to_string(),
                    EleInstGeosData {
                        inst_key: parent_refno.to_string(),
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
