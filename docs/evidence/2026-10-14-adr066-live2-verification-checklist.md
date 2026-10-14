# ADR-066 Live #2 真机验证清单

- 日期：2026-10-14
- 关联：Issue #36、ADR-066、`docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md`
- 目的：在翻 `model_transform_cheap_path` 默认值之前，完成真机链验证

---

## 1. 测试准备

### 1.1 环境要求

- E3D 3.1 环境（Design + Paragon）
- 共享样例工程 ams7997（ZONE 24381/103249 存在）
- gen-model 服务已启动，开关 `model_transform_cheap_path=true`（通过环境变量 `AIOS_MODEL_TRANSFORM_CHEAP_PATH=1` 或 DbOption.toml）
- plant-ui 可访问（用于验证显示）

### 1.2 前置检查

```bash
# 1. 确认服务健康
curl http://127.0.0.1:8022/api/v1/health

# 2. 确认开关已打开（在日志或 /health 中查看）
# 或通过环境变量临时打开
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "1"

# 3. 确认 ams7997 的 ZONE 103249 存在
# 在 E3D 中：=24381/103249，Q CE 应返回 ZONE
```

---

## 2. 测试脚本

### 2.1 脚本清单

| 脚本 | 用途 | 路径 |
|------|------|------|
| `adr066_live2_rvm_export_before.mac` | 挪动前导出 RVM 基线 | `scripts/e3d/` |
| `adr066_live2_zone_103249_move_apply.mac` | 挪 ZONE（东移 1000mm）+ SAVEWORK | `scripts/e3d/` |
| `adr066_live2_rvm_export_after.mac` | 挪动后导出 RVM 真值 | `scripts/e3d/` |
| `adr066_live2_move_and_export.mac` | 链式执行（挪 + 导出） | `scripts/e3d/` |
| `adr066_live2_zone_103249_move_restore.mac` | 还原（西移 1000mm） | `scripts/e3d/` |

### 2.2 执行方式

**方式 A：链式执行（推荐）**

```bash
# 在 E3D TTY 中执行
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_move_and_export.mac"
```

**方式 B：分步执行**

```bash
# 1. 导出基线
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_before.mac"

# 2. 挪 ZONE + SAVEWORK
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"

# 3. 导出真值
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_after.mac"

# 4. 测试完成后还原
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"
```

---

## 3. 验证项

### 3.1 回执验证（gen-model 侧）

```bash
# 查看最近的任务回执，确认有 transform 单元
curl "http://127.0.0.1:8022/api/v1/model/records?root=24381/103249"

# 预期：units[].kind == "transform" 的单元存在
# 示例回执片段：
# {
#   "units": [
#     {"kind": "transform", "root": "24381/103249", "records": 237, ...}
#   ]
# }
```

**检查点**：
- [ ] 回执里有 `kind == "transform"` 的单元
- [ ] transform 单元的记录数 = 237（9 根 BRAN，含 128 隐式管身）
- [ ] 没有 `kind == "regen"` 的单元（这一窗应该全走 transform）

### 3.2 水位同步验证

```bash
# 等待 /dbnums 显示 in_sync（120 秒内）
curl "http://127.0.0.1:8022/api/v1/dbnums"

# 预期：ams7997 的状态为 in_sync，水位已前进
```

**检查点**：
- [ ] 120 秒内 `in_sync`
- [ ] 水位（model_sesno）已前进到 T

### 3.3 房间边验证

```bash
# 查询房间边是否随之更新
curl "http://127.0.0.1:8022/api/v1/spatial/rooms?dbnum=7997"

# 或通过 plant-ui 查看房间高亮
```

**检查点**：
- [ ] ZONE 103249 所在房间的边界已更新
- [ ] 房间内构件的归属正确

### 3.4 plant-ui 显示验证

1. 打开 plant-ui（http://127.0.0.1:8022）
2. 导航到 ams7997
3. 选中 ZONE 24381/103249
4. 确认 3D 视图中 ZONE 已东移 1000mm
5. 确认房间高亮正确

**检查点**：
- [ ] 3D 视图显示正确（东移 1000mm）
- [ ] 房间高亮正确
- [ ] 构件属性面板显示正常

