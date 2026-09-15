//! `POST /api/v1/element/plines` —— 型材（SCTN / GENSEC）的 **PLINE 线**，给 plant3d-web 测量
//! 拾取层的 Pline 过滤器用（E3D `EDGPLINE.line(dbRef, pline)` = `PLSTART pline → PLEND pline`）。
//!
//! gen-model-v1 的读透形态只存烘好的网格：PLINE 是目录 `SPRF.PSTR`（PTSS）里的二维 p-line
//! （`PLIN`：`PKEY` / `PX` / `PY` / `PLAX`），从来没有落库，前端从网格上也认不出「哪条棱是 NA」。
//! 这条端点不走库：与 `element/ptset` 同一口径直读 dabacon，复用 e3d-model 建型材实体的那条链
//! （`section_catalogue → evaluate_section_profile → section_placement / spine_path`），把每条
//! p-line 放进截面标架再乘世界矩阵——与画出来的梁是**同一份**对齐（`JUSL`）、镜像（`LMIRR`）、
//! `BANG` 与中性轴旋转，所以线一定落在梁的实体上。
//!
//! 口径（E3D 3.1 PMLLIB `edgpline.pmlobj`）：`line()` 缺省 `cut = false`，取 `plStart / plEnd`——
//! p-line 在 `POSS` / `POSE` 两个截面平面上的点，**不**按 `DRNS` / `DRNE` 斜切；斜切后的端点
//! （`plStCut / plEnCut`）另给在 `start_cut` / `end_cut`（只在端面不垂直于轴时出现）。
//! GENSEC 只接直 SPINE（一段 `POINSP → POINSP`）；含弧的 SPINE 在 E3D 里是 `arc()` 不是 `line()`，
//! 这里回空并说明原因。
//!
//! **Significant Snap Points**（E3D Pick Settings「Sections & Walls」的 Fittings / Joints / Nodes 三档）：
//! `EDGPLINE.snapLine` 在 `intermediates` 开着时 `COLLECT ALL (FITT | SJOI SUBJ | SNOD) FOR sctn`，把每个成员的
//! `position` 用 `LINE.near` 投到 p-line 上、按到起点的距离排序，Snap / Distance / Proportion / Fraction 只作用在
//! 光标所在的那一小段。分不分段、哪几类算，是前端 Pick Settings 的事；这里只把**成员与它的位置**给出来
//! （`snap_points`）：SNOD 的 `ZDIS` 是沿轴离 `POSS` 的距离，位置取对齐线（`JUSL`）上 z = ZDIS 的点；SJOI / SUBJ
//! 自己没有 ZDIS，位置随属主链上最近的 SNOD（E3D 的关节挂在节点上）；FITT 的位置走 `section_fitting_placement`
//! （Core3D GFIORG：POSL 线上 z = ZDIS 再加 `DELP`），与画出来的配件同一处。只 SCTN 给——E3D 的 GENSEC 分支
//! （`snap()` GENSEC）不调 `snapLine`，GENSEC 一律空表。
//!
//! 响应形状（mm，世界系）：
//!
//! ```json
//! {
//!   "source": "e3d-model",
//!   "refno": "24381/177301", "dbnum": 7997, "noun": "SCTN", "name": "/…", "unit": "mm",
//!   "justification_line": "NA",
//!   "plines": [{ "key": "NA", "offset": [0.0, 0.0], "start": [x,y,z], "end": [x,y,z],
//!                "dir": [x,y,z], "length": 3000.0, "start_cut": [x,y,z], "end_cut": [x,y,z] }],
//!   "snap_points": [{ "refno": "24381/177316", "noun": "SNOD", "kind": "node", "zdis": 435.56, "position": [x,y,z] },
//!                   { "refno": "24381/177317", "noun": "SJOI", "kind": "joint", "zdis": 435.56, "position": [x,y,z] }],
//!   "reason": null,
//!   "notes": []
//! }
//! ```
//!
//! 状态语义：
//! - `200`：构件存在。不是 SCTN / GENSEC、没有 `SPRE`、目录链断、SPRF 无 `PSTR`、GENSEC 含弧 →
//!   `plines` 为空、`reason` 说明（E3D 同样没有 p-line 可拾），不是错。
//! - `400 bad_request`：refno 写法不对；`404 not_found`：refno 在所钉会话里不存在；`500`：读库出错。

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::refno::RefNo;
use e3d_model::section::{
    SectionChain, SectionCutPlane, SectionCuts, SectionError, SectionPlacement, SectionProfile,
    evaluate_section_profile, placement_between, section_catalogue, section_cut_planes,
    section_fitting_placement, section_placement, spine_path,
};
use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};

