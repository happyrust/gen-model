# kv-mem 默认形态 · 零摄入 + 读透（Read-through）· gen-model 与 plant-ui 双仓计划

> **历史文档。** 仅 §9 的 G0–G3 落地记录仍具参考价值；现行设计与后续验收统一见
> `2026-09-05-memory-first-mdb-and-read-through-merged-plan.md`。

- 日期：2026-09-05
- 提出：用户「当前模型生成默认使用 kv-mem 模式，这样我们不需要解析任何数据，直接通过 e3d-io 的解析
  和 e3d-model 的实时模型生成，就可以把我们需要的 pe、属性数据和模型数据保存到 kv-mem 里，相当于做到
  开箱即用；并让 plant-ui 来适配这种模式」
- 状态：**Q1–Q5 已定案**（2026-09-05 用户拍板「五条拍板全按推荐项，先做 gen-model G0–G3，把写透跑通再说」；
  Plannotator 评审 `approved`；共识记 d-581）。**G0 / G1 / G2 / G3 已于本日落地**（记录见 §9），
  G4 / G5 / G6 与 plant-ui U0–U5 未开工。**未 live**：写透的整条路（`POST /model/ensure` → `pe` 行）只有源码钉
  与 `mem://` 行级对拍作证，没有起过服务。
- 承接（不重复）：
  - `docs/plans/2026-09-04-spawned-memory-store-out-of-box.md`（A′：`store_mode = "spawned-mem"` 已是默认，
    P0–P4 落地、P6 服务侧实测通过；**P5 与 plant-ui 端到端未做**）
  - `plant-ui/docs/plans/2026-09-04-kv-mem-out-of-box-adaptation.md`（D1 = A′ 已定；**M0–M5 一条未做**）
  - ADR-058（direct 默认、CATA 永不入库、启动 = 退化窗口）、ADR-059（生成期读取走池级元素缓存）、
    ADR-056（模型面不以 `pe` 为前置）
- 范围：gen-model `src/options.rs` / `src/lib.rs`（启动分派）/ `src/data_interface/{ele_ops,e3d_collector}.rs`
  （行渲染与读取，只加不改）/ 新增 `src/data_interface/read_through.rs` / `src/fast_model/e3d_model_service.rs`
  （生成后挂写透）/ `src/web_service/{mod,handlers}.rs`（一条新入口 + `/health` `/dbnums` 两格）/
  `vendor/e3d-model/src/pipeline.rs`（`GenerationOutcome` 多带一张访问名单）/ `scripts/Verify-OutOfBox.ps1`；
  plant-ui `plant-ui/crates/plant-ui/src/task_queue.rs`、`plant-ui/crates/plant-ui-app/src/{data,model_update_api}.rs`、
  `plant-ui/crates/plant-ui-data/src/{lib,name_index}.rs`。

---

## 0. 一句话

A′ 已经把「库」做成开箱即用（起 gen-model，memory 后端的 SurrealDB 跟着起）；**还没开箱即用的是「数据」**：
默认形态下启动仍会对纳入的 DESI 库跑一次整库基线（ADR-058 D3），而不跑基线（`AIOS_STARTUP_AUTORUN=0`）
的话 `pe` 是 0 行，plant-ui 的属性面板、名称搜索、`nouns_of`、隐式直管的 `in.owner` 全部踩空。

本计划把数据面在 kv-mem 默认形态下改成**读透缓存**：

1. **零摄入**：默认形态启动不跑 DESI 基线、不开数据批次、不推进 `applied_sesno`；
2. **生成即写透**：e3d-model 生成一根时 e3d-io 已把子树每个元素读过一遍（ADR-059 池级缓存），把这份
   `EleData` 经**同一个** `ElementRows` 渲染器（`src/data_interface/ele_ops.rs:1488`）写成 `pe` / `ATT_<noun>` / `ATT_UDA` /
   `pe_owner` 行，与模型行一起落进 kv-mem——「我们需要的 pe 和属性」就是模型碰到的那些；
3. **按需读透**：没生成过的元素由一条新入口 `POST /api/v1/element/ensure` 从文件读进库，plant-ui 在查空时调它；
4. **plant-ui 适配**：读面继续直连 SurrealDB（A′ 的承诺不变），只在四处改口径：工程标识改吃 `/dbnums`、
   属性面板查空先 ensure、隐式直管的 `owner` 不再硬依赖 `pe` 行、搜索与索引把覆盖面说实话。

对下游一个字不改：`SUL_DB` 仍是 ws、`/sql` 探针 / `rvm_verify` / 取证脚本照旧够得着。

---

## 1. 现状锚点（2026-09-05 工作树实位，全部核过）

### 1.1 gen-model 已经站住的

