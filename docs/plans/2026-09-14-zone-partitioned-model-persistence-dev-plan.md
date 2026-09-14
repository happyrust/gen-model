# 按 ZONE 分组落库：删掉对账读

## 目标

**让一个 ZONE 的「生成 → 落库」跑得快。** 不是吞吐量，不是内存，就是单个 ZONE 的往返成本。

**不建新表。** 表结构保持现状，`inst_relate` / `tubi_relate` 照旧。

## 现状的代价

`persist_roots_batched` 每组落库之前都要读一次「库里现有的 e3d-model 几何身份」做对账。7997 那一轮实测：68 组累计读约 **180 万行**，单笔事务从头部的 400 ms 涨到尾部的 **1280 ms**——写入行数一路没变，涨的全是这个读。

它慢的原因不是「读」本身，是**读的范围**：每组都去取「这个库至今为止的全部几何身份」，而写的人正是自己。库越写越大，读也越来越大，典型 O(N²)。

### 对账整个不要

`inst_relate` / `tubi_relate` 一行同时承载**属性面**（`in`/`out`/`generic`/`anc`/`dbnum`/`solid`/`dt`/`has_cata_neg`，由 `pdms_inst.rs` 写）和**模型面**（`direct_model`/`booled_id`/`insts_flat`/mesh 指针，由 `model_db_adapter.rs` 写）。看上去模型管道只拥有行上一个子对象、删不掉整行，所以必须先读出确切 id。

**但这个前提是错的。** 两件事推翻它：

1. **模型管道渲染的是整行，不是补丁。** `render_chunk` 吐的 `inst_relate` 行把 `in`/`out`/`generic`/`dbnum`/`anc`/`solid` 全写齐了。一行只要带模型面的列，就说明它是模型管道整行写下去的。
2. **它本来就在整行删。** `append_geometry_removal` 删的是 `inst_relate` + `inst_info` + `aabb` + `trans` + `geo_relate` 五张表的整行。它缺的从来只是「删哪些 id」这份名单。

所以把名单换成谓词就行，不需要新表：

```sql
BEGIN;
  -- 删整片：这些 ZONE 名下、模型管道拥有的全部行
  --   `model_sesno != NONE` 是护栏：属性面独有的行（pdms_inst 写的、
  --   从没被模型碰过的）不带这一列，谓词碰不到它们。
  LET $slice_rels = SELECT id, out FROM inst_relate
                    WHERE anc CONTAINSANY [<gen_root>…] AND model_sesno != NONE;
  -- 连带 inst_info / aabb / trans / geo_relate / tubi_relate 同一批 id
  -- 插整片：本次投影的全集
  INSERT RELATION IGNORE INTO inst_relate [ … ];
  -- 房间边同事务先清后写；gen_root 回执同事务盖章
COMMIT;
```

存活的行反正会被重新插一遍，所以不必挑出来保留——**「删整片 + 插整片」写的行数与今天「删陈旧 + 插全集」完全一样**。

180 万行不再跨线，16 MiB 帧问题、重连循环、分页、整库基线缓存与它的 7 个失效钩子，全部消失。

**一处没能做到零读**：房间清边与空间树摘条目都要「这一片里有哪些来源元素」这份名单，树上没有祖先信息、给不了谓词。所以整片删之前仍读一次 `slice_sources_on`——作用域是**这一片**、只取 `in` 的 refno、不取几何身份、不缓存、不分页。O(N²) 的斜率因此拉平（读的大小由 ZONE 决定，不再由库里攒了多少决定），但不是零读。

## 决策

| # | 决策 |
| --- | --- |
| 表结构 | **不动**。不建新表、不迁移、不双写、不对拍 |
| 内存 | 不考虑。LRU / 字节记账 / `ModelCacheStats` 整层删除 |
| 分组键 | `gen_root`；默认取 ZONE 层，拿不到就上溯到 SITE。恒存在且唯一 |
| 对账读 | **整个不要**。改成按 `anc CONTAINSANY [<gen_root>…] AND model_sesno != NONE` 谓词删整片 |
| 行上的来源账 | `direct_model` 整个对象删掉，只留一列 `model_sesno`。**本轮保留**，见下 |
| 写入 | 一个 ZONE 一笔事务 |
| 失败 | 整体回退 → 踢掉坏根 → 剩下的重落 |
| 过大 ZONE | 不处理。超过上限就整个 ZONE 失败并报错。拆块与 `publishing` 栅栏一并删除（**已落**）|
| 并发 | `max_concurrent_zones` 只管生成，默认 1；落库单线程串行，中间一个队列 |
| 搬家 | 两个 ZONE 的 `gen_root` 打 `stale`，下轮重建 |
| `inst_geo` | 内容寻址的共享原型，不分区、只增不删（ADR-060 D2） |
| ACK 机制 | 暂不动，本轮跑通后再评估 |

