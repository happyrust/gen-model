use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use crate::fast_model::{EXIST_MESH_GEO_HASHES, utils};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::error::{init_deserialize_error, init_query_error, init_save_database_error};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
#[cfg(feature = "occ")]
use aios_core::prim_geo::basic::OccSharedShape;
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
use glam::DMat4;
use itertools::Itertools;
#[cfg(feature = "occ")]
use opencascade::primitives::IntoShape;
use parry3d::bounding_volume::*;
use parry3d::math::{Isometry, Point};
use parse_pdms_db::parse::round_f32;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
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
#[cfg(feature = "occ")]
#[test]
fn ams_room_panel_self_intersections_are_repaired_for_occ() {
    let params: Vec<PdmsGeoParam> = serde_json::from_str(include_str!(
        "../../tests/fixtures/room_panel_self_intersecting_extrusions.json"
    ))
    .expect("fixture parses");

    for (index, param) in params.into_iter().enumerate() {
        let shape = param
            .gen_occ_shape()
            .unwrap_or_else(|error| panic!("panel fixture {index} must build: {error}"));
        assert!(shape.edges().next().is_some(), "panel fixture {index}");
    }
}

/// Real GENSEC `/6KA02-MSUP-E0090-V1` (24384/25743) from dbnum 8000.
///
/// Its straight SPINE uses outward end normals (-Z at the start, +Z at the
/// end). The constant SPRO profile must remain a regular extrusion and be
/// triangulatable by OCC.
#[cfg(feature = "occ")]
#[test]
fn gensec_straight_spro_can_be_triangulated() {
    use aios_core::parsed_data::{CateProfileParam, SProfileData};
    use aios_core::prim_geo::spine::{Line3D, SweepPath3D};
    use aios_core::prim_geo::sweep_solid::SweepSolid;
    use aios_core::shape::pdms_shape::BrepShapeTrait;
    use glam::{DVec3, Vec2, Vec3};

    std::thread::Builder::new()
        .name("gensec-occ-regression".into())
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

            let shape = sweep
                .gen_occ_shape()
                .expect("GENSEC shape must be generated");
            let mesh = PlantMesh::gen_occ_mesh(&shape, 1.4777433776855469)
                .expect("GENSEC shape must be triangulated");

            assert!(!mesh.vertices.is_empty());
            assert!(!mesh.indices.is_empty());
        })
        .expect("GENSEC OCC test thread must start")
        .join()
        .expect("GENSEC OCC test thread must finish");
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
        // 分段累计。此前这里只有一个跨越整个循环的计时器，却挂着「布尔运算花费时间」
        // 的名字——它把两次深度查询、网格生成、AABB 落库、房间入队全算进了布尔运算，
        // 于是这项统计能超过整个进程的 CPU 总时间。四段分开记才知道该优化哪一步。
        let mut query_ms = 0u128;
        let mut mesh_ms = 0u128;
        let mut aabb_ms = 0u128;
        let mut boolean_ms = 0u128;
        for &refno in refnos {
            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
            println!("更新模型结点: {}", refno);
            let t_query = std::time::Instant::now();
            let mut target_visible_refnos = vec![];
            let mut update_refnos = query_deep_visible_inst_refnos(refno)
                .await
                .unwrap_or_default();
            target_visible_refnos.extend(update_refnos.clone());
            // dbg!(&target_visible_refnos);

            let neg_refnos = query_deep_neg_inst_refnos(refno).await.unwrap_or_default();
            update_refnos.extend(neg_refnos);
            query_ms += t_query.elapsed().as_millis();

            // #[cfg(any(feture = "debug_model", feature = "debug_model_no_obj"))]
            if update_refnos.is_empty() {
                continue;
            }

            println!("实际需要更新模型结点数量: {}", update_refnos.len());
            //缩小范围
            if dboption.gen_mesh {
                // dbg!(&target_refnos);
                // 生成模型文件
                let t_mesh = std::time::Instant::now();
                gen_inst_meshes(&update_refnos, replace_exist, dir.clone()).await?;
                mesh_ms += t_mesh.elapsed().as_millis();

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
                aabb_ms += t_aabb.elapsed().as_millis();
            }

            if target_visible_refnos.is_empty() {
                continue;
            }

            if dboption.apply_boolean_operation {
                // apply_cata_neg_boolean_occ(None).await.unwrap();
                // dbg!(target_visible_refnos.len());
                let t_bool = std::time::Instant::now();
                //生成元件库内部几何体的负实体运算
                apply_cata_neg_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                apply_insts_boolean_manifold(&target_visible_refnos, replace_exist, dir.clone())
                    .await?;
                //有一些布尔运算要精确计算，不然会有薄片出现
                //生成负实体的布尔运算
                // apply_insts_boolean_occ(&target_visible_refnos, replace_exist, dir.clone()).await?;
                boolean_ms += t_bool.elapsed().as_millis();

                // 布尔阶段会新增/改指最终可见几何（例如 REDU 的 booled 关系）。上面的
                // 第一次 AABB 刷新发生在布尔之前，只能描述原始正实体；若不在这里按
                // 最终关系再刷一次，同一 session 会出现两种稳定结果：增量队列随后有
                // post_regen_aabb 时得到布尔后包围盒，而按需 ensure 直接返回布尔前包围盒。
                // 2026-08-11 AMS db8000 / 24384/24682 实证为 maxZ 3400 vs 3340。
                let t_aabb = std::time::Instant::now();
                if dboption.debug_root_refnos.is_some() {
                    update_inst_relate_aabbs_by_refnos_incremental(&target_visible_refnos, true)
                        .await?;
                } else {
                    update_inst_relate_aabbs_by_refnos(&target_visible_refnos, true).await?;
                }
                aabb_ms += t_aabb.elapsed().as_millis();
            }
        }
        println!(
            "模型结点更新耗时: {} ms（深度查询 {} / 网格生成 {} / AABB落库 {} / 布尔运算 {}）",
            time.elapsed().as_millis(),
            query_ms,
            mesh_ms,
            aabb_ms,
            boolean_ms
        );
    }
    Ok(())
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
    #[cfg(not(feature = "occ"))]
    {
        let _ = (refnos, replace_exist, dir);
        eprintln!("warning: gen_inst_meshes skipped (feature `occ` disabled)");
        return Ok(());
    }
    #[cfg(feature = "occ")]
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
        let thing_has_neg_map = inst_geo_ids
            .iter()
            .map(|(x, y)| (x.as_ref().unwrap().id.to_raw(), *y))
            .collect::<HashMap<_, _>>();
        let thing_has_neg_map_arc = Arc::new(thing_has_neg_map);
        // dbg!(&thing_map);
        let mut tasks = vec![];
        // 跨任务共享只为收尾回填 `EXIST_MESH_GEO_HASHES`；库内记录由各任务自己写。
        let aabb_map = Arc::new(DashMap::new());
        for (idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
            let ids = chunk
                .into_iter()
                .map(|(x, _)| x.as_ref().unwrap().to_string())
                .join(",");
            let thing_neg_map = thing_has_neg_map_arc.clone();
            let dir = dir.clone();
            let aabb_map = aabb_map.clone();
            let task = crate::data_interface::staging::write_context::spawn_with_staged_io(
                async move {
                    let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();
                    // 形状都建不出来的几何。它们进不了 `shapes_map`，所以下面那句
                    // `set bad = true` 一辈子轮不到它们——得在这里自己记下来。
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
                                //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
                                #[cfg(any(
                                    feature = "debug_model",
                                    feature = "debug_model_no_obj"
                                ))]
                                println!("gen mesh param: {}", &g.id);
                                //检查是否是PrimPolyhedron
                                let is_polyhedron = match &g.param {
                                    PdmsGeoParam::PrimPolyhedron(_) => true,
                                    _ => false,
                                };
                                match g.param.gen_occ_shape() {
                                    Ok(shape) => {
                                        let mut aabb = Aabb::new_invalid();
                                        for edge in shape.edges() {
                                            for point in
                                                edge.approximation_segments_custom(2.0, 2.0)
                                            {
                                                aabb.take_point(nalgebra::Point3::new(
                                                    point.x as f32,
                                                    point.y as f32,
                                                    point.z as f32,
                                                ));
                                            }
                                        }
                                        //如果作为负实体，需要缩小一些范围？如果作为负实体的母体，需要把精度提高一些
                                        //如果是作为负实体可以稍微降一些？
                                        let mut coeff = 0.005;
                                        // dbg!(&g.id);
                                        if thing_neg_map.get(&g.id).copied().unwrap_or(false) {
                                            match g.param {
                                                PdmsGeoParam::PrimExtrusion(_)
                                                | PdmsGeoParam::PrimRevolution(_) => {
                                                    coeff /= 10.0;
                                                    // dbg!(&coeff);
                                                }
                                                _ => {
                                                    coeff /= 5.0;
                                                }
                                            };
                                        }

                                        let mut tol = if is_polyhedron {
                                            0.01
                                        } else {
                                            (aabb.half_extents().magnitude() as f64 * coeff)
                                                .min(50.0)
                                        };
                                        // dbg!(tol);
                                        shapes_map.insert(g.id, (shape, tol));
                                    }
                                    // 参数不可能出几何（空轮廓的挤出体就是一例）。
                                    // :417 的取数按 `!out.bad` 过滤，所以标不上 bad
                                    // 就等于每一轮生成都把同一份废参数重算一遍。
                                    Err(e) => {
                                        let affected = aios_core::query_refnos_by_geo_hash(&g.id)
                                            .await
                                            .unwrap_or_default();
                                        eprintln!(
                                            "几何 {} 建不出形状，标记跳过（波及 {} 个构件）：{e}",
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
                                update_sql
                                    .push_str(&format!("update inst_geo:⟨{id}⟩ set bad = true;"));
                            }

                            for (id, (s, tol)) in &shapes_map {
                                let mut m_tol = *tol;
                                // dbg!(m_tol);
                                let mut success = false;
                                // #[cfg(feature = "debug_model")]
                                // s.write_step(format!("{}.step", id)).unwrap();
                                #[cfg(any(
                                    feature = "debug_model",
                                    feature = "debug_model_no_obj"
                                ))]
                                println!("gen mesh hash: {}", id);
                                match PlantMesh::gen_occ_mesh(s, m_tol) {
                                    Ok(mesh) if mesh.aabb.is_none() => {
                                        // 三角化出来没有包围盒，与失败等价。原先这里
                                        // `continue` 跳过了下面的标记，于是它也每轮重算。
                                        eprintln!("几何 {id} 三角化后没有包围盒，标记跳过");
                                    }
                                    Ok(mesh) => {
                                        #[cfg(feature = "debug_model")]
                                        mesh.export_obj(false, &format!("{}.obj", id));
                                        // dbg!((id, m_tol, mesh.vertices.len()));
                                        //保存到文件到dir下
                                        mesh.ser_to_file(&dir.join(format!("{}.mesh", id)))
                                            .map_err(|error| {
                                                anyhow!("save generated mesh {id} failed: {error}")
                                            })?;
                                        #[cfg(feature = "debug_model")]
                                        mesh.export_obj(false, &format!("{}.obj", id));
                                        let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                                        let mut pt_hashes = HashSet::new();
                                        for edge in s.edges() {
                                            //TODO edge 这里取中点就可以了
                                            // for point in edge.approximation_segments_custom(1.0, 1.0) {
                                            for point in [edge.start_point(), edge.end_point()] {
                                                let pts_hash = RsVec3(point.as_vec3()).gen_hash();
                                                pt_hashes.insert(format!("vec3:⟨{}⟩", pts_hash));
                                                if !chunk_pts.contains_key(&pts_hash) {
                                                    chunk_pts.insert(
                                                        pts_hash,
                                                        serde_json::to_string(&point).map_err(
                                                            |error| {
                                                                anyhow!(
                                                                    "serialize mesh point failed: {error}"
                                                                )
                                                            },
                                                        )?,
                                                    );
                                                }
                                            }
                                        }
                                        update_sql.push_str(&format!(
                                        "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩, pts=[{}];",
                                        id,
                                        aabb_hash,
                                        pt_hashes.into_iter().join(","),
                                    ));
                                        aabb_map
                                            .entry(aabb_hash.to_string())
                                            .or_insert(mesh.aabb.unwrap());
                                        chunk_aabbs
                                            .entry(aabb_hash.to_string())
                                            .or_insert(mesh.aabb.unwrap());
                                        success = true;
                                    }
                                    //显示哪些模型可能会受影响
                                    Err(e) => {
                                        let affected = aios_core::query_refnos_by_geo_hash(id)
                                            .await
                                            .unwrap_or_default();
                                        eprintln!(
                                            "几何 {id} 三角化失败，标记跳过（波及 {} 个构件）：{e}",
                                            affected.len()
                                        );
                                        #[cfg(any(
                                            feature = "debug_model",
                                            feature = "debug_model_no_obj",
                                            feature = "log_error"
                                        ))]
                                        println!("受影响的构件: {affected:?}");
                                    }
                                }
                                if !success {
                                    //有问题的模型，就不需要每次都重复生成了
                                    update_sql.push_str(&format!(
                                        "update inst_geo:⟨{}⟩ set bad=true;",
                                        id
                                    ));
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
                },
            );
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
    } // cfg(feature = "occ")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: Thing,
    pub refno: RefnoEnum,
    pub noun: String,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    #[serde(deserialize_with = "deserialize_transform_flexible")]
    pub world_trans: Transform,
    /// 更新前已存在的包围盒。`rstar` 的 `remove` 按整值相等匹配，拿新值删不掉旧条目，
    /// 只有带上它才能把 R 树里的旧条目清干净（ADR-010 D3）。
    #[serde(default, deserialize_with = "deserialize_optional_aabb_flexible")]
    pub old_aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    #[serde(deserialize_with = "deserialize_transform_flexible")]
    pub trans: Transform,
    #[serde(deserialize_with = "deserialize_aabb_flexible")]
    pub aabb: Aabb,
}

