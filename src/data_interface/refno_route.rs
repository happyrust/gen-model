//! RefNo → dbnum：**唯一**的解法是位解码（母计划 `2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`
//! 不变量 1；执行切片 `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` W1）。
//!
//! core.dll 里这一步就是一行（E3D 3.1 `sub_5B05260`，经 `DB_Element::dbno` → `db_get_dbno_from_reference`
//! → opcode 58；2.1 `sub_1064EF60:22` 同式）：
//!
//! ```text
//! dbno = word0 & 0x1FFF | (word0 >> 13) & 0x3E000
//! ```
//!
//! `word0` 的 bits 13..25 是 extent 序号，所以同一个库有多个 `word0`（每个 extent 一个）。
//! 这是「`get_0()` 不是 dbnum」那句老注释成立的原因，也是它错误地引出「必须建全局 ref0→dbnum 反查表」
//! 的地方——按位解码就够了，不需要表。本模块之外**不得**再出现第二种从 refno 求 dbnum 的写法。

use aios_core::RefU64;
use e3d_io::refno::RefNo;

/// `RefU64` → e3d-io `RefNo`，两个 word 原样搬，不做任何解释。
#[inline]
pub fn to_refno(refno: RefU64) -> RefNo {
    RefNo::new(refno.get_0(), refno.get_1())
}

/// 合法 refno 解出 dbnum；非法（`word1` 为负、或解出 dbnum 0）给 `None`。
///
/// `None` 不是「库不在 MDB」——那是路由目录的回答（`MdbRouteDirectory::route_refno`）；
/// 这里只回答「这串数字是不是一个能指向某个库的引用」。
#[inline]
pub fn refno_dbnum(refno: RefU64) -> Option<u32> {
    refno_dbnum_of(to_refno(refno))
}

/// 同 [`refno_dbnum`]，输入已是 e3d-io `RefNo`。
#[inline]
pub fn refno_dbnum_of(refno: RefNo) -> Option<u32> {
    refno.is_valid().then(|| refno.dbno())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RefNo::dbno` 的逆（与 `reverse_reference::tests::encode_dbno` 同式）：低 13 位原位，
    /// 高 5 位段左移 13 落到 bits 26..30，中间 bits 13..25 留给 extent。
    fn word0_for(dbno: u32, extent: u32) -> u32 {
        (dbno & 0x1FFF) | ((dbno & 0x3E000) << 13) | (extent << 13)
    }

    /// JEU `jeuglb` 现场样本：`24572/115` 是 dbnum 8188 的第 2 个 extent。
    #[test]
    fn jeu_global_sample_decodes_to_8188() {
        assert_eq!(refno_dbnum(RefU64::from_two_nums(24_572, 115)), Some(8_188));
        assert_eq!(refno_dbnum_of(RefNo::new(24_572, 115)), Some(8_188));
    }

    /// 同一个库的每个 extent 都解到同一个 dbnum——这正是「word0 ≠ dbnum」却不需要反查表的原因。
    #[test]
    fn every_extent_of_one_database_decodes_to_the_same_dbnum() {
        for word0 in [16_191, 24_383, 32_575] {
            assert_eq!(
                refno_dbnum(RefU64::from_two_nums(word0, 1)),
                Some(7_999),
                "word0={word0}"
            );
        }
        // ams251000_0001：dbnum 装不进低 13 位，高位段绕过 extent 段落在 bits 26..30。
        let primary = RefU64::from_two_nums(2_013_279_352, 1);
        let secondary = RefU64::from_two_nums(2_013_279_352 + 0x2000, 1);
        assert_eq!(refno_dbnum(primary), Some(251_000));
        assert_eq!(refno_dbnum(secondary), Some(251_000));
    }

    /// 全部可编码的 dbnum（18 位）与若干 extent 往返一致：解码是编码的逆。
    #[test]
    fn decode_inverts_encode_across_the_whole_dbnum_range() {
        let samples = (1u32..=0x3FFFF)
            .step_by(97)
            .chain([1, 8_188, 8_191, 8_192, 251_000, 0x3FFFF]);
        for dbno in samples {
            for extent in [1u32, 2, 3, 0x1FFF] {
                let refno = RefU64::from_two_nums(word0_for(dbno, extent), 7);
                assert_eq!(
                    refno_dbnum(refno),
                    Some(dbno),
                    "dbno={dbno} extent={extent}"
                );
            }
        }
    }

    /// 非法引用给 `None`，不给 0，也不 panic：`word1` 符号位置位（Nulref 家族）与解出 dbnum 0 的空槽。
    #[test]
    fn invalid_references_decode_to_none() {
        assert_eq!(refno_dbnum(RefU64::from_two_nums(0, 0)), None);
        assert_eq!(
            refno_dbnum(RefU64::from_two_nums(24_572, 0x8000_0000)),
            None
        );
        // extent 位有值、dbnum 位全零：仍是 dbnum 0，仍非法。
        assert_eq!(refno_dbnum(RefU64::from_two_nums(3 << 13, 5)), None);
        // `word0` 的 bit 31 不参与 dbnum：它置位不会让解码失败，也不会污染结果。
        assert_eq!(
            refno_dbnum(RefU64::from_two_nums(0x8000_0000 | 8_188, 5)),
            Some(8_188)
        );
    }

    /// `to_refno` 不解释、不归一：两个 word 原样过去，负 `word1` 也原样保留。
    #[test]
    fn to_refno_copies_both_words_verbatim() {
        let refno = to_refno(RefU64::from_two_nums(24_572, 0x8000_0000));
        assert_eq!(refno.word0, 24_572);
        assert_eq!(refno.word1, 0x8000_0000);
        assert!(!refno.is_valid());
    }
}
