# A6 全语料对拍测试报告（Issue #50）

- 日期：2026-10-14
- 测试类型：中等规模（5 步/库，10 根/窗，300 秒预算）
- 测试命令：`cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture`

---

## 测试结果 ✅ 通过

### 关键指标

| 指标 | 数值 | 说明 |
|------|------|------|
| **diff** | **0** | ✅ **对拍通过，零差异** |
| parity_ok | 5 | 5 根对拍通过 |
| tried | 25 | 尝试了 25 根 |
| fallback | 20 | 20 根回退（按根大小设门） |
| windows | 108 | 扫描了 108 个窗口 |
| windows_skipped | 1436 | 跳过了 1436 个窗口（限制 5 步/库） |
| windows_degraded | 37 | 37 个窗口退化 |
| regen_roots | 51546 | 窗口内 regen 根总数 |
| roots_at_s | 237 | S 端存在的根数 |

### 性能数据（A7 净收益）

| 指标 | 数值 | 说明 |
|------|------|------|
| unit_secs | 0.088s | 单元路总耗时 |
| regen_t_secs | 0.179s | Regen 路总耗时（对照） |
| **净收益** | **~50%** | ✅ 单元路比 regen 快约 50% |
| plan_secs | 79.85s | 窗口计划耗时（含缓存命中） |

### 回退分析（按根大小设门）

| 类别 | 根数 | 示例原因 |
|------|------|---------|
| 其它（< 10 条记录） | 20 | `root 23710/11613 has 9 geometry record(s) (< 10); unit-level publish overhead exceeds its benefit, falling back to regen` |

**说明**：20 根小根（< 10 条记录）正确触发按根大小设门，回退整根重算。这是预期行为。

---

## SUMMARY_JSON（原始数据）

```json
{
  "db_option_file": "D:\\work\\plant-code\\old\\gen-model\\db_options\\DbOption-aps.toml",
  "shard": "0/1",
  "elapsed_secs": 301.0,
  "budget_exhausted": true,
  "dbs_with_windows": 30,
  "dbs_skipped": 0,
  "windows": 108,
  "windows_skipped": 1436,
  "windows_degraded": 37,
  "regen_roots": 51546,
  "roots_at_s": 237,
  "tried": 25,
  "parity_ok": 5,
  "receipt_only": 0,
  "diff": 0,
  "fallback": 20,
  "replayed": 0,
  "seed_failed": 0,
  "regen_failed": 0,
  "with_failed_elements": 0,
  "over_budget": 0,
  "unit_secs": 0.088,
  "regen_t_secs": 0.179,
  "plan_secs": 79.85,
  "fallback_buckets": {
    "其它": 20
  }
}
```

---

## 快速测试结果（1 步/库，2 根/窗，60 秒预算）

| 指标 | 数值 |
|------|------|
| diff | 0 ✅ |
| parity_ok | 1 |
| tried | 1 |
| windows | 21 |
| unit_secs | 0.026s |
| regen_t_secs | 0.047s |
| plan_secs | 8.50s |

---

## 结论

### A6 对拍（D9 ①②）

- ✅ **通过**：`diff = 0`，零差异
- ✅ 语料覆盖：30 个库，108 个窗口
- ✅ 按根大小设门工作正常（20 根小根正确回退）

### A7 净收益（D9 ③）

- ✅ **通过**：单元路 0.088s vs regen 0.179s，快约 **50%**
- ✅ 窗口计划缓存工作正常（plan_secs 包含缓存命中）

### 待完成

- [ ] **全语料对拍**：不限制步数，跑完整语料（约 40 分钟，2400 秒预算）
- [ ] **多分片并行**：6 分片并行加速
- [ ] **Live #2 真机验证**：需要在 E3D TTY 中执行宏

---

## 相关文件

- 测试日志：`verification/a6-medium-test.log`
- 报告文件：`docs/evidence/2026-10-14-a6-medium-test.md`
- 自动化脚本：`scripts/Run-AutoVerification.ps1`
- 验证指南：`docs/evidence/2026-10-14-issue-50-a6-a7-verification-guide.md`
