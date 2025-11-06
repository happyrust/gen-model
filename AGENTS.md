# Repository Guidelines

## 项目结构与模块组织
核心 Rust 代码位于 `src/`，其中 `lib.rs` 暴露通用库，`main.rs` 和 `src/bin/` 下的入口二进制负责 CLI 与 Web UI。`src/cata/`、`src/data_interface/` 等子模块以领域拆分功能，测试样例集中在 `src/test/` 与 `test_data/`。静态资源与示例数据分别放在 `assets/` 与 `data/`，自动化脚本位于 `scripts/` 与仓库根目录的 `run_*.sh` 文件。前端与可视化配套位于 `frontend/` 与 `js/`，`docs/` 保留面向外部的设计文档。

## 构建、测试与开发命令
`cargo build --all-features` 编译完整功能集，推荐在提交前执行以验证依赖特性互相兼容。`cargo run --bin web_server --features web_server` 启动内置 Web UI 调试界面，默认读取 `DbOption.toml` 中的配置。核心回归测试使用 `cargo test --all-targets`，并在需要重放模型导出流程时运行 `./run_model_gen_test.sh`。若需复现 XKT 生成链路，可执行 `./test_xkt_generation.sh` 或 `node test_complete_flow.js`。

## 代码风格与命名约定
Rust 代码统一采用四空格缩进并运行 `cargo fmt`，提交前请补充 `cargo clippy --all-targets --all-features` 以捕获潜在缺陷。模块命名保持蛇形命名（`snake_case`），公开类型与 trait 使用帕斯卡命名（`PascalCase`），配置文件沿用全小写短横线风格。脚本与 Node 模块遵循 ES 模块语法，文件名保持小写短横线（如 `generate_zone_demo.js`）。

## 测试准则
Rust 测试默认放在与实现同目录的 `mod tests` 或 `src/test/` 独立模块，命名以 `_test` 结尾突出意图。空间索引与数据库兼容性测试使用嵌入式 SQLite，因此本地运行前确保 `test_data/` 下示例数据库齐备。重现端到端模型生成时，先执行 `cargo build`，再调用 `./run_model_gen_test.sh` 以比对输出目录差异；必要时将生成的日志上传至 `logs/` 或 `output/` 目录便于复查。

## 提交与合并请求指南
Git 历史既包含 `feat:` 前缀的变更，也有简洁的命令式短句，建议统一采用动词开头的英文一句话摘要（如 `Add surreal cache warmup`）。若修复特定缺陷，请在正文引用关联 issue（`Fixes #123`）并概述验证方法。创建合并请求时附带功能说明、测试结果与必要的截屏或日志；涉及配置或数据格式更新时同时更新 `docs/` 与对应的 `DbOption*.toml` 注释。

## 配置与环境提示
默认配置文件位于仓库根目录的 `DbOption*.toml`，本地调试请复制 `DbOption.toml` 为私有版本并避免提交敏感路径。运行需要外部依赖（如 SurrealDB、MQTT 或 LiteFS）时，可使用 `start_surreal_with_check.sh` 与 `litefs-start.sh` 快速拉起必要服务；在 CI 环境请禁用 `--features web_server` 以缩短构建时间。
