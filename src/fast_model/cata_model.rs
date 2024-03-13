use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use aios_core::pdms_types::*;
use dashmap::DashMap;
use aios_core::{gen_bytes_hash, HASH_PSEUDO_ATT_MAPS, NamedAttrMap, NamedAttrValue, RefU64, SUL_DB};
use aios_core::pe::SPdmsElement;
use glam::{DMat4, DVec3, Vec3};
use std::time::Instant;
use std::collections::HashMap;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use bevy_transform::components::Transform;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use nalgebra::Point3;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use std::mem::take;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use aios_core::prim_geo::{PdmsTubing, TubiEdge};
use aios_core::tool::math_tool::to_pdms_vec_str;
use aios_core::parsed_data::CateGeomsInfo;
use crate::cata::sctn::geo::create_profile_geos;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model;
use crate::fast_model::shared;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use crate::consts::*;
use aios_core::prim_geo::*;

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


///获取单个元件的模型数据
pub async fn gen_cata_single_geoms(
    mgr: Arc<AiosDBManager>,
    design_refno: RefU64,
    brep_shape_map: &CateBrepShapeMap,
    design_axis_map: &DashMap<RefU64, AIOSAxisMap>,
) -> anyhow::Result<bool> {
    let desi_att = aios_core::get_named_attmap(design_refno).await?;
    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        return Ok(false);
    }
    let geoms_info = mgr.resolve_desi_comp(design_refno, None).await?;
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
        return Ok(true);
    } else {
        let CateGeomsInfo {
            refno,
            geometries,
            n_geometries,
            axis_map,
        } = geoms_info;
        for geom in geometries {
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                // if cate_shape.refno == "13245_896722".into() {
                //     dbg!(&geom);
                // }
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
        design_axis_map.insert(design_refno, axis_map);
        return Ok(true);
    }
}

