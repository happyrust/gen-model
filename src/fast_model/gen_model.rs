use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::pdms_inst::save_instance_data;
use crate::fast_model::{
    cata_model, loop_model, prim_model, process_meshes_update_db_deep, resolve_desi_comp, shared,
};
use crate::versioned_db::task::{get_global_db_sender, get_global_inst_sender};
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{PlantGeoData, ShapeInstancesData};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::SUL_DB;
use aios_core::{pdms_types::*, RefU64};
use aios_core::{prim_geo::*, DBType};
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use futures::StreamExt;
use glam::DVec3;
use glam::{DMat4, Vec3};
use nom::complete::bool;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::TryFrom;
use std::io::Read;
use std::mem::take;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

///生成几何体数据
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefU64>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    if is_incr_update || has_manual_refnos {
        gen_geos_data(None, manual_refnos.clone(), db_option, incr_updates.clone()).await?;
        return Ok(true);
    }
    let dbnos = if db_option.manual_db_nums.is_some() {
        db_option.manual_db_nums.clone().unwrap()
    } else {
        aios_core::query_mdb_db_nums(DBType::DESI).await?
    };
    dbg!(&dbnos);
    for dbno in dbnos {
        gen_geos_data(
            Some(dbno),
            manual_refnos.clone(),
            db_option,
            incr_updates.clone(),
        )
        .await?;
    }

    Ok(true)
}

