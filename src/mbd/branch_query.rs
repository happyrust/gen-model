//! e3d-model `Route` → `plant_mbd::BranchQueryResult`：PML `isobran.getbranmems`（L236）的等价物。
//!
//! 逐成员产出 `{noun, arrive_axis(adir), leave_axis(ldir), start(apos), end(lpos), pos}`：
//!
//! - **站点成员**（`Route::members`，`walk_route` 判为串接站点的 TUBI/FTUB/ELBO/BEND/VALV… 与
//!   目录 `GTYP=ANCI` 的 ATTA）：到达 / 离开 P 点经 [`resolve_end`] + [`CataloguePoints`] 解出，
//!   走 `SPRE → SPCO.CATR → SCOM.PTRE → PTAX/PTCA`，世界系 mm。成员 k 的到达端是
//!   `tubes[k].to`、离开端是 `tubes[k+1].from`（`tubes.len() == members.len() + 1`）。
//! - **穿过件**（`Route::non_route_members`，目录 `GTYP≠ANCI` 的 ATTA、WELD 等）：PML
//!   `getisolines` L1512-1515 对它们 `skip`、不看方向，这里只给 `pos` + `noun`，**留在序列里**
//!   ——切段的成员序戳出一个洞比缺一条尺寸糟得多（rs-mbd 2026-08-20 教训）。
//! - **端口方向朝外**：`arrive_axis` 指向来流一侧、`leave_axis` 指向去流一侧，相邻成员满足
//!   `next.arrive_axis == -prev.leave_axis`。这与 e3d-model `slot_alignment` 的
//!   `ArriveCheck::Member`（拿 `−to.dir` 与槽位向量比）是同一口径。
//! - `pos` = 构件世界原点（PML `pos of $!mem wrt/*`）：`world_matrix` 的平移列。
//! - `outside_diameter`（PML `aod of $!mem`）：到达这一站的那条槽位按 `CGETOD` 取管子目录件
//!   `PARA[2]`（e3d-model `departure_tube_outer_diameter`），取不到时按 PML `isoline.getod` 的回落
//!   `min(bore × 1.1, bore + 10)` 用到达侧通径折算。Phase 4 起尺寸行位与字高都从它派生。
//! - `ppoints` = P1 / P2 / P3（`CataloguePoints` 逐号解，世界系）：PML `getIsoUsedDirs` 判手柄、
//!   `considerTee` 补点要用；`arrive_ppoint` / `leave_ppoint` 是 `walk_route` 给的到达 / 离开口编号。
//! - **隐式管身**：E3D 里两个管件之间的直管不是成员（只有 FTUB 这种定长管才是），成员表收不到，
//!   而管道尺寸标注量的主要就是这些管长（PML `getcuttubi` / 线性尺寸）。这里按 E3D 画隐式管身的
//!   同一套规则（e3d-model `walk_route` 的槽位：上一站离开点 → 本站到达点，含 Head→首站、末站→Tail；
//!   长度 ≥ [`ZERO_LENGTH_TOL_MM`]；两端共轴 [`slot_alignment`]）合成 `TUBI` 成员（refno `A~B`），
//!   插在前一站之后。E3D 不画的槽位（反向 / 偏轴）这里也不合成，只记一条 Suppress issue
//!   `adapter.implied_tube_not_drawn`——那种断点（`24383_100018 → 100019` 差 50 mm 拐 85°）
//!   该在诊断里出声，不该长成一根假管。求解器（plant-mbd）不猜空隙，见其 `branch_to_split_members` 文档。
//!
//! 失败分两档（plant-mbd ADR-0002）：单个成员端口解不出 / 缺方向 → 记 `Suppress` issue、成员照常
//! 下发；整条 BRAN 没有任何带几何的成员、或连近似 Head 都建不起来 → [`BranchQueryError::RootError`]
//! （宿主映射 422）。refno 不存在 → `NotFound`（404）；不是派生隐式管身的路由容器 →
//! `NotRouteContainer`（422）；是风管那一支 → `NotPipeRoute`（422）。

use std::collections::HashMap;
use std::sync::Arc;

