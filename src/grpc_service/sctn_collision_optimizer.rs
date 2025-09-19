use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::partitioning::{Qbvh, QbvhDataGenerator};
use parry3d::query::contact;
use parry3d::shape::Cuboid;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::grpc_service::sctn_contact_detector::{CableTraySection, ContactResult, ContactType};

/// 碰撞优化器 - 使用层次包围体优化碰撞检测
pub struct SctnCollisionOptimizer {
    bvh_tree: Option<Qbvh<u32>>,
    sections: Vec<CableTraySection>,
    tolerance: f32,
}

impl SctnCollisionOptimizer {
    pub fn new(tolerance: f32) -> Self {
        Self {
            bvh_tree: None,
            sections: Vec::new(),
            tolerance,
        }
    }

    /// 构建BVH树以优化碰撞检测
    pub fn build_bvh(&mut self, sections: Vec<CableTraySection>) {
        self.sections = sections;

        // 创建BVH数据生成器
        let data_generator = SctnBvhDataGenerator {
            sections: &self.sections,
        };

        // 构建BVH树
        self.bvh_tree = Some(Qbvh::new(data_generator));
    }

    /// 批量碰撞检测（使用BVH优化）
    pub fn batch_collision_detection(&self) -> Vec<CollisionPair> {
        if self.bvh_tree.is_none() || self.sections.is_empty() {
            return Vec::new();
        }

        let bvh = self.bvh_tree.as_ref().unwrap();
        let mut collision_pairs = Vec::new();

        // 并行检测每个SCTN与其他SCTN的碰撞
        let results: Vec<_> = self
            .sections
            .par_iter()
            .enumerate()
            .flat_map(|(i, sctn)| self.detect_collisions_for_section(i, sctn, bvh))
            .collect();

        // 去重
        let mut seen = HashSet::new();
        for pair in results {
            let key = if pair.section1 < pair.section2 {
                (pair.section1, pair.section2)
            } else {
                (pair.section2, pair.section1)
            };

            if seen.insert(key) {
                collision_pairs.push(pair);
            }
        }

        collision_pairs
    }

    /// 检测单个SCTN的碰撞
    fn detect_collisions_for_section(
        &self,
        index: usize,
        sctn: &CableTraySection,
        bvh: &Qbvh<u32>,
    ) -> Vec<CollisionPair> {
        let mut pairs = Vec::new();
        let query_aabb = self.expand_aabb(&sctn.bbox, self.tolerance);

        // 使用BVH查询潜在碰撞
        let mut visitor = |leaf: &u32| -> bool {
            let other_index = *leaf as usize;
            if other_index != index {
                let other = &self.sections[other_index];

                // 精确碰撞检测
                if let Some(contact) = self.precise_collision_check(sctn, other) {
                    pairs.push(CollisionPair {
                        section1: sctn.refno,
                        section2: other.refno,
                        contact_type: contact.contact_type,
                        penetration_depth: contact.penetration_depth,
                        contact_points: contact.contact_points,
                        resolution_suggestion: self.suggest_resolution(&contact),
                    });
                }
            }
            true // 继续遍历
        };

        bvh.traverse_depth_first_with_aabb(&query_aabb, &mut visitor);

        pairs
    }

    /// 扩展包围盒
    fn expand_aabb(&self, aabb: &Aabb, tolerance: f32) -> Aabb {
        Aabb::new(
            aabb.mins - Vector3::new(tolerance, tolerance, tolerance),
            aabb.maxs + Vector3::new(tolerance, tolerance, tolerance),
        )
    }

    /// 精确碰撞检测
    fn precise_collision_check(
        &self,
        sctn1: &CableTraySection,
        sctn2: &CableTraySection,
    ) -> Option<ContactResult> {
        // 创建形状
        let shape1 = Cuboid::new(Vector3::new(
            sctn1.width / 2.0,
            sctn1.height / 2.0,
            sctn1.depth / 2.0,
        ));

        let shape2 = Cuboid::new(Vector3::new(
            sctn2.width / 2.0,
            sctn2.height / 2.0,
            sctn2.depth / 2.0,
        ));

        // 计算位姿
        let pos1 = Isometry3::translation(
            sctn1.bbox.center().x,
            sctn1.bbox.center().y,
            sctn1.bbox.center().z,
        );

        let pos2 = Isometry3::translation(
            sctn2.bbox.center().x,
            sctn2.bbox.center().y,
            sctn2.bbox.center().z,
        );

        // 检测接触
        contact(&pos1, &shape1, &pos2, &shape2, self.tolerance)
            .ok()
            .flatten()
            .map(|c| ContactResult {
                contact_type: if c.dist < -self.tolerance {
                    ContactType::Penetration
                } else if c.dist.abs() < 0.001 {
                    ContactType::Surface
                } else {
                    ContactType::Proximity
                },
                contact_points: vec![c.point1.into(), c.point2.into()],
                contact_normal: c.normal1,
                penetration_depth: c.dist.abs(),
                contact_area: 0.0, // 简化
                distance: c.dist.abs(),
            })
    }

