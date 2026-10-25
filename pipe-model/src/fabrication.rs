//! I 组：制造（spec Phase D；功能文档 §8、§18.10、§18.14）。
//!
//! 语义要点：
//! - `generate_spools`/`validate_fabrication` 为接口面；FAB_* 内核是
//!   一致性边界，实现可用托管 PF* 等价语义（文档已录）
//! - datacon 门控 = `pipeValidDataCheck` 谓词表（§18.10）：只认
//!   22-55（57 除外）/63-84/87/89-91 错误码段
//! - 重生成保名 = `PSPOOLLIST` 比较器（§18.14）：FULL/COMPS/START/END/NONE
//!   四档匹配 + option 1-3 保名策略 + 先 unname 再写

use crate::batch::{MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::context::Context;
use crate::error::PipeError;
use crate::outcome::{FabMessage, FabStatus, PipeWarning};
use crate::service::PipeModelService;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// `pipeValidDataCheck` 错误码段（§18.10）：段内码才算 spool 相关不一致。
/// 57 被刻意排除（U-bend 合法）。
pub const DATACON_BLOCKING_RANGES: &[(u32, u32)] = &[
    (22, 55), // 不含 57：见 datacon_blocking(57)
    (63, 84),
    (87, 87),
    (89, 91),
];

/// 单个错误码是否挡制造（pipeValidDataCheck 谓词翻译）。
pub fn datacon_blocking(code: u32) -> bool {
    code != 57 && DATACON_BLOCKING_RANGES.iter().any(|(lo, hi)| code >= *lo && code <= *hi)
}

/// 管段重生成比较档（PSPOOLLIST，§18.14）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolMatch {
    /// 排序后 componentList 全等且体积未变。
    Full,
    /// componentList 全等但 volume 变。
    Comps,
    /// 前缀连续命中。
    Start,
    /// 后缀连续命中。
    End,
    None,
}

/// 保名策略（PSPOOLLIST option 1-3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameReuse {
    /// 1：START/END/COMPS 也复用旧名。
    Maximal,
    /// 2：只 FULL+COMPS 复用。
    Strict,
    /// 3：一律换新名（namOpt 自动命名）。
    Fresh,
}

/// 默认弯管机名（initializeFabrication 默认机表）。
pub const DEFAULT_BEND_MACHINE: &str = "/BENDING-MACHINE-1(3D)";
/// 默认料长上限 mm。
pub const DEFAULT_STOCK_LEN_MM: f32 = 6000.0;

// ---- L 组：断点/余量/管端（spec Story 51-56；§18.16）-------------------

/// spool 断点形态（§18.16 双形态模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolBreakMode {
    /// No-weld：元素标志位 `ASFBR`/`LSFBR`/`CSFBR`/`TSFBR` 成对写。
    NoWeld,
    /// Weld：真 WELD 件 `/WELDGAP`+STYPE FWS+`DESP[1]=0.001m`+SHOP=false。
    Weld,
}

/// 断点边界（内部解析结果）。
#[derive(Debug)]
enum BreakBoundary {
    /// 同支管成员对：`first` 的 leave 侧 ↔ `second` 的 arrive 侧。
    Adjacent { first: RefU64, second: RefU64, branch: RefU64 },
    /// 同支管但隔管片：标志只打 part1 面向 part2 的一侧（§18.16 existTube 分支）。
    WithTube { part: RefU64, leave_side: bool, branch: RefU64 },
    /// 跨支管 TEE/OLET cref 连接：tee.csfbreak + 对方 BRAN.tsfbreak。
    TeeCref { tee: RefU64, other_branch: RefU64 },
}

impl BreakBoundary {
    /// 断口所在支管（TeeCref 时取对方支管——焊件挂在它名下）。
    fn branch(&self) -> RefU64 {
        match self {
            BreakBoundary::Adjacent { branch, .. } | BreakBoundary::WithTube { branch, .. } => {
                *branch
            }
            BreakBoundary::TeeCref { other_branch, .. } => *other_branch,
        }
    }
}

/// FTUB 管端参数（`pfFabricationPipeEndControl::createPipeEnd` 提交面）。
#[derive(Debug, Clone)]
pub struct PipeEndSpec {
    /// head（流向 +1）还是 tail（-1）。
    pub is_head: bool,
    /// 斜切端（true=`/SLANTEDPIPEEND`）还是螺纹端（false=`/THREADPIPEEND`）。
    pub is_slanted: bool,
    /// 斜切角度（螺纹端恒 0）。
    pub angle_deg: f64,
    /// 螺纹连接型（`threadInfo` 的 `@` 前半；DESP 串面写不下时进 warning）。
    pub thread_type: Option<String>,
    /// 管外径 mm。
    pub tube_od: f64,
    /// 内管长 mm（`ltLength`——FLSO 属主扣法兰厚度由调用方预先折算）。
    pub tube_len: f64,
    /// MATN 备注（`{deg}` 占位由调用方替换）。
    pub note: Option<String>,
}

