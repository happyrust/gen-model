//! MBD 统一端点的宿主侧（plant3d-web#103 M7）：被选中的一个元素 → 标注类型 + 归属对象 → plant-mbd 统一入口
//! [`plant_mbd::layout_object`]。HTTP 端点 `GET /api/mbd/v2/object/{refno}` 在 `web_service::mbd`。
//!
//! # 判型（[`classify`]）
//!
//! PML `function/export/getmbdtype.pmlfnc` 加各窗体 `addonece` 的取对象口径，分两步：
//!
//! 1. **结构**：沿属主链（含自身）认对象族与归属对象，先中者为准——
//!    - BRAN → `pipe`，归属 BRAN，管件 / ATTA 等成员上溯到它；属主是 HVAC 的 BRAN 不可标注（风管不出管道尺寸，
//!      与 `/pipe` 的 `NotPipeRoute` 同一道门）；
//!    - SUPPO → `support`，归属 SUPPO；
//!    - EQUI → `equipment`，归属 EQUI，NOZZ / ELCONN / SUBE / 图元上溯到它（设备窗体的 `equi` 导航）；
//!    - WALL / GWALL / STWALL / FLOOR → `civil`，归属那块墙板，门窗 FITT、负体上溯到它（土建窗体按这四类上溯）；
//!    - 支架区（[`MbdTypeRules::is_support_zone`]）里 ZONE 一级的 STRU / REST 及其下属（FRMW / SCTN / PANE / HANG /
//!      PCLA…）→ `support`，归属见下节。
//!
//!    都不中（PIPE / ZONE / SITE 这类容器、CWALL、房间 FRMW、支架区外的 STRU…）→
//!    [`ObjectQueryError::NotAnnotatable`]。
//! 2. **关键字**：`DbOption.toml` 的 `[mbd_type_rules]`（[`crate::options::MbdTypeRulesConfig`]）按 SITE / ZONE 名
//!    在九种标注类型里定一种。
//!
//! 结构先于关键字，比 PML 窄一处：getmbdtype 对 ZONE 名含 MTGD / COMM / CASE / LIGHT 的**任何**元素都回 EQUI，
//! 这里只对 EQUI 名下的元素生效，BRAN 落在这种区里照样按管道判——设备窗体收到 BRAN 本来也只会报
//! 「Select equi pls」。
//!
//! UDET 支架（`:SUPP`）还认不出：`UDTYPE` 翻成 `:名` 要 DICT 库里的 UDET 表（`e3d_model::udet::UdetCatalog`），
//! AMS 里没有这种支架。
//!
//! # 支架归属（[`support_parts`]，PML `function/marksupport/getsuppname.pmlfnc`）
//!
//! - 有 SUPPO（风管支架那一支，L80-91）：归属 SUPPO，STRU 取它第一个 STRU 成员，「REST」取第一个 SUPC 成员。
//! - 没有 SUPPO：PML 先取 `zone of`（L60-77，那是 `:SUPP` 一个支架自成一区的建法）；AMS 的支架区里是成对的 ZONE 级
//!   STRU + REST，走 PML 按名字配对的那一支（L22-58）：REST 名取 `/REST` 之前的部分得支架名 S，配对的 STRU 依次找
//!   `S/STRU`、`S`；反过来 STRU 名取 `/STRU` 之前得 S，REST 依次找 `S/REST`、`S`。只在**同一 ZONE 的成员**里找
//!   （PML 是全库按名查，这里不做全树扫）。归属对象是 STRU；选中 REST / HANG / PCLA 而配不到 STRU 时 REST 自己当
//!   归属对象，并在视图 `issues` 里留一条（入口转成 `data.view_issue` 告警）。
//!
//! # 出包（[`layout_classified`]）
//!
//! 管道两类走现成的 [`layout_pipe`]，再补数据包层的 `annotation_type` / `subject`（`/api/mbd/v2/pipe/{refno}` 是
//! 管道的别名，输出不变）。EM9 管道现在与 EM4 同一份布局——plant-mbd 的 `LayoutRequest::instrument_pipe` 还是空
//! 操作，EM9 规则接线时改走 `layout_object`。设备两类读真视图（[`equipment_view`]：
//! [`super::equipment_query::query_equipment`] + 判型说明）进 `layout_object`，土建 / 建筑同样（[`civil_view`]：
//! [`super::civil_query::query_civil_with`]，按标注类型取土建或建筑的数），支架三类同样（[`support_view`]：
//! [`super::support_query::query_support`]）。只有归属的占位视图（[`stub_source`]）只留给要「只看归属」的调用方。
//! `meta.source_version` 一律按 `layout_pipe` 的口径注入。

