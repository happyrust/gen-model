# 按 ZONE 分组落库：删掉对账读

## 目标

**让一个 ZONE 的「生成 → 落库」跑得快。** 不是吞吐量，不是内存，就是单个 ZONE 的往返成本。

**不建新表。** 表结构保持现状，`inst_relate` / `tubi_relate` 照旧。

## 现状的代价

`persist_roots_batched` 每组落库之前都要读一次「库里现有的 e3d-model 几何身份」做对账。7997 那一轮实测：68 组累计读约 **180 万行**，单笔事务从头部的 400 ms 涨到尾部的 **1280 ms**——写入行数一路没变，涨的全是这个读。

它慢的原因不是「读」本身，是**读的范围**：每组都去取「这个库至今为止的全部几何身份」，而写的人正是自己。库越写越大，读也越来越大，典型 O(N²)。

### 对账整个不要

`inst_relate` / `tubi_relate` 一行同时承载**属性面**（`in`/`out`/`generic`/`anc`/`dbnum`/`solid`/`has_cata_neg`，由 `pdms_inst.rs` 写；`dt` 那一列 2026-09-14 已退役，见本文末）和**模型面**（`direct_model`/`booled_id`/`insts_flat`/mesh 指针，由 `model_db_adapter.rs` 写）。看上去模型管道只拥有行上一个子对象、删不掉整行，所以必须先读出确切 id。

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
| 分组键 | `gen_root`；**只取 ZONE 层，不上溯 SITE**（2026-09-14 用户定：SITE 只拥有 ZONE，几何都在某个 ZONE 之下；上溯会造出一个罩住别的 ZONE 根的 SITE 根，它的整片删会抹掉邻居刚落一半的行）。库里没有 ZONE 按错误上抛，不静默零根 |
| 对账读 | **整个不要**。改成按 `anc CONTAINSANY [<gen_root>…] AND model_sesno != NONE` 谓词删整片 |
| 行上的来源账 | `direct_model` 整个对象删掉，只留一列 `model_sesno`。**本轮保留**，见下 |
| 写入 | 一个 ZONE 一笔事务 |
| 失败 | 整体回退 → 踢掉坏根 → 剩下的重落 |
| 过大 ZONE | 不处理。超过上限就整个 ZONE 失败并报错。拆块与 `publishing` 栅栏一并删除（**已落**）|
| 并发 | 生成侧按根并行（ZONE 根让几何闸利用率 6% → 39%）；**落库单写者**，中间一个队列。`AIOS_MODEL_PERSIST_WRITERS` 定死默认 1、只留作实验开关（> 1 出声告警）——多写者在 kv-mem 与 RocksDB 上实测都更慢，见下「方案评估」 |
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
   - **取根已落 2026-09-14**：`E3dModelService::generation_roots_of` 改成每个 ZONE 一根、不上溯 SITE（按 noun hash 扫文件头挑 ZONE，`elements_with_noun`）；旧的文件顶层口径留作 `top_level_roots_of`，只给 `e3d_persist_smoke --roots file` 对照。烟测默认档即 ZONE 根：7997 → 142 根，生成 17.4 s（文件顶层根 25.8–32 s），回读六项与旧口径逐字相同。
   - **只动了整库取根这一处。** 生产的整库初始化（`model_dbnum_ensure::capture_snapshot`）仍走 `enumerate_generation_roots` 取交付单元根（7997 上 2,277 根），增量的 `resolve_element_generation_root` 同样落到交付单元；`generation_root.rs` 的口径是「WORL / SITE / ZONE 永远不是生成根」。要把 ZONE 根推进生产，得先决定回执 / 覆盖判定 / 增量归根是否一并换到 ZONE 层——那是这一步剩下的决策，不是顺手能改的。
   - **合块 vs 一 ZONE 一串**未定：单写者下 142 个 ZONE 合成 32 笔 17.1 s；各自成链（`persist_dbnum` 逐根）149 笔、提交合计多 5–6 s（22.3 s vs 15.7–17 s），小于 50 条的 43 笔平均 7 ms 一笔，多出来的主要不是小事务本身而是每根一份的整片删 / 盖章 / epoch；逐根那条路收尾 36 s 里另有约 13 s 不在提交上（渲染与每根的收尾工作），待查。

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

