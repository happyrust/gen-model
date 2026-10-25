//! H 组：焊接（spec Phase D；功能文档 §14、§16.16、§18.7 pfModifyWelds）。
//!
//! 语义要点：
//! - `NEW WELD CHOOSE SPEC <spco> SHOP T|F STYP <styp>` 每缝一句；
//!   **无规格路径降级**（Story 35）：`SELECT SPEC /WELDGAP` 选不出 → 实测 gap
//!   （`aPos.distance(lPos)`）写 `DESP` 数组第 1 槽
//! - Story 36：规格选不出 → 去掉 SHOP 条件重试一次；仍失败收集为行级
//!   warning 不中断批
//! - Story 37：超规格焊缝统计（`spref of weld ≠ pspec of pipe`）随批回传——
//!   这里用 warning 计数承载；excess 超限回滚由调用方按 UNDODB 语义整批放弃
//! - `wldNumber=0` 标记手工焊；非 upDate 模式全量 `renumberAllWelds` 语义在
//!   [`PipeModel::renumber_welds`]
//!
//! 焊缝落点：相邻成员对（tube↔component 接合处）。本 crate 不做几何判断——
//! 每对相邻非 TUBI 接合写一条 WELD；PTAP/ATTA 类跨支管接合待内核读面。

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::outcome::PipeWarning;
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// 焊接策略（Weld Handling 表单提交面）。
#[derive(Debug, Clone)]
pub struct WeldPolicy {
    /// `SHOP T|F`：预制焊缝（SHOP）还是合拢焊缝（SITE/FIELD）。
    pub shop: bool,
    /// `STYP <type>`：焊缝型（如 BW/SW/FW）；空 = 不写。
    pub styp: Option<String>,
    /// CHOOSE SPEC 目标（焊缝规格元件）；`None` = 无规格路径降级
    /// （实测 gap 写 DESP 第 1 槽）。
    pub weld_spec: Option<RefU64>,
    /// `wldNumber` 起始号；0 = 手工焊标记（§16.16）。
    pub first_number: u32,
}

impl Default for WeldPolicy {
    /// E3D 默认：SHOP TRUE（预制视角），自动编号从 1。
    fn default() -> Self {
        Self { shop: true, styp: None, weld_spec: None, first_number: 1 }
    }
}

/// 焊件批量属性编辑（`pfModifyWelds` 提交面；§18.16）。
///
/// E3D `changeAttributes(weldList, shop, machType, fromSpec, weldType)`——
/// Weld Type `SHOP/FIELD/SITE` 映射 `WELDTY` 词 + `SHOP` 布尔成对写；
/// `machType`（MANU/MACH/ORBI）在 WELD 词表无对应列（取证 noun_layout），
/// 属 Fab.Type 内核/UDF 面，显式 warning 不伪造。
#[derive(Debug, Clone, Default)]
pub struct WeldAttrEdit {
    /// `WELDTY` 词：SHOP/FIELD/SITE；`None` 不改。
    pub weld_type: Option<String>,
    /// Fab.Type `MANU/MACH/ORBI`；`None` 不改。无落库列，写 warning。
    pub mach_type: Option<String>,
}

/// `WELDTY` 词 → `SHOP` 布尔（pfModifyWelds：SHOP→true，其余→false）。
pub fn weld_type_shop(weld_type: &str) -> bool {
    weld_type.eq_ignore_ascii_case("SHOP")
}

