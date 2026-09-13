# 开发计划：external（ingest）模式——dbnum 初始化与模型增量更新都要「可证明地正确」

- 日期：2026-09-12
- 状态：**草案，待用户拍板 §4 D1–D4**；未实施
- 起因：用户「分析当前的 external 模式，使用 oracle mcp 制定下一步的针对 external 模式的开发计划。保证模型的增量更新和 dbnum 都能正确初始化」
- 方法：代码勘察（实位以 2026-09-12 工作树为准，含 ~6,300 行未提交改动）+ Oracle（ChatGPT Pro，会话 `gen-model-external-mode-plan-20260912`）
  对 §2 缺口清单与 §5 分阶段计划的评审；Oracle 的评级与推荐在文中标 **[Oracle]**，与我方勘察不一致处按代码实位为准并注明。
- 承接（不重复）：
  - `docs/plans/2026-09-08-external-increment-refactor-plan.md`（母计划；S0 已落，S3 ADR-065 已接受，S4–S9 随 ADR-065 / 071 / 072 / 073 大体落地；
    **S1 live #1、S11 live #2 一次都没跑**）
  - `docs/adr/ADR-073-…`（任何未完成写入 → 整库 Reinitialize；房间随根；`room_membership=false` 不阻断模型）
  - `docs/plans/2026-09-11-kv-mem-room-startup-and-publication-plan.md` §9「external / spawned-rocksdb 的 batch worker 与水位语义不退化」**未勾**
  - `docs/plans/2026-09-12-lazy-startup-dev-plan.md`（已批准未实施；本文 §5 P3 处理它与 ingest 面的耦合）
  - `docs/plans/2026-09-09-new-dbnum-manual-init-default-plan.md`（初始化准入门；其 live A1–A4 未跑）
  - `issues/ISSUE-029`（确定性失败的根无终态；Open）

---

## 0. 一句话

**external 的主链设计没有错，缺的是证据与三处契约。** external = 连外置 SurrealDB（rocksdb 后端）+ **ingest** 数据面，与 `spawned-rocksdb` 共用同一条
基线 / 增量窗口 / 水位 / 模型持久化路径（`options.rs:739` `is_durable()` → `data_face_for_store` → `Ingest`）。ADR-073 之后这条路**一次 live 都没有**；
同时有三处契约会让「初始化正确」「增量正确」说不清：① 空库 + 默认门 → 清单为空即 `data_ready`（§2 G4）；② 一个确定性失败的根钉死 `model_sesno`、
且重启不水化该库投影（§2 G3）；③ 懒加载计划删骨架时漏了 ingest 面的 sweep（§2 G2）。顺序是：**先证（P0）→ 修阻断（P1）→ 改契约（P2）→ 接懒加载（P3）→ 规模（P4）**。

---

## 1. 现状锚点（2026-09-12 实位）

### 1.1 形态

| 事实 | 位置 |
|---|---|
| `StoreMode::{External, SpawnedRocks, SpawnedMem, EmbeddedMem}`；`is_durable()` = External ∨ SpawnedRocks；`data_face_for_store`：durable → `Ingest`，否则 `ReadThrough` | `src/options.rs:713-905` |
| 默认 **`spawned-mem`**（2026-09-11 拍板翻转）；`"rocksdb"` 是 external 的历史别名；`in_memory_db=false` 且没写 `store_mode` 仍回 external | `options.rs:767-808` |
| 出厂模板 `spawned-mem`；`db_options/` 8 份夹具全是 `external + ingest`（护栏 `every_db_option_fixture_declares_its_store_mode_and_data_face`） | `scripts/templates/DbOption.factory.toml`、`db_options/*.toml` |
| external 直连：`connect_configured_store` → `init_surreal()` 失败即退出；不拉起、不收尸 | `lib.rs:1680-1695`、`:1782` |
| 两把写者锁：运行目录级 `ProcessInstanceLock`（external 拿）+ 机器级 `database_writer_lock`（endpoint+ns+db；**跨机器不保护**） | `lib.rs:370`、`src/data_interface/database_writer_lock.rs` |
| 数据面签名：read-through 用过的库切 external 报 `DATA_FACE_SIGNATURE_MISMATCH`，不自动迁移 | `src/data_interface/data_face_signature.rs`、`docs/database-storage-modes.md` |
| 设置模式：external 且 `v_ip` 空 / `v_port=0` → 只绑 HTTP（`mode:"setup"`），不做网络探测 | `CONTEXT.md`「设置模式」、`lib.rs` 启动臂 |

### 1.2 external 启动链（`lib.rs::run_cli`）

