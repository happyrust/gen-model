//! SCOSEL 规则的纯函数求值器：只回答「这条规则命中了哪些元素」。
//!
//! 分工（`architecture.md` §2 的核心判断）：求值器把命中的元素交出去，子树展开与生成根解析
//! 由现有机制自动继承。所以这里**不碰 store、不碰文件系统、不知道库和 MDB 的概念**——
//! 「求值域是 MDB 全量跨库」是 [`ScoselScope`] 实现方的事。
//!
//! 语义全部来自真实字节，不来自任何文档叙述（`library/scosel-semantics.md` §5 的四处纠偏）：
//!
//! * `StrEq(Attr{hash: DESC}, Text(s))` 是**全串相等**（`402`），不是子串。`AREA01` 子串命中
//!   20、全串 0；`PIPE` 子串 301、全串 0。
//! * 属性只有 `DESC`（604480）认。27 条真实样本的 `106,4,<hash>` 全是它；按 `NAME` 查全库
//!   命中 0。别的 hash 一律记 [`EvalIssueKind::UnsupportedAttribute`]，绝不静默返回空。
//! * `Attr.qualifiers` 里的 `Typed{words}` 是 **noun 限定**（`1602` 的 `640493` 是 `ZONE`，
//!   不是属性），形状 `[2, <noun_hash>, <类型码>, 1701]`——类型码在**倒数第二**字，末字
//!   `1701` 是标记。全库没有任何元素的属性叫 ZONE，按属性筛选必然查空。
//! * `301` 是 OR（并集）、`302` 是 AND（交集）。
//!
//! 两处保守取值，都是「宁可漏命中，不可错命中」，并且**都不静默**：
//!
//! 1. **区分大小写**。真实语料的 DESC 全大写、无大小写混合样本，E3D 的真实口径无法判定；
//!    首版不做任何大小写折叠，把原文原样交给 scope（契约 VAL-EVAL-009 钉住这个决定）。
//! 2. **认不出的形状返回空集 + 记 issue**。`Before`、非 DESC 属性、非 noun 限定、hunk 值，
//!    一律不进 members。空集是「这条规则没算出东西」，`unsupported` 才是「为什么没算出」——
//!    上层据此出 `scosel_unsupported` 原因码，不会把「算不出来」误报成「命中 0 个」。
//!
//! **`ScoselRule.attr` 不参与筛选**：它是规则组的 noun hash（`640493` = ZONE），与
//! `Attr.qualifiers` 里的 noun 限定在 27 条真实样本里逐条相同。不拿它再取一次交，是因为
//! 那样一旦 scope 对某个 noun 没有索引，结果会被静默清零——正是本 mission 要消灭的失败模式。
//! 求值域由 `Attr.qualifiers` 里的 noun 限定表达（`ALL ZONE WITH (...)` 的 ZONE 就在那里）。

use std::collections::HashSet;

use aios_core::RefU64;

use super::scosel::{ScoselExpr, ScoselQualifier, ScoselRule, OP_QUALIFIER_TYPE};

/// `Attr.qualifiers` 里表示 noun 限定的类型码（`Typed.words` 的倒数第二字）。
///
/// `6` 的取证：27 条真实样本的 `1602,4,2,<noun>,6,1701` 全是 `6`，noun 分别是
/// `640493`（ZONE，24 条）与 `67635315`（REFGRD，3 条）。
const QUALIFIER_NOUN_TYPE: i32 = 6;

/// 规则里唯一被求值器认得的匹配属性：`DESC`。
///
/// 用 `db1_hash` 现算而不是写死 `604_480`：hash 表的权威来源是 attlib，写死一个数字就等于
/// 在代码里放第二份真相。单测 `the_desc_hash_agrees_with_the_known_hash_table` 钉住它等于 604480。
pub fn desc_attribute_hash() -> u32 {
    e3d_attlib::db1_hash("DESC")
}

