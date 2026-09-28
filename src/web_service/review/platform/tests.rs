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
//! | `test_workflow_verify_blank_form_without_task_returns_soft_block` | handler 对 `active` + 空白主单据有显式放行分支（「表单尚未创建 task，active 可继续」） | 移植时曾断言 200 passed=true；**2026-09-29 起改回软阻断**：verify 是 sync 的干跑，`sync active` 对同一状态本来就 409，放行分支删掉，断言 passed=false + `TASK_NOT_CREATED`（计划 `2026-09-28-review-workflow-defects-fix-plan.md` D3 / T3） |
//! | `test_workflow_verify_active_on_non_sj_node_returns_soft_block` | verify 的节点闸（`ensure_action_allowed_on_node`）不带 `task_status` / `next_step` | 这两格为 None，其余不变 |
//! | `test_workflow_verify_missing_form_returns_404` | `active` + 主单据不存在也走上面那条放行分支 | 移植时改用 `agree` 复现 404、另钉 `active` 放行；**2026-09-29 起**放行分支删掉，`active` 同样 404 |
//! | `test_workflow_verify_agree_owner_mismatch_returns_structured_diagnostics` | 外部驱动模式（claims 无 `workflow_mode`）**跳过** owner 校验（单测 `external_workflow_verify_skips_owner_match` 同口径） | 断言外部模式放行；OWNER_MISMATCH 由 `workflow_sync::tests` 的内部模式单测覆盖 |
//! | `test_workflow_verify_agree_returns_annotation_gate_block_without_mutation` | verify 不读 `next_step`，诊断里 `next_step` / `requested_next_step` 为 None | 这两格为 None，其余不变 |
//! | `test_verify_active_blocks_when_open_exists` / `test_verify_return_blocks_when_no_problem` | verify 对 `active` / `return` **不跑**批注闸门（`verify_action_intent_for_node`） | 断言放行。**2026-09-17 c8d5f6b92 之后 `active` 那一半改回来**：`annotation_check_intent_for_action` 把 `(Active, _)` 收进 ActiveSubmit，verify 带 open 批注 → 200 + passed=false + `ANNOTATION_CHECK_FAILED`（3.6.1）；`return` 照旧放行 |
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
        // 夹具就是一台会蒸发的 `mem://`，而持久化闸（ADR-075）拒的正是 `mem`。介质的默认档 2026-09-18
        // 起翻成了 rocksdb（ADR-083 D6），所以不设环境变量跑测试时闸并不拒；但会话里只要有
        // `AIOS_STORE_MODE=mem` 就会撞上，而这个默认档已经翻过两次——夹具不押在它身上，照旧显式
        // 声明「知道会丢」，走测试专用开关而不是 set_var（d-410）。
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

/// 旧名 `test_workflow_verify_blank_form_without_task_returns_soft_block`（再往前一版曾断言
/// 「active + 空白主单据放行」）。2026-09-29 起 verify 是 sync 的干跑：`active` 对尚未建 task 的
/// 主单据回 `passed=false, block_code=TASK_NOT_CREATED`——同一状态下 `sync active` 本来就 409
/// （`docs/plans/2026-09-28-review-workflow-defects-fix-plan.md` D3 / T3，Q1=A）。
#[tokio::test]
async fn test_workflow_verify_blank_form_without_task_soft_blocks_active() {
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
    assert!(!data.passed, "空白主单据的 active 不再放行");
    assert_eq!(data.action, "active");
    assert_eq!(data.block_code.as_deref(), Some("TASK_NOT_CREATED"));
    assert_eq!(data.task_status.as_deref(), Some("blank"));
    assert_eq!(data.next_step, None);
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("表单尚未保存编校审数据"), "{}", data.reason);

    // 非 active 动作对空白主单据同样 soft block，同一 block_code。
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
    assert_eq!(data.block_code.as_deref(), Some("TASK_NOT_CREATED"));
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.task_status.as_deref(), Some("blank"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("尚未创建活动 review task"));

    cleanup_form(form_id).await;
}

