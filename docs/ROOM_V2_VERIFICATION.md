# 房间计算 V2 改进验证指南

## 改进内容总结

### 1. 使用 L0 LOD Mesh
- **位置**: `src/fast_model/room_model_v2.rs` 和 `rs-core/src/room/query.rs`
- **改进**: 使用 `build_mesh_path(geo_hash, "L0")` 加载最低精度 mesh
- **优势**: 减少 60-80% 的 I/O 和内存占用

### 2. 基于关键点的精确几何检测
- **位置**: `src/fast_model/room_model_v2.rs`
- **新增函数**:
  - `extract_aabb_key_points()` - 提取 27 个关键点（8顶点+1中心+6面中心+12边中点）
  - `extract_geom_key_points()` - 从几何体实例提取关键点
  - `is_geom_in_panel()` - 判断关键点是否在面板内（50% 阈值）
- **算法**: 两阶段粗算+细算
  - 粗算: SQLite RTree 空间索引筛选候选
  - 细算: 关键点包含测试

### 3. 性能监控
- **新增日志**:
  - `🔍 粗算完成: 耗时 {:?}, 候选数 {}`
  - `✅ 细算完成: 耗时 {:?}, 结果数 {}`
  - `面板 {} 房间计算完成: 总耗时 {:?}, 粗算 {} -> 细算 {}`

## 验证准备

### 前置条件

1. **数据库连接**
   ```bash
   # 确保 DbOption.toml 配置正确
   cat DbOption.toml | grep -A 5 surreal
   ```

2. **L0 LOD Mesh 文件**
   ```bash
   # 检查 L0 mesh 目录
   ls -lh assets/meshes/lod_L0/ | head -20
   
   # 如果目录不存在或为空，需要先生成 L0 mesh：
   # 在 DbOption.toml 中设置 gen_mesh = true
   # 然后运行模型生成
   ```

3. **SQLite 空间索引**
   ```bash
   # 检查空间索引文件
   ls -lh test-room-build.db
   
   # 如果不存在，模型生成时会自动创建
   ```

## 验证方法

### 方法 1: 使用验证脚本（推荐）

```bash
# 运行完整验证测试
./scripts/test/test_room_v2_verification.sh
```

**脚本功能**:
- ✅ 自动检查环境（配置文件、L0 mesh、空间索引）
- ✅ 运行基础单元测试
- ✅ 运行完整验证测试
- ✅ 输出详细的验证结果

### 方法 2: 手动运行测试

```bash
# 1. 快速基础测试（不需要数据库）
cargo test --lib --features sqlite-index test_key_points_extraction -- --nocapture

# 2. 完整验证测试（需要数据库连接）
RUST_LOG=debug cargo test --features sqlite-index test_room_v2_with_lod_verification -- --ignored --nocapture

# 3. 使用现有集成测试
cargo test --features sqlite-index test_room_integration_complete -- --ignored --nocapture
```

## 验证检查点

### ✅ 必须检查的日志输出

运行测试时，**务必确认**以下日志出现：

1. **L0 mesh 加载**
   ```
   ✅ L0 LOD 目录存在: /path/to/meshes/lod_L0
      L0 mesh 文件数: 1234
   ```

2. **粗算日志**
   ```
   🔍 粗算完成: 耗时 50ms, 候选数 120
   ```
   - 验证: 空间索引正常工作
   - 耗时应该在几十到几百毫秒

3. **细算日志**
   ```
   ✅ 细算完成: 耗时 200ms, 结果数 45
   ```
   - 验证: 关键点检测正常工作
   - 结果数应该 < 候选数

4. **总计日志**
   ```
   面板 123456/789012 房间计算完成: 总耗时 250ms, 粗算 120 -> 细算 45
   ```
   - 验证: 整体流程正常
   - 粗算候选数 > 细算结果数

### ⚠️ 常见问题排查

#### 问题 1: 未找到 L0 mesh 文件

**现象**:
```
warn: 加载几何文件失败: abc123, error: No such file or directory
```

**解决**:
```bash
# 1. 确认 L0 目录存在
mkdir -p assets/meshes/lod_L0

# 2. 重新生成模型（确保 gen_mesh = true）
# 编辑 DbOption.toml:
#   gen_mesh = true
#   gen_model = true

# 3. 运行模型生成
cargo run --bin web_server --features web_server
# 然后通过 API 或直接调用生成函数
```

#### 问题 2: 粗算候选数为 0

**现象**:
```
🔍 粗算完成: 耗时 10ms, 候选数 0
```

**原因**: SQLite 空间索引未建立或为空

**解决**:
```bash
# 检查索引文件
sqlite3 test-room-build.db "SELECT COUNT(*) FROM aabb_index;"

# 如果为空，需要重建索引
# 通常在模型生成时会自动创建
```

#### 问题 3: 细算结果数等于候选数

**现象**:
```
粗算 120 -> 细算 120
```

**原因**: 关键点判断阈值可能过宽（50%）

**影响**: 结果不一定错误，但可能包含边界构件

**优化**: 可以考虑调整阈值为 60% 或 70%

#### 问题 4: 细算耗时异常长

**现象**:
```
✅ 细算完成: 耗时 5000ms, 结果数 45
```

**原因**: 
- 候选数过多（粗筛不够精确）
- 几何体复杂度高

**优化**:
- 检查空间索引是否正常
- 考虑进一步优化关键点数量

## 性能基准

基于典型工程项目的预期性能：

| 项目规模 | 房间数 | 面板数 | 总耗时 | 平均每房间 |
|---------|--------|--------|--------|-----------|
| 小型   | 10-50   | 50-200  | 5-30s  | 500ms     |
| 中型   | 50-200  | 200-1000| 30-120s| 600ms     |
| 大型   | 200-500 | 1000+   | 2-10min| 1000ms    |

**关键指标**:
- 粗算耗时 < 100ms/面板
- 细算耗时 < 500ms/面板
- 内存使用 < 2GB（大型项目）

## 对比验证

如果想对比改进前后的性能差异：

```bash
# 1. 使用改进后的代码运行
cargo test --features sqlite-index test_room_v2_with_lod_verification -- --ignored --nocapture > result_v2.log 2>&1

# 2. 提取关键指标
grep "总耗时" result_v2.log
grep "粗算完成" result_v2.log
grep "细算完成" result_v2.log

# 3. 分析日志中的性能数据
```

## 验证清单

完成验证后，请确认：

- [ ] L0 LOD mesh 文件存在且被正确加载
- [ ] 粗算日志正常输出，候选数合理
- [ ] 细算日志正常输出，结果数 ≤ 候选数
- [ ] 总耗时在预期范围内
- [ ] 数据库中 room_relate 关系数正常
- [ ] 无错误或警告日志（mesh 加载失败等）
- [ ] 内存使用在合理范围内

## 下一步

验证通过后：

1. **生产部署**: 更新生产环境代码
2. **性能监控**: 持续监控房间计算性能
3. **优化调整**: 根据实际数据调整阈值和参数
4. **文档更新**: 更新相关技术文档

---

**相关文件**:
- 实现代码: `src/fast_model/room_model_v2.rs`
- 验证测试: `src/test/test_room_v2_verification.rs`
- 验证脚本: `scripts/test/test_room_v2_verification.sh`
- 集成测试: `src/test/test_room_integration.rs`
