//! 单元不存在的判定（design §1.6，Requirement 6.5）。
//!
//! 单元根不在请求会话的元素索引里时，取该单元「仅自身」版本表（[`UnitVersion`]，链序 = sesno 升序），
//! 按请求会话之前（含）的最后一行分辨原因：
//!
//! - 表为空：refno 在会话链的任何会话里都没有记录 → 404 `REFNO_NOT_IN_CHAIN`。
//! - 请求会话之前（含）没有行：单元还没出现 → 404 `UNIT_NOT_PRESENT`，`reason: "not-yet"`。
//! - 那一行是 `tombstone`：单元已被删 → 404 `UNIT_NOT_PRESENT`，`reason: "deleted"`。
//! - 那一行不是 `tombstone`：版本表说单元在，索引却没有根，数据不一致 → 500 `GEOMETRY_GENERATION_FAILED`。
//!
//! 只按 sesno 比较，不看 Session_Time；不假定输入已排序。

use crate::fast_model::unit_versions::{UnitVersion, UnitVersionImpact};

/// `UNIT_NOT_PRESENT` 的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsentReason {
    /// 请求会话早于单元首次出现的会话。
    NotYet,
    /// 请求会话晚于或等于单元被删除的会话。
    Deleted,
}

impl AbsentReason {
    /// 响应体 `reason` 字段的取值。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotYet => "not-yet",
            Self::Deleted => "deleted",
        }
    }
}

/// 根不在索引时的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsentKind {
    /// 版本表为空。
    RefnoNotInChain,
    /// 单元在该会话下不存在。
    UnitNotPresent(AbsentReason),
    /// 请求会话之前（含）的最后一行不是 tombstone，但索引里没有根。
    Inconsistent {
        /// 那一行的 sesno。
        last_sesno: u32,
        /// 那一行的影响。
        last_impact: UnitVersionImpact,
    },
}

impl AbsentKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RefnoNotInChain => "REFNO_NOT_IN_CHAIN",
            Self::UnitNotPresent(_) => "UNIT_NOT_PRESENT",
            Self::Inconsistent { .. } => "GEOMETRY_GENERATION_FAILED",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::RefnoNotInChain | Self::UnitNotPresent(_) => 404,
            Self::Inconsistent { .. } => 500,
        }
    }

    /// `UNIT_NOT_PRESENT` 的原因；其它结果为 `None`。
    pub fn reason(&self) -> Option<AbsentReason> {
        match self {
            Self::UnitNotPresent(reason) => Some(*reason),
            _ => None,
        }
    }

    /// 给响应体 `message` 用的说明。
    pub fn message(&self, requested_sesno: u32) -> String {
        match self {
            Self::RefnoNotInChain => "会话链中没有该 refno".to_string(),
            Self::UnitNotPresent(AbsentReason::NotYet) => {
                format!("单元在会话 {requested_sesno} 还没有出现")
            }
            Self::UnitNotPresent(AbsentReason::Deleted) => {
                format!("单元在会话 {requested_sesno} 已被删除")
            }
            Self::Inconsistent {
                last_sesno,
                last_impact,
            } => format!(
                "数据不一致：版本表显示单元在会话 {last_sesno}（{last_impact:?}）之后仍存在，\
                 但会话 {requested_sesno} 的元素索引里没有单元根"
            ),
        }
    }
}

