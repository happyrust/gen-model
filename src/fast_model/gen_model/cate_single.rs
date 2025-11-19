// 单个 Cate 元件的几何生成
//
// 从 cata_model.rs 提取的 gen_cata_single_geoms 函数及其依赖

use crate::data_interface::structs::PlantAxisMap;
use crate::fast_model::{debug_model, debug_model_debug, resolve_desi_comp};
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::prim_geo::category::{CateCsgShape, convert_to_csg_shapes};
use aios_core::prim_geo::profile::create_profile_geos;
use aios_core::{NamedAttrMap, RefnoEnum};
use dashmap::DashMap;
use std::sync::Arc;

pub type CateCsgShapeMap = DashMap<RefnoEnum, Vec<CateCsgShape>>;

/// 获取单个元件的模型数据
///
/// # Arguments
/// * `design_refno` - 设计元件的 refno
/// * `csg_shape_map` - CSG 形状映射表
/// * `design_axis_map` - 设计轴映射表
///
/// # 处理流程
/// 1. 获取元件属性
/// 2. 解析元件几何信息
/// 3. 特殊处理 Profile 类型 (SCTN/STWALL/GENSEC/WALL)
/// 4. 普通元件转换为 CSG 形状
/// 5. 处理负实体 (n_geometries)
///
/// # 性能优化
/// - 使用 #[cfg(feature = "profile")] 条件编译性能跟踪
/// - 分段计时各个处理步骤
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

    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        use crate::fast_model::ModelErrorKind;
        crate::model_error!(
            code = "E-REF-002",
            kind = ModelErrorKind::InvalidReference,
            stage = "validate_owner",
            refno = design_refno,
            desc = "DESI元件owner无效",
            "design_refno={}, owner={}, type_name={}",
            design_refno,
            owner,
            type_name
        );
        return Ok(false);
    }

    // Timing for resolve_desi_comp
    let t_resolve = std::time::Instant::now();
    let geoms_info = resolve_desi_comp(design_refno, None).await?;
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

    // Profile 类型特殊处理
    if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" || type_name == "WALL"
    {
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
    }

    // 普通元件处理
    let CateGeomsInfo {
        refno,
        geometries,
        n_geometries,
        axis_map,
    } = geoms_info;

    debug_model!(
        "geometries.len()={}, n_geometries.len()={}",
        geometries.len(),
        n_geometries.len()
    );

    // 转换正实体几何
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

    // 转换负实体几何 (NGMR)
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

    // 保存轴映射
    let t_axis_map = std::time::Instant::now();
    design_axis_map.insert(design_refno, axis_map);
    let axis_map_time = t_axis_map.elapsed().as_millis();

    debug_model!(
        "Final stats: geo_count={}, ngeo_count={}, csg_shape_map entry count for design_refno={}",
        geo_count,
        ngeo_count,
        csg_shape_map
            .get(&design_refno)
            .map(|v| v.len())
            .unwrap_or(0)
    );

    // 检查是否完全没有几何成功转换
    if geo_count == 0 && ngeo_count == 0 {
        use crate::fast_model::ModelErrorKind;
        crate::model_error!(
            code = "E-GEO-003",
            kind = ModelErrorKind::UnsupportedGeometry,
            stage = "convert_to_csg_shapes",
            refno = design_refno,
            desc = "元件未生成任何几何",
            "design_refno={}, type_name={}, geometries_len={}, n_geometries_len={}",
            design_refno,
            type_name,
            geometries.len(),
            n_geometries.len()
        );
    }

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

    Ok(true)
}