/// SurrealDB preserves the numeric kind of stored array members. Geometry
/// records written with an integral coordinate therefore come back as `i64`,
/// while Bevy/glam and parry derive strict `f32` deserializers. Accept every
/// finite JSON/Surreal numeric representation at this database boundary and
/// normalize it to the engine's `f32` scalar.
#[derive(Debug, Clone, Copy)]
struct FlexibleF32(f32);

impl<'de> serde::Deserialize<'de> for FlexibleF32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = FlexibleF32;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an integer or floating-point coordinate")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_f32<E>(self, value: f32) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(serde::Deserialize)]
struct AabbWire {
    mins: [FlexibleF32; 3],
    maxs: [FlexibleF32; 3],
}

impl AabbWire {
    fn into_aabb(self) -> Aabb {
        let [min_x, min_y, min_z] = self.mins;
        let [max_x, max_y, max_z] = self.maxs;
        Aabb::new(
            Point::new(min_x.0, min_y.0, min_z.0),
            Point::new(max_x.0, max_y.0, max_z.0),
        )
    }
}

#[derive(serde::Deserialize)]
struct TransformWire {
    translation: [FlexibleF32; 3],
    rotation: [FlexibleF32; 4],
    scale: [FlexibleF32; 3],
}

impl TransformWire {
    fn into_transform(self) -> Transform {
        let [tx, ty, tz] = self.translation;
        let [rx, ry, rz, rw] = self.rotation;
        let [sx, sy, sz] = self.scale;
        Transform {
            translation: glam::Vec3::new(tx.0, ty.0, tz.0),
            rotation: glam::Quat::from_array([rx.0, ry.0, rz.0, rw.0]),
            scale: glam::Vec3::new(sx.0, sy.0, sz.0),
        }
    }
}

