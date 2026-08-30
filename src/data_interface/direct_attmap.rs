//! ADR-053 Q4：把 e3d-io 直读出来的元素属性转成生成链消费的 `NamedAttrMap`。
//!
//! **形状的权威是 DB 读侧的 schema，不是文件。** `aios_core` 的
//! `From<SurlValue> for NamedAttrMap` 决定每个键落成哪个 `NamedAttrValue` 变体：
//! 它拿 `PdmsDatabaseInfo::named_attr_info_map[noun][key].default_val` 的变体去套
//! Surreal 值，`REFNO`/`OWNER`/`TYPE`/`PGNO`/`SESNO` 先走特判，schema 不认识的键
//! 直接 `continue` 跳过。生成链上那些 `if let NamedAttrValue::Vec3Type(v)` 就是照这
//! 张表写的，所以 direct 侧必须查**同一张表**——各自按文件里的存储类型自由发挥，
//! POS 会变成 `F32VecType`，一个 `if let` 就静默失配了。
//!
//! 于是本模块只做一件事：拿 e3d-io 的 `DescriptorValue`（文件说这个值**是什么**），
//! 按 schema 说的**该是什么**去转。转不过去的不猜——记进
//! [`DirectAttrs::shape_conflicts`] 由调用方处置。
//!
//! P0（`docs/plans/direct-mode-model-generation.md`）定下的四类残差，本模块的处置：
//!
//! 1. **词哈希归一**：文件里存的是词哈希整数，DB 读侧反哈希成 `WordType` 字符串
//!    （0 → 空串）。这里照做，用 `e3d_attlib::db1_dehash` / `lookup_system_name`——
//!    与写库侧同一套 base-27 反哈希。
//! 2. **DB 读损耗键**（TYPEX/UNIPAR 等）：direct 有值、DB 读侧落空串。保留 direct 原值，
//!    超集无害（生成不消费这些键的值）。
//! 3. **DB 行历史缺键**（SPAMAP/BULG 等）：同上，保留。
//! 4. **SESNO**：写库簿记字段（行最后写入时的会话号）与文件里元素的会话号语义不同，
//!    是元数据不是属性——本模块**不产出**它，也不假装产出。
//!
//! 还有一类 P0 观察到的：`Text` 解出来只剩不可见字节（未设字段的零/遗留字节被
//! stringify）。语义上就是空，[`normalize_text`] 归一为空串以对齐 DB 视图。

use std::collections::BTreeMap;

use aios_core::types::NamedAttrValue;
use aios_core::{AttrVal, NamedAttrMap, RefU64, RefnoEnum};
use e3d_io::record::descriptor::{
    AttributeExtractionStatus, DescriptorValue, ElementExtraction, ExtractedAttribute,
};
use e3d_io::refno::RefNo;
use glam::Vec3;

