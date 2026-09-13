//! 校审域 · 首启建表建索引。
//!
//! DDL 本体在 `rs_surreal/review/review_tables.surql`，`include_str!` 冻进二进制
//! （同 `data_interface::embedded_surql` 的纪律：函数与表定义随代码版本走，不靠部署目录里
//! 那份会漂移的副本）。以哪份旧 surql 为真、为什么，写在那个文件的头上。
//!
//! 触发点是 [`super::db::fresh_review_db`]：六个业务子模块每次操作前本来就要取一次句柄，
//! 挂在那里等于「首次用到校审库时自动建好」，T2/T3 零改动。旧端是两个 OnceCell
//! （`ensure_review_query_indexes` / `ensure_review_workflow_history_schema`）分别懒加载，
//! 这里合并成一次整文件 DDL——两段本来就只在同一个进程里各跑一次，分开只会让
//! 「schema 到底就位没有」有两个答案。
//!
//! 文件归 T1b（`docs/plans/2026-09-12-review-backend-port-plan.md` §5 T1）。

use anyhow::Result;
use log::info;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::sync::OnceCell;

use super::db::await_review_ddl;

/// 表与索引定义的编译期快照。改行为改那个文件即可，不必动代码。
pub const REVIEW_SCHEMA_SURQL: &str =
    include_str!("../../../rs_surreal/review/review_tables.surql");

static REVIEW_SCHEMA_READY: OnceCell<()> = OnceCell::const_new();

/// 确保校审表与索引已就位。进程内只真跑一次；失败不写 OnceCell，下次调用自动重试。
pub async fn ensure_review_schema(db: &Surreal<Any>) -> Result<()> {
    REVIEW_SCHEMA_READY
        .get_or_try_init(|| apply_review_schema(db))
        .await?;
    Ok(())
}

/// schema 是否已在本进程就位。
pub fn review_schema_ready() -> bool {
    REVIEW_SCHEMA_READY.get().is_some()
}

/// `review_workflow_history` 是否已升级到 RUS-244 形态（旧端
/// `review_db::review_workflow_history_schema_ready` 的同名同义替身）。
///
/// 旧端把它单独做成一个 OnceCell，是因为那张表的升级与索引预热分两次跑；这里整份 DDL
/// 一次过，所以两个问题同一个答案。工作流写入路径据此判断要不要打
/// 「schema warmup is not ready」——语义不变。
pub fn review_workflow_history_schema_ready() -> bool {
    review_schema_ready()
}