fn deserialize_aabb_flexible<'de, D>(deserializer: D) -> Result<Aabb, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <AabbWire as serde::Deserialize>::deserialize(deserializer).map(AabbWire::into_aabb)
}

fn deserialize_optional_aabb_flexible<'de, D>(deserializer: D) -> Result<Option<Aabb>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<AabbWire> as serde::Deserialize>::deserialize(deserializer)
        .map(|value| value.map(AabbWire::into_aabb))
}

fn deserialize_transform_flexible<'de, D>(deserializer: D) -> Result<Transform, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <TransformWire as serde::Deserialize>::deserialize(deserializer)
        .map(TransformWire::into_transform)
}

/// 一个元素的包围盒确实变了。
///
/// `noun` 决定它进哪条房间分支（ADR-010 §2）：PANE 自己一动，整间房的成员全变，
/// 元素级表达不了，必须整块面板重算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AabbChange {
    pub refno: RefnoEnum,
    pub noun: String,
}

/// Return explicit targets that currently have a usable AABB.
///
/// A post-regeneration action cannot use `GLOBAL_AABB_TREE` as its old baseline:
/// the root generator may already have synchronized the new box into that tree. The
/// action itself proves a pose changed in this window, so geometry existence is the
/// final gate that excludes ANCI and other no-geometry nouns.
pub async fn existing_geometric_aabb_changes(
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<AabbChange>> {
    #[derive(serde::Deserialize)]
    struct Row {
        refno: RefnoEnum,
        noun: String,
    }

    let mut changes = Vec::new();
    for chunk in refnos.chunks(100) {
        if chunk.is_empty() {
            continue;
        }
        let keys = get_inst_relate_keys(chunk);
        let mut response = crate::data_interface::staging::active_data_db()
            .query(format!(
                "SELECT in AS refno, in.noun AS noun FROM {keys} WHERE aabb.d != NONE;"
            ))
            .await?
            .check()?;
        changes.extend(
            response
                .take::<Vec<Row>>(0)?
                .into_iter()
                .map(|row| AabbChange {
                    refno: row.refno,
                    noun: row.noun,
                }),
        );
    }
    changes.sort_by_key(|change| change.refno);
    changes.dedup_by_key(|change| change.refno);
    Ok(changes)
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
/// 包围盒**确实变了**的那些元素。房间归属的触发源就是它（ADR-010 §4）。
///
/// 变更基线取**空间树上的旧值**而不是行内的 `old_aabb`：定向重生成走的是「先删行再
/// 重插」（`save_instance_data(replace_exist)`），行内旧值在刷新时刻恒为 none 或者
/// 恒等于刚插入的新值，拿它作基线会退化成「根下每个元素每次重生成都算变」；树上的
/// 条目跨过删行重插存活，才是房间系统上一次真正看到的状态。树上没有条目（首次见到）
/// 同样算变——房间系统从没算过它，正需要一次回填。
///
/// 新值优先从 geo 侧重算；重算不出（`geo_aabbs` 为空或不可用）而行内有既有指针的，
/// 以指针值为准——隐含直管段（TUBI/BOXI）的 aabb 由生成层在插入时算好，geo 侧的
/// 共享单位几何没有 `aabb`/`pts`，此前这类行被整体跳过，从未进过空间树，也就从未
/// 参与过房间归属。
///
/// 注意返回的是「变更集」而不是「处理过的集合」——`manual_update_aabbs` 这类全量重刷
/// 会把整个库喂进来，按处理集入队等于给每个元素都排一次房间重算。
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    update_inst_relate_aabbs_by_refnos_mode(refnos, replace_exist, false).await
}

/// 定向增量刷新入口。
///
/// 与普通刷新的差别只剩两点。其一，直写路径把 `model_update_pending` 房间任务也放进
/// 那个事务——全量生成本来就以 `build_room_relations` 的整体重建收尾，逐元素排房间
/// 任务等于给每个元素排一次重算。其二，写锁从**读输入之前**就取：本入口的调用方
/// （定向 regen 与 TransformOnly）会对同一个 refno 反复刷新，锁只跨事务的话，两次
/// 刷新可以先后算出 A、B 再按 B、A 的顺序落树，把陈旧的 A 发布在最后。
///
/// 两条路径共有的部分（指针写与 spatial epoch bump 同事务、事务成功后才推进
/// `GLOBAL_AABB_TREE`、锁跨 [判定 → 事务 → 同步]）不因入口而异。暂存路径一律不提前
/// 发布控制面任务，仍由窗口尾事务统一收口。
pub async fn update_inst_relate_aabbs_by_refnos_incremental(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    update_inst_relate_aabbs_by_refnos_mode(refnos, replace_exist, true).await
}

async fn update_inst_relate_aabbs_by_refnos_mode(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    durable_room_trigger: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    const CHUNK: usize = 100;
    let staged_writes = crate::data_interface::staging::active_staging_writes();
    let mut changes = Vec::new();
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        // The durable direct path must serialize before it reads the geometry /
        // transform inputs. Taking the lock only after `new_boxes` was computed
        // allowed two refreshes of the same refno to calculate A then B, acquire
        // the lock in reverse order, and publish stale A last. The plain direct
        // path takes the same locks further down, once the expensive input read
        // is behind it.
        //
        // 锁序（一致性闭环方案 D6）：SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE。
        // 空间串行锁把直写路径与 staged 提交后收敛、指针重建换树段、快照发布
        // 串成一条线；声明顺序保证释放顺序相反（先还树锁再还串行锁）。
        let mut _direct_serial = None;
        let mut direct_tree = None;
        if staged_writes.is_none() && durable_room_trigger {
            _direct_serial =
                Some(crate::fast_model::spatial_state::lock_spatial_serial().await);
            direct_tree = Some(GLOBAL_AABB_TREE.write().await);
        }
        let mut rstar_objs = Vec::new();
        let inst_keys = get_inst_relate_keys(chunk);
        let mut sql = format!(
            r#"select id, in as refno, world_trans.d as world_trans, in.noun as noun, aabb.d as old_aabb,
            (select out.aabb.d as aabb, trans.d as trans from out->geo_relate where out.aabb.d != none and trans.d != none)
            as geo_aabbs from {inst_keys} where world_trans.d != none"#,
        );
        //替换所有的aabb
        if !replace_exist {
            sql.push_str(" and aabb=none");
        }
        // 失败即中止，整批上抛（与 persist_latest_main_data 同一纪律）：本函数所有写入
        // 幂等，调用方把整批当一个任务结算、重放收敛。此前这里是 `.unwrap()` + 反序列化
        // 失败静默 continue——传输抖动直接 panic（同款 panic 在生产日志有实证，os error
        // 10054），坏一块就无声丢掉 100 个元素的包围盒与房间触发。
        let db = crate::data_interface::staging::active_data_db();
        let mut response = db
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("查询 inst_relate 包围盒输入失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("查询 inst_relate 包围盒输入语句失败: {e}"))?;
        let result: Vec<QueryAabbParam> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析 inst_relate 包围盒输入失败: {e}"))?;
        let chunk_aabbs: DashMap<String, Aabb> = DashMap::new();
        let mut update_sql = String::new();
        // 本块每行的「当前真值」，树同步与变更判定共用。
        let mut new_boxes: Vec<(RefnoEnum, String, Aabb)> = Vec::new();
        for r in result {
            let mut computed = Aabb::new_invalid();
            for g in r.geo_aabbs {
                let t = r.world_trans * g.trans;
                let tmp_aabb = g.aabb.scaled(&t.scale.into());
                let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                    rotation: t.rotation.into(),
                    translation: t.translation.into(),
                });
                computed.merge(&tmp_aabb);
            }
            let magnitude = computed.extents().magnitude();
            let new_box = if magnitude.is_nan() || magnitude.is_infinite() {
                // geo 侧重算不出。有既有指针的（隐含直管段这类插入时写死 aabb 的行）
                // 以指针值为当前真值；两头都没有的才是真的无几何可用，跳过。
                match r.old_aabb {
                    Some(existing) => existing,
                    None => {
                        #[cfg(feature = "debug_model")]
                        dbg!("Found nan aabb");
                        continue;
                    }
                }
            } else {
                // 只有重算出来的值需要写库；指针回退的那条本来就是库里现值
                // （TUBI 这类建行写死 aabb 的行，aabb_d 也在建行时一并写过）。
                // aabb_d 与指针同语句原子写（P4 写时物化）：值在内存，渲染
                // 纯字面量，journal 维持纯数据。
                let aabb_hash = gen_bytes_hash::<_, 64>(&computed).to_string();
                let aabb_json = serde_json::to_string(&computed)
                    .map_err(|e| anyhow::anyhow!("序列化 Aabb 失败: {e}"))?;
                chunk_aabbs.entry(aabb_hash.clone()).or_insert(computed);
                update_sql.push_str(&format!(
                    "update {} set aabb = aabb:⟨{}⟩, aabb_d = {};",
                    r.refno.to_inst_relate_key(),
                    aabb_hash,
                    aabb_json,
                ));
                computed
            };
            rstar_objs.push(RStarBoundingBox::new(new_box, r.refno, r.noun.clone()));
            new_boxes.push((r.refno, r.noun, new_box));
        }
        // 变更必须在任何指针写入、内存树推进之前按旧树判定。直写事务若随后失败，
        // 指针和树都还留在旧基线，原模型任务重试时仍能再次得到同一批房间目标。
        let target_refnos = new_boxes
            .iter()
            .map(|(refno, _, _)| refno.refno())
            .collect::<HashSet<_>>();
        // 普通直写分支在这里补上写锁，跨度是 [变更判定 → 事务 → 树同步]。空闲轮否则
        // 可能在「DB epoch 已递增、树尚未同步」的极窄窗口把旧树盖上新 epoch sidecar；
        // 并发的删除清理也会挤进事务与同步之间，让刚摘掉的条目又被同步回树上。刻意
        // 不把它提前到读输入段——那一段含几何 join，是全量生成里最贵的部分，而镜像
        // 一致性只要求「要不要 bump」与「树到底动没动」由同一个加锁快照裁决。
        // durable 增量的锁更早（读输入之前就取，见上），这里只接管普通直写分支。
        // 锁序同上：先空间串行锁、后树写锁。
        if staged_writes.is_none() && direct_tree.is_none() {
            _direct_serial =
                Some(crate::fast_model::spatial_state::lock_spatial_serial().await);
            direct_tree = Some(GLOBAL_AABB_TREE.write().await);
        }
        let stale_by_refno = if let Some(tree) = direct_tree.as_ref() {
            let mut stale = HashMap::<RefU64, Vec<Aabb>>::new();
            for old in tree.iter().filter(|old| target_refnos.contains(&old.refno)) {
                stale.entry(old.refno).or_default().push(old.aabb);
            }
            stale
        } else {
            // 暂存窗口这一轮不动树，读一次持久主库的旧基线就够：窗口内的变化寄存进
            // 上下文，提交后由 `sync_tree_from_committed_pointers` 按已提交指针收敛。
            let tree = GLOBAL_AABB_TREE.read().await;
            let mut stale = HashMap::<RefU64, Vec<Aabb>>::new();
            for old in tree.iter().filter(|old| target_refnos.contains(&old.refno)) {
                stale.entry(old.refno).or_default().push(old.aabb);
            }
            stale
        };
        let chunk_changes = new_boxes
            .iter()
            .filter_map(|(refno, noun, new_box)| {
                let olds = stale_by_refno
                    .get(&refno.refno())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                tree_box_changed(olds, new_box).then(|| AabbChange {
                    refno: *refno,
                    noun: noun.clone(),
                })
            })
            .collect::<Vec<_>>();

        // aabb 记录先落库、指针后落库（与 trans 记录同一条 D9 教训，方向不能反）：
        // 反过来的话，两条语句之间的并发读者与中途崩溃都会看到指向缺位记录的指针，
        // `aabb.d` 为 none，元素从 `where aabb.d != none` 的所有读者里整条消失。
        utils::save_aabb_to_surreal(&chunk_aabbs).await?;
        if let Some(context) = &staged_writes {
            if !update_sql.is_empty() {
                crate::surreal_retry::execute_model_write(
                    &update_sql,
                    "update inst_relate aabb pointers",
                )
                .await?;
            }
            let refnos = new_boxes
                .iter()
                .map(|(refno, _, _)| *refno)
                .collect::<Vec<_>>();
            context.defer_spatial_refresh(&refnos).await;
            context.defer_room_changes(&chunk_changes).await;
            continue;
        }

        if !chunk_changes.is_empty() {
            // 本块确有包围盒变化 → 指针写与 epoch bump 必须同事务，无论是不是定向增量。
            // 直写路径不产生 `spatial_reconcile` 意图行，epoch 是它在库侧留下的**唯一**
            // 痕迹：少 bump 一次，落盘前崩溃的重启就会看到 sidecar 与库指纹相等、按
            // Reuse 复用一棵陈旧的树，而 /health 的 drift 恒为 false，没有人看得见。
            // 关掉房间增量、或走非定向的全量生成，都只摘掉 room_recalc 这一条语句。
            let room_upserts = (durable_room_trigger && crate::options::room_incremental())
                .then(|| {
                    crate::data_interface::model_update_pending::render_room_recalc_upserts(
                        &chunk_changes,
                    )
                });
            let mut statements = Vec::with_capacity(3);
            if !update_sql.is_empty() {
                statements.push(update_sql.clone());
            }
            if let Some(room_upserts) = room_upserts {
                statements.push(room_upserts);
            }
            statements.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            let transaction =
                crate::data_interface::increment_pipeline::wrap_in_transaction(&statements)
                    .expect("直写 AABB 事务至少包含 epoch bump");
            crate::surreal_retry::execute_surreal_checked(
                &transaction,
                "update inst_relate aabb pointers with spatial epoch bump",
            )
            .await?;
        } else if !update_sql.is_empty() {
            // 重算值与树上旧值逐位相等：库侧「树应有内容」没变，不 bump——没动树的
            // 提交不该作废别人已经落好的树文件。
            crate::surreal_retry::execute_model_write(
                &update_sql,
                "update inst_relate aabb pointers",
            )
            .await?;
        }

        // 崩溃窗口 ①（一致性闭环方案 §8）：DB 事务已提交、内存树未同步。epoch 已
        // 随事务 bump，重启判据必然认出指纹失配并走指针重建。
        crate::fast_model::spatial_state::failpoint("spatial_direct_after_db_commit");

        // 内存树只在本块 DB 写入全部成功后才动：失败块不留「树新库旧」的半掺状态。
        // sync_refnos 一次遍历摘掉这些 refno 的全部旧条目（含历史堆叠的重复）并插入新值。
        let tree = direct_tree
            .as_mut()
            .expect("直写分支必须持有写锁直到树同步结束");
        tree.sync_refnos(rstar_objs.clone());
        if !rstar_objs.is_empty() || !stale_by_refno.is_empty() {
            crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
        }
        drop(direct_tree);
        changes.extend(chunk_changes);
    }

    // aabb 一到位，行才够格进 insts_flat 清扫（谓词含 `aabb.d != none`）：置脏，
    // 空闲轮收口（P4 写时物化）。
    crate::fast_model::pdms_inst::mark_insts_flat_dirty();

    Ok(changes)
}