1. `acquire_process_instance_lock` → 几何并发闸 → 日志（`:711-771`）
2. surql 函数：磁盘脚本 → `embedded_surql::define_embedded_functions` → `selfcheck_surreal_functions`（`:776-815`）
3. `DbnumState::ensure_increment_state_storage()`：`dbnum_watermark / dbnum_info_table / dbnum_write_state / queue_control` + `idx_inst_info_dbnum`（`:817`，`dbnum_state.rs:255-265`）
4. `init_inst_relate_indices` → `backfill_inst_relate_anc`（`:829-835`）
5. `seed_legacy_model_watermarks()`（ADR-060 D10：有数据、有旧投影、`model_sesno` 从未写 → 填成 `applied_sesno`，`legacy_adopted`）（`:841`）
6. `startup_reconcile::schedule_unfinished_writes_for_reinitialize()`（ADR-073 D3：`writing` → 排 `Reinitialize`；登记行缺路径则只留标记）（`:855`，`startup_reconcile.rs:78-171`）
7. `enable_watermark_mirror` → `hydrate_watermark_mirror()` → **`hydrate_model_projections()`：只水化 `model_sesno > 0 && DESI` 的库** → `retire_publish_run_tables()`（`:875-884`，`:630-662`）
8. `init_form_config` → `init_mdb` → **`mdb_memory_store::ensure()`**（全 MDB 骨架，懒加载计划 L3 要删）（`:935-1000`）
9. `configure_model_bootstrap(false)` → `begin_startup_reconcile()`（`:1017-1020`）；read-through 才 `mark_data_ready_without_manifest`
10. `restore_persisted_pause` → HTTP `serve_if_configured` 先起（`:1029-1061`）
11. `prepare_room_resident_at_startup`（空间树；`room_membership=false` 默认 → 不建常驻房间模型）（`:1075`）
12. **ingest 才** `init_watcher()` = `sweep_watch_dirs("init", true)`（`:1084`；`increment_manager.rs:3597-3706`）
13. `ensure_batch_worker` → `async_watch`（`:1103-1129`）
14. `reconcile_model_coverage_at_startup()`（`:1141`；`startup_reconcile.rs:58-565`）：凭证下限 ↔ 文件最新；落后的库排 `ModelOnly`（`model_start_sesno = settled_floor ?? recorded`）
15. `wait_for_data_ready`（受阻且无批次在跑连续 180 s → 降级）→ `wait_for_model_ready` → 横幅（`:1147-`）

### 1.3 dbnum 任务体（`batch_worker.rs`）

- `execute_frozen_batch`（`:1300-1620`）：`ModelOnly` 走 `execute_model_only_body`，否则 `execute_frozen_batch_body`；Ref0 统计中途一笔（`closes_task:false`）；
  `round_complete` = 数据应用到位 ∧ **全部单元 `Generated`**（`:1555-1570`）；只有 `round_complete && DESI` 才 `task_end::commit_task_end_on(.. model: Some(TaskModelOutcome{ all_roots_ok: true, dead_roots: 0 }), closes_task: true)` 盖 `model_sesno`（`:1581-1615`）；
  否则末尾兜底只收写入标记。`TaskModelOutcome{all_roots_ok:false, dead_roots:n}` 在 `task_end.rs:27-34/85/132` 有实现，**worker 从不调用**。
- `execute_frozen_batch_body`（`:2100-2248`）：`execute_one_dbnum`（数据半边）→ `model_phase_is_deferred(model_incremental, applied)`（`:2254`）→
  `run_model_side_actions` → `explicit_regen_units` + 级联 → `merge_unit_worklist` → `run_unit_worklist`（逐根，失败 `mark_generation_failed` 写 `gen_root.status='failed'`）。
- `manual_update.rs::execute_one_dbnum`（`:5009-5290`）：`classify_scan` → `record_observation` → `reinit_wipe_decision` → `wipe_dbnum_for_reinit`
  （`fast_delete.rs:734-747`：清行 + 投影 `clear_dbnum` + `invalidate_geometry_baseline` + `invalidate_census` + `mark_aabb_tree_dirty` + 镜像作废 + `announce_source_changed(Memory)`）→
  `needs_initial_load`（`:164-173`）→ 基线 `initialize_dbnum_baseline` **或** 增量窗口 `increment_pipeline::apply_one`。

### 1.4 发现与初始化准入（`increment_manager.rs`）

- `automatic_discovery_admits(initialized, auto_initialize_new_dbnums, already_queued) = initialized || auto || already_queued`（`:51-58`）；
  默认 `auto_initialize_new_dbnums=false` → 从未应用过的库不入队，`DiscoveryOutcome::AwaitingManualInitialization`，`/dbnums.awaiting_manual_initialization=true`、`/health` 两处点名（`handlers.rs:755/900`）。**SYS meta 同样被拦、不开例外**（09-09 计划 §4 D4）。
- 手动路径不受门控：`/api/v1/update/preview` + `/update/execute`（plant-ui 手动更新向导；`/setup.html` 第⑤步是它们的薄壳）。
- `sweep_watch_dirs` ingest 分支开头无条件 `mdb_memory_store::refresh_if_stale`（`:3675-3690`，注释「Direct tree/attribute readers use the same in-memory MDB snapshot in RocksDB mode」）。
- `install_manifest(epoch, phases, held, blockers)`：`data_ready = current_phase.is_none()`——**清单里一个批次都没有时立即为真**，`model_phase_open = true`（`initialization_phase.rs:393-447`）；
  `mark_model_ready` 只再看 `startup_reconcile_pending` / `startup_rooms_pending`（`:682-695`）。

