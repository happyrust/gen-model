# gen_geos_data 性能测试工具更新说明

## 🎯 更新概述

根据您的反馈，我们已经对 `gen_geos_data` 性能测试工具进行了重要更新，确保它正确理解和实现了以下关键点：

### ✅ 关键修复

1. **数据库连接初始化**
   - 在 `test_gen_geos_data_performance` 函数中添加了数据库连接初始化
   - 参考 main 函数中的用法，使用 `init_surreal()` 和 `SUL_DB` 进行连接
   - 支持 WebSocket (`ws` feature) 和本地 (`local` feature) 两种连接模式

2. **正确的 dbno 参数处理**
   - 当手动指定 `refnos` 时，不向 `gen_geos_data` 函数传入 `dbno` 参数
   - 因为参考号（如 `24383_66456`）已经包含了数据库信息
   - 调用时使用 `None` 作为 `dbno` 参数

3. **函数行为理解**
   - 传入的参考号作为**根节点**
   - `gen_geos_data` 函数会查找该元件下的**所有子节点**
   - 为所有子节点生成几何体数据
   - 因此传入一个参考号会生成该元件下的**所有几何体**

## 🔧 技术实现细节

### 数据库连接代码

```rust
// 第一步：初始化数据库连接
info!("初始化数据库连接...");
use aios_core::{init_surreal, SUL_DB};

#[cfg(feature = "ws")]
{
    match init_surreal().await {
        Ok(_) => {
            info!("数据库连接成功: {}", db_option.project_name);
        }
        Err(e) => {
            error!("数据库连接失败: {}", e);
            return Err(anyhow::anyhow!("数据库连接失败: {}", e));
        }
    }
}

#[cfg(feature = "local")]
{
    let config = surrealdb::opt::Config::default().ast_payload();
    SUL_DB
        .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
        .with_capacity(1000)
        .await?;
    info!("本地数据库连接成功: {}", db_option.project_name);
}
```

### gen_geos_data 函数调用

```rust
// 调用 gen_geos_data 函数
// 注意：当手动指定 refnos 时，不需要传入 dbno，因为参考号已经包含了数据库信息
let result = crate::fast_model::gen_model::gen_geos_data(
    None, // dbno - 手动指定 refnos 时不需要传入
    manual_refnos.clone(),
    db_option,
    None, // incr_updates
    sender,
).await;
```

### 数据库配置

```rust
// 配置数据库选项 - 使用项目的默认配置
let mut db_option = aios_core::get_db_option().clone();
db_option.gen_model = true;
db_option.gen_mesh = true;
db_option.debug_refno_types = args.types.clone();
```

## 📊 更新的性能统计

### 新增统计项

- **输入根节点数量**: 传入的参考号数量
- **处理子节点数量**: 实际处理的子节点数量  
- **生成总形状数量**: 生成的所有形状数量
- **子节点扩展比例**: 每个根节点包含多少子节点
- **形状生成比例**: 每个子节点生成多少形状

### 性能指标

- **子节点处理速度**: 节点/秒
- **实例生成速度**: 实例/秒
- **形状生成速度**: 形状/秒
- **平均子节点处理时间**: ms/节点
- **平均实例生成时间**: ms/实例
- **平均形状生成时间**: ms/形状

## 🚀 使用示例

### 测试单个元件下的所有几何体

```bash
# 测试 24383_66456 元件下的所有几何体
cargo run --release --bin test_gen_geos_data -- --mode manual --refnos "24383_66456"
```

### 从数据库查询根节点进行测试

```bash
# 从数据库查询 PRIM 类型的参考号作为根节点
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 5
```

### 批量测试多个根节点

```bash
# 批量测试多个根节点的性能
cargo run --release --bin test_gen_geos_data -- --mode batch --dbno 24383 --batch-count 3 --batch-size 5
```

## 📋 输出报告示例

```
基本信息:
  输入根节点数量: 1
  处理子节点数量: 52
  生成实例数量: 416
  生成形状数据组数: 52
  生成总形状数量: 1040
  执行状态: 成功

时间统计:
  总耗时: 800ms
  整体耗时: 820ms

性能指标:
  子节点处理速度: 65.00 节点/秒
  实例生成速度: 520.00 实例/秒
  形状生成速度: 1300.00 形状/秒
  平均子节点处理时间: 15.38ms/节点
  平均实例生成时间: 1.92ms/实例
  平均形状生成时间: 0.77ms/形状

效率评估:
  效率等级: 优秀 🌟
  子节点扩展比例: 52.0:1 (每个根节点平均包含 52.0 个子节点)
  形状生成比例: 20.0 形状/子节点
```

## ✅ 验证状态

- ✅ 数据库连接初始化：已实现
- ✅ 正确的 dbno 参数处理：已修复
- ✅ 函数行为理解：已更正
- ✅ 性能统计完善：已更新
- ✅ 编译测试：通过
- ✅ 文档更新：完成

## 🎯 核心改进

1. **正确理解函数行为**: 明确了传入参考号作为根节点，函数生成该元件下所有几何体的工作原理
2. **数据库连接**: 添加了完整的数据库初始化流程
3. **参数处理**: 修正了 dbno 参数的使用逻辑
4. **性能统计**: 增强了统计维度，更准确反映函数的实际工作量
5. **错误处理**: 改进了数据库连接失败的错误处理

现在工具已经完全符合您的需求，能够正确测试 `gen_geos_data` 函数的性能！🎉
