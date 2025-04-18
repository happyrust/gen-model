use crate::run_cli;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::{
    form::FieldBuilder,
    h_flex,
    input::TextInput,
    label::Label,
    list::List,
    notification::{Notification, NotificationType},
    progress::Progress,
    scroll::ScrollbarShow,
    switch::Switch,
    theme::ActiveTheme,
    v_flex, Disableable, Sizable,
};
use crate::gui::logs::{add_global_log, log_from_thread, LogLevel, LogListDelegate, GLOBAL_LOGS, LogUpdateEvent};
use story::Story;

// 使用gpui_component中的View类型
use gpui_component::form::FieldBuilder::View;

use std::borrow::Borrow;
use std::time::Duration;

// 已经不需要额外的UpdateLogEvent，直接使用logs模块中的LogUpdateEvent
// #[derive(Debug, Clone)]
// pub struct UpdateLogEvent;

// impl EventEmitter<UpdateLogEvent> for ConfigPanelStory {}

pub struct ConfigPanelStory {
    focus_handle: FocusHandle,
    parse_all: bool,
    parse_part: bool,
    parse_part_input: Entity<TextInput>,
    project_path: Entity<TextInput>,
    included_projects: Entity<TextInput>,
    project_name: Entity<TextInput>,
    mdb_name: Entity<TextInput>,
    db_ip: Entity<TextInput>,
    db_port: Entity<TextInput>,
    db_username: Entity<TextInput>,
    db_password: Entity<TextInput>,
    generate_all: bool,
    generate_part: bool,
    generate_part_input: Entity<TextInput>,
    live_update: bool,
    remote_sync: bool,
    active_tab: SharedString,
    show_logs: bool,
    log_list: Entity<List<LogListDelegate>>,
    log_subscription: Option<Subscription>,
    // 移除定时器句柄
    // timer_handle: Option<gpui::Task<()>>,
}

impl Story for ConfigPanelStory {
    fn title() -> &'static str {
        "配置面板"
    }

    fn new_view(window: &mut Window, cx: &mut App) -> Entity<impl Render + Focusable> {
        Self::view(window, cx)
    }
}

