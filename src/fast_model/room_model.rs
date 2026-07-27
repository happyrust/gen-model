use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::increment_pipeline::wrap_in_transaction;
use crate::fast_model::room_predicate::{
    AabbVerdict, aabb_is_usable, any_point_inside, center_distance, count_vertices_inside,
    element_in_panel, verdict_of,
};
use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::options::DbOption;
use aios_core::room::algorithm::*;
use aios_core::room::room::{GLOBAL_AABB_TREE, load_aabb_tree};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, GeomPtsQuery, ModelHashInst, RefU64, SUL_DB};
use aios_core::{RefnoEnum, init_demo_test_surreal, init_test_surreal};
use bevy_transform::TransformPoint;
use bevy_transform::components::Transform;
use dashmap::{DashMap, DashSet};
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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[tokio::test]
#[ignore = "manual integration: requires the configured Surreal project and room mesh files"]
pub async fn test_cal_rooms() -> anyhow::Result<()> {
    let option = init_test_surreal().await?;
    let refno = "24381/35844".into();
    // process_meshes_update_db_deep(None, (&["24381/34303".into(), refno]))
    //     .await
    //     .unwrap();
    load_aabb_tree().await.unwrap();
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
#[ignore = "manual integration: requires the configured Surreal project and mesh files"]
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
    let room_panel_map = build_room_panels_relate(&room_key_words).await?;
    // 排除集覆盖**所有**面板，包括命名不合规、因而不参与归属计算的那些房间的：
    // 面板本身不该成为另一间房的成员。
    let exclude_panel_refnos = &room_panel_map.all_panels;
    println!(
        "房间归属重建: {} 间房 / {} 块面板",
        room_panel_map.rooms.len(),
        exclude_panel_refnos.len()
    );

    // 单块面板失败不中断整轮：每块面板的写入是自己的事务、先清后写、可重放，
    // 一个坏面板拖垮全量重建只会让其余 123 间房也拿不到结果。失败逐条收集，
    // 收尾统一上抛——既不静默，也不放大。
    let mut failures = Vec::new();
    for room in &room_panel_map.rooms {
        for &panel_refno in &room.panels {
            let members =
                match cal_room_refnos(&mesh_dir, panel_refno, exclude_panel_refnos, 0.1).await {
                    Ok(members) => members,
                    Err(error) => {
                        failures.push(format!("{panel_refno} 计算房间成员失败: {error:#}"));
                        continue;
                    }
                };
            // 成员为空也要写：先清后写里那一步 DELETE 正是「面板挪走后旧成员必须掉」。
            if let Err(error) = save_room_relate(panel_refno, &members, &room.room_num).await {
                failures.push(format!("{panel_refno} 写入房间归属失败: {error:#}"));
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} 块面板的房间归属未能重建: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(())
}

/// 渲染一块面板的房间归属写入：**先清后写**，整体一个事务（ADR-010 §8）。
///
/// 为什么不是差分：差分要先读旧集合再比对，多一次往返，并发下更难保证；先清后写天然
/// 幂等、可重放，而「增量收敛结果 == 全量重建结果」的对拍验收（ADR-010 §9）正是押在
/// 这个幂等性上。此前这里是裸 `relate`，record id 固定为 `panel_member`，第二次跑同一
/// 个 id 会报「已存在」，而调用处没有 `.check()`，错误被静默吞掉——于是全量重建既删不掉
/// 陈旧边，也不会报错（缺陷 D6）。
///
/// 成员为空时只剩那条 DELETE。这不是退化情形而是主要用途之一：面板挪走、房间清空之后，
/// 旧成员必须掉。
///
/// 成员按 refno 排序后渲染：`HashMap` 的遍历顺序每次都不同，不排序的话同一份输入会渲染
/// 出不同的 SQL，重放与逐边对拍都失去意义。
fn render_room_relate_write(
    panel_refno: RefnoEnum,
    within_refnos: &HashMap<RefnoEnum, RoomMember>,
    room_num: &str,
) -> String {
    let panel_key = panel_refno.to_pe_key();
    let mut statements = vec![format!("DELETE room_relate WHERE in = {panel_key}")];

    let mut members: Vec<&RoomMember> = within_refnos.values().collect();
    members.sort_by_key(|member| member.refno.to_string());
    for member in members {
        // inside_count / center_dist 是 fn::room_relate_of 的排序键（ADR-010 §5）。
        statements.push(format!(
            "RELATE {panel_key}->room_relate:{}_{}->{} SET room_num = '{}', \
             inside_count = {}, center_dist = {}",
            panel_refno,
            member.refno,
            member.refno.to_pe_key(),
            escape_surql_str(room_num),
            member.inside_count,
            member.center_dist,
        ));
    }

    wrap_in_transaction(&statements).unwrap_or_default()
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
    within_refnos: &HashMap<RefnoEnum, RoomMember>,
    room_num: &str,
) -> anyhow::Result<()> {
    let sql = render_room_relate_write(panel_refno, within_refnos, room_num);
    SUL_DB
        .query(&sql)
        .await
        .map_err(|error| anyhow::anyhow!("写入 {panel_refno} 的房间归属失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("写入 {panel_refno} 的房间归属语句失败: {error}"))?;
    Ok(())
}

/// 一间通过命名校验、参与房间归属计算的房间及其面板。
#[derive(Debug, Clone)]
pub struct RoomPanels {
    pub room: RefnoEnum,
    pub room_num: String,
    pub panels: Vec<RefnoEnum>,
}

/// 房间 → 面板的映射结果。
///
/// 分成两块是因为两者的范围不同：**所有**面板都要从成员候选里排除（面板本身不该被判成
/// 另一间房的成员），但只有命名通过校验的房间才参与归属计算、才写 `room_relate`。
/// 此前两者共用一个列表，`build_room_relations` 直接遍历它，于是命名校验没通过的房间
/// 照样被写进了 `room_relate`，而它的 `room_panel_relate` 又被跳过——两张表就此对不上。
#[derive(Debug, Clone, Default)]
pub struct RoomPanelMap {
    pub rooms: Vec<RoomPanels>,
    pub all_panels: HashSet<RefnoEnum>,
}

impl RoomPanelMap {
    /// 一块面板所属房间的房间号。
    ///
    /// 只有通过命名校验的房间在册，所以不在册的面板——命名不合规房间的面板，或者
    /// 压根不隶属任何房间的 PANE——拿不到房间号，也就不该产生 `room_relate` 边。
    /// 增量两条分支都靠它决定「这块面板还算不算数」。
    pub fn room_num_of(&self, panel: RefnoEnum) -> Option<&str> {
        self.rooms
            .iter()
            .find(|room| room.panels.contains(&panel))
            .map(|room| room.room_num.as_str())
    }
}

/// 房间命名规则按项目在编译期选定。
fn configured_match_room_fn() -> fn(&str) -> bool {
    #[cfg(feature = "project_hd")]
    return match_room_name_hd;

    #[cfg(feature = "project_hh")]
    return match_room_name_hh;
}

/// 只读地加载房间 → 面板映射，供增量路径使用。
///
/// 与 [`build_room_panels_relate`] 的区别只在于不写 `room_panel_relate`：增量重算
/// 需要的是「谁是面板、面板属于哪间房」这份现状，而不是重建它。
///
/// **调用方需按轮复用**：这是一次房间类型表的全表扫描外加逐行图遍历（本项目
/// 2889 个 FRMW 里筛出 124 间），每个任务各扫一遍，一轮几十个任务就会被它拖垮。
pub async fn load_room_panel_map(db_option: &DbOption) -> anyhow::Result<RoomPanelMap> {
    load_room_panel_groups(&db_option.get_room_key_word(), configured_match_room_fn()).await
}

/// 构建房间和面板之间的关联关系
///
/// # 参数
/// * `room_key_word` - 房间关键词列表,用于匹配房间名称
///
/// # 功能说明
/// 根据不同的项目特性(project_hd或project_hh)调用对应的房间名称匹配函数,
/// 通过 build_room_panels_relate_common 函数构建房间和面板的关联关系
async fn build_room_panels_relate(room_key_word: &Vec<String>) -> anyhow::Result<RoomPanelMap> {
    build_room_panels_relate_common(room_key_word, configured_match_room_fn()).await
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

/// 一间房的 `room_panel_relate` 写入：与 [`render_room_relate_write`] 同样是先清后写。
///
/// 此前是 `relate {room}->room_panel_relate->[{panels}]`，**不带 record id**——每跑一次
/// 就多一批完全重复的边（缺陷 D6）。带上确定的 `{room}_{panel}` id 之后重复不再产生，
/// 而先删本房名下的旧边则让「面板从这间房挪走」也能收敛。
fn render_room_panel_relate_write(
    room_refno: RefnoEnum,
    panels: &[RefnoEnum],
    room_num: &str,
) -> String {
    let room_key = room_refno.to_pe_key();
    let mut statements = vec![format!("DELETE room_panel_relate WHERE in = {room_key}")];
    for panel in panels {
        statements.push(format!(
            "RELATE {room_key}->room_panel_relate:{room_refno}_{panel}->{} SET room_num = '{}'",
            panel.to_pe_key(),
            escape_surql_str(room_num),
        ));
    }
    wrap_in_transaction(&statements).unwrap_or_default()
}

/// 构建房间和面板之间的关联关系
///
/// # 参数
/// * `room_key_word` - 用于匹配房间的关键词列表
/// * `match_room_fn` - 用于匹配房间号的函数
async fn build_room_panels_relate_common<F>(
    room_key_word: &Vec<String>,
    match_room_fn: F,
) -> anyhow::Result<RoomPanelMap>
where
    F: Fn(&str) -> bool,
{
    let map = load_room_panel_groups(room_key_word, match_room_fn).await?;
    write_room_panel_relate(&map).await?;
    Ok(map)
}

/// 从库里读出房间 → 面板的现状，不写任何东西。
async fn load_room_panel_groups<F>(
    room_key_word: &Vec<String>,
    match_room_fn: F,
) -> anyhow::Result<RoomPanelMap>
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

    let mut response = SUL_DB.query(sql).await?.check()?;
    let room_groups: Vec<(RefnoEnum, String, Vec<RefnoEnum>)> = response.take(0)?;

    let mut map = RoomPanelMap::default();
    for (room_refno, room_num_str, panel_refnos) in room_groups {
        map.all_panels.extend(panel_refnos.iter().copied());
        // 判断 room_num是否符合规则
        if !match_room_fn(&room_num_str) {
            continue;
        }
        map.rooms.push(RoomPanels {
            room: room_refno,
            room_num: room_num_str,
            panels: panel_refnos,
        });
    }
    Ok(map)
}

/// 把房间 → 面板的现状写回 `room_panel_relate`。
async fn write_room_panel_relate(map: &RoomPanelMap) -> anyhow::Result<()> {
    let blocks: Vec<String> = map
        .rooms
        .iter()
        .map(|room| render_room_panel_relate_write(room.room, &room.panels, &room.room_num))
        .collect();

    // 每间房一个事务，分批下发：一次几百间房的 SQL 拼成一条会把单条语句撑得过大，
    // 而事务边界本来就该落在「一间房」上——一间房写坏不该回滚其它房间。
    const ROOM_WRITE_CHUNK: usize = 100;
    for chunk in blocks.chunks(ROOM_WRITE_CHUNK) {
        SUL_DB
            .query(chunk.join("\n"))
            .await
            .map_err(|error| anyhow::anyhow!("写入房间-面板关系失败: {error}"))?
            .check()
            .map_err(|error| anyhow::anyhow!("写入房间-面板关系语句失败: {error}"))?;
    }
    Ok(())
}

/// 一个构件相对某块面板的归属，附带排序用的强度。
///
/// 数据模型允许一个件同时属于多间房（横跨两室的桶形件就是），但
/// `fn::room_code` 是 `limit 1` 取首条。没有排序键时取到哪条由存储顺序决定——
/// 全量重建时碰巧稳定，一旦改成增量、边删了再写回去就会变，表现为房间号无规律跳动。
/// 这两个字段给它一个全序（ADR-010 §5、缺陷 D7）。
#[derive(Debug, Clone, Copy)]
pub struct RoomMember {
    pub refno: RefnoEnum,
    /// 元素 AABB 八顶点落在面板内的个数（0–8），越大归属越强。
    pub inside_count: u8,
    /// 元素中心到面板中心的距离，平局时越小越强。
    pub center_dist: f32,
}

impl RoomMember {
    /// 同一个件可能在多次实例遍历中被算到，保留更强的那次。
    fn stronger(self, other: Self) -> Self {
        let mine = (self.inside_count, -self.center_dist);
        let theirs = (other.inside_count, -other.center_dist);
        if theirs.0 > mine.0 || (theirs.0 == mine.0 && theirs.1 > mine.1) {
            other
        } else {
            self
        }
    }
}

fn merge_member(acc: &mut HashMap<RefnoEnum, RoomMember>, member: RoomMember) {
    acc.entry(member.refno)
        .and_modify(|existing| *existing = existing.stronger(member))
        .or_insert(member);
}

/// 第二轮逐点兜底的数据源：一批构件的世界变换、包围盒与实际几何点。
///
/// `where !booled` 是判定口径的一部分——被布尔运算吃掉的实例不再代表真实几何。
/// 正反两个方向都从这里取点，取法不同就等于判定口径不同（ADR-010 §3）。
async fn query_geom_pts(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<GeomPtsQuery>> {
    let pes = refnos.iter().map(RefnoEnum::to_pe_key).join(",");
    let mut response = SUL_DB
        .query(format!(
            r#"select
                 in.id as refno, world_trans.d as world_trans, aabb.d as world_aabb,
                 (select value [trans.d, (->inst_geo[?pts!=none].pts[?d!=none].d) ] from ->inst_info->geo_relate) as pts_group
               from array::flatten([{pes}]->inst_relate)  where !booled
            "#
        ))
        .await
        .map_err(|error| anyhow::anyhow!("查询失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("语句失败: {error}"))?;
    response
        .take::<Vec<GeomPtsQuery>>(0)
        .map_err(|error| anyhow::anyhow!("解析失败: {error}"))
}

pub async fn cal_room_refnos(
    mesh_dir: &PathBuf,
    panel_refno: RefnoEnum,
    exclude_refnos: &HashSet<RefnoEnum>,
    inside_tol: f32,
) -> anyhow::Result<HashMap<RefnoEnum, RoomMember>> {
    //查询到aabb直接完全在这个房间里的mesh里，就不用做点的检查
    // 这里曾经是 `unwrap_or_default()`：面板实例只要有一个字段形状不对
    //（`GeomInstQuery` 的 `pe.owner` / `inst_relate.generic` 都是非 Option 字符串），
    // 反序列化错误就被吞成空 Vec，紧接着下面的 `is_empty()` 让整间房**无声地**算成
    // 0 个成员，日志里一行都没有。合成夹具首跑就是被它藏了半天。
    let geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno], true)
        .await
        .map_err(|error| anyhow::anyhow!("查询面板 {panel_refno} 的实例失败: {error}"))?;
    // dbg!(&geom_insts);
    if geom_insts.is_empty() {
        return Ok(Default::default());
    }

    let mut within_refnos: HashMap<RefnoEnum, RoomMember> = HashMap::new();
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
            let mut read = GLOBAL_AABB_TREE.read().await;
            let mut contains_query = read
                .locate_intersecting_bounds(&geom_inst.world_aabb)
                .collect::<Vec<_>>();
            if contains_query.is_empty() {
                continue;
            }
            // dbg!(&contains_query);
            // 第二轮要用的：既记下待查 refno，也记下它第一轮的顶点计数，
            // 免得点检查通过后还得为了排序再算一遍。
            let mut need_check_refnos: HashMap<RefU64, u8> = HashMap::default();
            let mut vertex_counts: HashMap<RefU64, u8> = HashMap::default();
            contains_query.retain(|RStarBoundingBox { refno, aabb, .. }| {
                if !aabb_is_usable(aabb) {
                    return false;
                }
                //排除自己
                let r: RefnoEnum = (*refno).into();
                if exclude_refnos.contains(&r) || panel_refno.refno() == *refno {
                    return false;
                }
                // 判定口径只有 room_predicate 一份，反向路径调的是同一个（ADR-010 §3）。
                let inside = count_vertices_inside(&tri_mesh, aabb);
                vertex_counts.insert(*refno, inside);
                match verdict_of(inside) {
                    AabbVerdict::Inside => true,
                    AabbVerdict::NeedsPointCheck => {
                        need_check_refnos.insert(*refno, inside);
                        false
                    }
                    AabbVerdict::Outside => false,
                }
            });
            //for test
            // dbg!(tri_mesh.contains_point(&Isometry::identity(), &Point::new(0.0, 0.0, 0.0) ));
            // if !contains_query.is_empty() {
            //     dbg!(&contains_query);
            // }
            for bbox in &contains_query {
                merge_member(
                    &mut within_refnos,
                    RoomMember {
                        refno: bbox.refno.into(),
                        inside_count: vertex_counts.get(&bbox.refno).copied().unwrap_or(8),
                        center_dist: center_distance(&geom_inst.world_aabb, &bbox.aabb),
                    },
                );
            }
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
                // 这一步过去是 `let Ok(..) else { continue }`：查询或反序列化一失败，
                // 整轮逐点兜底被静默跳过，跨界构件就此丢掉归属而不留痕迹。
                let candidates: Vec<RefnoEnum> = need_check_refnos
                    .keys()
                    .map(|refno| RefnoEnum::from(*refno))
                    .collect();
                let geom_pts = query_geom_pts(&candidates).await.map_err(|error| {
                    anyhow::anyhow!("面板 {panel_refno} 的候选几何点: {error:#}")
                })?;
                // dbg!(&geom_pts);
                // Aabb 既不是 Eq 也不是 Hash，所以按 refno 建映射而不是塞进 Set。
                let intersect_set: DashMap<RefnoEnum, Aabb> = DashMap::new();
                geom_pts.par_iter().for_each(|g| {
                    if g.pts_group
                        .par_iter()
                        .find_any(|(trans, o_pts)| {
                            let Some(pts) = o_pts else {
                                return false;
                            };
                            let pt_trans = (g.world_trans * (*trans)).compute_matrix();
                            any_point_inside(
                                &tri_mesh,
                                pts.iter().map(|pt| {
                                    pt_trans.as_dmat4().transform_point3(*pt).as_vec3().into()
                                }),
                            )
                        })
                        .is_some()
                    {
                        intersect_set.insert(g.refno, g.world_aabb);
                    }
                });
                #[cfg(feature = "debug_room")]
                if !intersect_set.is_empty() {
                    println!(
                        "found intersect room panel {}, refnos: {}",
                        panel_refno,
                        &intersect_set.iter().map(|x| x.key().to_string()).join(",")
                    );
                }
                for entry in intersect_set.iter() {
                    let (refno, world_aabb) = (*entry.key(), *entry.value());
                    merge_member(
                        &mut within_refnos,
                        RoomMember {
                            refno,
                            // 第二轮进来的必然是部分包含（1–7），沿用第一轮已算好的计数。
                            inside_count: need_check_refnos
                                .get(&refno.refno())
                                .copied()
                                .unwrap_or(1),
                            center_dist: center_distance(&geom_inst.world_aabb, &world_aabb),
                        },
                    );
                }
            }
        }
    }

    Ok(within_refnos)
}

/// 整间分支：一块面板动了，重算它名下的全部归属（ADR-010 §2）。
///
/// 返回本次写入的成员集合，供同一轮 drain 跳过重复的元素任务（§8 的冲突规则）。
/// 跳过纯粹是省一次网格加载与点检测：两条分支共用 `room_predicate` 的判定、共用
/// `{panel}_{element}` 边 id、都是先清后写，因此谁先谁后都收敛到同一个边集。
pub async fn recalc_panel_membership(
    db_option: &DbOption,
    rooms: &RoomPanelMap,
    panel: RefnoEnum,
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let Some(room_num) = rooms.room_num_of(panel).map(str::to_string) else {
        // 面板已不在册：房间改名后不再合规、面板被挪出房间、或房间本身没了。
        // 旧边仍要清，否则它会一直挂着上一次的归属，且没有任何人会再来碰它。
        save_room_relate(panel, &HashMap::new(), "").await?;
        return Ok(HashSet::new());
    };
    let members = cal_room_refnos(
        &db_option.get_meshes_path(),
        panel,
        &rooms.all_panels,
        0.1,
    )
    .await?;
    save_room_relate(panel, &members, &room_num).await?;
    Ok(members.keys().copied().collect())
}

/// 元素分支：一个构件动了，反向定位它落在哪些面板里（ADR-010 §2）。
pub async fn recalc_element_membership(
    db_option: &DbOption,
    rooms: &RoomPanelMap,
    element: RefnoEnum,
) -> anyhow::Result<()> {
    // 面板本身不参与归属。整间分支把**所有**面板排除在成员之外，反向路径必须同口径，
    // 否则一块面板在两条路径下会得到不同答案。PANE 自身变更按 ADR §2 走整间分支。
    if rooms.all_panels.contains(&element) {
        return Ok(());
    }

    let insts: Vec<GeomInstQuery> = aios_core::query_insts(&[element], true)
        .await
        .map_err(|error| anyhow::anyhow!("查询构件 {element} 的实例失败: {error}"))?;
    // 没有几何、或包围盒不可用的构件不可能属于任何房间——但旧边照样要清掉。
    let Some(element_inst) = insts.into_iter().next() else {
        return write_element_room_relate(element, &[]).await;
    };
    let element_aabb = element_inst.world_aabb;
    if !aabb_is_usable(&element_aabb) {
        return write_element_room_relate(element, &[]).await;
    }

    // 候选面板取自唯一那棵全局树并按 noun 过滤（ADR §6 两树合一），再与在册面板
    // 取交集——不在册的 PANE 没有房间号，写不出边。
    //
    // 用库里的包围盒而不是树里那份：本任务正是「这个构件的包围盒变了」才入队的，
    // 而入队点就是刷新包围盒的那一处，库与树在此刻同步。
    load_aabb_tree().await?;
    let candidates: Vec<RefnoEnum> = {
        let tree = GLOBAL_AABB_TREE.read().await;
        tree.locate_intersecting_bounds(&element_aabb)
            .filter(|bbox| bbox.noun == "PANE")
            .map(|bbox| RefnoEnum::from(bbox.refno))
            .filter(|panel| rooms.room_num_of(*panel).is_some())
            .collect()
    };
    if candidates.is_empty() {
        return write_element_room_relate(element, &[]).await;
    }

    // 一次取齐第二轮要用的实际几何点：候选面板通常只有一两块，为每块各查一次不值当。
    let world_pts = element_world_points(element).await.map_err(|error| {
        anyhow::anyhow!("构件 {element} 的几何点: {error:#}")
    })?;
    let panel_insts: Vec<GeomInstQuery> = aios_core::query_insts(&candidates, true)
        .await
        .map_err(|error| anyhow::anyhow!("查询 {element} 的候选面板实例失败: {error}"))?;

    let mesh_dir = db_option.get_meshes_path();
    let mut edges: HashMap<RefnoEnum, ElementRoomEdge> = HashMap::new();
    for panel_inst in &panel_insts {
        let panel = panel_inst.refno;
        let Some(room_num) = rooms.room_num_of(panel) else {
            continue;
        };
        for inst in &panel_inst.insts {
            let Ok(mesh) =
                PlantMesh::des_mesh_file(&mesh_dir.join(format!("{}.mesh", inst.geo_hash)))
            else {
                continue;
            };
            let Some(tri_mesh) = mesh.get_tri_mesh_with_flag(
                (panel_inst.world_trans * inst.transform).compute_matrix(),
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            ) else {
                continue;
            };
            if !element_in_panel(&tri_mesh, &element_aabb, || world_pts.iter().copied()) {
                continue;
            }
            // 命中的面板通常只有一两块，为排序键再数一遍八顶点比让判定函数多返回
            // 一个计数值划算——后者会让共享谓词为反向路径长出一个专用签名。
            let member = RoomMember {
                refno: element,
                inside_count: count_vertices_inside(&tri_mesh, &element_aabb),
                center_dist: center_distance(&panel_inst.world_aabb, &element_aabb),
            };
            edges
                .entry(panel)
                .and_modify(|edge| edge.member = edge.member.stronger(member))
                .or_insert(ElementRoomEdge {
                    panel,
                    room_num: room_num.to_string(),
                    member,
                });
        }
    }

    let edges: Vec<ElementRoomEdge> = edges.into_values().collect();
    write_element_room_relate(element, &edges).await
}

/// 构件在世界坐标系下的实际几何点，与正向第二轮取的是同一批。
async fn element_world_points(element: RefnoEnum) -> anyhow::Result<Vec<Point<Real>>> {
    let mut points = Vec::new();
    for geom_pts in query_geom_pts(&[element]).await? {
        for (trans, pts) in &geom_pts.pts_group {
            let Some(pts) = pts else {
                continue;
            };
            let pt_trans = (geom_pts.world_trans * (*trans)).compute_matrix();
            points.extend(pts.iter().map(|pt| -> Point<Real> {
                pt_trans.as_dmat4().transform_point3(*pt).as_vec3().into()
            }));
        }
    }
    Ok(points)
}

/// 元素分支算出来的一条归属边。
#[derive(Debug, Clone)]
struct ElementRoomEdge {
    panel: RefnoEnum,
    room_num: String,
    member: RoomMember,
}

/// 元素分支的写入：删掉指向该构件的**所有** `room_relate` 入边，再写回本次算出的边
/// （ADR-010 §8）。
///
/// 边 id 与整间分支逐字一致（`{panel}_{element}`）。这不是巧合而是必要条件：两条
/// 分支迟早会在同一条边上相遇，id 不同就会各写一行，`fn::room_relate_of` 取到哪条
/// 全看存储顺序——正是排序键要消灭的那种不确定性。
fn render_element_relate_write(element: RefnoEnum, edges: &[ElementRoomEdge]) -> String {
    let element_key = element.to_pe_key();
    let mut statements = vec![format!("DELETE room_relate WHERE out = {element_key}")];

    let mut edges: Vec<&ElementRoomEdge> = edges.iter().collect();
    edges.sort_by_key(|edge| edge.panel.to_string());
    for edge in edges {
        statements.push(format!(
            "RELATE {}->room_relate:{}_{}->{element_key} SET room_num = '{}', \
             inside_count = {}, center_dist = {}",
            edge.panel.to_pe_key(),
            edge.panel,
            element,
            escape_surql_str(&edge.room_num),
            edge.member.inside_count,
            edge.member.center_dist,
        ));
    }

    wrap_in_transaction(&statements).unwrap_or_default()
}

async fn write_element_room_relate(
    element: RefnoEnum,
    edges: &[ElementRoomEdge],
) -> anyhow::Result<()> {
    let sql = render_element_relate_write(element, edges);
    SUL_DB
        .query(&sql)
        .await
        .map_err(|error| anyhow::anyhow!("写入 {element} 的房间归属失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("写入 {element} 的房间归属语句失败: {error}"))?;
    Ok(())
}

#[tokio::test]
#[ignore = "manual integration: mutates the configured Surreal project database"]
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
    assert_eq!(result.rooms.len(), 6, "Should return 6 room relationships");

    dbg!(&result);

    // Clean up test data
    // let cleanup_sql = r#"
    //     DELETE FRMW;
    //     DELETE SBFR;
    // "#;
    // SUL_DB.query(cleanup_sql).await?;

    Ok(())
}

/// 写入层的幂等性守护（ADR-010 §8）。
///
/// 全部不连库：断言的是渲染出来的 SQL 本身。「增量收敛结果 == 全量重建结果」的对拍
/// （§9）押在先清后写与确定性渲染上，而这两条恰恰是最容易在后续改动里被悄悄破坏的
/// ——写坏了不会报错，只会让边越积越多或者房间号跳动。
#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> RefnoEnum {
        RefnoEnum::from("4000000001_10")
    }

    fn member(seq: u64, inside_count: u8, center_dist: f32) -> RoomMember {
        RoomMember {
            refno: RefnoEnum::from(format!("4000000001_{seq}").as_str()),
            inside_count,
            center_dist,
        }
    }

    fn members(entries: impl IntoIterator<Item = RoomMember>) -> HashMap<RefnoEnum, RoomMember> {
        entries.into_iter().map(|m| (m.refno, m)).collect()
    }

    fn position_of(sql: &str, needle: &str) -> usize {
        sql.find(needle)
            .unwrap_or_else(|| panic!("渲染结果里找不到 {needle}:\n{sql}"))
    }

    /// 「清」不是可选项：面板挪走、房间清空之后，旧成员边只能靠这条 DELETE 掉。
    /// 此前 `build_room_relations` 里的 `if !refnos.is_empty()` 守卫正好把这种情形跳过，
    /// 于是空房间永远保留着上一次的成员。
    #[test]
    fn empty_member_set_still_clears_the_old_edges() {
        let sql = render_room_relate_write(panel(), &HashMap::new(), "K100");
        assert!(
            sql.contains("DELETE room_relate WHERE in = pe:4000000001_10"),
            "{sql}"
        );
        assert!(!sql.contains("RELATE"), "空成员集不该写任何边:\n{sql}");
    }

    /// 删除必须与写入同处一个事务：中途失败若只落了 DELETE，这块面板的房间归属就凭空消失。
    #[test]
    fn room_relate_write_clears_before_it_writes_in_one_transaction() {
        let sql = render_room_relate_write(
            panel(),
            &members([member(20, 8, 0.0), member(24, 4, 450.0)]),
            "K100",
        );
        assert!(
            position_of(&sql, "DELETE room_relate") < position_of(&sql, "RELATE"),
            "DELETE 必须排在所有 RELATE 之前:\n{sql}"
        );
        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with(";\nCOMMIT TRANSACTION;"), "{sql}");
    }

    /// 固定的 `{panel}_{member}` record id 是幂等的另一半：没有它，同一条边每重建一次
    /// 就新增一行——`room_panel_relate` 此前正是如此。
    #[test]
    fn edge_ids_are_derived_from_both_endpoints() {
        let sql = render_room_relate_write(panel(), &members([member(20, 8, 12.5)]), "K100");
        assert!(
            sql.contains(
                "RELATE pe:4000000001_10->room_relate:4000000001_10_4000000001_20\
                 ->pe:4000000001_20"
            ),
            "{sql}"
        );
        // 排序键跟着边一起写，缺了 fn::room_relate_of 会退化成按 room_num 排序。
        assert!(sql.contains("inside_count = 8"), "{sql}");
        assert!(sql.contains("center_dist = 12.5"), "{sql}");
    }

    /// 同一份成员集必须渲染出同一条 SQL。`HashMap` 每次构造都换哈希种子，遍历顺序
    /// 因此逐个实例不同；不排序的话重放和逐边对拍都失去意义。
    #[test]
    fn rendering_is_stable_across_map_iteration_order() {
        let entries = [member(20, 8, 0.0), member(21, 8, 10.0), member(24, 4, 450.0)];
        let forward = members(entries);
        let backward = members(entries.into_iter().rev());
        let sql = render_room_relate_write(panel(), &forward, "K100");

        assert_eq!(sql, render_room_relate_write(panel(), &backward, "K100"));
        assert!(
            position_of(&sql, "->pe:4000000001_20") < position_of(&sql, "->pe:4000000001_21")
                && position_of(&sql, "->pe:4000000001_21")
                    < position_of(&sql, "->pe:4000000001_24"),
            "成员应按 refno 升序渲染:\n{sql}"
        );
    }

    /// 房间号取自 `NAME` 的末段，是任意库内文本。带引号的名字会把语句截断——
    /// 加了 `.check()` 之后这会变成一个响亮的错误，而不再是静默吞掉。
    #[test]
    fn room_num_is_escaped_into_the_literal() {
        let sql = render_room_relate_write(panel(), &members([member(20, 8, 0.0)]), "K'100");
        assert!(sql.contains(r"room_num = 'K\'100'"), "{sql}");
    }

    #[test]
    fn room_panel_write_clears_then_writes_addressable_edges() {
        let room = RefnoEnum::from("4000000001_1");
        let panels = [
            RefnoEnum::from("4000000001_10"),
            RefnoEnum::from("4000000001_11"),
        ];
        let sql = render_room_panel_relate_write(room, &panels, "K100");

        assert!(
            position_of(&sql, "DELETE room_panel_relate WHERE in = pe:4000000001_1")
                < position_of(&sql, "RELATE"),
            "{sql}"
        );
        assert!(
            sql.contains(
                "RELATE pe:4000000001_1->room_panel_relate:4000000001_1_4000000001_10\
                 ->pe:4000000001_10"
            ),
            "{sql}"
        );
    }

    /// 两条增量分支迟早会在同一条边上相遇：整间分支写 (面板 → 成员)，元素分支写
    /// (面板 → 该构件)。边 id 一旦不同就会各写一行，`fn::room_relate_of` 取到哪条
    /// 全看存储顺序——正是排序键要消灭的那种不确定性。
    #[test]
    fn both_branches_address_the_same_edge_identically() {
        let element = RefnoEnum::from("4000000001_20");
        let member = member(20, 8, 12.5);
        let panel_sql = render_room_relate_write(panel(), &members([member]), "K100");
        let element_sql = render_element_relate_write(
            element,
            &[ElementRoomEdge {
                panel: panel(),
                room_num: "K100".into(),
                member,
            }],
        );

        let edge_id = "room_relate:4000000001_10_4000000001_20";
        assert!(panel_sql.contains(edge_id), "{panel_sql}");
        assert!(element_sql.contains(edge_id), "{element_sql}");
    }

    /// 两条分支的删除范围不同，方向不能弄反：整间分支删面板的**出**边，元素分支删
    /// 构件的**入**边。写成同一个方向，一条分支就会把另一条的结果整片抹掉。
    #[test]
    fn the_element_branch_clears_the_edges_pointing_at_it() {
        let element = RefnoEnum::from("4000000001_20");
        let sql = render_element_relate_write(element, &[]);
        assert!(
            sql.contains("DELETE room_relate WHERE out = pe:4000000001_20"),
            "{sql}"
        );
        assert!(!sql.contains("RELATE"), "{sql}");

        let panel_sql = render_room_relate_write(panel(), &HashMap::new(), "K100");
        assert!(
            panel_sql.contains("DELETE room_relate WHERE in = pe:4000000001_10"),
            "{panel_sql}"
        );
    }

    /// 跨界构件会同时落在两块面板里，边从 `HashMap` 取出，顺序同样不能随缘。
    #[test]
    fn element_edges_render_in_a_stable_panel_order() {
        let element = RefnoEnum::from("4000000001_24");
        let edge = |seq: u64| ElementRoomEdge {
            panel: RefnoEnum::from(format!("4000000001_{seq}").as_str()),
            room_num: "K100".into(),
            member: member(24, 4, 450.0),
        };
        assert_eq!(
            render_element_relate_write(element, &[edge(10), edge(11)]),
            render_element_relate_write(element, &[edge(11), edge(10)]),
        );
    }

    /// 「在排除集里」与「在册」是两回事，增量两条分支都靠这个区分。
    #[test]
    fn only_registered_panels_resolve_to_a_room_number() {
        let unregistered = RefnoEnum::from("4000000001_11");
        let map = RoomPanelMap {
            rooms: vec![RoomPanels {
                room: RefnoEnum::from("4000000001_1"),
                room_num: "K100".into(),
                panels: vec![panel()],
            }],
            all_panels: HashSet::from([panel(), unregistered]),
        };

        assert_eq!(map.room_num_of(panel()), Some("K100"));
        // 命名不合规房间的面板拿不到房间号、不产生 room_relate 边，
        // 但它仍在排除集里——面板不该被别的房间收为成员。
        assert_eq!(map.room_num_of(unregistered), None);
        assert!(map.all_panels.contains(&unregistered));
    }

    /// 面板从这间房挪走之后，房间自己也要能收敛到「一块面板都没有」。
    #[test]
    fn room_with_no_panels_still_clears() {
        let sql = render_room_panel_relate_write(RefnoEnum::from("4000000001_1"), &[], "K100");
        assert!(
            sql.contains("DELETE room_panel_relate WHERE in = pe:4000000001_1"),
            "{sql}"
        );
        assert!(!sql.contains("RELATE"), "{sql}");
    }
}
