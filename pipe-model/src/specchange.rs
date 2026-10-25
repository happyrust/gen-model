//! F 组：规格/孔径变更（spec Phase C；功能文档 §12、§16.15 specborechange）。
//!
//! 语义要点：
//! - 四模式：换规格 / 改孔径 / 保温 / 伴热；本实现覆盖**换规格**主链，
//!   改孔径 = 调用方把 PBOR 槽位改写进答卷再走同一链（§18.6 第二级），
//!   保温/伴热走 [`crate::service::PipeModel::update_process_data`] 级联面。
//! - `spref=` 直写命中即写，落空走 `RESELECT WITH SPEC` 答卷回落
//!   （回落产物 = [`ComponentOutcome::SelectorFallback`]，全落空 `NeedsSelection`）
//! - 答卷来源：`SpecCatalog::derive_answers` 沿现行 SPCO 的 SELE 祖先链
//!   运行时反推（§18.8）；推不出 → 该件 `NeedsSelection`
//! - ATTA 挂点件变更期 `SPKBRK=TRUE` 钉住、写完复位（Story 21）
//! - TEE/OLET/VTWA off-leg：`offleg = 6-(arrive+leave)`（Story 22）；回写被接
//!   支管 HBOR/HPOS 需要跨支管连接读面——当前写 spare P 点号并 warning
//! - BEND 换规格后 RADI 自洽提醒（Story 20）
//! - 逐件结果 [`SpecChangeResult`] 供 Story 32 三态着色（可改/不可改/已改）

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::outcome::{ComponentOutcome, PipeWarning};
use crate::service::PipeModelService;
use crate::spec_resolver;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// 逐件结果（Story 32 的"可改/不可改/已改"数据面）。
#[derive(Debug)]
pub struct SpecChangeResult {
    pub element: RefU64,
    pub noun: String,
    pub outcome: ComponentOutcome,
}

