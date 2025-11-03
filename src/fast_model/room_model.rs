use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::options::DbOption;
use aios_core::room::algorithm::*;

use aios_core::RecordId;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use aios_core::{RefnoEnum, init_demo_test_surreal, init_test_surreal};
use bevy_transform::TransformPoint;
use bevy_transform::components::Transform;
use dashmap::DashSet;
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

#[tokio::test]
pub async fn test_cal_rooms() -> anyhow::Result<()> {
    let option = init_test_surreal().await?;
    let refno = "24381/35844".into();
    // process_meshes_update_db_deep(None, (&["24381/34303".into(), refno]))
    //     .await
    //     .unwrap();
    // SQLite R*-tree is used for spatial indexing
    build_room_relations(&option).await.unwrap();
    let mesh_path = option.get_meshes_path();
    let within_refnos = cal_room_refnos(&mesh_path, refno, &HashSet::new(), 0.1)
        .await
        .unwrap();
    dbg!(&within_refnos);
    Ok(())
}

//TODO need figure out
#[tokio::test]
pub async fn test_cal_distance() -> anyhow::Result<()> {
    init_test_surreal().await;
    let panel_refno = "24381/34303".into();
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno], true)
        .await
        .unwrap_or_default();
    // dbg!(&geom_insts);
    if geom_insts.is_empty() {
        return Ok(());
    }

    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            let Ok(mesh) =
                PlantMesh::des_mesh_file(&format!("assets/meshes/{}.mesh", inst.geo_hash))
            else {
                continue;
            };
            let Some(mut tri_mesh) =
                mesh.get_tri_mesh_with_flag(inst.transform.to_matrix(), TriMeshFlags::ORIENTED)
            else {
                continue;
            };
            dbg!(tri_mesh.indices().len());
            dbg!(tri_mesh.vertices().len());

            dbg!(tri_mesh.local_aabb());

            let point = Vec3::new(8495.01953125, -8.15999984741211, 0.0);
            dbg!(tri_mesh.local_aabb().contains_local_point(&point.into()));
            dbg!(tri_mesh.contains_local_point(&point.into()));

            let mat = (geom_inst.world_trans * inst.transform).to_matrix();
        }
    }
    return Ok(());
}

/// 构建房间关系
///
/// 该函数用于构建房间之间的空间关系,包括:
/// 1. 根据房间关键词匹配房间和面板的对应关系
/// 2. 计算每个面板内包含的构件
/// 3. 保存房间和构件的关联关系
///
/// # 参数
/// * `db_option` - 数据库配置选项,包含房间关键词等参数
///
/// # 返回值
/// * `anyhow::Result<()>` - 返回构建结果,成功返回Ok(()),失败返回错误信息
pub async fn build_room_relations(db_option: &DbOption) -> anyhow::Result<()> {
    let mesh_dir = db_option.get_meshes_path();
    let room_key_words = db_option.get_room_key_word();
    let room_panel_map = build_room_panels_relate(&room_key_words).await?;
    let exclude_panel_refnos = room_panel_map
        .iter()
        .map(|(_, _, panel_refnos)| panel_refnos.clone())
        .flatten()
        .collect::<HashSet<_>>();
    dbg!(room_panel_map.len());
    // 打印一次 R*-tree 索引统计，确认元素数量
    #[cfg(feature = "sqlite-index")]
    if crate::spatial_index::SqliteSpatialIndex::is_enabled() {
        if let Ok(index) = crate::spatial_index::SqliteSpatialIndex::with_default_path() {
            if let Ok(stats) = index.get_stats() {
                println!(
                    "SQLite R*-tree stats: total_elements={}, index_type={}",
                    stats.total_elements, stats.index_type
                );
            }
        }
    }

    for (_room_refno, room_num, panel_refnos) in room_panel_map {
        for panel_refno in panel_refnos {
            let refnos = cal_room_refnos(&mesh_dir, panel_refno, &exclude_panel_refnos, 0.1)
                .await
                .unwrap();
            if !refnos.is_empty() {
                dbg!(refnos.len());
                save_room_relate(panel_refno, &refnos, &room_num)
                    .await
                    .unwrap();
            }
        }
    }
    Ok(())
}

