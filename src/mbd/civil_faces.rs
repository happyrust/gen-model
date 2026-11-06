//! 找墙候选（plant3d-web#103 M3 宿主侧）：STWALL / GWALL / FLOOR / WALL → `plant_mbd::CivilCandidate`，以及沿
//! 基准点五条射线从 SPAMAP 收候选。字段与 PML 口径见 plant-mbd `docs/field-dictionary.md`「找墙候选」（markCivil
//! `getDataOfCivil` L431-1043、mbdlocation `getconindir` L682-872）。设备（[`super::equipment_query`]）先用，EM9
//! 仪表架支架的找墙与土建读取器随后共用。
//!
//! # 面（一律世界 mm，`world_matrix`）
//!
//! - **STWALL**：与 SCTN 同一条截面链（`section_catalogue → evaluate_section_profile → section_placement`，两端
//!   `section_cut_planes`），即画出来的那堵墙的标架。截面外包矩形 `[x0, x1] × [y0, y1]`（已减 `JUSL`、按 `LMIRR`
//!   镜像）在 `x = x0`、`x = x1` 各出一张大面：竖向 `y0..y1`，沿轴从起端切面到终端切面。PML 用 `prop thk / height`
//!   与 `ppli na`（L440-524），NA 落在墙面上时两者一样；`prop haut`（变高墙）不接，截面链本身也是等截面。
//! - **FLOOR / GWALL**：`loop_profile`（第一个 PLOO / LOOP 环的 PAVE / VERT 位置与 FRAD、HEIG、SJUS 下移）放进
//!   `world_with_sjus`：底面与顶面，逐点 FRAD 进 `radii`；相邻重合（不到 0.1 mm）的点并掉（L743）。FLOOR 上面在前
//!   （L626-633），厚度是环高。GWALL 另按环的每条边出侧面（不带圆角，L771-810），再按面积挑两张最大的放前面——离
//!   水平不到 20° 的面面积按 0 计（L816-853）；厚度是两张大面的距离，墙高是那些水平面的高差（与 HEIG 差不到 0.1
//!   时取 HEIG，L857-862）。边短于 1 mm 的侧面不出（求解器会把它判成退化、连累整个候选）。
//! - **WALL**：`spine_path` 读 SPINE（POINSP / CURVE / POINSP，THRU / FILL / CENT 都认）：同一个圆上首尾相接的弧
//!   （单段弧，或两段拼成的整圈）出 [`CurvedWall`]，单段直线按截面出两张大面，别的形状报错。截面同 STWALL
//!   （`/Single_leaf_wall` 一类：OBOW / NA 在 x = 0、IBOW 在 x = 厚）；弧上截面 x 朝行进方向左手侧（俯视）：绕
//!   WALL 的 Z 逆时针时内外半径是 `r − x1 / r − x0`，顺时针 `r + x0 / r + x1`（e3d-model `circular_wall_radii`
//!   同一口径，与 PML 按 `ppli cbow` 在弧内外加减墙厚 L605-609 等价）。弧轴翻到 WALL 的 Z 那一侧（PML
//!   `Z is arczdir`，L598-603），弧心挪到墙底平面（spine 平面沿 Z 挪 `y0`；Z 朝下的墙挪 `y1`，求解器的墙身一律从
//!   弧平面往上），墙高 `y1 − y0`。
//!
//! 截面目录链走不通（无 SPRE、链断、目录库没注册、截面没有顶点）时退回 DESP `[厚, 高]` 矩形、原点对齐（仍认
//! `LMIRR`），并留一条说明；DESP 不满两项就没有这条退路，候选读不成。
//!
//! 厚度：`:JJ_QTHD`（PML `getatts` 读 `frmw of` 墙板的，L346-347；这里先 FRMW、再墙板自己），没有按几何；离整数
//! 不到 0.1 取整（L919-921），UDA 与几何差 0.1 以上留一条说明（L910-913）。墙高离整数不到 0.01 取整（L923-925）。
//!
//! # 收候选（[`locate_candidates`]）
//!
//! 计划决策 D7：不建 AABB 树，读 E3D 自己的 SPAMAP（[`host::spamap_index`]，按文件 + 会话缓存）。从基准点起五条
//! 走廊——E / N / W / S 水平、D 向下，各长 [`LOCATE_RAY_MM`]（求解器最外一环），横截面 ±[`LOCATE_CORRIDOR_HALF_WIDTH_MM`]
//! （求解器的细盒是 ±1 mm，放宽是为了抵 SPAMAP 0.1 mm 的精度与包围盒口径差：多给的候选求解器按环与细盒再筛，少给就
//! 漏墙）。包围盒碰到水平走廊的 STWALL / GWALL / WALL、碰到向下走廊的 FLOOR 收进来，逐个转成候选；转不成的跳过并
//! 留一条说明。转成之后再拿候选自己的包围盒（[`candidate_aabb`]，与求解器 `locate::prepare` 同一算法）重测一遍
//! 走廊，不碰的丢掉：E3D 给曲墙记的 SPAMAP 盒比墙大得多（24381/44303 的盒宽 3.9 万 mm，墙是半径 28 m 的四分之一圈），
//! 只靠 SPAMAP 会多给一大批 WALL。求解器只比 ±1 mm 细盒碰到候选包围盒的那些，走廊又罩住全部细盒，所以这一步丢不掉
//! 它会用到的墙板。
//!
//! 搜哪些库：`DbSet` 里**登记了的全部 DESI 库**（库号升序）。服务端 `build_set` 把 MDB 的设计库全部钉进来，墙在
//! 7997、设备在 7999 照样找得到；离线（测试、`examples/mbd_object_dump`）只有显式钉入的那几座。SPAMAP 表读不动的库
//! 跳过并留一条说明，会话里没有 SPAMAP 表的库安静跳过。候选按库号、再按 refno 排（PML `coll` 的库内序只在距离相等
//! 时起作用）。

