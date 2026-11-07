//! plant-mbd M2a / M2b 要的宿主字段（plant3d-web#103）：成员的 `para` / `desp`、`spref_name` / `cate_name` /
//! `gmref_name`、`aconn` / `lconn` / `p3_conn`、ATTA 的 `prop_bang` / `rpro_dod` / `rpro_dthi` / `atta_adlongth`、
//! `dmf_targets`、`catalogue_primitives`（含 `pdia_bare`）、`vol`、`ipara`，成员之间的 `navigation`，BRAN 的
//! `flow_direction` / `heat_tracing` / `dmf_walls`。字段语义与 PML 出处见 plant-mbd `docs/field-dictionary.md`「M2b 宿主
//! 字段」「M2a 极坐标摆位的宿主字段」，这里只讲从 E3D 库怎么读。
//!
//! - **目录件**：`catalogue_point_set(member).component`，即 PML 的 `catref of`（`SPRE → [SPCO → CATR]* → SCOM`）。
//!   `para` 先读构件自己的 `PARA`、没有再读目录件的（Core `DESPRE` 同序）；`cate_name` 是目录件属主链上的 CATE，
//!   `gmref_name` 是目录件的 `GMRE`；名字一律按 PML `name of`（没有存名给 `=w0/w1`）。
//! - **连接类型**：P 点（目录件 `PTRE` 下 `NUMB = n` 的 PTAX / PTCA）的 `PCON`。它存成目录表达式：拼出来的词（`BWD`）
//!   直接取；`PARAM n` 一类求值后是存成实数的词哈希（AMS 的 `/HN/…` 目录把 `SWF` 放在 `PARA` 里），按系统名表、再按
//!   `db1_dehash` 还原；`[0, 1, 0, 0]` 求值为 0，按没有。到达 / 离开口的编号取 `walk_route` 的槽位端，穿过件读自己的
//!   `ARRI` / `LEAV`。
//! - **目录原语**：几何集（ATTA 先 `GSTR` 再 `GMRE`，PML `getattageo` L741-748；其余先 `GMRE` 再 `GSTR`，
//!   `getPipeMaxdia` L19-25）下的 `LSNO / SRTO / LCYL / SCYL / SSCY / SCTO`，连同各自名下的负体
//!   `NLCY / NSCY / NSSL / NSSC`。尺寸与轴在「设计构件 + 目录件」的环境里求值，**带保温**：PML 的 `!!evalCata` 走
//!   `dbref.evaluate`，`IPARAM` 答实例的保温参数。具名轴按构件的世界旋转转到世界系；`Pn` 轴取那个 P 点的世界方向与
//!   位置（`evalCata` 回 `[dir, pos]`）。
//! - **`pdia_bare`**：构件带保温（`ipara` 非空）时，`LCYL / SCYL / SSCY` 的 `PDIA` 另在不带保温的环境里求一次
//!   （`DbElementParamEnv::without_insulation`，`IPARAM` 一律答 0）。`getPipeMaxdia` 认定这几种圆柱的 `PDIA` 含保温、
//!   一律扣 `ipara[1]`，AMS 的套接 OLET 却写成不含（24381/145487：`PDIA` = `PARAM 10` = 33，`IPARAM 1` = 340），扣成
//!   负数就退到直段外径；裸径对两种写法都对（含 `IPARAM` 时去掉保温，不含时原值不变），求解器有它就不扣（plant-mbd
//!   `D-PIPE-021`）。不带保温时不给，求解器照 PML 公式（`ipara` 为空即不扣）。
//! - **`ipara`**：保温选件（`ISPE → SELE … → SPCO → CATR`）的 `PARA`，经 `DbElementParamEnv::iparam` 逐格取。公开接口
//!   越界答 0、分不出「没有这一格」，所以取前 [`IPARA_SLOTS`] 格再截掉末尾的 0；没有保温就是空表（求解器按不扣）。
//! - **`vol`**：E3D 自己的 SPAMAP（`WITHIN VOLUME` 用的那张表，0.1 mm，存盘快照）里成员的世界包围盒
//!   （[`host::spamap_index`]）。AMS 三座设计库的管道成员每一类都有条目。管件的盒子连着它离开侧的隐式管身（E3D 把
//!   隐式管身记在起头的构件名下，24381/145021 的弯头盒子一直伸到下一只弯头），ATTA 是 P3 一带的小盒——照 E3D 原样给，
//!   不拿目录几何另算。
//! - **墙体定位**：PML 的 `dmfarr` 在模板里叫 `DMFA`（引用数组）；目标经找墙候选的同一套组装
//!   （[`civil_candidate_of`]）进 BRAN 级 `dmf_walls`。
//!
//! 标签、安装角度与调整焊要的事实（plant-mbd 字段字典「管道标签 / 安装角度 / 调整焊的宿主字段」）：
//!
//! - **`ori`**：成员世界矩阵的三根轴（PML `ori of $!mem wrt /*`），归一。
//! - **TEE / OLET / VALV**：UDA `:YKSX` 原文（VALV 的由求解器接在 Tail 连接标签末行，PML `isobran.gettags`
//!   L2207-2217）。TEE / OLET 另给 `CREF` 的目标（refno、类型、目标自己所在 ZONE 的 ITUB / INST 标志，标志与
//!   `connection` 的过滤同一口径）。`connection` 的过滤本身不动。
//! - **PCOM**：`CRFA`（元素引用数组）逐项的存名与 `HPOS`（世界系），跳过 Nulref 与不在所钉会话的目标。
//! - **SITE 名含 INST 的 ATTA**：UDA `:NPHSASR` 第一项（AMS 字典里它是单个 REF，PML 按数组取 `[1]`，两样都收）的
//!   属主就是支架，给它的 refno、存名与 `:ZD_BZBM`。
//! - **BRAN**：SITE 的 NAME（PML `name of`，没有存名给 `=w0/w1`）、自己的 `:ZD_NBBM` / `:ZD_WJBM`（PML `setattvalue`
//!   只读 BRAN 本身）、`HREF` / `TREF` 的目标，
//!   以及目标是 BRAN 时它的末 / 首根隐式管身（[`super::adjacent_tube`]）。
//!
//! 读不出就留空：UDA 没定义（AMS 没有 `:ZD_GDBR` / `:ATTA-ADLONGTH` / `:YKSX` / `:ZD_*`）、目录链断、表达式求不出都算
//! 没有。读库出错也不让整条 BRAN 失败：一个成员（或 BRAN 自己）读坏的字段汇成一条 Info `adapter.member_field_unreadable`；
//! 墙板转不成候选是 `adapter.dmf_wall_unresolved`，转成了但有说明（截面退回 DESP 之类）是 `adapter.dmf_wall_note`。

