-- ============================================================================
-- 三维校审域 · 表与索引（首启自动执行）
--
-- 【以哪份为真】旧端 plant-model-gen 有两份互相打架的表定义：
--   * `rs_surreal/review/review_tables.surql`   —— 把 review_form / review_opinion /
--     review_attachment / review_form_model 定义成 **SCHEMAFULL**；
--   * `resource/surreal/review_workflow.surql`  —— 把同名的三张表定义成 **SCHEMALESS**，
--     字段集也不同（多 created_at、少 file_name/file_size/mime_type）。
-- 两份都**不是**旧端跑起来时的实际形态：旧端启动只执行下面三处运行时 DDL，两份 surql
-- 文件从未被加载。因此本文件**以这三处运行时 DDL 为真**，两份旧 surql 只当史料。
--   A. `src/web_api/review_db.rs:190-207`（`ensure_review_query_indexes_inner`）——查询索引；
--   B. `src/web_api/review_db.rs:247-267`（`ensure_review_workflow_history_schema_inner`）
--      —— workflow_history 升级；
--   C. `src/web_api/platform_api/review_form.rs:37-50`（`ensure_review_forms_schema_inner`）
--      —— `review_forms` 的表定义与 `idx_form_id ... UNIQUE`，由 `src/web_server/mod.rs:568`
--      的 `warm_review_forms_schema()` 在启动时执行。**C 容易被漏**：它不在 review_db.rs 里，
--      本文件初版就漏了它那条唯一索引（复核见 `docs/evidence/2026-09-13-review-schema-parity.md`
--      §7 P1-1）。往后核对三处都要走一遍。
-- 移植计划 `docs/plans/2026-09-12-review-backend-port-plan.md` §1.3 末行、
-- 落点决定见 `docs/adr/ADR-072-review-data-persistence.md`。
--
-- 由此产生的四处「与旧端不同」，都是有意的：
--   1. review_attachment 是 **SCHEMALESS**。旧 SCHEMAFULL 版没有 file_name / file_size /
--      mime_type 三个字段，而上传 handler 每次都写它们（`review/attachments.rs:512-526`）
--      ——照旧 surql 建表，附件一条也存不进去。
--   2. 不定义 review_form（单数）与 review_opinion。两张表在旧端**无人读写**（全仓
--      `CREATE/UPDATE/DELETE/FROM` 扫过：写的是复数的 review_forms），建出来只会让
--      「库里有哪些表」这个问题多两个假答案。
--   3. 显式 `DEFINE TABLE IF NOT EXISTS … SCHEMALESS`。旧端靠首次 CREATE 隐式建表，
--      于是「表在不在」取决于有没有人写过；这里让首启后 `INFO FOR DB` 就是完整清单。
--      `IF NOT EXISTS` 不动既有定义：从旧端 SCHEMAFULL 库整体迁移过来的场景不在本次
--      范围（ADR-072「未决」一节）。
--   4. review_forms 取 **SCHEMALESS**，不照出处 C 那样 SCHEMAFULL + 13 个 DEFINE FIELD。
--      写入路径不依赖 DB 侧 DEFAULT（`platform/review_form.rs:148-161` 的 CREATE 把 12 个
--      字段全显式写出，三处 UPDATE 也显式写 deleted / updated_at），丢的是类型约束不是
--      数据；其余 9 张表一律 SCHEMALESS，单独把这张改回去会让 ADR-072「旧库整体迁入」
--      更难办。但 **`idx_form_id ... UNIQUE` 必须照搬**——那是行为，不是口径，见下方第 2 节。
--
-- 执行方：`src/web_service/review/schema.rs::ensure_review_schema`（`include_str!` 冻进
-- 二进制，进程内 OnceCell 只跑一次，30s DDL 超时 = 旧端 REVIEW_DDL_TIMEOUT）。
-- 改本文件即改行为，不必改代码；但 schema.rs 的 mem:// 实跑测试会跟着验它。
-- ============================================================================

-- ----------------------------------------------------------------------------
-- 1. 表。除 review_workflow_history 外一律 SCHEMALESS —— 旧端 handler 按需加字段
--    （TaskRow 的 15 个可选字段、旧数据的多种形态），SCHEMAFULL 会把它们挡在门外。
-- ----------------------------------------------------------------------------
DEFINE TABLE IF NOT EXISTS review_tasks SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_forms SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_form_model SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_records SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_comments SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_annotation_states SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_annotation_severity SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_attachment SCHEMALESS;
DEFINE TABLE IF NOT EXISTS review_history SCHEMALESS;

