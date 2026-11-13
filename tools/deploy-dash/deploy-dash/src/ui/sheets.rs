//! 右侧抽屉（DASHBOARD-DESIGN 第 9 节、7.1）：配置工程、服务详情、后端任务。
//! `open_sheet_at` 的构建闭包可能被反复调用、内容不会自己刷新，所以抽屉里放一个观察面板的视图：
//! 面板一通知，抽屉跟着重画（服务详情里的走势、检查结果都是活的）。
use std::time::{Instant, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::input::Input;
use gpui_kit::component::link::Link;
use gpui_kit::component::select::Select;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::stepper::{Stepper, StepperItem};
use gpui_kit::component::{Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::model::{QueueState, Svc, SvcState};
use crate::state::RowKind;
use crate::ui::format;
use crate::ui::marks;
use crate::ui::services::{status_chip, svc_icon};
use crate::ui::tokens::{Tokens, tnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetKind {
    Config,
    Service(Svc),
    Jobs,
    /// 生成模型：选库（只给勾选的库生成）。
    Generate,
    /// 服务器：添加、编辑、删除、测试连接（多服务器，R13）。
    Servers,
}

pub struct SheetView {
    app: WeakEntity<DashApp>,
    kind: SheetKind,
    _sub: Subscription,
}

impl SheetView {
    pub fn new(app: &Entity<DashApp>, kind: SheetKind, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(app, |_, _, cx| cx.notify());
        Self { app: app.downgrade(), kind, _sub: sub }
    }
}

impl Render for SheetView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else { return div().into_any_element() };
        let t = Tokens::new(cx);
        let weak = self.app.clone();
        let a = app.read(cx);
        match self.kind {
            SheetKind::Config => a.config_sheet(&t, weak, cx),
            SheetKind::Service(svc) => a.service_sheet(&t, svc, weak, cx),
            SheetKind::Jobs => a.jobs_sheet(&t, weak, cx),
            SheetKind::Generate => a.generate_sheet(&t, weak, cx),
            SheetKind::Servers => a.servers_sheet(&t, weak, cx),
        }
    }
}

/// 抽屉里的一个小节：小标题 + 内容。
fn section(t: &Tokens, title: &str, body: impl IntoElement) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.muted).child(title.to_string()))
        .child(body)
        .into_any_element()
}

fn act(weak: &WeakEntity<DashApp>, f: fn(&mut DashApp, &mut Window, &mut Context<DashApp>)) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let weak = weak.clone();
    move |_, window, cx| {
        let _ = weak.update(cx, |app, cx| f(app, window, cx));
    }
}