    /// 建议碰撞解决方案
    fn suggest_resolution(&self, contact: &ContactResult) -> ResolutionSuggestion {
        match contact.contact_type {
            ContactType::Penetration => {
                // 计算分离向量
                let separation = contact.contact_normal * (contact.penetration_depth + 0.01);
                ResolutionSuggestion::Move {
                    direction: separation,
                    distance: separation.norm(),
                    priority: Priority::High,
                }
            }
            ContactType::Surface => {
                // 表面接触，可能需要小幅调整
                ResolutionSuggestion::Adjust {
                    offset: contact.contact_normal * 0.005,
                    priority: Priority::Low,
                }
            }
            ContactType::Proximity => {
                // 接近但未接触，可能不需要调整
                ResolutionSuggestion::None
            }
            _ => ResolutionSuggestion::None,
        }
    }

    /// 自动解决碰撞
    pub fn auto_resolve_collisions(&mut self, max_iterations: usize) -> Vec<CollisionResolution> {
        let mut resolutions = Vec::new();
        let mut iteration = 0;

        while iteration < max_iterations {
            let collisions = self.batch_collision_detection();

            if collisions.is_empty() {
                break;
            }

            // 按优先级排序
            let mut prioritized: Vec<_> = collisions
                .into_iter()
                .filter(|c| matches!(c.contact_type, ContactType::Penetration))
                .collect();

            prioritized.sort_by(|a, b| {
                b.penetration_depth
                    .partial_cmp(&a.penetration_depth)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // 解决最严重的碰撞
            if let Some(worst) = prioritized.first() {
                let resolution = self.resolve_single_collision(worst);
                resolutions.push(resolution);

                // 重建BVH
                self.build_bvh(self.sections.clone());
            } else {
                break;
            }

            iteration += 1;
        }

        resolutions
    }

    /// 解决单个碰撞
    fn resolve_single_collision(&mut self, collision: &CollisionPair) -> CollisionResolution {
        // 找到涉及的SCTN
        let (idx1, idx2) =
            self.sections
                .iter()
                .enumerate()
                .fold((None, None), |(i1, i2), (idx, sctn)| {
                    if sctn.refno == collision.section1 {
                        (Some(idx), i2)
                    } else if sctn.refno == collision.section2 {
                        (i1, Some(idx))
                    } else {
                        (i1, i2)
                    }
                });

        if let (Some(i1), Some(i2)) = (idx1, idx2) {
            // 移动较小的SCTN
            let move_first = self.sections[i1].bbox.volume() < self.sections[i2].bbox.volume();
            let (move_idx, fixed_idx) = if move_first { (i1, i2) } else { (i2, i1) };

            // 计算移动向量
            if let ResolutionSuggestion::Move { direction, .. } = &collision.resolution_suggestion {
                let old_pos = self.sections[move_idx].bbox.center();
                let new_pos = old_pos + direction;

                // 更新位置
                self.sections[move_idx].bbox = Aabb::new(
                    self.sections[move_idx].bbox.mins + direction,
                    self.sections[move_idx].bbox.maxs + direction,
                );

                return CollisionResolution {
                    section_moved: self.sections[move_idx].refno,
                    section_fixed: self.sections[fixed_idx].refno,
                    movement: *direction,
                    old_position: old_pos,
                    new_position: new_pos,
                };
            }
        }

        CollisionResolution::default()
    }
}

/// BVH数据生成器
struct SctnBvhDataGenerator<'a> {
    sections: &'a [CableTraySection],
}

impl<'a> QbvhDataGenerator<u32> for SctnBvhDataGenerator<'a> {
    fn size_hint(&self) -> usize {
        self.sections.len()
    }

