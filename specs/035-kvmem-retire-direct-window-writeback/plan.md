# 035 kv-mem 暂存层退役实施计划

> 决策依据：`docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md`（D1–D8 全采推荐项，共识 d-52）。
> 阶段展开与风险：`docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` §4–§6。
> 基线：`docs/evidence/2026-09-02-kvmem-retire-baseline.md`（1388 = 1300 绿 / 8 红 / 80 ignored；五窗 `unexplained=0`）。
> 实施位置：本仓 `d:\work\plant-code\old\gen-model`（当前分支 `codex/libgm-primitive-caliber`，HEAD `6580a339`，
> 工作树有 85 个他人在飞的已修改文件，其中含 `src/data_interface/batch_worker.rs` / `src/data_interface/increment_pipeline.rs` /
> `src/data_interface/model_refresh.rs` / `src/data_interface/model_update_pending.rs` / `src/data_interface/staging/mod.rs` /
> `src/data_interface/staging/write_context.rs` / `src/data_interface/staging/lifecycle.rs` / `src/data_interface/staging/ancestor_preload.rs`）；
> P2-5 在 `../vendor/e3d-model`；P3 的读路由在 `../vendor/old-aios-core`；P4 的收集器在 `../vendor/e3d-io`。

## Constitution Check

- **I 水位是承诺**：直写路径的纪律原样保留——TX_CHUNK 分块、任一块失败水位不动、整窗口按持久化的固定区间
  重放（`load_attempt` → `validate_prepared_attempt` → 重放）；`finalize_attempt` 仍是唯一推进 `applied_sesno`
  的地方且在窗口语句批全部成功之后。模型侧从水位解绑是 ADR-025 §7 / ADR-054 已定口径的落地，不是新放松：
  与水位共命运的副作用（durable 模型意图、空间意图）仍在尾事务里；只能单独提交的（`ref_rev`）走
  `SideEffectCompensator` 持久补偿队列，今天直写路径已如此。
- **II 一条规则只有一份实现**：本规格的目标之一。P1 后数据窗口只有一条写法；P2 后选根只有一份判据来源
  （e3d-model `plan_update`，旧 `model_impact` 降为 oracle）；P4 后变更检测只有一份（N6）。
  D10 的车道判定 `batch_needs_exclusive_lane` 仍是手动与 watcher 共用的一处谓词；根枚举 `enumerate_generation_roots`
  是 `/model/ensure` direct 分支与窗口选根共用的一处判定（N7）。
- **III 静默失效零容忍**：删掉的每条暂存分叉都有对应的「不含 `active_staging_writes`」源码断言接替，
  不是删测试；`AIOS_STAGING_WINDOW_MAX_SESSIONS` 改名后旧名被设置时**响亮告警并沿用其值**（P5 再删别名），
  不静默忽略部署里的配置；写回被持久层确定性拒绝仍以 `window_block` 记终态并带原始错误；凭证前移的三道
  护栏（`unresolved` 非空放弃、`only_e3d_model` 桶不前移、`../vendor/e3d-model/tests/increment_real.rs` 新门）任一触发都记
  `credential_advance_degraded`，不无声退化。
- **IV 队列任务三条出路**：`model_update_pending` 不新增 action，attempts / revision / 死信裁决不动；
  `run_staged_non_regen_work` 退役后位姿 / 删除 / 级联工作项一律经 `drain_non_regen_report` 消费（今天直写路径
  已是）。**新增一种补偿任务** `enqueue_cata_ref_rev`（D8-A 提前到 P1）——三条出路在 T126 里逐条给出：
  可消费（`SideEffectCompensator::drain` 的过滤器恰好覆盖它）、可收口（成功删行；`missing > 0` 记 warning 不算失败）、
  可复活（同 `enqueue_ref_rev` 的 `MAX_ATTEMPTS` 通道，新窗口到来重新入队即清零）。`src/data_interface/staging/attempts.rs` 的 per-root attempts /
  `window_block` 是持久层控制面，随文件搬家不随目录删。D9 之后进 `model_update_pending` 的只有受影响根，
  「可消费」判据不变（action 集合没变）。
- **V 标识只用真值**：`S = 提交前 applied_sesno`、`T = 窗口 end_sesno` 显式传入 `plan_update`，不再
  `start − 1` 推算（审核 S6）；`ref0 → dbnum` 仍走 `cata_closure` 定位器。
- **VI 不变量由可执行的守护看住**：P1 每删一段分叉翻一条源码断言；崩溃重放对拍在拆 `src/data_interface/staging/parity.rs` 之前先有
  直写替身（T171）；P2 的 `touches_roots` / 凭证前移 / `Reparented` 两端根各有纯函数单测，真库门在
  `../vendor/e3d-model/tests/increment_real.rs`；live 结果记 `docs/2026-08-12_live-test-ledger.md`。

**运行环境**：Windows / PowerShell / nightly，禁 `cargo clean`；`CARGO_TARGET_DIR=D:\Rust\target` 共享；
vendor 改动经上游提交 + 升 rev 消费，开发期 `scripts/Toggle-LocalDeps.ps1` 重定向，**不得带本地 patch 推 main**。

## Complexity Tracking

1. **宪法「附加约束 · 并发模型」段写死了 `STAGED_COMMIT_SERIAL` 与「并发仅限稳态 DESI 暂存窗口」**。
   ADR-056 使这段失真。无法避免：这是被取代的架构本身。缓解：P1 只改名 `DATA_COMMIT_SERIAL` 并按 D10-A
   让全部数据批次独占（行为与今天 `direct_emergency` 逐字节相同，不新增并发面）；P5 以 PATCH 修订宪法
   该段（动机 = ADR-056，受影响 ADR = ADR-011 / ADR-017，迁移 = 无代码项）。
