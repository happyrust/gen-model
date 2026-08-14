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