## 分组与并发

- 循环键从「512 根一组」换成 `gen_root`。一个 ZONE 一组、一笔事务、一次对账读。
- 生成阶段并发度 = `max_concurrent_zones`，默认 1。
- 落库阶段恒为单线程。所有跨 ZONE 共享的写（`inst_geo`、房间边、`aabb`/`trans`、水位）因此不会并发冲突。
- 两阶段之间一个队列：`max_concurrent_zones = 1` 时仍有一层重叠，生成 Z2 的同时落库 Z1。

现有的 `ModelWriteBehind` 就是那个串行落库级，`ModelMemoryStore` 就是那个队列——这一层是重新接线，不是新盖。

## 删除清单

- ~~`ModelMemoryStore` 的整个 LRU 有界缓存：`receipt_resident_bytes`、`geometry_bytes`、`resident_bytes`、`total_resident_bytes`、`current_max_bytes`、`with_cache_limit()`、`touch_root()`、两个淘汰循环、`resident_bytes_of_root()`、`CacheControl`~~ **已落**
- ~~`ModelCacheStats` 整个结构与 `cache_stats()`，`/health` 的 `model_cache` 段~~ **已落**
- ~~`OverlayBackpressure` 的字节语义 → `OverlayRejected`~~ **已落**
- ~~`model_write_behind` 的 `pending_bytes` 链路~~ **已落**
- ~~`options::model_cache_max_bytes()` 与配置键~~ **已落**
- ~~所有查询里的 `direct_model.source = 'e3d-model'` 谓词，以及 `legacy_model_gen`~~ **已落**
- ~~`existing_geometry_for_window` / `existing_geometry_of_dbnums_on` / `existing_geometry_pages_on` 与刚写的分页，以及它要绕开的 16 MiB 帧问题与重连循环~~ **已落**
- ~~`GeometryBaseline` 结构与全局 Mutex 缓存、`advance_geometry_baseline()`、`invalidate_geometry_baseline()`（7 个调用点）~~ **已落**
- ~~`ExistingGeometry` / `under()` 这条客户端对账链~~ **已落**（`reconciliation_ready()` 留着——它现在决定「敢不敢把这一片推掉」）
- ~~`projected_geometry_ids_on` / `projected_geometry_ids_query`（每根一问的旧路）~~ **已落**
- ~~二分重试里那个不收窄的整库读~~ **已落**
- ~~行上的 `memory_projection`（整条投影 JSON 副本）与那套只有测试在用的库→内存回灌路径~~ **已落**
- ~~`direct_model` 对象（`source` / `format` / `geometry_id` / `mesh_id` / `primitive_key`），只留平铺的 `model_sesno`~~ **已落**
- ~~`chunk_bytes` 参数穿线、`geometry_chunks` 组装、`render_publication_fence` / `render_publication_fence_batch`、`publication_status='publishing'` 中间态~~ **已落**
- ~~`PERSIST_CHUNK`、`persist_root_in_chunks` / `persist_dbnum_in_chunks` / `persist_roots_batched_with_chunk_bytes`、逐根那条「渲染 > 2 MiB 劈半」的重切循环，以及 `e3d_persist_smoke --chunk`~~ **已落**

## 执行顺序

