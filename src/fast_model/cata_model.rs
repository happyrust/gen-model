use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use aios_core::pdms_types::*;
use dashmap::DashMap;
use aios_core::{gen_bytes_hash, HASH_PSEUDO_ATT_MAPS, NamedAttrMap, NamedAttrValue, RefU64, SUL_DB};
use aios_core::pe::SPdmsElement;
use glam::{DMat4, DVec3, Vec3};
use std::time::Instant;
use std::collections::{HashMap, HashSet};
use aios_core::csg::manifold::ManifoldRust;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use bevy_transform::components::Transform;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use nalgebra::Point3;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use std::mem::take;
use aios_core::consts::NGMR_OWN_TYPES;
use aios_core::geometry::*;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use aios_core::prim_geo::{PdmsTubing, TubiEdge};
use aios_core::tool::math_tool::to_pdms_vec_str;
use aios_core::parsed_data::CateGeomsInfo;
use crate::cata::sctn::geo::create_profile_geos;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{PlantAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model;
use crate::fast_model::shared;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use crate::consts::*;
use aios_core::prim_geo::*;
use std::borrow::BorrowMut;
use std::collections::BTreeMap;

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
    design_axis_map: &DashMap<RefU64, PlantAxisMap>,
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
    let mut tubi_relates = vec![];
    // let replace_mesh = mgr.db_option.replace_mesh;
    let replace_mesh = false;
    let multi_threads = mgr.db_option.multi_threads;
    let mut local_al_map = Arc::new(DashMap::new());

    let all_unique_keys = Arc::new(
        target_cata_map
            .iter()
            .map(|x| x.cata_hash.clone())
            .collect::<Vec<_>>()
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
            let local_al_map_clone = local_al_map.clone();

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
                    let mut shape_insts_data = instance_mgr.write().await;
                    let mut process_refno = None;
                    let mut ptset_map = BTreeMap::new();
                    //如果inst_info 已经存在了，可以直接跳过生成，直接指向过去就可以了
                    if replace_mesh || !target_cata.exist_inst {
                        //如果没有已有的，需要生成
                        let ele_refno = target_cata.group_refnos[0];
                        process_refno = Some(ele_refno);
                        println!(
                            "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                            j,
                            ele_refno.to_string(),
                            processed_cnt.lock().await.to_owned()
                        );
                        *processed_cnt.lock().await -= 1;
                        let Ok(Some(cata_refno)) = aios_core::get_cat_refno(ele_refno).await else {
                            println!("{ele_refno} 的元件库引用为空，跳过");
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
                        ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                        for (ele_refno, shapes) in brep_shapes_map {
                            // let mut found_ngmr = false;
                            let Ok(Some(mut world_transform)) =
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
                            #[cfg(debug_assertions)]
                            dbg!((ele_refno, gmse_refno));

                            //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                            //反过来查询负实体，然后查询它的owner，来找到相邻的正实体
                            let mut pos_neg_map: HashMap<RefU64, Vec<RefU64>> = if gmse_refno.is_valid() {
                                aios_core::query_refnos_has_pos_neg_map(&[gmse_refno], Some(true))
                                    .await
                                    .unwrap_or_default()
                            } else {
                                HashMap::new()
                            };
                            // dbg!(&pos_neg_map);
                            let mut neg_own_pos_map: HashMap<RefU64, RefU64> = pos_neg_map
                                .iter()
                                .map(|(k, negs)| negs.iter().map(|x| (*x, *k)))
                                .flatten()
                                .collect();
                            //如果有负实体，需要合在一起
                            ptset_map = design_axis_map
                                .remove(&ele_refno)
                                .map(|x| x.1)
                                .unwrap_or_default();

                            let mut geos_info = EleGeosInfo {
                                refno: ele_refno,
                                cata_hash: Some(cata_hash.clone()),
                                visible: true,
                                generic_type: mgr_clone.get_generic_type(ele_refno).await,
                                aabb: None,
                                world_transform,
                                cata_refno: Some(cata_refno),
                                neg_refnos: vec![],   //负实体是自己，这样好处理
                                has_cata_neg: false,
                                ptset_map: ptset_map.clone(),
                                ..Default::default()
                            };

                            if ele_att.contains_key("ARRI") && !ptset_map.is_empty() {
                                let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                                let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                                if let Some(a) = ptset_map.values().find(|x| x.number == arrive)
                                    && let Some(l) = ptset_map.values().find(|x| x.number == leave) {
                                    local_al_map_clone.insert(ele_refno, [a.clone(), l.clone()]);
                                }
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
                                let mut trans = brep_shape.get_trans();
                                let is_neg = neg_own_pos_map.contains_key(&refno);
                                let geo_hash = brep_shape.hash_unit_mesh_params();
                                let rot = transform.rotation;
                                let translation =
                                    transform.translation + transform.rotation * trans.translation;
                                let scale = trans.scale;
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
                                let mut cata_neg_refnos = pos_neg_map.remove(&refno).unwrap_or_default();
                                cata_neg_refnos.retain(|x| visible_set.contains(x));
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
                                    owner_pos_refnos: Default::default(),
                                    cata_neg_refnos,
                                };
                                if is_ngmr {
                                    if let Ok(target_owners) = query_ngmr_owner(ele_refno, refno).await {
                                        shape_insts_data.insert_ngmr(ele_refno, target_owners);
                                    }
                                }
                                geo_insts.push(geom_inst);
                            }
                            {
                                let mut inst_key = geos_info.get_inst_key();
                                let mut geos_data = EleInstGeosData {
                                    inst_key,
                                    refno: ele_refno,
                                    insts: geo_insts,
                                    aabb: None,
                                    type_name: cur_type.to_string(),
                                    ..Default::default()
                                };
                                #[cfg(debug_assertions)]
                                dbg!(geos_data.insts.len());
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
                        println!(
                            "正在处理同类元件库的模型当前参考号：{}",
                            ele_refno.to_string(),
                        );
                        let Ok(Some(mut origin_trans)) =
                            mgr_clone.get_world_transform(ele_refno).await
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
                            }
                        }
                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type: mgr_clone.get_generic_type(ele_refno).await,
                            aabb: None,
                            world_transform: origin_trans,
                            ..Default::default()
                        };
                        if ele_att.contains_key("ARRI") && !ptset_map.is_empty() {
                            let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                            let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                            if let Some(a) = ptset_map.values().find(|x| x.number == arrive)
                                && let Some(l) = ptset_map.values().find(|x| x.number == leave) {
                                local_al_map_clone.insert(ele_refno, [a.clone(), l.clone()]);
                            }
                        };
                        shape_insts_data.insert_info(ele_refno, geos_info);
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
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());

        let is_hang = branch_att.get_type_str() == "HANG";
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();

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
                                cata_refno: None,
                                ..Default::default()
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
        // dbg!(&children.iter().map(|x| &x.refno).collect::<Vec<_>>());
        let exist_al_map = {
            //.filter(|x| !local_al_map.contains_key(x)
            aios_core::query_arrive_leave_points(
                children.iter().map(|x| &x.refno).filter(|x| !local_al_map.contains_key(x)), false)
                .await
                .unwrap_or_default()
        };
        // dbg!(&exist_al_map);
        // dbg!(&local_al_map);
        for (index, ele) in children.into_iter().enumerate() {
            let refno = ele.refno;
            // dbg!(refno);
            let cur_type = ele.noun.as_str();
            //get the inst info
            if let Some(inst_info) = shape_insts_data.get_inst_info(refno) {
                println!("正在处理直段{}: {}", cur_type, refno.to_string());
                let world_trans = inst_info.world_transform;
                //有隐含管段
                if let Some(axis_map) = exist_al_map
                    .get(&refno)
                    .map(|x| x.clone())
                    .or(
                        local_al_map.get(&refno)
                            .map(|x| [x[0].transformed(&world_trans), x[1].transformed(&world_trans)])
                    ) {
                    // dbg!(&axis_map);
                    bran_comp_vec.push(refno);
                    current_tubing.arrive_refno = refno;
                    //ATTA，如果设置成SPKBRK，产生直段，否则不产生直段
                    let skip = (cur_type == "ATTA")
                        && !aios_core::get_named_attmap(refno)
                        .await?
                        .get_bool_or_default("SPKBRK");
                    // dbg!(skip);
                    if !skip {
                        let a_pos = axis_map[0].pt;
                        let a_dir = axis_map[0].dir;

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
                                        // dbg!(&lstube_cat_ref);
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
                                                ..Default::default()
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
                    {
                        let l_dir = axis_map[1].dir;
                        let ref_dir = axis_map[1].ref_dir;
                        // let cond = if l_dir.cross(Vec3::Y).z >= 0.0 { 1.0 } else { 0.0 };
                        //todo 需要弄清楚为啥是Vec3::Z
                        let mut l_ref_dir = world_trans.transform_vec3(Vec3::Z).normalize_or_zero();
                        if l_ref_dir.dot(l_dir) >= 0.99 {
                            let cond = if l_dir.cross(ref_dir).z >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            };
                            // dbg!(cond);
                            l_ref_dir = cond * ref_dir;
                        }
                        if skip {} else {
                            let l_pos = axis_map[1].pt;
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
            }

            if index == len - 1 && !is_hvac {
                let last_dist = bran_ttube_pt.distance(current_tubing.start_pt);
                // dbg!(last_dist);
                if last_dist > TUBI_TOL {
                    //检查是否有一端是世界坐标原点
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
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
                                    ..Default::default()
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


//收集ngmr的信息
pub async fn query_ngmr_owner(refno: RefU64, ngmr_geo_refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
    // dbg!((refno, ngmr_geo_refno));
    let att = aios_core::get_named_attmap(refno).await.unwrap_or_default();
    let c_ref = att.get_foreign_refno("CREF");
    // #[cfg(debug_assertions)]
    // dbg!(c_ref);
    let ance_result = aios_core::query_filter_ancestors(
        refno.clone(),
        NGMR_OWN_TYPES.map(String::from).to_vec(),
    )
        .await?;
    let o_ref = ance_result.into_iter().next();
    // #[cfg(debug_assertions)]
    // dbg!(o_ref);
    let geo_att = aios_core::get_named_attmap(ngmr_geo_refno)
        .await
        .unwrap_or_default();
    let removed_type =
        NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1))
            .unwrap_or_default();
    // #[cfg(debug_assertions)]
    // dbg!(removed_type);
    let mut target_refnos = vec![];
    match removed_type {
        NgmrRemovedType::Nothing => {}
        NgmrRemovedType::Attached => { c_ref.map(|x| target_refnos.push(x)); }
        NgmrRemovedType::AsDefault | NgmrRemovedType::Owner => { o_ref.map(|x| target_refnos.push(x)); }
        NgmrRemovedType::Item => { target_refnos.push(refno) }
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
    // dbg!(&target_refnos);
    Ok(target_refnos)
}