# 开发计划：启动链真正懒加载（母计划阶段 4，捎带 3B/3C 与阶段 5 的最小定向刷新）

- 日期：2026-09-12
- 状态：**已批准**（2026-09-12，用户逐项确认；§8 六项拍板全按推荐项执行）；未实施。
  开工前置：跨库属性切片 W4–W9 合入、L0（AMS 改前基线）采完
- 母计划：`docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md` 阶段 4（§6），
  兼并阶段 3B/3C（树脱骨架）与阶段 5 的最小形态（按 dbnum 轮询会话号），理由见 §0.3
- 前置切片：`docs/plans/2026-09-12-cross-db-attribute-refno-routing-dev-plan.md`（W1–W3 已提交
  `bb16105d2` / `5cc5e3e04`；本切片要求其 W4–W6 先合入——属性 / 树两个服务的 **locator** 已经不靠骨架，
  本切片删的是它们剩下的 `ensure()` 与启动链的预热）
- 调试 / 验收项目：AMS（E3D 3.1 `AvevaMarineSample` / MDB `ALL`，28 个 DESI/ISOD，2026-09-05 实测骨架
  611,345 元素 / 0.72 s / 19.8 MiB）。JEU（417 库 / 1,221,858 元素 / 68 s）本机不可达，只作旧成本记录。
  AMS 上「HTTP 不等骨架」体感不明显，**验收全靠计数器与日志断言**，不靠计时。

## 0. 现状与为什么要一起做

### 0.1 启动链上现在还在建骨架的地方（`room_membership=false`）

| 位置 | 做了什么 | 后果 |
|---|---|---|
| `lib.rs` `run_cli` :964–995 | `init_mdb` 之后无条件 `mdb_memory_store::ensure()`，失败只告警 | ingest 面启动阻塞在全 MDB 头扫描 |
| `lib.rs` `run_read_through_core` :2000–2008 | 先 `tokio::spawn(serve_if_configured)`，**再**无条件 `ensure()`，`snapshot.describe()` 进横幅 | HTTP 任务已 spawn，但见下一行 |
| `web_service/mod.rs` `serve` :338–344 | `Router::new()`（:369）与 `bind_with_handoff`（:473）之前 `element_attributes.get().await` | 属性服务构造 → `ElementAttributeService::open` → `ensure()`：与上一行**并发建两份**同一骨架（"双重预热"），端口要等它 |
| `lib.rs` `run_read_through_core` :2010–2024 | `InitializationCoordinator` 的 `mark_data_ready_without_manifest` / `open_model_phase` / `mark_model_ready` 排在 `ensure()` 之后 | `/health.initialization` 的 model ready 被骨架挡住 |
| `lib.rs` :2026–2104 | 每 2 s `mdb_memory_store::refresh_if_stale()`：任一库 sesno 变 → `MdbMembership::resolve` + `MdbMemoryStore::load` **整份重建**；`snapshot.contains(root)` 判删根；`invalidate_all_receipts` 全量作废 | 一个 DESI 的 SAVEWORK = 重扫全 MDB |
| `lib.rs` `bootstrap_read_through_rooms` :1785 | 只在 `room_membership=true`；从 `snapshot.databases()` 取 DESI 列表，逐库 `model_dbnum_ensure::start_and_wait` | 它要的只是 dbnum 列表，不是元素表 |
| `direct_tree.rs` :55 / :129 / :185 / :234 / :246 | `ensure()`；`roots()` 用 `refnos_of_noun(dbnum, "WORL")`；`contains`；`ancestors`；`dbnum_of` | 树服务是最后一个**必须**骨架的调用方 |
| `element_attributes.rs` :157 / :183 | `ensure()` 只为 `uda_catalog()`（W5 之后 locator 已换） | W5 落地后这里只剩 UDA |
| `read_through.rs` :477 | `ensure()` 只为 `uda_catalog()` | 同上 |
| `handlers.rs` :700–719 / :837–848 | `/health.core` 的 `status / snapshot_epoch / session_vector / stale_reason / skeleton_elements` 全部来自 `mdb_memory_store::get()` + `stale_reason()` | 骨架不建，health 永远 `unavailable` |
| `handlers.rs` :2096 / :2113 / :2210 / :2358 | `/dbnums.ref0s` 来自 `service.skeleton().ref0s_of(dbnum)` | 骨架不建，`/dbnums` 要么 `ensure()` 要么没 ref0s |
| `direct_tree.rs` :376 / `element_attributes.rs` :801 | 两个 provider 的 `get()` 先看 `mdb_memory_store::stale_reason()`，再比 `session_fingerprint` | 新鲜度判据是骨架指纹 |

