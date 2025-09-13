use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::options::DbOption;
use aios_core::room::algorithm::*;
// Removed GLOBAL_AABB_TREE dependency - using SQLite R*-tree instead
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{init_demo_test_surreal, init_test_surreal, RefnoEnum};
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use bevy_transform::components::Transform;
use bevy_transform::TransformPoint;
use dashmap::DashSet;
use glam::{Mat4, Vec3};
use itertools::Itertools;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Vector};
use parry3d::math::{Point, Real};
use parry3d::query::PointQuery;
use parry3d::shape::{TriMesh, TriMeshFlags};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use regex::Regex;

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
            let Some(mut tri_mesh) = mesh
                .get_tri_mesh_with_flag(inst.transform.compute_matrix(), TriMeshFlags::ORIENTED)
            else {
                continue;
            };
            dbg!(tri_mesh.indices().len());
            dbg!(tri_mesh.vertices().len());

            dbg!(tri_mesh.local_aabb());

            let point = Vec3::new(8495.01953125, -8.15999984741211, 0.0);
            dbg!(tri_mesh.local_aabb().contains_local_point(&point.into()));
            dbg!(tri_mesh.contains_local_point(&point.into()));

            let mat = (geom_inst.world_trans * inst.transform).compute_matrix();
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
    let room_panel_map = build_room_panels_relate(&room_key_words).await.unwrap();
    let exclude_panel_refnos = room_panel_map
        .iter()
        .map(|(_, _, panel_refnos)| panel_refnos.clone())
        .flatten()
        .collect::<HashSet<_>>();
    dbg!(room_panel_map.len());
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
) -> anyhow::Result<Vec<(RefnoEnum, String, Vec<RefnoEnum>)>>{
    #[cfg(feature="project_hd")]
    return build_room_panels_relate_common(room_key_word, match_room_name_hd).await;

    #[cfg(feature="project_hh")]
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
    #[cfg(feature="project_hd")]
    let sql = format!(
        r#"
        select value [  id, 
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe, REFNO<-pe_owner<-pe<-pe_owner<-pe])[?noun='PANE']
                    ] from FRMW where {filter}
    "#
    );
    #[cfg(feature="project_hh")]
    let sql = format!(
        r#"
        select value [  id, 
                        array::last(string::split(NAME, '-')),
                        array::flatten([REFNO<-pe_owner<-pe])[?noun='PANE']
                    ] from SBFR where {filter}
    "#
    );

    let mut response = SUL_DB.query(sql).await?;
    let room_groups: Vec<(RefnoEnum, String, Vec<RefnoEnum>)> = response.take(0)?;
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
            let file_path = mesh_dir.join(format!("{}.mesh", inst.geo_hash));
            let Ok(mesh) = PlantMesh::des_mesh_file(&file_path) else {
                continue;
            };
            // dbg!(&file_path);
            let Some(mut tri_mesh) = mesh.get_tri_mesh_with_flag(
                (geom_inst.world_trans * inst.transform).compute_matrix(),
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            ) else {
                continue;
            };
            // Use SQLite R*-tree for spatial queries
            let mut contains_query = Vec::new();
            #[cfg(feature = "sqlite-index")]
            if crate::spatial_index::SqliteSpatialIndex::is_enabled() {
                let spatial_index = crate::spatial_index::SqliteSpatialIndex::with_default_path()
                    .expect("Failed to open spatial index");
                if let Ok(ids) = spatial_index.query_intersect(&geom_inst.world_aabb) {
                    for id in ids {
                        if let Ok(Some(bbox)) = spatial_index.get_aabb(id) {
                            contains_query.push(RStarBoundingBox::from_aabb(bbox, RefnoEnum::from(id)));
                        }
                    }
                }
            }
            if contains_query.is_empty() {
                continue;
            }
            // dbg!(&contains_query);
            let mut need_check_refnos: HashSet<RefU64> = HashSet::default();
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
                // dbg!(&bbox);
                let contains: Vec<bool> = aabb
                    .vertices()
                    .iter()
                    .map(|x| tri_mesh.contains_point(&Isometry::identity(), &x))
                    .collect::<Vec<_>>();
                //每一个点都在mesh里面
                if contains.iter().all(|&x| x) {
                    return true;
                } else {
                    //只要有一个点在mesh里面，就需要继续检查是否真的相交
                    if contains.iter().any(|&x| x) {
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
                       from array::flatten([{}]->inst_relate)  where !booled
                    "#,
                    pes
                ))
                .await else {
                    continue;
                };
                let Ok(geom_pts) = repsonse.take::<Vec<GeomPtsQuery>>(0) else {
                    continue;
                };
                // dbg!(&geom_pts);
                let mut intersect_set: DashSet<RefnoEnum> = DashSet::new();
                geom_pts.par_iter().for_each(|g| {
                    if g.pts_group
                        .par_iter()
                        .find_any(|(trans, o_pts)| {
                            if let Some(pts) = o_pts {
                                let pt_trans = (g.world_trans * (*trans)).compute_matrix();
                                pts.par_iter()
                                    .find_any(|&pt| {
                                        tri_mesh.contains_point(
                                            &Isometry::identity(),
                                            &pt_trans
                                                .as_dmat4()
                                                .transform_point3(*pt)
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
