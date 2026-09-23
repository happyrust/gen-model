# room_membership 真机跑通 + 六条 mem:// / 真文件验收（ADR-073 D8–D13）

- 日期：2026-09-16（本机 UTC+8）
- 范围：把 S0–S4（ADR-073 2026-09-16 修订，D8–D13，源码钉 10–18）在 **ams /ALL 真文件** 上跑一遍，
  并把源码钉 14 / 16 / 18 那六条「未 live」的验收（缺 DESI / 缺面板几何 / 文件前进 / 改 PANE）逐条落实。
- 代码：`codex/model-projection-cache`（HEAD `da461b71f`）+ S0–S4 在途改动（未提交）。
- 目标：结束此前每一刀结尾都挂着的「未 live：源码钉 14 / 16 / 18 的六条 mem:// 真测都没跑」。

## 0. 一句话

`cargo test --lib` 全绿（**2142 passed / 0 failed / 111 ignored**），四条合成 mem:// / 单元验收（a/d/e/f）在内；
三条真 AMS 文件用例（缺 DESI 降级 b、开工前对齐重建 c、1112 房间边优化==参照 §9 硬验收）全过；
真机 `serve`（room_membership=1，真 /ALL）从空库把 **dbnum 1112** 生成一遍：18/18 根、**12342 条房间边落库、0 根失败**，
`/health` `status=ok / room=available / degraded=[]`。
同日下午再在**摄入数据面的 rocksdb 真库**（外部 SurrealDB 8049）上跑一遍（§6）：1112 / 7997 / 8000 三个 DESI 首次初始化全成（7743 根、0 失败、
`room_relate` 70386 条），冷重启不重摄入，`e3d.room.lookup` 对三个库的构件都回出**非空房间号**；材料表 `fn::room_num_of` / `fn::get_room_number`
回得出房间号，`fn::room_code`（hd 版）恒回 NONE——是它自己的 SurrealQL 块值缺陷（§6.5 F1），不是房间没算；用户拍板后已修
（`fn_query_room_code.surql` 分支改表达式形式 + mem:// 钉），重编二进制在同一座库上回 `1RX-R144`（§6.7），钉 9 四句全落实。
F2（durable 形态下 `calculation_status` 恒 `not_computed`）随后也拍了：水位行补一格整库房间对齐承诺
`model_room_generation`（ADR-073 追记 D14），同一座库上 `model/rebuild` 盖章 → **冷重启后**（进程内零 ACK）
`e3d.room.lookup` 回出 `ready_nonempty` 与 `ready_empty`（§6.8）。

## 1. 台子

| 项 | 值 |
| --- | --- |
| 二进制 | `cargo build --bin aios-database`（dev），`D:\Rust\target\debug\aios-database.exe`，自报 `0.1.27+gda461b71f14d.…dirty` |
| 测试 harness | `cargo test --lib --no-run`（同 worktree、同 target `D:\Rust\target`），增量 1m42s |
| schema | 缺 `AIOS_E3D_SCHEMA_ROOT` 时回落 `e3d_io::bundled_schema_root()` = `vendor\e3d-io\schema\e3d31`（自动，无需配） |
| meshes | `assets/meshes` 暖缓存（25,696 件），真文件用例与真机跑全部命中缓存、不重生 |
| 真文件根 | `D:\AVEVA\Projects\E3D3.1`（全 5 项目，含 SCB）；缺 SCB 的用例用联接根 `D:\adr078-verify\rootNoScb`（无 SCB → 6003 缺文件） |
| 配置 | `D:\adr078-verify\DbOption-room-verify-full.toml`（全 DESI）/ `-noscb.toml`（缺 SCB）/ `-room-live.toml`（真机 serve，mem 库、watch=[1112]、9199）/ `live-run-rocks\DbOption.toml`（§6：`store_mode="rocksdb"`、`v_port=8049`、watch=[1112] → [1112, 7997, 8000]） |
| 外部库（§6） | `bin\surreal.exe`（2.1.x）`start --bind 127.0.0.1:8049 rocksdb:D:\adr078-verify\live-rocks`，12:27 起、截稿仍在跑；`/sql` 直查用 `scripts/Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8049/sql` |

## 2. 单元 + 合成 mem:// 验收（a / d / e / f）

`cargo test --lib` → **2142 passed / 0 failed / 111 ignored（11.80s）**。其中覆盖六条里的四条：

- **(a) 缺面板几何不判死**：`a_registered_panel_without_geometry_keeps_its_old_edges_and_blocks_no_root`（model_db_adapter，mem://）、
  `a_registered_panel_without_geometry_is_protected_not_blocking`（room_model）。
