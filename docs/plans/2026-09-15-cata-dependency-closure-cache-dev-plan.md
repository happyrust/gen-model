# CATA 依赖闭包进程级缓存：开发计划

> 来源：用户 PLAN（2026-09-15，「CATA 跨库依赖解析缓存优化」）。本文把它对到 `codex/model-projection-cache@917e85893`
> 的代码事实上：哪些今天已经成立、哪些要做、哪些改口径。§9 有逐条对照。
> 状态：**已批准**（2026-09-15，Plannotator gate `approved` + 面板确认；§8 五条全按推荐项定）。
> **S0 已落**（拆项账 + 8000 / 7997 / 1112 基线，见 `docs/evidence/2026-09-15-cata-dependency-cache.md`）——读数改了三处口径：
> 解析在初始化上只值亚秒、贵的是渲染与字节（D6 是主菜）；轮内 28 % 白解单独成片（S1a）；会话保活做（S2b）。

## 0. 一句话

**同一个目录元素在一个进程里只解析一次、只渲一次、只往属性 lane 发一次。** 进程内加一份有界（条目 + 字节双上限、LRU）
的依赖闭包解析缓存：键是「项目 / 目录库 / refno」，条目盖着「文件指纹 + 目录会话号」的库级戳，跨初始化任务与增量窗口保留，
文件身份一变整库作废。RocksDB 仍是唯一事实，不加持久化缓存表；不动模型 lane、不动水位、不动 `pe.dependency` / `pe_owner` 的口径。

## 1. 现状（代码事实，2026-09-15）

### 1.1 闭包只有一条路

```text
seeds(BTreeSet<RefU64>)
  → cata_dependency::resolve_dependency_rows(project, design_dbnum, seeds)
      → InMemoryCataLocator::build_for_project(project)      ← 已有进程级缓存 LOCATOR_CACHE（按 DEPENDENCY_MANIFEST_VERSION 失效）
      → CataClosureResolver::new(locator, cfg).with_retain_attmaps(true).resolve()
            每轮按 dbnum 聚合 frontier
            → 每库一次 OnDemandDbSession::open（ReadOnlyEngine::open；会话只活在这一个 resolver 里）
            → parse_refnos_with_session 逐 refno parse_element（B+ 树下探）
            → outbound / owner / children 扩边 → visited 去重
      → take_attmaps() → element_from_attmap → CataDependencyRows { by_dbnum: dbnum → Vec<(sesno, EleData)>, manifest }
  → render_dependency_transactions(rows, uda_keys, Fresh)    ← element_batch，INSERT IGNORE，pe.dependency = true
  → 属性 lane（基线）/ pe_batch::execute_on（窗口 / 模型根 / 兜底）
```

`sesno` = `catalogue_sesno(locator, dbnum)` = `FrozenSource::capture(path).target_sesno()`，每库每次 resolve 读一次文件头。

### 1.2 四个调用点

| 路 | 入口 | 频次 | 已有的去重 |
| --- | --- | --- | --- |
| 基线收尾（初始化，ADR-077） | `baseline_ingest::persist_dependency_closure` → 行经 `WriteJob` 进属性 lane | 每个 DESI 文件一次（seed 跨块合并） | 无——每个 dbnum 各自一份闭包，1112 / 7997 / 8000 共享的目录元素各解一次 |
| 数据窗口 | `increment_pipeline::apply_one`（`StageTimings.cata_dependency` 计时）→ 窗口工件 → `window_landing` 的 `persist_resolved_rows_on` | 每窗一次 | 无——每窗从头 open + parse + render |
| 模型根发布 | `model_refresh` → `persist_for_design_subtree` | 每根一次 | `CataSeedMemo`：同一数据窗口内老 seed 不再解（2026-09-10：7997 2,469 根重解重写 387,132 行 / 323.7 s 的那一半） |
| 惰性兜底 / D7 前移刷新 | `cata_closure::ensure_cata_refnos_parsed`（只剩 `staging::preload`）/ `refresh_moved_catalogues_on`（`Replace`） | 少 | 无 |

### 1.3 什么今天已经不重复

- **一次 resolve 内**：`visited` 去重、`manifest.by_dbnum` 是 `BTreeSet` → 同一目录元素在一次闭包里只解一次、只渲一行，
  **单笔属性事务里同一元素不会出现两次**。PLAN 的「事务渲染阶段按 refno 去重」今天已成立，只差一枚钉。
