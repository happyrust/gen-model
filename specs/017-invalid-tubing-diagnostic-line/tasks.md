# Tasks 017：不可成管直段的诊断线型

- [x] T001 [US1] `specs/017-invalid-tubing-diagnostic-line/spec.md`：定义不可成管直段的产出、标记与排除语义。
- [x] T002 [US1] `specs/017-invalid-tubing-diagnostic-line/plan.md`：完成 Constitution Check 并引用 ADR-010/014/017 与 Spec 016。
- [x] T003 [US1] `src/fast_model/cata_model.rs`：增加「方向判定失败仍产出带标记直段」「口径未知仍产出」「轴线判定不得再挡住写入」三条回归。
- [x] T004 [US1] `src/fast_model/cata_model.rs`：`TubiRelationSpec` 增加 `invalid: Option<TubiInvalidReason>`，渲染器写出 `invalid` / `invalid_reason`。
- [x] T005 [US1] `src/fast_model/cata_model.rs`：抽出 `tubi_spec_from` / `diagnostic_centre_line`，三个产生点共用；口径解析提前到轴线判定之前。
- [x] T006 [US2] `../plant-ui/vendor/rs-core/src/rs_surreal/inst.rs`：`query_tubi_insts_by_brans` 与 `query_tubi_insts_by_flow` 的诊断标记改为记录标记与端点删除的并集，并在 8009 实库上验过缺字段行仍解析为 false。
- [x] T007 [US1] 复核空间归属与料表口径：`room_model.rs` / `spatial_state.rs` 不读 `tubi_relate`，料表 surql 走 `->n`，均不受新行影响，因此无代码改动。
- [x] T008 `changelog.md`：记录不可成管直段从静默丢弃改为诊断线型。
- [x] T009 `docs/evidence/2026-08-20-invalid-tubing-diagnostic-line.md`、`docs/2026-08-12_live-test-ledger.md`：执行并登记指定 BRAN live 复测（6 行，后两段 `invalid=direction`；Plant UI A/B 证明标记驱动虚线）。
- [x] T010 运行 `rustfmt`、定向 `cargo test`（新增 5 条 + 016 的 7 条全绿）、`cargo check`。

## Dependencies

- T003 → T004 → T005 → T007。
- T006 可与 T003–T005 并行，但当前会话串行执行以避免同工作树冲突。
- T008 在代码终态后执行；T009/T010 依赖 T004–T007。
