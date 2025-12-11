# Fast Model 架构设计

## 整体架构

```
┌─────────────────────────────────────────────────────────┐
│                    orchestrator.rs                       │
│              (主入口：gen_all_geos_data)                 │
└───────────────────────┬─────────────────────────────────┘
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
┌───────────────┐ ┌───────────────┐ ┌───────────────┐
│ Full Noun     │ │ Non Full Noun │ │ Incremental   │
│ Mode          │ │ Mode          │ │ Update        │
└───────┬───────┘ └───────┬───────┘ └───────┬───────┘
        │                 │                 │
        └─────────────────┼─────────────────┘
                          ▼
        ┌─────────────────────────────────┐
        │         Processors              │
        │  ┌─────────┬─────────┬────────┐ │
        │  │  CATE   │  PRIM   │  LOOP  │ │
        │  └─────────┴─────────┴────────┘ │
        └─────────────────┬───────────────┘
                          ▼
        ┌─────────────────────────────────┐
        │      mesh_generate.rs           │
        │   (CSG 网格生成 + 布尔运算)     │
        └─────────────────┬───────────────┘
                          ▼
        ┌─────────────────────────────────┐
        │      export_model/              │
        │   (GLB/GLTF/OBJ 导出)           │
        └─────────────────────────────────┘
```

## 核心组件

### 1. Orchestrator (编排器)
**文件**: `gen_model/orchestrator.rs`

主入口函数，负责：
- 解析配置和参数
- 选择执行模式（Full Noun / Non Full Noun）
- 协调各处理器执行
- 汇总处理结果

```rust
pub async fn gen_all_geos_data(
    refnos: Vec<RefnoEnum>,
    db_option_ext: &DbOptionExt,
    batch_size: Option<usize>,
    progress_callback: Option<Box<dyn Fn(f32) + Send + Sync>>,
) -> anyhow::Result<()>
```

### 2. Full Noun Mode
**文件**: `gen_model/full_noun_mode.rs`

全量生成模式，用于：
- 首次完整模型生成
- 全量数据重建

特点：
- 按 Noun 类型分类处理
- 并发批量处理
- 自动 SJUS 映射验证

### 3. Non Full Noun Mode
**文件**: `gen_model/non_full_noun.rs`

增量/调试模式，用于：
- 指定 refno 生成
- 增量更新
- 调试单个组件

### 4. Processors (处理器)
三种几何体处理器：

| 处理器 | 处理对象 | 特点 |
|--------|----------|------|
| CATE | 元件库实例 | 支持负体布尔运算 |
| PRIM | 基本体 | Box/Cylinder/Cone 等 |
| LOOP | 循环体 | Extrusion/Revolution |

## 数据结构

### NounCategory
```rust
pub enum NounCategory {
    Cate,  // 元件库类型
    Prim,  // 基本体类型
    Loop,  // 循环体类型
}
```

### DbModelInstRefnos
```rust
pub struct DbModelInstRefnos {
    pub cate_refnos: Vec<RefnoEnum>,
    pub prim_refnos: Vec<RefnoEnum>,
    pub loop_refnos: Vec<RefnoEnum>,
}
```

### CategorizedRefnos
```rust
pub struct CategorizedRefnos {
    pub cate: DashMap<u64, Vec<RefnoEnum>>,
    pub prim: DashMap<u64, Vec<RefnoEnum>>,
    pub loop_: DashMap<u64, Vec<RefnoEnum>>,
}
```

## 配置管理

### FullNounConfig
```rust
pub struct FullNounConfig {
    pub batch_size: BatchSize,
    pub concurrency: Concurrency,
    pub enable_lod: bool,
    pub mesh_precision: MeshPrecisionSettings,
}
```

## 并发模型
- 使用 `tokio` 异步运行时
- `DashMap` / `DashSet` 并发集合
- 批量处理减少数据库往返
- `rayon` 并行迭代器加速计算