/// 保存房间关联关系到数据库
///
/// # 参数
/// * `panel_refno` - 面板的引用号
/// * `within_refnos` - 面板内包含的构件引用号集合
/// * `room_num` - 房间号
///
/// # 返回值
/// * `anyhow::Result<()>` - 成功返回Ok(()), 失败返回错误信息
async fn save_room_relate(
    panel_refno: RefnoEnum,
    within_refnos: &HashSet<RefnoEnum>,
    room_num: &str,
) -> anyhow::Result<()> {
    let mut final_sql = "".to_string();
    for refno in within_refnos {
        let relation_id = format!("{}_{}", panel_refno, refno);
        let sql = format!(
            "relate {}->room_relate:{}->{}  set room_num='{}';",
            panel_refno.to_pe_key(),
            relation_id,
            refno.to_pe_key(),
            room_num
        );
        final_sql.push_str(&sql);
    }
    // dbg!(&final_sql);
    SUL_DB.query(&final_sql).await?;
    Ok(())
}

/// 构建房间和面板之间的关联关系
///
/// # 参数
/// * `room_key_word` - 房间关键词列表,用于匹配房间名称
///
/// # 返回值
/// 返回一个元组列表,每个元组包含:
/// * 房间的引用号(RefnoEnum)
/// * 房间号(String)
/// * 该房间关联的面板引用号列表(Vec<RefnoEnum>)
///
/// # 功能说明
/// 根据不同的项目特性(project_hd或project_hh)调用对应的房间名称匹配函数,
/// 通过 build_room_panels_relate_common 函数构建房间和面板的关联关系
async fn build_room_panels_relate(
    room_key_word: &Vec<String>,
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>> {
    #[cfg(feature = "project_hd")]
    return build_room_panels_relate_common(room_key_word, match_room_name_hd).await;

    #[cfg(feature = "project_hh")]
    return build_room_panels_relate_common(room_key_word, match_room_name_hh).await;
}

/// hd 正则匹配是否满足房间命名规则
pub fn match_room_name_hd(room_name: &str) -> bool {
    let regex = Regex::new(r"^[A-Z]\d{3}$").unwrap();
    regex.is_match(room_name)
}

/// hh 正则匹配是否满足房间命名规则
pub fn match_room_name_hh(room_name: &str) -> bool {
    true
}

/// 构建房间和面板之间的关联关系
///
/// # 参数
/// * `room_key_word` - 用于匹配房间的关键词列表
/// * `match_room_fn` - 用于匹配房间号的函数
///
/// # 返回值
/// 返回一个元组列表,每个元组包含:
/// * 房间的引用号(RefnoEnum)
/// * 房间号(String)
/// * 该房间关联的面板引用号列表(Vec<RefnoEnum>)
async fn build_room_panels_relate_common<F>(
    room_key_word: &Vec<String>,
    match_room_fn: F,
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>>
where
    F: Fn(&str) -> bool,
{
    // 拼接判断条件
    let filter = room_key_word
        .iter()
        .map(|x| format!("'{}' in NAME", x))
        .join(" or ");
    //属于room的panel
    #[cfg(feature = "project_hd")]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe, REFNO<-pe_owner<-pe<-pe_owner<-pe])[?noun='PANE']
                    ] from FRMW where {filter}
    "#
    );
    #[cfg(feature = "project_hh")]
    let sql = format!(
        r#"
        select value [  id,
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe])[?noun='PANE']
                    ] from SBFR where {filter}
    "#
    );

    let mut response = SUL_DB.query(sql).await?;

    // 使用Thing类型来处理RecordId
    let raw_result: Vec<(RecordId, String, Vec<RecordId>)> = response.take(0)?;

    // 转换为RefnoEnum
    let room_groups: Vec<(RefnoEnum, String, Vec<RefnoEnum>)> = raw_result
        .into_iter()
        .map(|(room_thing, room_num, panel_things)| {
            // 从Thing中提取RefnoEnum
            let room_refno = RefnoEnum::from(room_thing);
            let refno = if room_refno.is_valid() {
                room_refno
            } else {
                RefnoEnum::default()
            };

            // 转换面板Thing为RefnoEnum
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

            (refno, room_num, panel_refnos)
        })
        .collect();
    let mut sql_string = String::new();
    for (room_refno, room_num_str, panel_refnos) in &room_groups {
        // 判断 room_num是否符合规则
        if !match_room_fn(room_num_str) {
            continue;
        }
        let sql = format!(
            "relate {}->room_panel_relate->[{}] set room_num='{}';",
            room_refno.to_pe_key(),
            panel_refnos.iter().map(|x| x.to_pe_key()).join(","),
            room_num_str
        );
        sql_string.push_str(&sql);
    }
    SUL_DB.query(sql_string).await?;
    Ok(room_groups)
}

