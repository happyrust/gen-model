use crate::gui::logs::{LogLevel, add_global_log};
use crate::options::DbOptionExt;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    Disableable, Sizable,
    button::Button,
    h_flex,
    input::{InputState, TextInput},
    label::Label,
    switch::Switch,
    theme::ActiveTheme,
    v_flex,
};
use std::path::Path;

/// 解析面板状态
pub struct ParsePanelState {
    pub parse_all: bool,
    pub parse_part: bool,
    pub parse_part_input: Entity<InputState>,
    pub project_path: Entity<InputState>,
    pub project_name: Entity<InputState>,
    pub included_projects: Entity<InputState>,
    pub mdb_name: Entity<InputState>,
    pub only_sync_sys: bool,
    pub is_parsing: bool,
}

impl ParsePanelState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            parse_all: false,
            parse_part: false,
            parse_part_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder("请输入数据库文件名, 多个用逗号分隔")
            }),
            project_path: cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/project")),
            project_name: cx.new(|cx| InputState::new(window, cx).placeholder("项目名称")),
            included_projects: cx
                .new(|cx| InputState::new(window, cx).placeholder("包含的项目，逗号分隔")),
            mdb_name: cx.new(|cx| InputState::new(window, cx).placeholder("ALL")),
            only_sync_sys: false,
            is_parsing: false,
        }
    }

    /// 从配置加载状态
    pub fn load_from_config(&mut self, config: &DbOptionExt, window: &mut Window, cx: &mut App) {
        self.parse_all = config.total_sync;
        self.parse_part = config.incr_sync;
        self.only_sync_sys = config.only_sync_sys;

        self.project_path.update(cx, |input, cx| {
            input.set_value(config.project_path.clone(), window, cx)
        });
        self.project_name.update(cx, |input, cx| {
            input.set_value(config.project_name.clone(), window, cx)
        });
        self.included_projects.update(cx, |input, cx| {
            input.set_value(config.included_projects.join(","), window, cx)
        });
        self.mdb_name.update(cx, |input, cx| {
            input.set_value(config.mdb_name.clone(), window, cx)
        });
    }

    /// 应用到配置
    pub fn apply_to_config(&self, config: &mut DbOptionExt, cx: &App) {
        config.total_sync = self.parse_all;
        config.incr_sync = self.parse_part;
        config.only_sync_sys = self.only_sync_sys;

        config.project_path = self.project_path.read(cx).value().to_string();
        config.project_name = self.project_name.read(cx).value().to_string();
        config.mdb_name = self.mdb_name.read(cx).value().to_string();

        let included_text = self.included_projects.read(cx).value();
        config.included_projects = if included_text.trim().is_empty() {
            Vec::new()
        } else {
            included_text
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        };

        config.included_db_files = {
            let text = self.parse_part_input.read(cx).value();
            if text.trim().is_empty() {
                None
            } else {
                Some(text.split(',').map(|s| s.trim().to_string()).collect())
            }
        };
    }

    /// 验证配置
    pub fn validate(&self, cx: &App) -> Vec<String> {
        let mut errors = Vec::new();

        let project_path = self.project_path.read(cx).value();
        if project_path.is_empty() {
            errors.push("项目路径不能为空".to_string());
        } else if !Path::new(project_path.as_ref()).exists() {
            errors.push(format!("项目路径不存在: {}", project_path));
        }

        let project_name = self.project_name.read(cx).value();
        if project_name.is_empty() {
            errors.push("项目名称不能为空".to_string());
        }

        if !self.parse_all && !self.parse_part {
            errors.push("请至少选择一种解析模式（全部解析或部分解析）".to_string());
        }

        if self.parse_part {
            let db_files = self.parse_part_input.read(cx).value();
            if db_files.trim().is_empty() {
                errors.push("部分解析模式下，数据库文件名不能为空".to_string());
            }
        }

        errors
    }

    /// 渲染解析面板
    pub fn render<V: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .gap_6()
            .p_4()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("解析模块配置")
                            .text_lg()
                            .text_color(theme.foreground),
                    )
                    .child(
                        Button::new("validate_parse")
                            .label("验证配置")
                            .small()
                            .disabled(self.is_parsing)
                            .on_click(cx.listener(|this: &mut V, _, window, cx| {
                                // 验证逻辑将在父组件中实现
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_4()
                    .p_4()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .child(
                        Label::new("解析模式")
                            .text_sm()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("全部重新解析")).child(
                                    Label::new("完整解析所有数据库文件")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("parse_all")
                                    .checked(self.parse_all)
                                    .disabled(self.is_parsing)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("增量解析")).child(
                                    Label::new("仅解析指定的数据库文件")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("parse_part")
                                    .checked(self.parse_part)
                                    .disabled(self.is_parsing)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    )
                    .when(self.parse_part, |flex| {
                        flex.child(
                            v_flex()
                                .gap_2()
                                .pl_4()
                                .child(Label::new("数据库文件名").text_sm())
                                .child(
                                    TextInput::new(&self.parse_part_input)
                                        .disabled(self.is_parsing),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("仅同步系统数据")).child(
                                    Label::new("只同步系统级别的数据")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("only_sync_sys")
                                    .checked(self.only_sync_sys)
                                    .disabled(self.is_parsing)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_4()
                    .p_4()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .child(
                        Label::new("项目配置")
                            .text_sm()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Label::new("项目路径").text_sm())
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .w_full()
                                    .child(
                                        TextInput::new(&self.project_path)
                                            .disabled(self.is_parsing),
                                    )
                                    .child(
                                        Button::new("path_file_sel")
                                            .label("选择")
                                            .small()
                                            .w(px(60.))
                                            .disabled(self.is_parsing)
                                            .on_click(cx.listener(
                                                |this: &mut V, _, window, cx| {
                                                    // 文件选择逻辑将在父组件中实现
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Label::new("项目名称").text_sm())
                            .child(TextInput::new(&self.project_name).disabled(self.is_parsing)),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Label::new("包含项目").text_sm())
                            .child(
                                TextInput::new(&self.included_projects).disabled(self.is_parsing),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Label::new("MDB名称").text_sm())
                            .child(TextInput::new(&self.mdb_name).disabled(self.is_parsing)),
                    ),
            )
    }
}
