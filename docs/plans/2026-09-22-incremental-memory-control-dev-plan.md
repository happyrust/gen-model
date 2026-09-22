# 增量更新的内存控制——把 ADR-082 的「两道按数量的闸」补成「每一块常驻都有界、逐出不改语义、写锁不被逐出拖住」（开发计划）

> 来源：用户 2026-09-22 11:09（fable-5-1-27）「使用 plannator 制定内存控制的方案」。前一步是同日 10:58 的审核
> （静态审阅 ADR-082 / 证据档 2026-09-21 / `model_memory_store.rs` / `model_write_behind.rs` / `options.rs` / `e3d_model_service.rs` /
> vendor `db_element.rs` / `room_recompute.rs` / `query_service.rs` / `model_dbnum_ensure.rs`），结论按严重度排 A–F 七项；其中 **A（逐出打破
> F3 前提）已按方案 1 落在工作树**（未提交，`model_memory_store.rs` +156/−5、`task_end.rs` +16，`model_memory_store::tests::` 37 绿）。
> 本计划把 A 之外的六项与 ADR-082 §不做 里推给「下一张决策卡」的 `GenerationInput` 一项收成一份可分段验收的方案。
> 状态：**已批准**（2026-09-22 15:0x，Plannotator `--gate` + 面板确认「已在 Plannotator 里批准，按 D1–D8 的推荐项落地，先做 P0」；§4 八条全按推荐项定：
> D1 删树、D2 按库数 LRU、D3 先 0 落地再量、D4 Historical 16 条、D5 元素缓存先不动、D6 缺失证据档改引用、D7 钉、D8 P0→P1→P2→P3→P4→P6）。
> **P0 已落地**（同日，见 changelog 2026-09-22；`enforce_root_limit(limit, room_membership_enabled)`、`ack_owes_room_edges`、ADR-082 D1/D2/实施约束改口、CONTEXT）。
> 代码事实对到本仓工作树 2026-09-22 14:3x（主线 `579b1d8b2` + 上述未提交改动）；行号以那一刻为准，落地后以源码为准。
> 关联：ADR-082（两道闸、D5 默认值、§不做）、ADR-073 D13 / F3（`RoomOnly` 冷根、stray 代号）、ADR-041 / ADR-052（瞬时峰值，不在本计划）、
> `docs/evidence/2026-09-21-d149-live-chain.md`（W1–W6 真机链）。

## 0. 一句话

**ADR-082 立住的策略不改**（按数量、只逐已落盘的根、满则清、0 = 不限、只在 durable 介质下动）；本计划做的是把它**落地里的四类欠账**补齐：
① 一处正确性缺口（已修，待入库与文档跟上）；② 两处写锁热点（`drop_root` 共有判定 O(回执数)、每次提交 / 逐出整棵重建的死树 `room_spatial`）；
③ 三处仍无界的常驻（`GenerationInput × 库数`、`Historical(*)` 作用域、`evict_missing_root` 留下的孤儿 ACK）；④ 文档 / 观测欠账。
每一段都可以单独提交、单独验收；`GenerationInput` 那一段沿 ADR-082 D5 的路径（先 0 不限落地 → 真机量 → 定默认值）。

## 1. 现状盘点：常驻内存的每一块与它现在的界