/// V5 对拍：同一夹具、同一 (form_id, action, actor) 跑 verify 与 sync，`verify.passed` 必须等于
/// `sync` 是否 2xx——差别只在 verify 不写库。三格：空白主单据 active（都挡）、不存在的主单据
/// active（都 404）、sj 草稿 active（都过；sync 之后任务到 jd，verify 之前任务不动）。
#[tokio::test]
async fn test_workflow_verify_matches_sync_outcome_for_active() {
    let actor = serde_json::json!({ "id": "SJ", "name": "SJ", "roles": "sj" });
    let next_step = serde_json::json!({ "assignee_id": "JH", "name": "JH", "roles": "jd" });
    let body = |form_id: &str| {
        serde_json::json!({
            "form_id": form_id,
            "token": s2s_token("SJ", "sj"),
            "action": "active",
            "actor": actor,
            "next_step": next_step
        })
    };

    // 1. 主单据存在、未建 task
    let blank_form = "FORM-PARITY-BLANK";
    cleanup_form(blank_form).await;
    seed_form_stub(blank_form, "SJ", Some("sj")).await;
    let (verify_status, verify) = post_verify(body(blank_form)).await;
    let (sync_status, sync) = post_sync(body(blank_form)).await;
    assert_eq!(verify_status, StatusCode::OK);
    let verify_data = verify.data.expect("verify data");
    assert!(!verify_data.passed);
    assert_eq!(sync_status, StatusCode::CONFLICT, "{}", sync.message);
    assert_eq!(verify_data.passed, sync_status.is_success());
    assert!(sync.message.contains("尚未创建活动 review task"), "{}", sync.message);
    assert!(
        super::review_form::find_task_by_form_id(blank_form)
            .await
            .expect("query")
            .is_none(),
        "两条路都不该建 task"
    );
    cleanup_form(blank_form).await;

    // 2. 主单据不存在
    let missing_form = "FORM-PARITY-MISSING";
    cleanup_form(missing_form).await;
    let (verify_status, verify) = post_verify(body(missing_form)).await;
    let (sync_status, _sync) = post_sync(body(missing_form)).await;
    assert_eq!(verify_status, StatusCode::NOT_FOUND);
    assert!(verify.data.is_none());
    assert_eq!(sync_status, StatusCode::NOT_FOUND);

    // 3. sj 草稿：verify 过且不动库，sync 过且推进到 jd
    let draft_form = "FORM-PARITY-DRAFT";
    insert_task_seed(draft_form, "SJ", "JH", "SH", "sj", "draft").await;
    seed_form_stub(draft_form, "SJ", Some("sj")).await;
    let (verify_status, verify) = post_verify(body(draft_form)).await;
    assert_eq!(verify_status, StatusCode::OK);
    let verify_data = verify.data.expect("verify data");
    assert!(verify_data.passed, "{}", verify_data.reason);
    let before = super::review_form::find_task_by_form_id(draft_form)
        .await
        .expect("query")
        .expect("task");
    assert_eq!(before.current_node, "sj");
    let (sync_status, sync) = post_sync(body(draft_form)).await;
    assert_eq!(sync_status, StatusCode::OK, "{}", sync.message);
    assert_eq!(verify_data.passed, sync_status.is_success());
    let after = super::review_form::find_task_by_form_id(draft_form)
        .await
        .expect("query")
        .expect("task");
    assert_eq!(after.current_node, "jd");
    cleanup_form(draft_form).await;
}

/// V6：external 模式（S2S JWT 不带 `workflow_mode`）verify 通过时也回填 `actor_id / owner_id /
/// owner_source`——只是诊断，判定仍不校 owner（非负责人 JH 对 sj 节点 active 照样 passed）。
/// 旧端这三格恒空，PMS「目标人预选」拿不到平台侧负责人（计划 D4 / T4）。
#[tokio::test]
async fn test_workflow_verify_pass_carries_owner_diagnostics_in_external_mode() {
    let form_id = "FORM-VERIFY-OWNER-DIAG";
    insert_task_seed(form_id, "SJ", "JH", "SH", "sj", "draft").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "active",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "external 模式不校 owner：{}", data.reason);
    assert_eq!(data.actor_id.as_deref(), Some("JH"));
    assert_eq!(data.owner_id.as_deref(), Some("SJ"));
    assert_eq!(data.owner_source.as_deref(), Some("requester"));
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.next_step.as_deref(), Some("jd"));

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

