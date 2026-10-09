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
- `Retry-After: 2` 只给「等一等就有」的那几档（`overlay_rejected`、
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

### 4.5 `POST /api/v1/model/generate`（旧名 `/api/v1/model/ensure`）— 按需生成节点范围的模型（同步）

> 2026-10-09 ADR-086 D4 改名：请求体、回执、`force` 语义一字不变，只是名字说的事从「确保有」改成「生成」。
> `/api/v1/model/ensure` 作为别名指向同一 handler、每次命中打一行 `deprecated`，保留一个版本后删除。下文凡写
> `/model/ensure` 处均同时适用于 `/model/generate`。
- HTTP 层从 e3d-io direct tree 枚举请求子树的生成根，再交给同一模型执行层。
- 每根所属库先经 `model_read_route::route_for(dbnum)`；读源与发布目标分开判：
  ingest 中尚未完成的 durable 库使用 `ProjectionOnly`，read-through 的 spawned/embedded
  kv-mem 使用 `ProcessStore{durable:false}`，会把模型行与可选房间边逐根同事务写入本进程 store
  （逐根回执不落库，记进进程内的生成根账本，ADR-079）。
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
- **`tube`**（2026-09-22，plant3d-web 方案 B T2；实现 `model_record_reader::model_records_of_root` / `ModelRecord`）：隐式管身的记录
  （`generic = TUBI`、`insts[0].is_tubi = true`，挂在容器 BRAN 自己的 refno 上）在**记录一级**多一格
  `"tube": {"ordinal": 0, "from": "24381_100678", "to": "24381_100679"}`——两端管件的 `a_b` 与同 `(from, to)` 内的序号，
  与房间层级树 `tubes=1` 的直段身份 `(BRAN = refno, from, to, ordinal)` 是**同一个四元组**（§4.13.5），前端拿它把树里的直段行与
  场景里的直管对象对上。构件记录没有这一格（键不出现），别的字段一字不改。投影路来自 `ProjectedTube`；冷读路来自 `tubi_relate` 的
  `leave` / `arrive` / `route_ordinal`——`route_ordinal` 自本日起随行落盘（`model_db_adapter::render_records`），此前的老行没有这一格，
  读侧按同 `(容器, from, to)` 内的出现序（记录 id 序）补 0、1、2…；两端缺失的历史行没有 `tube`。放在记录一级而不是 `insts[]` 里：
  `ModelHashInst` 是 `aios_core` 的类型，这一格只属于本仓的读面，且一条管身记录恰好一个实例。
- 错误：refno 连格式都不对、或解不出所属库（不在 MDB）→ `400 bad_request`。
- **根按祖先理解（ADR-086，2026-10-09）**：`generation_root` 不必是生成根——给 ZONE / SITE 也行，给生成根以下的一个件
  （BRAN 里的一个 VALV）也行。库路由本来就按 `anc CONTAINS` 取子树；内存形态下没有回执的节点分两种：在根之上的展成投影里
  它名下盖过章的生成根再逐根取，在根之下的取它头上那根的记录、按祖先链只留它名下的（每条记录的 `owner` 两种情形下都是
  生成根）。名下一条都没有 → **`200` 空数组**；`409 conflict` 只剩该库正被整库任务占用。`not_generated:` 退役——
  「没生成过」在读接口上不是错误，要生成走 §4.5 `/model/generate`。

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
- 批里的根同样按祖先理解（ADR-086）：没有回执的根展成名下的生成根，一根都没有的那一项 `total = 0`、`items` 里缺席，
  **不再整批 409 `not_generated`**。
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
  "room_alignment": { "generation": "efd4ebe1…", "resident_generation": "efd4ebe1…",
                      "current": true },
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

- `room_alignment`（2026-09-16，ADR-073 D14）：**这个库的房间边整库对齐到哪一代常驻房间模型**。
  它是「`e3d.room.lookup` 为什么对这个库回 `not_computed`」的唯一出处：进程内没有该根的发布 ACK 时
  （ACK 不持久，落盘形态冷重启后一条都没有），那一层就是查询的就绪判据（§4.7.2）。**DESI 行才有**，
  其余整格缺失。

  | 字段 | 类型 | 取值 / 含义 |
  |---|---|---|
  | `generation` | string \| null | 库上那一格（`dbnum_watermark.model_room_generation`）。`null` = 从没整库对齐过：旧行、关着房间开关生成的、或这个库一路只跑过普通增量窗口（它们按 D14 不盖这一格） |
  | `resident_generation` | string \| null | 此刻本进程常驻房间模型这一代。`null` = 房间体制此刻说不出代号（开关关着 / 还在装 / `Blocked`） |
  | `current` | bool | 两者同一代且形态落盘。为真时 `e3d.room.lookup` 对这个库的根答得出 `ready_empty` / `ready_nonempty`，哪怕进程内一条 ACK 都没有；为假时那一层不成立，只能靠本进程刚发布过的根 |

  **它不是第三枚水位**：没有自己的推进规则，只随 `model_sesno` 由能替整库说话的任务同一笔盖
  （初始化 / 整库重排的 ModelOnly / 跑过整库房间重算的任务），`room_membership=false` 下盖水位时清空。
  `current: false` 而两枚水位相等是正常状态，不是告警——该库的下一个 dbnum 任务会按这一格欠着
  跑一次整库房间重算（不在投影里的根回读库上几何，2026-09-17）并把它盖上；不想等就给该库跑一次
  `POST /api/v1/dbnums/{dbnum}/model/rebuild` 或重初始化。

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
`not_computed`、`initializing` 与 `failed` 不能被客户端画成空归属。

ready 判据有两层，先进程后库上（ADR-073 D14，2026-09-16）：

1. 当前根的 `RootPublicationAck`——房间开启时还要求 ACK 匹配当前 resident generation；
2. ACK 够不着时（ACK 不持久，rocksdb 冷重启后一条都没有）看库上的整库对齐承诺
   `dbnum_watermark.model_room_generation`：与当前 resident generation 同一代，且本进程的生成根账本里
   这根没欠着。落盘形态（`AIOS_STORE_MODE=rocksdb`）才有这一层；进程内嵌 `mem://` 活不过重启，仍只认第 1 层。

两层用的是同一个代号，所以房间 / 面板真变过时两层一起作废 → `not_computed`。库上那一格由能替**整库**
说话的任务（初始化 / 整库重排的 ModelOnly / 跑过整库房间重算的任务）随 `model_sesno` 同盖，
`room_membership=false` 下盖水位时清空；普通增量窗口不动它。**它不是第三枚水位**，`DbnumStatus`
的水位仍是两枚。

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

### 4.11.1 `POST /api/v1/element/ptset` — 构件目录 P 点集（E3D `PTSET`）

给 plant3d-web 测量捕捉（P-Point 吸附、点→线垂距的轴线）用。此前浏览器端的 P-Point 只有
legacy 模型包的 `ptsets.parquet` 或旧后端 `:3100` `/api/pdms/ptset/{refno}` 两条来源，
gen-model-v1 读透形态两样都没有（投影只存网格与世界矩阵，`ptset` 从未落库）。这条端点**不经
投影、不连库**：与 `element/attributes` 同一口径直读 dabacon 文件，用 e3d-model 的目录链
`SPRE → SCOM → PTRE → PTSE`（`e3d_model::catalogue_point_set`）解出整个点集——与隐式管身两端
所用的 P 点是同一份求值环境算出来的数字（`web_service::ptset`）。

- 请求：`{ "refno": "24381/145031", "include_members": false }`（`refno` 收 `a/b` / `a_b` /
  带 `=` 前缀；身份三元组可选，同 §3）。`include_members=true` 一并解**直属成员**的点集：
  BRAN / EQUI 这类容器自身没有 `SPRE`，测量悬停在容器上时要显示的是成员的 P 点（E3D 元素级
  捕捉的等价口径），前端用这一发代替逐成员请求。
- 响应（构件字段平铺；`members` 仅 `include_members=true` 时出现）：

  ```json
  {
    "source": "e3d-model",
    "refno": "24381/145031", "dbnum": 7997, "noun": "ELBO", "name": null,
    "catalogue": { "component": "24380/1234", "point_set": "24380/1235" },
    "world_transform": [16 个数], "unit": "mm",
    "points": [{ "number": 1, "noun": "PTAX", "pt": [x, y, z], "dir": [x, y, z], "bore": 100.0 }],
    "unresolved": [{ "number": 3, "reason": "P3（PTCA）方向解不出" }],
    "members": [ …同形，每个直属成员一条… ]
  }
  ```

  `pt` / `dir` 在**构件局部系**（mm，`dir` 已归一化）；`world_transform` 是局部→世界的列主序
  16 元素矩阵（THREE `Matrix4.fromArray` 布局，平移在 12–14，mm），与 `model/records` 的
  `world_trans` 同一折叠（`e3d_model::transform::world_matrix`）。`points` 按 `PTSE` 成员原序；
  `unresolved` 是 `PTSE` 里有、但位置 / 方向表达式求不出的点，`reason` 区分「点不在 PTSET」
  与「解码缺口」。