### 0.2 已经不需要动的

- HTTP 地址后备 `http_api_addr -> server_release_ip -> 127.0.0.1:9099`（`web_service/mod.rs` :41、:278）已完成。
- 两个 provider 各自有 `reload: Mutex` 做重建单飞（`direct_tree.rs` :385、`element_attributes.rs` :811）；缺的是**跨 provider**
  与**启动链**三方共享同一份昂贵资源的单飞，不是 provider 内部。
- SYS/CURD 解析按 NAME 精确查 MDB；`mdb_membership.rs` :405 那次 SYS `scan_elements(Named)` 只在 `mdb_name` 拼错的
  错误路径上跑。
- e3d-io 侧读成本计数器 `read_stats::{database_opens, scan_calls, scanned_elements, index_point_lookups}` 已在（阶段 0）。

### 0.3 为什么 3B/3C 与阶段 5 最小形态要跟阶段 4 同一个包

- 只删启动预热、不给树脱骨架：`DirectTreeService::open` 的 `ensure()` 还在，第一次点树照样扫 61 万 / 122 万元素——
  这正是母计划 §10-4 拒绝"临时包"的理由（把 68 秒从启动挪到第一次点击）。所以 3B/3C 必须与阶段 4 同包。
- 只删预热、不改 2 s 循环：`refresh_if_stale()` 在第一次 DESI 保存时会把整份骨架**建回来**，启动省下的又回来了。
  所以循环必须在本切片改成按 dbnum 轮询（阶段 5 的最小形态）；阶段 5 剩下的（`DbSet` 缓存定向清理指标、CATA/DICT
  依赖处理、`/health` 最近定向刷新字段）另起切片。
- 骨架彻底没有调用方之后，`/health.core` 的 `status` 若还看 `mdb_memory_store::get()` 就永远 `unavailable`，
  所以健康接口的 core 段必须在本切片换源（阶段 6 只剩删字段与删模块）。

树首页不需要扫库找 WORL：2026-09-12 全语料实测（1,947 库）每库恰好一个 WORL、owner 为 0，全部 1,905 个非 SYS/COMM/MISC 库
的 WORL 都在 `RefNo(descriptor.primary_extent_ref, 0)` 即 `(dbno | 0x2000) / 0`，0 例外（跨库属性切片 §10.2）。

## 1. 目标 / 非目标

**目标**
1. `room_membership=false` 冷启动：HTTP bind 之前不构造任何 provider、不读任何 DESI 元素；启动期 `read_stats.scan_calls == 0`。
2. 启动链、树服务、属性服务、read-through、`/health`、`/dbnums` 全部不再调用 `mdb_memory_store::{ensure, get, refresh_if_stale, stale_reason}`。
3. 树：`roots()` 每库 1 次 WORL 点查 + 1 次成员表；`ancestors()` 走 `DbElement::owner()` 逐级跨库；`contains` 走 `element_exists`；
   `dbnum_of` 走 `refno_dbnum()`。
4. 文件变化只让对应 dbnum 的 `DbSet` 槽、会话与已生成 roots 失效；不重建任何全 MDB 结构。
5. `/health.core` 报路由目录状态、路由库数、已开库数、缓存元素数、stale/failed dbnum、最近一次定向刷新；`read_stats` 快照进 `/health`。
6. 首次并发请求对共享读上下文（schema / membership / 路由目录 / UDA catalog）single-flight。

