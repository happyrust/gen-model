//! 支架视图（plant3d-web#103 M4 / M5 宿主侧）：一个支架 → `plant_mbd::SupportQueryResult`，EM4 支架、EM9 仪表支架、
//! EM9 仪表管支架共用。字段与 PML 口径见 plant-mbd `docs/field-dictionary.md`「支架视图」（EM9 `instSupport` 的取数
//! 加 EM4 `suiSupport` 的增补）；型钢截面、板、目录原语的几何在 [`super::support_geometry`]。
//!
//! # 支架的范围
//!
//! [`super::object::support_parts`]（PML `getsuppname`）给出 SUPPO，或支架区里 ZONE 级的 STRU + REST 一对。PML 的
//! `coll all for $suppname` 在 `:SUPP` 支架上是整个支架；AMS 的支架名就是 STRU 的名字，照抄会漏掉 REST 下的吊挂件，
//! 这里按原意收：SUPPO 收它的全部后代，否则先 STRU、再 REST 的全部后代（深度优先、文件原序，根本身不收）。
//!
//! # 构件（`members`）
//!
//! 每个后代一项：类型 / NAME / 属主类型与 refno、是否在 STRU 下；`SPRE` 读得到（`has_spref`）时给 SPREF 名（无名时
//! `=w0/w1`，同 PML `name of`）与 `DTXR`（`SPRE` 是 SPCO 时取 `DETA` 的 `RTEX`）；世界 `POS` / `ORI`（型钢取截面标架，
//! 见 [`super::support_geometry`]）；有 `ARRI` / `LEAV` 的取 `APOS` / `LPOS` / `LDIR`（目录 P 点）；目录 P 点全集（PTSE
//! 原序，世界系；PTSE 里没有 P0 的目录件补上构件原点、局部 +Z 的 P0——E3D 照样答 `p0 pos / dir`，PML 拿它当管夹点）；
//! 目录件的 `PARA`、带保温规格（`ISPE`）时的 `IPARA[1]`、`name of cate of catref` 与已求值的几何原语（板上 PFIT 的
//! 实例标架贴板面，同 e3d-model；带保温时圆柱另带不带保温的 `pdia_bare`，见 [`super::support_geometry`]）；SCTN /
//! GENSEC 的型钢数据、PANE 的板数据；`CUTL`（E3D 现算的伪属性，这里按 `|POSE − POSS|` 给，只对 SCTN）；`DESP`；UDA
//! `:itemcode`、`:SUPP-ADLONGTH`（型钢）、`:ZD-XCQG`、`:weldTP`（只给 STRU 下 `DBDEPTH > 7` 的 BOX，WORL 记 0）。
//! 包围盒（PML `volume`）先查本库 SPAMAP（[`host::spamap_index`]），没有条目的按已取到的几何点（截面两端、板两面、
//! 原语、P 点）求；材料码表不在宿主，`material_desc` 不给。
//!
//! # 管道附件（`pipe_attas`）
//!
//! 名字以支架名开头的 ATTA（PML `matchwild(name, suppname + '*')`）：读 `DbSet` 里全部 DESI 库的 NAME 表（按库文件 +
//! 会话缓存）按前缀取，不做全树扫。EM4（`suiSupport.addattatopartobjects`）全库收，读不到 SPREF 的跳过；EM9
//! （`instSupport.addattatopartobjects`）只收 REST 下 HANG 的 `HREF` 所指 ATTA 的那几条 BRAN 上的。按属主 BRAN 分组
//! （首次出现的顺序）。每个附件：世界位置、到达 / 离开方向（`ARRI` / `LEAV` 的 P 点）、`ABORE`（到达 P 点通径）、
//! `AOD`（附件所在那段管的外径：前一个非 ATTA 成员离开侧——没有时 BRAN 头——的管子目录件 `PARA[2]`，PML `aod` 读不到
//! 时就退到这根管）、P3 位置、前 / 后管的通径（`getattatubebore`：前一个成员的离开通径，没有时 BRAN 的 `HBOR`，再退到
//! 后一个成员的到达通径），以及目录里有效的 SRTO / LCYL / SCYL / SSCY（`pipeatta.getattageo`）：有就是带几何的附件，
//! 圆柱另成 ISOLEG 支腿（`pos ∓ 轴 · 高 / 2`、外径）；穿过附件位置、到达与离开方向对顶（> 179°）的圆柱是管身本身，
//! 不算（L1160-1171）。
//!
//! # 墙板与找墙
//!
//! - `concretes`：支架下 LNKI 的 `LREF` 指向 WALL / GWALL / STWALL / FLOOR 的，按 [`civil_candidate_of`] 建面（两张大面
//!   在前、侧面在后，曲墙给内外两道弧与墙高），面的法向按墙板全部顶点的包围盒中心定成朝外。
//! - `civil_candidates`（[`locate_candidates`]）：EM9 仪表支架的基准点是全部构件取点的包围盒中心——直接拿求解器的
//!   构件收集算（`support::em9::parts::collect_parts` + `labels::location_position`），没有取点时退到支架包围盒中心；
//!   EM4 是按名字排第一的点附件的位置（`suiSupport.getlocationposition`），没有点附件时不找（求解器同样不找墙）；EM9
//!   仪表管支架求解器不找墙，不收。
//!
//! 支架包围盒（`bbox`，PML `getvolume`）：FRMW 与 HANG 的直属成员的包围盒之并，跳过 LNKS / LNKI 与 `SPRE` unset 的
//! SCTN（悬空的引用不是 unset，照收）。`function_code` 是支架根的 UDA `:ZD_BZBM`（AMS 字典没有 `:ZD_*`，一律缺）。
//!
//! 错误：归属对象不在所钉会话 → [`SupportQueryError::NotFound`]；读归属本身出错原样上抛；单个构件 / 附件 / 墙板 /
//! 找墙读不成都降级成 `issues` 里的一句话（入口逐条转成 `data.view_issue`），不让整个对象 500。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::index::{NamedTable, name_table_hash};
use e3d_io::record::catalogue_eval::ParamEnv;
use e3d_io::refno::RefNo;
use e3d_io::session::DbView;
use e3d_model::route::{PPointResolver, RouteEnd, departure_tube_outer_diameter};
use e3d_model::{CataloguePoint, CataloguePoints, catalogue_point_set};
use glam::{DMat3, DMat4, DVec3};
use plant_mbd::support::em9::labels::location_position;
use plant_mbd::support::em9::parts::collect_parts;
use plant_mbd::support::view::{
    SupportArcFace, SupportAttaLeg, SupportBox, SupportConcrete, SupportFace, SupportMember,
    SupportPPoint, SupportPipeAtta,
};
use plant_mbd::{AnnotationType, CivilCandidate, SupportQueryResult};