### 3.5 RVM 对比验证（真值）

```bash
# 使用 rvm_verify 对比 before/after 两份 RVM
# 预期：两份之差恰是刚体平移 (1000, 0, 0)

# 或使用现有的对比脚本
python scripts/rvm_verify.py \
  output/e3d-live2/zone_103249_before.rvm \
  output/e3d-live2/zone_103249_after.rvm
```

**检查点**：
- [ ] 所有构件的位移 = (1000, 0, 0) mm
- [ ] 网格形状不变（只是平移）
- [ ] 隐式管身数量和位置正确

---

## 4. 性能测试（验收 3）

### 4.1 测试方案

**目标**：证明开关打开后 Task 时长省 ≥ 30%。

**步骤**：

1. **关开关跑**（基线）
   ```bash
   # 设置环境变量关闭
   $env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "0"
   # 重启服务或触发一次增量更新
   # 记录：Task 时长、生成器 CPU 时间
   ```

2. **开开关跑**（优化）
   ```bash
   # 设置环境变量打开
   $env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "1"
   # 重启服务或触发同样的增量更新
   # 记录：Task 时长、生成器 CPU 时间
   ```

3. **对比**
   ```
   省时比例 = (基线时长 - 优化时长) / 基线时长
   验收标准：省时比例 ≥ 30%
   ```

### 4.2 测试窗口选择

选择一个典型的「只动 POS」窗口：
- 挪一个 ZONE（名下多个根）
- 窗口内只有 POS/ORI 变化
- 没有几何变化

推荐：ams7997 窗 270→271（ZONE 103249 被挪出的那一窗）

### 4.3 记录模板

| 指标 | 关开关（基线） | 开开关（优化） | 省时比例 |
|------|--------------|--------------|---------|
| Task 总时长 | ___ s | ___ s | ___ % |
| 生成器 CPU 时间 | ___ s | ___ s | ___ % |
| 网格重算次数 | ___ | 0 | - |
| 发布次数 | ___ | ___ | - |

---

## 5. 验收标准（汇总）

- [ ] **回执验证**：`units[].kind == "transform"` 存在，记录数正确
- [ ] **水位同步**：120 秒内 `in_sync`
- [ ] **房间边**：随之更新
- [ ] **plant-ui**：3D 显示正确，房间高亮正确
- [ ] **RVM 对比**：位移 = (1000, 0, 0) mm，形状不变
- [ ] **性能测试**：Task 时长省 ≥ 30%
- [ ] **九个钉死窗口**：桶位不变（已有测试 `live_ams8000_pinned_windows_land_in_the_expected_buckets`）

---

## 6. 测试完成后

全部通过后，执行翻默认值步骤：

1. 修改 `src/options.rs` 中 `effective_model_transform_cheap_path` 的 `.unwrap_or(false)` → `.unwrap_or(true)`
2. 更新注释和文档
3. 运行全量测试
4. 提交并推送

---

## 7. 故障排查

| 问题 | 可能原因 | 解决方案 |
|------|---------|---------|
| 回执里没有 transform 单元 | 开关未打开 | 检查环境变量或 DbOption.toml |
| 窗口退化 | 候选数超过索引键数 30% | 检查净候选数（candidates - cache_stamp）|
| 锚点被排除 | 命中规则 3 排除（路由容器/P23/负体等） | 查看 `transform_fallback_reason` |
| 水位不同步 | 发布失败 | 查看服务日志 |
| RVM 对比不等 | Δ 计算错误或网格读取失败 | 查看 `world_bounds` 差异 |

---

## 8. 相关文件

- ADR-066: `docs/adr/ADR-066-placement-only-windows-republish-by-rigid-motion-without-regenerating-meshes.md`
- S4b 计划: `docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md`
- 开关定义: `src/options.rs` (`model_transform_cheap_path`)
- 规划器: `src/data_interface/window_root_plan.rs` (`rigid_anchor`)
- 执行体: `src/data_interface/rigid_motion.rs`
- 测试脚本: `scripts/e3d/adr066_live2_*.mac`