use std::collections::HashMap;

use e3d_attlib::{db1_dehash, lookup_system_name};
use e3d_io::db_element::{DbElement, DbElementError};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::record::axis_spec;
use e3d_io::record::catalogue_eval::ParamEnv;
use e3d_io::record::catalogue_expr::{self, CatalogueExpression};
use e3d_io::record::descriptor::DescriptorValue;
use e3d_io::refno::RefNo;
use e3d_model::route::PPointResolver;
use e3d_model::{CataloguePoints, catalogue_point_set};
use glam::{DMat3, DMat4, DVec3};
use plant_mbd::{
    AdjacentTube, AttaSupport, BranchMember, CatalogueAxis, CataloguePrimitive, CivilCandidate,
    ConnectionTarget, CrfaTarget, IssueCategory, IssueSeverity, MbdV2Issue, MemberNavigation,
    SourceBox,
};

use super::{AdjacentEnd, BranchQueryError, adjacent_tube, wire_refno, zone_flags};
use crate::mbd::civil_faces::civil_candidate_of;
use crate::mbd::host::{self, SpamapIndex, soft};

/// 保温参数取前几格（见模块文档）。
const IPARA_SLOTS: i32 = 4;
/// 求解器认的目录原语（字段字典 `CataloguePrimitive.kind`）。
const PRIMITIVE_KINDS: [&str; 6] = ["LSNO", "SRTO", "LCYL", "SCYL", "SSCY", "SCTO"];
/// 正体名下收的负体：PML `coll all (nlcy nscy nssl) for $!prim`，字段字典写作 `NSSC` 的那一种也收。
const NEGATIVE_KINDS: [&str; 4] = ["NLCY", "NSCY", "NSSL", "NSSC"];
/// `-Pn` 轴的编码基数（e3d-model `catalogue_point::NEGATED_POINT_BASE`，crate 私有）。
const NEGATED_POINT_BASE: u32 = 1000;
/// 正体名下找负体的深度上限（目录树实测一两层）。
const MAX_NEGATIVE_DEPTH: usize = 8;

/// 一个 E3D 成员的 M2a / M2b 字段（隐式管身不是 E3D 成员，没有这些）。
#[derive(Debug, Default)]
pub(super) struct MemberFields {
    para: Vec<f64>,
    desp: Vec<f64>,
    spref_name: Option<String>,
    cate_name: Option<String>,
    gmref_name: Option<String>,
    aconn: Option<String>,
    lconn: Option<String>,
    p3_conn: Option<String>,
    prop_bang: Option<String>,
    rpro_dod: Option<f64>,
    rpro_dthi: Option<f64>,
    atta_adlongth: Option<f64>,
    pub(super) dmf_targets: Vec<RefNo>,
    catalogue_primitives: Vec<CataloguePrimitive>,
    vol: Option<SourceBox>,
    ipara: Vec<f64>,
    ori: Option<[[f64; 3]; 3]>,
    instrument_name: Option<String>,
    connection_target: Option<ConnectionTarget>,
    crfa_targets: Vec<CrfaTarget>,
    atta_support: Option<AttaSupport>,
}

