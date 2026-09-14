//! `POST /api/review/embed-url`（旧端 `platform_api/embed_url.rs` 逐字搬入）：
//! PMS 换取带 `user_token` 的三维校审嵌入地址。
//!
//! 与旧端的差异：
//! - **Q5**：前端基址链里「站点注册表 `managed_project_sites::get_site(site_id).viewer_url`」那一级
//!   换成配置项 `[platform_auth].review_web_base_url`（[`PLATFORM_CONFIG`]），位置不变：
//!   `extra_parameters.frontend_base_url` → **review_web_base_url** → `Origin` / `Referer` →
//!   `PLATFORM_CONFIG.frontend_base_url`。
//! - `review-internal-workflow` 特性按计划 Q7 不移植：`workflow_mode` 只告警忽略、不进 claims。
//! - `reqwest::Url` → `url::Url`（同一 crate 的再导出）。

use axum::{
    extract::Json,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use log::{info, warn};

use super::super::auth::{Role, create_token, decode_token_unsafe, generate_form_id};
use super::auth::verify_s2s_token;
use super::config::PLATFORM_CONFIG;
use super::review_form::{ensure_review_form_stub, find_task_by_form_id};
use super::types::{
    EmbedLineage, EmbedUrlData, EmbedUrlQuery, EmbedUrlRequest, EmbedUrlResponse, ReviewFormSummary,
};

pub async fn get_embed_url(
    headers: HeaderMap,
    Json(request): Json<EmbedUrlRequest>,
) -> impl IntoResponse {
    let request_form_id = request
        .form_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    info!(
        "Embed URL request: project_id={}, user_id={}, form_id={:?}, workflow_role={:?}, workflow_mode={:?}, has_token={}, extra_parameters={}",
        request.project_id,
        request.user_id,
        request_form_id,
        request.workflow_role,
        request.workflow_mode,
        request
            .token
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        summarize_extra_parameters(request.extra_parameters.as_ref())
    );

    let inbound_token = request
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if let Err((_status, msg)) = verify_s2s_token(inbound_token) {
        warn!(
            "Embed URL S2S token verification failed: project_id={}, user_id={}, reason={}",
            request.project_id, request.user_id, msg
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(EmbedUrlResponse {
                code: 401,
                message: "unauthorized".to_string(),
                data: None,
                url: None,
            }),
        );
    }
    info!(
        "Embed URL S2S token accepted: project_id={}, user_id={}",
        request.project_id, request.user_id
    );

    info!(
        "Embed URL lineage resolution: request_form_id={:?}, token_claim_form_id={:?}, resolved_form_id={:?}",
        request_form_id, None::<String>, request_form_id
    );

    let requested_role = match resolve_embed_request_role(&request, None) {
        Ok(role) => role,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(EmbedUrlResponse {
                    code: 400,
                    message,
                    data: None,
                    url: None,
                }),
            );
        }
    };
    let requested_workflow_mode = resolve_embed_request_workflow_mode(&request);

    let form_id = request_form_id.clone().unwrap_or_else(generate_form_id);
    info!(
        "Embed URL resolved context: form_id={}, requested_role={:?}, requested_workflow_mode={:?}",
        form_id, requested_role, requested_workflow_mode
    );
    let ensured_form = match ensure_review_form_stub(
        &form_id,
        request.project_id.as_str(),
        request.user_id.as_str(),
        requested_role.as_deref(),
        "pms_embed",
    )
    .await
    {
        Ok(form) => form,
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("已删除") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            warn!(
                "Embed URL ensure_review_form_stub failed: form_id={}, project_id={}, user_id={}, status={}, error={}",
                form_id,
                request.project_id,
                request.user_id,
                status.as_u16(),
                message
            );
            return (
                status,
                Json(EmbedUrlResponse {
                    code: status.as_u16() as i32,
                    message,
                    data: None,
                    url: None,
                }),
            );
        }
    };

    let existing_task = match find_task_by_form_id(&form_id).await {
        Ok(task) => task,
        Err(e) => {
            warn!("Failed to load task for form_id={}: {}", form_id, e);
            None
        }
    };
    info!(
        "Embed URL form snapshot: form_id={}, form_status={}, task_created={}, existing_task_id={:?}, current_node={:?}, task_status={:?}",
        ensured_form.form_id,
        ensured_form.status,
        ensured_form.task_created,
        existing_task.as_ref().map(|t| t.id.as_str()),
        existing_task.as_ref().map(|t| t.current_node.as_str()),
        existing_task.as_ref().map(|t| t.status.as_str())
    );

    let resolved_role = requested_role.clone().or_else(|| ensured_form.role.clone());
    // plant3d-web `resolveTrustedEmbedIdentity` 要求 JWT claims 含非空 `role`。
    // 未传 workflow_role 且 review_forms 尚无 role 时，此前会签发无 role 的 JWT，嵌入页报「缺少可信身份声明」。
    let jwt_workflow_role: &str = resolved_role.as_deref().unwrap_or("sj");

    match create_token(
        &request.project_id,
        &request.user_id,
        None,
        Some(jwt_workflow_role),
        requested_workflow_mode.as_deref(),
    ) {
        Ok((token, _expires_at)) => {
            let is_reviewer = request
                .extra_parameters
                .as_ref()
                .and_then(|p| p.get("is_reviewer"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match decode_token_unsafe(&token) {
                Ok(generated_claims) => {
                    info!(
                        "Embed URL token generated: request_form_id={:?}, response_query_form_id={}, token_claim_form_id={:?}, token_project_id={}, token_user_id={}, token_role={:?}, token_workflow_mode={:?}, token_len={}",
                        request_form_id,
                        form_id,
                        generated_claims.legacy_form_id,
                        generated_claims.project_id,
                        generated_claims.user_id,
                        generated_claims.role,
                        generated_claims.workflow_mode,
                        token.len()
                    );
                }
                Err(error) => {
                    warn!(
                        "Embed URL token generated but decode_token_unsafe failed: response_query_form_id={}, error={}",
                        form_id, error
                    );
                }
            }

            let frontend_base_url = resolve_embed_frontend_base_url(&request, &headers);
            let full_url = build_embed_url(
                frontend_base_url.as_deref().unwrap_or_default(),
                PLATFORM_CONFIG.frontend_relative_path.as_str(),
                token.as_str(),
                request.project_id.as_str(),
            );
            info!(
                "Embed URL response ready: request_form_id={:?}, response_query_form_id={}, lineage_form_id={}, relative_path={}, frontend_base_url={}, url={}",
                request_form_id,
                form_id,
                form_id,
                PLATFORM_CONFIG.frontend_relative_path,
                frontend_base_url.as_deref().unwrap_or("<empty>"),
                summarize_url_for_log(full_url.as_deref()).unwrap_or_else(|| "<none>".to_string())
            );

            (
                StatusCode::OK,
                Json(EmbedUrlResponse {
                    code: 200,
                    message: "ok".to_string(),
                    data: Some(EmbedUrlData {
                        relative_path: PLATFORM_CONFIG.frontend_relative_path.clone(),
                        token,
                        query: EmbedUrlQuery {
                            form_id: form_id.clone(),
                            is_reviewer,
                        },
                        lineage: EmbedLineage {
                            form_id: form_id.clone(),
                            task_id: existing_task.as_ref().map(|t| t.id.clone()),
                            current_node: existing_task.as_ref().map(|t| t.current_node.clone()),
                            status: existing_task.as_ref().map(|t| t.status.clone()),
                        },
                        form: ReviewFormSummary {
                            form_id: form_id.clone(),
                            exists: true,
                            status: Some(ensured_form.status.clone()),
                            task_created: Some(ensured_form.task_created),
                        },
                        task: existing_task,
                    }),
                    url: full_url,
                }),
            )
        }
        Err(e) => {
            warn!("Token generation failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(EmbedUrlResponse {
                    code: -1,
                    message: format!("Token generation failed: {}", e),
                    data: None,
                    url: None,
                }),
            )
        }
    }
}

