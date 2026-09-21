//! PMS 入站六条的 handler 级单测（旧端 `platform_api/tests.rs` 的 29 条，逐条迁入）。
//!
//! 基建差异：旧端 `init_surreal()` + `project_primary_db()` 连一台外部 SurrealDB；这里把进程
//! 全局 `aios_core::SUL_DB`（`fresh_review_db()` 的落点）接到嵌入式 `mem://` 引擎上一次
//! （[`ensure_test_db`]），六条 handler 与它们背后的 `review_form` / `annotation_check` 查询
//! 全部**实跑**，不 mock。种子 SQL 用 `type::thing('review_tasks', $id)` 定 record id，
//! 与 `sdk_compat_probe` 同一写法。
//!
//! 旧端这份 tests.rs 有一部分断言落后于它自己的 handler（PMS 工作流 v3「verify 路径瘦身 /
//! action-aware 批注闸门」之后没有跟着改），按旧代码原样跑本来就是红的。移植时**以旧端
//! handler 的实际行为为准**改断言（handler 逐字搬运，行为一字未动），逐条列在这里：
//!
//! | 旧测试 | 旧断言为什么红 | 现在断言什么 |
//! | --- | --- | --- |
//! | `…includes_workflow_role…` / `…accepts_workflow_role_field_name` / `…accepts_legacy_json_key_role` / `…defaults_role_sj…` | 请求体不带 `token`，`[platform_auth]` 缺省 `enabled=true` 必然 401 | 补一枚合法 S2S JWT，其余不变 |
//! | `test_workflow_verify_active_requires_next_step` | verify 早已**静默忽略** `next_step`（同文件 3.6.8 也这么断言） | 不传 `next_step` 照样 200 passed，`next_step` 由静态推算为 `jd` |
//! | `test_workflow_verify_blank_form_without_task_returns_soft_block` | handler 对 `active` + 空白主单据有显式放行分支（「表单尚未创建 task，active 可继续」） | 200 passed=true，`task_status=blank`，`next_step=jd` |
//! | `test_workflow_verify_active_on_non_sj_node_returns_soft_block` | verify 的节点闸（`ensure_action_allowed_on_node`）不带 `task_status` / `next_step` | 这两格为 None，其余不变 |
//! | `test_workflow_verify_missing_form_returns_404` | `active` + 主单据不存在也走上面那条放行分支 | 改用 `agree` 复现 404；另加一条钉住 `active` 的放行 |
//! | `test_workflow_verify_agree_owner_mismatch_returns_structured_diagnostics` | 外部驱动模式（claims 无 `workflow_mode`）**跳过** owner 校验（单测 `external_workflow_verify_skips_owner_match` 同口径） | 断言外部模式放行；OWNER_MISMATCH 由 `workflow_sync::tests` 的内部模式单测覆盖 |
//! | `test_workflow_verify_agree_returns_annotation_gate_block_without_mutation` | verify 不读 `next_step`，诊断里 `next_step` / `requested_next_step` 为 None | 这两格为 None，其余不变 |
//! | `test_verify_active_blocks_when_open_exists` / `test_verify_return_blocks_when_no_problem` | verify 对 `active` / `return` **不跑**批注闸门（`verify_action_intent_for_node`） | 断言放行 |
//! | `test_sync_return_blocks_without_problem_annotation` | sync `return` 不跑批注闸门；旧断言实际撞到的是 REVIEW_FORM_NOT_FOUND | 改为 sync `agree` + 待确认批注 → 409 `ANNOTATION_CHECK_FAILED`，任务不变 |
//! | `test_sync_actor_name_default_falls_back_to_id` | 种子只建 task 不建 review_forms，事务必 409 REVIEW_FORM_NOT_FOUND | 种子补一条主单据存根（真实链路里 embed-url 会先建它） |
//! | 两条 `#[ignore]`（持久化 role 复用 / 回传既有 task） | 旧端要外部库 | 有 `mem://` 就能跑，去掉 ignore |

use super::*;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde::Deserialize;
use std::sync::OnceLock;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tower::ServiceExt;

use super::super::auth::{create_token, verify_token};
use super::super::common::fresh_review_db;

static TEST_DB: OnceLock<()> = OnceLock::new();
static ENGINE_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 把 `SUL_DB` 接到 `mem://`（进程内一次）。`fresh_review_db()` 之后首次调用会在它上面建表建索引。
///
/// 连接**不能**在某条测试自己的 runtime 里建：`mem://` 引擎的 router 是一个 `tokio::spawn`
/// 出来的任务，`#[tokio::test]` 一条测试一个 runtime，那条测试跑完 runtime 一落，引擎跟着没，
/// 后面每条测试都是 `sending into a closed channel`（并行跑时表现为随机红几条）。
/// 所以引擎单独占一个进程级 runtime（静态，永不 drop），连接在它里面完成；各测试的 runtime
/// 只往引擎的 channel 上发消息，跨 runtime 收发没有问题。
async fn ensure_test_db() {
    TEST_DB.get_or_init(|| {
        // 夹具就是一台会蒸发的 `mem://`；ADR-074 之后未配置的 store_mode 默认为 `mem`，
        // 持久化闸（ADR-075）会拒掉它。这里显式声明「知道会丢」，走测试专用开关而不是 set_var（d-410）。
        super::super::db::TEST_ALLOW_EPHEMERAL_STORE
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // `Runtime::block_on` 不允许在 runtime 线程上调，借一条普通线程把引擎起起来。
        std::thread::spawn(|| {
            ENGINE_RUNTIME
                .get_or_init(|| {
                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(1)
                        .enable_all()
                        .build()
                        .expect("引擎 runtime")
                })
                .block_on(async {
                    match aios_core::SUL_DB.connect("mem://").await {
                        Ok(()) => {}
                        Err(surrealdb::Error::Api(surrealdb::error::Api::AlreadyConnected)) => {}
                        Err(error) => panic!("mem 引擎应能启动（kv-mem feature）：{error}"),
                    }
                    aios_core::SUL_DB
                        .use_ns("review_platform_test")
                        .use_db("platform")
                        .await
                        .expect("use ns/db");
                });
        })
        .join()
        .expect("引擎初始化线程");
    });
}