use std::sync::{Arc, OnceLock};

use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::refno::RefNo;
use e3d_model::category::{RouteDiscipline, route_discipline};
use plant_mbd::{
    AnnotationType, CivilQueryResult, EquipmentQueryResult, InMemoryMbdSource, MbdV2PipeData,
    MbdV2Subject, ObjectLayoutRequest, SupportQueryResult, layout_object,
};

use super::civil_query::{CivilMode, CivilQueryError, DoorRule, query_civil_with};
use super::equipment_query::{EquipmentQueryError, query_equipment};
use super::host::{self, ChainLink, HostError, soft};
use super::support_query::{SupportQueryError, query_support};
use super::{BranchQueryError, SOURCE_TO_DESIGN_MM_TO_M, layout_pipe, wire_refno};
use crate::options::{
    MbdTypeRuleConfig, MbdTypeRulesConfig, default_mbd_type_rules, effective_mbd_type_rules,
};

/// 土建 / 建筑的归属对象类型（PML 土建窗体 `coll all (FLOOR WALL GWALL STWALL)`）。
pub const CIVIL_NOUNS: [&str; 4] = ["WALL", "GWALL", "STWALL", "FLOOR"];
/// 支架区里能当支架根的 ZONE 级元素。
pub const SUPPORT_ROOT_NOUNS: [&str; 2] = ["STRU", "REST"];
/// PML 各窗体把标注存在名为 `<对象名>-MBD` 的 STRU 里（支架窗体遇到它直接 return），它不是支架。
pub const MBD_AID_SUFFIX: &str = "-MBD";
/// 自身就是层级容器、没有标注的类型。
const CONTAINER_NOUNS: [&str; 5] = ["WORL", "SITE", "ZONE", "PIPE", "HVAC"];

/// 结构判出的对象族，与 `[[mbd_type_rules.rules]]` 的 `subject` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    Pipe,
    Support,
    Equipment,
    Civil,
}

impl SubjectKind {
    pub const ALL: [Self; 4] = [Self::Pipe, Self::Support, Self::Equipment, Self::Civil];

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Pipe => "pipe",
            Self::Support => "support",
            Self::Equipment => "equipment",
            Self::Civil => "civil",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.wire_name().eq_ignore_ascii_case(text))
    }

    /// 这个对象族能出哪些标注类型。
    pub fn accepts(self, annotation_type: AnnotationType) -> bool {
        use AnnotationType::*;
        match self {
            Self::Pipe => matches!(annotation_type, Em4Pipe | Em9Pipe),
            Self::Support => matches!(annotation_type, Em4Supp | Em9InstSupp | Em9PipeSupp),
            Self::Equipment => matches!(annotation_type, Em4Equi | Em9Equi),
            Self::Civil => matches!(annotation_type, Civil | Arch),
        }
    }

    /// plant-mbd 对象 fixture 里视图的键（`plant-mbd-cli` `object_case.rs`）。
    pub const fn view_key(self) -> &'static str {
        match self {
            Self::Pipe => "branch",
            Self::Support => "support",
            Self::Equipment => "equipment",
            Self::Civil => "civil",
        }
    }
}

/// plant-mbd `AnnotationType` 的 wire 值（`em4_pipe` …）→ 枚举，不分大小写。
pub fn parse_annotation_type(text: &str) -> Option<AnnotationType> {
    let text = text.trim();
    AnnotationType::ALL
        .into_iter()
        .find(|annotation_type| annotation_type.wire_name().eq_ignore_ascii_case(text))
}

/// 规则表的一条（已校验）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRule {
    pub kind: SubjectKind,
    pub site_keywords: Vec<String>,
    pub zone_keywords: Vec<String>,
    pub annotation_type: AnnotationType,
}

