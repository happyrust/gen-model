//! C 组：元件创建/重选/翻转（spec Phase B/C；功能文档 §16.4、§18.6-§18.8）。
//!
//! 关键语义：
//! - 建件 = `SELECT WITH <CHOICE>` 命令串 + 成组插入（TUBI 宿主反解、P 点定位）
//! - 答卷 `questions/answers` 是调用参数，不落库；命中后可按 questionIndex
//!   规则自导出（§18.8）
//! - 定位三选一：Connect / At Position / 3D Position；snapDist/minTube 走
//!   [`crate::settings::PipingSettings`]
//! - flip 三态：orientation/position/flip 控件分别改 ANGL、ARIVE/LEAVE 交换
//!
//! 实现边界：内核推导项（Connect 定位算点、3D 轴线求交吸附、LSTU 几何）在本批
//! 以显式 warning 标出——批本身只承载属主关系 + SPREF + 可推导属性。

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::choice::QuestionAnswers;
use crate::context::Context;
use crate::error::PipeError;
use crate::outcome::{ComponentOutcome, PipeWarning};
use crate::service::PipeModelService;
use crate::spec_resolver;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// 元件选择载荷（Create 表单提交）。
#[derive(Debug, Clone)]
pub struct ComponentSelect {
    /// 元件类型名（ELBO/TEE/FLAN/VALV…，= E3D 新建元素的 noun）。
    pub comp_type: String,
    /// 答卷（`PBOR0`/`STYP`/`ANGL`… 规范键值对）。
    pub qa: QuestionAnswers,
    /// 定位模式。
    pub position: ComponentPosition,
    /// 逆流建件（表单 With/Against Flow 钮的显式覆盖；
    /// 全局方向在 [`Context::flow`]）。
    pub against_flow: bool,
}

/// 元件定位（Component Editor 三定位钮，Story 16）。
#[derive(Debug, Clone)]
pub enum ComponentPosition {
    /// `CONN` 到前驱件/支管端 P 点。
    Connect { target: RefU64, p_point: u32 },
    /// `AT POSITION` 沿支管轴偏移 mm。
    AtPosition { offset_mm: f64 },
    /// 自由 3D 点（宿主反解 TUBI + 距邻件 < snapDist 自动吸附）。
    At3D { position: [f64; 3] },
}

