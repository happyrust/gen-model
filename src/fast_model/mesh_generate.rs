//! CSG 几何体网格生成模块
//!
//! 本模块提供基于 CSG（Constructive Solid Geometry）的几何体网格生成功能，包括：
//! - 实例网格生成（使用 Manifold 库）
//! - 包围盒（AABB）更新
//! - 布尔运算处理
//! - SQLite 空间索引优化支持

use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use crate::fast_model::{EXIST_MESH_GEO_HASHES, utils};
use crate::fast_model::{debug_model, debug_model_debug, debug_model_trace, debug_model_warn};
use crate::{batch_update_err, db_err, deser_err, log_err, query_err};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::geometry::csg::GeneratedMesh;
use aios_core::mesh_precision::MeshPrecisionSettings;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;

use crate::spatial_index::SqliteSpatialIndex;
use aios_core::shape::pdms_shape::{PlantMesh, RsVec3};
use aios_core::tool::float_tool::{dvec4_round_3, f64_round};
use aios_core::SurrealQueryExt;
use aios_core::{
    RecordId, RefU64, RefnoEnum, SUL_DB, gen_bytes_hash, get_inst_relate_keys,
    query_deep_neg_inst_refnos, query_deep_visible_inst_refnos, utils::RecordIdExt,
};
use aios_core::{get_db_option, init_test_surreal};
// 导入几何查询相关的结构体和方法
use aios_core::{
    CataNegGroup, GeoAabbTrans, GeoParam, GmGeoData, ManiGeoTransQuery, NegInfo, ParamNegInfo,
    QueryAabbParam, QueryGeoParam, query_aabb_params, query_geo_params, query_inst_geo_ids,
};
// 使用 aios_core 中查询方法的宏
use aios_core::query_db;
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::DMat4;
use itertools::Itertools;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use parse_pdms_db::parse::round_f32;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::geometry::csg::generate_csg_mesh;

/// 在数据库中生成网格模型并更新包围盒
///
/// 该函数按批次处理参考号，依次执行：
/// 1. 生成实例网格文件
/// 2. 更新实例关联的包围盒数据
///
/// # 参数
///
/// * `option` - 数据库选项，包含网格路径、精度设置等配置
/// * `refnos` - 需要处理的参考号数组
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示执行是否成功
pub async fn gen_meshes_in_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    let replace_exist = option
        .as_ref()
        .map(|x| x.is_replace_mesh())
        .unwrap_or(false);
    // let time = std::time::Instant::now();
    let dir = option
        .as_ref()
        .map(|x| x.get_meshes_path())
        .unwrap_or("assets/meshes".into());

    // Check if the directory exists, if not, create it
    if !std::path::Path::new(&dir).exists() {
        std::fs::create_dir_all(&dir)?;
    }
    let precision = Arc::new(
        option
            .as_ref()
            .map(|opt| opt.mesh_precision().clone())
            .unwrap_or_else(|| get_db_option().mesh_precision().clone()),
    );
    for chunk in refnos.chunks(100) {
        // 生成模型文件
        gen_inst_meshes(chunk, replace_exist, dir.clone(), precision.clone())
            .await
            .unwrap();
        // println!(
        //     "gen_inst_meshes finished: {} ms",
        //     time.elapsed().as_millis()
        // );
        // let time = std::time::Instant::now();
        update_inst_relate_aabbs_by_refnos(chunk, replace_exist)
            .await
            .unwrap();
        // println!(
        //     "update_inst_relate_aabbs finished: {} ms",
        //     time.elapsed().as_millis()
        // );
    }
    Ok(())
}

/// 查询需要执行 catalog 级布尔运算的实例列表
async fn query_pending_cata_boolean(
    limit: usize,
    replace_exist: bool,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let filter_booled = if replace_exist {
        String::new()
    } else {
        "AND (booled = false OR booled = NONE)".to_string()
    };

    let sql = format!(
        r#"SELECT VALUE in
FROM inst_relate
WHERE has_cata_neg = true
  AND (bad_bool = false OR bad_bool = NONE)
  {filter_booled}
LIMIT {limit};"#,
    );

    let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;
    Ok(refnos)
}

