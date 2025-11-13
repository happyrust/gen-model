## 项目概览
- **定位**: 工厂模型/几何生成与导出平台，核心任务是把 SurrealDB 中的 PDMS/instanced 数据转成多种格式（预打包 LOD glb、XKT、Web 可视化等），并提供 CLI/后端 API 与前端展示。
- **主要语言/框架**: Rust 为主（workspace，包括 `gen-model-fork`、`rs-core` 等 crate），配合 SurrealDB、LiteFS、MQTT 等；前端/工具链使用 TypeScript + Vite、Node.js 脚本、pnpm。
- **结构**: `src/` 内为核心 Rust 逻辑（`fast_model`, `web_api`, `data_interface` 等）；`rs-core/` 保存通用几何/数据库层；`docs/`、`INSTANCED_BUNDLE_*` 等提供设计说明；前端资源在 `instanced-mesh/`、`frontend/`；脚本在 `scripts/`、根目录 `run_*.sh`。
- **特殊依赖**: SurrealDB（多端口，配置在 `DbOption*.toml`）；Bevy/egui/SurrealDB 相关实现遵循 AGENTS 指南；多数据文件 (`assets/`, `data/`, `test_data/`) 需保持齐备。