- 状态：`200` 构件存在（没有 `SPRE` 链或目录件没有 `PTRE` → `catalogue: null`、`points: []`，
  那是「这个构件没有 P 点」，E3D 也不画，不是错）；`400 bad_request` refno 写法不对；
  `404 not_found` refno 在所钉会话里不存在；`500 internal` 读库出错。
- 数据时点同 `/api/mbd/v2/pipe`：目标库按 ADR-054 取**文件最新会话**（每次请求新开 `DbSet`），
  MDB 视图进程内只建一次。

### 4.11.2 `POST /api/v1/element/plines` — 型材 PLINE 线（E3D `PLSTART / PLEND pline`）

给 plant3d-web 测量拾取层的 **Pline 过滤器**用（E3D `EDGPLINE.line(dbRef, pline)`：Snap 吸最近端、
Mid-Point / Fraction / Proportion / Distance 沿线派生、Intersect 当 LINE、Perpendicular to 取这条线）。
gen-model-v1 读透形态只存烘好的网格；PLINE 是目录 `SPRF.PSTR`（PTSS）里的二维 p-line（`PLIN`：`PKEY` /
`PX` / `PY`），从未落库，前端从网格上也认不出「哪条棱是 NA」。这条端点**不经投影、不连库**：与
`element/ptset` 同一口径直读 dabacon，复用 e3d-model 建型材实体的那条链
（`section_catalogue → evaluate_section_profile → section_placement | spine_path`，`web_service::plines`），
把每条 p-line 放进截面标架再乘世界矩阵——与画出来的梁是同一份 `JUSL` 对齐、`LMIRR` 镜像、`BANG` 与
中性轴旋转，所以线一定落在梁的实体上。

- 请求：`{ "refno": "24381/177301" }`（`refno` 收 `a/b` / `a_b` / 带 `=` 前缀；身份三元组可选，同 §3）。
- 响应（世界系，mm）：

  ```json
  {
    "source": "e3d-model",
    "refno": "24381/177301", "dbnum": 7997, "noun": "SCTN", "name": "/COL-1", "unit": "mm",
    "justification_line": "NA",
    "plines": [
      { "key": "NA",  "offset": [0.0, 0.0],   "start": [x, y, z], "end": [x, y, z], "dir": [x, y, z], "length": 3000.0 },
      { "key": "TOS", "offset": [0.0, 150.0], "start": [x, y, z], "end": [x, y, z], "dir": [x, y, z], "length": 3000.0,
        "start_cut": [x, y, z], "end_cut": [x, y, z] }
    ],
    "snap_points": [
      { "refno": "24381/177316", "noun": "SNOD", "kind": "node",  "zdis": 435.56, "position": [x, y, z] },
      { "refno": "24381/177317", "noun": "SJOI", "kind": "joint", "zdis": 435.56, "position": [x, y, z] }
    ],
    "notes": []
  }
  ```

  `start` / `end` 是 E3D `PLSTART pline` / `PLEND pline`：p-line 在 `POSS` / `POSE` 两个截面平面上的点
  （`EDGPLINE.line` 缺省 `cut = false` 取的就是这两个），**不**按 `DRNS` / `DRNE` 斜切；斜切后的端点
  `PLSTCUT / PLENCUT` 另给在 `start_cut` / `end_cut`，只在端面不垂直于轴时出现（E3D Pick Settings「Pline End
  Position = Cut」时前端拿它当端点）。`offset` 是 p-line 在截面平面里、`JUSL` 对齐与 `LMIRR` 镜像之后的位置
  （`JUSL` 那条线恒为 `[0, 0]`，它就是 `POSS → POSE`）。`plines` 按 `PSTR` 成员原序；`notes` 是截面求值 / SPINE
  建路径的非致命记录（与实体生成的 notes 同源）。
- `snap_points`（2026-09-14 起）：E3D Pick Settings「Significant Snap Points」（Fittings / Joints / Nodes 三档）
  的分段点来源——`EDGPLINE.snapLine` 在 Significant Snaps 开着时 `COLLECT ALL (FITT | SJOI SUBJ | SNOD) FOR sctn`，
  把每个成员的位置 `LINE.near` 投到 p-line 上，Snap / Distance / Proportion / Fraction 只作用在光标所在的那一小段。
  这里只给**成员与它的位置**（分不分段、哪几档算是前端 Pick Settings 的事）：走遍 SCTN 名下全部后代，
  `kind` 按 noun 归档（`FITT` → `fitting`，`SJOI` / `SUBJ` → `joint`，`SNOD` → `node`；PJOI / SCOJ 等其余成员不算），
  `zdis` 是沿轴离 `POSS` 的距离（自己的 `ZDIS`；SJOI / SUBJ 的模板没有 ZDIS，取属主链上最近 SNOD 的——E3D 的关节挂在
  节点上），`position` 是世界系 mm：SNOD / SJOI / SUBJ 取对齐线（`JUSL`）上 z = ZDIS 的点，FITT 取它的目录标架原点
  （`section_fitting_placement`，与画出来的配件同一处）。按 `zdis` 升序、同一 ZDIS 节点先于关节。
  **只 SCTN 给**——E3D 的 GENSEC 分支不调 `snapLine`，GENSEC 与 `plines` 为空时一律 `[]`。老构建没有这一格，
  前端按空处理。
- GENSEC 只接**直 SPINE**（一段 `POINSP → POINSP`，两端 + `BANG` 走 `placement_between`）；含弧的 SPINE 在 E3D
  里是 `edgPline.arc()` 不是 `line()`，回空并在 `reason` 说明。GENSEC 的 `DRNS / DRNE` 在 SPINE 上，本期不给 cut 端点。
- 状态：`200` 构件存在——不是 SCTN / GENSEC、没有 `SPRE`、目录链断、SPRF 无 `PSTR`、GENSEC 含弧 →
  `plines: []` 且带 `reason`（E3D 同样没有 p-line 可拾，不是错）；`400 bad_request` refno 写法不对；
  `404 not_found` refno 在所钉会话里不存在（含其 db 不在项目 / MDB 里）；`500 internal` 读库出错。
- 数据时点同 §4.11.1。

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
  kv-mem（仍不落 rocksdb），连同房间边（逐根回执不落库，ADR-079）。回执多四格：
  `dbnum`、`model_source`、`model_source_reason`、`durable`（`false` = 这批行活不过进程重启，
  不是“没有写 process store”）；`publication_status` 取 `ready` / `empty`。
- **`memory` 库上的 `POST /model/records`**：从进程内投影回，`source = "model-memory"`；请求的根按祖先理解，
  投影里它名下盖过章的生成根逐根取；一根都没有回 `200` 空数组（ADR-086，2026-10-09 起；此前是 409
  `not_generated:`，客户端先 ensure 再查的顺序已随 plant-ui ADR-0030 取消）。
- **`database` 库上**两条接口与 §4.5 逐字相同：`records` 读 rocksdb（`source = "model-database"`），
  `ensure` 落库后读库。一次 `records` 请求只有一根、一个库，所以一个响应只有一个源；
  回执同样带 `dbnum` / `model_source` / `model_source_reason` / `durable`。
- **`GET /dbnums`** 每行加 `model_source` / `model_source_reason`（§4.7），喂给判据的是行上自己的
  `applied_sesno` / `model_sesno`；没有模型判决列的行（非 DESI 库、这一次
  普查失败）**不判落后**——「不知道」不写成 `model_lagging`。读透形态的冷行恒 `memory` / `read-through`。
- **翻面事件**：该库的 dbnum 任务跑完（收尾那一笔把两枚水位盖齐）→ `database`；回退重建清库 →
  `memory`。两处都在 WS `tasks` 主题发 `model_source_changed { dbnum, model_source }`（§5.3）。
  客户端只刷那一格、不重载场景——内存投影与刚落的行是同一版文件算出来的，下一次范围重载自然读到 rocksdb。

### 4.13 `GET /api/v1/spatial/{nearby, nearby/refnos, nearby/tree, negative-nouns, nearest-clearance, centerline, surface-clearance, rooms, rooms/{refno}/tree}` — 进程内空间树的邻近查询

