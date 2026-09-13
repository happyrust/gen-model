# 开发计划：跨库属性改走 RefNo 原生路由（母计划阶段 1–2 + 3A 的执行切片）

- 日期：2026-09-12
- 状态：Plannotator 已批准（2026-09-12，无附注）；§8 四项拍板按推荐项执行；**§9 十五项口径已于
  2026-09-12 与用户逐题对齐**（本文件为唯一记录，母计划同步）
- 进度（截至 2026-09-12 13:25，分支 `refactor/retire-publish-stage`）：
  - W1+W2 已提交 `bb16105d2`（`refno_route.rs`、`mdb_route_directory.rs`）；
  - W3 已提交 `5cc5e3e04`（`DirectStore` 改位解码，删 `UnresolvedRef0`，加 `DbNumberMismatch`，
    真库门 `direct_store_refno_live_*` 在 AMS `/ALL` DESI 1112 上通过）；
  - W4+W5+W6+W8 已提交 `425ca98d8`（`dbnum_of_root` 位解码 + 目标库一次点查，删 `DBNUM_BY_REF0`；
    属性服务与树服务的定位器改从 `MdbRouteDirectory::from_membership` 造；`tests/refno_native_guards.rs`
    六条文本护栏）。提交树单独检出 `cargo check --lib --tests` 通过；`direct_store::tests::installed_e3d_*`
    三条在基线 `e22023492` 上同样红（依赖另一会话未提交的 schema 选择改动），与本切片无关；
  - **端到端已验**（2026-09-12 11:34 起，隔离副本 `D:\work\plant-code\old\_runs\ams-refno`，
    `store_mode=spawned-rocksdb` / `19099` / `watch_dbnums=[7998,8000]`，`build_id 0.1.23+g5cc5e3e04973…dirty`
    = HEAD `5cc5e3e04` + 工作树里的 W4–W6）：`/health` `initialization.status=model_ready`、
    `startup.reconcile.result=ok`、design 2/2 与 meta 13/13 阶段零失败零阻断、`dbnum_consistency 2/2 in_sync`、
    7998 `[12,12]` / 8000 `[444,444]` 数据与模型水位一致；plant-ui（服务供数，`PLANT_READ_FACE=service`）
    连上 `ns 1516` 根层 38 个 SITE，展开 `/1RX03-EQUI` 三个 ZONE 逐个点眼按需生成并显示
    （`/1RX03-CASE-DQ` 23/26 根、23 个网格实例，服务侧 `model-persist failed_roots=0`）；
    整 SITE `/1RX03-EQUI`（1,978 元素）也显示成功，但 `model/ensure` 走满 120 s 回 504 后由客户端
    后台补齐——在案，不属本切片；
  - W7 已提交 `7f501ab0d`（探针退役全库扫描定位器、加对账栏；ams8000 真库 1313/1313 相等、338 条跨库引用 0 失败）；
  - W4 补齐 `db49238e3`（并行会话，13:43）：`element_exists` 点查 + 核头、`RootDbError` 六类、`mdb_design_dbnums` DESI+ISOD、
    护栏加 `read_stats` 行为断言（8 passed）；
  - W9 已提交 `d9b631fa2`：`docs/evidence/2026-09-12-ida-core-dabacon-refno-routing.md`（core.dll 取证原文）、CONTEXT.md
    「RefNo 库归属 / dbnum 路由目录 / 跨库属性」三条新词条 + Ref0 三条退役、母计划状态与阶段 1 / 2 / 3A 结果段；
  - 只剩 W0(d)(e)（AMS 阶段 0 基线 JSON、contract 注释）未做；本切片其余工作项全部提交。
- 母计划：`docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`（已批准；阶段 0 已落地：
  `vendor/e3d-io/tests/refno_dbno_corpus.rs`、`refno_native_stage0.rs`、`tests/refno_native_api_contract.rs`、
  `docs/baselines/2026-09-12-jeu-refno-routing-stage0.{json,md}`；阶段 0 退出门「位解码 corpus 零分歧」
  已在本机全部 E3D 2.1/3.1 语料上实测通过，见 §10）
- 调试 / 验收项目：**AMS**（`DbOption.toml`：E3D 3.1 `AvevaMarineSample` / MDB `ALL` / `e3d_version="e3d31"` /
  `store_mode="spawned-mem"` / `room_membership=false` / `watch_dbnums=[7998, 8000]`）。JEU 在另一台机器上，
  本机不可访问；母计划里 JEU 的 417 库 / 1,221,858 元素 / 68 s 只作旧成本记录，本机不得估算填充。
- 本切片范围：把**跨库属性读取**（引用属性 `SPRE/CATR/LSTU/PSPE` 换库、`OWNER` 跨库上溯、生成根定库）
  从「ref0 全局反查表」改成「RefNo 位解码 + dbnum 目录点查」。不碰启动链懒加载（阶段 4）、
  按 dbnum 失效（阶段 5）、健康接口与旧模块删除（阶段 6）。

## 0. 为什么现在做、做成什么样

### 0.1 core.dll 的做法（ida-bridge 只读取证，2026-09-12）

母计划 §2 记的是 C++ 层；本次把 DABACON（Fortran）层也打通了，两层一致：

| 环节 | E3D 3.1 `core.dll`（`D:\ida_scratch\plant3\core.dll.i64`） | 结论 |
|---|---|---|
| 引用属性取值 | `DB_Element::getElement(DB_Attribute*,int)` `0x5939c60`；`dabGetAtt(int, DB_Element&)` `0x592c7f0` → opcode 486 | 只把记录里两个 word 拷进 `DB_Ref`，**不开库不查表**，返回惰性句柄 |
| refno → dbno | `DB_Element::dbno` `0x5931280` → `db_get_dbno_from_reference` `0x5aade70` → opcode 58 → `sub_5B05260` | 全文一行：`w0 & 0x1FFF \| (w0 >> 13) & 0x3E000`；2.1 `sub_1064EF60:22` 同式 |
| dbno → DB 对象 | `DB_Element::getDB` `0x5939520` → `DB_DB::findDB(int)` `0x5902e80` | `DBLookup_` 红黑树按 dbno 键 |
| dbno → 打开态 | `elGotoCPP` `0x59316d0` → opcode 474 `sub_5ACF400` → `sub_5AEB6B0:56` 解 dbno → `sub_5AF6600` → `sub_5AF5820` → `sub_5AF58E0` | 在有序表 `dword_6A54190`（`[slot, dbno]`，长度 `dword_6A54180`）上**二分**；库没开 → 错 41 `"Dbno is %d"`；槽全无效 → 错 635 |
| 目标元素 | `sub_5AEB6B0:181–182` | 目标库索引 B+tree 点查，页地址 `w1 & 0x1FFF` / `(w1>>13) & 0xFFF` |
| 库怎么进表 | `DB_MDB::internalOpenDB` `0x10559680`（2.1）读 MDB 定义 `DBNO/ISTYP` 开单库；`DB_MDB::getDB(int)` `0x59f4e10` 按向量位置取 | MDB 只决定「开哪些库」，**没有任何 ref0→dbnum 全局表** |

