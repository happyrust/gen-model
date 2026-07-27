# Web 服务接口设计（axum REST + WebSocket）

- 状态：已评审通过（2026-07-26），按里程碑 M1→M4 实施；实现见 `src/web_service/`
- 日期：2026-07-26
- 决策：方案选型已定为 **axum 0.8.9 REST + WebSocket**（用户决策，见会话 gen-model-49）
- 相关规格：`docs/specs/manual-model-update.md`（手动更新语义）、`CONTEXT.md`（术语表）

> **修订注记（2026-07-27，ADR-011 合流落地）**：手动与自动已合流到一条数据批次队列，
> 本文以下与实现不符的点以 ADR-011 与 `plant-ui/docs/plans/task-queue-rollout.md`
> 第九节为准，全文修订随服务端 5–9 项（暂停端点 / health / dbnums 扩展）一并做：
> ① `/update/execute` 不再 409/422，一律 202 返回入队回执
> `{scanned, enqueued:[{task_id,dbnum,position,…}], merged, already_covered, blocked, up_to_date, warnings}`；
> ② `TaskState` 新增 `queued`，kind 由 `manual_update` 改为 `data_batch` / `room_recalc`，
> `TaskEntry` 增加 `dbnum / db_type / start_sesno / end_sesno / started_at / units_done / total_units`；
> ③ `increments` 主题与 `incr_applied` 事件已删除（从未有过消费者），
> 任务注册表搬至 feature 无关层（`data_interface/task_registry.rs`），容量 1000、分层保留。

## 1. 背景与目标

`aios-database` 目前是长驻进程（`run_app -> run_cli -> async_watch`），增量数据落 SurrealDB，MQTT 仅做跨节点文件同步，**没有任何面向前端的服务层**。本设计为前端（浏览器 3D viewer / 桌面端）提供：

1. **触发模型生成**：手动更新预览与执行、按需生成单个构件模型；
2. **增量更新信息**：更新任务进度实时推送、增量结果（IncrResult 摘要）、水位与待重试单元查询。

非目标（本期不做）：鉴权与多租户、TLS、模型网格数据本身的下发（仍走 SurrealDB / 文件服务器现有通道）、MQTT 通道改造。

## 2. 总体架构

```
前端 --HTTP(JSON)--> axum Router --+--> TaskRegistry（任务注册表, DashMap）
     <--WebSocket--  /api/v1/ws    |         | 状态查询
                          ^        |         v
                          |   tokio::spawn(execute_manual_update(progress))
                     broadcast <---+-- ManualUpdateProgress 回调 / IncrResult 桥接
```

- 新增模块 `src/web_service/`，feature flag **`http_api`**（默认不启用，`console` feature 可包含它）。
- 服务与 `async_watch` 在 `run_cli` 内并行：`tokio::join!(mgr.async_watch(), web_service::serve(state))`，互不阻塞；`http_api` 未启用时行为与现在完全一致。
- **共享状态 `AppState`**：`Arc<AiosDBManager>` + `TaskRegistry` + `tokio::sync::broadcast::Sender<WsEvent>`（容量 1024，慢消费者掉线自补）。
- 事件源两处接入，均为已有扩展点、不改领域逻辑：
  - 手动路径：`execute_manual_update(project, Some(progress))` 的 `ManualUpdateProgress` 回调（代码注释即写明"前端把事件转发进自己的任务状态"），回调内转发到 broadcast；
  - 自动路径（`sync_live=true`）：在 `sync_publisher` 发布 `IncrResult` 的同一位置挂钩，广播 `incr_applied` 摘要事件。

## 3. 通用约定

- Base path：`/api/v1`，请求/响应均为 `application/json; charset=utf-8`。
- refno 一律用 PDMS 字符串形式 `"a/b"`（如 `"24381/100817"`），与领域结构体序列化一致。
- 枚举值一律 `snake_case`，直接复用领域结构体的 serde 输出（`ManualUpdatePreview`、`ManualUpdateResult`、`OnDemandModelResult` 等**原样透传**，不做二次映射——单一权威，避免字段漂移）。
- 错误响应统一结构：

