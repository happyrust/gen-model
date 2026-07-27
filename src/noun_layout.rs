//! 隐含块寻址：把活 E3D 导出的 noun 有序属性表折算成 `AttrInfo.offset`。
//!
//! 见 ADR-008。`AttrInfo.offset` 是个打包字段：低 20 位是字偏移，高位是 bit 下标
//! （只对 BOOL 非零）——这正是 `parse_implicit_attr_value` 已经在吃的编码，所以
//! 本模块只负责生成偏移表，不需要动解析器。
//!
//! 【重要约束】“哪些属性占槽”无法从属性字典判定（ADR-008 已系统排除），因此
//! [`compute_offsets`] 把占槽集作为**显式入参**：已知 noun 用快照的占槽集，未知 noun
//! 需先由实测 `impl_len` 反解（目前交叉验证仅 66%，尚不可直接用于生产）。

use std::collections::HashSet;

use aios_core::AttrVal;
use aios_core::pdms_types::{AttrInfo, DbAttributeType};
use serde::Deserialize;

/// 隐含块起始字：word0=长度、word1-2=refno、word3=type、word4-5=owner，之后才是属性。
pub const IMPLICIT_BLOCK_START_WORD: u32 = 11;
/// `offset` 里 bit 下标的移位，与解析器的 `attr_info.offset >> 0x14` 对应。
pub const BIT_INDEX_SHIFT: u32 = 20;
pub const WORD_OFFSET_MASK: u32 = 0x000F_FFFF;

/// 一个属性在 `noun_layout.json` 里的原始记录。
#[derive(Debug, Clone, Deserialize)]
pub struct LayoutAttr {
    pub name: String,
    pub hash: i32,
    #[serde(rename = "type")]
    pub type_code: i32,
    #[serde(rename = "typeName")]
    pub type_name: String,
    #[serde(rename = "isArray")]
    pub is_array: bool,
    #[serde(rename = "maxSize")]
    pub max_size: i32,
    #[serde(rename = "trueSize")]
    pub true_size: i32,
    #[serde(rename = "isUda", default)]
    pub is_uda: bool,
    #[serde(rename = "isPseudo", default)]
    pub is_pseudo: bool,
}

/// 一个 noun 的导出记录。属性顺序就是 `DbElementType.SystemAttributes()` 的返回顺序。
#[derive(Debug, Clone, Deserialize)]
pub struct LayoutNoun {
    pub noun: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub hard: String,
    #[serde(rename = "dbTypes", default)]
    pub db_types: Vec<i32>,
    pub attrs: Vec<LayoutAttr>,
}

/// 一个标量值占几个字。数组额外带一个前置长度字。
fn scalar_unit(type_code: i32) -> Option<u32> {
    match type_code {
        1 | 6 => Some(1),             // INTEGER / WORD
        2 | 5 | 7 | 8 | 9 => Some(2), // DOUBLE / ELEMENT / DIRECTION / POSITION / ORIENTATION
        _ => None,
    }
}

/// STRING 槽不是声明长度的函数：目录几何那批（PTDI/PBDI/PDIA/PAXI…）存的是编译
/// 后的表达式，定长字符串才遵循 `1 + ceil(n/4)`。前者只能实测。
fn string_words(a: &LayoutAttr) -> Option<u32> {
    match (a.max_size, a.true_size) {
        (1, 0) => Some(1),
        (30, 0) => Some(3),
        (120, 0) => Some(3),
        (1000, 0) => Some(2),
        (1000, 1) => Some(5),
        (m, t) if t > 0 && t == m => Some(1 + (t as u32).div_ceil(4)),
        _ => None,
    }
}

/// 该属性在隐含块里占几个字。BOOL 返回 1，但整个 BOOL 游共用这一个字。
pub fn word_len(a: &LayoutAttr) -> Option<u32> {
    match a.type_code {
        3 => Some(1),
        4 => string_words(a),
        code => {
            let unit = scalar_unit(code)?;
            Some(if a.is_array {
                1 + a.max_size.max(0) as u32 * unit
            } else {
                unit
            })
        }
    }
}

