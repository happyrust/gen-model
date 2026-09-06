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
//! - `outside_diameter`：暂只回落到到达侧通径（`tube_outer_diameter(None, None, bore)`）；目录
//!   `PARA[2]` 外径的读法在 e3d-model 里是 `pub(crate)`，等 Phase 4 偏移量真正用到 aod 时再开口。
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
//! （宿主映射 422）。refno 不存在 → `NotFound`（404）；不是派生隐式管身的路由容器 → `NotRouteContainer`（422）。

use std::collections::HashMap;
use std::sync::Arc;

use e3d_io::db_element::{DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::CataloguePoints;
use e3d_model::category::derives_implied_tubes;
use e3d_model::route::{
    ArriveCheck, RouteEnd, RoutePoint, SlotAlignment, ZERO_LENGTH_TOL_MM, resolve_end,
    slot_alignment, tube_length, tube_outer_diameter, walk_route,
};
use e3d_model::transform::world_matrix;
use glam::DVec3;
use plant_mbd::{BranchMember, BranchQueryResult, MbdV2Issue, root_error_issue, suppress_issue};

/// 前端 `mbd_refno` 与 plant-mbd fixtures 的 refno 写法：`w0_w1`。
pub fn wire_refno(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1 as i32)
}

#[derive(Debug)]
pub enum BranchQueryError {
    /// refno 在所钉会话里不存在。
    NotFound { refno: RefNo },
    /// 元素存在，但不是派生隐式管身的路由容器（`derives_implied_tubes` 为假，如 HVAC 下的 BRAN）。
    NotRouteContainer { refno: RefNo, noun: String },
    /// 根错误：整条 BRAN 无法建立可信的成员结点视图；`issues` 带全部已收集的定位信息。
    RootError { refno: RefNo, issues: Vec<MbdV2Issue> },
    /// 求解器拒绝（`layout_branch` 的 `Err`，目前只有「查不到分支」一种，理论上不会到这里）。
    Solver { refno: RefNo, message: String },
    Db(DbElementError),
}

impl std::fmt::Display for BranchQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::NotRouteContainer { refno, noun } => write!(
                f,
                "refno {refno}（{noun}）不是派生隐式管身的路由容器，没有管道尺寸标注"
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

    let branch_refno = wire_refno(refno);
    let route = walk_route(&container)?;
    let resolver = CataloguePoints;
    let mut issues: Vec<MbdV2Issue> = Vec::new();
    if route.pass_through_unresolved > 0 {
        // 目录链走不到 SCOM（无 SPRE / 目录库没开 / 目标不在所钉会话）的 ATTA 被按穿过件处理。
        // MDS 梯架支撑那种 GTYP=ANCI 的 ATTA 本该是站点、两侧断管——目录库不在本次 DbSet 里时
        // 它们会静默降级成穿过件，管身少几段。出声，别让人去怪切段。
        issues.push(suppress_issue(
            &branch_refno,
            &branch_refno,
            "adapter.atta_catalogue_unresolved",
            &format!(
                "{} ATTA member(s) could not be classified through SPRE -> SCOM.GTYP (catalogue not open or \
                 target missing) and were treated as pass-through; ANCI supports among them would be stations",
                route.pass_through_unresolved
            ),
        ));
    }
    let mut members: Vec<BranchMember> = Vec::with_capacity(route.members.len() + route.non_route_members.len());

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

    // 隐式管身的起点：上一站的离开点（Head 从 HPOS 起步；HDIR 缺时只判长度不判共轴）。
    let mut last_leave: Option<(String, RoutePoint, bool)> = head_pos.map(|pos| {
        (
            "Head".to_string(),
            RoutePoint {
                pos: DVec3::from_array(pos),
                dir: head_dir.map(DVec3::from_array).unwrap_or(DVec3::Z),
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
                resolve_port(&route.tubes[k].to, &resolver, &branch_refno, &object_refno, "arrive", &mut issues)?,
                resolve_port(&route.tubes[k + 1].from, &resolver, &branch_refno, &object_refno, "leave", &mut issues)?,
            ),
            None => (None, None),
        };

        if let (Some(_), Some(arrive_point), Some((from_refno, from, from_dir_known))) =
            (station, arrive, last_leave.as_ref())
        {
            push_implied_tube(
                &mut members,
                &mut issues,
                &branch_refno,
                from_refno,
                from,
                *from_dir_known,
                &object_refno,
                &arrive_point,
                ArriveCheck::Member,
            );
        }

        let pos = match world_matrix(&member) {
            Ok(matrix) => Some(matrix.w_axis.truncate()),
            Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => None,
            Err(error) => return Err(error.into()),
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
            outside_diameter: tube_outer_diameter(None, None, arrive.and_then(|point| point.bore)),
            arrive_noun: None,
        });

        if station.is_some() {
            // 站点：下一段隐式管身从它的离开点起；离开点解不出就断掉这一段（不猜）。
            last_leave = leave.map(|point| (object_refno.clone(), point, true));
        }
    }

    // 末站→Tail 的尾管（TPOS；TDIR 缺时只判离开侧，同 e3d-model `build_route_tubes`）。
    if let (Some(tail), Some((from_refno, from, from_dir_known))) = (tail_pos, last_leave.as_ref()) {
        let tail_point = RoutePoint {
            pos: DVec3::from_array(tail),
            dir: tail_dir.map(DVec3::from_array).unwrap_or(DVec3::Z),
            bore: container.get_double("TBOR").ok().flatten(),
        };
        let arrive_check = if tail_dir.is_some() { ArriveCheck::Member } else { ArriveCheck::Skip };
        push_implied_tube(
            &mut members,
            &mut issues,
            &branch_refno,
            from_refno,
            from,
            *from_dir_known,
            "Tail",
            &tail_point,
            arrive_check,
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
    Ok(BranchQueryResult {
        branch_refno,
        members,
        head_pos,
        head_dir,
        tail_pos,
        tail_dir,
        split_members: None,
        issues,
    })
}

fn refno_key(refno: RefNo) -> (u32, u32) {
    (refno.word0, refno.word1)
}

/// E3D 不画的槽位短于这个长度就不记诊断（建模噪声）；达到它的断口记 `adapter.implied_tube_not_drawn`。
pub const IMPLIED_TUBE_DIAGNOSTIC_MIN_MM: f64 = 10.0;

/// 一段隐式管身：按 E3D 画管的两道门（长度 ≥ 1 mm、两端共轴）决定合成还是留痕。
///
/// 合成的 `TUBI` 成员方向按几何取——`leave_axis` 顺流、`arrive_axis` 朝外（逆流），与真实成员
/// 的口径一致；`pos` 取中点；`outside_diameter` 按 `CGETOD` 回落到到达侧通径。
#[allow(clippy::too_many_arguments)]
fn push_implied_tube(
    members: &mut Vec<BranchMember>,
    issues: &mut Vec<MbdV2Issue>,
    branch_refno: &str,
    from_refno: &str,
    from: &RoutePoint,
    from_dir_known: bool,
    to_refno: &str,
    to: &RoutePoint,
    arrive_check: ArriveCheck,
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
            SlotAlignment::Misaligned { deviation_deg } | SlotAlignment::Reversed { deviation_deg } => {
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
        outside_diameter: tube_outer_diameter(None, None, to.bore),
        arrive_noun: None,
    });
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
