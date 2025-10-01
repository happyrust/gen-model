# SurrealDB 适配 gen_geos 分析报告

## 当前状态分析

### 1. 接口定义情况

在 `src/data_interface/interface.rs` 中，PdmsDataInterface trait 已经扩展了支持 gen_geos 所需的接口：

```rust
// ========== 新增的查询接口，用于支持 gen_geos ========== //

- query_type_refnos_by_dbnum          // 根据类型和数据库编号查询
- query_use_cate_refnos_by_dbnum      // 查询使用元件库的参考号
- query_mdb_db_nums                   // 查询所有 MDB 数据库编号
- query_multi_children_refnos         // 查询多个节点的子节点
- query_multi_deep_versioned_children_filter_inst  // 深度查询带版本过滤
- query_multi_deep_children_filter_spre             // 深度查询带 SPRE 过滤
- query_group_by_cata_hash            // 根据元件库哈希值分组
- get_children_pes                    // 获取节点的直接子元素
- get_pe                             // 获取元素基本信息
- get_named_attmap                   // 获取命名属性映射
- execute_sql                        // 执行 SQL 查询
- save_instance_data                 // 保存实例数据
- gen_meshes_in_db                   // 生成网格数据
- booleans_meshes_in_db              // 布尔运算处理网格
```

### 2. SurrealDBManager 实现情况

**严重问题：SurrealDBManager 没有实现这些新增的查询接口！**

当前 `src/data_interface/surreal_manager.rs` 只实现了基础的 PdmsDataInterface 接口，但**缺少所有 gen_geos 相关的查询接口实现**。

### 3. 架构问题

1. **gen_geos 直接调用 aios_core**：
   - gen_geos 函数中大量直接调用 `aios_core::query_*` 函数
   - 这些调用绕过了 PdmsDataInterface 抽象层
   - 导致无法通过替换数据库实现来切换数据源

2. **全局数据库依赖**：
   - 使用全局 `SUL_DB` 实例
   - 难以进行依赖注入和测试

3. **RuntimeDatabaseAdapter 未使用**：
   - 虽然定义了 RuntimeDatabaseAdapter，但 gen_geos 没有使用它

## 解决方案

### 方案一：完整实现 SurrealDBManager（推荐）

1. **在 SurrealDBManager 中实现所有缺失的接口**：

```rust
impl SurrealDBManager {
    // 实现 gen_geos 需要的所有查询接口
    async fn query_type_refnos_by_dbnum(...) -> anyhow::Result<Vec<RefnoEnum>> {
        // 实现 SurrealDB 查询逻辑
    }
    
    // ... 其他接口实现
}
```

2. **修改 gen_geos 使用接口而非直接调用**：
   - 将所有 `aios_core::query_*` 调用改为通过 PdmsDataInterface
   - 传入数据库接口实例而不是依赖全局变量

### 方案二：适配层方案（快速但不完美）

1. **创建一个适配层，将 aios_core 调用转发到 SurrealDB**：
   - 保持 gen_geos 代码不变
   - 在 aios_core 层面增加数据库切换逻辑

2. **问题**：
   - 仍然存在全局依赖
   - 架构不够清晰

### 方案三：重构 gen_geos（长期最佳）

1. **重构 gen_geos 函数签名**：
```rust
pub async fn gen_all_geos_data<T: PdmsDataInterface>(
    db_interface: &T,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool>
```

2. **优势**：
   - 完全解耦数据访问
   - 易于测试和维护
   - 支持多种数据源

## 当前阻塞点

1. **SurrealDBManager 缺少关键接口实现**
2. **gen_geos 硬编码依赖 aios_core 函数**
3. **缺少数据迁移和测试验证**

## 建议行动计划

### 短期（1-2周）
1. 实现 SurrealDBManager 中缺失的接口（至少实现核心查询接口）
2. 创建测试用例验证查询结果的正确性
3. 建立数据迁移工具，确保 SurrealDB 中有测试数据

### 中期（3-4周）
1. 修改 gen_geos 通过接口访问数据
2. 集成测试，验证模型生成的正确性
3. 性能优化和调优

### 长期（1-2月）
1. 完全重构数据访问层
2. 建立完整的抽象和依赖注入机制
3. 支持多种数据源的无缝切换

## 风险评估

1. **数据一致性风险**：需要确保 SurrealDB 查询结果与原 TiDB 一致
2. **性能风险**：某些复杂查询可能需要优化
3. **兼容性风险**：需要保证生成的模型数据格式正确

## 结论

当前 SurrealDB 的适配工作还未完成，主要是因为：
1. SurrealDBManager 没有实现 gen_geos 需要的查询接口
2. gen_geos 直接依赖底层实现而非通过接口

要让 gen_geos 正确运行在 SurrealDB 上，需要先完成接口实现，然后进行充分的测试验证。