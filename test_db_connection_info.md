# 数据库连接信息打印功能测试

## 测试目的
验证数据库连接初始化时是否正确打印服务器信息和配置文件信息。

## 测试方法

### 1. 测试 rs-core 的 init_surreal 函数
```bash
# 在 rs-core 目录下
cargo run --example test_unified_query
```

预期输出应包含：
```
🔧 正在初始化数据库连接...
📄 使用配置文件: DbOption.toml
🌐 连接服务器: ws://localhost:8009
🏷️  命名空间: your_namespace
💾 数据库名: your_database
👤 用户名: root
✅ 数据库连接成功！
```

### 2. 测试 gen-model-fork 的 web_server 模块
```bash
# 在 gen-model-fork 目录下
cargo run --bin web_server --features web_server
```

预期输出应包含：
```
🔧 正在初始化数据库连接...
📄 使用配置文件: DbOption.toml
🌐 连接服务器: ws://localhost:8009
🏷️  命名空间: your_namespace  
💾 数据库名: your_database
👤 用户名: root
✅ 数据库连接成功！
```

### 3. 测试 init_surreal_with_retry 函数
```bash
# 使用任何调用 init_surreal_with_retry 的示例
cargo run --example debug_query_multi_filter
```

预期输出应包含配置信息和重试信息。

## 验证要点

1. ✅ 配置文件名正确显示
2. ✅ 服务器地址和端口正确显示
3. ✅ 命名空间信息正确显示
4. ✅ 数据库名正确显示
5. ✅ 用户名正确显示（不显示密码）
6. ✅ 连接成功后显示确认信息
7. ✅ 错误处理不受影响

## 环境变量测试

测试使用不同配置文件：
```bash
export DB_OPTION_FILE=DbOption-ams
cargo run --example test_unified_query
```

应该显示：
```
📄 使用配置文件: DbOption-ams.toml
```
