# gen_geos 数据查询接口分析文档

## 概述

`gen_geos` 是一个用于生成几何体数据的核心功能模块，主要负责从数据库中查询相关数据并生成三维模型。本文档详细分析了 `gen_geos` 使用的所有数据查询接口及其在数据库接口层的实现情况。

## 主要函数

### 1. `gen_all_geos_data`
- **位置**: src/fast_model/gen_model.rs:165
- **功能**: 生成所有几何体数据的主入口函数
- **参数**: 
  - `manual_refnos`: 手动指定的引用号列表
  - `db_option`: 数据库选项配置
  - `incr_updates`: 增量更新日志

### 2. `gen_geos_data_by_dbnum`
- **位置**: src/fast_model/gen_model.rs:313
- **功能**: 根据数据库编号生成几何体数据
- **参数**: 
  - `dbno`: 数据库编号
  - `db_option_arc`: 数据库选项的Arc指针
  - `sender`: 形状实例数据的发送通道

### 3. `gen_geos_data`
- **位置**: src/fast_model/gen_model.rs:537
- **功能**: 核心的几何体数据生成函数
- **参数**: 
  - `dbno`: 可选的数据库编号
  - `manual_refnos`: 手动指定的引用号列表
  - `db_option`: 数据库选项
  - `incr_updates`: 增量更新日志
  - `sender`: 数据发送通道

## 数据查询接口分析

### 1. 数据库查询接口

#### 1.1 `query_type_refnos_by_dbnum`
- **功能**: 根据类型名称和数据库编号查询参考号
- **使用场景**:
  - 查询 ZONE 类型 (gen_model.rs:320)
  - 查询 PLOO 类型 (gen_model.rs:344)
  - 查询 BRAN/HANG 类型 (gen_model.rs:378)
  - 查询 LOOP 相关类型 (gen_model.rs:475)
  - 查询 PRIM 基本体类型 (gen_model.rs:499)
  - 查询 SITE 类型 (gen_model.rs:292, 591)

#### 1.2 `query_use_cate_refnos_by_dbnum`
- **功能**: 查询使用元件库的参考号
- **使用场景**: 查询元件库使用情况 (gen_model.rs:436)

#### 1.3 `query_mdb_db_nums`
- **功能**: 查询所有 MDB 数据库编号
- **使用场景**: 获取所有需要处理的数据库 (gen_model.rs:205)

### 2. 层级关系查询接口

#### 2.1 `query_multi_children_refnos`
- **功能**: 查询多个节点的子节点参考号
- **使用场景**: 
  - 查询 BRAN/HANG 的子节点 (gen_model.rs:84, 135)

#### 2.2 `query_multi_deep_versioned_children_filter_inst`
- **功能**: 深度查询带版本过滤的子节点
- **使用场景**:
  - 查询 PLOO 类型子节点 (gen_model.rs:634)
  - 查询 BRAN/HANG 类型子节点 (gen_model.rs:673)
  - 查询 LOOP 类型子节点 (gen_model.rs:807)
  - 查询 PRIM 类型子节点 (gen_model.rs:837)

#### 2.3 `query_multi_deep_children_filter_spre`
- **功能**: 深度查询带 SPRE 过滤的子节点
- **使用场景**: 查询独立使用的元件库 (gen_model.rs:730)

#### 2.4 `get_children_pes`
- **功能**: 获取节点的直接子元素
- **使用场景**: 
  - 获取 BRAN 下的子节点 (gen_model.rs:390, 750)

### 3. 元件库相关查询接口

#### 3.1 `query_group_by_cata_hash`
- **功能**: 根据元件库哈希值分组查询
- **使用场景**:
  - 分组管道/支吊架元件库 (gen_model.rs:397)
  - 分组独立元件库 (gen_model.rs:446, 683, 735)

### 4. 属性查询接口

#### 4.1 `get_named_attmap`
- **功能**: 获取命名属性映射
- **使用场景**:
  - 获取 PLOO 的属性 (gen_model.rs:648)
  - 获取 TUBI 的属性 (gen_model.rs:894)

#### 4.2 `get_pe`
- **功能**: 获取元素基本信息
- **使用场景**: 获取元件库元素信息 (gen_model.rs:695)

### 5. SQL 查询接口

#### 5.1 `SUL_DB.query`
- **功能**: 执行 SQL 查询
- **使用场景**:
  - 查询 PLOO 的 SJUS 对齐信息 (gen_model.rs:360)
  - 查询 BRAN/HANG 的子节点 (gen_model.rs:718)
  - 查询层级关系 (gen_model.rs:1196)

### 6. 数据处理接口