**非目标**
- 不删 `MdbMemoryStore` 模块、不删 `/health.core.skeleton_elements` 字段（阶段 6；字段本切片值为 0）。
- 不做 `DbSet` 元素缓存的 LRU；不做 CATA 变化按依赖 roots 的精确失效（阶段 5 余下部分）。
- 不做全局 NAME 搜索的后台索引任务（`search_names` 仍是显式请求时逐库 `indexes()`）。
- 不动单根生成的 `scan_index`（母计划阶段 7，已定为独立阶段）。
- 不改 `room_membership=true` 的房间语义，只换它取 dbnum 列表的来源。

## 2. 不变量落点（母计划 §4）

| # | 不变量 | 本切片落点 |
|---|---|---|
| 6 | 按需成本 | 启动 `scan_calls == 0`；树首页 `database_opens == 路由库数`、`scan_calls == 0`；护栏见 §5.2 |
| 7 | 定向失效 | `DbGeneration` 按 dbnum 记 `(FileIdentity, latest_sesno, base_sesno)`，变化类型区分前进 / 回滚 / 换文件；回滚有两种形状——会话号变小（文件恢复）与会话号变大但 `base_sesno` 低于上次会话号（BACKTRACK 后 SAVEWORK），见 §3.4 规则表 |
| 9 | 全量操作显式 | `room_membership=true` 的启动房间任务按库流式，进度进 `/health.initialization`；NAME 搜索仍逐库显式 |
| 10 | HTTP 不等数据预热 | `serve()` 里 provider 构造移到 bind 之后的后台任务或首次请求；`/health`、`/setup.html` 立即可用 |

## 3. 设计

### 3.1 共享读上下文与单飞

```rust
/// 进程内一份，首次请求时建，后续克隆 Arc。建的是"目录级"东西，没有任何元素。
pub struct SharedReadContext {
    pub schema: Arc<DirectSchema>,            // attlib + 模板目录（open_from_env 一次）
    pub membership: Arc<MdbMembership>,       // SYS/CURD
    pub directory: Arc<MdbRouteDirectory>,    // dbnum → 文件；含 DbGeneration 表
    pub uda_catalog: Arc<UdaCatalog>,         // 只读 DICT
}
static SHARED: tokio::sync::OnceCell<anyhow::Result<Arc<SharedReadContext>>>;
pub async fn shared_read_context() -> anyhow::Result<Arc<SharedReadContext>>; // OnceCell::get_or_try_init
```

- `DirectTreeProvider::get()` / `ElementAttributeProvider::get()` 都先 `shared_read_context().await`，再各建自己的服务；
  两个 provider 与任何后台任务并发首击只会建一份 schema / membership / 目录 / UDA。
- 失败不缓存成功位：`get_or_try_init` 的 `Err` 不会被记住，下次再试（与 provider 现有"下次重试"语义一致）。
- `SharedReadContext` 的新鲜度 = 目录 `DbGeneration` 表 + DICT 文件身份；provider 的 `is_current()` 改比"本服务已触达 dbnum
  的 generation 集"，不再比骨架指纹（跨库属性切片 3A 已定方向）。

### 3.2 `serve()`：先 bind，再一切

- 删 :338–344 的 `element_attributes.get().await`。
- provider 仍在 `AppState` 里以惰性形态存在（已是 `DirectTreeProvider::new` / `ElementAttributeProvider::new`，零 I/O）。
- bind 成功、`axum::serve` 起来之后，**可选**后台 `tokio::spawn(shared_read_context())` 预热目录级上下文（毫秒级，不读元素）；
  由 `DbOption` 新键 `warm_read_context_after_bind`（默认 `true`）控制，方便测试关掉。
- `/health` 在上下文未建时返回 `core.status = "warming"`，不是 `unavailable`；`/setup.html` 与 `/api/v1/health` 都不依赖它。

### 3.3 `run_cli` / `run_read_through_core`

