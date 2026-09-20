//! 空间查询的「所属专业」（`spec_value`）派生（plant3d-web ADR 0067；计划
//! `plant3d-web/docs/plans/2026-09-20-spatial-query-room-and-discipline-filter-plan.md` §4.2）。
//!
//! gen-model 的任何表都没有专业这一列。legacy 后端的口径是 **SITE 名关键字**
//! （`plant-model-gen/src/fast_model/export_model/spec_info.rs::site_name_to_spec_value`，沿属主链继承给子孙），
//! 这里按同一规则从候选的属主链上找到 `TYPE = SITE` 的那一级、拿它的 `NAME` 分类——两种数据源对同一构件报同一个专业。
//! 规则表可在 `DbOption.toml` 的 `spec_value_rules` 里改，缺省即 legacy 那张（[`crate::options::default_spec_value_rules`]）。
//!
//! 代价：每级要回元素记录读一次 OWNER（与 `DirectTreeService::name_of` 同代价）。[`SpecMemo`] 逐级记忆：
//! 链上任何一个已算过的祖先都能截断本次爬升、并把这一段路径上的元素一并记下，所以整个进程生命期里每个元素的记录
//! **最多读一次**；记忆按 `DirectTreeService` 的换代键（会话指纹）整体作废。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use aios_core::RefU64;

use crate::options::SpecValueRuleConfig;

/// 属主链上的一级：这一级的 noun / NAME 与它的 OWNER。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageStep {
    pub noun: String,
    pub name: String,
    /// `None` = 到顶了（WORL 之上没有 OWNER，或 OWNER 指向自己 / 非法）。
    pub owner: Option<RefU64>,
}

/// 属主链的取数：生产是 `DirectTreeService::lineage_step`，单测用查表实现。
pub trait Lineage {
    /// 读不出记录回 `None`；此时该元素与它这一段路径按 0「其他」记。
    fn lineage_step(&self, refno: RefU64) -> Option<LineageStep>;
}

impl Lineage for crate::data_interface::direct_tree::DirectTreeService {
    fn lineage_step(&self, refno: RefU64) -> Option<LineageStep> {
        crate::data_interface::direct_tree::DirectTreeService::lineage_step(self, refno)
    }
}

/// 专业值 0：「其他 / 未分类」。
pub const SPEC_VALUE_OTHER: i64 = 0;

/// 属主链最多爬多少级（与 `DirectTreeService::ancestors` 的上限同）。
const MAX_DEPTH: usize = 4096;

/// SITE 名 → 专业值：首个命中的规则生效，关键字大小写不敏感（ASCII 大写比较；中文原样），都不中为 0。
pub fn classify_site_name(rules: &[SpecValueRuleConfig], name: &str) -> i64 {
    let upper = name.trim().to_uppercase();
    if upper.is_empty() {
        return SPEC_VALUE_OTHER;
    }
    rules
        .iter()
        .find(|rule| {
            rule.keywords
                .iter()
                .any(|word| !word.trim().is_empty() && upper.contains(&word.trim().to_uppercase()))
        })
        .map(|rule| rule.value)
        .unwrap_or(SPEC_VALUE_OTHER)
}

fn is_site_noun(noun: &str) -> bool {
    noun.trim().eq_ignore_ascii_case("SITE")
}

/// 一棵属主链上的专业记忆：`refno → spec_value`，链上每个走过的元素都记（SITE 自己也记）。
#[derive(Debug, Default)]
pub struct SpecMemo {
    values: HashMap<RefU64, i64>,
}