use super::handlers::{ProjectReq, resolve_identity};
use super::{ApiError, AppState};
use crate::fast_model::e3d_model_service::parse_refno;
use crate::mbd::wire_refno;

#[derive(Debug, Deserialize)]
pub struct ElementPlinesReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub refno: String,
}

/// 一条 p-line 在世界系里的线（mm）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlineDto {
    /// `PKEY`，大写（`NA` / `TOS` / `BOS` / `LTOS` …）。
    pub key: String,
    /// 在截面平面里、`JUSL` 对齐与 `LMIRR` 镜像之后的位置 `(x, y)`——`JUSL` 那条线恒为 `(0, 0)`。
    pub offset: [f64; 2],
    /// `PLSTART pline`：p-line 在 `POSS` 截面平面上的点。
    pub start: [f64; 3],
    /// `PLEND pline`：p-line 在 `POSE` 截面平面上的点。
    pub end: [f64; 3],
    /// `unit(end − start)`。
    pub dir: [f64; 3],
    /// `|end − start|`。
    pub length: f64,
    /// `PLSTCUT pline`：按 `DRNS` 斜切后的起点；端面垂直于轴（平头）时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_cut: Option<[f64; 3]>,
    /// `PLENCUT pline`：按 `DRNE` 斜切后的终点；同上。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_cut: Option<[f64; 3]>,
}

/// E3D Pick Settings「Significant Snap Points」的一个分段点来源：SCTN 名下的 FITT / SJOI / SUBJ / SNOD
/// （`EDGPLINE.snapLine` 的 `COLLECT ALL (FITT | SJOI SUBJ | SNOD) FOR sctn`）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SectionSnapPointDto {
    /// `a/b`
    pub refno: String,
    /// `FITT` / `SJOI` / `SUBJ` / `SNOD`
    pub noun: String,
    /// Pick Settings 的三档：`fitting`（FITT）/ `joint`（SJOI、SUBJ）/ `node`（SNOD）。
    pub kind: &'static str,
    /// 沿轴离 `POSS` 的距离（mm）：自己的 `ZDIS`，SJOI / SUBJ 取属主链上最近的 SNOD 的。
    pub zdis: f64,
    /// 世界系位置（mm）：SNOD / SJOI / SUBJ 是对齐线上 z = ZDIS 的点，FITT 是它的目录标架原点
    /// （`section_fitting_placement`）。前端按 E3D `LINE.near` 投到各 p-line 上得到分段点。
    pub position: [f64; 3],
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementPlinesResponse {
    pub source: &'static str,
    /// `a/b`
    pub refno: String,
    pub dbnum: u32,
    pub noun: String,
    pub name: Option<String>,
    pub unit: &'static str,
    /// 实际用到的对齐线名（`JUSL`，缺省 `NA`）；没走到截面时为 null。
    pub justification_line: Option<String>,
    /// `PSTR` 成员原序。
    pub plines: Vec<PlineDto>,
    /// SCTN 名下的 FITT / SJOI / SUBJ / SNOD，按 `zdis` 升序；GENSEC 与没有 p-line 时为空。
    pub snap_points: Vec<SectionSnapPointDto>,
    /// `plines` 为空时的原因（E3D 同样没有 p-line 可拾）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 截面求值 / SPINE 建路径过程中的非致命记录（与实体生成的 notes 同一来源）。
    pub notes: Vec<String>,
}