use e3d_io::db_element::{DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::CataloguePoints;
use e3d_model::category::{RouteDiscipline, derives_implied_tubes, route_discipline};
use e3d_model::route::{
    ArriveCheck, PPointResolver, RouteEnd, RoutePoint, SlotAlignment, ZERO_LENGTH_TOL_MM,
    departure_tube_outer_diameter, resolve_end, slot_alignment, tube_length, tube_outer_diameter,
    walk_route,
};
use e3d_model::transform::world_matrix;
use glam::{DMat4, DVec3};
use plant_mbd::{
    BranchMember, BranchQueryResult, ConnectionRef, MbdV2Issue, PPoint, root_error_issue,
    suppress_issue,
};

/// 前端 `mbd_refno` 与 plant-mbd fixtures 的 refno 写法：`w0_w1`。
pub fn wire_refno(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1 as i32)
}

#[derive(Debug)]
pub enum BranchQueryError {
    SourceChanged { dbnum: u32 },
    /// refno 在所钉会话里不存在。
    NotFound {
        refno: RefNo,
    },
    /// 元素存在，但不是派生隐式管身的路由容器（`derives_implied_tubes` 为假，如 EQUI）。
    NotRouteContainer {
        refno: RefNo,
        noun: String,
    },
    /// 是派生隐式管身的路由容器，但属主是 HVAC（`route_discipline` 为 `Hvac`）。
    ///
    /// 2026-09-18 之前这一档落在 `NotRouteContainer` 里——那时派生门自己把风管关在外面。
    /// e3d-model `f93fd66` 让风管 BRAN 也派生隐式直段（断面是矩形 / 圆两制）之后门开了，
    /// 而 MBD 是管道等轴测的量具：外径会按 `tube_outer_diameter` 回落成 `PBOR = 宽 + 高`，
    /// 答出来的尺寸标注没有意义。学科在这一层挡，不动派生门（`ledger-seg15-frameall.md` §13.5）。
    NotPipeRoute {
        refno: RefNo,
        noun: String,
    },
    /// 根错误：整条 BRAN 无法建立可信的成员结点视图；`issues` 带全部已收集的定位信息。
    RootError {
        refno: RefNo,
        issues: Vec<MbdV2Issue>,
    },
    /// 求解器拒绝（`layout_branch` 的 `Err`，目前只有「查不到分支」一种，理论上不会到这里）。
    Solver {
        refno: RefNo,
        message: String,
    },
    Db(DbElementError),
}

impl std::fmt::Display for BranchQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceChanged { dbnum } => write!(f, "库 {dbnum} 在管道读取期间变化，请重新读取"),
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::NotRouteContainer { refno, noun } => write!(
                f,
                "refno {refno}（{noun}）不是派生隐式管身的路由容器，没有管道尺寸标注"
            ),
            Self::NotPipeRoute { refno, noun } => write!(
                f,
                "refno {refno}（{noun}）的属主是 HVAC，风管不出管道尺寸标注"
            ),
            Self::RootError { refno, issues } => write!(
                f,
                "BRAN {refno} 建不起可信的成员结点视图：{}",
                issues
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            ),
            Self::Solver { refno, message } => write!(f, "BRAN {refno} 求解器拒绝：{message}"),
            Self::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BranchQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbElementError> for BranchQueryError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// 一条 BRAN 的成员结点视图。
pub fn branch_query(set: &Arc<DbSet>, refno: RefNo) -> Result<BranchQueryResult, BranchQueryError> {
    branch_query_with_diameters(set, refno).map(|query| query.result)
}

/// Host-side evidence is kept outside the frozen plant-mbd solver contract.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DiameterEvidence {
    pub source: &'static str,
    pub catalogue_od_mm: Option<f64>,
    pub arrive_bore_mm: Option<f64>,
}

