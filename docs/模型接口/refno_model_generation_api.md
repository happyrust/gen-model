# 指定参考号模型生成 API 开发方案

## 1. 背景与目标

- 现有 Web Server 支持数据库级别的模型生成，但缺少按参考号（Refno）精确触发的生成入口。
- `fast_model::gen_model::gen_all_geos_data` 已支持 `manual_refnos` 参数，可复用核心能力。
- 目标：提供 `POST /api/model/generate-by-refno` 接口，接收 `db_num` 与 `refnos` 列表，创建并执行后台任务，完成指定参考号的模型生成。

## 2. 依赖与上下游

| 模块 | 作用 |
| --- | --- |
| `src/web_server/models.rs` | TaskType、TaskInfo、Config 结构体定义；需新增 Request/Response 与 TaskType 变体 |
| `src/web_server/mod.rs` | Axum Router；需注册新路由 |
| `src/web_server/handlers.rs` | `execute_real_task` 执行入口；需扩展任务分支处理 Refno 生成 |
| `src/fast_model/gen_model.rs` | `gen_all_geos_data` 核心逻辑，已支持 `manual_refnos` |
| TaskManager / AppState | 负责任务登记、状态跟踪、日志记录 |

## 3. 需求分解

1. **数据结构**
   - `RefnoModelGenerationRequest { db_num: u32, refnos: Vec<String>, generate_mesh?: bool, generate_spatial_tree?: bool, target_sesno?: u32 }`
   - `RefnoModelGenerationResponse { task_id: String, status: TaskStatus, message: String }`
   - `TaskType::RefnoModelGeneration`
   - `TaskConfig.manual_refnos: Option<Vec<String>>`（或封装为 `RefnoEnum` 集合）
2. **API 行为**
   - 路由：`POST /api/model/generate-by-refno`
   - 校验：`db_num` 必填，`refnos` 非空且格式合法；可选参数提供默认值
   - 成功时立即返回任务 ID，后续通过现有任务查询接口获取进度
3. **任务执行**
   - 构建 `TaskInfo`：名称可按 `"Refno Generation (db#{db_num})"`
   - 任务配置：`manual_db_nums = Some(vec![db_num])`，`manual_refnos = Some(refnos)`，`gen_model = true`，其余开关根据请求赋值
   - `execute_real_task` 中新增分支：当 `task_type == TaskType::RefnoModelGeneration` 时，直接进入几何生成步骤，调用 `gen_all_geos_data(manual_refnos, &db_option, None, config.target_sesno)`
   - 保留原有进度更新/错误处理/取消逻辑

## 4. 实现步骤

1. **结构体与枚举扩展**
   - `models.rs`
     - 新增请求/响应结构体
     - `TaskType` 增加枚举值并更新 `Display` / 序列化实现
     - `TaskConfig` 添加 `manual_refnos: Vec<String>`（或 `Vec<RefnoEnum>`），并在 `Default`/`serde` 中处理
2. **路由与 Handler**
   - `mod.rs` 注册 `router.route("/api/model/generate-by-refno", post(generate_by_refno_handler))`
   - 新建 `generate_by_refno_handler`
     1. 解析 `Json<RefnoModelGenerationRequest>`
     2. 校验入参，标准化 refno（trim / upper）
     3. 创建 `TaskInfo` 并写入 `task_manager`
     4. 直接 `tokio::spawn` 调用 `execute_real_task`
     5. 返回 `RefnoModelGenerationResponse`
3. **执行逻辑**
   - `execute_real_task`
     - 提取 `manual_refnos` 并转换为 `Vec<RefnoEnum>`
     - 当 `TaskType::RefnoModelGeneration` 时跳过解析步骤，可配置 `needs_parse_first = false`
     - 在“生成几何数据”阶段调用 `gen_all_geos_data(manual_refnos, &db_option, None, config.target_sesno)`
     - 保持进度监听与错误处理一致
4. **文档与校验**
   - 更新 `ROOM_API_DESIGN.md` 或新增 Quickstart，列出示例请求与任务状态查询流程
   - 运行 `cargo fmt && cargo clippy --all-targets --all-features`
   - 如需集成测试，可编写 `tests/refno_generation_api.rs`，mock TaskManager / AppState 验证请求校验逻辑

## 5. 验证方法

1. **本地启动 web_server**：`cargo run --bin web_server --features web_server`
2. **发起请求示例**：

```bash
curl -X POST http://localhost:8000/api/model/generate-by-refno \
  -H "Content-Type: application/json" \
  -d '{"db_num":1,"refnos":["SITE/123","SITE/456"],"generate_mesh":true}'
```

3. **查看任务**：`GET /api/tasks/{task_id}`，确认进度由 Pending→Running→Completed
4. **结果验证**：检查输出目录（XKT/缓存库）或任务日志，确认仅处理指定 refno

## 6. 后续扩展

- 支持批量 refno 的优先级或并发队列控制
- 前端任务创建界面增加 refno 输入表单
- 接入权限校验（例如仅允许特定角色触发局部生成）
