//! 顶栏（DASHBOARD-DESIGN 4.2 B）：左边回答「好不好、哪里不好、点哪里修」；起停期间换成进度；
//! 右边是「更新于」和全局操作。
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::data::Fault;
use crate::model::Svc;
use crate::state::{Overall, Sev, StepState};
use crate::ui::dialogs::DialogKind;
use crate::ui::format;
use crate::ui::tokens::{Tokens, tnum};

impl DashApp {
    pub(crate) fn render_header(&self, t: &Tokens, now: Instant, narrow: bool, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let overall = self.st.overall(now);
        let problems = self.st.problems(now);
        let (ok, enabled) = self.st.counts();
        let job_running = self.st.job.is_some();
        let lock_held = self.st.snapshot.as_ref().is_some_and(|s| s.lock.held);
        let any_running = self.st.snapshot.as_ref().is_some_and(|s| s.services.iter().any(|x| x.pid != 0));
        let all_ok = matches!(overall, Overall::AllOk) || (ok == enabled && enabled > 0 && !matches!(overall, Overall::Partial { .. } | Overall::Stopped));
        let show_fix = matches!(overall, Overall::Partial { .. }) && !problems.is_empty();

        let left: AnyElement = match overall {
            Overall::Starting | Overall::Stopping => self.render_progress(t, now, overall == Overall::Stopping),
            _ => {
                let (icon, color, word) = match overall {
                    Overall::Loading => (IconName::LoaderCircle, t.muted, "正在读取状态"),
                    Overall::Error => (IconName::CircleX, t.bad_text, "拿不到状态"),
                    Overall::Unconfigured => (IconName::CircleAlert, t.muted, "未配置"),
                    Overall::Stopped => (IconName::Square, t.muted, "已停止"),
                    Overall::AllOk => (IconName::CircleCheck, t.ok, "全部正常"),
                    Overall::Partial { red: true } => (IconName::CircleX, t.bad_text, "部分异常"),
                    Overall::Partial { red: false } => (IconName::TriangleAlert, t.warn_text, "部分异常"),
                    _ => (IconName::Info, t.muted, ""),
                };
                let word_color = match overall {
                    Overall::Partial { red: true } | Overall::Error => t.bad_text,
                    Overall::Partial { red: false } => t.warn_text,
                    _ => t.ink,
                };
                let count = match overall {
                    Overall::Stopped | Overall::AllOk | Overall::Partial { .. } => Some(format!("{ok}/{enabled}")),
                    _ => None,
                };
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(Icon::new(icon).size_5().text_color(color))
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(word_color).whitespace_nowrap().child(word))
                    .when_some(count, |this, c| this.child(div().text_lg().font_features(tnum()).text_color(t.ink2).child(c)))
                    .when(show_fix, |this| {
                        let top = problems[0].clone();
                        let rest: Vec<_> = problems.iter().skip(1).cloned().collect();
                        let color = t.sev_text(top.sev);
                        let fix = top.fix;
                        this.child(div().w(px(1.)).h(px(18.)).bg(t.line).mx_1())
                            .child(div().text_sm().text_color(color).whitespace_nowrap().truncate().child(top.text.clone()))
                            .child(
                                Button::new("fix-top")
                                    .primary()
                                    .small()
                                    .label(top.fix_label.clone())
                                    .on_click(cx.listener(move |this, _, window, cx| this.fix(fix, window, cx))),
                            )
                            .when(!rest.is_empty(), |this| {
                                let weak = cx.entity().downgrade();
                                let label = format!("另 {} 项", rest.len());
                                this.child(Button::new("more-problems").ghost().small().label(label).dropdown_menu(move |mut menu, _, _| {
                                    for p in rest.iter() {
                                        let weak = weak.clone();
                                        let fix = p.fix;
                                        let icon = if p.sev == Sev::Red { IconName::CircleX } else { IconName::TriangleAlert };
                                        menu = menu.item(PopupMenuItem::new(format!("{} · {}", p.text, p.fix_label)).icon(icon).on_click(move |_, window, cx| {
                                            let _ = weak.update(cx, |app, cx| app.fix(fix, window, cx));
                                        }));
                                    }
                                    menu
                                }))
                            })
                    })
                    .into_any_element()
            }
        };