- **(d) 前进无房间的 DESI → 代号不变**：`the_resident_generation_follows_the_calculation_input_not_the_file_composite`（room_model）。
- **(e) 改 PANE → 整库房间重算**：`moving_a_panel_updates_the_other_roots_edges_through_the_library_recompute`、
  `recomputing_a_librarys_room_edges_reuses_the_per_root_rendering_and_refreshes_acks`（model_db_adapter，mem://）。
- **(f) 只改 BRAN → 不触发**：`a_change_to_any_room_definition_noun_triggers_and_nothing_else_does`、
  `a_task_recomputes_rooms_on_either_trigger`、`the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end`（room_recompute）。

## 3. 真 AMS 文件验收（b / c / §9 1112 parity）

跑法：`$env:DB_OPTION_FILE=<config>; cargo test --lib -- --ignored --exact --nocapture --test-threads=1 <test>`。

### (c) 源码钉 16 —— 开工前对齐：陈旧键 → 重建 → 再探 Unchanged

`fast_model::room_resident::tests::ams_task_start_refresh_rebuilds_from_a_stale_key_and_then_reuses_the_generation`
（config = full）→ **ok（2.10s）**：

```
AMS_ROOM_REFRESH {"rebuilt_ms":2062,"probe_only_ms":23,"rooms":215,"registered_panels":230,
                  "missing_panels":0,"generation":"efd4ebe1…b46ad269","diff":"removed=3 recalc=230 renumbered_rooms=0"}
```

装一份键对不上此刻文件版本的陈旧模型 → 第一次 `refresh_before_task` 从真 /ALL 文件整份重建（`Rebuilt`、代号换真值、
`missing_panels=0`、`last_swap.note = task-start refresh`）；紧接着第二次探针 composite 已相同 → `Unchanged`、23ms 零重建
（这一半即 (d) 的真文件侧）。

### (b) 源码钉 14 —— 缺 DESI 文件：降级不判死

`fast_model::room_resident::tests::ams_missing_desi_degrades_the_room_reference_instead_of_blocking_it`
（config = noscb，`AMS_ROOM_EXPECT_MISSING_DESI=6003`）→ **ok（2.49s）**：

```
AMS_ROOM_MISSING_DESI {"missing_design":[[6003,"*MASTER/SCBDESI"]],"sources":28,"rooms":215,
                       "registered_panels":230,"missing_panels":0,"problems":1,"build_ms":779}
```

搜索根里没有 SCB → 探针把 6003 点进 `missing_design`、冻结源 28（少 SCB 那一个）；常驻模型**照样建起来**
（`load_latest` 不 Err）、生命周期 `Degraded`、`MemoryRoomCalculator::new` 照冻、`ensure_ready` 放行。

### §9 硬验收 —— 1112 房间边「优化 == 参照」

`fast_model::room_resident::tests::ams_cached_room_vertex_optimization_parity`（config = full）→ **ok（35.52s）**：

```
AMS_DBNUM_TIMING {"dbnum":1112,"sesno":729,"roots":18,"records":4892,"model_ms":2161,
                  "room_ms":30140,"optimized_room_ms":14028,"reference_room_ms":16109,
                  "room_edges":6171,"failures":[]}
```

18 根、6171 条 `room_relate`、0 失败；`check_parity=true` 内部断言「顶点优化路 == 参照逐边路」逐边相等
（ADR-010 §9 / ADR-073 实施约束 8 的对照物：同一份数据从零算的结果）。

## 4. 真机 serve 跑一遍（room_membership=1，真 /ALL，从空库）

`aios-database serve`，工作目录 `D:\adr078-verify\live-run`（内放 `DbOption.toml` = 上面的 `-room-live.toml`：
`store_mode="mem"`（进程内库、隔离、退出即消失，不碰任何真库）、`room_membership=true`、`watch_dbnums=[1112]`、
`auto_initialize_new_dbnums=true`、`http_api_addr=127.0.0.1:9199`、`project_path=D:/AVEVA/Projects/E3D3.1`）。

启动横幅（`live-1112-*.out.log`）：

```
[房间初始化] latest 文件 [] → 215 间房 / 230 块在册面板 / 230 块有几何 / 0 块无几何 / 网格 230/230 键 0.6 MB 耗时 751ms
```

`/health.room_build.resident`：`status=ready`、`enabled=true`、`mdb_dbnums_loaded` = 全 29 个 MDB DESI（`mdb_dbnums_missing=[]`）、
215 rooms / 230 registered_panels / 230 panels_ready / 0 missing。

从空 mem 库 `POST /api/v1/dbnums/1112/model/ensure`（memory-routed 整库任务）→ **succeeded**，`ready_total=18/18`：

```
[几何生成完成] elapsed_ms=4219 dbnum=1112 failed=0
[model-persist] dbnum=1112 roots=18 groups=1 rows=9784 removed=0 receipts=18
                room_edges=12342 commits=7 max_transaction_bytes=2396596 failed_roots=0 elapsed_ms=37724
[处理结束] dbnum=1112 completed=[18 根] failures=[] rows=9784 room_edges=12342
```

