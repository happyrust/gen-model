//! 编辑上下文（spec 决策 4）：等价 E3D 的 CE + 工作态。
//!
//! E3D 用两个全局态表达同样的东西——`!!ce`（当前元素）与 `Forwards/Backwards`
//! 遍历方向 MODE。本服务把它们显式建模为参数，不进全局态。

use aios_core::RefU64;

/// 遍历方向（E3D `Forwards/Backwards MODE`）。
///
/// 影响连接语义、成组插入顺序、元件默认朝向（componentcreation :5054-5059
/// 逆流时交换 arrive/leave）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flow {
    #[default]
    Forwards,
    Backwards,
}

/// 支管端操作面（E3D 的 head/tail 工作态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchEnd {
    Head,
    Tail,
}

/// 一次操作的调用上下文。
///
/// `element` = CE（当前元素）；`dbnum` = 目标库号；`flow`/`end` 为可缺省工作态。
/// 服务返回的结果对象里带新的 `element` 供调用方更新 CE。
#[derive(Debug, Clone)]
pub struct Context {
    pub element: RefU64,
    pub dbnum: u32,
    pub flow: Flow,
    pub end: Option<BranchEnd>,
}
