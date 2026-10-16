# 房间重建「在飞」的 RAII 收口 —— 2026-10-15（第六笔）

靶：`d:\work\plant-code\old\gen-model`，分支 `feat/issues-36-50-34`，起点 HEAD `a519d2c9`。
约定：本目录与 `verification/` 下的证据**不入库**（本文件即证据原文）。

## 一刀是什么

第三处未闭面：整库重建的第三段（逐库边重算 + 收尾）里 **panic 展开**时，`edge_recompute_in_flight`
留在原处 —— 此后每一枚 `POST /api/v1/rooms/rebuild` 都 409「正在重建中」，直到进程重启。
同一把焊枪还有第二个扳机：准入闸曾拿**展示帧**（相位 + 逐库计数）兜底，而循环里炸掉之后展示帧
会如实停在死处（`phase = "edge-recompute"`、`dbnums_done = 1`）——那一帧谁也不会再去改。

## 红（修前，行为钉）

```
cargo test --offline --lib room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence > .scratch\tt6-red.txt 2>&1
```

- 退出码 **101**（41.9s；stdout 重定向，末尾以上一次读数 lines=0 计）
- `.scratch\tt6-red.txt` 尾巴：

```
thread 'fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence' (67696) panicked at src\fast_model\room_rebuild_regression_tests.rs:139:5:
帧停在「死在哪一步」是如实、不是判据：准入只读「在飞」那一格，重建口不许被它焊死

failures:
    fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2652 filtered out; finished in 0.00s
error: test failed, to rerun pass `--lib`
```

## 修法形状（守卫持有、Drop 放开）

`src/fast_model/room_resident.rs`（`sha256 2db1933e…`，+57 −36）：

```rust
#[must_use = "守卫一落就放开重建准入闸：它要活到第三段（含收尾）走完"]
pub(crate) struct RoomRebuildFlight<'a> {
    slot: &'a RwLock<ResidentProgress>,
}

impl Drop for RoomRebuildFlight<'_> {
    fn drop(&mut self) {
        note_edge_recompute_finished_in(self.slot);
    }
}

pub(crate) fn start_edge_recompute(total: usize) -> RoomRebuildFlight<'static> {
    start_edge_recompute_in(progress_slot(), total)
}

fn start_edge_recompute_in(progress: &RwLock<ResidentProgress>, total: usize) -> RoomRebuildFlight<'_> {
    progress.write().unwrap_or_else(PoisonError::into_inner).phase = "edge-recompute".to_owned();
    note_recompute_dbnums_total_in(progress, total);
    RoomRebuildFlight { slot: progress }
}
```

- **开工一次就位**：相位 + 逐库总数 + 「在飞」同一笔立起（`start_edge_recompute_in`），此后没有
  「立起来之前」的缝。
- **放开只有一条路**：守卫的 `Drop`。三个公开项 `note_progress_phase` /
  `note_recompute_dbnums_total` / `note_edge_recompute_finished` **已删除** —— 收工不再有第二个写者。
- **清零只接手那一格**：`reset_progress_in`（换代清零展示帧）读 `slot.edge_recompute_in_flight`
  再原样带回，其余字段（相位 / 计数）按展示帧清零 —— 判据没有被建回展示帧上。
- **准入只读那一格**：`try_begin_rebuild_in` 看 `edge_recompute_in_flight`，不再看相位 / 计数。

`src/web_service/handlers.rs`（`sha256 b0b38c9c…`，+9 −9）：

```rust
let _flight = resident::start_edge_recompute(dbnums.len());
... 逐库循环 ...
refresh_room_update_record_with_model(...);   // 收尾
// 函数末尾 Drop 才放开 —— 第五笔「收尾落定之后才放」与展开路都在这一条路上
```

## 绿（修后）

```
cargo test --offline --lib room_rebuild_regression > .scratch\tt6-green.txt 2>&1
```

- **10 passed; 0 failed; 0 ignored** —— 列表（名字可核对）：

