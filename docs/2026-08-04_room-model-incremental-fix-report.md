# 房间/模型增量更新遗留缺陷修复（2026-08-04）

- 范围：`gen-model` 仓内房间增量与模型增量链路在 2026-07-30 使用面审计
  （`2026-07-30_increment-update-usage-audit.md`）之后仍未收口的缺陷。
- 前置状态核对：spec 001（`specs/001-incr-update-integrity-fixes`）五个故事
  （副本冻库 / 类型替换阻断 / 级联丢引用者 / 派生根复活 / CATA 标注）已全部落地并提交，
  本轮不重复。
- 验证：`cargo check --lib --features http_api` 干净；
  `cargo test --lib --features http_api` **303 passed / 0 failed / 58 ignored**
  （对照 07-31 基线 285 passed，新增 6 条回归测试均绿）。

## 修复清单

| # | 审计编号 | 域 | 改动 |
| --- | --- | --- | --- |
| 1 | B2（高） | 房间增量 | `TaskRegistry::set_detail` 新增；`room_round` 在 `drain_rooms` 之后重新 `count_room_targets()` 并覆盖任务 `detail`。收敛到 0 的那一轮从此会把 `{panels:0, elements:0, dead_letters:N}` 写回，泳道不再永久显示开跑前的待重算数、不再 30 分钟假饥饿。统计失败时保留旧 detail 并打日志。 |
| 2 | A1（高） | 服务面 | `web_service::serve` 资源目录缺失从 `anyhow::bail!` 改为告警降级：`/assets` 返回 404，REST/WS 照常启动；`AppState` 新增 `static_assets`，`/health` 回带该字段（补齐 spec §4.1 缺口的一半，A4 的 `ref0_affiliation_conflicts` 仍未实现）。 |
| 3 | C2（中） | 模型增量 | `drain_where` 批量收口（`clear_regen_work_batch`）失败不再对**生成成功**的根逐个 `record_failure`（旧行为：attempts+1，5 次进死信）。行留在表里不动，下一轮 drain 重跑幂等生成再收口；与 `batch_worker` 的 `settlement_failed` 口径对齐。收口失败仍进 drain 汇总如实报错。 |
| 4 | A2（高） | 模型增量 | 实现 spec §4.6.1 `POST /api/v1/update/pending-units/retry`：只允许操作已存在的 `(action, target_refno)`，一条原子 UPDATE 完成 `revision+1 / attempts=0 / 清 last_error / status='pending'`，404 不凭空建行，202 回执带复活后的行。新增 `ModelWorkAction::parse`、`BatchScheduler::wake()`（复活后立即唤醒 worker，不等 30s 兜底轮询）。认领了会话号又等不来新会话的死信从此有了 HTTP 出口。 |
| 5 | C1（中） | 模型增量 | `ensure_model_generated` 生成前先经新函数 `ensure_regen_pending`（复用 `render_upsert`）落 durable pending 行并取回收口令牌，替换掉只读现有行的 `current_regen_revision` 路径。进程崩溃后该根留在表里由空闲轮接手，满足 spec §4.5「先写入 durable pending 再同步等待」。 |
| 6 | 顺带发现（tasks.md） | 模型增量 | `render_upsert` 非房间分支：本次入队不认领来源库（`dbnum == 0`，派生根/按需生成）时改为 `dbnum = dbnum?:0`，不再把行上已存的真实库号抹成 0（旧行为使该根从本库批次工作单掉进空闲轮，延迟消化）。 |

## 新增回归测试（回退旧实现即红）

- `task_registry`：`a_room_round_detail_can_be_overwritten_after_convergence`
- `batch_worker`：`the_room_round_overwrites_its_detail_after_draining`（源码断言：重统计在 drain 之后、经 `set_detail` 写回）
- `web_service::mod`：`a_missing_asset_dir_degrades_instead_of_killing_the_service`（源码断言：`serve` 无 `bail!(`、有 `static_assets`）
- `model_update_pending`：`batch_settlement_failure_never_marks_generated_roots_failed`（源码断言：收口失败 arm 无 `record_failure(`/`mark_failed(`、有 `failures.push`）
- `model_update_pending`：`a_manual_retry_revives_in_one_atomic_statement`（渲染断言：UPDATE 非 UPSERT、三件事同语句、按 `(action, target)` 寻址、RETURN AFTER）
- `model_update_pending`：`an_enqueue_that_claims_no_dbnum_keeps_the_stored_one`
- `model_update_plan`：`action_names_roundtrip_through_parse`
- `on_demand_model`：`a_durable_pending_row_is_written_before_generation_runs`（源码断言：落行在生成之前，且不再走 `current_regen_revision`）

## 本轮不动（范围外）

- **B1 / B3 / B4**：客户端接 ensure、宿主补 MDB 与 code 透传——在 `plant-ui` / `rs-plant3-d` 仓。
- **A3**：ensure 超时/忙碌语义三处不一致（spec 202 方案 vs 实现 504/409）需要跨仓契约决策；
  在 B1 接上之前只影响 sweep 脚本。定方案后三处一起改。
- **A4 另一半 / A5-A9 / D1-D3**：spec 全文修订与词表处置，纯文档工作，建议一次做完
  （摘掉 spec 顶部挂了多日的修订注记）。
- 静态审核未实机复现的两条（B2 泳道表现、A3 超时路径）建议起服务各验一次，十分钟内。