impl DashApp {
    fn field(&self, t: &Tokens, key: &'static str, label: &str, hint: Option<&str>, control: AnyElement) -> AnyElement {
        let err = self.form_errors.iter().find(|(k, _)| *k == key).map(|(_, m)| m.clone());
        v_flex()
            .w_full()
            .gap_1()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label.to_string()))
            .child(control)
            .when_some(err.clone(), |this, e| {
                this.child(h_flex().gap_1().items_center().text_xs().text_color(t.bad_text).child(Icon::new(IconName::CircleX).size_3()).child(e))
            })
            .when(err.is_none() && hint.is_some(), |this| this.child(div().text_xs().text_color(t.muted).child(hint.unwrap_or_default().to_string())))
            .into_any_element()
    }

    pub(crate) fn config_sheet(&self, t: &Tokens, weak: WeakEntity<DashApp>, cx: &App) -> AnyElement {
        let snap = self.st.snapshot.as_ref();
        let running = snap.is_some_and(|s| s.services.iter().any(|x| x.pid != 0));
        let generated = snap.is_some_and(|s| s.backend_config_generated);
        let locked = running || generated;
        let step = self.config_step;
        let stepper = Stepper::new("cfg-steps")
            .small()
            .selected_index(step)
            .items([StepperItem::new().child("工程"), StepperItem::new().child("检查"), StepperItem::new().child("启动")]);

        let body: AnyElement = if step == 0 {
            // 远程（R10）：路径是服务器上的，本机的选目录对话框没有意义，换成问代理「探测」
            let remote = self.current_is_remote();
            let pick = if remote {
                Button::new("probe-root")
                    .label("探测")
                    .accessibility_label("工程根目录 探测")
                    .loading(self.root_probe == crate::app::RootProbe::Running)
                    .disabled(locked)
                    .on_click(act(&weak, |a, w, c| a.probe_root(w, c)))
            } else {
                Button::new("pick-root").label("选择…").disabled(locked).on_click(act(&weak, |a, w, c| a.pick_root(w, c)))
            };
            let probe_line = match &self.root_probe {
                crate::app::RootProbe::Done(ok, text) if remote => Some(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_start()
                        .text_xs()
                        .text_color(if *ok { t.ink2 } else { t.warn_text })
                        .child(Icon::new(if *ok { IconName::CircleCheck } else { IconName::TriangleAlert }).size_3().flex_none().mt_0p5().text_color(if *ok { t.ok } else { t.warn_text }))
                        .child(div().flex_1().min_w_0().child(text.clone())),
                ),
                _ => None,
            };
            let root_row = v_flex()
                .w_full()
                .gap_1()
                .child(h_flex().w_full().gap_2().child(div().flex_1().child(Input::new(&self.form.root).disabled(locked))).child(pick))
                .children(probe_line)
                .into_any_element();
            v_flex()
                .w_full()
                .gap_4()
                .when(running, |this| {
                    this.child(note(t, IconName::Info, "有服务在运行：先全部停止，才能改工程配置。", t.muted))
                })
                .when(!running && generated, |this| {
                    this.child(note(
                        t,
                        IconName::TriangleAlert,
                        "后端配置已经生成：这里改工程目录，后端不会跟着变。要换工程，先全部停止，把运行目录下 work\\backend\\DbOption.toml 挪走，再保存并检查。",
                        t.warn_text,
                    ))
                })
                .child(self.field(t, "root", "E3D 工程根目录", Some(if remote { "服务器上的路径；填好点「探测」让代理核对" } else { "它下面是各个项目的文件夹" }), root_row))
                .child(self.field(t, "name", "项目名", None, Input::new(&self.form.name).disabled(locked).into_any_element()))
                .child(self.field(t, "dir", "文件夹名", Some("和项目名不一样时才填；只能写一段名字"), Input::new(&self.form.dir).disabled(locked).into_any_element()))
                .child(self.field(t, "code", "项目编码", Some("整数；同时用作数据库里的命名空间"), Input::new(&self.form.code).disabled(locked).into_any_element()))
                .child(self.field(t, "version", "E3D 版本", None, div().w(px(220.)).child(Select::new(&self.form.version).disabled(locked)).into_any_element()))
                .child(self.field(t, "mdb", "MDB", Some("缺省 ALL：全部"), Input::new(&self.form.mdb).disabled(locked).into_any_element()))
                .into_any_element()
        } else if self.st.doctor_running || self.st.doctor.is_none() {
            v_flex()
                .w_full()
                .gap_3()
                .py_6()
                .items_center()
                .child(Spinner::new().large().color(t.accent))
                .child(div().text_sm().child("检查中…"))
                .child(div().text_xs().text_color(t.muted).child("第一次在这台机器上运行时，杀毒软件要先扫一遍程序，可能要几十秒。"))
                .into_any_element()
        } else {
            let report = self.st.doctor.clone().unwrap_or_else(|| crate::model::DoctorReport { can_start: false, problems: Vec::new(), notes: Vec::new() });
            let mut col = v_flex().w_full().gap_3();
            if report.can_start {
                col = col.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::CircleCheck).size_5().text_color(t.ok))
                        .child(div().text_base().font_weight(FontWeight::MEDIUM).child("检查通过")),
                );
            }
            for p in &report.problems {
                col = col.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .p_3()
                        .border_1()
                        .border_color(t.line)
                        .rounded(t.radius)
                        .child(h_flex().gap_1p5().items_center().child(Icon::new(IconName::CircleX).size_4().text_color(t.bad_text)).child(div().text_sm().font_weight(FontWeight::MEDIUM).child(p.what.clone())))
                        .child(div().text_xs().text_color(t.muted).child(format!("查的是：{}", p.checked)))
                        .child(div().text_xs().child(format!("结果：{}", p.result)))
                        .child(div().text_xs().text_color(t.warn_text).child(format!("下一步：{}", p.next))),
                );
            }
            col = col.child(section(
                t,
                "备注",
                v_flex().gap_1().children(report.notes.iter().map(|n| div().text_xs().text_color(t.muted).font_features(tnum()).child(format!("· {n}")))),
            ));
            col.into_any_element()
        };

        let can_start = self.st.doctor.as_ref().is_some_and(|d| d.can_start) && !self.st.doctor_running;
        let footer = if step == 0 {
            h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(Button::new("cfg-cancel").ghost().label("取消").on_click(act(&weak, |a, w, c| a.close_sheet(w, c))))
                .child(Button::new("cfg-save").primary().label("保存并检查").disabled(locked).on_click(act(&weak, |a, w, c| a.save_and_check(w, c))))
        } else {
            h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(Button::new("cfg-back").ghost().label("返回修改").on_click(act(&weak, |a, _, c| {
                    a.config_step = 0;
                    c.notify();
                })))
                .child(Button::new("cfg-start").primary().icon(IconName::Play).label("启动").disabled(!can_start || running).on_click(act(&weak, |a, w, c| a.start_from_sheet(w, c))))
        };
        let _ = cx;
        v_flex()
            .size_full()
            .gap_5()
            .pt_1()
            .child(stepper)
            .child(div().flex_1().min_h_0().child(body))
            .child(div().pt_3().pb_4().border_t_1().border_color(t.line).child(footer))
            .into_any_element()
    }

    pub(crate) fn service_sheet(&self, t: &Tokens, svc: Svc, weak: WeakEntity<DashApp>, _cx: &App) -> AnyElement {
        let now = Instant::now();
        let i = svc.index();
        let snap = self.st.snapshot.as_ref().and_then(|s| s.svc(svc)).cloned();
        let kind = self.st.row_kind(svc);
        let reason = snap.as_ref().map(|s| s.reason_code.phrase()).unwrap_or_default();
        let pid = snap.as_ref().map(|s| s.pid).unwrap_or(0);
        let busy = self.st.job.is_some();
        // 和服务行一样：stack 还没有公开的 -Service，真实模式下在跑的服务不能单独重启、停止
        let single_ok = !self.driver.is_real();
        let w0 = weak.clone();
        let w1 = weak.clone();
        let w2 = weak.clone();
        let w3 = weak.clone();
        let actions = h_flex()
            .gap_2()
            .when((kind == RowKind::Bad || kind == RowKind::Stopped) && (single_ok || pid == 0), |this| {
                this.child(Button::new("sd-start").primary().small().label(if kind == RowKind::Bad { "重启" } else { "启动" }).disabled(busy).on_click(move |_, window, cx| {
                    let _ = w0.update(cx, |app, cx| app.restart(svc, window, cx));
                }))
            })
            .when(matches!(kind, RowKind::Ok | RowKind::Degraded | RowKind::Waiting | RowKind::WaitingLong) && (single_ok || pid == 0), |this| {
                this.child(Button::new("sd-restart").outline().small().label("重启").disabled(busy).on_click(move |_, window, cx| {
                    let _ = w1.update(cx, |app, cx| app.restart(svc, window, cx));
                }))
            })
            .child(Button::new("sd-stop").danger().outline().small().label("停止…").disabled(pid == 0 || busy || !single_ok).on_click(move |_, window, cx| {
                let _ = w2.update(cx, |app, cx| app.confirm_stop(svc, window, cx));
            }))
            .child(Button::new("sd-logs").ghost().small().label("看日志").on_click(move |_, window, cx| {
                let _ = w3.update(cx, |app, cx| {
                    app.view_logs(svc, cx);
                    app.close_sheet(window, cx);
                });
            }));

        let head = h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .child(div().size_9().rounded(t.radius).bg(t.soft).flex().items_center().justify_center().child(Icon::new(svc_icon(svc)).size_5().text_color(t.ink2)))
            .child(status_chip(t, kind, reason, None, None, self.st.frozen(Instant::now())))
            .child(div().flex_1())
            .child(actions);

        let state_body: AnyElement = match &snap {
            Some(s) if !s.reason.is_empty() || !s.next.is_empty() => v_flex()
                .gap_1p5()
                .when(!s.reason.is_empty(), |this| this.child(div().text_sm().text_color(if kind == RowKind::Bad { t.bad_text } else { t.ink }).child(s.reason.clone())))
                .child(div().text_xs().text_color(t.muted).child(format!("查的是：{}", s.checked)))
                .when(!s.next.is_empty(), |this| this.child(div().text_xs().text_color(t.warn_text).child(format!("下一步：{}", s.next))))
                .into_any_element(),
            Some(s) => v_flex()
                .gap_1()
                .child(div().text_sm().child(if s.state == SvcState::Ok { "检查通过" } else { "没有在运行" }))
                .child(div().text_xs().text_color(t.muted).child(format!("查的是：{}", s.checked)))
                .into_any_element(),
            None => div().text_sm().text_color(t.muted).child("还没有状态").into_any_element(),
        };
        let state_body = if !single_ok && pid != 0 {
            v_flex()
                .gap_1p5()
                .child(state_body)
                .child(div().text_xs().text_color(t.muted).child("stack 还不能单独停、单独重启在跑的服务：要停就用顶栏「全部停止…」，要重启就全部停止后再「一键启动」。"))
                .into_any_element()
        } else {
            state_body
        };

        let cpu = self.st.cpu[i].buckets(now, 90);
        let mem = self.st.mem[i].buckets(now, 90);
        let cpu_max = cpu.iter().flatten().fold(0.0f64, |a, b| a.max(*b));
        let mem_limit = self.st.snapshot.as_ref().map(|s| s.guard.max_backend_gb).unwrap_or(0.0);
        let mem_refs = if svc == Svc::Backend && mem_limit > 0.0 { vec![(mem_limit, t.bad.opacity(0.7))] } else { Vec::new() };
        let mem_top = mem.iter().flatten().fold(0.0f64, |a, b| a.max(*b)).max(if mem_refs.is_empty() { 0.0 } else { mem_limit }) * 1.15;
        let cpu_top = (cpu_max * 1.15).max(1.0);
        let cpu_now = self.st.metrics.as_ref().and_then(|m| m.per_svc[i]).map(|p| p.cpu_pct).filter(|_| pid != 0);
        let mem_now = self.st.metrics.as_ref().and_then(|m| m.per_svc[i]).map(|p| p.mem_gb).filter(|_| pid != 0);
        let cores = self.st.metrics.as_ref().map(|m| m.cores).unwrap_or(0);
        let metric_title = |label: &str, now_v: Option<String>| {
            h_flex()
                .w_full()
                .justify_between()
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.muted).child(label.to_string()))
                .child(div().text_sm().font_features(tnum()).child(now_v.unwrap_or_else(|| format::NONE.into())))
        };

        let mut info = DescriptionList::new().columns(2).bordered(false);
        if let Some(s) = &snap {
            let started = s.started_at.map(format::date_clock).unwrap_or_else(|| format::NONE.into());
            let up = self.st.uptime(svc).map(format::duration).unwrap_or_else(|| format::NONE.into());
            info = info
                .item("端口", SharedString::from(s.port.to_string()), 1)
                .item("进程号", SharedString::from(if s.pid == 0 { format::NONE.to_string() } else { s.pid.to_string() }), 1)
                .item("起于", SharedString::from(started), 1)
                .item("已运行", SharedString::from(up), 1)
                .item("版本", SharedString::from(s.version.clone()), 1)
                .item("本套记录", if s.ours { "是（进程号、启动时间、程序路径都对得上）" } else { "否" }, 1);
        }
        let log_path = snap.as_ref().map(|s| s.log.clone()).unwrap_or_default();
        let log_dir = std::path::Path::new(&log_path).parent().map(|p| p.display().to_string()).unwrap_or_default();
        let w4 = weak.clone();
        let log_row = h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .child(div().flex_1().min_w_0().text_xs().font_family(t.mono.clone()).text_color(t.ink2).child(log_path.clone()))
            .child(Button::new("sd-open-log").ghost().xsmall().label("打开所在文件夹").on_click(move |_, window, cx| {
                let d = log_dir.clone();
                let _ = w4.update(cx, |app, cx| app.open_folder(d, window, cx));
            }));
        if svc == Svc::Backend
            && let Some(b) = self.st.snapshot.as_ref().and_then(|s| s.backend.clone())
        {
            info = info
                .item("健康", SharedString::from(if b.status == "ok" { "正常".to_string() } else { format!("降级：{}", b.degraded_sections.join("、")) }), 1)
                .item("主项目定位", SharedString::from(if b.mdb_closure_status == "ok" { "已定位".to_string() } else { "找不到工程目录".to_string() }), 1)
                .item("批处理线程", SharedString::from(if b.worker_alive { format!("活着，空闲 {} 秒", b.worker_idle_secs) } else { "没有心跳".to_string() }), 1)
                .item("HTTP 请求数", SharedString::from(b.requests_total.map(format::thousands).map(|n| format!("{n}（模拟；后端现在没有这项，待接）")).unwrap_or_else(|| "待接".into())), 1);
        }

        let urls = self.st.snapshot.as_ref().map(|s| s.urls.clone()).unwrap_or_default();
        let links: Vec<(String, String)> = match svc {
            Svc::Frontend => vec![("打开前端网页".into(), urls.frontend.clone())],
            Svc::Backend => vec![("打开后端设置页".into(), format!("{}setup.html", urls.backend)), ("打开运维面板".into(), urls.ops.clone())],
            _ => Vec::new(),
        };

        v_flex()
            .w_full()
            .gap_5()
            .pb_6()
            .child(head)
            .child(section(t, "状态", state_body))
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(metric_title(&format!("CPU（占整机 {cores} 核）· 近 15 分钟"), cpu_now.map(format::pct)))
                    .child(marks::trend(cpu, t.accent, Some(cpu_top), Vec::new(), format::pct(cpu_top), t)),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(metric_title("内存 · 近 15 分钟", mem_now.map(format::gb)))
                    .child(marks::trend(mem, t.accent, Some(mem_top.max(0.1)), mem_refs, format::gb(mem_top.max(0.1)), t)),
            )
            .child(section(t, "信息", info))
            .when(!log_path.is_empty(), |this| this.child(section(t, "日志文件", log_row)))
            .when(!links.is_empty(), |this| {
                this.child(h_flex().gap_4().children(links.into_iter().enumerate().map(|(k, (label, url))| {
                    Link::new(("sd-link", k)).href(url).child(h_flex().gap_1().items_center().text_sm().child(label).child(Icon::new(IconName::ExternalLink).size_3p5()))
                })))
            })
            .into_any_element()
    }

    pub(crate) fn jobs_sheet(&self, t: &Tokens, _weak: WeakEntity<DashApp>, _cx: &App) -> AnyElement {
        let up = self.st.backend_up();
        let jobs = self.st.backend_jobs.as_ref().filter(|_| up);
        let urls = self.st.snapshot.as_ref().map(|s| s.urls.clone()).unwrap_or_default();
        let Some(j) = jobs else {
            return v_flex().py_6().items_center().child(div().text_sm().text_color(t.muted).child("后端没在跑，没有任务可看。")).into_any_element();
        };
        let rows = v_flex().w_full().children(j.rows.iter().map(|r| {
            let since = SystemTime::now().duration_since(r.since).map(format::duration).unwrap_or_default();
            h_flex()
                .h_10()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(t.line)
                .text_sm()
                .child(div().w_20().font_features(tnum()).child(format!("库 {}", r.dbnum)))
                .child(div().w_16().child(r.intent.clone()))
                .child(div().flex_1().text_color(t.ink2).child(r.phase.clone()))
                .child(div().w_12().text_xs().text_color(if r.state == QueueState::Running { t.accent } else { t.muted }).child(if r.state == QueueState::Running { "在跑" } else { "排队" }))
                .child(div().w_24().text_right().text_xs().text_color(t.muted).font_features(tnum()).child(since))
        }));
        let failures = v_flex().w_full().children(j.failures_24h.iter().map(|f| {
            v_flex()
                .py_2()
                .gap_0p5()
                .border_b_1()
                .border_color(t.line)
                .child(h_flex().gap_2().text_sm().child(div().font_features(tnum()).child(format!("库 {}", f.dbnum))).child(div().text_xs().text_color(t.muted).font_features(tnum()).child(format::date_clock(f.at))))
                .child(div().text_sm().text_color(t.bad_text).child(f.message.clone()))
        }));
        v_flex()
            .w_full()
            .gap_5()
            .pb_6()
            .child(section(t, &format!("在跑 {} · 排队 {} · 积压 {}", j.running(), j.queued(), j.held), rows))
            .child(section(t, &format!("失败 {} · 近 24 小时", j.failures_24h.len()), failures))
            .child(
                Link::new("jobs-ops")
                    .href(urls.ops.clone())
                    .child(h_flex().gap_1().items_center().text_sm().child("打开运维面板（重试、暂停出队、看完整回执）").child(Icon::new(IconName::ExternalLink).size_3p5())),
            )
            .into_any_element()
    }

    pub(crate) fn servers_sheet(&self, t: &Tokens, weak: WeakEntity<DashApp>, _cx: &App) -> AnyElement {
        let on: crate::ui::fleet::OnFleet = std::rc::Rc::new(move |action, window, cx| {
            let _ = weak.update(cx, |app, cx| app.fleet_action(action, window, cx));
        });
        let view = crate::fleet_state::DrawerView {
            rows: self.fleet.drawer_rows(Instant::now()),
            mode: self.drawer.mode,
            errors: self.drawer.errors.clone(),
            test: self.drawer.test.clone(),
            test_endpoint: self.drawer.test_endpoint.clone(),
            notes: self.store_notes.clone(),
        };
        crate::ui::fleet::servers_drawer(t, &view, &self.server_form, self.hover_ui(), &on)
    }
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

