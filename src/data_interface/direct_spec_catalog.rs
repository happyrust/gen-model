//! `pipe_model::SpecCatalog` 的生产实现：架在 [`DirectStore`]（e3d-io 文件直读面）上，
//! 读 CATA 库里的 `SPEC → SELE → SPCO → CATR` 选择树。
//!
//! 落库形态由 `tests/pipe_spec_forensics.rs` 对真实样本库
//! `AvevaCatalogue/acp000/acp7000_0001`（dbnum=7000，SPEC=55 / SELE=25978 / SPCO=18439）
//! 取证确认，与功能文档 §18.6/§18.8 的 PML 语义逐字段对上：
//!
//! ```text
//! SPWL (spec world)
//! └─ SPEC  /A150        QUES="TYPE" QUAL=0            ← 根问题节点：元件 noun
//!    └─ SELE            TANS="TUBE"（自身答案）; QUES="PBOR" QUAL=0（子问题）
//!       └─ SELE         ANSW=0   （自身答案）; QUES="STYP" QUAL=0
//!          └─ SELE      ANSW=15  （自身答案）; QUES="SHOP" QUAL=0
//!             └─ SPCO   TANS="TRUE"（自身答案）; CATR→目录元件
//! ```
//!
//! 语义规则（全部有取证锚点）：
//! - 每个节点同时携带**自身问题**（`QUES`+`QUAL`，其成员来回答）与**自身答案**
//!   （`TANS` 文本 / `ANSW`+`MAXA` 数值区间，回答父节点的问题）；
//! - 问键编码 `question & qualifier`：`QUAL>0` → `QUES&QUAL`；`QUAL=0` → 裸 `QUES`，
//!   但索引族问题（`PBOR`/`PSHA`/`PWID`/`PHEI`）恒带序号后缀（§18.8：`PBOR→PBOR0`）；
//! - `TANS="ANY"` 是通配行；数值答案按 `ANSW-0.001 ≤ v ≤ MAXA+0.001` 区间匹配；
//! - 答卷缺该键时：`table_lookup`（严格全答）不命中；`sele_descend` 走选择器
//!   的 `DEFA`/`TDEF` 默认子项，`TDEF="EQUA"` 时走 member[1] 等值链（§18.8）。
//!
//! `table_lookup` 与 `sele_descend` 的差异即 E3D 的档间差异：直查要求**每个问题
//! 都有答卷**（不带默认），SELE 回落允许默认子项补位——`spec_resolver::resolve`
//! 的三级回落靠这个区分出 `Direct`/`SelectorFallback` 两档结果。

use crate::data_interface::direct_store::DirectStore;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use pipe_model::{QuestionAnswers, SpecCatalog};
use std::sync::Arc;

/// E3D 数值答案匹配容差（孔径区间 `ANSW..MAXA` 两端的 ±tol）。
const RANGE_TOL: f64 = 0.001;

/// 索引族问题：`QUAL=0` 时问键仍带 `0` 后缀（§18.8 编码规则）。
const INDEXED_QUESTIONS: &[&str] = &["PBOR", "PSHA", "PWID", "PHEI"];

/// `TYPE` 问题的别名：设计侧 noun 与规格树答案名词不一致时的显式映射
/// （取证值：`TANS="TUBE"` 对应 TUBI 管片）。
fn type_aliases(answer: &str) -> &[&str] {
    match answer {
        "TUBI" => &["TUBE"],
        _ => &[],
    }
}

/// 文件直读面的 `SpecCatalog`。共享 [`DirectStore`]；SPEC/SELE/SPCO 住在 CATA 库，
/// 接入方负责先把对应 dbnum pin 进来（`named_attmap`/`members` 会按需自 pin）。
pub struct DirectSpecCatalog {
    store: Arc<DirectStore>,
}

impl DirectSpecCatalog {
    pub fn new(store: Arc<DirectStore>) -> Self {
        Self { store }
    }

    fn attmap(&self, refno: RefU64) -> Option<NamedAttrMap> {
        self.store.named_attmap(refno).ok()
    }