出处：`plant3d-web/docs/plans/2026-09-13-spatial-range-query-gen-model-v1-memory-tree-plan.md`（§3.1 契约、§3.2 流水线，
§5 六项拍板均按推荐项 (a)）；实现 `fast_model::spatial_query` + `web_service::handlers::spatial_*`；消费方是 plant3d-web
`model-source/genModelV1/spatialSource.ts`（抽屉「按范围查找」在 `?model_source=gen-model-v1` 下走这三条，legacy 源仍走旧后端
`/api/sqlite-spatial/*`）。2026-09-16 起（本仓 `docs/plans/2026-09-16-bran-centerline-nearest-clearance-v1-dev-plan.md` §3.1 / §3.2）
`nearby` 与 `nearby/refnos` 多一档 `source_mode=bran_centerline`（§4.13.1），并新增第四条 `nearest-clearance`（§4.13.2）；
同日晚再加第五条 `centerline`（§4.13.3：把一条 BRAN 的中心线线段表原样给前端，只读库、不碰树）；第六条 `surface-clearance`
见 plant3d-web `docs/plans/2026-09-17-component-to-wall-surface-clearance-plan.md` §5。**2026-09-20 起**（plant3d-web ADR 0067 /
`docs/plans/2026-09-20-spatial-query-room-and-discipline-filter-plan.md`）`nearby` 与 `nearby/refnos` 多两个过滤维度 **`rooms=`（房间）**
与 **`spec_values=`（所属专业）**，结果多 `spec_value` / `spec_groups` / `filter_options.spec_values` / `by_spec_value` / `room_status`，
并新增第七条 `rooms`（§4.13.4：在册房间清单 + 房间体制状态）。**同日晚**（plant3d-web ADR 0068 /
`docs/plans/2026-09-20-spatial-room-hierarchy-tree-plan.md`）再加第八、九条 **`nearby/tree`**（§4.13.5：同参的一次查询折成
房间 → 专业 → 最小交付单元类型 → 单元 → 构件 的层级树）与 **`rooms/{refno}/tree`**（§4.13.6：一间房的同一棵树，给房间浏览）。
九条都是只读 GET、都挂在 `/api/v1/spatial/` 下（`web_service/mod.rs` 的 `spatial_routes_are_read_only_get_endpoints` 钉住）。

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
| `sort` | `distance`（默认：距离升序，同距按 dbnum、refno 兜底）\| `name`（名字只对本页解，全集按 noun、refno 作近似序）\| `spec_distance`（2026-09-20：先按 `spec_value` 升序、0「其他」在前，同专业再按距离 / dbnum / refno——legacy `/api/sqlite-spatial/nearby` 的同名档，抽屉「按专业」排序用它；只在页内按专业分组给不出跨页的专业序）；三种都是全序，翻页稳定。`nearby/refnos` 的全集不看它、恒按距离 |
| `include_self` | 默认 `false`：refno 模式剔除目标自身与祖先链含目标的整个子树；点模式无意义 |
| `include_negative` | 默认 `false`：按 `aios_core::pdms_types::TOTAL_NEG_NOUN_NAMES` 剔除负实体（NBOX / NCYL …） |
| `dbnums` | 逗号分隔 u32，限定库；给了它时答不出库号的条目一律剔除 |
| `rooms` | 逗号分隔的**房间 refno**（`a/b` / `a_b`，2026-09-20）：只保留房间归属含**任一**所选房间的候选（横跨两间房的构件两边都算；材料表取的「主归属」不在这里用）。不在册的房间 → 400 `bad_request`（消息指去 `/spatial/rooms` 看清单）；房间体制此刻答不出来（`room_membership=false` / 模型未就绪 / 非 hd·hh 构建）→ 422 `precondition`，`detail.reason = rooms_unavailable`、`detail.status` 同 §4.13.4 的 `status`。成员集怎么解见下文「房间归属」 |
| `spec_values` | 逗号分隔的专业值整数（2026-09-20；0 = 其他）：只保留派生专业在其中的候选。专业怎么派生见下文「所属专业」 |
| `page` / `per_page` | `page` 从 1 起；`per_page` 默认 500、夹在 `1..=1000`（超出钳位，不报错） |
| `source_mode` | `aabb`（默认：源 = 目标含子树的并集盒）\| `bran_centerline`（源 = 这条 BRAN 的真实中心线走廊，见 §4.13.1）；大小写不敏感；只能与 `refno` 搭配，与 `x,y,z` 同给 400 |

- **中心**：点模式 `center.source = "position"`。refno 模式先在 `ModelMemoryStore` 里并「来源是目标或祖先链含目标、`mesh_id`
  非空、盒可用」的记录（与 `/api/v1/query` 的 `model.spatial.bounds` subtree 口径逐字相同），投影里没有再看树上该 refno 自己的
  条目（`AccelerationTree::entries_of`，只读、不触发索引重建），两处都没有 → **404 `not_found`**（消息「…还没生成过模型」；
  客户端提示先显示该构件，接口内**不**隐式 `ensure`——把生成塞进查询会让一次 GET 挂 120 s，计划 §5-3 按 (a)）。
  解出来的是目标含子树的并集盒，`center.source = "refno_aabb_center"`。
- **距离口径**（与前端本地扫描 `aabbMinDistanceToPoint` 同口径，两条纯函数有单测钉住）：点模式是候选盒到点的最小表面距离
  （盒内为 0；sphere 走 rstar `locate_within_distance(p, r²)`，即 `sqrt(distance_2)`；cube 是与 `center ± r` 的盒相交）；
  refno 模式是候选盒到目标并集盒的最小表面距离（逐轴 `max(0, a.min − b.max, b.min − a.max)` 的欧氏范数；sphere 再按 `≤ r`
  过滤，cube 只要与目标盒外扩 r 相交）。`within_radius = distance ≤ radius`，cube 下角落上的条目可以是 `false`。
- **过滤顺序固定**（2026-09-20 起多两格）：`include_self` → 负实体 → `dbnums` → **`rooms`** → 记 `filter_options.nouns` /
  **`filter_options.spec_values`** 计数 → `nouns` → **`spec_values`** → `keyword`。两个 facet 计数落在 `nouns` / `spec_values` /
  `keyword` 之前：它们是给筛选器画可选项用的，选中一个不能让别的从清单上消失；落在 `rooms` 之后：房间是与 `dbnums` 同级的范围收窄。
- **所属专业（`spec_value`，ADR 0067）**：候选属主链上 `TYPE = SITE` 那一级的 `NAME` 按 `DbOption.toml` 的 `spec_value_rules`
  分类（首个命中生效、关键字大小写不敏感；缺省即 legacy 后端 `spec_info.rs::site_name_to_spec_value` 那张：PIPE→1 管道、
  ELEC/DIANQI/电气→2 电气、INST/INSTR→3 仪表、HVAC→4 暖通、CIVI/CIVIL/ARCH→5 土建、STRU/STRUCT→6 结构），找不到 SITE / 都不中为 0「其他」。
  实现 `fast_model::spatial_discipline`：沿 OWNER 逐级读记录、逐级记忆（键含 `DirectTreeService` 的会话指纹，换代整体清空），
  每个元素整个进程生命期最多读一次；首个大半径查询付一次账（5 万候选 ≈ 5 万次记录读，与 `name_of` 同代价）。因为这一账要对
  **全部**候选付，锁外的后半程（过滤 / 派生专业 / 排序 / 切页 / 补 `name`）整段在 `spawn_blocking` 里跑、不占 tokio worker；
  共享记忆的锁只罩查表与写回、不罩记录读，并发查询不在 I/O 上互相串行（2026-09-20 复审后改）。
- **房间归属（`rooms=`，`fast_model::spatial_rooms`）**：成员集的取法与 `e3d.room.lookup` 同一条判据分两路——
  读透形态（`data_face = read-through`）库上不落 `room_relate` 边，用常驻房间模型 + 候选的内存投影记录现算
  （`MemoryRoomCalculator::calculate_for_root`，只读投影快照、**不取空间串行锁**）；没有投影记录的候选（树从快照恢复而本进程没再
  ensure 过的根）判不出归属，计入 `room_status.unresolved`、从结果剔除、`warnings` 加一句。摄入 / 落盘形态直接读 `SUL_DB` 的
  `room_relate`（`in ∈ 所选面板`），并按候选所在库核 `dbnum_watermark.model_room_generation` 是否当前代（`room_status.library_alignment_current`），
  不是当前代**照常答**、`warnings` 加一句，不 409。
- **上限**：读锁内最多取 `candidate_cap = 200000` 条候选，超出置 `truncated_candidates: true`（`candidate_count` 是过滤前的
  候选数）。`groups`（按 dbnum 计数）与 `total_count` 都是**过滤后的全集**、不受分页影响；答不出库号的条目不进 `groups`。
  不做服务端扫描快照缓存，每页重扫（rstar 查询 ms 级，计划 §5-6 按 (a)）。

响应（200）：

```json
{ "results": [{ "refno": "24383_71586", "dbnum": 24383, "noun": "PIPE", "spec_value": 1, "name": "/P-1001",
                "aabb": { "min": [x, y, z], "max": [x, y, z] }, "distance": 123.4, "within_radius": true }],
  "center": { "x": 0, "y": 0, "z": 0, "source": "position | refno_aabb_center" },
  "radius": 5000, "shape": "sphere",
  "total_count": 0, "returned_count": 0, "page": 1, "per_page": 500, "has_more": false,
  "candidate_count": 0, "truncated_candidates": false, "candidate_cap": 200000,
  "groups": [{ "dbnum": 24383, "count": 0 }],
  "spec_groups": [{ "spec_value": 1, "count": 0 }],
  "filter_options": { "nouns": [{ "value": "PIPE", "count": 0, "is_negative": false }],
                      "spec_values": [{ "value": 1, "count": 0 }] },
  "room_status": { "rooms": [{ "refno": "17496_8516", "room_num": "A101" }], "source": "memory | durable",
                   "matched": 12, "unresolved": 0, "definition_version": "…", "library_alignment_current": null },
  "warnings": ["房间过滤：3 个候选没有内存投影记录、判不出房间归属，已从结果剔除（先显示它们再查会被纳入）"],
  "spatial_state": "ready", "coverage": "global-tree" }
```

