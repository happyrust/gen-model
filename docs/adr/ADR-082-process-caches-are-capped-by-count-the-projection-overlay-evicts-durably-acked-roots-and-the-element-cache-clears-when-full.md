# ADR-082：进程内缓存按数量设上限——模型投影 overlay 按根数逐出已落盘的根，生成用元素缓存按元素数满则清；两个上限默认 0 = 不限

- 状态：**已接受、已实施**（2026-09-20 用户拍板，共识 **d-149**：「两道都要：overlay 按根数、元素缓存按元素数；RoomOnly 改成也读 RocksDB 冷根；默认 0 不限」；
  同日落地——`ModelMemoryStore::enforce_root_limit` + `model_write_behind` 两处接线、vendor e3d-io `DbSet::with_element_cache_limit`、
  `recompute_dbnum_room_edges` 的根全集与冷根读库、`DbOption.toml` 两键；实施记录见文末源码钉）
- 日期：2026-09-20
- 关联：
  - **修订** ADR-073 D13（`RoomOnly` 整库房间重算）：根全集不再只是「投影里有回执的根」——从源文件枚举、不在 overlay 里的冷根从库里读几何行再算（本文 D3）；
  - **沿用** ADR-076（增量任务在 RocksDB 有它之后才收尾；新鲜度来自模型投影）与 ADR-079（生成根成败只活在进程里，`model_sesno` 是模型面唯一的持久事实）——本文不给模型面加任何持久记录，逐出只动进程内的缓存态；
  - **沿用** `model_cache_mode = incremental-overlay`（2026-09-16：启动不回填、冷根直读 RocksDB）——本文让「冷根」在进程活着的时候也会出现，走的仍是那条读路；
  - **沿用** ADR-041（内存批切片：生成中的峰值 = 一片 + 一根）与 ADR-052（几何 CPU 域只管 CPU 不管内存）——它们管**瞬时峰值**，本文管**常驻**；
  - **不动** ADR-056 D3 / ADR-066 / ADR-080 的回退语义——单元级发布与刚体前移拿不到 S 端基线本来就回退 regen，逐出只是让这一支更常被走到；
  - 词汇：CONTEXT「投影 overlay 根数上限」「冷根」「元素缓存元素数上限」。

## 背景

2026-09-20 对增量服务的内存做了一次清点（会话 fable-5-1-58，12:03）：**没有全局内存预算**。瞬时峰值有几道局部闸
（切片元素数 `model_memory_slice_elements`、在飞根数 `model_root_inflight_max`、几何 CPU 额度、事务字节 / 行上限），
另有一个按 RSS 降并发的自适应控制器（`model_concurrency`，RSS > 1.25× 基线就把在飞根数减半）——**它只会少干，不会吐**。
常驻那一半里有两块是**无界**的，它们才是长驻进程 RSS 单调上涨的来源：

1. **模型投影 overlay**（`ModelMemoryStore` 的 `Current` 作用域）：源码自己写着「无界：不记字节、不淘汰，一个根进来就一直在」；
   只在 `evict_missing_root`（库里没了）/ `clear_dbnum`（整库重初始化）时整根删。随「进程起来后碰过的不同根数」增长；
   `incremental-overlay` 只保证启动不回填，**ACK 过的热根也不逐出**。`model_write_behind.rs` 头注释里那句「淘汰只挑有同源 ACK 的根…
   撞硬上限回 503」是旧设计的残词，那道闸从来没有。
2. **生成用元素缓存**（`GENERATION_INPUT_CACHE` 里每库一份 `DbSet` 的池级元素缓存，`model_element_cache` 默认开）：vendor
   e3d-io `DbSet::with_element_cache` 是一张裸 `HashMap`，「不淘汰」；同库换 sesno 只替换那一条，稳态窗口跑完不释放。
   官方数据：7997 一轮峰值 768 MB → 1.55 GB 就是它；N 个库 = N 份（CATA 链上的元素各库重复缓存）。

其余常驻结构要么有界（`mdb_memory_store` 元素骨架 34 B/元素、`cata_closure_cache` LRU 双限、四张版本表缓存满则清、
e3d-io `PageCache` 每引擎 256 页、常驻房间模型 `MeshCache` 换代 `retain(live)`），要么很小（`EXIST_MESH_GEO_HASHES`、
生成根账本、`publication_acks`，十万级 ≈ 十几 MB）。

用户要的是**按数量**限（12:06「我现在想根据数量来控制内存的限制」），并在三个问题上拍了板（12:17）：两道闸都要；
`RoomOnly` 改成也读 RocksDB 冷根；默认 0 = 不限。