/// 「这个元素的包围盒相对房间系统上一次看到的状态变了吗」的唯一判据（纯函数）。
///
/// 不变的唯一形态是：树上恰有一条旧条目且与新值逐位相等。没有旧条目是「首次见到」
/// ——房间从没算过它，必须回填；多于一条是历史堆叠的残留——状态本身已经坏了，
/// 重算一次才能收敛。
fn tree_box_changed(old_entries: &[Aabb], new_box: &Aabb) -> bool {
    !(old_entries.len() == 1 && old_entries[0] == *new_box)
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

#[cfg(feature = "occ")]
pub async fn apply_insts_boolean_occ(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let inst_keys = get_inst_relate_keys(refnos);
    //筛选出来 "Neg", "CataCrossNeg" 的关联
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
    match crate::data_interface::staging::active_data_db()
        .query(&sql)
        .await
    {
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
                            if crate::surreal_retry::execute_model_write(
                                &update_sql,
                                "mark boolean model state",
                            )
                            .await
                            .is_err()
                            {
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
#[cfg(feature = "occ")]
pub async fn apply_cata_neg_boolean_occ(dir: PathBuf) -> anyhow::Result<()> {
    let sql = r#"
        select in as refno, (->inst_info)[0] as inst_info_id, (select value array::flatten([geom_refno, cata_neg])
        from ->inst_info->geo_relate where visible and !out.bad and cata_neg!=none) as boolean_group from inst_relate where (->inst_info)[0]!=none and has_cata_neg and !bad_bool and !booled
    "#;
    let mut response = crate::data_interface::staging::active_data_db()
        .query(sql)
        .await?
        .check()?;
    let params: Vec<CataNegGroup> = response.take(0)?;
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
        let task = crate::data_interface::staging::write_context::spawn_with_staged_io(
            async move {
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
                    let mut resp = crate::data_interface::staging::active_data_db()
                        .query(&sql)
                        .await?
                        .check()?;
                    let gms = resp
                        .take::<Vec<GmGeoData>>(0)
                        .map_err(|error| anyhow!("decode OCC boolean inputs failed: {error}"))?;
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
                                if let Ok(mesh) = PlantMesh::gen_occ_mesh(&pos_shape, tol as _) {
                                    mesh.ser_to_file(&dir_clone.join(format!("{}.mesh", new_id)))
                                        .map_err(|error| {
                                            anyhow!(
                                                "save OCC boolean mesh {new_id} failed: {error}"
                                            )
                                        })?;
                                    // `new_id` 是确定性的：上一次尝试可能已经把这条提交进
                                    // 持久层、批里后面的语句才失败，重试重放必须落到同一行
                                    // 上而不是永远卡在 record already exists（manifold 侧
                                    // `render_catalogue_manifold_result_write` 同款）。
                                    update_sql.push_str(&format!(
                                        "upsert inst_geo:⟨{}⟩ set meshed = true, aabb = {}, visible = true;",
                                        new_id, &pos.aabb_id
                                    ));
                                    update_sql.push_str(&format!(
                                        "INSERT RELATION IGNORE INTO geo_relate [{{ id: geo_relate:[{}, inst_geo:⟨{}⟩], in: {}, out: inst_geo:⟨{}⟩, geom_refno: pe:{}, geo_type: 'Pos', trans: trans:⟨0⟩ }}];",
                                        &g.inst_info_id,
                                        new_id,
                                        &g.inst_info_id,
                                        new_id,
                                        format!("{}_b", bg[0]),
                                    ));
                                    update_sql.push_str(&format!(
                                        "update {}<-inst_relate set booled=true;",
                                        &g.inst_info_id,
                                    ));
                                } else {
                                    update_sql.push_str(&format!(
                                        "update {}<-inst_relate set bad_bool=true;",
                                        &g.inst_info_id,
                                    ));
                                }
                            }
                        }
                    }
                    if !update_sql.is_empty() {
                        crate::surreal_retry::execute_model_write(
                            &update_sql,
                            "mark catalogue boolean model state",
                        )
                        .await?;
                    }
                }
                Ok::<(), anyhow::Error>(())
            },
        );
        tasks.push(task);
    }
    let task_results = futures::future::join_all(tasks).await;
    for result in task_results {
        let result = result.map_err(|error| anyhow!("OCC boolean worker join failed: {error}"))?;
        result?;
    }

    Ok(())
}

