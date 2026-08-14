# Tasks

- [x] T1（串行）`docs/adr/ADR-025-strict-initialization-phases.md`、`specs/006-strict-initialization-order/*`：完成 ADR → spec → plan → tasks。
- [ ] T2（串行）`src/data_interface/initialization_phase.rs`、`src/options.rs`：以纯测试实现阶段状态、epoch、manifest 与项目优先级裁决。
- [ ] T3（串行，依赖 T2）`src/data_interface/batch_queue.rs`、`src/data_interface/batch_scheduler.rs`：批次携带阶段并实施阶段派发屏障。
- [ ] T4（串行，依赖 T3）`src/data_interface/increment_manager.rs`、`src/data_interface/manual_update.rs`：共享完整候选扫描、CATA 入队与阶段重扫。
- [ ] T5（串行，依赖 T4）`src/data_interface/batch_worker.rs`、`src/data_interface/model_update_pending.rs`：数据收口驱动阶段、模型 drain 门控。
- [ ] T6（可与 T5 并行）`src/versioned_db/database.rs`、`src/lib.rs`、HTTP/Python 接口：全量三阶段与启动/按需模型门。
- [ ] T7（串行）单测、四个集成目标、Python offline、六条 live 用例与证据台账。
- [ ] T8（串行）`changelog.md`、接口文档、`cargo fmt/check/test`、SigMap 审查与三批提交。