use super::civil_faces::{
    CIVIL_CANDIDATE_NOUNS, candidate_aabb, civil_candidate_of, design_dbs, locate_candidates,
};
use super::host::{self, Aabb, CatalogueEval, HostError, SpamapIndex, noun_of, soft};
use super::object::{Classified, SupportParts, support_parts};
use super::support_geometry::{
    PrimitiveContext, attribute_word, geometry_set, number, read_plate, read_section,
};
use super::wire_refno;

/// 子树深度上限：支架 → FRMW → SCTN → FITT → 图元实测五六层，64 只可能是环。
const MAX_DEPTH: usize = 64;
/// 弧墙 / 穿管判据里的「对顶」（`pipeatta.pmlobj` L1163）。
const STRAIGHT_THROUGH_DEG: f64 = 179.0;
/// EM4 焊缝：`DBDEPTH` 大于它的 BOX（`suiSupport.getwelds` L856-859）。
const WELD_BOX_MIN_DEPTH: usize = 7;
/// 管道附件按名字取时的类型。
const ATTA_NOUN: &str = "ATTA";

#[derive(Debug)]
pub enum SupportQueryError {
    /// 归属对象在所钉会话里不存在。
    NotFound {
        refno: RefNo,
    },
    Db(DbElementError),
}

impl std::fmt::Display for SupportQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SupportQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::NotFound { .. } => None,
        }
    }
}