/// POST /api/v1/element/plines
pub async fn element_plines(
    State(state): State<AppState>,
    Json(req): Json<ElementPlinesReq>,
) -> Result<Json<ElementPlinesResponse>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let parsed = parse_refno(&req.refno).map_err(|error| {
        ApiError::bad_request(format!("refno `{}` 无法解析：{error:#}", req.refno))
    })?;
    let service = super::mbd::service().await?;
    let set = service
        .build_set(parsed.dbno(), None)
        .map_err(ApiError::from_domain)?;
    tokio::task::spawn_blocking(move || respond(&set, parsed))
        .await
        .map_err(|join| ApiError::from_domain(anyhow::anyhow!("plines task aborted: {join}")))?
        .map(Json)
}

/// 与 `AppState` / MDB 视图无关的主体：给定一个 `DbSet` 就能答。
pub fn respond(set: &Arc<DbSet>, refno: RefNo) -> Result<ElementPlinesResponse, ApiError> {
    let element = set.element(refno);
    // 库没注册进这个 DbSet（refno 的 db 不在项目 / MDB 里）与索引里没有它，对调用方都是「没有这个构件」。
    let exists = match element.exists() {
        Ok(exists) => exists,
        Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => false,
        Err(error) => return Err(db_error(error)),
    };
    if !exists {
        return Err(ApiError::not_found(format!(
            "refno {} 在所钉会话里不存在",
            wire_refno(refno)
        )));
    }
    element_dto(&element).map_err(|error| match error {
        SectionError::Db(db) => db_error(db),
        other => ApiError::from_domain(other.into()),
    })
}

/// 截面标架就位后的一段：`local` 把截面平面 `(x, y, z)` 放进构件（属主）局部系，`z ∈ [0, length]`。
struct Frame {
    placement: SectionPlacement,
    /// SCTN 才有两端斜切面；GENSEC 的 `DRNS` / `DRNE` 在 SPINE 上，本期不接。
    cuts: Option<SectionCuts>,
}

fn element_dto(element: &DbElement) -> Result<ElementPlinesResponse, SectionError> {
    let refno = element.refno();
    let noun = element.element_type()?;
    let mut response = ElementPlinesResponse {
        source: "e3d-model",
        refno: pdms_refno(refno),
        dbnum: refno.dbno(),
        noun: noun.clone(),
        name: element.stored_name()?,
        unit: "mm",
        justification_line: None,
        plines: Vec::new(),
        snap_points: Vec::new(),
        reason: None,
        notes: Vec::new(),
    };
    if noun != "SCTN" && noun != "GENSEC" {
        response.reason = Some(format!("{noun} 不是 SCTN / GENSEC，没有 PLINE"));
        return Ok(response);
    }
    let catalogue = match section_catalogue(element)? {
        SectionChain::NoSpre => {
            response.reason = Some("无 SPRE：E3D 只画中心线，没有目录 p-line".to_string());
            return Ok(response);
        }
        SectionChain::Broken { hop, at, why } => {
            response.reason = Some(format!("SPRE 链第 {hop} 跳断在 {at}：{why}"));
            return Ok(response);
        }
        SectionChain::Resolved(catalogue) => catalogue,
    };
    let profile = evaluate_section_profile(element, &catalogue)?;
    response.notes.extend(profile.notes.iter().cloned());
    response.justification_line = Some(profile.justification_line.clone());

    let frame = match section_frame(element, &noun, &profile, &mut response.notes)? {
        Ok(frame) => frame,
        Err(reason) => {
            response.reason = Some(reason);
            return Ok(response);
        }
    };
    let world = e3d_model::transform::world_matrix(element)? * frame.placement.local;

    let Some(ptss) = catalogue.sprf.get_element("PSTR")? else {
        response.reason = Some(format!(
            "SPRF {} 无 PSTR（p-line 集）",
            catalogue.sprf.refno()
        ));
        return Ok(response);
    };
    let env = DbElementParamEnv::new(Some(element.clone()), Some(catalogue.sprf.clone()))
        .without_insulation();
    for pline in ptss.members()? {
        if !pline.element_type()?.eq_ignore_ascii_case("PLIN") {
            continue;
        }
        let key = pline
            .get_string("PKEY")
            .ok()
            .flatten()
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase();
        if key.is_empty() {
            response
                .notes
                .push(format!("PLIN {} 没有 PKEY，跳过", pline.refno()));
            continue;
        }
        let raw = [number(&pline, "PX", &env), number(&pline, "PY", &env)];
        let offset = place_in_section(raw, &profile);
        response.plines.push(pline_dto(key, offset, &frame, world));
    }
    if response.plines.is_empty() {
        response.reason = Some(format!("PSTR {} 里没有 PLIN", ptss.refno()));
        return Ok(response);
    }
    // E3D `snapLine` 只在 SCTN 分支收 FITT / SJOI / SUBJ / SNOD；GENSEC 的 `snap()` 不分段。
    if noun == "SCTN" {
        let section_world = e3d_model::transform::world_matrix(element)?;
        collect_snap_points(
            element,
            element,
            None,
            section_world,
            world,
            &mut response.snap_points,
            &mut response.notes,
        )?;
        sort_snap_points(&mut response.snap_points);
    }
    Ok(response)
}