#[cfg(test)]
mod aabb_write_order_tests {
    #[test]
    fn mesh_workers_propagate_query_write_and_join_failures() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn gen_inst_meshes(")
            .expect("gen_inst_meshes exists")
            .1
            .split_once("pub async fn update_inst_relate_aabbs_by_refnos(")
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
            .split_once("pub async fn update_inst_relate_aabbs_by_refnos(")
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

    /// `aabb:⟨hash⟩` 记录必须先于 `inst_relate.aabb` 指针落库（与 `trans` 记录同一条
    /// D9 教训）。顺序一旦被整理代码时悄悄换回去，不会有任何编译或运行报错——只会在
    /// 崩溃/并发窗口里让 `aabb.d` 读者取到 none。这里把书写顺序钉成断言。
    #[test]
    fn aabb_records_persist_before_the_pointers_that_reference_them() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("async fn update_inst_relate_aabbs_by_refnos_mode(")
            .expect("update_inst_relate_aabbs_by_refnos_mode must exist")
            .1
            .split_once("\n#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        let records_at = body
            .find("save_aabb_to_surreal(&chunk_aabbs)")
            .expect("per-chunk aabb record insert missing");
        let pointers_at = body
            .find("update inst_relate aabb pointers")
            .expect("pointer update missing");
        let tree_at = body
            .find("tree.sync_refnos(rstar_objs.clone())")
            .expect("tree update missing");

        assert!(
            records_at < pointers_at,
            "aabb 记录必须先于指针落库，否则指针会指向缺位记录"
        );
        assert!(
            pointers_at < tree_at,
            "内存树必须在本块 DB 写入全部成功之后才动，失败块不得留下树新库旧的半掺状态"
        );
    }

    #[test]
    fn direct_increment_publishes_pointer_room_trigger_and_epoch_before_tree_sync() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("async fn update_inst_relate_aabbs_by_refnos_mode(")
            .expect("update_inst_relate_aabbs_by_refnos_mode must exist")
            .1
            .split_once("\n#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        let classify_at = body
            .find("let chunk_changes =")
            .expect("old-tree change classification missing");
        let lock_at = body
            .find("Some(GLOBAL_AABB_TREE.write().await)")
            .expect("direct tree lock missing");
        let input_at = body
            .find("查询 inst_relate 包围盒输入失败")
            .expect("aabb input query missing");
        let records_at = body
            .find("save_aabb_to_surreal(&chunk_aabbs)")
            .expect("aabb record insert missing");
        let pointer_at = body
            .find("statements.push(update_sql.clone())")
            .expect("pointer statement is not part of the direct transaction");
        assert!(
            body.contains("render_room_recalc_upserts"),
            "durable room upsert renderer missing"
        );
        let room_at = body
            .find("statements.push(room_upserts)")
            .expect("room upserts are not part of the direct transaction");
        let epoch_at = body
            .find("render_spatial_epoch_bump")
            .expect("spatial epoch bump missing");
        let commit_at = body
            .find("execute_surreal_checked(")
            .expect("direct transaction execution missing");
        let tree_at = body
            .find("tree.sync_refnos(rstar_objs.clone())")
            .expect("tree sync missing");

        assert!(
            lock_at < input_at,
            "直写锁必须先于本块输入查询，禁止提交陈旧快照"
        );
        assert!(classify_at < records_at, "变化判定必须发生在任何持久写之前");
        assert!(records_at < pointer_at, "AABB 内容记录必须先于指针事务");
        assert!(
            pointer_at < room_at && room_at < epoch_at,
            "指针、房间任务、epoch 顺序漂移"
        );
        assert!(
            epoch_at < commit_at && commit_at < tree_at,
            "事务成功之前不得推进内存树"
        );
        assert!(body.contains("wrap_in_transaction(&statements)"));
    }

    /// 直写路径凡使「树应有内容」发生变化的已提交变更，必在同一事务内 bump spatial
    /// epoch（2026-08-12 方案 G1）。
    ///
    /// 门控一旦退回 `durable_room_trigger && ...`，全量生成与 `manual_update_aabbs`
    /// 的提交又会变成无痕迹变更：它们不产生 `spatial_reconcile` 意图行，落盘前崩溃
    /// 的重启于是看到 sidecar 与库指纹相等、按 Reuse 复用一棵陈旧的树，而 /health
    /// 的 drift 恒为 false，没有人看得见。回退即红。
    #[test]
    fn every_direct_box_change_bumps_the_spatial_epoch_in_the_same_transaction() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("async fn update_inst_relate_aabbs_by_refnos_mode(")
            .expect("update_inst_relate_aabbs_by_refnos_mode must exist")
            .1
            .split_once("\n#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        assert!(
            !body.contains("durable_room_trigger && !chunk_changes.is_empty()"),
            "事务与 bump 不得再由 durable_room_trigger 门控: {body}"
        );
        assert!(
            body.contains("if !chunk_changes.is_empty() {"),
            "本块确有包围盒变化就必须走事务 + bump: {body}"
        );
        // durable_room_trigger 从此只决定「要不要随事务发布房间任务」。
        assert!(
            body.contains("durable_room_trigger && crate::options::room_incremental()"),
            "room_upserts 的门控漂移: {body}"
        );

        let bump_at = body
            .find("render_spatial_epoch_bump")
            .expect("spatial epoch bump missing");
        let plain_at = body
            .find("} else if !update_sql.is_empty() {")
            .expect("无变化的普通写分支必须存在");
        assert!(
            bump_at < plain_at,
            "唯一允许不 bump 的直写是「重算值与树上旧值逐位相等」那一支: {body}"
        );
    }