impl MemberFields {
    pub(super) fn apply(self, member: &mut BranchMember) {
        member.para = self.para;
        member.desp = self.desp;
        member.spref_name = self.spref_name;
        member.cate_name = self.cate_name;
        member.gmref_name = self.gmref_name;
        member.aconn = self.aconn;
        member.lconn = self.lconn;
        member.p3_conn = self.p3_conn;
        member.prop_bang = self.prop_bang;
        member.rpro_dod = self.rpro_dod;
        member.rpro_dthi = self.rpro_dthi;
        member.atta_adlongth = self.atta_adlongth;
        member.dmf_targets = self.dmf_targets.into_iter().map(wire_refno).collect();
        member.catalogue_primitives = self.catalogue_primitives;
        member.vol = self.vol;
        member.ipara = self.ipara;
        member.ori = self.ori;
        member.instrument_name = self.instrument_name;
        member.connection_target = self.connection_target;
        member.crfa_targets = self.crfa_targets;
        member.atta_support = self.atta_support;
    }
}

/// 读一个成员要的上下文：成员、类型、世界矩阵（具名轴转世界系用）与槽位给的到达 / 离开口编号；`instrument_atta`
/// 是 SITE 名含 INST 的 ATTA（位号走所属支架）。
pub(super) struct MemberSite<'a> {
    pub(super) member: &'a DbElement,
    pub(super) noun: &'a str,
    pub(super) world: Option<DMat4>,
    pub(super) arrive_ppoint: Option<u32>,
    pub(super) leave_ppoint: Option<u32>,
    pub(super) instrument_atta: bool,
}

/// 读坏的字段名与原因，最后汇成一条 issue。
#[derive(Default)]
struct Unreadable(Vec<String>);

impl Unreadable {
    fn keep<T, E: std::fmt::Display>(&mut self, field: &str, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.0.push(format!("{field}: {error}"));
                None
            }
        }
    }
}

/// 一个成员的字段，见模块文档。读坏的字段留空，原因进 `issues`。
pub(super) fn member_fields(
    site: &MemberSite<'_>,
    spamap: Option<&SpamapIndex>,
    branch_refno: &str,
    issues: &mut Vec<MbdV2Issue>,
) -> MemberFields {
    let member = site.member;
    let mut bad = Unreadable::default();
    let mut fields = MemberFields {
        vol: spamap
            .and_then(|index| index.get(member.refno()))
            .map(|aabb| SourceBox {
                min: aabb.min.to_array(),
                max: aabb.max.to_array(),
            }),
        ..MemberFields::default()
    };

    let catalogue = bad
        .keep("catref", soft(catalogue_point_set(member)))
        .flatten();
    let set = member.db_set();
    let component = catalogue
        .as_ref()
        .and_then(|points| points.component)
        .map(|refno| set.element(refno));
    let point_set = catalogue
        .as_ref()
        .and_then(|points| points.point_set)
        .map(|refno| set.element(refno));
    let env = DbElementParamEnv::new(Some(member.clone()), component.clone());

    fields.desp = bad
        .keep("DESP", real_array(member, "DESP"))
        .flatten()
        .unwrap_or_default();
    let own_para = bad.keep("PARA", real_array(member, "PARA")).flatten();
    fields.para = match (own_para, &component) {
        (Some(para), _) => para,
        (None, Some(component)) => bad
            .keep("catref PARA", real_array(component, "PARA"))
            .flatten()
            .unwrap_or_default(),
        (None, None) => Vec::new(),
    };
    fields.spref_name = bad.keep("SPRE", referenced_name(member, "SPRE")).flatten();
    if let Some(component) = &component {
        fields.cate_name = bad
            .keep(
                "cate of catref",
                soft(host::ancestor_of_type(component, "CATE"))
                    .map(Option::flatten)
                    .and_then(|cate| cate.map_or(Ok(None), |cate| pml_name(&cate))),
            )
            .flatten();
        fields.gmref_name = bad
            .keep("gmref of catref", referenced_name(component, "GMRE"))
            .flatten();
    }

    let arrive = site.arrive_ppoint.or_else(|| whole_number(member, "ARRI"));
    let leave = site.leave_ppoint.or_else(|| whole_number(member, "LEAV"));
    fields.aconn = bad
        .keep("ACONN", connection(point_set.as_ref(), arrive, &env))
        .flatten();
    fields.lconn = bad
        .keep("LCONN", connection(point_set.as_ref(), leave, &env))
        .flatten();
    fields.p3_conn = bad
        .keep("p3 conn", connection(point_set.as_ref(), Some(3), &env))
        .flatten();

    if site.noun == "ATTA" {
        if component.is_some() {
            fields.prop_bang = env.rpro("BANG").map(|angle| angle.to_string());
            fields.rpro_dod = env.rpro("DOD");
            fields.rpro_dthi = env.rpro("DTHI");
        }
        fields.atta_adlongth = bad
            .keep(
                ":ATTA-ADLONGTH",
                host::optional_double(member, ":ATTA-ADLONGTH"),
            )
            .flatten();
    }

    fields.dmf_targets = bad.keep("DMFARR", dmf_targets(member)).unwrap_or_default();
    fields.ipara = bad
        .keep("IPARA", insulation_parameters(member, &env))
        .unwrap_or_default();
    if let Some(component) = &component {
        let bare_env = (!fields.ipara.is_empty()).then(|| {
            DbElementParamEnv::new(Some(member.clone()), Some(component.clone()))
                .without_insulation()
        });
        let frame = AxisFrame {
            member,
            rotation: site.world.map(|world| DMat3::from_mat4(world)),
            env: &env,
            bare_env: bare_env.as_ref(),
        };
        fields.catalogue_primitives = bad
            .keep(
                "catalogue primitives",
                catalogue_primitives(site.noun, component, &frame),
            )
            .unwrap_or_default();
    }

    fields.ori = site.world.and_then(world_axes);
    if matches!(site.noun, "TEE" | "OLET" | "VALV") {
        fields.instrument_name = bad
            .keep(":YKSX", host::optional_text(member, ":YKSX"))
            .flatten();
    }
    if matches!(site.noun, "TEE" | "OLET") {
        fields.connection_target = bad
            .keep("CREF target", referenced_target(member, "CREF"))
            .flatten();
    }
    if site.noun == "PCOM" {
        fields.crfa_targets = bad.keep("CRFA", crfa_targets(member)).unwrap_or_default();
    }
    if site.instrument_atta {
        fields.atta_support = bad.keep(":NPHSASR", atta_support(member)).flatten();
    }

    if !bad.0.is_empty() {
        let object_refno = wire_refno(member.refno());
        issues.push(info_issue(
            branch_refno,
            &object_refno,
            "adapter.member_field_unreadable",
            format!(
                "{} {object_refno}: these solver fields could not be read and are left out: {}",
                site.noun,
                bad.0.join("; ")
            ),
        ));
    }
    fields
}

