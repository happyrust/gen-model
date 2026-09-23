//! PMS 入站 S2S 鉴权（旧端 `platform_api/auth.rs` 逐字搬入）。
//!
//! 只认 `[platform_auth]`（[`PLATFORM_AUTH_CONFIG`]），与校审前端的 `[review_auth]` 分离：
//! `enabled = true` 走真实 JWT 校验；`enabled = false` 仍要求请求体 token 与 `debug_token`
//! 精确相等，`debug_token` 为空则一律拒绝——不存在「关掉就放行」的档位。

use axum::http::StatusCode;
use log::warn;

use super::super::auth::{PLATFORM_AUTH_CONFIG, TokenClaims, verify_token};

/// Unified S2S token verification with claims returned.
///
/// 当 `PLATFORM_AUTH_CONFIG.enabled = false` 走 debug_token 路径，没有真正的 JWT
/// claims 可解，此时返回 `Ok(None)`；JWT 模式下返回 `Ok(Some(claims))`。
/// 调用方在 JWT 模式下可以从 claims 取出 `user_id` / `role` 等做身份推导，
/// 避免请求体重复携带 actor 字段。
pub fn verify_s2s_token_with_claims(
    token: &str,
) -> Result<Option<TokenClaims>, (StatusCode, String)> {
    if !PLATFORM_AUTH_CONFIG.enabled {
        let expected = PLATFORM_AUTH_CONFIG.debug_token.trim();
        if expected.is_empty() {
            warn!("platform_auth.enabled=false 但未配置 debug_token，拒绝 S2S 请求");
            return Err((
                StatusCode::UNAUTHORIZED,
                "platform auth debug_token 未配置".to_string(),
            ));
        }

        if token.trim() == expected {
            return Ok(None);
        }

        warn!("S2S debug_token 不匹配");
        return Err((StatusCode::UNAUTHORIZED, "invalid debug_token".to_string()));
    }

    match verify_token(token) {
        Ok(claims) => {
            if let Some(legacy_form_id) = claims.legacy_form_id.as_deref() {
                warn!(
                    "S2S JWT decoded legacy form_id={} for project_id={}, user_id={}; explicit request form_id remains authoritative",
                    legacy_form_id, claims.project_id, claims.user_id
                );
            }
            Ok(Some(claims))
        }
        Err(e) => {
            warn!("S2S JWT verification failed: {}", e);
            Err((StatusCode::UNAUTHORIZED, format!("invalid token: {}", e)))
        }
    }
}

/// Unified S2S token verification: validates JWT when auth is enabled;
/// disabled mode still requires exact debug_token match.
///
/// 该函数是 [`verify_s2s_token_with_claims`] 的薄包装，丢弃 claims，
/// 用于不需要身份信息的场景。需要 actor / role 推导的调用方请改用前者。
pub fn verify_s2s_token(token: &str) -> Result<(), (StatusCode, String)> {
    verify_s2s_token_with_claims(token).map(|_| ())
}