pub async fn cal_room_refnos(
    mesh_dir: &PathBuf,
    panel_refno: RefnoEnum,
    exclude_refnos: &HashSet<RefnoEnum>,
    inside_tol: f32,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    //查询到aabb直接完全在这个房间里的mesh里，就不用做点的检查
    let mut geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno], true)
        .await
        .unwrap_or_default();
    // dbg!(&geom_insts);
    if geom_insts.is_empty() {
        return Ok(Default::default());
    }

    let mut within_refnos: HashSet<RefnoEnum> = HashSet::new();
    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            // 构建 panel TriMesh（带简单缓存以避免重复构建）
            use std::collections::HashMap as StdHashMap;
            use std::sync::Arc;
            use std::sync::Mutex;
            use std::sync::OnceLock;

            static PANEL_TRI_CACHE: OnceLock<Mutex<StdHashMap<String, Arc<TriMesh>>>> =
                OnceLock::new();
            let cache = PANEL_TRI_CACHE.get_or_init(|| Mutex::new(StdHashMap::new()));

            let tri_mesh = {
                let mut guard = cache.lock().unwrap();
                if let Some(cached) = guard.get(&inst.geo_hash) {
                    cached.clone()
                } else {
                    let file_path = mesh_dir.join(format!("{}.mesh", inst.geo_hash));
                    let Ok(mesh) = PlantMesh::des_mesh_file(&file_path) else {
                        continue;
                    };
                    let Some(tri) = mesh.get_tri_mesh_with_flag(
                        (geom_inst.world_trans * inst.transform).to_matrix(),
                        TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
                    ) else {
                        continue;
                    };
                    let tri = Arc::new(tri);
                    guard.insert(inst.geo_hash.clone(), tri.clone());
                    tri
                }
            };
            // 使用 SQLite R*-tree 进行空间查询（先 contains 再 intersect）
            let mut contains_query = Vec::new();
            let mut need_check_refnos: HashSet<RefU64> = HashSet::default();
            #[cfg(feature = "sqlite-index")]
            if crate::spatial_index::SqliteSpatialIndex::is_enabled() {
                use crate::spatial_index::SpatialQueryBackend;
                let spatial_index = crate::spatial_index::SqliteSpatialIndex::with_default_path()
                    .expect("Failed to open spatial index");

                // 构建查询参数：带容差、返回 bbox，并排除面板自身与外部排除集合
                let mut opts = crate::spatial_index::QueryOptions::default();
                opts.tolerance = inside_tol.max(0.0);
                opts.include_bbox = true;
                opts.exclude.push(panel_refno.refno());
                for r in exclude_refnos {
                    opts.exclude.push(r.refno());
                }

                // 1) 完全包含：直接计入 contains_query
                let query_aabb: Aabb = geom_inst.world_aabb.into();
                if let Ok(hits) = spatial_index.query_contains_hits(&query_aabb, &opts) {
                    for h in hits {
                        if let Some(bb) = h.bbox {
                            contains_query
                                .push(RStarBoundingBox::from_aabb(bb, RefnoEnum::from(h.refno)));
                        }
                    }
                }

                // 2) 边界相交：作为后续点检查候选（去重避免与 contains 重复）
                if let Ok(mut hits) = spatial_index.query_intersect_hits(&query_aabb, &opts) {
                    let contained_set: std::collections::HashSet<u64> =
                        contains_query.iter().map(|b| b.refno.0).collect();
                    hits.retain(|h| !contained_set.contains(&h.refno.0));
                    for h in hits {
                        need_check_refnos.insert(h.refno);
                    }
                }
                // 统计与日志
                let contains_cnt = contains_query.len();
                let border_cnt = need_check_refnos.len();
                println!(
                    "[Room] panel {} contains={}, border_candidates={}",
                    panel_refno, contains_cnt, border_cnt
                );
            }
            if contains_query.is_empty() && need_check_refnos.is_empty() {
                continue;
            }
            // dbg!(&contains_query);
            // 采样函数：AABB 顶点 + 中心 + 12条边中点
            let sample_aabb_points = |aabb: &Aabb| -> Vec<Point<Real>> {
                let mut pts: Vec<Point<Real>> = aabb.vertices().to_vec();
                let mins = aabb.mins;
                let maxs = aabb.maxs;
                let center = Point::from((mins.coords + maxs.coords) / 2.0);
                pts.push(center);
                let corners = aabb.vertices();
                let add_mid =
                    |a: Point<Real>, b: Point<Real>| Point::from((a.coords + b.coords) / 2.0);
                // 12条边（基于立方体顶点索引拓扑）
                let edges = [
                    (0, 1),
                    (1, 3),
                    (3, 2),
                    (2, 0), // bottom face
                    (4, 5),
                    (5, 7),
                    (7, 6),
                    (6, 4), // top face
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 7), // vertical edges
                ];
                for (i, j) in edges {
                    pts.push(add_mid(corners[i], corners[j]));
                }
                pts
            };

            contains_query.retain(|RStarBoundingBox { refno, aabb, .. }| {
                //filter the wrong aabb
                if aabb.extents().magnitude().is_nan() || aabb.extents().magnitude().is_infinite() {
                    dbg!(refno);
                    return false;
                }
                //排除自己
                let r: RefnoEnum = RefnoEnum::from(RefU64(refno.0));
                if exclude_refnos.contains(&r) || panel_refno.refno() == RefU64(refno.0) {
                    return false;
                }
                // 使用带容差的点测内：距离 <= inside_tol 视为在内
                let samples = sample_aabb_points(aabb);
                let mut all_inside = true;
                let mut any_inside = false;
                for p in &samples {
                    // 优先用距离（容差），其次 fallback contains
                    let d = TriMesh::distance_to_point(&*tri_mesh, &Isometry::identity(), p, true);
                    if d <= inside_tol as Real {
                        any_inside = true;
                    } else if TriMesh::contains_point(&*tri_mesh, &Isometry::identity(), p) {
                        any_inside = true;
                    } else {
                        all_inside = false;
                    }
                }
                if all_inside {
                    return true;
                } else {
                    // 只要有一个点在mesh里面，就需要继续检查是否真的相交
                    if any_inside {
                        need_check_refnos.insert(*refno);
                    }
                    return false;
                }
            });
            //for test
            // dbg!(tri_mesh.contains_point(&Isometry::identity(), &Point::new(0.0, 0.0, 0.0) ));
            // if !contains_query.is_empty() {
            //     dbg!(&contains_query);
            // }
            within_refnos.extend(contains_query.iter().map(|r| {
                let r: RefnoEnum = r.refno.into();
                r
            }));
            // if within_refnos.len() > 1 {
            //     dbg!(&within_refnos);
            // }
            // let need_check_refnos: Vec<RefU64> = vec!["24383_71586".into()];
            // dbg!(&need_check_refnos);
            if !need_check_refnos.is_empty() {
                // dbg!(panel_refno);
                // dbg!(&within_refnos);
                // dbg!(&need_check_refnos);
                //首先判断，如果是包围盒完全不在里面，直接跳过
                //继续的点检查可能会比较耗时，后续应该加开关，让用户判断是否需要继续做检查
                let pes = need_check_refnos.iter().map(|x| x.to_pe_key()).join(",");
                let Ok(mut repsonse) = SUL_DB.query(format!(
                    r#"select
                         in.id as refno, world_trans.d as world_trans, aabb.d as world_aabb,
                         (select value [trans.d, (->inst_geo[?pts!=none].pts[?d!=none].d) ] from ->inst_info->geo_relate) as pts_group
                       from array::flatten([{}]->inst_relate)
                    "#,
                    pes
                ))
                .await else {
                    continue;
                };
                let Ok(raw_values) = repsonse.take::<Vec<JsonValue>>(0) else {
                    continue;
                };
                let Ok(geom_pts) = raw_values
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<GeomPtsQuery>, _>>()
                else {
                    continue;
                };
                // dbg!(&geom_pts);
                let mut intersect_set: DashSet<RefnoEnum> = DashSet::new();
                geom_pts.par_iter().for_each(|g| {
                    if g.pts_group
                        .par_iter()
                        .find_any(|(trans, o_pts)| {
                            if let Some(pts) = o_pts {
                                let pt_trans = (g.world_trans * (*trans)).to_matrix();
                                pts.par_iter()
                                    .find_any(|&pt| {
                                        tri_mesh.contains_point(
                                            &Isometry::identity(),
                                            &pt_trans
                                                .as_dmat4()
                                                .transform_point3((*(*pt)).into())
                                                .as_vec3()
                                                .into(),
                                        )
                                    })
                                    .is_some()
                            } else {
                                false
                            }
                        })
                        .is_some()
                    {
                        // dbg!(g.refno);
                        intersect_set.insert(g.refno);
                    }
                });
                #[cfg(feature = "debug_room")]
                if !intersect_set.is_empty() {
                    println!(
                        "found intersect room panel {}, refnos: {}",
                        panel_refno,
                        &intersect_set.iter().map(|x| x.to_string()).join(",")
                    );
                }
                within_refnos.extend(intersect_set);
                println!(
                    "[Room] panel {} final_within={}",
                    panel_refno,
                    within_refnos.len()
                );

                // dbg!(&within_refnos);
            }
        }
    }

    Ok(within_refnos)
}