/// 一次转换的产物：属性表，加上「没能进表的东西」。
///
/// 后三个列表不是日志，是回执：调用方（探针 / provider）据此判定这次直读能不能当数。
/// 只把 `map` 交出去、把落空的键咽掉，正是静默失效。
#[derive(Debug, Clone, Default)]
pub struct DirectAttrs {
    pub map: NamedAttrMap,
    /// schema 不认识的键。DB 读侧对它们 `continue`，所以生成期本来就看不见；
    /// direct 侧同样不放进 `map`，只在这里留名。
    pub outside_schema: Vec<String>,
    /// 文件说这个属性是 E3D 的逻辑 `unset`——它与 0 / 空串 / Nulref 都不是一回事。
    /// 不编一个默认值塞进去；DB 行在这些键上放的是什么，由对拍量出来再定。
    pub unset: Vec<String>,
    /// 文件里的存储形状与 schema 声明的**数值/引用**类型对不上。**不猜**，列出来。
    ///
    /// 这一类必须当错处理：声明成数字的属性生成链会拿去算几何，按错的读法解出来的
    /// 数字长得和对的一模一样。已知一例是 `BANG`——e3d-io 把它解成 `Word`，真库上
    /// `raw` 是 i32 的百分之一度（`4294958296` = −9000 → DB 的 −90.0）。那是 e3d-io
    /// 描述符定型的缺口，该在那边修，不该在这里按一个样本编一条缩放规则。
    pub shape_conflicts: Vec<ShapeConflict>,
    /// schema 声明成文本、文件里却是字/整数的键。
    ///
    /// 与上面一类分开，因为**它们不是同一种风险**。这些都是 DB 读侧自己也读不出来的
    /// 簿记键（`TYPEX` / `UNIPAR` / `SPAMAP`），DB 视图上是空串或干脆没这个键，生成链
    /// 不消费它们的值（P0 残差分类第 2、3 条）。这里按文件原样交出去——direct 是超集，
    /// 超集无害；抹成空串反倒是把信息扔了。
    pub view_divergence: Vec<ShapeConflict>,
    /// 描述符在场但没解出值，且原因不是「本就没有值」。
    pub undecoded: Vec<Undecoded>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeConflict {
    pub name: String,
    /// 文件里的形状，如 `RealArray[2]`。
    pub found: String,
    /// schema 声明的类型，如 `Vec3Type`。
    pub declared: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undecoded {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectAttrError {
    /// noun 哈希反不出名字就查不到 schema，而 schema 是形状的权威。没有它只能瞎猜
    /// 每个键该是什么类型，那正是本模块存在的理由。
    UnnamedNoun {
        refno: String,
        noun_hash: u32,
    },

    NounNotInSchema {
        noun: String,
        refno: String,
    },
}

impl std::fmt::Display for DirectAttrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnnamedNoun { refno, noun_hash } => write!(
                f,
                "元素 {refno} 的 noun 哈希 {noun_hash:#x} 反不出名字，取不到属性 schema"
            ),
            Self::NounNotInSchema { noun, refno } => write!(
                f,
                "noun {noun}（元素 {refno}）不在属性 schema 里，它的键一个都定不了型"
            ),
        }
    }
}

impl std::error::Error for DirectAttrError {}

/// `ElementExtraction` → `NamedAttrMap`，按 DB 读侧 schema 定形。
pub fn to_named_attmap(extraction: &ElementExtraction) -> Result<DirectAttrs, DirectAttrError> {
    let refno = refu64(extraction.refno);
    let noun = extraction
        .noun_name
        .as_deref()
        .ok_or_else(|| DirectAttrError::UnnamedNoun {
            refno: refno.to_string(),
            noun_hash: extraction.noun_hash,
        })?;

    let db_info = aios_core::get_default_pdms_db_info();
    let schema =
        db_info
            .named_attr_info_map
            .get(noun)
            .ok_or_else(|| DirectAttrError::NounNotInSchema {
                noun: noun.to_string(),
                refno: refno.to_string(),
            })?;

    // 只借出「键 → 声明类型」这一件事，不把 schema 容器的类型摆到内部签名上。
    let declared_type = |key: &str| schema.get(key).map(|info| info.default_val.clone());

    let mut out = DirectAttrs::default();
    let mut map: BTreeMap<String, NamedAttrValue> = BTreeMap::new();

    // 元素身份三键，与 DB 读侧的特判同序同形。
    map.insert("REFNO".to_string(), NamedAttrValue::RefU64Type(refno));
    map.insert(
        "OWNER".to_string(),
        NamedAttrValue::RefU64Type(refu64(extraction.owner)),
    );
    map.insert(
        "TYPE".to_string(),
        NamedAttrValue::StringType(noun.to_string()),
    );

    for attribute in &extraction.attributes {
        convert_one(attribute, &declared_type, &mut map, &mut out);
    }

    out.map = NamedAttrMap { map };
    Ok(out)
}

