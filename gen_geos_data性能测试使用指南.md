# gen_geos_data 函数性能测试使用指南

## 🎯 概述

这个工具专门用于测试 `gen_model::gen_geos_data` 函数的性能，可以通过传入 `manual_refnos` 参数（如 `[24383_66456]`）来分析函数的计算时间。

**重要说明**: `gen_geos_data` 函数接收的参考号作为**根节点**，函数会：
1. 以传入的参考号作为起点
2. 查找该元件下的所有子节点（包括 PLOO、CATA、LOOP、PRIM 等类型）
3. 为所有这些子节点生成几何体数据

因此，传入一个参考号（如 `24383_66456`）实际上会生成该元件下的**所有几何体**。

## 🔧 核心功能

### 1. 专门的函数测试
- **目标函数**: `crate::fast_model::gen_model::gen_geos_data`
- **输入参数**: `manual_refnos: Vec<RefnoEnum>`
- **测试维度**: 函数执行时间、处理效率、生成结果统计

### 2. 多种测试模式
- **手动模式**: 直接指定参考号列表
- **数据库模式**: 从数据库查询参考号
- **批量模式**: 测试不同规模的参考号组

### 3. 详细性能分析
- 函数执行时间测量
- 参考号处理速度统计
- 实例生成效率分析
- 成功率和错误分析

## 🚀 快速开始

### 1. 使用命令行工具

```bash
# 基本测试 - 从数据库查询参考号
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 10

# 手动指定参考号测试
cargo run --release --bin test_gen_geos_data -- --mode manual --refnos "24383_123456,24383_123457,24383_123458"

# 批量测试不同规模
cargo run --release --bin test_gen_geos_data -- --mode batch --dbno 24383 --types PRIM --batch-count 3 --batch-size 20

# 启用性能追踪
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM LOOP --trace
```

### 2. 使用批处理脚本

**Windows:**
```cmd
scripts\test_gen_geos_data.bat
```

**Linux/Mac:**
```bash
./scripts/test_gen_geos_data.sh
```

### 3. 运行示例程序

```bash
cargo run --release --example test_gen_geos_data_example
```

## 📋 命令行参数详解

### 基本参数
- `--mode <MODE>`: 测试模式 (manual/database/batch)
- `--dbno <DBNO>`: 数据库号 (默认: 24383)
- `--types <TYPES>`: 参考号类型 (默认: PRIM, LOOP)
- `--max-refnos <NUM>`: 最大参考号数量限制
- `--output <FILE>`: 输出报告文件名

### 高级参数
- `--trace`: 启用性能追踪 (生成 Chrome DevTools 兼容文件)
- `--refnos <LIST>`: 手动指定参考号 (逗号分隔)
- `--batch-count <NUM>`: 批量测试组数
- `--batch-size <NUM>`: 每组参考号数量

## 📊 测试模式详解

### 1. 手动模式 (manual)
直接指定要测试的参考号列表：

```bash
cargo run --release --bin test_gen_geos_data -- \
  --mode manual \
  --refnos "24383_123456,24383_123457,24383_123458" \
  --output manual_test_report.txt
```

**适用场景:**
- 测试特定的参考号
- 调试特定问题
- 验证修复效果

### 2. 数据库模式 (database)
从数据库查询参考号进行测试：

```bash
cargo run --release --bin test_gen_geos_data -- \
  --mode database \
  --dbno 24383 \
  --types PRIM LOOP CATA \
  --max-refnos 50 \
  --trace \
  --output database_test_report.txt
```

**适用场景:**
- 常规性能测试
- 不同类型参考号的性能对比
- 大规模数据测试

### 3. 批量模式 (batch)
测试不同规模的参考号组：

```bash
cargo run --release --bin test_gen_geos_data -- \
  --mode batch \
  --dbno 24383 \
  --types PRIM \
  --batch-count 5 \
  --batch-size 15 \
  --trace \
  --output batch_test_report.txt
```

