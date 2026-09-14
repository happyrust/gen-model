# 校审域响应形状对照复核（R5 / T2）

- 对应计划 `docs/plans/2026-09-12-review-backend-port-plan.md` §7 风险：「`review_api.rs` 单文件 7k 行，
  拆分时路径/形状走样 → 前端静默失效」。
- 新端：`src/web_service/review/{auth,users,tasks,records,comments,annotation_state,attachments,sync,mod}.rs`
- 旧端（只读）：`D:\work\plant-code\plant-model-gen\src\web_api\{review_api.rs, jwt_auth.rs, review_annotation_state.rs}`
- 契约参照（只读）：`D:\work\plant-code\old\plant3d-web\src\api\reviewApi.ts`
- 本轮基线 HEAD `065c7a57f`（本文与 §5 的两条护栏一并提交）；
  `cargo test --features http_api --lib web_service::review`
  → **150 passed; 0 failed; 0 ignored; 0 measured; 2144 filtered out**，
  `touch src/lib.rs` 强制重编 28.43s（首次 1m00s）。

> 交接：本条先后经 fable-5-1-31、opus-5-20 两任（均中途掉线），本轮由 opus-5-27 收尾。
> §1 是 fable-5-1-31 的测试照出来、opus-5-20 修掉并入库的缺陷；§2 起的逐条对照是本轮补的。

---

## 1. 抓到的缺陷：`type::record` 参数顺序倒置，四条前端主链路运行时 500

**这是本条任务抓到的唯一实质缺陷，已修并入库（`3b01b64e8`）。**

本仓 2.1.4 fork 的 `type::record` 是**值在前、表在后**。移植过来的 12 条语句一律写成
`type::record('表', $id)`，于是把表名当值、把 id 当表：

```
Db(ConvertTo { from: Strand(Strand("review_records")), into: "record<`rec-a`>" })
```

前端拿到的是 HTTP 500。**它解析得过、编译得过、原有单测也测不到**——只有真打到库上才会炸，
正是计划 §7 说的那种「静默失效」。

| 文件 | 处数 | 影响的前端链路 |
| --- | --- | --- |
| `records.rs` 502 / 570 / 601 / 623 / 871 | 5 | 确认记录的 CREATE / UPDATE / MERGE 与单条删除 |
| `comments.rs` 351 / 355 / 435 / 496 / 688 | 5 | 评论软删的查与写、作者查询、评论改文、批注字段更新 |
| `annotation_state.rs` 370 / 749 | 2 | 批注处理状态 UPSERT（两处 slot） |

修法：一律改为 `type::thing(表, id)`——与各文件里本来就能跑通的 seed 语句同款，
用嵌入式 `mem://` 实跑验证（`records::tests::delete_record_sql_removes_exactly_that_row`、
`clear_task_records_sql_only_touches_that_task`）。

新增源码文本护栏 `records::tests::type_thing_is_the_only_record_constructor_in_this_domain`：
扫本域八个文件的非注释行，禁止表名在前的写法（文档注释里引用旧端原文照常放行；
针用 `concat!` 在测试里拼出来，否则护栏自己那一行会被自己扫中）。
**变异验证**：把 `annotation_state.rs` 两处改回去 → 护栏真的红并逐行点名；还原后全绿。

坑已记进共享记忆 `mem-271`，建议其他从 3.x fork 搬 SurrealQL 的域也 grep 一遍。

---

## 2. 37 条路由逐条对照表

**做法不是肉眼比对**：脚本按 `async fn` 签名与同缩进 `}` 切出两端每个 handler 的完整函数体，
剥掉空行与行注释后逐行 `Compare-Object`。归一化只抹掉两处已知且与契约无关的东西——
两个 SDK fork 的类型名（`types::RecordId/Datetime` ↔ `sql::Thing/Datetime`）与 §1 修掉的
`type::record`→`type::thing`。**其余一律保留**：任何一个 `StatusCode::`、任何一句错误文案、
任何一个 `unwrap_or` 默认值有出入，都会落在差异里。

