use crate::consts::*;
use crate::data_interface::db_model::TUBI_TOL;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::PlantAxisMap;
use crate::fast_model;
use crate::fast_model::{SEND_INST_SIZE, get_generic_type, resolve_desi_comp, shared};
use crate::fast_model::{debug_model, debug_model_debug};
use aios_core::consts::{CIVIL_TYPES, NGMR_OWN_TYPES};
use aios_core::geometry::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use aios_core::prim_geo::category::{CateCsgShape, convert_to_csg_shapes};
use aios_core::prim_geo::profile::create_profile_geos;
use aios_core::prim_geo::*;
use aios_core::prim_geo::{PdmsTubing, TubiEdge};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::math_tool::to_pdms_vec_str;
use aios_core::{
    HASH_PSEUDO_ATT_MAPS, NamedAttrMap, NamedAttrValue, RefU64, RefnoEnum, SUL_DB, gen_bytes_hash,
};
use bevy_transform::components::Transform;
use dashmap::DashMap;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use glam::{DMat4, DVec3, Vec3};
use nalgebra::Point3;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use parry3d::bounding_volume::*;
use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

// #[cfg(feature = "profile")]
use tracing::{Level, info_span, instrument};

// For Chrome tracing
use std::path::Path;
#[cfg(feature = "profile")]
use tracing_chrome::{ChromeLayerBuilder, FlushGuard};
#[cfg(feature = "profile")]
use tracing_subscriber::fmt;
#[cfg(feature = "profile")]
use tracing_subscriber::prelude::*;

// Global variable to ensure tracing is initialized only once
#[cfg(feature = "profile")]
static TRACING_INITIALIZED: AtomicBool = AtomicBool::new(false);
// Global tracing guard
#[cfg(feature = "profile")]
static mut TRACING_GUARD: Option<FlushGuard> = None;

/// Initializes Chrome tracing for performance analysis
#[cfg(feature = "profile")]
pub fn init_chrome_tracing() -> anyhow::Result<()> {
    // Only initialize once
    if TRACING_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }

    let trace_path = "chrome_trace_cata_model.json";

    // Create a fresh trace file
    create_fresh_trace_file(trace_path)?;

    // Create a new builder with simplified options to reduce chances of JSON errors
    let (chrome_layer, guard) = ChromeLayerBuilder::new()
        .file(trace_path)
        .include_args(false) // Disable including args which can cause JSON formatting issues
        .include_locations(false) // Disable including locations to simplify JSON
        .build();

    // Store the guard so it doesn't get dropped
    unsafe {
        TRACING_GUARD = Some(guard);
    }

    // Only create the Chrome tracing layer without the console output layer
    tracing_subscriber::registry().with(chrome_layer).init();

    println!(
        "Chrome tracing initialized. Output will be written to {}",
        trace_path
    );
    Ok(())
}

