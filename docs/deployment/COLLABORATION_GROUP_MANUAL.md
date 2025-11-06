# 协同分组初始实施手册

## 1. 背景与目标

- 针对 LiteFS/MQTT 异地协同场景，先通过“手动建组 + 站点同步”方式完成首轮上线。
- 以现有 `remote_sync_envs` / `remote_sync_sites` 表为基础，统一术语与操作路径，降低前端实现复杂度。
- 为后续自动化策略（定时同步、冲突处理、节点扩容）预留字段与流程接口。

## 2. 核心概念对齐

| 名称 | 说明 | 数据来源 |
| ---- | ---- | ---- |
| 协同分组（Collaboration Group） | 对应一个远程环境，负责承载一组站点的配置同步与数据交换 | `remote_sync_envs` |
| 站点（Remote Site） | 分组内的具体节点，通常是一个地理位置或业务域的部署实例 | `remote_sync_sites` |
| 主站点（Primary Site） | 分组中负责产出基线数据的节点，向其他站点推送增量 | 分组属性 `primary_site_id`（待扩展） |
| 从站点（Secondary Site） | 消费主站点增量的节点，可按需执行反向同步 | `remote_sync_sites` + UI 标识 |

> 现阶段 `CollaborationGroup` 类型通过 `envToGroup` 映射自远程环境，后端不需要新增表结构即可完成 MVP。

## 3. 操作流程概览

```
┌──────────┐    ┌───────────────┐    ┌────────────────┐    ┌────────────────┐
│ 准备信息 │ -> │ 创建协同分组   │ -> │ 同步/补录站点   │ -> │ 指定主从关系     │
└──────────┘    └───────────────┘    └────────────────┘    └────────────────┘
     ↑                                                       │
     └───────────────文档化配置、校验字段、版本记录─────────────┘
```

1. **准备信息**：汇总 MQTT、文件服务、地区编码、负责 DB 列表等基础参数，确认站点 IP/域名与访问凭证。
2. **创建协同分组**：前端提交 `POST /api/remote-sync/envs`，写入 SQLite；创建成功后 UI 映射为 `CollaborationGroup` 实例。
3. **同步/补录站点**：读取 `GET /api/remote-sync/envs/{id}/sites`，逐项核对；缺失站点使用 `POST` 补充，确保 `env_id` 对齐。
4. **指定主从关系**：在分组详情页选择主站点，记录 `primary_site_id` 与显式标签；其余站点标记为从站并配置同步策略。

## 4. 数据映射与字段校验

### 4.1 分组（远程环境）

| 字段 | 说明 | 校验建议 |
| ---- | ---- | ---- |
| `name` | 分组名称 | 唯一且 < 50 字符 |
| `mqtt_host` / `mqtt_port` | MQTT Broker 地址 | host 为 FQDN/IP，端口默认 1883；支持 TLS 时预留 8883 |
| `file_server_host` | `.cba` 文件下载地址 | 需可达且以 HTTP/HTTPS 开头 |
| `location` | 地区代号 | 与 `DbOption.toml` 中保持一致，如 `bj`/`zz` |
| `location_dbs` | 负责 DB 列表 | 逗号分隔数字，保存前去重、排序 |
| `reconnect_initial_ms` / `reconnect_max_ms` | MQTT 重连退避区间 | 默认 `1000/60000`，需满足 `initial <= max` |

> 若需要扩展同步策略，可在 `shared_config` 中追加 JSON 配置，并在 `groupToEnvPayload` 内序列化。

### 4.2 站点

| 字段 | 说明 | 校验建议 |
| ---- | ---- | ---- |
| `name` | 站点名称 | 与物理部署一致，便于追踪 |
| `http_host` | 站点对外 HTTP 服务 | 包含协议头，建议使用域名 |
| `location` | 站点地区 | 对应协同地区编码 |
| `dbnums` | 同步 DB 列表 | 仅包含该站点需从主站点获取的 DB |
| `is_local`（可选扩展） | 是否本地站点 | 有助于 UI 显示和权限隔离 |

## 5. 手动创建协同分组指南

1. 进入前端 “异地协同配置” 页面，点击 **创建协同组**。
2. 填写基础信息：名称、MQTT 地址、文件服务、地区、负责 DB 数组。
3. 提交后调用 `createRemoteSyncEnv`，若成功返回，新分组会出现在列表中。
4. 打开分组详情，触发 API `listRemoteSyncSites`，校验站点完整性：
   - 若已有站点记录：检查 `http_host`/`dbnums` 等字段，必要时点击编辑。
   - 若缺失站点：点击 **新增站点**，补录必要信息并保存。

## 6. 指定主从站点

1. 在分组详情页添加主站点选择器（单选），默认未选时提示“请指定主站点”。
2. 选择后调用后台（推荐新增 `PUT /api/remote-sync/envs/{id}/primary-site`）写入 `primary_site_id`。
3. UI 对主站点添加徽标，并在站点列表中提供“设为主站点”按钮。
4. 记录在案：更新 `docs`/`README_web_server.md`，注明主站点负责推送增量，切换需重新确认 MQTT 权限与文件服务写权限。

## 7. 运维与记录

- 建议将每次分组/站点变动在 `deployment_sites.sqlite` 外另存变更记录（例如 `docs/deployment/logs/`），便于回溯。
- 当主站点切换或增删站点时，需同步更新 `DbOption.toml` 并触发 `activate_env`，确保 watcher/MQTT 正确重启。
- 定期执行 `GET /api/remote-sync/runtime`（待实现）核查运行状态，确认 MQTT 连接/同步队列正常。

## 8. 后续演进建议

- **字段扩展**：为 `remote_sync_envs` 加入 `status`、`sync_mode` 等字段，以支持自动化策略。
- **冲突处理**：引入 `conflict_resolution` 配置，前端提供选择面板（主站优先/最新优先/人工）。
- **可视化增强**：将主从关系、站点拓扑以图形化呈现，结合 `SyncControlCenter` 的指标面板。
- **自动化脚本**：开发脚本从现有部署信息批量导入站点，减少手动录入错误。

---

> 在手动阶段保持流程透明、记录完备，为后续自动化接管提供可靠的配置基线。

