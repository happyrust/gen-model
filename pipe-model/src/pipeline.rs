//! A/B 组：PIPE/BRAN 生命周期与支管端连接（spec Phase A）。
//!
//! 行为锚点：§4.2 建管序列、§4.3 改名级联、§4.4 过程字段级联、
//! §6 端编辑四序写、§7 CONNECT 四终态与 FCONN 兜底。

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::{BranchEnd, Context};
use crate::error::PipeError;
use crate::naming;
use crate::outcome::{ConnectOutcome, PipeWarning};
use crate::service::{PipeModel, PipeModelService};
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use glam::Vec3;

/// `NEW PIPE` 入参（pipecreation 表单反写 §4.2）。
#[derive(Debug, Clone)]
pub struct PipeSpec {
    /// 属主（ZONE 或 EQUI；非法属主在建批前拒绝）。
    pub owner: RefU64,
    pub name: Option<String>,
    /// PSPE 规格 refno。
    pub spec: RefU64,
    /// HBORE/TBORE（mm）；同径管二者相同。
    pub bore_mm: f64,
    /// 保温/伴热/温度/压力/坡度参考（可空）。
    pub insulation: Option<String>,
    pub tracing: Option<String>,
    pub temperature: Option<f64>,
    pub pressure: Option<f64>,
    pub slope_ref: Option<RefU64>,
}

/// 过程字段补丁（PIPINGPROCESSDATA 级联，§4.4）。
///
/// 外层 `Option` = 该字段是否参与本次更新；内层 `Option<String>` 的 `None`
/// = 清空（写 unset）。
#[derive(Debug, Clone, Default)]
pub struct ProcessPatch {
    pub insulation: Option<Option<String>>,
    pub tracing: Option<Option<String>>,
    pub temperature: Option<f64>,
    pub pressure: Option<f64>,
}

/// 端编辑载荷：四序写按 `xBOR → POSITION → DIRECTION → CONNECTION` 排进批（§6）。
#[derive(Debug, Clone)]
pub struct EndEdit {
    pub bore_mm: Option<f64>,
    /// 世界坐标位置（mm）。
    pub position: Option<[f64; 3]>,
    /// 方向（单位向量）。
    pub direction: Option<[f64; 3]>,
    /// 连接目标 refno；`None` 且 `free=true` 时把端置成游离（CON 写 'OPEN'）。
    pub connection: Option<RefU64>,
    pub free: bool,
}

/// ROUTER 途经点（§18.11 `NEW POINT AT <pos> AFTER <elem>`）。
#[derive(Debug, Clone)]
pub struct RoutePoint {
    pub position: [f64; 3],
    /// 途经锚定的既有元件；首点常为支管端。
    pub after: Option<RefU64>,
}

/// 端属性名前缀（H/T）→ 全名。
fn end_attr(end: BranchEnd, base: &str) -> String {
    let p = match end {
        BranchEnd::Head => "H",
        BranchEnd::Tail => "T",
    };
    format!("{p}{base}")
}

/// 单属性 SetAttrs 便捷构造——端编辑写序必须按序出多个 op
/// （`NamedAttrMap` 是 BTreeMap，塞一起会丢 E3D 要求的属性序）。
fn set_attr(element: RefU64, name: &str, value: NamedAttrValue) -> MutationOp {
    let mut attrs = NamedAttrMap::default();
    attrs.insert(name.into(), value);
    MutationOp::SetAttrs { element, attrs }
}

