# gen_geos_data 函数性能测试工具

## 🎯 项目概述

这是一个专门为 AIOS 项目开发的 `gen_geos_data` 函数性能测试工具。与之前的通用性能测试不同，这个工具专注于测试单个核心函数 `gen_model::gen_geos_data` 的性能表现。

**重要理解**:
- 传入的参考号（如 `24383_66456`）作为**根节点**
- `gen_geos_data` 函数会以此为起点，查找该元件下的**所有子节点**（PLOO、CATA、LOOP、PRIM等）
- 为所有这些子节点生成几何体数据
- 因此，传入一个参考号实际上会生成该元件下的**所有几何体**

## ✨ 核心特性

### 🎯 专门的函数测试
- **目标函数**: `crate::fast_model::gen_model::gen_geos_data`
- **输入参数**: `manual_refnos: Vec<RefnoEnum>`
- **测试维度**: 函数执行时间、处理效率、生成结果统计

### 🔧 多种测试模式
- **手动模式 (manual)**: 直接指定参考号列表进行测试
- **数据库模式 (database)**: 从数据库查询参考号进行测试
- **批量模式 (batch)**: 测试不同规模的参考号组

### 📊 详细性能分析
- 函数执行时间精确测量
- 参考号处理速度统计
- 实例生成效率分析
- 成功率和错误分析
- Chrome DevTools 兼容的性能追踪

## 🚀 快速开始

### 1. 基本测试命令

```bash
# 快速测试 - 从数据库查询少量参考号作为根节点
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 5

# 手动指定参考号测试 - 测试单个元件下的所有几何体
cargo run --release --bin test_gen_geos_data -- --mode manual --refnos "24383_66456"

# 批量测试不同规模 - 测试多个根节点
cargo run --release --bin test_gen_geos_data -- --mode batch --dbno 24383 --batch-count 3 --batch-size 5

# 启用性能追踪 - 深度分析性能瓶颈
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --trace --max-refnos 3
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

## 📋 命令行参数

| 参数 | 简写 | 默认值 | 说明 |
|------|------|--------|------|
| `--mode` | `-m` | database | 测试模式 (manual/database/batch) |
| `--dbno` | `-d` | 24383 | 数据库号 |
| `--types` | `-t` | PRIM LOOP | 参考号类型 |
| `--max-refnos` | | 无限制 | 最大参考号数量 |
| `--refnos` | `-r` | 无 | 手动指定参考号 (逗号分隔) |
| `--output` | `-o` | gen_geos_data_performance_report.txt | 输出文件名 |
| `--trace` | `-T` | false | 启用性能追踪 |
| `--batch-count` | `-b` | 3 | 批量测试组数 |
| `--batch-size` | `-S` | 10 | 每组参考号数量 |

## 📊 输出结果示例

### 基本统计信息
```
基本信息:
  输入根节点数量: 3
  处理子节点数量: 156
  生成实例数量: 1248
  生成形状数据组数: 156
  生成总形状数量: 3120
  执行状态: 成功

时间统计:
  总耗时: 2450ms
  整体耗时: 2480ms

性能指标:
  子节点处理速度: 63.67 节点/秒
  实例生成速度: 509.39 实例/秒
  形状生成速度: 1273.47 形状/秒
  平均子节点处理时间: 15.71ms/节点
  平均实例生成时间: 1.96ms/实例
  平均形状生成时间: 0.79ms/形状

效率评估:
  效率等级: 优秀 🌟
  子节点扩展比例: 52.0:1 (每个根节点平均包含 52.0 个子节点)
  形状生成比例: 20.0 形状/子节点
