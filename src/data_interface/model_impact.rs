//! 属性变化 → 模型生成影响的判定（「宁多勿漏」保守机制）。
//!
//! 取代 `pdms_io::EleOperationDetail::is_geometry_update` 的「白名单 + 类型硬门」旧语义。
//! 旧语义只有「类型命中硬编码名单」且「改动属性命中极小几何属性集」才判为需重生成，
//! 命不中即当作不用重生成 —— 这是**漏判 = 模型陈旧**的正确性 bug。
//!
//! 新语义（对齐 `plant-model-gen`）：
//! - 未知属性 / UDA 一律**保守触发**（漏判=正确性 bug；误判=多算一次，成本可控）；
//! - **WNOEVT 事件门**（core `putAtt` 流程里 DCHC 之前的第一道门）：字典置 `wnoevt` 的属性被写时
//!   core 连变更事件都不发，故增量判决把它**无条件短路**为「不动模型」，不再问 DCHC / 读取集合两票
//!   （[`attribute_no_event`]，attlib.dat 离线解出，全 7 个是激光/照明/温度属性，均不在模型 schema）；
//! - 「不影响模型、可跳过」要两票：E3D 字典 DCHC = 0 **且** 生成器不读它
//!   （[`attribute_is_metadata_only`]，2026-09-15 S6；此前只放行 `NAME`/`DESC`/`PURP`/`FUNC`/`LCHKDA`）；
//! - noun/类型不再作为分类前置门，只用于最终生成器路由。
//!
//! 属性清单来源：AVEVA Everything3D `core.dll` / `Core3D.dll` 逆向（`DCHC/EVALAT`
//! 设计变化层为主，`wnoevt` 事件门作 DCHC 之前的短路）+ 运行库 `att_meta`(702) 交叉校验（100% 命中），
//! 详见 `plant-model-gen/docs/reverse/core_dll_noun_att_model_update.md` §13/§14。

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, OnceLock};

use aios_core::pdms_types::{DbAttributeType, TOTAL_LOOP_NOUN_NAMES, TOTAL_VERT_NOUN_NAMES};
use aios_core::{NamedAttrValue, RefU64Vec, RefnoEnum};
pub use e3d_model::increment::NoModelTier;
use serde::Deserialize;

use super::ele_ops::{
    ChildrenDelta, EleOperationData, EleOperationDetail, ModifiedElement, classify_children_delta,
};

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
    /// 纯业务元数据：NAME/DESC/PURP/FUNC，不影响几何/模型。
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
///
/// 名字用 E3D 的**属性短名**（`attlib` / 字典与运行库 schema 都只认这一份）。这里曾写着
/// `FUNCTION`：字典 4270 名与运行库 descriptor 表里都没有这个名字，它从落表那天起一次都
/// 没命中过，而真名 `FUNC`（633 个 noun 上有，字典 DCHC=0）反而落 `Unknown` 整根重算。
/// 2026-09-08 改回短名，并由 [`tests::curated_tables_are_reconciled_against_the_runtime_schema`]
/// 钉住「这张减免表里不许再有匹配不上任何东西的名字」。
///
/// 两个**戳**（`CACHID` / `SPAMAP`，e3d-model `PLACEMENT_STAMP_ATTRIBUTES`）也登记在这里：效果表判它们
/// DataOnly，但它们**不是老名单**（字典没收、[`attribute_is_curated_metadata`] 为假），放行靠戳表那一票
/// （[`attribute_is_cache_stamp`]）。`SPAMAP` 2026-09-19 与 `CACHID` 同档进表（计划 §8 Q6 题 B / t-334）。
pub const DATA_ONLY_ATTR_NAMES: &[&str] =
    &["NAME", "DESC", "PURP", "FUNC", "CACHID", "SPAMAP", "LCHKDA"];

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

/// 属性 → 效果的事实源之一：五张**手写**表，按效果分组声明，组间顺序不再携带语义。
///
/// 以前 `classify_attribute_effect` 用 if-else 链逐张表试，「一个属性归哪一类」由它写在链上
/// 的位置决定而不是由语义决定。现在合并成一张映射，重名由测试直接报错。第六张表
/// [`dictionary_change_class_attr_names`] 由字典**生成**，与这五张一起合进 [`attribute_effects`]。
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

/// 第六张效果表：E3D 字典里设计变化码 DCHC ∈ {1, 2, 4}、而上面五张手写表**一个都没登记**的
/// 全部属性名（2026-09-15：217 个 = DCHC=4 ×213 + DCHC=1/2 的 `HZAXI` `LHEA` `LTAI` `TZAXI`）。
///
/// 来源是 core 自己的口径：字典 DCHC 非 0 的属性，Core3D `change/EVALAT` 会为它排重画
/// （类别 4 → `change/UPGRPH` 走 `REPELE` / `SEGREP` 重建；类别 1 / 2 → 转成类别 4 落到前一元件 /
/// 自身），见 `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §3–§4。
/// 这些名字此前落 `Unknown`——判决方向本来就对（`Unknown` 也是 `Regen`），但回执与
/// `change_impact` 报表把 213 个 core 明说「会重画」的属性显示成「未知属性」。这张表只消
/// 那层噪音，**不改任何判决**：
///
/// - `dependency_cascade`：DCHC = 1 / 2（元件头 / 尾连通性，与 `HBOR HPOS TPOS …` 同族）以及
///   运行库 schema 判为 `ELEMENT` 引用的名字（`STAREF ENDREF JOIS JOIE CFRA PFRE …`）。后者此前
///   经 [`classify_attribute_effect_with_meta`] 的 A2 升级也落 `DependencyCascade`，登记在这一侧
///   是为了让不带 meta 的 [`classify_attribute_effect`] 给出同一个答案；
/// - `direct_geometry`：其余 DCHC = 4 的名字（尺寸 / 表示 / 可见性 / 楼梯栏杆桥架族 …）。
///
/// 由 `output/gen_dchc_fixture.py` 从字典导出 + 五张表 + 运行库 schema 生成为
/// `dictionary_change_class_attr_names.json`，随源码入库、编译期嵌入；手抄 217 个名字必漂。
/// 守卫：[`tests::dictionary_change_class_table_covers_every_unregistered_nonzero_attribute`]
/// 钉「与字典快照的差集为空」，[`tests::attribute_effect_tables_have_no_duplicate_names`]
/// 钉它与五张手写表不重名。五张手写表改了名单就重跑生成脚本。
#[derive(Debug, Deserialize)]
pub struct DictionaryChangeClassAttrNames {
    /// DCHC = 1 / 2，或 DCHC = 4 且 schema `att_type == ELEMENT` → [`AttributeEffect::DependencyCascade`]。
    pub dependency_cascade: Vec<String>,
    /// 其余 DCHC = 4 → [`AttributeEffect::DirectGeometry`]。
    pub direct_geometry: Vec<String>,
}

impl DictionaryChangeClassAttrNames {
    /// 两侧合起来的 `(名字, 效果)`，供合并进查找表与守卫测试逐名核对。
    pub fn entries(&self) -> impl Iterator<Item = (&str, AttributeEffect)> + '_ {
        self.dependency_cascade
            .iter()
            .map(|name| (name.as_str(), AttributeEffect::DependencyCascade))
            .chain(
                self.direct_geometry
                    .iter()
                    .map(|name| (name.as_str(), AttributeEffect::DirectGeometry)),
            )
    }
}

/// 嵌入的第六张表（见 [`DictionaryChangeClassAttrNames`]），懒解析一次。
pub fn dictionary_change_class_attr_names() -> &'static DictionaryChangeClassAttrNames {
    static TABLE: OnceLock<DictionaryChangeClassAttrNames> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("dictionary_change_class_attr_names.json"))
            .expect("dictionary_change_class_attr_names.json 随源码入库，必须是合法 JSON")
    })
}