结论：**37 / 37 条在响应契约上与旧端一致**。其中 15 条归一化后逐字零差异，
22 条只在下表「差异归类」列出的那几类上不同，逐条核过，**没有一类会改变线上响应**。

| # | 方法 · 路径 | 旧端出处 | 新端出处 | 结论 | 前端消费 |
| --- | --- | --- | --- | --- | --- |
| 1 | `POST /api/auth/token` | jwt_auth.rs:668-805 | auth.rs:653-765 | 一致（A） | 是 · reviewApi.ts:2360 |
| 2 | `POST /api/auth/verify` | jwt_auth.rs:809-873 | auth.rs:768-832 | 一致 | 是 · reviewApi.ts:2421 |
| 3 | `GET /api/users` | review_api.rs:4958-4977 | users.rs:164-183 | 一致（V） | 是 · reviewApi.ts:1786 |
| 4 | `GET /api/users/me` | review_api.rs:4980-5007 | users.rs:186-213 | 一致（V） | 是 · reviewApi.ts:1795 |
| 5 | `GET /api/users/reviewers` | review_api.rs:5010-5031 | users.rs:216-237 | 一致（V） | 是 · reviewApi.ts:1803 |
| 6 | `POST /api/review/tasks` | review_api.rs:1819-2204 | tasks.rs:430-815 | 一致 | 是 · reviewApi.ts:1013 |
| 7 | `GET /api/review/tasks` | review_api.rs:2207-2339 | tasks.rs:818-950 | 一致（S） | 是 · reviewApi.ts:1047 |
| 8 | `GET /api/review/tasks/{id}` | review_api.rs:2342-2399 | tasks.rs:953-1010 | 一致 | 是 · reviewApi.ts:1065 |
| 9 | `PATCH /api/review/tasks/{id}` | review_api.rs:2402-2556 | tasks.rs:1013-1167 | 一致 | 是 · reviewApi.ts:1083 |
| 10 | `DELETE /api/review/tasks/{id}` | review_api.rs:2559-2651 | tasks.rs:1170-1262 | 一致 | 是 · reviewApi.ts:1094 |
| 11 | `POST /api/review/tasks/{id}/start-review` | review_api.rs:2658-2679 | tasks.rs:1269-1290 | 一致 | 是 · reviewApi.ts:1107 |
| 12 | `POST /api/review/tasks/{id}/approve` | review_api.rs:2682-2697 | tasks.rs:1293-1308 | 一致 | 是 · reviewApi.ts:1121 |
| 13 | `POST /api/review/tasks/{id}/reject` | review_api.rs:2700-2719 | tasks.rs:1311-1330 | 一致 | 是 · reviewApi.ts:1138 |
| 14 | `POST /api/review/tasks/{id}/cancel` | review_api.rs:2722-2742 | tasks.rs:1333-1353 | 一致 | 是 · reviewApi.ts:1155 |
| 15 | `GET /api/review/tasks/{id}/history` | review_api.rs:2888-2982 | tasks.rs:1499-1593 | 一致（S+K） | 是 · reviewApi.ts:1472 |
| 16 | `POST /api/review/tasks/{id}/submit` | review_api.rs:5059-5325 | tasks.rs:1798-2064 | 一致 | 是 · reviewApi.ts:1174 |
| 17 | `POST /api/review/tasks/{id}/return` | review_api.rs:5328-5546 | tasks.rs:2067-2285 | 一致 | 是 · reviewApi.ts:1192 |
| 18 | `GET /api/review/tasks/{id}/workflow` | review_api.rs:5561-5687 | tasks.rs:2300-2426 | 一致（S） | 是 · reviewApi.ts:1208 |
| 19 | `GET /api/review/tasks/returned` | review_api.rs:4785-4837 | tasks.rs:1600-1652 | 一致 | 否 |
| 20 | `POST /api/review/tasks/batch-reactivate` | review_api.rs:4847-4955 | tasks.rs:1662-1770 | 一致 | 否 |
| 21 | `POST /api/review/records` | review_api.rs:3087-3586 | records.rs:307-806 | 一致（V） | 是 · reviewApi.ts:1415 |
| 22 | `GET /api/review/records/by-task/{task_id}` | review_api.rs:3589-3646 | records.rs:809-866 | 一致（V） | 是 · reviewApi.ts:1437 |
| 23 | `DELETE /api/review/records/item/{record_id}` | review_api.rs:3649-3693 | records.rs:885-929 | 一致（V+Q） | 是 · reviewApi.ts:1448 |
| 24 | `DELETE /api/review/records/clear-task/{task_id}` | review_api.rs:3696-3743 | records.rs:932-976 | 一致（V+Q） | 是 · reviewApi.ts:1459 |
| 25 | `POST /api/review/comments` | review_api.rs:3804-3914 | comments.rs:119-229 | 一致（V） | 是 · reviewApi.ts:1503 |
| 26 | `GET /api/review/comments/by-annotation/{annotation_id}` | review_api.rs:3917-4025 | comments.rs:232-340 | 一致（V+S+K） | 是 · reviewApi.ts:1544 |
| 27 | `DELETE /api/review/comments/item/{comment_id}` | review_api.rs:4102-4215 | comments.rs:417-530 | 一致（V+S） | 是 · reviewApi.ts:1563 |
| 28 | `PATCH /api/review/comments/item/{comment_id}` | review_api.rs:4028-4093 | comments.rs:343-408 | 一致（V） | 是 · reviewApi.ts:1518 |
| 29 | `PATCH /api/review/annotations/{annotation_id}` | review_api.rs:4399-4507 | comments.rs:714-822 | 一致（V） | 是 · reviewApi.ts:1637 |
| 30 | `PATCH /api/review/annotations/{annotation_id}/severity` | review_api.rs:4512-4657 | comments.rs:827-972 | 一致（V） | 是 · reviewApi.ts:1600 |
| 31 | `POST /api/review/annotation-states/apply` | review_annotation_state.rs:166-451 | annotation_state.rs:157-442 | 一致 | 是 · reviewApi.ts:1752 |
| 32 | `GET /api/review/annotation-states` | review_annotation_state.rs:453-532 | annotation_state.rs:444-523 | 一致 | 是 · reviewApi.ts:1768 |
| 33 | `POST /api/review/attachments` | review_api.rs:5798-6263 | attachments.rs:165-634 | 一致（D） | 是 · reviewApi.ts:1955 |
| 34 | `DELETE /api/review/attachments/{attachment_id}` | review_api.rs:6270-6512 | attachments.rs:641-881 | 一致（D+S） | 是 · reviewApi.ts:2087 |
| 35 | `POST /api/review/sync/export` | review_api.rs:6541-6693 | sync.rs:71-223 | 一致（S+K） | 是 · reviewApi.ts:2502 |
| 36 | `POST /api/review/sync/import` | review_api.rs:6715-6869 | sync.rs:253-406 | 一致（Q） | 是 · reviewApi.ts:2515 |
| 37 | `GET /api/review/health` | review_api.rs:1758-1812 | mod.rs:88-127 | 一致（H） | 否 |