    /// owner：identity 键 `OWNER` 快路 → descriptor 抽取面兜底（与
    /// `direct_pipe_catalog`/`assemble_ele_data` 同一出处）。
    fn owner_of(&self, refno: RefU64) -> Option<RefU64> {
        if let Some(map) = self.attmap(refno) {
            let owner: RefU64 = map.get_owner().into();
            if owner != RefU64::default() {
                return Some(owner);
            }
        }
        let snap = self.store.element_snapshot(refno).ok()?;
        let owner = RefU64::from_two_nums(snap.extraction.owner.word0, snap.extraction.owner.word1);
        (owner != RefU64::default()).then_some(owner)
    }

    /// 按 `answer` 严格下钻一层：成员序首匹配（`ANY` 通配按序参与）。
    /// `answer` 为空 = 严格模式不命中；`use_default` 允许走 `DEFA`/`TDEF`/`EQUA`。
    fn descend(&self, selector: RefU64, qa: &QuestionAnswers, use_default: bool) -> Option<RefU64> {
        let map = self.attmap(selector)?;
        let ques = text_attr(&map, "QUES")?;
        let qual = int_attr(&map, "QUAL").unwrap_or(0);
        let key = question_key(&ques, qual);
        let answer = lookup_answer(qa, &key, &ques);

        let members = self.store.members(selector).unwrap_or_default();
        match answer {
            Some(ans) => {
                for member in members {
                    let mmap = self.attmap(member)?;
                    let noun = mmap.get_type();
                    if noun != "SELE" && noun != "SPCO" {
                        continue;
                    }
                    if member_matches(&mmap, ans) {
                        return Some(member);
                    }
                }
                None
            }
            None if use_default => {
                // TDEF="EQUA"：等值链 —— member[1]（PML 1 基 = 首个 SELE/SPCO 成员）。
                if text_attr(&map, "TDEF").as_deref() == Some("EQUA") {
                    return members.into_iter().find(|m| {
                        self.attmap(*m)
                            .map(|mm| matches!(mm.get_type().as_str(), "SELE" | "SPCO"))
                            .unwrap_or(false)
                    });
                }
                let defa = num_attr(&map, "DEFA");
                let tdef = text_attr(&map, "TDEF");
                for member in members {
                    let mmap = self.attmap(member)?;
                    let noun = mmap.get_type();
                    if noun != "SELE" && noun != "SPCO" {
                        continue;
                    }
                    if member_is_default(&mmap, defa, tdef.as_deref()) {
                        return Some(member);
                    }
                }
                None
            }
            None => None,
        }
    }
}

impl SpecCatalog for DirectSpecCatalog {
    /// 取证确认 SPEC 自身即根问题节点（`QUES="TYPE"`），根选择器 = SPEC 元素本身。
    /// `None` = 该元素不是 SPEC 或没有可读的 `QUES`。
    fn root_selector(&self, spref: RefU64) -> Option<RefU64> {
        let map = self.attmap(spref)?;
        (map.get_type() == "SPEC" && text_attr(&map, "QUES").is_some()).then_some(spref)
    }

    /// 严格表直查：注入 `TYPE=comp_type` 答卷后从 SPEC 逐层下钻，**每个问题都必须
    /// 有答卷**（不走默认）。全链走到 SPCO 叶子才算命中。
    /// TEE 老规格兜底：查空且答卷 ≥3 对时，以 `answers[2]="BORE(1)"` 重查一次
    /// （§18.6 决策 6 的第三槽改写语义）。
    fn table_lookup(&self, spref: RefU64, comp_type: &str, qa: &QuestionAnswers) -> Option<RefU64> {
        let mut qa = qa.clone();
        if !qa.iter().any(|(k, _)| k == "TYPE") {
            qa.insert(0, ("TYPE".into(), comp_type.into()));
        }
        self.strict_walk(spref, &qa).or_else(|| {
            if qa.len() > 2 {
                let mut retry = qa.clone();
                retry[2].1 = "BORE(1)".into();
                self.strict_walk(spref, &retry)
            } else {
                None
            }
        })
    }

    /// SELE 树单步下钻：答卷有键按序匹配，缺键走 `DEFA`/`TDEF`（`EQUA`→member[1]）。
    fn sele_descend(&self, selector: RefU64, qa: &QuestionAnswers) -> Option<RefU64> {
        self.descend(selector, qa, true)
    }

