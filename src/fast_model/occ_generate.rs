use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use crate::fast_model::{EXIST_MESH_GEO_HASHES, utils};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::basic::OccSharedShape;
// Removed GLOBAL_AABB_TREE dependency - using SQLite R*-tree instead
use crate::spatial_index::SqliteSpatialIndex;
use aios_core::shape::pdms_shape::{PlantMesh, RsVec3};
use aios_core::tool::float_tool::{dvec4_round_3, f64_round};
use aios_core::{
    RefnoEnum, SUL_DB, gen_bytes_hash, get_inst_relate_keys, query_deep_neg_inst_refnos,
    query_deep_visible_inst_refnos,
};
use aios_core::{get_db_option, init_test_surreal};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::DMat4;
use itertools::Itertools;
use opencascade::primitives::IntoShape;
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use parse_pdms_db::parse::round_f32;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use surrealdb::sql::Thing;

///生成小的几何体
#[tokio::test]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    process_meshes_update_db_deep_default((&["17496/171559".into(), "24381/35844".into()]))
        .await
        .unwrap();
    Ok(())
}

///生成模型的部分，update aabb
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
    for chunk in refnos.chunks(100) {
        // 生成模型文件
        gen_inst_meshes(chunk, replace_exist, dir.clone())
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
        //有一些布尔运算要精确计算，不然会有薄片出现
        //生成负实体的布尔运算
        // apply_insts_boolean_occ(&refnos, replace_exist, dir.clone()).await?;
        // println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());
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
    // dbg!(&target_refnos);
    // 生成模型文件
    gen_inst_meshes(&refnos, replace_exist, dir.clone())
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
    apply_insts_boolean_manifold(&refnos, replace_exist, dir.clone()).await?;
    //有一些布尔运算要精确计算，不然会有薄片出现
    //生成负实体的布尔运算
    // apply_insts_boolean_occ(&refnos, replace_exist, dir.clone()).await?;
    // println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());

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
        // dbg!(refnos.len());
        println!("更新模型结点数量: {}", refnos.len());
        let time = std::time::Instant::now();
        for &refno in refnos {
            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
            println!("更新模型结点: {}", refno);
            let mut target_visible_refnos = vec![];
            let mut update_refnos = query_deep_visible_inst_refnos(refno)
                .await
                .unwrap_or_default();
            target_visible_refnos.extend(update_refnos.clone());
            // dbg!(&target_visible_refnos);

            let neg_refnos = query_deep_neg_inst_refnos(refno).await.unwrap_or_default();
            update_refnos.extend(neg_refnos);

            // #[cfg(any(feture = "debug_model", feature = "debug_model_no_obj"))]
            if update_refnos.is_empty() {
                continue;
            }

            println!("实际需要更新模型结点数量: {}", update_refnos.len());
            //缩小范围
            if dboption.gen_mesh {
                // dbg!(&target_refnos);
                // 生成模型文件
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                let time = std::time::Instant::now();
                gen_inst_meshes(&update_refnos, replace_exist, dir.clone())
                    .await
                    .unwrap();
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                println!(
                    "gen_inst_meshes finished: {} ms",
                    time.elapsed().as_millis()
                );
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                let time = std::time::Instant::now();
                // 更新aabb 到inst relate，geo relate
                update_inst_relate_aabbs_by_refnos(&update_refnos, replace_exist)
                    .await
                    .unwrap();
                #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                println!(
                    "update_inst_relate_aabbs finished: {} ms",
                    time.elapsed().as_millis()
                );
            }

            if target_visible_refnos.is_empty() {
                continue;
            }

            if dboption.apply_boolean_operation {
                // apply_cata_neg_boolean_occ(None).await.unwrap();
                // dbg!(target_visible_refnos.len());
                let time = std::time::Instant::now();
                //生成元件库内部几何体的负实体运算
                apply_cata_neg_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                apply_insts_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                //有一些布尔运算要精确计算，不然会有薄片出现
                //生成负实体的布尔运算
                // apply_insts_boolean_occ(&target_visible_refnos, replace_exist, dir.clone()).await?;
            }
        }
        println!("布尔运算花费时间: {} ms", time.elapsed().as_millis());
    }
    Ok(())
}

