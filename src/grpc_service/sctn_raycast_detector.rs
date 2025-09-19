use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use parry3d::query::{Ray, RayCast, RayIntersection};
use parry3d::shape::{Ball, Cuboid, TriMesh};
use rayon::prelude::*;
use std::sync::Arc;

use crate::grpc_service::sctn_contact_detector::{CableTraySection, SupportRelation, SupportType};
use crate::spatial_index::SqliteSpatialIndex;

/// 射线投射支撑检测器
pub struct SctnRaycastDetector {
    spatial_index: Option<SqliteSpatialIndex>,
    max_ray_distance: f32,
    ray_samples_per_meter: u32,
}

impl SctnRaycastDetector {
    pub fn new(max_distance: f32) -> Result<Self> {
        let spatial_index = if SqliteSpatialIndex::is_enabled() {
            Some(SqliteSpatialIndex::with_default_path()?)
        } else {
            None
        };

        Ok(Self {
            spatial_index,
            max_ray_distance: max_distance,
            ray_samples_per_meter: 3, // 每米3个采样点
        })
    }

    /// 使用射线投射检测桥架的支撑关系
    pub async fn detect_supports_by_raycast(
        &self,
        sctn: &CableTraySection,
        candidate_supports: &[SupportCandidate],
    ) -> Result<Vec<SupportRelation>> {
        let mut relations = Vec::new();

        // 生成射线采样点
        let ray_origins = self.generate_ray_origins(sctn);

        // 并行处理每个射线
        let ray_results: Vec<_> = ray_origins
            .par_iter()
            .filter_map(|origin| self.cast_support_ray(*origin, candidate_supports).ok())
            .collect();

        // 聚合结果
        for result in ray_results {
            if let Some(hit) = result {
                relations.push(SupportRelation {
                    tray_section: sctn.refno,
                    support: hit.support_refno,
                    support_type: self.classify_support_type(&hit),
                    contact_point: hit.intersection_point,
                    load_distribution: self.calculate_load_factor(&hit, sctn),
                });
            }
        }

        // 去重和优化
        self.optimize_support_relations(&mut relations);

        Ok(relations)
    }

    /// 生成射线起点
    fn generate_ray_origins(&self, sctn: &CableTraySection) -> Vec<Point3<f32>> {
        let mut origins = Vec::new();

        // 计算采样点数量
        let num_samples = (sctn.depth * self.ray_samples_per_meter as f32) as usize;
        let num_samples = num_samples.max(2); // 至少2个点

        // 沿桥架长度方向生成采样点
        for i in 0..num_samples {
            let t = i as f32 / (num_samples - 1) as f32;

            // 沿宽度方向也生成多个点（左中右）
            for w in &[0.0, 0.5, 1.0] {
                let x = sctn.bbox.mins.x + t * (sctn.bbox.maxs.x - sctn.bbox.mins.x);
                let y = sctn.bbox.mins.y; // 底部
                let z = sctn.bbox.mins.z + w * (sctn.bbox.maxs.z - sctn.bbox.mins.z);

                origins.push(Point3::new(x, y, z));
            }
        }

        origins
    }

    /// 投射单条支撑检测射线
    fn cast_support_ray(
        &self,
        origin: Point3<f32>,
        candidates: &[SupportCandidate],
    ) -> Result<Option<RayHit>> {
        // 创建向下的射线
        let ray = Ray::new(origin, -Vector3::y());

        let mut closest_hit: Option<RayHit> = None;
        let mut min_distance = self.max_ray_distance;

        for candidate in candidates {
            // 创建候选支撑的形状
            let shape = self.create_shape_for_candidate(candidate);

            // 执行射线投射
            if let Some(toi) = shape.cast_ray(&ray, self.max_ray_distance, true) {
                if toi < min_distance {
                    min_distance = toi;
                    closest_hit = Some(RayHit {
                        support_refno: candidate.refno,
                        intersection_point: ray.point_at(toi),
                        distance: toi,
                        normal: Vector3::y(), // 简化：使用向上的法线
                        support_type: candidate.element_type.clone(),
                    });
                }
            }
        }

        Ok(closest_hit)
    }

