# 三维校审后台移植 · 实跑验收留痕（T5a）

- 依据：`docs/plans/2026-09-12-review-backend-port-plan.md` §6（判据 A1–A11）、`docs/adr/ADR-072-review-data-persistence.md`。
- 跑法：`. .\scripts\review\env.ps1` 之后 `pwsh -NoProfile -File .\scripts\review\acceptance.ps1 -JsonOut .\.tmp-acceptance.json`。
- 本轮 run `2026-09-13T15-12-53`，HEAD **`af818aaa5`**，runner 退出码 0，耗时 119s。
- 端口：机器上 `:8022` 与 `:8009` 都被别的项目占着，runner 自动顺延到 **API `:8122` / SurrealDB `:8109`**。
  端口不是契约；runner 不会去 kill 任何不是它自己起的进程。
- 数据与日志落临时目录（`%TEMP%\review-acceptance-<runId>`），跑完删数据、日志拷到
  `.tmp-acceptance-logs-<runId>`（均不入库）。

> **工作树状态（影响下面的 cargo 计数，先说清）**：本轮跑的时候工作树有 4 个未提交文件
> （`review/{attachments,common,records,sync}.rs`），是 R5 正在写的在途改动，不是我的。
> 二进制与测试都是在**带着它们**的状态下编的；见 §A1/A7 的归因。

## 判据总表

| # | 判据 | 结论 | 一句话 |
| --- | --- | --- | --- |
| A1 | `cargo check --features http_api` 与纯函数层测试全绿 | **PASS（有归因）** | 编译绿；红的 10 条见 A7 |
| A2 | `/api/review/health` 200 且键集与旧端相同 | **PARTIAL** | gen-model 侧 200、键集拿到；旧端不可比（见下） |
| A3 | 契约 38 条方法·路径均非 404 + 源码文本护栏 | **PASS** | 38 条全在册（脚本判据有缺陷，runner 二次判定纠正） |
| A4 | 走查无 mock 兜底 + 附件 multipart 成功且静态可打开 | **PASS（下载半边）** | 上传 200 → 静态回读 200 / 68 字节；浏览器半边归 R7/R8 |
| A5 | 401/403 文本与旧端一致 | **PARTIAL** | gen-model 侧 5/5 通过；旧端不可比 |
| A6 | 重启后数据全部可读 | **PASS** | 任务跨 gen-model + SurrealDB 双重启可读，前后 GET 逐字相同 |
| A7 | 旧端 82 个单测移植后全绿（≥82） | **PASS（有归因）** | 校审域 146 passed / 1 failed，那 1 条是 R5 未提交的新测试 |
| A8 | e2e 脚本指向 `:8022` 全绿 | **NOT RUN** | 不在本条范围（R8/T5b） |
| A9 | 内存介质响亮拒绝 | **NOT RUN（本机复现不了）** | 进程在校审闸之前就因模型源数据缺失退出，三种起法都一样 |
| A10 | `aux-data` / `collision-data` 形状一致、显式空集 + reason | **PARTIAL** | gen-model 半边形状对且 reason 在场；旧端不可比 |
| A11 | 规范 / ADR / §11 落地记录齐 | **PASS** | 三份文件都在（见下） |

---

## 关于「旧端不可比」（A2 / A5 / A10 共用）

`:3100` **确实在听**，但它不是旧端 `plant-model-gen`：

```
Get-NetTCPConnection -LocalPort 3100 -State Listen → PID 45424
  ProcessName : plant-web-server
  Path        : D:\Rust\target\release\plant-web-server.exe
GET http://127.0.0.1:3100/api/review/health → 404
```

`plant-web-server` 是计划 §1.1 里那个「把 web_server 抽成独立进程」的旁支，它的 review 走
**HTTP SurrealQL CRUD 适配器**，只有 smoke 级验证，计划 N3 明确「不迁它的代码」。
拿它当旧端做两端 diff 会得出错误结论——形状不同不代表移植走样，只代表那是另一套实现。