行号是 handler 的起止行。旧端行号系于 `docs/evidence/2026-09-13-plant-model-gen-baseline-sha256.md`
里那份 sha256 基线（移植期间旧端源零漂移）。

---

## 3. 七类差异逐条交代

### V —— `pub(super)` 可见性（11 条）

旧端全在一个文件里，handler 都是私有 `async fn`；拆成八个子模块后 `routes()` 与 handler
不在同一模块，只能 `pub(super)`。不过线。

### S —— 少一个 `SurrealValue` derive（7 条）

旧端行内的反序列化目标（`TaskCountRow` / `HistoryRow` / `CommentRow` / `WorkflowRow` …）
是 `#[derive(Debug, Deserialize, SurrealValue)]`；`SurrealValue` 是 3.x fork 的原生取值 derive，
2.1.4 fork 没有这个宏，本仓走 serde 桥，所以只剩 `#[derive(Debug, Deserialize)]`。
这些类型是**读库用的行映射，不是响应体**（响应体见 §6）。

### K —— `r.id.key` → `r.id.id`（3 条：#15 / #26 / #35）

**这一类值得单独说，因为它真的进响应体。** 三处把行主键写成 `format!("{:?}", …)`：
`get_task_history` 的 `HistoryItem.id`、`get_comments_by_annotation` 的 `AnnotationComment.id`、
`sync export` 的同名字段。前端拿到评论 id 之后会回头调
`DELETE|PATCH /api/review/comments/item/{comment_id}`——形制变了就是整条链断。

