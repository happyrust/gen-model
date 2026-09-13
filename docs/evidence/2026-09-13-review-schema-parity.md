# 校审域 schema / DDL 逐表对照复核（旧端 plant-model-gen → 新端 gen-model）

- 日期：2026-09-13
- 范围：计划 [`docs/plans/2026-09-12-review-backend-port-plan.md`](../plans/2026-09-12-review-backend-port-plan.md) §1.3「两份 surql 有重叠且不一致，需先定以哪份为真」；落点口径见 [`docs/adr/ADR-072-review-data-persistence.md`](../adr/ADR-072-review-data-persistence.md) D1–D6
- 性质：复核为只读；查出的 P1 经决策卡 q-228 拍板（选项 a）临时放开归属后**由本人代改并已提交**，见 §15
- 结论：12/12 张表有结论。**P1 一条（已修，commit `987faa2fc`）**、P2 七条、其余逐字一致

## 0. 先定「哪几份 DDL 在旧端真的跑过」

复核第一步是把「旧端运行时 DDL」这件事本身核清楚，因为它决定了后面每一格的基准。

| # | 出处 | 启动时是否真执行 | 覆盖内容 |
| --- | --- | --- | --- |
| A | `src/web_api/review_db.rs:190-207`（`ensure_review_query_indexes_inner`） | 是，进程级 `OnceCell` 懒加载 | 12 条查询索引 + `review_records` 补字段 |
| B | `src/web_api/review_db.rs:247-267`（`ensure_review_workflow_history_schema_inner`） | 是，进程级 `OnceCell` 懒加载 | `review_workflow_history` SCHEMAFULL 全量 + `review_history` 三个字段 |
| C | `src/web_api/platform_api/review_form.rs:37-50`（`ensure_review_forms_schema_inner`） | **是**，`src/web_server/mod.rs:568` 启动时调 `warm_review_forms_schema()` | `review_forms` SCHEMAFULL 13 字段 + `idx_form_id ... UNIQUE` |
| D | `rs_surreal/review/review_tables.surql` | 否，全仓无加载方 | 史料 |
| E | `resource/surreal/review_workflow.surql` | 否，全仓无加载方 | 史料 |

**任务书只列了 A 与 B，C 是本次复核补出来的第三处。** 新端 `rs_surreal/review/review_tables.surql` 的文件头也只声明「以 `review_db.rs` 的运行时 DDL 为真」，同样漏掉了 C —— 本报告唯一的 P1 就落在这里。

D 与 E 互相打架（同名表一个 SCHEMAFULL 一个 SCHEMALESS、字段集不同），新端判定「两份都不是旧端实际形态、只当史料」这一条经核**成立**：全仓 grep 未找到任何加载这两个文件的代码路径。

新端 DDL 只有一处：`rs_surreal/review/review_tables.surql`，由 `src/web_service/review/schema.rs:24`（`include_str!`）冻进二进制、`ensure_review_schema` 进程内跑一次。

## 1. review_tasks

- 旧端：无表定义（首次 `CREATE` 隐式建表，SCHEMALESS）；索引 `review_db.rs:192-198` 共 7 条
- 新端：`review_tables.surql:36` 显式 `DEFINE TABLE IF NOT EXISTS review_tasks SCHEMALESS;`；索引 `:49-55` 共 7 条
- 差异：① 表定义由隐式改显式（P2-2）；② 7 条索引名称、字段、非唯一性逐字一致
- 影响：无。SCHEMALESS 语义相同，显式定义只是让首启后 `INFO FOR DB` 清单完整
- 建议：保持

## 2. review_records

- 旧端：无表定义；索引 `review_db.rs:199-200` 共 2 条；补字段语句 `review_db.rs:201-203`
- 新端：`review_tables.surql:39` 表；`:57-58` 两条索引；`:65` 补字段
- 差异：补字段语句旧端夹在两条索引之间、新端单列成第 3 节，语义与幂等性一致（`WHERE dimension_document_version = NONE`，只补缺格的行）
- 影响：无
- 建议：保持。`schema.rs:324` 的 `the_record_backfill_only_touches_rows_missing_the_field` 已把「只补缺的、不改已有值」钉住

## 3. review_comments

