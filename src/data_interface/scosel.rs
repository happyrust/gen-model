//! GPSET 规则成员 `SCOSEL` 的编解码：序列化栈机 ↔ 表达式树（只管编解码，不求值、不接线）。
//!
//! `SCOSEL` 是 `DB_Collection` 的序列化栈机：头部、class 段、规则组与它的 filter 表达式段；表达式是
//! 后缀写法——操作数入栈，算子弹两个、压回一个。opcode 语义见
//! `docs/plans/2026-11-08-scosel-opcode-reference.md`（IDA 取证：§2 容器层、§3 表达式层）。
//!
//! v1 只认已确认子集（同文档 §6.3），其余一律 [`ScoselError::Unsupported`]，带 opcode 数值与数组下标；
//! 不跳过不认识的字往下读——栈机会整体错位。注意 `1306` 是 after、`1307` 才是 before；`301` 是 OR
//! （并集）、`302` 是 AND（交集）。
//!
//! 操作数布局取证文档没有写死。下表标「假设」的按最自然的读法定，靠四道自检兜住——任何一条假设
//! 猜错都会撞上其中之一而报错，不会静默错位：
//!
//! 1. 总字数：首字 `word_count` 必须等于数组长度；
//! 2. 段长度：`100015` 声明的长度必须被表达式恰好读完——操作数读过段尾报 [`ScoselError::Truncated`]，
//!    段里读到容器层 opcode（长度偏长）报 [`ScoselError::Malformed`]；
//! 3. 栈深度：算子弹栈不足两个、或段读完时栈里不是恰好一棵树，报 [`ScoselError::Malformed`]；
//! 4. 段后必须是已知 opcode：每段读完，下一个字必须是容器层已认的 opcode，否则报 `Unsupported`。
//!
//! | 字 | 布局 | 依据 |
//! |---|---|---|
//! | `[word_count]` | 数组总字数，**含自身** | 27 条真实样本逐条成立 |
//! | `100016,1` | 头部，值必须是 `1` | 取证文档 §1 |
//! | `100009,N,<word>×(N-1)` | `N` 是段字数，**含 `N` 自身、不含 `100009`**。`N=3` → 段是 `3,100007,640493`，下一个字正好是 `100015` | 27 条真实样本逐条成立 |
//! | `100007,<attr>` | 规则组，一个操作数，其后紧跟它的 `100015` 段 | 文档 §1 形状 |
//! | `100015,<len>,1,<expr>×(<len>-2)` | `len` **含自身**。35 字样本 `len=27` 落在下标 8，`8+27=35` 正好是数组末 | 27 条真实样本逐条成立 |
//! | `100011` / `100012` | 段边界，无操作数（假设） | 自检 4 |
//! | `100001` | 显式成员组：布局未查清，遇到即报 [`ScoselError::Undecoded`]，不猜后面几个字 | 文档 §5 |
//! | `106,4,<attr>,<t1>,<t2>,<t3>` | 属性引用：计数字必须是 `4`，尾部 3 字原样留存。**入栈一个操作数** | 文档 §5、样本聚类 |
//! | `1602,<count>,<word>×<count>` | 属性引用的限定段，紧跟 `106`，**不入栈**；`count=4` 时末字是 `1701` 类型标记 | 27 条真实样本逐条成立 |
//! | `120,<value>` | 条件内部限定，**不入栈** | 样本聚类 |
//! | `118,N,<char>×N` | 文本字面量，一字一个 ASCII 字符。**入栈一个操作数** | 样本聚类 |
//! | `402` / `301` / `302` | 纯栈算子，无内联操作数，二元：弹两个、压回一个 | 自检 3 |
//!
//! **`1306` / `1307` / `1317` 在 27 条真实样本里一个都不出现**：这 27 组是等值分组（`402`
//! 全串相等），不是前缀分组。常量与分支保留给已确认的前缀形状，v1 用不上。
//!
//! 表达式层因此是**普通后缀栈**，不是「条件结构」：`106` 压一个、`118` 压一个、`402` 弹两个
//! 压回一个，段末栈深恰好 1。`1602` 与 `120` 是 `106` 条件的内部限定，不参与栈运算——把它们
//! 也各压一个才会算出「段末剩 2 个」，27 条逐条演算都是这个结论。
//!
//! 样本聚类见 `docs/plans/2026-11-08-gpwl-gpset-group-nodes.md` §0。

use std::fmt;

