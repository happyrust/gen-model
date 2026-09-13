# 历史模型（按 sesno / time 生成）live 回执 · 2026-09-13 22:33–22:37

**目的**：对真库验证 `POST /api/v1/model/history/generate` 按指定 `sesno` 生成历史模型这条路是通的，并记录回执。
**结论**：通。`sesno` / `time` / `latest` 三种选择器都能在真库上落到正确会话并产出几何；旧会话的快照与最新快照
确实不同（EQUI 24384/24776 在 556 有 2 个实例，574 只剩 1 个）。

## 现场

| 项 | 值 |
|---|---|
| 二进制 | `aios_database.exe` debug，`0.1.23+gdec2c67d83b9.1789309834.dirty`（HEAD `dec2c67d8` + 工作树，2026-09-13 22:30 本轮 `cargo build` 产出） |
| 配置 | `DbOption.toml`（本目录副本）：AvevaMarineSample / MDB `ALL`，`data_face = read-through`，`store_mode = embedded-mem`，`http_api_addr = 127.0.0.1:8047`，独立 `meshes_path`（Temp 目录，不与别的实例共享） |
| 运行目录 | `%TEMP%\hist-live-20260913-223333`（仓外，结束后删除；stdout / stderr 抄在本目录） |
| 源文件 | `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`，文件最新会话 **574** |
| 启动 | `serve` 后 2 s `/api/v1/health` 即 `initialization.status = model_ready`、`features.history.status = available` |

## 用例与回执

`*.request.json` = 请求体；`*.task.json` = `GET /api/v1/tasks/{task_id}` 终态；`*.query-<tool>.json` = `POST /api/v1/model/history/query` 原样回包。

| # | 用例 | 请求 | 结果 | 回执要点 |
|---|---|---|---|---|
| 1 | BRAN 24384/26229 · latest | 不带 `sesno` / `time` | `succeeded` 648 ms | `24384_26229@574`，visited 13 / generated 16 / failed 0，mesh_written 10 / reused 6，unique 10 |
| 2 | BRAN 24384/26229 · **sesno=250** | `sesno: 250` | `succeeded` 645 ms | `24384_26229@250`，session_time `2026-08-20T01:24:28`；16 件几何**全部 mesh_reused**（与最新一致，该根 250→574 未变） |
| 3 | BRAN 24384/26229 · sesno=100 | `sesno: 100` | `failed` | `GEOMETRY_GENERATION_FAILED: generate root 24384/26229`——该根在 100 大概率还不存在，但错误码没落到 `REFNO_NOT_FOUND_AT_SESSION`（分类靠字符串匹配 `索引` / `not found`），消息也看不出原因 |
| 4 | sesno=99999（不存在） | `sesno: 99999` | `failed` | `SESSION_NOT_FOUND: … no session 99999; newest is 574; the nearest retained session is 574, below it`——e3d-io 的就近信息已经透进 message |
| 5 | time=2016-06-01 | `time` | `failed` | `TIME_BEFORE_OLDEST_SESSION`（该文件最早会话晚于 2016） |
| 5b | time=2026-09-01T00:00:00+08:00 | `time` | `succeeded` 507 ms | 解到 **264**（session_time `2026-08-28T17:21:30`），链序回走正确 |
| 6 | dbnum 7998 + refno 24384/26229 | 库号不符 | `failed` | `INVALID_SELECTOR: refno 24384/26229 belongs to dbnum 8000, not requested dbnum 7998` |
| 7 | `sesno` 与 `time` 同时给 | — | **HTTP 400** | 请求期就拒，不入队 |
| 8 | EQUI 24384/24776 · latest | — | `succeeded` | `24384_24776@574`，visited 2 / generated **1**（BOX 24384_24777） |
| 9 | EQUI 24384/24776 · **sesno=556** | `sesno: 556` | `succeeded` 516 ms | `24384_24776@556`，visited 3 / generated **2**（BOX 24384_24777 + BOX **24384_26467**）——净窗口探针显示该 EQUI 在 557 改过 `MEMBERS`，历史快照确实多出一个成员 |
| 10 | EQUI 24384/24776 · sesno=573 | `sesno: 573` | `succeeded` | `24384_24776@573`，generated 1，与 574 相同 |

查询 / 生命周期端点：

