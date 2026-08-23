# Tasks 016：BRAN 直管关系权威替换

- [x] T001 [US1] `specs/016-authoritative-bran-tubing/spec.md`：定义完整关系集合、空集合与失败保旧语义。
- [x] T002 [US1] `specs/016-authoritative-bran-tubing/plan.md`：完成 Constitution Check 并引用 ADR-010/014/017/024。
- [x] T003 [US1] `src/fast_model/cata_model.rs`：先增加“多段变少段、旧索引消失”的失败回归。
- [x] T004 [US2] `src/fast_model/cata_model.rs`：实现单 BRAN 原子替换渲染，持久化 `trans` / `aabb` 内容并通过 `execute_model_write` 路由。
- [x] T005 [US3] `src/data_interface/helper.rs`：删除清理同步移除当前 refno 的 `tubi_relate` 出边并补回归测试。
- [x] T006 [US1] `src/fast_model/cata_model.rs`：把三个直管产生点接入每 BRAN 完整产物，空集合也替换。
- [x] T007 [US1] `src/fast_model/cata_model.rs`：增加 ReplaySafe、幂等重放、内容解引用与事务失败保旧测试。
- [x] T008 `changelog.md`：记录直管关系陈旧与悬空内容引用修复。
- [x] T009 `docs/evidence/2026-08-20-bran-tubing-authoritative-replacement.md`、`docs/2026-08-12_live-test-ledger.md`：执行并登记指定 BRAN live 复测（7 行 → 4 行，高位索引消失，不再引用已删元件，内容全部可解引用）。
- [x] T010 运行 `cargo fmt`、定向 `cargo test`、`cargo check`、`sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`（限定本规范 10 文件复核；仅报告两处源码采用内联单测、未发现独立测试文件）。

## Dependencies

- T003 → T004 → T006 → T007。
- T005 可与 T003/T004 并行，但当前会话串行执行以避免同工作树冲突。
- T008 可在代码终态后执行；T009/T010 依赖 T004–T007。
