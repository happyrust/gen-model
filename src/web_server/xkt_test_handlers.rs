//! XKT 模型测试 API 处理器

use axum::{
    extract::{Json, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

/// XKT 生成请求
#[derive(Debug, Deserialize)]
pub struct XktGenerateRequest {
    /// 参考号列表（逗号分隔）
    pub refnos: String,
    /// 是否压缩
    #[serde(default = "default_true")]
    pub compress: bool,
    /// 是否包含子孙节点
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    /// 是否跳过 Mesh
    #[serde(default)]
    pub skip_mesh: bool,
}

/// 测试立方体生成请求
#[derive(Debug, Deserialize)]
pub struct TestCubeRequest {
    /// 是否压缩
    #[serde(default = "default_true")]
    pub compress: bool,
}

fn default_true() -> bool {
    true
}

/// XKT 生成响应
#[derive(Debug, Serialize)]
pub struct XktGenerateResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 生成的文件路径
    pub file_path: Option<String>,
    /// 文件大小（字节）
    pub file_size: Option<u64>,
    /// 统计信息
    pub stats: Option<XktStats>,
    /// 进度日志
    pub progress_logs: Option<Vec<String>>,
}

/// XKT 统计信息
#[derive(Debug, Serialize)]
pub struct XktStats {
    /// 几何体数量
    pub geometries: u32,
    /// 网格数量
    pub meshes: u32,
    /// 实体数量
    pub entities: u32,
    /// 顶点数量
    pub vertices: Option<u32>,
    /// 三角形数量
    pub triangles: Option<u32>,
}

/// XKT 验证请求
#[derive(Debug, Deserialize)]
pub struct XktValidateRequest {
    /// XKT 文件路径
    pub file_path: String,
}

/// XKT 验证响应
#[derive(Debug, Serialize)]
pub struct XktValidateResponse {
    /// 是否有效
    pub valid: bool,
    /// 文件路径
    pub file_path: String,
    /// 文件大小
    pub file_size: u64,
    /// 版本
    pub version: u32,
    /// 是否压缩
    pub compressed: bool,
    /// 统计信息
    pub statistics: XktStats,
    /// 错误信息
    pub errors: Vec<String>,
    /// 警告信息
    pub warnings: Vec<String>,
    /// 元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// XKT 文件列表项
#[derive(Debug, Serialize)]
pub struct XktFileItem {
    /// 文件名
    pub name: String,
    /// 文件路径
    pub path: String,
    /// 文件大小
    pub size: u64,
    /// 修改时间
    pub modified: String,
}

/// 生成 XKT 模型
pub async fn generate_xkt(
    Json(req): Json<XktGenerateRequest>,
) -> Result<Json<XktGenerateResponse>, (StatusCode, String)> {
    // 解析参考号
    let refnos: Vec<&str> = req.refnos.split(',').map(|s| s.trim()).collect();
    if refnos.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "参考号不能为空".to_string()));
    }

    // 构建输出文件路径
    let output_dir = PathBuf::from("output/xkt_test");
    std::fs::create_dir_all(&output_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("创建输出目录失败: {}", e),
        )
    })?;

    let refno_str = refnos.join("_");
    let compress_suffix = if req.compress {
        "compressed"
    } else {
        "uncompressed"
    };
    let output_file = output_dir.join(format!("{}_{}.xkt", refno_str, compress_suffix));

    // 构建命令
    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("--bin")
        .arg("aios-database")
        .arg("--")
        .arg("--debug-model-refnos")
        .arg(&req.refnos)
        .arg("--export-xkt")
        .arg("--export-obj-output")
        .arg(&output_file)
        .arg("--xkt-compress")
        .arg(if req.compress { "true" } else { "false" });

    if req.skip_mesh {
        cmd.arg("--xkt-skip-mesh");
    }

    // 打印完整命令（用于调试）
    println!("🔧 执行命令: {:?}", cmd);

    // 执行命令
    let output = cmd.output().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("执行命令失败: {}", e),
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("❌ XKT 生成失败:");
        println!("STDERR: {}", stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("XKT 生成失败: {}", stderr),
        ));
    }

    // 解析输出获取统计信息
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("📋 命令输出:");
    println!("{}", stdout);

    let stats = parse_xkt_stats(&stdout);

    // 提取进度日志
    let progress_logs: Vec<String> = stdout
        .lines()
        .filter(|line| {
            line.contains("收集")
                || line.contains("几何体")
                || line.contains("网格")
                || line.contains("实体")
                || line.contains("XKT")
                || line.contains("转换")
                || line.contains("压缩")
        })
        .map(|s| s.to_string())
        .collect();

    // 获取文件大小
    let file_size = std::fs::metadata(&output_file).ok().map(|m| m.len());

    // 提取相对路径（相对于 output/xkt 目录）
    let relative_path = output_file
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("xkt/{}", name))
        .unwrap_or_else(|| output_file.to_string_lossy().to_string());

    println!("📁 文件路径: {}", relative_path);

    Ok(Json(XktGenerateResponse {
        success: true,
        message: "XKT 模型生成成功".to_string(),
        file_path: Some(relative_path),
        file_size,
        stats,
        progress_logs: Some(progress_logs),
    }))
}

