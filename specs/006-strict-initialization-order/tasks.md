# Tasks

- [x] T1（串行）`docs/adr/ADR-025-strict-initialization-phases.md`、`specs/006-strict-initialization-order/*`：完成 ADR → spec → plan → tasks。
- [x] T2（串行）`src/data_interface/initialization_phase.rs`、`src/options.rs`：以纯测试实现阶段状态、epoch、manifest 与项目优先级裁决。
- [x] T3（串行，依赖 T2）`src/data_interface/batch_queue.rs`、`src/data_interface/batch_scheduler.rs`：批次携带阶段并实施阶段派发屏障。
- [x] T4（串行，依赖 T3）`src/data_interface/increment_manager.rs`、`src/data_interface/manual_update.rs`：共享完整候选扫描、CATA 入队与阶段重扫。
- [x] T5（串行，依赖 T4）`src/data_interface/batch_worker.rs`、`src/data_interface/model_update_pending.rs`：数据收口驱动阶段、模型 drain 门控。
- [x] T6（可与 T5 并行）`src/versioned_db/database.rs`、`src/lib.rs`、HTTP/Python 接口：全量三阶段与启动/按需模型门。
- [ ] T7（串行）单测、四个集成目标和 Python offline 已通过；六条破坏性 live 用例与台账仍待沙箱验收。
- [x] T8（串行）`changelog.md`、接口文档、`cargo fmt/check/test`、SigMap 审查与三批提交。
- [ ] T9（串行）`src/data_interface/model_update_pending.rs`、`batch_worker.rs`、
  `task_registry.rs`：逐根生成、数据让位、独立模型任务和无 attempts 消耗回归测试。
- [ ] T10（依赖 T9）REST/Python/Plant UI：公开 `model_drain` / `yielded` 并按模型终态刷新。