/// 查询需要执行实例级布尔运算的实例列表
async fn query_pending_inst_boolean(
    limit: usize,
    replace_exist: bool,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let filter_booled = if replace_exist {
        String::new()
    } else {
        "AND booled_id = NONE".to_string()
    };

    let sql = format!(
        r#"SELECT VALUE in
FROM inst_relate
WHERE ((in<-neg_relate)[0] != NONE OR (in<-ngmr_relate)[0] != NONE)
  AND aabb.d != NONE
  AND (bad_bool = false OR bad_bool = NONE)
  {filter_booled}
LIMIT {limit};"#,
    );

    let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;
    Ok(refnos)
}

/// 基于 inst_relate 状态的布尔运算 Worker
///
/// 按批次扫描需要布尔运算的实例（catalog & 实例级），并复用现有
/// `booleans_meshes_in_db` 管道完成实际计算。
pub async fn run_boolean_worker(
    db_option: Arc<DbOption>,
    batch_size: usize,
) -> anyhow::Result<()> {
    let batch_size = batch_size.max(1);
    let replace_exist = db_option.is_replace_mesh();

    loop {
        let cata_refnos = query_pending_cata_boolean(batch_size, replace_exist).await?;
        let inst_refnos = query_pending_inst_boolean(batch_size, replace_exist).await?;

        if cata_refnos.is_empty() && inst_refnos.is_empty() {
            debug_model!("[boolean_worker] no pending boolean tasks, exit");
            break;
        }

        if !cata_refnos.is_empty() {
            debug_model!(
                "[boolean_worker] processing {} catalog-boolean refnos",
                cata_refnos.len()
            );
            booleans_meshes_in_db(Some(db_option.clone()), &cata_refnos).await?;
        }

        if !inst_refnos.is_empty() {
            debug_model!(
                "[boolean_worker] processing {} instance-boolean refnos",
                inst_refnos.len()
            );
            booleans_meshes_in_db(Some(db_option.clone()), &inst_refnos).await?;
        }
    }

    Ok(())
}

///执行布尔运算的部分
pub async fn booleans_meshes_in_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    for chunk in refnos.chunks(100) {
        let dir = option
            .as_ref()
            .map(|x| x.get_meshes_path())
            .unwrap_or("assets/meshes".into());
        let replace_exist = option
            .as_ref()
            .map(|x| x.is_replace_mesh())
            .unwrap_or(false);
        let time = std::time::Instant::now();
        //生成元件库内部几何体的负实体运算
        apply_cata_neg_boolean_manifold(chunk, replace_exist, dir.clone())
            .await
            .unwrap();
        apply_insts_boolean_manifold(chunk, replace_exist, dir.clone()).await?;
        // 布尔运算已统一使用 Manifold 库实现
    }
    Ok(())
}

/// 处理网格并更新数据库
///
/// # 参数
/// * `option` - 数据库选项，包含网格路径和是否替换现有网格等配置
/// * `refnos` - 需要处理的引用号列表
///
/// # 返回值
/// * `anyhow::Result<()>` - 执行结果
pub async fn process_meshes_update_db(
    option: Option<Arc<DbOption>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    let replace_exist = option
        .as_ref()
        .map(|x| x.is_replace_mesh())
        .unwrap_or(false);
    let time = std::time::Instant::now();
    let dir = option
        .as_ref()
        .map(|x| x.get_meshes_path())
        .unwrap_or("assets/meshes".into());
    let precision = Arc::new(
        option
            .as_ref()
            .map(|opt| opt.mesh_precision().clone())
            .unwrap_or_else(|| get_db_option().mesh_precision().clone()),
    );
    // dbg!(&target_refnos);
    // 生成模型文件
    gen_inst_meshes(&refnos, replace_exist, dir.clone(), precision.clone())
        .await
        .unwrap();
    println!(
        "gen_inst_meshes finished: {} ms",
        time.elapsed().as_millis()
    );
    let time = std::time::Instant::now();
    update_inst_relate_aabbs_by_refnos(&refnos, replace_exist)
        .await
        .unwrap();
    println!(
        "update_inst_relate_aabbs finished: {} ms",
        time.elapsed().as_millis()
    );

    let time = std::time::Instant::now();
    //生成元件库内部几何体的负实体运算
    apply_cata_neg_boolean_manifold(&refnos, replace_exist, dir.clone())
        .await
        .unwrap();
    // 使用 Manifold 库进行布尔运算
    apply_insts_boolean_manifold(&refnos, replace_exist, dir.clone()).await?;

    Ok(())
}