### 0.2 我们现在的两条路

- **路 A（与 core 同构，已存在）**：`vendor/e3d-io` `RefNo::dbno()`（`refno.rs:17`）+ `DbSet::db_slot()`（`db_element.rs:422`）
  未命中才问 `DbFileResolver::resolve(dbno)`，开库后核对 `descriptor().db_mark == dbno`；
  `DbElement::get_element()`（`:939`）/ `owner()`（`:590`）都是零 I/O 句柄，首次访问按 `dbno()` 重路由。
  已有 resolver：`E3dDbResolver`（`e3d_model_service.rs:491`）、`SysMetadataResolver`（`mdb_membership.rs:866`）。
- **路 B（要退役）**：`DirectStore::dbnum_of()`（`direct_store.rs:624`）→ `CataDbLocator::resolve_ref0(get_0())` →
  `InMemoryCataLocator.ref0_to_dbnum`（`cata_closure.rs:78`）；表由 `MdbMemoryStore` 逐元素扫描建成；
  `ElementAttributeService::open`（`element_attributes.rs:155`）与 `DirectTreeService::open`（`direct_tree.rs:55`）
  都先 `mdb_memory_store::ensure()`；`model_source::dbnum_of_root`（`model_source.rs:133`）逐个打开全部 DESI 试查，
  命中缓存 `DBNUM_BY_REF0` 以 `word0` 为键。

`word0 ≠ dbnum` 是对的（bits 13..25 是 extent，同库多 extent 多个 word0），但**按位解码**就够了，不需要表。

### 0.3 本切片完成后的形状

```text
refno ──RefNo::dbno()──▶ dbnum ──MdbRouteDirectory::pin(dbnum)──▶ DbFilePin ──DbSet/DirectStore 开库+核头──▶ B+tree 点查
                                      │
                                      └─ 不在当前 MDB / 无文件 / 类型不符 → 响亮报错，不猜
```

## 1. 目标 / 非目标

**目标**
1. 单元素属性、跨库引用换库、跨库 owner 上溯、生成根定库，全部经 `RefNo::dbno()` 路由，**不再调用 `resolve_ref0`**。
2. `ElementAttributeService` 不再依赖 `mdb_memory_store::ensure()`；只依赖 `MdbMembership` + `UdaCatalog`。
3. `model_source::dbnum_of_root` 不再遍历全部 DESI；删除 `DBNUM_BY_REF0`。
4. 保持现有语义：会话冻结（`DbPin.sesno: None` = 开库时解一次最新后冻住，ADR-054）、文件身份守卫（`FileReplaced`）、
   项目优先级（`select_dependency_file`）、`DbNumberMismatch` 核头。
5. 错误可诊断：`UnresolvedRef0` 替换为 `InvalidRefNo` / `DbNotInMdb` / `NoFileForDbnum`（后者已有）。

**非目标**
- 不改 `RefNo` 位格式、不改 SYS/CURD 成员语义、不写死任何现场 dbnum。
- 不动 `DirectTreeService` 的 WORL/根枚举（仍读骨架；那是母计划 3B/3C）；不动启动链预热顺序（阶段 4）。
  3C 需要的「WORL 在哪」已在全语料上实测为确定位置（§10.2），3B/3C 不再需要「首次展开单库扫描」兜底，
  但仍留给下一切片。
- 不删 `MdbMemoryStore` 模块（阶段 6）；本切片只让**属性与根定库路径**脱离它。
- 反向引用（谁引用了我）的跨库并集（`reverse_reference.rs` `foreign_referrers`、`cable_endpoint.rs` 196 条跨库边）不在本切片。
- 单根生成的 `e3d_model_service::scan_index`（目标库一次 Header 全扫建 `SourceIndex.owners`）不在本切片：
  已定为母计划**阶段 7**，独立阶段（§9 Q16，2026-09-12）。

## 2. 不变量（照母计划 §4，本切片逐条落到代码）

| # | 不变量 | 落点 |
|---|---|---|
| 1 | dbnum 只由 `RefNo::dbno()` 给出 | 新 helper `refno_dbnum()`；源码护栏禁止其它解法 |
| 2 | dbnum 只回答编号，文件由 MDB/依赖上下文解 | `MdbRouteDirectory` 只从 `MdbMembership` + `included_projects` 造目录 |
| 3 | 开文件必核头 | 沿用 `DbSet::db_slot` 的 `DbNumberMismatch`；`DirectStore::open_session` 补同一检查 |
| 4 | 时点冻结 | `DbPin.sesno` 语义不变 |
| 5 | 范围封闭 | 解出的 dbnum 不在目录 → `DbNotInMdb`，不回退扫描 |
| 6 | 按需成本 | 属性读取只开触达库；测试计数器断言 `scan_elements == 0` |
| 8 | 项目歧义显式 | 同号多项目继续走 `select_dependency_file`，不复制优先级 |

## 3. 设计

### 3.1 `src/data_interface/mdb_route_directory.rs`（新）

```rust
pub struct MdbRouteDirectory {
    membership: Arc<MdbMembership>,
    routes: HashMap<u32, DbRoute>,          // dbnum → {db_type, project, path, shadowed}
    project_files: ProjectDbFiles,          // 复用 current_mdb_source_inventory 的构建
    template_dir: PathBuf,
}
pub struct DbRoute { pub dbnum: u32, pub db_type: String, pub project: String, pub path: PathBuf, pub shadowed: Vec<PathBuf> }
pub enum RouteError { InvalidRefNo { refno: String }, DbNotInMdb { dbnum: u32 }, NoFileForDbnum { dbnum: u32 }, TemplateMissing { db_type: String } }

impl MdbRouteDirectory {
    pub fn from_membership(membership: Arc<MdbMembership>, schema: &DirectSchema) -> anyhow::Result<Self>;
    pub fn route_refno(&self, refno: RefNo) -> Result<&DbRoute, RouteError>;   // dbno() + 查表
    pub fn route_dbno(&self, dbno: u32) -> Result<&DbRoute, RouteError>;
    pub fn pin(&self, dbno: u32) -> Result<DbFilePin, RouteError>;             // sesno: None
    pub fn element_databases(&self) -> impl Iterator<Item = &DbRoute>;          // DESI/ISOD
}
impl e3d_io::DbFileResolver for MdbRouteDirectory { fn resolve(&self, dbno) -> Option<DbFilePin> }
impl CataDbLocator for MdbRouteDirectory {           // 迁移期兼容壳
    fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> { None }   // 明确不再回答 ref0
    fn db_type_of(&self, dbnum) -> Option<String>;
    fn file_of(&self, dbnum) -> Option<(String, PathBuf)>;
}
```