生成完成后 `/health`：`status=ok`、`init=model_ready`、`features.room = {status:available, generation:efd4…, executor:memory}`、
`degraded_sections=[]`（1112 有了数据，`dbnum_consistency` 随之消掉）。房间读路活着：`e3d.room.lookup` 回
`calculation_status=computed`、`room_membership=true`、`room_definition_version=efd4…`。

**读透 mem 的边界（不影响房间正确性）**：这台是 `data_face=read-through` 的内存库，只 ensure 了 1112 的模型投影，
没有把 `pe` / 元素行摄入库（读透形态下本就不摄入，见 `empty_kv_mem_publishes_an_indoor_root_without_attribute_ingestion`）。
所以对抽样的 1112 顶层结构根做 `e3d.room.lookup` 时 `memberships=[]`、`e3d.element.identity` 503——这是读透内存库没有 pe 行的
既有形态，不是房间没算：房间边确实落了库（persist 提交计数 12342、0 失败），且其正确性由 §3 的 1112 parity（6171 条逐边==参照）钉住。
要 `room.lookup` 回出非空成员名单，需要一台把数据面也摄入的 rocksdb 库——§6 就是那一台。

## 5. 结论：六条「未 live」逐条落实

| 源码钉 | 场景 | 落实方式 | 结果 |
| --- | --- | --- | --- |
| 14 (a) | 缺面板几何 → 保护集、不判死 | 合成 mem:// | 全绿（2142 suite） |
| 14 (b) | 缺一个 DESI → 降级不判死 | 真文件 `ams_missing_desi_degrades…` | ok（6003 缺 → Degraded） |
| 16 (c) | 文件前进 → 开工前重建 | 真文件 `ams_task_start_refresh…` | ok（Rebuilt→Unchanged） |
| 16 (d) | 前进无房间 DESI → 代号不变 | 合成 mem:// + (c) 再探 Unchanged | 全绿 / 23ms 复用 |
| 18 (e) | 改 PANE → 整库房间重算 | 合成 mem:// `moving_a_panel…` / `recomputing…` | 全绿 |
| 18 (f) | 只改 BRAN → 不触发 | 单元 `a_change_to_any_room_definition_noun…` | 全绿 |
| §9 | 1112 房间边 优化==参照 | 真文件 `ams_cached_room_vertex_optimization_parity` | ok（6171 逐边相等） |
| §9 / 钉 9 | 真机开 room_membership 跑一遍 | `serve` mem 库真 /ALL 生成 1112 | 18/18 根、12342 边落库、0 失败、health ok |
| 钉 9 | rocksdb 真库（摄入数据面）：`room.lookup` 非空 | §6 外部 surreal 8049，1112 / 7997 / 8000 首次初始化 + 冷重启 | 7743 根 0 失败、`room_relate` 70386、`room.lookup` 三库构件均非空 |
| 钉 9 | `/dbnums` 行：`write_state.status=finished`、无 `room_sesno` 格、`applied = model` | §6.3 `GET /dbnums?project=…&mdb=ALL` | 20 行 `finished`、整份回执 0 个 `room_sesno*` 键、三库 applied==model==in_sync |
| 钉 9 | stdout 无房间段耗时行 | §6.3 三份 serve 日志 grep | 0 命中（只有 `[房间初始化]` 常驻横幅与模型 lane 的 `房间边=` 计数） |
| 钉 9 | 材料表 `fn::room_code` 对生成过面板的房间回得出房间号 | §6.4 8049 `/sql` 直查 → §6.7 修 F1 后重编复验 | 修前 `fn::room_code`（hd）恒回 NONE（函数自身的块值缺陷 F1）；**修后**重编二进制起在同一座库上，`fn::room_code` 回 `1RX-R144` / `1RX-R212` / `1RX-R232` / `1RX-R231`；`fn::room_num_of` / `fn::get_room_number` 一直回 R144 等 |

未验：跨库成员（面板在 A、成员在 B）的传播（ADR-073 §未决 U3）；S5 死代码清理（§6.5 F3 的 `room_panel_relate` 写者就在其中）。

## 6. rocksdb 真库（摄入数据面）复跑：`room.lookup` 非空 + 材料表（钉 9 收尾）

§4 那台读透 mem 库没有 `pe` 行，`room.lookup` 只能回空；这一节换成**外部 SurrealDB（RocksDB 后端）+ `data_face=ingest`**，
让数据面（`pe` / `pe_owner` / `ATT_*`）与房间边落在同一座持久库里，然后看 `e3d.room.lookup` 与材料表函数。

### 6.1 台子

