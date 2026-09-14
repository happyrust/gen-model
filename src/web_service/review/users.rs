//! 校审域 · 用户（`GET /api/users`、`/api/users/me`、`/api/users/reviewers`）。
//!
//! 旧端 `src/web_api/review_api.rs`「用户 API」段逐字搬入：内置四个 PMS HumanCode 形态的演示用户
//! （SJ / JH / SH / PZ），`/me` 优先按 JWT claims 映射。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::get,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::common::*;

// ============================================================================
// Routes
// ============================================================================

/// 用户路由（旧端 `create_review_api_routes` 的「用户 API」段）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/users", get(list_users))
        .route("/api/users/me", get(get_current_user))
        .route("/api/users/reviewers", get(get_reviewers))
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Handlers - 用户 API
// ============================================================================

/// 用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub department: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserListResponse {
    pub success: bool,
    pub users: Vec<User>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserListQuery {
    pub role: Option<String>,
    pub status: Option<String>,
}

pub(super) fn build_mock_review_users() -> Vec<User> {
    vec![
        User {
            id: "SJ".to_string(),
            username: "SJ".to_string(),
            name: "王设计师".to_string(),
            email: "sj@company.com".to_string(),
            role: "designer".to_string(),
            department: Some("设计部".to_string()),
            avatar: None,
        },
        User {
            id: "JH".to_string(),
            username: "JH".to_string(),
            name: "张校对员".to_string(),
            email: "jh@company.com".to_string(),
            role: "proofreader".to_string(),
            department: Some("质量部".to_string()),
            avatar: None,
        },
        User {
            id: "SH".to_string(),
            username: "SH".to_string(),
            name: "李审核员".to_string(),
            email: "sh@company.com".to_string(),
            role: "reviewer".to_string(),
            department: Some("技术部".to_string()),
            avatar: None,
        },
        User {
            id: "PZ".to_string(),
            username: "PZ".to_string(),
            name: "陈经理".to_string(),
            email: "pz@company.com".to_string(),
            role: "manager".to_string(),
            department: Some("工程部".to_string()),
            avatar: None,
        },
    ]
}

pub(super) fn default_mock_user() -> User {
    build_mock_review_users()
        .into_iter()
        .find(|user| user.role == "designer")
        .unwrap_or(User {
            id: "SJ".to_string(),
            username: "SJ".to_string(),
            name: "王设计师".to_string(),
            email: "sj@company.com".to_string(),
            role: "designer".to_string(),
            department: Some("设计部".to_string()),
            avatar: None,
        })
}

