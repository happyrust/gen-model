//! 合并日志区（DASHBOARD-DESIGN 第 8 节）：按来源、级别筛，能搜索、能暂停滚动；点服务行就筛到那个服务。
use gpui_kit::component::button::{Button, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::events::LogLevel;
use crate::ui::format;
use crate::ui::tokens::{Tokens, tnum};

const SOURCES: [&str; 7] = ["全部", "数据库", "后端", "rs-server", "前端", "任务", "面板"];
const LEVELS: [&str; 3] = ["全部级别", "警告及以上", "错误"];

impl DashApp {
    pub(crate) fn render_logs(&self, t: &Tokens, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let view = self.log_view.clone();
        let (muted, warn, bad, ink, mono) = (t.muted, t.warn_text, t.bad_text, t.ink, t.mono.clone());
        let scroller = MessageScroller::new("log-list", self.log_state.clone(), move |ix, _window, _cx| {
            let line = view.borrow().get(ix).cloned();
            let row = h_flex().w_full().gap_3().px_3().h(px(20.)).items_center().text_xs().font_family(mono.clone());
            match line {
                Some(l) => {
                    let (lvl, color) = match l.level {
                        LogLevel::Error => ("ERROR", bad),
                        LogLevel::Warn => ("WARN", warn),
                        LogLevel::Info => ("", muted),
                    };
                    row.child(div().w(px(92.)).flex_shrink_0().text_color(muted).child(l.time))
                        .child(div().w(px(68.)).flex_shrink_0().whitespace_nowrap().truncate().text_color(muted).child(l.source.label()))
                        .child(div().w(px(40.)).flex_shrink_0().text_color(color).child(lvl))
                        .child(div().flex_1().min_w_0().whitespace_nowrap().truncate().text_color(ink).child(l.text))
                }
                None => row.child("…"),
            }
        })
        .with_row_style(StyleRefinement::default().pt(px(0.)).pb(px(0.)))
        .with_jump_button_label("回到最新")
        .size_full();

        let source = self.log_filter.source;
        let level_ix = match self.log_filter.min_level {
            None => 0,
            Some(LogLevel::Warn) | Some(LogLevel::Info) => 1,
            Some(LogLevel::Error) => 2,
        };
        let shown = self.log_view.borrow().len();
        let kept = self.logs.kept();

        let sources = ToggleGroup::new("log-source")
            .segmented()
            .single()
            .outline()
            .small()
            .children(SOURCES.iter().enumerate().map(|(k, label)| Toggle::new(("src", k)).label(*label).checked(source == k)))
            .on_click(cx.listener(|this, checks: &Vec<bool>, _, cx| {
                let src = checks.iter().position(|c| *c).unwrap_or(0);
                this.set_log_source(src, cx);
            }));
        let levels = ToggleGroup::new("log-level")
            .segmented()
            .single()
            .outline()
            .small()
            .children(LEVELS.iter().enumerate().map(|(k, label)| Toggle::new(("lvl", k)).label(if narrow && k == 1 { "警告+" } else { *label }).checked(level_ix == k)))
            .on_click(cx.listener(|this, checks: &Vec<bool>, _, cx| {
                let level = match checks.iter().position(|c| *c) {
                    Some(1) => Some(LogLevel::Warn),
                    Some(2) => Some(LogLevel::Error),
                    _ => None,
                };
                this.set_log_level(level, cx);
            }));

        // 底色用页面色：gpui-kit 分段按钮的选中填充正是 muted 那个灰，工具栏也用它的话选中段就看不出来
        let toolbar = h_flex()
            .h_10()
            .flex_shrink_0()
            .px_3()
            .gap_3()
            .items_center()
            .bg(t.page)
            .border_b_1()
            .border_color(t.line)
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("日志"))
            .child(sources)
            .child(levels)
            .child(
                div()
                    .id("level-hint")
                    .child(Icon::new(IconName::Info).size_3p5().text_color(t.muted))
                    .when(self.hover_ui(), |this| this.tooltip(|window, cx| Tooltip::new("级别按行内关键字判断（ERROR、WARN、错误、失败…）；各服务日志格式不同，只作参考").build(window, cx))),
            )
            .child(div().w(px(if narrow { 160. } else { 220. })).child(Input::new(&self.search).small().prefix(Icon::new(IconName::Search).size_3p5()).cleanable(true)))
            .child(
                Toggle::new("log-pause")
                    .outline()
                    .small()
                    .icon(IconName::Pause)
                    .label("暂停")
                    .checked(self.log_paused)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_log_paused(*checked, cx))),
            )
            .when(self.log_paused && self.log_pending > 0, |this| {
                this.child(div().text_xs().text_color(t.warn_text).font_features(tnum()).child(format!("新增 {} 行", format::thousands(self.log_pending))))
                    .child(Button::new("log-resume").ghost().xsmall().label("回到最新").on_click(cx.listener(|this, _, _, cx| this.set_log_paused(false, cx))))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(t.muted)
                    .font_features(tnum())
                    .whitespace_nowrap()
                    .child(format!("显示 {} / 共 {} 行", format::thousands(shown as u64), format::thousands(kept as u64))),
            );

        v_flex()
            .size_full()
            .border_1()
            .border_color(t.line)
            .rounded(t.radius_lg)
            .overflow_hidden()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(scroller))
            .into_any_element()
    }
}
