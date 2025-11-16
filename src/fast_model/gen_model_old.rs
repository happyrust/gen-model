use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::sesno_increment::get_changes_at_sesno;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::pdms_inst::save_instance_data_optimize;
use crate::fast_model::{
    booleans_meshes_in_db, cata_model, gen_meshes_in_db, loop_model, prim_model,
    process_meshes_update_db_deep, resolve_desi_comp, shared,
};
use crate::fast_model::{capture::capture_refnos_if_enabled, debug_model_debug, debug_model_trace};
use crate::{e3d_dbg, e3d_info, e3d_trace};
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{
    EleGeosInfo, EleInstGeo, EleInstGeosData, GeoBasicType, PlantGeoData, ShapeInstancesData,
};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;

use aios_core::SUL_DB;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{DBType, prim_geo::*};
use aios_core::{RefU64, RefnoEnum, pdms_types::*};

// 新的查询接口
use crate::fast_model::query_provider::{
    count_noun_all_db, get_children_batch, query_noun_page_all_db, query_by_noun_all_db,
    query_by_type, query_multi_descendants,
};
use anyhow::{anyhow, bail};
use futures::stream::{self, StreamExt, TryStreamExt};

#[cfg(feature = "sqlite-index")]
use crate::spatial_index::SqliteSpatialIndex;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
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
use tokio::task::JoinHandle;

/// Noun 分类枚举，用于 Full Noun 模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NounCategory {
    /// 使用元件库的 Noun
    Cate,
    /// Loop owner Noun
    LoopOwner,
    /// 基本体 Noun
    Prim,
}

async fn process_loop_refno_page(
    ctx: &NounProcessContext,
    loop_sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    if !loop_model::gen_loop_geos(ctx.db_option.clone(), refnos, loop_sjus_map_arc, sender)
        .await?
    {
        bail!("loop geos generation failed");
    }
    Ok(())
}

async fn process_prim_refno_page(
    ctx: &NounProcessContext,
    sender: flume::Sender<ShapeInstancesData>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    if !prim_model::gen_prim_geos(ctx.db_option.clone(), refnos, sender).await? {
        bail!("prim geos generation failed");
    }
    Ok(())
}

async fn process_cate_refno_page(
    ctx: &NounProcessContext,
    loop_sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    let target_cata_map = Arc::new(
        aios_core::query_group_by_cata_hash(refnos)
            .await
            .unwrap_or_default(),
    );

    if target_cata_map.is_empty() {
        return Ok(());
    }

    if !cata_model::gen_cata_geos(
        ctx.db_option.clone(),
        target_cata_map,
        Arc::new(Default::default()),
        loop_sjus_map_arc,
        sender,
    )
    .await?
    {
        bail!("cate geos generation failed");
    }
    Ok(())
}

/// Full Noun 模式下的 Noun 列表聚合结果
#[derive(Debug, Clone)]
pub struct FullNounCollection {
    /// 按类别分组的 Noun 列表
    pub cate_nouns: Vec<&'static str>,
    pub loop_owner_nouns: Vec<&'static str>,
    pub prim_nouns: Vec<&'static str>,
    /// 所有 Noun 的去重集合（用于快速查找）
    pub all_nouns: HashSet<&'static str>,
}

