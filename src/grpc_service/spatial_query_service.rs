use aios_core::pdms_types::{RefU64, RefnoEnum};
use nalgebra::Point3;
use parry3d::bounding_volume::Aabb;
use rstar::{AABB, RTree, RTreeObject};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};

use crate::grpc_service::sctn_contact_detector::{
    BatchSctnDetector, CableTraySection, ContactResult as SctnContactResult,
    ContactType as SctnContactType, SctnContactDetector, SupportRelation as SctnSupportRelation,
    SupportType as SctnSupportType,
};
use crate::grpc_service::sctn_geometry_extractor::SctnGeometryExtractor;
use crate::grpc_service::spatial_index_builder::{
    SpatialIndexBuilder, SpatialIndexConfig, SpatialIndexPersistence,
};

// 引入生成的 protobuf 代码
pub mod spatial_query {
    tonic::include_proto!("spatial_query");
}

use spatial_query::{spatial_query_service_server::SpatialQueryService, *};

/// 空间元素结构体，用于 R-star 树索引
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialElement {
    pub refno: RefU64,
    pub bbox: Aabb,
    pub element_type: String,
    pub element_name: String,
    pub last_updated: SystemTime,
}

impl RTreeObject for SpatialElement {
    type Envelope = AABB<[f32; 3]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bbox.mins.x, self.bbox.mins.y, self.bbox.mins.z],
            [self.bbox.maxs.x, self.bbox.maxs.y, self.bbox.maxs.z],
        )
    }
}

/// 空间查询服务实现
pub struct SpatialQueryServiceImpl {
    spatial_index: Arc<RwLock<RTree<SpatialElement>>>,
    last_rebuild_time: Arc<RwLock<SystemTime>>,
    db_manager: Option<Arc<crate::data_interface::tidb_manager::AiosDBManager>>,
    index_file_path: Option<std::path::PathBuf>,
}

impl SpatialQueryServiceImpl {
    pub async fn new() -> anyhow::Result<Self> {
        let spatial_index = Self::build_initial_index().await?;

        Ok(Self {
            spatial_index: Arc::new(RwLock::new(spatial_index)),
            last_rebuild_time: Arc::new(RwLock::new(SystemTime::now())),
            db_manager: None,
            index_file_path: None,
        })
    }

    /// 使用预构建的索引文件创建服务
    pub async fn from_index_file(index_file: impl AsRef<Path>) -> anyhow::Result<Self> {
        let index_path = index_file.as_ref().to_path_buf();

        if !SpatialIndexPersistence::is_valid_index_file(&index_path) {
            return Err(anyhow::anyhow!("无效的索引文件: {:?}", index_path));
        }

        let (spatial_index, _statistics) = SpatialIndexPersistence::load_index(&index_path)?;
        log::info!(
            "从文件加载空间索引: {:?}, 元素数量: {}",
            index_path,
            spatial_index.size()
        );

        Ok(Self {
            spatial_index: Arc::new(RwLock::new(spatial_index)),
            last_rebuild_time: Arc::new(RwLock::new(SystemTime::now())),
            db_manager: None,
            index_file_path: Some(index_path),
        })
    }

    /// 使用数据库管理器创建服务（支持动态重建）
    pub async fn with_db_manager(
        db_manager: Arc<crate::data_interface::tidb_manager::AiosDBManager>,
    ) -> anyhow::Result<Self> {
        let spatial_index = Self::build_initial_index().await?;

        Ok(Self {
            spatial_index: Arc::new(RwLock::new(spatial_index)),
            last_rebuild_time: Arc::new(RwLock::new(SystemTime::now())),
            db_manager: Some(db_manager),
            index_file_path: None,
        })
    }

    /// 从数据库构建索引
    pub async fn build_from_database(&self, db_nos: &[i32]) -> anyhow::Result<()> {
        if let Some(ref db_manager) = self.db_manager {
            let builder = SpatialIndexBuilder::new(db_manager.clone());
            let (new_index, statistics) = builder.build_from_database(db_nos).await?;

            // 更新索引
            let mut spatial_index = self.spatial_index.write().await;
            *spatial_index = new_index;

            let mut last_rebuild = self.last_rebuild_time.write().await;
            *last_rebuild = SystemTime::now();

            log::info!("从数据库重建索引完成: {:?}", statistics);

            // 如果设置了文件路径，保存索引
            if let Some(ref file_path) = self.index_file_path {
                if let Err(e) =
                    SpatialIndexPersistence::save_index(&spatial_index, &statistics, file_path)
                {
                    log::warn!("保存索引文件失败: {:?}, 错误: {}", file_path, e);
                }
            }

            Ok(())
        } else {
            Err(anyhow::anyhow!("未设置数据库管理器，无法从数据库构建索引"))
        }
    }

