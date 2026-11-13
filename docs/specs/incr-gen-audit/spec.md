# gen-model 模型增量更新链路 · 规则全集与审核规格（Spec）

状态：审核稿（源码只读审计产出，未经用户逐条确认）
日期：2026-10-27
范围：`gen-model` 模型增量更新链路——从文件发现、窗口规划、生成根选取、三条发布路（regen / transform / unit）、落库删改、房间边、缓存、双水位收尾、一致性判决，到按需生成与 HTTP 面；以及钉住这些规则的测试实体。
关联：`docs/specs/incr-gen-audit/plan.md`（开发计划）、`tasks.md`（任务清单）、`review.md`（审核结论）；被审对象 `docs/specs/incr-gen-fixes/*`、`docs/specs/core3d-partial-update-conformance.md`、`docs/specs/web-service-api.md`、`docs/adr/ADR-060 … ADR-089`、`CONTEXT.md`。

> 读法：本文件是 **WHAT / WHY + 规则全集**。每条规则一行：`R-<域>-<n> | 规则（MUST/SHOULD）| 出处 | 置信度`。出处是审计时（2026-10-27 工作树，含未提交改动）读到的 `相对路径:行号` 或 `ADR-0xx Dn`；行号会随代码漂移，符号名与 ADR 编号才是稳定锚。置信度「中」表示规则从 ADR/注释推出、未读完对应代码段。
> 术语沿用 `CONTEXT.md`：生成根 / 交付单元根（MDU）/ 正常颗粒 / Significant Owner / 应用水位 `applied_sesno` / 模型水位 `model_sesno` / 生成根账本 / 单元级发布 / 切片删 / 净变化。**已退役词**（规格里不得再当现行概念）：待重试单元、`model_update_pending`、`gen_root`、`room_sesno`、`model_drain`、`room_recalc`、`included_projects`、`legacy_adopted` 判定。

---

## Problem Statement

增量更新链路在 2026-09 ～ 2026-10 经历了 ADR-060 ～ ADR-089 三十份决策的连续重构：pending 表与 `gen_root` 退役、双水位与进程内账本、单元级发布、切片删、MDB 闭包、房间随根、缓存闸、按需新鲜度。但仓库里唯一一份「增量修复规格」`docs/specs/incr-gen-fixes/*` 停在 2026-07-24 的旧管线（`init_watcher → IncrementPipeline::apply → model_update_pending::drain → replace_exist`），其 F1–F9 的根因叙述、引用符号与验收标准大半已不在源码里；`core3d-partial-update-conformance.md` 多条 ✅ 项引用的符号（`discard_pending_for_full_rebuild`、`persist_root_in_chunks`、`projected_geometry_ids_on`）已不存在；若干 ADR 正文仍写着被后续 ADR 推翻的句子（三枚水位、`gen_root` 回灌、单元级发布默认关、`Start-AMS.ps1` 回填 `mem`）；源码注释与默认值打架（`model_unit_level_publish`「默认 false」vs 实现默认开）。结果是：没有一份能当施工对照的「现行规则全集」，后续任何人按旧规格开工都会修错层。

## Solution

把链路拆成 12 个域、19 条审计线（A–P、K1/K2、L1–L3、M），逐域从**现行源码 + 已接受 ADR** 提取可执行规则（MUST/SHOULD，带出处），对照三份旧规格与 ADR 正文列出偏差，给出按依赖排序的开发计划与原子任务，并在 `review.md` 给出现状 / 缺口 / 风险 / 下一步。本规格本身**不改变任何行为**；它把已落地的行为写成规则，把未拍板的决策列为待办。

## User Stories

1. 作为增量链路维护者，我要一份与源码同步的规则全集，以便改任一环节前能查到不变量与出处，而不是靠读 7000 行 `batch_worker.rs`。
2. 作为维护者，我要知道每条规则由哪份 ADR 支撑、哪些 ADR 句子已过时，以便改文档时不抄错。
3. 作为维护者，我要旧规格 `incr-gen-fixes` 的 F1–F9 逐条有「已修 / 架构替换 / 真缺口」判定，以便决定冻结还是重写。
4. 作为建模工程师，我在 E3D SAVEWORK 之后，要只有被改到的生成根重算、没动的根不碰，以便增量窗口在秒级完成。
5. 作为建模工程师，我只挪 EQUI 的位置时，要走刚体前移而不重网格，以便大设备移位不触发整根重算。
6. 作为建模工程师，我改一根 BRAN 里的一个构件时，要只重建该单元并保留根回执，以便局部发布不抹掉同根其它单元。
7. 作为建模工程师，我删掉一个容器时，要它整棵子树的几何、关系行和两张房间边一起消失，不留孤儿。
8. 作为建模工程师，我改 PANE/FRMW 时，要受影响的房间边在刷盘后被定向重算，而不是整库重算。
9. 作为运维，我要 `/health` 一眼看出每个 dbnum 是 `in_sync / lagging / not_judged`，并知道 lagging 是哪一格（chasing / abandoned / lazy）。
10. 作为运维，我要进程崩溃后重启时数据库自己走到一致状态（整库 Reinitialize 或 ModelOnly 补窗），不需要人工清表。
11. 作为运维，我要配置面只有一处决定 `store_mode`，并且漏设时失败出声而不是静默落到内存。
12. 作为运维，我要知道哪些 `AIOS_*` 环境变量已退役、写了只点名不生效。
13. 作为运维，我要测试钩（`AIOS_TEST_FAIL_*`）在生产进程里不会被误触发停跑。
14. 作为运维，我要一库数据失败被 park 时，其它库继续前进。
15. 作为前端（plant-ui）开发者，我要 `/model/records` 对没生成过的根返回 200 空而不是 409，并且 lagging 时按根回读 durable。
16. 作为前端开发者，我要 `/model/generate` 的超时与 409/504 语义只有一种书面口径。
17. 作为前端开发者，我要 `/dbnums` 把 `excluded`（MDB 没声明）、`blocked`（文件异常）、被 `watch_dbnums` 圈掉三种分开。
18. 作为前端开发者，我要 `/model/versions`、`/history/generate`、`/node/diff-summary` 在 API 规格里有专节。
19. 作为测试工程师，我要 L2 回放用例集与登记表 `increment_cases.json` 是单一事实，计划文档不再手写第二份状态表。
20. 作为测试工程师，我要 L2 回放服务跑在 `run_dir` 而不是仓根，以便与常驻服务不抢单实例锁、不污染仓根。
21. 作为测试工程师，我要缺失的链（PIPE 整条位移、NOZZ、PANE、两库互不阻断）有夹具可录。
22. 作为架构师，我要 ADR 之间的覆盖关系（072→073、076 D5→071 D4、080 D4→089 D7、083 标题→D1）在旧文里有横幅，而不是只在新文里单向声明。
23. 作为架构师，我要没有 ADR 支撑的已落地决策（`NameChanged`、#76 种子、刚体 P4 前置、#73 指纹、#71 拍点、mesh-purge 不删管身键、数据批次 park）补 ADR 或追记。
24. 作为架构师，我要两份都叫 ADR-060 的文件有明确区分方式。
25. 作为支持人员，我要诊断技能 `debug-model-generation` 的四格与 `/health` 的四格对得上，不再把退役开关当成「没算房间」的原因。
26. 作为代码评审者，我要源码注释里的默认值与 `options.rs` 实现一致（单元级发布、电缆级联、元素缓存上限、`auto_initialize_new_dbnums`、HTTP 默认地址）。
27. 作为发布工程师，我要 `v0.1.43` / `v0.1.44` 有 tag、抽查留痕入库。

---

## 规则全集

### §A 双水位、任务收尾与一致性判决

- R-A-1 | `applied_sesno` MUST 只在数据面成功落库后单调推进（`math::max`，不回退）；扫描/观察 MUST 不写它。稳态 `ApplyWindow` 写点是数据尾事务 `increment_tail::finalize_attempt`；初始化走 `TaskEnd.data` 与模型水位同笔（ADR-077 D7）。 | `src/data_interface/increment_tail.rs:52-84,142-155,214-221`；`increment_pipeline.rs:966-967,1498-1500`；`dbnum_state.rs:1780-1787,1856-1865`；`task_end.rs:78-94,270-271`；`dbnum_initialize.rs:334-342` | 高
- R-A-2 | `model_sesno` MUST 只由 `task_end` 盖，且仅当 `TaskModelOutcome.all_roots_ok`；`Partial` / 失败根 MUST 不盖水位，但仍写 `model_dead_roots` / `model_roots_observed_at`。盖章 SQL 带 `WHERE (applied_sesno ?: 0) >= T`。 | `task_end.rs:32-34,116-120,140-213,561-571,588-606`；ADR-071 D4 | 高
- R-A-3 | `model_sesno ≤ applied_sesno` MUST 恒成立；`recovery_stage` 在 `applied < model` 时报 `InvalidWatermarks`；启动对账把 `model ≥ applied` 视为已追平。 | `task_end.rs:205-207,478-489`；`dbnum_state.rs:421-423`；`startup_reconcile.rs:38-43,626-631` | 高
- R-A-4 | `model_catalogue_sessions` MUST 合并写且只在 `all_roots_ok` 时进 SET；`model_room_generation` MUST 仅整库资格任务 `Aligned`/`Cleared`，普通增量 `Untouched`；`all_roots_ok=false` 时两格都不盖。 | `task_end.rs:37-57,170-193,216-234`；`batch_worker.rs:2809-2842`；ADR-079 D4；ADR-073 D14 | 高
- R-A-5 | 一致性判决 MUST 只有 `in_sync` / `lagging` / `not_judged`（`NotJudgedReason::{ReadThrough, DataWatermarkUnestablished}`）；HTTP/健康检查 MUST 调 `dbnum_consistency::verdict`，不得重拼。 | `dbnum_consistency.rs:1-5,23-26,101-120,155-202`；ADR-060(水位) D9 | 高
- R-A-6 | `in_sync` MUST 同时满足 `model_sesno == applied_sesno` 且进程内普查 `chasing==0 && abandoned==0`；重启后账本为空，只靠两枚水位说话。 | `dbnum_consistency.rs:7-9,188-201,550-558` | 高
- R-A-7 | `DataFace::ReadThrough` MUST 直接 `not_judged(read-through)`；Ingest 下 `applied_sesno <= 0` MUST `not_judged(data_watermark_unestablished)`。`store_mode=mem` 派生 `ReadThrough`。 | `dbnum_consistency.rs:156-172,571-576`；`options.rs:1488-1525` | 高
- R-A-8 | `/health.dbnum_consistency` MUST 暴露 `health_summary`（仅 DESI ∩ watch 范围；lagging 不进 `blocking_conditions`）；`/dbnums` MUST 经 `VerdictColumns` 露出 `model_verdict` / `model_verdict_reason` / 四格根数。 | `web_service/handlers.rs:524-533,1024-1032,1212-1213,3760-3816,3920-3961`；`dbnum_consistency.rs:524-533` | 高
- R-A-9 | 队列意图 MUST 只有 `BatchIntent::{ApplyWindow, Reinitialize, ModelOnly}`；`RoomOnly` 是 `ModelWorkAction` / `WriteIntent`，不是队列意图；崩溃恢复一律折成 `Reinitialize`。 | `batch_queue.rs:20-33`；`model_update_plan.rs:36`；`write_state.rs:42-47,88-94` | 高
- R-A-10 | 任务状态 MUST 为 `TaskState::{Queued, Running, Succeeded, Partial, Yielded, Failed}`；`parked` 不是任务状态，是进程内连败账停自动重跑。 | `task_registry.rs:71-105`；`batch_worker.rs:47-51,431-432,1213-1216` | 高
- R-A-11 | 进程死在写库中途（`dbnum_write_state.status=writing`）MUST 整库 `Reinitialize`，不按 sesno 回退；开工前残留标记就地改意图；正常 `Partial`（标记已 `finished`）MUST 不触发重建。 | `startup_reconcile.rs:9-10,92-102`；`batch_worker.rs:1414-1422`；`write_state.rs:1-5,88-94,372-387`；ADR-073 D3；ADR-076 D8 | 高
- R-A-12 | 数据水位已推、模型未盖 → `lagging`；重启由 `startup_model_recovery_plan` 排 `ModelOnly`（左端 `model_sesno`，`0`=整库）；同进程不自动重试失败根（ADR-065 D4）。 | `startup_reconcile.rs:1-6,38-52,618-643`；`task_end.rs:33-34` | 高
- R-A-13 | 收尾盖 `model_sesno` 前 MUST `await_roots_durable` 等本任务单元根 ACK，`all_acked()` 才允许 `all_roots_ok`；非持久档 `publish` 不登记队列，「不在队列 = 已 ACK」。 | `fast_model/model_write_behind.rs:7-9,512-516,683-692`；`batch_worker.rs:1638-1650,1734-1751` | 高
- R-A-14 | 刷盘预算用尽 / `Blocked` MUST `abandon_undurable_roots`、任务 `Partial`、`model_sesno` 不盖，但写入标记 MUST 收成 `finished`（避免跨重启整库活锁）。 | `batch_worker.rs:1781-1785,1929-1934`；`task_end.rs:110-112,558-571`；ADR-076 D6 修订段 | 高
- R-A-15 | 启动顺序 MUST：legacy 模型水位播种 → `schedule_unfinished_writes_for_reinitialize` → `enable_watermark_mirror` + `hydrate_watermark_mirror`。 | `src/lib.rs:1032-1068`；`startup_reconcile.rs:95-98`；`dbnum_state.rs:1346-1353` | 高
- R-A-16 | `classify_scan` / `record_observation` MUST 只刷新观察/身份，永不写 `applied_sesno`；回退文件阻断但不回退水位。 | `dbnum_state.rs:1636-1736,1780-1833` | 高
- R-A-17 | 截断窗（`committed_end < file_latest`）MUST 仍等本批根 ACK，但 MUST 不盖 `model_sesno`；盖水位只在追平窗。 | `batch_worker.rs:1645-1650,1675,1736-1738,3363-3374` | 高
- R-A-18 | `finished` 全仓唯一写点 MUST 是 `task_end::render_mark_finished`；收尾一笔依次：可选 `applied` 推进（仅 `data: Some`）→ Ref0 快照 → `status=finished` → `UPDATE dbnum_watermark`。 | `task_end.rs:269-302,519-532` | 高

