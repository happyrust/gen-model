use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::increment_pipeline::wrap_in_transaction;
use crate::fast_model::room_predicate::{
    AabbVerdict, aabb_is_usable, any_point_inside, center_distance, count_vertices_inside,
    element_in_panel, membership_by_aabb, verdict_of,
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
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Isometry, Vector};
use parry3d::math::{Point, Real};
use parry3d::query::PointQuery;
use parry3d::shape::{TriMesh, TriMeshFlags};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[tokio::test]
#[ignore = "manual integration: requires the configured Surreal project and room mesh files"]
pub async fn test_cal_rooms() -> anyhow::Result<()> {
    let option = init_test_surreal().await?;
    load_aabb_tree().await?;

    let rooms = load_room_panel_map(&option).await?;
    assert_eq!(rooms.rooms.len(), 124, "AMS 应识别出 124 间房");
    assert_eq!(rooms.all_panels.len(), 147, "AMS 应识别出 147 块房间面板");
    build_room_relations(&option).await?;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Edge {
        panel: String,
        member: String,
        room_num: String,
        inside_count: i64,
        center_dist: f64,
    }

    let sql = "SELECT record::id(in) AS panel, record::id(out) AS member, \
               room_num, inside_count, center_dist FROM room_relate \
               WHERE string::starts_with(record::id(out), '24381_') \
               ORDER BY panel, member";
    let mut response = SUL_DB.query(sql).await?.check()?;
    let baseline: Vec<Edge> = response.take(0)?;
    let first = baseline
        .first()
        .ok_or_else(|| anyhow::anyhow!("7997 全量房间计算没有产生任何成员边"))?;

    let element = RefnoEnum::from(first.member.replace('_', "/").as_str());
    let panels = load_panel_index(&option, &rooms).await?;
    let history = ElementRoomHistory::load(&[element]).await?;
    recalc_element_membership(&rooms, &panels, &history, element).await?;

    let mut response = SUL_DB.query(sql).await?.check()?;
    let incremental: Vec<Edge> = response.take(0)?;
    assert_eq!(
        incremental, baseline,
        "7997 单构件增量重算必须与全量基线逐边一致"
    );
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

    // 整间分支的成员候选取自空间树（少量面板 × 大量构件，这才是树的正当用途；元素
    // 分支那个反方向的依赖已经拆掉，见 [`PanelIndex`]）。树空着时每块面板都会算出
    // 0 个成员，而这套写入是先清后写——一次启动就足以把整库房间归属抹平。判不了就
    // 不写，与元素分支同一个口径。调用点（`lib.rs`）已把失败降级为告警，启动不受
    // 影响；存量归属边陈旧也比被清成空强，它下一轮还能收敛回来。
    if !room_panel_map.rooms.is_empty() && GLOBAL_AABB_TREE.read().await.is_empty() {
        anyhow::bail!(
            "空间树是空的，{} 间在册房间的成员判不了：跳过本次全量重建，不清掉存量房间\
             归属边。先确认 accel_tree.bin 与库的对账（sync_aabb_tree_with_db）",
            room_panel_map.rooms.len()
        );
    }

    // 重建前的存量成员数，一条查询查完。收尾时用它说出「哪几块面板从有成员变成了 0」
    // ——先清后写这条路上唯一会造成数据损失的转变就是它，而此前全量重建这一侧一行日志
    // 都没有（增量两条分支反倒都有）。查不到就不报这一项，不影响重建本身。
    let previous_members = existing_member_counts().await.unwrap_or_else(|error| {
        println!("[房间全量] 读取存量成员数失败（本次不报成员变化）: {error:#}");
        HashMap::new()
    });

    // 单块面板失败不中断整轮：每块面板的写入是自己的事务、先清后写、可重放，
    // 一个坏面板拖垮全量重建只会让其余 123 间房也拿不到结果。失败逐条收集，
    // 收尾统一上抛——既不静默，也不放大。
    let mut failures = Vec::new();
    let mut without_geometry: Vec<RefnoEnum> = Vec::new();
    let mut written_edges = 0usize;
    let mut emptied: Vec<RefnoEnum> = Vec::new();
    for room in &room_panel_map.rooms {
        for &panel_refno in &room.panels {
            let members = match cal_room_refnos(&mesh_dir, panel_refno, exclude_panel_refnos).await
            {
                // 没有几何就判不了，跳过而**不写**：写空集等于把这块面板的存量归属边
                // 抹掉。成批出现（结构库没生成）时逐块报错只会刷屏，收尾汇总一行即可。
                Ok(PanelMembers::NoGeometry) => {
                    without_geometry.push(panel_refno);
                    continue;
                }
                Ok(PanelMembers::Computed(members)) => members,
                Err(error) => {
                    failures.push(format!("{panel_refno} 计算房间成员失败: {error:#}"));
                    continue;
                }
            };
            // 成员为空也要写：先清后写里那一步 DELETE 正是「面板挪走后旧成员必须掉」。
            if members.is_empty() && previous_members.get(&panel_refno).copied().unwrap_or(0) > 0 {
                emptied.push(panel_refno);
            }
            written_edges += members.len();
            if let Err(error) = save_room_relate(panel_refno, &members, &room.room_num).await {
                failures.push(format!("{panel_refno} 写入房间归属失败: {error:#}"));
            }
        }
    }

    report_full_rebuild(
        exclude_panel_refnos.len(),
        written_edges,
        &without_geometry,
        &emptied,
    );

    if !failures.is_empty() {
        anyhow::bail!(
            "{} 块面板的房间归属未能重建: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(())
}

/// 每块面板当前收着多少个成员，一条查询查完。
async fn existing_member_counts() -> anyhow::Result<HashMap<RefnoEnum, usize>> {
    #[derive(Deserialize)]
    struct Row {
        panel: RefnoEnum,
        c: usize,
    }
    let mut response = crate::data_interface::staging::active_data_db()
        .query("SELECT in AS panel, count() AS c FROM room_relate GROUP BY panel;")
        .await
        .map_err(|error| anyhow::anyhow!("查询存量房间成员数失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("查询存量房间成员数语句失败: {error}"))?;
    Ok(response
        .take::<Vec<Row>>(0)
        .map_err(|error| anyhow::anyhow!("解析存量房间成员数失败: {error}"))?
        .into_iter()
        .map(|row| (row.panel, row.c))
        .collect())
}

/// 全量重建的收尾汇报：写了多少、几块面板没几何、几块从有成员掉到 0。
///
/// 后两项是这条路上唯二会「让房间号消失」的形态。此前它们都不出声：没几何的面板被
/// 当成 0 个成员照常写（把存量边清掉），而掉到 0 这件事从来没人统计。
fn report_full_rebuild(
    registered_panels: usize,
    written_edges: usize,
    without_geometry: &[RefnoEnum],
    emptied: &[RefnoEnum],
) {
    let sample = |refnos: &[RefnoEnum]| {
        refnos
            .iter()
            .take(5)
            .map(RefnoEnum::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!("房间归属重建完成: 写入 {written_edges} 条成员边");
    if !without_geometry.is_empty() {
        println!(
            "  {} / {} 块在册面板没有任何几何实例，已跳过（**未**清除它们的存量归属边）\
             ——例如 {}。这通常意味着结构库还没生成过",
            without_geometry.len(),
            registered_panels,
            sample(without_geometry)
        );
    }
    if !emptied.is_empty() {
        println!(
            "  {} 块面板的成员从非空变成 0（例如 {}）——若非预期，先查它们的网格与包围盒",
            emptied.len(),
            sample(emptied)
        );
    }
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
    crate::surreal_retry::execute_model_scoped_delete(
        &sql,
        &format!("写入 {panel_refno} 的房间归属"),
    )
    .await
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

    let mut response = crate::data_interface::staging::active_data_db()
        .query(sql)
        .await?
        .check()?;
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
    let mut response = crate::data_interface::staging::active_data_db()
        .query(format!(
            r#"select
                 in as refno, world_trans.d as world_trans, aabb.d as world_aabb,
                 (select value [trans.d, (->inst_geo[?pts!=none].pts[?d!=none].d) ] from ->inst_info->geo_relate) as pts_group
               from array::flatten([{pes}]->inst_relate)
               where !booled and aabb.d != none and world_trans.d != none
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

/// 一块面板的成员计算结果。
///
/// 「算出来是空集」与「压根算不了」必须在类型上分开：写入是先清后写，把后者当成前者
/// 就等于**主动**把这块面板的存量归属边抹掉，而且任务还算成功。
#[derive(Debug)]
pub enum PanelMembers {
    /// 算出来了。空集是正当结果——「这块面板里确实没有构件」，那条 DELETE 该发。
    Computed(HashMap<RefnoEnum, RoomMember>),
    /// 这块面板在库里没有任何几何实例，判不了。
    ///
    /// 与「网格读不出来」同样是判不了，区别只在它通常成批出现（结构库从未生成过——
    /// 本项目 147 块在册面板只有 12 块有几何），所以交给调用方汇总上报，而不是逐块
    /// 当成一次失败刷屏。
    NoGeometry,
}

pub async fn cal_room_refnos(
    mesh_dir: &PathBuf,
    panel_refno: RefnoEnum,
    exclude_refnos: &HashSet<RefnoEnum>,
) -> anyhow::Result<PanelMembers> {
    //查询到aabb直接完全在这个房间里的mesh里，就不用做点的检查
    // 这里曾经是 `unwrap_or_default()`：面板实例只要有一个字段形状不对
    //（`GeomInstQuery` 的 `pe.owner` / `inst_relate.generic` 都是非 Option 字符串），
    // 反序列化错误就被吞成空 Vec，紧接着下面的 `is_empty()` 让整间房**无声地**算成
    // 0 个成员，日志里一行都没有。合成夹具首跑就是被它藏了半天。
    let geom_insts: Vec<GeomInstQuery> = aios_core::query_insts(&[panel_refno], true)
        .await
        .map_err(|error| anyhow::anyhow!("查询面板 {panel_refno} 的实例失败: {error}"))?;
    // dbg!(&geom_insts);
    // 此前这里 `return Ok(Default::default())`——把「没有几何」当成「没有成员」交出去，
    // 调用方紧接着先清后写，于是这块面板的存量归属边被静默清空。它与下面「网格一个都
    // 不可用」是同一件事，处置也该一样：不写。
    if geom_insts.is_empty() {
        return Ok(PanelMembers::NoGeometry);
    }

    let mut within_refnos: HashMap<RefnoEnum, RoomMember> = HashMap::new();
    // 网格读不出来不能静默当成「没有成员」：这套写入是先清后写，判不了却照常
    // 返回结果会把整间房的存量边部分抹掉且不留痕迹。逐个实例的失败先收集，
    // 只要有一个失败就中止写入并保留任务重试。
    let mut usable_meshes = 0usize;
    let mut mesh_failures: Vec<String> = Vec::new();
    //将panel的 plant mesh 转换成TriMesh
    for geom_inst in geom_insts {
        for inst in geom_inst.insts {
            let file_path = mesh_dir.join(format!("{}.mesh", inst.geo_hash));
            let mesh = match PlantMesh::des_mesh_file(&file_path) {
                Ok(mesh) => mesh,
                Err(error) => {
                    mesh_failures.push(format!("{}: {error}", file_path.display()));
                    continue;
                }
            };
            // dbg!(&file_path);
            let Some(mut tri_mesh) = mesh.get_tri_mesh_with_flag(
                (geom_inst.world_trans * inst.transform).compute_matrix(),
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            ) else {
                mesh_failures.push(format!("{}: 三角网转换失败", file_path.display()));
                continue;
            };
            usable_meshes += 1;
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

    if !mesh_failures.is_empty() {
        anyhow::bail!(
            "面板 {panel_refno} 有 {} 个网格不可用，本次不改写归属: {}",
            mesh_failures.len(),
            mesh_failures.join("; ")
        );
    }
    if usable_meshes == 0 {
        anyhow::bail!("面板 {panel_refno} 没有可用网格，本次不改写归属");
    }

    Ok(PanelMembers::Computed(within_refnos))
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
        let old_members = existing_members_of_panel(panel)
            .await
            .unwrap_or_else(|error| {
                println!("[房间增量] 读取面板 {panel} 旧成员失败（仅日志受影响）: {error:#}");
                HashSet::new()
            });
        if !old_members.is_empty() {
            println!(
                "[房间增量] 面板 {panel} 已不在册，清空其房间归属（原 {} 个成员掉出）",
                old_members.len()
            );
        }
        save_room_relate(panel, &HashMap::new(), "").await?;
        return Ok(HashSet::new());
    };
    // 与 [`build_room_relations`] 同一道门。成员候选取自空间树，树空着时这块面板会
    // 算出 0 个成员，而写入是先清后写——一次房间改名或一次面板移动就足以把这间房的
    // 归属清空，任务还返回成功、队列行随即删除、日志一行没有。上面那条清边路径不受
    // 影响：面板已不在册是与树无关的事实，它的边本来就该清掉。
    //
    // 判不了就不写：上抛让任务保留重试，存量边陈旧也比被清成空强。
    if GLOBAL_AABB_TREE.read().await.is_empty() {
        anyhow::bail!(
            "空间树是空的，面板 {panel} 的成员判不了：不改写它的房间归属（任务保留重试）。\
             先确认 accel_tree.bin 与库的对账（sync_aabb_tree_with_db）"
        );
    }
    // 没有几何同样是「判不了」，在这里要比全量那一侧更响：整间任务的入队条件是这块
    // 面板的包围盒确实变过，能变就说明它刚才还有几何，此刻却查不到，本身就是信号。
    // 上抛让任务保留重试，而不是写一个空集把这间房清掉。
    let members = match cal_room_refnos(&db_option.get_meshes_path(), panel, &rooms.all_panels)
        .await?
    {
        PanelMembers::Computed(members) => members,
        PanelMembers::NoGeometry => anyhow::bail!(
            "面板 {panel} 查不到任何几何实例，不改写它的房间归属（任务保留重试）。\
             它的包围盒刚变过才排的这次重算，此刻却没有几何——先确认结构库是否被清过"
        ),
    };
    let new_members: HashSet<RefnoEnum> = members.keys().copied().collect();
    let old_members = existing_members_of_panel(panel)
        .await
        .unwrap_or_else(|error| {
            println!("[房间增量] 读取面板 {panel} 旧成员失败（仅日志受影响）: {error:#}");
            HashSet::new()
        });
    log_panel_membership_change(panel, &room_num, &old_members, &new_members);
    save_room_relate(panel, &members, &room_num).await?;
    Ok(new_members)
}

/// 一轮 drain 复用的在册面板几何：元素分支的候选面板从这里选，**不经过空间树**。
///
/// 为什么不用树：两条重算分支对树的依赖方向是相反的。整间分支拿面板自己的包围盒去
/// 树上捞成员，面板在不在树上都算得出来；元素分支反过来要在树上按 `noun == "PANE"`
/// 找候选，于是多出一个只打中增量的前提——树里得有在册面板。那个前提破了（空树、
/// `accel_tree.bin` 来自没生成过结构库的那一次、数量对账放行、或没走 `run_app` 那次
/// 对账）时，启动的全量重建照样写得出 `room_relate`，而每一个元素任务都会捞不到候选，
/// 把该构件的存量归属边按「不属于任何房间」清掉——静默、无日志、任务还算成功。
///
/// 在册面板只有百来块（本项目 124 间房 / 147 块），一次 `query_insts` 就能整轮复用，
/// 于是这个前提可以直接不要：候选改为在库里的面板包围盒上做相交筛选，与整间分支同
/// 一个数据源、同一个相交关系，只是反着问。代价是候选筛选从 R 树的 O(log n) 变成
/// 面板数的线性扫描，在这个量级上是噪音；换来的是元素分支与树彻底解耦。
///
/// 顺带省掉原来每个元素任务各发一次的候选面板 `query_insts`。
#[derive(Default)]
pub struct PanelIndex {
    entries: Vec<PanelEntry>,
    /// 与 `entries` 同序的世界包围盒，交集筛选用。
    boxes: Vec<Aabb>,
    /// 面板网格所在目录，随索引一起定下来——缓存好的三角网只对这个目录有效。
    mesh_dir: PathBuf,
    /// 在册、但没能进索引的面板：`query_insts` 没返回行，或世界包围盒不可用。
    missing: Vec<RefnoEnum>,
}

/// 一块在册面板的一条实例（同一个 refno 可能有多条 `inst_relate` 行）。
struct PanelEntry {
    panel: RefnoEnum,
    room_num: String,
    inst: GeomInstQuery,
    /// 本轮第一次用到这块面板时构建，之后整轮复用（见 [`PanelEntry::meshes`]）。
    mesh_cache: OnceLock<PanelMeshes>,
}

/// 一块面板在世界坐标系下的三角网，以及构建过程中判不了的部分。
///
/// 失败逐条留着而不是当场上抛：调用方要按面板汇总「哪几块判不了」，且这个结果整轮
/// 复用，同一个失败不该在每个元素任务里各报一次不同的话。
struct PanelMeshes {
    tri_meshes: Vec<TriMesh>,
    failures: Vec<String>,
}

impl PanelMeshes {
    fn build(inst: &GeomInstQuery, mesh_dir: &Path) -> Self {
        let mut tri_meshes = Vec::new();
        let mut failures = Vec::new();
        for geo in &inst.insts {
            let file_path = mesh_dir.join(format!("{}.mesh", geo.geo_hash));
            let mesh = match PlantMesh::des_mesh_file(&file_path) {
                Ok(mesh) => mesh,
                Err(error) => {
                    failures.push(format!("{}: {error}", file_path.display()));
                    continue;
                }
            };
            let Some(tri_mesh) = mesh.get_tri_mesh_with_flag(
                (inst.world_trans * geo.transform).compute_matrix(),
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            ) else {
                failures.push(format!("{}: 三角网转换失败", file_path.display()));
                continue;
            };
            tri_meshes.push(tri_mesh);
        }
        Self {
            tri_meshes,
            failures,
        }
    }
}

impl PanelEntry {
    /// 本块面板的世界坐标三角网，**一轮只构建一次**。
    ///
    /// 缓存的理由不是省 CPU 而是省重复：元素侧一页最多几百个任务，它们通常挤在同一间
    /// 房里，同一块墙板的 `.mesh` 于是被读盘并三角化上百遍。[`PanelIndex`] 此前只把面板
    /// 的**库内行**整轮复用，网格仍在每个元素任务里现做。
    ///
    /// 惰性而非加载时构建：一轮只有两个元素任务时，不该为 147 块在册面板全部读盘。
    fn meshes(&self, mesh_dir: &Path) -> &PanelMeshes {
        self.mesh_cache
            .get_or_init(|| PanelMeshes::build(&self.inst, mesh_dir))
    }
}

/// 候选筛选（纯函数）：与构件世界包围盒相交的面板块下标。
///
/// 相交口径必须与整间分支一致——那边走 rstar 的 `locate_in_envelope_intersecting`，
/// 与 parry 的 `Aabb::intersects` 同样是闭区间（贴面算相交）。
fn intersecting_panel_slots(panel_boxes: &[Aabb], element_aabb: &Aabb) -> Vec<usize> {
    panel_boxes
        .iter()
        .enumerate()
        .filter(|(_, panel)| panel.intersects(element_aabb))
        .map(|(slot, _)| slot)
        .collect()
}

impl PanelIndex {
    /// 在册且几何可用的面板块数。为 0 时元素分支算出来的必然是空集——那与全量重建
    /// 一致（那些面板在整间分支里同样一个成员都算不出来），但值得说一声。
    pub fn usable_panels(&self) -> usize {
        self.entries.len()
    }

    /// 在册却没能进索引的那些面板。
    ///
    /// 元素分支的候选只在进了索引的面板里选，所以这些面板对增量路径等于不存在——落在
    /// 它们里面的构件会被收敛成「不属于任何房间」。这与全量重建**一致**（那边现在同样
    /// 跳过它们），所以不是两条分支分歧，而是覆盖率问题：覆盖率低到什么程度算异常只有
    /// 现场知道，所以如实报出来，别替它判。
    ///
    /// 此前这件事只有「一块都没有」时才说得出口，而 147 块里只有 12 块有几何这种状态
    /// （issue #7 审核实测）一声不响。
    pub fn missing_panels(&self) -> &[RefnoEnum] {
        &self.missing
    }

    fn mesh_dir(&self) -> &Path {
        &self.mesh_dir
    }

    fn candidates(&self, element_aabb: &Aabb) -> Vec<&PanelEntry> {
        intersecting_panel_slots(&self.boxes, element_aabb)
            .into_iter()
            .map(|slot| &self.entries[slot])
            .collect()
    }

    /// 与构件世界包围盒相交的在册候选面板 refno 集合。
    ///
    /// 与 [`recalc_element_membership`] 里的 `candidates` 用的是同一份库内面板几何、
    /// 同一个相交口径。同轮吸收（ADR-010 §8）的封闭性检查靠它预测元素分支会碰哪些
    /// 面板，两者必须同源——此前吸收检查从空间树取候选，树缺在册 PANE 条目
    /// （issue #7 的典型态）时会拿到空候选、错误吸收，把元素分支本会写的边永久跳过。
    pub fn candidate_panel_refnos(&self, element_aabb: &Aabb) -> HashSet<RefnoEnum> {
        self.candidates(element_aabb)
            .into_iter()
            .map(|entry| entry.panel)
            .collect()
    }
}

/// 一次查齐在册面板的几何，**按轮调用**。
///
/// 与 [`load_room_panel_map`] 同一个理由：每个元素任务各查一遍，一轮几十个任务就会被
/// 它拖垮。
pub async fn load_panel_index(
    db_option: &DbOption,
    rooms: &RoomPanelMap,
) -> anyhow::Result<PanelIndex> {
    let mut index = PanelIndex {
        mesh_dir: db_option.get_meshes_path(),
        ..Default::default()
    };
    let registered: Vec<RefnoEnum> = rooms
        .rooms
        .iter()
        .flat_map(|room| room.panels.iter().copied())
        .unique()
        .collect();
    if registered.is_empty() {
        return Ok(index);
    }
    let insts: Vec<GeomInstQuery> =
        aios_core::query_insts(&registered, true)
            .await
            .map_err(|error| {
                anyhow::anyhow!("查询 {} 块在册面板的实例失败: {error}", registered.len())
            })?;

    let mut indexed: HashSet<RefnoEnum> = HashSet::new();
    for inst in insts {
        let Some(room_num) = rooms.room_num_of(inst.refno) else {
            continue;
        };
        if !aabb_is_usable(&inst.world_aabb) {
            continue;
        }
        indexed.insert(inst.refno);
        index.boxes.push(inst.world_aabb);
        index.entries.push(PanelEntry {
            panel: inst.refno,
            room_num: room_num.to_string(),
            inst,
            mesh_cache: OnceLock::new(),
        });
    }
    // 在册却没进索引的：查不到实例，或包围盒不可用。顺序固定，日志才对得上。
    index.missing = registered
        .into_iter()
        .filter(|panel| !indexed.contains(panel))
        .collect();
    index.missing.sort_by_key(RefnoEnum::to_string);
    Ok(index)
}

/// 一轮 drain 复用的构件现存归属快照：整页元素的 `room_relate` 入边，一条 SELECT 查完。
///
/// 两个消费者读的本来就是同一份边——元素分支的归属变化日志要旧**房间号**，同轮吸收的
/// 封闭性检查要旧**归属面板**（ADR-010 §8）。此前前者按元素各发一次查询、后者再为吸收
/// 候选发一次，同一张表在一轮里被问 N+1 遍，而这些查询与重算结果无关，纯属陪跑。
///
/// 查不到 `room_num` 的边照样记下面板：房间号只服务日志，而封闭性检查一条边都不能漏
/// ——漏掉的旧边会让「旧边 ⊆ 本轮已重算面板」凭空成立，把本该照跑的元素任务错误吸收。
#[derive(Debug, Default)]
pub struct ElementRoomHistory {
    edges: HashMap<RefnoEnum, Vec<(RefnoEnum, Option<String>)>>,
}

impl ElementRoomHistory {
    pub async fn load(elements: &[RefnoEnum]) -> anyhow::Result<Self> {
        let mut history = Self::default();
        if elements.is_empty() {
            return Ok(history);
        }
        #[derive(Deserialize)]
        struct EdgeRow {
            panel: RefnoEnum,
            element: RefnoEnum,
            #[serde(default)]
            room_num: Option<String>,
        }
        let keys = elements
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = crate::data_interface::staging::active_data_db()
            .query(format!(
                "SELECT in AS panel, out AS element, room_num \
                 FROM room_relate WHERE out IN [{keys}];"
            ))
            .await
            .map_err(|error| {
                anyhow::anyhow!("查询 {} 个构件的现存房间归属失败: {error}", elements.len())
            })?
            .check()
            .map_err(|error| anyhow::anyhow!("查询构件现存房间归属语句失败: {error}"))?;
        for row in response
            .take::<Vec<EdgeRow>>(0)
            .map_err(|error| anyhow::anyhow!("解析构件现存房间归属失败: {error}"))?
        {
            history
                .edges
                .entry(row.element)
                .or_default()
                .push((row.panel, row.room_num));
        }
        Ok(history)
    }

    /// 一个构件当前挂在哪些房间。有序集合：日志渲染押在确定性上。
    pub fn room_nums_of(&self, element: RefnoEnum) -> BTreeSet<String> {
        self.edges
            .get(&element)
            .into_iter()
            .flatten()
            .filter_map(|(_, room_num)| room_num.clone())
            .collect()
    }

    /// 一个构件的现存归属边指向哪些面板。
    pub fn panels_of(&self, element: RefnoEnum) -> HashSet<RefnoEnum> {
        self.edges
            .get(&element)
            .into_iter()
            .flatten()
            .map(|(panel, _)| *panel)
            .collect()
    }
}

/// 一批构件各自与在册面板相交的候选面板集合，候选面板取自 [`PanelIndex`]、构件包围盒
/// 取自库——与 [`recalc_element_membership`] 完全同源。
///
/// 同轮吸收（ADR-010 §8）的封闭性检查用它预测元素分支会落进哪些面板：预测与实算读的
/// 必须是同一份库内几何，否则树/库分歧时（issue #7 那类）两者会给出不同答案，导致错误
/// 吸收、静默漏写归属边。查不到实例或包围盒不可用的构件如实留空（不插入映射）——在
/// `absorption_verdict` 里「缺项」表示候选未知、一律不吸收，让元素任务照跑。
pub async fn element_candidate_panels(
    panels: &PanelIndex,
    elements: &[RefnoEnum],
) -> anyhow::Result<HashMap<RefnoEnum, HashSet<RefnoEnum>>> {
    let mut out: HashMap<RefnoEnum, HashSet<RefnoEnum>> = HashMap::new();
    if elements.is_empty() {
        return Ok(out);
    }
    let insts: Vec<GeomInstQuery> =
        aios_core::query_insts(elements, true)
            .await
            .map_err(|error| {
                anyhow::anyhow!("查询 {} 个吸收候选构件的实例失败: {error}", elements.len())
            })?;
    for inst in insts {
        // 没有可用世界包围盒的构件：元素分支会把它收敛成空集，本身也不该有候选面板。
        // 留空（不插入）而非插入空集——两者在 absorption_verdict 里含义不同：缺项＝
        // 候选未知、保守不吸收。
        if !aabb_is_usable(&inst.world_aabb) {
            continue;
        }
        out.insert(inst.refno, panels.candidate_panel_refnos(&inst.world_aabb));
    }
    Ok(out)
}

/// 元素分支算出一条边时的合并：同一块面板的多条实例只留归属更强的那次。
///
/// `inside_count` 由第一轮的顶点计数直接带过来。此前这里为排序键又调了一次
/// `count_vertices_inside`，同一块面板的八次点包含测试因此跑两遍——而判定函数
/// 本来就数过了，只是没把数字交出来。
fn merge_element_edge(
    edges: &mut HashMap<RefnoEnum, ElementRoomEdge>,
    candidate: &PanelEntry,
    element: RefnoEnum,
    element_aabb: &Aabb,
    inside_count: u8,
) {
    let member = RoomMember {
        refno: element,
        inside_count,
        center_dist: center_distance(&candidate.inst.world_aabb, element_aabb),
    };
    edges
        .entry(candidate.panel)
        .and_modify(|edge| edge.member = edge.member.stronger(member))
        .or_insert(ElementRoomEdge {
            panel: candidate.panel,
            room_num: candidate.room_num.clone(),
            member,
        });
}

/// 元素分支：一个构件动了，反向定位它落在哪些面板里（ADR-010 §2）。
pub async fn recalc_element_membership(
    rooms: &RoomPanelMap,
    panels: &PanelIndex,
    history: &ElementRoomHistory,
    element: RefnoEnum,
) -> anyhow::Result<()> {
    // 面板本身不参与归属。整间分支把**所有**面板排除在成员之外，反向路径必须同口径，
    // 否则一块面板在两条路径下会得到不同答案。PANE 自身变更按 ADR §2 走整间分支。
    if rooms.all_panels.contains(&element) {
        return Ok(());
    }

    // 归属变化日志用：这个构件此刻挂在哪些房间，收敛后与新结果对照打印「从哪到哪」。
    // 取自本轮那份整页快照（[`ElementRoomHistory`]），不再按元素各查一次。
    let old_rooms = history.room_nums_of(element);

    let insts: Vec<GeomInstQuery> = aios_core::query_insts(&[element], true)
        .await
        .map_err(|error| anyhow::anyhow!("查询构件 {element} 的实例失败: {error}"))?;
    // 没有几何、或包围盒不可用的构件不可能属于任何房间——但旧边照样要清掉：全量重建
    // 也捞不到它（进不了空间树就不是任何面板的候选），空集才是与全量一致的结果。
    // 打一行日志是因为这条路本不该走到：元素任务的入队条件是「包围盒确实变了」，能变
    // 就说明刚才还算得出来，此刻却查不到，本身就是要查的信号。
    let Some(element_inst) = insts.into_iter().next() else {
        println!("构件 {element} 查不到几何实例，房间归属按空集收敛（存量入边已清）");
        return write_element_room_relate_logged(element, &[], &old_rooms).await;
    };
    let element_aabb = element_inst.world_aabb;
    if !aabb_is_usable(&element_aabb) {
        println!("构件 {element} 的世界包围盒不可用，房间归属按空集收敛（存量入边已清）");
        return write_element_room_relate_logged(element, &[], &old_rooms).await;
    }

    // 候选面板：在册面板里包围盒与本构件相交的那些，取自本轮加载好的库内几何
    // （[`PanelIndex`]），不经过空间树。构件这一侧的包围盒同样取自库——本任务正是
    // 「这个构件的包围盒变了」才入队的，而入队点就是刷新包围盒的那一处。
    let candidates = panels.candidates(&element_aabb);
    if candidates.is_empty() {
        // 这里的空集是可信的：候选取自库，而整间分支对这批面板算成员用的是同一份
        // 数据、同一个相交关系，它同样不会把这个构件收进去。真正「判不了」的情形
        // ——候选面板的网格读不出来——在下面单独处置，不会走到这条清边路径上。
        return write_element_room_relate_logged(element, &[], &old_rooms).await;
    }

    let mesh_dir = panels.mesh_dir();
    let mut edges: HashMap<RefnoEnum, ElementRoomEdge> = HashMap::new();
    // 候选面板的网格读不出来时不能把「判不了」当「不在里面」：本函数是先删该构件
    // 全部入边再写回，静默跳过一块面板等于悄悄退掉该构件在这块面板的归属。
    // 任一网格失败或面板没有可用网格都中止写入并保留任务重试。
    let mut undecidable_panels: Vec<String> = Vec::new();
    // 第二轮逐点兜底的待办：(候选块下标, 该块内的网格下标, 第一轮的顶点计数)。
    let mut pending_point_checks: Vec<(usize, usize, u8)> = Vec::new();

    // 第一轮只问包围盒。三角网取自本轮加载的面板索引，同一块面板整轮只构建一次
    // ——此前每个元素任务都要把候选面板的 `.mesh` 重读一遍并重新三角化。
    for (slot, candidate) in candidates.iter().enumerate() {
        let meshes = candidate.meshes(mesh_dir);
        for (mesh_slot, tri_mesh) in meshes.tri_meshes.iter().enumerate() {
            // element_aabb 上面已经过 aabb_is_usable，这条走不到；真走到了当
            // 「不在里面」，与 element_in_panel 同口径。
            let Some((verdict, inside_count)) = membership_by_aabb(tri_mesh, &element_aabb) else {
                continue;
            };
            match verdict {
                AabbVerdict::Inside => {
                    merge_element_edge(&mut edges, candidate, element, &element_aabb, inside_count)
                }
                AabbVerdict::Outside => {}
                AabbVerdict::NeedsPointCheck => {
                    pending_point_checks.push((slot, mesh_slot, inside_count))
                }
            }
        }
        if !meshes.failures.is_empty() {
            undecidable_panels.push(format!(
                "{}({})",
                candidate.panel,
                meshes.failures.join("; ")
            ));
        } else if meshes.tri_meshes.is_empty() {
            undecidable_panels.push(format!("{}(没有可用网格)", candidate.panel));
        }
    }

    // 判不了就在这里收手：本次不写、任务保留重试。排在第二轮之前，是因为这一轮的结果
    // 无论如何都不会落库，没必要再为它取一次几何点。
    if !undecidable_panels.is_empty() {
        anyhow::bail!(
            "构件 {element} 的 {} 块候选面板网格不可完整判定，本次不改写归属: {}",
            undecidable_panels.len(),
            undecidable_panels.join(", ")
        );
    }

    // 只有跨界构件才需要实际几何点，而取点是一次库往返。此前无论第一轮判成什么都先
    // 取一遍，八顶点全在内或全在外的构件（绝大多数）白付一次查询。
    if !pending_point_checks.is_empty() {
        let world_pts = element_world_points(element)
            .await
            .map_err(|error| anyhow::anyhow!("构件 {element} 的几何点: {error:#}"))?;
        for (slot, mesh_slot, inside_count) in pending_point_checks {
            let candidate = candidates[slot];
            let tri_mesh = &candidate.meshes(mesh_dir).tri_meshes[mesh_slot];
            if element_in_panel(tri_mesh, &element_aabb, || world_pts.iter().copied()) {
                merge_element_edge(&mut edges, candidate, element, &element_aabb, inside_count);
            }
        }
    }

    let edges: Vec<ElementRoomEdge> = edges.into_values().collect();
    write_element_room_relate_logged(element, &edges, &old_rooms).await
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
    crate::surreal_retry::execute_model_scoped_delete(
        &sql,
        &format!("写入 {element} 的房间归属"),
    )
    .await
}

/// 元素分支写入 + 归属变化日志：先算本次收敛出的房间集合，与旧集合对照打印
/// 「从哪到哪」，再落库。日志只在真的变了时说话（见 [`log_room_membership_change`]）。
async fn write_element_room_relate_logged(
    element: RefnoEnum,
    edges: &[ElementRoomEdge],
    old_rooms: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let new_rooms: BTreeSet<String> = edges.iter().map(|edge| edge.room_num.clone()).collect();
    log_room_membership_change("构件", element, old_rooms, &new_rooms);
    write_element_room_relate(element, edges).await
}

/// 增量房间归属变化的控制台日志：只在归属真的变了时说一句，把「从哪到哪」讲清楚
/// （无房间 → R、A → B、A → 无房间）。
///
/// 房间号集合用有序集合渲染，保证同一份变化每次打印一致（`HashSet` 遍历顺序不稳，
/// 对拍与重放都押在确定性上）。
fn log_room_membership_change(
    kind: &str,
    target: RefnoEnum,
    old_rooms: &BTreeSet<String>,
    new_rooms: &BTreeSet<String>,
) {
    if old_rooms == new_rooms {
        return;
    }
    let render = |rooms: &BTreeSet<String>| {
        if rooms.is_empty() {
            "无房间".to_string()
        } else {
            rooms.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    };
    println!(
        "[房间增量] {kind} {target} 归属: {} -> {}",
        render(old_rooms),
        render(new_rooms)
    );
}

/// 整间分支的成员变化日志：哪些构件进了这间房、哪些掉了出去。
///
/// 整间重算是先清后写、按面板出边整批替换，因此进/出直接由新旧成员集合求差得到，
/// 正好覆盖「构件从无到有」（新进）与「构件移出该房」（掉出）两种可见变化。
fn log_panel_membership_change(
    panel: RefnoEnum,
    room_num: &str,
    old_members: &HashSet<RefnoEnum>,
    new_members: &HashSet<RefnoEnum>,
) {
    if old_members == new_members {
        return;
    }
    let mut entered: Vec<String> = new_members
        .difference(old_members)
        .map(|refno| refno.to_string())
        .collect();
    let mut left: Vec<String> = old_members
        .difference(new_members)
        .map(|refno| refno.to_string())
        .collect();
    entered.sort();
    left.sort();
    let detail = |label: &str, refnos: &[String]| {
        if refnos.is_empty() {
            String::new()
        } else {
            format!("；{label}: {}", refnos.join(", "))
        }
    };
    println!(
        "[房间增量] 面板 {panel} 房间 {room_num}: 成员 {} -> {}（+{} 新进 / -{} 掉出）{}{}",
        old_members.len(),
        new_members.len(),
        entered.len(),
        left.len(),
        detail("新进", &entered),
        detail("掉出", &left),
    );
}

/// 一块面板当前收着哪些构件：现存 `room_relate` 出边（`in = panel`）的 out 端去重。
/// 仅供日志对照。
async fn existing_members_of_panel(panel: RefnoEnum) -> anyhow::Result<HashSet<RefnoEnum>> {
    let mut response = crate::data_interface::staging::active_data_db()
        .query(format!(
            "SELECT VALUE out FROM room_relate WHERE in = {};",
            panel.to_pe_key()
        ))
        .await
        .map_err(|error| anyhow::anyhow!("查询面板 {panel} 现存成员失败: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("查询面板 {panel} 现存成员语句失败: {error}"))?;
    let members: Vec<RefnoEnum> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("解析面板 {panel} 现存成员失败: {error}"))?;
    Ok(members.into_iter().collect())
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
        let entries = [
            member(20, 8, 0.0),
            member(21, 8, 10.0),
            member(24, 4, 450.0),
        ];
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

    /// 候选筛选的相交口径（纯函数）：闭区间，贴面算相交。
    ///
    /// 必须与整间分支一致——那边走 rstar 的 `locate_in_envelope_intersecting`，同样是
    /// 闭区间。两边口径差一个等号，跨界构件就会在两条分支下得到不同答案，而 ADR-010
    /// §9 的唯一硬标准正是「增量 == 全量」。
    #[test]
    fn panel_candidates_are_selected_by_closed_interval_overlap() {
        let box_of = |min: f32, max: f32| {
            Aabb::new(Point::new(min, 0.0, 0.0), Point::new(max, 100.0, 100.0))
        };
        let panels = [
            box_of(0.0, 100.0),
            box_of(200.0, 300.0),
            box_of(90.0, 210.0),
        ];

        // 完全落在第一块里。
        assert_eq!(
            intersecting_panel_slots(&panels, &box_of(10.0, 20.0)),
            vec![0]
        );
        // 跨界：第一块与第三块都要收进来，多归属正是靠这个。
        assert_eq!(
            intersecting_panel_slots(&panels, &box_of(95.0, 105.0)),
            vec![0, 2]
        );
        // 贴面：闭区间，算相交。
        assert_eq!(
            intersecting_panel_slots(&panels, &box_of(100.0, 150.0)),
            vec![0, 2]
        );
        // 谁都不挨着。
        assert!(intersecting_panel_slots(&panels, &box_of(400.0, 500.0)).is_empty());
        // 一块在册面板都没有时不会凭空造出候选。
        assert!(intersecting_panel_slots(&[], &box_of(0.0, 1.0)).is_empty());
    }

    /// 全量重建在空树上必须拒跑，而不是把整库房间归属清成空。
    ///
    /// 它是先清后写的：树空着时每块面板都算出 0 个成员，一次启动就抹平整库。这与元素
    /// 分支「判不了就不改写」是同一条纪律，只是方向相反——那边缺的是面板，这边缺的是
    /// 构件。
    #[test]
    fn the_full_rebuild_refuses_to_run_against_an_empty_tree() {
        let source = include_str!("room_model.rs");
        let body = source
            .split_once("pub async fn build_room_relations(")
            .expect("build_room_relations 必须存在")
            .1
            .split_once("\n/// 渲染一块面板的房间归属写入")
            .expect("全量重建之后是 render_room_relate_write")
            .0;

        let guard_at = body
            .find("GLOBAL_AABB_TREE.read().await.is_empty()")
            .expect("空树必须挡在重建之前");
        let bail_at = body[guard_at..]
            .find("anyhow::bail!")
            .map(|at| guard_at + at)
            .expect("空树必须上抛，交给调用点降级为告警");
        let recalc_at = body
            .find("cal_room_refnos(")
            .expect("重建必须调 cal_room_refnos");
        let write_at = body
            .find("save_room_relate(")
            .expect("重建必须写 room_relate");

        assert!(
            bail_at < recalc_at && bail_at < write_at,
            "空树判定必须排在任何一次重算与写入之前: {body}"
        );
    }

    /// 增量整间分支在空树上同样必须拒跑。
    ///
    /// 全量重建那道门只挡住了启动那一次。增量整间分支走的是同一个 `cal_room_refnos`、
    /// 同一套先清后写，缺门时一条 `RoomRecalcPanel` 任务就能把那间房的归属清空——而
    /// 那种任务是 D12 房间改名 / 面板搬迁与 PANE 自身移动的常规产物，不是罕见路径。
    /// 元素分支已经把树依赖整个拆掉，这条是剩下的那一半。
    ///
    /// 「面板已不在册」那条清边路径**不**受这道门管：它与树无关，面板不在册就是不在
    /// 册，边本来就该清掉。
    #[test]
    fn the_panel_branch_refuses_to_recalc_against_an_empty_tree() {
        let source = include_str!("room_model.rs");
        let body = source
            .split_once("pub async fn recalc_panel_membership(")
            .expect("recalc_panel_membership 必须存在")
            .1
            .split_once("/// 一轮 drain 复用的在册面板几何")
            .expect("整间分支之后是 PanelIndex")
            .0;

        let unregistered_at = body
            .find("save_room_relate(panel, &HashMap::new(), \"\")")
            .expect("面板不在册时仍要清边");
        let guard_at = body
            .find("GLOBAL_AABB_TREE.read().await.is_empty()")
            .expect("空树必须挡在重算之前");
        let recalc_at = body.find("cal_room_refnos(").expect("整间分支必须算成员");
        let write_at = body
            .find("save_room_relate(panel, &members")
            .expect("整间分支必须写回成员");

        assert!(
            unregistered_at < guard_at,
            "不在册的清边与树无关，不该被这道门挡住: {body}"
        );
        assert!(
            guard_at < recalc_at && guard_at < write_at,
            "空树判定必须排在重算与写入之前: {body}"
        );
        let bail_at = body[guard_at..]
            .find("anyhow::bail!")
            .map(|at| guard_at + at)
            .expect("空树必须上抛，让任务保留重试而不是算成功");
        assert!(bail_at < recalc_at, "{body}");
    }

    /// 「没有几何」不得被折成「没有成员」写下去。
    ///
    /// 两条路都是先清后写：把 `NoGeometry` 当空集交出去，就等于**主动**把这块面板的
    /// 存量归属边抹掉，而任务还算成功、日志一行没有。全量那边跳过并在收尾汇总上报
    /// （结构库没生成时这是成批的，逐块报错只会刷屏）；增量整间分支上抛保留重试
    /// （它的入队条件是这块面板的包围盒刚变过，此刻却没有几何，本身就是信号）。
    #[test]
    fn a_panel_without_geometry_is_never_written_as_an_empty_member_set() {
        let source = include_str!("room_model.rs");

        let full = source
            .split_once("pub async fn build_room_relations(")
            .expect("build_room_relations 必须存在")
            .1
            .split_once("\n/// 每块面板当前收着多少个成员")
            .expect("全量重建之后是 existing_member_counts")
            .0;
        let no_geometry_at = full
            .find("PanelMembers::NoGeometry)")
            .expect("全量重建必须显式处理没有几何的面板");
        let write_at = full
            .find("save_room_relate(")
            .expect("重建必须写 room_relate");
        assert!(no_geometry_at < write_at, "{full}");
        assert!(
            full[no_geometry_at..write_at].contains("continue"),
            "没有几何的面板必须跳过，而不是写一个空集把它的存量边清掉: {full}"
        );

        let panel_branch = source
            .split_once("pub async fn recalc_panel_membership(")
            .expect("recalc_panel_membership 必须存在")
            .1
            .split_once("/// 一轮 drain 复用的在册面板几何")
            .expect("整间分支之后是 PanelIndex")
            .0;
        let arm_at = panel_branch
            .find("PanelMembers::NoGeometry =>")
            .expect("整间分支必须显式处理没有几何的面板");
        let bail_at = panel_branch[arm_at..]
            .find("anyhow::bail!")
            .map(|at| arm_at + at)
            .expect("整间分支必须上抛，让任务保留重试而不是算成功");
        let write_at = panel_branch
            .find("save_room_relate(panel, &members")
            .expect("整间分支必须写回成员");
        assert!(bail_at < write_at, "{panel_branch}");
    }

    /// 元素分支的函数体，供下面几条源码断言共用。
    fn element_branch_source() -> &'static str {
        let source = include_str!("room_model.rs");
        source
            .split_once("pub async fn recalc_element_membership(")
            .expect("recalc_element_membership 必须存在")
            .1
            .split_once("\n/// 构件在世界坐标系下的实际几何点")
            .expect("元素分支之后是 element_world_points")
            .0
    }

    /// 元素分支不许再碰空间树。
    ///
    /// 整间分支拿面板包围盒去树上捞成员，面板在不在树上都算得出来；元素分支一旦反过来
    /// 依赖「树里有 PANE 条目」，就多出一个只打中增量的前提，破了会静默清掉存量归属边
    /// （issue #7 的主嫌）。候选改从库里的面板包围盒选之后，这个前提不该再长回来。
    #[test]
    fn the_element_branch_does_not_depend_on_the_spatial_tree() {
        let body = element_branch_source();

        assert!(
            !body.contains("GLOBAL_AABB_TREE") && !body.contains("load_aabb_tree"),
            "元素分支的候选必须来自 PanelIndex，不能回到空间树: {body}"
        );
        assert!(
            body.contains("panels.candidates(&element_aabb)"),
            "候选必须走本轮加载的在册面板索引: {body}"
        );
    }

    /// 候选面板的三角网必须来自本轮加载的 [`PanelIndex`]，元素分支不得自己读盘。
    ///
    /// 元素侧一页最多 256 个任务，而它们通常挤在同一间房里。分支里现做网格的话，同一块
    /// 墙板的 `.mesh` 会被反序列化并三角化上百遍——这是一轮房间收敛里最大的一笔开销，
    /// 且它不产生任何新信息。整轮缓存一旦被后来的改动挪回分支内部，不会报错也不会算错，
    /// 只会让房间轮悄悄慢一个数量级。
    #[test]
    fn the_element_branch_builds_no_meshes_of_its_own() {
        let body = element_branch_source();

        assert!(
            !body.contains("des_mesh_file") && !body.contains("get_tri_mesh_with_flag"),
            "候选面板的三角网必须走 PanelIndex 的整轮缓存: {body}"
        );
        assert!(
            body.contains("candidate.meshes(mesh_dir)"),
            "网格必须从候选块上取: {body}"
        );
    }

    /// 实际几何点只在第一轮判不出结论时才去取。
    ///
    /// 取点是一次库往返，而八顶点全在内或全在外的构件——绝大多数——根本用不上它。
    /// 这里断言的是**次序**：第一轮必须先跑完，取点必须落在「有待办」那个分支里，
    /// 而不是回到候选循环之前无条件先取一遍。
    #[test]
    fn the_second_round_points_are_fetched_only_when_something_straddles() {
        let body = element_branch_source();

        let first_round_at = body
            .find("membership_by_aabb(tri_mesh, &element_aabb)")
            .expect("第一轮必须只问包围盒");
        let guard_at = body
            .find("if !pending_point_checks.is_empty()")
            .expect("取点必须被待办列表守着");
        let fetch_at = body
            .find("element_world_points(element)")
            .expect("第二轮必须取实际几何点");

        assert!(first_round_at < guard_at, "第一轮必须先跑完: {body}");
        assert!(guard_at < fetch_at, "取点必须排在守卫之后: {body}");
    }

    /// 快照缺项读成空集。
    ///
    /// 元素分支据此把旧房间显示成「无房间」，只影响日志；吸收那一侧另有一道门——
    /// `drain_rooms` 在整页快照加载失败时一个都不吸收，因为空集会让「旧边 ⊆ 本轮已重算
    /// 面板」凭空成立。
    #[test]
    fn a_missing_history_entry_reads_as_no_rooms_and_no_panels() {
        let history = ElementRoomHistory::default();
        let element = RefnoEnum::from("4000000001_20");

        assert!(history.room_nums_of(element).is_empty());
        assert!(history.panels_of(element).is_empty());
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