    /// 保存当前索引到文件
    pub async fn save_index_to_file(&self, file_path: impl AsRef<Path>) -> anyhow::Result<()> {
        let spatial_index = self.spatial_index.read().await;
        let statistics = crate::grpc_service::spatial_index_builder::IndexStatistics {
            total_elements: spatial_index.size(),
            indexed_elements: spatial_index.size(),
            skipped_elements: 0,
            build_time_ms: 0,
            memory_estimate_mb: 0.0,
            type_distribution: std::collections::HashMap::new(),
        };

        SpatialIndexPersistence::save_index(&spatial_index, &statistics, file_path.as_ref())?;
        log::info!("索引已保存到文件: {:?}", file_path.as_ref());
        Ok(())
    }

    /// 构建初始空间索引
    async fn build_initial_index() -> anyhow::Result<RTree<SpatialElement>> {
        let mut elements = Vec::new();

        // 模拟数据 - 在实际应用中这里应该从数据库查询
        // 创建一些测试数据
        elements.push(SpatialElement {
            refno: RefU64(1001),
            bbox: Aabb::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0)),
            element_type: "PIPE".to_string(),
            element_name: "管道001".to_string(),
            last_updated: SystemTime::now(),
        });

        elements.push(SpatialElement {
            refno: RefU64(1002),
            bbox: Aabb::new(Point3::new(0.5, 0.5, 0.5), Point3::new(1.5, 1.5, 1.5)),
            element_type: "EQUI".to_string(),
            element_name: "设备001".to_string(),
            last_updated: SystemTime::now(),
        });

        elements.push(SpatialElement {
            refno: RefU64(1003),
            bbox: Aabb::new(Point3::new(2.0, 2.0, 2.0), Point3::new(3.0, 3.0, 3.0)),
            element_type: "PIPE".to_string(),
            element_name: "管道002".to_string(),
            last_updated: SystemTime::now(),
        });

        elements.push(SpatialElement {
            refno: RefU64(1004),
            bbox: Aabb::new(Point3::new(0.8, 0.8, 0.8), Point3::new(2.2, 2.2, 2.2)),
            element_type: "STRU".to_string(),
            element_name: "结构001".to_string(),
            last_updated: SystemTime::now(),
        });

        Ok(RTree::bulk_load(elements))
    }

    /// 查询与指定参考号相交的构件
    async fn query_intersecting_elements(
        &self,
        target_refno: RefU64,
        custom_bbox: Option<Aabb>,
        element_types: &[String],
        include_self: bool,
        tolerance: f32,
        max_results: u32,
    ) -> anyhow::Result<Vec<IntersectingElement>> {
        // 1. 获取目标构件的包围盒
        let target_bbox = if let Some(bbox) = custom_bbox {
            bbox
        } else {
            self.get_element_bbox(target_refno).await?
        };

        // 2. 应用容差扩展包围盒
        let expanded_bbox = Aabb::new(
            target_bbox.mins - Point3::new(tolerance, tolerance, tolerance),
            target_bbox.maxs + Point3::new(tolerance, tolerance, tolerance),
        );

        // 3. 在空间索引中查询相交的构件（优先使用 SQLite RTree，如果启用的话）
        let target_center = (target_bbox.mins.coords + target_bbox.maxs.coords) * 0.5;
        let mut intersecting: Vec<IntersectingElement> = Vec::new();

        #[cfg(feature = "sqlite-index")]
        if crate::spatial_index::SqliteSpatialIndex::is_enabled() {
            let spatial_index = crate::spatial_index::SqliteSpatialIndex::with_default_path()
                .expect("Failed to open spatial index");
            if let Ok(ids) = spatial_index.query_intersect(&expanded_bbox) {
                for id in ids {
                    if !include_self && id == target_refno {
                        continue;
                    }
                    if let Ok(Some(bbox)) = spatial_index.get_aabb(id) {
                        let element_center = (bbox.mins.coords + bbox.maxs.coords) * 0.5;
                        let distance = (target_center - element_center).norm();
                        let intersection_volume =
                            self.calculate_intersection_volume(&target_bbox, &bbox);
                        intersecting.push(IntersectingElement {
                            refno: id.0,
                            element_type: element_types
                                .get(0)
                                .cloned()
                                .unwrap_or_else(|| "UNK".to_string()),
                            bbox: Some(BoundingBox {
                                min: Some(Point3D {
                                    x: bbox.mins.x,
                                    y: bbox.mins.y,
                                    z: bbox.mins.z,
                                }),
                                max: Some(Point3D {
                                    x: bbox.maxs.x,
                                    y: bbox.maxs.y,
                                    z: bbox.maxs.z,
                                }),
                            }),
                            intersection_volume,
                            distance_to_center: distance,
                            element_name: String::new(),
                        });
                    }
                }
            }
        }

        if intersecting.is_empty() {
            let spatial_index = self.spatial_index.read().await;
            let query_envelope = AABB::from_corners(
                [
                    expanded_bbox.mins.x,
                    expanded_bbox.mins.y,
                    expanded_bbox.mins.z,
                ],
                [
                    expanded_bbox.maxs.x,
                    expanded_bbox.maxs.y,
                    expanded_bbox.maxs.z,
                ],
            );
            intersecting = spatial_index
                .locate_in_envelope_intersecting(&query_envelope)
                .filter(|element| {
                    if !include_self && element.refno == target_refno {
                        return false;
                    }
                    if !element_types.is_empty() && !element_types.contains(&element.element_type) {
                        return false;
                    }
                    true
                })
                .map(|element| {
                    let element_center =
                        (element.bbox.mins.coords + element.bbox.maxs.coords) * 0.5;
                    let distance = (target_center - element_center).norm();
                    let intersection_volume =
                        self.calculate_intersection_volume(&target_bbox, &element.bbox);
                    IntersectingElement {
                        refno: element.refno.0,
                        element_type: element.element_type.clone(),
                        bbox: Some(BoundingBox {
                            min: Some(Point3D {
                                x: element.bbox.mins.x,
                                y: element.bbox.mins.y,
                                z: element.bbox.mins.z,
                            }),
                            max: Some(Point3D {
                                x: element.bbox.maxs.x,
                                y: element.bbox.maxs.y,
                                z: element.bbox.maxs.z,
                            }),
                        }),
                        intersection_volume,
                        distance_to_center: distance,
                        element_name: element.element_name.clone(),
                    }
                })
                .collect();
        }

        // 4. 按距离排序并限制结果数量
        intersecting.sort_by(|a, b| {
            a.distance_to_center
                .partial_cmp(&b.distance_to_center)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        intersecting.truncate(max_results as usize);

        Ok(intersecting)
    }

    /// 获取构件包围盒（从索引中查找）
    async fn get_element_bbox(&self, refno: RefU64) -> anyhow::Result<Aabb> {
        let spatial_index = self.spatial_index.read().await;

        for element in spatial_index.iter() {
            if element.refno == refno {
                return Ok(element.bbox);
            }
        }

        Err(anyhow::anyhow!("Element with refno {} not found", refno.0))
    }

    /// 计算相交体积
    fn calculate_intersection_volume(&self, bbox1: &Aabb, bbox2: &Aabb) -> f32 {
        let intersection = Aabb::new(
            Point3::new(
                bbox1.mins.x.max(bbox2.mins.x),
                bbox1.mins.y.max(bbox2.mins.y),
                bbox1.mins.z.max(bbox2.mins.z),
            ),
            Point3::new(
                bbox1.maxs.x.min(bbox2.maxs.x),
                bbox1.maxs.y.min(bbox2.maxs.y),
                bbox1.maxs.z.min(bbox2.maxs.z),
            ),
        );

        let size = intersection.maxs - intersection.mins;
        if size.x > 0.0 && size.y > 0.0 && size.z > 0.0 {
            size.x * size.y * size.z
        } else {
            0.0
        }
    }
}

#[tonic::async_trait]
impl SpatialQueryService for SpatialQueryServiceImpl {
    async fn query_intersecting_elements(
        &self,
        request: Request<SpatialQueryRequest>,
    ) -> Result<Response<SpatialQueryResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();

        let custom_bbox = req.custom_bbox.map(|bbox| {
            let min = bbox.min.unwrap_or_default();
            let max = bbox.max.unwrap_or_default();
            Aabb::new(
                Point3::new(min.x, min.y, min.z),
                Point3::new(max.x, max.y, max.z),
            )
        });

        let tolerance = if req.tolerance > 0.0 {
            req.tolerance
        } else {
            0.001
        };
        let max_results = if req.max_results > 0 {
            req.max_results
        } else {
            1000
        };

        match self
            .query_intersecting_elements(
                RefU64(req.refno),
                custom_bbox,
                &req.element_types,
                req.include_self,
                tolerance,
                max_results,
            )
            .await
        {
            Ok(elements) => {
                let query_time = start_time.elapsed().as_millis();

                Ok(Response::new(SpatialQueryResponse {
                    elements,
                    total_count: elements.len() as u32,
                    query_time_ms: query_time.to_string(),
                    success: true,
                    error_message: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(SpatialQueryResponse {
                elements: vec![],
                total_count: 0,
                query_time_ms: start_time.elapsed().as_millis().to_string(),
                success: false,
                error_message: e.to_string(),
            })),
        }
    }

    async fn batch_query_intersecting(
        &self,
        request: Request<BatchSpatialQueryRequest>,
    ) -> Result<Response<BatchSpatialQueryResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();

        let mut responses = Vec::new();
        let mut successful_queries = 0;
        let mut failed_queries = 0;

        for query_req in req.requests {
            let single_request = Request::new(query_req);
            match self.query_intersecting_elements(single_request).await {
                Ok(response) => {
                    let response_inner = response.into_inner();
                    if response_inner.success {
                        successful_queries += 1;
                    } else {
                        failed_queries += 1;
                    }
                    responses.push(response_inner);
                }
                Err(_) => {
                    failed_queries += 1;
                    responses.push(SpatialQueryResponse {
                        elements: vec![],
                        total_count: 0,
                        query_time_ms: "0".to_string(),
                        success: false,
                        error_message: "Internal error".to_string(),
                    });
                }
            }
        }

        Ok(Response::new(BatchSpatialQueryResponse {
            responses,
            total_time_ms: start_time.elapsed().as_millis().to_string(),
            successful_queries,
            failed_queries,
        }))
    }

    async fn rebuild_spatial_index(
        &self,
        _request: Request<RebuildIndexRequest>,
    ) -> Result<Response<RebuildIndexResponse>, Status> {
        let start_time = Instant::now();

        match Self::build_initial_index().await {
            Ok(new_index) => {
                let mut spatial_index = self.spatial_index.write().await;
                let indexed_elements = new_index.size();
                *spatial_index = new_index;

                let mut last_rebuild = self.last_rebuild_time.write().await;
                *last_rebuild = SystemTime::now();

                Ok(Response::new(RebuildIndexResponse {
                    success: true,
                    message: "Spatial index rebuilt successfully".to_string(),
                    indexed_elements: indexed_elements as u32,
                    rebuild_time_ms: start_time.elapsed().as_millis().to_string(),
                }))
            }
            Err(e) => Ok(Response::new(RebuildIndexResponse {
                success: false,
                message: format!("Rebuild failed: {}", e),
                indexed_elements: 0,
                rebuild_time_ms: start_time.elapsed().as_millis().to_string(),
            })),
        }
    }

    async fn get_index_stats(
        &self,
        _request: Request<IndexStatsRequest>,
    ) -> Result<Response<IndexStatsResponse>, Status> {
        let spatial_index = self.spatial_index.read().await;
        let last_rebuild = self.last_rebuild_time.read().await;

        let total_elements = spatial_index.size() as u32;
        let mut type_counts = std::collections::HashMap::new();

        for element in spatial_index.iter() {
            *type_counts.entry(element.element_type.clone()).or_insert(0) += 1;
        }

        let type_stats = type_counts
            .into_iter()
            .map(|(element_type, count)| ElementTypeStats {
                element_type,
                count,
            })
            .collect();

        let last_rebuild_time = format!("{:?}", *last_rebuild);

        Ok(Response::new(IndexStatsResponse {
            total_elements,
            indexed_elements: total_elements,
            last_rebuild_time,
            type_stats,
            index_memory_mb: 0.0, // 简化实现，实际应该计算内存使用
        }))
    }

    async fn detect_sctn_contacts(
        &self,
        request: Request<SctnContactRequest>,
    ) -> Result<Response<SctnContactResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();

        // 创建SCTN接触检测器
        let detector = match SctnContactDetector::new(req.tolerance) {
            Ok(d) => d,
            Err(e) => {
                return Ok(Response::new(SctnContactResponse {
                    contacts: vec![],
                    total_contacts: 0,
                    query_time_ms: "0".to_string(),
                    success: false,
                    error_message: format!("Failed to create detector: {}", e),
                }));
            }
        };

        // 获取SCTN的几何信息
        let sctn = match self.get_sctn_geometry(RefU64(req.sctn_refno)).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Response::new(SctnContactResponse {
                    contacts: vec![],
                    total_contacts: 0,
                    query_time_ms: "0".to_string(),
                    success: false,
                    error_message: format!("Failed to get SCTN geometry: {}", e),
                }));
            }
        };

        // 执行接触检测
        match detector
            .detect_sctn_contacts(&sctn, &req.target_types, req.include_proximity)
            .await
        {
            Ok(results) => {
                let contacts: Vec<ContactInfo> = results
                    .into_iter()
                    .take(req.max_results as usize)
                    .map(|(refno, contact)| ContactInfo {
                        target_refno: refno.0,
                        target_type: "UNKNOWN".to_string(), // TODO: 获取实际类型
                        contact_type: convert_contact_type(contact.contact_type) as i32,
                        contact_points: contact
                            .contact_points
                            .into_iter()
                            .map(|p| Point3D {
                                x: p.x,
                                y: p.y,
                                z: p.z,
                            })
                            .collect(),
                        contact_normal: Some(Vector3D {
                            x: contact.contact_normal.x,
                            y: contact.contact_normal.y,
                            z: contact.contact_normal.z,
                        }),
                        penetration_depth: contact.penetration_depth,
                        contact_area: contact.contact_area,
                        distance: contact.distance,
                    })
                    .collect();

                let total_contacts = contacts.len() as u32;
                let query_time = start_time.elapsed().as_millis();

                Ok(Response::new(SctnContactResponse {
                    contacts,
                    total_contacts,
                    query_time_ms: query_time.to_string(),
                    success: true,
                    error_message: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(SctnContactResponse {
                contacts: vec![],
                total_contacts: 0,
                query_time_ms: start_time.elapsed().as_millis().to_string(),
                success: false,
                error_message: e.to_string(),
            })),
        }
    }

    async fn batch_detect_sctn_contacts(
        &self,
        request: Request<BatchSctnContactRequest>,
    ) -> Result<Response<BatchSctnContactResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();
        let mut results = std::collections::HashMap::new();

        // 创建批量检测器
        let detector = match BatchSctnDetector::new(req.tolerance) {
            Ok(d) => d,
            Err(e) => {
                return Ok(Response::new(BatchSctnContactResponse {
                    results,
                    total_time_ms: "0".to_string(),
                    processed_count: 0,
                }));
            }
        };

        // 批量处理SCTN
        let mut processed_count = 0;
        for sctn_refno in req.sctn_refnos {
            let single_req = SctnContactRequest {
                sctn_refno,
                target_types: req.target_types.clone(),
                tolerance: req.tolerance,
                include_proximity: true,
                max_results: 100,
            };

            let single_request = Request::new(single_req);
            if let Ok(response) = self.detect_sctn_contacts(single_request).await {
                results.insert(sctn_refno, response.into_inner());
                processed_count += 1;
            }
        }

        Ok(Response::new(BatchSctnContactResponse {
            results,
            total_time_ms: start_time.elapsed().as_millis().to_string(),
            processed_count,
        }))
    }

    async fn detect_tray_supports(
        &self,
        request: Request<TraySupportRequest>,
    ) -> Result<Response<TraySupportResponse>, Status> {
        let req = request.into_inner();
        let start_time = Instant::now();

        // 创建检测器
        let detector = match SctnContactDetector::new(0.001) {
            Ok(d) => d,
            Err(e) => {
                return Ok(Response::new(TraySupportResponse {
                    relations: vec![],
                    total_supports: 0,
                    query_time_ms: "0".to_string(),
                    success: false,
                }));
            }
        };

        // 获取桥架分支下的所有SCTN
        let sctns = self
            .get_branch_sections(RefU64(req.bran_refno))
            .await
            .unwrap_or_else(|_| vec![]);

        let mut all_relations = Vec::new();

        // 检测每个SCTN的支撑关系
        for sctn in sctns {
            match detector
                .detect_support_relationships(&sctn, req.max_distance)
                .await
            {
                Ok(relations) => {
                    for rel in relations {
                        all_relations.push(SupportRelation {
                            tray_section: rel.tray_section.0,
                            support_refno: rel.support.0,
                            support_type: convert_support_type(rel.support_type) as i32,
                            contact_point: Some(Point3D {
                                x: rel.contact_point.x,
                                y: rel.contact_point.y,
                                z: rel.contact_point.z,
                            }),
                            load_distribution: rel.load_distribution,
                        });
                    }
                }
                Err(_) => continue,
            }
        }

        let total_supports = all_relations.len() as u32;
        let query_time = start_time.elapsed().as_millis();

        Ok(Response::new(TraySupportResponse {
            relations: all_relations,
            total_supports,
            query_time_ms: query_time.to_string(),
            success: true,
        }))
    }
}