use std::sync::Arc;

use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::discretise::FilletVertex;
use e3d_model::elmodl::world_with_sjus;
use e3d_model::profile::{ProfileError, loop_profile};
use e3d_model::section::{
    SectionChain, SectionCutPlane, SectionCuts, SectionError, SectionProfile, SpineSegment,
    evaluate_section_profile, section_catalogue, section_cut_planes, section_placement, spine_path,
};
use e3d_model::transform::world_matrix;
use glam::{DMat3, DMat4, DVec3};
use plant_mbd::{CivilCandidate, CivilFace, CurvedWall};

use super::host::{self, Aabb, HostError, noun_of, soft};
use super::wire_refno;

/// 找墙候选的类型（markCivil L259-271）。
pub const CIVIL_CANDIDATE_NOUNS: [&str; 4] = ["STWALL", "GWALL", "WALL", "FLOOR"];
/// 五条射线的长度：求解器最外一环（`plant_mbd::locate::SEARCH_RINGS_MM` 的 50 m）。
pub const LOCATE_RAY_MM: f64 = 50_000.0;
/// 走廊横截面的半宽，见模块文档「收候选」。
pub const LOCATE_CORRIDOR_HALF_WIDTH_MM: f64 = 50.0;
/// 环上相邻点重合的判据（markCivil L743）。
const MERGE_POINT_MM: f64 = 0.1;
/// 离水平不到这个角的面按水平面（markCivil L820）。
const HORIZONTAL_FACE_DEG: f64 = 20.0;
/// 厚度离整数不到它就取整（markCivil L919）；GWALL 的面高差与 HEIG 差不到它取 HEIG（L857）。
const THICKNESS_SNAP: f64 = 0.1;
/// 墙高离整数不到它就取整（markCivil L923）。
const HEIGHT_SNAP: f64 = 0.01;
/// GWALL 侧面的边短于它就不出：求解器把轮廓里短于 1 mm 的线丢掉（`getLinesAndArcsfortwoarrays` L182），这样的
/// 侧面只剩两条边、整个候选会被判成 `locate.candidate_invalid`。
const SIDE_EDGE_MIN_MM: f64 = 1.0;
/// 一条 SPINE 上的几段弧算同一个圆的容差（弧心、半径、首尾相接）。
const SAME_CIRCLE_MM: f64 = 0.1;
const LENGTH_EPS: f64 = 1e-6;

#[derive(Debug)]
pub enum CivilFaceError {
    /// refno 在所钉会话里不存在。
    NotFound {
        refno: RefNo,
    },
    /// 不是 STWALL / GWALL / WALL / FLOOR。
    NotCivil {
        refno: RefNo,
        noun: String,
    },
    /// 读得到，但推不出面（缺 POSS / POSE、没有环、SPINE 形状不认识、截面退化…）。
    Geometry {
        refno: RefNo,
        noun: String,
        detail: String,
    },
    Db(DbElementError),
}

impl std::fmt::Display for CivilFaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::NotCivil { refno, noun } => {
                write!(f, "{noun} {refno} 不是 STWALL / GWALL / WALL / FLOOR")
            }
            Self::Geometry {
                refno,
                noun,
                detail,
            } => write!(f, "{noun} {refno} 推不出找墙用的面：{detail}"),
            Self::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CivilFaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbElementError> for CivilFaceError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// 一块墙板 → 找墙候选，见模块文档。
pub fn civil_candidate(set: &Arc<DbSet>, refno: RefNo) -> Result<CivilCandidate, CivilFaceError> {
    civil_candidate_of(&set.element(refno), &mut Vec::new())
}

