//! 服务表（DASHBOARD-DESIGN 4.2 D）：四行固定顺序，问题在这里亮，修的按钮就在这一行上。
//! gpui-kit 的静态 `Table` 行没有点击事件，而「单击一行就把日志筛到这个服务」是核心交互，所以行用 `h_flex` 自己排。
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gpui_kit::component::empty::{Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle};

use crate::app::DashApp;
use crate::model::{ReasonCode, Svc, SvcState};
use crate::state::{HEARTBEAT_LATE, Overall, RowKind};
use crate::ui::format;
use crate::ui::marks;
use crate::ui::tokens::{Tokens, tnum};

pub const HEAD_H: f32 = 34.0;
pub const ROW_H: f32 = 47.0;
pub const TABLE_H: f32 = HEAD_H + 1.0 + 4.0 * ROW_H + 3.0 + 2.0;

struct Cols {
    name: f32,
    status: f32,
    ticks: f32,
    cpu: f32,
    mem: f32,
    port: f32,
    pid: f32,
    up: f32,
    beat: f32,
}

fn cols(narrow: bool) -> Cols {
    if narrow {
        Cols { name: 150., status: 160., ticks: 150., cpu: 116., mem: 128., port: 0., pid: 0., up: 92., beat: 0. }
    } else {
        Cols { name: 136., status: 172., ticks: 180., cpu: 124., mem: 136., port: 56., pid: 64., up: 100., beat: 44. }
    }
}

pub fn svc_icon(svc: Svc) -> IconName {
    match svc {
        Svc::Database => IconName::HardDrive,
        Svc::Backend => IconName::Cpu,
        Svc::RsServer => IconName::Network,
        Svc::Frontend => IconName::Globe,
    }
}

/// 状态标签：词 + 图标形状；颜色只给问题（skill「安静」）。
/// `frozen`：拿不到状态，这一行停在最后一份快照的样子，起停中的转圈换成不动的暂停记号。
pub fn status_chip(t: &Tokens, kind: RowKind, phrase: &str, starting_for: Option<Duration>, waiting_for: Option<Duration>, frozen: bool) -> AnyElement {
    let phrase_el = |color: Hsla| div().text_xs().text_color(color).whitespace_nowrap().truncate().child(phrase.to_string());
    match kind {
        RowKind::Ok => h_flex()
            .gap_1p5()
            .items_center()
            .child(div().size_2().rounded_full().bg(t.ok))
            .child(div().text_sm().child("正常"))
            .into_any_element(),
        RowKind::Bad => h_flex()
            .gap_1p5()
            .items_center()
            .min_w_0()
            .child(Tag::danger().small().rounded_full().child(h_flex().gap_1().items_center().child(Icon::new(IconName::CircleX).size_3()).child("异常")))
            .child(phrase_el(t.bad_text))
            .into_any_element(),
        RowKind::Degraded | RowKind::WaitingLong => h_flex()
            .gap_1p5()
            .items_center()
            .min_w_0()
            .child(
                Tag::warning()
                    .small()
                    .rounded_full()
                    .child(h_flex().gap_1().items_center().child(Icon::new(IconName::TriangleAlert).size_3()).child(if kind == RowKind::Degraded { "降级" } else { "等待" })),
            )
            .child(phrase_el(t.warn_text))
            .into_any_element(),
        RowKind::Waiting => div()
            .text_sm()
            .text_color(t.muted)
            .whitespace_nowrap()
            .child(match waiting_for {
                Some(d) => format!("等待 · {}", format::duration(d)),
                None => "等待".to_string(),
            })
            .into_any_element(),
        RowKind::Starting | RowKind::Stopping => h_flex()
            .gap_1p5()
            .items_center()
            .child(if frozen { Icon::new(IconName::Pause).size_3().text_color(t.muted).into_any_element() } else { Spinner::new().xsmall().color(t.accent).into_any_element() })
            .child(div().text_sm().whitespace_nowrap().font_features(tnum()).child(match (kind, starting_for) {
                (RowKind::Starting, Some(d)) => format!("启动中 {}", format::secs1(d)),
                (RowKind::Starting, None) => "启动中".to_string(),
                _ => "停止中".to_string(),
            }))
            .into_any_element(),
        RowKind::Queued => div().text_sm().text_color(t.muted).child("排队").into_any_element(),
        RowKind::Stopped => Tag::secondary()
            .small()
            .rounded_full()
            .child(h_flex().gap_1().items_center().child(Icon::new(IconName::Square).size_3()).child("已停止"))
            .into_any_element(),
        RowKind::Disabled => div().text_sm().text_color(t.muted).child("未启用").into_any_element(),
        RowKind::Unconfigured => div().text_sm().text_color(t.muted).child("未配置").into_any_element(),
    }
}