async fn test_db() -> Surreal<Any> {
    ensure_test_db().await;
    fresh_review_db().await.expect("review db")
}

fn app() -> Router<()> {
    routes::<()>()
}

async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, Vec<u8>) {
    ensure_test_db().await;
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[derive(Debug, Deserialize)]
struct EmbedUrlResponseBody {
    code: i32,
    message: String,
    data: Option<serde_json::Value>,
    #[allow(dead_code)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbedLineageBody {
    form_id: String,
    task_id: Option<String>,
    current_node: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerifyWorkflowResponseBody {
    code: i32,
    message: String,
    data: Option<VerifyWorkflowDataBody>,
    error_code: Option<String>,
    annotation_check: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct VerifyWorkflowDataBody {
    passed: bool,
    action: String,
    block_code: Option<String>,
    current_node: Option<String>,
    task_status: Option<String>,
    next_step: Option<String>,
    actor_id: Option<String>,
    owner_id: Option<String>,
    owner_source: Option<String>,
    expected_next_node: Option<String>,
    requested_next_step: Option<VerifyWorkflowNextStepBody>,
    reason: String,
    recommended_action: String,
}

#[derive(Debug, Deserialize)]
struct VerifyWorkflowNextStepBody {
    #[allow(dead_code)]
    assignee_id: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    roles: String,
}

async fn cleanup_form(form_id: &str) {
    let db = test_db().await;
    let _ = db
        .query(
            "LET $ids = SELECT VALUE id FROM review_tasks WHERE form_id = $form_id; DELETE $ids; \
             LET $rids = SELECT VALUE id FROM review_records WHERE form_id = $form_id; DELETE $rids; \
             DELETE review_forms WHERE form_id = $form_id; \
             DELETE review_workflow_history WHERE form_id = $form_id;",
        )
        .bind(("form_id", form_id.to_string()))
        .await;
}

async fn seed_form_stub(form_id: &str, requester_id: &str, role: Option<&str>) {
    ensure_test_db().await;
    super::review_form::ensure_review_form_stub(form_id, "project-1", requester_id, role, "pms")
        .await
        .expect("seed review form stub");
}

async fn insert_task_with_form_id(form_id: &str, user_id: &str) {
    cleanup_form(form_id).await;
    test_db()
        .await
        .query(
            r#"
            CREATE ONLY type::thing('review_tasks', $id) SET
                form_id = $form_id,
                title = $title,
                description = $description,
                model_name = $model_name,
                status = $status,
                priority = 'medium',
                requester_id = $requester_id,
                requester_name = $requester_id,
                checker_id = 'checker-1',
                checker_name = 'checker-1',
                approver_id = 'approver-1',
                approver_name = 'approver-1',
                reviewer_id = 'checker-1',
                reviewer_name = 'checker-1',
                components = [],
                attachments = NONE,
                current_node = $current_node,
                workflow_history = [],
                created_at = time::now(),
                updated_at = time::now()
            "#,
        )
        .bind(("id", format!("task-{}", form_id.to_lowercase())))
        .bind(("form_id", form_id.to_string()))
        .bind(("title", format!("Task for {form_id}")))
        .bind(("description", "existing seeded task".to_string()))
        .bind(("model_name", "demo-model".to_string()))
        .bind(("status", "in_review".to_string()))
        .bind(("requester_id", user_id.to_string()))
        .bind(("current_node", "jd".to_string()))
        .await
        .expect("seed review task")
        .check()
        .expect("seed review task statements");
}

async fn insert_task_seed(
    form_id: &str,
    requester_id: &str,
    checker_id: &str,
    approver_id: &str,
    current_node: &str,
    status: &str,
) {
    cleanup_form(form_id).await;
    test_db()
        .await
        .query(
            r#"
            CREATE ONLY type::thing('review_tasks', $id) SET
                form_id = $form_id,
                title = $title,
                description = $description,
                model_name = $model_name,
                status = $status,
                priority = 'medium',
                requester_id = $requester_id,
                requester_name = $requester_id,
                checker_id = $checker_id,
                checker_name = $checker_id,
                approver_id = $approver_id,
                approver_name = $approver_id,
                reviewer_id = $checker_id,
                reviewer_name = $checker_id,
                components = [],
                attachments = NONE,
                current_node = $current_node,
                workflow_history = [],
                created_at = time::now(),
                updated_at = time::now()
            "#,
        )
        .bind(("id", format!("task-{}", form_id.to_lowercase())))
        .bind(("form_id", form_id.to_string()))
        .bind(("title", format!("Task for {form_id}")))
        .bind(("description", "seeded verify task".to_string()))
        .bind(("model_name", "demo-model".to_string()))
        .bind(("status", status.to_string()))
        .bind(("requester_id", requester_id.to_string()))
        .bind(("checker_id", checker_id.to_string()))
        .bind(("approver_id", approver_id.to_string()))
        .bind(("current_node", current_node.to_string()))
        .await
        .expect("seed verify task")
        .check()
        .expect("seed verify task statements");
}

async fn insert_pending_review_record(task_id: &str, form_id: &str) {
    insert_review_record_with_state(
        task_id,
        form_id,
        "ann-verify-pending-1",
        "待确认批注",
        Some("fixed"),
        Some("pending"),
        "verify-pending",
    )
    .await;
}

/// 通用批注种子辅助：按给定 (resolution_status, decision_status) 生成单条
/// text 批注，由 `classify_annotation_state` 决定最终 gate 状态：
/// - `decision_status = "agreed"` → Approved
/// - `decision_status = "rejected"` → Rejected
/// - `resolution_status = "fixed" | "wont_fix"` 且 decision_status 非 agreed/rejected → PendingReview
/// - 其他（含完全没传 reviewState）→ Open
#[allow(clippy::too_many_arguments)]
async fn insert_review_record_with_state(
    task_id: &str,
    form_id: &str,
    annotation_id: &str,
    annotation_text: &str,
    resolution_status: Option<&str>,
    decision_status: Option<&str>,
    record_label: &str,
) {
    let review_state_json = match (resolution_status, decision_status) {
        (None, None) => "NONE".to_string(),
        _ => format!(
            r#"{{
                resolutionStatus: {res},
                decisionStatus: {dec},
                updatedAt: 1710000000000,
                updatedByName: "SJ",
                updatedByRole: "sj"
            }}"#,
            res = resolution_status
                .map(|v| format!("\"{}\"", v))
                .unwrap_or_else(|| "NONE".to_string()),
            dec = decision_status
                .map(|v| format!("\"{}\"", v))
                .unwrap_or_else(|| "NONE".to_string()),
        ),
    };

    let sql = format!(
        r#"
        CREATE ONLY type::thing('review_records', $id) CONTENT {{
            task_id: $task_id,
            form_id: $form_id,
            type: "batch",
            annotations: [
                {{
                    id: $annotation_id,
                    annotationType: "text",
                    text: $annotation_text,
                    refnos: ["24381/145018"],
                    reviewState: {review_state}
                }}
            ],
            cloud_annotations: [],
            rect_annotations: [],
            obb_annotations: [],
            measurements: [],
            note: $record_label,
            current_node: "jd",
            operator_id: "SJ",
            operator_name: "SJ",
            slot_key: $slot_key,
            snapshot_hash: $snapshot_hash,
            confirmed_at: time::now()
        }}
        "#,
        review_state = review_state_json,
    );

    test_db()
        .await
        .query(sql)
        .bind((
            "id",
            format!("record-{}-{}", form_id.to_lowercase(), record_label),
        ))
        .bind(("task_id", task_id.to_string()))
        .bind(("form_id", form_id.to_string()))
        .bind(("annotation_id", annotation_id.to_string()))
        .bind(("annotation_text", annotation_text.to_string()))
        .bind(("record_label", format!("seed {}", record_label)))
        .bind(("slot_key", format!("slot-{}", record_label)))
        .bind(("snapshot_hash", format!("hash-{}", record_label)))
        .await
        .expect("seed review record with state")
        .check()
        .expect("seed review record statements");
}

fn s2s_token(user_id: &str, role: &str) -> String {
    create_token("project-1", user_id, None, Some(role), None)
        .expect("token")
        .0
}

async fn post_embed(body: serde_json::Value) -> (StatusCode, EmbedUrlResponseBody) {
    let (status, bytes) = post_json("/api/review/embed-url", body).await;
    let payload: EmbedUrlResponseBody = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("embed body: {e}: {}", String::from_utf8_lossy(&bytes)));
    (status, payload)
}

