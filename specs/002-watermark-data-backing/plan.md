# Implementation Plan: 水位必须有数据支撑，回退默认整库重建

**Branch**: `002-watermark-data-backing` | **Date**: 2026-08-13（事后补录）| **Spec**: `specs/002-watermark-data-backing/spec.md`
**Input**: ADR-021（修订 ADR-001，取代其 2026-08-12 watermark_realign 修订）

> **补录说明**：本特性于 2026-08-13 当日完成实现与验证；plan 属事后补录——同日的增量
> 流程一致性审计发现「有 spec 无 plan、Constitution Check 无处留痕」的流程缺口，按已
> 发生的事实补齐留痕，不虚构决策过程。

## Summary

「基线还是增量」的路由判定补上数据支撑维度：`applied_sesno > 0` 而 `pe` 零行（幽灵水位）
路由到首次导入重建基线；文件回退（`file_latest_sesno < applied_sesno`）从「阻断等人」改为
默认整库重建——扫描只分类入队重建批次，worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`
清库并按首次导入重解析。`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN` 环境变量、
`POST /api/v1/dbnums/{dbnum}/realign` 端点与 `aios_db.sync.realign` 绑定全部移除。

## Technical Context

- 判定承载：`needs_initial_load`（扩入参增加数据支撑维度，不改位置）+
  `dbnum_has_any_pe_row` 存在性探针（只在 `applied_sesno > 0` 时查询、每批次一次，
  失败上浮为批次 Failed）——`src/data_interface/manual_update.rs`。
- 扫描三态：`scan_and_check_file` 返回 `ScanGate`（放行 / 阻断 / 重建），sweep、watch
  与手动路径对回退构造显式 `Reinitialize` 重建批次；零会话用 `0..=0` 控制窗口，
  排队行被提升、运行中行追加后继；扫描路径不删任何数据
  ——`src/data_interface/increment_manager.rs`。
- 清库：`fast_delete::wipe_dbnum_for_reinit`（与整库快删同源的三阶段删除；元数据阶段 =
  统计与队列残留清空 + spatial epoch 递增 + 水位行清值不删行，整组显式事务且水位置尾）。
  关系阶段的 `pe_owner` 走 OWNER 复合 id 区间（每个权威 Ref0 一条闭区间），不走图遍历；
  该形状跨 owner，只在完整清理成立，`prune_above_watermark` 与 `staging` 重放路径均不适用。
  后置条件同时验 `pe` 归零与逐 Ref0 的 `pe_owner` 区间残留归零。
- 执行体：`execute_one_dbnum` 冻结点复核仍判回退才清库；`batch_worker` 开窗前过
  `batch_reroutes_to_initial_load` 预判（applied=0 / 回退 / 幽灵水位一律不开 ADR-017
  暂存窗口，直接走执行体），与执行体共用同一个数据支撑探针。
- 预览与执行同谓词（`blocked` / `initialization_required`），回退行保留 `anomaly` 证据。
- 基线入口显式匹配共享 `ScanGate`；`Blocked` / `Reinit` 在计数、解析、水位之前退出。
- 范围外 CATA 收集全部候选，复用 `duplicate_dbnums`；重复组零 observation 并把路径
  写进预览/入队 warnings，唯一候选才登记。
- 初始化两阶段：`batch_worker::initialization_defers_model_phase` 识别首次导入、回退重建
  与冻结点才改道的幽灵水位批次；第一阶段只收口数据、水位与 durable pending，数据队列
  清空后再由既有 `idle_round` → `drain_data_phases` 分页执行模型，不新增消费路径。

## Constitution Check

对照宪法（实现当日为 v1.0.0；本批文档面修订为 v1.1.0）：

- **I 水位是承诺**：本特性给承诺补上读侧对偶（数据支撑）；「回退默认整库重建」与
  v1.0.0 I 条「文件回退时阻断该 dbnum」字面冲突。**处置 = 修宪**（2026-08-13 审计定案：
  v1.1.0 已按 ADR-021 改写 I 条回退语义，Governance 留修订记录）。实现对「写失败不推进
  水位、幂等重放」的纪律无任何放松。
- **II 一条规则只有一份实现**：符合——`ScanGate` 由手动 / 自动路径共用；
  `FileAnomaly::requires_reinit` 是「哪些异常转重建」的唯一裁决；预览与执行体共用路由
  谓词（FR-002）；数据支撑判据不进入队门（FR-003，守护测试钉住）。
  基线入口同样消费该 gate，范围外 CATA Duplicate 复用 watcher 权威集合（FR-016/017）。
- **III 静默失效是最高级别缺陷**：符合——幽灵水位与回退检出必须在日志与批次回执发声
  （FR-006 / FR-013，含删除规模）；存在性查询失败上浮，不吞任何默认值（FR-005）。
- **IV 队列任务可消费、可收口、可复活**：符合——重建批次走同一条队列同一个派发器；
  排队时意图占优、运行中留后继；清库失败批次 Failed、元数据事务回滚、下一轮幂等重放
  （FR-010）。初始化模型工作持久化后交给既有空闲轮消费，进程中断仍可复活。
- **V 标识只用真值**：符合——拒绝以 `dbnum_info_table` 行数代证数据支撑（观察值不是
  权威值，「让嫌疑人给自己作证」）；存在性只问 `pe`。
- **VI 不变量由可执行的守护看住**：符合——每条修复附「回退旧实现即红」的回归测试
  （FR-015）；「清库只在 worker、扫描只入队」「判据不进入队门」由源码顺序 / 守护测试钉住。

**Complexity Tracking**：无超出宪法的复杂度引入；净减一个配置档位、一个环境变量、
一个端点、一个 Python 绑定。

## Verification（已完成，证据）

- CI 口径受影响模块单测 155 绿（含 `needs_initial_load` 真值表、`ScanGate` 逐类映射与
  四条源码钉）；全量 lib 683 passed / 0 failed。
- live：`live_rollback_wipe_clears_the_dbnum_for_reinit`（4.7s @8019）、
  `live_rollback_and_ghost_watermark_reinit_end_to_end`（22.3s @8019，两幕）——live 台账
  两行 + `docs/evidence/2026-08-13-adr021-rollback-reinit-live.md` 双留痕。
- Python 闭环：`python/tests/test_rollback_reinit.py` 3 passed（@8071 一次性内存库）；
  全套 `pytest -q` 80 绿（含离线档 62）。
- live 首跑抓出并修复一个真缺陷：增量形状批次先开暂存窗口、执行体改道基线后窗口缺
  finalize plan 而 failed → `batch_reroutes_to_initial_load` 冻结点预判（见 Technical
  Context），ADR-021 已补衔接条款。

## Progress Tracking

- [x] spec（`spec.md`，2026-08-13）
- [x] 实现 + 拆除面（同日，见 `tasks.md`）
- [x] 单测 / live / Python 验证与证据留痕
- [x] 文档同步（web-service-api、DbOption 配置台账、changelog、live 台账）
- [x] 初始化两阶段现场验证：6 个数据基线连续成功后队列清空，模型页随后启动
- [x] Constitution Check 收口（宪法 v1.1.0 修订，2026-08-13 文档面批次）
- [ ] 后续项（单独一轮）：水位行记录「来源」（基线收口 / 增量收口 / 播种回填）；
      `applied_sesno_time` 交叉核验（停机窗口内回退又长回去）
