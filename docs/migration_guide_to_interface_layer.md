# gen_geos 数据接口层迁移指南

## 概述

本文档说明如何将现有代码从直接调用 `aios_core` 函数迁移到使用新的数据接口层架构。

## 架构变化

### 旧架构（直接调用）
```rust
// 直接调用 aios_core
let attrs = aios_core::get_named_attmap(refno).await?;
let children = aios_core::query_multi_children_refnos(&refnos).await?;
```

### 新架构（接口层）
```rust
// 通过数据接口层调用
let data_interface: Arc<dyn PdmsDataInterface> = get_data_interface();
let attrs = data_interface.get_named_attmap(refno).await?;
let children = data_interface.query_multi_children_refnos(&refnos).await?;
```

## 迁移步骤

### 1. 创建数据接口

使用 `DatabaseFactory` 创建数据接口：

```rust
use aios_database::data_interface::database_factory::{DatabaseFactory, DatabaseConfig, DatabaseType};

let config = DatabaseConfig {
    db_type: DatabaseType::SurrealDB,  // 或 TiDB, Hybrid
    db_option: DbOption::default(),
    project_path: "./project".to_string(),
    projects: vec![],
};

let data_interface = DatabaseFactory::create_interface(config).await?;
```

### 2. 修改函数签名

将函数修改为接受数据接口参数：

```rust
// 旧版本
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    // 直接调用 aios_core
}

// 新版本
pub async fn gen_all_geos_data<T: PdmsDataInterface>(
    data_interface: &T,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    // 通过 data_interface 调用
}
```

### 3. 替换具体调用

| 旧调用 | 新调用 |
|--------|--------|
| `aios_core::get_named_attmap(refno)` | `data_interface.get_named_attmap(refno)` |
| `aios_core::query_type_refnos_by_dbnum(...)` | `data_interface.query_type_refnos_by_dbnum(...)` |
| `aios_core::query_multi_children_refnos(...)` | `data_interface.query_multi_children_refnos(...)` |
| `aios_core::get_children_pes(refno)` | `data_interface.get_children_pes(refno)` |
| `aios_core::save_instance_data(...)` | `data_interface.save_instance_data(...)` |

### 4. 处理 SQL 查询

由于 `execute_sql` 方法不是 trait 的一部分，需要特殊处理：

```rust
// 旧版本
let response = aios_core::execute_sql::<T>(sql).await?;

// 新版本 - 需要使用具体的查询方法或自定义实现
// 例如，使用特定的查询接口
let refnos = data_interface.query_type_refnos_by_dbnum(...).await?;
```

## 完整示例

### 迁移前
```rust
pub async fn process_model(refno: RefnoEnum) -> anyhow::Result<()> {
    let attrs = aios_core::get_named_attmap(refno).await?;
    let children = aios_core::get_children_pes(refno).await?;
    
    for child in children {
        // 处理子节点
    }
    
    Ok(())
}
```

### 迁移后
```rust
pub async fn process_model<T: PdmsDataInterface>(
    data_interface: &T,
    refno: RefnoEnum
) -> anyhow::Result<()> {
    let attrs = data_interface.get_named_attmap(refno).await?;
    let children = data_interface.get_children_pes(refno).await?;
    
    for child in children {
        // 处理子节点
    }
    
    Ok(())
}
```

## 依赖注入示例

使用 `DiContainer` 进行依赖注入：

```rust
use aios_database::data_interface::database_factory::DiContainer;

// 注册数据接口
let mut container = DiContainer::new();
let data_interface = DatabaseFactory::create_default().await?;
container.register("data_interface", data_interface);

// 在需要时获取
let data_interface = container.resolve::<Arc<dyn PdmsDataInterface>>("data_interface")
    .expect("Data interface not found");
```

## 测试策略

1. **单元测试**：使用 `MockDataManager` 进行测试
2. **集成测试**：使用实际的数据库连接
3. **性能测试**：比较新旧架构的性能差异

## 注意事项

1. **循环依赖**：避免在 trait 实现中调用依赖该 trait 的函数
2. **错误处理**：新架构使用 `anyhow::Result` 统一错误处理
3. **异步特性**：所有数据访问操作都是异步的
4. **类型安全**：利用 Rust 的类型系统确保接口正确使用

## 迁移检查清单

- [ ] 识别所有直接调用 `aios_core` 的位置
- [ ] 创建合适的 `DatabaseConfig`
- [ ] 修改函数签名以接受数据接口参数
- [ ] 替换所有直接调用为接口调用
- [ ] 处理特殊情况（如 SQL 查询）
- [ ] 更新测试代码
- [ ] 验证功能正确性
- [ ] 性能测试和优化

## 常见问题

### Q: 如何处理 execute_sql？
A: 由于泛型方法不能成为 dyn trait 的一部分，建议使用具体的查询方法或创建特定的 SQL 执行函数。

### Q: 如何选择数据库类型？
A: 使用 `DatabaseType` 枚举：
- `TiDB`: 传统 MySQL/TiDB 数据库
- `SurrealDB`: 新一代多模型数据库
- `Hybrid`: 混合模式，支持运行时切换

### Q: 如何处理性能问题？
A: 新架构通过接口抽象可能带来轻微的性能开销，但提供了更好的可维护性和可测试性。如果性能关键，可以使用泛型而非 trait object。