//! 支架视图的几何取数（plant3d-web#103 M4 / M5 宿主侧，[`super::support_query`] 调用）：型钢（SCTN / GENSEC）的
//! 截面标架、截面点与 PPLI 线，PANE 的基面轮廓 / NXTR 轮廓差 / 弧形护板，以及目录原语按实例参数求值。字段口径见
//! plant-mbd `docs/field-dictionary.md`「支架视图」，坐标一律世界 mm（PML `wrt /*`）。
//!
//! # 型钢
//!
//! 与 `web_service::plines` 同一条截面链（`section_catalogue → evaluate_section_profile → section_placement`，GENSEC
//! 走直 SPINE 的 `placement_between`）：标架 = `world_matrix × SectionPlacement::local`，列 x / y 是截面平面、z 沿
//! `POSS → POSE`，原点在 `POSS`——这就是 PML `ori / pos of $!sctn wrt /*`。截面点（`!!getSctnPoints`）是截面原点
//! （`getsctnpos`：`POS` 退回 `JUSL` 线的 `PX` / `PY`）沿标架 x / y 偏 SPVE 的 `PX` / `PY`，即截面坐标减 `JUSL` 对齐
//! 量（`LMIRR` 时沿 x 镜像，与画出来的梁同一张截面）后乘标架；只取第一个有效（LEVEL 含 6、OBST 0 / 2）的 SPRO（≥ 3
//! 个不同顶点）/ SREC 四角 / PRAD 方框（裁决 D-SUPP9-HOST-01）。PPLI 线（`pos ppli KEY prop 0 / 1`）是那条 p-line 在
//! `POSS` / `POSE` 两个截面平面上的点（不按 `DRNS` / `DRNE` 斜切）。没有 SPRE 的型钢（E3D 只画中心线）照给两端与
//! 端向、标架按空截面放，截面点、PPLI 与原语都空。
//!
//! # 板
//!
//! PML `vertPoints(item, false)`：基面过 PANE 的 `POS`、法向 `ORI z`；第一个 PLOO（没有时 LOOP）的 `SJUS` 含 `TO` 时
//! 厚度方向反过来，含 `CEN` 时基面沿 `ORI z` 下移半厚；PAVE / VERT 的世界位置投到基面上就是 `vertices`（环序原样、
//! 不并圆角），原样的世界位置是 EM4 的 `paves`。板下有 NXTR 时 `cut_vertices` 是轮廓（逐点 FRAD 倒圆）减去各 NXTR
//! 第一个 LOOP 之后的第一块的各边起点（`supportpane.pmlobj` L84-104），在 PANE 的局部平面里做二维布尔、落回基面。
//!
//! 弧形护板（`supppanepad`，没有 PLOO 或同时有 NBOX 与 NCYL 的 PANE）：两面轮廓（`vertPoints(item, true)`：并点并
//! 圆角）的中心连线是管长方向与 `len`，第一面轮廓减去与该方向垂直的 NBOX 矩形、减去以第一面中心为圆心的 NCYL 圆，
//! 剩下的轮廓里半径最大的弧是大弧、另一条是小弧：`thick` = 半径差（1 位小数），`angle_deg` = 大弧扫角，`arc_len` =
//! `R × angle × π / 180`，锚点 = 大弧中点沿管长挪半个 `len`。
//!
//! # 目录原语
//!
//! `gmref of catref`（取不到 `gstref`）的成员原序、含无效的，各带 `LEVE` / `OBST` / `TUFL`。求值环境是设计构件 + 目录
//! 件（[`super::host::CatalogueEval`]），轴（`PAXI` / `PAAX` / `PBAX`）具名的走 `axis_spec::direction`，`Pn` 形的取那个
//! P 点（局部系，`1000 + n` 是反向）；最后乘构件的世界矩阵：
//!
//! - SBOX：中心 `(PX, PY, PZ)`、三向长度 `|PXLE| / |PYLE| / |PZLE|`（沿构件 ORI x / y / z）；
//! - SCYL / SSCY：中心 `轴原点 + 轴向 · (PDIS + PHEI / 2)`、高 `|PHEI|`；LCYL：中心 `轴原点 + 轴向 · (PBDI + PTDI) / 2`、
//!   高 `|PTDI − PBDI|`；直径 `|PDIA|`，轴向原样（PAXI）。构件带保温（`ipara` 非空）时另给 `pdia_bare`：`|PDIA|` 在
//!   不带保温的环境（`IPARAM` 一律答 0）里的值。`CatalogueEval` 的环境本身就不带保温，所以它与直径相同；它告诉求解器
//!   这是裸径，`!!getPipeMaxdia` 不再扣 `IPARA[1]`（plant-mbd `D-SUPP9-021` / `D-SUPP4-020`）；
//! - SEXT：`PAAX` 为 x、`PBAX` 正交化为 y、z = x × y 的标架放在 `轴原点 + R · (PX, PY, PZ)`，第一个 SLOO 的 SVER 顶点
//!   与 PRAD，沿 z 拉伸 `|PHEI|`；
//! - 型钢 GMSS 的 SPRO（SPVE + PRAD）/ SREC（四角）/ SANN（圆心与直径）落在 `POSS` 截面平面上，拉伸方向与长度取标架
//!   z 与 `|POSE − POSS|`；
//! - 其余原语只给类型与有效性。
//!
//! 单个原语、截面、板求不出只记一句话（进视图 `issues`），不拖垮别的；只有读库出错才 `Err`。

use cavalier_contours::polyline::{
    BooleanOp, PlineSource, PlineSourceMut, Polyline, seg_arc_radius_and_center,
};
use e3d_io::db_element::{DbElement, DbElementError};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::record::axis_spec;
use e3d_io::record::descriptor::DescriptorValue;
use e3d_model::section::{
    SectionCatalogue, SectionChain, SectionError, SectionPlacement, SectionProfile,
    evaluate_section_profile, placement_between, section_catalogue, section_placement, spine_path,
};
use e3d_model::{CataloguePoint, catalogue_point_set};
use glam::{DMat3, DMat4, DVec2, DVec3};
use plant_mbd::geom::profile::{ProfileSegment, profile_from_vertices, regularize_vertices};
use plant_mbd::support::view::{
    SupportFlangeLine, SupportHole, SupportPanePad, SupportPlate, SupportPrimitive, SupportSection,
};