fn convert_one(
    attribute: &ExtractedAttribute,
    declared_type: &dyn Fn(&str) -> Option<AttrVal>,
    map: &mut BTreeMap<String, NamedAttrValue>,
    out: &mut DirectAttrs,
) {
    let name = attribute.name.as_str();
    // 身份三键由上面写死，描述符里再来一份就以上面为准（同一事实两处来源，取权威那处）。
    if matches!(name, "REFNO" | "OWNER" | "TYPE" | "SESNO") {
        return;
    }

    let Some(value) = attribute.value.as_ref() else {
        if let Some(reason) = undecoded_reason(&attribute.status) {
            out.undecoded.push(Undecoded {
                name: name.to_string(),
                reason,
            });
        }
        return;
    };

    if matches!(value, DescriptorValue::Unset) {
        out.unset.push(name.to_string());
        return;
    }

    let Some(declared) = declared_type(name) else {
        out.outside_schema.push(name.to_string());
        return;
    };

    // 目录几何可以在数字位置上放一条公式，公式压过槽位（`BLTP.Bdiameter` 槽里是
    // 0.0，E3D 印的是 `( 24 )`）。它按文本走，不受 schema 声明的数值类型约束。
    if attribute.status == AttributeExtractionStatus::DecodedExplicit
        && let DescriptorValue::Text(text) = value
    {
        map.insert(
            name.to_string(),
            NamedAttrValue::StringType(normalize_text(text)),
        );
        return;
    }

    if let Some(named) = coerce(value, &declared) {
        map.insert(name.to_string(), named);
        return;
    }

    let conflict = ShapeConflict {
        name: name.to_string(),
        found: shape_of(value),
        declared: declared_name(&declared).to_string(),
    };

    // 空引用、全零字：文件在说「这里什么都没有」。DB 行上它们是空串或干脆没这个键，
    // 两边都是空——不编一个值，记成 unset。
    if is_nothing(value) {
        out.unset.push(name.to_string());
        return;
    }

    // schema 声明文本、文件里是字/整数：DB 读侧同样读不出来，交原样、记一笔。
    if is_text_declaration(&declared)
        && let Some(natural) = natural(value)
    {
        map.insert(name.to_string(), natural);
        out.view_divergence.push(conflict);
        return;
    }

    out.shape_conflicts.push(conflict);
}

/// 声明成文本的类型。数值与引用不在其列——那两类错了会算出错的几何。
fn is_text_declaration(declared: &AttrVal) -> bool {
    matches!(
        declared,
        AttrVal::StringType(_) | AttrVal::StringArrayType(_) | AttrVal::StringHashType(_)
    )
}

/// 文件在说「这里什么都没有」。
///
/// 空引用（`0/0`）与全零字都是未写入的槽位，不是值为零。真库上 `SLOREF` 解出
/// `RefNo(0/0)` 而 DB 是空串，`LCHKDA` 解出 `RawWords([0, 0])` 而 DB 根本没这个键。
fn is_nothing(value: &DescriptorValue) -> bool {
    use DescriptorValue as D;
    match value {
        D::Unset => true,
        D::RefNo(r) => r.word0 == 0 && r.word1 == 0,
        D::RefNoArray(v) => v.iter().all(|r| r.word0 == 0 && r.word1 == 0),
        D::RawWords(v) => v.iter().all(|w| *w == 0),
        D::WordArray(v) => v.iter().all(|w| *w == 0),
        D::IntArray(v) => v.is_empty(),
        D::RealArray(v) => v.is_empty(),
        D::Text(s) => normalize_text(s).is_empty(),
        _ => false,
    }
}