/// PIPE 自动名：`/PIPE<i>` 探测式（待接 namOpt 规则文件，§18.9）。
fn auto_pipe_name(cat: &impl PipeCatalog) -> Option<String> {
    (1..=9999)
        .map(|i| format!("/PIPE{i}"))
        .find(|n| !cat.name_exists(n))
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 建管属主校验（Story 3：先拒绝，不留半成品）。
    fn check_pipe_owner(&self, owner: RefU64) -> Result<(), PipeError> {
        let noun = self.catalog.noun_of(owner).ok_or_else(|| PipeError::Blocking {
            message: format!("PIPE 属主 {:?} 不在数据面", owner),
        })?;
        if !matches!(noun.as_str(), "ZONE" | "EQUI") {
            return Err(PipeError::Blocking {
                message: format!("PIPE 属主必须 ZONE/EQUI，实得 {noun}"),
            });
        }
        if !self.catalog.writable(owner) {
            return Err(PipeError::Blocking {
                message: "属主无写权限，未建任何元素".into(),
            });
        }
        Ok(())
    }

    /// 递归收集 name 以 `old_prefix/` 为前缀的全部后代（改名级联 §4.3）。
    fn collect_prefixed_descendants(&self, root: RefU64, old_prefix: &str) -> Vec<(RefU64, String)> {
        let mut out = Vec::new();
        let mut stack = self.catalog.members_of(root);
        let prefix = format!("{old_prefix}/");
        while let Some(el) = stack.pop() {
            stack.extend(self.catalog.members_of(el));
            if let Some(name) = self.catalog.name_of(el) {
                if name.starts_with(&prefix) {
                    out.push((el, name));
                }
            }
        }
        out
    }

    /// 支管建批（create_pipe 与 create_branch 共用）。
    fn branch_create_ops(
        &self,
        pipe: RefU64,
        pipe_name: &str,
        spec: RefU64,
        bore_mm: f64,
        batch: &mut MutationBatch,
    ) {
        let member_count = self.catalog.members_of(pipe).len();
        let name = naming::branch_name(pipe_name, member_count, "/", "B{i}", |n| {
            self.catalog.name_exists(n)
        })
        .or_else(|| {
            batch.warnings.push(PipeWarning {
                message: "支管自动名耗尽（B1..=成员数+1 全占），支管未命名".into(),
                side_effect: None,
            });
            None
        });
        let mut attrs = NamedAttrMap::default();
        attrs.insert("PSPE".into(), NamedAttrValue::RefU64Type(spec));
        // HBOR=TBOR 继承管孔径；BRAN 不写 BORE（Story 7）。
        let bore = NamedAttrValue::F32Type(bore_mm as f32);
        attrs.insert("HBOR".into(), bore.clone());
        attrs.insert("TBOR".into(), bore);
        // TODO(HSTU)：E3D 建支管后 `SELECT HSTU` 选头管——走 spec_resolver，
        // Phase C 接 SpecCatalog 后在此补写 HSTU 行。
        batch.ops.push(MutationOp::Create {
            refno: self.catalog.alloc_refno(pipe),
            noun: "BRAN".into(),
            under: pipe,
            name,
            attrs,
        });
    }
}

impl<C: PipeCatalog> PipeModel for PipeModelService<C> {
    /// `NEW PIPE` + 首支管自动建（§4.2）。先校验再排批——校验失败不产生任何 op。
    fn create_pipe(&self, _ctx: &Context, spec: &PipeSpec) -> Result<MutationBatch, PipeError> {
        self.check_pipe_owner(spec.owner)?;

        let mut batch = MutationBatch::named("create_pipe");
        let pipe = self.catalog.alloc_refno(spec.owner);
        let pipe_name = match &spec.name {
            Some(n) => n.clone(),
            None => auto_pipe_name(&self.catalog).ok_or_else(|| PipeError::Blocking {
                message: "PIPE 自动命名耗尽".into(),
            })?,
        };

        let mut attrs = NamedAttrMap::default();
        attrs.insert("PSPE".into(), NamedAttrValue::RefU64Type(spec.spec));
        attrs.insert("BORE".into(), NamedAttrValue::F32Type(spec.bore_mm as f32));
        if let Some(v) = &spec.insulation {
            attrs.insert("ISPEC".into(), NamedAttrValue::StringType(v.clone()));
        }
        if let Some(v) = &spec.tracing {
            attrs.insert("TSPEC".into(), NamedAttrValue::StringType(v.clone()));
        }
        if let Some(v) = spec.temperature {
            attrs.insert("TEMP".into(), NamedAttrValue::F32Type(v as f32));
        }
        if let Some(v) = spec.pressure {
            attrs.insert("PRES".into(), NamedAttrValue::F32Type(v as f32));
        }
        if let Some(v) = spec.slope_ref {
            attrs.insert("SLRF".into(), NamedAttrValue::RefU64Type(v));
        }
        batch.ops.push(MutationOp::Create {
            refno: pipe,
            noun: "PIPE".into(),
            under: spec.owner,
            name: Some(pipe_name.clone()),
            attrs,
        });

        // 首个 BRAN（Story 2）：HBOR=TBOR 继承、自动名、PSPE 继承。
        self.branch_create_ops(pipe, &pipe_name, spec.spec, spec.bore_mm, &mut batch);
        Ok(batch)
    }

