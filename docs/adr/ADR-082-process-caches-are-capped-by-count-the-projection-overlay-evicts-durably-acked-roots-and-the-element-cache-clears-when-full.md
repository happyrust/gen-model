# ADR-082：进程内缓存按数量设上限——模型投影 overlay 按根数逐出已落盘的根，生成用元素缓存按元素数满则清；默认 overlay 50,000 根 / 元素缓存 200,000 条，写 0 = 不限

- 状态：**已接受、已实施**（2026-09-20 用户拍板，共识 **d-149**：「两道都要：overlay 按根数、元素缓存按元素数；RoomOnly 改成也读 RocksDB 冷根；默认 0 不限」；同日 17:0x 用户要求在 mem 档实例上量了一组数、定了两个默认值（D5）；
  同日落地——`ModelMemoryStore::enforce_root_limit` + `model_write_behind` 两处接线、vendor e3d-io `DbSet::with_element_cache_limit`、
  `recompute_dbnum_room_edges` 的根全集与冷根读库、`DbOption.toml` 两键；实施记录见文末源码钉。
  **2026-09-21 追记**：真机整条链 W1–W6 跑过——W5 / W6 `RoomOnly` 撞出冷读 SQL 在 SurrealDB 里退成全表扫描（像卡死，不是锁），
  拆成两路各走索引后 6,770 个冷根 34 s，见文末追记）
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
  **2026-09-22 补丁（d-149 审核 A / D7）**：再排除两类——库里有它的几何、但「它的房间边是另一代算的 / 还没算」这句话**只有这枚 ACK 说得出**：
  (a) ACK 房间代号是本进程这一世写进 `model_room_stray_generations` 的 stray 代号（`ScopeState::stray_generations_written`）、而库上最近一次
  `model_room_generation` 盖的不是这一代（`stamped_room_generations`，`task_end::commit_task_end_on` 真盖上之后记）——`ScopeState::ack_is_stray_room_evidence`；
  (b) 房间开关开着（`options::room_membership()`，由 `model_write_behind` 传进 `enforce_root_limit(limit, room_membership_enabled)`，store 对配置无知）
  却 `!(ack.room_membership && ack.room_generation.is_some())`——`ScopeState::ack_owes_room_edges`。
  这类根很少，定向 / 整库盖章把库上那一格对齐到它们那一代之后自然放行。
- **序**：每根记「最近一次盖回执」的单调序号（`ScopeState::commit_ticks`），最旧的先出。**读不刷新它**——被逐出的根读路直读 RocksDB，
  刷新读序只会让「常读的旧根」把「刚生成的新根」挤出去。
- **逐出 = 整根从缓存态掉**：回执、ACK、失效标记、序号，以及名下**不与别的常驻回执共有**的记录（嵌套根共有的记录归另一个回执继续持有）
  ——与 `drop_cached_root` 同一段代码（`ScopeState::drop_root`）；记录走 `evict` 不走 `remove`（不记成模型删除、不进房间变化账）；
  ~~随后 `flush_room_spatial`~~（2026-09-22 P1b：每库房间空间树 `room_spatial` 已删——没有生产读者，房间候选走 `GLOBAL_AABB_TREE`，
  却在每次提交 / 逐出后对脏库整棵重建、全在写锁里；同日 P1a 把 `drop_root` 的共有判定从「按回执表查祖先链」换成「按祖先链查回执表」，
  O(回执数) → O(祖先数 × log 回执数)，同一个谓词）。**库里那一份不动**——这是缓存态的收回，不是生成出来的删除 delta。
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
| 定向房间重算的准入与点名（`room_recompute::request_for_task` 的 `unaccounted_stray_generations` 靠「本进程写过的根 ACK 都在投影里」放行 stray 代号、`stale_acked_roots` 靠 ACK 点名）与 `query_service::room_calculation_status`（`ProcessAck::Stale` 一票否决） | **没有去处**：ACK 是唯一证据，逐出 = 证据消失——定向重算漏根、就绪误判。所以这类 ACK 的根**不进候选**（D1 2026-09-22 补丁），不是给它另找去处 | 2026-09-22 审核 A / D7 |

### D3　`RoomOnly` 整库房间重算：根全集从源文件枚举，冷根从库里读几何行（修订 ADR-073 D13）

