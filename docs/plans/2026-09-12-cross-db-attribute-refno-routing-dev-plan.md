# 开发计划：跨库属性改走 RefNo 原生路由（母计划阶段 1–2 + 3A 的执行切片）

- 日期：2026-09-12
- 状态：Plannotator 已批准（2026-09-12，无附注）；§8 四项拍板按推荐项执行，未实施
- 母计划：`docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`（已批准；阶段 0 已落地：
  `vendor/e3d-io/tests/refno_dbno_corpus.rs`、`refno_native_stage0.rs`、`tests/refno_native_api_contract.rs`、
  `docs/baselines/2026-09-12-jeu-refno-routing-stage0.{json,md}`）
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
- 不删 `MdbMemoryStore` 模块（阶段 6）；本切片只让**属性与根定库路径**脱离它。
- 反向引用（谁引用了我）的跨库并集（`reverse_reference.rs` `foreign_referrers`、`cable_endpoint.rs` 196 条跨库边）不在本切片。

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
    anyhow::ensure!(root.is_valid(), "生成根 {root} 不是合法 RefNo");
    let dbnum = root.dbno();
    let (pins, _) = current_mdb_sources()?;
    let pin = pins.iter().find(|p| p.dbnum == dbnum as i32 && matches!(p.db_type.as_str(), "DESI" | "ISOD"))
        .with_context(|| format!("生成根 {root} 解出 dbnum {dbnum}，不在当前 MDB 的 DESI/ISOD 库里"))?;
    // 可选：只在目标库点查一次，区分「编号合法但元素不存在」
    Ok(dbnum)
}
```

删除 `DBNUM_BY_REF0`。是否保留 `element_exists` 点查由评审拍板（见 §8 Q2）。

### 3.5 `ElementAttributeService::open`

- 参数不变；内部改为 `MdbRouteDirectory::from_membership(membership, &schema)` → `DirectStore::new(schema, Arc::new(directory))`。
- 删除 `mdb_memory_store::ensure()`、`locator_parts()`、`ref0_count() > 0` 断言、`build_from_dir` 合并。
- `catalog`：从 `UdaCatalog::from_dictionaries(membership.dictionary_paths(), &schema)` 取（母计划 5.2 `UdaCatalogProvider` 的最小形态；
  若现有 `skeleton.uda_catalog()` 的构造逻辑不能直接复用，本切片先抽一个只读 DICT 的构造函数，不做缓存/失效）。
- `is_current()`：不再比骨架指纹，只比 `membership_fingerprint`（DICT 文件身份纳入指纹）。
- `snapshot_epoch`：来源改为 `membership` 的文件指纹哈希；`/health.core.skeleton_elements` 字段**保留**并标 deprecated（母计划 §10 Q2）。

### 3.6 `DirectTreeService::open`

- 只改 locator 来源：`MdbRouteDirectory` 代替 `skeleton.locator_parts()`；`skeleton` 字段与 `roots()` 的 `refnos_of_noun` 保留（3B 的活）。
- 这样树的 `ancestors()`/`node()` 属性读取已经走新路由，`ensure()` 仍在（因为 WORL 枚举要它）——本切片不承诺树首页零扫描。

### 3.7 `direct_attmap_probe`

- `--owner-climb` 与 `cross_db_references()` 改用 `store.dbnum_of()`（内部已是解码）；输出多两栏：`decoded_dbnum` 与 `descriptor_dbnum`，
  逐条断言相等——这是真库上「解码 = 文件自报」的活体对账。

## 4. 工作项与顺序

| # | 工作项 | 文件 | 验收 |
|---|---|---|---|
| W1 | 抽 `refno_dbnum()` helper + 单测 | `refno_route.rs`（新） | `24572/115 → 8188`；primary/secondary extent 同 dbnum；`251000`；非法/空 → `None` |
| W2 | `MdbRouteDirectory` + `DbFileResolver`/`CataDbLocator` 实现 + 单测 | `mdb_route_directory.rs`（新）、`e3d_model_service.rs`（抽共享构建） | 用 `MdbMembership` 夹具：dbnum 在/不在 MDB、文件头 dbnum 不符、同号跨项目按 `preferred_project` 选、DICT/CATA 依赖并入 |
| W3 | `DirectStore` 换路由与错误变体 | `direct_store.rs` | 现有 `direct_store::tests` 全绿；新增「双库夹具跨库引用读通」「解码 dbnum 不在目录报 `DbNotInMdb`」「文件自报不符报 `DbNumberMismatch`」 |
| W4 | `model_source::dbnum_of_root` 改解码；删 `DBNUM_BY_REF0` | `model_source.rs` | 单测：合法根→dbnum 不开任何库；不在 MDB 报错含 dbnum；源码护栏 |
| W5 | `ElementAttributeService` 脱离骨架 | `element_attributes.rs` | `read()` 对拍不变（direct attribute/UDA parity）；`open()` 期间 `scan_elements` 计数 = 0 |
| W6 | `DirectTreeService` locator 换源 | `direct_tree.rs` | direct tree TTY 对拍逐字相同 |
| W7 | 探针补对账栏 | `bin/direct_attmap_probe.rs` | ams8000 `--owner-climb`：`decoded == descriptor` 100%；`cross_db_refs` 计数与改前相同 |
| W8 | 源码护栏测试 | `tests/refno_native_guards.rs`（新） | 见 §5.2 |
| W9 | 文档 | `CONTEXT.md`（跨库属性词条）、母计划状态、`docs/evidence/2026-09-12-ida-core-dabacon-refno-routing.md` | 证据文件记录 §0.1 全部地址与反编译片段 |

W1→W2→W3 串行；W4 可与 W3 并行；W5/W6 依赖 W2+W3；W7/W8 最后。

## 5. 测试

### 5.1 单元 / 属性
- W1 解码表（含 stage-0 语料复用）。
- W2 目录：`from_membership` 只读头页（用 `read_stats` 计数器断言元素扫描为 0）。
- W3 双库夹具：库 A 的元素引用属性指向库 B 的元素；`store.attrs(ref_in_B)` 读通且只开 A、B 两库；把 B 从目录删掉后同一读取报 `DbNotInMdb{dbnum: B}`。
- 冻结语义回归：`pinned_sesno` 在两次读之间不变；文件替换后 `verify_file_identity` 仍报 `FileReplaced`。

### 5.2 源码护栏（`tests/refno_native_guards.rs`，`include_str!` 扫源）
- `direct_store.rs` 不含 `resolve_ref0(`、`dbnum_of_ref0(`。
- `model_source.rs` 不含 `DBNUM_BY_REF0`，`dbnum_of_root` 函数体不含 `for pin in` / `find_element(`（若 Q2 决定保留单库点查，则允许一次 `element_exists(`）。
- `element_attributes.rs` 不含 `mdb_memory_store::ensure(`。
- 生产源码不含字面 `8188`（测试与基线文件除外）。
- `DirectStoreError::UnresolvedRef0` 不再存在。

### 5.3 现有回归（必须全绿）
`web_service::tests`、direct tree TTY 对拍、direct attribute/UDA parity、model generation root parity、CATA dependency closure、
MDB Global resolver、E3D 2.1 与 3.1 schema pack、`tests/refno_native_api_contract.rs`。

### 5.4 真库（手动，记入 `docs/2026-08-12_live-test-ledger.md`）
- ams8000：`direct_attmap_probe --owner-climb --nouns EQUI,BRAN,GENSEC`，对账栏 100% 相等；跨库引用换库成功数不低于改前。
- JEU 离线：`/api/v1/attributes/24572/115` 与 TTY 相同；日志里打开的库数 = 1 + 实际依赖数；不出现「DESI 骨架 N 个元素」于属性服务构造期
  （树服务仍会打印——在案，3B）。

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

## 8. 需要评审拍板

1. `MdbRouteDirectory` 是否同时实现 `CataDbLocator` 兼容壳（推荐：是，`dbnum_of_ref0` 恒 `None`，让 `DirectStore::new` 签名本切片不动）；
   还是直接把 `DirectStore` 改成持 `Arc<dyn DbFileResolver>`（改动面更大，牵动 7 处构造）。
2. `dbnum_of_root` 是否保留一次目标库 `element_exists` 点查以区分「编号合法但元素不存在」（推荐：保留，代价是开一库一次点查）。
3. `ElementAttributeService` 的 UDA catalog 本切片是「只读 DICT 构造、不缓存」还是直接做成母计划 5.2 的 `UdaCatalogProvider`（推荐：前者，provider 留给阶段 5）。
4. `snapshot_epoch` 改源是否可接受，或本切片先保持从骨架取（意味着属性服务仍要 `mdb_memory_store::get()` 但不 `ensure()`）。
