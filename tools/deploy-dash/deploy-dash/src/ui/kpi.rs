//! 四个指标磁贴（DASHBOARD-DESIGN 4.2 C）：服务、后端任务、磁盘、内存。只报数，不驱动图表；
//! 四个磁贴内容量一样：标签行、大数、标记行、底部行。宽屏另有右侧的后端任务栏。
use std::time::{Instant, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::model::{QueueState, Svc};
use crate::state::{Overall, RowKind, Sev};
use crate::ui::format;
use crate::ui::marks;
use crate::ui::tokens::{Tokens, tnum};

pub const TILE_H: f32 = 120.0;

struct Tile {
    label: String,
    meta: String,
    value: String,
    suffix: String,
    mark: AnyElement,
    foot_left: AnyElement,
    foot_right: AnyElement,
    /// 解释放在提示里，磁贴上不写句子（skill「无字」）。
    tip: String,
}

fn tile(t: &Tokens, id: &'static str, tile: Tile, hover: bool) -> AnyElement {
    let tip = tile.tip.clone();
    v_flex()
        .id(id)
        .when(hover, |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
        .flex_1()
        .min_w_0()
        .h(px(TILE_H))
        .px_4()
        .py_3()
        .gap_1()
        .border_1()
        .border_color(t.line)
        .rounded(t.radius_lg)
        .bg(t.page)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(div().text_sm().text_color(t.ink2).whitespace_nowrap().child(tile.label))
                .child(div().text_xs().text_color(t.muted).whitespace_nowrap().font_features(tnum()).child(tile.meta)),
        )
        .child(
            h_flex()
                .items_baseline()
                .gap_1p5()
                .child(div().text_3xl().line_height(px(36.)).font_weight(FontWeight::MEDIUM).font_features(tnum()).whitespace_nowrap().child(tile.value))
                .child(div().text_sm().text_color(t.muted).whitespace_nowrap().truncate().child(tile.suffix)),
        )
        .child(div().w_full().h(px(18.)).child(tile.mark))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .text_xs()
                .child(div().min_w_0().overflow_hidden().child(tile.foot_left))
                .child(div().flex_shrink_0().child(tile.foot_right)),
        )
        .into_any_element()
}

fn flag(t: &Tokens, sev: Sev, text: String) -> AnyElement {
    let (icon, color) = match sev {
        Sev::Red => (IconName::CircleX, t.bad_text),
        Sev::Amber => (IconName::TriangleAlert, t.warn_text),
    };
    h_flex()
        .gap_1()
        .items_center()
        .text_color(color)
        .child(Icon::new(icon).size_3p5())
        .child(div().whitespace_nowrap().truncate().child(text))
        .into_any_element()
}

fn muted(t: &Tokens, text: impl Into<SharedString>) -> AnyElement {
    div().text_color(t.muted).whitespace_nowrap().truncate().font_features(tnum()).child(text.into()).into_any_element()
}

impl DashApp {
    pub(crate) fn render_kpis(&self, t: &Tokens, now: Instant, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .gap_3()
            .child(self.tile_services(t, now))
            .child(self.tile_jobs(t, now, wide, cx))
            .child(self.tile_disk(t, now))
            .child(self.tile_memory(t, now))
            .into_any_element()
    }

    fn tile_services(&self, t: &Tokens, now: Instant) -> AnyElement {
        let overall = self.st.overall(now);
        let (ok, enabled) = self.st.counts();
        let n = 30;
        let per: Vec<Vec<Option<RowKind>>> = Svc::ALL.iter().map(|s| self.st.rows[s.index()].buckets(now, n)).collect();
        let fleet: Vec<Option<RowKind>> = (0..n)
            .map(|i| {
                per.iter().filter_map(|b| b[i]).fold(None, |acc: Option<RowKind>, k| match acc {
                    Some(a) if a.weight() >= k.weight() => Some(a),
                    _ => Some(k),
                })
            })
            .collect();
        let bad: Vec<Svc> = Svc::ALL.iter().copied().filter(|s| self.st.row_kind(*s) == RowKind::Bad).collect();
        let (value, suffix) = match overall {
            Overall::Unconfigured => (format::NONE.to_string(), "未配置".to_string()),
            Overall::Loading => (format::NONE.to_string(), String::new()),
            _ => (format!("{ok} / {enabled}"), "正常".to_string()),
        };
        let foot_left = if let Some(first) = bad.first() {
            let text = if bad.len() > 1 { format!("{} 等 {} 个异常", first.title(), bad.len()) } else { format!("{} 异常", first.title()) };
            flag(t, Sev::Red, text)
        } else {
            match overall {
                Overall::AllOk => muted(t, "全部正常"),
                Overall::Stopped => muted(t, "已停止"),
                Overall::Unconfigured => muted(t, "还没有服务"),
                Overall::Starting => muted(t, "启动中"),
                Overall::Stopping => muted(t, "停止中"),
                _ => muted(t, ""),
            }
        };
        let uptime = self.st.stack_uptime().map(|d| format!("运行 {}", format::duration(d))).unwrap_or_default();
        tile(
            t,
            "tile-services",
            Tile {
                label: "服务".into(),
                meta: "近 15 分钟".into(),
                value,
                suffix,
                mark: marks::ticks(fleet, t).into_any_element(),
                foot_left,
                foot_right: muted(t, uptime),
                tip: "正常数 / 启用数。刻度是近 15 分钟全套最差的状态，每格 30 秒；空心 = 那段时间面板还没开".into(),
            },
            self.hover_ui(),
        )
    }