```
web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_success_stamp_has_mirror_invalidation_contract
web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror
web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_the_flight_is_held_until_the_settle_lands
web_service::handlers::room_rebuild_regression_tests::room_initialization_and_the_full_rebuild_never_consult_the_watch_scope
fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_second_request_rejected_during_edge_recompute
fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate
fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_panicking_edge_loop_must_not_shut_the_gate_forever
fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence
web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_the_edge_loop_flight_is_not_a_display_frame_fact
web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_same_generation_failure_keeps_alignment_owed
```

- 全量：`cargo test --offline --lib` → **2522 passed; 0 failed; 132 ignored**（15.66s；工作树含另一条线
  未提交改动）。
- 四文件编辑器诊断：errors 0 / warnings 0。

## 新增/改写的钉

- 行为红钉（本轮新，`fast_model`）：`…_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence`
  —— 帧停在 `edge-recompute` / `done=1` / `total=3`、那一格 = false ⇒ 必须放行。
- 展开路（本轮新，`fast_model`）：`…_a_panicking_edge_loop_must_not_shut_the_gate_forever`
  —— `catch_unwind` 包 `start_edge_recompute_in`，循环跑到一半 `panic!` ⇒ 格落下、展示帧照旧停在
  死处、闸放行。
- 源码契约（本轮改写，`web_service`）：`…_the_edge_loop_flight_is_not_a_display_frame_fact`
   —— `let _flight = resident::start_edge_recompute(` 在 `for dbnum in &dbnums {` 之前；文件里不许再出现
  `note_edge_recompute_finished(`；清零只接手那一格；准入不许读相位 / 计数。
  `…_the_flight_is_held_until_the_settle_lands` —— 守卫 < 循环 < 收尾，且无提前手工放开。

## 顺手抓到的假绿（上一轮遗留）

上一轮编辑把限定域源码契约钉 `room_initialization_and_the_full_rebuild_never_consult_the_watch_scope`
的 `#[test]` 弄丢了：属性粘到了相邻测试上（那条因此跑了**两遍**），它自己**从没跑过**。本轮把
`#[test]` 与文档注释归位，现在 10 条钉全在列表里、名字可核对（见上）。

## 强杀这一面（为什么不需要超时自愈）

库里没有被持久化的「正在重建」标记。开工写下的持久面是：两张边表清空 + `dbnum_watermark` 整表
`model_room_generation = NONE` + `room_build:main` 的 `room_update_record = NONE`。它**本身就是欠账
信号**：

- 水位 `NONE` ≠ 常驻代号 ⇒ `library_alignment_owed` 判欠（第五笔/缺陷一的口径）；
- 记录 `NONE` ⇒ 下次启动检查信记录 → `NeedsFullInit`，目标集 `room_dbnums ∪ expected_dbnums`
  （明确不看 `watch_scope`，2026-10-15 口径）。

强杀只是让**进程内**那一格随进程一起消失 —— 重启后格是默认 `false`，重建口不再 409，欠下的库由
启动初始化 / 逐库任务补齐。因此这一面由「欠账 + 重启」兜着，不需要再加一层超时；RAII 只管住
**同一个进程还在跑**的那些路（panic 展开），那也正是唯一需要它的地方。

## 文件与哈希（提交前工作树）

| 文件 | sha256（前 12） | 差异 |
| --- | --- | --- |
| `src/fast_model/room_resident.rs` | `2DB1933E6E65` | +57 −36 |
| `src/web_service/handlers.rs` | `B0B38C9CC0AF` | +9 −9 |
| `src/fast_model/room_rebuild_regression_tests.rs` | `6921CAD61B58` | +70 −21 |
| `src/web_service/room_rebuild_regression_tests.rs` | `F3063F3ED6CB` | +60 −55 |
| `docs/verification/2026-10-15-room-rebuild-regressions.md` | `FFE1341229FC` | 第六节 |
