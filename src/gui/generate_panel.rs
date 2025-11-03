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

/// 模型生成面板状态
pub struct GeneratePanelState {
    pub generate_all: bool,
    pub generate_part: bool,
    pub generate_part_input: Entity<InputState>,
    pub gen_mesh: bool,
    pub gen_spatial_tree: bool,
    pub apply_boolean_operation: bool,
    pub mesh_tol_ratio: Entity<InputState>,
    pub is_generating: bool,
}

impl GeneratePanelState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            generate_all: false,
            generate_part: false,
            generate_part_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("请输入refno, 多个用逗号分隔")),
            gen_mesh: false,
            gen_spatial_tree: true,
            apply_boolean_operation: true,
            mesh_tol_ratio: cx.new(|cx| InputState::new(window, cx).placeholder("3.0")),
            is_generating: false,
        }
    }

    /// 从配置加载状态
    pub fn load_from_config(&mut self, config: &DbOptionExt, window: &mut Window, cx: &mut App) {
        self.gen_mesh = config.gen_mesh;
        self.gen_spatial_tree = config.gen_spatial_tree;
        self.apply_boolean_operation = config.apply_boolean_operation;

        if let Some(ratio) = config.mesh_tol_ratio {
            self.mesh_tol_ratio.update(cx, |input, cx| {
                input.set_value(ratio.to_string(), window, cx)
            });
        }
    }

    /// 应用到配置
    pub fn apply_to_config(&self, config: &mut DbOptionExt, cx: &App) {
        config.gen_model = self.generate_all || self.generate_part;
        config.gen_mesh = self.gen_mesh;
        config.gen_spatial_tree = self.gen_spatial_tree;
        config.apply_boolean_operation = self.apply_boolean_operation;

        let ratio_text = self.mesh_tol_ratio.read(cx).value();
        config.mesh_tol_ratio = ratio_text.parse().ok();

        config.manual_db_nums = {
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
    }

    /// 验证配置
    pub fn validate(&self, cx: &App) -> Vec<String> {
        let mut errors = Vec::new();

        if !self.generate_all && !self.generate_part {
            errors.push("请至少选择一种生成模式（全部生成或部分生成）".to_string());
        }

        if self.generate_part {
            let refnos = self.generate_part_input.read(cx).value();
            if refnos.trim().is_empty() {
                errors.push("部分生成模式下，refno不能为空".to_string());
            }
        }

        if self.gen_mesh {
            let ratio_text = self.mesh_tol_ratio.read(cx).value();
            if let Ok(ratio) = ratio_text.parse::<f64>() {
                if ratio <= 0.0 {
                    errors.push("网格容差比率必须大于0".to_string());
                }
            } else if !ratio_text.is_empty() {
                errors.push("网格容差比率必须是有效的数字".to_string());
            }
        }

        errors
    }

    /// 渲染生成面板
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
                        Label::new("模型生成配置")
                            .text_lg()
                            .text_color(theme.foreground),
                    )
                    .child(
                        Button::new("validate_generate")
                            .label("验证配置")
                            .small()
                            .disabled(self.is_generating)
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
                        Label::new("生成模式")
                            .text_sm()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("全部生成")).child(
                                    Label::new("生成所有数据库的模型")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("generate_all")
                                    .checked(self.generate_all)
                                    .disabled(self.is_generating)
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
                                v_flex().gap_1().child(Label::new("部分生成")).child(
                                    Label::new("仅生成指定refno的模型")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("generate_part")
                                    .checked(self.generate_part)
                                    .disabled(self.is_generating)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    )
                    .when(self.generate_part, |flex| {
                        flex.child(
                            v_flex()
                                .gap_2()
                                .pl_4()
                                .child(Label::new("Refno列表").text_sm())
                                .child(
                                    TextInput::new(&self.generate_part_input)
                                        .disabled(self.is_generating),
                                ),
                        )
                    }),
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
                        Label::new("生成选项")
                            .text_sm()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("生成网格")).child(
                                    Label::new("生成三角网格数据")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("gen_mesh")
                                    .checked(self.gen_mesh)
                                    .disabled(self.is_generating)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    )
                    .when(self.gen_mesh, |flex| {
                        flex.child(
                            v_flex()
                                .gap_2()
                                .pl_4()
                                .child(Label::new("网格容差比率").text_sm())
                                .child(
                                    TextInput::new(&self.mesh_tol_ratio)
                                        .disabled(self.is_generating),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex().gap_1().child(Label::new("生成空间树")).child(
                                    Label::new("生成空间索引树结构")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("gen_spatial_tree")
                                    .checked(self.gen_spatial_tree)
                                    .disabled(self.is_generating)
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
                                v_flex().gap_1().child(Label::new("应用布尔运算")).child(
                                    Label::new("对模型应用布尔运算优化")
                                        .text_xs()
                                        .text_color(theme.muted_foreground),
                                ),
                            )
                            .child(
                                Switch::new("apply_boolean_operation")
                                    .checked(self.apply_boolean_operation)
                                    .disabled(self.is_generating)
                                    .on_click(cx.listener(|this: &mut V, checked, window, cx| {
                                        // 切换逻辑将在父组件中实现
                                    })),
                            ),
                    ),
            )
    }
}