impl TypeRule {
    /// 对象族相同，SITE / ZONE 关键字各自为空或命中（[`host::name_contains_any`]）。
    pub fn matches(
        &self,
        kind: SubjectKind,
        site_name: Option<&str>,
        zone_name: Option<&str>,
    ) -> bool {
        self.kind == kind
            && (self.site_keywords.is_empty()
                || host::name_contains_any(site_name, &self.site_keywords))
            && (self.zone_keywords.is_empty()
                || host::name_contains_any(zone_name, &self.zone_keywords))
    }
}

/// 生效的判型规则（`[mbd_type_rules]` 校验之后）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbdTypeRules {
    pub support_zone_keywords: Vec<String>,
    pub support_zone_purposes: Vec<String>,
    pub support_exclude_keywords: Vec<String>,
    pub rules: Vec<TypeRule>,
}

impl MbdTypeRules {
    /// 缺省规则表（[`default_mbd_type_rules`]）。
    pub fn defaults() -> Self {
        Self::from_config(&default_mbd_type_rules()).0
    }

    /// 校验一份配置：认不出的 `subject` / `annotation_type`、对象族与标注类型对不上的规则跳过并各留一句话；
    /// 一条可用的都不剩时回落缺省规则表。
    pub fn from_config(config: &MbdTypeRulesConfig) -> (Self, Vec<String>) {
        let config = effective_mbd_type_rules(Some(config));
        let mut warnings = Vec::new();
        let mut rules = typed_rules(&config.rules, &mut warnings);
        if rules.is_empty() {
            warnings.push("没有一条可用的规则，回落缺省规则表".to_string());
            rules = typed_rules(&default_mbd_type_rules().rules, &mut Vec::new());
        }
        (
            Self {
                support_zone_keywords: config.support_zone_keywords,
                support_zone_purposes: config.support_zone_purposes,
                support_exclude_keywords: config.support_exclude_keywords,
                rules,
            },
            warnings,
        )
    }

    /// 本进程的规则表：第一次用到时按 `DbOption.toml` 校验一次，跳过的规则在 stderr 点名。
    pub fn current() -> Arc<Self> {
        static CURRENT: OnceLock<Arc<MbdTypeRules>> = OnceLock::new();
        CURRENT
            .get_or_init(|| {
                let (rules, warnings) = Self::from_config(&crate::options::mbd_type_rules());
                for warning in warnings {
                    eprintln!("⚠ [mbd_type_rules] {warning}");
                }
                Arc::new(rules)
            })
            .clone()
    }

    /// 第一条命中的规则给出的标注类型；一条都不中给 `None`。
    pub fn annotation_type(
        &self,
        kind: SubjectKind,
        site_name: Option<&str>,
        zone_name: Option<&str>,
    ) -> Option<AnnotationType> {
        self.rules
            .iter()
            .find(|rule| rule.matches(kind, site_name, zone_name))
            .map(|rule| rule.annotation_type)
    }

    /// 支架区：ZONE 的 NAME 含 `support_zone_keywords` 之一，或 PURP 属于 `support_zone_purposes`（AMS 的
    /// `/1RX02-LSUP`、`/1RX03-MSUP` 名字里没有 SUPP，靠 `PURP = SUPP` 认出来）。
    pub fn is_support_zone(&self, zone_name: Option<&str>, zone_purpose: Option<&str>) -> bool {
        host::name_contains_any(zone_name, &self.support_zone_keywords)
            || zone_purpose.is_some_and(|purpose| {
                self.support_zone_purposes
                    .iter()
                    .any(|wanted| wanted.trim().eq_ignore_ascii_case(purpose.trim()))
            })
    }

    /// 支架根的名字含 `support_exclude_keywords` 之一（缺省 `CIV`）就不算支架。
    pub fn excludes_support(&self, root_name: Option<&str>) -> bool {
        host::name_contains_any(root_name, &self.support_exclude_keywords)
    }
}

