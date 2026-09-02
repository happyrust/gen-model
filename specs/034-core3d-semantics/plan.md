# 034 新版 pdms-io 的 Core3D 元素语义层实施计划

> 决策依据：`docs/adr/ADR-055-pdms-io-v2-core3d-semantics.md`（Q1–Q8 已拍板，全采推荐项）。
> 完整拷问记录与阶段展开：`docs/plans/pdms-io-v2-core3d-alignment.md`。
> 实施位置：引擎侧在 `d:\work\plant-code\pdms-io-fork-engine-v2`（`pdmsdb_engine_v2` 所在
> 工作副本，当前分支 `codex/occ-retire-paged-session`，HEAD `348d187`，领先 gen-model
> 钉的 rev `13a17e1f` 两个提交）；gen-model 联动在本仓（P5）。

## Constitution Check

- **I 水位是承诺**：本规格全程只读（Q8 写侧冻结），不碰 `applied_sesno`、暂存窗口、
  持久补偿队列。P5 的 gen-model 联动只换「分类/遍历从哪来」，direct 模式的时点语义
  仍按 ADR-053 Q3 pin `applied_sesno`；`open_at(sesno)` 正是把这条承诺下沉到引擎，
  让「pin 时点」与「读最新」共用一条实现而不是两套时钟。
- **II 一条规则只有一份实现**：这是本规格的立身之本。noun 位分类、三模遍历、
  significant 攀爬当前在 gen-model 参考模型、（将来的）db4、E3D 现场三处各有形态；
  落地后权威实现只在两处且分工明确——可执行参考模型（共享 crate `core3d_model`，
  单一来源，gen-model 与 pdms-io 都依赖它）当契约，db4 `Core3dSemantics` 当生产实现，
  oracle 把两者钉在一起。gen-model 的 `DbElement` 门面是薄封装，禁止本地再实现判据
  （P5 验收项）。`NounBitSource` 快照/FFI 两实现对同一接口，1931 noun 全等测试钉住。
- **III 静默失效零容忍**：快照 `core_sha256` 校验不过**加载报错不回落**；多 extent
  在补齐前**显式报错并点名文件**（替代 gen-model 现在的静默回落 legacy 全文件读）；
  页大小断言不过报错；「字段未登记 = 位为假」是 Core3D 原语义，但未登记命中必须可统计，
  不许无声吞掉。mode 2 死代码实现但 `#[doc(hidden)]`，不给生产调用方留静默入口。
- **IV 队列任务三条出路**：不新增任何持久队列 action。`model_update_pending` 的
  attempts / revision / 死信裁决不动（Q6：调度语义留在 gen-model）。
- **V 标识只用真值**：`ref0 → dbnum` 仍走 `cata_closure` 定位器（跨库跳转必须过它，
  不得用 `RefU64::get_0()` 顶替）；noun 位真值只来自钉了 `core_sha256` 的快照或
  core.dll FFI 现取，不凭类型名猜；`DbKind` 对齐 `DB_DB::type` 的数值语义而非库名后缀。
- **VI 不变量由可执行的守护看住**：每条 R 规则先是 C 用例再是实现——`core3d_oracle`
  以可执行参考模型为期望值，C 编号用例数据驱动；「非 significant 子节点挡住 significant
  孙节点」（R11 最易实现错的一条）单独用例钉死；快照被篡改必须转红；17 个真库文件的
  页大小回归、双 extent 夹具、`NavDirection` 五方向 round-trip 各自有测试。
  FFI 全等（需装 E3D）按仓规用 `#[ignore]` live 测试补，前置条件写进测试名。

**运行环境**：Windows / PowerShell / nightly，禁 `cargo clean`；pdms-io 侧改动
经上游提交 + gen-model 升 rev 消费，开发期允许 `scripts/Toggle-LocalDeps.ps1` 本地
重定向，**不得带本地 patch 推 main**（pre-push 守卫已有）。

## Complexity Tracking

1. **跨仓交付**：语义层主体在 pdms-io（独立 git 仓、独立工作副本），gen-model 只能经
   「升 rev」消费，一次特性横跨两仓四次握手（实现 → 上游提交 → 升 rev → 联动改造）。
   无法避免：引擎本来就是上游 crate（`pdmsdb_engine_v2` git 依赖），把语义层塞回
   gen-model 会违反原则 II（第二套判据实现）。缓解：P0–P4 全部只动 pdms-io 侧、
   P5 一次性升 rev 收口；开发期 Toggle-LocalDeps 重定向，验收前必须钉回正式 rev。