/// 验证 XKT 模型
pub async fn validate_xkt(
    Query(req): Query<XktValidateRequest>,
) -> Result<Json<XktValidateResponse>, (StatusCode, String)> {
    let file_path = if req.file_path.starts_with("output/") {
        PathBuf::from(&req.file_path)
    } else {
        PathBuf::from("output").join(&req.file_path)
    };

    // 检查文件是否存在
    if !file_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("文件不存在: {}", req.file_path),
        ));
    }

    // 获取文件大小
    let file_size = std::fs::metadata(&file_path)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("读取文件信息失败: {}", e),
            )
        })?
        .len();

    // 调用 Node.js 验证脚本
    let output = Command::new("node")
        .arg("validate_xkt_with_xeokit.js")
        .arg(&file_path)
        .arg("/tmp/xkt_validation.json")
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("执行验证脚本失败: {}", e),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("验证失败: {}", stderr),
        ));
    }

    // 读取验证结果
    let validation_json = std::fs::read_to_string("/tmp/xkt_validation.json").map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("读取验证结果失败: {}", e),
        )
    })?;

    let validation: serde_json::Value = serde_json::from_str(&validation_json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("解析验证结果失败: {}", e),
        )
    })?;

    // 提取统计信息
    let stats = validation
        .get("statistics")
        .and_then(|s| {
            Some(XktStats {
                geometries: s.get("geometries")?.as_u64()? as u32,
                meshes: s.get("meshes")?.as_u64()? as u32,
                entities: s.get("entities")?.as_u64()? as u32,
                vertices: s.get("vertices")?.as_u64().map(|v| v as u32),
                triangles: s.get("triangles")?.as_u64().map(|v| v as u32),
            })
        })
        .unwrap_or(XktStats {
            geometries: 0,
            meshes: 0,
            entities: 0,
            vertices: None,
            triangles: None,
        });

    // 提取 metadata
    let metadata = validation.get("metadata").cloned();

    Ok(Json(XktValidateResponse {
        valid: validation
            .get("valid")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        file_path: req.file_path,
        file_size,
        version: validation
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        compressed: validation
            .get("compressed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        statistics: stats,
        errors: validation
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        warnings: validation
            .get("warnings")
            .and_then(|w| w.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        metadata,
    }))
}

/// 列出已生成的 XKT 文件
pub async fn list_xkt_files() -> Result<Json<Vec<XktFileItem>>, (StatusCode, String)> {
    let output_dir = PathBuf::from("output/xkt_test");

    if !output_dir.exists() {
        return Ok(Json(vec![]));
    }

    let mut files = Vec::new();

    let entries = std::fs::read_dir(&output_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("读取目录失败: {}", e),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("读取文件条目失败: {}", e),
            )
        })?;

        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("xkt") {
            let metadata = entry.metadata().map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("读取文件元数据失败: {}", e),
                )
            })?;

            files.push(XktFileItem {
                name: path.file_name().unwrap().to_string_lossy().to_string(),
                path: path.to_string_lossy().to_string(),
                size: metadata.len(),
                modified: format!("{:?}", metadata.modified().ok()),
            });
        }
    }

    // 按修改时间排序（最新的在前）
    files.sort_by(|a, b| b.modified.cmp(&a.modified));

    Ok(Json(files))
}