fn resolve_embed_request_role(
    request: &EmbedUrlRequest,
    verified_claim_role: Option<&str>,
) -> Result<Option<String>, String> {
    let explicit_role = request
        .workflow_role
        .as_deref()
        .or_else(|| extract_role_from_extra_parameters(request, "workflow_role"))
        .or_else(|| extract_role_from_extra_parameters(request, "role"));

    if let Some(role) = verified_claim_role {
        if let Some(explicit_role) = explicit_role {
            let validated_explicit = validate_embed_role(explicit_role)?;
            if validated_explicit.as_deref() != Some(role) {
                return Err("JWT workflow_role mismatch".to_string());
            }
        }
        return validate_embed_role(role);
    }

    if let Some(role) = explicit_role {
        return validate_embed_role(role);
    }

    Ok(None)
}

/// `review-internal-workflow` 不移植（计划 Q7）：旧端默认分支——收到 `workflow_mode` 只告警忽略。
fn resolve_embed_request_workflow_mode(request: &EmbedUrlRequest) -> Option<String> {
    let explicit = request
        .workflow_mode
        .as_deref()
        .or_else(|| extract_extra_parameter(request, "workflow_mode"))
        .or_else(|| extract_extra_parameter(request, "workflowMode"));
    if explicit
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        warn!("Ignoring embed workflow_mode because review-internal-workflow feature is disabled");
    }
    None
}