use super::host::{self, noun_of, soft};
use super::wire_refno;

/// 型钢翼缘 PPLI（`supportSctn.pmlobj` L38-98）。
pub const FLANGE_LINE_KEYS: [&str; 7] = ["FOC", "LBOC", "LTOC", "LTOS", "LBOS", "RBOS", "RTOS"];
/// 截面中性轴（`supppanes.getcatadirection` L165-168）。
const NEUTRAL_AXIS_KEY: &str = "NA";
/// 截面点去重的判据（PML `array.unique()` 比的是位置值）。
const SAME_POINT_MM: f64 = 1e-6;
const LENGTH_EPS: f64 = 1e-6;
/// Core3D `NAXIS`：`1000 + n` 是反向的 `Pn` 轴（e3d-model `catalogue_point` 同口径）。
const NEGATED_POINT_BASE: u32 = 1000;
/// 轴规格的种类字：`[3, n]` 是 `Pn`（`axis_spec` 模块头）。
const KIND_POINT: u32 = 3;
/// 护板：NBOX / NCYL 的轴与管长方向平行的判据（`supppanepad.pmlobj` L49 / L117 的 0.1°）。
const PAD_AXIS_DEG: f64 = 0.1;

// ---- 属性读法 ------------------------------------------------------------------------------------

