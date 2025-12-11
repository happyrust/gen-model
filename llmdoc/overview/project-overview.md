# 项目概述

## 项目名称
aios-database

## 简介
aios-database 是一个基于 Rust 的 PDMS (Plant Design Management System) 数据处理和 3D 模型生成系统。用于从工厂设计数据库读取数据，生成 3D 网格模型，执行布尔运算，并导出为多种格式。

## 核心功能
- **PDMS 数据同步**：从 PDMS 数据库解析和同步工厂设计数据
- **3D 模型生成**：基于 PDMS 数据生成高质量 3D 网格模型
- **布尔运算**：使用 Manifold 库执行 CSG 布尔运算
- **多格式导出**：支持 GLB、GLTF、OBJ、XKT 等格式
- **空间索引**：R*-tree 空间查询和房间关系计算
- **版本管理**：支持数据增量更新和版本控制

## 技术栈
- **语言**：Rust (Edition 2024)
- **数据库**：SurrealDB (主), SQLite (空间索引缓存)
- **几何库**：parry3d, nalgebra, glam
- **布尔运算**：Manifold (通过 aios_core)
- **序列化**：serde, rkyv, bincode

## 主要模块
| 模块 | 路径 | 职责 |
|------|------|------|
| fast_model | `src/fast_model/` | 3D 模型生成核心 |
| data_interface | `src/data_interface/` | 数据访问抽象层 |
| versioned_db | `src/versioned_db/` | 版本化数据管理 |
| pcf | `src/pcf/` | 管道组件族处理 |
| spatial_index | `src/spatial_index.rs` | 空间索引 |

## 构建与运行
```bash
# 默认构建
cargo build

# 发布构建
cargo build --release

# CentOS 7 交叉编译
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17
```

## 配置文件
- `DbOption.toml` - 主配置文件
- `DbOption-*.toml` - 项目特定配置