/// [`civil_candidate`]，非致命的说明（截面退回 DESP、`:JJ_QTHD` 与几何不符）追加进 `notes`。
pub fn civil_candidate_of(
    element: &DbElement,
    notes: &mut Vec<String>,
) -> Result<CivilCandidate, CivilFaceError> {
    let refno = element.refno();
    match element.exists() {
        Ok(true) => {}
        Ok(false)
        | Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            return Err(CivilFaceError::NotFound { refno });
        }
        Err(error) => return Err(error.into()),
    }
    let noun = noun_of(element)?;
    let label = format!("{noun} {}", wire_refno(refno));
    let shape = match noun.as_str() {
        "STWALL" => stwall_shape(element, &label, notes),
        "GWALL" | "FLOOR" => prism_shape(element, &noun),
        "WALL" => wall_shape(element, &label, notes),
        _ => {
            return Err(CivilFaceError::NotCivil {
                refno,
                noun: noun.clone(),
            });
        }
    }
    .map_err(|failure| match failure {
        Failure::Db(error) => CivilFaceError::Db(error),
        Failure::Geometry(detail) => CivilFaceError::Geometry {
            refno,
            noun: noun.clone(),
            detail,
        },
    })?;
    let thickness = match jj_qthd(element)? {
        Some(uda) => {
            if (uda - shape.thickness).abs() > THICKNESS_SNAP {
                notes.push(format!(
                    "{label} 的 :JJ_QTHD {uda} 与几何厚度 {:.1} 不符（PML：Please check the :JJ_QTHD），按 UDA",
                    shape.thickness
                ));
            }
            uda
        }
        None => shape.thickness,
    };
    let owner_name = match soft(element.owner())?.flatten() {
        Some(owner) => soft(owner.stored_name())?.flatten(),
        None => None,
    };
    Ok(CivilCandidate {
        refno: wire_refno(refno),
        noun,
        name: soft(element.stored_name())?.flatten(),
        owner_name,
        faces: shape.faces,
        thickness: Some(snap_to_integer(tidy(thickness), THICKNESS_SNAP)),
        height: shape
            .height
            .map(|height| snap_to_integer(tidy(height), HEIGHT_SNAP)),
        curved: shape.curved,
    })
}

/// 离整数不到 `tolerance` 就取整（PML `abs(x − x.nint()) lt tol`）。
pub fn snap_to_integer(value: f64, tolerance: f64) -> f64 {
    let rounded = value.round();
    if (value - rounded).abs() < tolerance {
        rounded
    } else {
        value
    }
}

