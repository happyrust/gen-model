//! 土建 / 建筑视图（plant3d-web#103 M6 宿主侧）：一块墙板 → `plant_mbd::CivilQueryResult`。字段与 PML 口径见 plant-mbd
//! `docs/field-dictionary.md`「土建 / 建筑视图」与 `civil/view.rs` 的模块注释：墙板给**面**，孔洞 / 埋件给**已求值的
//! 棱线集**，板上的墙算不算、棱线与墙面求交都在求解器。PML 事实源 `object/markCivil/markCivil.pmlobj`（`getatts`
//! L345-409、`getDataOfCivil` L431-1043、`getDataOfMem` L1338-1439、`getonwallindices` L1823、`getrooms` L2092-2135）、
//! `civilMem.pmlobj`、`floorRoom.pmlobj`、`vertPoints.pmlobj`、`function/civil/getabakt.pmlfnc`。坐标一律世界 mm。
//!
//! # 墙板（`element`；楼板旁的墙 `walls[].element` 同一套）
//!
//! - 面、厚、高走找墙候选那套面构造（[`civil_shape_of`]）：STWALL 两张大面（截面链 + 两端切面，就是画出来的那堵墙；
//!   目录设计表有 `HAUT` 时顶边从起端斜到终端，另给 `height_end`），FLOOR 上下两面，GWALL 拉伸体的全部面，`height`
//!   取环高 `HEIG`。弧形 SPINE 的 WALL 不给面，给 SPINE 点（起点、各段弧中点、终点）、`cbow`（截面底边中点在起点处）
//!   与截面宽 / 高，求解器照 L525-617 建两道弧；直线 SPINE 的 WALL 给两张大面（两端按 SPINE 的 DRNS / DRNE 斜切），
//!   求解器照 STWALL 的面路径建（PML 在 L544 报错停画，plant-mbd 裁决 D-CIVIL-022）。多段 SPINE 只交得出最长的那段，
//!   留一句话。
//! - 轴向：STWALL 取截面系（x 厚度向、y 高度向、z 沿轴，即 PML `ori of STWALL wrt/*`），其余取元素的世界矩阵。
//!   STWALL 另给 POSS / POSE（JUSL 线两端，JUSL 为 NA 时就是 `ppli na prop 0 / 1`）；给了面时求解器不用它们。
//! - 属主 FRMW 的 NAME、PURP、`:JJ_QTHD` / `:JJ_QDDJ` / `:ZD_NBBM` / `:ROOM_NO`（字典里没有的 UDA 读成没有——AMS
//!   就是这样），SITE 的 NAME。
//!
//! # 构件（`members`，库内顺序）
//!
//! 土建：墙板名下（任意深度，文件原序，PML `coll all … for`）的 NBOX / NXTR / NPYR / NCYL / NRTO / PFIT / FITT /
//! SBFI / FIXING，后面接 AB / AK / AT 框架下的 PANE（[`abakt_names`]）；建筑：只给 NXTR（建筑只标楼板，楼板只要
//! 排水沟，L1358-1363）。每件的棱线集：
//!
//! - NBOX：XLEN / YLEN / ZLEN 居中的 8 个角点、PML `getBoxTwelveLinesWithPoss` 的 12 条棱。
//! - NPYR：顶面 XTOP × YTOP 在 P1（+XOFF/2, +YOFF/2, +HEIG/2）、底面 XBOT × YBOT 在 P2（取负），12 条棱
//!   （`getNpyrEightPoss`；e3d-model `pyramid_solid` 同一约定）。
//! - NCYL：局部 Z 上 ±HEIG/2 的轴线，`diameter` = DIAM（`getLines` L956-962）。
//! - NXTR：第一个环（`vertPoints`：PLOO 认 SJUS，顶点按 `mergePointsRadiuss` 规整）。有倒圆时只给第一段弧的圆心
//!   沿拉伸向的一条线、`diameter` = 2R，否则每个顶点一条竖棱（`getNxtrLines`）。
//! - NRTO：上下各两道弧（按不超过 7.5° 折线）、弧上各分点的竖棱、两端的径向棱。PML 的 `getShape` 不认 NRTO，
//!   求解器有棱线就照常标。
//! - PFIT / FITT / SBFI / FIXING：目录件按实例求值，取 PML `getCataData` 挑的那几只——GMRE 下（任意深度）的 NLCY，
//!   没有就 NSBO / NSEX，再没有换 NGMR 照找。NLCY 给 PAXI 上 PBDI → PTDI 的轴线、`diameter` = 最大 PDIA；NSBO 给
//!   12 条棱（PX / PY / PZ 居中，轴同构件）；NSEX 给每个 SVER 的竖棱（PAAX / PBAX 标架、位置 = 标架 ·（PX, PY, PZ）、
//!   沿标架 +Z 拉 PHEI——e3d-model `evaluate_sext` 的口径，即 E3D 的几何：Core3D 的 SEXT 与 NSEX 走同一支 `QSSEXT`，
//!   AMS 的 `/B_600x300` 埋板正体 SEXT 与墙上凹槽 NSEX 同参数，按 +Z 才重合。PML 从构件原点按构件轴偏 PX / PY / PZ、
//!   沿 PAAX × PBAX 拉 −PHEI，不照搬，plant-mbd 裁决 D-CIVIL-023）。公式里的 `ATTRIB ODES[n]` / `OPAR[n]`
//!   在构件挂在 STWALL 上时由宿主答（STWALL 的 DESP / 目录 PARA）：e3d-io 照 E3D 的目录导出对墙上门不答
//!   （GATPAR 202），而门洞那只 NSBO 的深度正是 `ODES[1]`（墙厚）。构件标架：PFIT 贴在属主板 SJUS 指的那面（e3d-model
//!   `panel_fitting_world`，E3D TTY 在 ams1112 上 199/199 对上；PML 用 `pos of PFIT wrt/*`，差 POS.z 与板面之差，不照搬，
//!   plant-mbd 裁决 D-CIVIL-024），FIXING 取 e3d-model 建目录实例
//!   的标架（墙上 FIXING 的宿主标架只在那里），其余 `world_matrix`（已含 STWALL 上 FITT、CMFI 名下 SBFI 的现算标架）。
//!   没有目录链（SPRE 空、指到 TEXT）的不给棱线，求解器报 `civil.member_shape_missing`（PML 静默丢掉）。
//! - PANE：两张板面（`vertPoints(pane, true)` 的 points1 / points2）。
//!
//! # 楼板：附近的墙与房间
//!
//! - 墙（PML `coll all (wall gwall stwall) within volume 楼板 5`）：楼板的 SPAMAP 盒（E3D 的 `volume`）各向外扩
//!   5 mm，在 `DbSet` 登记的全部 DESI 库的 SPAMAP 里收 WALL / GWALL / STWALL（按库号、refno），再拿墙自己的几何
//!   包围盒复测一遍（求解器 `getonwallindices` 的第一条判据就是它，曲墙的 SPAMAP 盒松得多）。墙在不在这块板上
//!   （标高、墙底端点）由求解器判。建筑另给每面墙上的门：名下的 FITT 按 [`DoorRule`]（`DbOption.toml` 的
//!   `[mbd_civil]`）判——AMS 的门没有 PURP = DOOR，缺省认 SPRE 所在 SPEC 为 `/DOOR-SPEC` 或名字合 `-M\d+`；判成门的
//!   `purpose` 交 `DOOR`（求解器只认它）。
//! - 房间（`getrooms`）：楼板盒的顶面再抬 1000 mm，收属主是 SBFR 的 PANE，按所在 FRMW 归并；FRMW 名含 `-ARCH-` /
//!   `-RM` / `-R-` 且名下有 SBFR 才算（`floorRoom` L45-52），FRMW 名下每块 PANE 给中心较低的那张面（`getdrawpoints`）。
//!
//! # 问题
//!
//! 墙板读不出面、单个构件 / 墙 / 房间读不成、SPAMAP 读不动……都降级成 `issues` 里的一句话（求解器逐条转成
//! `data.view_issue`），不让整个对象失败；只有归属对象本身不在所钉会话里是 [`CivilQueryError::NotFound`]。

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::record::axis_spec;
use e3d_io::record::catalogue_eval::{AttribRef, ParamEnv};
use e3d_io::record::catalogue_expr::ParamArray;
use e3d_io::record::descriptor::DescriptorValue;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::world_with_sjus;
use e3d_model::profile::{ProfileError, loop_extent, loop_profile, sjus_drop};
use e3d_model::transform::{local_matrix, world_matrix};
use e3d_model::{CataloguePoint, catalogue_point_set, model_catalogue_member};
use glam::{DMat3, DMat4, DVec2, DVec3};
use plant_mbd::geom::{ProfileSegment, profile_from_vertices, regularize_vertices};
use plant_mbd::{
    AnnotationType, CivilElement, CivilFace, CivilMember, CivilQueryResult, CivilRoom,
    CivilSegment, CivilWall,
};
use regex::Regex;

