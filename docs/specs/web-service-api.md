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
前端 --HTTP(JSON)--> axum Router ---> 数据批次调度器 ---> 批次 worker
     <--WebSocket--  /api/v1/ws          |                 |
                          ^              v                 v
                          +-------- TaskRegistry   model_update_pending
                                                           |
                                                           v
                                                共享模型生成执行器
                                              （按需 ensure 也走这里）
```

- 新增模块 `src/web_service/`，feature flag **`http_api`**（默认不启用，`console` feature 可包含它）。
- 服务与 `async_watch` 在 `run_cli` 内并行：`tokio::join!(mgr.async_watch(), web_service::serve(state))`，互不阻塞；`http_api` 未启用时行为与现在完全一致。
- **共享状态 `AppState`**：`Arc<AiosDBManager>` + `TaskRegistry` + `tokio::sync::broadcast::Sender<WsEvent>`（容量 1024，慢消费者掉线自补）。
- 手动提交与自动 watcher 只负责把数据范围交给同一调度器；进度与终态统一由批次 worker
  更新 `TaskRegistry` 并广播 tasks 事件（ADR-011 2026-08-09 修订：一个派发器 + 至多
  `data_batch_workers` 个在飞批次，默认 1、上限 8，同 dbnum 恒串行、特殊批次独占），
  不再保留 HTTP 直跑或 `incr_applied` 旁路。
- 自动、手动、级联补偿与按需 ensure 产生的模型工作统一进入 durable pending，并由共享
  执行器完成加锁、生成、结果写入和 revision 收口。

## 3. 通用约定

- Base path：`/api/v1`，请求/响应均为 `application/json; charset=utf-8`。
- refno 一律用 PDMS 字符串形式 `"a/b"`（如 `"24381/100817"`），与领域结构体序列化一致。
- 枚举值一律 `snake_case`，直接复用领域结构体的 serde 输出（`ManualUpdatePreview`、`ManualUpdateResult`、`OnDemandModelResult` 等**原样透传**，不做二次映射——单一权威，避免字段漂移）。
- 错误响应统一结构：

```json
{ "code": "ref0_affiliation_conflict",
  "message": "Ref0 24381 同时归属 dbnum 7997 与 8001", "detail": null }
```

| HTTP | code | 场景 |
| --- | --- | --- |
| 400 | `bad_request` | 参数缺失 / refno 格式非法 |
| 404 | `not_found` | task_id 不存在，或模型依赖查询成功且 refno 确实不存在 |
| 409 | `ref0_affiliation_conflict` | 同一 Ref0 同时归属多个 dbnum；需修正项目文件 |
| 503 | `ref0_affiliation_unavailable` | 合法 Ref0 当前无法解析到所属 dbnum；模型任务保留待重试 |
| 503 | `model_dependency_unavailable` | 数据库查询、项目文件读取或 locator 临时不可用；可重试 |
| 500 | `generation_failed` | 确定性的解析、数据损坏或模型生成失败；pending 保留 |
| 500 | `internal` | 未分类的服务端缺陷 |

> 旧的通用 409 `conflict` 与「`sync_live=true` 拒手动」的 422 随 ADR-011 §12 退役：数据批次
> 由派发器统一出队（默认单 worker 串行，ADR-011 2026-08-09 修订），执行请求一律 202 入队
> （见 §4.3）。`model/ensure` 的
> 422 `container` / `precondition` 以及归属不变量被破坏时的
> `409 ref0_affiliation_conflict` 不受影响。

## 4. REST 接口定义

### 4.1 `GET /api/v1/health`
存活探针。响应：

```json
{ "status": "ok", "project": "HD", "sync_live": false, "version": "0.1.3",
  "started_at": "2026-07-27T21:02:11+08:00",
  "queue_paused": false, "static_assets": false,
  "initialization": {
    "status": "running", "epoch_id": 7, "current_phase": "catalogue",
    "data_ready": false, "model_ready": false, "model_in_flight": false,
    "phases": {}, "blockers": [], "shadowed": []
  } }