- 构建只读 SYS/CURD 成员清单与每库文件头（`DbFileHeader::read`，核 dbnum/type），**不读任何元素**。
- 目录构建逻辑从 `e3d_model_service::current_mdb_source_inventory()` 抽出为共享函数，两处共用一份，
  消除第二份目录（母计划阶段 1 退出标准）。
- `included_projects` 的目录库（CATA/PROP/DICT）并入目录，替代 `ElementAttributeService::open` 里那段
  `InMemoryCataLocator::build_from_dir` 合并；DESI/DICT 仍按现规则忽略。

**已落地形态（`bb16105d2`，与上面草稿的差别）**

- 构造：`from_membership(&MdbMembership, template_dir)` / `from_databases(project, mdb, &[MdbDatabase], template_dir)`；
  `add_dependency_dir(project, root_dir) -> DependencyScan` 并入依赖项目的 `DEPENDENCY_ROUTE_TYPES = [CATA, PROP]`
  （比旧 `merge_cata_files` 多收 PROP）；`register_project_file` 登记同号跨项目候选。
- 查询：`route_refno / route_refu64 / route_dbno -> &DbRoute`；`pin(dbnum)` = `pin_for(dbnum, None)`；
  **`pin_for(dbnum, preferred_project)`** 就是 §9 Q11 定的「目标库上下文」入口——同类型副本核头通过才用，
  否则回落 MDB 声明文件并 warn。`E3dDbResolver` 与 `current_mdb_source_inventory()` 尚未改为从目录派生（W5/W6 之后收敛）。
- `DbRoute { dbnum, db_type, project, path, shadowed, origin: Mdb | Dependency }`；`shadowed` 只记不选（§9 Q12）。
- `RouteError` 六类：`InvalidRefNo / DbNotInMdb / NoFileForDbnum / TemplateMissing / HeaderUnreadable / HeaderMismatch`，
  「声明了没文件」与「根本没声明」分开报。
- `verify_all_headers()` 给启动复核 / 真库门；`stats() -> RouteDirectoryStats { routes, mdb_routes, dependency_routes,
  element_routes, declared_without_file, project_candidates, problems }` 给日志与将来 `/health.core`。
- `impl DbFileResolver`（miss → `None`，debug 级日志）；`impl CataDbLocator`（`dbnum_of_ref0` 恒 `None`）。
- `curd_order` 保 CURD 原序：`element_databases()` 按它出，树首页与增量排工都要这个序。

**阶段 5 前置字段（本切片未做，接口先定，§9 Q13）**

每个 dbnum 一份 `DbGeneration { generation: u64, identity: FileIdentity(len, mtime), latest_sesno: u32, base_sesno: Option<u32> }`：
`identity` 或 `latest_sesno` 任一变化则 `generation += 1`，并标出变化类型——`sesno` 变大 = 会话前进、变小 = 回滚、
身份变但 `sesno` 不变 = 换文件；**2026-09-12 追加**：`sesno` 变大但新会话页的 `BASESN`（e3d-io `SessionPage::tail.base_sesno`，`c7cc6c8`）
低于上次观察到的 `sesno` 也是**回滚**——E3D BACKTRACK 后 SAVEWORK 会话号照涨、内容回到旧会话，且其 `REASON` 为 0，
只有 BASESN 能看出来（3.1 `ams8000_0001` 第 243 会话基于 239、其后保留 242）。规则表与跨多会话时的沿链回看见
`docs/plans/2026-09-12-lazy-startup-dev-plan.md` §3.4。这是母计划不变量 7「换文件、回滚、session 前进必须能区分」的落点，
`refresh_path(path) -> RouteRefresh { dbnum, from, to, kind }` 在阶段 5 加；`/health.core` 的 `route_directory.generation`
（§7 风险 2）以它为源。

### 3.2 共享解码 helper

`src/data_interface/refno_route.rs`（或放进 `mdb_route_directory.rs`）：

```rust
pub fn refno_dbnum(refno: RefU64) -> Option<u32> {
    let r = RefNo::new(refno.get_0(), refno.get_1());
    r.is_valid().then(|| r.dbno())
}
```

`DirectStore::dbnum_of` / `locator_dbnum` / `dbnum_hint`、`model_source::dbnum_of_root`、`direct_attmap_probe`
全部只经它取 dbnum。

### 3.3 `DirectStore` 改动（`direct_store.rs`）

- `dbnum_of(refno)`：`refno_dbnum()` → `None` 报 `InvalidRefNo`；`Some(d)` 但 `locator.file_of(d)` 为空 → `DbNotInMdb`。
- `locator_dbnum(refno)`：解码成功且目录有该库 → `Some`，否则 `None`（保持「问路不打断扫描」语义）。
- `dbnum_hint(refno)`：同上。
- `pin_from_locator(dbnum)`：不变（已是 dbnum 键的 `file_of`/`db_type_of`）。
- `open_session`：开库后核 `engine.descriptor().db_mark == dbnum`，不符报新变体 `DbNumberMismatch`。
- 删除 `DirectStoreError::UnresolvedRef0`；新增 `InvalidRefNo { refno }`、`DbNotInMdb { dbnum, refno }`、`DbNumberMismatch { dbnum, actual, file }`。
- `CataDbLocator` trait 的 `dbnum_of_ref0/resolve_ref0` 标 `#[deprecated]`，`DirectStore` 内零调用。

### 3.4 `model_source::dbnum_of_root`

```rust
pub fn dbnum_of_root(root: RefNo) -> anyhow::Result<u32> {
    anyhow::ensure!(root.is_valid(), "生成根 {root} 不是合法 RefNo");            // InvalidRefNo
    let dbnum = root.dbno();
    let (pins, _) = current_mdb_sources()?;
    let pin = pins.iter().find(|p| p.dbnum == dbnum as i32 && matches!(p.db_type.as_str(), "DESI" | "ISOD"))
        .with_context(|| format!("生成根 {root} 解出 dbnum {dbnum}，不在当前 MDB 的 DESI/ISOD 库里"))?; // DbNotInMdb
    // 已定（§9 Q5）：只在目标库点查一次，区分「编号合法但元素不存在」——1 次开库 + 1 次 B+tree 点查，
    // 后面生成很快也要开它，几乎免费；换来「历史 gen_root 行指向已删元素」能一句说清。
    anyhow::ensure!(element_exists_in(pin, root)?, "生成根 {root} 在 DESI/ISOD {dbnum} 的当前会话里不存在"); // ElementMissing
    Ok(dbnum)
}
```