- **定位器**：`LOCATOR_CACHE` 已按项目缓存 `InMemoryCataLocator`；`ref0` 扫描另有磁盘指纹缓存（`Ref0IndexCache`）。
  `cata_dependency.rs` 里「每一次调用还都重建一遍 `InMemoryCataLocator`」那句是 2026-09-10 的旧账，S6 顺手改掉。

### 1.4 什么在重复

**跨调用**（跨窗口、跨 dbnum 初始化、跨根）同一目录元素反复：open 会话 → `parse_element` → `element_from_attmap` →
`ElementRows::render` → 一笔 SQL 进 lane。DB 端 `INSERT IGNORE` 命中零成本，但解析 CPU、渲染、SQL 字节、lane 事务数照付——
PLAN 要减的四样正是这四步。

### 1.5 数字（S0 取证 2026-09-15，详见 evidence）

| 库 | seeds → 闭包行 / 库 | resolve（open + parse） | `parse_element` 请求 / 成功 | render | SQL | 占属性 lane 字节 |
| --- | --- | --- | --- | --- | --- | --- |
| 8000 | 158 → 14,021 / 7 | 527 ms（178 + 339） | 15,489 / 14,021 | 1,733 ms | 10.3 MB | **62 %**（16.8 MB） |
| 7997 | 705 → 15,500 / 10 | 777 ms（300 + 459） | 21,592 / 15,500 | 5,188 ms | 17.6 MB | 13 %（131.6 MB） |
| 1112 | 8 → 162 / 2 | 34 ms | 229 / 162 | 11 ms | 0.13 MB | — |

- **解析便宜、字节贵**：属性 lane ≈ 0.8 MB/s，CATA 块在 lane 上 7997 ≈ 22 s、8000 ≈ 13 s（小库初始化墙钟的一大半）；
  resolve 只有 0.5–0.8 s。省字节只能靠「不发」（D6），解析缓存一个字节都省不了。
- **轮内白解 28 %**（7997：21,592 次请求 vs 15,500 成功）：`frontier` 不去重，同一 refno 被多个父元素指到就解几遍。一行去重（S1a）。
- **跨库复用只值 4.5 %**：1112 → 8000 → 7997 同进程顺序初始化，7997 首见 14,801 / 15,500。缓存的用处在初始化**之后**的窗口与模型根。
- **open 占 resolve 39 %**（≈ 30 ms / 库）→ D5 会话保活做（Q4 的 10 % 门过了）。
- 模型阶段（2026-09-10）：2,469 次逐根调用 387,132 行 / 323.7 s——已由 `CataSeedMemo` 压掉窗内那一半，**跨窗仍从头来**。
- **仍缺**：窗口路 / 模型根路的真实数字（要 TTY + 隔离工程 + live 服务的 family 流程），S5 A/B 时量；evidence 里有按单位成本的估算。

### 1.6 历史：这不是复活 `CataDepCache`

2026-09-07（ADR-064 Q2）删掉的 `CataDepCache` / `preload_cata_for_roots` 是惰性兜底路上「哪些 refno 已解析进库」的**存在性**缓存，
绕着生成路走、有自己的落库口。本计划缓存的是**解析产物**（属性表 + 成员 + 出向边），挂在 `CataClosureResolver` 里；
落库出口仍只有 `cata_dependency` 一个，`pe.dependency = true` 只在那里打（ADR-064 E4 不变）。

## 2. 目标与非目标

**目标**