> **2026-09-21 合并追记**：`codex/model-projection-cache` 并入主线时发现那边 2026-09-17 已独立落了同一件事（ADR-073 D13「U1 第二半」：
> `library_roots: Option<&[RefNo]>` 从 `DbSet@T` 枚举、不在投影里的根经 `model_record_reader::durable_records_of_root` 读库、
> `room_model::durable_room_inputs` 折形状、回执 `covered_roots` / `projected_roots` / `durable_roots`，覆盖面对不上是
> `RoomRecomputeVerdict::PartialCoverage`——D14 那一格不盖、电平型触发 ③ 下一个任务再点）。合并取了那一套，本节 2026-09-20 落的
> `durable_room_records_of_root` / `RoomEdgeRecompute::cold_roots` / `RoomOnlyRecompute::Incomplete` / `library_generation_roots(dbnum)`
> 与源码钉随之退役；**D3 的结论不变**（根全集不从库里猜、冷根从库读几何再算、逐出之后「整库重算 == 从零初始化」仍成立），
> 下面按 2026-09-20 写的实现细节只作历史记录，现行接线见 ADR-073 D13 与源码钉 17 / 18。

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

### D5　默认值：overlay 50,000 根、元素缓存 200,000 条 / 库；写 0 = 不限

- 落地那一刻（d-149 第 4 条）两个上限默认 0——只加能力，行为一个字不变，先量再配。**同日 17:0x 用户要求量了定默认值**
  （「在 mem 档实例上跑一窗，把 `/health.model_cache.overlay.approx_bytes` 与 `process_rss_bytes` 对着记一组数，给两个上限定默认值」），
  证据档 `docs/evidence/2026-09-20-d149-mem-footprint.md`：release 二进制、mem 档、整库即时生成 8000 / 7998 / 7997 / 1112 四库
  → 7,743 根 / 59,929 条记录，overlay 106.5 MB（≈ **13.8 KB / 根**、1.8 KB / 记录），元素缓存 219,623 条 820.6 MB
  （≈ **3.6–4.7 KB / 元素**，7997 单库整库一遍 176,982 条 643.8 MB），RSS 2.04 GB 里这两块合计 46%（其余是 mem 档的 kv-mem 模型行）。
- **`model_overlay_max_roots` 默认 50,000**（`options::MODEL_OVERLAY_MAX_ROOTS_DEFAULT`）：≈ 0.7 GB；整套 AMS 样例 MDB 装得下六遍
  （样例现场永远不逐出），「AMS 全量 12 万根」量级下把常驻压到四成。再小就拿便宜路换内存。
- **`model_element_cache_max_entries` 默认 200,000**（`options::MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT`）：≈ 0.7–0.95 GB / 库；
  样例里最大的库整库一遍装到 177k，默认值下一遍生成不会清一次，清的是长驻之后越过一整遍的累积。它管不住「库数 ×」那一维
  （§不做：稳态窗口收尾释放 `GenerationInput`）。
- 三层裁决：环境变量 > 配置键 > 默认值；**任何一层写 0 = 不限**（`effective_count_limit(env, configured, default)`）。
  `enforce_root_limit(0)` 早退、`with_element_cache_limit(0)` 与 `with_element_cache()` 同形。overlay 那道只在 durable 介质逐出，
  mem 档下默认值不改变任何行为；元素缓存那道两档都作用，样例规模下一遍生成也碰不到。
- 现场对着看：`/health.model_cache.overlay` / `element_cache`（下一条）、`write_behind.overlay_root_limit` / `overlay_evicted_total` /
  `element_cache_max_entries`。「设了上限却一直不逐」= 常驻的全是还没落盘的根。
- **估算字节（2026-09-20 用户追加拍板：「查时现算、不落库」）**：`/health.model_cache.overlay`
  `{ records, roots, acks, approx_bytes, max_roots, evicted_total }` 与 `/health.model_cache.element_cache`
  `{ enabled, max_entries, parsed_entries, approx_bytes, databases[{ dbnum, source_sesno, parsed_entries, extraction_entries, approx_bytes, max_entries }] }`。
  两个 `approx_bytes` 都是**被查时现算、直接回给前端**的数，不落库、不缓存、不进任何表：
  - overlay（`ModelMemoryStore::overlay_footprint`）：记录那一半是插入 / 删除时随手记的账（`ScopeState::record_bytes`，每条按
    `approx_record_bytes` = 内联结构 + 自己的字串与祖先链 + 四张索引里的 `record_id` 副本与树节点），O(1) 读；回执与 ACK 在读锁里
    走一遍（根的量级）。**不含**盘上的网格（每库房间空间树此前也不在里面，2026-09-22 已删，见 D1 追记——D5 的 13.8 KB / 根从此不再低估）。
  - 元素缓存（`e3d_model_service::generation_input_cache_footprint` → e3d-io `DbSet::element_cache_stats`）：在缓存锁里抄出各份
    `Arc<DbSet>`、锁外逐份走它的两张表（`Arc` 块 + 表槽 + 内联结构 + `ParsedElement` / `ElementExtraction::approx_heap_bytes`
    的字串 / 数组载荷），O(条数)，按 `/health` 的观测频率付，不进生成路径。
  - 都按**长度不按容量**：是给人对着 RSS 定上限用的估算，不是分配器审计。