fn extract_extra_parameter<'a>(request: &'a EmbedUrlRequest, key: &str) -> Option<&'a str> {
    request
        .extra_parameters
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
}

fn extract_role_from_extra_parameters<'a>(
    request: &'a EmbedUrlRequest,
    key: &str,
) -> Option<&'a str> {
    extract_extra_parameter(request, key)
}

fn summarize_extra_parameters(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return "None".to_string();
    };

    match value.as_object() {
        Some(map) => {
            let mut items: Vec<String> = map
                .iter()
                .map(|(key, nested)| {
                    let normalized = key.to_lowercase();
                    if normalized.contains("token")
                        || normalized.contains("password")
                        || normalized.ends_with("key")
                    {
                        format!("{}=[redacted]", key)
                    } else {
                        format!("{}={}", key, summarize_json_value(nested))
                    }
                })
                .collect();
            items.sort();
            format!("{{{}}}", items.join(", "))
        }
        None => summarize_json_value(value),
    }
}

fn summarize_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(v) => v.to_string(),
        serde_json::Value::Number(v) => v.to_string(),
        serde_json::Value::String(v) => {
            if v.len() > 64 {
                format!("{}…({})", &v[..16], v.len())
            } else {
                v.clone()
            }
        }
        serde_json::Value::Array(v) => format!("[array:{}]", v.len()),
        serde_json::Value::Object(v) => format!("{{object:{}}}", v.len()),
    }
}

fn summarize_url_for_log(url: Option<&str>) -> Option<String> {
    let trimmed = url?.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut summarized = trimmed.to_string();
    if let Some(token_idx) = summarized.find("user_token=") {
        let tail = &summarized[token_idx + "user_token=".len()..];
        let token_end = tail.find('&').unwrap_or(tail.len());
        let token = &tail[..token_end];
        if !token.is_empty() {
            let redacted = format!(
                "{}...({})",
                token.chars().take(12).collect::<String>(),
                token.len()
            );
            summarized.replace_range(
                token_idx + "user_token=".len()..token_idx + "user_token=".len() + token_end,
                &redacted,
            );
        }
    }

    Some(summarized)
}

/// 前端基址取值链（旧端同名函数）；第二级由站点注册表换成配置项（Q5）。
fn resolve_embed_frontend_base_url(
    request: &EmbedUrlRequest,
    headers: &HeaderMap,
) -> Option<String> {
    frontend_base_url_from_extra_parameters(request)
        .or_else(configured_review_web_base_url)
        .or_else(|| frontend_origin_from_headers(headers))
        .or_else(|| normalize_frontend_base_url(PLATFORM_CONFIG.frontend_base_url.as_str()))
}

fn frontend_base_url_from_extra_parameters(request: &EmbedUrlRequest) -> Option<String> {
    let map = request.extra_parameters.as_ref()?.as_object()?;

    for key in ["frontend_base_url", "frontendBaseUrl"] {
        if let Some(value) = map.get(key).and_then(|value| value.as_str()) {
            if let Some(base) = normalize_frontend_base_url(value) {
                return Some(base);
            }
        }
    }

    for key in ["viewer_url", "viewerUrl", "frontend_url", "frontendUrl"] {
        if let Some(value) = map.get(key).and_then(|value| value.as_str()) {
            if let Some(base) = normalize_frontend_origin(value) {
                return Some(base);
            }
        }
    }

    None
}

