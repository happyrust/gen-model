// 导出管理模块
//
// 负责处理模型导出相关的 HTTP 请求，支持 GLTF/GLB/XKT 格式

use aios_core::{RefU64, RefnoEnum};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Json, Response},
};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path as StdPath, PathBuf};
use uuid::Uuid;

use crate::fast_model::{
    export_glb::GlbExporter,
    export_gltf::GltfExporter,
    export_model::model_exporter::{
        CommonExportConfig, GlbExportConfig, GltfExportConfig, ModelExporter, XktExportConfig,
    },
    export_xkt::XktExporter,
    model_exporter::ExportStats,
    unit_converter::UnitConverter,
};

use crate::web_server::AppState;

// ================= 数据结构定义 =================

/// 导出请求结构体
#[derive(Debug, Deserialize, Clone)]
pub struct ExportRequest {
    /// 要导出的参考号列表
    pub refnos: Vec<String>,

    /// 导出格式 (gltf/glb/xkt)
    pub format: String,

    /// 可选的输出文件名（不含扩展名）
    pub file_name: Option<String>,

    /// 是否包含子孙节点（默认true）
    pub include_descendants: Option<bool>,

    /// 过滤名词列表（可选）
    pub filter_nouns: Option<Vec<String>>,

    /// 是否使用基础材质（不使用PBR，默认false）
    pub use_basic_materials: Option<bool>,

    /// Mesh文件目录（可选，默认使用配置中的路径）
    pub mesh_dir: Option<String>,
}

/// 导出响应结构体
#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub success: bool,
    pub task_id: String,
    pub message: String,
    pub export_stats: Option<serde_json::Value>,
}

/// 导出状态查询响应
#[derive(Debug, Serialize)]
pub struct ExportStatusResponse {
    pub task_id: String,
    pub status: String, // pending/running/completed/failed
    pub progress: Option<u8>,
    pub message: Option<String>,
    pub result_url: Option<String>,
    pub error: Option<String>,
}

/// 导出进度信息结构体
#[derive(Debug, Clone)]
struct ExportProgress {
    task_id: String,
    status: String,
    progress: u8,
    message: String,
    result_path: Option<PathBuf>,
    error: Option<String>,
    export_stats: Option<serde_json::Value>,
}

/// 全局导出任务存储
static EXPORT_TASKS: Lazy<DashMap<String, ExportProgress>> =
    Lazy::new(|| DashMap::new());

// ================= API 处理器 =================

/// 创建导出任务（异步）
pub async fn create_export_task(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 验证请求
    if request.refnos.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "参考号列表不能为空"})),
        ));
    }

    let format = request.format.to_lowercase();
    if !["gltf", "glb", "xkt"].contains(&format.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "不支持的导出格式，支持的格式: gltf, glb, xkt"})),
        ));
    }

    // 解析参考号
    let mut parsed_refnos = Vec::new();
    for refno_str in &request.refnos {
        match refno_str.parse::<u64>() {
            Ok(num) => parsed_refnos.push(RefnoEnum::Refno(RefU64(num))),
            Err(_) => {
                // 尝试解析 RefnoEnum 格式（如果有的话）
                // 这里可以添加更复杂的解析逻辑
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("无效的参考号格式: {}", refno_str)})),
                ));
            }
        }
    }

    // 生成任务ID
    let task_id = Uuid::new_v4().to_string();

    // 创建导出进度记录
    let progress = ExportProgress {
        task_id: task_id.clone(),
        status: "pending".to_string(),
        progress: 0,
        message: "任务已创建".to_string(),
        result_path: None,
        error: None,
        export_stats: None,
    };
    EXPORT_TASKS.insert(task_id.clone(), progress);

    // 异步执行导出任务
    let task_id_clone = task_id.clone();
    let request_clone = request.clone();
    let mesh_dir = request
        .mesh_dir
        .as_ref()
        .map(|s| StdPath::new(s).to_path_buf())
        .unwrap_or_else(|| StdPath::new("assets/meshes").to_path_buf());

    tokio::spawn(async move {
        execute_export_task(task_id_clone, request_clone, parsed_refnos, mesh_dir).await;
    });

    Ok(Json(ExportResponse {
        success: true,
        task_id,
        message: "导出任务已创建，正在后台执行".to_string(),
        export_stats: None,
    }))
}