- 旧端：无表定义；索引 `review_db.rs:204` 一条（`idx_review_comments_annotation_id`）
- 新端：`review_tables.surql:40` 表；`:59` 同名索引
- 差异：仅 P2-2
- 影响：无
- 建议：保持

## 4. review_annotation_severity

- 旧端：无表定义、无索引、无 surql 记载
- 新端：`review_tables.surql:42` 表，无索引
- 差异：仅 P2-2
- 影响：无。两端都靠全表扫，新端未额外加索引 = 不擅自改口径
- 建议：保持。若日后批注量上来，加索引应走独立变更，不在移植里夹带

## 5. review_annotation_states

- 旧端：无表定义、无索引、无 surql 记载
- 新端：`review_tables.surql:41` 表，无索引
- 差异：仅 P2-2
- 影响：无
- 建议：同上

## 6. review_attachment

- 旧端运行时：无表定义（隐式 SCHEMALESS）；索引 `review_db.rs:205` 一条 `idx_review_attachment_form_id`
- 旧端史料 D（`rs_surreal/review/review_tables.surql:31-39`）：SCHEMAFULL，7 个字段，索引名 `idx_attachment_form`
- 旧端史料 E（`resource/surreal/review_workflow.surql:29-41`）：SCHEMALESS，8 个字段（多 `created_at ... VALUE time::now()`），索引 `review_attachment_form_idx` + `review_attachment_file_idx(form_id, file_id)`
- 新端：`review_tables.surql:43` SCHEMALESS；索引 `:59` 取运行时那一条
- 差异（P2-3）：新端与史料 D 的 SCHEMAFULL 相左。**这一条判得对**：上传 handler（新 `attachments.rs:516-529`）每次写 `file_name` / `file_size` / `mime_type` 三格，而史料 D 的 SCHEMAFULL 版本没有这三个字段定义，照它建表附件一条也存不进去
- 影响：无（与旧端运行时一致）
- 建议：保持。`schema.rs:249` 的 `attachment_rows_keep_the_fields_the_upload_handler_writes` 是「谁哪天改回 SCHEMAFULL」的绊线，留着

## 7. review_forms ⚠ **P1**

- 旧端运行时（出处 C，`platform_api/review_form.rs:37-50`，启动时经 `web_server/mod.rs:568` 真执行）：

  ```
  DEFINE TABLE IF NOT EXISTS review_forms SCHEMAFULL;
  DEFINE FIELD IF NOT EXISTS form_id       ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS project_id    ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS user_id       ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS role          ON TABLE review_forms TYPE none | string;
  DEFINE FIELD IF NOT EXISTS requester_id  ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS source        ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS status        ON TABLE review_forms TYPE string;
  DEFINE FIELD IF NOT EXISTS task_created  ON TABLE review_forms TYPE bool DEFAULT false;
  DEFINE FIELD IF NOT EXISTS deleted       ON TABLE review_forms TYPE bool DEFAULT false;
  DEFINE FIELD IF NOT EXISTS created_at    ON TABLE review_forms TYPE datetime;
  DEFINE FIELD IF NOT EXISTS updated_at    ON TABLE review_forms TYPE datetime DEFAULT time::now();
  DEFINE FIELD IF NOT EXISTS deleted_at    ON TABLE review_forms TYPE option<datetime>;
  DEFINE INDEX IF NOT EXISTS idx_form_id   ON TABLE review_forms FIELDS form_id UNIQUE;
  ```

- 新端：`review_tables.surql:37` 只有一行 `DEFINE TABLE IF NOT EXISTS review_forms SCHEMALESS;`。**13 个 `DEFINE FIELD` 与唯一那条索引一个都没有**
- 差异：
  - **P1-1：`idx_form_id ... UNIQUE` 丢失。** 旧端由数据库保证「一个 `form_id` 只有一行主单据」，新端无任何约束
  - P2-1：SCHEMAFULL → SCHEMALESS，13 个字段定义（含 `task_created` / `deleted` 两个 `DEFAULT false`、`updated_at DEFAULT time::now()`）全部消失
