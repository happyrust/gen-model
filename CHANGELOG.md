# Changelog

## 2025-10-27

### Fixed
- **修复 SurrealDB 查询错误 "Expected any, got record"**
  - 问题：在多个查询中使用 `in.id != none` 条件导致 SurrealDB 执行记录存在性检查，触发类型不匹配错误
  - 影响：导致模型生成过程中 panic，错误信息为 "更新模型数据失败: Internal error: Expected any, got record"
  - 修复位置：
    - `src/fast_model/manifold_bool.rs` (第 79-85, 286-290 行) - 移除 2 处 `in.id != none` 条件
    - `src/fast_model/occ_generate.rs` (第 705-713 行) - 移除 `in.id != none` 条件
    - `src/web_ui/handlers.rs` (第 1730-1735 行) - 移除 `in.id != none` 条件
  - 原理：`inst_relate` 关系表的 `in` 字段总是指向有效的 `pe:{refno}`，不需要额外的存在性检查
  - 结果：程序现在可以正常运行，成功处理 GLB 模型导出

### Changed
- **优化 SurrealDB 查询性能**
  - 移除冗余的 `in.id != none` 检查条件，减少不必要的数据库操作
  - 简化查询逻辑，提升查询效率

## 2025-10-16

### Fixed
- **修复 `get_ancestor_attmaps` 中 NONE 值导致的反序列化失败问题**
  - 问题：`fn::ancestor({}).refno.*` 查询返回的祖先链中包含 NONE 值（当节点的属性记录不存在时），导致 `try_into::<NamedAttrMap>()` 反序列化失败
  - 影响：导致 `get_world_transform` 无法获取世界变换，几何体生成被跳过
  - 修复：在 `AttributeQueryService::get_ancestor_attmaps` 和 `query.rs::get_ancestor_attmaps` 中使用 `filter_map` + `try_into().ok()` 过滤掉无法转换的 NONE 值
  - 相关提交：rs-core@2dd7c11, gen-model@f41002f4

### Added
- **为 `gen_prim_geos` 添加详细调试日志**
  - 添加函数入口/出口日志，记录总数量、批次策略
  - 添加每批次的详细执行日志（开始、完成、耗时）
  - 添加详细的错误处理日志，区分不同跳过原因（世界变换失败、brep_shape 创建失败等）
  - 添加实体处理进度跟踪（处理数、跳过数、发送次数）
  - 使用 `e3d_dbg!` 宏统一调试日志输出

## 2025-02-14

- 将 `external/rs-corel` 中的连接、类型封装全部合并进 `external/rs-core`。
- 主项目直接使用 `aios_core` 公开的运行时 / WebUI 接口，移除独立 `rs-corel` 依赖。
- Web UI 及增量数据模块改为使用 `aios_core` 的 `RecordId`、`Datetime` 与连接工具。
- 构建脚本验证通过（`cargo check`）。
