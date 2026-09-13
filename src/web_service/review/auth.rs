//! 校审域 · 鉴权（`POST /api/auth/token`、`POST /api/auth/verify`，JWT 中间件与 `[review_auth]` /
//! `[platform_auth]` 配置）。
//!
//! 旧端 `src/web_api/jwt_auth.rs` 逐字搬入：JWT 签发 / 校验 / 不验签解码、角色 `admin/sj/jd/sh/pz`、
//! `review_auth_middleware`（启用时校验 Bearer，关闭时注入调试身份）、`require_roles` 角色闸门，
//! 以及 401 / 403 的错误文本（计划 §6 A5 逐字对照 `jwt_auth.rs:513,647`）。
//!
//! 与旧端的差异：
//! - 旧端的 `#[cfg(feature = "web_server")]` 门在这里由 `http_api` 特性统一把守（`lib.rs` 上的
//!   `pub mod web_service`），故不再逐项标注；
//! - `review-internal-workflow` 特性按计划 Q7 不移植，旧端 `#[cfg(feature)]` 双分支处只保留
//!   默认（外部驱动）分支：`workflow_mode` 不写进 claims，`/api/auth/token` 收到该字段只告警忽略；
//! - `[review_auth]` / `[platform_auth]` 自 T1b 起走 `crate::options::{review_auth, platform_auth}`
//!   这一个解析口，缺省值与旧端 `jwt_auth.rs` 的 `Default` 逐字相同；JWT 密钥不在其中，
//!   仍是 `JWT_SECRET` 环境变量与 `model_center.token_secret` 那一路，因此本文件的
//!   `load_config()`（`DB_OPTION_FILE` → `db_options/DbOption` → `DbOption`）保留——
//!   `JwtConfig` 与 T3 的 `platform::config` 都还从它取值。
//!
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use axum::{
    Router,
    extract::{Json, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::IntoResponse,
    routing::post,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, TokenData, Validation, decode, encode,
};

// ============================================================================
// Configuration
// ============================================================================

/// JWT 配置
#[derive(Clone, Debug)]
pub struct JwtConfig {
    /// 密钥 (用于签名和验证)
    pub secret: String,
    /// Token 过期时间（小时）
    pub expiration_hours: u64,
}

/// PMS 入站 S2S 认证配置
#[derive(Clone, Debug)]
pub struct PlatformAuthConfig {
    /// 是否启用真实 JWT 校验
    pub enabled: bool,
    /// 关闭真实校验时允许的调试 token；为空则仍拒绝
    pub debug_token: String,
}

/// 校审接口认证配置
#[derive(Clone, Debug)]
pub struct ReviewAuthConfig {
    /// 是否启用 JWT 认证
    pub enabled: bool,
    /// 关闭认证时注入的调试项目号
    pub debug_project_id: String,
    /// 关闭认证时注入的调试用户
    pub debug_user_id: String,
    /// 关闭认证时注入的调试角色
    pub debug_role: String,
}

const DEFAULT_JWT_SECRET: &str = "default-jwt-secret-key";

fn warn_if_default_jwt_secret(secret: &str) {
    if secret == DEFAULT_JWT_SECRET {
        warn!(
            "JWT_SECRET 命中默认密钥 default-jwt-secret-key；生产和联调环境都不能依赖默认密钥，请尽快改为独立密钥"
        );
    }
}

impl Default for JwtConfig {
    fn default() -> Self {
        let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| DEFAULT_JWT_SECRET.to_string());
        warn_if_default_jwt_secret(&secret);
        Self {
            secret,
            expiration_hours: 24,
        }
    }
}

impl Default for PlatformAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debug_token: String::new(),
        }
    }
}

impl Default for ReviewAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debug_project_id: "debug-project".to_string(),
            debug_user_id: "designer_001".to_string(),
            debug_role: "sj".to_string(),
        }
    }
}

fn build_config_names(config_path: Option<&str>) -> Vec<String> {
    let mut config_names = Vec::new();

    if let Some(config_path) = config_path {
        let normalized = config_path
            .strip_suffix(".toml")
            .unwrap_or(config_path)
            .to_string();
        config_names.push(normalized);
    }

    config_names.extend([
        "db_options/DbOption".to_string(),
        "../db_options/DbOption".to_string(),
        "DbOption".to_string(),
    ]);

    config_names
}

