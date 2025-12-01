# 导出功能文档索引

本目录包含 gen-model 导出功能的详细文档说明。

---

## 📚 文档列表

### 1. [导出流程总览](./导出流程总览.md)
**推荐首先阅读** ⭐

- 📊 完整的流程图（Mermaid 格式）
- 🎯 核心功能概览
- 🔑 关键步骤说明
- 📝 输出格式示例
- 适合快速了解整体流程

### 2. [build_instances_payload导出流程](./build_instances_payload导出流程.md)
**详细实现文档**

- 📖 每个步骤的详细说明
- 💻 完整的代码示例
- 🔍 数据结构详解
- ⚡ 性能优化策略
- 🐛 错误处理方案
- 适合深入学习实现细节

---

## 🔄 导出流程概述

```
ExportData (输入)
    ↓
收集所有者信息 (BRAN/EQUI/TUBI)
    ↓
分组处理 (按所有者分组)
    ↓
构建层级结构 (BRAN组/EQUI组/未分组)
    ↓
生成实例数据 (矩阵/颜色/LOD)
    ↓
JSON 序列化
    ↓
instances.json (输出)
```

---

## 🎯 核心函数

### build_instances_payload

**位置**: `src/fast_model/export_model/export_prepack_lod.rs`  
**行号**: 约 656-1000

**功能**:
- 将 ExportData 转换为 instances.json 格式
- 按 BRAN/EQUI 分组构件
- 生成实例数据（矩阵、颜色、LOD掩码等）
- 管理名称和颜色调色板

**输入**:
- `export_data`: 导出数据（构件、TUBI、几何体）
- `geo_index_map`: 几何体索引映射
- `lod_assets`: LOD 资产信息
- `unit_converter`: 单位转换器
- `material_library`: 材质库
- `refno_name_map`: 构件名称映射

**输出**:
- `instances_json`: JSON 格式的实例数据
- `component_instance_count`: 构件实例总数

---

## 📊 数据流转

```
ExportData
    ├─ components (构件列表)
    ├─ tubings (管道列表)
    └─ unique_geometries (几何体)
        ↓
    所有者集合
    ├─ bran_owners (BRAN/HANG)
    └─ equi_owners (EQUI)
        ↓
    分组映射
    ├─ bran_children_map
    ├─ bran_tubi_map
    └─ equi_children_map
        ↓
    层级结构
    ├─ bran_groups
    ├─ equi_groups
    └─ ungrouped
        ↓
    instances.json
```

---

## 🔗 相关文件

### 实现文件
- `src/fast_model/export_model/export_prepack_lod.rs` - 主导出逻辑
- `src/fast_model/export_model/export_common.rs` - 数据结构定义
- `src/fast_model/export_model/export_glb.rs` - GLB 导出

### 前端加载器
- `examples/aios-prepack-loader.ts` - TypeScript 加载器
- `examples/aios-prepack-loader.html` - 示例页面

### 配置文件
- `ColorSchemes.toml` - 颜色配置

---

## 📝 输出格式

### instances.json 结构

```json
{
  "version": 2,
  "generated_at": "2024-11-27T16:00:00Z",
  "colors": [
    [r, g, b, a],
    ...
  ],
  "names": [
    {"kind": "bran", "value": "BRAN-001"},
    {"kind": "component", "value": "NOZZLE-001"},
    ...
  ],
  "bran_groups": [
    {
      "refno": "...",
      "noun": "BRAN",
      "name": "...",
      "children": [...],
      "tubings": [...]
    }
  ],
  "equi_groups": [
    {
      "refno": "...",
      "noun": "EQUI",
      "name": "...",
      "children": [...]
    }
  ],
  "ungrouped": [...]
}
```

---

## 🚀 快速开始

1. **了解整体流程**: 阅读 [导出流程总览](./导出流程总览.md)
2. **查看流程图**: 在文档中查看 Mermaid 流程图
3. **深入细节**: 阅读 [详细实现文档](./build_instances_payload导出流程.md)
4. **查看代码**: 打开 `src/fast_model/export_model/export_prepack_lod.rs`

---

**文档版本**: 1.0  
**最后更新**: 2024-11-27  
**维护者**: Development Team

