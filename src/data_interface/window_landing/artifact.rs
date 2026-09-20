//! 增量数据窗口的**内存产物**：Task 侧把窗口**终态**（`PersistElementChange` 逐 refno 一条）、
//! refno 复用的旧 noun 行、解好的 CATA 依赖闭包行，以及已经渲染成纯字串的 `ref_rev` 语句与
//! 随窗语句（`datacenter_version` 状态 + OWNER 搬迁 anc 修复）装进来，随即由
//! `window_landing::land_increment_window` **同步**落库（ADR-071 D1 / D11）——渲染器仍是
//! `IncrementPipeline::terminal_persist_batches` / `cata_dependency::persist_resolved_rows_on`
//! 那一份，产物里放的是事实，不是 SQL。它不落盘、不进 journal：任务与库之间没有中间文件。
//!
//! **为什么自带一套线格式、不直接 `serde` 领域类型**：`aios_core::NamedAttrValue` 的
//! `Deserialize` 是手写的、按库行形状猜类型（数字→`IntegerType`、字串→`StringType`、序列按
//! 首元素猜），与它派生出来的 `Serialize`（外部标签枚举）不成对——`RefU64Type` / `WordType` /
//! `ElementType` / `Vec3Type` 一来一回全变形。线格式是这份产物的稳定形状（日志 / 诊断导出 /
//! 测试对拍都靠它），必须逐位忠实，所以这里每个变体显式带标签、浮点按位存（`NaN` 也要原样
//! 回来），两边各一个 `match`。钉：`wire_round_trip_preserves_every_attribute_variant`。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::types::refno::RefnoSesno;
use aios_core::types::whole_attmap::ExplicitAttr;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64, RefU64Vec, RefnoEnum, WholeAttMap};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use crate::data_interface::cata_closure::CataClosureManifest;
use crate::data_interface::cata_dependency::CataDependencyRows;
use crate::data_interface::e3d_collector::RetiredTypeRow;
use crate::data_interface::ele_data::EleData;
use crate::data_interface::ele_ops::{PersistElementChange, PersistElementState};

/// 产物 `kind` 字段的取值。
pub const INCREMENT_WINDOW_KIND: &str = "increment_window";
/// 线格式版本。字段增删就 +1。
///
/// v2（2026-09-10）：加 `cache_refnos`——Task 侧算出的进程内缓存失效集随产物走，落库后
/// 再失效一次（见 [`IncrementWindowArtifact::cache_refnos`]）。
pub const INCREMENT_WINDOW_VERSION: u32 = 2;

/// 一个增量数据窗口的全部落库材料。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncrementWindowArtifact {
    pub kind: String,
    pub version: u32,
    pub project: String,
    pub dbnum: u32,
    pub db_type: String,
    /// 窗口基侧会话（= 上一水位）。
    pub from_sesno: u32,
    /// 窗口右端会话（= 本窗要前移到的水位）。
    pub target_sesno: u32,
    /// 每 refno 一条终态（`ele_ops::terminal_persist_changes` 的产物）。
    pub changes: Vec<WireElementChange>,
    /// refno 复用：旧 noun 的 `ATT_<old>:{id}` 行要显式删。
    pub retired_type_rows: Vec<WireRetiredTypeRow>,
    /// 本窗 DESI 元素依赖的 CATA 闭包行；非 DESI 窗口或没有 seed 时为 `None`。
    pub cata: Option<WireCataDependencyRows>,
    /// `manual_update::build_reverse_index_statements` 已渲染好的 `ref_rev` 语句（纯字串）。
    pub reverse_index_statements: Vec<String>,
    /// `datacenter_version` 状态更新 + OWNER 搬迁 anc 修复，固定目标 UPDATE（纯字串）。
    pub window_statements: Vec<String>,
    /// Task 侧 `collect_cache_invalidation_refnos` + 生成根算出的进程内缓存失效集（packed
    /// refno，升序去重）。Task 在装产物之前已经失效过一次；落库入口在结构行全部落定之后按这份
    /// 名单再失效一次（[`Self::cache_refnos`] 还并上产物里的元素与属主，名单缺席也不至于漏掉
    /// 本窗元素）——落库中途失败时前一次失效已生效，不留陈旧缓存。
    #[serde(default)]
    pub cache_refnos: Vec<u64>,
}