- `refno` 一律 `a_b`；`dbnum` 先取投影记录，缺则 `DirectTreeService::dbnum_of`，两处都答不出为 `null`（与 §4.10 同义：
  「答不出」，不是 0 号库）；`name` **只对本页补**（`DirectTreeService::name_of`，回元素记录读一次 `NAME`，≤ 1000 条），
  没有名字为 `null`；`aabb` 是 mm、Z-up 的世界坐标盒；`filter_options.nouns` 按 count 降序、同 count 按 noun 字典序。
- **`spec_value`（2026-09-20 起，ADR 0067）**：上文「所属专业」的派生值，每条结果都带（0 = 其他，不是「答不出」）。`spec_groups`
  是**过滤后全集**按专业的计数（与 `groups` 同口径、按专业值升序），`filter_options.spec_values` 是**过滤前**（`rooms` 之后）候选按专业的
  计数（按 count 降序、同 count 按值升序）。此前这里写的「没有专业维度、前端隐藏专业筛选」自本日作废：plant3d-web v1 源
  `capabilities.specValues` 翻为 true，专业筛选 / 排序 / 分组原样点亮。
- **`room_status`** 只在给了 `rooms=` 时出现（老客户端一字不变）：`rooms` 是请求点到的在册房间（refno `a_b` + 房间号）；`source`
  `memory`（读透现算）/ `durable`（读边）；`matched` 是所选房间在本次候选里命中的成员数（过滤前口径）；`unresolved` 见上文；
  `definition_version` 是常驻房间模型这一代的代号；`library_alignment_current` 落盘形态下候选所在库是否都对齐到当前代，读透 `null`。

`GET /spatial/nearby/refnos`：同参、忽略 `page` / `per_page` / `sort`，按距离序给完整命中集、不补 `name`，上限
`result_cap = 100000`（超出置 `truncated_results: true`，`total_count` 仍是全集数；`by_dbnum` 只收答得出库号的条目；
`by_spec_value` 与它同形、键是专业值的十进制字串、0「其他」也进桶；`rooms=` 给了同样带 `room_status` / `warnings`）：

```json
{ "refnos": ["24383_71586"], "by_dbnum": { "24383": ["24383_71586"] }, "by_spec_value": { "1": ["24383_71586"] },
  "total_count": 1, "truncated_results": false, "result_cap": 100000,
  "center": { "x": 0, "y": 0, "z": 0, "source": "position" }, "radius": 5000, "shape": "sphere",
  "spatial_state": "ready", "coverage": "global-tree" }
```

`GET /spatial/negative-nouns` → `{ "nouns": ["NBOX", "NCYL", …] }`：`TOTAL_NEG_NOUN_NAMES` 去重、保持原序；只过身份门，
不看空间树状态。

#### 4.13.1 `source_mode=bran_centerline`（`nearby` / `nearby/refnos`）— 沿 BRAN 真实中心线走廊取候选（2026-09-16）

出处：`docs/plans/2026-09-16-bran-centerline-nearest-clearance-v1-dev-plan.md` §3.1；实现 `fast_model::spatial_centerline`
（走廊几何，纯函数）+ `spatial_query::{resolve_center, collect_corridor_candidates}` + `handlers::prepare_centerline`。
消费方：plant3d-web 抽屉「距离查询」的「沿 BRAN 中心线」一档（`GEN_MODEL_V1_SPATIAL_CAPABILITIES.branCenterline = true`，
`genModelV1/spatialSource.ts` 把 `source_mode` 原样转进 v1 参数）。

- **中心线从哪来**：本仓没有 SurrealDB / `tubi_relate`。走廊来自 `mbd::branch_query(set, refno)`——MBD 管道尺寸
  （`/api/mbd/v2/pipe/{refno}`）用的同一份成员序：每个成员的到达 / 离开点（世界 mm）成一段 `start → end`，含按 E3D 规则合成的
  隐式管身 `TUBI`（refno 写作 `a_b~c_d`）；穿过件（`start == end`，只有 `pos`）不成段。handler 在拿树的**读锁之前**
  `spawn_blocking` 预取走廊（读 E3D 库不能压在树锁下），再走 `/nearby` 原有的读锁内取候选 / 锁外过滤分页流水线。
- **候选**：读锁内对每一段用「段盒外扩 `radius`」当探针 `locate_intersecting_bounds`，按 refno 去重；距离是**整条走廊到候选盒的
  精确最小距离**（线段↔AABB 最近点对，从 legacy `sqlite_spatial_api` 移植、有单测），不是到段盒的。`shape=sphere` 再按 `≤ radius`
  过滤；`cube` 保留探针命中的全部（角落上的 `within_radius` 可为 `false`）。`candidate_cap` 仍是 200000。
  候选覆盖面不变（仍只有 `GLOBAL_AABB_TREE` 里的盒）；隐式管身自己没有 refno、也不在树里，不会成为候选。
- **自身**：`include_self=false`（默认）剔除的集合 = §4.13 的投影子树 **∪** `branch_query` 给出的全部成员 refno——BRAN 的隐含管子与
  成员构件都不算「周边」。`include_self=true` 时两者都放行。
- **中心与响应**：`center` 是走廊盒的中心、`center.source = "bran_centerline"`；响应多一块 `source`，`warnings` 只在非空时出现，
  其余字段与默认模式逐字相同（非中心线模式的响应一字不变）：

```json
{ "center": { "x": 0, "y": 0, "z": 0, "source": "bran_centerline" },
  "source": { "kind": "bran_centerline", "refno": "24381_145018", "segment_count": 12,
              "centerline_bbox": { "min": [x, y, z], "max": [x, y, z] }, "outside_diameter_mm": 219.1 },
  "warnings": ["2 个成员没有长度（穿过件），不参与走廊"],
  "results": [ … ], "radius": 1500, "shape": "sphere", … }
```

  `outside_diameter_mm` 是首个给出外径的成员的外径，取不到为 `null`（这里只透传，§4.13.2 的 `surface=1` 扣的就是它的一半）。
  `nearby/refnos` 同参同实现，只是不分页。
- **错误**（在 §4.13 那一览之上追加，均在树的读锁之前判定）：`source_mode=bran_centerline` 配 `x,y,z` → 400；`branch_query`
  找不到这条 refno → **404 `not_found`**；不是派生隐式管身的路由容器（不是 BRAN）或整条 BRAN 一段都拼不出 → **422 `precondition`**
  （消息直说，与 `/api/mbd/v2/pipe` 同一套映射）。

#### 4.13.2 `GET /api/v1/spatial/nearest-clearance` — BRAN 最近净距：每组最近目标 + 可直接画尺寸的两个端点（2026-09-16）

出处：同上计划 §3.2；实现 `fast_model::spatial_clearance`（参数、量距、分桶全在这一个文件，纯函数 + 本地树夹具单测），
handler `spatial_nearest_clearance` 只做身份 / 取服务 / 状态码。消费方：plant3d-web `genModelV1Api.genModelV1SpatialNearestClearance`
→ `useSpatialCompute`「BRAN 中心线最近清距」（按数据源分流：legacy 源仍打旧后端同名路由），结果经 `branExternalDimensions`
以 `bran-clearance` external source 画进尺寸系统。

**与 legacy `/api/sqlite-spatial/nearest-clearance` 的关系**：入参名、分桶语义、`noun_counts` / `excluded_self_members` /
`resolved_filters` 的形状同形，前端 `BranNearestClearanceRequest/Response` 一字不改就能吃。两点不同，都是本仓既有约定：
① 失败走 HTTP 状态码 + §3 信封（legacy 是 200 + `success:false`），成功体里仍带 `success: true`；② 多一个 `surface=1`
（legacy 没有外径这一列）。

query 参数（全部按字串收；布尔格都认 `1/true/yes/on` 与 `0/false/no/off`；错误 400 `bad_request`，`message` 直说哪一格）：