impl SpecMemo {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已经记过多少个元素（单测钉「只读一次」用）。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 某元素的专业：先查记忆，没有就沿 OWNER 往上爬到 SITE（或到顶 / 读不出），把整段路径一并记下。
    pub fn spec_value_of(
        &mut self,
        lineage: &dyn Lineage,
        rules: &[SpecValueRuleConfig],
        refno: RefU64,
    ) -> i64 {
        if let Some(value) = self.values.get(&refno) {
            return *value;
        }
        let mut path: Vec<RefU64> = Vec::new();
        let mut current = refno;
        let value = loop {
            if let Some(value) = self.values.get(&current) {
                break *value;
            }
            if path.len() >= MAX_DEPTH || path.contains(&current) {
                break SPEC_VALUE_OTHER;
            }
            path.push(current);
            let Some(step) = lineage.lineage_step(current) else {
                break SPEC_VALUE_OTHER;
            };
            if is_site_noun(&step.noun) {
                break classify_site_name(rules, &step.name);
            }
            match step.owner {
                Some(owner) if owner.0 != 0 && owner != current => current = owner,
                _ => break SPEC_VALUE_OTHER,
            }
        };
        for visited in path {
            self.values.insert(visited, value);
        }
        value
    }
}

/// 进程内共享的记忆 + 它对应的换代键；换代（会话指纹变了）整体清空。
struct SharedMemo {
    epoch: u64,
    rules: Vec<SpecValueRuleConfig>,
    memo: SpecMemo,
}

fn shared_memo() -> &'static Mutex<Option<SharedMemo>> {
    static SLOT: OnceLock<Mutex<Option<SharedMemo>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 生产入口：按 `epoch`（`DirectTreeService` 的会话指纹）取共享记忆答某元素的专业。