2. **工作树带着他人未提交改动，且正好压在本规格要改的文件上**（`src/data_interface/batch_worker.rs` 在 +9822/−2770 那批里）。
   缓解：P1 改动按阶段单独分支（R6）；开工前与在飞改动的作者对齐或等其落地；改文件前 `git diff --stat`
   确认所动区间不与在飞 hunk 重叠；不 revert、不 rebase、不代为提交。
3. **两个 before 回执要用户侧环境**（8009 SurrealDB 长驻 + E3D 驱动宏）。缓解：基线 §3 给了 A/B/C 三条
   命令与待填表；P1 源码改动可先行，P1 验收第 2 条等回执补齐后再关。
4. **跨仓交付**（P2-5 e3d-model、P3 aios_core、P4 e3d-io）：与 034 相同形状，缓解同 034 计划 Complexity 1。
5. **计划文档同日有两位作者**：2026-09-02 12:06 二轮分析把 F10 / N7 / D9（eager 范围）/ P2-7 写进
   `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md`，与本 spec 的并发车道决策同时诞生，编号撞车——
   本 spec 的车道决策改号 **D10**，ADR-056 的 D9 / N7 追记由二轮那一侧补。改同一份文档前先看 `LastWriteTime`、只做定点替换。

## 前置事实修正（相对 docs/plans 计划文档 §4 P1 表）

- `AIOS_STAGING_WINDOW_MAX_SESSIONS` 不在 `src/options.rs`，在 `src/data_interface/batch_worker.rs:340–354`
  （`window_session_budget` / `effective_window_session_budget`）；`AIOS_STAGING_{WARN,REFUSE_ABSORB,ABANDON}_{BYTES,ROWS}`
  在 `src/data_interface/staging/resources.rs:44–49`（随 P3 目录删除），`src/options.rs` 里没有任何 `AIOS_STAGING_*`。
- `batch_reroutes_to_initial_load`（`src/data_interface/batch_worker.rs:1371`）唯一消费点是开窗判定（`:1542`），其 doc 明写
  「不替执行体拍板，执行体自己还会复核一次」——ADR-021 回退重建的权威在 `execute_one_dbnum` 里。
  计划表写「保留」，实际应**随开窗判定一起删除**（T104），ADR-021 语义不受影响。
- 今天 `increment_mode` 的两个标签是 `"staged"` / `"direct_emergency"`（`src/data_interface/batch_worker.rs:1350` `increment_mode_for`），
  不是 `"direct"`；P1 收成 `"direct"` 是**改值**，`web/ops.html` 与监控若按旧值匹配要同批改。
- `model_refresh::apply_window` 的策略是 staged → `Required`、直写 → `BestEffortFallback`（`:120–124`）；
  `generate_roots_report` 同形（`:269–273`）。P1 固定为 `BestEffortFallback` 即今天直写值。
- 现有 e2e 脚本 `scripts/Run-Issue7E2E.ps1:52`、`scripts/Start-AiosDatabaseManual.ps1:22` 都钉
  `GEN_MODEL_DIRECT_INCREMENT='1'`；P1 删掉该环境变量后这两行成为死设置，要同批删除（T173）。

## 实施阶段与阶段门

### P0 · 冻结基线与决策落地（已完成大半）

ADR-056 ✅；基线 evidence ✅（单测计数、五窗对拍）；issue7 / e2e-8009 两份 before 回执 ⏸ 用户侧；
P0-3 S8 度量 ⏸ 用户侧（E3D SAVEWORK）。

**阶段门**：ADR 落文件；基线 evidence 落文件；两个数字有了（S8 与 e2e 回执可与 P1 源码改动并行补）。

### P1 · 数据面：直写成为唯一路径

`src/data_interface/batch_worker.rs` 删开窗与写回、删 `execute_frozen_batch_body` 两大 staged 块与 `staged` 变量、
删直写开关族、`STAGED_COMMIT_SERIAL → DATA_COMMIT_SERIAL`、车道按 D10-A；`src/data_interface/increment_pipeline.rs` 的 `apply_one`
只留直写四步；`src/data_interface/model_refresh.rs` 策略固定 + `prepare_required_dependencies` **整个删除**（D8-A 提前到 P1，
二轮修订）+ 新补偿任务 `enqueue_cata_ref_rev`；`src/surreal_retry.rs` 三个写入口去掉 staging 路由；
`src/data_interface/staging/mod.rs` 的 `active_data_db` 恒返 `SUL_DB`；`/health` 去 `staging_windows` / `staging_commit`；护栏测试翻转；
直写版崩溃重放对拍替身。

**阶段门**：成功标准 1、2；日志无「暂存窗口 / journal / 写回」；`/health.increment_mode == "direct"`。

### P2 · 模型面：e3d-model 差分接到选根位置，凭证前移

计划文档 §4 P2-1 … P2-7（P2-1 根集从文件枚举 `enumerate_generation_roots`，N7；P2-7 eager 只对受影响根 + 启动复核
改口径，D9）。**阶段门**：成功标准 3。

### P3 · 拆除 kv-mem 暂存基础设施

计划文档 §4 P3 表。**阶段门**：成功标准 4；`tests/staged_*` 三个文件删除前 T171 替身已绿。

### P4 · 收集器换底座 old-pdms-io → e3d-io（可与 P3 并行启动）

计划文档 §4 P4：影子模式对拍 → 切换。**阶段门**：成功标准 5。

### P5 · 文档与口径收口（随各阶段滚动）

计划文档 §4 P5 + 宪法「并发模型」段 PATCH 修订（按 D10 结论）+ 删 `AIOS_STAGING_WINDOW_MAX_SESSIONS` 别名。
**阶段门**：成功标准 6。
