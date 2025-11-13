use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::options::DbOption;
use aios_core::room::algorithm::*;
use aios_core::RecordId;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use aios_core::{RefnoEnum, init_demo_test_surreal, init_test_surreal};

// 使用改进的房间查询模块
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
use aios_core::room::query_v2::{
    query_room_number_by_point_v2,
    batch_query_room_numbers,
    get_room_query_stats,
    clear_geometry_cache,
    preheat_geometry_cache,
};

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
use aios_core::spatial::hybrid_index::{get_hybrid_index, QueryOptions};

use bevy_transform::TransformPoint;
use bevy_transform::components::Transform;
use dashmap::DashMap;
use glam::{Mat4, Vec3};
use itertools::Itertools;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Vector};
use parry3d::math::{Point, Real};
use parry3d::query::PointQuery;
use parry3d::shape::{TriMesh, TriMeshFlags};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};

/// 房间关系构建统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomBuildStats {
    pub total_rooms: usize,
    pub total_panels: usize,
    pub total_components: usize,
    pub build_time_ms: u64,
    pub cache_hit_rate: f32,
    pub memory_usage_mb: f32,
}

/// 改进的几何网格缓存
/// 使用 Arc 和 DashMap 提升并发性能和内存效率
static ENHANCED_GEOMETRY_CACHE: tokio::sync::OnceCell<DashMap<String, Arc<PlantMesh>>> = 
    tokio::sync::OnceCell::const_new();

async fn get_enhanced_geometry_cache() -> &'static DashMap<String, Arc<PlantMesh>> {
    ENHANCED_GEOMETRY_CACHE.get_or_init(|| async {
        DashMap::new()
    }).await
}