/// 宿主的非致命说明（字段读坏、墙板转不成候选、SPAMAP 读不动）：Info / Data，不抑制任何图元。
pub(super) fn info_issue(
    branch_refno: &str,
    object_refno: &str,
    rule_id: &str,
    message: String,
) -> MbdV2Issue {
    MbdV2Issue {
        id: format!("{branch_refno}:{rule_id}:{object_refno}"),
        severity: IssueSeverity::Info,
        category: IssueCategory::Data,
        message,
        refno: Some(branch_refno.to_string()),
        isoline_idx: None,
        object_refno: Some(object_refno.to_string()),
        rule_id: Some(rule_id.to_string()),
    }
}

/// PML `name of`：有存名给原文，没有给 `=w0/w1`；元素不在所钉会话按没有。
fn pml_name(element: &DbElement) -> Result<Option<String>, DbElementError> {
    Ok(soft(element.stored_name())?.map(|stored| {
        stored.unwrap_or_else(|| {
            let refno = element.refno();
            format!("={}/{}", refno.word0, refno.word1 as i32)
        })
    }))
}

/// `name of <引用> of $!el`；引用不在模板里、为空都按没有。
fn referenced_name(element: &DbElement, reference: &str) -> Result<Option<String>, DbElementError> {
    match host::optional_element(element, reference)? {
        Some(target) => pml_name(&target),
        None => Ok(None),
    }
}

/// 实数数组属性（整数数组按值转）；不在模板里、没写都按没有。
fn real_array(element: &DbElement, name: &str) -> Result<Option<Vec<f64>>, DbElementError> {
    match element.attribute(name) {
        Ok(Some(DescriptorValue::RealArray(items))) => Ok(Some(items)),
        Ok(Some(DescriptorValue::IntArray(items))) => {
            Ok(Some(items.into_iter().map(f64::from).collect()))
        }
        Ok(_) | Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 存成 int 或 word 的整数属性（`ARRI` / `LEAV` / `NUMB`）。
pub(super) fn whole_number(element: &DbElement, name: &str) -> Option<u32> {
    match element.attribute(name) {
        Ok(Some(DescriptorValue::Int(value))) => u32::try_from(value).ok(),
        Ok(Some(DescriptorValue::Word { raw, .. })) => Some(raw),
        _ => None,
    }
}

/// 词哈希的名字：系统名表优先，再按 `db1_dehash`（`catalogue_expr` 还原拼出来的名字是同一顺序）。
fn word_name(hash: u32) -> Option<String> {
    lookup_system_name(hash)
        .map(String::from)
        .or_else(|| db1_dehash(hash))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// 求值得到的实数当词哈希读：只认正整数。
fn word_from_number(value: f64) -> Option<String> {
    (value.is_finite() && value > 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX))
        .then(|| word_name(value as u32))
        .flatten()
}

/// P 点 `n` 的连接类型（PML `aconn` / `lconn` / `p<n> conn`），见模块文档。
fn connection(
    point_set: Option<&DbElement>,
    number: Option<u32>,
    env: &DbElementParamEnv,
) -> Result<Option<String>, DbElementError> {
    let (Some(point_set), Some(number)) = (point_set, number) else {
        return Ok(None);
    };
    for point in point_set.members()? {
        if whole_number(&point, "NUMB") == Some(number) {
            return connection_word(&point, env);
        }
    }
    Ok(None)
}

fn connection_word(
    point: &DbElement,
    env: &DbElementParamEnv,
) -> Result<Option<String>, DbElementError> {
    match point.attribute("PCON") {
        Ok(None) | Err(DbElementError::UnknownAttribute { .. }) => return Ok(None),
        Ok(Some(DescriptorValue::Word { raw, text })) => return Ok(text.or_else(|| word_name(raw))),
        Ok(Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words))) => {
            if let Some(CatalogueExpression::Named(name)) = catalogue_expr::decode(&words) {
                return Ok(Some(name));
            }
        }
        Ok(Some(_)) => {}
        Err(error) => return Err(error),
    }
    Ok(point
        .get_number("PCON", env)?
        .value()
        .and_then(word_from_number))
}