| 事实 | 位置 |
|---|---|
| `store_mode` 三值，默认 `spawned-mem`；13 份夹具显式 `external` + 护栏 `every_db_option_fixture_declares_its_store_mode` | `src/options.rs`、`db_options/*.toml` |
| 自带存储子进程：查找 / 版本闸 / 署名四格裁决 / `Ctrl+C` 收尸 / `/health.sul_db{endpoint,medium,durable}` | `src/store_process.rs`、`src/web_service/handlers.rs` |
| P6 服务侧实测：启动 6.47 s；`POST /model/ensure` 一个 SITE 89.3 s → `inst_relate` 416 行、平表三件齐活；**`pe` 0 行** | 09-04 计划 §8 |
| 树走文件：`DirectTreeService` 用 e3d-io 打开 MDB 的全部 DESI，不查 `pe` / `pe_owner` | `src/data_interface/direct_tree.rs:27` |
| 模型按需：`POST /api/v1/model/ensure`；生成产物经 `model_db_adapter::persist_root` 落库 | `src/web_service/mod.rs:315`、`src/fast_model/model_db_adapter.rs:213` |
| **模型行引用 `pe` 却从不写 `pe`**：`inst_relate.in = type::thing('pe', …)` 是悬空引用 | `src/fast_model/model_db_adapter.rs:635/640` |
| 生成期读取：`E3dModelService::build_set` 建 `DbSet`（e3d-io，池级元素缓存），`generate_snapshot_from_set` → e3d-model `generate_subtree_with_cache` **DFS 访问子树每个元素**，但只回 `Report.visited: usize` 一个计数与有几何的 `elements` | `src/fast_model/e3d_model_service.rs:1113/1200`、`vendor/e3d-model/src/pipeline.rs:137/246/261` |
| 数据面行形状只有一份：`ElementRows::render(ele_data, id, dbnum, sesno, uda_keys)`，基线 `INSERT IGNORE`、增量 `upsert_statements`（宪法 II） | `src/data_interface/ele_ops.rs:1488-1569` |
| 在钉住会话的 `DbSet` 上读一个元素的数据面终态：`e3d_collector::read_ele_data(set, refno)`（与生产收集器同一条读法） | `src/data_interface/e3d_collector.rs:310` |
| 基线是 e3d-io 退化窗口 `stream_baseline`（`pe.sesno` = 元素 last-touch 会话，由 `PageSessions` 反查） | `src/data_interface/e3d_collector.rs:517-560` |
| 启动即自动干活 `startup_autorun` 默认 `true`；`AIOS_STARTUP_AUTORUN=0` 是今天「不预解析」的唯一开关，**但它同时关掉队列消费与全量房间重建**，不是数据面的开关 | `src/options.rs:887`、`scripts/Verify-OutOfBox.ps1:82` |
| `/dbnums` = 登记表 ∪ 项目扫描（文件侧），零解析下仍有 DESI 行，但 `applied_sesno = 0`、`initialization_required = true`；**每行不带库类型** | `src/web_service/handlers.rs:1232`、09-04 审核 #6、plant-ui 计划 G3 |
| HTTP 面上**没有**：属性、名称搜索、按需数据 | `src/web_service/mod.rs:297-357` |

### 1.2 plant-ui 今天怎么读、零摄入下怎么坏

| 读面 | 位置 | 打哪 | 零摄入下 |
|---|---|---|---|
| 启动 `ready()` → `connect()` → `project_identity()` | `plant-ui/crates/plant-ui-app/src/data.rs:497`、`plant-ui/crates/plant-ui-data/src/lib.rs:17/504` | `init_surreal` + `query_mdb_db_nums(DESI)`（读 `MDB` / `CURD` / `WORL` 表，`vendor/old-aios-core/src/rs_surreal/mdb.rs:193`） | 连得上（A′）；**DESI 名单为空** → 「模型更新」本期范围空 |
| 树 roots / children / ancestors | `plant-ui/crates/plant-ui-app/src/data.rs:24-57` | HTTP（默认 direct） | ✅ |
| 三维实例 `model_instances_anc` | `plant-ui/crates/plant-ui-data/src/lib.rs:117` | `inst_relate.anc` + 平表 | ✅（P6 实测 416/416） |
| **隐式直管** `query_derived_insts_by_inputs` | `plant-ui/crates/plant-ui-data/src/lib.rs:248`，`in.owner AS owner`（`:263`） | `inst_relate.in.owner` → `pe` 行 | `pe` 缺行 → `owner` 为 NONE，而 `GeomInstQuery.owner` 不是 `Option` → **一根 BRAN 的直管整批解不出** |
| 属性面板 `element_props` | `plant-ui/crates/plant-ui-data/src/lib.rs:550` → `get_ui_named_attmap` | `pe` / `ATT_*` | 空表，且界面把它读成「无」 |
| 名称前缀搜索 `search_names_by_prefix` | `plant-ui/crates/plant-ui-data/src/lib.rs:447` | `pe.name` 范围扫 | 查不到 |
| ngram 语料与陈旧戳 | `plant-ui/crates/plant-ui-data/src/name_index.rs:167/170` | `pe` 全量 + `dbnum_watermark` | 语料空；戳表空 |
| 重新生成取材 `nouns_of(anc)` | `plant-ui/crates/plant-ui-data/src/lib.rs:382` | `pe` | 祖先缺行 → 生成根归并少算 |
| 房间 | `plant-ui/crates/plant-ui-data/src/room.rs:387/430/444` | `pe_owner` / `pe` / `room_*` | 面板与成员的 `pe` / `pe_owner` 缺行 |
| `Health` | `plant-ui/crates/plant-ui/src/task_queue.rs` | HTTP | **不解 `sul_db` / `initialization`**（grep 全 crates 无 `sul_db`）——09-04 计划 M0 未做 |

---

## 2. 名词（进 `CONTEXT.md`）

**数据面读透 (Data-face Read-through)**：
数据面的第二种形态（与「摄入 (Ingest)」相对）：不做基线、不开数据窗口、不推进应用水位；`pe` / `ATT_*` / `pe_owner`
行是**模型生成的副产品**与**按需请求的结果**，行形状与摄入形态逐字节相同（同一个 `ElementRows`），但语义是
**缓存**——缺行表示「还没读到」，不表示「不存在」；`applied_sesno` 恒 0，不是幽灵水位。
_Avoid_: 懒解析、部分解析（那是退役的 CATA 词条）、按需基线、data cache（泛指时）