/// 求值要问的两个问题。实现方（M2 的 `DirectTreeService::scosel_scope`）负责
/// 「求值域 = MDB 全量跨库」这件事；求值器自己不知道库的概念。
pub trait ScoselScope {
    /// 全 MDB 范围内，属性 `attr_hash` 的文本值**全串等于** `text` 的元素。
    ///
    /// 空索引、未知属性 hash、原文不在表里，三种都返回空集，不 panic、不报错——
    /// 一条规则不该把请求打挂（`DbIndexes::refnos_of_attr_text` 的既有口径）。
    fn refnos_with_attr_text(&self, attr_hash: u32, text: &str) -> HashSet<RefU64>;

    /// 全 MDB 范围内，noun 为该 hash 的全部元素（用于 noun 限定取交）。
    fn refnos_of_noun(&self, noun_hash: u32) -> HashSet<RefU64>;
}

/// 求值不认得的形状。不静默跳过，全部记下来供上层出原因码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalIssue {
    pub kind: EvalIssueKind,
    /// 人可读的附加信息，进日志与原因码细节（哪个 hash、哪个 opcode、限定段的原文）。
    pub detail: String,
}

/// [`EvalIssue`] 的分类。四类对应四种「算不出来」，上层可以各自出不同的原因码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalIssueKind {
    /// `Attr.hash` 不是 `DESC`（604480）。
    UnsupportedAttribute,
    /// 限定形状认不出：`Typed.words` 的类型码不是 noun（`6`），或 `Value{..}`。
    UnsupportedQualifier,
    /// 操作符/操作数形状算不了：`Before(..)`、裸 `Attr` / 裸 `Text` 当条件。
    UnsupportedOp,
    /// 属性值是 hunk（`RawWords`），原文不在 AST 里，做不了全串相等比较。
    HunkValue,
}

/// 一次求值的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutcome {
    /// 命中元素，按 [`RefU64`] 位模式升序——同一规则在同样数据上永远给同一顺序
    /// （快照测试与 E3D 对照都依赖这点）。
    pub members: Vec<RefU64>,
    /// 非空表示「这条规则有部分没算」：`members` 可能为空，也可能只是缺了一块。
    pub unsupported: Vec<EvalIssue>,
}

/// 入口。`rule` 是 `scosel::decode` 的产物。
pub fn evaluate(rule: &ScoselRule, scope: &dyn ScoselScope) -> EvalOutcome {
    let mut unsupported = Vec::new();
    let hits = eval_expr(&rule.filter, scope, &mut unsupported);
    let mut members: Vec<RefU64> = hits.into_iter().collect();
    // `RefU64` 的 `Ord` 就是内层 u64 的序，但这里显式按键排序，把「按位模式」写明白。
    members.sort_unstable_by_key(|refno| refno.0);
    EvalOutcome {
        members,
        unsupported,
    }
}

/// 表达式求值。返回命中集；算不了的形状记进 `issues` 并给空集。
///
/// 组合语义按集合算：`Or` 并集、`And` 交集。子树出 issue 时它自己回空集，
/// 于是 `Or` 会保留另一侧的结果（部分命中），`And` 会归空（一侧算不出来就不能断言同时满足）。
fn eval_expr(
    expr: &ScoselExpr,
    scope: &dyn ScoselScope,
    issues: &mut Vec<EvalIssue>,
) -> HashSet<RefU64> {
    match expr {
        ScoselExpr::StrEq(lhs, rhs) => eval_str_eq(lhs, rhs, scope, issues),
        ScoselExpr::Or(lhs, rhs) => {
            let mut out = eval_expr(lhs, scope, issues);
            out.extend(eval_expr(rhs, scope, issues));
            out
        }
        ScoselExpr::And(lhs, rhs) => {
            let left = eval_expr(lhs, scope, issues);
            let right = eval_expr(rhs, scope, issues);
            left.intersection(&right).copied().collect()
        }
        ScoselExpr::Before(lhs, rhs) => {
            // 27 条真实样本里一个都没有（它们是等值分组，不是前缀分组），首版不实现求值。
            issues.push(EvalIssue {
                kind: EvalIssueKind::UnsupportedOp,
                detail: format!(
                    "Before(..) 未实现求值：{} vs {}",
                    describe(lhs),
                    describe(rhs)
                ),
            });
            HashSet::new()
        }
        // 裸操作数当条件：没有比较算子，无从判断命中。
        other => {
            issues.push(EvalIssue {
                kind: EvalIssueKind::UnsupportedOp,
                detail: format!("条件位置上出现裸操作数：{}", describe(other)),
            });
            HashSet::new()
        }
    }
}