fn weld_attrs(policy: &WeldPolicy, gap: Option<f32>, number: u32) -> NamedAttrMap {
    let mut attrs = NamedAttrMap::default();
    attrs.insert("SHOP".into(), NamedAttrValue::BoolType(policy.shop));
    if let Some(styp) = &policy.styp {
        attrs.insert("STYP".into(), NamedAttrValue::WordType(styp.clone()));
    }
    match policy.weld_spec {
        Some(spco) => {
            attrs.insert("SPREF".into(), NamedAttrValue::RefU64Type(spco));
        }
        None => {
            // 降级路径：实测 gap 写 DESP NUM 1。
            attrs.insert(
                "DESP".into(),
                NamedAttrValue::F32VecType(vec![gap.unwrap_or(0.0)]),
            );
        }
    }
    attrs.insert("WLDNUM".into(), NamedAttrValue::IntegerType(number as i32));
    attrs
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 按边界自动造焊缝（Story 35-37）。
    pub(crate) fn create_welds_impl(
        &self,
        _ctx: &Context,
        pipe: RefU64,
        policy: &WeldPolicy,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(pipe).as_deref() != Some("PIPE") {
            return Err(PipeError::Blocking {
                message: "create_welds 目标必须 PIPE".into(),
            });
        }
        let mut batch = MutationBatch::named("create_welds");
        let mut number = policy.first_number;
        let mut joints = 0usize;

        for bran in self.catalog.members_of(pipe) {
            if self.catalog.noun_of(bran).as_deref() != Some("BRAN") {
                continue;
            }
            let members = self.catalog.members_of(bran);
            for pair in members.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                // TUBI-TUBI 相邻无焊口（同段直管）；其余相邻对都造缝。
                let (na, nb) = (
                    self.catalog.noun_of(a).unwrap_or_default(),
                    self.catalog.noun_of(b).unwrap_or_default(),
                );
                if na == "TUBI" && nb == "TUBI" {
                    continue;
                }
                joints += 1;
                // 无规格降级：实测 gap = APOS/LPOS 距（读不到按 0 + warning）。
                let gap = if policy.weld_spec.is_none() {
                    match (
                        self.catalog.attr(a, "APOS"),
                        self.catalog.attr(b, "LPOS"),
                    ) {
                        (Some(NamedAttrValue::F32Type(ap)), Some(NamedAttrValue::F32Type(lp))) => {
                            Some((lp - ap).abs())
                        }
                        _ => {
                            batch.warnings.push(PipeWarning {
                                message: format!("{na}→{nb} 缝：APOS/LPOS 缺，gap 按 0 写"),
                                side_effect: None,
                            });
                            None
                        }
                    }
                } else {
                    None
                };
                batch.ops.push(MutationOp::Create {
                    refno: self.catalog.alloc_refno(bran),
                    noun: "WELD".into(),
                    under: bran,
                    name: None,
                    attrs: weld_attrs(policy, gap, number),
                });
                if number > 0 {
                    number += 1;
                }
            }
        }

        if joints == 0 {
            batch.warnings.push(PipeWarning {
                message: "无可焊边界（支管无相邻元件对）".into(),
                side_effect: None,
            });
        }
        // Story 37 尾段：超规格统计——规格焊缝数 vs 管规格（PSPE 一致性检查
        // 需逐缝 SPREF↔PSPE 比，本批只记策略面）。
        if policy.weld_spec.is_some() {
            batch.warnings.push(PipeWarning {
                message: format!(
                    "共 {joints} 缝带 CHOOSE SPEC；超规格（spref≠pspec）复核在校验面"
                ),
                side_effect: None,
            });
        }
        Ok(batch)
    }

    /// 焊缝重编号（Story 38）：按支管→成员序扫 WELD 写 WLDNUM。
    /// `first_number=0` 时编号即手工焊标记——沿用 policy 约定，这里恒从 1。
    pub(crate) fn renumber_welds_impl(
        &self,
        _ctx: &Context,
        pipe: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(pipe).as_deref() != Some("PIPE") {
            return Err(PipeError::Blocking {
                message: "renumber_welds 目标必须 PIPE".into(),
            });
        }
        let mut batch = MutationBatch::named("renumber_welds");
        let mut n = 1i32;
        for bran in self.catalog.members_of(pipe) {
            if self.catalog.noun_of(bran).as_deref() != Some("BRAN") {
                continue;
            }
            for m in self.catalog.members_of(bran) {
                if self.catalog.noun_of(m).as_deref() == Some("WELD") {
                    let mut attrs = NamedAttrMap::default();
                    attrs.insert("WLDNUM".into(), NamedAttrValue::IntegerType(n));
                    batch.ops.push(MutationOp::SetAttrs { element: m, attrs });
                    n += 1;
                }
            }
        }
        Ok(batch)
    }

    /// 焊件批量属性编辑（Story 56）：`WELDTY` 词 + `SHOP` 布尔成对写；
    /// 非 WELD 元素跳过并记 warning；`machType` 无落库列显式标注。
    pub(crate) fn modify_welds_impl(
        &self,
        _ctx: &Context,
        welds: &[RefU64],
        edit: &WeldAttrEdit,
    ) -> Result<MutationBatch, PipeError> {
        let mut batch = MutationBatch::named("modify_welds");
        for w in welds {
            if self.catalog.noun_of(*w).as_deref() != Some("WELD") {
                batch.warnings.push(PipeWarning {
                    message: format!("{w:?} 非 WELD，跳过"),
                    side_effect: None,
                });
                continue;
            }
            let mut attrs = NamedAttrMap::default();
            if let Some(t) = &edit.weld_type {
                attrs.insert("WELDTY".into(), NamedAttrValue::WordType(t.clone()));
                attrs.insert(
                    "SHOP".into(),
                    NamedAttrValue::BoolType(weld_type_shop(t)),
                );
            }
            if attrs.map.is_empty() {
                continue;
            }
            batch.ops.push(MutationOp::SetAttrs { element: *w, attrs });
        }
        if let Some(mt) = &edit.mach_type {
            batch.warnings.push(PipeWarning {
                message: format!(
                    "Fab.Type {mt}（MANU/MACH/ORBI）在 WELD 词表无落库列，待内核/UDF 面确认"
                ),
                side_effect: None,
            });
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog};

    fn rig() -> PipeModelService<MockCatalog> {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        // TUBI(21) - ELBO(22) - TUBI(23)：两接合缝；TUBI-TUBI 单独测跳过。
        for (r, n) in [(RefU64(21), "TUBI"), (RefU64(22), "ELBO"), (RefU64(23), "TUBI")] {
            cat.nouns.insert(r, n.into());
            cat.owners.insert(r, RefU64(11));
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        cat.attrs.insert(RefU64(22), std::collections::BTreeMap::from([
            ("APOS".into(), NamedAttrValue::F32Type(100.0)),
        ]));
        cat.attrs.insert(RefU64(23), std::collections::BTreeMap::from([
            ("LPOS".into(), NamedAttrValue::F32Type(160.0)),
        ]));
        PipeModelService::new(cat)
    }

    #[test]
    fn creates_welds_at_joints_with_spec() {
        let svc = rig();
        let policy = WeldPolicy {
            shop: true,
            styp: Some("BW".into()),
            weld_spec: Some(RefU64(900)),
            first_number: 1,
        };
        let batch = svc.create_welds(&ctx(), RefU64(10), &policy).unwrap();
        let welds: Vec<&MutationOp> = batch
            .ops
            .iter()
            .filter(|o| matches!(o, MutationOp::Create { noun, .. } if noun == "WELD"))
            .collect();
        assert_eq!(welds.len(), 2, "TUBI-ELBO、ELBO-TUBI 两缝");
        let MutationOp::Create { attrs, .. } = welds[0] else { panic!() };
        assert_eq!(attrs.get("SHOP"), Some(&NamedAttrValue::BoolType(true)));
        assert_eq!(attrs.get("STYP"), Some(&NamedAttrValue::WordType("BW".into())));
        assert_eq!(attrs.get("SPREF"), Some(&NamedAttrValue::RefU64Type(RefU64(900))));
        assert_eq!(attrs.get("WLDNUM"), Some(&NamedAttrValue::IntegerType(1)));
    }

    #[test]
    fn no_spec_falls_back_to_gap_measurement() {
        let svc = rig();
        let policy = WeldPolicy { weld_spec: None, ..Default::default() };
        let batch = svc.create_welds(&ctx(), RefU64(10), &policy).unwrap();
        // 降级：DESP[1]=实测 gap。第一缝 APOS 缺→0+warning；第二缝 22→23 = |160-100|=60。
        let desps: Vec<f32> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::Create { attrs, .. } => match attrs.get("DESP") {
                    Some(NamedAttrValue::F32VecType(v)) => Some(v[0]),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(desps, vec![0.0, 60.0]);
        assert!(!batch.warnings.is_empty());
    }

    #[test]
    fn manual_weld_marks_wldnum_zero() {
        let svc = rig();
        let policy = WeldPolicy { first_number: 0, ..Default::default() };
        let batch = svc.create_welds(&ctx(), RefU64(10), &policy).unwrap();
        let nums: Vec<i32> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::Create { attrs, .. } => match attrs.get("WLDNUM") {
                    Some(NamedAttrValue::IntegerType(n)) => Some(*n),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(nums, vec![0, 0], "手工焊 wldNumber=0 不自增");
    }

    #[test]
    fn renumber_by_member_order() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        for (r, n) in [(RefU64(31), "WELD"), (RefU64(32), "TUBI"), (RefU64(33), "WELD")] {
            cat.nouns.insert(r, n.into());
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        let svc = PipeModelService::new(cat);
        let batch = svc.renumber_welds(&ctx(), RefU64(10)).unwrap();
        let nums: Vec<i32> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { attrs, .. } => match attrs.get("WLDNUM") {
                    Some(NamedAttrValue::IntegerType(n)) => Some(*n),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(nums, vec![1, 2]);
    }

    #[test]
    fn modify_welds_pairs_weldty_and_shop() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        for (r, n) in [(RefU64(31), "WELD"), (RefU64(32), "TUBI"), (RefU64(33), "WELD")] {
            cat.nouns.insert(r, n.into());
        }
        let svc = PipeModelService::new(cat);
        let edit = WeldAttrEdit { weld_type: Some("FIELD".into()), mach_type: Some("MACH".into()) };
        let batch = svc
            .modify_welds(&ctx(), &[RefU64(31), RefU64(32), RefU64(33)], &edit)
            .unwrap();
        // 非 WELD(32) 跳过 + machType 无落库列 warning。
        assert!(batch.warnings.iter().any(|w| w.message.contains("非 WELD")));
        assert!(batch.warnings.iter().any(|w| w.message.contains("无落库列")));
        let writes: Vec<&NamedAttrMap> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { attrs, .. } => Some(attrs),
                _ => None,
            })
            .collect();
        assert_eq!(writes.len(), 2);
        for m in writes {
            assert_eq!(m.get("WELDTY"), Some(&NamedAttrValue::WordType("FIELD".into())));
            assert_eq!(m.get("SHOP"), Some(&NamedAttrValue::BoolType(false)));
        }
    }
}