**写透 (Write-through)**：
读透形态下，生成一根模型时把该根子树访问到的每个元素（含祖先链）的数据面终态随模型行一起落库。开销与
生成子树成正比，不与库规模成正比。
_Avoid_: 顺手落库、生成时解析

---

## 3. 设计

### 3.1 形态开关（Q1）

`DbOption.toml` 新键，`AIOS_DATA_FACE` 压过配置，取值规则同 `store_mode`（认不出的值按未设置处理并出声）：

```toml
# 数据面形态。默认 "read-through" = 开箱即用：不做基线、不开数据窗口，pe / ATT_* 行随模型生成
# 与按需请求写进库；缺行 = 还没读到。
#   "ingest"        整库基线 + 稳态增量窗口，推进 applied_sesno（生产落盘形态；今天的行为）
#   "read-through"  读透缓存（默认）
data_face = "read-through"
```

- **与 `store_mode` 正交**，不派生：`external + read-through`（落盘的读透缓存）合法；`spawned-mem + ingest`
  也合法（每次开机基线一遍——就是今天 A′ 的默认）。介质与数据面是两件事，捆在一起就又要靠注释解释。
- **默认翻转的到达面与 P0 同款**：13 份现场 / 夹具配置逐份补 `data_face = "ingest"`（仓根 `DbOption.toml` 不写），
  护栏 `every_db_option_fixture_declares_its_store_mode` 扩成「`store_mode` 与 `data_face` 两键都必须声明」；
  五处「先起库再起服务」的脚本补 `AIOS_DATA_FACE=ingest`。理由与 09-04 P0 一字不差：默认翻转只在翻的那天有人记得。
- **库要记住自己是哪种数据面写的**：`gen_model/store` 署名（spawned）或 `dbnum_state` 旁一行 `data_face:current`。
  启动时配置与库不一致 → **拒绝启动并给出路**（读透缓存的部分 `pe` 行被摄入基线的 `INSERT IGNORE`「已有行赢」
  接管，是一种不报错的错；反向则是把整库摄入结果当缓存作废）。kv-mem 每次开机是空库，这一格对它永远静默。

### 3.2 写透：生成一根 → 子树元素行落库（Q2）

**入口只有一个**：新增 `src/data_interface/read_through.rs`

```rust
/// 把一批元素在 `set` 钉住会话上的数据面终态写进库。行形状 = `ElementRows`（与基线 / 增量同一份），
/// 动词 = `UPSERT`（幂等；同一元素再读一次就是刷新）。CATA 库的 refno 一律过滤（ADR-058 S3）。
pub async fn write_through(set: &Arc<DbSet>, dbnum: u32, refnos: &[RefNo]) -> anyhow::Result<ReadThroughReceipt>;
```

调用方两处：生成后挂钩（本节）与 HTTP 按需（§3.3）。数据面的行形状归数据面，`fast_model` 不许自己拼 `pe` 行。

1. **访问名单从哪来**：e3d-model `generate_subtree_with_cache` 的 DFS 已经逐个 `stack.pop()` 到每个元素
   （`vendor/e3d-model/src/pipeline.rs:271-277`），`GenerationOutcome` 加一项 `visited: Vec<RefNo>`（`Report.visited` 计数保留），
   经 `GeneratedSnapshot` → `BakedSnapshot` 带进 `generate_refs`。**不另起一次子树枚举**——那是第二份「子树是什么」。
2. **范围 = 访问名单 ∪ 根的祖先链到 WORL**。祖先链是 plant-ui `nouns_of(anc)`、房间的 `pe_owner` 上溯与
   `in.owner` 都要的；一条链几层，便宜。
3. **读取走 `read_ele_data(set, refno)`**（`src/data_interface/e3d_collector.rs:310`），`set` 就是这次生成的 `DbSet`——池级缓存
   已经热了（ADR-059），第二遍读几乎不付解析费。
4. **`pe.sesno` 仍是元素 last-touch 会话**，与基线同一口径（2026-09-04 起 `stream_baseline` 就这么写），
   不许拿生成时点 `source_sesno` 顶替；`e3d_collector` 露一个 `read_ele_data_with_sesno`（`PageSessions` 反查，
   与 `stream_baseline:558` 同一行代码搬出来复用）。`UdaKeyMap` 从哪拿基线怎么拿、读透就怎么拿。
5. **落库时机**：在该根模型行的发布**之前**、同一次 `persist` 调用里，单独一批事务（TX_CHUNK 分块，与
   `persist_latest_main_data` 同款）。不进根级 CAS：元素行是缓存，不参与「这根发布成没成」；先写它们是为了
   plant-ui 在 `ensure` 回执之后读模型行时 `in.owner` 已经在。
6. **失败不拦模型**：写透失败记进 `E3dPersistReport.warnings`（`read_through_degraded`）并出日志，模型照常发布——
   反过来才是把缓存当成了前置。

**删除怎么办**：`UPSERT` 覆盖不到「子树里少了的元素」。读透形态下删除由 §3.4 的窗口处理（`collect_window` 差分
的 `Deleted` 软删 `pe.deleted = true`，与今天增量同款）；没有 watcher 时残留到重启（kv-mem 随进程消失）。
不做「根范围先删后插」：要先知道上一轮写了哪些行，那又是一份账。

### 3.3 按需读透：`POST /api/v1/element/ensure`（Q4）

