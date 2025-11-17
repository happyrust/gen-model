# MinIO 异地增量同步开发计划

## 1. 背景与目标

当前方案：
- 增量由 `increment_manager` 从 PDMS 提取，写入 SurrealDB / `element_changes` 表。
- 生成 `.cba` 压缩包，写入本地文件目录或文件服务器。
- 通过 `SyncE3dFileMsg`（file_names + file_hashes + file_server_host + location）经 MQTT 通知远端，远端从文件服务器拉取 `.cba` 并应用。

目标：
- 引入 MinIO（S3 协议对象存储），统一管理 `.cba` 增量文件。
- 使用对象存储事件 + MQTT/任务队列驱动多机组/多地域同步，解耦“文件分发”和“任务执行”。
- 保留现有方案作为回退路径，逐步灰度迁移。

不改变：
- 增量提取逻辑（`collect_increment_eles`、`update_elements_to_database`）。
- 模型/网格生成主流程（`gen_all_geos_data` 等）。

---

## 2. 目标架构概述（线框）

### 2.1 组件视图

```text
[站点 A]
  IncrementManager_A  →  MinIO_A (本地或中心)      →  Bucket Notification
                                           │
                                           ▼
                             Sync Orchestrator（中心/站点内）
                                           │
                                           ▼
                                   MQTT: Sync/E3dObject
                                           │
                                           ▼
[站点 B]
  RemoteSyncAgent_B  →  MinIO_A / CentralMinIO  →  E3dIncrementApplier_B
```

### 2.2 时序概览

1. 站点 A：
   - PDMS 文件变化 → `increment_manager` 收集增量；
   - 写入 SurrealDB / `element_changes`；
   - 生成 `.cba` → 上传 MinIO（写入 batch_id、source_location 等 metadata）。
2. MinIO：
   - 触发 Bucket ObjectCreated 事件 → 推送到 Sync Orchestrator。
3. Sync Orchestrator：
   - 根据事件构造 `SyncE3dObjectMsg`，写入任务表 `sync_jobs`，发布到 MQTT `Sync/E3dObject` 主题。
4. 站点 B：
   - `RemoteSyncAgent` 订阅 `Sync/E3dObject`，按 `target_locations` / 本地配置过滤；
   - 幂等检查后，从 MinIO 下载 `.cba`，写本地 `increment_jobs_B`，调用 `E3dIncrementApplier_B` 应用增量；
   - 写入 `applied_batches` 标记完成，可选向 Orchestrator ACK。

---

## 3. 消息与数据结构

### 3.1 MinIO 对象 metadata

上传 `.cba` 时写入以下 metadata（键名可根据 MinIO/S3 规范前缀 `x-amz-meta-`）：

- `batch_id`: 增量批次 ID，如 `site_a-20251117-000123` 或 `site_a-<sesno_range>-<uuid>`。
- `source_location`: 源站点标识，如 `site_a`。
- `project`: 项目/数据库标识，如 `projectA` 或 `db_aba`。
- `sesno_range`: `start_sesno-end_sesno`。
- `hash`: `.cba` 内容 hash，复用现有计算逻辑。

### 3.2 新的 MQTT 消息结构 `SyncE3dObjectMsg`

在 `mqtt_service/mod.rs` 中新增结构体（伪代码）：

- `batch_id: String`  // 增量批次 ID
- `bucket: String`    // MinIO bucket 名，如 `e3d-increments`
- `object_key: String` // 对象 key，如 `projectA/siteA/2025-11-17/batch-123.cba`
- `object_hash: String` // 对象内容 hash
- `object_size: u64`    // 对象大小（可选）
- `source_location: String` // 源站点标识
- `target_locations: Vec<String>` // 目标站点列表（可选）
- `project: String`      // 项目/数据库标识
- `created_at: String`   // ISO8601 时间戳

保留现有 `SyncE3dFileMsg`，通过配置决定使用哪种模式。

---

## 4. 开发阶段划分

### 阶段 0：基础设施与配置准备

**目标**：MinIO 可用，配置项就位，但业务代码暂不依赖。