pub(super) fn map_claim_role_to_user_role(role: Option<&str>) -> String {
    match role.unwrap_or("viewer") {
        "sj" => "designer".to_string(),
        "jd" => "proofreader".to_string(),
        "sh" => "reviewer".to_string(),
        "pz" => "manager".to_string(),
        "admin" => "admin".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn current_user_from_claims(claims: &super::auth::TokenClaims) -> User {
    build_mock_review_users()
        .into_iter()
        .find(|user| user.id == claims.user_id)
        .unwrap_or(User {
            id: claims.user_id.clone(),
            username: claims.user_id.clone(),
            name: preferred_name(Some(claims.user_name.as_str()), claims.user_id.as_str()),
            email: format!("{}@example.com", claims.user_id),
            role: map_claim_role_to_user_role(claims.role.as_deref()),
            department: None,
            avatar: None,
        })
}


/// GET /api/users - 获取用户列表
pub(super) async fn list_users(Query(query): Query<UserListQuery>) -> impl IntoResponse {
    info!("Listing users");

    let mock_users = build_mock_review_users();

    let users = if let Some(ref role) = query.role {
        mock_users.into_iter().filter(|u| &u.role == role).collect()
    } else {
        mock_users
    };

    (
        StatusCode::OK,
        Json(UserListResponse {
            success: true,
            users,
            error_message: None,
        }),
    )
}

/// GET /api/users/me - 获取当前用户
pub(super) async fn get_current_user(request: axum::extract::Request) -> impl IntoResponse {
    // 尝试从 JWT Claims 获取用户信息
    use super::auth::TokenClaims;

    if let Some(claims) = request.extensions().get::<TokenClaims>() {
        let user = current_user_from_claims(claims);
        return (
            StatusCode::OK,
            Json(UserResponse {
                success: true,
                user: Some(user),
                error_message: None,
            }),
        );
    }

    // 如果没有 JWT，返回 mock 用户
    let user = default_mock_user();

    (
        StatusCode::OK,
        Json(UserResponse {
            success: true,
            user: Some(user),
            error_message: None,
        }),
    )
}

/// GET /api/users/reviewers - 获取审核人员列表
pub(super) async fn get_reviewers() -> impl IntoResponse {
    info!("Getting reviewers");

    let reviewers = build_mock_review_users()
        .into_iter()
        .filter(|user| {
            matches!(
                user.role.as_str(),
                "proofreader" | "reviewer" | "manager" | "admin"
            )
        })
        .collect();

    (
        StatusCode::OK,
        Json(UserListResponse {
            success: true,
            users: reviewers,
            error_message: None,
        }),
    )
}


// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{self, Body},
        http::{Request, StatusCode},
        routing::get,
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::web_service::review::auth::TokenClaims;
    #[test]
    fn test_build_mock_review_users_matches_frontend_contract() {
        let users = build_mock_review_users();
        let ids = users
            .iter()
            .map(|user| user.id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"SJ"));
        assert!(ids.contains(&"JH"));
        assert!(ids.contains(&"SH"));
        assert!(ids.contains(&"PZ"));
        assert!(!ids.contains(&"designer_001"));
        assert!(!ids.contains(&"proofreader_001"));
        assert!(!ids.contains(&"reviewer_001"));
        assert!(!ids.contains(&"manager_001"));
    }

    #[test]
    fn test_build_mock_reviewers_only_returns_review_capable_roles() {
        let reviewers = build_mock_review_users()
            .into_iter()
            .filter(|user| {
                matches!(
                    user.role.as_str(),
                    "proofreader" | "reviewer" | "manager" | "admin"
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(reviewers.len(), 3);
        assert!(reviewers.iter().all(|user| user.id != "SJ"));
        assert!(reviewers.iter().any(|user| user.id == "JH"));
        assert!(reviewers.iter().any(|user| user.id == "SH"));
        assert!(reviewers.iter().any(|user| user.id == "PZ"));
    }

    #[test]
    fn test_default_mock_user_matches_frontend_designer_contract() {
        let user = default_mock_user();

        assert_eq!(user.id, "SJ");
        assert_eq!(user.username, "SJ");
        assert_eq!(user.role, "designer");
    }

    #[tokio::test]
    async fn test_get_current_user_returns_frontend_designer_contract_without_claims() {
        let app = Router::new().route("/api/users/me", get(get_current_user));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/users/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: UserResponse = serde_json::from_slice(&body).unwrap();
        let user = payload.user.expect("expected current user payload");

        assert_eq!(user.id, "SJ");
        assert_eq!(user.username, "SJ");
        assert_eq!(user.role, "designer");
    }

    #[tokio::test]
    async fn test_get_reviewers_returns_only_review_capable_users() {
        let app = Router::new().route("/api/users/reviewers", get(get_reviewers));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/users/reviewers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: UserListResponse = serde_json::from_slice(&body).unwrap();
        let ids = payload
            .users
            .iter()
            .map(|user| user.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(payload.users.len(), 3);
        assert!(!ids.contains(&"designer_001"));
        assert!(!ids.contains(&"proofreader_001"));
        assert!(!ids.contains(&"reviewer_001"));
        assert!(!ids.contains(&"manager_001"));
        assert!(ids.contains(&"JH"));
        assert!(ids.contains(&"SH"));
        assert!(ids.contains(&"PZ"));
    }

    #[tokio::test]
    async fn test_get_current_user_maps_pz_claim_to_explicit_pz_user() {
        let app = Router::new().route("/api/users/me", get(get_current_user));
        let claims = TokenClaims {
            project_id: "project-123".to_string(),
            user_id: "PZ".to_string(),
            user_name: "PZ批准人".to_string(),
            role: Some("pz".to_string()),
            workflow_mode: None,
            legacy_form_id: None,
            exp: 4_102_444_800,
            iat: 1_704_067_200,
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/users/me")
                    .extension(claims)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: UserResponse = serde_json::from_slice(&body).unwrap();
        let user = payload.user.expect("expected current user payload");

        assert_eq!(user.id, "PZ");
        assert_eq!(user.username, "PZ");
        assert_eq!(user.name, "陈经理");
        assert_eq!(user.role, "manager");
        assert_eq!(user.email, "pz@company.com");
    }

    #[tokio::test]
    async fn test_get_current_user_uses_token_claims_when_present() {
        let app = Router::new().route("/api/users/me", get(get_current_user));
        let claims = TokenClaims {
            project_id: "project-123".to_string(),
            user_id: "SH".to_string(),
            user_name: "李审核员".to_string(),
            role: Some("sh".to_string()),
            workflow_mode: None,
            legacy_form_id: None,
            exp: 4_102_444_800,
            iat: 1_704_067_200,
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/users/me")
                    .extension(claims)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: UserResponse = serde_json::from_slice(&body).unwrap();
        let user = payload.user.expect("expected current user payload");

        assert_eq!(user.id, "SH");
        assert_eq!(user.username, "SH");
        assert_eq!(user.name, "李审核员");
        assert_eq!(user.role, "reviewer");
        assert_eq!(user.email, "sh@company.com");
    }
}