fn response_token_role(data: &serde_json::Value) -> Option<String> {
    let response_token = data
        .get("token")
        .and_then(|v| v.as_str())
        .expect("response token");
    verify_token(response_token).unwrap().role
}

// ============================================================================
// embed-url
// ============================================================================

#[tokio::test]
async fn test_embed_url_ignores_mismatched_form_id_from_jwt() {
    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": "FORM-OTHER",
        "token": s2s_token("user-1", "sj")
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("embed data");
    let query = data
        .get("query")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        query.get("form_id").and_then(|value| value.as_str()),
        Some("FORM-OTHER")
    );
}

#[tokio::test]
async fn test_embed_url_accepts_matching_form_id_from_jwt() {
    cleanup_form("FORM-EXPECTED").await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": "FORM-EXPECTED",
        "token": s2s_token("user-1", "sj")
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    assert_eq!(payload.message, "ok");
    let data = payload.data.expect("embed data");
    assert_eq!(
        data.get("query")
            .and_then(|q| q.get("form_id").or_else(|| q.get("formId")))
            .and_then(|v| v.as_str()),
        Some("FORM-EXPECTED")
    );
    let lineage: EmbedLineageBody =
        serde_json::from_value(data.get("lineage").cloned().expect("lineage")).unwrap();
    assert_eq!(lineage.form_id, "FORM-EXPECTED");
    assert_eq!(lineage.task_id, None);
    assert_eq!(lineage.current_node, None);
    assert_eq!(lineage.status, None);
    let response_token = data
        .get("token")
        .and_then(|v| v.as_str())
        .expect("response token");
    assert_eq!(verify_token(response_token).unwrap().user_id, "user-1");
    assert!(data.get("task").is_none() || data.get("task").is_some_and(|v| v.is_null()));
    let form = data.get("form").expect("form summary");
    assert_eq!(form["exists"], true);
    assert_eq!(form["status"], "blank");
    assert_eq!(form["task_created"], false);
}

