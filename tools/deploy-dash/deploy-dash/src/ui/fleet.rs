//! 多服务器界面（REMOTE-ACCESS-DESIGN 第 5 节）：左侧服务器列表（展开 / 图标栏）、顶部跨机汇总条、
//! 服务器抽屉（表 + 表单、测试连接、删除确认）、连不上的横幅、第一次连接时的骨架。
//! 都是组件函数：输入是 `fleet_state` 算好的视图数据，点击统一回调 [`FleetAction`]；状态归面板（`app.rs`），这里不存东西。
//! 每个能点的控件都有读得出的名字（AccessKit），agent-desktop 按名字点：「服务器 <名> 切到」「服务器 添加」……
#![allow(dead_code)]

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::client_store::DEFAULT_PORT;
use crate::fleet_state::{
    Banner, BarView, DrawerRow, DrawerView, Field, FormInput, FormMode, Health, LIST_W, Level, ListMode, RAIL_W, RowView, ServerId,
    TestState, Tone, probe_text,
};
use crate::ui::format;
use crate::ui::kpi::TILE_H;
use crate::ui::services::{HEAD_H, ROW_H};
use crate::ui::tokens::{Tokens, tnum};

gpui_kit::actions!(
    fleet,
    [ToggleServerList, PrevServer, NextServer, Server1, Server2, Server3, Server4, Server5, Server6, Server7, Server8, Server9]
);

/// 服务器列表的键盘上下文：↑ ↓ 只在列表拿着焦点时生效，不和输入框、日志抢键。
pub const LIST_CONTEXT: &str = "FleetList";
/// 选中的那台连不上时，主区域旧数据的透明度（设计 3.4：变暗 40%）。
pub const DIM_OPACITY: f32 = 0.4;
/// 顶部汇总条的高度。
pub const BAR_H: f32 = 32.0;

const SHORTCUT_KEYS: [&str; 9] = ["ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8", "ctrl-9"];
const TOKEN_HINT_NEW: &str = "在服务器上运行 deploy-dash --agent show-token 抄下来";
const TOKEN_HINT_SAVED: &str = "已保存，重新输入可替换";

/// `Ctrl+B` 折叠列表、`Ctrl+1…9` 直达、列表里 ↑ ↓ 切换。面板起来时 `cx.bind_keys(fleet::key_bindings())`。
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-b", ToggleServerList, None),
        KeyBinding::new("up", PrevServer, Some(LIST_CONTEXT)),
        KeyBinding::new("down", NextServer, Some(LIST_CONTEXT)),
        KeyBinding::new("ctrl-1", Server1, None),
        KeyBinding::new("ctrl-2", Server2, None),
        KeyBinding::new("ctrl-3", Server3, None),
        KeyBinding::new("ctrl-4", Server4, None),
        KeyBinding::new("ctrl-5", Server5, None),
        KeyBinding::new("ctrl-6", Server6, None),
        KeyBinding::new("ctrl-7", Server7, None),
        KeyBinding::new("ctrl-8", Server8, None),
        KeyBinding::new("ctrl-9", Server9, None),
    ]
}

/// 界面上的动作，面板（`app.rs`）收到后改状态、发命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetAction {
    Select(ServerId),
    ToggleList,
    /// 打开服务器抽屉。
    OpenServers,
    /// 抽屉里开始添加（抽屉没开时先打开）。
    Add,
    /// 开始编辑（横幅上的「编辑服务器…」也是它；抽屉没开时先打开）。
    Edit(ServerId),
    /// 确认框里点了「删除」。
    Delete(ServerId),
    /// 上移（-1）/ 下移（+1）。
    Move(ServerId, i32),
    Test,
    Save,
    CancelEdit,
    Retry(ServerId),
    Relaunch(ServerId),
}

pub type OnFleet = Rc<dyn Fn(FleetAction, &mut Window, &mut App)>;

fn click(on: &OnFleet, action: FleetAction) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let on = on.clone();
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| on(action.clone(), window, cx)
}

/// 状态图标：● 正常、▲ 琥珀、✕ 红 / 连不上、■ 已停止、◌ 未配置、转圈 = 连接中 / 起停中（DASHBOARD-DESIGN 11.2 同一套）。
pub fn health_mark(t: &Tokens, h: Health) -> AnyElement {
    match h {
        Health::Ok => div().size_2().rounded_full().bg(t.ok).into_any_element(),
        Health::Warn => Icon::new(IconName::TriangleAlert).size_3p5().text_color(t.warn_text).into_any_element(),
        Health::Bad | Health::Down => Icon::new(IconName::CircleX).size_3p5().text_color(t.bad_text).into_any_element(),
        Health::Stopped => Icon::new(IconName::Square).size_3().text_color(t.muted).into_any_element(),
        Health::Unconfigured => Icon::new(IconName::CircleAlert).size_3p5().text_color(t.muted).into_any_element(),
        Health::Connecting | Health::Starting | Health::Stopping => Spinner::new().xsmall().color(t.muted).into_any_element(),
    }
}