/// 执行导出任务的异步函数
async fn execute_export_task(
    task_id: String,
    request: ExportRequest,
    refnos: Vec<RefnoEnum>,
    mesh_dir: PathBuf,
) {
    // 更新状态为运行中
    {
        let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
        progress.status = "running".to_string();
        progress.progress = 10;
        progress.message = "开始导出...".to_string();
    }

    // 构建配置
    let common_config = CommonExportConfig {
        include_descendants: request.include_descendants.unwrap_or(true),
        filter_nouns: request.filter_nouns,
        verbose: true,
        unit_converter: UnitConverter::default(),
        use_basic_materials: request.use_basic_materials.unwrap_or(false),
    };

    // 生成输出文件路径
    let timestamp_str = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let file_name = request
        .file_name
        .unwrap_or_else(|| format!("export_{}_{}", request.format.to_lowercase(), timestamp_str));

    // 创建临时输出目录
    let output_dir = StdPath::new("exports").join(&timestamp_str);
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
        progress.status = "failed".to_string();
        progress.error = Some(format!("创建输出目录失败: {}", e));
        return;
    }

    let output_path = output_dir.join(format!("{}.{}", file_name, request.format));

    // 根据格式选择导出器
    let export_result: Result<ExportStats, String> = match request.format.to_lowercase().as_str() {
        "gltf" => {
            let config = GltfExportConfig {
                common: common_config,
            };
            let exporter = GltfExporter::new();
            match exporter
                .export(&refnos, &mesh_dir, output_path.to_str().unwrap(), config)
                .await
            {
                Ok(stats) => Ok(stats),
                Err(e) => Err(e.to_string()),
            }
        }
        "glb" => {
            let config = GlbExportConfig {
                common: common_config,
            };
            let exporter = GlbExporter::new();
            match exporter
                .export(&refnos, &mesh_dir, output_path.to_str().unwrap(), config)
                .await
            {
                Ok(result) => Ok(result.stats),
                Err(e) => Err(e.to_string()),
            }
        }
        "xkt" => {
            let config = XktExportConfig {
                common: common_config,
                compress: true,
                validate: false,
                skip_mesh: false,
                db_config: None,
                dbno: None,
            };
            let exporter = XktExporter::new();
            match exporter
                .export(&refnos, &mesh_dir, output_path.to_str().unwrap(), config)
                .await
            {
                Ok(stats) => Ok(stats),
                Err(e) => Err(e.to_string()),
            }
        }
        _ => Err("不支持的格式".to_string()),
    };

    match export_result {
        Ok(stats) => {
            // 序列化统计信息
            let stats_json = serde_json::json!({
                "refno_count": stats.refno_count,
                "descendant_count": stats.descendant_count,
                "geometry_count": stats.geometry_count,
                "mesh_files_found": stats.mesh_files_found,
                "mesh_files_missing": stats.mesh_files_missing
            });

            let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
            progress.status = "completed".to_string();
            progress.progress = 100;
            progress.message = "导出完成".to_string();
            progress.result_path = Some(output_path);

            // 存储统计信息
            progress.export_stats = Some(stats_json);
        }
        Err(e) => {
            let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
            progress.status = "failed".to_string();
            progress.error = Some(e);
        }
    }
}

/// 查询导出任务状态
pub async fn get_export_status(
    Path(task_id): Path<String>,
) -> Result<Json<ExportStatusResponse>, StatusCode> {
    match EXPORT_TASKS.get(&task_id) {
        Some(progress) => {
            let progress = progress.clone();

            let status_response = ExportStatusResponse {
                task_id: progress.task_id.clone(),
                status: progress.status,
                progress: Some(progress.progress),
                message: Some(progress.message),
                result_url: progress.result_path.as_ref().and_then(|p| {
                    p.to_str().map(|s| {
                        format!(
                            "/api/export/download/{}?path={}",
                            progress.task_id,
                            urlencoding::encode(s)
                        )
                    })
                }),
                error: progress.error,
            };

            Ok(Json(status_response))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// 下载导出结果文件
pub async fn download_export(
    Path(task_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response<Body>, StatusCode> {
    // 查询任务状态
    let progress = match EXPORT_TASKS.get(&task_id) {
        Some(p) => p.clone(),
        None => return Err(StatusCode::NOT_FOUND),
    };

    // 检查任务是否完成
    if progress.status != "completed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 获取文件路径
    let file_path = match params.get("path") {
        Some(p) => {
            // URL解码
            let decoded = urlencoding::decode(p).map_err(|_| StatusCode::BAD_REQUEST)?;
            PathBuf::from(decoded.into_owned())
        }
        None => {
            // 如果没有提供路径，尝试从结果路径获取
            progress.result_path.ok_or(StatusCode::BAD_REQUEST)?
        }
    };

    // 检查文件是否存在
    if !file_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // 读取文件
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // 获取MIME类型
    let mime_type = if file_path.extension().and_then(|s| s.to_str()) == Some("gltf") {
        "model/gltf+json"
    } else if file_path.extension().and_then(|s| s.to_str()) == Some("glb") {
        "model/gltf-binary"
    } else if file_path.extension().and_then(|s| s.to_str()) == Some("xkt") {
        "model/xkt"
    } else {
        "application/octet-stream"
    };

    // 构建响应
    let disposition = format!(
        "attachment; filename=\"{}\"",
        file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("export")
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// 列出导出任务
pub async fn list_export_tasks(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let status_filter = params.get("status");
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50);

    let mut tasks = Vec::new();
    for entry in EXPORT_TASKS.iter() {
        let progress = entry.value();

        // 应用状态过滤
        if let Some(filter) = status_filter {
            if &progress.status != filter {
                continue;
            }
        }

        tasks.push(serde_json::json!({
            "task_id": progress.task_id,
            "status": progress.status,
            "progress": progress.progress,
            "message": progress.message,
            "result_path": progress.result_path.as_ref().and_then(|p| p.to_str()),
            "error": progress.error,
        }));

        if tasks.len() >= limit {
            break;
        }
    }

    Ok(Json(serde_json::json!({
        "tasks": tasks,
        "total": tasks.len()
    })))
}

/// 清理完成的导出任务
pub async fn cleanup_export_tasks() -> Result<Json<serde_json::Value>, StatusCode> {
    let mut removed_count = 0;
    let _now = chrono::Utc::now();

    let tasks_to_remove: Vec<String> = EXPORT_TASKS
        .iter()
        .filter_map(|entry| {
            let progress = entry.value();
            // 只保留最近1小时的任务
            if progress.status == "completed" || progress.status == "failed" {
                // 这里简化处理，实际应该检查时间戳
                // 暂时不自动删除
                None
            } else {
                None
            }
        })
        .collect();

    for task_id in tasks_to_remove {
        EXPORT_TASKS.remove(&task_id);
        removed_count += 1;
    }

    Ok(Json(json!({
        "success": true,
        "removed_count": removed_count,
        "message": format!("清理了 {} 个任务", removed_count)
    })))
}