- 影响：
  - P1-1 有两层。**一是并发写**：`ensure_review_form_stub`（新 `platform/review_form.rs:88-181`）是「先 SELECT 再 CREATE」的读改写，中间没有事务也没有唯一约束兜底；PMS 的 `embed-url` 与 `workflow/sync` 对同一 `form_id` 并发进来时，旧端第二条 `CREATE` 会被唯一索引挡下并报错，新端会**静默多出一行**。此后 `get_review_form_by_form_id`（`:66-71`，`ORDER BY updated_at DESC, created_at DESC LIMIT 1`）只会取到其中一行，另一行成为读不到但仍在库里的影子记录，主单据的 `status` / `role` / `task_created` 会随哪一行被更新而漂。**二是查询代价**：这是 `review_forms` 上唯一的索引，丢了之后每次按 `form_id` 找主单据都是全表扫
  - P2-1 经逐写入路径核对**不构成字段静默丢失**：`CREATE`（`platform/review_form.rs:148-161`）把 12 个字段全部显式写出，三处 `UPDATE`（`:111-120`、`:207-218`、`:241` 起）也都显式写 `deleted` / `updated_at`，不依赖 DB 侧 `DEFAULT`。丢的是类型约束（例如 `status` 不再被强制为 string），不是数据
- 建议（已全部落地，见 §15）：
  1. 在 `rs_surreal/review/review_tables.surql` 补 `DEFINE INDEX IF NOT EXISTS idx_form_id ON TABLE review_forms FIELDS form_id UNIQUE;`，并在 `schema.rs` 的索引护栏表里加上 `review_forms → idx_form_id`（原护栏 `every_query_index_from_the_legacy_runtime_ddl_is_created` 只照着 `review_db.rs` 那 12 条抄，**结构上不可能发现这一条**，因为它来自出处 C）
  2. 顺带在 `review_tables.surql` 文件头的「以哪份为真」里把出处 C 补进去，否则下一个人还会照着「只有 review_db.rs 两段」的口径复核
  3. 是否同时补回 13 个 `DEFINE FIELD`（即回到 SCHEMAFULL）建议**不做**：新端其余 9 张表一律 SCHEMALESS，单独把这张改回 SCHEMAFULL 会让「旧库整体迁入」这个 ADR-072 未决项更难办；类型约束的收益也不抵风险

## 8. review_form_model

- 旧端运行时：无表定义；索引 `review_db.rs:206` 一条 `idx_review_form_model_form_id`
- 旧端史料 D（`review_tables.surql:14-17`）：SCHEMAFULL，2 字段，索引 `idx_form_model`
- 旧端史料 E（`review_workflow.surql:5-12`）：SCHEMALESS，3 字段（多 `created_at ... VALUE time::now()`），索引 `review_form_model_form_idx` + **`review_form_model_unique_idx(form_id, model_refno) UNIQUE`**
- 新端：`review_tables.surql:38` SCHEMALESS；索引 `:60` 取运行时那一条
- 差异（P2-5）：史料 E 想要的 `(form_id, model_refno)` 唯一索引两端都没有
- 影响：与旧端**一致**，不是移植引入的。写入方（新 `tasks.rs:720-724`、`sync.rs:364`；旧 `review_api.rs:2109`、`:6834`，两端同为 `CREATE ONLY ... SET form_id, model_refno, created_at`）只在单次请求内用 `HashSet` 去重（`tasks.rs:710`），跨请求重复调用会累积重复行；下游 `workflow_sync.rs:2207` 的 `SELECT VALUE model_refno` 会把重复项一并返回
- 建议：不在本次移植里改（改了就不是逐字兼容）。作为已知的继承问题记到 ADR-072「未决」，需要时单独立项

## 9. review_opinion

- 旧端史料 D（`review_tables.surql:20-28`）SCHEMAFULL 7 字段 + 索引；史料 E（`review_workflow.surql:15-26`）SCHEMALESS 6 字段 + 2 索引
- 旧端运行时：**无定义**
- 新端：**不建**，且 `schema.rs:133` 有反向护栏钉死它不许出现
- 差异（P2-4）：新端不建这张表
- 影响：无。独立复核过：`plant-model-gen/src` 全仓 grep `review_opinion` **零命中**，旧端从来没有任何代码读或写这张表
- 建议：保持

## 10. review_form（单数）