/// 头部：属性 id 段，值恒为 `1`。
pub const OP_ATTRIBUTE_ID: i32 = 100_016;
/// class 段：`100009,N,<class>×N`。
pub const OP_CLASS: i32 = 100_009;
/// 规则组：`100007,<attr>`，其后紧跟 [`OP_FILTER`] 段。
pub const OP_RULE_GROUP: i32 = 100_007;
/// 显式成员组（GPITEM 链那一支）：布局未查清。
pub const OP_EXPLICIT_GROUP: i32 = 100_001;
/// filter 表达式段：`100015,<len>,1,<expr>×len`。
pub const OP_FILTER: i32 = 100_015;
/// 段边界，无操作数。
pub const OP_SEGMENT_BOUNDARIES: [i32; 2] = [100_011, 100_012];
/// 属性引用：`106,4,<attr>,<t1>,<t2>,<t3>`。
pub const OP_ATTR_REF: i32 = 106;
/// 属性引用的限定段：`1602,<count>,<word>×<count>`，紧跟 `106`，不入栈。
pub const OP_ATTR_QUALIFIER: i32 = 1602;
/// 条件内部限定：`120,<value>`，不入栈。
pub const OP_QUALIFIER: i32 = 120;
/// 限定段的末字类型标记，出现在 `1602` 的最后一个字位置。
pub const OP_QUALIFIER_TYPE: i32 = 1701;
/// 文本字面量：`118,N,<char>×N`。
pub const OP_TEXT: i32 = 118;
/// 字符串相等。
pub const OP_STR_EQ: i32 = 402;
/// before（`1306` 才是 after）。
pub const OP_BEFORE: i32 = 1307;
/// OR（并集）。
pub const OP_OR: i32 = 301;
/// AND（交集）。
pub const OP_AND: i32 = 302;

const ATTRIBUTE_ID_VALUE: i32 = 1;
const FILTER_LEAD: i32 = 1;
const ATTR_REF_WORDS: usize = 4;
/// 容器层 opcode 都在这之上，表达式层都在这之下。
const CONTAINER_OPCODE_FLOOR: i32 = 100_000;

/// 解码结果：class 段（求值域先按它缩窄）与规则组，按存储原序。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scosel {
    pub classes: Vec<i32>,
    pub rules: Vec<ScoselRule>,
}

/// 一个规则组：`100007` 的操作数与紧随其后的 filter 表达式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoselRule {
    pub attr: i32,
    pub filter: ScoselExpr,
}

/// filter 表达式树，节点照 opcode 语义命名；求值不在这里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoselExpr {
    /// `106`：属性引用；`tail` 是属性 hash 之后的 3 个字，语义未查清，原样留存。
    /// `qualifiers` 是紧随其后的 `1602` / `120` 限定段——它们描述这个属性引用怎么取，
    /// 不参与栈运算（各压一个会让段末栈深变成 2，27 条真实样本逐条演算都是这个结果）。
    Attr {
        hash: i32,
        tail: [i32; 3],
        qualifiers: Vec<ScoselQualifier>,
    },
    /// `118`：文本字面量。
    Text(String),
    /// `1307`：before（`1306` 才是 after）。27 条真实样本里不出现，v1 用不上。
    Before(Box<ScoselExpr>, Box<ScoselExpr>),
    /// `402`：字符串相等。
    StrEq(Box<ScoselExpr>, Box<ScoselExpr>),
    /// `301`：OR（并集）。
    Or(Box<ScoselExpr>, Box<ScoselExpr>),
    /// `302`：AND（交集）。
    And(Box<ScoselExpr>, Box<ScoselExpr>),
}

/// `106` 属性引用后面的限定段，原样留存：v1 不解它的语义，只保证逐字往返。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoselQualifier {
    /// `1602,<count>,<word>×<count>`：`words` 是计数字之后的那些字（含末字类型标记 `1701`）。
    Typed { words: Vec<i32> },
    /// `120,<value>`。
    Value { value: i32 },
}

/// 解码失败；下标都是在 `SCOSEL` 数组里的位置，拿真库样本能直接对上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoselError {
    /// 不在 v1 子集里的 opcode。
    Unsupported { opcode: i32, index: usize },
    /// 已识别、但操作数布局未查清的 opcode（`100001`）：不猜后面几个字。
    Undecoded { opcode: i32, index: usize },
    /// `index` 处的 opcode 要读到下标 `needs`，数组或所在段只到 `end`。
    Truncated {
        index: usize,
        needs: usize,
        end: usize,
    },
    /// 结构自检不过：总字数、头部、计数字、栈深度等。
    Malformed { index: usize, reason: &'static str },
}

impl fmt::Display for ScoselError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { opcode, index } => {
                write!(f, "SCOSEL unsupported opcode {opcode}（下标 {index}）")
            }
            Self::Undecoded { opcode, index } => {
                write!(
                    f,
                    "SCOSEL opcode {opcode} 已识别但布局未查清（下标 {index}）"
                )
            }
            Self::Truncated { index, needs, end } => {
                write!(
                    f,
                    "SCOSEL 下标 {index} 处要读到下标 {needs}，数据只到 {end}"
                )
            }
            Self::Malformed { index, reason } => write!(f, "SCOSEL 下标 {index}：{reason}"),
        }
    }
}

impl std::error::Error for ScoselError {}

