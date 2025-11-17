# 房间集成测试使用说明

## 概述

`test_room_integration.rs` 提供了完整的房间查询、模型生成和房间计算集成测试案例。

## 前置条件

### 1. 数据库准备
- 确保 SurrealDB 正在运行
- 数据库中包含测试数据（房间、面板等）
- 配置文件 `DbOption.toml` 正确配置

### 2. Feature Flags
必须启用以下特性：
- `sqlite` - SQLite 支持
- `sqlite-index` - SQLite 空间索引支持

### 3. 配置检查
检查 `DbOption.toml` 中的关键配置：
```toml
# 房间关键词（根据项目调整）
room_keyword = "-R-"

# 模型生成配置
gen_model = true
gen_mesh = true
apply_boolean_operation = true

# 房间计算开关
gen_spatial_tree = true

# Mesh 路径
meshes_path = "/path/to/meshes"
```

## 测试案例说明

### 1. `test_room_integration_complete`
**完整集成测试** - 端到端流程

**流程：**
1. 初始化数据库连接
2. 查询房间信息（基于关键词）
3. 触发模型生成（所有房间面板）
4. 执行房间计算（构建空间关系）
5. 验证结果

**运行：**
```bash
cargo test --test test_room_integration --features sqlite,sqlite-index test_room_integration_complete -- --ignored --nocapture
```

**输出示例：**
```
🏗️  房间集成测试开始
================================================================================

📡 步骤 1: 初始化数据库连接
--------------------------------------------------------------------------------
✅ 数据库连接成功
   项目名称: AvevaMarineSample
   项目代码: 1516
   Mesh 路径: /path/to/meshes

🔍 步骤 2: 查询房间信息
--------------------------------------------------------------------------------
🏷️  房间关键词: ["-R-"]
✅ 房间查询完成
   查询耗时: 250ms
   房间数量: 15
   总面板数: 45
   平均每房间面板数: 3.00

⚙️  步骤 3: 触发模型生成
--------------------------------------------------------------------------------
✅ 模型生成完成
   生成耗时: 8.5s
   处理元素数: 45

🏠 步骤 4: 执行房间计算
--------------------------------------------------------------------------------
✅ 房间计算完成
   计算耗时: 2.3s
   处理房间数: 15
   处理面板数: 45
   处理构件数: 1250

✅ 步骤 5: 验证结果
--------------------------------------------------------------------------------
📊 数据库验证:
   room_relate 关系数: 1250

🎉 测试完成
   总耗时: 11.2s
```

### 2. `test_query_room_info_only`
**仅查询房间信息** - 快速验证

**用途：**
- 验证房间查询逻辑是否正确
- 检查数据库中的房间数据
- 不执行模型生成和房间计算

**运行：**
```bash
cargo test --test test_room_integration --features sqlite,sqlite-index test_query_room_info_only -- --ignored --nocapture
```

**输出示例：**
```
🔍 房间信息查询测试
================================================================================
🏷️  房间关键词: ["-R-"]

✅ 找到 15 个房间

房间 #1 - K100
  Room Refno: pe:1516:SBFR/123456
  面板数量: 3
  面板列表:
    [1] pe:1516:PANE/234567
    [2] pe:1516:PANE/234568
    [3] pe:1516:PANE/234569
...
```

### 3. `test_rebuild_specific_rooms`
**特定房间重建** - 针对性测试

**用途：**
- 测试针对特定房间号重建关系
- 适用于需要重新计算特定房间的场景
- 默认测试前 3 个房间

**运行：**
```bash
cargo test --test test_room_integration --features sqlite,sqlite-index test_rebuild_specific_rooms -- --ignored --nocapture
```

### 4. `test_limited_room_integration`
**限制数量集成测试** - 快速验证

**用途：**
- 大规模数据库中快速验证流程
- 只处理前 5 个房间（可调整 `MAX_ROOMS` 常量）
- 适合开发调试