    /// 既有 PIPE 追加支管（Story 2 同一组语义，单用接口）。
    fn create_branch(&self, _ctx: &Context, pipe: RefU64) -> Result<MutationBatch, PipeError> {
        let noun = self.catalog.noun_of(pipe).ok_or_else(|| PipeError::Blocking {
            message: format!("{:?} 不在数据面", pipe),
        })?;
        if noun != "PIPE" {
            return Err(PipeError::Blocking {
                message: format!("create_branch 目标是 {noun}，必须 PIPE"),
            });
        }
        if !self.catalog.writable(pipe) {
            return Err(PipeError::Blocking { message: "PIPE 无写权限".into() });
        }
        let pipe_name = self.catalog.name_of(pipe).unwrap_or_default();
        let (spec, bore) = match (
            self.catalog.attr(pipe, "PSPE"),
            self.catalog.attr(pipe, "BORE"),
        ) {
            (Some(NamedAttrValue::RefU64Type(s)), Some(NamedAttrValue::F32Type(b))) => (s, b as f64),
            _ => {
                return Err(PipeError::Blocking {
                    message: "PIPE 缺 PSPE/BORE，无法派生支管".into(),
                })
            }
        };

        let mut batch = MutationBatch::named("create_branch");
        self.branch_create_ops(pipe, &pipe_name, spec, bore, &mut batch);
        Ok(batch)
    }

    /// 改名级联（§4.3）：元素自身 + 全子树前缀匹配者（`/OLD/...` → `/NEW/...`）。
    fn rename_cascade(
        &self,
        _ctx: &Context,
        element: RefU64,
        name: &str,
    ) -> Result<MutationBatch, PipeError> {
        let old_name = self.catalog.name_of(element).ok_or_else(|| PipeError::Blocking {
            message: format!("{:?} 无 NAME，不可改名级联", element),
        })?;
        let mut batch = MutationBatch::named("rename_cascade");
        batch.ops.push(MutationOp::Rename {
            element,
            name: name.to_string(),
            cascade: true,
        });
        for (el, child_name) in self.collect_prefixed_descendants(element, &old_name) {
            let rest = child_name[old_name.len()..].to_string(); // 含前导 '/'
            batch.ops.push(MutationOp::Rename {
                element: el,
                name: format!("{name}{rest}"),
                cascade: false,
            });
        }
        Ok(batch)
    }