/// 规则表在第一次用到时读一次配置（`crate::options::spec_value_rules`），随记忆一起换代。
pub fn spec_value_of_shared(lineage: &dyn Lineage, epoch: u64, refno: RefU64) -> i64 {
    let mut guard = shared_memo().lock().unwrap_or_else(PoisonError::into_inner);
    let stale = guard.as_ref().is_none_or(|shared| shared.epoch != epoch);
    if stale {
        *guard = Some(SharedMemo {
            epoch,
            rules: crate::options::spec_value_rules(),
            memo: SpecMemo::new(),
        });
    }
    let shared = guard.as_mut().expect("shared memo was just initialised");
    let SharedMemo { rules, memo, .. } = shared;
    memo.spec_value_of(lineage, rules, refno)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn rules() -> Vec<SpecValueRuleConfig> {
        crate::options::default_spec_value_rules()
    }

    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_012, seq)
    }

    #[test]
    fn site_names_classify_like_the_legacy_table() {
        let rules = rules();
        assert_eq!(classify_site_name(&rules, "/PIPE-SITE"), 1);
        assert_eq!(classify_site_name(&rules, "/pipe_area"), 1, "大小写不敏感");
        assert_eq!(classify_site_name(&rules, "/ELEC-1"), 2);
        assert_eq!(classify_site_name(&rules, "/DIANQI"), 2);
        assert_eq!(classify_site_name(&rules, "/一号电气房"), 2, "中文关键字原样比较");
        assert_eq!(classify_site_name(&rules, "/INSTR-A"), 3);
        assert_eq!(classify_site_name(&rules, "/HVAC-B"), 4);
        assert_eq!(classify_site_name(&rules, "/CIVIL-01"), 5);
        assert_eq!(classify_site_name(&rules, "/ARCH"), 5);
        assert_eq!(classify_site_name(&rules, "/STRUCT-01"), 6);
        assert_eq!(classify_site_name(&rules, "/STRU"), 6);
        assert_eq!(classify_site_name(&rules, "/SOMETHING"), 0);
        assert_eq!(classify_site_name(&rules, ""), 0);
    }

    #[test]
    fn first_matching_rule_wins_and_custom_rules_replace_the_table() {
        // 同一个名字既含 PIPE 又含 STRU：表里 PIPE 在前，legacy 同样先判 PIPE。
        assert_eq!(classify_site_name(&rules(), "/STRU-PIPE-RACK"), 1);
        let custom = vec![SpecValueRuleConfig {
            value: 9,
            keywords: vec!["RACK".into()],
        }];
        assert_eq!(classify_site_name(&custom, "/STRU-PIPE-RACK"), 9);
        assert_eq!(classify_site_name(&custom, "/PIPE"), 0, "自定义表不带缺省那几条");
    }

    /// 查表实现：`(refno → (noun, name, owner))`，并数一数被读了几次。
    struct TableLineage {
        rows: HashMap<RefU64, LineageStep>,
        reads: Cell<usize>,
    }

    impl TableLineage {
        fn new(rows: Vec<(RefU64, &str, &str, Option<RefU64>)>) -> Self {
            Self {
                rows: rows
                    .into_iter()
                    .map(|(refno, noun, name, owner)| {
                        (
                            refno,
                            LineageStep {
                                noun: noun.to_string(),
                                name: name.to_string(),
                                owner,
                            },
                        )
                    })
                    .collect(),
                reads: Cell::new(0),
            }
        }
    }

    impl Lineage for TableLineage {
        fn lineage_step(&self, refno: RefU64) -> Option<LineageStep> {
            self.reads.set(self.reads.get() + 1);
            self.rows.get(&refno).cloned()
        }
    }

    /// WORL(1) ← SITE /PIPE(2) ← ZONE(3) ← PIPE(4) ← BRAN(5) ← ELBO(6)；另一棵 SITE /STRU(7) ← SCTN(8)。
    fn tree() -> TableLineage {
        TableLineage::new(vec![
            (refno(1), "WORL", "/*", None),
            (refno(2), "SITE", "/PIPE-AREA", Some(refno(1))),
            (refno(3), "ZONE", "/Z1", Some(refno(2))),
            (refno(4), "PIPE", "/P1", Some(refno(3))),
            (refno(5), "BRAN", "/P1/B1", Some(refno(4))),
            (refno(6), "ELBO", "", Some(refno(5))),
            (refno(7), "SITE", "/STRU-01", Some(refno(1))),
            (refno(8), "SCTN", "", Some(refno(7))),
        ])
    }

    #[test]
    fn walks_up_to_the_site_and_memoises_the_whole_path() {
        let lineage = tree();
        let rules = rules();
        let mut memo = SpecMemo::new();
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(6)), 1);
        // ELBO → BRAN → PIPE → ZONE → SITE：五次读，五个元素都记下了（SITE 自己也记）。
        assert_eq!(lineage.reads.get(), 5);
        assert_eq!(memo.len(), 5);
        // 同一棵下的另一个构件：BRAN 已记，只读它自己一次。
        let reads_before = lineage.reads.get();
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(5)), 1);
        assert_eq!(lineage.reads.get(), reads_before, "父已算过，子不再读记录");
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(8)), 6);
        assert_eq!(lineage.reads.get(), reads_before + 2, "SCTN + 它的 SITE 各读一次");
    }

    #[test]
    fn elements_without_a_site_or_without_a_record_fall_back_to_other() {
        let lineage = tree();
        let rules = rules();
        let mut memo = SpecMemo::new();
        // WORL 自己：没有 OWNER、不是 SITE → 0。
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(1)), 0);
        // 表里没有的元素：读不出记录 → 0，且记下来不再读。
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(99)), 0);
        let reads = lineage.reads.get();
        assert_eq!(memo.spec_value_of(&lineage, &rules, refno(99)), 0);
        assert_eq!(lineage.reads.get(), reads);
    }

    #[test]
    fn owner_cycles_terminate_with_other() {
        let lineage = TableLineage::new(vec![
            (refno(10), "ZONE", "/A", Some(refno(11))),
            (refno(11), "ZONE", "/B", Some(refno(10))),
        ]);
        let mut memo = SpecMemo::new();
        assert_eq!(memo.spec_value_of(&lineage, &rules(), refno(10)), 0);
        assert_eq!(memo.len(), 2);
    }
}