fn set_attr(element: RefU64, name: &str, value: NamedAttrValue) -> MutationOp {
    let mut attrs = NamedAttrMap::default();
    attrs.insert(name.into(), value);
    MutationOp::SetAttrs { element, attrs }
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 建件（Story 15/16/19）：答卷 → SPCO → 定向三段，结果显式分档。
    pub(crate) fn create_component_impl(
        &self,
        _ctx: &Context,
        branch: RefU64,
        sel: &ComponentSelect,
    ) -> Result<(MutationBatch, ComponentOutcome), PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking {
                message: "create_component 宿主必须 BRAN".into(),
            });
        }
        if !self.catalog.writable(branch) {
            return Err(PipeError::Blocking { message: "BRAN 无写权限".into() });
        }
        let Some(spref) = self
            .catalog
            .attr(branch, "PSPE")
            .and_then(|v| match v {
                NamedAttrValue::RefU64Type(r) => Some(r),
                _ => None,
            })
        else {
            return Err(PipeError::Blocking {
                message: "BRAN 无 PSPE，无法解析元件规格".into(),
            });
        };
        let spec = self.spec.as_deref().ok_or_else(|| PipeError::Blocking {
            message: "规格库读面未接入（with_spec）".into(),
        })?;

        let mut batch = MutationBatch::named("create_component");
        let outcome = spec_resolver::resolve(
            spec,
            spref,
            spec.root_selector(spref),
            &sel.comp_type,
            &sel.qa,
        );
        let spco = match outcome {
            ComponentOutcome::Direct { spco } | ComponentOutcome::SelectorFallback { spco, .. } => {
                spco
            }
            ComponentOutcome::NeedsSelection => {
                batch.warnings.push(PipeWarning {
                    message: "SPCO 表查与 SELE 回落全落空——需要 Select Specification 手动答卷"
                        .into(),
                    side_effect: None,
                });
                return Ok((batch, ComponentOutcome::NeedsSelection));
            }
        };
        if let ComponentOutcome::SelectorFallback { via, .. } = &outcome {
            batch.warnings.push(PipeWarning {
                message: format!("SPCO 表直查落空，经 SELE 回落命中：{via}"),
                side_effect: None,
            });
        }

        let el = self.catalog.alloc_refno(branch);
        let mut attrs = NamedAttrMap::default();
        // 命中件写 SPREF（规格元件指针）；CATREF/PTSE/GMSE 由持久层按 SPCO 解出。
        attrs.insert("SPREF".into(), NamedAttrValue::RefU64Type(spco));
        match &sel.position {
            ComponentPosition::AtPosition { offset_mm } => {
                attrs.insert("APOS".into(), NamedAttrValue::F32Type(*offset_mm as f32));
            }
            ComponentPosition::Connect { .. } | ComponentPosition::At3D { .. } => {}
        }
        batch.ops.push(MutationOp::Create {
            refno: el,
            noun: sel.comp_type.clone(),
            under: branch,
            name: None,
            attrs,
        });
        match &sel.position {
            ComponentPosition::Connect { target, p_point } => {
                batch.warnings.push(PipeWarning {
                    message: format!(
                        "CONN 定位（P{p_point} of {target:?}）待连接内核：当前只挂成员+SPREF"
                    ),
                    side_effect: None,
                });
            }
            ComponentPosition::At3D { .. } => {
                batch.warnings.push(PipeWarning {
                    message: format!(
                        "3D 点定位待内核（TUBI 反解+snapDist={} 吸附）",
                        self.settings.snap_dist
                    ),
                    side_effect: None,
                });
            }
            ComponentPosition::AtPosition { .. } => {}
        }
        if sel.against_flow {
            batch.warnings.push(PipeWarning {
                message: "逆流建件：孔径不等时 arrive/leave 交换待内核判定（Story 19）".into(),
                side_effect: None,
            });
        }
        Ok((batch, outcome))
    }

    /// 重选（Story 20）：`spref=` 直写语义 = 命中即写；落空走答卷回落，
    /// 两端都空 → `NeedsSelection`（批内无 op，UI 开选择器）。
    ///
    /// BEND 半径自洽校验需要 LSTU 几何——当前只在读面能给 RADI 时做存在性
    /// 提醒，内核接入后换真比对。
    pub(crate) fn reselect_component_impl(
        &self,
        _ctx: &Context,
        component: RefU64,
        qa: &QuestionAnswers,
    ) -> Result<(MutationBatch, ComponentOutcome), PipeError> {
        let noun = self.catalog.noun_of(component).ok_or_else(|| PipeError::Blocking {
            message: format!("{component:?} 不在数据面"),
        })?;
        let owner = self.catalog.owner_of(component).ok_or_else(|| PipeError::Blocking {
            message: "元件无属主".into(),
        })?;
        let Some(spref) = self
            .catalog
            .attr(owner, "PSPE")
            .and_then(|v| match v {
                NamedAttrValue::RefU64Type(r) => Some(r),
                _ => None,
            })
        else {
            return Err(PipeError::Blocking {
                message: "属主无 PSPE，无法解析重选规格".into(),
            });
        };
        let spec = self.spec.as_deref().ok_or_else(|| PipeError::Blocking {
            message: "规格库读面未接入（with_spec）".into(),
        })?;

        let mut batch = MutationBatch::named("reselect_component");
        let outcome = spec_resolver::resolve(
            spec,
            spref,
            spec.root_selector(spref),
            &noun,
            qa,
        );
        match outcome {
            ComponentOutcome::Direct { spco }
            | ComponentOutcome::SelectorFallback { spco, .. } => {
                batch.ops.push(set_attr(
                    component,
                    "SPREF",
                    NamedAttrValue::RefU64Type(spco),
                ));
                if noun == "BEND" && self.catalog.attr(component, "RADI").is_none() {
                    batch.warnings.push(PipeWarning {
                        message: "BEND 重选后缺 RADI 可读值，半径自洽校验跳过（Story 20）".into(),
                        side_effect: None,
                    });
                }
            }
            ComponentOutcome::NeedsSelection => {
                batch.warnings.push(PipeWarning {
                    message: "重选落空：表查与 SELE 链均未命中，需手动答卷".into(),
                    side_effect: None,
                });
            }
        }
        Ok((batch, outcome))
    }

    /// 单件翻转（Story 18 属性面）：ARIVE/LEAVE 互换 + ANGL+180°。
    ///
    /// LSTU 前驱传名与规格侧 `sel lstu` 属内核/规格面——读面拿得到就换值，
    /// 拿不到按 §决策 10 出 warning 不吞。
    pub(crate) fn flip_component_impl(
        &self,
        _ctx: &Context,
        component: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(component).is_none() {
            return Err(PipeError::Blocking {
                message: format!("{component:?} 不在数据面"),
            });
        }
        let mut batch = MutationBatch::named("flip_component");
        let arrive = self.catalog.attr(component, "ARIVE");
        let leave = self.catalog.attr(component, "LEAVE");
        match (arrive, leave) {
            (Some(a), Some(l)) => {
                batch.ops.push(set_attr(component, "ARIVE", l));
                batch.ops.push(set_attr(component, "LEAVE", a));
            }
            _ => batch.warnings.push(PipeWarning {
                message: "ARIVE/LEAVE 缺值：P 点对调跳过（多 LSTU 规格须手动重选，Story 18）".into(),
                side_effect: None,
            }),
        }
        let angl = match self.catalog.attr(component, "ANGL") {
            Some(NamedAttrValue::F32Type(d)) => (d + 180.0) % 360.0,
            _ => 180.0,
        };
        batch.ops.push(set_attr(component, "ANGL", NamedAttrValue::F32Type(angl)));
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog, MockSpec};
    use std::collections::BTreeMap;

    fn setup() -> (MockCatalog, MockSpec, PipeModelService<MockCatalog>) {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        let spec = MockSpec::default();
        (cat, spec, PipeModelService::new(MockCatalog::new()))
    }

    fn sel(comp_type: &str) -> ComponentSelect {
        ComponentSelect {
            comp_type: comp_type.into(),
            qa: vec![("PBOR0".into(), "100".into()), ("STYP".into(), "EQUAL".into())],
            position: ComponentPosition::AtPosition { offset_mm: 500.0 },
            against_flow: false,
        }
    }

    #[test]
    fn create_component_direct_hit() {
        let (cat, mut spec, _) = setup();
        spec.table.insert((RefU64(77), "ELBO".into()), RefU64(700));
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (batch, outcome) = svc
            .create_component(&ctx(), RefU64(11), &sel("ELBO"))
            .unwrap();
        assert!(matches!(outcome, ComponentOutcome::Direct { spco } if spco == RefU64(700)));
        let MutationOp::Create { noun, under, attrs, refno, .. } = &batch.ops[0] else { panic!() };
        assert!(refno.0 >= 1_000_000, "新元件 refno 落在编辑段");
        assert_eq!(noun, "ELBO");
        assert_eq!(*under, RefU64(11));
        assert_eq!(attrs.get("SPREF"), Some(&NamedAttrValue::RefU64Type(RefU64(700))));
        assert_eq!(attrs.get("APOS"), Some(&NamedAttrValue::F32Type(500.0)));
    }

    #[test]
    fn create_component_sele_fallback() {
        let (cat, mut spec, _) = setup();
        // 表查落空 → SELE 链 root(80) → SPCO(701)。
        spec.roots.insert(RefU64(77), RefU64(80));
        spec.sele_next.insert(RefU64(80), RefU64(701));
        spec.spco_set.insert(RefU64(701));
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (batch, outcome) = svc.create_component(&ctx(), RefU64(11), &sel("TEE")).unwrap();
        assert!(matches!(
            outcome,
            ComponentOutcome::SelectorFallback { spco, .. } if spco == RefU64(701)
        ));
        // 回落命中仍建件，但批里带 warning（§决策 10）。
        assert!(matches!(batch.ops[0], MutationOp::Create { .. }));
        assert!(!batch.warnings.is_empty());
    }

    #[test]
    fn create_component_needs_selection() {
        let (cat, spec, _) = setup();
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (batch, outcome) = svc.create_component(&ctx(), RefU64(11), &sel("TEE")).unwrap();
        assert!(matches!(outcome, ComponentOutcome::NeedsSelection));
        assert!(batch.ops.is_empty(), "落空不建件");
    }

    #[test]
    fn create_component_requires_spec_catalog() {
        let (cat, _spec, _) = setup();
        let svc = PipeModelService::new(cat); // 不接 with_spec
        assert!(matches!(
            svc.create_component(&ctx(), RefU64(11), &sel("ELBO")).unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }

    #[test]
    fn reselect_writes_spref_on_hit() {
        let (mut cat, mut spec, _) = setup();
        cat.nouns.insert(RefU64(20), "FLAN".into());
        cat.owners.insert(RefU64(20), RefU64(11));
        spec.table.insert((RefU64(77), "FLAN".into()), RefU64(702));
        let svc = PipeModelService::new(cat).with_spec(spec);
        let (batch, outcome) = svc
            .reselect_component(&ctx(), RefU64(20), &vec![("PBOR0".into(), "100".into())])
            .unwrap();
        assert!(matches!(outcome, ComponentOutcome::Direct { .. }));
        let MutationOp::SetAttrs { element, attrs } = &batch.ops[0] else { panic!() };
        assert_eq!(*element, RefU64(20));
        assert_eq!(attrs.get("SPREF"), Some(&NamedAttrValue::RefU64Type(RefU64(702))));
    }

    #[test]
    fn flip_swaps_arrive_leave_and_rotates() {
        let (mut cat, _spec, _) = setup();
        cat.nouns.insert(RefU64(20), "TEE".into());
        cat.owners.insert(RefU64(20), RefU64(11));
        cat.attrs.insert(RefU64(20), BTreeMap::from([
            ("ARIVE".into(), NamedAttrValue::IntegerType(1)),
            ("LEAVE".into(), NamedAttrValue::IntegerType(2)),
            ("ANGL".into(), NamedAttrValue::F32Type(30.0)),
        ]));
        let svc = PipeModelService::new(cat);
        let batch = svc.flip_component(&ctx(), RefU64(20)).unwrap();
        let get = |name: &str| {
            batch.ops.iter().find_map(|op| match op {
                MutationOp::SetAttrs { attrs, .. } => attrs.get(name).cloned(),
                _ => None,
            })
        };
        assert_eq!(get("ARIVE"), Some(NamedAttrValue::IntegerType(2)));
        assert_eq!(get("LEAVE"), Some(NamedAttrValue::IntegerType(1)));
        assert_eq!(get("ANGL"), Some(NamedAttrValue::F32Type(210.0)));
    }
}