fn typed_rules(configured: &[MbdTypeRuleConfig], warnings: &mut Vec<String>) -> Vec<TypeRule> {
    let mut rules = Vec::with_capacity(configured.len());
    for (index, rule) in configured.iter().enumerate() {
        let at = index + 1;
        let Some(kind) = SubjectKind::parse(&rule.subject) else {
            warnings.push(format!(
                "第 {at} 条规则的 subject {:?} 不认识（pipe / support / equipment / civil），跳过",
                rule.subject
            ));
            continue;
        };
        let Some(annotation_type) = parse_annotation_type(&rule.annotation_type) else {
            warnings.push(format!(
                "第 {at} 条规则的 annotation_type {:?} 不是 plant-mbd 的九种标注类型之一，跳过",
                rule.annotation_type
            ));
            continue;
        };
        if !kind.accepts(annotation_type) {
            warnings.push(format!(
                "第 {at} 条规则把 {} 判成 {}，对象族与标注类型对不上，跳过",
                kind.wire_name(),
                annotation_type.wire_name()
            ));
            continue;
        }
        rules.push(TypeRule {
            kind,
            site_keywords: rule.site_keywords.clone(),
            zone_keywords: rule.zone_keywords.clone(),
            annotation_type,
        });
    }
    rules
}

/// [`classify`] 的结果。
#[derive(Debug, Clone)]
pub struct Classified {
    pub annotation_type: AnnotationType,
    pub kind: SubjectKind,
    /// 归属对象（BRAN / SUPPO / STRU / REST / EQUI / WALL / GWALL / STWALL / FLOOR）。
    pub subject_refno: RefNo,
    pub subject_noun: String,
    /// 归属对象的 NAME 原文（`/…`），没存名是 `None`。
    pub subject_name: Option<String>,
    /// 属主链上 SITE / ZONE 的 NAME 原文。
    pub site_name: Option<String>,
    pub zone_name: Option<String>,
    /// 判型留下的非致命说明（目前只有「REST 配不到 STRU」），进占位视图的 `issues`。
    pub notes: Vec<String>,
}

impl Classified {
    pub fn subject_wire_refno(&self) -> String {
        wire_refno(self.subject_refno)
    }

    /// 数据包层的 `subject`。
    pub fn subject(&self) -> MbdV2Subject {
        MbdV2Subject {
            refno: self.subject_wire_refno(),
            noun: self.subject_noun.clone(),
        }
    }
}

#[derive(Debug)]
pub enum ObjectQueryError {
    /// refno 在所钉会话里不存在（→ 404）。
    NotFound {
        refno: RefNo,
    },
    /// 元素存在，但不是任何一种标注的对象（→ 422 `not_annotatable`）。
    NotAnnotatable {
        refno: RefNo,
        noun: String,
        reason: String,
    },
    /// 管道两类的出包失败，按 `/pipe` 的口径映射。
    Pipe(BranchQueryError),
    /// 统一入口拒绝（归属空、视图查不到；占位路径上理论上不会到这里）。
    Solver {
        refno: RefNo,
        message: String,
    },
    SourceChanged {
        dbnum: u32,
    },
    Db(DbElementError),
    Internal(String),
}

impl std::fmt::Display for ObjectQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { refno } => write!(f, "refno {refno} 在所钉会话里不存在"),
            Self::NotAnnotatable {
                refno,
                noun,
                reason,
            } => write!(f, "{noun} {refno} 没有 MBD 标注：{reason}"),
            Self::Pipe(error) => write!(f, "{error}"),
            Self::Solver { refno, message } => write!(f, "{refno} 求解器拒绝：{message}"),
            Self::SourceChanged { dbnum } => write!(f, "库 {dbnum} 在读取期间变化，请重新读取"),
            Self::Db(error) => write!(f, "{error}"),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ObjectQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pipe(error) => Some(error),
            Self::Db(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbElementError> for ObjectQueryError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

impl From<HostError> for ObjectQueryError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Db(error) => Self::Db(error),
            HostError::SourceChanged { dbnum } => Self::SourceChanged { dbnum },
            error @ HostError::Spamap { .. } => Self::Internal(error.to_string()),
        }
    }
}

impl From<EquipmentQueryError> for ObjectQueryError {
    fn from(error: EquipmentQueryError) -> Self {
        match error {
            EquipmentQueryError::NotFound { refno } => Self::NotFound { refno },
            EquipmentQueryError::Db(error) => Self::Db(error),
        }
    }
}