/// 几何量去掉浮点尾数（记到 1e-6 mm；E3D 自己的位置记到 0.01 mm）。
fn tidy(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// 一块墙板读出来的几何（厚度是几何值，UDA 在外面盖）。
struct Shape {
    faces: Vec<CivilFace>,
    curved: Option<CurvedWall>,
    thickness: f64,
    height: Option<f64>,
}

enum Failure {
    Db(DbElementError),
    Geometry(String),
}

impl From<DbElementError> for Failure {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

impl From<SectionError> for Failure {
    fn from(error: SectionError) -> Self {
        match error {
            SectionError::Db(error) => Self::Db(error),
            error => Self::Geometry(error.to_string()),
        }
    }
}

impl From<ProfileError> for Failure {
    fn from(error: ProfileError) -> Self {
        match error {
            ProfileError::Db(error) => Self::Db(error),
            error => Self::Geometry(error.to_string()),
        }
    }
}

/// `:JJ_QTHD`：属主链上的 FRMW 优先（PML `getatts`），再墙板自己；非正数、读不成数都按没有。
fn jj_qthd(element: &DbElement) -> Result<Option<f64>, DbElementError> {
    if let Some(frmw) = host::ancestor_of_type(element, "FRMW")?
        && let Some(value) = uda_number(&frmw, ":JJ_QTHD")?
    {
        return Ok(Some(value));
    }
    uda_number(element, ":JJ_QTHD")
}

/// 数值 UDA：字典里没有 / 对这个 noun 无效按没有；存成文本的按文本解数。
fn uda_number(element: &DbElement, name: &str) -> Result<Option<f64>, DbElementError> {
    let value = match element.get_double(name) {
        Ok(value) => value,
        Err(DbElementError::UnknownAttribute { .. } | DbElementError::Uda { .. }) => None,
        Err(DbElementError::TypeMismatch { .. }) => host::optional_text(element, name)
            .ok()
            .flatten()
            .and_then(|text| text.trim().parse::<f64>().ok()),
        Err(error) => return Err(error),
    };
    Ok(value.filter(|value| value.is_finite() && *value > 0.0))
}

// ---- 截面（STWALL / WALL） -----------------------------------------------------------------------

/// 截面外包矩形，在截面平面里（已减 `JUSL`、按 `LMIRR` 镜像）：x 是墙厚方向，y 朝上。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionBox {
    pub x: [f64; 2],
    pub y: [f64; 2],
}

impl SectionBox {
    pub fn width(&self) -> f64 {
        self.x[1] - self.x[0]
    }

    pub fn height(&self) -> f64 {
        self.y[1] - self.y[0]
    }
}

/// 截面：目录链求得出就用目录截面（连同求值结果，放置要它的中性轴转角），否则退回 DESP 矩形。
fn wall_section(
    element: &DbElement,
    label: &str,
    notes: &mut Vec<String>,
) -> Result<(SectionBox, Option<SectionProfile>), Failure> {
    let why = match section_catalogue(element) {
        Ok(SectionChain::Resolved(catalogue)) => {
            match evaluate_section_profile(element, &catalogue) {
                Ok(profile) => match profile_box(&profile) {
                    Some(section) => return Ok((section, Some(profile))),
                    None => "目录截面没有顶点".to_string(),
                },
                Err(SectionError::Db(error)) => return Err(error.into()),
                Err(error) => format!("目录截面求不出（{error}）"),
            }
        }
        Ok(SectionChain::NoSpre) => "没有 SPRE".to_string(),
        Ok(SectionChain::Broken { hop, at, why }) => {
            format!("SPRE 链第 {hop} 跳断在 {at}（{why}）")
        }
        Err(SectionError::Db(error)) => return Err(error.into()),
        Err(error) => format!("截面目录链读不成（{error}）"),
    };
    let desp = host::optional_doubles(element, "DESP")?.unwrap_or_default();
    match desp[..] {
        [thickness, height, ..] if thickness > LENGTH_EPS && height > LENGTH_EPS => {
            notes.push(format!(
                "{label} {why}：截面按 DESP [{thickness}, {height}] 矩形、原点对齐"
            ));
            let mirrored = matches!(element.get_bool("LMIRR"), Ok(Some(true)));
            let x = if mirrored {
                [-thickness, 0.0]
            } else {
                [0.0, thickness]
            };
            Ok((
                SectionBox {
                    x,
                    y: [0.0, height],
                },
                None,
            ))
        }
        _ => Err(Failure::Geometry(format!(
            "{why}，DESP {desp:?} 又没给满厚、高两项"
        ))),
    }
}

/// 目录截面的外包矩形：顶点减 `JUSL` 偏移，`LMIRR` 时沿 x 镜像（e3d-model `wall_section_from_profile` 同口径）。
fn profile_box(profile: &SectionProfile) -> Option<SectionBox> {
    let [jx, jy] = profile.justification;
    let mut x = [f64::INFINITY, f64::NEG_INFINITY];
    let mut y = [f64::INFINITY, f64::NEG_INFINITY];
    for vertex in profile
        .members
        .iter()
        .flat_map(|member| member.rings.iter().flatten())
    {
        x = [x[0].min(vertex.x - jx), x[1].max(vertex.x - jx)];
        y = [y[0].min(vertex.y - jy), y[1].max(vertex.y - jy)];
    }
    if !(x[1] - x[0] > LENGTH_EPS && y[1] - y[0] > LENGTH_EPS) {
        return None;
    }
    Some(SectionBox {
        x: if profile.mirrored { [-x[1], -x[0]] } else { x },
        y,
    })
}

/// DESP 退路上放置用的空截面：不平移、不转。
fn bare_profile() -> SectionProfile {
    SectionProfile {
        members: Vec::new(),
        justification: [0.0, 0.0],
        justification_line: String::new(),
        justification_found: false,
        neutral_axis_spin: 0.0,
        level: 0,
        mirrored: false,
        notes: Vec::new(),
    }
}

fn stwall_shape(
    element: &DbElement,
    label: &str,
    notes: &mut Vec<String>,
) -> Result<Shape, Failure> {
    let (section, profile) = wall_section(element, label, notes)?;
    let profile = profile.unwrap_or_else(bare_profile);
    let placement = section_placement(element, &profile)?;
    let cuts = section_cut_planes(element, &placement)?;
    let to_world = world_matrix(element)? * placement.local;
    let faces = section_wall_faces(to_world, placement.length, Some(&cuts), section);
    Ok(Shape {
        faces: Vec::from(faces),
        curved: None,
        thickness: section.width(),
        height: Some(section.height()),
    })
}

/// 直段墙的两张大面：`to_world` 把截面平面 `(x, y)` + 沿轴 `z ∈ [0, length]` 放进世界（`world_matrix ×
/// SectionPlacement::local`）；在 `x0`、`x1` 两处各一张，竖向 `y0..y1`，两端按切面（`SectionCuts`，局部系）收到
/// 斜切的位置。顶点序：起端底、终端底、终端顶、起端顶。
pub fn section_wall_faces(
    to_world: DMat4,
    length: f64,
    cuts: Option<&SectionCuts>,
    section: SectionBox,
) -> [CivilFace; 2] {
    let rotation = DMat3::from_mat4(to_world);
    let side = |x: f64, outward: f64| {
        let z_at = |y: f64, end: bool| match cuts {
            Some(cuts) if end => cut_z(&cuts.end, x, y).unwrap_or(length),
            Some(cuts) => cut_z(&cuts.start, x, y).unwrap_or(0.0),
            None if end => length,
            None => 0.0,
        };
        let point = |y: f64, end: bool| to_world.transform_point3(DVec3::new(x, y, z_at(y, end)));
        let [y0, y1] = section.y;
        let points = [
            point(y0, false),
            point(y0, true),
            point(y1, true),
            point(y1, false),
        ];
        civil_face(&points, Some(rotation * DVec3::X * outward), Vec::new())
    };
    [side(section.x[0], -1.0), side(section.x[1], 1.0)]
}

/// 截面点 `(x, y)` 那条平行于轴的棱与切面的交点 z（XSPYR 的 `POSCAL`，`|n.z|` 过小时不交）。
fn cut_z(plane: &SectionCutPlane, x: f64, y: f64) -> Option<f64> {
    (plane.normal.z.abs() > 1e-4).then(|| {
        plane.point.z
            - (plane.normal.x * (x - plane.point.x) + plane.normal.y * (y - plane.point.y))
                / plane.normal.z
    })
}

fn wall_shape(element: &DbElement, label: &str, notes: &mut Vec<String>) -> Result<Shape, Failure> {
    let Some(path) = spine_path(element)? else {
        return Err(Failure::Geometry("没有 SPINE".into()));
    };
    let (section, _) = wall_section(element, label, notes)?;
    let world = world_matrix(element)?;
    let up = world.transform_vector3(DVec3::Z);
    if let Some(arc) = one_circle(&path.segments) {
        let curved = curved_wall(
            world.transform_point3(arc.centre),
            world.transform_vector3(arc.axis),
            world.transform_point3(arc.start),
            world.transform_point3(arc.end),
            arc.sweep,
            up,
            section,
        )
        .map_err(Failure::Geometry)?;
        return Ok(Shape {
            faces: Vec::new(),
            curved: Some(curved),
            thickness: section.width(),
            height: Some(section.height()),
        });
    }
    match path.segments.as_slice() {
        [SpineSegment::Straight { start, end }] => {
            let faces = straight_wall_faces(
                world.transform_point3(*start),
                world.transform_point3(*end),
                up,
                section,
            )
            .map_err(Failure::Geometry)?;
            Ok(Shape {
                faces: Vec::from(faces),
                curved: None,
                thickness: section.width(),
                height: Some(section.height()),
            })
        }
        segments => {
            let kinds: Vec<&str> = segments
                .iter()
                .map(|segment| match segment {
                    SpineSegment::Straight { .. } => "直段",
                    SpineSegment::Arc { .. } => "弧",
                })
                .collect();
            let problems = if path.problems.is_empty() {
                String::new()
            } else {
                format!("；{}", path.problems.join("；"))
            };
            Err(Failure::Geometry(format!(
                "SPINE 走成 {} 段 {kinds:?}，只认同一个圆上的弧或单段直线{problems}",
                segments.len()
            )))
        }
    }
}

/// 同一个圆上首尾相接的几段弧（含 POINSP / CURVE / POINSP / CURVE / POINSP 的整圈），并成一段。
struct OneCircle {
    start: DVec3,
    end: DVec3,
    centre: DVec3,
    axis: DVec3,
    sweep: f64,
}

/// SPINE 全是弧、弧心 / 轴向 / 半径一致且首尾相接时并成一段（扫角相加，封顶一整圈）；PML 也是三点定圆、逐点累加
/// 夹角（markCivil L542-569）。否则 `None`。
fn one_circle(segments: &[SpineSegment]) -> Option<OneCircle> {
    let mut merged: Option<OneCircle> = None;
    for segment in segments {
        let SpineSegment::Arc {
            start,
            end,
            centre,
            axis,
            sweep,
        } = segment
        else {
            return None;
        };
        match merged.as_mut() {
            None => {
                merged = Some(OneCircle {
                    start: *start,
                    end: *end,
                    centre: *centre,
                    axis: *axis,
                    sweep: *sweep,
                });
            }
            Some(circle) => {
                let radius = (circle.start - circle.centre).length();
                let same = (*centre - circle.centre).length() < SAME_CIRCLE_MM
                    && axis.dot(circle.axis) > 1.0 - 1e-9
                    && ((*start - circle.centre).length() - radius).abs() < SAME_CIRCLE_MM
                    && (*start - circle.end).length() < SAME_CIRCLE_MM;
                if !same {
                    return None;
                }
                circle.end = *end;
                circle.sweep = (circle.sweep + sweep).min(std::f64::consts::TAU);
            }
        }
    }
    merged
}

/// 弧形 spine 的 WALL → [`CurvedWall`]。入参都在世界系：弧心、行进方向的右手轴、起止点、扫角（弧度，> 0）、WALL
/// 自己的 Z（截面的 y 向）与截面外包矩形。半径、轴向与弧心的口径见模块文档「WALL」。
pub fn curved_wall(
    centre: DVec3,
    axis: DVec3,
    start: DVec3,
    end: DVec3,
    sweep_rad: f64,
    up: DVec3,
    section: SectionBox,
) -> Result<CurvedWall, String> {
    let axis = axis.try_normalize().ok_or("弧轴是零向量")?;
    let up = up.try_normalize().ok_or("WALL 的 Z 是零向量")?;
    let in_plane = |point: DVec3| {
        let radial = point - centre;
        radial - axis * radial.dot(axis)
    };
    let radius = in_plane(start).length();
    if !(radius > LENGTH_EPS) {
        return Err(format!("spine 半径 {radius} 退化"));
    }
    let sweep_deg = sweep_rad.to_degrees();
    if !(sweep_deg > 1e-9 && sweep_deg <= 360.0 + 1e-6) {
        return Err(format!("扫角 {sweep_deg}° 不在 (0, 360]"));
    }
    let counter_clockwise = axis.dot(up) >= 0.0;
    let [x0, x1] = section.x;
    let (inner, outer) = if counter_clockwise {
        (radius - x1, radius - x0)
    } else {
        (radius + x0, radius + x1)
    };
    if !(inner > 0.0 && outer > inner) {
        return Err(format!(
            "内外半径 {inner:.3} / {outer:.3} 不成立（spine 半径 {radius:.3}，截面 x {:?}）",
            section.x
        ));
    }
    let (arc_axis, from) = if counter_clockwise {
        (axis, start)
    } else {
        (-axis, end)
    };
    let x_axis = in_plane(from).try_normalize().ok_or("弧端点与弧心重合")?;
    // 求解器把弧平面当墙底、墙身朝世界上方（`arc_hit` 只认弧平面以上 `height` 内的交点）：WALL 的 Z 朝下时，
    // 墙底是 spine 平面沿 Z 挪 `y1` 的那一面。
    let bottom = if up.z >= 0.0 {
        section.y[0]
    } else {
        section.y[1]
    };
    Ok(CurvedWall {
        center: (centre + up * bottom).to_array(),
        axis: arc_axis.to_array(),
        x_axis: x_axis.to_array(),
        sweep_deg: sweep_deg.min(360.0),
        inner_radius: tidy(inner),
        outer_radius: tidy(outer),
    })
}

/// 直 SPINE 的 WALL：截面 x = WALL 的 Z × 行进方向（与弧墙同一套截面系），两张大面同 [`section_wall_faces`]，
/// 两端平头（SPINE 的 `DRNS` / `DRNE` 不接）。
fn straight_wall_faces(
    start: DVec3,
    end: DVec3,
    up: DVec3,
    section: SectionBox,
) -> Result<[CivilFace; 2], String> {
    let along = (end - start).try_normalize().ok_or("SPINE 两点重合")?;
    let x_axis = up
        .cross(along)
        .try_normalize()
        .ok_or("SPINE 与 WALL 的 Z 平行，截面朝向定不了")?;
    let y_axis = along.cross(x_axis);
    let [y0, y1] = section.y;
    let side = |x: f64, outward: f64| {
        let at = |base: DVec3, y: f64| base + x_axis * x + y_axis * y;
        let points = [at(start, y0), at(end, y0), at(end, y1), at(start, y1)];
        civil_face(&points, Some(x_axis * outward), Vec::new())
    };
    Ok([side(section.x[0], -1.0), side(section.x[1], 1.0)])
}

// ---- 环拉伸（FLOOR / GWALL） ---------------------------------------------------------------------

fn prism_shape(element: &DbElement, noun: &str) -> Result<Shape, Failure> {
    let profile = loop_profile(element)?;
    if !(profile.height > LENGTH_EPS) {
        return Err(Failure::Geometry(format!(
            "环高 HEIG = {}（E3D 同样不出几何）",
            profile.height
        )));
    }
    let ring = merged_ring(&profile.rings[0]);
    if ring.len() < 3 {
        return Err(Failure::Geometry(format!(
            "外环并掉重合点后只剩 {} 个顶点",
            ring.len()
        )));
    }
    let world = world_with_sjus(world_matrix(element)?, profile.sjus_drop);
    let prism = prism_faces(world, &ring, profile.height, noun == "GWALL");
    Ok(Shape {
        faces: prism.faces,
        curved: None,
        thickness: prism.thickness,
        height: prism.height,
    })
}

/// 相邻重合（不到 0.1 mm）的顶点并掉，首尾重合的也并（markCivil L743）；留先到的那个点的 FRAD。
fn merged_ring(ring: &[FilletVertex]) -> Vec<FilletVertex> {
    let close = |a: &FilletVertex, b: &FilletVertex| (a.x - b.x).hypot(a.y - b.y) < MERGE_POINT_MM;
    let mut merged: Vec<FilletVertex> = Vec::with_capacity(ring.len());
    for vertex in ring {
        if merged.last().is_some_and(|last| close(last, vertex)) {
            continue;
        }
        merged.push(*vertex);
    }
    while merged.len() > 1 && close(&merged[0], &merged[merged.len() - 1]) {
        merged.pop();
    }
    merged
}

/// [`prism_faces`] 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct PrismFaces {
    /// 两张大面在前（FLOOR：上面、下面；GWALL：面积最大的两张），GWALL 的其余面接在后面。
    pub faces: Vec<CivilFace>,
    /// 几何厚度：FLOOR 是环高，GWALL 是两张大面的距离（markCivil L699 / L854）。
    pub thickness: f64,
    /// GWALL 的墙高：离水平不到 20° 的面的高差（与环高差不到 0.1 取环高）；FLOOR 没有。
    pub height: Option<f64>,
}

