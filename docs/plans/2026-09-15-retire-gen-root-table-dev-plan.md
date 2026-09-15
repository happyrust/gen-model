# `gen_root` 表退役：生成根成败进进程内账本，模型面只留水位——开发计划（ADR-079）

> 来源：用户 2026-09-15 16:17 对 `gen-model-model-cache` 的拍板——「gen-roots 的信息不应该存表，放到内存里就够了。如果程序终止，
> 就消失就好了，不要存在数据库里。数据库的同步我们以水位为准就行，同步了一半的，我们清理之后，重新增量或者初始化就行。」
> 状态：**已落地**（2026-09-15，三段会话接力：fable-5-1-10 → fable-5-1-17 → fable-5-1-23）。代码事实对到 `codex/model-projection-cache@cfed5fcce`
> + 当日在途改动（同一棵树上还有 ADR-078 的项目闭包改动，互不相干）。
> 验证：`cargo check --all-targets` 绿、`cargo clippy --lib` 本仓零告警、`cargo test --lib` **2114 passed / 0 failed / 109 ignored**。
> **未验证**：真库（mem:// 之外的 rocksdb 实例）重启一次看 `retire_publish_run_tables` 删表那一句与启动核对的排队行；plant-ui 对 `/model/ensure`
> 回执少掉的四格有没有消费者。

## 0. 一句话

**逐根的「成了 / 失败了」只活在进程里；库里模型面只留 `dbnum_watermark.model_sesno`（追平到哪一版）与 `model_catalogue_sessions`
（照的哪几版目录）。** 恢复靠两枚水位与写入标记：没盖到 T 就重排 `model_sesno → T`，死在写库中途就整库重初始化。

## 1. 改了什么（按模块）

| 模块 | 之前 | 之后 |
| --- | --- | --- |
| `model_root_ledger.rs`（新） | — | 进程内账本：`record_generated / record_failed / record_refused / forget_root / forget_dbnum / entry / chasing_roots / chasing_entries / census / census_all`，`LazyLock<RwLock<_>>` |
| `model_db_adapter.rs` | 每笔事务收尾 `UPSERT gen_root … publication_status='ready'` + `root_dependency` | 收尾段为空；全部笔提交后 `record_root_generated` 进账本（只对 `PersistTarget::record_ledger` 的目标）；`ensure_root_cached_on` 改问 `inst_relate` 有没有行 |
| `window_root_plan.rs` | `roots_S` 读 `gen_root`；`advance_root_credentials_on` 前移凭证；启动窗口左端取安定凭证下限 | `roots_S` = `DbSet@model_sesno` 枚举；`settled_base_roots` = S 端根集 − 账本欠账；`advance` 只进回执（`CredentialAdvanceReceipt::from_plan`）；`startup_window_base(model_sesno, T)` |
| `increment_tail.rs` | `finalize_attempt` 水位之后 `UPDATE gen_root SET source_end_sesno = T` | 只落尾事务，返回 `()` |
| `task_end.rs` | `TaskModelOutcome { all_roots_ok, dead_roots }` | 多 `catalogue_sessions: Option<BTreeMap<u32,u32>>`，`render_catalogue_stamp_merge` 合并写 `model_catalogue_sessions` |
| `dbnum_state.rs` | — | `DbnumState.model_catalogue_sessions: Vec<CatalogueStampRow>` 随镜像读 |
| `dbnum_initialize.rs` / `model_initial_persist.rs` | 死信 `render_generation_failed` 交模型 lane；根回执随行落库 | 死信只进账本；两 lane 排空后 `record_in_ledger`；全库成时 `catalogue_sessions` 取最旧随收尾盖 |
| `batch_worker.rs` | `mark_generation_failed` UPSERT `gen_root`；`DeleteCleanup` 删 `gen_root:{key}`；`render_generation_failed` | `mark_generation_failed` → `ledger::record_failed`（同步）；`DeleteCleanup` → `forget_root`；`task_catalogue_stamps` 决定收尾盖哪些库级落款 |
| `batch_scheduler.rs` | `DiscoveredBatch / RowMeta / FrozenBatch.regen_roots` | 多 `catalogue_stamps: BTreeMap<u32,u32>`（并入取较新，`merge_catalogue_stamps`）；名单空、落款非空的 ModelOnly 也不被「已覆盖」吞掉 |
| `model_coverage.rs` | `generation_roots_cache_current` 一次主键查 `gen_root`；`model_coverage_current` / `startup_coverage_floors` / `coverage_tracked_dbnums` / `backfill_gen_root_dbnum_on` 全表谓词；`published_catalogue_sessions_on` / `roots_behind_catalogue_on` 读逐根落款 | `root_credential_covers`（库级承诺 ∨ 根级账本，悬着失败否决）；模型门 = 水位 ∧ 账本无欠账；`library_catalogue_stamps_on` / `oldest_catalogue_sessions` / `libraries_behind_catalogue` 读水位行；`enqueue_catalogue_cascade` 按库排、带落款、`stamp_only` 空任务；floors / backfill / tracked 删 |
| `model_watermark.rs` | ADR-060 D6 / D7 探针与下调（三趟 `gen_root` 全库扫） | **整个模块删除** |
| `startup_reconcile.rs` | 凭证下限 + 探针 + `legacy_adopted` 三合一决策 | `startup_model_recovery_plan(applied, model)` 纯函数：`model < applied` 才排，左端 `model`（0 = 整库）；`no_credential_dbnums` = 没盖过模型水位的库 |
| `dbnum_consistency.rs` | `gen_root` 整表一扫 + 20 s 缓存 + `invalidate_census` | 普查读 `ledger::census_all()`；`NoGenerationRoots` 退役（水位不等 = 落后）；缓存与作废口删 |
| `fast_delete.rs` | 回退 / 清库 / 删库都 `DELETE gen_root …` | 不点名它；`rollback_to_watermark` / `wipe_dbnum_for_reinit` / `delete_dbnum_fast` 调 `forget_dbnum` |
| `on_demand_model.rs` | 删子树后 `DELETE type::thing('gen_root', …)` | `forget_root` |
| `handlers.rs` | `/model/ensure` 回执 `SELECT … FROM ONLY type::thing('gen_root', …)` | `publication_state_of`：投影 ACK → `ready` / `stale`，账本失败 → `stale` + `last_error`，都没有按 `model_available` |
| `model_rebuild.rs` | 全量名单 + `model_start_sesno = latest` | `model_start_sesno = Some(0)`（整库重排那一档，收尾盖库级落款），名单不随身 |
| `reverse_reference.rs` | `CatalogueCascadeReport` | 多 `stamp_only: Vec<u32>`；`lazy_roots` 恒 0（字段留） |
| `embedded_surql.rs` / `resource/surreal/gen_root.surql` | 内置快照含 `gen_root.surql`，核验面含 `gen_root_of` / `gen_roots_todo` | 文件删除、快照与核验面摘掉 |
| `lib.rs` | `retire_publish_run_tables` 13 张表 | 15 张表 + 12 个 `fn::gen_root_*` 函数 |