- `POST /query {tool: instances}`：`@556` 2 行、`@574` 1 行；`tubes` / `geometry` / `snapshot` 四个 tool 都 200。
- `GET /api/v1/model/history/24384_24776@556` → 200 回执；不存在的 key → 404。
- `DELETE /api/v1/model/history/24384_24776@573` → `{"status":"deleted"}`；随后 `GET` 404、`query instances` 0 行；邻居 `@574` 仍 1 行（不连累）。
- 独立 `meshes_path` 下落盘 11 个 mesh 文件（BRAN 10 + BOX 1），内容寻址，旧会话快照全部命中复用。

## 本轮看到的边界（与读码结论一致）

1. 只钉目标 DESI 库；CATA 与其它 DESI 仍是文件最新（`e3d_model_service.rs` `E3dDbResolver::resolve`）。本轮两个根都不涉及目录件变动，所以没暴露出来。
2. `Sesno(n)` 是 Exact，不就近（用例 4）。
3. 用例 3 的错误分类值得修：根在该会话不存在时应是 `REFNO_NOT_FOUND_AT_SESSION`（404），现在落成 500 级的 `GEOMETRY_GENERATION_FAILED`，且消息无原因。
4. 快照只在进程内：服务一停，本目录的 `*.query-*.json` 就是全部残留。
5. 服务日志里没有 `historical model ready …` 那一行（`enable_log = false`），端到端凭据只有任务表与本目录回执。

## 用例 3 的修复与复验（22:45–22:48，`fix-*.json`）

改动（未提交，见 `git diff`）：

- `e3d_model_service.rs`：新增 `SnapshotSourceError::RootNotInSession { root, dbnum, sesno }`；
  `generate_snapshot_source` 在求几何**之前**查该会话的元素索引（`input.index.owners`），根不在就以这个
  类型化错误退出。
- `historical_model.rs`：`classify_generation_error` 按 downcast 分码（`REFNO_NOT_FOUND_AT_SESSION` /
  `GEOMETRY_GENERATION_FAILED`），消息用 `{:#}` 带全因果链；删掉 `contains("索引") || contains("not found")`
  那段文本匹配。
- 单测 3 条：分类按类型不按文本；几何失败消息带全链；源码守卫「索引门 + 类型化错误排在
  `generate_snapshot_from_set` 之前」。`cargo test --lib -- e3d_model_service historical_model`：37 passed / 0 failed / 8 ignored。

同一配置重起修复后的 debug 构建（`0.1.23+gdec2c67d83b9.1789310749.dirty`，22:45:49）复验：

| 用例 | 请求 | 修复前 | 修复后 |
|---|---|---|---|
| fix-case3 | BRAN 24384/26229 · sesno=100 | `GEOMETRY_GENERATION_FAILED: generate root 24384/26229` | **`REFNO_NOT_FOUND_AT_SESSION: refno 24384/26229 is not in the element index of dbnum 8000 at session 100`** |
| fix-case3b | EQUI 24384/24776 · sesno=1 | — | `REFNO_NOT_FOUND_AT_SESSION … at session 1` |
| fix-case3c | 不存在的 refno 24384/99999999 · sesno=574 | — | `REFNO_NOT_FOUND_AT_SESSION … at session 574` |
| fix-control | BRAN 24384/26229 · sesno=250 | 成功 16 件 | 成功 16 件（不变） |
| fix-control | EQUI 24384/24776 · sesno=556 | 成功 2 件 | 成功 2 件（不变） |

## 怎么复跑

```powershell
# 起服务（仓外运行目录，DbOption.toml 用本目录副本，改 meshes_path / http_api_addr）
$env:DB_OPTION_FILE = "<run>\DbOption.toml"; $env:AIOS_STORE_MODE = 'embedded-mem'; $env:AIOS_DATA_FACE = 'read-through'; $env:AIOS_OPEN_BROWSER = '0'
Start-Process "<run>\aios-database.exe" -ArgumentList serve -WorkingDirectory "<run>"

# 生成 + 轮询
$r = Invoke-RestMethod -Method Post http://127.0.0.1:8047/api/v1/model/history/generate -ContentType application/json `
  -Body '{"project":"AvevaMarineSample","mdb":"ALL","dbnum":8000,"refno":"=24384/24776","sesno":556}'
Invoke-RestMethod "http://127.0.0.1:8047/api/v1/tasks/$($r.task_id)"
Invoke-RestMethod -Method Post http://127.0.0.1:8047/api/v1/model/history/query -ContentType application/json `
  -Body '{"snapshot_key":"24384_24776@556","tool":"instances"}'
```
