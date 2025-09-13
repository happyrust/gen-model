use anyhow::Result;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::web_ui::models::DatabaseConfig;

/// 全局数据库连接池，按部署站点ID存储
pub static DEPLOYMENT_DB_CONNECTIONS: once_cell::sync::Lazy<Arc<RwLock<std::collections::HashMap<String, Arc<Surreal<Client>>>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(std::collections::HashMap::new())));

/// 使用部署站点配置初始化数据库连接
pub async fn init_surreal_with_config(config: &DatabaseConfig) -> Result<Arc<Surreal<Client>>> {
    // 构建连接字符串
    let address = format!("{}:{}", config.db_ip, config.db_port);
    
    println!("正在连接到 SurrealDB: {}", address);
    
    // 创建数据库连接
    let db = Surreal::new::<Ws>(address.as_str()).await?;
    
    // 认证
    db.signin(Root {
        username: &config.db_user,
        password: &config.db_password,
    })
    .await?;
    
    // 使用命名空间和数据库
    // 命名空间使用项目代码
    let namespace = config.project_code.to_string();
    let database = config.project_name.clone();
    
    db.use_ns(&namespace).use_db(&database).await?;
    
    println!("✅ 成功连接到数据库 - NS: {}, DB: {}", namespace, database);
    
    Ok(Arc::new(db))
}

/// 获取或创建部署站点的数据库连接
pub async fn get_or_create_deployment_connection(
    deployment_id: &str,
    config: &DatabaseConfig,
) -> Result<Arc<Surreal<Client>>> {
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
    // 构建连接字符串
    let address = format!("{}:{}", db_ip, db_port);
    
    println!("========== 开始测试数据库连接 ==========");
    println!("连接地址: ws://{}", address);
    println!("用户名: {}", db_user);
    println!("密码: {} (长度: {})", "*".repeat(db_password.len()), db_password.len());
    println!("命名空间(NS): {}", project_code);
    println!("数据库(DB): {}", project_name);
    
    // 创建数据库连接
    println!("1. 尝试连接到数据库服务器...");
    let db = match Surreal::new::<Ws>(address.as_str()).await {
        Ok(db) => {
            println!("   ✓ 成功连接到服务器");
            db
        }
        Err(e) => {
            println!("   ✗ 连接失败: {}", e);
            return Err(anyhow::anyhow!("无法连接到数据库服务器 ws://{}: {}", address, e));
        }
    };
    
    // 认证
    println!("2. 尝试认证...");
    match db.signin(Root {
        username: db_user,
        password: db_password,
    }).await {
        Ok(_) => {
            println!("   ✓ 认证成功");
        }
        Err(e) => {
            println!("   ✗ 认证失败: {}", e);
            println!("   请检查用户名: {} 和密码是否正确", db_user);
            
            // 检查是否是认证错误
            let error_str = e.to_string();
            if error_str.contains("Authentication") || error_str.contains("credentials") {
                return Err(anyhow::anyhow!(
                    "认证失败：用户名或密码错误\n用户名: {}\n请确认密码是否正确", 
                    db_user
                ));
            } else {
                return Err(anyhow::anyhow!("认证失败 (用户: {}): {}", db_user, e));
            }
        }
    }
    
    // 使用命名空间和数据库
    println!("3. 尝试使用命名空间 '{}' 和数据库 '{}'...", project_code, project_name);
    match db.use_ns(project_code).use_db(project_name).await {
        Ok(_) => {
            println!("   ✓ 成功切换到指定的命名空间和数据库");
        }
        Err(e) => {
            println!("   ✗ 切换失败: {}", e);
            println!("   命名空间或数据库可能不存在，需要先创建");
            return Err(anyhow::anyhow!("无法使用指定的命名空间 '{}' 和数据库 '{}': {}", 
                                      project_code, project_name, e));
        }
    }
    
    // 执行简单查询测试连接
    println!("4. 执行测试查询...");
    match db.query("SELECT 'test' as result").await {
        Ok(mut response) => {
            match response.take::<Vec<serde_json::Value>>(0) {
                Ok(_) => {
                    println!("   ✓ 查询执行成功");
                }
                Err(e) => {
                    println!("   ✗ 查询结果处理失败: {}", e);
                    // 即使查询结果处理失败，连接本身是成功的
                    println!("   注意：虽然查询结果处理有问题，但数据库连接是正常的");
                }
            }
        }
        Err(e) => {
            println!("   ✗ 查询执行失败: {}", e);
            return Err(anyhow::anyhow!("查询测试失败: {}", e));
        }
    }
    
    println!("========================================");
    println!("✅ 数据库连接测试成功！");
    println!("========================================");
    
    Ok(())
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