/// Pick Settings 三档对 noun 的归类（`EDGPLINE.snapLine` 的 `searchList`：
/// fitting → `FITT`，joint → `SJOI SUBJ`，node → `SNOD`）；其余成员不是分段点。
fn snap_point_kind(noun: &str) -> Option<&'static str> {
    match noun {
        "FITT" => Some("fitting"),
        "SJOI" | "SUBJ" => Some("joint"),
        "SNOD" => Some("node"),
        _ => None,
    }
}

/// E3D 按到起点的距离排（`distances.sortedIndices()`）；同一 ZDIS 上节点在前、关节在后（关节挂在节点上），
/// 再按 refno 稳定。
fn sort_snap_points(points: &mut [SectionSnapPointDto]) {
    points.sort_by(|a, b| {
        a.zdis
            .total_cmp(&b.zdis)
            .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then_with(|| a.refno.cmp(&b.refno))
    });
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "node" => 0,
        "joint" => 1,
        _ => 2,
    }
}

/// `COLLECT ALL (FITT SJOI SUBJ SNOD) FOR sctn` 的递归：走遍 SCTN 名下全部后代，
/// `inherited_zdis` 是属主链上最近的 `ZDIS`（SNOD → SJOI → SUBJ 一路带下去）。
/// `section_world` 是 SCTN 的世界矩阵（FITT 标架在属主局部系），`jusl_world` 是对齐线标架
/// （`world_matrix × placement.local`，z 沿轴、原点在 `POSS` 平面）。
fn collect_snap_points(
    section: &DbElement,
    owner: &DbElement,
    inherited_zdis: Option<f64>,
    section_world: DMat4,
    jusl_world: DMat4,
    out: &mut Vec<SectionSnapPointDto>,
    notes: &mut Vec<String>,
) -> Result<(), SectionError> {
    for member in owner.members()? {
        let noun = member.element_type()?;
        // SNOD / FITT 自己带 ZDIS；SJOI / SUBJ / LNKS … 的模板里根本没有这个属性——那不是错，
        // 沿属主链继承（实机 24381/177317 SJOI、24381/177362 LNKS 曾把整条响应打成 500）。
        let own_zdis = match member.get_double("ZDIS") {
            Ok(value) => value.filter(|value| value.is_finite()),
            Err(DbElementError::UnknownAttribute { .. }) => None,
            Err(error) => return Err(error.into()),
        };
        let zdis = own_zdis.or(inherited_zdis);
        if let Some(kind) = snap_point_kind(&noun) {
            match zdis {
                Some(zdis) => {
                    let position = if noun == "FITT" {
                        match section_fitting_placement(section, &member) {
                            Ok(placement) => {
                                (section_world * placement).transform_point3(DVec3::ZERO)
                            }
                            Err(error) => {
                                notes.push(format!(
                                    "FITT {} 的目录标架算不出（{error}），分段点退到对齐线 z = ZDIS",
                                    member.refno()
                                ));
                                jusl_world.transform_point3(DVec3::new(0.0, 0.0, zdis))
                            }
                        }
                    } else {
                        jusl_world.transform_point3(DVec3::new(0.0, 0.0, zdis))
                    };
                    out.push(SectionSnapPointDto {
                        refno: pdms_refno(member.refno()),
                        noun: noun.clone(),
                        kind,
                        zdis,
                        position: position.to_array(),
                    });
                }
                None => notes.push(format!(
                    "{noun} {} 自身与属主链都没有 ZDIS，不作 Significant Snap 分段点",
                    member.refno()
                )),
            }
        }
        collect_snap_points(
            section,
            &member,
            zdis,
            section_world,
            jusl_world,
            out,
            notes,
        )?;
    }
    Ok(())
}

