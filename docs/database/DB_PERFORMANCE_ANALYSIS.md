# 数据库性能对比分析

## 概述

本文档分析了当前系统中几何节点查询的实现方式，并提供了 SurrealDB 与 HelixDB 的性能对比方案。

## 当前 SurrealDB 查询实现

### 核心查询接口

系统通过 `aios_core` 库与 SurrealDB 交互，主要查询接口包括：

#### 1. 节点属性查询
```rust
// 获取完整属性映射
let attr_map = aios_core::get_named_attmap(refno).await?;

// 获取节点类型
let type_name = aios_core::get_type_name(refno).await?;
```

**实现位置**: `src/data_interface/tidb_manager.rs:72-89`

#### 2. 层级关系查询
```rust
// 获取子节点引用号列表
let children = aios_core::get_children_refnos(refno).await?;

// 获取子节点及其属性
let children_atts = aios_core::get_children_named_attmaps(refno).await?;
```

**实现位置**: `src/data_interface/tidb_manager.rs:288-311`

#### 3. 类型筛选查询
```rust
// 按类型获取节点集合
let refnos = db_manager.get_refnos_by_types(
    project_name,
    &["SITE"],  // 目标类型
    &[db_no]    // 数据库编号
).await?;
```

**实现位置**: `src/data_interface/tidb_manager.rs:315-326`

### Site 节点查询示例

从代码库中的 Site 节点查询模式：

```rust
// 示例 1: 获取 Site 根节点（db_manager.rs:42）
let refnos = self
    .get_refnos_by_types(
        db_option.project_name.as_str(),
        &["SITE"],
        &[db_no]
    )
    .await?;

// 示例 2: 批量查询 Site 节点（gen_model.rs:829）
let sites = query_type_refnos_by_dbnum(
    &["SITE"],
    dbno,
    None,
    include_history
).await?;

// 示例 3: 查询祖先节点中的 Site（children.rs:680）
let site = query_ancestor_of_type(refno, "SITE", &pool).await?;
```

### 几何节点查询流程

以生成模型时的几何查询为例（`src/fast_model/query.rs`）：

```rust
pub async fn query_gm_params(refno: RefnoEnum) -> anyhow::Result<Vec<GmParam>> {
    let mut gms = vec![];
    let mut children = vec![];

    // 1. 获取所有子节点及属性
    for c in aios_core::get_children_named_attmaps(refno).await? {
        // 2. 筛选几何类型节点
        if TOTAL_CATA_GEO_NOUN_NAMES.contains(&c.get_type_str()) {
            children.push(c.clone());
        }

        // 3. 处理嵌套负实体
        for cc in aios_core::get_children_named_attmaps(c.get_refno_or_default()).await? {
            if TOTAL_CATA_GEO_NOUN_NAMES.contains(&cc.get_type_str()) {
                children.push(cc);
            }
        }
    }

    // 4. 查询每个几何节点的参数
    for geo_am in children {
        if !geo_am.is_visible_by_level(None).unwrap_or(true) {
            continue;
        }
        let is_spro = geo_am.get_type_str() == "SPRO";
        let geom = query_gm_param(&geo_am, is_spro).await.unwrap_or_default();
        gms.push(geom);
    }

    Ok(gms)
}
```

### 查询性能特点

#### 优点
1. **接口简洁**: 统一的异步查询接口
2. **缓存支持**: 使用 `PDMS_ATT_MAP_CACHE` 缓存常用数据
3. **类型安全**: 强类型的 `RefU64` 和 `RefnoEnum`

#### 潜在性能瓶颈
1. **多次网络往返**: 每次查询都可能触发数据库调用
2. **递归查询**: 深度遍历需要多次数据库交互
3. **批量查询不足**: 缺少批量优化的查询接口

## 性能对比测试方案

### 测试场景

我创建了全面的性能对比测试 (`examples/db_performance_comparison.rs`)，包含：

#### 1. Site 节点基础查询
- **测试内容**:
  - 获取节点属性
  - 获取节点类型
  - 获取子节点列表
- **性能指标**: 单次查询延迟、吞吐量

#### 2. 批量子节点查询
- **测试内容**:
  - 查询 20 个父节点的子节点
  - 统计总耗时和平均耗时
- **性能指标**: 批量查询效率、QPS

#### 3. 递归遍历查询
- **测试内容**:
  - 从根节点深度遍历（最大深度 3）
  - 统计遍历节点数和查询次数
- **性能指标**: 递归查询性能、网络往返次数

### 性能指标

对比框架收集以下指标：

```rust
struct PerformanceMetrics {
    query_count: usize,        // 查询次数
    total_time_ms: u128,       // 总耗时
    avg_time_ms: f64,          // 平均耗时
    min_time_ms: u128,         // 最小耗时
    max_time_ms: u128,         // 最大耗时
    queries_per_second: f64,   // 查询速率
}
```

### 对比结果展示

测试程序会自动计算并展示：

1. **单项测试对比**
   - SurrealDB vs HelixDB 的详细指标
   - 性能倍数对比

2. **总体性能汇总**
   - 各测试场景的性能对比
   - 性能提升统计

