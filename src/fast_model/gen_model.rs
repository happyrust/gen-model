use aios_core::consts::NGMR_OWN_TYPES;
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{pdms_types::*, RefU64};
use aios_core::prim_geo::*;
use aios_core::SUL_DB;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::DVec3;
use glam::{DMat4, Vec3};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::TryFrom;
use std::io::Read;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::cata_model::NgmrRemovedType;
use crate::fast_model::{cata_model, loop_model, prim_model, shared};
use crate::graph_db::pdms_inst_arango::*;
use crate::graph_db::pdms_mesh_arango::save_mesh_data;

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

    let replace_mesh = mgr.db_option.replace_mesh;
    if is_incr_update || is_debug {
        //处理增量更新，不需要使用db_nos
        db_nos = vec![0];
    } else if db_nos.is_empty() {
        db_nos = aios_core::get_design_dbnos(db_option.mdb_name.clone()).await?;
        dbg!(&db_nos);
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
        let mut target_refnos = vec![];
        if !is_incr_update && !is_debug {
            // root_refnos = mgr.get_gen_model_root_refnos(&target_dbnos).await?;
            let mut response = SUL_DB
                .query(format!(
                    "select value id from SITE where REFNO.dbnum={}",
                    db_no
                ))
                .await?;
            target_refnos = response.take(0)?;
        } else if is_debug {
            target_refnos = debug_root_refnos.clone();
        } else if is_incr_update {
            // root_refnos 为incr_update_log里的loop_refnos，basic_cata_refnos， prim_refnos的合集
            target_refnos = incr_update_log
                .as_ref()
                .unwrap()
                .basic_cata_refnos
                .clone()
                .into_iter()
                .collect();
            target_refnos.extend(incr_update_log.as_ref().unwrap().loop_refnos.clone());
            target_refnos.extend(incr_update_log.as_ref().unwrap().prim_refnos.clone());
        }

        dbg!(target_refnos.len());

        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        {
            //todo 区别，一个是从db nums 里获取，一个是从增量更新数据，debug数据里获取
            let target_ploo_refnos = aios_core::query_multi_filter_deep_children(
                target_refnos.clone(),
                vec!["PLOO".into()],
            )
            .await?;
            // dbg!(&target_ploo_refnos);
            for r in target_ploo_refnos {
                let Ok(loop_att) = aios_core::get_named_attmap(r).await else {
                    continue;
                };
                let owner = loop_att.get_owner();
                let mut height = loop_att
                    .get_f32("HEIG")
                    .unwrap_or(loop_att.get_f32("HEIG").unwrap_or_default());
                let sjus = loop_att.get_str("SJUS").unwrap_or_default();
                let off_z = cata_model::cal_sjus_value(sjus, height);
                //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
                //插入方向和偏移距离
                loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
            }
        }

        let loop_sjus_map_arc = Arc::new(loop_sjus_map);
        //元件库的模型计算
        {
            let target_bran_hanger_refnos: Vec<RefU64> = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .bran_hanger_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else {
                let r = aios_core::query_multi_filter_deep_children(
                    target_refnos.clone(),
                    vec!["BRAN".into(), "HANG".into()],
                )
                .await?;
                target_refnos.retain_mut(|x| !r.contains(x));
                // dbg!(&r);
                r.into_iter().collect()
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

            let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
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
                map
            };

            //获取重用的信息
            #[cfg(debug_assertions)]
            {
                dbg!(target_bran_reuse_cata_map.len());
            }

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
            } else {
                let mut response = SUL_DB
                    .query(format!(
                        "select value refno from [{}] where owner.noun in ['BRAN', 'HANG']",
                        target_refnos
                            .iter()
                            .map(|x| x.to_pe_key())
                            .collect::<Vec<_>>()
                            .join(",")
                    ))
                    .await
                    .unwrap();
                let bran_children_refnos: Vec<RefU64> = response.take(0)?;
                let mut use_cata_refnos = aios_core::query_multi_filter_deep_children(
                    target_refnos.clone(),
                    CATA_WITHOUT_REUSE_GEO_NAMES.map(String::from).to_vec(),
                )
                .await?;
                use_cata_refnos.extend(bran_children_refnos);
                let use_cata_map = DashMap::new();
                //直接使用group的办法，按cata_hash 进行分组
                for r in use_cata_refnos {
                    let Ok(Some(att)) = aios_core::get_pe(r).await else {
                        continue;
                    };
                    use_cata_map.insert(
                        att.cata_hash.clone(),
                        CataHashRefnoKV {
                            cata_hash: att.cata_hash,
                            exist_geo: None,
                            group_refnos: vec![r],
                        },
                    );
                }
                use_cata_map
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
                        cata_model::gen_cata_geos(
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
                        cata_model::gen_cata_geos(
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

        //loop 和 基本体的处理
        {
            let target_loop_refnos: Vec<RefU64> = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .loop_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else {
                let mut loop_refnos = aios_core::query_multi_filter_deep_children(
                    target_refnos.clone(),
                    GNERAL_LOOP_NOUN_NAMES.map(String::from).to_vec(),
                )
                .await?;
                // dbg!(&loop_refnos);
                loop_refnos.into_iter().collect()
            };
            println!("使用LOOP的数量: {}", target_loop_refnos.len());
            if run_cache_loop && !target_loop_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let mgr_clone = mgr.clone();
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let handle = tokio::spawn(async move {
                    loop_model::gen_loop_geos(
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
            let target_prim_refnos: Vec<RefU64> = if is_incr_update {
                incr_update_log
                    .as_ref()
                    .unwrap()
                    .prim_refnos
                    .iter()
                    .cloned()
                    .collect()
            } else {
                let mut prim_refnos = aios_core::query_multi_filter_deep_children(
                    target_refnos.clone(),
                    GNERAL_PRIM_NOUN_NAMES.map(String::from).to_vec(),
                )
                .await?;
                #[cfg(debug_assertions)]
                dbg!(&prim_refnos);
                prim_refnos.into_iter().collect()
            };
            println!("使用基本体数量: {}", target_prim_refnos.len());
            if run_cache_prim && !target_prim_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    prim_model::gen_prim_geos(
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
            let has_pos_neg_map =
                aios_core::query_refnos_has_pos_neg_map(&target_refnos, Some(false))
                    .await
                    .unwrap_or_default();
            dbg!(has_pos_neg_map.len());

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

                    for (pos_refno, origin_neg_refnos) in has_pos_neg_map {
                        println!("正在处理: {} 下的负实体", pos_refno);

                        // let Ok(children_refnos) = mgr.get_children_from_localdb(comp_refno)
                        let Ok(children_refnos) = aios_core::get_children_refnos(pos_refno).await
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
                        let mut compound_inst_info_result_map_clone =
                            compound_inst_info_result_map.clone();
                        let mut compound_inst_geos_result_map_clone =
                            compound_inst_geos_result_map.clone();

                        let mut batch_manifolds = vec![];
                        //没有正实体的情况，直接跳过
                        // if neg_refnos.is_empty() {
                        //     return;
                        // }
                        let Some(w_trans) = trans_map.get(&pos_refno).map(|x| x.value().clone())
                        else {
                            continue;
                        };
                        // #[cfg(debug_assertions)]
                        // {
                        //     dbg!(w_trans);
                        //     dbg!(quat_to_pdms_ori_str(&w_trans.rotation));
                        // }
                        // dbg!(w_trans);
                        let mut total_refnos = vec![pos_refno];
                        total_refnos.extend_from_slice(&neg_refnos);
                        let inverse_mat = w_trans.compute_matrix().as_dmat4().inverse();

                        let origin_aabb =
                            { inst_data.get_info(&pos_refno).map(|x| x.aabb).flatten() };

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
                                    // dbg!(world_geo_mat);
                                    // dbg!(quat_to_pdms_ori_str(&world_geo_mat.rotation));
                                }
                                let ele_mat =
                                    inverse_mat * world_geo_mat.compute_matrix().as_dmat4();
                                let mut local_mat =
                                    ele_mat * geo_inst.transform.compute_matrix().as_dmat4();

                                #[cfg(debug_assertions)]
                                {
                                    // dbg!(ele_mat);
                                    // dbg!(&geo_inst);
                                    // dbg!(local_mat);
                                    // dbg!(to_pdms_ori_str(&Mat3::from_mat4(local_mat.as_mat4())));
                                }

                                //如果是第一个正实体，需要生成模型计算
                                //如果是负实体，需要生成模型计算
                                let is_neg = t_refno != pos_refno || geo_inst.is_neg();
                                if t_refno == pos_refno || is_neg {
                                    if pos_refno == t_refno {
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
                                        // 使用OCC 扫描生成，如果负实体个数比较少时，可以很快生成
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
                                        // dbg!(t_refno);
                                        // dbg!(mesh.vertices.len());
                                    }

                                    let manifold: ManifoldRust = (mesh, &local_mat).into();
                                    // let manifold: ManifoldRust = mesh.into();
                                    // let new_mesh: PlantMesh = (&manifold).into();
                                    //create models dir TODO: 使用feature
                                    #[cfg(feature = "debug_obj_export")]
                                    {
                                        let _ = std::fs::create_dir_all("models");
                                        mesh.export_obj(
                                            false,
                                            &format!("models/{}.obj", t_refno.to_string()),
                                        )
                                        .expect("TODO: panic message");
                                    }

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
                        let geo_hash = *pos_refno;
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
                            // dbg!(final_manifold.num_tri());
                            let final_mesh: PlantMesh = final_manifold.clone().into();
                            //todo 使用feature
                            #[cfg(feature = "debug_obj_export")]
                            {
                                final_mesh
                                    .export_obj(false, &format!("{}.obj", "final"))
                                    .expect("TODO: panic message");
                            }
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
                            refno: pos_refno,
                            owner_pos_refnos: Default::default(),
                            pts: vec![],
                            aabb: origin_aabb.clone(),
                            transform: Transform::IDENTITY,
                            geo_param: PdmsGeoParam::CompoundShape,
                            visible: true,
                            is_tubi: false,
                            geo_type: GeoBasicType::Compound,
                        };

                        let inst_key = hash_two_str(&pos_refno.to_string(), "compound");
                        let mut comp_geos_info = EleGeosInfo {
                            refno: pos_refno,
                            visible: true,
                            generic_type: mgr.get_generic_type(pos_refno).await,
                            aabb: origin_aabb.map(|x| shared::aabb_apply_transform(&x, &w_trans)),
                            world_transform: w_trans,
                            //cata hash 用作唯一的标识符就行，后面需要变名称
                            cata_hash: Some(inst_key.to_string()),
                            flow_pt_indexs: vec![],
                            geo_type: GeoBasicType::Compound,
                            cata_refno: None,
                            ptset_map: Default::default(),
                        };
                        // dbg!(&comp_geos_info);
                        compound_inst_info_result_map_clone.insert(pos_refno, comp_geos_info);
                        let comp_type = aios_core::get_type_name(pos_refno)
                            .await
                            .unwrap_or_default();

                        compound_inst_geos_result_map_clone.insert(
                            inst_key.to_string(),
                            EleInstGeosData {
                                inst_key: inst_key.to_string(),
                                refno: pos_refno,
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
                    let data = Arc::try_unwrap(compound_inst_geos_result_map).unwrap();
                    // dbg!(data.len());
                    for (k, v) in data {
                        inst_data.insert_geos_data(k, v);
                    }
                    let data = Arc::try_unwrap(compound_inst_info_result_map).unwrap();
                    for (k, v) in data {
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
                    let ance_result = aios_core::query_filter_ancestors(
                        refno.clone(),
                        NGMR_OWN_TYPES.map(String::from).to_vec(),
                    )
                    .await?;
                    let o_ref = ance_result.into_iter().next();
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

                println!("开始处理ngmr的负实体模型：{}", boolean_ngmr_map.len());
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
                    new_geos_info.update_to_compound(Some(parent.to_string().as_str()));
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
) -> anyhow::Result<TubiSize> {
    let tubi_geoms_info = mgr
        .resolve_desi_comp(refno, Some(tubi_cat_ref))
        .await
        .unwrap_or_default();
    for geom in &tubi_geoms_info.geometries {
        if let BoxImplied(d) = geom {
            return Ok(TubiSize::BoxSize((d.height, d.width)));
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