pub(crate) fn load_config() -> Option<config::Config> {
    use config as cfg;

    let env_config = std::env::var("DB_OPTION_FILE").ok();
    let config_names = build_config_names(env_config.as_deref());

    for name in config_names {
        let file_path = format!("{}.toml", name);
        if std::path::Path::new(&file_path).exists() {
            if let Ok(config) = cfg::Config::builder()
                .add_source(cfg::File::with_name(&name))
                .build()
            {
                return Some(config);
            }
        }
    }

    None
}

impl JwtConfig {
    /// 从配置文件加载
    pub fn from_config_file() -> Self {
        if let Some(config) = load_config() {
            let secret = config
                .get_string("model_center.token_secret")
                .unwrap_or_else(|_| DEFAULT_JWT_SECRET.to_string());
            warn_if_default_jwt_secret(&secret);
            return Self {
                secret,
                expiration_hours: config
                    .get_int("model_center.token_expiration_hours")
                    .unwrap_or(24) as u64,
            };
        }
        Self::default()
    }
}

impl PlatformAuthConfig {
    /// 从 `[platform_auth]` 加载（T1b 的 `options::platform_auth()` 是唯一解析口，
    /// 缺省值与本文件的 `Default` 逐字相同）。
    pub fn from_config_file() -> Self {
        let section = crate::options::platform_auth();
        Self {
            enabled: section.enabled(),
            debug_token: section.debug_token().to_string(),
        }
    }
}

impl ReviewAuthConfig {
    /// 从 `[review_auth]` 加载（同上，走 `options::review_auth()`）。
    pub fn from_config_file() -> Self {
        let section = crate::options::review_auth();
        Self {
            enabled: section.enabled(),
            debug_project_id: section.debug_project_id().to_string(),
            debug_user_id: section.debug_user_id().to_string(),
            debug_role: section.debug_role().to_string(),
        }
    }
}

// ============================================================================
// Role Definition
// ============================================================================

/// 角色枚举
/// - admin: 管理员
/// - sj: 设计（编）
/// - jd: 校对（校）
/// - sh: 审核（审）
/// - pz: 批准
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Sj,
    Jd,
    Sh,
    Pz,
}

impl Role {
    /// 从字符串解析角色
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "admin" => Some(Role::Admin),
            "sj" => Some(Role::Sj),
            "jd" => Some(Role::Jd),
            "sh" => Some(Role::Sh),
            "pz" => Some(Role::Pz),
            _ => None,
        }
    }

    /// 转换为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Sj => "sj",
            Role::Jd => "jd",
            Role::Sh => "sh",
            Role::Pz => "pz",
        }
    }

    /// 获取角色显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            Role::Admin => "管理员",
            Role::Sj => "设计（编）",
            Role::Jd => "校对（校）",
            Role::Sh => "审核（审）",
            Role::Pz => "批准",
        }
    }

    /// 获取所有有效角色值
    pub fn valid_values() -> &'static [&'static str] {
        &["admin", "sj", "jd", "sh", "pz"]
    }
}

pub(crate) fn normalize_workflow_mode(value: Option<&str>) -> Option<String> {
    let normalized = value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())?;

    match normalized.as_str() {
        "external" | "manual" | "internal" => Some(normalized),
        _ => None,
    }
}

// ============================================================================
// JWT Claims
// ============================================================================

/// JWT Payload (Claims)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TokenClaims {
    /// 项目号
    pub project_id: String,
    /// 用户ID
    pub user_id: String,
    /// 用户姓名
    #[serde(default)]
    pub user_name: String,
    /// 角色 (可选)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 工作流模式 (可选)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_mode: Option<String>,
    /// 兼容旧 token：读取时可接收旧的 form_id，但新 token 不再序列化它
    #[serde(default, alias = "form_id", skip_serializing)]
    pub legacy_form_id: Option<String>,
    /// 过期时间戳 (Unix timestamp)
    pub exp: u64,
    /// 签发时间戳 (Unix timestamp)
    pub iat: u64,
}

