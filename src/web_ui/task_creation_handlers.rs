use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::web_ui::AppState;
// 暂时注释掉 grpc_service 依赖，使用简化的实现
// use crate::grpc_service::managers::task_manager::TaskManager;
// use crate::grpc_service::types::{TaskRequest, TaskType, TaskStatus, TaskPriority};

// 简化的类型定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    DataParsingWizard,
    ModelGeneration,
    SpatialTreeGeneration,
    FullSync,
    IncrementalSync,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: String,
    pub name: String,
    pub task_type: TaskType,
    pub priority: TaskPriority,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOptions {
    pub max_concurrent: u32,
    pub timeout_seconds: Option<u32>,
    pub retry_count: u32,
}

/// 任务创建请求
#[derive(Debug, Deserialize)]
pub struct TaskCreationRequest {
    #[serde(rename = "taskName")]
    pub task_name: String,
    #[serde(rename = "taskType")]
    pub task_type: String,
    #[serde(rename = "siteId")]
    pub site_id: String,
    pub priority: String,
    pub description: Option<String>,
    pub parameters: TaskParameters,
}

/// 任务参数
#[derive(Debug, Deserialize)]
pub struct TaskParameters {
    // 解析任务参数
    #[serde(rename = "parseMode")]
    pub parse_mode: Option<String>,
    pub dbnum: Option<u32>,
    pub refno: Option<String>,

    // 模型生成参数
    #[serde(rename = "generateModels")]
    pub generate_models: Option<bool>,
    #[serde(rename = "generateMesh")]
    pub generate_mesh: Option<bool>,
    #[serde(rename = "generateSpatialTree")]
    pub generate_spatial_tree: Option<bool>,
    #[serde(rename = "applyBooleanOperation")]
    pub apply_boolean_operation: Option<bool>,
    #[serde(rename = "meshTolRatio")]
    pub mesh_tol_ratio: Option<f64>,

    // 同步任务参数
    #[serde(rename = "syncMode")]
    pub sync_mode: Option<String>,
    #[serde(rename = "targetSesno")]
    pub target_sesno: Option<u32>,

    // 通用参数
    #[serde(rename = "maxConcurrent")]
    pub max_concurrent: Option<u32>,
    #[serde(rename = "parallelProcessing")]
    pub parallel_processing: Option<bool>,
}

/// 任务创建响应
#[derive(Debug, Serialize)]
pub struct TaskCreationResponse {
    pub success: bool,
    pub task_id: String,
    pub message: String,
    pub error: Option<String>,
}

/// 部署站点信息
#[derive(Debug, Serialize)]
pub struct DeploymentSite {
    pub id: String,
    pub name: String,
    pub status: String,
    pub environment: String,
    pub description: Option<String>,
    pub config: Option<serde_json::Value>,
}

/// 任务模板
#[derive(Debug, Serialize)]
pub struct TaskTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub task_type: String,
    pub default_parameters: serde_json::Value,
    pub estimated_duration: Option<u32>,
}

/// 任务名称验证响应
#[derive(Debug, Serialize)]
pub struct TaskNameValidationResponse {
    pub available: bool,
    pub message: Option<String>,
}

/// 任务配置预览响应
#[derive(Debug, Serialize)]
pub struct TaskPreviewResponse {
    pub estimated_duration: u32,
    pub resource_requirements: ResourceRequirements,
    pub warnings: Vec<String>,
}

/// 资源需求
#[derive(Debug, Serialize)]
pub struct ResourceRequirements {
    pub memory: String,
    pub cpu: String,
    pub disk: String,
}

/// 创建任务
pub async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<TaskCreationRequest>,
) -> Result<Json<TaskCreationResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 验证请求参数
    if request.task_name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "任务名称不能为空",
                "error_type": "validation_error"
            })),
        ));
    }

    if request.site_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "站点ID不能为空",
                "error_type": "validation_error"
            })),
        ));
    }

    // 解析任务类型
    let task_type = match request.task_type.as_str() {
        "DataParsingWizard" => TaskType::DataParsingWizard,
        "ModelGeneration" => TaskType::ModelGeneration,
        "SpatialTreeGeneration" => TaskType::SpatialTreeGeneration,
        "FullSync" => TaskType::FullSync,
        "IncrementalSync" => TaskType::IncrementalSync,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("不支持的任务类型: {}", request.task_type),
                    "error_type": "validation_error"
                })),
            ));
        }
    };

    // 解析优先级
    let priority = match request.priority.as_str() {
        "Low" => TaskPriority::Low,
        "Normal" => TaskPriority::Normal,
        "High" => TaskPriority::High,
        "Critical" => TaskPriority::Critical,
        _ => TaskPriority::Normal,
    };

    // 生成任务ID
    let task_id = Uuid::new_v4().to_string();

    // 暂时使用简化的实现，直接返回成功
    // TODO: 集成真实的任务管理器
    println!("创建任务: {} (ID: {})", request.task_name, task_id);
    println!("任务类型: {:?}", task_type);
    println!("优先级: {:?}", priority);
    println!("参数: {:?}", request.parameters);

    // 保存任务信息到数据库
    if let Err(e) = save_task_to_database(&task_id, &request, &task_type).await {
        eprintln!("保存任务到数据库失败: {}", e);
        // 继续执行，这不是致命错误
    }

    Ok(Json(TaskCreationResponse {
        success: true,
        task_id,
        message: "任务创建成功".to_string(),
        error: None,
    }))
}