**适用场景:**
- 性能扩展性测试
- 寻找最优批处理大小
- 负载测试

## 📈 输出结果解读

### 1. 基本统计信息
```
基本信息:
  输入参考号数量: 30
  处理参考号数量: 28
  生成实例数量: 156
  生成形状数据数量: 28
  执行状态: 成功
```

### 2. 时间统计
```
时间统计:
  总耗时: 1250ms
  整体耗时: 1280ms
```

### 3. 性能指标
```
性能指标:
  处理速度: 22.40 参考号/秒
  生成速度: 124.80 实例/秒
  平均处理时间: 44.64ms/参考号
  平均生成时间: 8.01ms/实例
```

### 4. 效率评估
```
效率评估:
  效率等级: 优秀 🌟
  处理成功率: 93.3%
```

## 🔍 性能分析技巧

### 1. 基准测试
建立性能基准线：

```bash
# 小规模基准测试
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 10

# 中规模基准测试  
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 50

# 大规模基准测试
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 200
```

### 2. 性能对比
对比不同参考号类型的性能：

```bash
# 测试 PRIM 类型
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 50 --output prim_test.txt

# 测试 LOOP 类型
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types LOOP --max-refnos 50 --output loop_test.txt

# 测试 CATA 类型
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types CATA --max-refnos 50 --output cata_test.txt
```

### 3. 深度分析
使用性能追踪进行深度分析：

```bash
cargo run --release --bin test_gen_geos_data -- \
  --mode database \
  --dbno 24383 \
  --types PRIM \
  --max-refnos 20 \
  --trace \
  --output detailed_analysis.txt
```

然后在 Chrome 浏览器中：
1. 打开 `chrome://tracing/`
2. 加载生成的 `performance_trace.json` 文件
3. 分析函数调用时间线

## 🎯 性能优化建议

### 1. 根据处理速度优化
- **> 20 参考号/秒**: 性能优秀，无需优化
- **10-20 参考号/秒**: 性能良好，可考虑微调
- **5-10 参考号/秒**: 性能一般，建议优化
- **< 5 参考号/秒**: 性能较差，需要重点优化

### 2. 根据成功率优化
- **> 95%**: 稳定性优秀
- **90-95%**: 稳定性良好
- **< 90%**: 需要提高容错性

### 3. 常见优化方向
- 数据库查询优化
- 内存分配优化
- 并行处理优化
- 算法复杂度优化

## 🔧 故障排除

### 1. 编译问题
```bash
# 检查编译
cargo check --bin test_gen_geos_data

# 清理重新编译
cargo clean
cargo build --release --bin test_gen_geos_data
```

### 2. 数据库连接问题
- 确保数据库服务正在运行
- 检查数据库连接配置
- 验证数据库中是否有测试数据

### 3. 参考号查询问题
```bash
# 检查数据库中的参考号
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 1
```

## 📁 相关文件

### 核心文件
- `src/bin/test_gen_geos_data.rs` - 命令行测试工具
- `src/test/test_performance.rs` - 核心测试函数
- `examples/test_gen_geos_data_example.rs` - 使用示例

### 脚本文件
- `scripts/test_gen_geos_data.bat` - Windows 批处理脚本
- `scripts/test_gen_geos_data.sh` - Linux/Mac 脚本

### 输出文件
- `*_test_report.txt` - 测试报告
- `performance_trace.json` - 性能追踪文件

## 💡 最佳实践

1. **渐进式测试**: 从小规模开始，逐步增加测试规模
2. **多次测试**: 运行多次测试取平均值，避免偶然因素
3. **对比测试**: 在优化前后进行对比测试
4. **追踪分析**: 对性能瓶颈使用追踪功能深度分析
5. **文档记录**: 记录测试结果和优化效果

---

**开发完成时间**: 2025-06-29  
**版本**: v1.0  
**状态**: ✅ 可用  
**兼容性**: AIOS v3.0+