/// Q5：`[platform_auth].review_web_base_url`，替代旧端 `managed_site_viewer_base_url`
/// （`web_server.site_id` → 站点注册表 `viewer_url`）。未配置即跳过这一级。
fn configured_review_web_base_url() -> Option<String> {
    PLATFORM_CONFIG
        .review_web_base_url
        .as_deref()
        .and_then(normalize_frontend_base_url)
}

fn frontend_origin_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_frontend_origin)
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|value| value.to_str().ok())
                .and_then(normalize_frontend_origin)
        })
}

fn normalize_frontend_base_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }

    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed).trim();
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .trim();

    if without_query.is_empty() {
        return None;
    }

    if let Ok(mut url) = url::Url::parse(without_query) {
        url.set_query(None);
        url.set_fragment(None);
        if url.path() == "/" {
            url.set_path("");
        }
        return Some(url.as_str().trim_end_matches('/').to_string());
    }

    Some(without_query.trim_end_matches('/').to_string()).filter(|value| !value.is_empty())
}

fn normalize_frontend_origin(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }

    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.starts_with("//") {
        format!("http:{}", trimmed)
    } else {
        format!("http://{}", trimmed)
    };

    let mut url = url::Url::parse(&candidate).ok()?;
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.as_str().trim_end_matches('/').to_string())
}

fn build_embed_url(
    base_url: &str,
    relative_path: &str,
    token: &str,
    project_id: &str,
) -> Option<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }

    let clean_path = if relative_path.starts_with('/') {
        relative_path.to_string()
    } else {
        format!("/{}", relative_path)
    };
    let encoded_token = urlencoding::encode(token);
    let encoded_project_id = urlencoding::encode(project_id);

    Some(format!(
        "{}{}?user_token={}&project_id={}&output_project={}",
        base, clean_path, encoded_token, encoded_project_id, encoded_project_id
    ))
}

fn validate_embed_role(raw_role: &str) -> Result<Option<String>, String> {
    let normalized = normalize_embed_role(Some(raw_role));
    if normalized.is_some() {
        return Ok(normalized);
    }

    Err(format!(
        "Invalid role: '{}'. Valid values are: {:?}",
        raw_role,
        Role::valid_values()
    ))
}

fn normalize_embed_role(role: Option<&str>) -> Option<String> {
    let trimmed = role.map(str::trim).filter(|value| !value.is_empty())?;
    Role::from_str(trimmed).map(|value| value.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::{build_embed_url, normalize_frontend_base_url, normalize_frontend_origin};

    #[test]
    fn build_embed_url_includes_project_scope() {
        let url = build_embed_url(
            "https://example.com/",
            "review/3d-view",
            "token-123",
            "Aveva Marine/Sample",
        )
        .expect("url");

        assert_eq!(
            url,
            "https://example.com/review/3d-view?user_token=token-123&project_id=Aveva%20Marine%2FSample&output_project=Aveva%20Marine%2FSample"
        );
    }

    /// Q5 配置项走 `normalize_frontend_base_url`：保留路径前缀、去掉 query / fragment / 尾斜杠；
    /// `Origin` 头走 `normalize_frontend_origin`：只留 origin。两条归一规则与旧端同一份。
    #[test]
    fn review_web_base_url_keeps_path_prefix_while_origin_drops_it() {
        assert_eq!(
            normalize_frontend_base_url("http://123.57.182.243/plant3d/?x=1#top"),
            Some("http://123.57.182.243/plant3d".to_string())
        );
        assert_eq!(
            normalize_frontend_base_url("http://localhost:5173/"),
            Some("http://localhost:5173".to_string())
        );
        assert_eq!(normalize_frontend_base_url("   "), None);
        assert_eq!(normalize_frontend_base_url("null"), None);
        assert_eq!(
            normalize_frontend_origin("http://localhost:5173/review/3d-view?a=b"),
            Some("http://localhost:5173".to_string())
        );
        assert_eq!(
            normalize_frontend_origin("192.168.31.58:5173"),
            Some("http://192.168.31.58:5173".to_string())
        );
    }
}