fn word_color(t: &Tokens, h: Health) -> Hsla {
    match h {
        Health::Bad | Health::Down => t.bad_text,
        Health::Warn => t.warn_text,
        Health::Ok => t.ink2,
        _ => t.muted,
    }
}

fn shortcut_label(n: u8) -> Option<Kbd> {
    let key: &'static str = SHORTCUT_KEYS.get(usize::from(n).checked_sub(1)?).copied()?;
    Keystroke::parse(key).ok().map(|k| Kbd::new(k).appearance(false))
}

// ───────────────────────── 左侧列表 ─────────────────────────

/// 左侧服务器列表。`ListMode::Hidden`（只有一台）时是空的。`focus` 给了就挂上，↑ ↓ 在列表拿着焦点时切换。
pub fn server_list(t: &Tokens, rows: &[RowView], mode: ListMode, hover: bool, focus: Option<&FocusHandle>, on: &OnFleet) -> AnyElement {
    match mode {
        ListMode::Hidden => div().into_any_element(),
        ListMode::Full => full_list(t, rows, hover, focus, on),
        ListMode::Rail => rail(t, rows, hover, focus, on),
    }
}

fn toggle_button(on: &OnFleet, collapse: bool, hover: bool) -> Button {
    Button::new(if collapse { "fleet-collapse" } else { "fleet-expand" })
        .ghost()
        .xsmall()
        .icon(if collapse { IconName::PanelLeftClose } else { IconName::PanelLeftOpen })
        .accessibility_label(if collapse { "服务器列表 收起" } else { "服务器列表 展开" })
        .when(hover, |b| b.tooltip(if collapse { "收起列表（Ctrl+B）" } else { "展开列表（Ctrl+B）" }))
        .on_click(click(on, FleetAction::ToggleList))
}

fn pane(t: &Tokens, id: &'static str, width: f32, focus: Option<&FocusHandle>) -> Stateful<Div> {
    v_flex()
        .id(id)
        .key_context(LIST_CONTEXT)
        .when_some(focus, |this, f| this.track_focus(f))
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .bg(t.page)
        .border_r_1()
        .border_color(t.line)
}

fn full_list(t: &Tokens, rows: &[RowView], hover: bool, focus: Option<&FocusHandle>, on: &OnFleet) -> AnyElement {
    let header = h_flex()
        .h(px(36.))
        .flex_shrink_0()
        .pl_3()
        .pr_1()
        .items_center()
        .justify_between()
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.muted).child("服务器"))
        .child(toggle_button(on, true, hover));
    let list = v_flex().id("fleet-rows").flex_1().min_h_0().overflow_y_scroll().px_2().gap_0p5().children(rows.iter().map(|r| full_row(t, r, hover, on)));
    let add = Button::new("fleet-add")
        .ghost()
        .small()
        .icon(IconName::Plus)
        .label("添加服务器…")
        .accessibility_label("服务器 添加")
        .on_click(click(on, FleetAction::Add));
    pane(t, "fleet-list", LIST_W, focus)
        .child(header)
        .child(list)
        .child(div().flex_shrink_0().px_2().py_2().border_t_1().border_color(t.line).child(add))
        .into_any_element()
}