| 参数 | 说明 |
|---|---|
| `source_refno` | **必填**，`a/b` 或 `a_b`。中心线模式必须是 BRAN；`aabb` 模式任意有盒的构件 |
| `source_mode` | `bran_centerline`（**默认**——与 `/nearby` 相反，这条路由就是为中心线做的）\| `aabb`（别名 `default`：按源含子树的并集盒量距） |
| `target_groups` | 逗号分隔的预置组，大小写不敏感：`wall` = WALL / PANE / GWALL / STWALL，`column` = COLU / SCTN / GENSEC（与 legacy 逐字相同）；未知组名 400 |
| `target_nouns` | 直接点名的 NOUN 白名单，合成一桶 `target_nouns`；可与 `target_groups` 并用 |
| `group_by` | `target_groups`（默认；别名 `group` / `groups`）：一组一桶 \| `noun`（别名 `nouns`）：半径内出现的每个 NOUN 自成一桶。**`target_groups` 下两种目标都不给 → 400**（服务端不补 `wall,column`，legacy 也是 400，前端 `queryBranCenterlineNearestClearance` 自己补）；`noun` 下不给目标 = 半径内全部类型，负几何（`TOTAL_NEG_NOUN_NAMES`）默认剔、显式点名放行 |
| `exclude_nouns` | 目标过滤**之后**再剔掉的噪声类型（如 `WELD,ATTA`），两种分桶都生效 |
| `radius` | mm，默认 **5000**，`0 < r ≤ 100000`（同 `/nearby`） |
| `scope` / `dbnums` | `all_loaded`（默认）\| `same_dbnum`（只看与源同库；答不出源库号 → 不限定 + warning）\| `explicit_dbnums`（必须同时给 `dbnums`）。**给了 `dbnums` 一律按显式限定，`scope` 写什么都不管**（与 legacy 同：dbnums 说了算）；限定下答不出库号的候选剔除 |
| `max_per_group` | 每桶最多几条，默认 **1**，钳到 `1..=100` |
| `include_self` | 默认 **false**（净距场景没人要自己）：剔除 投影子树 ∪ BRAN 成员（§4.13.1 同一口径） |
| `surface` | 默认 `false`。`1` = 距离扣掉 `outside_diameter / 2`（不小于 0），`source_point` 沿连线推到管外表面；**只在中心线模式下生效**——BRAN 没有可用外径 → warning、照中心线口径答；`aabb` 模式下给了 → warning、忽略。`resolved_filters.surface` 回的是**实际生效值** |
| `debug` | 默认 `false`；`1` 多回 `debug{}` 计数块 |

- **流水线**：借 `/nearby` 同一条 `scan_global_tree`——树的读锁内只解中心 + 取候选（源、模式、半径生效，`shape` 恒 sphere）；
  NOUN / 库号 / 自身过滤**全部在锁外、本模块里做**，因为每一步都要计数（`excluded_self_members`、`debug`）。锁外顺序固定：
  `include_self` → NOUN 白名单（或不限 NOUN 时的负几何）→ `exclude_nouns` → 库号 → `distance ≤ radius` → 量距 → 分桶。
  走廊预取与 §4.13.1 完全相同（读锁之前、`spawn_blocking`）。
- **量距**：中心线模式 = 走廊到候选盒的精确最近点对——`nearest.source_point` 在中心线上、`target_point` 在候选盒面上，
  `source_segment_refno` / `source_segment_order` 是命中的那一段（隐式管身 `a_b~c_d`）；`aabb` 模式 = 源盒↔候选盒逐轴取朝向对方的面
  （某轴重叠取重叠区间中点，标注落在两盒真正贴近处而不是角上），`source_segment_refno` = 源自己、`order` 为 `null`。
  `intersects = distance_mm ≤ 0`。`annotation{start_point, end_point, label_mm}` 就是同一对端点 + 距离，前端不再算、直接画。
  `distance_method`：`centerline_aabb_clearance_mm` / `centerline_surface_aabb_clearance_mm`（`surface` 实际生效）/ `aabb_clearance_mm`。
- **分桶**（与 legacy 同语义）：`target_groups` 一组一桶、桶序照请求里的组序、**空桶保留**并记一条 warning「组 `x` 在半径内没有目标」
  （「查了但没有」与「压根没查」对前端是两回事）；`noun` 每 NOUN 一桶、桶序按各桶最近距离再按 NOUN 名、没有空桶。桶内距离升序、
  同距按库号再 refno（与 `/nearby` 同一把尺子）；`max_per_group` 截断。`noun_counts` 是**截断前**、过完全部过滤的按 NOUN 计数，
  两种分桶都带，前端直接当类型 facet。

响应（200；`nearest_by_group` 是数组形态，一桶一项）：

```json
{ "success": true,
  "source": { "kind": "bran_centerline", "refno": "24381_145018", "dbnum": 24381, "aabb": null,
              "segment_count": 12, "centerline_bbox": { "min": {"x":0,"y":0,"z":0}, "max": {"x":0,"y":0,"z":0} },
              "outside_diameter_mm": 219.1 },
  "distance_method": "centerline_aabb_clearance_mm", "unit": "mm",
  "query_bbox": { "min": {"x":0,"y":0,"z":0}, "max": {"x":0,"y":0,"z":0} },
  "resolved_filters": { "target_nouns": ["COLU", "GENSEC", "GWALL", "PANE", "SCTN", "STWALL", "WALL"],
                        "target_groups": [{ "name": "wall", "nouns": ["WALL","PANE","GWALL","STWALL"] },
                                          { "name": "column", "nouns": ["COLU","SCTN","GENSEC"] }],
                        "group_by": "target_groups", "exclude_nouns": [], "scope": "all_loaded", "dbnums": null,
                        "radius": 5000, "max_per_group": 1, "include_self": false, "surface": false },
  "nearest_by_group": [
    { "group": "wall", "nouns": ["WALL","PANE","GWALL","STWALL"],
      "candidates": [{ "refno": "24381_8821", "noun": "WALL", "dbnum": 24381, "distance_mm": 1250.0, "intersects": false,
                       "aabb": { "min": {"x":0,"y":0,"z":0}, "max": {"x":0,"y":0,"z":0} },
                       "nearest": { "source_segment_refno": "24381_145123~24381_145124", "source_segment_order": 3,
                                    "source_point": {"x":0,"y":0,"z":0}, "target_point": {"x":0,"y":0,"z":0},
                                    "vector": {"dx":0,"dy":0,"dz":0} },
                       "annotation": { "start_point": {"x":0,"y":0,"z":0}, "end_point": {"x":0,"y":0,"z":0}, "label_mm": 1250.0 } }] },
    { "group": "column", "nouns": ["COLU","SCTN","GENSEC"], "candidates": [] } ],
  "noun_counts": { "WALL": 3, "SCTN": 7 },
  "excluded_self_members": 37,
  "warnings": ["组 `column` 在半径内没有目标"],
  "spatial_state": "ready", "coverage": "global-tree",
  "debug": { "candidate_ids": 0, "rows_examined": 0, "scope_filtered": 0, "noun_filtered": 0, "distance_filtered": 0,
             "self_filtered": 0, "groups_with_hits": 0, "returned_candidates": 0, "truncated_candidates": false } }
```

- 这条路由的盒一律是 `{x,y,z}` 对象（前端尺寸系统直接吃），与 `/nearby` 的 `[x,y,z]` 三元组**不同**。`refno` 一律 `a_b`；
  `dbnum` 答不出为 `null`（本仓没有 `spec_value` 这一列，前端按可选处理）；`source.aabb` 只在 `aabb` 模式下有值，
  `segment_count` / `centerline_bbox` / `outside_diameter_mm` 只在中心线模式下有值，其余为 `null`；`query_bbox` 是源盒 / 走廊盒外扩
  `radius`；`debug` 仅 `debug=1` 时出现。`spatial_state` / `coverage` 与 §4.13 同义。
- **只读**：GET、不取空间串行锁、不写树、不新增表——`spatial_clearance.rs` 里有一道「不碰 `GLOBAL_AABB_TREE`」的源码钉，
  `aabb_tree.rs` 的写点白名单扫描也不认它。
- **未做**（计划 §3.4）：目标侧仍是 AABB，精算到目标网格留给前端；SCTN / GENSEC 不按姿态区分「竖直才算柱」（D6：GENSEC 目前算 `column`）；
  `surface` 缺省是否该为 `1`（D4）未拍板，当前缺省 `false`。

#### 4.13.3 `GET /api/v1/spatial/centerline` — 一条 BRAN 的中心线线段表：成员到达点→离开点 + 隐式管身，原样给前端（2026-09-16）

出处：同上计划 §3.3 ④（管-管间距纠偏：「两个排管的距离测量是指两条 BRAN 的直段之间，有平行的部分就标注它们的间距」）。
实现 `fast_model::spatial_centerline::{CenterlineSegmentJson, CenterlineResponse, centerline_response}`（纯函数 + 单测），
handler `spatial_centerline` 只做身份 / 参数 / 状态码，走廊取法与 §4.13.1 的 `prepare_centerline` **同一个** `query_branch`
（`mbd::branch_query`，`spawn_blocking` 里读盘）。消费方：plant3d-web `genModelV1Api.genModelV1SpatialCenterline` →
`useDtxTools.measure_pipe_to_pipe`（点两根管 → 各自所属 BRAN → 两条中心线 → 在前端把连续共线的段合成直段、配平行对、算中心距，
写进 `useSpatialCompute`「BRAN 中心线最近清距」同一份结果画尺寸）。

**为什么是「原样给线段表」而不是服务端算平行**：前端已经有整套「结果 → 尺寸」的链路与三维里的点选上下文（点在哪一段、
两根管各是谁），几何只是几十条线段的配对；服务端只负责它独有的事——把 E3D 库里的成员序与隐式管身拼成真实中心线。
前端从网格猜轴线的老路（`computeApproxNearestBetweenObjects` 网格采样）在管-管场景下作废：它给的是任意两个网格的最近点对，
不是两条直段之间的间距。