impl IncrementWindowArtifact {
    /// 领域类型 → 产物（Task 侧）。
    pub fn from_window(
        project: &str,
        dbnum: u32,
        db_type: &str,
        from_sesno: u32,
        target_sesno: u32,
        changes: &[PersistElementChange],
        retired_type_rows: &[RetiredTypeRow],
        cata: Option<&CataDependencyRows>,
        reverse_index_statements: Vec<String>,
        window_statements: Vec<String>,
        cache_refnos: impl IntoIterator<Item = RefnoEnum>,
    ) -> Self {
        // 升序去重：同一窗口重建出的工件必须逐字节相同（同内容同 id 才能在 journal 里认出
        // 「已经 accept 过」），`HashSet` 的遍历序不能漏进线格式。
        let cache_refnos: BTreeSet<u64> = cache_refnos
            .into_iter()
            .map(|refno| refno.refno().0)
            .collect();
        Self {
            kind: INCREMENT_WINDOW_KIND.to_owned(),
            version: INCREMENT_WINDOW_VERSION,
            project: project.to_owned(),
            dbnum,
            db_type: db_type.to_owned(),
            from_sesno,
            target_sesno,
            changes: changes.iter().map(WireElementChange::from).collect(),
            retired_type_rows: retired_type_rows
                .iter()
                .map(WireRetiredTypeRow::from)
                .collect(),
            cata: cata
                .filter(|rows| rows.elements() > 0 || rows.manifest.missing > 0)
                .map(WireCataDependencyRows::from),
            reverse_index_statements,
            window_statements,
            cache_refnos: cache_refnos.into_iter().collect(),
        }
    }

    /// lane 落库后要失效的进程内缓存键：Task 随工件带来的名单，并上本窗每条终态的元素与
    /// 属主（Deleted 取基侧属主）。并集而不是二选一——名单缺席（旧工件）时至少本窗元素不漏。
    pub fn cache_refnos(&self) -> BTreeSet<RefnoEnum> {
        let mut refnos: BTreeSet<RefnoEnum> = self
            .cache_refnos
            .iter()
            .map(|refno| RefnoEnum::from(RefU64(*refno)))
            .collect();
        for change in &self.changes {
            refnos.insert(RefnoEnum::from(RefU64(change.refno)));
            let element = match &change.state {
                WireElementState::Add { element } | WireElementState::Modified { element } => {
                    Some(element)
                }
                WireElementState::Deleted { base } => base.as_ref(),
            };
            if let Some(element) = element.filter(|element| element.owner != 0) {
                refnos.insert(RefnoEnum::from(RefU64(element.owner)));
            }
        }
        refnos
    }

    /// 工件 → 领域类型（sink 侧）。
    pub fn persist_changes(&self) -> Vec<PersistElementChange> {
        self.changes
            .iter()
            .map(PersistElementChange::from)
            .collect()
    }

    pub fn retired_rows(&self) -> Vec<RetiredTypeRow> {
        self.retired_type_rows
            .iter()
            .map(RetiredTypeRow::from)
            .collect()
    }