/// `dmfarr of $!mem`：模板里叫 `DMFA` 的引用数组，Nulref 跳过。
fn dmf_targets(member: &DbElement) -> Result<Vec<RefNo>, DbElementError> {
    match member.attribute("DMFA") {
        Ok(Some(DescriptorValue::RefNoArray(refnos))) => {
            Ok(refnos.into_iter().filter(RefNo::is_valid).collect())
        }
        Ok(Some(DescriptorValue::RefNo(refno))) => {
            Ok(refno.is_valid().then_some(refno).into_iter().collect())
        }
        Ok(None) | Err(DbElementError::UnknownAttribute { .. }) => Ok(Vec::new()),
        Ok(Some(other)) => Err(DbElementError::TypeMismatch {
            refno: member.refno(),
            name: "DMFA".to_string(),
            found: format!("{other:?}"),
            wanted: "RefNoArray",
        }),
        Err(error) => Err(error),
    }
}

/// PML `name of site of $!el`：属主链上第一个 SITE 的 NAME，没有存名给 `=w0/w1`（管段号前缀按它含不含 `NPIC` / `23`
/// 分写法，无名的 SITE 也照 PML 参与判断）。
fn site_name(element: &DbElement) -> Result<Option<String>, DbElementError> {
    match host::ancestor_of_type(element, "SITE")? {
        Some(site) => pml_name(&site),
        None => Ok(None),
    }
}

/// 世界矩阵的三根轴（PML `ori of $!mem wrt /*`），归一；有一根退化就不给。
fn world_axes(world: DMat4) -> Option<[[f64; 3]; 3]> {
    let rotation = DMat3::from_mat4(world);
    let axis = |column: DVec3| column.try_normalize().map(|unit| unit.to_array());
    Some([
        axis(rotation.x_axis)?,
        axis(rotation.y_axis)?,
        axis(rotation.z_axis)?,
    ])
}

/// 引用属性指到的连接对象本身（plant-mbd `ConnectionTarget`）；引用不在模板里、为空都按没有。
fn referenced_target(
    element: &DbElement,
    reference: &str,
) -> Result<Option<ConnectionTarget>, BranchQueryError> {
    match host::optional_element(element, reference)? {
        Some(target) => connection_target(&target),
        None => Ok(None),
    }
}

/// 连接对象的 refno、类型与它自己所在 ZONE 的标志；目标不在所钉会话按没有。
fn connection_target(target: &DbElement) -> Result<Option<ConnectionTarget>, BranchQueryError> {
    let Some(noun) = soft(host::noun_of(target))? else {
        return Ok(None);
    };
    Ok(Some(ConnectionTarget {
        refno: wire_refno(target.refno()),
        noun,
        zone: zone_flags(target)?,
    }))
}

/// PCOM 的 `crfa`（模板里叫 `CRFA`，元素引用数组）：库内序，跳过 Nulref（PML `unset()`）与不在所钉会话的目标。
fn crfa_targets(member: &DbElement) -> Result<Vec<CrfaTarget>, DbElementError> {
    let refnos = match member.attribute("CRFA") {
        Ok(Some(DescriptorValue::RefNoArray(refnos))) => refnos,
        Ok(Some(DescriptorValue::RefNo(refno))) => vec![refno],
        Ok(None) | Err(DbElementError::UnknownAttribute { .. }) => return Ok(Vec::new()),
        Ok(Some(other)) => {
            return Err(DbElementError::TypeMismatch {
                refno: member.refno(),
                name: "CRFA".to_string(),
                found: format!("{other:?}"),
                wanted: "RefNoArray",
            });
        }
        Err(error) => return Err(error),
    };
    let set = member.db_set();
    let mut out = Vec::new();
    for refno in refnos.into_iter().filter(RefNo::is_valid) {
        let target = set.element(refno);
        let Some(name) = pml_name(&target)? else {
            continue;
        };
        out.push(CrfaTarget {
            refno: wire_refno(refno),
            name,
            hpos: host::world_point(&target, "HPOS")?.map(|pos| pos.to_array()),
        });
    }
    Ok(out)
}

/// SITE 名含 INST 的 ATTA 所属支架（`isoTag` L155-163）：UDA `:NPHSASR` 第一项的属主，带它的 `:ZD_BZBM`。AMS 字典里
/// 这个 UDA 是单个 REF，PML 写的是数组 `[1]`，两样都收；第一项是 Nulref、目标或属主不在所钉会话都按没有。
fn atta_support(member: &DbElement) -> Result<Option<AttaSupport>, DbElementError> {
    let first = match member.attribute(":NPHSASR") {
        Ok(Some(DescriptorValue::RefNo(refno))) => refno,
        Ok(Some(DescriptorValue::RefNoArray(refnos))) => match refnos.first() {
            Some(&refno) => refno,
            None => return Ok(None),
        },
        Ok(None) | Err(DbElementError::UnknownAttribute { .. }) => return Ok(None),
        Ok(Some(other)) => {
            return Err(DbElementError::TypeMismatch {
                refno: member.refno(),
                name: ":NPHSASR".to_string(),
                found: format!("{other:?}"),
                wanted: "RefNo / RefNoArray",
            });
        }
        Err(error) => return Err(error),
    };
    if !first.is_valid() {
        return Ok(None);
    }
    let Some(support) = soft(member.db_set().element(first).owner())?.flatten() else {
        return Ok(None);
    };
    let Some(name) = pml_name(&support)? else {
        return Ok(None);
    };
    Ok(Some(AttaSupport {
        refno: wire_refno(support.refno()),
        name: Some(name),
        function_code: host::optional_text(&support, ":ZD_BZBM")?,
    }))
}