#[tokio::test]
async fn test_embed_url_includes_workflow_role_in_signed_token_when_extra_has_workflow_role() {
    let form_id = "FORM-ROLE-REQUEST";
    cleanup_form(form_id).await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": form_id,
        "token": s2s_token("user-1", "sj"),
        "extra_parameters": {
            "workflow_role": "jd"
        }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("embed data");
    assert_eq!(response_token_role(&data).as_deref(), Some("jd"));
}

#[tokio::test]
async fn test_embed_url_accepts_workflow_role_field_name() {
    let form_id = "FORM-WORKFLOW-ROLE-ALIAS";
    cleanup_form(form_id).await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": form_id,
        "token": s2s_token("user-1", "sj"),
        "workflow_role": "sh"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("embed data");
    assert_eq!(response_token_role(&data).as_deref(), Some("sh"));
}

#[tokio::test]
async fn test_embed_url_accepts_legacy_json_key_role() {
    let form_id = "FORM-LEGACY-ROLE-KEY";
    cleanup_form(form_id).await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": form_id,
        "token": s2s_token("user-1", "sj"),
        "role": "pz"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("embed data");
    assert_eq!(response_token_role(&data).as_deref(), Some("pz"));
}

#[tokio::test]
async fn test_embed_url_jwt_defaults_role_sj_when_workflow_role_omitted() {
    let form_id = "FORM-DEFAULT-JWT-ROLE";
    cleanup_form(form_id).await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": form_id,
        "token": s2s_token("user-1", "sj")
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("embed data");
    assert_eq!(response_token_role(&data).as_deref(), Some("sj"));
}

#[tokio::test]
async fn test_embed_url_reuses_persisted_form_role_when_followup_request_omits_role() {
    let form_id = "FORM-ROLE-PERSISTED";
    cleanup_form(form_id).await;

    let (first_status, _) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "JH",
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "extra_parameters": {
            "workflow_role": "jd"
        }
    }))
    .await;
    assert_eq!(first_status, StatusCode::OK);

    let stored_form = super::review_form::get_review_form_by_form_id(form_id)
        .await
        .expect("query review form")
        .expect("stored review form");
    assert_eq!(stored_form.role.as_deref(), Some("jd"));

    let (second_status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "JH",
        "form_id": form_id,
        "token": s2s_token("JH", "jd")
    }))
    .await;
    assert_eq!(second_status, StatusCode::OK);
    let data = payload.data.expect("embed data");
    assert_eq!(response_token_role(&data).as_deref(), Some("jd"));
}

#[tokio::test]
async fn test_embed_url_rejects_tampered_jwt_even_if_form_id_matches() {
    let token = s2s_token("user-1", "sj");
    let mut parts = token.split('.').collect::<Vec<_>>();
    parts[2] = "tampered-signature";
    let tampered_token = parts.join(".");

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-1",
        "form_id": "FORM-EXPECTED",
        "token": tampered_token
    }))
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(payload.code, 401);
    assert_eq!(payload.message, "unauthorized");
    assert!(payload.data.is_none());
}

