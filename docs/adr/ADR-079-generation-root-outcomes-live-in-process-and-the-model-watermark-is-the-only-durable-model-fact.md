# ADR-079：生成根的成败只活在进程里，模型水位是模型面唯一的持久事实（`gen_root` 表退役）

- 状态：Accepted（2026-09-15。用户 16:17 拍板原话：「gen-roots 的信息不应该存表，放到内存里就够了。如果程序终止，就消失就好了，
  不要存在数据库里。数据库的同步我们以水位为准就行，同步了一半的，我们清理之后，重新增量或者初始化就行。」代码已接线，
  见 `docs/plans/2026-09-15-retire-gen-root-table-dev-plan.md`）
- 日期：2026-09-15
- 取代：ADR-054「模型完成凭证走文件刻度」里**逐根凭证行**那一半（`gen_root.source_end_sesno` / `published_model_target`）；
  ADR-056 P2-2「未触根凭证前移」（`UPDATE gen_root SET source_end_sesno = T`）；ADR-060 D6 / D7「模型水位对账 / 幽灵模型水位」；
  ADR-060 D10「兼容接管」里按 `legacy_adopted` 跳过恢复任务那一格（接管播种本身保留）；ADR-076 D8「按凭证缺口排 ModelOnly」改为
  按**水位缺口**
- 修订：ADR-058 D4 / D5（启动退化窗口的左端与一级过滤：从「安定凭证下限」改为 `model_sesno`）；ADR-072 D4 / ADR-071 D4
  （模型水位的写法不变，写下的前提从「无未安定 `gen_root` 行」改为「本任务的根全部落库」）；ADR-077 D5（初始化死信不再往库里写痕）；
  2026-09-06 计划 P3「目录级联排工」的过滤器从逐根落款改为**库级**落款
- 引用：ADR-060（模型水位是被写下的承诺）、ADR-071（库是任务结果的同步序列化）、ADR-072（写入标记 + 回退到水位）、
  ADR-073 D3（未完成写入 → 整库重初始化）、ADR-076（增量任务以落盘为边界）

## 背景

`gen_root` 是模型面的逐根状态表：每个生成根一行，记 `status` / `publication_status` / `desired_revision` / `published_revision` /
`source_end_sesno` / `published_model_target.catalogue`（目录落款）/ `last_error`。它同时扮演了五个角色：

1. **回执**：一根落库成功后 `UPSERT … publication_status='ready'`，落库适配器每笔事务的收尾段；
2. **失败留痕**：生成失败 / 判死 `UPSERT … status='failed', last_error=…`；
3. **规划输入**：窗口规划器读它当 `roots_S`，`carry_unsettled_generation` 从它找「还欠着的根」；
4. **启动证据**：`startup_coverage_floors` 全表 `GROUP BY` 算「安定凭证下限」，`read_model_watermark_probe` 三趟全库扫算
   「有证据支撑的模型水位」，比行上记的高就下调（ADR-060 D6 / D7）；
5. **目录落款**：`published_model_target.catalogue` 逐根记「照哪一版目录算的」，目录版本核对靠它点名、过滤、排工。

代价：全 AMS 12 万根时一致性普查整表一扫 4–6 s、每 30 s 一次（于是要一份 20 s 缓存）；`read_model_watermark_probe` 7997 实测
avg 5.5 s；每根落库多一笔 `UPSERT`；回退、清库、删库都要多删一张表；`fn::gen_root_*` 12 个巡检函数只服务它。

而 ADR-071 / ADR-072 / ADR-076 之后，这五个角色里**没有一个需要持久化**：

- 库里模型面唯一要回答的问题是「这个库的模型追平到哪一版」——`dbnum_watermark.model_sesno` 已经是那个答案，而且它是任务收尾
  只在本任务的根全部落库时才盖的**承诺**（ADR-060 / ADR-072 D4），不是从逐根行算出来的派生量；
- 「上一进程里哪一根失败了」对恢复没有用：`model_sesno` 没盖到 T，`model_sesno → T` 这一窗的差分里本来就有那根；
  写入标记停在 `writing` 就整库重初始化（ADR-073 D3）。没有一条恢复路需要逐根的痕；
- 逐根的成功事实已经有宿主（`ModelMemoryStore::RootReceipt`，按哪一版源 / 哪一份 `ModelTarget` 生成）；
- 目录落款说的是「这个库的模型照哪一版目录算」——一个库的全部根共用同一份 `ModelTarget`，逐根记它就是把一个数抄几万遍。

