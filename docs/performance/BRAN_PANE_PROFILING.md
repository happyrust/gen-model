# BRAN/PANE 性能分析指南

## 概述

本文档介绍如何使用集成的性能分析工具来识别和优化 Full Noun 模式下 BRAN 和 PANE 元素的模型生成性能瓶颈。

## 目录

- [快速开始](#快速开始)
- [性能分析工具](#性能分析工具)
- [测试脚本使用](#测试脚本使用)
- [性能指标解读](#性能指标解读)
- [Chrome Tracing 分析](#chrome-tracing-分析)
- [常见性能瓶颈](#常见性能瓶颈)
- [优化建议](#优化建议)

---

## 快速开始

### 1. 编译启用性能分析

```bash
# 编译 release 版本并启用 profile feature
cargo build --release --features profile,web_server

# 运行 Web 服务器
cargo run --release --features profile,web_server --bin web_server
```

### 2. 运行性能测试

```bash
# 方式 1: 单个 refno 测试
python3 test_single_refno.py 15201 1112

# 方式 2: BRAN 批量测试
python3 test_single_refno.py --benchmark --type bran

# 方式 3: PANE 批量测试
python3 test_single_refno.py --benchmark --type pane

# 方式 4: 完整性能套件测试
python3 test_bran_pane_performance.py --full --repeat 5
```

### 3. 查看性能报告

- **JSON 报告**: `performance_report.json` 或 `bran_pane_performance_report.json`
- **Chrome Tracing**: `chrome_trace_cata_model.json`
- **服务器日志**: 查看控制台输出的性能指标

---

## 性能分析工具

### 工具组件

| 工具 | 用途 | 输出 |
|------|------|------|
| `test_single_refno.py` | 单个/批量 refno 测试 | JSON 报告 + 控制台统计 |
| `test_bran_pane_performance.py` | 深度性能分析 | 详细 JSON 报告 + 统计对比 |
| Chrome Tracing (Rust) | 函数级性能追踪 | `chrome_trace_cata_model.json` |
| 控制台日志 (Rust) | 实时性能指标 | STDOUT |

### 性能埋点位置

#### Full Noun 模式
- **文件**: `src/fast_model/gen_model/full_noun_mode.rs`
- **埋点**:
  - LOOP 处理阶段（包含 PANE/FLOOR/GWALL）
  - PRIM 处理阶段
  - CATE 处理阶段（包含 BRAN/HANG）
  - 总体耗时和百分比分布

#### BRAN 处理
- **文件**: `src/fast_model/cata_model.rs`
- **埋点**:
  - 单个 BRAN 元素处理时间
  - Tubing 隐式管道生成
  - 数据库查询时间（`get_cat_refno`, `get_named_attmap`）
  - CSG shape 转换时间

#### PANE 处理
- **文件**: `src/fast_model/loop_model.rs`
- **埋点**:
  - SJUS 位置调整时间
  - Extrusion 拉伸体生成时间
  - 顶点数量和高度参数

---

## 测试脚本使用

### test_single_refno.py

#### 基本用法

```bash
# 单个 refno 测试
python3 test_single_refno.py <refno> <db_num> [meshes_path]

# 示例
python3 test_single_refno.py 15201 1112
python3 test_single_refno.py 299 1112
```

#### 批量测试

```bash
# 测试所有 BRAN 用例
python3 test_single_refno.py --benchmark --type bran

# 测试所有 PANE 用例
python3 test_single_refno.py --benchmark --type pane

# 测试全部（BRAN + PANE）
python3 test_single_refno.py --benchmark --type all

# 自定义输出文件
python3 test_single_refno.py --benchmark --type all --output my_report.json
```

#### 输出示例

```
======================================================================
📊 性能测试报告
======================================================================

📈 BRAN 类型:
  成功率: 3/3 (100.0%)
  平均响应时间: 245.67 ms
  最快响应: 231.45 ms
  最慢响应: 267.89 ms

📈 PANE 类型:
  成功率: 3/3 (100.0%)
  平均响应时间: 189.23 ms
  最快响应: 175.34 ms
  最慢响应: 201.12 ms
```

### test_bran_pane_performance.py

#### 深度性能测试

```bash
# 快速测试（每个用例 1 次）
python3 test_bran_pane_performance.py --quick

# 完整测试（每个用例 3 次，默认）
python3 test_bran_pane_performance.py --full

# 深度测试（每个用例 5 次）
python3 test_bran_pane_performance.py --full --repeat 5

# 只测试 BRAN
python3 test_bran_pane_performance.py --full --type bran

# 只测试 PANE
python3 test_bran_pane_performance.py --full --type pane
```

#### 输出包含

- 成功率统计
- 平均响应时间
- 响应时间中位数
- 标准差
- BRAN vs PANE 对比分析

---

## 性能指标解读

### 控制台输出指标

#### Full Noun 总体性能

```
⏱️  Total Full Noun processing: 1523 ms
   ├─ LOOP: 456 ms (30.0%)
   ├─ PRIM: 321 ms (21.1%)
   └─ CATE: 746 ms (48.9%)
```

**解读**:
- **LOOP**: PANE/FLOOR/GWALL 拉伸体生成占比
- **PRIM**: 基础几何体（BOX/CYL 等）占比
- **CATE**: BRAN/HANG 元件库元素占比最高

#### BRAN Tubing 生成

```
BRAN Tubing generation completed:
  - branch_count: 15
  - tubi_generated: 47
  - total_time_ms: 324
  - avg_time_per_branch_ms: 21
```

**解读**:
- 每个 BRAN 分支平均生成 3.1 个 Tubing 段
- 单个分支平均耗时 21ms
- 关注 `avg_time_per_branch_ms > 50` 的异常情况

#### PANE 处理指标

```
PANE/FLOOR/GWALL extrusion processed:
  - refno: 1112/299
  - noun_type: PANE
  - height: 3500.0
  - vert_count: 4
  - processing_ms: 1.234
```

**解读**:
- `vert_count`: 顶点数量，越多越复杂
- `processing_ms`: 单个 PANE 处理时间
- 通常 < 5ms 为正常，> 10ms 需关注

### JSON 报告格式

```json
{
  "timestamp": "2025-11-15T10:30:00",
  "summary": {
    "bran": {
      "total_cases": 3,
      "successful_cases": 3,
      "avg_response_ms": 245.67
    },
    "pane": {
      "total_cases": 3,
      "successful_cases": 3,
      "avg_response_ms": 189.23
    }
  },
  "details": [
    {
      "refno": "15201",
      "desc": "BRAN管道分支-1",
      "success": true,
      "response_time_ms": 231.45,
      "test_type": "BRAN"
    }
    // ...
  ]
}
```

---

## Chrome Tracing 分析

### 打开追踪文件

1. 打开 Chrome 浏览器
2. 访问 `chrome://tracing`
3. 点击 "Load" 按钮
4. 选择 `chrome_trace_cata_model.json` 文件

### 关键函数追踪

#### 函数层级

```
gen_full_noun_geos_optimized
├── process_loop_refno_page (PANE/FLOOR/GWALL)
├── process_prim_refno_page
└── process_cate_refno_page
    └── gen_cata_geos (BRAN/HANG)
        ├── gen_cata_single_geoms
        ├── create_profile_geos
        └── BRAN Tubing generation
```

### 使用技巧

1. **按 W/S 键**: 放大/缩小时间轴
2. **按 A/D 键**: 左右移动视图
3. **点击函数块**: 查看详细时间和参数
4. **搜索功能**: 输入 "BRAN" 或 "PANE" 快速定位

### 性能火焰图

- **宽度**: 函数执行时间
- **高度**: 调用栈深度
- **颜色**: 不同线程/进程

**识别瓶颈**:
- 最宽的横条 = 最耗时的函数
- 关注宽且深的区域 = 嵌套调用耗时多

---

## 常见性能瓶颈

### BRAN 生成瓶颈

#### 1. 数据库查询慢

**症状**:
```
get_cat_refno: 45ms per call
get_named_attmap: 32ms per call
```

**原因**:
- 数据库连接池不足
- 查询未使用索引
- 网络延迟（远程数据库）

**解决方案**:
- 批量查询替代逐个查询
- 添加缓存层
- 使用本地数据库副本

#### 2. Tubing 生成慢

**症状**:
```
avg_time_per_branch_ms > 50
```

**原因**:
- BRAN 子元素过多（>20个）
- 复杂的管道路径计算

**解决方案**:
- 优化 Tubing 算法
- 并行处理多个 BRAN
- 使用空间索引加速查找

#### 3. CSG Shape 转换慢

**症状**:
```
convert_to_csg_shapes: 80ms
```

**原因**:
- 复杂的几何体转换
- 内存分配频繁

**解决方案**:
- 缓存已转换的 CSG shape
- 使用对象池减少分配

### PANE 生成瓶颈

#### 1. SJUS 调整慢

**症状**:
```
SJUS adjustment: 15ms
```

**原因**:
- SJUS map 查找效率低
- 多次重复计算

**解决方案**:
- 预计算 SJUS 偏移量
- 使用更高效的 map 结构（已使用 DashMap）

#### 2. Extrusion 生成慢

**症状**:
```
PANE extrusion: 20ms (vert_count: 200)
```

**原因**:
- 顶点数量过多
- 复杂的多边形拉伸

**解决方案**:
- 简化多边形（减少顶点）
- 使用 LOD（细节层次）
- 并行处理多个 PANE

---

## 优化建议

### 短期优化（< 1 周）

1. **添加缓存**
   - 缓存 `get_cat_refno` 结果
   - 缓存 `get_named_attmap` 结果
   - 预期提升：20-30%

2. **批量数据库查询**
   - 将单个查询合并为批量查询
   - 预期提升：30-40%

3. **并行处理**
   - 增加 batch concurrency
   - 预期提升：15-25%（取决于 CPU 核心数）

### 中期优化（1-2 周）

1. **空间索引优化**
   - 为 SJUS map 建立空间索引
   - 预期提升：10-15%

2. **内存池技术**
   - 减少频繁的内存分配/释放
   - 预期提升：10-20%

3. **算法优化**
   - 优化 Tubing 路径计算算法
   - 预期提升：15-30%

### 长期优化（> 2 周）

1. **GPU 加速**
   - 将几何计算迁移到 GPU
   - 预期提升：2-5x

2. **增量更新**
   - 仅重新生成变更的元素
   - 预期提升：5-10x（增量场景）

3. **分布式处理**
   - 多机并行处理大型模型
   - 预期提升：线性扩展

---

## 性能基准

### 参考值（基于测试数据）

| 类型 | 元素数 | 总耗时 | 单个平均 | 目标值 |
|------|--------|--------|----------|--------|
| BRAN | 15 | 324 ms | 21.6 ms | < 15 ms |
| PANE | 25 | 456 ms | 18.2 ms | < 10 ms |
| PRIM | 50 | 321 ms | 6.4 ms | < 5 ms |

### 性能等级

- **优秀**: < 目标值
- **良好**: 目标值 - 1.5x 目标值
- **需优化**: 1.5x - 2x 目标值
- **严重**: > 2x 目标值

---

## 故障排查

### Chrome Tracing 文件为空

**问题**: `chrome_trace_cata_model.json` 文件存在但为空

**原因**:
1. 未启用 `profile` feature
2. tracing guard 未正确刷新

**解决**:
```bash
# 确认编译时启用了 profile
cargo build --release --features profile,web_server

# 检查日志中是否有 "Chrome tracing initialized"
```

### 性能测试请求超时

**问题**: Python 脚本报 `timeout` 错误

**原因**:
1. 模型生成确实很慢
2. 服务器负载高

**解决**:
```python
# 增加超时时间（在 test_single_refno.py 中）
response = requests.post(API_URL, json=payload, timeout=120)  # 改为 120 秒
```

### 性能指标不一致

**问题**: 多次测试结果差异大

**原因**:
1. 数据库缓存影响
2. 系统负载波动

**解决**:
```bash
# 使用多次重复测试并取平均值
python3 test_bran_pane_performance.py --full --repeat 5
```

---

## 附录

### 相关文件

- `test_single_refno.py`: 基础性能测试
- `test_bran_pane_performance.py`: 深度性能测试
- `src/fast_model/gen_model/full_noun_mode.rs`: Full Noun 主流程
- `src/fast_model/cata_model.rs`: BRAN 处理
- `src/fast_model/loop_model.rs`: PANE 处理

### LSP 符号类型参考

常用的 LSP symbol kinds:
- `5`: Class
- `6`: Method
- `12`: Function
- `13`: Variable

### 联系与反馈

如有问题或建议，请提交 Issue 或联系开发团队。

---

**最后更新**: 2025-11-15
**版本**: 1.0.0