## 决策

### D1　投影 overlay 按**根数**设上限，只逐出已落盘的根

- 键 `model_overlay_max_roots`（`DbOption.toml`），环境变量 `AIOS_MODEL_OVERLAY_MAX_ROOTS` 压过；**0 = 不限**。
  全局计，不分库：数的是 `Current` 作用域里**有回执**的根。
- **候选**：有回执、且 `publication_acks` 里那枚 ACK **介质是 durable** 且 **与回执逐字相符**（`ack.medium.is_durable() && ack.matches(receipt)`）
  ——库里已经有这一版，逐出不丢任何东西。没落盘的（ACK 缺 / 又提交过一版让 ACK 失效 / 介质是 kv-mem）**永远不在候选里**；
  候选用尽仍在上限之上就停手，只把数字报出来（`OverlayEviction::still_over_limit`）。
- **序**：每根记「最近一次盖回执」的单调序号（`ScopeState::commit_ticks`），最旧的先出。**读不刷新它**——被逐出的根读路直读 RocksDB，
  刷新读序只会让「常读的旧根」把「刚生成的新根」挤出去。
- **逐出 = 整根从缓存态掉**：回执、ACK、失效标记、序号，以及名下**不与别的常驻回执共有**的记录（嵌套根共有的记录归另一个回执继续持有）
  ——与 `drop_cached_root` 同一段代码（`ScopeState::drop_root`）；记录走 `evict` 不走 `remove`（不记成模型删除、不进房间变化账）；
  随后 `flush_room_spatial`。**库里那一份不动**——这是缓存态的收回，不是生成出来的删除 delta。
- **时机**归 `model_write_behind` 守：每次发布之后（刚发布的这批还没 ACK、不在候选里，被挤出去的是更早落盘的根）、
  每批 durable ACK 记下之后（刚落盘的根成为候选，上限之上这时才收得动）。**只在 `store_mode().is_durable()` 下动**：
  嵌入 kv-mem 时 overlay 就是唯一一份，没有可回退的冷读。逐了就按批打一行日志；收不下去不吵。

### D2　逐出之后的每一条路都已经有去处

| 谁来读被逐出的根 | 去处 | 出处 |
|---|---|---|
| 读路（`/model/records` 等） | 没回执就直读 RocksDB（`model_record_reader::records_of_root`，不回填） | `incremental-overlay`，既有 |
| 单元级发布 / 刚体前移（便宜路） | 拿不到 S 端基线 → 回退整根 regen | ADR-080 约束 / ADR-066 规则，既有 |
| 按需生成 `/model/ensure` | overlay 没回执就生成，与冷启动同一形态 | 既有 |
| `RoomOnly` 整库房间重算 | 此前只遍历「投影里有回执的根」，逐出后会**漏算**——本文 D3 改口 | **本文** |

### D3　`RoomOnly` 整库房间重算：根全集从源文件枚举，冷根从库里读几何行（修订 ADR-073 D13）

- `recompute_dbnum_room_edges(target, store, dbnum, library_roots, rooms)` 多收一份 `library_roots`：根的全集 = overlay 里有回执的根 ∪
  `library_roots`。在 overlay 里的根拿投影里的记录；不在的（按根数上限逐出过、进程重启后没回填——两种冷根同一形态）
  从库里把几何行读回来（`model_record_reader::durable_room_records_of_root`，与读路的冷读**同一条 SQL**）。两种来源都走
  `calculate_for_root` + `render_root_room_edges` 同一套渲染，所以「整库重算 == 从零初始化」逐边相等的硬验收（ADR-010 §9）
  在逐出之后**仍成立**。冷根没有进程内 ACK 可对齐（读路对它本来就直读库），`acks_refreshed` 不计它；报表多一格 `cold_roots`。
- `library_roots` 由调用方 `batch_worker::library_generation_roots` 从**源文件**枚举：本任务冻结的源（`active_frozen_source`）→
  `E3dModelService::from_frozen_source` → `initialization_roots(dbnum)`——与从零初始化取根**同一条路**，一行库都不读。
  同一份冻结源算出同一个 `generation_identity`，本窗刚用过的那份生成输入（`DbSet` / 索引）直接复用，不另开一份。
  **为什么不从库里猜根集**：`gen_root` 已退役（ADR-079），几何行的 `anc` 是整条祖先链、分不出谁是生成根。
- **取不齐根集就说不了「整库」**：`library_generation_roots` 出错 → `RoomOnlyRecompute::Incomplete`（任务 `Partial`、`model_sesno` 不盖、
  D13 欠账留着，下一个任务再还）。`library_roots` 给空是允许的（只算 overlay，今天之前的口径）——单测与不带源的调用方用它。