    /// 过程字段级联（§4.4 / PIPINGPROCESSDATA.updateElement）：写 PIPE 自身 +
    /// 级联所有"现值等于 PIPE 旧值"的子元素——已被单独改过的子值不被覆盖。
    fn update_process_data(
        &self,
        _ctx: &Context,
        pipe: RefU64,
        patch: &ProcessPatch,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(pipe).as_deref() != Some("PIPE") {
            return Err(PipeError::Blocking { message: "update_process_data 目标必须 PIPE".into() });
        }
        let mut batch = MutationBatch::named("update_process_data");
        // (patch 值, 属性名)；字符串字段统一走 ElementType（ISPEC/TSPEC 是规格名）。
        let fields: [(&str, Option<NamedAttrValue>); 4] = [
            ("ISPEC", patch.insulation.clone().map(|v| {
                v.map(NamedAttrValue::ElementType).unwrap_or(NamedAttrValue::InvalidType)
            })),
            ("TSPEC", patch.tracing.clone().map(|v| {
                v.map(NamedAttrValue::ElementType).unwrap_or(NamedAttrValue::InvalidType)
            })),
            ("TEMP", patch.temperature.map(|v| NamedAttrValue::F32Type(v as f32))),
            ("PRES", patch.pressure.map(|v| NamedAttrValue::F32Type(v as f32))),
        ];
        // 全子树（含各支管与元件）一次收齐，逐字段过滤"现值==旧值"的级联目标。
        let mut descendants = Vec::new();
        let mut stack = self.catalog.members_of(pipe);
        while let Some(el) = stack.pop() {
            stack.extend(self.catalog.members_of(el));
            descendants.push(el);
        }
        for (attr_name, new_val) in fields {
            let Some(new_val) = new_val else { continue };
            let old_val = self.catalog.attr(pipe, attr_name);
            batch.ops.push(set_attr(pipe, attr_name, new_val.clone()));
            let mut cascaded = 0usize;
            for el in &descendants {
                if self.catalog.attr(*el, attr_name) == old_val && old_val.is_some() {
                    batch.ops.push(set_attr(*el, attr_name, new_val.clone()));
                    cascaded += 1;
                }
            }
            if cascaded == 0 && old_val.is_some() {
                // §决策 10：级联收集为空转 warning，不吞。
                batch.warnings.push(PipeWarning {
                    message: format!("{attr_name} 级联收集为空：无子元素现值等于 PIPE 旧值"),
                    side_effect: None,
                });
            }
        }
        Ok(batch)
    }