### D6　按数量、不按字节

一根从 1 条到上千条记录不等，按记录数或字节更接近内存——但逐出单位无论如何是整根（回执 / ACK / CAS 都是根级，ADR-080 D5），
按字节记账要逐条估算 `ProjectedGeometry` 的大小、每次插入 / 删除都走一遍；按数量零成本、现场直观（`resident_roots` 已经在 `/health` 上）。
元素缓存同理：`ParsedElement` 大小差几十倍，但「多少个元素」是现场能对着 7997 那类实测数（15,968 个元素 → 数百 MB）配出来的量。
要按字节，另开 ADR。

## 不变量

1. **未落盘的根一条不丢**：逐出候选 = durable ACK 与回执逐字相符；候选用尽仍超限就停手。
2. **逐出不是删除**：库里的几何行、边、水位一格不动；不产生 removal delta、不进房间变化账（`evict` 不是 `remove`）。
3. **非 durable 介质下闸不动**：`store_mode().is_durable()` 为假时 `enforce_overlay_root_limit` 直接返回。
4. **任何一层把上限写成 0 时行为与此前逐位相同**（默认值不是 0 之后，「一个字不变」要靠显式写 0 换取）。
5. **整库房间重算 == 从零初始化**：根全集与从零初始化同一条枚举（`initialization_roots`），冷根的几何来自库里那一份、渲染同一套。
6. 元素缓存上限只换时间不换产物：`READBACK` 两态逐字相同（缓存只省重复的 open + parse）。

## 后果

- 长驻进程的 RSS 有了两个可配的旋钮，且都不需要重启之外的任何迁移；默认关，零风险上线。
- 设了 overlay 上限之后：冷根读路直读 RocksDB（慢于内存）；便宜路（单元级发布 / 刚体前移）回退 regen 的概率随之上升；
  `RoomOnly` 多读库（冷根的几何行）。这三条都是既有回退，只是更常被走到。
- 设了元素缓存上限之后：生成的 owner 链上溯 / 目录链走查在清空之后那一段重新变冷（7999 实测缓存全开 3.84 s、全关 71 s，上限落在中间）。
- `RoomOnly` 多了一步「从源文件枚举根集」（O(ZONE 数) 的索引走查、复用本窗的生成输入），取不齐就 `Incomplete`——比此前静默漏算冷根严格。
- 可观测：`write_behind` 三格之外，`/health.model_cache.overlay` / `element_cache` 各给条数、上限与估算字节（D5）；
  `/health` 每次多付一遍回执 / ACK 与元素缓存的遍历。

## 实施约束（源码钉）

1. `model_memory_store.rs`：`ScopeState::{commit_ticks, next_commit_tick, overlay_evictions}`、`drop_root`（`drop_cached_root` 与限额逐出共用）、
   `evict_acked_roots_over`、`ModelMemoryStore::enforce_root_limit` → `OverlayEviction`、`overlay_evictions_total`；`clear_dbnum` 连带清序号。
   钉：`the_root_limit_evicts_durably_acked_roots_oldest_commit_first_and_never_unacked_ones`、
   `a_mem_ack_or_a_root_republished_after_its_ack_does_not_make_it_evictable`、
   `evicting_a_nested_root_keeps_the_records_its_resident_parent_still_holds`、
   `recommitting_a_root_moves_it_to_the_newest_end_of_the_eviction_order`。
   2026-09-22 补丁：`ScopeState::{stamped_room_generations, ack_is_stray_room_evidence, ack_owes_room_edges}`、
   `ModelMemoryStore::note_room_generation_stamped`（`task_end::commit_task_end_on` 真盖上 / 真清空之后调；`clear_dbnum` 连带清）、
   `enforce_root_limit(limit, room_membership_enabled)` 多一位由 `model_write_behind::enforce_overlay_root_limit` 传 `options::room_membership()`；
   钉 `a_root_whose_ack_carries_an_unstamped_stray_room_generation_is_not_evictable`、
   `a_root_published_without_room_membership_while_the_switch_is_on_is_not_evictable`。
   后续（写锁热点、`GenerationInput` 按库数上限、`Historical` 封顶、孤儿 ACK）见 `docs/plans/2026-09-22-incremental-memory-control-dev-plan.md`。