/// 按 ADR-008 的布局规则算出每个占槽属性的 `(hash, 打包 offset)`。
///
/// `slotted` 是占槽属性的 hash 集合；不在其中的属性直接跳过、不推进偏移。
/// 遇到字长未知的属性会报错而不是猜：布局错一个字，后面整块全错。
pub fn compute_offsets(
    attrs: &[LayoutAttr],
    slotted: &HashSet<i32>,
) -> Result<Vec<(i32, u32)>, String> {
    let mut out = Vec::with_capacity(slotted.len());
    let mut cursor = IMPLICIT_BLOCK_START_WORD;
    // 该 noun 的所有 BOOL 塌缩进这一个字，位置由第一个 BOOL 出现处决定。
    let mut bool_word: Option<u32> = None;
    let mut bit_index: u32 = 0;

    for a in attrs {
        if !slotted.contains(&a.hash) {
            continue;
        }
        if a.type_code == 3 {
            let word = *bool_word.get_or_insert_with(|| {
                let w = cursor;
                cursor += 1;
                w
            });
            out.push((a.hash, (bit_index << BIT_INDEX_SHIFT) | word));
            bit_index += 1;
            continue;
        }
        let w =
            word_len(a).ok_or_else(|| format!("attribute {} has no known word length", a.name))?;
        out.push((a.hash, cursor));
        cursor += w;
    }
    Ok(out)
}

/// 一个 noun 的隐含块总字长，即元素记录 word0 应该等于的值。
/// 用实测 `impl_len` 反验占槽集时靠它。
pub fn implicit_len(attrs: &[LayoutAttr], slotted: &HashSet<i32>) -> Result<u32, String> {
    let mut cursor = IMPLICIT_BLOCK_START_WORD;
    let mut saw_bool = false;
    for a in attrs {
        if !slotted.contains(&a.hash) {
            continue;
        }
        if a.type_code == 3 {
            if !saw_bool {
                saw_bool = true;
                cursor += 1;
            }
            continue;
        }
        cursor +=
            word_len(a).ok_or_else(|| format!("attribute {} has no known word length", a.name))?;
    }
    Ok(cursor)
}

fn att_type_of(code: i32) -> DbAttributeType {
    match code {
        1 => DbAttributeType::INTEGER,
        2 => DbAttributeType::DOUBLE,
        3 => DbAttributeType::BOOL,
        4 => DbAttributeType::STRING,
        5 => DbAttributeType::ELEMENT,
        6 => DbAttributeType::WORD,
        7 => DbAttributeType::DIRECTION,
        8 => DbAttributeType::POSITION,
        9 => DbAttributeType::ORIENTATION,
        10 => DbAttributeType::DATETIME,
        _ => DbAttributeType::Unknown,
    }
}

/// `parse_implicit_attr_value` 是按 `default_val` 的**变体**分派的（不是 `att_type`），
/// 所以这里必须给对应变体的零值，否则会按错误的分支读。
fn default_val_of(a: &LayoutAttr) -> AttrVal {
    match a.type_code {
        1 => AttrVal::IntegerType(0),
        2 => AttrVal::DoubleType(0.0),
        3 => AttrVal::BoolType(false),
        4 => AttrVal::StringType(String::new()),
        5 => AttrVal::RefU64Type(Default::default()),
        6 => AttrVal::WordType(String::new()),
        7 | 8 | 9 => AttrVal::Vec3Type([0.0; 3]),
        _ => AttrVal::InvalidType,
    }
}

