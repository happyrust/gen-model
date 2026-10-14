# 真机验证快速指南（2026-10-14）

- 目的：在 E3D TTY 中执行 live #2（Issue #36）与 A6/A7（Issue #50）验证
- 前置：gen-model 服务已启动，ams7997 库可用

---

## 第一部分：Issue #36 — Live #2 真机验证（刚体前移）

### 步骤 1：确认服务与开关

```powershell
# 在 PowerShell 中执行
# 1. 确认服务健康
curl http://127.0.0.1:8022/api/v1/health

# 2. 确认 ams7997 在同步状态
curl http://127.0.0.1:8022/api/v1/dbnums | ConvertFrom-Json | Where-Object { $_.dbnum -eq 7997 }
```

**预期**：返回 `in_sync: true`

### 步骤 2：在 E3D TTY 中执行挪动宏

打开 E3D 3.1，进入 TTY（命令行界面），执行：

```text
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"
```

**这个宏会做**：
1. 选中 ZONE 24381/103249
2. 东移 1000mm（POS X + 1000）
3. 执行 SAVEWORK

### 步骤 3：等待增量更新完成

```powershell
# 在 PowerShell 中轮询，直到 in_sync
do {
    Start-Sleep -Seconds 5
    $status = curl http://127.0.0.1:8022/api/v1/dbnums | ConvertFrom-Json
    $ams = $status | Where-Object { $_.dbnum -eq 7997 }
    Write-Host "ams7997: $($ams.state) (model_sesno=$($ams.model_sesno))"
} until ($ams.in_sync -eq $true)
```

**预期**：120 秒内 `in_sync: true`

### 步骤 4：验证回执（关键）

```powershell
# 查询 ZONE 103249 的回执，确认走的是 transform 路
curl "http://127.0.0.1:8022/api/v1/model/records?root=24381/103249" | ConvertFrom-Json | ConvertTo-Json -Depth 10
```

**检查点**：
- [ ] 回执里有 `kind == "transform"` 的单元
- [ ] transform 单元的记录数 ≈ 237（9 根 BRAN，含隐式管身）
- [ ] **没有** `kind == "regen"` 的单元

### 步骤 5：验证 RVM 对比（真值）

```powershell
# 先导出基线（在挪动前执行，如果还没做）
# 在 E3D TTY 中：
#   $M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_before.mac"

# 挪动后导出真值（在 E3D TTY 中）：
#   $M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_after.mac"

# 对比（在 PowerShell 中）
python D:/work/plant-code/old/gen-model/scripts/rvm_verify.py `
  D:/work/plant-code/old/gen-model/output/e3d-live2/zone_103249_before.rvm `
  D:/work/plant-code/old/gen-model/output/e3d-live2/zone_103249_after.rvm
```

**检查点**：
- [ ] 所有构件的位移 = (1000, 0, 0) mm
- [ ] 网格形状不变（只是平移）

### 步骤 6：性能测试（验收 3）

**6.1 基线测试（开关关）**

```powershell
# 1. 还原 ZONE（在 E3D TTY 中）
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"

# 2. 等待 in_sync
# ... 轮询直到 in_sync ...

# 3. 关闭开关（重启服务或设置环境变量）
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "0"
# 重启 gen-model 服务

# 4. 记录启动时间
$baseline_start = Get-Date

# 5. 在 E3D TTY 中再次挪动
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"

# 6. 等待完成并记录时长
$baseline_end = Get-Date
$baseline_duration = ($baseline_end - $baseline_start).TotalSeconds
Write-Host "基线 Task 时长: $baseline_duration 秒"
```

**6.2 优化测试（开关开）**