// ============================================================================
// Request/Response Structs
// ============================================================================

/// Token 获取请求
/// 支持两种格式:
/// 1. 新格式: {"username": "xxx", "project": "xxx", "role": "xxx"}
/// 2. 旧格式: {"project_id": "xxx", "user_id": "xxx", "form_id": "xxx"}
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// 项目号 (新格式使用 project，旧格式使用 project_id)
    #[serde(alias = "project")]
    pub project_id: Option<String>,
    /// 用户ID (新格式使用 username，旧格式使用 user_id)
    #[serde(alias = "username")]
    pub user_id: Option<String>,
    /// 用户姓名（可选，不传则回退到 user_id）
    #[serde(alias = "name", alias = "display_name")]
    pub user_name: Option<String>,
    /// 可选的 form_id，如果不传则自动生成
    pub form_id: Option<String>,
    /// 角色 (可选，用于权限控制)
    pub role: Option<String>,
    /// 工作流模式（仅 `review-internal-workflow` feature 开启时生效；默认外部模式会忽略）
    #[serde(alias = "workflowMode")]
    pub workflow_mode: Option<String>,
}

/// Token 获取响应
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<TokenData_>,
}

#[derive(Debug, Serialize)]
pub struct TokenData_ {
    /// JWT Token
    pub token: String,
    /// Token 过期时间 (Unix timestamp)
    pub expires_at: u64,
    /// 表单ID
    pub form_id: String,
}

/// Token 验证请求
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub token: String,
    #[serde(default)]
    pub form_id: Option<String>,
}

/// Token 验证响应
#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<VerifyData>,
}

#[derive(Debug, Serialize)]
pub struct VerifyData {
    /// 是否有效
    pub valid: bool,
    /// 解码后的 Claims
    pub claims: Option<TokenClaims>,
    /// 如果无效，错误原因
    pub error: Option<String>,
}

// ============================================================================
// JWT Functions
// ============================================================================

lazy_static::lazy_static! {
    static ref JWT_CONFIG: JwtConfig = JwtConfig::from_config_file();
    pub static ref PLATFORM_AUTH_CONFIG: PlatformAuthConfig = PlatformAuthConfig::from_config_file();
    pub static ref REVIEW_AUTH_CONFIG: ReviewAuthConfig = ReviewAuthConfig::from_config_file();
}

/// 生成 form_id (UUID)
pub fn generate_form_id() -> String {
    format!(
        "FORM-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .to_uppercase()[..12]
            .to_string()
    )
}

/// 生成 JWT Token
pub fn create_token(
    project_id: &str,
    user_id: &str,
    user_name: Option<&str>,
    role: Option<&str>,
    workflow_mode: Option<&str>,
) -> Result<(String, u64), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();

    let exp = now + JWT_CONFIG.expiration_hours * 3600;

    let claims = TokenClaims {
        project_id: project_id.to_string(),
        user_id: user_id.to_string(),
        user_name: user_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(user_id)
            .to_string(),
        role: role.map(|s| s.to_string()),
        // `review-internal-workflow` 不移植（计划 Q7）：外部驱动模式下 claims 不携带 workflow_mode。
        workflow_mode: {
            let _ = workflow_mode;
            None
        },
        legacy_form_id: None,
        exp,
        iat: now,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_CONFIG.secret.as_bytes()),
    )
    .map_err(|e| e.to_string())?;

    Ok((token, exp))
}

/// 验证 JWT Token 并返回 Claims
pub fn verify_token(token: &str) -> Result<TokenClaims, String> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let token_data: TokenData<TokenClaims> = decode(
        token,
        &DecodingKey::from_secret(JWT_CONFIG.secret.as_bytes()),
        &validation,
    )
    .map_err(|e| e.to_string())?;

    Ok(token_data.claims)
}

/// 解码 JWT Token（不验证签名，用于调试）
pub fn decode_token_unsafe(token: &str) -> Result<TokenClaims, String> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;

    let token_data: TokenData<TokenClaims> = decode(
        token,
        &DecodingKey::from_secret(&[]), // 不需要密钥
        &validation,
    )
    .map_err(|e| e.to_string())?;

    Ok(token_data.claims)
}