/// 旧名 `test_workflow_verify_missing_form_returns_404`。主单据不存在时任何 action 都 404
/// （2026-09-29 之前 `active` 走「空白放行」分支被放过去，见 D3 / T3）。
#[tokio::test]
async fn test_workflow_verify_missing_form_returns_404_for_every_action() {
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
    assert_eq!(status, StatusCode::NOT_FOUND, "active 对不存在的主单据不再按空白放行");
    assert_eq!(payload.code, 404);
    assert!(payload.data.is_none());
    assert!(payload.message.contains("未找到 review form"));
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
/// 旧名 `test_workflow_verify_agree_external_mode_skips_owner_match`（移植时曾断言 external 放行）。
/// 2026-09-29 Q6=A 起 external 模式 `agree` / `return` 也校 owner：jd 节点负责人是 JH（上一步 PMS
/// `next_step` 写进 `checker_id` 的人），SH 来 agree → 200 + passed=false + OWNER_MISMATCH，诊断带
/// `actor_id / owner_id / owner_source`；同参数 `sync agree` 403 且任务不动
/// （`docs/plans/2026-09-28-review-workflow-defects-fix-plan.md` D4 / D8 / T7）。
#[tokio::test]
async fn test_workflow_agree_external_mode_rejects_non_owner() {
    let form_id = "FORM-VERIFY-OWNER-MISMATCH";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    let body = serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SH", "sh"),
        "action": "agree",
        "actor": { "id": "SH", "name": "SH", "roles": "sh" },
        "next_step": { "assignee_id": "PZ", "name": "PZ", "roles": "pz" }
    });

    let (status, payload) = post_verify(body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload.code, 200);
    let data = payload.data.expect("verify data");
    assert!(!data.passed, "external agree by a non-owner must be blocked: {data:?}");
    assert_eq!(data.action, "agree");
    assert_eq!(data.block_code.as_deref(), Some("OWNER_MISMATCH"));
    assert_eq!(data.current_node.as_deref(), Some("jd"));
    assert_eq!(data.task_status.as_deref(), Some("submitted"));
    assert_eq!(data.actor_id.as_deref(), Some("SH"));
    assert_eq!(data.owner_id.as_deref(), Some("JH"));
    assert_eq!(data.owner_source.as_deref(), Some("checker"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("权限不足"), "{}", data.reason);

    let (status, sync) = post_sync(body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{}", sync.message);
    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(task_after.current_node, "jd");
    assert_eq!(task_after.status, "submitted");

    // 负责人本人 JH 照常通过
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(data.passed, "{}", data.reason);
    assert_eq!(data.owner_id.as_deref(), Some("JH"));

    cleanup_form(form_id).await;
}

/// D8 / T7：请求体自报的 `actor.id` 必须是 token 持有人。持 SJ 的嵌入 token 冒 JH 来 agree → verify /
/// sync 都 403 ACTOR_MISMATCH，任务不动；PMS 按人领 token 再送审的现网口径不受影响（上一条用例）。
#[tokio::test]
async fn test_workflow_actor_must_match_token_holder() {
    let form_id = "FORM-ACTOR-MISMATCH";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    let body = serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    });

    let (status, verify) = post_verify(body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(verify.code, 403);
    assert_eq!(verify.error_code.as_deref(), Some("ACTOR_MISMATCH"));
    assert!(verify.data.is_none());
    assert!(verify.message.contains("JH") && verify.message.contains("SJ"), "{}", verify.message);

    let (status, sync) = post_sync(body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(sync.error_code.as_deref(), Some("ACTOR_MISMATCH"));
    let task_after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(task_after.current_node, "jd");

    // actor.id 与 token 持有人只差大小写 → 同一人（HumanCode 统一按大写比）
    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "jh", "name": "JH", "roles": "jd" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{}", payload.message);
    assert!(payload.data.expect("verify data").passed);

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

/// 只有 obb 批注的确认记录（`obb_annotations` 有一条 open，其余三类为空）。
async fn insert_obb_only_record(task_id: &str, form_id: &str, annotation_id: &str) {
    test_db()
        .await
        .query(
            r#"
            CREATE ONLY type::thing('review_records', $id) CONTENT {
                task_id: $task_id,
                form_id: $form_id,
                type: "batch",
                annotations: [],
                cloud_annotations: [],
                rect_annotations: [],
                obb_annotations: [
                    {
                        id: $annotation_id,
                        annotationType: "obb",
                        title: "箱体批注",
                        refnos: ["24381/145018"],
                        reviewState: NONE
                    }
                ],
                measurements: [],
                note: "seed obb only",
                current_node: "jd",
                operator_id: "JH",
                operator_name: "JH",
                slot_key: "slot-obb-only",
                snapshot_hash: "hash-obb-only",
                confirmed_at: time::now()
            }
            "#,
        )
        .bind(("id", format!("record-{}-obb-only", form_id.to_lowercase())))
        .bind(("task_id", task_id.to_string()))
        .bind(("form_id", form_id.to_string()))
        .bind(("annotation_id", annotation_id.to_string()))
        .await
        .expect("seed obb record")
        .check()
        .expect("seed obb record statements");
}

/// T6（D7，Q7=A）：只画了 OBB 批注并留着 open 时，`agree` 必须被闸门挡下、blocker 类型是 `obb`；
/// `annotations/check` 显式 `included_types:["obb"]` 也不再 400。旧闸门 `SUPPORTED_ANNOTATION_TYPES`
/// 没有 obb，这一格 agree 照样放行。
#[tokio::test]
async fn test_obb_only_open_annotation_blocks_agree() {
    let form_id = "FORM-OBB-ONLY-GATE";
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    insert_obb_only_record(&task.id, form_id, "ann-obb-open-1").await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(!data.passed, "obb open 批注必须挡住 agree：{}", data.reason);
    assert_eq!(data.block_code.as_deref(), Some("ANNOTATION_CHECK_FAILED"));
    let annotation_check = payload.annotation_check.expect("annotation_check");
    assert_eq!(annotation_check["summary"]["total"], 1);
    assert_eq!(annotation_check["summary"]["open"], 1);
    assert_eq!(annotation_check["blockers"][0]["annotation_id"], "ann-obb-open-1");
    assert_eq!(annotation_check["blockers"][0]["annotation_type"], "obb");

    // 独立闸门接口：显式只查 obb 也认
    let (status, bytes) = post_json(
        "/api/review/annotations/check",
        serde_json::json!({
            "form_id": form_id,
            "token": s2s_token("JH", "jd"),
            "intent": "agree_advance",
            "included_types": ["obb"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let check: serde_json::Value = serde_json::from_slice(&bytes).expect("check body");
    assert_eq!(check["success"], true);
    assert_eq!(check["data"]["passed"], false);
    assert_eq!(check["data"]["summary"]["open"], 1);

    cleanup_form(form_id).await;
}

/// D5b（T7）：闸门取记录与 `workflow/sync query` 同口径——按 form_id 取，兼收 task_id。form 下有两条
/// 活动 task、记录挂在旧 task 上时，对 active task 的 agree 仍看得见那条 open 批注。
#[tokio::test]
async fn test_annotation_gate_sees_records_hung_on_sibling_task_of_same_form() {
    let form_id = "FORM-GATE-SIBLING-TASK";
    // 旧 task（记录挂它）+ 新 task（active，updated_at 更新）
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    test_db()
        .await
        .query(
            r#"
            UPDATE type::thing('review_tasks', $old_id) SET updated_at = time::now() - 1h;
            CREATE ONLY type::thing('review_tasks', $new_id) SET
                form_id = $form_id, title = 'newer', description = '', model_name = 'demo-model',
                status = 'submitted', priority = 'medium', requester_id = 'SJ', requester_name = 'SJ',
                checker_id = 'JH', checker_name = 'JH', approver_id = 'SH', approver_name = 'SH',
                reviewer_id = 'JH', reviewer_name = 'JH', components = [], attachments = NONE,
                current_node = 'jd', workflow_history = [],
                created_at = time::now(), updated_at = time::now();
            "#,
        )
        .bind(("old_id", format!("task-{}", form_id.to_lowercase())))
        .bind(("new_id", format!("task-{}-newer", form_id.to_lowercase())))
        .bind(("form_id", form_id.to_string()))
        .await
        .expect("seed sibling task")
        .check()
        .expect("seed sibling task statements");
    let active = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(active.id, format!("task-{}-newer", form_id.to_lowercase()));
    insert_review_record_with_state(
        &format!("task-{}", form_id.to_lowercase()),
        form_id,
        "ann-sibling-open-1",
        "挂在旧 task 上的未处理批注",
        None,
        None,
        "sibling-open",
    )
    .await;

    let (status, payload) = post_verify(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("JH", "jd"),
        "action": "agree",
        "actor": { "id": "JH", "name": "JH", "roles": "jd" },
        "next_step": { "assignee_id": "SH", "name": "SH", "roles": "sh" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = payload.data.expect("verify data");
    assert!(!data.passed, "记录挂在同 form 的旧 task 上也必须进闸门：{}", data.reason);
    assert_eq!(data.block_code.as_deref(), Some("ANNOTATION_CHECK_FAILED"));
    assert_eq!(
        payload.annotation_check.expect("annotation_check")["blockers"][0]["annotation_id"],
        "ann-sibling-open-1"
    );

    cleanup_form(form_id).await;
}

// 3.6.1：verify 对 active 走 ActiveSubmit 批注闸门（c8d5f6b92，2026-09-17：`annotation_check_intent_for_action`
// 把 `(Active, _)` 收进 ActiveSubmit，verify 与 sync 共用一张表——此前 Active => None 被整个跳过，JH 驳回后
// SJ 不回复新批注就能原样重提）。带一条 open（没人回过的）批注 → soft block：200 + passed=false +
// ANNOTATION_CHECK_FAILED，任务留在 sj、诊断指向 jd。只卡 open：fixed / pending / rejected 都算回过，见 3.6.2。
// （移植时曾按当时 handler 的行为断言成「不跑闸门、放行」，c8d5f6b92 之后那条断言落后于 handler，改回来。）
#[tokio::test]
async fn test_verify_active_blocks_when_open_exists() {
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
    assert_eq!(payload.code, 200);
    assert_eq!(
        payload.error_code.as_deref(),
        Some("ANNOTATION_CHECK_FAILED")
    );
    let data = payload.data.expect("verify data");
    assert!(!data.passed, "{data:?}");
    assert_eq!(data.action, "active");
    assert_eq!(data.block_code.as_deref(), Some("ANNOTATION_CHECK_FAILED"));
    assert_eq!(data.current_node.as_deref(), Some("sj"));
    assert_eq!(data.task_status.as_deref(), Some("draft"));
    assert_eq!(data.expected_next_node.as_deref(), Some("jd"));
    assert_eq!(data.recommended_action, "block");
    assert!(data.reason.contains("未处理批注"), "{}", data.reason);
    assert!(payload.annotation_check.is_some());

    // verify 不落库：任务一字不动，仍在 sj / draft。
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    assert_eq!(task.current_node, "sj");
    assert_eq!(task.status, "draft");

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

// ============================================================================
// 设计端二次保存 / `review_forms.project_id` 护栏
// （docs/plans/2026-09-28-review-workflow-defects-fix-plan.md T1 / T2；验收 V1–V4）
// ============================================================================
//
// 这几条跨两张 router：`tasks::routes()`（设计端 `/api/review/tasks*`，Bearer JWT）与本模块的
// `/api/review/workflow/sync`（PMS 侧 S2S）——所以放在有 `mem://` 夹具的这里，而不是 `tasks.rs`。

fn tasks_app() -> Router<()> {
    super::super::tasks::routes::<()>()
}

/// 设计端 token（`review_auth_middleware` 校验的那枚；与 PMS S2S 同一密钥、同一 `create_token`）。
fn designer_token(project_id: &str, user_id: &str) -> String {
    create_token(project_id, user_id, Some(user_id), Some("sj"), None)
        .expect("designer token")
        .0
}

async fn tasks_request(
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    ensure_test_db().await;
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = tasks_app()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("tasks body: {e}: {}", String::from_utf8_lossy(&bytes)));
    (status, json)
}

fn component(refno: &str) -> serde_json::Value {
    serde_json::json!({
        "id": refno,
        "name": format!("构件 {refno}"),
        "refNo": refno,
        "type": "EQUI"
    })
}

fn create_task_body(
    form_id: &str,
    title: &str,
    description: &str,
    refnos: &[&str],
) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "description": description,
        "modelName": title,
        "checkerId": "JH",
        "approverId": "SH",
        "formId": form_id,
        "components": refnos.iter().map(|refno| component(refno)).collect::<Vec<_>>(),
    })
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn task_refnos(task: &serde_json::Value) -> Vec<String> {
    sorted(
        task["components"]
            .as_array()
            .expect("components array")
            .iter()
            .map(|component| component["refNo"].as_str().expect("refNo").to_string())
            .collect(),
    )
}

async fn take_strings(sql: &str, key: &str, value: &str) -> Vec<String> {
    let mut response = test_db()
        .await
        .query(sql)
        .bind((key.to_string(), value.to_string()))
        .await
        .unwrap_or_else(|e| panic!("query `{sql}`: {e}"));
    let rows: Vec<String> = response
        .take(0)
        .unwrap_or_else(|e| panic!("take `{sql}`: {e}"));
    sorted(rows)
}

/// `review_form_model` 里该 form 的构件集（PMS `workflow/sync` 的 `data.models` 读的就是它）。
async fn form_model_refnos(form_id: &str) -> Vec<String> {
    take_strings(
        "SELECT VALUE model_refno FROM review_form_model WHERE form_id = $form_id",
        "form_id",
        form_id,
    )
    .await
}

async fn history_actions(task_id: &str) -> Vec<String> {
    take_strings(
        "SELECT VALUE action FROM review_workflow_history WHERE task_id = $task_id",
        "task_id",
        task_id,
    )
    .await
}

async fn active_task_ids(form_id: &str) -> Vec<String> {
    take_strings(
        "SELECT VALUE record::id(id) FROM review_tasks WHERE form_id = $form_id AND (deleted IS NONE OR deleted = false)",
        "form_id",
        form_id,
    )
    .await
}

async fn task_attachment_ids(task_id: &str) -> Vec<String> {
    let mut response = test_db()
        .await
        .query("SELECT VALUE attachments FROM review_tasks WHERE record::id(id) = $task_id")
        .bind(("task_id", task_id.to_string()))
        .await
        .expect("query attachments");
    let rows: Vec<Option<Vec<serde_json::Value>>> = response.take(0).expect("take attachments");
    sorted(
        rows.into_iter()
            .flatten()
            .flatten()
            .filter_map(|attachment| attachment["id"].as_str().map(str::to_string))
            .collect(),
    )
}

async fn review_form_project_id(form_id: &str) -> Option<String> {
    super::review_form::get_review_form_by_form_id(form_id)
        .await
        .expect("query review_forms")
        .map(|form| form.project_id)
}

/// `cleanup_form` 不管 `review_form_model`，这里补上。
async fn cleanup_form_bundle(form_id: &str) {
    cleanup_form(form_id).await;
    let _ = test_db()
        .await
        .query(
            "LET $ids = SELECT VALUE id FROM review_form_model WHERE form_id = $form_id; DELETE $ids;",
        )
        .bind(("form_id", form_id.to_string()))
        .await;
}

/// V1：同 `form_id` 在 `sj` 节点二次保存 → 200、同一 task、字段与 `review_form_model` 都是新值；
/// `GET /api/review/tasks/{id}` 与 `workflow/sync action=query` 两条读路一致；历史是 `resave` 而不是
/// 旧端那条假的 `resubmit→jd`；请求不带 `attachments` 时保留旧附件。
#[tokio::test]
async fn test_create_task_resave_in_sj_updates_fields_and_form_models() {
    let form_id = "FORM-RESAVE-SJ";
    cleanup_form_bundle(form_id).await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    let token = designer_token("project-1", "SJ");

    let mut first_body = create_task_body(form_id, "包A", "第一次保存", &["24381/1", "24381/2"]);
    first_body["attachments"] = serde_json::json!([{
        "id": "att-1",
        "name": "a.png",
        "url": "/review_attachments/att-1.png",
        "size": 10,
        "mimeType": "image/png"
    }]);
    let (status, first) = tasks_request("POST", "/api/review/tasks", &token, Some(first_body)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["success"], true);
    let task_id = first["task"]["id"].as_str().expect("task id").to_string();
    assert_eq!(form_model_refnos(form_id).await, vec!["24381/1", "24381/2"]);
    assert_eq!(task_attachment_ids(&task_id).await, vec!["att-1"]);

    // 二次保存：改标题 / 描述 / 构件（含一个重复 refno），不带 attachments
    let (status, second) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token,
        Some(create_task_body(
            form_id,
            "包B",
            "第二次保存改了构件",
            &["24381/2", "24381/3", "24381/3"],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["success"], true);
    assert_eq!(second["task"]["id"], task_id.as_str(), "同 form_id 不新建任务");
    assert_eq!(second["task"]["title"], "包B");
    assert_eq!(second["task"]["modelName"], "包B");
    assert_eq!(second["task"]["description"], "第二次保存改了构件");
    assert_eq!(second["task"]["currentNode"], "sj");
    assert_eq!(second["task"]["status"], "draft");
    assert_eq!(second["task"]["checkerId"], "JH", "负责人字段不动");
    assert_eq!(
        task_refnos(&second["task"]),
        vec!["24381/2", "24381/3", "24381/3"]
    );
    assert_eq!(active_task_ids(form_id).await, vec![task_id.clone()]);
    assert_eq!(task_attachment_ids(&task_id).await, vec!["att-1"], "请求没带 attachments → 保留");

    // GET /api/review/tasks/{id} 读回的是新值
    let (status, fetched) =
        tasks_request("GET", &format!("/api/review/tasks/{task_id}"), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["task"]["title"], "包B");
    assert_eq!(
        task_refnos(&fetched["task"]),
        vec!["24381/2", "24381/3", "24381/3"]
    );

    // workflow/sync action=query 的 data.models 与之一致（review_form_model 已重刷、按 refno 去重）
    let (status, payload) = post_sync(serde_json::json!({
        "form_id": form_id,
        "token": s2s_token("SJ", "sj"),
        "action": "query",
        "actor": { "id": "SJ", "name": "SJ", "roles": "sj" }
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{}", payload.message);
    let data = payload.data.expect("sync data");
    let models: Vec<String> = serde_json::from_value(data["models"].clone()).expect("models");
    assert_eq!(sorted(models), vec!["24381/2", "24381/3"]);
    assert_eq!(data["task_id"], task_id.as_str());
    assert_eq!(data["current_node"], "sj");
    assert_eq!(form_model_refnos(form_id).await, vec!["24381/2", "24381/3"]);

    // 历史：恰好一条 resave；没有旧端那条假的 resubmit
    assert_eq!(history_actions(&task_id).await, vec!["resave"]);

    cleanup_form_bundle(form_id).await;
}

/// V2：任务已在 `jd`，二次保存 → 409 + 含当前节点中文名的原文；`review_tasks` /
/// `review_workflow_history` / `review_form_model` 一个字节不变。
#[tokio::test]
async fn test_create_task_resave_off_sj_node_is_rejected_without_writes() {
    let form_id = "FORM-RESAVE-JD";
    cleanup_form_bundle(form_id).await;
    insert_task_seed(form_id, "SJ", "JH", "SH", "jd", "submitted").await;
    let task_id = format!("task-{}", form_id.to_lowercase());
    let token = designer_token("project-1", "SJ");

    let (status, payload) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token,
        Some(create_task_body(form_id, "改了标题", "改了描述", &["24381/9"])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{payload}");
    assert_eq!(payload["success"], false);
    assert!(payload.get("task").is_none(), "409 不回任务：{payload}");
    let message = payload["error_message"].as_str().expect("error_message");
    assert!(message.contains("jd") && message.contains("校对"), "{message}");
    assert!(message.contains("不可修改"), "{message}");

    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("seeded task");
    assert_eq!(task.id, task_id);
    assert_eq!(task.title, format!("Task for {form_id}"));
    assert_eq!(task.description, "seeded verify task");
    assert!(task.components.is_empty());
    assert_eq!(task.current_node, "jd");
    assert_eq!(task.status, "submitted");
    assert_eq!(active_task_ids(form_id).await, vec![task_id.clone()]);
    assert!(history_actions(&task_id).await.is_empty());
    assert!(form_model_refnos(form_id).await.is_empty());

    cleanup_form_bundle(form_id).await;
}

/// G5：`PATCH /api/review/tasks/{id}` 改 `components` 后 `review_form_model` 与之一致（按 refno 去重）。
#[tokio::test]
async fn test_update_task_components_refreshes_review_form_model() {
    let form_id = "FORM-PATCH-MODELS";
    cleanup_form_bundle(form_id).await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    let token = designer_token("project-1", "SJ");

    let (status, created) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token,
        Some(create_task_body(form_id, "包C", "PATCH 前", &["24381/1"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let task_id = created["task"]["id"].as_str().expect("task id").to_string();
    assert_eq!(form_model_refnos(form_id).await, vec!["24381/1"]);

    let (status, patched) = tasks_request(
        "PATCH",
        &format!("/api/review/tasks/{task_id}"),
        &token,
        Some(serde_json::json!({
            "components": [component("24381/5"), component("24381/6"), component("24381/6")]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["success"], true);
    assert!(
        patched.get("error_message").is_none(),
        "同步 review_form_model 不该报错：{patched}"
    );
    assert_eq!(
        task_refnos(&patched["task"]),
        vec!["24381/5", "24381/6", "24381/6"]
    );
    assert_eq!(form_model_refnos(form_id).await, vec!["24381/5", "24381/6"]);

    // 不带 components 的 PATCH 不碰 review_form_model
    let (status, patched_title) = tasks_request(
        "PATCH",
        &format!("/api/review/tasks/{task_id}"),
        &token,
        Some(serde_json::json!({ "title": "只改标题" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched_title}");
    assert_eq!(form_model_refnos(form_id).await, vec!["24381/5", "24381/6"]);

    cleanup_form_bundle(form_id).await;
}

/// V3：两条同 `form_id` 的活动任务，`create_task` 去重与 `find_task_by_form_id` 选中同一条
/// （`updated_at DESC`；旧去重口径 `created_at ASC` 会选到另一条）。
#[tokio::test]
async fn test_create_task_dedup_and_find_task_by_form_id_pick_the_same_task() {
    let form_id = "FORM-DEDUP-ORDER";
    cleanup_form_bundle(form_id).await;
    seed_form_stub(form_id, "SJ", Some("sj")).await;
    test_db()
        .await
        .query(
            r#"
            CREATE ONLY type::thing('review_tasks', 'task-dedup-older') SET
                form_id = $form_id, title = 'older', description = '', model_name = 'older',
                status = 'draft', priority = 'medium', requester_id = 'SJ', requester_name = 'SJ',
                checker_id = 'JH', checker_name = 'JH', approver_id = 'SH', approver_name = 'SH',
                reviewer_id = 'JH', reviewer_name = 'JH', components = [], attachments = NONE,
                current_node = 'sj', workflow_history = [],
                created_at = time::now() - 2h, updated_at = time::now() - 2h;
            CREATE ONLY type::thing('review_tasks', 'task-dedup-newer') SET
                form_id = $form_id, title = 'newer', description = '', model_name = 'newer',
                status = 'draft', priority = 'medium', requester_id = 'SJ', requester_name = 'SJ',
                checker_id = 'JH', checker_name = 'JH', approver_id = 'SH', approver_name = 'SH',
                reviewer_id = 'JH', reviewer_name = 'JH', components = [], attachments = NONE,
                current_node = 'sj', workflow_history = [],
                created_at = time::now() - 1h, updated_at = time::now() - 1h;
            "#,
        )
        .bind(("form_id", form_id.to_string()))
        .await
        .expect("seed duplicate tasks")
        .check()
        .expect("seed duplicate tasks statements");

    let lookup = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(lookup.id, "task-dedup-newer");

    let token = designer_token("project-1", "SJ");
    let (status, saved) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token,
        Some(create_task_body(form_id, "resaved", "", &["24381/7"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["task"]["id"], "task-dedup-newer",
        "去重与 find_task_by_form_id 同源"
    );
    assert_eq!(
        active_task_ids(form_id).await,
        vec!["task-dedup-newer", "task-dedup-older"],
        "不新建第三条"
    );

    let after = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(after.id, "task-dedup-newer");
    assert_eq!(after.title, "resaved");
    let older = take_strings(
        "SELECT VALUE title FROM review_tasks WHERE record::id(id) = $id",
        "id",
        "task-dedup-older",
    )
    .await;
    assert_eq!(older, vec!["older"], "另一条一个字节没动");

    cleanup_form_bundle(form_id).await;
}

/// V4：embed-url 建存根写入的 `review_forms.project_id`，在设计端首次保存（含 claims 无项目号的
/// 「保留」分支）、二次保存、内部 `submit` 之后全程不变；`modelName`（数据包名称）不再流向它。
#[tokio::test]
async fn test_review_forms_project_id_survives_create_task_and_internal_submit() {
    let form_id = "FORM-PROJECT-ID-GUARD";
    cleanup_form_bundle(form_id).await;
    super::review_form::ensure_review_form_stub(form_id, "P-REAL", "SJ", Some("sj"), "pms")
        .await
        .expect("embed-url stub");
    assert_eq!(
        review_form_project_id(form_id).await.as_deref(),
        Some("P-REAL")
    );

    // 首次保存：claims 没带项目号 → 传空串 → 保留存根里的值
    let token_without_project = designer_token("", "SJ");
    let (status, created) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token_without_project,
        Some(create_task_body(form_id, "某数据包名称", "", &["24381/1"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["task"]["modelName"], "某数据包名称");
    assert_eq!(
        review_form_project_id(form_id).await.as_deref(),
        Some("P-REAL"),
        "数据包名称不得流入 review_forms.project_id"
    );

    // 二次保存：claims 带同一项目号（embed-url 签发的 token 就是这样）
    let token = designer_token("P-REAL", "SJ");
    let (status, resaved) = tasks_request(
        "POST",
        "/api/review/tasks",
        &token,
        Some(create_task_body(form_id, "改名后的数据包", "", &["24381/1"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resaved}");
    assert_eq!(resaved["task"]["modelName"], "改名后的数据包");
    assert_eq!(
        review_form_project_id(form_id).await.as_deref(),
        Some("P-REAL")
    );

    // 内部流转 submit（sj → jd）走 apply_internal_workflow_mutation_transaction，同样保留
    let task_id = created["task"]["id"].as_str().expect("task id").to_string();
    let (status, submitted) = tasks_request(
        "POST",
        &format!("/api/review/tasks/{task_id}/submit"),
        &token,
        Some(serde_json::json!({ "comment": "送审" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    assert_eq!(
        review_form_project_id(form_id).await.as_deref(),
        Some("P-REAL")
    );
    let task = super::review_form::find_task_by_form_id(form_id)
        .await
        .expect("query task")
        .expect("task");
    assert_eq!(task.current_node, "jd");

    cleanup_form_bundle(form_id).await;
}