两端的差异只是 SDK 的**字段名**：
- 旧端 `RecordId.key`，类型 `RecordIdKey`，在 3.1 fork `crates/types/src/value/record_id/key.rs:18`
  起 `#[derive(Clone, Debug, …)]`，变体 `Number(i64)` / `String(String)`；
- 新端 `Thing.id`，类型 `sql::Id`，在 2.1.4 fork `crates/core/src/sql/id/mod.rs:31`
  起同样 `#[derive(Clone, Debug, …)]`，变体也是 `Number(i64)` / `String(String)`。

两个都是 derive 的 `Debug`、变体名相同，所以 `format!("{:?}")` 的输出逐字相同
（字符串主键 → `String("cmt-1")`，数字主键 → `Number(7)`）。
**本轮补了断言** `common::tests::record_id_debug_shape_matches_the_legacy_sdk` 把新端这一侧钉住：
哪天 SDK 给 `Id` 加自定义 `Debug`，这条会红，而不是等前端删评论 404 才发现。

### Q —— 三条 SurrealQL 提取成具名常量（#23 / #24 / #36）

| 处 | 旧端 | 新端 |
| --- | --- | --- |
| #23 `delete_record` | `review_api.rs:3652` `DELETE [type::record('review_records', $id)]` | `records.rs:876` `DELETE_RECORD_SQL` = `DELETE type::thing('review_records', $id)` |
| #24 `clear_records_by_task` | `review_api.rs:3699-3702` 行内 `r#"…"#` | `records.rs:879` `CLEAR_TASK_RECORDS_SQL`，**语句正文逐字相同** |
| #36 `import_review_data` | `review_api.rs:6739` `SELECT id FROM review_tasks WHERE record::id(id) = $id` | `sync.rs:249` `IMPORT_TASK_EXISTS_SQL` = `SELECT record::id(id) AS id FROM …`（同一 WHERE） |

#23 除了 §1 的构造子修正，还去掉了外层的数组字面量（`DELETE [x]` → `DELETE x`）——不是风格调整：
本仓 2.1.4 fork 的解析器对数组目标报 ``Unexpected token `[` ``，整条 500。
`records.rs:868-875` 有注释交代，`tests::delete_record_sql_removes_exactly_that_row`
用嵌入式 `mem://` 实跑钉住「只删那一行、另一行还在」。

#36 改了投影（`id` → `record::id(id) AS id`）：两端消费面都是
`let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default(); !rows.is_empty()`，
只看空不空，投影不进响应；`sync.rs:436` 起有命中/不命中两个方向的实跑断言。

### D —— 附件落盘目录走配置（#33 / #34）

旧端硬编码 `assets/review_attachments`；新端取 `crate::options::review_attachments_dir()`，
即 `asset_root()` + `[review_attachments].dir`，缺省段名就是 `review_attachments`
（常量 `options.rs:323`，取值 `options.rs:369-370`）。这是 R1 的持久化落点改造，不是 T2 的改动。

**响应里那条 URL 两端逐字相同**：旧 `review_api.rs:6146` 与新 `attachments.rs:517`
都是 `format!("/files/review_attachments/{}", stored_name)`。变的只是文件写在磁盘哪儿。

### A —— `/api/auth/token` 的 `workflow_mode` 校验（#1）