/// 环沿局部 +Z 拉伸 `height` 的面：`world` 已含 SJUS 下移（`world_with_sjus`），环在局部 XY 平面。底面 / 顶面带逐点
/// FRAD；`with_sides`（GWALL）时环的每条边再出一张侧面，按面积挑两张最大的放前面。见模块文档「FLOOR / GWALL」。
pub fn prism_faces(
    world: DMat4,
    ring: &[FilletVertex],
    height: f64,
    with_sides: bool,
) -> PrismFaces {
    let at =
        |vertex: &FilletVertex, z: f64| world.transform_point3(DVec3::new(vertex.x, vertex.y, z));
    let bottom: Vec<DVec3> = ring.iter().map(|vertex| at(vertex, 0.0)).collect();
    let top: Vec<DVec3> = ring.iter().map(|vertex| at(vertex, height)).collect();
    let extrusion = world.transform_vector3(DVec3::Z);
    let radii: Vec<f64> = if ring.iter().any(|vertex| vertex.frad != 0.0) {
        ring.iter().map(|vertex| vertex.frad).collect()
    } else {
        Vec::new()
    };
    if !with_sides {
        let upper_first = mean_z(&top) >= mean_z(&bottom);
        let top_face = civil_face(&top, Some(extrusion), radii.clone());
        let bottom_face = civil_face(&bottom, Some(-extrusion), radii);
        let faces = if upper_first {
            vec![top_face, bottom_face]
        } else {
            vec![bottom_face, top_face]
        };
        return PrismFaces {
            faces,
            thickness: height,
            height: None,
        };
    }
    let count = ring.len();
    let mut drafts: Vec<(Vec<DVec3>, DVec3, Vec<f64>)> = Vec::with_capacity(count + 2);
    drafts.push((bottom.clone(), -extrusion, radii.clone()));
    drafts.push((top.clone(), extrusion, radii));
    for index in 0..count {
        let next = (index + 1) % count;
        let edge = bottom[next] - bottom[index];
        if edge.length() < SIDE_EDGE_MIN_MM || height < SIDE_EDGE_MIN_MM {
            continue;
        }
        drafts.push((
            vec![bottom[index], bottom[next], top[next], top[index]],
            edge.cross(extrusion),
            Vec::new(),
        ));
    }
    // 离水平不到 20° 的面面积按 0（markCivil L816-837）；稳定排序，面积相同保留原序。
    let sort_area = |points: &[DVec3]| {
        let normal = newell(points);
        if is_horizontal(normal) {
            0.0
        } else {
            normal.length() * 0.5
        }
    };
    drafts.sort_by(|a, b| sort_area(&b.0).total_cmp(&sort_area(&a.0)));
    let thickness = plane_distance(&drafts[0].0, drafts[0].1, drafts[1].0[0]);
    let levels: Vec<f64> = drafts
        .iter()
        .filter(|(points, ..)| is_horizontal(newell(points)))
        .map(|(points, ..)| mean_z(points))
        .collect();
    let span = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - levels.iter().copied().fold(f64::INFINITY, f64::min);
    let wall_height = (levels.len() >= 2 && span > LENGTH_EPS).then(|| {
        if (span - height).abs() < THICKNESS_SNAP {
            height
        } else {
            span
        }
    });
    PrismFaces {
        faces: drafts
            .into_iter()
            .map(|(points, normal, radii)| civil_face(&points, Some(normal), radii))
            .collect(),
        thickness,
        height: wall_height,
    }
}