### 1.5 读源路由与发布策略（`model_read_route.rs:150-263`）

`route`：read-through → Memory；`applied<=0` → Memory(`data_watermark_unestablished`)；任务在飞 → Memory；`model<applied` → Memory(`model_lagging`)；`model>applied` → Memory(`watermarks_inconsistent`)；否则 **Database**。
`publication_policy_for(memory_only, face, mode)`：ingest 下 `ProcessStore{durable:true}`，`memory_only` 时 `ProjectionOnly`。
`dbnum_consistency::verdict`：`in_sync` ⇔ `model==applied ∧ chasing==0 ∧ abandoned==0`；**「已放弃」那一格随 pending 死信退了，恒 0**（`dbnum_consistency.rs:180-233`）。

### 1.6 最近一次持久形态证据

2026-09-10 `ams-init-1112-7997` 冷启动（rocksdb 空库）：1112 / 7997 `applied = model`、`in_sync`，7997 模型阶段 10.3 min；遗留 `room_build.rebuild_required=True` /
`spatial_tree=ready_empty`（changelog 2026-09-10「live 复跑」）。**此后**：ADR-073、`room_membership` 默认关、`spawned-mem` 默认、kv-mem 房间启动屏障
（`bootstrap_read_through_rooms` / `model_dbnum_ensure`）、RefNo 原生路由 W1–W9——都动了 `batch_worker.rs` / `lib.rs` / `initialization_phase.rs` / `model_db_adapter.rs`，external 没有再验。

---

## 2. 缺口与评级

| # | 缺口 | 评级（我方 / [Oracle]） | 依据（实位） |
|---|---|---|---|
| **G1** | external 全链（冷启动初始化 → 稳态 SAVEWORK → 崩溃 Reinitialize → 重启水化）在 ADR-073 与 6k 行未提交改动之后**零 live 证据**；母计划 S1 / S11 从未执行，`Run-ExternalIncrementLive.ps1` 只干跑过 `prepare` | 阻断 / [High，接近 Blocking] | 母计划 §4 状态列、§8；kv-mem 计划 §9 未勾项 |
| **G2** | 懒加载计划删骨架（L3 / L7 护栏「生产源码不含 `mdb_memory_store::`」）后，ingest 面 `sweep_watch_dirs` 的 `refresh_if_stale` 要么违反护栏、要么每次 sweep 把 61 万元素骨架建回来；计划 §0.1 清单**没有这一处**，也没说 ingest 的 watcher/sweep 与 `poll_generations` 的关系 | 高 / [High] | `increment_manager.rs:3675-3690`；懒加载计划 §0.1、§3.4、§5.2 |
| **G3** | **死根钉死 `model_sesno`**：`round_complete` 要全部单元 `Generated`；一个确定性失败的根（ISSUE-029 那类甲方数据问题）让该库永远 `model_lagging`：(a) 读源永远是内存投影；(b) `hydrate_model_projections` 只水化 `model_sesno>0`——**首次初始化就带一个死根的库 `model_sesno` 恒 0，重启后已落库的几千根投影不水化**，plant-ui 读空 / 缺，直到按需 ensure 逐根重建；(c) 启动复核每次排 `ModelOnly`、死根再失败一次；(d) `TaskModelOutcome{all_roots_ok:false}` 那条路 worker 从不走，`/dbnums.model_dead_roots` 恒 0 / 未观察。ADR-060 D3/D5「已放弃的根不拦水位、但要露出来」与 ADR-073 W3「全部根成功才推进」**相抵** | 高 / [High] | `batch_worker.rs:1555-1615`、`task_end.rs:27-34`、`lib.rs:630-662`、`dbnum_consistency.rs:225-233`、ISSUE-029 |
| **G4** | **空 external 库 + 默认配置起服务什么都不会自动发生**（门把 SYS meta 也拦）；且 `install_manifest` 收到空清单即 `data_ready=true`、模型门开，`mark_model_ready` 可能在一根都没生成时宣告 ready——「空」与「就绪」在 `/health.initialization` 上分不开；配置向导第⑤步 / plant-ui 手动执行这条路在 external 上端到端未验 | 阻断（语义）/ [Blocking 风险] | `increment_manager.rs:51-58`、`initialization_phase.rs:426`；09-09 计划 §4 D4 |
| **G5** | `hydrate_model_projections` 每次启动整体读回全部 `model_sesno>0` 的 DESI 模型行；AMS 尚可，JEU（417 库 / 122 万元素）未知；懒加载计划不覆盖这段 | 中 / [Medium] | `lib.rs:630-662` |
| **G6** | 母计划 §7 未证项：背靠背两次 SAVEWORK 不丢、第二次读代价（Q4，>20% 立项）、多库 Task 同压 rocksdb 时延 | 高 / [High] | 母计划 §5「live #2 另加」、§7 |
| **G7** | `room_build.rebuild_required=True` / `spatial_tree=ready_empty` 让旧 monitor 的 DONE 判据不成立 | 低 / [Low，降级]——ADR-073 后房间不再是独立阶段，这两格不再是 external 正确性判据；新判据见 §3 | changelog 2026-09-10 |
| **G8** | `legacy_adopted` 接管只把 `model_sesno` 填成 `applied_sesno`，没有校验投影完整性 | 中 / [Medium] | `dbnum_state.rs` `seed_legacy_model_watermarks`、`startup_reconcile.rs:392-400` |
| **G9** | CATA 前移（H1：改一个目录件 → SAVEWORK → 不重启看 `gen_root` / `/health.catalogue_reconcile`）在 external 上未 live | 中 / [Medium] | changelog 2026-09-08 H1 条目「未起服务」 |