impl From<CivilQueryError> for ObjectQueryError {
    fn from(error: CivilQueryError) -> Self {
        match error {
            CivilQueryError::NotFound { refno } => Self::NotFound { refno },
            CivilQueryError::Db(error) => Self::Db(error),
        }
    }
}

impl From<SupportQueryError> for ObjectQueryError {
    fn from(error: SupportQueryError) -> Self {
        match error {
            SupportQueryError::NotFound { refno } => Self::NotFound { refno },
            SupportQueryError::Db(error) => Self::Db(error),
        }
    }
}

/// 判型：标注类型 + 归属对象，规则取 [`MbdTypeRules::current`]。口径见模块文档。
pub fn classify(set: &Arc<DbSet>, refno: RefNo) -> Result<Classified, ObjectQueryError> {
    classify_with(set, refno, &MbdTypeRules::current())
}

/// [`classify`]，规则由调用方给。
pub fn classify_with(
    set: &Arc<DbSet>,
    refno: RefNo,
    rules: &MbdTypeRules,
) -> Result<Classified, ObjectQueryError> {
    let element = set.element(refno);
    match element.exists() {
        Ok(true) => {}
        Ok(false)
        | Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => {
            return Err(ObjectQueryError::NotFound { refno });
        }
        Err(error) => return Err(error.into()),
    }
    let chain = host::owner_chain(&element)?;
    let target = &chain[0];
    let site_name = match chain.iter().find(|link| link.noun == "SITE") {
        Some(site) => soft(site.element.stored_name())?.flatten(),
        None => None,
    };
    let zone_index = chain.iter().position(|link| link.noun == "ZONE");
    let zone_name = match zone_index {
        Some(index) => soft(chain[index].element.stored_name())?.flatten(),
        None => None,
    };
    let first = |nouns: &[&str]| {
        chain
            .iter()
            .find(|link| nouns.contains(&link.noun.as_str()))
    };

    let (kind, subject, notes) = if let Some(bran) = first(&["BRAN"]) {
        if route_discipline(&bran.element)? == RouteDiscipline::Hvac {
            return Err(not_annotatable(
                target,
                format!("{} 的属主是 HVAC，风管不出管道尺寸标注", describe(bran)),
            ));
        }
        (SubjectKind::Pipe, bran.clone(), Vec::new())
    } else if let Some(suppo) = first(&["SUPPO"]) {
        (SubjectKind::Support, suppo.clone(), Vec::new())
    } else if let Some(equi) = first(&["EQUI"]) {
        (SubjectKind::Equipment, equi.clone(), Vec::new())
    } else if let Some(wall) = first(&CIVIL_NOUNS) {
        (SubjectKind::Civil, wall.clone(), Vec::new())
    } else if CONTAINER_NOUNS.contains(&target.noun.as_str()) {
        let reason = if matches!(target.noun.as_str(), "PIPE" | "HVAC") {
            "它是 BRAN 的容器，请选其中一条 BRAN".to_string()
        } else {
            "层级容器没有标注，请选其中的构件".to_string()
        };
        return Err(not_annotatable(target, reason));
    } else {
        match zone_support(&chain, zone_index, zone_name.as_deref(), rules)? {
            ZoneSupport::Root { subject, notes } => (SubjectKind::Support, subject, notes),
            ZoneSupport::Refused(reason) => return Err(not_annotatable(target, reason)),
        }
    };

    let Some(annotation_type) =
        rules.annotation_type(kind, site_name.as_deref(), zone_name.as_deref())
    else {
        return Err(not_annotatable(
            target,
            format!(
                "mbd_type_rules 里没有一条 {} 规则命中 SITE {} / ZONE {}",
                kind.wire_name(),
                site_name.as_deref().unwrap_or("（无名）"),
                zone_name.as_deref().unwrap_or("（无名）")
            ),
        ));
    };
    Ok(Classified {
        annotation_type,
        kind,
        subject_refno: subject.element.refno(),
        subject_noun: subject.noun.clone(),
        subject_name: soft(subject.element.stored_name())?.flatten(),
        site_name,
        zone_name,
        notes,
    })
}

