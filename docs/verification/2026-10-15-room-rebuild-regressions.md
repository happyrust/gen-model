# 房间重建：修复前的隔离回归测试

日期：2026-10-15。基线 HEAD：`700e22c01`，叠加用户原有未提交修改。

## 范围与安全边界

仅新增测试及不改变行为的测试入口提取，不修复业务逻辑。

- `handlers.rs`：原清边 SQL 提取到 `clear_room_rebuild_edges_on(db)`，生产仍传 `SUL_DB`。
- `room_resident.rs`：原准入判断提取到 `try_begin_rebuild_in(lifecycle, reset)`，生产仍使用原全局槽与回调。
- 测试不调用 HTTP 重建端点、不初始化 `SUL_DB`、不加载项目配置、不读取 E3D 源模型，也不修改进程级镜像。
- 唯一数据库写入对象是测试新建的 `mem://` 嵌入式连接；无 TCP 监听、无外部数据库。

## 运行

```text
cargo test --offline --lib room_rebuild_regression_ -- --nocapture --test-threads=1
```

## 实测结果：编译成功，三条预期断言失败

退出码 101，`0 passed; 3 failed; 0 ignored`。这些是未修复缺陷的红测试，不是编译失败；未使用 `should_panic` 或 `ignore` 隐藏失败。普通完整测试套件也会包含它们，在对应修复前保持红色。

1. `room_rebuild_regression_same_generation_failure_keeps_alignment_owed`
   - 全新内存库里建立两类关系边及代号 g1 的水位。
   - 控制断言确认清边前有边且已对齐。
   - 执行生产清边 helper；控制断言确认两张关系表确实已空。
   - 在清边后注入重算失败边界：不发布替代边、不写成功章，源输入仍是 g1。
   - 正确行为应继续欠重算，实测 `alignment=Some("g1")`，电平型判据仍认为已对齐，故断言失败。
   - 这是清边/恢复凭证的边界行为测试，不是完整 HTTP 或后台任务端到端测试。
2. `room_rebuild_regression_second_request_rejected_during_edge_recompute`
   - 对局部生命周期槽运行真实生产准入 helper。
   - 控制断言确认 Loading 期间第二次请求被拒。
   - 按当前后台流程，把生命周期切为 Ready、阶段设为 edge-recompute。
   - 实测第二次请求获准，故断言失败。
   - 此测试验证状态准入漏洞，不是双 HTTP 请求/数据库写入交错的并发压力测试。
3. `room_rebuild_regression_success_stamp_has_mirror_invalidation_contract`
   - 对当前生产水位写入函数的源代码做限定范围检查。
   - 直接 UPDATE 之后缺少 DbnumState 镜像失效调用，故断言失败。
   - 这是源码契约守卫，不是缓存行为复现；未来若把写入封装进统一入口，需要同时更新此守卫。

## 测试文件

- `src/web_service/room_rebuild_regression_tests.rs`
- `src/fast_model/room_rebuild_regression_tests.rs`

首次运行时测试计数夹具的返回形状与 SDK 不一致，已改为显式 `count` 对象解码并重跑；最终三条均在上述目标断言失败，而非夹具初始化/反序列化失败。

## 后续修复（2026-10-15）

- 第 1 条（#52）：已按方案 A 修复。`clear_room_rebuild_edges_on` 在同一笔里把整表水位置为 `model_room_generation = NONE, model_room_stray_generations = []`；`run_rooms_rebuild` 在对 `SUL_DB` 清边成功后调用 `DbnumState::invalidate_mirror()`。
  - 单测：`cargo test --offline --lib room_rebuild_regression_same_generation_failure_keeps_alignment_owed -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`1 passed; 2 failed`，仍为红的只剩第 2 和第 3 条。这两条未在本次修改范围内。
  - `handlers.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改尚未提交，仍在当前工作区内。