/// 解码一段 `SCOSEL`（int 数组，含首字 `word_count`）。
pub fn decode(words: &[i32]) -> Result<Scosel, ScoselError> {
    let len = words.len();
    let declared = slice(words, 0, 0, 1, len)?[0];
    if usize::try_from(declared).ok() != Some(len) {
        return Err(malformed(0, "首字 word_count 与数组长度不符"));
    }
    if slice(words, 1, 1, 1, len)?[0] != OP_ATTRIBUTE_ID {
        return Err(malformed(1, "头部应以 100016 开头"));
    }
    if slice(words, 1, 2, 1, len)?[0] != ATTRIBUTE_ID_VALUE {
        return Err(malformed(2, "100016 的值应为 1"));
    }
    let mut scosel = Scosel::default();
    // `100009` 段里读到的规则组属性 hash，按段内原序，等它后面的 `100015` 段来配对。
    // 这是解码过程中的中间态，不属于 `Scosel`：`Scosel` 只描述「有哪些规则」，配对完就该空。
    let mut pending_attrs: std::collections::VecDeque<i32> = std::collections::VecDeque::new();
    let mut at = 3;
    while at < len {
        let opcode = words[at];
        match opcode {
            OP_CLASS => {
                // `100009,<N>,<word>×(N-1)`：`N` 是段字数，含自身、不含 opcode。
                //
                // 35 字真实样本里段是 `100009,3,100007,640493`：N=3 覆盖 `3,100007,640493`
                // 三个字，段内除 N 之外的内容是 `100007,640493`——**规则组的 opcode 与它的属性
                // hash 就在 class 段里面**，不是独立的一段。段尾下一个字正好是 `100015`。
                //
                // 所以这一段不产出 `classes`（样本里没有 class id），它只承载规则组的标记。
                // 把 N 当 class 个数会让 `100007` 被当成 class id、`100015` 被当成普通字，
                // 27 条真实样本全部错位。
                let segment = count_operand(words, at, len)?;
                if segment == 0 {
                    return Err(malformed(at + 1, "100009 的段字数不能为 0（至少含自身）"));
                }
                let body = slice(words, at, at + 2, segment - 1, len)?;
                let mut cursor = 0;
                while cursor < body.len() {
                    if body[cursor] == OP_RULE_GROUP {
                        let attr = *body.get(cursor + 1).ok_or_else(|| {
                            malformed(at + 2 + cursor, "100009 段里的 100007 后面缺属性 hash")
                        })?;
                        pending_attrs.push_back(attr);
                        cursor += 2;
                    } else {
                        scosel.classes.push(body[cursor]);
                        cursor += 1;
                    }
                }
                at += 1 + segment;
            }
            OP_RULE_GROUP => {
                // 段外的 `100007,<attr>` 形式（老形状）：属性直接跟在这里。
                let attr = slice(words, at, at + 1, 1, len)?[0];
                let filter_at = at + 2;
                if slice(words, at, filter_at, 1, len)?[0] != OP_FILTER {
                    return Err(malformed(
                        filter_at,
                        "100007 规则组后应紧跟 100015 filter 段",
                    ));
                }
                let (filter, next) = decode_filter(words, filter_at)?;
                scosel.rules.push(ScoselRule { attr, filter });
                at = next;
            }
            OP_FILTER => {
                // `100009` 段里读到的 `100007,<attr>` 标记，与紧随其后的 `100015` 段按序配对。
                // 27 条真实样本都是这个形状：段内一个标记、段后一个 filter。
                let Some(attr) = pending_attrs.pop_front() else {
                    return Err(malformed(at, "100015 filter 段前面没有 100007 规则组"));
                };
                let (filter, next) = decode_filter(words, at)?;
                scosel.rules.push(ScoselRule { attr, filter });
                at = next;
            }
            OP_EXPLICIT_GROUP => return Err(ScoselError::Undecoded { opcode, index: at }),
            _ if OP_SEGMENT_BOUNDARIES.contains(&opcode) => at += 1,
            _ => return Err(ScoselError::Unsupported { opcode, index: at }),
        }
    }
    Ok(scosel)
}

/// `100015,<len>,1,<expr>×…`：返回表达式树与段后的下标。
///
/// `len` **含自身**：35 字真实样本 `len=27` 落在下标 8，`8+27=35` 正好是数组末；60 字样本
/// `len=52` 同理。`len` 在 `at+1`，所以段尾是 `at + 1 + len`，表达式从 `at+3` 读到那里。
fn decode_filter(words: &[i32], at: usize) -> Result<(ScoselExpr, usize), ScoselError> {
    let segment = count_operand(words, at, words.len())?;
    if segment < 2 {
        return Err(malformed(
            at + 1,
            "100015 的段字数至少为 2（含自身与 1）",
        ));
    }
    if slice(words, at, at + 2, 1, words.len())?[0] != FILTER_LEAD {
        return Err(malformed(at + 2, "100015 的长度之后应为 1"));
    }
    let start = at + 3;
    let end = at + 1 + segment;
    slice(words, at, start, end.saturating_sub(start), words.len())?;
    Ok((decode_expr(words, start, end)?, end))
}

