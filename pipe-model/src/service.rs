//! 服务面（spec §4）：`PipeModel` trait 按 E3D 对象边界分组。
//!
//! 所有方法同一契约：`(Context, 参数) -> Result<MutationBatch/Outcome, PipeError>`。
//! 骨架阶段各领域模块内 `impl PipeModelService` 的方法体统一返回
//! [`PipeError::unimplemented`]，纯函数（choice/naming/settings/spec_resolver）
//! 已实现并可测。

use crate::batch::MutationBatch;
use crate::context::{BranchEnd, Context};
use crate::error::PipeError;
use crate::outcome::{ComponentOutcome, ConnectOutcome, FabMessage, FabStatus};
use aios_core::RefU64;

/// 服务体：读面经 [`PipeCatalog`] 注入（生产接 SurrealDB 读面，测试用内存 mock），
/// 全局默认值经 [`PipingSettings`] 携带（等价 `!!pipingSettings`）。
///
/// 所依赖的规格库读面在 [`crate::spec_resolver::SpecCatalog`] 另立——
/// Phase C 元件组实现时注入。
pub struct PipeModelService<C> {
    pub catalog: C,
    /// 规格库读面（[`crate::spec_resolver::SpecCatalog`]，trait object）。
    /// `None` 时元件组操作返回 Blocking——与 Phase A 管编操作解耦。
    pub spec: Option<Box<dyn crate::spec_resolver::SpecCatalog>>,
    pub settings: crate::settings::PipingSettings,
}

impl<C: crate::catalog::PipeCatalog> PipeModelService<C> {
    pub fn new(catalog: C) -> Self {
        Self { catalog, spec: None, settings: crate::settings::PipingSettings::default() }
    }

    /// 接入规格库读面（Phase C 元件组的前置）。
    pub fn with_spec(mut self, spec: impl crate::spec_resolver::SpecCatalog + 'static) -> Self {
        self.spec = Some(Box::new(spec));
        self
    }
}

/// 管编域服务。
pub trait PipeModel {
    // ---- A 组：PIPE/BRAN 生命周期（Phase A）-------------------------------
    /// `NEW PIPE` 建管：ZONE/EQUI 属主校验 + 首支管自动建（§4.2）。
    fn create_pipe(&self, ctx: &Context, spec: &crate::pipeline::PipeSpec)
        -> Result<MutationBatch, PipeError>;

    /// 追加支管（自动名 `B{i}`，继承 PSPE 与 HBOR）。
    fn create_branch(&self, ctx: &Context, pipe: RefU64)
        -> Result<MutationBatch, PipeError>;

    /// 改名级联（prefixed children，§4.3）。
    fn rename_cascade(&self, ctx: &Context, element: RefU64, name: &str)
        -> Result<MutationBatch, PipeError>;

    /// 过程字段级联：保温/伴热/温度/压力只级联等于旧值的子元素（§4.4）。
    fn update_process_data(&self, ctx: &Context, pipe: RefU64,
        patch: &crate::pipeline::ProcessPatch) -> Result<MutationBatch, PipeError>;

    // ---- B 组：支管端编辑与连接（Phase A）--------------------------------
    /// 端编辑四序写：xBOR → xPOSITION → xDIRECTION → xCONNECTION → L*=TRUE（§6）。
    fn edit_branch_end(&self, ctx: &Context, branch: RefU64, end: BranchEnd,
        edit: &crate::pipeline::EndEdit) -> Result<MutationBatch, PipeError>;

    /// `CONN PH/PT TO ...`：返回 [`ConnectOutcome`] 区分四种终态（§7）。
    fn connect_end(&self, ctx: &Context, branch: RefU64, end: BranchEnd,
        target: RefU64) -> Result<(MutationBatch, ConnectOutcome), PipeError>;

    /// 游离端改派与兜底（FCONN+minTube 占位，可配长度，默认 100mm）。
    fn disconnect_end(&self, ctx: &Context, branch: RefU64, end: BranchEnd)
        -> Result<MutationBatch, PipeError>;

    // ---- C 组：元件（Phase B）--------------------------------------------
    /// `SELECT WITH <CHOICE>` 建件 + 成组插入（§18.6/§18.8）。
    fn create_component(&self, ctx: &Context, branch: RefU64,
        sel: &crate::component::ComponentSelect)
        -> Result<(MutationBatch, ComponentOutcome), PipeError>;

    /// `RESELECT WITH ...` 重选；SPRE 直写未命中自动回落重选（§18.6）。
    fn reselect_component(&self, ctx: &Context, component: RefU64,
        qa: &crate::choice::QuestionAnswers)
        -> Result<(MutationBatch, ComponentOutcome), PipeError>;

    /// 翻转三态：orientation/position/flip 控件改写 ANGL/ARIVE/LEAVE（§16.4）。
    fn flip_component(&self, ctx: &Context, component: RefU64)
        -> Result<MutationBatch, PipeError>;

    // ---- D 组：开孔/支管座（Phase C）--------------------------------------
    /// PTAP 开孔 + 支管座两步提交（§10）。
    fn create_tapping(&self, ctx: &Context, host_branch: RefU64,
        tap: &crate::tapping::TappingSpec) -> Result<MutationBatch, PipeError>;