    fn for_each(&mut self, mut f: impl FnMut(u32, Aabb)) {
        for (i, section) in self.sections.iter().enumerate() {
            f(i as u32, section.bbox);
        }
    }
}

/// 碰撞对
#[derive(Debug, Clone)]
pub struct CollisionPair {
    pub section1: RefU64,
    pub section2: RefU64,
    pub contact_type: ContactType,
    pub penetration_depth: f32,
    pub contact_points: Vec<Point3<f32>>,
    pub resolution_suggestion: ResolutionSuggestion,
}

/// 解决建议
#[derive(Debug, Clone)]
pub enum ResolutionSuggestion {
    None,
    Move {
        direction: Vector3<f32>,
        distance: f32,
        priority: Priority,
    },
    Adjust {
        offset: Vector3<f32>,
        priority: Priority,
    },
    Reroute {
        alternative_path: Vec<RefU64>,
        priority: Priority,
    },
}

/// 优先级
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

/// 碰撞解决结果
#[derive(Debug, Clone, Default)]
pub struct CollisionResolution {
    pub section_moved: RefU64,
    pub section_fixed: RefU64,
    pub movement: Vector3<f32>,
    pub old_position: Point3<f32>,
    pub new_position: Point3<f32>,
}

/// 高级碰撞分析
pub struct AdvancedCollisionAnalyzer {
    optimizer: SctnCollisionOptimizer,
}

impl AdvancedCollisionAnalyzer {
    pub fn new(tolerance: f32) -> Self {
        Self {
            optimizer: SctnCollisionOptimizer::new(tolerance),
        }
    }

    /// 分析碰撞热点
    pub fn analyze_collision_hotspots(
        &mut self,
        sections: Vec<CableTraySection>,
    ) -> HotspotAnalysis {
        self.optimizer.build_bvh(sections);
        let collisions = self.optimizer.batch_collision_detection();

        // 统计每个SCTN的碰撞次数
        let mut collision_counts: HashMap<RefU64, usize> = HashMap::new();

        for collision in &collisions {
            *collision_counts.entry(collision.section1).or_insert(0) += 1;
            *collision_counts.entry(collision.section2).or_insert(0) += 1;
        }

        // 找出热点
        let mut hotspots: Vec<_> = collision_counts
            .into_iter()
            .map(|(refno, count)| Hotspot {
                refno,
                collision_count: count,
            })
            .collect();

        hotspots.sort_by(|a, b| b.collision_count.cmp(&a.collision_count));

        // 分类碰撞类型
        let mut type_distribution = HashMap::new();
        for collision in &collisions {
            *type_distribution
                .entry(collision.contact_type.clone())
                .or_insert(0) += 1;
        }

        HotspotAnalysis {
            hotspots,
            total_collisions: collisions.len(),
            type_distribution,
            critical_pairs: collisions
                .into_iter()
                .filter(|c| c.penetration_depth > 0.05)
                .collect(),
        }
    }

    /// 优化布局以减少碰撞
    pub fn optimize_layout(&mut self, sections: Vec<CableTraySection>) -> LayoutOptimization {
        self.optimizer.build_bvh(sections.clone());

        let initial_collisions = self.optimizer.batch_collision_detection().len();
        let resolutions = self.optimizer.auto_resolve_collisions(20);
        let final_collisions = self.optimizer.batch_collision_detection().len();

        LayoutOptimization {
            initial_collision_count: initial_collisions,
            final_collision_count: final_collisions,
            resolutions,
            improvement_percentage: if initial_collisions > 0 {
                ((initial_collisions - final_collisions) as f32 / initial_collisions as f32) * 100.0
            } else {
                0.0
            },
        }
    }
}

/// 热点分析结果
#[derive(Debug, Clone)]
pub struct HotspotAnalysis {
    pub hotspots: Vec<Hotspot>,
    pub total_collisions: usize,
    pub type_distribution: HashMap<ContactType, usize>,
    pub critical_pairs: Vec<CollisionPair>,
}

/// 碰撞热点
#[derive(Debug, Clone)]
pub struct Hotspot {
    pub refno: RefU64,
    pub collision_count: usize,
}

/// 布局优化结果
#[derive(Debug, Clone)]
pub struct LayoutOptimization {
    pub initial_collision_count: usize,
    pub final_collision_count: usize,
    pub resolutions: Vec<CollisionResolution>,
    pub improvement_percentage: f32,
}
