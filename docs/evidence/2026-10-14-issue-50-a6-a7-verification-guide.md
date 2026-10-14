# Issue #50 验证运行指南（A6 全语料对拍 + A7 性能测试）

- 日期：2026-10-14
- 关联：Issue #50、ADR-080 D9 验收门 ①②③
- 目的：指导在真机环境执行 A6 全语料对拍与 A7 性能测试

---

## 1. 前置条件

### 1.1 环境要求

| 项目 | 要求 |
|------|------|
| E3D 环境 | E3D 3.1 已安装，模板目录可访问 |
| 真库文件 | `D:\AVEVA\Projects\E3D3.1\` 下五个目录的库文件 |
| MDB 配置 | `DB_OPTION_FILE` 指向正确的 `DbOption.toml` |
| 服务状态 | aios-database 服务已启动（或使用探针独立运行） |

### 1.2 环境变量

```powershell
# MDB 配置文件
$env:DB_OPTION_FILE = "D:\work\plant-code\old\gen-model\db_options\DbOption-aps.toml"

# 单元级发布开关（探针需要开着）
$env:AIOS_MODEL_UNIT_LEVEL_PUBLISH = "1"

# 三条级联同开（D10 分桶需要）
$env:AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE = "1"
$env:AIOS_MODEL_CABLE_ENDPOINT_CASCADE = "1"
$env:AIOS_MODEL_DESIGN_REFERENCE_CASCADE = "1"

# 语料目录（默认五个，可自定义）
$env:AIOS_POSREF_DIRS = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000;D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000;D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000;D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000;D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000"
```

---

## 2. A6 全语料对拍（ADR-080 D9 ①②）

### 2.1 验证目标

**D9 ①**：两态对拍全绿 — 同一窗分别走 unit 路与 regen 路，两端投影记录按 `GeometryId` 配对、`ParityInvariants` 全等，且整根 `manifest_hash` 逐字相同。

**D9 ②**：语料覆盖 — 全语料（五个目录），不止 ams8000 一条链。

**判红标准**：一根 DIFF 即红。

### 2.2 运行命令

```powershell
cd D:\work\plant-code\old\gen-model

# 单分片运行（默认，2400 秒预算）
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture

# 多分片并行（推荐，6 个分片）
# 分片 0/6
$env:AIOS_UNIT_PUBLISH_SHARD = "0/6"
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture

# 分片 1/6
$env:AIOS_UNIT_PUBLISH_SHARD = "1/6"
cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture

# ... 以此类推到 5/6
```

### 2.3 可调旋钮

| 环境变量 | 默认值 | 说明 |
|---------|--------|------|
| `AIOS_UNIT_PUBLISH_STEPS_PER_DB` | 0（整条链） | 每库会话链最后 K 步 |
| `AIOS_UNIT_PUBLISH_ROOTS_PER_WINDOW` | 40 | 每窗抽样根数，0 = 全量 |
| `AIOS_UNIT_PUBLISH_BUDGET_SECS` | 2400 | 总预算秒数，到了只数不量 |
| `AIOS_UNIT_PUBLISH_MESH_DIR` | 临时目录 | 烘焙目录 |
| `AIOS_UNIT_PUBLISH_KEEP_MESHES` | - | =1 留着网格文件 |
| `AIOS_UNIT_PUBLISH_REPORT` | `docs/evidence/2026-09-17-unit-publish-parity-corpus.md` | 报告路径，每跑一次追加一节 |
| `AIOS_UNIT_PUBLISH_SHARD` | - | `k/n` 分片，n 个进程并行 |

### 2.4 预期输出

报告文件末尾应有 `SUMMARY_JSON` 行，格式类似：

```json
{"windows": 969, "roots_checked": 3876, "parity_ok": 3876, "diff": 0, "fallbacks": 12, "replayed": 3, "failed": 0}
```

**通过标准**：`diff == 0`

### 2.5 缓存效果验证

本次代码修改（窗口计划缓存）应显著减少重复计算时间。对比指标：

| 指标 | 修改前 | 修改后（预期） |
|------|--------|---------------|
| 每窗 plan_update 次数 | 2 次（规划器 + 执行体各一次） | 1 次（缓存命中） |
| 969 窗重复计算开销 | ~300.5s | ~0s |
| 单窗端到端时延 | 基线 | 应持平或略快 |

在报告中关注每窗的耗时分布，确认缓存命中后没有引入额外开销。

---

## 3. A7 性能测试（ADR-080 D9 ③）

### 3.1 验证目标

**净收益为正**：一窗端到端时延（含 digest 重算与整根落盘）不慢于 regen 路。

### 3.2 测试方法

#### 方法 A：探针内置计时（推荐）

`probe_unit_publish_corpus_against_regen` 已内置计时，报告中会输出：
- 每根 unit 路耗时
- 每根 regen 路耗时（对照）
- 汇总统计

#### 方法 B：手动对比（开关对比）

```powershell
# 基线：开关关（纯 regen 路）
$env:AIOS_MODEL_UNIT_LEVEL_PUBLISH = "0"
# 运行增量更新，记录 Task 总时长