删除 `DBNUM_BY_REF0`。三类错误分开：`InvalidRefNo` / `DbNotInMdb` / `ElementMissing`。类型集合放宽到 **DESI + ISOD**
（与 baseline 的 417 口径一致），`mdb_design_dbnums()` 同步放宽——它是启动复核「哪些库没有凭证」的分母，改了要看
`/health.startup.reconcile` 的回归。`root_source_version()` 的调用方（启动复核、增量排工）里若有按错误文本匹配的，W4 一并改。

### 3.5 `ElementAttributeService::open`

- 参数不变；内部改为 `MdbRouteDirectory::from_membership(membership, &schema)` → `DirectStore::new(schema, Arc::new(directory))`。
- 删除 `mdb_memory_store::ensure()`、`locator_parts()`、`ref0_count() > 0` 断言、`build_from_dir` 合并。
- `catalog`：从 `UdaCatalog::from_dictionaries(membership.dictionary_paths(), &schema)` 取（母计划 5.2 `UdaCatalogProvider` 的最小形态；
  若现有 `skeleton.uda_catalog()` 的构造逻辑不能直接复用，本切片先抽一个只读 DICT 的构造函数，不做缓存/失效）。
- `is_current()`：不再比骨架指纹，只比 `membership_fingerprint`（DICT 文件身份纳入指纹）。
- `snapshot_epoch`：来源改为 `membership` 的文件指纹哈希；`/health.core.skeleton_elements` 在**本切片**仍有值
  （树服务还持骨架），但最终口径已定为**阶段 6 直接删、不留 deprecated 版**（§9 Q3：plant-ui 与 plant3d-web 都不读它，
  唯一"消费者"是 `tests/refno_native_api_contract.rs` 的冻结清单——阶段 6 改路由时同步改那条测试，别把它当回归）。

### 3.6 `DirectTreeService::open`

- 只改 locator 来源：`MdbRouteDirectory` 代替 `skeleton.locator_parts()`；`skeleton` 字段与 `roots()` 的 `refnos_of_noun` 保留（3B 的活）。
- 这样树的 `ancestors()`/`node()` 属性读取已经走新路由，`ensure()` 仍在（因为 WORL 枚举要它）——本切片不承诺树首页零扫描。

### 3.7 `direct_attmap_probe`

- `--owner-climb` 与 `cross_db_references()` 改用 `store.dbnum_of()`（内部已是解码）；输出多两栏：`decoded_dbnum` 与 `descriptor_dbnum`，
  逐条断言相等——这是真库上「解码 = 文件自报」的活体对账。
- **已落地（`7f501ab0d`）**：定位器从 `InMemoryCataLocator::build_for_project`（全库扫描建 ref0 表，退役）换成
  `MdbRouteDirectory::from_membership + add_dependency_dir`，与属性 / 树服务同一种造法；`DbnumLedger` 对每个样本、
  owner 链每一跳、每条跨库引用目标在读出属性后记一笔（`refno_dbnum` == 打开的 dbnum == 文件头 `db_mark`，每库只读一次头），
  不符计入 fatal，汇总打印触达的库。

## 4. 工作项与顺序