pub struct BranchQueryWithDiameters {
    pub result: BranchQueryResult,
    pub diameters: HashMap<String, DiameterEvidence>,
    pub source_version: BranchReadVersion,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DatabaseReadVersion {
    pub dbnum: u32,
    pub sesno: u32,
    pub db_type: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BranchReadVersion {
    pub dbnum: u32,
    pub sesno: u32,
    /// All files actually opened by this read, including catalogue/owner dependencies.
    pub databases: Vec<DatabaseReadVersion>,
}

pub fn branch_query_with_diameters(set: &Arc<DbSet>, refno: RefNo) -> Result<BranchQueryWithDiameters, BranchQueryError> {
    let container = set.element(refno);
    if !container.exists()? {
        return Err(BranchQueryError::NotFound { refno });
    }
    let container_noun = container.element_type()?;
    if !derives_implied_tubes(&container)? {
        return Err(BranchQueryError::NotRouteContainer {
            refno,
            noun: container_noun,
        });
    }
    if route_discipline(&container)? == RouteDiscipline::Hvac {
        return Err(BranchQueryError::NotPipeRoute {
            refno,
            noun: container_noun,
        });
    }

    let branch_refno = wire_refno(refno);
    let route = walk_route(&container)?;
    let resolver = CataloguePoints;
    let mut issues: Vec<MbdV2Issue> = Vec::new();
    let mut diameters = HashMap::new();
    if route.pass_through_unresolved > 0 {
        // 模板里没有 SPKBRK（老模板）的 ATTA 被按穿过件处理。E3D 按 ATTA 自己的 SPKBRK 定它断不断管身：
        // MDS 梯架支撑的 DATUM / SREF 那种置真的本该是站点、两侧断管——读不到这个属性时
        // 它们会静默降级成穿过件，管身少几段。出声，别让人去怪切段。
        issues.push(suppress_issue(
            &branch_refno,
            &branch_refno,
            "adapter.atta_catalogue_unresolved",
            &format!(
                "{} ATTA member(s) have no SPKBRK in their template and were treated as pass-through; \
                 tube-breaking supports among them would be stations",
                route.pass_through_unresolved
            ),
        ));
    }
    let mut members: Vec<BranchMember> =
        Vec::with_capacity(route.members.len() + route.non_route_members.len());

    // 站点成员在链上的下标：成员 k 的到达端 = tubes[k].to，离开端 = tubes[k+1].from。
    let station_index: HashMap<(u32, u32), usize> = route
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (refno_key(member.refno()), index))
        .collect();

    // Head：BRAN 真实的 HPOS/HDIR（PML L1493-1494）。`container_head` 会把缺 HDIR 补成 +Z，
    // 这里分开读，缺了就如实缺、走兜底并留痕。
    let head_pos = container.get_position("HPOS")?;
    let head_dir = container.get_direction("HDIR")?;
    let tail_pos = container.get_position("TPOS")?;
    let tail_dir = container.get_direction("TDIR")?;

    // 这四个读数是**属主相对**的，而构件端走 `CataloguePoints` / `world_matrix(&member)` 给的是
    // 世界系。两端不折到同一个系里，属主链非单位阵的支管上头 / 尾段整条都落在系与系之间
    // （e3d-model `route::container_point` 是同一处修复）。折不出来时如实记一笔，不拿单位阵冒充。
    let container_world = match world_matrix(&container) {
        Ok(matrix) => matrix,
        Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            issues.push(suppress_issue(
                &branch_refno,
                &branch_refno,
                "adapter.container_frame_unresolved",
                "the container's owner chain could not be folded into world; head / tail points are \
                 reported in the owner frame",
            ));
            DMat4::IDENTITY
        }
        Err(error) => return Err(error.into()),
    };

    // 隐式管身的起点：上一站的离开点（Head 从 HPOS 起步；HDIR 缺时只判长度不判共轴）。
    let mut last_leave: Option<(String, RoutePoint, bool)> = head_pos.map(|pos| {
        (
            "Head".to_string(),
            RoutePoint {
                pos: container_world.transform_point3(DVec3::from_array(pos)),
                dir: head_dir
                    .map(|d| container_world.transform_vector3(DVec3::from_array(d)))
                    .unwrap_or(DVec3::Z),
                bore: container.get_double("HBOR").ok().flatten(),
            },
            head_dir.is_some(),
        )
    });