///生成几何体数据
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefU64>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    let skip_exist = !db_option.is_replace_mesh();
    let time = Instant::now();
    // dbg!(&incr_updates);
    const CHUNK_SIZE: usize = 100;
    //根据需要拉入数据到本地数据库也可以
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    //排除增量更新的情况，如果debug_root_refnos 为空，即没有模型需要生成
    let debug_root_refnos = db_option.get_all_debug_refnos().await;
    if !is_incr_update
        //debug_root_refnos = [] 时表示不生成模型，如果没有这个属性表示生成所有
        && (db_option.debug_root_refnos.is_some() && debug_root_refnos.is_empty())
        && (!has_manual_refnos)
    {
        return Ok(true);
    }
    if is_incr_update && incr_updates.as_ref().unwrap().count() == 0 {
        return Ok(false);
    }
    let db_option_arc = Arc::new(db_option.clone());
    let is_debug = debug_root_refnos.len() > 0;
    let mut db_nos = db_option_arc.manual_db_nums.clone().unwrap_or_default();

    let is_replace_mesh = db_option_arc.is_replace_mesh();
    let incr_count = if is_incr_update {
        incr_updates.as_ref().unwrap().count()
    } else {
        0
    };

    let sender = get_global_inst_sender().await.clone();
    let mut all_handles = vec![];

    let mut target_root_refnos = vec![];
    if is_incr_update {
        // root_refnos 为incr_update_log里的loop_refnos，basic_cata_refnos， prim_refnos的合集
        target_root_refnos = incr_updates
            .as_ref()
            .unwrap()
            .get_all_visible_refnos()
            .into_iter()
            .collect();
    } else if is_debug || has_manual_refnos {
        target_root_refnos = if has_manual_refnos {
            manual_refnos.clone()
        } else {
            debug_root_refnos.clone()
        };
    } else if dbno.is_some() {
        let mut response = SUL_DB
            .query(format!(
                "select value id from SITE where REFNO.dbnum={}",
                dbno.unwrap()
            ))
            .await
            .unwrap();
        target_root_refnos = response.take(0).unwrap();
    }
    dbg!(target_root_refnos.len());
    let origin_root_refnos = target_root_refnos.clone();
    // let process_handle = tokio::spawn(async move {
    // let mut handles = vec![];
    if is_incr_update {
        println!("处理更新模型数量: {}", incr_count);
    } else if has_manual_refnos {
        println!("处理生成模型数量: {}", manual_refnos.len());
    } else if is_debug {
        println!("调试模型数量: {:?}", debug_root_refnos.len());
    } else if dbno.is_some() {
        println!("处理db: {}", dbno.unwrap());
    }
    let d_types = db_option_arc.debug_refno_types.clone();
    let mut gen_cata_flag = d_types.iter().any(|x| x == "CATA");
    let mut gen_loop_flag = d_types.iter().any(|x| x == "LOOP");
    let mut gen_prim_flag = d_types.iter().any(|x| x == "PRIM");

    // dbg!(origin_root_refnos.len());
    let incr_updates_log_arc = Arc::new(incr_updates.clone().unwrap_or_default());
    //需要在这里把origin_root_refnos 打断成小块
    let mut chunked_root_refnos = origin_root_refnos.chunks(CHUNK_SIZE);
    //遍历小块
    while let Some(target_refnos) = chunked_root_refnos.next() {
        // dbg!(target_refnos.len());
        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        // let mut gen_inst_handles = vec![];
        //TODO 检查两个类型是否有可能在一个层级树里，如果不需要可以跳过
        {
            //查找到子节点的所有PLOO类型
            let Ok(target_ploo_refnos) = aios_core::query_multi_deep_children_filter_inst(
                target_refnos,
                &["PLOO"],
                skip_exist,
            )
            .await
            else {
                continue;
            };
            #[cfg(debug_assertions)]
            {
                println!("target_ploo_refnos: {:?}", target_ploo_refnos.len());
            }
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

        //Step 2、按类目先逐个分好类的参考号集合
        //2.1 管道或者支吊架的分类
        // let mut target_refnos = vec![];
        let target_bran_hanger_refnos: Vec<RefU64> = if is_incr_update {
            incr_updates_log_arc
                .bran_hanger_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let r = aios_core::query_multi_deep_children_filter_inst(
                target_refnos,
                &["BRAN", "HANG"],
                skip_exist,
            )
            .await
            .unwrap();
            r.into_iter().collect()
        };
        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
            let map = aios_core::query_group_by_cata_hash(&target_bran_hanger_refnos)
                .await
                .unwrap_or_default();
            map
        };
        //查询单个使用元件库的数量
        let target_single_cata_map = if is_incr_update {
            let cata_map = DashMap::new();
            let cata_refnos = &incr_updates_log_arc.basic_cata_refnos;
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
                        ..Default::default()
                    },
                );
            }
            cata_map
        } else {
            //查询是否是单个使用元件库，父节点是BRAN HANG
            let sql = format!(
                "select value refno from [{}] where owner.noun in ['BRAN', 'HANG']",
                target_refnos
                    .iter()
                    .map(|x| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut response = SUL_DB.query(sql).await.unwrap();

            let Ok(bran_children_refnos) = response.take::<Vec<RefU64>>(0) else {
                dbg!("查询BRAN, HANG出错");
                continue;
            };
            let mut use_cata_refnos = aios_core::query_multi_deep_children_filter_spre(
                target_refnos.to_vec(),
                skip_exist,
            )
            .await
            .unwrap_or_default();
            // dbg!(&use_cata_refnos);
            use_cata_refnos.extend(bran_children_refnos);
            let map = aios_core::query_group_by_cata_hash(&use_cata_refnos)
                .await
                .unwrap_or_default();
            map
        };
        //打印管道/支吊架的使用数量
        if !target_bran_hanger_refnos.is_empty() && gen_cata_flag {
            println!(
                "当前分段使用管道或者支吊架元件库数量: {}",
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

            //元件库的模型计算
            //bran，hanger下需要重用的模型
            if !target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty() {
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let db_option = db_option_arc.clone();
                let sender = sender.clone();
                let handle = tokio::spawn(async move {
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        sjus_map_clone,
                        sender,
                    )
                    .await
                    .unwrap();
                });
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
            println!(
                "当前分段使用元件库数量: {}",
                target_bran_hanger_refnos.len()
            );
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle = tokio::spawn(async move {
                cata_model::gen_cata_geos(
                    db_option,
                    Arc::new(target_single_cata_map),
                    Arc::new(Default::default()),
                    sjus_map_clone,
                    sender,
                )
                .await
                .unwrap();
            });
            all_handles.push(handle);
        }

        let target_loop_owner_refnos: Vec<RefU64> = if is_incr_update {
            incr_updates_log_arc
                .loop_owner_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let mut loop_owner_refnos = aios_core::query_multi_deep_children_filter_inst(
                target_refnos,
                &GNERAL_LOOP_OWNER_NOUN_NAMES,
                skip_exist,
            )
            .await
            .unwrap_or_default();
            loop_owner_refnos.into_iter().collect()
        };
        if gen_loop_flag && !target_loop_owner_refnos.is_empty() {
            println!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let sender = sender.clone();
            let db_option = db_option_arc.clone();
            let handle = tokio::spawn(async move {
                loop_model::gen_loop_geos(
                    db_option,
                    &target_loop_owner_refnos,
                    sjus_map_clone,
                    sender,
                )
                .await
                .unwrap();
            });
            all_handles.push(handle);
        }

        let target_prim_refnos: Vec<RefU64> = if is_incr_update {
            incr_updates_log_arc.prim_refnos.iter().cloned().collect()
        } else {
            let mut prim_refnos = aios_core::query_multi_deep_children_filter_inst(
                target_refnos,
                &GNERAL_PRIM_NOUN_NAMES,
                skip_exist,
            )
            .await
            .unwrap_or_default();
            prim_refnos.into_iter().collect()
        };

        //基本元件的生成
        if gen_prim_flag && !target_prim_refnos.is_empty() {
            println!("当前分段使用基本体数量: {}", target_prim_refnos.len());
            //基本体模型的生成
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle = tokio::spawn(async move {
                prim_model::gen_prim_geos(db_option, target_prim_refnos.as_slice(), sender)
                    .await
                    .unwrap();
            });
            all_handles.push(handle);
        }
        // if gen_inst_handles.is_empty() {
        //     futures::future::join_all(gen_inst_handles).await;
        // }
        if is_incr_update {
            break;
        }
    }

    if !all_handles.is_empty() {
        futures::future::join_all(take(&mut all_handles)).await;
    }
    if dbno.is_some() {
        println!("数据库号： {} 生成完毕。", dbno.unwrap());
    }
    // process_meshes_update_db_deep(&db_option, &target_root_refnos)
    //     .await
    //     .expect("更新模型数据失败");
    // println!("更新所有模型时间: {}ms", time.elapsed().as_millis());
    Ok(true)
}

///查询tubi的大小
pub async fn query_tubi_size(
    refno: RefU64,
    tubi_cat_ref: RefU64,
    is_hang: bool,
) -> anyhow::Result<TubiSize> {
    let tubi_geoms_info = resolve_desi_comp(refno, Some(tubi_cat_ref))
        .await
        .unwrap_or_default();
    // dbg!(&tubi_geoms_info);
    for geom in &tubi_geoms_info.geometries {
        if let BoxImplied(d) = geom {
            return Ok(TubiSize::BoxSize((d.height, d.width)));
        } else if let TubeImplied(d) = geom {
            return Ok(TubiSize::BoreSize(d.diameter));
        }
    }
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
