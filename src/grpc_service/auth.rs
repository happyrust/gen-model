//! GRPC服务认证和授权模块

use crate::grpc_service::error::{ServiceError, ServiceResult};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::{Request, Status};

/// JWT Claims结构
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,        // 用户ID
    pub name: String,       // 用户名
    pub roles: Vec<String>, // 用户角色
    pub exp: usize,         // 过期时间
    pub iat: usize,         // 签发时间
}

/// 用户权限枚举
#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    ReadMdb,
    StartTask,
    StopTask,
    ViewProgress,
    AdminAccess,
}

/// 认证配置
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub token_expiry_hours: u64,
    pub enable_auth: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "default_secret_change_in_production".to_string(),
            token_expiry_hours: 24,
            enable_auth: true,
        }
    }
}

/// 认证服务
#[derive(Debug, Clone)]
pub struct AuthService {
    config: AuthConfig,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl AuthService {
    /// 创建新的认证服务
    pub fn new(config: AuthConfig) -> Self {
        let encoding_key = EncodingKey::from_secret(config.jwt_secret.as_ref());
        let decoding_key = DecodingKey::from_secret(config.jwt_secret.as_ref());

        Self {
            config,
            encoding_key,
            decoding_key,
        }
    }

    /// 生成JWT token
    pub fn generate_token(
        &self,
        user_id: &str,
        username: &str,
        roles: Vec<String>,
    ) -> ServiceResult<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ServiceError::Internal(anyhow::anyhow!("Time error: {}", e)))?
            .as_secs() as usize;

        let exp = now + (self.config.token_expiry_hours * 3600) as usize;

        let claims = Claims {
            sub: user_id.to_string(),
            name: username.to_string(),
            roles,
            exp,
            iat: now,
        };

        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| ServiceError::AuthenticationFailed)
    }

    /// 验证JWT token
    pub fn validate_token(&self, token: &str) -> ServiceResult<Claims> {
        let validation = Validation::new(Algorithm::HS256);

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|_| ServiceError::AuthenticationFailed)
    }

    /// 检查用户权限
    pub fn check_permission(&self, claims: &Claims, permission: Permission) -> bool {
        match permission {
            Permission::ReadMdb => {
                claims.roles.contains(&"user".to_string())
                    || claims.roles.contains(&"admin".to_string())
            }
            Permission::ViewProgress => {
                claims.roles.contains(&"user".to_string())
                    || claims.roles.contains(&"admin".to_string())
            }
            Permission::StartTask | Permission::StopTask => {
                claims.roles.contains(&"operator".to_string())
                    || claims.roles.contains(&"admin".to_string())
            }
            Permission::AdminAccess => claims.roles.contains(&"admin".to_string()),
        }
    }

    /// 从请求中提取token
    pub fn extract_token_from_request<T>(&self, request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_string())
    }
}

/// 认证拦截器
#[derive(Debug, Clone)]
pub struct AuthInterceptor {
    auth_service: AuthService,
}

impl AuthInterceptor {
    /// 创建新的认证拦截器
    pub fn new(auth_service: AuthService) -> Self {
        Self { auth_service }
    }

    /// 验证请求
    pub fn authenticate<T>(&self, request: &Request<T>) -> Result<Claims, Status> {
        // 如果认证被禁用，返回默认claims
        if !self.auth_service.config.enable_auth {
            return Ok(Claims {
                sub: "system".to_string(),
                name: "System User".to_string(),
                roles: vec!["admin".to_string()],
                exp: 0,
                iat: 0,
            });
        }

        let token = self
            .auth_service
            .extract_token_from_request(request)
            .ok_or_else(|| Status::unauthenticated("Missing authorization token"))?;

        self.auth_service
            .validate_token(&token)
            .map_err(|_| Status::unauthenticated("Invalid token"))
    }

    /// 检查权限
    pub fn authorize(&self, claims: &Claims, permission: Permission) -> Result<(), Status> {
        if self.auth_service.check_permission(claims, permission) {
            Ok(())
        } else {
            Err(Status::permission_denied("Insufficient permissions"))
        }
    }
}

/// 速率限制器
#[derive(Debug)]
pub struct RateLimiter {
    requests: dashmap::DashMap<String, std::collections::VecDeque<std::time::Instant>>,
    max_requests: usize,
    window_duration: std::time::Duration,
}

impl RateLimiter {
    /// 创建新的速率限制器
    pub fn new(max_requests: usize, window_duration: std::time::Duration) -> Self {
        Self {
            requests: dashmap::DashMap::new(),
            max_requests,
            window_duration,
        }
    }

    /// 检查是否允许请求
    pub fn allow_request(&self, client_id: &str) -> bool {
        let now = std::time::Instant::now();
        let mut entry = self
            .requests
            .entry(client_id.to_string())
            .or_insert_with(|| std::collections::VecDeque::new());

        // 清理过期的请求记录
        while let Some(&front_time) = entry.front() {
            if now.duration_since(front_time) > self.window_duration {
                entry.pop_front();
            } else {
                break;
            }
        }

        // 检查是否超过限制
        if entry.len() >= self.max_requests {
            false
        } else {
            entry.push_back(now);
            true
        }
    }
}

/// 输入验证器
pub struct InputValidator;

impl InputValidator {
    /// 验证任务ID
    pub fn validate_task_id(task_id: &str) -> ServiceResult<()> {
        if task_id.is_empty() {
            return Err(ServiceError::InvalidRequest(
                "Task ID cannot be empty".to_string(),
            ));
        }

        if task_id.len() > 100 {
            return Err(ServiceError::InvalidRequest("Task ID too long".to_string()));
        }

        // 检查是否包含有害字符
        if task_id.contains(['<', '>', '"', '\'', '&']) {
            return Err(ServiceError::InvalidRequest(
                "Task ID contains invalid characters".to_string(),
            ));
        }

        Ok(())
    }

    /// 验证MDB名称
    pub fn validate_mdb_name(mdb_name: &str) -> ServiceResult<()> {
        if mdb_name.is_empty() {
            return Err(ServiceError::InvalidRequest(
                "MDB name cannot be empty".to_string(),
            ));
        }

        if mdb_name.len() > 200 {
            return Err(ServiceError::InvalidRequest(
                "MDB name too long".to_string(),
            ));
        }

        // 检查路径遍历攻击
        if mdb_name.contains("..") || mdb_name.contains('/') {
            return Err(ServiceError::InvalidRequest(
                "MDB name contains invalid path characters".to_string(),
            ));
        }

        Ok(())
    }

    /// 清理字符串输入
    pub fn sanitize_string(input: &str) -> String {
        input
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#x27;")
            .replace('&', "&amp;")
    }
}