/// SCTN：`POSS → POSE` 标架 + 两端切面；GENSEC：直 SPINE 的两端 + `BANG`，含弧的不接。
fn section_frame(
    element: &DbElement,
    noun: &str,
    profile: &SectionProfile,
    notes: &mut Vec<String>,
) -> Result<Result<Frame, String>, SectionError> {
    if noun == "GENSEC" {
        let Some(path) = spine_path(element)? else {
            return Ok(Err(
                "没有 SPINE：E3D 同样不出实体，也没有 p-line 可拾".to_string()
            ));
        };
        notes.extend(path.problems.iter().cloned());
        let Some((start, end)) = path.as_straight() else {
            return Ok(Err(format!(
                "GENSEC 的 SPINE 不是单一直段（{} 段，弧型 {}）：E3D 把它的 p-line 当弧（edgPline.arc），未接",
                path.segments.len(),
                if path.curve_types.is_empty() {
                    "无".to_string()
                } else {
                    path.curve_types.join(",")
                }
            )));
        };
        let bang_deg = element.get_double("BANG")?.unwrap_or(0.0);
        return Ok(
            match placement_between(
                &element.refno().to_string(),
                noun,
                start,
                end,
                bang_deg,
                profile,
            ) {
                Ok(placement) => Ok(Frame {
                    placement,
                    cuts: None,
                }),
                Err(SectionError::EmptyLength { .. }) => {
                    Err("SPINE 两端重合（长度 ≤ 0.001 mm），没有 p-line".to_string())
                }
                Err(error) => return Err(error),
            },
        );
    }
    let placement = match section_placement(element, profile) {
        Ok(placement) => placement,
        Err(SectionError::EmptyLength { .. }) => {
            return Ok(Err(
                "POSS / POSE 重合（长度 ≤ 0.001 mm），没有 p-line".to_string()
            ));
        }
        Err(error) => return Err(error),
    };
    let cuts = section_cut_planes(element, &placement)?;
    Ok(Ok(Frame {
        placement,
        cuts: Some(cuts),
    }))
}

