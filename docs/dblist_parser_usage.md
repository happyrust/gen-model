# dblist 解析和模型生成测试工具

## 概述

这个工具提供了从 PDMS dblist 文件解析数据，加载到 SurrealDB 内存数据库，并执行模型生成的完整流程。

## 功能特性

- **dblist 文件解析**：支持 PDMS dblist 文本格式，提取元素类型、属性和层级关系
- **内存数据库加载**：将解析后的数据加载到 SurrealDB 内存数据库中
- **模型生成测试**：可选地执行模型生成流程，验证数据完整性
- **详细统计报告**：提供解析和加载的详细统计信息

## 使用方法

### 基本用法（仅解析和验证）

```bash
cargo run --bin test_dblist --features test -- <dblist_file_path>
```

### 完整用法（包含模型生成）

```bash
cargo run --bin test_dblist --features test -- <dblist_file_path> --generate
```

### 参数说明

- `<dblist_file_path>`：必需参数，指定要解析的 dblist 文件路径
- `--generate` 或 `-g`：可选参数，解析完成后执行模型生成

## 示例

```bash
# 解析并验证数据
cargo run --bin test_dblist --features test -- test_data/dblist/FRMW_17496_266203.txt

# 解析、验证并生成模型
cargo run --bin test_dblist --features test -- test_data/dblist/FRMW_17496_266203.txt --generate
```

## 输出说明

### 解析阶段

- 📚 解析完成，显示找到的元素数量
- 📊 加载统计信息，按类型统计元素数量

### 数据库阶段

- 🧠 初始化内存数据库
- 🧹 清理现有数据
- 📦 逐个加载元素到数据库

### 验证阶段

- 🔍 获取 RefnoEnum 数量
- 📊 数据库验证，显示总记录数和分类统计

### 模型生成阶段（如果启用）

- 🏗️ 开始生成模型
- 🔄 处理各个模型节点
- ✅ 模型生成完成

## 支持的元素类型

当前支持以下 PDMS 元素类型：

- FRMWORK（框架）
- PANEL（面板）
- GENSEC（截面）
- SPINE（脊线）
- POINSP（点支撑）
- PAVERT（顶点）
- 以及其他常见类型

## 技术架构

### 核心模块

1. **解析器** (`src/dblist_parser/parser.rs`)
   - `DblistParser`：主解析器
   - `PdmsElement`：PDMS 元素数据结构
   - `ElementType`：元素类型枚举

2. **数据加载器** (`src/dblist_parser/db_loader.rs`)
   - `DblistLoader`：数据库加载器
   - 递归加载元素和子元素
   - 提供 RefnoEnum 转换功能

3. **测试程序** (`src/bin/test_dblist.rs`)
   - 命令行参数处理
   - 完整的测试流程编排
   - 详细的进度和统计报告

### 数据流程

```
dblist 文件 → 解析器 → PdmsElement 结构 → 加载器 → SurrealDB 内存数据库 → 模型生成
```

## 错误处理

工具包含完善的错误处理机制：

- 文件读取错误
- 解析格式错误
- 数据库连接错误
- 模型生成错误

所有错误都会显示详细的错误信息和上下文。

## 开发说明

### 编译要求

```bash
cargo build --bin test_dblist --features test
```

### 依赖项

- `aios_core`：核心数据库和类型定义
- `surrealdb`：内存数据库
- `serde_json`：JSON 序列化
- `anyhow`：错误处理
- `clap`：命令行参数解析

### 扩展开发

如需支持新的元素类型：

1. 在 `ElementType` 枚举中添加新类型
2. 在 `from_str` 方法中添加解析逻辑
3. 在 `to_noun` 方法中添加名称映射

## 注意事项

1. **内存限制**：使用内存数据库，大量数据可能消耗较多内存
2. **数据格式**：确保 dblist 文件符合预期的格式规范
3. **权限要求**：需要读取指定文件的权限

## 故障排除

### 常见问题

1. **编译错误**：检查是否启用了 `test` feature
2. **文件未找到**：确认文件路径正确
3. **解析失败**：检查 dblist 文件格式是否正确
4. **数据库错误**：确保 SurrealDB 函数定义文件存在

### 调试建议

- 使用小文件进行初始测试
- 检查输出中的统计信息
- 关注错误信息和警告

## 更新日志

- v0.1.0：初始版本，支持基本解析和加载功能
- v0.2.0：添加模型生成测试功能
- v0.3.0：优化错误处理和输出格式
