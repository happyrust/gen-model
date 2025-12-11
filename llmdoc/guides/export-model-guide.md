# 模型导出指南

## 支持的导出格式

| 格式 | 文件扩展名 | 用途 |
|------|-----------|------|
| GLB | `.glb` | 通用 3D 格式，单文件二进制 |
| GLTF | `.gltf` + `.bin` | 通用 3D 格式，分离资源 |
| OBJ | `.obj` | 传统 3D 格式，广泛兼容 |
| Instanced Bundle | `.bundle` | 实例化优化格式 |

## 命令行导出

### GLB 导出
```bash
cargo run --release -- export --format glb --output model.glb
```

### GLTF 导出
```bash
cargo run --release -- export --format gltf --output model.gltf
```

### OBJ 导出
```bash
cargo run --release -- export --format obj --output model.obj
```

### 实例化包导出
```bash
cargo run --release -- export --format instanced-bundle --output bundle/
```

## API 使用

### GLB 导出
```rust
use aios_database::fast_model::export_glb::GlbExporter;
use aios_database::fast_model::model_exporter::{ModelExporter, GlbExportConfig};

let exporter = GlbExporter::new();
let config = GlbExportConfig {
    unit_conversion: ("mm", "dm"),
    include_materials: true,
    ..Default::default()
};

exporter.export(&refnos, mesh_dir, "output.glb", config).await?;
```

### GLTF 导出
```rust
use aios_database::fast_model::export_gltf::export_gltf_for_refnos;

export_gltf_for_refnos(
    &refnos,
    "assets/meshes",
    "output.gltf",
    &db_option,
).await?;
```

### OBJ 导出
```rust
use aios_database::fast_model::export_model::export_obj::ObjExporter;

let exporter = ObjExporter::new();
exporter.export(&refnos, mesh_dir, "output.obj", config).await?;
```

### 实例化包导出
```rust
use aios_database::fast_model::export_instanced_bundle::export_instanced_bundle_for_refnos;

export_instanced_bundle_for_refnos(
    &refnos,
    "assets/meshes",
    "output_bundle/",
    &db_option,
).await?;
```

## 导出配置

### CommonExportConfig
```rust
pub struct CommonExportConfig {
    pub source_unit: String,    // 源单位 (默认 "mm")
    pub target_unit: String,    // 目标单位 (默认 "dm")
    pub include_hidden: bool,   // 包含隐藏对象
    pub simplify_mesh: bool,    // 简化网格
}
```

### 单位转换
```rust
// mm -> dm 转换（常用于 Web 显示）
let config = config.with_unit_conversion("mm", "dm");
```

### 名称配置
```rust
use aios_database::fast_model::export_model::NameConfig;

// 从 Excel 加载名称映射
let name_config = NameConfig::from_excel("names.xlsx")?;

// 导出时使用
exporter.export_with_names(&refnos, mesh_dir, output, config, &name_config).await?;
```

## 按区域导出

### 按数据库编号导出
```rust
// 导出特定 dbno 的所有模型
let dbnos = vec![1112, 1113];
for dbno in dbnos {
    let refnos = query_refnos_by_dbnum(dbno).await?;
    exporter.export(&refnos, mesh_dir, &format!("db_{}.glb", dbno), config).await?;
}
```

### 按 Zone 导出
```rust
// 导出特定 Zone 的模型
let zone_refnos = query_zone_children(zone_refno).await?;
exporter.export(&zone_refnos, mesh_dir, "zone.glb", config).await?;
```

## 实例化导出

### 概念
实例化导出将相同几何体的多个实例合并，只存储一份几何数据 + 多个变换矩阵，大幅减小文件体积。

### 使用场景
- Web 3D 查看器
- 大规模工厂模型
- 内存受限环境

### 配置
```rust
let config = InstancedBundleConfig {
    min_instance_count: 3,  // 至少 3 个实例才合并
    group_by_equipment: true,  // 按设备分组
    ..Default::default()
};
```

## 性能优化

### 分批导出
```rust
for (i, chunk) in refnos.chunks(1000).enumerate() {
    let output = format!("model_part_{}.glb", i);
    exporter.export(chunk, mesh_dir, &output, config).await?;
}
```

### 并行导出
```rust
use futures::future::join_all;

let futures: Vec<_> = dbnos.iter().map(|dbno| {
    export_by_dbno(*dbno, mesh_dir, config.clone())
}).collect();

join_all(futures).await;
```
