# 数据库抽象接口使用指南

## 概述

本项目已完成数据库抽象接口的完善，支持多种数据库后端，包括 TiDB、MySQL 和 SurrealDB。通过统一的 `PdmsDataInterface` trait，您可以在不修改业务逻辑的情况下切换不同的数据库实现。

## 架构设计

### 核心组件

1. **PdmsDataInterface** - 统一的数据库操作接口 trait
2. **AiosDBManager** - TiDB/MySQL 实现
3. **SurrealDBManager** - SurrealDB 实现
4. **DatabaseFactory** - 数据库实例工厂

### 文件结构

```
src/data_interface/
├── interface.rs        # PdmsDataInterface trait 定义
├── tidb_manager.rs     # TiDB/MySQL 实现
├── surreal_manager.rs  # SurrealDB 实现
├── database_factory.rs # 工厂模式实现
└── tests.rs           # 单元测试
```

## 配置

在 `DbOption.toml` 配置文件中添加数据库类型配置：

```toml
# 数据库类型: "tidb", "mysql", "surrealdb"
db_type = "surrealdb"

# SurrealDB 配置
ip = "127.0.0.1"
port = "8000"
user = "root"
password = "root"
```

## 使用示例

### 1. 从配置创建数据库实例

```rust
use aios_database::data_interface::database_factory::DatabaseFactory;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 自动根据配置创建相应的数据库实例
    let db = DatabaseFactory::create_from_config().await?;
    
    // 使用统一接口
    let refno = RefU64::from(12345u64);
    let attrs = db.get_attr(refno).await?;
    
    Ok(())
}
```

### 2. 显式创建特定类型的数据库

```rust
use aios_database::data_interface::database_factory::{DatabaseFactory, DatabaseType};

let db_option = aios_core::get_db_option();
let surreal_db = DatabaseFactory::create_database(
    DatabaseType::SurrealDB,
    &db_option
).await?;
```

### 3. 使用全局 SurrealDB 实例

```rust
let db = DatabaseFactory::create_surreal_from_global().await?;
```

### 4. 类型向下转换

当需要访问特定数据库实现的专有功能时：

```rust
use aios_database::data_interface::database_factory::DatabaseAdapter;

let db = DatabaseFactory::create_from_config().await?;

if let Some(tidb) = db.as_tidb() {
    // 使用 TiDB 特定功能
} else if let Some(surreal) = db.as_surreal() {
    // 使用 SurrealDB 特定功能
}
```

## 主要接口方法

`PdmsDataInterface` trait 提供了以下核心方法：

- `get_attr(refno)` - 获取元素属性
- `get_type_name(refno)` - 获取元素类型名称
- `get_children_refs(refno)` - 获取子元素引用
- `get_ancestor_nodes(refno)` - 获取祖先节点
- `get_world_transform(refno)` - 获取世界坐标变换
- 更多方法请参见 `interface.rs`

## 测试

运行测试：

```bash
cargo test --features sql,grpc
```

## 注意事项

1. SurrealDB 实现中，某些同步方法（如 `get_owner`）由于 trait 限制暂时返回默认值
2. 建议在生产环境中使用缓存来优化性能
3. 确保数据库连接配置正确，特别是 SurrealDB 的 WebSocket 连接

## 扩展新的数据库支持

1. 创建新的管理器结构体
2. 实现 `PdmsDataInterface` trait
3. 在 `DatabaseFactory` 中添加新的数据库类型
4. 更新配置解析逻辑