impl DashApp {
    /// 生成模型：勾选 + 选库。不勾就绝不生成；勾了只给选中的库生成（= stack init <库号…>）。
    pub(crate) fn generate_sheet(&self, t: &Tokens, weak: WeakEntity<DashApp>, _cx: &App) -> AnyElement {
        let now = Instant::now();
        let backend_up = self.st.backend_up();
        let job = self.st.job.as_ref();
        let generating = job.is_some_and(|j| j.kind == crate::model::JobKind::Init);
        let busy = job.is_some();
        let incr = self.st.snapshot.as_ref().and_then(|s| s.increments_on());
        let enabled = self.gen_checked;

        let w0 = weak.clone();
        let head = Checkbox::new("gen-model-sheet")
            .label("生成模型（缺省不生成）")
            .accessibility_label("生成模型（抽屉里）")
            .checked(enabled)
            .on_click(move |checked: &bool, window, cx| {
                let checked = *checked;
                let _ = w0.update(cx, |app, cx| app.toggle_generate(checked, window, cx));
            });
        let mut explain = String::from("只给勾选的库生成。生成 = stack init：把这个库解析进数据库，并生成它的模型（后端里这是一件事，分不开）；系统库和字典库后端会一并解析，但不生成模型。");
        if incr == Some(false) {
            explain += "增量同步现在关着：生成要走同一条批次队列，生成期间先临时打开，生成完再关回。";
        }

        let mut rows: Vec<&crate::model::DbnumRow> = self.dbnums.iter().filter(|r| r.db_type.is_empty() || r.db_type == "DESI" || r.db_type == "ISOD").collect();
        rows.sort_by_key(|r| r.dbnum);
        let mut list = v_flex().id("gen-list").w_full().max_h(px(470.)).overflow_y_scroll().border_1().border_color(t.line).rounded(t.radius);
        list = list.child(
            h_flex()
                .h(px(30.))
                .px_3()
                .gap_3()
                .items_center()
                .bg(t.soft)
                .text_xs()
                .text_color(t.muted)
                .child(div().w(px(104.)).child("库号"))
                .child(div().w(px(44.)).child("类型"))
                .child(div().flex_1().child("名字（WORLD 下的根）"))
                .child(div().w(px(64.)).text_right().child("大小"))
                .child(div().w(px(96.)).text_right().child("模型")),
        );
        if rows.is_empty() {
            let msg = if backend_up { "正在读库列表…（后端读完工程才有）" } else { "后端没在跑：先一键启动，起好后这里列出 APS 的库" };
            list = list.child(div().p_4().text_sm().text_color(t.muted).child(msg));
        }
        for row in rows {
            let db = row.dbnum;
            let w = weak.clone();
            let in_job = generating && self.generating.contains(&db);
            let state = if in_job && !row.initialized {
                "生成中…".to_string()
            } else if row.initialized {
                match row.pieces {
                    Some(n) => format!("已生成 {n} 件"),
                    None => "已生成".to_string(),
                }
            } else {
                "未生成".to_string()
            };
            let state_color = if in_job { t.accent } else if row.initialized { t.ok } else { t.muted };
            let name = if row.name.is_empty() { format::NONE.to_string() } else { row.name.clone() };
            let size = if row.file_size > 0 { size_text(row.file_size) } else { format::NONE.to_string() };
            let spoken = [format!("库 {db}"), row.db_type.clone(), row.name.clone(), if row.file_size > 0 { size.clone() } else { String::new() }]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            list = list.child(
                h_flex()
                    .h(px(34.))
                    .px_3()
                    .gap_3()
                    .items_center()
                    .border_t_1()
                    .border_color(t.line)
                    .text_sm()
                    .child(
                        div().w(px(104.)).child(
                            Checkbox::new(("gen-db", db as usize))
                                .label(format!("库 {db}"))
                                .accessibility_label(spoken)
                                .checked(self.picked.contains(&db))
                                .disabled(!enabled || row.initialized || busy)
                                .on_click(move |checked: &bool, _, cx| {
                                    let checked = *checked;
                                    let _ = w.update(cx, |app, cx| {
                                        if checked {
                                            app.picked.insert(db);
                                        } else {
                                            app.picked.remove(&db);
                                        }
                                        cx.notify();
                                    });
                                }),
                        ),
                    )
                    .child(div().w(px(44.)).text_xs().text_color(t.muted).child(row.db_type.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_color(t.ink2).child(name))
                    .child(div().w(px(64.)).text_right().text_xs().text_color(t.muted).font_features(tnum()).child(size))
                    .child(div().w(px(96.)).text_right().text_xs().text_color(state_color).font_features(tnum()).child(state)),
            );
        }

        let progress = job.filter(|_| generating).map(|j| {
            let dbs = self.generating.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
            let queue: Vec<String> = self
                .st
                .backend_jobs
                .as_ref()
                .map(|b| b.rows.iter().map(|r| format!("库 {} · {} · {}{}", r.dbnum, r.intent, r.phase, if r.state == QueueState::Running { "（在跑）" } else { "（排队）" })).collect())
                .unwrap_or_default();
            v_flex()
                .w_full()
                .gap_1p5()
                .p_3()
                .rounded(t.radius)
                .bg(t.soft)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(if self.st.frozen(now) {
                            Icon::new(IconName::Pause).size_4().text_color(t.muted).into_any_element()
                        } else {
                            Spinner::new().small().color(t.accent).into_any_element()
                        })
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(format!("正在生成：库 {dbs}")))
                        .child(div().text_xs().text_color(t.muted).font_features(tnum()).child(format!("用时 {}", format::secs1(self.st.clock(now).saturating_duration_since(j.started))))),
                )
                .children(queue.into_iter().take(6).map(|q| div().text_xs().text_color(t.ink2).font_features(tnum()).child(q)))
                .into_any_element()
        });

