use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::Result;
use bevy_transform::prelude::Transform;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use std::sync::Arc;

use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::grpc_service::sctn_contact_detector::CableTraySection;

/// SCTN几何信息提取器
pub struct SctnGeometryExtractor {
    db_manager: Arc<AiosDBManager>,
}

impl SctnGeometryExtractor {
    pub fn new(db_manager: Arc<AiosDBManager>) -> Self {
        Self { db_manager }
    }

    /// 从数据库获取SCTN的完整几何信息
    pub async fn extract_sctn_geometry(&self, sctn_refno: RefU64) -> Result<CableTraySection> {
        // 获取SCTN的属性
        let attrs = self.db_manager.get_attr(sctn_refno).await?;

        // 获取世界坐标变换
        let transform = self
            .db_manager
            .get_world_transform(sctn_refno)
            .await?
            .unwrap_or_default()
            .unwrap_or_default();

        // 提取尺寸参数
        let width = self.extract_width(&attrs).unwrap_or(0.3);
        let height = self.extract_height(&attrs).unwrap_or(0.1);
        let depth = self.extract_depth(&attrs).unwrap_or(1.0);

        // 获取包围盒
        let bbox = self
            .calculate_sctn_bbox(sctn_refno, &transform, width, height, depth)
            .await?;

        // 获取中心线
        let centerline = self.extract_centerline(sctn_refno, &transform).await?;

        // 获取方向
        let direction = self.extract_direction(&transform);

        // 获取支撑点
        let support_points = self.find_support_points(sctn_refno).await?;

        Ok(CableTraySection {
            refno: sctn_refno,
            bbox,
            centerline,
            width,
            height,
            depth,
            direction,
            support_points,
            section_type: "SCTN".to_string(),
        })
    }