/// 后缀表达式 `words[start..end]`：操作数入栈，算子弹两个压一个；读到段尾必须恰好剩一棵树。
fn decode_expr(words: &[i32], start: usize, end: usize) -> Result<ScoselExpr, ScoselError> {
    let mut stack = Vec::new();
    let mut at = start;
    while at < end {
        let opcode = words[at];
        match opcode {
            OP_ATTR_REF => {
                if count_operand(words, at, end)? != ATTR_REF_WORDS {
                    return Err(malformed(at + 1, "106 的计数字应为 4"));
                }
                let body = slice(words, at, at + 2, ATTR_REF_WORDS, end)?;
                let mut expr = ScoselExpr::Attr {
                    hash: body[0],
                    tail: [body[1], body[2], body[3]],
                    qualifiers: Vec::new(),
                };
                at += 2 + ATTR_REF_WORDS;
                // `106` 的条件常常紧跟限定段（`1602,<count>,…` 与 `120,<value>`）。它们描述
                // 这个属性引用怎么取，不是独立操作数：各压一个会让段末栈深变成 2，27 条真实
                // 样本逐条演算都是这个结果。所以在这里一并吃掉，挂在 Attr 上。
                let mut qualifiers = Vec::new();
                while at < end {
                    match words[at] {
                        OP_ATTR_QUALIFIER => {
                            let count = count_operand(words, at, end)?;
                            let body = slice(words, at, at + 2, count, end)?;
                            qualifiers.push(ScoselQualifier::Typed {
                                words: body.to_vec(),
                            });
                            at += 2 + count;
                        }
                        OP_QUALIFIER => {
                            let value = slice(words, at, at + 1, 1, end)?[0];
                            qualifiers.push(ScoselQualifier::Value { value });
                            at += 2;
                        }
                        _ => break,
                    }
                }
                if let ScoselExpr::Attr { qualifiers: slot, .. } = &mut expr {
                    *slot = qualifiers;
                }
                stack.push(expr);
            }
            OP_TEXT => {
                let count = count_operand(words, at, end)?;
                let chars = slice(words, at, at + 2, count, end)?;
                let text = chars
                    .iter()
                    .enumerate()
                    .map(|(offset, &word)| {
                        u8::try_from(word)
                            .ok()
                            .filter(u8::is_ascii)
                            .map(char::from)
                            .ok_or_else(|| malformed(at + 2 + offset, "118 字面量里有非 ASCII 字"))
                    })
                    .collect::<Result<String, _>>()?;
                stack.push(ScoselExpr::Text(text));
                at += 2 + count;
            }
            OP_BEFORE | OP_STR_EQ | OP_OR | OP_AND => {
                let (Some(rhs), Some(lhs)) = (stack.pop(), stack.pop()) else {
                    return Err(malformed(at, "算子弹栈不足两个操作数"));
                };
                let (lhs, rhs) = (Box::new(lhs), Box::new(rhs));
                stack.push(match opcode {
                    OP_BEFORE => ScoselExpr::Before(lhs, rhs),
                    OP_STR_EQ => ScoselExpr::StrEq(lhs, rhs),
                    OP_OR => ScoselExpr::Or(lhs, rhs),
                    _ => ScoselExpr::And(lhs, rhs),
                });
                at += 1;
            }
            _ if opcode >= CONTAINER_OPCODE_FLOOR => {
                return Err(malformed(
                    at,
                    "filter 段里读到容器层 opcode（100015 的长度偏长）",
                ));
            }
            _ => return Err(ScoselError::Unsupported { opcode, index: at }),
        }
    }
    match (stack.pop(), stack.is_empty()) {
        (Some(tree), true) => Ok(tree),
        _ => Err(malformed(end, "filter 段读完时栈里不是恰好一棵树")),
    }
}

/// `words[at + 1]` 当计数读，负数不是合法计数。
fn count_operand(words: &[i32], at: usize, end: usize) -> Result<usize, ScoselError> {
    let count = slice(words, at, at + 1, 1, end)?[0];
    usize::try_from(count).map_err(|_| malformed(at + 1, "计数字为负"))
}

/// `words[from..from + count]`；`end`（数组长度或所在段尾）之前读不完，就报 `at` 处的 opcode 截断。
fn slice(
    words: &[i32],
    at: usize,
    from: usize,
    count: usize,
    end: usize,
) -> Result<&[i32], ScoselError> {
    let needs = from.saturating_add(count);
    if needs > end {
        return Err(ScoselError::Truncated {
            index: at,
            needs,
            end,
        });
    }
    Ok(&words[from..needs])
}

fn malformed(index: usize, reason: &'static str) -> ScoselError {
    ScoselError::Malformed { index, reason }
}

