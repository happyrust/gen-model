# Fast Model 模块概述

## 简介
`fast_model` 是 aios-database 的核心模块，负责 3D 模型生成、布尔运算和网格处理。

## 模块位置
```
src/fast_model/
```

## 核心职责
1. **网格生成** - 从 PDMS 几何参数生成 3D 网格
2. **布尔运算** - CSG 布尔运算（交、并、差）
3. **LOD 支持** - 多级细节模型生成
4. **模型导出** - GLB/GLTF/OBJ 等格式导出
5. **空间索引** - AABB 缓存和 R*-tree 索引

## 子模块结构

### 核心生成模块 (`gen_model/`)
| 文件 | 职责 |
|------|------|
| `orchestrator.rs` | 主入口，流程编排 |
| `full_noun_mode.rs` | Full Noun 模式生成 |
| `non_full_noun.rs` | 增量/调试模式生成 |
| `mesh_processing.rs` | 网格后处理 |

### 处理器模块
| 文件 | 职责 |
|------|------|
| `cate_processor.rs` | 元件库 (CATE) 处理 |
| `prim_processor.rs` | 基本体 (PRIM) 处理 |
| `loop_processor.rs` | 循环体 (LOOP) 处理 |

### 导出模块 (`export_model/`)
| 文件 | 职责 |
|------|------|
| `export_glb.rs` | GLB 格式导出 |
| `export_gltf.rs` | GLTF 格式导出 |
| `export_obj.rs` | OBJ 格式导出 |
| `export_instanced_bundle.rs` | 实例化包导出 |

### 布尔运算
| 文件 | 职责 |
|------|------|
| `manifold_bool.rs` | Manifold 库集成 |
| `mesh_generate.rs` | CSG 网格生成 |

### 空间索引
| 文件 | 职责 |
|------|------|
| `aabb_cache.rs` | AABB 缓存 (SQLite) |
| `room_model_v2.rs` | 房间关系构建 |

## 关键数据流
```
PDMS 数据 → 几何参数解析 → 网格生成 → 布尔运算 → 导出
                ↓
          AABB 缓存更新
```

## 依赖关系
- `aios_core` - 核心类型和几何库
- `parry3d` - 碰撞检测和 AABB
- `Manifold` - 布尔运算引擎