1. 跨调用同一目录元素不再重复 open / parse：缓存命中的 refno 不碰文件。
2. 已在本进程成功落库、且目录文件没动过的元素，不再重渲、不再往 lane 发：属性 lane 上 CATA 依赖块的 SQL 字节与事务数下降。
3. 缓存开 / 关两态**落库结果逐行一致**（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`、`cata_dependency_state.sesno`、水位、`gen_root`、`model_verdict`）。
4. 内存有上限、可观测、可一键关掉（配置 `0`）；缓存永远不是任务失败的原因。

**非目标**

- 不新增 RocksDB 表 / 持久化缓存格式；进程重启缓存消失。
- 不动模型 lane、根粒度、任务调度、`ModelWriteBehind`、e3d-io 的 `DbSet` 元素缓存（那是 dual-lane plan「有界化待拍板」的另一件事）。
- 不改闭包判据（`CATALOGUE_CHAIN_ATTRIBUTES`、`precise()` 白名单、只收 CATA）——缓存只省重复劳动，闭包结果集必须与今天位相同。
- 不改 `INSERT IGNORE` / `UPSERT` 语义、`pe_owner` 方向、`dbnum_watermark`；`cata_dependency_state` 仍不是水位。
- 不给 `/health` 加字段；不加 env 开关；不做 refno 级 singleflight。

## 3. 设计决策

### D1 缓存住哪、存什么

新文件 `src/data_interface/cata_closure_cache.rs`（`cata_closure.rs` 已 1,780 行，不再往里塞），进程级 `static`：

```rust
pub(crate) struct CachedCataElement {   // 一个目录元素解析一次得到的全部东西
    parsed: ParsedCataEle,               // refno / owner / noun / noun_name / outbound / children —— BFS 扩边要的
    att: NamedAttrMap,                   // element_from_attmap → ElementRows::render 要的完整属性表
    approx_bytes: usize,                 // 入表时估一次
}
key   = (project: Arc<str>, cata_dbnum: u32, refno: RefU64)
stamp = CatalogueIdentity { fingerprint: String /* size:mtime(|parent) */, target_sesno: u32 }   // 按库存一份，不逐条存
value = Arc<CachedCataElement>
```

值用 `Arc` 共享；命中路 `element_from_attmap` 需要 owned `NamedAttrMap`——先 `clone` 一次（一次 clone 远比一次文件解析便宜；
S0 量过再决定要不要把渲染改成借用）。

缓存的是元素的**原始事实**（属性、成员、出向边）；扩边规则（容器白名单 / 排除库 / owner 链 / 只收 CATA）仍在 `resolve()` 里按
**本次** `cfg` 施加——四个调用点 `cfg` 的差别不会污染缓存，也不会互相借到不该借的边。

### D2 键与身份：戳相等才命中，不做「更早 sesno 兼容」

每库一份 `CatalogueIdentity`，在一次 `resolve()` 里**每库只算一次**（`locator_fingerprint(path)` + `FrozenSource::capture(path).target_sesno()`，
与 `catalogue_sesno` 同源），不逐 refno 算。命中条件：条目所属库的当前身份 == 条目入表时的身份。身份一变，该库全部条目整体作废
（库级 generation 计数：旧 generation 的条目下一次碰到时按 miss 覆盖，淘汰时清账）。

与 PLAN 的差异：PLAN 想「目标 sesno 变化时读相同或更早 sesno 的兼容项」。这里**不做**——某元素在会话 N 与 M 之间有没有变要看差分窗口
（`cata_cascade_window`），那正是 D7 前移刷新的活；目录文件很少动，动了整库重解一次，比在缓存里再养一套会话兼容判据便宜、也不会错。

项目 / dbnum / 路径变化天然不复用：键里有项目与 dbnum，路径在指纹里。

### D3 有界：条目 + 字节双上限，LRU，不加新依赖

仓里没有 `lru` crate，不为此加：`HashMap<Key, Entry>` + `BTreeMap<tick, Key>` 的最近使用序（O(log n)），命中刷 tick。
`approx_bytes` = 键 + `NamedAttrMap` 键值长度之和 + (`outbound` + `children`) × 8 + 固定开销——**是估算**，上限管的是量级不是精确字节。
插入后超任一上限就从最旧淘汰到线内；淘汰、作废各记一个计数。`max_entries == 0 || max_bytes == 0` = 关：查询直接 miss、不写入、不上锁。

### D4 接线点只有一个：`CataClosureResolver::resolve()`

每轮每库拿到 `to_parse` 后：

1. **先查缓存**（同库身份戳相同）→ 命中的直接进 `by_dbnum`、需要时进 `attmaps`、按 `parsed` 扩边。扩边抽成一个 `expand(&ParsedCataEle)`，
   命中路与解析路共用同一段——不许出现两份扩边逻辑（源码钉）。命中不再记 `parse_error::note_element_success`（上次解析已记）。
2. **未命中的**照今天走 `parse_refnos_with_session`；`Ok(Some)` 写入缓存。`Ok(None)`（库里没有）、`Err`（解析失败，今天按 skip 记 missing）、
   无文件 / 无 db_type / 打不开会话——四种缺失与错误一律不写。
3. **并发**：每库一把 `tokio::sync::Mutex` 包住「miss → parse → insert」；同库并发的 resolver 排队、异库并行。排到的先重查缓存再解析，
   重复解析压到「同一批同时 miss」这一种情形。PLAN 的「其他请求等待并复用结果」在库级成立，不做 refno 级。

四条路（基线 / 窗口 / 模型根 / 兜底与 D7 刷新）都经这一个 `resolve()`，接一处全都吃到。D7 刷新走 `Replace`，命中的元素照样重渲
（它要的就是重渲），只省解析。

### D5 会话保活（S0 定：做）

`OnDemandDbSession` 今天活在一个 resolver 里，resolve 完就关；下一窗再 `ReadOnlyEngine::open` 十次。S0 量出 open 占 resolve **39 %**
（≈ 30 ms / 库），过了 Q4 的 10 % 门：加一个按 `(dbnum, fingerprint)` 键的小会话池（上限 16、指纹失配即关、`parse_element` 要 `&mut` →
池给独占借用），S2b 落。它只对**部分命中**的窗口有用——缓存全命中时根本不开会话。

### D6 已落库去重：`PersistedDependencyMemo`（PLAN 目标 2 / 4 的真正来源）

解析缓存只省 CPU；要省 lane 上的字节与事务，得知道「这行已经在库里」。沿 `CataSeedMemo` 的纪律（ADR-071 D1：任务是这个库唯一的写者，
**它知道自己写了什么**），加一份进程级 `(cata_dbnum, refno) → sesno`：

- **记**：只在 `persist_resolved_rows_on` 里那一库的事务全部 `execute_on` 成功之后记（与 `remember_seeds` 同一条「写成功之后」纪律，
  钉 `…_records_only_after_every_transaction_of_that_dbnum_succeeded`）。基线 lane 是异步单写者，成功要等 drain——**第一阶段基线路只查不记**
  （Q3，已定：第一阶段不记，S5 看数字再议）。
- **用**：`render_dependency_transactions` 之前把 memo 里「同库、同 sesno 已落」的元素从 `rows.by_dbnum` 摘掉；闭包账（`manifest` / `missing`）
  不摘，回执另加 `skipped_persisted`。摘光的库不发 `render_dependency_state`（`rows` 是量级账，不加零）。
- **作废**：D7 前移刷新（`refresh_moved_catalogues_on` 对那一库 Replace / 墓碑之后整库清——sesno 变了本来也不会命中，清是不留死条目）；
  `wipe_dbnum_for_reinit` 清全部；进程重启自然消失。`invalidate_cata_seed_memo()` 今天只有测试在调——与本 memo 的作废并成一个
  `forget_persisted_catalogue_rows(scope)`，在上述两处真接上。
- **与 `Fresh` 的关系**：`Fresh` = `INSERT IGNORE`，行在（任何 sesno）就是 no-op；memo 只是把这个 no-op 提前到进程里，语义不变。
  memo 错在「以为在、其实不在」这一种：只在写成功后记 + 两处作废，把它收口到「别的进程动了库」——那本来就在 ADR-071 D1 之外。

与解析缓存共用开关：配置为 `0` 时两者都关（Q2，已定：共用）。

### D7 观测

`CataDependencyReport` 加 `cache_hits / cache_misses / parsed / skipped_persisted / sql_bytes / resolve_ms`
（`serde(default, skip_serializing_if = "is_zero")`——回执与 `/health` 已经在露 `CataDependencyReport` 的地方自然带上，**不另加字段**）；
`[cata_dependency]` 那行 println 补 `hit=… miss=… skipped=… bytes=…`；窗口 `StageTimings.cata_dependency` 不变；
缓存自身的淘汰 / 作废 / 当前条目数与字节数在每次淘汰发生时 `log::info!` 一行（同一 resolve 内合并成一行，不刷屏）。

### D8 配置 —— **取消**（2026-09-15 用户拍板：「不需要加额外的开关，保持默认」）

上限固定在 `CacheLimits::default()`（200,000 条 / 256 MiB，Q1 的固定值即最终值），**不进 `options.rs`、不进 `DbOption.toml`、不加 env 开关**。
`CacheLimits` 的「任一为 `0` 即关」只是留给单测的内部能力，不是运行时开关。后果：S5 的「关」态没有运行时办法跑出来，
对照数取 S0 / S1a 的同机实测（evidence「基线」「S1a」两节；缓存接线前的代码就是「关」态）。

原案（未实施，留作记录）：`options.rs` ext 字段 `cata_dependency_cache_max_entries`（200_000）/ `cata_dependency_cache_max_bytes`
（256 MiB），`0` 关，`DbOption.toml` 两行注释。

## 4. 切片

| # | 内容 | 落点 | 验收 |
| --- | --- | --- | --- |
| S0 ✅ | **取证**：`resolve()` 拆项账（`ClosureAccount`：open / parse / expand、请求 / 成功 / 首见、会话数）、`resolve_dependency_rows` identity / 装行、回执 render / SQL 字节（只加账，不改行为）；8000 / 7997 / 1112 各一轮 + 三库同进程一轮（`%TEMP%\cata-cache-eval\run-init-cata.ps1`，8011 端口） | `cata_closure.rs`、`cata_dependency.rs`、`baseline_ingest.rs`、`window_landing/artifact.rs`、evidence「基线」段 | 已落：基线路拆项表 + 跨库重叠 + 轮内白解；窗口路 / 模型根路留 S5（要 TTY + live 服务） |
| S1a ✅ | **轮内去重**：`to_parse` 经 `round_parse_list` 去掉本轮重复 refno（`frontier` 同一 refno 被多个父元素推进来），行为不变 | `cata_closure.rs::resolve()` | 已落：单测 + 源码钉；7997 复测 `parse_element` **21,592 → 15,502**，闭包 / `COUNTS` / 水位逐字相同（时间只省 30 ms——重复命中的是热页，见 evidence） |
| S1 ✅ | **缓存核心**：`cata_closure_cache.rs`——键 `(project_id, dbnum, refno)` / 库级身份戳 / tick-LRU / 条目 + 字节双上限 / 计数 / `0` 关；纯数据结构，全局一份 `closure_cache()` | 新文件 + `mod.rs` | 已落：6 条单测（同键一份；项目 / 库 / 指纹 / sesno 任一不同即 miss；LRU 双上限、单条超上限不进；身份变了整库作废不伤邻库；`0` 关连账都不记；字节估算随负载与边表增长）。S2 之前它还没有调用方 |
| S2 ✅ | **接进 `resolve()`**（D4）：查 → 命中扩边 → miss 解析 → 成功写入；每库一把闸；扩边抽 `expand_into()`；S0 的首见表 `SEEN_PARSED` 由缓存的命中账取代、删掉 | `cata_closure.rs` | 已落：4 条单测 + 2 条真库 live 钉。ams8000 同 seeds 连解两次：**冷 0 命中 / 13,998 未命中 · 456 ms → 热 13,995 命中 / 3 未命中 · 57 ms（8×）**，闭包逐位相同；重来的 3 个是库里根本没有的（`Ok(None)` 按设计不记）。对拍 oracle 在**热缓存**下重跑 `unexplained=0`、closure 13,995 与冷跑逐字相同 |
| S2b | **会话保活**（D5）：`(dbnum, fingerprint)` 键的小会话池，上限 16，失配即关 | `cata_closure.rs` / `on_demand_db.rs` | 单测：同库连续两次 resolve 只 `open` 一次；指纹变了重开；池满按最旧关 |
| S3 ✅ | **已落库去重**（D6）：memo、`persist_resolved_rows_on` 成功后记、渲染前摘、两处作废接线；`invalidate_cata_seed_memo` 并入 | `cata_dependency.rs`、`fast_delete.rs`（wipe 处调一句） | 已落：5 条单测。`mem://` 同一份 rows 连落两次——第二次 **0 笔事务 / 0 字节**，`pe` 行数与 `cata_dependency_state.rows` 都不动；换 sesno 就整库重发；`execute_on` Err 那一轮一行不记、重试仍是全量。落库那一段抽成 `land_dependency_rows_on`（前移刷新要真定位器，分开才测得了）。真库 1112 + 8000（`--with-model`）：`pe` / `inst_relate` / `gen_root` / 水位 / 两条 lane 的 tx 与字节**与 S2 那轮逐字相同**，`已落库跳过 0`——基线路第一阶段只查不记，本来就不该有区别 |
| S4 ✂ | **配置**（D8）**取消**（2026-09-15 用户拍板不加开关，保持默认）；**观测**（D7）已随 S0 / S2 / S3 落：`CataDependencyReport` 的 `resolve_ms / render_ms / sql_bytes / parse_requests / parsed / cache_hits / cache_misses / skipped_persisted`（零值不序列化）、`[cata_dependency]` / `[cata_closure]` 两行账。**剩**：淘汰 / 作废发生时那一行 `log::info!`（今天上限远没到顶，没触发过） | `cata_closure.rs`（那一行日志） | 回执零值字段不序列化已由 `serde(skip_serializing_if)` 钉；无三态单测（没有配置了） |
| S5 | **实测**：缓存**开态**实测对照 S0 / S1a 的**关态**（没有运行时开关，关 = 接线前的代码，同机同日）——7997 主基准（初始化总耗时不回退、`parse_element` 调用次数、属性 lane 上 CATA 块字节 / 事务数、峰值 RSS）；8000 与 1112 小库 `COUNTS` / `READBACK` / `gen_root` / 水位 与 S0 逐字相同；`live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure` 缓存开着再跑一遍仍 `unexplained = 0`（S2 已做）。台子 `D:\cata-cache-eval\run-ab.ps1`（采峰值 RSS，读回 COUNTS / DEPSTATE / DEPROWS，出 `ab-<Tag>-summary.md`）。录制窗口回放留窗口路（要 TTY） | `docs/evidence/2026-09-15-cata-dependency-cache.md` | 数据进本文 §5.3 表 |
| S6 | **文档**：CONTEXT.md 词条「依赖闭包缓存 (Dependency Closure Cache)」；ADR-064 D5 补一句「解析产物可在进程内缓存，落库出口不变」；`cata_dependency.rs` 那句旧账改掉；本文状态 | docs、注释 | 用户过目 |