崩溃点 → 恢复（规则化）：`mark_writing` 之前死 → 无动作、窗口可重入；`writing` 已落、数据尾未提交 → 整库 Reinitialize；数据尾已推 `applied_sesno`、`task_end` 未落 → **同样**整库 Reinitialize（ADR-072 D2 的分面回退已被 ADR-073 D3 取代）；`task_end` 已 `finished` 且 `all_roots_ok=false` → `lagging`，启动排 `ModelOnly`；kv-mem / ReadThrough → `not_judged`，write-behind 不登记。

### §B 生成根与进程内账本

- R-B-1 | 变更元素 → 生成根的归一 MUST 只有一份，在 `generation_root.rs`；执行端 `ModelRefreshPolicy` 不另做选根。 | `src/data_interface/model_refresh.rs:8-14`；`generation_root.rs:1-11` | 高
- R-B-2 | 默认最小交付单元 MUST 为 `BRAN` / `HANG` / `SUPPO` / `EQUI`；`delivery_unit_types` 整体替换（空数组 = 无 MDU），`append_delivery_unit_types` 只扩默认集。 | `generation_root.rs:29-32,255-261,288-300`；`handlers.rs:1158-1162` | 高
- R-B-3 | `WORL` / `WORLD` / `SITE` / `ZONE` MUST 永不为生成根，也不作 fallback 单元。 | `generation_root.rs:5-6,34-35,208-211,390-391,603-605` | 高
- R-B-4 | `FTUB` MUST 永不作为 MDU；解析时跳过 component-only，上卷到可渲染属主。 | `generation_root.rs:36-37,213-224`；测试 `:1336-1377` | 高
- R-B-5 | 元素选根 MUST：① 链上最近 MDU；② 否则跨 loop 容器取第一个非容器、非 component owner；③ owner 为粗层级或缺席时以自身为根；④ 自身为粗层级则 `None`。OWNER 引用走 `resolve_owner_generation_root`，MDU 仍优先。 | `generation_root.rs:370-429` | 高
- R-B-6 | 子树枚举 MUST 与解析共用 `enumerate_generation_roots_in_subtree`：交付单元优先且其下不再出根；交付单元外 Core `significant` 为真的 noun 各成根（显著名下的显著节点也是根）；文件侧 `enumerate_generation_roots(DbSet)` 与 `pe` 侧 `enumerate_durable_generation_roots` 同一段遍历。 | `generation_root.rs:599-656,697-732,766-818` | 高
- R-B-7 | MDU 配置 MUST 压过位表：挂在 EQUI 下的显著 point 仍归 EQUI；`model_significant_over_point` 开着时仅 `core_significant_bit == Some(true)` 的 point 容器自成根。 | `generation_root.rs:353-368`；测试 `:1655-1679` | 高
- R-B-8 | Core `toplevel` 位 MUST 不参与任何选根判定；快照外 noun 的 `noun_is_significant` 保守为真。 | `generation_root.rs:70-77,131-137,154-159` | 高
- R-B-9 | 默认 MDU 与 Core `significant` 在 `SUPPO` 上 MUST 允许分叉（core 判不显著，我方仍当 MDU）。 | 测试 `generation_root.rs:948-963,983-985` | 高
- R-B-10 | 稳态窗选根输入 MUST 只有文件（`DbSet@S`/`DbSet@T`、`plan_update` 闭包、两端枚举）+ 进程内账本；一行 `pe` / `gen_root` 都不读；`S` MUST 等于该库 `model_sesno`。 | `window_root_plan.rs:1-10,73-76,1517-1518`；ADR-079 D3 | 高
- R-B-11 | 候选 = `roots_S ∪ roots_T`；四去向 MUST 为 `regen = touched ∩ roots_T`、`delete = roots_S \ roots_T`、`advance = (roots_S ∩ roots_T) \ touched`、`lazy = (roots_T \ roots_S) \ touched`；退化时 `advance` 清空、`roots_S ∩ roots_T` 全进 `regen`，`lazy` 不动。 | `window_root_plan.rs:12-27,163-171,238-270` | 高
- R-B-12 | 闭包护栏：不完整、`unresolved>0`、设计引用级联 degraded、或净候选 `candidates−cache_stamp` 严格大于索引键数 30% → 不信闭包。 | `window_root_plan.rs:70-71,118-154` | 高
- R-B-13 | 「安定」MUST = `roots_S \ chasing_roots`；`Refused` 根不算 chasing；欠账 MUST 由 `carry_unsettled_generation` 并进本窗 `regen` 并从 `advance`/`lazy` 摘掉；T 端已不是根的欠账随 `delete`。 | `window_root_plan.rs:1893-1906,2211-2224,2421-2443`；测试 `:3171-3225` | 高
- R-B-14 | 启动 / ModelOnly：`S = model_sesno`（`0`/缺 = 整库 `DbSet@T` 重排；`≥ T` = 不开窗）；`chasing_entries` 无条件重排；重启后账本为空，欠账由水位缺口窗的差分兜住。 | `window_root_plan.rs:2646-2668,2687-2724,2766-2767`；ADR-079 D3 | 高
- R-B-15 | 逐根成败 MUST 只活在进程内 `model_root_ledger`（一把 `RwLock`）；库里模型面唯一持久事实是 `dbnum_watermark.model_sesno`。 | `model_root_ledger.rs:1-16,99-104`；ADR-079 D2/D3 | 高
- R-B-16 | 结局枚举 MUST 为 `Generated` / `Failed{error}` / `Refused{error}`；普查三格 `settled←Generated`、`chasing←Failed`、`abandoned←Refused`；HTTP `status_label` 为 `"Generated"` / `"failed"` / `"http_422"`。 | `model_root_ledger.rs:36-47,61-97,344-353` | 高
- R-B-17 | `Generated` 给普查计数且参与按需凭证的根级半边（`root_credential_covers`）；成功事实宿主是 `RootReceipt`。（账本模块头「不参与任何放行判据」与 ADR-079 决策 6 矛盾，见 review） | `model_root_ledger.rs:18-19`；`model_coverage.rs:83-108`；ADR-079 决策 6 | 高
- R-B-18 | 根自身生成报错同一句连败 `DEAD_ROOT_STREAK=5` 次 MUST 转 `Refused`（不再进 regen、不拦后续窗 `model_sesno`）；错误变了从 1 重数；刷盘/房间/整批失败走 `record_transient_failure`（记 `Failed`、连败清零）。 | `model_root_ledger.rs:30-34,145-223`；测试 `:409-503`；`batch_worker.rs:3285-3317` | 高
- R-B-19 | 判死那一窗 MUST 撤旧模型（`delete_model_subtree`）再 `restore` 账本条目；未 ACK / 预算用尽走 `abandon_undurable_roots`：摘队列、`drop_cached_root`、`FailureEvidence::Circumstance`、任务 `Partial`、本窗不盖 `model_sesno`。 | `batch_worker.rs:1646-1673,1929-2013`；`model_root_ledger.rs:11-13,276-279` | 高
- R-B-20 | `/health` 根账 MUST 经 `dbnum_consistency`：`census_all()` 三格 + 水位行 `model_lazy_roots` 合成 `RootQuadrants`（`caught_up/chasing/abandoned/lazy`）；`model_cache_health` 只报投影占用，不含账本。 | `dbnum_consistency.rs:37-44,155-199,406-411`；`handlers.rs:204-260,1024-1026,1212-1213`；ADR-079 D7 | 高
- R-B-21 | 诊断四格（逐 refno）：已追平 = 账本 `Generated` 且 `source_end_sesno` 够新；还在赶 = `Failed` 或窗未覆盖；已放弃 = `Refused`/`http_422`；没生成过 = 账本无记录且水位未覆盖 → 懒生成 / `/model/generate`。（失败条目 `source_end_sesno` 恒 0，见 review） | `.devin/skills/debug-model-generation/SKILL.md:48-69`；`batch_worker.rs:3313-3316`；`on_demand_model.rs:788-790` | 高
- R-B-22 | `RootReceipt` MUST 含 `source_sesno` / `model_target` / `manifest_hash` / `geometry_count` / `published_via`；ACK 身份是前四元组；`published_via` 只观测（`"regen"` / `"transform"` / `"unit"`）。 | `fast_model/model_memory_store.rs:145-213,241-246`；ADR-080 D4/D5 | 高
- R-B-23 | `/model/records` MUST 按祖先回答（精确回执根 / `receipt_roots_under` / `receipt_root_above` + `overlay_records_under_node`；库路由 `anc CONTAINS`）；没产物 MUST 200 空，409 只留给 `conflict`。 | `handlers.rs:4403-4432,4998-5022`；`model_record_reader.rs:1-4,101-115,232-245`；ADR-086 D1/D2 | 高
- R-B-24 | 单元级发布 MUST 不改选根；只改「拿到一根之后」在根回执下落地局部清单。 | ADR-080 实施约束 6；`model_refresh.rs:126-131` | 高