- [ ] 部署 MinIO（开发环境可单节点，生产用分布式）：
  - 规划 bucket：`e3d-increments`（或 `e3d-increments-{site}`）。
  - 配置访问端点、AK/SK、TLS 等。
- [ ] 配置 bucket Notification：
  - 针对 `ObjectCreated` 事件；
  - 目标先指向开发用 HTTP 端点（后续改为 Sync Orchestrator）。
- [ ] 扩展配置文件（如 `DbOption*.toml`）：
  - `use_minio_sync: bool`（默认 false）；
  - `minio_endpoint`、`minio_access_key`、`minio_secret_key`、`minio_bucket`、`minio_region` 等字段。

### 阶段 1：站点内 `.cba → MinIO` 改造

**目标**：在单站点内打通 `.cba` 上传 MinIO，保留旧方案。

- [ ] 在 SurrealDB 中增加 `increment_jobs` 表（轻量版）：
  - 字段：`batch_id`、`sesno_start`、`sesno_end`、`db_file`、`status`、`created_at` 等。
- [ ] 在 `increment_manager.rs` 中生成 `batch_id`：
  - 例如：`{location}-{date}-{seq}` 或 `{location}-{sesno_start}-{sesno_end}-{uuid}`。
- [ ] 在 `execute_compress` 完成后新增：`upload_cba_to_minio(...)`：
  - 从配置读取 MinIO 参数；
  - 创建 `object_key = project/location/date/batch_id.cba`；
  - 设置 metadata：`batch_id`、`source_location`、`project`、`sesno_range`、`hash`；
  - 成功后更新 `increment_jobs.status = 'ARCHIVE_UPLOADED'`。
- [ ] 配置开关：
  - `use_minio_sync == false`：沿用当前写本地目录 + `SyncE3dFileMsg`；
  - `use_minio_sync == true`：在写本地目录的同时上传 MinIO（双写，为后续迁移做准备）。

### 阶段 2：Sync Orchestrator 与 `SyncE3dObjectMsg`

**目标**：把 MinIO ObjectCreated 事件转换成标准 MQTT 消息，写入中心任务表。

- [ ] 在 `mqtt_service/mod.rs` 中定义 `SyncE3dObjectMsg` 结构体及序列化：
  - 提供 `fn from_minio_event(event, targets) -> Self` 帮助函数。
- [ ] 新建或扩展 Orchestrator 模块（如 `src/sync_orchestrator.rs`）：
  - 暴露 HTTP 接口接收 MinIO Bucket Notification。
  - 解析事件 JSON，提取：`bucket`、`object_key`、`size` 和 metadata 中的 `batch_id`、`source_location`、`project`、`sesno_range`、`hash`。
  - 根据配置/拓扑计算 `target_locations`。
  - 写入 `sync_jobs` 表：
    - `batch_id`、`bucket`、`object_key`、`source_location`、`target_locations`、`status='PUBLISHED'`。
  - 构造 `SyncE3dObjectMsg` 并通过 MQTT 发布到 `Sync/E3dObject` 主题。
- [ ] 单站点验证：
  - 手动上传 `.cba` → 观察 Orchestrator 是否收到事件，并正确发布 `SyncE3dObjectMsg`。

### 阶段 3：远端站点 RemoteSyncAgent 与增量应用

**目标**：在远端站点通过 `Sync/E3dObject` + MinIO 完成本地增量应用。

- [ ] 新建 `remote_sync_agent` 模块：
  - 订阅 `Sync/E3dObject` 主题；
  - 收到 `SyncE3dObjectMsg` 后：
    - 若配置了 `target_locations`，检查本地 `location` 是否在其中；
    - 查询本地 `applied_batches`：
      - 若已有该 `batch_id`，忽略消息（幂等）；
    - 调用 MinIO：`GET bucket/object_key`，下载 `.cba` 至本地临时目录；
    - 写入本地 `increment_jobs_B`，状态 `DOWNLOADED`；
    - 调用 `E3dIncrementApplier_B::apply_increment(batch_id, cba_path)`。
