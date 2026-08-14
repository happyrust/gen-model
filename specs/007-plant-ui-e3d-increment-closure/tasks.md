# Tasks

- [x] T1（串行）修订 ADR-025/spec 006，新增 ADR-027/spec 007/plan/tasks。
- [x] T2（串行）`model_update_pending.rs`、`batch_worker.rs`、`task_registry.rs`：实现模型让位与任务观测。
- [x] T3（可与 T2 分离）`e3d_query.rs`、`src/bin/l3_suite*`：实现进程证据、文件会话分类和重试保护。
- [ ] T4（依赖 T2/T3）Plant UI 设置、树项语义与刷新屏障；l3 夹具同步配置和断言。
- [ ] T5（串行）单测、集成、live、证据、台账、changelog、SigMap 与提交。
