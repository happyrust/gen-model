//! MDB管理器
//!
//! 负责管理MDB文件列表和相关信息

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::types::{DbFileInfo, DbFileStatus, MdbInfo, MdbMetadata};
use chrono::{DateTime, Utc};
use dashmap::DashMap;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// MDB管理器
#[derive(Debug)]
pub struct MdbManager {
    /// 数据库连接池
    // db_pool: Arc<Pool<MySql>>, // 暂时注释掉，使用 aios_core 接口
    /// 缓存的MDB列表
    cached_mdb_list: Arc<RwLock<Vec<MdbInfo>>>,
    /// 最后更新时间
    last_update: Arc<RwLock<DateTime<Utc>>>,
    /// 缓存过期时间（秒）
    cache_duration: i64,
}

impl MdbManager {
    /// 创建新的MDB管理器
    pub fn new() -> Self {
        Self {
            // db_pool,
            cached_mdb_list: Arc::new(RwLock::new(Vec::new())),
            last_update: Arc::new(RwLock::new(DateTime::<Utc>::MIN_UTC)),
            cache_duration: 300, // 5分钟缓存
        }
    }

    /// 获取MDB列表
    pub async fn get_mdb_list(&self) -> ServiceResult<Vec<MdbInfo>> {
        // 检查缓存是否过期
        let last_update = *self.last_update.read().await;
        let now = Utc::now();

        if (now - last_update).num_seconds() < self.cache_duration {
            // 返回缓存的数据
            let cached_list = self.cached_mdb_list.read().await;
            return Ok(cached_list.clone());
        }

        // 重新加载MDB列表
        self.reload_mdb_list().await
    }

    /// 重新加载MDB列表
    async fn reload_mdb_list(&self) -> ServiceResult<Vec<MdbInfo>> {
        // 使用现有的get_project_mdb函数
        // TODO: 使用 aios_core 接口查询项目数据库
        let mdb_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        let mut mdb_list = Vec::new();

        for (mdb_name, db_nums) in mdb_map.iter() {
            let db_files: Vec<DbFileInfo> = db_nums
                .iter()
                .map(|&db_num| DbFileInfo {
                    db_num,
                    name: format!("DB_{}", db_num),
                    size: 0, // TODO: 获取实际文件大小
                    status: DbFileStatus::Available,
                })
                .collect();

            let mdb_info = MdbInfo {
                name: mdb_name.clone(),
                refno: 0, // TODO: 从数据库获取实际refno
                path: format!("/{}", mdb_name),
                size: 0,                 // TODO: 计算总大小
                created_at: Utc::now(),  // TODO: 获取实际创建时间
                modified_at: Utc::now(), // TODO: 获取实际修改时间
                db_files,
                metadata: MdbMetadata {
                    version: "1.0".to_string(),
                    description: format!("MDB: {}", mdb_name),
                    tags: vec!["pdms".to_string()],
                    properties: HashMap::new(),
                },
            };

            mdb_list.push(mdb_info);
        }

        // 更新缓存
        {
            let mut cached_list = self.cached_mdb_list.write().await;
            *cached_list = mdb_list.clone();
        }
        {
            let mut last_update = self.last_update.write().await;
            *last_update = Utc::now();
        }

        Ok(mdb_list)
    }

    /// 获取MDB详情
    pub async fn get_mdb_details(&self, mdb_name: &str) -> ServiceResult<Option<MdbInfo>> {
        let mdb_list = self.get_mdb_list().await?;

        Ok(mdb_list.into_iter().find(|mdb| mdb.name == mdb_name))
    }

    /// 检查MDB是否存在
    pub async fn mdb_exists(&self, mdb_name: &str) -> ServiceResult<bool> {
        let mdb_list = self.get_mdb_list().await?;
        Ok(mdb_list.iter().any(|mdb| mdb.name == mdb_name))
    }

    /// 强制刷新缓存
    pub async fn refresh_cache(&self) -> ServiceResult<()> {
        self.reload_mdb_list().await?;
        Ok(())
    }
}