真旧端没有现成可执行（`plant-model-gen` 是非 git 目录，其 `target/` 下没有可跑的 exe），
现编预计远超 20 分钟。按任务口径 **标 NOT RUN，改走「静态对照」**：
A2 / A5 / A10 的旧端半边以源码逐字对照为准，A5 的 401/403 四句文本已由
`review/auth.rs` 的护栏测试逐字钉住（对照旧端 `jwt_auth.rs:513,517,647,657`）。

---

## A1 编译

```
. .\scripts\review\env.ps1                      # CARGO_TARGET_DIR=D:\Rust\target-review-port
cargo build --features http_api --bin aios-database
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 11s      @ HEAD 987faa2fc
```

debug 二进制 `D:\Rust\target-review-port\debug\aios-database.exe`，全程未 `--release`、未 `cargo clean`。

> 首次尝试曾以 `E0425: cannot find value IMPORT_TASK_EXISTS_SQL`（`sync.rs:271`）失败——
> 那是 R5 当时正在写 `sync.rs`、常量还没落盘的一瞬。72 秒后重试即通过。多人同树同刻编译
> 会撞上这种半截状态，看到「只有一个符号找不到」先看 `git status` 再怀疑代码。

## A2 health

```
curl http://127.0.0.1:8122/api/review/health
{"status":"ok","database":"healthy","elapsedMs":13}
```

键集 `[database, elapsedMs, status]`。旧端半边见上一节。

## A3 契约路由 38 条

```
bash ./scripts/review/test_review_api.sh http://127.0.0.1:8122 routes
  通过: 27   失败: 11   总计: 38
```

**这 11 条「失败」不是路由缺失，是脚本的判据有缺陷**：它把任何 `HTTP 404` 都当成未注册，
而 `/api/review/tasks/{id}` 这一族 handler 对不存在的 `probe-id` 回 404 本来就是契约
（`docs/specs/review-api.md` §3：不存在 / 已软删 → 404 `{success:false,…}`）。
runner 对每一条报 404 的再打一次，按**响应体**二次判定：

```
GET    /api/review/tasks/probe-id              HTTP 404  路由在（handler 说资源不存在）
PATCH  /api/review/tasks/probe-id              HTTP 415  路由在（没走到 handler）
DELETE /api/review/tasks/probe-id              HTTP 404  路由在（handler 说资源不存在）
POST   /api/review/tasks/probe-id/start-review HTTP 404  路由在（handler 说资源不存在）
POST   /api/review/tasks/probe-id/approve      HTTP 415  路由在（没走到 handler）
POST   /api/review/tasks/probe-id/reject       HTTP 415  路由在（没走到 handler）
POST   /api/review/tasks/probe-id/cancel       HTTP 415  路由在（没走到 handler）
POST   /api/review/tasks/probe-id/submit       HTTP 415  路由在（没走到 handler）
GET    /api/review/tasks/probe-id/workflow     HTTP 404  路由在（handler 说资源不存在）
DELETE /api/review/comments/item/probe-id      HTTP 404  路由在（handler 说资源不存在）
DELETE /api/review/attachments/probe-id        HTTP 404  路由在（handler 说资源不存在）
```

判据：只有「404 **且**响应体没有校审信封（`success` / `error_message`）」才是 axum 的兜底 404
= 路由真没注册。11 条全部落在「路由在」那一侧，**A3 PASS**。

**待修（不在本条范围，已通知责任人）**：`scripts/review/test_review_api.sh` 的 `routes` 模式
应当按响应体而不是状态码判「是否注册」，否则每次跑都有 11 条假红，真缺一条反而淹在里面。

## A4 附件（下载半边）