2. `model_write_behind.rs`：`enforce_overlay_root_limit(store, moment)` 两处调（`publish` 之后、`persist_batch` 记 durable ACK 之后），
   `store_mode().is_durable()` 门内；`WriteBehindSnapshot` 多三格。
3. `options.rs`：`model_overlay_max_roots` / `model_element_cache_max_entries` / `effective_count_limit`（环境变量合法非负整数压过配置、
   拼错当没设、没写 = 0）；钉 `a_count_limit_takes_a_valid_env_override_falls_back_to_config_and_defaults_to_unbounded`。
4. `e3d_model_service.rs` `build_set`：`with_element_cache_limit(model_element_cache_max_entries())`。
5. vendor e3d-io `db_element.rs`：`DbSet::with_element_cache_limit` / `element_cache_limit` / `element_cache_entries` 计数、`store_parsed` 满则清、
   `forget_cached_db` 饱和减、`fork_without` 重数；钉 `tests/dbset_element_cache_limit.rs::the_element_cache_limit_clears_the_whole_pool_when_full_and_zero_means_unbounded`
   （单开一个测试二进制：`read_stats` 是进程全局的，`dbset_generations.rs` 断言精确的打开次数）。vendor 提交 `d51c3dd`。
6. （2026-09-21 合并后改口）冷根读路是 `model_record_reader::durable_records_of_root` → `room_model::durable_room_inputs`
   （ADR-073 D13 U1 第二半那一套）；`model_db_adapter.rs`：`recompute_dbnum_room_edges(…, rooms, library_roots: Option<&[RefNo]>, RootScope)`、
   回执 `projected_roots` / `durable_roots` / `covered_roots`；钉 `a_root_outside_the_projection_gets_its_room_edges_recomputed_from_the_durable_rows`。
   `batch_worker.rs`：`library_generation_roots(dbnum, target_sesno)` → `run_room_only_recompute`，枚举不出 = `RoomRecomputeVerdict::PartialCoverage`。
   2026-09-20 落的 `durable_room_records_of_root` / `cold_roots` / `RoomOnlyRecompute` 已随合并退役（主线钉 `the_durable_room_view_keeps_only_what_the_room_calculator_reads`
   随之删除）。
   冷读 SQL 本身（读路与 `RoomOnly` 冷根共用）在 `durable_records_sql` / `durable_scope_reads`：每张表拆成 `anc CONTAINS` 与 `in =` 两句各走
   `idx_<表>_anc` / `idx_<表>_in`，库端 `array::union` 再 `ORDER BY record_order`——**不许合成一条 `… OR …`**（2026-09-21 追记）；
   钉 `durable_cold_read_scopes_are_planned_through_indexes`（四路 `EXPLAIN FULL` 不出现 `Iterate Table`、落在对应索引，并反证旧 OR 形态退回扫表）。
7. `DbOption.toml`：`model_overlay_max_roots = 0` / `model_element_cache_max_entries = 0` 与注释；ADR-073 D13 段与源码钉 18 同步改口；
   CONTEXT「投影 overlay 根数上限」「冷根」「元素缓存元素数上限」。
8. 估算字节（D5 追加）：`model_memory_store.rs` `ScopeState::record_bytes`（`insert` 加 / `remove_internal` 减）、`approx_record_bytes`、
   `approx_model_target_bytes`、`ModelMemoryStore::overlay_footprint` → `OverlayFootprint`；钉
   `the_overlay_footprint_counts_what_is_resident_and_grows_with_it`（删光归零 = 加减账对得上）。
   `e3d_model_service::generation_input_cache_footprint` → `GenerationInputFootprint`；`handlers::model_cache_health` 的
   `overlay` / `element_cache` 两块。vendor e3d-io：`ParsedElement::approx_heap_bytes`、`ElementExtraction::approx_heap_bytes`、
   `DbSet::element_cache_stats` → `ElementCacheStats`（钉在 `dbset_element_cache_limit.rs` 同一条用例末尾；vendor 提交 `4a76d2a`）。

