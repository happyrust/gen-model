# Tasks: 水位必须有数据支撑，回退默认整库重建（事后补记）

**Input**: `specs/002-watermark-data-backing/plan.md`
**Prerequisites**: ADR-021（已接受）；宪法 v1.1.0（I 条回退语义）

> 2026-08-13 当日实现已完成，本清单按已发生事实补记（流程留痕用），完成项全部勾选；
> 工作当日串行落地，无并行标注需求。文件路径为实际改动位置。

- [x] T001 路由判定：`needs_initial_load` 增加数据支撑维度（`has_any_data` 入参）+
  `dbnum_has_any_pe_row` 存在性探针（仅 `applied_sesno > 0` 时查询、失败上浮）
  ——`src/data_interface/manual_update.rs`
- [x] T002 扫描三态：`scan_and_check_file` 返回 `ScanGate`（放行 / 阻断 / 重建），回退
  不再阻断；`FileAnomaly::auto_realignable` 更名 `requires_reinit`（覆盖集不变，仅
  Rollback）——`src/data_interface/increment_manager.rs`、`src/data_interface/dbnum_state.rs`
- [x] T003 重建批次入队：sweep 与 watch 对回退构造 `applied_sesno: 0` 形状的重建批次，
  扫描路径零删除——`src/data_interface/increment_manager.rs`
- [x] T004 整库清空例程 `wipe_dbnum_for_reinit`（三阶段删除；元数据阶段 = 统计与队列
  残留清空 + spatial epoch 递增 + 水位行清值不删行，置尾作提交点）
  ——`src/data_interface/fast_delete.rs`
- [x] T005 执行体冻结点复核：仍判回退才清库，清库失败计批次 Failed、水位未动、幂等重放
  ——`src/data_interface/manual_update.rs`（`execute_one_dbnum`）
- [x] T006 开窗预判 `batch_reroutes_to_initial_load`：applied=0 / 回退 / 幽灵水位一律
  不开 ADR-017 暂存窗口，直接走执行体；与执行体共用数据支撑探针
  ——`src/data_interface/batch_worker.rs`
- [x] T007 预览同谓词：`blocked` / `initialization_required` 与执行体一致，回退行保留
  `anomaly` 证据——`src/data_interface/manual_update.rs`、`src/web_service/handlers.rs`
- [x] T008 拆除面：`WatermarkRealign` 档位与 `AIOS_WATERMARK_REALIGN`（`src/options.rs`）、
  `realign_rolled_back_dbnum` / `realign_dbnum_checked`、HTTP `POST /dbnums/{dbnum}/realign`
  （`src/web_service/handlers.rs`）、`aios_db.sync.realign` 绑定（`python/src/exec_api.rs`）、
  `AiosClient.realign_dbnum`（`python/aios_client.py`）、`python/tests/test_watermark_realign.py`
  （由 `test_rollback_reinit.py` 接棒）
- [x] T009 测试：`needs_initial_load` 真值表、`ScanGate` 逐类映射、四条源码顺序钉、
  live 两条（`live_rollback_wipe_clears_the_dbnum_for_reinit`、
  `live_rollback_and_ghost_watermark_reinit_end_to_end`，`src/data_interface/manual_update.rs`）、
  Python 三条（`python/tests/test_rollback_reinit.py`，回退整库重建 / 幽灵水位路由基线 /
  类型变更照旧阻断）
- [x] T010 文档：ADR-021、`specs/002-watermark-data-backing/spec.md`、
  `docs/specs/web-service-api.md`（§4.3 / §4.7 / §4.9）、
  `docs/2026-08-04_dboption-config-changelog.md`（watermark_realign 移除条目）、
  `changelog.md`、`docs/evidence/2026-08-13-adr021-rollback-reinit-live.md`、live 台账两行
- [x] T011 流程收口（2026-08-13 文档面批次，流程审计定案）：宪法 v1.1.0、AGENTS.md
  水位段对齐、ADR-021 状态改「已接受」、本 plan / tasks 补录、CONTEXT.md 词条
  （数据支撑 / 幽灵水位 / 重建批次）
- [ ] T012 后续项（单独一轮，ADR-021 已记录）：水位行「来源」字段（基线收口 / 增量收口 /
  播种回填，动 schema 与启动播种路径）；`applied_sesno_time` 交叉核验（停机窗口内回退
  又长回去的检出）
- [x] T013 审查修复：引入 `BatchIntent::{ApplyWindow, Reinitialize}`，覆盖零会话、排队
  提升、运行中后继与冻结点复核；空文件清库后 Applied 收口
  ——`src/data_interface/batch_queue.rs`、`batch_scheduler.rs`、`increment_manager.rs`、
  `manual_update.rs`
- [x] T014 审查修复：`wipe_dbnum_for_reinit` 元数据阶段显式事务，水位置尾；附渲染顺序
  回归与故障注入入口——`src/data_interface/fast_delete.rs`
- [x] T015 审查修复：基线入口消费 `ScanGate` 三态，阻断/重建在计数、解析、水位前退出
  ——`src/data_interface/manual_update.rs`
- [x] T016 审查修复：范围外 CATA 跨 scope 收集全部候选并复用 `duplicate_dbnums`，
  重复组零 observation、撤销旧 locator 身份且 warnings 列全路径
  ——`src/data_interface/manual_update.rs`、`increment_manager.rs`、`dbnum_state.rs`、
  `cata_closure.rs`
- [x] T017 初始化两阶段：首次导入、回退重建与冻结点改道的幽灵水位批次只收口数据、
  水位和 durable pending；模型工作在数据队列清空后由既有空闲轮分页消费。补纯函数
  回归测试与 test-workspace 六库现场顺序验证
  ——`src/data_interface/batch_worker.rs`、`docs/adr/ADR-011-one-data-batch-queue-for-manual-and-auto.md`
- [x] T018 清库性能修复（FR-009a / FR-009b）：关系阶段的 `pe_owner` 改按 OWNER 复合 id
  区间删除（每个权威 Ref0 一条闭区间，取代两句 `array::flatten(SELECT VALUE
  ->/<-pe_owner FROM pe:{ref0}_0..)` 图遍历），跨 owner 区间的适用边界就地写进源码且
  不放宽 `replay_safe` 的既有拒绝；`prune_above_watermark` 保持逐元素双向删除不动；
  后置条件加验逐 Ref0 的 `pe_owner` 区间残留归零。三条纯函数回归
  （`the_owner_edges_go_by_id_range_not_by_graph_traversal`、
  `the_owner_range_is_ref0_scoped_and_never_open_ended`、
  `the_postcondition_counts_owner_edges_in_every_ref0_range`）+ 隔离库实测
  ——`src/data_interface/fast_delete.rs`、`docs/adr/ADR-021-watermark-must-be-data-backed.md`、
  `docs/evidence/pe-owner-range-fast-delete-20260820/`
- [x] T019 初始化模型有界消费回归（FR-010a）：自动 Regen 页固定 100 根，
  post-regen AABB 页固定 256 条；每页 Regen 后重新探测并在待办未清时禁止
  AABB 越过。忙根锁延后不增加 attempts，任务详情限制为 10 个根样本，
  单次任务列表最多 160 条且响应小于 256 KiB，启动等待日志按真实阶段与页进度播报
  ——`src/data_interface/model_update_pending.rs`、`src/data_interface/batch_worker.rs`、
  `src/web_service/handlers.rs`、`src/lib.rs`