/// 把 [`Scosel`] 写回 `SCOSEL` int 数组（含首字 `word_count`），是 [`decode`] 的逆：
/// `decode(&encode(&x)) == Ok(x)`。写侧（M5 的 `set_gpset_rules` 收 `&[i32]`）用它产出数组。
///
/// 按文件头的布局表写：头部、一段 `100009`（`N` 可以是 0）、各规则组按原序。段边界 `100011` /
/// `100012` 不写——样本里它们的位置还没查清，[`decode`] 也不靠它们分段。所以往返只证明与 [`decode`]
/// 自洽；和 E3D 自己写出的数组逐字一致，要等真库样本做到 `encode(decode(样本)) == 样本` 才算核实。
///
/// `Text` 只应含 ASCII（[`decode`] 只产出 ASCII）：非 ASCII 字符按码位写出，[`decode`] 读回时在
/// 该下标报 [`ScoselError::Malformed`]，不会静默改字。
pub fn encode(scosel: &Scosel) -> Vec<i32> {
    // `100009` 段里既放 class id，也放 `100007,<attr>` 规则组标记（27 条真实样本的形状：
    // `100009,3,100007,640493`）。段字数 N 含自身、不含 opcode，所以 N = 1 + 段内容字数。
    let mut segment = Vec::new();
    for class in &scosel.classes {
        segment.push(*class);
    }
    for rule in &scosel.rules {
        segment.push(OP_RULE_GROUP);
        segment.push(rule.attr);
    }
    let mut words = vec![0, OP_ATTRIBUTE_ID, ATTRIBUTE_ID_VALUE, OP_CLASS];
    words.push(count_word(1 + segment.len()));
    words.extend_from_slice(&segment);
    for rule in &scosel.rules {
        words.push(OP_FILTER);
        // `100015` 的 `len` 含自身：`len = 1(len 自己) + 1(前导 1) + 表达式字数`。
        let len_at = words.len();
        words.extend_from_slice(&[0, FILTER_LEAD]);
        let start = words.len();
        encode_expr(&rule.filter, &mut words);
        words[len_at] = count_word(2 + (words.len() - start));
    }
    words[0] = count_word(words.len());
    words
}

fn encode_expr(expr: &ScoselExpr, words: &mut Vec<i32>) {
    match expr {
        ScoselExpr::Attr {
            hash,
            tail,
            qualifiers,
        } => {
            words.extend_from_slice(&[OP_ATTR_REF, count_word(ATTR_REF_WORDS), *hash]);
            words.extend_from_slice(tail);
            for qualifier in qualifiers {
                match qualifier {
                    ScoselQualifier::Typed { words: body } => {
                        words.extend_from_slice(&[OP_ATTR_QUALIFIER, count_word(body.len())]);
                        words.extend_from_slice(body);
                    }
                    ScoselQualifier::Value { value } => {
                        words.extend_from_slice(&[OP_QUALIFIER, *value]);
                    }
                }
            }
        }
        ScoselExpr::Text(text) => {
            words.extend_from_slice(&[OP_TEXT, count_word(text.chars().count())]);
            words.extend(text.chars().map(|c| c as i32));
        }
        ScoselExpr::Before(lhs, rhs) => encode_operator(OP_BEFORE, lhs, rhs, words),
        ScoselExpr::StrEq(lhs, rhs) => encode_operator(OP_STR_EQ, lhs, rhs, words),
        ScoselExpr::Or(lhs, rhs) => encode_operator(OP_OR, lhs, rhs, words),
        ScoselExpr::And(lhs, rhs) => encode_operator(OP_AND, lhs, rhs, words),
    }
}

/// 后缀写法：左、右两棵子树在前，算子在后——[`decode`] 先弹出的是右操作数。
fn encode_operator(opcode: i32, lhs: &ScoselExpr, rhs: &ScoselExpr, words: &mut Vec<i32>) {
    encode_expr(lhs, words);
    encode_expr(rhs, words);
    words.push(opcode);
}

