# 结构连接级联（JOIS/JOIE ↔ CREF）翻默认前置清单

- 日期：2026-10-14
- 关联：Issue #34、Issue #31（父）、Issue #33（阻塞）、ADR-056、联动对齐计划 C2
- 目的：明确 `model_structural_connection_cascade` 翻默认所需的全部前置条件和验收门

---

## 1. 背景

`model_structural_connection_cascade` 开关默认关。今天几何上不显（e3d-model 第 4 阶段 SCTN 关节负体 / 端部切割未接）；第 4 阶段一落地，改一端不重算对端就是几何错误。

**当前状态**：
- 开关默认关（`src/options.rs`）
- 代码已实现（`src/data_interface/structural_connection.rs`）
- 被外部前置阻塞：e3d-model 第 4 阶段 SCTN 关节负体 / 端部切割未落地
- Blocked by #33（核查跨库结构连接）

---

## 2. 前置条件清单

### 2.1 外部前置（阻塞项）

| # | 前置项 | 状态 | 说明 |
|---|--------|------|------|
| E1 | **e3d-model 第 4 阶段落地** | ⏳ 待定 | SCTN 关节负体 / 端部切割接上 e3d-model，几何上才显出正确性缺口 |
| E2 | **Issue #33 核查完成** | ✅ 已关（09-22） | 结论：跨库对端读得到（12/12 回指成立）、生产口径上卷不到（0/12）→ 由此才有跨库交接（前置 2） |

### 2.2 内部前置（可准备项）

| # | 前置项 | 状态 | 验收标准 |
|---|--------|------|---------|
| I1 | **跨库可读验证** | ✅ 已验证（2026-10-14 复跑） | `live_probe_cross_db_structural_connection_edges`：12/12 读到、拆账 `far_end_cross_db=12 / far_end_without_root=0`、跨库交接 8 库 18 根（合并进 refactor 后真库重跑通过） |
| I2 | **两真库窗口两态对拍** | ⬜ 待验证 | `aps7200_0001` 3→4、`aps250110_0001` 29→30：对端根全进 `regen` |
| I3 | **九窗两态一致性** | ⬜ 待验证 | 九个钉死窗口桶位不变（开关开态） |
| I4 | **源码钉守卫** | ✅ 已存在 | `window_root_plan.rs` 中的级联守卫测试 |
| I5 | **单元测试覆盖** | ✅ 已存在 | `cargo test --lib -- structural_connection` 全绿 |

### 2.3 文档前置

| # | 前置项 | 状态 | 输出物 |
|---|--------|------|--------|
| D1 | **前置清单成文** | ✅ 本文档 | `docs/plans/2026-10-14-structural-connection-cascade-default-on-checklist.md` |
| D2 | **ADR 更新** | ⬜ 待更新 | ADR-056 或新增 ADR 记录翻默认决策 |
| D3 | **CONTEXT.md 词条** | ⬜ 待更新 | 更新「结构连接级联」词条 |
| D4 | **联动对齐计划改口** | ⬜ 待更新 | `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` C2 |

---

## 3. 验收门（翻默认前必须全部通过）

### 3.1 代码级验收

```bash
# 1. 开关开着复跑两真库窗口
cargo test --lib -- structural_connection -- --nocapture

# 2. 验证对端根全进 regen
cargo test --lib -- probe_structural_connection_cascade_windows

# 3. 九窗两态一致性（开关开态）
cargo test --lib -- live_ams8000_pinned_windows_land_in_the_expected_buckets

# 4. 全量测试
cargo test --lib
```

**检查点**：
- [ ] `aps7200_0001` 3→4：对端根全进 `regen`
- [ ] `aps250110_0001` 29→30：对端根全进 `regen`
- [ ] 九个钉死窗口桶位不变
- [ ] 全量测试绿（2472+ passed, 0 failed）

### 3.2 真机级验收（第 4 阶段落地后）

**测试场景**：
1. 在 E3D 中打开 `aps7200_0001` 或 `aps250110_0001`
2. 修改一端的 SCTN（如移动一个关节 FIXING）
3. 执行 SAVEWORK
4. 观察对端梁的端部切割是否跟着变

**验证方式**：
- 截图对比（修改前 vs 修改后）
- RVM 导出对比（端部切割几何逐分量相等）

**检查点**：
- [ ] 改一端 SAVEWORK 后，对端梁的端部切割跟着变
- [ ] 截图 / RVM 对比通过
- [ ] plant-ui 显示正确

---

## 4. 翻默认步骤（第 4 阶段落地后执行）

### 4.1 代码修改

```rust
// src/options.rs
// 修改前
fn effective_model_structural_connection_cascade(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(false)  // 改为 true
}

// 修改后
fn effective_model_structural_connection_cascade(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)  // 默认开
}
```

### 4.2 文档更新

| 文件 | 修改内容 |
|------|---------|
| `src/options.rs` | 更新注释（默认关 → 默认开）|
| `CONTEXT.md` | 更新「结构连接级联」词条 |
| `docs/adr/ADR-056-*.md` 或新增 ADR | 记录翻默认决策 |
| `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` | C2 改口 |
| `changelog.md` | 添加变更记录 |
| `DbOption.toml` 模板 | 更新注释 |

### 4.3 测试验证

```bash
# 更新默认值测试
cargo test --lib -- options::tests::structural_connection_cascade

# 全量测试
cargo test --lib
```

---

## 5. 时间线

```
当前（2026-10-14）：完成前置清单文档（本文档）
    ↓
e3d-model 第 4 阶段落地（外部前置 E1）
    ↓
完成 Issue #33 核查（外部前置 E2）
    ↓
执行内部前置 I1-I3 验证
    ↓
真机级验收（3.2）
    ↓
翻默认值（4.1-4.3）
    ↓
关闭 Issue #34
```

---

## 6. 风险与注意事项

1. **外部阻塞**：第 4 阶段未落地前，翻默认无意义（几何上不显）
2. **跨库核查**：#33 未完成前，跨库场景不确定
3. **向后兼容**：翻默认后，关着时行为与今天逐位相同（已有守卫测试）
4. **性能影响**：级联会增加窗口规划的计算量，需要评估全语料影响

---

## 7. 相关文件

- 开关定义：`src/options.rs` (`model_structural_connection_cascade`)
- 级联实现：`src/data_interface/structural_connection.rs`
- 窗口规划：`src/data_interface/window_root_plan.rs`（`expand_touched` C2）
- ADR-056: `docs/adr/ADR-056-*.md`
- 联动对齐计划：`docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md`
- Issue #33: 核查跨库结构连接
- Issue #34: 本文档对应的 issue
- Issue #31: 父追踪清单

---

## 8. 待办事项（本文档完成后）

- [ ] 提交本文档到 git
- [ ] 更新 Issue #34，链接本文档
- [ ] 更新 Issue #31（父），标记 #34 前置清单已完成
- [ ] 等待 e3d-model 第 4 阶段落地
- [x] ~~等待 Issue #33 核查完成~~（2026-09-22 已关）
- [x] I1 跨库可读验证（2026-10-14 合并后复跑通过）
- [ ] 执行内部前置 I2-I3 验证
- [ ] 执行真机级验收
- [ ] 翻默认值
- [ ] 关闭 Issue #34
