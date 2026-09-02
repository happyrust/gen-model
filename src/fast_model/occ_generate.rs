use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
#[cfg(feature = "manifold")]
use crate::fast_model::manifold_tessellate::tessellate_libgm_param;
use crate::fast_model::manifold_types::{CataNegGroup, GmGeoData, ManiGeoTransQuery, NegInfo};
use crate::fast_model::{EXIST_MESH_GEO_HASHES, utils};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::{PlantMesh, RsVec3};
use aios_core::tool::float_tool::{dvec4_round_3, f64_round};
use aios_core::{
    RefU64, RefnoEnum, gen_bytes_hash, get_inst_relate_keys, query_deep_neg_inst_refnos,
    query_deep_visible_inst_refnos,
};
use aios_core::{get_db_option, init_test_surreal};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use futures::{StreamExt, stream};
use glam::DMat4;
use itertools::Itertools;
use parry3d::bounding_volume::*;
use parry3d::math::Point;
use parse_pdms_db::parse::round_f32;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use surrealdb::sql::Thing;

///生成小的几何体
#[tokio::test]
#[ignore = "manual integration: requires the configured Surreal project and mesh files"]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    process_meshes_update_db_deep_default((&["17496/171559".into(), "24381/35844".into()]))
        .await
        .unwrap();
    Ok(())
}

/// AMS 库里 5 块必需房间面板实际只对应 3 份共享挤出参数。轮廓含重复点、
/// 共线回折和一个小自交环；这些都应当被保守地收口成可用边界，而不是把
/// `inst_geo` 永久标成 `bad`。
///
/// 轮廓修复发生在 wire 层，与后端无关，所以这条断言跟着生产路径走
/// `tessellate_libgm_param`——原先它挂在 `occ` 下，而 CI 不带 `occ`，等于从没跑过。
#[cfg(feature = "manifold")]
#[test]
fn ams_room_panel_self_intersections_are_repaired() {
    let params: Vec<PdmsGeoParam> = serde_json::from_str(include_str!(
        "../../tests/fixtures/room_panel_self_intersecting_extrusions.json"
    ))
    .expect("fixture parses");

    for (index, param) in params.into_iter().enumerate() {
        let mesh = tessellate_libgm_param(&param)
            .unwrap_or_else(|error| panic!("panel fixture {index} must build: {error}"))
            .unwrap_or_else(|| panic!("panel fixture {index} must be a shape, not None"));
        assert!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty(),
            "panel fixture {index}"
        );
    }
}