/// 解析 XKT 统计信息
fn parse_xkt_stats(output: &str) -> Option<XktStats> {
    let mut geometries = None;
    let mut meshes = None;
    let mut entities = None;

    for line in output.lines() {
        if line.contains("几何体数量:") || line.contains("唯一几何体:") {
            if let Some(num_str) = line.split(':').nth(1) {
                geometries = num_str.trim().parse().ok();
            }
        } else if line.contains("网格数量:") {
            if let Some(num_str) = line.split(':').nth(1) {
                meshes = num_str.trim().parse().ok();
            }
        } else if line.contains("实体数量:") {
            if let Some(num_str) = line.split(':').nth(1) {
                entities = num_str.trim().parse().ok();
            }
        }
    }

    if geometries.is_some() || meshes.is_some() || entities.is_some() {
        Some(XktStats {
            geometries: geometries.unwrap_or(0),
            meshes: meshes.unwrap_or(0),
            entities: entities.unwrap_or(0),
            vertices: None,
            triangles: None,
        })
    } else {
        None
    }
}

/// 获取参考号全名称请求
#[derive(Debug, Deserialize)]
pub struct GetRefnoNameRequest {
    pub refno: String,
}

/// 获取参考号全名称响应
#[derive(Debug, Serialize)]
pub struct GetRefnoNameResponse {
    pub success: bool,
    pub refno: String,
    pub full_name: Option<String>,
}

/// 获取参考号的全名称
pub async fn get_refno_name(Query(params): Query<GetRefnoNameRequest>) -> impl IntoResponse {
    // 使用全局 SurrealDB 实例查询
    use aios_core::SUL_DB;

    let query = format!(
        "SELECT default_full_name FROM pe WHERE refno = '{}'",
        params.refno
    );

    match SUL_DB.query(&query).await {
        Ok(mut result) => {
            let records: Vec<serde_json::Value> = result.take(0).unwrap_or_default();

            let full_name = records
                .first()
                .and_then(|r: &serde_json::Value| r.get("default_full_name"))
                .and_then(|v: &serde_json::Value| v.as_str())
                .map(|s: &str| s.to_string());

            Json(GetRefnoNameResponse {
                success: true,
                refno: params.refno,
                full_name,
            })
        }
        Err(e) => {
            eprintln!("查询参考号全名称失败: {}", e);
            Json(GetRefnoNameResponse {
                success: false,
                refno: params.refno,
                full_name: None,
            })
        }
    }
}

