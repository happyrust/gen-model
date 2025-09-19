use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// XKT 实体数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTEntity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub mesh_ids: Vec<String>,
    pub parent_id: Option<String>,
    pub children_ids: Vec<String>,
    pub properties: HashMap<String, String>,
    pub visible: bool,
    pub pickable: bool,
    pub highlighted: bool,
    pub selected: bool,
    pub xrayed: bool,
    pub clippable: bool,
    pub collidable: bool,
    pub castsShadow: bool,
    pub receivesShadow: bool,
}

impl XKTEntity {
    /// 创建新的实体
    pub fn new(id: String, name: String, entity_type: String) -> Self {
        Self {
            id,
            name,
            entity_type,
            mesh_ids: Vec::new(),
            parent_id: None,
            children_ids: Vec::new(),
            properties: HashMap::new(),
            visible: true,
            pickable: true,
            highlighted: false,
            selected: false,
            xrayed: false,
            clippable: true,
            collidable: true,
            castsShadow: true,
            receivesShadow: true,
        }
    }

    /// 添加网格
    pub fn add_mesh(&mut self, mesh_id: String) {
        if !self.mesh_ids.contains(&mesh_id) {
            self.mesh_ids.push(mesh_id);
        }
    }

    /// 移除网格
    pub fn remove_mesh(&mut self, mesh_id: &str) {
        self.mesh_ids.retain(|id| id != mesh_id);
    }

    /// 设置父实体
    pub fn set_parent(&mut self, parent_id: String) {
        self.parent_id = Some(parent_id);
    }

    /// 添加子实体
    pub fn add_child(&mut self, child_id: String) {
        if !self.children_ids.contains(&child_id) {
            self.children_ids.push(child_id);
        }
    }

    /// 移除子实体
    pub fn remove_child(&mut self, child_id: &str) {
        self.children_ids.retain(|id| id != child_id);
    }

    /// 设置属性
    pub fn set_property(&mut self, key: String, value: String) {
        self.properties.insert(key, value);
    }

    /// 获取属性
    pub fn get_property(&self, key: &str) -> Option<&String> {
        self.properties.get(key)
    }

    /// 设置可见性
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// 设置可选择性
    pub fn set_pickable(&mut self, pickable: bool) {
        self.pickable = pickable;
    }

    /// 设置高亮状态
    pub fn set_highlighted(&mut self, highlighted: bool) {
        self.highlighted = highlighted;
    }

    /// 设置选中状态
    pub fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }

    /// 设置X射线模式
    pub fn set_xrayed(&mut self, xrayed: bool) {
        self.xrayed = xrayed;
    }

    /// 设置可裁剪性
    pub fn set_clippable(&mut self, clippable: bool) {
        self.clippable = clippable;
    }

    /// 设置可碰撞性
    pub fn set_collidable(&mut self, collidable: bool) {
        self.collidable = collidable;
    }

    /// 设置投射阴影
    pub fn set_casts_shadow(&mut self, casts_shadow: bool) {
        self.castsShadow = casts_shadow;
    }

    /// 设置接收阴影
    pub fn set_receives_shadow(&mut self, receives_shadow: bool) {
        self.receivesShadow = receives_shadow;
    }

    /// 检查是否为根实体
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }

    /// 检查是否为叶子实体
    pub fn is_leaf(&self) -> bool {
        self.children_ids.is_empty()
    }

    /// 获取网格数量
    pub fn mesh_count(&self) -> usize {
        self.mesh_ids.len()
    }

    /// 获取子实体数量
    pub fn children_count(&self) -> usize {
        self.children_ids.len()
    }
}

/// XKT 实体树结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTEntityTree {
    pub entities: HashMap<String, XKTEntity>,
    pub root_entities: Vec<String>,
}

impl XKTEntityTree {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            root_entities: Vec::new(),
        }
    }

    /// 添加实体
    pub fn add_entity(&mut self, entity: XKTEntity) {
        let id = entity.id.clone();
        let is_root = entity.is_root();

        self.entities.insert(id.clone(), entity);

        if is_root && !self.root_entities.contains(&id) {
            self.root_entities.push(id);
        }
    }

    /// 获取实体
    pub fn get_entity(&self, id: &str) -> Option<&XKTEntity> {
        self.entities.get(id)
    }

    /// 获取可变实体
    pub fn get_entity_mut(&mut self, id: &str) -> Option<&mut XKTEntity> {
        self.entities.get_mut(id)
    }

    /// 建立父子关系
    pub fn set_parent_child_relationship(
        &mut self,
        parent_id: &str,
        child_id: &str,
    ) -> Result<(), String> {
        // 检查实体是否存在
        if !self.entities.contains_key(parent_id) {
            return Err(format!("Parent entity '{}' not found", parent_id));
        }
        if !self.entities.contains_key(child_id) {
            return Err(format!("Child entity '{}' not found", child_id));
        }

        // 检查是否会形成循环引用
        if self.would_create_cycle(parent_id, child_id) {
            return Err("Setting parent would create a cycle".to_string());
        }

        // 从根实体列表中移除子实体（如果存在）
        self.root_entities.retain(|id| id != child_id);

        // 设置父子关系
        if let Some(parent) = self.entities.get_mut(parent_id) {
            parent.add_child(child_id.to_string());
        }

        if let Some(child) = self.entities.get_mut(child_id) {
            child.set_parent(parent_id.to_string());
        }

        Ok(())
    }

    /// 检查是否会形成循环引用
    fn would_create_cycle(&self, parent_id: &str, child_id: &str) -> bool {
        let mut current = parent_id;
        while let Some(entity) = self.entities.get(current) {
            if let Some(parent) = &entity.parent_id {
                if parent == child_id {
                    return true;
                }
                current = parent;
            } else {
                break;
            }
        }
        false
    }

    /// 获取实体的所有祖先
    pub fn get_ancestors(&self, entity_id: &str) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut current = entity_id;

        while let Some(entity) = self.entities.get(current) {
            if let Some(parent_id) = &entity.parent_id {
                ancestors.push(parent_id.clone());
                current = parent_id;
            } else {
                break;
            }
        }

        ancestors
    }

    /// 获取实体的所有后代
    pub fn get_descendants(&self, entity_id: &str) -> Vec<String> {
        let mut descendants = Vec::new();
        self.collect_descendants(entity_id, &mut descendants);
        descendants
    }

    fn collect_descendants(&self, entity_id: &str, descendants: &mut Vec<String>) {
        if let Some(entity) = self.entities.get(entity_id) {
            for child_id in &entity.children_ids {
                descendants.push(child_id.clone());
                self.collect_descendants(child_id, descendants);
            }
        }
    }

    /// 获取实体树的深度
    pub fn get_depth(&self, entity_id: &str) -> usize {
        self.get_ancestors(entity_id).len()
    }

    /// 获取实体的兄弟节点
    pub fn get_siblings(&self, entity_id: &str) -> Vec<String> {
        if let Some(entity) = self.entities.get(entity_id) {
            if let Some(parent_id) = &entity.parent_id {
                if let Some(parent) = self.entities.get(parent_id) {
                    return parent
                        .children_ids
                        .iter()
                        .filter(|&id| id != entity_id)
                        .cloned()
                        .collect();
                }
            } else {
                // 根实体的兄弟节点是其他根实体
                return self
                    .root_entities
                    .iter()
                    .filter(|&id| id != entity_id)
                    .cloned()
                    .collect();
            }
        }
        Vec::new()
    }
}