use super::civil_faces::{
    CIVIL_CANDIDATE_NOUNS, CivilFaceError, CivilShape, civil_shape_of, design_dbs, main_piece,
    merged_ring, shape_aabb, tidy, uda_number,
};
use super::host::{self, Aabb, noun_of, soft};
use super::wire_refno;
use crate::options::MbdCivilConfig;

/// 土建取的构件类型（`getDataOfMem` L1365）；AB / AK / AT 框架下的 PANE 接在后面。
pub const CIVIL_MEMBER_NOUNS: [&str; 9] = [
    "NBOX", "NXTR", "NPYR", "NCYL", "NRTO", "PFIT", "FITT", "SBFI", "FIXING",
];
/// 建筑只要排水沟（L1360）。
pub const ARCH_MEMBER_NOUNS: [&str; 1] = ["NXTR"];
/// 楼板找墙的类型（L1823）。
pub const FLOOR_WALL_NOUNS: [&str; 3] = ["WALL", "GWALL", "STWALL"];
/// `within volume 楼板 5`（L1823）。
pub const FLOOR_WALL_PAD_MM: f64 = 5.0;
/// 房间：楼板体积的顶面再抬这么高（`getrooms` L2093-2102）。
pub const ROOM_RISE_MM: f64 = 1000.0;
/// 房间 FRMW 名的关键字（`floorRoom` L45）。
pub const ROOM_FRMW_KEYWORDS: [&str; 3] = ["-ARCH-", "-RM", "-R-"];
/// 找 AB / AK / AT 框架时从 ZONE 往下看几层（ZONE → STRU → FRMW → CWALL / CFLOOR）。
const ABAKT_SEARCH_DEPTH: usize = 3;
/// 往下只钻这几类容器（框架是 FRMW，旧规则的名字落在 CWALL / CFLOOR 上）。
const ABAKT_CONTAINERS: [&str; 3] = ["STRU", "FRMW", "SBFR"];
/// NRTO 弧边折线的最大角步。
const NRTO_STEP_DEG: f64 = 7.5;
/// 子树深度上限：墙板名下实测个位数层，64 只可能是环。
const MAX_DEPTH: usize = 64;
/// 墙上配件的「属主截面」最多往上找几级（FITT → STWALL 一级，CMFI 名下 SBFI 两级）。
const OWNER_SECTION_HOPS: usize = 3;
const LENGTH_EPS: f64 = 1e-6;

/// 视图按哪种窗体取数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CivilMode {
    /// 土建（`markcivilform`）：构件全给，楼板旁的墙不带门。
    Civil,
    /// 建筑（`markarchform`）：构件只给 NXTR，楼板旁的墙带门。
    Arch,
}

impl CivilMode {
    pub fn of(annotation_type: AnnotationType) -> Self {
        if annotation_type == AnnotationType::Arch {
            Self::Arch
        } else {
            Self::Civil
        }
    }
}

#[derive(Debug)]
pub enum CivilQueryError {
    /// refno 在所钉会话里不存在。
    NotFound {
        refno: RefNo,
    },
    Db(DbElementError),
}

impl std::fmt::Display for CivilQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CivilQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::NotFound { .. } => None,
        }
    }
}

impl From<DbElementError> for CivilQueryError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

// ---- 门规则 --------------------------------------------------------------------------------------

/// 建筑楼板上「哪些 FITT 是门」（`[mbd_civil]`，[`MbdCivilConfig`]）：PURP = DOOR，或 SPRE 所在 SPEC 的 NAME 在
/// `specs` 里（不分大小写），或 NAME 合 `name_patterns` 里任一正则。
#[derive(Debug, Clone)]
pub struct DoorRule {
    pub specs: Vec<String>,
    pub name_patterns: Vec<Regex>,
}

impl DoorRule {
    /// 缺省规则（[`MbdCivilConfig::default`]）。
    pub fn defaults() -> Self {
        Self::from_config(&MbdCivilConfig::default()).0
    }

    /// 编译一份配置：写错的正则跳过并各留一句话。
    pub fn from_config(config: &MbdCivilConfig) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let specs = config
            .door_specs
            .iter()
            .map(|spec| spec.trim().to_string())
            .filter(|spec| !spec.is_empty())
            .collect();
        let name_patterns = config
            .door_name_patterns
            .iter()
            .map(|pattern| pattern.trim())
            .filter(|pattern| !pattern.is_empty())
            .filter_map(|pattern| match Regex::new(pattern) {
                Ok(regex) => Some(regex),
                Err(error) => {
                    warnings.push(format!(
                        "door_name_patterns 的 {pattern:?} 不是正则（{error}），跳过"
                    ));
                    None
                }
            })
            .collect();
        (
            Self {
                specs,
                name_patterns,
            },
            warnings,
        )
    }

    /// 本进程的门规则：第一次用到时按 `DbOption.toml` 编译一次，跳过的正则在 stderr 点名。
    pub fn current() -> Arc<Self> {
        static CURRENT: OnceLock<Arc<DoorRule>> = OnceLock::new();
        CURRENT
            .get_or_init(|| {
                let (rule, warnings) = Self::from_config(&crate::options::mbd_civil());
                for warning in warnings {
                    eprintln!("⚠ [mbd_civil] {warning}");
                }
                Arc::new(rule)
            })
            .clone()
    }

    pub fn is_door(&self, fitt: &DbElement) -> Result<bool, DbElementError> {
        if host::optional_text(fitt, "PURP")?
            .is_some_and(|purpose| purpose.eq_ignore_ascii_case("DOOR"))
        {
            return Ok(true);
        }
        if !self.specs.is_empty()
            && let Some(spre) = soft(host::optional_element(fitt, "SPRE"))?.flatten()
            && let Some(spec) = soft(host::ancestor_name(&spre, "SPEC"))?.flatten()
            && self
                .specs
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(spec.trim()))
        {
            return Ok(true);
        }
        Ok(soft(fitt.stored_name())?.flatten().is_some_and(|name| {
            self.name_patterns
                .iter()
                .any(|pattern| pattern.is_match(&name))
        }))
    }
}

// ---- 入口 ----------------------------------------------------------------------------------------

/// 一块墙板的土建视图（土建窗体的取数），门规则取 [`DoorRule::current`]。见模块文档。
pub fn query_civil(
    set: &Arc<DbSet>,
    subject_refno: RefNo,
) -> Result<CivilQueryResult, CivilQueryError> {
    query_civil_with(set, subject_refno, CivilMode::Civil, &DoorRule::current())
}

