//! 属性变化 → 模型生成影响的判定（「宁多勿漏」保守机制）。
//!
//! 取代 `pdms_io::EleOperationDetail::is_geometry_update` 的「白名单 + 类型硬门」旧语义。
//! 旧语义只有「类型命中硬编码名单」且「改动属性命中极小几何属性集」才判为需重生成，
//! 命不中即当作不用重生成 —— 这是**漏判 = 模型陈旧**的正确性 bug。
//!
//! 新语义（对齐 `plant-model-gen`）：
//! - 未知属性 / UDA 一律**保守触发**（漏判=正确性 bug；误判=多算一次，成本可控）；
//! - 只有明确的业务元数据（`NAME`/`DESC`/`PURP`/`FUNCTION`）判为中性、可跳过；
//! - noun/类型不再作为分类前置门，只用于最终生成器路由。
//!
//! 属性清单来源：AVEVA Everything3D `core.dll` / `Core3D.dll` 逆向（`DCHC/EVALAT`
//! 设计变化层，非 `wnoevt` 事件门）+ 运行库 `att_meta`(702) 交叉校验（100% 命中），
//! 详见 `plant-model-gen/docs/reverse/core_dll_noun_att_model_update.md` §13/§14。

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, OnceLock};

use aios_core::pdms_types::{DbAttributeType, TOTAL_LOOP_NOUN_NAMES, TOTAL_VERT_NOUN_NAMES};
use aios_core::{NamedAttrValue, RefU64Vec, RefnoEnum};
use pdms_io::io::{
    ChildrenDelta, EleOperationData, EleOperationDetail, ModifiedElement, classify_children_delta,
};
use serde::Deserialize;

/// 属性对模型生成输入的影响（三态，保留作向后兼容 / 粗判）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeModelImpact {
    AffectsModel,
    KnownNeutral,
    Unknown,
}

/// 更细的「效果分类」——把 bool/三态升级为 effect（对齐 core.dll/Core3D 逆向
/// reverse §7/§13.4 的建议）。影响判定：`DataOnly` 不影响模型；其余（含 `Unknown`）
/// 都触发（宁多勿漏）。`TransformOnly` 走便宜的 world-transform 更新，其余触发重生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AttributeEffect {
    /// 纯业务元数据：NAME/DESC/PURP/FUNCTION，不影响几何/模型。
    DataOnly,
    /// 仅位姿/方位：POS/ORI/…，只需更新 world transform，网格不变。
    TransformOnly,
    /// 直接几何输入：尺寸/形状/图元/P-point 参数，改动直接改元素网格。
    DirectGeometry,
    /// 目录/规格/设计表/连接依赖：CATR/SPRE/PRTREF/HREF/… 经外部目录或邻居级联影响几何。
    DependencyCascade,
    /// 结构/成员/类型：OWNER/CHILDREN/NOUN/TYPE/LEVE，改变层级、生成分派或参与范围。
    StructuralMembership,
    /// 未知属性：静态清单未覆盖，保守视为影响模型。
    Unknown,
}

impl AttributeEffect {
    /// 是否影响模型（需重生成或变换更新）。仅 `DataOnly` 为否。
    #[inline]
    pub fn affects_model(self) -> bool {
        !matches!(self, AttributeEffect::DataOnly)
    }
}

/// 一次操作对模型的处理动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationImpact {
    /// 需重生成几何模型（新增/删除/几何或未知属性变化）。
    Regen,
    /// 仅位姿变化：不重生成网格，只更新子树的 world transform。
    TransformOnly,
    /// 无需处理（纯业务元数据变化 / 无操作）。
    Skip,
}

/// 一次操作的「效果汇总」：动作标志 + 观察到的效果集合 + 保留的原始 DCHC code + 变更属性名。
#[derive(Debug, Clone, Default)]
pub struct OperationEffectSummary {
    /// 命中需重生成的效果（DirectGeometry/DependencyCascade/StructuralMembership/Unknown）。
    pub impact_regen: bool,
    /// 命中纯位姿效果（TransformOnly）。
    pub impact_transform: bool,
    /// 本次变更观察到的去重效果集合（排序）。
    pub effects: Vec<AttributeEffect>,
    /// 若可静态获得，保留 Core3D 原始设计变化码（DCHC，取变更属性里的最大值）。
    pub max_dchc: Option<i32>,
    /// 归一化后的变更属性名（含 `UDA:<id>`）。
    pub changed_attributes: Vec<String>,
    /// 数组属性中实际变化的 `(属性名, 一基 qualifier)`。
    pub qualified_changes: Vec<(String, usize)>,
    /// Ordered child-list semantics when present.
    pub children_delta: Option<ChildrenDelta>,
}

impl OperationEffectSummary {
    /// 归约为调度用的三态动作。
    pub fn impact(&self) -> OperationImpact {
        if self.impact_regen {
            OperationImpact::Regen
        } else if self.impact_transform {
            OperationImpact::TransformOnly
        } else {
            OperationImpact::Skip
        }
    }
}

/// 纯位姿/方位属性：只改这些时走「仅更新 world transform」的便宜路径（网格不变）。
///
/// 口径对齐 E3D 字典的设计变化类（DCHC）：内核只把 `POS`/`ORI`/`BFORI` 归入位姿类（码 3）。
/// 原先还收了 `POSL`/`POSS`/`POSE`/`NPOS`/`CPOS`/`YDIR`/`ZDIR`，但它们在内核里属通用类（码 4），
/// 且都是几何定义参数而非摆放参数——`POSS`/`POSE` 只属于 `SCTN`/`STWALL`（型材几何就是沿起点
/// →终点拉伸而成），`CPOS` 属于 `CURVE`，`YDIR` 属于 `SPINE`。移出后它们经
/// `attribute_affects_model` 落 `DirectGeometry` 走重生成。`BFORI` 未补入：往便宜路径里加属性
/// 是「少算」方向，无实证不做。依据见 `docs/2026-07-26_p3-t903-t904-assessment.md`。
pub const TRANSFORM_ONLY_ATTR_NAMES: &[&str] = &["POS", "ORI"];

/// 业务元数据（不影响模型）。
pub const DATA_ONLY_ATTR_NAMES: &[&str] = &["NAME", "DESC", "PURP", "FUNCTION", "CACHID", "LCHKDA"];

/// 结构/成员/类型属性（改变层级、生成分派或参与范围）。
pub const STRUCTURAL_ATTR_NAMES: &[&str] = &["OWNER", "CHILDREN", "NOUN", "TYPE", "LEVE", "LEVEL"];

/// 目录/规格/设计表/连接依赖属性（经外部目录或邻居级联影响几何；含未来可反向传播的引用）。
///
/// 三组，按序：catalogue / specification / design-template 引用（`CATR`…`IPARAM`）、
/// 管路连接/方向/布线（`HREF`…`JLIN`，依赖邻居与目录）、设计表默认值 / 属性覆盖
/// （`PKEY`…`PKDI`）。2026-07-26 清理：`SPCO`/`SCOM`/`DDSE`/`DDAT`/`TMPL`/`BRCO` 是
/// dabacon noun 名而非属性，字典 4270 名与运行库 701 名中均不存在，永远不会命中，已移除
/// （推导见 `output/gen_dchc_fixture.py`）。
pub const DEPENDENCY_CASCADE_ATTR_NAMES: &[&str] = &[
    "CATR", "CREF", "SPRE", "SPREF", "PSPREF", "FSPREF", "SCREF", "PSPE", "PRTREF", "DESP", "DKEY",
    "DDPR", "GMREF", "GMRE", "GSTR", "GTYP", "DPRO", "DTRE", "ISPE", "DDANGLE", "DDHEIGHT",
    "DDRADIUS", "IPARAM", "HREF", "TREF", "LSTU", "HSTU", "STYP", "CONN", "HPOS", "TPOS", "HDIR",
    "TDIR", "HBOR", "TBOR", "ADIR", "RDIR", "LDIR", "ZDIS", "LEAV", "CURD", "CURTYP", "OPDI",
    "ROUT", "DRNS", "DRNE", "DETR", "DELP", "RINS", "CTYP", "JFRE", "JLIN", "PKEY", "PPRO", "PSTR",
    "PTRE", "PTYP", "PVER", "PKDI",
];

