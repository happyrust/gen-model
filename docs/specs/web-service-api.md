# Web 服务接口设计（axum REST + WebSocket）

- 状态：已评审通过（2026-07-26），按里程碑 M1→M4 实施；实现见 `src/web_service/`
- 日期：2026-07-26
- 决策：方案选型已定为 **axum 0.8.9 REST + WebSocket**（用户决策，见会话 gen-model-49）
- 相关规格：`docs/specs/manual-model-update.md`（手动更新语义）、`CONTEXT.md`（术语表）

> **实施基线（2026-09-11）**：后端基线 `757b23e94390ae202b4dabfda3c596c4b1e8dea7`
>（`refactor/retire-publish-stage`），前端基线
> `7cbc1cf65ae1c43c605513afbe3295999ea5b58d`（`main`）。后端工作树在本功能开工前已有
> 发布阶段退役、存储默认值和配置向导等未提交改动；本功能在该代码真值上前移植，不 reset、
> 不 cherry-pick 历史实现，也不把既有改动冒充本功能提交。下文凡仍提到
> `model_update_pending`、model drain、发布 run/ACK 的段落均为历史说明，以 §4.7.1 的退役清单
> 和本次新增的 §4.5.1B/§4.5.1C 为准。

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
                          +-------- TaskRegistry   任务内模型生成/落地
                                                 ↘ ModelMemoryStore
```

- 新增模块 `src/web_service/`，feature flag **`http_api`**（2026-08-27 起并入 `default`；`--no-default-features` 时需自行点名。是否真正监听仍由 `http_api_addr` 配置决定）。
- 服务与 `async_watch` 在 `run_cli` 内并行：`tokio::join!(mgr.async_watch(), web_service::serve(state))`，互不阻塞；`http_api` 未启用时行为与现在完全一致。
- **共享状态 `AppState`**：`Arc<AiosDBManager>` + `TaskRegistry` + `tokio::sync::broadcast::Sender<WsEvent>`（容量 1024，慢消费者掉线自补）。
- 手动提交与自动 watcher 只负责把数据范围交给同一调度器；进度与终态统一由批次 worker
  更新 `TaskRegistry` 并广播 tasks 事件（ADR-011 2026-08-09 修订：一个派发器 + 至多
  `data_batch_workers` 个在飞批次，默认 1、上限 8，同 dbnum 恒串行、特殊批次独占），
  不再保留 HTTP 直跑或 `incr_applied` 旁路。
- 数据批次选出的模型根在该 dbnum 任务内同步生成并落地；没有 durable model pending、
  drain 页、发布 run 或 ACK。单节点按需 ensure 复用同一生成执行层。
- memory-routed 整库 ensure 是独立的进程内异步任务：冻结一次源文件与根集合，只写
  `ModelMemoryStore`，不进入数据批次队列、不推进任何水位。

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
| 503 | `dependency_db_initializing` | 依赖库（典型是 CATA）还在入库：队列里有它的行，或数据面尚未收敛。`detail { dbnum, ingest }`，带 `Retry-After: 2` |
| 503 | `dependency_db_unreadable` | 依赖库的文件开不开 / 读不动（占用、损坏、权限）。`detail { dbnum }`，带 `Retry-After: 2` |
| 422 | `dependency_file_missing` | 数据面已收敛、队列里也没有它：这个 dbnum 在项目里就是没有文件。**重试无用**，去 MDB 补这个库或修路径。`detail { dbnum, ingest }` |
| 422 | `dependency_file_mismatch` | 路径上那份文件自报的是别的库号。**重试无用**，换文件或改定位。`detail { dbnum, actual_dbnum, file }` |
| 500 | `generation_failed` | 确定性的解析、数据损坏或模型生成失败；pending 保留 |
| 500 | `internal` | 未分类的服务端缺陷 |

- 上面四条 `dependency_*` 是 2026-09-13 从 500 `internal` 里分出来的：依赖库还在入库、
  文件根本不在、文件是别的库——三者以前长得一模一样，而出路分别是**等**、**改项目**、
  **换文件**。`detail.dbnum` 点名是哪个库，客户端不必从 `message` 里抠数字；
  `detail.ingest` 是判据本身（`queued` / `data_face_initializing` / `settled`），
  让「为什么说它还在入库」可被复核。判据只问进程内的批次队列与初始化阶段，不额外读库。
- `Retry-After: 2` 只给「等一等就有」的那几档（`overlay_backpressure`、
  `dependency_db_initializing`、`dependency_db_unreadable`）。确定性失败不给——给了
  就是承诺一个并不存在的重试时刻。

> 旧的通用 409 `conflict` 与「`sync_live=true` 拒手动」的 422 随 ADR-011 §12 退役：数据批次
> 由派发器统一出队（默认单 worker 串行，ADR-011 2026-08-09 修订），执行请求一律 202 入队
> （见 §4.3）。`model/ensure` 的
> 422 `container` / `precondition` 以及归属不变量被破坏时的
> `409 ref0_affiliation_conflict` 不受影响。

## 4. REST 接口定义

### 4.1 `GET /api/v1/health`
`core` 从当前 MDB 路由账本读取，不初始化属性或树 provider，不打开源文件。
RefNo 原生路由改造后删除 `core.skeleton_elements` / `core.stale_reason`，使用
`route_count`、`open_db_count`、`open_dbnums`、`cached_elements`、`stale_dbnums`、
`failed_dbnums` 和 `last_targeted_refresh`。`snapshot_epoch` / `session_vector`
来自按 dbnum 的世代账本；目录尚未注册时 `core.status=initializing`。
`open_db_count` 是 HTTP 属性与树服务仍持有会话的不同 dbnum 数，不是累计文件打开数；
`source_reads` 单独报告进程累计 `database_opens`、`scan_calls`、`scanned_elements`
与 `index_point_lookups`，包含 SYS/DICT 等依赖读取。缺失的已声明文件进入
`failed_dbnums`，有旧世代但当前读取失败的库同时进入 `stale_dbnums`。

存活探针。响应：

```json
{ "status": "ok", "project": "HD", "version": "0.1.22",
  "build_id": "0.1.22+patch.1",
  "started_at": "2026-07-27T21:02:11+08:00",
  "capabilities": {
    "dbnum_model_ensure": true,
    "dbnum_model_ready_roots": true,
    "model_records_batch_max_roots": 64,
    "mesh_formats": ["mesh", "glb"],
    "room_complete_publication_ack": true,
    "room_rebuild_executor": "memory"
  },
  "dabacon_tags": {
    "status": "ready", "scope": "startup", "refreshing": false,
    "sample_duration_ms": 360, "sample_age_ms": 8123,
    "discovered_databases": 3, "readable_databases": 3,
    "issue_count": 0, "issues": [], "issues_truncated": 0,
    "latest": {
      "tag_database_counts": {"2_35_23o": 1, "2_28_19_": 2},
      "untagged_databases": 0
    },
    "history": {
      "tag_session_counts": {"2_35_23o": 2, "2_28_19_": 5},
      "tag_database_counts": {"2_35_23o": 1, "2_28_19_": 2},
      "tagged_sessions": 7,
      "untagged_databases": 0, "untagged_sessions": 0
    },
    "release_selection": "diagnostic_only"
  },
  "queue_paused": false, "static_assets": false,
  "features": {
    "room": {"status":"initializing","generation":"...","executor":"memory"}
  },
  "blocking_conditions": [],
  "initialization": {
    "status": "running", "epoch_id": 7, "current_phase": "catalogue",
    "data_ready": false, "model_ready": false, "model_in_flight": false,
    "room_bootstrap": {
      "status":"initializing","dbnums_total":28,"dbnums_done":4,
      "roots_total":932,"roots_ready":601,"roots_failed":0,
      "current_dbnum":7997,"room_edges_written":1834,
      "max_transaction_bytes":481320,"elapsed_ms":41782,"rss_mib":862
    },
    "phases": {}, "blockers": [], "blocked_by": [], "shadowed": []
  } }