```

- `started_at`：进程启动时刻。队列不持久、重启由重扫重建（ADR-011 §4），界面靠它
  说出「服务 xx:xx 重启过，这条队列是按水位重建的；排队时长从重启起算」。
- `gen_spatial_tree` 字段已于 2026-08-07 退役：空间/房间计算恒开启，界面不再需要
  「房间增量没开」的降级文案（历史语义见 ADR-011 §8）。
- `queue_paused`：随 §4.8 的暂停接口变化；重启后按持久化标志恢复。
- `initialization`：ADR-025 的当前 epoch、严格数据阶段、数据/模型门、逐阶段计数、
  blocker 与被项目优先级遮蔽的 DICT/CATA。`data_ready=false` 时模型写保持待处理。
- `static_assets`：当前是否找到可服务的前端资源目录。`false` 只表示 UI 静态资源不可用，
  不降低 REST/WS 与增量 worker 的健康状态。
- `ref0_affiliation_conflicts`：locator 构建时发现的冲突 Ref0 数量；非零时只阻断命中这些
  Ref0 的工作，不代表整个项目停止服务。
- `sul_db.endpoint`（2026-08-12 新增）：本服务连接的 SurrealDB 端点（配置的
  `v_ip:v_port` 原样字符串）。`aios_db.full_init` 的跨部署互踩探测靠它区分
  「同名工程、不同库」的隔离沙箱与真共享一个库的部署——老版本服务端没有该键时，
  探测端退回只按 `project` 保守判。`sul_db` 其余五键（connected / ping_ms /
  disconnects_total / last_disconnect_at / last_disconnect_error）语义不变。

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
    "applied_sesno_time": "2026-07-30T18:24:07+00:00",
    "file_latest_sesno_time": "2026-08-07T14:10:33+00:00",
    "sessions": [], "net_added": 3, "net_modified": 5, "net_deleted": 1,
    "model_affecting": 6, "units": [], "zones": [],
    "sites": [{
      "site_refno": "24381/2", "name": "/1WCC-PIPE",
      "added": 2, "modified": 3, "deleted": 1, "moved_in": 0, "moved_out": 0,
      "model_affecting": 5, "units": []
    }],
    "anomaly": null, "blocked": false
  }],
  "pending_model_retries": [{
    "action": "regen_root", "target_refno": "24381/100817", "dbnum": 7997,
    "revision": 4, "noun": "BRAN", "source_dbnum": 7997, "source_end_sesno": 81,
    "attempts": 2, "last_error": "..."
  }],
  "shadowed": [{
    "project": "ZDJ", "dbnum": 7001, "file_path": "...",
    "selected_project": "AvevaCatalogue"
  }],
  "warnings": [],
  "up_to_date": false
}
```
- 错误：500（项目目录不存在等）。`sync_live=true` 时同样可用（ADR-011 §12 合流：
  预览与数据批次并发时「待应用」可能偏大——正在被应用的会话也会算进去，界面按
  队列快照里的运行中批次数标注「N 个库正在应用，数字可能偏大」。
- ADR-020（2026-08-07）新增三个字段，全部 `serde(default)` 向后兼容：
  `sites[]`（净变化按最近 SITE 祖先分桶，`ZoneSummary` 同款做法；空 `site_refno` =
  「SITE 归属未知」桶；S2-G 预览树的顶层语言）；`applied_sesno_time` /
  `file_latest_sesno_time`（**那个会话在 E3D 里被写入的时刻**，RFC3339，同一把尺子，
  相减 = 文件里没被吸收的时间跨度；从未应用 / 会话页读不到 → `null`）。
  预览请求**不变**：预览永远全范围扫描，勾选是 execute 的事。
- ADR-031（2026-08-18）统一净窗口口径：`sessions[]` 来自本次冻结范围内的会话页
  清单，按 sesno 升序去重，不能从净操作 key 反推（所以空保存/自我抵消会话仍可见）；
  `warnings[0]` 固定自报“ADR-031 单一口径”的净窗口，并携带终稿不可解析、不可读
  子页与层级异常等全部容忍计数。索引根页不可读或 last-touch 缺失则该 dbnum 预览
  失败，不伪造空窗口。

### 4.3 `POST /api/v1/update/execute` — 扫描 + 入队（ADR-011 合流后）
- 映射：`AiosDBManager::enqueue_manual_update(project, mdb, dbnums)`；执行由进程内唯一的数据批次
  派发器从队列取走并派给 worker（`batch_worker`，至多 `data_batch_workers` 个在飞、默认 1
  ——ADR-011 2026-08-09 修订；与 `async_watch` 自动发现共用同一条队列与冻结语义）。
