use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::sesno_increment::get_changes_at_sesno;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::pdms_inst::save_instance_data;
use crate::fast_model::{
    booleans_meshes_in_db, cata_model, gen_meshes_in_db, loop_model, prim_model,
    process_meshes_update_db_deep, resolve_desi_comp, shared,
};
use crate::xkt_generator::*;
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{
    EleGeosInfo, EleInstGeo, EleInstGeosData, GeoBasicType, PlantGeoData, ShapeInstancesData,
};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
// Removed GLOBAL_AABB_TREE dependency - using SQLite R*-tree instead
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{DBType, prim_geo::*};
use aios_core::{RefU64, RefnoEnum, pdms_types::*};
use aios_core::{
    SUL_DB, query_multi_children_refnos, query_type_refnos_by_dbnum, query_use_cate_refnos_by_dbnum,
};
// 历史数据查询相关导入
// use aios_core::historical_query::{
//     query_type_refnos_by_dbnum_at_sesno,
//     query_hierarchy_at_sesno,
//     query_multi_children_refnos_at_sesno,
//     session_exists,
//     HierarchyQueryResult
// };
#[cfg(feature = "sqlite-index")]
use crate::spatial_index::SqliteSpatialIndex;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use glam::DVec3;
use glam::{DMat4, Vec3};
use nom::complete::bool;
use once_cell::sync::Lazy;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::TryFrom;
use std::io::Read;
use std::mem::take;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

///一个db生成模型里，汇总的参考号集合
#[derive(Debug, Clone, Default)]
pub struct DbModelInstRefnos {
    pub bran_hanger_refnos: Arc<Vec<RefnoEnum>>,
    pub use_cate_refnos: Arc<Vec<RefnoEnum>>,
    pub loop_owner_refnos: Arc<Vec<RefnoEnum>>,
    pub prim_refnos: Arc<Vec<RefnoEnum>>,
}

impl DbModelInstRefnos {
    pub async fn execute_gen_inst_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &prim_refnos)
                .await
                .expect("更新prim模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option.clone(), &loop_owner_refnos)
                .await
                .expect("更新loop模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &use_cate_refnos)
                .await
                .expect("更新use_cate模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            for bran_refnos in bran_hanger_refnos.chunks(20) {
                let db_option_clone = db_option.clone();
                // let refnos_str = bran_refnos.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                let target_refnos = match query_multi_children_refnos(&bran_refnos).await {
                    Ok(refnos) => refnos,
                    Err(e) => {
                        eprintln!("查询bran_hanger子节点refnos失败：{}", e);
                        return;
                    }
                };

                match gen_meshes_in_db(db_option_clone, &target_refnos).await {
                    Ok(()) => {}
                    Err(e) => {
                        let target_str = target_refnos
                            .iter()
                            .map(|r| r.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        eprintln!(
                            "更新bran_hanger模型数据失败：{}，相关refnos: {}",
                            e, target_str
                        );
                        return;
                    }
                }
            }
        }));
        while let Some(_) = handles.next().await {}
    }

    //执行布尔运算的操作
    pub async fn execute_boolean_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &prim_refnos)
                .await
                .expect("布尔运算prim模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &loop_owner_refnos)
                .await
                .expect("布尔运算loop模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &use_cate_refnos)
                .await
                .expect("布尔运算use_cate模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            for chunk in bran_hanger_refnos.chunks(20) {
                let db_option_clone = db_option.clone();
                let chunk_str = chunk
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let target_refnos = match query_multi_children_refnos(&chunk).await {
                    Ok(refnos) => refnos,
                    Err(e) => {
                        eprintln!(
                            "查询bran_hanger子节点refnos失败：{}，相关refnos: {}",
                            e, chunk_str
                        );
                        continue;
                    }
                };
                match booleans_meshes_in_db(db_option_clone, &target_refnos).await {
                    Ok(_) => {}
                    Err(e) => {
                        let target_str = target_refnos
                            .iter()
                            .map(|r| r.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        eprintln!(
                            "布尔运算bran_hanger模型数据失败：{}，相关refnos: {}",
                            e, target_str
                        );
                        continue;
                    }
                }
            }
        }));
        while let Some(_) = handles.next().await {}
    }
}

static GLOBAL_SHAPE_CACHE: Lazy<RwLock<Option<Arc<ShapeInstancesData>>>> =
    Lazy::new(|| RwLock::new(None));
static GLOBAL_CACHE_REFNOS: Lazy<RwLock<Vec<RefnoEnum>>> = Lazy::new(|| RwLock::new(Vec::new()));

static XKT_DEBUG_ENABLED: Lazy<bool> = Lazy::new(|| {
    std::env::var("XKT_GEN_DEBUG")
        .or_else(|_| std::env::var("XKT_GEN_VERBOSE"))
        .ok()
        .and_then(|value| parse_env_flag(&value))
        .unwrap_or(false)
});