S1a 独立、最先落（它的收益要单独量，别混进缓存的账）；S1 独立；S2 依赖 S1；S2b 依赖 S2；S3 与 S1 / S2 无关（可并行）；
S4 依赖 S2 + S3；S5 依赖 S4。

## 5. 验证与验收

### 5.1 单测（名字即判据）

S1 ✅（`cata_closure_cache.rs`）：

- `the_cache_keeps_one_entry_per_project_dbnum_refno`
- `a_different_project_dbnum_fingerprint_or_sesno_is_a_miss`
- `eviction_is_lru_and_honours_both_the_entry_and_the_byte_limit`
- `a_changed_catalogue_identity_retires_the_whole_dbnum_but_not_its_neighbours`
- `zero_limits_switch_the_cache_off`
- `approx_bytes_grows_with_attribute_payload_and_edges`

S2 ✅（`cata_closure.rs` / `cata_dependency.rs`）：

- `a_cached_element_expands_and_lands_without_opening_the_catalogue_file`（命中路：不开会话、照样扩边、照样交属性表）
- `a_catalogue_that_cannot_be_opened_leaves_nothing_behind_to_hit`（失败不进缓存，连解两次都是 miss）
- `one_gate_per_catalogue_queues_the_same_dbnum_and_lets_the_others_through`
- `the_hit_path_and_the_parse_path_share_one_expansion_behind_one_gate`（源码钉：一处扩边；过闸 → 重查 → 解析的次序）
- `live_a_second_resolve_of_the_same_seeds_comes_from_the_closure_cache`（真库：桩身份证不了「真文件的指纹与会话号两次取值相等」）