fn full_row(t: &Tokens, r: &RowView, hover: bool, on: &OnFleet) -> AnyElement {
    let tip = r.tip.clone();
    v_flex()
        .id(("fleet-row", r.id as usize))
        .role(Role::Button)
        .aria_label(format!("服务器 {} 切到", r.name))
        .aria_selected(r.selected)
        .when_some(r.shortcut, |this, n| this.aria_keyshortcuts(format!("Ctrl+{n}")))
        .w_full()
        .px_2()
        .py_1p5()
        .gap_0p5()
        .rounded(t.radius)
        .border_1()
        .border_color(if r.selected { t.selection_ring } else { transparent_black() })
        .when(r.selected, |this| this.bg(t.selection_fill))
        .when(!r.selected && hover, |this| this.hover(|s| s.bg(t.soft.opacity(0.6))))
        .cursor_pointer()
        .on_click(click(on, FleetAction::Select(r.id)))
        .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .min_w_0()
                .child(div().w_4().flex_shrink_0().flex().items_center().justify_center().child(health_mark(t, r.health)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .font_weight(if r.selected { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                        .truncate()
                        .child(r.name.clone()),
                )
                .child(div().flex_shrink_0().text_xs().font_features(tnum()).text_color(word_color(t, r.health)).child(r.word.clone())),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .pl_6()
                .child(div().flex_1().min_w_0().text_xs().text_color(t.muted).truncate().child(r.address.clone()))
                .children(r.shortcut.and_then(shortcut_label).map(|k| div().flex_shrink_0().text_xs().text_color(t.muted).child(k))),
        )
        .into_any_element()
}

fn rail(t: &Tokens, rows: &[RowView], hover: bool, focus: Option<&FocusHandle>, on: &OnFleet) -> AnyElement {
    let items = v_flex()
        .id("fleet-rail-items")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .items_center()
        .gap_1()
        .py_1()
        .children(rows.iter().map(|r| rail_item(t, r, hover, on)));
    let add = Button::new("fleet-add")
        .ghost()
        .small()
        .icon(IconName::Plus)
        .accessibility_label("服务器 添加")
        .when(hover, |b| b.tooltip("添加服务器…"))
        .on_click(click(on, FleetAction::Add));
    pane(t, "fleet-rail", RAIL_W, focus)
        .child(h_flex().h(px(36.)).flex_shrink_0().justify_center().items_center().child(toggle_button(on, false, hover)))
        .child(items)
        .child(h_flex().flex_shrink_0().py_2().justify_center().border_t_1().border_color(t.line).child(add))
        .into_any_element()
}

fn rail_item(t: &Tokens, r: &RowView, hover: bool, on: &OnFleet) -> AnyElement {
    let tip = r.tip.clone();
    div()
        .id(("fleet-rail-item", r.id as usize))
        .role(Role::Button)
        .aria_label(format!("服务器 {} 切到", r.name))
        .aria_selected(r.selected)
        .when_some(r.shortcut, |this, n| this.aria_keyshortcuts(format!("Ctrl+{n}")))
        .relative()
        .size_10()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius)
        .border_1()
        .border_color(if r.selected { t.selection_ring } else { transparent_black() })
        .when(r.selected, |this| this.bg(t.selection_fill))
        .when(!r.selected && hover, |this| this.hover(|s| s.bg(t.soft.opacity(0.6))))
        .cursor_pointer()
        .on_click(click(on, FleetAction::Select(r.id)))
        .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(if r.selected { t.ink } else { t.ink2 }).child(r.initial.clone()))
        .child(div().absolute().right(px(2.)).bottom(px(2.)).child(health_mark(t, r.health)))
        .into_any_element()
}

// ───────────────────────── 顶部汇总条 ─────────────────────────

/// 顶部跨机汇总条（A 和 B 之间，高 32）：左边各类几台；中间点名最要紧的问题和「切到」；右边「服务器…」。
/// 都正常时中间空着，整条安静。
pub fn summary_bar(t: &Tokens, bar: &BarView, hover: bool, on: &OnFleet) -> AnyElement {
    let mut left = h_flex().gap_2().items_center().flex_shrink_0();
    for (k, (tone, text)) in bar.parts.iter().enumerate() {
        if k > 0 {
            left = left.child(div().text_xs().text_color(t.muted).child("·"));
        }
        let (icon, color) = match tone {
            Tone::Quiet => (None, t.ink2),
            Tone::Amber => (Some(IconName::TriangleAlert), t.warn_text),
            Tone::Red => (Some(IconName::CircleX), t.bad_text),
        };
        left = left.child(
            h_flex()
                .gap_1()
                .items_center()
                .text_sm()
                .text_color(color)
                .font_features(tnum())
                .whitespace_nowrap()
                .when_some(icon, |this, i| this.child(Icon::new(i).size_3p5()))
                .child(text.clone()),
        );
    }
    let middle = bar.worst.as_ref().map(|w| {
        let color = if w.level == Level::Red { t.bad_text } else { t.warn_text };
        h_flex()
            .gap_2()
            .items_center()
            .min_w_0()
            .child(div().flex_shrink_0().text_xs().text_color(t.muted).child("最要紧"))
            .child(div().flex_shrink_0().text_sm().font_weight(FontWeight::MEDIUM).child(w.name.clone()))
            .child(div().min_w_0().text_sm().text_color(color).truncate().child(w.text.clone()))
            .when(w.others > 0, |this| this.child(div().flex_shrink_0().text_xs().text_color(t.muted).whitespace_nowrap().child(format!("另 {} 台也有问题", w.others))))
            .when(!bar.worst_is_selected, |this| {
                this.child(
                    Button::new("fleet-switch-worst")
                        .outline()
                        .xsmall()
                        .label(format!("切到 {}", w.name))
                        .accessibility_label(format!("汇总 切到 {}", w.name))
                        .on_click(click(on, FleetAction::Select(w.id))),
                )
            })
    });
    h_flex()
        .id("fleet-summary")
        .w_full()
        .h(px(BAR_H))
        .flex_shrink_0()
        .px_4()
        .gap_4()
        .items_center()
        .border_b_1()
        .border_color(t.line)
        .child(left)
        .child(div().flex_1().min_w_0().overflow_hidden().children(middle))
        .child(
            Button::new("fleet-open-servers")
                .ghost()
                .xsmall()
                .icon(IconName::Settings2)
                .label("服务器…")
                .accessibility_label("服务器 管理")
                .when(hover, |b| b.tooltip("添加、编辑、删除服务器"))
                .on_click(click(on, FleetAction::OpenServers)),
        )
        .into_any_element()
}

