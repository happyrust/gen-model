# ADR-064：元素数据行只有「批量先删后插」一种写法；CATA 按 DESI 依赖入库；模型取数源按 dbnum 就绪度选

状态：**已接受**（2026-09-06 用户拍板：「写入 rocksdb 尽量批量、先提前删除、然后批量写入，不要单独 upsert / update；
写回的数据也要按需保存依赖的 CATA」；「未初始化的 dbnum 查模型时 rocksdb 没准备好就用 API 供数给 plant-ui，已初始化的
以 rocksdb 查询接口为准」；评审批注「需要保存，按 DESI 解析的依赖的 CATA，按依赖保存」。计划
`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` Plannotator 三轮 `approved`，共识 **d-29**）；
**已实施** P0-3 / P1 / P2 / P3 / P4 / P5（服务端）/ P6（2026-09-06，见计划 §9）；U1（plant-ui）2026-09-07 已落地
（plant-ui ADR-0025）；**未 live**（真库验收门留给用户终端）。
日期：2026-09-06
关联：
- **承接** ADR-060「模型行 id 就是记录 id；写入只有先删后插，没有 UPDATE」——模型几何行的口径升为**全库口径**；
  ADR-056（稳态窗口直写、顺序纪律 persist → invalidate → reverse_index → 窗口语句批 → finalize）不动。
- **修订** ADR-058 D1 / S3「CATA 不入持久层；任何把 CATA 元素写进 `pe` / `ATT_*` 的路径都是边界破坏」→
  「CATA **只按 DESI 依赖入库**，`pe.dependency = true`，整库 CATA 基线仍不做」；ADR-058 实施约束 4 里 `cata_closure.rs`
  的闭包解析器**保留并收窄**（容器白名单），不再整体删。
- **修订** ADR-056 D8 一句：CATA 闭包解析进 Surreal「只服务 `ref_rev` 与 UI」→「服务 `ref_rev`、属性面板与依赖备份」。
- **补齐** ADR-060 D4 的实现：`inst_geo` 的 `ON DUPLICATE KEY UPDATE` 是 D4「没有 UPDATE / UPSERT」的唯一漏网，降为
  `INSERT IGNORE`，模型面从此零 UPDATE（ADR-060 D4 已加实施注记）。
- **不动** ADR-001 / ADR-021（`applied_sesno` 只承诺数据、须有数据支撑）、ADR-025 §7、ADR-054、ADR-057（e3d-model 纯函数；
  本 ADR 没有改 vendor crate）、ADR-060 D2（共享网格只增不删）、ADR-060（模型水位）。
- 证据：`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §1（写口清单）、§9（实施记录，含 D2 修正的现场）；
  `src/test/fork_surreal_compat.rs` 双引擎用例（`dual_array_delete_*` / `dual_insert_ignore_*` / `dual_multi_target_update_*` /
  `dual_delete_then_insert_ignore_block_*` / `dual_pe_rows_replace_in_place_and_keep_their_edges`）。

## 背景

2026-09-06 之前数据面有两种动词：基线是 `INSERT IGNORE INTO … [..]` 批（`versioned_db::baseline`），稳态增量是**逐元素**
`UPSERT pe:{id} CONTENT …` / `UPSERT <NOUN>:{id} CONTENT …` / `UPSERT ATT_UDA:{id}` 加软删 `UPDATE pe SET deleted = true`
（`ele_ops::ElementRows::upsert_statements`），事务切块借的还是暂存期 `mem://` 的 64 KiB / 250 行 / 100 元素。模型几何行已在
ADR-060 走「先删后插 + 每表一条 `INSERT IGNORE`」，两边口径不一。CATA 自 ADR-058 D1 起不入持久层，属性面板对目录件是空的，
`ref_rev` 的 CATA 侧只靠水位之后的补偿队列（`enqueue_cata_ref_rev` → `preload_cata_for_roots`，UPSERT 逐行）。
`/model/records` 按**进程介质**选源、`/model/ensure` 在摄入形态下先等初始化发布 run，一个库正在初始化的那段时间
plant-ui 对它要么等、要么拿到错误。