    /// 批量提取桥架分支下的所有SCTN
    pub async fn extract_branch_sections(
        &self,
        bran_refno: RefU64,
    ) -> Result<Vec<CableTraySection>> {
        let mut sections = Vec::new();

        // 获取BRAN下的所有子元素
        let children = self.db_manager.get_children_refs(bran_refno).await?;

        for child in children.iter() {
            let type_name = self.db_manager.get_type_name(*child).await;

            if type_name == "SCTN" {
                match self.extract_sctn_geometry(*child).await {
                    Ok(sctn) => sections.push(sctn),
                    Err(e) => {
                        log::warn!("Failed to extract SCTN {} geometry: {}", child.0, e);
                    }
                }
            }
        }

        // 按照空间位置排序
        sections.sort_by(|a, b| {
            let dist_a = a.bbox.center().coords.norm();
            let dist_b = b.bbox.center().coords.norm();
            dist_a
                .partial_cmp(&dist_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(sections)
    }

    /// 提取宽度参数
    fn extract_width(&self, attrs: &aios_core::AttrMap) -> Option<f32> {
        attrs
            .get("WIDTH")
            .or_else(|| attrs.get("WIDT"))
            .or_else(|| attrs.get("WID"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
    }

    /// 提取高度参数
    fn extract_height(&self, attrs: &aios_core::AttrMap) -> Option<f32> {
        attrs
            .get("HEIGHT")
            .or_else(|| attrs.get("HEIG"))
            .or_else(|| attrs.get("HEI"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
    }

    /// 提取深度/长度参数
    fn extract_depth(&self, attrs: &aios_core::AttrMap) -> Option<f32> {
        attrs
            .get("LENGTH")
            .or_else(|| attrs.get("LENG"))
            .or_else(|| attrs.get("DEPTH"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
    }

    /// 计算SCTN的包围盒
    async fn calculate_sctn_bbox(
        &self,
        refno: RefU64,
        transform: &Transform,
        width: f32,
        height: f32,
        depth: f32,
    ) -> Result<Aabb> {
        // 首先尝试从缓存获取
        if let Ok(Some(cached_bbox)) = self.get_cached_bbox(refno).await {
            return Ok(cached_bbox);
        }

        // 根据变换和尺寸计算包围盒
        let position = transform.translation;
        let half_width = width / 2.0;
        let half_height = height / 2.0;
        let half_depth = depth / 2.0;

        // 考虑旋转的包围盒
        let corners = vec![
            Point3::new(-half_width, -half_height, -half_depth),
            Point3::new(half_width, -half_height, -half_depth),
            Point3::new(-half_width, half_height, -half_depth),
            Point3::new(half_width, half_height, -half_depth),
            Point3::new(-half_width, -half_height, half_depth),
            Point3::new(half_width, -half_height, half_depth),
            Point3::new(-half_width, half_height, half_depth),
            Point3::new(half_width, half_height, half_depth),
        ];

        let mut min = Point3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = Point3::new(f32::MIN, f32::MIN, f32::MIN);

        for corner in corners {
            let rotated = transform.rotation * corner.coords;
            let world_point = Point3::from(position + rotated);

            min.x = min.x.min(world_point.x);
            min.y = min.y.min(world_point.y);
            min.z = min.z.min(world_point.z);

            max.x = max.x.max(world_point.x);
            max.y = max.y.max(world_point.y);
            max.z = max.z.max(world_point.z);
        }

        Ok(Aabb::new(min, max))
    }

    /// 从缓存获取包围盒
    async fn get_cached_bbox(&self, refno: RefU64) -> Result<Option<Aabb>> {
        #[cfg(feature = "sqlite-index")]
        if crate::spatial_index::SqliteSpatialIndex::is_enabled() {
            let index = crate::spatial_index::SqliteSpatialIndex::with_default_path()?;
            return index.get_aabb(refno);
        }
        Ok(None)
    }

    /// 提取中心线
    async fn extract_centerline(
        &self,
        refno: RefU64,
        transform: &Transform,
    ) -> Result<Vec<Point3<f32>>> {
        // 获取SCTN的路径点
        let mut centerline = Vec::new();

        // 获取起点和终点
        let attrs = self.db_manager.get_attr(refno).await?;

        // 尝试从属性中获取PPOS（起点）和QPOS（终点）
        if let (Some(ppos), Some(qpos)) = (
            self.extract_point(&attrs, "PPOS"),
            self.extract_point(&attrs, "QPOS"),
        ) {
            centerline.push(self.transform_point(ppos, transform));
            centerline.push(self.transform_point(qpos, transform));
        } else {
            // 如果没有明确的起终点，使用包围盒中心作为单点
            let center = Point3::from(transform.translation);
            centerline.push(center);
        }

        Ok(centerline)
    }

    /// 提取点坐标
    fn extract_point(&self, attrs: &aios_core::AttrMap, key: &str) -> Option<Point3<f32>> {
        attrs.get(key).and_then(|v| {
            if let Some(arr) = v.as_array() {
                if arr.len() >= 3 {
                    Some(Point3::new(
                        arr[0].as_f64()? as f32,
                        arr[1].as_f64()? as f32,
                        arr[2].as_f64()? as f32,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    /// 变换点到世界坐标
    fn transform_point(&self, point: Point3<f32>, transform: &Transform) -> Point3<f32> {
        let rotated = transform.rotation * point.coords;
        Point3::from(transform.translation + rotated)
    }

    /// 提取方向向量
    fn extract_direction(&self, transform: &Transform) -> Vector3<f32> {
        // 使用变换的前向向量作为方向
        transform.rotation * Vector3::x()
    }

    /// 查找支撑点
    async fn find_support_points(&self, sctn_refno: RefU64) -> Result<Vec<Point3<f32>>> {
        let mut support_points = Vec::new();

        // 查询与SCTN相关的支撑构件
        let supports = self
            .db_manager
            .query_foreign_refnos(
                &[sctn_refno],
                &[&["SCTN"]],
                &["SUPPO", "ATTA", "FITTING"],
                &[],
                2,
            )
            .await?;

        for support_refno in supports {
            // 获取支撑点的位置
            if let Ok(Some(transform)) = self.db_manager.get_world_transform(support_refno).await {
                if let Some(t) = transform {
                    support_points.push(Point3::from(t.translation));
                }
            }
        }

        Ok(support_points)
    }

    /// 获取桥架的类型和规格
    pub async fn get_tray_specification(&self, refno: RefU64) -> Result<TraySpecification> {
        let attrs = self.db_manager.get_attr(refno).await?;

        // 提取规格参数
        let spec_type = attrs
            .get("STYP")
            .or_else(|| attrs.get("SPEC"))
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        let material = attrs
            .get("MATE")
            .or_else(|| attrs.get("MATERIAL"))
            .and_then(|v| v.as_str())
            .unwrap_or("STEEL")
            .to_string();

        let thickness = attrs
            .get("THIC")
            .or_else(|| attrs.get("THICKNESS"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(2.0); // 默认2mm厚度

        let load_class = attrs
            .get("LOAD")
            .or_else(|| attrs.get("LOAD_CLASS"))
            .and_then(|v| v.as_str())
            .unwrap_or("MEDIUM")
            .to_string();

        Ok(TraySpecification {
            spec_type,
            material,
            thickness,
            load_class,
        })
    }
}

/// 桥架规格信息
#[derive(Debug, Clone)]
pub struct TraySpecification {
    pub spec_type: String,  // 类型（梯架、槽盒、托盘等）
    pub material: String,   // 材质
    pub thickness: f32,     // 厚度
    pub load_class: String, // 荷载等级
}

/// 高级几何分析
pub struct SctnGeometryAnalyzer {
    extractor: SctnGeometryExtractor,
}

impl SctnGeometryAnalyzer {
    pub fn new(db_manager: Arc<AiosDBManager>) -> Self {
        Self {
            extractor: SctnGeometryExtractor::new(db_manager),
        }
    }

    /// 分析桥架的弯曲和转角
    pub async fn analyze_tray_bends(&self, sections: &[CableTraySection]) -> Vec<TrayBend> {
        let mut bends = Vec::new();

        for i in 1..sections.len() - 1 {
            let prev = &sections[i - 1];
            let curr = &sections[i];
            let next = &sections[i + 1];

            // 计算方向变化
            let angle = prev.direction.angle(&next.direction);

            if angle > 0.1 {
                // 有明显的方向变化
                let bend_type = if angle < std::f32::consts::FRAC_PI_4 {
                    BendType::Slight
                } else if angle < std::f32::consts::FRAC_PI_2 {
                    BendType::Medium
                } else if angle < std::f32::consts::PI * 3.0 / 4.0 {
                    BendType::Right
                } else {
                    BendType::Sharp
                };

                bends.push(TrayBend {
                    section_refno: curr.refno,
                    angle: angle.to_degrees(),
                    bend_type,
                    position: curr.bbox.center(),
                });
            }
        }

        bends
    }

    /// 计算桥架的总长度
    pub fn calculate_total_length(&self, sections: &[CableTraySection]) -> f32 {
        let mut total_length = 0.0;

        for section in sections {
            if section.centerline.len() >= 2 {
                for i in 1..section.centerline.len() {
                    let dist = (section.centerline[i] - section.centerline[i - 1]).norm();
                    total_length += dist;
                }
            } else {
                // 使用包围盒估算长度
                total_length += section.depth;
            }
        }

        total_length
    }

    /// 检测桥架的连续性
    pub fn check_continuity(&self, sections: &[CableTraySection], max_gap: f32) -> Vec<GapInfo> {
        let mut gaps = Vec::new();

        for i in 1..sections.len() {
            let prev_end = sections[i - 1].bbox.maxs;
            let curr_start = sections[i].bbox.mins;

            let gap_distance = (curr_start - prev_end).norm();

            if gap_distance > max_gap {
                gaps.push(GapInfo {
                    from_section: sections[i - 1].refno,
                    to_section: sections[i].refno,
                    gap_distance,
                    gap_position: (prev_end + curr_start.coords) / 2.0,
                });
            }
        }

        gaps
    }
}

/// 弯曲信息
#[derive(Debug, Clone)]
pub struct TrayBend {
    pub section_refno: RefU64,
    pub angle: f32,
    pub bend_type: BendType,
    pub position: Point3<f32>,
}

/// 弯曲类型
#[derive(Debug, Clone)]
pub enum BendType {
    Slight, // 小于45度
    Medium, // 45-90度
    Right,  // 90度左右
    Sharp,  // 大于135度
}

/// 间隙信息
#[derive(Debug, Clone)]
pub struct GapInfo {
    pub from_section: RefU64,
    pub to_section: RefU64,
    pub gap_distance: f32,
    pub gap_position: Point3<f32>,
}
