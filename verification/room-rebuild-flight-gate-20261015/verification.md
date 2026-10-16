# 第三段的「在飞」不许只活在展示帧里（2026-10-15）

- 仓库：`d:\work\plant-code\old\gen-model`，分支 `feat/issues-36-50-34`
- 改动文件（4）：`src/fast_model/room_resident.rs`、`src/web_service/handlers.rs`、
  `src/web_service/room_rebuild_regression_tests.rs`、`src/fast_model/room_rebuild_regression_tests.rs`
- 本文按约定不入库（`verification/`）；结论已写进 `docs/verification/2026-10-15-room-rebuild-regressions.md`。

## 缺陷

#53 的准入纪律「上一趟逐库边重算（`edge-recompute`）还在飞时不许放第二趟『重新生成房间』进」，
在飞判据是 `ResidentProgress` 的**展示帧**：`phase == "edge-recompute" && dbnums_done < dbnums_total`。

第三段跑在 `Ready` 下（既成事实：`try_begin_rebuild_in` 的注释与
`room_rebuild_regression_second_request_rejected_during_edge_recompute` 都这么说），而**任务开工前
对齐的换代**会把同一帧清零重写：

```text
refresh_before_task（每个任务开工，ADR-073 D12；只在 lifecycle 是 Loading 时早退——第三段是 Ready，挡不住）
  → 探到 composite 变了
  → initialize_file_backed_at_startup(...)
  → mark_loading("mesh-warm")            // room_resident.rs:2068（同 2293/2332 三处）
  → reset_progress("mesh-warm") → reset_progress_in(全局槽, "mesh-warm")
      → *slot = ResidentProgress { phase: "mesh-warm", dbnums_*: 0, ..default() }
```

于是：第三段还在飞，`phase` 已被抹成 `mesh-warm`、`dbnums_total/done` 归 0 → 闸判「没在飞」→
另一枚 `POST /api/v1/rooms/rebuild` 照进（`Loading` + 第二趟 `rebuild_resident_model` → 它的
`clear_room_rebuild_edges_on` 与第一趟的逐库循环交错）→ 两趟交错清边、互相覆盖盖章 = #53 要挡的
事重新出现。同时 `GET /health` 的 `room_build.resident.progress` 显示 `mesh-warm 0/0`，客户端
会把「第三段仍在跑」当成空闲/已完成。

触发面不是边角：第三段按库逐个整库重算（分钟量级），任务开工每个都探一次 composite，文件真变过
时换代必发生；而「重新生成房间」正是用户改完 MBD 之后按的那一下。

## 修法

1. `ResidentProgress` 增 `edge_recompute_in_flight: bool`：这一格不是展示，是准入判据；
2. `note_recompute_dbnums_total`（第三段开工）立起它；新增 `note_edge_recompute_finished()`
   在逐库循环跑完（成败都算）时放下它；`run_rooms_rebuild` 的循环由这一对括起来；
3. `reset_progress_in` 清零展示帧时**从被换掉的那帧接手**这个事实：
   `in_flight = slot.edge_recompute_in_flight || (slot.phase == "edge-recompute" && slot.dbnums_done < slot.dbnums_total)`；
4. `try_begin_rebuild_in` 两条都看：新那一格是判据，`phase` + 计数留作兜底（也覆盖立起来之前的一瞬）。

## 复现

```cmd
cd /d d:\work\plant-code\old\gen-model
cargo test --offline --lib room_rebuild_regression_
cargo test --offline --lib
```

## 红（修复前）

```text
test fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate ... FAILED
thread '...a_generation_swap_must_not_reopen_the_edge_loop_gate' (62804) panicked at src\fast_model\room_rebuild_regression_tests.rs:101:5:
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 2641 filtered out; finished in 0.41s
```

红钉只用现成符号（编译得过、断言失败）：把帧摆成第三段在飞（`edge-recompute` 1/3）→ 用
`mark_loading` 走的那一步 `reset_progress_in(&progress, "mesh-warm")` 清零 → 断言闸仍拒。
第一条 `control` 断言（帧还写着 `edge-recompute` 时拒）通过，说明夹具本身对；失败在第二条。

## 绿（修复后）

```text
test web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_success_stamp_has_mirror_invalidation_contract ... ok
test web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror ... ok
test fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_second_request_rejected_during_edge_recompute ... ok
test fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate ... ok
test web_service::handlers::room_rebuild_regression_tests::room_initialization_and_the_full_rebuild_never_consult_the_watch_scope ... ok
test web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_same_generation_failure_keeps_alignment_owed ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2641 filtered out; finished in 0.44s
```

红钉另加收工方向断言（`note_recompute_dbnums_total_in` 立起 → 拒；`note_edge_recompute_finished_in`
放下 → 放），保证这一格不会把闸焊死。

全量：

```text
test result: ok. 2516 passed; 0 failed; 132 ignored; 0 measured; 0 filtered out; finished in 16.42s
```

（较上一轮 2514 +2：行为红钉 + 源码契约钉 `room_rebuild_regression_the_edge_loop_flight_is_not_a_display_frame_fact`。）

编辑器诊断（`get_problems`，四个文件逐个）：errors 0 / warnings 0。

## 文件 SHA256（修复后）

```text
src/fast_model/room_resident.rs                  EB300F5FA701A982531056A9E5B4C42A592324789CD1BB8B0BDA248A6C5421EA
src/web_service/handlers.rs                      357954EA9B505771B845CB8A1E263D97324B2CBC011201094E363DE657496DAC
src/web_service/room_rebuild_regression_tests.rs 9CA53D444F012BF87E1AB1973ACA4C7A069898D24ED9B8512FD3F2A3982C1244
src/fast_model/room_rebuild_regression_tests.rs  5F82466D5653A7C4A75F3F55468EA8814B2DD7EE873ECA796D86C0F0C607F977
```

## 证据等级与未覆盖面

- 红钉是**行为**证据：真调 `try_begin_rebuild_in` / `reset_progress_in`，fixture 自持生命周期与
  进度槽，不碰进程单例、不碰库。
- 源码契约钉只钉接线位置（循环前 `note_recompute_dbnums_total`、循环后 `note_edge_recompute_finished`；
  `reset_progress_in` 含接手；闸读那一格）。
- 未覆盖（要真机）：运行时换代**真的**落在第三段期间的现场（任务开工 + composite 变化 + 第二枚
  `POST /rooms/rebuild`）；`/health` 客户端在新格上的显示。
- 已知同一条失效面（未扩大）：循环中途 panic 会把 `edge_recompute_in_flight` 留在「在飞」——与修前
  「帧停在 `edge-recompute` 且没跑完」等价，未加 RAII 兜底（不在本次范围）。