旧端 `jwt_auth.rs:729` / `:754` 是一对 `#[cfg(feature = "review-internal-workflow")]` /
`#[cfg(not(...))]` 分支：开着特性时非法 `workflow_mode` 回 400
`"Invalid workflow_mode: valid values are external, manual, internal"`（文案在 `jwt_auth.rs:745`），
关着时直接忽略。
新端按计划 Q7 / N6 只保留了默认分支（`auth.rs:9-13` 的模块注释交代过）。

**这不构成两端差异**：旧端 `Cargo.toml:186` 的 `default = ["review"]` 不含
`review-internal-workflow`（它在 `Cargo.toml:219` 单独定义、无人引用），
所以**旧端 :3100 实际出厂的二进制走的也是 `not(feature)` 那一支**，同样只忽略不报 400。
两端行为相同，差的只是新端把那段死代码没搬过来。

### H —— health 拆成三个小函数（#37）

旧端 `review_health` 是一整段；新端 `mod.rs:88` 的 `health()` 把探活与 JSON 组装拆成
`probe_review_store()` / `health_ok_payload()` / `health_error_payload()`（`mod.rs:141/156/164`）。
**键集与状态码逐字保留**：200 `{status:"ok", database:"healthy", elapsedMs}`；
504 `{status:"error", database:"unreachable"|"unhealthy", operation, elapsedMs, message}`，
两句错误文案 `校审数据库连接失败: {}` / `校审数据库查询失败: {}` 也一字不差。
`mod.rs:183` 起有键集断言。**前端不消费这条**（`plant3d-web/src` 全仓无 `/api/review/health`），
它是 R6 实跑验收和走查 preflight 在用。

---

## 4. HTTP 状态码：37 / 37 逐条相同

除了 §3 的函数体逐行 diff（任何状态码差异都会落进去），另外单独按 handler 抽了一遍
`StatusCode::*` 的集合做交叉验证，**37 条的集合两端完全相等**。覆盖到的档位包括：

| 档位 | 出处 |
| --- | --- |
| 400 参数非法 | `tasks.rs` 流转节点校验、`comments.rs` severity 取值 |
| 401 / 403 | `auth.rs` 四句文本逐字断言旧端 `jwt_auth.rs:513,517,647,657`；任务写授权 403 由 `common.rs::format_owner_mismatch_error` 钉住 |
| 404 | `/tasks/{id}` 一族「任务不存在或已删除: {}」 |
| 409 | `records.rs` dimensionDocument 版本 CAS（两端各 7 处） |
| 413 / 415 | `attachments.rs` 大小上限（各 3 处）、扩展名白名单与文件头校验（各 2 处） |
| 500 / 504 | 查询失败与连库超时 |

任务书点名要对的 **422 两端都不存在**（`StatusCode::UNPROCESSABLE_ENTITY` 在两端均为 0 处）：
附件那条走的是 413 / 415，不是 422。R6 探针里看到的 415 / 422 是 axum 的 body 提取器在
handler 之前拒绝的，不来自校审代码。

> R6 实跑（`docs/evidence/2026-09-13-review-port-acceptance.md` §A3）里「失败 11 / 总计 38」
> 不是路由缺失：`test_review_api.sh` 把任何 404 都当未注册，而 `/tasks/{id}` 一族对不存在的
> probe-id 回 404 本来就是契约。11 条全落在「路由在」一侧。

---

## 5. 查询参数与分页 / 排序默认值

**查询参数结构体两端逐字相同**（含 `#[serde(alias = …)]`）：

| 结构体 | 旧端 | 新端 |
| --- | --- | --- |
| `TaskListQuery`（9 个可选参数 + `formId` alias） | review_api.rs:579 | common.rs:225 |
| `UserListQuery` | review_api.rs:4693 | users.rs:75 |
| `CommentQuery`（`type` / `form_id` / `task_id` 三个可选） | review_api.rs:3797 | comments.rs:112 |
| `QueryAnnotationStatesRequest`（`formId` / `taskId` alias） | review_annotation_state.rs:72 | annotation_state.rs:80 |
| `AnnotationSeverityTypeQuery` | review_api.rs:4222 | comments.rs:537 |