/// 使用默认数据库选项更新深层模型网格数据
///
/// # 参数
///
/// * `refnos` - 参考号数组
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示更新是否成功
pub async fn process_meshes_update_db_deep_default(refnos: &[RefnoEnum]) -> anyhow::Result<()> {
    let dboption = get_db_option();
    process_meshes_update_db_deep(&dboption, refnos).await
}

/// 使用指定数据库选项更新深层模型网格数据
///
/// # 参数
///
/// * `dboption` - 数据库选项
/// * `refnos` - 参考号数组
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示更新是否成功
pub async fn process_meshes_update_db_deep(
    dboption: &DbOption,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if !refnos.is_empty() {
        let dir = dboption.get_meshes_path();
        let replace_exist = dboption.is_replace_mesh();
        let precision = Arc::new(dboption.mesh_precision().clone());
        println!("📊 更新模型结点数量: {}", refnos.len());
        let time = std::time::Instant::now();

        for (idx, &refno) in refnos.iter().enumerate() {
            println!(
                "\n🔄 [{}/{}] 处理模型结点: {}",
                idx + 1,
                refnos.len(),
                refno
            );

            // 使用 match 来捕获错误并继续处理其他 refno
            let result: anyhow::Result<()> = async {
                let mut target_visible_refnos = vec![];
                let mut update_refnos =
                    query_deep_visible_inst_refnos(refno).await.map_err(|e| {
                        eprintln!("⚠️  查询可见实例失败 (refno: {}): {}", refno, e);
                        e
                    })?;
                target_visible_refnos.extend(update_refnos.clone());

                let neg_refnos = query_deep_neg_inst_refnos(refno).await.map_err(|e| {
                    eprintln!("⚠️  查询负实例失败 (refno: {}): {}", refno, e);
                    e
                })?;
                update_refnos.extend(neg_refnos);

                if update_refnos.is_empty() {
                    debug_model_trace!("跳过空的 update_refnos for refno: {}", refno);
                    return Ok(());
                }

                println!("  📦 实际需要更新模型结点数量: {}", update_refnos.len());

                if dboption.gen_mesh {
                    // 生成模型文件
                    let mesh_time = std::time::Instant::now();
                    gen_inst_meshes(
                        &update_refnos,
                        replace_exist,
                        dir.clone(),
                        precision.clone(),
                    )
                    .await
                    .map_err(|e| {
                        eprintln!("❌ gen_inst_meshes 失败 (refno: {}): {}", refno, e);
                        anyhow::anyhow!("生成网格失败 for refno {}: {}", refno, e)
                    })?;
                    debug_model!(
                        "  ✅ gen_inst_meshes 完成: {} ms",
                        mesh_time.elapsed().as_millis()
                    );

                    let aabb_time = std::time::Instant::now();
                    // 更新aabb 到inst relate，geo relate
                    update_inst_relate_aabbs_by_refnos(&update_refnos, replace_exist)
                        .await
                        .map_err(|e| {
                            eprintln!(
                                "❌ update_inst_relate_aabbs_by_refnos 失败 (refno: {}): {}",
                                refno, e
                            );
                            anyhow::anyhow!("更新 AABB 失败 for refno {}: {}", refno, e)
                        })?;
                    debug_model!(
                        "  ✅ update_inst_relate_aabbs 完成: {} ms",
                        aabb_time.elapsed().as_millis()
                    );
                }

                if target_visible_refnos.is_empty() {
                    debug_model_trace!("跳过空的 target_visible_refnos for refno: {}", refno);
                    return Ok(());
                }

                if dboption.apply_boolean_operation {
                    let bool_time = std::time::Instant::now();
                    //生成元件库内部几何体的负实体运算
                    apply_cata_neg_boolean_manifold(
                        &target_visible_refnos,
                        replace_exist,
                        dir.clone(),
                    )
                    .await
                    .map_err(|e| {
                        eprintln!(
                            "❌ apply_cata_neg_boolean_manifold 失败 (refno: {}): {}",
                            refno, e
                        );
                        e
                    })?;
                    apply_insts_boolean_manifold(
                        &target_visible_refnos,
                        replace_exist,
                        dir.clone(),
                    )
                    .await
                    .map_err(|e| {
                        eprintln!(
                            "❌ apply_insts_boolean_manifold 失败 (refno: {}): {}",
                            refno, e
                        );
                        e
                    })?;
                    debug_model!("  ✅ 布尔运算完成: {} ms", bool_time.elapsed().as_millis());
                }

                Ok(())
            }
            .await;

            // 如果处理失败，打印错误但继续处理下一个 refno
            if let Err(e) = result {
                eprintln!("❌ 处理 refno {} 失败: {}", refno, e);
                eprintln!("   继续处理下一个节点...\n");
            } else {
                println!("✅ 成功处理 refno: {}", refno);
            }
        }
        println!("\n⏱️  总耗时: {} ms", time.elapsed().as_millis());
    }
    Ok(())
}