### §P 变化分类、窗口计划与级联

- R-P-1 | `OperationImpact` MUST 为三态 `Regen | TransformOnly | Skip`；归约先 `impact_regen` 再 `impact_transform`，否则 `Skip`。 | `src/data_interface/model_impact.rs:69-76,99-106` | 高
- R-P-2 | `TRANSFORM_ONLY_ATTR_NAMES` MUST 仅为 `POS`/`ORI`；`BFORI` MUST NOT 进便宜路径。 | `model_impact.rs:110-118`；`dchc_change_classes.json:196` | 高
- R-P-3 | `classify_operation_impact` MUST 只调用 `classify_operation_effects(op).impact()`；`max_dchc` MUST 只记录、不参与分桶。 | `model_impact.rs:87-88,711-736,748-749`；`model_update_plan.rs:325-333` | 高
- R-P-4 | `raw_dchc_code` MUST 强制 `REDRAW→4`、`INTUBE→1`，其余查嵌入字典快照（注释 4270 条）。 | `model_impact.rs:344-355,2304-2315` | 高（条数来自注释）
- R-P-5 | 「不影响模型」MUST 过 WNOEVT 短路，或两票：`DCHC==0`（戳 `CACHID`/`SPAMAP` 例外）∧ 生成器不读；未知/UDA MUST 保守 `Regen`。 | `model_impact.rs:9-13,560-567,655-667,699-730` | 高
- R-P-6 | 元数据门 `model_skip_metadata_only_changes` 默认 MUST 开；开时 `plan_update_with(change_metadata_tier)`，关时逐位 `plan_update`。 | `options.rs:2234-2271`；`window_root_plan.rs:1468-1490` | 高
- R-P-7 | （GitHub #76）门放行且档为 `MetadataOnly`/`UnreadAttribute`、变属性不含戳时 MUST 不做闭包种子（`metadata_only_change_seeds_no_root`）；纯戳/`CacheStamp` MUST 仍做种子。 | `window_root_plan.rs:380-403,1495-1512` | 高
- R-P-8 | 1272 无几何 noun 门 `noun_is_no_geometry_model_entry` 默认 MUST 关；未知 noun 返回 false；`WORL/SITE/ZONE/FTUB` 与 loop 容器 MUST 排除。 | `model_impact.rs:752-772,1032-1052`；`options.rs:2210-2228` | 高
- R-P-9 | DESI 窗口 MUST 调 e3d-model `collect_window` + `plan_update(S→T)`（或 `plan_update_with`）再 `affected_closure`；选根只读文件 `DbSet@S/@T` 与账本。 | `window_root_plan.rs:1-10,48-51,1438-1512` | 高
- R-P-10 | `touched` MUST 先取 `(roots_S ∪ roots_T) ∩ closure`，再由 C2/C8/P4 `expand_touched` 并入。 | `window_root_plan.rs:1744-1751,1870-1880` | 高
- R-P-11 | `xgeometry_observed` MUST 只观察、MUST NOT 筛工作项或凭证前移（DESI 生产路径）；PE/CATA 路径 `xgeometry_gates` 才按开关筛。 | `window_root_plan.rs:690-692,1406-1434,2300-2303`；`manual_update.rs:579-595`；`model_update_plan.rs:961-991`；`options.rs:2153-2172` | 高
- R-P-12 | 结构连接级联 C2（`ADSTCH` 同形，`JOIS/JOIE/CREF/CFRA` 正向读 + 回指 + `STRU` 门）默认 MUST 关；关 = 零 I/O。 | `structural_connection.rs:1-24,18,112,236,573`；`options.rs:2385-2409`；`window_root_plan.rs:1649-1660` | 高
- R-P-13 | 电缆端点级联 C8（`ELCXRF`/`CLIST` → `STAREF`/`ENDREF`，点查文件表、不读 `ref_rev`）代码默认 MUST 开（2026-10-17）。 | `cable_endpoint.rs:1-27`；`options.rs:2415-2444` | 高
- R-P-14 | 设计引用级联 P4 默认 MUST 开；行为名单仅 `POSREF`（其余 kind-16 只记账）；反查失败 MUST 置 `cascade_degraded` 并放弃凭证前移；权威源是文件反向表（`reverse_reference.rs`），选根 MUST NOT 读 `ref_rev`。 | `reverse_reference.rs:1-6,721`；`options.rs:2450-2479`；ADR-063 D1 | 高
- R-P-15 | ADR-080 D10 分桶 MUST 三条级联全开；任一关则整窗不点名 `no_model_only`；五条件：未退化、settled、直接命中且非级联目标、有候选、候选全 `NoModel`。 | `window_root_plan.rs:1826-1846,1992-2003,2048-2052`；测试 `:3741` | 高
- R-P-16 | 刚体分桶 MUST 以 P4 开为前置（`rigid_motion_cascade_off`）；级联扩进的根一律不走便宜路；刚体开关 `model_transform_cheap_path` 默认开，关则 `classify_transform_roots` 空返回。 | `window_root_plan.rs:1754-1757,1797-1806`；`rigid_motion.rs:90-91,107-110,143-147`；`options.rs:2310-2342` | 高
- R-P-17 | `republish_gate` MUST：`receipt == T → SkipAlreadyAtTarget`；`receipt ≤ S → Proceed`；否则 `FallbackBaselineMismatch`。 | `rigid_motion.rs:367-399`；ADR-089 D7 | 高
- R-P-18 | 设计窗之外任一格（目录依赖会话 / 模板 / 生成器指纹）变了，便宜路 MUST 回退 regen。 | ADR-066 追记；ADR-080 D10；ADR-089 | 中
- R-P-19 | 生产 DESI 上 `transform` MUST 是 `regen` 的子集加提示（`transform_units`）；`work_items`/`units` 不改动作。 | `model_update_plan.rs:144-147`；`window_root_plan.rs:1909-1911,2362-2365` | 高
- R-P-20 | `ModelWorkAction` MUST 恰 8 变体：`RegenRoot`/`Transform`/`DeleteCleanup`/`CascadeExpand`/`PostRegenAabb`/`RoomRecalcElement`/`RoomRecalcPanel`/`RoomOnly`；`insert_item` 去重键 `(action, target_refno)`。 | `model_update_plan.rs:21-36,191-196,1130` | 高
- R-P-21 | `topmost_deleted_refnos` MUST 沿 owner 上溯最多 `HOP_CAP=32`；祖先也在删除集则丢子；owner 解不出 MUST 保留。 | `model_update_plan.rs:252-279` | 高
- R-P-22 | DESI 生产 MUST 走 `build_model_update_plan_from_window`；CATA/PE 走 `build_model_update_plan`（CATA 只产 `CascadeExpand`，执行体 live 仍查 `ref_rev`）；其它库空计划。 | `window_root_plan.rs:2140-2151,2448-2473`；`model_update_plan.rs:836-901`；`manual_update.rs:2163-2176` | 高
- R-P-23 | loop/点容器的 `POS` MUST 判 `Regen`（属主网格输入，不是实例变换）。 | `model_impact.rs:721-724` | 高
- R-P-24 | 窗口规划器 MUST 复用 `WINDOW_PLAN_CACHE`（`CAPACITY=8`，命中刷新、同文件新窗替旧窗），避免同窗二次 `collect_window`+`plan_update`。 | `fast_model/e3d_model_service.rs:188-189,318-338,356-395,945-956` | 高

开关表（代码默认 / `DbOption.toml` 现值）：`model_skip_metadata_only_changes` 开/未写；`model_entry_drop_no_geometry` 关/未写；`model_design_reference_cascade` 开/`true`；`model_structural_connection_cascade` 关/未写；`model_cable_endpoint_cascade` **开**/`true`（文档仍写关）；`model_transform_cheap_path` 开/`true`；`model_unit_level_publish` 开（`effective_default_on_switch`）/`true`；`model_xgeometry_gate` 关/未写；`model_significant_over_point` 关。出处 `options.rs:2153-2522`、`DbOption.toml:213-234`。

### §C 单元级发布与按需版本