/// 改进版本的房间关系构建函数
/// 
/// 主要改进：
/// 1. 使用混合空间索引提升查询性能
/// 2. 优化几何缓存机制，减少重复加载
/// 3. 添加详细的性能统计和监控
/// 4. 支持并发处理和批量操作
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub async fn build_room_relations_v2(db_option: &DbOption) -> anyhow::Result<RoomBuildStats> {
    let start_time = Instant::now();
    info!("开始构建房间关系 (改进版本)");
    
    let mesh_dir = db_option.get_meshes_path();
    let room_key_words = db_option.get_room_key_word();
    
    // 1. 预热混合空间索引
    let hybrid_index = get_hybrid_index().await;
    hybrid_index.rebuild_memory_index().await?;
    
    // 2. 构建房间面板映射关系
    let room_panel_map = build_room_panels_relate_v2(&room_key_words).await?;
    let exclude_panel_refnos = room_panel_map
        .iter()
        .map(|(_, _, panel_refnos)| panel_refnos.clone())
        .flatten()
        .collect::<HashSet<_>>();
    
    info!("找到 {} 个房间面板映射关系", room_panel_map.len());
    
    // 3. 预热几何缓存
    let all_geo_hashes = collect_geometry_hashes(&room_panel_map).await?;
    preheat_geometry_cache(all_geo_hashes).await?;
    
    let mut total_components = 0;
    let mut processed_rooms = 0;
    
    // 4. 并发处理房间关系构建
    use futures::stream::{self, StreamExt};
    
    let results = stream::iter(room_panel_map)
        .map(|(room_refno, room_num, panel_refnos)| async move {
            let mut room_components = 0;
            
            for panel_refno in panel_refnos {
                match cal_room_refnos_v2(&mesh_dir, panel_refno, &exclude_panel_refnos, 0.1).await {
                    Ok(refnos) => {
                        if !refnos.is_empty() {
                            room_components += refnos.len();
                            if let Err(e) = save_room_relate_v2(panel_refno, &refnos, &room_num).await {
                                error!("保存房间关系失败: panel={}, error={}", panel_refno, e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("计算房间构件失败: panel={}, error={}", panel_refno, e);
                    }
                }
            }
            
            (room_refno, room_num, room_components)
        })
        .buffer_unordered(4) // 限制并发数量
        .collect::<Vec<_>>()
        .await;
    
    // 5. 统计结果
    for (_, _, components) in results {
        total_components += components;
        processed_rooms += 1;
    }
    
    let build_time = start_time.elapsed();
    let query_stats = get_room_query_stats().await;
    
    let stats = RoomBuildStats {
        total_rooms: processed_rooms,
        total_panels: exclude_panel_refnos.len(),
        total_components,
        build_time_ms: build_time.as_millis() as u64,
        cache_hit_rate: query_stats.cache_hits as f32 / query_stats.total_queries.max(1) as f32,
        memory_usage_mb: estimate_memory_usage().await,
    };
    
    info!(
        "房间关系构建完成: 处理 {} 个房间, {} 个面板, {} 个构件, 耗时 {:?}",
        stats.total_rooms, stats.total_panels, stats.total_components, build_time
    );
    
    Ok(stats)
}

/// 改进版本的房间面板关系构建
async fn build_room_panels_relate_v2(
    room_key_word: &Vec<String>,
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>> {
    #[cfg(feature = "project_hd")]
    return build_room_panels_relate_common_v2(room_key_word, match_room_name_hd).await;

    #[cfg(feature = "project_hh")]
    return build_room_panels_relate_common_v2(room_key_word, match_room_name_hh).await;
    
    // 默认情况
    build_room_panels_relate_common_v2(room_key_word, |_| true).await
}

/// 改进版本的房间面板关系构建通用函数
async fn build_room_panels_relate_common_v2<F>(
    room_key_word: &Vec<String>,
    match_room_fn: F,
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>>
where
    F: Fn(&str) -> bool + Send + Sync,
{
    let start_time = Instant::now();
    
    // 构建查询条件
    let filter = room_key_word
        .iter()
        .map(|x| format!("'{}' in NAME", x))
        .join(" or ");
    
    // 根据项目类型选择查询语句
    #[cfg(feature = "project_hd")]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe, REFNO<-pe_owner<-pe<-pe_owner<-pe])[?noun='PANE']
                    ] from FRMW where {filter}
    "#
    );
    
    #[cfg(feature = "project_hh")]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe])[?noun='PANE']
                    ] from SBFR where {filter}
    "#
    );
    
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe])[?noun='PANE']
                    ] from FRMW where {filter}
    "#
    );

    let mut response = SUL_DB.query(sql).await?;
    let raw_result: Vec<(RecordId, String, Vec<RecordId>)> = response.take(0)?;

    // 转换并过滤结果
    let room_groups: Vec<(RefnoEnum, String, Vec<RefnoEnum>)> = raw_result
        .into_iter()
        .filter_map(|(room_thing, room_num, panel_things)| {
            // 验证房间号格式
            if !match_room_fn(&room_num) {
                debug!("跳过不匹配的房间号: {}", room_num);
                return None;
            }
            
            // 这里克隆一次以避免后续日志对 room_thing 的使用发生 move
            let room_refno = RefnoEnum::from(room_thing.clone());
            if !room_refno.is_valid() {
                warn!("无效的房间引用号: {:?}", room_thing);
                return None;
            }

            let panel_refnos: Vec<RefnoEnum> = panel_things
                .into_iter()
                .filter_map(|panel_thing| {
                    let panel_refno = RefnoEnum::from(panel_thing);
                    if panel_refno.is_valid() {
                        Some(panel_refno)
                    } else {
                        None
                    }
                })
                .collect();

            if panel_refnos.is_empty() {
                debug!("房间 {} 没有关联的面板", room_num);
                return None;
            }

            Some((room_refno, room_num, panel_refnos))
        })
        .collect();

    // 批量创建房间面板关系
    if !room_groups.is_empty() {
        create_room_panel_relations_batch(&room_groups).await?;
    }
    
    info!(
        "房间面板关系构建完成: {} 个关系, 耗时 {:?}",
        room_groups.len(),
        start_time.elapsed()
    );

    Ok(room_groups)
}

/// 批量创建房间面板关系
async fn create_room_panel_relations_batch(
    room_groups: &[(RefnoEnum, String, Vec<RefnoEnum>)],
) -> anyhow::Result<()> {
    let mut sql_statements = Vec::new();
    
    for (room_refno, room_num_str, panel_refnos) in room_groups {
        let sql = format!(
            "relate {}->room_panel_relate->[{}] set room_num='{}';",
            room_refno.to_pe_key(),
            panel_refnos.iter().map(|x| x.to_pe_key()).join(","),
            room_num_str
        );
        sql_statements.push(sql);
    }
    
    // 批量执行 SQL
    let batch_sql = sql_statements.join("\n");
    SUL_DB.query(batch_sql).await?;
    
    Ok(())
}