## 不做 / 未决

- **不做**字节记账、不做 overlay 的读刷新 LRU、不做元素缓存 LRU（D6 / D4 已说明）。
- **不在本文**让稳态窗口收尾释放 `GenerationInput`（`forget_generation_input`，初始化那一路已经这么做）：它会改今天默认行为
  （每窗重开 MDB、元素缓存重新变冷），与 D5「一个字不变」冲突——要做单开决策卡，先量每窗重开的代价。
  → **2026-09-22 那张卡是 ADR-084**：按**库数**设上限（`model_generation_input_max_dbnums`，最近一次生成用到的先后逐库整份出、命中刷新），
  落地默认 0 = 不限，默认值等两库交替窗量过重开代价再定；「每窗收尾释放」仍不做。
- 默认值已按 mem 档实测定下（D5）；**没量**的是 rocksdb 档下逐出真正发生之后的三条回退路与稳态小窗口的增量——现场先看
  `/health.model_cache.overlay.evicted_total` 有没有动。`/dbnums` 面板没有 overlay 那一格，只有 `/health`。估算字节只在 `/health` 上
  现算，**不做**任何持久化 / 历史曲线（2026-09-20 用户拍板）。
- ~~**未验证**：真机（live）上没有跑过「设上限 → 逐出 → 冷根读路 / 便宜路回退 / `RoomOnly` 读库」整条链~~ → **2026-09-21 在 d-149 私有实例上跑过
  W1–W6**（文末追记）：逐出、冷根读路、便宜路回退、`RoomOnly` 读库都走到了；`RoomOnly` 撞出冷读 SQL 退成扫表，已修。**仍没量**的是 rocksdb 档下
  便宜路回退 regen 的单独代价，与每窗重开 `GenerationInput` 的代价（上一条决策卡）。
- 跨库成员（面板在 A、成员在 B）的 `RoomOnly` 那一问（ADR-073 U3）不在本文。

## 追记（2026-09-21 11:5x）：真机整条链 W1–W6——W5 / W6 `RoomOnly` 停 67 分钟不是锁，是冷读 SQL 在 SurrealDB 里退成全表扫描；拆两路走索引后 6,770 个冷根 34 s

用户 2026-09-20 20:36 拍 (a)：起私有实例压小上限跑几窗，把整条链量出来。

