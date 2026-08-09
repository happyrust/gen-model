# 当前增量更新三链路审核：kv-mem 计算与 RocksDB 写回（2026-08-08）

## 1. 审核基准与范围

- 工作区：`D:\work\plant-code\old\gen-model`
- HEAD：`e0adaa1b`（`codex/increment-staging-closure`，审核时含当前 working tree）
- 合约：`docs/adr/ADR-017-staged-increment-window-commit.md`
- 范围：数据增量、模型 `Transform/DeleteCleanup/RegenRoot`、房间增量、journal 写回、watermark/pending/attempt、提交后空间收敛。
- Oracle MCP：
  - 主审会话 `increment-kvmem-rocksdb-current`（GPT-5.6 Sol，completed）。
  - 主审提出的“房间输入链未证明走 staging”经本仓逐函数追踪复核，结论不成立；取证见 §4.2。
  - 定点复核会话 `room-read-route-adjudicati` 已启动，但浏览器会话长时间无 UI 进展；本报告不以该未完成会话作证据。

本次只审核，不修改生产源码。

## 2. 总结论

当前实现可以通过以下口径：

> 默认稳态增量中，数据与模型数据面先在每窗口 kv-mem database 内计算并写 journal；模型失败会阻断窗口；成功的房间结果跟随窗口写回，失败房间任务转 durable pending；journal 分块重放后，以一个尾事务发布 watermark、pending/revision/attempt 与空间意图。

不能使用以下强口径：

> 数据、模型、房间全部完成后，以一个 RocksDB 原子事务一次写回，且 watermark 前持久层零写入。

原因有四项：

1. 写回是多个 `TX_CHUNK` 事务加一个尾事务，不是一个 RocksDB 事务；不经 watermark 门控的读者可见短暂中间态。
2. 房间是 best-effort：失败不会阻断数据/模型窗口，而是随尾事务保留 pending。
3. `prepare_attempt`、窗口阻断、队列等控制面允许在提交前直写持久层。
4. 基线/冷启动以及 `GEN_MODEL_DIRECT_INCREMENT` 紧急路径不走 kv-mem。

在所审代码与现有自动测试中，未发现新的 P0/P1 数据一致性缺陷。发现一个实际 P2 配置缺陷，以及一个尚未关闭的 release-evidence 缺口。

## 3. 当前成立的端到端链路

### 3.1 数据增量

1. 新窗口先在持久层窗口前态上构建固定模型计划，再写入 `increment_update_attempt`。这是崩溃重放的固定输入，不是 staging read escape（`increment_pipeline.rs:591-641`、`model_update_plan.rs:685-725`）。
2. 活动窗口存在时，PE 与反向索引语句逐条走 `StagingWriteContext::execute(..., Both)`，先在 mem 生效并进入 journal（`increment_pipeline.rs:664-698`）。
3. staged 路径只登记 `StagedFinalize`，不在解析阶段推进 watermark（`increment_pipeline.rs:749-783`）。

### 3.2 模型增量

1. 模型数据面统一入口 `execute_model_write` 在 staged context 中路由到 `ExecMode::Both`，上下文缺席才直写 `SUL_DB`（`surreal_retry.rs:88-98`）。
2. Transform 指针更新已走该入口（`increment_manager.rs:2363`）；生成、布尔、AABB、房间边写均使用同一写路由。
3. 窗口先按窗口前态解析并排序持有全部受影响生成根锁，锁随窗口存活（`batch_worker.rs:416-465`、`write_context.rs:162-182`）。
4. 任一模型根失败或模型前置失败会废弃窗口、保持 watermark 不动；成功根才从 staged finalize plan 中结算（`batch_worker.rs:623-681`、`:1578-1668`、`:1766-1813`）。

### 3.3 房间增量

1. staged 房间轮运行在 `window.scope(...)` 中；该 scope 同时安装 staging read 与 write task-local（`batch_worker.rs:762-771`、`lifecycle.rs:381-397`）。
2. `load_room_panel_map_from_pe` 明确查询 `active_data_db()`（`room_model.rs:498-524`）。
3. `load_panel_index` 调 `staging::query_valid_insts`；后者查询 `active_data_db()`（`room_model.rs:1220-1268`、`staging/mod.rs:36-72`）。
4. `ElementRoomHistory::load` 明确查询 `active_data_db()`（`room_model.rs:1284-1324`）。
5. PanelIndex 不完整时元素分支 fail-closed；持久层房间轮还建立缺面板覆盖屏障与修复根（`room_model.rs:1171-1188`、`model_update_pending.rs:1750-1825`）。
6. 同一 refno 多实例按实例逐个取候选并 union/stronger，不再 `.next()` 或覆盖丢失（`room_model.rs:1354-1414`、`:1468-1541`）。
7. room round 的 `Failed` 会压过 `MoreWork`，不会立即热循环烧尽 attempts（`batch_worker.rs:1998-2017`、`:2043-2078`、`:2206-2229`）。

### 3.4 写回与恢复

1. `StagedExecutor::commit_to` 先按 `TX_CHUNK=500` 分块重放 journal，再执行尾事务（`staging/executor.rs:171-233`）。
2. 尾事务顺序为：窗口语句、未完成模型/房间 pending、空间 intent + epoch、regen revision 条件结算、watermark、attempts/恢复记录清除（`model_update_pending.rs:586-617`）。
3. 写回失败时进程内无限退避重放同一 journal；进程崩溃时 watermark 未推进，按固定 attempt 重建窗口。
4. spatial intent 与 watermark 同事务登记；提交后立即收敛，且每次领取下一批前再次收敛，失败即停止出队；房间轮也检查 durable spatial backlog（`batch_worker.rs:196-219`、`:853-895`、`:2112-2125`、`side_effect_pending.rs:233-313`）。