/// 生成测试立方体
pub async fn generate_test_cube(
    Json(req): Json<TestCubeRequest>,
) -> Result<Json<XktGenerateResponse>, (StatusCode, String)> {
    println!("🔲 开始生成测试立方体...");

    // 构建输出文件路径
    let output_dir = PathBuf::from("output/xkt_test");
    std::fs::create_dir_all(&output_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("创建输出目录失败: {}", e),
        )
    })?;

    let compress_suffix = if req.compress {
        "compressed"
    } else {
        "uncompressed"
    };
    let output_file = output_dir.join(format!("test_cube_{}.xkt", compress_suffix));

    // 创建一个简单的立方体 XKT 文件
    // 立方体的 8 个顶点（原点在中心，边长 2米）
    // 注意：如果 PDMS 数据单位是 mm，需要将其转换为 m（除以 1000）
    // 但这里我们直接使用米作为单位，边长 2 米
    let positions = vec![
        -1.0, -1.0, -1.0, // 0: 左下后
        1.0, -1.0, -1.0, // 1: 右下后
        1.0, 1.0, -1.0, // 2: 右上后
        -1.0, 1.0, -1.0, // 3: 左上后
        -1.0, -1.0, 1.0, // 4: 左下前
        1.0, -1.0, 1.0, // 5: 右前下
        1.0, 1.0, 1.0, // 6: 右上前
        -1.0, 1.0, 1.0, // 7: 左上前
    ];

    // 立方体的 12 个三角形（每个面 2 个三角形）
    let indices = vec![
        // 后面
        0, 1, 2, 2, 3, 0, // 前面
        4, 7, 6, 6, 5, 4, // 左面
        0, 3, 7, 7, 4, 0, // 右面
        1, 5, 6, 6, 2, 1, // 底面
        0, 4, 5, 5, 1, 0, // 顶面
        3, 2, 6, 6, 7, 3,
    ];

    // 法向量（每个顶点一个）
    let normals = vec![
        -0.5773503, -0.5773503, -0.5773503, // 0
        0.5773503, -0.5773503, -0.5773503, // 1
        0.5773503, 0.5773503, -0.5773503, // 2
        -0.5773503, 0.5773503, -0.5773503, // 3
        -0.5773503, -0.5773503, 0.5773503, // 4
        0.5773503, -0.5773503, 0.5773503, // 5
        0.5773503, 0.5773503, 0.5773503, // 6
        -0.5773503, 0.5773503, 0.5773503, // 7
    ];

    // 构建 XKT 文件数据
    let xkt_data = build_xkt_file(&positions, &normals, &indices, req.compress)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("构建 XKT 文件失败: {}", e),
            )
        })?;

    // 写入文件
    std::fs::write(&output_file, &xkt_data).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("写入文件失败: {}", e),
        )
    })?;

    // 获取文件大小
    let file_size = std::fs::metadata(&output_file).ok().map(|m| m.len());

    // 提取相对路径 - 修正为正确的路径格式
    let relative_path = output_file
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("xkt_test/{}", name))
        .unwrap_or_else(|| output_file.to_string_lossy().to_string());

    println!("✅ 测试立方体已生成: {}", relative_path);
    println!("📁 文件大小: {} bytes", file_size.unwrap_or(0));

    Ok(Json(XktGenerateResponse {
        success: true,
        message: "测试立方体生成成功".to_string(),
        file_path: Some(relative_path),
        file_size,
        stats: Some(XktStats {
            geometries: 1,
            meshes: 1,
            entities: 1,
            vertices: Some(8),
            triangles: Some(12),
        }),
        progress_logs: Some(vec![
            "创建立方体几何体".to_string(),
            "生成 8 个顶点和 12 个三角形".to_string(),
            "写入 XKT 文件".to_string(),
        ]),
    }))
}

/// 构建 XKT 文件数据（使用 gen-xkt 库）
async fn build_xkt_file(
    positions: &[f32],
    normals: &[f32],
    indices: &[u32],
    compress: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use gen_xkt::prelude::*;

    // 创建 XKT 文件
    let mut xkt_file = XKTFile::new();

    // 设置元数据
    xkt_file.model.metadata.title = "Test Cube".to_string();
    xkt_file.model.metadata.author = "gen-model".to_string();
    xkt_file.model.metadata.created = chrono::Utc::now().to_rfc3339();

    // 创建立方体几何体
    let mut geometry = XKTGeometry::new("cube_geometry".to_string(), XKTGeometryType::Triangles);
    geometry.positions = positions.to_vec();
    geometry.normals = Some(normals.to_vec());
    geometry.indices = indices.to_vec();

    // 添加到文件
    xkt_file.model.create_geometry(geometry)?;

    // 创建立方体网格
    let mut mesh = XKTMesh::new("cube_mesh".to_string(), "cube_geometry".to_string());
    mesh.color = glam::Vec3::new(1.0, 0.0, 0.0); // 红色
    mesh.opacity = 1.0;
    mesh.metallic = 0.5; // 金属度 50%
    mesh.roughness = 0.3; // 粗糙度 30%（较低，更光滑）

    xkt_file.model.create_mesh(mesh)?;

    // 创建立方体实体
    let mut entity = XKTEntity::new(
        "cube_entity".to_string(),
        "Cube".to_string(),
        "CUBE".to_string(),
    );
    entity.add_mesh("cube_mesh".to_string());

    xkt_file.model.create_entity(entity)?;

    // 完成模型构建
    xkt_file.model.finalize().await?;

    // 编码为二进制数据
    let encoded = xkt_file.to_bytes(compress)?;

    Ok(encoded)
}
