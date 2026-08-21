# 实现清单

- `src/fast_model/prim_model.rs`: `targeted_invalid_brep_message` 为 `CYLI/SLCY/NCYL` 的定向 BREP 硬失败加入 `DIAM`、`HEIG`、参考号与 noun。
- `src/data_interface/model_update_pending.rs`: `model_pending_status` 单次往返产生 retryable/dead-letter、阶段、action 与最多十条阻断样本快照。
- `src/data_interface/batch_worker.rs`: `announce_model_dead_letters` 按首次、指纹变化、300 秒和清零恢复公告，保持 Model→Room 门控。
- `src/web_service/handlers.rs`: `/api/v1/health` 新增 `model_update_pending` 与 `blocking_conditions`，模型或房间死信令状态为 `degraded`。
- `scripts/e3d/remove_zero_ncyl_24381_38635.mac`: 保存前严格断言并删除唯一的零尺寸空 NCYL。
- `scripts/Repair-ZeroNcylDeadLetter.ps1`: dry-run 优先的备份、隔离导入验证、E3D 修复、7997 重建、单次 retry、收敛观察编排。
- `scripts/Rollback-ZeroNcylDeadLetter.ps1`: 校验成对基线后恢复 E3D 文件、Surreal 导出和部署二进制。
- `specs/022-model-dead-letter-recovery/{spec,plan,tasks}.md`: ADR-011/018/021/025 约束下的规格、计划和任务。
- `docs/specs/web-service-api.md`、`changelog.md`、`docs/2026-08-12_live-test-ledger.md`: API、变更和现场执行状态同步。

构建产物：`D:/Rust/target/release/aios-database.exe`（由当前 `CARGO_TARGET_DIR` 决定）。

现场执行状态：交互式 E3D 进程 PID 57080 仍持有 `ams7997_0001` 的独占锁，源文件删除、成对备份、7997 重建及队列收敛尚未执行；守卫已把流程停在 Backup 前置条件，原部署进程已恢复。