## 方案评估（2026-09-14，AMS 7997，从解析侧层级出发）

`e3d_persist_smoke --analyze` 只读文件头拿 `(refno, owner, noun)`（`hierarchy_of`），把内存里的 52,373 条投影记录按祖先链归到各层，算每个分组方案的形状；`--roots zone|site|file` 再把候选方案真跑一遍。7997：170,085 个元素，13 SITE / 142 ZONE，15,642 个唯一 mesh。

| 方案 | 组数 | 最大组（占比） | 中位 / p90 | 事务数（2000 条一笔） | 跨组共享 mesh | 判定 |
| --- | --- | --- | --- | --- | --- | --- |
| A 文件顶层根（旧 `generation_roots_of`） | 1 | 52,373（100%） | — | 27 | 0 | 并行度 0；单根渲成 66 MB 撞硬顶 |
| B SITE | 13 | 34,312（66%） | 806 / 4,301 | 37 | 130 | 一个 SITE 占 2/3，K≥2 之后是 13 s 平台 |
| **C ZONE** | 138（另 4 个 ZONE 无几何） | 5,321（10%） | 106 / 1,076 | 145 | 659（4%） | **选它**：记录 100% 在某个 ZONE 之下，最大组 6.4 MB 三笔装完 |
| D ZONE 下一级（EQUI / STRU / PIPE…） | 2,277 | 1,027（2%） | 10 / 44 | 2,277 | 1,387 | 太碎，每组固定开销压过收益 |
| E 固定 2000 条一组 | 27 | 2,000 | — | 27 | 694 | 不是层级子树，`anc CONTAINSANY` 整片删用不上，排除 |

8000 同口径：3 SITE / 10 ZONE，最大 ZONE 占 73%——小库天然是一个写者的活。

**生成侧**（release，kv-mem 内嵌）：文件顶层根 25.8–32.2 s（闸利用率 6%）→ SITE 根 21.0 s（17.8%）→ **ZONE 根 16.4–18.9 s（39%）**。根的粒度直接决定生成能不能并行；这是这批数据里唯一真正的提速。

**落库侧**（`--batched`，每轮全新空库，回读六项全部逐字一致）：

| 介质 | 写者 | 落库用时 | 事务数 | 提交耗时合计 | 单笔最长 | SQL 字节 / 提交时间 |
| --- | --- | --- | --- | --- | --- | --- |
| kv-mem 内嵌 | 1（512 根合块） | **16.3 s** | 32 | 15.7 s | 0.75 s | 4.25 MB/s |
| kv-mem 内嵌 | 4（一根一写者） | 17.4 s | 149 | 60.8 s | 9.3 s | 1.1 MB/s |
| kv-mem 内嵌 | 8 | 21.9 s | 149 | 118 s | 14.8 s | 0.57 MB/s |
| RocksDB（fork surreal 2.1.4，ws，已建索引） | **1** | **19.6 s** | 32 | 18.6 s | 0.94 s | 3.57 MB/s |
| RocksDB | 4 | 47.5 s | 149 | 166 s | 27.5 s | 0.40 MB/s |
| RocksDB | 8 | 85.2 s | 149 | 626 s | 55.2 s | 0.11 MB/s |

提交耗时合计随写者数线性放大（RocksDB 上超线性），说明库那头的提交是串行的：kv-mem 一把全局写锁，RocksDB 乐观事务在 `spatial_epoch:current` / 共享 `inst_geo` / 房间边上互撞，再被逐笔冲突重试放大。多写者抢不到并行，只多付锁等待与重试。单笔 commit 与 `sql_bytes` 线性（约 0.2–0.25 ms/KB）、从第 1 笔到第 32 笔不随已落行数增长——验收第二条的证据。