impl From<DbElementError> for SupportQueryError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// 一个支架的视图，见模块文档。判型的 `notes` 排在 `issues` 最前。
pub fn query_support(
    set: &Arc<DbSet>,
    classified: &Classified,
) -> Result<SupportQueryResult, SupportQueryError> {
    let subject = set.element(classified.subject_refno);
    match subject.exists() {
        Ok(true) => {}
        Ok(false)
        | Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            return Err(SupportQueryError::NotFound {
                refno: classified.subject_refno,
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut view = SupportQueryResult {
        subject_refno: classified.subject_wire_refno(),
        subject_noun: classified.subject_noun.clone(),
        name: classified.subject_name.clone(),
        issues: classified.notes.clone(),
        site_name: classified.site_name.clone(),
        zone_name: classified.zone_name.clone(),
        ..SupportQueryResult::default()
    };
    let parts = match support_parts(&subject)? {
        Some(parts) => parts,
        None => {
            view.issues.push(format!(
                "{} {} 既不在 SUPPO 之下、也不是支架区里 ZONE 一级的 STRU / REST，只读它自己的子树",
                classified.subject_noun, view.subject_refno
            ));
            SupportParts {
                name: classified.subject_name.clone(),
                suppo: None,
                stru: (classified.subject_noun == "STRU").then(|| subject.clone()),
                rest: (classified.subject_noun != "STRU").then(|| subject.clone()),
            }
        }
    };
    view.supp_name = parts.name.clone();
    view.stru_name = stored_name(parts.stru.as_ref())?;
    view.rest_name = stored_name(parts.rest.as_ref())?;

    let walk = walk_support(&parts, &mut view.issues)?;
    let mut reader = MemberReader::new(set);
    for item in &walk {
        match reader.member(item, &mut view.issues) {
            Ok(member) => view.members.push(member),
            Err(error) => {
                view.issues.push(format!(
                    "{} {} 读不成（{error}），只留类型与 NAME",
                    item.noun,
                    wire_refno(item.element.refno())
                ));
                view.members.push(SupportMember {
                    refno: wire_refno(item.element.refno()),
                    noun: item.noun.clone(),
                    name: item.element.stored_name().ok().flatten(),
                    in_stru: item.in_stru,
                    ..SupportMember::default()
                });
            }
        }
    }
    view.bbox = support_bbox(&view.members, &reader.spre_set);
    view.concretes = reader.concretes(&walk, &mut view.issues);
    match function_code(&parts) {
        Ok(code) => view.function_code = code,
        Err(error) => view
            .issues
            .push(format!(":ZD_BZBM 读不成（{error}），没有支架功能")),
    }

    let hangs: Vec<DbElement> = walk
        .iter()
        .filter(|item| item.noun == "HANG" && !item.in_stru)
        .map(|item| item.element.clone())
        .collect();
    match pipe_attas(
        set,
        classified.annotation_type,
        parts.name.as_deref(),
        &hangs,
        &mut view.issues,
    ) {
        Ok(attas) => view.pipe_attas = attas,
        Err(error) => view
            .issues
            .push(format!("管道附件读不成（{error}），没有管道附件")),
    }
    if let Some(base) = locate_base(classified.annotation_type, &view) {
        match locate_candidates(set, base) {
            Ok(found) => {
                view.civil_candidates = found.candidates;
                view.issues.extend(found.issues);
            }
            Err(error) => view
                .issues
                .push(format!("找墙候选读不成（{error}），没有找墙候选")),
        }
    }
    Ok(view)
}

fn stored_name(element: Option<&DbElement>) -> Result<Option<String>, DbElementError> {
    match element {
        Some(element) => Ok(soft(element.stored_name())?.flatten()),
        None => Ok(None),
    }
}

/// PML `name of`：有存名给原文，没有给 `=w0/w1`。
fn pml_name(element: &DbElement) -> Result<String, DbElementError> {
    let refno = element.refno();
    Ok(soft(element.stored_name())?
        .flatten()
        .unwrap_or_else(|| format!("={}/{}", refno.word0, refno.word1 as i32)))
}

// ---- 子树 ----------------------------------------------------------------------------------------

/// 支架下的一个元素。
struct WalkItem {
    element: DbElement,
    noun: String,
    in_stru: bool,
    /// PML `DBDEPTH`（WORL 记 0）。
    depth: usize,
}

/// 支架的全部后代，见模块文档「支架的范围」。走不动的根留一句话、跳过。
fn walk_support(
    parts: &SupportParts,
    issues: &mut Vec<String>,
) -> Result<Vec<WalkItem>, DbElementError> {
    let roots: Vec<&DbElement> = match &parts.suppo {
        Some(suppo) => vec![suppo],
        None => parts.stru.iter().chain(parts.rest.iter()).collect(),
    };
    let mut items = Vec::new();
    for root in roots {
        let noun = noun_of(root)?;
        let depth = host::owner_chain(root)?.len().saturating_sub(1);
        // 走到一半读不动：已收的照用，留一句话（子树里的元素坏了不该让整个支架 500）。
        if let Err(error) = walk(root, noun == "STRU", depth, &mut items) {
            issues.push(format!(
                "{noun} {} 的子树没走完（{error}），只用已收到的 {} 个元素",
                wire_refno(root.refno()),
                items.len()
            ));
        }
    }
    Ok(items)
}

fn walk(
    owner: &DbElement,
    in_stru: bool,
    depth: usize,
    items: &mut Vec<WalkItem>,
) -> Result<(), DbElementError> {
    if items.len() > 100_000 || depth > MAX_DEPTH * 2 {
        return Err(DbElementError::OwnerCycle {
            start: owner.refno(),
            at: owner.refno(),
        });
    }
    for member in owner.members()? {
        let noun = soft(noun_of(&member))?.unwrap_or_default();
        let member_in_stru = in_stru || noun == "STRU";
        items.push(WalkItem {
            element: member.clone(),
            noun,
            in_stru: member_in_stru,
            depth: depth + 1,
        });
        walk(&member, member_in_stru, depth + 1, items)?;
    }
    Ok(())
}

// ---- 构件 ----------------------------------------------------------------------------------------

/// 读构件；按库缓存 SPAMAP。
struct MemberReader<'a> {
    set: &'a Arc<DbSet>,
    spamaps: HashMap<u32, Option<Arc<SpamapIndex>>>,
    /// `SPRE` 设了（非 Nulref，目标读不读得到不论）的构件 refno：支架包围盒只跳过 `SPRE` unset 的 SCTN（PML
    /// `spref.unset()`），悬空的引用照收。
    spre_set: HashSet<String>,
}

impl<'a> MemberReader<'a> {
    fn new(set: &'a Arc<DbSet>) -> Self {
        Self {
            set,
            spamaps: HashMap::new(),
            spre_set: HashSet::new(),
        }
    }

    fn spamap(
        &mut self,
        dbnum: u32,
        issues: &mut Vec<String>,
    ) -> Result<Option<Arc<SpamapIndex>>, DbElementError> {
        if let Some(cached) = self.spamaps.get(&dbnum) {
            return Ok(cached.clone());
        }
        let index = match host::spamap_index(self.set, dbnum) {
            Ok(index) => index,
            Err(HostError::Db(error)) => return Err(error),
            Err(error) => {
                issues.push(format!("{error}：构件包围盒按几何点求"));
                None
            }
        };
        self.spamaps.insert(dbnum, index.clone());
        Ok(index)
    }

    /// 元素在本库 SPAMAP 里的包围盒。
    fn spamap_box(
        &mut self,
        element: &DbElement,
        issues: &mut Vec<String>,
    ) -> Result<Option<SupportBox>, DbElementError> {
        Ok(self
            .spamap(element.db_no(), issues)?
            .and_then(|index| index.get(element.refno()))
            .map(support_box))
    }

    fn member(
        &mut self,
        item: &WalkItem,
        issues: &mut Vec<String>,
    ) -> Result<SupportMember, DbElementError> {
        let element = &item.element;
        let noun = item.noun.as_str();
        let label = format!("{noun} {}", wire_refno(element.refno()));
        let mut member = SupportMember {
            refno: wire_refno(element.refno()),
            noun: noun.to_string(),
            name: soft(element.stored_name())?.flatten(),
            in_stru: item.in_stru,
            ..SupportMember::default()
        };
        if let Some(owner) = soft(element.owner())?.flatten() {
            member.owner_noun = soft(noun_of(&owner))?;
            member.owner_refno = Some(wire_refno(owner.refno()));
        }
        if let Some(spre) = host::optional_element(element, "SPRE")? {
            self.spre_set.insert(member.refno.clone());
            if soft(spre.exists())? == Some(true) {
                member.has_spref = true;
                member.spref_name = Some(pml_name(&spre)?);
                member.dtxr = detail_text(&spre)?;
            }
        }
        let world = host::world_matrix_soft(element)?;
        if world.is_none() {
            issues.push(format!(
                "{label} 的属主链折不进世界系（链上有库没注册），没有位置与几何"
            ));
        }

        let is_section = matches!(noun, "SCTN" | "GENSEC");
        if is_section {
            let data = read_section(element, noun, world, issues)?;
            member.pos = match noun {
                "SCTN" => data.frame.map(|frame| frame.w_axis.truncate().to_array()),
                _ => world.map(|world| world.w_axis.truncate().to_array()),
            };
            member.ori = data
                .frame
                .and_then(|frame| axes(DMat3::from_mat4(frame)))
                .or_else(|| world.and_then(|world| axes(DMat3::from_mat4(world))));
            if noun == "SCTN" {
                member.cutl = data.length;
            }
            member.primitives = data.primitives;
            member.para = data.para;
            member.cate_name = data.cate_name;
            member.section = Some(data.section);
            member.adjust_len = uda_number(element, ":SUPP-ADLONGTH")?;
        } else if let Some(world) = world {
            member.pos = Some(world.w_axis.truncate().to_array());
            member.ori = axes(DMat3::from_mat4(world));
        }

        if !is_section && let Some(eval) = soft(CatalogueEval::of(element))?.flatten() {
            member.para = host::optional_doubles(eval.component(), "PARA")?.unwrap_or_default();
            member.cate_name = host::ancestor_name(eval.component(), "CATE")?;
            member.ipara = insulation_parameters(element, eval.component())?;
            if let (Some(world), Some(geometry)) = (world, geometry_set(eval.component())?) {
                let world = catalogue_world(element, noun, world)?;
                let bare_env = (!member.ipara.is_empty()).then(|| eval.env());
                let mut context =
                    PrimitiveContext::new(element, eval.env(), world).with_bare_env(bare_env);
                for primitive in geometry.members()? {
                    match context.primitive(&primitive) {
                        Ok(primitive) => member.primitives.push(primitive),
                        Err(
                            error @ (DbElementError::DbNotRegistered { .. }
                            | DbElementError::NotFound { .. }),
                        ) => issues.push(format!(
                            "{label} 的目录原语 {} 读不成（{error}）",
                            wire_refno(primitive.refno())
                        )),
                        Err(error) => return Err(error),
                    }
                }
            }
        }

        if let Some(world) = world {
            let points = match soft(catalogue_point_set(element))? {
                // P0 是构件自己的原点、方向局部 +Z，不看目录（e3d-model `catalogue_point` 同口径）：PTSE 里没有
                // P0 的目录件 E3D 照样答 `p0 pos / dir`（PML `suppclamp` 取它当管夹点）。
                Some(set)
                    if set.component.is_some() && !set.points.iter().any(|p| p.number == 0) =>
                {
                    let mut points = vec![CataloguePoint {
                        number: 0,
                        noun: "P0".to_string(),
                        pos: DVec3::ZERO,
                        dir: DVec3::Z,
                        bore: None,
                    }];
                    points.extend(set.points);
                    points
                }
                Some(set) => set.points,
                None => Vec::new(),
            };
            let place = |point: &CataloguePoint| SupportPPoint {
                number: point.number,
                pos: world.transform_point3(point.pos).to_array(),
                dir: world
                    .transform_vector3(point.dir)
                    .try_normalize()
                    .map(|dir| dir.to_array()),
            };
            member.ppoints = points.iter().map(place).collect();
            let find = |number: u32| {
                points
                    .iter()
                    .find(|point| point.number == number)
                    .map(place)
            };
            if let Some(arrive) = attribute_word(element, "ARRI")? {
                member.apos = find(arrive).map(|point| point.pos);
            }
            if let Some(leave) = attribute_word(element, "LEAV")?
                && let Some(point) = find(leave)
            {
                member.lpos = Some(point.pos);
                member.ldir = point.dir;
            }
            if noun == "PANE" {
                let data = read_plate(element, world, issues)?;
                member.plate = data.plate;
                member.pane_pad = data.pane_pad;
            }
        }

        member.bbox = match self.spamap_box(element, issues)? {
            Some(bbox) => Some(bbox),
            None => geometry_box(&member),
        };
        member.desp = host::optional_doubles(element, "DESP")?.unwrap_or_default();
        member.item_code = uda_text(element, ":itemcode")?;
        member.xcqg = uda_text(element, ":ZD-XCQG")?;
        if noun == "BOX" && item.in_stru && item.depth > WELD_BOX_MIN_DEPTH {
            member.weld_text = uda_text(element, ":weldTP")?;
        }
        Ok(member)
    }

    /// LNKI 关联的墙板，见模块文档「墙板与找墙」。
    fn concretes(&mut self, walk: &[WalkItem], issues: &mut Vec<String>) -> Vec<SupportConcrete> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for item in walk.iter().filter(|item| item.noun == "LNKI") {
            match self.concrete(&item.element, &mut seen, issues) {
                Ok(Some(concrete)) => out.push(concrete),
                Ok(None) => {}
                Err(error) => issues.push(format!(
                    "LNKI {} 的墙板读不成（{error}）",
                    wire_refno(item.element.refno())
                )),
            }
        }
        out
    }

    fn concrete(
        &mut self,
        lnki: &DbElement,
        seen: &mut HashSet<RefNo>,
        issues: &mut Vec<String>,
    ) -> Result<Option<SupportConcrete>, DbElementError> {
        let Some(target) = soft(host::optional_element(lnki, "LREF"))?.flatten() else {
            return Ok(None);
        };
        let Some(noun) = soft(noun_of(&target))? else {
            return Ok(None);
        };
        if !CIVIL_CANDIDATE_NOUNS.contains(&noun.as_str()) || !seen.insert(target.refno()) {
            return Ok(None);
        }
        let candidate = match civil_candidate_of(&target, issues) {
            Ok(candidate) => candidate,
            Err(error) => {
                issues.push(format!(
                    "LNKI {} 关联的 {noun} {} 建不成墙面：{error}",
                    wire_refno(lnki.refno()),
                    wire_refno(target.refno())
                ));
                return Ok(None);
            }
        };
        let bbox = match self.spamap_box(&target, issues)? {
            Some(bbox) => Some(bbox),
            None => candidate_aabb(&candidate).map(support_box),
        };
        Ok(Some(concrete_of(candidate, bbox)))
    }
}

/// 目录几何的实例标架：一般就是 `world_matrix`；板上的 PFIT 贴在属主板 `SJUS` 指的那一面上、`POS.z` 不进实例、
/// `BANG` 绕板法向转（e3d-model `catalogue::panel_fitting_world` 同口径，那边是 crate 私有）。
fn catalogue_world(member: &DbElement, noun: &str, world: DMat4) -> Result<DMat4, DbElementError> {
    if noun != "PFIT" {
        return Ok(world);
    }
    let Some(panel) = soft(member.owner())?.flatten() else {
        return Ok(world);
    };
    let surface = match e3d_model::profile::loop_extent(&panel)? {
        Some(extent) => {
            let sjus = host::optional_text(member, "SJUS")?.unwrap_or_default();
            e3d_model::profile::sjus_drop(sjus.trim(), extent.height) - extent.sjus_drop
        }
        None => 0.0,
    };
    let local = e3d_model::transform::local_matrix(member)?;
    let on_panel = DMat4::from_cols(
        local.x_axis,
        local.y_axis,
        local.z_axis,
        DVec3::new(local.w_axis.x, local.w_axis.y, surface).extend(1.0),
    );
    let bang = host::optional_double(member, "BANG")?
        .unwrap_or(0.0)
        .to_radians();
    Ok(world * local.inverse() * on_panel * DMat4::from_rotation_z(bang))
}

fn axes(rotation: DMat3) -> Option<[[f64; 3]; 3]> {
    let unit = |axis: DVec3| axis.try_normalize().map(|axis| axis.to_array());
    Some([
        unit(rotation.x_axis)?,
        unit(rotation.y_axis)?,
        unit(rotation.z_axis)?,
    ])
}

fn support_box(aabb: Aabb) -> SupportBox {
    SupportBox {
        min: aabb.min.to_array(),
        max: aabb.max.to_array(),
    }
}

/// `DTXR`：`SPRE` 是 SPCO 时取它 `DETA` 的 `RTEX`；空串与 `unset` 按没有。
fn detail_text(spre: &DbElement) -> Result<Option<String>, DbElementError> {
    if soft(noun_of(spre))?.as_deref() != Some("SPCO") {
        return Ok(None);
    }
    let Some(detail) = soft(host::optional_element(spre, "DETA"))?.flatten() else {
        return Ok(None);
    };
    Ok(soft(host::optional_text(&detail, "RTEX"))?
        .flatten()
        .filter(|text| !text.eq_ignore_ascii_case("unset")))
}

/// UDA 文本：字典里没有、对这个 noun 无效、没写都按没有。
fn uda_text(element: &DbElement, name: &str) -> Result<Option<String>, DbElementError> {
    match element.get_string(name) {
        Ok(value) => Ok(value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("unset"))),
        Err(
            DbElementError::UnknownAttribute { .. }
            | DbElementError::Uda { .. }
            | DbElementError::TypeMismatch { .. },
        ) => Ok(None),
        Err(error) => Err(error),
    }
}