fn set_attr(element: RefU64, name: &str, value: NamedAttrValue) -> MutationOp {
    let mut attrs = NamedAttrMap::default();
    attrs.insert(name.into(), value);
    MutationOp::SetAttrs { element, attrs }
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 换规格主链。element 可 PIPE（级联全部支管）/BRAN/单件。
    pub(crate) fn change_spec_impl(
        &self,
        _ctx: &Context,
        element: RefU64,
        new_spec: RefU64,
    ) -> Result<(MutationBatch, Vec<SpecChangeResult>), PipeError> {
        let noun = self.catalog.noun_of(element).ok_or_else(|| PipeError::Blocking {
            message: format!("{element:?} 不在数据面"),
        })?;
        if self.spec.is_none() {
            return Err(PipeError::Blocking {
                message: "规格库读面未接入（with_spec）".into(),
            });
        }
        let mut batch = MutationBatch::named("change_spec");
        let mut results = Vec::new();
        match noun.as_str() {
            "PIPE" => {
                batch.ops.push(set_attr(
                    element,
                    "PSPE",
                    NamedAttrValue::RefU64Type(new_spec),
                ));
                for bran in self.catalog.members_of(element) {
                    if self.catalog.noun_of(bran).as_deref() == Some("BRAN") {
                        self.rebranch_spec(&mut batch, &mut results, bran, new_spec);
                    }
                }
            }
            "BRAN" => self.rebranch_spec(&mut batch, &mut results, element, new_spec),
            _ => {
                let r = self.reselect_one(&mut batch, element, &noun, new_spec);
                results.push(r);
            }
        }
        Ok((batch, results))
    }

    /// 支管换规格：写 PSPE → 重选 HSTU（待内核，warning）→ 逐件重选。
    fn rebranch_spec(
        &self,
        batch: &mut MutationBatch,
        results: &mut Vec<SpecChangeResult>,
        bran: RefU64,
        new_spec: RefU64,
    ) {
        batch.ops.push(set_attr(bran, "PSPE", NamedAttrValue::RefU64Type(new_spec)));
        batch.warnings.push(PipeWarning {
            message: "BRAN 换 PSPE 后的 HSTU 重选待内核（Story 6/28 sel hstu 语义）".into(),
            side_effect: None,
        });
        for member in self.catalog.members_of(bran) {
            let noun = self.catalog.noun_of(member).unwrap_or_default();
            if matches!(noun.as_str(), "TUBI") {
                continue; // 直管段不参与 SPCO 重选
            }
            let r = self.reselect_one(batch, member, &noun, new_spec);
            results.push(r);
        }
    }

    /// 逐件重选：`SPREF 直写` → 落空走答卷回落。ATTA 先钉后解。
    fn reselect_one(
        &self,
        batch: &mut MutationBatch,
        el: RefU64,
        noun: &str,
        new_spec: RefU64,
    ) -> SpecChangeResult {
        let spec = self.spec.as_deref().expect("checked in caller");

        // Story 21：ATTA 变更期钉住防漂移。
        let pinned = noun == "ATTA";
        if pinned {
            batch.ops.push(set_attr(el, "SPKBRK", NamedAttrValue::BoolType(true)));
        }

        // 答卷来源：现行 SPREF→SPCO 的 SELE 祖先链反推（§18.8）。
        let qa = self
            .catalog
            .attr(el, "SPREF")
            .and_then(|v| match v {
                NamedAttrValue::RefU64Type(r) => Some(r),
                _ => None,
            })
            .and_then(|spco| spec.derive_answers(spco));
        let Some(qa) = qa else {
            if pinned {
                batch.ops.push(set_attr(el, "SPKBRK", NamedAttrValue::BoolType(false)));
            }
            return SpecChangeResult {
                element: el,
                noun: noun.into(),
                outcome: ComponentOutcome::NeedsSelection,
            };
        };

        let outcome = spec_resolver::resolve(
            spec,
            new_spec,
            spec.root_selector(new_spec),
            noun,
            &qa,
        );
        match &outcome {
            ComponentOutcome::Direct { spco }
            | ComponentOutcome::SelectorFallback { spco, .. } => {
                batch.ops.push(set_attr(el, "SPREF", NamedAttrValue::RefU64Type(*spco)));
            }
            ComponentOutcome::NeedsSelection => {}
        }
        if matches!(outcome, ComponentOutcome::SelectorFallback { .. }) {
            batch.warnings.push(PipeWarning {
                message: format!("{noun} {el:?} 表查落空，经 SELE 回落命中"),
                side_effect: None,
            });
        }

        // Story 22：TEE/OLET/VTWA off-leg P 点号 = 6-(arrive+leave)。
        if matches!(noun, "TEE" | "OLET" | "VTWA") {
            let int = |name: &str| match self.catalog.attr(el, name) {
                Some(NamedAttrValue::IntegerType(i)) => Some(i),
                _ => None,
            };
            match (int("ARIVE"), int("LEAVE")) {
                (Some(a), Some(l)) => batch.warnings.push(PipeWarning {
                    message: format!(
                        "{noun} off-leg P 点号 = 6-({a}+{l}) = {}；被接支管 HBOR/HPOS 回写待跨支管读面",
                        6 - (a + l)
                    ),
                    side_effect: None,
                }),
                _ => batch.warnings.push(PipeWarning {
                    message: format!("{noun} 缺 ARIVE/LEAVE，off-leg 推不出"),
                    side_effect: None,
                }),
            }
        }
        // Story 20 尾段：BEND 半径自洽。
        if noun == "BEND" && self.catalog.attr(el, "RADI").is_none() {
            batch.warnings.push(PipeWarning {
                message: "BEND 换规格后缺 RADI 可读值，半径自洽校验跳过".into(),
                side_effect: None,
            });
        }

        if pinned {
            batch.ops.push(set_attr(el, "SPKBRK", NamedAttrValue::BoolType(false)));
        }
        SpecChangeResult { element: el, noun: noun.into(), outcome }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog, MockSpec};

    fn rig() -> (MockCatalog, MockSpec) {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        cat.owners.insert(RefU64(10), RefU64(9));
        // 元件：TUBI(跳过) + ELBO(有现行 SPREF) + ATTA(钉住路径) + TEE(off-leg)。
        for (r, n) in [
            (RefU64(21), "TUBI"),
            (RefU64(22), "ELBO"),
            (RefU64(23), "ATTA"),
            (RefU64(24), "TEE"),
        ] {
            cat.nouns.insert(r, n.into());
            cat.owners.insert(r, RefU64(11));
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        cat.attrs.insert(RefU64(22), std::collections::BTreeMap::from([
            ("SPREF".into(), NamedAttrValue::RefU64Type(RefU64(700))),
        ]));
        cat.attrs.insert(RefU64(23), std::collections::BTreeMap::from([
            ("SPREF".into(), NamedAttrValue::RefU64Type(RefU64(700))),
        ]));
        cat.attrs.insert(RefU64(24), std::collections::BTreeMap::from([
            ("SPREF".into(), NamedAttrValue::RefU64Type(RefU64(700))),
            ("ARIVE".into(), NamedAttrValue::IntegerType(1)),
            ("LEAVE".into(), NamedAttrValue::IntegerType(2)),
        ]));
        let mut spec = MockSpec::default();
        // 现行 SPCO 700 的答卷反推。
        spec.answers_map.insert(
            RefU64(700),
            vec![("PBOR0".into(), "100".into()), ("STYP".into(), "EQUAL".into())],
        );
        (cat, spec)
    }

    #[test]
    fn pipe_change_spec_cascades_branches_and_components() {
        let (cat, mut spec) = rig();
        // 新规格 88：ELBO/ATTA 直查命中、TEE 落空。
        spec.table.insert((RefU64(88), "ELBO".into()), RefU64(801));
        spec.table.insert((RefU64(88), "ATTA".into()), RefU64(802));
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (batch, results) = svc.change_spec(&ctx(), RefU64(10), RefU64(88)).unwrap();

        // PIPE.PSPE + BRAN.PSPE 都写。
        let pspe_writes: Vec<RefU64> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, attrs }
                    if attrs.contains_key("PSPE") =>
                {
                    Some(*element)
                }
                _ => None,
            })
            .collect();
        assert_eq!(pspe_writes, vec![RefU64(10), RefU64(11)]);

        // 逐件结果：TUBI 跳过；ELBO/ATTA Direct；TEE NeedsSelection。
        assert_eq!(results.len(), 3);
        let get = |el: RefU64| results.iter().find(|r| r.element == el).unwrap();
        assert!(matches!(get(RefU64(22)).outcome, ComponentOutcome::Direct { .. }));
        assert!(matches!(get(RefU64(23)).outcome, ComponentOutcome::Direct { .. }));
        assert!(matches!(get(RefU64(24)).outcome, ComponentOutcome::NeedsSelection));

        // ATTA：SPKBRK TRUE 在其 SPREF 写前、FALSE 在后。
        let seq: Vec<(RefU64, String)> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, attrs } => {
                    Some((*element, attrs.keys().next().cloned().unwrap_or_default()))
                }
                _ => None,
            })
            .collect();
        let atta: Vec<&String> = seq
            .iter()
            .filter(|(e, _)| *e == RefU64(23))
            .map(|(_, k)| k)
            .collect();
        assert_eq!(atta, ["SPKBRK", "SPREF", "SPKBRK"]);

        // TEE：off-leg P 点号 warning（6-(1+2)=3）。
        assert!(batch.warnings.iter().any(|w| w.message.contains("= 3")));
    }

    #[test]
    fn component_change_spec_direct() {
        let (cat, mut spec) = rig();
        spec.table.insert((RefU64(88), "ELBO".into()), RefU64(801));
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (_batch, results) = svc.change_spec(&ctx(), RefU64(22), RefU64(88)).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].outcome, ComponentOutcome::Direct { spco } if spco == RefU64(801)));
    }

    #[test]
    fn no_spec_catalog_blocks() {
        let (cat, _spec) = rig();
        let svc = PipeModelService::new(cat);
        assert!(matches!(
            svc.change_spec(&ctx(), RefU64(10), RefU64(88)).unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }
}