```

- `started_at`：进程启动时刻。队列不持久、重启由重扫重建（ADR-011 §4），界面靠它
  说出「服务 xx:xx 重启过，这条队列是按水位重建的；排队时长从重启起算」。
- `capabilities`：只加不改的能力协商块。两个整库能力同时为 `true` 才表示
  §4.5.1B/§4.5.1C 可用；老服务没有整块时客户端可以试探固定路由。批量 records 上限和 mesh
  格式与实际路由同源公布。`room_complete_publication_ack=true` 表示 ready 根已包含 process-store
  模型行/房间边/数据库回执的完整确认；`room_rebuild_executor` 在读透形态为 `memory`，摄入形态为
  `durable`。
- `dabacon_tags`：启动时对项目目录全部主数据库做的一次只读聚合。`latest` 按数据库
  统计最新 session tag，`history` 汇总全部保留 session；`issues` 最多 20 条，
  `issues_truncated` 说明其余数量。启动先登记 pending，实际文件扫描放进后台，
  health server 不等待它；扫描期间为 `status=pending, refreshing=true`，完成后
  `status=ready|partial|empty|unavailable, refreshing=false`。状态只描述这次
  诊断采样，不进入顶层 `status`、`degraded_sections` 或 `blocking_conditions`；
  `release_selection` 恒为 `diagnostic_only`，不得据此自动选择 `e3d210/e3d31`。
  每个数据库在它被打开时各自冻结 latest-session 指针，因此这不是跨文件事务快照。
- `gen_spatial_tree` 字段已于 2026-08-07 退役：空间/房间计算恒开启，界面不再需要
  「房间增量没开」的降级文案（历史语义见 ADR-011 §8）。
- `queue_paused`：随 §4.8 的暂停接口变化；重启后按持久化标志恢复。
- `initialization`：ADR-025 的当前 epoch、严格数据阶段、数据/模型门、逐阶段计数、
  blocker 与被项目优先级遮蔽的 DICT/CATA。`data_ready=false` 时模型写保持待处理。
- `initialization.room_bootstrap`：`room_membership=true` 的 kv-mem 冷启动屏障，状态为
  `idle|initializing|ready|partial|blocked`。只有 `ready` 才允许 `model_ready=true`；
  进度覆盖 MDB 全部可读 DESI，不受 `watch_dbnums` 限制。
- `features.room.status=available` 必须同时满足配置开启、resident ready、启动屏障 ready、
  process store 可用且对应执行器在线；仅数据库连通不能推出 available。
- `initialization.blockers` / `initialization.blocked_by`：同一批阻断的两副面孔。
  前者是渲染好的 `"<phase>: <message>"` 字符串数组，形状与结构化之前一字不差；
  后者是结构化那份，每条形如
  `{ "phase": "meta", "dbnum": 8191, "project": "JEU", "task_id": "db-…",
  "reason_ref": "db-…", "message": "读取增量数据失败: …" }`。
  `dbnum` 为 `null` 表示这条阻断本来就指不出某一个库（目录清单不可读、整轮重扫
  失败），消费者不得替它挑一个。`reason_ref` 非空时，该 task 的全文原因可经
  `GET /api/v1/batch-failures?dbnum=<dbnum>` 从磁盘取回（重启后仍在）；为 `null`
  表示这条阻断没有对应的落盘失败记录，`message` 就是全部原因。
- `static_assets`：当前是否找到可服务的前端资源目录。`false` 只表示 UI 静态资源不可用，
  不降低 REST/WS 与增量 worker 的健康状态。
- `model_update_pending` 整格已退役，不再出现；根级失败留在所属任务的 `result` 与失败账本，
  库级追平看两枚水位。`blocking_conditions` 不再从模型 pending/drain 拼装。
- `geom_errors`：数据库 `geom_error` 的当前错误摘要。除 `bool_pos` / `bool_neg` 布尔降级外，
  `primitive` 表示基本体缺失/非法 BREP 或 NaN 变换；记录以 `(kind, target)` 唯一，
  `noun` 保存元素类型，`last_error` 保存包含尺寸的原始失败，成功生成后按类别自动销账。
- `degraded_sections` 仍只表示 health 子查询失败或超过统一 2 秒预算。
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

`version` 是发布线；`build_id` 定位该发布线内的一次具体构建。生成二进制 patch 时
同时记录 `build_id` 与 EXE SHA-256，真正的准入判据是后者。
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
  units: [ModelUnitResult], warnings, model_verdict?, model_caught_up_at? }`（一行任务一个批次，
  「一次运行」的复数形态随 ADR-011 退役）。双水位字段在任务收尾那一笔之后读取（ADR-072 D4
  之前是「模型与文件轮次发布之后」——文件轮次退役了）；`model_caught_up_at` 只在
  `model_verdict = in_sync` 时出现，取自水位行的 `model_settled_at`。回执里文件轮次 /
  pending / drain / publish 那一族格随 ADR-072 一并消失，逐条见 §4.7.1。
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
  "result": {
    "status": "success", "batch": {}, "units": [], "warnings": [],
    "model_verdict": "in_sync", "model_caught_up_at": "2026-09-05T17:01:43Z"
  }
}
```

- `kind = "dbnum_model_ensure"`：memory-routed 库的整库即时任务，
  `running -> succeeded|partial|failed`。`total_units` 是任务创建前从同一冻结源枚举出的固定根数，
  `units_done` 是其中已有完整根 ACK 的数量；读透且房间开启时，ACK 必须同时匹配冻结源会话与
  resident generation。`detail` 带 `purpose`（`on_demand|startup_room_bootstrap|forced_rebuild`）、
  数量、冻结源摘要和失败数，
  不把完整根数组塞进每次任务 JSON；完整集合由 §4.5.1C 按 `task_id` 查询。
- 这类任务与投影一样只活在进程内。终态根快照短期、有界保留，服务重启后旧 task id 与快照
  同时返回 404。

### 4.5 `POST /api/v1/model/ensure` — 按需生成单构件模型（同步）
- HTTP 层从 e3d-io direct tree 枚举请求子树的生成根，再交给同一模型执行层。
- 每根所属库先经 `model_read_route::route_for(dbnum)`；读源与发布目标分开判：
  ingest 中尚未完成的 durable 库使用 `ProjectionOnly`，read-through 的 spawned/embedded
  kv-mem 使用 `ProcessStore{durable:false}`，会把模型行、可选房间边与 `gen_root` 回执逐根同事务
  写入本进程 store。
- 请求：`{ "refno": "24381/100677" }`，可选 `"force": true`。
- 响应 200：`OnDemandModelResult` 原样，含 `generation_root`、`model_instance_count`（画得出来的实例数）与 `generated_instance_count`（生成写出的实例数，含画不出来的）。`status` 三种：
  - `AlreadyAvailable` — 已经有画得出来的实例，没跑生成；
  - `Generated` — 这次跑了生成，拿到了画得出来的实例；
  - `NoRenderableGeometry` — 这次跑了生成，但没有一条画得出来（`model_available` 为 false）。两种形态都归这里：写出了实例却全都画不出来，以及**一条都没写出**（无子件的 BRAN、纯作层级用的 STRU，`generated_instance_count` 为 0）。这是数据的终局不是失败，所以走 200 而不是 5xx：底下的几何不修好，重发只会把同样的生成再跑一遍。
- HTTP 等待预算为 120 秒；超时返回 `504 timeout`，但后台生成不取消。客户端稍后以
  `force=false` 重查，命中已经落下的投影回执时直接复用。
- `force` 只给「人明确要求重生成」用（S4-C 的重试）：它跳过实例与成功结果复用并新增
  一次重生成，但仍服从共享根锁。显示补齐**不要**传，否则每显示一次都会重算；超时后的
  重查也不得继续携带 `force=true`。
- 按需边界使用一个最小领域错误枚举完成以下分型，不把所有 `anyhow::Error` 压成
  `not_found` 或 `internal`，也不扩建全局错误框架：
  - `422 container` — WORL / SITE / ZONE，按契约恒被拒绝做生成根。**这不是失败**：客户端应展开一层，对子节点逐个 ensure（ADR-0009）。
  - `404 not_found` — 依赖查询成功，并确认库里没有这个 refno；只有该情形允许负缓存。
  - `422 precondition` — 构件在、也不是容器，但向上找不到任何合法生成根。
  - `503 ref0_affiliation_unavailable` — refno 合法，但当前缺少 `Ref0 → dbnum` 库归属；客户端不得按 404 负缓存，可稍后重试。
  - `409 ref0_affiliation_conflict` — 同一 Ref0 同时出现在多个 `dbnum`；需先纠正项目文件。
  - `503 model_dependency_unavailable` — SurrealDB 查询、项目文件读取或 locator 暂时失败；
    客户端可重试，不得负缓存。
  - `500 generation_failed` — 确定性的解析、数据损坏或生成失败；不写成功回执。
  - `400 bad_request` — refno 连格式都不对。
- 单根生成通常秒级，同步等待即可；HTTP 层等待预算为 120s，耗尽后按上述 202 契约返回，
  不取消后台生成。实测 AMS 8000 的 SUPPO 与风管 BRAN 冷生成要 99–104s，贴着这条线，
  客户端自身超时不能设得比它更短。

### 4.5.1 `DELETE /api/v1/model/subtree` — 清理指定 refno 的模型子树

- 查询参数：`refno=24381%2F100677&confirm=24381%2F100677`；`confirm` 必须与
  `refno` 原文完全一致，否则返回 `400 bad_request`。
- 删除范围严格是传入 refno 自身及其全部 PE 后代，不向上归一到 generation root。
  generation root 解析只用于取得与按需/增量生成相同的互斥锁；任一覆盖根正在生成时
  返回 `409 conflict`，不与生成并发互相覆盖。
- 删除模型关系、无剩余引用的实例信息、直管边和空间树条目；不删除 PE、
  内容寻址共享的 `inst_geo` 或磁盘 `.mesh`。被删除来源的 `room_relate` /
  `room_panel_relate` 随模型对账同事务清理（§4.7.2）。
- 重复调用幂等，响应
  `{ "requested_refno": "24381/100677", "scope": "exact_subtree", "status": "deleted" }`。
- 本端点只清理。需要重建时，由调用方随后调用 §4.5，并显式传 `force=true`。

### 4.5.1A `POST /api/v1/dbnums/{dbnum}/model/rebuild` — 指定库模型全量重建

请求体复用 `ProjectReq` 的 `project` / `mdb` / `namespace` 身份字段；端点本身即表示
强制重建，不接收 `force`。入口先按 `model_read_route::route_for(dbnum)` 分派：

- `Database`：保留 durable `model_rebuild` / batch worker 路径，继续受 `watch_scope` 与
  `model_full_rebuild_enabled` 约束；
- `Memory`：走与 §4.5.1B 相同的进程内整库执行器（purpose=`forced_rebuild`），不向没有消费者的
  durable queue 入队；同库已有启动/整库任务时复用 single-flight。

成功返回 `202 Accepted`：

```json
{"task_id":"model-rebuild-7997-...","dbnum":7997,"expected_roots":2720,"state":"queued"}
```

相同 dbnum 已有活动重建时返回同一 `task_id`。任务调用权威生成根同步并把全部根投入
既有模型协调器，不直调整库生成函数；不预删旧模型、不改变水位。重建期间同库
`/model/ensure` 返回 409。任务仅进程内有效，重启后需重新调用本端点。

### 4.5.1B `POST /api/v1/dbnums/{dbnum}/model/ensure` — 整库即时生成

请求体复用 `ProjectReq`。入口先调用 `model_read_route::route_for(dbnum)`：

- `Memory`：允许，返回 `202 Accepted`；
- `Database`：返回 `409 conflict`，本端点不会改走 rebuild，也不会推进水位；
- 不受 `watch_scope` 限制，与单节点按需显示同轴。

任务创建前只冻结一次设计文件的稳定身份、库类型与源会话，并从这份冻结源枚举固定、去重保序的
生成根集合。`expected_roots`、`TaskRegistry.total_units`、后台生成和 §4.5.1C 的任务 roots
查询全部消费这一集合；后台不得重新读取“当前最新”根集合。

```json
{
  "task_id": "dbnum-model-ensure-7997-...",
  "dbnum": 7997,
  "expected_roots": 2720,
  "already_ready": 18,
  "source_sesno": 264,
  "state": "running",
  "model_source": "memory",
  "model_source_reason": "read-through",
  "durable": false
}
```

同库并发 POST 在 `TaskRegistry` 的同一把锁内按 `(kind, dbnum)` get-or-create，只能有一个
创建者启动 worker；其余请求返回同一个 task id、固定分母和当前状态。worker 自身运行在
`scope_memory_only`，但该作用域只禁止写 durable 权威库，不再等价于“不写任何数据库”；
read-through kv-mem 会复用逐根 process-store 事务。所有未 ready 根一次交给
`ModelRefreshPolicy::generate_roots_report_from_frozen_source`，不进入数据批次队列、不调用
rebuild、不推进应用/模型水位。

初始 `units_done` 等于已满足完整根 ACK 判据的根数；生成期间每秒用同一判据刷新。
零根立即成功。终态规则：

- 全根 ready 且没有生成报告失败：`succeeded`；
- 至少一根 ready，但有报告失败或缺失：`partial`；
- 非空集合一根都未 ready：`failed`。

`result` 至少带 `expected`、`already_ready`、`newly_ready`、`ready`、`failed`、`missing`、
有界的 `failure_samples` 与 `missing_samples`。顶层错误和 generation task panic/JoinError
也必须收成一个终态，不能留下永久 running 行。

### 4.5.1C `GET /api/v1/dbnums/{dbnum}/model/roots` — 冻结根与 ready 状态

- `task_id=<id>`：读取该整库任务固定的冻结根集合；任务运行中与终态短期对账都用这一形态。
- 不带 `task_id`：冻结当前文件并返回一次诊断快照，不得拿它与某个既有任务的分母/进度对账。
- `ready=1|true|yes|on`：响应只保留 ready 根，但 `total` 恒为全根数、`ready_total` 恒为全根中
  当前 ready 数，`only_ready` 回显过滤形态。
- ready 的判据：候选几何回执不早于任务冻结源、记录数与回执相符；若发布策略是
  `ProcessStore`，还必须有同源的 `RootPublicationAck`。`room_membership=true` 时 ACK 还须匹配
  任务冻结的 resident generation。几何候选可用于诊断预览，但数据库/房间事务失败时不得冒充 ready。

```json
{
  "source": "direct",
  "task_id": "dbnum-model-ensure-7997-...",
  "dbnum": 7997,
  "source_sesno": 264,
  "total": 2720,
  "ready_total": 128,
  "only_ready": true,
  "roots": [
    {"generation_root":"24381/100677","noun":"BRAN","name":"/B1","ready":true}
  ]
}
```

### 4.5.2 `POST /api/v1/model/records` — 按生成根分页取模型记录（单根现状 + 多根批量草案）

实现 `handlers::model_records`；取数源按**这根所属的库**选（§4.12），不按进程介质。
本节前半是单根契约（此前只散见于 §4.12，这里补齐），后半是 **多根批量**
（plant3d-web 收口计划 2026-09-09 §3 P9-1/P9-2，决策 d-181 按 A 拍板；服务端 2026-09-09 深夜落地，
只改 `handlers::model_records` 与 `ModelRecordsReq`，路由不动）。

**单根（现状）**

- 请求：`{ "generation_root": "24381/100677", "limit": 1000, "cursor": 0 }`，外加 `ProjectReq` 的
  `project` / `mdb` / `namespace` 身份字段。`limit` 夹在 `1..=5000`（缺省 1000），`cursor` 缺省 0。
- 响应 200：

  ```json
  {"source":"model-database","dbnum":7997,"model_source":"database","model_source_reason":null,
   "durable":true,"complete":true,"incomplete_roots":[],
   "generation_root":"24381/100677","snapshot_epoch":3,
   "session_vector":[{"dbnum":7997,"sesno":812}],
   "items":[{"refno":"24381_100678","old_refno":null,"owner":"24381_100677","world_aabb":{"mins":[0,0,0],"maxs":[1,1,1]},
             "world_trans":{"translation":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
             "insts":[{"geo_hash":"…","transform":{"…":"…"},"is_tubi":false,"is_invalid_tubi":false}],
             "has_neg":false,"generic":"ELBO","pts":null,"date":null}],
   "total":22,"truncated":false,"next_cursor":null}
  ```

  `items` 是 `GeomInstQuery` 原样，`owner` 填的是**生成根**（不是直接属主）；`total` 是这根的全部条数，
  `items` 是 `[cursor, cursor+limit)` 那一段，`truncated = next < total`，`next_cursor` 只在还有下一页时给。
  `source` 为 `model-database`（读 rocksdb）或 `model-memory`（读进程内投影）；
  `dbnum` / `model_source` / `model_source_reason` / `durable` 四格见 §4.12。
  `complete` 表示这些记录对应的完整根发布已提交；`model-memory` 可在数据库/房间事务失败后仍返回
  候选几何供诊断，此时 `complete=false` 且 `incomplete_roots` 点名根，绝不能把候选 receipt 当成功。
- 错误：refno 连格式都不对、或解不出所属库（不在 MDB）→ `400 bad_request`；内存形态下这根还没
  `ensure` 过 → `409 conflict`，消息以 `not_generated:` 开头（客户端先去 §4.5；**空数组不表达「没生成过」**）。

**多根批量（已实现，2026-09-09）**

动机：整库入口一根一次 `records` 是瓶颈（plant3d-web 母计划 §8.9：200 根预算 269 s，单次 0.5 s–4 min 长尾），
服务端一次拿一批根能把往返与逐根建连的开销一起省掉；方案 A 只在既有端点上加一个可选字段，
plant-ui 的单根调用零影响。

- 请求体加可选 `"generation_roots": ["24381/100677", "24381/100691", …]`：
  - `1..=64` 条、**不重复**；与 `generation_root` **互斥**——两个都给、都不给、空数组、超 64 条、同一根出现两次
    → `400 bad_request`（`MAX_MODEL_RECORDS_ROOTS`；判定在 `requested_generation_roots`，有单测钉着）。
  - 一批根须属**同一 dbnum**（客户端手里的根本来就出自同一次 `ensure` 解出的 `generation_roots`
    或同一次整库收集），跨库 → `400 bad_request`。于是一个响应仍只有一个 `source`。
  - plant3d-web 的口径（2026-09-09 用户拍板）：前端只吃 **`ensure` 即时生成、`records` 原样回**的数据，
    不区分也不依赖记录是从哪种介质回的；批量契约因此不对存储形态作任何承诺，`source` 等回执格
    只是可观测信息。
- 响应 `items` **平铺**：顺序 = 请求里根的顺序 × 每根现有的稳定顺序；客户端按 `owner`（就是生成根）分组，
  不需要另一层嵌套。
- 分页：`cursor` 是平铺列表的下标，**跨根连续**；`limit` 仍是总条数（`1..=5000`）；`total` 是这一批的总条数，
  `truncated` / `next_cursor` 语义与单根相同。翻页时原样带同一批 `generation_roots`（顺序也不变）。
- 响应回显 `"generation_roots": [...]`（替代单根的 `generation_root`），并多一格
  `"roots": [{"generation_root":"24381/100677","total":22}, …]`——整批逐根总数，每页都带，
  让「这根就是 0 条」显式可见，不用靠 `items` 里缺席去猜。
- 批里任一根还没 `ensure` 过（服务端手里没有它的记录）→ **整批** `409 conflict`，消息仍以 `not_generated:` 开头
  并列出缺的根；客户端对缺的那几根补 `ensure` 后重发。**空数组仍不表达「没生成过」。**
- 单响应体积：64 根 × 每根几百条，靠分页（`limit` ≤ 5000）兜住，不另设上限。
- 不动的：单根请求 / 响应形状一字不改；`/model/ensure` 不变；不新开 `model/records/batch` 端点（B 案已否）。
- 实现说明：所属库逐根解（`dbnum_for_roots`，跨库 400）→ 一次 `route_for(dbnum)` → 内存形态先按 receipt 收齐缺的根
  再决定 409 → 逐根取记录（库形态 `model_record_reader::records_of_root`，内存形态投影 → `GeomInstQuery`）→
  `page_records_across_roots` 平铺切页（单根 = 一根的特例，切页与原来逐字相同）。单测：
  `model_records_request_takes_exactly_one_root_form_and_bounds_the_batch`、
  `model_records_pages_flatten_across_roots_with_a_continuous_cursor`（`cargo test --lib -- web_service` 62 passed）。
  **未 live**：本机在跑的 `:9099` 是 0.1.21 出厂包，没有这段代码；起新构建后
  `curl -X POST :8022/api/v1/model/records -d '{"generation_roots":["24381/145018","24381/145052"],"limit":5}'`
  应回 `generation_roots` + `roots[]` + 平铺 `items`，单根请求回执与之前逐字段相同。

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
  "model_sesno": 1001, "model_sesno_time": "2026-09-06T10:15:00+08:00",
  "model_caught_up_at": null,
  "model_verdict": "lagging", "model_verdict_reason": null,
  "model_caught_up_roots": 956, "model_chasing_roots": 1,
  "model_dead_roots": 0, "model_lazy_roots": 12,
  "initialized": true,
  "write_state": { "status": "finished", "start_sesno": 1001, "end_sesno": 1005,
                   "intent": "apply_window",
                   "started_at": "2026-09-10T17:02:11+08:00",
                   "finished_at": "2026-09-10T17:06:48+08:00" },
  "anomaly": { "kind": "rollback", "file_latest_sesno": 812, "applied_sesno": 1005 },
  "blocked": false, "excluded": false }
```

- `write_state`（2026-09-10，ADR-072 D1）：**这个库有没有一次没写完的任务**。它不回答
  「欠什么」——欠什么仍由两枚水位回答。一库一行（`dbnum_write_state:{dbnum}`），任务第一笔
  写库之前置 `writing{S,T}`，任务收尾那一笔与两枚水位同事务置 `finished`（行留着，不删）。

  | 字段 | 类型 | 取值 / 含义 |
  |---|---|---|
  | `status` | string \| null | `writing`（正在写库，或上一次死在写库中途）\| `finished`（那一次任务的写库结束了）\| `null`（这个库从没被任务写过；等价于整格缺失，见下） |
  | `start_sesno` | int ≥ 0 | 本次任务的起点 **S**。崩溃后回退删的是 `sesno > S` 的痕迹、两枚水位退回它。初始化 / 重初始化恒 `0`；`apply_window` 是任务开工时的 `applied_sesno`；`model_only` 是起点 `model_sesno` |
  | `end_sesno` | int ≥ 0 | 目标 **T**。回退之后按 `S→T` 重排 |
  | `intent` | string | `initialize` \| `apply_window` \| `model_only` \| `reinitialize`。决定回退走哪条清理路：前者与 `reinitialize` 整库清空重排，其余按 sesno 谓词删。`room_only` 随房间流程退役（§4.7.2），不再作为取值出现 |
  | `started_at` | RFC3339 \| null | 置 `writing` 的时刻 |
  | `finished_at` | RFC3339 \| null | 置 `finished` 的时刻；`status = writing` 时恒 `null` |

  **整格缺失 / `status: null` 是同一件事**：这个库从没被任务写过，或上一次回退把标记行删掉了、
  下一次任务还没开工。两种形态客户端都要认，处置一样——那一栏留空，**别退回 `finished` 再据此
  画绿灯**。

  `status` 与两枚水位的分工要分清：`finished` 只说「本任务的写库结束了」，**不说这个库追平了**
  ——欠什么仍由 `applied_sesno` / `model_sesno` 回答。一个模型失败的任务照样以
  `finished` 收尾（`model_sesno` 不动），那不是矛盾。反过来 `writing` 也不表示数据坏了：它只说
  「这一次写库还没结束」，启动水化段或该库下一次任务开工前会把它回退到 `start_sesno` 再从
  `start_sesno → end_sesno` 重跑。

  界面读法：`status = writing` 且这个库**不在** `/queue` 的运行行里 = 上一次死在写库中途。
  回退期间这一行会短暂缺失、两枚水位往回走、`state_revision` 已经前进——那不是异常，是回退在进行。

  字段真值以 `src/data_interface/write_state.rs` 的 `WriteState` 为准（`dbnum` 那一格是行主键，
  不重复出到 JSON 里；库里还有一个 `updated_at` 是写入时刻的运维痕迹，不进契约）。
- `model_verdict` 是双水位的唯一判决：`in_sync` 表示 `model_sesno == applied_sesno`
  且没有根仍在追赶；`lagging` 表示数据与模型暂时不同步；`not_judged` 表示当前形态
  没有可比较的两枚水位。读透形态的 reason 为 `read-through`，这是中性状态。
  **字段名与取值不变，来源换了**（ADR-072 D4）：判决此前由 `dbnum_consistency::verdict_for`
  在任务收尾之后回问库算出来，现在由水位镜像算——任务内不回读库。消费者不受影响。
- `model_sesno_time` 表示对应 E3D 保存时刻；`model_caught_up_at` 表示模型水位真正写下的
  挂钟时刻，只在 `model_verdict = in_sync` 时非空。
- `model_dead_roots` 与 `model_lazy_roots` 不阻断水位，但必须并排展示；后者为 `null`
  表示尚未观察，不等于零。
- `ref0s`：这个库拥有的全部 Ref0（升序）。`dbnum → Ref0` 是一对多（`CONTEXT.md`
  「Ref0 库归属」），这是那张归属表的反向视图，给只拿得到 refno 的客户端（浏览器）
  自己算库号用。骨架没预热过的形态下**整格不写**——不写空数组，空数组读起来是
  「这个库一个 Ref0 都没有」，那是另一句话。

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
- `model_source` / `model_source_reason`：这个库的模型现在该从哪儿读（§4.12）。
- `dependency_rows` / `dependency_sesno`（2026-09-06，plan §3 D6/D7）：**目录库才有**——按 DESI 依赖入库的
  CATA 行数与它们按哪一版目录文件渲染（`cata_dependency_state:{dbnum}`，不是水位、不进 `applied_sesno`）。
  `dependency_rows` 是**累计送入**的依赖行数（`Fresh` 命中已有行也计，版本前移的 `Replace` 重渲不计），是量级
  不是精确行数；要精确数就 `SELECT count() FROM pe WHERE dbnum = … AND dependency = true GROUP ALL`。
  没有依赖行的库**整格不写**——写 0 读起来像「一个依赖都没有」，那是另一句话。
- 旧字段是新形状的子集，既有消费者不受影响。

#### 4.7.1 随发布阶段退役的格（ADR-072 D4/D5，2026-09-10）

发布阶段（文件轮次屏障）与第二条异步账整体退役之后，下面这些格**从回执与 `/dbnums` 里消失**。
它们不是改名、不是置空，是不再出现——客户端读不到就是读不到，按「没有这一格」处理，
别退回默认值再据此画状态。

| 消失的格 | 出处 | 为什么没了 |
|---|---|---|
| `file_round` / `file_round_status`（`collecting` / `pending` / `done`）、`published_version`、`published_input_manifest` | 任务回执、`/dbnums` | 屏障退役：同库单写者同步写，没有「让三条异步路看到同一版本」这回事。想知道「这一次写完没有」看 `write_state.status` |
| `model_pending` / `model_pending_status` / `model_pending_phase`、`abandoned`、`attempts` / `MAX_ATTEMPTS` 那一族 | `/dbnums`、`web/ops.html` | `model_update_pending` 整表退役。任务的单元清单就是真值；「已放弃」改由镜像的失败根集回答 |
| `model_drain_*`（`telemetry` / `page_starved` / `starved_pages`）、`drain_report`、`drain_page_progress` | `/dbnums`、WS `task_started` / `task_finished` 的 `model_drain` 分支 | 没有 drain 了：模型随任务同步生成 |
| `spatial_pending` / `spatial_reconcile_*` / `published_spatial_version` | `/dbnums`、`/health` | 空间树随 `persist_roots_batched` 就地收敛，不再留意图行 |
| `room_published_*`（`room_published_version` / `room_topology_source`） | 房间查询面 | `room_published_inst` / `room_published_topology` / `room_topology_source` 三表退役。当时的替身是 `room_sesno`，而 `room_sesno` 本身随房间流程一并退役（§4.7.2）——整条房间查询面不再有「已发布到哪一版」这个问题 |

**不动的格**：`applied_sesno` / `model_sesno` 两枚水位、`model_verdict` /
`model_caught_up_at`（名字与取值不变，来源换镜像）、`credential_advance`、`anomaly` / `blocked` /
`excluded`、`model_source` / `model_source_reason`。契约上这一轮**只加一格 `write_state`**，
其余全是减法。

> 本节写成时水位是三枚。第三枚 `room_sesno` 与整条房间对外面随后一并退役，见 §4.7.2
> ——那是另一刀，不在 ADR-072 的账上。

##### 逐条对照表（T4 改 `handlers.rs` / `ops.html` 时按这张表判真假）

「定义点 / 消费点」一栏给的是**当前源码里 grep 得到的位置**；改完之后那一栏应当查无此项。

| 端点 · 格 | 定义点 / 消费点（grep 得到） | 数据来源 | 退役依据 | plant-ui / ops 侧处置 |
|---|---|---|---|---|
| `/health` · `model_update_pending`、`model_update_pending_observation` | `handlers.rs:771`（`sampled_model_pending_status`，:528）、`:979`、`:581` | `model_update_pending::model_pending_status()` | ADR-072 D5（整表退）；计划 C.1 第 2 行 | 整格删；「欠什么」看两枚水位 |
| `/health` · `model_drain`、`blocking_conditions` 里的 `model_drain.page_starved` | `handlers.rs:726`、`:956`、`:803`–`:804` | `model_update_pending::model_drain_telemetry_snapshot()` / `model_drain_page_starved()` | 计划 C.1 第 2 行（没有 drain 了） | 整格删；`blocking_conditions` 少一档 |
| `/health` · `spatial_reconcile` | `handlers.rs:755`–`:764`、`:974`；`ops.html:1390`（`m.h.spatial_reconcile`） | `SideEffectCompensator::spatial_reconcile_status()` | ADR-072 D4（空间树随 `persist_roots_batched` 就地收敛）；计划 C.2 第 4 行 | 整格删；树是否落后看下面那两格 |
| `/health` · `spatial_tree.pending` / `published_version` / `published_input_files` / `published_input_sha256` | 已落 —— 提交 `f2bdaa21c` | 发布输入 manifest（`published_file_input`） | ADR-072 D4；计划 C.2 第 3 行 | **已退**；换成同一节新增的 `snapshot_watermarks` / `db_watermarks` 两格（快照头记的各库 `(applied, model)` 数对 vs 库里现值，不等即重建） |
| `/health` · `window_blocks` | 已落 —— 提交 `0025edaa4`（`handlers.rs` 取数与回执键）与 `588e71915`（`ops.html` 的「窗口阻断」卡、`renderStaging` 与三处读取整块删） | `staging::attempts::load_window_blocks()`（`increment_update_attempt` 的 `kind = 'window_block'` 行） | 计划 §5 R4（`staging/attempts.rs` 整文件退，`increment_update_attempt` `REMOVE TABLE`） | **已退**：ops 那三处随 R4 一起删了；写了一半的库看 `/dbnums` 每行的 `write_state`（ops.html KPI「写入中 / 未写完」） |
| 任务回执 · `file_round` / `published_version` / 文件轮次一族 | `handlers.rs:3292`–`:3307`（回执测试里的 `file_rounds` / `published_version`） | `side_effect_pending::CompletedFileRound` | ADR-072 D4/D5 | 不画 |
| 任务回执 · `model_verdict` / `model_caught_up_at` | `handlers.rs`（`/dbnums` 与 `/tasks` 两处同名） | 由 `dbnum_consistency::verdict_for` 回问库 → **改读水位镜像** | ADR-072 D4（任务内不回读库） | **名字与取值不变，照画**；只是不再等模型那一段 |
| 任务回执 · `credential_advance` | `window_root_plan::CredentialAdvanceOutcome` | 数据尾事务之后的凭证前移 | 不在本轮（d-427） | 照画 |

判真假的方法：表里每条「定义点」都能 `rg -n "<字段名>" src/web_service/handlers.rs web/ops.html`
命中；T4 / R4 改完之后同一条命令应当查无此项（`window_blocks` 那一行随 R4 在 `588e71915` 收掉）。

### 4.7.2 房间流程退役后保留的对外契约（2026-09-11 修订）

退役的是独立房间流水线，不是房间结果：`room_sesno` 一族、`room_recalc` 任务/队列、
`model_update_pending.room_phase` 与 `model_source_reason=room_lagging` 不再出现。房间归属改为
每个模型根产出的一部分，写者是 `persist_roots_batched` 的逐根事务；`room_relate`、
`room_panel_relate` 及数据库函数继续保留。

`POST /api/v1/query` 的 `tool="e3d.room.lookup"` 继续存在。它直接按
`room_relate.out = type::thing('pe', member)` 读关系边，因此 read-through kv-mem 不需要摄入
`pe` / `ATT_*` / `pe_owner` 属性行。响应保留：

- `memberships[]`、`room_membership`；
- `published_version` / `stale` / `publication_status`：版本三元组语义不变，read-through 为
  `null / null / "unsupported"`，关闭房间开关为 `null / null / "disabled"`；
- `calculation_status`：`ready_nonempty|ready_empty|not_computed|initializing|failed|disabled`。

空 `memberships` 只有在 `calculation_status=ready_empty` 时才表示“已经计算且不属于任何房间”；
`not_computed`、`initializing` 与 `failed` 不能被客户端画成空归属。ready 判据使用当前根的
`RootPublicationAck`，房间开启时还要求 ACK 匹配当前 resident generation。

plant3d-web 的 `/api/room/*` 与 `/api/room-tree/*` 属于另一套旧后端，本服务仍不实现。

### 4.8 `GET /api/v1/queue`、`POST /api/v1/queue/pause`、`POST /api/v1/queue/resume`（ADR-011 §9）
- `GET /queue` → `{ "paused": false, "epoch_id": 7, "blocked_by_phase": "catalogue",
  "blockers": [], "shadowed": [], "rows": [{ "task_id": "db-…", "dbnum": 7997,
  "db_type": "DESI", "phase": "design", "epoch_id": 7,
  "blocked_by_phase": "catalogue", "intent": "apply_window", "state": "queued",
  "start_sesno": 85, "end_sesno": 92 }, …] }`：
  队列快照，行按队列序（运行中在前），经 `task_id` 与 §4.4 的任务行对得上。
  `intent` 取值同 §4.3；`reinitialize` 明示该行冻结点仍需复核回退后才清库。
- 行上的 `blocked_by_phase` 只说得出「这行在等哪一相」。挡住那一相的是谁，在顶层
  `blockers` 里——与 §4.1 `initialization.blocked_by` 同一份结构化清单，同相取第一条
  带 `dbnum` 的那条即肇事库。带在这里是为了让「读队列」和「找肇事者」不必分两趟
  请求：不这样的话消费者得再取一趟 `/health` 才拼得出因果。
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
  （整库快删），操作契约（暂停队列 → 等该库空闲 → 精确 confirm）不变。
- **`DELETE /dbnums/{dbnum}/data/above/{watermark}`（按水位删残留）随「恢复即整库
  重初始化」退役**（2026-09-10，决策 d-747）：削库一律改走
  `POST /dbnums/{dbnum}/data/reinitialize`——它排一条整库重建批次，清库在 worker
  冻结点做，与「按水位删掉一段」不是一回事，别当同义替换。
  **同一路径的 `GET` 半边保留**：它只 SELECT，是清库之后自证「真的干净了」的唯一
  现成只读工具。同一句话的规格出处见 `specs/002-watermark-data-backing/spec.md`
  的 FR-012 / FR-012a。

### 4.10 `GET /api/v1/tree/{roots,children,ancestors}`、`GET /api/v1/search` — 直读模型树
- 数据源是 e3d-io 直读（`DirectTreeService` + 进程级骨架），**不查 `pe` / `pe_owner`**：
  读透形态（`data_face = read-through`）下 SurrealDB 里根本没有元素行，树只能从文件解。
  四条端点都只读，可选 `?project=&mdb=&namespace=`——给了就必须与本进程身份一致，
  否则 422 `identity_mismatch`（§3）。`?refno=` 收 `a/b` 与 `a_b` 两种写法。
- `GET /tree/roots` → `{ "source": "direct", "project": "…", "mdb": "/ALL", "nodes": [...] }`：
  每个 DESI/ISOD 库 WORL 下的 SITE 平铺。**没有单一 WORL 根**——一个 MDB 挂多个库、
  每库一棵，客户端要单根就自己合成一个虚拟根。
- `GET /tree/children?refno=a/b` → `{ "source", "parent": "a/b", "nodes": [...] }`：直接成员，
  按元素记录里成员表的存储顺序，服务端不截断（要分页的自己截）。
- `GET /tree/ancestors?refno=a/b` → `{ "source", "refnos": ["a_b", …] }`：自己在前，
  沿 OWNER 向上到库顶。
- `children` / `ancestors` 的 `refno` 不在本 MDB 时回 **404 `not_found`**：既包括库里索引不到
  这个 refno，也包括其 Ref0 在定位器里反查不到任何 dbnum（`1/1` 这类）——后者是确定性的
  「不在这个 MDB」，与前者同码，客户端可负缓存（2026-09-08 前这一种误回 500 `internal`）。
  定位器 / 会话文件的暂时故障仍是 500。
- 节点 = `EleTreeNode` 原样序列化 + HTTP 层外包的一格 `dbnum`：

```json
{ "refno": "24381_2", "noun": "SITE", "name": "/1WCC-PIPE", "owner": "24381_1",
  "order": 0, "children_count": 12, "op": "…", "mod_cnt": null,
  "children_updated": null, "status_code": null, "dbnum": 7997 }
```

- `dbnum` 不加在 `EleTreeNode` 类型上（plant-ui 用 serde 解那个类型，多余字段忽略即
  兼容），而是 handler 在 JSON 层外包一格。它必须由服务端给：refno 的前半段是 Ref0
  不是库号（`CONTEXT.md`「Ref0 库归属」），而浏览器端的几何缓存、可见性、选中全按
  库号分桶。Ref0 不在本 MDB 骨架里时为 `null`——那是「答不出」，不是 0 号库。
- `GET /search?query=&limit=&cursor=` → `{ "source": "snapshot", "epoch", "session_vector",
  "items": [{ "name", "refno": "a/b", "dbnum", "noun" }], "total", "truncated", "next_cursor" }`：
  NAME 子串、大小写不敏感、全库扫描；`limit` 默认 100、夹在 `1..=500`；`query` 为空 400。
- `noun` 只给**这一页**解：名称索引里没有类型，要它就得回元素记录读一次 `TYPE`——
  搜「PIPE」在大库上命中几万条，对全集解会把搜索拖成秒级。读不出来给空串而不是
  `null`：plant-ui 那边 `noun: String` 带 `serde(default)`，缺省可以，`null` 会让整页
  反序列化失败。

### 4.11 `GET /api/v1/meshes/{geo_hash}.mesh | {geo_hash}.glb` — 网格的两种线上形态
- 同一个路由、同一道门、同一次读盘，按后缀选输出形态（`web_service::handlers::mesh_glb`）：
  - **`.mesh`（默认口径，plant3d-web 2026-09-09 拍板）**：原样吐 rkyv 归档字节，零转换，
    `Content-Type: application/octet-stream`。前端 `parseMeshGeometry.ts` 手解——布局是
    **冻结契约**：语料按内容寻址落盘，rkyv 一升级连本服务都读不了旧文件；两头各有一道钉
    （本仓 `mesh_glb.rs` 单测、plant3d-web 金样单测，2026-09-09 全量 48,590 份语料扫描通过）。
  - **`.glb`（过渡口径，留一个发布周期）**：现场转 glTF 二进制，
    `Content-Type: model/gltf-binary`。输出**一个三角面 primitive**：`POSITION` + 可选
    `NORMAL`（f32 VEC3）+ `indices`（u32）。法向量数量与顶点数不等时整段省掉 `NORMAL`
    ——错位的法向量比没有法向量更坏。`wire_vertices` 不进 GLB。
- 与 `/assets/meshes/{geo_hash}.mesh`（静态 `ServeDir`，plant-ui 口径）是同一份文件：
  API 版多了 geo_hash 字符集门、404/400 分型与 `immutable` 缓存头，且走 `/api/v1` 前缀，
  plant3d-web 的反向代理 / baseUrl 拼接不必另开一条静态路径。
- 坐标是 E3D 原生 mm、Z-up，不做单位换算（两种形态一致）。
- `geo_hash` 只放行 `[0-9A-Za-z_]{1,128}`。这既是内容寻址键的真实字符集，也是唯一一道
  目录穿越的门——`..`、`/`、`\` 连门都进不了。网格目录取 `get_meshes_path()`，与生成侧
  同一份配置。
- 响应头（两种形态相同）：`Cache-Control: public, max-age=31536000, immutable`、
  `ETag: "{geo_hash}"`。内容寻址键对应的几何永不改变，所以缓存可以给到 `immutable`；
  `.glb` 的转换成本只付第一次，`.mesh` 连这一次都没有。
- 错误：路径不是 `{hash}.mesh` / `{hash}.glb` 或字符集不合 400 `bad_request`；文件不在
  404 `not_found`（与「refno 不存在」同码，客户端可负缓存）；`.glb` 下文件在但编不成
  三角网格（没有顶点、索引数不是 3 的倍数、索引越界）422 `precondition`——那是数据的
  终局，重发同一个键还是同一个结果，不该让客户端当 500 重试。`.mesh` 不解析不转换，
  文件在就按字节交付（空网格这类终局由前端解析器判 null，与 `.glb` 的 422 同一语义）。
- `1` / `2` / `3` 是仓内三份单位几何（单位盒 / 单位圆柱 / 单位球）。客户端本地造这三份
  几何时，参数须与它们对齐，见 `web_service::mesh_glb` 的
  `repository_unit_meshes_convert_end_to_end`（`cargo test -- --nocapture` 打印包围盒）。

### 4.12 模型取数源路由（`model_source`）——未初始化的库由 API 供数，已初始化的以 rocksdb 为准

出处：`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §8（共识 d-29）；
实现 `data_interface::model_read_route`。

- **判据只有一处**（`model_read_route::route`）：`database` ⇔ 数据面是 `ingest` ∧ 该库
  `applied_sesno > 0` ∧ 该库没有数据批次任务在跑（`task_registry`）∧ `model_sesno == applied_sesno`
  （ADR-071 D8 原文是「三水位相等」；第三枚 `room_sesno` 随房间流程退役后只剩两枚，仍是同一次
  读进程内镜像）；其余 `memory`，理由四选一、按此优先序只报最靠前的一条：
  `read-through` / `data_watermark_unestablished`（基线未落）/
  `initialization_publishing`（基线落了、该库的 dbnum 任务在飞；字面值沿用，含义 = 「这个库正在被写」）/
  `model_lagging`（数据到了 T、模型水位没追上：库里的模型行是旧版，进程内投影更全）。
  `room_lagging` 随房间流程退役（§4.7.2），不再作为理由出现。
  模型水位跑到数据**前面**不算落后（那是 M2 违反，另有对账管）。
- **`memory` 库上的 `POST /model/ensure`**：不等初始化队列、不进 durable 队列。ingest 的临时
  memory 路由只写进程内投影；read-through 则把同一候选投影逐根事务发布到 spawned/embedded
  kv-mem（仍不落 rocksdb），连同房间边和 `gen_root` 回执。回执多四格：
  `dbnum`、`model_source`、`model_source_reason`、`durable`（`false` = 这批行活不过进程重启，
  不是“没有写 process store”）；`publication_status` 取 `ready` / `empty`。
- **`memory` 库上的 `POST /model/records`**：从进程内投影回，`source = "model-memory"`；投影里
  没有这根（还没 ensure 过）回 `409 conflict`，消息以 `not_generated:` 开头——客户端先去
  `/model/ensure`，它今天本来就是这个顺序。空数组**不**用来表达「没生成过」。
- **`database` 库上**两条接口与 §4.5 逐字相同：`records` 读 rocksdb（`source = "model-database"`），
  `ensure` 落库后读库。一次 `records` 请求只有一根、一个库，所以一个响应只有一个源；
  回执同样带 `dbnum` / `model_source` / `model_source_reason` / `durable`。
- **`GET /dbnums`** 每行加 `model_source` / `model_source_reason`（§4.7），喂给判据的是行上自己的
  `applied_sesno` / `model_sesno`；没有模型判决列的行（非 DESI 库、这一次
  普查失败）**不判落后**——「不知道」不写成 `model_lagging`。读透形态的冷行恒 `memory` / `read-through`。
- **翻面事件**：该库的 dbnum 任务跑完（收尾那一笔把两枚水位盖齐）→ `database`；回退重建清库 →
  `memory`。两处都在 WS `tasks` 主题发 `model_source_changed { dbnum, model_source }`（§5.3）。
  客户端只刷那一格、不重载场景——内存投影与刚落的行是同一版文件算出来的，下一次范围重载自然读到 rocksdb。

### 4.13 `GET /api/v1/spatial/{nearby, nearby/refnos, negative-nouns}` — 进程内空间树的邻近查询

出处：`plant3d-web/docs/plans/2026-09-13-spatial-range-query-gen-model-v1-memory-tree-plan.md`（§3.1 契约、§3.2 流水线，
§5 六项拍板均按推荐项 (a)）；实现 `fast_model::spatial_query` + `web_service::handlers::spatial_*`；消费方是 plant3d-web
`model-source/genModelV1/spatialSource.ts`（抽屉「按范围查找」在 `?model_source=gen-model-v1` 下走这三条，legacy 源仍走旧后端
`/api/sqlite-spatial/*`）。

- **候选只来自进程内的 `GLOBAL_AABB_TREE`**（`aios_core::room::room`）：拿**读锁**、锁内只把候选 `collect()` 成一个 `Vec`、
  随即放锁；不取空间串行锁、不写树（`aabb_tree.rs` 的写点白名单测试只认三个文件，本模块不在其中；
  `spatial_query_never_writes_the_tree_or_takes_the_serial_lock` 另钉一道源码扫描）。dbnum / name 都在锁外补——补属性要回
  元素记录读盘，不该压在树的读锁下拖慢写侧收敛。三条都只读、都是 GET，可选 `?project=&mdb=&namespace=`，与 §4.10 同一道
  身份门（不一致 422 `identity_mismatch`）。
- **覆盖面 = 「已生成过模型的构件」**（响应 `coverage: "global-tree"`）：树里只有经 `converge_tree_after_persist` 进过树的盒；
  没 `ensure` 过的构件、隐式管身 `tube`、`ModelMemoryStore`-only（`ProjectionOnly` / memory-only 库）的盒第一版都不是候选
  （计划 §5-5 按 (a)）。客户端在结果区提示「结果仅含已生成过模型的构件」，要更全就先 `/model/ensure` 或整库生成。
- **门禁**：`spatial_state::current_state()` 不在 `Ready` / `ReadyEmpty` → **503 `spatial_not_ready`**，带 `Retry-After: 5`，
  `detail.state` 是状态字面值（`uninitialized` / `loading` / `rebuilding` / `replay_required` / `degraded_reuse` /
  `degraded_blocked`）。`handlers::SpatialApiError` 仍单独成型，但理由只剩退避时长与统一信封那个固定的 2 秒不同
  （`detail` 与 `Retry-After` 自 2026-09-13 起统一信封也带得动，见 §3 的 `dependency_*`）——客户端按 `code` 分型重试，
  不要当 500。`ReadyEmpty` 放行、直接回空结果
  （`spatial_state: "ready_empty"`）；`AIOS_SPATIAL_COMPUTE=0` 的进程恒回 503。

`GET /spatial/nearby` 的 query 参数（全部按字串收、服务端自己解；错误直接是 400 `bad_request`，`message` 直说哪一格错）：

| 参数 | 说明 |
|---|---|
| `refno` \| `x,y,z` | 二选一，同时给 400。`refno` 收 `a/b` 与 `a_b`；`x,y,z` 三个一起给，f32、mm、E3D 世界坐标 |
| `radius` | 必填，mm，`0 < r ≤ 100000`（100 m；再大就是整库扫描，不是「邻近」） |
| `shape` | `sphere`（默认）\| `cube` |
| `nouns` | 逗号分隔，大小写不敏感（服务端按大写比较） |
| `keyword` | 分页前过滤，大小写不敏感；只匹配 refno（`a_b` / `a/b` 两种写法）与 noun，**不匹配 name**（计划 §5-2 按 (a)） |
| `sort` | `distance`（默认：距离升序，同距按 dbnum、refno 兜底）\| `name`（名字只对本页解，全集按 noun、refno 作近似序）；两种都是全序，翻页稳定 |
| `include_self` | 默认 `false`：refno 模式剔除目标自身与祖先链含目标的整个子树；点模式无意义 |
| `include_negative` | 默认 `false`：按 `aios_core::pdms_types::TOTAL_NEG_NOUN_NAMES` 剔除负实体（NBOX / NCYL …） |
| `dbnums` | 逗号分隔 u32，限定库；给了它时答不出库号的条目一律剔除 |
| `page` / `per_page` | `page` 从 1 起；`per_page` 默认 500、夹在 `1..=1000`（超出钳位，不报错） |

- **中心**：点模式 `center.source = "position"`。refno 模式先在 `ModelMemoryStore` 里并「来源是目标或祖先链含目标、`mesh_id`
  非空、盒可用」的记录（与 `/api/v1/query` 的 `model.spatial.bounds` subtree 口径逐字相同），投影里没有再看树上该 refno 自己的
  条目（`AccelerationTree::entries_of`，只读、不触发索引重建），两处都没有 → **404 `not_found`**（消息「…还没生成过模型」；
  客户端提示先显示该构件，接口内**不**隐式 `ensure`——把生成塞进查询会让一次 GET 挂 120 s，计划 §5-3 按 (a)）。
  解出来的是目标含子树的并集盒，`center.source = "refno_aabb_center"`。
- **距离口径**（与前端本地扫描 `aabbMinDistanceToPoint` 同口径，两条纯函数有单测钉住）：点模式是候选盒到点的最小表面距离
  （盒内为 0；sphere 走 rstar `locate_within_distance(p, r²)`，即 `sqrt(distance_2)`；cube 是与 `center ± r` 的盒相交）；
  refno 模式是候选盒到目标并集盒的最小表面距离（逐轴 `max(0, a.min − b.max, b.min − a.max)` 的欧氏范数；sphere 再按 `≤ r`
  过滤，cube 只要与目标盒外扩 r 相交）。`within_radius = distance ≤ radius`，cube 下角落上的条目可以是 `false`。
- **过滤顺序固定**：`include_self` → 负实体 → `dbnums` → 记 `filter_options.nouns` 计数 → `nouns` → `keyword`。noun 计数落在
  `nouns` / `keyword` 之前：它是给筛选器画可选项用的，选中一个 noun 不能让别的 noun 从清单上消失。
- **上限**：读锁内最多取 `candidate_cap = 200000` 条候选，超出置 `truncated_candidates: true`（`candidate_count` 是过滤前的
  候选数）。`groups`（按 dbnum 计数）与 `total_count` 都是**过滤后的全集**、不受分页影响；答不出库号的条目不进 `groups`。
  不做服务端扫描快照缓存，每页重扫（rstar 查询 ms 级，计划 §5-6 按 (a)）。

响应（200）：

```json
{ "results": [{ "refno": "24383_71586", "dbnum": 24383, "noun": "PIPE", "name": "/P-1001",
                "aabb": { "min": [x, y, z], "max": [x, y, z] }, "distance": 123.4, "within_radius": true }],
  "center": { "x": 0, "y": 0, "z": 0, "source": "position | refno_aabb_center" },
  "radius": 5000, "shape": "sphere",
  "total_count": 0, "returned_count": 0, "page": 1, "per_page": 500, "has_more": false,
  "candidate_count": 0, "truncated_candidates": false, "candidate_cap": 200000,
  "groups": [{ "dbnum": 24383, "count": 0 }],
  "filter_options": { "nouns": [{ "value": "PIPE", "count": 0, "is_negative": false }] },
  "spatial_state": "ready", "coverage": "global-tree" }
```

- `refno` 一律 `a_b`；`dbnum` 先取投影记录，缺则 `DirectTreeService::dbnum_of`，两处都答不出为 `null`（与 §4.10 同义：
  「答不出」，不是 0 号库）；`name` **只对本页补**（`DirectTreeService::name_of`，回元素记录读一次 `NAME`，≤ 1000 条），
  没有名字为 `null`；`aabb` 是 mm、Z-up 的世界坐标盒；`filter_options.nouns` 按 count 降序、同 count 按 noun 字典序。
  **没有专业（`spec_value`）维度**——v1 链路里根本没有它，前端在这一源下隐藏专业筛选、改按 `groups` 的 dbnum 分组
  （计划 §5-1 按 (a)；按 noun 派生专业要产品口径，另起切片）。

`GET /spatial/nearby/refnos`：同参、忽略 `page` / `per_page` / `sort`，按距离序给完整命中集、不补 `name`，上限
`result_cap = 100000`（超出置 `truncated_results: true`，`total_count` 仍是全集数；`by_dbnum` 只收答得出库号的条目）：

```json
{ "refnos": ["24383_71586"], "by_dbnum": { "24383": ["24383_71586"] }, "total_count": 1,
  "truncated_results": false, "result_cap": 100000,
  "center": { "x": 0, "y": 0, "z": 0, "source": "position" }, "radius": 5000, "shape": "sphere",
  "spatial_state": "ready", "coverage": "global-tree" }
```

`GET /spatial/negative-nouns` → `{ "nouns": ["NBOX", "NCYL", …] }`：`TOTAL_NEG_NOUN_NAMES` 去重、保持原序；只过身份门，
不看空间树状态。

错误一览：400 `bad_request`（参数）、404 `not_found`（refno 模式中心在投影与树里都没有盒）、422 `identity_mismatch`（§3）、
503 `spatial_not_ready`（上文，唯一带 `Retry-After` 与非空 `detail` 的一档）；其余暂时故障（`DirectTreeService` 取不到等）
按 §3 统一信封 500。
> 三维校审域（`/api/review/*`、`/api/users/*`、`/api/auth/*`、`/files/review_attachments/*`；plant3d-web 写死的契约路径，不在 `/api/v1` 下，响应沿用旧端 `{success,…}` 信封而非 §3 的 `ApiError`）另见 `docs/specs/review-api.md`（自 plant-model-gen `:3100` 移植，`docs/plans/2026-09-12-review-backend-port-plan.md` Q10）。

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
| `task_started` | tasks | `{ task_id, kind, project }`；`kind = "model_drain"`（一页模型消化被认领）时另带 `total_units`（本页根数）与 `detail`（认领时的 `epoch_id / queue_total / roots[] / root_samples_truncated`，`roots[].target_refno` 为 `a/b`；`roots` 最多 10 条样本，与 `GET /tasks/{id}` 里的同一份） |
| `task_progress` | tasks | `ManualUpdateEvent` 原样（`kind: data_batch_started / data_batch_finished / model_unit_started / model_unit_finished`，两阶段、无百分比，与规格一致）|
| `task_finished` | tasks | `{ task_id, state, result: ManualUpdateResult }`；`model_drain` 页收口时另带 `kind: "model_drain"`、`project` 与收口时刻的 `detail`（同上），`result` 为 `{ completed, failed, unstarted, claimed_epoch, current_epoch, yield_reason, failures }`。**客户端据此重载**：`detail.roots[].target_refno` ∩ 自己已加载的生成根 → 重载那几根；`root_samples_truncated > 0` 时样本不全，按 `roots[].dbnum` 整库对齐或退回 REST 轮询。2026-09-08 起发（此前 drain 页在 WS 上一声不响，浏览器端只能 15 s 轮询 `GET /tasks?kind=model_drain`）；发送点在 `task_registry` 的 `insert_running_model_drain` / `finish`，不在 `model_update_pending` 的调用点——drain 的认领与收口只经这两个口。（plant3d-web 自 2026-09-09 起**不再消费**这条事件也不轮询 `GET /tasks`：它只吃自己 `ensure` 出来的数据，见其收口计划 §12；事件保留给其它客户端） |
| `model_source_changed` | tasks | `{ dbnum, model_source }`（`memory` / `database`，§4.12）；`task_id` 为空。初始化发布 run 收口 → `database`，回退重建清库 → `memory`。客户端只刷 `/dbnums` 那一行的来源格，不重载场景；字面值认不出当没收到，下一拍 `/dbnums` 带回真值 |
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
  `running -> succeeded | partial | failed | yielded`。kind 三种：
  `data_batch`（一行 = 一个数据批次；同一 dbnum 至多两行——运行中一行 + 排队中一行）、
  `model_drain`（持久模型工作的一页进程内观察）、`manual_update`（已退役，
  不再产生新行）。`room_recalc`（曾是队列跑空时收的一轮房间收敛）随房间流程退役
  （§4.7.2）：不再产生新行，`GET /tasks?kind=room_recalc` 恒回空列表。
- TaskId 格式 `db-{yyyyMMdd-HHmmss}-{4位随机hex}`（数据批次）/
  `model-…`（模型消费页）。`room-…`（房间轮）随 `room_recalc` 一并退役，不再产生。
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
# Web 服务监听地址；缺失/留空时兼容 server_release_ip，两者都空则回环监听 127.0.0.1:9099
http_api_addr = "0.0.0.0:8020"     # 评审决议：局域网可访问；8009/8010/1883/8000 已被占用，避开
# 对外展示地址；只用于日志、自动打开和分享链接，不参与 socket 绑定
http_public_addr = "192.168.1.9:8020"
http_api_cors = ["*"]              # 开发期放开，上线前收敛为前端 origin
```

- 有效监听地址按 `http_api_addr` → 旧配置键 `server_release_ip` → `127.0.0.1:9099`
  选择。最后一档只绑定回环，不因缺配置意外开放局域网；地址存在但无法绑定时必须明确报错，
  不得把 Web task 当作正常完成。read-through 主流程依赖该 task 常驻，不能因新键缺失而
  无错误退出。
- `/health.http` 返回 `{ bind_addr, public_addr }`。客户端不得把 `0.0.0.0` 当作可导航链接；
  配置向导和 restart 轮询固定使用回环地址，不消费 `http_public_addr`。
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

## 10. 给 plant-ui 的一句话（ADR-072，2026-09-10）

**`DbnumStatus` 多一格可选的 `write_state`，此外全是减法：文件轮次 / pending / drain / publish
那一族格不再出现，消失的格就别画。**

展开成三句：

1. `write_state` 是**可选**的（`DbnumStatus.write_state?: WriteState`）：整格缺失 = 这个库
   从没被任务写过，或上一次回退刚把标记删掉、下一次任务还没开工。缺失时那一栏留空，
   别退回 `finished` 再据此画绿灯。字段清单见 §4.7。
2. 画法：`finished` 是常态（安静，不必出声）；`writing` 且该库在 `/queue` 的运行行里 =
   正在写库；`writing` 而队列里没有它 = 上一次死在写库中途，下一次启动或该库下一次任务
   开工会回退到 `start_sesno` 再从 `start_sesno → end_sesno` 重跑——这一档值得在库那一行
   出声，它是「为什么这个库的水位会往回走」的唯一出处。
3. 回退进行中那几秒，`write_state` 整格会短暂消失、两枚水位往回走、`state_revision` 前进。
   **水位回退不是异常**，别按数据丢失报警；跨仓同步一句即可，不阻本仓落地。

## 11. 给前端的一句话（房间流程退役，2026-09-11 修订）

独立房间水位与任务已退役；房间结果随模型根事务发布。对外保留
`e3d.room.lookup`，并用 `calculation_status` 区分“已算空”与“尚未计算/正在初始化/失败”。

分两头说，因为两个仓的处境不一样：

1. **plant3d-web：不必改。** 你那八条 `/api/room/*` 与 `/api/room-tree/*` 打的本来就是
   另一套旧后端，本服务从没实现过它们——退役前后都是 404，行为一个字不变。
2. **plant-ui：**
   - 任务面板的**房间泳道**（`task_queue.rs` 的 `KIND_ROOM_RECALC`）：`room_recalc` 任务
     不再产生，泳道会恒空。请把它摘掉，而不是留一条永远「距上一轮 ∞」的空泳道。
   - `q room` / `q e3d.room.lookup` 命令继续可用；必须消费 `calculation_status`，只有
     `ready_empty` 才能把空 `memberships` 显示成“不属于任何房间”。
   - 你**直连 SurrealDB** 读的 `room_relate` / `room_panel_relate` / `fn::room_code`
     （房间浏览器、材料表 surql）不随本节退役，写者是模型根的 process-store/durable
     发布事务；kv-mem 启动屏障完成前不得把空结果当最终值。

`DbnumStatus` 上少的那一枚是 `room_sesno`（连同 `room_sesno_time` 等五列）：
水位从三枚回到两枚，`applied_sesno` / `model_sesno` 语义不变。**少的格别退回默认值再画**
——`room_sesno` 缺席不等于 `0`，`0` 读起来是「房间一次都没算过」，那是另一句话。