/// 生成实例的网格数据
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的网格数据
/// * `dir` - 模型文件目录路径
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示生成是否成功
///
/// # 侧效与说明
/// - 并发分批查询 inst_geo 参数并生成网格
/// - 将网格序列化保存到磁盘（dir/*.mesh）
/// - 回写 SurrealDB: inst_geo.meshed/aabb/pts 字段，错误则标记 bad=true
/// - 更新内存缓存 EXIST_MESH_GEO_HASHES；最后批量保存 aabb/pts 到 SurrealDB
pub async fn gen_inst_meshes(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
    precision: Arc<MeshPrecisionSettings>,
) -> anyhow::Result<()> {
    debug_model_debug!(
        "gen_inst_meshes start: refnos={}, replace_exist={}, dir={}",
        refnos.len(),
        replace_exist,
        dir.display()
    );
    // 每批并发处理的 inst_geo 数量上限，控制单批任务规模
    const PAGE_NUM: usize = 100;
    // 计数/调试用途（目前未外显）
    let mut i = 0;

    // 根据 LOD 级别创建子目录（如果传入的 dir 不是已经包含 lod_ 前缀）
    let dir = if let Some(dir_name) = dir.file_name() {
        let dir_str = dir_name.to_string_lossy();
        // 如果目录名已经是 lod_XX 格式，直接使用
        if dir_str.starts_with("lod_") {
            dir
        } else {
            // 否则创建 LOD 子目录
            let lod_dir = dir.join(format!("lod_{:?}", precision.default_lod));
            if !lod_dir.exists() {
                std::fs::create_dir_all(&lod_dir)?;
            }
            lod_dir
        }
    } else {
        // 如果无法获取目录名，创建 LOD 子目录
        let lod_dir = dir.join(format!("lod_{:?}", precision.default_lod));
        if !lod_dir.exists() {
            std::fs::create_dir_all(&lod_dir)?;
        }
        lod_dir
    };

    // 使用结构化的 query_inst_geo_ids API 查询几何 ID
    // 根据 replace_exist 决定是否跳过已生成或异常的几何：
    // - replace_exist=true：不过滤 aabb/meshed，允许覆盖，但仍过滤 bad
    // - replace_exist=false：仅选择 aabb 为空、未网格化且非 bad 的几何
    // 返回包含 geo_id 和 has_neg_relate 字段的结构化结果
    let inst_geo_ids = match query_inst_geo_ids(refnos, replace_exist).await {
        Ok(ids) => ids,
        Err(e) => {
            debug_model_debug!(
                "query_inst_geo_ids failed for refnos={:?}: {}. This is normal for objects without geometry (e.g., FLOOR, or pipe tubing).",
                refnos,
                e
            );
            return Ok(());
        }
    };
    debug_model_debug!(
        "gen_inst_meshes fetched inst_geo_ids: {}",
        inst_geo_ids.len()
    );
    debug_model_trace!("inst_geo_ids: {:?}", &inst_geo_ids);
    // 无可处理对象则直接返回
    if inst_geo_ids.is_empty() {
        debug_model_debug!(
            "[WARN] gen_inst_meshes: inst_geo_ids empty for refnos={:?}",
            refnos
        );
        return Ok(());
    }
    let mut tasks = vec![];
    // 线程安全缓存：aabb_map 用于累积 aabb；pts_json_map 用于存储端点 JSON（去重）
    let aabb_map = Arc::new(DashMap::new());
    let pts_json_map = Arc::new(DashMap::new());
    let inst_aabb_map = Arc::new(DashMap::new());

    // 分批并发处理 inst_geo
    for (chunk_idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
        debug_model_debug!(
            "gen_inst_meshes chunk {} processing {} inst_geo ids",
            chunk_idx,
            chunk.len()
        );
        // 将本批次 inst_geo id 合并为 SurrealDB in 子查询集合，并构建 refno 映射
        let mut chunk_records: Vec<(String, Option<RefnoEnum>)> = chunk
            .iter()
            .map(|result| (result.geo_id.to_raw(), result.refno.clone()))
            .collect();
        let ids = chunk_records.iter().map(|(raw, _)| raw.as_str()).join(",");
        let ref_lookup: HashMap<String, Option<RefnoEnum>> = chunk_records.drain(..).collect();
        // 克隆所需上下文到异步任务中
        let dir = dir.clone();
        let aabb_map = aabb_map.clone();
        let pts_json_map = pts_json_map.clone();
        let precision = precision.clone();
        let inst_aabb_map = inst_aabb_map.clone();
        let chunk_idx = chunk_idx;
        let ref_lookup = ref_lookup;
        // 每批一个异步任务：查询参数 -> CSG 网格化 -> 回写
        let task = tokio::spawn(async move {
            // 查询本批所有 inst_geo 的参数
            let sql = format!("select id, param from [{}] where param != NONE", ids);
            match SUL_DB.query(&sql).await {
                Ok(mut response) => {
                    let result: Vec<QueryGeoParam> = response.take(0).unwrap();
                    debug_model_debug!(
                        "chunk {} query_geo_params count={}",
                        chunk_idx,
                        result.len()
                    );
                    debug_model_trace!("chunk {} result detail: {:?}", chunk_idx, &result);
                    if result.is_empty() {
                        debug_model_debug!(
                            "[WARN] gen_inst_meshes chunk {} returned empty query result (ids={})",
                            chunk_idx,
                            ids
                        );
                        return;
                    }
                    i += 1;
                    let mut update_sql = String::new();
                    // 遍历每个几何参数并使用 CSG 生成网格
                    for g in result {
                        debug_model_debug!("gen mesh param: {:?}", &g.param);
                        let geo_type_name = g.param.type_name();
                        let profile = precision.profile_for_geo(geo_type_name);
                        let non_scalable_geo = precision.is_non_scalable_geo(geo_type_name);
                        let mesh_id = g.id.to_mesh_id();
                        let geo_raw = g.id.to_raw();
                        let refno_for_mesh: Option<RefU64> =
                            ref_lookup.get(&geo_raw).cloned().and_then(|opt| {
                                opt.map(|refno_enum| {
                                    let ref_u64: RefU64 = refno_enum.into();
                                    ref_u64
                                })
                            });

                        // 统一使用 CSG 方式生成网格
                        match generate_csg_mesh(
                            &g.param,
                            &profile.csg_settings,
                            non_scalable_geo,
                            refno_for_mesh,
                        ) {
                            Some(csg_mesh) => {
                                // 构造带 LOD 后缀的文件名，保持所有 LOD 级别命名一致
                                let mesh_filename =
                                    format!("{}_{:?}", mesh_id, precision.default_lod);

                                if let Err(e) = handle_csg_mesh(
                                    &dir,
                                    &mesh_id,
                                    &mesh_filename,
                                    csg_mesh,
                                    &aabb_map,
                                    &pts_json_map,
                                    &inst_aabb_map,
                                    &mut update_sql,
                                )
                                .await
                                {
                                    debug_model_warn!(
                                        "CSG mesh generation failed for {}: {}",
                                        mesh_id,
                                        e
                                    );
                                    // 标记 bad，避免后续重复尝试
                                    update_sql.push_str(&format!(
                                        "update inst_geo:⟨{}⟩ set bad=true;",
                                        mesh_id
                                    ));
                                } else {
                                    // 基础 mesh 生成成功，现在生成其他 LOD 级别的 mesh
                                    use aios_core::mesh_precision::LodLevel;
                                    const LOD_LEVELS: &[LodLevel] =
                                        &[LodLevel::L1, LodLevel::L2, LodLevel::L3];

                                    // 获取基础 mesh 目录的父目录
                                    let base_mesh_dir = dir.parent().unwrap_or(&dir);

                                    for &lod_level in LOD_LEVELS {
                                        // 跳过已经生成的 default_lod
                                        if lod_level == precision.default_lod {
                                            continue;
                                        }

                                        // 获取 LOD 精度设置
                                        let lod_settings = precision.lod_settings(lod_level);

                                        // 确定 LOD 目录
                                        let lod_dir = if let Some(subdir) =
                                            precision.output_subdir(lod_level)
                                        {
                                            base_mesh_dir.join(subdir)
                                        } else {
                                            base_mesh_dir.join(format!("lod_{:?}", lod_level))
                                        };

                                        // 创建目录（如果不存在）
                                        if !lod_dir.exists() {
                                            if let Err(e) = std::fs::create_dir_all(&lod_dir) {
                                                debug_model_warn!(
                                                    "   ⚠️  创建 LOD {:?} 目录失败: {}",
                                                    lod_level,
                                                    e
                                                );
                                                continue;
                                            }
                                        }

                                        // 生成 LOD mesh
                                        match generate_csg_mesh(
                                            &g.param,
                                            &lod_settings,
                                            non_scalable_geo,
                                            refno_for_mesh,
                                        ) {
                                            Some(lod_mesh) => {
                                                // 文件名包含 LOD 后缀
                                                let lod_filename =
                                                    format!("{}_{:?}.mesh", mesh_id, lod_level);
                                                let lod_mesh_path = lod_dir.join(&lod_filename);
                                                if let Err(e) =
                                                    lod_mesh.mesh.ser_to_file(&lod_mesh_path)
                                                {
                                                    debug_model_warn!(
                                                        "   ⚠️  保存 LOD {:?} mesh 失败: {} - {}",
                                                        lod_level,
                                                        mesh_id,
                                                        e
                                                    );
                                                } else {
                                                    debug_model_debug!(
                                                        "   ✅ 生成 LOD {:?} mesh: {}",
                                                        lod_level,
                                                        lod_filename
                                                    );
                                                }
                                            }
                                            None => {
                                                debug_model_warn!(
                                                    "   ⚠️  生成 LOD {:?} mesh 失败: {}",
                                                    lod_level,
                                                    mesh_id
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            None => {
                                // CSG 生成失败
                                let failed_refnos = aios_core::query_refnos_by_geo_hash(&mesh_id)
                                    .await
                                    .unwrap_or_default();
                                debug_model_warn!(
                                    "{:?} CSG mesh generation not supported for type: {}",
                                    failed_refnos,
                                    geo_type_name
                                );
                                // 标记 bad，避免后续重复尝试
                                update_sql.push_str(&format!(
                                    "update inst_geo:⟨{}⟩ set bad=true;",
                                    mesh_id
                                ));
                            }
                        }
                    }
                    if !update_sql.is_empty() {
                        // 批量回写 SurrealDB（使用一个语句拼接多条 update）
                        debug_model_trace!("准备执行批量更新 SQL，长度: {}", update_sql.len());
                        match SUL_DB.query(&update_sql).await {
                            Ok(_) => {
                                debug_model_trace!("✅ 批量更新成功");
                            }
                            Err(e) => {
                                let ctx = crate::fast_model::error_macros::ErrorContext {
                                    location: format!("{}:{}", file!(), line!()),
                                    error_msg: e.to_string(),
                                    extra_info: vec![(
                                        "📄 SQL (前500字符)".to_string(),
                                        update_sql.chars().take(500).collect::<String>(),
                                    )],
                                };
                                ctx.print("gen_inst_meshes 批量更新失败");
                                init_save_database_error(
                                    &update_sql,
                                    &std::panic::Location::caller().to_string(),
                                );
                            }
                        }
                    }
                }
                // 本批次查询失败：记录错误并继续其他批次
                Err(e) => {
                    init_query_error(&sql, e, &std::panic::Location::caller().to_string());
                }
            }
        });
        tasks.push(task);
    }

    // 等待所有批次任务完成
    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }

    // 用新生成的 aabb 更新内存缓存，避免重复计算
    for result in inst_geo_ids {
        let h = result.geo_id.to_mesh_id();
        if let Some(aabb) = inst_aabb_map.get(&h) {
            EXIST_MESH_GEO_HASHES.insert(h.clone(), *aabb);
        }
    }

    // 批量持久化点集与 aabb 实体
    utils::save_pts_to_surreal(&pts_json_map).await;
    utils::save_aabb_to_surreal(&aabb_map).await;

    Ok(())
}

async fn handle_csg_mesh(
    dir: &Path,
    inst_key: &str,
    mesh_id: &str,
    mut generated: GeneratedMesh,
    aabb_map: &Arc<DashMap<String, Aabb>>,
    pts_json_map: &Arc<DashMap<u64, String>>,
    inst_aabb_map: &Arc<DashMap<String, Aabb>>,
    update_sql: &mut String,
) -> anyhow::Result<()> {
    if generated.mesh.aabb.is_none() {
        generated.mesh.aabb = generated.aabb;
    }
    let mesh_aabb = generated
        .mesh
        .aabb
        .ok_or_else(|| anyhow!("CSG mesh 缺少有效的 AABB"))?;

    let pt_refs = derive_csg_points(&generated.mesh, pts_json_map);

    generated
        .mesh
        .ser_to_file(&dir.join(format!("{}.mesh", mesh_id)))?;

    let aabb_hash = gen_bytes_hash(&mesh_aabb);
    aabb_map.entry(aabb_hash.to_string()).or_insert(mesh_aabb);
    if !EXIST_MESH_GEO_HASHES.contains_key(mesh_id) {
        EXIST_MESH_GEO_HASHES.insert(mesh_id.to_string(), mesh_aabb);
    }
    inst_aabb_map.insert(mesh_id.to_string(), mesh_aabb);

    update_sql.push_str(&format!(
        "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩, pts=[{}];",
        inst_key,
        aabb_hash,
        pt_refs.join(","),
    ));

    Ok(())
}

fn derive_csg_points(mesh: &PlantMesh, pts_json_map: &Arc<DashMap<u64, String>>) -> Vec<String> {
    let mut hashes = HashSet::new();
    for vertex in &mesh.vertices {
        let rs_vec = RsVec3(*vertex);
        let hash = rs_vec.gen_hash();
        if hashes.insert(hash) && !pts_json_map.contains_key(&hash) {
            if let Ok(serialized) = serde_json::to_string(&rs_vec) {
                pts_json_map.insert(hash, serialized);
            }
        }
    }
    hashes
        .into_iter()
        .map(|hash| format!("vec3:⟨{}⟩", hash))
        .collect()
}

/// 更新实例关联的包围盒数据（带 SQLite 空间索引优化）
///
/// 这是 aios_core::update_inst_relate_aabbs_by_refnos 的增强版本，
/// 集成了 SQLite 空间索引支持，用于读取和写入 AABB 缓存。
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的包围盒数据
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示更新是否成功
///
/// # 优化说明
///
/// 如果启用了 `sqlite-index` feature：
/// - 优先从 SQLite 空间索引读取已缓存的 AABB
/// - 计算新的 AABB 后写入 SQLite 空间索引
/// - 减少重复计算，提升性能
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()> {
    // 如果没有启用 SQLite 索引，直接使用 aios_core 的基础版本
    #[cfg(not(feature = "sqlite-index"))]
    {
        return aios_core::update_inst_relate_aabbs_by_refnos(refnos, replace_exist).await;
    }

    // 启用了 SQLite 索引，使用优化版本
    #[cfg(feature = "sqlite-index")]
    {
        const CHUNK: usize = 100;
        let aabb_map = DashMap::new();

        // 🔥 创建 channel 用于异步 SQLite 写入
        let (sqlite_sender, sqlite_receiver) = flume::unbounded::<(RefU64, Aabb, String)>();

        // 🔥 启动异步 SQLite 批量写入任务
        let sqlite_task = tokio::spawn(async move {
            if !SqliteSpatialIndex::is_enabled() {
                return;
            }

            let spatial_index = match SqliteSpatialIndex::with_default_path() {
                Ok(idx) => idx,
                Err(e) => {
                    debug_model_warn!("SQLite 空间索引打开失败: {}", e);
                    return;
                }
            };

            let mut batch = Vec::with_capacity(100);
            while let Ok((refno, aabb, noun)) = sqlite_receiver.recv() {
                batch.push((refno, aabb, noun));

                // 批量写入，减少 I/O 次数
                if batch.len() >= 100 {
                    for (r, a, n) in batch.drain(..) {
                        let _ = spatial_index.insert_aabb(r, &a, Some(&n));
                    }
                }
            }

            // 处理剩余数据
            for (r, a, n) in batch {
                let _ = spatial_index.insert_aabb(r, &a, Some(&n));
            }

            debug_model_trace!("✅ SQLite 异步写入任务完成");
        });

        for chunk in refnos.chunks(CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let inst_keys = get_inst_relate_keys(chunk);
            debug_model_trace!("查询 AABB 参数，chunk 大小: {}", chunk.len());

            // 查询 AABB 参数
            let result = query_aabb_params(&inst_keys, replace_exist)
                .await
                .map_err(db_err!(
                    "query_aabb_params 失败",
                    chunk_size: chunk.len(),
                    inst_keys: &inst_keys.chars().take(200).collect::<String>()
                ))?;
            debug_model_trace!("查询到 {} 条 AABB 结果", result.len());

            let mut update_sql = String::new();
            for r in result {
                // 优先尝试从 SQLite 空间索引读取
                if SqliteSpatialIndex::is_enabled() {
                    let spatial_index = SqliteSpatialIndex::with_default_path()
                        .expect("Failed to open spatial index");
                    if let Ok(Some(aabb)) = spatial_index.get_aabb(r.refno.refno()) {
                        let aabb_hash = gen_bytes_hash(&aabb).to_string();
                        aabb_map.entry(aabb_hash.clone()).or_insert(aabb);
                        let sql = format!(
                            "update {} set aabb = aabb:⟨{}⟩;",
                            r.refno.to_inst_relate_key(),
                            aabb_hash,
                        );
                        update_sql.push_str(&sql);
                        continue;
                    }
                }

                // 缓存未命中则计算并回填
                let mut aabb = Aabb::new_invalid();
                for g in &r.geo_aabbs {
                    let t = r.world_trans * g.trans;
                    let tmp_aabb = g.aabb.scaled(&t.scale.into());
                    let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                        rotation: t.rotation.into(),
                        translation: t.translation.into(),
                    });
                    aabb.merge(&tmp_aabb);
                }

                if aabb.extents().magnitude().is_nan() || aabb.extents().magnitude().is_infinite() {
                    debug_model_warn!("发现无效 AABB for refno: {:?}", r.refno);
                    continue;
                }

                let aabb_hash = gen_bytes_hash(&aabb).to_string();
                aabb_map.entry(aabb_hash.clone()).or_insert(aabb);
                let bbox = RStarBoundingBox::new(aabb, r.refno, r.noun.clone());

                // 🔥 异步发送到 SQLite 写入任务
                if SqliteSpatialIndex::is_enabled() {
                    let _ = sqlite_sender.send((bbox.refno, bbox.aabb, bbox.noun));
                }

                let sql = format!(
                    "update {} set aabb = aabb:⟨{}⟩;",
                    r.refno.to_inst_relate_key(),
                    aabb_hash,
                );
                update_sql.push_str(&sql);
            }

            if !update_sql.is_empty() {
                debug_model_trace!("准备执行 AABB 更新 SQL，长度: {}", update_sql.len());
                SUL_DB.query(&update_sql).await.map_err(batch_update_err!(
                    "update_inst_relate_aabbs_by_refnos",
                    update_sql
                ))?;
                debug_model_trace!("✅ AABB 批量更新成功");
            }
        }

        // 🔥 关闭 sender，通知 SQLite 任务结束
        drop(sqlite_sender);

        // 🔥 等待 SQLite 写入任务完成
        let _ = sqlite_task.await;

        utils::save_aabb_to_surreal(&aabb_map).await;
        Ok(())
    }
}

// Database query structures are now imported from aios_core::query_structs
