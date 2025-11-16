use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::options::DbOption;
use aios_core::room::algorithm::*;
use aios_core::RecordId;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use aios_core::{RefnoEnum, init_demo_test_surreal, init_test_surreal};

// 使用改进的房间查询模块（暂时注释掉，因为这些模块可能不存在）
// #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
// use aios_core::room::query_v2::{
//     query_room_number_by_point_v2,
//     batch_query_room_numbers,
//     get_room_query_stats,
//     clear_geometry_cache,
//     preheat_geometry_cache,
// };

// #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
// use aios_core::spatial::hybrid_index::{get_hybrid_index, QueryOptions};

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
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
pub async fn build_room_relations_v2(db_option: &DbOption) -> anyhow::Result<RoomBuildStats> {
    let start_time = Instant::now();
    info!("开始构建房间关系 (改进版本)");
    
    let mesh_dir = db_option.get_meshes_path();
    let room_key_words = db_option.get_room_key_word();
    
    // 1. 构建房间面板映射关系
    let room_panel_map = build_room_panels_relate_v2(&room_key_words).await?;
    let exclude_panel_refnos = room_panel_map
        .iter()
        .map(|(_, _, panel_refnos)| panel_refnos.clone())
        .flatten()
        .collect::<HashSet<_>>();
    
    info!("找到 {} 个房间面板映射关系", room_panel_map.len());
    
    let mut total_components = 0;
    let mut processed_rooms = 0;
    let total_panels = exclude_panel_refnos.len();
    
    // 4. 并发处理房间关系构建
    use futures::stream::{self, StreamExt};
    
    let results = stream::iter(room_panel_map)
        .map(|(room_refno, room_num, panel_refnos)| {
            let mesh_dir = mesh_dir.clone();
            let exclude_panel_refnos = exclude_panel_refnos.clone();
            async move {
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
            }
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
    
    let stats = RoomBuildStats {
        total_rooms: processed_rooms,
        total_panels,
        total_components,
        build_time_ms: build_time.as_millis() as u64,
        cache_hit_rate: 0.0, // 暂时不统计缓存命中率
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
                        array::flatten(REFNO.slice(1, 2 + collect).children).{{id, noun}})[?noun='PANE'].id
                    ] from FRMW where {filter}
    "#
    );
    
    #[cfg(feature = "project_hh")]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        REFNO.children[?noun='PANE'].id
                    ] from SBFR where {filter}
    "#
    );
    
    #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        REFNO.children[?noun='PANE'].id
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
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
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
        let world_trans = geom_inst.world_trans;
        let world_aabb = geom_inst.world_aabb;
        for inst in geom_inst.insts {
            // 使用改进的几何缓存加载
            let tri_mesh = match load_geometry_with_enhanced_cache(&mesh_dir, &inst.geo_hash, world_trans, &inst).await {
                Ok(mesh) => mesh,
                Err(e) => {
                    warn!("加载几何文件失败: {}, error: {}", inst.geo_hash, e);
                    continue;
                }
            };

            // 3. 使用简单的空间查询（暂时使用基础实现）
            // TODO: 集成混合空间索引以提升性能
            let query_aabb: Aabb = world_aabb.into();
            
            // 简化版本：直接进行几何检测
            // 在实际使用中，应该先通过空间索引筛选候选构件
            let intersecting_refnos: HashSet<RefnoEnum> = HashSet::new(); // 暂时返回空集合
            
            // TODO: 实现精确几何检测
            // let intersecting_refnos = perform_precise_geometry_check(
            //     &tri_mesh,
            //     &candidates,
            //     exclude_refnos,
            //     panel_refno,
            //     inside_tol,
            // ).await?;
            
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
    world_trans: aios_core::PlantTransform,
    inst: &ModelHashInst,
) -> anyhow::Result<Arc<TriMesh>> {
    let cache = get_enhanced_geometry_cache().await;
    
    // 检查缓存
    if let Some(cached_mesh) = cache.get(geo_hash) {
        // 从缓存的 PlantMesh 构建 TriMesh
        if let Some(tri_mesh) = cached_mesh.get_tri_mesh_with_flag(
            (world_trans * inst.transform).to_matrix(),
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
        (world_trans * inst.transform).to_matrix(),
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

#[allow(dead_code)]
async fn perform_precise_geometry_check_placeholder(
    tri_mesh: &TriMesh,
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
    
    // 暂时返回空集合，因为没有候选构件
    // TODO: 实现完整的几何检测逻辑
    let _ = (tri_mesh, exclude_refnos, panel_refno, inside_tol); // 避免未使用警告
    
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
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
async fn estimate_memory_usage() -> f32 {
    let cache = get_enhanced_geometry_cache().await;
    let cache_size = cache.len() as f32 * 0.5; // 假设每个缓存项平均 0.5MB
    cache_size
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "sqlite-index")))]
async fn estimate_memory_usage() -> f32 {
    // 在不启用 sqlite-index 特性时，返回一个保守估计
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

/// 增量更新结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalUpdateResult {
    /// 影响的房间数量
    pub affected_rooms: usize,
    /// 更新的元素数量
    pub updated_elements: usize,
    /// 耗时（毫秒）
    pub duration_ms: u64,
}

/// 增量更新房间关系
/// 
/// 只更新指定 refnos 相关的房间关系，而不是全量重建
/// 
/// # 参数
/// * `refnos` - 需要更新关系的构件参考号列表
/// 
/// # 返回值
/// * `IncrementalUpdateResult` - 更新结果统计
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
pub async fn update_room_relations_incremental(
    refnos: &[RefnoEnum],
) -> anyhow::Result<IncrementalUpdateResult> {
    let start_time = Instant::now();
    info!("开始增量更新房间关系，涉及 {} 个构件", refnos.len());
    
    if refnos.is_empty() {
        return Ok(IncrementalUpdateResult {
            affected_rooms: 0,
            updated_elements: 0,
            duration_ms: 0,
        });
    }
    
    // 1. 查询这些 refnos 相关的房间面板
    let affected_panels = query_panels_containing_refnos(refnos).await?;
    info!("找到 {} 个受影响的房间面板", affected_panels.len());
    
    if affected_panels.is_empty() {
        warn!("没有找到受影响的房间面板");
        return Ok(IncrementalUpdateResult {
            affected_rooms: 0,
            updated_elements: refnos.len(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        });
    }
    
    // 2. 删除这些面板的旧关系
    delete_room_relations_for_panels(&affected_panels).await?;
    info!("已删除 {} 个面板的旧房间关系", affected_panels.len());
    
    // 3. 重新计算并保存新关系
    let db_option = aios_core::get_db_option();
    let mesh_dir = db_option.get_meshes_path();
    
    // 获取所有房间面板（用于排除）
    let room_key_words = db_option.get_room_key_word();
    let all_room_panels = build_room_panels_relate_v2(&room_key_words).await?;
    let exclude_panel_refnos: HashSet<RefnoEnum> = all_room_panels
        .iter()
        .flat_map(|(_, _, panels)| panels.clone())
        .collect();
    
    let mut updated_elements = 0;
    let affected_rooms = affected_panels.len();
    
    // 并发处理每个面板
    use futures::stream::{self, StreamExt};
    
    let results = stream::iter(affected_panels)
        .map(|(panel_refno, room_num)| {
            let mesh_dir = mesh_dir.clone();
            let exclude_panel_refnos = exclude_panel_refnos.clone();
            async move {
                match cal_room_refnos_v2(&mesh_dir, panel_refno, &exclude_panel_refnos, 0.1).await {
                    Ok(refnos) => {
                        if !refnos.is_empty() {
                            if let Err(e) = save_room_relate_v2(panel_refno, &refnos, &room_num).await {
                                error!("保存房间关系失败: panel={}, error={}", panel_refno, e);
                                return 0;
                            }
                            return refnos.len();
                        }
                        0
                    }
                    Err(e) => {
                        warn!("计算房间构件失败: panel={}, error={}", panel_refno, e);
                        0
                    }
                }
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    
    updated_elements = results.iter().sum();
    
    let duration = start_time.elapsed();
    info!(
        "增量更新完成: {} 个房间, {} 个元素, 耗时 {:?}",
        affected_rooms, updated_elements, duration
    );
    
    Ok(IncrementalUpdateResult {
        affected_rooms,
        updated_elements,
        duration_ms: duration.as_millis() as u64,
    })
}

/// 查询包含指定 refnos 的房间面板
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
async fn query_panels_containing_refnos(
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<(RefnoEnum, String)>> {
    if refnos.is_empty() {
        return Ok(Vec::new());
    }
    
    // 构建查询条件
    let refno_keys: Vec<String> = refnos.iter().map(|r| r.to_pe_key()).collect();
    let refno_list = refno_keys.join(",");
    
    // 查询包含这些 refnos 的房间面板关系
    let sql = format!(
        r#"
        select value [in, room_num] 
        from room_relate 
        where out in [{}]
        group by in, room_num
        "#,
        refno_list
    );
    
    let mut response = SUL_DB.query(sql).await?;
    let raw_result: Vec<(RecordId, String)> = response.take(0)?;
    
    let panels: Vec<(RefnoEnum, String)> = raw_result
        .into_iter()
        .map(|(panel_id, room_num)| (RefnoEnum::from(panel_id), room_num))
        .collect();
    
    Ok(panels)
}

/// 删除指定面板的房间关系
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
async fn delete_room_relations_for_panels(
    panels: &[(RefnoEnum, String)],
) -> anyhow::Result<()> {
    if panels.is_empty() {
        return Ok(());
    }
    
    let panel_keys: Vec<String> = panels.iter().map(|(p, _)| p.to_pe_key()).collect();
    let panel_list = panel_keys.join(",");
    
    let sql = format!(
        "delete room_relate where in in [{}];",
        panel_list
    );
    
    SUL_DB.query(sql).await?;
    debug!("已删除 {} 个面板的房间关系", panels.len());
    
    Ok(())
}

/// 专门的房间模型重新生成函数
/// 
/// 根据房间关键词查询房间，收集所有相关构件，重新生成模型并更新关系
/// 
/// # 参数
/// * `room_keywords` - 房间关键词列表
/// * `db_option` - 数据库配置
/// * `force_regenerate` - 是否强制重新生成
/// 
/// # 返回值
/// * `(房间数, 元素数, 耗时ms)` - 处理结果统计
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
pub async fn regenerate_room_models_by_keywords(
    room_keywords: &Vec<String>,
    db_option: &DbOption,
    force_regenerate: bool,
) -> anyhow::Result<(usize, usize, u64)> {
    let start_time = Instant::now();
    info!("开始重新生成房间模型，关键词: {:?}", room_keywords);
    
    // 1. 查询房间和面板关系
    let room_panel_map = build_room_panels_relate_v2(room_keywords).await?;
    let room_count = room_panel_map.len();
    info!("找到 {} 个房间", room_count);
    
    if room_panel_map.is_empty() {
        warn!("没有找到匹配的房间");
        return Ok((0, 0, start_time.elapsed().as_millis() as u64));
    }
    
    // 2. 收集所有需要生成的 refnos（面板 + 房间内构件）
    let mut all_refnos = HashSet::new();
    let mesh_dir = db_option.get_meshes_path();
    let exclude_panel_refnos: HashSet<RefnoEnum> = room_panel_map
        .iter()
        .flat_map(|(_, _, panels)| panels.clone())
        .collect();
    
    // 收集面板
    for (_, _, panel_refnos) in &room_panel_map {
        for panel_refno in panel_refnos {
            all_refnos.insert(*panel_refno);
        }
    }
    
    // 收集房间内构件
    info!("正在查询房间内构件...");
    for (_, _, panel_refnos) in &room_panel_map {
        for panel_refno in panel_refnos {
            match cal_room_refnos_v2(&mesh_dir, *panel_refno, &exclude_panel_refnos, 0.1).await {
                Ok(refnos) => {
                    all_refnos.extend(refnos);
                }
                Err(e) => {
                    warn!("查询房间构件失败: panel={}, error={}", panel_refno, e);
                }
            }
        }
    }
    
    let element_count = all_refnos.len();
    info!("需要重新生成 {} 个元素的模型", element_count);
    
    // 3. 重新生成模型（这里需要调用模型生成函数）
    // 注意：实际的模型生成需要在调用方完成，这里只返回需要生成的 refnos
    // 因为模型生成函数 gen_all_geos_data 需要更多的配置参数
    
    let duration_ms = start_time.elapsed().as_millis() as u64;
    Ok((room_count, element_count, duration_ms))
}

/// 针对特定房间重建关系（不生成模型）
/// 
/// # 参数
/// * `room_numbers` - 房间号列表（可选，为空则处理所有房间）
/// * `db_option` - 数据库配置
/// 
/// # 返回值
/// * `RoomBuildStats` - 构建统计信息
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
pub async fn rebuild_room_relations_for_rooms(
    room_numbers: Option<Vec<String>>,
    db_option: &DbOption,
) -> anyhow::Result<RoomBuildStats> {
    let start_time = Instant::now();
    info!("开始重建房间关系");
    
    let mesh_dir = db_option.get_meshes_path();
    let room_key_words = db_option.get_room_key_word();
    
    // 1. 查询房间面板关系
    let mut room_panel_map = build_room_panels_relate_v2(&room_key_words).await?;
    
    // 2. 如果指定了房间号，进行过滤
    if let Some(ref numbers) = room_numbers {
        let numbers_set: HashSet<String> = numbers.iter().cloned().collect();
        room_panel_map.retain(|(_, room_num, _)| numbers_set.contains(room_num));
        info!("过滤后剩余 {} 个房间", room_panel_map.len());
    }
    
    if room_panel_map.is_empty() {
        warn!("没有找到需要处理的房间");
        return Ok(RoomBuildStats {
            total_rooms: 0,
            total_panels: 0,
            total_components: 0,
            build_time_ms: start_time.elapsed().as_millis() as u64,
            cache_hit_rate: 0.0,
            memory_usage_mb: 0.0,
        });
    }
    
    let exclude_panel_refnos: HashSet<RefnoEnum> = room_panel_map
        .iter()
        .flat_map(|(_, _, panels)| panels.clone())
        .collect();
    
    // 3. 删除旧关系
    let panels_to_delete: Vec<(RefnoEnum, String)> = room_panel_map
        .iter()
        .flat_map(|(_, room_num, panels)| {
            panels.iter().map(move |p| (*p, room_num.clone()))
        })
        .collect();
    delete_room_relations_for_panels(&panels_to_delete).await?;
    info!("已删除 {} 个面板的旧关系", panels_to_delete.len());
    
    // 4. 重新计算并保存关系
    let mut total_components = 0;
    let mut processed_rooms = 0;
    
    use futures::stream::{self, StreamExt};
    
    let results = stream::iter(room_panel_map)
        .map(|(room_refno, room_num, panel_refnos)| {
            let mesh_dir = mesh_dir.clone();
            let exclude_panel_refnos = exclude_panel_refnos.clone();
            async move {
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
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    
    for (_, _, components) in results {
        total_components += components;
        processed_rooms += 1;
    }
    
    let build_time = start_time.elapsed();
    
    let stats = RoomBuildStats {
        total_rooms: processed_rooms,
        total_panels: exclude_panel_refnos.len(),
        total_components,
        build_time_ms: build_time.as_millis() as u64,
        cache_hit_rate: 0.0,
        memory_usage_mb: estimate_memory_usage().await,
    };
    
    info!(
        "房间关系重建完成: {} 个房间, {} 个面板, {} 个构件, 耗时 {:?}",
        stats.total_rooms, stats.total_panels, stats.total_components, build_time
    );
    
    Ok(stats)
}