    // 文件原序（PML `coll all bran mem`），站点与穿过件混排；隐式管身插在前一站之后。
    // order = 输出序列下标（含合成管身）。
    for member in container.members()? {
        let object_refno = wire_refno(member.refno());
        let noun = member.element_type()?;
        let station = station_index.get(&refno_key(member.refno())).copied();

        let (arrive, leave) = match station {
            Some(k) => (
                resolve_port(
                    &route.tubes[k].to,
                    &resolver,
                    &branch_refno,
                    &object_refno,
                    "arrive",
                    &mut issues,
                )?,
                resolve_port(
                    &route.tubes[k + 1].from,
                    &resolver,
                    &branch_refno,
                    &object_refno,
                    "leave",
                    &mut issues,
                )?,
            ),
            None => (None, None),
        };

        // CGETOD：到达这一站的槽位（Head→首站也算）离开侧选中的管子目录件外径。
        let slot_od = match station {
            Some(k) => catalogue_od(&route.tubes[k].from)?,
            None => None,
        };

        if let (Some(_), Some(arrive_point), Some((from_refno, from, from_dir_known))) =
            (station, arrive, last_leave.as_ref())
        {
            push_implied_tube(
                &mut members,
                &mut diameters,
                &mut issues,
                &branch_refno,
                from_refno,
                from,
                *from_dir_known,
                &object_refno,
                &arrive_point,
                ArriveCheck::Member,
                slot_od,
            );
        }

        let pos = match world_matrix(&member) {
            Ok(matrix) => Some(matrix.w_axis.truncate()),
            Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => None,
            Err(error) => return Err(error.into()),
        };
        // P1 / P2 / P3：isoDim 的手柄判定与 TEE 补点要它们；解不出的号就不填。
        let ppoints = catalogue_ppoints(&member, &resolver)?;
        // 焊缝：DTXR 含 SHOP 为车间焊（isobran.getwelds L4211-4216）；编号是项目 UDA :ZD_WLDN（L4665），
        // 字典里没有这个 UDA 就当没有。
        let (dtxr, weld_number) = if noun == "WELD" {
            (
                optional_text(&member, "DTXR")?,
                optional_text(&member, ":ZD_WLDN")?,
            )
        } else {
            (None, None)
        };
        // 位号标签（isoTag / isobran.gettags，plant-mbd ADR-0009）要的文本：库里存的 NAME 原文；ATTA 的 STEX
        // （SITE 名含 INST 的是仪表支架，PML 走支撑 :ZD_BZBM 那条路，未迁、不给）；TEE / OLET 支管口 CREF 连到的对象。
        let name = member.stored_name()?;
        let stext = if noun == "ATTA" && !ancestor_name_contains(&member, "SITE", "INST")? {
            optional_text(&member, "STEX")?
        } else {
            None
        };
        let connection = if matches!(noun.as_str(), "TEE" | "OLET") {
            connection_ref(member.get_element("CREF")?)?
        } else {
            None
        };
        let (arrive_ppoint, leave_ppoint) = match station {
            Some(k) => (
                route_end_ppoint(&route.tubes[k].to),
                route_end_ppoint(&route.tubes[k + 1].from),
            ),
            None => (None, None),
        };

        let start = arrive.map(|point| point.pos).or(pos);
        let end = leave.map(|point| point.pos).or(pos);
        let (Some(start), Some(end)) = (start, end) else {
            // 连世界原点都算不出：这个成员没有任何坐标可用，跳过并留痕。
            issues.push(suppress_issue(
                &branch_refno,
                &object_refno,
                "adapter.member_geometry_missing",
                &format!("{noun} has neither resolvable ports nor a world origin; skipped"),
            ));
            continue;
        };
        if station.is_some() && (arrive.is_none() || leave.is_none()) {
            issues.push(suppress_issue(
                &branch_refno,
                &object_refno,
                "adapter.member_directions_missing",
                &format!("{noun} port direction is missing; member kept without a direction"),
            ));
        }

        diameters.insert(object_refno.clone(), diameter_evidence(slot_od, arrive.and_then(|point| point.bore)));
        members.push(BranchMember {
            refno: object_refno.clone(),
            owner_refno: branch_refno.clone(),
            start: start.to_array(),
            end: end.to_array(),
            noun: Some(noun),
            arrive_axis: arrive.map(|point| point.dir.to_array()),
            leave_axis: leave.map(|point| point.dir.to_array()),
            pos: pos.map(|origin: DVec3| origin.to_array()),
            arrive_refno: None,
            order: members.len() as u32,
            outside_diameter: outside_diameter(slot_od, arrive.and_then(|point| point.bore)),
            arrive_noun: None,
            ppoints,
            arrive_ppoint,
            leave_ppoint,
            dtxr,
            weld_number,
            name,
            stext,
            connection,
        });

        if station.is_some() {
            // 站点：下一段隐式管身从它的离开点起；离开点解不出就断掉这一段（不猜）。
            last_leave = leave.map(|point| (object_refno.clone(), point, true));
        }
    }

