use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

/// XKT 空间分区瓦片
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTTile {
    /// 世界空间轴对齐包围盒 [xmin, ymin, zmin, xmax, ymax, zmax]
    pub aabb: [f64; 6],

    /// 瓦片中心点
    pub center: Vec3,

    /// 包含的实体ID列表
    pub entity_ids: Vec<String>,

    /// RTC（Relative-to-Center）变换矩阵
    pub rtc_matrix: Option<Mat4>,
}

impl XKTTile {
    pub fn new(aabb: [f64; 6]) -> Self {
        let center = Vec3::new(
            ((aabb[0] + aabb[3]) / 2.0) as f32,
            ((aabb[1] + aabb[4]) / 2.0) as f32,
            ((aabb[2] + aabb[5]) / 2.0) as f32,
        );

        Self {
            aabb,
            center,
            entity_ids: Vec::new(),
            rtc_matrix: None,
        }
    }

    pub fn add_entity(&mut self, entity_id: String) {
        if !self.entity_ids.contains(&entity_id) {
            self.entity_ids.push(entity_id);
        }
    }

    pub fn get_diagonal(&self) -> f64 {
        let dx = self.aabb[3] - self.aabb[0];
        let dy = self.aabb[4] - self.aabb[1];
        let dz = self.aabb[5] - self.aabb[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// KD树节点（用于空间分区）
#[derive(Debug, Clone)]
pub struct KDNode {
    pub aabb: [f64; 6],
    pub entities: Option<Vec<String>>,
    pub left: Option<Box<KDNode>>,
    pub right: Option<Box<KDNode>>,
}

impl KDNode {
    pub fn new(aabb: [f64; 6]) -> Self {
        Self {
            aabb,
            entities: None,
            left: None,
            right: None,
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.left.is_none() && self.right.is_none()
    }

    pub fn get_diagonal(&self) -> f64 {
        let dx = self.aabb[3] - self.aabb[0];
        let dy = self.aabb[4] - self.aabb[1];
        let dz = self.aabb[5] - self.aabb[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// 空间索引配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialConfig {
    /// 最小瓦片大小
    pub min_tile_size: f64,

    /// 每个瓦片最大实体数
    pub max_entities_per_tile: usize,

    /// 是否启用RTC坐标系统
    pub enable_rtc: bool,
}

impl Default for SpatialConfig {
    fn default() -> Self {
        Self {
            min_tile_size: 500.0,
            max_entities_per_tile: 4096,
            enable_rtc: true,
        }
    }
}

/// 空间分区管理器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTSpatialIndex {
    pub tiles: Vec<XKTTile>,
    pub config: SpatialConfig,
    pub model_aabb: Option<[f64; 6]>,
}

impl XKTSpatialIndex {
    pub fn new(config: SpatialConfig) -> Self {
        Self {
            tiles: Vec::new(),
            config,
            model_aabb: None,
        }
    }

    pub fn set_model_aabb(&mut self, aabb: [f64; 6]) {
        self.model_aabb = Some(aabb);
    }

    /// 从实体列表构建空间分区
    pub fn build_from_entities(&mut self, entities: &[(String, [f64; 6])]) -> anyhow::Result<()> {
        // 计算总体AABB
        let mut model_aabb = if let Some(aabb) = self.model_aabb {
            aabb
        } else {
            self.calculate_model_aabb(entities)
        };

        // 构建KD树
        let root_node = self.build_kd_tree(entities, model_aabb)?;

        // 从KD树创建瓦片
        self.create_tiles_from_kd_tree(&root_node);

        Ok(())
    }

    fn calculate_model_aabb(&self, entities: &[(String, [f64; 6])]) -> [f64; 6] {
        if entities.is_empty() {
            return [0.0; 6];
        }

        let mut aabb = entities[0].1;
        for (_, entity_aabb) in entities.iter().skip(1) {
            // 扩展AABB
            aabb[0] = aabb[0].min(entity_aabb[0]);
            aabb[1] = aabb[1].min(entity_aabb[1]);
            aabb[2] = aabb[2].min(entity_aabb[2]);
            aabb[3] = aabb[3].max(entity_aabb[3]);
            aabb[4] = aabb[4].max(entity_aabb[4]);
            aabb[5] = aabb[5].max(entity_aabb[5]);
        }

        aabb
    }

    fn build_kd_tree(
        &self,
        entities: &[(String, [f64; 6])],
        aabb: [f64; 6],
    ) -> anyhow::Result<KDNode> {
        let mut root = KDNode::new(aabb);

        for (entity_id, entity_aabb) in entities {
            self.insert_entity_into_kd_tree(&mut root, entity_id.clone(), *entity_aabb);
        }

        Ok(root)
    }

    fn insert_entity_into_kd_tree(
        &self,
        node: &mut KDNode,
        entity_id: String,
        entity_aabb: [f64; 6],
    ) {
        let node_diagonal = node.get_diagonal();

        // 如果节点太小，直接添加实体
        if node_diagonal < self.config.min_tile_size {
            if node.entities.is_none() {
                node.entities = Some(Vec::new());
            }
            node.entities.as_mut().unwrap().push(entity_id);
            return;
        }

        // 尝试插入到子节点
        if let Some(ref mut left) = node.left {
            if self.aabb_contains(&left.aabb, &entity_aabb) {
                self.insert_entity_into_kd_tree(left, entity_id, entity_aabb);
                return;
            }
        }

        if let Some(ref mut right) = node.right {
            if self.aabb_contains(&right.aabb, &entity_aabb) {
                self.insert_entity_into_kd_tree(right, entity_id, entity_aabb);
                return;
            }
        }

        // 需要分割节点
        if node.left.is_none() && node.right.is_none() {
            self.split_kd_node(node);

            // 重新尝试插入
            self.insert_entity_into_kd_tree(node, entity_id, entity_aabb);
        } else {
            // 无法插入子节点，添加到当前节点
            if node.entities.is_none() {
                node.entities = Some(Vec::new());
            }
            node.entities.as_mut().unwrap().push(entity_id);
        }
    }

    fn split_kd_node(&self, node: &mut KDNode) {
        let aabb = node.aabb;
        let dx = aabb[3] - aabb[0];
        let dy = aabb[4] - aabb[1];
        let dz = aabb[5] - aabb[2];

        // 选择最长的轴进行分割
        let split_axis = if dx >= dy && dx >= dz {
            0
        } else if dy >= dz {
            1
        } else {
            2
        };

        let split_pos = (aabb[split_axis] + aabb[split_axis + 3]) / 2.0;

        // 创建左子节点
        let mut left_aabb = aabb;
        left_aabb[split_axis + 3] = split_pos;
        node.left = Some(Box::new(KDNode::new(left_aabb)));

        // 创建右子节点
        let mut right_aabb = aabb;
        right_aabb[split_axis] = split_pos;
        node.right = Some(Box::new(KDNode::new(right_aabb)));
    }

    fn aabb_contains(&self, container: &[f64; 6], contained: &[f64; 6]) -> bool {
        container[0] <= contained[0]
            && container[1] <= contained[1]
            && container[2] <= contained[2]
            && container[3] >= contained[3]
            && container[4] >= contained[4]
            && container[5] >= contained[5]
    }

    fn create_tiles_from_kd_tree(&mut self, node: &KDNode) {
        if let Some(ref entities) = node.entities {
            if !entities.is_empty() {
                let mut tile = XKTTile::new(node.aabb);
                for entity_id in entities {
                    tile.add_entity(entity_id.clone());
                }
                self.tiles.push(tile);
            }
        }

        if let Some(ref left) = node.left {
            self.create_tiles_from_kd_tree(left);
        }

        if let Some(ref right) = node.right {
            self.create_tiles_from_kd_tree(right);
        }
    }
}