**[Oracle] 对 G3 的补充**：问题本质不在房间（`room_membership=false` 时 `model_sesno` 只承诺几何，ADR-073 修订已明确），在于「成功根 + 已放弃根 = 是否允许推进 `model_sesno`」这一契约缺失——是 ADR 决策，不是 bug 修补（→ §4 D1）。

---

## 3. 完成定义（机器可断言；验收一律 AMS E3D 3.1 `/ALL`，隔离 external 实例）

通用锚点：`/api/v1/dbnums` 行字段（`initialized / awaiting_manual_initialization / applied_sesno / model_sesno / model_verdict / model_verdict_reason / model_chasing_roots / model_dead_roots / write_state.status`）、
`/api/v1/health`（`initialization.status / data_ready / model_ready / awaiting_manual_initialization[] / sul_db.medium / durable / data_face`）、
库表 `dbnum_watermark` / `dbnum_write_state` / `gen_root`、e3d-io `read_stats`（`database_opens / scan_calls / scanned_elements`）、日志行。

| 场景 | 必须成立 | 禁止 |
|---|---|---|
| **S1 空 external 库冷启动，`auto_initialize_new_dbnums=false`** | 每个 MDB 声明的 DESI：`initialized=false`、`awaiting_manual_initialization=true`、`applied=model=0`、`model_verdict=not_judged`；`/health.awaiting_manual_initialization` 非空；`initialization.status` **明确表达「等待初始化」**（D2 定名）；sweep 汇总一行「N 个库等待前端手动初始化」 | `model_ready=true` 且无任何解释（G4）；数据面任何写入 |
| **S1' 同上，`auto_initialize_new_dbnums=true`**（或 `serve --auto-initialize-new-dbnums`） | Meta → Catalogue → Design 清单按序收口；每个 DESI 一个 Task：数据基线 + 该库全部根 + `task_end`；终态 `dbnum_watermark.applied_sesno = model_sesno = 文件最新`、`dbnum_write_state.status=finished`、`gen_root` 全部安定（`source_end_sesno = T`）、`/dbnums.model_verdict=in_sync`；日志每库一段「模型生成开始 dbnum=…」 | `model_update_pending` 等任何退役表被写；`writing` 残留 |
| **S1'' 手动初始化**（`/update/preview` → `/update/execute` 勾选一个 DESI；`/setup.html` 第⑤步同路） | SYS meta 随批；该库终态同 S1'；其余库仍 `awaiting_manual_initialization=true` 且不受影响 | 门拦住已入队的手动行（09-09 计划「已在队列里的行」） |
| **S2 已初始化库重启** | 启动水化 `hydrate_model_projections` 覆盖全部 `applied>0` 且有安定根的 DESI（D1 (B) 之后 = `model_sesno>0`）；启动复核「开窗 0 库」；`/dbnums.model_verdict=in_sync`；`read_stats.scan_calls` 启动期 == 0（懒加载 L3 之后） | 重新 ingest；`applied` 变动；每次重启都排 `ModelOnly`（G3 (c)） |
| **S3 稳态 SAVEWORK**（`db8000_bran_ftub_move_apply.mac`） | ≤ 120 s：该库 `applied = model = N`、`in_sync`、变更根 `gen_root.source_end_sesno = N`；对照根 `/model/records` 逐字不变；`database_opens` 增量只含目标库 + 依赖，`scan_calls` 不含全 MDB 扫描；全程无人工 execute（`/health` manual 计数不动） | 模型工作被 defer 到下一轮（`model_phase_is_deferred` 只许因 `!model_incremental` 或数据未应用） |
| **S4 背靠背两次 SAVEWORK**（N、N+1，第二次落在第一窗 Task 运行中） | 同 dbnum 串行：第二窗排在第一窗之后（`/queue` 可见）；终态 `model_sesno = N+1`；两窗变更根都到 N+1 | `model_sesno` 停在 N；N+1 数据在、模型仍 N |
| **S5 崩溃恢复**（数据块中途 / 模型块中途 / `task_end` 前 kill -9） | 重启：`dbnum_write_state.status=writing` 被 `schedule_unfinished_writes_for_reinitialize` 点名 → 排 `Reinitialize` → `wipe_dbnum_for_reinit` → 首次导入 → 整库模型；终态各表与一次成功初始化**逐表相等**（ADR-073 实施约束 7 (a) 的 live 版） | 任何按 `sesno > S` 的删除；`writing` 残留超过一次重启 |
| **S6 死根存在**（用 ADR-073 约束 7 (b) 的方式让一根确定性失败） | 按 D1 拍板：(B) `model_sesno` 照推、`/dbnums.model_dead_roots=1`、`model_verdict_reason` 点名、`/health` `degraded`；重启后投影水化、其余根 `in_sync` 读库；(A) 则 `model_verdict=lagging` + `reason=blocked_by_dead_roots` **且**重启水化不受影响 | 永远 `model_lagging` 且重启后投影为空（G3 (b)） |
| **S7 CATA 前移**（改一个目录件 → SAVEWORK） | 不重启：sweep 触发 `reconcile_catalogue_versions`；`/health.catalogue_reconcile` 点名 stale dbnum；反查命中的根排带名单的 `ModelOnly`；引用根 `gen_root.source_end_sesno` 前移；`catalogue_dependency_refresh` 出声 | 引用根几何原样不动、`/health` 一声不响（H1 原症状） |

