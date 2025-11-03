# 模型导出命令使用指南

本文档说明如何使用 `aios-database` 工具导出不同格式的模型文件。

## 概述

`aios-database` 支持两种导出模式：
1. **调试模式**：使用 `--debug-model-refnos` 配合导出选项（`--export-obj`, `--export-glb`, `--export-gltf`, `--export-xkt`）
2. **非调试模式**：直接使用独立的导出参数（推荐用于生产环境）

## 支持格式

- **OBJ**: `.obj` 格式
- **GLB**: 二进制 glTF (`.glb`)
- **glTF**: JSON glTF (`.gltf`)
- **XKT**: 专有格式 (`.xkt`)

## 非调试模式（推荐）

这种方式不会启用调试模式，适合生产环境使用。

### 导出 OBJ 格式
```bash
cargo run --bin aios-database -- --export-obj-refnos="21491_18957"
```

### 导出 GLB 格式
```bash
cargo run --bin aios-database -- --export-glb-refnos="21491_18957"
```

### 导出 glTF 格式
```bash
cargo run --bin aios-database -- --export-gltf-refnos="21491_18957"
```

### 导出 XKT 格式
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957"
```

### 多个参考号
使用逗号分隔多个参考号：
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957,21491_18958"
```

## 调试模式

这种方式会同时启用调试模式，适合开发调试使用。

### 导出 OBJ
```bash
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-obj
```

### 导出 GLB
```bash
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-glb
```

### 导出 glTF
```bash
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-gltf
```

### 导出 XKT
```bash
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-xkt
```

## 通用参数

所有导出命令都支持以下通用参数：

### 指定输出路径
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --export-obj-output="path/to/output.xkt"
```

### 过滤特定类型
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --export-filter-nouns="EQUI,PIPE,VALV"
```

### 包含子孙节点
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --export-include-descendants=false
```

### 详细输出
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --verbose
```

## XKT 特定参数

导出 XKT 格式时，可以使用以下额外参数：

### 禁用压缩
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --xkt-compress=false
```

### 验证输出文件
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --xkt-validate
```

### 跳过 Mesh 生成
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --xkt-skip-mesh
```

### 指定数据库配置
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --xkt-db-config="DbOption.toml"
```

### 指定数据库编号
```bash
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957" --xkt-dbno=1112
```

## 配置选项

使用 `-c` 或 `--config` 参数指定配置文件：
```bash
cargo run --bin aios-database -- -c DbOption-ams --export-xkt-refnos="21491_18957"
```

## 完整示例

### 导出 XKT 并验证
```bash
cargo run --bin aios-database -- \
  --config=DbOption \
  --export-xkt-refnos="21491_18957,21491_18958" \
  --export-filter-nouns="EQUI,PIPE" \
  --export-include-descendants=true \
  --xkt-compress=true \
  --xkt-validate \
  --verbose
```

### 导出 GLB 到指定路径
```bash
cargo run --bin aios-database -- \
  --export-glb-refnos="21491_18957" \
  --export-obj-output="output/my_model.glb" \
  --export-include-descendants=true
```

## 输出位置

- 如果没有指定 `--export-obj-output`，文件将保存到 `output/` 目录
- 文件名基于 PE 的 name 属性自动生成
- 例如：`output/MyEquipment.xkt`

## 注意事项

1. **推荐使用非调试模式**：`--export-*-refnos` 参数不会启用调试功能，更适合生产环境
2. **参考号格式**：使用下划线连接，例如 `21491_18957`（内部会转换为 `21491/18957`）
3. **配置文件**：确保指定的配置文件存在于工作目录
4. **数据库连接**：确保数据库服务正在运行
