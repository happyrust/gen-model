# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- 新增XKT生成器前端组件 (frontend/v0-aios-database-management/app/xkt-generator/)
  - 实现XKT生成器页面和路由配置
  - 添加XKT生成器核心组件和测试文件
  - 集成Alert, Breadcrumb, Switch, Tabs等UI组件
- 添加XKT生成器文档 (frontend/XKT_GENERATOR_README.md)
- 新增XKT数据库生成器Rust实现 (src/xkt_generator/xkt_database_generator.rs)
- 添加XKT加载验证脚本 (tests/scripts/verify_xkt_load.cjs)
- 新增Kuzu数据库测试示例 (examples/test_kuzu_simple.rs)

### Changed

- 重组文档结构，将分散的文档迁移到docs目录下的分类子目录
  - docs/api/ - API相关文档
  - docs/architecture/ - 架构设计文档
  - docs/database/ - 数据库相关文档
  - docs/deployment/ - 部署文档
  - docs/guides/ - 指南和教程
  - docs/legacy/ - 遗留文档
  - docs/reports/ - 报告文档
  - docs/xkt-generator/ - XKT生成器文档
- 更新.gitignore，添加对外部模块的忽略规则
- 清理根目录下的临时文件和测试文件

## [2.4] - 2025-09-02

### Changed

- 更新Cargo依赖配置，优化项目依赖管理

## [2.3] - 2025-08-04

### Added

- 完成GRPC微服务架构实现和调试
- 实现GRPC微服务基础架构
- 添加自动增量更新文件的修改，启动时会检查当前数据库和E3d数据库的一致性
- 新增通过数据库编号及文件名查询最新会话号的功能
- 新增`define_dbnum_event_array_id`函数，用于处理`pe`的`id`为数组的情况
- 为`watcher.headers`增加日志输出，方便调试和问题定位
- 在保存PE数据时，添加对`dbnum_info_table`的统计并以`UPSERT`形式更新

### Changed

- 更新依赖项，调整配置文件，优化代码结构
- 优化了方法`update_elements_to_database`的调用格式，提升代码可读性
- 修改了`define_dbnum_event`函数，使用`UPSERT MERGE`替代`UPSERT SET`以更高效的方式更新`dbnum_info_table`统计数据
- 在`DbOption.toml`配置文件中，将`gen_spatial_tree`改为`false`，调整了数据库文件和编号配置
- 在`increment_manager.rs`中增强了调试日志，增加了文件路径及操作状态的调试输出
- 优化`run_app`启动逻辑，重新定义事件以提高性能
- 修改了`database.rs`中的变量类型转换逻辑，强制将`array::at()`的返回值转为`int`类型
- 重构增量更新逻辑，进一步细化增量解析的条件判断与参数设置

### Removed

- 删除了`bran/atta.rs`文件及其相关代码，移除了针对数据中心获取属性的具体实现逻辑
- 删除了`auto_get_attr.rs`文件及其相关代码，移除了自动提取数据中心属性的功能
- 移除了数据中心属性的单元测试和相关测试代码
- 移除冗余函数，优化项目组织结构

### Fixed

- 修正了多处注释中的格式问题，去除多余空格
- 修改了`query_latest_sesno_by_dbnum`调用的错误处理逻辑，提升错误提示的清晰度
- 更新了增量更新逻辑，在处理文件属性时增加更严格的条件检查，避免无效更新
- 优化`update_dbnum_event`的SQL逻辑，增强对`CREATE`、`UPDATE`、`DELETE`事件的精确处理
- 修复和规范化代码样式问题，并改进了部分异常处理和日志输出逻辑