S3 ✅（`cata_dependency.rs`）：

- `rows_that_are_already_in_the_database_are_not_sent_a_second_time`（`mem://`：第二次 0 笔事务 / 0 字节，`pe` 与状态行都不动；换 sesno 整库重发）
- `the_persisted_memo_records_only_after_every_transaction_of_that_dbnum_succeeded`（写炸的那一轮一行不记；源码钉：记忆排在 `execute_on` 之后）
- `persisted_rows_are_dropped_from_rendering_but_not_from_the_closure_account`（摘光的库整个不出现——不渲事务也不发状态行；`elements` / `missing` 不摘）
- `a_moved_catalogue_and_a_wipe_both_forget_the_memo`（那一库 / 全部两个范围 + 两处接线的源码钉）
- `a_dependency_transaction_never_lists_one_refno_twice`（钉今天已成立的事实）

三条碰进程级记忆的单测共用一把 `MEMO_TESTS` 锁串行跑——两份记忆都是全局的，并行会互相清账。

### 5.2 回归

- 现有 34 条 `cata_closure` / `cata_dependency` 测试与 `element_batch`、`ATT_UDA`、`pe_owner` 方向测试全过；
  `cargo test --lib` 不退步（交接时 2065 passed / 108 ignored，以动手当天的基线为准）。
  S2 落地时：**2076 passed / 0 failed / 109 ignored**（S0 起累计 +11 条；新增的第 109 条 ignored 是真库 live 钉）。