/// Real GENSEC `/6KA02-MSUP-E0090-V1` (24384/25743) from dbnum 8000.
///
/// Its straight SPINE uses outward end normals (-Z at the start, +Z at the
/// end). The constant SPRO profile must remain a regular extrusion and be
/// triangulatable — now by the production backend rather than by OCC, so the
/// case finally runs in CI.
#[cfg(feature = "manifold")]
#[test]
fn gensec_straight_spro_can_be_triangulated() {
    use aios_core::parsed_data::{CateProfileParam, SProfileData};
    use aios_core::prim_geo::spine::{Line3D, SweepPath3D};
    use aios_core::prim_geo::sweep_solid::SweepSolid;
    use glam::{DVec3, Vec2, Vec3};

    std::thread::Builder::new()
        .name("gensec-spro-regression".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let profile = SProfileData {
                refno: Default::default(),
                verts: vec![
                    Vec2::new(-65.0, 0.0),
                    Vec2::new(-75.0, 0.0),
                    Vec2::new(-75.0, 40.0),
                    Vec2::new(-10.633, 40.0),
                    Vec2::new(-42.519, 2.0),
                    Vec2::new(42.519, 2.0),
                    Vec2::new(10.633, 40.0),
                    Vec2::new(75.0, 40.0),
                    Vec2::new(75.0, 0.0),
                    Vec2::new(65.0, 0.0),
                    Vec2::new(65.0, 2.0),
                    Vec2::new(73.0, 2.0),
                    Vec2::new(73.0, 38.0),
                    Vec2::new(14.922, 38.0),
                    Vec2::new(46.808, 0.0),
                    Vec2::new(-46.808, 0.0),
                    Vec2::new(-14.922, 38.0),
                    Vec2::new(-73.0, 38.0),
                    Vec2::new(-73.0, 2.0),
                    Vec2::new(-65.0, 2.0),
                ],
                frads: vec![
                    0.0, 6.0, 6.0, 6.0, 4.0, 4.0, 6.0, 6.0, 6.0, 0.0, 0.0, 4.0, 4.0, 4.0, 6.0, 6.0,
                    4.0, 4.0, 4.0, 0.0,
                ],
                plax: Vec3::Y,
                plin_pos: Vec2::ZERO,
                plin_axis: Vec3::Y,
                na_axis: Vec3::Y,
            };
            let sweep = SweepSolid {
                profile: CateProfileParam::SPRO(profile),
                drns: Some(DVec3::new(0.0, 0.000999999547497755, -0.9999995000003274)),
                drne: Some(DVec3::Z),
                bangle: 0.0,
                plax: Vec3::Y,
                extrude_dir: DVec3::Z,
                height: 0.0,
                path: SweepPath3D::Line(Line3D {
                    start: Vec3::ZERO,
                    end: Vec3::Z * 560.00006,
                    is_spine: true,
                }),
                lmirror: false,
            };

            let mesh = tessellate_libgm_param(&PdmsGeoParam::PrimLoft(sweep))
                .expect("GENSEC shape must be triangulated")
                .expect("GENSEC SPRO is a shape, not None");

            assert!(!mesh.vertices.is_empty());
            assert!(!mesh.indices.is_empty());
        })
        .expect("GENSEC test thread must start")
        .join()
        .expect("GENSEC test thread must finish");
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
        gen_inst_meshes(chunk, replace_exist, dir.clone()).await?;
        // println!(
        //     "gen_inst_meshes finished: {} ms",
        //     time.elapsed().as_millis()
        // );
        // let time = std::time::Instant::now();
        update_inst_relate_aabbs_by_refnos(chunk, replace_exist).await?;
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
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
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
        apply_cata_neg_boolean_manifold(chunk, replace_exist, dir.clone(), failure_policy).await?;
        apply_insts_boolean_manifold(chunk, replace_exist, dir.clone(), failure_policy).await?;
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
    gen_inst_meshes(&refnos, replace_exist, dir.clone()).await?;
    println!(
        "gen_inst_meshes finished: {} ms",
        time.elapsed().as_millis()
    );
    let time = std::time::Instant::now();
    update_inst_relate_aabbs_by_refnos(&refnos, replace_exist).await?;
    println!(
        "update_inst_relate_aabbs finished: {} ms",
        time.elapsed().as_millis()
    );

    let time = std::time::Instant::now();
    //生成元件库内部几何体的负实体运算
    let failure_policy =
        crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
    apply_cata_neg_boolean_manifold(&refnos, replace_exist, dir.clone(), failure_policy).await?;
    apply_insts_boolean_manifold(&refnos, replace_exist, dir.clone(), failure_policy).await?;
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
    process_meshes_update_db_deep_with_policy(
        dboption,
        refnos,
        crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
    )
    .await
}

