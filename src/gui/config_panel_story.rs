use crate::gui::config_manager::{ConfigManager, ConfigSite};
use crate::gui::logs::{
    GLOBAL_LOGS, LogLevel, LogListDelegate, LogUpdateEvent, add_global_log, log_from_thread,
};
use crate::options::{DbOptionExt, get_db_option_ext};
use crate::run_cli;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::{
    Disableable, Sizable,
    form::FieldBuilder,
    h_flex,
    input::{InputState, TextInput},
    label::Label,
    list::List,
    notification::{Notification, NotificationType},
    progress::Progress,
    scroll::ScrollbarShow,
    switch::Switch,
    theme::ActiveTheme,
    v_flex,
};
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
    parse_part_input: Entity<InputState>,
    project_path: Entity<InputState>,
    included_projects: Entity<InputState>,
    project_name: Entity<InputState>,
    mdb_name: Entity<InputState>,
    db_ip: Entity<InputState>,
    db_port: Entity<InputState>,
    db_username: Entity<InputState>,
    db_password: Entity<InputState>,
    generate_all: bool,
    generate_part: bool,
    generate_part_input: Entity<InputState>,
    live_update: bool,
    remote_sync: bool,
    active_tab: SharedString,
    show_logs: bool,
    log_list: Entity<List<LogListDelegate>>,
    log_subscription: Option<Subscription>,
    // 添加运行状态标志
    is_running: bool,
    // 异地部署页面相关属性
    mqtt_server: Entity<InputState>,
    mqtt_port: Entity<InputState>,
    http_server: Entity<InputState>,
    http_port: Entity<InputState>,
    mqtt_running: bool,
    http_running: bool,
    // 配置管理
    config_manager: ConfigManager,
    current_site_id: Option<String>,
    available_sites: Vec<ConfigSite>,
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
        // 初始化配置管理器
        let config_manager = ConfigManager::new().unwrap_or_else(|e| {
            add_global_log(&format!("配置管理器初始化失败: {}", e), LogLevel::Error);
            ConfigManager::new().unwrap() // 重试一次
        });

        // 加载可用站点
        let available_sites = config_manager.list_sites().unwrap_or_default();

        let parse_part_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("请输入数据库文件名, 多个用逗号分隔")
        });
        let project_path = cx.new(|cx| InputState::new(window, cx));
        let project_name = cx.new(|cx| InputState::new(window, cx));
        let included_projects = cx.new(|cx| InputState::new(window, cx));
        let mdb_name = cx.new(|cx| InputState::new(window, cx));
        let db_ip = cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1"));
        let db_port = cx.new(|cx| InputState::new(window, cx).placeholder("8009"));
        let db_username = cx.new(|cx| InputState::new(window, cx).placeholder("root"));
        let db_password = cx.new(|cx| InputState::new(window, cx).placeholder("root"));
        let generate_part_input = cx.new(|cx| InputState::new(window, cx));

        // 异地部署页面相关输入框
        let mqtt_server = cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.100"));
        let mqtt_port = cx.new(|cx| InputState::new(window, cx).placeholder("1883"));
        let http_server = cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.100"));
        let http_port = cx.new(|cx| InputState::new(window, cx).placeholder("8080"));

        // 创建日志列表
        let delegate = LogListDelegate::new();
        let log_list = cx.new(|cx| List::new(delegate, window, cx));

        let db_option = get_db_option_ext();

        // Initialize text inputs with values from db_option
        project_path.update(cx, |input, cx| {
            input.set_value(db_option.project_path.clone(), window, cx)
        });
        project_name.update(cx, |input, cx| {
            input.set_value(db_option.project_name.clone(), window, cx)
        });
        included_projects.update(cx, |input, cx| {
            input.set_value(db_option.included_projects.join(","), window, cx)
        });
        mdb_name.update(cx, |input, cx| {
            input.set_value(db_option.mdb_name.clone(), window, cx)
        });
        db_ip.update(cx, |input, cx| {
            input.set_value(db_option.v_ip.clone(), window, cx)
        });
        db_port.update(cx, |input, cx| {
            input.set_value(db_option.v_port.to_string(), window, cx)
        });
        db_username.update(cx, |input, cx| {
            input.set_value(db_option.v_user.clone(), window, cx)
        });
        db_password.update(cx, |input, cx| {
            input.set_value(db_option.v_password.clone(), window, cx)
        });

        // 初始化异地部署相关输入框
        if let Some(server) = &db_option.mqtt_server {
            mqtt_server.update(cx, |input, cx| input.set_value(server.clone(), window, cx));
        }
        if let Some(port) = db_option.mqtt_port {
            mqtt_port.update(cx, |input, cx| {
                input.set_value(port.to_string(), window, cx)
            });
        }
        if let Some(server) = &db_option.http_server {
            http_server.update(cx, |input, cx| input.set_value(server.clone(), window, cx));
        }
        if let Some(port) = db_option.http_port {
            http_port.update(cx, |input, cx| {
                input.set_value(port.to_string(), window, cx)
            });
        }

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
            // 添加运行状态初始值为false
            is_running: false,
            // 异地部署页面相关属性
            mqtt_server,
            mqtt_port,
            http_server,
            http_port,
            mqtt_running: false,
            http_running: false,
            // 配置管理
            config_manager,
            current_site_id: None,
            available_sites,
            // 移除定时器句柄
            // timer_handle: None,
        };

        instance
    }

    const ID: usize = 0;

    /// 获取覆盖配置
    fn get_overwrite_config(&self, cx: &mut Context<Self>) -> DbOptionExt {
        let mut db_option = get_db_option_ext();

        db_option.project_path = self.project_path.read(cx).value().to_string();
        db_option.project_name = self.project_name.read(cx).value().to_string();
        db_option.mdb_name = self.mdb_name.read(cx).value().to_string();
        db_option.v_ip = self.db_ip.read(cx).value().to_string();
        db_option.v_port = self.db_port.read(cx).value().parse().unwrap_or(8008);
        db_option.v_user = self.db_username.read(cx).value().to_string();
        db_option.v_password = self.db_password.read(cx).value().to_string();

        db_option.sync_live = Some(self.live_update);
        db_option.sync_graph_db = Some(self.remote_sync);
        db_option.total_sync = self.parse_all;
        db_option.incr_sync = self.parse_part;

        // 添加异地部署设置
        if self.remote_sync {
            // 添加mqtt服务器配置
            db_option.mqtt_server = Some(self.mqtt_server.read(cx).value().to_string());
            db_option.mqtt_port = self.mqtt_port.read(cx).value().parse().ok();

            // 添加http服务器配置
            db_option.http_server = Some(self.http_server.read(cx).value().to_string());
            db_option.http_port = self.http_port.read(cx).value().parse().ok();
        } else {
            // 如果未启用异地同步，清空相关配置
            db_option.mqtt_server = None;
            db_option.mqtt_port = None;
            db_option.http_server = None;
            db_option.http_port = None;
        }

        db_option.included_db_files = {
            let text = self.parse_part_input.read(cx).value();
            if text.trim().is_empty() {
                None
            } else {
                Some(text.split(',').map(|s| s.trim().to_string()).collect())
            }
        };

        db_option.gen_model = self.generate_all | self.generate_part;

        db_option.manual_db_nums = {
            let text = self.generate_part_input.read(cx).value();
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

    /// 保存当前配置到文件
    fn save_current_config(&mut self, cx: &mut Context<Self>) {
        let config = self.get_overwrite_config(cx);

        match self.config_manager.save_to_current(&config) {
            Ok(_) => {
                add_global_log("配置已保存到 DbOption.toml", LogLevel::Info);
            }
            Err(e) => {
                add_global_log(&format!("保存配置失败: {}", e), LogLevel::Error);
            }
        }
    }

    /// 从文件加载配置
    fn load_config_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match ConfigManager::load_from_current() {
            Ok(config) => {
                self.apply_config(config, window, cx);
                add_global_log("配置已从 DbOption.toml 加载", LogLevel::Info);
            }
            Err(e) => {
                add_global_log(&format!("加载配置失败: {}", e), LogLevel::Error);
            }
        }
    }

    /// 应用配置到界面
    fn apply_config(&mut self, config: DbOptionExt, window: &mut Window, cx: &mut Context<Self>) {
        self.project_path.update(cx, |input, cx| {
            input.set_value(config.project_path.clone(), window, cx)
        });
        self.project_name.update(cx, |input, cx| {
            input.set_value(config.project_name.clone(), window, cx)
        });
        self.mdb_name.update(cx, |input, cx| {
            input.set_value(config.mdb_name.clone(), window, cx)
        });
        self.db_ip.update(cx, |input, cx| {
            input.set_value(config.v_ip.clone(), window, cx)
        });
        self.db_port.update(cx, |input, cx| {
            input.set_value(config.v_port.to_string(), window, cx)
        });
        self.db_username.update(cx, |input, cx| {
            input.set_value(config.v_user.clone(), window, cx)
        });
        self.db_password.update(cx, |input, cx| {
            input.set_value(config.v_password.clone(), window, cx)
        });

        self.parse_all = config.total_sync;
        self.parse_part = config.incr_sync;
        self.live_update = config.sync_live.unwrap_or(false);
        self.remote_sync = config.sync_graph_db.unwrap_or(false);

        cx.notify();
    }

    /// 验证当前配置
    fn validate_current_config(&self, cx: &mut Context<Self>) -> Vec<String> {
        let config = self.get_overwrite_config(cx);
        self.config_manager
            .validate_config(&config)
            .unwrap_or_default()
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
                    .when(self.parse_part || self.parse_all, |flex| {
                        flex.child(
                            h_flex()
                                .gap_2()
                                .w_full()
                                .text_size(px(12.0))
                                .pl_4()
                                .child(Label::new("数据库名称"))
                                .child(TextInput::new(&self.parse_part_input)),
                        )
                    }),
            )
            .child(
                v_flex().gap_2().child(Label::new("项目路径")).child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(TextInput::new(&self.project_path))
                        .child(
                            Button::new("path_file_sel")
                                .label("选择")
                                .w(px(60.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.spawn_in(window, async move |this, mut cx| {
                                        if let Some(folder) =
                                            rfd::AsyncFileDialog::new().pick_folder().await
                                        {
                                            let path = folder.path().to_string_lossy().to_string();
                                            this.update_in(cx, |config, window, cx| {
                                                config.project_path.update(cx, |input, cx| {
                                                    input.set_value(path, window, cx);
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
                    .child(TextInput::new(&self.project_name)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("包含项目"))
                    .child(TextInput::new(&self.included_projects)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("MDB名称"))
                    .child(TextInput::new(&self.mdb_name)),
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
                            .child(TextInput::new(&self.db_ip)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("端口").w(px(80.)))
                            .child(TextInput::new(&self.db_port)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("用户名").w(px(80.)))
                            .child(TextInput::new(&self.db_username)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new("密码").w(px(80.)))
                            .child(TextInput::new(&self.db_password)),
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
                        flex.child(TextInput::new(&self.generate_part_input))
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

    // 添加异地部署页面渲染方法
    fn render_remote_deploy_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(Label::new("异地部署配置").text_lg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("启用异地更新"))
                    .child(
                        Switch::new("remote_sync")
                            .checked(self.remote_sync)
                            .on_click(cx.listener(|this, checked, window, cx| {
                                this.remote_sync = *checked;
                                this.notify(cx);
                            })),
                    ),
            )
            .when(self.remote_sync, |flex| {
                flex.child(
                    v_flex()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(Label::new("MQTT服务配置").text_lg())
                                .child(
                                    h_flex()
                                    .gap_2()
                                    .w_full()
                                    .items_center()
                                    .child(Label::new("服务器").text_sm())
                                    .child(TextInput::new(&self.mqtt_server)).flex_grow()
                                    .child(TextInput::new(&self.mqtt_port)).flex_grow()
                                )
                                .child(
                                    Button::new("start_mqtt")
                                        .label(if self.mqtt_running { "停止MQTT服务" } else { "启动MQTT服务" })
                                        .on_click(cx.listener(|this, _, new_window, cx| {
                                            let server = this.mqtt_server.read(cx).value().to_string();
                                            let port = this.mqtt_port.read(cx).value().to_string();

                                            if this.mqtt_running {
                                                // 停止服务
                                                add_global_log("正在停止MQTT服务...", LogLevel::Info);
                                                this.mqtt_running = false;
                                                this.notify(cx);

                                                cx.background_executor()
                                                    .spawn(async {
                                                        let _ = std::process::Command::new("cmd")
                                                            .args(["/C", "taskkill /F /IM mqtt-server.exe"])
                                                            .spawn();
                                                    })
                                                    .detach();
                                            } else {
                                                // 启动服务
                                                add_global_log(format!("正在启动MQTT服务 ({}:{})...", server, port), LogLevel::Info);
                                                this.mqtt_running = true;
                                                this.notify(cx);

                                                let server_copy = server.clone();
                                                let port_copy = port.clone();
                                                cx.background_executor()
                                                    .spawn(async move {
                                                        let _ = std::process::Command::new("cmd")
                                                            .args(["/C", &format!("start /B mqtt-server -h {} -p {}", server_copy, port_copy)])
                                                            .spawn();
                                                    })
                                                    .detach();
                                            }
                                            this.update_logs(cx);
                                        }))
                                )
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(Label::new("数据HTTP服务配置").text_lg())
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .w_full()
                                        .items_center()
                                        .child(Label::new("服务器").text_sm())
                                        .child(TextInput::new(&self.http_server)).flex_grow()
                                        .child(TextInput::new(&self.http_port)).flex_grow()
                                )
                                .child(
                                    Button::new("start_http")
                                        .label(if self.http_running { "停止HTTP服务" } else { "启动HTTP服务" })
                                        .on_click(cx.listener(|this, _, new_window, cx| {
                                            let server = this.http_server.read(cx).value().to_string();
                                            let port = this.http_port.read(cx).value().to_string();

                                            if this.http_running {
                                                // 停止服务
                                                add_global_log("正在停止HTTP服务...", LogLevel::Info);
                                                this.http_running = false;
                                                this.notify(cx);

                                                cx.background_executor()
                                                    .spawn(async {
                                                        let _ = std::process::Command::new("cmd")
                                                            .args(["/C", "taskkill /F /IM simple-http-server.exe"])
                                                            .spawn();

                                                        log_from_thread("已停止HTTP服务", LogLevel::Info);
                                                    })
                                                    .detach();
                                            } else {
                                                // 启动服务
                                                add_global_log(format!("正在启动HTTP服务 ({}:{})...", server, port), LogLevel::Info);
                                                this.http_running = true;
                                                this.notify(cx);

                                                let server_copy = server.clone();
                                                let port_copy = port.clone();
                                                cx.background_executor()
                                                    .spawn(async move {
                                                        let _ = std::process::Command::new("cmd")
                                                            .args(["/C", &format!("start /B simple-http-server --ip {} --port {}", server_copy, port_copy)])
                                                            .spawn();

                                                        log_from_thread("已启动HTTP服务", LogLevel::Info);
                                                    })
                                                    .detach();
                                            }
                                            this.update_logs(cx);
                                        }))
                                )
                        )
                )
            })
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
                                ("remote_deploy", "异地部署"),
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
                            v_flex().gap_4().mt_6().child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(Label::new("显示日志"))
                                    .child(
                                        Switch::new("show_logs").checked(self.show_logs).on_click(
                                            cx.listener(|this, checked, window, cx| {
                                                this.show_logs = *checked;
                                                this.notify(cx);
                                            }),
                                        ),
                                    ),
                            ),
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
                            "remote_deploy" => {
                                self.render_remote_deploy_tab(window, cx).into_any_element()
                            }
                            _ => div().into_any_element(),
                        }),
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
                                            .child(self.log_list.clone()),
                                    )
                                    .child(h_flex().justify_end().mt_2().child(
                                        Button::new("clear_logs").label("清空日志").on_click(
                                            cx.listener(|this, _, _window, cx| {
                                                // 清空日志
                                                if let Ok(mut logs) = GLOBAL_LOGS.lock() {
                                                    logs.clear();
                                                }
                                                this.log_list.update(cx, |list, cx| {
                                                    let mut delegate = list.delegate_mut();
                                                    delegate.logs.clear();
                                                    delegate.selected_index = None;
                                                });
                                            }),
                                        ),
                                    )),
                            ),
                    )
                })
                .child(
                    // 底部按钮
                    h_flex()
                        .absolute()
                        .bottom(px(24.))
                        .right(px(24.))
                        .gap_3()
                        // 添加执行按钮，根据运行状态禁用
                        .child(
                            Button::new("execute_button")
                                .label("开始执行")
                                .disabled(self.is_running)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    // 已在运行中，不执行任何操作
                                    if this.is_running {
                                        return;
                                    }

                                    // 设置为运行状态
                                    this.is_running = true;
                                    this.notify(cx);

                                    // 保存当前配置
                                    // this.save(cx);

                                    let db_option = this.get_overwrite_config(cx);

                                    // 添加执行开始日志
                                    add_global_log("开始执行任务...", LogLevel::Info);
                                    // 立即更新日志显示
                                    this.update_logs(cx);

                                    // 启动任务
                                    let task = cx
                                        .background_executor()
                                        .spawn(async move {
                                            // 创建一个新的 Tokio 运行时
                                            let runtime = match tokio::runtime::Runtime::new() {
                                                Ok(rt) => rt,
                                                Err(e) => {
                                                    log_from_thread(
                                                        format!("创建 Tokio 运行时失败: {}", e),
                                                        LogLevel::Error,
                                                    );
                                                    return false;
                                                }
                                            };

                                            // 在 Tokio 运行时内执行异步任务
                                            let result = runtime.block_on(async {
                                                crate::run_app(Some(db_option)).await
                                            });

                                            true

                                            // match result {
                                            //     Ok(_) => {
                                            //         // log_from_thread("执行成功！", LogLevel::Info);
                                            //         println!("执行成功！");
                                            //         true
                                            //     },
                                            //     Err(e) => {
                                            //         log_from_thread(error_msg, LogLevel::Error);
                                            //         false
                                            //     },
                                            // };
                                        })
                                        .detach();

                                    // 启动一个定时器来检查任务是否完成并更新日志
                                    // cx.spawn_timer(Duration::from_millis(500), move |this, cx| {
                                    //     // 更新日志显示
                                    //     this.update_logs(cx);

                                    //     // 检查任务是否完成
                                    //     if let Some(success) = task.completed() {
                                    //         // 任务完成，恢复按钮状态
                                    //         this.is_running = false;
                                    //         this.notify(cx);

                                    //         // 根据结果添加最终日志
                                    //         if *success {
                                    //             // 任务成功，在日志中显示
                                    //             add_global_log("✅ 任务执行成功，执行完毕!", LogLevel::Info);
                                    //         } else {
                                    //             // 任务失败，在日志中显示
                                    //             add_global_log("❌ 任务执行失败，请查看错误日志!", LogLevel::Error);
                                    //         }

                                    //         // 再次更新日志显示
                                    //         this.update_logs(cx);

                                    //         // 停止定时器
                                    //         return false;
                                    //     }

                                    //     // 继续定时器
                                    //     true
                                    // }).detach();
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