    // 末站→Tail 的尾管（TPOS；TDIR 缺时只判离开侧，同 e3d-model `build_route_tubes`）。
    if let (Some(tail), Some((from_refno, from, from_dir_known))) = (tail_pos, last_leave.as_ref())
    {
        let tail_point = RoutePoint {
            pos: container_world.transform_point3(DVec3::from_array(tail)),
            dir: tail_dir
                .map(|d| container_world.transform_vector3(DVec3::from_array(d)))
                .unwrap_or(DVec3::Z),
            bore: container.get_double("TBOR").ok().flatten(),
        };
        let arrive_check = if tail_dir.is_some() {
            ArriveCheck::Member
        } else {
            ArriveCheck::Skip
        };
        let tail_slot_od = route
            .tubes
            .last()
            .map(|slot| catalogue_od(&slot.from))
            .transpose()?
            .flatten();
        push_implied_tube(
            &mut members,
            &mut diameters,
            &mut issues,
            &branch_refno,
            from_refno,
            from,
            *from_dir_known,
            "Tail",
            &tail_point,
            arrive_check,
            tail_slot_od,
        );
    }

    let (head_pos, head_dir) = match (head_pos, head_dir) {
        (Some(pos), Some(dir)) => (Some(pos), Some(dir)),
        _ => {
            // 兜底：首个带到达方向的成员——它的 apos 就是 Head 侧管身的终点，
            // Head 的离开方向（顺流）是它到达方向（朝外、逆流）的反向。
            let fallback = members
                .iter()
                .find_map(|member| member.arrive_axis.map(|axis| (member.start, axis)));
            match fallback {
                Some((start, axis)) => {
                    issues.push(suppress_issue(
                        &branch_refno,
                        &branch_refno,
                        "adapter.head_attrs_missing",
                        "BRAN has no HPOS/HDIR; Head falls back to the first member's arrive port",
                    ));
                    (Some(start), Some([-axis[0], -axis[1], -axis[2]]))
                }
                None => (None, None),
            }
        }
    };

    let mut root_errors = Vec::new();
    if head_pos.is_none() {
        root_errors.push(root_error_issue(
            &branch_refno,
            &branch_refno,
            "adapter.head_port_missing",
            "branch has neither HPOS/HDIR nor a member arrive port to seed Head",
        ));
    }
    if members.is_empty() {
        root_errors.push(root_error_issue(
            &branch_refno,
            &branch_refno,
            "adapter.no_members_with_geometry",
            "every branch member was skipped for missing geometry",
        ));
    }
    if !root_errors.is_empty() {
        root_errors.extend(issues);
        return Err(BranchQueryError::RootError {
            refno,
            issues: root_errors,
        });
    }

    // Tail：PML 对 Tail 的位置一律取 `tpos of bran`（L1812/L1901/L4255）；切段不读它（L1504 注释掉了），
    // 只影响链尺寸终点。缺了不兜底——`branch_to_split_members` 会退回最后一个成员的 lpos。
    // `volume $!bran` 的中心不在这里算：那要建整条 BRAN 的几何。求解器按成员坐标的包围盒中心近似，
    // 它只决定 E/N/W/S 的排序（`CalculateDimChardirs`）。
    // 管道名称标签（isobran.getbranmlabel L5375-5383）：NAME 第一段 + 口径 UDA :ZD_GDWJ；
    // Head / Tail 的连接标签（isoTag L18-77）：HREF / TREF 连到的对象。
    let name = container.stored_name()?;
    let label_bore_text = optional_text(&container, ":ZD_GDWJ")?;
    let head_connection = connection_ref(container.get_element("HREF")?)?;
    let tail_connection = connection_ref(container.get_element("TREF")?)?;

