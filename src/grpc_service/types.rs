//! GRPC服务公共类型定义

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 任务状态枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

/// 任务类型枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    FullSync,
    IncrementalSync,
    ModelGeneration,
    SpatialTreeGeneration,
}

/// 任务优先级
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for TaskPriority {
    fn default() -> Self {
        TaskPriority::Normal
    }
}

/// 进度更新信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub task_id: String,
    pub progress: f32,
    pub status: TaskStatus,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub details: Option<ProgressDetails>,
}

/// 进度详细信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressDetails {
    pub current_step: String,
    pub total_steps: u32,
    pub current_step_index: u32,
    pub processed_items: u64,
    pub total_items: u64,
    pub errors: Vec<String>,
}

/// 任务进度信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task_id: String,
    pub progress: f32,
    pub status: TaskStatus,
    pub message: String,
    pub start_time: DateTime<Utc>,
    pub estimated_completion: Option<DateTime<Utc>>,
    pub details: Option<ProgressDetails>,
}

/// MDB信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdbInfo {
    pub name: String,
    pub refno: u64,
    pub path: String,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub db_files: Vec<DbFileInfo>,
    pub metadata: MdbMetadata,
}

/// DB文件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbFileInfo {
    pub db_num: u32,
    pub name: String,
    pub size: u64,
    pub status: DbFileStatus,
}

/// DB文件状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DbFileStatus {
    Available,
    Processing,
    Completed,
    Error(String),
}

/// MDB元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdbMetadata {
    pub version: String,
    pub description: String,
    pub tags: Vec<String>,
    pub properties: HashMap<String, String>,
}

/// 任务请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: String,
    pub task_type: TaskType,
    pub mdb_name: String,
    pub db_files: Vec<u32>,
    pub options: TaskOptions,
    pub priority: TaskPriority,
}

/// 任务选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOptions {
    pub enable_logging: bool,
    pub generate_models: bool,
    pub build_spatial_tree: bool,
    pub sync_team_data: bool,
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            enable_logging: true,
            generate_models: false,
            build_spatial_tree: false,
            sync_team_data: false,
        }
    }
}