    /// 普通直写分支必须在「变更判定 → 事务 → 树同步」之前拿到写锁并一直持有。
    ///
    /// 只在同步那一瞬取锁有两个交错窗口：空闲轮可以在「epoch 已递增、树尚未同步」
    /// 之间把旧树盖上新章；并发的删除清理可以挤在事务与同步之间，让刚摘掉的条目又
    /// 被这里同步回树上，成为要等下次指针重建才自愈的幽灵。回退即红。
    #[test]
    fn the_plain_direct_branch_holds_the_tree_lock_across_its_transaction() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("async fn update_inst_relate_aabbs_by_refnos_mode(")
            .expect("update_inst_relate_aabbs_by_refnos_mode must exist")
            .1
            .split_once("\n#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        let lock_at = body
            .find("direct_tree = Some(GLOBAL_AABB_TREE.write().await)")
            .expect("普通直写分支必须补上写锁");
        let classify_at = body
            .find("let chunk_changes =")
            .expect("old-tree change classification missing");
        let commit_at = body
            .find("execute_surreal_checked(")
            .expect("direct transaction execution missing");
        let sync_at = body
            .find("tree.sync_refnos(rstar_objs.clone())")
            .expect("tree sync missing");

        assert!(lock_at < classify_at, "变更判定必须在锁下: {body}");
        assert!(
            classify_at < commit_at && commit_at < sync_at,
            "判定 / 事务 / 同步的次序漂移: {body}"
        );
        assert!(
            !body.contains("let mut tree = GLOBAL_AABB_TREE.write().await;"),
            "同步时才临时取锁等于把锁纪律退回原样: {body}"
        );
    }

