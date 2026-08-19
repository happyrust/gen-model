# 2026-08-19 Dabacon 增量读取完整性证据

## 结论

- `parse_pdms_db` 的最小身份解析能在 canonical 解码失败时识别 MNUM，普通 noun 不会误入白名单。
- `pdms-io` 的终稿、refno、索引 child、文件身份和捕获期 length/header 稳定门定向测试全部通过；UDA add/modify/delete raw hash/value 路径通过。
- 主仓在持久化前强制要求 `SnapshotToken`，冻结 token 随 `FileCandidate` 跨队列传递；模型存在性复核与空模型计划提交门均复用同一冻结 target。基线 writer 失败清理全部已调度 dbnum，CATA error/empty/missing 均不发布缓存。
- issue-019/020 离线固定窗口四个集成目标全部通过，无假 Deleted。

## 绿色门禁（exit 0）

```text
old-parse-pdms-db: cargo test boundary_tests --lib -- --nocapture
  test result: 9 passed
old-parse-pdms-db: cargo fmt -- --check

old-pdms-io: cargo test net_window --lib -- --nocapture
  test result: 20 passed
old-pdms-io: cargo test session_index_diff --lib -- --nocapture
  test result: 14 passed; 1 ignored
old-pdms-io: cargo test snapshot --lib -- --nocapture
test result: 3 passed
old-pdms-io: cargo test raw_uda_hash_value_path_preserves_add_modify_delete --lib -- --nocapture
  test result: 1 passed
old-pdms-io: cargo fmt -- --check

gen-model: cargo test --locked --test db8000_session_pairs --no-default-features --features ws,gen_model,manifold,project_hd,legacy_session_replay -- --nocapture
  test result: 21 passed
gen-model: cargo test --locked --test db_session_fixture_selfcheck --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
  test result: 15 passed
gen-model: cargo test --locked --test db8000_two_delete_fixture --no-default-features --features ws,gen_model,manifold,project_hd,legacy_session_replay -- --nocapture
  test result: 6 passed
gen-model: cargo test --locked --test pdms_record_boundary --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
  test result: 3 passed
gen-model: cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd
  cargo check: 0 errors, 208 warnings
gen-model: cargo check --locked -p aios-py
  cargo check: 0 errors, 205 warnings
gen-model: cargo fmt --all -- --check
三仓 task-specific git diff --check
```

依赖 rev 并行调整后，最终门禁另以 Cargo CLI patch 同时绑定本地 `old-aios-core`、
`old-parse-pdms-db`、`old-pdms-io` 复跑（`--offline`，随后逐字节恢复现场 `Cargo.lock`）：
主仓 `cargo check` 为 0 errors/207 warnings，`aios-py` 为 0 errors/204 warnings；四个集成目标
仍为 21/15/6/3 全绿。

新增定向回归同样为 exit 0：`assert_refresh_candidate_snapshot_contract`、
`empty_model_plan_cannot_bypass_snapshot_generation_verification`、
`baseline_writer_failure_cleans_every_scheduled_dbnum`、
`locator_scan_failure_is_a_result_and_cannot_cache_an_empty_success`，以及
`finalize_state_is_registered_without_entering_the_journal` 中的暂存 finalize 上下文断言。

## Oracle 复核

Oracle 会话 `dabacon-completene-final-review-20260819`（GPT-5.6 Sol，extra-high）给出五项：
冻结 token 在 refresh→collect 间丢失、全量 reader 读到冻结长度之后、writer 失败未纳入清库、
空模型候选绕过世代门、CATA `Ok(empty)` 被缓存。实现逐项收口，并新增上述回归；最终补丁再经
同模型会话 `dabacon-completene-remediatio-review-20260819` 聚焦复核，确认前四项闭环，并补充
初次捕获 append 竞态、旧空 Ref0 缓存与 Required 未解析根两项读侧 P1。二次修补加入稳定捕获
复核、缓存 schema version、空 hit 驱逐和 Required 阻断。会话
`dabacon-completene-final-p1-review` 随后指出捕获错误在边界变化时应重试而非提前上浮；将
capture `Result` 延后到前后边界裁决后，会话 `dabacon-completene-final-zero-review` 最终结论为
`0 P0 / 0 P1`。完整记录见交付目录 `verification.md`。

上述 cargo 命令均设置 `CARGO_INCREMENTAL=0`；未执行 `cargo clean`。

## 全量 vendor 现状

- `pdms-io cargo test --lib`（以 CLI `--config` 临时指向本地 parse 仓）得到 72 passed、2 ignored、5 failed。失败为既有 Windows 路径字面量断言，以及 DbOption/数据库/现场文件前置；本次新增测试均通过。
- `parse_pdms_db cargo test --lib` 得到 40 passed、9 ignored、98 failed。失败集中于缺少 `noun_flags.json`、现场样本和现有 DbOption 形态；最小身份的两条新增测试通过。

## 现场边界

此前同日 data-only 8000 已验证 `data=true/model=false/room=false` 的阶段隔离，但现场仍有
`staging_8000_1` active 且水位停在 33（见 `2026-08-19-increment-stage-data-only-live.md`）。
本轮未在该未收口事务上追加写入型复跑，因此只声明冻结窗口与离线 fixture 通过，不声明新的
8009 数据提交结果。