- 库：`bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8049 rocksdb:D:\adr078-verify\live-rocks`（12:27:49 起，空目录起步）。
- 服务：同一枚 dev 二进制 `0.1.27+gda461b71f14d.1789531584.dirty`（built 12:06:24），工作目录 `D:\adr078-verify\live-run-rocks`，
  `DbOption.toml` = §4 的 `-room-live.toml` 只改两键：`store_mode = "rocksdb"`、`v_port = 8049`（其余：`room_membership=true`、
  `auto_initialize_new_dbnums=true`、`startup_autorun=true`、`http_api_addr=127.0.0.1:9199`、真 /ALL、暖 meshes）。
  `AIOS_DATA_READ_MODE` 未设 = direct（读文件），数据面 ingest 是 `store_mode=rocksdb` 派生的档：横幅
  `存储介质：rocksdb（store_mode=rocksdb，连配置指的 ws://localhost:8049——本进程不拉起它）` / `数据面：ingest（整库基线 + 稳态增量窗口，推进 applied_sesno）`。
- 三段进程（日志 `D:\adr078-verify\live-rocks-1112-{122938,172117,172642}.out.log`）：

| 段 | 时刻 | watch_dbnums | 做了什么 |
| --- | --- | --- | --- |
| ① 空库首启 | 12:29:38 | `[1112]` | 重扫发现 18 个从未解析的文件（SYST/COMM/MISC + 14 个 DICT + 1112）逐个「首次按需初始化（全量基线 + 整库模型，双 lane 直写）」 |
| ② 冷重启 | 17:21:17 | `[1112]` | 同一座库、不重摄入：`模型水位核对完成：核对 1 库，落后 0 库`，启动总耗时 10.73s |
| ③ 扩监听重启 | 17:26:42 | `[1112, 7997, 8000]` | 为让材料表拿到面板那一侧的 `pe` 行（面板全在 7997 / 8000），把这两个 DESI 也首次初始化 |

### 6.2 初始化与生成（服务日志原句）

```
dbnum=1112 初始化完成 耗时=32397ms  pe=30942  worl=1 属性 lane: tx=35  bytes=21012732  冲突重试=0 超时=0；模型 lane: tx=3   bytes=12522818 根=18   失败根=0 记录=9784  房间边=12342 model_sesno=729
dbnum=7997 初始化完成 耗时=219470ms pe=170085 worl=1 属性 lane: tx=241 bytes=131634082 冲突重试=0 超时=0；模型 lane: tx=424 bytes=93252529 根=6772 失败根=0 记录=66425 房间边=68162 model_sesno=184
dbnum=8000 初始化完成 耗时=25462ms  pe=6639   worl=1 属性 lane: tx=42  bytes=16794705  冲突重试=0 超时=0；模型 lane: tx=60  bytes=3783655  根=953  失败根=0 记录=2785  房间边=4371  model_sesno=607
```

三库合计 **7743 根、0 失败根**；三段的 `[房间初始化]` 横幅一致：`215 间房 / 230 块在册面板 / 230 块有几何 / 0 块无几何`，常驻代 `efd4ebe1e71b`。
段 ③ 的队列看门狗记了一条 `queue_stall`：8000 在 7997 的 219s 初始化后面排队超过 60s（单消费者，预期形态，不是卡死）。
段 ③ 启动总耗时 4m14.4s，`/health` 收敛后 `room=available (executor=durable)`，resident 215/230/0。

库上直查（8049 `/sql`）：

| 表 / 行 | 值 |
| --- | --- |
| `pe` | 260429 行（1112: 30942 / 7997: 170085 / 8000: 6639，其余 SYS/DICT），`owner` 全部非空 |
| `room_relate` | **70386** 条（1112: 6171 / 7997: 59844 / 8000: 4371），182 个不同 `room_num`，40635 个不同成员 |
| `room_panel_relate` | 表不存在（0 行，见 F3） |
| `dbnum_watermark` | 1112 / 7997 / 8000：`applied_sesno == model_sesno` = 729 / 184 / 607；17 个 SYS/DICT 库 `model_sesno` 为空 |
| `dbnum_write_state` | 20 行，全部 `status=finished`、`intent=initialize` |

1112 的 6171 条与 §3 parity 用例的 6171 相同。日志里的 `房间边=` 计数与库上条数**不是一回事**（1112 12342 vs 6171、7997 68162 vs 59844、
8000 4371 vs 4371）——它数的是模型 lane 写出的边语句，本轮没有追这个差值的来历（F4），库上条数以直查为准。

### 6.3 钉 9 的三条形态

