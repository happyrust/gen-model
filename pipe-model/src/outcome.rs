//! 结果对象（spec 决策 7）：E3D 的静默降级路径在此显式化。

use aios_core::RefU64;

/// 警告（E3D `alert.warning`）。
#[derive(Debug, Clone)]
pub struct PipeWarning {
    /// 警告文本。
    pub message: String,
    /// 已完成的中间状态描述（如 connectOnlyPosDir 的"端已移位未连接"）；
    /// 空 = 无副作用的纯提示。
    pub side_effect: Option<String>,
}

/// 支管端连接结果——区分 E3D 内部四种结果码。
#[derive(Debug, Clone)]
pub enum ConnectOutcome {
    /// 正常连接（HCON/TCON 等已按 CONNECT 语义写入）。
    Connected {
        /// 连接后的实际 CON 类型（如 TEE/XCONN）。
        con_type: String,
        /// 连接对端。
        peer: RefU64,
    },
    /// `connectOnlyPosDir`：只搬了位置和方向，未连接——E3D 里要靠 alert 提示，
    /// 这里显式返回（伴随 [`PipeWarning::side_effect`]）。
    PositionedOnly,
    /// 几何对齐兜底成功（E3D handle(42,94) 路径）；原 `_toRefno` 丢失。
    FallbackAligned,
    /// 兜底也失败，端停留在游离态，等待用户改派（E3D 的 FCONN+minTube 占位）。
    Unassigned { placeholder_length_mm: f64 },
}

/// 元件创建/重选结果——规格回落链的最终落点必须显式可见。
#[derive(Debug, Clone)]
pub enum ComponentOutcome {
    /// 直接命中 SPCO。
    Direct { spco: RefU64 },
    /// 经 SELE 默认路径回落命中（default/tdefault/EQUA 链）。
    SelectorFallback { spco: RefU64, via: String },
    /// 全部落空，需要 UI 打开 Select Specification 让用户手动答卷。
    NeedsSelection,
}

/// 制造检查结果（pfFabricationCheck 三级：Valid / ValidWithWarning / Failed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FabStatus {
    Valid,
    ValidWithWarning,
    Failed,
}

/// 检查行级消息（信息/警告/错误三级，§8.4）。
#[derive(Debug, Clone)]
pub struct FabMessage {
    pub status: FabStatus,
    /// 出问题的管片/元件 refno（可空）。
    pub element: Option<RefU64>,
    pub text: String,
}
