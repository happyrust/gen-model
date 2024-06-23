use crate::consts::*;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::PlantAxisMap;
use crate::fast_model;
use crate::fast_model::{get_generic_type, resolve_desi_comp, shared};
use aios_core::consts::NGMR_OWN_TYPES;
use aios_core::geometry::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use aios_core::prim_geo::category::{convert_to_brep_shapes, CateBrepShape};
use aios_core::prim_geo::*;
use aios_core::prim_geo::{PdmsTubing, TubiEdge};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::math_tool::to_pdms_vec_str;
use aios_core::{
    gen_bytes_hash, NamedAttrMap, NamedAttrValue, RefU64, HASH_PSEUDO_ATT_MAPS, SUL_DB,
};
use bevy_transform::components::Transform;
use dashmap::DashMap;
use glam::{DMat4, DVec3, Vec3};
use nalgebra::Point3;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use parry3d::bounding_volume::*;
use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;
use aios_core::prim_geo::profile::create_profile_geos;
use tokio::sync::{Mutex, RwLock};

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
    design_refno: RefU64,
    brep_shape_map: &CateBrepShapeMap,
    design_axis_map: &DashMap<RefU64, PlantAxisMap>,
) -> anyhow::Result<bool> {
    let desi_att = aios_core::get_named_attmap(design_refno).await?;
    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        return Ok(false);
    }
    let geoms_info = resolve_desi_comp(design_refno, None).await?;
    if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" || type_name == "WALL"
    {
        create_profile_geos(design_refno, &geoms_info, &brep_shape_map).await?;
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
/// 动态修改tubi，还是要单独出来, 还是直接去修改整个bran？
/// 先暂时整个重新生成？
pub async fn gen_cata_geos(
    db_option: Arc<DbOption>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<DashMap<RefU64, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefU64, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    let batch_size = db_option.gen_model_batch_size;
    let t = Instant::now();
    let mut handles = vec![];
    let mut tubi_relates = vec![];
    let gen_mesh = db_option.gen_mesh;
    // let multi_threads = db_option.multi_threads;
    let mut local_al_map = Arc::new(DashMap::new());
    let is_bran = branch_map.len() > 0;
    // let processed_cnt = Arc::new(Mutex::new(target_cata_map.len()));
    let all_unique_keys = Arc::new(
        target_cata_map
            .iter()
            .map(|x| x.cata_hash.clone())
            .collect::<Vec<_>>(),
    );
    let unique_cata_cnt = all_unique_keys.len();
    let mut batch_chunks_cnt = 16;
    let mut batch_size = all_unique_keys.len() / batch_chunks_cnt + 1;
    //如果只有一个元件，就不分块了
    if batch_size == 1 {
        batch_chunks_cnt = all_unique_keys.len();
    }
    println!("使用元件库的模型总数：{unique_cata_cnt}, 分块数量: {batch_chunks_cnt}");
    if !all_unique_keys.is_empty() {
        for i in 0..batch_chunks_cnt {
            let all_unique_keys = all_unique_keys.clone();
            let target_cata_map = target_cata_map.clone();
            let sjus_map_clone = sjus_map_arc.clone();
            let local_al_map_clone = local_al_map.clone();
            let sender = sender.clone();

            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > unique_cata_cnt {
                    end_idx = unique_cata_cnt;
                }
                println!("当前范围: {start_idx} ~ {end_idx}");
                let mut shape_insts_data = ShapeInstancesData::default();
                if is_bran {
                    shape_insts_data.fill_basic_shapes();
                }
                for j in start_idx..end_idx {
                    let cata_hash = all_unique_keys[j].clone();
                    if cata_hash == "0" {
                        continue;
                    }
                    let target_cata = target_cata_map.get(&cata_hash).unwrap();
                    let mut process_refno = None;
                    let mut ptset_map = None;
                    //如果inst_info 已经存在了，可以直接跳过生成，直接指向过去就可以了
                    if gen_mesh || !target_cata.exist_inst {
                        //如果没有已有的，需要生成
                        let ele_refno = target_cata.group_refnos[0];
                        process_refno = Some(ele_refno);
                        let Ok(Some(cata_refno)) = aios_core::get_cat_refno(ele_refno).await else {
                            #[cfg(feature = "debug_model")]
                            println!("{ele_refno} 的元件库引用为空，跳过");
                            continue;
                        };
                        #[cfg(feature = "debug_model")]
                        println!("开始生成元件库模型: {ele_refno}, 元件库参考号: {cata_refno}");
                        //在这里直接处理完所有需要处理的transform
                        let brep_shapes_map = CateBrepShapeMap::new();
                        let desi_att = aios_core::get_named_attmap(ele_refno)
                            .await
                            .unwrap_or_default();
                        let mut design_axis_map = DashMap::new();

                        let cur_type = desi_att.get_type_str();
                        // #[cfg(debug_assertions)]
                        // dbg!(ele_refno);
                        let r =
                            gen_cata_single_geoms(ele_refno, &brep_shapes_map, &design_axis_map)
                                .await;
                        match r {
                            Ok(_) => {}
                            Err(e) => {
                                println!("{ele_refno} 生成元件库模型失败: {:?}", e);
                                continue;
                            }
                        };
                        // #[cfg(debug_assertions)]
                        // dbg!(brep_shapes_map.len());
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
                        }

                        ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                        for (ele_refno, shapes) in brep_shapes_map {
                            let Ok(Some(mut world_transform)) =
                                aios_core::get_world_transform(ele_refno).await
                                else {
                                    continue;
                                };
                            let Ok(ele_att) = aios_core::get_named_attmap(ele_refno).await else {
                                continue;
                            };

                            if let Some(sjus) = ele_att.get_str("SJUS") {
                                let parent = ele_att.get_owner();
                                if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                    let height = sjus_adjust.value().1;
                                    let off_z = cal_sjus_value(sjus, height);
                                    let parent_trans = aios_core::get_world_transform(parent)
                                        .await
                                        .unwrap_or_default()
                                        .unwrap_or_default();

                                    world_transform.translation.z = parent_trans.translation.z;
                                    world_transform.translation = world_transform.translation
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
                            // #[cfg(debug_assertions)]
                            // dbg!((ele_refno, gmse_refno));

                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            //反过来查询负实体，然后查询它的owner，来找到相邻的正实体
                            let mut pos_neg_map: HashMap<RefU64, Vec<RefU64>> = if gmse_refno
                                .is_valid()
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
                            let cur_ptset_map = design_axis_map
                                .remove(&ele_refno)
                                .map(|x| x.1)
                                .unwrap_or_default();
                            // dbg!(ele_att.get_e3d_version());
                            let mut geos_info = EleGeosInfo {
                                refno: ele_refno,
                                version: ele_att.get_e3d_version(),
                                cata_hash: Some(cata_hash.clone()),
                                visible: true,
                                generic_type: get_generic_type(ele_refno).await.unwrap_or_default(),
                                aabb: None,
                                world_transform,
                                cata_refno: Some(cata_refno),
                                ptset_map: cur_ptset_map.clone(),
                                is_solid: true,
                                ..Default::default()
                            };

                            if ele_att.contains_key("ARRI") && !cur_ptset_map.is_empty() {
                                let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                                let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                                if let Some(a) = cur_ptset_map.values().find(|x| x.number == arrive)
                                    && let Some(l) =
                                    cur_ptset_map.values().find(|x| x.number == leave)
                                {
                                    local_al_map_clone.insert(ele_refno, [a.clone(), l.clone()]);
                                }
                                ptset_map = Some(cur_ptset_map);
                            };

                            let mut geo_insts = vec![];
                            let mut visible_set = HashSet::new();
                            for s in &shapes {
                                if s.visible {
                                    visible_set.insert(s.refno);
                                }
                            }
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
                                if !visible {
                                    continue;
                                }
                                let mut shape_trans = brep_shape.get_trans();
                                let is_neg = neg_own_pos_map.contains_key(&refno);
                                let geo_hash = brep_shape.hash_unit_mesh_params();
                                let rot = transform.rotation;
                                let translation =
                                    transform.translation + transform.rotation * shape_trans.translation;
                                let scale = shape_trans.scale;
                                let transform = Transform {
                                    translation,
                                    rotation: rot,
                                    scale,
                                };
                                // dbg!(&transform);
                                if transform.is_nan() {
                                    continue;
                                }
                                //如果不可见直接跳过
                                let mut cata_neg_refnos =
                                    pos_neg_map.remove(&refno).unwrap_or_default();
                                // dbg!(&cata_neg_refnos);
                                cata_neg_refnos.retain(|x| visible_set.contains(x));
                                // dbg!(&cata_neg_refnos);
                                if !cata_neg_refnos.is_empty() {
                                    geos_info.has_cata_neg = true;
                                }
                                let geom_inst = EleInstGeo {
                                    geo_hash,
                                    refno,
                                    pts,
                                    aabb: None,
                                    transform,
                                    geo_param: brep_shape
                                        .convert_to_geo_param()
                                        .unwrap_or(PdmsGeoParam::Unknown),
                                    visible,
                                    is_tubi,
                                    geo_type: if is_ngmr {
                                        GeoBasicType::CataCrossNeg
                                    } else if is_neg {
                                        GeoBasicType::CataNeg
                                    } else if !cata_neg_refnos.is_empty() {
                                        GeoBasicType::Compound
                                    } else {
                                        GeoBasicType::Pos
                                    },

                                    cata_neg_refnos,
                                };
                                if is_ngmr {
                                    //获得ngmr的关系
                                    if let Ok(target_owners) =
                                        query_ngmr_owner(ele_refno, refno).await {
                                        shape_insts_data.insert_ngmr(ele_refno, target_owners);
                                    }
                                }
                                geo_insts.push(geom_inst);
                            }
                            {
                                let mut inst_key = geos_info.get_inst_key();
                                geos_info.is_solid = geo_insts.iter().any(|x| x.geo_type == GeoBasicType::Pos
                                    || x.geo_type == GeoBasicType::Compound);
                                let mut geos_data = EleInstGeosData {
                                    inst_key,
                                    refno: ele_refno,
                                    insts: geo_insts,
                                    aabb: None,
                                    type_name: cur_type.to_string(),
                                    ..Default::default()
                                };
                                // #[cfg(debug_assertions)]
                                // dbg!(geos_data.insts.len());
                                if geos_data.insts.len() > 0 {
                                    shape_insts_data.insert_info(ele_refno, geos_info.clone());
                                    shape_insts_data
                                        .insert_geos_data(geos_info.get_inst_key(), geos_data);
                                }
                            }
                            break;
                        }
                    }
                    for ele_refno in target_cata.group_refnos.clone() {
                        if Some(ele_refno) == process_refno {
                            continue;
                        }
                        let cur_ptset_map = ptset_map
                            .as_ref()
                            .or(target_cata.ptset.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        let Ok(Some(mut origin_trans)) =
                            aios_core::get_world_transform(ele_refno).await
                            else {
                                continue;
                            };

                        let ele_att = aios_core::get_named_attmap(ele_refno)
                            .await
                            .unwrap_or_default();
                        if let Some(sjus) = ele_att.get_str("SJUS") {
                            let parent = ele_att.get_owner();
                            if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                let height = sjus_adjust.value().1;
                                let off_z = cal_sjus_value(sjus, height);
                                origin_trans.translation += sjus_adjust.value().0
                                    + origin_trans.rotation * Vec3::new(0.0, 0.0, off_z);
                                // println!("Offset by sjus {}", origin_trans.translation);
                            }
                        }

                        if ele_att.contains_key("ARRI") && !cur_ptset_map.is_empty() {
                            let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                            let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                            if let Some(a) = cur_ptset_map.values().find(|x| x.number == arrive)
                                && let Some(l) = cur_ptset_map.values().find(|x| x.number == leave)
                            {
                                local_al_map_clone.insert(ele_refno, [a.clone(), l.clone()]);
                            }
                        };
                        let geos_info = EleGeosInfo {
                            refno: ele_refno,
                            version: ele_att.get_e3d_version(),
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type: get_generic_type(ele_refno).await.unwrap_or_default(),
                            world_transform: origin_trans,
                            ptset_map: cur_ptset_map,
                            is_solid: true,  //TODO 这里是不是需要取查一下？
                            ..Default::default()
                        };
                        shape_insts_data.insert_info(ele_refno, geos_info);
                    }
                }

                sender
                    .send(shape_insts_data)
                    .expect("send cata shape_insts_data failed.");
            });
            handles.push(handle);
        }
    }
    // dbg!(handles.len());
    futures::future::join_all(take(&mut handles)).await;

    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    //直段需要插入一个单位的cylinder

    let mut tubi_shape_insts_data = ShapeInstancesData::default();
    for bran in branch_map.iter() {
        let branch_refno = *bran.key();
        let Ok(children) = aios_core::get_children_pes(branch_refno).await else {
            continue;
        };
        let Ok(branch_att) = aios_core::get_named_attmap(branch_refno).await else {
            continue;
        };
        //可能只有branch 元素需要做一遍求解
        let Ok(Some(branch_transform)) = aios_core::get_world_transform(branch_refno).await else {
            continue;
        };
        let htube_pt = branch_transform.transform_point(branch_att.get_vec3("HPOS").unwrap());
        let hdir = branch_transform
            .transform_vec3(branch_att.get_vec3("HDIR").unwrap())
            .normalize_or_zero();
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());
        // dbg!(bran_ttube_pt);

        let is_hang = branch_att.get_type_str() == "HANG";
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();

        let tubi_att = aios_core::get_named_attmap(h_ref).await.unwrap_or_default();
        let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        let mut h_tubi_size =
            fast_model::query_tubi_size(branch_refno, tubi_cat_ref, is_hang).await?;
        //todo 其实这里应该待定比较好
        let mut tubi_geo_hash = if matches!(h_tubi_size, TubiSize::BoxSize(_)) {
            BOXI_GEO_HASH
        } else {
            TUBI_GEO_HASH
        };

        let tref = branch_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let tdir = branch_transform
            .transform_vec3(branch_att.get_vec3("TDIR").unwrap())
            .normalize_or_zero();
        let mut current_tubing = PdmsTubing {
            leave_refno: branch_refno,
            arrive_refno: tref,
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            leave_ref_dir: None,
            desire_arrive_dir: Default::default(),
            tubi_size: h_tubi_size,
            index: 0,
        };

        let bran_owner_type = aios_core::get_type_name(branch_att.get_owner())
            .await
            .unwrap_or_default();
        let is_hvac = bran_owner_type == "HVAC";
        // 需要求解出 leave bore
        if children.len() == 0 && !is_hvac {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                //需要检查href的方位
                current_tubing.desire_arrive_dir = tdir;
                let dist = current_tubing.end_pt.distance(current_tubing.start_pt);
                //检查一下方向是否一致，不一致的，不显示，或者加标记位
                if dist > TUBI_TOL && current_tubing.is_dir_ok() {
                    if let Some(t) = current_tubing.get_transform() {
                        let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                        tubi_shape_insts_data.insert_tubi(
                            branch_refno,
                            EleGeosInfo {
                                refno: branch_refno,
                                version: branch_att.get_e3d_version(),
                                cata_hash: Some(tubi_geo_hash.to_string()),
                                visible: true,
                                generic_type: get_generic_type(branch_refno)
                                    .await
                                    .unwrap_or_default(),
                                aabb: Some(aabb),
                                world_transform: t,
                                flow_pt_indexs: vec![],
                                cata_refno: None,
                                is_solid: true,
                                ..Default::default()
                            },
                        );
                        #[cfg(feature = "debug_model")]
                        println!(
                            "发现直段{}->{}, 方向: {}, 辅助方向: {}, 距离: {:.3}",
                            current_tubing.leave_refno.to_slash_string(),
                            current_tubing.arrive_refno.to_slash_string(),
                            to_pdms_vec_str(&current_tubing.desire_leave_dir),
                            to_pdms_vec_str(
                                &current_tubing.leave_ref_dir.unwrap_or_default()
                            ),
                            dist
                        );
                        tubi_relates.push(format!(
                                "relate pe:{}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                                set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                branch_refno,
                                branch_refno.to_pe_key(),
                                current_tubing.index,
                                current_tubing.leave_refno,
                                current_tubing.arrive_refno,
                                gen_bytes_hash::<_, 64>(&aabb),
                                gen_bytes_hash::<_, 64>(&t),
                                current_tubing.tubi_size.to_string(),
                            ));
                        current_tubing.index += 1;
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
        let exist_refnos = children
            .iter()
            .map(|x| x.refno)
            .filter(|x| !local_al_map.contains_key(x))
            .collect::<Vec<_>>();
        // dbg!(&exist_refnos);
        let exist_al_map = aios_core::query_arrive_leave_points_by_cata_hash(&exist_refnos[..])
            .await
            .unwrap_or_default();
        let mut leave_type = "BRAN".to_string();
        for (index, ele) in children.into_iter().enumerate() {
            let refno = ele.refno;
            let arrive_type = ele.noun.as_str();
            // let exclude = (is_hvac && leave_type != "STRT" && leave_type != "TRNS");
            let exclude = false;
            {
                // println!("正在处理直段{}: {}", cur_type, refno.to_string());
                let world_trans = aios_core::get_world_transform(refno)
                    .await?
                    .unwrap_or_default();
                //有隐含管段
                if let Some(axis_map) =
                    exist_al_map
                        .get(&refno)
                        .or(local_al_map.get(&refno))
                        .map(|x| {
                            [
                                x[0].transformed(&world_trans),
                                x[1].transformed(&world_trans),
                            ]
                        })
                {
                    bran_comp_vec.push(refno);
                    current_tubing.arrive_refno = refno;
                    //ATTA，如果设置成SPKBRK，产生直段，否则不产生直段
                    let mut skip = (arrive_type == "ATTA" || arrive_type == "BRCO")
                        && !aios_core::get_named_attmap(refno)
                        .await?
                        .get_bool_or_default("SPKBRK");
                    // dbg!(skip);
                    if !skip {
                        let a_pos = axis_map[0].pt;
                        let Some(a_dir) = axis_map[0].dir else{
                            continue;
                        };

                        let actual_vec = a_pos - current_tubing.start_pt;
                        // dbg!(actual_vec);
                        let actual_dir = actual_vec.normalize_or_zero();
                        //判断actual_dir 和 a_dir 是否一致，一致的话说明有重叠
                        let same_dir = actual_dir.dot(a_dir) > 0.99;
                        #[cfg(feature = "debug_model")]
                        if same_dir {
                            dbg!(to_pdms_vec_str(&actual_dir));
                            dbg!(to_pdms_vec_str(&a_dir));
                        }
                        current_tubing.end_pt = a_pos;
                        current_tubing.desire_arrive_dir = a_dir;
                        let dist = actual_vec.length();
                        if dist  > TUBI_TOL && !same_dir {
                            // 如果是hvac 必须leave 的是STRT才可以
                            //风管开头这样的不需要处理
                            if !exclude {
                                if current_tubing.is_dir_ok() {
                                    // 检测到有重叠的情况，就需要忽略
                                    //如果 leave 的 还是 bran 的参考号，说明还是要用h_tubi_size
                                    if current_tubing.leave_refno == branch_refno {
                                        #[cfg(feature = "debug_model")]
                                        {
                                            dbg!(&current_tubing);
                                            println!("管道 bran 开头有个直段.");
                                        }
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
                                        // dbg!((current_tubing.leave_refno, lstube_cat_ref));
                                        current_tubing.tubi_size = fast_model::query_tubi_size(
                                            current_tubing.leave_refno,
                                            lstube_cat_ref,
                                            is_hang,
                                        ).await?;
                                    }
                                    #[cfg(feature = "debug_model")]
                                    dbg!(&current_tubing.tubi_size);
                                    tubi_geo_hash =
                                        if matches!(current_tubing.tubi_size, TubiSize::BoxSize(_))
                                        {
                                            BOXI_GEO_HASH
                                        } else {
                                            TUBI_GEO_HASH
                                        };
                                    if let Some(t) = current_tubing.get_transform() {
                                        let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                                        tubi_shape_insts_data.insert_tubi(
                                            current_tubing.leave_refno,
                                            EleGeosInfo {
                                                refno: current_tubing.leave_refno,
                                                version: branch_att.get_e3d_version(),
                                                cata_hash: Some(tubi_geo_hash.to_string()),
                                                visible: true,
                                                generic_type: get_generic_type(
                                                    current_tubing.leave_refno,
                                                )
                                                    .await
                                                    .unwrap_or_default(),
                                                aabb: Some(aabb),
                                                world_transform: t,
                                                is_solid: true,
                                                ..Default::default()
                                            },
                                        );
                                        #[cfg(feature = "debug_model")]
                                        println!(
                                            "发现直段{}->{}, 方向: {}, 辅助方向: {}, 距离: {:.3}",
                                            current_tubing.leave_refno.to_slash_string(),
                                            current_tubing.arrive_refno.to_slash_string(),
                                            to_pdms_vec_str(&current_tubing.desire_leave_dir),
                                            to_pdms_vec_str(
                                                &current_tubing.leave_ref_dir.unwrap_or_default()
                                            ),
                                            dist
                                        );
                                        let sql = format!(
                                            "relate pe:{}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                                            set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                            branch_refno,
                                            branch_refno.to_pe_key(),
                                            current_tubing.index,
                                            current_tubing.leave_refno,
                                            current_tubing.arrive_refno,
                                            gen_bytes_hash::<_, 64>(&aabb),
                                            gen_bytes_hash::<_, 64>(&t),
                                            current_tubing.tubi_size.to_string(),
                                        );
                                        // println!("sql is {}", &sql);
                                        tubi_relates.push(sql);
                                        current_tubing.index += 1;
                                    }
                                } else {
                                    #[cfg(feature = "debug_model")]
                                    {
                                        dbg!(&current_tubing);
                                        dbg!(to_pdms_vec_str(&current_tubing.desire_arrive_dir));
                                        dbg!(to_pdms_vec_str(&current_tubing.desire_leave_dir));
                                        println!("{} 的直段方向有问题", refno.to_string());
                                    }
                                }
                            }
                        }
                    }
                    {
                        let l_dir = axis_map[1].dir.unwrap_or_default();
                        let ref_dir = axis_map[1].ref_dir.unwrap_or_default();
                        // dbg!(ref_dir);

                        //todo 需要弄清楚为啥是Vec3::Z
                        let mut l_ref_dir = world_trans.transform_vec3(ref_dir).normalize_or_zero();
                        if l_ref_dir.dot(l_dir) >= 0.99 {
                            let cond = if l_dir.cross(ref_dir).z >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            };
                            l_ref_dir = cond * ref_dir;
                        }
                        if !skip {
                            let l_pos = axis_map[1].pt;
                            // dbg!(l_pos);
                            current_tubing.start_pt = l_pos;
                            current_tubing.desire_leave_dir = l_dir;
                            // dbg!(l_ref_dir);
                            current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                                Some(l_ref_dir)
                            } else {
                                None
                            };
                            current_tubing.leave_refno = refno;
                        }
                        // dbg!((current_tubing.leave_refno, l_dir, ref_dir));
                    }
                }
            }

            if index == len - 1 && !exclude {
                let last_dist = bran_ttube_pt.distance(current_tubing.start_pt);

                if last_dist > TUBI_TOL {
                    //检查是否有一端是世界坐标原点
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
                    current_tubing.desire_arrive_dir = tdir;
                    if current_tubing.is_dir_ok() {
                        if matches!(current_tubing.tubi_size, TubiSize::None) {
                            let lstube_cat_ref = aios_core::query_single_by_paths(
                                current_tubing.leave_refno,
                                &["->LSTU->CATR"],
                                &["refno"],
                            )
                                .await
                                .map(|x| x.get_refno_lossy().unwrap_or_default())
                                .unwrap_or_default();
                            // dbg!((current_tubing.leave_refno, lstube_cat_ref));
                            current_tubing.tubi_size = fast_model::query_tubi_size(
                                current_tubing.leave_refno,
                                lstube_cat_ref,
                                is_hang,
                            ).await?;
                        }
                        // dbg!(&current_tubing);
                        if let Some(t) = current_tubing.get_transform() {
                            let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                            tubi_shape_insts_data.insert_tubi(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    version: branch_att.get_e3d_version(),
                                    cata_hash: Some(tubi_geo_hash.to_string()),
                                    visible: true,
                                    generic_type: get_generic_type(current_tubing.leave_refno)
                                        .await
                                        .unwrap_or_default(),
                                    aabb: Some(aabb),
                                    world_transform: t,
                                    is_solid: true,
                                    ..Default::default()
                                },
                            );
                            #[cfg(feature = "debug_model")]
                            println!(
                                "发现直段{}->{}, 方向: {}, 辅助方向: {}, 距离: {:.3}",
                                current_tubing.leave_refno.to_slash_string(),
                                current_tubing.arrive_refno.to_slash_string(),
                                to_pdms_vec_str(&current_tubing.desire_leave_dir),
                                to_pdms_vec_str(
                                    &current_tubing.leave_ref_dir.unwrap_or_default()
                                ),
                                last_dist
                            );
                            tubi_relates.push(format!(
                                "relate pe:{}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                                set leave=pe:{},arrive=pe:{},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                branch_refno,
                                branch_refno.to_pe_key(),
                                current_tubing.index,
                                current_tubing.leave_refno,
                                current_tubing.arrive_refno,
                                gen_bytes_hash::<_, 64>(&aabb),
                                gen_bytes_hash::<_, 64>(&t),
                                current_tubing.tubi_size.to_string(),
                            ));
                            current_tubing.index += 1;
                        }
                    } else {
                        #[cfg(feature = "debug_model")]
                        {
                            dbg!(current_tubing.desire_arrive_dir);
                            println!("{} 的直段方向有问题", refno.to_string());
                        }
                    }
                }
            }
            leave_type = arrive_type.to_string();
        }
    }

    sender
        .send(tubi_shape_insts_data)
        .expect("send tubi shape_insts_data failed.");

    if !tubi_relates.is_empty() {
        // println!("tubi relate: {}", tubi_relates.join(""));
        SUL_DB.query(tubi_relates.join("")).await.unwrap();
    }
    println!(
        "处理元件库几何体: {} 花费时间: {} ms",
        unique_cata_cnt,
        t.elapsed().as_millis()
    );
    Ok(true)
}