fn civil_face(points: &[DVec3], normal: Option<DVec3>, radii: Vec<f64>) -> CivilFace {
    CivilFace {
        vertices: points.iter().map(|point| point.to_array()).collect(),
        normal: normal
            .and_then(DVec3::try_normalize)
            .map(|normal| normal.to_array()),
        radii,
    }
}

/// 多边形的 Newell 向量：方向是法向，长度是面积的两倍。
fn newell(points: &[DVec3]) -> DVec3 {
    let count = points.len();
    (0..count)
        .map(|index| points[index].cross(points[(index + 1) % count]))
        .sum()
}

/// 法向离竖直不到 20°（markCivil `min(angle, 180 − angle) lt 20`）。
fn is_horizontal(normal: DVec3) -> bool {
    normal
        .try_normalize()
        .is_some_and(|normal| normal.z.abs().min(1.0).acos().to_degrees() < HORIZONTAL_FACE_DEG)
}

fn mean_z(points: &[DVec3]) -> f64 {
    points.iter().map(|point| point.z).sum::<f64>() / points.len().max(1) as f64
}

/// `point` 到 `face` 所在平面的距离（平面法向取顶点的 Newell 向量，退化时用 `fallback`）。
fn plane_distance(face: &[DVec3], fallback: DVec3, point: DVec3) -> f64 {
    let Some(normal) = newell(face)
        .try_normalize()
        .or_else(|| fallback.try_normalize())
    else {
        return 0.0;
    };
    (point - face[0]).dot(normal).abs()
}