- 第 2 条（#53）：已按「进度完成计数」方案修复。`try_begin_rebuild_in` 改为同时接收 lifecycle 与 progress 两个槽：生命周期为 `Loading`，或 progress 处于 `edge-recompute` 且 `dbnums_done < dbnums_total` 时拒绝，且拒绝时不重置进度、不翻生命周期；逐库循环完成后门自动放开。测试夹具由本地 `RefCell<String>` 改为本地 `ResidentProgress`，并补了「循环结束后重新放行」的断言。
  - 单测：`cargo test --offline --lib room_rebuild_regression_second_request_rejected_during_edge_recompute -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`2 passed; 1 failed`，仍为红的只剩第 3 条。
  - `room_resident.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改尚未提交，仍在当前工作区内。
- 第 3 条：已按最小方案修复（本次未单独建 GitHub Issue，经当面确认方案后直接修复）。`recompute_dbnum_room_edges_now` 的盖章由 `if let Err` 改为 `match`：`SUL_DB` 的 UPDATE 经 `.check()` 确认成功后，在 Ok 分支调用 `DbnumState::invalidate_mirrored(dbnum)`；失败分支保留原日志且不作废镜像。未连带调用 `note_room_generation_stamped`，理由是它只影响 ACK 逐出候选、后果保守且无测试覆盖，见当面方案记录。
  - 需要注意的是，第 3 条测试仍是源码契约守卫：它只能确认失效调用出现在 UPDATE 之后，无法自动证明调用位于成功分支；本次实现确实位于 Ok 分支，但这一点依赖代码评审，而非该测试本身。
  - 单测：`cargo test --offline --lib room_rebuild_regression_success_stamp_has_mirror_invalidation_contract -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`3 passed; 0 failed`。至此，本文件中的三条回归已全部完成修复。
  - `handlers.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改（本文件的第 1–3 条）随后以 `3cf1e2728` 入库；下面第二轮是同一条的尾巴。

## 后续修复（2026-10-15，第二轮）：盖章路径的第二层镜像

第 3 条当时明确留下一句「未连带调用 `note_room_generation_stamped`，理由是它只影响 ACK 逐出候选、
后果保守且无测试覆盖」。第二轮把它读完，结论是**后果不总是保守**：

`ScopeState::ack_is_stray_room_evidence`（`src/fast_model/model_memory_store.rs`）拿
`stamped_room_generations[dbnum]`（库上那一格的进程镜像）判一枚 ACK 还能不能进逐出候选。
`task_end::commit_task_end_on` 会同步这一格，但两处绕开 `task_end` 的写库点没有：
`recompute_dbnum_room_edges_now` 盖章（库上盖成**新**代、镜像还留**旧**代）与
`clear_room_rebuild_edges_on`（整表写 `NONE`、镜像没清）。两种情况下旧代的 ACK 都被当成
「库上已经是这一代」放行逐出——而它是「那几根的边是另一代算的」的唯一凭据，
`room_recompute::request_for_task` 的空段定向准入与 `query_service::room_calculation_status`
的一票否决都点它（ADR-073 F3 / d-149 补丁）。

- 修复：`handlers.rs` 两处各追加一行镜像 —— 盖章成功后
  `note_room_generation_stamped(dbnum, Some(rooms.resident_generation()))`（与写库同一个 `rooms`，代号同源）；
  清边成功那一支追加 `clear_stamped_room_generations()`；`ModelMemoryStore` 新增该方法（整表清 = 库上整表 `NONE`）。
  共 +144 行，全部为新增，未改任何既有语义。
- 红：`cargo test --offline --lib room_rebuild_regression_generation_stamp_feeds_the_process_eviction_mirror -- --nocapture --test-threads=1`
  → 修复前 exit `101`、`0 passed; 1 failed`，panic 在 `src\web_service\room_rebuild_regression_tests.rs:138`
  （断言失败，不是编译失败）。
- 绿：`cargo test --offline --lib room_rebuild_regression_ -- --nocapture --test-threads=1` → `4 passed; 0 failed`；
  `cargo test --offline --lib the_eviction_mirror_follows_the_durable_room_generation -- --nocapture --test-threads=1` → `1 passed`；
  全量 `cargo test --offline --lib` → `2511 passed; 0 failed; 132 ignored`。