// ============================================================================
// Axum Handlers
// ============================================================================

/// JWT 认证路由（旧端 `create_jwt_auth_routes`）。
///
/// 取 token / 验 token 本身不经 `review_auth_middleware`（旧端同样注册在校审路由之外）。
/// 状态类型留成泛型：主 Router 用 `AppState`，handler 级单测直接 `Router<()>`。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/auth/token", post(get_token))
        .route("/api/auth/verify", post(verify_token_handler))
}

// ============================================================================
// Axum Middleware
// ============================================================================

use axum::{extract::Request, middleware::Next, response::Response};

fn unauthorized_error(message: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "success": false,
            "error_message": message
        })),
    )
}

fn extract_bearer_token<'a>(headers: &'a HeaderMap) -> Option<&'a str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn resolve_token_claims(
    headers: &HeaderMap,
) -> Result<TokenClaims, (StatusCode, Json<serde_json::Value>)> {
    let token = extract_bearer_token(headers)
        .ok_or_else(|| unauthorized_error("Missing or invalid Authorization header".to_string()))?;

    verify_token(token).map_err(|e| {
        warn!("JWT verification failed: {}", e);
        unauthorized_error(format!("Invalid token: {}", e))
    })
}

fn build_debug_claims(config: &ReviewAuthConfig) -> TokenClaims {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let role = config.debug_role.trim();

    TokenClaims {
        project_id: config.debug_project_id.clone(),
        user_id: config.debug_user_id.clone(),
        user_name: config.debug_user_id.clone(),
        role: if role.is_empty() {
            None
        } else {
            Some(role.to_string())
        },
        // `review-internal-workflow` 不移植（计划 Q7）：调试身份同样不带 workflow_mode。
        workflow_mode: None,
        legacy_form_id: None,
        exp: now + 365 * 24 * 3600,
        iat: now,
    }
}

/// JWT 认证中间件
/// 从 Authorization header 提取 Bearer token 并验证
pub async fn jwt_auth_middleware(
    mut request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let claims = resolve_token_claims(request.headers())?;
    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

/// 校审 API 认证中间件
/// 启用认证时校验 JWT；关闭认证时注入调试身份
pub async fn review_auth_middleware(
    State(config): State<ReviewAuthConfig>,
    mut request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    if config.enabled {
        let claims = resolve_token_claims(request.headers())?;
        request.extensions_mut().insert(claims);
    } else {
        let claims = extract_bearer_token(request.headers())
            .and_then(|token| match decode_token_unsafe(token) {
                Ok(claims) => Some(claims),
                Err(error) => {
                    warn!(
                        "JWT decode without verification failed in debug mode: {}",
                        error
                    );
                    None
                }
            })
            .unwrap_or_else(|| build_debug_claims(&config));
        request.extensions_mut().insert(claims);
    }

    Ok(next.run(request).await)
}

/// 可选的 JWT 认证中间件（不强制要求 token）
pub async fn jwt_auth_optional_middleware(mut request: Request, next: Next) -> Response {
    // 尝试从 Authorization header 提取 token
    if let Some(auth_header) = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if auth_header.starts_with("Bearer ") {
            let token = &auth_header[7..];
            if let Ok(claims) = verify_token(token) {
                request.extensions_mut().insert(claims);
            }
        }
    }
    next.run(request).await
}

/// 角色验证中间件工厂
/// 检查用户是否具有指定角色之一
pub fn require_roles(
    allowed_roles: &'static [&'static str],
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Response, (StatusCode, Json<serde_json::Value>)>>
            + Send,
    >,
> + Clone
+ Send {
    move |request: Request, next: Next| {
        Box::pin(async move {
            // 从 extensions 获取 claims
            let claims = request.extensions().get::<TokenClaims>();

            match claims {
                Some(c) => {
                    // 检查角色
                    let user_role = c.role.as_deref().unwrap_or("");
                    if allowed_roles.contains(&user_role) || allowed_roles.contains(&"*") {
                        Ok(next.run(request).await)
                    } else {
                        Err((
                            StatusCode::FORBIDDEN,
                            Json(serde_json::json!({
                                "success": false,
                                "error_message": format!(
                                    "Access denied. Required roles: {:?}, your role: {}",
                                    allowed_roles, user_role
                                )
                            })),
                        ))
                    }
                }
                None => Err((
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "success": false,
                        "error_message": "No authentication token found"
                    })),
                )),
            }
        })
    }
}