- `all_three_write_points_persist_catalogue_dependencies` 源码钉不改口径（写入点一个不变）。

### 5.3 性能门（S5，每轮全新 rocksdb；「关」= 接线前的代码在同机同日的实测，没有运行时开关——D8 取消）

| 库 | 指标 | 关（S0 / S1a 实测） | 开 | 门 |
| --- | --- | --- | --- | --- |
| 7997 | 初始化总耗时 | 162.7 s（同机同日；ADR-077 evidence 150.5 s） | — | 不回退（≤ 关 + 2 %）——对解析缓存本身没区分度（省 < 1 s），只是护栏 |
| 7997 | `parse_element` 调用次数（基线路） | 21,592（成功 15,500） | — | S1a 后 ≈ 15,504；缓存 + 三库同进程后第二个 DESI 库起接近 0 |
| 7997 | CATA 块 SQL 字节 / render | 17.6 MB / 5.2 s | — | 基线路第一阶段只查不记，这一格不动；降幅看窗口路 |
| 7997 | 峰值 RSS | 1,543 MB | — | 增量 ≤ 缓存字节上限 + 10 % |
| 8000 | `parse_element` / CATA 块 SQL | 15,489 / 10.3 MB（属性 lane 的 62 %） | — | 同上 |
| 8000 / 1112 | `COUNTS` / `READBACK` / `gen_root` / 水位 / `model_verdict` | 见 evidence | — | 两态逐字相同 |
| 增量窗口（真窗口，S5 量） | 闭包 resolve 耗时 / open 次数 / CATA 块 SQL 字节 / 事务数 | S5 | — | 第二窗起 open ≈ 0、parse ≈ 0、已落库元素不再发（D6）；渲出 SQL 两态逐行 diff 为空 |

