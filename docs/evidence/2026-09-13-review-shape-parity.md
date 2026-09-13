# 校审域响应形状对照复核（R5 / T2）

- 对应计划 `docs/plans/2026-09-12-review-backend-port-plan.md` §7 风险：「`review_api.rs` 单文件 7k 行，
  拆分时路径/形状走样 → 前端静默失效」。
- 新端：`src/web_service/review/{auth,users,tasks,records,comments,annotation_state,attachments,sync,mod}.rs`
- 旧端（只读）：`D:\work\plant-code\plant-model-gen\src\web_api\{review_api.rs, jwt_auth.rs, review_annotation_state.rs}`
- 契约参照（只读）：`D:\work\plant-code\old\plant3d-web\src\api\reviewApi.ts`
- 本轮 HEAD `3b01b64e8`；`cargo test --features http_api --lib web_service::review`
  → **148 passed; 0 failed; 0 ignored; 0 measured; 2144 filtered out**，编译 22.07s。

> 交接：本条原执行人 fable-5-1-31 中途掉线，他的在途成果（records / comments / attachments /
> sync / common 的对照与护栏测试）压在工作树里未提交，由本会话接手、复核后随
> commit `3b01b64e8` 一并入库。**本文的第 1 节就是他那条新测试照出来的。**

---

## 1. 抓到的缺陷：`type::record` 参数顺序倒置，四条前端主链路运行时 500

**这是本轮唯一的实质缺陷，已修并入库（`3b01b64e8`）。**

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
**变异验证**：把 `annotation_state.rs` 两处改回去 → 护栏真的红并逐行点名；还原后 148/0 全绿。

坑已记进共享记忆 `mem-271`，建议其他从 3.x fork 搬 SurrealQL 的域也 grep 一遍。

## 2. 线上形状：21 个响应结构体逐字相同

用脚本把两端同名结构体的**字段名、类型、serde 属性**逐行抽出来对比（不是肉眼看）：

| 范围 | 结果 |
| --- | --- |
| 前端真正消费的响应结构体（下表 21 个） | **21 / 21 逐字相同** |
| 两端全部同名结构体（含请求体、行映射） | 76 个中 **68 个逐字相同**，8 个有差异且全部是内部行类型（见 §3） |

逐字相同的 21 个：`TokenResponse`、`VerifyResponse`、`UserListResponse`、`UserResponse`、
`TaskListResponse`、`TaskResponse`、`ActionResponse`、`ReviewTask`、`ReviewComponent`、
`ReviewAttachment`、`ConfirmedRecordResponse`、`CommentResponse`、`AnnotationSeverityResponse`、
`AnnotationBasicFieldsResponse`、`ApplyAnnotationStateResponse`、`AnnotationStateView`、
`QueryAnnotationStatesResponse`、`AttachmentUploadResponse`、`ExportResponse`、`ImportResponse`、
`WorkflowHistoryResponse`。

「逐字相同」包含 `#[serde(rename_all = "camelCase")]` 这类属性在内，所以**信封键集、字段名、
大小写风格三件事一起被这一条覆盖了**——计划 §1.3 里最担心的「拆分时字段名走样」在这一层没有发生。

## 3. 八处差异：全是 SDK fork 差异，不过线

| 结构体 | 差异 |
| --- | --- |
| `TaskRow` / `ReviewRecordRow` / `CommentRow` / `HistoryRow` / `WorkflowRow` / `AttachmentRow` / `AnnotationStateRow` | 旧端 `surrealdb::types::RecordId` / `types::Datetime`（3.x fork）↔ 新端 `surrealdb::sql::Thing` / `sql::Datetime`（2.1.4 fork） |
| `TaskRecordContext` | 同上，字段名与顺序一致 |

这些是**反序列化目标**，不是响应体：它们把库里的行读进内存，再转成 §2 那 21 个响应结构体。
两个 fork 的 SDK 类型名不同是计划 §7 第一条风险本来就预期的，不构成契约差异。
真正要盯的是「行 → 视图」的转换是否把日期/id 转成同一种线上形制，这一层由
`common.rs::TaskRow::to_review_task` 一族承担，且已有 mem 实跑测试钉住三种 `due_date` 在库形态
（字符串 / 原生 datetime / 缺省）都能读成毫秒或 None。

## 4. 路由清单与鉴权文本

| 面 | 怎么钉住的 |
| --- | --- |
| 37 条方法·路径 | `common.rs::REVIEW_CONTRACT_ROUTES` 源码文本护栏，与八个子模块 `routes()` 双向集合比对；R3 做过两次变异验证 |
| 路由真在册（运行时） | R6 实跑：38 条探针全部非 404（`docs/evidence/2026-09-13-review-port-acceptance.md` §A3） |
| 401 / 403 四句文本 | `auth.rs` 护栏测试逐字对照旧端 `jwt_auth.rs:513,517,647,657` |
| 任务写授权的 403 文本 | `common.rs::format_owner_mismatch_error`，R3 已钉 |

## 5. 本轮覆盖到哪、还欠什么

**已机器验证**（不是肉眼）：

- 21 个响应结构体的键集 / 字段名 / serde 属性 —— 脚本逐行 diff，21/21 相同；
- 37 条方法·路径在册 —— 源码文本护栏 + R6 运行时 38 探针；
- 401/403 文本 —— 逐字断言；
- 本域 SurrealQL 的记录构造子 —— 新护栏 + 变异验证；
- 校审域 148 条单测全绿 @ `3b01b64e8`。

**还欠**（下一棒接着做，别当已完成）：

1. **逐路由的 HTTP 状态码对照**。本轮只覆盖了 401/403 与「不存在 → 404」这两类；
   `tasks.rs` 15 条里的 409（dimensionDocument 版本 CAS）、413（附件超限）、422 等还没和旧端逐条对。
   现有 10 条 `tasks.rs` 测试覆盖了一部分，但不是按「37 条逐条」组织的。
2. **查询参数与分页 / 排序默认值**。`GET /api/review/tasks` 的 `limit` / `offset` 默认、
   `ORDER BY created_at DESC`、`annotation-states` 的 `ORDER BY updated_at DESC` 已目视一致，
   但没有做成断言；`comments/by-annotation` 的 `type` / `form_id` / `task_id` 三个可选参数的
   缺省行为也还没对。
3. **37 行逐条表**（路由 · 旧端行号 · 新端行号 · 结论 · 前端是否消费）——本文用「按结构体 + 按面」
   组织，还没铺成任务书要求的那张 37 行表。铺表本身不难，难的是第 1、2 条的逐条判定要先做完。

**为什么先交这一版**：第 1 节那个缺陷是真的会让前端 500 的东西，值得马上入库并广播；
把它压到「37 行表铺完」之后才交，风险明显更大。上面三条欠账写清楚了，不算完成判据达标。