| # | 状态 | 工作项 | 文件 | 验收 |
|---|---|---|---|---|
| W0 | **(a)(b)(c) 已做**（2026-09-12，未提交）；(d)(e) 未做 | **阶段 0 收口**（§9 Q1/Q4/Q8）：(a) 修 `vendor/e3d-io/tests/refno_dbno_corpus.rs::looks_like_primary_database` 的 `split_at(3)` 字节切崩溃（中文文件名 → `end byte index 3 is not a char boundary`），加 `!name.is_ascii()` 直接返回 `false`；(b) 语料根改读环境变量 `E3D_CORPUS_ROOTS`（`;` 分隔），未设置则 skip 而非 `#[ignore]`；(c) 把 §10.1 的 1,947 库 / 38,891,013 元素 / 0 分歧写进 `docs/baselines/2026-09-12-jeu-refno-routing-stage0.md`，JEU 三项 `null` 标「现场不可达」；(d) 采一份 **AMS 3.1 基线** `docs/baselines/2026-09-12-ams-refno-routing-stage0.json`（同 schema：现版启动 `mdb_memory_store::ensure` 的 `read_stats` 差值 `database_opens / scan_calls / scanned_elements`、骨架耗时、RSS 前后、首属性、首根；首属性/首根取 DESI 8000 里 `scripts/e3d/db8000_bran_probe.mac` 用的那个 BRAN）；(e) `tests/refno_native_api_contract.rs` 里 `skeleton_elements` 一行加注「阶段 6 删」 | `vendor/e3d-io/tests/refno_dbno_corpus.rs`、`docs/baselines/*`、`tests/refno_native_api_contract.rs` | 全语料门 `cargo test --test refno_dbno_corpus -- --include-ignored` 在本机跑通且 0 分歧；AMS JSON 无 `null` |
| W1 | **已提交 `bb16105d2`** | 抽 `refno_dbnum()` helper + 单测 | `refno_route.rs`（新） | `24572/115 → 8188`；primary/secondary extent 同 dbnum；`251000`；非法/空 → `None`；全 18 位 dbnum 往返 |
| W2 | **已提交 `bb16105d2`** | `MdbRouteDirectory` + `DbFileResolver`/`CataDbLocator` 实现 + 单测 | `mdb_route_directory.rs`（新）；`e3d_model_service.rs` 的共享构建**尚未抽**（留到 W5/W6 后收敛） | 用 `MdbMembership` 夹具：dbnum 在/不在 MDB、文件头 dbnum 不符、同号跨项目按 `preferred_project` 选、DICT/CATA 依赖并入；真库门 `live_directory_matches_every_file_header` |
| W3 | **已提交 `5cc5e3e04`** | `DirectStore` 换路由与错误变体 | `direct_store.rs`、`direct_store_refno_live_tests.rs`（新）、`web_service/mod.rs`、`on_demand_db.rs`、`element_attributes.rs`（missing 判定） | `direct_store::tests` 全绿；新增「解码 dbnum 不在目录报 `DbNotInMdb`」「非法 refno 报 `InvalidRefNo`」「N+1 钉到 N 的文件报 `DbNumberMismatch`」；真库门：AMS `/ALL` DESI 1112 扫 2,132 元素、25 条跨库引用按解码换库读通（CATA 7001，仅开 2 个会话）；本文件源码护栏 `production_code_never_consults_the_ref0_table` |
| W4 | **已提交 `425ca98d8` + `db49238e3`** | `425ca98d8`：`dbnum_of_root` 改解码 + 只开目标库；删 `DBNUM_BY_REF0`。`db49238e3`（并行会话补齐）：拆 `dbnum_of_root_in(root, \|\| members)`，点查收进 `element_exists(pin, root)`（`ReadOnlyEngine::open` 一次 + 核 `descriptor().db_mark == dbnum` + `search_index` 一次）；错误成类型 `RootDbError { InvalidRefNo / DbNotInMdb / NotAnElementDb / Open / DbNumberMismatch / ElementMissing }`，签名仍回 `anyhow::Result<u32>`；`mdb_design_dbnums()` 分母放宽到 DESI+ISOD，与定库共用 `is_element_db()` | `model_source.rs` | `cargo test --lib model_source` 11 passed；三处调用方零改动。**未验真库**：ISOD 进分母后 `/health.startup.reconcile` 的回归（AMS `/ALL` 的 ISOD 成员是否有凭证行） |
| W5 | **已提交 `425ca98d8`** | `ElementAttributeService` 脱离骨架：`MdbRouteDirectory::from_membership` + `add_dependency_dir`（CATA / PROP 只读文件头），`is_current` / `snapshot_epoch` 只看成员文件指纹，UDA 字典 `load_uda_catalog` 直接读 DICT | `element_attributes.rs` | 真库：AMS `/ALL` 上 plant-ui 选中 SITE / ZONE 属性面板正常（通用 + 元素属性 + UDA）；`open()` 期间 `scan_elements == 0` 的计数器断言未加（W8 行为断言那一档，见 §5.2） |
| W6 | **已提交 `425ca98d8`** | `DirectTreeService` locator 换源（`ensure()` 仍在：WORL 枚举与 noun / owner 查询，3B / 3C） | `direct_tree.rs` | 真库：根层 38 个 SITE、展开 `/1RX03-EQUI` 三个 ZONE 正常；direct tree TTY 对拍未重跑 |
| W7 | **已提交 `7f501ab0d`** | 探针定位器换 `MdbRouteDirectory`（退役 `InMemoryCataLocator::build_for_project` 全库扫描），加 `DbnumLedger`「解码 = 文件自报」对账栏（样本 + owner 链每跳 + 跨库引用目标），不符计 fatal | `bin/direct_attmap_probe.rs` | ams8000 @444 `--sample 200 --owner-climb --nouns EQUI,BRAN,GENSEC`（DB 侧 `_runs/ams-refno` 18909）：路由目录 77 条 / 29 元素库 / 7 依赖 2.2 s；200 样本 0 真值冲突；跨库引用 **338 条换库读通、0 失败**（CATA 5052 @189）；owner 链 200 条到根未出本库；对账 **1313/1313 相等、0 不符**；exit 0，6 s。「计数与改前相同」因随机采样不可复现，改记「0 失败」；日志 `D:\work\plant-code\old\_runs\ams-refno\probe-w7-ams8000.log` |
| W8 | **已提交 `425ca98d8` + `db49238e3`** | 源码护栏测试：文本护栏（`425ca98d8`）+ `read_stats` 行为护栏（`db49238e3`，合成 4 页库文件） | `tests/refno_native_guards.rs`（新） | 8 passed（CRLF 检出下同样成立）：位公式只在 `refno_route`；`direct_store` 无 ref0 表 / `.get_0()` / `UnresolvedRef0`；`dbnum_of_root` 不遍历 pins、`element_exists(` 恰一次、整模块只一处开库且核 `db_mark`、分母与定库共用 `is_element_db`；属性 / 树服务定位器来自路由目录；路由目录无逐元素表；不写死 8188；行为：合法根 `database_opens=1 / scan_calls=0 / index_point_lookups=1`，元素不存在 → `ElementMissing`，头自报 1002 登记 1001 → `DbNumberMismatch`，不在 MDB / CATA / 非法 / 声明了没文件零新增开库 |
| W9 | 未做 | 文档 | `CONTEXT.md`（跨库属性词条）、母计划状态、`docs/evidence/2026-09-12-ida-core-dabacon-refno-routing.md` | 证据文件记录 §0.1 全部地址与反编译片段 |

顺序（§9 Q1 定：**2A 前置**）：W0 → **W4** → W5/W6（依赖已提交的 W2+W3）→ W7/W8 → W9。W4 不依赖 W2：它只用
`root.dbno()` + 现有 `source_file_lookup()`，目录派生留到收敛 `current_mdb_source_inventory()` 时一起换。
W1–W3 已由并行会话提交，本表按提交号对账，不重做。

W0-a/b 的实际落法：全语料门去掉 `#[ignore]`，语料根读 `E3D_CORPUS_ROOTS`（`;` 分隔，`std::env::split_paths`），未设置则打印
skip 并返回；本机 `E3D_CORPUS_ROOTS="D:\AVEVA\Projects\E3D2.1;D:\AVEVA\Projects\E3D3.1" cargo test --release --test refno_dbno_corpus`
→ 1,947 库 / 38,891,013 元素 / 0 分歧，38 s。W0-c 写进 `docs/baselines/2026-09-12-jeu-refno-routing-stage0.md`
（新增「位解码 corpus 门」「WORL 位置」两节，判定段改为退出标准已过）。

**下一切片**：`docs/plans/2026-09-12-lazy-startup-dev-plan.md`（母计划阶段 4，兼并 3B/3C 与阶段 5 最小形态），
依赖本切片 W4–W6 合入。

## 5. 测试

### 5.1 单元 / 属性
- W1 解码表（含 stage-0 语料复用）。
- W2 目录：`from_membership` 只读头页（用 `read_stats` 计数器断言元素扫描为 0）。
- W3 双库夹具：库 A 的元素引用属性指向库 B 的元素；`store.attrs(ref_in_B)` 读通且只开 A、B 两库；把 B 从目录删掉后同一读取报 `DbNotInMdb{dbnum: B}`。
- 冻结语义回归：`pinned_sesno` 在两次读之间不变；文件替换后 `verify_file_identity` 仍报 `FileReplaced`。

### 5.2 源码护栏（`tests/refno_native_guards.rs`，`include_str!` 扫源）

