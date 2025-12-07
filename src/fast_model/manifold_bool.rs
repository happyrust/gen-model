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
use aios_core::transform::get_local_transform;
use aios_core::{
    CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo, query_cata_neg_boolean_groups,
    query_geom_mesh_data, query_negative_entities_batch,
};
use aios_core::{RefnoEnum, SUL_DB, gen_bytes_hash, utils::RecordIdExt};
use bevy_transform::components::Transform;
use glam::{DMat4, DVec4, Mat4};
use serde_json;
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

#[inline]
fn transform_to_dmat4(t: &Transform) -> DMat4 {
    let m = t.to_matrix();
    DMat4::from_cols(
        DVec4::new(
            m.x_axis.x as f64,
            m.x_axis.y as f64,
            m.x_axis.z as f64,
            m.x_axis.w as f64,
        ),
        DVec4::new(
            m.y_axis.x as f64,
            m.y_axis.y as f64,
            m.y_axis.z as f64,
            m.y_axis.w as f64,
        ),
        DVec4::new(
            m.z_axis.x as f64,
            m.z_axis.y as f64,
            m.z_axis.z as f64,
            m.z_axis.w as f64,
        ),
        DVec4::new(
            m.w_axis.x as f64,
            m.w_axis.y as f64,
            m.w_axis.z as f64,
            m.w_axis.w as f64,
        ),
    )
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
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
                    "update {}<-inst_relate set bool_status='Failed';",
                    &g.inst_info_id.to_raw(),
                ));
                continue;
            };

            debug_model_debug!("加载 catalog 正实体 mesh: {}", pos.id.to_mesh_id());
            // 组合父子局部变换（get_local_transform）与几何体自身变换
            // let pos_local_mat = match get_local_transform(pos.geom_refno).await {
            //     Ok(Some(t)) => transform_to_dmat4(&t),
            //     _ => DMat4::IDENTITY,
            // };
            let pos_mat = pos.trans.0.to_matrix().as_dmat4();
            let Ok(mut pos_manifold) = load_manifold(&pos.id.to_mesh_id(), pos_mat, false) else {
                println!("布尔运算失败: 无法加载正实体 manifold, refno: {}", &g.refno);
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set bool_status='Failed';",
                    &g.inst_info_id.to_raw(),
                ));
                continue;
            };

            let mut neg_manifolds = Vec::new();
            for &neg in bg.iter().skip(1) {
                let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                    continue;
                };
                // 负实体 = get_local_transform(相对父) * 几何体 transform
                let neg_local_mat = match get_local_transform(neg_geo.geom_refno).await {
                    Ok(Some(t)) => transform_to_dmat4(&t),
                    _ => DMat4::IDENTITY,
                };
                let neg_mat = neg_local_mat * neg_geo.trans.0.to_matrix().as_dmat4();
                if let Ok(manifold) = load_manifold(&neg_geo.id.to_mesh_id(), neg_mat, true) {
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

                // 1. 创建布尔后的 Compound 几何体记录
                update_sql.push_str(&format!(
                    "CREATE inst_geo:⟨{}⟩ SET meshed = true, aabb = {};",
                    new_id,
                    &pos.aabb_id.to_raw()
                ));
                
                // 2. 创建 geo_relate 关系，类型为 Compound
                let relate_sql = format!(
                    "RELATE {}->geo_relate->inst_geo:⟨{}⟩ SET geom_refno=pe:⟨{}⟩, geo_type='Compound', trans=trans:⟨0⟩, visible=true;",
                    &g.inst_info_id.to_raw(),
                    new_id,
                    format!("{}_b", bg[0]),
                );
                update_sql.push_str(relate_sql.as_str());
                
                // 3. 更新原始 Pos 的 geo_relate，设置 booled_id 指向新的 Compound
                update_sql.push_str(&format!(
                    "UPDATE geo_relate SET booled_id = inst_geo:⟨{}⟩ WHERE out = {};",
                    new_id,
                    &pos.id.to_raw()
                ));
                
                // 4. 更新 inst_relate 状态（不设置 booled_id，只标记状态）
                if let Some(aabb) = mesh.cal_aabb() {
                    let aabb_hash = gen_bytes_hash(&aabb).to_string();
                    let aabb_json =
                        serde_json::to_string(&aabb).unwrap_or_else(|_| "{}".to_string());
                    update_sql.push_str(&format!(
                        "INSERT IGNORE INTO aabb {{ 'id': aabb:⟨{}⟩, 'd': {} }};",
                        aabb_hash, aabb_json
                    ));
                    update_sql.push_str(&format!(
                        "UPDATE {}<-inst_relate SET bool_status='Success', aabb = aabb:⟨{}⟩;",
                        &g.inst_info_id.to_raw(),
                        aabb_hash,
                    ));
                } else {
                    update_sql.push_str(&format!(
                        "UPDATE {}<-inst_relate SET bool_status='Success';",
                        &g.inst_info_id.to_raw(),
                    ));
                }
            } else {
                update_sql.push_str(&format!(
                    "update {}<-inst_relate set bool_status='Failed';",
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
    // inst_relate 是关系表，需要用 WHERE in = pe:xxx 来更新
    let pe_key = query.refno.to_pe_key();

    // 非替换模式下，已有 booled_id 则跳过
    if !replace_exist {
        let check_sql = format!("SELECT value booled_id FROM inst_relate WHERE in = {} LIMIT 1", pe_key);
        if let Ok(Some(existing)) = SUL_DB.query_take::<Option<String>>(&check_sql, 0).await {
            if !existing.is_empty() {
                return Ok(());
            }
        }
    }

    // 使用正实体的世界坐标系作为基准坐标系
    let inst_world_mat = query.inst_world_trans.0.to_matrix().as_dmat4();

    let mut pos_manifolds = Vec::new();
    for (pos_id, geo_local_trans) in query.pos_geos.iter() {
        let pos_mesh_id = pos_id.to_mesh_id();
        // 正实体使用其局部变换（相对于 inst_relate 的变换）
        let geo_local_mat = geo_local_trans.0.to_matrix().as_dmat4();
        debug_model_debug!(
            "加载正实体 mesh: {} (应用局部变换)",
            pos_mesh_id
        );
        if let Ok(manifold) = load_manifold(&pos_mesh_id, geo_local_mat, false) {
            pos_manifolds.push(manifold);
        }
    }

    if pos_manifolds.is_empty() {
        println!(
            "布尔运算失败: 未找到正实体 manifold，refno: {}, 正几何数量={}",
            query.refno,
            query.pos_geos.len()
        );
        let sql = format!("UPDATE inst_relate SET bool_status='Failed' WHERE in = {};", pe_key);
        SUL_DB.query(sql).await?;
        return Ok(());
    }

    let mut pos_manifold = ManifoldRust::batch_boolean(&pos_manifolds, 0);
    if pos_manifold.num_tri() == 0 {
        println!(
            "布尔运算失败: 正实体 manifold 没有三角形, refno: {}",
            query.refno
        );
        let sql = format!("UPDATE inst_relate SET bool_status='Failed' WHERE in = {};", pe_key);
        SUL_DB.query(sql).await?;
        return Ok(());
    }

    // 计算实例世界坐标系的逆矩阵，用于将负实体转换到实例的相对坐标系
    let inverse_inst_world = inst_world_mat.inverse();
    let mut neg_manifolds = Vec::new();
    for (_, _, neg_infos) in query.neg_ts.iter() {
        for NegInfo {
            id,
            geo_local_trans,
            aabb,
            carrier_world_trans,
            ..
        } in neg_infos.iter().cloned()
        {
            if aabb.is_none() {
                continue;
            }

            // 获取负载体的世界坐标变换
            // 如果没有 carrier_world_trans，使用单位矩阵（兼容旧数据）
            let carrier_world_mat = carrier_world_trans
                .map(|wt| wt.0.to_matrix().as_dmat4())
                .unwrap_or(glam::DMat4::IDENTITY);

            // 计算负实体相对于实例坐标系的变换矩阵
            // 相对变换 = inverse(实例世界坐标) × 负实体世界坐标
            // 负实体世界坐标 = carrier_world_mat × geo_local_trans
            let neg_world_mat = carrier_world_mat * geo_local_trans.0.to_matrix().as_dmat4();
            let relative_mat = inverse_inst_world * neg_world_mat;

            debug_model_debug!("加载负实体 mesh: {} (相对于实例坐标系)", id.to_mesh_id());

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
        let sql = format!("UPDATE inst_relate SET bool_status='Failed' WHERE in = {};", pe_key);
        SUL_DB.query(sql).await?;
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

        // 获取 inst_info_id（geo_relate 的 in 端）
        let inst_info_sql = format!("SELECT value string::concat('inst_info:', record::id(out)) FROM inst_relate WHERE in = {} LIMIT 1", pe_key);
        let inst_info_id: Option<String> = SUL_DB.query_take(&inst_info_sql, 0).await.ok().flatten();
        
        let mut update_sql = String::new();
        
        // 1. 创建布尔后的 Compound 几何体记录
        update_sql.push_str(&format!(
            "CREATE inst_geo:⟨{}⟩ SET meshed = true;",
            mesh_id
        ));
        
        // 2. 创建 geo_relate 关系，类型为 Compound，trans 为单位变换
        if let Some(ref info_id) = inst_info_id {
            update_sql.push_str(&format!(
                "RELATE {}->geo_relate->inst_geo:⟨{}⟩ SET geom_refno=pe:⟨{}_b⟩, geo_type='Compound', trans=trans:⟨0⟩, visible=true;",
                info_id,
                mesh_id,
                query.refno
            ));
        }
        
        // 3. 更新所有参与布尔的原始 Pos 的 geo_relate，设置 booled_id 指向新的 Compound
        for (pos_id, _) in query.pos_geos.iter() {
            update_sql.push_str(&format!(
                "UPDATE geo_relate SET booled_id = inst_geo:⟨{}⟩ WHERE out = {};",
                mesh_id,
                pos_id.to_raw()
            ));
        }
        
        // 4. 更新 inst_relate 状态（不设置 booled_id，只标记状态）
        if let Some(aabb) = mesh.cal_aabb() {
            let aabb_hash = gen_bytes_hash(&aabb).to_string();
            let aabb_json = serde_json::to_string(&aabb).unwrap_or_else(|_| "{}".to_string());
            update_sql.push_str(&format!(
                "INSERT IGNORE INTO aabb {{ 'id': aabb:⟨{}⟩, 'd': {} }};",
                aabb_hash, aabb_json
            ));
            update_sql.push_str(&format!(
                "UPDATE inst_relate SET bool_status='Success', aabb = aabb:⟨{}⟩ WHERE in = {};",
                aabb_hash, pe_key
            ));
        } else {
            update_sql.push_str(&format!(
                "UPDATE inst_relate SET bool_status='Success' WHERE in = {};",
                pe_key
            ));
        }
        
        debug_model!("[布尔更新] 执行 SQL: {}", update_sql);
        SUL_DB.query(&update_sql).await?;
        
        println!("布尔运算完成: refno={} mesh={}", query.refno, mesh_id);
        // 验证更新是否成功
        let verify_sql = format!("SELECT bool_status FROM inst_relate WHERE in = {}", pe_key);
        if let Ok(result) = SUL_DB.query(&verify_sql).await {
            debug_model!("[布尔验证] 查询结果: {:?}", result);
        }
        return Ok(());
    }

    println!("布尔运算失败: 无法保存结果 mesh, refno: {}", query.refno);
    let sql = format!("UPDATE inst_relate SET bool_status='Failed' WHERE in = {};", pe_key);
    SUL_DB.query(sql).await?;
    Ok(())
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
