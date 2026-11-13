//! 令牌层（DASHBOARD-DESIGN 11.1）。调用处只用这里的名字；色值只出现在这个文件里
//! （gpui-kit 设计指南：缺的语义角色在应用的令牌层定义，不在调用处写色值）。
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, FontFeatures, Hsla, Pixels, SharedString, rgb};

use crate::state::{RowKind, Sev};

#[derive(Clone)]
pub struct Tokens {
    pub dark: bool,
    pub page: Hsla,
    pub soft: Hsla,
    pub line: Hsla,
    pub ink: Hsla,
    pub ink2: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub ok: Hsla,
    pub warn_fill: Hsla,
    pub warn_text: Hsla,
    pub bad: Hsla,
    pub bad_text: Hsla,
    pub selection_fill: Hsla,
    pub selection_ring: Hsla,
    pub spark: Hsla,
    pub radius: Pixels,
    pub radius_lg: Pixels,
    pub mono: SharedString,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Tokens {
    pub fn new(cx: &App) -> Self {
        let th = cx.theme();
        let dark = th.is_dark();
        Self {
            dark,
            page: th.background,
            soft: th.muted,
            line: th.border,
            ink: th.foreground,
            ink2: if dark { c(0xd4d4d4) } else { c(0x525252) },
            muted: th.muted_foreground,
            accent: th.chart_2,
            ok: th.success,
            warn_fill: th.warning,
            warn_text: if dark { c(0xfacc15) } else { c(0xa16207) },
            bad: th.danger,
            bad_text: if dark { c(0xf87171) } else { c(0xdc2626) },
            selection_fill: th.chart_2.opacity(if dark { 0.14 } else { 0.07 }),
            selection_ring: th.chart_2.opacity(if dark { 0.45 } else { 0.30 }),
            spark: th.muted_foreground.opacity(0.75),
            radius: th.radius,
            radius_lg: th.radius_lg,
            mono: th.mono_font_family.clone(),
        }
    }

    pub fn sev_text(&self, sev: Sev) -> Hsla {
        match sev {
            Sev::Red => self.bad_text,
            Sev::Amber => self.warn_text,
        }
    }

    /// 刻度的颜色：健康最淡，问题最显眼（skill：强度朝问题增长）。
    pub fn tick(&self, k: RowKind) -> Hsla {
        match k {
            RowKind::Ok => self.ok.opacity(if self.dark { 0.45 } else { 0.38 }),
            RowKind::Starting | RowKind::Stopping | RowKind::Queued | RowKind::Waiting => self.muted.opacity(0.45),
            RowKind::WaitingLong | RowKind::Degraded => self.warn_fill,
            RowKind::Bad => self.bad,
            RowKind::Stopped | RowKind::Disabled | RowKind::Unconfigured => self.line,
        }
    }
}

/// 等宽数字（gpui-kit `time_field.rs` 同样写法）。
pub fn tnum() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}