impl ConfigPanelStory {
    pub fn view(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx))
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let parse_part_input = cx
            .new(|cx| TextInput::new(window, cx).placeholder("请输入数据库文件名, 多个用逗号分隔"));
        let project_path = cx.new(|cx| TextInput::new(window, cx));
        let project_name = cx.new(|cx| TextInput::new(window, cx));
        let included_projects = cx.new(|cx| TextInput::new(window, cx));
        let mdb_name = cx.new(|cx| TextInput::new(window, cx));
        let db_ip = cx.new(|cx| TextInput::new(window, cx).placeholder("127.0.0.1"));
        let db_port = cx.new(|cx| TextInput::new(window, cx).placeholder("8008"));
        let db_username = cx.new(|cx| TextInput::new(window, cx).placeholder("root"));
        let db_password = cx.new(|cx| TextInput::new(window, cx).placeholder("password"));
        let generate_part_input = cx.new(|cx| TextInput::new(window, cx));

        // 创建日志列表
        let delegate = LogListDelegate::new();
        let log_list = cx.new(|cx| {
            List::new(delegate, window, cx)
        });

        let db_option = get_db_option();
        // Initialize text inputs with values from db_option
        project_path.update(cx, |input, cx| {
            input.set_text(db_option.project_path.clone(), window, cx)
        });
        project_name.update(cx, |input, cx| {
            input.set_text(db_option.project_name.clone(), window, cx)
        });
        included_projects.update(cx, |input, cx| {
            input.set_text(db_option.included_projects.join(","), window, cx)
        });
        mdb_name.update(cx, |input, cx| {
            input.set_text(db_option.mdb_name.clone(), window, cx)
        });
        db_ip.update(cx, |input, cx| {
            input.set_text(db_option.v_ip.clone(), window, cx)
        });
        db_port.update(cx, |input, cx| {
            input.set_text(db_option.v_port.to_string(), window, cx)
        });
        db_username.update(cx, |input, cx| {
            input.set_text(db_option.v_user.clone(), window, cx)
        });
        db_password.update(cx, |input, cx| {
            input.set_text(db_option.v_password.clone(), window, cx)
        });

        // Initialize switches
        let live_update = db_option.sync_live.unwrap_or(false);
        let remote_sync = db_option.sync_graph_db.unwrap_or(false);
        let parse_all = db_option.total_sync;
        let parse_part = db_option.incr_sync;

        let instance = Self {
            focus_handle: cx.focus_handle(),
            parse_all,
            parse_part,
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
            live_update,
            remote_sync,
            active_tab: "parse".into(),
            included_projects,
            show_logs: true,
            log_list,
            log_subscription: None,
            // 移除定时器句柄
            // timer_handle: None,
        };

        instance
    }

    const ID: usize = 0;

    /// 获取覆盖配置
    fn get_overwrite_config(&self, cx: &mut Context<Self>) -> DbOption {
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
                let parsed_nums: Vec<u32> = text
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if parsed_nums.is_empty() {
                    None
                } else {
                    Some(parsed_nums)
                }
            }
        };

        if db_option.manual_db_nums.is_some() {
            dbg!(&db_option.manual_db_nums);
        }
        db_option
    }

    fn save(&self, cx: &mut Context<Self>) {
        let db_option = self.get_overwrite_config(cx);
        // 将配置写入DbOption.toml文件
        let toml = toml::to_string(&db_option).unwrap();
        std::fs::write("DbOption.toml", toml).unwrap();
    }

    // 添加更新日志的方法
    fn update_logs(&mut self, cx: &mut Context<Self>) {
        if let Ok(logs) = GLOBAL_LOGS.lock() {
            if logs.is_empty() {
                return;
            }
            
            self.log_list.update(cx, |list, cx| {
                let mut delegate = list.delegate_mut();
                let current_count = delegate.logs.len();
                
                // 只添加新日志
                if current_count < logs.len() {
                    for i in current_count..logs.len() {
                        if let Some(log) = logs.get(i) {
                            delegate.logs.push(log.clone());
                        }
                    }
                }
            });
        }
    }

    // 添加示例日志的方法（用于测试）
    fn add_example_logs(&mut self, cx: &mut Context<Self>) {
        // 添加一些示例日志
        add_global_log("初始化应用程序...".to_string(), LogLevel::Info);
        add_global_log("正在加载配置...".to_string(), LogLevel::Info);
        add_global_log("部分配置文件缺失".to_string(), LogLevel::Warning);
        add_global_log("正在连接数据库...".to_string(), LogLevel::Info);
        add_global_log("数据库连接失败，尝试重连".to_string(), LogLevel::Error);
        add_global_log("重新连接成功".to_string(), LogLevel::Info);
        
        // 直接更新日志列表（无需通过事件，因为已在同一上下文中）
        self.update_logs(cx);
    }

    fn render_parse_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                            .on_click(cx.listener(|this, checked, window, cx| {
                                this.parse_all = *checked;
                                this.notify(cx);
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
                                cx.listener(|this, checked, window, cx| {
                                    this.parse_part = *checked;
                                    this.notify(cx);
                                }),
                            )),
                    )
                    .when(self.parse_part, |flex| {
                        flex.child(
                            h_flex()
                                .gap_2()
                                .w_full()
                                .text_size(px(12.0))
                                .pl_4()
                                .child(Label::new("数据库名称"))
                                .child(self.parse_part_input.clone()),
                        )
                    }),
            )
            .child(
                v_flex().gap_2().child(Label::new("项目路径")).child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(self.project_path.clone())
                        .child(
                            Button::new("path_file_sel")
                                .label("选择")
                                .w(px(60.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    // 使用rfd库打开目录选择对话框
                                    cx.spawn_in(window, async move |this, mut cx| {
                                        if let Some(folder) =
                                            rfd::AsyncFileDialog::new().pick_folder().await
                                        {
                                            let path = folder.path().to_string_lossy().to_string();
                                            // cx.spawn_in(window, |cx| {
                                                this.update_in(cx, |config, window, cx| {
                                                    config.project_path.update(cx, |input, cx| {
                                                        input.set_text(path, window, cx);
                                                    });
                                                });
                                        }
                                    })
                                    .detach()
                                })),
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

    fn render_database_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    fn render_generate_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                            .on_click(cx.listener(|this, checked, window, cx| {
                                this.generate_all = *checked;
                                this.notify(cx);
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
                                    .on_click(cx.listener(|this, checked, window, cx| {
                                        this.generate_part = *checked;
                                        this.notify(cx);
                                    })),
                            ),
                    )
                    .when(self.generate_part, |flex| {
                        flex.child(self.generate_part_input.clone())
                    }),
            )
    }

    fn render_update_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                            .on_click(cx.listener(|this, checked, window, cx| {
                                this.live_update = *checked;
                                this.notify(cx);
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
                            .on_click(cx.listener(|this, checked, window, cx| {
                                this.remote_sync = *checked;
                                this.notify(cx);
                            })),
                    ),
            )
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        cx.notify()
    }
}