| 块 | 位置 | 现在的界 | 大小口径 | 状态 |
| --- | --- | --- | --- | --- |
| 模型投影 overlay（`Current`） | `model_memory_store.rs` `ScopeState`（L340–375）、`evict_acked_roots_over` L688、`enforce_root_limit` L1304 | `model_overlay_max_roots` 默认 50,000（`options.rs:1048`），逐出候选 = durable ACK 与回执逐字相符 **且不是 F3 证据**（工作树新加） | ≈ 13.8 KB / 根（ADR-082 D5，**不含** `room_spatial`） | 有界；候选过滤已补 A |
| 每库房间空间树 `room_spatial` | `ScopeState::room_spatial{,_dirty,_rebuild_*}` L361–365、`flush_room_spatial` L398–433 | 随 overlay，无自己的界；`try_commit_current` / `evict_acked_roots_over` / `evict_missing_root` / `drop_cached_root` / `clear_dbnum` 每次对脏库 `AccelerationTree::load(该库全部驻留记录)` | 不在 `approx_bytes` 里（`OverlayFootprint` L787 注明） | **死重**：唯一读者 `room_members_intersecting`（L893）全仓只有本文件 8 处测试在调；生产房间候选走 `GLOBAL_AABB_TREE`（CONTEXT L581，`room_model.rs:770`） |
| `publication_acks` / `commit_ticks` 孤儿 | `evict_missing_root` L1153–1173 | 删记录（`remove`）、删回执，**不删** ACK / 序号 / 失效标记 | 每条几十字节 | **无界**，随源上删根数累积；逐出候选按 `receipts` 迭代，永远收不到 |
| 生成用元素缓存（每份 `DbSet`） | vendor `db_element.rs` `store_parsed` L631–651、`clear_element_cache` L486 | `model_element_cache_max_entries` 默认 200,000 / 份，满则整张清 | ≈ 3.6–4.7 KB / 元素（D5） | 有界（单份） |
| `GENERATION_INPUT_CACHE × 库数` | `e3d_model_service.rs` L1098–1148：键 `(dbnum, sesno, identity)`，插入时只 `retain` 掉**同库**旧条 | **按库数无界**；`forget_generation_input`（L1213）只有初始化收尾（`dbnum_initialize.rs:466`）与常驻房间两处在调 | 每份 = 一份 `DbSet`（MDB 全部 DESI 都 `add_db`，L2854）+ 元素缓存 ≤ 200k × 3.6–4.7 KB ≈ 0.7–0.95 GB + `SourceIndex`；CATA 链元素在每份里各缓存一遍 | **无界（最大的一项）**；ADR-082 §不做 推给下一张决策卡 |
| `Historical(*)` 作用域 | `historical_model.rs` L110–135、`POST /model/history`（`handlers.rs:1939`，300 s 超时） | 只有显式 `DELETE` / 同 key 重生成才 `drop_scope`（L1694）；无条数上限、无 TTL | 每格一份完整投影（一根到一库不等） | **无界**；`overlay_footprint` 只数 `Current`，`/health` 看不见 |
| 四张版本表缓存 / `SwapLedger` / `MAX_TASK_SNAPSHOTS` / `cata_closure_cache` / e3d-io `PageCache` / 常驻房间 `MeshCache` | 各处 | 64 / 64 / 32（`model_dbnum_ensure.rs:36`）/ 200k 条 + 256 MiB LRU / 256 页 / `retain(live)` | — | 有界，不动 |
| RSS 自适应控制器 | `data_interface/model_concurrency.rs` | RSS > 1.25× 基线把在飞根数减半；基线随采样单调上抬 | 相对量 | 「只少干、不吐」；不是绝对闸，不动（§3.7） |

## 2. 已落地（本计划的起点，工作树未提交）

**A · 逐出候选排除 F3 证据**（`model_memory_store.rs` / `task_end.rs`，2026-09-22 11:18）：

- `ScopeState::stamped_room_generations: BTreeMap<u32, String>`（库上 `model_room_generation` 最近一次盖成哪一代）+ `ack_is_stray_room_evidence(&ack)`
  （`ack.room_generation ∈ stray_generations_written[dbnum]` 且 ≠ `stamped_room_generations[dbnum]`）；`evict_acked_roots_over` 候选多一条 `&& !self.ack_is_stray_room_evidence(ack)`。
- `ModelMemoryStore::note_room_generation_stamped(dbnum, Option<&str>)`；`task_end::commit_task_end_on` 收尾真盖上之后 `Aligned(g)` → `Some(g)`、`Cleared` → `None`；`clear_dbnum` 连带清。
- 单测 `a_root_whose_ack_carries_an_unstamped_stray_room_generation_is_not_evictable`。验证：`model_memory_store::tests::` 37 绿、`task_end / model_write_behind / room_recompute::tests::` 58 绿、`cargo check --lib --tests --bins` 0 error。
- 为什么这样修而不是单看 stray 集合 / 盖章时清集合：见 fable-5-1-27 11:18 说明——单看集合会把限额废掉（面板一变之后每根 ACK 都是那一代）；盖章清集合会在「并发 stray 刷盘先提交、盖章抹掉库上那一列」的缝里丢证据。判据与 F3 自己的定义（`ProcessAck::Stale` = ACK 代号 ≠ 常驻 / 库上）同一句话。