- 旧端史料 D（`review_tables.surql:6-11`）：SCHEMAFULL 4 字段 + `idx_form_id ... UNIQUE`
- 旧端运行时：**无定义**
- 新端：**不建**，`schema.rs:133` 反向护栏
- 差异（P2-4）：同上
- 影响：无。独立复核过：旧端所有 `review_form` 字样命中的都是 Rust 模块名（`platform_api/review_form.rs`、`super::review_form::…`），没有一处是表名；真正在用的是复数的 `review_forms`
- 建议：保持。注意史料 D 里这张表上的 `idx_form_id UNIQUE` 与 §7 的同名索引**不是一回事**（一个在 `review_form`，一个在 `review_forms`），对照时容易看串

## 11. review_workflow_history

- 旧端：`review_db.rs:247-263` —— `DEFINE TABLE OVERWRITE ... SCHEMAFULL` + 14 个 `DEFINE FIELD OVERWRITE` + 2 个 `DEFINE INDEX OVERWRITE`
- 新端：`review_tables.surql:76-92`
- 差异：**逐字一致**，包括 `OVERWRITE` 写法、字段顺序、三个旧字段（`operator_id` / `operator_name` / `timestamp`）的 `option<...>`、`timestamp` 的 `DEFAULT time::now()`、两条索引名
- 写入面复核（这是全域唯一 SCHEMAFULL 表，字段没定义就会被静默丢弃，所以逐站点核过）：

  | 写入点 | 写的字段 | 是否全部在定义内 |
  | --- | --- | --- |
  | `platform/workflow_sync.rs:342-357` | task_id, form_id, node, target_node, action, operator_id, operator_name, actor_id, actor_role, actor_name, source, comment, timestamp, created_at（14/14） | 是 |
  | `tasks.rs:210-225` | 同上 14 个（`source` 为字面量 `'plant3d-internal'`） | 是 |
  | `tasks.rs:554-566` | 上述去掉 `operator_id` / `operator_name` / `timestamp`（11 个） | 是；`timestamp` 由 `DEFAULT time::now()` 补 |

  全仓无 `UPDATE` / `MERGE review_workflow_history`，不存在「改写时带新字段」的第二条路径
- 影响：无
- 建议：保持。`schema.rs:187` / `:230` 两条测试把 RUS-244 新形态与旧 `operator_*` 形态都钉住了，是这张表最值钱的两条

## 12. review_history

- 旧端：`review_db.rs:265-267` —— 无 `DEFINE TABLE`（保持隐式 SCHEMALESS，注释明写「不强制 SCHEMAFULL 避免破坏老数据」），只补 `form_id` / `source` / `created_at` 三个 `option` 字段
- 新端：`review_tables.surql:44` 多了一行显式 `DEFINE TABLE IF NOT EXISTS review_history SCHEMALESS;`；`:94-96` 三个字段与旧端逐字一致
- 差异（P2-2）：仅表定义由隐式改显式
- 影响：无。写入点只有 `tasks.rs:1435-1445` 一处，9 个字段（task_id, form_id, action, user_id, user_name, source, comment, timestamp, created_at），SCHEMALESS 全收
- 建议：保持

## 13. collision_events（只读）

- 旧端：无 DDL（表由碰撞检测侧生产）；读取点 `web_api/collision_api.rs:80`、`web_api/review_integration.rs:180,344,349`
- 新端：无 DDL，**也不读**。`integration.rs:168` 的 `COLLISION_SOURCE_UNAVAILABLE_REASON` 直接回显式空集 + 原因，`:208` 恒为 `CollisionSource::Unavailable`
- 差异（P2-7）：新端不查表
- 影响：与计划 Q4「无碰撞数据源时显式空集 + reason，不回落 mock」一致。gen-model 的数据面（e3d-direct / RocksDB）本来就没有同形表，加 DDL 只会建出一张永远空的表，把「没有数据源」伪装成「查到零条」
- 建议：保持。接入方案记在 ADR-072「未决 · 碰撞数据源待接」

## 14. 差异汇总

