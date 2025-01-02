use super::story::Story;
use crate::run_cli;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, rems, AnyElement, FocusHandle, FocusableView, IntoElement, ParentElement, Render,
    SharedString, Styled, Task, Timer, View, ViewContext, VisualContext, WindowContext,
};
use std::borrow::Borrow;
// use tokio::sync::mpsc::{self, Receiver, Sender};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use ui::button::ButtonStyled;
use ui::ContextModal;
use ui::{
    button::{Button, ButtonStyle},
    divider::Divider,
    h_flex,
    input::TextInput,
    label::Label,
    notification::{Notification, NotificationType},
    progress::Progress,
    switch::Switch,
    // tabs::{Tab, Tabs},
    theme::ActiveTheme,
    v_flex,
    Disableable,
    IconName,
    Sizable,
    StyledExt,
};

pub struct ConfigPanelStory {
    focus_handle: FocusHandle,
    parse_all: bool,
    parse_part: bool,
    parse_part_input: View<TextInput>,
    project_path: View<TextInput>,
    included_projects: View<TextInput>,
    project_name: View<TextInput>,
    mdb_name: View<TextInput>,
    db_ip: View<TextInput>,
    db_port: View<TextInput>,
    db_username: View<TextInput>,
    db_password: View<TextInput>,
    generate_all: bool,
    generate_part: bool,
    generate_part_input: View<TextInput>,
    live_update: bool,
    remote_sync: bool,
    active_tab: SharedString,
    progress_value: i32,
    task_running: bool,
    progress_tx: Sender<i32>,
    _progress_task: Option<Task<()>>,
}

impl Story for ConfigPanelStory {
    fn title() -> &'static str {
        "配置面板"
    }

    fn new_view(cx: &mut WindowContext) -> View<impl FocusableView> {
        Self::view(cx)
    }
}

impl ConfigPanelStory {
    pub fn view(cx: &mut WindowContext) -> View<Self> {
        cx.new_view(Self::new)
    }