- **怎么跑**：私有实例 `D:\d149live`（release 二进制 `d258c0fba`、HTTP `:18093`、surreal `:18149` **rocksdb**、dbnum 7997 AvevaMarineSample、
  `DbOption-d149-live.toml`），`AIOS_MODEL_OVERLAY_MAX_ROOTS=4`、`AIOS_MODEL_ELEMENT_CACHE_MAX_ENTRIES=100000`，W2 起加 `AIOS_MODEL_TRANSFORM_CHEAP_PATH=1`；
  起窗 + 采样 + 切日志用 `Run-Window.ps1`，每窗的任务 JSON / `/health` 采样 / 日志切片落在 `D:\d149live\windows\W*\`；
  六窗的数与读法整理在 **`docs/evidence/2026-09-21-d149-live-chain.md`**。
- **W1 zone-move → W4 restore-mixed 都走通**（每窗 9 根 BRAN、2.6–2.8 s、`in_sync`）：逐出如 D1——每窗 durable ACK 后「9 → 4 根，逐出 5」，
  2 s 采样能看到 9 根在 overlay 里的那一瞬；`POST /model/records` 读被逐的根走 durable 冷根路、结果正常。**便宜路一次都没走成**：被逐 / 重启后冷的根
  → `projection incomplete` → 回退整根 regen（D2 那条，真机走到了）；留在 overlay 里的 4 根 → `model target: immutable target changed outside the
  design window … rigid motion cannot cover it`——前一窗刚 regen 过、目录 / 模板没动，**为什么判成这样没查**（新欠账，不影响正确性）。
- **W5 room-rename（`RoomOnly`）与 W6 room-restore 复跑都「卡死」**：W5 task `db-20260921-002335-000007` 00:23:39 起停在 `rooms` 阶段、7 分钟没动被杀
  （证据 `D:\d149live\windows\W5-room-rename-roomonly\{task-000007-stuck.json, health-stuck.json, stuck-process.txt}`）；W6 用同一二进制复跑，00:35:37 起到 01:42
  被杀，67 分钟。现场像锁：aios 115 线程全 Wait、CPU 57 s 后不再涨、`stage_last_progress_at` 不动（逐根循环里没有进度计）。**但 surreal 单核满转**
  （6 s 墙钟吃 6.0 s CPU），aios 侧 e3d-io 读计数（`/health.core.source_reads`）从进 `rooms` 十几秒后起一格不动——aios 不在读源，时间全在库端；
  aios 到 surreal 只有一条 WS 连接，`recompute_dbnum_room_edges` 对每个冷根串行 `await` 一次 `durable_room_records_of_root`，所以客户端那边只看得到空转。
- **根因**：`model_record_reader::durable_records_of_root` 那条冷读 SQL（读路 `/model/records` 与 D3 的冷根共用）
  `WHERE (anc CONTAINS <根> OR in = <根>) AND aabb.d != NONE AND world_trans.d != NONE`——SurrealDB 2.1.4 对**纯 OR** 会做索引并集，
  一挂上别的 AND 谓词就整条退回 `Iterate Table`（`EXPLAIN FULL` 实测）。47,751 行的 `inst_relate` 上一根 **786 ms**，`tubi_relate` 再扫一遍；
  7997 共 6,772 根、overlay 上限 4 → 6,770 个冷根 × 0.8–1.6 s ≈ 90–177 分钟。`model_db_adapter.rs` 的 `render_spooled_scope_cleanup` 早就记过并绕开了
  同一个坑，冷读这条没跟上；本文之前 overlay 不逐出、冷根只在重启后出现，所以一直没被撞到。
- **修法**（`model_record_reader.rs`，+186/−25）：`durable_scope_reads` 把每张表拆成两句（`anc CONTAINS` 走 `idx_<表>_anc`、`in =` 走 `idx_<表>_in`），
  `durable_records_sql` 在库端 `array::union` 去重后 `ORDER BY record_order`；行集与顺序和原来那一条 OR **逐字相同**（6 根现场对过），读路的游标偏移不变。
  每根 ~2 ms 服务端（22 ms 含 HTTP）。单测 `durable_cold_read_scopes_are_planned_through_indexes` 钉住四路计划都落在索引、不出现 `Iterate Table`，并反证
  旧 OR 形态在这引擎上确实退回扫表（哪天不退了这条会红，提醒把注释改掉）；原用例扩到 5 行，含「根自己的几何（`anc` 缺失的历史行）」只靠 `in =` 那一路兜住、
  「两路都命中的管身行」去重。`cargo test --lib -q model_record_reader`：d149live 树（`d258c0fba` + 修法）5 绿；主线 `04b103f2e` + 同一份文件在隔离
  worktree 重跑 5 绿（11:51）。
- **真机验证**（同一实例换 build2 = `d258c0fba` + 修法，serve5，01:46 起）：杀 W6 后 Reinitialize 1..366 **111 s**（6,772 根、68,143 房间边）；
  紧接 365..366 窗（FRMW 属性改 → `RoomOnly`）**34 s 成功**，日志原文「整库房间重算完成：根 6433/6772（其中冷根 6770 从库读）、成员边 68260、事务 36、
  ACK 对齐 2、耗时 32103ms」。对照 W5 7 分钟 / W6 67 分钟没动。
- **对本文的意思**：D2 表里「读路直读 RocksDB」与 D3「冷根从库里读几何行」的单价从 O(表) 回到 O(命中行)；D3「与读路的冷读同一条 SQL」仍成立
  （同一个 `durable_records_sql`）。`RoomOnly` 在 rocksdb 档**重启后的第一窗**会对整库逐根冷读（`incremental-overlay` 不回填），修前那就是 90 分钟——
  0.1.29（`92ef5365c`）/ 0.1.30（`3c04f0ddb`）两个包都还带着旧形态（release exe 里字串 4 处），下一个包收进。
- **仍开着**：每窗重开 `GenerationInput` 的代价（§不做那张决策卡；这组窗同库连续、没有重开发生，量不到）；rocksdb 档便宜路回退 regen 的单独代价
  （便宜路本来就没走成，量不出来）；留在 overlay 里的根为什么被判 `immutable target changed`；`/dbnums` 面板「逐出 / 上限」那一格；
  `RoomOnly` 修后的 34 s 仍是 6,770 个冷根串行读库，并行 / 批读没看。
