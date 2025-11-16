# 配置文件使用说明

## 问题描述

之前代码中存在配置文件硬编码问题：
- `gen-model-fork` 项目支持通过 `--config` 参数指定配置文件
- 但 `rs-core` 库硬编码使用 `DbOption.toml` 文件名
- 导致创建了 `DbOption-ams.toml` 后，代码仍在使用默认的 `DbOption.toml`

## 解决方案

### 1. 修改 rs-core 库

在 `rs-core/src/lib.rs` 中：
- 添加了 `get_config_file_name()` 辅助函数，支持环境变量 `DB_OPTION_FILE`
- 修改了所有硬编码 `"DbOption"` 的地方，改为使用环境变量

### 2. 修改 gen-model-fork 主程序

在 `gen-model-fork/src/main.rs` 中：
- 在获取配置文件路径后，设置环境变量 `DB_OPTION_FILE`
- 确保 rs-core 库使用正确的配置文件

### 3. 修改 Web 服务器

在 `gen-model-fork/src/bin/web_server.rs` 中：
- 添加了 `--config` 命令行参数支持
- 修改启动函数支持配置文件参数

## 使用方法

### 1. 命令行工具

```bash
# 使用默认配置文件 DbOption.toml
./target/release/aios-database

# 使用指定配置文件 DbOption-ams.toml
./target/release/aios-database --config DbOption-ams

# 导出模型时使用指定配置
./target/release/aios-database --config DbOption-ams --export-xkt-refnos "21491_18946"
```

### 2. Web 服务器

```bash
# 使用默认配置文件启动 Web UI
./target/release/web_server

# 使用指定配置文件启动 Web UI
./target/release/web_server --config DbOption-ams
```

### 3. 环境变量方式

```bash
# 设置环境变量
export DB_OPTION_FILE=DbOption-ams

# 然后运行任何程序都会使用指定的配置文件
./target/release/aios-database
./target/release/web_server
```

## 配置文件对比

### DbOption.toml (默认配置)
- 项目: YCYK-E3D
- 数据库端口: 8009
- 项目代码: 1500
- 调试模型: ["21491_18946"]

### DbOption-ams.toml (AMS 配置)
- 项目: AvevaMarineSample
- 数据库端口: 8020
- 项目代码: 1516
- 调试模型: ["17496/201375"]
- 包含的数据库文件: ["ams1112_0001"]
- 手动数据库编号: [1112]

## 验证方法

1. **检查配置是否生效**：
   ```bash
   ./target/release/aios-database --config DbOption-ams --debug-model
   ```
   应该看到加载 AMS 项目配置的日志信息。

2. **检查 Web UI**：
   ```bash
   ./target/release/web_server --config DbOption-ams
   ```
   Web UI 应该显示使用 `DbOption-ams.toml` 配置文件。

3. **检查数据库连接**：
   程序应该连接到端口 8020 的 SurrealDB，而不是默认的 8009。

## 注意事项

1. **配置文件路径**: 参数值不需要包含 `.toml` 扩展名
2. **工作目录**: 配置文件应该在当前工作目录中
3. **环境变量优先级**: 如果设置了环境变量 `DB_OPTION_FILE`，会覆盖默认值
4. **向后兼容**: 不指定 `--config` 参数时，仍使用默认的 `DbOption.toml`