- R-C-1 | 单元模型版本 MUST 从会话链按需算出，只进进程内缓存（键 `(dbnum, unit_refno, 文件指纹)`），MUST NOT 落库。 | `fast_model/unit_versions.rs:1-6,237-246`；ADR-081 D3 | 高
- R-C-2 | `GET /api/v1/model/versions` 的 `refno` MUST 是 `delivery_unit_types` 下的单元根；否则 `422 NOT_A_DELIVERY_UNIT_ROOT`。 | `unit_versions.rs:700-720`；ADR-081 D1 | 高
- R-C-3 | 一窗 `impact_kind` MUST 与生成路共用 `classify_operation_impact`：单元根 `Add`→`delivery`、`Deleted`→`tombstone` 压过一切；否则成员最重 `Regen`→`mesh` / `TransformOnly`→`placement` / 全 `Skip`→`noop`；一窗没碰到 MUST NOT 成为版本。 | `unit_versions.rs:8-10,160-194`；ADR-081 D2 | 高
- R-C-4 | `element_versions` / `node_versions` MUST 同一纪律（按需算、进程内缓存）；`node_diff_summary` MUST 只做 `A→B` 索引差分、不生成几何、不落库，`needs_confirm` 由 `MODEL_HISTORY_CONFIRM_UNITS`（缺省 20）门。 | `element_versions.rs:1-14,50-57`；`node_versions.rs:1-14,51-62`；`node_diff_summary.rs:1-11,165-176`；ADR-081 追记 | 高
- R-C-5 | `CommitScope::Partial` MUST 自报「缺席什么也不证明」；要删 MUST 显式进 `removals` / `tube_branch_resets`；拿缺席推删除 MUST 只有 `whole_root_removals` 一处（`WholeRoot`）。 | `model_memory_store.rs:222-247`；`e3d_model_service.rs:4093-4098`；ADR-080 D2 | 高
- R-C-6 | `Partial` 回执的 `manifest_hash` / `geometry_count` MUST 由存储在写锁里、落完后 `stamp_partial_receipt` 按 `record_ids_under(root)` 重算覆盖；`WholeRoot` 回执原样入库。 | `model_memory_store.rs:241-247,615-628,782-797`；ADR-080 D5 | 高
- R-C-7 | 单元级发布 MUST 只重建 `plan_update(S→T)` 点名的单元（`GeometryId`），以 `Partial` 叠在根级 `RootReceipt` 下；`published_via="unit"` 只观测。 | `e3d_model_service.rs:2734-2744,4237-4255`；ADR-080 D1/D5 | 高
- R-C-8 | 执行体顺序 MUST 为 完整性前置 → `republish_gate` → `produce` → 管段门 / 空清单守卫 → 整窗一次 `publish`。 | `e3d_model_service.rs:4151-4261`；`rigid_motion.rs:369-399`；ADR-080 D4 | 高
- R-C-9 | 单元路任一失败（`full_rebuild` / 读挂 / 设计窗外目标变 / `execute_plan` 失败 / removal 回溯失败 / 管段门不过 / 投影残缺）MUST 整根回退 regen，MUST NOT 发半截清单。 | `e3d_model_service.rs:815-821,4197-4203`；ADR-080 D8 | 高
- R-C-10 | 空 `Partial` MUST 只来自 `no_model_only` 根（D10）；其它空 delta MUST 回退；删空根 MUST NOT 走 `Partial`。 | `e3d_model_service.rs:4205-4235,4284-4287`；ADR-080 D10/D2 | 高
- R-C-11 | durable 落盘第一版 MUST 仍从 `records_of_root` 整根刷（D7）；对账 `record.source_sesno <= receipt.source_sesno`（D3）。 | `batch_worker.rs:3899-3900`；`model_db_adapter.rs:2143-2167`；ADR-080 D3/D7 | 高
- R-C-12 | worklist MUST 定死优先序：刚体前移 → 单元级 → 整根 regen；开关关时对应桶空。 | `batch_worker.rs:4485-4530`；`options.rs:2357-2358`；ADR-080 实施约束 7/9 | 高
- R-C-13 | 刚体前移 MUST 对「投影停在 S 或之前」的根把 `records_of_root` 全量左乘 `Δ` 后 `WholeRoot` upsert（省网格、不省记录写入）；设计窗外目标变 / 网格读不到 MUST 回退。 | `e3d_model_service.rs:747-766,2565-2728`；ADR-080 背景 | 高
- R-C-14 | `RootPublicationAck::matches` MUST 只比 `source_sesno` / `geometry_count` / `manifest_hash` / `model_target`。 | `model_memory_store.rs:206-214`；ADR-080 D4 | 高
- R-C-15 | 几何 `< MIN_RECORDS_FOR_UNIT_PUBLISH(10)` 的根（非 D10）SHOULD 回退整根 regen。 | `e3d_model_service.rs:841-870,949`；ADR-080 文首 2026-10-14 | 高
- R-C-16 | 房间快照：`rooms_before` 开工前拍（读 `SUL_DB` 边目标）；`rooms_after` MUST 在 `await_units_durable` 与房间重算之后补；「后」拍不到 MUST 连「前」一起摘。 | `unit_room_snapshot.rs:1-11`；`batch_worker.rs:1896-1919,4463-4470,4728-4736` | 高
- R-C-17 | 几何快照前后都走 `records_of_root`（投影优先）；`geometry_after` 在 worklist 收尾拍，不必等 durable；一批 `> MAX_ROOTS(64)` 或读透 MUST 不拍。 | `unit_geometry_snapshot.rs:1-12,70-75`；`batch_worker.rs:4472-4474,4738-4753` | 高
- R-C-18 | `model_unit_level_publish` / `model_transform_cheap_path` 未配置时 MUST 为开；`AIOS_MODEL_UNIT_LEVEL_PUBLISH=0` 可关回。 | `options.rs:2335-2342,2369-2378,2482-2489` | 高

### §D 模型行删改、切片删与几何指针

- R-D-1 | 有回执的根 MUST 用谓词整片删（`render_slice_delete`）再 `WriteMode::Initial`（只 `INSERT IGNORE`）；禁止再按本批 id 点删。 | `fast_model/model_db_adapter.rs:1893-1943,1997-2001,2185-2238`；ADR-088 D1 | 高
- R-D-2 | 无回执的根 MUST NOT 整片删；MUST 用 `WriteMode::Incremental`（先按本批 `record_id` 删整组表示再插）；缺席不得解释成删除。 | `model_db_adapter.rs:2179-2240,2337-2344,2571-2578`；测试 `:4949-4957` | 高
- R-D-3 | 整片删键集 MUST 是两路并集：`anc CONTAINSANY 根 AND model_sesno != NONE`，以及同根 `model_sesno = NONE AND aabb != NONE AND world_trans != NONE`；两路分开 `LET`，禁止合成 `OR`。 | `model_db_adapter.rs:844-854,888-893`；ADR-088 D1/D6 | 高
- R-D-4 | 同根带几何指针的 legacy `inst_relate` MUST 按同键整组替换（关系行 + `inst_info:<key>` / `aabb:<key>` / `trans:<key>` + `geo_relate` 按 `in`）；legacy 自己的摘要键指针（`booled_id` / `inst_geo` / 摘要 `aabb`/`trans`）MUST NOT 跟随删除。 | `model_db_adapter.rs:848-851,896-904`；ADR-088 D2；测试 `:3460-3468,3503-3578` | 高
- R-D-5 | 没有几何指针的属性面行 MUST 一字不动。 | ADR-088 D3；测试 `model_db_adapter.rs:3530,3571,3582-3607` | 高
- R-D-6 | `slice_sources_on` MUST 同步读模型面行与带指针 legacy 行，名单只喂房间清边 / 空间树摘条目，不用来点名删行。 | `model_db_adapter.rs:790-832,1959-1974`；ADR-088 D4 | 高
- R-D-7 | 删行 MUST `DELETE $数组` 点名记录；`aabb`/`trans`/`inst_info`/`inst_relate` 禁止 `DELETE … WHERE` 谓词；`geo_relate` 是唯一先 `SELECT id` 再删的谓词句。 | `model_db_adapter.rs:856-863,884-904` | 高
- R-D-8 | 元素的 `aabb`/`trans`/`inst_info` 与 `inst_relate` 同键（`collect_record`）；管身 `tubi_relate` id 是 `[pe, 槽位]`，盒/变换按几何记录 id 另键，MUST 顺指针删、禁止从槽位 id 拼。 | `model_db_adapter.rs:16-18,873-882,2664-2729` | 高
- R-D-9 | prelude 顺序 MUST 是 `render_slice_delete` → `append_tube_branch_reset` → `render_tube_identity_sweep` → `render_room_edge_clear`。 | `model_db_adapter.rs:1997-2009,2244-2256`；测试 `:3408-3432` | 高
- R-D-10 | `append_tube_branch_reset` MUST 按 `in = pe:⟨branch⟩`（`idx_tubi_relate_in`）先捞指针再删盒/变换，再按 id 删全部 `tubi_relate`；禁止 `pe->tubi_relate` 图遍历。 | `model_db_adapter.rs:3041-3056`；测试 `:3303-3327` | 高
- R-D-11 | 本轮要插的管身 MUST 再按身份点删 `aabb:⟨record_id⟩` / `trans:⟨record_id⟩`（`render_tube_identity_sweep`）。 | `model_db_adapter.rs:3059-3088`；测试 `:4883-4946` | 高
- R-D-12 | 模型面写入只有两档、零 UPDATE/UPSERT：`INSERT IGNORE` / `INSERT RELATION IGNORE`；`INSERT IGNORE` 撞 id 保留旧行，所以「先删」是正确性前提。 | `model_db_adapter.rs:2335-2344,2601-2635`；ADR-060(行id) D4/N3 | 高
- R-D-13 | `Incremental` 的按 id 删 MUST 覆盖 `geo_relate`/`inst_info`/`inst_relate`/`aabb:<id>`/`trans:<id>`；MUST NOT 删 `inst_geo`、共享本地盒、`.mesh`、`tubi_relate`。 | `model_db_adapter.rs:3102-3142`；测试 `:4032-4042`；ADR-060 D2/N2 | 高
- R-D-14 | 有回执根的生产落库 MUST 走 `persist_roots_batched` / `persist_root_on`（`model_refresh` 直写与 write-behind），不是 `pdms_inst::save_instance_data`；禁止逐根 `persist_root(` 循环（ADR-071）。 | `model_db_adapter.rs:1-6,932-938`；`model_refresh.rs:520-541` | 高
- R-D-15 | 镜像 spool / 墓碑清理 MUST 与整片删同一套键规则并多一路 `in = $mirror_owner`；`render_spooled_scope_deletion` MUST 渲 cleanup + `render_tube_branch_resets` + `spatial_epoch` bump，包进事务；共享 `.mesh` 不删。 | `model_db_adapter.rs:2496-2559`；ADR-088 D5；测试 `:4504-4534` | 高
- R-D-16 | 初始化直写 MUST `WriteMode::Initial`、不做整片删、不读 `slice_sources_on`；撞上带指针的 legacy 行本次不改（留给第一次有回执的增量）。 | `fast_model/model_initial_persist.rs:6-8,108`；ADR-088 D7；ADR-077 D4 | 高
- R-D-17 | 整库 wipe 的 `RANGE_TABLES` MUST 含 `inst_relate`/`tubi_relate`/`geo_relate`/`inst_info`/`aabb`/`trans`/`pe_ptset` 等按 Ref0 区间可圈的表；`aabb:<mesh hex>` 落不进区间、只增不删；`inst_geo` 不在清单。`render_model_face_wipe` 去掉 `pe`/`ref_rev`；`Purge` 时生成锁内、删行前收键、删行后 `purge_geometry`。 | `data_interface/fast_delete.rs:15-44,600-640,865-918`；测试 `:1783-1800,1887-1911` | 高
- R-D-18 | 增量落库共享网格 MUST 只增不删；显式 `mesh_purge` 时：单位键 `1`/`2`/`3` 永不删；`tubi_relate.out` 与投影管身键 MUST NOT 进收集集；其余本范围 baked `inst_geo`+`.mesh` 按用户口径删除、靠重生成补回、不做跨范围引用计数。 | `mesh_purge.rs:14-27,108-119,154-156,261-276`；ADR-060 D2/N2；`docs/plans/2026-10-22-mesh-purge-*.md` D1/§7.1 | 高
- R-D-19 | `clear_model_scope` / `delete_model_subtree` MUST：删投影 + 走删除入口 `data_interface::projection_removal` + spool 墓碑 + `delete_inst_relate_subtree`；`Purge` 时收键→删行→清几何。2026-11-08 ADR-097 起删除只有一个入口。 | `on_demand_model.rs:108-143,277-345` | 高
- R-D-20 | 祖先根整片删圈的是 `anc CONTAINS 根`（含嵌套根行）；嵌套根行能活下来当且仅当本轮投影把它们重新插入。 | 测试 `model_db_adapter.rs:9013-9073`；conformance `:841-866` | 高
- R-D-21 | `tubi_relate` 整片删谓词仍只认 `model_sesno != NONE`（legacy 管身本次不圈）。 | ADR-088 D7；`model_db_adapter.rs:894-895` | 高
- R-D-22 | 行 id 就是记录 id（`<word0>_<word1>`，无 `direct_`/`world_`/`hist_` 前缀）；共享图元本地变换写 `geo_relate.transform`；SQL 值走 `sql_string()`、id 走 `bracketed_id()`。 | `e3d_mesh_store.rs:105`；`model_db_adapter.rs:3271,3281`；ADR-060(行id) D1/D3/D10 | 高
- R-D-23 | （W2/W3 本轮）`pe_ptset` 行 MUST 在同一发布事务里随根落地：增量 prelude `DELETE (SELECT VALUE id FROM pe_ptset WHERE root = pe:⟨r⟩)` 后 UPSERT；初始化 `render_root` 直接追加；整库 wipe / 回滚随 `RANGE_TABLES` 与 `sesno > watermark` 删除；不做异步 backfill。 | `fast_model/ptset_rows.rs`；`model_initial_persist.rs`；`fast_delete.rs`；`resource/surreal/gy_common.surql`（`pe_ptset` / `fn::pe_ptset_of` / `fn::bran_comp_len`） | 高

