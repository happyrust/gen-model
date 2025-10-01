# SurrealDB 迁移进度报告

## 已完成的工作

### 1. TiDB Manager 弃用处理
- ✅ 将 `tidb_manager.rs` 中的所有实现方法标记为已弃用
- ✅ 所有方法返回错误提示用户使用 SurrealDBManager
- ✅ 保留了结构体定义以维持编译兼容性

### 2. 数据库工厂更新
- ✅ 将默认数据库类型从 TiDB 改为 SurrealDB
- ✅ 在 `database_factory.rs` 中实现了 SurrealDB 接口创建
- ✅ 更新了混合模式，默认使用 SurrealDB，TiDB 作为备用
- ✅ 在 `runtime_adapter.rs` 中添加了 `new_with_surreal` 方法

### 3. SurrealDB 基础实现
- ✅ 创建了 `surreal_manager.rs` 实现 PdmsDataInterface
- ✅ 实现了基础连接和配置管理
- ✅ 支持从环境变量读取连接信息
- ✅ 实现了基础的元素查询方法（get_element, query_children）

### 4. 数据库模型设计
- ✅ 创建了 `surreal_schema.sql` 定义表结构
- ✅ 设计了以下核心表：
  - `pdms_elements` - 核心元素表
  - `owns` - 元素层级关系
  - `catalog_refs` - 元件库引用
  - `shape_instances` - 几何实例
  - `named_attributes` - 命名属性映射
  - `implicit_attributes` - 隐含属性
  - `world_transforms` - 世界坐标变换缓存
  - `mdb_worlds` - MDB世界节点
  - `increment_records` - 增量记录

## 待完成的工作

### 1. 核心查询接口实现
需要在 `surreal_manager.rs` 中完善以下方法的实现：
- [ ] `query_type_refnos_by_dbnum` - 按类型查询参考号
- [ ] `query_use_cate_refnos_by_dbnum` - 按元件库查询
- [ ] `query_multi_deep_versioned_children_filter_inst` - 深度查询子节点
- [ ] `query_group_by_cata_hash` - 按元件库哈希分组
- [ ] `get_named_attmap` - 获取命名属性映射

### 2. 模型生成接口实现
- [ ] `gen_all_geos_data` - 生成所有几何体数据
- [ ] `gen_geos_data_by_dbnum` - 按数据库编号生成
- [ ] `save_instance_data` - 保存实例数据
- [ ] `gen_meshes_in_db` - 生成网格数据
- [ ] `booleans_meshes_in_db` - 布尔运算处理

### 3. 数据迁移工具
- [ ] 创建从 TiDB 到 SurrealDB 的数据迁移脚本
- [ ] 实现数据验证和一致性检查
- [ ] 性能测试和优化

### 4. 缓存机制
- [ ] 实现 SurrealDB 的缓存策略
- [ ] 与现有的 DashMap 缓存集成
- [ ] 优化查询性能

### 5. 事务支持
- [ ] 实现事务处理机制
- [ ] 支持增量更新和回滚

## 技术挑战

1. **数据类型映射**
   - RefU64、RefnoEnum 等自定义类型需要正确序列化/反序列化
   - NamedAttrMap 的复杂结构需要映射到 SurrealDB 的对象类型

2. **图查询优化**
   - 深度遍历查询需要利用 SurrealDB 的图数据库特性
   - 复杂的关系查询需要优化

3. **性能考虑**
   - 批量操作的优化
   - 索引策略的设计
   - 查询缓存的实现

## 下一步计划

1. 完善 SurrealDBManager 中的核心查询方法实现
2. 创建单元测试验证功能正确性
3. 实现数据迁移工具
4. 进行性能测试和优化
5. 编写使用文档和迁移指南

## 使用方式

### 环境变量配置
```bash
export SURREAL_URL=ws://localhost:8000
export SURREAL_NS=aios
export SURREAL_DB=pdms
export SURREAL_USER=root
export SURREAL_PASS=root
```

### 代码使用示例
```rust
use crate::data_interface::database_factory::{DatabaseFactory, DatabaseConfig, DatabaseType};

// 创建 SurrealDB 接口
let config = DatabaseConfig {
    db_type: DatabaseType::SurrealDB,
    db_option: DbOption::default(),
    project_path: "./".to_string(),
    projects: vec![],
};

let data_interface = DatabaseFactory::create_interface(config).await?;

// 使用接口进行查询
let refnos = data_interface.query_type_refnos_by_dbnum(
    &["PIPE", "EQUI"],
    100,
    Some(true),
    false
).await?;
```