```jsonc
// 请求
{ "refno": "24381/34109", "depth": 1, "project": "...", "mdb": "..." }
// 回执 200
{ "refno": "24381/34109", "dbnum": 7997, "noun": "ZONE", "written": 38, "sesno": 264,
  "source": "read-through", "elapsed_ms": 41 }
```

- 范围 = 该元素 ∪ 祖先链到 WORL ∪ `depth` 层后代（默认 1 = 直接子层，够树右键与属性面板）；上限
  `read_through_max_elements`（默认 5000），超了回 `422` 并指路 `POST /model/ensure`——子树级的活归模型面，
  它顺手就把行写了。
- 读取走 `E3dModelService::build_set(dbnum, None)`（文件最新会话，与 `model/ensure` 同一时点口径，ADR-054），
  不新造第二个文件打开路径。`DirectTreeService` 的 `DirectStore` 会话是启动时冻结的（`src/data_interface/direct_tree.rs:24-26`），
  拿它写库会把树看到的那一版与模型看到的那一版混进同一张表。
- 域外库（`watch_scope` 不放行）与 CATA 库的 refno：**拒绝**，回执明说，与 `model/ensure` 同一套嗓音
  （`src/web_service/handlers.rs` 既有的域外拒绝）。CATA 见 Q5。
- 只有 `data_face = read-through` 才挂这条路由？**不**——摄入形态下它同样合法（相当于「立刻把这几行刷新到文件最新」），
  但回执 `source` 要说清是 `ingest`。路由恒在，行为看形态。
- `/health` 加 `data_face` 一节：`mode` / `written_rows` / `ensure_calls` / `degraded`。

### 3.4 读透形态下的文件事件与人工更新（Q3）

读透没有数据窗口，但**模型增量不能没有**（ADR-058 D8：watcher 一直盯着自动追）。今天模型意图由数据窗口的尾事务
写下（`model_update_pending`），读透下这条链的前半截不存在。收法：

- **文件事件 = 一次退化窗口**。watcher 观察到某 DESI 前进 → 不入数据批次，改调 `window_root_plan::reconcile_startup_window`
  同一条路（S = 该库 `gen_root` 凭证下限、T = 文件最新；ADR-058 S1「启动没有自己的选根判据」在这里再用一次：
  **文件事件也没有**）。四桶分流、护栏、`model_update_pending` 入队照旧，模型车道消化。
- **缓存刷新 = refresh-if-present**：窗口差分给出的 `Modified` refno 里**已经有 `pe` 行的**走 `write_through` 刷新，
  `Deleted` 软删，没有行的不管（还没人读过，读的时候自然是新的）。开销与变更量成正比。
- **`applied_sesno` 不动**，`/dbnums` 每行加 `data_face`；读透下 `initialization_required` 恒 `false`
  （它的语义是「库还没导入过」，读透没有导入这件事）。
- **人工「模型更新」**（`POST /update/preview|execute`）在读透下 = 上面那条退化窗口的人工触发：只排模型工作、不摄入、
  回执 `data_face: "read-through"` 点名「本次不摄入数据」。`increment_manager::discover_batch` 的首次导入 / 幽灵水位路由
  在读透下**不可达**（护栏：读透形态全程不调 `stream_baseline`、不写 `applied_sesno`）。
- **启动**：读透形态跳过 Meta → Design 摄入链，模型复核（`reconcile_model_coverage_at_startup`）照跑（它本来就是文件尺），
  `data_ready` 在读透下即刻为真（没有数据要等）。`startup_autorun` 保持原义（队列消费 / 房间重建的总闸），不再兼职
  「不预解析」——`Verify-OutOfBox.ps1` 把 `AIOS_STARTUP_AUTORUN=0` 那行删掉。

### 3.5 搜索与名称索引（Q4 后半）

`pe.name` 范围扫与 ngram 语料在读透下只覆盖已读到的行。**不在本计划里造全库名称索引**：读全部元素的 `NAME`
就是一次整库扫描（AMS 8.8M 行），那正是「零摄入」要省掉的东西。本计划只要求**说实话**（§3.6 U4），
全库名称索引（按库懒建、e3d-io 侧、落磁盘缓存）另起一份计划。

### 3.6 plant-ui 适配（读面直连 SurrealDB 不变；四处改口径）