形态按 §9 Q14：沿用仓内 `include_str!` 文本护栏（`lib.rs` 切 `run_cli(` 段落、`mdb_membership.rs` 断言生产区无 `8188`
的既有写法），**加** `e3d_io::read_stats` 计数器行为断言。文本护栏粗，但「有人把 `ensure()` 加回启动」这类回归在
`cargo test` 里就红，不用真启动。

- `direct_store.rs` 不含 `resolve_ref0(`、`dbnum_of_ref0(`、`.get_0()`（W3 已在文件内自带 `production_code_never_consults_the_ref0_table`，W8 只把它挪进公共护栏文件或保留双份）。
- `model_source.rs` 不含 `DBNUM_BY_REF0`，`dbnum_of_root` 函数体不含 `for pin in` / `find_element(` / `ReadOnlyEngine::open(`；允许恰好一次 `element_exists(`（Q5 已定保留点查）。
- `element_attributes.rs` 不含 `mdb_memory_store::ensure(`。
- 生产源码不含字面 `8188`（测试与基线文件除外）。
- `DirectStoreError::UnresolvedRef0` 不再存在（W3 已删）。
- 行为断言（合成多库夹具，复用 `refno_native_stage0.rs` 的写法）：属性服务 `open()` 期间 `scan_calls == 0`；定一个根 `database_opens ≤ 1 + 依赖数`；单属性读取 `scan_calls == 0`。
- 阶段 4 到来时再加：`run_cli(` / `run_read_through_core(` / `serve(` 段落不含 `mdb_memory_store::ensure(`、`scan_elements(`；`serve(` 里 `.get().await` 不得出现在 `Router::new()` 之前。本切片不加，避免先红。

### 5.3 现有回归（必须全绿）
`web_service::tests`、direct tree TTY 对拍、direct attribute/UDA parity、model generation root parity、CATA dependency closure、
MDB Global resolver、E3D 2.1 与 3.1 schema pack、`tests/refno_native_api_contract.rs`。

### 5.4 真库（手动，记入 `docs/2026-08-12_live-test-ledger.md`）

一律在 **AMS 3.1**（当前 `DbOption.toml`）上跑（§9 Q9）；2.1 的 AMS 端到端留到阶段 4 收尾补一次——路由改的是
RefNo → dbnum → 文件，与 schema pack 无关，2.1/3.1 的差异在属性解码，那块已有 parity 测试。JEU 不可达，
下面原 JEU 项全部改写为 AMS 口径；JEU 只在拿到现场机器时按母计划 §7.4 补采。

- ams8000：`direct_attmap_probe --owner-climb --nouns EQUI,BRAN,GENSEC`，对账栏 100% 相等；跨库引用换库成功数不低于改前。
- ams `/ALL` 属性：任取 DESI 8000 一个 BRAN 的 `/api/v1/element/attributes`，与 E3D TTY 相同；`read_stats` 差值里
  `database_opens = 1 + 实际依赖数`、`scan_calls = 0`；属性服务构造期不出现「DESI 骨架 N 个元素」
  （树服务仍会打印——在案，3B）。
- 单库 SAVEWORK 只刷对应 dbnum（阶段 5 的门，夹具现在就定，§9 Q10）：用现成的
  `scripts/e3d/db8000_bran_ftub_move_apply.mac` / `..._restore.mac`（apply + restore 业务净变化为空），
  验 8000 刷新、7998 的 pin/cache generation 不动（两者都在 `watch_dbnums`）；阶段 5 给 `/health` 加
  「最近定向刷新」字段后即可断言。
- 树首页（3B/3C 到来时）：每库 1 次开库 + 1 次 WORL 点查 + 1 次成员表，`scan_calls == 0`；WORL 位置见 §10.2。

## 6. 发布与回滚

- 与母计划 §8 一致：本切片对应「先合入阶段 0–2，保留旧骨架但停止让属性/模型路由依赖它」。
- 不加开关：两套路由并存正是要消除的东西；回滚 = 回退本切片提交。
- 新 build_id 出包；`/health.core.skeleton_elements` 仍有值（来自树服务的骨架），字段标 deprecated。

## 7. 风险

- **`InMemoryCataLocator` 的其它消费者**（`cata_closure` 闭包引擎 `:1138`、`generation_root`、`on_demand_db`）仍读 `resolve_ref0`：
  本切片只改属性/根定库四处；闭包引擎那一处在 W3 里评估——若它也能改成解码就一并改，否则记进母计划阶段 6 清单，不静默。
- **`snapshot_epoch` 语义变化**：前端把它当「数据是否变了」的信号；改成 membership 指纹哈希后，DESI 保存不再改变它——
  这正是阶段 5 要解决的按 dbnum 失效；本切片在 `/health` 里同时暴露 `route_directory.generation`，前端先不依赖。
- **DICT 变更**：属性服务的 UDA catalog 不再随骨架刷新；本切片让 `is_current()` 把 DICT 文件身份算进指纹，变了就重建服务（粗粒度但不静默）。

## 8. 评审拍板（已按推荐项执行）

1. `MdbRouteDirectory` 同时实现 `CataDbLocator` 兼容壳，`dbnum_of_ref0` 恒 `None`，`DirectStore::new` 签名本切片不动 —— **已落地**（`bb16105d2`）。
2. `dbnum_of_root` 保留一次目标库 `element_exists` 点查 —— **已定**（§9 Q5，另加类型集 DESI+ISOD）。
3. UDA catalog 本切片「只读 DICT 构造、不缓存」，`UdaCatalogProvider` 留给阶段 5 —— **按推荐**。
4. `snapshot_epoch` 改源为 membership 指纹哈希，同时暴露 `route_directory.generation` —— **按推荐**；generation 的定义见 §3.1 末段。

## 9. 口径记录（2026-09-12，与用户逐题对齐；十五题全按推荐项）

编号沿用对齐时的问法，后续文档与提交信息引用 `§9 Qn` 即可。