query 参数（`?project=&mdb=&namespace=` 身份门同 §4.10）：

| 参数 | 说明 |
|---|---|
| `refno` | **必填**，`a/b` 或 `a_b`，必须是 BRAN（不是 → 422 `precondition`，与 §4.13.1 同映射）。空 / 格式非法 → 400 |

响应（200）：

```json
{ "refno": "24381_145018", "dbnum": 24381, "segment_count": 21, "outside_diameter_mm": 114.3,
  "centerline_bbox": { "min": {"x":0,"y":0,"z":0}, "max": {"x":0,"y":0,"z":0} },
  "segments": [
    { "refno": "24381_145019", "order": 0, "noun": "ELBO", "implicit": false,
      "start": {"x":0,"y":0,"z":0}, "end": {"x":0,"y":0,"z":0}, "length_mm": 152.4, "outside_diameter_mm": 114.3 },
    { "refno": "24381_145019~24381_145020", "order": 1, "noun": "TUBI", "implicit": true,
      "start": {"x":0,"y":0,"z":0}, "end": {"x":0,"y":0,"z":0}, "length_mm": 2380.0, "outside_diameter_mm": null } ],
  "warnings": ["7 个成员没有长度（穿过件），不参与走廊"] }
```

- `segments` **按成员序**排好，一段 = 一个成员的到达点→离开点（E3D 世界 mm，与 §4.13.2 `annotation` 端点同一坐标系）；
  穿过件（`start == end`）不出现，所以 `segment_count` ≤ 成员数。`implicit = true` 是按 E3D 规则合成的隐式管身
  （refno `a_b~c_d`，不是构件），`noun` 给 `TUBI`；其余段的 `noun` 是成员自身类型（大写；成员没给 → `UNKNOWN`，不猜）。
  **前端挑「直段」就靠这两格**：隐式管身一定是直的；ELBO / BEND 的到达→离开是弦不是轴，不能当直段用。
- `outside_diameter_mm`：段上的是成员自己的外径（隐式管身没有成员，为 `null`，用顶层那格——首个给出外径的成员的外径，
  §4.13.2 `surface=1` 扣的也是它）。`centerline_bbox` / `warnings` 与走廊一致（§4.13.1）。`refno` 一律 `a_b`。
- **只读库、不碰空间树**：这条不需要 `GLOBAL_AABB_TREE`，所以**没有** 503 `spatial_not_ready` 一档，也不带 `spatial_state` / `coverage`；
  库没 ensure 过也答得出（中心线来自 E3D 库的成员序，不依赖生成过模型）。

#### 4.13.4 `GET /api/v1/spatial/rooms` — 在册房间清单 + 房间体制此刻的状态（2026-09-20）

出处：plant3d-web ADR 0067 / `docs/plans/2026-09-20-spatial-query-room-and-discipline-filter-plan.md` §4.1（Q4 / Q5）；
实现 `fast_model::spatial_rooms::list_rooms` + `handlers::spatial_rooms`。给抽屉「按房间过滤」画可搜索多选下拉：房间号不保证全库唯一，
所以线上一律用房间 refno，房间号在客户端对着这份清单解析。只过身份门（`?project=&mdb=&namespace=`），只读常驻房间模型，
不看空间树状态、不碰 `GLOBAL_AABB_TREE`。

```json
{ "status": "ready | degraded | initializing | disabled | unsupported | failed",
  "reason": "room_membership=false：服务端未开启房间归属计算",
  "definition_version": "…",
  "rooms": [{ "refno": "17496_8516", "room_num": "A101", "name": "/A101-RM", "dbnum": 17496, "panel_count": 6 }] }
```

- `status` 由总开关 `room_membership` / `AIOS_SPATIAL_COMPUTE` 与常驻房间模型的生命周期折出：开关关 → `disabled`；`Loading` →
  `initializing`；`Unsupported` → `unsupported`；`Blocked` → `failed`；`Ready` → `ready`；`Degraded` → `degraded`（仍给清单，`reason`
  是 problems 首句——Degraded 的模型照常算房间，ADR-073 D10）。非 `ready` / `degraded` 时 `rooms` 为空、`definition_version` 为 `null`；
  `reason` 没有就不出现。客户端据此整块收起房间过滤并写原因。
- `rooms` 只列**在册**房间（名字含 `room_key_word` 且房间号过项目命名校验的 FRMW / SBFR），按 `room_num`、refno 排；`name` 是房间元素
  的 `NAME`（`DirectTreeService::name_of`，百级），`dbnum` 答不出为 `null`，`panel_count` 是它名下在册面板数。
- 与 `nearby?rooms=` 的关系：那里的 422 `rooms_unavailable` 的 `detail.status` 就是这里的 `status`；那里的 400「不是在册房间」指的就是不在这份清单里。

#### 4.13.5 `GET /api/v1/spatial/nearby/tree` — 同参的一次查询折成房间层级树（2026-09-20）

出处：plant3d-web ADR 0068 / `docs/plans/2026-09-20-spatial-room-hierarchy-tree-plan.md` §4.1 / §4.2；实现
`fast_model::spatial_tree_view::assemble_tree` + `spatial_query::nearby_tree` + `handlers::spatial_nearby_tree`。抽屉选了房间之后结果区不再
平铺分组、改画这棵树；模型树 ROOM 标签页浏览一间房走 §4.13.6 的同一棵。

参数与 `nearby` **完全相同**（`refno | x,y,z`、`radius`、`shape`、`source_mode`、`nouns`、`keyword`、`include_self`、`include_negative`、
`dbnums`、`rooms`、`spec_values`），过滤链也相同（`include_self → 负实体 → dbnums → rooms → nouns → spec_values → keyword`），三处不同：

- **`rooms=` 必填**：树以所选房间为根，没给 → 400 `bad_request`「nearby/tree 需要 rooms=」。不按房间过滤请用 `nearby`——没选房间的命中
  要对全部候选反查归属才有根可挂，那是另一笔账（ADR 0067 Q7 不给房间 facet 的同一理由），先不给。
- **不分页、按 refno 去重**：`sort` / `page` / `per_page` 收下不生效；空间树里同一 refno 的多条条目在这里只算一条（距离取最近）。
- 另两个可选参数只在叶子超上限后用：`unit=<单元 refno>`（`a/b` / `a_b`）/ `other_noun=<NOUN>`——只把这一组的叶子内联进来，树骨架照常给；
  两个同时给 400。

```json
{ "center": { "x": 1.0, "y": 2.0, "z": 3.0, "source": "refno_aabb_center" }, "radius": 3000, "shape": "sphere",
  "total_count": 1055,
  "candidate_count": 3940, "truncated_candidates": false, "candidate_cap": 200000,
  "leaves_inline": true, "leaf_cap": 5000, "leaf_count": 1061,
  "delivery_unit_types": ["BRAN", "HANG", "SUPPO", "EQUI"],
  "rooms": [
    { "refno": "24381_35580", "room_num": "R432", "name": "/1RX-RM04-R432", "count": 1055,
      "specs": [
        { "spec_value": 0, "count": 8, "unit_types": [],
          "others": { "count": 8, "by_noun": [ { "noun": "GWALL", "count": 5, "min_distance": 0.0,
                                                  "elements": [ { "refno": "24381_4090", "noun": "GWALL", "distance": 0.0 } ] } ] } },
        { "spec_value": 3, "count": 1047,
          "unit_types": [
            { "noun": "BRAN", "count": 900, "units": [
                { "refno": "24381_1200", "noun": "BRAN", "name": "/C-IY-1R330-B", "count": 80, "min_distance": 12.3,
                  "elements": [ { "refno": "24381_1240", "noun": "TUBI", "distance": 12.3, "shared_rooms": 2 } ] } ] },
            { "noun": "EQUI", "count": 100, "units": [] } ],
          "others": { "count": 47, "by_noun": [] } } ] } ],
  "room_status": { "rooms": [{ "refno": "24381_35580", "room_num": "R432" }], "source": "memory", "matched": 1298, "unresolved": 0,
                   "definition_version": "…", "library_alignment_current": null },
  "warnings": ["…"],
  "spatial_state": "ready", "coverage": "global-tree" }
```

- **层级**：`rooms[]`（按 `room_num`、refno 排）→ `specs[]`（`spec_value` 升序，0「其他」在前，同 `sort=spec_distance`）→
  `unit_types[]`（按本项目 `delivery_unit_types` 配置表顺序，即 `/health.delivery_unit_types`）→ `units[]`（按最近成员距离、refno）→
  `elements[]`（按距离、refno）；专业下另有 `others`——不属于任何最小交付单元的构件（面板 / 框架 / 楼板 / 型面……）按 noun 分组
  （`by_noun[]` 按计数降序、同计数按 noun），它是「没有单元」的兜底，不是一种专业也不是一种单元类型。
