//! 设备视图（plant3d-web#103 M3 宿主侧）：一台 EQUI → `plant_mbd::EquipmentQueryResult`。字段与 PML 口径见 plant-mbd
//! `docs/field-dictionary.md`「设备视图」（markEqui 构造 L92-124、mbdlocation `getnozzdim` L628-681 /
//! `drawEquiMlabel` L1533-1612）。
//!
//! - `origin` / `orientation`：EQUI 的世界矩阵（`pos / ori of $!equi wrt /*`）的平移列与三根轴。
//! - `bbox`（`volume $!equi`）：EQUI 是容器，SPAMAP 里没有它；取它与全部后代（BOX / CYLI / DISH / NOZZ… 图元；负体与
//!   容器同样没有条目）在本库 SPAMAP（[`host::spamap_index`]）里的包围盒之并。SPAMAP 是存盘快照、0.1 mm；一个条目都
//!   没有时缺省并留一句话（求解器再报 `equipment.bbox_missing`）。e3d-io 的 `BuiltInAabb` 只认 BOX / CYLI，不用。
//! - `elconn_pos`：后代里第一个 ELCONN（深度优先、文件原序，即 PML `coll all elconn` 的 `[1]`）的世界原点。
//! - `nozzles`：后代里全部 NOZZ（同序）。P1 走 `CataloguePoints`（CATR → SCOM → PTSE，世界系）：位置、朝外方向、PBOR。
//!   CATR 为空（AMS 很多管口是 0/0）就留空，求解器跳过并报 `equipment.nozzle_without_p1`；目录库没注册、目标不在所钉
//!   会话也留空，另留一句话。
//! - `civil_candidates`：基准点（ELCONN，否则原点）五条射线上的墙板（[`locate_candidates`]：搜 `DbSet` 里全部 DESI
//!   库的 SPAMAP，墙在 7997、设备在 7999 也找得到）。
//!
//! 错误：EQUI 本身不在所钉会话里 → [`EquipmentQueryError::NotFound`]；读库出错原样上抛；其余（包围盒、单个管口、单个
//! 候选、读不动的 SPAMAP）降级成 `issues` 里的一句话，求解器逐条转成 `data.view_issue`。

use std::sync::Arc;

use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::CataloguePoints;
use e3d_model::route::PPointResolver;
use glam::{DMat3, DVec3};
use plant_mbd::{EquipmentAxes, EquipmentQueryResult, NozzleView, SourceBox};

use super::civil_faces::locate_candidates;
use super::host::{self, HostError, noun_of, soft};
use super::wire_refno;

/// 设备子树的深度上限：EQUI → SUBE → 图元实测三层，64 只可能是环。
const MAX_DEPTH: usize = 64;

#[derive(Debug)]
pub enum EquipmentQueryError {
    /// refno 在所钉会话里不存在。
    NotFound {
        refno: RefNo,
    },
    Db(DbElementError),
}

impl std::fmt::Display for EquipmentQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for EquipmentQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::NotFound { .. } => None,
        }
    }
}