| # | 事 | 位置 | 改法 |
|---|---|---|---|
| U0 | `Health` 补 `sul_db{endpoint,medium,durable}` / `data_face` / `initialization`（全部 `serde(default)`） | `plant-ui/crates/plant-ui/src/task_queue.rs` | 09-04 计划 M0 原文 + `data_face` 一格；`Vm::store_unreachable_reason()` 只对 `medium == "embedded-mem"` 说话 |
| U1 | 工程标识的 DESI 名单改吃 `/api/v1/dbnums`（队列轮询本来每拍都取），不再 `query_mdb_db_nums` | `plant-ui/crates/plant-ui-data/src/lib.rs:504`、`plant-ui/crates/plant-ui-app/src/data.rs:499` | `project_identity()` 拆成「配置里的项目 / MDB / ns」+「`/dbnums` 的 DESI 行」；需要 gen-model `/dbnums` 每行带 `db_type`（G3 前置）。「未应用保存数」提示在 `data_face == read-through` 下不画（`applied_sesno` 恒 0 不是落后） |
| U2 | 属性面板：查空 → `POST /element/ensure` → 重查；三档文案 | `plant-ui/crates/plant-ui-app/src/data.rs` `Req::Props`、面板空态 | 读透：「正在从库文件读取…」→ 回执后重查；仍空且 `dbnum ∈ CATA` → 09-04 M2 的定论文案；摄入形态维持 09-04 M1「数据仍在装载（N / M 库）」。**「无」只在三档都不命中时出现** |
| U3 | 隐式直管 `owner` 不再硬依赖 `pe` 行 | `plant-ui/crates/plant-ui-data/src/lib.rs:263`、`tubi_to_geom:311` | 查询改取 `in.owner ?? NONE` 与 `anc[1] AS owner_packed`，Rust 侧 `owner = in.owner.or(anc[1])`——与平表快路 `query_insts_flat` 同一口径（那条路本来就从 `anc[1]` 还原 owner）。写透落地后 `in.owner` 会在，这条是**不让一行缺失拖垮整根**的保险 |
| U4 | 搜索与 ngram 说实话 | `plant-ui/crates/plant-ui-data/src/name_index.rs:167/170`、搜索框空态 | 陈旧戳改吃 `/dbnums` 的 `applied_sesno` / `file_latest_sesno`（09-04 M3），读透下语料 = 已读到的行、戳 = `written_rows`；搜索框空结果在读透下写「搜索只覆盖已读到的元素（N 行）」而不是「无匹配」 |
| U5 | 文档与 `CONTEXT.md` | `plant-ui/docs`、`CONTEXT.md` | 「起 gen-model → 起 plant-ui」两段；`v_ip/v_port` 必须一致；`PLANT_MESH_DIR` 指 gen-model `meshes_path`；新词条「冷库」改写为「读透缓存」口径（09-04 M4 的文案里「属性要等灌到才有」在读透下是错的：**不是等，是点**） |

**09-04 plant-ui 计划的修订**：M1「数据仍在装载」只对摄入形态成立；M5 第 2 条「灌库期间点一个元素 → 灌完再点」
改成「点一个未生成元素 → 面板 200 ms 内出属性（`element/ensure`）」。其余 M0 / M2 / M3 / M4 原文并入 U0–U5。

---

## 4. 要拍板的五条

| # | 题 | 推荐 | 备选 | 为什么推荐 |
|---|---|---|---|---|
| Q1 | 数据面开关的键名与默认 | `data_face = "read-through" \| "ingest"`，默认 `read-through`，与 `store_mode` 正交；13 份夹具补 `ingest` + 护栏 + 库内 `data_face:current` 一致性拒绝 | 由 `store_mode` 派生（mem → 读透、external → 摄入） | 派生要靠注释解释，而且堵死了「落盘的读透缓存」这个便宜形态；P0 的迁移模式已经走通一次 |
| Q2 | 写透的范围与动词 | 访问名单 ∪ 祖先链；`UPSERT`；删除交给窗口软删 | 根范围先删后插 | 先删后插要记「上一轮写了哪些」，多一份账；`UPSERT` 幂等且与增量路同一动词 |
| Q3 | 读透下文件事件与人工更新 | 文件事件 = 退化窗口（复用 `reconcile_startup_window`），refresh-if-present，`applied_sesno` 不动，`initialization_required` 恒 false | 保留数据批次、只跳过渲染 | 备选要改 `applied_sesno` 的语义与 ADR-021 三条判据（ADR-058 D3 明确拒绝过这条路） |
| Q4 | 未生成元素的属性与搜索 | 属性：`element/ensure` 写透后 plant-ui 照旧读 SurrealDB（读面零改动）；搜索：本计划只说实话 | 属性走 HTTP 内联返回（09-02 的 G4 全版）；搜索按库懒建全库名称索引 | 内联返回等于给 plant-ui 开第二条属性读面（HTTP 与 SurrealDB 两份格式化逻辑）；全库名称索引是整库扫描，另立计划 |
| Q5 | CATA 元素的属性 | 维持 ADR-058 S3：不入库，`element/ensure` 对 CATA refno 拒绝，plant-ui 给定论文案（09-04 M2） | `element/ensure` 对 CATA 只读不写、内联回属性 | 备选要修 ADR-058 S3 并给 plant-ui 开 HTTP 属性读面；先看用户点开元件库元素的频率再定 |

---

## 5. 里程碑

### gen-model