- **`/dbnums` 行**：不带参数是文件骨架（`summary:true`，只有 `dbnum/db_type/file_*`）；`GET /api/v1/dbnums?project=AvevaMarineSample&mdb=ALL`
  才是登记表 ∪ 扫描（46 行）。其中 20 行带 `write_state`，**全部 `status=finished`**；整份回执里 `room_sesno` / `room_settled` /
  `room_input_version` / `room_result_version` / `room_required_input` **0 处命中**；1112 / 7997 / 8000 三行 `applied_sesno == model_sesno`
  （729 / 184 / 607）、`model_verdict=in_sync`、`model_caught_up_roots` 0 / 6772 / 953（1112 那个 0 = 它是段 ① 生成、段 ③ 进程没再碰）、
  `cached_pe_rows` 30942 / 170085 / 6639。留档 `D:\adr078-verify\live-rocks-dbnums-full.json`。
- **stdout 无房间段耗时行**：三份日志按 `房间阶段|房间段|阶段=rooms|phase=rooms|room_sesno|房间收敛` 搜 → **0 命中**；含「房间」二字的只有
  `[房间初始化]` 常驻横幅、`房间关键字为`、`房间计算采用逐 dbnum 实时内存模式` 与模型 lane 那行的 `房间边=` 计数。
- **`e3d.room.lookup` 非空**（`POST /api/v1/query`，留档 `live-rocks-room-lookup.txt`）：

```
17496/72903  (1112 FLOOR) → memberships 4：R144(panel 24381_1445, room 24381_1443) / R320 / R101 / R201   published_version=729 stale=false publication_status=current
17496/100011 (1112)       → 6：R144 / R320 / R413 / R323 / R101 / R201
17496/105823 (1112)       → 3：R212 / R112 / R144
24381/105161 (7997 ATTA)  → 2：R232 / R332                                                             published_version=184
24381/72538  (7997 SCTN)  → 3：R332 / R231 / R330
24384/23030  (8000 ELBO)  → 3：R231 / R333 / R330                                                       published_version=607
```

  首条即 `fn::room_relate_of` 那条（`inside_count DESC, center_dist ASC, room_num ASC`）：17496_72903 在库上 4 条边按此序为 R144(8, 2960) →
  R320(8, 15663) → R101(6, 2656) → R201(3, 3225)，与响应逐条同序；`room_refno` 由常驻模型的面板→房间映射填出（24381_1445 → FRMW 24381_1443）。
  三库的响应 `calculation_status` 全是 **`not_computed`**——见 F2，边与版本三元组都是对的，是判据字段在 durable 形态下够不着 ACK。

### 6.4 材料表函数（8049 `/sql` 直查，留档 `live-rocks-material-fns.json`）

```
fn::room_relate_of(pe:17496_72903)  → room_relate:24381_1445_17496_72903 {in: pe:24381_1445, room_num: 'R144', inside_count: 8, center_dist: 2960.361}
fn::room_num_of(pe:17496_72903)     → "R144"        fn::get_room_number(pe:17496_72903) → "R144"
fn::room_num_of(pe:17496_105823 / 24381_105161 / 24381_72538 / 24384_23030) → "R212" / "R232" / "R332" / "R231"
fn::room_code(pe:17496_72903 / 17496_105823 / 24381_105161 / 24384_23030)   → NONE / NONE / NONE / NONE
pe:24381_1445 (PANE).owner → pe:24381_1444 (SBFR) name "/1RX-RM01-R144-VOLU"
```

`fn::room_num_of` / `fn::get_room_number`（材料表的一条取号路）对生成过面板的房间**回得出房间号**；`fn::room_code`（另一条）**恒回 NONE**——
钉 9 最后那一句在这台上没过，原因是函数自己的（F1）。用户拍板修 F1，修后复验见 §6.7。

### 6.5 本轮发现（未改代码，逐条待拍）

- **F1 `fn::room_code`（hd 版，`resource/surreal/fn_query_room_code.surql`）恒回 NONE。** 函数体写的是
  `let $code = if (string::len($res.room_num) == 4) { $code = $res.room_num; } else if … { … } else { $code = none; };`，
  SurrealDB 2.1 把它存成 `LET $code = IF (…) { LET $code = $res.room_num; } …`——**块的值是块内最后一条语句的值，`LET` 的值是 NONE**
  （库上实测：`RETURN (IF true { LET $x = 5; })` → NONE，`RETURN (IF true { 5 })` → 5），于是 `$code == NONE → RETURN NONE`。
  开头那段 `let $id = return if …` 同一毛病（结果没人用，不致命）。同一函数体改成表达式形式在 8049 上直接跑：
  `LET $code = IF (string::len($res.room_num) == 4) { $res.room_num } ELSE IF (string::len($res.room_num) == 5) { string::concat(string::slice($res.room_num, 0, 1), string::slice($res.room_num, 2)) } ELSE { NONE }; RETURN string::concat($prefix, '-', $code);`
  → **`"1RX-R144"`**。另有一条运维形态：段 ① / ② 只摄入了 1112、面板所在的 7997 没有 `pe` 行时，`fn::room_code` 不是回 NONE 而是**报错**
  `string::slice() Argument 1 … Expected a string but found NONE`（`$res.in.owner.name` 取不到）——`watch_dbnums` 圈掉面板库的部署，材料表
  取房间码会整体失败。hh 版（`fn_query_room_code_hh.surql`）走 `fn::room_num_of`，不受 F1 影响。第一轮没改（它是 ADR-073 D4 说「一律不动」的
  库层函数，且内置快照是编译期灌入，改文件要重编才生效）；**用户随即拍板修，修法与复验见 §6.7。**
