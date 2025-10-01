# 使用 Raphtory 实现 sesno 之间增量变化计算的可行性分析

## 一、Raphtory 时序图特性分析

### 1.1 Raphtory 核心能力
Raphtory 是一个时序图数据库，具有以下特性：

1. **时间维度管理**
   - 节点和边都带有时间戳
   - 支持时间窗口查询
   - 可以查询任意时间点的图快照

2. **变更追踪**
   - 自动记录图结构的演化历史
   - 支持时间范围内的变更查询
   - 可以获取节点/边的添加、删除、更新历史

3. **高效的时间切片**
   - 内置时间索引优化
   - 支持并行时间窗口计算

## 二、基于 Raphtory 的增量计算实现方案

### 2.1 数据模型设计

```rust
// 在 Raphtory 中存储模型数据的方案
pub struct RaphtoryModelGraph {
    graph: Graph,
}

impl RaphtoryModelGraph {
    /// 添加模型节点（带 sesno 时间戳）
    pub fn add_model_node(&mut self, refno: RefnoEnum, sesno: u32, attrs: AttrMap) {
        let node = self.graph.add_node(
            refno.to_string(),
            sesno as i64, // 时间戳使用 sesno
            HashMap::new(),
        );
        
        // 添加属性作为节点的时序属性
        for (key, value) in attrs.iter() {
            node.add_property(key, value, sesno as i64);
        }
    }

    /// 添加层级关系（带 sesno 时间戳）
    pub fn add_hierarchy_edge(&mut self, parent: RefnoEnum, child: RefnoEnum, sesno: u32) {
        self.graph.add_edge(
            parent.to_string(),
            child.to_string(),
            sesno as i64,
            HashMap::from([("type", "hierarchy")]),
        );
    }
}
```

### 2.2 增量计算实现

```rust
/// 使用 Raphtory 计算两个 sesno 之间的增量
pub async fn calculate_increments_with_raphtory(
    graph: &RaphtoryModelGraph,
    from_sesno: u32,
    to_sesno: u32,
) -> anyhow::Result<IncrGeoUpdateLog> {
    let mut result = IncrGeoUpdateLog::default();
    
    // 1. 获取时间窗口内的所有变更
    let window = graph.graph.window(from_sesno as i64, to_sesno as i64);
    
    // 2. 查找新增的节点
    let new_nodes = window.nodes()
        .filter(|node| {
            let creation_time = node.earliest_time();
            creation_time > from_sesno as i64 && creation_time <= to_sesno as i64
        });
    
    // 3. 查找删除的节点（存在于 from_sesno 但不存在于 to_sesno）
    let snapshot_from = graph.graph.at(from_sesno as i64);
    let snapshot_to = graph.graph.at(to_sesno as i64);
    
    for node in snapshot_from.nodes() {
        if !snapshot_to.has_node(node.id()) {
            result.delete_refnos.insert(RefnoEnum::from_str(&node.id())?);
        }
    }
    
    // 4. 查找修改的节点（属性变化）
    for node in window.nodes() {
        let refno = RefnoEnum::from_str(&node.id())?;
        let changes = node.properties()
            .changes_between(from_sesno as i64, to_sesno as i64);
        
        if !changes.is_empty() {
            // 根据节点类型分类
            match get_node_type(&node) {
                "PRIM" => result.prim_refnos.insert(refno),
                "LOOP" => result.loop_owner_refnos.insert(refno),
                "BRAN" | "HANGER" => result.bran_hanger_refnos.insert(refno),
                "CATA" => result.basic_cata_refnos.insert(refno),
                _ => false,
            };
        }
    }
    
    Ok(result)
}
```

## 三、实现复杂度评估

### 3.1 优势
1. **原生时序支持**: Raphtory 内置时间维度，无需额外实现
2. **高效查询**: 时间窗口查询经过优化，性能良好
3. **自动变更追踪**: 无需手动记录每次变更
4. **并行计算**: 支持多个时间窗口的并行处理

### 3.2 挑战
1. **数据迁移**: 需要将现有数据导入 Raphtory
2. **属性存储**: 复杂的 AttrMap 需要序列化存储
3. **查询转换**: 需要适配现有的查询接口
4. **性能调优**: 大规模数据可能需要优化策略