/// 目录 `PX` / `PY`：表达式经设计件 + SPRF 的参数环境求值，未设 / 解不出按 0（与 `member_frame` 同口径）。
fn number(element: &DbElement, name: &str, env: &DbElementParamEnv) -> f64 {
    element
        .get_number(name, env)
        .ok()
        .and_then(|outcome| outcome.value())
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

/// p-line 的 `(PX, PY)` → 截面平面里的最终位置：先减 `JUSL` 对齐量，再按 `LMIRR` 沿 x 镜像
/// （与 `section::placed_profile_rings` 对截面顶点做的完全一样，所以线与实体同一张截面）。
fn place_in_section(raw: [f64; 2], profile: &SectionProfile) -> [f64; 2] {
    let [jx, jy] = profile.justification;
    let x = raw[0] - jx;
    [if profile.mirrored { -x } else { x }, raw[1] - jy]
}

/// 斜切端点与平头端点差在这个数以内就当同一点（mm）——E3D 的位置本来就记到 0.01 mm，差不到它不是另一个点。
/// 不能取更严：`DRNS` 是精确的方向（实机 `24381/177301` 恰是 5°，`sin 5° = 0.0871557427…`），而轴向由记到
/// 0.01 mm 的 `POSS / POSE` 算出，两者夹角带 1e-7 ～ 1e-5 rad 的记录误差；平头梁上离轴 50 mm 的 `LTOS` 因此
/// 交出一个偏 1.6e-5 mm 的「斜切点」，按 1e-6 mm 的门就被当成真斜切给了出去。
const CUT_COINCIDENCE_MM: f64 = 0.01;

/// 截面点 `(x, y)` 那条平行于轴的棱与切面的交点 z（`SectionCutPlane::axis_crossing` 的口径）；
/// 平头端面（法向轴向）不出。
fn cut_crossing(plane: &SectionCutPlane, x: f64, y: f64) -> Option<f64> {
    if plane.is_axial() || plane.normal.z.abs() <= 1e-9 {
        return None;
    }
    Some(
        plane.point.z
            - (plane.normal.x * (x - plane.point.x) + plane.normal.y * (y - plane.point.y))
                / plane.normal.z,
    )
}

fn pline_dto(key: String, offset: [f64; 2], frame: &Frame, world: DMat4) -> PlineDto {
    let [x, y] = offset;
    let at = |z: f64| world.transform_point3(DVec3::new(x, y, z)).to_array();
    let start = at(0.0);
    let end = at(frame.placement.length);
    let delta = DVec3::from_array(end) - DVec3::from_array(start);
    let length = delta.length();
    let dir = if length > 0.0 {
        (delta / length).to_array()
    } else {
        [0.0, 0.0, 0.0]
    };
    // 斜切端点与平头端点重合（DRNS / DRNE 就是轴向，只差浮点尾数）时不另给——字段出现即表示端面真斜。
    let distinct = |cut: [f64; 3], plain: [f64; 3]| {
        (DVec3::from_array(cut) - DVec3::from_array(plain)).length() > CUT_COINCIDENCE_MM
    };
    let (start_cut, end_cut) = match &frame.cuts {
        Some(cuts) => (
            cut_crossing(&cuts.start, x, y)
                .map(at)
                .filter(|cut| distinct(*cut, start)),
            cut_crossing(&cuts.end, x, y)
                .map(at)
                .filter(|cut| distinct(*cut, end)),
        ),
        None => (None, None),
    };
    PlineDto {
        key,
        offset,
        start,
        end,
        dir,
        length,
        start_cut,
        end_cut,
    }
}

/// `a/b`，与 `element/ptset` 同一写法。
fn pdms_refno(refno: RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1 as i32)
}

