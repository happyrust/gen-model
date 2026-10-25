//! E 组：分合管（spec Phase C；功能文档 §11、§16.13）。
//!
//! 语义要点（splitpipe / mergeTwoBranches 全链）：
//! - SEGMENT：双拾取定段（Esc=splitSegment 触发）——几何求交属内核，本组
//!   拒绝时显式 Blocking，不伪造切点
//! - FEATURE：拾取元件为切点；`insert_assembly` 时先在切点
//!   `PIPINGASSEMBLYMGR.buildInTube` 插装配（Phase C 挂接口，当前 warning）
//! - 分段 = **复制管** + 新支管 `-Split(n)`：旧支管保留 HEAD 半段，新支管拿
//!   TAIL 几何（TPOS/TDIR/TCON）与原 TREF；原支管 PT 接新支管 PH（互写 xREF）
//! - 连接型互换修正法兰 return-ref 属内核——warning 占位
//! - MERGE：Source→Target 两击制；成员迁入 Target（原序保持）；Target 尾端
//!   数据拿 Source 的；Source 空即删、Source 管全空级联删；跨属主移动入
//!   `requires_confirm`（跨库确认语义）

use crate::batch::{Confirm, MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::naming;
use crate::outcome::PipeWarning;
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// 分切位置描述。
#[derive(Debug, Clone)]
pub enum SplitAt {
    /// 段模式：起点/终点两击坐标（世界坐标 mm）。几何求交属内核。
    Segment { from: [f64; 3], to: [f64; 3] },
    /// 特征模式：以既有元件为切点（切点在元件**到侧**——切点元件本身
    /// 迁入新支管作首成员，`splitpipe.pmlfnc` 的 `newBranchMem` 含 findComp）。
    /// `insert_assembly` 时先在切点插装配；`move_to` 指定新支管去向。
    Feature { at: RefU64, insert_assembly: bool, move_to: MoveTo },
    /// 平面模式：三点定面与支管求交。几何求交属内核。
    Plane { points: [[f64; 3]; 3] },
}

/// `!!splitPipe(!component, !moveTo, !number)` 的 moveTo 去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTo {
    /// `moveTo='PIPE'`：新建复制管（`NEW PIPE COPY ATT OF`），新支管名
    /// 为 `<新管名>/B1`。
    NewPipe,
    /// `moveTo='BRANCH'`：同管新增支管，名 `<管名>/B<mCount(BRAN)+1>`。
    SamePipe,
}

/// 分段时按序迁移的尾端属性（新支管继承旧支管的 TAIL 侧）。
const TAIL_ATTRS: &[&str] = &["TPOS", "TDIR", "TCON", "TREF", "TBOR"];
/// 复制管要抄的 PIPE 属性（`NEW PIPE COPY ATT OF` 语义的最小过程面）。
const PIPE_COPY_ATTRS: &[&str] = &["PSPE", "BORE", "ISPEC", "TSPEC", "TEMP", "PRES", "SLRF"];

