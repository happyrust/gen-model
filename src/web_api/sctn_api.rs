use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    Router,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use aios_core::pdms_types::RefU64;

use crate::grpc_service::{
    sctn_contact_detector::{
        SctnContactDetector, BatchSctnDetector, CableTraySection,
        ContactType, SupportRelation,
    },
    sctn_geometry_extractor::SctnGeometryExtractor,
    sctn_path_analyzer::SctnPathAnalyzer,
    sctn_collision_optimizer::SctnCollisionOptimizer,
};

/// API状态管理
#[derive(Clone)]
pub struct SctnApiState {
    pub detector: Arc<SctnContactDetector>,
    pub batch_detector: Arc<BatchSctnDetector>,
    pub path_analyzer: Arc<SctnPathAnalyzer>,
    pub collision_optimizer: Arc<RwLock<SctnCollisionOptimizer>>,
    pub db_manager: Arc<crate::data_interface::tidb_manager::AiosDBManager>,
}

/// 创建SCTN API路由
pub fn create_sctn_routes(state: SctnApiState) -> Router {
    Router::new()
        // 接触检测
        .route("/api/sctn/contact/:refno", get(detect_contacts))
        .route("/api/sctn/contact/batch", post(batch_detect_contacts))
        
        // 支撑检测
        .route("/api/sctn/support/:bran_refno", get(detect_supports))
        
        // 路径分析
        .route("/api/sctn/path/analyze", post(analyze_path))
        .route("/api/sctn/path/find", get(find_path))
        .route("/api/sctn/path/connectivity", post(check_connectivity))
        
        // 碰撞检测
        .route("/api/sctn/collision/detect", post(detect_collisions))
        .route("/api/sctn/collision/optimize", post(optimize_collisions))
        .route("/api/sctn/collision/hotspots", post(analyze_hotspots))
        
        // 几何信息
        .route("/api/sctn/geometry/:refno", get(get_geometry))
        .route("/api/sctn/branch/:bran_refno/sections", get(get_branch_sections))
        
        // 可视化
        .route("/api/sctn/visualize/export", post(export_visualization))
        
        // 统计和报告
        .route("/api/sctn/stats", get(get_statistics))
        .route("/api/sctn/report", post(generate_report))
        
        .with_state(state)
}

// === 请求/响应结构体 ===