- 请求：`{ "project": "HD" }`；可选 `"dbnums": [7997, 8000]`（ADR-020：**范围内的
  子集选择**，S2-G 勾选折算出的库号名单）。缺省（不带字段）= 全范围，行为不变。
- 响应 202（入队回执，rollout 第八节第 7 条）：

```json
{
  "project": "HD",
  "scanned": 3,
  "enqueued": [{ "task_id": "db-20260727-210301-7f3a", "dbnum": 7997, "db_type": "DESI",
                  "phase": "design", "epoch_id": 7,
                  "intent": "apply_window", "position": 1,
                  "start_sesno": 85, "end_sesno": 92 }],
  "merged": [],
  "already_covered": [],
  "blocked": [{ "dbnum": 8004, "reason": "库类型变更（登记 DESI → 现场 SYST），已阻断" }],
  "shadowed": [],
  "up_to_date": 2,
  "unselected": [],
  "warnings": ["dbnum=8003: 检测到文件回退（file_latest_sesno=812 < applied_sesno=1005），已按整库重建入队：worker 执行时将清空该库数据并按首次导入重新解析当前文件"]
}
```

- 语义：手动触发**不插队**（ADR-011 §6）——对已在队里的库只是并入会话（`merged`），
  它剩下的唯一新意义是「别等下一个 30s 轮询」。阻断与排除的库压根不入队（`blocked`）。
  `sync_live=true` 时同样可用（422 已退役）。
- 回退默认整库重建（ADR-021，2026-08-13，取代 2026-08-12 的 watermark_realign 档位）：
  判定为**回退**的库不进 `blocked`——按首次导入形状（窗口 `1..file_latest`）入队并经
  `warnings` 报告（样例见上）。入队本身**不删任何数据**；worker 出队后在冻结点复核，
  仍判回退才整库清空（`wipe_dbnum_for_reinit`）并按首次导入重新解析，清库失败该批次
  终态 `failed`（水位未动，下一轮幂等重放）。`type_changed` / `duplicate` / `missing` /
  归属不符照旧 `blocked`，形状与措辞不变。
- `enqueued` / `merged` / `already_covered` 的每一行都带只读 `intent`：普通窗口为
  `"apply_window"`，已裁决回退重建为 `"reinitialize"`。它只描述调度意图，不是新状态；
  旧客户端忽略新增字段即可。`reinitialize` 在合并时占优，运行中同库则排一条后继行。
- `dbnums` 子集语义（ADR-020）：每个请求的 dbnum 先过 `UpdateScope::admits`，不在当前
  MDB 声明名单里的**直接拒**（回执 `warnings`，不给绕过 ADR-0013 统一范围门的第二条路）；
  未勾选的常规库不扫描、不入队、水位不动（回执 `unselected`），预览之后新产生的会话不会
  被偷偷并入；SYS meta（SYST/DICT/GLB/GLOB）不是勾选对象，永远随批（S2-H「会一并处理」段）。
  202 回执、排队中合并、运行中冻结全部照旧——过滤只决定谁入队。
- 每个数据批次是一条 `kind = "data_batch"` 的任务：`queued -> running -> succeeded|partial|failed`；
  进度经 WebSocket 推送（第 5 节），终态 result 为 `{ project, status, batch: DataBatchResult,
  units: [ModelUnitResult], warnings }`（一行任务一个批次，「一次运行」的复数形态随 ADR-011 退役）。
- 模型持久工作由正式消费者 `kind = "model_drain"` 分页认领（每页至多 16 根）。状态为
  `running -> succeeded|partial|failed|yielded`；`yielded` 表示初始化门或数据优先级要求让位，
  是本次进程内任务的终态，但未执行的 durable pending 行、revision 与 attempts 均保持原值。
  `detail` 带 `epoch_id` 和 `roots[] { dbnum, source_end_sesno, target_refno, action, revision }`，
  `result` 带完成、失败、未执行数量及让位原因。重启恢复只认 `model_update_pending`，不恢复
  瞬态 task 行。

### 4.4 `GET /api/v1/tasks` 与 `GET /api/v1/tasks/{id}`
- 列表支持 `?state=running&kind=data_batch&limit=50`（进程重启即清空——durable 状态以
  `model_update_pending` 表与应用水位为准，本表只是 UI 视图）。
- 详情响应：