// ───────────────────────── 连不上的样子 ─────────────────────────

/// 选中的那台连不上时的页级横幅（全屏只有这一处）。样子同 `header.rs` 的「拿不到状态」横幅。
pub fn unreachable_banner(t: &Tokens, id: ServerId, banner: &Banner, on: &OnFleet) -> AnyElement {
    let retry_note = match banner.retry_in {
        Some(d) if d >= Duration::from_secs(1) => Some(format!("{} 后自动重试", format::secs1(d))),
        Some(_) => None,
        None => Some("已停止自动重试".to_string()),
    };
    h_flex()
        .w_full()
        .px_4()
        .py_2()
        .gap_3()
        .items_center()
        .bg(t.bad.opacity(0.10))
        .border_b_1()
        .border_color(t.bad.opacity(0.35))
        .child(Icon::new(IconName::CircleX).size_4().text_color(t.bad_text))
        .child(div().flex_1().min_w_0().text_sm().child(banner.text.clone()))
        .children(retry_note.map(|n| div().flex_shrink_0().text_xs().text_color(t.muted).font_features(tnum()).child(n)))
        .child(Button::new("fleet-banner-retry").small().outline().label("重试").accessibility_label("连不上 重试").on_click(click(on, FleetAction::Retry(id))))
        .when(banner.edit, |this| {
            this.child(Button::new("fleet-banner-edit").small().ghost().label("编辑服务器…").accessibility_label("连不上 编辑服务器").on_click(click(on, FleetAction::Edit(id))))
        })
        .when(banner.relaunch, |this| {
            this.child(Button::new("fleet-banner-relaunch").small().ghost().label("重新拉起代理").accessibility_label("连不上 重新拉起代理").on_click(click(on, FleetAction::Relaunch(id))))
        })
        .into_any_element()
}

/// 新加的服务器第一次连接时主区域的骨架：大小和磁贴、服务表一样，连上以后原地换成真数据。
pub fn first_connect_skeleton(t: &Tokens) -> AnyElement {
    let tile = |k: usize| {
        v_flex()
            .id(("fleet-skel-tile", k))
            .flex_1()
            .min_w_0()
            .h(px(TILE_H))
            .px_4()
            .py_3()
            .gap_3()
            .border_1()
            .border_color(t.line)
            .rounded(t.radius_lg)
            .child(Skeleton::new().w(px(96.)).h_3().rounded(t.radius))
            .child(Skeleton::new().w(px(140.)).h_8().rounded(t.radius))
            .child(Skeleton::new().w_full().h_5().rounded(t.radius))
            .child(Skeleton::new().w(px(120.)).h_3().rounded(t.radius))
    };
    let row = |k: usize| {
        h_flex()
            .id(("fleet-skel-row", k))
            .h(px(ROW_H))
            .px_3()
            .gap_3()
            .items_center()
            .when(k > 0, |this| this.border_t_1().border_color(t.line))
            .child(Skeleton::new().size_7().rounded(t.radius))
            .child(Skeleton::new().w(px(96.)).h_4().rounded(t.radius))
            .child(Skeleton::new().w(px(140.)).h_4().rounded(t.radius))
            .child(Skeleton::new().flex_1().h_4().rounded(t.radius))
    };
    v_flex()
        .w_full()
        .gap_3()
        .child(h_flex().w_full().gap_3().children((0..4).map(tile)))
        .child(
            v_flex()
                .w_full()
                .border_1()
                .border_color(t.line)
                .rounded(t.radius_lg)
                .overflow_hidden()
                .child(div().h(px(HEAD_H)).bg(t.soft).border_b_1().border_color(t.line))
                .children((0..4).map(row)),
        )
        .into_any_element()
}