/// `ipara of $!mem`，见模块文档。
fn insulation_parameters(
    member: &DbElement,
    env: &DbElementParamEnv,
) -> Result<Vec<f64>, DbElementError> {
    if host::optional_element(member, "ISPE")?.is_none() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for slot in 1..=IPARA_SLOTS {
        match env.iparam(slot) {
            Some(value) if value.is_finite() => values.push(value),
            _ => break,
        }
    }
    while values.last() == Some(&0.0) {
        values.pop();
    }
    Ok(values)
}

/// 原语的轴求值要的：成员（`Pn` 轴问它的 P 点）、它的世界旋转（具名轴）、求值环境；构件带保温时另有不带保温的
/// 环境（`pdia_bare`，见模块文档）。
struct AxisFrame<'a> {
    member: &'a DbElement,
    rotation: Option<DMat3>,
    env: &'a DbElementParamEnv,
    bare_env: Option<&'a DbElementParamEnv>,
}

/// 几何集下求解器认的原语，各带名下的负体。
fn catalogue_primitives(
    noun: &str,
    component: &DbElement,
    frame: &AxisFrame<'_>,
) -> Result<Vec<CataloguePrimitive>, DbElementError> {
    let order = if noun == "ATTA" {
        ["GSTR", "GMRE"]
    } else {
        ["GMRE", "GSTR"]
    };
    let mut geometry = None;
    for set in order {
        if let Some(found) = host::optional_element(component, set)? {
            geometry = Some(found);
            break;
        }
    }
    let Some(geometry) = geometry else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for primitive in geometry.members()? {
        let kind = host::noun_of(&primitive)?;
        if !PRIMITIVE_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let mut evaluated = evaluate_primitive(&primitive, kind, frame);
        let mut negatives = Vec::new();
        collect_negatives(&primitive, 0, &mut negatives)?;
        for negative in negatives {
            let kind = host::noun_of(&negative)?;
            evaluated
                .negatives
                .push(evaluate_primitive(&negative, kind, frame));
        }
        out.push(evaluated);
    }
    Ok(out)
}

fn collect_negatives(
    owner: &DbElement,
    depth: usize,
    out: &mut Vec<DbElement>,
) -> Result<(), DbElementError> {
    if depth >= MAX_NEGATIVE_DEPTH {
        return Ok(());
    }
    for child in owner.members()? {
        if NEGATIVE_KINDS.contains(&host::noun_of(&child)?.as_str()) {
            out.push(child.clone());
        }
        collect_negatives(&child, depth + 1, out)?;
    }
    Ok(())
}

/// 一个原语按实例求值：过滤三件套原样给，尺寸与轴只填这一类型用得到的（字段字典 `CataloguePrimitive`）。读不出的
/// 那一项留空（PML 在这里 `handle any → return`，求解器按缺项跳过这个原语）。
fn evaluate_primitive(
    primitive: &DbElement,
    kind: String,
    frame: &AxisFrame<'_>,
) -> CataloguePrimitive {
    let number = |name: &str| evaluated_number(primitive, name, frame.env);
    let axis = |name: &str| catalogue_axis(primitive, name, frame);
    let mut out = CataloguePrimitive {
        level: level(primitive),
        tuflag: primitive.get_bool("TUFL").ok().flatten(),
        obstruction: obstruction(primitive),
        ..CataloguePrimitive::default()
    };
    match kind.as_str() {
        "SRTO" => {
            out.paax = axis("PAAX");
            out.pbax = axis("PBAX");
            out.pdia = number("PDIA");
            out.phei = number("PHEI");
        }
        "LCYL" | "NLCY" => {
            out.paxi = axis("PAXI");
            out.pdia = number("PDIA");
            out.pbdi = number("PBDI");
            out.ptdi = number("PTDI");
        }
        "SCYL" | "SSCY" | "NSCY" | "NSSC" | "NSSL" => {
            out.paxi = axis("PAXI");
            out.phei = number("PHEI");
            out.pdia = number("PDIA");
            out.pdis = number("PDIS");
        }
        "LSNO" => {
            out.paax = axis("PAAX");
            out.pbax = axis("PBAX");
            out.pbdi = number("PBDI");
            out.ptdi = number("PTDI");
            out.pbdm = number("PBDM");
            out.ptdm = number("PTDM");
            out.poff = number("POFF");
        }
        "SCTO" => {
            out.paax = axis("PAAX");
            out.pbax = axis("PBAX");
            out.pdia = number("PDIA");
        }
        _ => {}
    }
    if matches!(kind.as_str(), "LCYL" | "SCYL" | "SSCY") {
        out.pdia_bare = frame
            .bare_env
            .and_then(|env| evaluated_number(primitive, "PDIA", env));
    }
    out.kind = kind;
    out
}