```json
{ "code": "conflict", "message": "项目 HD 已有手动更新任务正在执行", "detail": null }
```

| HTTP | code | 场景 |
| --- | --- | --- |
| 400 | `bad_request` | 参数缺失 / refno 格式非法 |
| 404 | `not_found` | task_id 不存在 |
| 500 | `internal` | 领域层 `anyhow::Error` |

> 409 `conflict` 与「`sync_live=true` 拒手动」的 422 随 ADR-011 §12 退役：数据批次
> 由单 worker 天然串行，执行请求一律 202 入队（见 §4.3）。`model/ensure` 的
> 422 `container` / `precondition` 不受影响。

## 4. REST 接口定义

### 4.1 `GET /api/v1/health`
存活探针。响应：`{ "status": "ok", "project": "HD", "sync_live": false, "version": "0.1.3" }`。

### 4.2 `POST /api/v1/update/preview` — 手动更新预览
- 映射：`AiosDBManager::preview_manual_update(project)`（只读，可能刷新扫描观察字段，故用 POST）。
- 请求：`{ "project": "HD" }`（缺省取当前 `db_option` 项目）。
- 响应 200：`ManualUpdatePreview` 原样序列化：

```json
{
  "project": "HD",
  "dbnums": [{
    "dbnum": 7997, "db_type": "DESI", "file_name": "des000.db",
    "file_path": "...", "applied_sesno": 80, "file_latest_sesno": 82,
    "sessions": [], "net_added": 3, "net_modified": 5, "net_deleted": 1,
    "model_affecting": 6, "units": [], "zones": [],
    "anomaly": null, "blocked": false
  }],
  "pending_model_retries": [{ "dbnum": 7997, "root_refno": "24381/100817", "noun": "BRAN", "source_end_sesno": 81, "attempts": 2, "last_error": "..." }],
  "warnings": [],
  "up_to_date": false
}
```
- 错误：422（`sync_live=true`）、500（项目目录不存在等）。

### 4.3 `POST /api/v1/update/execute` — 扫描 + 入队（ADR-011 合流后）
- 映射：`AiosDBManager::enqueue_manual_update(project)`；执行由进程内唯一的数据批次
  worker 从队列取走（`batch_worker`，与 `async_watch` 自动发现共用同一条队列与冻结语义）。
- 请求：`{ "project": "HD" }`。
- 响应 202（入队回执，rollout 第八节第 7 条）：

```json
{
  "project": "HD",
  "scanned": 3,
  "enqueued": [{ "task_id": "db-20260727-210301-7f3a", "dbnum": 7997, "db_type": "DESI",
                  "position": 1, "start_sesno": 85, "end_sesno": 92 }],
  "merged": [],
  "already_covered": [],
  "blocked": [{ "dbnum": 8003, "reason": "文件回退或被替换（file_latest_sesno=812 < applied_sesno=1005），已阻断" }],
  "up_to_date": 2,
  "warnings": []
}
```

- 语义：手动触发**不插队**（ADR-011 §6）——对已在队里的库只是并入会话（`merged`），
  它剩下的唯一新意义是「别等下一个 30s 轮询」。阻断与排除的库压根不入队（`blocked`）。
  `sync_live=true` 时同样可用（422 已退役）。
- 每个数据批次是一条 `kind = "data_batch"` 的任务：`queued -> running -> succeeded|partial|failed`；
  进度经 WebSocket 推送（第 5 节），终态 result 为 `{ project, status, batch: DataBatchResult,
  units: [ModelUnitResult], warnings }`（一行任务一个批次，「一次运行」的复数形态随 ADR-011 退役）。

### 4.4 `GET /api/v1/tasks` 与 `GET /api/v1/tasks/{id}`
- 列表支持 `?state=running&kind=manual_update&limit=50`（内存中保留最近 200 条，进程重启即清空——durable 状态以 `manual_model_pending` 表与水位为准，本表只是 UI 视图）。
- 详情响应：