```powershell
# 1. 还原 ZONE（在 E3D TTY 中）
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"

# 2. 等待 in_sync
# ...

# 3. 打开开关
$env:AIOS_MODEL_TRANSFORM_CHEAP_PATH = "1"
# 重启 gen-model 服务

# 4. 记录启动时间
$opt_start = Get-Date

# 5. 在 E3D TTY 中再次挪动
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"

# 6. 等待完成并记录时长
$opt_end = Get-Date
$opt_duration = ($opt_end - $opt_start).TotalSeconds
Write-Host "优化 Task 时长: $opt_duration 秒"

# 7. 计算省时比例
$saved = ($baseline_duration - $opt_duration) / $baseline_duration * 100
Write-Host "省时比例: $saved %"
```

**通过标准**：省时比例 ≥ 30%

### 步骤 7：还原环境

```powershell
# 在 E3D TTY 中还原 ZONE
$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"

# 等待 in_sync
# ...

# 恢复环境变量
Remove-Item Env:AIOS_MODEL_TRANSFORM_CHEAP_PATH
```

---

## 第二部分：Issue #50 — A6 全语料对拍 + A7 性能测试

### 步骤 1：设置环境变量

```powershell
# 在 PowerShell 中执行
$env:DB_OPTION_FILE = "D:\work\plant-code\old\gen-model\db_options\DbOption-aps.toml"
$env:AIOS_MODEL_UNIT_LEVEL_PUBLISH = "1"
$env:AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE = "1"
$env:AIOS_MODEL_CABLE_ENDPOINT_CASCADE = "1"
$env:AIOS_MODEL_DESIGN_REFERENCE_CASCADE = "1"
```

### 步骤 2：运行 A6 全语料对拍

```powershell
cd D:\work\plant-code\old\gen-model

# 单分片运行（2400 秒预算，约 40 分钟）
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture
```

**或者多分片并行（推荐，6 个分片）**：

```powershell
# 分片 0/6
$env:AIOS_UNIT_PUBLISH_SHARD = "0/6"
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture

# 分片 1/6
$env:AIOS_UNIT_PUBLISH_SHARD = "1/6"
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture

# ... 以此类推到 5/6
```

**通过标准**：报告中 `diff == 0`（一根 DIFF 即红）

### 步骤 3：查看报告

```powershell
# 报告路径
Get-Content D:\work\plant-code\old\gen-model\docs\evidence\2026-09-17-unit-publish-parity-corpus.md | Select-Object -Last 50
```

**关注指标**：
- `SUMMARY_JSON` 行：`{"windows": ..., "roots_checked": ..., "parity_ok": ..., "diff": 0, ...}`
- 每窗的耗时分布（确认缓存命中后没有引入额外开销）

### 步骤 4：A7 性能测试

A7 的性能数据已包含在 A6 探针报告中：
- 每根 unit 路耗时
- 每根 regen 路耗时（对照）

**通过标准**：单元路总时延 ≤ regen 路总时延（净收益为正）

---

## 第三部分：验证完成后

将结果告诉我，我会帮你：
1. 翻默认值 `model_transform_cheap_path`（Issue #36）
2. 翻默认值 `model_unit_level_publish`（Issue #50）
3. 更新 ADR 状态行与 changelog
4. 关闭对应的 GitHub Issues

---

## 附录：常用 E3D TTY 命令

| 命令 | 说明 |
|------|------|
| `$M "path/to/macro.mac"` | 执行宏文件 |
| `=24381/103249` | 选中元素 |
| `Q CE` | 查询当前元素 |
| `SAVEWORK` | 保存工作 |
| `EXIT` | 退出 E3D |

## 附录：常用 PowerShell 命令

| 命令 | 说明 |
|------|------|
| `curl http://127.0.0.1:8022/api/v1/health` | 健康检查 |
| `curl http://127.0.0.1:8022/api/v1/dbnums` | 查看所有库状态 |
| `curl "http://127.0.0.1:8022/api/v1/model/records?root=24381/103249"` | 查询根的回执 |
| `cargo test --lib <name> -- --ignored --nocapture` | 运行被忽略的测试 |