/// 直接几何输入：尺寸、图元 / P-point 参数、loop/profile 定义等直接改元素自身网格的属性。
///
/// 曾经写在 `attribute_affects_model` 的 `matches!` 里，既无法在运行期枚举（守护测试覆盖
/// 不到），又和上面四张表大量重名——因为旧判定链是短路的，73 条重名项永远走不到，
/// 属于静默失效。改成常量数组后重名即为错误，见 `attribute_effect_tables_have_no_duplicate_names`。
pub const DIRECT_GEOMETRY_ATTR_NAMES: &[&str] = &[
    // 管路、连接与方向依赖。
    "ANGF", "ANGL", "ABOR", "LBOR", "PBOR", "SBOR", "BORE",
    // 通用 primitive 尺寸/形状参数。
    "XLEN", "YLEN", "ZLEN", "LENG", "HEIG", "DIAM", "RADI", "IRAD", "ORAD", "FRAD", "DRAD", "CRAD",
    "DTOP", "DBOT", "XBOT", "YBOT", "XTOP", "YTOP", "XOFF", "YOFF", "ZOFF", "THIC", "WIDE", "DEPT",
    "SIZE", "SHEA", "TAPER", "ECC", "DWID", "DHEI", "DIMD", "SDIA", "SHEI", "STHI", "SWID",
    "ARRHEI", "ARRI", "ARRWID", "LEAHEI", "LEAWID", "MAXA", "CENT", "DCEN", "UBOT", "UCEN", "UTOP",
    // P-point / profile 参数。
    // （2026-07-26 清理：曾混入 31 条 dabacon noun 名（SCTN/STWALL/POHE/POLYHE/SPVE…），
    //  与 noun 表精确同名、在字典与运行库中均无同名属性，永远不会命中，已移除；
    //  推导见 output/gen_dchc_fixture.py。）
    "PTDI", "PTCI", "PAXI", "PHEI", "PANG", "PPOS", "PXDI", "PYDI", "PZDI", "PAAX", "PBAX", "PBBT",
    "PBDI", "PBDM", "PBOF", "PBTP", "PCAX", "PCBT", "PCOF", "PCON", "PCTP", "PDIA", "PDIS", "PLAX",
    "POFF", "PRAD", "PTCD", "PTCP", "PTCPOS", "PTDM", "PWID", "PXBS", "PXLE", "PXTS", "PYBS",
    "PYLE", "PYTS", "PZAXI", "PZLE", "PARA", "PARAM",
    "UNIPAR", // loop/profile/negative geometry definitions。
    "NAPP", "NGMR", "SJUS", "ORRF", "PTOF", "VXREF", "CLFL", "JUSL", "NUMB", "RPRO",
    "TUFL", // 可见性、负实体和布尔生成开关。
    "OBST", "NEG", "POSI", "BOOL",
    // 顶点/坐标：SPVE/SVER/PVER 等顶点改坐标时 modified_attrs 为 PX/PY/PZ。
    "PX", "PY", "PZ", "DX", "DY", // 定位变体、朝向 Y/Z 轴分量与弯角。
    "POSL", "POSS", "POSE", "NPOS", "CPOS", "YDIR", "ZDIR", "BANG",
];

/// 属性 → 效果的**唯一**事实源：按效果分组声明，组间顺序不再携带语义。
///
/// 以前 `classify_attribute_effect` 用 if-else 链逐张表试，「一个属性归哪一类」由它写在链上
/// 的位置决定而不是由语义决定。现在合并成一张映射，重名由测试直接报错。
pub const ATTRIBUTE_EFFECT_TABLES: &[(&[&str], AttributeEffect)] = &[
    (DATA_ONLY_ATTR_NAMES, AttributeEffect::DataOnly),
    (STRUCTURAL_ATTR_NAMES, AttributeEffect::StructuralMembership),
    (TRANSFORM_ONLY_ATTR_NAMES, AttributeEffect::TransformOnly),
    (
        DEPENDENCY_CASCADE_ATTR_NAMES,
        AttributeEffect::DependencyCascade,
    ),
    (DIRECT_GEOMETRY_ATTR_NAMES, AttributeEffect::DirectGeometry),
];

/// 合并后的查找表，懒加载一次。
fn attribute_effects() -> &'static HashMap<&'static str, AttributeEffect> {
    static TABLE: OnceLock<HashMap<&'static str, AttributeEffect>> = OnceLock::new();
    TABLE.get_or_init(|| {
        ATTRIBUTE_EFFECT_TABLES
            .iter()
            .flat_map(|(names, effect)| names.iter().map(move |name| (*name, *effect)))
            .collect()
    })
}

/// 归一化属性名：去掉 `att.` 前缀与限定段，大写。
pub fn normalize_attribute_name(raw_name: &str) -> String {
    raw_name
        .trim()
        .trim_start_matches("att.")
        .trim_start_matches("ATT.")
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// 细粒度效果分类：查 [`ATTRIBUTE_EFFECT_TABLES`] 合并出的映射，未命中则 `Unknown`。
pub fn classify_attribute_effect(raw_name: &str) -> AttributeEffect {
    let name = normalize_attribute_name(raw_name);
    if let Some(effect) = attribute_effects().get(name.as_str()) {
        return *effect;
    }
    // `PARA1`/`PARAM7` 这类带序号的参数变体不逐个登记，按前缀归入几何输入。
    if name.starts_with("PARA") {
        return AttributeEffect::DirectGeometry;
    }
    AttributeEffect::Unknown
}

/// 带 att_meta 提示的效果分类（A2 · ELEMENT 自动级联）。
///
/// 先按名字 [`classify_attribute_effect`] 分类；若名字**未命中任何静态清单**（落 `Unknown`）
/// 且该属性是**引用类型**（att_meta 里 `att_type == ELEMENT`，即指向其它元素的引用），
/// 则升级为 [`AttributeEffect::DependencyCascade`]——引用属性即便不在 curated 依赖清单里，
/// 也应进入目录/邻居**级联反查**（`cascade_refnos`），而非仅按 `Unknown` 重生成自身。
///
/// `is_reference` 由调用方从 schema（`all_attr_info.json` 的 `att_type`）提供；名字已命中
/// 具体清单（Data/Transform/Geometry/Cascade/Structural）时不受 `is_reference` 影响。
pub fn classify_attribute_effect_with_meta(raw_name: &str, is_reference: bool) -> AttributeEffect {
    let base = classify_attribute_effect(raw_name);
    if is_reference && matches!(base, AttributeEffect::Unknown) {
        AttributeEffect::DependencyCascade
    } else {
        base
    }
}

/// 引用类型（`att_type == ELEMENT`）属性名集合——从默认 schema(`all_attr_info`) 聚合、
/// 按名字大写去重，懒加载一次。用于 A2 判定某属性是否为"指向其它元素的引用"。
static REFERENCE_ATTR_NAMES: OnceLock<HashSet<String>> = OnceLock::new();

fn reference_attr_names() -> &'static HashSet<String> {
    REFERENCE_ATTR_NAMES.get_or_init(|| {
        let mut set = HashSet::new();
        let info = aios_core::get_default_pdms_db_info();
        for noun in info.named_attr_info_map.iter() {
            for ai in noun.value().iter() {
                if matches!(ai.att_type, DbAttributeType::ELEMENT) {
                    set.insert(ai.name.trim().to_ascii_uppercase());
                }
            }
        }
        set
    })
}

/// 某属性名是否为**引用类型**（schema `att_type == ELEMENT`）——A2 ELEMENT 自动级联的数据源。
pub fn attribute_is_reference(raw_name: &str) -> bool {
    reference_attr_names().contains(&normalize_attribute_name(raw_name))
}

/// 三态分类（向后兼容）：由细粒度效果归约——DataOnly→KnownNeutral、Unknown→Unknown、其余→AffectsModel。
pub fn classify_attribute_model_impact(raw_name: &str) -> AttributeModelImpact {
    match classify_attribute_effect(raw_name) {
        AttributeEffect::DataOnly => AttributeModelImpact::KnownNeutral,
        AttributeEffect::Unknown => AttributeModelImpact::Unknown,
        _ => AttributeModelImpact::AffectsModel,
    }
}

/// 保留的 Core3D 原始设计变化码（DCHC，字段 596407）。
///
/// 逆向确认的两个**强制 code** 优先且不走字典：`REDRAW` 强制 4、`INTUBE` 强制 1
/// （reverse §4.3/§11.2.1）。其余属性查 E3D 字典快照 [`dchc_change_classes`]——
/// 4270 条（0 码 3941 / 码 1×7 / 码 2×4 / 码 3×3 / 码 4×315）。字典没有的属性
/// （UDA、伪属性）返回 `None`。
pub fn raw_dchc_code(raw_name: &str) -> Option<i32> {
    match normalize_attribute_name(raw_name).as_str() {
        "REDRAW" => Some(4),
        "INTUBE" => Some(1),
        name => dchc_change_classes().get(name).copied(),
    }
}

/// E3D 属性字典的 DCHC 快照：由 `output/gen_dchc_fixture.py` 从字典导出
/// （`output/noun_attr_fields.json`，ADR-008）提炼为 `dchc_change_classes.json`
/// 并随源码入库，编译期嵌入 —— 运行期与测试均无外部文件依赖。懒解析一次。
fn dchc_change_classes() -> &'static HashMap<String, i32> {
    static TABLE: OnceLock<HashMap<String, i32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("dchc_change_classes.json"))
            .expect("dchc_change_classes.json 随源码入库，必须是合法 JSON")
    })
}

/// 属性是否命中「已知会影响模型」的分类——除 `DataOnly` 与 `Unknown` 之外的一切。
///
/// 只表示「命中已知影响集合」；未命中项由 [`classify_attribute_model_impact`] 标为
/// `Unknown`，在采集层保守触发（宁多勿漏）。
pub fn attribute_affects_model(raw_name: &str) -> bool {
    !matches!(
        classify_attribute_effect(raw_name),
        AttributeEffect::DataOnly | AttributeEffect::Unknown
    )
}

