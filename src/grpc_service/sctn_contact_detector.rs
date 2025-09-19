use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use parry3d::query::{Contact, PointQuery, Ray, contact};
use parry3d::shape::{Ball, Cuboid};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::spatial_query_service::SpatialElement;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::grpc_service::sctn_geometry_extractor::SctnGeometryExtractor;
use crate::spatial_index::{
    QueryOptions, SortBy, SortOrder, SpatialQueryBackend, SqliteSpatialIndex,
};

/// 桥架截面（SCTN）数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CableTraySection {
    pub refno: RefU64,
    pub bbox: Aabb,
    pub centerline: Vec<Point3<f32>>,     // 桥架中心线
    pub width: f32,                       // 桥架宽度
    pub height: f32,                      // 桥架高度
    pub depth: f32,                       // 桥架深度/长度
    pub direction: Vector3<f32>,          // 桥架走向
    pub support_points: Vec<Point3<f32>>, // 支撑点
    pub section_type: String,             // 截面类型
}

/// 接触类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContactType {
    Surface,     // 表面接触
    Edge,        // 边缘接触
    Point,       // 点接触
    Penetration, // 穿透
    Proximity,   // 接近（在容差范围内）
    None,        // 无接触
}

/// 接触检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactResult {
    pub contact_type: ContactType,
    pub contact_points: Vec<Point3<f32>>,
    pub contact_normal: Vector3<f32>,
    pub penetration_depth: f32,
    pub contact_area: f32,
    pub distance: f32,
}

/// 支撑关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportRelation {
    pub tray_section: RefU64,
    pub support: RefU64,
    pub support_type: SupportType,
    pub contact_point: Point3<f32>,
    pub load_distribution: f32,
}

/// 支撑类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SupportType {
    DirectSupport,  // 直接支撑
    HangerSupport,  // 吊架支撑
    BracketSupport, // 托架支撑
    WallMount,      // 墙装支撑
    Unknown,
}

/// SCTN接触检测器
pub struct SctnContactDetector {
    spatial_index: Option<SqliteSpatialIndex>,
    db_manager: Option<Arc<AiosDBManager>>,
    tolerance: f32,
}

impl SctnContactDetector {
    /// 创建新的检测器
    pub fn new(tolerance: f32) -> Result<Self> {
        let spatial_index = if SqliteSpatialIndex::is_enabled() {
            Some(SqliteSpatialIndex::with_default_path()?)
        } else {
            None
        };

        Ok(Self {
            spatial_index,
            db_manager: None,
            tolerance,
        })
    }

    /// 创建带数据库管理器的检测器
    pub fn with_db_manager(tolerance: f32, db_manager: Arc<AiosDBManager>) -> Result<Self> {
        let spatial_index = if SqliteSpatialIndex::is_enabled() {
            Some(SqliteSpatialIndex::with_default_path()?)
        } else {
            None
        };

        Ok(Self {
            spatial_index,
            db_manager: Some(db_manager),
            tolerance,
        })
    }

    /// 使用指定的空间索引构造（便于测试或自定义索引路径）
    pub fn with_index(tolerance: f32, index: SqliteSpatialIndex) -> Result<Self> {
        Ok(Self {
            spatial_index: Some(index),
            db_manager: None,
            tolerance,
        })
    }