- 删 `run_cli` :967–995 与 `run_read_through_core` :2004–2008 的 `ensure()`；横幅改打 `MdbRouteDirectory::stats()`
  （路由库数 / 元素库数 / 声明无文件数 / 问题数）与 `read_stats::snapshot().summary()`。
- `InitializationCoordinator` 三步（`mark_data_ready_without_manifest` → `open_model_phase` → `mark_model_ready`）紧跟
  `init_mdb` 之后，不等任何元素读取。
- `room_membership=true`：`bootstrap_read_through_rooms` 的签名从 `&MdbMemoryStore` 改为 `&MdbRouteDirectory`，
  dbnum 列表取 `directory.element_databases()`（CURD 序）；每库照旧 `model_dbnum_ensure::start_and_wait`
  （它自己的 `scan_index` 是显式整库操作，允许）；进度照旧进 `/health.initialization`。

### 3.4 2 s 循环 → 按 dbnum 轮询会话号（阶段 5 最小形态）

```rust
pub struct DbGeneration {
    pub generation: u64,
    pub identity: FileIdentity,
    pub latest_sesno: u32,
    /// 最新会话页的 BASESN（e3d-io `SessionPage::tail.base_sesno`）：这一会话建立在哪个会话之上；0 = 无。
    pub base_sesno: Option<u32>,
    /// 最新会话页的 UDATE（3.1 引擎写的会话才有）或 DATE，进 `/health` 当人读时间。
    pub written_at: Option<SessionDate>,
}
pub enum GenerationChange { SessionAdvanced, RolledBack, FileReplaced }
impl MdbRouteDirectory {
    /// 只读每个路由库的 60 字节头 + 会话页；返回变化的 dbnum。不读元素。
    pub fn poll_generations(&self) -> Vec<(u32, DbGeneration, GenerationChange)>;
}
```

**`classify` 的规则表（2026-09-12 定稿，含 BASESN 一条）**——上一版只看会话号：

| 观察到 | 判定 | 依据 |
|---|---|---|
| `sesno` 变小 | `RolledBack` | 文件被整份恢复 / 换成旧备份 |
| `sesno` 变大，且新会话页 `base_sesno` **< 上次观察到的 `sesno`**（`base_sesno != 0`） | **`RolledBack`** | E3D 里 BACKTRACK 到旧会话再 SAVEWORK：会话号照涨，内容却回到 `base_sesno` 那一刻。3.1 `ams8000_0001` 第 243 会话 `BASESN=239`，链上其后保留的是 242，而它的 `REASON` 是 **0**（普通 SAVEWORK）——**REASON 不标回退，只有 BASESN 标**（e3d-io `tests/session_tail_real.rs` 实测，2026-09-12） |
| `sesno` 变大，`base_sesno` ≥ 上次 `sesno` 或为 0 | `SessionAdvanced` | 直叠（= 上次）、跨了没看见的会话（> 上次）、或 3.1 `Saving Meta Data` 那种无 base 的系统会话（reason 15，`BASESN=0`） |
| `sesno` 不变、`identity` 变 | `FileReplaced` | 同号会话在另一个文件里 |
| 都不变 | 无变化 | — |

只看会话号时第二行会被当成 `SessionAdvanced`，(a)(b)(c) 只作废「上次 sesno 之后新写的」那部分，而 BACKTRACK 改掉的是 `base_sesno..=上次 sesno`
之间已经生成过的东西——`RolledBack` 的处置（该 dbnum 已生成 roots 全部按新会话重生成、旧凭证不作覆盖）才对。
**跳过多个会话时**：新会话的 `base_sesno` 只说它自己踩在谁上，中间被跳过的会话里可能藏着一次回退（240 基于 230，我们只看见 239 → 241）；
`poll_generations()` 发现 `sesno` 一次前进超过 1 时，沿会话链从最新页往回读到上次观察到的会话号为止，任一页 `base_sesno` 低于它链上的
前一页即判 `RolledBack`——成本是新增会话数那么多页，不读元素。链读用 e3d-io `session::walk_chain` 或逐页 `previous`。