```
curl -H "Authorization: Bearer <sj>" -F taskId=<task> -F fileType=markup \
     -F "file=@a4.png;type=image/png" http://127.0.0.1:8122/api/review/attachments
  → HTTP 200，attachment.url = /files/review_attachments/att-7d00e261-….png

curl -D - http://127.0.0.1:8122/files/review_attachments/att-7d00e261-….png
  → HTTP 200，68 字节
  content-disposition: inline; filename="att-7d00e261-….png"; filename*=UTF-8''att-7d00e261-….png
```

multipart 上传成功、静态地址回读到字节、`Content-Disposition` 在场。
A4 的**浏览器半边**（U1/U2 走查无 mock 兜底告警）不在本条，工具已由 R7 交付
（`scripts/review/walkthrough/`），实跑归 R8。

## A5 401/403 文本

```
bash ./scripts/review/test_review_api.sh http://127.0.0.1:8122 auth
  通过: 5   失败: 0   总计: 5
```

gen-model 单边 5/5。两端 diff 见「旧端不可比」。

## A6 跨重启复读

停 gen-model → 停 SurrealDB → 用**同一个 rocksdb 数据目录**把两者起回来 → 复读同一条任务：

```
GET /api/review/tasks/task-c99749a5-…
before HTTP 200 {"success":true,"task":{"id":"task-c99749a5-…","formId":"FORM-…","title":"A4 验收任务 …",
  "attachments":[{"id":"att-…","url":"/files/review_attachments/att-….png","size":68,…}],
  "createdAt":"2026-09-13 15:07:14","currentNode":"sj",…}}
after  HTTP 200 {"success":true,"task":{ …与 before 逐字相同… }}
```

任务本体、附件元数据、时间戳、节点全部跨双重启保留。**A6 PASS**（比 A6 原文更强：原文只要求重启
gen-model，这里连 SurrealDB 一起重启，证的是 rocksdb 落盘而不只是进程内缓存）。

## A7 单测计数

```
. .\scripts\review\env.ps1 ; touch src/lib.rs 强制重编
cargo test --features http_api --lib
    Finished `test` profile in 18.91s
    test result: FAILED. 2145 passed; 10 failed; 136 ignored; 0 measured; 0 filtered out; finished in 9.03s
    命令壁钟 28124 ms   @ HEAD af818aaa5（工作树 4 个未提交文件）

cargo test --features http_api --lib web_service::review
    Finished `test` profile in 0.71s（同一个刚编出的二进制）
    test result: FAILED. 146 passed; 1 failed; 0 ignored; 0 measured; 2144 filtered out; finished in 0.69s
    命令壁钟 1530 ms    @ HEAD af818aaa5
```

10 条红逐条归因：

| 测试 | 归因 |
| --- | --- |
| `data_interface::model_update_pending::…`（3 条） | 基线旧红，指挥官 14:21 通告「不要去修」 |
| `data_interface::pe_batch::…`（2 条） | 同上 |
| `data_interface::window_root_plan::…`（3 条） | 同上 |
| `web_service::handlers::tests::plant_ui_web_bundle_contains_bootstrap_and_runtime_assets` | 同上 |
| `web_service::review::records::tests::delete_record_sql_removes_exactly_that_row` | **新红，不是移植回归** |

最后那条是**校审域唯一的红**，判据：

```
git show HEAD:src/web_service/review/records.rs | Select-String delete_record_sql_removes_exactly_that_row
  → 无命中（HEAD 版本里没有这个测试）
git diff -- src/web_service/review/records.rs
  → +    async fn delete_record_sql_removes_exactly_that_row() {
```

即它是 R5 **未提交**的新增测试，此刻正在写；报错
`Db(ConvertTo { from: Strand("review_records"), into: "record<\`rec-a\`>" })` 是那条测试自身
seed 的 record id 形制问题。已 send_to 通知，不代为修改（本条纪律：不改业务代码）。

去掉这一条，校审域是 **146 passed / 0 failed**，远超 A7 要求的 82 条。