enum ZoneSupport {
    Root {
        subject: ChainLink,
        notes: Vec<String>,
    },
    Refused(String),
}

/// 支架区里 ZONE 一级的 STRU / REST（AMS 没有 SUPPO、也没有 `:SUPP`，支架就是区里成对的 STRU + REST）。
fn zone_support(
    chain: &[ChainLink],
    zone_index: Option<usize>,
    zone_name: Option<&str>,
    rules: &MbdTypeRules,
) -> Result<ZoneSupport, DbElementError> {
    let target = &chain[0];
    let unrelated = || {
        ZoneSupport::Refused(format!(
            "{} 不在 BRAN / SUPPO / EQUI / WALL·GWALL·STWALL·FLOOR 之下，也不是支架区里 ZONE 一级的 STRU / REST 或其下属",
            target.noun
        ))
    };
    let Some(zone_index) = zone_index.filter(|index| *index > 0) else {
        return Ok(unrelated());
    };
    let zone = &chain[zone_index];
    let root = &chain[zone_index - 1];
    if !SUPPORT_ROOT_NOUNS.contains(&root.noun.as_str()) {
        return Ok(unrelated());
    }
    let purpose = host::optional_text(&zone.element, "PURP")?;
    if !rules.is_support_zone(zone_name, purpose.as_deref()) {
        return Ok(ZoneSupport::Refused(format!(
            "{} 所在的 {} 不是支架区（NAME 不含 {:?}，PURP {} 不在 {:?}），也不在 WALL / GWALL / STWALL / FLOOR 之下",
            describe(root),
            describe(zone),
            rules.support_zone_keywords,
            purpose.as_deref().unwrap_or("unset"),
            rules.support_zone_purposes
        )));
    }
    let root_name = soft(root.element.stored_name())?.flatten();
    if rules.excludes_support(root_name.as_deref()) {
        return Ok(ZoneSupport::Refused(format!(
            "{} 的名字含 {:?} 之一，PML 支架窗体不把它当支架",
            describe(root),
            rules.support_exclude_keywords
        )));
    }
    if root_name
        .as_deref()
        .is_some_and(|name| name.ends_with(MBD_AID_SUFFIX))
    {
        return Ok(ZoneSupport::Refused(format!(
            "{} 是 PML 存放标注的 STRU（名字以 {MBD_AID_SUFFIX} 结尾），不是支架",
            describe(root)
        )));
    }
    let parts = zone_level_parts(&zone.element, root, root_name)?;
    if let Some(stru) = parts.stru {
        return Ok(ZoneSupport::Root {
            subject: ChainLink {
                element: stru,
                noun: "STRU".to_string(),
            },
            notes: Vec::new(),
        });
    }
    let base = parts.name.unwrap_or_default();
    Ok(ZoneSupport::Root {
        subject: root.clone(),
        notes: vec![format!(
            "{} 在 {} 里配不到 STRU（找过 {base}/STRU、{base}），以 REST 为归属对象",
            describe(root),
            describe(zone)
        )],
    })
}

/// 一个支架的几个部分（PML getsuppname 回的 `[suppname, struname, restname]`）。
#[derive(Clone)]
pub struct SupportParts {
    /// PML `suppname`：SUPPO 的名字；ZONE 级支架是 STRU / REST 名去掉 `/STRU` / `/REST` 之后的部分。
    pub name: Option<String>,
    pub suppo: Option<DbElement>,
    pub stru: Option<DbElement>,
    /// REST；风管支架（SUPPO）是第一个 SUPC 成员。
    pub rest: Option<DbElement>,
}

impl SupportParts {
    /// 归属对象：SUPPO，否则 STRU，否则 REST。
    pub fn root(&self) -> Option<&DbElement> {
        self.suppo
            .as_ref()
            .or(self.stru.as_ref())
            .or(self.rest.as_ref())
    }
}