```

### 详细数据表格
```
序号   根节点数   子节点数   生成实例   生成形状   耗时(ms)   子节点速度   实例速度   形状速度   状态
1      1         52        416       1040      800       65.00      520.00    1300.00   成功
2      2         104       832       2080      1600      65.00      520.00    1300.00   成功
3      3         156       1248      3120      2400      65.00      520.00    1300.00   成功
```

## 🎯 性能评估标准

### 子节点处理速度等级
- **> 50 节点/秒**: 优秀 🌟
- **20-50 节点/秒**: 良好 👍
- **10-20 节点/秒**: 一般 ⚠️
- **< 10 节点/秒**: 需要优化 🔧

### 形状生成速度等级
- **> 1000 形状/秒**: 优秀 🌟
- **500-1000 形状/秒**: 良好 👍
- **100-500 形状/秒**: 一般 ⚠️
- **< 100 形状/秒**: 需要优化 🔧

### 扩展比例评估
- **> 30:1**: 复杂元件（包含大量子节点）
- **10-30:1**: 中等复杂度元件
- **< 10:1**: 简单元件

### 成功率标准
- **> 95%**: 稳定性优秀
- **90-95%**: 稳定性良好
- **< 90%**: 需要提高容错性

## 🔍 深度性能分析

### 启用性能追踪
```bash
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --trace
```

生成的 `performance_trace.json` 文件可以在 Chrome 浏览器中分析：
1. 打开 `chrome://tracing/`
2. 加载 `performance_trace.json` 文件
3. 分析函数调用时间线和性能瓶颈

## 📁 项目结构

```
├── src/
│   ├── bin/
│   │   └── test_gen_geos_data.rs          # 命令行测试工具
│   └── test/
│       └── test_performance.rs            # 核心测试函数
├── examples/
│   └── test_gen_geos_data_example.rs      # 使用示例
├── scripts/
│   ├── test_gen_geos_data.bat             # Windows 批处理脚本
│   └── test_gen_geos_data.sh              # Linux/Mac 脚本
└── docs/
    ├── gen_geos_data性能测试使用指南.md    # 详细使用指南
    └── README_gen_geos_data性能测试.md     # 本文档
```

## 🛠️ 开发和扩展

### 核心测试函数
```rust
// 测试 gen_geos_data 函数性能
pub async fn test_gen_geos_data_performance(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<GenGeosDataPerformanceStats>

// 批量测试
pub async fn batch_test_gen_geos_data_performance(
    refno_groups: Vec<Vec<RefnoEnum>>,
    db_option: &DbOption,
) -> anyhow::Result<Vec<GenGeosDataPerformanceStats>>

// 从数据库查询并测试
pub async fn test_gen_geos_data_from_database(
    dbno: u32,
    refno_types: &[&str],
    max_refnos: Option<usize>,
    db_option: &DbOption,
) -> anyhow::Result<GenGeosDataPerformanceStats>
```

### 性能统计结构
```rust
pub struct GenGeosDataPerformanceStats {
    pub input_refno_count: usize,           // 输入参考号数量
    pub processed_refno_count: usize,       // 处理参考号数量
    pub generated_instance_count: usize,    // 生成实例数量
    pub generated_shape_data_count: usize,  // 生成形状数据数量
    pub total_time_ms: u128,                // 总耗时
    pub success: bool,                      // 是否成功
    pub performance_metrics: GenGeosDataMetrics, // 性能指标
}
```

## 🔧 故障排除

### 常见问题

1. **编译错误**
   ```bash
   cargo clean
   cargo build --release --bin test_gen_geos_data
   ```

2. **数据库连接问题**
   - 确保数据库服务正在运行
   - 检查数据库连接配置
   - 验证数据库中是否有测试数据

3. **参考号查询为空**
   ```bash
   # 检查数据库中的参考号
   cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 1
   ```

## 💡 最佳实践

1. **渐进式测试**: 从小规模开始，逐步增加测试规模
2. **多次测试**: 运行多次测试取平均值，避免偶然因素
3. **对比测试**: 在优化前后进行对比测试
4. **追踪分析**: 对性能瓶颈使用追踪功能深度分析
5. **文档记录**: 记录测试结果和优化效果

## 🎯 优化建议

根据测试结果，系统会自动生成针对性的优化建议：

- **处理速度 < 5 参考号/秒**: 检查算法复杂度，考虑并行处理
- **成功率 < 90%**: 提高错误处理的健壮性
- **内存使用过高**: 优化内存分配策略
- **I/O 等待时间长**: 优化数据库查询和文件操作

## 📞 技术支持

如果您在使用过程中遇到问题，请：

1. 查看详细使用指南: `gen_geos_data性能测试使用指南.md`
2. 运行示例程序验证环境配置
3. 检查生成的错误日志和性能报告
4. 联系开发团队获取技术支持

---

**开发完成时间**: 2025-06-29  
**版本**: v1.0  
**状态**: ✅ 完成并可用  
**兼容性**: AIOS v3.0+  
**许可证**: 与 AIOS 项目保持一致