    /// 为候选支撑创建碰撞形状
    fn create_shape_for_candidate(&self, candidate: &SupportCandidate) -> Box<dyn RayCast> {
        let size = candidate.bbox.maxs - candidate.bbox.mins;
        Box::new(Cuboid::new(size / 2.0))
    }

    /// 分类支撑类型
    fn classify_support_type(&self, hit: &RayHit) -> SupportType {
        match hit.support_type.as_str() {
            "HANG" | "HANGER" => SupportType::HangerSupport,
            "BRKT" | "BRACKET" => SupportType::BracketSupport,
            "WALL" | "WFIX" => SupportType::WallMount,
            "SUPP" | "SUPPORT" => SupportType::DirectSupport,
            _ => SupportType::Unknown,
        }
    }

    /// 计算荷载分布系数
    fn calculate_load_factor(&self, hit: &RayHit, sctn: &CableTraySection) -> f32 {
        // 基于距离和位置计算荷载分布
        let center = sctn.bbox.center();
        let distance_from_center = (hit.intersection_point - center).norm();
        let max_distance = sctn.depth / 2.0;

        // 越靠近中心，荷载系数越大
        1.0 - (distance_from_center / max_distance).min(1.0) * 0.5
    }

    /// 优化支撑关系（去除重复和无效的）
    fn optimize_support_relations(&self, relations: &mut Vec<SupportRelation>) {
        // 按支撑点位置排序
        relations.sort_by(|a, b| {
            a.contact_point
                .x
                .partial_cmp(&b.contact_point.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 去除过于接近的重复支撑
        let mut filtered = Vec::new();
        let min_spacing = 0.1; // 最小间距10cm

        for relation in relations.iter() {
            let too_close = filtered.iter().any(|existing: &SupportRelation| {
                (existing.contact_point - relation.contact_point).norm() < min_spacing
            });

            if !too_close {
                filtered.push(relation.clone());
            }
        }

        *relations = filtered;
    }

    /// 验证支撑的有效性
    pub fn verify_support_validity(
        &self,
        sctn: &CableTraySection,
        support: &SupportCandidate,
    ) -> bool {
        // 检查支撑是否在桥架下方
        if support.bbox.maxs.y > sctn.bbox.mins.y {
            return false;
        }

        // 检查水平重叠
        let x_overlap =
            sctn.bbox.maxs.x > support.bbox.mins.x && sctn.bbox.mins.x < support.bbox.maxs.x;
        let z_overlap =
            sctn.bbox.maxs.z > support.bbox.mins.z && sctn.bbox.mins.z < support.bbox.maxs.z;

        x_overlap && z_overlap
    }
}

/// 支撑候选
#[derive(Debug, Clone)]
pub struct SupportCandidate {
    pub refno: RefU64,
    pub bbox: Aabb,
    pub element_type: String,
    pub attributes: std::collections::HashMap<String, f32>,
}

/// 射线命中结果
#[derive(Debug, Clone)]
struct RayHit {
    support_refno: RefU64,
    intersection_point: Point3<f32>,
    distance: f32,
    normal: Vector3<f32>,
    support_type: String,
}

/// 高级射线投射分析
pub struct AdvancedRaycastAnalyzer {
    detector: SctnRaycastDetector,
}

impl AdvancedRaycastAnalyzer {
    pub fn new(max_distance: f32) -> Result<Self> {
        Ok(Self {
            detector: SctnRaycastDetector::new(max_distance)?,
        })
    }

    /// 分析桥架的支撑跨度
    pub fn analyze_support_spans(&self, relations: &[SupportRelation]) -> SpanAnalysis {
        if relations.is_empty() {
            return SpanAnalysis::default();
        }

        let mut spans = Vec::new();

        // 按X坐标排序
        let mut sorted_supports = relations.to_vec();
        sorted_supports.sort_by(|a, b| {
            a.contact_point
                .x
                .partial_cmp(&b.contact_point.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 计算跨度
        for i in 1..sorted_supports.len() {
            let span = sorted_supports[i].contact_point.x - sorted_supports[i - 1].contact_point.x;
            spans.push(span);
        }

        let max_span = spans.iter().cloned().fold(0.0_f32, f32::max);
        let min_span = spans.iter().cloned().fold(f32::MAX, f32::min);
        let avg_span = spans.iter().sum::<f32>() / spans.len() as f32;

        SpanAnalysis {
            max_span,
            min_span,
            avg_span,
            num_supports: relations.len(),
            spans,
        }
    }

    /// 检测悬空段
    pub fn detect_unsupported_segments(
        &self,
        sctn: &CableTraySection,
        supports: &[SupportRelation],
        max_unsupported_length: f32,
    ) -> Vec<UnsupportedSegment> {
        let mut unsupported = Vec::new();

        if supports.is_empty() {
            // 整个桥架都未支撑
            unsupported.push(UnsupportedSegment {
                start: sctn.bbox.mins,
                end: sctn.bbox.maxs,
                length: sctn.depth,
                severity: if sctn.depth > max_unsupported_length * 2.0 {
                    Severity::Critical
                } else if sctn.depth > max_unsupported_length {
                    Severity::Warning
                } else {
                    Severity::Info
                },
            });
            return unsupported;
        }

        // 按位置排序
        let mut sorted_supports = supports.to_vec();
        sorted_supports.sort_by(|a, b| {
            a.contact_point
                .x
                .partial_cmp(&b.contact_point.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 检查起始段
        let first_support_x = sorted_supports[0].contact_point.x;
        let start_gap = first_support_x - sctn.bbox.mins.x;
        if start_gap > max_unsupported_length {
            unsupported.push(UnsupportedSegment {
                start: sctn.bbox.mins,
                end: sorted_supports[0].contact_point,
                length: start_gap,
                severity: self.assess_severity(start_gap, max_unsupported_length),
            });
        }

        // 检查中间段
        for i in 1..sorted_supports.len() {
            let gap = sorted_supports[i].contact_point.x - sorted_supports[i - 1].contact_point.x;
            if gap > max_unsupported_length {
                unsupported.push(UnsupportedSegment {
                    start: sorted_supports[i - 1].contact_point,
                    end: sorted_supports[i].contact_point,
                    length: gap,
                    severity: self.assess_severity(gap, max_unsupported_length),
                });
            }
        }

        // 检查末尾段
        let last_support_x = sorted_supports.last().unwrap().contact_point.x;
        let end_gap = sctn.bbox.maxs.x - last_support_x;
        if end_gap > max_unsupported_length {
            unsupported.push(UnsupportedSegment {
                start: sorted_supports.last().unwrap().contact_point,
                end: sctn.bbox.maxs,
                length: end_gap,
                severity: self.assess_severity(end_gap, max_unsupported_length),
            });
        }

        unsupported
    }

    fn assess_severity(&self, gap: f32, max_allowed: f32) -> Severity {
        if gap > max_allowed * 2.0 {
            Severity::Critical
        } else if gap > max_allowed * 1.5 {
            Severity::Warning
        } else {
            Severity::Info
        }
    }
}

/// 跨度分析结果
#[derive(Debug, Clone, Default)]
pub struct SpanAnalysis {
    pub max_span: f32,
    pub min_span: f32,
    pub avg_span: f32,
    pub num_supports: usize,
    pub spans: Vec<f32>,
}

/// 无支撑段
#[derive(Debug, Clone)]
pub struct UnsupportedSegment {
    pub start: Point3<f32>,
    pub end: Point3<f32>,
    pub length: f32,
    pub severity: Severity,
}

/// 严重程度
#[derive(Debug, Clone)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}