/// PML `getsuppname` 的移植（不判支架区，那是 [`classify`] 的事；配对口径见模块文档）。元素不在 SUPPO 之下、
/// 也不在 ZONE 一级的 STRU / REST 之下时给 `None`。
pub fn support_parts(element: &DbElement) -> Result<Option<SupportParts>, DbElementError> {
    let chain = host::owner_chain(element)?;
    if let Some(suppo) = chain.iter().find(|link| link.noun == "SUPPO") {
        return Ok(Some(SupportParts {
            name: soft(suppo.element.stored_name())?.flatten(),
            suppo: Some(suppo.element.clone()),
            stru: suppo.element.members_of_type("STRU")?.into_iter().next(),
            rest: suppo.element.members_of_type("SUPC")?.into_iter().next(),
        }));
    }
    let Some(zone_index) = chain
        .iter()
        .position(|link| link.noun == "ZONE")
        .filter(|index| *index > 0)
    else {
        return Ok(None);
    };
    let root = &chain[zone_index - 1];
    if !SUPPORT_ROOT_NOUNS.contains(&root.noun.as_str()) {
        return Ok(None);
    }
    let root_name = soft(root.element.stored_name())?.flatten();
    zone_level_parts(&chain[zone_index].element, root, root_name).map(Some)
}

/// ZONE 级 STRU / REST 按名字配对（PML getsuppname L22-58）。
fn zone_level_parts(
    zone: &DbElement,
    root: &ChainLink,
    root_name: Option<String>,
) -> Result<SupportParts, DbElementError> {
    let (own_suffix, pair_suffix, pair_noun) = if root.noun == "STRU" {
        ("/STRU", "/REST", "REST")
    } else {
        ("/REST", "/STRU", "STRU")
    };
    let base = root_name.as_deref().map(|name| {
        name.find(own_suffix)
            .map_or(name, |at| &name[..at])
            .to_string()
    });
    let pair = match base.as_deref() {
        Some(base) => find_pair(zone, base, pair_suffix, pair_noun)?,
        None => None,
    };
    let (stru, rest) = if root.noun == "STRU" {
        (Some(root.element.clone()), pair)
    } else {
        (pair, Some(root.element.clone()))
    };
    Ok(SupportParts {
        name: base,
        suppo: None,
        stru,
        rest,
    })
}

/// ZONE 成员里类型为 `noun` 的 `base + suffix`，没有再找 `base` 本身（PML 的先后）。
fn find_pair(
    zone: &DbElement,
    base: &str,
    suffix: &str,
    noun: &str,
) -> Result<Option<DbElement>, DbElementError> {
    let preferred = format!("{base}{suffix}");
    let mut fallback = None;
    for member in zone.members()? {
        let Some(name) = soft(member.stored_name())?.flatten() else {
            continue;
        };
        if name != preferred && name != base {
            continue;
        }
        if soft(host::noun_of(&member))?.as_deref() != Some(noun) {
            continue;
        }
        if name == preferred {
            return Ok(Some(member));
        }
        if fallback.is_none() {
            fallback = Some(member);
        }
    }
    Ok(fallback)
}

fn not_annotatable(target: &ChainLink, reason: String) -> ObjectQueryError {
    ObjectQueryError::NotAnnotatable {
        refno: target.element.refno(),
        noun: target.noun.clone(),
        reason,
    }
}

/// 报错用的「noun 名字」，没存名用 `=w0/w1`。
fn describe(link: &ChainLink) -> String {
    let refno = link.element.refno();
    let name = link
        .element
        .stored_name()
        .ok()
        .flatten()
        .unwrap_or_else(|| format!("={}/{}", refno.word0, refno.word1 as i32));
    format!("{} {name}", link.noun)
}

/// `GET /api/mbd/v2/object/{refno}` 的主体：判型（[`classify`]）→ 出包（[`layout_classified`]）。
pub fn layout_object_refno(
    set: &Arc<DbSet>,
    refno: RefNo,
    input_refno: &str,
) -> Result<MbdV2PipeData, ObjectQueryError> {
    let classified = classify(set, refno)?;
    layout_classified(set, &classified, input_refno)
}

