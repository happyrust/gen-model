## 任务完成前须知
- 对代码改动执行 `cargo fmt`、`cargo clippy --all-targets --all-features` 以及相关 `cargo test`/专项脚本（如导出或 XKT 测试）。
- 导出/前端联调任务需跑 `cargo run --bin aios-database ...` 并同步输出到 `instanced-mesh/public/bundles/`，在 Vite dev server 中验证。
- 若改动影响文档/配置（如 `DbOption*.toml`, `docs/`），同步更新说明文件。
- 确认未引入脏的 auto-generated 文件，必要时更新 `DEVELOPMENT_PLAN.md` 或相关规约。
