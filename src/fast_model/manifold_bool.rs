//! Manifold 布尔运算模块
//!
//! 本模块提供基于 Manifold 库的几何体布尔运算功能。
//! 所有布尔运算操作均使用 Manifold 库实现，不再依赖 OpenCASCADE。

use crate::fast_model::{debug_model, debug_model_debug, debug_model_trace};
use crate::{db_err, deser_err, log_err, query_err};
use aios_core::SurrealQueryExt;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::get_db_option;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{
    CataNegGroup, GmGeoData, NegInfo,
    query_cata_neg_boolean_groups, query_geom_mesh_data, query_negative_entities,
};
use aios_core::{
    RecordId, RefnoEnum, SUL_DB, gen_bytes_hash, get_inst_relate_keys, init_test_surreal,
    utils::RecordIdExt,
};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use glam::DMat4;
use nalgebra::Isometry;
use parry3d::bounding_volume::Aabb;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 根据 mesh_id 和当前 LOD 配置构建完整的 mesh 文件路径
///
/// # 参数
///
/// * `base_dir` - mesh 基础目录（通常是 DbOption.meshes_path 或其父目录）
/// * `mesh_id` - mesh 文件 ID
///
/// # 返回
///
/// 完整的 mesh 文件路径，格式为：
/// - `{base_dir}/lod_{LOD}/{mesh_id}_{LOD}.mesh`（启用 LOD 时）
/// - `{base_dir}/{mesh_id}.mesh`（无 LOD 或旧格式）
///
/// # 示例
///
/// ```ignore
/// let path = build_lod_mesh_path(Path::new("/assets/meshes"), "12232319344565648304");
/// // 返回: /assets/meshes/lod_L2/12232319344565648304_L2.mesh
/// ```
fn build_lod_mesh_path(base_dir: &Path, mesh_id: &str) -> PathBuf {
    use aios_core::mesh_precision::LodLevel;

    let db_option = get_db_option();
    let default_lod = db_option.mesh_precision().default_lod;

    // 检查 base_dir 是否已经是 LOD 子目录（如 "lod_L2"）
    let is_already_lod_dir = base_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.starts_with("lod_"))
        .unwrap_or(false);

    let lod_filename = format!("{}_{:?}.mesh", mesh_id, default_lod);

    if is_already_lod_dir {
        // 已经在 LOD 目录下，直接拼接文件名
        base_dir.join(lod_filename)
    } else {
        // 需要添加 LOD 子目录
        let lod_dir = base_dir.join(format!("lod_{:?}", default_lod));
        lod_dir.join(lod_filename)
    }
}

fn mesh_base_dir() -> PathBuf {
    get_db_option().get_meshes_path()
}

/// 从文件加载网格数据
///
/// # 参数
///
/// * `id` - 网格文件的ID
///
/// # 返回值
///
/// 返回 `anyhow::Result<PlantMesh>` 表示加载是否成功以及加载的网格数据
#[inline]
fn load_mesh(id: &str) -> anyhow::Result<PlantMesh> {
    let base_dir = mesh_base_dir();
    let mesh_path = build_lod_mesh_path(&base_dir, id);
    let mesh = PlantMesh::des_mesh_file(&mesh_path)?;
    Ok(mesh)
}

/// 从文件加载流形数据
///
/// # 参数
///
/// * `dir` - 模型文件目录路径
/// * `id` - 网格文件的ID
/// * `mat` - 变换矩阵
/// * `more_precision` - 是否需要更高精度
///
/// # 返回值
///
/// 返回 `anyhow::Result<ManifoldRust>` 表示加载是否成功以及加载的流形数据
#[inline]
fn load_manifold(id: &str, mat: DMat4, more_precision: bool) -> anyhow::Result<ManifoldRust> {
    let base_dir = mesh_base_dir();
    let mesh_path = build_lod_mesh_path(&base_dir, id);
    let mesh = PlantMesh::des_mesh_file(&mesh_path)?;
    let manifold = ManifoldRust::convert_to_manifold(mesh, mat, more_precision);
    Ok(manifold)
}