---

## 4. 决策点（用户拍板；每条给选项与推荐）

**D1　死根与 `model_sesno` 的关系（G3；ISSUE-029 的终局）**
- (A) 维持 ADR-073 W3：任一根失败 → `model_sesno` 不推进。强一致；代价是一个坏根拖住整库读源与水化。
- **(B) 推荐 [Oracle 亦推荐]**：引入根级终态 `gen_root.status = terminal_failed`（连败达阈值或错误类确定性——先只按 attempts 次数，ISSUE-025 落地后再按错误类）；
  `round_complete` = 每个单元 `Generated ∨ TerminalFailed`；`task_end` 走现成的 `TaskModelOutcome{all_roots_ok:false, dead_roots:n}` 落 `model_dead_roots` **并盖 `model_sesno`**；
  `dbnum_consistency::verdict` 恢复「已放弃」那一格（`abandoned` 从 `gen_root` 读，不再恒 0）：`in_sync` 保持 `abandoned==0` 才成立，新增 `model_verdict_reason=dead_roots`；
  `/health` 标 `degraded`；复活路 = 下一窗触到它 / `POST /model/ensure` / `model/rebuild`。回到 ADR-060 D3/D5 原口径，修订 ADR-073 W3 一句。
- (C) 保持严格水位，但 `model_verdict_reason=blocked_by_dead_roots` 点名 + 水化不看 `model_sesno`。不推荐长期。
- **无论 A/B/C 都要做**：`hydrate_model_projections` 的门从 `model_sesno > 0` 改成「`applied_sesno > 0` ∧ 该库有安定 `gen_root`」（P1.3），否则 (b) 那个洞不随 D1 关闭。

**D2　空库 / 有未初始化库时 `/health.initialization` 的语义（G4）**
- (a) [Oracle]：只要存在 `awaiting_manual_initialization` 的库就 `data_ready=false / model_ready=false`、`status=awaiting_initialization`。语义最硬；但既有部署新增一个库会把整个服务标成未就绪。
- **(b) 推荐（分档）**：**一个已初始化的 DESI 都没有**（空库）→ `status=awaiting_initialization`、`model_ready=false`；**混合**（有已初始化库 + 有等待的库）→ `ready`，但 `initialization.awaiting_manual_initialization[]` 非空且横幅 / `/health` 点名。判定点一处（`install_manifest` 收到空清单时问 `awaiting_manual_initialization_snapshot()` 与 `list_registered`）。
- 连带：出厂模板与向导「初始化两档单选」文案对齐；`startup_autorun=false` 与本门正交不变。

**D3　与懒加载计划的先后（G2）**
- **推荐**：懒加载计划 §4 的 L0（AMS 改前基线）可先采；**L2 之后的工作项等本文 P0 live 出账、P1 落地后再开**；本文 P3 把 ingest 面的两处（`sweep_watch_dirs` 的 `refresh_if_stale`、ingest 下 `direct_tree` / `element_attributes` 的骨架依赖）补进懒加载计划 L3 / L7 的范围与护栏。理由：删骨架后第一次点树扫全 MDB 这段区间「不出包」，external 的 live 若与它重叠，任何一条红都分不清来源。

**D4　P4 规模项现在立不立项（G5 / G8）**
- 推荐：P0 顺手量 `hydrate_model_projections` 的行数与耗时（AMS 28 库）进 `docs/baselines/`；**只有**外推到 JEU（按元素数线性）> 30 s 才在 P4 做「按 dbnum 懒水化 + 首次请求触发」；否则只加计数器。`legacy_adopted` 只加一次性校验（`gen_root` 数 vs 投影根数不符则告警、不接管），不做迁移。

---

## 5. 工作项与顺序

### P0　先证不改（只新增脚本 / 探针 / 测试，不动 `lib.rs` / `batch_worker.rs`）

