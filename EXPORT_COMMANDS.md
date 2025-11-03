# 导出命令使用说明

## 概述

`aios-database` 现在支持两种导出模式：

### 1. 非调试模式（推荐用于生产环境）

使用独立的导出参数，不启用调试功能：

```bash
# 导出 OBJ 格式
cargo run --bin aios-database -- --export-obj-refnos="21491_18957"

# 导出 GLB 格式
cargo run --bin aios-database -- --export-glb-refnos="21491_18957"

# 导出 glTF 格式
cargo run --bin aios-database -- --export-gltf-refnos="21491_18957"

# 导出 XKT 格式
cargo run --bin aios-database -- --export-xkt-refnos="21491_18957"
```

### 2. 调试模式（用于开发调试）

使用 `--debug-model-refnos` 配合导出选项，会同时启用调试功能：

```bash
# 导出 OBJ 格式（调试模式）
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-obj

# 导出 GLB 格式（调试模式）
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-glb

# 导出 glTF 格式（调试模式）
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-gltf

# 导出 XKT 格式（调试模式）
cargo run --bin aios-database -- --debug-model-refnos="21491_18957" --export-xkt
```

## 主要区别

- **`--export-*-refnos`**: 独立参数，不启用调试模式，适合生产环境使用
- **`--debug-model-refnos` + `--export-*`**: 同时启用调试模式，适合开发调试使用

## 支持的格式

- OBJ (`.obj`)
- GLB (`.glb`)
- glTF (`.gltf`)
- XKT (`.xkt`)

## 通用参数

所有导出命令都支持以下参数：

```bash
# 指定输出路径
--export-obj-output="path/to/output.xkt"

# 过滤特定类型
--export-filter-nouns="EQUI,PIPE,VALV"

# 是否包含子孙节点
--export-include-descendants=false

# 重新生成 mesh（强制 replace_mesh 模式）
--gen-mesh

# 详细输出
--verbose
```

## XKT 特定参数

```bash
# 禁用压缩
--xkt-compress=false

# 验证输出文件
--xkt-validate

# 跳过 Mesh 生成
--xkt-skip-mesh

# 指定数据库配置
--xkt-db-config="DbOption.toml"

# 指定数据库编号
--xkt-dbno=1112
```

## 完整示例

```bash
# 非调试模式 - 导出 XKT 并验证
cargo run --bin aios-database -- \
  --config=DbOption \
  --export-xkt-refnos="21491_18957" \
  --export-filter-nouns="EQUI,PIPE" \
  --export-include-descendants=true \
  --xkt-compress=true \
  --xkt-validate \
  --verbose

# 调试模式 - 导出 glTF
cargo run --bin aios-database -- \
  --debug-model-refnos="21491_18957" \
  --export-gltf \
  --export-obj-output="output/my_model.gltf"

# 重新生成 mesh 并导出 OBJ
cargo run --bin aios-database -- \
  --debug-model-refnos="21491_18957" \
  --gen-mesh \
  --export-obj
```

## 详细文档

更多详细信息请参阅: `docs/guides/export_commands.md`