impl From<DbElementError> for EquipmentQueryError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// 一台设备的视图，见模块文档。归属不是 EQUI 时照读（求解器按 `subject_noun` 只出定位尺寸）。
pub fn query_equipment(
    set: &Arc<DbSet>,
    subject_refno: RefNo,
) -> Result<EquipmentQueryResult, EquipmentQueryError> {
    let equi = set.element(subject_refno);
    match equi.exists() {
        Ok(true) => {}
        Ok(false)
        | Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            return Err(EquipmentQueryError::NotFound {
                refno: subject_refno,
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut view = EquipmentQueryResult {
        subject_refno: wire_refno(subject_refno),
        subject_noun: noun_of(&equi)?,
        name: soft(equi.stored_name())?.flatten(),
        ..EquipmentQueryResult::default()
    };
    match host::world_matrix_soft(&equi)? {
        Some(world) => {
            view.origin = Some(world.w_axis.truncate().to_array());
            view.orientation = axes(DMat3::from_mat4(world));
        }
        None => view.issues.push(format!(
            "{} {} 的属主链折不进世界系（链上有库没注册），没有原点与朝向",
            view.subject_noun, view.subject_refno
        )),
    }

    let mut tree = Descendants::default();
    walk(&equi, 0, &mut tree)?;
    view.bbox = bbox(set, &equi, &tree.refnos, &mut view.issues)?;
    if let Some(elconn) = tree.elconns.first() {
        view.elconn_pos = host::world_origin(elconn)?.map(|point| point.to_array());
        if view.elconn_pos.is_none() {
            view.issues.push(format!(
                "ELCONN {} 的属主链折不进世界系，基准点退用设备原点",
                wire_refno(elconn.refno())
            ));
        }
    }
    for nozzle in &tree.nozzles {
        view.nozzles.push(nozzle_view(nozzle, &mut view.issues)?);
    }
    if let Some(base) = view.elconn_pos.or(view.origin) {
        let found = locate_candidates(set, DVec3::from_array(base))?;
        view.civil_candidates = found.candidates;
        view.issues.extend(found.issues);
    }
    Ok(view)
}

/// 三根轴各自归一；有一根退化就不给。
fn axes(rotation: DMat3) -> Option<EquipmentAxes> {
    let unit = |axis: DVec3| axis.try_normalize().map(|axis| axis.to_array());
    Some(EquipmentAxes {
        x: unit(rotation.x_axis)?,
        y: unit(rotation.y_axis)?,
        z: unit(rotation.z_axis)?,
    })
}

/// 设备子树：全部后代的 refno，以及其中的 ELCONN / NOZZ（深度优先、文件原序）。
#[derive(Default)]
struct Descendants {
    refnos: Vec<RefNo>,
    elconns: Vec<DbElement>,
    nozzles: Vec<DbElement>,
}

fn walk(owner: &DbElement, depth: usize, tree: &mut Descendants) -> Result<(), DbElementError> {
    if depth >= MAX_DEPTH {
        return Err(DbElementError::OwnerCycle {
            start: owner.refno(),
            at: owner.refno(),
        });
    }
    for member in owner.members()? {
        tree.refnos.push(member.refno());
        match soft(noun_of(&member))?.as_deref() {
            Some("ELCONN") => tree.elconns.push(member.clone()),
            Some("NOZZ") => tree.nozzles.push(member.clone()),
            _ => {}
        }
        walk(&member, depth + 1, tree)?;
    }
    Ok(())
}

/// EQUI 与后代在本库 SPAMAP 里的包围盒之并；表读不动、一个条目都没有时 `None` 并留一句话。
fn bbox(
    set: &Arc<DbSet>,
    equi: &DbElement,
    descendants: &[RefNo],
    issues: &mut Vec<String>,
) -> Result<Option<SourceBox>, DbElementError> {
    let dbnum = equi.db_no();
    let index = match host::spamap_index(set, dbnum) {
        Ok(Some(index)) => index,
        Ok(None) => {
            issues.push(format!("库 {dbnum} 所钉会话没有 SPAMAP 表，设备包围盒缺"));
            return Ok(None);
        }
        Err(HostError::Db(error)) => return Err(error),
        Err(error) => {
            issues.push(format!("{error}：设备包围盒缺"));
            return Ok(None);
        }
    };
    let merged = std::iter::once(equi.refno())
        .chain(descendants.iter().copied())
        .filter_map(|refno| index.get(refno))
        .reduce(|merged, aabb| merged.union(&aabb));
    match merged {
        Some(aabb) => Ok(Some(SourceBox {
            min: aabb.min.to_array(),
            max: aabb.max.to_array(),
        })),
        None => {
            issues.push(format!(
                "{} 与它的 {} 个后代在库 {dbnum} 的 SPAMAP 里都没有包围盒，设备包围盒缺",
                wire_refno(equi.refno()),
                descendants.len()
            ));
            Ok(None)
        }
    }
}

/// 一个管口：P1 的世界位置、朝外方向、PBOR（PML `p1 pos / dir / bore of $!nozz wrt/*`）。
fn nozzle_view(nozzle: &DbElement, issues: &mut Vec<String>) -> Result<NozzleView, DbElementError> {
    let refno = wire_refno(nozzle.refno());
    let p1 = match CataloguePoints.leave(nozzle, Some(1)) {
        Ok(point) => point,
        Err(DbElementError::DbNotRegistered { dbno }) => {
            issues.push(format!("管口 {refno} 的目录在库 {dbno}，没注册进来，P1 缺"));
            None
        }
        Err(DbElementError::NotFound {
            refno: target,
            dbno,
            ..
        }) => {
            issues.push(format!(
                "管口 {refno} 的目录件 {target} 不在库 {dbno} 的所钉会话里，P1 缺"
            ));
            None
        }
        Err(error) => return Err(error),
    };
    Ok(NozzleView {
        refno,
        name: soft(nozzle.stored_name())?.flatten(),
        p1_pos: p1.map(|point| point.pos.to_array()),
        p1_dir: p1.map(|point| point.dir.to_array()),
        p1_bore: p1
            .and_then(|point| point.bore)
            .filter(|bore| bore.is_finite()),
    })
}