    fn tile_jobs(&self, t: &Tokens, now: Instant, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let up = self.st.backend_up();
        let jobs = self.st.backend_jobs.as_ref().filter(|_| up);
        let series = self.st.queue.buckets(now, 90);
        let (value, suffix, foot_left, foot_right) = match jobs {
            Some(j) => {
                let running = j.running();
                let queued = j.queued();
                let top = j.rows.iter().find(|r| r.state == QueueState::Running);
                let left = if j.paused {
                    muted(t, "已暂停出队")
                } else if wide {
                    muted(t, format!("积压 {}", j.held))
                } else {
                    match top {
                        Some(r) => muted(t, format!("库 {} · {} · {}", r.dbnum, r.intent, r.phase)),
                        None => muted(t, "空闲"),
                    }
                };
                let right = if j.failures_24h.is_empty() {
                    muted(t, "无失败")
                } else {
                    div()
                        .id("tile-failures")
                        .cursor_pointer()
                        .rounded(t.radius)
                        .px_1()
                        .when(self.hover_ui(), |this| this.hover(|s| s.bg(t.soft)))
                        .child(flag(t, Sev::Red, format!("失败 {} · 看失败", j.failures_24h.len())))
                        .on_click(cx.listener(|this, _, window, cx| this.open_jobs_sheet(window, cx)))
                        .into_any_element()
                };
                (running.to_string(), format!("在跑 · {queued} 排队"), left, right)
            }
            None => (format::NONE.to_string(), "后端没在跑".to_string(), muted(t, ""), muted(t, "")),
        };
        tile(
            t,
            "tile-jobs",
            Tile {
                label: "后端任务".into(),
                meta: "失败数近 24 小时".into(),
                value,
                suffix,
                mark: marks::spark(series, t.spark, None, Vec::new()).into_any_element(),
                foot_left,
                foot_right,
                tip: "后端队列里在跑和排队的批次（积压的不算）；折线是近 15 分钟的队列长度；失败数来自后端落盘的失败记录，后端重启后也还在".into(),
            },
            self.hover_ui(),
        )
    }

    fn tile_disk(&self, t: &Tokens, now: Instant) -> AnyElement {
        let d = self.st.disk_eval(now);
        let series = self.st.disk.buckets(now, 90);
        let color = match d.sev {
            Some(Sev::Red) => t.bad,
            Some(Sev::Amber) => t.warn_text,
            None => t.spark,
        };
        let foot_left = match (d.sev, d.minutes_to_red) {
            (Some(Sev::Red), _) => flag(t, Sev::Red, "低于红线".into()),
            (Some(Sev::Amber), Some(m)) => flag(t, Sev::Amber, format!("约 {m} 分钟到红线")),
            (Some(Sev::Amber), None) => flag(t, Sev::Amber, format!("不到 {:.0} GB", d.amber_below)),
            (None, _) => {
                // 历史不满 15 分钟时照实写「近 N 分钟」；不到 1 分钟不下结论
                let m = d.span_minutes.round().min(15.0) as u32;
                match d.delta_15m {
                    _ if m < 1 => muted(t, ""),
                    Some(delta) if delta.abs() >= 0.1 => muted(t, format!("近 {m} 分钟 {}{:.1} GB", if delta < 0.0 { "−" } else { "+" }, delta.abs())),
                    Some(_) => muted(t, format!("近 {m} 分钟稳定")),
                    None => muted(t, ""),
                }
            }
        };
        let used = match d.free {
            Some(f) if d.total > 0.0 => format!("已用 {:.0}%", (1.0 - f / d.total) * 100.0),
            _ => String::new(),
        };
        let sys = self.st.metrics.as_ref().map(|m| format!("；系统盘 {} 可用 {}（红线 1.5 GB）", m.sys_disk.drive, format::gb(m.sys_disk.free_gb))).unwrap_or_default();
        let tip = format!("运行目录所在盘的可用空间；虚线是红线（[guard] min_disk_free_gb）{sys}；「多久到红线」按近 15 分钟的下降速度算，下降不到 1 GB 时不算");
        tile(
            t,
            "tile-disk",
            Tile {
                label: if d.drive.is_empty() { "磁盘".into() } else { format!("磁盘 {}", d.drive) },
                meta: format!("红线 {}", format::gb(d.red_line)),
                value: d.free.map(format::gb).unwrap_or_else(|| format::NONE.into()),
                suffix: "可用".into(),
                mark: marks::spark(series, color, None, vec![(d.red_line, t.bad.opacity(0.7))]).into_any_element(),
                foot_left,
                foot_right: muted(t, used),
                tip,
            },
            self.hover_ui(),
        )
    }