#[tokio::test]
async fn test_build_room_panels_relate_common() -> anyhow::Result<()> {
    // Initialize test database
    init_demo_test_surreal().await;

    // Create test hierarchy data
    let create_sql = r#"
        -- Create FRMW node
        CREATE FRMW SET
            id = "FRMW_AE_AC01_R",
            NAME = "AE-AC01-R",
            REFNO = "1000";

        -- Create SBFR nodes under FRMW
        CREATE SBFR SET
            id = "SBFR_AE01055A",
            NAME = "AE-AC01-R-AE01055A",
            REFNO = "1001";
        CREATE SBFR SET
            id = "SBFR_AE01911A",
            NAME = "AE-AC01-R-AE01911A",
            REFNO = "1002";
        CREATE SBFR SET
            id = "SBFR_AE01945A",
            NAME = "AE-AC01-R-AE01945A",
            REFNO = "1003";
        CREATE SBFR SET
            id = "SBFR_AE01907G",
            NAME = "AE-AC01-R-AE01907G",
            REFNO = "1004";
        CREATE SBFR SET
            id = "SBFR_AE01906G",
            NAME = "AE-AC01-R-AE01906G",
            REFNO = "1005";
        CREATE SBFR SET
            id = "SBFR_AE01910A",
            NAME = "AE-AC01-R-AE01910A",
            REFNO = "1006";

        -- Create pe_owner relationships
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01055A;
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01911A;
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01945A;
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01907G;
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01906G;
        RELATE FRMW:FRMW_AE_AC01_R->pe_owner->SBFR:SBFR_AE01910A;
    "#;

    SUL_DB.query(create_sql).await?;

    // Test build_room_panels_relate_common
    let room_key_words = vec!["AE-AC01-R".to_string()];
    let match_room_fn = |room_num: &str| room_num.contains("AE");

    let result = build_room_panels_relate_common(&room_key_words, match_room_fn).await?;

    // Verify results
    assert_eq!(result.len(), 6, "Should return 6 room relationships");

    dbg!(&result);

    // Clean up test data
    // let cleanup_sql = r#"
    //     DELETE FRMW;
    //     DELETE SBFR;
    // "#;
    // SUL_DB.query(cleanup_sql).await?;

    Ok(())
}