**账本已落地**（2026-09-12，`src/data_interface/db_generation.rs`，15 条单测全绿）：`FileIdentity::of(path)`（只 stat）、
`Observed { identity, latest_sesno, base_sesno: Option<u32> }`、`classify(previous: Observed, current: Observed) -> Option<GenerationChange>`
（**上表五行已全部实现**，含 BASESN 那一行；单测有「上次 242、新页 243 基于 239 → RolledBack」「基于 0 / 短页没说 → 前进」「会话号不变或变小时 BASESN 不改判」）、
`GenerationLedger { observe_session(dbnum, Observed) -> Observation::{First, Unchanged, Changed{from,to,kind}}, observe(dbnum, identity, sesno)（= base 为 None 的旧口径，
判不出 BACKTRACK 后的 SAVEWORK）, generation_of, epoch()（之和，§8-3 的 `snapshot_epoch` 来源）, forget, iter }`，`DbGeneration { generation, identity, latest_sesno, base_sesno }` /
`GenerationChange` 可 `serde::Serialize` 直接进 `/health`。**`mdb_route_directory::poll_generations` 目前走的是不带 BASESN 的 `observe`**（读 `DbFileHeader::read(..).latest_sesno()`）——
换成 `observe_session` 只差把最新会话页的 `tail.base_sesno` 一起交进来（e3d-io `SessionPage::tail`，`c7cc6c8`；gen-model `SessionStamp` 尚未透出这一字）。
L4 只需让 `MdbRouteDirectory` 持一份 `GenerationLedger`，`poll_generations()` = 对每条路由 `FileIdentity::of` + 读最新会话页
（e3d-io `SessionPage`：`sesno`、`tail.base_sesno`、`utc_date`/`date`，均已在 e3d-io `c7cc6c8` / `dc80fb8` 落地）
→ `ledger.observe(...)`，收 `Changed` 的那几条。`direct_store::FileIdentity` / `model_source::FileStamp` 两个私有同构类型接线时收敛到它；
gen-model `db_file_header::SessionStamp` 自己的 stamp 拆法与 e3d-io `SessionDate` 字字相同（IDA `DB_DateTime::set` 已证），接线时删掉改用 e3d-io 的。

- 循环体：`poll_generations()` → 对每个变化 dbnum：(a) 共享 `DbSet` 清该 dbnum 的槽与元素缓存（e3d-io 已按 `refno.dbno()` 清库）；
  (b) `ModelMemoryStore` 只取**该 dbnum** 的已生成 roots，`element_exists` 判删根 → 删子树；活根 → `generate_roots_report`；
  (c) `invalidate_all_receipts` 改为按 dbnum 作废（若 `ModelMemoryStore` 没有按库接口，本切片先加最小的一个）；
  (d) 记 `last_targeted_refresh = { dbnum, from, to, kind, at }` 供 `/health`。
- 轮询周期沿用 2 s；每轮读 N 个头（AMS 28 / JEU 417 个 60 字节读 + 一页会话页），成本与库数成正比、与元素数无关。
- 母计划阶段 5 余下的：CATA/DICT 变化的依赖失效、`DbSet` 缓存命中 / 条目 / 按 dbnum 失效指标、低频 header 漂移审计——另起切片。

### 3.5 树脱骨架（3B/3C）

- `DirectTreeService { directory, store: DirectStore(locator = directory), set: Arc<DbSet>(resolver = directory) }`，删 `skeleton` 字段。
- `roots()`：`for route in directory.element_databases()`：`world = RefNo::new(route.dbnum | 0x2000 /* primary_extent_ref */, 0)`
  → `set.element_exists(world)`（不存在 → 记问题、跳过该库，不 panic）→ `store.members_in(dbnum, world)` → SITE 过滤。
  `primary_extent_ref` 优先从已开库的 `descriptor()` 读，未开库时用位公式；两者不一致时报错（不变量 3 的树版）。
