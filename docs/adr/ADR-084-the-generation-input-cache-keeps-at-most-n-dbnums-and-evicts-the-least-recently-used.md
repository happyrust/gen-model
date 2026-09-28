# ADR-084：生成输入缓存按**库数**设上限——每库一份 `GenerationInput`，超过上限按「最近一次生成用到」的先后逐库整份出；默认 0 = 不限，默认值待真机量过每窗重开的代价再定

- 状态：**已接受、已实施并完成本地验收（能力）；默认值未定，实机验收延期**（2026-09-22 用户拍板 `docs/plans/2026-09-22-incremental-memory-control-dev-plan.md` D2「按库数 LRU」/ D3「先 0 不限落地，真机量每窗重开代价后定」；
  同日落地——`options::model_generation_input_max_dbnums`、`e3d_model_service::DbnumLru`、`/health.model_cache.element_cache` 三格、`DbOption.toml` 一键；实施记录见文末源码钉）。
- 日期：2026-09-22
- 关联：
  - **承接** ADR-082 §不做「不在本文让稳态窗口收尾释放 `GenerationInput`……要做单开决策卡，先量每窗重开的代价」——本文就是那张决策卡；
  - **沿用** ADR-082 D4 / D5（元素缓存按元素数、满则清；三层裁决 `effective_count_limit`，任何一层写 0 = 不限）与 D6（按数量不按字节）；
  - **不动** ADR-082 D1–D3（overlay 那道闸与冷根读路）、ADR-041（内存批切片：生成中的峰值）——本文管的是「库数 ×」这一维的常驻；
  - **取代** 分支 `codex/adr084-cache-followups`（`958155a04`，2026-09-21，自 `gen-model-refactor@0b2bf527b` 分出、未并入）上的同号草案
    「ADR-082 的补遗——历史快照按数量 + 时限收、生成输入缓存按库数收、ACK 之后摘掉回执上的房间边」：2026-09-22 用户拍 **以本支为准、那条分支作废**。
    对照：它的 S2（`model_generation_input_max_dbnums`，同名同义）= 本文；S1（`model_historical_max_snapshots` 默认 4 + `model_historical_ttl_secs` 1800）
    → 本支 P4 按 D4 只做条数、键名 `model_historical_snapshots_max`、默认 16、**不做 TTL**；S0（`/health` 观测）→ 本支 P3 / P4 的 `/health.model_cache` 三块；
    它没落地的 D3「ACK 之后摘掉回执上的房间边」→ 本支 **P7**（见下 D5）；它补的 `scripts/templates/DbOption.factory.toml` 四键 → 本支同日补上并钉住与代码默认一致。
  - 词汇：CONTEXT「生成输入缓存库数上限」。

## 背景

ADR-082 给了两道按数量的闸：overlay 按根数、每份 `DbSet` 的元素缓存按元素数。后者管住了**单份**的大小（≤ 200,000 条 × 3.6–4.7 KB ≈ 0.7–0.95 GB），
管不住**份数**：`GENERATION_INPUT_CACHE` 按 `(dbnum, sesno, identity)` 键，每个 DESI 库一份 `GenerationInput` = 一份 `DbSet`（MDB 里**所有** DESI 文件都 `add_db` 进来，
CATA 经解析器惰性开）+ 它的池级元素缓存 + 一份 `SourceIndex`；插入时只 `retain` 掉**同库**旧条（换 sesno = 换时点，旧条不能再喂），`forget_generation_input`
只有初始化收尾（`dbnum_initialize.rs`）与常驻房间换代（`room_resident.rs`）两处在调。长驻进程碰过 N 个 DESI 库就留 N 份，CATA 链上的元素在每份里各缓存一遍——
2026-09-22 审核（fable-5-1-27）把它列为仍无界的三处常驻里**最大的一项**（N × ~1 GB）。

ADR-082 当时没做，理由是「稳态窗口收尾释放」会改今天的默认行为（每窗重开 MDB、元素缓存重新变冷），要先量每窗重开的代价。真机 W1–W6 那组窗都是同库连续窗，
没有「重开」发生，量不到（证据档 2026-09-21 §没量到）。

## 决策

### D1　按**库数**设上限，逐出单位是一个库的整份 `GenerationInput`