    fn new(cx: &mut ViewContext<Self>) -> Self {
        let parse_part_input =
            cx.new_view(|cx| TextInput::new(cx).placeholder("请输入数据库文件名, 多个用逗号分隔"));
        let project_path = cx.new_view(|cx| TextInput::new(cx));
        let project_name = cx.new_view(|cx| TextInput::new(cx));
        let included_projects = cx.new_view(|cx| TextInput::new(cx));
        let mdb_name = cx.new_view(|cx| TextInput::new(cx));
        let db_ip = cx.new_view(|cx| TextInput::new(cx).placeholder("127.0.0.1"));
        let db_port = cx.new_view(|cx| TextInput::new(cx).placeholder("8008"));
        let db_username = cx.new_view(|cx| TextInput::new(cx).placeholder("root"));
        let db_password = cx.new_view(|cx| TextInput::new(cx).placeholder("password"));
        let generate_part_input = cx.new_view(|cx| TextInput::new(cx));

        let db_option = get_db_option();
        // Initialize text inputs with values from db_option
        project_path.update(cx, |input, cx| {
            input.set_text(db_option.project_path.clone(), cx)
        });
        project_name.update(cx, |input, cx| {
            input.set_text(db_option.project_name.clone(), cx)
        });
        included_projects.update(cx, |input, cx| {
            input.set_text(db_option.included_projects.join(","), cx)
        });
        mdb_name.update(cx, |input, cx| {
            input.set_text(db_option.mdb_name.clone(), cx)
        });
        db_ip.update(cx, |input, cx| input.set_text(db_option.v_ip.clone(), cx));
        db_port.update(cx, |input, cx| {
            input.set_text(db_option.v_port.to_string(), cx)
        });
        db_username.update(cx, |input, cx| input.set_text(db_option.v_user.clone(), cx));
        db_password.update(cx, |input, cx| {
            input.set_text(db_option.v_password.clone(), cx)
        });

        // Initialize switches
        let live_update = db_option.sync_live.unwrap_or(false);
        let remote_sync = db_option.sync_graph_db.unwrap_or(false);
        let parse_all = db_option.total_sync;
        let parse_part = db_option.incr_sync;
        let (tx, mut rx) = mpsc::channel::<i32>();
        // Spawn progress update task
        let task = cx.spawn(|this, mut cx| async move {
            loop {
                if let Ok(value) = rx.try_recv() {
                    // dbg!(&value);
                    if let Some(this) = this.upgrade() {
                        this.update(&mut cx, |this, cx| {
                            this.progress_value = value as i32;
                            if value == 100 {
                                this.task_running = false;
                            }
                            // this.slider1
                            //     .update(cx, |slider, _| slider.set_value(value, cx));
                            cx.notify();
                        })
                        .ok();
                    }
                    // this.update(cx, |this, _cx| {
                    //     this.progress_value = value;
                    //     if value == 100 {
                    //         this.task_running = false;
                    //     }
                    // });
                    // cx.notify();
                }
                Timer::after(Duration::from_secs(1)).await;
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            parse_all: false,
            parse_part: false,
            parse_part_input,
            project_path,
            project_name,
            mdb_name,
            db_ip,
            db_port,
            db_username,
            db_password,
            generate_all: false,
            generate_part: false,
            generate_part_input,
            live_update: false,
            remote_sync: false,
            active_tab: "parse".into(),
            progress_value: 0,
            progress_tx: tx,
            _progress_task: Some(task),
            task_running: false,
            included_projects,
        }
    }

    fn get_overwrite_config(&self, cx: &mut ViewContext<Self>) -> DbOption {
        let mut db_option = get_db_option().clone();
        db_option.project_path = self.project_path.read(cx).text().to_string();
        db_option.project_name = self.project_name.read(cx).text().to_string();
        db_option.mdb_name = self.mdb_name.read(cx).text().to_string();
        db_option.v_ip = self.db_ip.read(cx).text().to_string();
        db_option.v_port = self.db_port.read(cx).text().parse().unwrap_or(8008);
        db_option.v_user = self.db_username.read(cx).text().to_string();
        db_option.v_password = self.db_password.read(cx).text().to_string();
        db_option.sync_live = Some(self.live_update);
        db_option.sync_graph_db = Some(self.remote_sync);
        db_option.total_sync = self.parse_all;
        db_option.incr_sync = self.parse_part;
        db_option.included_db_files = {
            let text = self.parse_part_input.read(cx).text();
            if text.trim().is_empty() {
                None
            } else {
                Some(text.split(',').map(|s| s.trim().to_string()).collect())
            }
        };
        db_option.gen_model = self.generate_all | self.generate_part;
        db_option.manual_db_nums = {
            let text = self.generate_part_input.read(cx).text();
            if text.trim().is_empty() {
                None
            } else {
                let nums: Vec<u32> = text
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if nums.is_empty() {
                    None
                } else {
                    Some(nums)
                }
            }
        };
        dbg!(&db_option.manual_db_nums);
        db_option
    }

    fn save(&self, cx: &mut ViewContext<Self>) {
        let db_option = self.get_overwrite_config(cx);
        // 将配置写入DbOption.toml文件
        let toml = toml::to_string(&db_option).unwrap();
        std::fs::write("DbOption.toml", toml).unwrap();
    }

    fn render_parse_tab(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(Label::new("解析模块配置").text_lg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("全部重新解析"))
                    .child(
                        Switch::new("parse_all")
                            .checked(self.parse_all)
                            .on_click(cx.listener(|this, checked, _cx| {
                                this.parse_all = *checked;
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(Label::new("部分解析"))
                            .child(Switch::new("parse_part").checked(self.parse_part).on_click(
                                cx.listener(|this, checked, _cx| {
                                    this.parse_part = *checked;
                                }),
                            )),
                    )
                    .when(self.parse_part, |flex| {
                        flex.child(
                            h_flex()
                                .gap_2()
                                .text_size(px(12.0))
                                .pl_4()
                                .child(Label::new("数据库名称"))
                                .child(div().flex_1().child(self.parse_part_input.clone())),
                        )
                    }),
            )
            .child(
                v_flex().gap_2().child(Label::new("项目路径")).child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(self.project_path.clone()))
                        .child(
                            Button::new("path_file_sel")
                                .label("选择")
                                .on_click(cx.listener(|this, _event, cx| {
                                    cx.spawn(|this, mut cx| async move {
                                        if let Some(folder) =
                                            rfd::AsyncFileDialog::new().pick_folder().await
                                        {
                                            let path = folder.path().to_string_lossy().to_string();
                                            cx.update(|cx| {
                                                this.update(cx, |config, cx| {
                                                    config.project_path.update(cx, |input, cx| {
                                                        input.set_text(path, cx);
                                                    });
                                                })
                                                .ok();
                                            })
                                            .ok();
                                        }
                                    })
                                    .detach();
                                }))
                                .style(ButtonStyle::Secondary),
                        ),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("项目名称"))
                    .child(self.project_name.clone()),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("包含项目"))
                    .child(self.included_projects.clone()),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("MDB名称"))
                    .child(self.mdb_name.clone()),
            )
    }