- 编辑器诊断：`handlers.rs`、`model_memory_store.rs`、`room_rebuild_regression_tests.rs` 均 errors 0 / warnings 0。
- 证据等级照实记：`web_service` 新增那条仍是**源码契约守卫**（证明两处镜像调用都排在写库/清库之后、
  且盖章那处在成功分支一侧——写库与镜像之间不出现 `Err(` 文本；证不了运行时真的走到那一支）；
  `model_memory_store` 新增那条是**行为测试**，走真实 `enforce_root_limit` 逐出面钉两个方向
  （镜像整表清时 stray 代号的 ACK 不进候选；镜像同步到该代号后它不再是证据、按提交序出）。
- 证据原文（按约定不入库）：`verification/room-eviction-mirror-20261015/verification.md`，
  含三个文件的 SHA256 与三条复现命令。
- 范围外：`changelog.md` 本轮未改——它当前带着另一条线（ADR-085 / 更新范围声明）的未提交改动，
  混入会把两件事装进一笔。

## 后续修复（2026-10-15，第三轮）：房间初始化不受限定域限定

用户面授口径：**「watch 目前只是作为增量监测的范围，不要限定房间的初始化」**。仓库里本有同一条
源码钉（`room_resident.rs::room_files_never_consult_the_watch_scope`：「房间是参照系，不是增量的一部分」），
房间侧却有三处没照它办：

1. `check_and_init_rooms_at_startup` 的 `NeedsFullInit` 臂：目标集合曾是（初始化覆盖库 ∪ 含房库）
   **∩ 限定域**，全被滤掉时才退回含房库——未监听库的房间因此**从没被初始化过**；
2. `run_rooms_rebuild` 的逐库列表：`expected_dbnums()` 同样经 `admits` 过滤；
3. 增量臂：限定域外的库打印「跳过」后 `continue`，但收尾的记录刷新仍把它写成本轮水位——
   「欠一次增量检查」的证据被抹掉，之后它进了监听范围也再判不出来。

修法与证据等级：

- ① ② 去过滤：新增纯函数 `startup_full_init_targets`（并集，不看限定域），
  `run_rooms_rebuild` 直接用 `outcome.expected_dbnums()`。
- ③ 留白：新增纯函数 `hold_unchecked_room_sesnos`，跳过的库在记录里保留上一笔会话号（上一笔没有写 `0`），
  `refresh_room_update_record_with_model` 多两个参数（`unchecked` / `previous`），留白排在 `source_sessions`
  回填**之后**；增量臂如实出声「其中 N 个库…本轮未检查，欠账留存」。
- 顺带把 `run_rooms_rebuild` 收尾日志改成如实：`all_ok` 才说「完成」，否则 eprintln「有未完成项，
  `room_updated_at` 不推进」（此前无条件说「完成」，日志撒谎）。
- 红：`cargo test --offline --lib room_initialization_and_the_full_rebuild_never_consult_the_watch_scope
  -- --nocapture --test-threads=1` → 修复前 `0 passed; 1 failed`，失败断言「整库重建不得按限定域圈库」
  （消息里回显的 `run_rooms_rebuild` 函数体中 `watch_scope::admits` 过滤正排在 `expected_dbnums()` 之后）；
- 绿：同批七条 `7 passed; 0 failed`；全量 `cargo test --offline --lib` → `2514 passed; 0 failed; 132 ignored`
  （较上一轮 +3，即本轮新增三条）；`room_recompute.rs` / `handlers.rs` / `room_rebuild_regression_tests.rs`
  编辑器诊断 errors 0 / warnings 0。
- 证据等级：新增的源码契约守卫只钉接线位置（`run_rooms_rebuild` 不含 `watch_scope`、
  `NeedsFullInit` 臂不含且走纯函数、增量臂保留 `admits` 并把跳过的库交给 `unchecked`、刷新函数含留白调用）；
  另两条是纯函数真值表 / 行为测试。「运行时按留白记录判欠」要 `SUL_DB` 与项目配置，本轮不碰。
- 证据原文（按约定不入库）：`verification/room-init-watch-scope-20261015/verification.md`，
  含三个文件 SHA256 与四条复现命令。
