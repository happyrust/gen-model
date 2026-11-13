//! 部署控制条（顶栏下面一行）：「增量同步」开关、「生成模型」勾选、两个前端的地址。
//! 增量同步以后端的 `queue_paused` 为准；生成模型缺省不勾，勾上才打开选库的抽屉。
//! 每个控件都有读得出来的名字（AccessKit），agent-desktop 能按名字点。
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::model::{JobKind, Svc, SvcState};
use crate::ui::tokens::{Tokens, tnum};

impl DashApp {
    pub(crate) fn render_controls(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.st.snapshot.as_ref();
        let backend_up = self.st.backend_up();
        let incr = snap.and_then(|s| s.increments_on());
        let generating = self.st.job.as_ref().is_some_and(|j| j.kind == JobKind::Init);
        let svc_ok = |svc: Svc| snap.and_then(|s| s.svc(svc)).is_some_and(|x| x.state == SvcState::Ok);
        let urls = snap.map(|s| s.urls.clone()).unwrap_or_default();
        let (incr_text, incr_color) = match incr {
            Some(true) => ("开 · 文件变了就处理", t.ink2),
            Some(false) => ("关 · 批次先攒着不处理", t.muted),
            None => ("后端没在跑", t.muted),
        };
        let generated = self.dbnums.iter().filter(|r| r.initialized).count();
        let gen_text = if generating {
            let dbs = self.generating.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
            format!("生成中：库 {dbs}")
        } else if self.gen_checked {
            format!("已勾 {} 个库", self.picked.len())
        } else if generated > 0 {
            format!("不勾不生成 · 已生成 {generated} 个库")
        } else {
            "缺省不生成".to_string()
        };
        let p3d = urls.frontend.clone();
        let pui = urls.backend.clone();
        let link = |id: &'static str, title: &'static str, url: String, enabled: bool| {
            let u = url.clone();
            Button::new(id)
                .ghost()
                .small()
                .icon(IconName::ExternalLink)
                .label(format!("{title}  {}", if url.is_empty() { "—".to_string() } else { url.trim_end_matches('/').to_string() }))
                .accessibility_label(format!("打开 {title}"))
                .disabled(!enabled || url.is_empty())
                .when(self.hover_ui(), |b| b.tooltip(format!("在浏览器里打开 {title}：{url}")))
                .on_click(move |_, _, cx| cx.open_url(&u))
        };
        h_flex()
            .w_full()
            .h(px(44.))
            .flex_shrink_0()
            .px_4()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(t.line)
            .child(
                Switch::new("incr-switch")
                    .checked(incr == Some(true))
                    .label("增量同步")
                    .accessibility_label("开始增量同步")
                    .disabled(!backend_up || generating || incr.is_none())
                    .when(self.hover_ui(), |s| s.tooltip("开 = 后端处理文件变化（增量）；关 = 队列暂停，变化先攒着。以后端的 queue_paused 为准"))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_increments(*checked, cx))),
            )
            .child(div().text_xs().text_color(incr_color).whitespace_nowrap().child(incr_text))
            .child(div().w(px(1.)).h(px(18.)).bg(t.line).mx_1())
            .child(
                Checkbox::new("gen-model")
                    .label("生成模型")
                    .accessibility_label("生成模型")
                    .checked(self.gen_checked)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| this.toggle_generate(*checked, window, cx))),
            )
            .child(div().text_xs().text_color(if generating { t.accent } else { t.muted }).font_features(tnum()).whitespace_nowrap().child(gen_text))
            .child(div().flex_1())
            .child(link("open-plant3d-web", "plant3d-web", p3d, svc_ok(Svc::Frontend)))
            .child(link("open-plant-ui", "plant-ui", pui, svc_ok(Svc::Backend)))
            .when(!self.driver.is_real(), |this| {
                this.child(h_flex().gap_1().items_center().text_xs().text_color(t.muted).child(Icon::new(IconName::Info).size_3p5()).child("模拟数据"))
            })
            .into_any_element()
    }
}