| # | 问题 | 定论 |
|---|---|---|
| Q1 | 下一切片到哪 | 阶段 0 收口（W0）+ **阶段 2A 前置**（W4），阶段 1 的目录收敛（`current_mdb_source_inventory` / `E3dDbResolver` 派生）再下一片 |
| Q2 | 在哪棵树做 | 就在 `refactor/retire-publish-stage` 当前工作树；先把另外三条无关流水各自提交（见 §11），不开新 worktree——e3d-io 是共享 path 依赖，worktree 隔离对它是假的 |
| Q3 | `skeleton_elements` | 阶段 6 **直接删**，不留 deprecated 版；同步改 `refno_native_api_contract.rs` |
| Q4 | 全语料门 | 修 `split_at` 崩溃 + `E3D_CORPUS_ROOTS` 环境变量 + 把 §10.1 结果写进 baseline |
| Q5 | 2A 语义 | 目录 + 目标库一次 `element_exists`；类型集 DESI+ISOD；三类错误 `InvalidRefNo / DbNotInMdb / ElementMissing` |
| Q6 | 谁做 | 单会话做到阶段 2 结束；阶段 3 起（树 / 属性两服务分属不同文件）再考虑分组 |
| Q7 | JEU 三项 null | 不阻塞；JEU 本机不可达，只能现场补采 |
| Q8 | AMS 基线 | 采一份 AMS 3.1 阶段 0 基线（W0-d），否则「改前多慢」在 AMS 上没有数 |
| Q9 | 验收版本 | 只跑 3.1；2.1 在阶段 4 收尾补一次 |
| Q10 | SAVEWORK 夹具 | `db8000_bran_ftub_move_apply/restore.mac`，验 8000 刷、7998 不动 |
| Q11 | 目录边界 | 目录带「目标库上下文」：`pin_for(dbnum, preferred_project)`（已落地），后续给 `resolver_for(target_dbnum)` 视图，`E3dDbResolver` 变壳或删；`SysMetadataResolver` 独立不并（它在 membership 之前） |
| Q12 | MDB 内同号 | MDB 声明路径胜出，`shadowed` 只记不选、只作依赖候选；真歧义（同目录两文件自报同 dbnum）已由 `initialization_phase` blocker 与 `SysMetadataResolver` 的拒绝覆盖 |
| Q13 | generation | 每 dbnum 单调 `u64`，存 `(FileIdentity, latest_sesno, base_sesno)`，任一变化 +1 并标类型（前进 / 回滚 / 换文件）；回滚含「会话号变大但 BASESN 低于上次会话号」（2026-09-12 追加，§3.1 / 阶段 4 切片 §3.4）；阶段 1 定接口，阶段 5 实现 |
| Q14 | 护栏形态 | `include_str!` 文本护栏 + `read_stats` 计数器行为断言，两者都要 |
| Q15 | 提交拆法 | 按 hunk 拆：e3d-io 两笔、gen-model 四笔（三条无关流水 + 阶段 0）；提交前先贴 file→commit map；提交信息沿仓内惯例但**省略 t-编号**，只引 ADR / plan 路径 |
| Q16 | 单根生成的 `scan_index` 单库全扫 | **已定（2026-09-12）**：记为母计划**阶段 7**，独立阶段，不入本片也不入阶段 4 切片 |

## 10. 实测记录（2026-09-12，本机，仓外临时工程，已删）

### 10.1 阶段 0 退出门：位解码 corpus 零分歧

仓内 `refno_dbno_corpus.rs` 的全语料门因 `split_at(3)` 在中文文件名上 panic，从未在本机跑通（W0-a 修）。用同一逻辑
（修掉崩溃）在 `D:\AVEVA\Projects\E3D2.1` 与 `E3D3.1` 全部主库文件上重跑：

| 语料 | 主库文件 | 元素（Header 扫） | `refno.dbno() != descriptor.db_mark` | 最大 dbno |
|---|---|---|---|---|
| E3D 2.1 | 883 | 16,154,107 | 0 | 251,453 |
| E3D 3.1 | 1,064 | 22,736,906 | 0 | 251,712 |
| 合计 | 1,947 | 38,891,013 | **0** | — |

母计划阶段 0 的退出标准成立；三个真实样本文件的非 ignore 门（375,392 元素）与 `refno_native_stage0.rs`、
`refno_native_api_contract.rs`（4 条）本机全绿。

### 10.2 3C spike：WORL 位置确定，不需要单库扫描兜底

同一 1,947 个库上统计每库 WORL（`noun_hash == db1_hash("WORL")`）与无主根：

- 每库**恰好一个** WORL，owner 全为 0；
- 全部 1,905 个非 SYS/COMM/MISC 库（DESI/CATA/DICT/PADD/ISOD…）WORL == `RefNo(descriptor.primary_extent_ref, 0)`，
  即 `(dbno | 0x2000) / 0`，**0 例外**，`find_element` 一次点查命中；
- 例外只有项目管理库：COMM(8190) `16382/1`、MISC(8189) `16381/1`、SYST(8191) `24575/3`（extent 2）——都不是 CURD 元素库；
- 每库另有恰好 1 个无主的 `MNUM` 系统元素（noun_hash 803020），不是 WORL；`DirectTreeService::roots()` 只取 WORL 成员所以不受影响，
  `scan_index().roots` 里它会出现但 `enumerate_generation_roots` 对它产 0 根，现状已如此。

与 IDA `DB_DB::getWorld` 口径一致。结论：3B/3C 的树首页 = 每库 1 次开库 + 1 次 WORL 点查 + 1 次成员表读，
`by_noun` 表的唯一消费者（`roots()` 的 `refnos_of_noun(dbnum, "WORL")`）可以零扫描替代；母计划 3C 里
「若格式库暂时无法复现原生索引，只允许首次展开该 dbnum 时扫描该单库」这条兜底不再需要。已记入共享记忆 mem-532。

### 10.3 骨架消费者对账（回答「改完能不能不全库扫描」）

**能。** `MdbMemoryStore` 每个消费者的替代与扫描成本：

| 骨架字段 | 消费者 | 替代 | 元素扫描 |
|---|---|---|---|
| `affiliations: ref0→dbnum` | `dbnum_of/dbnum_of_ref0`（direct_tree.rs）；`locator_parts()`→`InMemoryCataLocator`（direct_tree.rs、element_attributes.rs）；`ref0s_of`→`/dbnums.ref0s`（handlers.rs） | `RefNo::dbno()`；`ref0s` = e3d-io `FileInfo::extent_refs(descriptor)`（page 0 + page 1） | 0 |
| `elements: refno→(noun, owner)` | `contains`（direct_tree.rs；lib.rs 2 s 循环判删根）；`ancestors/owner_of`；`noun_hash_of` | `DbSet::element_exists` 点查；`DbElement::owner()` 逐级跨库；noun 从记录头 | 0（链长 × 点查） |
| `by_noun` | `roots()` 找 WORL | §10.2 确定位置 | 0 |
| `uda_catalog` | element_attributes.rs、read_through.rs | 只读 DICT 构造（阶段 5 变 `UdaCatalogProvider`） | 0 |
| `session_fingerprint/is_current` | `/health` `snapshot_epoch/session_vector`；两个 provider 的新鲜度 | 目录 per-dbnum generation（只读文件头） | 0 |
| `describe/stats` | 启动横幅、`skeleton_elements` | `RouteDirectoryStats` | 0 |
| 整份 snapshot | `bootstrap_read_through_rooms`（仅 `room_membership=true`） | 按库流式 `scan_index`，母计划允许的显式全量 | 默认 false 不触发 |

