# 生成历史数据模型结合 Raphtory 的数据接口设计

## 一、当前系统分析

### 1.1 现有增量记录结构
当前系统使用 `IncrementRecord` 来记录模型的增量变化：

- **IncrGeoUpdateLog**: 几何模型修改记录
  - `prim_refnos`: 基本体模型修改的参考号
  - `loop_owner_refnos`: 拉伸体模型修改的参考号
  - `bran_hanger_refnos`: 元件库模型属性修改的参考号
  - `basic_cata_refnos`: 基础目录修改的参考号
  - `delete_refnos`: 删除的模型参考号

- **IncrEleUpdateLog**: 元素更新记录
  - `refno`: 参考号
  - `data_operate`: 操作类型（增删改）
  - `old_attr/new_attr`: 新旧属性对比
  - `timestamp`: 时间戳
  - `new_version/old_version`: 版本号

### 1.2 现有数据接口
`PdmsDataInterface` 提供了基础的数据访问接口，但缺少时序相关的查询能力。

## 二、Raphtory 图数据库特性

Raphtory 是一个时序图数据库，特别适合处理：
- 时间序列的图结构变化
- 版本历史追踪
- 增量更新和回滚
- 时间窗口查询

## 三、需要在 aios-core 中实现的数据接口

### 3.1 时序查询接口

```rust
/// 时序数据查询接口
#[async_trait]
pub trait TemporalDataInterface: Send + Sync {
    /// 查询指定时间点的模型状态
    async fn query_model_at_time(
        &self,
        refno: RefnoEnum,
        timestamp: DateTime<Utc>,
    ) -> anyhow::Result<Option<AttrMap>>;

    /// 查询时间范围内的变更历史
    async fn query_changes_in_range(
        &self,
        refno: RefnoEnum,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> anyhow::Result<Vec<IncrEleUpdateLog>>;

    /// 按会话号查询模型状态
    async fn query_model_at_sesno(
        &self,
        refno: RefnoEnum,
        sesno: u32,
    ) -> anyhow::Result<Option<AttrMap>>;

    /// 获取模型的完整变更历史
    async fn get_model_history(
        &self,
        refno: RefnoEnum,
    ) -> anyhow::Result<Vec<ModelVersion>>;
}
```

### 3.2 图结构时序接口

```rust
/// 图结构时序查询接口
#[async_trait]
pub trait GraphTemporalInterface: Send + Sync {
    /// 查询指定时间点的层级结构
    async fn query_hierarchy_at_time(
        &self,
        root_refno: RefnoEnum,
        timestamp: DateTime<Utc>,
    ) -> anyhow::Result<HierarchySnapshot>;

    /// 查询时间范围内的拓扑变化
    async fn query_topology_changes(
        &self,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> anyhow::Result<Vec<TopologyChange>>;

    /// 获取指定时间点的父子关系
    async fn get_children_at_time(
        &self,
        parent_refno: RefnoEnum,
        timestamp: DateTime<Utc>,
    ) -> anyhow::Result<Vec<RefnoEnum>>;

    /// 获取指定时间点的引用关系
    async fn get_references_at_time(
        &self,
        refno: RefnoEnum,
        timestamp: DateTime<Utc>,
    ) -> anyhow::Result<Vec<(String, RefnoEnum)>>;
}
```

### 3.3 增量同步接口

```rust
/// 增量数据同步接口
#[async_trait]
pub trait IncrementalSyncInterface: Send + Sync {
    /// 将增量数据写入 Raphtory
    async fn write_increment_to_graph(
        &self,
        increment: IncrEleUpdateLog,
    ) -> anyhow::Result<()>;

    /// 批量写入增量数据
    async fn batch_write_increments(
        &self,
        increments: Vec<IncrEleUpdateLog>,
    ) -> anyhow::Result<()>;

    /// 从指定检查点同步增量数据
    async fn sync_from_checkpoint(
        &self,
        checkpoint: Checkpoint,
    ) -> anyhow::Result<SyncResult>;

    /// 创建同步检查点
    async fn create_checkpoint(
        &self,
        name: String,
    ) -> anyhow::Result<Checkpoint>;
}
```

### 3.4 历史模型生成接口

```rust
/// 历史模型生成接口
#[async_trait]
pub trait HistoricalModelGenerator: Send + Sync {
    /// 生成指定时间点的几何模型
    async fn generate_geometry_at_time(
        &self,
        refno: RefnoEnum,
        timestamp: DateTime<Utc>,
        options: &ModelGenerationOptions,
    ) -> anyhow::Result<PlantGeoData>;

    /// 生成指定会话的几何模型
    async fn generate_geometry_at_sesno(
        &self,
        refno: RefnoEnum,
        sesno: u32,
        options: &ModelGenerationOptions,
    ) -> anyhow::Result<PlantGeoData>;

    /// 生成时间范围内的模型变化动画
    async fn generate_model_animation(
        &self,
        refno: RefnoEnum,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        frame_interval: Duration,
    ) -> anyhow::Result<Vec<(DateTime<Utc>, PlantGeoData)>>;

    /// 对比两个时间点的模型差异
    async fn compare_models_at_times(
        &self,
        refno: RefnoEnum,
        time1: DateTime<Utc>,
        time2: DateTime<Utc>,
    ) -> anyhow::Result<ModelDiff>;
}
```