/// 获取 Token
async fn get_token(Json(request): Json<TokenRequest>) -> impl IntoResponse {
    // 获取 project_id 和 user_id (支持两种格式)
    let project_id = match &request.project_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            warn!("Missing project_id/project");
            return (
                StatusCode::BAD_REQUEST,
                Json(TokenResponse {
                    code: -1,
                    message: "Missing required field: project_id or project".to_string(),
                    data: None,
                }),
            );
        }
    };

    let user_id = match &request.user_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            warn!("Missing user_id/username");
            return (
                StatusCode::BAD_REQUEST,
                Json(TokenResponse {
                    code: -1,
                    message: "Missing required field: user_id or username".to_string(),
                    data: None,
                }),
            );
        }
    };

    info!(
        "Token request: project_id={}, user_id={}, role={:?}, workflow_mode={:?}",
        project_id, user_id, request.role, request.workflow_mode
    );

    // 验证 role (如果提供了)
    let validated_role = if let Some(ref role_str) = request.role {
        match Role::from_str(role_str) {
            Some(role) => Some(role.as_str()),
            None => {
                warn!("Invalid role: {}", role_str);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(TokenResponse {
                        code: -1,
                        message: format!(
                            "Invalid role: '{}'. Valid values are: {:?}",
                            role_str,
                            Role::valid_values()
                        ),
                        data: None,
                    }),
                );
            }
        }
    } else {
        None
    };

    // `review-internal-workflow` 不移植（计划 Q7）：与旧端默认构建一致，收到 workflow_mode 只告警忽略。
    let validated_workflow_mode: Option<String> = {
        if request
            .workflow_mode
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            warn!("Ignoring workflow_mode because review-internal-workflow feature is disabled");
        }
        None
    };

    // 生成或使用传入的 form_id
    let form_id = request.form_id.unwrap_or_else(generate_form_id);

    // 生成 token
    match create_token(
        &project_id,
        &user_id,
        request.user_name.as_deref(),
        validated_role,
        validated_workflow_mode.as_deref(),
    ) {
        Ok((token, expires_at)) => {
            info!("Token generated for user={}, form_id={}", user_id, form_id);
            (
                StatusCode::OK,
                Json(TokenResponse {
                    code: 0,
                    message: "ok".to_string(),
                    data: Some(TokenData_ {
                        token,
                        expires_at,
                        form_id,
                    }),
                }),
            )
        }
        Err(e) => {
            warn!("Token generation failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TokenResponse {
                    code: -1,
                    message: format!("Token generation failed: {}", e),
                    data: None,
                }),
            )
        }
    }
}