/// Creates a fresh trace file, removing the existing one if present
#[cfg(feature = "profile")]
fn create_fresh_trace_file(path: &str) -> anyhow::Result<()> {
    // Remove existing file if it exists
    if std::fs::metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }

    // Create an empty JSON array file to ensure valid JSON structure
    let empty_trace =
        r#"{"traceEvents":[],"displayTimeUnit":"ns","systemTraceEvents":"","otherData":{}}"#;
    std::fs::write(path, empty_trace)?;

    Ok(())
}

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
    design_refno: RefnoEnum,
    csg_shape_map: &CateCsgShapeMap,
    design_axis_map: &DashMap<RefnoEnum, PlantAxisMap>,
) -> anyhow::Result<bool> {
    let total_start = std::time::Instant::now();

    // Timing for get_named_attmap
    let t_get_attmap = std::time::Instant::now();
    let desi_att = aios_core::get_named_attmap(design_refno).await?;
    let get_attmap_time = t_get_attmap.elapsed().as_millis();
    // dbg!(&desi_att);

    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        return Ok(false);
    }

    // Timing for resolve_desi_comp
    let t_resolve = std::time::Instant::now();
    let geoms_info = resolve_desi_comp(design_refno, None).await.unwrap();
    let resolve_time = t_resolve.elapsed().as_millis();

    // DEBUG: Print basic info
    debug_model!(
        "🎯 gen_cata_single_geoms: design_refno={}, type_name={}, owner={}",
        design_refno,
        type_name,
        owner
    );

    // 🔍 调试：记录 design 元素的详细信息
    if let Some(name) = desi_att.get_as_string("NAME") {
        debug_model_debug!("   NAME: {}", name);
    }
    if let Some(desc) = desi_att.get_as_string("DESC") {
        debug_model_debug!("   DESC: {}", desc);
    }
    if let Some(cat_refno) = aios_core::get_cat_refno(design_refno).await.ok().flatten() {
        debug_model_debug!("   元件库参考号: {}", cat_refno);
        if let Ok(cat_att) = aios_core::get_named_attmap(cat_refno).await {
            if let Some(cat_name) = cat_att.get_as_string("NAME") {
                debug_model_debug!("   元件库名称: {}", cat_name);
            }
        }
    }

    if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" || type_name == "WALL"
    {
        // Timing for profile geometry creation
        let t_profile = std::time::Instant::now();
        create_profile_geos(design_refno, &geoms_info, &csg_shape_map).await?;
        let profile_time = t_profile.elapsed().as_millis();

        #[cfg(feature = "profile")]
        {
            let timestamp = chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string();
            tracing::info!(
                "Performance - gen_cata_single_geoms profile: timestamp={}, refno={:?}, get_attmap={}ms, resolve={}ms, profile={}ms, total={}ms",
                timestamp,
                design_refno,
                get_attmap_time,
                resolve_time,
                profile_time,
                total_start.elapsed().as_millis()
            );
        }

        #[cfg(not(feature = "profile"))]
        let _ = (get_attmap_time, resolve_time, profile_time);

        return Ok(true);
    } else {
        let CateGeomsInfo {
            refno,
            geometries,
            n_geometries,
            axis_map,
        } = geoms_info;

        // DEBUG: Print geometries info
        debug_model!(
            "geometries.len()={}, n_geometries.len()={}",
            geometries.len(),
            n_geometries.len()
        );

        // Timing for convert_to_csg_shapes (geometries)
        let t_convert_geo = std::time::Instant::now();
        let mut geo_count = 0;
        for (idx, geom) in geometries.iter().enumerate() {
            debug_model!("Processing geometry[{}]: {:?}", idx, geom);
            match convert_to_csg_shapes(&geom) {
                Some(cate_shape) => {
                    debug_model!("Successfully converted geometry[{}] to csg shape", idx);
                    csg_shape_map
                        .entry(design_refno)
                        .or_insert(Vec::new())
                        .push(cate_shape);
                    geo_count += 1;
                }
                None => {
                    debug_model!(
                        "Failed to convert geometry[{}] to csg shape (returned None)",
                        idx
                    );
                }
            }
        }
        let convert_geo_time = t_convert_geo.elapsed().as_millis();

        // Timing for convert_to_csg_shapes (n_geometries)
        let t_convert_ngeo = std::time::Instant::now();
        let mut ngeo_count = 0;
        for (idx, geom) in n_geometries.iter().enumerate() {
            debug_model!("Processing n_geometry[{}]: {:?}", idx, geom);
            match convert_to_csg_shapes(&geom) {
                Some(mut cate_shape) => {
                    debug_model!("Successfully converted n_geometry[{}] to csg shape", idx);
                    cate_shape.is_ngmr = true;
                    csg_shape_map
                        .entry(design_refno)
                        .or_insert(Vec::new())
                        .push(cate_shape);
                    ngeo_count += 1;
                }
                None => {
                    debug_model!(
                        "Failed to convert n_geometry[{}] to csg shape (returned None)",
                        idx
                    );
                }
            }
        }
        let convert_ngeo_time = t_convert_ngeo.elapsed().as_millis();

        // Timing for axis_map insertion
        let t_axis_map = std::time::Instant::now();
        design_axis_map.insert(design_refno, axis_map);
        let axis_map_time = t_axis_map.elapsed().as_millis();

        // DEBUG: Print final statistics
        debug_model!(
            "Final stats: geo_count={}, ngeo_count={}, csg_shape_map entry count for design_refno={}",
            geo_count,
            ngeo_count,
            csg_shape_map
                .get(&design_refno)
                .map(|v| v.len())
                .unwrap_or(0)
        );

        #[cfg(feature = "profile")]
        {
            let timestamp = chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string();
            tracing::info!(
                "Performance - gen_cata_single_geoms regular: timestamp={}, refno={:?}, get_attmap={}ms, resolve={}ms, convert_geo(count={})={}ms, convert_ngeo(count={})={}ms, axis_map={}ms, total={}ms",
                timestamp,
                design_refno,
                get_attmap_time,
                resolve_time,
                geo_count,
                convert_geo_time,
                ngeo_count,
                convert_ngeo_time,
                axis_map_time,
                total_start.elapsed().as_millis()
            );
        }

        #[cfg(not(feature = "profile"))]
        let _ = (
            get_attmap_time,
            resolve_time,
            geo_count,
            convert_geo_time,
            ngeo_count,
            convert_ngeo_time,
            axis_map_time,
        );

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
#[instrument(skip(db_option, target_cata_map, branch_map, sjus_map_arc, sender))]
pub async fn gen_cata_geos(
    db_option: Arc<DbOption>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<DashMap<RefnoEnum, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    // Initialize Chrome tracing
    #[cfg(feature = "profile")]
    init_chrome_tracing()?;

    let total_t = Instant::now();
    // let mut handles = FuturesUnordered::new();
    let mut tubi_relates = vec![];
    let gen_mesh = db_option.gen_mesh;
    let mut local_al_map = Arc::new(DashMap::new());
    let is_bran = branch_map.len() > 0;

    // 用于收集总耗时的互斥锁
    let total_time_stats = Arc::new(Mutex::new(HashMap::new()));

    let db_time_fetch_keys = Instant::now();
    let all_unique_keys = Arc::new(
        target_cata_map
            .iter()
            .map(|x| x.cata_hash.clone())
            .collect::<Vec<_>>(),
    );

    let unique_cata_cnt = all_unique_keys.len();
    debug_model_debug!(
        "gen_cata_geos start: unique_cata_cnt={}, target_cata_map_len={}, branch_map_len={}",
        unique_cata_cnt,
        target_cata_map.len(),
        branch_map.len()
    );
    let mut batch_chunks_cnt = 4;
    let mut batch_size = all_unique_keys.len() / batch_chunks_cnt + 1;
    let test_refno = db_option.get_test_refno();
    //如果只有一个元件，就不分块了
    if batch_size == 1 {
        batch_chunks_cnt = all_unique_keys.len();
    }
    #[cfg(feature = "profile")]
    tracing::info!(
        unique_cata_cnt,
        batch_chunks_cnt,
        "Starting to process catalog models"
    );

    if !all_unique_keys.is_empty() {
        for i in 0..batch_chunks_cnt {
            let all_unique_keys = all_unique_keys.clone();
            let target_cata_map = target_cata_map.clone();
            let sjus_map_clone = sjus_map_arc.clone();
            let local_al_map_clone = local_al_map.clone();
            let sender = sender.clone();
            let total_time_stats = total_time_stats.clone();
            let batch_id = i + 1;

            #[cfg(feature = "profile")]
            tracing::info!(batch_id, "Starting batch processing");

            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > unique_cata_cnt {
                end_idx = unique_cata_cnt;
            }
            #[cfg(feature = "profile")]
            tracing::info!(start_idx, end_idx, "Processing batch range");
            let mut shape_insts_data = ShapeInstancesData::default();
            if is_bran {
                shape_insts_data.fill_basic_shapes();
            }

            let mut db_time_get_named_attmap = 0;
            let mut db_time_get_world_transform = 0;
            let mut db_time_get_cat_refno = 0;
            let mut db_time_query_single = 0;
            let mut db_time_gen_single_geoms = 0;
            let mut db_time_get_generic_type = 0;
            let mut db_time_hash_lock = 0;
            let mut db_time_query_refnos = 0;

            for j in start_idx..end_idx {
                #[cfg(feature = "profile")]
                tracing::debug!(item_idx = j, "Processing item");

                let cata_hash = all_unique_keys[j].clone();
                if cata_hash == "0" {
                    continue;
                }
                let target_cata = target_cata_map.get(&cata_hash).unwrap();
                let mut process_refno = None;
                let mut ptset_map = None;
                debug_model_debug!(
                    "[cata_hash={}] exist_inst={}, group_refnos={:?}",
                    cata_hash,
                    target_cata.exist_inst,
                    target_cata.group_refnos
                );

                //如果inst_info 已经存在了，可以直接跳过生成，直接指向过去就可以了
                if gen_mesh || !target_cata.exist_inst {
                    //如果没有已有的，需要生成
                    let ele_refno = target_cata.group_refnos[0];
                    process_refno = Some(ele_refno);

                    let t_get_cat_refno = Instant::now();
                    #[cfg(feature = "profile")]
                    tracing::debug!(ele_refno = ?ele_refno, "Getting cat refno");
                    let result = aios_core::get_cat_refno(ele_refno).await;
                    let cata_refno = if let Ok(Some(refno)) = result {
                        debug_model_debug!(
                            "[cata_hash={}] ele_refno={} cat_refno={}",
                            cata_hash,
                            ele_refno,
                            refno
                        );
                        refno
                    } else {
                        debug_model_debug!(
                            "[WARN] get_cat_refno failed for ele_refno={} (result={:?})",
                            ele_refno,
                            result
                        );
                        #[cfg(feature = "profile")]
                        tracing::debug!(ele_refno = ?ele_refno, "元件库引用为空，跳过");
                        continue;
                    };
                    db_time_get_cat_refno += t_get_cat_refno.elapsed().as_millis();

                    #[cfg(feature = "profile")]
                    tracing::debug!(ele_refno = ?ele_refno, cata_refno = ?cata_refno, "开始生成元件库模型");

                    let t_query_single = Instant::now();
                    #[cfg(feature = "profile")]
                    tracing::debug!(cata_refno = ?cata_refno, "Querying GMSE");
                    let gmse_refno = aios_core::query_single_by_paths(
                        cata_refno,
                        &["->GMRE", "->GSTR"],
                        &["REFNO"],
                    )
                    .await
                    .map(|x| x.get_refno_or_default());
                    db_time_query_single += t_query_single.elapsed().as_millis();
                    match gmse_refno {
                        Ok(gmse) => {
                            debug_model_debug!(
                                "[cata_hash={}] ele_refno={} gmse_refno={}",
                                cata_hash,
                                ele_refno,
                                gmse
                            );
                        }
                        Err(err) => {
                            debug_model_debug!(
                                "[WARN] query_single_by_paths gmse_refno failed for ele_refno={}, cata_refno={}: {}",
                                ele_refno,
                                cata_refno,
                                err
                            );
                            continue;
                        }
                    }

                    let t_query_single2 = Instant::now();
                    #[cfg(feature = "profile")]
                    tracing::debug!(cata_refno = ?cata_refno, "Querying NGMR");
                    let ngmr_refno =
                        aios_core::query_single_by_paths(cata_refno, &["->NGMR"], &["REFNO"])
                            .await
                            .map(|x| x.get_refno_or_default());
                    db_time_query_single += t_query_single2.elapsed().as_millis();

                    let valid_gmse = gmse_refno.as_ref().map(|r| r.is_valid()).unwrap_or(false);
                    let valid_ngmr = ngmr_refno.as_ref().map(|r| r.is_valid()).unwrap_or(false);

                    if !valid_gmse && !valid_ngmr {
                        continue;
                    }

                    let csg_shapes_map = CateCsgShapeMap::new();
                    debug_model!(
                        "Created new csg_shapes_map for ele_refno={}, cata_hash={}",
                        ele_refno,
                        &cata_hash
                    );

                    let t_get_named_attmap = Instant::now();
                    #[cfg(feature = "profile")]
                    tracing::debug!(ele_refno = ?ele_refno, "Getting named attmap");
                    let desi_att = aios_core::get_named_attmap(ele_refno)
                        .await
                        .unwrap_or_default();
                    db_time_get_named_attmap += t_get_named_attmap.elapsed().as_millis();

                    let mut design_axis_map = DashMap::new();
                    let cur_type = desi_att.get_type_str();
                    debug_model!(
                        "ele_refno={}, cur_type={}, valid_gmse={}, valid_ngmr={}",
                        ele_refno,
                        cur_type,
                        valid_gmse,
                        valid_ngmr
                    );

                    let t_gen_single_geoms = Instant::now();
                    #[cfg(feature = "profile")]
                    tracing::debug!(ele_refno = ?ele_refno, "Generating single geoms");
                    debug_model!("Calling gen_cata_single_geoms for ele_refno={}", ele_refno);
                    let r =
                        gen_cata_single_geoms(ele_refno, &csg_shapes_map, &design_axis_map).await;
                    db_time_gen_single_geoms += t_gen_single_geoms.elapsed().as_millis();
                    debug_model!(
                        "After gen_cata_single_geoms, csg_shapes_map.len()={}",
                        csg_shapes_map.len()
                    );
                    // dbg!(&csg_shapes_map);

                    match r {
                        Ok(_) => {
                            #[cfg(feature = "profile")]
                            tracing::debug!(ele_refno = ?ele_refno, "生成元件库模型成功");
                        }
                        Err(e) => {
                            #[cfg(feature = "profile")]
                            tracing::error!(ele_refno = ?ele_refno, error = ?e, "生成元件库模型失败");
                            continue;
                        }
                    };

                    {
                        // 将一些伪属性需要用到的值存下来，后面也要更新维护这些伪属性，避免重复计算
                        let t_lock = Instant::now();
                        let mut lock = HASH_PSEUDO_ATT_MAPS.write().await;
                        db_time_hash_lock += t_lock.elapsed().as_millis();

                        let psudo_map = lock
                            .entry(cata_hash.clone())
                            .or_insert(NamedAttrMap::default());

                        if desi_att.contains_key("LEAV") {
                            let arrive = desi_att.get_i32("ARRI").unwrap_or_default();
                            let leave = desi_att.get_i32("LEAV").unwrap_or_default();
                            let axis_map = design_axis_map.get(&ele_refno).unwrap();
                            if axis_map.contains_key(&arrive) {
                                let v = axis_map.get(&arrive).unwrap();
                                psudo_map
                                    .insert("ARRWID".into(), NamedAttrValue::F32Type(v.pwidth));
                                psudo_map
                                    .insert("ARRHEI".into(), NamedAttrValue::F32Type(v.pheight));
                                psudo_map.insert("ABOR".into(), NamedAttrValue::F32Type(v.pbore));
                            }

                            if axis_map.contains_key(&leave) {
                                let v = axis_map.get(&leave).unwrap();
                                psudo_map
                                    .insert("LEAWID".into(), NamedAttrValue::F32Type(v.pwidth));
                                psudo_map
                                    .insert("LEAHEI".into(), NamedAttrValue::F32Type(v.pheight));
                                psudo_map.insert("LBOR".into(), NamedAttrValue::F32Type(v.pbore));
                            }
                        }
                    }

                    ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                    debug_model!(
                        "Processing csg_shapes_map, entries count: {}",
                        csg_shapes_map.len()
                    );
                    for (ele_refno, shapes) in csg_shapes_map {
                        debug_model!(
                            "Processing ele_refno={}, shapes.len()={}",
                            ele_refno,
                            shapes.len()
                        );
                        let t_get_world_transform = Instant::now();
                        let Ok(Some(mut world_transform)) =
                            aios_core::get_world_transform(ele_refno).await
                        else {
                            debug_model!(
                                "Failed to get world_transform for ele_refno={}, skipping",
                                ele_refno
                            );
                            continue;
                        };
                        db_time_get_world_transform += t_get_world_transform.elapsed().as_millis();

                        let t_get_named_attmap2 = Instant::now();
                        let Ok(ele_att) = aios_core::get_named_attmap(ele_refno).await else {
                            debug_model!(
                                "Failed to get named_attmap for ele_refno={}, skipping",
                                ele_refno
                            );
                            continue;
                        };
                        db_time_get_named_attmap += t_get_named_attmap2.elapsed().as_millis();

                        if let Some(sjus) = ele_att.get_str("SJUS") {
                            let parent = ele_att.get_owner();
                            if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                let height = sjus_adjust.value().1;
                                let off_z = cal_sjus_value(sjus, height);

                                let t_get_world_transform2 = Instant::now();
                                let parent_trans = aios_core::get_world_transform(parent)
                                    .await
                                    .unwrap_or_default()
                                    .unwrap_or_default();
                                db_time_get_world_transform +=
                                    t_get_world_transform2.elapsed().as_millis();

                                world_transform.translation.z = parent_trans.translation.z;
                                world_transform.translation = world_transform.translation
                                    + sjus_adjust.value().0
                                    + Vec3::new(0.0, 0.0, off_z);
                            }
                        }

                        //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                        //反过来查询负实体，然后查询它的owner，来找到相邻的正实体
                        let t_query_refnos = Instant::now();
                        let mut pos_neg_map: HashMap<RefnoEnum, Vec<RefnoEnum>> = if valid_gmse {
                            if let Ok(gmse) = &gmse_refno {
                                aios_core::query_refnos_has_pos_neg_map(&[*gmse], Some(true))
                                    .await
                                    .unwrap_or_default()
                            } else {
                                HashMap::new()
                            }
                        } else {
                            HashMap::new()
                        };
                        db_time_query_refnos += t_query_refnos.elapsed().as_millis();

                        let mut neg_own_pos_map: HashMap<RefnoEnum, RefnoEnum> = pos_neg_map
                            .iter()
                            .map(|(k, negs)| negs.iter().map(|x| (*x, *k)))
                            .flatten()
                            .collect();

                        let cur_ptset_map = design_axis_map
                            .remove(&ele_refno)
                            .map(|x| x.1)
                            .unwrap_or_default();

                        let t_get_generic_type = Instant::now();
                        let generic_type = get_generic_type(ele_refno).await.unwrap_or_default();
                        db_time_get_generic_type += t_get_generic_type.elapsed().as_millis();

                        let mut geos_info = EleGeosInfo {
                            refno: ele_refno,
                            sesno: ele_att.sesno(),
                            cata_hash: Some(cata_hash.clone()),
                            visible: true,
                            generic_type,
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
                                && let Some(l) = cur_ptset_map.values().find(|x| x.number == leave)
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

                        debug_model!(
                            "About to process {} shapes for ele_refno={}",
                            shapes.len(),
                            ele_refno
                        );

                        for (shape_idx, shape) in shapes.into_iter().enumerate() {
                            debug_model!(
                                "Processing shape[{}] for ele_refno={}",
                                shape_idx,
                                ele_refno
                            );
                            let CateCsgShape {
                                refno: geom_refno,
                                csg_shape,
                                transform,
                                visible,
                                is_tubi,
                                pts,
                                is_ngmr,
                                ..
                            } = shape;

                            if !csg_shape.check_valid() {
                                debug_model!(
                                    "shape[{}] csg_shape.check_valid() failed, skipping",
                                    shape_idx
                                );
                                continue;
                            }
                            if !visible {
                                debug_model!("shape[{}] not visible, skipping", shape_idx);
                                continue;
                            }
                            let mut shape_trans = csg_shape.get_trans();
                            let is_neg = neg_own_pos_map.contains_key(&geom_refno);
                            let geo_hash = csg_shape.hash_unit_mesh_params();
                            let rot = transform.rotation;
                            let translation = transform.translation
                                + transform.rotation * shape_trans.translation;
                            let scale = shape_trans.scale;
                            let transform = Transform {
                                translation,
                                rotation: rot,
                                scale,
                            };
                            if transform.translation.is_nan()
                                || transform.rotation.is_nan()
                                || transform.scale.is_nan()
                            {
                                debug_model!(
                                    "shape[{}] transform contains NaN, skipping",
                                    shape_idx
                                );
                                continue;
                            }
                            let mut cata_neg_refnos =
                                pos_neg_map.remove(&geom_refno).unwrap_or_default();
                            cata_neg_refnos.retain(|x| visible_set.contains(x));
                            if !cata_neg_refnos.is_empty() {
                                geos_info.has_cata_neg = true;
                            }
                            let geo_type = if is_ngmr {
                                GeoBasicType::CataCrossNeg
                            } else if is_neg {
                                GeoBasicType::CataNeg
                            } else if !cata_neg_refnos.is_empty() {
                                GeoBasicType::Compound
                            } else {
                                GeoBasicType::Pos
                            };
                            let geom_inst = EleInstGeo {
                                geo_hash,
                                refno: geom_refno,
                                pts,
                                aabb: None,
                                transform,
                                geo_param: csg_shape
                                    .convert_to_geo_param()
                                    .unwrap_or(PdmsGeoParam::Unknown),
                                visible: geo_type == GeoBasicType::Pos
                                    || geo_type == GeoBasicType::Compound,
                                is_tubi,
                                geo_type,
                                cata_neg_refnos,
                            };
                            if is_ngmr {
                                if let Ok(target_owners) =
                                    query_ngmr_owner(ele_refno, geom_refno).await
                                {
                                    shape_insts_data.insert_ngmr(
                                        ele_refno,
                                        target_owners,
                                        geom_refno,
                                    );
                                }
                            }
                            debug_model!("shape[{}] successfully added to geo_insts", shape_idx);
                            geo_insts.push(geom_inst);
                        }
                        {
                            debug_model!(
                                "Finished processing shapes, geo_insts.len()={}",
                                geo_insts.len()
                            );
                            let mut inst_key = geos_info.get_inst_key();
                            geos_info.is_solid = geo_insts.iter().any(|x| {
                                x.geo_type == GeoBasicType::Pos
                                    || x.geo_type == GeoBasicType::Compound
                            });
                            let mut geos_data = EleInstGeosData {
                                inst_key,
                                refno: ele_refno,
                                insts: geo_insts,
                                aabb: None,
                                type_name: cur_type.to_string(),
                                ..Default::default()
                            };
                            if geos_data.insts.len() > 0 {
                                debug_model!(
                                    "Inserting geos_data for ele_refno={}, insts.len()={}",
                                    ele_refno,
                                    geos_data.insts.len()
                                );
                                shape_insts_data.insert_info(ele_refno, geos_info.clone());
                                shape_insts_data
                                    .insert_geos_data(geos_info.get_inst_key(), geos_data);
                            } else {
                                debug_model_debug!(
                                    "[WARN] geos_data.insts is empty, NOT inserting for ele_refno={}",
                                    ele_refno
                                );
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
                        sesno: ele_att.sesno(),
                        cata_hash: Some(cata_hash.clone()),
                        visible: true,
                        generic_type: get_generic_type(ele_refno).await.unwrap_or_default(),
                        world_transform: origin_trans,
                        ptset_map: cur_ptset_map,
                        is_solid: true,
                        ..Default::default()
                    };
                    if let Some(r_refno) = test_refno
                        && r_refno == ele_refno
                    {
                        tracing::debug!("{:?}", &geos_info);
                    }
                    shape_insts_data.insert_info(ele_refno, geos_info);
                }
                if shape_insts_data.inst_cnt() >= SEND_INST_SIZE {
                    #[cfg(feature = "profile")]
                    tracing::info!(
                        batch_id,
                        items_cnt = shape_insts_data.inst_cnt(),
                        "Sending batch data due to size limit"
                    );

                    sender
                        .send(std::mem::take(&mut shape_insts_data))
                        .expect("send cate shape_insts_data error");
                }
            }

            // 将本批次的时间统计添加到总时间统计中
            #[cfg(feature = "profile")]
            {
                let mut stats = total_time_stats.lock().await;
                *stats.entry("get_named_attmap".to_string()).or_insert(0) +=
                    db_time_get_named_attmap as u64;
                *stats.entry("get_world_transform".to_string()).or_insert(0) +=
                    db_time_get_world_transform as u64;
                *stats.entry("get_cat_refno".to_string()).or_insert(0) +=
                    db_time_get_cat_refno as u64;
                *stats.entry("query_single".to_string()).or_insert(0) +=
                    db_time_query_single as u64;
                *stats.entry("gen_single_geoms".to_string()).or_insert(0) +=
                    db_time_gen_single_geoms as u64;
                *stats.entry("get_generic_type".to_string()).or_insert(0) +=
                    db_time_get_generic_type as u64;
                *stats.entry("hash_lock".to_string()).or_insert(0) += db_time_hash_lock as u64;
                *stats.entry("query_refnos".to_string()).or_insert(0) +=
                    db_time_query_refnos as u64;
            }

            if shape_insts_data.inst_cnt() > 0 {
                debug_model!(
                    "Sending shape_insts_data at end of batch, inst_cnt={}",
                    shape_insts_data.inst_cnt()
                );
                sender
                    .send(shape_insts_data)
                    .expect("send prim shape_insts_data error");
            } else {
                debug_model!("shape_insts_data.inst_cnt() is 0, NOT sending");
            }

            #[cfg(feature = "profile")]
            tracing::info!(batch_id, "Batch processing complete");
        }
    }

    #[cfg(feature = "profile")]
    tracing::info!("Waiting for batches to complete");

    // Wait for batches to complete
    // while let Some(_) = handles.next().await {}

    #[cfg(feature = "profile")]
    tracing::info!("Processing branches");
    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    let mut tubi_shape_insts_data = ShapeInstancesData::default();

    let t_process_branch = Instant::now();
    let mut db_time_get_children = 0;
    let mut db_time_get_branch_att = 0;
    let mut db_time_get_branch_transform = 0;

    for bran_data in branch_map.iter() {
        let branch_refno = *bran_data.key();
        let children = bran_data.value();

        let t_get_children = Instant::now();
        // let Ok(children) = aios_core::get_children_pes(branch_refno).await else {
        //     continue;
        // };
        db_time_get_children += t_get_children.elapsed().as_millis();

        let t_get_named_attmap = Instant::now();
        let Ok(branch_att) = aios_core::get_named_attmap(branch_refno).await else {
            continue;
        };
        db_time_get_branch_att += t_get_named_attmap.elapsed().as_millis();

        let t_get_world_transform = Instant::now();
        let Ok(Some(branch_transform)) = aios_core::get_world_transform(branch_refno).await else {
            continue;
        };
        db_time_get_branch_transform += t_get_world_transform.elapsed().as_millis();

        let Some(hpt) = branch_att.get_vec3("HPOS") else {
            continue;
        };
        let htube_pt = branch_transform.transform_point(hpt);
        let hdir = branch_transform
            .to_matrix()
            .transform_vector3(branch_att.get_vec3("HDIR").unwrap())
            .normalize_or_zero();
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());

        let is_hang = branch_att.get_type_str() == "HANG";
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();

        let tubi_att = aios_core::get_named_attmap(h_ref).await.unwrap_or_default();
        let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        let mut h_tubi_size =
            fast_model::query_tubi_size(branch_refno, tubi_cat_ref, is_hang).await?;
        let mut tubi_geo_hash = if matches!(h_tubi_size, TubiSize::BoxSize(_)) {
            BOXI_GEO_HASH
        } else {
            TUBI_GEO_HASH
        };

        let tref = branch_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let tdir = branch_transform
            .to_matrix()
            .transform_vector3(branch_att.get_vec3("TDIR").unwrap())
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
        if children.len() == 0 && !is_hvac {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                current_tubing.desire_arrive_dir = tdir;
                let dist = current_tubing.end_pt.distance(current_tubing.start_pt);
                if dist > TUBI_TOL && current_tubing.is_dir_ok() {
                    if let Some(t) = current_tubing.get_transform() {
                        let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                        tubi_shape_insts_data.insert_tubi(
                            branch_refno,
                            EleGeosInfo {
                                refno: branch_refno,
                                sesno: branch_att.sesno(),
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
                        tubi_relates.push(format!(
                                "relate {}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                                set leave={},arrive={},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                branch_refno.to_pe_key(),
                                branch_refno.to_pe_key(),
                                current_tubing.index,
                                current_tubing.leave_refno.to_pe_key(),
                                current_tubing.arrive_refno.to_pe_key(),
                                gen_bytes_hash(&aabb),
                                gen_bytes_hash(&t),
                                current_tubing.tubi_size.to_string(),
                            ));
                        current_tubing.index += 1;
                    }
                }
            }
            continue;
        }

        let mut bran_comp_vec = vec![];
        let len = children.len();
        let exist_refnos = children
            .iter()
            .map(|x| x.refno)
            .filter(|x| !local_al_map.contains_key(x))
            .collect::<Vec<_>>();
        let refus: Vec<RefU64> = exist_refnos.iter().map(|x| x.refno()).collect();
        let exist_al_map = aios_core::query_arrive_leave_points_of_component(&refus[..])
            .await
            .unwrap_or_default();
        let mut leave_type = "BRAN".to_string();
        for (index, ele) in children.into_iter().enumerate() {
            let refno = ele.refno;
            let arrive_type = ele.noun.as_str();
            let exclude = (is_hvac && index == 0);
            {
                let world_trans = aios_core::get_world_transform(refno)
                    .await?
                    .unwrap_or_default();
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
                    let mut skip =
                        (arrive_type == "ATTA" || arrive_type == "STIF" || arrive_type == "BRCO")
                            && !aios_core::get_named_attmap(refno)
                                .await?
                                .get_bool_or_default("SPKBRK");
                    if !skip {
                        let a_pos = &axis_map[0].pt;
                        let Some(ref a_dir) = axis_map[0].dir else {
                            continue;
                        };

                        let actual_vec = **a_pos - current_tubing.start_pt;
                        let actual_dir = actual_vec.normalize_or_zero();
                        let same_dir = actual_dir.dot(**a_dir) > 0.99;
                        if same_dir {
                            debug_model!(
                                "actual_dir: {:?}, a_dir: {:?}",
                                to_pdms_vec_str(&actual_dir, false),
                                to_pdms_vec_str(&a_dir, false)
                            );
                        }
                        current_tubing.end_pt = **a_pos;
                        current_tubing.desire_arrive_dir = **a_dir;
                        let dist = actual_vec.length();
                        if dist > TUBI_TOL && !same_dir {
                            if !exclude {
                                if current_tubing.is_dir_ok() {
                                    if current_tubing.leave_refno == branch_refno {
                                        debug_model!(
                                            "current_tubing: {:?}, 管道 bran 开头有个直段.",
                                            &current_tubing
                                        );
                                        current_tubing.tubi_size = h_tubi_size;
                                    } else {
                                        let lstube_cat_ref = aios_core::query_single_by_paths(
                                            current_tubing.leave_refno,
                                            &["->LSTU->CATR"],
                                            &["REFNO"],
                                        )
                                        .await
                                        .map(|x| x.get_refno_or_default())
                                        .unwrap_or_default();
                                        current_tubing.tubi_size = fast_model::query_tubi_size(
                                            current_tubing.leave_refno,
                                            lstube_cat_ref,
                                            is_hang,
                                        )
                                        .await?;
                                    }
                                    debug_model!(
                                        "current_tubing.tubi_size: {:?}",
                                        &current_tubing.tubi_size
                                    );
                                    tubi_geo_hash =
                                        if matches!(current_tubing.tubi_size, TubiSize::BoxSize(_))
                                        {
                                            BOXI_GEO_HASH
                                        } else {
                                            TUBI_GEO_HASH
                                        };
                                    if let Some(t) = current_tubing.get_transform() {
                                        let aabb =
                                            shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                                        tubi_shape_insts_data.insert_tubi(
                                            current_tubing.leave_refno,
                                            EleGeosInfo {
                                                refno: current_tubing.leave_refno,
                                                sesno: branch_att.sesno(),
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
                                        debug_model!(
                                            "发现直段{}->{}, 方向: {}, 辅助方向: {}, 距离: {:.3}",
                                            current_tubing.leave_refno.to_e3d_id(),
                                            current_tubing.arrive_refno.to_e3d_id(),
                                            to_pdms_vec_str(
                                                &current_tubing.desire_leave_dir,
                                                false
                                            ),
                                            to_pdms_vec_str(
                                                &current_tubing.leave_ref_dir.unwrap_or_default(),
                                                false
                                            ),
                                            dist
                                        );
                                        let sql = format!(
                                            "relate {}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                            set leave={},arrive={},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                            branch_refno.to_pe_key(),
                                            branch_refno.to_pe_key(),
                                            current_tubing.index,
                                            current_tubing.leave_refno.to_pe_key(),
                                            current_tubing.arrive_refno.to_pe_key(),
                                            gen_bytes_hash(&aabb),
                                            gen_bytes_hash(&t),
                                            current_tubing.tubi_size.to_string(),
                                        );
                                        tubi_relates.push(sql);
                                        current_tubing.index += 1;
                                    }
                                } else {
                                    debug_model!(
                                        "current_tubing: {:?}, desire_arrive_dir: {}, desire_leave_dir: {}, {} 的直段方向有问题",
                                        &current_tubing,
                                        to_pdms_vec_str(&current_tubing.desire_arrive_dir, false),
                                        to_pdms_vec_str(&current_tubing.desire_leave_dir, false),
                                        refno.to_string()
                                    );
                                }
                            }
                        }
                    }
                    {
                        let l_dir = axis_map[1].dir.as_ref().map(|x| **x).unwrap_or_default();
                        let ref_dir = axis_map[1]
                            .ref_dir
                            .as_ref()
                            .map(|x| **x)
                            .unwrap_or_default();
                        let mut l_ref_dir = world_trans
                            .to_matrix()
                            .transform_vector3(ref_dir)
                            .normalize_or_zero();
                        if l_ref_dir.dot(l_dir) >= 0.99 {
                            let cond = if l_dir.cross(ref_dir).z >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            };
                            l_ref_dir = ref_dir * cond;
                        }
                        if !skip {
                            let l_pos = &axis_map[1].pt;
                            current_tubing.start_pt = **l_pos;
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

            if index == len - 1 && !exclude {
                let last_dist = bran_ttube_pt.distance(current_tubing.start_pt);

                if last_dist > TUBI_TOL {
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
                    current_tubing.desire_arrive_dir = tdir;
                    if current_tubing.is_dir_ok() {
                        if matches!(current_tubing.tubi_size, TubiSize::None) {
                            let lstube_cat_ref = aios_core::query_single_by_paths(
                                current_tubing.leave_refno,
                                &["->LSTU->CATR"],
                                &["REFNO"],
                            )
                            .await
                            .map(|x| x.get_refno_or_default())
                            .unwrap_or_default();
                            current_tubing.tubi_size = fast_model::query_tubi_size(
                                current_tubing.leave_refno,
                                lstube_cat_ref,
                                is_hang,
                            )
                            .await?;
                        }
                        if let Some(t) = current_tubing.get_transform() {
                            let aabb = shared::aabb_apply_transform(&unit_cyli_aabb, &t);
                            tubi_shape_insts_data.insert_tubi(
                                current_tubing.leave_refno,
                                EleGeosInfo {
                                    refno: current_tubing.leave_refno,
                                    sesno: branch_att.sesno(),
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
                            tubi_relates.push(format!(
                                "relate {}->tubi_relate:[{}, {}]->inst_geo:⟨{tubi_geo_hash}⟩  \
                                set leave={},arrive={},aabb=aabb:⟨{}⟩,world_trans=trans:⟨{}⟩, bore_size={};",
                                branch_refno.to_pe_key(),
                                branch_refno.to_pe_key(),
                                current_tubing.index,
                                current_tubing.leave_refno.to_pe_key(),
                                current_tubing.arrive_refno.to_pe_key(),
                                gen_bytes_hash(&aabb),
                                gen_bytes_hash(&t),
                                current_tubing.tubi_size.to_string(),
                            ));
                            current_tubing.index += 1;
                        }
                    } else {
                        debug_model!(
                            "desire_arrive_dir: {:?}, {} 的直段方向有问题",
                            current_tubing.desire_arrive_dir,
                            refno.to_string()
                        );
                    }
                }
            }
            leave_type = arrive_type.to_string();
        }
    }
    let process_branch_time = t_process_branch.elapsed().as_millis();

    // 在发送之前提取需要更新has_tubi的refno列表
    let tubi_refnos: Vec<String> = tubi_shape_insts_data
        .inst_tubi_map
        .iter()
        .map(|(refno, _)| refno.to_pe_key())
        .collect();

    let t_send_data = Instant::now();
    if tubi_shape_insts_data.inst_cnt() > 0 {
        sender
            .send(tubi_shape_insts_data)
            .expect("send tubi shape_insts_data failed.");
    }
    let send_data_time = t_send_data.elapsed().as_millis();

    let mut tubi_query_time = 0;
    if !tubi_relates.is_empty() {
        let t_query = Instant::now();
        SUL_DB.query(tubi_relates.join("")).await.unwrap();
        tubi_query_time = t_query.elapsed().as_millis();

        // 更新PE表的has_tubi字段，标记哪些元素有隐式管道
        if !tubi_refnos.is_empty() {
            let update_pe_tubi_sql =
                format!("UPDATE [{}] SET has_tubi = true;", tubi_refnos.join(","));
            SUL_DB.query(update_pe_tubi_sql).await.unwrap();
        }
    }

    // 获取并打印汇总统计信息
    let mut time_stats = HashMap::new();
    if let Ok(stats) = Arc::try_unwrap(total_time_stats) {
        time_stats = stats.into_inner();
    }

    // 添加分支处理的时间统计
    time_stats.insert("process_branch".to_string(), process_branch_time as u64);
    time_stats.insert("get_children".to_string(), db_time_get_children as u64);
    time_stats.insert("get_branch_att".to_string(), db_time_get_branch_att as u64);
    time_stats.insert(
        "get_branch_transform".to_string(),
        db_time_get_branch_transform as u64,
    );
    time_stats.insert("send_data".to_string(), send_data_time as u64);
    time_stats.insert("tubi_query".to_string(), tubi_query_time as u64);

    // 打印汇总统计信息
    println!("\n==== 数据库操作总耗时统计 (ms) ====");
    let mut stats_vec: Vec<(String, u64)> = time_stats.into_iter().collect();
    stats_vec.sort_by(|a, b| b.1.cmp(&a.1)); // 按耗时降序排序

    #[cfg(feature = "profile")]
    {
        for (op_name, time) in stats_vec {
            println!("{}: {} ms", op_name, time);
        }
        let timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        tracing::info!(
            timestamp = timestamp,
            unique_cata_cnt = unique_cata_cnt,
            total_time_ms = total_t.elapsed().as_millis() as u64,
            "处理元件库几何体完成"
        );
    }

    println!(
        "处理元件库几何体: {} 花费总时间: {} ms",
        unique_cata_cnt,
        total_t.elapsed().as_millis()
    );
    Ok(true)
}

//收集ngmr的信息
pub async fn query_ngmr_owner(
    refno: RefnoEnum,
    ngmr_geo_refno: RefnoEnum,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let att = aios_core::get_named_attmap(refno).await.unwrap_or_default();
    let owner = att.get_owner();
    let c_ref = att.get_foreign_refno("CREF");
    let ance_result = aios_core::query_filter_ancestors(refno.clone(), &NGMR_OWN_TYPES).await?;
    let o_ref = ance_result.into_iter().next();
    let geo_att = aios_core::get_named_attmap(ngmr_geo_refno)
        .await
        .unwrap_or_default();
    let removed_type =
        NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1)).unwrap_or_default();
    let mut target_refnos = vec![];
    match removed_type {
        NgmrRemovedType::AsDefault => {
            if let Some(o_refno) = o_ref {
                let o_type = aios_core::get_type_name(o_refno).await.unwrap_or_default();
                if CIVIL_TYPES.contains(&o_type.as_str()) {
                    target_refnos.push(o_refno);
                }
            }
        }
        NgmrRemovedType::Nothing => {}
        NgmrRemovedType::Attached => {
            c_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Owner => {
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
    Ok(target_refnos)
}
