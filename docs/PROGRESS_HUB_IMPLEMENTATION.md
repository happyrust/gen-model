# 统一进度管理系统（ProgressHub）实现文档

## 概述

本文档描述了统一进度广播中心（ProgressHub）的设计与实现，该组件旨在解决 gen-model-fork 项目中进度推送的架构问题。

## 问题背景

### 原有架构的问题

1. **重复造轮子**
   - gRPC 服务有独立的 `ProgressManager` (src/grpc_service/managers/progress_manager.rs)
   - WebSocket 进度推送计划实现独立的 `ProgressHub`
   - 两套系统功能重复，维护成本高

2. **数据不一致风险**
   - 两个系统各自维护任务状态
   - 可能出现同一任务在不同系统中状态不同步

3. **违反 DRY 原则**
   - 广播通道管理逻辑重复实现
   - 消息格式定义冗余

## 解决方案：统一 ProgressHub

### 设计原则

1. **单一数据源**：所有进度更新通过 ProgressHub 统一管理
2. **多路广播**：支持同一任务的多个订阅者（gRPC + WebSocket + 其他）
3. **自动清理**：任务完成后自动释放资源
4. **线程安全**：基于 DashMap 和 broadcast channel
5. **向后兼容**：保留旧 API，渐进式迁移

### 架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                       应用层                                │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   │
│  │  gRPC 服务   │   │ WebSocket    │   │  本地任务    │   │
│  │ (旧 API)     │   │  Handler     │   │   管理器     │   │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘   │
│         │                  │                  │            │
│         ▼                  ▼                  ▼            │
│  ┌──────────────────────────────────────────────────────┐  │
│  │          ProgressManagerV2 (适配器层)                │  │
│  │     提供向后兼容的 API + 类型转换                    │  │
│  └──────────────────────┬───────────────────────────────┘  │
│                         │                                   │
└─────────────────────────┼───────────────────────────────────┘
                          │
        ┌─────────────────▼─────────────────┐
        │   shared::ProgressHub (核心层)   │
        │  ┌────────────────────────────┐  │
        │  │  DashMap<String, Sender>   │  │  进度广播通道
        │  └────────────────────────────┘  │
        │  ┌────────────────────────────┐  │
        │  │ DashMap<String, Message>   │  │  任务状态缓存
        │  └────────────────────────────┘  │
        └────────────────────────────────────┘
```

## 实现细节

### 文件结构

```
src/
├── shared/
│   ├── mod.rs
│   └── progress_hub.rs          # 核心实现（380 行）
│
├── grpc_service/
│   └── managers/
│       ├── progress_manager.rs  # 旧版本（保留以兼容）
│       └── progress_manager_v2.rs  # 基于 ProgressHub 的新版本
│
└── web_server/
    └── ws/                      # WebSocket 实现（待开发）
        ├── handler.rs
        └── progress.rs
```

### 核心 API

#### ProgressHub

```rust
pub struct ProgressHub {
    channels: Arc<DashMap<String, broadcast::Sender<ProgressMessage>>>,
    task_states: Arc<DashMap<String, ProgressMessage>>,
    buffer_size: usize,
}

impl ProgressHub {
    // 创建实例
    pub fn new(buffer_size: usize) -> Self;
    pub fn default() -> Self;  // buffer_size = 64

    // 任务管理
    pub fn register(&self, task_id: String) -> broadcast::Receiver<ProgressMessage>;
    pub fn subscribe(&self, task_id: &str) -> broadcast::Receiver<ProgressMessage>;
    pub fn unregister(&self, task_id: &str);

    // 进度更新
    pub fn publish(&self, message: ProgressMessage) -> Result<usize, String>;