// ───────────────────────── 服务器抽屉 ─────────────────────────

/// 抽屉里的表单：输入框状态建在面板里，抽屉每次重建时还在（同 `ConfigForm`）。
/// 令牌框是密码框（显示成圆点），从不填进已保存的令牌，也没有「显示明文」的按钮。
pub struct ServerForm {
    pub name: Entity<InputState>,
    pub address: Entity<InputState>,
    pub port: Entity<InputState>,
    pub token: Entity<InputState>,
    pub note: Entity<InputState>,
}

impl ServerForm {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let input = |window: &mut Window, cx: &mut App, ph: &str| {
            let ph = ph.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        Self {
            name: input(window, cx, "例如 二楼机房"),
            address: input(window, cx, "主机名或 IP，可以带 :端口"),
            port: input(window, cx, &DEFAULT_PORT.to_string()),
            token: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(TOKEN_HINT_NEW)),
            note: input(window, cx, "可空"),
        }
    }

    /// 读出表单（令牌原样交给 `fleet_state::validate_form`，不进日志）。
    pub fn read(&self, cx: &App) -> FormInput {
        let v = |s: &Entity<InputState>| s.read(cx).value().to_string();
        FormInput { name: v(&self.name), address: v(&self.address), port: v(&self.port), token: v(&self.token), note: v(&self.note) }
    }

    /// 开始添加（`None`）或编辑：填名称、地址、端口、备注；令牌框永远清空，只换提示。
    pub fn load(&self, entry: Option<(&str, &str, u16, &str, bool)>, window: &mut Window, cx: &mut App) {
        let (name, address, port, note, has_token) = entry.unwrap_or(("", "", DEFAULT_PORT, "", false));
        let port = if port == DEFAULT_PORT { String::new() } else { port.to_string() };
        for (state, value) in [(&self.name, name.to_string()), (&self.address, address.to_string()), (&self.port, port), (&self.note, note.to_string())] {
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        self.token.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_placeholder(if has_token { TOKEN_HINT_SAVED } else { TOKEN_HINT_NEW }, window, cx);
        });
    }

    /// 保存或取消之后清掉令牌框：明文不在界面状态里多留。
    pub fn clear_token(&self, window: &mut Window, cx: &mut App) {
        self.token.update(cx, |s, cx| s.set_value("", window, cx));
    }
}

/// 删除确认（`open_alert_dialog`）。只是不再显示，服务器上什么都不变。
pub fn confirm_delete(window: &mut Window, cx: &mut App, id: ServerId, name: String, on: OnFleet) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on = on.clone();
        alert
            .title(format!("从列表里删掉 {name}？"))
            .description("只是不再显示它，服务器上什么都不会变。")
            .show_cancel(true)
            .ok_text("删除")
            .ok_variant(ButtonVariant::Danger)
            .cancel_text("取消")
            .on_ok(move |_, window, cx| {
                on(FleetAction::Delete(id), window, cx);
                true
            })
    });
}

fn last_ok_text(r: &DrawerRow) -> String {
    match r.last_ok_at {
        Some(t) if SystemTime::now().duration_since(t).is_ok_and(|d| d < Duration::from_secs(20 * 3600)) => format::clock(t),
        Some(t) => format::date_clock(t),
        None if r.health == Health::Connecting => format::NONE.to_string(),
        None => "从没连上".to_string(),
    }
}