    /// 锁序（一致性闭环方案 D6）：`SPATIAL_STATE_SERIAL` 必须先于 `GLOBAL_AABB_TREE`
    /// 写锁取得——durable 与普通直写两个获取点都是。次序反过来会与「持串行锁再取
    /// 树锁」的收敛/重建/落盘路径互相等待，形成死锁。崩溃窗口 ① 的注入点必须落在
    /// 「事务提交后、树同步前」。回退即红。
    #[test]
    fn direct_paths_take_the_spatial_serial_lock_before_the_tree_lock() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("async fn update_inst_relate_aabbs_by_refnos_mode(")
            .expect("update_inst_relate_aabbs_by_refnos_mode must exist")
            .1
            .split_once("\n#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        let mut cursor = 0usize;
        let mut lock_pairs = 0usize;
        while let Some(offset) =
            body[cursor..].find("direct_tree = Some(GLOBAL_AABB_TREE.write().await)")
        {
            let tree_at = cursor + offset;
            assert!(
                body[cursor..tree_at].contains("lock_spatial_serial().await"),
                "第 {} 个树写锁获取点之前必须先取空间串行锁: {body}",
                lock_pairs + 1
            );
            lock_pairs += 1;
            cursor = tree_at + 1;
        }
        assert_eq!(
            lock_pairs, 2,
            "durable 与普通直写应各有一个取锁点: {body}"
        );

        let commit_at = body
            .find("execute_surreal_checked(")
            .expect("direct transaction execution missing");
        let fail_at = body
            .find("failpoint(\"spatial_direct_after_db_commit\")")
            .expect("崩溃窗口 ① 注入点缺失");
        let sync_at = body
            .find("tree.sync_refnos(rstar_objs.clone())")
            .expect("tree sync missing");
        assert!(
            commit_at < fail_at && fail_at < sync_at,
            "崩溃注入点必须落在事务提交后、树同步前: {body}"
        );
    }

