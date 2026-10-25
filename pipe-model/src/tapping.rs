//! D 组：开孔/支管座（spec Phase C；功能文档 §10、§18.15 pipestubinconnection）。
//!
//! PTAP 四阶段：选型 → 位置/方向 → 宿主确认 → **两步提交**（开孔件 + 新支管头接入）。
//! 语义要点（Story 24-27）：
//! - 开孔件（OLET/VTWA/STUB 等）挂在宿主支管 TUBI 上，写入
//!   through/orientation/CREF/depth——**几何推导（OLET 与 BEND 两套平面）属内核**，
//!   本批写可推导面 + warning
//! - 新支管头 `HREF=开孔件`、`HCON='TAP'`（createPtapConnection 的接法）
//! - `Adjust…`（specialBranchDirection 的 rotateAroundPlane/nudgeAlong）是几何
//!   微调——不进批，接入内核后再补
//! - 中途取消的半成品态由批的原子性保证（批不提交 = 无半成品残留）

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::naming;
use crate::outcome::PipeWarning;
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use glam::Vec3;

/// 开孔载荷。
#[derive(Debug, Clone)]
pub struct TappingSpec {
    /// 支管座类型（OLET/VTWA/STUB 等 SPRE——也是新建元素的 noun）。
    pub stub_type: String,
    /// 挂接点（宿主支管上的 TUBI/直管件 refno）。
    pub host_tube: RefU64,
    /// 孔位：宿主坐标偏移 mm + 出射方向。
    pub offset_mm: f64,
    pub out_direction: [f64; 3],
    /// 新生支管的规格 refno 与孔径 mm。
    pub new_branch_spec: RefU64,
    pub bore_mm: f64,
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// PTAP 两步提交（Story 24）：开孔件 + 新支管头接 `HREF=开孔件`、`HCON='TAP'`。
    pub(crate) fn create_tapping_impl(
        &self,
        _ctx: &Context,
        host_branch: RefU64,
        tap: &TappingSpec,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(host_branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking {
                message: "create_tapping 宿主必须 BRAN".into(),
            });
        }
        if !self.catalog.writable(host_branch) {
            return Err(PipeError::Blocking { message: "宿主支管无写权限".into() });
        }
        // 挂接点必须是宿主直属成员。
        if !self.catalog.members_of(host_branch).contains(&tap.host_tube) {
            return Err(PipeError::Blocking {
                message: "host_tube 不是宿主支管直属成员".into(),
            });
        }
        let pipe = self.catalog.owner_of(host_branch).ok_or_else(|| PipeError::Blocking {
            message: "宿主支管无属主管".into(),
        })?;
        let pipe_name = self.catalog.name_of(pipe).unwrap_or_default();

        let mut batch = MutationBatch::named("create_tapping");

        // 1) 开孔件：挂宿主支管成员位（through/orientation/CREF/depth 属内核推导）。
        let tap_el = self.catalog.alloc_refno(host_branch);
        let mut attrs = NamedAttrMap::default();
        attrs.insert("APOS".into(), NamedAttrValue::F32Type(tap.offset_mm as f32));
        attrs.insert(
            "DIR".into(),
            NamedAttrValue::Vec3Type(Vec3::new(
                tap.out_direction[0] as f32,
                tap.out_direction[1] as f32,
                tap.out_direction[2] as f32,
            )),
        );
        batch.ops.push(MutationOp::Create {
            refno: tap_el,
            noun: tap.stub_type.clone(),
            under: host_branch,
            name: None,
            attrs,
        });
        batch.warnings.push(PipeWarning {
            message: "开孔件的 through/orientation/CREF/depth 待内核平面推导（OLET/BEND 两套）"
                .into(),
            side_effect: None,
        });

        // 2) 新支管：自动名 + 规格/孔径 + 头接开孔件（HREF=件、HCON='TAP'）。
        let member_count = self.catalog.members_of(pipe).len();
        let name = naming::branch_name(&pipe_name, member_count, "/", "B{i}", |n| {
            self.catalog.name_exists(n)
        });
        let mut bran_attrs = NamedAttrMap::default();
        bran_attrs.insert("PSPE".into(), NamedAttrValue::RefU64Type(tap.new_branch_spec));
        let bore = NamedAttrValue::F32Type(tap.bore_mm as f32);
        bran_attrs.insert("HBOR".into(), bore.clone());
        bran_attrs.insert("TBOR".into(), bore);
        bran_attrs.insert("HREF".into(), NamedAttrValue::RefU64Type(tap_el));
        bran_attrs.insert("HCON".into(), NamedAttrValue::WordType("TAP".into()));
        batch.ops.push(MutationOp::Create {
            refno: self.catalog.alloc_refno(pipe),
            noun: "BRAN".into(),
            under: pipe,
            name,
            attrs: bran_attrs,
        });
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
        cat.owners.insert(RefU64(10), RefU64(9));
        cat.nouns.insert(RefU64(21), "TUBI".into());
        cat.owners.insert(RefU64(21), RefU64(11));
        cat.members.entry(RefU64(11)).or_default().push(RefU64(21));
        PipeModelService::new(cat)
    }

    #[test]
    fn tapping_two_step_commit() {
        let svc = rig();
        let tap = TappingSpec {
            stub_type: "OLET".into(),
            host_tube: RefU64(21),
            offset_mm: 500.0,
            out_direction: [0.0, 0.0, 1.0],
            new_branch_spec: RefU64(88),
            bore_mm: 50.0,
        };
        let batch = svc.create_tapping(&ctx(), RefU64(11), &tap).unwrap();
        assert_eq!(batch.ops.len(), 2);

        let MutationOp::Create { refno: tap_el, noun, under, attrs, .. } = &batch.ops[0]
        else { panic!() };
        assert_eq!(noun, "OLET");
        assert_eq!(*under, RefU64(11));
        assert_eq!(attrs.get("APOS"), Some(&NamedAttrValue::F32Type(500.0)));

        let MutationOp::Create { noun, under, name, attrs, .. } = &batch.ops[1] else { panic!() };
        assert_eq!(noun, "BRAN");
        assert_eq!(*under, RefU64(10), "新支管挂宿主所在管");
        assert_eq!(name.as_deref(), Some("/P1/B2"));
        assert_eq!(attrs.get("HREF"), Some(&NamedAttrValue::RefU64Type(*tap_el)));
        assert_eq!(attrs.get("HCON"), Some(&NamedAttrValue::WordType("TAP".into())));
        assert_eq!(attrs.get("HBOR"), Some(&NamedAttrValue::F32Type(50.0)));
    }

    #[test]
    fn tapping_rejects_foreign_tube() {
        let svc = rig();
        let tap = TappingSpec {
            stub_type: "OLET".into(),
            host_tube: RefU64(999),
            offset_mm: 0.0,
            out_direction: [0.0, 0.0, 1.0],
            new_branch_spec: RefU64(88),
            bore_mm: 50.0,
        };
        assert!(matches!(
            svc.create_tapping(&ctx(), RefU64(11), &tap).unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }
}