fn server_table(t: &Tokens, rows: &[DrawerRow], editing: Option<ServerId>, hover: bool, on: &OnFleet) -> AnyElement {
    let head = TableHeader::new().child(
        TableRow::new()
            .child(TableHead::new().w(px(170.)).min_w(px(0.)).child("服务器"))
            .child(TableHead::new().w(px(96.)).min_w(px(0.)).child("状态"))
            .child(TableHead::new().w(px(76.)).min_w(px(0.)).child("最近连上"))
            .child(TableHead::new().flex_1().min_w(px(0.)).child("")),
    );
    let mut body = TableBody::new();
    for r in rows {
        let i = r.id as usize;
        let name = r.name.clone();
        let ops = h_flex()
            .w_full()
            .gap_0p5()
            .justify_end()
            .items_center()
            .when(!r.local, |this| {
                this.child(
                    Button::new(("fleet-up", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ArrowUp)
                        .accessibility_label(format!("服务器 {name} 上移"))
                        .disabled(!r.can_up)
                        .when(hover, |b| b.tooltip("上移"))
                        .on_click(click(on, FleetAction::Move(r.id, -1))),
                )
                .child(
                    Button::new(("fleet-down", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ArrowDown)
                        .accessibility_label(format!("服务器 {name} 下移"))
                        .disabled(!r.can_down)
                        .when(hover, |b| b.tooltip("下移"))
                        .on_click(click(on, FleetAction::Move(r.id, 1))),
                )
                .child(
                    Button::new(("fleet-edit", i))
                        .ghost()
                        .xsmall()
                        .label("编辑")
                        .accessibility_label(format!("服务器 {name} 编辑"))
                        .disabled(editing == Some(r.id))
                        .on_click(click(on, FleetAction::Edit(r.id))),
                )
                .child({
                    let on = on.clone();
                    let (id, name) = (r.id, r.name.clone());
                    Button::new(("fleet-delete", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .accessibility_label(format!("服务器 {name} 删除"))
                        .when(hover, |b| b.tooltip("删除…"))
                        .on_click(move |_, window, cx| confirm_delete(window, cx, id, name.clone(), on.clone()))
                })
            })
            .when(r.local, |this| this.child(div().text_xs().text_color(t.muted).child("这台电脑，自动列出")));
        body = body.child(
            TableRow::new()
                .id(("fleet-table-row", i))
                .when(editing == Some(r.id), |this| this.bg(t.selection_fill))
                .child(
                    TableCell::new().w(px(170.)).min_w(px(0.)).child(
                        v_flex()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .min_w_0()
                                    .child(div().min_w_0().text_sm().font_weight(FontWeight::MEDIUM).truncate().child(r.name.clone()))
                                    .when(r.local, |this| this.child(Tag::secondary().small().outline().child("自动"))),
                            )
                            .child(div().text_xs().text_color(t.muted).truncate().child(r.endpoint.clone())),
                    ),
                )
                .child(
                    TableCell::new().w(px(96.)).min_w(px(0.)).child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(health_mark(t, r.health))
                            .child(div().text_xs().text_color(word_color(t, r.health)).font_features(tnum()).whitespace_nowrap().child(r.word.clone())),
                    ),
                )
                .child(TableCell::new().w(px(76.)).min_w(px(0.)).child(div().text_xs().text_color(t.muted).font_features(tnum()).child(last_ok_text(r))))
                .child(TableCell::new().flex_1().min_w(px(0.)).child(ops)),
        );
    }
    Table::new()
        .small()
        .accessibility_label("服务器列表")
        .border_1()
        .border_color(t.line)
        .rounded(t.radius_lg)
        .child(head)
        .child(body)
        .into_any_element()
}

fn form_field(t: &Tokens, view: &DrawerView, which: Field, label: &'static str, required: bool, hint: Option<String>, control: impl IntoElement) -> gpui_kit::component::form::Field {
    let error = view.errors.iter().find(|(f, _)| *f == which).map(|(_, m)| m.clone());
    let bad = t.bad_text;
    let f = field().label(label).required(required).child(control);
    match (error, hint) {
        (Some(msg), _) => f.description_fn(move |_, _| {
            h_flex().gap_1().items_center().text_color(bad).child(Icon::new(IconName::CircleX).size_3()).child(msg.clone())
        }),
        (None, Some(h)) => f.description(h),
        (None, None) => f,
    }
}

fn form_block(t: &Tokens, view: &DrawerView, form: &ServerForm, on: &OnFleet) -> AnyElement {
    let has_token = matches!(view.mode, FormMode::Editing { has_token: true, .. });
    let title = if matches!(view.mode, FormMode::Adding) { "添加服务器" } else { "编辑服务器" };
    let token_hint = if has_token { "已保存的令牌从不显示；留空就不改，重新输入就替换" } else { TOKEN_HINT_NEW };
    let fields = v_form()
        .child(form_field(t, view, Field::Name, "名称", true, None, Input::new(&form.name).aria_label("服务器 名称")))
        .child(form_field(t, view, Field::Address, "地址", true, Some("主机名或 IP；第一版走 http".into()), Input::new(&form.address).aria_label("服务器 地址")))
        .child(form_field(
            t,
            view,
            Field::Port,
            "端口",
            false,
            Some(format!("缺省 {DEFAULT_PORT}")),
            div().w(px(140.)).child(Input::new(&form.port).aria_label("服务器 端口")),
        ))
        .child(form_field(
            t,
            view,
            Field::Token,
            "令牌",
            !has_token,
            Some(token_hint.into()),
            Input::new(&form.token).content_type(InputContentType::Password).aria_label("服务器 令牌"),
        ))
        .child(form_field(t, view, Field::Note, "备注", false, None, Input::new(&form.note).aria_label("服务器 备注")));
    let testing = view.test == TestState::Running;
    let result: Option<AnyElement> = match &view.test {
        TestState::Idle => None,
        TestState::Running => Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(Spinner::new().xsmall().color(t.accent))
                .child(div().text_xs().text_color(t.muted).child(format!("正在测试 {}…", view.test_endpoint)))
                .into_any_element(),
        ),
        TestState::Done(probe) => {
            let (ok, text) = probe_text(probe, &view.test_endpoint);
            Some(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_start()
                    .child(Icon::new(if ok { IconName::CircleCheck } else { IconName::CircleX }).size_4().flex_none().text_color(if ok { t.ok } else { t.bad_text }))
                    .child(div().flex_1().min_w_0().text_xs().text_color(if ok { t.ink } else { t.bad_text }).child(text))
                    .into_any_element(),
            )
        }
    };
    let footer = h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(
            Button::new("fleet-test")
                .outline()
                .small()
                .icon(IconName::Network)
                .label("测试连接")
                .accessibility_label("服务器 测试连接")
                .loading(testing)
                .on_click(click(on, FleetAction::Test)),
        )
        .child(div().flex_1())
        .child(Button::new("fleet-cancel").ghost().small().label("取消").accessibility_label("服务器 取消编辑").on_click(click(on, FleetAction::CancelEdit)))
        .child(Button::new("fleet-save").primary().small().label("保存").accessibility_label("服务器 保存").on_click(click(on, FleetAction::Save)));
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .rounded(t.radius_lg)
        .border_1()
        .border_color(t.line)
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(fields)
        .children(result)
        .child(footer)
        .into_any_element()
}

/// 服务器抽屉（右侧 `Sheet`，520 px）的内容：上半服务器表，下半添加 / 编辑表单。
/// 面板在 `ui\sheets.rs` 的 `SheetKind::Servers` 那一支里调它（见 `remote\fleet-ui\PATCH-R13-app.md`）。
pub fn servers_drawer(t: &Tokens, view: &DrawerView, form: &ServerForm, hover: bool, on: &OnFleet) -> AnyElement {
    let editing = match view.mode {
        FormMode::Editing { id, .. } => Some(id),
        _ => None,
    };
    let notes = (!view.notes.is_empty()).then(|| {
        v_flex()
            .w_full()
            .gap_1()
            .p_3()
            .rounded(t.radius)
            .bg(t.soft)
            .children(view.notes.iter().map(|n| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(Icon::new(IconName::Info).size_3p5().text_color(t.warn_text))
                    .child(div().flex_1().text_xs().child(n.clone()))
            }))
    });
    let lower: AnyElement = match view.mode {
        FormMode::Closed => h_flex()
            .w_full()
            .child(
                Button::new("fleet-drawer-add")
                    .outline()
                    .small()
                    .icon(IconName::Plus)
                    .label("添加服务器")
                    .accessibility_label("抽屉 添加服务器")
                    .on_click(click(on, FleetAction::Add)),
            )
            .into_any_element(),
        _ => form_block(t, view, form, on),
    };
    v_flex()
        .w_full()
        .gap_5()
        .pt_1()
        .pb_6()
        .child(server_table(t, &view.rows, editing, hover, on))
        .children(notes)
        .child(lower)
        .into_any_element()
}