/// 改进版本的房间构件计算
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub async fn cal_room_refnos_v2(
    mesh_dir: &PathBuf,
    panel_refno: RefnoEnum,
    exclude_refnos: &HashSet<RefnoEnum>,
    inside_tol: f32,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let start_time = Instant::now();
    
    // 1. 查询几何实例
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno], true)
        .await
        .unwrap_or_default();

    if geom_insts.is_empty() {
        debug!("面板 {} 没有几何实例", panel_refno);
        return Ok(Default::default());
    }

    let mut within_refnos: HashSet<RefnoEnum> = HashSet::new();
    
    // 2. 处理每个几何实例
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            // 使用改进的几何缓存加载
            let tri_mesh = match load_geometry_with_enhanced_cache(&mesh_dir, &inst.geo_hash, &geom_inst, &inst).await {
                Ok(mesh) => mesh,
                Err(e) => {
                    warn!("加载几何文件失败: {}, error: {}", inst.geo_hash, e);
                    continue;
                }
            };

            // 3. 使用混合空间索引查询候选构件
            let query_aabb: Aabb = geom_inst.world_aabb.into();
            let hybrid_index = get_hybrid_index().await;
            
            let query_options = QueryOptions {
                tolerance: inside_tol,
                max_results: 1000,
                use_cache: true,
                ..Default::default()
            };
            
            let candidates = match hybrid_index.query_containing_point(
                query_aabb.center().into(),
                &query_options
            ).await {
                Ok(results) => results,
                Err(e) => {
                    warn!("空间索引查询失败: {}", e);
                    continue;
                }
            };
            
            // 4. 精确几何检测
            let intersecting_refnos = perform_precise_geometry_check(
                &tri_mesh,
                &candidates,
                exclude_refnos,
                panel_refno,
                inside_tol,
            ).await?;
            
            within_refnos.extend(intersecting_refnos);
        }
    }

    debug!(
        "面板 {} 包含 {} 个构件, 耗时 {:?}",
        panel_refno,
        within_refnos.len(),
        start_time.elapsed()
    );

    Ok(within_refnos)
}

/// 使用增强缓存加载几何文件
async fn load_geometry_with_enhanced_cache(
    mesh_dir: &PathBuf,
    geo_hash: &str,
    geom_inst: &GeomInstQuery,
    inst: &ModelHashInst,
) -> anyhow::Result<Arc<TriMesh>> {
    let cache = get_enhanced_geometry_cache().await;
    
    // 检查缓存
    if let Some(cached_mesh) = cache.get(geo_hash) {
        // 从缓存的 PlantMesh 构建 TriMesh
        if let Some(tri_mesh) = cached_mesh.get_tri_mesh_with_flag(
            (geom_inst.world_trans * inst.transform).to_matrix(),
            TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
        ) {
            return Ok(Arc::new(tri_mesh));
        }
    }
    
    // 加载几何文件
    let file_path = mesh_dir.join(format!("{}.mesh", geo_hash));
    let mesh = tokio::task::spawn_blocking(move || {
        PlantMesh::des_mesh_file(&file_path)
    }).await??;
    
    // 构建 TriMesh
    let tri_mesh = mesh.get_tri_mesh_with_flag(
        (geom_inst.world_trans * inst.transform).to_matrix(),
        TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
    ).ok_or_else(|| anyhow::anyhow!("无法构建 TriMesh"))?;
    
    // 更新缓存
    cache.insert(geo_hash.to_string(), Arc::new(mesh));
    
    // 缓存管理
    if cache.len() > 2000 {
        cleanup_geometry_cache(&cache).await;
    }
    
    Ok(Arc::new(tri_mesh))
}

/// 清理几何缓存
async fn cleanup_geometry_cache(cache: &DashMap<String, Arc<PlantMesh>>) {
    // 简单的清理策略：移除一半的条目
    let keys_to_remove: Vec<String> = cache
        .iter()
        .take(cache.len() / 2)
        .map(|entry| entry.key().clone())
        .collect();
    
    for key in keys_to_remove {
        cache.remove(&key);
    }
    
    info!("几何缓存清理完成，当前大小: {}", cache.len());
}