- **F2 durable 形态下 `e3d.room.lookup.calculation_status` 恒为 `not_computed`。** 判据（spec §4.7.2）是当前根的 `RootPublicationAck`
  在进程内存里且 `room_generation` 匹配；rocksdb + `incremental-overlay` 的模型 lane 直写、不留内存投影（`/health.model_cache.resident_roots=0`），
  刚生成完的 7997 / 8000 根也没有 ACK，冷重启后更没有——`ready_empty` / `ready_nonempty` 在生产存储形态下**不可达**，客户端分不出
  「算过且不属任何房间」与「没算」。`memberships` / `published_version` / `stale` / `publication_status` 本身都对。要一个 durable 侧的就绪判据
  （`model_sesno` / 根的库上凭证）替代或补充内存 ACK，属设计题。**用户随即拍板补，修法与复验见 §6.8（ADR-073 追记 D14）。**
- **F3 `room_panel_relate` 在新库上不物化。** 8049 上根本没有这张表；源码里唯一的写者 `room_model::build_room_panels_relate` 无人调用
  （S5 死代码范围），逐根路径只写 `room_relate`，房间→面板映射活在常驻模型内存里（`room.lookup.room_refno` 就从那儿填）。
  CONTEXT.md「两类边」与 ADR-073 钉 7 / 钉 8 仍把它当作会被写的表——文档与钉子跟实现有漂移，随 S5 一并定。
- **F4 日志 `房间边=` 计数 ≠ 库上条数**（§6.2）。8000 相等、1112 恰为 2 倍、7997 介于其间；未追。

### 6.6 收尾

段 ③ 的 throwaway 服务已停（9199 释放、实例锁清掉、会话 env 清空）。**8049 的 surreal 仍在跑**、数据在 `D:\adr078-verify\live-rocks`
（进程持有文件句柄期间资源管理器里文件大小显示滞后，直查条数以 §6.2 为准），随时可用 `Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8049/sql`
复核；不要了就 `Stop-Process -Id 65988`。

### 6.7 修 F1：`fn::room_code` 分支改成表达式形式，重编后在同一座库上复验

- 改动（仅两个文件）：
  - `resource/surreal/fn_query_room_code.surql`：`$id` 与 `$code` 两处 `if` 块，各分支从 `{ $x = …; }` 改成只放一个表达式
    （`{ $res.room_num }` / `{ string::concat(…) }` / `{ none }`），`let $id = return if …` 那个 `return` 一并去掉；其余（`$noun` 取法、
    `fn::room_relate_of($pe)`、`$prefix` 取面板属主名首段、四字 / 五字房间号两种写法、`string::concat($prefix, '-', $code)`）一字不动。
    `$id` 仍然算完不用——与 `e43c1d0fd` 首版一致，这轮不改语义。
  - `src/data_interface/embedded_surql.rs` 新增 mem:// 钉 `hd_room_code_returns_prefix_and_room_number_for_a_member_with_an_edge`
    （`project_hd`）：SBFR `/1RX-RM01-R144-VOLU` → PANE（`owner`）→ `room_relate` → FLOOR 成员，四字 `R144` 与五字 `R0144` 都回 `1RX-R144`，
    没有边的成员回 NONE。修前这条会红（§6.4 同形状在 8049 上实测 NONE）。
- 先在 8049 上把新函数体整段 `/sql` 跑一遍（同一段文本）：`[fn::room_code(pe:17496_72903), …105823, …24381_105161, …24384_23030]` →
  `["1RX-R144","1RX-R212","1RX-R232","1RX-R231"]`；BRAN `24384_23028`、PANE `24381_1445` 自身没有边 → NONE（与修前语义一致）。