- 范围外：`changelog.md` 仍未改（同上一条线未提交）；`blocked_by_data` / `room_only_work_item` 那两处
  `admits` 属**增量**触发侧，归限定域管，未动。

## 后续修复（2026-10-15，第四轮）：第三段的「在飞」不许只活在展示帧里

**缺陷**：#53 那条准入纪律（上一趟逐库边重算还在飞时不许放第二趟「重新生成房间」进）判「在飞」靠的是
`ResidentProgress` 的**展示帧**（`phase == "edge-recompute"` 且 `dbnums_done < dbnums_total`）。
第三段跑在 `Ready` 下——注释自己写着「那一段在 `Ready` 下跑，只看生命周期会漏守」——而**任务开工前
对齐的换代**（`refresh_before_task` → `initialize_file_backed_at_startup` → `mark_loading("mesh-warm")`，
`room_resident.rs` 三处）会把**同一帧**清零重写成别的相位（`reset_progress`）。于是：第三段还在飞，
另一枚 `POST /api/v1/rooms/rebuild` 照进（相位已被换代抹成 `mesh-warm`、计数归 0）→ 两趟重建交错
清边、互相覆盖盖章 = #53 要挡的事重新出现；`/health` 这时也如实报不出来（显示 `mesh-warm 0/0`，
客户端会当空闲/已完成）。触发面不是边角：第三段按库逐个整库重算（分钟量级），任务开工每个都探一次
composite，文件真变过时换代必发生，而这一按正是用户改完 MBD 之后按的。

**修法与证据等级**：

- `ResidentProgress` 增 `edge_recompute_in_flight: bool`：这一格**不是展示，是准入判据**；
- `note_recompute_dbnums_total`（第三段开工）立起它；新增 `note_edge_recompute_finished()` 在逐库循环
  跑完（成败都算）时放下它，`run_rooms_rebuild` 的循环由这一对括起来；
- `reset_progress_in` 清零展示帧时**从被换掉的那帧接手**：帧上写着 `edge-recompute` 且还没跑完 →
  新帧的 `edge_recompute_in_flight = true`（展示帧可以清零，判据不许被抹掉）；
- `try_begin_rebuild_in` 两条都看：新那一格是判据，`phase` + 计数留作兜底（也覆盖立起来之前的一瞬）。
- 红：新增行为红钉
  `fast_model::room_resident::room_rebuild_regression_tests::room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate`
  （只用现成符号：把帧摆成第三段在飞 → 用 `mark_loading` 走的那一步 `reset_progress_in` 清零 →
  断言闸仍拒）→ 修复前 `cargo test --offline --lib room_rebuild_regression_` = `5 passed; 1 failed`，
  panic 在 `src/fast_model/room_rebuild_regression_tests.rs:101:5`；第一条 `control` 断言（帧还写着
  `edge-recompute` 时拒）通过，失败在清零之后那条。
- 绿：同批六条 `6 passed; 0 failed`（红钉转绿，并补上「循环跑完闸重开、不许焊死」的收工方向）；
  全量 `cargo test --offline --lib` → `2516 passed; 0 failed; 132 ignored`（较上一轮 +2：行为红钉 +
  源码契约钉 `room_rebuild_regression_the_edge_loop_flight_is_not_a_display_frame_fact`）；
  四个改动文件编辑器诊断 errors 0 / warnings 0。
- 证据等级：红钉是**行为**证据（真调 `try_begin_rebuild_in` / `reset_progress_in`，fixture 自持生命周期
  与进度槽，不碰进程单例、不碰库）；源码契约钉只钉接线位置（`run_rooms_rebuild` 的
  `note_recompute_dbnums_total` 在循环前、`note_edge_recompute_finished` 在循环后；`reset_progress_in`
  含接手；闸读那一格）。「运行时换代真的落在第三段期间」要真机（任务开工 + composite 变化 + 第二枚
  `POST`），本轮不碰。
- 证据原文（按约定不入库）：`verification/room-rebuild-flight-gate-20261015/verification.md`，
  含四个文件 SHA256 与复现命令。