- 键 `model_generation_input_max_dbnums`（`DbOption.toml`），环境变量 `AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS` 压过；三层裁决同 ADR-082 D5（`effective_count_limit`），**任何一层写 0 = 不限**。
- 数的是缓存里**不同 dbnum 的个数**（每库本来就最多一条）。为什么不按字节：单份的大小已由 ADR-082 D4 管住、量级一致（一份 ≈ 一个数），「库数」是现场能对着 `/health` 的
  `databases[]` 直接数出来的量；按字节要每次插入走一遍元素缓存的估算——那是 `/health` 观测频率上才付得起的账（ADR-082 D5）。

### D2　序 = 最近一次**生成用到**，命中也刷新

- 每条记一个进程内单调序号（`DbnumLru::next_tick`），`generation_input()` **命中时刷新**、装入时置新。与 overlay 那道「读不刷新」（ADR-082 D1）**相反**——那边刷新会让
  「常读的旧根」挤掉「刚生成的新根」；这边刷新保护的正是「最近在生成的库」，而且一窗内同库只刷一次，成本可忽略。
- 装入时先替掉同库旧条（不计逐出），再 `while 库数 > 上限` 把最久没用的**别的**库整份出——刚装入的那份永远留下（上限 1 就是「只留当前库」）。
- 被替 / 被逐的值**交还调用方、在缓存锁外 drop**：`DbSet` 的析构要关文件、放元素缓存，不该在 `GENERATION_INPUT_CACHE` 的锁里做。手里还握着 `Arc<DbSet>` 的切片不受影响，
  用完自然释放；该库下一窗 `generation_input` 从文件重开（`scan_index` + `build_set`），元素缓存从零变热。
- `forget_generation_input`（初始化收尾 / 常驻房间换代）照旧整库拿走，**不计入**按上限的逐出数。

### D3　默认 **0 = 不限**；默认值待真机

- 落地这一刻行为一个字不变（与 ADR-082 D5 落地那一刻同一条路：先只加能力，先量再配）。
- 2026-09-28 本地验收已完成：`model_memory_store`、`e3d_model_service`、`historical_model` 相关测试通过，代码默认仍为 `0`。d149live 当前无运行进程，本轮不启动外部服务；因此未产生两库交替窗口证据，也不提前把默认值改成候选值。
- **量法**（计划 §3.4 / §5）：d149live 私有实例（rocksdb，`D:\d149live`），**两个 DESI 库交替出窗**、`AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS=1`，每窗记任务耗时、
  `/health.core.source_reads.db_opens` 增量、RSS 前后峰、`element_cache.databases[].parsed_entries` 从 0 起的爬升；对照不限时同一组窗。最接近的既有数是 W5 / W6 里
  `RoomOnly` 枚举根集那一步（RSS +90 MB、解析 70,084 元素、十几秒），但它复用了本窗的生成输入、不是从零开池。数进 `docs/evidence/2026-09-2x-generation-input-reopen-cost.md`，
  据此把 `MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT` 从 0 改成一个数（候选 **2–4**：一个长驻实例通常同时活跃的 DESI 库数），本节追记。
- 上限 ≥ 活跃库数时与不设**逐位相同**（不会有逐出发生）——默认值定下来之后样例现场（同时活跃 1–2 库）不会被它碰到。

### D4　可观测

- `/health.model_cache.element_cache` 多三格：`max_dbnums`（0 = 不限）、`resident_dbnums`、`evicted_dbnums_total`（按上限逐出的份数；替换与 `forget` 不计）；
  `databases[]` 每条多 `last_used_tick`（进程内单调序号，越大越新——对着看谁下一个出）。查时现算、不落库（ADR-082 D5 口径）。
- 逐出时打一行 `dbnum=… 生成输入缓存按库数上限收回：放掉 N 份，现留 M 库，上限 K`；上限为 0 时不打。

### D5　（P7，承作废分支的 D3）ACK 记下之后摘掉回执上随根携带的房间边

- `RootReceipt::room`（`CarriedRoomEdges`，ADR-073 D1 B/T3）在 bake 时算好挂上回执，**只在** flush 的 `room_stage` 里读一次（`carried_edges_for`）；
  ACK 记下 = 那些边已经在库里，回执上这份从此没有读者——一根几百构件的边是几十 KB 字串，默认 50,000 根常驻时是 overlay 里最大的一块**隐形**常驻
  （此前不在 `approx_bytes` 的账里，D5 的 13.8 KB / 根量的是 RSS 差、含它）。
