# 给前端进度条补齐的六项后端改动

前端要在执行与预览两侧都画出**确定态**进度条。摸完两侧代码后，六件事必须在 gen-model 这边
做，前端做不了。决定见 plant-ui 的 `docs/adr/0007-progress-counters-live-on-the-task-record.md`
与 `0008-model-service-address-is-a-setting.md`，客户端字段出处见
`plant-ui/design/MODEL-UPDATE-FIELD-MAP.md`。

> 本文件**故意不写行号**。`manual_update.rs` 正在被频繁改动，行号很快就对不上，按函数名与
> 判断条件定位。

---

## 1. `http_api` 补进 `console` feature

`Cargo.toml` 现在是 `console = ["ws", "gen_model", "manifold", "occ", "project_hd", "mqtt"]`，
漏了 `http_api`。而 `docs/specs/web-service-api.md` §2 写的是「默认不启用，`console` feature
可包含它」——实现漂离了文档。`default` 不动。

**验收**：`cargo build --features console` 之后 `/api/v1/health` 能通。

---

## 2. `TaskEntry` 带上四个进度计数

**这是六项里最要紧的一项。** `total_batches` / `batches_done` / `total_units` / `units_done`
存进 `TaskEntry`，跟着 `GET /api/v1/tasks/{id}` 与 `GET /api/v1/tasks` 一起返回。

只发事件不够：事件不重放，`seq` 是连接内单调的序号而不是可回放的游标。客户端晚连一秒、
断一次线，分母就永远拿不到了。放进任务记录之后，WebSocket 断开只影响逐行明细，进度条依旧准确。

`web_service/handlers.rs` 的 `update_execute` 里那个 `ManualUpdateProgress` 回调现在只做
`tasks.bump_events(&tid)`，改成按事件类型分别累加即可。

**别指望 `events_seen` 能换算回进度**：它把四类事件混在一起数，而 `DataBatchFinished`
有两个 emit 点。反推只能靠猜。

**验收**：执行期轮询 `GET /tasks/{id}`，四个计数单调递增，终态时 `*_done == *_total`。

---

## 3. 两个阶段各发一条带 `total` 的事件

`ManualUpdateEvent` 加 `DataPhaseStarted { total_batches }` 与
`ModelPhaseStarted { total_units }`，在各自的循环开始前发。

两个总数在循环前就已经算好了，取现成的即可：模型侧是 `merge_unit_worklist` 的返回值
`worklist`，数据侧是 `execute_manual_update` 里排序后的 `dbnums`。

**前端千万不能自己算这个分母。** `MODEL-UPDATE-FIELD-MAP.md` 早先写的
「分母 = 预览的新单元数 + 待重试单元数」两个方向都会偏：执行开始时会重新扫描，预览之后
新产生的会话自动并入（`merged_sesnos`）带来新单元，偏小；而 `merge_unit_worklist` 按
`(dbnum, root_refno)` 去重，待重试单元若本窗口又被改到会与新单元塌成一条，偏大。

---

## 4. 批次事件包到 `execute_one_dbnum` 的函数边界上

现在 `DataBatchStarted` 在函数很靠后的位置才 emit，它前面排着**四条直接 return 的路**，
每一条都产出了 `DataBatchResult` 却一条事件都不发：

| 出口 | 条件 | 结果状态 |
|---|---|---|
| 同 dbnum 多文件 | `candidates.len() > 1`（在 `execute_manual_update` 里 `continue`，压根不进本函数） | `Skipped` |
| 文件回退 | `cand.file_latest_sesno < applied` | `Skipped` |
| 首次按需初始化 | `needs_initial_load(...)` | **`Applied`（是成功的）** |
| 无事可做 | `SesnoRangeResolver` 返回 `Ok(None)` | 无结果 |

所以 `total_batches = dbnums.len()` 的话，阶段一的条**永远走不满**，会卡在那里直到整个
任务结束。

修法不是逐个补 `emit`，而是把「开头发 `Started`、每条出口发 `Finished`」提到函数边界上，
让「每个批次都有头有尾」成为不变式——否则以后再加一条早退路径又会漏。`execute_manual_update`
里 duplicate-dbnum 那个 `continue` 分支也要一并发。

**验收**：构造一个同 dbnum 多文件的库，执行时前端能实时看到它被阻断，而不是等终态才冒出来；
且 `batches_done` 最终等于 `total_batches`。

---

## 5. 预览改成异步任务（ADR-0006）

`POST /api/v1/update/preview` 现在是同步处理器，`update_preview` 直接 await 掉
`preview_manual_update`，客户端超时给到 600 秒。而预览要逐个 dbnum 打开设计库文件按会话号
比对，单库可达数分钟——界面上只有一个转圈，十分钟说不出任何进展。

照 `update_execute` 抄一遍：202 + `task_id`，进度经 WS 推，终态落在 `GET /tasks/{id}`。
任务注册表、任务查询、WS 主题都是按 `kind` 泛化的，不认死 `manual_update`，换个 kind 就能复用。

扫描期间按 dbnum 发两条新事件：

- `PreviewDbStarted { dbnum, db_type, file_path }`
- `PreviewDbFinished { dbnum, pending_sessions, changed_elements }`

外加第 3 项那样的一条 total（分母是**本次真的会进扫描循环**的 DESI 库数，不是「已登记 DESI
dbnum 总数」——已登记里含文件缺失的库，它们不进循环）。终态 `result` 就是现在这个
`ManualUpdatePreview`，一个字段都不用改。

---

## 6. 预览的扫描循环先按 dbnum 排序

`preview_manual_update` 的循环是 `for (db_num, candidates) in by_dbnum`，`by_dbnum` 是
`IndexMap`，走的是 `WalkDir` 目录遍历顺序；而最终结果在返回前被 `dbnums.sort_by_key(|d| d.dbnum)`
排过。前端照事件到达顺序堆出来的列表，会在预览完成那一瞬间重排——人看着就是列表"跳了一下"。

执行侧本来就在循环前 `sort_unstable()`，两条路径对齐即可，前端照事件 append 就行。

---

## 顺带：前端那侧还有一处 8020

`rs-plant3-d/src/plant_ui_host.rs` 把模型服务地址默认写死成了 `http://127.0.0.1:8020`，而
本仓库 `DbOption.toml` 的注释已经交代过 8020 的回环侧被 plant-web-server 的 surreal 占着。
指向 8020 不会干净地连不上，而是打进那个实例、拿回一个看着像模像样的 HTTP 错误。
plant-ui 那份已经改了，**rs-plant3-d 这份才是真正会发出去的那份**。