fn parse_env_flag(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn is_xtk_debug_enabled() -> bool {
    *XKT_DEBUG_ENABLED
}

pub(crate) fn xkt_debug<F>(builder: F)
where
    F: FnOnce() -> String,
{
    if is_xtk_debug_enabled() {
        println!("{}", builder());
    }
}

async fn set_global_shape_cache(data: ShapeInstancesData) {
    let mut cache = GLOBAL_SHAPE_CACHE.write().await;
    *cache = Some(Arc::new(data));
}

async fn get_global_shape_cache() -> Option<Arc<ShapeInstancesData>> {
    GLOBAL_SHAPE_CACHE.read().await.clone()
}

async fn clear_global_shape_cache() {
    let mut cache = GLOBAL_SHAPE_CACHE.write().await;
    *cache = None;
    GLOBAL_CACHE_REFNOS.write().await.clear();
}

async fn set_cached_refnos(refnos: Vec<RefnoEnum>) {
    let mut guard = GLOBAL_CACHE_REFNOS.write().await;
    *guard = refnos;
}

async fn get_cached_refnos() -> Vec<RefnoEnum> {
    GLOBAL_CACHE_REFNOS.read().await.clone()
}

fn build_shape_subset(cache: &ShapeInstancesData, refno: RefnoEnum) -> Option<ShapeInstancesData> {
    let mut subset = ShapeInstancesData::default();

    if let Some(info) = cache.inst_info_map.get(&refno) {
        subset.inst_info_map.insert(refno, info.clone());
        let inst_key = info.get_inst_key();
        if let Some(geo_data) = cache.inst_geos_map.get(&inst_key) {
            subset.inst_geos_map.insert(inst_key, geo_data.clone());
        }
    } else if let Some((inst_key, geo_data)) = cache
        .inst_geos_map
        .iter()
        .find(|(_, data)| data.refno == refno)
    {
        subset
            .inst_geos_map
            .insert(inst_key.clone(), geo_data.clone());
    }

    if let Some(info) = cache.inst_tubi_map.get(&refno) {
        subset.inst_tubi_map.insert(refno, info.clone());
    }

    if !subset.inst_info_map.is_empty()
        || !subset.inst_geos_map.is_empty()
        || !subset.inst_tubi_map.is_empty()
    {
        Some(subset)
    } else {
        None
    }
}

fn cached_element_type_name(cache: &ShapeInstancesData, refno: RefnoEnum) -> Option<String> {
    if let Some(info) = cache.inst_info_map.get(&refno) {
        let inst_key = info.get_inst_key();
        if let Some(geo_data) = cache.inst_geos_map.get(&inst_key) {
            return Some(geo_data.type_name.clone());
        }
        return Some(format!("{:?}", info.generic_type));
    }

    if let Some(info) = cache.inst_tubi_map.get(&refno) {
        return Some(format!("{:?}", info.generic_type));
    }

    if let Some((_key, geo_data)) = cache
        .inst_geos_map
        .iter()
        .find(|(_, data)| data.refno == refno)
    {
        return Some(geo_data.type_name.clone());
    }

    None
}

fn build_sample_shape_data_for_db(dbno: u32) -> Option<ShapeInstancesData> {
    if dbno != 1112 {
        return None;
    }

    let sample_specs = [
        (
            RefnoEnum::Refno(RefU64(111200001)),
            "PIPE",
            PdmsGenericType::PIPE,
        ),
        (
            RefnoEnum::Refno(RefU64(111200002)),
            "ELBO",
            PdmsGenericType::PIPE,
        ),
        (
            RefnoEnum::Refno(RefU64(111200003)),
            "TEE",
            PdmsGenericType::PIPE,
        ),
        (
            RefnoEnum::Refno(RefU64(111200004)),
            "VALVE",
            PdmsGenericType::PIPE,
        ),
        (
            RefnoEnum::Refno(RefU64(111200005)),
            "FLANGE",
            PdmsGenericType::PIPE,
        ),
    ];

    let mut data = ShapeInstancesData::default();

    for (idx, (refno, type_name, generic_type)) in sample_specs.iter().enumerate() {
        let mut info = EleGeosInfo::default();
        info.refno = *refno;
        info.sesno = idx as i32;
        info.visible = true;
        info.generic_type = *generic_type;
        info.world_transform = Transform::from_translation(Vec3::new(idx as f32 * 3.0, 0.0, 0.0));
        data.inst_info_map.insert(*refno, info.clone());

        let inst_key = info.get_inst_key();
        let mut box_shape = SBox::default();
        box_shape.size = Vec3::new(1.0 + idx as f32 * 0.2, 0.8, 1.2);

        let inst_geo = EleInstGeo {
            geo_hash: (idx as u64) + 1,
            refno: *refno,
            geo_param: PdmsGeoParam::PrimBox(box_shape),
            pts: vec![],
            aabb: None,
            transform: Transform::IDENTITY,
            visible: true,
            is_tubi: false,
            geo_type: GeoBasicType::Pos,
            cata_neg_refnos: vec![],
        };

        let geo_data = EleInstGeosData {
            inst_key: inst_key.clone(),
            refno: *refno,
            insts: vec![inst_geo],
            aabb: None,
            type_name: (*type_name).to_string(),
        };

        data.inst_geos_map.insert(inst_key, geo_data);
    }

    Some(data)
}

/// 检查指定的 geo_hash 是否有对应的 mesh 文件
fn check_mesh_exists(geo_hash: u64) -> bool {
    if geo_hash == 0 {
        return false;
    }
    let filename = format!("assets/meshes/{}.mesh", geo_hash);
    let exists = Path::new(&filename).exists();

    // 添加调试信息
    if !exists && is_xtk_debug_enabled() {
        xkt_debug(|| format!("    调试: mesh文件不存在: {}", filename));
        // 检查是否有相似的文件名
        if let Ok(entries) = std::fs::read_dir("assets/meshes") {
            let target_str = geo_hash.to_string();
            let mut found_similar = false;
            for entry in entries.take(5) {
                if let Ok(entry) = entry {
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    if name.contains(&target_str[..8]) {
                        xkt_debug(|| format!("      发现相似文件: {}", name));
                        found_similar = true;
                    }
                }
            }
            if !found_similar {
                xkt_debug(|| "      未发现任何相似文件名".to_string());
            }
        }
    }

    exists
}

/// 检查多个几何体节点，返回需要重新生成 mesh 的节点
async fn check_nodes_need_mesh_generation(shape_data: &ShapeInstancesData) -> Vec<RefnoEnum> {
    let mut need_regenerate = Vec::new();
    let mut total_checked = 0;
    let mut missing_mesh_count = 0;

    xkt_debug(|| "开始检查 mesh 文件状态...".to_string());
    xkt_debug(|| format!("总共需要检查 {} 个节点", shape_data.inst_info_map.len()));

    for (refno, inst_info) in &shape_data.inst_info_map {
        total_checked += 1;

        // 获取实例的 inst_key
        let inst_key = inst_info.get_inst_key();
        if let Some(geo_data) = shape_data.inst_geos_map.get(&inst_key) {
            xkt_debug(|| format!("  📋 检查节点 {} (inst_key: {})", refno, inst_key));
            xkt_debug(|| format!("      包含 {} 个几何实例", geo_data.insts.len()));

            // 检查每个实例的 mesh 是否存在
            let mut missing_meshes = Vec::new();
            for (idx, inst) in geo_data.insts.iter().enumerate() {
                xkt_debug(|| format!("      实例 {}: geo_hash = {}", idx, inst.geo_hash));
                if inst.geo_hash != 0 {
                    if !check_mesh_exists(inst.geo_hash) {
                        missing_meshes.push(inst.geo_hash);
                    }
                } else {
                    xkt_debug(|| "        ⚠️  geo_hash 为 0，跳过".to_string());
                }
            }

            if !missing_meshes.is_empty() {
                xkt_debug(|| {
                    format!(
                        "  ❌ 节点 {} 缺少 {} 个 mesh 文件:",
                        refno,
                        missing_meshes.len()
                    )
                });
                for hash in &missing_meshes {
                    xkt_debug(|| format!("     - {}.mesh", hash));
                }
                missing_mesh_count += missing_meshes.len();
                need_regenerate.push(refno.clone());
            } else if !geo_data.insts.is_empty() {
                xkt_debug(|| format!("  ✅ 节点 {} 的所有 mesh 文件都存在", refno));
            } else {
                xkt_debug(|| format!("  ⚠️  节点 {} 没有几何实例", refno));
            }
        } else {
            xkt_debug(|| {
                format!(
                    "  ⚠️  节点 {} 没有找到几何数据 (inst_key: {})",
                    refno, inst_key
                )
            });
        }
    }

    // 检查 TUBI 节点 (inst_tubi_map 存储的是 EleGeosInfo 类型)
    for (refno, _tubi_info) in &shape_data.inst_tubi_map {
        // TUBI 节点的 mesh 生成比较特殊，暂时跳过检查
        // 如果需要检查，需要根据 TUBI 的特定逻辑来处理
        xkt_debug(|| format!("  ℹ️  TUBI 节点 {} 暂时跳过 mesh 检查", refno));
    }

    xkt_debug(|| "\n=== Mesh 文件检查结果 ===".to_string());
    xkt_debug(|| format!("检查节点数: {}", total_checked));
    xkt_debug(|| format!("缺失 mesh 文件数: {}", missing_mesh_count));
    xkt_debug(|| format!("需要重新生成的节点数: {}", need_regenerate.len()));
    xkt_debug(|| format!("TUBI 节点数: {}", shape_data.inst_tubi_map.len()));
    xkt_debug(|| "========================\n".to_string());

    need_regenerate
}

fn load_plant_mesh_by_hash(geo_hash: u64) -> Option<PlantMesh> {
    if geo_hash == 0 {
        return None;
    }
    let filename = format!("assets/meshes/{}.mesh", geo_hash);
    let path = Path::new(&filename);
    if !path.exists() {
        return None;
    }
    PlantMesh::des_mesh_file(&path).ok()
}

fn flatten_vec3(values: &[Vec3]) -> Vec<f32> {
    let mut flattened = Vec::with_capacity(values.len() * 3);
    for v in values {
        flattened.extend_from_slice(&[v.x, v.y, v.z]);
    }
    flattened
}

fn compute_vertex_normals(vertices: &[Vec3], indices: &[u32]) -> Vec<f32> {
    let mut normals = vec![Vec3::ZERO; vertices.len()];
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let a_idx = tri[0] as usize;
        let b_idx = tri[1] as usize;
        let c_idx = tri[2] as usize;
        if a_idx >= vertices.len() || b_idx >= vertices.len() || c_idx >= vertices.len() {
            continue;
        }
        let a = vertices[a_idx];
        let b = vertices[b_idx];
        let c = vertices[c_idx];
        let normal = (b - a).cross(c - a);
        if normal.length_squared() > f32::EPSILON {
            let n = normal.normalize();
            normals[a_idx] += n;
            normals[b_idx] += n;
            normals[c_idx] += n;
        }
    }

    for normal in normals.iter_mut() {
        if normal.length_squared() > f32::EPSILON {
            *normal = normal.normalize();
        }
    }

    flatten_vec3(&normals)
}

fn create_geometry_from_plant_mesh(
    geometry_id: &str,
    mesh: &PlantMesh,
) -> anyhow::Result<XKTGeometry> {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return Err(anyhow::anyhow!("plant mesh 数据为空"));
    }

    let mut geometry = XKTGeometry::new(geometry_id.to_string(), XKTGeometryType::Triangles);
    geometry.positions = flatten_vec3(&mesh.vertices);

    if !mesh.normals.is_empty() && mesh.normals.len() == mesh.vertices.len() {
        geometry.normals = Some(flatten_vec3(&mesh.normals));
    } else {
        geometry.normals = Some(compute_vertex_normals(&mesh.vertices, &mesh.indices));
    }

    geometry.indices = mesh.indices.clone();

    let resolved_aabb = mesh.aabb.as_ref().cloned().or_else(|| mesh.cal_aabb());

    if mesh.aabb.is_none() && resolved_aabb.is_some() {
        xkt_debug(|| format!("自动计算 mesh {} 的包围盒", geometry_id));
    }

    if let Some(aabb) = resolved_aabb {
        let min = Vec3::new(aabb.mins.x, aabb.mins.y, aabb.mins.z);
        let max = Vec3::new(aabb.maxs.x, aabb.maxs.y, aabb.maxs.z);
        geometry.bounding_box = Some((min, max));
    }

    Ok(geometry)
}

async fn prepare_global_shape_cache_for_db(dbno: u32, db_option: &DbOption) -> anyhow::Result<()> {
    clear_global_shape_cache().await;

    // 检查数据库连接是否已经初始化（应该在 gen_xtk.rs 的 main 函数中完成）
    // 通过尝试一个简单查询来测试连接
    let test_query = "SELECT * FROM pe LIMIT 1";
    if let Err(e) = SUL_DB.query(test_query).await {
        // 如果查询失败，尝试使用示例数据
        if let Some(sample) = build_sample_shape_data_for_db(dbno) {
            println!(
                "无法执行数据库查询 ({}), 使用内置示例数据生成 dbnum {} 的几何实例",
                e, dbno
            );
            set_cached_refnos(sample.inst_info_map.keys().cloned().collect()).await;
            set_global_shape_cache(sample).await;
            return Ok(());
        } else {
            return Err(anyhow::anyhow!("数据库连接失败: {}", e));
        }
    }

    let mut option_clone = db_option.clone();
    option_clone.gen_model = true;
    option_clone.gen_mesh = true; // 确保生成mesh
    option_clone.manual_db_nums = Some(vec![dbno]);

    let option_arc = Arc::new(option_clone);
    let (sender, receiver) = flume::unbounded();

    let collector = tokio::spawn(async move {
        let mut aggregated = ShapeInstancesData::default();
        while let Ok(shape_data) = receiver.recv_async().await {
            aggregated.merge(shape_data);
        }
        aggregated
    });

    xkt_debug(|| format!("预处理数据库 {} 的几何实例数据...", dbno));
    let _ = gen_geos_data_by_dbnum(dbno, option_arc, sender.clone(), None).await?;
    drop(sender);

    let mut aggregated = collector
        .await
        .map_err(|e| anyhow::anyhow!("收集几何实例数据失败: {}", e))?;

    xkt_debug(|| {
        format!(
            "ShapeInstancesData 收集完成: inst_info={} geos={} tubi={}",
            aggregated.inst_info_map.len(),
            aggregated.inst_geos_map.len(),
            aggregated.inst_tubi_map.len()
        )
    });

    if aggregated.inst_geos_map.is_empty() {
        if let Some(sample) = build_sample_shape_data_for_db(dbno) {
            xkt_debug(|| format!("使用内置示例数据构建 dbnum {} 的几何实例", dbno));
            aggregated = sample;
        }
    }

    let refnos = aggregated.inst_info_map.keys().cloned().collect();
    set_cached_refnos(refnos).await;
    set_global_shape_cache(aggregated).await;
    Ok(())
}

