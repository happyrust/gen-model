# aios-database 文档索引

> PDMS 工厂设计数据处理与 3D 模型生成系统

## 快速导航

### 📖 概述 (Overview)
- [项目概述](overview/project-overview.md) - 项目简介、技术栈、模块结构
- [Fast Model 概述](overview/fast-model-overview.md) - 3D 模型生成核心模块

### 🏗️ 架构 (Architecture)
- [Fast Model 架构设计](architecture/fast-model-architecture.md) - 模块架构、数据结构、并发模型
- [网格生成与布尔运算流程](architecture/mesh-generation-flow.md) - 从几何参数到网格文件的完整流程

### 📚 指南 (Guides)
- [模型生成使用指南](guides/model-generation-guide.md) - 配置、API 使用、调试技巧
- [模型导出指南](guides/export-model-guide.md) - GLB/GLTF/OBJ 等格式导出

### 📋 参考 (Reference)
- [编码约定](reference/coding-conventions.md) - Rust 代码规范、模块组织
- [Git 约定](reference/git-conventions.md) - 提交信息格式、分支策略

---

## 核心模块速查

| 模块 | 路径 | 文档 |
|------|------|------|
| **fast_model** | `src/fast_model/` | [概述](overview/fast-model-overview.md) / [架构](architecture/fast-model-architecture.md) |
| data_interface | `src/data_interface/` | - |
| versioned_db | `src/versioned_db/` | - |
| pcf | `src/pcf/` | - |

## 常用命令

```bash
# 构建
cargo build --release

# 运行模型生成
cargo run --release

# Full Noun 模式
FULL_NOUN_MODE=true cargo run --release

# 运行测试
cargo test
```

## 配置文件
- `DbOption.toml` - 主配置
- `ColorSchemes.toml` - 颜色方案

---

*文档更新时间: 2025-12-09*