**还欠**：提交；ADR-082 D1「候选 = durable ACK 与回执逐字相符」改口、D2 表补第五个读者（`room_recompute::request_for_task` / `query_service::room_calculation_status`）；CONTEXT「投影 overlay 根数上限」加这一条例外；changelog 2026-09-22。→ 本计划 **P0**。

## 3. 方案（分段落地，每段可独立提交、独立验收）

### 3.1 P0 · A 入库 + 文档跟上（≈ 0.5 人日）

- 提交工作树那两个文件（`fix(overlay): 逐出候选排除 F3 证据……`）。`Cargo.lock` 里 cargo 顺手改的 vendor e3d-io 0.1.0 → 0.2.0 那一行**单独一笔**入库（与本题无关，谁 build 都会再出现）。
- ADR-082：D1 候选定义加「且 ACK 房间代号不是本进程写过、库上又还没盖到的 stray 代号」；D2 表加第五行「`request_for_task` 的 `unaccounted_stray_generations` 与 `room_calculation_status` 的 `ProcessAck::Stale` —— 靠 ACK 本身，逐出前先看它是不是唯一证据（本补丁）」；实施约束加钉名。
- **D7（待拍板）**：开关开着却 `room_membership == false` 的 ACK（发布时没算房间）也是 `stale_acked_roots` 会点名的一类（L359–372：`None` 也算 stale）。store 不读 options，要钉得由调用方传：`enforce_root_limit(limit, room_membership_enabled: bool)`，`model_write_behind::enforce_overlay_root_limit` 传 `options::room_membership()`。推荐**钉**（同一条不变量：逐出不许让 `stale_acked_roots` 少点一根），随 P0 一起。
- CONTEXT「投影 overlay 根数上限」加一句例外；changelog 2026-09-22 条目。

### 3.2 P1 · 逐出与发布的写锁成本（≈ 1 人日）

**P1a `drop_root` 共有判定**（`model_memory_store.rs:651–656`）：

- 现状 `self.receipts.keys().any(|other| record.ancestors.contains(other))` 对**不共有**的记录（绝大多数）要把全部回执走完：默认 50,000 根时每条记录 ≈ 5 万 × 祖先数次比较；一批 512 根（`ROOTS_PER_FLUSH`，`model_write_behind.rs:30`）的 durable ACK 之后逐 512 根、每根十来条记录 ≈ 10⁹ 量级操作，全在 `ModelMemoryStore` **写锁**里（`/model/records`、发布、房间查询都等着）。真机只压过上限 4，这条路没被量到。
- 改成 `record.ancestors.iter().any(|a| self.receipts.contains_key(a))`：O(祖先数 × log R)。语义逐字相同（同一个「祖先链里有别的常驻回执」谓词，换了迭代方向）。`restore_root`（L1262–1267）同一写法但是 `cfg(test)`，顺手一并改。
- 顺手：`enforce_overlay_root_limit` 那行日志加 `耗时 {ms}ms`（`Instant` 包住 `store.enforce_root_limit`），真机对着看逐出一批要占写锁多久——这是 P1 唯一的观测面，零成本。

**P1b 删死树 `room_spatial`**（**D1 待拍板**：删 / 改懒建 / 留）：