/// 验证 Token
async fn verify_token_handler(Json(request): Json<VerifyRequest>) -> impl IntoResponse {
    let request_form_id = request
        .form_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    info!(
        "Token verification request: has_form_id_hint={}, requested_form_id_hint={:?}",
        request_form_id.is_some(),
        request_form_id
    );

    match verify_token(&request.token) {
        Ok(claims) => {
            if request_form_id.is_some() {
                info!(
                    "Token verification received deprecated form_id hint; ignoring request_form_id={:?}, project_id={}, user_id={}, role={:?}",
                    request_form_id, claims.project_id, claims.user_id, claims.role
                );
            }
            if let Some(legacy_form_id) = claims.legacy_form_id.as_deref() {
                warn!(
                    "Token verification decoded legacy token claim form_id={}, project_id={}, user_id={}; explicit form_id request fields remain authoritative",
                    legacy_form_id, claims.project_id, claims.user_id
                );
            }
            info!(
                "Token verified: request_form_id_hint={:?}, project_id={}, user_id={}, role={:?}, workflow_mode={:?}",
                request_form_id,
                claims.project_id,
                claims.user_id,
                claims.role,
                claims.workflow_mode
            );
            (
                StatusCode::OK,
                Json(VerifyResponse {
                    code: 0,
                    message: "ok".to_string(),
                    data: Some(VerifyData {
                        valid: true,
                        claims: Some(claims),
                        error: None,
                    }),
                }),
            )
        }
        Err(e) => {
            warn!("Token verification failed: {}", e);
            (
                StatusCode::OK,
                Json(VerifyResponse {
                    code: 0,
                    message: "ok".to_string(),
                    data: Some(VerifyData {
                        valid: false,
                        claims: None,
                        error: Some(e),
                    }),
                }),
            )
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension, Json, Router,
        http::{Request, StatusCode},
        middleware,
        routing::get,
    };
    use tower::ServiceExt;

    #[derive(Debug, Deserialize)]
    struct VerifyResponseBody {
        code: i32,
        data: Option<VerifyDataBody>,
    }

    #[derive(Debug, Deserialize)]
    struct VerifyDataBody {
        valid: bool,
        claims: Option<TokenClaims>,
        error: Option<String>,
    }

    async fn claims_echo_handler(Extension(claims): Extension<TokenClaims>) -> Json<TokenClaims> {
        Json(claims)
    }

    #[test]
    fn test_create_and_verify_token() {
        let (token, _exp) = create_token("2410", "kangwp", None, None, None).unwrap();
        assert!(!token.is_empty());

        let claims = verify_token(&token).unwrap();
        assert_eq!(claims.project_id, "2410");
        assert_eq!(claims.user_id, "kangwp");
        assert_eq!(claims.user_name, "kangwp");
        assert_eq!(claims.legacy_form_id, None);
        assert_eq!(claims.role, None);
    }

    #[test]
    fn test_create_token_with_role() {
        let (token, _exp) = create_token(
            "testproject",
            "testuser",
            Some("测试用户"),
            Some("pz"),
            None,
        )
        .unwrap();
        assert!(!token.is_empty());

        let claims = verify_token(&token).unwrap();
        assert_eq!(claims.project_id, "testproject");
        assert_eq!(claims.user_id, "testuser");
        assert_eq!(claims.user_name, "测试用户");
        assert_eq!(claims.legacy_form_id, None);
        assert_eq!(claims.role, Some("pz".to_string()));
    }

    /// 签发后的 JWT **原始 payload** 不得出现 `form_id`（单据维度只走 URL/query，与 PMS 对齐）。
    #[test]
    fn test_jwt_payload_json_has_no_form_id_key() {
        use base64::Engine;
        use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};

        let (token, _) =
            create_token("proj-a", "user-b", Some("N"), Some("sj"), Some("manual")).unwrap();
        let b64 = token.split('.').nth(1).expect("jwt payload segment");
        let json_bytes = URL_SAFE_NO_PAD.decode(b64).unwrap_or_else(|_| {
            let mut padded = b64.to_string();
            while padded.len() % 4 != 0 {
                padded.push('=');
            }
            URL_SAFE
                .decode(padded.as_bytes())
                .expect("jwt payload base64url")
        });
        let v: serde_json::Value = serde_json::from_slice(&json_bytes).expect("jwt payload json");
        let obj = v.as_object().expect("claims object");
        assert!(
            !obj.contains_key("form_id"),
            "user_token (JWT) must not serialize form_id; got keys={:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_token_claims_json_omit_form_id_even_if_legacy_set() {
        let claims = TokenClaims {
            project_id: "p".into(),
            user_id: "u".into(),
            user_name: "u".into(),
            role: Some("sj".into()),
            workflow_mode: Some("manual".into()),
            legacy_form_id: Some("FORM-LEGACY".into()),
            exp: 1,
            iat: 1,
        };
        let v = serde_json::to_value(&claims).unwrap();
        let o = v.as_object().unwrap();
        assert!(!o.contains_key("form_id"));
        assert!(!o.contains_key("legacy_form_id"));
    }

    #[test]
    fn test_decode_token_unsafe() {
        let (token, _exp) = create_token("2410", "kangwp", None, None, None).unwrap();

        let claims = decode_token_unsafe(&token).unwrap();
        assert_eq!(claims.project_id, "2410");
        assert_eq!(claims.user_id, "kangwp");
        assert_eq!(claims.user_name, "kangwp");
    }

    #[test]
    fn test_invalid_token() {
        let result = verify_token("invalid-token");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_review_auth_middleware_requires_authorization_when_enabled() {
        let app = Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn_with_state(
                ReviewAuthConfig {
                    enabled: true,
                    debug_project_id: "debug-project".to_string(),
                    debug_user_id: "debug-user".to_string(),
                    debug_role: "sj".to_string(),
                },
                review_auth_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_review_auth_middleware_injects_debug_claims_when_disabled() {
        let app = Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn_with_state(
                ReviewAuthConfig {
                    enabled: false,
                    debug_project_id: "debug-project".to_string(),
                    debug_user_id: "debug-user".to_string(),
                    debug_role: "sj".to_string(),
                },
                review_auth_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let claims: TokenClaims = serde_json::from_slice(&body).unwrap();

        assert_eq!(claims.project_id, "debug-project");
        assert_eq!(claims.user_id, "debug-user");
        assert_eq!(claims.user_name, "debug-user");
        assert_eq!(claims.role.as_deref(), Some("sj"));
    }

    #[test]
    fn test_build_config_names_prefers_explicit_config() {
        let names = build_config_names(Some("/tmp/demo-config.toml"));
        assert_eq!(names.first().map(String::as_str), Some("/tmp/demo-config"));
    }

    #[test]
    fn test_review_auth_config_default_matches_frontend_contract() {
        let config = ReviewAuthConfig::default();

        assert!(config.enabled);
        assert_eq!(config.debug_project_id, "debug-project");
        assert_eq!(config.debug_user_id, "designer_001");
        assert_eq!(config.debug_role, "sj");
    }

    #[tokio::test]
    async fn test_review_auth_middleware_decodes_token_without_verification_when_disabled() {
        let app = Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn_with_state(
                ReviewAuthConfig {
                    enabled: false,
                    debug_project_id: "debug-project".to_string(),
                    debug_user_id: "debug-user".to_string(),
                    debug_role: "sj".to_string(),
                },
                review_auth_middleware,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .header(
                        AUTHORIZATION,
                        format!(
                            "Bearer {}",
                            create_token("1516", "user-002", Some("李校对"), Some("jd"), None)
                                .unwrap()
                                .0
                        ),
                    )
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let claims: TokenClaims = serde_json::from_slice(&body).unwrap();

        assert_eq!(claims.project_id, "1516");
        assert_eq!(claims.user_id, "user-002");
        assert_eq!(claims.user_name, "李校对");
        assert_eq!(claims.role.as_deref(), Some("jd"));
    }

    #[tokio::test]
    async fn test_verify_token_handler_ignores_form_id_hint_mismatch() {
        let app: Router = routes();
        let (token, _) = create_token("project-1", "user-1", None, Some("sj"), None).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/verify")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "token": token,
                            "form_id": "FORM-OTHER"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: VerifyResponseBody = serde_json::from_slice(&body).unwrap();

        assert_eq!(payload.code, 0);
        assert_eq!(payload.data.as_ref().map(|data| data.valid), Some(true));
        assert_eq!(
            payload.data.as_ref().and_then(|data| data.error.as_deref()),
            None
        );
    }

    #[tokio::test]
    async fn test_verify_token_handler_accepts_matching_form_id() {
        let app: Router = routes();
        let (token, _) = create_token("project-1", "user-1", None, Some("sj"), None).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/verify")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "token": token,
                            "form_id": "FORM-EXPECTED"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: VerifyResponseBody = serde_json::from_slice(&body).unwrap();

        assert_eq!(payload.code, 0);
        assert_eq!(payload.data.as_ref().map(|data| data.valid), Some(true));
        assert_eq!(
            payload
                .data
                .as_ref()
                .and_then(|data| data.claims.as_ref())
                .map(|claims| claims.project_id.as_str()),
            Some("project-1")
        );
    }

    // ========================================================================
    // 401 / 403 错误文本逐字护栏（计划 §6 A5）
    // ========================================================================
    //
    // 前端按 `error_message` 的内容分支（`reviewApi.ts` 把它原样弹给用户），所以这三句
    // 是契约的一部分，不是日志。改动任意一句都要同步改旧端，否则两端行为分叉。

    /// 旧端 `plant-model-gen/src/web_api/jwt_auth.rs:513`
    const LEGACY_MISSING_BEARER_MESSAGE: &str = "Missing or invalid Authorization header";
    /// 旧端 `jwt_auth.rs:517`（`Invalid token: {jsonwebtoken 的错误}`）
    const LEGACY_INVALID_TOKEN_PREFIX: &str = "Invalid token: ";
    /// 旧端 `jwt_auth.rs:647`
    const LEGACY_ROLE_DENIED_TEMPLATE: &str = "Access denied. Required roles: {:?}, your role: {}";
    /// 旧端 `jwt_auth.rs:657`
    const LEGACY_NO_CLAIMS_MESSAGE: &str = "No authentication token found";

    async fn error_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("错误响应必须是 JSON")
    }

    fn enabled_auth_app() -> Router {
        Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn_with_state(
                ReviewAuthConfig {
                    enabled: true,
                    debug_project_id: "debug-project".to_string(),
                    debug_user_id: "designer_001".to_string(),
                    debug_role: "sj".to_string(),
                },
                review_auth_middleware,
            ))
    }

    #[tokio::test]
    async fn review_auth_401_body_is_verbatim_legacy_text() {
        let missing = enabled_auth_app()
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        let payload = error_body(missing).await;
        assert_eq!(payload["success"], false);
        assert_eq!(payload["error_message"], LEGACY_MISSING_BEARER_MESSAGE);

        // 带了 Bearer 但不是有效 JWT：旧端走 `verify_token` 失败分支，文本带 jsonwebtoken 的原因。
        let invalid = enabled_auth_app()
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .header(AUTHORIZATION, "Bearer not-a-jwt")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        let payload = error_body(invalid).await;
        assert_eq!(payload["success"], false);
        let message = payload["error_message"].as_str().expect("error_message");
        assert!(
            message.starts_with(LEGACY_INVALID_TOKEN_PREFIX),
            "401 文本要沿用旧端 `Invalid token: {{e}}`，实得 {message:?}"
        );

        // `Authorization` 存在但不是 Bearer 形态时，旧端归到「缺 header」那一句而不是「token 无效」。
        let not_bearer = enabled_auth_app()
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .header(AUTHORIZATION, "Basic dXNlcjpwYXNz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(not_bearer.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            error_body(not_bearer).await["error_message"],
            LEGACY_MISSING_BEARER_MESSAGE
        );
    }

    #[tokio::test]
    async fn require_roles_403_body_is_verbatim_legacy_text() {
        const ALLOWED: &[&str] = &["pz"];

        let claims = TokenClaims {
            project_id: "project-1".into(),
            user_id: "designer_001".into(),
            user_name: "张设计".into(),
            role: Some("sj".into()),
            workflow_mode: None,
            legacy_form_id: None,
            exp: 0,
            iat: 0,
        };

        let denied = Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn(require_roles(ALLOWED)))
            // 后加的 layer 在外层先跑，claims 因此在 require_roles 之前就进了 extensions。
            .layer(Extension(claims))
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let payload = error_body(denied).await;
        assert_eq!(payload["success"], false);
        assert_eq!(
            payload["error_message"],
            format!("Access denied. Required roles: {ALLOWED:?}, your role: sj"),
            "403 文本要逐字沿用旧端模板 {LEGACY_ROLE_DENIED_TEMPLATE:?}"
        );

        // 没有 claims（中间件没跑或 token 缺失）时旧端回 401 而不是 403。
        let anonymous = Router::new()
            .route("/claims", get(claims_echo_handler))
            .layer(middleware::from_fn(require_roles(ALLOWED)))
            .oneshot(
                Request::builder()
                    .uri("/claims")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            error_body(anonymous).await["error_message"],
            LEGACY_NO_CLAIMS_MESSAGE
        );
    }
}