impl FullNounCollection {
    /// 聚合并去重所有 Noun 列表
    ///
    /// 从 pdms_types 中的常量收集：
    /// - USE_CATE_NOUN_NAMES
    /// - GNERAL_LOOP_OWNER_NOUN_NAMES
    /// - GNERAL_PRIM_NOUN_NAMES
    ///
    /// 可选的 extra_nouns 用于扩展（调试或特殊场景）
    pub fn collect(extra_nouns: Option<&[&'static str]>) -> Self {
        let mut all_nouns = HashSet::new();

        // 收集 cate nouns
        let mut cate_nouns = Vec::new();
        for &noun in USE_CATE_NOUN_NAMES.iter() {
            if all_nouns.insert(noun) {
                cate_nouns.push(noun);
            }
        }

        // 收集 loop owner nouns
        let mut loop_owner_nouns = Vec::new();
        for &noun in GNERAL_LOOP_OWNER_NOUN_NAMES.iter() {
            if all_nouns.insert(noun) {
                loop_owner_nouns.push(noun);
            }
        }

        // 收集 prim nouns
        let mut prim_nouns = Vec::new();
        for &noun in GNERAL_PRIM_NOUN_NAMES.iter() {
            if all_nouns.insert(noun) {
                prim_nouns.push(noun);
            }
        }

        // 添加额外的 nouns（如果提供）
        if let Some(extras) = extra_nouns {
            for &noun in extras {
                all_nouns.insert(noun);
                // 简单策略：额外的 noun 默认归入 cate 类别
                // 实际使用时可以根据需要调整
                if !cate_nouns.contains(&noun)
                    && !loop_owner_nouns.contains(&noun)
                    && !prim_nouns.contains(&noun) {
                    cate_nouns.push(noun);
                }
            }
        }

        Self {
            cate_nouns,
            loop_owner_nouns,
            prim_nouns,
            all_nouns,
        }
    }

    /// 根据 Noun 名称判断其类别
    pub fn get_category(&self, noun: &str) -> Option<NounCategory> {
        if self.cate_nouns.contains(&noun) {
            Some(NounCategory::Cate)
        } else if self.loop_owner_nouns.contains(&noun) {
            Some(NounCategory::LoopOwner)
        } else if self.prim_nouns.contains(&noun) {
            Some(NounCategory::Prim)
        } else {
            None
        }
    }

    /// 获取所有 Noun 的总数
    pub fn total_count(&self) -> usize {
        self.all_nouns.len()
    }
}

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
                let target_refnos = match get_children_batch(&bran_refnos).await {
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
                let target_refnos = match get_children_batch(&chunk).await {
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

#[cfg(feature = "debug_e3d")]
static E3D_DEBUG_ENABLED: Lazy<bool> = Lazy::new(|| {
    // Backward compatible env names supported
    std::env::var("E3D_DEBUG")
        .or_else(|_| std::env::var("XKT_GEN_DEBUG"))
        .or_else(|_| std::env::var("XKT_GEN_VERBOSE"))
        .ok()
        .and_then(|value| parse_env_flag(&value))
        .unwrap_or(false)
});

#[cfg(feature = "debug_e3d")]
static E3D_INFO_ENABLED: Lazy<bool> = Lazy::new(|| {
    // TRACE implies INFO
    if is_e3d_trace_enabled() {
        return true;
    }
    std::env::var("E3D_INFO")
        .or_else(|_| std::env::var("E3D_DEBUG"))
        .or_else(|_| std::env::var("XKT_GEN_DEBUG"))
        .ok()
        .and_then(|value| parse_env_flag(&value))
        .unwrap_or(false)
});

#[cfg(feature = "debug_e3d")]
static E3D_TRACE_ENABLED: Lazy<bool> = Lazy::new(|| {
    std::env::var("E3D_TRACE")
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

#[allow(dead_code)]
pub(crate) fn is_e3d_debug_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        *E3D_DEBUG_ENABLED
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

#[allow(dead_code)]
pub(crate) fn is_e3d_info_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        *E3D_INFO_ENABLED
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

#[allow(dead_code)]
pub(crate) fn is_e3d_trace_enabled() -> bool {
    #[cfg(feature = "debug_e3d")]
    {
        *E3D_TRACE_ENABLED
    }
    #[cfg(not(feature = "debug_e3d"))]
    {
        false
    }
}

// Macros for leveled debugging - 使用 gen_model 中的定义
// #[macro_export]
// macro_rules! e3d_dbg {
//     ($($arg:tt)*) => {{
//         if crate::fast_model::gen_model_old::is_e3d_debug_enabled() {
//             println!($($arg)*);
//         }
//     }};
// }

#[macro_export]
macro_rules! e3d_info {
    ($($arg:tt)*) => {{
        if crate::fast_model::gen_model::is_e3d_info_enabled() {
            println!($($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! e3d_trace {
    ($($arg:tt)*) => {{
        if crate::fast_model::gen_model::is_e3d_trace_enabled() {
            println!($($arg)*);
        }
    }};
}

/// 检查指定的 geo_hash 是否有对应的 mesh 文件
fn check_mesh_exists(geo_hash: u64) -> bool {
    if geo_hash == 0 {
        return false;
    }
    let filename = format!("assets/meshes/{}.mesh", geo_hash);
    let exists = Path::new(&filename).exists();

    exists
}

/// 检查多个几何体节点，返回需要重新生成 mesh 的节点
async fn check_nodes_need_mesh_generation(shape_data: &ShapeInstancesData) -> Vec<RefnoEnum> {
    let mut need_regenerate = Vec::new();
    let mut total_checked = 0;
    let mut missing_mesh_count = 0;

    for (refno, inst_info) in &shape_data.inst_info_map {
        total_checked += 1;

        // 获取实例的 inst_key
        let inst_key = inst_info.get_inst_key();
        if let Some(geo_data) = shape_data.inst_geos_map.get(&inst_key) {
            // 检查每个实例的 mesh 是否存在
            let mut missing_meshes = Vec::new();
            for (idx, inst) in geo_data.insts.iter().enumerate() {
                if inst.geo_hash != 0 {
                    if !check_mesh_exists(inst.geo_hash) {
                        missing_meshes.push(inst.geo_hash);
                    }
                } else {
                }
            }

            if !missing_meshes.is_empty() {
                for hash in &missing_meshes {}
                missing_mesh_count += missing_meshes.len();
                need_regenerate.push(refno.clone());
            } else if !geo_data.insts.is_empty() {
            } else {
            }
        } else {
        }
    }

    // 检查 TUBI 节点 (inst_tubi_map 存储的是 EleGeosInfo 类型)
    for (refno, _tubi_info) in &shape_data.inst_tubi_map {
        // TUBI 节点的 mesh 生成比较特殊，暂时跳过检查
        // 如果需要检查，需要根据 TUBI 的特定逻辑来处理
    }

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
                        final_incr_updates = Some(sesno_changes);
                    } else {
                        println!("[gen_model] sesno {} 没有发现变更，跳过增量生成", sesno);
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

    let incr_count = final_incr_updates
        .as_ref()
        .map(|log| log.count())
        .unwrap_or(0);
    println!(
        "[gen_model] 启动 gen_all_geos_data: manual_refnos={}, incr_updates={}, target_sesno={:?}, gen_mesh={}, gen_model={}",
        manual_refnos.len(),
        incr_count,
        target_sesno,
        db_option.gen_mesh,
        db_option.gen_model
    );

    // 检查是否启用 Full Noun 模式（优先级最高）
    // 从环境变量读取 full_noun_mode 配置
    let full_noun_mode = std::env::var("FULL_NOUN_MODE")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    if full_noun_mode {
        // Full Noun 模式：直接按 Noun 全库扫描，忽略增量更新
        println!("[gen_model] 进入 Full Noun 模式（忽略增量更新）");

        if db_option.manual_db_nums.is_some() || db_option.exclude_db_nums.is_some() {
            println!(
                "[gen_model] 警告: Full Noun 模式下 manual_db_nums 和 exclude_db_nums 配置将被忽略"
            );
        }

        if final_incr_updates.is_some() {
            println!(
                "[gen_model] 警告: Full Noun 模式下增量更新将被忽略，将执行全库重建"
            );
        }

        let full_start = Instant::now();
        let db_refnos = gen_full_noun_geos(db_option, None).await?;

        println!(
            "[gen_model] Full Noun 模式 insts 入库完成，用时 {} ms",
            full_start.elapsed().as_millis()
        );

        // 可选执行 mesh 和布尔运算
        if db_option.gen_mesh {
            let mesh_start = Instant::now();
            println!("[gen_model] Full Noun 模式开始生成三角网格");
            db_refnos
                .execute_gen_inst_meshes(Some(Arc::new(db_option.clone())))
                .await;
            println!(
                "[gen_model] Full Noun 模式三角网格生成完成，用时 {} ms",
                mesh_start.elapsed().as_millis()
            );

            if db_option.apply_boolean_operation {
                let bool_start = Instant::now();
                println!("[gen_model] Full Noun 模式开始布尔运算");
                db_refnos
                    .execute_boolean_meshes(Some(Arc::new(db_option.clone())))
                    .await;
                println!(
                    "[gen_model] Full Noun 模式布尔运算完成，用时 {} ms",
                    bool_start.elapsed().as_millis()
                );
            }
        }

        println!(
            "[gen_model] Full Noun 模式全部完成，总用时 {} ms",
            full_start.elapsed().as_millis()
        );

        return Ok(true);
    }

    let is_incr_update = final_incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    let has_debug = db_option.debug_model_refnos.is_some();

    if is_incr_update || has_manual_refnos || has_debug {
        let mode_label = if is_incr_update {
            "增量"
        } else if has_manual_refnos {
            "手动"
        } else {
            "调试"
        };
        let target_count = if is_incr_update {
            incr_count
        } else if has_manual_refnos {
            manual_refnos.len()
        } else {
            db_option
                .debug_model_refnos
                .as_ref()
                .map(|items| items.len())
                .unwrap_or(0)
        };
        println!(
            "[gen_model] 进入{}生成路径，目标节点数: {}",
            mode_label, target_count
        );
        // let (sender, receiver) = flume::bounded(CHUNK_SIZE);
        let (sender, receiver) = flume::unbounded();
        let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();

        // ⚠️  使用 replace_mesh 配置项控制是否替换已存在的 inst_relate
        // plant3d 场景下默认不启用，避免删除已存在的 inst_relate
        // 即使开启了 debug_model_debug，也默认不 replace exist
        let replace_exist = db_option.is_replace_mesh();

        let insert_task = tokio::task::spawn(async move {
            while let Ok(shape_insts) = receiver.recv_async().await {
                save_instance_data_optimize(&shape_insts, replace_exist)
                    .await
                    .unwrap();
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
        println!(
            "[gen_model] {}路径模型生成完成，共 {} 个根节点",
            mode_label,
            target_root_refnos.len()
        );
        if db_option.gen_mesh {
            let mesh_start = Instant::now();
            println!(
                "[gen_model] 开始更新 {} 个根节点的 mesh 数据",
                target_root_refnos.len()
            );
            process_meshes_update_db_deep(db_option, &target_root_refnos)
                .await
                .expect("更新模型数据失败");
            println!(
                "[gen_model] 完成 mesh 更新，用时 {} ms",
                mesh_start.elapsed().as_millis()
            );
        }

        if let Err(err) = capture_refnos_if_enabled(&target_root_refnos, db_option).await {
            eprintln!("[capture] 捕获截图失败: {}", err);
        }
    } else {
        // 原有的按 dbno 循环生成路径
        let dbnos = if db_option.manual_db_nums.is_some() {
            db_option.manual_db_nums.clone().unwrap()
        } else {
            aios_core::query_mdb_db_nums(None, DBType::DESI).await?
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

        println!(
            "[gen_model] 进入全量生成路径，共 {} 个数据库待处理",
            dbnos.len()
        );
        let db_option_arc = Arc::new(db_option.clone());
        if dbnos.is_empty() {
            println!("[gen_model] 未找到需要生成的数据库，直接结束");
        }
        for dbno in dbnos.clone() {
            println!("[gen_model] -> 开始处理数据库 {}", dbno);
            let db_start = Instant::now();
            let (sender, receiver) = flume::unbounded();
            let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
            let insert_task = tokio::task::spawn(async move {
                while let Ok(shape_insts) = receiver.recv_async().await {
                    let time = Instant::now();
                    // save_instance_data_optimize(&shape_insts, false).await.unwrap();
                    save_instance_data_optimize(&shape_insts, false)
                        .await
                        .unwrap();
                }
            });
            let db_refnos =
                gen_geos_data_by_dbnum(dbno, db_option_arc.clone(), sender.clone(), target_sesno)
                    .await?;
            drop(sender);
            insert_task.await.unwrap();
            println!(
                "[gen_model] -> 数据库 {} insts 入库完成，用时 {} ms",
                dbno,
                db_start.elapsed().as_millis()
            );
            if db_option_arc.gen_mesh {
                let mesh_start = Instant::now();
                println!("[gen_model] -> 数据库 {} 开始生成三角网格", dbno);
                //模型生成完之后，再进行布尔运算
                db_refnos
                    .execute_gen_inst_meshes(Some(db_option_arc.clone()))
                    .await;
                println!(
                    "[gen_model] -> 数据库 {} 三角网格生成完成，用时 {} ms",
                    dbno,
                    mesh_start.elapsed().as_millis()
                );
                let boolean_start = Instant::now();
                println!("[gen_model] -> 数据库 {} 开始布尔运算", dbno);
                db_refnos
                    .execute_boolean_meshes(Some(db_option_arc.clone()))
                    .await;
                println!(
                    "[gen_model] -> 数据库 {} 布尔运算完成，用时 {} ms",
                    dbno,
                    boolean_start.elapsed().as_millis()
                );
            }
            println!(
                "[gen_model] -> 数据库 {} 处理完成，总耗时 {} ms",
                dbno,
                db_start.elapsed().as_millis()
            );
        }
    } // 关闭最外层 else 分支（全量生成路径）
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
    println!(
        "[gen_model] gen_all_geos_data 完成，总耗时 {} ms",
        time.elapsed().as_millis()
    );

    Ok(true)
}

/// Full Noun 模式：按 Noun 全库生成几何体数据
///
/// 直接以 Noun 表为根扫描全库，不引入 dbno 或 refno 层级约束。
///
/// # 参数
/// * `db_option` - 数据库选项配置
/// * `extra_nouns` - 可选的额外 Noun 列表（用于调试或扩展）
///
/// # 返回值
/// * `anyhow::Result<DbModelInstRefnos>` - 返回聚合的 refno 集合
///
/// # 实现说明
///
/// 1. 聚合 Noun 列表（USE_CATE + LOOP_OWNER + PRIM + extra）
/// 2. 创建 flume 通道用于异步数据入库
/// 3. 按 Noun 类别并发查询和生成：
///    - 使用 `query_by_noun_all_db` 查询全库 refno
///    - 根据 Noun 类别调用对应的生成管线
///    - 生成的 `ShapeInstancesData` 通过 flume 发送
/// 4. 汇总并去重 refno，构建 `DbModelInstRefnos`
/// 5. 可选执行 mesh 和布尔运算
///
/// # 注意
///
/// 由于 Full Noun 模式跳过了 dbno 层级，某些需要预处理的数据（如 sjus_map、branch_map）
/// 在此模式下会使用默认值或跳过相关逻辑。
pub async fn gen_full_noun_geos(
    db_option: &DbOption,
    extra_nouns: Option<&[&'static str]>,
) -> anyhow::Result<DbModelInstRefnos> {
    use crate::options::DbOptionExt;

    let start_time = Instant::now();

    // 从 DbOptionExt 获取 full noun 配置
    let db_option_ext = DbOptionExt::from(db_option.clone());
    let max_concurrent = db_option_ext.get_full_noun_concurrency();
    let batch_size = db_option_ext.get_full_noun_batch_size();
    let batch_concurrency = max_concurrent.max(1);

    println!(
        "[gen_full_noun_geos] 启动 Full Noun 模式，并发度: {}",
        max_concurrent
    );

    // 1. 聚合 Noun 列表
    let noun_collection = FullNounCollection::collect(extra_nouns);
    println!(
        "[gen_full_noun_geos] Noun 统计: cate={}, loop={}, prim={}, 总计={}",
        noun_collection.cate_nouns.len(),
        noun_collection.loop_owner_nouns.len(),
        noun_collection.prim_nouns.len(),
        noun_collection.total_count()
    );

    // 2. 创建 flume 通道
    let channel_cap = batch_concurrency * 2;
    let (sender, receiver) = flume::bounded::<ShapeInstancesData>(channel_cap);
    let receiver_clone = receiver.clone();

    // 启动异步入库任务
    let insert_task = tokio::task::spawn(async move {
        while let Ok(shape_insts) = receiver_clone.recv_async().await {
            save_instance_data_optimize(&shape_insts, false)
                .await
                .unwrap();
        }
    });

    // 3. 用于汇总所有 refno 的集合
    let all_use_cate_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));
    let all_loop_owner_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));
    let all_prim_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));

    // 4. 顺序处理各类别任务，内部使用批次并发
    let db_option_arc = Arc::new(db_option.clone());
    let loop_sjus_map_arc = Arc::new(DashMap::new()); // Full Noun 模式下使用空的 sjus_map

    process_cate_nouns(
        &noun_collection,
        db_option_arc.clone(),
        loop_sjus_map_arc.clone(),
        sender.clone(),
        batch_size,
        batch_concurrency,
        all_use_cate_refnos.clone(),
    )
    .await?;

    process_loop_nouns(
        &noun_collection,
        db_option_arc.clone(),
        loop_sjus_map_arc.clone(),
        sender.clone(),
        batch_size,
        batch_concurrency,
        all_loop_owner_refnos.clone(),
    )
    .await?;

    process_prim_nouns(
        &noun_collection,
        db_option_arc.clone(),
        sender.clone(),
        batch_size,
        batch_concurrency,
        all_prim_refnos.clone(),
    )
    .await?;

    // 7. 关闭 sender，等待入库任务完成
    drop(sender);
    insert_task.await.unwrap();

    println!(
        "[gen_full_noun_geos] 所有 Noun 任务完成，用时 {} ms",
        start_time.elapsed().as_millis()
    );

    // 8. 构建 DbModelInstRefnos
    let use_cate_vec: Vec<RefnoEnum> = all_use_cate_refnos.read().await.iter().copied().collect();
    let loop_owner_vec: Vec<RefnoEnum> = all_loop_owner_refnos.read().await.iter().copied().collect();
    let prim_vec: Vec<RefnoEnum> = all_prim_refnos.read().await.iter().copied().collect();

    println!(
        "[gen_full_noun_geos] 汇总结果: use_cate={}, loop_owner={}, prim={}",
        use_cate_vec.len(),
        loop_owner_vec.len(),
        prim_vec.len()
    );

    let db_refnos = DbModelInstRefnos {
        bran_hanger_refnos: Arc::new(vec![]), // Full Noun 模式下不处理 bran_hanger
        use_cate_refnos: Arc::new(use_cate_vec),
        loop_owner_refnos: Arc::new(loop_owner_vec),
        prim_refnos: Arc::new(prim_vec),
    };

    Ok(db_refnos)
}

#[derive(Clone)]
struct NounProcessContext {
    db_option: Arc<DbOption>,
    batch_size: usize,
    batch_concurrency: usize,
}

impl NounProcessContext {
    fn new(db_option: Arc<DbOption>, batch_size: usize, batch_concurrency: usize) -> Self {
        Self {
            db_option,
            batch_size,
            batch_concurrency: batch_concurrency.max(1),
        }
    }

    fn bounded_chunks(&self, total: usize) -> Vec<(usize, usize)> {
        if total == 0 {
            return vec![];
        }

        let chunk = self.batch_size.max(1);
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < total {
            let end = (start + chunk).min(total);
            ranges.push((start, end));
            start = end;
        }
        ranges
    }
}

async fn process_cate_nouns(
    collection: &FullNounCollection,
    db_option: Arc<DbOption>,
    loop_sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
    batch_size: usize,
    batch_concurrency: usize,
    refno_sink: Arc<RwLock<HashSet<RefnoEnum>>>,
) -> anyhow::Result<()> {
    let ctx = NounProcessContext::new(db_option, batch_size, batch_concurrency);
    let cate_nouns = collection.cate_nouns.clone();

    if cate_nouns.is_empty() {
        println!("[gen_full_noun_geos] cate nouns: 空列表，跳过");
        return Ok(());
    }

    let page_size = ctx.batch_size.max(1);

    let mut total_instances = 0usize;
    for &noun in cate_nouns.iter() {
        let total = count_noun_all_db(noun)
            .await
            .map_err(|e| anyhow!("统计 cate noun {} 失败: {}", noun, e))? as usize;

        if total == 0 {
            println!("[gen_full_noun_geos] cate noun {}: 无实例", noun);
            continue;
        }

        println!(
            "[gen_full_noun_geos] cate noun {}: 共 {} 个实例，分页大小 {}",
            noun, total, page_size
        );

        let mut processed = 0usize;
        while processed < total {
            let refnos = query_noun_page_all_db(noun, processed, page_size)
                .await
                .map_err(|e| anyhow!("分页查询 cate noun {} 失败: {}", noun, e))?;

            if refnos.is_empty() {
                break;
            }

            {
                let mut sink = refno_sink.write().await;
                sink.extend(refnos.iter().copied());
            }

            let page_index = processed / page_size + 1;
            println!(
                "[gen_full_noun_geos] cate noun {}: 处理第 {} 页 ({} ~ {})",
                noun,
                page_index,
                processed + 1,
                processed + refnos.len()
            );

            let batch_len = refnos.len();
            process_cate_refno_page(&ctx, loop_sjus_map_arc.clone(), sender.clone(), &refnos)
                .await?;

            processed += batch_len;
        }

        total_instances += total;
    }

    if total_instances == 0 {
        println!("[gen_full_noun_geos] cate nouns: 无实例");
    }

    Ok(())
}

async fn process_loop_nouns(
    collection: &FullNounCollection,
    db_option: Arc<DbOption>,
    loop_sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
    batch_size: usize,
    batch_concurrency: usize,
    refno_sink: Arc<RwLock<HashSet<RefnoEnum>>>,
) -> anyhow::Result<()> {
    let ctx = NounProcessContext::new(db_option, batch_size, batch_concurrency);
    let loop_nouns = collection.loop_owner_nouns.clone();

    if loop_nouns.is_empty() {
        println!("[gen_full_noun_geos] loop nouns: 空列表，跳过");
        return Ok(());
    }

    let page_size = ctx.batch_size.max(1);

    let mut total_instances = 0usize;
    for &noun in loop_nouns.iter() {
        let total = count_noun_all_db(noun)
            .await
            .map_err(|e| anyhow!("统计 loop noun {} 失败: {}", noun, e))? as usize;

        if total == 0 {
            println!("[gen_full_noun_geos] loop noun {}: 无实例", noun);
            continue;
        }

        println!(
            "[gen_full_noun_geos] loop noun {}: 共 {} 个实例，分页大小 {}",
            noun, total, page_size
        );

        let mut processed = 0usize;
        while processed < total {
            let refnos = query_noun_page_all_db(noun, processed, page_size)
                .await
                .map_err(|e| anyhow!("分页查询 loop noun {} 失败: {}", noun, e))?;

            if refnos.is_empty() {
                break;
            }

            {
                let mut sink = refno_sink.write().await;
                sink.extend(refnos.iter().copied());
            }

            let page_index = processed / page_size + 1;
            println!(
                "[gen_full_noun_geos] loop noun {}: 处理第 {} 页 ({} ~ {})",
                noun,
                page_index,
                processed + 1,
                processed + refnos.len()
            );

            let batch_len = refnos.len();
            process_loop_refno_page(&ctx, loop_sjus_map_arc.clone(), sender.clone(), &refnos)
                .await?;

            processed += batch_len;
        }

        total_instances += total;
    }

    if total_instances == 0 {
        println!("[gen_full_noun_geos] loop nouns: 无实例");
    }

    Ok(())
}

async fn process_prim_nouns(
    collection: &FullNounCollection,
    db_option: Arc<DbOption>,
    sender: flume::Sender<ShapeInstancesData>,
    batch_size: usize,
    batch_concurrency: usize,
    refno_sink: Arc<RwLock<HashSet<RefnoEnum>>>,
) -> anyhow::Result<()> {
    let ctx = NounProcessContext::new(db_option, batch_size, batch_concurrency);
    let prim_nouns = collection.prim_nouns.clone();

    if prim_nouns.is_empty() {
        println!("[gen_full_noun_geos] prim nouns: 空列表，跳过");
        return Ok(());
    }

    let page_size = ctx.batch_size.max(1);

    let mut total_instances = 0usize;
    for &noun in prim_nouns.iter() {
        let total = count_noun_all_db(noun)
            .await
            .map_err(|e| anyhow!("统计 prim noun {} 失败: {}", noun, e))? as usize;

        if total == 0 {
            println!("[gen_full_noun_geos] prim noun {}: 无实例", noun);
            continue;
        }

        println!(
            "[gen_full_noun_geos] prim noun {}: 共 {} 个实例，分页大小 {}",
            noun, total, page_size
        );

        let mut processed = 0usize;
        while processed < total {
            let refnos = query_noun_page_all_db(noun, processed, page_size)
                .await
                .map_err(|e| anyhow!("分页查询 prim noun {} 失败: {}", noun, e))?;

            if refnos.is_empty() {
                break;
            }

            {
                let mut sink = refno_sink.write().await;
                sink.extend(refnos.iter().copied());
            }

            let page_index = processed / page_size + 1;
            println!(
                "[gen_full_noun_geos] prim noun {}: 处理第 {} 页 ({} ~ {})",
                noun,
                page_index,
                processed + 1,
                processed + refnos.len()
            );

            let batch_len = refnos.len();
            process_prim_refno_page(&ctx, sender.clone(), &refnos).await?;

            processed += batch_len;
        }

        total_instances += total;
    }

    if total_instances == 0 {
        println!("[gen_full_noun_geos] prim nouns: 无实例");
    }

    Ok(())
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
        let sites = query_by_type(&["SITE"], dbno as i32, None).await?;
        process_meshes_update_db_deep(db_option, &sites)
            .await
            .expect("更新模型数据失败");
    }
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
        query_by_type(&["ZONE"], dbno as i32, Some(true))
            .await
            .unwrap_or_default()
    } else {
        // 使用当前数据查询
        query_by_type(&["ZONE"], dbno as i32, Some(true))
            .await
            .unwrap_or_default()
    };
    if zones.is_empty() {
        return Ok(Default::default());
    }
    // let mut all_handles = FuturesUnordered::new();

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
        let target_ploo_refnos = query_by_type(&["PLOO"], dbno as i32, Some(true))
            .await
            .unwrap_or_default();
        #[cfg(debug_assertions)]
        if !target_ploo_refnos.is_empty() {}
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
                    let off_z = crate::fast_model::gen_model::cate_helpers::cal_sjus_value(&sjus, height);
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
        Arc::new(query_by_type(&["BRAN", "HANG"], dbno as i32, None).await?);

    //打印管道/支吊架的使用数量
    if !target_bran_hanger_refnos.is_empty() && gen_cata_flag && gen_model {
        //查询出branch 和 branch 下的子节点
        let mut branch_refnos_map = DashMap::new();
        let mut bran_comp_eles = HashSet::new();
        for &refno in target_bran_hanger_refnos.as_slice() {
            // 使用新的泛型函数接口
            let children = aios_core::collect_children_elements(refno, &[])
                .await
                .unwrap_or_default();
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
                            debug_model_trace!("kv.value(): {:?}", kv.value());
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
            // });
            // all_handles.push(handle);
        }
    }
    let mut use_cate_refnos = vec![];
    for cate_names in USE_CATE_NOUN_NAMES.chunks(4) {
        let refnos = query_by_type(cate_names, dbno as i32, None).await?;
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
        debug_model_trace!(
            "target_single_cata_map.len(): {}",
            target_single_cata_map.len()
        );

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
            // });
            // all_handles.push(handle);
        }
    }

    let target_loop_owner_refnos = Arc::new(
        query_by_type(&GNERAL_LOOP_OWNER_NOUN_NAMES, dbno as i32, Some(true))
            .await
            .unwrap_or_default(),
    );
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
        query_by_type(&GNERAL_PRIM_NOUN_NAMES, dbno as i32, None)
            .await
            .unwrap_or_default(),
    );

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
    let mut all_handles = FuturesUnordered::new();
    // dbg!(&incr_updates);
    const CHUNK_SIZE: usize = 100;
    //根据需要拉入数据到本地数据库也可以
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    //排除增量更新的情况，如果debug_model_refnos 为空，即没有模型需要生成
    let debug_model_refnos = db_option.get_all_debug_refnos().await;
    let has_debug = !debug_model_refnos.is_empty();
    let skip_exist = !(db_option.is_replace_mesh() || has_manual_refnos || has_debug);
    println!("========== DEBUG: gen_geos_data ==========");
    println!(
        "debug_model_refnos 配置: {:?}",
        db_option.debug_model_refnos
    );
    println!("解析后的 debug_model_refnos: {:?}", debug_model_refnos);
    println!("debug_model_refnos 数量: {}", debug_model_refnos.len());
    println!(
        "is_incr_update: {}, has_manual_refnos: {}",
        is_incr_update, has_manual_refnos
    );
    debug_model_trace!("debug_model_refnos: {:?}", &debug_model_refnos);
    if !is_incr_update
        //debug_model_refnos = [] 时表示不生成模型，如果没有这个属性表示生成所有
        && (db_option.debug_model_refnos.is_some() && debug_model_refnos.is_empty())
        && (!has_manual_refnos)
    {
        println!("DEBUG: 没有模型需要生成，提前返回");
        return Ok(vec![]);
    }
    if is_incr_update && incr_updates.as_ref().unwrap().count() == 0 {
        return Ok(vec![]);
    }
    let db_option_arc = Arc::new(db_option.clone());
    let is_debug = debug_model_refnos.len() > 0;

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
            debug_model_refnos.clone()
        };
        debug_model_debug!(
            "DEBUG: 使用调试模式，target_root_refnos: {:?}",
            target_root_refnos
        );

        // 查询目标节点的基本信息
        for refno in &target_root_refnos {
            match aios_core::get_pe(*refno).await {
                Ok(Some(pe)) => {
                    debug_model_debug!("========== 目标节点详细信息 ==========");
                    debug_model_debug!("refno: {}", refno);
                    debug_model_debug!("noun: {}", pe.noun);
                    debug_model_debug!("name: {}", pe.name);
                    debug_model_debug!("cata_hash: {}", pe.cata_hash);
                    debug_model_debug!("owner: {:?}", pe.owner);

                    // 查询元件库关系
                    match aios_core::get_named_attmap(*refno).await {
                        Ok(att_map) => {
                            // 先检查是否有直接的 CATR 关系（如 NOZZ）
                            if let Some(catr_refno) = att_map.get_foreign_refno("CATR") {
                                debug_model_debug!("✅ 直接 CATR 关系: {}", catr_refno);
                                if let Some(catr_attr) = att_map.get_as_string("CATR") {
                                    debug_model_debug!("   CATR 属性原始值: {}", catr_attr);
                                }

                                // 查询 CATR 的详细信息
                                match aios_core::get_pe(catr_refno).await {
                                    Ok(Some(catr_pe)) => {
                                        debug_model_debug!(
                                            "   CATR noun: {}, name: {}",
                                            catr_pe.noun,
                                            catr_pe.name
                                        );
                                    }
                                    Ok(None) => {
                                        debug_model_debug!(
                                            "   ⚠️ 未找到 CATR 元素: {}",
                                            catr_refno
                                        );
                                    }
                                    Err(err) => {
                                        debug_model_debug!(
                                            "   ❌ 查询 CATR 元素失败 {}: {}",
                                            catr_refno,
                                            err
                                        );
                                    }
                                }
                            }
                            // 再检查是否有 SPRE 关系
                            else if let Some(spre_refno) = att_map.get_foreign_refno("SPRE") {
                                debug_model_debug!("SPRE refno: {}", spre_refno);

                                // 查询 SPRE 指向的 CATR
                                match aios_core::get_named_attmap(spre_refno).await {
                                    Ok(spre_att) => {
                                        if let Some(catr_refno) = spre_att.get_foreign_refno("CATR")
                                        {
                                            debug_model_debug!(
                                                "   通过 SPRE 的 CATR: {}",
                                                catr_refno
                                            );
                                        } else {
                                            debug_model_debug!("   ⚠️ SPRE 没有 CATR 关系");
                                        }
                                    }
                                    Err(err) => {
                                        debug_model_debug!(
                                            "   ❌ 查询 SPRE 属性失败 {}: {}",
                                            spre_refno,
                                            err
                                        );
                                    }
                                }
                            } else {
                                debug_model_debug!("⚠️ 没有 CATR 或 SPRE 关系");
                            }
                        }
                        Err(err) => {
                            debug_model_debug!("❌ 查询 attmap 失败 {}: {}", refno, err);
                        }
                    }
                }
                Ok(None) => {
                    debug_model_debug!("⚠️ 找不到元素 {}", refno);
                }
                Err(err) => {
                    debug_model_debug!("❌ 查询元素失败 {}: {}", refno, err);
                }
            }
        }
    } else if dbno.is_some() {
        // 检查是否需要进行历史查询
        if let Some(sesno) = target_sesno {
            println!(
                "使用历史查询，目标会话号: {} (注意：当前使用当前数据替代)",
                sesno
            );
            target_root_refnos = query_by_type(&["SITE"], dbno.unwrap() as i32, Some(true))
                .await?
                .into_iter()
                .collect();
        } else {
            // 使用当前数据查询
            target_root_refnos = query_by_type(&["SITE"], dbno.unwrap() as i32, Some(true))
                .await?
                .into_iter()
                .collect();
        }
    }
    if dbno.is_some() {
    } else {
    }
    let origin_root_refnos = target_root_refnos.clone();
    // let process_handle = tokio::spawn(async move {
    // let mut handles = vec![]
    if is_incr_update {
    } else if has_manual_refnos {
    } else if is_debug {
    } else if dbno.is_some() {
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

    debug_model_debug!("========== gen_geos_data 配置检查 ==========");
    debug_model_debug!("db_option_arc.gen_model: {}", db_option_arc.gen_model);
    debug_model_debug!("is_incr_update: {}", is_incr_update);
    debug_model_debug!("has_manual_refnos: {}", has_manual_refnos);
    debug_model_debug!("gen_model (最终值): {}", gen_model);
    debug_model_debug!("origin_root_refnos 数量: {}", origin_root_refnos.len());
    //遍历小块
    debug_model_debug!("========== 开始遍历 root_refnos 小块 ==========");
    debug_model_debug!("准备进入 while 循环");

    while gen_model && let Some(target_refnos) = chunked_root_refnos.next() {
        debug_model_debug!(
            "========== 处理一个小块，包含 {} 个节点 ==========",
            target_refnos.len()
        );
        debug_model_debug!("target_refnos: {:?}", target_refnos);

        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        {
            let Ok(target_ploo_refnos) = query_multi_descendants(target_refnos, &["PLOO"]).await
            else {
                continue;
            };
            #[cfg(debug_assertions)]
            if !target_ploo_refnos.is_empty() && is_e3d_debug_enabled() {
                debug_model_debug!("target_ploo_refnos: {:?}", target_ploo_refnos.len());
            }
            for r in target_ploo_refnos {
                let Ok(loop_att) = aios_core::get_named_attmap(r).await else {
                    continue;
                };
                let owner = loop_att.get_owner();
                let mut height = loop_att.get_f32("HEIG").unwrap_or_default();
                let sjus = loop_att.get_str("SJUS").unwrap_or_default();
                let off_z = crate::fast_model::gen_model::cate_helpers::cal_sjus_value(sjus, height);
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
            let r = query_multi_descendants(target_refnos, &["BRAN", "HANG"])
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
            let cata_map: DashMap<String, CataHashRefnoKV> = DashMap::new();
            let cata_refnos = &incr_updates_log_arc.basic_cata_refnos;
            //直接使用group的办法，按cata_hash 进行分组
            for &r in cata_refnos {
                if let Ok(Some(att)) = aios_core::get_pe(r).await {
                    let cata_hash = att.cata_hash.clone();
                    match cata_map.entry(cata_hash.clone()) {
                        Entry::Occupied(mut entry) => {
                            let value = entry.get_mut();
                            if !value.group_refnos.contains(&r) {
                                value.group_refnos.push(r);
                            }
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(CataHashRefnoKV {
                                cata_hash,
                                group_refnos: vec![r],
                                ..Default::default()
                            });
                        }
                    }
                }
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
                debug_model_debug!("[WARN] 查询BRAN, HANG出错");
                continue;
            };
            let single_refnos = target_refnos
                .iter()
                .filter(|x| !target_bran_hanger_refnos.contains(x))
                .map(|x| *x)
                .collect::<Vec<_>>();

            debug_model_debug!("========== 调试模式：查询子孙节点 ==========");
            debug_model_debug!("target_refnos: {:?}", target_refnos);
            debug_model_debug!(
                "target_bran_hanger_refnos: {:?}",
                &target_bran_hanger_refnos
            );
            debug_model_debug!("single_refnos: {:?}", &single_refnos);
            debug_model_debug!("single_refnos 数量: {}", single_refnos.len());

            use_cata_refnos =
                aios_core::query_deep_children_refnos_filter_spre(&single_refnos, skip_exist)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<HashSet<_>>();

            debug_model_debug!(
                "查询子孙节点后 use_cata_refnos 数量: {}",
                use_cata_refnos.len()
            );
            debug_model_debug!("use_cata_refnos: {:?}", &use_cata_refnos);

            use_cata_refnos.extend(bran_children_refnos);

            debug_model_debug!(
                "扩展 bran_children_refnos 后 use_cata_refnos 数量: {}",
                use_cata_refnos.len()
            );

            let map = aios_core::query_group_by_cata_hash(&use_cata_refnos)
                .await
                .unwrap_or_default();

            debug_model_debug!("query_group_by_cata_hash 返回的 map 数量: {}", map.len());
            for kv in map.iter() {
                debug_model_debug!(
                    "  cata_hash: {}, group_refnos: {:?}",
                    kv.key(),
                    kv.value().group_refnos
                );
            }
            map
        };
        //打印管道/支吊架的使用数量
        if !target_bran_hanger_refnos.is_empty() && gen_cata_flag {
            //查询出branch 和 branch 下的子节点
            let mut branch_refnos_map = DashMap::new();
            let mut bran_comp_eles = vec![];
            for &refno in &target_bran_hanger_refnos {
                // 使用新的泛型函数接口
                let children = aios_core::collect_children_elements(refno, &[])
                    .await
                    .unwrap_or_default();
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
                });
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
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
            let mut loop_owner_refnos =
                query_multi_descendants(target_refnos, &GNERAL_LOOP_OWNER_NOUN_NAMES)
                    .await
                    .unwrap_or_default();
            loop_owner_refnos.into_iter().collect()
        };
        if gen_loop_flag && !target_loop_owner_refnos.is_empty() {
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
            let mut prim_refnos = query_multi_descendants(target_refnos, &GNERAL_PRIM_NOUN_NAMES)
                .await
                .unwrap_or_default();
            debug_model_trace!("prim_refnos: {:?}", &prim_refnos);
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
        // 直接从 SurrealDB 查询元素类型
        let type_name = aios_core::get_type_name(refno)
            .await
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        Ok(Some(ElementInfo {
            name: Some(format!("元素-{}", refno)),
            type_name,
        }))
    }

    async fn get_shape_instances_data(
        &self,
        refno: RefnoEnum,
    ) -> anyhow::Result<Option<ShapeInstancesData>> {
        // 从 SurrealDB 查询inst_relate和inst_geo数据
        // 这里简化处理：如果需要数据，应该已经在步骤1生成并存入数据库
        // 旧的 XKT 生成逻辑会自行查询，这里返回 None
        Ok(None)
    }
}
