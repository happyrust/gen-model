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
use e3d_model::route::{RouteEnd, RoutePoint, resolve_end, tube_outer_diameter, walk_route};
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
    let mut members: Vec<BranchMember> = Vec::with_capacity(route.members.len() + route.non_route_members.len());

    // 站点成员在链上的下标：成员 k 的到达端 = tubes[k].to，离开端 = tubes[k+1].from。
    let station_index: HashMap<(u32, u32), usize> = route
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (refno_key(member.refno()), index))
        .collect();

    // 文件原序（PML `coll all bran mem`），站点与穿过件混排，order = 原序下标。
    for (order, member) in container.members()?.enumerate() {
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
            refno: object_refno,
            owner_refno: branch_refno.clone(),
            start: start.to_array(),
            end: end.to_array(),
            noun: Some(noun),
            arrive_axis: arrive.map(|point| point.dir.to_array()),
            leave_axis: leave.map(|point| point.dir.to_array()),
            pos: pos.map(|origin: DVec3| origin.to_array()),
            arrive_refno: None,
            order: order as u32,
            outside_diameter: tube_outer_diameter(None, None, arrive.and_then(|point| point.bore)),
            arrive_noun: None,
        });
    }

    // Head：BRAN 真实的 HPOS/HDIR（PML L1493-1494）。`container_head` 会把缺 HDIR 补成 +Z，
    // 这里分开读，缺了就如实缺、走兜底并留痕。
    let head_pos = container.get_position("HPOS")?;
    let head_dir = container.get_direction("HDIR")?;
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

    Ok(BranchQueryResult {
        branch_refno,
        members,
        head_pos,
        head_dir,
        split_members: None,
        issues,
    })
}

fn refno_key(refno: RefNo) -> (u32, u32) {
    (refno.word0, refno.word1)
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
