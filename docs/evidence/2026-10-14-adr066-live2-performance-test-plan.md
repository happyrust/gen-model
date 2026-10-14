# ADR-066 验收 3 性能测试方案

- 日期：2026-10-14
- 关联：Issue #36、ADR-066 验收 3
- 目的：证明 `model_transform_cheap_path` 打开后 Task 时长省 ≥ 30%

---

## 1. 测试目标

**验收标准**（ADR-066 验收 3）：
> 同一窗开 / 关各跑一次，Task 时长省 ≥ 30% 才翻默认。

**测试指标**：
- Task 总时长（从窗口规划到发布完成）
- 生成器 CPU 时间（几何求值 + 烘焙 + 发布）
- 网格重算次数（transform 路应该为 0）
- 内存占用峰值

---

## 2. 测试窗口选择

### 2.1 推荐窗口

**ams7997 窗 270→271**（ZONE 24381/103249 被挪出的那一窗）

- **变化**：ZONE 103249 的 POS 东移 1000mm
- **根数**：9 根 BRAN（237 记录，含 128 隐式管身）
- **预期**：全部走 `transform` 桶（只动 POS，无几何变化）

### 2.2 备选窗口

| 窗口 | 描述 | 根数 | 预期 transform 根数 |
|------|------|------|-------------------|
| ams8000 195→196 | 门 3 首跑窗口 | 7 窗 19 根 | 全部 |
| ams1112 724→725 | 嵌套根测试窗 | 1090 + 31 记录 | 部分（有嵌套根）|

---

## 3. 测试环境准备

### 3.1 服务配置

```toml
# DbOption.toml 或环境变量
[model]
# 测试时通过环境变量切换，不修改配置文件
# AIOS_MODEL_TRANSFORM_CHEAP_PATH=0  # 关（基线）
# AIOS_MODEL_TRANSFORM_CHEAP_PATH=1  # 开（优化）
```

### 3.2 环境变量

```powershell
# 关开关（基线测试）
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "0"

# 开开关（优化测试）
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "1"
```

### 3.3 前置条件

- 服务已启动，健康检查通过
- ams7997 库已初始化，模型已生成（水位在 S）
- 有足够的磁盘空间（RVM 导出、日志）

---

## 4. 测试步骤

### 4.1 基线测试（开关关）

```powershell
# 1. 设置环境变量
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "0"

# 2. 重启服务（或等待下一次增量触发）
# 记录启动时间
$baseline_start = Get-Date

# 3. 在 E3D 中执行挪动宏
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"

# 4. 等待任务完成（轮询 /dbnums 直到 in_sync）
do {
    Start-Sleep -Seconds 5
    $status = curl http://127.0.0.1:8022/api/v1/dbnums | ConvertFrom-Json
    $ams7997 = $status | Where-Object { $_.dbnum -eq 7997 }
} until ($ams7997.in_sync -eq $true -or ((Get-Date) - $baseline_start).TotalSeconds -gt 300)

$baseline_end = Get-Date
$baseline_duration = ($baseline_end - $baseline_start).TotalSeconds

# 5. 记录指标
# - Task 总时长：$baseline_duration
# - 从日志中提取生成器 CPU 时间
# - 网格重算次数：从回执中统计 kind == "regen" 的单元数
```

### 4.2 优化测试（开关开）

```powershell
# 1. 还原 ZONE 位置
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"
# 等待 in_sync

# 2. 设置环境变量
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "1"

# 3. 重启服务
# 记录启动时间
$opt_start = Get-Date

# 4. 在 E3D 中执行挪动宏（同样的挪动）
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"

# 5. 等待任务完成
do {
    Start-Sleep -Seconds 5
    $status = curl http://127.0.0.1:8022/api/v1/dbnums | ConvertFrom-Json
    $ams7997 = $status | Where-Object { $_.dbnum -eq 7997 }
} until ($ams7997.in_sync -eq $true -or ((Get-Date) - $opt_start).TotalSeconds -gt 300)

$opt_end = Get-Date
$opt_duration = ($opt_end - $opt_start).TotalSeconds

# 6. 记录指标
# - Task 总时长：$opt_duration
# - 从日志中提取生成器 CPU 时间
# - 网格重算次数：从回执中统计 kind == "transform" 的单元数（应该 = 9）
```

### 4.3 还原环境

```powershell
# 还原 ZONE 位置
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"

# 恢复环境变量
Remove-Item Env:AIOS_MODEL_TRANSFORM_CHEAP_PATH
```

---

## 5. 指标采集

### 5.1 自动采集脚本