/// 解析两元件间的断点边界。
fn break_boundary<C: PipeCatalog>(
    cat: &C,
    part1: RefU64,
    part2: RefU64,
) -> Result<BreakBoundary, PipeError> {
    let (own1, own2) = (cat.owner_of(part1), cat.owner_of(part2));
    if own1.is_some() && own1 == own2 {
        let branch = own1.unwrap();
        let members = cat.members_of(branch);
        let (i1, i2) = (
            members.iter().position(|r| *r == part1),
            members.iter().position(|r| *r == part2),
        );
        match (i1, i2) {
            (Some(a), Some(b)) if a + 1 == b => {
                Ok(BreakBoundary::Adjacent { first: part1, second: part2, branch })
            }
            (Some(a), Some(b)) if b + 1 == a => {
                Ok(BreakBoundary::Adjacent { first: part2, second: part1, branch })
            }
            (Some(a), Some(b)) => Ok(BreakBoundary::WithTube {
                part: part1,
                leave_side: a < b,
                branch,
            }),
            _ => Err(PipeError::Blocking {
                message: "元件不在同支管成员表内".into(),
            }),
        }
    } else {
        // 跨支管：part1 必须是 TEE/OLET（cref 侧断点），标志成对打到对方 BRAN。
        let n1 = cat.noun_of(part1).unwrap_or_default();
        if matches!(n1.as_str(), "TEE" | "OLET") && own2.is_some() {
            Ok(BreakBoundary::TeeCref { tee: part1, other_branch: own2.unwrap() })
        } else {
            Err(PipeError::Blocking {
                message: format!("跨支管断点要求首件为 TEE/OLET（cref 侧），实得 {n1}"),
            })
        }
    }
}