默认值与排序此前只是目视一致，**本轮做成了断言**
`common::tests::review_pagination_and_sort_defaults_are_pinned_verbatim`：
13 条字面量逐条钉住，每条在源码里附旧端行号。

| 面 | 值 | 旧端 → 新端 |
| --- | --- | --- |
| 列表分页缺省 | `limit=100` / `offset=0` | review_api.rs:2251-2252 → tasks.rs:862-863 |
| 列表页内排序 | `ORDER BY created_at DESC LIMIT {} START {}` | review_api.rs:1123 → common.rs:776 |
| 审批历史 / 工作流 | `timestamp DESC` / `timestamp ASC`（方向相反，两条都被前端渲染） | review_api.rs:2923,5644 → tasks.rs:1534,2383 |
| 驳回重流转列表 | `updated_at DESC` | review_api.rs:4794 → tasks.rs:1609 |
| 确认记录 / 评论 | `confirmed_at DESC` / `created_at ASC` | review_api.rs:3592,3948,4333 → records.rs:812, comments.rs:263,648 |
| 批注处理状态三分支 | `updated_at DESC` ×3 | review_annotation_state.rs:472,477,483 → annotation_state.rs:463,468,474 |
| 导出硬上限 | 任务 `LIMIT 100`、评论与记录 `LIMIT 10000` | review_api.rs:6547,6558,6623,6658 → sync.rs:77,88,153,188 |

**为什么值得单独立一道护栏**：这些值不写在响应结构体里，§6 那轮结构体逐字对比覆盖不到。
改掉一个 `ORDER BY` 方向或一个 `unwrap_or`，响应键集一字不变，前端却会拿到顺序相反的列表
或凭空多出来的一页——正是计划 §7 说的静默失效。

**变异验证（两轮，都真红）**：
1. 把 `comments.rs:263` 的 `created_at ASC` 改成 `DESC` → 护栏红并点名
   「comments.rs: 期望 1 处、实际 0 处 —— 旧端出处 review_api.rs:3948」；
2. 只把 `common.rs:776` handler 里那一处 `created_at DESC` 改成 `ASC`、**保留 `PINS` 表里的那份** →
   护栏仍然红（「旧端出处 review_api.rs:1123」）。第 2 轮专门证明「剥掉本表再扫」的逻辑生效，
   护栏不会因为自己表里有同一串而假绿。
   两轮都已还原，`git status --porcelain -- src` 干净。

---

## 6. 线上形状：21 个响应结构体逐字相同

脚本把两端同名结构体的**字段名、类型、serde 属性**逐行抽出来对比：

| 范围 | 结果 |
| --- | --- |
| 前端真正消费的响应结构体（下列 21 个） | **21 / 21 逐字相同** |
| 两端全部同名结构体（含请求体、行映射） | 76 个中 **68 个逐字相同**，8 个有差异且全部是内部行类型 |

逐字相同的 21 个：`TokenResponse`、`VerifyResponse`、`UserListResponse`、`UserResponse`、
`TaskListResponse`、`TaskResponse`、`ActionResponse`、`ReviewTask`、`ReviewComponent`、
`ReviewAttachment`、`ConfirmedRecordResponse`、`CommentResponse`、`AnnotationSeverityResponse`、
`AnnotationBasicFieldsResponse`、`ApplyAnnotationStateResponse`、`AnnotationStateView`、
`QueryAnnotationStatesResponse`、`AttachmentUploadResponse`、`ExportResponse`、`ImportResponse`、
`WorkflowHistoryResponse`。

「逐字相同」包含 `#[serde(rename_all = "camelCase")]` 这类属性在内，所以**信封键集、字段名、
大小写风格三件事一起被这一条覆盖了**。