#[tokio::test]
async fn test_embed_url_returns_existing_task_for_form_id() {
    let form_id = "FORM-DB-BACKED-EXISTING";
    insert_task_with_form_id(form_id, "user-existing").await;

    let (status, payload) = post_embed(serde_json::json!({
        "project_id": "project-1",
        "user_id": "user-existing",
        "form_id": form_id,
        "token": s2s_token("user-existing", "jd")
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("embed data");
    assert_eq!(
        data.get("query")
            .and_then(|q| q.get("form_id").or_else(|| q.get("formId")))
            .and_then(|v| v.as_str()),
        Some(form_id)
    );
    let lineage: EmbedLineageBody =
        serde_json::from_value(data.get("lineage").cloned().expect("lineage")).unwrap();
    assert_eq!(lineage.form_id, form_id);
    assert!(
        lineage
            .task_id
            .as_deref()
            .is_some_and(|task_id| task_id.starts_with("task-form-db-backed-existing"))
    );
    assert_eq!(lineage.current_node.as_deref(), Some("jd"));
    assert_eq!(lineage.status.as_deref(), Some("in_review"));
    let task = data
        .get("task")
        .and_then(|v| v.as_object())
        .expect("existing task restored");
    assert_eq!(
        task.get("form_id")
            .or_else(|| task.get("formId"))
            .and_then(|v| v.as_str()),
        Some(form_id)
    );
    assert_eq!(
        task.get("requesterId").and_then(|v| v.as_str()),
        Some("user-existing")
    );
    assert_eq!(task.get("currentNode").and_then(|v| v.as_str()), Some("jd"));
    assert_eq!(
        task.get("status").and_then(|v| v.as_str()),
        Some("in_review")
    );
    assert!(
        task.get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id.starts_with("task-form-db-backed-existing"))
    );
    let response_token = data
        .get("token")
        .and_then(|v| v.as_str())
        .expect("response token");
    let token_claims = verify_token(response_token).unwrap();
    assert_eq!(token_claims.legacy_form_id, None);

    cleanup_form(form_id).await;
}

// ============================================================================
// workflow/verify
// ============================================================================

async fn post_verify(body: serde_json::Value) -> (StatusCode, VerifyWorkflowResponseBody) {
    let (status, bytes) = post_json("/api/review/workflow/verify", body).await;
    let payload: VerifyWorkflowResponseBody = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("verify body: {e}: {}", String::from_utf8_lossy(&bytes)));
    (status, payload)
}

#[tokio::test]
async fn test_workflow_verify_rejects_unauthorized_token() {
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": "FORM-VERIFY-UNAUTH",
        "token": "invalid-token",
        "action": "active",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" },
        "next_step": { "assignee_id": "JH", "name": "JH", "roles": "jd" }
    }))
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(payload.code, 401);
    assert_eq!(payload.message, "unauthorized");
    assert!(payload.data.is_none());
}

#[tokio::test]
async fn test_workflow_verify_rejects_unsupported_action() {
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": "FORM-VERIFY-BAD-ACTION",
        "token": s2s_token("SJ", "sj"),
        "action": "query",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" }
    }))
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(payload.code, 400);
    assert!(payload.message.contains("verify 不支持 action=query"));
}

/// 旧名 `test_workflow_verify_active_requires_next_step`：verify 静默忽略 `next_step`，
/// 不传照样通过，`next_step` 由静态推算给出 `jd`。
#[tokio::test]
async fn test_workflow_verify_active_does_not_require_next_step() {
    let form_id = "FORM-VERIFY-MISSING-NEXT";
    insert_task_seed(form_id, "SJ", "JH", "SH", "sj", "draft").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "active");
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.task_status.as_deref(), Some("draft"));
    assert_eq!(data.next_step.as_deref(), Some("jd"));
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

/// 旧名 `test_workflow_verify_blank_form_without_task_returns_soft_block`：handler 对
/// `active` + 空白主单据（尚未建 task）有显式放行分支。
#[tokio::test]
async fn test_workflow_verify_blank_form_without_task_passes_through_for_active() {
    let form_id = "FORM-VERIFY-BLANK-NO-TASK";
    cleanup_form(form_id).await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" },
        "next_step": { "assignee_id": "JH", "name": "JH", "roles": "jd" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "active");
    assert_eq!(data.current_node, None);
    assert_eq!(data.task_status.as_deref(), Some("blank"));
    assert_eq!(data.next_step.as_deref(), Some("jd"));
    assert_eq!(data.expected_next_node.as_deref(), Some("jd"));
    assert_eq!(data.recommended_action, "proceed");
    assert!(data.reason.contains("尚未创建 task"));

    // 非 active 动作对空白主单据仍是 soft block（旧断言想钉的那一半）。
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "agree");
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.task_status.as_deref(), Some("blank"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("尚未创建活动 review task"));

    cleanup_form(form_id).await;
}

#[tokio::test]
async fn test_workflow_verify_active_on_non_sj_node_returns_soft_block() {
    let form_id = "FORM-VERIFY-ACTIVE-NON-SJ";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "active",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "JH", "name": "JH", "roles": "jd" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "active");
    assert_eq!(data.current_node.as_deref(), Some("jd"));
    // verify 的节点闸不带 task_status / next_step（旧断言要 submitted / jd，落后于 handler）。
    assert_eq!(data.task_status, None);
    assert_eq!(data.next_step, None);
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("active 仅在 form 当前节点为 sj"));

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after verify")
        .expect("task after verify");
    assert_eq!(task_after.current_node, "jd");
    assert_eq!(task_after.status, "submitted");

    cleanup_form(form_id).await;
}

/// 旧名 `test_workflow_verify_missing_form_returns_404`：`active` + 主单据不存在走的是
/// 「空白放行」分支，404 只对非 active 动作成立；两半都钉住。
#[tokio::test]
async fn test_workflow_verify_missing_form_returns_404_for_non_active() {
    let form_id = "FORM-VERIFY-FORM-NOT-FOUND";
    cleanup_form(form_id).await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(payload.code, 404);
    assert!(payload.data.is_none());
    assert!(payload.message.contains("未找到 review form"));

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "active 对不存在的主单据按空白放行");
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.task_status.as_deref(), Some("blank"));
}