    fn render_database_tab(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(Label::new("数据库配置").text_lg())
            .child(
                v_flex()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("IP地址").w(px(80.)))
                            .child(self.db_ip.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("端口").w(px(80.)))
                            .child(self.db_port.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("用户名").w(px(80.)))
                            .child(self.db_username.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("密码").w(px(80.)))
                            .child(self.db_password.clone()),
                    ),
            )
    }

    fn render_generate_tab(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(Label::new("模型生成配置").text_lg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("全部重新生成"))
                    .child(
                        Switch::new("generate_all")
                            .checked(self.generate_all)
                            .on_click(cx.listener(|this, checked, _cx| {
                                this.generate_all = *checked;
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(Label::new("部分生成"))
                            .child(
                                Switch::new("generate_part")
                                    .checked(self.generate_part)
                                    .on_click(cx.listener(|this, checked, _cx| {
                                        this.generate_part = *checked;
                                    })),
                            ),
                    )
                    .when(self.generate_part, |flex| {
                        flex.child(self.generate_part_input.clone())
                    }),
            )
    }

    fn render_update_tab(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(Label::new("自动增量更新配置").text_lg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("Live更新"))
                    .child(
                        Switch::new("live_update")
                            .checked(self.live_update)
                            .on_click(cx.listener(|this, checked, _cx| {
                                this.live_update = *checked;
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("异地同步"))
                    .child(
                        Switch::new("remote_sync")
                            .checked(self.remote_sync)
                            .on_click(cx.listener(|this, checked, _cx| {
                                this.remote_sync = *checked;
                            })),
                    ),
            )
    }
}

impl FocusableView for ConfigPanelStory {
    fn focus_handle(&self, _: &gpui::AppContext) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfigPanelStory {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .w(px(900.))
            .h(px(800.))
            .bg(theme.background)
            .rounded_lg()
            .shadow_lg()
            .relative() // 添加relative以支持absolute定位
            .child(
                // 侧边栏
                v_flex()
                    .w(px(192.))
                    .border_r(px(1.))
                    .border_color(theme.border)
                    .p_4()
                    .gap_2()
                    .children(
                        vec![
                            ("parse", "解析模块"),
                            ("database", "数据库配置"),
                            ("generate", "模型生成"),
                            ("update", "自动增量更新"),
                        ]
                        .into_iter()
                        .map(|(id, label)| {
                            Button::new(id)
                                .style(if self.active_tab == id {
                                    ButtonStyle::Primary
                                } else {
                                    ButtonStyle::Ghost
                                })
                                .label(label)
                                .on_click(cx.listener(move |this, _event, _cx| {
                                    this.active_tab = id.into();
                                }))
                        }),
                    ),
            )
            .child(
                // 主内容区域
                v_flex()
                    .flex_1()
                    .p_6()
                    .child(match self.active_tab.borrow() {
                        "parse" => self.render_parse_tab(cx).into_any_element(),
                        "database" => self.render_database_tab(cx).into_any_element(),
                        "generate" => self.render_generate_tab(cx).into_any_element(),
                        "update" => self.render_update_tab(cx).into_any_element(),
                        _ => div().into_any_element(),
                    })
                    .child(div().h(px(10.)))
                    .when(self.task_running, |flex| {
                        flex.child(Progress::new().value(self.progress_value as _))
                    }),
            )
            .child(
                // 底部按钮
                h_flex()
                    .absolute()
                    .bottom(px(24.))
                    .right(px(24.))
                    .gap_3()
                    .child(
                        Button::new("cancel")
                            .style(ButtonStyle::Secondary)
                            .label("保存配置")
                            .on_click(cx.listener(|this, _event, cx| {
                                this.save(cx);
                            })),
                    )
                    .child(
                        Button::new("run_task")
                            .style(ButtonStyle::Primary)
                            .disabled(self.task_running)
                            .label("运行任务")
                            .on_click(cx.listener(|this, _event, cx| {
                                let tx = this.progress_tx.clone();
                                this.task_running = true;
                                let db_option = this.get_overwrite_config(cx);
                                // cx.spawn(|this, mut cx| async move {
                                //     if let Err(e) = tx.send(50) {
                                //         dbg!(&e);
                                //     }
                                // })
                                // .detach();

                                // Spawn main task
                                cx.spawn(|this, mut cx| async move {
                                    if let Err(e) = run_cli(db_option, tx).await {
                                        cx.update(|cx| {
                                            cx.push_notification(
                                                Notification::new(e.to_string())
                                                    .with_type(NotificationType::Error)
                                                    .title("任务运行失败"),
                                            );
                                            this.update(cx, |this, _| this.task_running = false);
                                        })
                                        .ok();
                                    }
                                })
                                .detach();
                            })),
                    ),
            )
    }
}