```json
{
  "task_id": "db-20260729-100301-7f3a", "kind": "data_batch",
  "state": "succeeded", "project": "HD",
  "dbnum": 7997, "start_sesno": 85, "end_sesno": 92,
  "created_at": "2026-07-29T10:03:01+08:00", "finished_at": "2026-07-29T10:04:12+08:00",
  "events_seen": 42,
  "result": { "status": "success", "batch": {}, "units": [], "warnings": [] }
}
```

### 4.5 `POST /api/v1/model/ensure` — 按需生成单构件模型（同步）
- 数据清单或启动全量模型尚未收口时，已有稳定模型仍可只读返回；实际生成请求返回
  `409 initialization_not_ready`，message 含当前 phase、epoch 与 blockers，且不会
  创建新的模型 pending。
- 映射：`AiosDBManager::ensure_model_generated(refno, force)`；短路未命中时先写入
  `(regen_root, generation_root)` durable pending，再同步等待共享生成执行器的结果。
  命中已有 pending 或正在执行的同根任务时等待同一份工作，不另开生成路径。
- 请求：`{ "refno": "24381/100677" }`，可选 `"force": true`。
- 响应 200：`OnDemandModelResult` 原样，含 `generation_root`、`model_instance_count`（画得出来的实例数）与 `generated_instance_count`（生成写出的实例数，含画不出来的）。`status` 三种：
  - `AlreadyAvailable` — 已经有画得出来的实例，没跑生成；
  - `Generated` — 这次跑了生成，拿到了画得出来的实例；
  - `NoRenderableGeometry` — 这次跑了生成，但没有一条画得出来（`model_available` 为 false）。两种形态都归这里：写出了实例却全都画不出来，以及**一条都没写出**（无子件的 BRAN、纯作层级用的 STRU，`generated_instance_count` 为 0）。这是数据的终局不是失败，所以走 200 而不是 5xx：底下的几何不修好，重发只会把同样的生成再跑一遍。
- 等待超过 120s 且 durable pending 仍在排队或执行时响应
  `202 { "code": "generation_pending", "generation_root": "24381/100677" }`，并带
  `Retry-After` 响应头。后台工作不取消；客户端随后以 `force=false` 重查，复用或等待同一
  pending。202 不是生成失败，只有获得成功结果或失败终态后才返回 200 或对应 5xx。
- `force` 只给「人明确要求重生成」用（S4-C 的重试）：它跳过实例与成功结果复用并新增
  pending revision，但仍服从共享执行器、根锁和 revision 收口。显示补齐**不要**传，
  否则每显示一次都会提交新的重生成工作；收到 `generation_pending` 后的轮询也不得继续
  携带 `force=true`。
- 按需边界使用一个最小领域错误枚举完成以下分型，不把所有 `anyhow::Error` 压成
  `not_found` 或 `internal`，也不扩建全局错误框架：
  - `422 container` — WORL / SITE / ZONE，按契约恒被拒绝做生成根。**这不是失败**：客户端应展开一层，对子节点逐个 ensure（ADR-0009）。
  - `404 not_found` — 依赖查询成功，并确认库里没有这个 refno；只有该情形允许负缓存。
  - `422 precondition` — 构件在、也不是容器，但向上找不到任何合法生成根。
  - `503 ref0_affiliation_unavailable` — refno 合法，但当前缺少 `Ref0 → dbnum` 库归属；客户端不得按 404 负缓存，可稍后重试。
  - `409 ref0_affiliation_conflict` — 同一 Ref0 同时出现在多个 `dbnum`；pending 保留，
    自动重试不会自行修复，需先纠正项目文件。无冲突根不受影响。
  - `503 model_dependency_unavailable` — SurrealDB 查询、项目文件读取或 locator 暂时失败；
    客户端可重试，不得负缓存。
  - `500 generation_failed` — 确定性的解析、数据损坏或生成失败；已经建立的 pending 保留，
    不写成功结果。
  - `400 bad_request` — refno 连格式都不对。
- 单根生成通常秒级，同步等待即可；HTTP 层等待预算为 120s，耗尽后按上述 202 契约返回，
  不取消后台生成。实测 AMS 8000 的 SUPPO 与风管 BRAN 冷生成要 99–104s，贴着这条线，
  客户端自身超时不能设得比它更短。

### 4.6 `GET /api/v1/update/pending-units` — 待重试模型单元
- 映射：`load_pending_model_units()`，响应 `{ "units": [...] }`，元素为 `PendingModelUnit` 原样。

