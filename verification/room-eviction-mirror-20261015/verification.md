# 房间重建：进程逐出镜像补齐（2026-10-15 第二轮）

- 仓库：`d:\work\plant-code\old\gen-model`，分支 `feat/issues-36-50-34`
- 起点 HEAD：`8d8dceddd`（叠加用户原有未提交改动，本次未触碰那些文件）
- 范围：两处写 `dbnum_watermark.model_room_generation` 的地方补齐**进程逐出镜像**
  （`ModelMemoryStore::stamped_room_generations`）。不改调度、顺序、水位与会话语义。
- 本文件按约定不入库（证据原文）；可提交的叙述见 `docs/verification/2026-10-15-room-rebuild-regressions.md`。

## 1. 起因：上一轮记下的尾巴，核对后确认不是「保守」

`docs/verification/2026-10-15-room-rebuild-regressions.md` 第 3 条当时写着：「未连带调用
`note_room_generation_stamped`，理由是它只影响 ACK 逐出候选、后果保守且无测试覆盖」。

本轮把判据读到底：`model_memory_store.rs` 的 `ScopeState::ack_is_stray_room_evidence` 是

```text
ACK 的 room_generation ∈ stray_generations_written[dbnum]
  且 stamped_room_generations[dbnum] ≠ 该代号        → 是 F3 证据，不进逐出候选
```

`stamped_room_generations` 是库上那一格的**进程镜像**，`task_end::commit_task_end_on` 只在
收尾那一笔盖章/清章之后同步它。两处绕开 `task_end` 的写库点没有镜像：

| 写库点 | 库上那一格 | 镜像没跟上的后果 |
|---|---|---|
| `handlers::recompute_dbnum_room_edges_now` 盖章（整库重算） | 盖成**新**代 `gN` | 镜像里还留着**旧**代 `gO` → `stamped == gO` → 旧代 ACK 不再是证据、**放行逐出**。而它是「那几根的边是另一代算的」的唯一凭据 |
| `handlers::clear_room_rebuild_edges_on`（重建清边，整表） | 整表 `NONE` | 镜像里留着 `gO` → 同上，`gO` 的 ACK 被当成「库上已经是这一代」 |

后果不是省内存，而是**把另一代算的边当成这一代的**：`room_recompute::request_for_task` 的空段定向
准入与 `query_service::room_calculation_status` 的一票否决都点那枚 ACK（ADR-073 F3 / d-149 补丁）。
`clear_room_rebuild_edges_on` 只有一个生产调用点（`run_rooms_rebuild`），一处补齐即可。

## 2. 改动（+144 行，全部为新增）

- `src/fast_model/model_memory_store.rs`：新增 `ModelMemoryStore::clear_stamped_room_generations()`
  ——整表镜像清（与「整表写成 `NONE`」同义；逐库清仍走 `note_room_generation_stamped(dbnum, None)`）。
- `src/web_service/handlers.rs`：
  - `recompute_dbnum_room_edges_now`：UPDATE 确认成功后，除既有的 `DbnumState::invalidate_mirrored(dbnum)`
    之外，追加 `ModelMemoryStore::global().note_room_generation_stamped(dbnum, Some(rooms.resident_generation()))`
    （与写库同一个 `rooms`，代号同源）。
  - `run_rooms_rebuild`：清边成功那一支，除既有的 `DbnumState::invalidate_mirror()` 之外，
    追加 `clear_stamped_room_generations()`。

## 3. 证据

### 3.1 红（修复前，只有新测试）

```text
cargo test --offline --lib room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror -- --nocapture --test-threads=1
```

- exit `101`；`0 passed; 1 failed; 0 ignored`；`error: test failed`
- panic：`src\web_service\room_rebuild_regression_tests.rs:138`
  「整库重算盖章之后要把这一代告诉进程逐出镜像（否则另一代的 ACK 会被误判成库上那一代、放行逐出）」
- 断言失败，**不是**编译失败。

### 3.2 绿（修复后）

```text
cargo test --offline --lib room_rebuild_regression_ -- --nocapture --test-threads=1
   → test result: ok. 4 passed; 0 failed; 0 ignored          （原三条 + 新增源码契约一条）

cargo test --offline --lib the_eviction_mirror_follows_the_durable_room_generation -- --nocapture --test-threads=1
   → test result: ok. 1 passed; 0 failed; 0 ignored          （行为：镜像跟随库上那一代的两个方向）

cargo test --offline --lib
   → test result: ok. 2511 passed; 0 failed; 132 ignored; 0 filtered out; finished in 12.87s
```

- 编辑器诊断（`get_problems`，severity=warning，wait_ms=6000）：
  `handlers.rs` / `model_memory_store.rs` / `room_rebuild_regression_tests.rs` 三个文件均
  `errors=0 warnings=0`。

### 3.3 新增测试的两条证据等级（照实记）

- `room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror`（`web_service`）：
  **源码契约守卫**。它证明：两处镜像调用都排在写库/清库之后，且盖章那一处在成功分支一侧
  （写库与镜像之间不出现 `Err(` 文本）。它**不能**证明运行时真的走到那一支——那一半靠代码评审。
- `the_eviction_mirror_follows_the_durable_room_generation`（`model_memory_store`）：**行为测试**，
  走真实 `enforce_root_limit` 逐出面。两相：镜像整表清（库上 `NONE`）时 stray 代号的 ACK 不进候选、
  出的是最旧的普通根；镜像同步到该代号（库上盖到这一代）之后该代号不再是证据、按提交序出。
  它钉不到 handlers 的那两行（那条路要 `SUL_DB` 与项目配置，本计划一律不碰）。

### 3.4 哈希（修复后工作区文本）

| 文件 | SHA256 |
|---|---|
| `src/web_service/handlers.rs` | `56fb026103b51ef83df5ff2a5b8ed869db8b48fa45e68625e8443fea78bf6818` |
| `src/fast_model/model_memory_store.rs` | `9a1371588f87673a97eca421deed981bacbca25893b90eec87021a35297a8c12` |
| `src/web_service/room_rebuild_regression_tests.rs` | `77de21615d126947156c84fdb8aed6873a0d00553b9d30c86804e5604456c0dd` |

### 3.5 复现

```text
cargo test --offline --lib room_rebuild_regression_ -- --nocapture --test-threads=1
cargo test --offline --lib the_eviction_mirror_follows_the_durable_room_generation -- --nocapture --test-threads=1
cargo test --offline --lib
```

## 4. 范围外（本轮明确没做）

- 未改 `changelog.md`：该文件当前带着另一条线（ADR-085 / 更新范围声明）的未提交改动，混入会把两件事
  装进一笔；条目留待那一条线收口时补。
- 未动 `AIOS_WATCH_*`、未动 `run_rooms_rebuild` 的调度顺序、未动 `refresh_room_update_record_with_model`
  的推进纪律（上一轮已收口）。
- 未加「盖章后把整表镜像重新读库校准」这类额外机制：`task_end` 一行已有同款纪律，两处补齐即可。