# 优化：开关开（单元级发布）
$env:AIOS_MODEL_UNIT_LEVEL_PUBLISH = "1"
# 运行同样的增量更新，记录 Task 总时长
```

### 3.3 测试窗口选择

| 窗口 | 描述 | 根数 | 预期收益 |
|------|------|------|---------|
| ams8000 249→250 | 管路窗（BRAN 16 件） | 1 根大 BRAN | 单元路 0.02s vs regen 0.07s |
| ams7997 270→271 | ZONE 挪动窗（9 根 BRAN） | 9 根 | 单元路应显著快于 regen |
| aps250209 16→17 | 混合窗（373 根 regen） | 多根 | 大根收益明显，小根走整根路 |

### 3.4 通过标准

- **单元路总时延 ≤ regen 路总时延**（净收益为正）
- 大根（≥ 10 条记录）单元路显著快于 regen
- 小根（< 10 条记录）自动回退整根路（按根大小设门）

---

## 4. 翻默认值（验证通过后）

### 4.1 代码修改

**Issue #50：`model_unit_level_publish` 翻默认**

```rust
// src/options.rs
// 修改前（第 2312-2317 行）
pub fn model_unit_level_publish() -> bool {
    effective_default_off_switch(
        load_ext_fields().model_unit_level_publish,
        std::env::var(MODEL_UNIT_LEVEL_PUBLISH_ENV).ok().as_deref(),
    )
}

// 修改后：改为默认开
pub fn model_unit_level_publish() -> bool {
    effective_default_on_switch(  // 新增：默认开的辅助函数
        load_ext_fields().model_unit_level_publish,
        std::env::var(MODEL_UNIT_LEVEL_PUBLISH_ENV).ok().as_deref(),
    )
}
```

或直接修改 `effective_default_off_switch` 的调用：

```rust
// 找到 effective_default_off_switch 的定义，新增一个默认开的版本
fn effective_default_on_switch(configured: Option<bool>, env_override: Option<&str>) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)  // 默认开
}
```

### 4.2 文档更新清单

| 文件 | 修改内容 |
|------|---------|
| `src/options.rs` | 更新注释（默认关 → 默认开）|
| `CONTEXT.md` | 更新「单元级发布」词条 |
| `docs/adr/ADR-080-*.md` | 更新状态行（A6/A7 通过，D9 翻默认）|
| `changelog.md` | 添加变更记录 |
| `DbOption.toml` 模板 | 更新注释 |

---

## 5. 验证检查清单

### 5.1 A6 全语料对拍

- [ ] 环境变量已设置（`DB_OPTION_FILE`、`AIOS_MODEL_UNIT_LEVEL_PUBLISH=1`、三条级联开）
- [ ] 运行 `probe_unit_publish_corpus_against_regen`（单分片或多分片）
- [ ] 报告中 `diff == 0`
- [ ] 报告已保存到 `docs/evidence/`（按政策不入库，用 `git add -f` 点名）

### 5.2 A7 性能测试

- [ ] 探针报告中单元路总时延 ≤ regen 路总时延
- [ ] 大根（≥ 10 条记录）单元路显著快于 regen
- [ ] 小根（< 10 条记录）自动回退整根路（日志中有回退原因）
- [ ] 缓存命中率 > 95%（每窗 plan_update 只算一次）

### 5.3 翻默认值

- [ ] A6 通过（diff == 0）
- [ ] A7 通过（净收益为正）
- [ ] 修改 `src/options.rs` 翻默认值
- [ ] 更新文档（ADR-080、CONTEXT.md、changelog）
- [ ] 运行 `cargo test --lib` 全绿
- [ ] 提交并推送

---

## 6. 相关文件

- ADR-080: `docs/adr/ADR-080-unit-level-publish-rebuilds-only-changed-units-under-a-root-receipt.md`
- 探针源码：`src/fast_model/e3d_model_service.rs:7430` (`probe_unit_publish_corpus_against_regen`)
- 单根探针：`src/fast_model/e3d_model_service.rs:6464` (`probe_unit_publish_one_root_against_regen`)
- 缓存实现：`src/fast_model/e3d_model_service.rs:183-260` (`WINDOW_PLAN_CACHE`)
- 按根大小设门：`src/fast_model/e3d_model_service.rs:556-565` (`MIN_RECORDS_FOR_UNIT_PUBLISH`)
- 分支：`feat/issues-36-50-34`