**运行：**
```bash
cargo test --test test_room_integration --features sqlite,sqlite-index test_limited_room_integration -- --ignored --nocapture
```

## 常见问题

### Q1: 测试失败 "初始化 SurrealDB 失败"
**原因：** SurrealDB 未运行或配置错误

**解决：**
1. 检查 SurrealDB 是否运行：
   ```bash
   ps aux | grep surreal
   ```
2. 启动 SurrealDB（参考项目启动脚本）
3. 检查 `DbOption.toml` 中的数据库配置

### Q2: 测试失败 "查询房间信息失败"
**原因：** 数据库中没有符合条件的房间

**解决：**
1. 检查 `room_keyword` 配置是否正确
2. 在数据库中查询是否有房间数据：
   ```sql
   SELECT * FROM SBFR WHERE '-R-' in NAME LIMIT 5;
   ```
3. 调整房间关键词以匹配你的数据

### Q3: 测试失败 "模型生成失败"
**原因：** Mesh 路径错误或权限问题

**解决：**
1. 检查 `meshes_path` 配置
2. 确保路径存在且有写权限：
   ```bash
   ls -la /path/to/meshes
   ```
3. 创建目录：
   ```bash
   mkdir -p /path/to/meshes
   ```

### Q4: Feature 'sqlite-index' 未启用
**解决：**
确保在 `Cargo.toml` 中启用了相关特性，运行时添加 `--features sqlite,sqlite-index`

## 自定义测试

### 调整房间关键词
修改 `DbOption.toml`：
```toml
room_keyword = "-RM"  # 或其他关键词
```

### 限制测试房间数量
修改 `test_limited_room_integration` 中的 `MAX_ROOMS` 常量：
```rust
const MAX_ROOMS: usize = 10; // 处理前 10 个房间
```

### 自定义模型生成配置
在测试函数中修改 `gen_db_option`：
```rust
let mut gen_db_option = db_option.clone();
gen_db_option.gen_model = true;
gen_db_option.gen_mesh = false;  // 不生成 mesh
gen_db_option.apply_boolean_operation = Some(false); // 不应用布尔运算
```

## 性能基准

**参考性能（取决于硬件和数据规模）：**

| 操作 | 10个房间 | 50个房间 | 100个房间 |
|------|----------|----------|-----------|
| 查询房间 | ~0.2s | ~0.5s | ~1.0s |
| 模型生成 | ~3s | ~15s | ~30s |
| 房间计算 | ~1s | ~5s | ~10s |
| **总耗时** | **~4s** | **~20s** | **~40s** |

## 日志级别

运行时可以设置日志级别查看详细信息：
```bash
RUST_LOG=debug cargo test --test test_room_integration --features sqlite,sqlite-index test_room_integration_complete -- --ignored --nocapture
```

日志级别：
- `error` - 只显示错误
- `warn` - 显示警告和错误
- `info` - 显示一般信息（默认）
- `debug` - 显示调试信息
- `trace` - 显示所有追踪信息

## 数据清理

测试后如需清理房间关系数据：
```sql
DELETE FROM room_relate;
DELETE FROM room_panel_relate;
```

## 技术实现细节

### 房间查询 SQL
- 项目 HD：从 `FRMW` 表查询，支持多级 pe_owner 关系
- 其他项目：从 `SBFR` 表查询，单级 pe_owner 关系

### 模型生成
调用 `gen_all_geos_data()` 生成面板的几何模型和 Mesh

### 房间计算
调用 `build_room_relations_v2()` 构建房间-构件空间关系

### 空间索引
使用 SQLite 空间索引加速房间内构件查询

## 相关文件

- 测试实现：`src/test/test_room_integration.rs`
- 房间查询 API：`src/web_server/room_api.rs`
- 房间计算逻辑：`src/fast_model/room_model_v2.rs`
- 模型生成逻辑：`src/fast_model/gen_model.rs`
- 配置文件：`DbOption.toml`