fn db_error(error: DbElementError) -> ApiError {
    ApiError::from_domain(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空库上的 refno 是 404，不是 500：前端据此负缓存。
    #[test]
    fn a_missing_refno_is_not_found() {
        let set = Arc::new(DbSet::new(Arc::new(e3d_attlib::AttlibData::default())));
        let error = respond(&set, RefNo::new(1, 1)).unwrap_err();
        assert_eq!(error.code, "not_found", "{}", error.message);
    }

    /// 请求体：只要 `refno`，身份三元组可省，两种写法都收。
    #[test]
    fn request_accepts_both_refno_spellings() {
        let req: ElementPlinesReq = serde_json::from_str(r#"{"refno":"24381/177301"}"#).unwrap();
        assert_eq!(req.refno, "24381/177301");
        let req: ElementPlinesReq = serde_json::from_str(r#"{"refno":"24381_177301"}"#).unwrap();
        assert_eq!(req.refno, "24381_177301");
    }

    fn profile(justification: [f64; 2], mirrored: bool) -> SectionProfile {
        SectionProfile {
            members: Vec::new(),
            justification,
            justification_line: "NA".into(),
            justification_found: true,
            neutral_axis_spin: 0.0,
            level: 0,
            mirrored,
            notes: Vec::new(),
        }
    }

    /// `JUSL` 那条线自己落在 (0, 0)；其余线先减对齐量，`LMIRR` 再沿 x 镜像——与截面顶点同一套。
    #[test]
    fn plines_are_aligned_and_mirrored_like_the_profile() {
        let plain = profile([10.0, -5.0], false);
        assert_eq!(place_in_section([10.0, -5.0], &plain), [0.0, 0.0]);
        assert_eq!(place_in_section([40.0, 20.0], &plain), [30.0, 25.0]);
        let mirrored = profile([10.0, -5.0], true);
        assert_eq!(place_in_section([40.0, 20.0], &mirrored), [-30.0, 25.0]);
        assert_eq!(place_in_section([10.0, -5.0], &mirrored), [0.0, 0.0]);
    }

    /// 平头梁：`start` / `end` 就是截面点沿轴推到 0 与 L，`dir` 是轴向，没有 cut 字段；
    /// 斜切端面：`start_cut` / `end_cut` 是同一条棱与切面的交点。
    #[test]
    fn pline_endpoints_follow_the_section_frame() {
        let local = DMat4::from_translation(DVec3::new(100.0, 200.0, 300.0));
        let placement = SectionPlacement {
            local,
            length: 3000.0,
            bang_deg: 0.0,
        };
        let axial = SectionCuts {
            start: SectionCutPlane {
                point: DVec3::ZERO,
                normal: DVec3::Z,
            },
            end: SectionCutPlane {
                point: DVec3::new(0.0, 0.0, 3000.0),
                normal: -DVec3::Z,
            },
        };
        let frame = Frame {
            placement,
            cuts: Some(axial),
        };
        // 世界矩阵再平移 (0, 0, 1000)。
        let world = DMat4::from_translation(DVec3::new(0.0, 0.0, 1000.0)) * local;
        let dto = pline_dto("TOS".into(), [0.0, 50.0], &frame, world);
        assert_eq!(dto.start, [100.0, 250.0, 1300.0]);
        assert_eq!(dto.end, [100.0, 250.0, 4300.0]);
        assert_eq!(dto.dir, [0.0, 0.0, 1.0]);
        assert_eq!(dto.length, 3000.0);
        assert!(dto.start_cut.is_none() && dto.end_cut.is_none());

        // 起端切面法向 (1, 0, 1)/√2 过原点：棱 (x = 0, y = 50) 与它交在 z = 0；棱 (x = 40) 交在 z = −40。
        let skew = SectionCuts {
            start: SectionCutPlane {
                point: DVec3::ZERO,
                normal: DVec3::new(1.0, 0.0, 1.0).normalize(),
            },
            end: axial.end,
        };
        let frame = Frame {
            placement,
            cuts: Some(skew),
        };
        let dto = pline_dto("NA".into(), [40.0, 0.0], &frame, world);
        assert_eq!(dto.start, [140.0, 200.0, 1300.0]);
        let cut = dto.start_cut.expect("斜切端面给 start_cut");
        assert!((cut[2] - 1260.0).abs() < 1e-9, "{cut:?}");
        assert!(dto.end_cut.is_none());

        // 「轴向」端面的法向与轴只差记录误差（实机 `24381/177301`：DRNS 精确 5°，POSS / POSE 记到 0.01 mm，
        // 夹角 3e-7 rad；这里取 1e-5 rad 的上界）：离轴 50 mm 的棱交出的点偏不到 0.01 mm，不给 cut 字段。
        let nearly_axial = SectionCuts {
            start: SectionCutPlane {
                point: DVec3::ZERO,
                normal: DVec3::new(1e-5, 0.0, 1.0).normalize(),
            },
            end: axial.end,
        };
        let frame = Frame {
            placement,
            cuts: Some(nearly_axial),
        };
        let dto = pline_dto("LTOS".into(), [-50.0, 100.0], &frame, world);
        assert!(dto.start_cut.is_none() && dto.end_cut.is_none(), "{dto:?}");
        // 真斜切（1°）在同一条棱上偏 50·tan 1° ≈ 0.87 mm，照给。
        let one_degree = SectionCuts {
            start: SectionCutPlane {
                point: DVec3::ZERO,
                normal: DVec3::new(1f64.to_radians().tan(), 0.0, 1.0).normalize(),
            },
            end: axial.end,
        };
        let frame = Frame {
            placement,
            cuts: Some(one_degree),
        };
        let dto = pline_dto("LTOS".into(), [-50.0, 100.0], &frame, world);
        let cut = dto.start_cut.expect("1° 斜切给 start_cut");
        assert!(
            (cut[2] - (1300.0 + 50.0 * 1f64.to_radians().tan())).abs() < 1e-9,
            "{cut:?}"
        );
    }

    /// 响应：`plines` 空时带 `reason`，非空时省略；cut 字段缺省不出现。
    #[test]
    fn response_shape() {
        let response = ElementPlinesResponse {
            source: "e3d-model",
            refno: "24381/177301".into(),
            dbnum: 7997,
            noun: "SCTN".into(),
            name: None,
            unit: "mm",
            justification_line: Some("NA".into()),
            plines: vec![PlineDto {
                key: "NA".into(),
                offset: [0.0, 0.0],
                start: [0.0, 0.0, 0.0],
                end: [0.0, 0.0, 3000.0],
                dir: [0.0, 0.0, 1.0],
                length: 3000.0,
                start_cut: None,
                end_cut: None,
            }],
            snap_points: vec![SectionSnapPointDto {
                refno: "24381/177316".into(),
                noun: "SNOD".into(),
                kind: "node",
                zdis: 435.56,
                position: [0.0, 0.0, 435.56],
            }],
            reason: None,
            notes: Vec::new(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["source"], "e3d-model");
        assert_eq!(json["plines"][0]["key"], "NA");
        assert!(json["plines"][0].get("start_cut").is_none());
        assert_eq!(json["snap_points"][0]["kind"], "node");
        assert_eq!(json["snap_points"][0]["zdis"], 435.56);
        assert!(json.get("reason").is_none());
        let empty = ElementPlinesResponse {
            plines: Vec::new(),
            snap_points: Vec::new(),
            reason: Some("无 SPRE".into()),
            ..response
        };
        let json = serde_json::to_value(&empty).unwrap();
        assert_eq!(json["reason"], "无 SPRE");
        assert_eq!(json["plines"].as_array().unwrap().len(), 0);
        assert_eq!(json["snap_points"].as_array().unwrap().len(), 0);
    }

    /// `EDGPLINE.snapLine` 的 `searchList`：Fittings → FITT，Joints → SJOI + SUBJ，Nodes → SNOD；
    /// 其它成员（PJOI / SCOJ / 属性型节点…）不是分段点。
    #[test]
    fn snap_point_kinds_follow_the_e3d_search_list() {
        assert_eq!(snap_point_kind("FITT"), Some("fitting"));
        assert_eq!(snap_point_kind("SJOI"), Some("joint"));
        assert_eq!(snap_point_kind("SUBJ"), Some("joint"));
        assert_eq!(snap_point_kind("SNOD"), Some("node"));
        assert_eq!(snap_point_kind("SCOJ"), None);
        assert_eq!(snap_point_kind("PJOI"), None);
        assert_eq!(snap_point_kind("PLIN"), None);
    }

    /// 按沿轴距离升序；同一 ZDIS 上节点先于挂在它上面的关节，再按 refno 稳定。
    #[test]
    fn snap_points_sort_along_the_axis() {
        let point = |refno: &str, noun: &str, kind: &'static str, zdis: f64| SectionSnapPointDto {
            refno: refno.into(),
            noun: noun.into(),
            kind,
            zdis,
            position: [0.0, 0.0, zdis],
        };
        let mut points = vec![
            point("24381/9", "SJOI", "joint", 435.56),
            point("24381/8", "SNOD", "node", 435.56),
            point("24381/3", "FITT", "fitting", 120.0),
            point("24381/5", "SNOD", "node", 0.0),
        ];
        sort_snap_points(&mut points);
        let order: Vec<_> = points.iter().map(|p| p.refno.as_str()).collect();
        assert_eq!(order, ["24381/5", "24381/3", "24381/8", "24381/9"]);
    }
}
