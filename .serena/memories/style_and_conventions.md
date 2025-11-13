## 代码风格与约定
- Rust 统一 4 空格缩进，提交前运行 `cargo fmt`、`cargo clippy --all-targets --all-features`；模块命名 snake_case，公开类型/trait PascalCase。
- 测试位于实现同目录 `mod tests` 或 `src/test/`，命名 `_test` 结尾；示例数据在 `test_data/`。
- 文档/配置命名：配置文件小写短横线（如 `DbOption*.toml`），脚本/Node 模块也小写短横线。
- 代码注释：复杂逻辑前补充简洁注释，避免冗余说明。
- 文档首选官方来源（Bevy/egui/SurrealDB），输出示例给出最小可运行样例。
