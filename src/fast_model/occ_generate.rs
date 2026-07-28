use crate::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use crate::fast_model::{EXIST_MESH_GEO_HASHES, utils};
use crate::surreal_retry::execute_surreal_checked;
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
    RefU64, RefnoEnum, SUL_DB, gen_bytes_hash, get_inst_relate_keys, query_deep_neg_inst_refnos,
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
use parry3d::math::Isometry;
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
                let aabb_changes =
                    update_inst_relate_aabbs_by_refnos(&update_refnos, true).await?;
                // 几何重生成后包围盒变了 → 房间归属可能变（ADR-010 §4）。房间任务是
                // `drain` 的第三阶段，排在本轮 regen 之后，因此在这里入队正好被它捡起。
                //
                // 只在**定向**重生成时入队。`debug_root_refnos` 是 `gen_all_geos_data`
                // 用来区分两条分支的同一个信号，定向那条由 `ModelRefreshPolicy` 独家设置。
                // 全量生成会把整库元素都算成「包围盒从无到有」，逐个入队等于给每个元素
                // 排一次房间重算；而全量生成本来就以 `build_room_relations` 的整体重建
                // 收尾，那些任务纯属浪费。
                if dboption.debug_root_refnos.is_some() {
                    crate::data_interface::model_update_pending::enqueue_room_recalc(
                        dboption,
                        &aabb_changes,
                    )
                    .await?;
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
        let mut i = 0;
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
        let mut response = SUL_DB.query(sql).await?;
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
        //: DashMap<u64, String>
        let aabb_map = Arc::new(DashMap::new());
        let pts_json_map = Arc::new(DashMap::new());
        for (idx, chunk) in inst_geo_ids.chunks(PAGE_NUM).enumerate() {
            let ids = chunk
                .into_iter()
                .map(|(x, _)| x.as_ref().unwrap().to_string())
                .join(",");
            let thing_neg_map = thing_has_neg_map_arc.clone();
            let dir = dir.clone();
            let aabb_map = aabb_map.clone();
            let pts_json_map = pts_json_map.clone();
            let task = tokio::spawn(async move {
                let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();
                // 形状都建不出来的几何。它们进不了 `shapes_map`，所以下面那句
                // `set bad = true` 一辈子轮不到它们——得在这里自己记下来。
                let mut unbuildable: Vec<String> = Vec::new();
                let sql = format!(
                    "select <string> record::id(id) as id, param from [{}] where param != NONE",
                    ids
                );
                // println!("sql is {}", &sql);
                match SUL_DB.query(&sql).await {
                    Ok(mut response) => {
                        let r = response.take::<Vec<QueryGeoParam>>(0);
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
                        // dbg!(&result);
                        for g in result {
                            //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
                            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
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
                                        for point in edge.approximation_segments_custom(2.0, 2.0) {
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
                                        (aabb.half_extents().magnitude() as f64 * coeff).min(50.0)
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
                            update_sql.push_str(&format!("update inst_geo:⟨{id}⟩ set bad = true;"));
                        }

                        for (id, (s, tol)) in &shapes_map {
                            let mut m_tol = *tol;
                            // dbg!(m_tol);
                            let mut success = false;
                            // #[cfg(feature = "debug_model")]
                            // s.write_step(format!("{}.step", id)).unwrap();
                            #[cfg(any(feature = "debug_model", feature = "debug_model_no_obj"))]
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
                                    if mesh.ser_to_file(&dir.join(format!("{}.mesh", id))).is_ok() {
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
                                                if !pts_json_map.contains_key(&pts_hash) {
                                                    pts_json_map.insert(
                                                        pts_hash,
                                                        serde_json::to_string(&point).unwrap(),
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
                                        success = true;
                                    }
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
                                update_sql
                                    .push_str(&format!("update inst_geo:⟨{}⟩ set bad=true;", id));
                            }
                        }
                        if !update_sql.is_empty() {
                            //执行SUL_DB update,使用chunk 保存
                            if let Err(_) = SUL_DB.query(&update_sql).await {
                                init_save_database_error(
                                    &update_sql,
                                    &std::panic::Location::caller().to_string(),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        init_query_error(&sql, e, &std::panic::Location::caller().to_string());
                    }
                }
            });
            tasks.push(task);
        }

        match futures::future::try_join_all(tasks).await {
            Ok(_) => {}
            Err(e) => {
                dbg!(e);
            }
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

        utils::save_pts_to_surreal(&pts_json_map).await;
        // TODO(与 inst_relate 同款的 D9 顺序问题)：inst_geo.aabb 指针在上面的并发任务里
        // 先落，这里才补 aabb 记录。彻底修复需要把记录写入挪进每个任务、先于其 update；
        // 本轮先保证失败不再被静默吞掉。
        utils::save_aabb_to_surreal(&aabb_map).await?;

        Ok(())
    } // cfg(feature = "occ")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: Thing,
    pub refno: RefnoEnum,
    pub noun: String,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    pub world_trans: Transform,
    /// 更新前已存在的包围盒。`rstar` 的 `remove` 按整值相等匹配，拿新值删不掉旧条目，
    /// 只有带上它才能把 R 树里的旧条目清干净（ADR-010 D3）。
    #[serde(default)]
    pub old_aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    pub trans: Transform,
    pub aabb: Aabb,
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
    const CHUNK: usize = 100;
    let mut changes = Vec::new();
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
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
        let mut response = SUL_DB
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
                // 只有重算出来的值需要写库；指针回退的那条本来就是库里现值。
                let aabb_hash = gen_bytes_hash::<_, 64>(&computed).to_string();
                chunk_aabbs.entry(aabb_hash.clone()).or_insert(computed);
                update_sql.push_str(&format!(
                    "update {} set aabb = aabb:⟨{}⟩;",
                    r.refno.to_inst_relate_key(),
                    aabb_hash,
                ));
                computed
            };
            rstar_objs.push(RStarBoundingBox::new(new_box, r.refno, r.noun.clone()));
            new_boxes.push((r.refno, r.noun, new_box));
        }
        // aabb 记录先落库、指针后落库（与 trans 记录同一条 D9 教训，方向不能反）：
        // 反过来的话，两条语句之间的并发读者与中途崩溃都会看到指向缺位记录的指针，
        // `aabb.d` 为 none，元素从 `where aabb.d != none` 的所有读者里整条消失。
        utils::save_aabb_to_surreal(&chunk_aabbs).await?;
        if !update_sql.is_empty() {
            execute_surreal_checked(&update_sql, "update inst_relate aabb pointers").await?;
        }
        // 内存树只在本块 DB 写入全部成功后才动：失败块不留「树新库旧」的半掺状态。
        // sync_refnos 一次遍历摘掉这些 refno 的全部旧条目（含历史堆叠的重复）并插入
        // 新值，返回的旧条目正是变更判定的基线。
        let stale = {
            let mut tree = GLOBAL_AABB_TREE.write().await;
            tree.sync_refnos(rstar_objs.clone())
        };
        if !rstar_objs.is_empty() || !stale.is_empty() {
            crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
        }
        let mut stale_by_refno: HashMap<RefU64, Vec<Aabb>> = HashMap::new();
        for old in stale {
            stale_by_refno.entry(old.refno).or_default().push(old.aabb);
        }
        for (refno, noun, new_box) in new_boxes {
            let olds = stale_by_refno
                .get(&refno.refno())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if tree_box_changed(olds, &new_box) {
                changes.push(AabbChange { refno, noun });
            }
        }
    }

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
#[cfg(feature = "occ")]
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

#[cfg(test)]
mod aabb_write_order_tests {
    /// `aabb:⟨hash⟩` 记录必须先于 `inst_relate.aabb` 指针落库（与 `trans` 记录同一条
    /// D9 教训）。顺序一旦被整理代码时悄悄换回去，不会有任何编译或运行报错——只会在
    /// 崩溃/并发窗口里让 `aabb.d` 读者取到 none。这里把书写顺序钉成断言。
    #[test]
    fn aabb_records_persist_before_the_pointers_that_reference_them() {
        let source = include_str!("occ_generate.rs");
        let body = source
            .split_once("pub async fn update_inst_relate_aabbs_by_refnos(")
            .expect("update_inst_relate_aabbs_by_refnos must exist")
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
            .find("GLOBAL_AABB_TREE.write()")
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
        assert!(tree_box_changed(&unchanged, &cube(0.0, 11.0)), "盒子动了必须算变");
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