/// 获取部署站点列表
pub async fn get_deployment_sites(
    State(_state): State<AppState>,
) -> Result<Json<Vec<DeploymentSite>>, StatusCode> {
    // TODO: 从数据库加载部署站点
    // 临时返回模拟数据
    let sites = vec![
        DeploymentSite {
            id: "site-1".to_string(),
            name: "YCYK-E3D 开发站点".to_string(),
            status: "running".to_string(),
            environment: "dev".to_string(),
            description: Some("开发环境部署站点".to_string()),
            config: None,
        },
        DeploymentSite {
            id: "site-2".to_string(),
            name: "YCYK-E3D 测试站点".to_string(),
            status: "running".to_string(),
            environment: "test".to_string(),
            description: Some("测试环境部署站点".to_string()),
            config: None,
        },
    ];

    Ok(Json(sites))
}

/// 获取任务模板
pub async fn get_task_templates(
    State(_state): State<AppState>,
) -> Result<Json<Vec<TaskTemplate>>, StatusCode> {
    let templates = vec![
        TaskTemplate {
            id: "data-parsing".to_string(),
            name: "数据解析任务".to_string(),
            description: "解析PDMS数据库文件，提取几何和属性信息".to_string(),
            task_type: "DataParsingWizard".to_string(),
            default_parameters: serde_json::json!({
                "parse_mode": "all",
                "max_concurrent": 1,
                "parallel_processing": false
            }),
            estimated_duration: Some(1800), // 30分钟
        },
        TaskTemplate {
            id: "model-generation".to_string(),
            name: "模型生成任务".to_string(),
            description: "基于解析数据生成3D模型和网格文件".to_string(),
            task_type: "ModelGeneration".to_string(),
            default_parameters: serde_json::json!({
                "generate_models": true,
                "generate_mesh": false,
                "generate_spatial_tree": true,
                "apply_boolean_operation": true,
                "mesh_tol_ratio": 3.0,
                "max_concurrent": 1,
                "parallel_processing": false
            }),
            estimated_duration: Some(3600), // 60分钟
        },
        TaskTemplate {
            id: "spatial-tree".to_string(),
            name: "空间树生成任务".to_string(),
            description: "构建空间索引树，优化查询性能".to_string(),
            task_type: "SpatialTreeGeneration".to_string(),
            default_parameters: serde_json::json!({
                "generate_spatial_tree": true,
                "max_concurrent": 1,
                "parallel_processing": false
            }),
            estimated_duration: Some(900), // 15分钟
        },
    ];

    Ok(Json(templates))
}

/// 验证任务名称
pub async fn validate_task_name(
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<TaskNameValidationResponse>, StatusCode> {
    let task_name = match params.get("name") {
        Some(name) => name,
        None => {
            return Ok(Json(TaskNameValidationResponse {
                available: false,
                message: Some("任务名称不能为空".to_string()),
            }));
        }
    };

    if task_name.trim().is_empty() {
        return Ok(Json(TaskNameValidationResponse {
            available: false,
            message: Some("任务名称不能为空".to_string()),
        }));
    }

    // TODO: 检查数据库中是否存在同名任务
    // 临时返回可用
    Ok(Json(TaskNameValidationResponse {
        available: true,
        message: None,
    }))
}

/// 预览任务配置
pub async fn preview_task_config(
    Json(request): Json<serde_json::Value>,
) -> Result<Json<TaskPreviewResponse>, StatusCode> {
    // TODO: 根据任务类型和参数计算资源需求
    let estimated_duration = match request.get("task_type").and_then(|v| v.as_str()) {
        Some("DataParsingWizard") => 1800,    // 30分钟
        Some("ModelGeneration") => 3600,      // 60分钟
        Some("SpatialTreeGeneration") => 900, // 15分钟
        Some("FullSync") => 7200,             // 120分钟
        Some("IncrementalSync") => 1200,      // 20分钟
        _ => 1800,
    };

    let resource_requirements = ResourceRequirements {
        memory: "2GB".to_string(),
        cpu: "4 cores".to_string(),
        disk: "10GB".to_string(),
    };

    let warnings = vec![
        "建议在系统负载较低时执行此任务".to_string(),
        "确保有足够的磁盘空间存储生成的文件".to_string(),
    ];

    Ok(Json(TaskPreviewResponse {
        estimated_duration,
        resource_requirements,
        warnings,
    }))
}

/// 保存任务到数据库
async fn save_task_to_database(
    task_id: &str,
    request: &TaskCreationRequest,
    task_type: &TaskType,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: 实现数据库保存逻辑
    // 这里可以保存到SQLite或SurrealDB
    println!("保存任务到数据库: {} - {}", task_id, request.task_name);
    Ok(())
}