        let updated = self.render_updated(t, now);
        let unconfigured = overall == Overall::Unconfigured;
        let frontend_url = self
            .st
            .snapshot
            .as_ref()
            .filter(|s| s.svc(Svc::Frontend).is_some_and(|f| f.state == crate::model::SvcState::Ok))
            .map(|s| s.urls.frontend.clone());
        let start_disabled = unconfigured || job_running || lock_held || all_ok || matches!(overall, Overall::Loading | Overall::Error);
        // 拿不到状态时不知道任务还在不在跑：按钮不转圈（转圈像是后端卡在启动）
        let frozen = self.st.frozen(now);
        let start_tip = if frozen {
            "拿不到状态，不知道任务进行到哪了"
        } else if unconfigured {
            "先配置工程"
        } else if job_running || lock_held {
            "有任务在跑，等它结束"
        } else if all_ok {
            "都在运行"
        } else {
            "按 数据库 → 后端 → rs-server → 前端 的顺序起没起的"
        };
        let stop_disabled = !any_running || job_running || lock_held;
        let stop_tip = if job_running || lock_held { "启动任务不能中途取消；等它结束再停" } else { "按 前端 → rs-server → 后端 → 数据库 的顺序停" };

        h_flex()
            .w_full()
            .h_12()
            .flex_shrink_0()
            .px_4()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(t.line)
            .child(div().flex_1().min_w_0().overflow_hidden().child(left))
            .child(updated)
            .when_some(frontend_url, |this, url| {
                this.child(
                    Link::new("open-web")
                        .href(url)
                        .child(h_flex().gap_1().items_center().text_sm().child("打开网页").child(Icon::new(IconName::ExternalLink).size_3p5())),
                )
            })
            .when(unconfigured, |this| {
                // 空状态里有同一个主按钮，这里退成描边，主按钮全屏只留一个
                this.child(
                    Button::new("start-config")
                        .outline()
                        .icon(IconName::Settings)
                        .label("开始配置…")
                        .on_click(cx.listener(|this, _, window, cx| this.open_config_sheet(window, cx))),
                )
            })
            .child({
                let b = Button::new("start-all").icon(IconName::Play).label("一键启动").disabled(start_disabled).when(self.hover_ui(), |b| b.tooltip(start_tip));
                let b = if show_fix || unconfigured || all_ok { b.outline() } else { b.primary() };
                b.loading(self.start_spinning(now))
                    .on_click(cx.listener(|this, _, window, cx| this.start_all(window, cx)))
            })
            .child(
                Button::new("stop-all")
                    .danger()
                    .outline()
                    .icon(IconName::Square)
                    .label(if narrow { "停止…" } else { "全部停止…" })
                    .accessibility_label("全部停止…")
                    .disabled(stop_disabled)
                    .when(self.hover_ui(), |b| b.tooltip(stop_tip))
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_stop_all(window, cx))),
            )
            .child(self.render_more_menu(cx))
            .into_any_element()
    }

    /// 「一键启动」转不转圈：有起的任务在跑、而且拿得到状态。
    pub(crate) fn start_spinning(&self, now: Instant) -> bool {
        !self.st.frozen(now) && self.st.job.as_ref().is_some_and(|j| !j.kind.is_stopping() && j.kind.is_lifecycle())
    }

    /// 拿不到状态时的页级横幅：全屏只有这一处，旧数据留着但变暗（skill：Error 状态）。
    pub(crate) fn render_error_banner(&self, t: &Tokens, _now: Instant, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.st.snapshot_failures < 2 {
            return None;
        }
        let reason = self.st.last_failure.clone().unwrap_or_else(|| "stack.ps1 没有回应".into());
        let when = self.st.snapshot.as_ref().map(|s| format!("下面是 {} 的数据。", format::clock(s.generated_at))).unwrap_or_default();
        Some(
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
                .child(div().flex_1().text_sm().child(format!("拿不到状态：{reason}。{when}")))
                .child(Button::new("banner-retry").small().outline().label("重试").on_click(cx.listener(|this, _, _, cx| {
                    this.cmd(crate::data::Command::RefreshNow);
                    cx.notify();
                })))
                .child(Button::new("banner-output").small().ghost().label("看输出").on_click(cx.listener(|this, _, _, cx| this.set_log_source(5, cx))))
                .into_any_element(),
        )
    }

    fn render_updated(&self, t: &Tokens, now: Instant) -> AnyElement {
        let (dot, text, color) = if self.st.snapshot_failures >= 2 {
            (t.bad, "拿不到状态".to_string(), t.bad_text)
        } else if let Some(at) = self.st.snapshot_at {
            let age = now.saturating_duration_since(at);
            if self.st.stale(now) {
                (t.warn_fill, format!("{} 前", format::duration(age)), t.warn_text)
            } else {
                let clock = self.st.snapshot.as_ref().map(|s| format::clock(s.generated_at)).unwrap_or_default();
                (t.ok, format!("更新于 {clock}"), t.muted)
            }
        } else {
            (t.muted, "正在读取".to_string(), t.muted)
        };
        let tip = match &self.st.snapshot {
            Some(s) => format!("{}：起停时每 1 秒、平时每 2 秒问一次状态；上一次用时 {:.1} 秒", self.driver.source_label(), s.elapsed_ms as f64 / 1000.0),
            None => "还没有拿到第一份状态".to_string(),
        };
        h_flex()
            .id("updated")
            .gap_1p5()
            .items_center()
            .flex_shrink_0()
            .text_xs()
            .text_color(color)
            .font_features(tnum())
            .child(div().size_2().rounded_full().bg(dot))
            .child(text)
            .when(self.hover_ui(), |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
            .into_any_element()
    }

    /// 起停进度：四步，做完 ✓、当前转圈、后面空心点；右边写「启动中 1/4 · 用时 15 秒」。
    fn render_progress(&self, t: &Tokens, now: Instant, stopping: bool) -> AnyElement {
        // 拿不到状态（15 秒没有新快照）：「用时」由 job_progress 定格，转圈也换成不动的记号
        let frozen = self.st.frozen(now);
        let spinner = |color: Hsla| if frozen { Icon::new(IconName::Pause).size_4().text_color(t.muted).into_any_element() } else { Spinner::new().small().color(color).into_any_element() };
        let Some(p) = self.st.job_progress(now) else {
            let word = if stopping { "停止中" } else { "启动中" };
            return h_flex()
                .gap_2()
                .items_center()
                .child(spinner(t.muted))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(word))
                .child(div().text_sm().text_color(t.muted).child("另一个命令占着运行目录"))
                .into_any_element();
        };
        let done = p.steps.iter().filter(|s| s.1 == StepState::Done).count();
        let total = p.steps.len();
        let word = if stopping { "停止中" } else { "启动中" };
        let mut row = h_flex().gap_2().items_center().min_w_0();
        row = row
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(word))
            .child(div().text_lg().font_features(tnum()).text_color(t.ink2).child(format!("{done}/{total}")))
            .child(div().w(px(1.)).h(px(18.)).bg(t.line).mx_1());
        for (k, (svc, state, dur)) in p.steps.iter().enumerate() {
            if k > 0 {
                row = row.child(div().w_5().h(px(1.)).bg(t.line));
            }
            let mark: AnyElement = match state {
                StepState::Done => Icon::new(IconName::Check).size_4().text_color(t.ok).into_any_element(),
                StepState::Current => spinner(t.accent),
                StepState::Pending => div().size_2().rounded_full().border_1().border_color(t.muted).into_any_element(),
            };
            let text_color = if *state == StepState::Pending { t.muted } else { t.ink };
            row = row.child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .flex_shrink_0()
                    .child(mark)
                    .child(div().text_sm().text_color(text_color).whitespace_nowrap().child(svc.title()))
                    .when_some(*dur, |this, d| this.child(div().text_xs().text_color(t.muted).font_features(tnum()).child(format::secs1(d)))),
            );
        }
        row = row.child(div().ml_2().text_xs().text_color(t.muted).font_features(tnum()).whitespace_nowrap().child(format!("用时 {}", format::secs1(p.elapsed))));
        row.into_any_element()
    }

    fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let backend_up = self.st.backend_up();
        let configured = self.st.snapshot.as_ref().is_some_and(|s| !s.first_run);
        let urls = self.st.snapshot.as_ref().map(|s| s.urls.clone()).unwrap_or_default();
        let frontend_ok = self.st.snapshot.as_ref().and_then(|s| s.svc(Svc::Frontend)).is_some_and(|f| f.state == crate::model::SvcState::Ok);
        let busy = self.st.job.is_some();
        let real = self.driver.is_real();
        Button::new("more")
            .ghost()
            .icon(IconName::EllipsisVertical)
            .accessibility_label("更多操作")
            .when(self.hover_ui(), |b| b.tooltip("更多操作"))
            .dropdown_menu(move |menu, _, _| {
                let w = weak.clone();
                let act = move |f: fn(&mut DashApp, &mut Window, &mut Context<DashApp>)| {
                    let w = w.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = w.update(cx, |app, cx| f(app, window, cx));
                    }
                };
                let menu = if real {
                    menu.item(PopupMenuItem::new("生成模型…").disabled(!backend_up || busy).on_click(act(|a, w, c| a.toggle_generate(true, w, c))))
                } else {
                    menu.item(PopupMenuItem::new("初始化库…").disabled(!backend_up || busy).on_click(act(|a, w, c| a.open_db_dialog(DialogKind::Init, w, c))))
                        .item(PopupMenuItem::new("增量更新…").disabled(!backend_up || busy).on_click(act(|a, w, c| a.open_db_dialog(DialogKind::Update, w, c))))
                };
                let menu = menu
                    .item(PopupMenuItem::new("自检").disabled(!configured || busy).on_click(act(|a, w, c| {
                        a.run_doctor(c);
                        a.config_step = 1;
                        a.open_config_sheet(w, c);
                    })))
                    .item(PopupMenuItem::new("改配置…").on_click(act(|a, w, c| {
                        a.config_step = 0;
                        a.open_config_sheet(w, c);
                    })))
                    .separator()
                    .item(PopupMenuItem::new("打开前端网页").icon(IconName::ExternalLink).disabled(!frontend_ok).on_click({
                        let u = urls.frontend.clone();
                        move |_, _, cx| cx.open_url(&u)
                    }))
                    .item(PopupMenuItem::new("打开后端设置页").icon(IconName::ExternalLink).disabled(!backend_up).on_click({
                        let u = format!("{}setup.html", urls.backend);
                        move |_, _, cx| cx.open_url(&u)
                    }))
                    .item(PopupMenuItem::new("打开运维面板").icon(IconName::ExternalLink).disabled(!backend_up).on_click({
                        let u = urls.ops.clone();
                        move |_, _, cx| cx.open_url(&u)
                    }))
                    .separator()
                    .item(PopupMenuItem::new("打开运行目录").on_click(act(|a, w, c| {
                        let p = a.root_dir();
                        a.open_folder(p, w, c);
                    })))
                    .item(PopupMenuItem::new("打开日志目录").on_click(act(|a, w, c| {
                        let p = format!("{}\\logs", a.root_dir());
                        a.open_folder(p, w, c);
                    })));
                if real {
                    menu
                } else {
                    menu.separator()
                        .item(PopupMenuItem::new("模拟：rs-server 退出").on_click(act(|a, _, c| a.inject(Fault::RsServerExit, c))))
                        .item(PopupMenuItem::new("模拟：现场故障").on_click(act(|a, _, c| a.inject(Fault::FieldIncident, c))))
                }
            })
            .into_any_element()
    }
}