/// 收集一次修改里所有变动过的属性名（归一化 + UDA 以 `UDA:<id>` 表示）。
fn collect_modified_attribute_names(modified: &ModifiedElement) -> Vec<String> {
    let mut names = modified
        .added_attrs
        .keys()
        .chain(modified.deleted_attrs.keys())
        .chain(modified.modified_attrs.keys())
        .chain(modified.added_explicit_attrs.keys())
        .chain(modified.deleted_explicit_attrs.keys())
        .chain(modified.modified_explicit_attrs.keys())
        .map(|name| normalize_attribute_name(name))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    names.extend(
        modified
            .added_uda_attrs
            .keys()
            .chain(modified.deleted_uda_attrs.keys())
            .chain(modified.modified_uda_attrs.keys())
            .map(|id| format!("UDA:{id}")),
    );
    names.sort();
    names.dedup();
    names
}

/// 汇总一次操作的效果（宁多勿漏），并保留原始 DCHC code。
///
/// - `Add` / `Deleted` → 需重生成（结构/成员效果）；
/// - `Modified`：逐属性归类效果——`DataOnly` 忽略；`TransformOnly` 记位姿；其余
///   （DirectGeometry/DependencyCascade/StructuralMembership/Unknown，含 `UDA:*`）记重生成；
///   `children` 变化、或检测不到任何属性差异（如非 detail 解析）时保守记重生成；
/// - `None` → 无动作。
pub fn classify_operation_effects(op: &EleOperationData) -> OperationEffectSummary {
    let mut summary = OperationEffectSummary::default();
    match &op.detail {
        EleOperationDetail::Add(_) | EleOperationDetail::Deleted => {
            summary.impact_regen = true;
            summary.effects.push(AttributeEffect::StructuralMembership);
        }
        EleOperationDetail::None => {}
        EleOperationDetail::Modified(modified) => {
            summary.qualified_changes = modified.qualified_attribute_changes();
            if let Some((old, new)) = &modified.children_changed {
                // G3：成员/顺序差分按 `primaryList` 门控（core.dll 只对 primaryList 类型做
                // 成员表差分，见 `elementsChangedBetween` 0x58ffc50）。非 primaryList 类型不
                // 产生成员/顺序**事件标签**（`children_delta` 留空），但 children 变化本身仍
                // 保守触发重生成（宁多勿漏）。
                summary.children_delta = gated_children_delta(&modified.noun, old, new);
                summary.impact_regen = true;
                summary.effects.push(AttributeEffect::StructuralMembership);
            }

            let changed = collect_modified_attribute_names(modified);
            summary.changed_attributes = changed.clone();

            // 无法确认改了什么（如非 detail 解析导致属性差异为空）→ 保守重生成。
            if changed.is_empty() && !summary.impact_regen {
                summary.impact_regen = true;
                summary.effects.push(AttributeEffect::Unknown);
            }

            let mut dchc: Option<i32> = None;
            for name in &changed {
                let effect = if name.starts_with("UDA:") {
                    AttributeEffect::Unknown
                } else {
                    // A2：未命中静态清单的引用属性(att_type=ELEMENT)→DependencyCascade（进级联反查）。
                    classify_attribute_effect_with_meta(name, attribute_is_reference(name))
                };
                if let Some(code) = raw_dchc_code(name) {
                    dchc = Some(dchc.map_or(code, |c| c.max(code)));
                }
                match effect {
                    AttributeEffect::DataOnly => {}
                    AttributeEffect::TransformOnly if is_loop_container_noun(&modified.noun) => {
                        // A point/vertex's local POS is an input of its owner's
                        // mesh, not a standalone instance transform.
                        summary.impact_regen = true;
                    }
                    AttributeEffect::TransformOnly => summary.impact_transform = true,
                    AttributeEffect::DirectGeometry
                    | AttributeEffect::DependencyCascade
                    | AttributeEffect::StructuralMembership
                    | AttributeEffect::Unknown => summary.impact_regen = true,
                }
                if !summary.effects.contains(&effect) {
                    summary.effects.push(effect);
                }
            }
            summary.max_dchc = dchc;
        }
    }
    summary.effects.sort();
    summary.effects.dedup();
    summary
}

/// 判定一次操作应如何处理模型（宁多勿漏）：由 [`classify_operation_effects`] 归约为三态动作。
pub fn classify_operation_impact(op: &EleOperationData) -> OperationImpact {
    classify_operation_effects(op).impact()
}

/// 是否为 loop/坐标辅助类型（LOOP/PLOO/VERT/PAVE、*DATU 等）：这些自身不是
/// 几何生成根，需上溯到非容器 owner（如 PANE/EXTR/WALL/GENSEC）再重生成。
pub fn is_loop_container_noun(noun: &str) -> bool {
    let n = noun.trim().to_ascii_uppercase();
    matches!(n.as_str(), "JLDATU" | "PLDATU" | "ENDATU")
        || parse_pdms_db::dict::default_noun_capabilities()
            .get(&n)
            .map(|caps| caps.point)
            // Keep the established lists as a safe fallback for custom nouns absent
            // from the bundled dabacon snapshot.
            .unwrap_or_else(|| {
                TOTAL_LOOP_NOUN_NAMES.contains(&n.as_str())
                    || TOTAL_VERT_NOUN_NAMES.contains(&n.as_str())
            })
}

pub fn named_attr_refno(value: &NamedAttrValue) -> Option<RefnoEnum> {
    let refno = match value {
        NamedAttrValue::RefU64Type(r) => RefnoEnum::from(*r),
        NamedAttrValue::RefnoEnumType(r) => *r,
        _ => return None,
    };
    refno.is_valid().then_some(refno)
}

/// Extract the OLD and NEW OWNER references from a modify op (element relocation).
///
/// Returns `(old_owner, new_owner)`; either side is `None` when not present.
/// `Modified.modified_attrs["OWNER"]` carries `(old, new)`; a pure add/delete of
/// the OWNER attribute carries only the new / only the old side respectively.
/// Non-modify ops always return `(None, None)`.
pub fn owner_change(op: &EleOperationData) -> (Option<RefnoEnum>, Option<RefnoEnum>) {
    let EleOperationDetail::Modified(modified) = &op.detail else {
        return (None, None);
    };
    let mut old_owner = None;
    let mut new_owner = None;
    for (name, (old, new)) in modified
        .modified_attrs
        .iter()
        .chain(&modified.modified_explicit_attrs)
    {
        if normalize_attribute_name(name) == "OWNER" {
            old_owner = named_attr_refno(old);
            new_owner = named_attr_refno(new);
        }
    }
    for (name, old) in modified
        .deleted_attrs
        .iter()
        .chain(&modified.deleted_explicit_attrs)
    {
        if normalize_attribute_name(name) == "OWNER" {
            old_owner = named_attr_refno(old);
        }
    }
    for (name, new) in modified
        .added_attrs
        .iter()
        .chain(&modified.added_explicit_attrs)
    {
        if normalize_attribute_name(name) == "OWNER" {
            new_owner = named_attr_refno(new);
        }
    }
    (old_owner, new_owner)
}

/// 提取一次修改里 OWNER 属性的新旧引用（元素被搬迁时，新旧 owner 两侧都需重生成）。
pub fn changed_owner_refnos(op: &EleOperationData) -> Vec<RefnoEnum> {
    let (old, new) = owner_change(op);
    old.into_iter().chain(new).collect()
}

// ── core.dll DB_UserChanges 六变化桶（P2 · G1/G2/G3）─────────────────────────
//
// `DB_DB::elementsChangedBetween`(0x58ffc50) 把每个变化元素分派进 `DB_UserChanges`
// 的六个桶（对象偏移 +0/+8/+16/+24/+32/+40，见 `.ida_scratch/analysis/db_userchanges.c`
// 与 v2 测试计划 §1.2 / ADR-009）。本节按其**写入语义**给出纯函数模型，供增量差分
// 与 Batch B 单测复用。

/// core.dll `DB_UserChanges` 的六个变化桶（对象偏移升序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeBucket {
    /// +0：`elementCreated(elem)` —— 新建的元素本身。
    Created,
    /// +8：`elementDeleted(elem)` —— 删除的元素本身。
    Deleted,
    /// +16：`elementIncluded(elem, owner)` —— 因 OWNER 变化被搬迁的元素本身。
    Moved,
    /// +24：`elementCreated` 写其 owner；`elementIncluded` 写**新旧两个** owner；
    /// `elementReordered` 写 owner；`attributeModified(elem, ATT_MEMB)` 写 elem。
    MemberChanged,
    /// +32：`elementReordered(member)` —— primaryList 成员表里位置变化（码 == 3）的成员。
    Reordered,
    /// +40：`attributeModified(elem, attr, qual)` —— 普通属性变化的元素本身。
    Modified,
}

impl ChangeBucket {
    /// 全部六个桶（顺序 = core.dll `DB_UserChanges` 对象偏移升序）。
    pub const ALL: [ChangeBucket; 6] = [
        ChangeBucket::Created,
        ChangeBucket::Deleted,
        ChangeBucket::Moved,
        ChangeBucket::MemberChanged,
        ChangeBucket::Reordered,
        ChangeBucket::Modified,
    ];
}

/// 一个 `Add` 操作里新元素的 owner（G2：新建元素时其 owner 必须记 MemberChanged，
/// 对齐 `elementCreated` 0x5987a90 的 `sub_5986450(this+24, owner)`）。
///
/// 优先取解析出的 `EleData::owner`，回退到属性图里的 `OWNER`；非 `Add` 或无有效 owner
/// 时返回 `None`。
pub fn added_owner(op: &EleOperationData) -> Option<RefnoEnum> {
    let EleOperationDetail::Add(ele) = &op.detail else {
        return None;
    };
    let direct = RefnoEnum::from(ele.owner);
    if direct.is_valid() {
        return Some(direct);
    }
    let from_map = ele.att_map().get_owner();
    from_map.is_valid().then_some(from_map)
}