**结论**：根粒度取 ZONE；落库单写者、多 ZONE 合块进 2–3 MB 一笔；「ZONE 线程 → channel → 写者」的形状保留，价值在生成与落库**重叠**（两边都是 16–20 s 量级），不在并行提交；再往下压落库时间的杠杆是每条记录渲成的 SQL 字节（1.26 KB/条），不是并发。多写者要有落点只能是多个 datastore / 分片，不在这一轮。原始输出：`out/analyze-7997.txt`、`out/analyze-8000.txt`、`out/measure-7997-{zone-w1,zone-w4,zone-w8,site-w4}.txt`、`out/measure-7997-zone-w{1,4,8}-rocks.txt`（`out/` 不入库）。

## 落库时间花在哪：SQL 文本 vs `.bind()` 参数（2026-09-14，AMS 7997）

上一节末尾那句「再往下压落库时间的杠杆是每条记录渲成的 SQL 字节」**就地作废**。同一批行两条路各写一遍（`e3d_persist_smoke --bindbench`：等距抽 4,000 条记录 = 16,039 行 / 4.52 MB SQL，两臂各写各的全新库、轮间换先后手、取中位数；落完逐行比两个库的内容，三档全部逐行逐字段一致，数值类型也一致）：

| 目标 | 索引 | 文本 `db.query(String)` | 参数 `.bind()` | 服务端逐句合计（文本 / 参数） |
| --- | --- | --- | --- | --- |
| RocksDB（fork surreal 2.1.4，ws） | 7 个 | **1.22 s** | 1.24 s（+1.8%） | 0.70 s / 0.65 s |
| RocksDB | 不建 | 0.99 s | 1.03 s（+3.2%） | 0.55 s / 0.51 s |
| 进程内嵌 kv-mem（不过线、不落盘） | 7 个 | 1.21 s | 1.23 s（+2.7%） | 0.72 s / 0.68 s |

**「不走 SQL 文本、改传结构化参数」这条到此为止：省不出时间。** 服务端确实少了一次解析（0.70 → 0.65 s，−7%），但客户端把省下的又吐了回去——铸值 + revisioned 二进制序列化比「渲文本 + 解析 + 打印回来」还贵一点（未计入逐句的那一截 0.52 → 0.59 s）。

四条读数把杠杆指到了别处：

1. **不是解析。** 客户端 `syn::parse` 4.52 MB 只要 180 ms、`Display` 回字符串 62 ms（`db.query(String)` 每次都做这两下，此前没人量过）；服务端逐句合计才 700 ms，而把解析整个拿掉只值 50 ms。
2. **不是介质。** kv-mem（纯内存、不过线、不落盘）1.21 s vs RocksDB 1.22 s——**落库时间里存储几乎不占**。上一轮「单核 3.9 MB/s 是天花板」得改口径：那是 SurrealDB 逐行写值的速度，与后端是谁无关。
3. **不是字节，是行数与字段个数。** 服务端逐句：`inst_relate` 3,645 行 374 ms（占服务端 53%）、`aabb` 5,574 行 115 ms、`trans` 4,000 行 94 ms、`geo_relate` 1,219 行 72 ms、`inst_info` 1,219 行 18 ms、`inst_geo` 27 行 1 ms。`inst_relate` 每行 16 个字段（平表三件在内），单行成本是 `inst_info` 的 7 倍。
4. **索引占 18%。** 七个索引一个不建：服务端 0.70 → 0.55 s（`inst_relate` 374 → 246 ms）。不是大头，但也不是零。

**下一步要砍的是 `inst_relate` 的字段个数与总行数**（平表三件 `aabb_d` / `world_trans_d` / `insts_flat` 是同一笔事务里别的表已经写过一遍的副本），不是 SQL 文本的长度、不是事务粒度、更不是写者数。