```json
{
  "task_id": "mu-20260726-100301-7f3a", "kind": "manual_update",
  "state": "succeeded", "project": "HD",
  "created_at": "2026-07-26T10:03:01+08:00", "finished_at": "2026-07-26T10:04:12+08:00",
  "events_seen": 42,
  "result": { "project": "HD", "status": "success", "batches": [], "units": [], "warnings": [] }
}
```

### 4.5 `POST /api/v1/model/ensure` — 按需生成单构件模型（同步）
- 映射：`AiosDBManager::ensure_model_generated(refno, force)`；内部已有 per-生成根锁与二次检查，天然幂等，可并发。
- 请求：`{ "refno": "24381/100677" }`，可选 `"force": true`。
- 响应 200：`OnDemandModelResult` 原样，含 `generation_root`、`model_instance_count`（画得出来的实例数）与 `generated_instance_count`（生成写出的实例数，含画不出来的）。`status` 三种：
  - `AlreadyAvailable` — 已经有画得出来的实例，没跑生成；
  - `Generated` — 这次跑了生成，拿到了画得出来的实例；
  - `NoRenderableGeometry` — 生成写出了实例，但一条都画不出来（`model_available` 为 false）。这是数据的终局不是失败，所以走 200 而不是 5xx：底下的几何不修好，重发只会把同样的生成再跑一遍。
- `force` 只给「人明确要求重生成」用（S4-C 的重试）：它跳过上面两种短路，无条件重跑一次生成。显示补齐**不要**传，否则每显示一次就重生成一次。
- 解析不出生成根时按缘由分型，客户端对三种的出路各不相同，不要压成一个 `internal`：
  - `422 container` — WORL / SITE / ZONE，按契约恒被拒绝做生成根。**这不是失败**：客户端应展开一层，对子节点逐个 ensure（ADR-0009）。
  - `404 not_found` — 库里没有这个 refno。
  - `422 precondition` — 构件在、也不是容器，但向上找不到任何合法生成根。
  - `400 bad_request` — refno 连格式都不对。
- 单根生成通常秒级，同步等待即可；HTTP 层设 120s 超时，超时不取消后台生成，前端可重发（幂等）。实测 AMS 8000 的 SUPPO 与风管 BRAN 冷生成要 99–104s，贴着这条线，客户端超时不能设得比它更短。

### 4.6 `GET /api/v1/update/pending-units` — 待重试模型单元
- 映射：`load_pending_model_units()`，响应 `{ "units": [...] }`，元素为 `PendingModelUnit` 原样。

### 4.7 `GET /api/v1/dbnums` — 水位状态
- 映射：`DbnumState::list_registered()`，响应每个 dbnum 的 `db_type / file_name / file_path / applied_sesno / file_latest_sesno`，前端据此展示"是否有待更新会话"。

## 5. WebSocket 协议（`GET /api/v1/ws`）

### 5.1 信封
服务端到客户端统一信封（`type` 判别）：

```json
{ "type": "task_progress", "seq": 17, "ts": "2026-07-26T10:03:05+08:00", "task_id": "mu-...", "payload": {} }
```

`seq` 为连接内单调递增序号，仅用于客户端探测丢包（broadcast 慢消费者被跳过时 seq 出现空洞，客户端应走 5.4 节补偿）。

### 5.2 客户端到服务端

| 消息 | 说明 |
| --- | --- |
| `{ "type": "subscribe", "topics": ["tasks"] }` | 订阅主题；默认订阅 `tasks`（`increments` 已删，见顶部修订注记）|
| `{ "type": "unsubscribe", "topics": [...] }` | 退订 |
| `{ "type": "ping" }` | 心跳，服务端回 `pong` |

### 5.3 服务端到客户端事件

| type | 主题 | payload |
| --- | --- | --- |
| `task_started` | tasks | `{ task_id, kind, project }` |
| `task_progress` | tasks | `ManualUpdateEvent` 原样（`kind: data_batch_started / data_batch_finished / model_unit_started / model_unit_finished`，两阶段、无百分比，与规格一致）|
| `task_finished` | tasks | `{ task_id, state, result: ManualUpdateResult }` |
| ~~`incr_applied`~~ | ~~increments~~ | **已删（ADR-011 合流）**：自动路径的批次与手动同走 tasks 主题的 `task_started / task_progress / task_finished` |
| `pong` | 无 | `{}` |