- **单元**：候选属主链上（含自身）首个 noun 在 `delivery_unit_types` 里的元素（`generation_root::is_delivery_unit_noun`），到
  SITE / ZONE / WORL 仍没遇到 → 其他构件。派生沿 OWNER 逐级读记录、逐级记忆（同专业派生：链上每个走过的元素都记下答案，
  换代整体清空，锁只罩查表 / 写回不罩记录读）。`units[].name` 是单元的 `NAME`（爬链时顺手拿到，不另读），没有为 `null`；叶子**不带名字**。
- **计数**：所有 `count` 都是该节点下按 refno 去重的构件数；`total_count` 是全树去重数。**横跨多间所选房间的构件在每间房下都出现**
  （任一归属，ADR 0067 Q2 的同一口径），只在 `total_count` 里算一次，叶子上带 `shared_rooms`（所属的所选房间数，≥ 2 才出现），
  所以 `Σ rooms[].count ≥ total_count`。构件属于哪几间房来自 `rooms=` 解算时的 `面板 → 构件` 边（读透形态现算；落盘形态读
  `room_relate` 的 `in`）——`RoomScope::member_rooms`，`nearby` 的响应一字不变。
- **叶子**：`leaf_count` 是叶子放置数（跨房构件每间房各算一次）；≤ `leaf_cap`（5000）时全部 `elements` 内联、`leaves_inline = true`；
  超过就整体省略（所有 `elements` 键不出现）、`leaves_inline = false`，客户端按 `unit=` / `other_noun=` 再取一组——那一次响应里只有
  被点到的单元 / noun 组带 `elements`，`inlined` 字段说明是哪一组（`unit:24381_1200` / `other_noun:PANE`），`leaves_inline` 仍为 `false`。
- 与 `nearby` / `nearby/refnos` 的关系：同参数下 `total_count` = `nearby/refnos` 的 `refnos` 去重数；`room_status` / `warnings` 同一份；
  不再重复给 `filter_options` / `spec_groups` / `groups`。
- 错误：`rooms=` 缺失 400；不在册房间 400；房间体制不可用 422 `rooms_unavailable`（`detail.status` 同 §4.13.4）；其余同 `nearby`
  （404 / 503 / 422 `identity_mismatch`）。

**`tubes=1`——BRAN 单元下列出直段（2026-09-22；plant3d-web 方案 B，ADR 0068 追记 ②；计划
`docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md`；实现 `spatial_tree_view::TreeDraft` + `spatial_tree_tubes`）**

隐式管身（TUBI 直段）没有自己的 refno、不进空间树（ADR-072 D4 保持），所以缺省的树**列不出、也数不到**直段。`tubes=1`（`true/yes/on`
亦可；非法值 400）打开后，服务端折完管件、再按树里出现的每个 **BRAN 单元**读它根名下的管身记录（投影里有回执的根取投影——与 `/model/records`
同一谓词；其余冷读 `tubi_relate`，只按 `in` 一路各走索引、一次往返），把落在范围内的直段挂到该单元下：

- **缺省关**：不带 `tubes=` 的响应与不认识这个参数的旧版**逐字节相同**（下面所有新键都不出现）；两条树路由都认它。
- **响应**（只在 `tubes=1` 时出现）：`units[].tubes[]` 每段 `{ ordinal, from, to, from_noun, to_noun, distance, length, aabb: {min, max},
  invalid, shared_rooms? }`；`units[]` / `unit_types[]` / `specs[]` / `rooms[]` 各多一格 `tube_count`；顶层多 `total_tube_count`（按身份去重，
  跨房直段只算一次）与 `tubes_truncated`。`from` / `to` 是两端管件的 `a_b`（容器头 / 尾那一段的一端是 BRAN 自己，`*_noun` 为 `BRAN`）；
  两端 noun 命中里有就用命中的，没有回记录读一次，读不出为空串；`length` 是直管长度（mm，存下的世界变换 z 轴列的模）；`aabb` 给前端定位飞行。
- **身份**是 `(单元 refno, from, to, ordinal)` 四元组：`ordinal` 是同 `(from, to)` 内的序号（e3d-model `route_ordinal`），无关两段常为 0。
  落盘行在 T2 落 `route_ordinal` 之前没有这一格，冷读路按同 `(from, to)` 内的出现序（分支槽位序）补号——同 `(from, to)` 多段的 BRAN
  在冷读路上序号可能与投影不一致，是已知窗口。
- **计数**：`count` / `total_count` **不变**，直段不是构件、不算进去；`tube_count` 是该节点下落在范围内、属于该房间的直段**放置**数。
  直段放置数**计入 `leaf_count`、共用 `leaf_cap`**：超上限时 `tubes` 与 `elements` 一起省略，`unit=` 点到的单元连直段一起内联；
  `other_noun=` 组没有直段。`tubes[]` 按距离、再 `(from, to, ordinal)` 排；`min_distance` 仍只看管件。
- **过滤链**与管件同一把尺、走它能走的几格：范围按 `collect_candidates` 逐分支同判据对单个盒复算（`spatial_query::measure_box_in_range`：
  点 + sphere 盒到点 ≤ r；点 + cube 与 `center ± r` 相交；refno 目标与 `target ± r` 相交、sphere 再盒到盒 ≤ r；中心线走廊逐段探针 +
  走廊到盒距离）；`include_self=false` 时目标子树里的 BRAN 是「自己」，它的直段不列；`nouns=` 点名了类型却没点 `TUBI` → 直段不出；
  `keyword=` 用容器 BRAN 的 refno + `TUBI` 走同一个谓词；`dbnums` / `spec_values` / `rooms` 由「它挂在哪个单元下」隐含（直段与它的 BRAN 同库、
  同专业；单元能出现说明成员过了这几格）。
- **归属哪间房**（D4）：`member_rooms[from] ∪ member_rooms[to]`——两端管件任一属该房就挂到该房下的这个单元；只往**已有的单元节点**下挂
  （成员都被过滤掉的房间没有节点可挂）。两端都不是任一所选房间的成员时分两档：任一端**在范围里**（是候选）却不是成员 = 已经判过、属于别处
  ——它旁边的管件正是这样被 `rooms=` 滤掉的，直段跟着不列（R432 整房真机 176 段里 8 段是这一档：穿过房间盒、两端却在邻房）；两端都**不在
  范围里** = 解不出，按该单元出现的每一间房兜底。放到 ≥ 2 间房的带 `shared_rooms`。
- **预算**：一次响应最多为最近的 **500** 个 BRAN 单元（按最近成员距离）补直段；超出 `tubes_truncated=true`、`warnings` 加一句、没读的单元
  `tube_count = 0`。
- **已知漏洞**（接受，D1 取 (b)）：**孤直段**——一条 BRAN 的管件全在范围外、只有直段穿过范围，这根不在树上，直段补不出来；要全就得给
  管身另建一棵树，真被踩到再升。
- 与 `/model/records` 的关系：范围 = 整房时，一条 BRAN 在房间树里的 `tube_count` ≤ 它 `model/records` 里 `is_tubi` 的条数（相等或更少；
  多了就是 bug）。

#### 4.13.6 `GET /api/v1/spatial/rooms/{refno}/tree` — 一间在册房间的房间层级树（2026-09-20）

出处同 §4.13.5（计划 §4.5，给模型树 ROOM 标签页在 gen-model-v1 源下浏览一间房）；实现 `spatial_tree_view::room_tree_params` +
`handlers::spatial_room_tree`，之后与 §4.13.5 同一条流水线。

- **范围就是这间房**：目标 = 路径里的房间（`a_b`；同 `nearby?refno=` 的口径，它的并集盒来自投影里祖先链含它的记录——也就是它名下的
  面板——投影没有再看树上它自己的条目），`shape=cube`，`rooms=` 它自己，`include_self=false`（房间自己的面板不算成员）。
  `radius` 在这里是**盒的外扩量**，缺省 0 = 只要与房间盒相交、上限同 `nearby`（100 000 mm）。
- **不接受** `refno` / `x,y,z` / `source_mode` / `rooms`（都 400，范围与房间已由路径定死）；其余过滤（`nouns` / `keyword` /
  `include_negative` / `dbnums` / `spec_values`）与叶子选择器（`unit` / `other_noun`）、`tubes=1`（§4.13.5，2026-09-22）透传。
- 响应同 §4.13.5，`rooms[]` 恒一条、`center.source = refno_aabb_center`。房间从没生成过面板模型（投影与树里都没有它的盒）→ 404
  `not_found`；不在册（名字不含房间关键词或房间号不合规）→ 400；房间体制不可用 → 422 `rooms_unavailable`。

错误一览（九条路由）：400 `bad_request`（参数）、404 `not_found`（refno 模式中心在投影与树里都没有盒；中心线模式 `branch_query`
找不到这条 refno）、422 `precondition`（中心线模式源不是 BRAN / 路由容器，或一段都拼不出）、422 `identity_mismatch`（§3）、
503 `spatial_not_ready`（上文，唯一带 `Retry-After` 与非空 `detail` 的一档；`centerline` 不碰树、没有这一档）；其余暂时故障
（`DirectTreeService` 取不到、MBD 服务不可用等）按 §3 统一信封 500。
> 三维校审域（`/api/review/*`、`/api/users/*`、`/api/auth/*`、`/files/review_attachments/*`；plant3d-web 写死的契约路径，不在 `/api/v1` 下，响应沿用旧端 `{success,…}` 信封而非 §3 的 `ApiError`）另见 `docs/specs/review-api.md`（自 plant-model-gen `:3100` 移植，`docs/plans/2026-09-12-review-backend-port-plan.md` Q10）。