/// 合并后的查找表，懒加载一次：五张手写表在前，第六张（字典生成）表只补空位——两边按构造
/// 不重名（生成脚本剔除了五张表已登记的名字，守卫测试再钉一次），这里的先后只是防御。
fn attribute_effects() -> &'static HashMap<&'static str, AttributeEffect> {
    static TABLE: OnceLock<HashMap<&'static str, AttributeEffect>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: HashMap<&'static str, AttributeEffect> = ATTRIBUTE_EFFECT_TABLES
            .iter()
            .flat_map(|(names, effect)| names.iter().map(move |name| (*name, *effect)))
            .collect();
        for (name, effect) in dictionary_change_class_attr_names().entries() {
            table.entry(name).or_insert(effect);
        }
        table
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

/// E3D 字典 `PLCF` 字段（652066）的快照：`change/IDCHNG`（Core3D `0x1022E302`）在读 DCHC 之后
/// **再**读它，`== 1` 才置 INTUBE 旗标 `dword_10E98500`，`UPDATD` 收尾时据此调 `PLCDEL` 让隐含
/// 管段的放置缓存失效（2026-09-15 取证文档 §3.2；0906 把这一下写成「DCHC == 1 时置」，已改正）。
/// 字典里 58 个属性为 1：位姿三个（`POS ORI BFORI`）、目录 / 规格引用、结构连接与楼梯栏杆桥架的
/// 尺寸参数，以及一个 DCHC = 0 的 `CLGREF`。
///
/// 只作报表 / 探针：我们没有对应的隐含管段放置缓存，它**不进任何判决**——
/// [`tests::plcf_is_report_only_and_matches_the_dictionary_snapshot`] 钉住本文件之外没有调用方。
/// 由 `output/gen_dchc_fixture.py` 与 DCHC 同批导出为 `plcf_flags.json`，随源码入库。
pub fn raw_plcf(raw_name: &str) -> Option<bool> {
    let name = normalize_attribute_name(raw_name);
    dchc_change_classes()
        .contains_key(&name)
        .then(|| plcf_flagged_names().contains(name.as_str()))
}

/// E3D 字典 `WNOEVT` 字段（299311034）：core `putAtt` 流程里 **DCHC 之前的第一道门**——
/// `DB_Attribute::wnoevt`（`core.dll 0x58D5290`）为真的属性被写时 `DB_ElementChangesPlugger`
/// 一个变更事件都不广播（`sub_55BB6C3` 读进 `+184`）。因此这类属性改动 core 侧没有任何下游重算，
/// 我们的增量判决也应把它当作「什么都不用动」，且这判定**在 DCHC / 读取集合两票之前**、无条件成立。
///
/// 名单由 `attlib.dat` 纯离线解出（`e3d_attlib::dict::AttrDict` / `output/gen_wnoevt_flags.py`，
/// 解 DCHC 4270/4270 · PLCF 58/58 逐条校准后同批导出 `wnoevt_flags.json` 随源码入库）。E3D 3.1 全字典
/// 只有 7 个属性置了它（`DPOSCA EVITCA EVITCP EXTEMP LIGTCA LIGTCP TEMPLI`），全是激光/照明/温度属性，
/// 没有一个在模型生成的 schema 或读取集合里——所以本门当前对模型语料是恒等变换（回执基线不变），
/// 只在这些「静默」属性万一出现时把它们从「未知→保守重算」纠正为「不动模型」。
/// [`tests::wnoevt_gate_is_disjoint_from_reads_and_geometry`] 钉住这份不变量。
pub fn attribute_no_event(raw_name: &str) -> bool {
    wnoevt_no_event_names().contains(&normalize_attribute_name(raw_name))
}

/// gen-model 侧的模型面读点模块（开发计划 2026-09-15 D2 / S5）：目录求值入口、房间、结构连接、电缆端点、
/// direct 窗口规划器与刚体前移。生成读取集合的第二半从这些文件里扫（第一半是 e3d-model 整个 `src`，
/// 由它自己的 `data/attribute-read-set.json` 交付）。与 `output/gen_read_set.py::GEN_MODEL_READ_MODULES`
/// 一字不差，守卫钉两边相等。
pub const GEN_MODEL_READ_MODULES: [&str; 11] = [
    "src/fast_model/cata_model.rs",
    "src/fast_model/room_model.rs",
    "src/fast_model/room_topology.rs",
    "src/fast_model/room_predicate.rs",
    "src/fast_model/room_resident.rs",
    "src/data_interface/room_read_through.rs",
    "src/api/room_code.rs",
    "src/data_interface/structural_connection.rs",
    "src/data_interface/cable_endpoint.rs",
    "src/data_interface/window_root_plan.rs",
    "src/data_interface/rigid_motion.rs",
];

/// e3d-io 目录表达式求值环境（`DbElementParamEnv`）——替生成器读属性的第三处，**只作文档、不进判决**
/// （见 [`generation_read_set`]）。与 `gen_read_set.py::E3D_IO_CATALOGUE_EVAL_FILES` 一字不差。
pub const E3D_IO_CATALOGUE_EVAL_FILES: [&str; 3] = [
    "src/db_param_env.rs",
    "src/record/catalogue_eval.rs",
    "src/record/catalogue_expr.rs",
];

/// `generation_read_set.json` 的形状（`output/gen_read_set.py` 生成）。
#[derive(Debug, Deserialize)]
pub struct GenerationReadSetFile {
    /// **戳**（2026-09-16 Q5）：是属性名，但源码提它只为**按名字把它排除掉**——差分层判「只动了摆放」
    /// 时要忽略的那张表（`e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES`：`CACHID`，2026-09-19 起还有 `SPAMAP`）。
    /// 写一句名字不是读，所以扫描把它们从**每一侧**的 `names` 里摘出来；收进去就等于让这条判据
    /// 自己写的排除名单反过来把它挡住。真被传进 e3d-io 的按名读取口就照旧算读、照旧回到 `names`，
    /// 守卫随即红。生成脚本的 `STAMP_LITERALS` 与那张表逐名相等（两仓守卫各钉一次）。
    pub stamp_literals: Vec<String>,
    /// e3d-model `data/attribute-read-set.json` 的 `names` 原样一份——守卫钉它与
    /// `e3d_model::attribute_read_set()` 逐名相等，两个仓的产物漂开就红。
    pub e3d_model: Vec<String>,
    /// gen-model 模型面模块的读点。
    pub gen_model: ReadSetSide,
    /// e3d-io 目录求值环境的读点，`vote == false`。
    pub e3d_io_catalogue_eval: ReadSetSide,
    /// **判决用的那一份**：`e3d_model ∪ gen_model.names`。
    pub names: Vec<String>,
}

/// 读取集合的一侧：扫了哪些文件、读到哪些属性名、还见到哪些不是属性也不是 noun 的大写字面量。
#[derive(Debug, Deserialize)]
pub struct ReadSetSide {
    /// 是否参与 [`generation_read_set`] 那一票（缺省参与）。
    #[serde(default = "default_true")]
    pub vote: bool,
    /// 相对各自 crate 根的文件路径。
    pub files: Vec<String>,
    /// 属性名（在字典 ∪ 运行库 schema 里、或直接传进 e3d-io 读取口的大写字面量），升序。
    pub names: Vec<String>,
    /// 既不是属性名也不是字典 noun 的大写字面量，升序（让守卫分清「新冒出的」与「早知道不是属性的」）。
    pub non_attribute_literals: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// 嵌入的 `generation_read_set.json`，懒解析一次。
pub fn generation_read_set_file() -> &'static GenerationReadSetFile {
    static FILE: OnceLock<GenerationReadSetFile> = OnceLock::new();
    FILE.get_or_init(|| {
        serde_json::from_str(include_str!("generation_read_set.json"))
            .expect("generation_read_set.json 随源码入库，必须是合法 JSON")
    })
}

/// **生成读取集合**（开发计划 2026-09-15 D2 / S5）：模型生成器会读到的属性名——e3d-model 整个 `src`
/// 出现的属性名字面量 ∪ gen-model 模型面模块（[`GEN_MODEL_READ_MODULES`]）出现的属性名字面量，
/// 按最宽口径扫（多收只少跳一次、漏收是一张停在旧形状上的网格）。
///
/// 它是元数据门的**第二票**：S6 起 [`attribute_is_metadata_only`] 改成「字典 DCHC == 0 ∧ 不在这里」；
/// 本轮（S5）只交付产物与护栏，判决还没换。2026-09-15：138 个名字（2026-09-19 重生成后 139——e3d-model 09-18
/// 风管那条带进的 `LBOR`），DCHC=0 的恰好六个
/// `DIR FILL GTYP PKEY UKEY VXREF`（core 不重画、我们却真读的输入，正是第二票要挡的那几个；2026-09-27
/// 宿主 FIXING 扫描读目录件的 `NGMR`，成了第七个），
/// 老五个元数据名 `NAME DESC PURP FUNC LCHKDA` 一个都不在。
///
/// e3d-io 目录求值环境读到的名字（`e3d_io_catalogue_eval`，`ANGL RADI HEIG DESP UNIPAR` 与目录侧
/// `DATA.PURP/DKEY/NUMB`、`SELE.QUES/ANSW/MAXA/TANS`）**不进这一票**：设计侧那几个 DCHC ≠ 0 或已在
/// e3d-model 一侧，DCHC = 0 的那七个全在目录元素上读，而元数据门只跑 DESI 窗口。
///
/// 戳（[`GenerationReadSetFile::stamp_literals`]，2026-09-16 Q5）也不在这里：`CACHID` 在运行库 schema 里，
/// 差分层为了排除它而写下的那个字面量会被这条最宽的扫描当成读点，那是错的——它不是几何输入。
/// 它不在这里正是元数据门放行它的**第二票**（守卫 `stamp_literals ∩ names = ∅`）；第一票字典给不了
/// （压根没收它），2026-09-17 Q6 起由戳表 [`attribute_is_cache_stamp`] 给——只 `CACHID` 变的候选记
/// `no_model` 第三档 [`NoModelTier::CacheStamp`]，不再上卷重算。
pub fn generation_read_set() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| generation_read_set_file().names.iter().cloned().collect())
}

/// 生成器是否读这个属性名（归一化后查 [`generation_read_set`]）。
pub fn generation_reads_attribute(raw_name: &str) -> bool {
    generation_read_set().contains(&normalize_attribute_name(raw_name))
}

/// `plcf_flags.json` 里 `PLCF == 1` 的属性名集合，懒解析一次。
fn plcf_flagged_names() -> &'static HashSet<String> {
    #[derive(Deserialize)]
    struct PlcfFlags {
        names: Vec<String>,
    }
    static TABLE: OnceLock<HashSet<String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let flags: PlcfFlags = serde_json::from_str(include_str!("plcf_flags.json"))
            .expect("plcf_flags.json 随源码入库，必须是合法 JSON");
        flags.names.into_iter().collect()
    })
}

/// `wnoevt_flags.json` 里 `WNOEVT == 1` 的属性名集合（事件门），懒解析一次。
/// 与 DCHC/PLCF 同批由 `output/gen_wnoevt_flags.py`（及 `e3d_attlib::dict`）从 `attlib.dat` 导出。
fn wnoevt_no_event_names() -> &'static HashSet<String> {
    #[derive(Deserialize)]
    struct WnoevtFlags {
        names: Vec<String>,
    }
    static TABLE: OnceLock<HashSet<String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let flags: WnoevtFlags = serde_json::from_str(include_str!("wnoevt_flags.json"))
            .expect("wnoevt_flags.json 随源码入库，必须是合法 JSON");
        flags.names.into_iter().collect()
    })
}