/// UDA 数值：同 [`uda_text`]；存成文本的按文本解数。
fn uda_number(element: &DbElement, name: &str) -> Result<Option<f64>, DbElementError> {
    match element.get_double(name) {
        Ok(value) => Ok(value.filter(|value| value.is_finite())),
        Err(DbElementError::TypeMismatch { .. }) => Ok(uda_text(element, name)?
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|value| value.is_finite())),
        Err(DbElementError::UnknownAttribute { .. } | DbElementError::Uda { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `IPARA[1]`：只有设了保温规格（`ISPE`）的构件才有，按带保温的求值环境取。
fn insulation_parameters(
    member: &DbElement,
    component: &DbElement,
) -> Result<Vec<f64>, DbElementError> {
    if host::optional_element(member, "ISPE")?.is_none() {
        return Ok(Vec::new());
    }
    let env = DbElementParamEnv::new(Some(member.clone()), Some(component.clone()));
    Ok(env
        .iparam(1)
        .filter(|value| value.is_finite())
        .into_iter()
        .collect())
}

/// 包围盒的后备：已取到的几何点（截面两端、板两面、原语、P 点、APOS / LPOS）。
fn geometry_box(member: &SupportMember) -> Option<SupportBox> {
    let v = DVec3::from_array;
    let mut points: Vec<DVec3> = Vec::new();
    if let Some(section) = &member.section {
        let (start, end) = (v(section.posstart), v(section.posend));
        points.extend([start, end]);
        for point in &section.section_points {
            points.extend([v(*point), v(*point) + (end - start)]);
        }
    }
    if let Some(plate) = &member.plate {
        let lift = v(plate.normal) * plate.thickness;
        for point in &plate.vertices {
            points.extend([v(*point), v(*point) + lift]);
        }
    }
    let member_axes = member.ori.map(|[x, y, z]| [v(x), v(y), v(z)]);
    for primitive in &member.primitives {
        let Some(centre) = primitive.center.map(v) else {
            for point in &primitive.vertices {
                points.push(v(*point));
                if let (Some(axis), Some(height)) = (primitive.axis, primitive.height) {
                    points.push(v(*point) + v(axis) * height);
                }
            }
            continue;
        };
        if let (Some([x, y, z]), Some(size)) = (member_axes, primitive.size) {
            for sx in [-0.5, 0.5] {
                for sy in [-0.5, 0.5] {
                    for sz in [-0.5, 0.5] {
                        points.push(
                            centre + x * (sx * size[0]) + y * (sy * size[1]) + z * (sz * size[2]),
                        );
                    }
                }
            }
        } else if let (Some(axis), Some(height)) = (primitive.axis.map(v), primitive.height) {
            let radius = primitive.diameter.unwrap_or(0.0) / 2.0;
            let (p, q) = axis.any_orthonormal_pair();
            for end in [
                centre - axis * (height / 2.0),
                centre + axis * (height / 2.0),
            ] {
                points.extend([
                    end + p * radius,
                    end - p * radius,
                    end + q * radius,
                    end - q * radius,
                ]);
            }
        } else {
            points.push(centre);
        }
    }
    points.extend(member.ppoints.iter().map(|point| v(point.pos)));
    points.extend(member.apos.map(v));
    points.extend(member.lpos.map(v));
    let (first, rest) = points.split_first()?;
    let aabb = rest.iter().fold(Aabb::new(*first, *first), |aabb, point| {
        Aabb::new(aabb.min.min(*point), aabb.max.max(*point))
    });
    (aabb.min.is_finite() && aabb.max.is_finite()).then(|| support_box(aabb))
}

/// PML `getvolume`（`instSupport.pmlobj:3855-3903`）：FRMW 与 HANG 的直属成员之并，跳过 LNKS / LNKI 与 `SPRE` unset
/// 的 SCTN（`spre_set` 里没有的）。
fn support_bbox(members: &[SupportMember], spre_set: &HashSet<String>) -> Option<SupportBox> {
    members
        .iter()
        .filter(|member| matches!(member.owner_noun.as_deref(), Some("FRMW" | "HANG")))
        .filter(|member| !matches!(member.noun.as_str(), "LNKS" | "LNKI"))
        .filter(|member| member.noun != "SCTN" || spre_set.contains(&member.refno))
        .filter_map(|member| member.bbox)
        .reduce(|merged, bbox| SupportBox {
            min: DVec3::from_array(merged.min)
                .min(DVec3::from_array(bbox.min))
                .to_array(),
            max: DVec3::from_array(merged.max)
                .max(DVec3::from_array(bbox.max))
                .to_array(),
        })
}

/// 支架根（SUPPO，否则 STRU、REST）的 UDA `:ZD_BZBM`（`instSupport.gettextindexs` L4838）。
fn function_code(parts: &SupportParts) -> Result<Option<String>, DbElementError> {
    for root in [&parts.suppo, &parts.stru, &parts.rest]
        .into_iter()
        .flatten()
    {
        if let Some(text) = uda_text(root, ":ZD_BZBM")?
            && !matches!(text.as_str(), "=0/0" | "Nulref")
        {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// 墙板候选 → 支架墙板：面的法向定成朝外（指离全部顶点的包围盒中心），曲墙的内外两道弧。
fn concrete_of(candidate: CivilCandidate, bbox: Option<SupportBox>) -> SupportConcrete {
    let v = DVec3::from_array;
    let all: Vec<DVec3> = candidate
        .faces
        .iter()
        .flat_map(|face| face.vertices.iter().copied().map(v))
        .collect();
    let centre = (!all.is_empty()).then(|| {
        let (lo, hi) = all.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), point| (lo.min(*point), hi.max(*point)),
        );
        (lo + hi) / 2.0
    });
    let faces = candidate
        .faces
        .iter()
        .filter_map(|face| {
            let points: Vec<DVec3> = face.vertices.iter().copied().map(v).collect();
            let mut normal = match face.normal {
                Some(normal) => v(normal).try_normalize()?,
                None => plant_mbd::geom::newell_normal(&points)?,
            };
            let face_centre = points.iter().copied().sum::<DVec3>() / points.len().max(1) as f64;
            if let Some(centre) = centre
                && (face_centre - centre).dot(normal) < 0.0
            {
                normal = -normal;
            }
            Some(SupportFace {
                vertices: face.vertices.clone(),
                normal: normal.to_array(),
            })
        })
        .collect();
    let arcs = candidate
        .curved
        .iter()
        .flat_map(|curved| {
            [curved.inner_radius, curved.outer_radius].map(|radius| SupportArcFace {
                center: curved.center,
                axis: curved.axis,
                radius,
                start_dir: curved.x_axis,
                sweep_deg: curved.sweep_deg,
            })
        })
        .collect();
    SupportConcrete {
        refno: candidate.refno,
        noun: candidate.noun,
        name: candidate.name,
        bbox,
        faces,
        arcs,
        height: candidate.height,
    }
}

// ---- 管道附件 ------------------------------------------------------------------------------------

/// 见模块文档「管道附件」。只有读库出错才 `Err`；单个附件读不成留一句话。
fn pipe_attas(
    set: &Arc<DbSet>,
    annotation_type: AnnotationType,
    support_name: Option<&str>,
    hangs: &[DbElement],
    issues: &mut Vec<String>,
) -> Result<Vec<SupportPipeAtta>, DbElementError> {
    let Some(prefix) = support_name.filter(|name| !name.is_empty()) else {
        return Ok(Vec::new());
    };
    let em4 = annotation_type == AnnotationType::Em4Supp;
    let branches: Option<HashSet<RefNo>> = if em4 {
        None
    } else {
        let mut branches = HashSet::new();
        for hang in hangs {
            if let Some(target) = soft(host::optional_element(hang, "HREF"))?.flatten()
                && soft(noun_of(&target))?.as_deref() == Some(ATTA_NOUN)
                && let Some(bran) = soft(host::ancestor_of_type(&target, "BRAN"))?.flatten()
            {
                branches.insert(bran.refno());
            }
        }
        if branches.is_empty() {
            return Ok(Vec::new());
        }
        Some(branches)
    };
    let mut groups: Vec<(DbElement, Vec<DbElement>)> = Vec::new();
    for refno in named_with_prefix(set, prefix, issues)? {
        let atta = set.element(refno);
        if soft(noun_of(&atta))?.as_deref() != Some(ATTA_NOUN) {
            continue;
        }
        if em4 {
            let readable = match soft(host::optional_element(&atta, "SPRE"))?.flatten() {
                Some(spre) => soft(spre.exists())? == Some(true),
                None => false,
            };
            if !readable {
                continue;
            }
        }
        let Some(bran) = soft(host::ancestor_of_type(&atta, "BRAN"))?.flatten() else {
            issues.push(format!(
                "ATTA {} 名字以支架名开头，但不在 BRAN 之下，跳过",
                wire_refno(refno)
            ));
            continue;
        };
        if branches
            .as_ref()
            .is_some_and(|branches| !branches.contains(&bran.refno()))
        {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.refno() == bran.refno())
        {
            Some((_, attas)) => attas.push(atta),
            None => groups.push((bran, vec![atta])),
        }
    }
    let mut out = Vec::new();
    for (bran, attas) in groups {
        for atta in attas {
            match pipe_atta(&atta, &bran) {
                Ok(view) => out.push(view),
                Err(error) => issues.push(format!(
                    "管道附件 ATTA {} 读不成（{error}），跳过",
                    wire_refno(atta.refno())
                )),
            }
        }
    }
    Ok(out)
}

fn pipe_atta(atta: &DbElement, bran: &DbElement) -> Result<SupportPipeAtta, DbElementError> {
    let mut out = SupportPipeAtta {
        refno: wire_refno(atta.refno()),
        name: soft(atta.stored_name())?.flatten(),
        branch: soft(bran.stored_name())?.flatten(),
        ..SupportPipeAtta::default()
    };
    let world = host::world_matrix_soft(atta)?;
    let pos = world.map(|world| world.w_axis.truncate());
    out.pos = pos.unwrap_or_default().to_array();
    let points = match (world, soft(catalogue_point_set(atta))?) {
        (Some(world), Some(set)) => set
            .points
            .iter()
            .map(|point| {
                (
                    point.number,
                    world.transform_point3(point.pos),
                    world.transform_vector3(point.dir).try_normalize(),
                    point.bore,
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let find = |number: u32| points.iter().find(|point| point.0 == number);
    let arrive = find(attribute_word(atta, "ARRI")?.unwrap_or(1));
    let leave = find(attribute_word(atta, "LEAV")?.unwrap_or(2));
    out.adir = arrive.and_then(|point| point.2).map(|dir| dir.to_array());
    out.ldir = leave.and_then(|point| point.2).map(|dir| dir.to_array());
    out.abore = arrive.and_then(|point| point.3).filter(|bore| *bore > 0.0);
    out.p3_pos = find(3).map(|point| point.1.to_array());
    let (aod, tube_bore) = pipe_context(atta, bran)?;
    out.aod = aod;
    out.tube_bore = tube_bore;
    if let (Some(world), Some(pos)) = (world, pos) {
        let (has_geometry, legs) = atta_geometry(atta, world, pos, out.adir, out.ldir)?;
        out.has_geometry = has_geometry;
        out.legs = legs;
    }
    Ok(out)
}

/// 附件所在那段管的外径与通径（见模块文档「管道附件」）。
fn pipe_context(
    atta: &DbElement,
    bran: &DbElement,
) -> Result<(Option<f64>, Option<f64>), DbElementError> {
    let members: Vec<DbElement> = bran.members()?.collect();
    let Some(index) = members
        .iter()
        .position(|member| member.refno() == atta.refno())
    else {
        return Ok((None, None));
    };
    let mut route = Vec::with_capacity(members.len());
    for member in &members {
        route.push(soft(noun_of(member))?.as_deref() != Some(ATTA_NOUN));
    }
    let previous = (0..index).rev().find(|k| route[*k]).map(|k| &members[k]);
    let next = (index + 1..members.len())
        .find(|k| route[*k])
        .map(|k| &members[k]);
    let departure = match previous {
        Some(member) => RouteEnd::MemberLeave {
            member: member.clone(),
            ppoint: attribute_word(member, "LEAV")?,
        },
        None => RouteEnd::ContainerHead(bran.clone()),
    };
    let mut aod = soft(departure_tube_outer_diameter(&departure))?.flatten();
    if aod.is_none() {
        aod = soft(departure_tube_outer_diameter(&RouteEnd::MemberLeave {
            member: atta.clone(),
            ppoint: attribute_word(atta, "LEAV")?,
        }))?
        .flatten();
    }
    let positive = |bore: Option<f64>| bore.filter(|bore| bore.is_finite() && *bore > 0.0);
    let mut bore = match previous {
        Some(member) => positive(
            soft(CataloguePoints.leave(member, attribute_word(member, "LEAV")?))?
                .flatten()
                .and_then(|point| point.bore),
        ),
        None => positive(host::optional_double(bran, "HBOR")?),
    };
    if bore.is_none()
        && let Some(member) = next
    {
        bore = positive(
            soft(CataloguePoints.arrive(member, attribute_word(member, "ARRI")?))?
                .flatten()
                .and_then(|point| point.bore),
        );
    }
    Ok((aod.filter(|od| od.is_finite() && *od > 0.0), bore))
}

/// 附件目录里有效的 SRTO / LCYL / SCYL / SSCY（`pipeatta.getattageo` / `getcatamemdata`）：有就带几何，圆柱成 ISOLEG。
fn atta_geometry(
    atta: &DbElement,
    world: DMat4,
    pos: DVec3,
    adir: Option<[f64; 3]>,
    ldir: Option<[f64; 3]>,
) -> Result<(bool, Vec<SupportAttaLeg>), DbElementError> {
    let Some(eval) = soft(CatalogueEval::of(atta))?.flatten() else {
        return Ok((false, Vec::new()));
    };
    let Some(geometry) = geometry_set(eval.component())? else {
        return Ok((false, Vec::new()));
    };
    let straight_through = match (adir, ldir) {
        (Some(a), Some(l)) => {
            DVec3::from_array(a)
                .dot(DVec3::from_array(l))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
                > STRAIGHT_THROUGH_DEG
        }
        _ => false,
    };
    let mut context = PrimitiveContext::new(atta, eval.env(), world);
    let mut has_geometry = false;
    let mut legs = Vec::new();
    for member in geometry.members()? {
        let kind = soft(noun_of(&member))?.unwrap_or_default();
        if !matches!(kind.as_str(), "SRTO" | "LCYL" | "SCYL" | "SSCY") {
            continue;
        }
        let primitive = context.primitive(&member)?;
        if !plant_mbd::support::em9::parts::primitive_valid(&primitive) {
            continue;
        }
        if kind == "SRTO" {
            let dia = number(&member, "PDIA", eval.env())?.unwrap_or(0.0);
            let height = number(&member, "PHEI", eval.env())?.unwrap_or(0.0);
            if dia != 0.0 && height != 0.0 {
                has_geometry = true;
            }
            continue;
        }
        let (Some(centre), Some(axis), Some(height), Some(diameter)) = (
            primitive.center.map(DVec3::from_array),
            primitive.axis.map(DVec3::from_array),
            primitive.height,
            primitive.diameter,
        ) else {
            continue;
        };
        if height == 0.0 || diameter == 0.0 {
            continue;
        }
        let start = centre - axis * (height / 2.0);
        let end = centre + axis * (height / 2.0);
        let mut negatives = false;
        for child in member.members()? {
            if matches!(
                soft(noun_of(&child))?.as_deref(),
                Some("NLCY" | "NSCY" | "NSSL")
            ) {
                negatives = true;
                break;
            }
        }
        if !negatives && straight_through && on_segment(start, end, pos) {
            continue;
        }
        has_geometry = true;
        legs.push(SupportAttaLeg {
            refno: wire_refno(atta.refno()),
            start: start.to_array(),
            end: end.to_array(),
            outdia: diameter,
        });
    }
    Ok((has_geometry, legs))
}

/// PML `line.onprojected(pos)`：点投到线段上落在两端之间。
fn on_segment(start: DVec3, end: DVec3, point: DVec3) -> bool {
    let axis = end - start;
    let length2 = axis.length_squared();
    if length2 <= f64::EPSILON {
        return false;
    }
    let t = (point - start).dot(axis) / length2;
    (-1e-9..=1.0 + 1e-9).contains(&t)
}

// ---- 名表 ----------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct NameKey {
    file: PathBuf,
    sesno: u32,
    len: u64,
    modified: Option<SystemTime>,
}

/// 一个库在所钉会话的 NAME 表：存名升序（按字节）→ refno。
type NameIndex = Arc<Vec<(String, RefNo)>>;

/// NAME 表按库文件 + 会话 + 文件身份缓存；槽位与 SPAMAP 缓存同量级（一座 MDB 的设计库都放得下）。
const NAME_CACHE_SLOTS: usize = 64;

type NameSlots = Vec<(NameKey, Option<NameIndex>)>;

fn name_slots() -> MutexGuard<'static, NameSlots> {
    static SLOTS: OnceLock<Mutex<NameSlots>> = OnceLock::new();
    SLOTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug)]
pub enum NameIndexError {
    Db(DbElementError),
    /// NAME 表读不动（文件开不了、表结构坏）。
    Unreadable {
        dbnum: u32,
        file: PathBuf,
        detail: String,
    },
}

impl std::fmt::Display for NameIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(error) => write!(f, "{error}"),
            Self::Unreadable {
                dbnum,
                file,
                detail,
            } => write!(
                f,
                "库 {dbnum} 的 NAME 表（{}）读不动：{detail}",
                file.display()
            ),
        }
    }
}

impl From<DbElementError> for NameIndexError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// `dbnum` 在 `set` 所钉会话的 NAME 表；会话里没有这张表给 `None`。另开一个 `DbView` 按同一会话读。
pub fn name_index(set: &DbSet, dbnum: u32) -> Result<Option<NameIndex>, NameIndexError> {
    let source = set.metadata(dbnum)?.source;
    let key = NameKey {
        file: source.file,
        sesno: source.sesno,
        len: source.len,
        modified: source.modified,
    };
    if let Some((_, hit)) = name_slots().iter().find(|(slot, _)| *slot == key) {
        return Ok(hit.clone());
    }
    let fail = |detail: String| NameIndexError::Unreadable {
        dbnum,
        file: key.file.clone(),
        detail,
    };
    let mut view = DbView::open_at(&key.file, key.sesno)
        .map_err(|error| fail(format!("open at session {}: {error}", key.sesno)))?;
    let format = view.text_format();
    let loaded = match NamedTable::open(&mut view, name_table_hash()) {
        Err(e3d_io::index::NamedTableError::NoSuchTable { .. }) => None,
        Err(error) => return Err(fail(error.to_string())),
        Ok(mut table) => {
            let mut names = Vec::new();
            for entry in table.iter() {
                let entry = entry.map_err(|error| fail(error.to_string()))?;
                names.push((entry.key.text_as(format), entry.refno));
            }
            names.sort_by(|a, b| a.0.cmp(&b.0));
            Some(Arc::new(names))
        }
    };
    let mut slots = name_slots();
    if let Some((_, cached)) = slots.iter().find(|(slot, _)| *slot == key) {
        return Ok(cached.clone());
    }
    slots.push((key, loaded.clone()));
    if slots.len() > NAME_CACHE_SLOTS {
        slots.remove(0);
    }
    Ok(loaded)
}

/// `DbSet` 里全部 DESI 库中存名以 `prefix` 开头的元素（库号升序、各库内按名字）。读不动 NAME 表的库留一句话跳过。
pub fn named_with_prefix(
    set: &Arc<DbSet>,
    prefix: &str,
    issues: &mut Vec<String>,
) -> Result<Vec<RefNo>, DbElementError> {
    let mut out = Vec::new();
    for dbnum in design_dbs(set) {
        let index = match name_index(set, dbnum) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(NameIndexError::Db(error)) => return Err(error),
            Err(error) => {
                issues.push(format!("{error}：库 {dbnum} 里的管道附件没参与按名查找"));
                continue;
            }
        };
        let start = index.partition_point(|(name, _)| name.as_str() < prefix);
        out.extend(
            index[start..]
                .iter()
                .take_while(|(name, _)| name.starts_with(prefix))
                .map(|(_, refno)| *refno),
        );
    }
    Ok(out)
}

// ---- 找墙 ----------------------------------------------------------------------------------------

/// 找墙基准点，见模块文档「墙板与找墙」：EM9 仪表支架是求解器构件取点的包围盒中心（没有时支架包围盒中心），EM4 是按
/// 名字排第一的点附件，其余不找墙（`None`）。
pub fn locate_base(annotation_type: AnnotationType, view: &SupportQueryResult) -> Option<DVec3> {
    match annotation_type {
        AnnotationType::Em9InstSupp => {
            let (parts, _) = collect_parts(view);
            location_position(&parts).or_else(|| {
                view.bbox
                    .map(|bbox| (DVec3::from_array(bbox.min) + DVec3::from_array(bbox.max)) / 2.0)
            })
        }
        AnnotationType::Em4Supp => view
            .pipe_attas
            .iter()
            .filter(|atta| !atta.has_geometry)
            .min_by(|a, b| {
                a.name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.name.as_deref().unwrap_or(""))
            })
            .map(|atta| DVec3::from_array(atta.pos)),
        _ => None,
    }
}