    // ---- E 组：分合管（Phase C）------------------------------------------
    /// SEGMENT/FEATURE/PLANE 三分模式；MERGE 双支管合流（§11）。
    fn split_branch(&self, ctx: &Context, branch: RefU64,
        at: crate::splitmerge::SplitAt) -> Result<MutationBatch, PipeError>;
    fn merge_branches(&self, ctx: &Context, source: RefU64, target: RefU64)
        -> Result<MutationBatch, PipeError>;

    // ---- F 组：规格变更（Phase C）----------------------------------------
    /// PIPE/BRAN/元件三域 + spkbrk/TEE spare 回写（§12）。
    /// 逐件结果见 [`crate::specchange::SpecChangeResult`]（Story 32 三态着色面）。
    fn change_spec(&self, ctx: &Context, element: RefU64, new_spec: RefU64)
        -> Result<(MutationBatch, Vec<crate::specchange::SpecChangeResult>), PipeError>;

    // ---- G 组：斜管（Phase C）--------------------------------------------
    /// 逐 leg 旋转 + 锚点双箭语义（§13/Story 33-34）。
    /// 返回逐 leg 结果（可斜/不可斜原因），批内只含实际写入。
    fn apply_slope(&self, ctx: &Context, branch: RefU64, slope: f64)
        -> Result<(MutationBatch, Vec<crate::slope::LegResult>), PipeError>;

    // ---- H 组：焊接（Phase D）--------------------------------------------
    /// `NEW WELD CHOOSE SPEC ... SHOP T|F STYP`；降级路径显式入批（§14/Story 35-37）。
    /// 逐缝失败收集为行级 warning 不中断批（Story 36）。
    fn create_welds(&self, ctx: &Context, pipe: RefU64, policy: &crate::weld::WeldPolicy)
        -> Result<MutationBatch, PipeError>;

    /// 焊缝重编号（Story 38：`renumberAllWelds` 等价物——按支管成员序写 WLDNUM）。
    fn renumber_welds(&self, ctx: &Context, pipe: RefU64)
        -> Result<MutationBatch, PipeError>;

    // ---- I 组：制造（Phase D）---------------------------------------------
    /// 分段/管片生成接口；FAB_* 为一致性边界（stub 契约先行）。
    fn generate_spools(&self, ctx: &Context, pipe: RefU64)
        -> Result<MutationBatch, PipeError>;
    fn validate_fabrication(&self, ctx: &Context, pipe: RefU64)
        -> Result<(FabStatus, Vec<FabMessage>), PipeError>;

    // ---- L 组：制造断点/余量/管端（Story 51-56，§18.16）------------------
    /// 设置 spool 断点（双形态）：No-weld 写 `ASFBR/LSFBR/CSFBR/TSFBR` 标志对，
    /// Weld 建 `/WELDGAP`+STYPE FWS 现场焊（`DESP[1]=0.001`、`SHOP=false`）。
    /// 形态互遇/重复点击走 `requires_confirm` 替换/删除语义。
    fn set_spool_break(&self, ctx: &Context, part1: RefU64, part2: RefU64,
        mode: crate::fabrication::SpoolBreakMode) -> Result<MutationBatch, PipeError>;
    /// 删除断点：清标志对 + 删边界现场焊（confirm 语义随批回传）。
    fn remove_spool_break(&self, ctx: &Context, part1: RefU64, part2: RefU64)
        -> Result<MutationBatch, PipeError>;
    /// 端余量编辑：piece 边界对 `arrive_boundary.LEXCES` + `leave_boundary.AEXCES`。
    fn set_end_excess(&self, ctx: &Context, arrive_boundary: RefU64,
        leave_boundary: RefU64, arrive_excess: f32, leave_excess: f32)
        -> Result<MutationBatch, PipeError>;
    /// 进给余量编辑：`BEND.USEFED`。
    fn set_feed_excess(&self, ctx: &Context, bend: RefU64, feed: f32)
        -> Result<MutationBatch, PipeError>;
    /// FTUB 螺纹/斜切管端（`/THREADPIPEEND`/`/SLANTEDPIPEEND` 规格元件）。
    fn create_pipe_end(&self, ctx: &Context, branch: RefU64, tube: RefU64,
        spec: &crate::fabrication::PipeEndSpec) -> Result<MutationBatch, PipeError>;
    /// 焊件批量属性编辑（`pfModifyWelds` 语义）：WELDTY + SHOP 布尔成对写。
    fn modify_welds(&self, ctx: &Context, welds: &[RefU64],
        edit: &crate::weld::WeldAttrEdit) -> Result<MutationBatch, PipeError>;

    // ---- J 组：导入/设置（Phase D）----------------------------------------
    /// CSV 导入三段：validateHeadings → analyseRows → processData（§18.12）。
    /// 返回变更批 + 逐行报告（行级错误显式，不中断批）。
    fn import_rows(&self, ctx: &Context, rows: &crate::loader::ImportTable)
        -> Result<(MutationBatch, crate::loader::ImportReport), PipeError>;

    // ---- 接口空槽：ROUTER（契约已定，内核 Out of Scope，§18.11）------------
    /// `route(branch, viaPoints, config) → BSTAT`。本实现恒返回 Blocking——
    /// 由接入方替换为 ROUTER 内核绑定。
    fn route(&self, ctx: &Context, branch: RefU64,
        via: &[crate::pipeline::RoutePoint]) -> Result<MutationBatch, PipeError>;
}