| # | 工作项 | 文件 | 验收 |
|---|---|---|---|
| P0.0 | **边界 commit**：请在途会话（RefNo W4–W9、kv-mem 房间屏障）把各自 hunk 收进历史或至少打一个 WIP 提交；本文 P0 的证据要能指向一个 `build_id` | — | `git status` 里没有本文之外的 `M` 文件在 `src/` |
| P0.1 | **external live #1（母计划 S1 的补课）**：`Start-Surreal8009.ps1 -Bind 127.0.0.1:18109 -Datastore rocksdb:output/external-live/<stamp>/.surreal/external-live` + `Run-ExternalIncrementLive.ps1 prepare / serve / baseline-wait`；场景 §3 S1'（`auto_initialize_new_dbnums=true`，`watch_dbnums=[7997,7999]`）→ S2 重启 → S3（7997 BRAN 改段、OWNER 搬迁、FRMW 改名）→ S4 背靠背 → S7 CATA 前移 | `scripts/Run-ExternalIncrementLive.ps1`（已有；补 `restart` 与 `double-savework` 两个 phase）、`output/external-live/<stamp>/ledger.md` | §3 每格 PASS / FAIL 记账；`docs/evidence/2026-09-1x-external-live-1.md` |
| P0.2 | **`bin/external_live_probe`**（母计划 §8 末段「S1 现场要补的判据工具」）：只读 `--dbnum --refnos`，输出 `dbnum_watermark` / `dbnum_write_state` / `gen_root` 状态 + 变更元素 `pe / <NOUN> / ATT_UDA / pe_owner` 行与 `e3d_collector::read_ele_data` + `ElementRows::render` 的逐字段对拍 + `read_stats` 快照，JSON 出 | `src/bin/external_live_probe.rs`（新） | 对 S3 的三类变化各出一份 JSON，`diff` 为空 |
| P0.3 | **S1 空库两态 + S1'' 手动初始化**：空 rocksdb，默认门 → 抓 `/health` / `/dbnums` 快照；再走 `/update/preview` + `/update/execute` 只初始化 7998 | `Run-ExternalIncrementLive.ps1` 新 phase `cold-empty` | 现状快照进证据文件（G4 的「现状」是 D2 的输入） |
| P0.4 | **S5 崩溃三点**：数据块中途 / 模型块中途 / `task_end` 前 `Stop-Process -Force`，重启，对拍逐表 | 新 `scripts/Run-ExternalCrashRecoveryLive.ps1`（复用 ADR-073 约束 7 的 kill 点命名；崩溃点靠日志阶段行 `dbnum=… 阶段完成:` 定时） | 三次终态与一次成功初始化的 `pe / inst_relate / gen_root / room_* / dbnum_watermark / dbnum_write_state` 行数与摘要相等 |
| P0.5 | **S6 死根现状**：按 ADR-073 约束 7 (b) 的方式（目录件缺失）让 7999 一根确定性失败；记录 `model_sesno` / 读源 / 重启后投影行数 | 同上脚本 | 证实或推翻 G3 (a)(b)(c)(d)——这是 D1 的输入 |
| P0.6 | **回归网**：`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 全绿；`scripts/Run-L3Suite.ps1`（`db_options/DbOption-l3-suite.toml`，external）一轮 | — | 数字记进证据文件 |
| P0.7 | 量测：`hydrate_model_projections` 行数 / 耗时、7997 一窗第二次 `build_set + collect` 占比（母计划 Q4）、多库并发 Task 的 rocksdb 时延 | `docs/baselines/2026-09-1x-external-ams-stage0.json`（新） | JSON 无 `null`；JEU 数字留 null 不估算（d-536） |

P0 出口：**§3 S1'/S2/S3/S4/S5 任一 FAIL 且属阻断项（数据行错 / 水位不推 / 模型不出 / watcher 不响 / 恢复不收敛）→ 先修再往下（母计划 Q23 处置不变）。**

### P1　阻断修复（等 P0 出账、P0.0 边界 commit 之后）

| # | 工作项 | 文件 | 验收 / 护栏 |
|---|---|---|---|
| P1.1 | **空清单不等于就绪（D2）**：`install_manifest` 收到 0 批次时按 D2 分档给 `InitializationStatus::AwaitingInitialization`（新变体）或在 `InitializationSnapshot` 加 `awaiting_manual_initialization: Vec<u32>` + `uninitialized_only: bool`；`mark_model_ready` 在「空库」档拒绝；横幅一行 | `initialization_phase.rs:393-447/682-695`、`increment_manager.rs`（`sweep_dirs` 调 `install_manifest` 处）、`handlers.rs` `/health` 序列化、`lib.rs` 横幅 | 单测：空清单 + 0 已初始化 → 不 ready；空清单 + ≥1 已初始化 → ready 且列表非空；`web_service::tests` 契约不降；`refno_native_api_contract.rs` 键集加新键 |
| P1.2 | **死根终态（D1）**：按拍板实现；(B) 时：`gen_root.status='terminal_failed'` 写点在 `run_single_unit` 的失败分支（`batch_worker.rs:2263-2300`，连败阈值取 `MAX_ATTEMPTS` 常量）；`round_complete` 判据放宽到 `Generated ∨ TerminalFailed`（`:1555-1570`、`batch_round_complete :1993`）；`task_end` 用 `TaskModelOutcome{all_roots_ok:false, dead_roots}`；`dbnum_consistency::verdict` 的 `abandoned` 改从 `gen_root` 普查读；`startup_reconcile` 的 `credential_behind` 不把 `terminal_failed` 算「未安定」；`/dbnums.model_dead_roots`、`/health.degraded`；ISSUE-029 关闭 | `batch_worker.rs`、`task_end.rs`、`dbnum_consistency.rs`、`model_coverage.rs`（`startup_coverage_floors`）、`startup_reconcile.rs:440-450`、`docs/adr/ADR-074-…`（新，修订 ADR-073 W3、恢复 ADR-060 D3/D5 口径） | `mem://` 真测：一根确定性失败 → `model_sesno` 照推、`model_dead_roots=1`、`verdict_reason=dead_roots`、重启不排 `ModelOnly`；源码钉：`execute_frozen_batch(` 段落含 `dead_roots:` 且不再含 `dead_roots: 0,` 字面 |
| P1.3 | **水化门改口**：`hydrate_model_projections` 的过滤从 `model_sesno > 0` 改成 `applied_sesno > 0 ∧ (model_sesno > 0 ∨ 该库有安定 gen_root)`；`restore_projection_on` 只装安定根的行（待核实：它是否已按 `RootReceipt` 逐根装，`model_db_adapter.rs:1260`） | `lib.rs:630-662`、`model_db_adapter.rs:1260` | `mem://` 真测：`model_sesno=0` 但有 N 个安定根的库重启后投影 N 根；护栏：`hydrate_model_projections(` 段落不含 `model_sesno > 0 &&` |
| P1.4 | **手动初始化端到端**（S1''）若 P0.3 出红：修 `/update/execute` 在 external 上的 Meta 随批 / 顺序 / `startup_autorun` 交互 | `manual_update.rs:4566-4700`、`web_service/setup.rs`（第⑤步壳） | S1'' 全绿；09-09 计划 A1–A4 打勾 |

