//! PIPINGSETTINGS 默认值（功能文档 §18.4 `loadDefaultValues` + §18.13 控件映射表）。
//!
//! E3D 把这些值存成字符串表达式（`'2*OD'`/`'T'`），运行时由 PML 求值。
//! 本 crate 保持同样外形；求值在调用方或接入表达式引擎后做。

/// 管编全局设置。字段名与 `!!pipingSettings` 成员一一对应。
#[derive(Debug, Clone)]
pub struct PipingSettings {
    /// 元件吸附距离表达式（默认 `'2*OD'`）。
    pub snap_dist: String,
    /// 自动对齐。
    pub auto_align: bool,
    /// 自动建元件。
    pub auto_create: bool,
    /// 元件打 tag。
    pub tag_component: bool,
    /// 自动重连。
    pub auto_reconnect: bool,
    /// 最短管段 mm（CTUBE/minTube 下限）。
    pub min_tube_mm: f64,
    /// 等轴测象限 1-4（setIsoMode）。
    pub iso_mode: u8,
    /// 激光圆整容差（度）。
    pub round_laser_tolerance_deg: f64,
    /// 新建支管自动带头尾元件（createHead/TailComponents）。
    pub create_end_components: bool,
}

impl Default for PipingSettings {
    /// 出厂默认（§18.4）。
    fn default() -> Self {
        Self {
            snap_dist: "2*OD".into(),
            auto_align: true,
            auto_create: true,
            tag_component: true,
            auto_reconnect: true,
            min_tube_mm: 100.0,
            iso_mode: 3,
            round_laser_tolerance_deg: 1.0,
            create_end_components: true,
        }
    }
}