- 删 `ScopeState::{room_spatial, room_spatial_dirty, room_spatial_rebuild_count, room_spatial_rows_rebuilt, room_spatial_rebuild_duration_ms}`、`flush_room_spatial` 及其 7 处调用、`room_members_intersecting`（L893）、`room_spatial_rebuild_stats`（L1019）、`room_spatial_dirty.insert` 两处（L533 / L586）。`by_source` 保留（`room_records_for_members` 在用）。
- 测试：`room_spatial_accepts_large_incremental_pipe_like_cohorts`（L2583，专测这棵树）删；其余 4 条（L2548/2553、L2609/2620、L2658/2662、L2880）把 `room_members_intersecting(dbnum, &[bounds])` 断言改成对 `records(ProjectionScope::Current)` 按 `dbnum` + `world_bounds` 相交过滤的本地 helper——它们钉的是「记录换位 / 逐出 / 换属主之后 overlay 里谁还在、盒在哪」，不是树本身。
- 收益：每次发布 / 逐出少一次 O(该库全部驻留记录) 的 bulk-load（50,000 根 ≈ 40 万条记录时是一次 40 万盒的重建，在写锁里；源码注释自己写着「重建是已知的前台开销热点」）；overlay 真实每根成本回到 `approx_bytes` 能看见的那一份（D5 的 13.8 KB / 根不再低估）。
- 若拍「懒建」：`room_spatial: BTreeMap<u32, OnceCell<AccelerationTree>>`，脏了就 `take`，首次 `room_members_intersecting` 才建——保留了能力但生产仍没人调，只是把死重换成了死码。**推荐删**：真要用再从 `by_source` 起一棵，比留着一棵没人读却每次都重建的树便宜。

### 3.3 P2 · `evict_missing_root` 的孤儿 ACK（≈ 0.25 人日）

- L1168 之后补 `state.publication_acks.remove(&packed)`、`state.commit_ticks.remove(&packed)`、`state.invalidated_roots.remove(&packed)`（与 `drop_root` 前四行同一组；**不**改成调 `drop_root`——这里记录走的是 `remove` 不是 `evict`：源上真没了，是删除、要进房间变化账，语义不能换）。
- 单测 `evicting_a_missing_root_leaves_no_orphan_ack_or_tick`：装一根 + ACK → `evict_missing_root` → `overlay_footprint().acks == roots`、`commit_ticks` 里没有它；再装 `limit` 根、逐出时不会因为孤儿把 `resident_before` 数错。
- 现场症状（修前）：`/health.model_cache.overlay.acks > roots`。

### 3.4 P3 · `GenerationInput` 按库数 LRU 上限（新决策卡 → ADR-084；≈ 1.5 人日 + 一次真机）

ADR-082 §不做 把它推给「下一张决策卡，先量每窗重开的代价」。方向按 d-149「按数量」口径：**按库数**、零簿记、0 = 不限。**D2 / D3 待拍板**。

- 键 `model_generation_input_max_dbnums`（`DbOption.toml`）、环境变量 `AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS`，走 `effective_count_limit`（三层裁决同 ADR-082 D5），**落地默认 0 = 不限**（行为一个字不变），量完再定默认值（候选 2–4：一个长驻实例通常同时活跃的 DESI 库数）。
- 缓存改 `Mutex<HashMap<key, GenerationInput>>` → 同锁下再带一份 `BTreeMap<u64 /*tick*/, dbnum>` 或直接用 `VecDeque<dbnum>` 记最近使用序：`generation_input` **命中也刷新**（与 overlay 那道「读不刷新」相反——这里刷新的是「最近一次生成用过哪个库」，正是我们要保护的热度；一窗内同库只刷一次，成本可忽略）。插入后若不同 `dbnum` 数 > 上限，最久没用的库整份出，**在锁外 drop**（`forget_generation_input` 同一个做法）。
- 手里还握着 `Arc<DbSet>` 的切片不受影响（Arc）；下一次该库的 `generation_input` 从文件重开——代价 = 重开 MDB 全部 DESI 文件 + `scan_index` + 元素缓存重新变冷。最接近的实测数：W5 / W6 里 `RoomOnly` 枚举根集那一步 RSS +90 MB、解析 70,084 元素、十几秒（证据档 §没量到）——但它复用了本窗的生成输入，**不是**从零开池，所以要量。
- 观测：`/health.model_cache.element_cache` 加 `max_dbnums` / `evicted_dbnums_total`；`databases[]` 已逐库列出，加 `last_used_tick`（相对序，不是时刻）。
- **真机量法**（d149live 实例，两个 DESI 库交替出窗，`AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS=1`）：每窗记任务耗时、`db_opens` 增量、RSS 前后峰、`ec_parsed` 从 0 起的爬升；对照不限时同一组窗。数进 `docs/evidence/2026-09-2x-generation-input-reopen-cost.md`，据此定默认值、补 ADR-084 D-默认值那一节。
- **不做**「稳态窗口收尾无条件释放」：ADR-082 已说明它改今天的默认行为（每窗重开、元素缓存每窗变冷）；按库数 LRU 在上限 ≥ 活跃库数时**与今天逐位相同**，只在库数真的超了才动。