pub(crate) async fn process_meshes_update_db_deep_with_policy(
    dboption: &DbOption,
    refnos: &[RefnoEnum],
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
) -> anyhow::Result<()> {
    let report = process_meshes_update_db_deep_report(
        dboption,
        refnos,
        failure_policy,
        crate::options::model_root_inflight_max(),
    )
    .await;
    if report.failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(summarize_root_failures("model", &report.failures))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RootGenerationFailure {
    pub root: String,
    pub error: String,
}

/// 根级失败的统一摘要：件数 + 前三个根的错误。三处 bail（本文件的兼容包装、
/// `gen_model` 定向路径、`model_refresh::generate_roots`）共用这一份，别各拼各的。
pub(crate) fn summarize_root_failures(kind: &str, failures: &[RootGenerationFailure]) -> String {
    format!(
        "{} {kind} root(s) failed: {}",
        failures.len(),
        failures
            .iter()
            .take(3)
            .map(|failure| format!("{}: {}", failure.root, failure.error))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RootGenerationReport {
    pub completed: Vec<String>,
    pub failures: Vec<RootGenerationFailure>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RootStageTiming {
    query_ms: u128,
    mesh_ms: u128,
    aabb_ms: u128,
    boolean_ms: u128,
}

pub(crate) async fn process_meshes_update_db_deep_report(
    dboption: &DbOption,
    refnos: &[RefnoEnum],
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
    root_inflight_max: usize,
) -> RootGenerationReport {
    let mut report = RootGenerationReport::default();
    if !refnos.is_empty() {
        println!("更新模型结点数量: {}", refnos.len());
        let time = std::time::Instant::now();
        // 分段累计。此前这里只有一个跨越整个循环的计时器，却挂着「布尔运算花费时间」
        // 的名字——它把两次深度查询、网格生成、AABB 落库、房间入队全算进了布尔运算，
        // 于是这项统计能超过整个进程的 CPU 总时间。四段分开记才知道该优化哪一步。
        let mut timing = RootStageTiming::default();
        // 本窗口开始前先清零，免得把上一轮（或别的调用方）的读数算进来。
        let _ = take_stale_lookup_stats();
        let mut work = stream::iter(refnos.iter().copied().map(|refno| async move {
            let root = refno.to_string();
            (
                root,
                process_one_model_root(dboption, refno, failure_policy).await,
            )
        }))
        .buffer_unordered(root_inflight_max.max(1));
        while let Some((root, result)) = work.next().await {
            match result {
                Ok(root_timing) => {
                    timing.query_ms += root_timing.query_ms;
                    timing.mesh_ms += root_timing.mesh_ms;
                    timing.aabb_ms += root_timing.aabb_ms;
                    timing.boolean_ms += root_timing.boolean_ms;
                    report.completed.push(root);
                }
                Err(error) => report.failures.push(RootGenerationFailure {
                    root,
                    error: format!("{error:#}"),
                }),
            }
        }
        let stale = take_stale_lookup_stats();
        println!(
            "模型结点更新耗时: {} ms（深度查询 {} / 网格生成 {} / AABB落库 {} / 布尔运算 {}）\
             ；根并发 {}/{}，失败 {}；其中按 refno 取旧条目 {} ms / {} 次，树最大 {} 条",
            time.elapsed().as_millis(),
            timing.query_ms,
            timing.mesh_ms,
            timing.aabb_ms,
            timing.boolean_ms,
            root_inflight_max.max(1).min(refnos.len()),
            root_inflight_max.max(1),
            report.failures.len(),
            stale.micros / 1000,
            stale.calls,
            stale.max_tree_entries
        );
    }
    report
}

async fn process_one_model_root(
    dboption: &DbOption,
    refno: RefnoEnum,
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
) -> anyhow::Result<RootStageTiming> {
    let dir = dboption.get_meshes_path();
    let replace_exist = dboption.is_replace_mesh();
    let mut timing = RootStageTiming::default();
    #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
    println!("更新模型结点: {}", refno);
    let t_query = std::time::Instant::now();
    let mut target_visible_refnos = vec![];
    let mut update_refnos = query_deep_visible_inst_refnos(refno).await?;
    target_visible_refnos.extend(update_refnos.clone());
    // dbg!(&target_visible_refnos);

    let neg_refnos = query_deep_neg_inst_refnos(refno).await?;
    update_refnos.extend(neg_refnos);
    timing.query_ms += t_query.elapsed().as_millis();

    // #[cfg(any(feture = "debug_model", feature = "debug_model_no_obj"))]
    if update_refnos.is_empty() {
        return Ok(timing);
    }

    println!("实际需要更新模型结点数量: {}", update_refnos.len());
    //缩小范围
    if dboption.gen_mesh {
        // dbg!(&target_refnos);
        // 生成模型文件
        let t_mesh = std::time::Instant::now();
        gen_inst_meshes(&update_refnos, replace_exist, dir.clone()).await?;
        timing.mesh_ms += t_mesh.elapsed().as_millis();

        let t_aabb = std::time::Instant::now();
        // 更新aabb 到inst relate，geo relate。
        //
        // 这里必须强制 replace=true，不跟 `replace_mesh` 配置走（mesh 文件按内容
        // 寻址，replace 与否只影响要不要重写同名文件；包围盒不是）：默认配置
        // replace_mesh=false 会给刷新 SQL 追加 `and aabb=none`，凡是插入时就带
        // aabb 指针的行（隐含直管段 TUBI/BOXI）被整体过滤——它们因此从未进过
        // 空间树、从未触发过房间重算。与 `update_world_transforms` 强制 true
        // 是同一个理由（ADR-010 D2）。
        // 只在**定向**重生成时建立 durable 房间触发。直写路径由增量入口把
        // AABB 指针、room pending 与 spatial epoch 放进同一事务，事务成功后才
        // 推进内存树；暂存路径仍只写 journal 并把变化寄存在窗口里。全量生成
        // 本来就以 `build_room_relations` 的整体重建收尾，不逐元素排房间任务。
        if dboption.debug_root_refnos.is_some() {
            update_inst_relate_aabbs_by_refnos_incremental(&update_refnos, true).await?;
        } else {
            update_inst_relate_aabbs_by_refnos(&update_refnos, true).await?;
        }
        timing.aabb_ms += t_aabb.elapsed().as_millis();
    }

    if target_visible_refnos.is_empty() {
        return Ok(timing);
    }

    if dboption.apply_boolean_operation {
        // dbg!(target_visible_refnos.len());
        let t_bool = std::time::Instant::now();
        //生成元件库内部几何体的负实体运算
        apply_cata_neg_boolean_manifold(
            &target_visible_refnos,
            replace_exist,
            dir.clone(),
            failure_policy,
        )
        .await?;
        apply_insts_boolean_manifold(
            &target_visible_refnos,
            replace_exist,
            dir.clone(),
            failure_policy,
        )
        .await?;
        timing.boolean_ms += t_bool.elapsed().as_millis();

        // 布尔阶段会新增/改指最终可见几何（例如 REDU 的 booled 关系）。上面的
        // 第一次 AABB 刷新发生在布尔之前，只能描述原始正实体；若不在这里按
        // 最终关系再刷一次，同一 session 会出现两种稳定结果：增量队列随后有
        // post_regen_aabb 时得到布尔后包围盒，而按需 ensure 直接返回布尔前包围盒。
        // 2026-08-11 AMS db8000 / 24384/24682 实证为 maxZ 3400 vs 3340。
        let t_aabb = std::time::Instant::now();
        if dboption.debug_root_refnos.is_some() {
            update_inst_relate_aabbs_by_refnos_incremental(&target_visible_refnos, true).await?;
        } else {
            update_inst_relate_aabbs_by_refnos(&target_visible_refnos, true).await?;
        }
        timing.aabb_ms += t_aabb.elapsed().as_millis();
    }
    Ok(timing)
}

/// 几何参数查询结构体
///
/// # 字段
///
/// * `id` - 几何体ID
/// * `param` - PDMS几何体参数
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryGeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

/// libgm 路径没有 OCC 边。`pts` 取网格 AABB 八角，禁止空列表假装成功。
fn mesh_aabb_corner_pts(aabb: &Aabb) -> [glam::Vec3; 8] {
    aabb.vertices().map(|p| glam::Vec3::new(p.x, p.y, p.z))
}

fn persist_libgm_plant_mesh(
    id: &str,
    mesh: &PlantMesh,
    dir: &PathBuf,
    aabb_map: &DashMap<String, Aabb>,
    chunk_aabbs: &DashMap<String, Aabb>,
    chunk_pts: &DashMap<u64, String>,
    update_sql: &mut String,
) -> anyhow::Result<()> {
    let aabb = mesh
        .aabb
        .ok_or_else(|| anyhow!("几何 {id} libgm 三角化后没有包围盒"))?;
    if mesh.indices.len() < 3 || mesh.vertices.len() < 3 {
        anyhow::bail!("几何 {id} libgm 网格为空");
    }
    mesh.ser_to_file(&dir.join(format!("{id}.mesh")))
        .map_err(|error| anyhow!("save generated mesh {id} failed: {error}"))?;
    let mut pt_hashes = HashSet::new();
    for point in mesh_aabb_corner_pts(&aabb) {
        let pts_hash = RsVec3(point).gen_hash();
        pt_hashes.insert(format!("vec3:⟨{pts_hash}⟩"));
        if !chunk_pts.contains_key(&pts_hash) {
            chunk_pts.insert(
                pts_hash,
                serde_json::to_string(&point)
                    .map_err(|error| anyhow!("serialize mesh point failed: {error}"))?,
            );
        }
    }
    if pt_hashes.is_empty() {
        anyhow::bail!("几何 {id} libgm 网格 AABB 没有角点，拒绝空 pts");
    }
    let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
    update_sql.push_str(&format!(
        "update inst_geo:⟨{id}⟩ set meshed = true, aabb = aabb:⟨{aabb_hash}⟩, pts=[{}];",
        pt_hashes.into_iter().join(",")
    ));
    aabb_map.entry(aabb_hash.to_string()).or_insert(aabb);
    chunk_aabbs.entry(aabb_hash.to_string()).or_insert(aabb);
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
pub async fn gen_inst_meshes(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    // WP-F T037 + X1a：形状生成只有 manifold 一台引擎，`occ` 已从本仓摘除。
    // 没有后端就响亮失败，不许静默跳过。
    #[cfg(not(feature = "manifold"))]
    {
        let _ = (refnos, replace_exist, dir);
        anyhow::bail!("gen_inst_meshes: no tessellation backend (enable `manifold`)");
    }
    #[cfg(feature = "manifold")]
    {
        const PAGE_NUM: usize = 100;
        let inst_keys = get_inst_relate_keys(refnos);
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
        //out.aabb.d == none and
        // println!("sql is {}", &sql);
        let mut response = crate::data_interface::staging::active_data_db()
            .query(sql)
            .await?
            .check()?;
        let mut inst_geo_ids: Vec<(Option<Thing>, bool)> = response.take(0)?;
        //todo 排除已经生成了的模型
        // let mut update_geos_by_meshes = HashSet::default();
        inst_geo_ids.retain(|(x, y)| {
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
        if inst_geo_ids.is_empty() {
            return Ok(());
        }
        let mut tasks = vec![];
        // 跨任务共享只为收尾回填 `EXIST_MESH_GEO_HASHES`；库内记录由各任务自己写。
        let aabb_map = Arc::new(DashMap::new());
        for (idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
            let ids = chunk
                .into_iter()
                .map(|(x, _)| x.as_ref().unwrap().to_string())
                .join(",");
            let dir = dir.clone();
            let aabb_map = aabb_map.clone();
            // PAGE_NUM 只是查询分页宽度；在飞数由几何并发闸限住（specs/023），
            // 页数不再等于并发宽度。
            let task = crate::fast_model::concurrency::spawn_gated_leaf(async move {
                let mut libgm_meshes: HashMap<String, PlantMesh> = HashMap::new();
                // 形状建不出来（或不是形状）的几何。它们进不了 `libgm_meshes`，
                // 所以下面那句 `set bad = true` 一辈子轮不到它们——得在这里自己记下来。
                let mut unbuildable: Vec<String> = Vec::new();
                // 本任务 update_sql 引用到的 aabb / vec3 记录（D9 顺序）：记录
                // 必须先于指针在**本任务内**落库，跨任务去重靠 INSERT IGNORE
                // 幂等，不能靠共享 map——别的任务替你去了重，不等于替你把记录
                // 写进了库。
                let chunk_aabbs = DashMap::new();
                let chunk_pts = DashMap::new();
                let sql = format!(
                    "select <string> record::id(id) as id, param from [{}] where param != NONE",
                    ids
                );
                // println!("sql is {}", &sql);
                match crate::data_interface::staging::active_data_db()
                    .query(&sql)
                    .await
                {
                    Ok(response) => {
                        let mut response = response.check().map_err(|error| {
                            anyhow!("query mesh parameters statement failed: {error}")
                        })?;
                        let r = response.take::<Vec<QueryGeoParam>>(0);
                        if let Err(e) = &r {
                            init_deserialize_error(
                                "Vec<QueryGeoParam>",
                                e,
                                &sql,
                                &std::panic::Location::caller().to_string(),
                            );
                            return Err(anyhow!("decode mesh parameters failed: {e}"));
                        }
                        let result: Vec<QueryGeoParam> = r.unwrap();
                        if result.is_empty() {
                            return Ok(());
                        }
                        // dbg!(&result);
                        for g in result {
                            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
                            println!("gen mesh param: {}", &g.id);
                            // WP-F T037：形状只有 manifold 一台引擎，这个 match 之后
                            // 不许再长出 `#[cfg(feature = "occ")]` 的形状回退分支。
                            // `None` = 非形状（Unknown / CompoundShape），报错 = 坏参数；
                            // 两者都标 bad——上游取数按 `!out.bad` 过滤，标不上就
                            // 每一轮生成都把同一份废参数重算一遍。
                            match tessellate_libgm_param(&g.param) {
                                Ok(Some(mesh)) => {
                                    libgm_meshes.insert(g.id, mesh);
                                }
                                Ok(None) => {
                                    eprintln!(
                                        "几何 {} 不是形状（Unknown/CompoundShape），标记跳过",
                                        g.id
                                    );
                                    unbuildable.push(g.id);
                                }
                                Err(error) => {
                                    let affected = aios_core::query_refnos_by_geo_hash(&g.id)
                                        .await
                                        .unwrap_or_default();
                                    eprintln!(
                                        "几何 {} libgm 建不出形状，标记跳过（波及 {} 个构件）：{error}",
                                        g.id,
                                        affected.len()
                                    );
                                    #[cfg(any(
                                        feature = "debug_model",
                                        feature = "debug_model_no_obj",
                                        feature = "log_error"
                                    ))]
                                    println!("受影响的构件: {affected:?}");
                                    unbuildable.push(g.id);
                                }
                            }
                        }
                        let mut update_sql = "".to_string();
                        for id in &unbuildable {
                            update_sql.push_str(&format!("update inst_geo:⟨{id}⟩ set bad = true;"));
                        }

                        for (id, mesh) in &libgm_meshes {
                            if let Err(error) = persist_libgm_plant_mesh(
                                id,
                                mesh,
                                &dir,
                                &aabb_map,
                                &chunk_aabbs,
                                &chunk_pts,
                                &mut update_sql,
                            ) {
                                let affected = aios_core::query_refnos_by_geo_hash(id)
                                    .await
                                    .unwrap_or_default();
                                eprintln!(
                                    "几何 {id} libgm 网格落盘失败，标记跳过（波及 {} 个构件）：{error}",
                                    affected.len()
                                );
                                update_sql
                                    .push_str(&format!("update inst_geo:⟨{id}⟩ set bad=true;"));
                            }
                        }
                        if !update_sql.is_empty() {
                            // D9 顺序（与 inst_relate 指针同一条教训）：先把本任务
                            // 引用的 vec3 / aabb 记录写进库，再落 `inst_geo` 指针。
                            // 反过来的话，两步之间的崩溃或并发读者会拿到悬空指针，
                            // `aabb.d` 为 none 的读者把几何整条漏掉。
                            utils::save_pts_to_surreal(&chunk_pts).await?;
                            utils::save_aabb_to_surreal(&chunk_aabbs).await?;
                            //执行SUL_DB update,使用chunk 保存
                            if let Err(error) = crate::surreal_retry::execute_model_write(
                                &update_sql,
                                "mark generated inst_geo state",
                            )
                            .await
                            {
                                init_save_database_error(
                                    &update_sql,
                                    &std::panic::Location::caller().to_string(),
                                );
                                return Err(error);
                            }
                        }
                    }
                    Err(e) => {
                        init_query_error(&sql, &e, &std::panic::Location::caller().to_string());
                        return Err(anyhow!("query mesh parameters failed: {e}"));
                    }
                }
                Ok::<(), anyhow::Error>(())
            });
            tasks.push(task);
        }

        let task_results = futures::future::join_all(tasks).await;
        for result in task_results {
            let result = result.map_err(|error| anyhow!("mesh worker join failed: {error}"))?;
            result?;
        }

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

        // vec3 / aabb 记录已在每个任务内先于 `inst_geo` 指针落库（D9 顺序），
        // 这里不再有 join 之后的全局补写——那正是崩溃时留下悬空指针的窗口。

        Ok(())
    } // cfg(manifold)
}

pub(crate) use super::aabb_refresh::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
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
    pub sesno: u32,
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

#[cfg(test)]
mod aabb_write_order_tests {
    /// 生成器代码到这一行为止。AABB 刷新（`update_inst_relate_aabbs_by_refnos*`）连同
    /// 它的源码钉已搬到 `aabb_refresh.rs`，留在这里的只有这个 re-export 接缝。
    const GENERATOR_TAIL: &str = "\npub(crate) use super::aabb_refresh::*;";

    #[test]
    fn mesh_workers_propagate_query_write_and_join_failures() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn gen_inst_meshes(")
            .expect("gen_inst_meshes exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("gen_inst_meshes boundary")
            .0;
        assert!(body.contains(".check()?"), "{body}");
        assert!(body.contains("for result in task_results"), "{body}");
        assert!(
            body.contains("save_pts_to_surreal(&chunk_pts).await?"),
            "{body}"
        );
        assert!(body.contains("join_all(tasks).await"), "{body}");
        assert!(!body.contains("try_join_all(tasks)"), "{body}");
        // specs/023：页任务的在飞数由几何并发闸限住，PAGE_NUM 只是分页宽度。
        assert!(
            body.contains("concurrency::spawn_gated_leaf("),
            "网格页任务必须过几何并发闸: {body}"
        );
    }

    /// `gen_inst_meshes` 的任务体内，vec3 / aabb 记录必须先于 `inst_geo` 指针
    /// update 落库（与 `inst_relate` 指针同一条 D9 教训），且 join 之后不得再有
    /// 全局记录补写——「任务里先落指针、join 后统一补记录」正是修掉的悬空指针
    /// 窗口：两步之间崩溃或并发读，`aabb.d` 读者会把几何整条漏掉。
    #[test]
    fn mesh_records_land_before_inst_geo_pointers_inside_each_task() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn gen_inst_meshes(")
            .expect("gen_inst_meshes exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("gen_inst_meshes boundary")
            .0;

        let pts_at = body
            .find("save_pts_to_surreal(&chunk_pts)")
            .expect("任务体内必须先写 vec3 记录");
        let aabb_at = body
            .find("save_aabb_to_surreal(&chunk_aabbs)")
            .expect("任务体内必须先写 aabb 记录");
        let pointers_at = body
            .find("mark generated inst_geo state")
            .expect("任务体内的 inst_geo 指针 update 必须存在");
        assert!(
            pts_at < pointers_at && aabb_at < pointers_at,
            "记录必须在任务体内先于 inst_geo 指针落库"
        );
        assert!(
            !body.contains("save_aabb_to_surreal(&aabb_map)")
                && !body.contains("save_pts_to_surreal(&pts_json_map)"),
            "join 之后不得再有全局记录补写"
        );
    }

    #[test]
    fn gen_inst_meshes_bails_without_backend_and_tries_libgm_first() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn gen_inst_meshes(")
            .expect("gen_inst_meshes exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("gen_inst_meshes boundary")
            .0;
        assert!(
            body.contains("no tessellation backend"),
            "T005: 没有 manifold 后端必须 bail"
        );
        assert!(
            !body.contains("gen_inst_meshes skipped (feature `occ` disabled)"),
            "T005: silent skip is forbidden"
        );
        assert!(
            body.contains("tessellate_libgm_param"),
            "形状一律由 tessellate_libgm_param 裁决"
        );
        assert!(
            body.contains("persist_libgm_plant_mesh"),
            "T007: manifold meshes must persist AABB/pts from the mesh"
        );
    }

    /// WP-F T037（ADR-030 修订二）：`gen_inst_meshes` 里不再有第二台形状引擎。
    /// `None`（非形状）与 tessellate 报错都直接标 `bad`；把 `gen_occ_shape` 或
    /// `shapes_map` 加回来（哪怕挂在 `#[cfg(feature = "occ")]` 下）本测试红。
    #[test]
    fn gen_inst_meshes_has_no_occ_shape_fallback() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn gen_inst_meshes(")
            .expect("gen_inst_meshes exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("gen_inst_meshes boundary")
            .0;
        assert!(
            !body.contains("gen_occ_shape") && !body.contains("shapes_map"),
            "形状生成不得回退 OCC：{body}"
        );
        assert!(
            body.contains("不是形状") && body.contains("unbuildable.push"),
            "非形状判定必须留下可见的跳过记录并标 bad：{body}"
        );
    }

    #[test]
    fn mesh_aabb_corners_are_eight_and_non_empty() {
        use parry3d::bounding_volume::Aabb;
        use parry3d::math::Point;
        let aabb = Aabb::new(Point::new(-1.0, -2.0, -3.0), Point::new(4.0, 5.0, 6.0));
        let corners = super::mesh_aabb_corner_pts(&aabb);
        assert_eq!(corners.len(), 8);
        assert!(
            corners
                .iter()
                .any(|p| *p == glam::Vec3::new(-1.0, -2.0, -3.0))
        );
        assert!(corners.iter().any(|p| *p == glam::Vec3::new(4.0, 5.0, 6.0)));
    }

    #[test]
    fn targeted_regen_and_transform_use_the_incremental_aabb_entrypoint() {
        let regen = include_str!("occ_generate.rs")
            .split_once("pub async fn process_meshes_update_db_deep(")
            .expect("process_meshes_update_db_deep exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("aabb refresh boundary")
            .0;
        assert!(
            regen.contains("update_inst_relate_aabbs_by_refnos_incremental"),
            "定向 regen 必须走 durable 增量入口"
        );
        assert!(
            !regen.contains("enqueue_room_recalc"),
            "regen 不得在指针提交后再单独入队"
        );

        let transform = include_str!("../data_interface/increment_manager.rs")
            .split_once("pub(crate) async fn refresh_world_transform_products(")
            .expect("refresh_world_transform_products exists")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(body, _)| body)
            .unwrap_or_default();
        assert!(
            transform.contains("update_inst_relate_aabbs_by_refnos_incremental"),
            "transform 必须走 durable 增量入口"
        );
        assert!(
            !transform.contains("enqueue_room_recalc"),
            "transform 不得在指针提交后再单独入队"
        );
    }

    #[test]
    fn root_dependency_queries_fail_the_root_instead_of_becoming_empty_results() {
        let body = include_str!("occ_generate.rs")
            .split_once("async fn process_one_model_root(")
            .expect("root pipeline exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("root pipeline boundary")
            .0;
        assert!(body.contains("query_deep_visible_inst_refnos(refno).await?"));
        assert!(body.contains("query_deep_neg_inst_refnos(refno).await?"));
        assert!(
            !body.contains("query_deep_visible_inst_refnos(refno).await.unwrap_or_default()")
                && !body.contains("query_deep_neg_inst_refnos(refno).await.unwrap_or_default()"),
            "深度查询失败不得静默伪装成无可生成根"
        );
    }

    /// 最终 AABB 必须在布尔关系落库之后再刷新。只保留布尔前那一次，会让按需生成
    /// 返回原始正实体包围盒，而增量链因后续 post_regen_aabb 偶然得到另一结果。
    #[test]
    fn boolean_generation_refreshes_aabb_after_final_relations_exist() {
        let body = include_str!("occ_generate.rs")
            .split_once("pub async fn process_meshes_update_db_deep(")
            .expect("process_meshes_update_db_deep exists")
            .1
            .split_once(GENERATOR_TAIL)
            .expect("aabb refresh boundary")
            .0;
        // 只认函数名：调用一旦被 rustfmt 拆成多行，带实参的针就再也扎不中，
        // 这道顺序门会静默变成「找不到即 panic」而不是它要守的那条不变量。
        let boolean_at = body
            .find("apply_insts_boolean_manifold(")
            .expect("final boolean stage exists");
        let final_refresh_at = body[boolean_at..]
            .find("update_inst_relate_aabbs_by_refnos_incremental")
            .map(|offset| boolean_at + offset)
            .expect("targeted generation must refresh after boolean relations");
        assert!(boolean_at < final_refresh_at, "{body}");
    }
}