命中率低于预期或字节到顶：只淘汰、不报错、任务照常成功——缓存永远不是失败原因（S1 的 `0` 态与淘汰单测钉这一点）。

## 6. 不做（本轮）

- e3d-io `DbSet` 元素缓存的有界化（dual-lane plan「待拍板」那件）。
- ZONE 根、模型 lane、`ModelWriteBehind`、根粒度、任务调度。
- 持久化缓存（RocksDB 表 / 磁盘文件）。`Ref0IndexCache` 那种磁盘 json 不扩到解析产物。
- `/health` 新字段、env 开关、refno 级 singleflight、跨进程共享。
- 闭包判据、白名单、`CATALOGUE_CHAIN_ATTRIBUTES` 一个字不动。

## 7. 风险与取舍

- **指纹是 size + mtime 毫秒**：同毫秒内覆盖同大小文件会漏检——与 `Ref0IndexCache` 今天同一风险；戳里另有文件头的 `target_sesno`，
  两者都对上才命中，比只看 mtime 强一档。不在本计划里再加强。
- **memo「以为在、其实不在」**：只在写成功后记 + 两处作废；剩下的是别的进程动库——ADR-071 D1 之外。配置 `0` 即回到今天的行为。
- **字节估算不准**：上限是量级护栏；S5 用峰值 RSS 增量对一次估算偏差，偏差 > 2× 就修估算函数。
- **`NamedAttrMap` clone 成本**：命中路 clone 一次；S0 若量出可见，改 `ElementRows::render` 收借用。
- **锁粒度**：库级锁下同库并发 resolver 串行——1112 / 7997 / 8000 若并行初始化会在目录库上排队；它们共享同一批目录元素，
  排到的那一个大概率随后全命中。

## 8. 已定（2026-09-15，五条全按推荐项）

- **Q1 已定**：解析缓存默认上限取**固定值**，暂定 200,000 条 / 256 MiB，S0 量完按「整份 AMS 目录闭包 × 2」的量级校一次再落进 D8。
  → 2026-09-15 落定：固定值即最终值（三库闭包并集 ≈ 29,000 个元素，这是 × 6 的量级），**不进配置**（D8 取消）。
- **Q2 已定**：D6 的 memo 与解析缓存**共用一个开关**（`max_entries = 0` 或 `max_bytes = 0` 两者都关）——同一件事的两半。
  → 随 D8 取消：两者都**没有运行时开关**；`CacheLimits::OFF` 只给单测用。