/// `StrEq` 的求值：文本全串相等 + 限定取交。
///
/// 27 条真实样本的形态是 `Attr{hash: DESC, qualifiers: [Typed{...}]} == Text(s)`，两侧顺序
/// 在后缀写法里可能反过来，两种都认。
fn eval_str_eq(
    lhs: &ScoselExpr,
    rhs: &ScoselExpr,
    scope: &dyn ScoselScope,
    issues: &mut Vec<EvalIssue>,
) -> HashSet<RefU64> {
    let (attr, text) = match (lhs, rhs) {
        (ScoselExpr::Attr { .. }, ScoselExpr::Text(text)) => (lhs, text.as_str()),
        (ScoselExpr::Text(text), ScoselExpr::Attr { .. }) => (rhs, text.as_str()),
        // 两侧都不是「属性 vs 文本」：hunk 值就是这种形态——`DESC` 超过 120 字符时原文落在
        // hunk pages 里，`decode` 不产出 `Text`，这里做不了全串相等比较。用户决策是
        // 「不静默，当前语料用不到」，所以记 `HunkValue` 而不是当成空条件。
        _ => {
            issues.push(EvalIssue {
                kind: EvalIssueKind::HunkValue,
                detail: format!(
                    "等值比较的两侧不是「属性 vs 文本」：{} vs {}",
                    describe(lhs),
                    describe(rhs)
                ),
            });
            return HashSet::new();
        }
    };

    let ScoselExpr::Attr {
        hash, qualifiers, ..
    } = attr
    else {
        // 上面的 match 已经保证 attr 是 Attr，这里只是拿不到引用的兜底。
        return HashSet::new();
    };

    let Ok(hash) = u32::try_from(*hash) else {
        issues.push(EvalIssue {
            kind: EvalIssueKind::UnsupportedAttribute,
            detail: format!("属性 hash {hash} 不是合法的 u32"),
        });
        return HashSet::new();
    };

    let desc = desc_attribute_hash();
    if hash != desc {
        issues.push(EvalIssue {
            kind: EvalIssueKind::UnsupportedAttribute,
            detail: format!("属性 hash {hash} 不是 DESC（{desc}），首版只认 DESC"),
        });
        return HashSet::new();
    }

    let mut hits = scope.refnos_with_attr_text(hash, text);
    // 限定段全部认得出，才允许把结果交出去：认不出的限定意味着「还有一条限制没生效」，
    // 交出去就是错命中。与「非 DESC 属性返回空集」同一口径。
    let mut fully_understood = true;
    for qualifier in qualifiers {
        match qualifier {
            ScoselQualifier::Typed { words } => match noun_of_typed(words) {
                Some(noun) => {
                    let domain = scope.refnos_of_noun(noun);
                    hits.retain(|refno| domain.contains(refno));
                }
                None => {
                    issues.push(EvalIssue {
                        kind: EvalIssueKind::UnsupportedQualifier,
                        detail: format!("1602 限定段认不出 noun 限定：{words:?}"),
                    });
                    fully_understood = false;
                }
            },
            ScoselQualifier::Value { value } => {
                // 27 条真实样本里从不出现（`120,0` 被 `106` 的 4 字 body 吞进了 `Attr.tail`）。
                issues.push(EvalIssue {
                    kind: EvalIssueKind::UnsupportedQualifier,
                    detail: format!("120 限定段（value={value}）未实现求值"),
                });
                fully_understood = false;
            }
        }
    }

    if fully_understood {
        hits
    } else {
        HashSet::new()
    }
}

