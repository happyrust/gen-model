# Tasks 019：布尔成品平表的存量收敛

- [ ] T001 [US1] `specs/019-booled-flat-backfill-closure/spec.md`：评审并定稿存量收敛、读路径对齐、双引擎形态的验收语义。
- [ ] T002 [US1] `specs/019-booled-flat-backfill-closure/plan.md`：Constitution Check 复核；`array::first` 与下标语法在 fork 上二选一定稿。
- [ ] T003 [US1] `src/test/fork_surreal_compat.rs`：先落失败回归——植入「`booled_id` 有值 + `insts_flat` 带缩放正体”的行，断言修复段收敛、二轮零行、正体行不动、空串 `booled_id` 不改写（无修复段时必须红）。
- [ ] T004 [US1] `src/fast_model/pdms_inst.rs`：`sweep_inst_relate_flat` 追加批量修复段（BATCH=500、`RETURN array::len` 收敛判断、脏值计数入日志），修订「行只会缺不会错」注释，补修复谓词的源码形状断言。
- [ ] T005 [US2] `../plant-ui/vendor/rs-core/src/rs_surreal/inst.rs`：`query_insts_flat` 投影改 `IF booled_id != NONE THEN [{ geo_hash: booled_id }] ELSE insts_flat END` 并回 `has_neg`；`FlatInstRow` 增字段、`GeomInstQuery.has_neg` 透传；补单测钉住布尔行/正体行两种投影。
- [ ] T006 [US3] `src/fast_model/manifold_bool.rs`：成功路径补 `booled=true`，同步更新 `empty_difference_is_bad_bool_not_a_silent_swallow` 形状断言。
- [ ] T007 [US2] 四路一致性抽查：同一 booled refno 走 flat / slim / insts / zone，断言 insts 与 has_neg 一致（可并入 T003 的 live 段或单独 `#[ignore]` 测试）。
- [ ] T008 [US2] 取证 `'none'` 字面量：8009 上 `SELECT count() FROM inst_relate WHERE booled_id = 'none' OR booled_id = ''`，结果写进证据；据此给 `display_insts` 的过滤加注释（若恒零，注明纯防御）。
- [ ] T009 [US1] live 收敛（8009）：mismatch 基线计数 → 执行修复 → 复查为 0、二轮零行、`24381_36945` 保持正确、抽查正体行逐字节不变；证据落 `docs/evidence/2026-08-XX-booled-flat-backfill/`。
- [ ] T010 `changelog.md` 与 `../plant-ui/CHANGELOG.md`：各记一条存量收敛与读路径对齐。
- [ ] T011 运行 `rustfmt`、定向 `cargo test`（T003/T004/T005/T006 新增用例全绿）、`cargo check`（两仓）。

## Dependencies

- T003 → T004（先红后绿）；T005、T006 可与 T003/T004 并行，但同工作树建议串行。
- T007、T009 依赖 T004 + T005 落地；T008 可随时先行。
- T010/T011 在代码终态后执行。

## Notes

- 016/017/018 尚有在飞未提交改动，本特性动工前先确认 `src/fast_model/pdms_inst.rs`
  与 plant-ui `inst.rs` 的工作树状态，避免覆盖别人的现场。
- `.specify/feature.json` 当前指向 017，本提案未切换指针；正式动工时再切。