    fn tile_memory(&self, t: &Tokens, now: Instant) -> AnyElement {
        let m = self.st.mem_eval();
        let series = self.st.commit.buckets(now, 90);
        let color = match m.sev {
            Some(Sev::Red) => t.bad,
            Some(Sev::Amber) => t.warn_text,
            None => t.spark,
        };
        let mut refs = Vec::new();
        if m.start_line > 0.0 {
            refs.push((m.start_line, t.warn_text.opacity(0.7)));
        }
        if m.abort_line > 0.0 {
            refs.push((m.abort_line, t.bad.opacity(0.7)));
        }
        let meta = if m.start_line > 0.0 { format!("开工线 {}", format::gb(m.start_line)) } else { "没配护栏线".to_string() };
        let foot_left = match m.backend_gb {
            Some(b) if m.backend_over => flag(t, Sev::Red, format!("后端 {}（上限 {}）", format::gb(b), format::gb(m.backend_max))),
            Some(b) => muted(t, format!("后端 {}", format::gb(b))),
            None => muted(t, "后端没在跑"),
        };
        let total = if m.mem_total > 0.0 { format!("全机 {:.0} GB", m.mem_total) } else { String::new() };
        let _ = now;
        tile(
            t,
            "tile-memory",
            Tile {
                label: "内存（可提交）".into(),
                meta,
                value: m.commit.map(format::gb).unwrap_or_else(|| format::NONE.into()),
                suffix: "余量".into(),
                mark: marks::spark(series, color, None, refs).into_any_element(),
                foot_left,
                foot_right: muted(t, total),
                tip: "可提交内存余量（和 stack.ps1 用的 FreeVirtualMemory 同一个量）；虚线是开工线和收摊线（[guard] min_commit_free_gb / abort_commit_free_gb）".into(),
            },
            self.hover_ui(),
        )
    }

    /// 宽屏（≥1680）右侧的后端任务栏：在跑、排队、最近的失败，最多 5 行。
    pub(crate) fn render_jobs_panel(&self, t: &Tokens, _now: Instant, cx: &mut Context<Self>) -> AnyElement {
        let up = self.st.backend_up();
        let jobs = self.st.backend_jobs.as_ref().filter(|_| up);
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(j) = jobs {
            for r in j.rows.iter().take(3) {
                let since = SystemTime::now().duration_since(r.since).map(format::duration_short).unwrap_or_default();
                let state = if r.state == QueueState::Running { "在跑" } else { "排队" };
                rows.push(
                    h_flex()
                        .h_10()
                        .px_3()
                        .gap_2()
                        .items_center()
                        .border_t_1()
                        .border_color(t.line)
                        .text_sm()
                        .child(div().w_16().font_features(tnum()).child(format!("库 {}", r.dbnum)))
                        .child(div().flex_1().min_w_0().truncate().text_color(t.ink2).child(format!("{} · {}", r.intent, r.phase)))
                        .child(div().text_xs().text_color(if r.state == QueueState::Running { t.accent } else { t.muted }).child(state))
                        .child(div().w_12().text_xs().text_color(t.muted).text_right().font_features(tnum()).child(since))
                        .into_any_element(),
                );
            }
            for f in j.failures_24h.iter().take(2) {
                rows.push(
                    h_flex()
                        .h_10()
                        .px_3()
                        .gap_2()
                        .items_center()
                        .border_t_1()
                        .border_color(t.line)
                        .text_sm()
                        .child(div().w_16().font_features(tnum()).child(format!("库 {}", f.dbnum)))
                        .child(div().flex_1().min_w_0().truncate().text_color(t.bad_text).child(f.message.clone()))
                        .child(div().text_xs().text_color(t.muted).font_features(tnum()).child(format::clock(f.at)))
                        .into_any_element(),
                );
            }
        }
        let empty = rows.is_empty();
        v_flex()
            .w_full()
            .h(px(crate::ui::services::TABLE_H))
            .border_1()
            .border_color(t.line)
            .rounded(t.radius_lg)
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(34.))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .bg(t.soft)
                    .text_xs()
                    .text_color(t.muted)
                    .child("后端任务")
                    .child(Button::new("jobs-all").ghost().xsmall().label("全部…").on_click(cx.listener(|this, _, window, cx| this.open_jobs_sheet(window, cx)))),
            )
            .children(rows)
            .when(empty, |this| this.child(div().flex_1().flex().items_center().justify_center().text_sm().text_color(t.muted).child(if up { "空闲" } else { "后端没在跑" })))
            .into_any_element()
    }
}
