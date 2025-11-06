# 异地协同功能开发计划

## 背景与目标
当前前端界面已经提供“异地协同配置”入口（`app/collaboration/page.tsx`），但依赖的后端接口与站点管理能力尚未完备。目标是在现有 UI 基础上实现可用的协同环境管理、站点分配与同步操作，确保异地部署场景下的数据共享和状态监控可靠运行。

## 里程碑拆分
1. **基础连通**：补齐远端协同相关 API、环境变量配置与错误兜底，让页面能够拉取并展示真实数据。
2. **站点管理完善**：支持在协同组内增删本地/远程站点、设置主站点并保存到后端。
3. **同步控制闭环**：接入同步启动、暂停、查看记录等操作，提供实时状态反馈。
4. **可靠性与监控**：落实日志、健康检查与异常告警，保障远程协同稳定运行。

## 待办事项列表

### 配置流程
1. **准备主站点**：在目标环境配置 MQTT Broker 与数据库服务，可通过 `.env.local` 的 `NEXT_PUBLIC_API_BASE_URL` 指向该站点提供的 API。
2. **创建协同组**：在“异地协同配置”页面新建协同组，先指定主站点，系统将主站点写入 `shared_config.mqtt_primary_site_id` 并复用其 MQTT 配置。
3. **导入远程站点**：在“管理站点”弹窗粘贴远程站点的配置 URL，系统自动解析远程元数据（API 地址、鉴权等），并在 `shared_config.remote_sites` 中保存。
4. **确认拓扑关系**：通过 ReactFlow 拓扑图确认主站点与客户端站点关系，必要时调整主站点或重新导入。
5. **触发同步**：保存配置后点击“立即同步”，利用 `/api/collaboration-groups/{id}/sync` 启动一次同步任务，并在同步记录中查看结果。

### M1 基础连通（优先级 P0）
- [x] 明确并文档化需要依赖的后端接口：
  - `/api/remote-sync/envs` 系列（list/create/update/delete/activate/stop）。
  - `/api/collaboration-groups/*`（获取组、成员站点、同步记录等）。
  - `/api/remote-sites/*`（列出远程站点、连通性检测）。
- [ ] 与后端确认字段协议，补齐 `envToGroup` / `siteToRemoteSite` 映射缺失字段或调整不符字段。
- [x] 完成 `.env.local` 模板中远程协同必需变量的定义（例如 `NEXT_PUBLIC_API_BASE_URL`、MQTT/文件存储地址）。
- [x] 为 `listRemoteSyncEnvs()`、`fetchRemoteSites()` 等请求添加错误提示与重试策略，避免空白页。

### M2 站点管理完善（优先级 P1）
- [ ] 实现协同组详情侧栏/抽屉，展示组元数据、站点列表及主站点。
- [ ] 在前端调用新增的 `addSiteToGroup` / `removeSiteFromGroup` API，实现站点增删及界面状态同步。
- [ ] 支持在 `SiteSelector` 中搜索与分页本地站点，避免一次性加载过多数据。
- [ ] 保存主站点配置逻辑：前端调用后端接口更新主站点，同时处理返回错误。
- [ ] 增加远程站点编辑与连通性检测入口（调用 `updateRemoteSyncSite`、`testRemoteSiteConnection`）。

### M3 同步控制闭环（优先级 P1）
- [x] 在协同组页面加入“立即同步”“暂停同步”“查看记录”按钮，对接 `syncGroup` 与 `fetchSyncRecords`。
- [x] 展示最近同步结果、失败原因、耗时等关键信息，提供列表或时间线组件。
- [ ] 接入 WebSocket 或 SSE 通道（若后端支持），实时刷新同步状态；否则提供轮询兜底。
- [ ] 当同步失败时提供日志查看入口，与任务日志界面复用组件。

### M4 可靠性与监控（优先级 P2）
- [ ] 为协同功能追加 E2E 测试或集成测试脚本，校验创建协同组/添加站点/触发同步的完整路径。
- [ ] 建立健康检查接口（如 `/api/remote-sync/health`），在前端 Dashboard 显示连接状态。
- [ ] 结合 `NodeStatusBadge` 展示 MQTT Broker、文件服务等依赖的监控指标。
- [ ] 编写运维手册，涵盖配置步骤、常见错误处理、日志路径等信息。

## 风险与依赖
- **后端接口依赖**：需要后端团队优先实现或确认 API 规范，否则前端只能模拟数据。
- **认证与安全性**：远程站点涉及敏感配置，需确认 API 的认证/授权机制与 UI 的权限校验。
- **实时能力**：WebSocket 通道在部分部署环境可能受限，需准备轮询降级方案。