1. ~~**删内存层**：LRU / 字节记账 / `ModelCacheStats` / `pending_bytes` / `model_cache_max_bytes`~~ **已落 2026-09-14**，配置键连同模板与 `DbOption.toml` 一起摘。`direct_model.source` 谓词与 `legacy_model_gen` 同批退役。
2. ~~**对账读整个拿掉**，换成谓词删整片。三条路径一起改：窗口主路、二分重试路、`persist_root_chunks_impl` 的每根一问路。`GeometryBaseline` 整套缓存与失效钩子、分页、客户端 diff 一并删。~~ **已落 2026-09-14**
   - 行的删除已经完全不依赖读：`render_slice_delete` 一条 `anc CONTAINSANY [...] AND model_sesno != NONE` 圈掉六张表，多少根都是一个语句块。
   - `room_edges_to_clear` 那一块**没有**按原计划改成 SQL 谓词清。原因是这份名单还有第二个消费者：`converge_tree_after_persist` 要按它把进程内空间树上的条目摘掉，而树没有祖先信息、拿不到谓词。改成 `slice_sources_on`——整片删之前读一次**这一片**的来源 refno 清单（只取 `in`，不取几何身份、不缓存、不分页），房间边与树条目共用。作用域从「整库」收成「这一片」，O(N²) 那条斜率因此拉平；但落库路径上**不是零读**，验收第一条相应改写。
   - `direct_model` 与 `memory_projection` 两列一并从行上摘掉，只留 `model_sesno`（`fast_delete` 的回退到水位靠它，谓词从 `direct_model.sesno > S` 改成 `model_sesno > S`）。行因此小了一大截——`memory_projection` 是整条投影记录的 JSON 副本。
3. ~~**一个 ZONE 一笔事务**：删掉拆块与 `publishing` 栅栏；失败整体回退 + 踢坏根重落。~~ **已落 2026-09-14**
   - 两条路都收成一笔：合块那一路（`persist_group`）与逐根那一路（`persist_root_impl`）。删整片、插整片、房间边、盖章全在同一笔里，成不成是一个字。装不下就整组 / 整根失败（合块那一路照旧二分重试到单根），不拆。
   - `publication_status='publishing'` 跟着退役：它存在的唯一理由是「行已落、章未盖」这个中间态要让读侧避让，而中间态现在不存在了。`publication_status` 本身留着，`ready` / `stale` 照旧。
   - 「失败整体回退」由事务本身给；「踢坏根重落」沿用现有的二分重试与 `DeterministicPublicationFailure` 分类，没有新机制。
   - 本地载荷超上限的判断提前到**碰库之前**：一根渲出来就超 `PUBLICATION_STATEMENT_HARD_CAP` 时直接 `Blocked`，不会先去读一遍再发现。
   - 这一步落定之后 `model_sesno` 才能考虑摘掉，顺序不能反。它现在兼两份工：**护栏**（区分模型面行与 `pdms_inst` 写的属性面行，整片删靠它不误伤——这一份无论如何得有一列，只是不必带 sesno；现成的替代是 `inst_relate` 上的 `booled`，`tubi_relate` 在默认构建下没有第二个写者，`cata_model::gen_cata_geos` 只在 `legacy_model` feature 后面）与**回退到水位**（`model_sesno > S`）。后者换成 `gen_root.source_end_sesno > S` 驱动、复用整片删就行，但**前提是行与回执同生共死**——今天一组可能发好几笔事务、回执在最后一笔，崩在中间会留下有行没回执的根，只有 `model_sesno > S` 兜得住。
   - 2026-09-14 已与用户确认：`model_sesno` **先保留**。
4. **循环键换成 `gen_root`**：生成并发参数 + 串行落库队列。

## 实测（2026-09-14，AMS dbnum 8000）

release 构建、真写 RocksDB（fork surreal，全新空数据目录）。2,655 件几何、895 个唯一网格、3 个生成根 → 3 笔事务。

| 阶段 | 改之前 | 改之后 |
| --- | --- | --- |
| 生成 | 10.8 s | 10.4 s |
| 落库（冷，空库） | 16.7 s | **2.03 s** |
| 落库（热，同库再落一遍） | 50.3 s | **2.74 s** |

热路径快 18 倍。两个真因都不在「读不读库」上：

1. **列上套函数 → 丢索引 → 全表扫 + 线性找**。`DELETE aabb WHERE record::id(id) IN $ids`（`trans` 同）里 `record::id(id)` 让优化器整条放弃索引退回 `Iterate Table`，扫到的每一行再去 2,500 项数组里线性找一遍，两表两句 OR 就是上千万次比较。改成按记录点删（`DELETE $keys.map(|$k| type::thing('aabb', $k))`）。**同一个形态在 `render_spooled_scope_cleanup` 里还留着**，那条路暂不在热路径上。
2. **整片删之后又按 id 删一遍**。整片删覆盖得到的根（有回执）改走 `WriteMode::Initial`，省掉四个各两千多项的 `type::thing()` 数组；没回执的根照旧 `Incremental`。