### 4.14 `GET /api/v1/datacenter/deletes` — 交付删除记录（`datacenter_delete`）只读分页（2026-09-23，plant-ui #1）

- 读的是 **交付删除事件**（CONTEXT 词条）：交付单元自身被删 / 改位号时，增量随窗语句记录「这个 refno 曾发生过删除或改名」。
  表 `datacenter_delete` 的主键只取 refno 值 `datacenter_delete:⟨ref0_ref1⟩`，不含 `pe` 或 `name`；写入使用 `INSERT IGNORE`，同一
  refno 只保留首次事件。增量写入前会按同一 refno ID 批量点查 `datacenter_version`，仅已发布过的单元落事件。写者只有增量，
  `docs/plans/2026-09-22-datacenter-delete-records-dev-plan.md`）。给外部提资 / 中台侧拉「该删什么」用。
- **只读**：本服务对这张表只写不删（计划 D7），这条路由一个 `DELETE` 都没有——删行归 rs-server 在中台 `remove-apply`
  成功之后。**不带 `instance_code`**：那是 rs-server `change_name_into_illegal(name)`（去首 `/`、非法字符换 `_`）的编码规则，
  本服务不复制（计划 D3），调用方拿 `name` 原文自己编。鉴权 / 身份与本组其余 `/api/v1` 路由同门（`project` / `mdb` / `namespace`
  照 §3 校验）。
- 查询串（列表按本服务惯例**逗号分隔**）：

  | 参数 | 类型 | 含义 |
  |---|---|---|
  | `zone_refno` | `a/b[,a/b…]` | 只要 `zone_refno` 在其中的行（走 `idx_datacenter_delete_zone`）。`a_b` 也收。缺省不按 ZONE 筛 |
  | `dbnum` | int | 只要这个库的行 |
  | `reason` | `deleted` \| `renamed` | 大小写不敏感；缺省两种都要 |
  | `since_sesno` | int | 只要 `sesno >= since_sesno`（发生在这一版及之后）的行。**须同时给 `dbnum`**：sesno 是各库自己的会话号，跨库比大小没有意义，缺 `dbnum` → 400 |
  | `from` | RFC3339 \| `YYYY-MM-DD` | 只要 `sesno_time >= from` 的行（含）。RFC3339 须带时区；纯日期按**北京时间零点**。不要求 `dbnum`（时刻跨库可比） |
  | `to` | RFC3339 \| `YYYY-MM-DD` | 只要 `sesno_time < to` 的行（**不含**），写法同 `from`。`to=2026-09-28` = 到 9/28 北京时间零点为止，即含 9/27 全天 |
  | `limit` | int | 一页几条：缺省（或 `0`）500、上限 5000（超过夹到上限、不报错） |
  | `start` | int | 跳过前几条 |

- **按时间范围查**（`from` / `to`，2026-09-24）：半开区间 `[from, to)`，按天 / 按周连着查不重不漏；两个都可只给一个。按真实时刻比、不按文本比，
  存的文本带什么时区都对。带了任一个，`sesno_time` 为 `null` 的行就不出。查询串里时区的 `+` 必须写成 `%2B`（否则被解成空格、回 400），或者直接用 `Z`：

```text
GET /api/v1/datacenter/deletes?from=2026-09-21&to=2026-09-28                                    # 北京时间 9/21–9/27，所有库
GET /api/v1/datacenter/deletes?zone_refno=7997/5&reason=renamed&from=2026-09-23T00:00:00%2B08:00&to=2026-09-24
GET /api/v1/datacenter/deletes?dbnum=7997&from=2026-09-22T16:00:00Z
```

  这个时刻的边界：它是变化所在那一版（与 `sesno` 同一版）在 E3D 会话页上的写入时刻；来自写会话那台 E3D 的时钟，gen-model 按本机时区
  解释后存 UTC；同一 `[refno, 老位号]` 在更晚一窗再落一次，时刻跟着挪到后面那次。**别拿它做增量游标**：watcher 停过、整窗重放时，晚落库的行
  时刻反而更早，按「上次拉到的时刻之后」轮询会漏行——增量消费用 `dbnum + since_sesno`。

- 响应（`DeleteRecordsPage` 原样透传；行序 `dbnum, sesno, name, id` 是全序，表不动时翻页不漂）：

```json
{ "items": [
    { "refno": "7997/3", "name": "/A-OLD", "type": "BRAN", "zone_refno": "7997/5",
      "dbnum": 7997, "sesno": 12, "sesno_time": "2026-09-22T02:00:00+00:00",
      "reason": "renamed", "published": true, "published_status": "Insert" },
    { "refno": "7997/21", "name": "/E-101", "type": "EQUI", "zone_refno": null,
      "dbnum": 7997, "sesno": 30, "sesno_time": null,
      "reason": "deleted", "published": true, "published_status": "Delete" } ],
  "total": 2, "start": 0, "limit": 500, "has_more": false }
```

  | 字段 | 含义 |
  |---|---|
  | `refno` | 交付单元参考号 `a/b` |
  | `name` | **要在中台删掉的那个位号**（E3D `NAME` 原文，含首位 `/`）：被删 → 删前的位号；改名 → 改前的位号 |
  | `type` | 交付单元 noun（`BRAN` / `SUPPO` / `EQUI` / `HANG` …，按项目 `delivery_unit_types` 配置） |
  | `zone_refno` | 沿 owner 链最近的 ZONE `a/b`；链断 `null`（按 ZONE 筛时自然漏掉，调用方要兜底就按 `dbnum` 拉） |
  | `dbnum` / `sesno` / `sesno_time` | 发生在哪个库、哪一版（这次改名 / 删除所在的会话，不是窗口末端）、同一版在 E3D 里被写入的时刻（RFC3339，增量落的是 UTC 文本 `…+00:00`，要本地时间自己换；读不到 `null`） |
  | `reason` | `deleted` \| `renamed` |
  | `published` | `datacenter_version:{refno}` 有行 = 这个单元提资过。新写事件在写前已过存在性门，正常恒为 `true`；`false` 只用于兼容旧行或异常诊断 |
  | `published_status` | 那行的 `status`（`Insert` / `Modify` / `Delete`）；旧行或异常情况下 `published = false` 时 `null` |
  | `total` / `start` / `limit` / `has_more` | 过滤后总数（不分页）/ 起点 / 实际生效的页大小 / `start + items.len() < total` |

- **边处理边删的调用方别推进 `start`**：删掉的行会让后面的行整体前移，按「已读条数」翻下一页会整页跳过；这种用法始终 `start=0`
  反复拉，直到 `items` 为空。同一 refno 后续事件由 `INSERT IGNORE` 忽略，已有行不会改字段或挪位。
- 与 `datacenter_version` 的 `status = 'Delete'` 行**并存**（计划 D9）：被删单元两边都有，消费端按位号去重；改位号只有这里有。
  改回原名这条路由不判——调用方自己比对再跳过，判据是**待删位号推出的 `instance_code` == 该单元当前 `NAME` 推出的 `instance_code`**
  （即本次要发布的那个）。**别拿 `datacenter_version` 行上的 `instance_code` 比**：那是上次提资的值，普通改名（以 X 提资后改成 Y、还没再提资）
  时它仍是 X，会把该删的 X 误判成改回原名跳过。
- 错误：400 `bad_request`（`zone_refno` 解不出 refno、`reason` 不是两个枚举之一、带 `since_sesno` 没带 `dbnum`、`from` / `to` 解不出或
  `from >= to`、数字参数非数字）、422 `identity_mismatch`（§3）；
  库读不到按 §3 统一信封。

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
     `ready_empty` 才能把空 `memberships` 显示成“不属于任何房间”。2026-09-16 起
     `ready_empty` / `ready_nonempty` 在落盘形态下也回得出来（此前服务重启过就一律
     `not_computed`，见 §4.7.2 的两层判据）——取值集合与含义一个字没变，只是不再恒假。
     同日 `/dbnums` 每行多一格可选的 `room_alignment`（§4.7）：`current: false` 就是
     「这个库此刻只能回 `not_computed`」的解释，别把它画成告警。
   - 你**直连 SurrealDB** 读的 `room_relate` / `room_panel_relate` / `fn::room_code`
     （房间浏览器、材料表 surql）不随本节退役，写者是模型根的 process-store/durable
     发布事务；kv-mem 启动屏障完成前不得把空结果当最终值。

`DbnumStatus` 上少的那一枚是 `room_sesno`（连同 `room_sesno_time` 等五列）：
水位从三枚回到两枚，`applied_sesno` / `model_sesno` 语义不变。**少的格别退回默认值再画**
——`room_sesno` 缺席不等于 `0`，`0` 读起来是「房间一次都没算过」，那是另一句话。