### 4.6.1 `POST /api/v1/update/pending-units/retry` — 显式复活一个待重试单元

- 请求：`{ "action": "regen_root", "target_refno": "24381/100677" }`。
- 只允许操作已经存在的 `(action, target_refno)` pending；不存在返回 `404 not_found`，
  不根据请求凭空创建任务。
- 在一个原子更新中执行 `revision += 1`、`attempts = 0`、清除 `last_error` 并恢复 pending，
  返回 `202 { "action": "...", "target_refno": "...", "status": "pending" }`。
  正在执行旧 revision 的 worker 随后不能删除或标记这条已复活记录。
- 不提供批量重试；新数据触发仍走正常 enqueue，不调用本端点。

### 4.7 `GET /api/v1/dbnums` — 水位状态 + 阻断/排除（rollout 服务端第 8 项）
- 映射：`AiosDBManager::dbnum_statuses(project)`（登记表 ∪ 项目扫描；只读头部与
  最新会话号，不收集增量窗口）。可选 `?project=`，缺省当前项目。
- 响应：`{ "dbnums": [...], "warnings": [...] }`，每行：

```json
{ "dbnum": 8003, "db_type": "DESI", "file_name": "ams8003_0001", "file_path": "...",
  "file_size": 57948160, "file_latest_sesno": 812, "applied_sesno": 1005,
  "initialized": true,
  "anomaly": { "kind": "rollback", "file_latest_sesno": 812, "applied_sesno": 1005 },
  "blocked": false, "excluded": false }
```

- `anomaly` 五种（spec §文件异常）：`rollback` / `path_migrated` / `type_changed` /
  `duplicate`（带 `paths[]` 交给人挑）/ `missing`。`path_migrated` 不阻断；`rollback`
  自 ADR-021（2026-08-13）起也不再表现为阻断——它转成整库重建（见下），`anomaly`
  证据（判据两端与两端保存时刻）照带，界面据此把它与普通新库区分开。
- `blocked`：阻断的库压根不入队，队列面板没有它们的行——这里是「这个库的水位为
  什么一直不动」的唯一出处。`excluded`：不在本期执行范围，即**当前 MDB 没有声明
  这个 DESI**（2026-08-06 起范围只由 MDB 定，`manual_db_nums` 一类手写名单不再参与），
  与阻断不是一回事，界面上不许合成一行。
- `rollback` 的库在下一轮扫描/执行时按首次导入入队（窗口 `1..file_latest`），由 worker
  冻结点复核后整库清空重建，不再长期停留在 `blocked: true`；其余四种异常照旧阻断。
  原 `watermark_realign` 档位与单库对齐端点（曾为 §4.9）随 ADR-021 移除。
- 旧字段是新形状的子集，既有消费者不受影响。

### 4.8 `GET /api/v1/queue`、`POST /api/v1/queue/pause`、`POST /api/v1/queue/resume`（ADR-011 §9）
- `GET /queue` → `{ "paused": false, "epoch_id": 7, "blocked_by_phase": "catalogue",
  "shadowed": [], "rows": [{ "task_id": "db-…", "dbnum": 7997,
  "db_type": "DESI", "phase": "design", "epoch_id": 7,
  "blocked_by_phase": "catalogue", "intent": "apply_window", "state": "queued",
  "start_sesno": 85, "end_sesno": 92 }, …] }`：
  队列快照，行按队列序（运行中在前），经 `task_id` 与 §4.4 的任务行对得上。
  `intent` 取值同 §4.3；`reinitialize` 明示该行冻结点仍需复核回退后才清库。
- `POST /queue/pause` → `{ "paused": true }`：**只挡出队与空闲轮**，正在跑的那条会
  跑完为止（服务端没有中止接口，界面文案只能说「不再出队」）。标志持久化在
  `queue_control:main`（与水位同库，不进队列表），**活过重启**：重启后 worker 起跑
  前恢复它，队列重建完成也不开吃，直到 `resume`。
- `POST /queue/resume` → `{ "paused": false }`：恢复出队并立即唤醒 worker。
- 没有单条取消：队列是派生态，从队里移掉一行不会推水位，下一轮轮询照样把它发现
  回来——那是个会自己撤销的按钮（ADR-011 §9）。

