# 模型生成性能测试工具

本工具用于测试和分析AIOS数据库中模型生成的性能，特别针对24383-66456范围内的数据库进行全面的性能评估。

## 功能特性

- **全面性能测试**: 测试实例生成、网格生成、布尔运算等各个阶段的性能
- **详细性能追踪**: 使用Chrome DevTools兼容的追踪格式，可视化分析性能瓶颈
- **智能瓶颈分析**: 自动分析性能瓶颈并提供优化建议
- **详细报告生成**: 生成包含统计数据和优化建议的详细报告

## 使用方法

### 1. 运行性能测试

```bash
# 测试24383-66456范围内的所有数据库（默认）
cargo run --bin performance_test

# 测试指定范围的数据库
cargo run --bin performance_test --start 24383 --end 24400

# 启用性能追踪
cargo run --bin performance_test --trace

# 指定输出文件
cargo run --bin performance_test --output my_report.txt
```

### 2. 运行单元测试

```bash
# 运行专门的24383-66456性能测试
cargo test test_model_generation_24383_66456 -- --nocapture

# 运行所有性能测试
cargo test test_performance -- --nocapture
```

### 3. 查看性能追踪

1. 运行带有`--trace`参数的测试
2. 打开Chrome浏览器
3. 访问 `chrome://tracing/`
4. 加载生成的 `performance_trace.json` 文件

## 输出文件说明

### 性能报告文件
- **文件名**: `performance_report.txt` (可自定义)
- **内容**: 包含详细的性能统计、错误信息和优化建议

### 性能追踪文件
- **文件名**: `performance_trace.json`
- **用途**: 在Chrome DevTools中查看详细的函数调用时间线

## 性能指标说明

### 主要指标
- **总时间**: 完整处理一个数据库的总耗时
- **实例时间**: 生成几何实例的耗时
- **网格时间**: 生成三角网格的耗时
- **布尔时间**: 执行布尔运算的耗时
- **实例数**: 生成的几何实例数量
- **网格数**: 生成的三角网格数量

### 性能比率
- **实例/秒**: 每秒生成的实例数量
- **网格/秒**: 每秒生成的网格数量

## 性能优化建议

工具会自动分析性能数据并提供以下类型的优化建议：

### 实例生成优化
- 增加并行处理线程数
- 优化数据库查询策略
- 实现缓存机制
- 减少内存分配

### 网格生成优化
- 使用更高效的网格算法
- 调整网格精度
- 实现增量更新
- 考虑GPU加速

### 布尔运算优化
- 使用高性能布尔运算库
- 并行化布尔运算
- 实现结果缓存
- 优化输入数据

### 通用优化
- 启用编译器优化
- 使用SIMD指令
- 优化数据结构
- 实现分布式处理

## 代码结构

```
src/test/test_performance.rs    # 主要的性能测试代码
src/bin/performance_test.rs     # 命令行工具
src/test/mod.rs                 # 测试模块导出
```

### 主要函数

- `test_model_generation_performance()`: 主要的性能测试函数
- `analyze_performance_bottlenecks()`: 性能瓶颈分析
- `init_performance_tracing()`: 初始化性能追踪
- `PerformanceStats`: 性能统计数据结构

## 配置选项

可以通过修改`DbOption`来调整测试参数：

```rust
let mut db_option = DbOption::default();
db_option.gen_model = true;        // 启用模型生成
db_option.gen_mesh = true;         // 启用网格生成
db_option.debug_refno_types = vec![
    "CATA".to_string(),           // 测试元件库
    "LOOP".to_string(),           // 测试管道
    "PRIM".to_string(),           // 测试基本体
];
```

## 注意事项

1. **内存使用**: 大范围测试可能消耗大量内存，建议分批测试
2. **数据库连接**: 确保数据库连接配置正确
3. **磁盘空间**: 追踪文件可能较大，确保有足够磁盘空间
4. **测试环境**: 建议在专用测试环境中运行，避免其他程序干扰

## 故障排除

### 常见问题

1. **数据库连接失败**
   - 检查数据库配置
   - 确认网络连接

2. **内存不足**
   - 减少测试范围
   - 增加系统内存

3. **追踪文件过大**
   - 减少测试范围
   - 调整追踪级别

### 调试技巧

- 使用`RUST_LOG=debug`环境变量获取详细日志
- 检查生成的错误报告
- 使用Chrome DevTools分析追踪文件

## 扩展开发

如需添加新的性能测试功能：

1. 在`test_performance.rs`中添加新的测试函数
2. 使用`#[instrument]`宏添加追踪
3. 更新`PerformanceStats`结构体（如需要）
4. 在`analyze_performance_bottlenecks`中添加新的分析逻辑
