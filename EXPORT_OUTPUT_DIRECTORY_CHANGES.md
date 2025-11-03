# 导出模型输出目录更改说明

## 更改概述

将所有导出模型操作的默认输出目录统一设置为 `output` 目录。

## 修改的文件

### 1. `src/cli_modes.rs`

为所有导出模式（OBJ、GLB、GLTF、XKT）添加了默认输出路径到 `output` 目录：

- **OBJ 导出**：从 `filename.obj` 改为 `output/filename.obj`
- **GLB 导出**：从 `filename.glb` 改为 `output/filename.glb`
- **GLTF 导出**：从 `filename.gltf` 改为 `output/filename.gltf`
- **XKT 导出**：从 `filename.xkt` 改为 `output/filename.xkt`

### 2. `src/fast_model/export_obj.rs`

添加了自动创建输出目录的逻辑：
```rust
// 创建输出目录（如果不存在）
if let Some(parent) = Path::new(output_path).parent() {
    std::fs::create_dir_all(parent).context("创建输出目录失败")?;
}
```

### 3. `src/fast_model/export_glb.rs`

添加了自动创建输出目录的逻辑。

### 4. `src/fast_model/export_gltf.rs`

添加了自动创建输出目录的逻辑。

### 5. `src/fast_model/export_xkt.rs`

已经有创建输出目录的逻辑，无需修改。

## 行为变化

### 之前的行为
- 未指定输出路径时，文件直接保存在当前工作目录
- 文件名为 `<名称>.obj`、`<名称>.glb` 等

### 现在的行为
- 未指定输出路径时，文件保存在 `output` 目录
- 文件路径为 `output/<名称>.obj`、`output/<名称>.glb` 等
- `output` 目录会自动创建（如果不存在）

## 示例

### 之前的命令
```bash
cargo run --bin aios-database -- --debug-model-refnos 21491 --export-xkt
# 输出文件: 模型名称.xkt
```

### 现在的命令
```bash
cargo run --bin aios-database -- --debug-model-refnos 21491 --export-xkt
# 输出文件: output/模型名称.xkt
```

## 向后兼容性

- 如果用户手动指定了输出路径（使用 `--export-obj-output` 参数），将使用指定的路径，不会被修改
- 只有在未指定输出路径时，才会使用 `output` 目录作为默认输出位置

## 注意事项

1. 确保 `output` 目录有写权限
2. 如果 `output` 目录已存在文件，可能会被覆盖
3. 建议定期清理 `output` 目录中的旧文件