//收集ngmr的信息
pub async fn query_ngmr_owner(
    refno: RefU64,
    ngmr_geo_refno: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    // dbg!((refno, ngmr_geo_refno));
    let att = aios_core::get_named_attmap(refno).await.unwrap_or_default();
    let c_ref = att.get_foreign_refno("CREF");
    // #[cfg(debug_assertions)]
    // dbg!(c_ref);
    let ance_result =
        aios_core::query_filter_ancestors(refno.clone(), NGMR_OWN_TYPES.map(String::from).to_vec())
            .await?;
    let o_ref = ance_result.into_iter().next();
    // #[cfg(debug_assertions)]
    // dbg!(o_ref);
    let geo_att = aios_core::get_named_attmap(ngmr_geo_refno)
        .await
        .unwrap_or_default();
    let removed_type =
        NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1)).unwrap_or_default();
    // #[cfg(debug_assertions)]
    // dbg!(removed_type);
    let mut target_refnos = vec![];
    match removed_type {
        NgmrRemovedType::Nothing => {}
        NgmrRemovedType::Attached => {
            c_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::AsDefault | NgmrRemovedType::Owner => {
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Item => target_refnos.push(refno),
        NgmrRemovedType::AttachedAndOwner => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::AttachedAndItem => {
            c_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::OwnerAndItem => {
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::All => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno);
        }
    }
    // dbg!((refno, ngmr_geo_refno, &target_refnos));
    Ok(target_refnos)
}