### §G 房间边与 room_membership

- R-G-1 | 一个生成根完成 MUST 是「最终几何 → 点包含筛房间归属 → `inst_relate`/`tubi_relate` 与两张房间边同事务落库」；任务末 MUST NOT 再有整库房间段。 | ADR-073 D1；`fast_model/room_model.rs:2615,2654` | 高
- R-G-2 | 房间算不出或写不进 MUST 只判该根失败；MUST NOT 用第三枚 `room_sesno`。 | ADR-073 D2；`model_db_adapter.rs:725,1605` | 高
- R-G-3 | `room_relate` / `room_panel_relate` 写边 MUST 先 `DELETE` 边目标再 `INSERT RELATION`，同事务；空成员集仍只删不插。 | `room_model.rs:175,213,427`；测试 `:3339,3518` | 高
- R-G-4 | 逐根渲染 MUST 按构件 `DELETE {element}<-room_relate` 再插；面板再写 `room_panel_relate`；根里没有几何的构件边由对账清边清。 | `room_model.rs:2656`；`model_db_adapter.rs:104,135,2008` | 高
- R-G-5 | 在册但无可用几何的面板 MUST 进保护集（`$room_protected_panels`），MUST NOT 把「没算」写成「空成员」。 | ADR-073 D9；`room_model.rs:2663,2966` | 高
- R-G-6 | 缺 DESI / 面板缺几何 MUST 降级 `Degraded` 仍冻计算器；`ensure_ready` 只拦 `Loading`/`Blocked`。 | ADR-073 D8–D10；`room_resident.rs:2430,2523`；`model_db_adapter.rs:969` | 高
- R-G-7 | 常驻房间模型是算法输入缓存，不是第二份真值；归属入口 MUST 只经 `ResidentRooms::for_membership`。 | `room_resident.rs:1,1828,1931` | 高
- R-G-8 | 拓扑来源 MUST 是冻结 E3D 文件（hd=`FRMW` 子孙 `PANE`，hh=`SBFR` 直接子 `PANE`），不查库 noun 表；点包含 MUST 用唯一谓词（AABB 八顶点全在内则成员，否则射线奇偶三取二）。 | `room_topology.rs:1,8`；`room_predicate.rs:1,10,31` | 高
- R-G-9 | 每个 dbnum 任务开工前 MUST 把常驻对齐到此刻文件 composite；一任务 MUST 冻一台 `TaskRoomCalculator`；同一文件版本下重建连续失败 SHOULD 指数退避（30s 起、10min 封顶）。 | ADR-073 D12；`room_resident.rs:2944,3029-3041,3114`；`model_db_adapter.rs:990` | 高
- R-G-10 | 2026-10-23 起 `room_membership` / `AIOS_ROOM_MEMBERSHIP` MUST 不再裁决；能算房间的进程恒开（`spatial_compute && project_hd|hh`）；写了只点名。 | `options.rs:107-110,2013-2037,2076` | 高
- R-G-11 | 面板侧变化 MUST 在刷盘 ACK 之后、收尾之前重写边；常路定向（`room_reverse` 三路命中：全局树 + 投影管身 + 库上 `tubi_relate`，`PANE` 剔掉），树不就绪 / 折不出根退整库；跨库命中进程内停放。 | `room_recompute.rs:1,26`；`room_reverse.rs:1,7,24`；ADR-073 D13；`batch_worker.rs:1680` | 高
- R-G-12 | durable `dbnum_watermark.model_room_generation` MUST 只由能替整库说话的任务盖 `Aligned`；覆盖不全 MUST 不盖。 | ADR-073 D14；`room_recompute.rs:125`；`batch_worker.rs:1703` | 高
- R-G-13 | 读透点查 MUST 用当前投影现算、不读库存边；输入变了 MUST `RoomNotReady`；无生成根回 `not_computed`。 | `room_read_through.rs:1,18,109` | 高
- R-G-14 | 启动对齐：无 `RoomUpdateRecord` / 代号变了 MUST 全量初始化且不看 `watch_scope`；同代只对过期 dbnum 做窗口增量。 | `room_recompute.rs:852,943,1393`；`docs/verification/2026-10-15-room-rebuild-regressions.md:102` | 高
- R-G-15 | 被对账掉的来源两张房间边 MUST 跟删几何同一 prelude；清边与重算块 MUST NOT 对调。 | `model_db_adapter.rs:63,2008`；conformance `:655` | 高
- R-G-16 | bake 路径 SHOULD 把边装进 `RootReceipt.room`（`RoomEdgeSource::Carried`）；闸不过 MUST 在 flush 现算（`Recomputed`）；ACK 匹配后 MUST 摘 `receipt.room`。 | ADR-073 D1 追记；`model_db_adapter.rs:684,731`；`model_memory_store.rs:1773-1795`；ADR-084 D5 | 高

### §E 缓存与驱逐

- R-E-1 | overlay `Current` 作用域 MUST 按有回执根数封顶（`model_overlay_max_roots` / `AIOS_MODEL_OVERLAY_MAX_ROOTS`，默认 `50_000`，0 = 不限）。 | `options.rs:1073,1087-1109`；`model_memory_store.rs:985-988`；ADR-082 D1/D5 | 高
- R-E-2 | 超限 MUST 只逐出「有回执 ∧ `ack.medium.is_durable()` ∧ `ack.matches(receipt)`」的根；候选用尽仍超限 MUST 停手；stray 房间证据或欠房间边的 ACK MUST 不进候选。 | `model_memory_store.rs:716-763,1431-1435`；`model_write_behind.rs:808-811`；ADR-082 D1 | 高
- R-E-3 | overlay LRU 键 MUST 是 `commit_ticks`（最近一次盖回执的单调序号）；读路 MUST 不刷新。 | `model_memory_store.rs:362-365,637-639` | 高
- R-E-4 | 驱逐时机 MUST 归 `model_write_behind`：`publish_commits` 之后与 `persist_batch` 记下 durable ACK 之后；仅 `store_mode().is_durable()` 且 `limit > 0` 时动；kv-mem 禁止逐出。 | `model_write_behind.rs:789-807,996-997`；`options.rs:1275-1277` | 高
- R-E-5 | `still_over_limit` MUST 只报数字、不拒写；逐出 MUST 走 `drop_root`（记录 `evict`，不当生成删除、不进房间变化账）。 | `model_memory_store.rs:545-548,644-667,845-849,1206-1208,1281-1307`；ADR-082 D1 | 高
- R-E-6 | 生成用 `DbSet` 元素缓存 MUST 按元素数封顶（`model_element_cache_max_entries` / env，默认 `200_000`，0 = 不限）；满了 MUST 整张清（非 LRU）；每份 `DbSet` 各算。 | `options.rs:1078,1112-1128,2502-2517`；`e3d_model_service.rs:3484-3493`；ADR-082 D4 | 高
- R-E-7 | `GENERATION_INPUT_CACHE`（`DbnumLru`）MUST 按不同 dbnum 个数封顶（默认 0 = 不限）；键 `(dbnum, sesno, identity)` 每库一条；命中刷新；同库替换不计 `evicted_dbnums`；刚装入不被本次逐出；被逐值锁外 drop；`forget_generation_input` 不计入。 | `e3d_model_service.rs:1599-1680,1736-1763`；`options.rs:1079-1147`；ADR-084 D1–D3 | 高
- R-E-8 | `cata_closure_cache` MUST 条目+字节双上限 LRU（默认 `200_000` 条 / `256 MiB`，硬编码，不进 options）；键 `(project, dbnum, refno)`；库 `CatalogueIdentity` 变了 MUST 整库 `retire`。 | `cata_closure_cache.rs:9-10,71-78,155-231,247-257`；`cata_closure.rs:1462-1478` | 高
- R-E-9 | `model_file_identity` 缓存键 `(StableFileId, session)`，`IDENTITY_CACHE_CAP=4096`，满了整表清。 | `frozen_source.rs:461-498` | 高
- R-E-10 | 读透元素缓存是 SurrealDB 行缓存（`read_through_cache:{dbnum}` + `cache_epoch`），epoch 变更 MUST 落在第一笔成功数据事务里；CATA / 外库 refno MUST 不写；调用方拿到 `Err` MUST 不拦模型发布。 | `read_through.rs:1-24,96-114,545-550` | 高
- R-E-11 | `Historical(*)` 按格数封顶（`model_historical_snapshots_max`，默认 16）；刚发布这格永远留；`Historical` 不进 durable。 | `options.rs:1083-1165`；`historical_model.rs:112-122,392-438,1037`；ADR-081 D4 追记 | 高
- R-E-12 | 数量上限裁决 MUST 为：合法非负 env > 配置键 > 代码默认；拼错 env 当没设；任一层 0 = 不限。 | `options.rs:1168-1178`；ADR-082 D5 | 高
- R-E-13 | write-behind MUST 永不把 RocksDB 当 cache-fill 源；冷根读路直读 RocksDB、不回填 overlay；`Blocked` 根每 600s 重试。 | `model_write_behind.rs:3-5,34-43,512-515,685-693`；ADR-082 D2/D3 | 高

### §H MDB 声明、项目发现与文件异常

