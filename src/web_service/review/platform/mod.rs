//! 校审域 · PMS 平台入站（`POST /api/review/{embed-url,workflow/sync,workflow/verify,annotations/check,cache/preload,delete}`）。
//! 旧端 `src/web_api/platform_api/*`。目录归 T3（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）；
//! T1 只建空壳。
//!
//! ⚠ 下方「T2 编译脚手架」段是 T2（t-614）在 T3 无人持有期间为让 crate 可编译临时放的 **7 个契约符号**：
//! 签名按旧端 `platform_api/{review_form.rs:22,215,262, types.rs:478, annotation_check.rs:94,137,146,328}` 逐字，
//! 实现是最小占位（不含批注闸门逻辑），**T3 接手后整段覆盖为真实实现**。
//! `tasks.rs` / `sync.rs` 只依赖这些签名，不依赖占位行为。

use axum::Router;

use crate::web_service::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
}

// ============================================================================
// T2 编译脚手架 —— T3 提供（旧端 platform_api/review_form.rs、types.rs）
// ============================================================================

/// `review_tasks` 软删过滤子句（旧端 `review_form.rs:22`）。
pub const REVIEW_TASK_ACTIVE_SQL: &str = "(deleted IS NONE OR deleted = false)";

/// 任务状态 → review_forms 状态（旧端 `types.rs:478`）。
pub fn derive_review_form_status_from_task_status(task_status: &str) -> String {
    match task_status.trim().to_lowercase().as_str() {
        "draft" => "draft".to_string(),
        "cancelled" => "cancelled".to_string(),
        "approved" => "approved".to_string(),
        _ => "active".to_string(),
    }
}

/// 占位：T3 提供（旧端 `review_form.rs:215`，写 review_forms 存根并同步状态）。
pub async fn sync_review_form_with_task_status(
    form_id: &str,
    project_id: Option<&str>,
    requester_id: Option<&str>,
    source: &str,
    task_status: &str,
) -> anyhow::Result<()> {
    log::warn!(
        "[REVIEW.platform.stub] sync_review_form_with_task_status form_id={form_id} project_id={project_id:?} requester_id={requester_id:?} source={source} task_status={task_status} —— T3 未落地，未写 review_forms"
    );
    Ok(())
}

/// 占位：T3 提供（旧端 `review_form.rs:262`，软删 review_forms）。
pub async fn mark_review_form_deleted(form_id: &str) -> anyhow::Result<()> {
    log::warn!(
        "[REVIEW.platform.stub] mark_review_form_deleted form_id={form_id} —— T3 未落地，未写 review_forms"
    );
    Ok(())
}

/// T2 编译脚手架 —— T3 提供（旧端 `platform_api/annotation_check.rs`）。
pub mod annotation_check {
    use axum::http::StatusCode;
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    pub struct AnnotationCheckResponse {
        pub success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<AnnotationCheckResult>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub error_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub error_message: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct AnnotationCheckResult {
        pub passed: bool,
        pub recommended_action: String,
        pub current_node: String,
        pub summary: AnnotationCheckSummary,
        pub blockers: Vec<AnnotationCheckBlocker>,
        pub message: String,
    }

    #[derive(Debug, Clone, Serialize, Default)]
    pub struct AnnotationCheckSummary {
        pub total: usize,
        pub open: usize,
        pub pending_review: usize,
        pub approved: usize,
        pub rejected: usize,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct AnnotationCheckBlocker {
        pub annotation_id: String,
        pub annotation_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub state_code: String,
        pub state_label: String,
        pub refnos: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub updated_at: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub updated_by_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub updated_by_role: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub note: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct AnnotationCheckContext {
        pub task_id: String,
        pub form_id: String,
        pub current_node: String,
    }

    #[derive(Debug, Clone)]
    pub struct AnnotationCheckOptions {
        pub current_node: Option<String>,
        pub intent: Option<String>,
        pub included_types: Option<Vec<String>>,
    }

    /// 旧端 `annotation_check.rs:137`。
    pub fn annotation_check_failed_response(result: AnnotationCheckResult) -> AnnotationCheckResponse {
        AnnotationCheckResponse {
            success: false,
            error_code: Some("ANNOTATION_CHECK_FAILED".to_string()),
            error_message: Some(result.message.clone()),
            data: Some(result),
        }
    }

    /// 旧端 `annotation_check.rs:146`。
    pub fn build_annotation_check_context(
        task_id: impl Into<String>,
        form_id: impl Into<String>,
        current_node: impl Into<String>,
    ) -> AnnotationCheckContext {
        AnnotationCheckContext {
            task_id: task_id.into(),
            form_id: form_id.into(),
            current_node: current_node.into(),
        }
    }

    /// 占位：T3 提供（旧端 `annotation_check.rs:328`，≈700 行批注闸门）。
    /// 未落地前显式报 501，不假装「通过」（宪法 III：不静默）。
    pub async fn evaluate_annotation_check(
        context: &AnnotationCheckContext,
        _options: AnnotationCheckOptions,
    ) -> Result<AnnotationCheckResult, (StatusCode, String)> {
        Err((
            StatusCode::NOT_IMPLEMENTED,
            format!(
                "批注闸门（annotations/check）尚未移植（T3）：task_id={} form_id={} current_node={}",
                context.task_id, context.form_id, context.current_node
            ),
        ))
    }
}