### P2　契约与文档

| # | 工作项 | 文件 |
|---|---|---|
| P2.1 | `ADR-074`（D1 拍板的成文）：死根终态、`model_sesno` 的承诺分档（几何 / 几何+房间 / 含已放弃根）、`/dbnums` 四格换源；标 ISSUE-029 关闭 | `docs/adr/`、`issues/ISSUE-029` |
| P2.2 | `CONTEXT.md` 词条：「初始化准入门」（09-09 计划 O5 未做）、「等待初始化」状态、「已放弃根 (Terminal-Failed Root)」；`docs/specs/web-service-api.md` `/health.initialization` 新键、`/dbnums.model_dead_roots` 口径 | 文档 |
| P2.3 | `docs/database-storage-modes.md` 补一段「external 与 spawned-rocksdb 共用 ingest；只有 external 才有：直连失败即退、跨机器写者不受锁保护、版本闸 2.1.x」 | 文档 |
| P2.4 | 母计划 §4 状态列：S1 = 本文 P0.1、S11 = 本文 P0/P1 之后的 live #2；kv-mem 计划 §9 那一项按证据打勾 | 两份计划 |

### P3　与懒加载计划的耦合（D3）

| # | 工作项 | 文件 | 护栏 |
|---|---|---|---|
| P3.1 | 懒加载计划 §0.1 加两行：`increment_manager.rs::sweep_watch_dirs` ingest 分支的 `refresh_if_stale`（`:3675-3690`）与 ingest 下 `direct_tree` / `element_attributes` 的骨架依赖；L3 范围扩到 `sweep_watch_dirs`；L7 护栏对 `increment_manager.rs` 同样生效 | 懒加载计划 | `sweep_watch_dirs(` 段落不含 `mdb_memory_store::` |
| P3.2 | ingest 面的「变化检测」明确为：**sweep 本身就是按 dbnum 读文件头比 `applied_sesno`**（`discover_batch`），不需要骨架；`poll_generations()`（L4）在 ingest 下只服务读侧缓存失效（`DbSet` 槽 / provider generation），**不**替代 sweep 入队；两者读同一份 `GenerationLedger` | 懒加载计划 §3.4、`db_generation.rs`、`mdb_route_directory.rs` | 单测：ingest 下一次 SAVEWORK → sweep 排 1 个批次 ∧ `poll_generations` 报同一个 dbnum，其余 dbnum 的 generation 不变 |
| P3.3 | L5（树脱骨架）落地后在 external 上复跑 §3 S2 / S3 一次：`read_stats.scan_calls` 启动期 0、SAVEWORK 期不含全 MDB 扫描 | `Run-ExternalIncrementLive.ps1` | 数字进 `docs/baselines/` |