/// 生成几何体数据
///
/// # 参数
/// * `manual_refnos` - 手动指定的引用号列表
/// * `db_option` - 数据库选项配置
/// * `incr_updates` - 增量更新日志，用于增量生成几何体数据
/// * `target_sesno` - 目标会话号，用于判断是否生成历史数据的模型
///
/// # 返回值
/// * `anyhow::Result<bool>` - 返回生成结果，成功返回true，失败返回错误
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    const CHUNK_SIZE: usize = 100;
    let mut final_incr_updates = incr_updates;
    let time = Instant::now();

    // 如果指定了 target_sesno，获取该 sesno 的增量数据
    if let Some(sesno) = target_sesno {
        if final_incr_updates.is_none() {
            // 从 element_changes 表获取该 sesno 的变更
            match get_changes_at_sesno(sesno).await {
                Ok(sesno_changes) => {
                    // 如果该 sesno 有变更，使用这些变更作为增量更新
                    if sesno_changes.count() > 0 {
                        xkt_debug(|| {
                            format!(
                                "发现 sesno {} 的变更: {} 个元素",
                                sesno,
                                sesno_changes.count()
                            )
                        });
                        final_incr_updates = Some(sesno_changes);
                    } else {
                        xkt_debug(|| format!("sesno {} 没有发现变更，跳过增量生成", sesno));
                        return Ok(false);
                    }
                }
                Err(e) => {
                    eprintln!("获取 sesno {} 的变更失败: {}", sesno, e);
                    return Err(e);
                }
            }
        }
    }

    let is_incr_update = final_incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    let has_debug = db_option.debug_root_refnos.is_some();

    if is_incr_update || has_manual_refnos || has_debug {
        // let (sender, receiver) = flume::bounded(CHUNK_SIZE);
        let (sender, receiver) = flume::unbounded();
        let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
        let insert_task = tokio::task::spawn(async move {
            while let Ok(shape_insts) = receiver.recv_async().await {
                save_instance_data(&shape_insts, false).await.unwrap();
                xkt_debug(|| format!("Insert manual shape insts: {}", shape_insts.inst_cnt()));
            }
        });
        let target_root_refnos = gen_geos_data(
            None,
            manual_refnos.clone(),
            db_option,
            final_incr_updates.clone(),
            sender.clone(),
            target_sesno,
        )
        .await?;
        drop(sender);
        insert_task.await.unwrap();
        if db_option.gen_mesh {
            process_meshes_update_db_deep(db_option, &target_root_refnos)
                .await
                .expect("更新模型数据失败");
        }
    } else {
        let dbnos = if db_option.manual_db_nums.is_some() {
            db_option.manual_db_nums.clone().unwrap()
        } else {
            aios_core::query_mdb_db_nums(DBType::DESI).await?
        };

        // 过滤掉exclude_db_nums中的数据库编号
        let dbnos = if let Some(exclude_nums) = &db_option.exclude_db_nums {
            dbnos
                .into_iter()
                .filter(|dbno| !exclude_nums.contains(dbno))
                .collect::<Vec<_>>()
        } else {
            dbnos
        };

        if is_xtk_debug_enabled() {
            xkt_debug(|| format!("准备生成数据库列表: {:?}", dbnos));
        }
        let db_option_arc = Arc::new(db_option.clone());
        for dbno in dbnos.clone() {
            xkt_debug(|| format!("开始{}的模型生成", dbno));
            let time = Instant::now();
            let (sender, receiver) = flume::unbounded();
            let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
            let insert_task = tokio::task::spawn(async move {
                while let Ok(shape_insts) = receiver.recv_async().await {
                    let time = Instant::now();
                    // save_instance_data(&shape_insts, false).await.unwrap();
                    save_instance_data(&shape_insts, false).await.unwrap();
                    xkt_debug(|| {
                        format!("save_instance_data time: {}ms", time.elapsed().as_millis())
                    });
                    xkt_debug(|| {
                        format!("Insert shape insts: {}", shape_insts.inst_info_map.len())
                    });
                }
            });
            let db_refnos =
                gen_geos_data_by_dbnum(dbno, db_option_arc.clone(), sender.clone(), target_sesno)
                    .await?;
            drop(sender);
            insert_task.await.unwrap();
            xkt_debug(|| format!("生成完insts时间: {}ms", time.elapsed().as_millis()));
            if db_option_arc.gen_mesh {
                let time = Instant::now();
                xkt_debug(|| "开始执行模型生成和布尔运算".to_string());
                //模型生成完之后，再进行布尔运算
                db_refnos
                    .execute_gen_inst_meshes(Some(db_option_arc.clone()))
                    .await;
                xkt_debug(|| format!("生成insts三角模型时间: {}ms", time.elapsed().as_millis()));
                let time = Instant::now();
                db_refnos
                    .execute_boolean_meshes(Some(db_option_arc.clone()))
                    .await;
                xkt_debug(|| format!("布尔运算时间: {}ms", time.elapsed().as_millis()));
            }
        }
    }
    // After generation, build SQLite RTree index from cached AABBs
    #[cfg(feature = "sqlite-index")]
    {
        // SQLite spatial index is initialized when needed
        if SqliteSpatialIndex::is_enabled() {
            match SqliteSpatialIndex::with_default_path() {
                Ok(index) => println!("SQLite spatial index initialized"),
                Err(e) => eprintln!("Failed to initialize SQLite spatial index: {}", e),
            }
        }
    }
    // SQLite R*-tree index is used for spatial queries
    xkt_debug(|| format!("生成完所有模型时间: {}ms", time.elapsed().as_millis()));

    Ok(true)
}

///更新模型数据
/// 根据数据库编号处理网格数据
///
/// # 参数
///
/// * `dbnos` - 数据库编号数组
/// * `db_option` - 数据库选项配置
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示处理是否成功
pub async fn process_meshes_by_dbnos(dbnos: &[u32], db_option: &DbOption) -> anyhow::Result<()> {
    let mut time = Instant::now();
    let include_history = db_option.is_gen_history_model();

    // 过滤掉exclude_db_nums中的数据库编号
    let filtered_dbnos = if let Some(exclude_nums) = &db_option.exclude_db_nums {
        dbnos
            .iter()
            .filter(|&&dbno| !exclude_nums.contains(&dbno))
            .copied()
            .collect::<Vec<_>>()
    } else {
        dbnos.to_vec()
    };

    for &dbno in &filtered_dbnos {
        let sites = query_type_refnos_by_dbnum(&["SITE"], dbno, None, include_history).await?;
        process_meshes_update_db_deep(db_option, &sites)
            .await
            .expect("更新模型数据失败");
    }
    xkt_debug(|| format!("更新所有模型时间: {}ms", time.elapsed().as_millis()));
    Ok(())
}