- **冷根的房间视图**只有房间计算读的那几格忠实：`source_refno` / `noun` / `mesh_id` / `world_bounds` / `world_transform`
  （标准图元的本地摆放已折进世界变换，形状一律记 `Baked`——`memory_model_world_points` 对两种形状算出的世界点相同）；
  `record_id` 是 `<构件>@durable#<序>`（只进错误信息）、`ancestors` 只有构件自身与这根、`source_sesno` 为 0、`tube` 为 `None`。
  **不装回 overlay**。已知的口径差：库行上的世界变换是 f32（`trans.d`），overlay 里是 f64——恰好压在面板面上的顶点在 ulp 量级
  可能判得不同；显示读路本来就吃这份 f32，接受。

### D4　生成用元素缓存按**元素数**设上限，满则整张清

- 键 `model_element_cache_max_entries`（`DbOption.toml`），环境变量 `AIOS_MODEL_ELEMENT_CACHE_MAX_ENTRIES` 压过；**0 = 不限**
  （等同于此前的 `with_element_cache()`）。`model_element_cache=false` 时这个数没有意义。
- 缓存体在 vendor e3d-io：`DbSet::with_element_cache_limit(max_entries)`。**全池、跨库合计**，计数只数 `parsed`（抽取总跟在解析后面，
  与 `cached_element_count` 同一口径），一枚原子计数器、不逐库拿锁去数；装第 `limit + 1` 个之前先整张清空、再装它，下一次读从文件重来。
  **不做 LRU**：按访问序簿记要在每次命中的读路径上写一笔，而这张缓存的命中率是每元素上百次；「满则清」零簿记（与四张版本表缓存
  同一个做法），代价只是清空之后那一小段重新变冷。计数是近似的（并发装入时可能多一两条）——上限本来就是内存量级的旋钮。
  正在读的句柄手里的 `Arc` 不受影响；`fork_without` 带过去的条目按现状重数。
- 每份 `DbSet` 各算：生成输入缓存每库留一份，进程里的上限是「库数 × 这个数」。产物逐字不变（缓存只省重复的 open + parse）。

### D5　两个上限默认 0 = 不限：只加能力，今天的行为一个字不变

- `enforce_root_limit(0)` 早退、`with_element_cache_limit(0)` 与 `with_element_cache()` 同形——默认配置下两条代码路径与此前逐位相同。
- 现场**先量再配**：`/health.model_cache.resident_roots`（overlay 现值）、`/health.model_cache.write_behind.overlay_root_limit` /
  `overlay_evicted_total` / `element_cache_max_entries`（上限与累计逐出）。「设了上限却一直不逐」= 常驻的全是还没落盘的根。
  默认值（例如 overlay 20000 根 / 元素缓存 200000 条）等量过再拍，不在本文定。

### D6　按数量、不按字节

一根从 1 条到上千条记录不等，按记录数或字节更接近内存——但逐出单位无论如何是整根（回执 / ACK / CAS 都是根级，ADR-080 D5），
按字节记账要逐条估算 `ProjectedGeometry` 的大小、每次插入 / 删除都走一遍；按数量零成本、现场直观（`resident_roots` 已经在 `/health` 上）。
元素缓存同理：`ParsedElement` 大小差几十倍，但「多少个元素」是现场能对着 7997 那类实测数（15,968 个元素 → 数百 MB）配出来的量。
要按字节，另开 ADR。

## 不变量

1. **未落盘的根一条不丢**：逐出候选 = durable ACK 与回执逐字相符；候选用尽仍超限就停手。
2. **逐出不是删除**：库里的几何行、边、水位一格不动；不产生 removal delta、不进房间变化账（`evict` 不是 `remove`）。
3. **非 durable 介质下闸不动**：`store_mode().is_durable()` 为假时 `enforce_overlay_root_limit` 直接返回。
4. **两个上限为 0 时行为与此前逐位相同**。
5. **整库房间重算 == 从零初始化**：根全集与从零初始化同一条枚举（`initialization_roots`），冷根的几何来自库里那一份、渲染同一套。
6. 元素缓存上限只换时间不换产物：`READBACK` 两态逐字相同（缓存只省重复的 open + parse）。

## 后果

- 长驻进程的 RSS 有了两个可配的旋钮，且都不需要重启之外的任何迁移；默认关，零风险上线。
- 设了 overlay 上限之后：冷根读路直读 RocksDB（慢于内存）；便宜路（单元级发布 / 刚体前移）回退 regen 的概率随之上升；
  `RoomOnly` 多读库（冷根的几何行）。这三条都是既有回退，只是更常被走到。