- 范围外：`changelog.md` 仍未改（同上一条线未提交）；换代期间 `/health` 显示换代的相位（`mesh-warm`）
  是如实，未改；循环中途 panic 会把新那一格留在「在飞」——与修前「帧停在 `edge-recompute` 没跑完」
  同一条失效面，未扩大（未加 RAII 兜底）。

## 后续修复（2026-10-15，第五轮）：收尾落定之后才放下「在飞」

**缺陷**：第四轮把「在飞」单列一格守住了第三段的起点与全程，**终点却画在收尾之前**——
`note_edge_recompute_finished()`（准入闸重开的那一刻）排在 `refresh_room_update_record_with_model`
（收尾往库里写 `room_update_record`）之前。循环一跑完闸就开，而第一趟还没把账记完。窗口里第二趟
「重新生成房间」能被放进来：它先清账（`record_room_update_record(None)` + `clear_room_rebuild_edges_on`：
两张边表全删、水位整表 NONE、记录 NONE），composite 没变时它的第一段只是一次探针（零重建），很快就能
走到清边；第一趟随后写下的收尾记录却是**清边之前那一刻**的账（它自己的 model + 当时读到的 states），
说「所有含房库都已做完」。第二趟若没跑完（`all_ok=false`，或进程被杀），库里留下的是：**边没了、水位
NONE、记录却说已最新**——下次启动检查只信记录（`is_dbnum_timestamp_newer_than_room`）判 `UpToDate`，
那批库从此不补，只剩逐库任务（D14 电平型欠账）的自愈。

**修法与证据等级**：

- `run_rooms_rebuild`：`note_edge_recompute_finished()` 移到 if/else 收尾**之后**；`rooms_rebuild` 的
  函数文档补一条（「准入闸重开的那一刻，库上的账（边 + 水位 + 记录）必须已经一致」）。
- 红：新增源码契约红钉
  `web_service::handlers::room_rebuild_regression_tests::room_rebuild_regression_the_flight_is_held_until_the_settle_lands`
  （切 `run_rooms_rebuild` 函数体，断言 `refresh_room_update_record_with_model(` 在
  `note_edge_recompute_finished(` 之前）→ 修复前
  `cargo test --offline --lib room_rebuild_regression_the_flight_is_held_until_the_settle_lands`
  = 退出码 101，`0 passed; 1 failed; 2651 filtered out`（断言把整个函数体打出来）。
- 绿：`cargo test --offline --lib room_rebuild_regression` → `8 passed; 0 failed`；全量
  `cargo test --offline --lib` → `2520 passed; 0 failed; 132 ignored`（当前工作树，含另一条线未提交
  改动）；两个改动文件编辑器诊断 errors 0 / warnings 0。
- 证据等级：源码契约（接线位置）+ 隔离回归；「两趟真的交错」要真机——窗口是第一趟收尾那几笔 DB 写
  （毫秒量级）而对面的第一段通常很长，只有 composite 没变的探针路才走得进去，本轮不碰。
- 证据原文（按约定不入库）：`verification/room-rebuild-settle-order-20261015/verification.md`。
- 范围外：`changelog.md` 仍未改（另一条线未提交改动在里面）；循环中途 panic 把「在飞」留在原处
  （未加 RAII / 超时自愈）仍在，未扩大。

## 六、「在飞」收口：守卫持有、Drop 放开（2026-10-15，第六笔）

**为什么**：第三处未闭面——逐库循环 / 收尾里 panic 展开时，「在飞」那一格留在原处，此后每一枚
`POST /api/v1/rooms/rebuild` 都 409「正在重建中」，直到重启。第五笔把判据从展示帧搬到
`edge_recompute_in_flight` 一格之后，这一格的「放」仍是手工成对（`note_edge_recompute_finished_in`）：
**panic 展开的路上根本走不到**。另外，准入闸此前还拿相位 / 逐库计数兜底，而循环里炸掉之后
展示帧会如实停在死的地方（`edge-recompute`、`done=1`）——同一把焊枪的第二个扳机。

**修法（守卫 = RAII 收口，"在飞"只有一个所有权明确的判据）**：