- `ancestors(start)`：`set.element(start)` → 逐级 `owner()`；跨库由 resolver 自动开目标库；链外（`DbNotRegistered`）响亮失败。
- `generation_roots_in_subtree` 里的 `skeleton.contains(root)` → `set.element_exists(root)`；`dbnum_of` → `refno_dbnum()`。
- e3d-io 侧**已加**（2026-09-12，未提交）：`DbDescriptor::world_refno()`（= `RefNo::new(primary_extent_ref, 0)`）、
  `ReadOnlyEngine::world() -> Result<Option<ElementRecordView>>`（一次点查 + 核 noun 为 `WORL` 且 owner 为空，否则
  `E3dReaderError::NotAWorld`；索引里没有该键 → `None`）、`ElementRecordView::header()`（从 `raw_bytes` 重读 11 字头）。
  测试：`tests/world_refno_synthetic.rs`（合成库：命中 = 1 开库 / 0 扫描 / 1 点查；SITE 冒充、有主 WORL、无元素 0 三种拒绝）、
  `tests/world_refno_corpus.rs`（3 个真实样本 + 受 `E3D_CORPUS_ROOTS` 控制的全语料门：1,947 库 / 42 个 SYST·COMM·MISC
  例外 / 其余全部命中，38.9M 元素扫描交叉验证）。gen-model 的 `roots()` 只调 `engine.world()` / `set` 上的等价物。

### 3.6 `/health.core` 与 `/dbnums`

- `core`：`status`（`warming | ready | degraded`）、`route_directory: { generation, routes, element_routes, declared_without_file, problems }`、
  `open_databases`（`DbSet` 已开槽数）、`cached_elements`（`DbSet::with_element_cache` 条目数）、`stale_dbnums` / `failed_dbnums`、
  `last_targeted_refresh`、`read_stats`（`snapshot()` 全字段）；`snapshot_epoch` 改为目录 generation 之和或最大值（前端只当"变了没"用），
  `session_vector` 改为 `directory` 每个元素库的 `latest_sesno`；`skeleton_elements` **本切片保留键、恒为 0**，阶段 6 删。
- `/dbnums.ref0s`：`FileInfo::extent_refs(descriptor)`（e3d-io 已有）——打开描述符页 + page 1，不扫元素；`annotate_ref0s` 的闭包换源即可。

## 4. 工作项与顺序