- **Q3 已定**：基线 lane **第一阶段不记** memo；初始化后第一批窗口会把整份闭包再发一遍（7997 是 15,500 行、一次性），S5 看数字再议要不要在 `dbnum_initialize` drain 成功后补记。
- **Q4 已定**：D5 会话保活只在 S0 量出 open 占窗口路闭包耗时 **> 10 %** 时才做。
- **Q5 已定**：**不开新 ADR**——这是 ADR-064 E4 / ADR-071 D1 既有不变量下的实现优化；ADR-064 D5 补一句 + CONTEXT.md 一个词条（S6）。

## 9. 与 PLAN 的逐条对照

| PLAN 说 | 代码事实 | 本计划 |
| --- | --- | --- |
| 在 `cata_closure.rs` 加进程级缓存管理器 | 文件已 1,780 行 | 新文件 `cata_closure_cache.rs`，接线在 `cata_closure.rs::resolve()`（D1 / D4） |
| 键 `project / dbnum / source_fingerprint / target_sesno / refno` | 指纹 = `locator_fingerprint`（size:mtime，含 extract 父文件）；sesno = `FrozenSource::capture().target_sesno()` | 键 `(project, dbnum, refno)`，`(fingerprint, sesno)` 做库级戳（D2）——同一信息，少存 N 份 |
| 值：已验证的解析结果 + 渲染依赖行所需最小属性 | 渲染要完整 `NamedAttrMap` + children；扩边要 `ParsedCataEle` | 两者合成 `CachedCataElement`（D1） |
| `Arc` 共享 | — | 值 `Arc`；命中渲染 clone 一次，S0 量（D1） |
| 有界 LRU，条目 + 字节双上限 | 仓里没有 `lru` crate | 自写 tick-LRU，不加依赖（D3） |
| 命中 / 未命中 / 淘汰 / 失效 / 耗时 / 节省字节进现有诊断日志 | `CataDependencyReport` + `[cata_dependency]` println + `StageTimings` | D7，不给 `/health` 另加字段 |
| BFS 取出 refno 后先查缓存 | `resolve()` 每轮每库 `to_parse` | D4 步 1 |
| 命中不重开文件、不重解 | 会话只在 miss 时才 open | D4；会话保活另议（D5） |
| `missing` / 冲突 / 读错 / 不完整闭包不进缓存 | `Ok(None)` / `Err` / 无文件 / 无 db_type 四种 | D4 步 2 |
| `baseline_ingest` / `increment_pipeline` 复用同一缓存 | 两者都经 `resolve_dependency_rows` → 同一个 `resolve()` | 自动吃到，不必各自接线 |
| 初始化 SYS / DICT / CATA 预热可被 1112 / 7997 / 8000 复用 | 基线只对 DESI 起闭包；SYS / DICT 没有目录链 | 复用发生在第一个 DESI 库解过的目录元素上；不做「预热」动作 |
| 事务渲染按 refno 去重 | `manifest.by_dbnum` 是 `BTreeSet` → 今天已成立 | 只加一枚钉（§5.1 倒数第二条） |
| 保留 `INSERT IGNORE` / `UPSERT`、水位、依赖标记、`pe_owner` 方向 | — | 非目标里写死 |
| 指纹变 → 清该库 | — | D2 库级作废 |
| sesno 变 → 只读相同或更早的兼容项 | 兼容性要看差分窗口 | **不做**，戳相等才命中（D2 说明） |
| 项目 / dbnum / 路径变 → 不跨用 | — | 键 + 指纹天然保证 |
| `options.rs` 两个键，`0` 关 | ext 字段 + accessor 的既有样式 | D8 |
| 并发同 refno 只解一次、其他等待复用 | — | 库级锁（D4 步 3），不做 refno 级 |
| 录制窗口回放两态逐行一致 | `scripts/increment_matrix/run_window.py` | S5 |
| 7997 主基准、8000 / 1112 回归 | ADR-077 harness 在 `%TEMP%\adr077-eval` | S0 / S5 |
| 命中率低 / 内存超限 → 只淘汰、任务成功 | — | D3 + §5.3 最后一句 |
| （PLAN 未说）属性 lane 字节 / 事务数**怎么才会**降 | 解析缓存不减字节；只有不发才减 | **D6 已落库去重**是新增的那一块，`CataSeedMemo` 是它的先例 |