/// 断点候选校验（§18.16 `getSpoolBreakCandidates` 成员过滤面）。
/// TUBI 需 `itlength ≥ 1mm`（无属性放行——E3D `(2,201)` 语义为不收，
/// 但读面拿不到时本服务按可候选处理并 warning）；ATTA 需 `SPKBRK`。
fn break_candidate_ok<C: PipeCatalog>(
    cat: &C,
    part: RefU64,
    batch: &mut MutationBatch,
) -> Result<(), PipeError> {
    match cat.noun_of(part).as_deref() {
        Some("TUBI") => {
            match cat.attr(part, "ITLENGTH").and_then(|v| match v {
                NamedAttrValue::F32Type(v) => Some(v),
                _ => None,
            }) {
                Some(v) if v < 1.0 => Err(PipeError::Blocking {
                    message: "TUBI itlength<1mm 不能作断点宿主".into(),
                }),
                None => {
                    batch.warnings.push(PipeWarning {
                        message: "TUBI 无 ITLENGTH 属性，按可候选处理（E3D 不收此类）".into(),
                        side_effect: None,
                    });
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Some("ATTA") => match cat.attr(part, "SPKBRK") {
            Some(NamedAttrValue::BoolType(true)) => Ok(()),
            _ => Err(PipeError::Blocking {
                message: "ATTA 无 SPKBRK 标志，不能作断点宿主".into(),
            }),
        },
        _ => Ok(()),
    }
}

fn flag_attrs(pairs: &[(RefU64, &str, bool)]) -> Vec<MutationOp> {
    pairs
        .iter()
        .map(|(e, k, v)| MutationOp::SetAttrs {
            element: *e,
            attrs: NamedAttrMap {
                map: [((*k).to_string(), NamedAttrValue::BoolType(*v))]
                    .into_iter()
                    .collect(),
            },
        })
        .collect()
}

/// 边界处的现场焊探测（位距判重 <0.002 属几何面，本实现做成员级近似：
/// 边界两侧若是 WELD 且 `SHOP=false` 即认作断口焊）。
fn field_weld_at<C: PipeCatalog>(cat: &C, boundary: &BreakBoundary) -> Option<RefU64> {
    let shop_false = |r: RefU64| {
        cat.noun_of(r).as_deref() == Some("WELD")
            && cat.attr(r, "SHOP") != Some(NamedAttrValue::BoolType(true))
    };
    match boundary {
        BreakBoundary::Adjacent { first, second, branch } => {
            let members = cat.members_of(*branch);
            let (i1, i2) = (
                members.iter().position(|r| r == first),
                members.iter().position(|r| r == second),
            );
            match (i1, i2) {
                (Some(a), Some(b)) if b > a => members[a + 1..b].iter().copied().find(|r| shop_false(*r)),
                _ => None,
            }
        }
        BreakBoundary::WithTube { .. } => None,
        BreakBoundary::TeeCref { other_branch, .. } => cat
            .members_of(*other_branch)
            .into_iter()
            .find(|r| shop_false(*r)),
    }
}

impl<C: PipeCatalog> PipeModelService<C> {
    /// 分段/管片生成接口面（Story 39-43）。
    ///
    /// 本实现覆盖**结构面**：每支管一个 PSPOOL（`PSTYP=SHOP`）+ 每 TUBI 一个
    /// PPIECE（CutLen/BENDMC/WELDMC）。断管算法本体（PFEVALUATESPOOLMEMBER
    /// 的 asfbreak/lsfbreak/csfbreak 扫描、href/tref/cref 跨支管续扫、TEE
    /// off-leg 递归）属 FAB_* 一致性边界——实现接入托管 PF* 参照后替换。
    pub(crate) fn generate_spools_impl(
        &self,
        _ctx: &Context,
        pipe: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(pipe).as_deref() != Some("PIPE") {
            return Err(PipeError::Blocking {
                message: "generate_spools 目标必须 PIPE".into(),
            });
        }
        let mut batch = MutationBatch::named("generate_spools");
        let pipe_name = self.catalog.name_of(pipe).unwrap_or_default();
        let bend_mc = self.catalog.refno_by_name(DEFAULT_BEND_MACHINE);
        if bend_mc.is_none() {
            batch.warnings.push(PipeWarning {
                message: format!("默认弯管机 {DEFAULT_BEND_MACHINE} 不在数据面，BENDMC 跳过"),
                side_effect: None,
            });
        }
        let mut spool_n = 0u32;
        for bran in self.catalog.members_of(pipe) {
            if self.catalog.noun_of(bran).as_deref() != Some("BRAN") {
                continue;
            }
            spool_n += 1;
            let spool = self.catalog.alloc_refno(bran);
            let mut spool_attrs = NamedAttrMap::default();
            spool_attrs.insert("PSTYP".into(), NamedAttrValue::WordType("SHOP".into()));
            spool_attrs.insert("PSPR".into(), NamedAttrValue::RefU64Type(pipe));
            batch.ops.push(MutationOp::Create {
                refno: spool,
                noun: "PSPOOL".into(),
                under: pipe,
                name: Some(format!("{pipe_name}-SPL{spool_n}")),
                attrs: spool_attrs,
            });
            for m in self.catalog.members_of(bran) {
                if self.catalog.noun_of(m).as_deref() != Some("TUBI") {
                    continue;
                }
                let mut piece_attrs = NamedAttrMap::default();
                if let Some(mc) = bend_mc {
                    piece_attrs.insert("BENDMC".into(), NamedAttrValue::RefU64Type(mc));
                }
                piece_attrs.insert(
                    "PPCUTL".into(),
                    NamedAttrValue::F32Type(DEFAULT_STOCK_LEN_MM),
                );
                batch.ops.push(MutationOp::Create {
                    refno: self.catalog.alloc_refno(bran),
                    noun: "PPIECE".into(),
                    under: spool,
                    name: None,
                    attrs: piece_attrs,
                });
            }
        }
        if spool_n == 0 {
            batch.warnings.push(PipeWarning {
                message: "管下无支管，未生成 PSPOOL".into(),
                side_effect: None,
            });
        }
        Ok(batch)
    }

    /// 制造校验面（Story 42-44）：属性级缺陷检查 + 三级聚合。
    ///
    /// 检查项（datacon 码段谓词的属性面近似——`CHECK` 内核接入后换真码）：
    /// - 非 TUBI/WELD 元件缺 SPREF → Invalid
    /// - 支管缺 HBOR/TBOR → Invalid
    /// - 元件缺定位（APOS+LPOS 皆无）→ Warning 级
    /// 聚合：任一 Invalid → Failed；仅 Warning → ValidWithWarning；干净 → Valid。
    pub(crate) fn validate_fabrication_impl(
        &self,
        _ctx: &Context,
        pipe: RefU64,
    ) -> Result<(FabStatus, Vec<FabMessage>), PipeError> {
        if self.catalog.noun_of(pipe).as_deref() != Some("PIPE") {
            return Err(PipeError::Blocking {
                message: "validate_fabrication 目标必须 PIPE".into(),
            });
        }
        let mut messages = Vec::new();
        for bran in self.catalog.members_of(pipe) {
            if self.catalog.noun_of(bran).as_deref() != Some("BRAN") {
                continue;
            }
            for k in ["HBOR", "TBOR"] {
                if self.catalog.attr(bran, k).is_none() {
                    messages.push(FabMessage {
                        status: FabStatus::Failed,
                        element: Some(bran),
                        text: format!("支管缺 {k}"),
                    });
                }
            }
            for m in self.catalog.members_of(bran) {
                let noun = self.catalog.noun_of(m).unwrap_or_default();
                if matches!(noun.as_str(), "TUBI" | "WELD") {
                    continue;
                }
                if self.catalog.attr(m, "SPREF").is_none() {
                    messages.push(FabMessage {
                        status: FabStatus::Failed,
                        element: Some(m),
                        text: format!("{noun} 缺 SPREF（规格元件未选定）"),
                    });
                }
                if self.catalog.attr(m, "APOS").is_none()
                    && self.catalog.attr(m, "LPOS").is_none()
                {
                    messages.push(FabMessage {
                        status: FabStatus::ValidWithWarning,
                        element: Some(m),
                        text: format!("{noun} 无 APOS/LPOS 定位值可读"),
                    });
                }
            }
        }
        let status = if messages.iter().any(|m| m.status == FabStatus::Failed) {
            FabStatus::Failed
        } else if messages.is_empty() {
            FabStatus::Valid
        } else {
            FabStatus::ValidWithWarning
        };
        // Story 42：Release 门控——全选管 ValidWithWarning 才放行；本函数
        // 只报告状态，门控在调用方。
        Ok((status, messages))
    }

    /// 设置 spool 断点（Story 51-53）：双形态——No-weld 写标志对，Weld 建
    /// `/WELDGAP` 现场焊。形态互遇走 `requires_confirm` 替换语义；重复点击
    /// 同形断点即 `remove_spool_break` 的确认路径（E3D exists→"OK to remove?"）。
    pub(crate) fn set_spool_break_impl(
        &self,
        _ctx: &Context,
        part1: RefU64,
        part2: RefU64,
        mode: SpoolBreakMode,
    ) -> Result<MutationBatch, PipeError> {
        let mut batch = MutationBatch::named("set_spool_break");
        break_candidate_ok(&self.catalog, part1, &mut batch)?;
        break_candidate_ok(&self.catalog, part2, &mut batch)?;
        let boundary = break_boundary(&self.catalog, part1, part2)?;

        // 边界各侧的标志位对。
        let flags: Vec<(RefU64, &'static str)> = match &boundary {
            BreakBoundary::Adjacent { first, second, .. } => {
                vec![(*first, "LSFBR"), (*second, "ASFBR")]
            }
            BreakBoundary::WithTube { part, leave_side, .. } => {
                vec![(*part, if *leave_side { "LSFBR" } else { "ASFBR" })]
            }
            BreakBoundary::TeeCref { tee, other_branch } => {
                vec![(*tee, "CSFBR"), (*other_branch, "TSFBR")]
            }
        };
        let flags_set = flags.iter().all(|(e, k)| {
            self.catalog.attr(*e, k) == Some(NamedAttrValue::BoolType(true))
        });
        let weld = field_weld_at(&self.catalog, &boundary);

        match mode {
            SpoolBreakMode::NoWeld => {
                if flags_set {
                    // 已设同形断点 → 确认后清除（E3D exists→OK to remove）。
                    batch.requires_confirm.push(crate::batch::Confirm { prompt: "OK to remove spool limit?".into(), on_yes: Vec::new() });
                    batch.ops.extend(flags.iter().map(|(e, k)| MutationOp::SetAttrs {
                        element: *e,
                        attrs: NamedAttrMap {
                            map: [((*k).to_string(), NamedAttrValue::BoolType(false))]
                                .into_iter()
                                .collect(),
                        },
                    }));
                } else {
                    if let Some(w) = weld {
                        // Story 53 替换语义：现场焊 → confirm → 删焊再打标志。
                        batch.requires_confirm.push(crate::batch::Confirm { prompt: "Spool limit exists as a weld. Replace with 'No weld'?".into(), on_yes: Vec::new() });
                        batch.ops.push(MutationOp::Delete { element: w });
                    }
                    batch.ops.extend(flag_attrs(
                        &flags.iter().map(|(e, k)| (*e, *k, true)).collect::<Vec<_>>(),
                    ));
                }
            }
            SpoolBreakMode::Weld => {
                if weld.is_some() {
                    batch.requires_confirm.push(crate::batch::Confirm { prompt: "OK to remove spool-limit weld?".into(), on_yes: Vec::new() });
                    batch.ops.push(MutationOp::Delete { element: weld.unwrap() });
                    return Ok(batch);
                }
                if flags_set {
                    batch.requires_confirm.push(crate::batch::Confirm { prompt: "Spool limit exists as 'No weld'. Replace with a weld?".into(), on_yes: Vec::new() });
                    batch.ops.extend(flag_attrs(
                        &flags.iter().map(|(e, k)| (*e, *k, false)).collect::<Vec<_>>(),
                    ));
                }
                // 邻接 SHOP 件侧不打现场断点（candidate[3]=!prev.shop 语义：
                // SHOP 件边界自带 shop 焊界，断点等价已存在）。
                if let BreakBoundary::Adjacent { first, second, .. } = &boundary {
                    for side in [first, second] {
                        if self.catalog.attr(*side, "SHOP")
                            == Some(NamedAttrValue::BoolType(true))
                        {
                            batch.warnings.push(PipeWarning {
                                message: "邻接 SHOP 元件侧：断点已由 shop 焊界等价存在".into(),
                                side_effect: None,
                            });
                            return Ok(batch);
                        }
                    }
                }
                // 建 WELD：`/WELDGAP` 规格 STYPE=FWS 求解；无规格门面退化为
                // 名址引用 + warning（规格补选留 UI 回落）。
                let weld_ref = self.catalog.alloc_refno(boundary.branch());
                let mut attrs = NamedAttrMap::default();
                attrs.insert("SHOP".into(), NamedAttrValue::BoolType(false));
                attrs.insert(
                    "DESP".into(),
                    NamedAttrValue::F32VecType(vec![0.001]),
                );
                match self.resolve_weldgap() {
                    Some(spco) => {
                        attrs.insert("SPREF".into(), NamedAttrValue::RefU64Type(spco));
                    }
                    None => {
                        batch.warnings.push(PipeWarning {
                            message: "/WELDGAP STYPE=FWS 规格未命中，SPREF 缺省".into(),
                            side_effect: None,
                        });
                    }
                }
                batch.ops.push(MutationOp::Create {
                    refno: weld_ref,
                    noun: "WELD".into(),
                    under: match &boundary {
                        BreakBoundary::Adjacent { branch, .. }
                        | BreakBoundary::WithTube { branch, .. } => *branch,
                        BreakBoundary::TeeCref { other_branch, .. } => *other_branch,
                    },
                    name: None,
                    attrs,
                });
                // 插入序位：邻接对插在 first 之后（E3D `NEW WELD FORWARD` 等价）。
                if let BreakBoundary::Adjacent { first, branch, .. } = &boundary {
                    let pos = self
                        .catalog
                        .members_of(*branch)
                        .iter()
                        .position(|r| r == first)
                        .map(|i| i + 1);
                    batch.ops.push(MutationOp::Include {
                        element: weld_ref,
                        into: *branch,
                        order: pos,
                    });
                }
                batch.warnings.push(PipeWarning {
                    message: "CONNECT→FCONNECT→ORIENTATE 序属连接内核，weld 定位/对接待内核".into(),
                    side_effect: None,
                });
            }
        }
        Ok(batch)
    }

    /// 删除 spool 断点（Story 53 反向）：清标志对 + 删边界现场焊。
    pub(crate) fn remove_spool_break_impl(
        &self,
        _ctx: &Context,
        part1: RefU64,
        part2: RefU64,
    ) -> Result<MutationBatch, PipeError> {
        let mut batch = MutationBatch::named("remove_spool_break");
        let boundary = break_boundary(&self.catalog, part1, part2)?;
        batch.requires_confirm.push(crate::batch::Confirm { prompt: "OK to remove spool limit?".into(), on_yes: Vec::new() });
        let flags: Vec<(RefU64, &'static str)> = match &boundary {
            BreakBoundary::Adjacent { first, second, .. } => {
                vec![(*first, "LSFBR"), (*second, "ASFBR")]
            }
            BreakBoundary::WithTube { part, leave_side, .. } => {
                vec![(*part, if *leave_side { "LSFBR" } else { "ASFBR" })]
            }
            BreakBoundary::TeeCref { tee, other_branch } => {
                vec![(*tee, "CSFBR"), (*other_branch, "TSFBR")]
            }
        };
        batch.ops.extend(flag_attrs(
            &flags.iter().map(|(e, k)| (*e, *k, false)).collect::<Vec<_>>(),
        ));
        if let Some(w) = field_weld_at(&self.catalog, &boundary) {
            batch.ops.push(MutationOp::Delete { element: w });
        }
        Ok(batch)
    }

    /// 端余量编辑（Story 54）：piece 边界对 `LEXCES`/`AEXCES` 直写。
    /// `arrive_boundary`/`leave_boundary` 为 `pfGetTubeParts` 划出的边界件。
    pub(crate) fn set_end_excess_impl(
        &self,
        _ctx: &Context,
        arrive_boundary: RefU64,
        leave_boundary: RefU64,
        arrive_excess: f32,
        leave_excess: f32,
    ) -> Result<MutationBatch, PipeError> {
        let mut batch = MutationBatch::named("set_end_excess");
        for (e, k, v) in [
            (arrive_boundary, "LEXCES", arrive_excess),
            (leave_boundary, "AEXCES", leave_excess),
        ] {
            batch.ops.push(MutationOp::SetAttrs {
                element: e,
                attrs: NamedAttrMap {
                    map: [(k.to_string(), NamedAttrValue::F32Type(v))]
                        .into_iter()
                        .collect(),
                },
            });
        }
        Ok(batch)
    }

    /// 进给余量编辑（Story 54）：`BEND.USEFED` 直写。
    pub(crate) fn set_feed_excess_impl(
        &self,
        _ctx: &Context,
        bend: RefU64,
        feed: f32,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(bend).as_deref() != Some("BEND") {
            return Err(PipeError::Blocking {
                message: "进给余量只适用于 BEND（弯管机 feed）".into(),
            });
        }
        let mut batch = MutationBatch::named("set_feed_excess");
        batch.ops.push(MutationOp::SetAttrs {
            element: bend,
            attrs: NamedAttrMap {
                map: [("USEFED".to_string(), NamedAttrValue::F32Type(feed))]
                    .into_iter()
                    .collect(),
            },
        });
        Ok(batch)
    }

    /// FTUB 螺纹/斜切管端（Story 55；`pfFabricationPipeEndControl::createPipeEnd`）。
    ///
    /// 数据面：FTUB Create（`SHOP=T`、`MTOC=F`、`DESP[1..3]`=角度/外径/流向、
    /// `LSTU` 继承）+ 插入序位（head→首、tail→管片后）。内核面（pos/ori 推导、
    /// `CONN PH/PT`、重量 `desp[5]=0.5·uwei·fitlen`、斜切 `pfCalculateSlantedEndData`）
    /// 全部显式 warning。
    pub(crate) fn create_pipe_end_impl(
        &self,
        _ctx: &Context,
        branch: RefU64,
        tube: RefU64,
        spec: &PipeEndSpec,
    ) -> Result<MutationBatch, PipeError> {
        if self.catalog.noun_of(branch).as_deref() != Some("BRAN") {
            return Err(PipeError::Blocking {
                message: "create_pipe_end 目标必须 BRAN".into(),
            });
        }
        // 斜切校验：cutLen = OD/tan(90°-α) ≥ 管长 → 拒绝（§18.16）。
        if spec.is_slanted {
            let cut_len = spec.tube_od / (90.0_f64 - spec.angle_deg).to_radians().tan();
            if cut_len >= spec.tube_len {
                return Err(PipeError::Blocking {
                    message: "Cannot make slanted end. The angle exceeds original tube length".into(),
                });
            }
        }
        // SPREF 名址解析 + LIMBOSPEC 门禁。
        let mut batch = MutationBatch::named("create_pipe_end");
        let spec_name = if spec.is_slanted { "/SLANTEDPIPEEND" } else { "/THREADPIPEEND" };
        if let Some(spref) = self.catalog.refno_by_name(spec_name) {
            let mut limbo = false;
            let mut cur = spref;
            for _ in 0..8 {
                if self
                    .catalog
                    .name_of(cur)
                    .map(|n| n.contains("LIMBOSPEC"))
                    .unwrap_or(false)
                {
                    limbo = true;
                    break;
                }
                match self.catalog.owner_of(cur) {
                    Some(o) if o != cur && o != RefU64::default() => cur = o,
                    _ => break,
                }
            }
            if limbo {
                return Err(PipeError::Blocking {
                    message: "Function unavailable. Reinstate the specification from the Limbo spec.".into(),
                });
            }
        } else {
            batch.warnings.push(PipeWarning {
                message: format!("{spec_name} 不在数据面，SPREF 缺省"),
                side_effect: None,
            });
        }

        // LSTU 继承：head→HSTU OF BRAN；tail→LSTU OF PREV（缺→HSTU OF BRAN）。
        let lstu = if spec.is_head {
            self.catalog.attr(branch, "HSTU")
        } else {
            let members = self.catalog.members_of(branch);
            members
                .iter()
                .position(|r| *r == tube)
                .and_then(|i| (i > 0).then(|| members[i - 1]))
                .and_then(|prev| self.catalog.attr(prev, "LSTU"))
                .or_else(|| self.catalog.attr(branch, "HSTU"))
        };

        let flow: f32 = if spec.is_head { 1.0 } else { -1.0 };
        let ftub = self.catalog.alloc_refno(branch);
        let mut attrs = NamedAttrMap::default();
        attrs.insert("SHOP".into(), NamedAttrValue::BoolType(true));
        attrs.insert("MTOC".into(), NamedAttrValue::BoolType(false));
        attrs.insert(
            "DESP".into(),
            NamedAttrValue::F32VecType(vec![
                spec.angle_deg as f32,
                spec.tube_od as f32,
                flow,
            ]),
        );
        if let Some(l) = lstu {
            attrs.insert("LSTU".into(), l);
        }
        if let Some(spref) = self.catalog.refno_by_name(spec_name) {
            attrs.insert("SPREF".into(), NamedAttrValue::RefU64Type(spref));
        }
        if let Some(n) = &spec.note {
            attrs.insert("MATN".into(), NamedAttrValue::StringType(n.clone()));
        }
        batch.ops.push(MutationOp::Create {
            refno: ftub,
            noun: "FTUB".into(),
            under: branch,
            name: None,
            attrs,
        });
        // 序位：head→成员表首（E3D 插在支管头并 CONN PH TO FIRST MEM）；
        // tail→管片之后（CONN PT TO LAST MEM 等价插入位）。
        let pos = if spec.is_head {
            Some(0)
        } else {
            self.catalog
                .members_of(branch)
                .iter()
                .position(|r| *r == tube)
                .map(|i| i + 1)
        };
        batch.ops.push(MutationOp::Include { element: ftub, into: branch, order: pos });
        batch.warnings.push(PipeWarning {
            message: format!(
                "FTUB 内核面待接：pos/ori 推导（LINE(aPos,lPos)→ORIENTATION）、\
                 CONN PH/PT 接线、desp[4]=连接型（字面值入参数面）、\
                 desp[5]=0.5·uwei·fitlen 重量、setToRefPlane 清距修正"
            ),
            side_effect: None,
        });
        Ok(batch)
    }

    /// `/WELDGAP` 规格的 STYPE=FWS SPCO 求解（断口焊专用）。
    /// 走注入的 [`crate::spec_resolver::SpecCatalog`]；未接读面返回 None。
    fn resolve_weldgap(&self) -> Option<RefU64> {
        let spec_cat = self.spec.as_ref()?;
        let weldgap = self.catalog.refno_by_name("/WELDGAP")?;
        let qa = vec![("STYPE".to_string(), "FWS".to_string())];
        spec_cat.table_lookup(weldgap, "WELD", &qa).or_else(|| {
            let mut node = spec_cat.root_selector(weldgap);
            for _ in 0..32 {
                let next = spec_cat.sele_descend(node?, &qa)?;
                if spec_cat.is_spco(next) {
                    return Some(next);
                }
                node = Some(next);
            }
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PipeModel;
    use crate::testkit::{ctx, MockCatalog};

    #[test]
    fn datacon_ranges() {
        assert!(datacon_blocking(22));
        assert!(datacon_blocking(55));
        assert!(!datacon_blocking(57)); // U-bend 刻意放行
        assert!(datacon_blocking(63));
        assert!(!datacon_blocking(85));
        assert!(datacon_blocking(89));
        assert!(!datacon_blocking(1));
    }

    /// 基础管线：管+支管+机器名。
    fn rig() -> MockCatalog {
        let mut cat = MockCatalog::new();
        cat.zone(RefU64(9), "/Z1")
            .pipe(RefU64(10), "/P1", RefU64(77), 100.0)
            .branch(RefU64(11), "/P1/B1", RefU64(10), RefU64(77));
        cat.names.insert(RefU64(90), DEFAULT_BEND_MACHINE.into());
        cat
    }

    #[test]
    fn generate_spools_per_branch_and_tubi() {
        let mut cat = rig();
        for (r, n) in [(RefU64(21), "TUBI"), (RefU64(22), "ELBO"), (RefU64(23), "TUBI")] {
            cat.nouns.insert(r, n.into());
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        let svc = PipeModelService::new(cat);
        let batch = svc.generate_spools(&ctx(), RefU64(10)).unwrap();
        let spools: Vec<_> = batch
            .ops
            .iter()
            .filter(|o| matches!(o, MutationOp::Create { noun, .. } if noun == "PSPOOL"))
            .collect();
        let pieces: Vec<_> = batch
            .ops
            .iter()
            .filter(|o| matches!(o, MutationOp::Create { noun, .. } if noun == "PPIECE"))
            .collect();
        assert_eq!(spools.len(), 1);
        assert_eq!(pieces.len(), 2, "每 TUBI 一管片");
        let MutationOp::Create { refno: spool, .. } = spools[0] else { panic!() };
        assert!(pieces.iter().all(|p| matches!(p,
            MutationOp::Create { under, .. } if under == spool)));
        // 弯管机默认 + PSTYP。
        let MutationOp::Create { attrs, .. } = pieces[0] else { panic!() };
        assert_eq!(attrs.get("BENDMC"), Some(&NamedAttrValue::RefU64Type(RefU64(90))));
    }

    #[test]
    fn validate_reports_missing_spref() {
        let mut cat = rig();
        // ELBO 缺 SPREF → Failed 聚合。
        cat.nouns.insert(RefU64(22), "ELBO".into());
        cat.members.entry(RefU64(11)).or_default().push(RefU64(22));
        cat.attrs.get_mut(&RefU64(11)).unwrap().extend([
            ("HBOR".into(), NamedAttrValue::F32Type(100.0)),
            ("TBOR".into(), NamedAttrValue::F32Type(100.0)),
        ]);
        let svc = PipeModelService::new(cat);
        let (status, messages) = svc.validate_fabrication(&ctx(), RefU64(10)).unwrap();
        assert_eq!(status, FabStatus::Failed);
        assert!(messages.iter().any(|m| m.element == Some(RefU64(22))
            && m.status == FabStatus::Failed));
    }

    #[test]
    fn validate_clean_pipe() {
        let mut cat = rig();
        cat.attrs.get_mut(&RefU64(11)).unwrap().extend([
            ("HBOR".into(), NamedAttrValue::F32Type(100.0)),
            ("TBOR".into(), NamedAttrValue::F32Type(100.0)),
        ]);
        let svc = PipeModelService::new(cat);
        let (status, messages) = svc.validate_fabrication(&ctx(), RefU64(10)).unwrap();
        assert_eq!(status, FabStatus::Valid);
        assert!(messages.is_empty());
    }

    // ---- L 组测试（Story 51-56）-----------------------------------------

    /// 支管（11）：ELBO(21) - TUBI(22,itlength=50) - ELBO(23)。
    fn break_rig() -> MockCatalog {
        let mut cat = rig();
        for (r, n) in [(RefU64(21), "ELBO"), (RefU64(22), "TUBI"), (RefU64(23), "ELBO")] {
            cat.nouns.insert(r, n.into());
            cat.owners.insert(r, RefU64(11));
            cat.members.entry(RefU64(11)).or_default().push(r);
        }
        cat.attrs.insert(RefU64(22), std::collections::BTreeMap::from([
            ("ITLENGTH".into(), NamedAttrValue::F32Type(50.0)),
        ]));
        cat
    }

    fn set_attr(cat: &mut MockCatalog, r: RefU64, k: &str, v: NamedAttrValue) {
        cat.attrs.entry(r).or_default().insert(k.into(), v);
    }

    #[test]
    fn no_weld_break_writes_flag_pair() {
        let svc = PipeModelService::new(break_rig());
        let batch = svc
            .set_spool_break(&ctx(), RefU64(21), RefU64(22), SpoolBreakMode::NoWeld)
            .unwrap();
        assert!(batch.requires_confirm.is_empty());
        let pairs: Vec<(RefU64, &str, bool)> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, attrs } => Some((
                    *element,
                    attrs.map.keys().next()?.as_str(),
                    matches!(attrs.map.values().next(), Some(NamedAttrValue::BoolType(true))),
                )),
                _ => None,
            })
            .collect();
        // 邻接对：first.lsfbreak + second.asfbreak。
        assert_eq!(pairs, vec![(RefU64(21), "LSFBR", true), (RefU64(22), "ASFBR", true)]);
    }

    #[test]
    fn no_weld_toggle_confirms_and_clears() {
        let mut cat = break_rig();
        set_attr(&mut cat, RefU64(21), "LSFBR", NamedAttrValue::BoolType(true));
        set_attr(&mut cat, RefU64(22), "ASFBR", NamedAttrValue::BoolType(true));
        let svc = PipeModelService::new(cat);
        let batch = svc
            .set_spool_break(&ctx(), RefU64(21), RefU64(22), SpoolBreakMode::NoWeld)
            .unwrap();
        assert_eq!(batch.requires_confirm.len(), 1);
        assert!(batch.ops.iter().all(|op| matches!(op,
            MutationOp::SetAttrs { attrs, .. }
                if matches!(attrs.map.values().next(), Some(NamedAttrValue::BoolType(false))))));
    }

    #[test]
    fn weld_break_creates_field_weld() {
        let svc = PipeModelService::new(break_rig());
        let batch = svc
            .set_spool_break(&ctx(), RefU64(21), RefU64(22), SpoolBreakMode::Weld)
            .unwrap();
        let weld = batch.ops.iter().find(|op| matches!(op,
            MutationOp::Create { noun, .. } if noun == "WELD")).expect("WELD Create");
        let MutationOp::Create { refno, attrs, under, .. } = weld else { panic!() };
        assert_eq!(*under, RefU64(11));
        assert_eq!(attrs.get("SHOP"), Some(&NamedAttrValue::BoolType(false)));
        assert_eq!(attrs.get("DESP"), Some(&NamedAttrValue::F32VecType(vec![0.001])));
        // 序位：插在 first(21) 之后。
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Include { element, into, order }
                if element == refno && *into == RefU64(11) && *order == Some(1))));
        assert!(batch.warnings.iter().any(|w| w.message.contains("CONNECT")));
    }

    #[test]
    fn weld_break_replaces_flags() {
        let mut cat = break_rig();
        set_attr(&mut cat, RefU64(21), "LSFBR", NamedAttrValue::BoolType(true));
        set_attr(&mut cat, RefU64(22), "ASFBR", NamedAttrValue::BoolType(true));
        let svc = PipeModelService::new(cat);
        let batch = svc
            .set_spool_break(&ctx(), RefU64(21), RefU64(22), SpoolBreakMode::Weld)
            .unwrap();
        assert!(batch.requires_confirm.iter().any(|c| c.prompt.contains("Replace")));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::SetAttrs { attrs, .. }
                if matches!(attrs.map.values().next(), Some(NamedAttrValue::BoolType(false))))));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Create { noun, .. } if noun == "WELD")));
    }

    #[test]
    fn tee_cref_break_pairs_across_branches() {
        let mut cat = break_rig();
        // TEE(30) 在支管(12)，对方支管(13) 的元件(40)。
        cat.nouns.insert(RefU64(30), "TEE".into());
        cat.nouns.insert(RefU64(12), "BRAN".into());
        cat.nouns.insert(RefU64(13), "BRAN".into());
        cat.nouns.insert(RefU64(40), "TUBI".into());
        cat.owners.insert(RefU64(30), RefU64(12));
        cat.owners.insert(RefU64(40), RefU64(13));
        let svc = PipeModelService::new(cat);
        let batch = svc
            .set_spool_break(&ctx(), RefU64(30), RefU64(40), SpoolBreakMode::NoWeld)
            .unwrap();
        let keys: Vec<(RefU64, String)> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, attrs } => {
                    Some((*element, attrs.map.keys().next()?.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec![
            (RefU64(30), "CSFBR".to_string()),
            (RefU64(13), "TSFBR".to_string()),
        ]);
    }

    #[test]
    fn end_excess_writes_lexces_aexces() {
        let svc = PipeModelService::new(break_rig());
        let batch = svc
            .set_end_excess(&ctx(), RefU64(21), RefU64(23), 5.0, 8.0)
            .unwrap();
        let writes: Vec<(RefU64, &str, f32)> = batch
            .ops
            .iter()
            .filter_map(|op| match op {
                MutationOp::SetAttrs { element, attrs } => Some((
                    *element,
                    attrs.map.keys().next()?.as_str(),
                    match attrs.map.values().next() {
                        Some(NamedAttrValue::F32Type(v)) => *v,
                        _ => -1.0,
                    },
                )),
                _ => None,
            })
            .collect();
        assert_eq!(writes, vec![
            (RefU64(21), "LEXCES", 5.0),
            (RefU64(23), "AEXCES", 8.0),
        ]);
    }

    #[test]
    fn feed_excess_bend_only() {
        let mut cat = rig();
        cat.nouns.insert(RefU64(31), "BEND".into());
        cat.nouns.insert(RefU64(32), "ELBO".into());
        let svc = PipeModelService::new(cat);
        let batch = svc.set_feed_excess(&ctx(), RefU64(31), 120.0).unwrap();
        assert!(matches!(&batch.ops[0],
            MutationOp::SetAttrs { element, attrs }
                if *element == RefU64(31)
                && attrs.get("USEFED") == Some(&NamedAttrValue::F32Type(120.0))));
        assert!(svc.set_feed_excess(&ctx(), RefU64(32), 120.0).is_err());
    }

    #[test]
    fn pipe_end_desps_and_head_order() {
        let mut cat = break_rig();
        cat.names.insert(RefU64(500), "/THREADPIPEEND".into());
        cat.attrs.insert(RefU64(11), std::collections::BTreeMap::from([
            ("HSTU".into(), NamedAttrValue::RefU64Type(RefU64(700))),
        ]));
        let svc = PipeModelService::new(cat);
        let spec = PipeEndSpec {
            is_head: true,
            is_slanted: false,
            angle_deg: 0.0,
            thread_type: Some("BSP".into()),
            tube_od: 114.3,
            tube_len: 200.0,
            note: None,
        };
        let batch = svc.create_pipe_end(&ctx(), RefU64(11), RefU64(22), &spec).unwrap();
        let ftub = batch.ops.iter().find(|op| matches!(op,
            MutationOp::Create { noun, .. } if noun == "FTUB")).expect("FTUB");
        let MutationOp::Create { refno, attrs, .. } = ftub else { panic!() };
        assert_eq!(attrs.get("DESP"), Some(&NamedAttrValue::F32VecType(vec![0.0, 114.3, 1.0])));
        assert_eq!(attrs.get("SHOP"), Some(&NamedAttrValue::BoolType(true)));
        assert_eq!(attrs.get("MTOC"), Some(&NamedAttrValue::BoolType(false)));
        assert_eq!(attrs.get("LSTU"), Some(&NamedAttrValue::RefU64Type(RefU64(700))));
        assert_eq!(attrs.get("SPREF"), Some(&NamedAttrValue::RefU64Type(RefU64(500))));
        assert!(batch.ops.iter().any(|op| matches!(op,
            MutationOp::Include { element, order, .. }
                if element == refno && *order == Some(0))));
    }

    #[test]
    fn slanted_end_rejects_overlength() {
        let svc = PipeModelService::new(break_rig());
        // OD/tan(90-30) = 114.3/tan(60°) ≈ 66 < 200 → 通过；角度 89.9 → cutLen 巨大 → 拒。
        let bad = PipeEndSpec {
            is_head: false, is_slanted: true, angle_deg: 89.99,
            thread_type: None, tube_od: 114.3, tube_len: 200.0, note: None,
        };
        assert!(svc.create_pipe_end(&ctx(), RefU64(11), RefU64(22), &bad).is_err());
        let ok = PipeEndSpec { angle_deg: 30.0, ..bad };
        assert!(svc.create_pipe_end(&ctx(), RefU64(11), RefU64(22), &ok).is_ok());
    }
}