fn push_bucket(out: &mut Vec<(ChangeBucket, RefnoEnum)>, bucket: ChangeBucket, refno: RefnoEnum) {
    if refno.is_valid() && !out.contains(&(bucket, refno)) {
        out.push((bucket, refno));
    }
}

/// 一次元素操作产生的 `(桶, refno)` 分配，忠实于 core.dll `DB_UserChanges` 写入语义
/// （见 `.ida_scratch/analysis/db_userchanges.c`、ADR-009）：
///
/// - `Add`            → `Created(elem)` + `MemberChanged(owner)`（`elementCreated`，G2）；
/// - `Deleted`        → `Deleted(elem)`（`elementDeleted`；owner 侧刷新由生成根 rollup
///   的属主图负责，`Deleted` 操作本身不带 owner 数据）；
/// - `Modified` 含 OWNER 变化 → `Moved(elem)` + `MemberChanged(旧 owner)` +
///   `MemberChanged(新 owner)`（`elementIncluded`，G1）；
/// - `Modified` 含其它属性变化 → `Modified(elem)`（`attributeModified`）；
/// - `Modified` 且 primaryList 成员表变化 → `MemberChanged(elem)`
///   （`attributeModified(elem, ATT_MEMB)`，按 [`primary_list_hint`] 门控，G3）。
///
/// 一个 `Modified` 同时改 OWNER 和其它属性时，`Moved` 与 `Modified` 都会产生。净变化
/// 折叠（新建后又搬迁 = 净 Created 而非 Moved，B-EVT-06）由
/// [`crate::data_interface::manual_update::fold_net_op`] 处理，不在本单操作层。
pub fn user_change_buckets(op: &EleOperationData) -> Vec<(ChangeBucket, RefnoEnum)> {
    let mut out: Vec<(ChangeBucket, RefnoEnum)> = Vec::new();
    let refno = RefnoEnum::from(op.refno);
    match &op.detail {
        EleOperationDetail::Add(_) => {
            push_bucket(&mut out, ChangeBucket::Created, refno);
            if let Some(owner) = added_owner(op) {
                push_bucket(&mut out, ChangeBucket::MemberChanged, owner);
            }
        }
        EleOperationDetail::Deleted => {
            push_bucket(&mut out, ChangeBucket::Deleted, refno);
        }
        EleOperationDetail::None => {}
        EleOperationDetail::Modified(modified) => {
            let (old_owner, new_owner) = owner_change(op);
            if old_owner.is_some() || new_owner.is_some() {
                push_bucket(&mut out, ChangeBucket::Moved, refno);
                if let Some(o) = old_owner {
                    push_bucket(&mut out, ChangeBucket::MemberChanged, o);
                }
                if let Some(n) = new_owner {
                    push_bucket(&mut out, ChangeBucket::MemberChanged, n);
                }
            }
            // 非 OWNER 的普通属性变化 → Modified(elem)（attributeModified）。
            let has_non_owner_attr = collect_modified_attribute_names(modified)
                .iter()
                .any(|name| name != "OWNER");
            if has_non_owner_attr {
                push_bucket(&mut out, ChangeBucket::Modified, refno);
            }
            // primaryList 成员表变化：owner 元素本身 → MemberChanged
            // （attributeModified(elem, ATT_MEMB)）。成员个体的 Reordered 需成员级操作，
            // 不在本单操作层可见。
            if let Some((old, new)) = &modified.children_changed {
                if gated_children_delta(&modified.noun, old, new).is_some() {
                    push_bucket(&mut out, ChangeBucket::MemberChanged, refno);
                }
            }
        }
    }
    out
}

const CORE_PRIMARY_LIST_SNAPSHOT_JSON: &str =
    include_str!("../../tests/fixtures/core-primary-list-e3d31.json");

#[derive(Deserialize)]
struct CorePrimaryListSnapshot {
    nouns: HashMap<String, bool>,
}

static CORE_PRIMARY_LIST_SNAPSHOT: LazyLock<CorePrimaryListSnapshot> = LazyLock::new(|| {
    serde_json::from_str(CORE_PRIMARY_LIST_SNAPSHOT_JSON)
        .unwrap_or_else(|error| panic!("embedded core primaryList snapshot is invalid: {error}"))
});

fn primary_list_snapshot_value(noun: &str) -> Option<bool> {
    let normalized = noun.trim().to_ascii_uppercase();
    CORE_PRIMARY_LIST_SNAPSHOT.nouns.get(&normalized).copied()
}

/// `DB_Noun::primaryList(noun)` 的离线提示（G3 门控数据源）。
///
/// core.dll 只对 `primaryList == true` 的类型做成员表差分。字段不在普通 dabacon
/// 属性字典中；本实现使用 live E3D 内直接调用 `DB_Noun::getField(297853135, &out)`
/// 冻结的 E3D 3.1 快照。core 的判定是严格 `value == 1`，因此快照里的 `false` 会真正
/// 关掉成员差分。
///
/// 快照覆盖 `noun_flags.json` 的全部 1931 个 noun，没有 unknown（2026-08-28 起）。
/// 此前那 52 个「core 未返回字段值」是**读取通道的假象**——旧通道
/// `db_get_element_info` 只认五个写死的 field id，且 noun 查不到时直接报错返回；
/// 换成 core 自己导出的 `DB_Noun::findNoun` 之后 52 个全部解析，且**全部为
/// `false`**（取证：`docs/evidence/2026-08-28-core-noun-granularity-export.md` §4）。
///
/// 不在快照里的 noun 仍保守返回 `true`，保持「宁多勿漏」——那一支现在只可能是
/// `noun_flags.json` 之外的类型，不再是读不出来的类型。
pub fn primary_list_hint(noun: &str) -> bool {
    match primary_list_snapshot_value(noun) {
        Some(value) => value,
        None => true,
    }
}

/// 成员/顺序差分，按 `primaryList` **显式**门控（纯函数）：非 primaryList 类型
/// （`primary_list == false`）返回 `None`（不产生成员/顺序事件），primaryList 类型
/// 返回 [`classify_children_delta`] 的判定（同集合换序 → `Reordered`，集合增删 →
/// `MemberChanged`）。
pub fn classify_children_delta_gated(
    old: &RefU64Vec,
    new: &RefU64Vec,
    primary_list: bool,
) -> Option<ChildrenDelta> {
    primary_list.then(|| classify_children_delta(old, new))
}