## 决策

### D1　元素数据行只有一种写法：批量先删后插（`data_interface::element_batch`）

- 一块 = 前置块（`Add` / 墓碑「自己的旧出向边」`DELETE pe:X->pe_owner`，排在所有元素块之前——它动的是别的元素名下的边）
  之后：`DELETE pe_owner:[pe:X, NONE]..=[pe:X, ..]`（每元素子槽位，id 区间）→ `LET $uda_ids = [..]; DELETE $uda_ids`
  → 每个 noun 一组 `LET $del_n = [type::thing(noun, id), ..]; DELETE $del_n`（含换 noun 的旧表、墓碑的基侧 noun）
  → `pe` 行（见 D2）→ 每表一条 `INSERT IGNORE INTO <NOUN> [..]` / `INSERT IGNORE INTO ATT_UDA [..]`
  / `INSERT RELATION IGNORE INTO pe_owner [..]`。
- 两档：`Fresh`（基线 / 父层补缺：不删，只插，已有行赢）与 `Replace`（增量 / 依赖行版本前移）。
- 切块上限 `data_interface::tx_limits`：2 MiB / 5000 写行 / 1000 元素（`DbOption.toml` `data_tx_max_bytes` /
  `data_tx_max_write_rows` / `data_tx_elements`），超限劈半；每笔事务带 pe 统计观测（`dbnum_stats::transaction`）。
- **控制面单行**（`dbnum_watermark`、`gen_root`、`model_update_pending`、`increment_update_attempt`、`dbnum_info_table` /
  `dbnum_stats_state`、`spatial_epoch`、`model_publish_*`、`root_dependency`、`side_effect_pending`、`datacenter_version`、
  `cata_dependency_state`）是状态机，保留 UPSERT / UPDATE 点写，但只许出现在尾事务 / 回执块，绝不逐元素。
  `datacenter_version` 的 Modify 归组成多目标 `UPDATE a, b, c SET status = 'Modify'`；`ref_rev` 全部 referrer 的邻接 DELETE
  在前、整窗口的边攒成 `INSERT RELATION IGNORE`。

### D2　`pe` 行不删，一块一条批量 `INSERT INTO pe [..] ON DUPLICATE KEY UPDATE <全部字段>` 原地整行替换

实测（计划 §9.1）：`pe` 是图顶点，`DELETE pe:X` 会级联删掉挂在 X 身上的边——`pe_owner`（owner 的块重插）、`ref_rev`
（别的 referrer 的窗口维护）、`tubi_relate`（模型面写）——它们不在 X 的块里，X 的 `pe` 一删就静默消失。所以 `pe` 行按同一 id
原地整行替换，字段清单 = `SPdmsElement::gen_sur_json` 的全部字段 + `dependency`（`element_batch::PE_REPLACE_ASSIGNMENTS`，
测试钉「覆盖每个渲出字段」）。**这不是逐元素 UPSERT**：一块一条批量语句。`Fresh` 不受影响。

### D3　被删元素落墓碑

`deleted = true`、`sesno` = 删除落地的会话、noun / owner / name 照基侧（`DbSet@S`，收集器读回）保留；它的 `<NOUN>` / `ATT_UDA`
行与在 owner 名下的边随删除块消失（此前软删把它们留在库里）。基侧读不到退化成最小墓碑并在回执出声。

### D4　基线与增量共用一个渲染器

`versioned_db::baseline::render_baseline_batches` → `element_batch`（`Fresh`），有界队列每条消息 = 一笔事务
（`SenderJsonsData::ElementBatch`）；`PEJson / AttJson / PERelateJson` 退役；`pe_chunk` / `att_chunk` 不再参与切块；单写者不变。
宪法 II 从「行形状只有一份」扩到「语句形状只有一份」。