    /// 检测SCTN与其他构件的接触
    pub async fn detect_sctn_contacts(
        &self,
        sctn: &CableTraySection,
        target_types: &[String],
        include_proximity: bool,
    ) -> Result<Vec<(RefU64, ContactResult)>> {
        // 步骤1: 扩展包围盒进行粗筛选
        let expanded_bbox = self.expand_bbox(&sctn.bbox, self.tolerance);

        // 步骤2: 使用空间索引查询候选构件
        let candidates = self.query_candidates(&expanded_bbox, target_types).await?;

        // 步骤3: 精确接触检测
        let mut contacts = Vec::new();
        for candidate in candidates {
            if let Some(contact) =
                self.check_detailed_contact(sctn, &candidate, include_proximity)?
            {
                contacts.push((candidate.refno, contact));
            }
        }

        // 步骤4: 按距离排序
        contacts.sort_by(|a, b| {
            a.1.distance
                .partial_cmp(&b.1.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(contacts)
    }

    /// 检测桥架与支架的支撑关系
    pub async fn detect_support_relationships(
        &self,
        sctn: &CableTraySection,
        max_distance: f32,
    ) -> Result<Vec<SupportRelation>> {
        let mut relations = Vec::new();

        // 沿桥架底部创建多个检测点
        let check_points = self.generate_support_check_points(sctn);

        for point in check_points {
            // 向下投影检测支撑
            let ray = Ray::new(point, -Vector3::y());

            // 查找下方的支架构件
            if let Some(support) = self.raycast_to_support(ray, max_distance).await? {
                // 验证支撑关系
                if self.verify_support_relation(sctn, &support)? {
                    relations.push(SupportRelation {
                        tray_section: sctn.refno,
                        support: support.refno,
                        support_type: self.classify_support_type(&support),
                        contact_point: support.contact_point,
                        load_distribution: self.calculate_load_distribution(sctn, &support),
                    });
                }
            }
        }

        Ok(relations)
    }

    /// 扩展包围盒
    fn expand_bbox(&self, bbox: &Aabb, tolerance: f32) -> Aabb {
        Aabb::new(
            bbox.mins - Vector3::new(tolerance, tolerance, tolerance),
            bbox.maxs + Vector3::new(tolerance, tolerance, tolerance),
        )
    }

    /// 查询候选构件 - 使用真实数据
    async fn query_candidates(
        &self,
        bbox: &Aabb,
        target_types: &[String],
    ) -> Result<Vec<SpatialElement>> {
        let mut candidates = Vec::new();

        // 使用SQLite索引查询
        if let Some(ref index) = self.spatial_index {
            // 使用统一后端能力：相交 + 类型过滤 + 返回AABB
            let mut opts = QueryOptions::default();
            if !target_types.is_empty() {
                opts.types = target_types.to_vec();
            }
            opts.include_bbox = true;
            // 为了稳定输出，按id排序
            opts.sort = Some(SortBy::Id(SortOrder::Asc));
            let hits = index.query_intersect_hits(bbox, &opts)?;

            // 使用数据库管理器获取真实数据
            if let Some(ref db_manager) = self.db_manager {
                for hit in hits {
                    let refno = hit.refno;
                    // 获取构件类型
                    let element_type = db_manager.get_type_name(refno).await;

                    // 如果指定了类型过滤，检查类型
                    if !target_types.is_empty() && !target_types.contains(&element_type) {
                        continue;
                    }

                    // 获取属性
                    if let Ok(attrs) = db_manager.get_attr(refno).await {
                        let element_name = attrs
                            .get("NAME")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&format!("Element_{}", refno.0))
                            .to_string();

                        // 获取包围盒
                        let element_bbox = if let Some(bb) = hit.bbox {
                            bb
                        } else {
                            index.get_aabb(refno)?.unwrap_or_else(|| bbox.clone())
                        };

                        candidates.push(SpatialElement {
                            refno,
                            bbox: element_bbox,
                            element_type,
                            element_name,
                            last_updated: std::time::SystemTime::now(),
                        });
                    }
                }
            } else {
                // 没有数据库管理器时，使用默认值
                for hit in hits {
                    candidates.push(SpatialElement {
                        refno: hit.refno,
                        bbox: hit.bbox.unwrap_or_else(|| bbox.clone()),
                        element_type: "UNKNOWN".to_string(),
                        element_name: format!("Element_{}", hit.refno.0),
                        last_updated: std::time::SystemTime::now(),
                    });
                }
            }
        }

        Ok(candidates)
    }

    /// 详细接触检测
    pub fn check_detailed_contact(
        &self,
        sctn: &CableTraySection,
        target: &SpatialElement,
        include_proximity: bool,
    ) -> Result<Option<ContactResult>> {
        // 创建桥架的立方体形状
        let sctn_cuboid = Cuboid::new(Vector3::new(
            sctn.width / 2.0,
            sctn.height / 2.0,
            sctn.depth / 2.0,
        ));

        // 创建目标的立方体形状
        let target_size = target.bbox.maxs - target.bbox.mins;
        let target_cuboid = Cuboid::new(Vector3::new(
            target_size.x / 2.0,
            target_size.y / 2.0,
            target_size.z / 2.0,
        ));

        // 保存target_size供后续使用
        let sctn_size = Vector3::new(sctn.width, sctn.height, sctn.depth);

        // 计算位置和方向
        let sctn_pos = Isometry3::translation(
            sctn.bbox.center().x,
            sctn.bbox.center().y,
            sctn.bbox.center().z,
        );

        let target_pos = Isometry3::translation(
            target.bbox.center().x,
            target.bbox.center().y,
            target.bbox.center().z,
        );

        // 检测接触
        let contact_result = contact(
            &sctn_pos,
            &sctn_cuboid,
            &target_pos,
            &target_cuboid,
            self.tolerance,
        );

        if let Ok(Some(c)) = contact_result {
            // 分析接触类型
            let contact_type = self.analyze_contact_type(&c, sctn, target);
            let distance = (sctn.bbox.center() - target.bbox.center()).norm();

            return Ok(Some(ContactResult {
                contact_type,
                contact_points: vec![c.point1.into(), c.point2.into()],
                contact_normal: c.normal1,
                penetration_depth: c.dist.abs(),
                contact_area: self.estimate_contact_area(&c, sctn, target),
                distance,
            }));
        }

        // 检测接近关系
        if include_proximity {
            // 使用距离检测代替proximity函数
            let distance = (sctn.bbox.center() - target.bbox.center()).norm();
            let max_extent = (sctn_size.norm() + target_size.norm()) / 2.0;

            if distance < max_extent + self.tolerance {
                return Ok(Some(ContactResult {
                    contact_type: ContactType::Proximity,
                    contact_points: vec![],
                    contact_normal: Vector3::zeros(),
                    penetration_depth: 0.0,
                    contact_area: 0.0,
                    distance,
                }));
            }
        }

        Ok(None)
    }

    /// 分析接触类型
    fn analyze_contact_type(
        &self,
        contact: &Contact,
        _sctn: &CableTraySection,
        _target: &SpatialElement,
    ) -> ContactType {
        if contact.dist < -self.tolerance {
            ContactType::Penetration
        } else if contact.dist.abs() < 0.001 {
            ContactType::Surface
        } else if contact.dist < self.tolerance {
            ContactType::Proximity
        } else {
            ContactType::None
        }
    }

    /// 估算接触面积
    fn estimate_contact_area(
        &self,
        _contact: &Contact,
        sctn: &CableTraySection,
        target: &SpatialElement,
    ) -> f32 {
        // 简化计算：使用包围盒重叠面积估算
        let overlap_x = (sctn.bbox.maxs.x.min(target.bbox.maxs.x)
            - sctn.bbox.mins.x.max(target.bbox.mins.x))
        .max(0.0);
        let overlap_y = (sctn.bbox.maxs.y.min(target.bbox.maxs.y)
            - sctn.bbox.mins.y.max(target.bbox.mins.y))
        .max(0.0);
        let overlap_z = (sctn.bbox.maxs.z.min(target.bbox.maxs.z)
            - sctn.bbox.mins.z.max(target.bbox.mins.z))
        .max(0.0);

        // 返回最小的两个维度的乘积作为接触面积估算
        let mut dims = vec![overlap_x, overlap_y, overlap_z];
        dims.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dims[0] * dims[1]
    }

    /// 生成支撑检测点
    fn generate_support_check_points(&self, sctn: &CableTraySection) -> Vec<Point3<f32>> {
        let mut points = Vec::new();
        let bottom_y = sctn.bbox.mins.y;

        // 沿桥架长度方向生成多个检测点
        let num_points = 5;
        for i in 0..num_points {
            let t = i as f32 / (num_points - 1) as f32;
            let x = sctn.bbox.mins.x + t * (sctn.bbox.maxs.x - sctn.bbox.mins.x);
            let z = sctn.bbox.mins.z + t * (sctn.bbox.maxs.z - sctn.bbox.mins.z);
            points.push(Point3::new(x, bottom_y, z));
        }

        points
    }

    /// 射线投射检测支撑
    async fn raycast_to_support(
        &self,
        ray: Ray,
        max_distance: f32,
    ) -> Result<Option<SupportDetectionResult>> {
        // 需要空间索引支持
        let index = if let Some(ref idx) = self.spatial_index {
            idx
        } else {
            return Ok(None);
        };

        // 约定支架类型标识为 "SUPPO"
        let mut opts = QueryOptions::default();
        opts.types = vec!["SUPPO".to_string()];
        opts.limit = Some(1);
        opts.include_bbox = true;

        let origin = ray.origin;
        let dir = ray.dir;

        let hits = index.query_ray_hits(origin, dir, max_distance, &opts)?;
        if let Some(hit) = hits.into_iter().next() {
            if let Some(distance) = hit.distance {
                let d = if dir.norm() > 0.0 {
                    dir.normalize()
                } else {
                    Vector3::y() * -1.0
                };
                let contact_point = origin + d * distance;
                return Ok(Some(SupportDetectionResult {
                    refno: hit.refno,
                    contact_point,
                    element_type: "SUPPO".to_string(),
                }));
            }
        }
        Ok(None)
    }

    /// 验证支撑关系
    fn verify_support_relation(
        &self,
        _sctn: &CableTraySection,
        _support: &SupportDetectionResult,
    ) -> Result<bool> {
        // TODO: 实现支撑关系验证逻辑
        Ok(true)
    }

    /// 分类支撑类型
    fn classify_support_type(&self, _support: &SupportDetectionResult) -> SupportType {
        // TODO: 根据支撑构件的类型和位置关系分类
        SupportType::DirectSupport
    }

    /// 计算荷载分布
    fn calculate_load_distribution(
        &self,
        _sctn: &CableTraySection,
        _support: &SupportDetectionResult,
    ) -> f32 {
        // TODO: 实现荷载分布计算
        1.0
    }
}

/// 支撑检测结果
struct SupportDetectionResult {
    refno: RefU64,
    contact_point: Point3<f32>,
    element_type: String,
}

/// 批量SCTN接触检测
pub struct BatchSctnDetector {
    detector: SctnContactDetector,
}

impl BatchSctnDetector {
    pub fn new(tolerance: f32) -> Result<Self> {
        Ok(Self {
            detector: SctnContactDetector::new(tolerance)?,
        })
    }

    /// 批量检测多个SCTN的接触关系
    pub async fn detect_batch(
        &self,
        sections: Vec<CableTraySection>,
        target_types: &[String],
    ) -> Result<Vec<(RefU64, Vec<(RefU64, ContactResult)>)>> {
        let mut all_results = Vec::new();

        for sctn in sections {
            let contacts = self
                .detector
                .detect_sctn_contacts(&sctn, target_types, true)
                .await?;

            all_results.push((sctn.refno, contacts));
        }

        Ok(all_results)
    }

    /// 检测桥架间的连接关系
    pub async fn detect_tray_connections(
        &self,
        sections: &[CableTraySection],
    ) -> Result<Vec<TrayConnection>> {
        let mut connections = Vec::new();

        for i in 0..sections.len() {
            for j in i + 1..sections.len() {
                if let Some(conn) = self.check_tray_connection(&sections[i], &sections[j])? {
                    connections.push(conn);
                }
            }
        }

        Ok(connections)
    }

    /// 检查两个桥架截面是否连接
    fn check_tray_connection(
        &self,
        sctn1: &CableTraySection,
        sctn2: &CableTraySection,
    ) -> Result<Option<TrayConnection>> {
        let distance = (sctn1.bbox.center() - sctn2.bbox.center()).norm();

        // 检查是否在连接距离内
        if distance < self.detector.tolerance * 10.0 {
            // 检查方向是否一致或垂直
            let angle = sctn1.direction.angle(&sctn2.direction);
            let connection_type = if angle < 0.1 {
                ConnectionType::Straight
            } else if (angle - std::f32::consts::FRAC_PI_2).abs() < 0.1 {
                ConnectionType::Corner
            } else {
                ConnectionType::Branch
            };

            return Ok(Some(TrayConnection {
                section1: sctn1.refno,
                section2: sctn2.refno,
                connection_type,
                connection_point: (sctn1.bbox.center() + sctn2.bbox.center()) / 2.0,
            }));
        }

        Ok(None)
    }
}

/// 桥架连接关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConnection {
    pub section1: RefU64,
    pub section2: RefU64,
    pub connection_type: ConnectionType,
    pub connection_point: Point3<f32>,
}

/// 连接类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionType {
    Straight, // 直连
    Corner,   // 转角
    Branch,   // 分支
    Cross,    // 交叉
}