/// 把一个 noun 的占槽属性折成 `AttrInfo` 列表，可直接填进 `PdmsDatabaseInfo`。
pub fn to_attr_infos(noun: &LayoutNoun, slotted: &HashSet<i32>) -> Result<Vec<AttrInfo>, String> {
    let offsets = compute_offsets(&noun.attrs, slotted)?;
    let by_hash: std::collections::HashMap<i32, u32> = offsets.into_iter().collect();
    Ok(noun
        .attrs
        .iter()
        .filter_map(|a| {
            by_hash.get(&a.hash).map(|off| AttrInfo {
                name: a.name.clone(),
                hash: a.hash,
                offset: *off,
                default_val: default_val_of(a),
                att_type: att_type_of(a.type_code),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(name: &str, hash: i32, code: i32, is_array: bool, max_size: i32) -> LayoutAttr {
        LayoutAttr {
            name: name.into(),
            hash,
            type_code: code,
            type_name: String::new(),
            is_array,
            max_size,
            true_size: 0,
            is_uda: false,
            is_pseudo: false,
        }
    }

    /// CYLINDER 是 ADR-008 里端到端跑通的例子：快照给的 offset 是
    /// POS 11 / ORI 18 / LEVE 25 / OBST 28 / DIAM 29 / HEIG 31。
    #[test]
    fn cylinder_offsets_match_snapshot() {
        let attrs = vec![
            attr("POS", 545713, 8, true, 3),
            attr("ORI", 538503, 9, true, 3),
            attr("LEVE", 646041, 1, true, 2),
            attr("OBST", 939021, 1, false, 1),
            attr("DIAM", 788296, 2, false, 1),
            attr("HEIG", 675926, 2, false, 1),
        ];
        let slotted: HashSet<i32> = attrs.iter().map(|a| a.hash).collect();
        let got = compute_offsets(&attrs, &slotted).unwrap();
        let want = vec![
            (545713, 11),
            (538503, 18),
            (646041, 25),
            (939021, 28),
            (788296, 29),
            (675926, 31),
        ];
        assert_eq!(got, want);
        // 快照漏了末尾的 ORRF，所以实测 impl_len 是 35 而不是 33；这里只算到 HEIG。
        assert_eq!(implicit_len(&attrs, &slotted).unwrap(), 33);
    }

    /// 一个 noun 的所有 BOOL 共用一个字，即使它们在声明序列里被其它属性隔开；
    /// bit 下标按出现次序递增。HACC 就是这个形状。
    #[test]
    fn bools_collapse_into_one_word_with_rising_bit_index() {
        let attrs = vec![
            attr("POS", 1, 8, true, 3),   // 11..18
            attr("BUIL", 2, 3, false, 1), // bool word = 18
            attr("SPRE", 3, 5, false, 1), // 19..21
            attr("ORIF", 4, 3, false, 1), // 仍在 18，bit 1
            attr("ARRI", 5, 1, false, 1), // 21
            attr("LEND", 6, 3, false, 1), // 仍在 18，bit 2
        ];
        let slotted: HashSet<i32> = attrs.iter().map(|a| a.hash).collect();
        let got = compute_offsets(&attrs, &slotted).unwrap();
        assert_eq!(got[0], (1, 11));
        assert_eq!(got[1], (2, 18));
        assert_eq!(got[2], (3, 19));
        assert_eq!(got[3], (4, (1 << BIT_INDEX_SHIFT) | 18));
        assert_eq!(got[4], (5, 21));
        assert_eq!(got[5], (6, (2 << BIT_INDEX_SHIFT) | 18));
        assert_eq!(implicit_len(&attrs, &slotted).unwrap(), 22);
    }

    /// 不占槽的属性不能推进偏移：CYLINDER 的 NAME/PURP 就是这种。
    #[test]
    fn unslotted_attributes_do_not_advance_the_cursor() {
        let attrs = vec![
            attr("NAME", 1, 4, false, 500),
            attr("PURP", 2, 6, false, 1),
            attr("POS", 3, 8, true, 3),
        ];
        let slotted: HashSet<i32> = [3].into_iter().collect();
        assert_eq!(compute_offsets(&attrs, &slotted).unwrap(), vec![(3, 11)]);
    }

    /// 字长未知必须报错，不能静默算下去。
    #[test]
    fn unknown_word_length_is_an_error() {
        let attrs = vec![attr("WEIRD", 1, 4, false, 777)];
        let slotted: HashSet<i32> = [1].into_iter().collect();
        assert!(compute_offsets(&attrs, &slotted).is_err());
    }
}