        let n = self.picked.len();
        let w1 = weak.clone();
        let w2 = weak.clone();
        let footer = h_flex()
            .w_full()
            .justify_end()
            .gap_2()
            .child(Button::new("gen-cancel").ghost().label("不生成，关掉").accessibility_label("不生成，关掉").on_click(move |_, window, cx| {
                let _ = w1.update(cx, |app, cx| app.toggle_generate(false, window, cx));
            }))
            .child(
                Button::new("gen-start")
                    .primary()
                    .icon(IconName::Play)
                    .label(format!("生成 {n} 个库的模型"))
                    .accessibility_label(format!("生成 {n} 个库的模型"))
                    .disabled(!enabled || n == 0 || busy || !backend_up)
                    .on_click(move |_, _, cx| {
                        let _ = w2.update(cx, |app, cx| app.start_generate(cx));
                    }),
            );

        v_flex()
            .size_full()
            .gap_4()
            .pt_1()
            .child(head)
            .child(div().text_xs().text_color(t.muted).child(explain))
            .when_some(progress, |this, p| this.child(p))
            .child(list)
            .child(div().pt_3().pb_4().border_t_1().border_color(t.line).child(footer))
            .into_any_element()
    }
}

fn note(t: &Tokens, icon: IconName, text: &str, color: Hsla) -> AnyElement {
    h_flex()
        .w_full()
        .gap_2()
        .items_start()
        .p_3()
        .rounded(t.radius)
        .bg(t.soft)
        .child(Icon::new(icon).size_4().text_color(color))
        .child(div().flex_1().text_xs().child(text.to_string()))
        .into_any_element()
}