- 重编 `cargo build --bin aios-database`（42s，HEAD 此时已是 `1dbedf8c3`，二进制自报 `0.1.27+g1dbedf8c347a.1789552335.dirty`）。
  为证明是**内置快照**把函数抬上去的，先在 8049 上把旧函数体（`git show HEAD:…`）灌回去并确认 `fn::room_code(pe:17496_72903)` 又是 NONE，
  再用新二进制起 `serve`（同 `live-run-rocks` 配置、watch=[1112,7997,8000]）：启动 12.97s，`已灌入内置 surreal 函数集（7 个文件…）`，
  `模型水位核对完成：核对 3 库，落后 0 库`，收敛后 `/health status=ok / room=available / degraded=[]`、`dbnum_consistency judged=3 in_sync=3`；
  `INFO FOR DB` 里 `fn::room_code` 的存储体已是 `LET $code = IF (…) { $res.room_num } ELSE IF (…) { string::concat(…) } ELSE { NONE };`，
  `fn::room_code(pe:17496_72903 / 17496_105823 / 24381_105161 / 24384_23030)` → **`1RX-R144` / `1RX-R212` / `1RX-R232` / `1RX-R231`**
  （留档 `D:\adr078-verify\live-rocks-room-code-after-fix.json`）；`e3d.room.lookup 17496/72903` 仍是同样 4 条（F2 的 `not_computed` 未动）。
- 测试：`cargo test --lib -- embedded_surql mem_startup_define_replay_applies_and_hd_room_code_wins` 3 passed（含既有的 hd/hh 覆盖序钉与快照镜像钉）；
  全量 `cargo test --lib` **2152 passed / 0 failed / 111 ignored**（9.49s）。第一遍全量曾 15 红，全是
  `configuration file "" not found`——是我这个 shell 里 `DB_OPTION_FILE` 被设成了空串（PowerShell 赋空串不删变量），清掉后全绿，与代码无关。
- 钉 9 至此四句全落实。F2 / F3 / F4 仍开着。新二进制那台 throwaway 服务也已停，8049 库照旧留着。

### 6.8 修 F2：durable 侧就绪判据 `model_room_generation`（ADR-073 D14），同一座库上冷重启复验

判据此前只有一层——进程内该根的 `RootPublicationAck`。它按 ADR-079 不持久，而 rocksdb 形态下库是上一次运行灌的：
`/health.model_cache.resident_roots=0`，一条 ACK 都没有，于是 `ready_*` 不可达。补的是第二层：水位行上一格
`dbnum_watermark.model_room_generation` = 「这个库的房间边整库对齐到哪一代常驻模型」，取值与根 ACK 的 `room_generation`
同一个字串（`ResidentRoomModel::input_version`）。

- **改动**（五个文件）：`dbnum_state.rs` 加列与读出；`task_end.rs` 的 `RoomGenerationStamp{Untouched|Cleared|Aligned}`
  与 `render_watermark_update`（与 `model_sesno` 同在 `all_roots_ok` 门内）；`batch_worker.rs` 的
  `task_room_generation_stamp` / `room_generation_stamp_of` / `whole_library_model_only`（落款与房间代号共用同一句资格判定）；
  `dbnum_initialize.rs` 的 `ModelHalfReport.room_generation`（初始化生成的是每一个根，说得了整库那句话）；
  `query_service.rs` 的 `room_edges_ready` + `QueryService::library_room_generation`（只在 `store_mode` 落盘时启用第二层，
  并按 ADR-079 让进程账本里还欠着的根一票否决）。
- **段 ①（复现）**：新二进制 `serve`（同 `live-run-rocks` 配置、watch=[1112,7997,8000]，启动 12.3s），`17496/72903` 回 4 条边、
  `published_version=729 / stale=false / publication_status=current`，`calculation_status` = **`not_computed`**——库是旧二进制灌的，
  那一格还是 `null`（`SELECT model_room_generation FROM dbnum_watermark WHERE dbnum IN [1112,7997,8000]` 三行全 `null`）。
- **段 ②（盖章）**：`POST /api/v1/dbnums/1112/model/rebuild` → `task_id=db-20260916-202553-000002`，
  `detail.intent=model_only` / `model_start_sesno=0` / 18 根全 `generated` / 38.0s `succeeded`、`model_verdict=in_sync`。
  库上 `dbnum_watermark:1112.model_room_generation = efd4ebe1e71b89db7d5cadf4d55dd06a89f9e5244dff27c6fc230325b46ad269`；
  7997 / 8000 那一格仍 `null`（没跑过整库对齐，判据如实说不知道）。1112 的 `room_relate` 重算后仍是 **6171** 条、全库 **70386** 条，
  与 §6.2 逐字相同（整库重算 == 从零初始化，D13 那条口径在真库上再走一遍）。
