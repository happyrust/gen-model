# 零尺寸 NCYL 死信修复验证记录

日期：2026-08-21（Asia/Shanghai）

## 构建与静态质量门

| 命令 | 退出状态 | 字面结果 |
|---|---:|---|
| `cargo +nightly-2026-08-02 fmt --all -- --check` | 0 | 无格式差异 |
| `cargo +nightly-2026-08-02 check --locked --no-default-features --features ws,gen_model,manifold,project_hd,http_api` | 0 | `Finished dev profile`；仅既有 warning |
| `cargo +nightly-2026-08-02 build --release --locked --bin aios-database --no-default-features --features ws,gen_model,manifold,occ,project_hd,http_api` | 0 | `Finished release profile [optimized] target(s) in 2m 43s` |

Release 产物：`D:/Rust/target/release/aios-database.exe`（当前 `CARGO_TARGET_DIR=D:/Rust/target`）
SHA-256：`9dcd3f703ebfb214134d0f9a85690de2095caad692cc3deff65f7a20d2079199`

产物以 `aios-database.exe --help` 实际启动并退出 0，输出 `serve`、`trace`、`help` 命令面；`git apply --stat implementation.patch` 退出 0，解析出 15 个文件、1138 行新增和 13 行删除。

## 回归测试

以下过滤测试均使用 nightly、`--locked --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api -- --nocapture`，退出状态均为 0：

- `zero_sized_targeted_ncyl_error_contains_dimensions_and_still_fails`
- `non_cylinder_invalid_brep_error_keeps_generic_shape`
- `model_pending_status_splits_phases_actions_attempt_boundary_and_sample_cap`
- `unknown_pending_action_is_conservatively_data_phase`
- `model_dead_letter_notice_covers_first_suppression_repeat_change_and_recovery`
- `dead_letter_reporting_keeps_model_before_room`
- `health_is_degraded_only_for_dead_letters_or_query_failure`

## 脚本验证

| 命令 | 退出状态 | 字面结果 |
|---|---:|---|
| `powershell -ExecutionPolicy Bypass -File scripts/Repair-ZeroNcylDeadLetter.ps1 -Phase All` | 0 | 四阶段均打印 `[DRY-RUN]`，未写源文件或数据库 |
| `powershell -ExecutionPolicy Bypass -File scripts/Rollback-ZeroNcylDeadLetter.ps1` | 0 | 打印恢复目标及尚未生成的现场基线，未执行恢复 |

## SigMap

- `sigmap verify-plan specs/022-model-dead-letter-recovery/plan.md`：退出 0，PASS。
- `sigmap verify-ai-output docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/answer.md`：退出 0，`no hallucinations detected (2479 symbols indexed)`。
- `sigmap review-pr`：退出 1；扫描的是已有 495 文件变更的共享脏工作树，报告 106 项既有跨范围 finding（含 scope drift/secret heuristic），未把它们归因于本补丁。隔离的本次变更见 `implementation.patch`。

## 当前现场状态

- 原部署已恢复：PID `13804`，可执行文件 `D:/work/plant-code/old/test-worklspace/bin/aios-database.exe`。
- `GET http://127.0.0.1:9099/api/v1/health` 返回 HTTP 200、旧部署顶层 `status=ok`，`initialization.model_ready=false`，监听仍为 `[8000]`。
- 交互式 E3D Design PID `57080` 持有 `ams7997_0001` 独占锁；Backup 守卫在读取源文件前停止，不曾导出、删除 NCYL、导入或重建。
- 因成对现场基线尚未产生，T11/T12 保持未完成；现场结果没有标记为通过。

## 可恢复交付角色

- 修改后构建产物：`D:/Rust/target/release/aios-database.exe`
- 独立补丁：`D:/work/plant-code/old/gen-model/docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/implementation.patch`
- 本验证记录：`D:/work/plant-code/old/gen-model/docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/verification.md`
- 可运行回滚：`D:/work/plant-code/old/gen-model/scripts/Rollback-ZeroNcylDeadLetter.ps1`