替代后仍存在的扫描只有两处，都是**单库**：(1) `search_names` → `DirectStore::indexes(dbnum)` 的 `scan_elements(Full)`，
母计划已定为显式后台索引任务；(2) 单根 `/model/ensure` → `generation_input()` → `scan_index(&pin.file, sesno)`
（e3d_model_service.rs）对目标库一次 Header 全扫建 `SourceIndex{roots, owners}`，整库 owner 表随 `GeneratedSnapshot`
进 publication，按 `(dbnum, sesno, identity)` 缓存；kv-mem 的 `model_dbnum_ensure` 也走它。它不违反母计划的门
（「单根不打开全部 DESI」），但违反不变量 6 的字面（成本 ∝ 触达元素）。改法：publication 的 owner 从生成时已解析的
元素记录头取，`roots` 只在整库 ensure 时才要——见 §9 Q16。

启动链（`room_membership=false`）删掉 `run_cli` / `run_read_through_core` 的 `ensure()` 与 `serve()` bind 前的
`element_attributes.get()` 后，只剩 SYS/CURD 解析（按 NAME 精确查 MDB；`mdb_membership.rs` 那次 SYS `scan_elements(Named)`
**只在 `mdb_name` 拼错的错误路径**上跑）+ DICT UDA 目录 + 文件头读取：DESI 元素扫描 = 0，`read_stats.scan_calls` 可断言。

## 11. 工作树与提交拆分（§9 Q2 / Q15）

截至 2026-09-12 11:15，工作树 66 项未提交，混着四条流水；gen-model 的未提交代码依赖 e3d-io **未提交**的
`database_header_diagnostics`（HEAD 里没有），所以 e3d-io 先提交、gen-model 后提交。按 hunk 拆，提交前贴出 map：

> **e3d-io 侧已提交（2026-09-12 13:30，`main`）**：A → `b90c585` `feat(diagnostics): header/session diagnostics, DAB engine tags, GLOB template name`（14 文件）；
> D → `e8d0ece` `feat(read-stats): count opens/scans/point lookups; locate WORLD by descriptor, gate both on the corpus`（11 文件，含 CHANGELOG「Added」三条）。
> `engine.rs` / `meta/descriptor.rs` 按 hunk 拆（A：`DatabaseHeaderDiagnostics` 段 + 描述符 +0x24/+0x40/+0x154 字段 + 尾部 rustfmt；D：四个 `note_*` + `world()` + `world_refno()`）。
> A 单独检出全套 `cargo test --release` 68 套 399 通过 0 失败；两笔叠加 = 提交前工作树内容，72 套 406 通过 0 失败。gen-model 侧四笔未动。

| 流水 | e3d-io | gen-model | 备注 |
|---|---|---|---|
| A. E3D schema 版本选择 / 版本证据 / SysMetadataResolver / dabacon 汇总 | `schema.rs`、`meta/*`、`session/mod.rs`（DAB tag）、`engine.rs` 中 `DatabaseHeaderDiagnostics` 段、`db_element.rs`（GLOB→`glbvir.dat`）、`lib.rs` 导出、`dabacon_tags.rs`、`bin/e3d-descriptor.rs`、`tests/header_diagnostics_real.rs`、`tests/project_dabacon_tags.rs`、`examples/probe_world.rs`、`docs/evidence/2026-09-11-*`、`CHANGELOG.md` | `direct_store.rs`（`DEFAULT_E3D_VERSION` 段）、`mdb_membership.rs`（`SysMetadataResolver` + 版本证据）、`options.rs`、`increment_manager.rs`（部分）、`dabacon_tag_summary.rs`、`handlers.rs`（`dabacon_tags`）、`setup.rs` / `web/setup.html` / `web/ops.html`、`DbOption*.toml`、`templates/DbOption.factory.toml` | `engine.rs`、`handlers.rs`、`increment_manager.rs` 需 `add -p` |
| B. kv-mem 房间启动与发布（plan 2026-09-11） | — | `model_dbnum_ensure.rs`、`task_registry.rs`、`model_memory_store.rs`、`model_db_adapter.rs`、`e3d_model_service.rs`、`model_refresh.rs`、`model_read_route.rs`、`room_model.rs`、`e3d_mesh_store.rs`、`initialization_phase.rs`、`lib.rs` / `main.rs`（部分）、`Run-KvMemRoomBootstrap.ps1`、`docs/plans/2026-09-11-kv-mem-*.md`、ADR-073 | `lib.rs`、`main.rs` 需 `add -p` |
| C. aios-patch / build_info / 0.1.22 发布 | — | `tools/aios-patch/*`、`build.rs`、`build_info.rs`、`New-LocalRelease.ps1`、`New-/Install-AiosDatabasePatch.ps1`、`start-local-release.ps1`、`docs/release-notes/0.1.22.md`、`0.1.23.md`、ADR-068/069、`Cargo.toml` / `Cargo.lock`、`docs/specs/web-service-api.md`（部分） | `Cargo.toml` 若含 B 的依赖变更要核 |
| D. 本计划阶段 0 + 3C 封装 + 阶段 5 账本 | `read_stats.rs`、`refno.rs`、`engine.rs` 中四个 `note_*` 打点与 `world()`、`meta/descriptor.rs` 的 `world_refno()`、`error.rs` 的 `NotAWorld`、`record/mod.rs` 的 `ElementRecordView::header()`、`tests/refno_dbno_corpus.rs`、`tests/refno_native_stage0.rs`、`tests/world_refno_synthetic.rs`、`tests/world_refno_corpus.rs` | `tests/refno_native_api_contract.rs`、`docs/baselines/2026-09-12-*`、`docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`、`docs/plans/2026-09-12-lazy-startup-dev-plan.md`、本文件、`src/data_interface/db_generation.rs`（新）+ `data_interface/mod.rs` 的 `pub mod db_generation` 一行 | W0-a/b/c、A-4（WORL 封装）、A-5（`GenerationLedger`，未接线）都进这一笔；`engine.rs` / `meta/descriptor.rs` / `mod.rs` 与别的流水共文件，按 hunk 归 |

未归类前不动 `git add`；`handlers.rs` / `lib.rs` / `main.rs` / `increment_manager.rs` / `engine.rs` 五个跨流水文件按 hunk 归属。
提交信息：英文 conventional 前缀 + 中文正文 + `(ADR-0xx / docs/plans/...)` 尾注，**不写 t-编号**（无来源）。
