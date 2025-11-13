use aios_core::{
    ConnectionConfig as CoreConnectionConfig, ConnectionHandle, connect_with_config,
    test_database_connection as core_test_database_connection,
};
use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::web_server::models::DatabaseConfig;

/// 全局数据库连接池，按部署站点ID存储
pub static DEPLOYMENT_DB_CONNECTIONS: Lazy<
    Arc<RwLock<std::collections::HashMap<String, Arc<ConnectionHandle>>>>,
> = Lazy::new(|| Arc::new(RwLock::new(std::collections::HashMap::new())));

fn build_connection_config(config: &DatabaseConfig) -> Result<CoreConnectionConfig> {
    let port = config
        .db_port
        .parse::<u16>()
        .map_err(|_| anyhow!("数据库端口不是有效的数字: {}", config.db_port))?;

    let namespace = if config.surreal_ns > 0 {
        Some(config.surreal_ns.to_string())
    } else {
        Some(config.project_code.to_string())
    };

    Ok(CoreConnectionConfig {
        host: config.db_ip.clone(),
        port,
        username: config.db_user.clone(),
        password: config.db_password.clone(),
        namespace,
        database: Some(config.project_name.clone()),
        secure: false,
    })
}

/// 使用部署站点配置初始化数据库连接
pub async fn init_surreal_with_config(config: &DatabaseConfig) -> Result<Arc<ConnectionHandle>> {
    let core_cfg = build_connection_config(config)?;
    let namespace_label = core_cfg
        .namespace
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.project_code.to_string());
    let database_label = core_cfg
        .database
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.project_name.clone());

    println!("🔧 正在初始化数据库连接...");
    println!("📄 配置名称: {}", config.name);
    println!("🌐 连接服务器: {}:{}", core_cfg.host, core_cfg.port);
    println!("🏷️  命名空间: {}", namespace_label);
    println!("💾 数据库名: {}", database_label);
    println!("👤 用户名: {}", core_cfg.username);

    let handle = connect_with_config(&core_cfg).await?;

    println!("✅ 数据库连接成功！");

    Ok(Arc::new(handle))
}

/// 获取或创建部署站点的数据库连接
pub async fn get_or_create_deployment_connection(
    deployment_id: &str,
    config: &DatabaseConfig,
) -> Result<Arc<ConnectionHandle>> {
    let mut connections = DEPLOYMENT_DB_CONNECTIONS.write().await;

    // 检查是否已有连接
    if let Some(existing) = connections.get(deployment_id) {
        println!("使用现有数据库连接: {}", deployment_id);
        return Ok(existing.clone());
    }

    // 创建新连接
    println!("创建新的数据库连接: {}", deployment_id);
    let connection = init_surreal_with_config(config).await?;
    connections.insert(deployment_id.to_string(), connection.clone());

    Ok(connection)
}

/// 测试数据库连接（用于界面的测试连接功能）
pub async fn test_database_connection(
    db_ip: &str,
    db_port: &str,
    db_user: &str,
    db_password: &str,
    project_code: &str,
    project_name: &str,
) -> Result<()> {
    let port = db_port
        .parse::<u16>()
        .map_err(|_| anyhow!("数据库端口不是有效的数字: {}", db_port))?;
    let namespace = project_code.trim();
    let database = project_name.trim();
    let connection_config = CoreConnectionConfig {
        host: db_ip.to_string(),
        port,
        username: db_user.to_string(),
        password: db_password.to_string(),
        namespace: if namespace.is_empty() {
            None
        } else {
            Some(namespace.to_string())
        },
        database: if database.is_empty() {
            None
        } else {
            Some(database.to_string())
        },
        secure: false,
    };

    core_test_database_connection(&connection_config).await
}

/// 清理部署站点的数据库连接
pub async fn cleanup_deployment_connection(deployment_id: &str) {
    let mut connections = DEPLOYMENT_DB_CONNECTIONS.write().await;
    if connections.remove(deployment_id).is_some() {
        println!("已清理数据库连接: {}", deployment_id);
    }
}

/// 清理所有数据库连接
pub async fn cleanup_all_connections() {
    let mut connections = DEPLOYMENT_DB_CONNECTIONS.write().await;
    connections.clear();
    println!("已清理所有数据库连接");
}