// ---- 收候选 --------------------------------------------------------------------------------------

/// 从基准点出发的五条走廊（世界轴对齐盒）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayCorridors {
    /// E / N / W / S（`plant_mbd::SearchDir::ALL` 的前四个）。
    pub horizontal: [Aabb; 4],
    /// D。
    pub down: Aabb,
}

/// 一个包围盒碰到了哪类走廊。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CorridorHit {
    pub horizontal: bool,
    pub down: bool,
}

impl CorridorHit {
    pub fn any(&self) -> bool {
        self.horizontal || self.down
    }

    /// 这类构件在碰到的走廊里收不收（`getdimdata` L598-603：向下只收 FLOOR，水平收三种墙）。
    pub fn collects(&self, noun: &str) -> bool {
        match noun {
            "FLOOR" => self.down,
            "STWALL" | "GWALL" | "WALL" => self.horizontal,
            _ => false,
        }
    }
}

/// 五条走廊：每条从基准点身后半宽处起，沿射线伸 `length`，横截面 ±`half_width`。
pub fn ray_corridors(base: DVec3, length: f64, half_width: f64) -> RayCorridors {
    let w = half_width;
    let span = |lo: DVec3, hi: DVec3| Aabb::new(base + lo, base + hi);
    RayCorridors {
        horizontal: [
            span(DVec3::new(-w, -w, -w), DVec3::new(length, w, w)),
            span(DVec3::new(-w, -w, -w), DVec3::new(w, length, w)),
            span(DVec3::new(-length, -w, -w), DVec3::new(w, w, w)),
            span(DVec3::new(-w, -length, -w), DVec3::new(w, w, w)),
        ],
        down: span(DVec3::new(-w, -w, -length), DVec3::new(w, w, w)),
    }
}