| 步 | 内容 | 依赖 | 产物 / 测试 |
|---|---|---|---|
| **G0** | `data_face` 三件套（Q1）：`DataFace` 二值 + `data_face()` + `AIOS_DATA_FACE` + 出声；13 份配置补 `ingest`；护栏扩成两键；五处脚本补 `AIOS_DATA_FACE=ingest`；`/health.data_face.mode`、横幅一行。**行为一个字不改**，护栏 `introducing_data_face_does_not_change_ingest_yet` | Q1 | `cargo test --lib -- options::tests`；负例：注释掉一份夹具的 `data_face` 即红 |
| **G1** | e3d-model `GenerationOutcome.visited: Vec<RefNo>`（`vendor/e3d-model/src/pipeline.rs`）；gen-model `GeneratedSnapshot` / `BakedSnapshot` 带过来 | — | e3d-model 单测：`visited.len() == report.visited`；`gen_ams` 三份产物 SHA-256 逐字节不变（ADR-059 纪律） |
| **G2** | `src/data_interface/read_through.rs::write_through`：`read_ele_data_with_sesno` + `ElementRows::render` + `UPSERT` 分块；CATA 过滤；`ReadThroughReceipt` | G1 | `mem://` 真跑：写一根后 `pe` / `ATT_<noun>` / `pe_owner` 行与 `stream_baseline` 对同一元素渲出的行**逐字段相同**（行级对拍是这一步唯一的验收）；源码钉：`fast_model/` 全文不含 `INSERT IGNORE INTO pe` / `UPSERT pe:` |
| **G3** | 生成后挂钩：`generate_refs` 每根成功后、模型行发布前调 `write_through(访问名单 ∪ 祖先链)`；失败入 `warnings` 不拦；`/dbnums` 每行加 `db_type` 与 `data_face` | G2 | `mem://` 真跑：`POST /model/ensure` 一根 BRAN 后 `SELECT in.owner FROM inst_relate WHERE …` 无 NONE；护栏 `write_through_precedes_root_publication`、`write_through_failure_never_fails_the_root` |
| **G4** | `POST /api/v1/element/ensure`（§3.3）：范围 / 上限 / 域外与 CATA 拒绝 / 回执；`/health.data_face` 计数 | G2 | handler 单测（拒绝分支三条）+ `mem://` 真跑（祖先链齐、`depth` 生效、超限 422） |
| **G5** | 启动与增量按形态分派（Q3）：读透跳过摄入链、`data_ready` 即真；watcher 事件 → 退化窗口 + refresh-if-present；`update/preview\|execute` 读透语义；`initialization_required` 恒 false | G0、G2 | 源码钉：读透分支全文不含 `stream_baseline` / `finalize_attempt` / `applied_sesno` 写；`mem://` 真跑：模拟文件前进 → `model_update_pending` 出 regen 行、`pe` 已有行刷新、无行不写 |
| **G6** | `Verify-OutOfBox.ps1` 扩：删 `AIOS_STARTUP_AUTORUN=0`；自动判加「`model/ensure` 后 `pe` 行 > 0」「BRAN 的 `in.owner` 无 NONE」「`element/ensure` 200 且 `written > 0`」「`/dbnums` 无 `initialization_required`」；顺带收 09-04 计划 P5（版本闸单一来源护栏） | G3–G5 | 脚本跑通；P5 护栏绿 |

### plant-ui

| 步 | 内容 | 依赖 | 测试 |
|---|---|---|---|
| **U0** | `Health` 三格 + `data_face`；`store_unreachable_reason` | — | `an_embedded_store_is_named_as_unreachable`、`a_spawned_store_stays_silent`、`old_servers_without_the_keys_stay_silent` |
| **U1** | 工程标识 DESI 名单改吃 `/dbnums`；读透下不画「未应用保存数」 | gen-model G3 | `desi_dbnums_come_from_the_dbnums_poll_not_the_mdb_table`、`read_through_never_shows_pending_saves` |
| **U2** | 属性面板查空 → `element/ensure` → 重查；三档文案 | gen-model G4 | `an_empty_props_result_in_read_through_asks_the_service_first`、`a_catalogue_element_gets_a_verdict_not_an_empty_table` |
| **U3** | 隐式直管 `owner` 回退 `anc[1]` | — | `plant-ui/crates/plant-ui-data/tests/zone_tubi_visibility.rs` 加一条「`pe` 缺行时直管仍解出」 |
| **U4** | 搜索 / ngram 说实话；戳换 `/dbnums` | gen-model G3 | `the_stamp_comes_from_the_dbnum_poll_not_the_watermark_table`、`read_through_search_names_its_coverage` |
| **U5** | 文档 + `CONTEXT.md` 词条；09-04 计划 M1 / M5 改口 | — | — |

顺序：G0 → G1 → G2 → G3 → G4 → G5 → G6；U0 / U3 / U5 不等 gen-model，U1 / U2 / U4 等 G3 / G4。

---

## 6. 验收（AvevaMarineSample，`watch_dbnums` 先限定 7997 / 7999，两个进程）

1. **只起 gen-model**：仓根 `DbOption.toml` 不写 `store_mode` / `data_face`；日志「自带存储 pid=N」+「数据面：读透」；
   `/health.sul_db.medium = spawned-mem`、`/health.data_face.mode = read-through`；**启动全程无「基线」字样**，
   `pe` 0 行；启动到 `/health` 答话 < 10 s（P6 实测 6.47 s）。
2. **起 plant-ui**：配置一字不改；树立刻可展开；「模型更新」面板 DESI 名单非空、无「初始化」按钮、无「未应用保存数」。
3. **点一个从未生成的 EQUI 的眼睛**：`model/ensure` → 三维出来；库里该根子树每个元素有 `pe` / `ATT_<noun>` 行，
   `pe_owner` 边通到 WORL；点它 → 属性面板不经 ensure 直接有属性。
4. **点一根 BRAN 的眼睛**：直管**整批**出来（`in.owner` 无 NONE）；U3 的保险在 `pe` 行被手工删掉后仍成立。
5. **点一个没生成过的 ZONE**：面板先说「正在从库文件读取…」，200 ms 内出属性；回执 `written` 与该 ZONE 直接子层数 + 祖先链一致。
6. **点一个元件库元素**：定论文案，不是空表；`element/ensure` 回 4xx 且说清是 CATA。
7. **搜索一个未读到的名字**：提示「搜索只覆盖已读到的元素（N 行）」；生成过它所在的根之后再搜命中。
8. **在 E3D 改这根 BRAN 的一段 → SAVEWORK**：无人工干预模型在窗口内更新（退化窗口）；该 BRAN 的 `pe.sesno` 前进；
   `applied_sesno` 仍为 0；`/dbnums` 无 `initialization_required`。