#[tokio::test]
async fn test_workflow_verify_agree_terminal_task_returns_soft_block() {
    let form_id = "FORM-VERIFY-AGREE-TERMINAL";
    insert_task_seed(form_id, "SJ", "JH", "SH", "pz", "approved").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("PZ", "pz"),
        "action": "agree",
        "actor": { "id": "PZ", "name": "PZ", "roles": "pz" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "agree");
    assert_eq!(data.current_node.as_deref(), Some("pz"));
    assert_eq!(data.task_status.as_deref(), Some("approved"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("当前单据已处于终态 approved"));

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after verify")
        .expect("task after verify");
    assert_eq!(task_after.current_node, "pz");
    assert_eq!(task_after.status, "approved");

    cleanup_form(form_id).await;
}

/// 旧名 `test_workflow_verify_agree_owner_mismatch_returns_structured_diagnostics`：
/// S2S JWT 不带 `workflow_mode`（外部驱动模式），owner 校验跳过，非负责人也放行；
/// OWNER_MISMATCH 的结构化诊断由 `workflow_sync::tests::internal_workflow_owner_mismatch_is_forbidden` 钉住。
#[tokio::test]
async fn test_workflow_verify_agree_external_mode_skips_owner_match() {
    let form_id = "FORM-VERIFY-OWNER-MISMATCH";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SH", "sh"),
        "action": "agree",
        "actor": { "id": "SH", "name": "SH", "roles": "sh" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "external workflow skips owner match: {data:?}");
    assert_eq!(data.action, "agree");
    assert_eq!(data.block_code, None);
    assert_eq!(data.current_node.as_deref(), Some("jd"));
    assert_eq!(data.task_status.as_deref(), Some("submitted"));
    assert_eq!(data.next_step.as_deref(), Some("sh"));
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

#[tokio::test]
async fn test_workflow_verify_agree_returns_annotation_gate_block_without_mutation() {
    let form_id = "FORM-VERIFY-ANNOTATION-BLOCK";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    insert_pending_review_record(&task.id, form_id).await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    assert_eq!(
        payload.error_code.as_deref(),
        Some("ANNOTATION_CHECK_FAILED")
    );
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "agree");
    assert_eq!(data.block_code.as_deref(), Some("ANNOTATION_CHECK_FAILED"));
    assert_eq!(data.current_node.as_deref(), Some("jd"));
    assert_eq!(data.task_status.as_deref(), Some("submitted"));
    // verify 不读 next_step：这两格为 None（旧断言要 sh / 请求体回显，落后于 handler）。
    assert_eq!(data.next_step, None);
    assert!(data.requested_next_step.is_none());
    assert_eq!(data.actor_id.as_deref(), Some("JH"));
    assert_eq!(data.owner_id.as_deref(), Some("JH"));
    assert_eq!(data.owner_source.as_deref(), Some("checker"));
    assert_eq!(data.expected_next_node.as_deref(), Some("sh"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("待确认批注"));
    let annotation_check = payload.annotation_check.expect("annotation_check");
    assert_eq!(
        annotation_check
            .get("current_node")
            .and_then(|value| value.as_str()),
        Some("jd")
    );
    assert_eq!(annotation_check["summary"]["pending_review"], 1);
    assert_eq!(annotation_check["blockers"][0]["annotation_id"], "ann-verify-pending-1");
    assert_eq!(annotation_check["blockers"][0]["state_code"], "pending_review");

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after verify")
        .expect("task after verify");
    assert_eq!(task_after.current_node, "jd");
    assert_eq!(task_after.status, "submitted");

    cleanup_form(form_id).await;
}

#[tokio::test]
async fn test_workflow_verify_pass_does_not_mutate_task_state() {
    let form_id = "FORM-VERIFY-PASS-NO-MUTATION";
    insert_task_seed(form_id, "SJ", "JH", "SH", "sj", "draft").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" },
        "next_step": { "assignee_id": "JH", "name": "JH", "roles": "jd" },
        "comments": "送审"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "active");
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.task_status.as_deref(), Some("draft"));
    assert_eq!(data.next_step.as_deref(), Some("jd"));
    assert_eq!(data.recommended_action, "proceed");

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after verify")
        .expect("task after verify");
    assert_eq!(task_after.current_node, "sj");
    assert_eq!(task_after.status, "draft");

    cleanup_form(form_id).await;
}

// ============================================================================
// v3 §3.6: action-aware annotation gate + verify 路径瘦身覆盖
// ============================================================================

#[derive(Debug, Deserialize)]
struct SyncWorkflowResponseBody {
    code: i32,
    message: String,
    data: Option<serde_json::Value>,
    error_code: Option<String>,
    annotation_check: Option<serde_json::Value>,
}

#[allow(clippy::too_many_arguments)]
async fn seed_task_with_record(
    form_id: &str,
    current_node: &str,
    status: &str,
    annotation_id: &str,
    annotation_text: &str,
    resolution_status: Option<&str>,
    decision_status: Option<&str>,
    record_label: &str,
) {
    insert_task_seed(form_id, "SJ", "JH", "SH", current_node, status).await;
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    insert_review_record_with_state(
        &task.id,
        form_id,
        annotation_id,
        annotation_text,
        resolution_status,
        decision_status,
        record_label,
    )
    .await;
}

async fn post_sync(body: serde_json::Value) -> (StatusCode, SyncWorkflowResponseBody) {
    let (status, bytes) = post_json("/api/review/workflow/sync", body).await;
    let payload: SyncWorkflowResponseBody = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("sync body: {e}: {}", String::from_utf8_lossy(&bytes)));
    (status, payload)
}