/// `Typed.words` → noun hash。形状 `[2, <noun_hash>, <类型码>, 1701]`。
///
/// 类型码在**倒数第二**字，末字 `1701`（`OP_QUALIFIER_TYPE`）是标记——按「末字 = 类型码」
/// 读会把 `1701` 当成类型，于是 27 条真实样本全部判成认不出（`library/scosel-semantics.md` §3）。
/// `words[0]` 的 `2` 含义未查清，原样留着不解读。
fn noun_of_typed(words: &[i32]) -> Option<u32> {
    let len = words.len();
    if len < 3 {
        return None;
    }
    if words[len - 1] != OP_QUALIFIER_TYPE {
        return None;
    }
    if words[len - 2] != QUALIFIER_NOUN_TYPE {
        return None;
    }
    u32::try_from(words[len - 3]).ok()
}

/// 把 AST 节点印成一句人可读的描述，进 issue 的 detail（排查时不用回看原始字节）。
fn describe(expr: &ScoselExpr) -> String {
    match expr {
        ScoselExpr::Attr {
            hash, qualifiers, ..
        } => {
            format!("Attr(hash={hash}, qualifiers={})", qualifiers.len())
        }
        ScoselExpr::Text(text) => format!("Text({text:?})"),
        ScoselExpr::Before(..) => "Before(..)".to_string(),
        ScoselExpr::StrEq(..) => "StrEq(..)".to_string(),
        ScoselExpr::Or(..) => "Or(..)".to_string(),
        ScoselExpr::And(..) => "And(..)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;
    use crate::data_interface::scosel;

    /// 真库 hash 表（`library/scosel-semantics.md` §4）。
    const ZONE: u32 = 640_493;
    const REFGRD: u32 = 67_635_315;

    /// 夹具 scope：两张表 + 一份查询记录。
    ///
    /// 记录是给「大小写原样透传」这条断言用的：求值器不该替 scope 做任何规范化，
    /// 所以测试要能看到它到底把什么原文问了出去。
    #[derive(Default)]
    struct FixtureScope {
        texts: HashMap<(u32, String), HashSet<RefU64>>,
        nouns: HashMap<u32, HashSet<RefU64>>,
        asked: RefCell<Vec<(u32, String)>>,
    }

    impl FixtureScope {
        fn with_text(mut self, text: &str, refnos: &[u64]) -> Self {
            self.texts.insert(
                (desc_attribute_hash(), text.to_string()),
                refnos.iter().copied().map(RefU64).collect(),
            );
            self
        }

        fn with_noun(mut self, noun: u32, refnos: &[u64]) -> Self {
            self.nouns
                .insert(noun, refnos.iter().copied().map(RefU64).collect());
            self
        }
    }

    impl ScoselScope for FixtureScope {
        fn refnos_with_attr_text(&self, attr_hash: u32, text: &str) -> HashSet<RefU64> {
            self.asked.borrow_mut().push((attr_hash, text.to_string()));
            self.texts
                .get(&(attr_hash, text.to_string()))
                .cloned()
                .unwrap_or_default()
        }

        fn refnos_of_noun(&self, noun_hash: u32) -> HashSet<RefU64> {
            self.nouns.get(&noun_hash).cloned().unwrap_or_default()
        }
    }

    /// `1602,4,2,<noun>,6,1701` 的 AST 形态。
    fn noun_qualifier(noun: u32) -> ScoselQualifier {
        ScoselQualifier::Typed {
            words: vec![2, noun as i32, QUALIFIER_NOUN_TYPE, OP_QUALIFIER_TYPE],
        }
    }

    fn typed_qualifier(words: Vec<i32>) -> ScoselQualifier {
        ScoselQualifier::Typed { words }
    }

    /// `StrEq(Attr{hash, qualifiers}, Text(text))`。
    fn str_eq(hash: u32, qualifiers: Vec<ScoselQualifier>, text: &str) -> ScoselExpr {
        ScoselExpr::StrEq(
            Box::new(ScoselExpr::Attr {
                hash: hash as i32,
                tail: [1, 120, 0],
                qualifiers,
            }),
            Box::new(ScoselExpr::Text(text.to_string())),
        )
    }

    /// `/AREA01_PIPE` 形状：`ALL ZONE WITH (ATTRIB DESC OF ZONE EQ 'AREA01PIPE')`。
    fn desc_rule(noun: u32, text: &str) -> ScoselRule {
        ScoselRule {
            attr: noun as i32,
            filter: str_eq(desc_attribute_hash(), vec![noun_qualifier(noun)], text),
        }
    }

    fn kinds(outcome: &EvalOutcome) -> Vec<EvalIssueKind> {
        outcome.unsupported.iter().map(|issue| issue.kind).collect()
    }

    /// VAL-EVAL-001：单条「属性全串相等 + noun 限定」求值 = 两次查表的交集。
    ///
    /// 夹具里 3 个元素 DESC=s，其中 2 个是 ZONE：求值只回这 2 个。
    #[test]
    fn a_desc_text_match_intersects_with_the_noun_qualifier() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11, 22, 33])
            .with_noun(ZONE, &[11, 22, 99]);

        let outcome = evaluate(&desc_rule(ZONE, "AREA01PIPE"), &scope);

        assert_eq!(
            outcome.members,
            vec![RefU64(11), RefU64(22)],
            "DESC 命中的 3 个里只有 2 个是 ZONE，交集应只剩这 2 个"
        );
        assert!(
            outcome.unsupported.is_empty(),
            "27 条真实样本的形状不该产生 unsupported：{:?}",
            outcome.unsupported
        );
    }

    /// VAL-EVAL-002：`Or` 求并集，并去重。
    #[test]
    fn or_takes_the_union_without_duplicates() {
        let scope = FixtureScope::default()
            .with_text("AREA01GRID", &[1, 2])
            .with_text("PLANTGRID", &[2, 3])
            .with_noun(REFGRD, &[1, 2, 3]);
        let rule = ScoselRule {
            attr: REFGRD as i32,
            filter: ScoselExpr::Or(
                Box::new(str_eq(
                    desc_attribute_hash(),
                    vec![noun_qualifier(REFGRD)],
                    "AREA01GRID",
                )),
                Box::new(str_eq(
                    desc_attribute_hash(),
                    vec![noun_qualifier(REFGRD)],
                    "PLANTGRID",
                )),
            ),
        };

        let outcome = evaluate(&rule, &scope);

        assert_eq!(
            outcome.members,
            vec![RefU64(1), RefU64(2), RefU64(3)],
            "并集要含两侧全部元素，且 2 只出现一次"
        );
        assert!(outcome.unsupported.is_empty());
    }

    /// VAL-EVAL-003：`And` 求交集。
    #[test]
    fn and_takes_the_intersection_of_both_sides() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[1, 2, 3])
            .with_text("AREA02PIPE", &[2, 3, 4])
            .with_noun(ZONE, &[1, 2, 3, 4]);
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: ScoselExpr::And(
                Box::new(str_eq(
                    desc_attribute_hash(),
                    vec![noun_qualifier(ZONE)],
                    "AREA01PIPE",
                )),
                Box::new(str_eq(
                    desc_attribute_hash(),
                    vec![noun_qualifier(ZONE)],
                    "AREA02PIPE",
                )),
            ),
        };

        let outcome = evaluate(&rule, &scope);

        assert_eq!(
            outcome.members,
            vec![RefU64(2), RefU64(3)],
            "交集应恰为两侧重叠的 2、3"
        );
        assert!(outcome.unsupported.is_empty());
    }

    /// VAL-EVAL-004：非 `DESC` 属性进 `unsupported`，绝不静默返回空。
    ///
    /// 这是本 mission 最在意的一条：`NAME`（639374）在真库全库命中 0，如果实现按属性名猜错，
    /// 规则组就会永远空转且没人知道。所以「空 members」必须伴随一条 issue。
    #[test]
    fn an_attribute_other_than_desc_is_recorded_as_unsupported() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11])
            .with_noun(ZONE, &[11]);
        let name = e3d_attlib::db1_hash("NAME");
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: str_eq(name, vec![noun_qualifier(ZONE)], "AREA01PIPE"),
        };

        let outcome = evaluate(&rule, &scope);

        assert!(outcome.members.is_empty(), "认不出的属性不该有命中");
        assert_eq!(
            kinds(&outcome),
            vec![EvalIssueKind::UnsupportedAttribute],
            "必须记一条 UnsupportedAttribute，不能静默返回空"
        );
        assert!(
            outcome.unsupported[0].detail.contains(&name.to_string()),
            "detail 里要能看出是哪个 hash：{:?}",
            outcome.unsupported[0]
        );
    }

    /// VAL-EVAL-005：`Before(..)` 记 `UnsupportedOp`，不 panic、不静默。
    #[test]
    fn a_before_node_is_recorded_as_unsupported() {
        let scope = FixtureScope::default();
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: ScoselExpr::Before(
                Box::new(ScoselExpr::Attr {
                    hash: desc_attribute_hash() as i32,
                    tail: [1, 120, 0],
                    qualifiers: vec![noun_qualifier(ZONE)],
                }),
                Box::new(ScoselExpr::Text("AREA01".to_string())),
            ),
        };

        let outcome = evaluate(&rule, &scope);

        assert!(outcome.members.is_empty(), "未实现的操作符不该给出命中");
        assert_eq!(kinds(&outcome), vec![EvalIssueKind::UnsupportedOp]);
    }

    /// VAL-EVAL-006：认不出的限定形状进 `unsupported`。
    ///
    /// 两个场景：`Typed.words` 的类型码是 `4`（不是 noun 的 `6`），以及 `Value{..}`。
    /// 类型码在倒数第二字——把 `1701` 当类型码读的实现会在这里露馅（它会认不出真实样本）。
    #[test]
    fn a_qualifier_that_is_not_a_noun_restriction_is_unsupported() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11])
            .with_noun(ZONE, &[11]);

        // 类型码 4：`1602,4,2,640493,4,1701`。
        let typed_four = ScoselRule {
            attr: ZONE as i32,
            filter: str_eq(
                desc_attribute_hash(),
                vec![typed_qualifier(vec![2, ZONE as i32, 4, OP_QUALIFIER_TYPE])],
                "AREA01PIPE",
            ),
        };
        let typed_outcome = evaluate(&typed_four, &scope);
        assert!(typed_outcome.members.is_empty());
        assert_eq!(
            kinds(&typed_outcome),
            vec![EvalIssueKind::UnsupportedQualifier]
        );

        // 类型码 3：同样认不出。
        let typed_three = ScoselRule {
            attr: ZONE as i32,
            filter: str_eq(
                desc_attribute_hash(),
                vec![typed_qualifier(vec![2, ZONE as i32, 3, OP_QUALIFIER_TYPE])],
                "AREA01PIPE",
            ),
        };
        assert_eq!(
            kinds(&evaluate(&typed_three, &scope)),
            vec![EvalIssueKind::UnsupportedQualifier]
        );

        // `120,<value>` 限定段。
        let value_rule = ScoselRule {
            attr: ZONE as i32,
            filter: str_eq(
                desc_attribute_hash(),
                vec![ScoselQualifier::Value { value: 0 }],
                "AREA01PIPE",
            ),
        };
        let value_outcome = evaluate(&value_rule, &scope);
        assert!(value_outcome.members.is_empty());
        assert_eq!(
            kinds(&value_outcome),
            vec![EvalIssueKind::UnsupportedQualifier]
        );
    }

    /// hunk 值：等值比较的两侧不是「属性 vs 文本」时记 `HunkValue`。
    ///
    /// hunk 原文在 hunk pages 里，`decode` 不产出 `Text`，AST 里只剩「属性对属性」这种形态。
    /// 当前语料用不到（APS 全库 0 个 hunk 形态的 DESC），但不能静默当成空条件。
    #[test]
    fn a_comparison_without_a_text_literal_is_recorded_as_a_hunk_value() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11])
            .with_noun(ZONE, &[11]);
        let attr = || {
            Box::new(ScoselExpr::Attr {
                hash: desc_attribute_hash() as i32,
                tail: [1, 120, 0],
                qualifiers: vec![noun_qualifier(ZONE)],
            })
        };
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: ScoselExpr::StrEq(attr(), attr()),
        };

        let outcome = evaluate(&rule, &scope);

        assert!(outcome.members.is_empty());
        assert_eq!(kinds(&outcome), vec![EvalIssueKind::HunkValue]);
    }

    /// 裸操作数当条件（既不是 `StrEq` 也不是算子）：记 `UnsupportedOp`，不 panic。
    #[test]
    fn a_bare_operand_in_condition_position_is_unsupported() {
        let scope = FixtureScope::default();
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: ScoselExpr::Attr {
                hash: desc_attribute_hash() as i32,
                tail: [1, 120, 0],
                qualifiers: vec![noun_qualifier(ZONE)],
            },
        };

        let outcome = evaluate(&rule, &scope);

        assert!(outcome.members.is_empty());
        assert_eq!(kinds(&outcome), vec![EvalIssueKind::UnsupportedOp]);
    }

    /// VAL-EVAL-007：27 条真实样本全部 `decode` + `evaluate` 成功，`unsupported` 全空。
    ///
    /// 两个 noun 分支都要走到：24 条 ZONE、3 条 REFGRD（每条 GRID 规则有两个条件，都是 REFGRD）。
    /// 断言样本数 `== 27`，与 `scosel.rs` 的既有两条测试同一口径。
    #[test]
    fn every_real_sample_evaluates_without_unsupported() {
        let samples = real_samples();
        assert_eq!(samples.len(), 27, "样本数");

        // 夹具 scope 对任何 DESC 原文都回同一个 refno，两个 noun 域也都含它，
        // 于是每条规则都应命中它一次——这样「求值走通了」才不是空集侥幸通过。
        let scope = AnyTextScope::default();

        let mut nouns_seen = HashSet::new();
        for (name, words) in &samples {
            let parsed =
                scosel::decode(words).unwrap_or_else(|error| panic!("{name} 解不开：{error}"));
            assert_eq!(parsed.rules.len(), 1, "{name} 应只有一条规则");
            let outcome = evaluate(&parsed.rules[0], &scope);
            assert!(
                outcome.unsupported.is_empty(),
                "{name} 是已确认形状，不该有 unsupported：{:?}",
                outcome.unsupported
            );
            assert_eq!(
                outcome.members,
                vec![RefU64(AnyTextScope::HIT)],
                "{name} 应命中夹具元素"
            );
            nouns_seen.extend(scope.asked_nouns.borrow().iter().copied());
        }
        assert_eq!(
            nouns_seen,
            HashSet::from([ZONE, REFGRD]),
            "24 条 ZONE 与 3 条 REFGRD 两个 noun 分支都要走到"
        );
        assert!(
            scope.asked_texts.borrow().iter().any(|t| t == "AREA01PIPE"),
            "求值要把 DESC 原文交给 scope：{:?}",
            scope.asked_texts.borrow()
        );
    }

    /// 真实样本用的 scope：任何 DESC 原文都命中同一个 refno，两个 noun 域也都含它。
    #[derive(Default)]
    struct AnyTextScope {
        asked_nouns: RefCell<Vec<u32>>,
        asked_texts: RefCell<Vec<String>>,
    }

    impl AnyTextScope {
        /// 夹具里的假 refno：只要每条真实样本都命中同一个元素，就能证明求值走通了
        /// （而不是「两边都空、交集也空」的侥幸通过）。
        const HIT: u64 = 42;
    }

    impl ScoselScope for AnyTextScope {
        fn refnos_with_attr_text(&self, attr_hash: u32, text: &str) -> HashSet<RefU64> {
            assert_eq!(attr_hash, desc_attribute_hash(), "真实样本只该查 DESC");
            self.asked_texts.borrow_mut().push(text.to_string());
            HashSet::from([RefU64(Self::HIT)])
        }

        fn refnos_of_noun(&self, noun_hash: u32) -> HashSet<RefU64> {
            self.asked_nouns.borrow_mut().push(noun_hash);
            HashSet::from([RefU64(Self::HIT)])
        }
    }

    /// VAL-EVAL-008：`members` 按 `RefU64` 位模式排序，连续求值 3 次逐字相等。
    #[test]
    fn members_are_sorted_by_refno_so_repeated_evaluation_agrees() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[50, 7, 900, 3, 42])
            .with_noun(ZONE, &[50, 7, 900, 3, 42]);
        let rule = desc_rule(ZONE, "AREA01PIPE");

        let first = evaluate(&rule, &scope);
        let second = evaluate(&rule, &scope);
        let third = evaluate(&rule, &scope);

        assert_eq!(
            first.members,
            vec![RefU64(3), RefU64(7), RefU64(42), RefU64(50), RefU64(900)],
            "members 必须按 refno 升序"
        );
        assert_eq!(first, second, "同一规则第二次求值要逐字相同");
        assert_eq!(second, third, "同一规则第三次求值要逐字相同");
    }

    /// VAL-EVAL-009：首版区分大小写——求值器不做任何大小写折叠。
    ///
    /// 断言两件事：`area01pipe` 在只有 `AREA01PIPE` 的索引里命中 0；求值器交给 scope 的原文
    /// 与规则里的字面量逐字相同（没有 `to_ascii_lowercase` / `to_ascii_uppercase`）。
    /// 真实语料里 DESC 全大写、无大小写混合样本，这条只钉住实现决定，不代表与 E3D 一致。
    #[test]
    fn matching_is_case_sensitive_in_the_first_version() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11])
            .with_noun(ZONE, &[11]);

        let lower = evaluate(&desc_rule(ZONE, "area01pipe"), &scope);
        assert!(lower.members.is_empty(), "小写查询不该命中大写原文");
        assert!(
            lower.unsupported.is_empty(),
            "大小写不同是「没命中」，不是「算不出来」"
        );
        assert_eq!(
            scope.asked.borrow().last().map(|(_, text)| text.as_str()),
            Some("area01pipe"),
            "求值器必须把原文原样交给 scope，不得折叠大小写"
        );

        let exact = evaluate(&desc_rule(ZONE, "AREA01PIPE"), &scope);
        assert_eq!(exact.members, vec![RefU64(11)], "逐字相同的原文要命中");
    }

    /// 认得出的限定段缺了 `1701` 标记，同样算认不出（不猜末字语义）。
    #[test]
    fn a_typed_qualifier_without_the_type_marker_is_unsupported() {
        let scope = FixtureScope::default()
            .with_text("AREA01PIPE", &[11])
            .with_noun(ZONE, &[11]);
        let rule = ScoselRule {
            attr: ZONE as i32,
            filter: str_eq(
                desc_attribute_hash(),
                vec![typed_qualifier(vec![2, ZONE as i32, QUALIFIER_NOUN_TYPE])],
                "AREA01PIPE",
            ),
        };

        let outcome = evaluate(&rule, &scope);

        assert!(outcome.members.is_empty());
        assert_eq!(
            kinds(&outcome),
            vec![EvalIssueKind::UnsupportedQualifier],
            "缺 1701 标记的限定段认不出"
        );
    }

    /// `DESC` 的 hash 必须与真库 hash 表一致（`library/scosel-semantics.md` §4）。
    #[test]
    fn the_desc_hash_agrees_with_the_known_hash_table() {
        assert_eq!(desc_attribute_hash(), 604_480, "DESC 的 db1 哈希");
        assert_eq!(e3d_attlib::db1_hash("ZONE"), ZONE);
        assert_eq!(e3d_attlib::db1_hash("REFGRD"), REFGRD);
    }

    /// 夹具里那 27 条真实 `SCOSEL` 原字（与 `scosel.rs` 的测试读同一份文件）。
    fn real_samples() -> Vec<(String, Vec<i32>)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/group-contract/scosel-real-samples.json"
        );
        let text = std::fs::read_to_string(path).expect("read scosel-real-samples.json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse json");
        value["samples"]
            .as_array()
            .expect("samples array")
            .iter()
            .map(|sample| {
                let name = sample["name"].as_str().unwrap_or_default().to_owned();
                let words = sample["words"]
                    .as_array()
                    .expect("words array")
                    .iter()
                    .map(|word| i32::try_from(word.as_i64().expect("int word")).expect("fits i32"))
                    .collect();
                (name, words)
            })
            .collect()
    }
}