/// [`query_civil`]，窗体与门规则由调用方给。
pub fn query_civil_with(
    set: &Arc<DbSet>,
    subject_refno: RefNo,
    mode: CivilMode,
    doors: &DoorRule,
) -> Result<CivilQueryResult, CivilQueryError> {
    let subject = set.element(subject_refno);
    match subject.exists() {
        Ok(true) => {}
        Ok(false)
        | Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            return Err(CivilQueryError::NotFound {
                refno: subject_refno,
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut view = CivilQueryResult {
        subject_refno: wire_refno(subject_refno),
        subject_noun: noun_of(&subject)?,
        name: soft(subject.stored_name())?.flatten(),
        ..CivilQueryResult::default()
    };
    let label = format!("{} {}", view.subject_noun, view.subject_refno);
    if !CIVIL_CANDIDATE_NOUNS.contains(&view.subject_noun.as_str()) {
        view.issues.push(format!(
            "{label} 不是 STWALL / GWALL / WALL / FLOOR，没有墙板几何"
        ));
        return Ok(view);
    }
    let mut notes = Vec::new();
    let shape = match element_view(&subject, &mut notes) {
        Ok((element, shape)) => {
            view.element = element;
            shape
        }
        Err(error) => {
            notes.push(format!("{label} 的墙板读不成：{error}"));
            None
        }
    };
    view.issues.append(&mut notes);
    view.members = members_of(&subject, mode, &mut view.issues);
    if view.subject_noun == "FLOOR" {
        match floor_volume(set, &subject, shape.as_ref()) {
            Some(volume) => {
                view.walls = floor_walls(set, volume, mode, doors, &mut view.issues);
                view.rooms = floor_rooms(set, volume, &mut view.issues);
            }
            None => view.issues.push(format!(
                "{label} 在 SPAMAP 里没有包围盒、面也读不出，附近的墙与房间没找"
            )),
        }
    }
    Ok(view)
}

// ---- 墙板 ----------------------------------------------------------------------------------------

/// 一块墙板的 [`CivilElement`] 与它的几何。几何读不出时只有名字与 UDA，原因进 `notes`（求解器再报
/// `civil.geometry_missing`）。
fn element_view(
    element: &DbElement,
    notes: &mut Vec<String>,
) -> Result<(CivilElement, Option<CivilShape>), DbElementError> {
    let mut view = CivilElement::default();
    if let Some(frmw) = host::ancestor_of_type(element, "FRMW")? {
        view.frmw_name = soft(frmw.stored_name())?.flatten();
        view.thickness_uda = uda_number(&frmw, ":JJ_QTHD")?;
        view.strength_class = uda_text(&frmw, ":JJ_QDDJ")?;
        view.purpose = host::optional_text(&frmw, "PURP")?;
        view.drawing_number = uda_text(&frmw, ":ZD_NBBM")?;
        view.room_number = uda_text(&frmw, ":ROOM_NO")?;
    }
    view.site_name = host::site_name(element)?;
    let shape = match civil_shape_of(element, notes) {
        Ok(shape) => shape,
        Err(CivilFaceError::Db(error)) => return Err(error),
        Err(error) => {
            notes.push(error.to_string());
            return Ok((view, None));
        }
    };
    let axes = match shape.section_frame {
        Some(frame) => Some(DMat3::from_mat4(frame)),
        None => host::world_rotation(element)?,
    };
    if let Some(axes) = axes {
        view.x_axis = unit(axes.x_axis);
        view.y_axis = unit(axes.y_axis);
        view.z_axis = unit(axes.z_axis);
    }
    view.faces = shape.faces.clone();
    match shape.noun.as_str() {
        "STWALL" => {
            view.thickness = Some(tidy(shape.thickness));
            view.height = shape.height.map(tidy);
            view.height_end = shape.height_end.map(tidy);
            view.axis_start = host::world_point(element, "POSS")?.map(|point| point.to_array());
            view.axis_end = host::world_point(element, "POSE")?.map(|point| point.to_array());
        }
        "WALL" => {
            view.thickness = Some(tidy(shape.thickness));
            view.height = shape.height.map(tidy);
            view.spine = shape.spine.iter().map(|point| point.to_array()).collect();
            view.cbow = shape.cbow.map(|point| point.to_array());
            if shape.pieces.len() > 1 {
                let kinds: Vec<&str> = shape
                    .pieces
                    .iter()
                    .map(|piece| {
                        if piece.curved.is_some() {
                            "弧"
                        } else {
                            "直段"
                        }
                    })
                    .collect();
                notes.push(format!(
                    "WALL {} 的 SPINE 有 {} 段（{}）：只把最长的第 {} 段交给求解器建墙，别的段上的构件找不到墙面",
                    wire_refno(element.refno()),
                    kinds.len(),
                    kinds.join(" / "),
                    main_piece(&shape.pieces) + 1
                ));
            }
        }
        _ => view.height = shape.loop_height.map(tidy),
    }
    Ok((view, Some(shape)))
}

fn unit(axis: DVec3) -> Option<[f64; 3]> {
    axis.try_normalize().map(|axis| axis.to_array())
}

/// 文本 UDA：字典里没有 / 对这个 noun 无效按没有；没设的读回字典缺省 `UNSET`，PML 拿它比 `'UNSET'`（`getJgo`
/// L589、`getowneratt` L615），同样按没有；存成数的按数写。
fn uda_text(element: &DbElement, name: &str) -> Result<Option<String>, DbElementError> {
    match element.get_string(name) {
        Ok(value) => Ok(value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("UNSET"))),
        Err(DbElementError::UnknownAttribute { .. } | DbElementError::Uda { .. }) => Ok(None),
        Err(DbElementError::TypeMismatch { .. }) => {
            Ok(uda_number(element, name)?.map(|value| value.to_string()))
        }
        Err(error) => Err(error),
    }
}

// ---- 构件 ----------------------------------------------------------------------------------------

/// `members`：按窗体挑类型（见模块文档），库内顺序；读不成的构件照样占位（求解器报没有形状），原因进 `issues`。
fn members_of(subject: &DbElement, mode: CivilMode, issues: &mut Vec<String>) -> Vec<CivilMember> {
    let nouns: &[&str] = match mode {
        CivilMode::Civil => &CIVIL_MEMBER_NOUNS,
        CivilMode::Arch => &ARCH_MEMBER_NOUNS,
    };
    let mut found = Vec::new();
    if let Err(error) = descendants_of(subject, nouns, &mut found, 0) {
        issues.push(format!(
            "{} 名下的构件没收全：{error}",
            wire_refno(subject.refno())
        ));
    }
    if mode == CivilMode::Civil {
        match abakt_panes(subject) {
            Ok(panes) => found.extend(panes),
            Err(error) => issues.push(format!(
                "{} 的 AB / AK / AT 框架没找成：{error}",
                wire_refno(subject.refno())
            )),
        }
    }
    found
        .iter()
        .map(|member| member_view(member, false, issues))
        .collect()
}

/// `owner` 子树里（深度优先、文件原序）类型在 `nouns` 里的元素。
fn descendants_of(
    owner: &DbElement,
    nouns: &[&str],
    out: &mut Vec<DbElement>,
    depth: usize,
) -> Result<(), DbElementError> {
    if depth >= MAX_DEPTH {
        return Err(DbElementError::OwnerCycle {
            start: owner.refno(),
            at: owner.refno(),
        });
    }
    for member in owner.members()? {
        if soft(noun_of(&member))?.is_some_and(|noun| nouns.contains(&noun.as_str())) {
            out.push(member.clone());
        }
        descendants_of(&member, nouns, out, depth + 1)?;
    }
    Ok(())
}

/// PML `!!getabakt`：新规则 `FRMW 名 + -AB / -AK / -AT`；旧规则把 CWALL（楼板是 CFLOOR，没有就属主）名字按 `-`
/// 切开、倒数第二段换成 AB / AK / T。去重，保留先后。
pub fn abakt_names(subject: &DbElement) -> Result<Vec<String>, DbElementError> {
    let mut names = Vec::new();
    if let Some(frmw) = host::ancestor_of_type(subject, "FRMW")?
        && let Some(name) = soft(frmw.stored_name())?.flatten()
    {
        for suffix in ["-AB", "-AK", "-AT"] {
            names.push(format!("{name}{suffix}"));
        }
    }
    let owner = if noun_of(subject)? == "FLOOR" {
        match host::ancestor_of_type(subject, "CFLOOR")? {
            Some(cfloor) => Some(cfloor),
            None => soft(subject.owner())?.flatten(),
        }
    } else {
        host::ancestor_of_type(subject, "CWALL")?
    };
    if let Some(owner) = owner
        && let Some(owner_name) = soft(owner.stored_name())?.flatten()
    {
        let parts: Vec<&str> = owner_name.split('-').collect();
        if parts.len() >= 2 {
            for replacement in ["AB", "AK", "T"] {
                let mut renamed = parts.clone();
                let at = renamed.len() - 2;
                renamed[at] = replacement;
                names.push(renamed.join("-"));
            }
        }
    }
    let mut seen = HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    Ok(names)
}

/// AB / AK / AT 框架下的全部 PANE（`coll all pane for $!abakt`，按 [`abakt_names`] 的先后）。PML 按名字全库查；
/// 这里在墙板所在 ZONE 里往下看三层容器（框架是 FRMW，旧规则的名字落在 CWALL / CFLOOR 上），不做全库扫——
/// 7997 一次按名全库扫要近 1 s。
fn abakt_panes(subject: &DbElement) -> Result<Vec<DbElement>, DbElementError> {
    let names = abakt_names(subject)?;
    let Some(zone) = host::ancestor_of_type(subject, "ZONE")? else {
        return Ok(Vec::new());
    };
    let mut frames: Vec<Option<DbElement>> = vec![None; names.len()];
    find_named_below(&zone, &names, &mut frames, 1)?;
    let mut panes = Vec::new();
    for frame in frames.into_iter().flatten() {
        descendants_of(&frame, &["PANE"], &mut panes, 0)?;
    }
    Ok(panes)
}

fn find_named_below(
    owner: &DbElement,
    names: &[String],
    found: &mut [Option<DbElement>],
    depth: usize,
) -> Result<(), DbElementError> {
    for member in owner.members()? {
        if let Some(name) = soft(member.stored_name())?.flatten()
            && let Some(index) = names.iter().position(|wanted| *wanted == name)
            && found[index].is_none()
        {
            found[index] = Some(member.clone());
        }
        if depth < ABAKT_SEARCH_DEPTH
            && soft(noun_of(&member))?.is_some_and(|noun| ABAKT_CONTAINERS.contains(&noun.as_str()))
        {
            find_named_below(&member, names, found, depth + 1)?;
        }
    }
    Ok(())
}

/// 一个构件的视图；`door` 时 `purpose` 交 `DOOR`。读不成的部分留空，原因进 `issues`。
fn member_view(member: &DbElement, door: bool, issues: &mut Vec<String>) -> CivilMember {
    let mut view = CivilMember {
        refno: wire_refno(member.refno()),
        ..CivilMember::default()
    };
    if let Err(error) = fill_member(member, door, &mut view, issues) {
        issues.push(format!("构件 {} {} 读不成：{error}", view.noun, view.refno));
    }
    view
}

fn fill_member(
    member: &DbElement,
    door: bool,
    view: &mut CivilMember,
    issues: &mut Vec<String>,
) -> Result<(), DbElementError> {
    let noun = noun_of(member)?;
    view.noun = noun.clone();
    let stored = soft(member.stored_name())?.flatten();
    view.owner_name = if noun == "PANE" {
        match soft(member.owner())?.flatten() {
            Some(owner) => soft(owner.stored_name())?.flatten(),
            None => None,
        }
    } else if stored.is_none() {
        first_named_owner(member)?
    } else {
        None
    };
    view.name = Some(stored.unwrap_or_else(|| equals_refno(member.refno())));
    view.purpose = if door {
        Some("DOOR".to_string())
    } else {
        host::optional_text(member, "PURP")?
    };
    [view.jgobjbase, view.jgobjmat, view.jgobjnote] = jgobj(member)?;
    let label = format!("{noun} {}", view.refno);
    let shape = match noun.as_str() {
        "NBOX" => nbox_shape(member)?,
        "NPYR" => npyr_shape(member)?,
        "NCYL" => ncyl_shape(member)?,
        "NRTO" => nrto_shape(member)?,
        "NXTR" => match first_ring(member, true)? {
            Ok(ring) => nxtr_shape(&ring),
            Err(why) => {
                issues.push(format!("{label} 的环读不成：{why}"));
                MemberShape::default()
            }
        },
        "PANE" => match first_ring(member, true)? {
            Ok(ring) => MemberShape {
                plate_faces: vec![ring.face(0.0), ring.face(ring.height)],
                ..MemberShape::default()
            },
            Err(why) => {
                issues.push(format!("{label} 的环读不成：{why}"));
                MemberShape::default()
            }
        },
        "PFIT" | "FITT" | "SBFI" | "FIXING" => catalogue_shape(member, &noun, &label, issues)?,
        _ => MemberShape::default(),
    };
    view.lines = shape.lines;
    view.diameter = shape.diameter;
    view.plate_faces = shape.plate_faces;
    Ok(())
}

/// E3D 给无名元素显示的 `=w0/w1`。
fn equals_refno(refno: RefNo) -> String {
    format!("={}/{}", refno.word0, refno.word1 as i32)
}

/// 属主链上停在墙 / 板（类型名含 FLOOR 或 WALL，PML `match`）之前（PML `getowneratt` L601-659）。
fn is_civil_host(noun: &str) -> bool {
    noun.contains("FLOOR") || noun.contains("WALL")
}

/// 无名构件往上第一个有名属主，停在墙 / 板之前（`getowneratt('name')`）。
fn first_named_owner(member: &DbElement) -> Result<Option<String>, DbElementError> {
    let mut cursor = soft(member.owner())?.flatten();
    for _ in 0..MAX_DEPTH {
        let Some(current) = cursor else {
            return Ok(None);
        };
        if soft(noun_of(&current))?.is_none_or(|noun| is_civil_host(&noun)) {
            return Ok(None);
        }
        if let Some(name) = soft(current.stored_name())?.flatten() {
            return Ok(Some(name));
        }
        cursor = soft(current.owner())?.flatten();
    }
    Ok(None)
}

/// `:JGOBJBASE` / `:JGOBJMAT` / `:JGOBJNOTE`：构件自己设了 BASE 就取自己的三项，否则取往上（停在墙 / 板之前）第一个
/// 设了 BASE（且不以 `=` 开头）的属主的三项（`getJgo` L585-599 / `getowneratt` L601-628）。
fn jgobj(member: &DbElement) -> Result<[Option<String>; 3], DbElementError> {
    let triple = |element: &DbElement| -> Result<Option<[Option<String>; 3]>, DbElementError> {
        let Some(base) = uda_text(element, ":JGOBJBASE")?.filter(|base| !base.starts_with('='))
        else {
            return Ok(None);
        };
        Ok(Some([
            Some(base),
            uda_text(element, ":JGOBJMAT")?,
            uda_text(element, ":JGOBJNOTE")?,
        ]))
    };
    if let Some(own) = triple(member)? {
        return Ok(own);
    }
    let mut cursor = soft(member.owner())?.flatten();
    for _ in 0..MAX_DEPTH {
        let Some(current) = cursor else {
            break;
        };
        if soft(noun_of(&current))?.is_none_or(|noun| is_civil_host(&noun)) {
            break;
        }
        if let Some(found) = triple(&current)? {
            return Ok(found);
        }
        cursor = soft(current.owner())?.flatten();
    }
    Ok([None, None, None])
}

/// 一个构件交给求解器的形状。
#[derive(Debug, Default)]
struct MemberShape {
    lines: Vec<CivilSegment>,
    diameter: Option<f64>,
    plate_faces: Vec<CivilFace>,
}

fn segment(from: DVec3, to: DVec3) -> Option<CivilSegment> {
    (from.is_finite() && to.is_finite() && from.distance(to) > LENGTH_EPS).then(|| CivilSegment {
        from: from.to_array(),
        to: to.to_array(),
    })
}

/// 一个矩形的 4 个角：从 (+x, +y) 起，依次 −x、−y、+x（PML `getBoxEightPossWithPos` / `getNpyrEightPoss`）。
fn rect_corners(center: DVec3, half: DVec2) -> [DVec3; 4] {
    [
        center + DVec3::new(half.x, half.y, 0.0),
        center + DVec3::new(-half.x, half.y, 0.0),
        center + DVec3::new(-half.x, -half.y, 0.0),
        center + DVec3::new(half.x, -half.y, 0.0),
    ]
}

/// PML `getBoxTwelveLinesWithPoss`（L709-728）：前 4 个角成一圈、后 4 个成一圈、再对应相连。
fn box_lines(to_world: DMat4, upper: [DVec3; 4], lower: [DVec3; 4]) -> Vec<CivilSegment> {
    let poss: Vec<DVec3> = upper
        .into_iter()
        .chain(lower)
        .map(|point| to_world.transform_point3(point))
        .collect();
    let mut lines = Vec::with_capacity(12);
    for x in 0..8 {
        let next = if x == 3 || x == 7 { x - 3 } else { x + 1 };
        lines.extend(segment(poss[x], poss[next]));
    }
    for x in 0..4 {
        lines.extend(segment(poss[x], poss[x + 4]));
    }
    lines
}

fn positive(value: Option<f64>) -> Option<f64> {
    value.map(f64::abs).filter(|value| *value > LENGTH_EPS)
}

fn nbox_shape(member: &DbElement) -> Result<MemberShape, DbElementError> {
    let dimension = |name: &str| host::optional_double(member, name).map(positive);
    let (Some(x), Some(y), Some(z)) = (dimension("XLEN")?, dimension("YLEN")?, dimension("ZLEN")?)
    else {
        return Ok(MemberShape::default());
    };
    let Some(world) = host::world_matrix_soft(member)? else {
        return Ok(MemberShape::default());
    };
    let half = DVec2::new(x, y) * 0.5;
    Ok(MemberShape {
        lines: box_lines(
            world,
            rect_corners(DVec3::Z * (z * 0.5), half),
            rect_corners(DVec3::Z * (-z * 0.5), half),
        ),
        ..MemberShape::default()
    })
}

fn npyr_shape(member: &DbElement) -> Result<MemberShape, DbElementError> {
    let value = |name: &str| -> Result<f64, DbElementError> {
        Ok(host::optional_double(member, name)?.unwrap_or(0.0))
    };
    let height = value("HEIG")?;
    if height.abs() <= LENGTH_EPS {
        return Ok(MemberShape::default());
    }
    let Some(world) = host::world_matrix_soft(member)? else {
        return Ok(MemberShape::default());
    };
    let offset = DVec2::new(value("XOFF")?, value("YOFF")?) * 0.5;
    let top = DVec3::new(offset.x, offset.y, height * 0.5);
    let bottom = DVec3::new(-offset.x, -offset.y, -height * 0.5);
    let half = |x: f64, y: f64| DVec2::new(x.abs(), y.abs()) * 0.5;
    Ok(MemberShape {
        lines: box_lines(
            world,
            rect_corners(top, half(value("XTOP")?, value("YTOP")?)),
            rect_corners(bottom, half(value("XBOT")?, value("YBOT")?)),
        ),
        ..MemberShape::default()
    })
}

fn ncyl_shape(member: &DbElement) -> Result<MemberShape, DbElementError> {
    let (Some(diameter), Some(height)) = (
        positive(host::optional_double(member, "DIAM")?),
        positive(host::optional_double(member, "HEIG")?),
    ) else {
        return Ok(MemberShape::default());
    };
    let Some(world) = host::world_matrix_soft(member)? else {
        return Ok(MemberShape::default());
    };
    let end = |z: f64| world.transform_point3(DVec3::Z * z);
    Ok(MemberShape {
        lines: segment(end(-height * 0.5), end(height * 0.5))
            .into_iter()
            .collect(),
        diameter: Some(diameter),
        ..MemberShape::default()
    })
}

/// 矩形截面环（RINS..ROUT × ±HEIG/2）绕局部 Z 从 +X 扫 ANGL（负角从 −|ANGL| 扫到 0，e3d-model
/// `rect_torus_solid_with_segments` 同一约定）。
fn nrto_shape(member: &DbElement) -> Result<MemberShape, DbElementError> {
    let rins = host::optional_double(member, "RINS")?
        .unwrap_or(0.0)
        .max(0.0);
    let (Some(rout), Some(height), Some(angle)) = (
        positive(host::optional_double(member, "ROUT")?),
        positive(host::optional_double(member, "HEIG")?),
        host::optional_double(member, "ANGL")?.filter(|angle| angle.abs() > LENGTH_EPS),
    ) else {
        return Ok(MemberShape::default());
    };
    let Some(world) = host::world_matrix_soft(member)? else {
        return Ok(MemberShape::default());
    };
    let (inner, outer) = (rins.min(rout), rins.max(rout));
    let sweep = angle.clamp(-360.0, 360.0).to_radians();
    let (a0, a1) = if sweep >= 0.0 {
        (0.0, sweep)
    } else {
        (sweep, 0.0)
    };
    let steps = ((a1 - a0).to_degrees() / NRTO_STEP_DEG).ceil().max(2.0) as usize;
    let angles: Vec<f64> = (0..=steps)
        .map(|step| a0 + (a1 - a0) * step as f64 / steps as f64)
        .collect();
    let at = |radius: f64, angle: f64, z: f64| {
        world.transform_point3(DVec3::new(radius * angle.cos(), radius * angle.sin(), z))
    };
    let h2 = height * 0.5;
    let mut lines = Vec::new();
    for radius in [inner, outer] {
        if radius <= LENGTH_EPS {
            continue;
        }
        for angle in &angles {
            lines.extend(segment(at(radius, *angle, -h2), at(radius, *angle, h2)));
        }
        for z in [-h2, h2] {
            for pair in angles.windows(2) {
                lines.extend(segment(at(radius, pair[0], z), at(radius, pair[1], z)));
            }
        }
    }
    for angle in [a0, a1] {
        for z in [-h2, h2] {
            lines.extend(segment(at(inner, angle, z), at(outer, angle, z)));
        }
    }
    Ok(MemberShape {
        lines,
        ..MemberShape::default()
    })
}

/// 环拉伸元素（PANE / NXTR / 房间 PANE）的第一个环：世界标架（含 SJUS 下移，e3d-model `world_with_sjus`）、局部
/// 顶点与倒圆、环高。
struct Ring {
    world: DMat4,
    vertices: Vec<DVec2>,
    radii: Vec<f64>,
    height: f64,
}

impl Ring {
    /// 局部 `z` 处的那张面（`vertPoints` 的 points1 在 z = 0、points2 在环高处；TOP 族 SJUS 已在标架里）。
    fn face(&self, z: f64) -> CivilFace {
        CivilFace {
            vertices: self
                .vertices
                .iter()
                .map(|vertex| {
                    self.world
                        .transform_point3(DVec3::new(vertex.x, vertex.y, z))
                        .to_array()
                })
                .collect(),
            normal: None,
            radii: if self.radii.iter().any(|radius| *radius != 0.0) {
                self.radii.clone()
            } else {
                Vec::new()
            },
        }
    }
}

/// `regularize` 时顶点按 PML `mergePointsRadiuss` 规整（`vertPoints(item, true)`；规整不成就用并掉重合点的原环）。
/// 环读不成（没有环、顶点不够）给 `Ok(Err(原因))`。
fn first_ring(
    element: &DbElement,
    regularize: bool,
) -> Result<Result<Ring, String>, DbElementError> {
    let profile = match loop_profile(element) {
        Ok(profile) => profile,
        Err(ProfileError::Db(error)) => return Err(error),
        Err(error) => return Ok(Err(error.to_string())),
    };
    let world = world_with_sjus(world_matrix(element)?, profile.sjus_drop);
    let ring = merged_ring(&profile.rings[0]);
    let mut vertices: Vec<DVec2> = ring
        .iter()
        .map(|vertex| DVec2::new(vertex.x, vertex.y))
        .collect();
    let mut radii: Vec<f64> = ring.iter().map(|vertex| vertex.frad).collect();
    if regularize && let Some((regular, regular_radii)) = regularize_vertices(&vertices, &radii) {
        vertices = regular;
        radii = regular_radii;
    }
    if vertices.len() < 3 {
        return Ok(Err(format!("环只剩 {} 个顶点", vertices.len())));
    }
    Ok(Ok(Ring {
        world,
        vertices,
        radii,
        height: profile.height,
    }))
}

/// PML `getNxtrLines`（L661-683）：轮廓（`getLinesAndArcsfortwoarrays`）里有弧就取第一段弧的圆心沿拉伸向一条线、
/// 直径 2R，否则每个顶点一条竖棱。
fn nxtr_shape(ring: &Ring) -> MemberShape {
    if ring.height.abs() <= LENGTH_EPS {
        return MemberShape::default();
    }
    let at = |point: DVec2, z: f64| ring.world.transform_point3(DVec3::new(point.x, point.y, z));
    if ring.radii.iter().any(|radius| *radius != 0.0)
        && let Some(ProfileSegment::Arc { center, radius, .. }) =
            profile_from_vertices(&ring.vertices, &ring.radii)
                .into_iter()
                .find(|segment| matches!(segment, ProfileSegment::Arc { .. }))
    {
        return MemberShape {
            lines: segment(at(center, 0.0), at(center, ring.height))
                .into_iter()
                .collect(),
            diameter: Some(radius * 2.0),
            ..MemberShape::default()
        };
    }
    MemberShape {
        lines: ring
            .vertices
            .iter()
            .filter_map(|vertex| segment(at(*vertex, 0.0), at(*vertex, ring.height)))
            .collect(),
        ..MemberShape::default()
    }
}

// ---- 目录件 --------------------------------------------------------------------------------------

/// PML `getCataData` 挑出的目录图元。
enum Picked {
    /// NLCY：轴线 + 最大直径。
    Cylinders(Vec<DbElement>),
    /// NSBO / NSEX：棱线。
    Boxes(Vec<DbElement>),
}

/// GMRE（任意深度）里的 NLCY，没有就 NSBO / NSEX；GMRE 里都没有再到 NGMR 照找（`getCataData` L1397-1419）。
fn pick_primitives(component: &DbElement) -> Result<Option<Picked>, DbElementError> {
    for set_attribute in ["GMRE", "NGMR"] {
        let Some(geometry) = soft(host::optional_element(component, set_attribute))?.flatten()
        else {
            continue;
        };
        let mut cylinders = Vec::new();
        descendants_of(&geometry, &["NLCY"], &mut cylinders, 0)?;
        if !cylinders.is_empty() {
            return Ok(Some(Picked::Cylinders(cylinders)));
        }
        let mut boxes = Vec::new();
        descendants_of(&geometry, &["NSBO", "NSEX"], &mut boxes, 0)?;
        if !boxes.is_empty() {
            return Ok(Some(Picked::Boxes(boxes)));
        }
    }
    Ok(None)
}

fn catalogue_shape(
    member: &DbElement,
    noun: &str,
    label: &str,
    issues: &mut Vec<String>,
) -> Result<MemberShape, DbElementError> {
    let points = match catalogue_point_set(member) {
        Ok(points) => points,
        Err(DbElementError::DbNotRegistered { dbno }) => {
            issues.push(format!("{label} 的目录在库 {dbno}，没注册进来，没有棱线"));
            return Ok(MemberShape::default());
        }
        Err(DbElementError::NotFound { refno, .. }) => {
            issues.push(format!(
                "{label} 的目录链指到 {refno}，不在所钉会话里，没有棱线"
            ));
            return Ok(MemberShape::default());
        }
        Err(error) => return Err(error),
    };
    let Some(component) = points.component else {
        return Ok(MemberShape::default());
    };
    let component = member.db_set().element(component);
    let Some(picked) = pick_primitives(&component)? else {
        return Ok(MemberShape::default());
    };
    let Some(frame) = catalogue_frame(member, noun)? else {
        issues.push(format!("{label} 的属主链折不进世界系，没有棱线"));
        return Ok(MemberShape::default());
    };
    let env = FittingEnv::new(member, &component)?;
    let mut shape = MemberShape::default();
    let mut unresolved = Vec::new();
    let push = |shape: &mut MemberShape, from: DVec3, to: DVec3| {
        shape.lines.extend(segment(
            frame.transform_point3(from),
            frame.transform_point3(to),
        ));
    };
    match picked {
        Picked::Cylinders(cylinders) => {
            for cylinder in &cylinders {
                match cylinder_line(cylinder, &env, &points.points)? {
                    Ok((from, to, diameter)) => {
                        push(&mut shape, from, to);
                        shape.diameter = Some(shape.diameter.map_or(diameter, |d| d.max(diameter)));
                    }
                    Err(why) => unresolved.push(why),
                }
            }
        }
        Picked::Boxes(primitives) => {
            for primitive in &primitives {
                let edges = if soft(noun_of(primitive))?.as_deref() == Some("NSEX") {
                    extrusion_edges(primitive, &env, &points.points)?
                } else {
                    box_edges(primitive, &env)?
                };
                match edges {
                    Ok(edges) => {
                        for (from, to) in edges {
                            push(&mut shape, from, to);
                        }
                    }
                    Err(why) => unresolved.push(why),
                }
            }
        }
    }
    if !unresolved.is_empty() {
        issues.push(format!(
            "{label} 的目录图元求不出、跳过：{}",
            unresolved.join("；")
        ));
    }
    Ok(shape)
}

/// 目录图元在世界里的放置标架（见模块文档「构件」）。属主链折不进世界系时 `None`。
fn catalogue_frame(member: &DbElement, noun: &str) -> Result<Option<DMat4>, DbElementError> {
    match noun {
        "PFIT" => soft(panel_fitting_world(member)),
        "FIXING" => match model_catalogue_member(member) {
            Ok(build) => match build.generated {
                Some(generated) => Ok(Some(generated.world)),
                None => host::world_matrix_soft(member),
            },
            Err(_) => host::world_matrix_soft(member),
        },
        _ => host::world_matrix_soft(member),
    }
}

/// e3d-model `catalogue::panel_fitting_world`（crate 私有）的同口径：属主板标架 × `T(POS.x, POS.y, z板面)` × `R(ORI)`
/// × `Rz(BANG)`——`POS.z` 不进实例，目录几何贴在属主板上 PFIT `SJUS` 指的那一面（TOP 族顶面、CEN 族中面、其余底面）。
fn panel_fitting_world(member: &DbElement) -> Result<DMat4, DbElementError> {
    let local = local_matrix(member)?;
    let surface = match member.owner()? {
        Some(panel) => match loop_extent(&panel)? {
            Some(extent) => {
                let sjus = host::optional_text(member, "SJUS")?.unwrap_or_default();
                sjus_drop(sjus.trim(), extent.height) - extent.sjus_drop
            }
            None => 0.0,
        },
        None => 0.0,
    };
    let on_panel = DMat4::from_cols(
        local.x_axis,
        local.y_axis,
        local.z_axis,
        DVec3::new(local.w_axis.x, local.w_axis.y, surface).extend(1.0),
    );
    let bang = host::optional_double(member, "BANG")?
        .unwrap_or(0.0)
        .to_radians();
    Ok(world_matrix(member)? * local.inverse() * on_panel * DMat4::from_rotation_z(bang))
}

/// 目录求值环境：e3d-io 的实例环境，再给墙上配件补上属主截面的参数。
///
/// e3d-io 的 `OPAR` / `ODES` 照 Core3D `GTOREF` 的表只认 SCTN / GENSEC 属主，FITT 挂在 STWALL 上就答不上（GATPAR
/// 202，E3D 的目录导出因此只画门扇）。PML 标门时求的是门洞那只 NSBO，深度 `PZLE = ( ATTRIB ODES[1 ] )` 正是墙厚，
/// 所以 FITT / CMFI / SBFI 往上三级内有 STWALL 时，`ODES[n]` 取它的 DESP、`OPAR[n]` 取它（或它目录件）的 PARA。
struct FittingEnv {
    inner: DbElementParamEnv,
    owner_desp: Option<Vec<f64>>,
    owner_para: Option<Vec<f64>>,
}

impl FittingEnv {
    fn new(member: &DbElement, component: &DbElement) -> Result<Self, DbElementError> {
        let mut env = Self {
            inner: DbElementParamEnv::new(Some(member.clone()), Some(component.clone()))
                .without_insulation(),
            owner_desp: None,
            owner_para: None,
        };
        if matches!(noun_of(member)?.as_str(), "FITT" | "CMFI" | "SBFI")
            && let Some(owner) = stwall_owner(member)?
        {
            env.owner_desp = host::optional_doubles(&owner, "DESP")?;
            // 截面目录链走不通只是这一格答不上，不连累构件。
            env.owner_para = host::optional_doubles(&owner, "PARA")
                .ok()
                .flatten()
                .or_else(|| {
                    host::CatalogueEval::of(&owner)
                        .ok()
                        .flatten()
                        .and_then(|eval| host::optional_doubles(eval.component(), "PARA").ok())
                        .flatten()
                });
        }
        Ok(env)
    }

    fn owner_parameter(&self, array: ParamArray, n: i32) -> Option<f64> {
        let items = match array {
            ParamArray::ODes => self.owner_desp.as_ref()?,
            ParamArray::OPar => self.owner_para.as_ref()?,
            _ => return None,
        };
        let index = usize::try_from(n).ok()?.checked_sub(1)?;
        Some(items.get(index).copied().unwrap_or(0.0))
    }
}

fn stwall_owner(member: &DbElement) -> Result<Option<DbElement>, DbElementError> {
    let mut cursor = soft(member.owner())?.flatten();
    for _ in 0..OWNER_SECTION_HOPS {
        let Some(current) = cursor else {
            return Ok(None);
        };
        if soft(noun_of(&current))?.as_deref() == Some("STWALL") {
            return Ok(Some(current));
        }
        cursor = soft(current.owner())?.flatten();
    }
    Ok(None)
}

impl ParamEnv for FittingEnv {
    fn param(&self, n: i32) -> Option<f64> {
        self.inner.param(n)
    }

    fn iparam(&self, n: i32) -> Option<f64> {
        self.inner.iparam(n)
    }

    fn parameter(&self, array: ParamArray, n: i32) -> Option<f64> {
        self.inner
            .parameter(array, n)
            .or_else(|| self.owner_parameter(array, n))
    }

    fn design_dimension(&self, id: i32) -> Option<f64> {
        self.inner.design_dimension(id)
    }

    fn attribute_number(&self, attribute: &AttribRef<'_>) -> Option<f64> {
        self.inner.attribute_number(attribute).or_else(|| {
            let array = match attribute.name {
                "ODES" => ParamArray::ODes,
                "OPAR" => ParamArray::OPar,
                _ => return None,
            };
            if !attribute.owner_chain.is_empty() || attribute.qualifier.is_some() {
                return None;
            }
            let subscript = attribute.subscript.filter(|value| value.fract() == 0.0)?;
            self.owner_parameter(array, subscript as i32)
        })
    }

    fn uda_number(&self, key: u32, subscript: Option<f64>) -> Option<f64> {
        self.inner.uda_number(key, subscript)
    }

    fn fast_path_missing_parameter_is_zero(&self, name: &str, subscript: Option<f64>) -> bool {
        match name {
            "ODES" if self.owner_desp.is_some() => true,
            "OPAR" if self.owner_para.is_some() => true,
            _ => self
                .inner
                .fast_path_missing_parameter_is_zero(name, subscript),
        }
    }

    fn direction_angle(&self, program: &[i32]) -> Option<f64> {
        self.inner.direction_angle(program)
    }
}

/// 目录属性按 `env` 求值：存的数、求得出的公式给 `Some`；没设、求不出、不在模板里都是 `None`。
fn number(
    target: &DbElement,
    attribute: &str,
    env: &dyn ParamEnv,
) -> Result<Option<f64>, DbElementError> {
    match target.get_number(attribute, env) {
        Ok(number) => Ok(number.value().filter(|value| value.is_finite())),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 目录轴（构件局部系）：具名轴过目录件原点，`Pn` 轴过那个 P 点（`-Pn` 反向，Core3D `NAXIS`）。
struct Axis {
    origin: DVec3,
    direction: DVec3,
}

/// e3d-model `catalogue_point::resolve_catalogue_axis`（crate 私有）的同口径：先按具名轴解
/// （`e3d_io::record::axis_spec`），再按 `Pn` 在构件的 PTSE 里找。解不出给 `None`。
fn catalogue_axis(
    primitive: &DbElement,
    attribute: &str,
    env: &dyn ParamEnv,
    points: &[CataloguePoint],
) -> Result<Option<Axis>, DbElementError> {
    let words = match primitive.attribute(attribute) {
        Ok(Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words))) => words,
        Ok(Some(DescriptorValue::IntArray(words))) => {
            words.into_iter().map(|word| word as u32).collect()
        }
        Ok(_) | Err(DbElementError::UnknownAttribute { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Some(direction) = axis_spec::direction(&words, env) {
        return Ok(DVec3::from_array(direction)
            .try_normalize()
            .filter(|direction| direction.is_finite())
            .map(|direction| Axis {
                origin: DVec3::ZERO,
                direction,
            }));
    }
    let encoded = match words.as_slice() {
        [3, number] => *number,
        [count, 3, number] if *count == 2 => *number,
        _ => return Ok(None),
    };
    let (number, negated) = if encoded >= 1000 {
        (encoded - 1000, true)
    } else {
        (encoded, false)
    };
    let sign = if negated { -1.0 } else { 1.0 };
    if number == 0 {
        return Ok(Some(Axis {
            origin: DVec3::ZERO,
            direction: DVec3::Z * sign,
        }));
    }
    Ok(points
        .iter()
        .find(|point| point.number == number)
        .and_then(|point| {
            point.dir.try_normalize().map(|direction| Axis {
                origin: point.pos,
                direction: direction * sign,
            })
        }))
}

fn describe(element: &DbElement) -> String {
    wire_refno(element.refno())
}

/// NLCY：PAXI 上从 PBDI 到 PTDI 的轴线（构件局部系）与直径（`getCataData` L1426-1487）。
#[allow(clippy::type_complexity)]
fn cylinder_line(
    cylinder: &DbElement,
    env: &dyn ParamEnv,
    points: &[CataloguePoint],
) -> Result<Result<(DVec3, DVec3, f64), String>, DbElementError> {
    let (Some(diameter), Some(bottom), Some(top)) = (
        number(cylinder, "PDIA", env)?,
        number(cylinder, "PBDI", env)?,
        number(cylinder, "PTDI", env)?,
    ) else {
        return Ok(Err(format!("{} 的 PDIA / PBDI / PTDI", describe(cylinder))));
    };
    let Some(axis) = catalogue_axis(cylinder, "PAXI", env, points)? else {
        return Ok(Err(format!("{} 的 PAXI", describe(cylinder))));
    };
    Ok(Ok((
        axis.origin + axis.direction * bottom,
        axis.origin + axis.direction * top,
        diameter.abs(),
    )))
}

/// NSBO：PX / PY / PZ 居中、PXLE / PYLE / PZLE 的盒，轴同构件（`getBoxEightPossWithPos` 取 `this.ori`）。有一边为零的
/// 不出（PML `poss.unique()` 不满 8 个就 `skip`）。
#[allow(clippy::type_complexity)]
fn box_edges(
    primitive: &DbElement,
    env: &dyn ParamEnv,
) -> Result<Result<Vec<(DVec3, DVec3)>, String>, DbElementError> {
    let mut values = [0.0; 6];
    for (slot, attribute) in values
        .iter_mut()
        .zip(["PX", "PY", "PZ", "PXLE", "PYLE", "PZLE"])
    {
        match number(primitive, attribute, env)? {
            Some(value) => *slot = value,
            None => return Ok(Err(format!("{} 的 {attribute}", describe(primitive)))),
        }
    }
    let [x, y, z, xlen, ylen, zlen] = values;
    let size = DVec3::new(xlen, ylen, zlen).abs();
    if size.min_element() <= LENGTH_EPS {
        return Ok(Ok(Vec::new()));
    }
    let center = DVec3::new(x, y, z);
    let half = DVec2::new(size.x, size.y) * 0.5;
    let upper = rect_corners(center + DVec3::Z * (size.z * 0.5), half);
    let lower = rect_corners(center - DVec3::Z * (size.z * 0.5), half);
    let mut edges = Vec::with_capacity(12);
    let poss: Vec<DVec3> = upper.into_iter().chain(lower).collect();
    for index in 0..8 {
        let next = if index == 3 || index == 7 {
            index - 3
        } else {
            index + 1
        };
        edges.push((poss[index], poss[next]));
    }
    for index in 0..4 {
        edges.push((poss[index], poss[index + 4]));
    }
    Ok(Ok(edges))
}

/// NSEX：每个 SVER 一条竖棱（构件局部系）。标架 x = PAAX、z = x × PBAX、y = z × x，位置 = PAAX 的原点 + 标架 ·（PX, PY,
/// PZ），沿 +z 拉 |PHEI|（e3d-model `evaluate_sext` / `profile_frame`）。
#[allow(clippy::type_complexity)]
fn extrusion_edges(
    primitive: &DbElement,
    env: &dyn ParamEnv,
    points: &[CataloguePoint],
) -> Result<Result<Vec<(DVec3, DVec3)>, String>, DbElementError> {
    let label = describe(primitive);
    let Some(height) = number(primitive, "PHEI", env)? else {
        return Ok(Err(format!("{label} 的 PHEI")));
    };
    let (Some(pa), Some(pb)) = (
        catalogue_axis(primitive, "PAAX", env, points)?,
        catalogue_axis(primitive, "PBAX", env, points)?,
    ) else {
        return Ok(Err(format!("{label} 的 PAAX / PBAX")));
    };
    let x = pa.direction;
    let Some(z) = x.cross(pb.direction).try_normalize() else {
        return Ok(Err(format!("{label} 的 PAAX 与 PBAX 平行")));
    };
    let rotation = DMat3::from_cols(x, z.cross(x), z);
    let mut position = [0.0; 3];
    for (slot, attribute) in position.iter_mut().zip(["PX", "PY", "PZ"]) {
        *slot = number(primitive, attribute, env)?.unwrap_or(0.0);
    }
    let origin = pa.origin + rotation * DVec3::from_array(position);
    let rise = rotation * DVec3::new(0.0, 0.0, height.abs());
    let mut vertices = Vec::new();
    descendants_of(primitive, &["SVER"], &mut vertices, 0)?;
    let mut edges = Vec::with_capacity(vertices.len());
    for vertex in &vertices {
        let (Some(px), Some(py)) = (number(vertex, "PX", env)?, number(vertex, "PY", env)?) else {
            return Ok(Err(format!("{} 的 PX / PY", describe(vertex))));
        };
        let base = origin + rotation * DVec3::new(px, py, 0.0);
        edges.push((base, base + rise));
    }
    Ok(Ok(edges))
}

// ---- 楼板：附近的墙与房间 ------------------------------------------------------------------------------

/// 楼板的体积：SPAMAP 盒（E3D 的 `volume`），没有条目时取面的包围盒。
fn floor_volume(set: &Arc<DbSet>, floor: &DbElement, shape: Option<&CivilShape>) -> Option<Aabb> {
    host::spamap_index(set, floor.db_no())
        .ok()
        .flatten()
        .and_then(|index| index.get(floor.refno()))
        .or_else(|| shape.and_then(|shape| shape_aabb(&shape.faces, None, None)))
}

/// `DbSet` 登记的全部 DESI 库的 SPAMAP 里包围盒碰到 `probe`、类型在 `nouns` 里的元素，按库号、refno。读不动的库跳过并
/// 留一句话。
fn spamap_hits(
    set: &Arc<DbSet>,
    probe: &Aabb,
    nouns: &[&str],
    issues: &mut Vec<String>,
) -> Vec<RefNo> {
    let mut hits = Vec::new();
    for dbnum in design_dbs(set) {
        let index = match host::spamap_index(set, dbnum) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(error) => {
                issues.push(format!("{error}：库 {dbnum} 没参与楼板的空间查询"));
                continue;
            }
        };
        for (refno, aabb) in index.iter() {
            if !aabb.intersects(probe) {
                continue;
            }
            match soft(noun_of(&set.element(refno))) {
                Ok(Some(noun)) if nouns.contains(&noun.as_str()) => hits.push(refno),
                Ok(_) => {}
                Err(error) => issues.push(format!(
                    "SPAMAP 条目 {} 读不成类型，跳过：{error}",
                    wire_refno(refno)
                )),
            }
        }
    }
    hits
}

/// 楼板附近的墙（见模块文档「楼板」）。
fn floor_walls(
    set: &Arc<DbSet>,
    volume: Aabb,
    mode: CivilMode,
    doors: &DoorRule,
    issues: &mut Vec<String>,
) -> Vec<CivilWall> {
    let pad = DVec3::splat(FLOOR_WALL_PAD_MM);
    let probe = Aabb::new(volume.min - pad, volume.max + pad);
    let mut walls = Vec::new();
    for refno in spamap_hits(set, &probe, &FLOOR_WALL_NOUNS, issues) {
        let wall = set.element(refno);
        let mut notes = Vec::new();
        let (element, shape) = match element_view(&wall, &mut notes) {
            Ok((element, Some(shape))) => (element, shape),
            Ok((_, None)) => {
                issues.push(format!(
                    "楼板旁的墙 {} 没有几何，跳过：{}",
                    wire_refno(refno),
                    notes.join("；")
                ));
                continue;
            }
            Err(error) => {
                issues.push(format!(
                    "楼板旁的墙 {} 读不成，跳过：{error}",
                    wire_refno(refno)
                ));
                continue;
            }
        };
        if shape_aabb(&shape.faces, shape.curved.as_ref(), shape.height)
            .is_some_and(|aabb| !aabb.intersects(&probe))
        {
            continue;
        }
        issues.append(&mut notes);
        let members = match mode {
            CivilMode::Arch => doors_on(&wall, doors, issues),
            CivilMode::Civil => Vec::new(),
        };
        walls.push(CivilWall {
            refno: wire_refno(refno),
            noun: shape.noun,
            name: soft(wall.stored_name()).ok().flatten().flatten(),
            element,
            members,
        });
    }
    walls
}

/// 墙名下（任意深度）按门规则判成门的 FITT（建筑 `drawarchdoor`）。
fn doors_on(wall: &DbElement, doors: &DoorRule, issues: &mut Vec<String>) -> Vec<CivilMember> {
    let mut fitts = Vec::new();
    if let Err(error) = descendants_of(wall, &["FITT"], &mut fitts, 0) {
        issues.push(format!(
            "墙 {} 上的门没收全：{error}",
            wire_refno(wall.refno())
        ));
    }
    let mut members = Vec::new();
    for fitt in &fitts {
        match doors.is_door(fitt) {
            Ok(true) => members.push(member_view(fitt, true, issues)),
            Ok(false) => {}
            Err(error) => issues.push(format!(
                "FITT {} 判不了是不是门，跳过：{error}",
                wire_refno(fitt.refno())
            )),
        }
    }
    members
}

/// 楼板附近的房间（见模块文档「楼板」）。
fn floor_rooms(set: &Arc<DbSet>, volume: Aabb, issues: &mut Vec<String>) -> Vec<CivilRoom> {
    let probe = Aabb::new(volume.min, volume.max + DVec3::Z * ROOM_RISE_MM);
    let mut frames = Vec::new();
    let mut seen = HashSet::new();
    for refno in spamap_hits(set, &probe, &["PANE"], issues) {
        let pane = set.element(refno);
        let frame = (|| -> Result<Option<DbElement>, DbElementError> {
            let Some(owner) = soft(pane.owner())?.flatten() else {
                return Ok(None);
            };
            if soft(noun_of(&owner))?.as_deref() != Some("SBFR") {
                return Ok(None);
            }
            host::ancestor_of_type(&pane, "FRMW")
        })();
        match frame {
            Ok(Some(frame)) => {
                if seen.insert(frame.refno()) {
                    frames.push(frame);
                }
            }
            Ok(None) => {}
            Err(error) => issues.push(format!(
                "PANE {} 读不出所在房间，跳过：{error}",
                wire_refno(refno)
            )),
        }
    }
    let mut rooms = Vec::new();
    for frame in &frames {
        match room_view(frame, issues) {
            Ok(Some(room)) => rooms.push(room),
            Ok(None) => {}
            Err(error) => issues.push(format!(
                "房间 {} 读不成，跳过：{error}",
                wire_refno(frame.refno())
            )),
        }
    }
    rooms
}

/// PML `floorRoom`：名字含 `-ARCH-` / `-RM` / `-R-`、名下有 SBFR 的 FRMW；名下全部 PANE 各给中心较低的那张面。
fn room_view(
    frame: &DbElement,
    issues: &mut Vec<String>,
) -> Result<Option<CivilRoom>, DbElementError> {
    let Some(name) = soft(frame.stored_name())?.flatten() else {
        return Ok(None);
    };
    if !ROOM_FRMW_KEYWORDS
        .iter()
        .any(|keyword| name.contains(keyword))
        || frame.members_of_type("SBFR")?.is_empty()
    {
        return Ok(None);
    }
    let mut panes = Vec::new();
    descendants_of(frame, &["PANE"], &mut panes, 0)?;
    let mut faces = Vec::new();
    for pane in &panes {
        match first_ring(pane, false)? {
            Ok(ring) => {
                let (bottom, top) = (ring.face(0.0), ring.face(ring.height));
                let mean_z = |face: &CivilFace| {
                    face.vertices.iter().map(|vertex| vertex[2]).sum::<f64>()
                        / face.vertices.len().max(1) as f64
                };
                faces.push(if mean_z(&top) < mean_z(&bottom) {
                    top
                } else {
                    bottom
                });
            }
            Err(why) => issues.push(format!(
                "房间 {name} 的 PANE {} 读不成，跳过：{why}",
                wire_refno(pane.refno())
            )),
        }
    }
    Ok(Some(CivilRoom {
        frmw_name: name,
        panes: faces,
    }))
}