    /// 端编辑四序写（§6）：`xBOR → xPOSITION → xDIRECTION → xCONNECTION → L*=TRUE`。
    /// 每属性一个 op 保序（NamedAttrMap 是 BTreeMap，合并会丢序）。
    fn edit_branch_end(
        &self,
        _ctx: &Context,
        branch: RefU64,
        end: BranchEnd,
        edit: &EndEdit,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking { message: "edit_branch_end 目标必须 BRAN".into() });
        }
        let mut batch = MutationBatch::named("edit_branch_end");
        if let Some(b) = edit.bore_mm {
            batch.ops.push(set_attr(branch, &end_attr(end, "BOR"), NamedAttrValue::F32Type(b as f32)));
        }
        if let Some(p) = edit.position {
            batch.ops.push(set_attr(
                branch,
                &end_attr(end, "POS"),
                NamedAttrValue::Vec3Type(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)),
            ));
        }
        if let Some(d) = edit.direction {
            batch.ops.push(set_attr(
                branch,
                &end_attr(end, "DIR"),
                NamedAttrValue::Vec3Type(Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32)),
            ));
        }
        if let Some(target) = edit.connection {
            // E3D `CONN PH/PT TO <elem>` 写 xREF；xCON 连接词由连接内核推导——
            // 见 connect_end 的 outcome 语义，这里只记目标。
            batch.ops.push(set_attr(branch, &end_attr(end, "REF"), NamedAttrValue::RefU64Type(target)));
        } else if edit.free {
            batch.ops.push(set_attr(
                branch,
                &end_attr(end, "CON"),
                NamedAttrValue::WordType("OPEN".into()),
            ));
        }
        // LHEAD/LTAIL = "端已定义"标志，恒最后写（§6 写序末尾）。
        let lock = match end {
            BranchEnd::Head => "LHEAD",
            BranchEnd::Tail => "LTAIL",
        };
        batch.ops.push(set_attr(branch, lock, NamedAttrValue::BoolType(true)));
        Ok(batch)
    }

    /// `CONN PH/PT TO <elem>`（§7）。
    ///
    /// Phase A 实现边界：写 `{x}REF = target` + 回读对端 noun 作 con_type。
    /// xCON 连接词与 GASK/FLAN 自动补件（Story 12/13）依赖连接内核与目录
    /// 连接表——补件链路 Phase C 接；目标不存在走 Unassigned 占位（minTube）。
    fn connect_end(
        &self,
        _ctx: &Context,
        branch: RefU64,
        end: BranchEnd,
        target: RefU64,
    ) -> Result<(MutationBatch, ConnectOutcome), PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking { message: "connect_end 目标必须 BRAN".into() });
        }
        let mut batch = MutationBatch::named("connect_end");
        let Some(peer_noun) = self.catalog.noun_of(target) else {
            // 兜底路径二（§7/Story 11）：FCONN 强接 + minTube 占位，端留游离态。
            batch.ops.push(set_attr(
                branch,
                &end_attr(end, "CON"),
                NamedAttrValue::WordType("FCONN".into()),
            ));
            let min_tube = self.settings.min_tube_mm;
            batch.warnings.push(PipeWarning {
                message: "连接目标不在数据面：FCONN 强接 + minTube 占位，端留游离态".into(),
                side_effect: Some(format!("占位长度 {min_tube}mm")),
            });
            return Ok((batch, ConnectOutcome::Unassigned { placeholder_length_mm: min_tube }));
        };
        batch.ops.push(set_attr(
            branch,
            &end_attr(end, "REF"),
            NamedAttrValue::RefU64Type(target),
        ));
        batch.warnings.push(PipeWarning {
            message: "xCON 连接词/GASK+FLAN 自动补件待连接内核接入（Phase C），本批只写 xREF".into(),
            side_effect: None,
        });
        Ok((
            batch,
            ConnectOutcome::Connected { con_type: peer_noun, peer: target },
        ))
    }

    /// 断开端（Story 14）：xREF 置 nulref，端几何保留可再编辑。
    fn disconnect_end(
        &self,
        _ctx: &Context,
        branch: RefU64,
        end: BranchEnd,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking { message: "disconnect_end 目标必须 BRAN".into() });
        }
        let mut batch = MutationBatch::named("disconnect_end");
        batch.ops.push(set_attr(
            branch,
            &end_attr(end, "REF"),
            NamedAttrValue::RefU64Type(RefU64(0)),
        ));
        Ok(batch)
    }

    fn create_component(
        &self,
        ctx: &Context,
        branch: RefU64,
        sel: &crate::component::ComponentSelect,
    ) -> Result<(MutationBatch, crate::outcome::ComponentOutcome), PipeError> {
        self.create_component_impl(ctx, branch, sel)
    }

    fn reselect_component(
        &self,
        ctx: &Context,
        component: RefU64,
        qa: &crate::choice::QuestionAnswers,
    ) -> Result<(MutationBatch, crate::outcome::ComponentOutcome), PipeError> {
        self.reselect_component_impl(ctx, component, qa)
    }

    fn flip_component(
        &self,
        ctx: &Context,
        component: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        self.flip_component_impl(ctx, component)
    }

    fn create_tapping(
        &self,
        ctx: &Context,
        host_branch: RefU64,
        tap: &crate::tapping::TappingSpec,
    ) -> Result<MutationBatch, PipeError> {
        self.create_tapping_impl(ctx, host_branch, tap)
    }

    fn split_branch(
        &self,
        ctx: &Context,
        branch: RefU64,
        at: crate::splitmerge::SplitAt,
    ) -> Result<MutationBatch, PipeError> {
        self.split_branch_impl(ctx, branch, at)
    }

    fn merge_branches(
        &self,
        ctx: &Context,
        source: RefU64,
        target: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        self.merge_branches_impl(ctx, source, target)
    }

    fn change_spec(
        &self,
        ctx: &Context,
        element: RefU64,
        new_spec: RefU64,
    ) -> Result<(MutationBatch, Vec<crate::specchange::SpecChangeResult>), PipeError> {
        self.change_spec_impl(ctx, element, new_spec)
    }

    fn apply_slope(
        &self,
        ctx: &Context,
        branch: RefU64,
        slope: f64,
    ) -> Result<(MutationBatch, Vec<crate::slope::LegResult>), PipeError> {
        self.apply_slope_impl(ctx, branch, slope)
    }

    fn create_welds(
        &self,
        ctx: &Context,
        pipe: RefU64,
        policy: &crate::weld::WeldPolicy,
    ) -> Result<MutationBatch, PipeError> {
        self.create_welds_impl(ctx, pipe, policy)
    }

    fn renumber_welds(&self, ctx: &Context, pipe: RefU64) -> Result<MutationBatch, PipeError> {
        self.renumber_welds_impl(ctx, pipe)
    }

    fn generate_spools(&self, ctx: &Context, pipe: RefU64) -> Result<MutationBatch, PipeError> {
        self.generate_spools_impl(ctx, pipe)
    }

    fn validate_fabrication(
        &self,
        ctx: &Context,
        pipe: RefU64,
    ) -> Result<(crate::outcome::FabStatus, Vec<crate::outcome::FabMessage>), PipeError> {
        self.validate_fabrication_impl(ctx, pipe)
    }

    fn import_rows(
        &self,
        ctx: &Context,
        rows: &crate::loader::ImportTable,
    ) -> Result<(MutationBatch, crate::loader::ImportReport), PipeError> {
        self.import_rows_impl(ctx, rows)
    }

    fn set_spool_break(
        &self,
        ctx: &Context,
        part1: RefU64,
        part2: RefU64,
        mode: crate::fabrication::SpoolBreakMode,
    ) -> Result<MutationBatch, PipeError> {
        self.set_spool_break_impl(ctx, part1, part2, mode)
    }

    fn remove_spool_break(
        &self,
        ctx: &Context,
        part1: RefU64,
        part2: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        self.remove_spool_break_impl(ctx, part1, part2)
    }

    fn set_end_excess(
        &self,
        ctx: &Context,
        arrive_boundary: RefU64,
        leave_boundary: RefU64,
        arrive_excess: f32,
        leave_excess: f32,
    ) -> Result<MutationBatch, PipeError> {
        self.set_end_excess_impl(ctx, arrive_boundary, leave_boundary, arrive_excess, leave_excess)
    }

    fn set_feed_excess(
        &self,
        ctx: &Context,
        bend: RefU64,
        feed: f32,
    ) -> Result<MutationBatch, PipeError> {
        self.set_feed_excess_impl(ctx, bend, feed)
    }

    fn create_pipe_end(
        &self,
        ctx: &Context,
        branch: RefU64,
        tube: RefU64,
        spec: &crate::fabrication::PipeEndSpec,
    ) -> Result<MutationBatch, PipeError> {
        self.create_pipe_end_impl(ctx, branch, tube, spec)
    }

    fn modify_welds(
        &self,
        ctx: &Context,
        welds: &[RefU64],
        edit: &crate::weld::WeldAttrEdit,
    ) -> Result<MutationBatch, PipeError> {
        self.modify_welds_impl(ctx, welds, edit)
    }

    /// ROUTER 空槽：契约定型（§18.11），内核接入前恒 Blocking。
    fn route(
        &self,
        _ctx: &Context,
        _branch: RefU64,
        _via: &[RoutePoint],
    ) -> Result<MutationBatch, PipeError> {
        Err(PipeError::Blocking {
            message: "route: ROUTER 内核未接入（契约见 §18.11，BSTAT 0-10）".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ctx, MockCatalog};
    use std::collections::BTreeMap;

    #[test]
    fn create_pipe_rejects_non_zone_owner() {
        let mut cat = MockCatalog::new();
        cat.nouns.insert(RefU64(9), "SITE".into());
        cat.writable.insert(RefU64(9), true);
        let svc = PipeModelService::new(cat);
        let spec = PipeSpec {
            owner: RefU64(9),
            name: Some("/P1".into()),
            spec: RefU64(77),
            bore_mm: 100.0,
            insulation: None,
            tracing: None,
            temperature: None,
            pressure: None,
            slope_ref: None,
        };
        let err = svc.create_pipe(&ctx(), &spec).unwrap_err();
        assert!(matches!(err, PipeError::Blocking { .. }));
    }

    #[test]
    fn create_pipe_rejects_readonly_owner() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1");
        cat.writable.insert(RefU64(9), false);
        let svc = PipeModelService::new(cat);
        let spec = PipeSpec {
            owner: RefU64(9),
            name: Some("/P1".into()),
            spec: RefU64(77),
            bore_mm: 100.0,
            insulation: None,
            tracing: None,
            temperature: None,
            pressure: None,
            slope_ref: None,
        };
        assert!(matches!(
            svc.create_pipe(&ctx(), &spec).unwrap_err(),
            PipeError::Blocking { .. }
        ));
    }

    #[test]
    fn create_pipe_builds_pipe_and_first_branch() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1");
        let svc = PipeModelService::new(cat);
        let spec = PipeSpec {
            owner: RefU64(9),
            name: Some("/P1".into()),
            spec: RefU64(77),
            bore_mm: 100.0,
            insulation: Some("/INS".into()),
            tracing: None,
            temperature: Some(80.0),
            pressure: None,
            slope_ref: None,
        };
        let batch = svc.create_pipe(&ctx(), &spec).unwrap();
        assert_eq!(batch.ops.len(), 2);

        let MutationOp::Create { refno: pipe_refno, noun, under, name, attrs } = &batch.ops[0]
        else {
            panic!("op0 必须 PIPE Create")
        };
        assert_eq!(noun, "PIPE");
        assert_eq!(*under, RefU64(9));
        assert_eq!(name.as_deref(), Some("/P1"));
        assert_eq!(attrs.get("PSPE"), Some(&NamedAttrValue::RefU64Type(RefU64(77))));
        assert_eq!(attrs.get("BORE"), Some(&NamedAttrValue::F32Type(100.0)));
        assert_eq!(attrs.get("ISPEC"), Some(&NamedAttrValue::StringType("/INS".into())));
        assert_eq!(attrs.get("TEMP"), Some(&NamedAttrValue::F32Type(80.0)));

        let MutationOp::Create { noun, under, name, attrs, .. } = &batch.ops[1] else {
            panic!("op1 必须 BRAN Create")
        };
        assert_eq!(noun, "BRAN");
        // HBOR=TBOR 继承管孔径、自动名、PSPE 继承（Story 2/7）。
        assert_eq!(name.as_deref(), Some("/P1/B1"));
        assert_eq!(attrs.get("HBOR"), Some(&NamedAttrValue::F32Type(100.0)));
        assert_eq!(attrs.get("TBOR"), Some(&NamedAttrValue::F32Type(100.0)));
        assert!(attrs.get("BORE").is_none(), "BRAN 不写 BORE（Story 7）");
        assert_eq!(*under, *pipe_refno, "支管属主 = 新建 PIPE 的编辑段 refno");
    }

    #[test]
    fn branch_name_skips_taken() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        // /P1/B1 已被既有支管占用（E3D 探测窗是 1..=成员数+1，占名者必须是成员）。
        cat.names.insert(RefU64(11), "/P1/B1".into());
        cat.members.insert(RefU64(10), vec![RefU64(11)]);
        let svc = PipeModelService::new(cat);
        let batch = svc.create_branch(&ctx(), RefU64(10)).unwrap();
        let MutationOp::Create { name, .. } = &batch.ops[0] else { panic!() };
        assert_eq!(name.as_deref(), Some("/P1/B2"), "B1 占名时探测窗取 B2");
    }

    #[test]
    fn rename_cascade_rewrites_prefixed_children() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        cat.nouns.insert(RefU64(11), "BRAN".into());
        cat.names.insert(RefU64(11), "/P1/B1".into());
        cat.nouns.insert(RefU64(12), "BRAN".into());
        cat.names.insert(RefU64(12), "/P1/B2".into());
        cat.nouns.insert(RefU64(13), "BRAN".into());
        cat.names.insert(RefU64(13), "/OTHER".into()); // 前缀不匹配，不动
        cat.members.insert(RefU64(10), vec![RefU64(11), RefU64(12), RefU64(13)]);
        let svc = PipeModelService::new(cat);
        let batch = svc.rename_cascade(&ctx(), RefU64(10), "/PX").unwrap();
        let renames: Vec<(RefU64, String)> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::Rename { element, name, .. } => Some((*element, name.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(renames.len(), 3);
        assert_eq!(renames[0], (RefU64(10), "/PX".to_string()));
        assert!(renames.contains(&(RefU64(11), "/PX/B1".to_string())));
        assert!(renames.contains(&(RefU64(12), "/PX/B2".to_string())));
        assert!(!renames.iter().any(|(_, n)| n.contains("OTHER")));
    }

    #[test]
    fn process_data_cascades_only_equal_old_value() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        cat.attrs.get_mut(&RefU64(10)).unwrap()
            .insert("ISPEC".into(), NamedAttrValue::ElementType("/OLD-INS".into()));
        // B1 现值与 PIPE 旧值相同 → 级联；B2 被单独改过 → 不动。
        cat.nouns.insert(RefU64(11), "BRAN".into());
        cat.attrs.insert(RefU64(11), BTreeMap::from([
            ("ISPEC".into(), NamedAttrValue::ElementType("/OLD-INS".into())),
        ]));
        cat.nouns.insert(RefU64(12), "BRAN".into());
        cat.attrs.insert(RefU64(12), BTreeMap::from([
            ("ISPEC".into(), NamedAttrValue::ElementType("/CUSTOM".into())),
        ]));
        cat.members.insert(RefU64(10), vec![RefU64(11), RefU64(12)]);
        let svc = PipeModelService::new(cat);
        let patch = ProcessPatch {
            insulation: Some(Some("/NEW-INS".into())),
            ..Default::default()
        };
        let batch = svc.update_process_data(&ctx(), RefU64(10), &patch).unwrap();
        let touched: Vec<RefU64> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, .. } => Some(*element),
                _ => None,
            })
            .collect();
        assert!(touched.contains(&RefU64(10)));
        assert!(touched.contains(&RefU64(11)));
        assert!(!touched.contains(&RefU64(12)));
    }

    #[test]
    fn edit_end_write_order() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        cat.nouns.insert(RefU64(11), "BRAN".into());
        cat.members.insert(RefU64(10), vec![RefU64(11)]);
        let svc = PipeModelService::new(cat);
        let edit = EndEdit {
            bore_mm: Some(80.0),
            position: Some([0.0, 100.0, 0.0]),
            direction: Some([0.0, 1.0, 0.0]),
            connection: None,
            free: true,
        };
        let batch = svc.edit_branch_end(&ctx(), RefU64(11), BranchEnd::Head, &edit).unwrap();
        // §6 写序：HBOR → HPOS → HDIR → HCON('OPEN') → LHEAD。
        let names: Vec<String> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { attrs, .. } => attrs.keys().next().cloned(),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["HBOR", "HPOS", "HDIR", "HCON", "LHEAD"]);
    }

    #[test]
    fn connect_missing_target_goes_unassigned() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        cat.nouns.insert(RefU64(11), "BRAN".into());
        let svc = PipeModelService::new(cat);
        let (batch, outcome) = svc
            .connect_end(&ctx(), RefU64(11), BranchEnd::Tail, RefU64(999))
            .unwrap();
        // FCONN 占位 + minTube 警告（§7 兜底路径二显式化）。
        assert!(matches!(
            outcome,
            ConnectOutcome::Unassigned { placeholder_length_mm } if placeholder_length_mm == 100.0
        ));
        assert!(!batch.warnings.is_empty());
    }

    #[test]
    fn disconnect_nulls_ref() {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1").pipe(RefU64(10), "/P1", RefU64(77), 100.0);
        cat.nouns.insert(RefU64(11), "BRAN".into());
        let svc = PipeModelService::new(cat);
        let batch = svc.disconnect_end(&ctx(), RefU64(11), BranchEnd::Tail).unwrap();
        let MutationOp::SetAttrs { attrs, .. } = &batch.ops[0] else { panic!() };
        assert_eq!(
            attrs.get("TREF"),
            Some(&NamedAttrValue::RefU64Type(RefU64(0)))
        );
    }
}