用户 2026-09-15 的口径正是这一点：生成根的信息放内存就够，进程终止就消失；库的同步以水位为准，同步了一半的清理掉重来。

## 决策

1. **`gen_root` 表退役，连同 `root_dependency` 与 `fn::gen_root_*` / `fn::sync_gen_roots` / `fn::sync_root_covers` 12 个函数。**
   `resource/surreal/gen_root.surql` 从内置快照里摘掉；存量库由启动的一次性迁移 `retire_publish_run_tables` 用
   `REMOVE TABLE IF EXISTS` / `REMOVE FUNCTION IF EXISTS` 删（幂等）。落库适配器每笔事务不再有回执收尾段；回退（`render_rollback`）、
   整库清空（`wipe_dbnum_rows`）不再点名它。
2. **逐根成败进进程内账本 `data_interface::model_root_ledger`。** 每根一条 `{ noun, outcome: Generated | Failed{error} | Refused{error},
   source_end_sesno }`，一把 `RwLock`，进程全局。写者：落库适配器（全部笔提交后 `record_generated`，只对写全局库的目标
   `PersistTarget::record_ledger`）、任务执行体（`mark_generation_failed` → `record_failed`）、初始化模型半边（两 lane 排空后逐根
   `record_generated_str` / `record_failed_str`）、`DeleteCleanup` / 删子树（`forget_root`）、回退 / 整库清空 / 删库（`forget_dbnum`）。
   读者：窗口规划器（`chasing_roots` 并进 regen、从 advance / lazy 摘掉）、启动 / ModelOnly 规划（`chasing_entries` 无条件重排）、
   一致性判决（`census_all` 的三格）、`/model/ensure` 回执（`entry` 的 `status` / `last_error`）、按需 ensure 的凭证判据。
   **它不是第二份真值**：进程终止即消失，这是有意的。
3. **模型面唯一的持久事实是 `dbnum_watermark.model_sesno`**（ADR-060 的承诺，写法不变）。由它派生的三条改法：
   - 窗口规划器的 `roots_S` = `DbSet@S` 上枚举出的根集本身，`S = model_sesno`——「S 时刻每个根都已发布」正是那枚水位承诺的东西，
     不再读任何一行凭证；「安定」= S 端根集减去账本里还欠着的根（`settled_base_roots`）；
   - 「未触根凭证前移」（ADR-056 P2-2）退役：未被波及、两端都是根的根什么都不做，「这份几何到 T 仍有效」由任务收尾盖到 T 的
     `model_sesno` 一并承诺；`advance` 名单只进回执（`CredentialAdvanceReceipt::from_plan`，字段名与形状不动——跨仓契约）；
   - 启动核对（`startup_reconcile`）只比两枚水位：`applied_sesno > 0 ∧ model_sesno < applied_sesno` 才排 ModelOnly，左端
     `model_sesno`（`0` = 整库重排）。`startup_coverage_floors` / `backfill_gen_root_dbnum_on` / `read_model_watermark_probe` /
     `reconcile_model_watermarks`（ADR-060 D6 / D7 的下调与幽灵清零）/ `legacy_adopted` 判定全部退役——库里没有逐根的证据可拿来
     对水位，水位就是承诺本身；上调只由任务收尾一笔做，回退由写入标记 / 回退到水位做。
4. **目录落款收成库级一份**：`dbnum_watermark.model_catalogue_sessions = [{ dbnum, session }, …]`，「这个库的整份模型与该目录到这一版
   一致」。写法是**合并**（`task_end::render_catalogue_stamp_merge`：点名的目录换新值、其余原样留），只在 `all_roots_ok` 时随
   `model_sesno` 同一笔写；有资格说这句话的任务：
   - 整库重生成——初始化（`dbnum_initialize`：全部根的 `ModelTarget.catalogue` 取最旧）、起点为 0 的 ModelOnly（启动核对「从没追平过」、
     `/model/rebuild`；`batch_worker::task_catalogue_stamps` 从生成成功的根的投影回执取）；
   - 目录级联排的任务（带着 `DiscoveredBatch::catalogue_stamps` 入队，`X → T_X`）：随行名单上的根全部在本任务重算成了才盖，
     **不**从回执反推（免得把同期也前移了、却还没核对过的另一个目录库一并盖掉）。
   普通增量窗口只重算被触到的根，说不了这句，不写。