fn set_attr(element: RefU64, name: &str, value: NamedAttrValue) -> MutationOp {
    let mut attrs = NamedAttrMap::default();
    attrs.insert(name.into(), value);
    MutationOp::SetAttrs { element, attrs }
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 分段（Story 28/30）。Feature 模式全链；Segment/Plane 待几何内核。
    pub(crate) fn split_branch_impl(
        &self,
        _ctx: &Context,
        branch: RefU64,
        at: SplitAt,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking {
                message: "split_branch 目标必须 BRAN".into(),
            });
        }
        let SplitAt::Feature { at: cut, insert_assembly, move_to } = at else {
            return Err(PipeError::Blocking {
                message: "SEGMENT/PLANE 分段需几何求交内核，当前不支持（不伪造切点）".into(),
            });
        };

        // Story 30：切点合法性——必须直属成员。PML `splitpipe` 函数级仅拒
        // 首件（`sequence eq 1 AND atle eq 0`：旧支管需剩至少一件）；末件
        // 分段函数级放行（源码标 REVISIT 的已知缺口，UI 层另有拦截）。
        let members = self.catalog.members_of(branch);
        let Some(cut_idx) = members.iter().position(|m| *m == cut) else {
            return Err(PipeError::Blocking {
                message: "切点元件不是该支管直属成员".into(),
            });
        };
        if cut_idx == 0 {
            return Err(PipeError::Blocking {
                message: "首件不是合法分段点（切点在元件到侧，首件切点使旧支管为空）".into(),
            });
        }

        let pipe = self.catalog.owner_of(branch).ok_or_else(|| PipeError::Blocking {
            message: "BRAN 无属主管".into(),
        })?;
        let pipe_name = self.catalog.name_of(pipe).unwrap_or_default();

        let mut batch = MutationBatch::named("split_branch");

        // 1) moveTo=PIPE：复制管 NEW PIPE COPY ATT OF（过程面属性抄齐）；
        //    moveTo=BRANCH：不建管，新支管直接挂原管。
        let (new_bran_under, new_bran_name) = match move_to {
            MoveTo::NewPipe => {
                // 复制管挂原管同一属主——与原管同库，取原管定库号。
                let new_pipe = self.catalog.alloc_refno(pipe);
                let mut pipe_attrs = NamedAttrMap::default();
                for k in PIPE_COPY_ATTRS {
                    if let Some(v) = self.catalog.attr(pipe, k) {
                        pipe_attrs.insert((*k).into(), v);
                    }
                }
                let new_pipe_name =
                    naming::split_name(&pipe_name, 1, |n| self.catalog.name_exists(n))
                        .ok_or_else(|| PipeError::Blocking {
                            message: "派生管名耗尽".into(),
                        })?;
                let pipe_owner = self.catalog.owner_of(pipe).ok_or_else(|| {
                    PipeError::Blocking { message: "管无属主".into() }
                })?;
                batch.ops.push(MutationOp::Create {
                    refno: new_pipe,
                    noun: "PIPE".into(),
                    under: pipe_owner,
                    name: Some(new_pipe_name.clone()),
                    attrs: pipe_attrs,
                });
                (new_pipe, format!("{new_pipe_name}/B1"))
            }
            MoveTo::SamePipe => {
                let bran_count = self
                    .catalog
                    .members_of(pipe)
                    .iter()
                    .filter(|m| self.catalog.noun_of(**m).as_deref() == Some("BRAN"))
                    .count();
                (pipe, format!("{pipe_name}/B{}", bran_count + 1))
            }
        };
        // PML `handle ANY → alert('User should rename…')`：确定性名撞名时显式告警。
        if self.catalog.name_exists(&new_bran_name) {
            batch.warnings.push(PipeWarning {
                message: format!("新支管名 {new_bran_name} 已存在，需人工改名"),
                side_effect: None,
            });
        }

        // 2) 新支管（COPY ATT 语义的最小面）：PSPE/HBOR/TBOR + 旧支管 TAIL 侧数据。
        //    两种 moveTo 下新支管都与原支管同库——取原管定库号。
        let new_bran = self.catalog.alloc_refno(pipe);
        let mut bran_attrs = NamedAttrMap::default();
        for k in ["PSPE", "HBOR", "TBOR"] {
            if let Some(v) = self.catalog.attr(branch, k) {
                bran_attrs.insert(k.into(), v);
            }
        }
        for k in TAIL_ATTRS {
            if let Some(v) = self.catalog.attr(branch, *k) {
                bran_attrs.insert((*k).into(), v);
            }
        }
        batch.ops.push(MutationOp::Create {
            refno: new_bran,
            noun: "BRAN".into(),
            under: new_bran_under,
            name: Some(new_bran_name),
            attrs: bran_attrs,
        });

        if insert_assembly {
            batch.warnings.push(PipeWarning {
                message: "insert_assembly：切点 PIPINGASSEMBLYMGR.buildInTube 待装配接口".into(),
                side_effect: None,
            });
        }

        // 3) 下游成员**含切点元件**迁入新支管（PML `newBranchMem` 自 findComp
        //    起收；INCLUDE 保原相对序——切点成新支管首成员）。
        for (order, member) in members[cut_idx..].iter().enumerate() {
            batch.ops.push(MutationOp::Include {
                element: *member,
                into: new_bran,
                order: Some(order),
            });
        }

        // 4) 互连（splitpipe 原生序）：新支管 `CONN PH TO FIRST MEM`
        //    → HREF=切点元件；原支管 `CONN PT TO PH OF newBranch`
        //    → TREF=新支管。随后 `tconn $!hconn`/`hconn $!tconn` 交叉互换
        //    两侧连接型——xCON 值由 CONN 内核推导，批内显式 warning 不伪造。
        batch.ops.push(set_attr(branch, "TREF", NamedAttrValue::RefU64Type(new_bran)));
        batch.ops.push(set_attr(new_bran, "HREF", NamedAttrValue::RefU64Type(cut)));
        batch.warnings.push(PipeWarning {
            message: "分段互连的 HCON/TCON 交叉互换与法兰 return-ref 修正待连接内核（Story 28 尾段）"
                .into(),
            side_effect: Some("已写 TREF→新支管、HREF→切点元件；切点 HPOS/TPOS 待几何内核".into()),
        });
        Ok(batch)
    }

    /// 合并（Story 29）：Source 成员迁入 Target → Target 尾端拿 Source 尾数据 →
    /// Source 空即删 → Source 管全空级联删。跨属主移动入 requires_confirm。
    pub(crate) fn merge_branches_impl(
        &self,
        _ctx: &Context,
        source: RefU64,
        target: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        for (r, tag) in [(source, "source"), (target, "target")] {
            if self.catalog.noun_of(r).as_deref() != Some("BRAN") {
                return Err(PipeError::Blocking {
                    message: format!("merge_branches {tag} 必须 BRAN"),
                });
            }
        }
        let src_pipe = self.catalog.owner_of(source).ok_or_else(|| PipeError::Blocking {
            message: "source 无属主管".into(),
        })?;
        let tgt_pipe = self.catalog.owner_of(target).ok_or_else(|| PipeError::Blocking {
            message: "target 无属主管".into(),
        })?;

        let mut batch = MutationBatch::named("merge_branches");
        if src_pipe != tgt_pipe {
            batch.requires_confirm.push(Confirm {
                prompt: "跨管合并：成员将跨属主移动（跨库语义需确认）".into(),
                on_yes: Vec::new(),
            });
        }

        // 1) Source 成员原序迁入 Target 尾部。
        let base = self.catalog.members_of(target).len();
        for (i, m) in self.catalog.members_of(source).iter().enumerate() {
            batch.ops.push(MutationOp::Include {
                element: *m,
                into: target,
                order: Some(base + i),
            });
        }

        // 2) Target 尾端数据拿 Source 的（恢复头尾连接数据）。
        for k in TAIL_ATTRS {
            if let Some(v) = self.catalog.attr(source, *k) {
                batch.ops.push(set_attr(target, k, v));
            }
        }

        // 3) Source 空即删；4) Source 管余下无成员即级联删。
        batch.ops.push(MutationOp::Delete { element: source });
        let src_siblings = self
            .catalog
            .members_of(src_pipe)
            .iter()
            .filter(|m| **m != source)
            .count();
        if src_siblings == 0 {
            batch.ops.push(MutationOp::Delete { element: src_pipe });
            batch.warnings.push(PipeWarning {
                message: "source 管已全空，级联删除 PIPE".into(),
                side_effect: None,
            });
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::BranchEnd;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog};

    /// 支管挂三个元件，切点选中段。
    fn rig() -> (PipeModelService<MockCatalog>, RefU64) {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        cat.owners.insert(RefU64(10), RefU64(9));
        for (r, n) in [(RefU64(21), "TUBI"), (RefU64(22), "ELBO"), (RefU64(23), "TUBI")] {
            cat.nouns.insert(r, n.into());
            cat.owners.insert(r, RefU64(11));
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        // 旧支管尾端数据 + 管侧过程属性。
        cat.attrs.get_mut(&RefU64(11)).unwrap().extend([
            ("TPOS".into(), NamedAttrValue::F32Type(900.0)),
            ("TCON".into(), NamedAttrValue::WordType("TEE".into())),
            ("HBOR".into(), NamedAttrValue::F32Type(100.0)),
            ("TBOR".into(), NamedAttrValue::F32Type(100.0)),
        ]);
        cat.attrs.get_mut(&RefU64(10)).unwrap()
            .insert("ISPEC".into(), NamedAttrValue::ElementType("/INS".into()));
        (PipeModelService::new(cat), RefU64(11))
    }

    #[test]
    fn split_rejects_nonmember_and_first() {
        let (svc, bran) = rig();
        // 非直属成员
        assert!(matches!(
            svc.split_branch(&ctx(), bran, SplitAt::Feature { at: RefU64(99), insert_assembly: false, move_to: MoveTo::NewPipe })
                .unwrap_err(),
            PipeError::Blocking { .. }
        ));
        // 首件（切点在元件到侧，首件切点使旧支管空——PML sequence eq 1 AND atle eq 0）
        assert!(matches!(
            svc.split_branch(&ctx(), bran, SplitAt::Feature { at: RefU64(21), insert_assembly: false, move_to: MoveTo::NewPipe })
                .unwrap_err(),
            PipeError::Blocking { .. }
        ));
        // 末件：PML 函数级放行（REVISIT 缺口）——产出单成员新支管
        let last = svc
            .split_branch(&ctx(), bran, SplitAt::Feature { at: RefU64(23), insert_assembly: false, move_to: MoveTo::NewPipe })
            .unwrap();
        let migrated: Vec<RefU64> = last
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::Include { element, .. } => Some(*element),
                _ => None,
            })
            .collect();
        assert_eq!(migrated, vec![RefU64(23)]);
        // SEGMENT/PLANE 无内核 → Blocking 不伪造
        assert!(matches!(
            svc.split_branch(&ctx(), bran, SplitAt::Segment { from: [0.; 3], to: [1.; 3] })
                .unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }

    #[test]
    fn split_feature_full_batch() {
        let (svc, bran) = rig();
        let batch = svc
            .split_branch(&ctx(), bran, SplitAt::Feature { at: RefU64(22), insert_assembly: false, move_to: MoveTo::NewPipe })
            .unwrap();

        // op 序：新 PIPE Create → 新 BRAN Create → 成员 Include → 互写 REF。
        let creates: Vec<&MutationOp> = batch
            .ops
            .iter()
            .filter(|o| matches!(o, MutationOp::Create { .. }))
            .collect();
        assert_eq!(creates.len(), 2);
        let MutationOp::Create { refno: new_pipe, noun, name, attrs, .. } = creates[0] else { panic!() };
        assert_eq!(noun, "PIPE");
        assert_eq!(name.as_deref(), Some("/P1-Split(1)"));
        assert_eq!(attrs.get("ISPEC"), Some(&NamedAttrValue::ElementType("/INS".into())));
        let MutationOp::Create { refno: new_bran, noun, under, name, attrs, .. } = creates[1]
        else { panic!() };
        assert_eq!(noun, "BRAN");
        assert_eq!(*under, *new_pipe, "新支管挂在复制管下");
        assert_eq!(name.as_deref(), Some("/P1-Split(1)/B1"));
        // 新支管拿 TAIL 侧。
        assert_eq!(attrs.get("TPOS"), Some(&NamedAttrValue::F32Type(900.0)));
        assert_eq!(attrs.get("TCON"), Some(&NamedAttrValue::WordType("TEE".into())));

        // 下游成员**含切点元件**（ELBO 22 起）迁入新支管，原序位。
        let includes: Vec<&MutationOp> = batch
            .ops
            .iter()
            .filter(|o| matches!(o, MutationOp::Include { .. }))
            .collect();
        assert_eq!(includes.len(), 2);
        let MutationOp::Include { element, into, order } = includes[0] else { panic!() };
        assert_eq!(*element, RefU64(22), "切点元件成新支管首成员");
        assert_eq!(*into, *new_bran);
        assert_eq!(*order, Some(0));
        let MutationOp::Include { element, order, .. } = includes[1] else { panic!() };
        assert_eq!(*element, RefU64(23));
        assert_eq!(*order, Some(1));

        // 互写：原支管 TREF→新支管（CONN PT TO PH OF），新支管 HREF→切点元件
        // （CONN PH TO FIRST MEM）。
        let set_on = |el: RefU64, attr: &str| {
            batch.ops.iter().any(|op| matches!(op,
                MutationOp::SetAttrs { element, attrs }
                    if *element == el && attrs.contains_key(attr)))
        };
        assert!(set_on(bran, "TREF"));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::SetAttrs { element, attrs }
                if *element == *new_bran
                    && attrs.get("HREF") == Some(&NamedAttrValue::RefU64Type(RefU64(22))))));
        assert!(!batch.warnings.is_empty());
    }

    #[test]
    fn split_same_pipe_mode() {
        let (svc, bran) = rig();
        let batch = svc
            .split_branch(&ctx(), bran, SplitAt::Feature { at: RefU64(22), insert_assembly: false, move_to: MoveTo::SamePipe })
            .unwrap();
        // moveTo=BRANCH：不建 PIPE，新支管直挂原管，名 = 管名/B<mCount+1>。
        assert!(!batch.ops.iter().any(|op| matches!(op,
            MutationOp::Create { noun, .. } if noun == "PIPE")));
        let MutationOp::Create { noun, under, name, .. } = batch
            .ops
            .iter()
            .find(|op| matches!(op, MutationOp::Create { .. }))
            .unwrap() else { panic!() };
        assert_eq!(noun, "BRAN");
        assert_eq!(*under, RefU64(10));
        assert_eq!(name.as_deref(), Some("/P1/B2"));
        // 切点+下游迁入照旧。
        let migrated: Vec<RefU64> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::Include { element, .. } => Some(*element),
                _ => None,
            })
            .collect();
        assert_eq!(migrated, vec![RefU64(22), RefU64(23)]);
    }

    #[test]
    fn merge_moves_members_and_cascade_deletes() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77))
            .branch(RefU64(12), "/P1/B2", RefU64(10), RefU64(77));
        // 同管两条支管：source=B2（单成员），target=B1（两成员）。
        cat.nouns.insert(RefU64(21), "TUBI".into());
        cat.members.entry(RefU64(11)).or_default().extend([RefU64(21)]);
        cat.nouns.insert(RefU64(22), "ELBO".into());
        cat.members.entry(RefU64(12)).or_default().extend([RefU64(22)]);
        cat.attrs.get_mut(&RefU64(12)).unwrap()
            .insert("TPOS".into(), NamedAttrValue::F32Type(500.0));
        // 另挂一条支管防级联删先单独测——本用例 B2 删后 PIPE 仍有 B1，不级联。
        let svc = PipeModelService::new(cat);
        let batch = svc.merge_branches(&ctx(), RefU64(12), RefU64(11)).unwrap();
        // 同管合并不弹确认。
        assert!(batch.requires_confirm.is_empty());
        // B2 成员迁入 B1 尾部序位 1。
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Include { element: e, into, order }
                if *e == RefU64(22) && *into == RefU64(11) && *order == Some(1))));
        // Target 拿 Source 尾数据。
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::SetAttrs { element, attrs }
                if *element == RefU64(11) && attrs.get("TPOS") == Some(&NamedAttrValue::F32Type(500.0)))));
        // Source 删；PIPE 尚有 B1 不级联。
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Delete { element } if *element == RefU64(12))));
        assert!(!batch.ops.iter().any(|op| matches!(op,
            MutationOp::Delete { element } if *element == RefU64(10))));
    }

    #[test]
    fn merge_cascade_deletes_empty_pipe() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77))
            .pipe(RefU64(20), "/P2", RefU64(77), 100.0)
            .branch(RefU64(21), "/P2/B1", RefU64(20), RefU64(77));
        let svc = PipeModelService::new(cat);
        // source B2 所在 P2 只剩这一条支管 → 级联删 P2；且跨管移动弹确认。
        let batch = svc.merge_branches(&ctx(), RefU64(21), RefU64(11)).unwrap();
        assert_eq!(batch.requires_confirm.len(), 1);
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Delete { element } if *element == RefU64(21))));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Delete { element } if *element == RefU64(20))));
        // 尾数据字段缺失时 TAIL_ATTRS 无值可抄——不报错。
        let _ = BranchEnd::Head;
    }
}
