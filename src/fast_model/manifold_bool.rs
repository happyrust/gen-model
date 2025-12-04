//! Manifold 布尔运算模块
//!
//! 本模块提供基于 Manifold 库的几何体布尔运算功能。
//! 所有布尔运算操作均使用 Manifold 库实现，不再依赖 OpenCASCADE。

use crate::fast_model::{debug_model, debug_model_debug};
use aios_core::SurrealQueryExt;
use aios_core::csg::manifold::ManifoldRust;
use aios_core::get_db_option;
use aios_core::rs_surreal::boolean_query_optimized::query_manifold_boolean_operations_batch_optimized;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{
    CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo, query_cata_neg_boolean_groups,
    query_geom_mesh_data, query_negative_entities_batch,
};
use aios_core::{RefnoEnum, SUL_DB, utils::RecordIdExt};
use glam::DMat4;
use std::path::{Path, PathBuf};
use std::{fs, io};

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

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

async fn mark_bad_bool(inst_relate_id: &str) -> anyhow::Result<()> {
    let sql = format!("update {} set bad_bool=true;", inst_relate_id);
    SUL_DB.query(sql).await?;
    Ok(())
}

async fn update_booled_id(inst_relate_id: &str, mesh_id: &str) -> anyhow::Result<()> {
    let sql = format!("update {} set booled_id='{}';", inst_relate_id, mesh_id);
    SUL_DB.query(sql).await?;
    Ok(())
}

fn boolean_mesh_path(mesh_id: &str) -> PathBuf {
    build_lod_mesh_path(&mesh_base_dir(), mesh_id)
}

fn boolean_obj_path(mesh_id: &str) -> PathBuf {
    let mut path = boolean_mesh_path(mesh_id);
    path.set_extension("obj");
    path
}

/// 处理元件库负实体布尔（catalog 级别）
pub async fn apply_cata_neg_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()> {
    let params = query_cata_neg_boolean_groups(refnos, replace_exist).await?;
    if params.is_empty() {
        return Ok(());
    }

    for g in params {
        // 收集当前实例涉及的所有几何，批量查询 mesh 数据
        let geom_refnos: Vec<RefnoEnum> = g.boolean_group.iter().flatten().cloned().collect();
        let gms: Vec<GmGeoData> = query_geom_mesh_data(g.refno, &geom_refnos).await?;

        let mut update_sql = String::new();
        for bg in g.boolean_group {
            let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set bad_bool=true;",
                    &g.inst_info_id.to_raw(),
                ));
                continue;
            };

            debug_model_debug!("加载 catalog 正实体 mesh: {}", pos.id.to_mesh_id());
            let Ok(mut pos_manifold) = load_manifold(
                &pos.id.to_mesh_id(),
                pos.trans.0.to_matrix().as_dmat4(),
                false,
            ) else {
                println!("布尔运算失败: 无法加载正实体 manifold, refno: {}", &g.refno);
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set bad_bool=true;",
                    &g.inst_info_id.to_raw(),
                ));
                continue;
            };

            let mut neg_manifolds = Vec::new();
            for &neg in bg.iter().skip(1) {
                let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                    continue;
                };
                let m = neg_geo.trans.0.to_matrix().as_dmat4();
                if let Ok(manifold) = load_manifold(&neg_geo.id.to_mesh_id(), m, true) {
                    neg_manifolds.push(manifold);
                }
            }

            // 即使没有负实体，也标记已处理，避免重复计算
            let new_id = g.refno.hash_with_another_refno(bg[0]);
            let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
            let mesh = PlantMesh::from(&final_manifold);
            let target_path = boolean_mesh_path(&new_id.to_string());
            ensure_parent_dir(&target_path)?;

            if mesh.ser_to_file(&target_path).is_ok() {
                let obj_path = boolean_obj_path(&new_id.to_string());
                if let Err(e) = mesh.export_obj(false, obj_path.to_string_lossy().as_ref()) {
                    eprintln!("导出 OBJ 失败: refno={} err={}", g.refno, e);
                }

                update_sql.push_str(&format!(
                    "create inst_geo:⟨{}⟩ set meshed = true, aabb = {};",
                    new_id,
                    &pos.aabb_id.to_raw()
                ));
                let relate_sql = format!(
                    "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:⟨{}⟩, geo_type='Pos', trans=trans:⟨0⟩, visible = true;",
                    &g.inst_info_id.to_raw(),
                    new_id,
                    format!("{}_b", bg[0]),
                );
                update_sql.push_str(relate_sql.as_str());
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set booled=true;",
                    &g.inst_info_id.to_raw(),
                ));
            } else {
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set bad_bool=true;",
                    &g.inst_info_id.to_raw(),
                ));
            }
        }

        if !update_sql.is_empty() {
            SUL_DB.query(update_sql).await?;
        }
    }

    debug_model!("元件库的负实体计算{:?}完成", refnos);
    Ok(())
}