/// 「只改了这个属性，模型一个字都不用动」的**两票**判据（direct 窗口规划器的元数据门，
/// 2026-09-08 立、2026-09-15 S6 改口径）：E3D 字典的设计变化码说它是 0（core 自己也不为它重画，
/// `IDCHNG` 门），**并且**生成器不读它（[`generation_read_set`]：e3d-model 整个 `src` 与 gen-model
/// 模型面模块里一个读点都没有）。两票缺一不放行——字典是 core 的意见，读取集合是我们自己的；
/// 「少算」方向的判定只在两边都说「与几何无关」时放行。
///
/// S6 之前这里问的是「效果表 [`AttributeEffect::DataOnly`] ∧ DCHC = 0」，恰好放行五个名字；
/// 那张手写表退成报表分类与 `no_model` 的分档依据（[`attribute_is_curated_metadata`]），不再是判决输入。
/// 字典 3941 个 DCHC = 0 的名字里，读取集合只挡 `DIR FILL GTYP NGMR PKEY UKEY VXREF` 七个（core 不重画、
/// 我们却真拿来生成的输入），其余 3934 个都放行。其中 15 个被手写表登记成几何 / 级联输入
/// （`HREF TREF PSPE DKEY DDPR FSPREF SCREF STYP CURD JFRE PTYP` 与 `SIZE MAXA ORRF PTOF`）：
/// 它们既不是 core 的重画输入、生成器也不读——手写表那一格是当年按语义猜的，判决以两票为准；
/// [`tests::the_two_votes_override_the_hand_tables_for_exactly_fifteen_names`] 逐个点名钉住，名单变了就得重新问
/// 「新名字真的没人读吗」。
///
/// **戳是第一票的唯一例外**（2026-09-17 Q6，用户拍板）：`CACHID` 不在字典快照里（`raw_dchc_code` 回
/// `None`）——它是 3.1 运行库 schema 里的项目 UDA，字典按构造就不会收它，这一票不是「还没补上」而是
/// **永远不会有**。它的第一票由戳表 [`attribute_is_cache_stamp`]（e3d-model
/// `element_diff::PLACEMENT_STAMP_ATTRIBUTES`，两仓守卫钉住逐名相等）给：写方语义已坐实——Design 应用随每次
/// 图形重建写下的缓存 `+1` 计数（S8 普查 453 库 2961 窗 13724 次跳动全是 `+1`），core 自己的图形变化管理
/// 不看它（0831 取证 `changesBetweenSessions` 的 `DDES_GraphicsMngr` 一路），S4 对拍把它剔出比较集后 108 根
/// 两态全等 0 红。第二票照旧问读取集合（守卫 `stamp_literals ∩ names = ∅`）。`UDA:<id>` 与任何别的字典
/// 没有的名字仍不放行：例外只认那张表。
///
/// **2026-09-19 `SPAMAP` 同档进表**（用户拍板，计划 §8 Q6 题 B / t-334）：同是 3.1 运行库 schema 里的簿记键
/// （`INTVEC`，字典无票），Design 应用随图形 / 空间索引重建逐次 `+1`，生成器一处不读（同一对守卫钉住），
/// 四库全量两态 1718 根 0 红（归 `SPAMAP` 的 1034 根）、全语料 453 库命中 36 库、四门与 `CACHID` 同向无一格反向
/// （`docs/evidence/2026-09-16-q6-spamap-denominator-audit.md` §4.1）。名单仍只有 e3d-model 那一份，这里不点名。
pub fn attribute_is_metadata_only(raw_name: &str) -> bool {
    // WNOEVT 事件门（DCHC 之前的第一道门）：core 对置位属性不发任何变更事件，
    // 直接短路为「不动模型」，不再问 DCHC / 读取集合两票（见 [`attribute_no_event`]）。
    if attribute_no_event(raw_name) {
        return true;
    }
    (raw_dchc_code(raw_name) == Some(0) || attribute_is_cache_stamp(raw_name))
        && !generation_reads_attribute(raw_name)
}

/// 这个名字是不是**戳**（e3d-model `element_diff::PLACEMENT_STAMP_ATTRIBUTES`：`CACHID`，2026-09-19 起还有 `SPAMAP`）：
/// 应用随图形 / 空间索引重建写的簿记计数，不是设计变更。归一化后按名查；名单只有 e3d-model 那一份，
/// 读取集合产物的 `stamp_literals` 与它逐名相等（守卫钉两边）。
pub fn attribute_is_cache_stamp(raw_name: &str) -> bool {
    let name = normalize_attribute_name(raw_name);
    e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES
        .iter()
        .any(|stamp| *stamp == name)
}

/// 老名单（2026-09-08 至 S6 之前的元数据门）：效果表 [`AttributeEffect::DataOnly`] **且** 字典 DCHC = 0，
/// 恰好 `NAME DESC PURP FUNC LCHKDA`。S6 起它不再决定放不放行（那是 [`attribute_is_metadata_only`]），
/// 只用来把门省下的候选分成两档：全部落在这里的记 `metadata_only`，否则记 `unread_attribute`
/// （e3d-model [`NoModelTier`]），让「门变宽以后多省了多少」在回执上读得出来。
/// 按构造是 [`attribute_is_metadata_only`] 的子集（守卫钉）：这五个名字生成器一个都不读。
pub fn attribute_is_curated_metadata(raw_name: &str) -> bool {
    classify_attribute_effect(raw_name) == AttributeEffect::DataOnly
        && raw_dchc_code(raw_name) == Some(0)
}