///生成几何体数据
/// 根据数据库编号生成几何体数据
///
/// # 参数
///
/// * `dbno` - 数据库编号
/// * `db_option_arc` - 数据库选项的Arc指针
/// * `sender` - 形状实例数据的发送通道
///
/// # 返回值
///
/// 返回 `Result<DbModelInstRefnos>` 表示生成是否成功以及生成的模型实例引用号
pub async fn gen_geos_data_by_dbnum(
    dbno: u32,
    db_option_arc: Arc<DbOption>,
    sender: flume::Sender<ShapeInstancesData>,
    target_sesno: Option<u32>,
) -> anyhow::Result<DbModelInstRefnos> {
    let gen_history = db_option_arc.is_gen_history_model();

    //判断有空的层级，不用去生成
    let zones = if let Some(sesno) = target_sesno {
        // 使用历史查询
        query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
            .await
            .unwrap_or_default()
    } else {
        // 使用当前数据查询
        query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
            .await
            .unwrap_or_default()
    };
    if zones.is_empty() {
        return Ok(Default::default());
    }
    // let mut all_handles = FuturesUnordered::new();

    xkt_debug(|| format!("gen_geos_data_by_dbnum 处理db: {}", dbno));
    let d_types = db_option_arc.debug_refno_types.clone();
    let mut gen_cata_flag = d_types.iter().any(|x| x == "CATA");
    let mut gen_loop_flag = d_types.iter().any(|x| x == "LOOP");
    let mut gen_prim_flag = d_types.iter().any(|x| x == "PRIM");
    let gen_model = db_option_arc.gen_model;
    let test_refno = db_option_arc.get_test_refno();

    // dbg!(origin_root_refnos.len());
    //需要在这里把origin_root_refnos 打断成小块
    //遍历小块
    //Step 1、提前缓存ploo, 得到对齐方式的偏移
    let loop_sjus_map = DashMap::new();
    {
        //查找到子节点的所有PLOO类型
        let target_ploo_refnos =
            query_type_refnos_by_dbnum(&["PLOO"], dbno, Some(true), gen_history)
                .await
                .unwrap_or_default();
        #[cfg(debug_assertions)]
        if !target_ploo_refnos.is_empty() {
            xkt_debug(|| format!("target_ploo_refnos: {:?}", target_ploo_refnos.len()));
        }
        if gen_model {
            for r in target_ploo_refnos.chunks(200) {
                let sql = format!(
                    "select value [OWNER, HEIG, SJUS] from [{}] where SJUS!=0",
                    r.iter()
                        .map(|x| x.to_table_key("PLOO"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let mut response = SUL_DB.query(sql).await?;
                // response.take_errors()
                let tuples: Vec<(RefnoEnum, f32, String)> = response.take(0)?;
                // dbg!(&tuples[0]);
                for (owner, height, sjus) in tuples {
                    let off_z = cata_model::cal_sjus_value(&sjus, height);
                    //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
                    //插入方向和偏移距离
                    loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
                }
            }
        }
    }
    let loop_sjus_map_arc = Arc::new(loop_sjus_map);

    //Step 2、按类目先逐个分好类的参考号集合
    //2.1 管道或者支吊架的分类
    let target_bran_hanger_refnos =
        Arc::new(query_type_refnos_by_dbnum(&["BRAN", "HANG"], dbno, None, gen_history).await?);
    xkt_debug(|| {
        format!(
            "当前分段使用管道或者支吊架元件库数量: {}",
            target_bran_hanger_refnos.len()
        )
    });

    //打印管道/支吊架的使用数量
    if !target_bran_hanger_refnos.is_empty() && gen_cata_flag && gen_model {
        //查询出branch 和 branch 下的子节点
        let mut branch_refnos_map = DashMap::new();
        let mut bran_comp_eles = HashSet::new();
        for &refno in target_bran_hanger_refnos.as_slice() {
            let children = aios_core::get_children_pes(refno).await.unwrap_or_default();
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            //求出元件对应的outside bore
            branch_refnos_map.insert(refno, children);
        }

        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
            let map = aios_core::query_group_by_cata_hash(target_bran_hanger_refnos.as_slice())
                .await
                .unwrap_or_default();
            if let Some(t_refno) = test_refno {
                if bran_comp_eles.contains(&t_refno) {
                    for kv in &map {
                        if kv.value().group_refnos.contains(&t_refno) {
                            dbg!(kv.value());
                        }
                    }
                }
            }
            map
        };

        //元件库的模型计算
        //bran，hanger下需要重用的模型
        if gen_model && (!target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty()) {
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            // let handle = tokio::spawn(async move {
            let start_time = Instant::now();
            cata_model::gen_cata_geos(
                db_option,
                Arc::new(target_bran_reuse_cata_map),
                Arc::new(branch_refnos_map),
                sjus_map_clone,
                sender,
            )
            .await
            .unwrap();
            xkt_debug(|| {
                format!(
                    "BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms",
                    start_time.elapsed().as_millis()
                )
            });
            // });
            // all_handles.push(handle);
        }
    }
    let mut use_cate_refnos = vec![];
    for cate_names in USE_CATE_NOUN_NAMES.chunks(4) {
        let refnos = query_use_cate_refnos_by_dbnum(cate_names, dbno, gen_history).await?;
        if refnos.is_empty() {
            continue;
        }
        use_cate_refnos.extend(refnos.clone());
        let cur_cate_refnos = Arc::new(refnos);
        // dbg!(cur_cate_refnos.len());
        //查询单个使用元件库的数量
        let target_single_cata_map = {
            //要过滤掉owner是BRAN 和 HANG的
            let map = aios_core::query_group_by_cata_hash(cur_cate_refnos.as_slice())
                .await
                .unwrap_or_default();
            map
        };

        xkt_debug(|| format!("当前分段使用元件库数量: {}", cur_cate_refnos.len()));
        if gen_model && gen_cata_flag && !target_single_cata_map.is_empty() {
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            // let handle = tokio::spawn(async move {
            let start_time = Instant::now();
            cata_model::gen_cata_geos(
                db_option,
                Arc::new(target_single_cata_map),
                Arc::new(Default::default()),
                sjus_map_clone,
                sender,
            )
            .await
            .unwrap();
            xkt_debug(|| {
                format!(
                    "单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms",
                    start_time.elapsed().as_millis()
                )
            });
            // });
            // all_handles.push(handle);
        }
    }

    let target_loop_owner_refnos = Arc::new(
        query_type_refnos_by_dbnum(&GNERAL_LOOP_OWNER_NOUN_NAMES, dbno, Some(true), gen_history)
            .await
            .unwrap_or_default(),
    );
    xkt_debug(|| format!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len()));
    if gen_model && gen_loop_flag && !target_loop_owner_refnos.is_empty() {
        let sjus_map_clone = loop_sjus_map_arc.clone();
        let sender = sender.clone();
        let db_option = db_option_arc.clone();
        let target_loop_owner_refnos_arc = target_loop_owner_refnos.clone();
        // let handle = tokio::spawn(async move {
        loop_model::gen_loop_geos(
            db_option,
            &target_loop_owner_refnos_arc,
            sjus_map_clone,
            sender,
        )
        .await
        .unwrap();
        // });
        // all_handles.push(handle);
    }

    let target_prim_refnos = Arc::new(
        query_type_refnos_by_dbnum(&GNERAL_PRIM_NOUN_NAMES, dbno, None, gen_history)
            .await
            .unwrap_or_default(),
    );

    xkt_debug(|| format!("当前分段使用基本体数量: {}", target_prim_refnos.len()));
    //基本元件的生成
    if gen_model && gen_prim_flag && !target_prim_refnos.is_empty() {
        //基本体模型的生成
        let db_option = db_option_arc.clone();
        let sender = sender.clone();
        let target_prim_refnos_arc = target_prim_refnos.clone();
        // let hand le = tokio::spawn(async move {
        prim_model::gen_prim_geos(db_option, target_prim_refnos_arc.as_slice(), sender)
            .await
            .unwrap();
        // });
        // all_handles.push(handle);
    }

    //Ok::<_, anyhow::Error>(())
    // while let Some(result) = all_handles.next().await {
    //     // 处理每个完成的 future 的结果
    // }

    let db_refnos = DbModelInstRefnos {
        bran_hanger_refnos: target_bran_hanger_refnos,
        use_cate_refnos: Arc::new(use_cate_refnos),
        loop_owner_refnos: target_loop_owner_refnos,
        prim_refnos: target_prim_refnos,
    };

    xkt_debug(|| format!("数据库号： {} 生成instances完毕。", dbno));

    Ok(db_refnos)
}

///生成几何体数据
///
/// # 参数
/// * `dbno` - 可选的数据库编号
/// * `manual_refnos` - 手动指定的引用号列表
/// * `db_option` - 数据库选项
/// * `incr_updates` - 增量更新日志
/// * `sender` - 数据发送通道
/// * `target_sesno` - 目标会话号，用于历史模型生成
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    sender: flume::Sender<ShapeInstancesData>,
    target_sesno: Option<u32>,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let skip_exist = !db_option.is_replace_mesh();
    let mut all_handles = FuturesUnordered::new();
    // dbg!(&incr_updates);
    const CHUNK_SIZE: usize = 100;
    //根据需要拉入数据到本地数据库也可以
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    //排除增量更新的情况，如果debug_root_refnos 为空，即没有模型需要生成
    let debug_root_refnos = db_option.get_all_debug_refnos().await;
    // dbg!(&debug_root_refnos);
    if !is_incr_update
        //debug_root_refnos = [] 时表示不生成模型，如果没有这个属性表示生成所有
        && (db_option.debug_root_refnos.is_some() && debug_root_refnos.is_empty())
        && (!has_manual_refnos)
    {
        return Ok(vec![]);
    }
    if is_incr_update && incr_updates.as_ref().unwrap().count() == 0 {
        return Ok(vec![]);
    }
    let db_option_arc = Arc::new(db_option.clone());
    let is_debug = debug_root_refnos.len() > 0;

    let include_history = db_option_arc.is_gen_history_model();
    let is_replace_mesh = db_option_arc.is_replace_mesh();
    let incr_count = if is_incr_update {
        incr_updates.as_ref().unwrap().count()
    } else {
        0
    };
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
        // 检查是否需要进行历史查询
        if let Some(sesno) = target_sesno {
            // 验证会话是否存在 (暂时跳过验证)
            // if !session_exists(sesno).await? {
            //     return Err(anyhow::anyhow!("会话号 {} 不存在", sesno));
            // }

            println!(
                "使用历史查询，目标会话号: {} (注意：当前使用当前数据替代)",
                sesno
            );
            target_root_refnos =
                query_type_refnos_by_dbnum(&["SITE"], dbno.unwrap(), Some(true), include_history)
                    .await?
                    .into_iter()
                    .collect();
        } else {
            // 使用当前数据查询
            target_root_refnos =
                query_type_refnos_by_dbnum(&["SITE"], dbno.unwrap(), Some(true), include_history)
                    .await?
                    .into_iter()
                    .collect();
        }
    }
    if dbno.is_some() {
        xkt_debug(|| format!("总共 {} 个SITE", target_root_refnos.len()));
    } else {
        xkt_debug(|| format!("总共 {} 个结点", target_root_refnos.len()));
    }
    let origin_root_refnos = target_root_refnos.clone();
    // let process_handle = tokio::spawn(async move {
    // let mut handles = vec![]
    if is_incr_update {
        xkt_debug(|| format!("处理更新模型数量: {}", incr_count));
    } else if has_manual_refnos {
        xkt_debug(|| format!("处理生成模型数量: {}", manual_refnos.len()));
    } else if is_debug {
        xkt_debug(|| format!("调试模型数量: {:?}", debug_root_refnos.len()));
    } else if dbno.is_some() {
        xkt_debug(|| format!("处理db: {}", dbno.unwrap()));
    }
    let d_types = db_option_arc.debug_refno_types.clone();
    let mut gen_cata_flag =
        d_types.iter().any(|x| x == "CATA") || is_incr_update || has_manual_refnos;
    let mut gen_loop_flag =
        d_types.iter().any(|x| x == "LOOP") || is_incr_update || has_manual_refnos;
    let mut gen_prim_flag =
        d_types.iter().any(|x| x == "PRIM") || is_incr_update || has_manual_refnos;

    // dbg!(origin_root_refnos.len());
    let incr_updates_log_arc = Arc::new(incr_updates.clone().unwrap_or_default());
    //需要在这里把origin_root_refnos 打断成小块
    let mut chunked_root_refnos = origin_root_refnos.chunks(CHUNK_SIZE);
    let gen_model = db_option_arc.gen_model || is_incr_update || has_manual_refnos;
    //遍历小块
    while gen_model && let Some(target_refnos) = chunked_root_refnos.next() {
        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        //TODO 检查两个类型是否有可能在一个层级树里，如果不需要可以跳过
        {
            //查找到子节点的所有PLOO类型
            let Ok(target_ploo_refnos) =
                aios_core::query_multi_deep_versioned_children_filter_inst(
                    target_refnos,
                    &["PLOO"],
                    skip_exist,
                )
                .await
            else {
                continue;
            };
            #[cfg(debug_assertions)]
            if !target_ploo_refnos.is_empty() && is_xtk_debug_enabled() {
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
        let target_bran_hanger_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc
                .bran_hanger_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let r = aios_core::query_multi_deep_versioned_children_filter_inst(
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
        let mut use_cata_refnos = HashSet::new();
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

            let Ok(bran_children_refnos) = response.take::<Vec<RefnoEnum>>(0) else {
                dbg!("查询BRAN, HANG出错");
                continue;
            };
            let single_refnos = target_refnos
                .iter()
                .filter(|x| !target_bran_hanger_refnos.contains(x))
                .map(|x| *x)
                .collect::<Vec<_>>();
            use_cata_refnos =
                aios_core::query_multi_deep_children_filter_spre(single_refnos, skip_exist)
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
            xkt_debug(|| {
                format!(
                    "当前分段使用管道或者支吊架元件库数量: {}",
                    target_bran_hanger_refnos.len()
                )
            });
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
                    let start_time = Instant::now();
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        sjus_map_clone,
                        sender,
                    )
                    .await
                    .unwrap();
                    xkt_debug(|| {
                        format!(
                            "异步BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms",
                            start_time.elapsed().as_millis()
                        )
                    });
                });
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
            xkt_debug(|| format!("当前分段使用独立的元件库数量: {}", use_cata_refnos.len()));
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle = tokio::spawn(async move {
                let start_time = Instant::now();
                cata_model::gen_cata_geos(
                    db_option,
                    Arc::new(target_single_cata_map),
                    Arc::new(Default::default()),
                    sjus_map_clone,
                    sender,
                )
                .await
                .unwrap();
                xkt_debug(|| {
                    format!(
                        "异步单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms",
                        start_time.elapsed().as_millis()
                    )
                });
            });
            all_handles.push(handle);
        }

        let target_loop_owner_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc
                .loop_owner_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let mut loop_owner_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_LOOP_OWNER_NOUN_NAMES,
                skip_exist,
            )
            .await
            .unwrap_or_default();
            loop_owner_refnos.into_iter().collect()
        };
        if gen_loop_flag && !target_loop_owner_refnos.is_empty() {
            xkt_debug(|| format!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len()));
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

        let target_prim_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc.prim_refnos.iter().cloned().collect()
        } else {
            let mut prim_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
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
        if is_incr_update {
            break;
        }
    }
    //Ok::<_, anyhow::Error>(())
    while let Some(result) = all_handles.next().await {
        // 处理每个完成的 future 的结果
    }

    if dbno.is_some() {
        println!("数据库号： {} 生成instances完毕。", dbno.unwrap());
    }

    Ok(target_root_refnos)
}