///计算对齐偏移值
#[inline]
pub fn cal_sjus_value(sjus: &str, height: f32) -> f32 {
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
    println!("使用元件库的模型总数：{unique_cata_cnt}, 分块数量: {batch_chunks_cnt}");
    let mut handles = vec![];
    let processed_cnt = Arc::new(Mutex::new(unique_cata_cnt));
    let mut tubi_edges = Arc::new(DashMap::new());
    let mut tubi_relates = vec![];
    let replace_mesh = mgr.db_option.replace_mesh;
    let tol_ratio = mgr.db_option.mesh_tol_ratio;
    let multi_threads = mgr.db_option.multi_threads;

    let all_unique_keys = Arc::new(
        target_cata_map
            .iter()
            .map(|x| x.cata_hash.clone())
            .collect::<Vec<_>>(),
    );
    // dbg!(&all_unique_keys.len());
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
                        // println!(
                        //     "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                        //     j,
                        //     ele_refno.to_string(),
                        //     processed_cnt.lock().await.to_owned()
                        // );
                        *processed_cnt.lock().await -= 1;
                        let Ok(Some(cata_refno)) = aios_core::get_cat_refno(ele_refno).await else {
                            // println!("{ele_refno} 的元件库引用为空，跳过");
                            continue;
                        };
                        //在这里直接处理完所有需要处理的transform
                        let brep_shapes_map = CateBrepShapeMap::new();
                        let desi_att = aios_core::get_named_attmap(ele_refno)
                            .await
                            .unwrap_or_default();
                        let mut design_axis_map = DashMap::new();
                        let cur_type = desi_att.get_type_str();

                        let r = gen_cata_single_geoms(
                            mgr_clone.clone(),
                            ele_refno,
                            &brep_shapes_map,
                            &design_axis_map,
                        )
                        .await;
                        match r {
                            Ok(_) => {}
                            Err(e) => {
                                // println!("生成元件库模型失败: {:?}", e);
                                continue;
                            }
                        };
                        #[cfg(debug_assertions)]
                        dbg!(brep_shapes_map.len());
                        {
                            // 将一些伪属性需要用到的值存下来，后面也要更新维护这些伪属性，避免重复计算
                            let mut lock = HASH_PSEUDO_ATT_MAPS.write().await;
                            let psudo_map = lock
                                .entry(cata_hash.clone())
                                .or_insert(NamedAttrMap::default());

                            if desi_att.contains_key("LEAV") {
                                let arrive = desi_att.get_i32("ARRI").unwrap_or_default();
                                let leave = desi_att.get_i32("LEAV").unwrap_or_default();
                                let axis_map = design_axis_map.get(&ele_refno).unwrap();
                                // dbg!(axis_map);
                                if axis_map.contains_key(&arrive) {
                                    let v = axis_map.get(&arrive).unwrap();
                                    psudo_map
                                        .insert("ARRWID".into(), NamedAttrValue::F32Type(v.pwidth));
                                    psudo_map.insert(
                                        "ARRHEI".into(),
                                        NamedAttrValue::F32Type(v.pheight),
                                    );
                                    psudo_map
                                        .insert("ABOR".into(), NamedAttrValue::F32Type(v.pbore));
                                }

                                if axis_map.contains_key(&leave) {
                                    let v = axis_map.get(&leave).unwrap();
                                    psudo_map
                                        .insert("LEAWID".into(), NamedAttrValue::F32Type(v.pwidth));
                                    psudo_map.insert(
                                        "LEAHEI".into(),
                                        NamedAttrValue::F32Type(v.pheight),
                                    );
                                    psudo_map
                                        .insert("LBOR".into(), NamedAttrValue::F32Type(v.pbore));
                                }
                            }
                            // dbg!(ele_refno);
                            // dbg!(&cata_hash);
                            // dbg!(&psudo_map);
                        }

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

                            let Ok(gmse_refno) = aios_core::query_single_by_paths(
                                cata_refno,
                                &["->GMRE", "->GSTR"],
                                &["refno"],
                            )
                            .await
                            .map(|x| x.get_refno_lossy().unwrap_or_default()) else {
                                continue;
                            };
                            #[cfg(debug_assertions)]
                            dbg!(gmse_refno);

                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            //反过来查询负实体，然后查询它的owner，来找到相邻的正实体
                            let pos_neg_map: HashMap<RefU64, Vec<RefU64>> = if gmse_refno.is_valid()
                            {
                                aios_core::query_refnos_has_pos_neg_map(&[gmse_refno], Some(true))
                                    .await
                                    .unwrap_or_default()
                            } else {
                                HashMap::new()
                            };
                            let mut neg_own_pos_map: HashMap<RefU64, RefU64> = pos_neg_map
                                .iter()
                                .map(|(k, negs)| negs.iter().map(|x| (*x, *k)))
                                .flatten()
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
                                cata_refno: Some(cata_refno),
                                ptset_map: Default::default(),
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
                                    // dbg!(&aabb);
                                    //稍微扩张一点
                                    if is_neg {
                                        let center: Vec3 = aabb.center().into();
                                        let mut center = center.as_dvec3();
                                        let t_mat = DMat4::from_translation(center);
                                        let mut s = 1.002;
                                        let s_mat = DMat4::from_scale(DVec3::splat(s));
                                        let inv_t_mat = DMat4::from_translation(-center);
                                        local_mat = local_mat * t_mat * s_mat * inv_t_mat;
                                    }
                                    let new_mesh = mesh.transform_by(&(local_mat));
                                    #[cfg(feature = "debug_obj_export")]
                                    {
                                        let _ = std::fs::create_dir_all("models");
                                        mesh.export_obj(
                                            false,
                                            &format!("models/{}.obj", refno.to_string()),
                                        )
                                        .expect("TODO: panic message");
                                    }
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
                                // dbg!(&transform);
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
                                    //todo 后面使用record link，方便能直接找到
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
                                    Some(shared::aabb_apply_transform(&a, &geos_info.world_transform));
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
                                    refno: ele_refno,
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
                                    refno: ele_refno,
                                    insts: geo_insts
                                        .iter()
                                        .filter(|x| x.geo_type == GeoBasicType::Pos)
                                        .cloned()
                                        .collect(),
                                    aabb: merged_cata_aabb,
                                    type_name: cur_type.to_string(),
                                    ptset_map: design_axis_map
                                        .remove(&ele_refno)
                                        .map(|x| x.1)
                                        .unwrap_or_default(),
                                };
                                //TODO: 后续只需要存在 info 里，不用存在insts里面
                                geos_info.ptset_map = origin.ptset_map.clone();
                                // dbg!(&geo_insts);
                                //需要判断是否ngmr是个只有V--的，没有实体的，如果没有实体，就不需要加入到insts里面
                                //要保留POS的部分
                                target_geo_data_option = Some(origin.clone());
                                //TODO: 只保留正实体的部分在insts里面，比如FITT的套管，挖空的部分存在另外的地方，type为-1
                                #[cfg(debug_assertions)]
                                dbg!(origin.insts.len());
                                if origin.insts.len() > 0 {
                                    shape_insts_data.insert_info(ele_refno, geos_info.clone());
                                    shape_insts_data
                                        .insert_geos_data(geos_info.get_inst_key(), origin.clone());
                                }

                                //在这里执行负实体的运算
                                let mut final_geo_insts = geo_insts;
                                let mut final_compounds_map = HashMap::new();
                                let mut total_manifolds = vec![];

                                for (&k, neg_vec) in &pos_neg_map {
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
                                            hash_two_str(&inst_key.to_string(), &k.to_string());
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
                                        refno: ele_refno,
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
                    let Some(target_geo_insts) = target_geo_data_option else {
                        continue;
                    };
                    if target_geo_insts.aabb.is_none() {
                        continue;
                    }
                    for ele_refno in target_cata.group_refnos.clone() {
                        if Some(ele_refno) == process_refno {
                            continue;
                        }
                        // println!(
                        //     "正在处理同类元件库的模型当前参考号：{}",
                        //     ele_refno.to_string(),
                        // );
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

                        flow_pt_indexs = vec![
                            attr.get_i32("ARRI").unwrap_or(-1),
                            attr.get_i32("LEAV").unwrap_or(-1),
                        ];
                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type: mgr_clone.get_generic_type(ele_refno).await,
                            aabb: Some(shared::aabb_apply_transform(
                                target_geo_insts.aabb.as_ref().unwrap(),
                                &origin_trans,
                            )),
                            world_transform: origin_trans,
                            flow_pt_indexs,
                            geo_type: Default::default(),
                            cata_refno: None,
                            ptset_map: target_geo_insts.ptset_map.clone(),
                        };
                        shape_insts_data.insert_info(ele_refno, geos_info.clone());
                        //如果有负实体，需要特殊处理
                        if target_geo_insts.has_cata_neg() {
                            let mut compound_geos_info = geos_info.clone();
                            compound_geos_info.update_to_compound(None);
                            //为了不覆盖，这里需要动一下
                            shape_insts_data.insert_compound_info(ele_refno, compound_geos_info);
                        }

                        //如果有ngmr负实体，需要特殊处理
                        if target_geo_insts.has_ngmr() {
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

    for bran in branch_map.iter() {
        let shape_insts_data = main_instance_mgr.read().await;
        let branch_refno = *bran.key();
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
        let mut h_tubi_size = fast_model::query_tubi_size(&mgr, branch_refno, tubi_cat_ref, is_hang).await?;
        // dbg!(&tubi_size);
        //todo 其实这里应该待定比较好
        let mut tubi_geo_hash = if matches!(h_tubi_size, TubiSize::BoxSize(_)) {
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
            tubi_size: h_tubi_size,
        };

        let bran_owner_type = aios_core::get_type_name(branch_att.get_owner())
            .await
            .unwrap_or_default();
        let is_hvac = bran_owner_type == "HVAC";
        // dbg!(is_hvac);
        // 需要求解出 leave bore
        if children.len() == 0 && !is_hvac {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                //需要检查href的方位
                current_tubing.desire_arrive_dir = -current_tubing.get_dir();
                //检查一下方向是否一致，不一致的，不显示，或者加标记位
                if current_tubing.is_dir_ok() {
                    if let Some(t) = current_tubing.get_transform() {
                        let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                        inst_tubi_map.insert(
                            branch_refno,
                            EleGeosInfo {
                                refno: branch_refno,
                                cata_hash: Some(tubi_geo_hash.to_string()),
                                visible: true,
                                generic_type: mgr.get_generic_type(branch_refno).await,
                                aabb: Some(aabb),
                                world_transform: t,
                                flow_pt_indexs: vec![],
                                geo_type: Default::default(),
                                cata_refno: None,
                                ptset_map: Default::default(),
                            },
                        );
                        tubi_relates.push(format!(
                            "relate pe:{branch_refno}->tubi_relate->inst_geo:⟨{tubi_geo_hash}⟩  \
                                    set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={}",
                            current_tubing.leave_refno,
                            current_tubing.arrive_refno,
                            gen_bytes_hash::<_, 64>(&aabb),
                            gen_bytes_hash::<_, 64>(&t),
                            serde_json::to_string(&h_tubi_size).unwrap_or_default(),
                        ));
                        // 将 tubi 数据保存到图数据库
                        let key = h_ref.hash_with_another_refno(tref);

                    }
                } else {
                    // println!("{} 的直段方向有问题", branch_refno.to_string());
                }
            }
            continue;
        }

        //不包含atta的元件集合
        let mut bran_comp_vec = vec![];
        //第一遍完成后，然后生成tubing
        let len = children.len();
        for (index, ele) in children.into_iter().enumerate() {
            let refno = ele.refno;
            // dbg!(refno);
            let cur_type = ele.noun.as_str();
            //can get the inst info
            if let Some(inst_info) = shape_insts_data.get_inst_info(refno)
                && let Some(inst_geos_data) = shape_insts_data.get_inst_geos_data(inst_info)
            {
                println!("正在处理直段{}: {}", cur_type, refno.to_string());
                let world_trans = inst_info.world_transform;
                let axis_map = &inst_geos_data.ptset_map;
                let arrive = inst_info.flow_pt_indexs[0];
                let leave = inst_info.flow_pt_indexs[1];
                //有隐含管段
                // dbg!(axis_map);
                bran_comp_vec.push(refno);
                current_tubing.arrive_refno = refno;
                //ATTA，如果设置成SPKBRK，产生直段，否则不产生直段
                let skip = (cur_type == "ATTA")
                    && !aios_core::get_named_attmap(refno)
                        .await?
                        .get_bool_or_default("SPKBRK");
                // dbg!(skip);
                if !skip && axis_map.contains_key(&arrive) {
                    let a_pos = world_trans.transform_point(axis_map[&arrive].pt);
                    let dir = axis_map[&arrive].dir;

                    // dbg!(quat_to_pdms_ori_xyz_str(&world_trans.rotation));
                    let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                    let actual_vec = a_pos - current_tubing.start_pt;
                    // dbg!(actual_vec);
                    let actual_dir = actual_vec.normalize_or_zero();
                    //判断actual_dir 和 a_dir 是否一致，一致的话说明有重叠
                    let same_dir = actual_dir.dot(a_dir) > 0.99;
                    if same_dir {
                        dbg!(to_pdms_vec_str(&actual_dir));
                        dbg!(to_pdms_vec_str(&a_dir));
                    }
                    if actual_vec.length() > TUBI_TOL && !same_dir {
                        current_tubing.end_pt = a_pos;
                        current_tubing.desire_arrive_dir = a_dir;
                        //TODO: 需要弄清楚风管开头的不需要加直段?
                        let is_hvac_start = is_hvac && (index == 0);
                        //风管开头这样的不需要处理
                        if !is_hvac_start {
                            if current_tubing.is_dir_ok() {
                                // dbg!(&current_tubing);
                                // 检测到有重叠的情况，就需要忽略
                                //如果 leave 的 还是 bran 的参考号，说明还是要用h_tubi_size
                                if current_tubing.leave_refno == branch_refno {
                                    println!("管道 bran 开头有个直段.");
                                    current_tubing.tubi_size = h_tubi_size;
                                } else {
                                    //如果不是，就需要重新计算
                                    let lstube_cat_ref = aios_core::query_single_by_paths(
                                        current_tubing.leave_refno,
                                        &["->LSTU->CATR"],
                                        &["refno"],
                                    )
                                    .await
                                    .map(|x| x.get_refno_lossy().unwrap_or_default())
                                    .unwrap_or_default();
                                    dbg!(&lstube_cat_ref);
                                    current_tubing.tubi_size = fast_model::query_tubi_size(
                                        &mgr,
                                        current_tubing.leave_refno,
                                        lstube_cat_ref,
                                        is_hang,
                                    )
                                    .await?;
                                }
                                #[cfg(debug_assertions)]
                                dbg!(&current_tubing.tubi_size);
                                tubi_geo_hash =
                                    if matches!(current_tubing.tubi_size, TubiSize::BoxSize(_)) {
                                        BOXI_GEO_HASH
                                    } else {
                                        TUBI_GEO_HASH
                                    };
                                if let Some(t) = current_tubing.get_transform() {
                                    let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                                    inst_tubi_map.insert(
                                        current_tubing.leave_refno,
                                        EleGeosInfo {
                                            refno: current_tubing.leave_refno,
                                            cata_hash: Some(tubi_geo_hash.to_string()),
                                            visible: true,
                                            generic_type: mgr
                                                .get_generic_type(current_tubing.leave_refno)
                                                .await,
                                            aabb: Some(aabb),
                                            world_transform: t,
                                            flow_pt_indexs: vec![],
                                            geo_type: GeoBasicType::Tubi,
                                            cata_refno: None,
                                            ptset_map: Default::default(),
                                        },
                                    );
                                    println!(
                                        "发现直段{}->{}, 方向: {}, 辅助方向: {}",
                                        current_tubing.leave_refno.to_slash_string(),
                                        current_tubing.arrive_refno.to_slash_string(),
                                        to_pdms_vec_str(&current_tubing.desire_leave_dir),
                                        to_pdms_vec_str(
                                            &current_tubing.leave_ref_dir.unwrap_or_default()
                                        ),
                                    );
                                    tubi_relates.push(
                                        format!(
                                            "relate pe:{branch_refno}->tubi_relate->inst_geo:⟨{tubi_geo_hash}⟩ \
                                            set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans= trans:⟨{}⟩, bore_size={}",
                                            current_tubing.leave_refno,
                                            current_tubing.arrive_refno,
                                            gen_bytes_hash::<_, 64>(&aabb),
                                            gen_bytes_hash::<_, 64>(&t),
                                            serde_json::to_string(&h_tubi_size).unwrap_or_default(),
                                        ));
                                    // dbg!(t);
                                    let key = current_tubing
                                        .leave_refno
                                        .hash_with_another_refno(current_tubing.arrive_refno);
                                    tubi_edges.entry(branch_refno).or_insert(Vec::new()).push(
                                        TubiEdge {
                                            _key: key.to_string(),
                                            _from: format!(
                                                "{AQL_PDMS_ELES_COLLECTION}/{}",
                                                current_tubing.leave_refno.to_string()
                                            ),
                                            _to: format!(
                                                "{AQL_PDMS_ELES_COLLECTION}/{}",
                                                current_tubing.arrive_refno.to_string()
                                            ),
                                            start_pt: current_tubing.start_pt,
                                            end_pt: current_tubing.end_pt,
                                            att_type: ele.noun.clone(),
                                            extra_type: "".to_string(),
                                            tubi_size: current_tubing.tubi_size,
                                            bran_name: bran_name.clone(),
                                        },
                                    );
                                }
                            } else {
                                #[cfg(feature = "debug")]
                                {
                                    dbg!(&current_tubing);
                                    dbg!(to_pdms_vec_str(&current_tubing.desire_arrive_dir));
                                    dbg!(to_pdms_vec_str(&current_tubing.desire_leave_dir));
                                }
                                println!("{} 的直段方向有问题", refno.to_string());
                            }
                        }
                    }
                }
                if axis_map.contains_key(&leave) {
                    let dir = axis_map[&leave].dir;
                    let ref_dir = axis_map[&leave].ref_dir;
                    // dbg!(ref_dir);
                    let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                    // let cond = if l_dir.cross(Vec3::Y).z >= 0.0 { 1.0 } else { 0.0 };
                    //todo 需要弄清楚为啥是Vec3::Z
                    let mut l_ref_dir = world_trans.transform_vec3(Vec3::Z).normalize_or_zero();
                    if l_ref_dir.dot(l_dir) >= 0.99 {
                        // let cond = if l_dir.cross(Vec3::Y).z >= 0.0 { 1.0 } else { -1.0 };
                        let cond = if l_dir.cross(ref_dir).z >= 0.0 {
                            1.0
                        } else {
                            -1.0
                        };
                        // dbg!(cond);
                        l_ref_dir = cond * world_trans.transform_vec3(ref_dir).normalize_or_zero();
                    }
                    // let l_ref_dir = ref_dir;
                    // dbg!(to_pdms_vec_xyz_str(&l_ref_dir));

                    if skip {
                        // current_tubing.desire_leave_dir = l_dir;
                        // current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                        //     Some(l_ref_dir)
                        // } else {
                        //     None
                        // };
                    } else {
                        let l_pos = world_trans.transform_point(axis_map[&leave].pt);
                        current_tubing.start_pt = l_pos;
                        current_tubing.desire_leave_dir = l_dir;
                        current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                            Some(l_ref_dir)
                        } else {
                            None
                        };
                        current_tubing.leave_refno = refno;
                    }
                }
            }
            //有隐含管段
            //最后一段的管道, 风管不需要这么处理？
            if index == len - 1 && !is_hvac {
                let last_dist = bran_ttube_pt.distance(current_tubing.start_pt);
                //TODO: 需要弄清楚，是否是风管的不需要考虑最后一段直段
                if last_dist > TUBI_TOL {
                    // dbg!(last_dist);
                    //检查是否有一端是世界坐标原点
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
                    //todo 需要取得连接到的，tref的点对应的arrive方向
                    current_tubing.desire_arrive_dir = -current_tubing.desire_leave_dir;
                    if current_tubing.is_dir_ok() {
                        // dbg!(&current_tubing);
                        if let Some(t) = current_tubing.get_transform() {
                            let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                            inst_tubi_map.insert(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: mgr
                                        .get_generic_type(current_tubing.leave_refno)
                                        .await,
                                    aabb: Some(aabb),
                                    world_transform: t,
                                    flow_pt_indexs: vec![],
                                    geo_type: GeoBasicType::Tubi,
                                    cata_refno: None,
                                    ptset_map: Default::default(),
                                },
                            );
                            tubi_relates.push(
                                format!(
                                    "relate pe:{branch_refno}->tubi_relate->inst_geo:⟨{tubi_geo_hash}⟩ set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩,bore_size={}",
                                    current_tubing.leave_refno,
                                    current_tubing.arrive_refno,
                                    gen_bytes_hash::<_, 64>(&aabb),
                                    gen_bytes_hash::<_, 64>(&t),
                                    serde_json::to_string(&h_tubi_size).unwrap_or_default(),
                                )
                            );
                            let key = current_tubing
                                .leave_refno
                                .hash_with_another_refno(current_tubing.arrive_refno);
                            tubi_edges
                                .entry(branch_refno)
                                .or_insert(Vec::new())
                                .push(TubiEdge {
                                    _key: key.to_string(),
                                    _from: format!(
                                        "{AQL_PDMS_ELES_COLLECTION}/{}",
                                        current_tubing.leave_refno.to_string()
                                    ),
                                    _to: format!(
                                        "{AQL_PDMS_ELES_COLLECTION}/{}",
                                        current_tubing.arrive_refno.to_string()
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
                        println!("{} 的直段方向有问题", refno.to_string());
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

    //todo 暂时放在这里
    //将tubi的关系建立起来，直接指向inst_geo, 由bran出发，每段tubi都指向对应的inst_geo，
    //需要在edge上加上对应的参考号，如果是branch，需要加上branch的参考号
    //使用relate创建这个关系，先把relate语句保存到tubi_relate_vec
    if !tubi_relates.is_empty() {
        // dbg!(tubi_relates.join(";"));
        SUL_DB.query(tubi_relates.join(";")).await.unwrap();
    }
    println!(
        "处理元件库几何体: {} 花费时间: {} ms",
        unique_cata_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}