工具：`e3d_persist_smoke --bindbench [--bind-rows N] [--bind-rounds R] [--bind-endpoint <ws://…|mem://>] [--bind-noindex]`。值那条路由 `model_db_adapter::bind_records` 渲，与 `render_records` 逐项对齐；`bound_rows_land_exactly_what_the_rendered_text_lands` 钉住两条路落下去的行一模一样——耗时差只有在这一条绿着的时候才作数。原始输出在 `out/measure-7997-b{b,indbench}-*.txt`（`out/` 不入库）。

## 三个库的初始化分别卡在哪（2026-09-14，AMS 1112 / 7997 / 7999）

口径：每个库一台全新空 RocksDB（先建那七个生产索引）、release、生产形态（write-behind，`--roots zone`，单写者），每档跑两轮取一致值，`READBACK` 六项两轮逐字相同。

| 库 | ZONE 根 | 元素 | 生成 | 落库尾巴 | 进程 wall | 提交合计 / 笔数 | SQL | surreal 平均核 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1112 | 3 | 4,892 | 2.8 s | 1.6 s | **7.1 s** | 1.41 s / 3 | 4.8 MB | 0.22 |
| 7997 | 142 | 52,373 | 19.1 s | 6.6 s | **31.1 s** | 20.90 s / 35 | 59.4 MB | 0.63 |
| 7999 | 25 | 15,968 | 79.5 s | 0.8 s | **83.4 s** | 5.82 s / 12 | 16.0 MB | 0.07 |

**7999 是个反例，值得单列：行数只有 7997 的三分之一，墙钟却是它的 2.7 倍，而这段时间里数据库几乎没事干**（surreal 0.07 核，12 笔提交合计 5.8 s 摊在 73 s 里；落库时间线上 5 根在头 3 s 落完，之后**整整 68 s 一笔提交都没有**）。落库那一侧的账（上两节）在 7999 上完全不是瓶颈。

### 生成侧不再是黑箱：按根记账

给 `spawn_generation_worker` 加了一行 per-root 的账（`model root generated: root=… elements=… evaluate=…ms bake=…ms`，`evaluate` = 取数 + 几何求值，`bake` = 三角化 + SHA-256 + 网格落盘），`e3d_persist_smoke` 的日志过滤放行 `e3d_model_service`。逐根加总与闸的账对得上（7999：逐根 291 s vs 利用率反推 294 s，差 1%）。

| 库 | 生成 CPU 合计 | 其中 bake | 每元素 | 最长一根 | 前 6 根占比 |
| --- | --- | --- | --- | --- | --- |
| 1112 | 5.7 s | 0.5 s | 1.16 ms | 2.0 s | 100%（一共 3 根） |
| 7997 | 112.1 s | 9.9 s | 2.14 ms | 6.6 s | 25% |
| 7999 | 312.7 s | 4.2 s | 19.58 ms | **68.8 s** | **88%** |

7999 最贵的六根（`evaluate` / 元素数）：`24383/94431` 68.7 s / 728、`24383/82663` 54.1 s / 519、`24383/97120` 51.4 s / 119、`24383/92032` 43.9 s / **43**、`24383/92376` 34.6 s / 97、`24383/92879` 21.8 s / 425。**贵不贵与元素多少无关**——43 个元素那根平均一个元素一秒。

三条读数：

1. **7999 的墙钟就是最长那一根。** 68.8 s 的单根 ÷ 81 s 的生成，一根即全程；16 张许可等许可 0 ns、闸利用率只有 24%，因为根本没有 16 根活可派。**根粒度到 ZONE 为止**这条在 7999 上就是天花板：要再快只能把一根 ZONE 内部拆开并行，或者把那几个元素的几何求值本身弄快。
2. **不是烘焙，是求值。** bake（三角化 + 落盘）在 7999 上只有 4.2 s（1.3%），全部成本在 `generate_subtree_with_cache` 里。
3. **求值里还有三分之一不是在算。** 同一轮进程自身 CPU 191 s，而许可持有（= 根在飞）291 s：约 100 s 没有烧在 CPU 上。下一步该量的是这一段在等什么（源库页面换入 / 分配器），按元素记一行账就能把它落到具体构件上。