/// 执行精确几何检测（仅在启用 sqlite 特性时可用）
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
async fn perform_precise_geometry_check(
    tri_mesh: &TriMesh,
    candidates: &[aios_core::spatial::hybrid_index::QueryResult],
    exclude_refnos: &HashSet<RefnoEnum>,
    panel_refno: RefnoEnum,
    inside_tol: f32,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let mut intersecting_refnos = HashSet::new();
    
    // 采样函数：AABB 顶点 + 中心 + 边中点
    let sample_aabb_points = |aabb: &Aabb| -> Vec<Point<Real>> {
        let mut pts: Vec<Point<Real>> = aabb.vertices().to_vec();
        let center = aabb.center();
        pts.push(center);
        
        // 添加边中点采样
        let corners = aabb.vertices();
        for i in 0..corners.len() {
            for j in (i + 1)..corners.len() {
                let mid = Point::from((corners[i].coords + corners[j].coords) / 2.0);
                pts.push(mid);
            }
        }
        
        pts
    };
    
    for result in candidates {
        let refno = RefnoEnum::Refno(result.refno);
        
        // 排除自身和排除列表中的构件
        if exclude_refnos.contains(&refno) || panel_refno.refno() == result.refno {
            continue;
        }
        
        // 检查包围盒有效性
        if result.aabb.extents().magnitude().is_nan() || 
           result.aabb.extents().magnitude().is_infinite() {
            continue;
        }
        
        // 采样点检测
        let samples = sample_aabb_points(&result.aabb);
        let mut inside_count = 0;
        let total_samples = samples.len();
        
        for point in &samples {
            let distance = tri_mesh.distance_to_point(&Isometry::identity(), point, true);
            if distance <= inside_tol as Real {
                inside_count += 1;
            } else if tri_mesh.contains_point(&Isometry::identity(), point) {
                inside_count += 1;
            }
        }
        
        // 如果大部分采样点都在内部，认为构件在房间内
        let inside_ratio = inside_count as f32 / total_samples as f32;
        if inside_ratio > 0.5 {
            intersecting_refnos.insert(refno);
        }
    }
    
    Ok(intersecting_refnos)
}

/// 改进版本的房间关系保存
async fn save_room_relate_v2(
    panel_refno: RefnoEnum,
    within_refnos: &HashSet<RefnoEnum>,
    room_num: &str,
) -> anyhow::Result<()> {
    if within_refnos.is_empty() {
        return Ok(());
    }
    
    let mut sql_statements = Vec::new();
    
    for refno in within_refnos {
        let relation_id = format!("{}_{}", panel_refno, refno);
        let sql = format!(
            "relate {}->room_relate:{}->{}  set room_num='{}', confidence=0.9, created_at=time::now();",
            panel_refno.to_pe_key(),
            relation_id,
            refno.to_pe_key(),
            room_num
        );
        sql_statements.push(sql);
    }
    
    // 批量执行
    let batch_sql = sql_statements.join("\n");
    SUL_DB.query(&batch_sql).await?;
    
    debug!("保存房间关系: panel={}, components={}", panel_refno, within_refnos.len());
    Ok(())
}

/// 收集所有需要的几何哈希
async fn collect_geometry_hashes(
    room_panel_map: &[(RefnoEnum, String, Vec<RefnoEnum>)],
) -> anyhow::Result<Vec<String>> {
    let mut geo_hashes = HashSet::new();
    
    for (_, _, panel_refnos) in room_panel_map {
        for panel_refno in panel_refnos {
            let geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[*panel_refno], true)
                .await
                .unwrap_or_default();
            
            for geom_inst in geom_insts {
                for inst in geom_inst.insts {
                    geo_hashes.insert(inst.geo_hash);
                }
            }
        }
    }
    
    Ok(geo_hashes.into_iter().collect())
}

/// 估算内存使用量
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
async fn estimate_memory_usage() -> f32 {
    let cache = get_enhanced_geometry_cache().await;
    let cache_size = cache.len() as f32 * 0.5; // 假设每个缓存项平均 0.5MB

    let query_stats = get_room_query_stats().await;
    let query_cache_size = query_stats.geometry_cache_size as f32 * 0.1; // 假设每个查询缓存项 0.1MB

    cache_size + query_cache_size
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "sqlite")))]
async fn estimate_memory_usage() -> f32 {
    // 在不启用 sqlite 特性时，返回一个保守估计，避免依赖未导入的符号
    let cache = get_enhanced_geometry_cache().await;
    let cache_size = cache.len() as f32 * 0.5;
    cache_size
}

/// 房间名称匹配函数 (HD项目)
pub fn match_room_name_hd(room_name: &str) -> bool {
    let regex = Regex::new(r"^[A-Z]\d{3}$").unwrap();
    regex.is_match(room_name)
}

/// 房间名称匹配函数 (HH项目)
pub fn match_room_name_hh(room_name: &str) -> bool {
    true // HH项目接受所有房间名称
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_enhanced_geometry_cache() {
        let cache = get_enhanced_geometry_cache().await;
        assert_eq!(cache.len(), 0);
    }
    
    #[test]
    fn test_room_name_matching() {
        assert!(match_room_name_hd("A123"));
        assert!(!match_room_name_hd("AB123"));
        assert!(match_room_name_hh("任何名称"));
    }
    
    #[tokio::test]
    async fn test_memory_estimation() {
        let memory_mb = estimate_memory_usage().await;
        assert!(memory_mb >= 0.0);
    }
}