- 设了元素缓存上限之后：生成的 owner 链上溯 / 目录链走查在清空之后那一段重新变冷（7999 实测缓存全开 3.84 s、全关 71 s，上限落在中间）。
- `RoomOnly` 多了一步「从源文件枚举根集」（O(ZONE 数) 的索引走查、复用本窗的生成输入），取不齐就 `Incomplete`——比此前静默漏算冷根严格。
- 可观测多了三格；overlay / 元素缓存的**字节数**仍没有任何一格（§不做）。

## 实施约束（源码钉）

1. `model_memory_store.rs`：`ScopeState::{commit_ticks, next_commit_tick, overlay_evictions}`、`drop_root`（`drop_cached_root` 与限额逐出共用）、
   `evict_acked_roots_over`、`ModelMemoryStore::enforce_root_limit` → `OverlayEviction`、`overlay_evictions_total`；`clear_dbnum` 连带清序号。
   钉：`the_root_limit_evicts_durably_acked_roots_oldest_commit_first_and_never_unacked_ones`、
   `a_mem_ack_or_a_root_republished_after_its_ack_does_not_make_it_evictable`、
   `evicting_a_nested_root_keeps_the_records_its_resident_parent_still_holds`、
   `recommitting_a_root_moves_it_to_the_newest_end_of_the_eviction_order`。
2. `model_write_behind.rs`：`enforce_overlay_root_limit(store, moment)` 两处调（`publish` 之后、`persist_batch` 记 durable ACK 之后），
   `store_mode().is_durable()` 门内；`WriteBehindSnapshot` 多三格。
3. `options.rs`：`model_overlay_max_roots` / `model_element_cache_max_entries` / `effective_count_limit`（环境变量合法非负整数压过配置、
   拼错当没设、没写 = 0）；钉 `a_count_limit_takes_a_valid_env_override_falls_back_to_config_and_defaults_to_unbounded`。
4. `e3d_model_service.rs` `build_set`：`with_element_cache_limit(model_element_cache_max_entries())`。
5. vendor e3d-io `db_element.rs`：`DbSet::with_element_cache_limit` / `element_cache_limit` / `element_cache_entries` 计数、`store_parsed` 满则清、
   `forget_cached_db` 饱和减、`fork_without` 重数；钉 `tests/dbset_element_cache_limit.rs::the_element_cache_limit_clears_the_whole_pool_when_full_and_zero_means_unbounded`
   （单开一个测试二进制：`read_stats` 是进程全局的，`dbset_generations.rs` 断言精确的打开次数）。vendor 提交 `d51c3dd`。
6. `model_record_reader.rs`：`durable_room_records_of_root` + `room_record_from_durable`；钉 `the_durable_room_view_keeps_only_what_the_room_calculator_reads`。
   `model_db_adapter.rs`：`recompute_dbnum_room_edges(…, library_roots, …)`、`RoomEdgeRecompute::cold_roots`；
   钉 `a_root_evicted_from_the_overlay_is_still_recomputed_from_its_durable_rows`。
   `batch_worker.rs`：`library_generation_roots` → `run_room_only_recompute`（`Incomplete` 第四处）；源码钉里 `RoomOnlyRecompute::Incomplete` 计 4。
7. `DbOption.toml`：`model_overlay_max_roots = 0` / `model_element_cache_max_entries = 0` 与注释；ADR-073 D13 段与源码钉 18 同步改口；
   CONTEXT「投影 overlay 根数上限」「冷根」「元素缓存元素数上限」。

## 不做 / 未决

- **不做**字节记账、不做 overlay 的读刷新 LRU、不做元素缓存 LRU（D6 / D4 已说明）。
- **不在本文**让稳态窗口收尾释放 `GenerationInput`（`forget_generation_input`，初始化那一路已经这么做）：它会改今天默认行为
  （每窗重开 MDB、元素缓存重新变冷），与 D5「一个字不变」冲突——要做单开决策卡，先量每窗重开的代价。
- **未决**默认值：先在现场量 `resident_roots` 与 RSS 的关系再定；`/dbnums` 面板没有 overlay 那一格，只有 `/health`。
- **未验证**：真机（live）上没有跑过「设上限 → 逐出 → 冷根读路 / 便宜路回退 / `RoomOnly` 读库」整条链；本文的证据是 `mem://` 单测与源码钉。
- 跨库成员（面板在 A、成员在 B）的 `RoomOnly` 那一问（ADR-073 U3）不在本文。