async fn apply_review_schema(db: &Surreal<Any>) -> Result<()> {
    await_review_ddl("review.ensure_schema", db.query(REVIEW_SCHEMA_SURQL))
        .await?
        .check()?;
    info!(
        "[REVIEW_DB.schema] 校审表与索引已确认（tasks/forms/form_model/records/comments/\
         annotation_states/annotation_severity/attachment/history + workflow_history 升到 RUS-244 形态）"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use surrealdb::engine::any::connect;

    /// 每条测试自带一个干净的 `mem://`，并**直接调 [`apply_review_schema`]** 绕开
    /// 进程级 OnceCell——否则同一个测试进程里只有跑在最前面的那条真执行过 DDL。
    async fn fresh_db() -> Surreal<Any> {
        let db = connect("mem://").await.expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("review_schema_probe")
            .use_db("t1b")
            .await
            .expect("use ns/db");
        db
    }

    async fn table_names(db: &Surreal<Any>) -> Vec<String> {
        let mut response = db.query("INFO FOR DB;").await.expect("INFO FOR DB");
        let info: Option<Value> = response.take(0).expect("decode INFO FOR DB");
        info.as_ref()
            .and_then(|value| value.get("tables"))
            .and_then(Value::as_object)
            .map(|tables| tables.keys().cloned().collect())
            .unwrap_or_default()
    }

    async fn index_names(db: &Surreal<Any>, table: &str) -> Vec<String> {
        let mut response = db
            .query(format!("INFO FOR TABLE {table};"))
            .await
            .unwrap_or_else(|error| panic!("INFO FOR TABLE {table}: {error}"));
        let info: Option<Value> = response.take(0).expect("decode INFO FOR TABLE");
        info.as_ref()
            .and_then(|value| value.get("indexes"))
            .and_then(Value::as_object)
            .map(|indexes| indexes.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 整份 DDL 必须能在**本仓锁定的 2.1.4 fork** 上一次跑通。
    ///
    /// 这条不是「编译过」而是实跑：`DEFINE TABLE IF NOT EXISTS`、`DEFINE INDEX IF NOT EXISTS`、
    /// `DEFINE … OVERWRITE` 三种写法在两个 fork 上的支持度是这次移植的已知风险
    /// （计划 §7 风险 1）。换 SurrealDB 版本时这里先红。
    #[tokio::test]
    async fn the_whole_schema_applies_on_a_fresh_database() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("整份 DDL 应一次跑通");

        let tables = table_names(&db).await;
        for table in [
            "review_tasks",
            "review_forms",
            "review_form_model",
            "review_records",
            "review_comments",
            "review_annotation_states",
            "review_annotation_severity",
            "review_attachment",
            "review_history",
            "review_workflow_history",
        ] {
            assert!(
                tables.iter().any(|name| name == table),
                "首启之后 {table} 必须在库里：{tables:?}"
            );
        }

        // 旧 surql 里那两张没人读写的表不许跟着建出来（review_tables.surql 头部第 2 条）。
        for phantom in ["review_form", "review_opinion"] {
            assert!(
                !tables.iter().any(|name| name == phantom),
                "{phantom} 在旧端无人读写，不该出现在清单里：{tables:?}"
            );
        }
    }

    /// 索引清单逐条对旧端 `review_db.rs:190-207`：少一条就是某个列表页悄悄变成全表扫。
    #[tokio::test]
    async fn every_query_index_from_the_legacy_runtime_ddl_is_created() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("DDL");

        for (table, expected) in [
            (
                "review_tasks",
                vec![
                    "idx_review_tasks_form_id",
                    "idx_review_tasks_requester_id",
                    "idx_review_tasks_checker_id",
                    "idx_review_tasks_reviewer_id",
                    "idx_review_tasks_approver_id",
                    "idx_review_tasks_current_node",
                    "idx_review_tasks_status",
                ],
            ),
            (
                "review_records",
                vec!["idx_review_records_form_id", "idx_review_records_task_id"],
            ),
            ("review_comments", vec!["idx_review_comments_annotation_id"]),
            ("review_attachment", vec!["idx_review_attachment_form_id"]),
            (
                "review_form_model",
                vec!["idx_review_form_model_form_id"],
            ),
            (
                "review_workflow_history",
                vec!["idx_workflow_task", "idx_workflow_form"],
            ),
        ] {
            let actual = index_names(&db, table).await;
            for index in expected {
                assert!(
                    actual.iter().any(|name| name == index),
                    "{table} 缺索引 {index}：{actual:?}"
                );
            }
        }
    }

    /// RUS-244 回归的原样复现条件：只给 actor_* 一族、不给旧的 operator_* / timestamp。
    /// 旧字段若仍是必填 string/datetime，SCHEMAFULL 会**静默拒收**整条历史（2026-05-09 生产事故）。
    #[tokio::test]
    async fn workflow_history_accepts_rows_that_omit_the_legacy_operator_fields() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("DDL");

        db.query(
            "CREATE review_workflow_history SET \
                task_id = 'task-1', node = 'jd', action = 'approve', \
                form_id = 'form-1', target_node = 'sh', \
                actor_id = 'u-1', actor_role = 'jd', actor_name = '校对甲', \
                source = 'pms-sync', created_at = time::now();",
        )
        .await
        .expect("CREATE 应执行")
        .check()
        .expect("SCHEMAFULL 不得拒收 RUS-244 形态的历史行");

        let mut response = db
            .query(
                "SELECT task_id, action, actor_id, actor_role, source, \
                        <string>timestamp AS timestamp, operator_id \
                 FROM review_workflow_history;",
            )
            .await
            .expect("SELECT");
        let rows: Vec<Value> = response.take(0).expect("take rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row["task_id"], "task-1");
        assert_eq!(row["actor_role"], "jd");
        assert_eq!(row["source"], "pms-sync");
        assert!(
            row["operator_id"].is_null(),
            "旧字段必须是 option，不给就落 NONE：{row}"
        );
        chrono::DateTime::parse_from_rfc3339(
            row["timestamp"].as_str().expect("timestamp 必须有值"),
        )
        .expect("DEFAULT time::now() 必须把 timestamp 补上");
    }

    /// 旧形态（只给 operator_*）也要照收：移植不能把旧数据的写入路径写死。
    #[tokio::test]
    async fn workflow_history_still_accepts_the_legacy_operator_shape() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("DDL");

        db.query(
            "CREATE review_workflow_history SET \
                task_id = 'task-2', node = 'sj', action = 'submit', \
                operator_id = 'u-2', operator_name = '设计乙', timestamp = time::now();",
        )
        .await
        .expect("CREATE 应执行")
        .check()
        .expect("旧形态历史行同样要能写进去");
    }

    /// 附件表必须是 SCHEMALESS：旧 SCHEMAFULL 版没有 file_name / file_size / mime_type，
    /// 而上传 handler 每次都写这三格（`review/attachments.rs:512-526`）。这条测试就是
    /// 「谁哪天把 review_attachment 改回 SCHEMAFULL」的绊线。
    #[tokio::test]
    async fn attachment_rows_keep_the_fields_the_upload_handler_writes() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("DDL");

        db.query(
            "CREATE review_attachment CONTENT { \
                form_id: 'form-1', model_refnos: ['24381/145018'], file_id: 'att-1', \
                file_type: 'markup', download_url: '/files/review_attachments/att-1.pdf', \
                description: '云线', file_ext: '.pdf', file_name: '现场云线.pdf', \
                file_size: 2048, mime_type: 'application/pdf', created_at: time::now() };",
        )
        .await
        .expect("CREATE 应执行")
        .check()
        .expect("附件入库不得被表定义挡下");

        let mut response = db
            .query(
                "SELECT file_id, file_name, file_size, mime_type, download_url \
                 FROM review_attachment;",
            )
            .await
            .expect("SELECT");
        let rows: Vec<Value> = response.take(0).expect("take rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["file_name"], "现场云线.pdf");
        assert_eq!(rows[0]["file_size"], 2048);
        assert_eq!(rows[0]["mime_type"], "application/pdf");
        assert_eq!(
            rows[0]["download_url"],
            "/files/review_attachments/att-1.pdf"
        );
    }

    /// 第二次执行不得报错、也不得清掉第一次写进去的行——它每个进程都会跑一遍，
    /// 而 `review_workflow_history` 用的是 `OVERWRITE`（重定义表定义，不是重建表）。
    #[tokio::test]
    async fn applying_the_schema_twice_is_idempotent_and_keeps_existing_rows() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("第一次");

        db.query(
            "CREATE review_workflow_history SET task_id = 'task-3', node = 'sh', action = 'return', actor_id = 'u-3'; \
             CREATE review_tasks:t3 SET title = '保留我';",
        )
        .await
        .expect("CREATE")
        .check()
        .expect("种两行");

        apply_review_schema(&db).await.expect("第二次必须照样跑通");

        let mut response = db
            .query(
                "SELECT count() AS n FROM review_workflow_history GROUP ALL; \
                 SELECT title FROM review_tasks:t3;",
            )
            .await
            .expect("SELECT");
        let history: Option<Value> = response.take(0).expect("take history count");
        assert_eq!(
            history.as_ref().and_then(|row| row.get("n")),
            Some(&Value::from(1)),
            "重放 DDL 不该清掉历史行：{history:?}"
        );
        let task: Option<Value> = response.take(1).expect("take task");
        assert_eq!(
            task.as_ref().and_then(|row| row.get("title")),
            Some(&Value::from("保留我")),
            "重放 DDL 不该清掉任务行：{task:?}"
        );
    }

    /// 旧行补字段那一句（`review_db.rs:201-203`）要真的补，而且只补缺的那些。
    #[tokio::test]
    async fn the_record_backfill_only_touches_rows_missing_the_field() {
        let db = fresh_db().await;
        apply_review_schema(&db).await.expect("首次");

        db.query(
            "CREATE review_records:old SET form_id = 'f-1'; \
             CREATE review_records:new SET form_id = 'f-2', dimension_document_version = 7;",
        )
        .await
        .expect("CREATE")
        .check()
        .expect("种两行");

        // OnceCell 在 `ensure_review_schema` 那一层，直调 apply 就能重放这一句。
        apply_review_schema(&db).await.expect("重放");

        let mut response = db
            .query(
                "SELECT dimension_document_version AS v FROM review_records:old; \
                 SELECT dimension_document_version AS v FROM review_records:new;",
            )
            .await
            .expect("SELECT");
        let old: Option<Value> = response.take(0).expect("take old");
        let new: Option<Value> = response.take(1).expect("take new");
        assert_eq!(
            old.as_ref().and_then(|row| row.get("v")),
            Some(&Value::from(0)),
            "缺这一格的旧行要补 0：{old:?}"
        );
        assert_eq!(
            new.as_ref().and_then(|row| row.get("v")),
            Some(&Value::from(7)),
            "已有值的行不许被改写：{new:?}"
        );
    }
}