## 验收标准
- 协同列表能够展示真实环境数据，创建/删除操作立即生效并有反馈。
- 站点增删与主站点设置可持久化，刷新页面后信息准确还原。
- 同步操作可执行，前端能查看同步状态与历史记录，失败时有明确提示。
- 关键路径具备日志、监控与文档支撑，方便运维人员定位问题。

## 待完善项一览
- **接口契约确认**：与后端确认 `/api/remote-sites`、`/api/collaboration-groups` 等接口字段（如 `metadata.mqtt`、`shared_config.remote_sites`）及鉴权方案。
- **WebSocket 联调**：确认 `/ws/collaboration/groups/{id}` 的事件类型与 payload，统一 `sync_status` / `sync_record` / `remote_site_metadata` 等消息结构。
- **远程元数据校验**：导入 URL 时增加必要字段校验、时间戳检查与冲突处理策略；必要时提供手动覆盖确认。
- **批量导入与管理**：支持一次导入多条 URL、展示/刷新已导入站点列表并提供删除能力。
- **日志联动**：同步失败时快速跳转至任务日志或弹出关键日志，提供一键重试。
- **敏感信息治理**：对 `auth_token`、MQTT 密码等字段做遮罩/复制，储存方式与后端协同加密。
- **自动化测试**：补充单元测试（解析元数据、共享配置）和 E2E 场景（创建协同组 → 导入站点 → 同步验证）。

## API 接口约定

| 功能 | Method & Path | 请求体（摘要） | 关键响应字段 |
| --- | --- | --- | --- |
| 列出协同环境 | `GET /api/remote-sync/envs` | 无 | `id`, `name`, `status`, `mqtt_host`, `location` |
| 创建协同环境 | `POST /api/remote-sync/envs` | `name`, `mqtt_host`, `mqtt_port`, 可选 `mqtt_user`/`file_server_host` | `id`, `status` |
| 获取协同组 | `GET /api/collaboration-groups/{groupId}` | 无 | `id`, `name`, `group_type`, `primary_site_id`, `sync_strategy` |
| 组内站点列表 | `GET /api/collaboration-groups/{groupId}/sites` | 无 | `items[].site_id`, `items[].name`, `items[].is_primary` |
| 添加站点 | `POST /api/collaboration-groups/{groupId}/sites` | `{ "site_id": "xxx" }` | `status` |
| 移除站点 | `DELETE /api/collaboration-groups/{groupId}/sites/{siteId}` | 无 | `status` |
| 更新主站点 | `PUT /api/collaboration-groups/{groupId}` | `{ "primary_site_id": "xxx" }` | 更新后的 `item` |
| 获取同步状态 | `GET /api/collaboration-groups/{groupId}/sync-status` | 无 | `status`（`running`/`paused`/`failed`...） |
| 触发同步 | `POST /api/collaboration-groups/{groupId}/sync` | `{ "force": true }`（可选） | `status`, `sync_id` |
| 暂停同步 | `POST /api/collaboration-groups/{groupId}/pause` | 无 | `status` |
| 同步记录 | `GET /api/collaboration-groups/{groupId}/sync-records` | 可选 `page`/`limit` | `items[].status`, `items[].sync_type`, `items[].started_at`, `items[].completed_at`, `items[].error_message` |
| 远程站点列表 | `GET /api/remote-sites` | 无 | `items[].id`, `items[].name`, `items[].api_url`, `items[].status` |
| 测试远程站点 | `POST /api/remote-sites/{id}/test` | 无 | `connection_status`, `latency_ms`, `error` |
| 导入远程站点 | `POST /api/remote-sites` | `name`, `api_url`, 可选 `auth_token`、`metadata` | `item.id`, `item.api_url`, `item.metadata` |
| WebSocket 推送 | `WS /ws/collaboration/groups/{groupId}` | 订阅，不需请求体 | `type`（`sync_status`/`sync_record`/`remote_site_metadata` 等），对应 `status`、`record`、`metadata` |

> 建议后端在所有接口中统一返回 `error` 字段以便前端展示，同时保证时间字段为 ISO8601 字符串，数据量（如 `data_size`）使用字节整数。对于远程站点导入，建议 `metadata` 包含 `mqtt` 信息（`host`、`port`、`username`、`password`）与 `source_url`，便于前端自动填充 `shared_config.remote_sites`。若提供 WebSocket 推送，请使用 `/ws/collaboration/groups/{groupId}`，消息格式建议包括 `type`（如 `sync_status`、`sync_record`、`remote_site_metadata`）与对应数据。