5. **目录版本核对的过滤器改为库级**（`model_coverage`）：`published_catalogue_sessions_on` 从水位行取最旧落款点名过期目录库；
   `libraries_behind_catalogue(X, T_X)` = 落款里记着 `X` 且 `session < T_X` 的设计库。反查命中的根所在库落款仍旧 → 排它（名单 + 落款随任务）；
   库落款已到 T → `already_current`；退化目录库波及的库把 `DbSet@T` 上的全部根都排；落款仍旧却一根没被命中的库排一条**只带落款的空任务**
   （`stamp_only`），任务收尾把落款盖到 T——不盖的话下次核对又点名它。`lazy_roots` 那一格分不出来了，恒 0（字段留着不动 `/health` 形状）。
6. **按需 `ensure` 的凭证判据**（`model_coverage::root_credential_covers`）= 库级承诺（`model_sesno` 不早于文件最新 ∧ 库级目录落款不早于
   各目录文件最新）∨ 根级按需生成（账本里 `Generated` 且 `source_end_sesno` 不早于文件最新），账本里悬着失败的根一票否决；摄入形态下
   仍与投影回执那一半（`memory_root_current`）相与。
7. **一致性判决**：四格里的「已追平 / 还在赶 / 已放弃」读账本（零 I/O，普查缓存与作废口一起退）；`model_sesno == applied_sesno ∧ 无还在赶
   ∧ 无已放弃` = 一致，否则**落后**——`NotJudgedReason::NoGenerationRoots`（「表空不算清单」）退役，重启后账本为空时四格只剩水位能说的话。
8. **恢复口径**（用户原话）：库的同步以水位为准。任务中途死掉 → 写入标记 `writing` → 整库 `Reinitialize`（ADR-073 D3）；正常收尾但模型没成
   → `model_sesno` 不盖 → 下一次该库任务 / 启动核对按 `model_sesno → 文件最新` 重排。都不需要知道上一进程里哪一根曾经失败。

## 结果

- 每根落库少一笔 `UPSERT`，每个 dbnum 少一张要回退 / 清空 / 删除的表；一致性普查、模型门、启动核对全部零表扫描。
- `/dbnums` 的「已追平 / 还在赶 / 已放弃」在重启后从零起算，直到本进程跑过该库的任务——它们本来就是本进程的工作记录；
  「一致 / 落后」的判决不受影响（只看水位）。
- `/model/ensure` 回执不再有逐根的 `desired_revision` / `published_revision` / `desired_target` / `published_target`；
  `publication_status` 改由投影 ACK（`ready` / `stale`）与账本（`failed` 带 `last_error`）合成；`root_summaries` 多了 `source_sesno` /
  `published_via` / `published_geometry_count`。
- `/health.startup.no_credential_dbnums` 的含义从「没有 `gen_root` 行的库」变为「模型水位从没盖过的库」；`candidates` 从「有凭证记录的库数」
  变为「当前 MDB 声明的 DESI 库数」。
- `CatalogueCascadeReport` 多一格 `stamp_only`；`already_current` 改为按库判。
- 旧运维脚本 `scripts/Invoke-GenRootSweep.ps1` / `Import-SweepNdjson.ps1`（读写 `fn::gen_roots_todo` / `fn::gen_root_report`）失去目标表，
  随本 ADR 作废。

## 回滚

恢复 `gen_root.surql` 进内置快照、落库适配器的回执收尾段（`render_receipt_with_target`）、`model_watermark.rs` 探针与
`startup_coverage_floors`，把 `model_root_ledger` 的读者改回读表；`model_catalogue_sessions` 一列留着无害。回滚前要确认没有部署已经跑过
`retire_publish_run_tables` 删表——删过的库回滚后逐根凭证是空的，等价于「全部根按需懒生成」。

## 实现状态（2026-10-27 追记）

> 依据：`docs/specs/incr-gen-audit/spec.md` R-B-17、「Implementation Decisions」第 8 条与 `review.md` §2.2（审计按 2026-10-27 工作树）。
> 本节只追记，决策 6 原文不改。

- **T-L-1 · 决策 6 的按需凭证「回执半边等值判」已被 ADR-089 改为 `NotBehind` 问法（已修订 → ADR-089）。**
  `Generated` 仍参与按需凭证的根级半边（`root_credential_covers`，`model_coverage.rs:83-108`）；账本模块头「不参与任何
  放行判据」（`model_root_ledger.rs:18-19`）与此不符，以 ADR-089 为准。`NotBehind` 的代码落点本次未复核。