| # | 工作项 | 文件 | 验收 |
|---|---|---|---|
| L0 | **AMS 3.1 改前基线**（跨库切片 W0-d）——必须在 L2 之前采，量的就是 `ensure()` 的旧成本 | `docs/baselines/2026-09-12-ams-refno-routing-stage0.json` | JSON 无 `null`；含 `read_stats` 差值、骨架耗时、RSS、首属性、首根 |
| L1 | `SharedReadContext` + `shared_read_context()` 单飞 | `data_interface/shared_read_context.rs`（新） | 单测：100 个并发首击只建 1 份（计数器）；首次 `Err` 不被缓存 |
| L2 | `serve()` 删 bind 前 `get()`；bind 后可选预热；`warm_read_context_after_bind` 键 | `web_service/mod.rs`、`options.rs`、`setup.html`（若键进设置页） | 护栏：`serve(` 段落里 `.get().await` 不在 `Router::new()` 之前；`bind_with_handoff` 之前无 provider 调用 |
| L3 | `run_cli` / `run_read_through_core` 删 `ensure()`；横幅换 `RouteDirectoryStats` + `read_stats`；初始化三步前移；`bootstrap_read_through_rooms` 换 `&MdbRouteDirectory` | `lib.rs` | 护栏：两函数段落无 `mdb_memory_store::ensure(`、无 `scan_elements(`；`room_membership=true` 的现有 l3 / room E2E 全绿 |
| L4 | `poll_generations()`（`MdbRouteDirectory` 持 `GenerationLedger`）；2 s 循环改按 dbnum；`poll_generations` 改走 `observe_session` 把 `BASESN` 交进账本 + 跳过多会话时沿链回看 | `db_generation.rs`（**已落地**，纯内存 + 15 单测，含 BASESN 行）、`mdb_route_directory.rs`、`lib.rs`、`model_memory_store.rs`（按 dbnum 作废） | 三种 `GenerationChange` + BASESN 回退 / 基于 0 前进 单测已绿；补：跨多会话沿链回看抓到中间一次回退；接线后加：合成两库夹具改一库会话页只有它进结果；循环护栏：不含 `refresh_if_stale(`、`invalidate_all_receipts(` |
| L5 | 树脱骨架（3B/3C） | `direct_tree.rs`、`e3d-io` `meta/mod.rs`（`world_refno`）+ 新语料门 | direct tree TTY 对拍逐字相同；`roots()` 期间 `scan_calls == 0`、`database_opens == 元素库数`；`ancestors` 跨库用例 |
| L6 | `/health.core` 换源；`/dbnums.ref0s` 换 `extent_refs` | `handlers.rs`、`docs/specs/web-service-api.md` | `refno_native_api_contract.rs` 仍绿（键集不变，`skeleton_elements` 恒 0）；新增 `read_stats` / `route_directory` 键的契约 |
| L7 | `read_through.rs` :477 与 `element_attributes.rs` 剩余 `ensure()` → `shared_read_context().uda_catalog` | `read_through.rs`、`element_attributes.rs` | 护栏：生产源码（`mdb_memory_store.rs` 自身与测试除外）不含 `mdb_memory_store::` |
| L8 | 护栏测试与 AMS 验收脚本 | `tests/refno_native_guards.rs`（追加）、`scripts/Run-LazyStartupAcceptance.ps1`（新） | §5.2 / §5.4 |
| L9 | 文档：母计划阶段 4/5 状态、`CONTEXT.md` 词条（"启动预热"退役、"路由目录 generation"新增）、`web-service-api.md` | 文档 | — |

顺序：L0 → L1 → L2 → L3 → L4 → L5 → L6 → L7 → L8 → L9。L2/L3 落地后到 L5 之前，第一次点树仍会扫全 MDB——**这段区间不出包**。
L5 依赖跨库切片 W6（树的 locator 已换目录）。

## 5. 测试

### 5.1 单元
- L1 单飞计数；L4 三种变化类型（含 BASESN 回退：上次 242、新页 243 基于 239 → `RolledBack`；基于 0 → `SessionAdvanced`）；L5 `roots()` 对合成多库夹具（复用 `refno_native_stage0.rs` 的写法造 2–3 个库，各含 WORL/SITE）。
- 2 s 循环对夹具：改一个库的会话页 → 只有该 dbnum 进 `poll_generations()` 结果；另一个库的 `DbGeneration` 不变。

### 5.2 源码护栏（`tests/refno_native_guards.rs` 追加；`include_str!` 文本 + `read_stats` 行为）
- `lib.rs` 的 `run_cli(` / `run_read_through_core(` / `bootstrap_read_through_rooms(` 段落：无 `mdb_memory_store::`、无 `scan_elements(`。
- `web_service/mod.rs` 的 `serve(` 段落：`.get().await` 与 `shared_read_context()` 只能出现在 `bind_with_handoff(` 之后。
- 2 s 循环段落：无 `refresh_if_stale(`、无 `invalidate_all_receipts(`；含 `poll_generations(`。
- `direct_tree.rs`：无 `MdbMemoryStore`、无 `refnos_of_noun(`、无 `mdb_memory_store::`。
- `handlers.rs`：无 `mdb_memory_store::get(`、无 `.ref0s_of(`；含 `extent_refs(`。
- 行为：夹具上 `serve()` 起来到 `/health` 200 之间 `read_stats.scan_calls == 0`（进程内测试，用 `tokio::spawn(serve)` + 客户端）。

### 5.3 现有回归（全绿）
`web_service::tests`、direct tree TTY 对拍、direct attribute/UDA parity、model generation root parity、CATA closure、
MDB resolver、E3D 2.1/3.1 schema pack、room membership 开 / 关、`refno_native_api_contract.rs`、l3 suite。