/// 文件自己的形状，不问 schema。只在 [`is_text_declaration`] 那一类上用。
fn natural(value: &DescriptorValue) -> Option<NamedAttrValue> {
    use DescriptorValue as D;
    Some(match value {
        D::Bool(b) => NamedAttrValue::BoolType(*b),
        D::Int(i) => NamedAttrValue::IntegerType(*i),
        D::Real(f) => NamedAttrValue::F32Type(*f as f32),
        D::Text(s) => NamedAttrValue::StringType(normalize_text(s)),
        D::RefNo(r) => NamedAttrValue::RefU64Type(refu64(*r)),
        D::Word { raw, text } => NamedAttrValue::WordType(word_text(*raw, text.as_deref())),
        D::IntArray(v) => NamedAttrValue::IntArrayType(v.clone()),
        D::RawWords(v) => NamedAttrValue::IntArrayType(v.iter().map(|w| *w as i32).collect()),
        D::WordArray(v) => NamedAttrValue::IntArrayType(v.iter().map(|w| *w as i32).collect()),
        D::RealArray(v) => NamedAttrValue::F32VecType(v.iter().map(|x| *x as f32).collect()),
        D::BoolArray(v) => NamedAttrValue::BoolArrayType(v.clone()),
        D::RefNoArray(v) => {
            NamedAttrValue::RefU64Array(v.iter().map(|r| RefnoEnum::from(refu64(*r))).collect())
        }
        D::Unset => return None,
    })
}

/// 没有值时，哪些状态算「本就没有值」，哪些算「该有却没解出来」。
///
/// `NonImplicit` 是「这个属性不在隐式区」，`DefaultRequired` 是「没存过值、也没默认可用」
/// ——两者都不是解码失败。其余没有值的状态都要留名。
fn undecoded_reason(status: &AttributeExtractionStatus) -> Option<String> {
    match status {
        AttributeExtractionStatus::NonImplicit | AttributeExtractionStatus::DefaultRequired => None,
        other => Some(format!("{other:?}")),
    }
}