    let mut databases = Vec::new();
    for (dbnum, source) in set.opened_sources()? {
        if !source.is_current().unwrap_or(false) {
            return Err(BranchQueryError::SourceChanged { dbnum });
        }
        databases.push(DatabaseReadVersion { dbnum, sesno: source.sesno,
            db_type: set.db_type(dbnum).unwrap_or_default() });
    }
    databases.sort_by_key(|database| database.dbnum);
    let sesno = set.pinned_sesno(refno.dbno()).ok_or(BranchQueryError::SourceChanged { dbnum: refno.dbno() })?;
    let source_version = BranchReadVersion { dbnum: refno.dbno(), sesno, databases };
    Ok(BranchQueryWithDiameters { result: BranchQueryResult {
        branch_refno,
        members,
        head_pos,
        head_dir,
        tail_pos,
        tail_dir,
        volume_center: None,
        name,
        label_bore_text,
        head_connection,
        tail_connection,
        split_members: None,
        issues,
    }, diameters, source_version })
}

/// 引用目标在别的库、库没注册 / 元素不在所钉会话：按「没有」，不算错（同 [`catalogue_od`]）。
fn soft<T>(result: Result<T, DbElementError>) -> Result<Option<T>, BranchQueryError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// PML `name of`：有存名给原文（`/…`），没有给 refno 写法 `=w0/w1`。
fn pml_name(element: &e3d_io::db_element::DbElement) -> Result<Option<String>, BranchQueryError> {
    Ok(soft(element.stored_name())?.map(|stored| {
        stored.unwrap_or_else(|| {
            let refno = element.refno();
            format!("={}/{}", refno.word0, refno.word1 as i32)
        })
    }))
}

/// owner 链上第一个 `noun` 祖先（`zone of` / `site of`）的存名里含不含 `needle`。走不到 / 没名字按不含。
fn ancestor_name_contains(
    element: &e3d_io::db_element::DbElement,
    noun: &str,
    needle: &str,
) -> Result<bool, BranchQueryError> {
    let mut cursor = soft(element.owner())?.flatten();
    for _ in 0..32 {
        let Some(current) = cursor else {
            return Ok(false);
        };
        let Some(current_noun) = soft(current.element_type())? else {
            return Ok(false);
        };
        if current_noun == noun {
            return Ok(soft(current.stored_name())?
                .flatten()
                .is_some_and(|name| name.to_ascii_uppercase().contains(needle)));
        }
        cursor = soft(current.owner())?.flatten();
    }
    Ok(false)
}

/// PML `isoTag` L37-68 / L107-124：连接对象（HREF / TREF / CREF 的目标）的显示名与备注。
///
/// NOZZ 用喷嘴自己的名字；BRAN 用 BRAN 名；其它构件用属主 BRAN 的名字；备注是那条 BRAN 的 `:ZD_NBBM`。
/// 目标落在 `*ITUB*` / `*INST*` 区域（仪表管，L50-57 / L102-105）按没有连接。
fn connection_ref(
    target: Option<e3d_io::db_element::DbElement>,
) -> Result<Option<ConnectionRef>, BranchQueryError> {
    let Some(target) = target else {
        return Ok(None);
    };
    let Some(noun) = soft(target.element_type())? else {
        return Ok(None);
    };
    if noun == "NOZZ" {
        return Ok(pml_name(&target)?.map(|name| ConnectionRef { name, note: None }));
    }
    let bran = if noun == "BRAN" {
        Some(target)
    } else {
        soft(target.owner())?.flatten()
    };
    let Some(bran) = bran else {
        return Ok(None);
    };
    if ancestor_name_contains(&bran, "ZONE", "ITUB")?
        || ancestor_name_contains(&bran, "ZONE", "INST")?
    {
        return Ok(None);
    }
    let Some(name) = pml_name(&bran)? else {
        return Ok(None);
    };
    let note = match bran.get_string(":ZD_NBBM") {
        Ok(value) => value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        Err(
            DbElementError::UnknownAttribute { .. }
            | DbElementError::DbNotRegistered { .. }
            | DbElementError::NotFound { .. },
        ) => None,
        Err(error) => return Err(error.into()),
    };
    Ok(Some(ConnectionRef { name, note }))
}