/// 生产路径的成员/顺序差分：用 [`primary_list_hint`] 门控 [`classify_children_delta_gated`]。
pub fn gated_children_delta(noun: &str, old: &RefU64Vec, new: &RefU64Vec) -> Option<ChildrenDelta> {
    classify_children_delta_gated(old, new, primary_list_hint(noun))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn modified_operation(attr_name: &str) -> EleOperationData {
        modified_operation_with_attrs(&[attr_name])
    }

    fn modified_operation_with_attrs(attr_names: &[&str]) -> EleOperationData {
        let mut modified_attrs = HashMap::new();
        for attr_name in attr_names {
            modified_attrs.insert(
                (*attr_name).to_string(),
                (
                    NamedAttrValue::StringType("old".into()),
                    NamedAttrValue::StringType("new".into()),
                ),
            );
        }
        EleOperationData::new(
            aios_core::RefU64((7997u64 << 32) | 1),
            1,
            EleOperationDetail::Modified(ModifiedElement {
                current_data: Default::default(),
                added_attrs: Default::default(),
                deleted_attrs: Default::default(),
                modified_attrs,
                added_explicit_attrs: Default::default(),
                deleted_explicit_attrs: Default::default(),
                modified_explicit_attrs: Default::default(),
                added_uda_attrs: Default::default(),
                deleted_uda_attrs: Default::default(),
                modified_uda_attrs: Default::default(),
                noun: "DAMP".to_string(),
                children_changed: None,
            }),
        )
    }

    fn set_operation_noun(op: &mut EleOperationData, noun: &str) {
        if let EleOperationDetail::Modified(modified) = &mut op.detail {
            modified.noun = noun.to_string();
        }
    }

    #[test]
    fn pipe_spec_reference_regenerates_but_name_only_skips_model_work() {
        for attr_name in ["SPRE", "SPREF", "PSPREF", "FSPREF"] {
            let op = modified_operation(attr_name);
            assert_eq!(
                classify_operation_impact(&op),
                OperationImpact::Regen,
                "{attr_name} must regenerate the pipe delivery unit"
            );
            assert_eq!(
                classify_operation_effects(&op).effects,
                vec![AttributeEffect::DependencyCascade],
                "{attr_name} must participate in reverse-reference cascade"
            );
        }

        assert_eq!(
            classify_operation_impact(&modified_operation("NAME")),
            OperationImpact::Skip,
            "NAME is persisted and reflected in the tree without regenerating geometry"
        );
    }

    #[test]
    fn structural_part_edits_route_to_the_expected_incremental_work() {
        for attr_name in ["FRAD", "HEIG", "DESP"] {
            assert_eq!(
                classify_operation_impact(&modified_operation(attr_name)),
                OperationImpact::Regen,
                "{attr_name} changes FLOOR/WALL geometry"
            );
        }
        assert_eq!(
            classify_operation_impact(&modified_operation("POS")),
            OperationImpact::TransformOnly,
            "moving a structural part only needs an instance transform update"
        );
        assert_eq!(
            classify_operation_effects(&modified_operation("SPRE")).effects,
            vec![AttributeEffect::DependencyCascade],
            "changing a WALL/STWALL profile reference must regenerate catalogue dependants"
        );
        assert_eq!(
            classify_operation_impact(&modified_operation("NAME")),
            OperationImpact::Skip,
            "renaming a structural part updates the tree without rebuilding geometry"
        );

        for attr_name in ["GTYP", "SPRE", "BANG", "DRNS", "DRNE", "FRAD"] {
            assert_eq!(
                classify_operation_impact(&modified_operation(attr_name)),
                OperationImpact::Regen,
                "{attr_name} changes GENSEC profile or sweep geometry"
            );
        }
    }

    #[test]
    fn metadata_only_changes_are_known_neutral() {
        for name in [
            "NAME", "DESC", "PURP", "FUNCTION", "CACHID", "LCHKDA", "att.NAME", "att.DESC",
        ] {
            assert!(!attribute_affects_model(name), "{name}");
            assert_eq!(
                classify_attribute_model_impact(name),
                AttributeModelImpact::KnownNeutral,
                "{name}"
            );
        }
    }

    #[test]
    fn transform_catalogue_topology_and_dimensions_affect_model() {
        for name in [
            "POS", "ORI", "OWNER", "children", "att.CATR", "SPRE", "DIAM", "XLEN", "HREF", "DDPR",
            "PRTREF", "HEIG", "PARA", "PARAM1",
        ] {
            assert!(attribute_affects_model(name), "{name}");
        }
    }

    #[test]
    fn unknown_attribute_is_conservative() {
        assert_eq!(
            classify_attribute_model_impact("SOME_WEIRD_ATTR"),
            AttributeModelImpact::Unknown
        );
    }

    #[test]
    fn effects_are_classified() {
        assert_eq!(classify_attribute_effect("NAME"), AttributeEffect::DataOnly);
        assert_eq!(
            classify_attribute_effect("att.POS"),
            AttributeEffect::TransformOnly
        );
        assert_eq!(
            classify_attribute_effect("DIAM"),
            AttributeEffect::DirectGeometry
        );
        assert_eq!(
            classify_attribute_effect("HEIG"),
            AttributeEffect::DirectGeometry
        );
        assert_eq!(
            classify_attribute_effect("CATR"),
            AttributeEffect::DependencyCascade
        );
        assert_eq!(
            classify_attribute_effect("PRTREF"),
            AttributeEffect::DependencyCascade
        );
        assert_eq!(
            classify_attribute_effect("OWNER"),
            AttributeEffect::StructuralMembership
        );
        assert_eq!(
            classify_attribute_effect("WHATEVER_X"),
            AttributeEffect::Unknown
        );
    }

    #[test]
    fn curated_attribute_tables_map_to_their_declared_effects_and_actions() {
        for name in DATA_ONLY_ATTR_NAMES {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::DataOnly,
                "{name}"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Skip,
                "{name}"
            );
        }
        for name in TRANSFORM_ONLY_ATTR_NAMES {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::TransformOnly,
                "{name}"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::TransformOnly,
                "{name}"
            );
        }
        for name in STRUCTURAL_ATTR_NAMES {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::StructuralMembership,
                "{name}"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Regen,
                "{name}"
            );
        }
        for name in DEPENDENCY_CASCADE_ATTR_NAMES {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::DependencyCascade,
                "{name}"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Regen,
                "{name}"
            );
        }
    }

    #[test]
    fn core_user_change_kinds_and_effect_precedence_are_total() {
        let added = EleOperationData::new(
            aios_core::RefU64((7997u64 << 32) | 2),
            1,
            EleOperationDetail::Add(Default::default()),
        );
        let deleted = EleOperationData::new(
            aios_core::RefU64((7997u64 << 32) | 3),
            1,
            EleOperationDetail::Deleted,
        );
        let none = EleOperationData::new(
            aios_core::RefU64((7997u64 << 32) | 4),
            1,
            EleOperationDetail::None,
        );
        assert_eq!(classify_operation_impact(&added), OperationImpact::Regen);
        assert_eq!(classify_operation_impact(&deleted), OperationImpact::Regen);
        assert_eq!(classify_operation_impact(&none), OperationImpact::Skip);

        let empty_modified = modified_operation_with_attrs(&[]);
        let empty_summary = classify_operation_effects(&empty_modified);
        assert_eq!(empty_summary.impact(), OperationImpact::Regen);
        assert_eq!(empty_summary.effects, vec![AttributeEffect::Unknown]);

        let mixed =
            classify_operation_effects(&modified_operation_with_attrs(&["NAME", "POS", "DIAM"]));
        assert_eq!(mixed.impact(), OperationImpact::Regen);
        assert!(mixed.effects.contains(&AttributeEffect::DataOnly));
        assert!(mixed.effects.contains(&AttributeEffect::TransformOnly));
        assert!(mixed.effects.contains(&AttributeEffect::DirectGeometry));

        let data_and_transform =
            classify_operation_effects(&modified_operation_with_attrs(&["NAME", "ORI"]));
        assert_eq!(data_and_transform.impact(), OperationImpact::TransformOnly);
    }

    #[test]
    fn children_and_uda_changes_conservatively_regenerate() {
        let mut children = modified_operation("NAME");
        if let EleOperationDetail::Modified(modified) = &mut children.detail {
            modified.children_changed = Some(Default::default());
        }
        let children_summary = classify_operation_effects(&children);
        assert_eq!(children_summary.impact(), OperationImpact::Regen);
        assert!(
            children_summary
                .effects
                .contains(&AttributeEffect::StructuralMembership)
        );

        let mut uda = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut uda.detail {
            modified.added_uda_attrs.insert(
                42,
                NamedAttrValue::StringType("custom geometry input".into()),
            );
        }
        let uda_summary = classify_operation_effects(&uda);
        assert_eq!(uda_summary.impact(), OperationImpact::Regen);
        assert_eq!(uda_summary.changed_attributes, vec!["UDA:42"]);
        assert_eq!(uda_summary.effects, vec![AttributeEffect::Unknown]);
    }

    #[test]
    fn point_container_position_regenerates_parent_geometry() {
        let mut op = modified_operation("POS");
        set_operation_noun(&mut op, "POIN");
        assert_eq!(classify_operation_impact(&op), OperationImpact::Regen);

        set_operation_noun(&mut op, "DAMP");
        assert_eq!(
            classify_operation_impact(&op),
            OperationImpact::TransformOnly
        );
    }

    #[test]
    fn child_list_change_distinguishes_reorder_from_membership() {
        use aios_core::pdms_types::{RefU64, RefU64Vec};

        let mut reordered = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut reordered.detail {
            modified.children_changed = Some((
                RefU64Vec(vec![RefU64(1), RefU64(2)]),
                RefU64Vec(vec![RefU64(2), RefU64(1)]),
            ));
        }
        assert_eq!(
            classify_operation_effects(&reordered).children_delta,
            Some(ChildrenDelta::Reordered)
        );

        let mut membership = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut membership.detail {
            modified.children_changed = Some((
                RefU64Vec(vec![RefU64(1), RefU64(2)]),
                RefU64Vec(vec![RefU64(1), RefU64(3)]),
            ));
        }
        assert_eq!(
            classify_operation_effects(&membership).children_delta,
            Some(ChildrenDelta::MemberChanged)
        );
    }

    // ── Batch B：变化类型语义对齐（v2 测试计划 §4 批次 B / ADR-009）────────────

    /// B-EVT-01：`OWNER` 变化产生 Moved 语义——元素记 Moved，**旧 owner 与新 owner
    /// 都记 MemberChanged**（对齐 `elementIncluded` 0x5987ea0）。
    #[test]
    fn b_evt_01_owner_change_records_move_and_both_owner_membership() {
        let old = aios_core::RefU64((7997u64 << 32) | 10);
        let new = aios_core::RefU64((7997u64 << 32) | 20);
        let mut op = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(m) = &mut op.detail {
            m.modified_attrs.insert(
                "OWNER".into(),
                (
                    NamedAttrValue::RefU64Type(old),
                    NamedAttrValue::RefU64Type(new),
                ),
            );
        }
        let elem = RefnoEnum::from(op.refno);
        let buckets = user_change_buckets(&op);
        assert!(
            buckets.contains(&(ChangeBucket::Moved, elem)),
            "moved element must enter Moved: {buckets:?}"
        );
        assert!(
            buckets.contains(&(ChangeBucket::MemberChanged, RefnoEnum::from(old))),
            "old owner must enter MemberChanged: {buckets:?}"
        );
        assert!(
            buckets.contains(&(ChangeBucket::MemberChanged, RefnoEnum::from(new))),
            "new owner must enter MemberChanged: {buckets:?}"
        );
        // 纯 OWNER 变化不额外记 Modified 桶（OWNER 走 elementIncluded，非 attributeModified）。
        assert!(
            !buckets.iter().any(|(b, _)| *b == ChangeBucket::Modified),
            "pure OWNER change must not enter Modified: {buckets:?}"
        );
    }

    /// B-EVT-02：新建元素时其 owner 记 MemberChanged（对齐 `elementCreated` 0x5987a90
    /// 的 `sub_5986450(this+24, owner)`）。
    #[test]
    fn b_evt_02_created_element_records_owner_membership() {
        use parse_pdms_db::parse::EleData;
        let elem = aios_core::RefU64((7997u64 << 32) | 7);
        let owner = aios_core::RefU64((7997u64 << 32) | 55);
        let mut ele = EleData::default();
        ele.owner = owner;
        let op = EleOperationData::new(elem, 1, EleOperationDetail::Add(ele));
        assert_eq!(added_owner(&op), Some(RefnoEnum::from(owner)));
        let buckets = user_change_buckets(&op);
        assert!(
            buckets.contains(&(ChangeBucket::Created, RefnoEnum::from(elem))),
            "new element must enter Created: {buckets:?}"
        );
        assert!(
            buckets.contains(&(ChangeBucket::MemberChanged, RefnoEnum::from(owner))),
            "created element's owner must enter MemberChanged: {buckets:?}"
        );
    }

    /// B-EVT-03：成员差分只对 primaryList 类型执行；非 primaryList 类型的 children
    /// 差异不产生成员/顺序事件（`elementsChangedBetween` 只在 `primaryList` 为真时做
    /// 成员表差分）。
    #[test]
    fn b_evt_03_member_diff_only_runs_for_primary_list_types() {
        use aios_core::pdms_types::{RefU64, RefU64Vec};
        let old = RefU64Vec(vec![RefU64(1), RefU64(2)]);
        let reordered = RefU64Vec(vec![RefU64(2), RefU64(1)]);
        // primaryList 类型：产生成员/顺序事件。
        assert_eq!(
            classify_children_delta_gated(&old, &reordered, true),
            Some(ChildrenDelta::Reordered)
        );
        // 非 primaryList 类型：children 差异不产生成员/顺序事件。
        assert_eq!(classify_children_delta_gated(&old, &reordered, false), None);
        // 生产提示来自 core.dll 快照：DAMP=true，TP=false。ROD 曾经是 52 个「读不出
        // 来」之一、靠兜底取真，2026-08-28 换读取通道后 core 明确答 false。
        assert!(primary_list_hint("DAMP"));
        assert!(primary_list_hint(" damp "));
        assert!(!primary_list_hint("TP"));
        assert!(!primary_list_hint("ROD"));
        // 兜底本身还在，只是现在只对快照之外的 noun 生效。
        assert!(primary_list_hint("NOT-A-NOUN"));
        assert_eq!(
            gated_children_delta("DAMP", &old, &reordered),
            Some(ChildrenDelta::Reordered)
        );
        assert_eq!(gated_children_delta("TP", &old, &reordered), None);

        // 不是只测显式 gate：生产 user_change_buckets 也必须真的抑制 TP 的成员桶。
        let mut non_primary = modified_operation("NAME");
        set_operation_noun(&mut non_primary, "TP");
        if let EleOperationDetail::Modified(modified) = &mut non_primary.detail {
            modified.children_changed = Some((old.clone(), reordered.clone()));
        }
        assert!(
            user_change_buckets(&non_primary)
                .iter()
                .all(|(bucket, _)| *bucket != ChangeBucket::MemberChanged)
        );

        let mut primary = non_primary;
        set_operation_noun(&mut primary, "DAMP");
        assert!(
            user_change_buckets(&primary)
                .iter()
                .any(|(bucket, _)| *bucket == ChangeBucket::MemberChanged)
        );
    }

    /// 递归遍历 `dir` 下的所有 `.rs` 文件，交给 `visit` 看路径与全文。
    fn each_rs_file(dir: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("读取目录 {} 失败: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("目录项不可读").path();
            if path.is_dir() {
                each_rs_file(&path, visit);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
                visit(&path, &text);
            }
        }
    }

    /// ADR-032 边界 A 的守卫。core.dll 的 `DB_MemberCompare` 逐个差异点吐带 kind 的
    /// 记录（`kind == 3` → `elementReordered(member)`），我们的 `classify_children_delta`
    /// 只给整表三态，单操作层还原不出「哪个成员被换了位置」。这个缺口今天不影响输出，
    /// 前提正是 `Reordered` 桶既没人产也没人读——两头任一被打破，边界就得重新审视，
    /// 而不是让一个语义不完整的桶被当成可信输入。
    #[test]
    fn the_reordered_bucket_has_no_producer_and_no_consumer() {
        let own_source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/data_interface/model_impact.rs"
        ));
        let bucket = concat!("ChangeBucket::", "Reordered");
        // 本文件里只该有 `ALL` 数组那一处提到它；多出来的一处就是有人开始产它了。
        assert_eq!(
            own_source.matches(bucket).count(),
            1,
            "model_impact.rs 里这个桶的提及数变了：它今天只列在 ALL 数组里、没有任何产出点，见 ADR-032 边界 A"
        );

        let src_root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut consumers: Vec<String> = Vec::new();
        each_rs_file(&src_root, &mut |path, text| {
            if !path.ends_with("model_impact.rs") && text.contains(bucket) {
                consumers.push(path.display().to_string());
            }
        });
        assert!(
            consumers.is_empty(),
            "有人开始消费 Reordered 桶了（{consumers:?}）：它今天恒为空，见 ADR-032 边界 A"
        );
    }

    #[test]
    fn core_primary_list_snapshot_is_complete_and_self_consistent() {
        let snapshot: serde_json::Value =
            serde_json::from_str(CORE_PRIMARY_LIST_SNAPSHOT_JSON).unwrap();
        assert_eq!(snapshot["schema"], 1);
        assert_eq!(snapshot["field_id"], 297853135);
        assert_eq!(
            snapshot["core_sha256"],
            "668783707a924c343759e99e8676fa8482d14987f4d83f10a83246001a7f5c18"
        );
        assert_eq!(snapshot["count"], 1931);
        assert_eq!(snapshot["resolved_count"], 1931);
        assert_eq!(snapshot["unknown_count"], 0);
        assert_eq!(snapshot["true_count"], 1142);
        assert_eq!(snapshot["false_count"], 789);

        let nouns = snapshot["nouns"].as_object().unwrap();
        let unknown = snapshot["unknown"].as_array().unwrap();
        assert_eq!(nouns.len(), 1931);
        assert!(
            unknown.is_empty(),
            "the 52 unknowns were a read-channel artefact; a refreshed snapshot that \
             brings them back means the exporter regressed onto db_get_element_info"
        );
        assert_eq!(
            nouns
                .values()
                .filter(|value| value.as_bool() == Some(true))
                .count(),
            1142
        );
        assert_eq!(
            nouns
                .values()
                .filter(|value| value.as_bool() == Some(false))
                .count(),
            789
        );

        // The conservative branch is now unreachable from the snapshot itself, so
        // exercise it on a noun the snapshot does not carry - that is the only
        // shape "unknown" can still take.
        assert_eq!(primary_list_snapshot_value("NOT-A-NOUN"), None);
        assert!(primary_list_hint("NOT-A-NOUN"));
    }

    /// B-EVT-04：「同集合换顺序」判为 Reordered、「集合增删」判为 MemberChanged
    /// （事件类型不同），但两者都刷新父生成根（模型受影响 → Regen）。
    #[test]
    fn b_evt_04_reorder_and_membership_are_distinct_but_both_regenerate() {
        use aios_core::pdms_types::{RefU64, RefU64Vec};
        let base = RefU64Vec(vec![RefU64(1), RefU64(2)]);
        let reordered = RefU64Vec(vec![RefU64(2), RefU64(1)]);
        let membership = RefU64Vec(vec![RefU64(1), RefU64(3)]);
        assert_eq!(
            classify_children_delta(&base, &reordered),
            ChildrenDelta::Reordered
        );
        assert_eq!(
            classify_children_delta(&base, &membership),
            ChildrenDelta::MemberChanged
        );
        assert_ne!(
            classify_children_delta(&base, &reordered),
            classify_children_delta(&base, &membership)
        );
        for new in [reordered, membership] {
            let mut op = modified_operation_with_attrs(&[]);
            if let EleOperationDetail::Modified(m) = &mut op.detail {
                m.children_changed = Some((base.clone(), new));
            }
            assert_eq!(
                classify_operation_impact(&op),
                OperationImpact::Regen,
                "child-list change must regenerate the parent generation root"
            );
        }
    }

    #[test]
    fn array_attribute_effect_retains_changed_qualifier() {
        let mut op = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut op.detail {
            modified.modified_attrs.insert(
                "PARA".into(),
                (
                    NamedAttrValue::F32VecType(vec![1.0, 2.0, 3.0]),
                    NamedAttrValue::F32VecType(vec![1.0, 9.0, 3.0]),
                ),
            );
        }
        let summary = classify_operation_effects(&op);
        assert_eq!(summary.qualified_changes, vec![("PARA".into(), 2)]);
        assert_eq!(summary.impact(), OperationImpact::Regen);
    }

    #[test]
    fn all_dictionary_geometry_nouns_follow_the_same_update_contract() {
        let classifier = parse_pdms_db::dict::default_noun_classifier();
        let geometry_nouns: BTreeSet<String> = classifier
            .primitive_nouns()
            .into_iter()
            .chain(classifier.geomset_nouns())
            .chain(classifier.extrusion_nouns())
            .collect();

        assert_eq!(classifier.len(), 1931, "dabacon noun snapshot drifted");
        assert_eq!(
            geometry_nouns.len(),
            395,
            "primitive∪geomset∪extrusion snapshot drifted"
        );

        for noun in geometry_nouns {
            let mut rename = modified_operation("NAME");
            set_operation_noun(&mut rename, &noun);
            assert_eq!(
                classify_operation_impact(&rename),
                OperationImpact::Skip,
                "{noun}: NAME must remain data-only"
            );

            let mut moved = modified_operation("POS");
            set_operation_noun(&mut moved, &noun);
            let expected = if is_loop_container_noun(&noun) {
                OperationImpact::Regen
            } else {
                OperationImpact::TransformOnly
            };
            assert_eq!(
                classify_operation_impact(&moved),
                expected,
                "{noun}: POS must follow point-vs-instance semantics"
            );

            let mut geometry = modified_operation("DIAM");
            set_operation_noun(&mut geometry, &noun);
            assert_eq!(
                classify_operation_impact(&geometry),
                OperationImpact::Regen,
                "{noun}: direct geometry input must regenerate"
            );
        }
    }

    #[test]
    fn owner_move_retains_both_membership_sides() {
        let old = aios_core::RefU64((7997u64 << 32) | 10);
        let new = aios_core::RefU64((7997u64 << 32) | 20);
        let mut op = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut op.detail {
            modified.modified_attrs.insert(
                "OWNER".into(),
                (
                    NamedAttrValue::RefU64Type(old),
                    NamedAttrValue::RefU64Type(new),
                ),
            );
        }

        assert_eq!(
            owner_change(&op),
            (Some(RefnoEnum::from(old)), Some(RefnoEnum::from(new)))
        );
        assert_eq!(
            changed_owner_refnos(&op),
            vec![RefnoEnum::from(old), RefnoEnum::from(new)]
        );
        assert_eq!(classify_operation_impact(&op), OperationImpact::Regen);
    }

    #[test]
    fn explicit_owner_move_retains_both_membership_sides() {
        let old = aios_core::RefU64((7997u64 << 32) | 10);
        let new = aios_core::RefU64((7997u64 << 32) | 20);
        let mut op = modified_operation_with_attrs(&[]);
        if let EleOperationDetail::Modified(modified) = &mut op.detail {
            modified.modified_explicit_attrs.insert(
                "OWNER".into(),
                (
                    NamedAttrValue::RefU64Type(old),
                    NamedAttrValue::RefU64Type(new),
                ),
            );
        }

        assert_eq!(
            owner_change(&op),
            (Some(RefnoEnum::from(old)), Some(RefnoEnum::from(new)))
        );
        assert_eq!(
            changed_owner_refnos(&op),
            vec![RefnoEnum::from(old), RefnoEnum::from(new)]
        );
        assert_eq!(classify_operation_impact(&op), OperationImpact::Regen);
    }

    #[test]
    fn all_dictionary_nouns_have_a_total_incremental_update_policy() {
        let capabilities = parse_pdms_db::dict::default_noun_capabilities();
        let nouns = capabilities
            .iter()
            .filter_map(|caps| {
                let noun = caps.noun_name.trim().to_ascii_uppercase();
                (!noun.is_empty()).then_some(noun)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(nouns.len(), 1931, "dabacon noun snapshot drifted");

        for noun in nouns {
            let mut metadata = modified_operation("NAME");
            set_operation_noun(&mut metadata, &noun);
            assert_eq!(
                classify_operation_impact(&metadata),
                OperationImpact::Skip,
                "{noun}: metadata-only"
            );

            let mut transform = modified_operation("POS");
            set_operation_noun(&mut transform, &noun);
            let expected = if is_loop_container_noun(&noun) {
                OperationImpact::Regen
            } else {
                OperationImpact::TransformOnly
            };
            assert_eq!(
                classify_operation_impact(&transform),
                expected,
                "{noun}: point positions regenerate their owner geometry"
            );

            let mut structure = modified_operation("OWNER");
            set_operation_noun(&mut structure, &noun);
            assert_eq!(
                classify_operation_impact(&structure),
                OperationImpact::Regen,
                "{noun}: owner move"
            );

            let mut unknown = modified_operation("UNCLASSIFIED_MODEL_INPUT");
            set_operation_noun(&mut unknown, &noun);
            assert_eq!(
                classify_operation_impact(&unknown),
                OperationImpact::Regen,
                "{noun}: unknown attributes must conservatively regenerate"
            );
        }
    }

    #[test]
    fn every_dictionary_point_container_is_skipped_as_a_generation_root() {
        let point_nouns = parse_pdms_db::dict::default_noun_capabilities().point_nouns();
        assert_eq!(point_nouns.len(), 44, "dabacon point capability drifted");
        for noun in point_nouns {
            assert!(
                is_loop_container_noun(&noun),
                "{noun}: point container must roll up to its real geometry owner"
            );
        }
        assert!(!is_loop_container_noun("BOX"));
    }

    #[test]
    fn datum_coordinate_helpers_regenerate_their_owner_geometry() {
        for noun in ["JLDATU", "PLDATU", "ENDATU"] {
            assert!(is_loop_container_noun(noun), "{noun}");
            let mut moved = modified_operation("POS");
            set_operation_noun(&mut moved, noun);
            assert_eq!(
                classify_operation_impact(&moved),
                OperationImpact::Regen,
                "{noun} coordinates are inputs of the owner geometry"
            );
        }
    }

    #[test]
    fn effect_affects_model_only_excludes_data_only() {
        assert!(!AttributeEffect::DataOnly.affects_model());
        for e in [
            AttributeEffect::TransformOnly,
            AttributeEffect::DirectGeometry,
            AttributeEffect::DependencyCascade,
            AttributeEffect::StructuralMembership,
            AttributeEffect::Unknown,
        ] {
            assert!(e.affects_model(), "{e:?}");
        }
    }

    /// 独立数据源：运行库属性 schema（`all_attr_info.json` 经 `aios_core` 载入）。
    /// 它与本文件的手工清单没有任何共同来源，因此可以拿来对账。
    fn runtime_attribute_names() -> BTreeSet<String> {
        let info = aios_core::get_default_pdms_db_info();
        let mut names = BTreeSet::new();
        for noun in info.named_attr_info_map.iter() {
            for ai in noun.value().iter() {
                let name = ai.name.trim().to_ascii_uppercase();
                if !name.is_empty() {
                    names.insert(name);
                }
            }
        }
        names
    }

    /// E3D 属性字典的 per-attribute 设计变化码，来自随源码入库的嵌入快照
    /// （见 [`raw_dchc_code`] / `dchc_change_classes.json`）。快照入库后本函数
    /// 恒返回 `Some`——保留 `Option` 签名只为不动既有调用方的解构写法；
    /// 对账测试从此不再依赖未入库的 `output/noun_attr_fields.json`，任何环境都必跑。
    fn dictionary_change_classes() -> Option<BTreeMap<String, i64>> {
        Some(
            dchc_change_classes()
                .iter()
                .map(|(name, code)| (name.clone(), i64::from(*code)))
                .collect(),
        )
    }

    /// 外部对账 · 一：清单里的每个名字都应当是真实存在的属性名。
    ///
    /// 手工清单曾混进 37 条 dabacon noun 名死分支（与 noun 表精确同名、字典与
    /// 运行库均无同名属性），2026-07-26 已清理（推导见 `output/gen_dchc_fixture.py`）。
    /// 这里把每张表「对不上 schema 的条目」钉成明确名单：多一条会失败，清掉一条
    /// 也会失败，后者是提醒同步更新本快照。剩余对不上的条目多为字典有名、但
    /// schema 仍未含属主的真属性，保留（快照口径：d32b06ff 起的 1878-noun
    /// descriptor 表；它比旧 339-noun 快照多收编了 6 个名字，名单已随之重钉）。
    #[test]
    fn curated_tables_are_reconciled_against_the_runtime_schema() {
        let runtime = runtime_attribute_names();
        if runtime.is_empty() {
            return; // 无 schema 的环境软跳过
        }
        let unmatched = |table: &[&str]| -> String {
            table
                .iter()
                .map(|n| n.to_ascii_uppercase())
                .filter(|n| !runtime.contains(n))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        };

        assert!(
            unmatched(TRANSFORM_ONLY_ATTR_NAMES).is_empty(),
            "走便宜路径的属性必须真实存在，否则等于白写：{}",
            unmatched(TRANSFORM_ONLY_ATTR_NAMES)
        );
        // FUNCTION 是历史遗留；CACHID（项目 UDA）与 LCHKDA（BRAN 检查元数据）
        // 在 descriptor 表里有了属主，2026-08-27 起出列。
        assert_eq!(unmatched(DATA_ONLY_ATTR_NAMES), "FUNCTION");
        // CHILDREN / NOUN 是元素元数据而非属性；LEVEL 是 LEVE 的别名写法。
        assert_eq!(unmatched(STRUCTURAL_ATTR_NAMES), "CHILDREN, LEVEL, NOUN");
        // 目录/连接类短名：字典有名（如 ADIR/CONN/SPREF），descriptor 表仍未含属主，
        // 保留；FSPREF / SCREF 已被 1878-noun 表收编（2026-08-27），不再在此名单。
        assert_eq!(
            unmatched(DEPENDENCY_CASCADE_ATTR_NAMES),
            "ADIR, CONN, DDANGLE, DDHEIGHT, DDRADIUS, GMREF, \
             IPARAM, LDIR, PVER, RDIR, SPREF"
        );

        // DirectGeometry：31 条 noun 名死分支清理后剩 39 条；descriptor 表又收编
        // PTOF / SIZE（2026-08-27），余 37 条字典有名但仍无属主的真属性
        // （ABOR/ANGF/PARAM/…），钉住总数防再混入。
        let direct = unmatched(DIRECT_GEOMETRY_ATTR_NAMES);
        assert_eq!(
            direct.split(", ").count(),
            37,
            "DIRECT_GEOMETRY 对不上 schema 的条目数漂移：{direct}"
        );
        for noun_name in ["POHE", "POLYHE", "SEXT", "SPVE", "SCTN", "STWALL"] {
            assert!(
                !direct.contains(noun_name),
                "{noun_name} 是 noun 名死分支，2026-07-26 已从清单移除，不应回流"
            );
        }
    }

    /// B 改造的核心不变量：合并后的映射不得有重名。
    ///
    /// 同一个属性出现在两张效果表里，说明它的语义没定清楚，而不是「靠顺序解决」。
    /// 旧实现里这种重名有 73 条，全部静默失效。
    #[test]
    fn attribute_effect_tables_have_no_duplicate_names() {
        let mut seen: BTreeMap<&str, AttributeEffect> = BTreeMap::new();
        let mut duplicates = Vec::new();
        for (names, effect) in ATTRIBUTE_EFFECT_TABLES {
            for name in *names {
                if let Some(previous) = seen.insert(name, *effect) {
                    duplicates.push(format!("{name}({previous:?} vs {effect:?})"));
                }
            }
        }
        assert!(
            duplicates.is_empty(),
            "同一属性出现在多张效果表里：{}",
            duplicates.join(", ")
        );
    }

    /// 原有的 `curated_attribute_tables_map_to_their_declared_effects_and_actions`
    /// 只覆盖前四张表（当时第五张还是 `matches!` 无法枚举），这里补上。
    #[test]
    fn direct_geometry_table_maps_to_its_declared_effect_and_action() {
        for name in DIRECT_GEOMETRY_ATTR_NAMES {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::DirectGeometry,
                "{name}"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Regen,
                "{name}"
            );
        }
    }

    /// 带序号的参数变体不在表里，靠前缀规则兜底。
    #[test]
    fn numbered_parameter_variants_fall_back_to_the_prefix_rule() {
        for name in ["PARA1", "PARAM7", "PARAXYZ"] {
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::DirectGeometry,
                "{name}"
            );
        }
    }

    /// 外部对账 · 二：两张「减免」白名单必须与 E3D 字典的设计变化类一致。
    ///
    /// `DataOnly` 与 `TransformOnly` 是仅有的两处「少做事」判定，写宽一条的后果是
    /// **模型陈旧且没有任何测试会变红**——`POSS`/`POSE` 当纯位姿处理就这样潜伏了很久。
    /// 字典的 DCHC 是完全独立的第二意见：0 = 无设计变化，3 = 纯位姿类（内核只放了
    /// `POS`/`ORI`/`BFORI` 三条）。依据见 `docs/2026-07-26_p3-t903-t904-assessment.md`。
    #[test]
    fn exemption_tables_match_the_dictionary_change_class() {
        let Some(dchc) = dictionary_change_classes() else {
            return; // 未导出字典的环境软跳过
        };

        for name in DATA_ONLY_ATTR_NAMES {
            if let Some(code) = dchc.get(&name.to_ascii_uppercase()) {
                assert_eq!(
                    *code, 0,
                    "{name} 被判为 DataOnly（完全跳过），字典却给了设计变化码 {code}"
                );
            }
        }

        for name in TRANSFORM_ONLY_ATTR_NAMES {
            assert_eq!(
                dchc.get(&name.to_ascii_uppercase()).copied(),
                Some(3),
                "{name} 走 world-transform 便宜路径，但它不在内核位姿类（DCHC=3）里"
            );
        }

        let pose_class = dchc
            .iter()
            .filter(|(_, code)| **code == 3)
            .map(|(name, _)| name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            pose_class,
            BTreeSet::from(["BFORI", "ORI", "POS"]),
            "内核位姿类成员漂移，TRANSFORM_ONLY 的取值范围需要重新评估"
        );
    }

    #[test]
    fn runtime_noun_attribute_pairs_respect_dictionary_change_classes() {
        let Some(dchc) = dictionary_change_classes() else {
            return; // 未导出字典的环境软跳过
        };
        let info = aios_core::get_default_pdms_db_info();
        let mut pairs = 0usize;
        let mut compared = 0usize;
        for noun in info.named_attr_info_map.iter() {
            for attr in noun.value().iter() {
                let name = attr.name.trim().to_ascii_uppercase();
                if name.is_empty() {
                    continue;
                }
                pairs += 1;
                let Some(code) = dchc.get(&name) else {
                    continue;
                };
                compared += 1;
                let effect =
                    classify_attribute_effect_with_meta(&name, attribute_is_reference(&name));
                if *code != 0 {
                    assert_ne!(
                        effect,
                        AttributeEffect::DataOnly,
                        "{}.{name}: DCHC={code} 不能被完全跳过",
                        noun.key()
                    );
                }
                if effect == AttributeEffect::TransformOnly {
                    assert_eq!(
                        *code,
                        3,
                        "{}.{name}: 只有 DCHC=3 才能走纯位姿便宜路径",
                        noun.key()
                    );
                }
            }
        }
        assert!(pairs > 6_000, "运行时 noun/attribute 对过少: {pairs}");
        assert!(
            compared > 5_000,
            "与活字典 DCHC 可比的 noun/attribute 对过少: {compared}/{pairs}"
        );
    }

    #[test]
    fn dchc_codes_cover_forced_and_dictionary_snapshot() {
        // 逆向确认的强制码优先。
        assert_eq!(raw_dchc_code("REDRAW"), Some(4));
        assert_eq!(raw_dchc_code("INTUBE"), Some(1));
        // 其余走嵌入字典快照：位姿/头端/中性/通用各取一条锚点。
        assert_eq!(raw_dchc_code("POS"), Some(3));
        assert_eq!(raw_dchc_code("HBOR"), Some(1));
        assert_eq!(raw_dchc_code("NAME"), Some(0));
        assert_eq!(raw_dchc_code("DIAM"), Some(4));
        // 字典没有的（UDA、未知名）仍是 None。
        assert_eq!(raw_dchc_code("UDA:42"), None);
        assert_eq!(raw_dchc_code("NO_SUCH_ATTR"), None);
    }

    #[test]
    fn element_reference_meta_upgrades_unknown_to_cascade() {
        // A2：未知属性名 + 引用类型(att_type=ELEMENT) → DependencyCascade（进级联反查）。
        assert_eq!(
            classify_attribute_effect_with_meta("SOME_WEIRD_REF", true),
            AttributeEffect::DependencyCascade
        );
        // 未知属性名 + 非引用 → 仍 Unknown（保守，宁多勿漏）。
        assert_eq!(
            classify_attribute_effect_with_meta("SOME_WEIRD_X", false),
            AttributeEffect::Unknown
        );
        // 名字已命中具体清单时，is_reference 不改变结论。
        assert_eq!(
            classify_attribute_effect_with_meta("NAME", true),
            AttributeEffect::DataOnly
        );
        assert_eq!(
            classify_attribute_effect_with_meta("DIAM", true),
            AttributeEffect::DirectGeometry
        );
        assert_eq!(
            classify_attribute_effect_with_meta("POS", true),
            AttributeEffect::TransformOnly
        );
        assert_eq!(
            classify_attribute_effect_with_meta("CATR", false),
            AttributeEffect::DependencyCascade
        );
        assert_eq!(
            classify_attribute_effect_with_meta("OWNER", true),
            AttributeEffect::StructuralMembership
        );
    }

    /// A2 覆盖：遍历默认 schema(`all_attr_info`) 全部属性——每个都能分类（不 panic），
    /// 且**引用类型(att_type=ELEMENT)属性绝不落 Unknown、且都影响模型**（不漏级联）。
    #[test]
    fn att_meta_all_attributes_classify_and_references_affect_model() {
        let info = aios_core::get_default_pdms_db_info();
        let mut total = 0usize;
        let mut refs = 0usize;
        for noun in info.named_attr_info_map.iter() {
            for kv in noun.value().iter() {
                let a = kv.value();
                let name = a.name.as_str();
                if name.is_empty() {
                    continue;
                }
                let is_ref = attribute_is_reference(name);
                let eff = classify_attribute_effect_with_meta(name, is_ref);
                total += 1;
                if is_ref {
                    refs += 1;
                    assert_ne!(eff, AttributeEffect::Unknown, "引用属性 {name} 仍 Unknown");
                    assert!(eff.affects_model(), "引用属性 {name} 不影响模型: {eff:?}");
                }
            }
        }
        assert!(total > 100, "att_meta 属性过少: {total}");
        assert!(refs > 0, "未发现 ELEMENT 引用属性");
        println!("att_meta 覆盖：共 {total} 属性，引用类 {refs} 个，全部有判定且引用类均影响模型");
    }
}