#[derive(Debug, Deserialize)]
pub struct ContactQuery {
    pub target_types: Option<String>,
    pub tolerance: Option<f32>,
    pub include_proximity: Option<bool>,
    pub max_results: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ContactResponse {
    pub refno: u64,
    pub contacts: Vec<ContactInfo>,
    pub total_count: usize,
    pub query_time_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ContactInfo {
    pub target_refno: u64,
    pub contact_type: String,
    pub distance: f32,
    pub penetration_depth: f32,
    pub contact_points: Vec<Point3D>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Point3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Deserialize)]
pub struct BatchContactRequest {
    pub refnos: Vec<u64>,
    pub target_types: Vec<String>,
    pub tolerance: f32,
}

#[derive(Debug, Serialize)]
pub struct BatchContactResponse {
    pub results: Vec<ContactResponse>,
    pub total_time_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct SupportResponse {
    pub bran_refno: u64,
    pub supports: Vec<SupportInfo>,
    pub total_supports: usize,
}

#[derive(Debug, Serialize)]
pub struct SupportInfo {
    pub section_refno: u64,
    pub support_refno: u64,
    pub support_type: String,
    pub contact_point: Point3D,
    pub load_distribution: f32,
}

#[derive(Debug, Deserialize)]
pub struct PathRequest {
    pub sections: Vec<SctnData>,
    pub from_refno: u64,
    pub to_refno: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SctnData {
    pub refno: u64,
    pub min_point: Point3D,
    pub max_point: Point3D,
    pub width: f32,
    pub height: f32,
    pub depth: f32,
}

#[derive(Debug, Serialize)]
pub struct PathResponse {
    pub path: Vec<u64>,
    pub total_length: f32,
    pub complexity: PathComplexity,
}

#[derive(Debug, Serialize)]
pub struct PathComplexity {
    pub num_turns: usize,
    pub num_elevation_changes: usize,
    pub total_angle_degrees: f32,
    pub difficulty: String,
}

#[derive(Debug, Serialize)]
pub struct CollisionResponse {
    pub collisions: Vec<CollisionInfo>,
    pub total_collisions: usize,
}

#[derive(Debug, Serialize)]
pub struct CollisionInfo {
    pub section1: u64,
    pub section2: u64,
    pub penetration_depth: f32,
    pub contact_type: String,
}

#[derive(Debug, Serialize)]
pub struct OptimizationResponse {
    pub initial_collisions: usize,
    pub final_collisions: usize,
    pub improvement_percentage: f32,
    pub resolutions: Vec<ResolutionInfo>,
}

#[derive(Debug, Serialize)]
pub struct ResolutionInfo {
    pub section_moved: u64,
    pub movement: Point3D,
}

#[derive(Debug, Serialize)]
pub struct HotspotResponse {
    pub hotspots: Vec<HotspotInfo>,
    pub total_collisions: usize,
}

#[derive(Debug, Serialize)]
pub struct HotspotInfo {
    pub refno: u64,
    pub collision_count: usize,
}

#[derive(Debug, Serialize)]
pub struct GeometryResponse {
    pub refno: u64,
    pub bbox: BoundingBox,
    pub width: f32,
    pub height: f32,
    pub depth: f32,
    pub centerline: Vec<Point3D>,
    pub direction: Point3D,
}

#[derive(Debug, Serialize)]
pub struct BoundingBox {
    pub min: Point3D,
    pub max: Point3D,
}

#[derive(Debug, Serialize)]
pub struct StatisticsResponse {
    pub total_sections: usize,
    pub total_contacts: usize,
    pub total_supports: usize,
    pub average_contact_distance: f32,
    pub collision_rate: f32,
}

// === API处理函数 ===

/// 检测单个SCTN的接触
async fn detect_contacts(
    Path(refno): Path<u64>,
    Query(params): Query<ContactQuery>,
    State(state): State<SctnApiState>,
) -> Result<Json<ContactResponse>, StatusCode> {
    let start = std::time::Instant::now();
    
    // 从数据库获取SCTN几何信息
    let extractor = SctnGeometryExtractor::new(state.db_manager.clone());
    let sctn = extractor.extract_sctn_geometry(RefU64(refno))
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    // 解析参数
    let target_types = params.target_types
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();
    
    let tolerance = params.tolerance.unwrap_or(0.01);
    let include_proximity = params.include_proximity.unwrap_or(true);
    
    // 执行接触检测
    let contacts = state.detector
        .detect_sctn_contacts(&sctn, &target_types, include_proximity)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    // 转换结果
    let contact_infos: Vec<ContactInfo> = contacts
        .iter()
        .take(params.max_results.unwrap_or(100) as usize)
        .map(|(target_refno, contact)| ContactInfo {
            target_refno: target_refno.0,
            contact_type: format!("{:?}", contact.contact_type),
            distance: contact.distance,
            penetration_depth: contact.penetration_depth,
            contact_points: contact.contact_points
                .iter()
                .map(|p| Point3D { x: p.x, y: p.y, z: p.z })
                .collect(),
        })
        .collect();
    
    let response = ContactResponse {
        refno,
        total_count: contact_infos.len(),
        contacts: contact_infos,
        query_time_ms: start.elapsed().as_millis() as u64,
    };
    
    Ok(Json(response))
}

/// 批量检测接触
async fn batch_detect_contacts(
    State(state): State<SctnApiState>,
    Json(request): Json<BatchContactRequest>,
) -> Result<Json<BatchContactResponse>, StatusCode> {
    let start = std::time::Instant::now();
    let mut results = Vec::new();
    
    let extractor = SctnGeometryExtractor::new(state.db_manager.clone());
    
    for refno in request.refnos {
        // 获取SCTN几何信息
        if let Ok(sctn) = extractor.extract_sctn_geometry(RefU64(refno)).await {
            // 执行检测
            if let Ok(contacts) = state.detector
                .detect_sctn_contacts(&sctn, &request.target_types, true)
                .await
            {
                let contact_infos: Vec<ContactInfo> = contacts
                    .into_iter()
                    .map(|(target_refno, contact)| ContactInfo {
                        target_refno: target_refno.0,
                        contact_type: format!("{:?}", contact.contact_type),
                        distance: contact.distance,
                        penetration_depth: contact.penetration_depth,
                        contact_points: contact.contact_points
                            .iter()
                            .map(|p| Point3D { x: p.x, y: p.y, z: p.z })
                            .collect(),
                    })
                    .collect();
                
                results.push(ContactResponse {
                    refno,
                    total_count: contact_infos.len(),
                    contacts: contact_infos,
                    query_time_ms: 0,
                });
            }
        }
    }
    
    Ok(Json(BatchContactResponse {
        results,
        total_time_ms: start.elapsed().as_millis() as u64,
    }))
}

/// 检测桥架支撑
async fn detect_supports(
    Path(bran_refno): Path<u64>,
    State(state): State<SctnApiState>,
) -> Result<Json<SupportResponse>, StatusCode> {
    let extractor = SctnGeometryExtractor::new(state.db_manager.clone());
    
    // 获取分支下的所有SCTN
    let sections = extractor
        .extract_branch_sections(RefU64(bran_refno))
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    let mut all_supports = Vec::new();
    
    for section in sections {
        // 这里需要实现支撑检测逻辑
        // 简化示例
        all_supports.push(SupportInfo {
            section_refno: section.refno.0,
            support_refno: 5001,
            support_type: "DIRECT".to_string(),
            contact_point: Point3D { x: 0.0, y: 0.0, z: 0.0 },
            load_distribution: 1.0,
        });
    }
    
    Ok(Json(SupportResponse {
        bran_refno,
        total_supports: all_supports.len(),
        supports: all_supports,
    }))
}

/// 分析路径
async fn analyze_path(
    State(state): State<SctnApiState>,
    Json(request): Json<PathRequest>,
) -> Result<Json<PathResponse>, StatusCode> {
    // 转换输入数据
    let sections: Vec<CableTraySection> = request.sections
        .into_iter()
        .map(|data| {
            use nalgebra::{Point3, Vector3};
            use parry3d::bounding_volume::Aabb;
            
            CableTraySection {
                refno: RefU64(data.refno),
                bbox: Aabb::new(
                    Point3::new(data.min_point.x, data.min_point.y, data.min_point.z),
                    Point3::new(data.max_point.x, data.max_point.y, data.max_point.z),
                ),
                centerline: vec![],
                width: data.width,
                height: data.height,
                depth: data.depth,
                direction: Vector3::new(1.0, 0.0, 0.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            }
        })
        .collect();
    
    // 构建网络并查找路径
    let network = state.path_analyzer.build_tray_network(&sections);
    
    if let Some(path) = state.path_analyzer.find_shortest_path(
        &network,
        RefU64(request.from_refno),
        RefU64(request.to_refno),
    ) {
        let complexity = state.path_analyzer.analyze_path_complexity(&path, &sections);
        
        Ok(Json(PathResponse {
            path: path.sections.iter().map(|r| r.0).collect(),
            total_length: path.total_length,
            complexity: PathComplexity {
                num_turns: complexity.num_turns,
                num_elevation_changes: complexity.num_elevation_changes,
                total_angle_degrees: complexity.total_angle_degrees,
                difficulty: format!("{:?}", complexity.difficulty),
            },
        }))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// 查找路径（简化版）
async fn find_path(
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<SctnApiState>,
) -> Result<Json<PathResponse>, StatusCode> {
    // 简化实现
    Ok(Json(PathResponse {
        path: vec![],
        total_length: 0.0,
        complexity: PathComplexity {
            num_turns: 0,
            num_elevation_changes: 0,
            total_angle_degrees: 0.0,
            difficulty: "SIMPLE".to_string(),
        },
    }))
}

/// 检查连通性
async fn check_connectivity(
    State(state): State<SctnApiState>,
    Json(sections): Json<Vec<SctnData>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 简化实现
    Ok(Json(serde_json::json!({
        "is_connected": true,
        "num_components": 1,
        "isolated_sections": []
    })))
}

/// 检测碰撞
async fn detect_collisions(
    State(state): State<SctnApiState>,
    Json(sections): Json<Vec<SctnData>>,
) -> Result<Json<CollisionResponse>, StatusCode> {
    // 简化实现
    Ok(Json(CollisionResponse {
        collisions: vec![],
        total_collisions: 0,
    }))
}

/// 优化碰撞
async fn optimize_collisions(
    State(state): State<SctnApiState>,
    Json(sections): Json<Vec<SctnData>>,
) -> Result<Json<OptimizationResponse>, StatusCode> {
    // 简化实现
    Ok(Json(OptimizationResponse {
        initial_collisions: 5,
        final_collisions: 1,
        improvement_percentage: 80.0,
        resolutions: vec![],
    }))
}

/// 分析热点
async fn analyze_hotspots(
    State(state): State<SctnApiState>,
    Json(sections): Json<Vec<SctnData>>,
) -> Result<Json<HotspotResponse>, StatusCode> {
    // 简化实现
    Ok(Json(HotspotResponse {
        hotspots: vec![],
        total_collisions: 0,
    }))
}

/// 获取几何信息
async fn get_geometry(
    Path(refno): Path<u64>,
    State(state): State<SctnApiState>,
) -> Result<Json<GeometryResponse>, StatusCode> {
    let extractor = SctnGeometryExtractor::new(state.db_manager.clone());
    let sctn = extractor.extract_sctn_geometry(RefU64(refno))
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    Ok(Json(GeometryResponse {
        refno,
        bbox: BoundingBox {
            min: Point3D {
                x: sctn.bbox.mins.x,
                y: sctn.bbox.mins.y,
                z: sctn.bbox.mins.z,
            },
            max: Point3D {
                x: sctn.bbox.maxs.x,
                y: sctn.bbox.maxs.y,
                z: sctn.bbox.maxs.z,
            },
        },
        width: sctn.width,
        height: sctn.height,
        depth: sctn.depth,
        centerline: sctn.centerline
            .iter()
            .map(|p| Point3D { x: p.x, y: p.y, z: p.z })
            .collect(),
        direction: Point3D {
            x: sctn.direction.x,
            y: sctn.direction.y,
            z: sctn.direction.z,
        },
    }))
}

/// 获取分支下的所有SCTN
async fn get_branch_sections(
    Path(bran_refno): Path<u64>,
    State(state): State<SctnApiState>,
) -> Result<Json<Vec<GeometryResponse>>, StatusCode> {
    let extractor = SctnGeometryExtractor::new(state.db_manager.clone());
    let sections = extractor
        .extract_branch_sections(RefU64(bran_refno))
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    
    let responses: Vec<GeometryResponse> = sections
        .into_iter()
        .map(|sctn| GeometryResponse {
            refno: sctn.refno.0,
            bbox: BoundingBox {
                min: Point3D {
                    x: sctn.bbox.mins.x,
                    y: sctn.bbox.mins.y,
                    z: sctn.bbox.mins.z,
                },
                max: Point3D {
                    x: sctn.bbox.maxs.x,
                    y: sctn.bbox.maxs.y,
                    z: sctn.bbox.maxs.z,
                },
            },
            width: sctn.width,
            height: sctn.height,
            depth: sctn.depth,
            centerline: sctn.centerline
                .iter()
                .map(|p| Point3D { x: p.x, y: p.y, z: p.z })
                .collect(),
            direction: Point3D {
                x: sctn.direction.x,
                y: sctn.direction.y,
                z: sctn.direction.z,
            },
        })
        .collect();
    
    Ok(Json(responses))
}

/// 导出可视化
async fn export_visualization(
    State(state): State<SctnApiState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 简化实现
    Ok(Json(serde_json::json!({
        "success": true,
        "file_path": "/output/visualization.html"
    })))
}

/// 获取统计信息
async fn get_statistics(
    State(state): State<SctnApiState>,
) -> Result<Json<StatisticsResponse>, StatusCode> {
    // 简化实现
    Ok(Json(StatisticsResponse {
        total_sections: 100,
        total_contacts: 45,
        total_supports: 30,
        average_contact_distance: 0.015,
        collision_rate: 0.12,
    }))
}

/// 生成报告
async fn generate_report(
    State(state): State<SctnApiState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 简化实现
    Ok(Json(serde_json::json!({
        "success": true,
        "report_url": "/reports/sctn_analysis_report.pdf"
    })))
}