    pub fn cata_rows(&self) -> Option<CataDependencyRows> {
        self.cata.as_ref().map(CataDependencyRows::from)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireElementChange {
    pub refno: u64,
    pub sesno: u32,
    pub state: WireElementState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WireElementState {
    Add { element: WireElement },
    Modified { element: WireElement },
    Deleted { base: Option<WireElement> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireElement {
    pub refno: u64,
    pub owner: u64,
    pub noun: u32,
    pub name: String,
    pub children: Vec<u64>,
    pub attmap: Vec<(String, WireAttrValue)>,
    pub explicit_attmap: Vec<(String, WireAttrValue)>,
    pub uda_atts: Vec<WireExplicitAttr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireExplicitAttr {
    pub name: String,
    pub value: WireAttrValue,
    pub is_uda: bool,
    pub hash_val: i32,
}

/// `NamedAttrValue` 的逐变体显式编码。浮点一律按 IEEE 位存（`u32`），`NaN` / 负零原样回来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum WireAttrValue {
    Invalid,
    Integer(i32),
    String(String),
    F32(u32),
    F32Vec(Vec<u32>),
    Vec3([u32; 3]),
    StringArray(Vec<String>),
    BoolArray(Vec<bool>),
    IntArray(Vec<i32>),
    Bool(bool),
    Element(String),
    Word(String),
    RefU64(u64),
    RefU64Array(Vec<WireRefno>),
    Long(i64),
    Refno(WireRefno),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WireRefno {
    Refno { refno: u64 },
    SesRef { refno: u64, sesno: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireRetiredTypeRow {
    pub refno: u64,
    pub old_noun: String,
    pub new_noun: String,
}

/// `CataDependencyRows`：行按 CATA dbnum 分组的 `(sesno, 元素)`；闭包账只留回执要用的两格
/// （`missing` / `missing_samples`），`by_dbnum` 名单从行重建，BFS 统计不背。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireCataDependencyRows {
    pub by_dbnum: Vec<(u32, Vec<(u32, WireElement)>)>,
    pub missing: usize,
    pub missing_samples: Vec<String>,
}

// ---- 领域 → 线 ----

impl From<&PersistElementChange> for WireElementChange {
    fn from(change: &PersistElementChange) -> Self {
        Self {
            refno: change.refno.0,
            sesno: change.sesno,
            state: match &change.state {
                PersistElementState::Add(element) => WireElementState::Add {
                    element: WireElement::from(element),
                },
                PersistElementState::Modified(element) => WireElementState::Modified {
                    element: WireElement::from(element),
                },
                PersistElementState::Deleted { base } => WireElementState::Deleted {
                    base: base.as_ref().map(WireElement::from),
                },
            },
        }
    }
}

impl From<&EleData> for WireElement {
    fn from(element: &EleData) -> Self {
        Self {
            refno: element.refno.0,
            owner: element.owner.0,
            noun: element.noun,
            name: element.name.clone(),
            children: element.children.0.iter().map(|refno| refno.0).collect(),
            attmap: wire_attmap(&element.whole_attmap.attmap),
            explicit_attmap: wire_attmap(&element.whole_attmap.explicit_attmap),
            uda_atts: element
                .whole_attmap
                .uda_atts
                .iter()
                .map(|attr| WireExplicitAttr {
                    name: attr.name.clone(),
                    value: WireAttrValue::from(&attr.value),
                    is_uda: attr.is_uda,
                    hash_val: attr.hash_val,
                })
                .collect(),
        }
    }
}

fn wire_attmap(map: &NamedAttrMap) -> Vec<(String, WireAttrValue)> {
    map.map
        .iter()
        .map(|(key, value)| (key.clone(), WireAttrValue::from(value)))
        .collect()
}

impl From<&NamedAttrValue> for WireAttrValue {
    fn from(value: &NamedAttrValue) -> Self {
        match value {
            NamedAttrValue::InvalidType => Self::Invalid,
            NamedAttrValue::IntegerType(v) => Self::Integer(*v),
            NamedAttrValue::StringType(v) => Self::String(v.clone()),
            NamedAttrValue::F32Type(v) => Self::F32(v.to_bits()),
            NamedAttrValue::F32VecType(v) => Self::F32Vec(v.iter().map(|f| f.to_bits()).collect()),
            NamedAttrValue::Vec3Type(v) => {
                Self::Vec3([v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
            }
            NamedAttrValue::StringArrayType(v) => Self::StringArray(v.clone()),
            NamedAttrValue::BoolArrayType(v) => Self::BoolArray(v.clone()),
            NamedAttrValue::IntArrayType(v) => Self::IntArray(v.clone()),
            NamedAttrValue::BoolType(v) => Self::Bool(*v),
            NamedAttrValue::ElementType(v) => Self::Element(v.clone()),
            NamedAttrValue::WordType(v) => Self::Word(v.clone()),
            NamedAttrValue::RefU64Type(v) => Self::RefU64(v.0),
            NamedAttrValue::RefU64Array(v) => {
                Self::RefU64Array(v.iter().map(WireRefno::from).collect())
            }
            NamedAttrValue::LongType(v) => Self::Long(*v),
            NamedAttrValue::RefnoEnumType(v) => Self::Refno(WireRefno::from(v)),
        }
    }
}

impl From<&RefnoEnum> for WireRefno {
    fn from(value: &RefnoEnum) -> Self {
        match value {
            RefnoEnum::Refno(refno) => Self::Refno { refno: refno.0 },
            RefnoEnum::SesRef(ses) => Self::SesRef {
                refno: ses.refno.0,
                sesno: ses.sesno,
            },
        }
    }
}

impl From<&RetiredTypeRow> for WireRetiredTypeRow {
    fn from(row: &RetiredTypeRow) -> Self {
        Self {
            refno: row.refno.0,
            old_noun: row.old_noun.clone(),
            new_noun: row.new_noun.clone(),
        }
    }
}

impl From<&CataDependencyRows> for WireCataDependencyRows {
    fn from(rows: &CataDependencyRows) -> Self {
        Self {
            by_dbnum: rows
                .by_dbnum
                .iter()
                .map(|(dbnum, elements)| {
                    (
                        *dbnum,
                        elements
                            .iter()
                            .map(|(sesno, element)| (*sesno, WireElement::from(element)))
                            .collect(),
                    )
                })
                .collect(),
            missing: rows.manifest.missing,
            missing_samples: rows.manifest.missing_samples.clone(),
        }
    }
}

// ---- 线 → 领域 ----

impl From<&WireElementChange> for PersistElementChange {
    fn from(change: &WireElementChange) -> Self {
        Self {
            refno: RefU64(change.refno),
            sesno: change.sesno,
            state: match &change.state {
                WireElementState::Add { element } => {
                    PersistElementState::Add(EleData::from(element))
                }
                WireElementState::Modified { element } => {
                    PersistElementState::Modified(EleData::from(element))
                }
                WireElementState::Deleted { base } => PersistElementState::Deleted {
                    base: base.as_ref().map(EleData::from),
                },
            },
        }
    }
}

impl From<&WireElement> for EleData {
    fn from(element: &WireElement) -> Self {
        let mut out = EleData::default();
        out.refno = RefU64(element.refno);
        out.owner = RefU64(element.owner);
        out.noun = element.noun;
        out.name = element.name.clone();
        out.children = RefU64Vec(
            element
                .children
                .iter()
                .map(|refno| RefU64(*refno))
                .collect(),
        );
        out.whole_attmap = WholeAttMap {
            attmap: named_attmap(&element.attmap),
            explicit_attmap: named_attmap(&element.explicit_attmap),
            uda_atts: element
                .uda_atts
                .iter()
                .map(|attr| ExplicitAttr {
                    name: attr.name.clone(),
                    value: NamedAttrValue::from(&attr.value),
                    is_uda: attr.is_uda,
                    hash_val: attr.hash_val,
                })
                .collect(),
        };
        out
    }
}

fn named_attmap(entries: &[(String, WireAttrValue)]) -> NamedAttrMap {
    NamedAttrMap {
        map: entries
            .iter()
            .map(|(key, value)| (key.clone(), NamedAttrValue::from(value)))
            .collect::<BTreeMap<_, _>>(),
    }
}

impl From<&WireAttrValue> for NamedAttrValue {
    fn from(value: &WireAttrValue) -> Self {
        match value {
            WireAttrValue::Invalid => Self::InvalidType,
            WireAttrValue::Integer(v) => Self::IntegerType(*v),
            WireAttrValue::String(v) => Self::StringType(v.clone()),
            WireAttrValue::F32(bits) => Self::F32Type(f32::from_bits(*bits)),
            WireAttrValue::F32Vec(bits) => {
                Self::F32VecType(bits.iter().map(|bits| f32::from_bits(*bits)).collect())
            }
            WireAttrValue::Vec3([x, y, z]) => Self::Vec3Type(glam::Vec3::new(
                f32::from_bits(*x),
                f32::from_bits(*y),
                f32::from_bits(*z),
            )),
            WireAttrValue::StringArray(v) => Self::StringArrayType(v.clone()),
            WireAttrValue::BoolArray(v) => Self::BoolArrayType(v.clone()),
            WireAttrValue::IntArray(v) => Self::IntArrayType(v.clone()),
            WireAttrValue::Bool(v) => Self::BoolType(*v),
            WireAttrValue::Element(v) => Self::ElementType(v.clone()),
            WireAttrValue::Word(v) => Self::WordType(v.clone()),
            WireAttrValue::RefU64(v) => Self::RefU64Type(RefU64(*v)),
            WireAttrValue::RefU64Array(v) => {
                Self::RefU64Array(v.iter().map(RefnoEnum::from).collect())
            }
            WireAttrValue::Long(v) => Self::LongType(*v),
            WireAttrValue::Refno(v) => Self::RefnoEnumType(RefnoEnum::from(v)),
        }
    }
}

impl From<&WireRefno> for RefnoEnum {
    fn from(value: &WireRefno) -> Self {
        match value {
            WireRefno::Refno { refno } => Self::Refno(RefU64(*refno)),
            WireRefno::SesRef { refno, sesno } => {
                Self::SesRef(RefnoSesno::new(RefU64(*refno), *sesno))
            }
        }
    }
}

impl From<&WireRetiredTypeRow> for RetiredTypeRow {
    fn from(row: &WireRetiredTypeRow) -> Self {
        Self {
            refno: RefU64(row.refno),
            old_noun: row.old_noun.clone(),
            new_noun: row.new_noun.clone(),
        }
    }
}

impl From<&WireCataDependencyRows> for CataDependencyRows {
    fn from(rows: &WireCataDependencyRows) -> Self {
        let mut by_dbnum: BTreeMap<u32, Vec<(u32, EleData)>> = BTreeMap::new();
        let mut manifest_by_dbnum: BTreeMap<u32, BTreeSet<RefU64>> = BTreeMap::new();
        for (dbnum, elements) in &rows.by_dbnum {
            let entry = by_dbnum.entry(*dbnum).or_default();
            let names = manifest_by_dbnum.entry(*dbnum).or_default();
            for (sesno, element) in elements {
                names.insert(RefU64(element.refno));
                entry.push((*sesno, EleData::from(element)));
            }
        }
        let mut manifest = CataClosureManifest::default();
        manifest.by_dbnum = manifest_by_dbnum;
        manifest.missing = rows.missing;
        manifest.missing_samples = rows.missing_samples.clone();
        // 解析账不过线：落库侧只有渲染 / 字节那一半的账（`CataDependencyReport` 里零值不序列化）。
        Self {
            by_dbnum,
            manifest,
            account: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::ele_ops::UdaKeyMap;
    use crate::data_interface::increment_pipeline::IncrementPipeline;

    fn refu(n: u64) -> RefU64 {
        RefU64((4000000001u64 << 32) | n)
    }

    /// 一个把 `NamedAttrValue` 十六个变体都用上的元素，含 `NaN` / 负零 / 空数组这类
    /// JSON 天然会走样的值。
    fn every_variant_element(refno: RefU64, owner: RefU64) -> EleData {
        let mut element = EleData::default();
        element.refno = refno;
        element.owner = owner;
        element.noun = aios_core::tool::db_tool::db1_hash("EQUI");
        element.name = "/RT-EQUI".into();
        element.children = RefU64Vec(vec![refu(2), refu(3)]);
        let map = &mut element.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        map.insert("TYPE".into(), NamedAttrValue::StringType("EQUI".into()));
        map.insert("NAME".into(), NamedAttrValue::StringType("/RT-EQUI".into()));
        map.insert("INVALID".into(), NamedAttrValue::InvalidType);
        map.insert("LOCK".into(), NamedAttrValue::BoolType(true));
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(7981));
        map.insert("BIG".into(), NamedAttrValue::LongType(i64::MAX - 7));
        map.insert("ANGLE".into(), NamedAttrValue::F32Type(-0.0));
        map.insert("NOTANUMBER".into(), NamedAttrValue::F32Type(f32::NAN));
        map.insert(
            "PARA".into(),
            NamedAttrValue::F32VecType(vec![1.5, f32::INFINITY, -2.25e-7]),
        );
        map.insert("EMPTYPARA".into(), NamedAttrValue::F32VecType(Vec::new()));
        map.insert(
            "POS".into(),
            NamedAttrValue::Vec3Type(glam::Vec3::new(1.0, -2.0, f32::NEG_INFINITY)),
        );
        map.insert(
            "DESC".into(),
            NamedAttrValue::StringArrayType(vec!["a".into(), "".into()]),
        );
        map.insert(
            "FLAGS".into(),
            NamedAttrValue::BoolArrayType(vec![true, false]),
        );
        map.insert(
            "INTS".into(),
            NamedAttrValue::IntArrayType(vec![-1, 0, i32::MAX]),
        );
        map.insert("ELEM".into(), NamedAttrValue::ElementType("SITE".into()));
        map.insert("WORD".into(), NamedAttrValue::WordType("UNSET".into()));
        map.insert(
            "REFS".into(),
            NamedAttrValue::RefU64Array(vec![
                RefnoEnum::Refno(refu(9)),
                RefnoEnum::SesRef(RefnoSesno::new(refu(10), 44)),
            ]),
        );
        map.insert(
            "SESREF".into(),
            NamedAttrValue::RefnoEnumType(RefnoEnum::SesRef(RefnoSesno::new(refu(11), 45))),
        );
        element
            .whole_attmap
            .explicit_attmap
            .map
            .insert("PURP".into(), NamedAttrValue::StringType("pump".into()));
        element.whole_attmap.uda_atts.push(ExplicitAttr {
            name: ":ROOM".into(),
            value: NamedAttrValue::StringType("R-101".into()),
            is_uda: true,
            hash_val: 0x171F_AD39_u32 as i32 + 12,
        });
        element
    }

    fn fixture_changes() -> (Vec<PersistElementChange>, Vec<RetiredTypeRow>) {
        let base = every_variant_element(refu(1), refu(0));
        let mut modified = every_variant_element(refu(2), refu(1));
        modified.name = "/RT-MODIFIED".into();
        let changes = vec![
            PersistElementChange {
                refno: refu(1),
                sesno: 42,
                state: PersistElementState::Add(every_variant_element(refu(1), refu(0))),
            },
            PersistElementChange {
                refno: refu(2),
                sesno: 43,
                state: PersistElementState::Modified(modified),
            },
            PersistElementChange {
                refno: refu(3),
                sesno: 43,
                state: PersistElementState::Deleted { base: Some(base) },
            },
            PersistElementChange {
                refno: refu(4),
                sesno: 43,
                state: PersistElementState::Deleted { base: None },
            },
        ];
        let retired = vec![RetiredTypeRow {
            refno: refu(2),
            old_noun: "NOZZ".into(),
            new_noun: "EQUI".into(),
        }];
        (changes, retired)
    }

    fn render(changes: &[PersistElementChange], retired: &[RetiredTypeRow]) -> Vec<String> {
        IncrementPipeline::terminal_persist_batches(changes, retired, 7981, &UdaKeyMap::empty())
            .expect("render")
            .0
    }

    /// 线格式 → JSON → 线格式逐位相等；解回领域类型后渲出的 SQL 与原件逐字相等——
    /// 渲染器读的是每一个属性值，所以这一步也覆盖了走样最容易发生的地方。
    #[test]
    fn wire_round_trip_preserves_every_attribute_variant() {
        let (changes, retired) = fixture_changes();
        let mut cata = CataDependencyRows::default();
        cata.by_dbnum
            .insert(6001, vec![(17, every_variant_element(refu(21), refu(20)))]);
        cata.manifest.missing = 2;
        cata.manifest.missing_samples = vec!["6002/1".into()];
        let artifact = IncrementWindowArtifact::from_window(
            "AMS",
            7981,
            "DESI",
            41,
            43,
            &changes,
            &retired,
            Some(&cata),
            vec!["RELATE a->ref_rev->b;".into()],
            vec!["UPDATE datacenter_version:x SET status = 'Modify';".into()],
            // 故意乱序 + 重复：线格式里必须是升序去重的。
            [refu(30), refu(1), refu(30)].map(RefnoEnum::from),
        );
        assert_eq!(
            artifact.cache_refnos,
            vec![refu(1).0, refu(30).0],
            "失效名单升序去重，工件才能同内容同 id"
        );

        assert_eq!(
            (artifact.kind.as_str(), artifact.version),
            (INCREMENT_WINDOW_KIND, INCREMENT_WINDOW_VERSION)
        );
        let json = serde_json::to_vec(&artifact).expect("encode");
        let restored: IncrementWindowArtifact = serde_json::from_slice(&json).expect("decode");
        assert_eq!(restored, artifact, "线格式一来一回必须逐位相等");

        assert_eq!(
            render(&restored.persist_changes(), &restored.retired_rows()),
            render(&changes, &retired),
            "解回的终态渲出的 pe / ATT_ 语句必须与原件逐字相等"
        );
        let cata_back = restored.cata_rows().expect("cata rows");
        assert_eq!(cata_back.elements(), 1);
        assert_eq!(cata_back.manifest.missing, 2);
        assert_eq!(
            cata_back.manifest.by_dbnum.get(&6001).map(BTreeSet::len),
            Some(1),
            "闭包名单从行重建"
        );

        // 直接 `serde` 领域类型会走样的那几个值，逐个点名核一遍。
        let add = restored
            .persist_changes()
            .into_iter()
            .find(|change| change.refno == refu(1))
            .expect("add");
        let PersistElementState::Add(element) = add.state else {
            panic!("expected Add");
        };
        let map = &element.whole_attmap.attmap.map;
        assert_eq!(map["REFNO"], NamedAttrValue::RefU64Type(refu(1)));
        assert_eq!(map["WORD"], NamedAttrValue::WordType("UNSET".into()));
        assert_eq!(map["ELEM"], NamedAttrValue::ElementType("SITE".into()));
        assert_eq!(map["INVALID"], NamedAttrValue::InvalidType);
        match &map["NOTANUMBER"] {
            NamedAttrValue::F32Type(v) => assert!(v.is_nan(), "NaN 要原样回来"),
            other => panic!("NaN 变形成 {other:?}"),
        }
        match &map["ANGLE"] {
            NamedAttrValue::F32Type(v) => {
                assert!(v.is_sign_negative() && *v == 0.0, "负零要原样回来")
            }
            other => panic!("-0.0 变形成 {other:?}"),
        }
        assert_eq!(map["EMPTYPARA"], NamedAttrValue::F32VecType(Vec::new()));
        assert_eq!(
            map["SESREF"],
            NamedAttrValue::RefnoEnumType(RefnoEnum::SesRef(RefnoSesno::new(refu(11), 45)))
        );
        assert_eq!(element.whole_attmap.uda_atts.len(), 1);
        assert_eq!(element.children.0, vec![refu(2), refu(3)]);
    }

    /// 反例：直接 `serde_json` 领域类型确实会走样——这条钉住「为什么要自带线格式」，
    /// 若某天 aios_core 把 `NamedAttrValue` 的 `Deserialize` 修成对的，这条会红，届时可以
    /// 评估退掉本模块的手写编码。
    #[test]
    fn deriving_serde_on_the_domain_value_is_not_a_faithful_round_trip() {
        let original = NamedAttrValue::RefU64Type(refu(1));
        let json = serde_json::to_string(&original).expect("encode");
        match serde_json::from_str::<NamedAttrValue>(&json) {
            Ok(back) => assert_ne!(
                back, original,
                "aios_core 的 Deserialize 目前不与 Serialize 成对"
            ),
            Err(_) => {} // 解不回来也是「不成对」
        }
    }

    #[test]
    fn cata_is_omitted_when_the_window_has_no_dependency_rows() {
        let (changes, retired) = fixture_changes();
        let artifact = IncrementWindowArtifact::from_window(
            "AMS",
            7981,
            "DESI",
            41,
            43,
            &changes,
            &retired,
            Some(&CataDependencyRows::default()),
            Vec::new(),
            Vec::new(),
            [],
        );
        assert!(artifact.cata.is_none());
        assert!(artifact.cata_rows().is_none());
    }

    /// lane 侧的失效名单 = Task 名单 ∪ 本窗元素 ∪ 属主（Deleted 取基侧属主）：名单缺席
    /// （`cache_refnos` 为空）时本窗元素与属主也一个不漏。
    #[test]
    fn lane_cache_targets_cover_the_task_list_plus_every_element_and_owner() {
        let (changes, retired) = fixture_changes();
        let without_list = IncrementWindowArtifact::from_window(
            "AMS",
            7981,
            "DESI",
            41,
            43,
            &changes,
            &retired,
            None,
            Vec::new(),
            Vec::new(),
            [],
        );
        let derived = without_list.cache_refnos();
        for change in &changes {
            assert!(
                derived.contains(&RefnoEnum::from(change.refno)),
                "本窗元素 {:?} 必须在失效名单里",
                change.refno
            );
            let owner = match &change.state {
                PersistElementState::Add(e) | PersistElementState::Modified(e) => Some(e.owner),
                PersistElementState::Deleted { base } => base.as_ref().map(|e| e.owner),
            };
            if let Some(owner) = owner.filter(|owner| owner.0 != 0) {
                assert!(
                    derived.contains(&RefnoEnum::from(owner)),
                    "属主 {owner:?} 必须在失效名单里"
                );
            }
        }

        let with_list = IncrementWindowArtifact::from_window(
            "AMS",
            7981,
            "DESI",
            41,
            43,
            &changes,
            &retired,
            None,
            Vec::new(),
            Vec::new(),
            [RefnoEnum::from(refu(900))],
        );
        let merged = with_list.cache_refnos();
        assert!(
            merged.contains(&RefnoEnum::from(refu(900))),
            "Task 名单要并进来"
        );
        assert!(merged.is_superset(&derived), "并集，不是二选一");
    }
}