### D5　CATA 按 DESI 依赖入库（`data_interface::cata_dependency`）

- seed = 本批 DESI 元素上目录链引用属性（`SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU HSRO LSRO`，与 e3d-model
  `reference_reads_guard` 同源、有钉子）的出向 refno；闭包复用 `CataClosureResolver`（`precise()` 容器白名单、owner 链到 WORL、
  只收 CATA、排除本批 DESI）。闭包宽于 e3d-model 实读是允许的，窄就是漏。
- 行经元素批 `Fresh` 入库，`pe.dependency = true`；每个目录库一行 `cata_dependency_state:{dbnum} { sesno, rows }`——
  **不是水位**，不进 `dbnum_watermark`、不参与 ADR-021 判据。
- 三个写入点：数据窗口（排在 DESI 块与水位**之前**，闭包解析出错拦水位，「库不在 MDB / 元素不在」只 warning）、
  基线（流式攒 seed，文件收尾一次闭包，经同一条写者队列）、模型根发布前（`visited` 子树从文件读属性做 seed，独立事务、
  只 warning）。只写内存的库（D7）不落任何东西。
- 版本前移：每个写入点对**本批闭包碰到的**目录库，目录文件最新 > `cata_dependency_state.sesno` 时，先由
  `cata_cascade_window` 差分出变动目录件，与已入库依赖行求交，被改的 `Replace` 重渲、被删的落墓碑，然后推进 `sesno`，
  再做本批的 `Fresh`（顺序不能反：`Fresh` 已有行赢，会把旧版本留下）；差分不可用退化为该库全部依赖行重渲，回执点名
  `cata_dependency_full_refresh`。没被任何 DESI 批碰到的目录库不主动刷——它的依赖行没有读者在等新版本。

### D6　`inst_geo` 降为 `INSERT IGNORE`

内容寻址、同 id 必同内容（ADR-060 N3）；原 `ON DUPLICATE KEY UPDATE direct_model, aabb` 改不了任何读者看得见的东西。
模型面零 UPDATE。旧格式描述符照旧留着（ADR-060 D2 只增不删）。

### D7　模型取数源按 dbnum 就绪度选（`data_interface::model_read_route`）

`database` ⇔ 数据面 ingest ∧ `applied_sesno > 0` ∧ 该库没有活着的初始化发布 run；其余 `memory`（`read-through` /
`data_watermark_unestablished` / `initialization_publishing`）。`memory` 库的 `/model/ensure` 不等初始化队列、不进 durable 队列、
不落 rocksdb，直接从文件生成进进程内投影（有初始化 run 就钉它的 `source_sesno`）；`/model/records` 从投影供数（没 ensure 过回
409 `not_generated:`）。`database` 库一字不改。回执与 `/dbnums` 带 `model_source` / `model_source_reason` / `durable`；翻面
（run 收口 / 回退重建清库）发 WS `model_source_changed`。判据只有一处，读者不重拼（ADR-060 M8 同款）。

## 新不变量

- **E1** rocksdb 形态下元素数据行的写法只有 `element_batch` 一处出口；生产代码里没有逐元素 UPSERT / 逐记录 `UPDATE <表>:<id>`
  （源码钉 `element_batch::tests::production_code_renders_no_upsert_or_per_record_update`、
  `direct_window_replay_parity::production_persist_uses_the_shared_chunking`）。
- **E2** `pe` 行永不被数据面删除（图顶点）；替换是原地整行、字段清单与 `gen_sur_json` 同源。
- **E3** 崩溃重放逐表一致：生产路每个事务边界 kill → 整窗口重放 → 与一次成功逐表相等
  （`production_window_replay_converges_from_every_crash_point`，元素上限 1 / 2 / 1000）。