### 5.4 心跳与重连语义
- 客户端每 30s `ping`；服务端 90s 无任何入站消息主动断开。
- **事件不重放**：重连后客户端先 `GET /tasks?state=running` + `GET /dbnums` 对齐状态，再订阅增量事件。运行中任务的既往进度以任务详情里的累计计数为准（`events_seen` 及 result 内已完成批次/单元），不追发历史事件。

## 6. 任务模型与并发（ADR-011 合流后）

- 注册表住在 feature 无关层（`data_interface::task_registry`，进程级单例）；`web_service`
  只是它的 HTTP 视图。`TaskEntry { task_id, kind, state, project, created_at, started_at?,
  finished_at?, dbnum?, db_type?, start_sesno?, end_sesno?, units_done?, total_units?,
  events_seen, result? }`——`created_at` 是**入队时刻**，`started_at` 是开跑时刻，
  「已排」与「已用」是两个起点。
- 状态机：`queued -> running -> succeeded | partial | failed`。kind 三种：
  `data_batch`（一行 = 一个数据批次；同一 dbnum 至多两行——运行中一行 + 排队中一行）、
  `room_recalc`（队列跑空时收的一轮房间收敛，创建即 running）、`manual_update`（已退役，
  不再产生新行）。
- TaskId 格式 `db-{yyyyMMdd-HHmmss}-{4位随机hex}`（数据批次）/ `room-…`（房间轮）。
- 保留策略（ADR-011 §11）：内存上限 1000；queued / running 永不剔除；每个 dbnum 保留
  最近一条终态；其余按全局最老终态先剔。重启即清空，队列由 `init_watcher` 重扫水位重建
  （界面须说明「这是重建的队列」）。
- 并发约束汇总：数据批次由**单 worker** 串行消费（互斥是调度器的性质，HTTP 层不再有
  409 预检）；按需生成 per-生成根互斥（既有 `GENERATION_LOCKS`），与批次执行可并发。

## 7. 配置与依赖

- `DbOption` 新增（`DbOption.toml`）：

```toml
# Web 服务监听地址；不配置则即使编译了 http_api 也不启动
http_api_addr = "0.0.0.0:8020"     # 评审决议：局域网可访问；8009/8010/1883/8000 已被占用，避开
http_api_cors = ["*"]              # 开发期放开，上线前收敛为前端 origin
```

- 依赖（仅 `http_api` feature 引入）：

```toml
axum = { version = "0.8.9", features = ["ws"], optional = true }
tower-http = { version = "0.6", features = ["cors", "trace"], optional = true }
http_api = ["dep:axum", "dep:tower-http"]
```

tokio/serde/serde_json 均已存在，无其他新增。

## 8. 实施里程碑（评审通过后）

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| M1 | 脚手架：`web_service` 模块、AppState、`/health`、`/update/preview`、`/dbnums` | curl 全通，`http_api` 关闭时零影响 |
| M2 | `/update/execute` + TaskRegistry + WS（subscribe/进度/finished） | 前端可见两阶段进度直至 `ManualUpdateResult` |
| M3 | `/model/ensure`、`/update/pending-units`、自动模式 `incr_applied` 桥接 | 按需生成幂等；watch 模式下增量事件可见 |
| M4 | CORS/超时/日志（tower-http trace 接入现有 tracing）、联调收尾 | 前端跨域可用，异常路径错误码符合第 3 节 |

## 9. 评审决议（2026-07-26）

1. **监听地址与端口**：`0.0.0.0:8020`，供局域网前端访问（已写入 `DbOption.toml`）。
2. **鉴权**：本期不做；前端部署到共享环境前可追加最简 token 头校验。
3. **`incr_applied` 粒度**：仅摘要（dbnum/条数/会话号）；前端需要逐 refno 明细（局部刷新）时再扩展 payload。
4. **任务历史持久化**：仅内存保留最近 200 条（durable 语义由水位 + pending 表承担），不做持久化。