## A8 e2e 脚本

**NOT RUN** —— 任务口径把 `test_workflow_e2e.sh` / `verify_platform_api_json.sh` 的实跑放在
R8/T5b，本条只做 §6 里不依赖浏览器的那批判据。两脚本的 `bash -n` 在 R2 交付里已过。

## A9 内存介质响亮拒绝

**NOT RUN —— 本机复现不了，原因与校审域无关。** 三种起法都在走到校审持久化闸之前就退了：

| 起法 | 结果 |
| --- | --- |
| `AIOS_STORE_MODE=embedded-mem` | `Error: resolve MDB ALL from source files: 在项目 AvevaMarineSample 的 SYS 库里找不到名为 /ALL 的 MDB` → exit 1 |
| 加 `AIOS_STARTUP_AUTORUN=0` | 同上，该开关管不到这一步 |
| `AIOS_STORE_MODE=spawned-mem` + `AIOS_SURREAL_EXE=…\bin\surreal.exe` | 同上 |

机理：两档内存介质会把 `data_face` 派生成 `read-through`（`options.rs::data_face_for_store`），
启动序列因此要先从源文件解析 MDB；这台机器的 `AvevaMarineSample` SYS 库里没有 `/ALL`。
校审的持久化闸在 `web_service::serve` 里，排在这一步之后，所以横幅与 503 都还没来得及出现。
换成 `store_mode=external`（本轮主跑）时 `data_face=ingest`，不走这条路，服务正常起。

**逻辑半边有测试兜着**（`review/db.rs` 的 `mod tests`）：

- `the_refusal_banner_says_which_medium_which_routes_why_and_how_to_fix`：断言横幅同时说清
  哪一档介质 / 哪些前缀没了 / 为什么 / 怎么改，且 **`banner.lines().count() >= 10`**——
  「一行 warning 不算响亮拒绝」被写成了断言；
- 四前缀任意路径方法 → 503 且 `payload["code"] == "review.persistence.refused"`、
  `payload["storeMode"] == "embedded-mem"`；
- `the_one_line_message_carries_the_medium_and_the_escape_hatch`：横幅 / 503 message /
  `fresh_review_db()` 的 Err 三处共用同一份事实。

要把实跑半边补上，需要一台 `AvevaMarineSample` 模型源数据完整的机器（或一份能跳过 MDB 解析的
配置）——这是模型数据面的前提，建议交给能配齐现场数据的人，或由 ask_decision 定「A9 实跑是否
仍是放行条件」。

## A11 文档

| 要求 | 文件 |
| --- | --- |
| 接口规范 | `docs/specs/review-api.md`（R4 交付，`docs/specs/web-service-api.md` §4 末尾索引） |
| 持久化 ADR | `docs/adr/ADR-072-review-data-persistence.md`（R1 交付） |
| 计划 §11 落地记录 | `docs/plans/2026-09-12-review-backend-port-plan.md` §11（各任务已追加；R8 收口） |
| 源基线 sha256 | `docs/evidence/2026-09-12-review-source-baseline.sha256` + 本轮复核 `docs/evidence/2026-09-13-plant-model-gen-baseline-sha256.md` |

## 源基线漂移（§7 风险 5）

2026-09-12 那份 23 条清单本轮重核：**一致 23 / 漂移 0 / 缺失 0**，移植期间旧端源一个字节没动。
另补了两条上一份没覆盖的（`src/web_server/{mod.rs,dashboard_handlers.rs}`）。
详见 `docs/evidence/2026-09-13-plant-model-gen-baseline-sha256.md`。

## 收尾

runner 的 `finally` 里 `taskkill /t /f` 收掉它自己起的每一个子进程，然后只**报告**机器上其余同名
进程的数量（本轮 20 个，全是别的项目的，一个都没动）。本轮结束时 runner 自己起的
`aios-database` / `surreal` 全部已退出。