impl Focusable for ConfigPanelStory {
    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfigPanelStory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border.clone();
        let radius = theme.radius.clone();
        
        // 首次渲染时，添加示例日志
        if self.log_subscription.is_none() {
            // 添加示例日志
            // self.add_example_logs(cx);
            
            // 标记为已初始化，避免重复添加示例日志
            self.log_subscription = None;
        }
        
        // 每次渲染时检查是否有新日志
        // self.update_logs(cx);

        div().p_4().size_full().child(
            h_flex()
                .size_full()
                .bg(theme.background)
                .rounded_lg()
                .shadow_lg()
                .relative() // 添加relative以支持absolute定位
                .child(
                    // 侧边栏
                    v_flex()
                        .w(px(192.))
                        .border_r(px(1.))
                        .border_color(border)
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
                                let btn = Button::new(id).label(label);
                                let btn = if self.active_tab == id { btn } else { btn };

                                btn.on_click(cx.listener(move |this, _, window, cx| {
                                    this.active_tab = id.into();
                                    this.notify(cx);
                                }))
                            }),
                        )
                        .child(
                            v_flex()
                                .gap_4()
                                .mt_6()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(Label::new("显示日志"))
                                        .child(
                                            Switch::new("show_logs")
                                                .checked(self.show_logs)
                                                .on_click(cx.listener(|this, checked, window, cx| {
                                                    this.show_logs = *checked;
                                                    this.notify(cx);
                                                })),
                                        ),
                                )
                        ),
                )
                .child(
                    // 主内容区域
                    v_flex()
                        .flex_1()
                        .p_6()
                        .child(match self.active_tab.borrow() {
                            "parse" => self.render_parse_tab(window, cx).into_any_element(),
                            "database" => self.render_database_tab(window, cx).into_any_element(),
                            "generate" => self.render_generate_tab(window, cx).into_any_element(),
                            "update" => self.render_update_tab(window, cx).into_any_element(),
                            _ => div().into_any_element(),
                        })
                )
                .when(self.show_logs, |flex| {
                    // 添加日志查看区域（右侧）
                    flex.child(
                        v_flex()
                            .w(px(350.))
                            .border_l(px(1.))
                            .border_color(border)
                            .child(
                                v_flex()
                                    .p_2()
                                    .size_full()
                                    .child(Label::new("日志输出").text_lg().text_center())
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .border(px(1.))
                                            .border_color(border)
                                            .rounded(radius)
                                            .child(self.log_list.clone())
                                    )
                                    .child(
                                        h_flex()
                                            .justify_end()
                                            .mt_2()
                                            .child(
                                                Button::new("clear_logs")
                                                    .label("清空日志")
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        // 清空日志
                                                        if let Ok(mut logs) = GLOBAL_LOGS.lock() {
                                                            logs.clear();
                                                        }
                                                        this.log_list.update(cx, |list, cx| {
                                                            let mut delegate = list.delegate_mut();
                                                            delegate.logs.clear();
                                                            delegate.selected_index = None;
                                                        });
                                                    }))
                                            )
                                    )
                            )
                    )
                })
                .child(
                    // 底部按钮
                    h_flex()
                        .absolute()
                        .bottom(px(24.))
                        .right(px(24.))
                        .gap_3()
                        // 添加执行按钮
                        .child(
                            Button::new("execute_button")
                                .label("开始执行")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    // 保存当前配置
                                    // this.save(cx);

                                    let db_option = this.get_overwrite_config(cx);
                                    
                                    // 添加执行开始日志
                                    add_global_log("开始执行任务...", LogLevel::Info);
                                    // 立即更新日志显示
                                    this.update_logs(cx);
                                    
                                    // 启动任务
                                    cx.spawn(async move |_cx, _window| {
                                        match crate::run_app(Some(db_option)).await {
                                            Ok(_) => {
                                                log_from_thread("执行成功！", LogLevel::Info);
                                            },
                                            Err(e) => {
                                                log_from_thread(format!("执行出错: {}", e), LogLevel::Error);
                                            },
                                        }
                                    }).detach();
                                })),
                        )
                        .child(
                            Button::new("save_config")
                                .label("保存配置")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.save(cx);
                                    add_global_log("配置已保存", LogLevel::Info);
                                    // 立即更新日志显示
                                    this.update_logs(cx);
                                })),
                        ),
                ),
        )
    }
}
