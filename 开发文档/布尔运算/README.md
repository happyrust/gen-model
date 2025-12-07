# 布尔运算系统文档

## 概述

本目录包含 gen-model-fork 项目布尔运算系统的完整文档。

## 文档索引

| 文件 | 说明 |
|------|------|
| [01_架构概述.md](./01_架构概述.md) | 系统架构、模块关系 |
| [02_数据模型.md](./02_数据模型.md) | 数据结构、数据库表 |
| [03_流程图.md](./03_流程图.md) | Mermaid 流程图 |
| [04_代码实现.md](./04_代码实现.md) | 核心代码解析 |
| [05_问题排查.md](./05_问题排查.md) | 调试与排查指南 |

## 快速入口

- **布尔运算入口**: `src/fast_model/manifold_bool.rs`
- **数据库查询**: `aios_core` 中的 `query_*` 函数
- **测试程序**: `src/bin/test_boolean_debug.rs`

## 两种布尔类型

1. **元件库布尔** (`apply_cata_neg_boolean_manifold`)
   - 处理 `has_cata_neg=true` 的实例
   - 结果: 更新 `bool_status='Success'`, `booled=true`

2. **实例级布尔** (`apply_insts_boolean_manifold`)
   - 处理有 `neg_relate`/`ngmr_relate` 的实例
   - 结果: 更新 `bool_status='Success'`, `booled_id='{mesh_id}'`, `aabb=aabb:⟨hash⟩`

## 重要更新 (2024-12)

### neg_relate / ngmr_relate 新结构

```sql
-- 旧结构：in = 负载体 PE
neg_relate { in: pe:xxx, out: pe:target }

-- 新结构：in = 切割几何 geo_relate
neg_relate { 
    in: geo_relate:⟨id⟩,   -- 切割几何
    out: pe:⟨target⟩,      -- 被切正实体
    pe: pe:⟨carrier⟩       -- 负载体
}
```

**优点**：

- 查询直接 `SELECT in.* FROM pe:target<-neg_relate` 获取切割几何
- 无需遍历 carrier -> inst_relate -> geo_relate
- ID 简化为 `[geo_relate_id, target_pe]`

详见 [02_数据模型.md](./02_数据模型.md#3-关系说明)

## 相关文档

- [BOOLEAN_OPERATION_ANALYSIS.md](../../BOOLEAN_OPERATION_ANALYSIS.md)
- [docs/BOOLEAN_OPERATION_FLOWCHART.md](../../docs/BOOLEAN_OPERATION_FLOWCHART.md)
- [BOOLEAN_WORKER_ARCHITECTURE.md](../BOOLEAN_WORKER_ARCHITECTURE.md)
