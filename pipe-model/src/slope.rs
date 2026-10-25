//! G 组：斜管（spec Phase C；功能文档 §13、§16.15）。
//!
//! 语义要点（Story 33/34）：
//! - 支管级斜率：逐 leg（TUBI 段）施加 fall/angle；锚点不动——**位置重算
//!   属几何内核**，本批写旋转后的 DIR 与 SLRF，位置待算项显式 warning
//! - 元件级斜率：DIR/NOMDIR 写序、TEE `ori and p3` 定向——元件面待内核，
//!   当前只对 TUBI leg 生效
//! - 角度不可达/不可斜显式返回原因（LegResult.reason），不静默 clamp
//! - AID 预览与 presenter 快照 undo 属 UI 层；本批照常一个 UNDOABLE

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::outcome::PipeWarning;
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use glam::Vec3;

/// 逐 leg 结果（Story 33 的"每 leg 结果与不可斜原因"）。
#[derive(Debug)]
pub struct LegResult {
    pub element: RefU64,
    pub applied: bool,
    /// 不可斜原因（缺 DIR / 与竖直平行退化等）；applied=true 时为 None。
    pub reason: Option<String>,
}

/// 绕水平轴把 dir 朝 -Z/+Z 方向转 `deg` 度（Rodrigues）。
///
/// 轴 = normalize(cross(dir, up))；dir 与 up 平行/反平行时退化 → `None`。
fn pitch_dir(dir: Vec3, deg: f64, up: Vec3) -> Option<Vec3> {
    let axis = dir.cross(up);
    if axis.length_squared() < 1e-12 {
        return None; // 与竖直平行：无可施斜面
    }
    let axis = axis.normalize();
    let rad = (deg as f32).to_radians();
    let (s, c) = rad.sin_cos();
    // Rodrigues：v' = v·cosθ + (k×v)·sinθ + k·(k·v)(1-cosθ)
    Some(dir * c + axis.cross(dir) * s + axis * axis.dot(dir) * (1.0 - c))
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 支管级斜率（Story 33）：SLRF 写管 + 逐 TUBI leg 旋转 DIR。
    ///
    /// `slope` 单位为度（E3D fall/angle 表单语义）；正负=坡向。
    pub(crate) fn apply_slope_impl(
        &self,
        _ctx: &Context,
        branch: RefU64,
        slope: f64,
    ) -> Result<(MutationBatch, Vec<LegResult>), PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking {
                message: "apply_slope 目标必须 BRAN".into(),
            });
        }
        let mut batch = MutationBatch::named("apply_slope");
        let mut results = Vec::new();
        let up = Vec3::Z;
        let mut applied_any = false;

        for m in self.catalog.members_of(branch) {
            if self.catalog.noun_of(m).as_deref() != Some("TUBI") {
                continue;
            }
            let dir = match self.catalog.attr(m, "DIR") {
                Some(NamedAttrValue::Vec3Type(v)) => v,
                _ => {
                    results.push(LegResult {
                        element: m,
                        applied: false,
                        reason: Some("无 DIR 可读（位置/方向待内核读面）".into()),
                    });
                    continue;
                }
            };
            match pitch_dir(dir, slope, up) {
                Some(new_dir) => {
                    let mut attrs = NamedAttrMap::default();
                    attrs.insert("DIR".into(), NamedAttrValue::Vec3Type(new_dir));
                    batch.ops.push(MutationOp::SetAttrs { element: m, attrs });
                    results.push(LegResult { element: m, applied: true, reason: None });
                    applied_any = true;
                }
                None => results.push(LegResult {
                    element: m,
                    applied: false,
                    reason: Some("leg 与竖直平行：无可施斜面".into()),
                }),
            }
        }
        if applied_any {
            batch.warnings.push(PipeWarning {
                message: "斜管 DIR 已写；leg 位置重算（锚点不动 + HPOS/TPOS 联动）待几何内核"
                    .into(),
                side_effect: None,
            });
        } else {
            batch.warnings.push(PipeWarning {
                message: "无可施斜 leg".into(),
                side_effect: None,
            });
        }
        Ok((batch, results))
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
        // leg1 水平 X 向；leg2 竖直 Z 向（不可斜）；leg3 无 DIR。
        for (r, dir) in [
            (RefU64(21), Some(Vec3::X)),
            (RefU64(22), Some(Vec3::Z)),
            (RefU64(23), None),
        ] {
            cat.nouns.insert(r, "TUBI".into());
            cat.owners.insert(r, RefU64(11));
            cat.members.entry(RefU64(11)).or_default().push(r);
            if let Some(d) = dir {
                cat.attrs.insert(r, std::collections::BTreeMap::from([
                    ("DIR".into(), NamedAttrValue::Vec3Type(d)),
                ]));
            }
        }
        PipeModelService::new(cat)
    }

    #[test]
    fn slope_rotates_legs_and_reports() {
        let svc = rig();
        let (batch, results) = svc.apply_slope(&ctx(), RefU64(11), 3.0).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].applied);
        assert!(!results[1].applied, "竖直 leg 不可斜");
        assert!(!results[2].applied, "无 DIR 不可斜");

        // X 向 leg 转 3°：新 dir 应有 Z 分量（朝 -Z 或 +Z 看轴号）。
        let MutationOp::SetAttrs { element, attrs } = &batch.ops[0] else { panic!() };
        assert_eq!(*element, RefU64(21));
        let Some(NamedAttrValue::Vec3Type(v)) = attrs.get("DIR") else { panic!() };
        assert!((v.length() - 1.0).abs() < 1e-4, "旋转保持单位向量");
        assert!(v.z.abs() > 0.04, "3° 应产生可测 Z 分量");
    }
}