    fn is_spco(&self, node: RefU64) -> bool {
        self.attmap(node)
            .map(|m| m.get_type() == "SPCO")
            .unwrap_or(false)
    }

    /// 沿 SPCO 祖先链反推答卷（§18.8 `questionIndex`）：每个节点的答案对应**父节点**
    /// 的问题；返回 `root→leaf` 序（祖先链是 leaf→root，反转）。SPCO 自身答案 = 最深
    /// 问题的回答（取证：SPCO `TANS="TRUE"` 对应父 SELE `QUES="SHOP"`）。
    fn derive_answers(&self, spco: RefU64) -> Option<QuestionAnswers> {
        let mut pairs = Vec::new();
        let mut cur = spco;
        for _ in 0..64 {
            let owner = self.owner_of(cur)?;
            let omap = self.attmap(owner)?;
            let onoun = omap.get_type();
            if onoun != "SELE" && onoun != "SPEC" {
                break;
            }
            let ques = text_attr(&omap, "QUES")?;
            let qual = int_attr(&omap, "QUAL").unwrap_or(0);
            let cmap = self.attmap(cur)?;
            if let Some(ans) = own_answer(&cmap) {
                pairs.push((question_key(&ques, qual), ans));
            }
            cur = owner;
        }
        if pairs.is_empty() {
            return None;
        }
        pairs.reverse();
        Some(pairs)
    }
}

impl DirectSpecCatalog {
    /// 严格下钻：每层必须有答卷（不走默认）；走到 SPCO 返回，断链返回 None。
    fn strict_walk(&self, spref: RefU64, qa: &QuestionAnswers) -> Option<RefU64> {
        let mut cur = spref;
        for _ in 0..64 {
            let next = self.descend(cur, qa, false)?;
            if self.is_spco(next) {
                return Some(next);
            }
            cur = next;
        }
        None
    }
}

/// 问键编码：`QUAL>0` → `QUES&QUAL`；`QUAL=0` 索引族 → `QUES0`；否则裸 `QUES`。
fn question_key(ques: &str, qual: i64) -> String {
    if qual > 0 {
        format!("{ques}{qual}")
    } else if INDEXED_QUESTIONS.contains(&ques) {
        format!("{ques}0")
    } else {
        ques.to_string()
    }
}

/// 在答卷里查某问题的答案：先精确键（`PBOR0`），再裸键（`PBOR`）——
/// CHOICE 解码产出 `PBOR0`/`PBOR1`，`derive_answers` 亦按 `question_key` 产出，
/// 两侧在精确键上汇合；裸键兜底外部手构的答卷。
fn lookup_answer<'a>(qa: &'a QuestionAnswers, key: &str, bare: &str) -> Option<&'a str> {
    qa.iter()
        .find(|(k, _)| k == key)
        .or_else(|| qa.iter().find(|(k, _)| k == bare))
        .map(|(_, v)| v.as_str())
}

/// 节点自身答案：`TANS` 文本优先，否则 `ANSW`（空则 `MAXA`）数值格式化。
fn own_answer(map: &NamedAttrMap) -> Option<String> {
    if let Some(t) = text_attr(map, "TANS") {
        return Some(t);
    }
    num_attr(map, "ANSW")
        .or_else(|| num_attr(map, "MAXA"))
        .map(fmt_num)
}

/// 成员是否匹配给定答案：通配 `ANY` → `TANS` 文本相等 → `ANSW..MAXA` 区间。
/// 答案串先按别名展开（`TUBI`→`TUBE` 等显式映射），再逐个试。
fn member_matches(map: &NamedAttrMap, answer: &str) -> bool {
    let tans = text_attr(map, "TANS");
    if tans.as_deref() == Some("ANY") {
        return true;
    }
    let candidates: Vec<&str> = std::iter::once(answer)
        .chain(type_aliases(answer).iter().copied())
        .collect();
    for ans in candidates {
        if let Some(t) = &tans {
            if t == ans {
                return true;
            }
            continue; // 有 TANS 的节点不做数值匹配（答案面互斥）
        }
        if let (Some(answ), Ok(v)) = (num_attr(map, "ANSW"), ans.parse::<f64>()) {
            let maxa = num_attr(map, "MAXA").unwrap_or(answ);
            if v >= answ - RANGE_TOL && v <= maxa + RANGE_TOL {
                return true;
            }
        }
    }
    false
}