### 3.3 实现难度评估
- **难度等级**: 中等
- **开发时间**: 2-3 周
- **主要工作**:
  - 数据模型映射 (20%)
  - 查询接口适配 (30%)
  - 增量计算逻辑 (30%)
  - 性能优化 (20%)

## 四、具体实现建议

### 4.1 分阶段实施

#### 第一阶段：原型验证
```rust
// 简化版本，验证核心功能
pub struct SimpleRaphtoryAdapter {
    graph: Graph,
}

impl SimpleRaphtoryAdapter {
    pub fn add_model_change(&mut self, refno: RefnoEnum, sesno: u32, change_type: &str) {
        self.graph.add_node(refno.to_string(), sesno as i64, vec![
            ("change_type", change_type),
            ("sesno", &sesno.to_string()),
        ]);
    }
    
    pub fn get_changes_between(&self, from: u32, to: u32) -> Vec<(RefnoEnum, String)> {
        self.graph.window(from as i64, to as i64)
            .nodes()
            .map(|n| (
                RefnoEnum::from_str(&n.id()).unwrap(),
                n.properties().get("change_type").unwrap()
            ))
            .collect()
    }
}
```

#### 第二阶段：完整实现
```rust
pub struct RaphtoryIncrementCalculator {
    graph: Graph,
    attr_store: HashMap<(RefnoEnum, u32), AttrMap>, // 外部存储复杂属性
}

impl RaphtoryIncrementCalculator {
    /// 完整的增量计算，包括属性对比
    pub async fn calculate_detailed_increments(
        &self,
        from_sesno: u32,
        to_sesno: u32,
    ) -> anyhow::Result<Vec<IncrEleUpdateLog>> {
        let mut updates = Vec::new();
        
        // 获取变更的节点
        let changed_nodes = self.get_changed_nodes(from_sesno, to_sesno)?;
        
        for node_id in changed_nodes {
            let refno = RefnoEnum::from_str(&node_id)?;
            
            // 获取新旧属性
            let old_attr = self.attr_store.get(&(refno, from_sesno))
                .cloned()
                .unwrap_or_default();
            let new_attr = self.attr_store.get(&(refno, to_sesno))
                .cloned()
                .unwrap_or_default();
            
            // 判断操作类型
            let operation = if old_attr.is_empty() && !new_attr.is_empty() {
                EleOperation::Create
            } else if !old_attr.is_empty() && new_attr.is_empty() {
                EleOperation::Delete
            } else {
                EleOperation::Update
            };
            
            updates.push(IncrEleUpdateLog {
                refno,
                data_operate: operation,
                old_attr,
                new_attr,
                new_version: to_sesno,
                old_version: from_sesno,
                timestamp: self.get_sesno_timestamp(to_sesno)?,
                ..Default::default()
            });
        }
        
        Ok(updates)
    }
}
```

### 4.2 性能优化策略

1. **批量操作**
```rust
pub async fn batch_import_to_raphtory(
    changes: Vec<(RefnoEnum, u32, AttrMap)>,
) -> anyhow::Result<()> {
    // 批量导入，减少开销
    let batch_size = 1000;
    for chunk in changes.chunks(batch_size) {
        // 批量处理
    }
    Ok(())
}
```

2. **缓存策略**
```rust
pub struct CachedRaphtoryQuery {
    graph: Arc<Graph>,
    cache: DashMap<(u32, u32), IncrGeoUpdateLog>,
}
```

3. **索引优化**
```rust
// 为常用查询创建索引
graph.create_index("model_type");
graph.create_index("sesno");
```

## 五、总结

### 5.1 可行性结论
使用 Raphtory 实现两个 sesno 之间的增量变化计算是**完全可行的**，而且具有以下优势：
- 原生支持时序查询，实现相对简单
- 性能优秀，特别是时间窗口查询
- 可扩展性好，支持复杂的时序分析

### 5.2 实现建议
1. **先做原型验证**，确保核心功能满足需求
2. **分阶段迁移**，先迁移增量计算，再扩展到其他功能
3. **保留双写模式**，确保平滑过渡
4. **重点优化热点查询**，如常用时间窗口

### 5.3 替代方案
如果 Raphtory 的学习成本较高，可以考虑：
1. 基于现有 TiDB + 缓存层实现
2. 使用 TimescaleDB（PostgreSQL 时序扩展）
3. 自研轻量级时序索引

总的来说，Raphtory 是一个很好的选择，特别适合处理工程模型的版本演化和增量计算。