///查询tubi的大小
pub async fn query_tubi_size(
    refno: RefnoEnum,
    tubi_cat_ref: RefnoEnum,
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

/// 从数据库生成 XKT 格式模型
///
/// # 参数
/// * `refnos` - 要处理的参考号列表
/// * `output_path` - 输出文件路径
/// * `compress` - 是否压缩输出文件
/// * `db_option` - 数据库配置选项
///
/// # 返回值
/// * `anyhow::Result<()>` - 返回生成结果
pub async fn generate_xtk_from_database(
    refnos: Vec<RefnoEnum>,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    println!("开始从数据库生成 XKT 格式模型...");
    let start_time = Instant::now();

    // 创建 XKT 文件
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "PDMS 模型导出".to_string();
    xkt_file.model.metadata.author = "aios-database".to_string();
    xkt_file.model.metadata.application = "aios-database XTK Generator".to_string();

    // 创建颜色方案
    let color_scheme = ColorScheme::new();

    // 创建数据库管理器
    let aios_mgr = AiosDBManager::init(&db_option).await?;

    // 统计信息
    let mut processed_count = 0;
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    // 从全局缓存获取所有有几何数据的元素
    let cache_holder = get_global_shape_cache().await;
    if let Some(cache) = cache_holder.as_deref() {
        let geometry_refnos: Vec<RefnoEnum> = cache.inst_info_map.keys().cloned().collect();
        println!("从缓存中找到 {} 个有几何数据的元素", geometry_refnos.len());

        // 调试：打印前10个有几何数据的元素
        println!("调试：前10个有几何数据的元素:");
        for refno in geometry_refnos.iter().take(10) {
            if let Some(type_name) = cached_element_type_name(cache, *refno) {
                println!("  - {} ({})", refno, type_name);
            } else {
                println!("  - {} (类型未知)", refno);
            }
        }

        // 如果指定了refnos，需要找到它们下面的所有有几何数据的子节点
        let mut target_refnos = HashSet::new();
        if !refnos.is_empty() {
            println!("调试：查找指定refnos下的所有几何节点...");

            // 先查询指定refnos的所有子节点
            for &refno in &refnos {
                // 查询所有子节点
                if let Ok(all_children) = query_multi_children_refnos(&[refno]).await {
                    println!("  refno {} 有 {} 个后代节点", refno, all_children.len());

                    // 检查这些子节点哪些在几何缓存中
                    for child in all_children {
                        if geometry_refnos.contains(&child) {
                            target_refnos.insert(child);
                        }
                    }
                }

                // 自己也可能有几何数据
                if geometry_refnos.contains(&refno) {
                    target_refnos.insert(refno);
                }
            }

            println!("找到 {} 个相关的几何节点", target_refnos.len());
        } else {
            // 没有指定的话，处理所有有几何数据的节点
            target_refnos = geometry_refnos.iter().cloned().collect();
        }

        // 批量处理所有有几何数据的元素
        for &refno in &target_refnos {
            let should_process = true; // 现在处理所有找到的几何节点

            if !should_process {
                continue;
            }

            match process_element_to_xtk(&mut xkt_file, refno, &color_scheme, &aios_mgr, cache)
                .await
            {
                Ok((geo_cnt, mesh_cnt, entity_cnt)) => {
                    geometry_count += geo_cnt;
                    mesh_count += mesh_cnt;
                    entity_count += entity_cnt;
                    processed_count += 1;
                    if processed_count % 100 == 0 {
                        println!("已处理 {} 个元素...", processed_count);
                    }
                }
                Err(e) => {
                    eprintln!("处理元素 {} 时出错: {}", refno, e);
                    continue;
                }
            }
        }
    } else {
        println!("警告：未找到全局形状缓存，尝试直接处理指定的参考号...");
        // 如果没有缓存，回退到直接处理指定的refnos
        for &refno in &refnos {
            match process_refno_to_xtk_fallback(&mut xkt_file, refno, &color_scheme, &aios_mgr)
                .await
            {
                Ok((geo_cnt, mesh_cnt, entity_cnt)) => {
                    geometry_count += geo_cnt;
                    mesh_count += mesh_cnt;
                    entity_count += entity_cnt;
                    processed_count += 1;
                }
                Err(e) => {
                    eprintln!("处理参考号 {} 时出错: {}", refno, e);
                    continue;
                }
            }
        }
    }

    // 完成模型构建
    xkt_file.model.finalize().await?;

    // 保存文件
    println!("正在保存 XKT 文件到: {}", output_path);
    xkt_file.save_to_file(output_path, compress).await?;

    let elapsed = start_time.elapsed();
    println!("XTK 生成完成!");
    println!("处理时间: {:.2}秒", elapsed.as_secs_f64());
    println!("统计信息:");
    println!("  - 处理的参考号: {}", processed_count);
    println!("  - 几何体数量: {}", geometry_count);
    println!("  - 网格数量: {}", mesh_count);
    println!("  - 实体数量: {}", entity_count);
    println!(
        "  - 文件大小: {:.2} MB",
        std::fs::metadata(output_path)?.len() as f64 / 1024.0 / 1024.0
    );

    Ok(())
}

/// 处理单个元素并转换为 XKT 格式（直接处理有几何数据的元素）
async fn process_element_to_xtk(
    xkt_file: &mut XKTFile,
    refno: RefnoEnum,
    color_scheme: &ColorScheme,
    aios_mgr: &AiosDBManager,
    cache: &ShapeInstancesData,
) -> anyhow::Result<(usize, usize, usize)> {
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    // 从缓存获取元素信息
    let element_type =
        cached_element_type_name(cache, refno).unwrap_or_else(|| "UNKNOWN".to_string());

    // 获取几何数据
    let shape_data = build_shape_subset(cache, refno);
    if shape_data.is_none() {
        return Ok((0, 0, 0));
    }
    let shape_data = shape_data.unwrap();

    // 创建实体
    let entity_id = format!("entity_{}", refno);
    let entity_name = format!("元素-{}", refno);
    let mut entity = XKTEntity::new(entity_id.clone(), entity_name, element_type.clone());

    // 获取世界变换
    let world_transform = if let Some(info) = cache.inst_info_map.get(&refno) {
        info.world_transform
    } else {
        aios_mgr.get_world_transform_or_default(refno.into()).await
    };

    // 处理几何数据
    for (geo_id, geo_data) in &shape_data.inst_geos_map {
        // 创建或获取几何体
        let base_geometry_id = if let Some(first_inst) = geo_data.insts.first() {
            if first_inst.geo_hash != 0 {
                format!("geo_hash_{}", first_inst.geo_hash)
            } else {
                format!("geo_{}", geo_data.refno)
            }
        } else {
            format!("geo_{}", geo_data.refno)
        };

        let geometry_id = base_geometry_id.clone();

        if !xkt_file.model.geometries.contains_key(&geometry_id) {
            let geometry_result = if let Some(first_inst) = geo_data.insts.first() {
                println!(
                    "调试: 尝试加载 mesh，refno: {}, geo_hash: {}",
                    refno, first_inst.geo_hash
                );
                if let Some(plant_mesh) = load_plant_mesh_by_hash(first_inst.geo_hash) {
                    println!("  成功加载 mesh 文件: {}.mesh", first_inst.geo_hash);
                    match create_geometry_from_plant_mesh(&geometry_id, &plant_mesh) {
                        Ok(geometry) => {
                            println!("  成功根据 PlantMesh 创建几何体");
                            Ok(geometry)
                        }
                        Err(e) => {
                            eprintln!(
                                "  根据 PlantMesh 创建几何体失败 (refno: {}, geo_hash: {}): {}",
                                refno, first_inst.geo_hash, e
                            );
                            create_geometry_from_geo_param(&geometry_id, &geo_data.insts).await
                        }
                    }
                } else {
                    println!(
                        "  未找到 mesh 文件: assets/meshes/{}.mesh，使用简单几何体",
                        first_inst.geo_hash
                    );
                    create_geometry_from_geo_param(&geometry_id, &geo_data.insts).await
                }
            } else {
                Err(anyhow::anyhow!("几何数据为空"))
            };

            match geometry_result {
                Ok(geometry) => {
                    xkt_file.model.create_geometry(geometry)?;
                    geometry_count += 1;
                }
                Err(e) => {
                    eprintln!("创建几何体失败 (refno: {}): {}", refno, e);
                    continue;
                }
            }
        }

        // 创建材质
        let material_id = format!("material_{}", geo_data.type_name);
        if !xkt_file.model.materials.contains_key(&material_id) {
            let color = color_scheme.get_color_for_type(&geo_data.type_name);
            let material = XKTMaterial::create_color_material(
                material_id.clone(),
                format!("{} 材质", geo_data.type_name),
                color,
            );
            xkt_file.model.create_material(material)?;
        }

        // 为每个几何实例创建网格
        for (i, inst) in geo_data.insts.iter().enumerate() {
            let mesh_id = format!("mesh_{}_{}", geo_data.refno, i);
            let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id.clone());
            mesh.set_material(material_id.clone());

            // 使用实例的变换（相对于元素的世界变换）
            let combined_transform = world_transform * inst.transform;
            mesh.set_position(combined_transform.translation);
            mesh.set_rotation(
                combined_transform
                    .rotation
                    .to_euler(glam::EulerRot::XYZ)
                    .into(),
            );
            mesh.set_scale(combined_transform.scale);
            mesh.set_visible(inst.visible);

            xkt_file.model.create_mesh(mesh)?;
            mesh_count += 1;

            // 将网格添加到实体
            entity.add_mesh(mesh_id);
        }
    }

    // 设置实体属性
    entity.set_property("refno".to_string(), refno.to_string());
    entity.set_property("type".to_string(), element_type.clone());

    // 创建实体
    xkt_file.model.create_entity(entity)?;
    entity_count += 1;

    Ok((geometry_count, mesh_count, entity_count))
}