## 4. Oracle 结论的本仓裁定

| Oracle 候选 | 本仓复核 | 裁定 |
|---|---|---|
| `model_update_plan` 的显式 `SUL_DB` 是 read escape | 计划在 PE staged persist 之前构建，注释和调用顺序都明确要求窗口前态；纯位姿不改 OWNER | 驳回 |
| staged room topology / panel / history 未证明走 staging | 三个 loader 最终都命中 `active_data_db()`，且调用包在 `window.scope` | 驳回 |
| ReplaySafe 仍可能接受动态 SELECT 写目标 | `UPDATE (SELECT ...)` 已由 AST 目标校验拒绝并有测试（`replay_safe.rs:338-347`） | 驳回；带 WHERE 的表级集合写仍需逐调用审计 |
| `GEN_MODEL_DIRECT_INCREMENT=0` 仍启用直写 | 实现只判断变量是否存在 | 采纳，见 P2-1 |
| 分块 replay 对非 watermark 读者可见中间态 | 与代码和 ADR-017 §4 一致 | 采纳为设计限制，不是新 bug |

## 5. 当前 findings

### P2-1：`GEN_MODEL_DIRECT_INCREMENT=0` 会误启用紧急直写

> **2026-08-09 已修**：`direct_increment_enabled` 收口到 `direct_increment_flag`，
> 只认明确真值（`1/true/yes/on`，忽略大小写与首尾空白）；unset、空串与明确假值
> （`0/false/no/off`）一律关闭，认不出的值按关闭处理并告警一次。纯函数测试
> `only_explicit_truthy_values_enable_direct_increment` 覆盖三类输入并钉住
> 环境入口不许再出现裸 `is_some()`。

**证据**：`batch_worker.rs:352-369` 的注释承诺 `=1`，实现却是：

```rust
std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").is_some()
```

**触发序列**：部署模板显式注入 `GEN_MODEL_DIRECT_INCREMENT=0` → `is_some()==true` → `use_staged_increment_window` 返回 false → 稳态增量绕过 kv-mem。

**影响**：操作者以为关闭紧急开关，实际恢复旧直写语义；数据/模型在窗口计算期间即可对共享读者可见。

**最小修复**：只接受明确真值（建议 `1/true/yes/on`，大小写与空白归一），其余值和 unset 均为 false；非法非空值启动时告警。

**最小测试**：纯函数测试覆盖 unset、空串、`0`、`false`、`1`、`true` 与混合大小写；另钉 `increment_mode()`。

### P2-2：P5 live 验收门仍未完全关闭

**证据**：开发方案仍明确记录 T5.1–T5.5 的 live 隔离性、终态逐表对拍、故障注入与性能基线尚未落地（`docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md:63-68,88`；`docs/2026-08-07_three-chain-audit.md:109-121`）。当前已有精简 parity harness 和三个 ignored E2E 入口，但本次没有可用 E3D/fork live fixture 证据来执行它们。

**影响**：现有单元/内存集成测试能证明路由和幂等性质，却仍可能漏掉真实 fork/RocksDB、文件、进程崩溃与外部读者交互问题；2026-08-07 的 Transform 直写泄漏就是此类测试缺口的历史例证。

**最小闭环**：把 `staged_transform_e2e`、`staged_regen_e2e`、`staged_pane_replay_probe` 接入可重复 fixture；至少产出写回前持久层 diff、直写/暂存终态逐表 diff、chunk 中断重放、tail 失败重试、进程重启 spatial recovery 五份记录。

## 6. 接受但不能误称“原子”的限制

1. **可见性原子 ≠ RocksDB 事务原子**：分块 replay 期间，不使用 watermark/revision gate 的 viewer、材料表或 UI 可见新旧混合状态。若业务要求所有现有读者都完全不可见，只能推进 ADR-017 phase-2 服务器 overlay/原子 commit，或给所有读者补门控协议。
2. **房间不是 hard gate**：成功结果在 kv-mem 内随窗提交；失败结果以 durable pending 延后。因此“房间全部完成后才写回”不成立。
3. **控制面提前持久化**：attempt、window block、queue control 等是允许的恢复元数据；“watermark 前 RocksDB 零写入”不成立，正确口径应限定为数据/模型/房间数据面。
4. **并非所有入口 staged**：`start_sesno <= 1` 的基线/冷启动和紧急直写开关是明确豁免。
5. **journal 不是 redo log**：当前 ReplaySafe validator 与逐调用审计共同保证重放收敛；带 `WHERE` 的集合写仍不能仅因通过 validator 就自动视为语义安全。

## 7. 验证记录

本次在当前 working tree 上执行：

```text
cargo test --lib data_interface::staging:: -- --test-threads=1
59 passed

cargo test --lib data_interface::model_update_pending:: -- --test-threads=1
40 passed, 12 ignored

cargo test --lib fast_model::room_model:: -- --test-threads=1
27 passed, 3 ignored

cargo test --lib data_interface::batch_worker::tests -- --test-threads=1
26 passed

cargo test --lib -- --test-threads=1
544 passed, 79 ignored
```

## 8. 建议顺序

1. 先修 P2-1，消除“配置写 0 反而绕过 staging”的操作风险。
2. 再关闭 P5 live 验收门；这比继续增加源码字符串断言更能发现跨仓/跨进程泄漏。
3. 明确对外口径：**默认稳态数据与模型 hard-gated staged；房间 best-effort staged；写回由 watermark 发布而非一个 RocksDB 原子事务。**
4. 逐个审计共享读者是否真正以 watermark/revision 为可见性门；若没有，ADR-017 phase-1 的“秒级残余”就是实际用户可见风险，不应只留在文档里。