/// `name` 在 `env` 里求成的有限数；属性不在模板里、unset、求不出都按没有。
fn evaluated_number(primitive: &DbElement, name: &str, env: &DbElementParamEnv) -> Option<f64> {
    primitive
        .get_number(name, env)
        .ok()
        .and_then(|number| number.value())
        .filter(|value| value.is_finite())
}

/// `LEVE`：出图级别区间 `[最小, 最大]`，原值。
fn level(primitive: &DbElement) -> Option<[i32; 2]> {
    let words: Vec<i64> = match primitive.attribute("LEVE").ok()?? {
        DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words) => words
            .into_iter()
            .map(|word| i64::from(word as i32))
            .collect(),
        DescriptorValue::IntArray(values) => values.into_iter().map(i64::from).collect(),
        _ => return None,
    };
    match words.as_slice() {
        [low, high, ..] => Some([i32::try_from(*low).ok()?, i32::try_from(*high).ok()?]),
        _ => None,
    }
}

/// `OBST`：存成 word 或 int。
fn obstruction(primitive: &DbElement) -> Option<i32> {
    match primitive.attribute("OBST").ok()?? {
        DescriptorValue::Word { raw, .. } => i32::try_from(raw).ok(),
        DescriptorValue::Int(value) => Some(value),
        _ => None,
    }
}

/// 轴属性的原始词（`PAXI` / `PAAX` / `PBAX`，几种数组形状都收）。
fn axis_words(primitive: &DbElement, name: &str) -> Option<Vec<u32>> {
    match primitive.attribute(name).ok()?? {
        DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words) => Some(words),
        DescriptorValue::IntArray(words) => {
            Some(words.into_iter().map(|word| word as u32).collect())
        }
        _ => None,
    }
}

/// `Pn` / `-Pn` 轴（`[3, n]`，或带个数前缀的 `[2, 3, n]`）→ `(n, 取反)`；e3d-model `encoded_point_axis` 同一编码。
fn point_axis(words: &[u32]) -> Option<(u32, bool)> {
    let encoded = match words {
        [3, number] => *number,
        [count, 3, number] if *count == 2 => *number,
        _ => return None,
    };
    Some(if encoded >= NEGATED_POINT_BASE {
        (encoded - NEGATED_POINT_BASE, true)
    } else {
        (encoded, false)
    })
}

/// PML `!!evalCata(!mem, <axis> of $!prim)`：具名轴回世界方向；`Pn` 轴回那个 P 点的世界方向与位置。
fn catalogue_axis(
    primitive: &DbElement,
    name: &str,
    frame: &AxisFrame<'_>,
) -> Option<CatalogueAxis> {
    let words = axis_words(primitive, name)?;
    if let Some(local) = axis_spec::direction(&words, frame.env) {
        let dir = (frame.rotation? * DVec3::from_array(local)).try_normalize()?;
        return Some(CatalogueAxis {
            dir: dir.to_array(),
            pos: None,
        });
    }
    let (number, negated) = point_axis(&words)?;
    let point = CataloguePoints.leave(frame.member, Some(number)).ok()??;
    let dir = if negated { -point.dir } else { point.dir };
    Some(CatalogueAxis {
        dir: dir.to_array(),
        pos: Some(point.pos.to_array()),
    })
}

/// 成员之间的导航（PML `pre / next of`、`pre / next tubi of`、`pre / next elbo of`），值是同一 BRAN 成员的 refno：
///
/// - `pre_mem` / `next_mem`：文件序里前 / 后一个 E3D 成员（隐式管身不是成员）；BRAN 两端 E3D 答的是 HREF / TREF 那边
///   的对象，不是本 BRAN 的成员，不给。
/// - `pre_elbo` / `next_elbo`：文件序里最近的 ELBO。
/// - `pre_tubi` / `next_tubi`：站点取最近的管（隐式管身或 FTUB，到达 / 离开槽位合成了管就是它）；穿过件（不断管身的
///   ATTA、WELD）取它所在槽位的那根隐式管身——成员表把隐式管身排在下一站之前，按序往前找会越过它、落到上一段去。
///
/// `slot_from` 是穿过件 refno → 它所在槽位的起点站 refno（Head 起的那段是 `"Head"`）。
pub(super) fn assign_navigation(members: &mut [BranchMember], slot_from: &HashMap<String, String>) {
    let implied = |member: &BranchMember| member.refno.contains('~');
    let tube = |member: &BranchMember| matches!(member.noun.as_deref(), Some("TUBI" | "FTUB"));
    let elbo = |member: &BranchMember| member.noun.as_deref() == Some("ELBO");
    let refno = |member: &BranchMember| member.refno.clone();
    let mut navigation = Vec::with_capacity(members.len());
    for (index, member) in members.iter().enumerate() {
        if implied(member) {
            navigation.push(None);
            continue;
        }
        let (before, after) = (&members[..index], &members[index + 1..]);
        let nearest_before = |accept: &dyn Fn(&BranchMember) -> bool| {
            before.iter().rev().find(|m| accept(m)).map(refno)
        };
        let nearest_after =
            |accept: &dyn Fn(&BranchMember) -> bool| after.iter().find(|m| accept(m)).map(refno);
        let slot_tube = slot_from.get(&member.refno).and_then(|from| {
            let prefix = format!("{from}~");
            members
                .iter()
                .find(|candidate| candidate.refno.starts_with(&prefix))
                .map(refno)
        });
        let entry = MemberNavigation {
            pre_mem: nearest_before(&|m| !implied(m)),
            next_mem: nearest_after(&|m| !implied(m)),
            pre_tubi: slot_tube.clone().or_else(|| nearest_before(&tube)),
            next_tubi: slot_tube.or_else(|| nearest_after(&tube)),
            pre_elbo: nearest_before(&elbo),
            next_elbo: nearest_after(&elbo),
        };
        navigation.push((entry != MemberNavigation::default()).then_some(entry));
    }
    for (member, entry) in members.iter_mut().zip(navigation) {
        member.navigation = entry;
    }
}