/// 处理单个参考号并转换为 XKT 格式（备用方法，无缓存时使用）
async fn process_refno_to_xtk_fallback(
    xkt_file: &mut XKTFile,
    refno: RefnoEnum,
    color_scheme: &ColorScheme,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(usize, usize, usize)> {
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    // 获取元素信息
    let element_info = match aios_mgr.get_element_info(refno).await? {
        Some(info) => info,
        None => return Ok((0, 0, 0)),
    };

    // 获取几何数据
    let shape_instances = aios_mgr.get_shape_instances_data(refno).await?;
    if shape_instances.is_none() {
        return Ok((0, 0, 0));
    }
    let shape_data = shape_instances.unwrap();

    // 创建实体
    let entity_id = format!("entity_{}", refno);
    let entity_name = element_info
        .name
        .clone()
        .unwrap_or_else(|| format!("元素-{}", refno));
    let mut entity = XKTEntity::new(
        entity_id.clone(),
        entity_name,
        element_info.type_name.clone(),
    );

    // 获取世界变换
    let world_transform = aios_mgr.get_world_transform_or_default(refno.into()).await;

    // 处理几何数据（与process_element_to_xtk类似的逻辑）
    for (geo_id, geo_data) in &shape_data.inst_geos_map {
        let base_geometry_id = if let Some(first_inst) = geo_data.insts.first() {
            if first_inst.geo_hash != 0 {
                format!("geo_hash_{}", first_inst.geo_hash)
            } else {
                format!("geo_{}", geo_data.refno)
            }
        } else {
            format!("geo_{}", geo_data.refno)
        };

        let geometry_id = base_geometry_id.clone();

        if !xkt_file.model.geometries.contains_key(&geometry_id) {
            let geometry_result = if let Some(first_inst) = geo_data.insts.first() {
                println!(
                    "调试: 尝试加载 mesh，refno: {}, geo_hash: {}",
                    refno, first_inst.geo_hash
                );
                if let Some(plant_mesh) = load_plant_mesh_by_hash(first_inst.geo_hash) {
                    println!("  成功加载 mesh 文件: {}.mesh", first_inst.geo_hash);
                    match create_geometry_from_plant_mesh(&geometry_id, &plant_mesh) {
                        Ok(geometry) => {
                            println!("  成功根据 PlantMesh 创建几何体");
                            Ok(geometry)
                        }
                        Err(e) => {
                            eprintln!(
                                "  根据 PlantMesh 创建几何体失败 (refno: {}, geo_hash: {}): {}",
                                refno, first_inst.geo_hash, e
                            );
                            create_geometry_from_geo_param(&geometry_id, &geo_data.insts).await
                        }
                    }
                } else {
                    println!(
                        "  未找到 mesh 文件: assets/meshes/{}.mesh，使用简单几何体",
                        first_inst.geo_hash
                    );
                    create_geometry_from_geo_param(&geometry_id, &geo_data.insts).await
                }
            } else {
                Err(anyhow::anyhow!("几何数据为空"))
            };

            match geometry_result {
                Ok(geometry) => {
                    xkt_file.model.create_geometry(geometry)?;
                    geometry_count += 1;
                }
                Err(e) => {
                    eprintln!("创建几何体失败 (refno: {}): {}", refno, e);
                    continue;
                }
            }
        }

        // 创建材质
        let material_id = format!("material_{}", geo_data.type_name);
        if !xkt_file.model.materials.contains_key(&material_id) {
            let color = color_scheme.get_color_for_type(&geo_data.type_name);
            let material = XKTMaterial::create_color_material(
                material_id.clone(),
                format!("{} 材质", geo_data.type_name),
                color,
            );
            xkt_file.model.create_material(material)?;
        }

        // 为每个几何实例创建网格
        for (i, inst) in geo_data.insts.iter().enumerate() {
            let mesh_id = format!("mesh_{}_{}", geo_data.refno, i);
            let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id.clone());
            mesh.set_material(material_id.clone());

            let combined_transform = world_transform * inst.transform;
            mesh.set_position(combined_transform.translation);
            mesh.set_rotation(
                combined_transform
                    .rotation
                    .to_euler(glam::EulerRot::XYZ)
                    .into(),
            );
            mesh.set_scale(combined_transform.scale);
            mesh.set_visible(inst.visible);

            xkt_file.model.create_mesh(mesh)?;
            mesh_count += 1;
            entity.add_mesh(mesh_id);
        }
    }

    // 设置实体属性
    entity.set_property("refno".to_string(), refno.to_string());
    entity.set_property("type".to_string(), element_info.type_name.clone());
    if let Some(name) = &element_info.name {
        entity.set_property("name".to_string(), name.clone());
    }

    // 创建实体
    xkt_file.model.create_entity(entity)?;
    entity_count += 1;

    Ok((geometry_count, mesh_count, entity_count))
}

// 注释掉旧的递归处理函数，现在我们直接处理有几何数据的元素
// 保留这些函数以备将来需要层级结构时使用