### 3.5 性能优化接口

```rust
/// 性能优化相关接口
#[async_trait]
pub trait PerformanceOptimizationInterface: Send + Sync {
    /// 预计算指定时间点的模型快照
    async fn precompute_snapshot(
        &self,
        timestamp: DateTime<Utc>,
        refnos: Vec<RefnoEnum>,
    ) -> anyhow::Result<()>;

    /// 创建时间索引以加速查询
    async fn create_temporal_index(
        &self,
        index_name: String,
        time_granularity: TimeGranularity,
    ) -> anyhow::Result<()>;

    /// 缓存常用时间点的查询结果
    async fn cache_temporal_query(
        &self,
        query: TemporalQuery,
        ttl: Duration,
    ) -> anyhow::Result<()>;
}
```

## 四、数据结构定义

### 4.1 核心数据结构

```rust
/// 模型版本信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersion {
    pub refno: RefnoEnum,
    pub version: u32,
    pub timestamp: DateTime<Utc>,
    pub sesno: Option<u32>,
    pub author: String,
    pub changes: Vec<AttributeChange>,
}

/// 属性变更记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeChange {
    pub attr_name: String,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub change_type: ChangeType,
}

/// 层级结构快照
#[derive(Debug, Clone)]
pub struct HierarchySnapshot {
    pub timestamp: DateTime<Utc>,
    pub root: RefnoEnum,
    pub nodes: HashMap<RefnoEnum, NodeSnapshot>,
    pub edges: Vec<(RefnoEnum, RefnoEnum, EdgeType)>,
}

/// 拓扑变化记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyChange {
    pub timestamp: DateTime<Utc>,
    pub change_type: TopologyChangeType,
    pub affected_nodes: Vec<RefnoEnum>,
    pub details: TopologyChangeDetails,
}

/// 模型差异
#[derive(Debug, Clone)]
pub struct ModelDiff {
    pub added_nodes: Vec<RefnoEnum>,
    pub removed_nodes: Vec<RefnoEnum>,
    pub modified_nodes: Vec<(RefnoEnum, Vec<AttributeChange>)>,
    pub topology_changes: Vec<TopologyChange>,
}
```

### 4.2 查询参数结构

```rust
/// 时序查询参数
#[derive(Debug, Clone)]
pub struct TemporalQuery {
    pub target_refnos: Vec<RefnoEnum>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub attributes: Option<Vec<String>>,
    pub include_children: bool,
    pub max_depth: Option<u32>,
}

/// 模型生成选项
#[derive(Debug, Clone)]
pub struct ModelGenerationOptions {
    pub lod_level: LodLevel,
    pub include_invisible: bool,
    pub apply_boolean_ops: bool,
    pub coordinate_system: CoordinateSystem,
}
```

## 五、实现建议

### 5.1 分层架构
1. **数据访问层**: 封装 Raphtory API 调用
2. **缓存层**: 使用 Redis/内存缓存热点数据
3. **业务逻辑层**: 实现复杂的时序查询逻辑
4. **API 层**: 提供统一的接口给上层应用

### 5.2 性能优化策略
1. **时间分片**: 将历史数据按时间段分片存储
2. **增量计算**: 基于前一版本增量计算新版本
3. **并行处理**: 利用 Raphtory 的并行查询能力
4. **智能缓存**: 缓存常用时间点和热点模型

### 5.3 数据迁移方案
1. **历史数据导入**: 将现有 TiDB 中的增量记录导入 Raphtory
2. **双写过渡**: 新增量同时写入 TiDB 和 Raphtory
3. **逐步切换**: 按模块逐步切换到新接口

## 六、使用示例

```rust
// 查询特定时间点的模型
let model = temporal_interface
    .query_model_at_time(refno, timestamp)
    .await?;

// 生成历史模型
let historical_geo = historical_generator
    .generate_geometry_at_sesno(refno, sesno, &options)
    .await?;

// 获取模型变更历史
let history = temporal_interface
    .get_model_history(refno)
    .await?;

// 对比两个版本
let diff = historical_generator
    .compare_models_at_times(refno, time1, time2)
    .await?;
```

## 七、总结

通过实现上述接口，可以充分利用 Raphtory 的时序图数据库特性，为生成历史数据模型提供强大的支持。这些接口涵盖了时序查询、增量同步、历史模型生成等核心功能，能够满足工程模型版本管理的需求。