/// 按字存的属性（`LEVE`、轴规格 `PAXI` …）；不在模板里、没写都给 `None`。
pub fn attribute_words(
    element: &DbElement,
    name: &str,
) -> Result<Option<Vec<u32>>, DbElementError> {
    let value = match element.attribute(name) {
        Ok(value) => value,
        Err(DbElementError::UnknownAttribute { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(match value {
        Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words)) => Some(words),
        Some(DescriptorValue::IntArray(values)) => {
            Some(values.into_iter().map(|value| value as u32).collect())
        }
        Some(DescriptorValue::Word { raw, .. }) => Some(vec![raw]),
        Some(DescriptorValue::Int(value)) => Some(vec![value as u32]),
        _ => None,
    })
}

/// 一个字值（`LEAV` / `ARRI` / `OBST` 存成 `Word { raw }`）。
pub fn attribute_word(element: &DbElement, name: &str) -> Result<Option<u32>, DbElementError> {
    Ok(attribute_words(element, name)?.and_then(|words| words.first().copied()))
}

/// 目录原语的有效性三件（PML `checkvalid`）：`LEVE` 的上下限、`OBST`、`TUFL`。
fn validity(
    primitive: &DbElement,
) -> Result<(Option<[i32; 2]>, Option<i32>, Option<bool>), DbElementError> {
    let level = attribute_words(primitive, "LEVE")?.and_then(|words| match words[..] {
        [low, high, ..] => Some([low as i32, high as i32]),
        _ => None,
    });
    let obst = attribute_word(primitive, "OBST")?.map(|word| word as i32);
    let tuflag = match primitive.get_bool("TUFL") {
        Ok(value) => value,
        Err(DbElementError::UnknownAttribute { .. } | DbElementError::TypeMismatch { .. }) => None,
        Err(error) => return Err(error),
    };
    Ok((level, obst, tuflag))
}

/// LEVEL 含 6、OBST 是 0 / 2（缺省按有效，`getSctnPoints` L58-66）。
fn level_obst_ok(level: Option<[i32; 2]>, obst: Option<i32>) -> bool {
    level.is_none_or(|[low, high]| low <= 6 && high >= 6)
        && obst.is_none_or(|obst| obst == 0 || obst == 2)
}

/// 目录表达式按环境求值：存的数或求得出的数；unset / 求不出 / 不在模板里都是 `None`。
pub fn number(
    element: &DbElement,
    name: &str,
    env: &DbElementParamEnv,
) -> Result<Option<f64>, DbElementError> {
    match element.get_number(name, env) {
        Ok(outcome) => Ok(outcome.value().filter(|value| value.is_finite())),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 位置类的目录量（`PX` / `PDIS` …）：求不出按 0（e3d-model `catalogue` 对坐标槽位同口径）。
fn offset(element: &DbElement, name: &str, env: &DbElementParamEnv) -> Result<f64, DbElementError> {
    Ok(number(element, name, env)?.unwrap_or(0.0))
}

fn arr(point: DVec3) -> [f64; 3] {
    point.to_array()
}

fn tidy(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

// ---- 目录原语 ------------------------------------------------------------------------------------

/// 一个设计构件的目录求值上下文：环境、世界矩阵与（按需求的）局部 P 点；构件带保温时另有不带保温的环境
/// （`pdia_bare`，见模块文档）。
pub struct PrimitiveContext<'a> {
    member: &'a DbElement,
    env: &'a DbElementParamEnv,
    bare_env: Option<&'a DbElementParamEnv>,
    world: DMat4,
    points: Option<Vec<CataloguePoint>>,
}

impl<'a> PrimitiveContext<'a> {
    pub fn new(member: &'a DbElement, env: &'a DbElementParamEnv, world: DMat4) -> Self {
        Self {
            member,
            env,
            bare_env: None,
            world,
            points: None,
        }
    }

    /// 构件带保温（`ipara` 非空）时给：LCYL / SCYL / SSCY 的 `PDIA` 在这个环境里再求一次，填 `pdia_bare`。
    pub fn with_bare_env(mut self, bare_env: Option<&'a DbElementParamEnv>) -> Self {
        self.bare_env = bare_env;
        self
    }

    fn point(&mut self, number: u32) -> Result<Option<CataloguePoint>, DbElementError> {
        if self.points.is_none() {
            self.points = Some(match soft(catalogue_point_set(self.member))? {
                Some(set) => set.points,
                None => Vec::new(),
            });
        }
        Ok(self
            .points
            .as_ref()
            .and_then(|points| points.iter().find(|point| point.number == number))
            .cloned())
    }

    /// 轴规格 → 构件局部系里的（原点, 单位方向）：具名轴原点在构件原点，`Pn` 过那个 P 点。
    fn axis(
        &mut self,
        primitive: &DbElement,
        name: &str,
    ) -> Result<Option<(DVec3, DVec3)>, DbElementError> {
        let Some(words) = attribute_words(primitive, name)? else {
            return Ok(None);
        };
        if let Some(direction) = axis_spec::direction(&words, self.env) {
            return Ok(DVec3::from_array(direction)
                .try_normalize()
                .map(|direction| (DVec3::ZERO, direction)));
        }
        let encoded = match words[..] {
            [KIND_POINT, number] | [2, KIND_POINT, number] => number,
            _ => return Ok(None),
        };
        let (number, negated) = if encoded >= NEGATED_POINT_BASE {
            (encoded - NEGATED_POINT_BASE, true)
        } else {
            (encoded, false)
        };
        Ok(self.point(number)?.and_then(|point| {
            let direction = if negated { -point.dir } else { point.dir };
            direction
                .try_normalize()
                .map(|direction| (point.pos, direction))
        }))
    }

    fn to_world(&self, local: DVec3) -> DVec3 {
        self.world.transform_point3(local)
    }

    fn to_world_dir(&self, local: DVec3) -> Option<DVec3> {
        self.world.transform_vector3(local).try_normalize()
    }

    /// 一个原语（见模块文档「目录原语」）。几何求不出的只留类型与有效性。
    pub fn primitive(&mut self, primitive: &DbElement) -> Result<SupportPrimitive, DbElementError> {
        let kind = noun_of(primitive)?;
        let (level, obst, tuflag) = validity(primitive)?;
        let mut out = SupportPrimitive {
            kind: kind.clone(),
            level,
            obst,
            tuflag,
            ..SupportPrimitive::default()
        };
        let env = self.env;
        match kind.as_str() {
            "SBOX" => {
                let size = [
                    number(primitive, "PXLE", env)?,
                    number(primitive, "PYLE", env)?,
                    number(primitive, "PZLE", env)?,
                ];
                let centre = DVec3::new(
                    offset(primitive, "PX", env)?,
                    offset(primitive, "PY", env)?,
                    offset(primitive, "PZ", env)?,
                );
                out.center = Some(arr(self.to_world(centre)));
                if let [Some(x), Some(y), Some(z)] = size {
                    out.size = Some([x.abs(), y.abs(), z.abs()]);
                }
            }
            "SCYL" | "SSCY" | "LCYL" => {
                let Some((origin, direction)) = self.axis(primitive, "PAXI")? else {
                    return Ok(out);
                };
                let (centre_distance, height) = if kind == "LCYL" {
                    let (Some(bottom), Some(top)) = (
                        number(primitive, "PBDI", env)?,
                        number(primitive, "PTDI", env)?,
                    ) else {
                        return Ok(out);
                    };
                    ((bottom + top) / 2.0, (top - bottom).abs())
                } else {
                    let Some(height) = number(primitive, "PHEI", env)? else {
                        return Ok(out);
                    };
                    (offset(primitive, "PDIS", env)? + height / 2.0, height.abs())
                };
                out.center = Some(arr(self.to_world(origin + direction * centre_distance)));
                out.axis = self.to_world_dir(direction).map(arr);
                out.height = Some(height);
                out.diameter = number(primitive, "PDIA", env)?.map(f64::abs);
                if let Some(bare_env) = self.bare_env {
                    out.pdia_bare = number(primitive, "PDIA", bare_env)?.map(f64::abs);
                }
            }
            "SEXT" => self.sext(primitive, &mut out)?,
            _ => {}
        }
        Ok(out)
    }

    /// SEXT：`PAAX` / `PBAX` 标架、`(PX, PY, PZ)` 放置、第一个 SLOO 的 SVER 与 PRAD、沿标架 z 拉伸 `|PHEI|`。
    fn sext(
        &mut self,
        primitive: &DbElement,
        out: &mut SupportPrimitive,
    ) -> Result<(), DbElementError> {
        let env = self.env;
        let (Some((origin, x)), Some((_, b))) =
            (self.axis(primitive, "PAAX")?, self.axis(primitive, "PBAX")?)
        else {
            return Ok(());
        };
        let Some(z) = x.cross(b).try_normalize() else {
            return Ok(());
        };
        let y = z.cross(x);
        let frame = DMat3::from_cols(x, y, z);
        let position = DVec3::new(
            offset(primitive, "PX", env)?,
            offset(primitive, "PY", env)?,
            offset(primitive, "PZ", env)?,
        );
        let placed = origin + frame * position;
        let Some(sloo) = primitive
            .members()?
            .find(|member| soft(noun_of(member)).ok().flatten().as_deref() == Some("SLOO"))
        else {
            return Ok(());
        };
        for vertex in sloo.members()? {
            if soft(noun_of(&vertex))?.as_deref() != Some("SVER") {
                continue;
            }
            let local = placed
                + frame
                    * DVec3::new(
                        offset(&vertex, "PX", env)?,
                        offset(&vertex, "PY", env)?,
                        0.0,
                    );
            out.vertices.push(arr(self.to_world(local)));
            out.radii.push(offset(&vertex, "PRAD", env)?);
        }
        if out.radii.iter().all(|radius| *radius == 0.0) {
            out.radii.clear();
        }
        out.axis = self.to_world_dir(z).map(arr);
        out.height = number(primitive, "PHEI", env)?.map(f64::abs);
        Ok(())
    }
}

/// 目录件的几何集：`GMRE`，没有时 `GSTR`（PML `getvalidmems` 的先后）。
pub fn geometry_set(component: &DbElement) -> Result<Option<DbElement>, DbElementError> {
    for attribute in ["GMRE", "GSTR"] {
        if let Some(set) = host::optional_element(component, attribute)? {
            return Ok(Some(set));
        }
    }
    Ok(None)
}

// ---- 型钢 ----------------------------------------------------------------------------------------

/// [`read_section`] 的结果。
#[derive(Debug, Clone, Default)]
pub struct SectionData {
    pub section: SupportSection,
    /// `world_matrix × SectionPlacement::local`：构件的 `pos` / `ori`。
    pub frame: Option<DMat4>,
    /// `|POSE − POSS|`（GENSEC 是直 SPINE 的长度）。
    pub length: Option<f64>,
    /// GMSS 的成员（落在 `POSS` 截面平面上）。
    pub primitives: Vec<SupportPrimitive>,
    /// 型材目录件（SPRF）的 `PARA`。
    pub para: Vec<f64>,
    /// SPRF 往上第一个 CATE 的名字。
    pub cate_name: Option<String>,
}

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

/// 截面坐标 → 截面平面里的最终位置（减 `JUSL` 对齐量，`LMIRR` 时沿 x 镜像），z = 0。
fn place_in_section(profile: &SectionProfile, x: f64, y: f64) -> DVec3 {
    let [jx, jy] = profile.justification;
    let x = x - jx;
    DVec3::new(if profile.mirrored { -x } else { x }, y - jy, 0.0)
}

fn section_failure(
    error: SectionError,
    label: &str,
    what: &str,
    notes: &mut Vec<String>,
) -> Result<(), DbElementError> {
    match error {
        SectionError::Db(error) => Err(error),
        error => {
            notes.push(format!("{label} {what}：{error}"));
            Ok(())
        }
    }
}

/// SCTN / GENSEC 的型钢数据，见模块文档「型钢」。`world` 是 `world_matrix(element)`（链上有库没注册时 `None`）。
pub fn read_section(
    element: &DbElement,
    noun: &str,
    world: Option<DMat4>,
    notes: &mut Vec<String>,
) -> Result<SectionData, DbElementError> {
    let label = format!("{noun} {}", wire_refno(element.refno()));
    let mut data = SectionData::default();
    let catalogue = match section_catalogue(element) {
        Ok(SectionChain::Resolved(catalogue)) => Some(catalogue),
        Ok(SectionChain::NoSpre) => None,
        Ok(SectionChain::Broken { hop, at, why }) => {
            notes.push(format!(
                "{label} 的截面目录链第 {hop} 跳断在 {at}（{why}），没有截面点与目录原语"
            ));
            None
        }
        Err(error) => {
            section_failure(error, &label, "截面目录链读不成", notes)?;
            None
        }
    };
    let profile = match &catalogue {
        Some(catalogue) => match evaluate_section_profile(element, catalogue) {
            Ok(profile) => profile,
            Err(error) => {
                section_failure(error, &label, "截面求不出", notes)?;
                bare_profile()
            }
        },
        None => bare_profile(),
    };
    if let Some(catalogue) = &catalogue {
        data.para = host::optional_doubles(&catalogue.sprf, "PARA")?.unwrap_or_default();
        data.cate_name = host::ancestor_name(&catalogue.sprf, "CATE")?;
    }

    let placement = if noun == "GENSEC" {
        gensec_ends(element, &mut data.section)?;
        match spine_path(element) {
            Ok(Some(path)) => match path.as_straight() {
                Some((start, end)) => {
                    let bang = host::optional_double(element, "BANG")?.unwrap_or(0.0);
                    match placement_between(
                        &element.refno().to_string(),
                        noun,
                        start,
                        end,
                        bang,
                        &profile,
                    ) {
                        Ok(placement) => Some(placement),
                        Err(error) => {
                            section_failure(error, &label, "SPINE 放不成截面标架", notes)?;
                            None
                        }
                    }
                }
                None => {
                    notes.push(format!(
                        "{label} 的 SPINE 不是单一直段（{} 段），没有截面标架与截面点",
                        path.segments.len()
                    ));
                    None
                }
            },
            Ok(None) => {
                notes.push(format!("{label} 没有 SPINE，没有截面标架与截面点"));
                None
            }
            Err(error) => {
                section_failure(error, &label, "SPINE 读不成", notes)?;
                None
            }
        }
    } else {
        data.section.posstart = host::world_point(element, "POSS")?
            .map(arr)
            .unwrap_or_default();
        data.section.posend = host::world_point(element, "POSE")?
            .map(arr)
            .unwrap_or_default();
        data.section.drnstart = host::world_direction(element, "DRNS")?.map(arr);
        data.section.drnend = host::world_direction(element, "DRNE")?.map(arr);
        match section_placement(element, &profile) {
            Ok(placement) => Some(placement),
            Err(error) => {
                section_failure(error, &label, "放不成截面标架", notes)?;
                None
            }
        }
    };
    let Some(SectionPlacement { local, length, .. }) = placement else {
        return Ok(data);
    };
    data.length = Some(tidy(length));
    let Some(world) = world else {
        notes.push(format!("{label} 的属主链折不进世界系，没有截面标架"));
        return Ok(data);
    };
    let frame = world * local;
    data.frame = Some(frame);
    if let Some(catalogue) = &catalogue {
        section_catalogue_data(
            element, catalogue, &profile, frame, length, &mut data, &label, notes,
        )?;
    }
    Ok(data)
}

/// GENSEC 的两端（首末 POINSP 的世界位置）与首个 SPINE 的 `DRNS` / `DRNE`（`supportSctn.pmlobj` L105-111）。
fn gensec_ends(element: &DbElement, section: &mut SupportSection) -> Result<(), DbElementError> {
    let mut poinsps = Vec::new();
    collect_descendants(element, "POINSP", &mut poinsps, 0)?;
    if let Some(first) = poinsps.first() {
        section.posstart = host::world_origin(first)?.map(arr).unwrap_or_default();
    }
    if let Some(last) = poinsps.last() {
        section.posend = host::world_origin(last)?.map(arr).unwrap_or_default();
    }
    if let Some(spine) = element.members_of_type("SPINE")?.into_iter().next() {
        section.drnstart = host::world_direction(&spine, "DRNS")?.map(arr);
        section.drnend = host::world_direction(&spine, "DRNE")?.map(arr);
    }
    Ok(())
}

/// 深度优先、文件原序收某类后代（PML `coll all X for $!el`）。
pub fn collect_descendants(
    element: &DbElement,
    noun: &str,
    out: &mut Vec<DbElement>,
    depth: usize,
) -> Result<(), DbElementError> {
    if depth > 64 {
        return Ok(());
    }
    for member in element.members()? {
        if soft(noun_of(&member))?.as_deref() == Some(noun) {
            out.push(member.clone());
        }
        collect_descendants(&member, noun, out, depth + 1)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn section_catalogue_data(
    element: &DbElement,
    catalogue: &SectionCatalogue,
    profile: &SectionProfile,
    frame: DMat4,
    length: f64,
    data: &mut SectionData,
    label: &str,
    notes: &mut Vec<String>,
) -> Result<(), DbElementError> {
    let env = DbElementParamEnv::new(Some(element.clone()), Some(catalogue.sprf.clone()))
        .without_insulation();
    let place = |x: f64, y: f64| frame.transform_point3(place_in_section(profile, x, y));
    let extrusion = frame.transform_vector3(DVec3::Z).try_normalize();
    for member in catalogue.gmss.members()? {
        let kind = noun_of(&member)?;
        let (level, obst, tuflag) = validity(&member)?;
        let mut primitive = SupportPrimitive {
            kind: kind.clone(),
            level,
            obst,
            tuflag,
            ..SupportPrimitive::default()
        };
        let mut ring: Vec<DVec3> = Vec::new();
        match kind.as_str() {
            "SPRO" => {
                for spve in member.members()? {
                    if soft(noun_of(&spve))?.as_deref() != Some("SPVE") {
                        continue;
                    }
                    let point = place(offset(&spve, "PX", &env)?, offset(&spve, "PY", &env)?);
                    ring.push(point);
                    primitive.vertices.push(arr(point));
                    primitive.radii.push(offset(&spve, "PRAD", &env)?);
                }
                if primitive.radii.iter().all(|radius| *radius == 0.0) {
                    primitive.radii.clear();
                }
            }
            "SREC" => {
                let (cx, cy) = (offset(&member, "PX", &env)?, offset(&member, "PY", &env)?);
                if let (Some(xlen), Some(ylen)) = (
                    number(&member, "PXLE", &env)?,
                    number(&member, "PYLE", &env)?,
                ) {
                    let (hx, hy) = (xlen / 2.0, ylen / 2.0);
                    // `getSctnPoints` L118-121 的角序：(+x, +y)、(−x, +y)、(−x, −y)、(+x, −y)。
                    ring = [(hx, hy), (-hx, hy), (-hx, -hy), (hx, -hy)]
                        .into_iter()
                        .map(|(dx, dy)| place(cx + dx, cy + dy))
                        .collect();
                    primitive.vertices = ring.iter().copied().map(arr).collect();
                }
            }
            "SANN" => {
                if let Some(radius) = number(&member, "PRAD", &env)? {
                    let centre = place(offset(&member, "PX", &env)?, offset(&member, "PY", &env)?);
                    primitive.center = Some(arr(centre));
                    primitive.diameter = Some(radius.abs() * 2.0);
                    // `getSctnPoints` L126-135：截面原点上的 2·PRAD 方框。
                    let r = radius.abs();
                    ring = [(r, r), (-r, r), (-r, -r), (r, -r)]
                        .into_iter()
                        .map(|(dx, dy)| place(dx, dy))
                        .collect();
                }
            }
            _ => {}
        }
        if matches!(kind.as_str(), "SPRO" | "SREC" | "SANN") {
            primitive.axis = extrusion.map(arr);
            primitive.height = Some(tidy(length));
        }
        let first_valid = data.section.section_points.is_empty() && level_obst_ok(level, obst);
        if first_valid {
            let distinct = distinct_points(&ring);
            let take = match kind.as_str() {
                "SPRO" => distinct.len() >= 3,
                "SREC" | "SANN" => ring.len() == 4,
                _ => false,
            };
            if take {
                data.section.section_points = if kind == "SPRO" {
                    distinct
                } else {
                    ring.clone()
                }
                .into_iter()
                .map(arr)
                .collect();
            }
        }
        data.primitives.push(primitive);
    }
    if data.section.section_points.is_empty() {
        notes.push(format!(
            "{label} 的截面里没有有效（LEVEL 含 6、OBST 0 / 2）的 SPRO / SREC / SANN，没有截面点"
        ));
    }
    let Some(ptss) = host::optional_element(&catalogue.sprf, "PSTR")? else {
        return Ok(());
    };
    for pline in ptss.members()? {
        if soft(noun_of(&pline))?.as_deref() != Some("PLIN") {
            continue;
        }
        let Some(key) = host::optional_text(&pline, "PKEY")?.map(|key| key.to_ascii_uppercase())
        else {
            continue;
        };
        let offset_2d = place_in_section(
            profile,
            offset(&pline, "PX", &env)?,
            offset(&pline, "PY", &env)?,
        );
        let start = frame.transform_point3(offset_2d);
        let end = frame.transform_point3(offset_2d + DVec3::Z * length);
        if key == NEUTRAL_AXIS_KEY && data.section.na_start.is_none() {
            data.section.na_start = Some(arr(start));
            data.section.na_end = Some(arr(end));
        }
        if FLANGE_LINE_KEYS.contains(&key.as_str())
            && !data.section.flange_lines.iter().any(|line| line.key == key)
        {
            data.section.flange_lines.push(SupportFlangeLine {
                key,
                start: arr(start),
                end: arr(end),
            });
        }
    }
    Ok(())
}

/// 去掉重复点（保留先到的，PML `unique()`）。
fn distinct_points(points: &[DVec3]) -> Vec<DVec3> {
    let mut out: Vec<DVec3> = Vec::with_capacity(points.len());
    for point in points {
        if !out
            .iter()
            .any(|seen| seen.distance(*point) <= SAME_POINT_MM)
        {
            out.push(*point);
        }
    }
    out
}

// ---- 板 ------------------------------------------------------------------------------------------

/// [`read_plate`] 的结果。
#[derive(Debug, Clone, Default)]
pub struct PlateData {
    pub plate: Option<SupportPlate>,
    pub pane_pad: Option<SupportPanePad>,
}

/// 一个环上的顶点：局部位置（PANE 系）与 FRAD。
struct LoopVertex {
    local: DVec3,
    frad: f64,
}

fn loop_vertices(lp: &DbElement) -> Result<Vec<LoopVertex>, DbElementError> {
    let mut out = Vec::new();
    for vertex in lp.members()? {
        if !matches!(soft(noun_of(&vertex))?.as_deref(), Some("PAVE" | "VERT")) {
            continue;
        }
        let pos = match vertex.get_position("POS") {
            Ok(pos) => pos.unwrap_or([0.0; 3]),
            Err(DbElementError::UnknownAttribute { .. }) => [0.0; 3],
            Err(error) => return Err(error),
        };
        out.push(LoopVertex {
            local: DVec3::from_array(pos),
            frad: host::optional_double(&vertex, "FRAD")?.unwrap_or(0.0),
        });
    }
    Ok(out)
}

/// PANE 的基面与厚度方向（PML `vertPoints` L52-72）。
struct PlateFrame {
    world: DMat4,
    /// `ORI z`（世界）。
    ori_z: DVec3,
    /// 基面上的一点（`POS`，`SJUS` 含 CEN 时下移半厚）。
    base: DVec3,
    /// 基面指向另一面。
    zdir: DVec3,
    height: f64,
    /// 基面在 PANE 局部系里的 z。
    base_z: f64,
}

impl PlateFrame {
    fn project(&self, point: DVec3) -> DVec3 {
        point - self.ori_z * (point - self.base).dot(self.ori_z)
    }
}

fn plate_frame(
    pane: &DbElement,
    lp: &DbElement,
    is_ploo: bool,
    world: DMat4,
) -> Result<Option<PlateFrame>, DbElementError> {
    let height = if is_ploo {
        match host::optional_double(lp, "HEIG")? {
            Some(height) => Some(height),
            None => host::optional_double(pane, "HEIG")?,
        }
    } else {
        host::optional_double(pane, "HEIG")?
    };
    let Some(height) = height.filter(|height| *height > LENGTH_EPS) else {
        return Ok(None);
    };
    let Some(ori_z) = world.transform_vector3(DVec3::Z).try_normalize() else {
        return Ok(None);
    };
    let origin = world.w_axis.truncate();
    let sjus = if is_ploo {
        host::optional_text(lp, "SJUS")?
            .unwrap_or_default()
            .to_ascii_uppercase()
    } else {
        String::new()
    };
    let (zdir, base, base_z) = if sjus.contains("TO") {
        (-ori_z, origin, 0.0)
    } else if sjus.contains("CEN") {
        (ori_z, origin - ori_z * (height / 2.0), -height / 2.0)
    } else {
        (ori_z, origin, 0.0)
    };
    Ok(Some(PlateFrame {
        world,
        ori_z,
        base,
        zdir,
        height,
        base_z,
    }))
}

/// PANE 的板数据与（该算的）护板，见模块文档「板」。`world` 是 `world_matrix(pane)`。
pub fn read_plate(
    pane: &DbElement,
    world: DMat4,
    notes: &mut Vec<String>,
) -> Result<PlateData, DbElementError> {
    let label = format!("PANE {}", wire_refno(pane.refno()));
    let members: Vec<DbElement> = pane.members()?.collect();
    let mut nouns = Vec::with_capacity(members.len());
    for member in &members {
        nouns.push(soft(noun_of(member))?.unwrap_or_default());
    }
    let first = |noun: &str| {
        nouns
            .iter()
            .position(|candidate| candidate == noun)
            .map(|index| &members[index])
    };
    let count = |noun: &str| nouns.iter().filter(|candidate| *candidate == noun).count();
    let ploo = first("PLOO");
    let (lp, is_ploo) = match ploo {
        Some(ploo) => (Some(ploo), true),
        None => (first("LOOP"), false),
    };
    let mut data = PlateData::default();
    let Some(lp) = lp else {
        notes.push(format!("{label} 下没有 PLOO / LOOP，没有板数据"));
        return Ok(data);
    };
    let Some(frame) = plate_frame(pane, lp, is_ploo, world)? else {
        notes.push(format!(
            "{label} 的环高缺或为 0（E3D 同样不出几何），没有板数据"
        ));
        return Ok(data);
    };
    let vertices = loop_vertices(lp)?;
    if vertices.len() < 3 {
        notes.push(format!(
            "{label} 的第一个环只有 {} 个顶点，没有板数据",
            vertices.len()
        ));
        return Ok(data);
    }
    let world_points: Vec<DVec3> = vertices
        .iter()
        .map(|vertex| frame.world.transform_point3(vertex.local))
        .collect();
    let mut plate = SupportPlate {
        vertices: world_points
            .iter()
            .map(|point| arr(frame.project(*point)))
            .collect(),
        normal: arr(frame.zdir),
        thickness: frame.height,
        ori_z: Some(arr(frame.ori_z)),
        pfit_count: Some(count("PFIT") as u32),
        paves: if is_ploo {
            world_points.iter().copied().map(arr).collect()
        } else {
            Vec::new()
        },
        ..SupportPlate::default()
    };
    for (member, noun) in members.iter().zip(&nouns) {
        if noun != "NCYL" {
            continue;
        }
        let (Some(pos), Some(diameter)) = (
            host::world_origin(member)?,
            host::optional_double(member, "DIAM")?,
        ) else {
            notes.push(format!(
                "{label} 下的 NCYL {} 缺位置或 DIAM，不进孔表",
                wire_refno(member.refno())
            ));
            continue;
        };
        plate.ncyls.push(SupportHole {
            refno: wire_refno(member.refno()),
            pos: arr(pos),
            diameter,
        });
    }
    let nxtrs: Vec<&DbElement> = members
        .iter()
        .zip(&nouns)
        .filter(|(_, noun)| *noun == "NXTR")
        .map(|(member, _)| member)
        .collect();
    if !nxtrs.is_empty() {
        match cut_outline(&frame, &vertices, &nxtrs)? {
            Ok(cut) => plate.cut_vertices = cut.into_iter().map(arr).collect(),
            Err(why) => notes.push(format!(
                "{label} 的轮廓减 NXTR 求不出（{why}），没有 cut_vertices"
            )),
        }
    }
    let is_pad = ploo.is_none() || (count("NBOX") > 0 && count("NCYL") > 0);
    if is_pad {
        match pane_pad(&frame, &vertices, &members, &nouns)? {
            Ok(pad) => data.pane_pad = Some(pad),
            Err(why) => notes.push(format!("{label} 按弧形护板求不出（{why}），按普通板给")),
        }
    }
    data.plate = Some(plate);
    Ok(data)
}

/// 局部平面坐标（PANE 系的 x / y）。
fn planar(point: DVec3) -> DVec2 {
    DVec2::new(point.x, point.y)
}

/// 并点并圆角（`mergePointsRadiuss`）后建线 / 弧（`getLinesAndArcsfortwoarrays`），转成闭合多段线（逆时针）。
fn ring_polyline(points: &[DVec2], radii: &[f64], merge: bool) -> Option<Polyline<f64>> {
    let (points, radii) = if merge {
        regularize_vertices(points, radii)?
    } else {
        (points.to_vec(), radii.to_vec())
    };
    let segments = profile_from_vertices(&points, &radii);
    segments_polyline(&segments)
}

fn segments_polyline(segments: &[ProfileSegment]) -> Option<Polyline<f64>> {
    if segments.len() < 2 {
        return None;
    }
    let mut pline = Polyline::new_closed();
    for segment in segments {
        let start = segment.start();
        let bulge = match segment {
            ProfileSegment::Line { .. } => 0.0,
            ProfileSegment::Arc { sweep_rad, .. } => (sweep_rad / 4.0).tan(),
        };
        pline.add(start.x, start.y, bulge);
    }
    if pline.area() < 0.0 {
        pline = reversed(&pline);
    }
    Some(pline)
}

/// 方向反过来：顶点倒序，每段的 bulge 跟着它的起点挪、取反。
fn reversed(pline: &Polyline<f64>) -> Polyline<f64> {
    let count = pline.vertex_count();
    let mut out = Polyline::new_closed();
    for k in 0..count {
        let index = count - 1 - k;
        let vertex = pline.at(index);
        // 反向后从 index 出发的那段，原来是从 index − 1 到 index 的段。
        let previous = pline.at((index + count - 1) % count);
        out.add(vertex.x, vertex.y, -previous.bulge);
    }
    out
}

fn difference(base: &Polyline<f64>, cutter: &Polyline<f64>) -> Option<Polyline<f64>> {
    let result = base.boolean(cutter, BooleanOp::Not);
    result
        .pos_plines
        .into_iter()
        .next()
        .map(|first| first.pline)
}

/// 一个环：平面顶点与逐点圆角半径（FRAD / PRAD）。
pub type Ring = (Vec<DVec2>, Vec<f64>);

/// 各 NXTR 第一个 LOOP 在 PANE 局部平面里的环（NXTR 自己的世界矩阵 → PANE 局部系）。
fn nxtr_ring(frame: &PlateFrame, nxtr: &DbElement) -> Result<Result<Ring, String>, DbElementError> {
    let Some(lp) = nxtr.members_of_type("LOOP")?.into_iter().next() else {
        return Ok(Err(format!(
            "NXTR {} 下没有 LOOP",
            wire_refno(nxtr.refno())
        )));
    };
    let Some(nxtr_world) = host::world_matrix_soft(nxtr)? else {
        return Ok(Err(format!(
            "NXTR {} 的属主链折不进世界系",
            wire_refno(nxtr.refno())
        )));
    };
    let to_pane = frame.world.inverse() * nxtr_world;
    let vertices = loop_vertices(&lp)?;
    Ok(Ok((
        vertices
            .iter()
            .map(|vertex| planar(to_pane.transform_point3(vertex.local)))
            .collect(),
        vertices.iter().map(|vertex| vertex.frad).collect(),
    )))
}

/// 板轮廓减去各 NXTR（`supportpane.pmlobj` L84-104），各边起点落回基面。
fn cut_outline(
    frame: &PlateFrame,
    vertices: &[LoopVertex],
    nxtrs: &[&DbElement],
) -> Result<Result<Vec<DVec3>, String>, DbElementError> {
    let outline: Ring = (
        vertices.iter().map(|vertex| planar(vertex.local)).collect(),
        vertices.iter().map(|vertex| vertex.frad).collect(),
    );
    let mut cutters = Vec::with_capacity(nxtrs.len());
    for nxtr in nxtrs {
        match nxtr_ring(frame, nxtr)? {
            Ok(ring) => cutters.push(ring),
            Err(why) => return Ok(Err(why)),
        }
    }
    Ok(cut_ring(&outline, &cutters).map(|points| {
        points
            .into_iter()
            .map(|point| {
                frame
                    .world
                    .transform_point3(DVec3::new(point.x, point.y, frame.base_z))
            })
            .collect()
    }))
}

/// PML `profile.difference(…)[1]` 的二维版：轮廓（并点并圆角后建线 / 弧）逐个减去 `cutters`，取剩下的第一块，
/// 返回它各边的起点（`edges()` 的 `startposition`，环序原样）。
pub fn cut_ring(outline: &Ring, cutters: &[Ring]) -> Result<Vec<DVec2>, String> {
    let Some(mut rest) = ring_polyline(&outline.0, &outline.1, true) else {
        return Err("板轮廓建不成".into());
    };
    for (index, cutter) in cutters.iter().enumerate() {
        let Some(cutter) = ring_polyline(&cutter.0, &cutter.1, true) else {
            return Err(format!("第 {} 个孔的环建不成轮廓", index + 1));
        };
        rest = difference(&rest, &cutter)
            .ok_or_else(|| format!("减去第 {} 个孔后什么都不剩", index + 1))?;
    }
    Ok(rest
        .iter_vertexes()
        .map(|vertex| DVec2::new(vertex.x, vertex.y))
        .collect())
}

/// 弧形护板（`supppanepad.pmlobj` L27-110），见模块文档「板」。在 PANE 局部系里算，结果落回世界。
fn pane_pad(
    frame: &PlateFrame,
    vertices: &[LoopVertex],
    members: &[DbElement],
    nouns: &[String],
) -> Result<Result<SupportPanePad, String>, DbElementError> {
    let to_local = frame.world.inverse();
    // 两面中心的连线：基面沿厚度方向挪 HEIG，`dir` 就是局部的厚度方向，`len` 就是 HEIG（L34-35）。
    let dir = to_local.transform_vector3(frame.zdir).normalize_or_zero();
    let len = frame.height;
    let parallel = |axis: DVec3| {
        let angle = axis
            .normalize_or_zero()
            .dot(dir)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        angle < PAD_AXIS_DEG || angle > 180.0 - PAD_AXIS_DEG
    };
    let mut boxes = Vec::new();
    let mut holes = Vec::new();
    for (member, noun) in members.iter().zip(nouns) {
        if noun != "NBOX" && noun != "NCYL" {
            continue;
        }
        let Some(world) = host::world_matrix_soft(member)? else {
            continue;
        };
        let local = to_local * world;
        let rotation = DMat3::from_mat4(local);
        if noun == "NCYL" {
            if let Some(diameter) = host::optional_double(member, "DIAM")?
                && parallel(rotation.z_axis)
                && diameter > LENGTH_EPS
            {
                holes.push(diameter / 2.0);
            }
            continue;
        }
        let lengths = [
            host::optional_double(member, "XLEN")?,
            host::optional_double(member, "YLEN")?,
            host::optional_double(member, "ZLEN")?,
        ];
        let [Some(xlen), Some(ylen), Some(zlen)] = lengths else {
            continue;
        };
        // `getBoxprofile` L117-140：哪根轴与管长方向平行，就在另两根张成的平面里取矩形。
        let (x, y, xl, yl) = if parallel(rotation.z_axis) {
            (rotation.x_axis, rotation.y_axis, xlen, ylen)
        } else if parallel(rotation.x_axis) {
            (rotation.y_axis, rotation.z_axis, ylen, zlen)
        } else if parallel(rotation.y_axis) {
            (rotation.z_axis, rotation.x_axis, zlen, xlen)
        } else {
            continue;
        };
        let centre = local.w_axis.truncate();
        boxes.push(
            [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
                .map(|(sx, sy)| planar(centre + x * (sx * xl / 2.0) + y * (sy * yl / 2.0))),
        );
    }
    let outline: Ring = (
        vertices.iter().map(|vertex| planar(vertex.local)).collect(),
        vertices.iter().map(|vertex| vertex.frad).collect(),
    );
    let section = match pad_section(&outline, &boxes, &holes) {
        Ok(section) => section,
        Err(why) => return Ok(Err(why)),
    };
    let angle_deg = section.sweep.abs().to_degrees();
    let anchor = DVec3::new(section.mid.x, section.mid.y, frame.base_z) + dir * (len / 2.0);
    Ok(Ok(SupportPanePad {
        len,
        arc_len: section.big_radius * angle_deg.to_radians(),
        thick: ((section.big_radius - section.small_radius) * 10.0).round() / 10.0,
        angle_deg,
        pos: arr(frame.world.transform_point3(anchor)),
    }))
}

/// 弧形护板的截面（PANE 局部平面里）：轮廓减去 NBOX 矩形与 NCYL 圆之后的大弧与另一条弧。
#[derive(Debug, Clone, PartialEq)]
pub struct PadSection {
    pub big_radius: f64,
    pub small_radius: f64,
    /// 大弧的有向扫角（弧度，逆时针为正）。
    pub sweep: f64,
    /// 大弧中点（PML `angleposition((startangle + endangle) / 2)`）。
    pub mid: DVec2,
}

/// `supppanepad` 的截面那一半（L30-90）：轮廓并点并圆角（`vertPoints(item, true)`），逐个减去 `boxes`（已投到截面
/// 平面的四角）与以轮廓包围盒中心为圆心、半径 `hole_radii` 的圆（L52：PML 认定护板的孔就是管），并同心同径的相邻弧
/// （`mergearcs`），再按边序取第一条弧为大弧、遇到更大的就换（L63-81）。
pub fn pad_section(
    outline: &Ring,
    boxes: &[[DVec2; 4]],
    hole_radii: &[f64],
) -> Result<PadSection, String> {
    let Some((merged, merged_radii)) = regularize_vertices(&outline.0, &outline.1) else {
        return Err("轮廓并点后不成环".into());
    };
    let Some(mut profile) = ring_polyline(&merged, &merged_radii, false) else {
        return Err("轮廓建不成".into());
    };
    for (index, corners) in boxes.iter().enumerate() {
        let Some(cutter) = ring_polyline(corners, &[0.0; 4], false) else {
            continue;
        };
        profile = difference(&profile, &cutter)
            .ok_or_else(|| format!("减去第 {} 个 NBOX 后什么都不剩", index + 1))?;
    }
    let centre = bbox_center_2d(&merged);
    for (index, radius) in hole_radii.iter().enumerate() {
        let mut circle = Polyline::new_closed();
        circle.add(centre.x - radius, centre.y, 1.0);
        circle.add(centre.x + radius, centre.y, 1.0);
        profile = difference(&profile, &circle)
            .ok_or_else(|| format!("减去第 {} 个 NCYL 后什么都不剩", index + 1))?;
    }
    let arcs = merged_arcs(&profile);
    let mut big: Option<&PadArc> = None;
    let mut small: Option<&PadArc> = None;
    for arc in &arcs {
        match big {
            Some(max) if arc.radius <= max.radius => {
                small = Some(arc);
                break;
            }
            Some(previous) => {
                small = Some(previous);
                big = Some(arc);
                break;
            }
            None => big = Some(arc),
        }
    }
    let (Some(big), Some(small)) = (big, small) else {
        return Err("减完之后的轮廓里没有两条弧".into());
    };
    Ok(PadSection {
        big_radius: big.radius,
        small_radius: small.radius,
        sweep: big.sweep,
        mid: big.centre + DVec2::from_angle(big.sweep / 2.0).rotate(big.start - big.centre),
    })
}

/// 轮廓上的一条弧：圆心、半径、起点、有向扫角（弧度，逆时针为正）。
struct PadArc {
    centre: DVec2,
    radius: f64,
    start: DVec2,
    sweep: f64,
}

/// 多段线的弧段，首尾相接、同心同径、同向的并成一条（PML `profile.mergearcs()`）。
fn merged_arcs(pline: &Polyline<f64>) -> Vec<PadArc> {
    let same = |a: &PadArc, b: &PadArc| {
        a.centre.distance(b.centre) < 1e-6
            && (a.radius - b.radius).abs() < 1e-6
            && a.sweep.signum() == b.sweep.signum()
    };
    let mut arcs: Vec<PadArc> = Vec::new();
    let mut previous_was_arc = false;
    for (v1, v2) in pline.iter_segments() {
        if v1.bulge.abs() < 1e-12 {
            previous_was_arc = false;
            continue;
        }
        let (radius, centre) = seg_arc_radius_and_center(v1, v2);
        let arc = PadArc {
            centre: DVec2::new(centre.x, centre.y),
            radius,
            start: DVec2::new(v1.x, v1.y),
            sweep: 4.0 * v1.bulge.atan(),
        };
        match arcs.last_mut() {
            Some(last) if previous_was_arc && same(last, &arc) => last.sweep += arc.sweep,
            _ => arcs.push(arc),
        }
        previous_was_arc = true;
    }
    // 首尾相接的两条（环从一条弧的中间起头）也并。
    if arcs.len() > 1 && previous_was_arc && pline.at(0).bulge.abs() >= 1e-12 {
        let last = arcs.pop().expect("at least two arcs");
        if same(&last, &arcs[0]) {
            arcs[0].start = last.start;
            arcs[0].sweep += last.sweep;
        } else {
            arcs.push(last);
        }
    }
    arcs
}

fn bbox_center_2d(points: &[DVec2]) -> DVec2 {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for point in points {
        lo = lo.min(*point);
        hi = hi.max(*point);
    }
    (lo + hi) / 2.0
}
