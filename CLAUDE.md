# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

aios-database 是一个基于 Rust 的 PDMS (Plant Design Management System) 数据处理和 3D 模型生成系统。主要功能包括从 PDMS 数据库读取工厂设计数据，生成 3D 网格模型，执行布尔运算，并导出为各种格式（GLB、GLTF、XKT 等）。

## 构建和运行

### 基本命令

```bash
# 构建项目（默认特性：ws, gen_model, manifold, project_hd, sqlite-index, surreal-save）
cargo build

# 发布构建
cargo build --release

# 运行测试
cargo test

# 运行特定测试
cargo test <test_name>

# 运行单个二进制
cargo run --bin <binary_name>

# 运行示例
cargo run --example <example_name>
```

### CentOS 7 交叉编译

```bash
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17
```

### 重要的二进制程序

- `web_server` - Web 服务器（需要 web_server 特性）
- `db_option_ui` - 数据库配置界面（需要 web_server 特性）
- `test_dblist` - 数据库列表测试工具

### 常用示例

```bash
# 查询调试示例
cargo run --example debug_query_multi_filter

# LOD 特性验证
cargo run --example verify_lod_feature

# 特定引用号调试
cargo run --example debug_refno_21491_19209

# 房间计算演示（需要 gen_model 和 sqlite-index 特性）
cargo run --example room_calculation_demo --features "gen_model,sqlite-index"
```

## 核心架构

### 模块结构

- **src/fast_model/** - 快速模型生成核心
  - `gen_model/` - 模型生成主模块，包含完整和非完整名词模式、网格处理、编排器等
  - `mesh_generate.rs` - 网格生成和布尔运算
  - `manifold_bool.rs` - Manifold 库布尔运算集成
  - `room_model_v2.rs` - 改进版房间模型构建（需要 sqlite-index）
  - `export_model/` - 模型导出功能（GLB、GLTF、实例化包等）
  - `query.rs` - 数据库查询逻辑
  - `aabb_cache.rs` - AABB 缓存（需要 sqlite-index）

- **src/pcf/** - PDMS 组件族（Piping Component Family）
  - 包含各种管道组件：BRAN、TUBI、ELBO、TEE、VALV、FLAN 等
  - `pcf_api.rs` - PCF API 接口
  - `excel_api.rs` - Excel 导出 API

- **src/versioned_db/** - 版本化数据库管理
  - 支持数据增量更新和版本控制

- **src/data_interface/** - 数据接口层
  - 与 SurrealDB 和其他数据源的交互

- **src/spatial_index/** - 空间索引功能
  - 支持空间查询和优化

### 数据库架构

项目使用 SurrealDB 作为主要数据库，支持：
- PDMS 数据的结构化存储
- 实例关系（inst_relate）管理
- 几何数据缓存
- 版本化数据管理

可选的 SQLite 支持（sqlite-index 特性）：
- AABB 空间索引缓存
- 房间关系快速查询

### 特性标志（Features）

- `gen_model` - 启用 3D 模型生成（默认开启）
- `manifold` - 启用 Manifold 布尔运算库（默认开启）
- `web_server` - 启用 Web UI 和 API 服务器
- `sqlite-index` - 启用 SQLite 空间索引缓存（默认开启）
- `surreal-save` - 启用 SurrealDB 保存功能（默认开启）
- `project_hd` / `project_hh` - 项目特定配置
- `mqtt` - 启用 MQTT 服务
- `profile` - 启用性能分析和 tracing
- `debug_obj_export` - 调试用 OBJ 导出
- `debug_expr` - 调试表达式解析

### 关键数据流

1. **PDMS 数据导入** → 解析数据库文件 → 存储到 SurrealDB
2. **模型生成** → 查询组件数据 → 生成网格 → 执行布尔运算 → 缓存结果
3. **导出** → 批量查询 inst_relate → 聚合几何数据 → 生成 GLB/GLTF/实例化包
4. **房间构建** → 查询空间关系 → 构建拓扑 → 更新 SQLite 索引

## 配置文件

### DbOption.toml

主配置文件，定义：
- 数据库连接信息
- 项目路径和输出目录
- 模型生成参数
- 导出选项

示例配置文件：
- `DbOption.toml` - 主配置
- `DbOption-backup.toml` - 备份配置
- `DbOption-zsy.toml` - 项目特定配置

## 开发注意事项

### 模型生成重构

`gen_model_old.rs` 已迁移到模块化结构：
- 完整名词模式 → `gen_model/full_noun_mode.rs`
- 非完整名词模式 → `gen_model/non_full_noun.rs`
- 网格处理 → `gen_model/mesh_processing.rs`
- 编排器 → `gen_model/orchestrator.rs`

### 调试技巧

项目提供多个调试宏：
- `debug_model!()` - 基础调试输出
- `smart_debug_model!()` - 智能调试，可配置仅输出错误
- `smart_debug_error!()` - 强制输出错误信息

启用调试：设置环境变量或使用 `debug_expr` 特性

### 性能优化

- 使用 DashMap 和 DashSet 进行并发访问
- AABB 缓存避免重复计算
- 批量处理和流式写入大数据集
- 几何哈希去重避免重复存储

### 测试

测试位于：
- `src/test/` - 单元测试和集成测试
- `src/examples/` - 可执行示例
- `examples/` - 独立示例（如 room_calculation_demo）

运行特定测试集：
```bash
# 布尔运算测试
cargo run --bin test_boolean_batch

# 房间查询测试
./test_room_query.sh
```


## 文档位置

虽然项目包含 `llmdoc/` 目录结构，但当前为空。主要文档位于：
- `docs/` - 技术文档
- `开发文档/` - 中文开发文档
- 根目录 Markdown 文件 - 特性和问题追踪文档