剩下的「读 + 删」总共 0.71 s / 2,655 件，占端到端 13 s 的 5%——`slice_sources_on` 还能用内存里的 `ProjectionCommit.removals` 换掉，但按这个量级不值得拿健壮性去换（内存那份只在一个进程内与库一致）。

**一条读数须知**：这台机器上同时跑着十几个别的会话留下的 surreal 进程，绝对值有噪声；冷 / 热两轮是同进程同服务器背靠背跑的，差值才是可信的那个。

上表的「生成」一列量的是**已退役的串行入口**（`generate_dbnum` → `generate_refs`，几何并发闸利用率 0.0%、零等待），不是生产的数。烟测换到生产那条并行生产者之后的重测见下。

## 尺子校准之后（2026-09-14，同机同库，16 物理核）

`e3d_persist_smoke --dbnum` 现在走 `generation_roots_of` → `generate_and_persist_roots`（`model_refresh` 的生产路径）。

| | 生成 | 闸利用率 / 等许可 | 落库收尾 |
| --- | --- | --- | --- |
| 串行老入口（退役前） | 10.4 s | 0.0% / 0 ns | 2.03 s（冷）· 2.74 s（热） |
| 并行生产者 + `persist_dbnum` | 8.38 s（冷）· 8.0–11.0 s（热 ×3） | 6.2% / 2–4 µs | 1.78 s（冷）· 2.4–2.7 s（热）**5 次挂 2 次** |
| 并行生产者 + 等 write-behind（现在） | 8.96 s（冷）· 7.7–8.8 s（热 ×4） | 6.2% / 1–3 µs | **0.76 s**（冷）· **1.07–1.19 s**（热）5 次全绿 |

**16 个 worker 只换来 10.4 → 8.4 s，因为几何根本不是大头。** 利用率 6.2% × 额度 16 × 8.38 s ≈ **8.3 核·秒**的闸内持有时长——16 路铺开也就 0.5 s 墙钟。剩下的约 7.9 s 全在几何段之外（取根 / 解析 / 投影提交那一侧），等许可几微秒说明闸从来没堵过。**生成侧下一钐要砍的是那 7.9 s，不是加并发。**

第三行的「落库收尾」**与前两行不是同一个量**：它是生成收尾之后还要等的那一截。持久介质上 write-behind 在生成还没跑完时就已经在刷了，被盖住的那部分量不出来——所以 1.78 → 0.76 不是「又快了一倍」，是「大半被生成盖住了」。落库真正的总代价看前两行。

**烟测第二步曾经与生产不是同一条路，而且会打架。** `model_db_adapter::persist_dbnum` 在生产里**没有调用者**（ADR-071：durable 行只过 write-behind 那道缝，`model_refresh` 里连 `persist_root(` 都被测试钉着不许出现）。而 `generate_and_persist_roots` 一提交投影，write-behind 的后台 worker 就开始刷 durable——烟测紧接着再调 `persist_dbnum` 就是第二个写者，热跑 **5 次挂 2 次**：`Failed to commit transaction due to a read or write conflict`，81 条语句整笔回退（冷跑不挂，库里没有旧行要覆盖）。

2026-09-14 已改：烟测两档（整库 / `--root`）都改成 `wait_until_model_rows_are_durable`——持久介质上一行都不写，只等那道缝把交出去的根刷完；两档内存介质没有 write-behind（`publish` 在非持久 store 上只提交内存投影），由它自己补一次 `persist_dbnum`，那一档没有第二个写者。持久 5 次全绿、内存一次绿，`READBACK` 逐字不变。

## 验收

- **落库路径上不再把几何身份拉回客户端**：唯一剩下的读是 `slice_sources_on`，只取这一片的来源 refno，供房间清边与空间树摘条目用；行的删除完全走谓词。
- 读与单笔事务耗时都不随已落库量增长：同一个库连续落 N 个 ZONE，第 N 个与第 1 个同量级（现状是 400 ms → 1280 ms）。谓词必须走 `idx_*_anc`（`slice_predicates_are_planned_through_ancestor_indexes` 钉住）。
- 属性面独有的行（`model_sesno = NONE`）在落库前后逐字不变。
- 落库失败后库里不留半个 ZONE；坏根被单独标出，同 ZONE 其余根正常发布。
- 搬家之后两个 ZONE 的 `gen_root` 都是 `stale`，重建一轮后模型齐全。
- `max_concurrent_zones = 1` 时，生成与落库有重叠（不是严格串行等待）。