### 3.5 P4 · `Historical(*)` 作用域封顶 + 进 `/health`（≈ 0.75 人日；**D4 待拍板**）

- `HistoricalModelStore` 加 `model_historical_snapshots_max`（默认 **16**，0 = 不限；环境变量同套路）：`generate_historical` 落新格之前，快照数 ≥ 上限就按**生成时间**最旧的先 `drop_snapshot`（回执 + `drop_scope` 一起走，与 `DELETE` 同一条路，所以被顶掉的快照之后再查是既有的 `SNAPSHOT_NOT_FOUND`，规格里加一句）。同 key 重生成不算新格。
- 不做 TTL：按条数与 d-149 口径一致、零定时器；要 TTL 另开。
- 观测：`ModelMemoryStore::historical_footprint() -> Vec<(key, records, approx_bytes)>`（走各 `Historical` 作用域的 `record_bytes`，O(格数)）；`/health.model_cache.historical { snapshots, max_snapshots, records, approx_bytes, evicted_total }`。
- 单测：`historical_model::tests::` 加「第 N+1 个 generate 顶掉最旧的、DELETE 之后再 generate 不多算」；`model_memory_store::tests::scopes_are_isolated_and_droppable` 已钉 `drop_scope`。

### 3.6 P5 · 元素缓存「满则清」的形状（低；**D5 待拍板**，默认不做）

- 现状：满了整张清（vendor `clear_element_cache`），最热的 CATA 链条目一起丢，每次清空后目录闭包要重新暖一遍；生产 DESI 库一遍触到的元素若远超 200k（样例最大 177k），每 200k 清一次。
- 候选改法（都在 vendor e3d-io，`DbSet` 不知道谁是 CATA，只能按形状）：(a) **只清最大的那张表**（`cache_of` 按 dbno 分表，DESI 目标库一定是最大的；CATA 各表小而热留下），计数按饱和减；(b) 两代轮换（新旧两张池表，满了丢旧留新，命中先查新再查旧并搬过来）——命中路径多一次查表，与 D4「零簿记」冲突。
- 推荐：**先不动**，等 P3 真机那组窗顺手看 `ec_parsed` 有没有撞过 200k（W1–W6 每窗只装 240 条，样例现场撞不到）；撞到了再做 (a)。
- `store_parsed` 在 `clear_element_cache` 之后别的线程手里旧的 `Arc<RwLock<ElementCache>>` 仍会插入并 `fetch_add`（计数略偏高、下次清归零）——无害，记在 ADR-082 D4 一句即可，不改。

### 3.7 P6 · 文档 / 观测欠账（≈ 0.5 人日，可与任一段捎带）

- **D6 待拍板**：`docs/evidence/2026-09-20-d149-mem-footprint.md` 被 ADR-082 D5（L108）、CONTEXT（L362）、`options.rs` 三处引用，**仓库里不存在、git 历史里也从没有过**。推荐：三处引用改成「原始采样件未入库，数字见 ADR-082 D5」，**不**按 D5 的数字复原一份假证据档；若用户手里还有那份采样（`/health` JSON / RSS 记录），入库即可复原引用。
- `options.rs:1061–1062`：「逐出要 `root_needs_persist == false`」→ 实现只看 `ack.medium.is_durable() && ack.matches(receipt)`（等价，但写的检查并不存在），改口。
- `model_write_behind.rs:36–42`：「撞硬上限回 503」是旧设计残词（ADR-082 背景已标），改成「那时以为会有的硬上限，从来没有；今天的界是 d-149 的按根数逐出」。
- `model_memory_store.rs:834`「**默认无界**」→ 「默认 50,000 根（写 0 才无界）」；L1074「overlay 无界，不存在装不下」→ 「上限由 write-behind 事后收，提交时不做准入」。
- ADR-082 追记里两条与本题相关的开着的欠账**不在本计划做**、只在 §3.8 点名：留在 overlay 里的根被判 `immutable target changed`（便宜路一次没走成 → 「把根留在内存里换便宜路」这层收益目前为零，削弱了大 overlay 的理由——它是正确性 / 时间题，另立）；`RoomOnly` 6,770 冷根串行读库 34 s（时间题，另立）。
- `/dbnums` 面板「逐出 / 上限」那一格（ADR-082 §仍开着）：随 P4 把 `/health.model_cache` 三块（overlay / element_cache / historical）在面板上各给一行，顺手做。