- `fast_model::room_resident::RoomRebuildFlight<'a>`（`#[must_use]`）：起手一次就位「相位 + 逐库
  总数 + 在飞」，`Drop` 走 `note_edge_recompute_finished_in`（放下那一格、相位回 `idle`）。
- 生产入口 `start_edge_recompute(total) -> RoomRebuildFlight<'static>`；测试入口
  `start_edge_recompute_in(&RwLock<ResidentProgress>, total)`。
- 三个公开项 `note_progress_phase` / `note_recompute_dbnums_total` / `note_edge_recompute_finished`
  **删除**——收工只剩守卫 Drop 一条路，谁都不许提前手工放开。
- `handlers.rs::run_rooms_rebuild`：`let _flight = resident::start_edge_recompute(dbnums.len());`
  在逐库循环**之前** → 循环 → 收尾 `refresh_room_update_record_with_model` → 函数末尾 Drop 才放开
  （第五笔「收尾落定之后才放」由守卫的作用域一并保持）。
- `reset_progress_in`（换代清零展示帧）与 `try_begin_rebuild_in`（准入闸）都**只读那一格**；准入不再
  拿相位 / 计数兜底——收口时特意没有把判据建回展示帧上，展示帧停在哪都不是「在飞」的证据。

**红 → 绿**：

- 行为红钉（`fast_model`）：`room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence`
  —— 帧停在 `edge-recompute`、`done=1 / total=3`、格 = false ⇒ 必须放行。修前
  `cargo test --offline --lib room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence`
  = 退出码 101。
- 展开路新钉：`room_rebuild_regression_a_panicking_edge_loop_must_not_shut_the_gate_forever` ——
  `catch_unwind` 包住 `start_edge_recompute_in` + 循环中途 `panic!` ⇒ 格落下、展示帧照旧停在死处、闸放行。
- 源码契约钉（`web_service`）：`room_rebuild_regression_the_edge_loop_flight_is_not_a_display_frame_fact`
  改成守卫形态（`let _flight = resident::start_edge_recompute(` 在循环前、文件里不许再出现
  `note_edge_recompute_finished(`、清零只读那一格、准入不许读相位 / 计数）；
  `room_rebuild_regression_the_flight_is_held_until_the_settle_lands` 改成「守卫活到函数末尾」形态。
- 绿：`cargo test --offline --lib room_rebuild_regression` → `10 passed; 0 failed`；全量
  `cargo test --offline --lib` → `2522 passed; 0 failed; 132 ignored`（15.66s；工作树含另一条线
  未提交改动）；四个改动文件编辑器诊断 errors 0 / warnings 0。

**顺手抓到的假绿**：上一轮的编辑把限定域源码契约钉
`room_initialization_and_the_full_rebuild_never_consult_the_watch_scope` 的 `#[test]` 弄丢了——文档注释
落在别处、`#[test]` 粘到相邻测试上（那个测试因此跑了两遍），**这条钉从没跑过**。本轮把属性与注释归位，
现在 10 条钉全在列表里、名字可核对。

**强杀这一面（为什么不需要超时自愈）**：库里没有被持久化的「正在重建」标记。开工时写下的持久面是
**两张边表清空 + 水位整表 `NONE` + `room_update_record` NONE**——它本身就是欠账信号
（`library_alignment_owed` 比代号判 NONE ≠ 代号即欠；下次启动检查信记录、记录 NONE ⇒
`NeedsFullInit`，目标集 `room_dbnums ∪ expected_dbnums`）。强杀只是让进程内那一格随进程一起消失：
重启后格是默认的 false，重建口不再 409，欠下的库由启动初始化 / 逐库任务补齐。所以这一面由「欠账 +
重启」兜着，不需要再加一层超时；RAII 只管住**同一个进程还在跑**的那些路（panic 展开）——这正是它
能覆盖、也是唯一需要它的地方。

**证据等级 / 范围外**：源码契约 + 隔离回归（panic 展开实测；「两趟真的交错」与真机强杀不在本轮）。
证据原文（按约定不入库）：`verification/room-rebuild-in-flight-guard-20261015/verification.md`。

