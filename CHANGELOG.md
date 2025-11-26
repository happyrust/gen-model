# Changelog

## 2025-11-26

### Added
- **为 `test_full_boolean_flow` 添加 OBJ 模型导出功能**
  - 功能：在布尔运算完成后自动导出布尔前后的 OBJ 模型用于可视化验证
  - 实现位置：`src/bin/test_full_boolean_flow.rs`
  - 新增函数：`get_mesh_dir_with_lod()` - 根据配置获取正确的 LOD mesh 目录
  - 导出文件：
    - `test_output/boolean_exports/before_boolean_{refno}.obj` - 布尔运算前的正实体
    - `test_output/boolean_exports/after_boolean_{refno}.obj` - 布尔运算后的结果
  - 用途：
    - 可在 Blender、MeshLab 等 3D 软件中打开查看
    - 对比布尔运算前后的几何变化
    - 验证负实体是否正确被减去
  - 依赖：`aios_database::fast_model::export_model::export_obj::export_obj_for_refnos`

### Changed
- **更新完整布尔运算测试指南**
  - 文件：`llmdoc/guides/complete_boolean_test_guide.md`
  - 更新内容：
    - 在测试流程中添加"步骤 4: 导出 OBJ 模型"
    - 更新测试输出示例，包含 OBJ 导出日志
    - 添加输出文件路径和使用说明
    - 在总结部分添加 OBJ 相关的关键指标和成功标准
  - 新增章节：详细说明如何导出和查看 OBJ 模型

### Documentation
- **新增 `llmdoc/agent/boolean_obj_export_implementation.md`**
  - 完整记录 OBJ 导出功能的实现细节
  - 包含技术实现、使用方法、示例输出
  - 提供后续改进建议

## 2025-10-27

### Fixed
- **修复 SurrealDB 查询错误 "Expected any, got record"**
  - 问题：在多个查询中使用 `in.id != none` 条件导致 SurrealDB 执行记录存在性检查，触发类型不匹配错误
  - 影响：导致模型生成过程中 panic，错误信息为 "更新模型数据失败: Internal error: Expected any, got record"
  - 修复位置：
    - `src/fast_model/manifold_bool.rs` (第 79-85, 286-290 行) - 移除 2 处 `in.id != none` 条件
    - `src/fast_model/occ_generate.rs` (第 705-713 行) - 移除 `in.id != none` 条件
    - `src/web_server/handlers.rs` (第 1730-1735 行) - 移除 `in.id != none` 条件
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