async fn apply_boolean_for_query(
    query: ManiGeoTransQuery,
    replace_exist: bool,
) -> anyhow::Result<()> {
    let inst_relate_id = query.refno.to_table_key("inst_relate");

    // 非替换模式下，已有 booled_id 则跳过
    if !replace_exist {
        let check_sql = format!("select value booled_id from {} limit 1", inst_relate_id);
        if let Ok(Some(existing)) = SUL_DB.query_take::<Option<String>>(&check_sql, 0).await {
            if !existing.is_empty() {
                return Ok(());
            }
        }
    }

    // 使用正实体的世界坐标系作为基准坐标系
    // 正实体在基准坐标系中，使用单位矩阵（相对于自身的坐标系）
    let pos_world_mat = query.wt.0.to_matrix().as_dmat4();

    let mut pos_manifolds = Vec::new();
    for (pos_id, pos_t) in query.ts.iter() {
        let pos_mesh_id = pos_id.to_mesh_id();
        debug_model_debug!(
            "加载正实体 mesh: {} (使用单位矩阵，基准坐标系)",
            pos_mesh_id
        );
        // 正实体使用单位矩阵，因为它定义了基准坐标系
        if let Ok(manifold) = load_manifold(&pos_mesh_id, glam::DMat4::IDENTITY, false) {
            pos_manifolds.push(manifold);
        }
    }

    if pos_manifolds.is_empty() {
        println!(
            "布尔运算失败: 未找到正实体 manifold，refno: {}, 正几何数量={}",
            query.refno,
            query.ts.len()
        );
        mark_bad_bool(&inst_relate_id).await?;
        return Ok(());
    }

    let mut pos_manifold = ManifoldRust::batch_boolean(&pos_manifolds, 0);
    if pos_manifold.num_tri() == 0 {
        println!(
            "布尔运算失败: 正实体 manifold 没有三角形, refno: {}",
            query.refno
        );
        mark_bad_bool(&inst_relate_id).await?;
        return Ok(());
    }

    // 计算正实体世界坐标系的逆矩阵，用于将负实体转换到正实体的相对坐标系
    let inverse_pos_world = pos_world_mat.inverse();
    let mut neg_manifolds = Vec::new();
    for (_, carrier_wt, neg_infos) in query.neg_ts.iter() {
        // 负实体载体的世界坐标变换
        let carrier_world_mat = carrier_wt.0.to_matrix().as_dmat4();

        for NegInfo {
            id, trans, aabb, ..
        } in neg_infos.iter().cloned()
        {
            if aabb.is_none() {
                continue;
            }

            // 计算负实体相对于正实体坐标系的变换矩阵
            // 相对变换 = inverse(正实体世界坐标) × 负实体世界坐标
            // 负实体世界坐标 = carrier_world_mat × trans
            let neg_world_mat = carrier_world_mat * trans.0.to_matrix().as_dmat4();
            let relative_mat = inverse_pos_world * neg_world_mat;

            debug_model_debug!("加载负实体 mesh: {} (相对于正实体坐标系)", id.to_mesh_id());

            if let Ok(manifold) = load_manifold(&id.to_mesh_id(), relative_mat, true) {
                neg_manifolds.push(manifold);
            }
        }
    }

    if neg_manifolds.is_empty() {
        println!(
            "布尔运算失败: 未找到负实体 manifold，refno: {}, neg 载体数={}",
            query.refno,
            query.neg_ts.len()
        );
        mark_bad_bool(&inst_relate_id).await?;
        return Ok(());
    }

    let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
    let mesh = PlantMesh::from(&final_manifold);
    let mesh_id = if query.sesno == 0 {
        query.refno.to_string()
    } else {
        format!("{}_{}", query.refno, query.sesno)
    };
    let target_path = boolean_mesh_path(&mesh_id);
    ensure_parent_dir(&target_path)?;

    if mesh.ser_to_file(&target_path).is_ok() {
        let obj_path = boolean_obj_path(&mesh_id);
        if let Err(e) = mesh.export_obj(false, obj_path.to_string_lossy().as_ref()) {
            eprintln!("导出 OBJ 失败: refno={} err={}", query.refno, e);
        }

        update_booled_id(&inst_relate_id, &mesh_id).await?;
        debug_model!("布尔运算完成: refno={} mesh={}", query.refno, mesh_id);
        return Ok(());
    }

    println!("布尔运算失败: 无法保存结果 mesh, refno: {}", query.refno);
    mark_bad_bool(&inst_relate_id).await
}

/// 对多个实例进行布尔运算（使用 Manifold，新查询流程）
pub async fn apply_insts_boolean_manifold(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    // 先用新的批量 API 筛选出存在负实体的实例
    let neg_mapping = query_negative_entities_batch(refnos).await?;
    let targets: Vec<RefnoEnum> = neg_mapping
        .into_iter()
        .filter_map(|(pos, negs)| if negs.is_empty() { None } else { Some(pos) })
        .collect();

    if targets.is_empty() {
        debug_model!("没有需要布尔运算的实例，输入 {} 个", refnos.len());
        return Ok(());
    }

    let queries: Vec<ManiGeoTransQuery> =
        query_manifold_boolean_operations_batch_optimized(&targets).await?;
    println!(
        "布尔任务数量: {} (targets={})",
        queries.len(),
        targets.len()
    );
    if queries.is_empty() {
        debug_model!("未查询到布尔运算参数，跳过");
        return Ok(());
    }

    for query in queries {
        apply_boolean_for_query(query, replace_exist).await?;
    }

    Ok(())
}