    #[test]
    fn targeted_regen_and_transform_use_the_incremental_aabb_entrypoint() {
        let regen = include_str!("occ_generate.rs")
            .split_once("pub async fn process_meshes_update_db_deep(")
            .expect("process_meshes_update_db_deep exists")
            .1
            .split_once("pub async fn update_inst_relate_aabbs_by_refnos(")
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

    /// 最终 AABB 必须在布尔关系落库之后再刷新。只保留布尔前那一次，会让按需生成
    /// 返回原始正实体包围盒，而增量链因后续 post_regen_aabb 偶然得到另一结果。
    #[test]
    fn boolean_generation_refreshes_aabb_after_final_relations_exist() {
        let body = include_str!("occ_generate.rs")
            .split_once("pub async fn process_meshes_update_db_deep(")
            .expect("process_meshes_update_db_deep exists")
            .1
            .split_once("pub async fn update_inst_relate_aabbs_by_refnos(")
            .expect("aabb refresh boundary")
            .0;
        let boolean_at = body
            .find("apply_insts_boolean_manifold(&target_visible_refnos")
            .expect("final boolean stage exists");
        let final_refresh_at = body[boolean_at..]
            .find("update_inst_relate_aabbs_by_refnos_incremental")
            .map(|offset| boolean_at + offset)
            .expect("targeted generation must refresh after boolean relations");
        assert!(boolean_at < final_refresh_at, "{body}");
    }

    /// 2026-08-12 epoch 痕迹方案 §6 场景 2/5 的 live 验收：普通直写刷新
    /// （全量生成 / `manual_update_aabbs` 走的 H2 分支）的三段语义——
    /// 树上缺该条目时刷新必 bump、逐位相等的重刷不 bump、落盘前「崩溃」后
    /// 重启按指针重建且树追上库。
    ///
    /// 崩溃用「清空内存树 + 重新走启动加载」模拟，语义等价性与
    /// `helper.rs::live_direct_delete_crash_before_persist_recovers_by_rebuild`
    /// 的注释同一论证；真杀进程的剧本归 W5 门禁故障注入轮。用 testbed 沙箱跑
    /// （先 `run_full_loop.py` 完成基线+生成），会推进 epoch 并重建项目树文件。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 用已跑过基线+生成的 testbed 沙箱库（见 python/testbed/README.md）"]
    async fn live_direct_refresh_crash_before_persist_recovers_by_rebuild() {
        use aios_core::accel_tree::acceleration_tree::AccelerationTree;
        use aios_core::room::room::GLOBAL_AABB_TREE;

        aios_core::init_test_surreal().await.expect("connect surreal");
        // 本用例经直写刷新自己喂树、不走启动装载：按状态机的测试装载模式显式
        // 声明，否则进程态停在 Uninitialized，基线 persist 会被发布门拒绝
        // （一致性闭环方案 §2 步骤 0；用例写于状态机落地之前，2026-08-12 补）。
        crate::fast_model::spatial_state::mark_spatial_tree_fixture_preloaded();
        let pending =
            crate::data_interface::side_effect_pending::SideEffectCompensator::has_pending_spatial_work()
                .await
                .expect("query pending spatial work");
        assert!(
            !pending,
            "沙箱库还有未收敛的空间意图（会走 HealByReplay 而不是本用例要验的 Rebuild）"
        );

        // 采一个已生成、带双指针的普通实例（TUBI 走指针回退分支，不在本用例口径）。
        let mut response = aios_core::SUL_DB
            .query(
                "SELECT VALUE in FROM inst_relate \
                 WHERE generic != 'TUBI' AND aabb.d != none AND world_trans.d != none LIMIT 1;",
            )
            .await
            .expect("sample query transport")
            .check()
            .expect("sample query");
        let sample: Option<aios_core::RefnoEnum> = response
            .take::<Vec<aios_core::RefnoEnum>>(0)
            .expect("decode sample refno")
            .into_iter()
            .next();
        let refno =
            sample.expect("沙箱库里没有带指针的实例——先跑 python/testbed/run_full_loop.py");

        // 基线：树上还没有它 → 第一次刷新必须 bump（first sighting counts as changed），
        // 随后落盘，文件与库指纹自洽。
        super::update_inst_relate_aabbs_by_refnos(&[refno], true)
            .await
            .expect("baseline refresh");
        assert!(
            GLOBAL_AABB_TREE
                .read()
                .await
                .iter()
                .any(|entry| entry.refno == refno.refno()),
            "样本元素必须能算出包围盒并进树——换个样本或先跑生成"
        );
        crate::fast_model::aabb_tree::persist_aabb_tree()
            .await
            .expect("baseline persist");
        let baseline = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(baseline["drift"], false, "基线必须自洽: {baseline}");
        let epoch_baseline = baseline["db_epoch"].as_u64().expect("baseline db epoch");

        // 逐位相等的重刷：库侧「树应有内容」没变，不得 bump、不得作废树文件。
        super::update_inst_relate_aabbs_by_refnos(&[refno], true)
            .await
            .expect("no-op refresh");
        let unchanged = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(
            unchanged["db_epoch"].as_u64(),
            Some(epoch_baseline),
            "逐位相等的重刷不得 bump: {unchanged}"
        );
        assert_eq!(unchanged["drift"], false, "无变化重刷不得制造漂移: {unchanged}");

        // 树落后于库（全量生成中途的形态）：刷新必须 bump 并把树追上。
        GLOBAL_AABB_TREE
            .write()
            .await
            .remove_by_refnos(&std::collections::HashSet::from([refno.refno()]));
        super::update_inst_relate_aabbs_by_refnos(&[refno], true)
            .await
            .expect("catch-up refresh");
        let bumped = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(
            bumped["db_epoch"].as_u64(),
            Some(epoch_baseline + 1),
            "树上缺条目的刷新必须恰好 bump 一次: {bumped}"
        );
        assert_eq!(
            bumped["drift"], true,
            "落盘前的漂移必须在 /health 可见: {bumped}"
        );

        // 模拟崩溃重启：进程态丢失，文件陈旧 → 指纹失配且无意图 → 指针重建。
        *GLOBAL_AABB_TREE.write().await = AccelerationTree::load(Vec::new());
        crate::fast_model::aabb_tree::load_project_tree_verified()
            .await
            .expect("startup load");
        let recovered = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(
            recovered["startup_verdict"], "rebuilt",
            "指纹失配且无意图必须走指针重建: {recovered}"
        );
        assert!(
            GLOBAL_AABB_TREE
                .read()
                .await
                .iter()
                .any(|entry| entry.refno == refno.refno()),
            "重建后的树必须追上库指针（样本元素回到树上）"
        );
        assert_eq!(
            recovered["drift"], false,
            "重建落盘后指纹必须追平: {recovered}"
        );
    }
}

#[cfg(test)]
mod aabb_change_tests {
    use super::tree_box_changed;
    use parry3d::bounding_volume::Aabb;
    use parry3d::math::Point;

    fn cube(min: f32, max: f32) -> Aabb {
        Aabb::new(Point::new(min, min, min), Point::new(max, max, max))
    }

    /// 变更基线是树上的旧值：恰有一条且逐位相等才算「没变」。定向重生成走「先删行再
    /// 重插」，行内 old_aabb 恒为 none/新值，若拿它作基线，根下每个元素每次重生成都
    /// 会白排一次房间任务（ADR-010 §4 的差异信号被结构性摧毁）。
    #[test]
    fn unchanged_only_when_exactly_one_equal_entry() {
        let unchanged = [cube(0.0, 10.0)];
        assert!(!tree_box_changed(&unchanged, &cube(0.0, 10.0)));
        assert!(
            tree_box_changed(&unchanged, &cube(0.0, 11.0)),
            "盒子动了必须算变"
        );
    }

    /// 树上没有条目 = 房间系统从没见过它，必须回填一次——隐含直管段此前从未进树，
    /// 靠的正是这条语义完成一次性补账。
    #[test]
    fn first_sighting_counts_as_changed() {
        assert!(tree_box_changed(&[], &cube(0.0, 10.0)));
    }

    /// 历史堆叠的重复条目（update_aabbs 写反的去重条件留下的）说明状态已经坏了，
    /// 即使其中一条与新值相等也要重算一次才能收敛。
    #[test]
    fn historic_duplicates_force_a_recalc() {
        let stacked = [cube(0.0, 10.0), cube(5.0, 15.0)];
        assert!(tree_box_changed(&stacked, &cube(0.0, 10.0)));
    }
}

#[cfg(test)]
mod flexible_geometry_deserialize_tests {
    use super::{deserialize_aabb_flexible, deserialize_transform_flexible};
    use bevy_transform::prelude::Transform;
    use parry3d::bounding_volume::Aabb;

    #[derive(serde::Deserialize)]
    struct Row {
        #[serde(deserialize_with = "deserialize_aabb_flexible")]
        aabb: Aabb,
        #[serde(deserialize_with = "deserialize_transform_flexible")]
        transform: Transform,
    }

    /// Surreal keeps mathematically integral coordinates as `i64` even when
    /// neighboring coordinates are floats. The EQUI move regression contained
    /// exactly `-48340i64` inside an otherwise floating-point AABB.
    #[test]
    fn aabb_and_transform_accept_mixed_integer_and_float_coordinates() {
        let row: Row = serde_json::from_value(serde_json::json!({
            "aabb": {
                "mins": [-16390, -48900.023, -1640.3],
                "maxs": [-16240.0, -48340, -1600.0002]
            },
            "transform": {
                "translation": [-16315, -48900, -1640.3],
                "rotation": [0, 0.0, -0.70710677, 0.70710677],
                "scale": [1, 1.0, 1]
            }
        }))
        .expect("mixed Surreal numeric kinds must deserialize");

        assert_eq!(row.aabb.mins.y, -48900.023);
        assert_eq!(row.aabb.maxs.y, -48340.0);
        assert_eq!(row.transform.translation.x, -16315.0);
        assert_eq!(row.transform.scale, glam::Vec3::ONE);
    }
}