#### 6.1 `save_instance_data`
- **功能**: 保存实例数据到数据库
- **使用场景**: 保存生成的几何体实例数据 (gen_model.rs:182, 229)

#### 6.2 `gen_meshes_in_db`
- **功能**: 在数据库中生成网格数据
- **使用场景**: 
  - 生成 PRIM 模型 (gen_model.rs:63)
  - 生成 LOOP 模型 (gen_model.rs:69)
  - 生成元件库模型 (gen_model.rs:75)
  - 生成 BRAN/HANG 模型 (gen_model.rs:92)

#### 6.3 `booleans_meshes_in_db`
- **功能**: 执行布尔运算处理网格
- **使用场景**: 对各类模型执行布尔运算 (gen_model.rs:114, 120, 125, 142)

## 数据流程总结

1. **初始化阶段**:
   - 查询数据库编号列表
   - 根据配置过滤数据库

2. **数据收集阶段**:
   - 查询各类型节点（ZONE, PLOO, BRAN, HANG, LOOP, PRIM等）
   - 构建层级关系
   - 收集元件库信息

3. **几何体生成阶段**:
   - 处理元件库模型（cata_model）
   - 处理循环模型（loop_model）
   - 处理基本体模型（prim_model）

4. **后处理阶段**:
   - 执行网格生成
   - 执行布尔运算
   - 保存实例数据

## 性能优化策略

1. **批处理**: 
   - BRAN/HANG 节点按20个一批处理
   - PLOO 节点按200个一批查询
   - 根节点按100个一批处理

2. **并发处理**:
   - 使用 `tokio::spawn` 并发处理不同类型的模型
   - 使用 `FuturesUnordered` 管理异步任务

3. **缓存机制**:
   - 缓存 PLOO 的对齐信息（loop_sjus_map）
   - 缓存元件库分组信息

## 数据库接口层实现情况分析

### 1. PdmsDataInterface trait 定义
- **位置**: src/data_interface/interface.rs
- **功能**: 定义了数据访问的统一接口规范
- **包含方法**: 
  - get_attr: 获取属性
  - get_type_name: 获取类型名称
  - get_children_refs: 获取子节点引用
  - get_world_transform: 获取世界变换
  - 等等...

### 2. AiosDBManager 实现
- **位置**: src/data_interface/tidb_manager.rs
- **功能**: 实现 PdmsDataInterface trait 的具体数据库管理器

### 3. 接口实现覆盖情况

#### 已实现的接口
1. **get_attr**: 通过 aios_core::get_named_attmap 实现，带缓存机制
2. **get_type_name**: 通过 aios_core::get_type_name 实现

#### 未在 PdmsDataInterface 中定义的查询接口
以下是 gen_geos 使用但未在 PdmsDataInterface trait 中定义的接口：

1. **query_type_refnos_by_dbnum** - 直接调用 aios_core
2. **query_use_cate_refnos_by_dbnum** - 直接调用 aios_core
3. **query_mdb_db_nums** - 直接调用 aios_core
4. **query_multi_children_refnos** - 直接调用 aios_core
5. **query_multi_deep_versioned_children_filter_inst** - 直接调用 aios_core
6. **query_multi_deep_children_filter_spre** - 直接调用 aios_core
7. **query_group_by_cata_hash** - 直接调用 aios_core
8. **get_children_pes** - 直接调用 aios_core
9. **get_pe** - 直接调用 aios_core
10. **SUL_DB.query** - 直接使用全局数据库实例

### 4. 架构问题分析

#### 问题1: 接口层绕过
- gen_geos 大量直接调用 aios_core 的函数，绕过了 PdmsDataInterface 抽象层
- 这导致数据访问逻辑分散，难以统一管理和切换数据源

#### 问题2: 接口定义不完整
- PdmsDataInterface 没有涵盖所有需要的查询功能
- 缺少批量查询、分组查询等高级查询接口

#### 问题3: 全局依赖
- 直接使用 SUL_DB 全局实例，增加了耦合度
- 难以进行单元测试和数据源切换

### 5. 改进建议

1. **扩展 PdmsDataInterface**：
   - 添加缺失的查询接口定义
   - 支持批量操作和高级查询

2. **统一数据访问**：
   - 所有数据查询都通过 PdmsDataInterface 进行
   - 避免直接调用 aios_core 函数

3. **依赖注入**：
   - 消除全局数据库实例依赖
   - 通过接口注入数据访问对象

## 注意事项

1. 所有查询接口都支持历史数据的查询（通过 `include_history` 参数）
2. 支持增量更新模式，只处理变更的数据
3. 支持手动指定参考号列表进行处理
4. 支持调试模式，可以只处理特定的参考号
5. 当前实现存在架构问题，数据访问层抽象不够彻底