/// 根不在请求会话的元素索引时，按单元「仅自身」版本表判定原因。
///
/// `rows` 是该单元的版本表行（链序，旧 → 新）；`requested_sesno` 是解析出的请求会话。
pub fn classify_absent(rows: &[UnitVersion], requested_sesno: u32) -> AbsentKind {
    if rows.is_empty() {
        return AbsentKind::RefnoNotInChain;
    }
    let last = rows
        .iter()
        .filter(|row| row.sesno <= requested_sesno)
        .max_by_key(|row| row.sesno);
    match last {
        None => AbsentKind::UnitNotPresent(AbsentReason::NotYet),
        Some(row) if row.impact_kind == UnitVersionImpact::Tombstone => {
            AbsentKind::UnitNotPresent(AbsentReason::Deleted)
        }
        Some(row) => AbsentKind::Inconsistent {
            last_sesno: row.sesno,
            last_impact: row.impact_kind,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use UnitVersionImpact::*;

    fn rows(spec: &[(u32, UnitVersionImpact)]) -> Vec<UnitVersion> {
        spec.iter()
            .map(|&(sesno, impact_kind)| UnitVersion {
                sesno,
                session_time: None,
                impact_kind,
            })
            .collect()
    }

    #[test]
    fn empty_table_is_refno_not_in_chain() {
        let kind = classify_absent(&[], 5);
        assert_eq!(kind, AbsentKind::RefnoNotInChain);
        assert_eq!(kind.code(), "REFNO_NOT_IN_CHAIN");
        assert_eq!(kind.status(), 404);
        assert_eq!(kind.reason(), None);
    }

    #[test]
    fn before_first_row_is_not_yet() {
        let table = rows(&[(10, Delivery), (12, Mesh)]);
        let kind = classify_absent(&table, 9);
        assert_eq!(kind, AbsentKind::UnitNotPresent(AbsentReason::NotYet));
        assert_eq!(kind.code(), "UNIT_NOT_PRESENT");
        assert_eq!(kind.status(), 404);
        assert_eq!(kind.reason().map(|r| r.as_str()), Some("not-yet"));
    }

    #[test]
    fn at_or_after_tombstone_is_deleted() {
        let table = rows(&[(3, Delivery), (5, Placement), (8, Tombstone)]);
        for sesno in [8, 9, 100] {
            let kind = classify_absent(&table, sesno);
            assert_eq!(kind, AbsentKind::UnitNotPresent(AbsentReason::Deleted));
            assert_eq!(kind.reason().map(|r| r.as_str()), Some("deleted"));
            assert_eq!(kind.status(), 404);
        }
    }

    #[test]
    fn recreated_after_delete_uses_last_row_at_or_before_request() {
        let table = rows(&[(2, Delivery), (4, Tombstone), (7, Delivery), (9, Tombstone)]);
        assert_eq!(
            classify_absent(&table, 5),
            AbsentKind::UnitNotPresent(AbsentReason::Deleted)
        );
        assert_eq!(
            classify_absent(&table, 1),
            AbsentKind::UnitNotPresent(AbsentReason::NotYet)
        );
        assert_eq!(
            classify_absent(&table, 10),
            AbsentKind::UnitNotPresent(AbsentReason::Deleted)
        );
    }

    #[test]
    fn non_tombstone_last_row_is_inconsistent_500() {
        let table = rows(&[(2, Delivery), (4, Tombstone), (7, Delivery), (8, Noop)]);
        let kind = classify_absent(&table, 7);
        assert_eq!(
            kind,
            AbsentKind::Inconsistent {
                last_sesno: 7,
                last_impact: Delivery
            }
        );
        assert_eq!(kind.code(), "GEOMETRY_GENERATION_FAILED");
        assert_eq!(kind.status(), 500);
        assert_eq!(kind.reason(), None);
        assert!(kind.message(7).contains("数据不一致"));

        assert_eq!(
            classify_absent(&table, 20),
            AbsentKind::Inconsistent {
                last_sesno: 8,
                last_impact: Noop
            }
        );
    }

    #[test]
    fn unsorted_input_still_picks_max_sesno_at_or_before() {
        let table = rows(&[(9, Tombstone), (2, Delivery), (5, Mesh)]);
        assert_eq!(
            classify_absent(&table, 6),
            AbsentKind::Inconsistent {
                last_sesno: 5,
                last_impact: Mesh
            }
        );
        assert_eq!(
            classify_absent(&table, 9),
            AbsentKind::UnitNotPresent(AbsentReason::Deleted)
        );
    }

    // Feature: model-version-timeline-scrub, Property 34: 单元不存在的原因判定（后端）。
    mod prop_absent_reason {
        use proptest::prelude::*;

        use super::super::*;

        /// 一条链序版本行序列：sesno 严格递增，影响随机（含 tombstone 与删除后重建）。
        fn chain() -> impl Strategy<Value = Vec<UnitVersionImpact>> {
            (0usize..14).prop_flat_map(|len| {
                prop::collection::vec(
                    prop_oneof![
                        3 => Just(UnitVersionImpact::Delivery),
                        1 => Just(UnitVersionImpact::Mesh),
                        1 => Just(UnitVersionImpact::Placement),
                        1 => Just(UnitVersionImpact::Noop),
                        2 => Just(UnitVersionImpact::Tombstone),
                    ],
                    len,
                )
            })
        }

        /// 参考实现：按 sesno 找请求会话之前（含）的最后一行，再按四种情况分支。
        fn reference(rows: &[UnitVersion], requested: u32) -> AbsentKind {
            if rows.is_empty() {
                return AbsentKind::RefnoNotInChain;
            }
            let mut last: Option<&UnitVersion> = None;
            for row in rows {
                if row.sesno <= requested && last.is_none_or(|current| row.sesno > current.sesno) {
                    last = Some(row);
                }
            }
            match last {
                None => AbsentKind::UnitNotPresent(AbsentReason::NotYet),
                Some(row) if row.impact_kind == UnitVersionImpact::Tombstone => {
                    AbsentKind::UnitNotPresent(AbsentReason::Deleted)
                }
                Some(row) => AbsentKind::Inconsistent {
                    last_sesno: row.sesno,
                    last_impact: row.impact_kind,
                },
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn classify_matches_the_reference_and_its_http_surface(
                impacts in chain(),
                requested in 0u32..20,
            ) {
                let table: Vec<UnitVersion> = impacts
                    .into_iter()
                    .enumerate()
                    .map(|(index, impact_kind)| UnitVersion {
                        sesno: index as u32 * 2 + 1,
                        session_time: None,
                        impact_kind,
                    })
                    .collect();
                let kind = classify_absent(&table, requested);
                prop_assert_eq!(kind, reference(&table, requested));

                // 状态码与错误码与判定一一对应（Requirement 6.5）。
                match kind {
                    AbsentKind::RefnoNotInChain => {
                        prop_assert_eq!(kind.code(), "REFNO_NOT_IN_CHAIN");
                        prop_assert_eq!(kind.status(), 404);
                        prop_assert_eq!(kind.reason(), None);
                    }
                    AbsentKind::UnitNotPresent(AbsentReason::NotYet) => {
                        prop_assert_eq!(kind.code(), "UNIT_NOT_PRESENT");
                        prop_assert_eq!(kind.status(), 404);
                        prop_assert_eq!(kind.reason().map(|r| r.as_str()), Some("not-yet"));
                    }
                    AbsentKind::UnitNotPresent(AbsentReason::Deleted) => {
                        prop_assert_eq!(kind.code(), "UNIT_NOT_PRESENT");
                        prop_assert_eq!(kind.status(), 404);
                        prop_assert_eq!(kind.reason().map(|r| r.as_str()), Some("deleted"));
                    }
                    AbsentKind::Inconsistent { .. } => {
                        prop_assert_eq!(kind.code(), "GEOMETRY_GENERATION_FAILED");
                        prop_assert_eq!(kind.status(), 500);
                        prop_assert_eq!(kind.reason(), None);
                    }
                }
            }

            /// 顺序无关：任意重排输入结论不变（版本表来源不保证有序）。
            #[test]
            fn classify_is_independent_of_input_order(
                impacts in chain(),
                requested in 0u32..20,
                shift in any::<prop::sample::Index>(),
            ) {
                let mut table: Vec<UnitVersion> = impacts
                    .into_iter()
                    .enumerate()
                    .map(|(index, impact_kind)| UnitVersion {
                        sesno: index as u32 * 2 + 1,
                        session_time: None,
                        impact_kind,
                    })
                    .collect();
                let want = classify_absent(&table, requested);
                if !table.is_empty() {
                    let offset = shift.index(table.len());
                    table.rotate_left(offset);
                }
                prop_assert_eq!(classify_absent(&table, requested), want);
            }
        }
    }
}