/// 按 schema 声明的类型把文件里的值转过去。转不过去返回 `None`——调用方记冲突，不猜。
fn coerce(value: &DescriptorValue, declared: &AttrVal) -> Option<NamedAttrValue> {
    use DescriptorValue as D;

    Some(match (declared, value) {
        (AttrVal::IntegerType(_), D::Int(i)) => NamedAttrValue::IntegerType(*i),
        (AttrVal::IntegerType(_), D::Bool(b)) => NamedAttrValue::IntegerType(*b as i32),
        (AttrVal::IntegerType(_), D::Word { raw, .. }) => NamedAttrValue::IntegerType(*raw as i32),

        (AttrVal::BoolType(_), D::Bool(b)) => NamedAttrValue::BoolType(*b),
        (AttrVal::BoolType(_), D::Int(i)) => NamedAttrValue::BoolType(*i != 0),

        (AttrVal::DoubleType(_), D::Real(f)) => NamedAttrValue::F32Type(*f as f32),
        (AttrVal::DoubleType(_), D::Int(i)) => NamedAttrValue::F32Type(*i as f32),

        (AttrVal::StringType(_) | AttrVal::StringHashType(_), D::Text(s)) => {
            NamedAttrValue::StringType(normalize_text(s))
        }
        (AttrVal::StringType(_), D::Word { raw, text }) => {
            NamedAttrValue::StringType(word_text(*raw, text.as_deref()))
        }
        (AttrVal::ElementType(_), D::Text(s)) => NamedAttrValue::ElementType(normalize_text(s)),

        // 词属性：文件存哈希，生成链看字符串。0 是「未设」，反哈希不出来的留原样数字，
        // 不编一个名字（P0 规格 1）。
        (AttrVal::WordType(_), D::Word { raw, text }) => {
            NamedAttrValue::WordType(word_text(*raw, text.as_deref()))
        }
        (AttrVal::WordType(_), D::Int(i)) => NamedAttrValue::WordType(word_text(*i as u32, None)),
        (AttrVal::WordType(_), D::Text(s)) => NamedAttrValue::WordType(normalize_text(s)),

        (AttrVal::Vec3Type(_), D::RealArray(v)) if v.len() >= 3 => {
            NamedAttrValue::Vec3Type(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
        }
        (AttrVal::DoubleArrayType(_), D::RealArray(v)) => {
            NamedAttrValue::F32VecType(v.iter().map(|x| *x as f32).collect())
        }
        (AttrVal::DoubleArrayType(_), D::IntArray(v)) => {
            NamedAttrValue::F32VecType(v.iter().map(|x| *x as f32).collect())
        }

        (AttrVal::IntArrayType(_), D::IntArray(v)) => NamedAttrValue::IntArrayType(v.clone()),
        (AttrVal::IntArrayType(_), D::RawWords(v)) => {
            NamedAttrValue::IntArrayType(v.iter().map(|w| *w as i32).collect())
        }
        (AttrVal::IntArrayType(_), D::WordArray(v)) => {
            NamedAttrValue::IntArrayType(v.iter().map(|w| *w as i32).collect())
        }

        (AttrVal::BoolArrayType(_), D::BoolArray(v)) => NamedAttrValue::BoolArrayType(v.clone()),

        (AttrVal::StringArrayType(_), D::WordArray(v)) => {
            NamedAttrValue::StringArrayType(v.iter().map(|w| word_text(*w, None)).collect())
        }
        (AttrVal::StringArrayType(_), D::IntArray(v)) => {
            NamedAttrValue::StringArrayType(v.iter().map(|w| word_text(*w as u32, None)).collect())
        }

        (AttrVal::RefU64Type(_) | AttrVal::ElementType(_), D::RefNo(r)) => {
            NamedAttrValue::RefU64Type(refu64(*r))
        }
        (AttrVal::RefU64Array(_), D::RefNoArray(v)) => {
            NamedAttrValue::RefU64Array(v.iter().map(|r| RefnoEnum::from(refu64(*r))).collect())
        }
        (AttrVal::RefU64Array(_), D::RefNo(r)) => {
            NamedAttrValue::RefU64Array(vec![RefnoEnum::from(refu64(*r))])
        }

        _ => return None,
    })
}

/// 词哈希 → 词字符串。
///
/// 描述符解码时若已带出文本就用它；否则走 base-27 反哈希，再退到系统属性名表。
/// **0 是「未设」，给空串**（DB 读侧同款）；两条路都反不出来的，交回原始数字的十进制
/// 文本——那是事实，编一个名字不是。
fn word_text(raw: u32, decoded: Option<&str>) -> String {
    if let Some(text) = decoded {
        return normalize_text(text);
    }
    if raw == 0 {
        return String::new();
    }
    if let Some(name) = e3d_attlib::db1_dehash(raw) {
        return name;
    }
    if let Some(name) = e3d_attlib::lookup_system_name(raw) {
        return name.to_string();
    }
    raw.to_string()
}

/// 未设字段的零/遗留字节被 stringify 后，是一串没有任何可见 ASCII 的字符。
/// 语义上就是空，归一为空串以对齐 DB 视图（P0 残差分类第 4 条的处置）。
pub fn normalize_text(text: &str) -> String {
    let trimmed = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.bytes().any(|b| (0x21..=0x7e).contains(&b)) {
        trimmed.to_string()
    } else {
        String::new()
    }
}

fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

fn shape_of(value: &DescriptorValue) -> String {
    use DescriptorValue as D;
    match value {
        D::Unset => "Unset".to_string(),
        D::Bool(_) => "Bool".to_string(),
        D::Int(_) => "Int".to_string(),
        D::Real(_) => "Real".to_string(),
        D::Text(_) => "Text".to_string(),
        D::RefNo(_) => "RefNo".to_string(),
        D::Word { .. } => "Word".to_string(),
        D::IntArray(v) => format!("IntArray[{}]", v.len()),
        D::RealArray(v) => format!("RealArray[{}]", v.len()),
        D::RefNoArray(v) => format!("RefNoArray[{}]", v.len()),
        D::WordArray(v) => format!("WordArray[{}]", v.len()),
        D::BoolArray(v) => format!("BoolArray[{}]", v.len()),
        D::RawWords(v) => format!("RawWords[{}]", v.len()),
    }
}

fn declared_name(declared: &AttrVal) -> &'static str {
    match declared {
        AttrVal::InvalidType => "InvalidType",
        AttrVal::IntegerType(_) => "IntegerType",
        AttrVal::StringType(_) => "StringType",
        AttrVal::DoubleType(_) => "DoubleType",
        AttrVal::DoubleArrayType(_) => "DoubleArrayType",
        AttrVal::StringArrayType(_) => "StringArrayType",
        AttrVal::BoolArrayType(_) => "BoolArrayType",
        AttrVal::IntArrayType(_) => "IntArrayType",
        AttrVal::BoolType(_) => "BoolType",
        AttrVal::Vec3Type(_) => "Vec3Type",
        AttrVal::ElementType(_) => "ElementType",
        AttrVal::WordType(_) => "WordType",
        AttrVal::RefU64Type(_) => "RefU64Type",
        AttrVal::StringHashType(_) => "StringHashType",
        AttrVal::RefU64Array(_) => "RefU64Array",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_of_zero_is_the_empty_string_not_the_name_of_zero() {
        assert_eq!(word_text(0, None), "");
        assert_eq!(word_text(0, Some("")), "");
    }

    #[test]
    fn a_word_hash_reads_back_as_its_name() {
        let hash = e3d_attlib::db1_hash("TUBE");
        assert_eq!(word_text(hash, None), "TUBE");
    }

    /// **改成「反不出来就给空串」会让这条红。** 一个反不出名字的词哈希是一个事实；
    /// 空串是「未设」，是另一个事实。把前者说成后者，消费方永远查不出差别。
    #[test]
    fn a_word_that_dehashes_to_nothing_keeps_its_number() {
        assert_eq!(word_text(1, None), "1");
    }

    /// P0 残差分类第 4 条：未设字段的零/遗留字节。
    #[test]
    fn a_string_of_invisible_bytes_is_empty() {
        assert_eq!(normalize_text("\0"), "");
        assert_eq!(normalize_text("\0\t\u{1}"), "");
        assert_eq!(normalize_text("  BEND \0"), "BEND");
        assert_eq!(normalize_text("A B"), "A B");
    }

    #[test]
    fn a_refno_keeps_both_words_in_order() {
        let refno = refu64(RefNo::new(24_384, 18_447));
        assert_eq!(refno.get_0(), 24_384);
        assert_eq!(refno.get_1(), 18_447);
        assert_eq!(refno.to_string(), "24384_18447");
    }

    /// **形状对不上时返回 `None` 是这个模块的立场。** 一个只有两个数的 `RealArray`
    /// 不是 Vec3，补个零凑成三个就是发明数据。
    #[test]
    fn a_short_real_array_is_not_a_vec3() {
        let short = DescriptorValue::RealArray(vec![1.0, 2.0]);
        assert!(coerce(&short, &AttrVal::Vec3Type([0.0; 3])).is_none());

        let full = DescriptorValue::RealArray(vec![1.0, 2.0, 3.0]);
        assert_eq!(
            coerce(&full, &AttrVal::Vec3Type([0.0; 3])),
            Some(NamedAttrValue::Vec3Type(Vec3::new(1.0, 2.0, 3.0)))
        );
    }

    /// schema 说是词，文件存的是哈希整数——这是 P0 残差分类第 1 条，必须反哈希。
    #[test]
    fn an_integer_under_a_word_schema_is_dehashed() {
        let hash = e3d_attlib::db1_hash("FLOW");
        assert_eq!(
            coerce(
                &DescriptorValue::Int(hash as i32),
                &AttrVal::WordType(String::new())
            ),
            Some(NamedAttrValue::WordType("FLOW".to_string()))
        );
    }
}