9. **`Ctrl+C` gen-model**：端口放开、无残留 surreal（09-04 P6 第 5 条，补上）；再起：空库，重复 3–5，**耗时记账**。
10. **误配一次** `data_face = "ingest"` 对着一台读透写过的落盘库：拒绝启动并指路。

记录落 `changelog.md` 一条 + plant-ui `docs/2026-08-12_live-test-ledger.md` 同款格式。

---

## 7. 新不变量

- **R1 读透形态零基线**：`data_face = read-through` 下任何代码路径都不得调 `stream_baseline`、不得写 `applied_sesno`、
  不得进首次导入 / 幽灵水位路由。护栏钉源码。
- **R2 行形状只有一份**：写透与按需读透产出的 `pe` / `ATT_*` / `pe_owner` 行必须与 `stream_baseline` 对同一元素渲出的
  逐字段相同（宪法 II 的第三个消费者）。`fast_model/` 不许出现任何 `pe` 表的写语句。
- **R3 缓存不做前置**：写透失败不拦模型发布；`element/ensure` 失败不拦树与三维。反过来就是把 ADR-056 N7 又装回去。
- **R4 CATA 仍不入库**（ADR-058 S3 原文）：访问名单里的 CATA refno 一律过滤；`element/ensure` 对 CATA 拒绝。
- **R5 开销与触碰量成正比**：写透 ∝ 生成子树，刷新 ∝ 窗口变更量，按需 ∝ `depth`；任何 ∝ 库规模的路径（全库名称索引）
  都不属于读透形态。

---

## 8. 风险与不做

- **`pe.sesno` 口径**：写透若偷懒拿 `source_sesno` 当 `pe.sesno`，会让「元素 last-touch 会话」这一 2026-09-04 才纠正的
  口径在同一张表里出现两种含义。G2 的行级对拍就是为这一格设的，**必须包含 `sesno` 字段**。
- **e3d-model API 变动撞并行会话**：`GenerationOutcome` 加字段是纯增量，但 `vendor/e3d-model` 有别的会话在飞
  （SCTN 接线、W7-1 A 半边）。G1 只加字段不改签名，改前 `git status` 看一眼那边的工作树。
- **读透下的模型增量是第一次 live**：ADR-058 D8 的 watcher + 退化窗口至今**未 live 验证**（09-04 审核 §5 第 3 条），
  G5 会同时面对「第一次真实文件事件流」和「第一次读透」。验收第 8 条务必先限定域，异常先按 09-04 审核的账归因。
- **内存**：写透的行数 ∝ 用户看过的子树，kv-mem 下随进程消失；`--memory` 后端的占用仍未量过（09-04 计划 §7 第 4 条），
  验收第 3 / 9 条顺带记 `surreal.exe` 的工作集。
- **两个仓的 `/dbnums` 契约同时改**（`db_type` / `data_face` 两格）：先加字段、plant-ui `serde(default)`，老服务端不给就不画。
- **不做**：全库名称索引（另立计划，§3.5）；属性 HTTP 内联读面（Q4 备选）；CATA 属性读面（Q5 备选）；动 ADR-058 D3
  （摄入形态下基线照做，本计划只加一种形态不删一种）；动 `embedded-mem` / `mem://` 那条探针路；动 plant-ui-view3d。

---

## 9. 落地记录

### G0–G3（2026-09-05）

| 步 | 落地形态 | 与 §3 / §5 的差别 |
|---|---|---|
| **G0** | `src/options.rs`：`DataFace { Ingest, ReadThrough }` + `data_face()` + `AIOS_DATA_FACE` + `data_face_notice()`；`data_face` 进 `DbOptionExt` / `DbOptionExtFields` / 合并；13 份配置补 `data_face = "ingest"`（仓根 `DbOption.toml` 不写）；护栏改名 `every_db_option_fixture_declares_its_store_mode_and_data_face`，两键都守、漏哪个都点名（**已按负例验过**：注释掉一份的 `data_face` 即红并点名）；五处「先起库再起服务」补 `AIOS_DATA_FACE=ingest`（`l3_suite/fixture.rs`、`Run-LiveBatch` / `Run-Issue7E2E` / `Run-RoomE3DE2E` / `Start-AiosDatabaseManual`）；`/health.data_face`（顶层一格，值 `read-through` / `ingest`）；横幅一行；`run_cli` 顺手把此前有定义没人打的 `store_mode_notice()` 与新的 `data_face_notice()` 一起打出来 | §5 写的 `introducing_data_face_does_not_change_ingest_yet` 没做——G0 之后 `data_face()` 的唯一消费者就是 G3 的写透，那条护栏无物可钉；**行为在 G3 落地前确实一个字没改** |
| **G1** | `vendor/e3d-model/src/pipeline.rs`：`GenerationOutcome.visited: Vec<RefNo>`，DFS 每 pop 一个就 push（`debug_assert_eq!(visited.len(), report.visited)`）；`gen_ams.rs` / `examples/bran_audit.rs` 的解构补 `..`；gen-model `GeneratedSnapshot` / `BakedSnapshot` 带 `visited` | 无 |
| **G2** | 新增 `src/data_interface/read_through.rs`：`ReadThroughSource { dbnum, file, sesno }`、`render_rows`（同步、只读文件）、`write_through` / `write_through_on`（落 `SUL_DB` / 调用方句柄）、`ancestors_of` / `subtree_with_ancestors`、`ReadThroughReceipt { requested, written, foreign, missing, statements, warnings, … }`。读取走 `e3d_collector::read_ele_data`，渲染走 `ElementRows::render`，动词 `upsert_statements(DeleteWhenEmpty)`（改成 `pub(crate)`），分块走 `persist_transaction_batches(PERSIST_TX_CHUNK)`；last-touch 会话 = `session_chain` → `PageSessions` + `IndexCursor::seek` 反查，与 `stream_baseline:558` 同一口径；`RefNo::dbno() != dbnum` 一律不写（`foreign`） | §3.2 第 2 条的祖先链改从 `DbSet::element(..).owner()` 上溯（`ancestors_of`），不用 `SourceIndex::owners`——编排层拿不到那张表，而 owner 导航本来就在池级缓存里；§3.2 第 4 条的 `read_ele_data_with_sesno` 没加进 `e3d_collector`（那儿有「不碰库」与「整棵索引枚举只此一处」两条源码钉），反查逻辑放在 `read_through.rs` 自己身上 |
| **G3** | 挂钩不在 `generate_refs`（`the_generation_service_never_reaches_for_a_database` 钉死模型面不认识库），在编排层 `model_refresh::generate_roots_report`：`E3dPersistReport` 新增 `source_sesno: Option<u32>` 与 `visited_by_root: HashMap<RefNo, Vec<RefNo>>`（生成服务只报名单），编排层按 `data_face().is_read_through()` 用 `e3d_collector::open_pinned_set` 钉同一版会话开 `DbSet`，每根 `write_through_root(visited ∪ 祖先链)` **排在 `persist_root` 之前**；三条失败路（源打不开 / 祖先链上溯失败 / 落库失败）全部只记 `read_through_degraded` 日志。`/dbnums` 顶层加 `data_face`（每行本来就带 `db_type`，plant-ui 计划 G3 其实早已满足） | §3.2 第 5 条写的「同一次 `persist` 调用里」：实际是同一次 `generate_roots_report` 调用、同一根的 `persist_root` 之前，语义相同；`E3dPersistReport.warnings` / `read_through_rows` 两格没加——回执走日志，`/health.data_face` 计数随 G4 一起做 |