/// BRAN 级的 M2b / M2a 字段，与标签 / 调整焊要的 BRAN 级事实。
#[derive(Debug, Default)]
pub(super) struct BranchFields {
    pub(super) flow_direction: Option<String>,
    pub(super) heat_tracing: Option<String>,
    pub(super) dmf_walls: Vec<CivilCandidate>,
    pub(super) site_name: Option<String>,
    pub(super) drawing_number: Option<String>,
    pub(super) document_code: Option<String>,
    pub(super) head_connection_target: Option<ConnectionTarget>,
    pub(super) tail_connection_target: Option<ConnectionTarget>,
    pub(super) head_adjacent_tube: Option<AdjacentTube>,
    pub(super) tail_adjacent_tube: Option<AdjacentTube>,
}

/// `FLOWDIRECTION`（模板里叫 `FLOW`）、伴热 UDA `:ZD_GDBR`、成员 `DMFARR` 指到的墙板；SITE 的 NAME、`:ZD_NBBM` /
/// `:ZD_WJBM`、`HREF` / `TREF` 的目标与相邻 BRAN 的末 / 首根管，见模块文档。
pub(super) fn branch_fields(
    container: &DbElement,
    dmf_targets: &[RefNo],
    branch_refno: &str,
    issues: &mut Vec<MbdV2Issue>,
) -> BranchFields {
    let mut bad = Unreadable::default();
    let mut fields = BranchFields {
        flow_direction: bad
            .keep("FLOWDIRECTION", host::optional_text(container, "FLOW"))
            .flatten(),
        heat_tracing: bad
            .keep(":ZD_GDBR", host::optional_text(container, ":ZD_GDBR"))
            .flatten(),
        site_name: bad.keep("site of", site_name(container)).flatten(),
        drawing_number: bad
            .keep(":ZD_NBBM", host::optional_text(container, ":ZD_NBBM"))
            .flatten(),
        document_code: bad
            .keep(":ZD_WJBM", host::optional_text(container, ":ZD_WJBM"))
            .flatten(),
        ..BranchFields::default()
    };
    if let Some(head) = bad
        .keep("HREF", host::optional_element(container, "HREF"))
        .flatten()
    {
        fields.head_connection_target = bad.keep("HREF target", connection_target(&head)).flatten();
        fields.head_adjacent_tube = bad
            .keep("last tubi of HREF", adjacent_tube(&head, AdjacentEnd::Last))
            .flatten();
    }
    if let Some(tail) = bad
        .keep("TREF", host::optional_element(container, "TREF"))
        .flatten()
    {
        fields.tail_connection_target = bad.keep("TREF target", connection_target(&tail)).flatten();
        fields.tail_adjacent_tube = bad
            .keep(
                "first tubi of TREF",
                adjacent_tube(&tail, AdjacentEnd::First),
            )
            .flatten();
    }
    if !bad.0.is_empty() {
        issues.push(info_issue(
            branch_refno,
            branch_refno,
            "adapter.member_field_unreadable",
            format!(
                "BRAN {branch_refno}: these solver fields could not be read and are left out: {}",
                bad.0.join("; ")
            ),
        ));
    }
    let set = container.db_set();
    let mut seen = Vec::new();
    for &target in dmf_targets {
        if seen.contains(&target) {
            continue;
        }
        seen.push(target);
        let target_refno = wire_refno(target);
        let mut notes = Vec::new();
        match civil_candidate_of(&set.element(target), &mut notes) {
            Ok(candidate) => fields.dmf_walls.push(candidate),
            Err(error) => issues.push(info_issue(
                branch_refno,
                &target_refno,
                "adapter.dmf_wall_unresolved",
                format!(
                    "DMFARR target {target_refno} could not be turned into a wall candidate: {error}"
                ),
            )),
        }
        if !notes.is_empty() {
            issues.push(info_issue(
                branch_refno,
                &target_refno,
                "adapter.dmf_wall_note",
                notes.join("；"),
            ));
        }
    }
    fields
}