- [ ] `E3dIncrementApplier_B` 逻辑：
  - 解压 `.cba`，得到元素变更列表；
  - 写入 SurrealDB B 的相关表（`element_changes` + 业务表）；
  - 调用现有 `gen_all_geos_data`/mesh 生成逻辑完成重建；
  - 写入 `applied_batches(batch_id, location, applied_at)` 标记已应用；
  - 更新 `increment_jobs_B.status='APPLIED'`。
- [ ] 可选：向 Orchestrator 发送 ACK（HTTP/MQTT），更新 `sync_jobs` 中 per-site 状态。

### 阶段 4：多站点拓扑、权限与生命周期

**目标**：支持多机组、多地域部署，完善安全与运维策略。

- [ ] 拓扑配置：
  - 在配置或 DB 中描述站点间同步关系：
    - 如：`site_a -> [site_b, site_c]`，`site_b -> []`（只从中心拉不回推）。
  - Orchestrator 根据该配置生成 `target_locations`。
- [ ] MinIO 权限与访问控制：
  - 为每个站点配置专用的 AK/SK；
  - 限制各站点只可访问特定 bucket / 前缀；
  - 配置网络（VPN/专线/防火墙）保证跨站访问安全。
- [ ] 生命周期规则：
  - 在 MinIO 为 `.cba` 设置生命周期策略（如 30/90 天删除）；
  - 为 `increment_jobs` / `sync_jobs` / `applied_batches` 设计归档或定期清理脚本。

### 阶段 5：联调、灰度与迁移策略

**目标**：在不影响现有业务的前提下逐步切换到新方案，并保留回滚路径。

- [ ] 单站点联调：
  - 在测试环境开启 `use_minio_sync=true`；
  - 验证链路：增量 → MinIO 上传 → Orchestrator → MQTT → RemoteSyncAgent → 本地应用。
- [ ] 双轨运行（迁移期）：
  - 配置系统同时：
    - 保留现有 `SyncE3dFileMsg` + 文件服务器；
    - 启用 `SyncE3dObjectMsg` + MinIO；
  - 初期只在部分站点启用 MinIO 链路，逐步扩大范围。
- [ ] 回滚策略：
  - 随时可通过配置将 `use_minio_sync` 设为 false：
    - 停止上传 MinIO / 发送 `SyncE3dObjectMsg`；
    - 回退到原有文件服务器 + `SyncE3dFileMsg` 模式；
  - MinIO 中已有对象可作为备份保留，对旧方案无负面影响。

---

## 5. 风险与注意事项

- **MinIO 作为新核心组件的可靠性**：
  - 需做好监控（磁盘、网络、延迟）、备份、灾备演练；
  - 避免单点（建议生产使用分布式部署 + 纠删码）。
- **网络与安全**：
  - 跨站点访问 MinIO 与 MQTT 时，需要 VPN/专线及严格的访问控制；
  - 对 AK/SK、配置文件中的敏感信息要做好保护。
- **幂等性**：
  - 必须依赖 `batch_id` + `applied_batches` 避免重复应用增量；
  - 消息可能重复送达或乱序，业务逻辑需保证最终一致。
- **观测性**：
  - 为 IncrementManager、Sync Orchestrator、RemoteSyncAgent、E3dIncrementApplier 增加结构化日志和 metrics；
  - 建议在 `sync_jobs` / `increment_jobs` 中记录耗时、错误信息，便于排查问题。

---

## 6. 验收标准

- 在测试环境中：
  - 手动触发多次增量，确认：
    - `.cba` 正确上传到 MinIO，metadata 完整；
    - Orchestrator 正确接收 MinIO 事件，并推送 `SyncE3dObjectMsg` 至 MQTT；
    - 远端站点能根据消息从 MinIO 下载 `.cba`，完成增量应用与模型重建；
    - 重复消息不会导致重复应用（幂等性验证）。
- 在预生产/灰度环境：
  - 多站点场景中，按拓扑配置进行增量同步；
  - 观测一段时间内无明显延迟和错误堆积；
  - 出现故障时可一键回退到旧方案。