/// 一次改动的全部属性名过元数据门的结论：`None` = 至少一个属性没拿到两票（或一个属性都没有），
/// 照常上卷；`Some(档)` = 全部放行，档位按 [`attribute_is_curated_metadata`] 分——全在老名单里是
/// [`NoModelTier::MetadataOnly`]，否则 [`NoModelTier::UnreadAttribute`]。戳（[`attribute_is_cache_stamp`]）
/// 放行但**不改档**——跟着 `NAME` 记老档、跟着 `DUTY` 记新档；一次改动里**只有**戳跳了（2026-09-17 Q6）
/// 才记第三档 [`NoModelTier::CacheStamp`]。
///
/// 这是 `window_root_plan::change_metadata_tier` 与 [`classify_operation_effects`] 共问的那一份名单
/// （0908 纪律：预览与执行同一谓词）；结构条件（类型 / 属主 / 成员 / opaque）在调用方判。
pub fn metadata_only_tier<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<NoModelTier> {
    let mut tier: Option<NoModelTier> = None;
    let mut any = false;
    for name in names {
        if !attribute_is_metadata_only(name) {
            return None;
        }
        any = true;
        if attribute_is_cache_stamp(name) {
            continue;
        }
        tier = Some(match tier {
            Some(NoModelTier::UnreadAttribute) => NoModelTier::UnreadAttribute,
            _ if attribute_is_curated_metadata(name) => NoModelTier::MetadataOnly,
            _ => NoModelTier::UnreadAttribute,
        });
    }
    any.then(|| tier.unwrap_or(NoModelTier::CacheStamp))
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
/// - `Modified`：逐属性先问元数据门 [`attribute_is_metadata_only`]（字典 DCHC=0 ∧ 生成器不读）——
///   拿到两票的记 `DataOnly`、不排工作，哪怕手写表把它登记在别处；没拿到的按效果表归类：
///   `DataOnly`（只有效果表一票的——2026-09-17 Q6 起 `CACHID` 由戳表补上第一票，不再落这里）保守重生成；`TransformOnly` 记位姿；其余
///   （DirectGeometry/DependencyCascade/StructuralMembership/Unknown，含 `UDA:*`）记重生成；
///   `children` 变化、或检测不到任何属性差异（如非 detail 解析）时保守记重生成；
/// - `None` → 无动作。
///
/// 两票是谓词自己的判据，不受任何独立开关左右（S6 定稿，2026-09-17：第二档那个
/// `model_skip_unread_attribute_changes` 已退役）——预览与窗口规划器因此天然同口径，
/// 不会出现「预览说 Skip、规划器却整根重算」那种同一份回执里两句话。
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
                } else if attribute_is_metadata_only(name) {
                    // 「不影响模型」是**少算**方向，判据与 direct 窗口规划器的元数据门
                    // （`window_root_plan::change_metadata_tier`）共用同一道两票（字典 DCHC=0 ∧
                    // 生成器不读）：门放行的名字这里也不排工作，哪怕手写表把它登记成几何 /
                    // 级联输入——同一份回执里的 `details` 与 `units` 必须是一句话。
                    AttributeEffect::DataOnly
                } else {
                    // A2：未命中静态清单的引用属性(att_type=ELEMENT)→DependencyCascade（进级联反查）。
                    classify_attribute_effect_with_meta(name, attribute_is_reference(name))
                };
                if let Some(code) = raw_dchc_code(name) {
                    dchc = Some(dchc.map_or(code, |c| c.max(code)));
                }
                match effect {
                    // 效果表说是元数据、两票却没拿全的，规划器照旧上卷整根重算，这里就不能报
                    // 「不影响模型」。（`CACHID` 曾在这里——字典没投票；2026-09-17 Q6 起戳表给它第一票。）
                    AttributeEffect::DataOnly if !attribute_is_metadata_only(name) => {
                        summary.impact_regen = true;
                    }
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
///
/// 预览那条薄壳（`manual_update::merge_net_change_details`）与窗口规划器的元数据门走的都是
/// 这一条——两票没有自己的开关，同一个判据不会分出两句话。
pub fn classify_operation_impact(op: &EleOperationData) -> OperationImpact {
    classify_operation_effects(op).impact()
}

/// Core3D R13 / T2c 第一批：这个已知 noun 的变化只会制造模型队列噪音。
///
/// 第一批严格等于 P1 证据里的 1272 项：core 判为既非 significant 也非两位
/// primitive，dabacon 字典也没有 geomset、extrusion 或非零 graphics behaviour，且
/// 它原本可能成为我方普通生成根。loop/point 容器、粗层级和 FTUB 本来就不是普通根，
/// 不借这道门改变它们的 owner 上卷语义。未知 noun 保守返回 false。
pub fn noun_is_no_geometry_model_entry(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    let Some(caps) = crate::noun_dict::default_noun_capabilities().get(&noun) else {
        return false;
    };
    if is_loop_container_noun(&noun)
        || matches!(noun.as_str(), "WORL" | "WORLD" | "SITE" | "ZONE" | "FTUB")
    {
        return false;
    }
    !crate::data_interface::generation_root::noun_is_significant(&noun)
        && !crate::data_interface::generation_root::noun_is_primitive(&noun)
        && !caps.geomset
        && !caps.extrusion
        && caps.graphics_behaviour.unwrap_or(0) == 0
}

/// 是否为 loop/坐标辅助类型（LOOP/PLOO/VERT/PAVE、*DATU 等）：这些自身不是
/// 几何生成根，需上溯到非容器 owner（如 PANE/EXTR/WALL/GENSEC）再重生成。
pub fn is_loop_container_noun(noun: &str) -> bool {
    let n = noun.trim().to_ascii_uppercase();
    matches!(n.as_str(), "JLDATU" | "PLDATU" | "ENDATU")
        || crate::noun_dict::default_noun_capabilities()
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

    #[test]
    fn the_first_no_geometry_entry_batch_is_exactly_the_evidenced_1272_nouns() {
        let mut nouns = crate::noun_dict::default_noun_capabilities()
            .iter()
            .filter(|caps| noun_is_no_geometry_model_entry(&caps.noun_name))
            .map(|caps| caps.noun_name.as_str())
            .collect::<Vec<_>>();
        nouns.sort_unstable();
        assert_eq!(nouns.len(), 1272);
        for expected in ["DB", "PIPE"] {
            assert!(
                nouns.contains(&expected),
                "{expected} 是已知无几何普通 noun，应进入第一批"
            );
        }
        for protected in ["ZONE", "PAVE", "FTUB", "TUBE", "EQUI", "BOX"] {
            assert!(
                !nouns.contains(&protected),
                "{protected} 有根语义、几何代理、significant 或 primitive，必须保留"
            );
        }
        assert!(!noun_is_no_geometry_model_entry("UNKNOWN_FUTURE_NOUN"));
    }

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
        for attr_name in ["SPRE", "SPREF", "PSPREF"] {
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
        // `FSPREF` 手写表登记在级联侧，但字典 DCHC=0 且生成器不读：S6 起两票放行，预览与执行同口径。
        assert_eq!(
            classify_attribute_effect("FSPREF"),
            AttributeEffect::DependencyCascade
        );
        assert!(attribute_is_metadata_only("FSPREF"));
        assert_eq!(
            classify_operation_impact(&modified_operation("FSPREF")),
            OperationImpact::Skip
        );

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
            "NAME", "DESC", "PURP", "FUNC", "CACHID", "LCHKDA", "att.NAME", "att.DESC",
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
            // 动作不再只由效果表决定：`DataOnly` 还得拿到字典那第二票才落 `Skip`，
            // 见 `the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate`。
            let expected = if attribute_is_metadata_only(name) {
                OperationImpact::Skip
            } else {
                OperationImpact::Regen
            };
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                expected,
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
            // S6：字典 DCHC=0 且生成器不读的名字（`HREF TREF PSPE …`）两票放行，手写表那一格只剩报表意义，
            // 见 `the_two_votes_override_the_hand_tables_for_exactly_fifteen_names`。
            let expected = if attribute_is_metadata_only(name) {
                OperationImpact::Skip
            } else {
                OperationImpact::Regen
            };
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                expected,
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

    /// U-H2：通风构件的属性变化分类，背书 H-01（DESP）与 H-02（POS）两条 TTY 宏。
    ///
    /// - `DESP`（DAMP/STRT 的设计参数，`DESPAR NUMn` 落在 `DESP` 上）→ `DependencyCascade` → Regen；
    /// - HVAC 构件（都不是点容器）的 `POS`/`ORI` → `TransformOnly`（只更新 world transform）；
    /// - `NAME` → Skip（DataOnly）。
    #[test]
    fn hvac_component_attribute_changes_follow_the_update_contract() {
        assert_eq!(
            classify_attribute_effect("DESP"),
            AttributeEffect::DependencyCascade,
            "DESP 是设计参数依赖，改它要重生成"
        );
        for noun in ["DAMP", "STRT", "TRNS", "BATT", "BEND"] {
            assert!(
                !is_loop_container_noun(noun),
                "{noun} 不是点容器，POS 应走便宜的 transform 路径"
            );

            let mut desp = modified_operation("DESP");
            set_operation_noun(&mut desp, noun);
            assert_eq!(
                classify_operation_impact(&desp),
                OperationImpact::Regen,
                "{noun} DESP 改动必须重生成"
            );

            for attr in ["POS", "ORI"] {
                let mut moved = modified_operation(attr);
                set_operation_noun(&mut moved, noun);
                assert_eq!(
                    classify_operation_impact(&moved),
                    OperationImpact::TransformOnly,
                    "{noun} {attr} 只需更新 world transform"
                );
            }

            let mut renamed = modified_operation("NAME");
            set_operation_noun(&mut renamed, noun);
            assert_eq!(
                classify_operation_impact(&renamed),
                OperationImpact::Skip,
                "{noun} 改名是 DataOnly"
            );
        }
    }

    /// U-T3：电缆桥架路由/材料的变化分类。`POINTR`/`RPATH` 是点容器（noun_caps `point=true`），
    /// 它们的 `POS` 是属主几何的输入，必须 Regen（不是 TransformOnly、更不是 Skip）——对应 T-02
    /// POINTR 位移；`CTMTRL` 成员表变化（NEW/DELETE CTSTRA，T-03/T-04）也必须 Regen。
    #[test]
    fn cable_tray_route_and_material_changes_regenerate() {
        for noun in ["POINTR", "RPATH"] {
            assert!(
                is_loop_container_noun(noun),
                "{noun} 应是点容器（其 POS 是属主几何输入）"
            );
            let mut moved = modified_operation("POS");
            set_operation_noun(&mut moved, noun);
            assert_eq!(
                classify_operation_impact(&moved),
                OperationImpact::Regen,
                "{noun} POS 必须重生成属主几何"
            );
        }

        use aios_core::pdms_types::{RefU64, RefU64Vec};
        let mut members = modified_operation_with_attrs(&[]);
        set_operation_noun(&mut members, "CTMTRL");
        if let EleOperationDetail::Modified(modified) = &mut members.detail {
            modified.children_changed = Some((
                RefU64Vec(vec![RefU64(1), RefU64(2)]),
                RefU64Vec(vec![RefU64(1), RefU64(2), RefU64(3)]),
            ));
        }
        assert_eq!(
            classify_operation_impact(&members),
            OperationImpact::Regen,
            "CTMTRL 成员表增减（增删 CTSTRA）必须重生成"
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
        use crate::data_interface::ele_data::EleData;
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
        let classifier = crate::noun_dict::default_noun_classifier();
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
        let capabilities = crate::noun_dict::default_noun_capabilities();
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

    /// 字典里 44 个 point 容器全部按 loop 容器上卷、自身不是生成根——**唯一例外**是 `AIDTEX`：
    /// core 位表判它显著，`model_significant_over_point` 开着时它自己成根（T2b，
    /// `generation_root::tests::the_significant_over_point_override_only_moves_aidtex` 钉「恰好一个」）。
    /// 这里钉两件事：分类器仍把 44 个都判成 point 容器（上卷判据没变），且 44 个里 core 判显著的
    /// 恰好只有 AIDTEX——多出一个，例外名单就得跟着改。
    #[test]
    fn every_non_significant_point_container_is_skipped_as_a_generation_root() {
        let point_nouns = crate::noun_dict::default_noun_capabilities().point_nouns();
        assert_eq!(point_nouns.len(), 44, "dabacon point capability drifted");
        let mut core_significant = Vec::new();
        for noun in &point_nouns {
            assert!(
                is_loop_container_noun(noun),
                "{noun}: point container must roll up to its real geometry owner"
            );
            if crate::data_interface::generation_root::core_significant_bit(noun) == Some(true) {
                core_significant.push(noun.clone());
            }
        }
        assert_eq!(
            core_significant,
            vec!["AIDTEX".to_string()],
            "AIDTEX is the only point container core calls significant"
        );
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
        // 两张「减免」表一个死名都不许有：减免表上的名字匹配不到任何真属性时，它不只是
        // 白写，而是**把真属性挡在减免之外还看不出来**——`FUNCTION` 就这么躺了很久（字典与
        // descriptor 表都没有这个名字，真名是 `FUNC`），旧断言还把它当既成事实钉住。
        assert!(
            unmatched(DATA_ONLY_ATTR_NAMES).is_empty(),
            "判为纯元数据的属性必须真实存在，否则真名那条反而落 Unknown 整根重算：{}",
            unmatched(DATA_ONLY_ATTR_NAMES)
        );
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
        // 第六张（字典生成）表也算：生成脚本按构造剔除了五张表已登记的名字，五张表事后
        // 加了名却没重跑脚本，就会在这里重名。
        for (name, effect) in dictionary_change_class_attr_names().entries() {
            if let Some(previous) = seen.insert(name, effect) {
                duplicates.push(format!("{name}({previous:?} vs {effect:?} [字典表])"));
            }
        }
        assert!(
            duplicates.is_empty(),
            "同一属性出现在多张效果表里：{}",
            duplicates.join(", ")
        );
    }

    /// 第六张表守卫（开发计划 2026-09-15 S1 / D3）：它必须**恰好**等于「字典快照里 DCHC ∈ {1,2,4}
    /// 且五张手写表都没登记」的名字集合——两个方向的差集都为空。快照换了、五张表改了名单而
    /// 没重跑 `output/gen_dchc_fixture.py`，都会在这里红。
    #[test]
    fn dictionary_change_class_table_covers_every_unregistered_nonzero_attribute() {
        let curated: BTreeSet<&str> = ATTRIBUTE_EFFECT_TABLES
            .iter()
            .flat_map(|(names, _)| names.iter().copied())
            .collect();
        let expected: BTreeSet<&str> = dchc_change_classes()
            .iter()
            .filter(|(name, code)| matches!(code, 1 | 2 | 4) && !curated.contains(name.as_str()))
            .map(|(name, _)| name.as_str())
            .collect();

        let table = dictionary_change_class_attr_names();
        let actual: BTreeSet<&str> = table.entries().map(|(name, _)| name).collect();
        let missing: Vec<&str> = expected.difference(&actual).copied().collect();
        let stale: Vec<&str> = actual.difference(&expected).copied().collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "字典表与快照的差集不为空，重跑 output/gen_dchc_fixture.py：快照有、表没有 {missing:?}；表有、快照没有 {stale:?}"
        );
        assert_eq!(
            actual.len(),
            table.dependency_cascade.len() + table.direct_geometry.len(),
            "两侧之间有重名"
        );
        // 2026-09-15 的字典快照：213 个 DCHC=4 + 4 个 DCHC=1/2。数字漂了就是快照或五张表换了。
        assert_eq!(actual.len(), 217, "第六张表条数漂移");

        // 生成产物是规范形：两侧各自有序、无重复，diff 才可读。
        for side in [&table.dependency_cascade, &table.direct_geometry] {
            assert!(
                side.windows(2).all(|pair| pair[0] < pair[1]),
                "第六张表两侧必须严格升序：{side:?}"
            );
        }

        // 分侧规则：DCHC = 1 / 2 一律在级联侧（与 HBOR/HPOS/TPOS 同族），头尾连通的四个显式点名。
        for name in ["HZAXI", "LHEA", "LTAI", "TZAXI"] {
            assert!(
                table.dependency_cascade.iter().any(|n| n == name),
                "{name} 是 DCHC=1/2 的头尾连通属性，必须在 dependency_cascade 侧"
            );
        }
        for name in &table.direct_geometry {
            assert_eq!(raw_dchc_code(name), Some(4), "{name} 在几何侧却不是 DCHC=4");
        }
        for name in &table.dependency_cascade {
            assert!(
                matches!(raw_dchc_code(name), Some(1 | 2 | 4)),
                "{name} 的 DCHC 不在 {{1,2,4}} 里"
            );
        }
    }

    /// 第六张表逐名落到声明的效果，且动作与此前的 `Unknown` 一样是 `Regen`——这张表只消噪音。
    #[test]
    fn dictionary_change_class_table_maps_to_its_declared_effects_and_actions() {
        for (name, effect) in dictionary_change_class_attr_names().entries() {
            assert_eq!(classify_attribute_effect(name), effect, "{name}");
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Regen,
                "{name}"
            );
            // 元数据门对它们一律不放行（DCHC ≠ 0 本来就拿不到字典那一票）。
            assert!(!attribute_is_metadata_only(name), "{name}");
        }
    }

    /// 分侧与运行库 schema 一致：`ELEMENT` 引用一个不落地进级联侧、几何侧一个引用都没有。于是
    /// 不带 meta 的 [`classify_attribute_effect`] 与带 meta 的 [`classify_attribute_effect_with_meta`]
    /// 对这 217 个名字给出**同一个**答案——此前引用名靠 A2 升级才落 `DependencyCascade`，现在
    /// 登记即得；`reference_edge_eligible` 那一侧的建边资格因此逐名不变。
    #[test]
    fn dictionary_change_class_table_routes_schema_references_to_the_cascade_side() {
        if runtime_attribute_names().is_empty() {
            return; // 无 schema 的环境软跳过
        }
        let table = dictionary_change_class_attr_names();
        for name in &table.direct_geometry {
            assert!(
                !attribute_is_reference(name),
                "{name} 是 schema ELEMENT 引用，却登记在几何侧——重跑生成脚本"
            );
        }
        for name in &table.dependency_cascade {
            let head_or_tail = matches!(raw_dchc_code(name), Some(1 | 2));
            assert!(
                head_or_tail || attribute_is_reference(name),
                "{name} 既不是 DCHC=1/2 也不是 ELEMENT 引用，不该在级联侧"
            );
        }
        for (name, effect) in table.entries() {
            assert_eq!(
                classify_attribute_effect_with_meta(name, attribute_is_reference(name)),
                effect,
                "{name}: 带 / 不带 meta 的分类必须同答"
            );
        }
        // 18 个 DCHC=4 的引用名 + 4 个头尾连通名。
        assert_eq!(table.dependency_cascade.len(), 22);
        for name in ["STAREF", "ENDREF", "JOIS", "JOIE", "CFRA", "PFRE"] {
            assert!(
                table.dependency_cascade.iter().any(|n| n == name),
                "{name} 是 core EVALAT 规则里点过名的引用属性，应在级联侧"
            );
        }
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
            // S6：`SIZE MAXA ORRF PTOF` 字典 DCHC=0 且生成器不读，两票放行；其余照旧重算。
            let expected = if attribute_is_metadata_only(name) {
                OperationImpact::Skip
            } else {
                OperationImpact::Regen
            };
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                expected,
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

    // ── 生成读取集合（S5）：与 output/gen_read_set.py 逐字同一条扫描规则 ─────────────────

    /// 截到第一个 cfg(test) 标记（测试模块都在文件尾）。
    fn strip_test_module(source: &str) -> &str {
        match source.find("#[cfg(test)]") {
            Some(cut) => &source[..cut],
            None => source,
        }
    }

    /// 正则 `"([A-Z][A-Z0-9]{1,7})"` 的手写同款：引号、2–8 个大写字母数字（首字母）、引号，不看字串语境。
    fn uppercase_literals(source: &str, out: &mut BTreeSet<String>) {
        let bytes = source.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && end - start < 8 {
                let c = bytes[end];
                let ok = if end == start {
                    c.is_ascii_uppercase()
                } else {
                    c.is_ascii_uppercase() || c.is_ascii_digit()
                };
                if !ok {
                    break;
                }
                end += 1;
            }
            if end - start >= 2 && end < bytes.len() && bytes[end] == b'"' {
                out.insert(source[start..end].to_string());
                i = end + 1;
            } else {
                i += 1;
            }
        }
    }

    /// 直接传进 e3d-io 读取口的字面量（`.get_double("X")` / `.attribute("X")` …）。
    fn direct_reader_literals(source: &str, out: &mut BTreeSet<String>) {
        const READERS: &[&str] = &[
            ".get_string(\"",
            ".get_integer(\"",
            ".get_double(\"",
            ".get_bool(\"",
            ".get_ref(\"",
            ".get_integer_array(\"",
            ".get_double_array(\"",
            ".get_ref_array(\"",
            ".get_position(\"",
            ".get_direction(\"",
            ".get_orientation(\"",
            ".get_element(\"",
            ".get_element_array(\"",
            ".attribute(\"",
            ".has_attribute(\"",
        ];
        for reader in READERS {
            let mut from = 0;
            while let Some(at) = source[from..].find(reader) {
                let open = from + at + reader.len();
                let close = source[open..]
                    .find('"')
                    .map(|o| open + o)
                    .expect("closing quote");
                out.insert(source[open..close].to_string());
                from = close;
            }
        }
    }

    /// 扫一组文件：`(全部大写字面量, 直接读点字面量)`。
    fn scan_read_literals(
        root: &std::path::Path,
        files: &[String],
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut literals = BTreeSet::new();
        let mut direct = BTreeSet::new();
        for file in files {
            let path = root.join(file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
            let source = strip_test_module(&source);
            uppercase_literals(source, &mut literals);
            direct_reader_literals(source, &mut direct);
        }
        (literals, direct)
    }

    /// 与生成脚本 `classify` 同款：属性名 = 在全集里或直接被读，戳除外；非属性 = 其余非 noun 非戳。
    ///
    /// 戳只免掉「提了一句名字」，免不掉真读：直接读点无条件算属性名，所以一个戳要是被传进
    /// e3d-io 的按名读取口，它照旧落进 `names`，守卫那条「戳不许出现在 names」立刻红。
    fn classify_read_literals(
        literals: &BTreeSet<String>,
        direct: &BTreeSet<String>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let universe: BTreeSet<String> = dchc_change_classes()
            .keys()
            .cloned()
            .chain(runtime_attribute_names())
            .collect();
        let nouns: BTreeSet<&str> = e3d_model::category::dictionary_noun_names().collect();
        let stamps: BTreeSet<&str> = e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES
            .into_iter()
            .collect();
        let names = literals
            .iter()
            .filter(|l| {
                direct.contains(*l) || (universe.contains(*l) && !stamps.contains(l.as_str()))
            })
            .cloned()
            .collect();
        let other = literals
            .iter()
            .filter(|l| {
                !universe.contains(*l)
                    && !direct.contains(*l)
                    && !nouns.contains(l.as_str())
                    && !stamps.contains(l.as_str())
            })
            .cloned()
            .collect();
        (names, other)
    }

    fn sorted_set(list: &[String]) -> BTreeSet<String> {
        let set: BTreeSet<String> = list.iter().cloned().collect();
        assert_eq!(set.len(), list.len(), "名单里有重复：{list:?}");
        assert!(
            list.windows(2).all(|pair| pair[0] < pair[1]),
            "名单必须严格升序：{list:?}"
        );
        set
    }

    /// S5 护栏（gen-model 半边）：产物的三半与源码逐名相等——`e3d_model` 一半等于 e3d-model 自己嵌入的
    /// 读取集合（两个仓的产物漂开就红）；`gen_model` 一半等于按同一条规则重扫 [`GEN_MODEL_READ_MODULES`]
    /// 的结果；判决用的 `names` 恰好是两者之并。任何一处红都只有一个动作：重跑 `output/gen_read_set.py`。
    #[test]
    fn the_generation_read_set_mirrors_e3d_model_and_the_model_side_modules() {
        if runtime_attribute_names().is_empty() {
            return; // 无 schema 的环境软跳过
        }
        let file = generation_read_set_file();
        let e3d_model_side = sorted_set(&file.e3d_model);
        let upstream: BTreeSet<String> = e3d_model::attribute_read_set()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(
            e3d_model_side, upstream,
            "generation_read_set.json 的 e3d_model 一半与 e3d-model data/attribute-read-set.json 漂开了，重跑 output/gen_read_set.py"
        );

        // 戳（2026-09-16 Q5）：三处名单——生成脚本的 STAMP_LITERALS（产物里这一格）、e3d-model 差分层
        // 那张表、e3d-model 自己那份产物——必须是同一份，漂开就红。
        assert_eq!(
            file.stamp_literals,
            e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES,
            "stamp_literals 与 element_diff::PLACEMENT_STAMP_ATTRIBUTES 漂开了，重跑 output/gen_read_set.py"
        );
        assert_eq!(
            file.stamp_literals,
            e3d_model::attribute_reads::attribute_read_set_file().stamp_literals,
            "两仓产物的 stamp_literals 漂开了，重跑 output/gen_read_set.py"
        );

        assert!(file.gen_model.vote);
        assert_eq!(
            file.gen_model.files,
            GEN_MODEL_READ_MODULES
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>(),
            "gen_model.files 与 GEN_MODEL_READ_MODULES 不一致"
        );
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let (literals, direct) = scan_read_literals(root, &file.gen_model.files);
        let (names, other) = classify_read_literals(&literals, &direct);
        assert_eq!(
            sorted_set(&file.gen_model.names),
            names,
            "gen_model.names 与源码重扫不一致，重跑 output/gen_read_set.py"
        );
        assert_eq!(
            sorted_set(&file.gen_model.non_attribute_literals),
            other,
            "gen_model.non_attribute_literals 与源码重扫不一致，重跑 output/gen_read_set.py"
        );
        // 2026-09-15：目录参数 + 结构连接五个 + 电缆端点两个。
        assert_eq!(
            file.gen_model.names,
            [
                "CFRA", "CREF", "ENDREF", "JOIE", "JOIS", "PARA", "PFRE", "STAREF"
            ]
        );

        let union: BTreeSet<String> = e3d_model_side.union(&names).cloned().collect();
        assert_eq!(
            sorted_set(&file.names),
            union,
            "names 必须恰好是 e3d_model ∪ gen_model.names"
        );
        assert_eq!(generation_read_set().len(), file.names.len());
        assert!(generation_reads_attribute("pos") && generation_reads_attribute(" JOIS "));

        // 判决用的那一份里一个戳都不许有：它是差分层的排除名单，不是读点。
        for stamp in &file.stamp_literals {
            assert!(
                !generation_reads_attribute(stamp),
                "{stamp} 是戳却进了第二票——多半是新加了一处按名读取口，先弄清那处读是不是几何输入"
            );
        }
    }

    /// S5 护栏（e3d-io 半边）：目录求值环境读到的名字如实记录、按同一条规则可重扫，但 `vote == false`——
    /// 它 DCHC = 0 的七个名字全在目录 `DATA` / `SELE` 行上读（`PSPE` 只是 `ATTRIB … OF PSPE OF PIPE`
    /// 的属主链 token），设计侧的 `ANGL RADI HEIG DESP` 已在 e3d-model 一侧、`UNIPAR` 不在字典。
    /// 名单变了就得重新回答「新名字是在设计元素上读的吗」，答是就该改 `vote`。
    #[test]
    fn the_e3d_io_catalogue_evaluation_reads_are_documented_but_not_a_vote() {
        if runtime_attribute_names().is_empty() {
            return; // 无 schema 的环境软跳过
        }
        let file = generation_read_set_file();
        let side = &file.e3d_io_catalogue_eval;
        assert!(
            !side.vote,
            "e3d-io 目录求值的读点不进第二票（见 generation_read_set 文档）"
        );
        assert_eq!(
            side.files,
            E3D_IO_CATALOGUE_EVAL_FILES
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
        );
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/shared/e3d-io");
        assert!(
            root.join("Cargo.toml").is_file(),
            "e3d-io 应在 {}（Cargo.toml 的 path 依赖）",
            root.display()
        );
        let (literals, direct) = scan_read_literals(&root, &side.files);
        let (names, other) = classify_read_literals(&literals, &direct);
        assert_eq!(
            sorted_set(&side.names),
            names,
            "重跑 output/gen_read_set.py"
        );
        assert_eq!(
            sorted_set(&side.non_attribute_literals),
            other,
            "重跑 output/gen_read_set.py"
        );
        let mut zero: Vec<&str> = side
            .names
            .iter()
            .filter(|n| raw_dchc_code(n) == Some(0))
            .map(String::as_str)
            .collect();
        zero.sort_unstable();
        assert_eq!(
            zero,
            [
                "ANSW", "CMPR", "DKEY", "MAXA", "PSPE", "PTYP", "PURP", "QUES", "TANS"
            ],
            "e3d-io 目录求值读到的 DCHC=0 名字变了：核一遍新名字是不是在设计元素上读的"
        );
        // 它单独读到、而两票之外的名字里，没有一个是设计侧 DCHC=0 的：设计侧那几个要么 DCHC≠0 要么已在 e3d_model。
        let vote = generation_read_set();
        for name in &side.names {
            if !vote.contains(name) {
                assert!(
                    raw_dchc_code(name) != Some(0) || zero.contains(&name.as_str()),
                    "{name}"
                );
            }
        }
    }

    /// S5 验收：集合里 DCHC=0 的成员逐个点名（今天十个，全来自 e3d-model：`DIR` catalogue_point、
    /// `FILL`/`PKEY` section、`GTYP` route、`UKEY` udet、`VXREF` polyhedron、`NGMR` catalogue 的宿主扣减、
    /// 电缆在途段的 `CMPR`/`OUTD`/`MINBEN`——core 会话内不为它们重画，我们却真拿它们生成，S6 的第二票
    /// 就是为它们立的）；老五个元数据名一个都
    /// 不在（S6 「`NAME` 等仍放行」的前提）。
    #[test]
    fn the_generation_read_set_pins_its_dchc_zero_members_and_excludes_the_metadata_five() {
        let mut zero: Vec<&str> = generation_read_set()
            .iter()
            .filter(|n| raw_dchc_code(n) == Some(0))
            .map(String::as_str)
            .collect();
        zero.sort_unstable();
        assert_eq!(
            zero,
            [
                "CMPR", "DIR", "FILL", "GTYP", "MINBEN", "NGMR", "OUTD", "PKEY", "UKEY",
                "VXREF"
            ]
        );
        for name in DATA_ONLY_ATTR_NAMES {
            assert!(
                !generation_reads_attribute(name),
                "{name} 被生成器读到了——元数据门第二票会把它关掉，先弄清那处读是不是几何输入"
            );
        }
        // 位姿与几何输入当然在里面。
        for name in [
            "POS", "ORI", "DIAM", "HEIG", "SPRE", "CATR", "JOIS", "STAREF", "PARA",
        ] {
            assert!(generation_reads_attribute(name), "{name}");
        }
        // 2026-09-15 的规模：133 + 8 − 重叠 = 138。2026-09-19 随 `SPAMAP` 进戳表重生成产物：134 + 8 − 重叠 = 139
        // ——多出的一个是 `LBOR`（e3d-model 09-18 `226154c` 风管那条带进的读点，那边的产物早已 134，这边的副本
        // 一直没重生成）；两个戳都在 `stamp_literals` 里，一个都不进 `names`（上面那条循环钉住）。
        // 2026-09-27：GENSEC datum 标架带进 `HPREF`（140，当时漏改这里）；关节负体带进 `JOIS` / `JOIE` / `CREF` /
        // `NAPP`，前三个本就在模型面那一半里，并集只多 `NAPP`（141）；宿主 FIXING 扫描带进目录件的 `NGMR`（142）。
        // 2026-09-28：详图式扶手立柱 / 端头 / 门的现算标架（e3d-model `transform::handrail_post_world`，
        // 09-28 `128e831` 起那批栏杆提交）带进 `HRJUSL`（对正 LEFT / RIGH / FORW）与 `WIDT`（HRGATE 门宽）（144）。
        // 2026-10-16：e3d-model 电缆 EXTGEO 锚点解算在途（vendor cable.rs）带进 `CMPR` / `ENDPOI` /
        // `MINBEN` / `OUTD` / `STAPOI` / `YGAP`（150）。
        // 2026-10-21：随 e3d-model `f2a3b88`（墙 FIXING 的 LMIRR 镜像 + GENSEC 端部 datum 现算标架）重生成，
        // 带进 `HANICP`（151，`f909a3746` 当时漏改这里）。
        assert_eq!(generation_read_set().len(), 151, "读取集合条数漂移");
    }

    /// `PLCF`（开发计划 2026-09-15 S1 / D4）：三态与字典快照对得上，且**只作报表**——
    /// 本文件之外没有任何调用方，它一进判决这条就红。
    #[test]
    fn plcf_is_report_only_and_matches_the_dictionary_snapshot() {
        // 位姿三个都置旗标：POS/ORI 挪了，隐含管段的放置缓存就得失效。
        for name in ["POS", "ORI", "BFORI", "SPRE", "HEIG", "PARA", "att.pos"] {
            assert_eq!(raw_plcf(name), Some(true), "{name}");
        }
        // 字典里有名、旗标为 0（头尾连通的 HPOS/TPOS 也不置：它们走 INTUBE 强制码那条路）。
        for name in ["NAME", "DESC", "DIAM", "XLEN", "HPOS", "TPOS", "LEVE"] {
            assert_eq!(raw_plcf(name), Some(false), "{name}");
        }
        // 字典没有的：元素元数据、UDA、伪属性、强制码。
        for name in [
            "OWNER",
            "UDA:42",
            "CACHID",
            "REDRAW",
            "INTUBE",
            "NO_SUCH_ATTR",
        ] {
            assert_eq!(raw_plcf(name), None, "{name}");
        }

        // 快照口径：58 个置位，全在字典里；除 `CLGREF` 外都是 DCHC 3 / 4。
        let flagged = plcf_flagged_names();
        assert_eq!(flagged.len(), 58, "PLCF==1 的条数漂移");
        let mut zero_class: Vec<&str> = flagged
            .iter()
            .filter(|name| raw_dchc_code(name) == Some(0))
            .map(String::as_str)
            .collect();
        zero_class.sort_unstable();
        assert_eq!(zero_class, vec!["CLGREF"]);
        for name in flagged {
            assert!(
                matches!(raw_dchc_code(name), Some(0 | 3 | 4)),
                "{name}: PLCF==1 却是 DCHC 1/2——头尾连通属性不该置放置缓存旗标"
            );
        }
        let pose: BTreeSet<&str> = dchc_change_classes()
            .iter()
            .filter(|(_, code)| **code == 3)
            .map(|(name, _)| name.as_str())
            .collect();
        for name in pose {
            assert_eq!(raw_plcf(name), Some(true), "位姿类 {name} 必须置 PLCF");
        }

        // 源码钉：不进判决。本文件之外任何 .rs 提到 raw_plcf 都算越界。
        let src_root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut callers: Vec<String> = Vec::new();
        each_rs_file(&src_root, &mut |path, text| {
            if !path.ends_with("model_impact.rs") && text.contains("raw_plcf") {
                callers.push(path.display().to_string());
            }
        });
        assert!(
            callers.is_empty(),
            "PLCF 只作报表 / 探针，不进任何判决（D4）；有人开始消费它了：{callers:?}"
        );
    }

    /// WNOEVT 事件门（用户 2026-09-17 拍板接进增量判决）：core 对置位属性不发变更事件，
    /// [`attribute_is_metadata_only`] 在 DCHC 之前无条件短路它。这道门是 core `putAtt` 的第一关，
    /// 但对模型语料必须是恒等变换——本测试钉住：全部 7 个 WNOEVT 属性既不被生成器读、也不携带任何
    /// 正设计变化码（DCHC 0 或 None），所以无条件短路它不会漏算任何几何输入（守住「宁多勿漏」）。
    #[test]
    fn wnoevt_gate_is_disjoint_from_reads_and_geometry() {
        let names = wnoevt_no_event_names();
        // 快照口径：E3D 3.1 字典恰好 7 个（变了要重新确认它们仍非模型属性）。
        assert_eq!(names.len(), 7, "WNOEVT==1 的条数漂移");
        for name in [
            "DPOSCA", "EVITCA", "EVITCP", "EXTEMP", "LIGTCA", "LIGTCP", "TEMPLI",
        ] {
            assert!(names.contains(name), "{name} 应在 WNOEVT 名单里");
            // 事件门命中 → 无条件走「不动模型」，且这判定在 DCHC 之前（归一化 / att. 前缀同答）。
            assert!(attribute_no_event(name), "{name}");
            assert!(attribute_no_event(&format!("att.{name}")), "att.{name}");
            assert!(
                attribute_is_metadata_only(name),
                "{name} 事件门应短路为不动模型"
            );
            // 不动模型的安全前提：生成器不读它、字典也没有正的设计变化码（否则短路会漏算）。
            assert!(
                !generation_reads_attribute(name),
                "{name} 竟被生成器读取——无条件短路会漏算，需人工复核"
            );
            assert!(
                matches!(raw_dchc_code(name), None | Some(0)),
                "{name} 竟带正 DCHC 码——不该无条件短路"
            );
            // 端到端：单独改这一个 WNOEVT 属性 → 增量判决 Skip（接进 DCHC 之前那道短路的净效果；
            // 没接门时这些名字落 Unknown 会保守 Regen）。
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Skip,
                "{name} 应短路为 Skip"
            );
        }
        // 模型属性不受影响：事件门对它们一律 false。
        for name in ["POS", "ORI", "BORE", "DIAM", "HEIG", "NAME", "UDA:7"] {
            assert!(!attribute_no_event(name), "{name} 不该命中 WNOEVT 事件门");
        }
    }

    /// S6 真值表（开发计划 2026-09-15 D2）：元数据门要两票——字典 DCHC=0 **且** 生成器不读。
    /// 任一票缺席就照旧重算：读取集合里的 DCHC=0 属性不放行；`NAME` 等老五个仍放行；
    /// UDA / 字典没收的名字不放行。**唯一例外**是戳（2026-09-17 Q6）：`CACHID` 字典按构造收不到，
    /// 第一票由 e3d-model 的戳表给，第二票照旧问读取集合——放行，但不是老名单。
    #[test]
    fn metadata_only_needs_dchc_zero_and_an_unread_attribute() {
        // 老五个照旧放行（S5 钉过：生成器一个都不读），归一化后同答。
        for name in ["NAME", "DESC", "PURP", "FUNC", "LCHKDA", "name", "att.NAME"] {
            assert!(attribute_is_metadata_only(name), "{name}");
            assert!(attribute_is_curated_metadata(name), "{name}");
        }
        // 新档：字典 DCHC=0、生成器不读、手写表也不认识（此前落 Unknown 整根重算）。
        for name in ["DUTY", "PRES", "MATR", "BUIL", "ACCREF", "att.DUTY"] {
            assert_eq!(raw_dchc_code(name), Some(0), "{name}");
            assert!(!generation_reads_attribute(name), "{name}");
            assert_eq!(
                classify_attribute_effect(name),
                AttributeEffect::Unknown,
                "{name}"
            );
            assert!(attribute_is_metadata_only(name), "{name}");
            assert!(!attribute_is_curated_metadata(name), "{name}");
        }
        // 读取集合那一票：字典说 0、我们却真拿来生成的七个输入，一个都不放行。
        for name in [
            "DIR", "FILL", "GTYP", "NGMR", "PKEY", "UKEY", "VXREF", "att.dir",
        ] {
            assert_eq!(raw_dchc_code(name), Some(0), "{name}");
            assert!(generation_reads_attribute(name), "{name}");
            assert!(!attribute_is_metadata_only(name), "{name}");
        }
        // 字典那一票：DCHC ≠ 0 的几何 / 位姿 / 依赖属性，读不读都不放行。
        for name in [
            "DIAM", "POS", "ORI", "SPRE", "HEIG", "HPOS", "LHEA", "TEMP", "STEX",
        ] {
            assert_ne!(raw_dchc_code(name), Some(0), "{name}");
            assert!(!attribute_is_metadata_only(name), "{name}");
        }
        // 字典没投票（快照里没有）：不放行。`UDA:<id>`、伪属性、结构属性、写错的长名同理。
        for name in [
            "UDA:42",
            "OWNER",
            "FUNCTION",
            "NO_SUCH_ATTR",
            "REDRAW",
            "INTUBE",
        ] {
            assert_ne!(raw_dchc_code(name), Some(0), "{name}");
            assert!(!attribute_is_cache_stamp(name), "{name}");
            assert!(!attribute_is_metadata_only(name), "{name}");
        }
        // 戳（Q6，2026-09-17）：字典按构造收不到 `CACHID`（项目 UDA），第一票由 e3d-model 的戳表给、
        // 第二票读取集合守卫钉住它不是读点——放行；效果表判它 DataOnly，但它不是老名单（不是业务元数据）。
        for stamp in e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES {
            assert_eq!(
                raw_dchc_code(stamp),
                None,
                "{stamp} 进了字典快照？那就不该再走戳这条例外"
            );
            assert!(attribute_is_cache_stamp(stamp), "{stamp}");
            assert!(
                attribute_is_cache_stamp(&stamp.to_ascii_lowercase()),
                "{stamp}"
            );
            assert!(
                !generation_reads_attribute(stamp),
                "{stamp} 是戳却在读取集合里"
            );
            assert!(attribute_is_metadata_only(stamp), "{stamp}");
            assert!(!attribute_is_curated_metadata(stamp), "{stamp}");
            // 效果表也登记它 DataOnly（`DATA_ONLY_ATTR_NAMES`）——报表口径与门一致，但那不是放行的票。
            assert_eq!(
                classify_attribute_effect(stamp),
                AttributeEffect::DataOnly,
                "{stamp}"
            );
        }
        // 戳表今天恰好两个名字（`CACHID` 2026-09-16 / `SPAMAP` 2026-09-19）；多一个少一个都得回来重新过两票。
        let mut stamps: Vec<&str> = e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES.to_vec();
        stamps.sort_unstable();
        assert_eq!(stamps, ["CACHID", "SPAMAP"]);
        assert!(!attribute_is_cache_stamp("NAME") && !attribute_is_cache_stamp("POS"));
        // 老名单恰好五个，且是新门的子集。
        let curated: Vec<&str> = DATA_ONLY_ATTR_NAMES
            .iter()
            .copied()
            .filter(|name| attribute_is_curated_metadata(name))
            .collect();
        assert_eq!(curated, vec!["NAME", "DESC", "PURP", "FUNC", "LCHKDA"]);
        for name in curated {
            assert!(
                attribute_is_metadata_only(name),
                "{name} 老名单必须是新门的子集"
            );
        }
        // 规模：字典 3941 个 DCHC=0 的名字，读取集合挡十个，其余全部放行。
        let zero: Vec<&String> = dchc_change_classes()
            .iter()
            .filter(|(_, code)| **code == 0)
            .map(|(name, _)| name)
            .collect();
        assert_eq!(zero.len(), 3941, "字典 DCHC=0 条数漂移");
        let admitted = zero
            .iter()
            .filter(|name| attribute_is_metadata_only(name))
            .count();
        assert_eq!(
            admitted,
            3941 - 10,
            "放行数 = DCHC=0 − 读取集合里的 DCHC=0 成员"
        );
    }

    #[test]
    fn cable_property_inputs_are_not_metadata_only_even_when_dchc_is_zero() {
        for name in ["CMPR", "OUTD", "MINBEN"] {
            assert_eq!(raw_dchc_code(name), Some(0));
            assert!(generation_reads_attribute(name));
            assert!(!attribute_is_metadata_only(name));
            assert_eq!(metadata_only_tier([name]), None);
            assert_eq!(metadata_only_tier(["NAME", name]), None);
        }
    }

    /// S6 的分档：一次改动的属性全部放行才有档；全在老名单里是 `MetadataOnly`，混进一个新档名字
    /// 就是 `UnreadAttribute`（与顺序无关）；一个没拿到两票、或一个属性都没有 → `None`。
    /// Q6（2026-09-17）：戳放行但不改档，只有戳跳了才是第三档 `CacheStamp`。
    #[test]
    fn the_metadata_tier_is_curated_only_when_every_name_is_on_the_old_list() {
        let tier = |names: &[&str]| metadata_only_tier(names.iter().copied());
        assert_eq!(tier(&["NAME"]), Some(NoModelTier::MetadataOnly));
        assert_eq!(
            tier(&["DESC", "NAME", "PURP", "FUNC", "LCHKDA"]),
            Some(NoModelTier::MetadataOnly)
        );
        assert_eq!(tier(&["DUTY"]), Some(NoModelTier::UnreadAttribute));
        assert_eq!(tier(&["HREF"]), Some(NoModelTier::UnreadAttribute));
        assert_eq!(tier(&["NAME", "DUTY"]), Some(NoModelTier::UnreadAttribute));
        assert_eq!(tier(&["DUTY", "NAME"]), Some(NoModelTier::UnreadAttribute));
        assert_eq!(tier(&["NAME", "DIR"]), None);
        assert_eq!(tier(&["NAME", "DIAM"]), None);
        assert_eq!(tier(&["UDA:42"]), None);
        assert_eq!(tier(&[]), None);
        // 戳：单独跳是第三档；跟着谁就记谁的档；跟着没拿到两票的名字照旧 `None`。
        assert_eq!(tier(&["CACHID"]), Some(NoModelTier::CacheStamp));
        assert_eq!(tier(&["cachid"]), Some(NoModelTier::CacheStamp));
        assert_eq!(tier(&["NAME", "CACHID"]), Some(NoModelTier::MetadataOnly));
        assert_eq!(tier(&["CACHID", "NAME"]), Some(NoModelTier::MetadataOnly));
        assert_eq!(
            tier(&["DUTY", "CACHID"]),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            tier(&["CACHID", "NAME", "DUTY"]),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            tier(&["POS", "CACHID"]),
            None,
            "只动了摆放是锚点，不是元数据"
        );
        assert_eq!(tier(&["CACHID", "DIR"]), None);
        assert_eq!(tier(&["CACHID", "DIAM"]), None);
        assert_eq!(tier(&["CACHID", "UDA:42"]), None);
        // 第二个戳 `SPAMAP`（2026-09-19，与 `CACHID` 同档）：单独跳、两个一起跳都是第三档；跟着谁记谁的档；
        // 跟着摆放走锚点那条路。
        assert_eq!(tier(&["SPAMAP"]), Some(NoModelTier::CacheStamp));
        assert_eq!(tier(&["spamap"]), Some(NoModelTier::CacheStamp));
        assert_eq!(tier(&["CACHID", "SPAMAP"]), Some(NoModelTier::CacheStamp));
        assert_eq!(tier(&["NAME", "SPAMAP"]), Some(NoModelTier::MetadataOnly));
        assert_eq!(
            tier(&["SPAMAP", "DUTY", "CACHID"]),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(tier(&["POS", "SPAMAP"]), None);
        assert_eq!(tier(&["SPAMAP", "DIAM"]), None);
    }

    /// 两票压过手写表：五张手写表里登记为几何 / 级联输入、却字典 DCHC=0 且生成器不读的名字
    /// **恰好 15 个**，逐个点名。它们是当年按语义猜进表的（`HREF`/`TREF` 分支头尾引用——几何
    /// 读的是 `HPOS HDIR HBOR …` 这些 DCHC=1/2 的存储属性；`PSPE` 管道规格默认值——元件各有
    /// 自己的 `SPRE`；`DKEY`/`MAXA` 目录侧 `DATA`/`SELE` 才读；其余同理），core 不为它们重画、我们
    /// 也不读，判决以两票为准，手写表那一格只剩报表意义（`classify_attribute_effect` 照旧回级联 /
    /// 几何，建边资格不变）。名单变了就得重新问「新名字真的没人读吗」——多出来一个是读取集合漏了、
    /// 少一个是有人开始读它了：2026-09-27 少了 `NGMR`（e3d-model 宿主 FIXING 扫描读目录件的负体集）。
    #[test]
    fn the_two_votes_override_the_hand_tables_for_exactly_fifteen_names() {
        let mut overridden: Vec<&str> = ATTRIBUTE_EFFECT_TABLES
            .iter()
            .filter(|(_, effect)| *effect != AttributeEffect::DataOnly)
            .flat_map(|(names, _)| names.iter().copied())
            .filter(|name| attribute_is_metadata_only(name))
            .collect();
        overridden.sort_unstable();
        assert_eq!(
            overridden,
            [
                "CURD", "DDPR", "DKEY", "FSPREF", "HREF", "JFRE", "MAXA", "ORRF", "PSPE", "PTOF",
                "PTYP", "SCREF", "SIZE", "STYP", "TREF",
            ]
        );
        for name in &overridden {
            assert_eq!(raw_dchc_code(name), Some(0), "{name}");
            assert!(!generation_reads_attribute(name), "{name}");
            assert!(
                classify_attribute_effect(name).affects_model(),
                "{name} 手写表那一格没动"
            );
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                OperationImpact::Skip,
                "{name}"
            );
        }
        // 第六张（字典生成）表按构造只收 DCHC ≠ 0，一个都不会被压过。
        for (name, _) in dictionary_change_class_attr_names().entries() {
            assert!(!attribute_is_metadata_only(name), "{name}");
        }
    }

    /// 预览与执行同口径（2026-09-08）：`OperationImpact::Skip` 是「执行侧不会为它排模型
    /// 工作」这句承诺，判据必须与 direct 窗口规划器的元数据门逐名一致——两边共问
    /// [`attribute_is_metadata_only`] / [`metadata_only_tier`]，谁也不许自己再列一张名单。S6 起新档同理：
    /// `HREF` 只改了引用，规划器不上卷，预览也不许再说 Regen。Q6（2026-09-17）起 `CACHID` 由戳表拿到
    /// 第一票：规划器记 `no_model` 第三档，预览同样 Skip——此前它是「字典没投票 → 不是 Skip」的反例，
    /// 现在两边一起翻。
    #[test]
    fn the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate() {
        for name in DATA_ONLY_ATTR_NAMES {
            let expected = if attribute_is_metadata_only(name) {
                OperationImpact::Skip
            } else {
                OperationImpact::Regen
            };
            assert_eq!(
                classify_operation_impact(&modified_operation(name)),
                expected,
                "{name}"
            );
        }
        // 戳跟着老名单一起变照旧 Skip；只有戳跳了也是 Skip（规划器那边记 `CacheStamp`）。
        assert_eq!(
            classify_operation_impact(&modified_operation_with_attrs(&["NAME", "CACHID"])),
            OperationImpact::Skip
        );
        assert_eq!(
            classify_operation_impact(&modified_operation("CACHID")),
            OperationImpact::Skip
        );
        assert_eq!(
            metadata_only_tier(["CACHID"]),
            Some(NoModelTier::CacheStamp)
        );
        // 一次改动里混进一个没拿到第二票的名字（读取集合里的 `DIR`），整次操作就不再是 Skip。
        assert_eq!(
            classify_operation_impact(&modified_operation_with_attrs(&["CACHID", "DIR"])),
            OperationImpact::Regen
        );
        assert_eq!(
            classify_operation_impact(&modified_operation_with_attrs(&["NAME", "DESC"])),
            OperationImpact::Skip
        );
        // S6 新档：老名单 + 新档混合是 Skip；混进读取集合里的 DCHC=0 输入就不是。
        let mixed = classify_operation_effects(&modified_operation_with_attrs(&["NAME", "DUTY"]));
        assert_eq!(mixed.impact(), OperationImpact::Skip);
        assert_eq!(mixed.effects, vec![AttributeEffect::DataOnly]);
        assert_eq!(
            classify_operation_impact(&modified_operation_with_attrs(&["NAME", "DIR"])),
            OperationImpact::Regen
        );
        assert_eq!(
            metadata_only_tier(["NAME", "DUTY"]),
            Some(NoModelTier::UnreadAttribute)
        );
        // 源码钉：门那一侧问的是同一个谓词，不是自己抄的名单。
        let gate = include_str!("window_root_plan.rs");
        assert!(
            gate.contains("model_impact::metadata_only_tier"),
            "元数据门必须共用 metadata_only_tier"
        );
        assert!(
            gate.contains("model_impact::attribute_is_metadata_only"),
            "元数据门文档必须指回 attribute_is_metadata_only"
        );
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