/// 成员是否默认子项：节点自身答案等于选择器的 `DEFA`（数值）或 `TDEF`（文本）。
fn member_is_default(map: &NamedAttrMap, defa: Option<f64>, tdef: Option<&str>) -> bool {
    if let (Some(t), Some(d)) = (text_attr(map, "TANS"), tdef) {
        return t == d;
    }
    if let (Some(a), Some(d)) = (num_attr(map, "ANSW"), defa) {
        return (a - d).abs() <= RANGE_TOL;
    }
    false
}

/// 文本属性：`StringType`/`WordType`/`ElementType` 非空且非 `unset`。
fn text_attr(map: &NamedAttrMap, name: &str) -> Option<String> {
    let v = match map.get(name)? {
        NamedAttrValue::StringType(s)
        | NamedAttrValue::WordType(s)
        | NamedAttrValue::ElementType(s) => s.trim(),
        _ => return None,
    };
    (!v.is_empty() && v != "unset").then(|| v.to_string())
}

/// 数值属性：`F32Type`/`IntegerType`/`LongType`/`BoolType` 归一成 f64。
fn num_attr(map: &NamedAttrMap, name: &str) -> Option<f64> {
    match map.get(name)? {
        NamedAttrValue::F32Type(v) => Some(*v as f64),
        NamedAttrValue::IntegerType(v) => Some(*v as f64),
        NamedAttrValue::LongType(v) => Some(*v as f64),
        NamedAttrValue::BoolType(v) => Some(if *v { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn int_attr(map: &NamedAttrMap, name: &str) -> Option<i64> {
    match map.get(name)? {
        NamedAttrValue::IntegerType(v) => Some(*v as i64),
        NamedAttrValue::LongType(v) => Some(*v),
        NamedAttrValue::F32Type(v) => Some(*v as i64),
        _ => None,
    }
}

/// 数值答案格式化：`15.0` → `"15"`（CHOICE 串约定不带小数尾巴）。
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, NamedAttrValue)]) -> NamedAttrMap {
        NamedAttrMap {
            map: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn question_key_encoding() {
        assert_eq!(question_key("STYP", 0), "STYP");
        assert_eq!(question_key("PBOR", 0), "PBOR0");
        assert_eq!(question_key("PBOR", 1), "PBOR1");
        assert_eq!(question_key("SHOP", 0), "SHOP");
        assert_eq!(question_key("TYPE", 0), "TYPE");
    }

    #[test]
    fn member_match_text_any_and_range() {
        let any = map(&[("TANS", NamedAttrValue::StringType("ANY".into()))]);
        assert!(member_matches(&any, "whatever"));

        let text = map(&[("TANS", NamedAttrValue::StringType("BW".into()))]);
        assert!(member_matches(&text, "BW"));
        assert!(!member_matches(&text, "SW"));

        let range = map(&[
            ("ANSW", NamedAttrValue::F32Type(100.0)),
            ("MAXA", NamedAttrValue::F32Type(150.0)),
        ]);
        assert!(member_matches(&range, "100"));
        assert!(member_matches(&range, "150"));
        assert!(member_matches(&range, "125"));
        assert!(!member_matches(&range, "200"));
        // 带 TANS 的节点不走数值面
        let both = map(&[
            ("TANS", NamedAttrValue::StringType("TUBE".into())),
            ("ANSW", NamedAttrValue::F32Type(0.0)),
        ]);
        assert!(member_matches(&both, "TUBI")); // 别名命中
        assert!(!member_matches(&both, "0"));
    }

    #[test]
    fn member_default_match() {
        let member = map(&[("TANS", NamedAttrValue::StringType("A".into()))]);
        assert!(member_is_default(&member, None, Some("A")));
        assert!(!member_is_default(&member, None, Some("B")));
        let num = map(&[("ANSW", NamedAttrValue::F32Type(15.0))]);
        assert!(member_is_default(&num, Some(15.0), None));
        assert!(!member_is_default(&num, Some(16.0), None));
    }

    #[test]
    fn num_fmt_drops_decimal_tail() {
        assert_eq!(fmt_num(15.0), "15");
        assert_eq!(fmt_num(2.5), "2.5");
    }
}