// 只测「能构造」和文案；画出来的样子在集成步的自检截图里逐张看（输入框要窗口，表单不在这里构造）。
// 不能 `use super::*`：上面的 `gpui_kit::*` 带进来 GPUI 自己的 `test` 宏，会盖掉 `#[test]`。
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::time::{Duration, Instant, SystemTime};

    use gpui_kit::component::kbd::Kbd;
    use gpui_kit::{App, Hsla, Keystroke, Window, px, rgb};

    use super::{
        FleetAction, OnFleet, SHORTCUT_KEYS, first_connect_skeleton, health_mark, key_bindings, last_ok_text, server_list, server_table, shortcut_label,
        summary_bar, unreachable_banner,
    };
    use crate::fleet_state::{DrawerRow, Failure, FleetState, Health, LOCAL_NAME, ListMode, Summary, SvcLite};
    use crate::model::{ReasonCode, Svc, SvcState};
    use crate::ui::format;
    use crate::ui::tokens::Tokens;

    /// 亮色令牌（DASHBOARD-DESIGN 11.1 的取值）。tokens.rs 加字段时这里跟着加一行。
    fn tokens() -> Tokens {
        let c = |hex: u32| -> Hsla { rgb(hex).into() };
        Tokens {
            dark: false,
            page: c(0xffffff),
            soft: c(0xf5f5f5),
            line: c(0xe5e5e5),
            ink: c(0x0a0a0a),
            ink2: c(0x525252),
            muted: c(0x737373),
            accent: c(0x3b82f6),
            ok: c(0x22c55e),
            warn_fill: c(0xeab308),
            warn_text: c(0xa16207),
            bad: c(0xef4444),
            bad_text: c(0xdc2626),
            selection_fill: c(0x3b82f6).opacity(0.07),
            selection_ring: c(0x3b82f6).opacity(0.30),
            spark: c(0x737373).opacity(0.75),
            radius: px(6.),
            radius_lg: px(8.),
            mono: "Consolas".into(),
        }
    }

    fn noop() -> OnFleet {
        Rc::new(|_: FleetAction, _: &mut Window, _: &mut App| {})
    }

    fn summary(rs: SvcState) -> Summary {
        let ok = Some(SvcLite { state: SvcState::Ok, reason: ReasonCode::None, pid: Some(1) });
        let mut s = Summary { services: [ok, ok, ok, ok], commit_free_gb: Some(18.0), disk_free_gb: Some(100.0), ..Default::default() };
        s.services[Svc::RsServer.index()] = Some(SvcLite { state: rs, reason: ReasonCode::None, pid: Some(0) });
        s
    }

    #[test]
    fn components_construct_in_every_state() {
        let t = tokens();
        let on = noop();
        let now = Instant::now();
        let mut f = FleetState::default();
        let a = f.add(LOCAL_NAME, "127.0.0.1:8600", true, now);
        let b = f.add("服务器乙", "b.example:8600", false, now);
        let c = f.add("服务器丙", "c.example:8600", false, now);
        let d = f.add("服务器丁", "d.example:8600", false, now);
        f.on_summary(a, summary(SvcState::Ok), now, SystemTime::now(), Some(2));
        f.on_summary(b, summary(SvcState::Bad), now, SystemTime::now(), Some(9));
        f.on_failure(c, Failure::Unauthorized, now);
        f.on_failure(c, Failure::Unauthorized, now);
        f.select(b);
        let rows = f.rows(now);
        for mode in [ListMode::Full, ListMode::Rail, ListMode::Hidden] {
            for hover in [true, false] {
                let _ = server_list(&t, &rows, mode, hover, None, &on);
            }
        }
        let bar = f.bar(now);
        assert!(bar.worst_is_selected);
        let _ = summary_bar(&t, &bar, true, &on);
        f.select(a);
        let _ = summary_bar(&t, &f.bar(now), false, &on);
        f.select(c);
        let banner = f.banner(now, Some("15:31:40")).expect("令牌不对连着两次算连不上");
        let _ = unreachable_banner(&t, c, &banner, &on);
        f.select(d);
        assert!(f.needs_skeleton());
        let _ = first_connect_skeleton(&t);
        for h in [Health::Connecting, Health::Down, Health::Unconfigured, Health::Starting, Health::Stopping, Health::Stopped, Health::Ok, Health::Warn, Health::Bad] {
            let _ = health_mark(&t, h);
        }
        let _ = server_table(&t, &f.drawer_rows(now), Some(b), true, &on);
        let _ = server_table(&t, &[], None, false, &on);
    }

    #[test]
    fn shortcut_labels_and_key_bindings() {
        assert_eq!(key_bindings().len(), 12);
        assert!(shortcut_label(1).is_some() && shortcut_label(9).is_some());
        assert!(shortcut_label(0).is_none() && shortcut_label(10).is_none());
        let labels: Vec<String> = SHORTCUT_KEYS.iter().map(|k| Kbd::format(&Keystroke::parse(*k).unwrap())).collect();
        assert_eq!(labels.first().map(String::as_str), Some("Ctrl+1"));
        assert_eq!(labels.last().map(String::as_str), Some("Ctrl+9"));
    }

    #[test]
    fn last_connected_column() {
        let row = |health: Health, last_ok_at: Option<SystemTime>| DrawerRow {
            id: 1,
            name: "服务器乙".into(),
            endpoint: "b.example:8600".into(),
            health,
            word: String::new(),
            last_ok_at,
            local: false,
            can_up: false,
            can_down: false,
        };
        assert_eq!(last_ok_text(&row(Health::Connecting, None)), format::NONE);
        assert_eq!(last_ok_text(&row(Health::Down, None)), "从没连上");
        let recent = last_ok_text(&row(Health::Ok, Some(SystemTime::now())));
        assert_eq!(recent.len(), 8, "{recent}");
        let old = last_ok_text(&row(Health::Down, SystemTime::now().checked_sub(Duration::from_secs(3 * 86_400))));
        assert_eq!(old.len(), 14, "{old}");
    }
}
