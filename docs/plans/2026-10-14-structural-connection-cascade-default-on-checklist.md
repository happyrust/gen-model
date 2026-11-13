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
| E1 | **e3d-model 第 4 阶段落地** | ⚠️ 部分落地（2026-10-15，vendor main `a7ec940`） | SCTN 负体链路已接通（`elmodl.rs`：JOIS/JOIE→mode 2、成员链 FITT/CMFI→3·SBFI→6·SNOD→SJOI→1·SCOJ→2·SUBJ→1、TMPL 属主→mode 7、宿主 FIXING 扫描、`attachment_angles` 直段臂、`ImplicitFrame::SectionNode`）。真库实测：aps7200 对拍 137 条变化全在 SCTN——4 根真减体（/5907、/5910 各 −6125 mm³；/8622、/8626 各 −206753 mm³，成员链 mode-3/6 + 宿主扫描），133 根记账；aps250110 46 条全记账。**关键发现**：两个语料里的 SJOI/PJOI 全无 TMPL/SPRE 几何源——其端切语义在 `CUTP`/`CUTB` 属性上（DB_Sjoi 负体 builder 内部未翻），现按 skipped 记账、对 RVM 可观测；若 E3D 真对这些关节出端切，缺口显在这 416+70 条记账上。**未闭**：SAVEWORK/RVM 对拍验证 E3D 是否真切 |
| E2 | **Issue #33 核查完成** | ✅ 已关（09-22） | 结论：跨库对端读得到（12/12 回指成立）、生产口径上卷不到（0/12）→ 由此才有跨库交接（前置 2） |

### 2.2 内部前置（可准备项）

| # | 前置项 | 状态 | 验收标准 |
|---|--------|------|---------|
| I1 | **跨库可读验证** | ✅ 已验证（2026-10-14 复跑） | `live_probe_cross_db_structural_connection_edges`：12/12 读到、拆账 `far_end_cross_db=12 / far_end_without_root=0`、跨库交接 8 库 18 根（合并进 refactor 后真库重跑通过） |
| I2 | **两真库窗口两态对拍** | ✅ 已验证（2026-10-15） | `live_aps_one_ended_windows_pull_the_far_end_roots_into_regen`：aps7200 `94/94` 回指、对端 6 根全进 regen（关6/开8）；aps250110 `371` 边、对端 31 根全进（两态 258） |
| I3 | **九窗两态一致性** | ✅ 已验证（2026-10-15） | `live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off`：九窗两态逐字相同（本批窗口无边） |
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
# 1. 级联单元测试与守卫
cargo test --lib -- structural_connection -- --nocapture

# 2. 两真库窗口：对端根进 regen（aps7200 3→4 / aps250110 29→30）
cargo test --lib -- live_aps_one_ended_windows_pull_the_far_end_roots_into_regen

# 3. 九窗两态一致性（开关开/关各一态，钉死桶位）
cargo test --lib -- live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off

# 4. 跨库边探针（12 条跨库边、交接账）
cargo test --lib -- live_probe_cross_db_structural_connection_edges

