# 测试脚本使用说明

本目录包含用于模型生成测试的实用脚本。

## 脚本列表

### 0. `test_1112_complete.sh` - ⭐ 一键完整测试 (推荐)

自动化完整的 1112 测试流程,包括数据库启动、PDMS 解析、模型生成和验证。

#### 使用方法

```bash
# 一键运行完整测试
./scripts/test_1112_complete.sh
```

这个脚本会自动:
1. ✅ 检查配置文件
2. ✅ 启动测试数据库
3. ✅ 解析 PDMS 数据
4. ✅ 生成 3D 模型
5. ✅ 验证生成的数据
6. ✅ 自动清理资源

⏱️ 预计总耗时: 3-8 分钟 (首次编译会更长)

### 1. `start_test_db.sh` - 数据库启动脚本

独立启动 SurrealDB 测试数据库。

#### 使用方法

```bash
# 使用默认配置启动
./scripts/start_test_db.sh

# 自定义配置
DB_PORT=8001 DB_PATH=./data/my_test_db ./scripts/start_test_db.sh
```

#### 环境变量

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `SURREAL_PATH` | `surreal` | SurrealDB 可执行文件路径 |
| `DB_PATH` | `./data/test_db` | 数据库文件存储路径 |
| `DB_PORT` | `8000` | 数据库监听端口 |
| `DB_USER` | `root` | 数据库用户名 |
| `DB_PASS` | `root` | 数据库密码 |
| `DB_NS` | `test` | 数据库命名空间 |
| `DB_NAME` | `aios` | 数据库名称 |

#### 功能特性

- ✅ 自动检查端口占用
- ✅ 自动创建数据目录
- ✅ 健康检查等待
- ✅ 优雅的退出处理
- ✅ 彩色输出提示

### 2. `run_model_test.sh` - 完整测试流程

自动化测试流程,包括数据库启动、编译和测试运行。

#### 使用方法

```bash
# 运行默认测试 (test_1112_site_gen_model)
./scripts/run_model_test.sh

# 运行指定测试
TEST_EXAMPLE=verify_query_provider ./scripts/run_model_test.sh
```

#### 环境变量

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `DB_PORT` | `8000` | 数据库端口 |
| `TEST_EXAMPLE` | `test_1112_site_gen_model` | 要运行的测试示例名称 |

#### 测试流程

1. **检查数据库状态**
   - 如果数据库未运行,自动启动
   - 等待数据库就绪

2. **编译测试程序**
   - Release 模式编译
   - 仅编译指定的示例

3. **运行测试**
   - 执行模型生成测试
   - 输出测试结果

4. **清理资源**
   - 自动停止启动的数据库
   - 释放端口

## 快速开始

### 方式 1: 一键测试 (推荐)

```bash
# 运行完整测试流程
./scripts/run_model_test.sh
```

这个命令会:
1. 自动检查并启动数据库
2. 编译测试程序
3. 运行测试
4. 测试完成后自动清理

### 方式 2: 手动控制

```bash
# 1. 启动数据库 (在一个终端)
./scripts/start_test_db.sh

# 2. 运行测试 (在另一个终端)
cargo run --release --example test_1112_site_gen_model

# 3. 停止数据库 (Ctrl+C 第一个终端)
```

## 完整测试工作流程

### 1112 数据库完整测试流程

**重要**: 模型生成前需要先解析 PDMS 数据!

#### 步骤 1: 配置 DbOption.toml

确保以下配置正确:

```toml
# 同步配置 - 设置为 true 以解析 PDMS 数据
total_sync = true      # 或 incr_sync = true

# 数据库文件
included_db_files = ["ams1112_0001"]
manual_db_nums = [1112]

# 项目路径 - 包含 PDMS 数据库文件
project_path = "/path/to/e3d_models"

# SurrealDB 连接
v_ip = "127.0.0.1"
v_port = 8000
v_user = "root"
v_password = "root"
surreal_ns = "test"
project_name = "aios"
```

#### 步骤 2: 启动数据库

```bash
./scripts/start_test_db.sh
```

#### 步骤 3: 解析 PDMS 数据

```bash
# 方式 1: 使用主程序 (推荐)
cargo run --release

# 方式 2: 使用完整测试示例
cargo run --release --example test_1112_full_workflow
```

这一步会:
- 读取 `ams1112_0001` PDMS 数据库文件
- 解析元素、属性和层级关系
- 存储到 SurrealDB

⏱️ 预计耗时: 2-5 分钟 (取决于数据量)

#### 步骤 4: 生成模型 (可选)

如果配置了 `gen_model = true`,解析完成后会自动生成模型。

或者单独运行模型生成:

```bash
# 需要先禁用解析,只生成模型
# DbOption.toml: total_sync = false, incr_sync = false
cargo run --release
```

#### 步骤 5: 验证数据

```bash
# 查询 SITE 节点
cargo run --release --example test_1112_site_gen_model

# 运行查询测试
cargo run --release --example verify_query_provider
```

## 可用的测试示例

| 示例名称 | 说明 | 前置条件 |
|---------|------|----------|
| `test_1112_full_workflow` | ⭐ 完整流程: 解析 + 生成模型 | PDMS 文件存在 |
| `test_1112_site_gen_model` | SITE 模型查询测试 | 已解析 PDMS 数据 |
| `verify_query_provider` | 查询提供者验证测试 | 已解析 PDMS 数据 |
| `gen_model_query_benchmark` | 模型生成查询性能测试 | 已解析 PDMS 数据 |
| `db1112_dual_storage_test` | 双存储对比测试 | 已解析 PDMS 数据 |

## 故障排除

### 问题: 端口被占用

```bash
# 检查占用端口的进程
lsof -i :8000

# 手动清理
kill -9 <PID>
```

### 问题: SurrealDB 未安装

```bash
# macOS / Linux
curl -sSf https://install.surrealdb.com | sh

# 或使用 Homebrew (macOS)
brew install surrealdb/tap/surreal
```

### 问题: 数据库连接超时

1. 检查防火墙设置
2. 确认 SurrealDB 版本兼容性
3. 查看数据库日志

## 配置示例

### 使用自定义数据库路径

```bash
# 设置环境变量
export DB_PATH=/path/to/custom/db
export DB_PORT=8001

# 运行测试
./scripts/run_model_test.sh
```

### 持久化配置

创建 `.env` 文件:

```bash
# .env
export DB_PATH=./data/production_test_db
export DB_PORT=8000
export DB_USER=test_user
export DB_PASS=test_password
```

使用:

```bash
source .env
./scripts/run_model_test.sh
```

## 注意事项

1. **数据库文件**: 测试数据库文件存储在 `./data/test_db` 目录
2. **端口占用**: 默认使用 8000 端口,如有冲突请修改 `DB_PORT`
3. **自动清理**: `run_model_test.sh` 会在测试完成后自动停止数据库
4. **手动启动**: 如果使用 `start_test_db.sh`,需要手动按 Ctrl+C 停止

## 许可证

本项目遵循项目主许可证。
