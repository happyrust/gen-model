//! 平台前端配置（旧端 `platform_api/config.rs`：embed URL 的相对路径与前端基址）。
//!
//! 与旧端的两处差异，都来自计划 `docs/plans/2026-09-12-review-backend-port-plan.md`：
//! - **Q5**：旧端 `embed_url.rs:467` 经 `web_server.site_id` 查站点注册表
//!   `managed_project_sites::get_site(..).viewer_url` 拿校审前端地址，gen-model 没有站点注册表，
//!   改为配置项 **`[platform_auth].review_web_base_url`**（`DbOptionExt`，T1 在 `options.rs`
//!   的 [`crate::options::PlatformAuthSection`] 里加的那一格，经 [`crate::options::platform_auth`] 读）。
//! - 旧端最后一档「由 `web_server.port` / `web_server.bind_host` 推导本机地址」改为按 gen-model 的
//!   监听地址 `http_api_addr` 推导（同一语义：没配任何前端地址时回退到「后端所在主机」）。
//!
//! 取值优先级（[`PlatformConfig::frontend_base_url`]）沿用旧端：`web_server.frontend_url` →
//! `model_center.frontend_base_url` → `web_server.public_base_url` → 推导；`review_web_base_url`
//! 单独放在 [`PlatformConfig::review_web_base_url`]，由 `embed_url` 按 Q5 的位置插进它的链里。

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use super::super::auth::load_config;

/// 平台前端配置
#[derive(Clone, Debug)]
pub struct PlatformConfig {
    pub frontend_relative_path: String,
    /// 前端基地址（用于拼接完整 URL），为空时不返回 url 字段
    pub frontend_base_url: String,
    /// 校审 Web 基址（Q5，替代旧端站点注册表的 `viewer_url`）。未配置为 `None`。
    pub review_web_base_url: Option<String>,
    /// 后端对外基址（旧端 `web_server.public_base_url` / `web_server.backend_url`），
    /// 用于 workflow/sync 响应里附件的 `public_url`；未配置则不回传该字段。
    pub web_public_base_url: Option<String>,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            frontend_relative_path: "/review/3d-view".to_string(),
            frontend_base_url: String::new(),
            review_web_base_url: None,
            web_public_base_url: None,
        }
    }
}

impl PlatformConfig {
    pub fn from_config_file() -> Self {
        // Q5：`[platform_auth].review_web_base_url`（DbOptionExt）。空白 = 未配置，归一交给 embed_url。
        let review_web_base_url = crate::options::platform_auth()
            .review_web_base_url()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        if let Some(config) = load_config() {
            return Self {
                frontend_base_url: resolve_frontend_base_url(&config),
                review_web_base_url,
                web_public_base_url: get_nonempty_config_string(&config, "web_server.public_base_url")
                    .or_else(|| get_nonempty_config_string(&config, "web_server.backend_url")),
                ..Self::default()
            };
        }
        Self {
            review_web_base_url,
            ..Self::default()
        }
    }
}

fn resolve_frontend_base_url(config: &config::Config) -> String {
    get_nonempty_config_string(config, "web_server.frontend_url")
        .or_else(|| get_nonempty_config_string(config, "model_center.frontend_base_url"))
        .or_else(|| get_nonempty_config_string(config, "web_server.public_base_url"))
        .or_else(|| derive_frontend_url_from_http_api_addr(config))
        .unwrap_or_default()
}

fn get_nonempty_config_string(config: &config::Config, key: &str) -> Option<String> {
    config
        .get_string(key)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// 旧端 `derive_frontend_url_from_web_server`：gen-model 的监听地址是顶层 `http_api_addr`
/// （`host:port`），这里取它的 host / port 做同样的推导。
fn derive_frontend_url_from_http_api_addr(config: &config::Config) -> Option<String> {
    let addr = get_nonempty_config_string(config, "http_api_addr")?;
    let (bind_host, port) = split_host_port(&addr)?;
    let access_host = access_host_from_bind_host(bind_host);
    Some(format!("http://{}:{}", url_host(&access_host), port))
}

/// 把 `host:port` / `[v6]:port` 拆开；端口非法或为 0 视为未配置。
fn split_host_port(addr: &str) -> Option<(&str, u16)> {
    let trimmed = addr.trim();
    let (host, port) = if let Some(rest) = trimmed.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        (host, tail.strip_prefix(':')?)
    } else {
        trimmed.rsplit_once(':')?
    };
    let port = port.trim().parse::<u16>().ok().filter(|value| *value > 0)?;
    Some((host, port))
}

fn access_host_from_bind_host(bind_host: &str) -> String {
    let trimmed = bind_host.trim();
    if !is_loopback_or_unspecified_host(trimmed) {
        return trimmed.to_string();
    }

    get_local_ip_via_udp().unwrap_or_else(|err| {
        log::warn!("无法推断本机真实 IPv4: {err}");
        "0.0.0.0".to_string()
    })
}

fn is_loopback_or_unspecified_host(host: &str) -> bool {
    let normalized = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "" | "0.0.0.0" | "::" | "127.0.0.1" | "localhost" | "::1"
    ) {
        return true;
    }
    normalized
        .parse::<IpAddr>()
        .map(|ip| ip.is_loopback() || ip.is_unspecified())
        .unwrap_or(false)
}

fn get_local_ip_via_udp() -> Result<String, std::io::Error> {
    fn host_from_url_or_host(value: &str) -> Option<&str> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let without_scheme = trimmed
            .strip_prefix("http://")
            .or_else(|| trimmed.strip_prefix("https://"))
            .unwrap_or(trimmed);
        without_scheme
            .split('/')
            .next()
            .and_then(|host_port| host_port.split(':').next())
            .map(str::trim)
            .filter(|host| !host.is_empty())
    }

    for key in [
        "AIOS_PUBLIC_HOST",
        "AIOS_LOCAL_IP",
        "AIOS_VIEWER_HOST",
        "AIOS_VIEWER_BASE_URL",
    ] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let Some(host) = host_from_url_or_host(&value) else {
            continue;
        };
        if let Ok(IpAddr::V4(ipv4)) = host.parse::<IpAddr>() {
            if !ipv4.is_unspecified() && !ipv4.is_loopback() {
                return Ok(ipv4.to_string());
            }
        }
    }

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80))?;
    match socket.local_addr()?.ip() {
        IpAddr::V4(ipv4) if !ipv4.is_unspecified() && !ipv4.is_loopback() => Ok(ipv4.to_string()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "无法推断本机真实 IPv4，请设置 AIOS_PUBLIC_HOST 或 AIOS_LOCAL_IP",
        )),
    }
}

fn url_host(host: &str) -> String {
    let trimmed = host.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.contains(':') {
        format!("[{trimmed}]")
    } else {
        trimmed.to_string()
    }
}

lazy_static::lazy_static! {
    pub static ref PLATFORM_CONFIG: PlatformConfig = PlatformConfig::from_config_file();
}

#[cfg(test)]
mod tests {
    use super::split_host_port;

    #[test]
    fn split_host_port_handles_v4_v6_and_rejects_zero_port() {
        assert_eq!(split_host_port("0.0.0.0:8022"), Some(("0.0.0.0", 8022)));
        assert_eq!(split_host_port("[::]:8022"), Some(("::", 8022)));
        assert_eq!(split_host_port("192.168.1.9:80"), Some(("192.168.1.9", 80)));
        assert_eq!(split_host_port("0.0.0.0:0"), None);
        assert_eq!(split_host_port("no-port"), None);
    }
}
