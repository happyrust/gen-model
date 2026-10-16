# 房间初始化不受限定域限定（2026-10-15 第三轮）

- 仓库：`d:\work\plant-code\old\gen-model`，分支 `feat/issues-36-50-34`
- 起点 HEAD：`ccc18ad5a`（本轮开工时的工作区，未碰另一条线的未提交改动）
- 口径来源：用户当面裁定——**「watch 目前只是作为增量监测的范围，不要限定房间的初始化」**。
  仓库里早有同一条源码钉：`fast_model/room_resident.rs::room_files_never_consult_the_watch_scope`
  （「房间是参照系，不是增量的一部分；限定域圈的是数据批次」）。
- 范围：房间侧三处被 `watch_scope::admits` 圈住/被它抹掉证据的地方。不改任何可观测的调度顺序、
  水位语义、会话语义。

## 1. 三处缺陷

| # | 位置 | 现状 | 后果 |
|---|---|---|---|
| ① | `room_recompute.rs::check_and_init_rooms_at_startup` 的 `NeedsFullInit` 臂 | 目标集合 =（初始化覆盖库 ∪ 含房库）**∩ 限定域**，全被滤掉时才退回含房库 | 未监听库的房间**从没被初始化过**（「初始化」被增量监测的范围圈住了） |
| ② | `web_service/handlers.rs::run_rooms_rebuild` 的逐库列表 | `expected_dbnums()` 经 `admits` 过滤 | 整库重建同样只覆盖监听库 |
| ③ | 同函数的增量臂 | 限定域外的库打印「跳过」后 `continue`，但随后 `refresh_room_update_record_with_model` 仍把它记成当前水位 | 「欠一次增量检查」的证据被**抹掉**：之后该库进了监听范围也再判不出来 |

① / ② 是「限定域越权到初始化」；③ 是「跳过可以，但不许替它说已看过」——两条都从同一句口径推出来。

## 2. 改动（+118/−24）

- `room_recompute.rs`
  - 新增纯函数 `startup_full_init_targets(&BTreeSet<u32>, &[u32]) -> BTreeSet<u32>`：并集，**不看限定域**。
  - 新增纯函数 `hold_unchecked_room_sesnos(&mut RoomUpdateRecord, Option<&RoomUpdateRecord>, &BTreeSet<u32>)`：
    没看的库在记录里留白——保留上一笔记录的会话号，上一笔没有就写 `0`。
  - `refresh_room_update_record_with_model` 增两个参数（`unchecked`、`previous`），留白排在 `source_sessions`
    回填**之后**（回填替不了没看的库说话）；整库初始化与整库重建传空集合 + `None`。
  - `NeedsFullInit` 臂改用纯函数（删掉过滤与「滤空退回」）；增量臂维护 `unchecked` 并如实出声。
- `web_service/handlers.rs`
  - `run_rooms_rebuild` 逐库列表改为 `outcome.expected_dbnums()`（不筛）。
  - 收尾日志如实：`all_ok` 才说「完成」，否则 eprintln「有未完成项…`room_updated_at` 不推进」。

## 3. 证据

### 3.1 红（修复前，只有新钉）

```text
cargo test --offline --lib room_initialization_and_the_full_rebuild_never_consult_the_watch_scope -- --nocapture --test-threads=1
```

- 第一次运行：`test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2643 filtered out`
- 失败断言「整库重建不得按限定域圈库（房间是参照系，不是增量的一部分）」，消息里回显了
  `run_rooms_rebuild` 的整段函数体，其中 `.filter(|dbnum| crate::data_interface::watch_scope::admits(*dbnum))`
  正排在 `.expected_dbnums()` 之后——是断言失败，不是编译失败。

### 3.2 绿（修复后）

```text
cargo test --offline --lib -- room_initialization_and_the_full_rebuild_never_consult_the_watch_scope \
    the_full_init_targets_are_the_union_and_never_the_watch_scope \
    an_unchecked_library_keeps_its_previous_sesno_in_the_record room_rebuild_regression_ \
    --nocapture --test-threads=1
   → test result: ok. 7 passed; 0 failed; 0 ignored

cargo test --offline --lib
   → test result: ok. 2514 passed; 0 failed; 132 ignored; finished in 13.59s
     （较上一轮的 2511 多 3 条，即本轮新增的三条）
```

- 编辑器诊断（`get_problems`，severity=warning，wait_ms=6000）：`room_recompute.rs`、`handlers.rs`、
  `room_rebuild_regression_tests.rs` 均 errors 0 / warnings 0。

### 3.3 三条新测试的证据等级（照实记）

- `web_service::handlers::room_rebuild_regression_tests::room_initialization_and_the_full_rebuild_never_consult_the_watch_scope`
  —— **源码契约守卫**：`run_rooms_rebuild` 函数体不含 `watch_scope` 且含 `expected_dbnums()`；
  `NeedsFullInit` 臂不含 `watch_scope` 且走 `startup_full_init_targets(`；增量臂**保留**
  `watch_scope::admits(dbnum)` 且把跳过的库交给 `unchecked`；`refresh_room_update_record_with_model`
  函数体含 `hold_unchecked_room_sesnos(`。它证不了运行时走到哪一支。
- `data_interface::room_recompute::tests::the_full_init_targets_are_the_union_and_never_the_watch_scope`
  —— **纯函数真值表**：并集、两边皆空、初始化覆盖为空三种输入。
- `data_interface::room_recompute::tests::an_unchecked_library_keeps_its_previous_sesno_in_the_record`
  —— **纯函数行为**：没看的库保留上一笔会话号 / 上一笔没有则写 0 / 看过的库照旧写当前 / 整体时间戳照旧推进。

「运行时真的按留白记录判欠」那一半（启动两次、第二次仍把该库算成 `NeedsIncrementalCheck`）没有自动化
——它要 `SUL_DB` + `DbnumState` + 项目配置，本计划一律不碰。

### 3.4 哈希（修复后工作区文本）

| 文件 | SHA256 |
|---|---|
| `src/data_interface/room_recompute.rs` | `399f070e33c7c7cb4503ba2711290e321a76f63dd2b0cd2b45ab998b095d3b29` |
| `src/web_service/handlers.rs` | `62f80f063d90358ff1d0e06f0f2f300f3b52ecf0bfe93a874309ea567edfac3a` |
| `src/web_service/room_rebuild_regression_tests.rs` | `9bdf5fadaf2bcfecdbb234e2b73fd932a2ce0f1d9cb1e860bbd03150c26a1d3a` |

### 3.5 复现

```text
cargo test --offline --lib room_initialization_and_the_full_rebuild_never_consult_the_watch_scope -- --nocapture --test-threads=1
cargo test --offline --lib the_full_init_targets_are_the_union_and_never_the_watch_scope -- --nocapture --test-threads=1
cargo test --offline --lib an_unchecked_library_keeps_its_previous_sesno_in_the_record -- --nocapture --test-threads=1
cargo test --offline --lib
```

## 4. 范围外（本轮明确没做）

- 未改 `changelog.md`：仍带着另一条线（ADR-085 / 更新范围声明）的未提交改动，混入会把两件事装进一笔。
- 未动 `blocked_by_data` / `room_only_work_item` 那两处 `admits`（它们是**增量**触发侧，归限定域管）。
- 未动 `plan_startup_room_check`：它照旧把（模型含房 ∪ 上次记录的库）当候选，剔除与否现在由
  「初始化不看限定域、增量看限定域但留白」这两条实现。
