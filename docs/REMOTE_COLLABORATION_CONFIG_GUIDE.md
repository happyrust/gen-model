# 异地协同部署配置指南

> 目标：最少手动输入，快速完成主站点与远程站点的协同配置。

## 一、前置准备

1. **环境变量**
   - `NEXT_PUBLIC_API_BASE_URL`：指向主站点后端网关。
   - 可选 `NEXT_PUBLIC_XKT_API_BASE_URL`：若 XKT 服务独立部署。
   - 可选 `NEXT_PUBLIC_COLLAB_WS_ENABLED`（默认为 `false`）：开启后将使用 WebSocket `/ws/collaboration/groups/{id}` 实时刷新同步状态。
2. **主站点基础服务**
   - MQTT Broker（建议记录 host/port/user/password）。
   - 数据库服务（例如 SurrealDB），用于验证同步结果。

## 二、配置流程

1. **创建协同组**
   - 进入「异地协同配置」→「创建协同组」。
   - 先选主站点名称（本地站点）或通过 URL 导入新站点（见步骤 2）。
   - 保存后，主站点会写入 `shared_config.mqtt_primary_site_id` 并持有 MQTT 配置。

2. **导入远程站点（推荐）**
   - 打开「管理站点」，使用「通过 URL 导入远程站点」：
     - URL 应返回 JSON，包含 `name` / `api_url`（可选 `auth_token` 等）。
     - 推荐同时包含 `mqtt` 信息（`host`、`port`、`username`、`password`）和 `source_url`；前端会保存在 `shared_config.remote_sites` 中。
     - 前端调用 `createRemoteSite` 自动登记，并刷新站点列表。
   - 新站点默认加入协同组，若未设置主站点，会自动选中第一个导入站点作为主站点候选。
   - 也可以在协同组详情页的拓扑图中点击主站点，弹出“快速导入”窗口直接录入 URL。

3. **确定主站点**
   - 在站点列表选择一个主站点：
     - 主站点需要配置 MQTT，其他站点会通过 `shared_config.mqtt_client_site_ids` 自动引用。
     - 保存时前端会写入 `shared_config.remote_sites`，记录各站点的来源 URL 与元数据。

4. **确认拓扑**
   - 协同组详情页展示 ReactFlow 拓扑图：
     - 主站点居中，其他站点围绕，通过箭头指示 MQTT 客户端关系。
     - 检查无误后点击「保存」。

5. **触发同步**
   - 在同一页面点击「立即同步」，拉起一次全量同步。
   - 同步状态与历史可在「同步记录」中查看，并支持轮询刷新。

## 三、远程站点元数据格式

推荐远程服务返回如下 JSON（字段可按需扩展）：

```json
{
  "name": "上海-远程站点",
  "api_url": "https://remote.example.com/api",
  "auth_token": "Bearer token-123",
  "mqtt": {
    "host": "mqtt.remote.example.com",
    "port": 1883,
    "username": "remote-user",
    "password": "secret"
  },
  "file_server": {
    "url": "https://remote.example.com/files",
    "username": "fs-user",
    "password": "fs-secret"
  },
  "source_url": "https://remote.example.com/meta.json",
  "extras": {
    "location": "上海数据中心"
  }
}
```

> 导入成功后，前端会将上述信息保存到 `shared_config.remote_sites[site_id]`，并在下一次打开协同组详情时直接复用。

## 四、快速配置技巧

- **URL 自动填充**：后端统一提供站点元数据接口，包含 API 地址与 MQTT/Broker 信息，可显著节省手动输入。
- **主站点优先**：系统强制要求主站点，使 MQTT 配置集中管理，避免重复填写。
- **远程站点复用**：导入时会将元数据写入 `shared_config.remote_sites`，下次仅需点击刷新即可更新。
- **拓扑直观认知**：ReactFlow 拓扑帮助运营人员确认主从关系，提升配置正确率。
- **同步闭环**：保存配置后立即同步，并在记录中查看/重跑任务，缩短配置验证周期。

## 五、常见问题

| 问题 | 解决方案 |
| ---- | -------- |
| 导入 URL 失败 | 检查 URL 是否可访问、返回合法 JSON。若需认证，请在后端接口中返回 `auth_token`。 |
| 主站点配置错误 | 重新选择主站点后保存即可；MQTT 配置会覆盖。 |
| 远程站点列表不刷新 | 确认导入成功后点击「刷新站点」，或重新打开协同组详情，列表会自动重载。 |
| 同步状态不更新 | 触发同步后等待轮询刷新；如希望实时推送，请在 `.env.local` 中设置 `NEXT_PUBLIC_COLLAB_WS_ENABLED=true` 并确保后端提供 `/ws/collaboration/groups/{id}`。 |

## 六、后续迭代建议

- 引入 WebSocket/SSE 实时刷新同步状态。
- 支持批量导入站点 URL 列表。
- 在拓扑图节点上展示详细元数据及快捷操作（重试、删除），并在同步失败时联动任务日志支持一键重试。

通过以上流程，运维人员仅需准备主站点信息与若干远程站点 URL，即可完成异地协同环境的快速配置。欢迎在 `docs/REMOTE_COLLABORATION_DEV_PLAN.md` 中查看后续计划与 API 约定。*** End Patch