### P4　规模与可观测（D4）

| # | 工作项 | 文件 |
|---|---|---|
| P4.1 | `/health` 加 external 计时格：`startup_seconds / hydrate_rows / hydrate_ms / baseline_ms / apply_ms / model_ms`（按 dbnum 最近一次） | `initialization_phase.rs`、`handlers.rs` |
| P4.2 | （仅当 P0.7 外推 > 30 s）`hydrate_model_projections` 改按 dbnum 懒水化：启动只装 `watch_dbnums` / 最近访问的库，其余在首次 `/model/*` 请求时装；路由在未水化时给 `Memory(reason=projection_not_hydrated)` | `lib.rs`、`model_read_route.rs`、`model_memory_store.rs` |
| P4.3 | `legacy_adopted` 一次性校验：接管前比 `gen_root` 安定根数 vs 投影根数，不符只告警不接管 | `dbnum_state.rs` `seed_legacy_model_watermarks` |

---

## 6. 测试与验收矩阵

| 层 | 命令 / 夹具 | 覆盖 |
|---|---|---|
| 单测 | `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected`（仓库惯例；那条自 09-07 起硬崩测试二进制） | P1.1 / P1.2 / P1.3 的纯函数与源码钉 |
| `mem://` 真跑 | 现有 ADR-073 约束 7 的 kill/reinit 重放（`d127e336a` 那组）+ 新增死根 / 水化门两组 | S5 / S6 的进程内版 |
| 隔离 external | `scripts/Start-Surreal8009.ps1 -Bind 127.0.0.1:<port> -Datastore rocksdb:<dir>`（**只认 fork 2.1.x `bin/surreal.exe`**）+ `scripts/Run-ExternalIncrementLive.ps1`（已有 7 个 phase；补 `cold-empty / restart / double-savework`）+ 新 `Run-ExternalCrashRecoveryLive.ps1` + `bin/external_live_probe` | §3 全部场景（真 external：直连、锁、签名、版本闸都只在这一档出现） |
| `spawned-rocksdb` 对照 | 同一套脚本改 `store_mode=spawned-rocksdb`@18010（冒烟门 C 腿的形状） | 证明两档 ingest 行为一致；**不能替代** external 那一遍 |
| l3 suite | `scripts/Run-L3Suite.ps1`（`DbOption-l3-suite.toml` 已是 external） | 回归 |
| 可复用 | `Run-ExternalIncrementLive.ps1`、`Start-Surreal8009.ps1`、`Verify-RocksDb8000.ps1`、`db_options/DbOption-init-7997.toml`、`scripts/e3d/db8000_bran_ftub_move_*.mac`、`tests/read_through_rows_parity.rs` 的比法 | — |
| 要新写 | `bin/external_live_probe`、`Run-ExternalCrashRecoveryLive.ps1`、`cold-empty / restart / double-savework` 三个 phase、`docs/baselines/*external*` | — |

---

## 7. 不做 / 边界 / 撞车规避

- **不复活任何已退役机制**：`model_update_pending`、publish 阶段、第二条异步账、按 sesno 回退、第三枚房间水位、独立房间队列 / 房间轮、`ResidentForm::Published`（ADR-065 / 071 / 072 / 073）。
- 不动 read-through 侧的语义（kv-mem 房间屏障、`ModelPublicationPolicy` 的 `ProcessStore{durable:false}`）；不把「external 的问题」靠翻默认回 external 解决。
- 不做跨机器写者互斥；不做旧库迁移（ADR-073 D6）；不做 CATA→DESI 级联换 dab 反向表（另起）。
- 不做 JEU 估算填数（d-536）。
- **撞车规避与顺序 [Oracle 同]**：① 在途会话先收 W4–W9 与 kv-mem 屏障（P0.0 边界 commit）→ ② 本文 P0 只新增探针 / 脚本 / 测试与证据 → ③ P1 动 `lib.rs` / `batch_worker.rs` / `initialization_phase.rs` / `handlers.rs` 前与在途会话对锁（`acquire_file_locks`）→ ④ P3 之后才开懒加载 L2+。
- 与懒加载计划共用的验收口径：**计数器与逐表对拍，不靠计时**（d-536）。

---

## 8. Go / No-Go 门

- **No-Go（不得开懒加载 L2+、不得出 external 包）**：§3 S1' / S2 / S3 / S5 任一 FAIL；或 D1 未拍板而 S6 现状证实 G3 (b)（重启后投影为空）。
- **Go**：隔离 external 上 §3 全部场景至少各 1 次 PASS（S3 三类变化、S4、S5 三个 kill 点、S6 按 D1 口径、S7），且每次终态 `applied == model`、`write_state=finished`、`gen_root` 无非终态失败根、`cargo test --lib` 不降、l3 suite 绿；证据落 `docs/evidence/` 并回写母计划 §4 与 kv-mem 计划 §9。

---

## 9. 落地记录

（空；P0 开跑后逐项补：日期 / build_id / 端口 / 场景 / PASS-FAIL / 证据路径。）