impl RayCorridors {
    pub fn hit(&self, aabb: &Aabb) -> CorridorHit {
        CorridorHit {
            horizontal: self
                .horizontal
                .iter()
                .any(|corridor| corridor.intersects(aabb)),
            down: self.down.intersects(aabb),
        }
    }
}

/// 候选的世界包围盒，与求解器 `locate::prepare` 同一算法：有面取全部面顶点；曲墙取两道弧的端点与扫角内朝世界
/// E / N / W / S 的极点，各沿朝上的轴拉高墙高。几何读不懂时 `None`。
pub fn candidate_aabb(candidate: &CivilCandidate) -> Option<Aabb> {
    let mut points: Vec<DVec3> = candidate
        .faces
        .iter()
        .flat_map(|face| face.vertices.iter().copied().map(DVec3::from_array))
        .collect();
    if points.is_empty() {
        let curved = candidate.curved.as_ref()?;
        let height = candidate.height.filter(|height| *height > 0.0)?;
        let centre = DVec3::from_array(curved.center);
        let axis = DVec3::from_array(curved.axis).try_normalize()?;
        let raw_x = DVec3::from_array(curved.x_axis);
        let x_axis = (raw_x - axis * raw_x.dot(axis)).try_normalize()?;
        let y_axis = axis.cross(x_axis);
        let up = if axis.z >= 0.0 { axis } else { -axis };
        let sweep = curved.sweep_deg.to_radians();
        let mut angles = vec![0.0, sweep];
        for world in [DVec3::X, DVec3::Y, -DVec3::X, -DVec3::Y] {
            let angle = world
                .dot(y_axis)
                .atan2(world.dot(x_axis))
                .rem_euclid(std::f64::consts::TAU);
            if angle <= sweep {
                angles.push(angle);
            }
        }
        for radius in [curved.inner_radius, curved.outer_radius] {
            for angle in &angles {
                let point = centre + (x_axis * angle.cos() + y_axis * angle.sin()) * radius;
                points.push(point);
                points.push(point + up * height);
            }
        }
    }
    let (first, rest) = points.split_first()?;
    let aabb = rest.iter().fold(Aabb::new(*first, *first), |aabb, point| {
        Aabb::new(aabb.min.min(*point), aabb.max.max(*point))
    });
    (aabb.min.is_finite() && aabb.max.is_finite()).then_some(aabb)
}

/// [`locate_candidates`] 的结果。
#[derive(Debug, Clone, Default)]
pub struct LocateCandidates {
    /// 按库号、再按 refno。
    pub candidates: Vec<CivilCandidate>,
    /// 读不动的库、转不成的候选、退回 DESP 的截面…逐条一句话（进视图的 `issues`）。
    pub issues: Vec<String>,
    /// 读到了 SPAMAP 的设计库，库号升序。
    pub searched_dbs: Vec<u32>,
    /// SPAMAP 包围盒碰到走廊的墙板数（重测之前）。
    pub spamap_hits: usize,
}

/// `DbSet` 里登记了的 DESI 库，库号升序。
pub fn design_dbs(set: &DbSet) -> Vec<u32> {
    let mut dbs: Vec<u32> = set
        .db_numbers()
        .into_iter()
        .filter(|dbnum| {
            set.db_type(*dbnum)
                .is_some_and(|db_type| db_type.eq_ignore_ascii_case("DESI"))
        })
        .collect();
    dbs.sort_unstable();
    dbs
}

/// 沿基准点 `base`（世界 mm）的五条射线收找墙候选，见模块文档「收候选」。只有读库出错才 `Err`。
pub fn locate_candidates(
    set: &Arc<DbSet>,
    base: DVec3,
) -> Result<LocateCandidates, DbElementError> {
    let corridors = ray_corridors(base, LOCATE_RAY_MM, LOCATE_CORRIDOR_HALF_WIDTH_MM);
    let mut found = LocateCandidates::default();
    let mut hits = Vec::new();
    for dbnum in design_dbs(set) {
        let index = match host::spamap_index(set, dbnum) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(HostError::Db(error)) => return Err(error),
            Err(error) => {
                found
                    .issues
                    .push(format!("{error}：库 {dbnum} 里的墙板没参与找墙"));
                continue;
            }
        };
        found.searched_dbs.push(dbnum);
        for (refno, aabb) in index.iter() {
            let hit = corridors.hit(&aabb);
            if !hit.any() {
                continue;
            }
            if let Some(noun) = soft(noun_of(&set.element(refno)))?
                && hit.collects(&noun)
            {
                hits.push(refno);
            }
        }
    }
    found.spamap_hits = hits.len();
    for refno in hits {
        match civil_candidate_of(&set.element(refno), &mut found.issues) {
            Ok(candidate) => {
                let misses = candidate_aabb(&candidate)
                    .is_some_and(|aabb| !corridors.hit(&aabb).collects(&candidate.noun));
                if !misses {
                    found.candidates.push(candidate);
                }
            }
            Err(error) => found
                .issues
                .push(format!("找墙候选 {} 跳过：{error}", wire_refno(refno))),
        }
    }
    Ok(found)
}
