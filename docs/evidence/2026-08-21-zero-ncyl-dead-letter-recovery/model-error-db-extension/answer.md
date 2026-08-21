# 基本体错误持久化结果

- `src/data_interface/geom_error.rs` 新增 `primitive` kind、严格 `record_primitive_failure`、按 kind+target 的 `clear_primitive_failure` 和动态 `by_kind` health 汇总。
- `src/fast_model/prim_model.rs` 在 no BREP、invalid BREP、NaN transform 三个分支先持久写 `geom_error`，写入失败随模型失败上浮；成功后只销掉该基本体错误。
- 当前 `NCYL 24381/38635` 已回填为 `geom_error:['primitive','24381/38635']`，包含 noun、生成根、累计次数、首末时间以及 `DIAM=0, HEIG=0`。
- `scripts/Rollback-PrimitiveGeomErrorBackfill.ps1` 仅在记录仍匹配本次回填基线时删除该行。