2. **FFI oracle 依赖装着 E3D 的机器**：`CoreDllBits`（core.dll 现取）与 C 用例的
   FFI 对拍进不了 CI。无法避免：这是「位表跨版本会漂」的唯一自证口径（核对表 R0-2）。
   缓解：CI 跑快照口径（纯函数），FFI 全等为 `#[ignore]` live 测试并记
   `docs/2026-08-12_live-test-ledger.md` 台账。
3. **实施工作副本带着他人未提交改动**（`pdms-io-fork-engine-v2` 有 27 个已修改文件，
   属 occ-retire-paged-session 在飞工作）。缓解：P0 只**新增**文件（`crates/core3d_model/`、
   `compare/core3d_oracle.rs`）加两处最小注册改动（根 `Cargo.toml` members、
   `compare/mod.rs`，均不在已修改清单里）；不 revert、不 rebase、不代为提交，
   分支归属由用户在提交时裁决。

## 前置事实修正（相对 docs/plans 拷问稿）

拷问稿按 rev `13a17e1f` 写；实施工作副本已领先两个提交，其中 **P4 的两条根因已落地**：

- `348d187 fix(db): interpret header page length as words`——文件头 `0x34` 按 4 字节字
  解释（顺带 `legacy_oracle.rs` 更名 `fixture_oracle.rs`）；
- `cb7dd95 fix(paged): reject false page-size session matches`——探测器拒绝假匹配。

P4 相应改为「回归验证 + 补 `open_at(sesno)`」，不重复实现。

## 实施阶段与阶段门

### P0 · `core3d_oracle`：先把尺子做出来

共享 crate `crates/core3d_model/`（参考模型迁入 + `NounBitSource` 快照实现）、
`compare/core3d_oracle.rs`（oracle 驻点 + `CoreDllBits` FFI 实现）、C 用例夹具。

**阶段门**：快照与 FFI 两实现对 1931 noun 全等（live）；`core_sha256` 篡改加载报错
（CI 可跑）；C 用例在参考模型上全绿；`cargo check` 全 workspace 过。

### P1 · db4 元素语义层（主体）

`db4/core3d.rs` 新增 `Core3dSemantics`：`is_valid`(R3) / `db_type`(R1) / `climb`(R2) /
`is_significant`·`is_primitive`(R0-1) / `significant_owner`(R14) / `members`(R11) /
`exists`(R26)。members 显式栈 LIFO、收集与下潜两个独立闭包、返回迭代器不物化。

**阶段门**：oracle 驱动 C 用例对 db4 实现全绿；「非 significant 子挡 significant 孙」
独立用例过；每个公开函数 doc 注释带 R 编号回引。

### P2 · CE 导航栈补齐

`db4/ce.rs`：`NavDirection` 驱动 `navigate(dir)`，`DSAVE`/`DRESTO` 对齐的
`save_position`/`restore_position`，`owner_chain()` 迭代器承载 `climb`。

**阶段门**：深度 N 子树遍历 `record_pages_read` 与元素数同阶（不随栈深二次增长）；
五方向各一 round-trip 用例。

### P3 · db2 库类型与 extent

`db_lookup` 暴露 `DbKind`（DESI==1）；`extract.rs` 解析 extent 链、db1 按
`(extent, pgno)` 定址；补齐前多 extent 显式报错点名文件。

**阶段门**：双 extent 夹具跨 extent 定位解析成功；gen-model `on_demand_db.rs` 的
legacy 回退具备删除条件（P5 执行）。

### P4 · 页大小与会话时点（收尾）

回归验证 348d187/cb7dd95：17 个已知骗过探测器的真库文件不给 hint 读出 2048 与权威
sesno；补 `open_at(path, sesno)` 与「读最新」共用实现。

**阶段门**：17 文件回归用例进 `tests/engine_v2_read_real_db.rs` 且全绿；
`open_at` 与 `PdmsIO::search_latest_refno(_, Some(sesno))` 对同一 (refno, sesno) 同结果。

### P5 · gen-model 联动（升 rev 收口）

gen-model 依赖升 rev；`src/data_interface/core3d_reference.rs` 删除改为 re-export
共享 crate；`DbElement` 门面薄封装转调 db4；`generation_root` 名单判定接位表
（R9 口径：加层不换判据）。

**阶段门**：`direct_attmap_probe` 复跑 8000/7333 零真值冲突；`tests/model_impact.rs`
对账结论不变；gen-model 内 `rg` 搜不到第二份 significant/primitive 判据实现；
Toggle-LocalDeps 已钉回正式 rev。