### 5.4 AMS 验收（`room_membership=false`，E3D 3.1 `/ALL`；数字靠计数器，不靠计时）
1. 冷启动：`/api/v1/health` 200 且 `core.status ∈ {warming, ready}`；启动日志无「DESI 骨架」；`/health.core.read_stats.scan_calls == 0`；
   `route_directory.element_routes == 28`；`open_databases == 0`（预热只建目录）。
2. 点开树根：`open_databases == 28`（每库 1 次开库），`scan_calls` 仍为 0；SITE 列表与改前 TTY 对拍一致。
3. 读 DESI 8000 一个 BRAN 的属性：`database_opens` 增量 = 1 + 依赖数；`scan_calls` 不变；值与 TTY 相同。
4. `/model/ensure` 一个根：`database_opens` 只增目标库与依赖；`scan_calls` 增 1（`scan_index`，母计划阶段 7 候选，在案）。
5. `db8000_bran_ftub_move_apply.mac` + SAVEWORK：2 s 内 `/health.core.last_targeted_refresh.dbnum == 8000`，7998 的 generation 不变；
   模型更新到新 sesno；`restore.mac` 后属性 / owner / 几何恢复，apply + restore 净变化为空。
6. 与 L0 基线对比：启动期 `scanned_elements` 从 611,345 → 0；RSS 启动后不再有骨架的 ~20 MiB。

## 6. 发布与回滚

- 本切片完成并通过 §5 后，才按母计划 §8 第 5 条出新 build_id 的包（0.1.23 dirty 包不覆盖）。
- 不加"回退到骨架"的开关：两条路并存正是要消除的东西；回滚 = 回退本切片提交。
- 发布前保留原始构建与配置备份（母计划 §11 假设与边界）。

## 7. 风险

- **首击延迟**：删预热后第一次树 / 属性请求要开目标库（AMS 每库几页；JEU 417 库首页 = 417 次开库）。缓解：bind 后可选预热目录级上下文；
  树首页按库并行开（有界并发，如 8）。不回到启动阻塞。
- **`snapshot_epoch` 语义**：前端把它当"数据变了"的信号；改为目录 generation 之和后，DESI 保存会改变它（比骨架版更准），
  但 DICT/CATA 变化不再改变它——`UdaCatalog` 的 DICT 身份纳入 `route_directory.generation`。
- **`ModelMemoryStore` 按 dbnum 作废**：若没有现成按库接口，L4 加最小的一个；不能退回 `invalidate_all_receipts`。
- **房间 true 档**：`bootstrap_read_through_rooms` 只换 dbnum 列表来源；`model_dbnum_ensure` 内部照旧，风险低，但 l3/room E2E 必跑。
- **并行会话**：跨库切片 W4–W9 正在同一工作树推进；本切片动 `lib.rs` / `direct_tree.rs` / `element_attributes.rs` / `handlers.rs`，
  必须等那边合入后再开 L2 之后的工作项；L0 / L1 可先做。

## 8. 评审拍板（2026-09-12 已定，全按推荐项）

1. 3B/3C 与阶段 5 最小形态**并入本切片**，一个包发（理由 §0.3）。
2. bind 后**默认预热**目录级上下文，`warm_read_context_after_bind=true`；关掉只为测试"纯懒"路径。
3. `snapshot_epoch` = 目录各库 generation **之和**；`docs/specs/web-service-api.md` 同步一句"变了就变，不再等于骨架 epoch"。
4. 树首页每库开库**有界并发 8 路**。
5. `ModelMemoryStore` 按 dbnum 作废 receipts 的接口**由本切片加最小实现**，阶段 5 完整版在它之上扩。
6. `/health.core.read_stats` **暴露 e3d-io 计数器全字段**（`ReadStats` 直接序列化）。

另：单根生成的 `scan_index` 单库全扫已定为母计划**阶段 7**，独立阶段，不进本切片（跨库切片 §9 Q16）。