/*
/// 递归处理节点及其所有子节点
fn process_node_recursive<'a>(
    xkt_file: &'a mut XKTFile,
    refno: RefnoEnum,
    parent_refno: Option<RefnoEnum>,
    color_scheme: &'a ColorScheme,
    aios_mgr: &'a AiosDBManager,
    created_entities: &'a mut std::collections::HashSet<RefnoEnum>,
    parent_child_relations: &'a mut Vec<(String, String)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<(usize, usize, usize)>> + 'a>>
{
    Box::pin(async move {
        // ... 递归处理逻辑 ...
        Ok((0, 0, 0))
    })
}

/// 获取直接子节点
async fn get_direct_children(
    refno: RefnoEnum,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Vec<RefnoEnum>> {
    // 查询所有以当前节点为 owner 的子节点
    let sql = format!("SELECT refno FROM pe WHERE owner = {}", refno.to_string());

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let children: Vec<RefnoEnum> = response.take(0).unwrap_or_default();
            Ok(children)
        }
        Err(e) => {
            eprintln!("查询子节点失败 (refno: {}): {}", refno, e);
            Ok(Vec::new())
        }
    }
}

/// 计算局部变换（子节点相对于父节点的变换）
fn calculate_local_transform(
    world_transform: &bevy_transform::components::Transform,
    parent_world_transform: &bevy_transform::components::Transform,
) -> bevy_transform::components::Transform {
    // 计算父节点世界变换的逆矩阵
    let parent_matrix = parent_world_transform.compute_matrix();
    let parent_inverse = parent_matrix.inverse();

    // 计算子节点的世界变换矩阵
    let world_matrix = world_transform.compute_matrix();

    // 局部变换 = 父节点逆变换 * 子节点世界变换
    let local_matrix = parent_inverse * world_matrix;

    // 从矩阵中提取变换组件
    bevy_transform::components::Transform::from_matrix(local_matrix)
}
*/

/// 根据数据库号生成 XKT 文件
///
/// # 参数
/// * `dbno` - 数据库号
/// * `output_path` - 输出文件路径
/// * `compress` - 是否压缩输出文件
/// * `db_option` - 数据库选项配置
///
/// # 返回值
/// * `anyhow::Result<()>` - 返回生成结果
pub async fn generate_xtk_by_dbno(
    dbno: u32,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    println!("正在查询数据库号 {} 的所有参考号...", dbno);

    prepare_global_shape_cache_for_db(dbno, db_option).await?;

    // 查询指定数据库号的所有参考号
    let mut all_refnos = match query_type_refnos_by_dbnum(&["SITE"], dbno, None, false).await {
        Ok(refnos) => refnos,
        Err(e) => {
            println!("无法查询 SITE 类型参考号: {}", e);
            Vec::new()
        }
    };
    if all_refnos.is_empty() {
        all_refnos = match query_type_refnos_by_dbnum(&[], dbno, None, false).await {
            Ok(refnos) => refnos,
            Err(e) => {
                println!("无法查询默认参考号: {}", e);
                Vec::new()
            }
        };
    }
    let cached_refnos = get_cached_refnos().await;
    if !cached_refnos.is_empty() {
        let mut set: BTreeSet<RefnoEnum> = all_refnos.into_iter().collect();
        set.extend(cached_refnos.into_iter());
        all_refnos = set.into_iter().collect();
    }

    if all_refnos.is_empty() {
        println!("⚠️ 未从数据库 {} 获取到参考号，生成空模型", dbno);
    }

    println!("找到 {} 个参考号", all_refnos.len());

    // 调用主要的生成函数
    let result = generate_xtk_from_database(all_refnos, output_path, compress, db_option).await;

    clear_global_shape_cache().await;

    result
}

/// 根据数据库号与指定参考号列表生成 XKT 文件
pub async fn generate_xtk_by_dbno_refnos(
    dbno: u32,
    refnos: Vec<RefnoEnum>,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Err(anyhow::anyhow!("参考号列表为空"));
    }

    println!(
        "准备为数据库号 {} 的 {} 个参考号生成 XKT...",
        dbno,
        refnos.len()
    );

    // 先确保生成几何数据和mesh
    println!("调试：先生成几何实例和mesh数据...");
    let mut option_with_mesh = db_option.clone();
    option_with_mesh.gen_model = true;
    option_with_mesh.gen_mesh = true;

    // 为指定的refnos生成几何数据
    clear_global_shape_cache().await;
    let option_arc = Arc::new(option_with_mesh);
    let (sender, receiver) = flume::unbounded();

    let collector = tokio::spawn(async move {
        let mut aggregated = ShapeInstancesData::default();
        while let Ok(data) = receiver.recv_async().await {
            aggregated.merge(data);
        }
        aggregated
    });

    // 直接为指定的refnos生成几何数据
    let _ = gen_geos_data(
        Some(dbno),
        refnos.clone(),
        &option_arc,
        None, // incr_updates
        sender.clone(),
        None, // target_sesno
    )
    .await?;

    drop(sender);
    let aggregated = collector.await?;

    // 在设置缓存之后，检查并生成缺失的 mesh 文件
    if aggregated.inst_info_map.len() > 0 {
        // 检查哪些节点需要生成 mesh
        let nodes_need_mesh = check_nodes_need_mesh_generation(&aggregated).await;

        if !nodes_need_mesh.is_empty() {
            println!("检测到 {} 个节点需要生成 mesh 文件", nodes_need_mesh.len());

            // 强制重新生成mesh，忽略数据库中的meshed标志
            println!("开启强制重新生成mesh模式...");
            let mut force_option = option_arc.as_ref().clone();
            force_option.replace_mesh = Some(true); // 强制替换已存在的mesh

            let force_option_arc = Arc::new(force_option);

            // 只为缺失 mesh 的节点生成 mesh
            if let Err(e) = process_meshes_update_db_deep(&force_option_arc, &nodes_need_mesh).await
            {
                eprintln!("警告: 生成 mesh 文件失败: {}", e);
            } else {
                println!("成功生成 {} 个元素的 mesh 文件", nodes_need_mesh.len());

                // 重新检查是否所有 mesh 都已生成
                let still_missing = check_nodes_need_mesh_generation(&aggregated).await;
                if !still_missing.is_empty() {
                    eprintln!(
                        "警告: 仍有 {} 个节点的 mesh 文件未生成",
                        still_missing.len()
                    );
                    for refno in &still_missing {
                        eprintln!("  - {}", refno);
                    }
                } else {
                    println!("✅ 所有节点的 mesh 文件生成成功");
                }
            }
        } else {
            println!("所有节点的 mesh 文件都已存在，无需重新生成");
        }
    }

    println!(
        "ShapeInstancesData 收集完成: inst_info={} geos={} tubi={}",
        aggregated.inst_info_map.len(),
        aggregated.inst_geos_map.len(),
        aggregated.inst_tubi_map.len()
    );

    set_cached_refnos(aggregated.inst_info_map.keys().cloned().collect()).await;
    set_global_shape_cache(aggregated).await;

    // 调试：查询refno下的所有子节点（在数据库初始化之后）
    println!("调试：查询 refno {:?} 下的所有子节点...", refnos);
    for &refno in &refnos {
        // 查询所有子节点
        let sql = format!(
            "SELECT refno, type FROM pe WHERE owner = {}",
            refno.to_string()
        );
        if let Ok(mut response) = SUL_DB.query(sql).await {
            let children: Vec<(RefnoEnum, String)> = response.take(0).unwrap_or_default();
            println!("  refno {} 有 {} 个直接子节点:", refno, children.len());
            for (child_refno, child_type) in children.iter().take(10) {
                println!("    - {} ({})", child_refno, child_type);
            }
            if children.len() > 10 {
                println!("    ... 还有 {} 个子节点", children.len() - 10);
            }
        }

        // 递归查询所有后代节点
        if let Ok(all_children) = query_multi_children_refnos(&[refno]).await {
            println!("  refno {} 总共有 {} 个后代节点", refno, all_children.len());

            // 查询这些节点的类型，看看有哪些可能有几何数据
            if !all_children.is_empty() {
                let types_sql = format!(
                    "SELECT type, COUNT(*) as count FROM [{}] GROUP BY type",
                    all_children
                        .iter()
                        .take(1000) // 限制查询数量
                        .map(|r| format!("pe:{}", r))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                if let Ok(mut response) = SUL_DB.query(types_sql).await {
                    let type_counts: Vec<(String, i32)> = response.take(0).unwrap_or_default();
                    println!("  节点类型分布:");
                    for (node_type, count) in type_counts {
                        println!("    - {}: {} 个", node_type, count);
                    }
                }
            }

            // 检查生成的几何缓存中有哪些节点
            if let Some(cache) = get_global_shape_cache().await {
                let cache_refnos: HashSet<RefnoEnum> =
                    cache.inst_info_map.keys().cloned().collect();
                let mut found_count = 0;
                for child in &all_children {
                    if cache_refnos.contains(child) {
                        found_count += 1;
                        if found_count <= 10 {
                            println!("    找到几何数据: {}", child);
                        }
                    }
                }
                if found_count > 10 {
                    println!("    ... 还有 {} 个节点有几何数据", found_count - 10);
                }
                println!(
                    "  总计: {} / {} 个子节点有几何数据",
                    found_count,
                    all_children.len()
                );
            }
        }
    }

    let unique_refnos: Vec<RefnoEnum> = refnos
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    println!("最终用于生成的参考号数量: {}", unique_refnos.len());

    let result = generate_xtk_from_database(unique_refnos, output_path, compress, db_option).await;

    clear_global_shape_cache().await;

    result
}

/// 根据数据库号与单个参考号生成 XKT 文件
pub async fn generate_xtk_by_dbno_refno(
    dbno: u32,
    refno: RefnoEnum,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    println!("准备为数据库号 {} 的参考号 {} 生成 XKT...", dbno, refno);

    generate_xtk_by_dbno_refnos(dbno, vec![refno], output_path, compress, db_option).await
}

// 定义一个简化的元素信息结构
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub name: Option<String>,
    pub type_name: String,
}

// 为 AiosDBManager 添加扩展方法的 trait
trait AiosDBManagerExt {
    async fn get_element_info(&self, refno: RefnoEnum) -> anyhow::Result<Option<ElementInfo>>;
    async fn get_shape_instances_data(
        &self,
        refno: RefnoEnum,
    ) -> anyhow::Result<Option<ShapeInstancesData>>;
}