- **E4** 一个 DESI 元素落库，它的目录链依赖同批落库；`pe.dependency = true` 的行只由 `cata_dependency` 产出；
  CATA 库不进 `applied_sesno`。
- **E5** 「已初始化」只有 `model_read_route::route` 一个谓词；`/model/records` / `/model/ensure` 体内不按进程介质分叉。

## 取舍

- **放弃的**：`pe` 的「真删再插」。它在图顶点上等价于把别人的边一起删掉，`tubi_relate` 根本不属于数据窗口，块边界修不了。
- **换来的**：一条数据路径（基线与增量同一渲染器）、一份语句形状、切块上限与模型侧同量级；CATA 属性面板不再空、`ref_rev`
  的 CATA 侧不再只靠水位后的补偿；库正在初始化时 plant-ui 立刻有模型可看。
- **代价**：被删元素的 noun / UDA 行随删除消失（读者只有搜索 / 属性面板会碰已删元素，本就不该命中）；每窗多一次 CATA 闭包
  （seed 只取目录链属性，闭包按容器白名单收窄，`Fresh` 命中即零成本）；`inst_geo` 的旧描述符不再被修正。

## 开放问题

- ~~**Q1** D5 的对拍 oracle~~ → 2026-09-07 已落地：e3d-model `GenerationOutcome::catalogue_visited`（纯输出，ADR-057 B1；
  记录点在 e3d-io `read_stats::trace_element_reads`——`parsed` / `extraction` 读成功即记，与调用点无关，所以目录链每一跳
  都在里面），gen-model `cata_dependency::tests::live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure` 钉
  `catalogue_visited ⊆ closure`。**首次对拍就抓到三处**：① `on_demand_db::open_single` 反哈希的是 `descriptor().noun`
  而不是 `db_type_hash`，真库上每个目录文件都被拒、闭包一行也解不出（P4 三个写入点在真库上一直只出 warning）；
  ② `precise()` 容器名单没有正体（SBOX / LPYR / SCYL …），正体名下的负体（NSBO / NLPY / NSRT / NLCY）落不进库；
  ③ 名单写的是属性名 `PSTR`，p-line 集的 noun 是 `PTSS`，型材对齐读的 PLIN 落不进库。三处修后 ams8000 全库
  `catalogue_visited` 5,009 ⊆ closure 13,995（6 个目录库），`unexplained = 0`。