| 级别 | 编号 | 一句话 | 旧端出处 | 新端出处 |
| --- | --- | --- | --- | --- |
| **P1（已修）** | P1-1 | `review_forms` 丢了 `idx_form_id ... UNIQUE`（也是该表唯一的索引） | `platform_api/review_form.rs:50`（经 `web_server/mod.rs:568` 启动时执行） | 修前 `review_tables.surql:37` 只有表、无索引；修后见 commit `987faa2fc` |
| P2 | P2-1 | `review_forms` SCHEMAFULL→SCHEMALESS，13 个字段定义消失；经核所有写入路径显式写全，不丢数据 | `platform_api/review_form.rs:37-49` | `review_tables.surql:37` |
| P2 | P2-2 | 9 张表由隐式建表改显式 `DEFINE TABLE ... SCHEMALESS` | 无（隐式） | `review_tables.surql:36-44` |
| P2 | P2-3 | `review_attachment` 不采史料 D 的 SCHEMAFULL（采了附件写不进去） | `rs_surreal/review/review_tables.surql:31-39` | `review_tables.surql:43` |
| P2 | P2-4 | 不建 `review_form` / `review_opinion`（旧端零读写，已独立复核） | `rs_surreal/review/review_tables.surql:6-11,20-28` | `schema.rs:133` 反向护栏 |
| P2 | P2-5 | `review_form_model` 无 `(form_id, model_refno)` 唯一索引 —— 两端一致，继承问题 | `resource/surreal/review_workflow.surql:12` | `review_tables.surql:38,60` |
| P2 | P2-6 | 史料 E 的 `created_at ... VALUE time::now()` 等字段定义两端都未生效 | `resource/surreal/review_workflow.surql:9,23,38` | 无 |
| P2 | P2-7 | `collision_events` 新端不读、不建，回显式空集 | `web_api/review_integration.rs:180` | `review/integration.rs:168,208` |

索引总数对账：旧端运行时 15 条（`review_db.rs` 12 + workflow_history 2 + `review_form.rs` 1），新端修前 14 条 —— 差的正是 P1-1 那一条；修后 15 条，对齐。

## 15. P1 处置

1. **上报作者**：P1-1 已 `send_to` opus-5-19（`schema.rs` / `review_tables.surql` 作者，se-2f8ca6d9b4637de4），内容为本报告 §7 全文要点 + 三条建议。回执：**对方离线**，消息进宿主队列（`dm-mtzg9cq9-ewmawq`），上线后自动投递。截至本报告定稿**仍无作者答复**。
2. **开卡拍板**：因作者离线、而任务书禁止我自行改 `schema.rs` / `.surql`，开决策卡 `q-228`（severity=high）。指挥官 fable-5-1-30 选 **a：临时放开这两个文件的归属，由我代改这一处**。理由原文：「P1（并发下静默多行 = 宪法 III 静默失效），修复三行，作者离线，等不起。」
3. **已修**：commit `987faa2fc`（`fix(review): 补回 review_forms 的 idx_form_id UNIQUE，并把第三处旧端运行时 DDL 记进出处`），两文件 86 增 6 删，改前 `acquire_file_locks`、改后 `release`：
   - `rs_surreal/review/review_tables.surql`：补 `DEFINE INDEX IF NOT EXISTS idx_form_id ON TABLE review_forms FIELDS form_id UNIQUE;`（与出处 C 同名同字段）；文件头把三处运行时 DDL 列全并点名「C 容易被漏」；「有意偏离」由三条增为四条，新增第 4 条记 `review_forms` 保持 SCHEMALESS
   - `src/web_service/review/schema.rs`：索引护栏表加 `("review_forms", vec!["idx_form_id"])`；新增 `review_forms_rejects_a_second_row_with_the_same_form_id`，钉住**唯一性本身**（只钉索引名钉不住 `UNIQUE`）
4. **变异验证**（指挥官要求的「删掉那行索引要真的 FAILED」）：把那行索引注释掉后重编重跑，两条护栏都真的红 ——

   ```
   test …schema::tests::review_forms_rejects_a_second_row_with_the_same_form_id ... FAILED
   test …schema::tests::every_query_index_from_the_legacy_runtime_ddl_is_created ... FAILED
   test result: FAILED. 6 passed; 2 failed; 0 ignored; 0 measured; 2277 filtered out; finished in 0.54s
   ```

5. **还原后验证**（口径按指挥官 14:3x 通告：尾行 + 编译耗时 + HEAD）：

   ```
   cargo test --features http_api --lib web_service::review::schema
     Finished `test` profile … in 32.98s
     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 2277 filtered out; finished in 0.54s
   cargo test --features http_api --lib web_service::review
     Finished `test` profile … in 53.57s
     test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 2144 filtered out; finished in 0.62s
   ```

   均 @ `987faa2fc` 的父提交树 + 本次改动（提交后 HEAD = `987faa2fc`）。两组总数对账：8+2277 = 2285 = 141+2144，同一个二进制；比修前的 2284 多的正是新增那条唯一性护栏。