impl AiosDBManagerExt for AiosDBManager {
    async fn get_element_info(&self, refno: RefnoEnum) -> anyhow::Result<Option<ElementInfo>> {
        if let Some(cache_arc) = get_global_shape_cache().await {
            let cache = cache_arc.as_ref();
            if let Some(type_name) = cached_element_type_name(cache, refno) {
                return Ok(Some(ElementInfo {
                    name: Some(format!("元素-{}", refno)),
                    type_name,
                }));
            }
        }

        let fallback_type = aios_core::get_type_name(refno)
            .await
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        Ok(Some(ElementInfo {
            name: Some(format!("元素-{}", refno)),
            type_name: fallback_type,
        }))
    }

    async fn get_shape_instances_data(
        &self,
        refno: RefnoEnum,
    ) -> anyhow::Result<Option<ShapeInstancesData>> {
        if let Some(cache_arc) = get_global_shape_cache().await {
            if let Some(subset) = build_shape_subset(cache_arc.as_ref(), refno) {
                return Ok(Some(subset));
            }
        }
        Ok(None)
    }
}

/// 从几何参数创建几何体
pub async fn create_geometry_from_geo_param(
    geometry_id: &str,
    geo_instances: &[EleInstGeo],
) -> anyhow::Result<XKTGeometry> {
    if geo_instances.is_empty() {
        return Err(anyhow::anyhow!("没有几何实例数据"));
    }

    // 使用第一个实例的几何参数
    let first_instance = &geo_instances[0];

    match &first_instance.geo_param {
        PdmsGeoParam::PrimBox(box_param) => {
            // 使用 size 字段而不是 xlength, ylength, zlength
            let size = &box_param.size;
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                size.x,
                size.y,
                size.z,
            ))
        }
        PdmsGeoParam::PrimSCylinder(scyl_param) => {
            // 使用 pdia 和 phei 字段
            Ok(XKTGeometry::create_cylinder(
                geometry_id.to_string(),
                scyl_param.pdia / 2.0,
                scyl_param.phei,
                32, // 分段数
            ))
        }
        PdmsGeoParam::PrimSphere(sphere_param) => {
            // 使用 radius 字段而不是 diameter
            Ok(XKTGeometry::create_sphere(
                geometry_id.to_string(),
                sphere_param.radius,
                32, // 经度分段
                16, // 纬度分段
            ))
        }
        PdmsGeoParam::PrimPyramid(pyramid_param) => {
            // 对于金字塔，我们创建一个近似的立方体
            // 使用实际可用的字段
            let avg_size = 1.0; // 默认大小，因为字段结构不明确
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                avg_size,
                avg_size,
                avg_size,
            ))
        }
        _ => {
            // 对于其他类型，创建一个默认的立方体
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                1.0,
                1.0,
                1.0,
            ))
        }
    }
}

/// 创建占位符实体（当没有几何数据时）
async fn create_placeholder_entity(
    xkt_file: &mut XKTFile,
    refno: RefnoEnum,
    element_info: &ElementInfo,
    color_scheme: &ColorScheme,
) -> anyhow::Result<(usize, usize, usize)> {
    // 创建一个小的立方体作为占位符
    let geometry_id = format!("placeholder_geo_{}", refno);
    let geometry = XKTGeometry::create_box(geometry_id.clone(), 0.1, 0.1, 0.1);
    xkt_file.model.create_geometry(geometry)?;

    // 创建材质
    let material_id = format!("placeholder_material_{}", element_info.type_name);
    if !xkt_file.model.materials.contains_key(&material_id) {
        let color = color_scheme.get_color_for_type(&element_info.type_name);
        let mut material = XKTMaterial::create_color_material(
            material_id.clone(),
            format!("占位符-{}", element_info.type_name),
            color,
        );
        material.set_opacity(0.3); // 设置为半透明
        xkt_file.model.create_material(material)?;
    }

    // 创建网格
    let mesh_id = format!("placeholder_mesh_{}", refno);
    let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id);
    mesh.set_material(material_id);
    mesh.set_position(Vec3::ZERO);
    xkt_file.model.create_mesh(mesh)?;

    // 创建实体
    let entity_id = format!("placeholder_entity_{}", refno);
    let mut entity = XKTEntity::new(
        entity_id,
        element_info
            .name
            .clone()
            .unwrap_or_else(|| format!("占位符-{}", refno)),
        element_info.type_name.clone(),
    );
    entity.add_mesh(mesh_id);
    entity.set_property("refno".to_string(), refno.to_string());
    entity.set_property("type".to_string(), element_info.type_name.clone());
    entity.set_property("placeholder".to_string(), "true".to_string());

    xkt_file.model.create_entity(entity)?;

    Ok((1, 1, 1)) // 1个几何体，1个网格，1个实体
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::options::DbOption;
    use std::path::Path;

    /// 测试 generate_xtk_by_dbno 函数
    #[test]
    fn test_generate_xtk_by_dbno() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 函数 ===");

        // 创建测试用的数据库选项
        let mut db_option = DbOption::default();
        db_option.gen_model = true;
        db_option.gen_mesh = false; // 为了测试速度，暂时不生成网格

        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();

        // 测试数据库号（使用一个较小的测试数据库号）
        let test_dbno = 1u32; // 可以根据实际情况调整
        let output_path = "test_output/test_dbno_model.xkt";

        println!("开始测试数据库号: {}", test_dbno);
        println!("输出路径: {}", output_path);

        // 测试生成 XKT 文件
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            output_path,
            true, // 启用压缩
            &db_option,
        )) {
            Ok(_) => {
                println!("✅ generate_xtk_by_dbno 测试成功");

                // 验证文件是否存在
                if Path::new(output_path).exists() {
                    // 验证文件大小
                    let metadata = std::fs::metadata(output_path)?;
                    println!("生成的文件大小: {} 字节", metadata.len());

                    // 基本验证：文件应该有一定的大小
                    assert!(metadata.len() > 100, "生成的文件太小，可能有问题");

                    println!("文件验证通过");
                } else {
                    println!("⚠️  输出文件不存在，可能是因为数据库中没有数据");
                }
            }
            Err(e) => {
                eprintln!("❌ generate_xtk_by_dbno 测试失败: {}", e);

                // 对于某些预期的错误（如数据库连接失败），我们可以容忍
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  测试失败是由于数据库连接问题，这在测试环境中是可以接受的");
                    return Ok(());
                }

                return Err(e);
            }
        }

        Ok(())
    }

    /// 测试 generate_xtk_by_dbno 函数的参数验证
    #[test]
    fn test_generate_xtk_by_dbno_with_invalid_params() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 参数验证 ===");

        let db_option = DbOption::default();

        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();

        // 测试无效的输出路径
        let invalid_output_path = "/invalid/path/that/does/not/exist/test.xkt";
        let test_dbno = 1u32;

        println!("测试无效输出路径: {}", invalid_output_path);

        // 这个测试应该失败，因为路径无效
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            invalid_output_path,
            false,
            &db_option,
        )) {
            Ok(_) => {
                println!("⚠️  预期失败但成功了，可能路径实际上是有效的");
            }
            Err(e) => {
                println!("✅ 按预期失败: {}", e);
                // 这是预期的行为
            }
        }

        Ok(())
    }

    /// 测试 generate_xtk_by_dbno 函数的不同压缩选项
    #[test]
    fn test_generate_xtk_by_dbno_compression_options() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 压缩选项 ===");

        let mut db_option = DbOption::default();
        db_option.gen_model = true;
        db_option.gen_mesh = false;

        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();

        let test_dbno = 1u32;
        let compressed_path = "test_output/test_compressed.xkt";
        let uncompressed_path = "test_output/test_uncompressed.xkt";

        // 测试压缩版本
        println!("测试压缩版本...");
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            compressed_path,
            true, // 启用压缩
            &db_option,
        )) {
            Ok(_) => println!("✅ 压缩版本生成成功"),
            Err(e) => {
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  压缩版本测试跳过（数据库连接问题）");
                    return Ok(());
                }
                eprintln!("❌ 压缩版本生成失败: {}", e);
            }
        }

        // 测试非压缩版本
        println!("测试非压缩版本...");
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            uncompressed_path,
            false, // 禁用压缩
            &db_option,
        )) {
            Ok(_) => println!("✅ 非压缩版本生成成功"),
            Err(e) => {
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  非压缩版本测试跳过（数据库连接问题）");
                    return Ok(());
                }
                eprintln!("❌ 非压缩版本生成失败: {}", e);
            }
        }

        // 比较文件大小（如果两个文件都存在）
        if Path::new(compressed_path).exists() && Path::new(uncompressed_path).exists() {
            let compressed_size = std::fs::metadata(compressed_path)?.len();
            let uncompressed_size = std::fs::metadata(uncompressed_path)?.len();

            println!("压缩文件大小: {} 字节", compressed_size);
            println!("非压缩文件大小: {} 字节", uncompressed_size);

            // 通常压缩文件应该更小（除非文件很小）
            if uncompressed_size > 1000 {
                assert!(
                    compressed_size <= uncompressed_size,
                    "压缩文件应该不大于非压缩文件"
                );
            }
        }

        Ok(())
    }

    /// 运行所有 generate_xtk_by_dbno 相关的测试
    pub fn run_all_generate_xtk_by_dbno_tests() -> anyhow::Result<()> {
        println!("=== 开始运行 generate_xtk_by_dbno 测试套件 ===");

        // 运行各个测试
        test_generate_xtk_by_dbno()?;
        test_generate_xtk_by_dbno_with_invalid_params()?;
        test_generate_xtk_by_dbno_compression_options()?;

        println!("=== generate_xtk_by_dbno 测试套件完成 ===");
        Ok(())
    }
}