创建 `scripts/Measure-RigidMotionPerf.ps1`：

```powershell
param(
    [string]$Dbnum = "7997",
    [string]$RootRefno = "24381/103249",
    [int]$TimeoutSec = 300
)

# 采集任务指标
function Get-TaskMetrics {
    param([string]$TaskId)
    $task = curl "http://127.0.0.1:8022/api/v1/tasks/$TaskId" | ConvertFrom-Json
    return @{
        DurationSec = $task.duration_sec
        CpuSec = $task.generator_cpu_sec
        RegenCount = ($task.units | Where-Object { $_.kind -eq "regen" }).Count
        TransformCount = ($task.units | Where-Object { $_.kind -eq "transform" }).Count
        MeshRecalcCount = $task.mesh_recalc_count
    }
}

# 主流程
$start = Get-Date
# ... 触发增量更新 ...
$end = Get-Date

@{
    TotalDurationSec = ($end - $start).TotalSeconds
    TaskMetrics = Get-TaskMetrics -TaskId $taskId
    Timestamp = $start.ToString("o")
} | ConvertTo-Json | Tee-Object -FilePath "verification/perf-$(Get-Date -Format 'yyyyMMdd-HHmmss').json"
```

### 5.2 手动采集清单

| 指标 | 采集方式 | 位置 |
|------|---------|------|
| Task 总时长 | 服务日志时间戳差 | `logs/aios-database.log` |
| 生成器 CPU 时间 | 任务回执 `generator_cpu_sec` | `/api/v1/tasks/{id}` |
| 网格重算次数 | 回执 `mesh_recalc_count` | `/api/v1/model/records` |
| transform 单元数 | 回执 `units[].kind == "transform"` | `/api/v1/model/records` |
| 内存峰值 | `/health` 或进程监控 | `/api/v1/health` |

---

## 6. 结果判定

### 6.1 计算公式

```
省时比例 = (基线时长 - 优化时长) / 基线时长 × 100%

验收标准：省时比例 ≥ 30%
```

### 6.2 预期结果

| 指标 | 基线（关） | 优化（开） | 预期改善 |
|------|-----------|-----------|---------|
| Task 总时长 | ~60s | ~15s | **省 75%** |
| 生成器 CPU 时间 | ~45s | ~5s | **省 89%** |
| 网格重算次数 | 9 根 × 网格数 | 0 | **省 100%** |
| transform 单元数 | 0 | 9 | - |
| 内存峰值 | 基线 | 略低（不烘焙网格） | 略省 |

### 6.3 判定标准

- **通过**：省时比例 ≥ 30%，且 live #2 真机链验证通过
- **不通过**：省时比例 < 30%，维持默认关，记录原因

---

## 7. 测试报告模板

```markdown
# ADR-066 验收 3 性能测试报告

- 日期：YYYY-MM-DD
- 测试人：
- 环境：E3D 3.1 + ams7997

## 测试配置

| 项目 | 基线（关） | 优化（开） |
|------|-----------|-----------|
| 环境变量 | AIOS_MODEL_TRANSFORM_CHEAP_PATH=0 | AIOS_MODEL_TRANSFORM_CHEAP_PATH=1 |
| 窗口 | ams7997 270→271 | ams7997 270→271 |
| 根数 | 9 根 BRAN | 9 根 BRAN |

## 测试结果

| 指标 | 基线 | 优化 | 省时比例 |
|------|------|------|---------|
| Task 总时长 | ___ s | ___ s | ___ % |
| 生成器 CPU 时间 | ___ s | ___ s | ___ % |
| 网格重算次数 | ___ | 0 | - |
| transform 单元数 | 0 | ___ | - |

## 结论

- [ ] 省时比例 ≥ 30%，**通过**，建议翻默认
- [ ] 省时比例 < 30%，**不通过**，维持默认关

## 备注

___
```

---

## 8. 风险与注意事项

1. **服务重启**：每次切换开关需要重启服务，确保环境干净
2. **窗口唯一性**：确保测试窗口没有其他并发修改
3. **缓存影响**：第一次运行可能有冷缓存，建议预热后再测试
4. **测量误差**：多次测量取平均值，减少随机误差
5. **磁盘 I/O**：确保磁盘 I/O 不是瓶颈（使用 SSD）

---

## 9. 相关文件

- 验证清单：`docs/evidence/2026-10-14-adr066-live2-verification-checklist.md`
- ADR-066: `docs/adr/ADR-066-*.md`
- 开关定义：`src/options.rs` (`model_transform_cheap_path`)
- 测试脚本：`scripts/e3d/adr066_live2_*.mac`
