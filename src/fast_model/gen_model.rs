use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::cata_model::NgmrRemovedType;
use crate::fast_model::{cata_model, loop_model, prim_model, shared};
use crate::graph_db::pdms_inst::*;
use aios_core::consts::NGMR_OWN_TYPES;
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{PlantGeoData, ShapeInstancesData};
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::prim_geo::*;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::SUL_DB;
use aios_core::{pdms_types::*, RefU64};
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

/// 这个要不要做生成分页的批量处理
///生成几何体数据
pub async fn gen_all_geos_data(
    mut mgr: Arc<AiosDBManager>,
    incr_update_log: Option<IncrGeoUpdateLog>,
    skip_exist: bool,
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

        let mut target_refnos = vec![];
        if !is_incr_update && !is_debug {
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
            let target_ploo_refnos = aios_core::query_multi_deep_children_filter_inst(
                target_refnos.clone(),
                vec!["PLOO".into()],
                skip_exist,
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
                let r = aios_core::query_multi_deep_children_filter_inst(
                    target_refnos.clone(),
                    vec!["BRAN".into(), "HANG".into()],
                    skip_exist,
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
                    .unwrap_or_default();
                map
            };
            // dbg!(&target_bran_reuse_cata_map);
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
                            group_refnos: vec![r],
                            exist_inst: false,
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
                let mut use_cata_refnos = aios_core::query_multi_deep_children_filter_inst(
                    target_refnos.clone(),
                    CATA_WITHOUT_REUSE_GEO_NAMES.map(String::from).to_vec(),
                    skip_exist,
                )
                    .await?;
                dbg!(&use_cata_refnos);
                use_cata_refnos.extend(bran_children_refnos);
                let map = aios_core::query_group_by_cata_hash(&use_cata_refnos)
                    .await
                    .unwrap_or_default();
                map
            };
            #[cfg(debug_assertions)]
            {
                dbg!(&target_bran_reuse_cata_map.len());
                dbg!(&target_single_cata_map.len());
                // dbg!(&target_single_cata_map);
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
                    // let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
                    let shape_insts_data = instance_mgr.read().await;
                    println!("当前db下的元件库生成统计：");
                    // dbg!(mesh_mgr.len());
                    dbg!(shape_insts_data.inst_info_map.len());
                    // dbg!(&inst_data.inst_info_map);
                    dbg!(shape_insts_data.inst_tubi_map.len());
                }
            }
        }

        //loop 基本体的处理
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
                let mut loop_refnos = aios_core::query_multi_deep_children_filter_inst(
                    target_refnos.clone(),
                    GNERAL_LOOP_NOUN_NAMES.map(String::from).to_vec(),
                    skip_exist,
                )
                    .await?;
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
                let mut prim_refnos = aios_core::query_multi_deep_children_filter_inst(
                    target_refnos.clone(),
                    GNERAL_PRIM_NOUN_NAMES.map(String::from).to_vec(),
                    skip_exist,
                )
                    .await?;
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
        }

        {
            let inst_data = instance_mgr.read().await;
            println!("当前db下的基本体生成统计：");
            dbg!(inst_data.inst_geos_map.len());
            dbg!(inst_data.ngmr_relate_map.len());
            save_instance_data(&mgr, &inst_data).await?;
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