那 8 个差异全是 §3 的 **S** 类：`TaskRow` / `ReviewRecordRow` / `CommentRow` / `HistoryRow` /
`WorkflowRow` / `AttachmentRow` / `AnnotationStateRow` / `TaskRecordContext`，
旧端 `types::RecordId` / `types::Datetime`（3.x fork）↔ 新端 `sql::Thing` / `sql::Datetime`（2.1.4 fork）。
它们是**反序列化目标**，不是响应体：把库里的行读进内存，再转成上面那 21 个响应结构体。

**日期的线上形制**由 `ReviewTask` 上的 `serialize_beijing_datetime_millis` /
`serialize_optional_beijing_datetime_millis` 决定（两端同一段代码），
`common.rs::TaskRow::to_review_task` 一族负责行 → 视图的转换，
`task_row_reads_every_due_date_shape_through_the_sdk_bridge` 用 `mem://` 钉住三种在库形态
（RFC 3339 字符串 / 原生 datetime / 缺省）都读成毫秒或 None，并断言对外仍是
北京时间字符串 `"2026-09-20 16:00:00"`。

---

## 7. 本轮覆盖到哪

**已机器验证**（不是肉眼）：

| 面 | 手段 | 结果 |
| --- | --- | --- |
| 37 条 handler 的响应契约 | 函数体逐行 diff（只归一 SDK 类型名） | 37/37 一致，15 条零差异、22 条差异全部归入 §3 七类 |
| 逐路由 HTTP 状态码 | 函数体 diff + `StatusCode::*` 集合交叉验证 | 37/37 相同 |
| 21 个响应结构体的键集 / 字段名 / serde 属性 | 结构体逐行 diff | 21/21 相同 |
| 查询参数结构体 | 逐行 diff | 5/5 相同 |
| 分页 / 排序默认值 | 新护栏 13 条字面量 + 两轮变异验证 | 相同，且已钉住 |
| 记录 id 的线上形制 | 新断言 + 两端 SDK derive 出处 | 相同，且已钉住 |
| 37 条方法·路径在册 | `REVIEW_CONTRACT_ROUTES` 源码文本护栏 + R6 运行时 38 探针 | 在册 |
| 401/403 四句文本 | `auth.rs` 逐字断言 | 相同 |
| 本域 SurrealQL 构造子 | §1 护栏 + 变异验证 | 已修并钉住 |

`cargo test --features http_api --lib web_service::review`
→ **150 passed; 0 failed; 0 ignored; 0 measured; 2144 filtered out** @ `065c7a57f`
（本文两条护栏未提交时的基线是 148 passed）。
四次真重编：首次 1m00s，两轮变异验证各 27.19s / 27.11s，最后一次 `touch src/lib.rs` 28.43s；
每次都出现 `Compiling aios-database`，不是陈旧二进制。

**没做、也不打算在本条里做的**：

1. **两端 curl 对拍**。机器上 :3100 跑的是 `plant-web-server` 那个旁支而不是旧端
   `plant-model-gen`（R6 已查实，见 acceptance 报告 §A2/A5/A10），拿它对拍会得出错误结论。
   本文全部结论走静态对照 + 新端实跑单测，这是任务书在旧端不可比时给的口径。
2. **#19 / #20 / #37 三条前端不消费**。`/tasks/returned`、`/tasks/batch-reactivate` 旧端
   `review_api.rs:1733` 起注册着、前端当前不调，移植不减项；`/api/review/health` 是运维探针。
   三条的函数体对照照样做了，都一致。
3. **浏览器侧的端到端验证**归 R8 / A4（走查脚本在 `scripts/review/walkthrough/`）。

**给 R8 回写 §11 的两句**（本条不改计划与规范文档）：

- 旧端 `review-internal-workflow` 特性不在 `default` 里，所以「不移植它」对出厂行为零影响，
  可以把计划 Q7 那条从「偏离」降级为「与出厂二进制一致」。
- 附件落盘目录改走 `[review_attachments].dir`（缺省与旧端同名同位），响应 URL 未变；
  这一条 R1 已落地，规范文档里若还写着硬编码 `assets/review_attachments` 需要更新。