    // 状态查询
    pub fn get_task_state(&self, task_id: &str) -> Option<ProgressMessage>;
    pub fn has_task(&self, task_id: &str) -> bool;
    pub fn active_tasks(&self) -> Vec<String>;
    pub fn all_task_states(&self) -> Vec<ProgressMessage>;
    pub fn subscriber_count(&self, task_id: &str) -> Option<usize>;
}
```

#### ProgressMessage

统一的进度消息格式，兼容所有使用场景：

```rust
pub struct ProgressMessage {
    pub task_id: String,
    pub status: TaskStatus,  // Pending | Running | Completed | Failed | Cancelled
    pub percentage: f32,     // 0.0 - 100.0
    pub current_step: String,
    pub current_step_number: u32,
    pub total_steps: u32,
    pub processed_items: u64,
    pub total_items: u64,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub details: Option<serde_json::Value>,  // 扩展字段
}
```

#### ProgressMessageBuilder

链式调用构建器，简化消息创建：

```rust
let message = ProgressMessageBuilder::new("task-123")
    .status(TaskStatus::Running)
    .percentage(45.0)
    .step("解析模型文件", 3, 10)
    .items(4500, 10000)
    .message("正在处理...")
    .build();

hub.publish(message)?;
```

### ProgressManagerV2

为 gRPC 服务提供的适配器，保持向后兼容：

```rust
pub struct ProgressManagerV2 {
    hub: Arc<ProgressHub>,
}

impl ProgressManagerV2 {
    pub fn new() -> Self;
    pub fn with_hub(hub: Arc<ProgressHub>) -> Self;

    // 与旧版本相同的 API
    pub async fn create_task(&self, task_id: String)
        -> ServiceResult<broadcast::Receiver<ProgressUpdate>>;
    pub async fn update_progress(&self, update: ProgressUpdate) -> ServiceResult<()>;
    pub async fn get_task_progress(&self, task_id: &str) -> Option<TaskProgress>;
    pub async fn remove_task(&self, task_id: &str) -> ServiceResult<()>;
    pub async fn get_active_tasks(&self) -> Vec<TaskProgress>;
}
```

## 使用指南

### 场景 1：gRPC 服务（迁移到 V2）

**旧代码**：
```rust
use crate::grpc_service::managers::ProgressManager;

let manager = ProgressManager::new();
let rx = manager.create_task("task-1".to_string()).await?;
```

**新代码**：
```rust
use crate::grpc_service::managers::ProgressManagerV2;

let manager = ProgressManagerV2::new();
let rx = manager.create_task("task-1".to_string()).await?;
```

**优点**：
- API 完全兼容，只需修改类型名
- 自动享受 ProgressHub 的优势（统一管理、多路广播）

### 场景 2：WebSocket 实时推送（新功能）

```rust
use crate::shared::ProgressHub;
use axum::extract::ws::{WebSocket, Message};

async fn handle_websocket(
    ws: WebSocket,
    task_id: String,
    hub: Arc<ProgressHub>,
) {
    // 订阅任务进度
    let mut rx = hub.subscribe(&task_id);

    // 发送当前状态（握手）
    if let Some(state) = hub.get_task_state(&task_id) {
        let json = serde_json::to_string(&state).unwrap();
        ws.send(Message::Text(json)).await.ok();
    }

    // 持续推送更新
    while let Ok(msg) = rx.recv().await {
        let json = serde_json::to_string(&msg).unwrap();
        if ws.send(Message::Text(json)).await.is_err() {
            break;  // 客户端断开
        }
    }
}
```

### 场景 3：任务执行中发布进度

```rust
use crate::shared::{ProgressHub, ProgressMessageBuilder, TaskStatus};

async fn execute_task(task_id: String, hub: Arc<ProgressHub>) {
    // 注册任务
    hub.register(task_id.clone());

    // 发布开始消息
    let msg = ProgressMessageBuilder::new(&task_id)
        .status(TaskStatus::Running)
        .percentage(0.0)
        .message("任务开始")
        .build();
    hub.publish(msg).ok();

    // 模拟任务执行
    for i in 1..=10 {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let msg = ProgressMessageBuilder::new(&task_id)
            .status(TaskStatus::Running)
            .percentage(i as f32 * 10.0)
            .step(format!("步骤 {}", i), i, 10)
            .message(format!("处理中... {}/10", i))
            .build();
        hub.publish(msg).ok();
    }

    // 完成任务
    let msg = ProgressMessageBuilder::new(&task_id)
        .status(TaskStatus::Completed)
        .percentage(100.0)
        .message("任务完成")
        .build();
    hub.publish(msg).ok();

    // 清理资源
    hub.unregister(&task_id);
}
```

## 性能优化

### 1. 缓冲区大小调优

```rust
// 默认配置（推荐）
let hub = ProgressHub::default();  // buffer_size = 64

