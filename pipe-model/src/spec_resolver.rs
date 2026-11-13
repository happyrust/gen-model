//! 规格元件解析器（spec §5 决策 6；功能文档 §18.6/§18.8）。
//!
//! E3D 三级回落：SPCO 表直查 → PBOR 槽位归一 → SELE default/tdefault/EQUA 链。
//! 查表与 SELE 遍历要碰规格库（只读），由调用方注入 [`SpecCatalog`]；
//! 本 crate 编排回落顺序与 PBOR 槽位语义。

use crate::choice::QuestionAnswers;
use crate::outcome::ComponentOutcome;
use aios_core::RefU64;

/// 规格库只读门面。实现方负责把 SPCO 表/SELE 树从数据面读出来。
pub trait SpecCatalog {
    /// SPREF 的 SELE 树根（`loadTable` 失败后的回落入口）。
    /// `None` = 该规格无选择器树，三级回落无从谈起。
    fn root_selector(&self, spref: RefU64) -> Option<RefU64>;

    /// SPCO 表直查：`componentTable.components(questions, answers)`。
    /// 入参为规范键值对（见 [`crate::choice`]）。命中返回 SPCO refno。
    /// 实现方负责内部归一细节（TEE 老规格 `answers[2]=BORE(1)` 兜底重查等）。
    fn table_lookup(&self, spref: RefU64, comp_type: &str, qa: &QuestionAnswers) -> Option<RefU64>;

    /// SELE 树遍历：从 `selector` 节点按 (question, qualifier) 键匹配
    /// `answer` 下钻；无匹配走 `default`/`tdefault`（`tdefault='EQUA'` 时
    /// 走 member[1] 等值链）。返回命中的下一节点（SELE 或 SPCO 叶子）。
    fn sele_descend(&self, selector: RefU64, qa: &QuestionAnswers) -> Option<RefU64>;

    /// 节点是否 SPCO 叶子。
    fn is_spco(&self, node: RefU64) -> bool;

    /// 从 SPCO 反推答卷（§18.8 `questionIndex`）：沿 `ownLst` 祖先链逐节点取
    /// `owner.question & owner.qualifier` 为键、`tAnswer`（空则 `answer`）为值。
    /// questions/answers 不落库——这是运行时重建。`None` = SPCO 无 SELE 祖先
    /// 可推（此时换规格只能走手动答卷）。
    fn derive_answers(&self, _spco: RefU64) -> Option<QuestionAnswers> {
        None
    }
}

/// 三级回落解析（§18.6 `getSpecComponent`→`getDefaultComponent`→`getBoreSelector`）。
///
/// 1. 表直查前先 `PBOR0→PBOR1` 归一；TEE 老规格查空时以 `answers[2]=BORE(1)`
///    兜底重查（由实现方在 `table_lookup` 内处理）
/// 2. 改孔径 = 改 `answers[1]`；TEE/OLET/VTWA arrive=3 时 `answers[2]` 同步
/// 3. 表空走 SELE 链；到 SPCO 叶子即 `SelectorFallback`，走尽未命中 `NeedsSelection`
pub fn resolve(
    cat: &dyn SpecCatalog,
    spref: RefU64,
    root_sele: Option<RefU64>,
    comp_type: &str,
    qa: &QuestionAnswers,
) -> ComponentOutcome {
    // 第一级：表直查（PBOR0→PBOR1 归一在此做，归一后的 qa 副本留给 SELE 链复用）。
    let mut qa = qa.clone();
    if let Some(pos) = qa.iter().position(|(k, _)| k == "PBOR0") {
        qa[pos].0 = "PBOR1".into();
    }
    if let Some(spco) = cat.table_lookup(spref, comp_type, &qa) {
        return ComponentOutcome::Direct { spco };
    }
    // 第二级（PBOR 槽位改写）属于调用方对 qa 的预处理，不在这里展开；
    // 第三级：SELE 默认链。
    let mut node = root_sele;
    let mut via = Vec::new();
    while let Some(sele) = node {
        match cat.sele_descend(sele, &qa) {
            Some(next) if cat.is_spco(next) => {
                return ComponentOutcome::SelectorFallback {
                    spco: next,
                    via: via.join(" → "),
                };
            }
            Some(next) => {
                via.push(format!("SELE:{:?}", next));
                node = Some(next);
            }
            None => break,
        }
    }
    ComponentOutcome::NeedsSelection
}