/// 几何参数查询结构体
/// - id: inst_geo 的原始记录 ID（来自 SurrealDB：record::id(id) 字符串化）
/// - param: PDMS 几何参数（用于生成 OCC 形体与后续网格化）
/// 用于分批查询 inst_geo 的几何参数，配合 gen_inst_meshes 的并发处理
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryGeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
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
) -> anyhow::Result<()> {
    // 每批并发处理的 inst_geo 数量上限，控制单批任务规模
    const PAGE_NUM: usize = 100;
    // 计数/调试用途（目前未外显）
    let mut i = 0;
    // 根据 refnos 生成 inst_relate 的键集合（SurrealDB 查询范围）
    let inst_keys = get_inst_relate_keys(refnos);

    // 根据 replace_exist 决定是否跳过已生成或异常的几何：
    // - replace_exist=true：不过滤 aabb/meshed，允许覆盖，但仍过滤 bad
    // - replace_exist=false：仅选择 aabb 为空、未网格化且非 bad 的几何
    // 同时保留 ($parent<-neg_relate)[0] != none 的标记，用于后续容差调整
    let sql = if replace_exist {
        format!(
            r#"array::group(select value (select value [out, ($parent<-neg_relate)[0] != none] from out->geo_relate where  !out.bad)
            from {inst_keys})"#
        )
    } else {
        format!(
            r#"array::group(select value (select value [out, ($parent<-neg_relate)[0] != none] from out->geo_relate where out.aabb.d=none and !out.meshed and !out.bad)
            from {inst_keys})"#
        )
    };
    // 执行查询，返回 (inst_geo Thing, 是否存在负实体) 的二元组列表
    let mut response = SUL_DB.query(sql).await.unwrap();
    let mut inst_geo_ids: Vec<(Option<Thing>, bool)> = response.take(0).unwrap();
    // 进一步过滤：当不覆盖时，跳过内存缓存中已存在网格的几何（减少重复计算）
    // let mut update_geos_by_meshes = HashSet::default();
    inst_geo_ids.retain(|(x, _y)| {
        if let Some(t) = x {
            if replace_exist {
                true
            } else {
                if EXIST_MESH_GEO_HASHES.contains_key(&t.id.to_raw()) {
                    // update_geos_by_meshes.insert(t.id.to_raw());
                    false
                } else {
                    true
                }
            }
        } else {
            false
        }
    });
    // 无可处理对象则直接返回
    if inst_geo_ids.is_empty() {
        return Ok(());
    }
    // 记录每个几何是否具备负实体关系（影响容差选择与布尔精度）
    let thing_has_neg_map = inst_geo_ids
        .iter()
        .map(|(x, y)| (x.as_ref().unwrap().id.to_raw(), *y))
        .collect::<HashMap<_, _>>();
    let thing_has_neg_map_arc = Arc::new(thing_has_neg_map);

    let mut tasks = vec![];
    // 线程安全缓存：aabb_map 用于累积 aabb；pts_json_map 用于存储端点 JSON（去重）
    let aabb_map = Arc::new(DashMap::new());
    let pts_json_map = Arc::new(DashMap::new());

    // 分批并发处理 inst_geo
    for (_idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
        // 将本批次 inst_geo id 合并为 SurrealDB in 子查询集合
        let ids = chunk
            .into_iter()
            .map(|(x, _)| x.as_ref().unwrap().to_string())
            .join(",");
        // 克隆所需上下文到异步任务中
        let thing_neg_map = thing_has_neg_map_arc.clone();
        let dir = dir.clone();
        let aabb_map = aabb_map.clone();
        let pts_json_map = pts_json_map.clone();
        // 每批一个异步任务：查询参数 -> 构造形体 -> 网格化 -> 回写
        let task = tokio::spawn(async move {
            // shapes_map: 缓存 (几何hash -> (OCC形体, 容差))，统一批量网格化
            let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();
            // 查询本批所有 inst_geo 的参数
            let sql = format!(
                "select <string> record::id(id) as id, param from [{}] where param != NONE",
                ids
            );
            match SUL_DB.query(&sql).await {
                Ok(mut response) => {
                    let r = response.take::<Vec<QueryGeoParam>>(0);
                    // 反序列化失败：记录错误并跳过本批
                    if let Err(e) = &r {
                        init_deserialize_error(
                            "Vec<QueryGeoParam>",
                            e,
                            &sql,
                            &std::panic::Location::caller().to_string(),
                        );
                        return;
                    }
                    let result: Vec<QueryGeoParam> = r.unwrap();
                    if result.is_empty() {
                        return;
                    }
                    i += 1;
                    // 遍历每个几何参数并构造 OCC 形体
                    for g in result {
                        #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                        println!("gen mesh param: {}", &g.id);
                        // PrimPolyhedron 采用固定较小容差（面片模型）
                        let is_polyhedron = match &g.param {
                            PdmsGeoParam::PrimPolyhedron(_) => true,
                            _ => false,
                        };
                        match g.param.gen_occ_shape() {
                            Ok(shape) => {
                                // 基于边的采样近似计算 aabb，用于估算容差尺度
                                let mut aabb = Aabb::new_invalid();
                                for edge in shape.edges() {
                                    for point in edge.approximation_segments_custom(2.0, 2.0) {
                                        aabb.take_point(nalgebra::Point3::new(
                                            point.x as f32,
                                            point.y as f32,
                                            point.z as f32,
                                        ));
                                    }
                                }
                                // 负实体或参与布尔的母体需要更严格容差（避免薄片/空洞）
                                let mut coeff = 0.005;
                                if thing_neg_map.get(&g.id).copied().unwrap_or(false) {
                                    match g.param {
                                        // 拉伸/旋转体对布尔结果较敏感，进一步减小容差
                                        PdmsGeoParam::PrimExtrusion(_)
                                        | PdmsGeoParam::PrimRevolution(_) => {
                                            coeff /= 10.0;
                                        }
                                        _ => {
                                            coeff /= 5.0;
                                        }
                                    };
                                }

                                // 计算容差：
                                // - 多面体固定 0.01
                                // - 其他类型按 aabb 尺度 * 系数，上限 50.0（防止异常放大）
                                let mut tol = if is_polyhedron {
                                    0.01
                                } else {
                                    (aabb.half_extents().magnitude() as f64 * coeff).min(50.0)
                                };
                                shapes_map.insert(g.id, (shape, tol));
                            }
                            Err(e) => {
                                // 仅在启用日志特性时打印影响范围，便于排障
                                #[cfg(feature = "log_error")]
                                {
                                    let failed_refnos =
                                        aios_core::query_refnos_by_geo_hash(&g.id).await.unwrap();
                                    println!("{:?} mesh error: {}", failed_refnos, e.to_string());
                                }
                            }
                        }
                    }
                    // 批量回写语句缓冲
                    let mut update_sql = "".to_string();

                    // 执行网格化与落盘，并构建回写 SQL
                    for (id, (s, tol)) in &shapes_map {
                        let mut m_tol = *tol;
                        let mut success = false;
                        #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                        println!("gen mesh hash: {}", id);
                        match PlantMesh::gen_occ_mesh(s, m_tol) {
                            Ok(mesh) => {
                                if mesh.aabb.is_none() {
                                    // 无有效 aabb 直接跳过
                                    continue;
                                }
                                #[cfg(feature = "debug_model")]
                                mesh.export_obj(false, &format!("{}.obj", id));
                                // 保存 .mesh 文件
                                if mesh.ser_to_file(&dir.join(format!("{}.mesh", id))).is_ok() {
                                    #[cfg(feature = "debug_model")]
                                    mesh.export_obj(false, &format!("{}.obj", id));
                                    // 生成 aabb/pts 哈希并去重缓存
                                    let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                                    let mut pt_hashes = HashSet::new();
                                    for edge in s.edges() {
                                        // 采样端点即可（中点可选 TODO），降低点集规模
                                        for point in [edge.start_point(), edge.end_point()] {
                                            let pts_hash = RsVec3(point.as_vec3()).gen_hash();
                                            pt_hashes.insert(format!("vec3:⟨{}⟩", pts_hash));
                                            if !pts_json_map.contains_key(&pts_hash) {
                                                pts_json_map.insert(
                                                    pts_hash,
                                                    serde_json::to_string(&point).unwrap(),
                                                );
                                            }
                                        }
                                    }
                                    // 构建回写：标记已网格化、绑定 aabb、记录涉及的点集引用
                                    update_sql.push_str(&format!(
                                        "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩, pts=[{}];",
                                        id,
                                        aabb_hash,
                                        pt_hashes.into_iter().join(","),
                                    ));
                                    // 记录 aabb 实体（统一批量保存）
                                    aabb_map
                                        .entry(aabb_hash.to_string())
                                        .or_insert(mesh.aabb.unwrap());
                                    success = true;
                                }
                            }
                            // 网格化失败：仅在 debug 特性下打印受影响 refnos
                            Err(e) => {
                                #[cfg(any(
                                    feature = "debug_model",
                                    feature = "debug_model_no_obj"
                                ))]
                                {
                                    let failed_refnos =
                                        aios_core::query_refnos_by_geo_hash(id).await.unwrap();
                                    println!("{:?} mesh error: {}", failed_refnos, e.to_string());
                                }
                            }
                        }
                        if !success {
                            // 标记 bad，避免后续重复尝试；可另行排障后再清理此标记
                            update_sql.push_str(&format!("update inst_geo:⟨{}⟩ set bad=true;", id));
                        }
                    }
                    if !update_sql.is_empty() {
                        // 批量回写 SurrealDB（使用一个语句拼接多条 update）
                        if let Err(_) = SUL_DB.query(&update_sql).await {
                            init_save_database_error(
                                &update_sql,
                                &std::panic::Location::caller().to_string(),
                            );
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
    for (id, _) in inst_geo_ids {
        if let Some(id) = id {
            let h = id.to_raw();
            if !EXIST_MESH_GEO_HASHES.contains_key(&h) {
                if let Some(aabb) = aabb_map.get(&h) {
                    EXIST_MESH_GEO_HASHES.insert(h, *aabb);
                }
            }
        }
    }

    // 批量持久化点集与 aabb 实体
    utils::save_pts_to_surreal(&pts_json_map).await;
    utils::save_aabb_to_surreal(&aabb_map).await;

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: Thing,
    pub refno: RefnoEnum,
    pub noun: String,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    pub world_trans: Transform,
}

/// 查询 inst_relate 的 AABB 计算所需字段

/// 单个几何的变换与局部 AABB
/// - trans: 从几何到实例的局部变换
/// - aabb: 几何的局部包围盒
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    pub trans: Transform,
    pub aabb: Aabb,
}

///刷新inst_relate 的 aabb
/// 更新实例关联的包围盒数据
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的包围盒数据
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示更新是否成功
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<()> {
    const CHUNK: usize = 100;
    // SQL 说明：
    // - world_trans.d != none：仅处理拥有世界变换的实例
    // - 子查询 out->geo_relate 仅保留 out.aabb.d != none 且 trans.d != none 的几何（有局部AABB且有变换）
    // - 若 !replace_exist 则追加条件 and aabb=none，避免覆盖已存在的实例 AABB（增量回填）

    // dbg!(refnos);
    let aabb_map = DashMap::new();
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let mut rstar_objs = Vec::new();
        let inst_keys = get_inst_relate_keys(chunk);
        let mut sql = format!(
            r#"select id, in as refno, world_trans.d as world_trans, in.noun as noun,
            (select out.aabb.d as aabb, trans.d as trans from out->geo_relate where out.aabb.d != none and trans.d != none)
            as geo_aabbs from {inst_keys} where world_trans.d != none"#,
        );
        //替换所有的aabb
        if !replace_exist {
            sql.push_str(" and aabb=none");
        }
        let mut response = SUL_DB.query(sql).await.unwrap();
        let Ok(result) = response.take::<Vec<QueryAabbParam>>(0) else {
            continue;
        };
        let mut update_sql = String::new();
        for r in result {
            // 优先尝试从 SQLite 空间索引读取
            #[cfg(feature = "sqlite-index")]
            if SqliteSpatialIndex::is_enabled() {
                let spatial_index =
                    SqliteSpatialIndex::with_default_path().expect("Failed to open spatial index");
                if let Ok(Some(aabb)) = spatial_index.get_aabb(r.refno.refno()) {
                    let aabb_hash = gen_bytes_hash::<_, 64>(&aabb).to_string();
                    aabb_map.entry(aabb_hash.clone()).or_insert(aabb);
                    // 使用当前查询到的 noun，避免旧缓存的 noun 干扰筛选
                    rstar_objs.push(RStarBoundingBox::new(aabb, r.refno, r.noun));
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
            for g in r.geo_aabbs {
                let t = r.world_trans * g.trans;
                let tmp_aabb = g.aabb.scaled(&t.scale.into());
                let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                    rotation: t.rotation.into(),
                    translation: t.translation.into(),
                });
                aabb.merge(&tmp_aabb);
            }
            if aabb.extents().magnitude().is_nan() || aabb.extents().magnitude().is_infinite() {
                #[cfg(feature = "debug_model")]
                dbg!("Found nan aabb");
                continue;
            }
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb).to_string();
            aabb_map.entry(aabb_hash.clone()).or_insert(aabb);
            let bbox = RStarBoundingBox::new(aabb, r.refno, r.noun);
            rstar_objs.push(bbox.clone());
            // 写入 SQLite 空间索引
            #[cfg(feature = "sqlite-index")]
            if SqliteSpatialIndex::is_enabled() {
                let spatial_index =
                    SqliteSpatialIndex::with_default_path().expect("Failed to open spatial index");
                let _ = spatial_index.insert_aabb(bbox.refno, &bbox.aabb, Some(&bbox.noun));
            }
            // 记录依赖（仅记录世界变换哈希；几何哈希需在其他查询路径写入）
            // This dependency tracking can be handled separately if needed
            let sql = format!(
                "update {} set aabb = aabb:⟨{}⟩;",
                r.refno.to_inst_relate_key(),
                aabb_hash,
            );
            update_sql.push_str(&sql);
        }
        if !update_sql.is_empty() {
            // dbg!(&update_sql);
            SUL_DB.query(&update_sql).await.unwrap();
        }
        // SQLite R*-tree update is now handled directly
    }
    utils::save_aabb_to_surreal(&aabb_map).await;

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NegInfo {
    pub id: String,
    pub geo_type: String,
    #[serde(default)]
    pub para_type: String,
    pub trans: Transform,
    pub aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ManiGeoTransQuery {
    pub refno: RefnoEnum,
    pub sesno: u32,
    pub noun: String,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(String, Transform)>,
    pub neg_ts: Vec<(RefnoEnum, Transform, Vec<NegInfo>)>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParamNegInfo {
    // pub id: String,
    pub param: PdmsGeoParam,
    pub geo_type: String,
    pub para_type: String,
    pub trans: Transform,
    pub aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OccGeoTransQuery {
    pub refno: RefnoEnum,
    pub noun: String,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(PdmsGeoParam, Transform)>,
    pub neg_ts: Vec<(RefnoEnum, Transform, Vec<ParamNegInfo>)>,
}

#[inline]
fn round_dmat4(m: DMat4) -> DMat4 {
    DMat4 {
        x_axis: dvec4_round_3(m.x_axis),
        y_axis: dvec4_round_3(m.y_axis),
        z_axis: dvec4_round_3(m.z_axis),
        w_axis: dvec4_round_3(m.w_axis),
    }
}

pub async fn apply_insts_boolean_occ(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);
    //筛选出来 "Neg", "CataCrossNeg" 的关联
    // SQL 说明：
    // - 仅选择存在负实体关系的实例： (in<-neg_relate)[0] != none
    // - 排除已标记 bad_bool 的实例： where ... and !bad_bool
    // - 要求实例已有整体 AABB： aabb.d != none（避免后续布尔时范围未知）
    // - 内层负实体筛选： geo_type in ["Neg", "CataCrossNeg"] 且 trans.d != NONE（参与布尔的负实体几何及其变换）
    // - 若不替换已有布尔结果，应追加 and !booled 以避免重复布尔；当前实现始终追加 and !booled（即默认增量）

    let mut sql = format!(
        r#"
            select
                in as refno,
                in.noun as noun,
                world_trans.d as wt,
                aabb.d as aabb,
                (select value [out.param, trans.d] from out->geo_relate) as ts,
                (select value [in, world_trans.d, (select out.param as param, geo_type, trans.d as trans,
                out.aabb.d as aabb, object::keys(out.param)[0] as para_type
                from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"] and trans.d != NONE )]
            from array::flatten(in<-neg_relate.in->inst_relate) ) as neg_ts from {} where in.id != none and !bad_bool
            and (in<-neg_relate)[0] != none and aabb.d!=none
        "#,
        inst_keys
    );
    // if !replace_exist
    {
        sql.push_str(" and !booled");
    }
    match SUL_DB.query(&sql).await {
        Ok(mut response) => {
            match response.take::<Vec<OccGeoTransQuery>>(0) {
                Ok(boolean_query) => {
                    // #[cfg(debug_assertions)]
                    // println!("occ inst boolean len: {}", boolean_query.len());
                    // dbg!(boolean_query.len());
                    // dbg!(&boolean_query);
                    if boolean_query.is_empty() {
                        return Ok(());
                    }

                    let chunk = (boolean_query.len() / 16).max(1);
                    for chunk in boolean_query.chunks(chunk) {
                        let group = chunk.to_vec();
                        let dir_clone = dir.clone();
                        // let shapes_map_clone = shapes_map_arc.clone();
                        // let task = tokio::spawn(async move {
                        let mut update_sql = String::new();
                        for mut b in group {
                            if b.ts.is_empty() {
                                continue;
                            }
                            let Some((pos_param, pos_t)) = b.ts.pop() else {
                                continue;
                            };
                            let inst_relate_id = b.refno.to_table_key("inst_relate");
                            //没有实体的情况，下次就不要再继续计算布尔运算了
                            let Ok(mut pos_shape) = pos_param.gen_occ_shape() else {
                                println!("布尔运算失败: 无法生成正实体形状, refno: {}", &b.refno);
                                update_sql.push_str(&format!(
                                    "update {} set bad_bool=true;",
                                    &inst_relate_id
                                ));
                                continue;
                            };
                            let pos_matrix = pos_t.compute_matrix().as_dmat4();
                            let Ok(mut pos_shape) = pos_shape.transformed(&pos_matrix) else {
                                println!("布尔运算失败: 无法转换正实体形状, refno: {}", &b.refno);
                                update_sql.push_str(&format!(
                                    "update {} set bad_bool=true;",
                                    &inst_relate_id
                                ));
                                continue;
                            };

                            for (param, t) in b.ts.iter() {
                                // dbg!(id);
                                if let Ok(shape) = param.gen_occ_shape() {
                                    if let Ok(s) = shape.transformed(&t.compute_matrix().as_dmat4())
                                    {
                                        pos_shape = pos_shape.union(&s.0).shape.into();
                                    }
                                }
                            }
                            // dbg!(b.refno);
                            let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();

                            #[cfg(feature = "debug_model")]
                            pos_shape.write_step(format!("{}.step", "pos")).unwrap();
                            // dbg!(b.neg_ts.len());
                            let mut neg_shapes = vec![];
                            let mut cross_neg_shapes = vec![];
                            for (refno, neg_t, negs) in b.neg_ts.into_iter() {
                                for ParamNegInfo {
                                    param,
                                    geo_type,
                                    para_type,
                                    trans,
                                    aabb,
                                } in negs
                                {
                                    if aabb.is_none() {
                                        // dbg!(&id);
                                        continue;
                                    }
                                    if let Ok(neg_shape) = param.gen_occ_shape() {
                                        let m = round_dmat4(
                                            inverse_mat
                                                * neg_t.compute_matrix().as_dmat4()
                                                * trans.compute_matrix().as_dmat4(),
                                        );
                                        // dbg!(m);
                                        // dbg!(refno);
                                        if let Ok(t_neg_shape) = neg_shape.0.transformed_by_gmat(&m)
                                        {
                                            // t_neg_shape.write_step(format!("{}.step", &neg_id)).unwrap();
                                            if geo_type == "Neg" {
                                                // dbg!(refno);
                                                neg_shapes.push(t_neg_shape);
                                            } else {
                                                cross_neg_shapes.push(t_neg_shape);
                                            }
                                        }
                                    }
                                }
                            }
                            // dbg!((neg_shapes.len(), cross_neg_shapes.len()));
                            if !neg_shapes.is_empty() || !cross_neg_shapes.is_empty() {
                                let mut success = false;
                                let inst_relate_id = b.refno.to_table_key("inst_relate");
                                if let Ok(pos_shape) = pos_shape.subtract_shapes(&neg_shapes, false)
                                {
                                    if let Ok(final_shape) =
                                        pos_shape.subtract_shapes(&cross_neg_shapes, true)
                                    {
                                        let tol = b.aabb.half_extents().magnitude() * 0.01;
                                        #[cfg(feature = "debug_model")]
                                        {
                                            final_shape
                                                .write_step(format!("{}.step", b.refno))
                                                .unwrap();
                                        }
                                        if let Ok(mesh) =
                                            PlantMesh::gen_occ_mesh(&final_shape, tol as _)
                                        {
                                            //保存到文件到dir下
                                            if mesh
                                                .ser_to_file(
                                                    &dir_clone.join(format!("{}.mesh", b.refno)),
                                                )
                                                .is_ok()
                                            {
                                                update_sql.push_str(&format!(
                                                    "update {} set booled=true;",
                                                    &inst_relate_id
                                                ));
                                                success = true;
                                            }
                                        }
                                    }
                                }
                                if !success {
                                    println!(
                                        "布尔运算失败: 无法保存结果 mesh, refno: {}",
                                        &b.refno
                                    );
                                    update_sql.push_str(&format!(
                                        "update {} set bad_bool=true;",
                                        &inst_relate_id
                                    ));
                                }
                            }
                            // dbg!(&update_sql);
                        }
                        if !update_sql.is_empty() {
                            let r = SUL_DB.query(&update_sql).await;
                            if let Err(_e) = r {
                                init_save_database_error(
                                    &update_sql,
                                    &std::panic::Location::caller().to_string(),
                                );
                            }
                        }
                        // });
                        // tasks.push(task);
                    }
                }
                Err(e) => {
                    init_deserialize_error(
                        "Vec<OccGeoTransQuery>",
                        &e,
                        &sql,
                        &std::panic::Location::caller().to_string(),
                    );
                    return Err(anyhow!(e.to_string()));
                }
            }
        }
        Err(e) => {
            init_query_error(&sql, &e, &std::panic::Location::caller().to_string());
            return Err(anyhow!(e.to_string()));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CataNegGroup {
    pub refno: RefnoEnum,
    pub inst_info_id: Thing,
    pub boolean_group: Vec<Vec<RefnoEnum>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GmGeoData {
    pub id: String,
    pub geom_refno: RefnoEnum,
    pub trans: Transform,
    pub param: PdmsGeoParam,
    //暂时aabb 不变
    pub aabb_id: Thing,
}

//处理元件库有负实体的布尔运算
pub async fn apply_cata_neg_boolean_occ(dir: PathBuf) -> anyhow::Result<()> {
    let sql = r#"
        select in as refno, (->inst_info)[0] as inst_info_id, (select value array::flatten([geom_refno, cata_neg])
        from ->inst_info->geo_relate where visible and !out.bad and cata_neg!=none) as boolean_group from inst_relate where (->inst_info)[0]!=none and has_cata_neg and !bad_bool and !booled
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let mut params: Vec<CataNegGroup> = response.take(0)?;
    // dbg!(params.len());
    // dbg!(&params);
    if params.is_empty() {
        return Ok(());
    }

    let mut tasks = Vec::new();
    let chunk = (params.len() / 16).max(1);
    // let chunk = params.len();
    for chunk in params.chunks(chunk) {
        let group = chunk.to_vec();
        let dir_clone = dir.clone();
        let task = tokio::spawn(async move {
            for g in group {
                let pes = g
                    .boolean_group
                    .iter()
                    .flatten()
                    .map(|x| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",");
                // dbg!(g.refno);
                let sql = format!(
                    r#"
                    select record::id(out) as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
                    from {}->inst_relate->inst_info->geo_relate
                    where visible and !out.bad and geom_refno in [{}]  and out.aabb!=none and out.param!=none"#,
                    g.refno.to_pe_key(),
                    pes
                );
                // dbg!(&sql);
                let Ok(mut resp) = SUL_DB.query(&sql).await else {
                    continue;
                };
                // let gms: Vec<GmGeoData> = resp.take(0).unwrap();
                let Ok(gms) = resp.take::<Vec<GmGeoData>>(0) else {
                    dbg!(&sql);
                    continue;
                };
                // dbg!(&gms);

                let mut update_sql = String::new();
                for bg in g.boolean_group {
                    let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };
                    // dbg!(pos);
                    let Ok(Ok(mut pos_shape)) = pos
                        .param
                        .gen_occ_shape()
                        .map(|x| x.transformed(&pos.trans.compute_matrix().as_dmat4()))
                    else {
                        update_sql.push_str(&format!(
                            "update {}<-inst_relate set bad_bool=true;",
                            &g.inst_info_id,
                        ));
                        continue;
                    };
                    // pos_shape
                    //     .write_step(format!("{}.step", "pos"))
                    //     .unwrap();

                    let mut neg_shapes = vec![];
                    for &neg in bg.iter().skip(1) {
                        // dbg!(neg);
                        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
                            continue;
                        };
                        // dbg!(neg_geo.trans.compute_matrix().as_dmat4());
                        let Ok(neg_shape) = neg_geo.param.gen_occ_shape() else {
                            continue;
                        };
                        if let Ok(t_neg_shape) = neg_shape
                            .0
                            .transformed_by_gmat(&neg_geo.trans.compute_matrix().as_dmat4())
                        {
                            // #[cfg(debug_assertions)]
                            // t_neg_shape.write_step(format!("{}.step", neg)).unwrap();
                            neg_shapes.push(t_neg_shape);
                        }
                    }
                    if !neg_shapes.is_empty() {
                        // for neg_shape in neg_shapes {
                        let new_id = g.refno.hash_with_another_refno(bg[0]);
                        if let Ok(pos_shape) = pos_shape.subtract_shapes(&neg_shapes, true) {
                            let mut aabb = Aabb::new_invalid();
                            for edge in pos_shape.edges() {
                                for point in edge.approximation_segments_custom(1.0, 1.0) {
                                    aabb.take_point(nalgebra::Point3::new(
                                        point.x as f32,
                                        point.y as f32,
                                        point.z as f32,
                                    ));
                                }
                            }
                            let tol = aabb.half_extents().magnitude() as f64 * 0.01;
                            // dbg!(tol);
                            // #[cfg(debug_assertions)]
                            // pos_shape
                            //     .write_step(format!("{}.step", "final"))
                            //     .unwrap();
                            let mut success = false;
                            if let Ok(mesh) = PlantMesh::gen_occ_mesh(&pos_shape, tol as _) {
                                //保存到文件到dir下
                                if mesh
                                    .ser_to_file(&dir_clone.join(format!("{}.mesh", new_id)))
                                    .is_ok()
                                {
                                    update_sql.push_str(&format!(
                                        "create inst_geo:⟨{}⟩ set meshed = true, aabb = {}, visible = true;",
                                        new_id, &pos.aabb_id
                                    ));
                                    // 有索引的关系，所以geom_refno需要点变化
                                    update_sql.push_str(&format!(
                                        "relate {}->geo_relate->inst_geo:⟨{}⟩ set geom_refno=pe:{}, geo_type='Pos', trans=trans:⟨0⟩;",
                                        &g.inst_info_id,
                                        new_id,
                                        format!("{}_b", bg[0]),
                                    ));
                                    update_sql.push_str(&format!(
                                        "update {}<-inst_relate set booled=true;",
                                        &g.inst_info_id,
                                    ));
                                    success = true;
                                }
                            }

                            if !success {
                                update_sql.push_str(&format!(
                                    "update {}<-inst_relate set bad_bool=true;",
                                    &g.inst_info_id,
                                ));
                            }
                        }
                    }
                }
                if !update_sql.is_empty() {
                    SUL_DB.query(update_sql).await.unwrap();
                }
            }
        });
        tasks.push(task);
    }
    dbg!(tasks.len());
    match futures::future::try_join_all(tasks).await {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
        }
    }

    Ok(())
}