## 2. 设计判据（拍板时定下的）

- **D1 账本不是第二份真值。** 没有一条恢复路读它；重启后为空。`model_sesno` 没盖到 T 时 `model_sesno → T` 的差分里本来就有没做完的根。
- **D2 库级目录落款只由「有资格」的任务盖。** 整库重排从回执取最旧；目录级联任务盖入队时定的 `X → T_X`，不从回执反推——同期前移、
  还没核对的另一个目录库不能被顺手盖掉。普通增量窗口不盖。
- **D3 目录级联按库过滤。** 「落款仍在旧目录上」是库级判据；落款仍旧却没被波及的库排一条空名单任务只为盖落款，否则周期核对永远点名它。
- **D4 一致性判决里「水位不等 = 落后」。** 「表空不算清单」那一档随表退役；重启后四格从零起算是本进程工作记录的性质，不是缺陷。
- **D5 `/model/rebuild` = 起点 0 的 ModelOnly。** 与启动核对「从没追平过」同一档，名单不随身，收尾盖库级落款。

## 3. 验证

- `cargo test --lib`：2114 passed / 0 failed / 109 ignored。新钉：账本 3 条（`model_root_ledger`）、凭证判据纯函数 1 条
  （`a_root_is_current_when_the_library_promise_or_its_own_ledger_entry_covers_the_file`）、库级落款过滤 1 条
  （`libraries_behind_a_catalogue_are_those_still_stamped_before_its_target`）、模型门源码钉 2 条、调度器落款并入 1 条
  （`catalogue_stamps_survive_coverage_merge_and_freeze`）、任务失败进账本 1 条、启动决策 4 条、普查读账本 2 条（改写）、
  `task_end` 落款合并 1 条（改写夹具）。
- `mem://` 崩溃点对拍（`direct_window_replay_parity`）：夹具去掉 `gen_root` 行，`an_initialization_killed_at_any_step_wipes_and_reruns_from_scratch`
  新增断言「落库不再建 `gen_root` 表」。
- 源码钉：`model_db_adapter` / `model_coverage` / `batch_worker` / `window_root_plan` 的生产半边不含 `type::thing('gen_root'` /
  `FROM gen_root` / `gen_root:`。

## 4. 残余与后续

- `CONTEXT.md` 已把「模型水位失去支撑」「幽灵模型水位」「凭证前移」标成旧章，加了「生成根账本」「库级目录落款」两条。
- `scripts/Invoke-GenRootSweep.ps1` / `Import-SweepNdjson.ps1` 失去目标表，未删（运维脚本目录另议）。
- `model_type_scope.rs` 模块 doc 顺手改成水位口径；它的消费门今天只在初始化路（`dbnum_initialize`）生效，增量路没有调用点——不在本计划范围。
  后续（同日）：增量路既然不扣根，`increment_pipeline` 里那句「本窗 N 个生成根因 model_incremental_types 暂扣不生成」的 warning
  报的是没发生的事，连同 `window_plan_notice` / `held_counts_with` / `GATED_ACTIONS` 一并删掉；`mode_notice` 与 `handlers.rs`
  `/health` 注释里「其余类型的行留在队列里不消费」的口径仍是旧的，待定。
- 真部署验证见状态行。
