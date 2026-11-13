//! 选库对话框（DASHBOARD-DESIGN 6.1、6.3）：初始化库、增量更新。必须明确勾选库号，不给「全部」；
//! 增量更新先预览再执行（REVIEW A4：不带库号等于把范围内所有库入队）。
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::DashApp;
use crate::data::Command;
use crate::ui::tokens::{Tokens, tnum};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Init,
    Update,
}

pub struct DialogView {
    app: WeakEntity<DashApp>,
    kind: DialogKind,
    _sub: Subscription,
}

impl DialogView {
    pub fn new(app: &Entity<DashApp>, kind: DialogKind, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(app, |_, _, cx| cx.notify());
        Self { app: app.downgrade(), kind, _sub: sub }
    }
}

impl Render for DialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else { return div().into_any_element() };
        let t = Tokens::new(cx);
        let a = app.read(cx);
        let kind = self.kind;
        let weak = self.app.clone();
        let picked: Vec<u32> = a.picked.iter().copied().collect();
        let preview = a.preview.clone();
        let mut list = v_flex().w_full().gap_2();
        if a.dbnums.is_empty() {
            list = list.child(div().text_sm().text_color(t.muted).child("正在读库列表…"));
        }
        for row in &a.dbnums {
            let selectable = match kind {
                DialogKind::Init => !row.initialized,
                DialogKind::Update => row.initialized,
            };
            let db = row.dbnum;
            let w = weak.clone();
            let note = match (kind, row.initialized) {
                (DialogKind::Init, true) => "已初始化",
                (DialogKind::Update, false) => "没初始化，不能做增量",
                _ => "",
            };
            list = list.child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Checkbox::new(("db", db as usize))
                            .label(format!("库 {db}"))
                            .checked(a.picked.contains(&db))
                            .disabled(!selectable)
                            .on_click(move |checked: &bool, _, cx| {
                                let checked = *checked;
                                let _ = w.update(cx, |app, cx| {
                                    if checked {
                                        app.picked.insert(db);
                                    } else {
                                        app.picked.remove(&db);
                                    }
                                    app.preview = None;
                                    cx.notify();
                                });
                            }),
                    )
                    .when(!note.is_empty(), |this| this.child(div().text_xs().text_color(t.muted).child(note))),
            );
        }

        let body_note = match kind {
            DialogKind::Init => "每个库要解析全部元素并生成模型，大库要几个小时、占用大量内存。超过 10 分钟面板只是不再等，后端里照常跑。",
            DialogKind::Update => "只列已经初始化的库，必须明确勾选；先预览变化，再更新有变化的库。",
        };
        let n = picked.len();
        let changed: Vec<u32> = preview.as_ref().map(|p| p.iter().filter(|(_, c)| *c > 0).map(|(d, _)| *d).collect()).unwrap_or_default();

        let mut footer = h_flex().w_full().justify_end().gap_2().pt_2();
        footer = footer.child(Button::new("dlg-cancel").ghost().label("取消").on_click(|_, window, cx| window.close_dialog(cx)));
        match kind {
            DialogKind::Init => {
                let w = weak.clone();
                let dbs = picked.clone();
                footer = footer.child(Button::new("dlg-init").primary().label(format!("初始化 {n} 个库")).disabled(n == 0).on_click(move |_, window, cx| {
                    let dbs = dbs.clone();
                    let _ = w.update(cx, |app, _| app.cmd(Command::Init(dbs)));
                    window.close_dialog(cx);
                }));
            }
            DialogKind::Update => {
                let w = weak.clone();
                let dbs = picked.clone();
                footer = footer.child(Button::new("dlg-preview").outline().label("预览").disabled(n == 0).on_click(move |_, _, cx| {
                    let dbs = dbs.clone();
                    let _ = w.update(cx, |app, _| app.cmd(Command::PreviewUpdate(dbs)));
                }));
                let w = weak.clone();
                let todo = changed.clone();
                footer = footer.child(Button::new("dlg-update").primary().label(format!("更新 {} 个库", todo.len())).disabled(todo.is_empty()).on_click(move |_, window, cx| {
                    let dbs = todo.clone();
                    let _ = w.update(cx, |app, _| app.cmd(Command::IncrementalUpdate(dbs)));
                    window.close_dialog(cx);
                }));
            }
        }

        v_flex()
            .w_full()
            .gap_4()
            .child(div().text_xs().text_color(t.muted).child(body_note))
            .child(list)
            .when_some(preview, |this, rows| {
                this.child(
                    v_flex().gap_1().p_3().rounded(t.radius).bg(t.soft).children(rows.into_iter().map(|(db, c)| {
                        div().text_sm().font_features(tnum()).child(if c > 0 { format!("库 {db}：{c} 个元素有变化") } else { format!("库 {db}：没有变化，不更新") })
                    })),
                )
            })
            .child(footer)
            .into_any_element()
    }
}