- R-H-1 | 项目集合 MUST 由主项目 SYS 里该 MDB 的 CURD 按 `PROJ` 码派生（空码 = 主项目）；配置不得再写第二份名单。 | `mdb_closure.rs:1-7`；ADR-078 D1 | 高
- R-H-2 | 项目码 → 目录 MUST 只在 `[project_path] ++ project_search_paths` 里按 `<code>000` 三种摆法找；不读 evars、不执行 `.bat`。同根同码 → 该根不给答案；跨根同码 → 先到的赢、其余 `shadowed`。 | `project_locator.rs:1-24`；`mdb_closure.rs:702-708`；ADR-078 D2–D3 | 高
- R-H-3 | 根不可达 MUST 只让该根缺席（独立线程 + 15s 超时）；`unresolved` MUST NOT 拒绝启动，只进 `degraded_sections`。 | `mdb_closure.rs:17-18,466-471,548-551`；ADR-078 D4/D7 | 高
- R-H-4 | `included_projects` MUST 退役（非空只打「已忽略」）；`catalogue_project_priority` 只裁决未声明依赖库同号跨项目的数据面正本，不决定模型面读哪份（`CatalogueFileChoice` / `select_dependency_file`）。 | `mdb_closure.rs:840-859`；`e3d_model_service.rs:1320,3655`；ADR-078 D5 及 #47 追记 | 高
- R-H-5 | 声明（`MdbDeclaration`）MUST 未绑定文件；绑定只在声明项目内按 dbnum 选文件，禁止用外项目同号顶替。 | `mdb_closure.rs:37-38`；`mdb_membership.rs:16-18,525-529,1758-1759` | 高
- R-H-6 | 本期执行范围 MUST = 已登记成员的 `element_databases()`（`DESI|ISOD`）；声明了却没绑到文件的库仍在范围里；配置 MDB 的 `UpdateScope::fetch` 只读 `mdb_membership::get`。 | `update_scope.rs:1-15,88-123,266-274`；ADR-085 D1 | 高
- R-H-7 | 声明缓存 MUST 只在 SYST 批次 `applied` 时 `reload_declaration` 再 `rebind(SystRefresh)`；读失败留旧声明并写入该批 `warnings`；成员绑定只在启动 / SYST 刷新 / 重挂补目录三处重做；`phase=mdb_scope` 只由 `rebind` 打出。 | `mdb_closure.rs:136-140`；`mdb_membership.rs:20-26,1884-1914,1976-2016`；`batch_worker.rs:3589-3621`；ADR-085 D2/D3/D5 | 高
- R-H-8 | 执行范围 TTL 缓存 MUST 保持退役（`SCOPE_CACHE` / `resolve_cached` / `AIOS_SCOPE_CACHE_SECS` 不得长回）。 | `update_scope.rs:207-211,630-642`；ADR-085 D4 | 高
- R-H-9 | SYS meta（`SYST/DICT/COMM/MISC/GLB/GLOB`）MUST 绕过 MDB 门与 `watch_dbnums`；CATA MUST NOT 进 `UpdateScope::admits`（`unrestricted` 除外）。 | `update_scope.rs:256-274`；`sesno_range.rs:23`；`watch_scope.rs:152-154`；`increment_manager.rs:2926-2937` | 高
- R-H-10 | `watch_dbnums` / `--watch-dbnum` MUST 只圈 watcher 扫出来的数据批次与本进程模型工作单，不改 MDB 声明；`UpdateScope::unrestricted` 不被监听门挡；HTTP `excluded` MUST 表示「MDB 没声明这个 DESI」，不得与 `blocked` / `watch_dbnums` 合成一行。 | `watch_scope.rs:1-31,123-142`；`increment_manager.rs:2910-2965`；`web-service-api.md:855-858` | 高
- R-H-11 | 路由目录 MUST 按 CURD 序建 `dbnum→文件`；重复声明保留先到的、后到的记 `problems`；无文件进 `declared_without_file` 并报 `NoFileForDbnum`（带项目码）。 | `mdb_route_directory.rs:387-461,579-596,829-835` | 高
- R-H-12 | 同项目多文件 MUST 经 extract family：最高 `_NNNN` 叶为工作文件，母库与更旧叶 `shadowed`；master+leaf MUST NOT 判 `Duplicate`。 | `mdb_membership.rs:1801-1849`；`increment_manager.rs:1288-1290,3190-3194` | 高
- R-H-13 | 文件异常 MUST 走唯一入口 `DbnumState::classify_scan` → `check_file_against_state`；顺序：非身份路径 → `NameChanged` → sesno 回退 → 类型变更 → 同类型换路径 `PathMigrated`。七种 `FileAnomaly`：`Rollback/PathMigrated/NameChanged/TypeChanged/Duplicate/Missing/ForeignProject`。 | `dbnum_state.rs:549-613,810-891,1627-1631`；`increment_manager.rs:3246-3261,3537-3546` | 高
- R-H-14 | `PathMigrated` MUST 放行并更新登记路径；`Rollback` MUST 转整库重建（`ScanGate::Reinit`）；`TypeChanged`/`Duplicate`/`Missing`/`ForeignProject`/`NameChanged` MUST 阻断且不回退水位；`NameChanged`/`ForeignProject` 连观察值都不写。 | `dbnum_state.rs:616-695`；`increment_manager.rs:3251-3260`；ADR-021 | 高
- R-H-15 | 路由轮询层 MUST 用 `FileIdentity(len,mtime)+latest_sesno+BASESN` 分类 `GenerationChange::{SessionAdvanced, RolledBack, FileReplaced}`；身份没变不开文件。 | `db_generation.rs:1-23,117-204`；`mdb_route_directory.rs:185-204,658-674` | 高
- R-H-16 | 移出 MDB 的元素库 MUST 只点名不自动清；人工出口 `DELETE /dbnums/{dbnum}/data`。 | `mdb_membership.rs:1942-1972`；`startup_reconcile.rs:230`；ADR-078 追记 #41 | 高
- R-H-17 | CURD `DBSET` MUST 按 `DBL/DBF`、`DBSTL/DBSTF` 展开；环只进一次；`DBCRITERIA` MUST 报问题而不是静默近似。 | `mdb_membership.rs:11-14,702-716` | 高
- R-H-18 | 重挂健康闸：仅 `has_watch_gaps()` 时才重扫搜索根；全健康拍 MUST 零搜索根 IO。 | `mdb_closure.rs:540-551,1140-1204`；ADR-085 D3 2026-10-15 追记 | 高
- R-H-19 | 生成 resolver MUST 只认 MDB 声明的库；`RouteOrigin::Dependency` 的 CATA/PROP 只服务数据面；未声明库前移 MUST NOT 级联模型根。 | `mdb_route_directory.rs:94-110,144-148,483-486`；ADR-078 #46 追记 | 高
- R-H-20 | 文件身份 MUST 以冻结锚（`FrozenSource::identity_digest` / `model_file_identity`）为准，不记 mtime/长度；`model_target` 走锚。 | `frozen_source.rs:420,470`；`e3d_model_service.rs:3320-3330`；ADR-089 D2/D3 | 高

### §O 发现、队列与调度

- R-O-1 | 文件事件 MUST 只当脏位：`mpsc::channel::<()>(1)` + `try_send`，满则丢；真正判定 MUST 走完整清单重扫（水位）。 | `increment_manager.rs:4997-5056,5114-5127,5213-5229` | 高
- R-O-2 | `WatchSweepGate` MUST 攒脏位、`take_due` 未到期保留、`wait_until_due` 排到期唤醒；默认 poll 30s（`AIOS_WATCH_POLL_SECS`）、对账 300s（`AIOS_WATCH_RECONCILE_SECS`，0 关）、最小重扫间隔 5s（`AIOS_WATCH_MIN_SWEEP_GAP_SECS`）。 | `increment_manager.rs:3072-3091,3145-3181` | 高
- R-O-3 | 三条自动路径与手动候选扫描 MUST 共用 `is_candidate_db_file`（`should_exclude_file` 黑名单 ∧ `is_pdms_db_file_name` 白名单）。 | `increment_manager.rs:2751-2780,3883-3886,5046-5049` | 高
- R-O-4 | 监听门顺序 MUST `watch_scope_admits` 先于 `debug_scope_admits`；SYS meta 两道门永放行；未配置时恒 `true`。 | `increment_manager.rs:2888-2966` | 高
- R-O-5 | `init_watcher` MUST 只做首轮 `sweep_watch_dirs("init", ensure_archives=true)`；周期/事件/范围刷新复用 `sweep_dirs`；read-through MUST 跳过摄入重扫。`async_watch` MUST 用 `PollWatcher`，积压补偿不由 watcher 做（ADR-011 §2）。 | `increment_manager.rs:3670-3724,4976-4987,5125-5127`；`lib.rs:1278-1289` | 高
- R-O-6 | read-through 路由目录 MUST 每 2s `poll_generations`，变化库只走 `targeted_model_refresh`（只动该 dbnum 的根与回执）。 | `lib.rs:2119-2178,2431-2481` | 高
- R-O-7 | 入队三规则顺序不可换：① 同 dbnum 已有 Queued → 合并且只推高；② 否则同 dbnum Running → `BehindRunning` 另起一行接在冻结右端之后；③ 否则 New。同一 dbnum 队列里最多两行；区间倒挂当 `AlreadyCovered`。 | `batch_queue.rs:104-114,125-278`；ADR-011 D5 | 高
- R-O-8 | `Reinitialize` 对已排队行占优并重置窗口；`ApplyWindow` 落到 `ModelOnly` 排队行 MUST 升意图；`ModelOnly` 落到运行中数据任务 MUST 把右端抬到运行行冻结世代；带 `regen_roots` / `catalogue_stamps` 的 ModelOnly 被吞且无排队行时调度器 MUST 补退化 `[end,end]` 后继。 | `batch_queue.rs:158-255`；`batch_scheduler.rs:445-477` | 高
- R-O-9 | 重扫入队在 `!is_auto_work_armed()` 时 MUST `held=true`；上弦来源为 `startup_autorun || watch_scope_active`（ADR-048）。 | `batch_queue.rs:63-72,116-147`；`increment_manager.rs:4509-4511,4823-4826`；`batch_scheduler.rs:364-427` | 高
- R-O-10 | 出队 MUST 走 `freeze_next_concurrent`：同 dbnum 恒串行；held 不算队首；独占行在飞非空时 `HeadNeedsExclusive`；`paused` 只挡出队，标志持久化。 | `batch_queue.rs:311-357`；`batch_scheduler.rs:325-326,704-744`；ADR-011 D9 | 高
- R-O-11 | 数据批次当前 MUST 一律独占（`batch_needs_exclusive_lane` 恒 `true`，D10-A）；`data_batch_workers` 默认 1、夹到 1..=8，但跨库并行暂不产生。 | `batch_worker.rs:9-12,906-917`；`options.rs:1623-1633` | 高
- R-O-12 | 发现轮装新 epoch 前，若队列里已有任一 `running` 行，`enqueue_discovered` MUST 整轮 return。 | `increment_manager.rs:101-105,4770-4778` | 高
- R-O-13 | 数据批次自动重跑上限 `DATA_BATCH_PARK_AFTER = 1`；失败后重扫不再入队、记 `PhaseBlocker`；复活仅：文件右端前进或 `POST /update/execute`；账本进程内，持久痕迹在 `logs/batch-failures-*.jsonl` 与 `/health.batch_failures`。 | `batch_worker.rs:47-51,431-456,539-544`；`increment_manager.rs:4219-4272`；`initialization_phase.rs:448-486` | 中
- R-O-14 | 模型侧生成失败 MUST NOT 记入数据批次连败账，也没有对称 park（GitHub #72 待拍板）。 | `docs/plans/2026-10-27-open-issues-audit.md:26,41`；`batch_worker.rs:978-979` | 高
- R-O-15 | `AIOS_TEST_FAIL_DATA_ENDS` / `AIOS_TEST_FAIL_ROOTS` 只看环境变量、无 `cfg(test)`，release 进程同样生效；`fail_generations_for_test` 仅 `#[cfg(test)]`。 | `batch_worker.rs:974-990`；`model_refresh.rs:62-86,110-116,227-268` | 高
- R-O-16 | 任务 kind 常量：`data_batch` / `model_drain`（退役兼容）/ `model_rebuild` / `dbnum_model_ensure` / `model_history` / `room_rebuild`；`room_recalc` 已退役。 | `task_registry.rs:18-34` | 高
- R-O-17 | `link_supersession` MUST 只链：同库、更早入队、`Failed`、数据未 `applied`、右端落在后继窗口内、尚未被认领；`partial` MUST NOT 进链。 | `task_registry.rs:188-195,242-252,580-620`；`batch_scheduler.rs:581-626,930-933` | 高
- R-O-18 | 进程内 `MAX_TASKS=1000`：queued/running 永不剔除；收口 MUST `record_history` 落 `logs/task-history-*.jsonl`；不记 `model_drain`。 | `task_registry.rs:36-39,339-357,805-832,1041-1076`；`task_history_log.rs:1-36`；ADR-011 D11 | 高
- R-O-19 | 模型根在飞额度 `effective_root_inflight`：Legacy=1，Bounded=配置 max，Adaptive=30s 窗结算；不创建新线程池。 | `model_concurrency.rs:1-3,110-178` | 高
- R-O-20 | 队列不持久（ADR-011 D4）；durable 在水位；进程内唯一消费者 `ensure_batch_worker`（`OnceLock`）。 | `batch_scheduler.rs:8-9`；`batch_worker.rs:671-696` | 高
- R-O-21 | 常驻房间初始化 `prepare_room_resident_at_startup` MUST NOT 看 `startup_autorun`；`skip_startup_room_build` 已不存在于 `lib.rs`。 | `lib.rs:465-475,514-524` | 高