fn head(t: &Tokens, c: &Cols, narrow: bool) -> AnyElement {
    let cell = |w: f32, text: &'static str, right: bool| {
        div().w(px(w)).flex_shrink_0().whitespace_nowrap().when(right, |d| d.text_right()).child(text)
    };
    h_flex()
        .h(px(HEAD_H))
        .px_3()
        .gap_3()
        .items_center()
        .bg(t.soft)
        .border_b_1()
        .border_color(t.line)
        .text_xs()
        .text_color(t.muted)
        .child(cell(c.name, "服务", false))
        .child(cell(c.status, "状态", false))
        .child(cell(c.ticks, "近 15 分钟", false))
        .child(cell(c.cpu, "CPU", false))
        .child(cell(c.mem, "内存", false))
        .when(!narrow, |this| this.child(cell(c.port, "端口", true)).child(cell(c.pid, "进程号", true)))
        .child(cell(c.up, "已运行", true))
        .when(!narrow, |this| this.child(cell(c.beat, "心跳", false)))
        .child(div().flex_1())
        .into_any_element()
}

impl DashApp {
    pub(crate) fn render_services(&self, t: &Tokens, now: Instant, narrow: bool, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cols(narrow);
        let mut table = v_flex()
            .w_full()
            .h(px(TABLE_H))
            .border_1()
            .border_color(t.line)
            .rounded(t.radius_lg)
            .overflow_hidden()
            .child(head(t, &c, narrow));
        for (k, svc) in Svc::ALL.iter().copied().enumerate() {
            if k > 0 {
                table = table.child(div().h(px(1.)).w_full().bg(t.line));
            }
            table = table.child(self.render_row(t, now, svc, &c, narrow, cx));
        }
        if self.st.overall(now) != Overall::Unconfigured {
            return table.into_any_element();
        }
        // 没配置：保留表的结构，上面盖一个空状态（skill：没有数据不是好消息，给下一步）
        let empty = Empty::new()
            .header(
                EmptyHeader::new()
                    .media(EmptyMedia::new().with_variant(EmptyMediaVariant::Icon).child(Icon::new(IconName::Settings)))
                    .title(EmptyTitle::new().child("还没有配置工程"))
                    .description(EmptyDescription::new().child("填好工程目录、项目名和编码，检查通过就能启动。")),
            )
            .content(
                EmptyContent::new().child(
                    Button::new("empty-config")
                        .primary()
                        .icon(IconName::Settings)
                        .label("开始配置…")
                        .on_click(cx.listener(|this, _, window, cx| this.open_config_sheet(window, cx))),
                ),
            );
        div()
            .relative()
            .w_full()
            .child(table)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .top(px(HEAD_H + 1.0))
                    .bg(t.page.opacity(0.86))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(empty),
            )
            .into_any_element()
    }

    fn render_row(&self, t: &Tokens, now: Instant, svc: Svc, c: &Cols, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let i = svc.index();
        let snap = self.st.snapshot.as_ref().and_then(|s| s.svc(svc)).cloned();
        let kind = self.st.row_kind(svc);
        let selected = self.st.selected == Some(svc);
        let (port, pid, reason) = match &snap {
            Some(s) => (s.port, s.pid, s.reason_code),
            None => (0, 0, ReasonCode::None),
        };
        let phrase = match (kind, reason) {
            (RowKind::Bad, ReasonCode::None) => {
                if pid == 0 { "进程不在" } else { "检查没通过" }
            }
            (RowKind::Degraded, _) => "看详情",
            (_, r) => r.phrase(),
        };
        let clock = self.st.clock(now);
        let starting_for = self.st.lifecycle_job().and_then(|j| j.step_started[i]).map(|t0| clock.saturating_duration_since(t0));
        let waiting_for = self.st.waiting_since[i].map(|t0| clock.saturating_duration_since(t0));
        let line_color = if selected { t.accent } else { t.spark };

        // 名称（窄窗口时端口、进程号并进第二行）
        let name = h_flex()
            .w(px(c.name))
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(
                div()
                    .size_7()
                    .flex_shrink_0()
                    .rounded(t.radius)
                    .bg(t.soft)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(svc_icon(svc)).size_4().text_color(t.ink2)),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).whitespace_nowrap().child(svc.title()))
                    .when(narrow, |this| {
                        let pid_text = if pid == 0 { format::NONE.to_string() } else { pid.to_string() };
                        this.child(div().text_xs().text_color(t.muted).font_features(tnum()).whitespace_nowrap().child(format!("{port} · {pid_text}")))
                    }),
            );

        let ticks = self.st.rows[i].buckets(now, if narrow { 24 } else { 30 });
        let bad_for = self.st.rows[i].bad_duration(now);
        let tick_tip = if bad_for >= Duration::from_secs(1) { format!("近 15 分钟：异常约 {}；每格 30 秒，取那段时间最差的状态", format::duration(bad_for)) } else { "近 15 分钟没有异常；每格 30 秒，空心 = 那段时间面板还没开".to_string() };
        let hover = self.hover_ui();
        let ticks_el = div()
            .id(("ticks", i))
            .w(px(c.ticks))
            .h(px(16.))
            .flex_shrink_0()
            .child(marks::ticks(ticks, t))
            .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(tick_tip.clone()).build(window, cx)));

        let cpu_now = self.st.metrics.as_ref().and_then(|m| m.per_svc[i]).map(|p| p.cpu_pct).filter(|_| pid != 0);
        let mem_now = self.st.metrics.as_ref().and_then(|m| m.per_svc[i]).map(|p| p.mem_gb).filter(|_| pid != 0);
        let mem_limit = self.st.snapshot.as_ref().map(|s| s.guard.max_backend_gb).unwrap_or(0.0);
        let mem_over = svc == Svc::Backend && mem_limit > 0.0 && mem_now.is_some_and(|m| m > mem_limit);
        let cores = self.st.metrics.as_ref().map(|m| m.cores).unwrap_or(0);
        let metric = |id: (&'static str, usize), w: f32, series: Vec<Option<f64>>, color: Hsla, value: Option<String>, value_color: Hsla, refs: Vec<(f64, Hsla)>, tip: String| {
            h_flex()
                .id(id)
                .w(px(w))
                .flex_shrink_0()
                .gap_2()
                .items_center()
                .child(div().w(px(w - 56.)).h(px(22.)).child(marks::spark(series, color, None, refs)))
                .child(div().w(px(48.)).text_right().text_sm().font_features(tnum()).text_color(value_color).whitespace_nowrap().child(value.unwrap_or_else(|| format::NONE.into())))
                .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
        };
        let cpu_el = metric(
            ("cpu", i),
            c.cpu,
            self.st.cpu[i].buckets(now, 40),
            line_color,
            cpu_now.map(format::pct),
            t.ink,
            Vec::new(),
            format!("CPU 占整机（{cores} 核）；折线是近 15 分钟，每段约 22 秒取平均，没有进程的时段留空"),
        );
        let mem_refs = if svc == Svc::Backend && mem_limit > 0.0 { vec![(mem_limit, t.bad.opacity(0.45))] } else { Vec::new() };
        let mem_el = metric(
            ("mem", i),
            c.mem,
            self.st.mem[i].buckets(now, 40),
            if mem_over { t.bad } else { line_color },
            mem_now.map(format::gb),
            if mem_over { t.bad_text } else { t.ink },
            mem_refs,
            if svc == Svc::Backend && mem_limit > 0.0 { format!("工作集和私有提交取大的那个；虚线是上限 {}（[guard] max_backend_gb）", format::gb(mem_limit)) } else { "工作集和私有提交取大的那个（和 stack.ps1 的算法一样）".to_string() },
        );

        let uptime = self.st.uptime(svc);
        let started = snap.as_ref().and_then(|s| s.started_at);
        let up_tip = started.map(|s| format!("起于 {}", format::date_clock(s))).unwrap_or_else(|| "没有在运行".to_string());
        let up_el = div()
            .id(("uptime", i))
            .w(px(c.up))
            .flex_shrink_0()
            .text_right()
            .text_sm()
            .font_features(tnum())
            .whitespace_nowrap()
            .child(uptime.map(|d| if narrow { format::duration_short(d) } else { format::duration(d) }).unwrap_or_else(|| format::NONE.into()))
            .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(up_tip.clone()).build(window, cx)));

        let beat: AnyElement = match (snap.as_ref().map(|s| s.state), self.st.last_ok[i]) {
            (Some(SvcState::Ok), Some(at)) => {
                let age = now.saturating_duration_since(at);
                if age > HEARTBEAT_LATE {
                    div().text_xs().text_color(t.warn_text).font_features(tnum()).child(format!("{} 前", format::duration(age))).into_any_element()
                } else {
                    div().size_2().rounded_full().bg(t.ok).opacity(if age.as_millis() < 900 { 1.0 } else { 0.55 }).into_any_element()
                }
            }
            _ => div().text_sm().text_color(t.muted).child(format::NONE).into_any_element(),
        };

        let ops = self.render_row_ops(t, svc, kind, pid, cx);

        h_flex()
            .id(("svc-row", i))
            .h(px(ROW_H))
            .px_3()
            .gap_3()
            .items_center()
            .border_1()
            .border_color(if selected { t.selection_ring } else { transparent_black() })
            .rounded(t.radius)
            .cursor_pointer()
            .when(selected, |this| this.bg(t.selection_fill))
            .when(!selected && hover, |this| this.hover(|s| s.bg(t.soft.opacity(0.6))))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_select(svc, cx)))
            .child(name)
            .child(div().w(px(c.status)).flex_shrink_0().overflow_hidden().child(status_chip(t, kind, phrase, starting_for, waiting_for, self.st.frozen(now))))
            .child(ticks_el)
            .child(cpu_el)
            .child(mem_el)
            .when(!narrow, |this| {
                this.child(div().w(px(c.port)).flex_shrink_0().text_right().text_sm().text_color(t.muted).font_features(tnum()).child(port.to_string()))
                    .child(div().w(px(c.pid)).flex_shrink_0().text_right().text_sm().text_color(t.muted).font_features(tnum()).child(if pid == 0 { format::NONE.to_string() } else { pid.to_string() }))
            })
            .child(up_el)
            .when(!narrow, |this| this.child(div().w(px(c.beat)).flex_shrink_0().flex().items_center().child(beat)))
            .child(div().flex_1().min_w_0().child(ops))
            .into_any_element()
    }

    fn render_row_ops(&self, _t: &Tokens, svc: Svc, kind: RowKind, pid: u32, cx: &mut Context<Self>) -> AnyElement {
        let i = svc.index();
        let busy = self.st.job.is_some();
        let name = svc.title();
        // stack 还没有公开的 -Service：真实模式下在跑的服务不能单独重启、停止，按钮不出，菜单项置灰
        let single_ok = !self.driver.is_real();
        let primary: Option<AnyElement> = match kind {
            RowKind::Bad if single_ok || pid == 0 => Some(
                Button::new(("row-restart", i))
                    .small()
                    .outline()
                    .label("重启")
                    .accessibility_label(format!("{name} 重启"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.restart(svc, window, cx);
                    }))
                    .into_any_element(),
            ),
            RowKind::Stopped => Some(
                Button::new(("row-start", i))
                    .small()
                    .outline()
                    .label("启动")
                    .accessibility_label(format!("{name} 启动"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.cmd(crate::data::Command::StartService(svc));
                    }))
                    .into_any_element(),
            ),
            RowKind::Ok | RowKind::Degraded | RowKind::WaitingLong | RowKind::Waiting if !busy && (single_ok || pid == 0) => Some(
                Button::new(("row-restart", i))
                    .small()
                    .ghost()
                    .label("重启")
                    .accessibility_label(format!("{name} 重启"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.restart(svc, window, cx);
                    }))
                    .into_any_element(),
            ),
            _ => None,
        };
        let logs_btn = (kind == RowKind::Bad || kind == RowKind::Degraded).then(|| {
            Button::new(("row-logs", i))
                .small()
                .ghost()
                .label("看日志")
                .accessibility_label(format!("{name} 看日志"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.view_logs(svc, cx);
                }))
                .into_any_element()
        });
        let weak = cx.entity().downgrade();
        let running = pid != 0;
        let url = self.st.snapshot.as_ref().map(|s| match svc {
            Svc::Frontend => s.urls.frontend.clone(),
            Svc::Backend => s.urls.backend.clone(),
            _ => String::new(),
        });
        let log_dir = self
            .st
            .snapshot
            .as_ref()
            .and_then(|s| s.svc(svc))
            .and_then(|s| std::path::Path::new(&s.log).parent().map(|p| p.display().to_string()))
            .unwrap_or_default();
        let more = Button::new(("row-more", i))
            .small()
            .ghost()
            .icon(IconName::EllipsisVertical)
            .accessibility_label(format!("{name} 更多"))
            .when(self.hover_ui(), |b| b.tooltip(format!("{name} 更多")))
            .dropdown_menu(move |menu, _, _| {
            let w1 = weak.clone();
            let w2 = weak.clone();
            let w3 = weak.clone();
            let w4 = weak.clone();
            let dir = log_dir.clone();
            let mut menu = menu
                .item(PopupMenuItem::new("详情…").on_click(move |_, window, cx| {
                    let _ = w1.update(cx, |app, cx| app.open_service_sheet(svc, window, cx));
                }))
                .item(PopupMenuItem::new("看日志").on_click(move |_, _, cx| {
                    let _ = w2.update(cx, |app, cx| app.view_logs(svc, cx));
                }))
                .item(PopupMenuItem::new(if single_ok { "停止…" } else { "停止…（stack 不能单独停，用「全部停止…」）" }).disabled(!running || busy || !single_ok).on_click(move |_, window, cx| {
                    let _ = w3.update(cx, |app, cx| app.confirm_stop(svc, window, cx));
                }))
                .item(PopupMenuItem::new("打开日志所在文件夹").on_click(move |_, window, cx| {
                    let d = dir.clone();
                    let _ = w4.update(cx, |app, cx| app.open_folder(d, window, cx));
                }));
            if let Some(u) = url.clone().filter(|u| !u.is_empty()) {
                menu = menu.separator().item(PopupMenuItem::new("打开网页").icon(IconName::ExternalLink).disabled(!running).on_click(move |_, _, cx| cx.open_url(&u)));
            }
            menu
        });
        h_flex()
            .w_full()
            .justify_end()
            .gap_1()
            .items_center()
            .children(primary)
            .children(logs_btn)
            .child(more)
            .into_any_element()
    }
}