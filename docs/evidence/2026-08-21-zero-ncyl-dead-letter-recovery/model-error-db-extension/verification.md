# 基本体错误入库验证

日期：2026-08-21（Asia/Shanghai）

## 数据库结果

执行 `UPSERT geom_error:['primitive','24381/38635']` 后立即回读，退出状态 0：

```text
kind=primitive
target=24381/38635
noun=NCYL
source_action=regen_root
generation_root=24381/38436
occurrences=1
last_error=targeted primitive 24381_38635 (NCYL) produced an invalid BREP shape; DIAM=0, HEIG=0
first_seen_at=2026-08-21T03:06:43.843610300Z
last_seen_at=2026-08-21T03:06:43.843612700Z
```

旧部署的 `/api/v1/health` 已读到：`geom_errors.total=15`、`last_kind=primitive`、
`last_target=24381/38635` 和包含零尺寸的 `last_error`。新构建部署后，动态 `by_kind`
还会明确给出 `primitive=1`，并增加 `last_noun=NCYL`。

## 代码验证

- `cargo test ... the_ledger_statements_round_trip_on_surreal`：退出 0；`geom_error` 与 `parse_error` 两条同名内存 Surreal 回归均通过。
- `cargo test ... zero_sized_targeted_ncyl_reports_the_dimensions_and_stays_an_error`：退出 0。
- `cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd,http_api`：退出 0。
- Release 构建：退出 0，`Finished release profile [optimized] target(s) in 2m 09s`。
- `sigmap verify-plan`：退出 0。
- `sigmap verify-ai-output`：退出 0，`no hallucinations detected (2479 symbols indexed)`。
- `sigmap review-pr`：按要求执行；共享工作树已有 495 文件变更，仍报告同一批 106 项全树 finding，隔离差异见本目录 patch。

Release：`D:/Rust/target/release/aios-database.exe`
SHA-256：`9dcd3f703ebfb214134d0f9a85690de2095caad692cc3deff65f7a20d2079199`

## 恢复

- 数据行 dry-run/守卫回滚：`scripts/Rollback-PrimitiveGeomErrorBackfill.ps1`
- 代码反向差异：`model-error-db-extension/rollback.patch`
- 修改前 SHA：`model-error-db-extension/baseline/SHA256SUMS.txt`