原始输出：`out/measure-{1112,7997,7999}-init{,2}-wb-rocks.txt`、`out/measure-7999-init3-cpu-rocks.txt`（`out/` 不入库）；跑法 `powershell -File $env:TEMP\zone-eval\run-rocks3.ps1 -Dbnum <n> -Tag <tag> -NoBatched -StoreMode external`。

### 再往里一层：生成侧压根不是几何，是**把目录反复读了几百万遍**

`e3d_persist_smoke` 现在把 e3d-model 的分档计时（`PHASES`，各档**嵌套**不可相加）与 e3d-io 的读取计数（`READS`）各打一行。7999 那一轮（生成 74 s、CPU 312 s）：

- `catalogue` **251.9 s** / 11,552 次（21.8 ms 一次），其中 `cat_prims` **246.9 s**，而真建网格的 `mesh_build` 只有 **0.3 s**——GMRE 图元循环贵在**求值前的读**，不在建体。
- `READS`：点查 **5,369,928** 次、记录解析未命中 112 s、描述符抽取未命中 152 s，合计 **264 s ≈ 生成 CPU 的 85%**；`shared=0`，池级元素缓存根本没开。
- 换算下来 **每个元素读 336 遍**（7997 是 32 遍，1112 是 35 遍）——7999 的「几何贵」全是这个倍数堆出来的。

`DbSet::with_element_cache()`（池级元素缓存，e3d-io 自己的文档里就写着 ams8000 上「每元素真读 145 遍、占生成 88%」）在生成服务这条路上**曾经没打开**：`gen_ams` 那个独立二进制开着，`E3dModelService::build_set` 没开。同机同库开关两态各跑一轮：

| 库 | 生成 | 进程 wall | 点查次数 | 读耗时 | 峰值内存 |
| --- | --- | --- | --- | --- | --- |
| 1112 | 2.4 → **1.45 s** | 7.1 → **5.7 s** | 170,445 → 30,967 | 3.6 → 0.8 s | — |
| 7997 | 18.2 → **7.19 s** | 29.0 → **24.8 s** | 1,703,520 → 176,979 | 88 → 8.2 s | 768 → **1,546 MB** |
| 7999 | 71.0 → **3.84 s** | 74.8 → **10.7 s** | 5,369,928 → 46,168 | 264 → 2.4 s | 388 → **676 MB** |

三个库的 `READBACK` 六项与开关关着时**逐字相同**（7999 `inst_relate=12634 tubi_relate=3334 geo_relate=1265 inst_geo=25`），`cargo test --lib` 1903 passed。

**这把整张优先级表翻了个个儿**：

1. **7999 的 68.8 s 单根不是不可分的几何长尾，是重复读**——拆开一根 ZONE 内部并行只能把它摊到 16 个核上（理论上限约 4×），开缓存直接把总量砍掉 19×，而且不动任何并发结构。
2. **7997 从此是落库瓶颈**：生成 7.2 s、落库尾巴 13.0 s，wall 24.8 s 里大头是提交（总提交量 ~21 s）。上两节那条「砍 `inst_relate` 的字段个数与行数」现在是 7997 的**第一杠杆**，不再是第二。
3. **代价是内存**：缓存不淘汰，7997 峰值从 768 MB 涨到 1.55 GB（1112 是 209 → 353 MB）。整库生成一轮是有界的，长驻进程按库全量读不是。