/// 按判型结果出 V2 数据包；`input_refno` 原样回填，`branch_refno` / `subject.refno` 是归属对象的 `w0_w1`。
pub fn layout_classified(
    set: &Arc<DbSet>,
    classified: &Classified,
    input_refno: &str,
) -> Result<MbdV2PipeData, ObjectQueryError> {
    if classified.kind == SubjectKind::Pipe {
        let mut data = layout_pipe(set, classified.subject_refno, input_refno)
            .map_err(ObjectQueryError::Pipe)?;
        data.annotation_type = Some(classified.annotation_type);
        data.subject = Some(classified.subject());
        return Ok(data);
    }
    let request = ObjectLayoutRequest::new(
        classified.annotation_type,
        input_refno,
        classified.subject_wire_refno(),
        classified.subject_noun.clone(),
        SOURCE_TO_DESIGN_MM_TO_M,
    );
    let source = match classified.kind {
        SubjectKind::Equipment => InMemoryMbdSource {
            equipments: vec![equipment_view(set, classified)?],
            ..InMemoryMbdSource::default()
        },
        SubjectKind::Civil => InMemoryMbdSource {
            civils: vec![civil_view(set, classified)?],
            ..InMemoryMbdSource::default()
        },
        SubjectKind::Support => InMemoryMbdSource {
            supports: vec![support_view(set, classified)?],
            ..InMemoryMbdSource::default()
        },
        _ => stub_source(classified),
    };
    let mut data =
        layout_object(&request, &source).map_err(|message| ObjectQueryError::Solver {
            refno: classified.subject_refno,
            message,
        })?;
    data.meta.source_version = Some(host::source_version(set, classified.subject_refno.dbno())?);
    Ok(data)
}

/// 设备的真视图：[`query_equipment`] 读归属 EQUI，判型的 `notes` 排在读取器的问题之前（统一端点与
/// `examples/mbd_object_dump` 共用）。
pub fn equipment_view(
    set: &Arc<DbSet>,
    classified: &Classified,
) -> Result<EquipmentQueryResult, ObjectQueryError> {
    let mut view = query_equipment(set, classified.subject_refno)?;
    view.issues.splice(0..0, classified.notes.iter().cloned());
    Ok(view)
}

/// 土建 / 建筑的真视图：[`query_civil_with`] 按标注类型取数（建筑只给 NXTR、楼板旁的墙带门，门规则取
/// [`DoorRule::current`]），判型的 `notes` 排在读取器的问题之前（统一端点与 `examples/mbd_object_dump` 共用）。
pub fn civil_view(
    set: &Arc<DbSet>,
    classified: &Classified,
) -> Result<CivilQueryResult, ObjectQueryError> {
    let mut view = query_civil_with(
        set,
        classified.subject_refno,
        CivilMode::of(classified.annotation_type),
        &DoorRule::current(),
    )?;
    view.issues.splice(0..0, classified.notes.iter().cloned());
    Ok(view)
}

/// 支架三类的真视图：[`query_support`] 读归属支架（SUPPO，或 ZONE 级 STRU + REST），判型的 `notes` 排在读取器的
/// 问题之前（统一端点与 `examples/mbd_object_dump` 共用）。
pub fn support_view(
    set: &Arc<DbSet>,
    classified: &Classified,
) -> Result<SupportQueryResult, ObjectQueryError> {
    Ok(query_support(set, classified)?)
}

/// 归属对象的占位视图：只有 refno / noun / NAME，判型的 `notes` 进 `issues`。设备、土建、支架都已走真视图
/// （[`equipment_view`] / [`civil_view`] / [`support_view`]），这里只留给要「只看归属」的调用方；管道不用（走
/// [`layout_pipe`]），给空源。
pub fn stub_source(classified: &Classified) -> InMemoryMbdSource {
    let subject_refno = classified.subject_wire_refno();
    let subject_noun = classified.subject_noun.clone();
    let name = classified.subject_name.clone();
    let issues = classified.notes.clone();
    let mut source = InMemoryMbdSource::default();
    match classified.kind {
        SubjectKind::Support => source.supports.push(SupportQueryResult {
            subject_refno,
            subject_noun,
            name,
            issues,
            ..Default::default()
        }),
        SubjectKind::Civil => source.civils.push(CivilQueryResult {
            subject_refno,
            subject_noun,
            name,
            issues,
            ..Default::default()
        }),
        SubjectKind::Equipment => source.equipments.push(EquipmentQueryResult {
            subject_refno,
            subject_noun,
            name,
            issues,
            ..Default::default()
        }),
        SubjectKind::Pipe => {}
    }
    source
}