- ~~**Q2** 旧的惰性 CATA 落库（`cata_closure::ensure_cata_refnos_parsed`，只服务 `cata_ref_rev` 补偿队列与 resolve 层 miss）
  是否随 `ref_rev` 降级（ADR-063）一起退役，还是切到 `element_batch`~~ → **2026-09-07 两步都做了，不等 ADR-063**（用户拍板
  「直接把 cata_ref_rev 补偿队列退役（P4 已在水位前落行）」）：
  1. **落库切到统一渲染器**：`ensure_cata_refnos_parsed` → `cata_dependency::resolve_catalogue_rows_with`（与三个写入点同一份
     闭包配置：`precise()` 容器白名单、只收 CATA）+ `persist_resolved_rows`（`element_batch` `Fresh`、`pe.dependency = true`、
     D7 前移先刷新）。起因：Q1 的对拍修好 `on_demand_db` 的库类型字之后，这条路在真库上**第一次真的写进了行**（此前每个目录
     文件都被拒），而它逐元素 `UPSERT pe:X CONTENT {gen_sur_json}` 会把 P4 落的 `dependency = true` 整行抹掉——P5 的前移刷新
     （`dependency_rows_among` / `all_dependency_refnos` 都按 `dependency = true` 找行）就再也认不出这些行；`precise()` 原样跑
     还会把回指到别的 DESI 库的元素连 owner 链一起写进 `pe`（E1 / E4 两条都破）。源码钉
     `lazy_fallback_lands_rows_through_cata_dependency_and_renders_no_upsert`（`cata_closure.rs` 生产正文零 `UPSERT`）。
  2. **`cata_ref_rev` 补偿队列退役**（ADR-056 D8-A / spec 035 T126 的那一档；ADR-058 实施约束 4 早列了它）：`apply_one`
     不再在水位之后 `enqueue_cata_ref_rev`；`SideEffectKind::CataRefRev` 变体只为认得旧库里已排进表的行，drain 到它直接收口
     （不解析、不写）；`/health.side_effect_pending.by_kind` 不再宣告这一档。**影响评估**：它做的事只剩「按生成根子树的
     **全部**出向引用再解一遍闭包，水位之后逐元素重写目录行」——(a) 目录行：P4 已在水位**之前**按 D5 的唯一判据落下，
     对拍 oracle 证明覆盖全部几何实读；退役后不再落的是几何链**之外**的目录引用（如 ISPEC / TSPEC 指向的保温 / 示踪规格
     子树），属性面板对这些引用只能显 refno、不能显名——这是 D5「判据只有一份」的代价，要它就把属性名加进
     `CATALOGUE_CHAIN_ATTRIBUTES`，不另开一条路；(b) `ref_rev`：这条路从来**没有写过一条边**（`ensure_cata_refnos_parsed`
     只写 `pe` / noun / UDA / `pe_owner`），DESI→CATA 边由 `apply_one` 的 reverse_index 步维护、CATA→CATA 边只有
     `rebuild_reverse_index` 会建——退役不改变 `ref_rev` 的任何维护点；(c) `CataDepCache`（root → 目录 id 依赖缓存）失去
     唯一的生产写者，`discard_deferred_cache` 成空转，仓内没有别的读者（老规划器的 CATA 级联在 direct 下不可达），
     随 ADR-063 / 级联计划 P3 一起清；(d) 每个 DESI 窗口少一条副作用作业、少一遍全闭包重解析。ADR-063 的 P5 范围里
     `cata_ref_rev` 这一项从此没有了。源码钉 `cata_ref_rev_is_retired_and_legacy_rows_are_only_settled`。
  3. **那一簇死代码同日清掉**（用户拍板不等 ADR-063）：`cata_closure.rs` 删去根子树播种 `collect_design_subtree_outbound` /
     `collect_database_subtree_outbound(_on)` / `run_cata_closure_pass_for_refnos`、`ensure_cata_parsed_for_roots`、
     `try_lazy_cata_fallback`、依赖缓存 `CataDepCache` / `DEFERRED_CATA_CACHE` / `publish_deferred_cache` /
     `discard_deferred_cache` / `preload_cata_for_roots` / `DependencyCacheContext`、env 开关 `cata_closure_enabled`
     （`AIOS_CATA_CLOSURE_MODE`）、开/关几何冒烟 `geo_smoke_digest`（`bin/cata_smoke` 整个删除——它存在的前提就是那个
     开关）；`batch_worker` 不再 `discard_deferred_cache`；`bin/node_gen_room_probe` 不再要求开关、不再预解析（目录行由
     该库数据批的依赖块落）。留下的：`CataDbLocator` / `InMemoryCataLocator` / `CataClosureResolver`（依赖入库的引擎）、
     `ensure_cata_refnos_parsed` 薄壳（`staging::preload` 引用，随 spec 035 P3 走）、`render_pe_owner_replace`（双引擎对拍
     的标准形态）。ADR-004 状态改读「Phase 1 / 2 在用，Phase 3 ～ 6 已退役」；CONTEXT.md 旧章加注。
- ~~**Q3** plant-ui 侧 U1~~ → 2026-09-07 已落地（plant-ui `task_queue::ModelSource` / `model_record_union.rs` /
  `model_update_ws::decode_model_source_changed`；plant-ui ADR-0025「模型从哪儿读由服务端按库判决；客户端只消费，
  一次装载里一个库只认一个源」）。客户端不自己拼判据，与 E5 是同一条纪律的两端。