**已转正（2026-09-14）**：`E3dModelService::build_set` 默认开池级元素缓存，取值走 `options::model_element_cache()`——**默认开**，只留一个环境变量的关位 `AIOS_MODEL_ELEMENT_CACHE=false`（拼错当没设，回默认开；钉在 `the_element_cache_is_on_unless_someone_spells_the_switch_right`）。默认态复测：1112 wall 5.6 s / 生成 1.43 s，7997 wall 25.3 s / 生成 7.18 s，7999 wall 10.7 s / 生成 3.68 s；关位复测 1112 回到生成 2.66 s、点查 170,445、`shared=0`。两态 `READBACK` 六项逐字相同——它换的只是时间与内存，不是产物。

**还没做的那一半**：`build_set` 也给增量规划（`DbSet@S` / `DbSet@T`）用，而 `GENERATION_INPUT_CACHE` 会把生成用的 `DbSet` 按 dbnum 留着（只淘汰同 dbnum 的旧条目）。单库一轮有界，**多库长驻会按 dbnum 累加**——真撞上内存墙的处置今天只有那个关位，淘汰策略（或跑完即弃）还没定。
**2026-09-15**：初始化那条路已跑完即弃（`dbnum_initialize::model_half` 结束就 `forget_generation_input`，7997 上模型半边收工驻留 ~1.35 GB → ~420 MB，峰值不变）；有界化的候选与要先量的数见 `2026-09-14-dual-lane-initialization-dev-plan.md`「元素缓存」一节。

原始输出：`out/measure-{1112,7997,7999}-{phases,elcache2,base-rss,default-on}-rocks.txt`、`out/measure-1112-killswitch-rocks.txt`；关位跑法加 `-NoElementCache`。

### 顺手砍掉的一列：`inst_relate.dt`（2026-09-14）

`dt` 是属性面那一侧写的「元素所在会话的日期」（`fn::ses_date(pe)` 的渲染就绪字面量）。砍它的三条依据：

1. **读侧没人要**：`model_record_reader` 的内存投影那条路本来就给 `date: None`；`2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md` 早就把 `ptset/dt` 一起判成「UI 全链路无消费者，可砍」。
2. **写它要多跑两趟库**：`resolve_inst_meta_on` 为了这一列，每批行都得按 `(dbnum, sesno)` 点查 `ses:[…].date`——当前世界一趟，miss 再回落持久层一趟。
3. **要日期还有现成的路**：`fn::ses_date(in.id)` 读时现算，不必让每一行都驮一份。

落地范围：渲染行去掉 `dt:`；`ResolvedInstMeta` 去掉 `dt_literal`；`resolve_inst_meta_on` 去掉 `ses_fallback` 形参、`sesno` 读取与整段会话日期探针（`SELECT id, owner, dbnum` 少读一列）；读侧 `model_record_reader` 与 `staging::query_valid_insts_on` 的 `dt AS date` 一并摘掉。新钉 `resolving_meta_never_touches_the_session_table`：**一条 `ses` 行都不播**也必须解得出 anc/dbnum（旧解析器会在这里点查）。`cargo test --lib` 1903 passed。

## 验收

- **落库路径上不再把几何身份拉回客户端**：唯一剩下的读是 `slice_sources_on`，只取这一片的来源 refno，供房间清边与空间树摘条目用；行的删除完全走谓词。
- 读与单笔事务耗时都不随已落库量增长：同一个库连续落 N 个 ZONE，第 N 个与第 1 个同量级（现状是 400 ms → 1280 ms）。谓词必须走 `idx_*_anc`（`slice_predicates_are_planned_through_ancestor_indexes` 钉住）。
- 属性面独有的行（`model_sesno = NONE`）在落库前后逐字不变。
- 落库失败后库里不留半个 ZONE；坏根被单独标出，同 ZONE 其余根正常发布。
- 搬家之后两个 ZONE 的 `gen_root` 都是 `stale`，重建一轮后模型齐全。
- `max_concurrent_zones = 1` 时，生成与落库有重叠（不是严格串行等待）。