**验证**

- `cargo test --lib` 全量 **1530 passed / 0 failed / 104 ignored**（09-04 基线 1511 / 4 failed，那四条已被别的会话收掉）。
- 新增护栏：`options::tests::{the_default_data_face_is_read_through, the_data_face_env_override_wins_over_config, an_unrecognised_data_face_is_ignored_but_never_silent, every_db_option_fixture_declares_its_store_mode_and_data_face}`；
  `read_through::tests::the_read_through_path_reuses_the_one_row_renderer_and_only_upserts`（源码钉：只走 `ElementRows::render` / `read_ele_data` / `UPSERT`，不许 `INSERT` / `CREATE pe` / `gen_sur_json(` / `pe_owner_edge(` / `applied_sesno` / `stream_baseline(`）；
  `model_refresh::tests::{write_through_precedes_root_publication, write_through_failure_never_fails_the_root}`。
- **G2 的验收**：新增 `tests/read_through_rows_parity.rs`（issue-019 真实 db8000 归档，sesno 26），**4 passed**：
  ① 64 个等距样本读透渲出的 `pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner` 行与 `stream_baseline` **逐字段相同，`pe.sesno` 在内**；
  ② 祖先链从任一元素上溯都以 WORL 收尾、链上每个都在基线里，`subtree_with_ancestors` 去重保序；
  ③ 真 `mem://`：别的库的 refno `foreign = 1` 不写、索引外的 refno 进 `missing`、写 N 个元素 `pe` 恰 N 行、WORL 的 `pe.sesno` 落库后仍是 last-touch、`pe_owner` 边在、重写一遍 `pe` / `pe_owner` 行数不变（UPSERT 幂等）、每张 `ATT_<noun>` 行数 = 该 noun 元素数。
- `vendor/e3d-model`：`cargo check --all-targets` 通过（`gen_ams` / `bran_audit` 解构补 `..`）；`gen_ams` 三份产物 SHA-256 对拍**没跑**（本机没有 ams 样例的 `--catalogue` 全套参数现成；`visited` 只多带一张名单，不改遍历顺序与几何）。
- `rustfmt --check --edition 2024` 对本轮新写 / 新改的文件通过；`ele_ops.rs` / `e3d_collector.rs` / `gen_ams.rs` 里既有的格式偏差不是本轮的，没动。
- **行尾**：`git ls-files --eol` 显示 `e3d_collector.rs` / `ele_ops.rs` / `e3d_mesh_store.rs` / `e3d_model_service.rs` / `handlers.rs` 在本轮开始前工作树里就是 CRLF（别的会话所为，索引仍是 LF）——本轮没有把它们改回 LF（正在被别的会话编辑），只把自己新建的三个文件钉成 LF。全量测试在当前行尾下是绿的。
- **未 live**：没有起过服务；`POST /model/ensure` 之后 `pe` 行出现、BRAN 的 `in.owner` 无 NONE 这两格（§5 G3 / §6 第 3–4 条）等 G6 的 `Verify-OutOfBox.ps1` 扩展一起跑。

**没做的**：G4（`element/ensure`）、G5（读透形态跳过摄入链、退化窗口、`initialization_required`）、G6（脚本 + P5）；plant-ui U0–U5。
**注意**：G5 落地前，默认形态（`spawned-mem` + `read-through`）启动仍会跑 DESI 基线（`startup_autorun` 默认 true）——基线 `INSERT IGNORE` 与写透 `UPSERT` 落的是同一形状的行，共存无冲突，只是「零摄入」这一半还没兑现。