# 5. 全量测试
cargo test --lib
```

**检查点**：
- [x] `aps7200_0001` 3→4：对端根全进 `regen`（2026-10-15，关6/开8，6 根对端全进）
- [x] `aps250110_0001` 29→30：对端根全进 `regen`（2026-10-15，两态 258，对端 31 根全在）
- [x] 九个钉死窗口桶位不变（2026-10-15，两态逐字相同）
- [x] 全量测试绿（2026-10-15，gen-model 2492 passed / 0 failed；e3d-model lib 366 passed / 0 failed）

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
- [x] ~~等待 e3d-model 第 4 阶段落地~~（2026-10-15 部分落地：SCTN 负体链全通、真库实测 4 根减体；SJOI/PJOI 的 CUTP/CUTB 端切语义仍待 DB_Sjoi 逆向 + RVM 对拍，缺口已在 negatives_skipped 记账可观测）
- [x] ~~等待 Issue #33 核查完成~~（2026-09-22 已关）
- [x] I1 跨库可读验证（2026-10-14 合并后复跑通过）
- [x] 执行内部前置 I2-I3 验证（2026-10-15，两窗两态对拍 + 九窗钉死全过）
- [ ] 执行真机级验收
- [ ] 翻默认值
- [ ] 关闭 Issue #34

---

## 9. 2026-11-11 复核：两个方向的真库证据、重算范围的量法、语料门还差什么

背景：e3d-model 把 SCTN / GENSEC 的关节负体接齐了（10-15 起；11-10 `036e89d` 主关节 PJOI 的朝向照 Core3D 现算，三本库
48 根 `/MDS/FT54…` 杆的切口跟着变）。ADR-090 D2（10-27）定了 C2 要翻开、前面有一道语料门。这一节是门里离线做得了的那部分。

**普查**（`cross_root_reference_census`，AMS + APS 两个工程 390 个 DESI 文件；09-07 那次是五个工程 486 库）。结构连接边里
回指成立、对端最近的根不在变更端根链上、且在 `STRU` 下的 3,128 条：FIXING ↔ GENSEC 2,050、SJOI ↔ SCTN 624、
PJOI ↔ SCTN 406（`PJOI.CREF → SCTN` 33，`SCTN.JOIS / JOIE → PJOI` 373）、FIXING ↔ STWALL 48；`SUBJ`、`CMPF` 为 0。
正式后端监听的五个库（7997 / 7998 / 8000 / 251095 / 251137）与 MDS 支架库（7324 / 7329 / 7350）都是 0——结构连接边两端同根。
跨库的结构连接边仍是 APS 那 12 条（6 对 `FIXING.CREF ↔ GENSEC.JOIE`）。

**嵌套根的口径**（读代码）：外层根整根重算时，内层根名下的记录也一起重算（`records_of_root` 按祖先链取）。所以上面
「对端最近的根不在变更端根链上」只在外层根走单元级发布、只重建点名的单元时才是真缺口；外层根整根重算时，只有两端根链
完全没有交集的才是。后者在假定源那五本库里只有 aps7200 的 SJOI ↔ SCTN（18 / 10 / 8 条），PJOI 一条都没有。

**假定源，两个方向**（`structural_connection::tests::live::live_a_changed_joint_pulls_its_beam_into_regen_and_a_changed_beam_pulls_its_joints`）。
五本库钉死会话，5,110 条会级联的同库边逐条假定持属性端单独改：开关开，对端属主链上每一个根都在 `regen`；开关关，`regen`
恰好是变更端自己链上的根；开着只多出对端根。关着时对端最近的根不在 `regen` 里的边数逐类钉成表，与普查逐格相同。
单条边开着最多多出 3 根（对端属主链上的 SBFR / FRMW / STRU）。

**自然窗口，反方向**（`live_aps_windows_where_only_the_beam_changed_pull_the_joint_roots_into_regen`）。梁那一端单独改、
又没退化的窗语料里只有 `aps250209_0001` 15→16 与 16→17，开关关着关节的根也全在 `regen`（两窗各重排 373 根，库里一共 382 根，几乎是全库，证明力有限）。

**重算范围放大多少**（`live_measure_how_much_the_cascade_widens_regen_on_real_windows`，手动）。20 个库各取会话链最后 12 步共
130 窗：开关开着**没有一窗多出根**（`regen` 合计 10103 / 10103），读了 8,459 条边，规划耗时量不出差别。APS 五本结构库整条
会话链 38 窗同样是 0（2601 / 2601）。原因：自然窗口里结构连接的两端要么都在同一窗里变了，要么对端已由 e3d-model
`plan_update` 自己的同库关节级联（FIXING ↔ GENSEC、带目录正体的 SJOI ↔ SCTN）点名，要么整窗退化。

**ADR-090 D2 的语料门**

| 门 | 状态 | 依据 |
|---|---|---|
| (i) 前半：改关节后被截杆所在根进 `regen` | ✅ 规划器层 | 上面两条真库测试 |
| (i) 后半：几何与整库重建逐条相等 | ❌ 没做 | 要一窗「只改关节」的真实会话。语料里只改 FIXING / SJOI 的有，只改 PJOI 的没有；另见下面单元级发布那一条 |
| (ii) 纯戳窗 D10 只推回执、端到端不慢于不分桶 | ❌ 没在今天的代码上量，也没按这道门的口径做过 | 三条级联同开的对拍有过两轮：不限预算的全语料是 2026-10-01 的 A6（DIFF 0，receipt-only 673 根），早于 SCTN 关节负体接上（10-15）；2026-10-21 按工程分开、每工程 300 秒预算重跑过一次（`verification/2026-10-14-a6-test-report.md`：AMS 118 根、APS 220 根，diff 0，D10 只推回执 28 / 115 根，整窗单元路快 10.8× / 3.9×；APS 带几何对比的只有 19 根，报告自己还列着「不限预算跑完整语料」待完成）。10-21 那次的 e3d-model 是 `f2a3b88`，带目录正体的 SJOI 切梁端（10-23）、PJOI / SCOJ 目录关节（11-04）、PJOI 判画与朝向（11-06 / 11-10）都在它之后 |

**读代码发现、没有测试证实的一处**。级联只把根并进 `regen`；根怎么重算由单元级发布决定——子计划为空回退整根，不空就只重建
`plan_update` 点名的单元（`UnitDeltaProducer::produce`）。级联拉进来的根如果同一窗里自己也有别的改动，被牵连的那一件不在
子计划里。FIXING / 带目录正体的 SJOI 不受影响（e3d-model 自己点名了梁）；带目录正体的 PJOI 与它同根的梁、对端根另有
改动的 PJOI / SUBJ / CMPF 受影响。刚体前移与 D10 都有「级联目标根不走便宜路」的规则，局部重建这一档没有。门 (i) 后半要过
就绕不开它。

**翻开的连带后果**。三条级联都开着，ADR-080 D10 才分桶；今天缺省配置下 D10 一次都没生效过。翻 C2 等于同时让 D10 在
生产上生效。