// 辅助函数：转换接触类型
fn convert_contact_type(ct: SctnContactType) -> ContactType {
    match ct {
        SctnContactType::Surface => ContactType::Surface,
        SctnContactType::Edge => ContactType::Edge,
        SctnContactType::Point => ContactType::Point,
        SctnContactType::Penetration => ContactType::Penetration,
        SctnContactType::Proximity => ContactType::Proximity,
        SctnContactType::None => ContactType::None,
    }
}

// 辅助函数：转换支撑类型
fn convert_support_type(st: SctnSupportType) -> SupportType {
    match st {
        SctnSupportType::DirectSupport => SupportType::Direct,
        SctnSupportType::HangerSupport => SupportType::Hanger,
        SctnSupportType::BracketSupport => SupportType::Bracket,
        SctnSupportType::WallMount => SupportType::WallMount,
        SctnSupportType::Unknown => SupportType::Unknown,
    }
}

impl SpatialQueryServiceImpl {
    /// 获取SCTN的几何信息 - 使用真实数据
    async fn get_sctn_geometry(&self, refno: RefU64) -> anyhow::Result<CableTraySection> {
        // 使用真实的数据库管理器
        if let Some(ref db_manager) = self.db_manager {
            let extractor = SctnGeometryExtractor::new(db_manager.clone());
            return extractor.extract_sctn_geometry(refno).await;
        }

        // 如果没有数据库管理器，尝试从空间索引获取基本信息
        let spatial_index = self.spatial_index.read().await;
        for element in spatial_index.iter() {
            if element.refno == refno {
                // 从索引中只能获取基本包围盒信息
                return Ok(CableTraySection {
                    refno,
                    bbox: element.bbox.clone(),
                    centerline: vec![element.bbox.center()],
                    width: 0.3,  // 使用默认值
                    height: 0.1, // 使用默认值
                    depth: (element.bbox.maxs.z - element.bbox.mins.z).max(1.0),
                    direction: nalgebra::Vector3::new(1.0, 0.0, 0.0),
                    support_points: vec![],
                    section_type: element.element_type.clone(),
                });
            }
        }

        Err(anyhow::anyhow!("SCTN {} not found", refno.0))
    }

    /// 获取桥架分支下的所有SCTN - 使用真实数据
    async fn get_branch_sections(
        &self,
        bran_refno: RefU64,
    ) -> anyhow::Result<Vec<CableTraySection>> {
        if let Some(ref db_manager) = self.db_manager {
            let extractor = SctnGeometryExtractor::new(db_manager.clone());
            return extractor.extract_branch_sections(bran_refno).await;
        }

        // 没有数据库连接时返回空
        Ok(vec![])
    }
}