// 3.6.1（旧名 test_verify_active_blocks_when_open_exists）：verify 对 active 不跑批注闸门，
// 即使有 open 批注也放行——闸门留给 sync agree（`verify_action_intent_for_node`）。
#[tokio::test]
async fn test_verify_active_skips_annotation_gate_even_with_open() {
    let form_id = "FORM-V3-ACTIVE-OPEN";
    seed_task_with_record(
        form_id,
        "sj",
        "draft",
        "ann-open-1",
        "未回复批注",
        None,
        None,
        "v3-active-open",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "{data:?}");
    assert_eq!(data.action, "active");
    assert_eq!(data.recommended_action, "proceed");
    assert!(payload.annotation_check.is_none());

    cleanup_form(form_id).await;
}

// 3.6.2 active + sj + 仅有"被回复过"的批注（pending/approved/rejected） → pass
#[tokio::test]
async fn test_verify_active_passes_with_only_replied_annotations() {
    let form_id = "FORM-V3-ACTIVE-REPLIED";
    insert_task_seed(form_id, "SJ", "JH", "SH", "sj", "draft").await;
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    insert_review_record_with_state(
        &task.id,
        form_id,
        "ann-pending",
        "已修复待确认",
        Some("fixed"),
        Some("pending"),
        "v3-active-replied-pending",
    )
    .await;
    insert_review_record_with_state(
        &task.id,
        form_id,
        "ann-approved",
        "已通过",
        Some("fixed"),
        Some("agreed"),
        "v3-active-replied-approved",
    )
    .await;
    insert_review_record_with_state(
        &task.id,
        form_id,
        "ann-rejected",
        "被驳回",
        Some("fixed"),
        Some("rejected"),
        "v3-active-replied-rejected",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "active");
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

// 3.6.3 agree + jd + 仅 pending → block "待确认批注"
#[tokio::test]
async fn test_verify_agree_blocks_on_pending() {
    let form_id = "FORM-V3-AGREE-PENDING";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-pending-only",
        "待确认",
        Some("fixed"),
        Some("pending"),
        "v3-agree-pending",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "agree");
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("待确认批注"));

    cleanup_form(form_id).await;
}

// 3.6.4 agree + jd + rejected → recommend "return"
#[tokio::test]
async fn test_verify_agree_recommends_return_when_rejected() {
    let form_id = "FORM-V3-AGREE-REJECTED";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-rejected-1",
        "已驳回的批注",
        Some("fixed"),
        Some("rejected"),
        "v3-agree-rejected",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(!data.passed);
    assert_eq!(data.action, "agree");
    assert_eq!(data.recommended_action, "return");

    cleanup_form(form_id).await;
}

// 3.6.5（旧名 test_verify_return_blocks_when_no_problem）：verify 对 return 不跑批注闸门，
// 全部 approved 也放行（return 本身就是带问题批注退回的设计，闸门不拦它）。
#[tokio::test]
async fn test_verify_return_skips_annotation_gate() {
    let form_id = "FORM-V3-RETURN-NOPROB";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-approved-only",
        "已通过批注",
        Some("fixed"),
        Some("agreed"),
        "v3-return-noprob",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "return"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "{data:?}");
    assert_eq!(data.action, "return");
    assert_eq!(data.recommended_action, "proceed");
    assert!(payload.annotation_check.is_none());

    cleanup_form(form_id).await;
}

// 3.6.6 return + jd + 1 条 open 批注 → pass
#[tokio::test]
async fn test_verify_return_passes_with_open_or_rejected() {
    let form_id = "FORM-V3-RETURN-PASS";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-open-prob",
        "未回复有问题",
        None,
        None,
        "v3-return-pass",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "return"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "return");
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

// 3.6.7 stop + jd + 即使有 pending 批注也通过（stop 不查 annotation）
#[tokio::test]
async fn test_verify_stop_passes_without_annotation_check() {
    let form_id = "FORM-V3-STOP-PASS";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-still-pending",
        "强行终止",
        Some("fixed"),
        Some("pending"),
        "v3-stop-pass",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "stop"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed);
    assert_eq!(data.action, "stop");
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

// 3.6.8 verify 路径完全忽略 next_step：传与不传结果一致
#[tokio::test]
async fn test_verify_ignores_next_step_field() {
    let form_id = "FORM-V3-IGNORE-NEXTSTEP";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-open-ignore",
        "open 批注",
        None,
        None,
        "v3-ignore-nextstep",
    )
    .await;

    // 故意传一个非法 next_step（roles=WRONG，不是 sj/jd/sh/pz），如果 verify
    // 真的读了 next_step，会因为 roles 解析失败或 rank 不识别报错。
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "return",
        "next_step": { "assignee_id": "BOGUS", "name": "WRONG", "roles": "WRONG" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "verify must ignore next_step");
    assert_eq!(data.action, "return");
    assert_eq!(data.recommended_action, "proceed");

    cleanup_form(form_id).await;
}