### §N 按需生成、读路由与 HTTP 面

- R-N-1 | `POST /api/v1/model/generate` MUST 是按需生成正式入口；`/model/ensure` MUST 作为别名转发同一 handler 并打 deprecated。 | `web_service/mod.rs:552`；`handlers.rs:2916`；ADR-086 D4 | 高
- R-N-2 | HTTP generate MUST 先 `route_for(dbnum)`；非 `Database` 时包进 `scope_memory_only`；整库 `dbnum_model_ensure` 在飞时 MUST `409 conflict`。 | `handlers.rs:2946` | 高
- R-N-3 | 范围生成 MUST 由 e3d-io 骨架解根后走 `ensure_model_scope_generated_from_roots`；容器（ZONE/SITE）是合法显示范围；只有子树无生成根才 422 container。 | `handlers.rs:2985-2991`；`on_demand_model.rs:487,996` | 高
- R-N-4 | `force=true` MUST 先 `clear_model_scope(..., ClearGeometry::Purge)` 再逐根生成；根锁被占 MUST 整次 409、一行不删。 | `on_demand_model.rs:108,569`；`mesh_purge.rs:48` | 高
- R-N-5 | 凭证命中 MUST 是「库级承诺 ∨ 根级账本 `Generated`」，账本未 settle 一票否决；回执半边 MUST 只证「同一世代、没落后」：库级盖到 → `SessionRule::NotBehind`（`session ≤ latest`），听不到 → `Exact`；摄入形态 `① ∧ ②`。 | `model_coverage.rs:83`；`on_demand_model.rs:436,599,642`；`room_read_through.rs:25`；ADR-079 决策 6；ADR-089 D1/D5 | 高
- R-N-6 | 按需生成失败记账本 MUST 仅在 `force=true` 且 `!retryable` 时 `record_failed`；单根失败不中止整片（200 + `failed_roots`）；全部失败上抛首错。 | `on_demand_model.rs:531,765,786` | 高
- R-N-7 | HTTP 等待 120s 超时 MUST `504 timeout` 且不取消后台；同根 coalescing 飞行窗 110s MUST `ModelGenerationInProgress` → 409；后验源前移 MUST `ModelStaleAfterGeneration` → 409。 | `on_demand_model.rs:397`；`handlers.rs:2979,3434`；`web_service/mod.rs:208` | 高
- R-N-8 | 读路由 `Database` ⇔ ingest ∧ `applied_sesno > 0` ∧ 无数据批次在飞 ∧ 两水位相等；其余 `Memory`；`allows_durable_fallback` MUST 仅 `MemoryReason::ModelLagging`，且投影无回执时按根回读 durable，回执点名 `durable_fallback_roots`。 | `model_read_route.rs:13,124,128`；`handlers.rs:4416`；ADR-071 D8 追记（#74） | 高
- R-N-9 | 手动增量入口 MUST 是 `POST /api/v1/update/preview` 与 `/update/execute`；预览与执行 MUST 共用 `build_effective_model_update_plan` → DESI `build_model_update_plan_from_window`。 | `web_service/mod.rs:539`；`window_root_plan.rs:2446`；`manual_update.rs:4419`；`increment_pipeline.rs:1061` | 高
- R-N-10 | `initialize_dbnum_baseline` 遇 `ScanGate::Blocked` / `Reinit` MUST 拒绝直接覆盖；半截基线 MUST wipe 后重解析。 | `manual_update.rs:3478` | 高
- R-N-11 | `/dbnums` 行 MUST 分开 `anomaly` / `blocked` / `excluded` / `not_in_project`；`watch` 不是行字段，进程级在 `/health.watch_dbnums`。 | `manual_update.rs:2844`；`handlers.rs:1231` | 高
- R-N-12 | `/tasks/{id}/changes` MUST 来自窗口账本落地后的 `ChangeStore`；任务不存在或无清单 MUST 404；失败数据批次被后继覆盖时 MUST 互指 `superseded_by` / `supersedes`。 | `task_change_list.rs:1`；`handlers.rs:2225`；`task_registry.rs:580` | 高
- R-N-13 | `/health` 增量相关格 MUST 含 `dbnum_consistency`、`model_cache`、`mdb_closure`、`features.room`、`watch_dbnums`、`model_scope_regen`、`batch_failures`。 | `handlers.rs:1000,1106,1157,1212` | 高
- R-N-14 | `GET /api/v1/model/versions`、`/element/versions`、`/element/attribute-history`、`/node/versions`、`/node/diff-summary`、`/model/history/*` 存在于路由表；`tree/*` MUST NOT 加 `sesno`。 | `web_service/mod.rs:565-626`；ADR-081 D1/D6 | 高

### §F store 模式、启动与配置面

- R-F-1 | `store_mode` MUST 只认 `mem` / `rocksdb`；进程只连、不拉起 SurrealDB；裁决三层 `AIOS_STORE_MODE` > `DbOption.toml` `store_mode` > 默认 `rocksdb`；漏设 MUST 落到 `rocksdb`，MUST NOT 静默落到 `mem`。 | `options.rs:1247-1258,1280-1313,1323-1327`；ADR-074 D1/D2；ADR-083 D1/D6 | 高
- R-F-2 | `in_memory_db` / `AIOS_IN_MEMORY_DB` MUST NOT 参与裁决，写了启动点名；退役别名 MUST 仍解析并出声（`embedded-mem`→`mem`、`external`→`rocksdb`、`spawned-mem`/`kv-mem`→进程内 `mem://`）；env 与文件不同档 MUST 并排点名。 | `options.rs:1195-1197,1335-1350,1420-1452`；`lib.rs:1623-1644,2563-2565`；ADR-083 D2；ADR-074 D4 | 高
- R-F-3 | `data_face` MUST 由 `store_mode` 派生（`rocksdb`→`ingest`，`mem`→`read-through`）；声明层不符只告警、不压过介质；`run_app` 按 `data_face().is_read_through()` 分臂。 | `options.rs:1479-1526,3400-3406`；`lib.rs:2579-2609`；ADR-074 D6 | 高
- R-F-4 | `mem` MUST `SUL_DB.connect("mem://")`、不 `signin`；`rocksdb` MUST `aios_core::init_surreal()` 连 `v_ip:v_port`，失败就地退出；唯一入口 `ensure_store_connected`。 | `lib.rs:2028-2098` | 高
- R-F-5 | 单实例锁 `.gen-model.instance.lock` MUST 落在进程 CWD、不跟 `project_path`；`mem` 不拿锁；`rocksdb` 拿锁并再拿机器级写者锁（端点+ns+db）；拿不到启动失败。 | `lib.rs:167-184,213-283,374-406`；`database_writer_lock.rs:25-48` | 高
- R-F-6 | 启动步骤 MUST 打 `[startup] stage=… event=begin|done|skip`；ingest 臂含 `instance-lock` / `option-checks` / `surql-functions` / `surql-embedded` / `surql-selfcheck` / `increment-state` / `watermark-mirror` / `db-manager` / `http` / `spatial-room` / `watcher-init` / `batch-worker` / `model-coverage`；read-through 臂先 HTTP 再路由目录，不整 MDB 生成。 | `lib.rs:623-655,1707-1763,2313-2372` | 高
- R-F-7 | surql 加载 MUST 先磁盘 `resource/surreal`（缺则内置兜底），再灌 `EMBEDDED_SURQL`，同名函数以内置为准；核验缺函数出声不拦启动。 | `embedded_surql.rs:1-20,61-91`；`lib.rs:960-991` | 高
- R-F-8 | `watch_dbnums` 默认空 = 全范围；CLI `--watch-dbnum` 压过文件；入范围判定 MUST 走 `watch_scope`。`auto_initialize_new_dbnums` 代码缺省 true（测试覆盖 > CLI > env > 文件 > 默认）；`startup_autorun` 默认 true。 | `options.rs:1568-1575,1727-1762,1813-1836`；`DbOption.toml:166-183,236-248` | 高
- R-F-9 | `AIOS_DATA_INCREMENTAL` / `AIOS_MODEL_INCREMENTAL` 压过 toml、默认 true、关只冻消费不冻发现；`AIOS_MODEL_INCREMENTAL_TYPES` 逗号名单。 | `options.rs:1839-1871`；`DbOption.toml:189-193,268-279` | 高
- R-F-10 | HTTP 监听 MUST `http_api_addr` > `server_release_ip` > `0.0.0.0:9099`；缺新键仍启动；bind 失败就地失败。 | `web_service/mod.rs:4-5,43-59`；`lib.rs:1237-1250` | 高
- R-F-11 | 配置文件定位 `DB_OPTION_FILE` > `DbOption[.toml]`（相对 CWD）；扩展表类型错 MUST `Err` 出声；未知键照收。 | `options.rs:690-726,809-813,2897-2924` | 高
- R-F-12 | `/health.sul_db` MUST 报 `medium=mem|rocksdb`、`endpoint`（`embedded:mem` / `v_ip:v_port`）、`durable` 仅 `rocksdb` 为真。 | `handlers.rs:891-908,1095-1098`；ADR-074 D5 | 高
- R-F-13 | 仓内 `DbOption*.toml` 夹具与出厂模板 MUST NOT 带未注释 `store_mode =` / `in_memory_db =`。 | `options.rs:3440-3448`；ADR-083 D3 | 高
- R-F-14 | `AIOS_DATA_READ_MODE` 只认 `direct`；`AIOS_SPATIAL_COMPUTE` 只认 env、默认开；退役只点名不改行为：`AIOS_NET_WINDOW`、`AIOS_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES`、`AIOS_SCOPE_CACHE_SECS`、`AIOS_WATERMARK_REALIGN`、`AIOS_ROOM_MEMBERSHIP`。 | `options.rs:1765-1810,1930-1942,2722-2931`；`DbOption.toml:289-301` | 高
- R-F-15 | 发布包 `Start-AMS.ps1`：给了 `-StoreMode` 就设 `AIOS_STORE_MODE`；不给时文件优先；文件也没写 MUST 回填 `rocksdb`（2026-10-26）。 | ADR-083 D5 修订；`scripts/New-LocalRelease.ps1:591`；`start-local-release.ps1:36`；`docs/database-storage-modes.md:22-27` | 中

