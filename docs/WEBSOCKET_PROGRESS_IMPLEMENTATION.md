# WebSocket 模型生成进度推送方案

## 1. 背景与目标

- Web UI 目前通过 REST 轮询 `/api/tasks` 与 `/api/tasks/{id}` 获取 `TaskInfo.progress`，延迟高且浪费请求数。
- 模型生成任务内部已经在 `execute_refno_model_generation` 等函数中多次调用 `task.update_progress(...)`、`task.add_log(...)`，但缺少实时推送通道。参考 rs-plant3-d 项目中的 `ModelLoadingProgressEvent`，可以把阶段变更即时推送给前端。
- 目标：为 gen-model-fork 提供 WebSocket 推送能力，使浏览器端可以实时订阅单个任务（尤其是模型生成任务）的进度、状态和日志摘要。

## 2. 现状盘点

| 模块 | 功能 | 现状 |
| --- | --- | --- |
| `src/web_server/handlers.rs` 中的 `api_generate_by_refno`、`execute_real_task` | 创建并执行 `TaskInfo` | 只改写 `TaskManager`，无实时推送。 |
| `src/web_server/models.rs` | 定义 `TaskInfo`/`TaskProgress` | 具备进度字段，但仅被 REST 返回。 |
| `src/grpc_service/managers/progress_manager.rs` | 为 gRPC 服务准备的 `ProgressManager` | 维护 `broadcast::Sender<ProgressUpdate>`，可复用。 |
| 前端 `frontend/src`（任务列表页） | 轮询 REST | 没有 WebSocket 客户端。 |

## 3. 整体设计

1. **进度采集**：在 `execute_real_task`、`execute_refno_model_generation` 等长任务函数中，除了更新 `TaskInfo`，同时调用新的 `ProgressHub::update(ProgressUpdate)`。
2. **广播中心（ProgressHub）**：
   - 封装现有 `ProgressManager` 或在 WebServer 内再实现一个 `DashMap<task_id, broadcast::Sender>`。
   - 任务启动时 `ProgressHub::register(task_id)`，返回 `Receiver` 给 WebSocket handler 使用。
   - 任务结束后调用 `ProgressHub::complete(task_id)` 清理通道。
3. **WebSocket 路由**：新增 `GET /ws/tasks/{id}`（axum `WebSocketUpgrade`）。
   - 验证任务存在 -> 订阅 `ProgressHub`。
   - 将 `ProgressUpdate` 序列化为 JSON 文本帧发送；若任务完成/失败后自动发送终止消息并关闭。
4. **前端集成**：
   - 任务列表页在创建任务后即连接 `ws://{host}/ws/tasks/{task_id}`。
   - 收到消息后更新 UI；若连接断开则回退到轮询。

## 4. 数据结构

```rust
#[derive(Serialize, Deserialize)]
pub struct WsProgressMessage {
    pub task_id: String,
    pub status: TaskStatus,
    pub percentage: f32,
    pub current_step: String,
    pub current_step_number: u32,
    pub total_steps: u32,
    pub processed_items: u64,
    pub total_items: u64,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}
```

- 兼容 `TaskProgress` + `ProgressUpdate`，前端只需解析一个统一对象。

## 5. API 设计

| 路由 | 方法 | 描述 |
| --- | --- | --- |
| `/ws/tasks/{task_id}` | GET (WebSocket) | 订阅任务进度推送 |
| 错误帧格式 | JSON | `{ "type": "error", "message": "Task not found" }` |
| 完成帧格式 | JSON | `{ "type": "complete", "status": "Completed" }` |

握手步骤：

1. 客户端发起 WS 连接，携带任务 ID。
2. 服务端校验任务是否属于当前 `TaskManager`（活跃或历史）。
3. 若任务尚未注册广播，则先 `ProgressHub::register(task_id)`。
4. 将历史最新进度立即推送一次（防止空白）。
5. 订阅通道并持续发送消息；若任务结束则发送完成帧并关闭连接。

## 6. 前端接入

- 封装 `useTaskProgress(taskId)` Hook：

  ```ts
  const socket = new WebSocket(`${baseWsUrl}/ws/tasks/${taskId}`);
  socket.onmessage = (evt) => {
      const payload = JSON.parse(evt.data);
      updateTaskStore(taskId, payload);
  };
  socket.onclose = () => scheduleFallbackPolling(taskId);
  ```

- UI 展示逻辑：
  - 进度条：`payload.percentage`
  - 当前阶段文案：`payload.current_step`
  - 处理数量：`payload.processed_items/total_items`
  - 同步日志：若 `payload.message` 变化则追加到日志列表。

## 7. 实施步骤

1. **封装 ProgressHub**
   - 新建 `src/web_server/progress_hub.rs`，内部持有 `DashMap<String, broadcast::Sender<WsProgressMessage>>`。
   - 提供 `register`, `unregister`, `send_update`。
2. **改造任务执行路径**
   - 在 `execute_real_task` / `execute_refno_model_generation` / 其他长任务开始时调用 `progress_hub.register(task_id)`。
   - 每次 `task.update_progress` 后同步调用 `progress_hub.send_update(task_id, WsProgressMessage::from(&task))`。
3. **新增 WebSocket handler**
   - 文件：`src/web_server/ws.rs`。
   - 路由挂载：`Router::route("/ws/tasks/{id}", get(ws_task_progress))`。
4. **前端 Hook 与状态管理**
   - 在 `frontend/src/hooks/useTaskProgress.ts` 实现连接/重连。
   - 更新任务列表页使用 Hook。
5. **测试与验证**
   - 本地运行 web_server，调用 `api_generate_by_refno`，在浏览器/CLI (wscat) 监听 `/ws/tasks/{id}`，观察实时推送。
   - 回归：REST 轮询仍可工作，确保无兼容性问题。

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 大量任务同时推送导致 broadcast channel 溢出 | 限制 buffer（如 64），必要时丢弃旧帧并在前端做节流。 |
| 客户端断开未清理 | `ProgressHub` 在 receiver dropped 时自动清理；或设置心跳帧。 |
| 任务启动和 WS 连接顺序竞争 | 在 handler 中若任务尚未 register，则先创建 sender 并返回最新 `TaskInfo.progress`。 |

## 9. 后续扩展

- 支持订阅任务列表（`/ws/tasks`）广播多任务摘要。
- 将房间计算等其他任务统一接入。
- 在 `TaskProgress` 增加阶段字段，方便前后端统一展示。
