use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::remote_sync_handlers::{RemoteSyncEnv, RemoteSyncSite};

/// 拓扑配置数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyData {
    pub environments: Vec<RemoteSyncEnv>,
    pub sites: Vec<RemoteSyncSite>,
    pub connections: Vec<TopologyConnection>,
}

/// 拓扑连接关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyConnection {
    pub env_id: String,
    pub site_id: String,
}

/// 拓扑验证错误
#[derive(Debug, Serialize)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

/// API 响应
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub status: String,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            status: "success".to_string(),
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            data: None,
            error: Some(message.into()),
        }
    }
}

impl TopologyData {
    /// 验证拓扑配置的有效性
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // 验证环境节点
        for env in &self.environments {
            if env.name.trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("environment.{}.name", env.id),
                    message: "环境名称不能为空".to_string(),
                });
            }

            if env.mqtt_host.is_none() || env.mqtt_host.as_ref().unwrap().trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("environment.{}.mqtt_host", env.id),
                    message: format!("环境 {} 缺少 MQTT 配置", env.name),
                });
            }

            if env.file_server_host.is_none()
                || env.file_server_host.as_ref().unwrap().trim().is_empty()
            {
                errors.push(ValidationError {
                    field: format!("environment.{}.file_server_host", env.id),
                    message: format!("环境 {} 缺少文件服务器配置", env.name),
                });
            }

            if env.location_dbs.is_none() || env.location_dbs.as_ref().unwrap().trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("environment.{}.location_dbs", env.id),
                    message: format!("环境 {} 缺少数据库编号配置", env.name),
                });
            }
        }

        // 验证站点节点
        for site in &self.sites {
            if site.name.trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("site.{}.name", site.id),
                    message: "站点名称不能为空".to_string(),
                });
            }

            // 验证站点必须关联到环境
            if !self.environments.iter().any(|e| e.id == site.env_id) {
                errors.push(ValidationError {
                    field: format!("site.{}.env_id", site.id),
                    message: format!("站点 {} 关联的环境不存在", site.name),
                });
            }

            if site.dbnums.is_none() || site.dbnums.as_ref().unwrap().trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("site.{}.dbnums", site.id),
                    message: format!("站点 {} 缺少数据库编号配置", site.name),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// 获取拓扑配置
pub async fn get_topology() -> Result<Json<ApiResponse<TopologyData>>, StatusCode> {
    // 从数据库读取环境和站点配置
    match load_topology_from_db().await {
        Ok(topology) => Ok(Json(ApiResponse::success(topology))),
        Err(e) => {
            eprintln!("获取拓扑配置失败: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// 保存拓扑配置
pub async fn save_topology(
    Json(topology): Json<TopologyData>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    // 验证拓扑
    if let Err(errors) = topology.validate() {
        let error_msg = errors
            .iter()
            .map(|e| format!("{}: {}", e.field, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err((StatusCode::BAD_REQUEST, Json(ApiResponse::error(error_msg))));
    }

    // 保存到数据库
    match save_topology_to_db(&topology).await {
        Ok(_) => Ok(Json(ApiResponse::success("拓扑配置保存成功".to_string()))),
        Err(e) => {
            eprintln!("保存拓扑配置失败: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!("保存失败: {}", e))),
            ))
        }
    }
}

/// 删除拓扑配置
pub async fn delete_topology() -> Result<Json<ApiResponse<String>>, StatusCode> {
    match delete_topology_from_db().await {
        Ok(_) => Ok(Json(ApiResponse::success("拓扑配置已删除".to_string()))),
        Err(e) => {
            eprintln!("删除拓扑配置失败: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// 从数据库加载拓扑配置
async fn load_topology_from_db() -> anyhow::Result<TopologyData> {
    use crate::web_server::remote_sync_handlers::{list_all_envs, list_all_sites};

    let environments = list_all_envs().await?;
    let sites = list_all_sites().await?;

    // 生成连接关系（基于 site.env_id）
    let connections = sites
        .iter()
        .map(|site| TopologyConnection {
            env_id: site.env_id.clone(),
            site_id: site.id.clone(),
        })
        .collect();

    Ok(TopologyData {
        environments,
        sites,
        connections,
    })
}

/// 保存拓扑配置到数据库
async fn save_topology_to_db(topology: &TopologyData) -> anyhow::Result<()> {
    use crate::web_server::remote_sync_handlers::{
        create_or_update_env, create_or_update_site, delete_all_envs, delete_all_sites,
    };

    // 清空现有配置
    delete_all_envs().await?;
    delete_all_sites().await?;

    // 保存环境
    for env in &topology.environments {
        create_or_update_env(env).await?;
    }

    // 保存站点
    for site in &topology.sites {
        create_or_update_site(site).await?;
    }

    Ok(())
}

/// 从数据库删除拓扑配置
async fn delete_topology_from_db() -> anyhow::Result<()> {
    use crate::web_server::remote_sync_handlers::{delete_all_envs, delete_all_sites};

    delete_all_sites().await?;
    delete_all_envs().await?;

    Ok(())
}