fn refno_key(refno: RefNo) -> (u32, u32) {
    (refno.word0, refno.word1)
}

/// `RouteEnd` 上的 P 点编号（`LEAV` / `ARRI`）；容器头尾没有。
fn route_end_ppoint(end: &RouteEnd) -> Option<u32> {
    match end {
        RouteEnd::MemberLeave { ppoint, .. } | RouteEnd::MemberArrive { ppoint, .. } => *ppoint,
        RouteEnd::ContainerHead(_) | RouteEnd::ContainerTail(_) => None,
    }
}

/// 槽位离开侧的管子目录件外径（`CGETOD` 的 A 段）。目录库没开 / 目标不在所钉会话都算「取不到」。
fn catalogue_od(from: &RouteEnd) -> Result<Option<f64>, BranchQueryError> {
    match departure_tube_outer_diameter(from) {
        Ok(od) => Ok(od),
        Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// PML `aod`：目录外径优先；只有通径时按 `isoline.getod` 的回落 `min(bore × 1.1, bore + 10)` 折算成外径。
fn outside_diameter(catalogue_od: Option<f64>, arrive_bore: Option<f64>) -> Option<f64> {
    match tube_outer_diameter(catalogue_od, None, None) {
        Some(od) => Some(od),
        None => arrive_bore
            .filter(|bore| bore.is_finite() && *bore > 0.0)
            .map(|bore| (bore * 1.1).min(bore + 10.0)),
    }
}

fn diameter_evidence(catalogue_od: Option<f64>, arrive_bore: Option<f64>) -> DiameterEvidence {
    let catalogue_od_mm = tube_outer_diameter(catalogue_od, None, None);
    let arrive_bore_mm = arrive_bore.filter(|value| value.is_finite() && *value > 0.0);
    DiameterEvidence {
        source: if catalogue_od_mm.is_some() { "catalogue" } else if arrive_bore_mm.is_some() { "bore-estimate" } else { "missing" },
        catalogue_od_mm,
        arrive_bore_mm,
    }
}

/// P1 / P2 / P3（世界系）。目录链走不到 / 点不存在 / 方向解不出都不填，不算错。
fn catalogue_ppoints(
    member: &e3d_io::db_element::DbElement,
    resolver: &CataloguePoints,
) -> Result<Vec<PPoint>, BranchQueryError> {
    let mut points = Vec::with_capacity(3);
    for number in 1..=3 {
        match resolver.leave(member, Some(number)) {
            Ok(Some(point)) => points.push(PPoint {
                number,
                pos: point.pos.to_array(),
                dir: point.dir.to_array(),
                bore: point.bore,
            }),
            Ok(None) => {}
            Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(points)
}

/// E3D 不画的槽位短于这个长度就不记诊断（建模噪声）；达到它的断口记 `adapter.implied_tube_not_drawn`。
pub const IMPLIED_TUBE_DIAGNOSTIC_MIN_MM: f64 = 10.0;

/// 一段隐式管身：按 E3D 画管的两道门（长度 ≥ 1 mm、两端共轴）决定合成还是留痕。
///
/// 合成的 `TUBI` 成员方向按几何取——`leave_axis` 顺流、`arrive_axis` 朝外（逆流），与真实成员
/// 的口径一致；`pos` 取中点；`outside_diameter` 按 `CGETOD`：槽位管子目录件外径 `catalogue_od`，
/// 取不到回落到到达侧通径。
#[allow(clippy::too_many_arguments)]
fn push_implied_tube(
    members: &mut Vec<BranchMember>,
    diameters: &mut HashMap<String, DiameterEvidence>,
    issues: &mut Vec<MbdV2Issue>,
    branch_refno: &str,
    from_refno: &str,
    from: &RoutePoint,
    from_dir_known: bool,
    to_refno: &str,
    to: &RoutePoint,
    arrive_check: ArriveCheck,
    catalogue_od: Option<f64>,
) {
    let refno = format!("{from_refno}~{to_refno}");
    let length = tube_length(from.pos, to.pos);
    if !(length >= ZERO_LENGTH_TOL_MM) {
        // 两站贴着（法兰对阀门之类）：没有管身，也不值得记一条。
        return;
    }
    if from_dir_known {
        match slot_alignment(from, to, arrive_check) {
            SlotAlignment::Aligned { .. } | SlotAlignment::Indeterminate => {}
            SlotAlignment::Misaligned { deviation_deg }
            | SlotAlignment::Reversed { deviation_deg } => {
                // 几毫米的装配余量 / 目录 P 点与构件端面的错位（现场 1.4 mm 偏 0.33°、15 mm 反向重叠）
                // 是建模噪声，E3D 不画也没人在意；只有够长的断口才值得在诊断里出声。
                if length >= IMPLIED_TUBE_DIAGNOSTIC_MIN_MM {
                    issues.push(suppress_issue(
                        branch_refno,
                        &refno,
                        "adapter.implied_tube_not_drawn",
                        &format!(
                            "implied tube {from_refno} -> {to_refno} ({length:.1} mm) is not coaxial with its ports \
                             ({deviation_deg:.2}° off); E3D draws no tube here, so no tube length is dimensioned"
                        ),
                    ));
                }
                return;
            }
        }
    }
    let dir = (to.pos - from.pos) / length;
    diameters.insert(refno.clone(), diameter_evidence(catalogue_od, to.bore));
    members.push(BranchMember {
        refno,
        owner_refno: branch_refno.to_string(),
        start: from.pos.to_array(),
        end: to.pos.to_array(),
        noun: Some("TUBI".to_string()),
        arrive_axis: Some((-dir).to_array()),
        leave_axis: Some(dir.to_array()),
        pos: Some(((from.pos + to.pos) * 0.5).to_array()),
        arrive_refno: None,
        order: members.len() as u32,
        outside_diameter: outside_diameter(catalogue_od, to.bore),
        arrive_noun: None,
        ppoints: Vec::new(),
        arrive_ppoint: None,
        leave_ppoint: None,
        dtxr: None,
        weld_number: None,
        name: None,
        stext: None,
        connection: None,
    });
}

/// 可选的文本属性：字典里没有（UDA 未定义）或没写都算没有；读库出错照常上抛。
fn optional_text(
    member: &e3d_io::db_element::DbElement,
    name: &str,
) -> Result<Option<String>, BranchQueryError> {
    match member.get_string(name) {
        Ok(value) => Ok(value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// 解一个端口；目录库没开 / 目标不在所钉会话 / 目录链走不到 P 点都算「解不出」——记一条
/// 局部抑制、成员留在序列里。其它错误是读库出了问题，原样上抛。
fn resolve_port(
    end: &RouteEnd,
    resolver: &CataloguePoints,
    branch_refno: &str,
    object_refno: &str,
    side: &str,
    issues: &mut Vec<MbdV2Issue>,
) -> Result<Option<RoutePoint>, BranchQueryError> {
    match resolve_end(end, resolver) {
        Ok(point) => Ok(point),
        Err(DbElementError::DbNotRegistered { dbno }) => {
            issues.push(suppress_issue(
                branch_refno,
                object_refno,
                "adapter.member_port_unresolved",
                &format!("{side} port needs catalogue db {dbno}, which is not registered"),
            ));
            Ok(None)
        }
        Err(DbElementError::NotFound { dbno, .. }) => {
            issues.push(suppress_issue(
                branch_refno,
                object_refno,
                "adapter.member_port_unresolved",
                &format!("{side} port catalogue target is missing in db {dbno}"),
            ));
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