-- ----------------------------------------------------------------------------
-- 2. 查询索引（逐字取自 review_db.rs:190-207 的 ensure_review_query_indexes_inner）
-- ----------------------------------------------------------------------------
DEFINE INDEX IF NOT EXISTS idx_review_tasks_form_id ON TABLE review_tasks FIELDS form_id;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_requester_id ON TABLE review_tasks FIELDS requester_id;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_checker_id ON TABLE review_tasks FIELDS checker_id;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_reviewer_id ON TABLE review_tasks FIELDS reviewer_id;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_approver_id ON TABLE review_tasks FIELDS approver_id;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_current_node ON TABLE review_tasks FIELDS current_node;
DEFINE INDEX IF NOT EXISTS idx_review_tasks_status ON TABLE review_tasks FIELDS status;
DEFINE INDEX IF NOT EXISTS idx_review_records_form_id ON TABLE review_records FIELDS form_id;
DEFINE INDEX IF NOT EXISTS idx_review_records_task_id ON TABLE review_records FIELDS task_id;
DEFINE INDEX IF NOT EXISTS idx_review_comments_annotation_id ON TABLE review_comments FIELDS annotation_id;
DEFINE INDEX IF NOT EXISTS idx_review_attachment_form_id ON TABLE review_attachment FIELDS form_id;
DEFINE INDEX IF NOT EXISTS idx_review_form_model_form_id ON TABLE review_form_model FIELDS form_id;

-- 主单据唯一索引（逐字取自出处 C `platform_api/review_form.rs:50`，同名同字段）。
-- 这一条不在 review_db.rs 里，所以跟着「只有两处运行时 DDL」的口径核对时会整条漏掉。
-- 它不是口径而是行为：`ensure_review_form_stub` 是「先 SELECT 再 CREATE」的读改写，
-- 没有事务；PMS 的 embed-url 与 workflow/sync 对同一 form_id 并发进来时，靠的就是这条
-- 唯一索引把第二条 CREATE 挡下来。没有它就会静默多出一行主单据，而读取只取
-- `ORDER BY updated_at DESC LIMIT 1`，另一行成为读不到却仍在库里的影子行。
DEFINE INDEX IF NOT EXISTS idx_form_id ON TABLE review_forms FIELDS form_id UNIQUE;

-- ----------------------------------------------------------------------------
-- 3. 旧行补字段（review_db.rs:201-203 夹在索引之间的那一句）。幂等：只碰缺这一格的行。
-- ----------------------------------------------------------------------------
UPDATE review_records SET dimension_document_version = 0 WHERE dimension_document_version = NONE;

-- ----------------------------------------------------------------------------
-- 4. review_workflow_history / review_history —— RUS-244 形态
--    （逐字取自 review_db.rs:246-268 的 ensure_review_workflow_history_schema_inner）
--
--    这张表是全域唯一的 SCHEMAFULL 表，且必须用 OVERWRITE：旧字段
--    operator_id / operator_name / timestamp 曾是必填 string/datetime，而 RUS-244 之后
--    的写入路径只给 actor_* 一族——不改成 option，SCHEMAFULL 会**静默拒收**整条历史
--    （2026-05-09 生产 regression 的原样复现条件）。
-- ----------------------------------------------------------------------------
DEFINE TABLE OVERWRITE review_workflow_history SCHEMAFULL;
DEFINE FIELD OVERWRITE task_id ON review_workflow_history TYPE string;
DEFINE FIELD OVERWRITE node ON review_workflow_history TYPE string;
DEFINE FIELD OVERWRITE action ON review_workflow_history TYPE string;
DEFINE FIELD OVERWRITE operator_id ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE operator_name ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE timestamp ON review_workflow_history TYPE option<datetime> DEFAULT time::now();
DEFINE FIELD OVERWRITE comment ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE form_id ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE target_node ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE actor_id ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE actor_role ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE actor_name ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE source ON review_workflow_history TYPE option<string>;
DEFINE FIELD OVERWRITE created_at ON review_workflow_history TYPE option<datetime>;
DEFINE INDEX OVERWRITE idx_workflow_task ON review_workflow_history FIELDS task_id;
DEFINE INDEX OVERWRITE idx_workflow_form ON review_workflow_history FIELDS form_id;

DEFINE FIELD OVERWRITE form_id ON review_history TYPE option<string>;
DEFINE FIELD OVERWRITE source ON review_history TYPE option<string>;
DEFINE FIELD OVERWRITE created_at ON review_history TYPE option<datetime>;