## 运行测试

### 编译和运行

```bash
# 编译测试程序
cargo build --example db_performance_comparison

# 运行性能对比测试
cargo run --example db_performance_comparison
```

### 前置条件

1. 确保 SurrealDB 已启动并连接
2. 配置正确的数据库连接参数（`DbOption`）
3. 准备测试数据（Site 节点及子节点）

## 实现 HelixDB 查询接口

要获得真实的性能对比数据，需要实现 HelixDB 的查询接口。建议的实现结构：

### 1. 创建 HelixDB 客户端

```rust
// src/data_interface/helix_manager.rs

pub struct HelixDBManager {
    client: HelixClient,
    connection_pool: HelixPool,
}

impl HelixDBManager {
    pub async fn connect(config: &HelixConfig) -> anyhow::Result<Self> {
        // 实现 HelixDB 连接
        todo!()
    }
}
```

### 2. 实现 PdmsDataInterface trait

```rust
#[async_trait]
impl PdmsDataInterface for HelixDBManager {
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<NamedAttrMap> {
        // 实现属性查询
        todo!()
    }

    async fn get_type_name(&self, refno: RefU64) -> String {
        // 实现类型查询
        todo!()
    }

    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        // 实现子节点查询
        todo!()
    }

    async fn get_refnos_by_types(
        &self,
        project: &str,
        att_types: &[&str],
        dbnos: &[i32],
    ) -> anyhow::Result<RefU64Vec> {
        // 实现按类型查询
        todo!()
    }
}
```

### 3. 更新性能测试代码

在 `examples/db_performance_comparison.rs` 中替换 TODO 部分：

```rust
async fn test_site_node_query_helixdb(
    helix_manager: &HelixDBManager,
    site_refno: RefU64,
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("Site节点查询", "HelixDB");

    // 实际的 HelixDB 查询
    let start = Instant::now();
    let attr_map = helix_manager.get_attr(site_refno).await?;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    let start = Instant::now();
    let type_name = helix_manager.get_type_name(site_refno).await;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    let start = Instant::now();
    let children = helix_manager.get_children_refs(site_refno).await?;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    metrics.finalize();
    Ok(metrics)
}
```

## HelixDB 优化建议

基于 SurrealDB 的查询模式，HelixDB 可以考虑以下优化：

### 1. 批量查询接口

```rust
// 支持一次请求获取多个节点的数据
async fn batch_get_attrs(&self, refnos: &[RefU64]) -> anyhow::Result<Vec<NamedAttrMap>>;

// 支持批量获取子节点
async fn batch_get_children(&self, parent_refnos: &[RefU64]) -> anyhow::Result<HashMap<RefU64, RefU64Vec>>;
```

### 2. 递归查询优化

```rust
// 单次请求获取子树
async fn get_subtree(
    &self,
    root: RefU64,
    max_depth: usize
) -> anyhow::Result<NodeTree>;

// 支持谓词下推的递归查询
async fn query_descendants(
    &self,
    root: RefU64,
    filter: NodeFilter,
    max_depth: usize,
) -> anyhow::Result<Vec<RefU64>>;
```

### 3. 图查询优化

```rust
// 支持图遍历模式
async fn traverse_graph(
    &self,
    start_nodes: &[RefU64],
    traversal_pattern: TraversalPattern,
) -> anyhow::Result<GraphResult>;
```

## 性能优化建议

### 对于当前 SurrealDB 实现

1. **增加批量查询接口**: 减少网络往返次数
2. **优化缓存策略**: 扩大缓存范围，延长缓存时效
3. **预加载优化**: 对于常见查询模式进行预加载
4. **连接池优化**: 调整连接池大小以适应并发需求

### 对于 HelixDB 设计

1. **原生图查询支持**: 利用图数据库的优势
2. **智能索引**: 针对层级查询优化索引
3. **查询计划缓存**: 缓存常用查询计划
4. **批量操作优化**: 提供高效的批量接口

## 测试数据准备

为了获得有意义的对比结果，建议准备：

1. **小规模测试**:
   - 1 个 Site 节点
   - 10-20 个子节点
   - 深度 2-3 的层级结构

2. **中等规模测试**:
   - 5-10 个 Site 节点
   - 100-200 个子节点
   - 深度 3-4 的层级结构

3. **大规模测试**:
   - 50+ 个 Site 节点
   - 1000+ 个子节点
   - 深度 5+ 的层级结构

## 下一步行动

1. ✅ 分析当前 SurrealDB 查询实现
2. ✅ 创建性能对比测试框架
3. ⏳ 实现 HelixDB 查询接口
4. ⏳ 准备测试数据集
5. ⏳ 运行完整性能对比测试
6. ⏳ 分析结果并优化

## 参考文件

- 查询接口定义: `src/data_interface/interface.rs`
- SurrealDB 实现: `src/data_interface/tidb_manager.rs`
- 几何查询示例: `src/fast_model/query.rs`
- 性能测试程序: `examples/db_performance_comparison.rs`
- Site 节点管理: `src/data_interface/db_manager.rs`