### 3.8 明确不做

- 字节记账 / 全局内存预算 / RSS 绝对闸（ADR-082 D6；要做另开 ADR）。
- overlay 读刷新 LRU（D1「读不刷新」不变）。
- 稳态窗口收尾无条件释放 `GenerationInput`（P3 的按库数上限覆盖它的目的、不改默认行为）。
- 元素缓存 LRU（D4）。
- mem 档行为：overlay 那道仍只在 `store_mode().is_durable()` 下动；元素缓存 / `GenerationInput` / `Historical` 三道两档都作用（它们与介质无关）。
- `RoomOnly` 冷根并行 / 批读、`immutable target changed` 误判——另立（时间 / 正确性题，不是内存题）。
- 跨库成员的 `RoomOnly`（ADR-073 U3）。

## 4. 待拍板（Plannotator 里逐条批注）

- **D1** `room_spatial` 死树：**删**（推荐，§3.2 P1b）/ 改懒建 / 留着不动。
- **D2** `GenerationInput` 上限的形态：**按库数 LRU，命中刷新**（推荐，与 d-149 口径一致、上限 ≥ 活跃库数时与今天逐位相同）/ 每窗收尾无条件释放 / 不做。
- **D3** `GenerationInput` 默认值：**先 0 不限落地，真机量每窗重开代价后定**（推荐，沿 ADR-082 D5 路径）/ 直接定 4（长驻实例常见的活跃 DESI 库数）。
- **D4** `Historical(*)` 封顶：**按条数 16**（推荐）/ TTL / 两者都要。
- **D5** 元素缓存清空形状：**先不动、等 P3 真机看 `ec_parsed` 是否撞 200k**（推荐）/ 只清最大的那张表 / 两代轮换。
- **D6** 缺失的证据档 `2026-09-20-d149-mem-footprint.md`：**改三处引用为「未入库，数字见 ADR-082 D5」**（推荐）/ 按 D5 数字复原一份并标「复原」/ 用户补原始采样件入库。
- **D7** `room_membership` 开着却 `ack.room_membership == false` 的根：**也钉住不逐**（推荐，调用方传开关进 `enforce_root_limit`，随 P0）/ 不钉。
- **D8** 顺序：**P0 → P1 → P2 → P3 → P4 → P6**（推荐；P5 视 P3 真机数再说）/ P3 提前（内存最大的一项先做）。

## 5. 验证口径

- 单测（每段各自）：P0 `a_root_whose_ack_carries_an_unstamped_stray_room_generation_is_not_evictable`（已有）+ D7 若钉 `a_root_published_without_room_membership_while_the_switch_is_on_is_not_evictable`；
  P1a 既有四条逐出单测 + `evicting_a_nested_root_keeps_the_records_its_resident_parent_still_holds` 不变（谓词等价）；P1b 4 条改写后的断言 + `room_spatial_accepts_large_incremental_pipe_like_cohorts` 删；
  P2 `evicting_a_missing_root_leaves_no_orphan_ack_or_tick`；P3 `the_generation_input_cache_keeps_at_most_n_dbnums_and_evicts_the_least_recently_used` + `options` 那条三层裁决用例扩一键；
  P4 `the_historical_store_caps_snapshots_and_drops_the_oldest_on_generate`。