### §I 测试实体（L2 回放 / 登记表 / 预算门）

- R-I-1 | L2 回放前置缺一即整档 skip：`AIOS_REPLAY_PROJECT_ROOT`（含 `AvevaMarineSample`）、仓根 `DbOption.toml`、仓内 `bin/surreal.exe`。 | `tests/increment_replay/main.rs:11-14,475-492,2635-2640` | 高
- R-I-2 | 派生配置只改六键：`project_path` / `http_api_addr` / `watch_dbnums` / `auto_initialize_new_dbnums` / `v_ip` / `v_port`；存储钉死 `AIOS_STORE_MODE=rocksdb` + `AIOS_DATA_FACE=ingest`。 | `main.rs:4-8,510-568,690-691` | 高
- R-I-3 | 被测服务 CWD MUST 是 `run_dir`（`.current_dir(run_dir)`），禁止仓根（避免 `.gen-model.instance.lock` 被常驻服务持有、避免 `dblog`/`accel_tree` 污染仓根）；`Service::start` MUST 先 `stage_surreal_scripts(repo, run_dir)` 再 `spawn_server`；源码级钉 `the_service_under_test_runs_from_the_run_directory_with_staged_surql_scripts`。 | `main.rs:630-668,671-688,739-741,2691-2732` | 高
- R-I-4 | 超时：初始化三水位 600s、每步默认 180s、`/health` 180s、surreal 60s、整库重建 900s、冷重启再同步 120s；跑法 `cargo test --test increment_replay -- --nocapture --test-threads=1`，`AIOS_REPLAY_CASE=<name>` 只跑一条；`known_red` 默认跳过。 | `main.rs:16-17,88-92,169-173,733-2194` | 高
- R-I-5 | 失败注入 MUST 在文件前移之前挂上（`fail_roots` → 冷重启带 `AIOS_TEST_FAIL_ROOTS`）；`expect_batch_parked` 路径 `continue` 后不走 `assert_step`。 | `main.rs:182-201,1516-1616` | 高
- R-I-6 | `budget_gate` 只开在基线链；断言后杀进程再 `python scripts/increment_budget.py collect --check`；WARN 不挡。 | `main.rs:50-53,1658-1701`；`scripts/increment_budget.py:1-28,193-258` | 高
- R-I-7 | `rebuild_*` MUST 链尾打；刚体对拍容差 `0.5`；`AABB_TOLERANCE_MM = 1.0`；`log_must_not_contain` 扫运行目录全部 `stdout*.log`；`records_present`/`records_absent` 双通道（HTTP `/model/records` 且库上 `inst_relate`）。 | `main.rs:63-75,132-133,392,431-433,1650-1652,2535-2621` | 高
- R-I-8 | 登记表 `tests/fixtures/increment_cases.json` 是单一事实（`status ∈ has|partial|todo|corpus`；`replay:<name>` 必须有对应 json）。现状 56 条：has 41 / partial 6 / todo 7 / corpus 2。 | `tests/increment_cases_registry.rs:3-9,114-139`；`increment_cases.json:2-70` | 高
- R-I-9 | 8 个 L2 用例：`ams8000-equi-rotate-chain`（录完未跑）、`ams8000-equi-room-cross`、`db8000-two-delete`、`ams8000-equi-move-chain`、`ams8000-catalogue-out-of-scope`、`ams8000-catalogue-stamp-only`、`ams8000-equi-data-fail-park`、`ams8000-equi-fail-inject`。Core3D `core3d-partial-update-test-cases.md` 的 C 编号与 L2 几乎正交（27 个 C 编号：0 有 / 2 部分 / 25 无）。 | `tests/increment_replay/cases/*.json`；`docs/specs/core3d-partial-update-test-cases.md:76-351` | 高

---

## Implementation Decisions

本规格不新增行为；以下是审计确认的**现行架构决策**，按 ADR 索引，并标出正文过时之处（细节见 `review.md` §5）。

1. **行与写法（ADR-060 行id）**：行 id 即记录 id；模型面零 UPDATE/UPSERT，先删后插；`inst_geo`/共享 `.mesh` 只增不删。正文 D6（`->tubi_relate` 图遍历）与 D8/D11（`gen_root` 凭证行）已过时。
2. **水位是承诺（ADR-060 水位）**：`model_sesno` 与 `applied_sesno` 同行；三态判决唯一口径。D3/D5/D6/D7/D10 中 `gen_root`、`ModelWatermarkProbe`、`startup_coverage_floors`、`legacy_adopted` 判定均已退役（ADR-079）。两份文件撞号 ADR-060，必须以文件名区分。
3. **库是任务结果的同步序列化（ADR-071）**：dbnum 任务是唯一写者；`applied_sesno` 在数据尾事务、`model_sesno` 在任务收尾；D8 由 #74 追记改为 lagging 按根回读。D2「`gen_root` ON DUP KEY」已失效。
4. **崩溃恢复（ADR-072 → ADR-073 D3 → ADR-076 D8）**：任何 `writing` 一律整库 Reinitialize；`Partial` 收标记不重建；模型落后靠启动 `ModelOnly`。ADR-072 D2/D3 的分面回退正文未加作废横幅。
5. **房间随根（ADR-073）**：无第三枚水位；`model_room_generation` 是就绪格；常驻模型 D12/D13 与 ADR-082 D3 修订。`room_membership` 开关 2026-10-23 退役，ADR 2026-09-11 修订段仍当开关。
6. **ADR-076/077**：收尾等 durable 栅栏；初始化双 lane、不进投影。ADR-076 D5「三枚水位同笔」与代码两写点不一致。
7. **MDB 闭包（ADR-078/085）**：项目集合由 CURD 派生；范围读登记表、SYST 落地刷新；`included_projects` 退役（符号名仍残留在 `initialization_phase.rs`）。
8. **根结局在进程内（ADR-079）**：`gen_root` 退役；账本三态；水位是唯一持久模型事实。决策 6 的凭证判法被 ADR-089 改为 `NotBehind`，079 未回写。
9. **单元级发布（ADR-080）**：`CommitScope::{WholeRoot, Partial}`；`stamp_partial_receipt`；闸 `receipt ≤ S`（ADR-089 D7）；默认开（2026-10-14）。状态行仍「草案」，D9「默认关」与 10-01「不翻」过时。
10. **按需版本（ADR-081）**：不落库；追记接口 `element/node/*`、`node/diff-summary`。
11. **缓存闸（ADR-082/084）**：overlay 按根数、durable-only 逐出；元素缓存满则清；GenerationInput 按库数 LRU（默认 0 未定）。ADR-082 实施钉「配置写 0」与现文件 50000/200000 不一致。
12. **store 模式（ADR-074/083）**：三层裁决、默认 `rocksdb`、脚本回填 `rocksdb`；ADR-083 标题与 D3/D5/D6 正文仍写 env-only / 回填 `mem`。
13. **records 按祖先（ADR-086）**、**切片删（ADR-088）**、**按需新鲜度与冻结锚（ADR-089）**：与代码一致。
14. **ADR-087**：无文件，号位被 changelog 拟稿（`.mesh` 原子重写，`MeshWrite::Repaired`）占用；无 ADR-09x。
15. **W2/W3 `pe_ptset`**：本轮新增的 P 点集表随发布事务落地，支撑 `fn::bran_comp_len` 等长度函数；无 backfill（用户决策：旧库重新生成）。尚无 ADR。
16. **没有 ADR 支撑的已落地决策**（需补）：`FileAnomaly::NameChanged` 与 B07 撤回；#76 元数据候选不做种子；刚体分桶 P4 前置；`DATA_BATCH_PARK_AFTER=1`；#73 生成器指纹整库重排；#71 `rooms_after` 拍点；mesh-purge 不删管身键/不反查；W3 变化清单落 jsonl；K 组 O6+RVM 真值。

## Testing Decisions

- **好的测试只断言外部行为**：库上行集合（`inst_relate`/`tubi_relate`/两张房间边/`pe_ptset`）、`/model/records`、`/health` 判决、水位值、任务终态与回执字段；不断言内部函数调用顺序（源码钉 `include_str!` 一类只用于契约护栏，如 R-I-3）。
- **接缝从高到低**：① L2 `tests/increment_replay`（真 SurrealDB + 真服务 + 录制的会话链；首选）；② L3 `src/bin/l3_suite.rs` 场景清单；③ 库内 `#[cfg(test)]`（`window_root_plan` / `model_db_adapter` / `batch_queue` / `task_end` / `dbnum_consistency` 等已有丰富钉）；④ `tests/increment_cases_registry.rs` 登记表护栏。新增验收优先挂到 ① 的 json 用例与 ④ 的登记表，不新开夹具层。
- **要测的模块**：双水位收尾（`task_end`）、崩溃恢复（`startup_reconcile`/`write_state`）、调度（`batch_queue`/`batch_scheduler`/park）、窗口四去向（`window_root_plan`）、切片删（`model_db_adapter`）、单元级发布（`e3d_model_service`/`model_memory_store`）、房间边（`room_model`/`room_recompute`）、缓存闸、按需凭证（`on_demand_model`/`model_coverage`）、HTTP 契约（`web_service/mod.rs` 守卫）。
- **既有先例**：`replay_cases_land_in_sync_and_match_their_expectations`；`the_root_limit_evicts_durably_acked_roots_oldest_commit_first_and_never_unacked_ones`；`slice_delete_is_one_predicate_block_guarded_by_the_model_column`；`the_rooms_after_snapshot_is_taken_after_the_durable_fence`；`every_unfinished_write_restarts_as_a_whole_dbnum_reinitialize`；`d10_refuses_to_classify_unless_all_three_cascades_are_on`；`the_republish_gate_proceeds_from_or_before_the_planned_baseline`；`model_records_answer_by_ancestor_and_never_say_not_generated`；`the_store_mode_layers_are_env_then_file_then_default`。
- **预算门**：`scripts/increment_budget.py` + `tests/fixtures/increment_budget_baseline.json`，只在基线链开，WARN 不挡。

## Out of Scope

- 几何对拍/RVM 一致性债（`docs/plans/2026-10-21-model-remaining-issues-fix-plan.md` 的 W1–W7、e3d-model vendor 缺陷）。
- plant3d-web 前端联动用例（K 组）的实现，仅登记表接入在范围内。
- Core3D `PartialUpdateDesiMgr` 地址表与指令流复核（只核我方对应物）。
- vendor `e3d-io` / `e3d-model` 源码（只核 gen-model 侧调用与 ADR 陈述）。
- 性能优化专项。
- 向 issue tracker 发布本规格（用户要求只写文件、不执行命令）。

## Further Notes

- 审计只读、未执行 `cargo test` / 真库；所有「存在」判断来自 grep/read；所有「零命中」限定在 `gen-model/src`（`.scratch/` 备份不计）。
- 本工作树含未提交改动（`pe_ptset`、L2 `run_dir` 等），规则已按工作树现状写入；提交后行号可能漂移。
- 规则 ID 保留审计线前缀（A/B/C/D/E/F/G/H/I/N/O/P），`tasks.md` 的任务 ID 与之同源，便于追溯。
- 与 `incr-gen-fixes` 的逐条对照、与 core3d 核对表 R1–R25 的符号核实、ADR 过时语句清单见 `review.md`。