- **段 ③（冷重启复验，判据的正题）**：停服重起（12.33s，`模型水位核对完成：核对 3 库，落后 0 库`，
  `/health.features.room` `status=available / executor=durable / generation=efd4ebe1e71b…`，`model_cache.resident_roots=0`
  = 进程内一条 ACK 都没有）：

  | refno | 库 | memberships | calculation_status |
  | --- | --- | --- | --- |
  | `17496/72903` | 1112 | 4（R144 / R320 / R101 / R201） | **`ready_nonempty`** |
  | `17496/100011` | 1112 | 6 | **`ready_nonempty`** |
  | `17496/105823` | 1112 | 3 | **`ready_nonempty`** |
  | `17496/121765`、`17496/121728` | 1112 | 0（库上确无边） | **`ready_empty`** |
  | `24381/105161` | 7997 | 2 | `not_computed`（那一格 `null`，尚未整库对齐） |
  | `24384/23030` | 8000 | 3 | `not_computed`（同上） |

  `/health.features.room.generation` 与库上那一格逐字相同——两层判据共用一个代号这件事在真库上对上了；
  `dbnum_consistency judged=3 / in_sync=3 / lagging=0`。留档：`D:\adr078-verify\live-rocks-d14-health.json`、
  `live-rocks-d14-*.out.log`、`live-rocks-d14-restart-*.out.log`。
- **段 ④（三库都点亮 + 再冷重启一次）**：`POST /dbnums/8000/model/rebuild` → 953/953 根、46.5s succeeded；
  `POST /dbnums/7997/model/rebuild` → 6772/6772 根、570.7s succeeded（两条跨库并行入队、同库串行，与 ADR-065 D5 一致）。
  三库水位行的 `model_room_generation` 现在都是同一个 `efd4ebe1…ad269`；**边数逐库与首次从零初始化（§6.2）一字不差**——
  1112 **6171** / 7997 **59844** / 8000 **4371**、合计 **70386**，即 ADR-010 §9「整库重算 == 从零初始化，逐边相等」在三个真库上
  又走了一遍。再冷重启（`resident_roots=0`、`dbnum_consistency judged=3 / in_sync=3`）后八个抽样：

  | refno | 库 | memberships（房间号） | calculation_status |
  | --- | --- | --- | --- |
  | `17496/72903` | 1112 | 4（R144 / R320 / R101 / R201） | **`ready_nonempty`** |
  | `17496/100011` | 1112 | 6（R144 / R320 / R413 / R323 / R101 / R201） | **`ready_nonempty`** |
  | `17496/105823` | 1112 | 3（R212 / R112 / R144） | **`ready_nonempty`** |
  | `17496/121765` | 1112 | 0 | **`ready_empty`** |
  | `24381/105161` | 7997 | 2（R232 / R332） | **`ready_nonempty`** |
  | `24381/72538` | 7997 | 3（R332 / R231 / R330） | **`ready_nonempty`** |
  | `24381/156157` | 7997 | 2（R231 / R331） | **`ready_nonempty`** |
  | `24384/23030` | 8000 | 3（R231 / R333 / R330） | **`ready_nonempty`** |

  房间号与 §6.3 那一轮（ACK 还在、`calculation_status` 恒 `not_computed` 的那次）逐条相同——变的只有判据那一格。
  留档 `D:\adr078-verify\live-rocks-d14-all3-health.json`、`live-rocks-d14-all3-*.out.log`。
- **段 ⑤（`/dbnums` 那一格）**：`GET /dbnums?project=AvevaMarineSample&mdb=ALL` 46 行，其中 29 个 DESI 行各带一格
  `room_alignment`，非 DESI 行整格缺失（0 个）：1112 / 7997 / 8000 三行
  `current=true`、`generation == resident_generation == efd4ebe1e71b…`；其余 26 个 DESI（这座 throwaway 库从没摄入过它们）
  `generation=null / current=false`——正是「这个库此刻只能回 `not_computed`」的解释。同一轮 `e3d.room.lookup 24381/105161`
  仍回 `ready_nonempty`（R232 / R332）。留档 `D:\adr078-verify\live-rocks-d14-dbnums.json`。
- **测试**：`cargo test --lib` **2157 passed / 0 failed / 111 ignored**。新增 5 条钉：
  `the_room_generation_stamp_follows_the_model_watermark`（收尾三档落地 + 模型没成不写）、
  `only_a_task_that_speaks_for_the_whole_library_stamps_the_durable_room_generation`、
  `the_whole_library_eligibility_is_decided_in_one_place`、
  `the_durable_alignment_answers_for_roots_this_process_never_published`、
  `the_room_status_reads_both_the_process_ack_and_the_durable_alignment`。
- **边界**：一个库要回 `ready_*`，得有一次能替整库说话的任务给它盖上那一格（初始化 / `model/rebuild` / 跑过整库房间重算的窗口）
  ——普通增量窗口按 D14 不盖，这是有意的；`/dbnums` 没把这一格摊给前端，要看现状直查水位行。F3 / F4 与 §未决 U3 仍开着。
- **现场**：段 ②/③/④ 的 throwaway 服务都已停（9199 释放、实例锁清掉、会话 env 清空）；8049 的 surreal 照旧在跑（pid 65988）。