- `cargo test --lib -- model_memory_store::tests:: task_end::tests:: model_write_behind::tests:: room_recompute::tests:: e3d_model_service::tests:: historical_model::tests::` 全绿；`cargo check --lib --tests --bins` 0 error；`cargo clippy --lib` 本仓零新增告警（ADR-079 口径）。
- 真机（d149live，`D:\d149live`，rocksdb，7997）：
  - P1：`AIOS_MODEL_OVERLAY_MAX_ROOTS=3000` 起实例 → Reinitialize（6,772 根进 overlay → 逐到 3,000）→ 日志新加的 `耗时 {ms}ms` 对照修前二进制同一步（预期从数秒级回到毫秒级；同时看 `/health.model_cache.overlay.approx_bytes` 修后没有变化——树本来就不在里面）。
  - P2：源上删一根后 `POST /model/records` 触 `evict_missing_root` → `/health.model_cache.overlay.acks == roots`。
  - P3：两库交替窗 × 上限 1（§3.4 量法），数进证据档。
  - P4：`POST /model/history` 17 次不同 key → `/health.model_cache.historical.snapshots == 16`、最早那个 key 再查 `SNAPSHOT_NOT_FOUND`。
- 不变量复核（ADR-082 §不变量 1–6 不变，加两条）：7. 逐出不会让 `stale_acked_roots` / `room_calculation_status` 少点一根（P0 / D7）；8. `overlay.acks ≤ overlay.roots` 恒成立（P2）。

## 6. 工作量与顺序（估）

| 段 | 内容 | 规模 | 依赖 |
| --- | --- | --- | --- |
| P0 | A 提交 + D7 + ADR-082 / CONTEXT / changelog | 代码 ~30 行 + 文档 | — |
| P1 | P1a 一行 + 日志耗时；P1b 删树 ~120 行删 + 4 条测试改写 | ~0.5 + 0.5 人日 | D1 |
| P2 | 三行 + 一条单测 | ~0.25 人日 | — |
| P3 | 键 + LRU + 锁外 drop + `/health` 三格 + 单测；真机一组窗 + 证据档 + ADR-084 | ~1.5 人日 + 真机半天 | D2 / D3 |
| P4 | 封顶 + `historical_footprint` + `/health` 一块 + 规格一句 + 单测 | ~0.75 人日 | D4 |
| P5 | （可选）vendor 只清最大表 | ~0.5 人日 | D5、P3 的数 |
| P6 | 引用改口 ×3、注释改口 ×3、`/dbnums` 三行 | ~0.5 人日 | D6 |

## 7. 风险

- **R1 P1b 删树**：8 处测试依赖它；改写成对 `records()` 过滤后断言语义不变，但若哪天房间候选要回到「按库、按投影里的盒」查，得从 `by_source` 重新起一棵——CONTEXT 明确生产走 `GLOBAL_AABB_TREE`，这条路今天不存在。
- **R2 P3 逐出正在生成的库**：只有 `Arc` 计数保护、没有「在飞」标记——被逐的库若下一窗紧接着来，重开一次（代价就是要量的那个数），不影响正确性；上限 ≥ 活跃库数时不会发生。
- **R3 P4 顶掉客户端正要查的快照**：与 `DELETE` 同一条路，`SNAPSHOT_NOT_FOUND` 是既有错误码；默认 16 对样例现场（一次分析开几个时点）够用，规格里写清「被顶掉 = 需重新 generate」。
- **R4 P2 改 `evict_missing_root` 的删除语义**：只加 ACK / 序号 / 失效标记三行，记录仍走 `remove`（进房间变化账）——不要顺手换成 `drop_root`。
- **R5 P0 / D7 传开关进 store**：`enforce_root_limit` 签名多一位，`model_write_behind` 与四条既有单测同步改；不让 store 自己读 options（保持它对配置无知）。
- **R6 默认值仍靠 mem 档实测**（D5 的 13.8 KB / 根修 P1b 后反而更准）；rocksdb 档默认 50,000 下逐出的真机数只有上限 4 那一档，P1 真机那一步顺手补一档 3,000。
