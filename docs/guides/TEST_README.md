# 模型生成测试指南

本文档说明如何运行 `pe:17496_230550` 及其他 refno 的模型生成测试。

## 测试文件

### Python 测试 (API 测试)

#### 1. `test_pe_17496_230550.py` - 专用测试脚本

测试特定 refno `pe:17496_230550` 的完整生成流程。

**功能特性:**
- ✅ 发起模型生成任务
- ✅ 监控任务执行状态
- ✅ 详细的进度输出
- ✅ 自动重试和超时处理
- ✅ 美观的终端输出

**使用方法:**

```bash
# 测试默认 refno (pe:17496_230550)
./test_pe_17496_230550.py

# 测试自定义 refno
./test_pe_17496_230550.py "pe:17496_230550" 1112

# 或使用 python 命令
python3 test_pe_17496_230550.py
```

**参数说明:**
- 参数 1: Refno (默认: `pe:17496_230550`)
- 参数 2: 数据库编号 (默认: `1112`)

**输出示例:**

```
╔══════════════════════════════════════════════════════════════════════════════╗
║                    模型生成测试工具                                          ║
║                                                                              ║
║  测试对象: pe:17496_230550                                                   ║
╚══════════════════════════════════════════════════════════════════════════════╝

================================================================================
  🧪 模型生成测试 - pe:17496_230550
================================================================================

────────────────────────────────────────────────────────────────────────────────
  📋 测试参数
────────────────────────────────────────────────────────────────────────────────
  🔖 Refno:           pe:17496_230550
  📊 数据库编号:      1112
  🔧 生成模型:        True
  🎨 生成网格:        True
  ⚡ 布尔运算:        True
  📁 输出路径:        使用默认配置
  ⏰ 测试时间:        2025-11-15 23:40:00

...

✅ 所有测试通过！
```

#### 2. `test_single_refno.py` - 通用测试脚本

测试任意 refno 的模型生成。

**使用方法:**

```bash
# 测试指定 refno
python3 test_single_refno.py "pe:17496_230550" 1112

# 测试其他 refno
python3 test_single_refno.py "1112/299" 1112
```

### Rust 集成测试

#### `tests/test_model_generation.rs` - Rust 单元测试

直接测试 Rust 代码的模型生成逻辑。

**使用方法:**

```bash
# 编译测试
cd /Volumes/DPC/work/plant-code/gen-model-fork
cargo test --test test_model_generation --no-run

# 运行所有测试 (包括被忽略的数据库测试)
cargo test --test test_model_generation -- --ignored --nocapture

# 运行特定测试
cargo test --test test_model_generation test_pe_17496_230550_generation -- --ignored --nocapture

# 运行性能测试
cargo test --test test_model_generation test_generation_performance -- --ignored --nocapture
```

**测试用例:**

1. **`test_pe_17496_230550_generation`**
   - 测试 `pe:17496_230550` 的完整生成流程
   - 验证元件类型、类别判断和几何生成
   - 需要数据库连接 (标记为 `#[ignore]`)

2. **`test_batch_generation`**
   - 批量测试多个 refno
   - 可添加更多测试用例

3. **`test_generation_performance`**
   - 性能基准测试
   - 验证生成耗时在可接受范围内

## 前置条件

### Python 测试

1. **安装依赖:**
   ```bash
   pip install requests
   ```

2. **启动 API 服务器:**
   ```bash
   # 确保后端 API 服务在运行
   # 默认地址: http://localhost:8080
   ```

3. **数据库连接:**
   - 确保数据库配置正确
   - 数据库编号默认为 `1112`

### Rust 测试

1. **数据库配置:**
   - 在 `assets/config/e3d.project.ron` 中配置数据库连接
   - 确保数据库可访问

2. **编译环境:**
   ```bash
   # 检查编译状态
   cargo check
   ```

## 测试覆盖范围

### 元件类型

测试覆盖以下元件类型:

#### Cate (元件库类型)
- SCTN, STWALL, GENSEC, WALL (Profile 类型)
- 其他标准元件库元件

#### Loop (循环拉伸/旋转类型)
- **旋转体:** REVO, NREV
- **拉伸体:** EXTR, NXTR, AEXTR
- **建筑类型:** PANE, FLOOR, GWALL, SCREED

#### Prim (基本体类型)
- BOX (长方体)
- CYL (圆柱体)
- CONE (圆锥体)
- SPHER (球体)
- TORUS (圆环体)
- POHE/POLYHE (多面体)

### 处理流程

1. **元件信息查询**
   - 获取元件属性
   - 判断元件类型和类别

2. **几何生成**
   - 调用对应处理器 (Cate/Loop/Prim)
   - 生成几何数据和网格

3. **结果验证**
   - 验证生成的实例数量
   - 检查数据完整性

## 故障排查

### 常见问题

#### 1. Python 测试连接失败

**错误:**
```
❌ 错误: HTTPConnectionPool(host='localhost', port=8080): Max retries exceeded
```

**解决方案:**
- 检查 API 服务器是否在运行
- 确认端口 8080 未被占用
- 检查防火墙设置

#### 2. Rust 测试数据库连接失败

**错误:**
```
thread 'test_pe_17496_230550_generation' panicked at '获取属性失败: ...'
```

**解决方案:**
- 检查数据库配置文件
- 确认数据库服务在运行
- 验证 refno 是否存在

#### 3. 测试超时

**错误:**
```
❌ 等待几何数据超时 (30秒)
```

**解决方案:**
- 增加超时时间
- 检查模型复杂度
- 查看服务器日志

## 扩展测试

### 添加新的测试用例

#### Python:

```python
# 在 test_pe_17496_230550.py 中修改
TEST_REFNO = "your_new_refno"
DB_NUM = your_db_num
```

#### Rust:

```rust
// 在 tests/test_model_generation.rs 中添加
#[tokio::test]
#[ignore]
async fn test_your_new_refno() {
    let refno = RefnoEnum::RefU64(RefU64(your_refno_id));
    // ... 测试逻辑
}
```

### 批量测试

创建包含多个 refno 的测试列表:

```python
# test_batch.py
TEST_REFNOS = [
    ("pe:17496_230550", 1112),
    ("pe:17496_230551", 1112),
    # ...
]

for refno, db_num in TEST_REFNOS:
    test_model_generation(refno, db_num)
```

## 性能基准

### 预期性能指标

| 元件类型 | 预期耗时 | 实例数量 |
|---------|---------|---------|
| Prim    | < 1s    | 1-10    |
| Loop    | < 5s    | 10-50   |
| Cate    | < 30s   | 50-500  |

### 性能优化建议

1. **批量处理:** 使用批量 API 减少网络开销
2. **并发生成:** 启用多线程加速
3. **缓存机制:** 复用已生成的几何数据

## 持续集成

### CI/CD 配置示例

```yaml
# .github/workflows/test.yml
name: Model Generation Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Run Rust tests
        run: cargo test --test test_model_generation
      - name: Run Python tests
        run: |
          pip install requests
          python3 test_api.py
```

## 相关文档

- [模型生成 API 文档](./API_DOCS.md)
- [重构总结](./REFACTORING_SUMMARY.md)
- [架构设计](./ARCHITECTURE.md)

## 联系方式

如有问题或建议,请提交 Issue 或联系开发团队。
