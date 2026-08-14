# Tasks

- [x] T1（串行）`docs/adr/ADR-024-shape-save-coalescing.md`、`specs/005-shape-save-coalescing/*`：完成 ADR → spec → plan → tasks 与 SigMap plan 核验。
- [x] T2（串行）`src/fast_model/shape_save.rs`、`src/fast_model/mod.rs`：以纯测试落 `SaveMode`、有界批、flush 规则、统计与 typed conflict。
- [x] T3（串行，依赖 T2）`src/fast_model/pdms_inst.rs`：拆出先计划后删除的确定性 `SavePlan` 构建和分阶段执行，移除 `param_map` 与静默 NaN 跳过。
- [x] T4（串行，依赖 T3）`src/fast_model/gen_model.rs`：定向/全量 receiver 接入统一保存器，成功 outcome 才计 produced，改结构化汇总日志。
- [x] T5（可与文档证据并行）`src/fast_model/shape_save.rs`、`src/fast_model/pdms_inst.rs`、staging 测试：补确定性、冲突、阈值、失败、幂等和性能门。
- [x] T6（串行）`docs/evidence/`、`docs/2026-08-12_live-test-ledger.md`：完成 CI 口径测试、test-workspace A/B 与 live 台账。
- [x] T7（串行）全 diff：`cargo fmt`、`cargo check`、SigMap 输出复核、提交并推送。