6. **P2-1（SCHEMAFULL→SCHEMALESS）按建议不改回**，指挥官同批拍板确认。给 R8 的可粘文字见 §17。

## 16. 给 R5 / 后续的知会

commit `987faa2fc` 改了 `rs_surreal/review/review_tables.surql` 与 `src/web_service/review/schema.rs`，HEAD 变了；`review_forms` 从此有唯一约束，**任何对同一 `form_id` 写第二行主单据的路径会开始报错而不再静默通过**——这正是旧端的行为，但若有测试或脚本依赖「能重复建同一 form_id 的主单据」，会在这一版开始红。已 `send_to` fable-5-1-31（R5）。

## 17. 给 R8 的可粘文字

> 以下两段可直接贴进 `docs/adr/ADR-072-review-data-persistence.md`「偏离 / 未决」与计划 `§11`，不必改写。

**贴 ADR-072「偏离」：**

> **D7（2026-09-13 补）· `review_forms` 取 SCHEMALESS，但保留唯一索引。** 旧端运行时对 `review_forms` 建的是 SCHEMAFULL + 13 个 `DEFINE FIELD` + `idx_form_id ... UNIQUE`（出处 `src/web_api/platform_api/review_form.rs:37-50`，由 `src/web_server/mod.rs:568` 的 `warm_review_forms_schema()` 在启动时执行——这是旧端第三处运行时 DDL，不在 `review_db.rs` 里，移植初版整块漏掉）。本仓取舍：**字段定义不移植，唯一索引必须移植**。字段定义不移植是因为写入路径不依赖 DB 侧 `DEFAULT`（`platform/review_form.rs:148-161` 的 `CREATE` 把 12 个字段全显式写出，三处 `UPDATE` 也显式写 `deleted` / `updated_at`），丢的是类型约束而非数据；且其余 9 张表一律 SCHEMALESS，单独把这张改回 SCHEMAFULL 会让「旧库整体迁入」更难办。唯一索引必须移植是因为它承担行为而非口径：`ensure_review_form_stub` 是无事务的「先 SELECT 再 CREATE」，并发下靠它挡住第二条 `CREATE`；缺失时不报错，只静默多出一行读不到的影子主单据。落地见 commit `987faa2fc`，复核见 `docs/evidence/2026-09-13-review-schema-parity.md`。

**贴计划 §11：**

> **R9 · schema/DDL 逐表对照复核（2026-09-13）。** 12/12 张表有结论，报告 `docs/evidence/2026-09-13-review-schema-parity.md`。查出 P1 一条：`review_forms` 漏了出处 C（`platform_api/review_form.rs:37-50`）的 `idx_form_id ... UNIQUE`，经决策卡 q-228 拍板由复核人代改，commit `987faa2fc` 已补，并新增两条护栏（索引名 + 唯一性行为），变异验证过。P2 七条均为有意偏离或两端一致的继承问题，逐条记在报告 §14。**计划 §1.3「两份 surql 以哪份为真」的答案要订正为：以旧端三处运行时 DDL 为真（`review_db.rs:190-207`、`review_db.rs:247-267`、`platform_api/review_form.rs:37-50`），两份 surql 文件从未被加载、只当史料。** 原口径只写了前两处，是本次 P1 的根因。

## 18. 本次复核没覆盖的

- 未起库实跑对拍：两端各建一个空库跑各自 DDL、再 `INFO FOR DB` / `INFO FOR TABLE` 逐张 diff，是比读源码更硬的证据。本任务原约束「不跑服务、不跑 cargo」，**NOT RUN**。（§15 的 cargo 跑动是 q-228 拍板后为验证修复而开的口子，只覆盖新端自己的 DDL，不是两端对拍）
- 未覆盖「旧库整体迁入」场景：旧库里 `review_forms` 已是 SCHEMAFULL，新端的 `DEFINE TABLE IF NOT EXISTS` 不会把它改回 SCHEMALESS，两边形态会不一致。该场景 ADR-072 明列为「未决」，不在本次范围
- 字段级类型对拍只做到「DDL 声明 vs handler 写入」，未核「handler 读出时的 serde 类型与 DDL 类型是否处处吻合」——那属于 R8 的接口规范核对