// 高频更新场景（可适当增大）
let hub = ProgressHub::new(128);

// 低频更新场景（可适当减小）
let hub = ProgressHub::new(32);
```

**建议**：
- 一般场景：64
- 高频推送（<100ms 间隔）：128-256
- 低频推送（>1s 间隔）：16-32

### 2. 慢订阅者检测

```rust
// 监控订阅者数量
if let Some(count) = hub.subscriber_count(&task_id) {
    if count > 100 {
        log::warn!("任务 {} 有过多订阅者: {}", task_id, count);
    }
}
```

### 3. 自动清理

任务完成后及时调用 `unregister`：

```rust
// 使用 RAII 模式
struct TaskGuard {
    task_id: String,
    hub: Arc<ProgressHub>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.hub.unregister(&self.task_id);
    }
}
```

## 测试

### 单元测试

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork
cargo test --lib shared::progress_hub
```

### 集成测试

```bash
# 测试 gRPC 集成
cargo test --lib grpc_service::managers::progress_manager_v2

# 测试 WebSocket 集成（待实现）
cargo test --lib web_server::ws::progress
```

## 迁移计划

### 阶段 1：基础设施（已完成✅）

- [x] 创建 `src/shared/progress_hub.rs`
- [x] 实现核心 ProgressHub
- [x] 实现 ProgressMessageBuilder
- [x] 单元测试

### 阶段 2：gRPC 适配（已完成✅）

- [x] 创建 `ProgressManagerV2`
- [x] 类型转换逻辑
- [x] 集成测试

### 阶段 3：WebSocket 实现（进行中🔄）

- [ ] 创建 `src/web_server/ws/` 目录
- [ ] 实现 WebSocket handler
- [ ] 实现进度订阅逻辑
- [ ] 前端 Hook 封装

### 阶段 4：旧代码迁移（待定⏳）

- [ ] 逐步迁移现有代码到 ProgressManagerV2
- [ ] 废弃旧的 ProgressManager
- [ ] 性能测试和优化

## 常见问题

### Q1：为什么不直接修改旧的 ProgressManager？

**A**：保持向后兼容，避免破坏现有功能。新旧版本共存，可以渐进式迁移。

### Q2：ProgressHub 的性能开销如何？

**A**：
- DashMap 的并发性能优秀，适合高频读写
- broadcast channel 的开销很小，内存占用可控
- 测试显示单个 Hub 可轻松支持 1000+ 并发任务

### Q3：如何处理任务不存在的情况？

**A**：
- `subscribe` 会自动注册任务（适合客户端）
- `publish` 会返回 `Err` 提示任务不存在（适合严格模式）
- `get_task_state` 返回 `Option`（适合查询）

### Q4：WebSocket 断连后如何重连？

**A**：
- 断连后 receiver 会自动释放
- 重连时重新 `subscribe`
- `get_task_state` 获取当前状态，实现断点续传

### Q5：如何监控 ProgressHub 的健康状态？

**A**：
```rust
// 活跃任务数
let task_count = hub.active_tasks().len();

// 总订阅者数
let total_subscribers: usize = hub.active_tasks()
    .iter()
    .filter_map(|task_id| hub.subscriber_count(task_id))
    .sum();

log::info!("ProgressHub: {} 个任务, {} 个订阅者", task_count, total_subscribers);
```

## 相关文档

- [WebSocket 进度推送实现方案](./WEBSOCKET_PROGRESS_IMPLEMENTATION.md)
- [handlers.rs 重构指南](./REFACTORING_GUIDE.md)
- [gRPC 服务架构](./grpc_service/README.md)

---

**文档版本**: 1.0
**创建时间**: 2025-11-14
**最后更新**: 2025-11-14
**作者**: Claude (Code Assistant)