/// 计数字与首字都是 i32，超过 `i32::MAX` 的长度写不进 `SCOSEL`。
fn count_word(n: usize) -> i32 {
    i32::try_from(n).expect("SCOSEL 的计数超出 i32")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的占位值，不是 E3D 的真实 hash / class id。
    const RULE_ATTR: i32 = 640_001;
    const NAME: i32 = 640_002;
    /// 样本聚类里 `106` 的尾部三字。
    const NAME_TAIL: [i32; 3] = [1, 120, 0];

    /// 合成一段 SCOSEL：补上首字 `word_count` 与 `100016,1` 头。
    fn scosel(body: &[i32]) -> Vec<i32> {
        let mut words = vec![0, OP_ATTRIBUTE_ID, 1];
        words.extend_from_slice(body);
        words[0] = word(words.len());
        words
    }

    fn word(n: usize) -> i32 {
        i32::try_from(n).unwrap()
    }

    fn class(ids: &[i32]) -> Vec<i32> {
        // `N` 是段字数，含自身：`1(N 自己) + ids.len()`。
        [&[OP_CLASS, word(1 + ids.len())][..], ids].concat()
    }

    /// 规则组：`100007,<attr>` + `100015,<len>,1,<expr>`。
    /// `len` 含自身：`1(len 自己) + 1(前导 1) + expr.len()`。
    fn rule(expr: &[i32]) -> Vec<i32> {
        [
            &[OP_RULE_GROUP, RULE_ATTR, OP_FILTER, word(2 + expr.len()), 1][..],
            expr,
        ]
        .concat()
    }

    fn name() -> Vec<i32> {
        [&[OP_ATTR_REF, 4, NAME][..], &NAME_TAIL[..]].concat()
    }

    fn text(s: &str) -> Vec<i32> {
        [
            vec![OP_TEXT, word(s.len())],
            s.bytes().map(i32::from).collect(),
        ]
        .concat()
    }

    /// `NAME before '<prefix>'`。
    fn before_name(prefix: &str) -> Vec<i32> {
        [name(), text(prefix), vec![OP_BEFORE]].concat()
    }

    fn name_node() -> Box<ScoselExpr> {
        Box::new(ScoselExpr::Attr {
            hash: NAME,
            tail: NAME_TAIL,
            qualifiers: Vec::new(),
        })
    }

    fn text_node(s: &str) -> Box<ScoselExpr> {
        Box::new(ScoselExpr::Text(s.to_owned()))
    }

    fn before_node(prefix: &str) -> ScoselExpr {
        ScoselExpr::Before(name_node(), text_node(prefix))
    }

    fn at(words: &[i32], opcode: i32) -> usize {
        words.iter().position(|&w| w == opcode).unwrap()
    }

    #[test]
    fn a_name_prefix_group_decodes_to_before() {
        let words = scosel(&[class(&[]), rule(&before_name("/AREA01_PIPE"))].concat());
        assert_eq!(
            decode(&words),
            Ok(Scosel {
                classes: vec![],
                rules: vec![ScoselRule {
                    attr: RULE_ATTR,
                    filter: before_node("/AREA01_PIPE")
                }],
            })
        );
    }

    #[test]
    fn two_prefix_rules_merge_with_301_as_or() {
        let expr = [
            before_name("/AREA01_PIPE"),
            before_name("/AREA02_PIPE"),
            vec![OP_OR],
        ]
        .concat();
        let words = scosel(&[class(&[]), rule(&expr)].concat());
        assert_eq!(
            decode(&words).unwrap().rules[0].filter,
            ScoselExpr::Or(
                Box::new(before_node("/AREA01_PIPE")),
                Box::new(before_node("/AREA02_PIPE"))
            )
        );
    }

    #[test]
    fn conditions_on_one_attribute_combine_with_302_as_and() {
        let equals = [name(), text("/AREA01_PIPE-100"), vec![OP_STR_EQ]].concat();
        let expr = [before_name("/AREA01"), equals, vec![OP_AND]].concat();
        let words = scosel(&[class(&[]), rule(&expr)].concat());
        assert_eq!(
            decode(&words).unwrap().rules[0].filter,
            ScoselExpr::And(
                Box::new(before_node("/AREA01")),
                Box::new(ScoselExpr::StrEq(
                    name_node(),
                    text_node("/AREA01_PIPE-100")
                )),
            )
        );
    }

    #[test]
    fn the_class_segment_lists_its_class_ids_and_boundaries_carry_no_operands() {
        let words = scosel(
            &[
                class(&[11, 12]),
                vec![100_011],
                rule(&before_name("/A")),
                vec![100_012],
            ]
            .concat(),
        );
        let decoded = decode(&words).unwrap();
        assert_eq!(decoded.classes, vec![11, 12]);
        assert_eq!(
            decoded.rules,
            vec![ScoselRule {
                attr: RULE_ATTR,
                filter: before_node("/A")
            }]
        );
    }

    #[test]
    fn unknown_opcodes_are_errors_carrying_value_and_index() {
        let id_of = [vec![1602, 4, 2, NAME, 6, 1701], text("/A"), vec![OP_STR_EQ]].concat();
        let words = scosel(&[class(&[]), rule(&id_of)].concat());
        assert_eq!(
            decode(&words),
            Err(ScoselError::Unsupported {
                opcode: 1602,
                index: at(&words, 1602)
            })
        );

        let words = scosel(&[class(&[]), vec![100_046, 0]].concat());
        let error = decode(&words).unwrap_err();
        assert_eq!(
            error,
            ScoselError::Unsupported {
                opcode: 100_046,
                index: 5
            }
        );
        assert_eq!(
            error.to_string(),
            "SCOSEL unsupported opcode 100046（下标 5）"
        );
    }

    #[test]
    fn inconsistent_lengths_and_stack_depth_are_errors() {
        let good = scosel(&[class(&[]), rule(&before_name("/A"))].concat());
        let mut words = good.clone();
        words[0] += 1;
        assert!(matches!(
            decode(&words),
            Err(ScoselError::Malformed { index: 0, .. })
        ));

        // 数组在表达式中间断掉（首字照实际长度改，让总字数自检过去）
        let mut words = good[..good.len() - 2].to_vec();
        words[0] = word(words.len());
        assert!(matches!(decode(&words), Err(ScoselError::Truncated { .. })));

        let lonely = [text("/A"), vec![OP_OR]].concat();
        let words = scosel(&[class(&[]), rule(&lonely)].concat());
        assert!(
            matches!(decode(&words), Err(ScoselError::Malformed { index, .. }) if index == at(&words, OP_OR))
        );

        let two_trees = [text("/A"), text("/B")].concat();
        let words = scosel(&[class(&[]), rule(&two_trees)].concat());
        assert!(
            matches!(decode(&words), Err(ScoselError::Malformed { index, .. }) if index == words.len())
        );
    }

    #[test]
    fn opcode_1306_is_after_and_is_not_read_as_before() {
        let after = [name(), text("/AREA01_PIPE"), vec![1306]].concat();
        let words = scosel(&[class(&[]), rule(&after)].concat());
        assert_eq!(
            decode(&words),
            Err(ScoselError::Unsupported {
                opcode: 1306,
                index: at(&words, 1306)
            })
        );
    }

    #[test]
    fn an_explicit_member_group_is_recognised_but_left_undecoded() {
        let words = scosel(&[class(&[]), vec![OP_EXPLICIT_GROUP, 3, 7, 8, 9]].concat());
        let error = decode(&words).unwrap_err();
        assert_eq!(
            error,
            ScoselError::Undecoded {
                opcode: OP_EXPLICIT_GROUP,
                index: 5
            }
        );
        assert_eq!(
            error.to_string(),
            "SCOSEL opcode 100001 已识别但布局未查清（下标 5）"
        );
    }

    #[test]
    fn a_class_count_of_zero_or_of_every_remaining_word_both_decode() {
        let words = scosel(&[class(&[]), rule(&before_name("/A"))].concat());
        let decoded = decode(&words).unwrap();
        assert!(decoded.classes.is_empty());
        assert_eq!(decoded.rules.len(), 1);

        let mut words = scosel(&class(&[21, 22, 23]));
        assert_eq!(
            decode(&words),
            Ok(Scosel {
                classes: vec![21, 22, 23],
                rules: vec![]
            })
        );
        words[4] += 1;
        assert_eq!(
            decode(&words),
            Err(ScoselError::Truncated {
                index: 3,
                needs: 9,
                end: 8
            })
        );
    }

    #[test]
    fn a_filter_length_that_disagrees_with_its_words_never_reads_past_the_segment() {
        let good = scosel(&[class(&[]), rule(&before_name("/A"))].concat());
        let filter_at = at(&good, OP_FILTER);

        // 长度比剩下的字多：段尾越过数组
        let mut words = good.clone();
        words[filter_at + 1] += 1;
        let end = words.len();
        assert_eq!(
            decode(&words),
            Err(ScoselError::Truncated {
                index: filter_at,
                needs: end + 1,
                end
            })
        );

        // 长度比表达式短：字面量读过段尾即停，不往段外接着读
        let mut words = good.clone();
        words[filter_at + 1] -= 2;
        let text_at = at(&words, OP_TEXT);
        assert_eq!(
            decode(&words),
            Err(ScoselError::Truncated {
                index: text_at,
                needs: text_at + 4,
                end: text_at + 3
            })
        );

        // 长度偏长、段后还有容器层的字：读到容器层 opcode 即报错
        let mut words = scosel(&[class(&[]), rule(&before_name("/A")), vec![100_012]].concat());
        let filter_at = at(&words, OP_FILTER);
        words[filter_at + 1] += 1;
        assert!(
            matches!(decode(&words), Err(ScoselError::Malformed { index, .. }) if index == words.len() - 1)
        );
    }

    fn rule_of(filter: ScoselExpr) -> ScoselRule {
        ScoselRule {
            attr: RULE_ATTR,
            filter,
        }
    }

    fn prefix_rule(prefix: &str) -> ScoselRule {
        rule_of(before_node(prefix))
    }

    fn rules_only(rules: Vec<ScoselRule>) -> Scosel {
        Scosel {
            classes: vec![],
            rules,
        }
    }

    /// decode 单测里出现过的形状，外加右嵌套与全空。
    fn shapes() -> Vec<(&'static str, Scosel)> {
        vec![
            (
                "纯 NAME 前缀",
                rules_only(vec![prefix_rule("/AREA01_PIPE")]),
            ),
            (
                "两个前缀用 301 合并",
                rules_only(vec![rule_of(ScoselExpr::Or(
                    Box::new(before_node("/AREA01_PIPE")),
                    Box::new(before_node("/AREA02_PIPE")),
                ))]),
            ),
            (
                "同一属性两个条件用 302",
                rules_only(vec![rule_of(ScoselExpr::And(
                    Box::new(before_node("/AREA01")),
                    Box::new(ScoselExpr::StrEq(
                        name_node(),
                        text_node("/AREA01_PIPE-100"),
                    )),
                ))]),
            ),
            (
                "右嵌套：前缀 OR（前缀 AND 相等）",
                rules_only(vec![rule_of(ScoselExpr::Or(
                    Box::new(before_node("/A")),
                    Box::new(ScoselExpr::And(
                        Box::new(before_node("/B")),
                        Box::new(ScoselExpr::StrEq(name_node(), text_node("/B-1"))),
                    )),
                ))]),
            ),
            (
                "class 段",
                Scosel {
                    classes: vec![11, 12],
                    rules: vec![prefix_rule("/A")],
                },
            ),
            (
                "空 rules",
                Scosel {
                    classes: vec![11],
                    rules: vec![],
                },
            ),
            (
                "空 classes、两个规则组",
                rules_only(vec![prefix_rule("/A"), prefix_rule("/B")]),
            ),
            ("全空", Scosel::default()),
        ]
    }

    #[test]
    fn encode_writes_the_layout_that_decode_reads() {
        // 真实形状（27 条样本）：class id 与 `100007,<attr>` 规则组标记同在 `100009` 段里，
        // 段后跟各规则组的 `100015` filter 段。段字数 N 含自身、不含 opcode：
        // 段内容是 `11,12,100007,640001` 四个字，所以 N = 1 + 4 = 5。
        let x = Scosel {
            classes: vec![11, 12],
            rules: vec![prefix_rule("/AREA01_PIPE")],
        };
        let segment = [11, 12, OP_RULE_GROUP, RULE_ATTR];
        let expected = scosel(
            &[
                [&[OP_CLASS, word(1 + segment.len())][..], &segment[..]].concat(),
                [&[OP_FILTER, word(2 + before_name("/AREA01_PIPE").len()), 1][..],
                 &before_name("/AREA01_PIPE")[..]].concat(),
            ]
            .concat(),
        );
        assert_eq!(encode(&x), expected);
        assert_eq!(decode(&expected), Ok(x));
    }

    #[test]
    fn every_decoded_shape_survives_an_encode_round_trip() {
        for (shape, x) in shapes() {
            assert_eq!(decode(&encode(&x)), Ok(x), "{shape}");
        }
    }

    #[test]
    fn the_first_encoded_word_counts_the_whole_array() {
        for (shape, x) in shapes() {
            let words = encode(&x);
            assert_eq!(words[0], word(words.len()), "{shape}");
        }
    }

    #[test]
    fn an_empty_scosel_encodes_to_a_header_and_an_empty_class_segment() {
        let words = encode(&Scosel::default());
        assert_eq!(words, scosel(&class(&[])));
        assert_eq!(decode(&words), Ok(Scosel::default()));
    }

    #[test]
    fn a_non_ascii_literal_is_written_as_its_code_point_and_refused_on_decode() {
        let words = encode(&rules_only(vec![prefix_rule("/管")]));
        let bad = words.iter().position(|&w| w == '管' as i32).unwrap();
        assert!(
            matches!(decode(&words), Err(ScoselError::Malformed { index, .. }) if index == bad)
        );
    }

    /// 真库取出的 27 条 `SCOSEL` 原字（aps250204 的 27 个 GPSET），最高证据。
    ///
    /// 验收线是逐字相等：`encode(decode(样本)) == 样本`。近似相等不算——`SCOSEL` 是栈机，
    /// 长度口径差一个字，E3D 读到的就是另一段字节。
    fn real_samples() -> Vec<(String, Vec<i32>)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/group-contract/scosel-real-samples.json"
        );
        let text = std::fs::read_to_string(path).expect("read scosel-real-samples.json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse json");
        let samples = value["samples"].as_array().expect("samples array");
        samples
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

    #[test]
    fn every_real_sample_decodes() {
        let samples = real_samples();
        assert_eq!(samples.len(), 27, "样本数");
        for (name, words) in &samples {
            if let Err(error) = decode(words) {
                panic!("{name} 解不开：{error}");
            }
        }
    }

    #[test]
    fn every_real_sample_round_trips_word_for_word() {
        let samples = real_samples();
        assert_eq!(samples.len(), 27, "样本数");
        let mut failures = Vec::new();
        for (name, words) in &samples {
            match decode(words) {
                Ok(parsed) => {
                    let re = encode(&parsed);
                    if &re != words {
                        let at = re
                            .iter()
                            .zip(words.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(re.len().min(words.len()));
                        failures.push(format!(
                            "{name}: 往返不等，首个差异在下标 {at}（写出 {} 字，样本 {} 字）",
                            re.len(),
                            words.len()
                        ));
                    }
                }
                Err(error) => failures.push(format!("{name}: 解不开：{error}")),
            }
        }
        assert!(
            failures.is_empty(),
            "{} / 27 条未逐字往返：\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