### 4.9 `POST /api/v1/dbnums/{dbnum}/realign` — 已移除（2026-08-13，ADR-021）
- 2026-08-12 引入、次日随 ADR-021 退役：回退的处置改为默认自动化——扫描/手动入队
  把回退库排成整库重建批次，worker 冻结点复核后 `wipe_dbnum_for_reinit` 清库并按
  首次导入重新解析，端点没有剩余职责（服役期间无对外消费者，plant-ui 未接入）。
- 调用该路径现在返回 404。手工兜底保留同家族的 `DELETE /dbnums/{dbnum}/data`
  （整库快删）与 `DELETE /dbnums/{dbnum}/data/above/{watermark}`（残留清理），
  操作契约（暂停队列 → 等该库空闲 → 精确 confirm）不变。

## 5. WebSocket 协议（`GET /api/v1/ws`）

### 5.1 信封
服务端到客户端统一信封（`type` 判别）：

```json
{ "type": "task_progress", "seq": 17, "ts": "2026-07-29T10:03:05+08:00", "task_id": "db-...", "payload": {} }
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
  events_seen, detail?, result? }`——`created_at` 是**入队时刻**，`started_at` 是开跑时刻，
  「已排」与「已用」是两个起点。
- 状态机：数据任务为 `queued -> running -> succeeded | partial | failed`；模型消费页为
  `running -> succeeded | partial | failed | yielded`。kind 四种：
  `data_batch`（一行 = 一个数据批次；同一 dbnum 至多两行——运行中一行 + 排队中一行）、
  `room_recalc`（队列跑空时收的一轮房间收敛，创建即 running；`detail` 携带
  `{ panels, elements, dead_letters }` 分项计数，done/total 用 `units_done`/`total_units`，
  ADR-011 §10）、`model_drain`（持久模型工作的一页进程内观察）、`manual_update`（已退役，
  不再产生新行）。
- TaskId 格式 `db-{yyyyMMdd-HHmmss}-{4位随机hex}`（数据批次）/ `room-…`（房间轮）/
  `model-…`（模型消费页）。
- 保留策略（ADR-011 §11）：内存上限 1000；queued / running 永不剔除；每个 dbnum 保留
  最近一条终态；其余按全局最老终态先剔。重启即清空，队列由 `init_watcher` 重扫水位重建
  （界面须说明「这是重建的队列」）。
- 并发约束汇总：数据批次由**一个派发器**统一出队（默认单 worker 串行；ADR-011
  2026-08-09 修订允许至多 `data_batch_workers` 个在飞批次、上限 8，同 dbnum 恒串行、
  非 DESI 与基线批次独占；互斥是调度器的性质，HTTP 层不再有 409 预检）；所有生成入口
  共享 durable pending、生成执行器和 per-生成根锁，同一根不会由按需与批次路径并发生成。

## 7. 配置与依赖

- `DbOption` 新增（`DbOption.toml`）：

```toml
# Web 服务监听地址；不配置则即使编译了 http_api 也不启动
http_api_addr = "0.0.0.0:8020"     # 评审决议：局域网可访问；8009/8010/1883/8000 已被占用，避开
http_api_cors = ["*"]              # 开发期放开，上线前收敛为前端 origin
```

- 静态前端资源是可选能力。资源目录存在时挂载 `/assets` 与 SPA fallback；目录缺失时只记录
  一次告警，静态路径返回 404，REST/WS 仍正常启动。无需为此增加配置开关，也不得因
  `PLANT_ASSET_ROOT` 缺失或无效终止服务。

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
| M3 | `/model/ensure`、`/update/pending-units` 查询/单项重试、统一 worker 事件桥接 | 按需生成幂等；watch 与手动提交产生相同 tasks 事件 |
| M4 | CORS/超时/日志（tower-http trace 接入现有 tracing）、联调收尾 | 前端跨域可用，异常路径错误码符合第 3 节 |

## 9. 评审决议（2026-07-26）

1. **监听地址与端口**：`0.0.0.0:8020`，供局域网前端访问（已写入 `DbOption.toml`）。
2. **鉴权**：本期不做；前端部署到共享环境前可追加最简 token 头校验。
3. **`incr_applied` 已退役**：自动与手动批次统一发布 tasks 主题事件。
4. **任务历史持久化**：仅内存分层保留最多 1000 条（durable 语义由水位 + pending 表承担），不做持久化。