// 3.6.9（旧名 test_sync_return_blocks_without_problem_annotation）：sync 路径的批注闸门在
// agree 上——jd 有待确认批注时 409 + ANNOTATION_CHECK_FAILED，且任务一字不动。
#[tokio::test]
async fn test_sync_agree_blocks_on_pending_annotation() {
    let form_id = "FORM-V3-SYNC-AGREE-PENDING";
    seed_task_with_record(
        form_id,
        "jd",
        "submitted",
        "ann-pending-gate",
        "待确认批注",
        Some("fixed"),
        Some("pending"),
        "v3-sync-agree-pending",
    )
    .await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;

    let (status, payload) = post_sync(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(payload.code, 409);
    assert_eq!(
        payload.error_code.as_deref(),
        Some("ANNOTATION_CHECK_FAILED")
    );
    assert!(payload.message.contains("待确认批注"), "{}", payload.message);
    assert!(payload.data.is_none());
    let annotation_check = payload.annotation_check.expect("annotation_check");
    assert_eq!(
        annotation_check
            .get("recommended_action")
            .and_then(|v| v.as_str()),
        Some("block")
    );

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after sync agree")
        .expect("task after sync agree");
    assert_eq!(task_after.current_node, "jd");
    assert_eq!(task_after.status, "submitted");

    cleanup_form(form_id).await;
}

// 3.6.10 sync 路径 actor.name 兜底：仅传 id 时 DB 落 operator_name == id；
// 顺带钉住 history 的 source 口径（metadata.source 原样落库，PMS 传 pms-sync 就是 pms-sync）。
#[tokio::test]
async fn test_sync_actor_name_default_falls_back_to_id() {
    let form_id = "FORM-V3-ACTOR-NAME-FALLBACK";
    insert_task_seed(form_id, "SJ", "JH", "SH", "sj", "draft").await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("seed task")
        .expect("seed task");
    let task_id = task.id.clone();

    let (status, payload) = post_sync(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "active",
        // actor 显式传，但 name 字段缺失（serde default → ""）
        "actor": { "id": "SJ", "roles": "sj" },
        "next_step": { "assignee_id": "JH", "name": "JH", "roles": "jd" },
        "metadata": { "source": "pms-sync" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK, "{}", payload.message);
    assert_eq!(payload.code, 200);
    assert_eq!(payload.message, "success");
    let data = payload.data.expect("sync data");
    assert_eq!(data["task_id"], task_id);
    assert_eq!(data["current_node"], "jd");
    assert_eq!(data["task_status"], "submitted");
    assert_eq!(data["next_step"], "jd");
    assert_eq!(data["form_exists"], true);
    assert_eq!(data["form_status"], "active");
    assert_eq!(data["task_created"], true);

    #[derive(Debug, Deserialize)]
    struct HistoryRow {
        operator_id: Option<String>,
        operator_name: Option<String>,
        actor_name: Option<String>,
        source: Option<String>,
        action: Option<String>,
        target_node: Option<String>,
    }

    let mut response = test_db()
        .await
        .query(
            r#"
            SELECT operator_id, operator_name, actor_name, source, action, target_node,
                   <string> timestamp AS timestamp
            FROM review_workflow_history
            WHERE task_id = $task_id
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(("task_id", task_id))
        .await
        .expect("query workflow history");
    let rows: Vec<HistoryRow> = response.take(0).expect("take history rows");
    let row = rows.into_iter().next().expect("at least one history row");
    assert_eq!(row.operator_id.as_deref(), Some("SJ"));
    assert_eq!(
        row.operator_name.as_deref(),
        Some("SJ"),
        "operator_name 应当从空串兜底为 actor.id"
    );
    assert_eq!(row.actor_name.as_deref(), Some("SJ"));
    assert_eq!(row.source.as_deref(), Some("pms-sync"));
    assert_eq!(row.action.as_deref(), Some("submit"));
    assert_eq!(row.target_node.as_deref(), Some("jd"));

    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task after sync")
        .expect("task after sync");
    assert_eq!(task_after.current_node, "jd");
    assert_eq!(task_after.status, "submitted");
    assert_eq!(task_after.checker_id, "JH");

    cleanup_form(form_id).await;
}

// 3.7 六条入站路径在真实 router 上都不是 404（计划 §6 A8）。
// `route_guard` 钉的是源码文本，这里钉运行时：上面的 oneshot 只实跑了 embed-url /
// workflow/sync / workflow/verify 三条，另外三条一旦漏挂，PMS 侧只会看到静默 404。
#[tokio::test]
async fn all_six_platform_paths_are_reachable_not_404() {
    // 故意用一枚不合法的 S2S token：够不到业务分支，但足以证明请求进了 handler。
    let body = serde_json::json!({
        "token": "__invalid_s2s_token__",
        "project_id": "P-ROUTE-PROBE",
        "initiator": "route-probe",
        "form_ids": [],
        "operator_id": "route-probe",
        "form_id": "FORM-ROUTE-PROBE",
        "user_id": "route-probe",
        "action": "active"
    });

    for path in [
        "/api/review/embed-url",
        "/api/review/workflow/sync",
        "/api/review/workflow/verify",
        "/api/review/annotations/check",
        "/api/review/cache/preload",
        "/api/review/delete",
    ] {
        let (status, _) = post_json(path, body.clone()).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{path} 未注册，PMS 会静默 404"
        );
        assert_ne!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} 注册了但不收 POST"
        );
    }

    // 反证：断言不是恒真——router 之外的路径确实回 404。
    let (status, _) = post_json("/api/review/not-a-real-endpoint", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
