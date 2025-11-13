## 常用命令
- 构建：`cargo build --all-features`
- 回归测试：`cargo test --all-targets`
- Web UI：`cargo run --bin web_server --features web_server`
- 快速模型生成校验：`./run_model_gen_test.sh`
- XKT 生成链路：`./test_xkt_generation.sh`
- LOD/Instanced 导出：`cargo run --bin aios-database -- --config DbOption --export-all-relates --verbose`
- 前端预览（instanced-mesh）：`pnpm install && pnpm start -- --host 0.0.0.0`，浏览器访问 `http://localhost:5173/examples/aios-prepack-loader.html`
- 同步导出到前端：`rsync -a output/instanced-bundle/<bundle>/ ../instanced-mesh/public/bundles/<bundle>/`