- `ModelMemoryStore::record_publication_ack` 收下一枚与回执相符的 ACK 时 `receipt.room = None`（任一介质；`ack.matches` 不看这一格，`root_needs_persist` 不变）。
  再发布一版带新的边来；刚体前移 / 单元级发布的回执本来就是 `None`（flush 现算，Q5 兜底）。没 ACK 的根（write-behind 积压 / `Blocked` 重试）照旧带着——重试还要用。
- 观测：`OverlayFootprint.receipt_room_bytes`（也计入 `approx_bytes`），`/health.model_cache.overlay.receipt_room_bytes`——长期不落回 0 = write-behind 积压。
- 钉 `a_matching_ack_drops_the_carried_room_edges_from_the_receipt`（对不上的 ACK 不收、边不动；对上了边摘掉、字节归零、回执别的格不动）。

## 不变量

1. 上限为 0、或上限 ≥ 出现过的库数时，缓存的内容与命中序列与 2026-09-22 之前逐位相同（只多了序号）。
2. 刚装入的那份永远不被这一次装入逐出。
3. 逐出只换时间不换产物：被逐库的下一次生成从文件重开，`READBACK` 两态逐字相同（缓存只省 open + parse，ADR-082 不变量 6）。
4. 被替 / 被逐的 `GenerationInput` 在缓存锁外析构；握着 `Arc` 的调用方用完为止。

## 后果

- 长驻进程的「库数 ×」这一维有了旋钮；默认不动。
- 设了之后活跃库数超过上限的现场：每次换库多一次重开（代价 = D3 要量的那个数），元素缓存的 CATA 链在每份里各暖一遍的成本随之重付。
- `/health` 多三格一列。

## 实施约束（源码钉）

1. `options.rs`：`model_generation_input_max_dbnums` / `MODEL_GENERATION_INPUT_MAX_DBNUMS_ENV` / `MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT = 0`（`ExtFields` 两处 + 默认 + 映射）；
   钉 `a_count_limit_takes_a_valid_env_override_falls_back_to_config_then_to_the_default` 扩到这一键。
2. `e3d_model_service.rs`：`DbnumLru<V>`（`get_touch` / `insert(key, value, max_dbnums) -> Vec<V>` / `forget_dbnum` / `dbnum_count` / `iter`）、
   `GENERATION_INPUT_CACHE: OnceLock<Mutex<DbnumLru<GenerationInput>>>`、`generation_input()` 命中刷新 + 装入后锁外 drop + 日志、
   `GenerationInputCacheLimits` / `generation_input_cache_limits()`、`GenerationInputFootprint.last_used_tick`、`forget_generation_input` 改走 `forget_dbnum`；
   钉 `the_generation_input_cache_keeps_at_most_n_dbnums_and_evicts_the_least_recently_used`（泛型上跑：不限 / 同库替换不计 / 命中刷新 / 上限 3→1 / forget 不计）。
3. `handlers.rs` `model_cache_health`：`element_cache.{max_dbnums, resident_dbnums, evicted_dbnums_total}`；`overlay.receipt_room_bytes`（D5）。
4. `DbOption.toml`：`model_generation_input_max_dbnums = 0` 与注释；`scripts/templates/DbOption.factory.toml` 四键（`options::tests::factory_template_room_membership_matches_code_default`
   钉住与代码默认一致）；CONTEXT「生成输入缓存库数上限」；changelog 2026-09-22。
5. D5：`model_memory_store.rs` `record_publication_ack` 摘 `receipt.room`、`approx_carried_room_bytes`、`OverlayFootprint.receipt_room_bytes`；
   钉 `a_matching_ack_drops_the_carried_room_edges_from_the_receipt`。

## 不做 / 未决

- **不做**每窗收尾无条件释放（ADR-082 §不做 的理由不变；本文的上限在「上限 ≥ 活跃库数」时覆盖它的目的、不改默认行为）。
- **不做** TTL、不做按字节。
- **未决**：默认值（D3）——等真机那组两库交替窗。
- 不在本文：元素缓存「满则清」的形状（计划 P5，D5 拍「先不动」）。
