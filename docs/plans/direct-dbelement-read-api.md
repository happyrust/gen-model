# 计划：pdms-io 直读 API（`Db` / `DbElement`）与免入库生成

> 状态：**草案**（2026-08-29 提出，待 ADR 裁决）。
> 目标形态由用户指定：「按 db1~db5 的方式和 DbElement 的方式去解析我们需要的数据，有这些接口后，
> 可以直接按 pdms-io 去读取解析数据，不需要提前缓存到数据库，再去数据库去生成模型。」
> 会话上下文：`.context/会话-2026-08-29-direct直读可行性核查.md`。
> **2026-08-30 审核回写**：specs/034 **T402 定为 D1 硬前置**（消双时钟读路径）；**D2 加 T501 前置门**
> （分类/遍历/攀爬判据单一来源，转调 `db4::Core3dSemantics`）；CATA 时点规则定稿于
> `pdms-io-v2-core3d-alignment.md` §2 末；pdms-io.git 本地 patch 模板见根 `Cargo.toml` 注释段
> （LOCAL-DEPS PATCH END 之后）。审核档案：`上下文/会话-2026-08-29-模型生成重构审核.md`。

## 与 ADR-053 的关系（必须先说清）

`ADR-053`（已接受）Q1 选的是 **A**：*只把「生成期读数据」改 direct，产物仍写 RocksDB，SurrealDB 仍是数据权威*；
并把「连数据管线也 direct、Surreal 退役」（Q1 选项 **B**）明确列为 **Non-Goal**。

本计划要的「不需要提前缓存到数据库」= **Q1 选项 B**。因此本计划：

- **D1–D5 阶段完全落在 ADR-053 已接受的范围内**（是它 P1–P3 的具体化 + API 门面化），不需要改 ADR；
- **D6 阶段越界**，需要一份 ADR 增补（暂命名 `ADR-054：免入库直生的范围与权威边界`）才能开工。

这样切的理由：D1–D5 交付后，「生成不查库」已经成立且可对拍；D6 只是把「谁是数据权威」这一条改掉，
它拦路的东西（名字索引、反向引用、增量水位、MDB 成员判定）在 D3 就会被逐个摆到台面上，届时再裁决成本最低。

## 对标的 API 表面（RE 存量，非臆造）

依据 `.ida_scratch/e3d_dbelem_api.txt`、`.ida_scratch/e3d_mdb_api.txt`、`.ida_scratch/e3d_netapi{,2}.txt`
（`Aveva.Core.Database` 的反射导出），以及 `teach/learning-records/0002/0003/0004/0009`。

### `MDB`（≈ 我们的 `DirectMdb`）

| AVEVA 成员 | 语义 | 本计划 |
|---|---|---|
| `Db[] GetDBArray()` / `GetDBArray(DbType)` | 枚举 MDB 里的库 = **db1~dbN** | D1 做 |
| `Db GetDB(Int32)` / `GetFirstDB/GetNextDB(DbType)` | 按序号/类型取库 | D1 做 |
| `DbElement FindElement(String)` / `FindElement(DbType, String)` | 全 MDB 按名字找元素 | **D3**（需名字索引） |
| `DbElement GetFirstWorld(DbType)` | 某类型第一个 WORL | D1 做 |
| `Claim/Release/SaveWork/Refresh/GetWork` | 写侧与并发控制 | **不做**（只读） |

### `Db`（≈ 我们的 `DirectDb`，一个 dabacon 文件一个）

| AVEVA 成员 | 语义 | 本计划 |
|---|---|---|
| `Int32 Number` | dbnum | D1 做（文件头 `db_no`） |
| `String Name` / `DbType Type` / `Int32 ExtractNumber` | 库名/类型/extract 号 | D1 做 |
| `DbElement World` / `DbElement[] WorldMembers()` | 库根与其成员 | D1 做（`refno_index` 已能识别 `WORLD_NOUN`） |
| `DbElement FindElement(String, DbAttribute)` / `ElementExists` | 库内按名字找 | **D3** |
| `ElementsChangedSince(DbSession)` 等 6 个 | 会话差分 | **已有等价物**：`pdms_io::session_index_diff` |
| `DbElement DbItem` | 库自身在 SYS 库里的元素 | D3（需 SYS 库） |

### `DbElement`（≈ 我们的 `DbElement` 句柄）

只取**读侧**（`Set*`/`Create*`/`Delete`/`Copy*`/`Claim` 一律不做）：

| AVEVA 成员 | 本计划映射 |
|---|---|
| `Int32[] RefNo()` / `Int32 DbNo()` | `refno()` / `db_no()` |
| `DbElementType ElementType` / `GetActualType()` | `element_type()`（noun hash → 名字，走 `dict`） |
| `DbElement Owner` | `owner()` |
| `DbElement[] Members()` / `Members(DbElementType)` | `members()` / `members_of_type()` |
| `FirstMember()/LastMember()/Member(i)/Next()/Previous()` | 同名游标方法（**不物化列表**，对齐 core.dll `NXTITM`） |
| `GetString/GetAsString/GetInteger/GetDouble/GetBool/GetDate` | 同名 typed getter |
| `GetStringArray/GetIntegerArray/GetDoubleArray/GetBoolArray` | 同名数组 getter |
| `GetPosition/GetOrientation/GetDirection` | 同名（几何三件套） |
| `GetElement(DbAttribute)` / `GetElementArray(DbAttribute)` | `get_element()` / `get_element_array()`——**跨库跳转，必须过 ref0 定位器** |
| `GetAttribute(attr, DbQualifier)` / `(attr, Int32)` | qualifier / 下标重载（`whole_attmap` 已有 qualifier 语义） |
| `IsValid/IsNull/IsDeleted` | 同名 |
| `AtDefault(attr)` / `IsAttributeValid(attr)` | 走 attlib dict 的默认值/schema 判定 |
| `Evaluate*(DbExpression)` / `GetExpression/GetRule` | **D2 末尾**，复用 `parse_explict_tools` 的表达式求值 |
| `HasElementChangedSince(DbSession)` 等 | **不做**（增量侧已有 `session_index_diff`） |

## 现状盘点（这些已经有了，不要重写）

| 能力 | 位置 | 状态 |
|---|---|---|
| 会话 B-tree 单点定位（2 KB 页、`0x28`→会话页、`+0x1C`→索引根、16 B 索引项） | `parse_pdms_db/src/refno_index.rs` | ✅ 与 core.dll 逐字段一致 |
| 页式引擎（只读索引页+命中记录页） | `parse_pdms_db/src/paged.rs` + `pdmsdb_engine_v2` | ✅ 生产默认；实测 ref0 全扫 `record_pages_read=0`、`bytes_read ≤ 15%` 文件 |
| 带 sesno pin 的定位 + 页缓存 | `pdms_io/src/io.rs::search_latest_refno` / `index_page_cache` | ✅ 唯一支持任意历史时点的入口 |
| 记录→元素（属性/UDA/children/owner） | `parse_pdms_db::parse::parse_ele_data_with_info` → `EleData{refno,owner,noun,children,whole_attmap}` | ✅ |
| attlib 字典（noun/属性 schema、词哈希） | `parse_pdms_db::dict::NounClassifier` | ✅ 1931 noun 交叉验证 |
| `ref0 → dbnum` 定位器（含冲突登记、磁盘指纹缓存） | `src/data_interface/cata_closure.rs::InMemoryCataLocator` | ✅ 生产在用 |
| 按需单库会话（legacy/compare/paged 三档，默认 paged，开库 fail-closed 校验） | `src/data_interface/on_demand_db.rs::OnDemandDbSession` | ✅ CATA 走这条 |
| direct vs DB attmap 逐字段对拍 | `src/bin/direct_attmap_probe.rs` | ✅ 8000/7333 共 200 样本 0 真值冲突 |
| MDB → 库列表（`MDB.CURD`） | `src/mdb.rs::get_project_mdb`（走 TiDB）、`src/data_interface/mdb_membership.rs` | ⚠️ 现有实现依赖库侧，D3 要出文件侧版本 |

**结论：L0 存储层完备，缺的是 L1 会话/定位门面、L2 元素句柄、L3 两个索引、L4 生成接入。**

## 分层设计

```
L4  生成接入      direct read-context 路由（fail loud，不静默回落 DB）
L3  索引层        name→refno 索引 / 反向引用索引（仅免入库形态需要）
L2  元素句柄      DbElement：惰性、按需拉记录、进程内 attmap 缓存
L1  会话与定位    DirectMdb → DirectDb(db1..dbN) → ref0 定位器 → 会话池
L0  存储（已有）  PagedDbSession / PdmsIO / refno_index / dict / paged
```

### L1 关键类型（签名草案）

```rust
/// 时点语义：两种形态一次说清，避免 D5/D6 各写一套。
pub enum TimePoint {
    /// 与 DB 模式同一逻辑时点（ADR-053 Q3=A），对拍用。
    Pinned(u32),
    /// 文件最新会话（Q3 选项 B），免入库直生用。
    Latest,
}

pub struct DirectMdb {
    name: String,
    dbs: Vec<Arc<DirectDb>>,          // ≈ MDB.GetDBArray()，即 db1~dbN
    ref0_index: Arc<dyn CataDbLocator>,
}

impl DirectMdb {
    pub fn open(project: &ProjectPaths, mdb: &str, at: TimePoint) -> Result<Self>;
    pub fn dbs(&self) -> &[Arc<DirectDb>];                 // GetDBArray()
    pub fn dbs_of_type(&self, t: DbType) -> Vec<Arc<DirectDb>>;
    pub fn db(&self, number: u32) -> Option<Arc<DirectDb>>; // GetDB(n)
    pub fn first_world(&self, t: DbType) -> Result<Option<DbElement>>;
    pub fn element(&self, refno: RefU64) -> Result<Option<DbElement>>; // ref0→db→定位
    pub fn find_element(&self, name: &str) -> Result<Option<DbElement>>; // D3
}

pub struct DirectDb {
    number: u32,                       // Db.Number
    name: String,                      // Db.Name
    db_type: DbType,                   // Db.Type
    extract: u32,                      // Db.ExtractNumber
    path: PathBuf,
    at: TimePoint,
    session: Mutex<DbSessionHandle>,   // 页缓存 + 文件句柄
}

impl DirectDb {
    pub fn world(&self) -> Result<DbElement>;              // Db.World
    pub fn world_members(&self) -> Result<Vec<DbElement>>; // Db.WorldMembers()
    pub fn element(&self, refno: RefU64) -> Result<Option<DbElement>>;
    pub fn page_stats(&self) -> PageReadStats;             // 可观测性，必须暴露
}
```

`DbSessionHandle` **不做时点分流**：`Latest` 与 `Pinned(sesno)` 共用页式引擎同一条实现。
**specs/034 T402（引擎 `db2/session.rs::open_at(path, sesno)`）是 D1 的硬前置**——P4 两条根因
（`0x34` 按字解释、页大小假匹配拒绝）已在 engine-v2 落地（`348d187`/`cb7dd95`），T402 只剩薄薄一层
入口，先补它再开工 D1。**不允许**「`Pinned` 走 old `PdmsIO::search_latest_refno`、`Latest` 走
`PagedDbSession`」的双路径兜底：同一读语义两条代码路径正是 T402 要消灭的东西，「补不了就长期二选一」
的兜底极易固化成永久双实现（2026-08-30 审核修订，原文即此兜底，已废除）。`page_stats()` 照常暴露页读统计。

### L2 元素句柄

```rust
#[derive(Clone)]
pub struct DbElement {
    refno: RefU64,
    dbnum: u32,
    store: Arc<DirectStore>,   // 回指 MDB + 会话池 + 缓存
}
```

要点：

1. **句柄不是快照**。`DbElement` 只存 `(refno, dbnum)`，属性在第一次 getter 时才拉记录。
   这样 `members()` 返回 N 个句柄不等于解析 N 个元素——对齐 core.dll `NXTITM` 游标语义（`teach/0009`）。
2. **`attmap()` 带进程内缓存**，键 `(dbnum, refno, TimePoint)`，对齐 DB 模式的 `#[cached]` 语义。
3. **跨库跳转**：`get_element(attr)` 拿到 `RefU64` 后必须走 `DirectMdb.ref0_index`，
   不能假定在同一个库（DESI→CATA、DESI→SITE 库都会跨）。
4. **fail loud**：定位不到 / 解析失败一律 `Err`，不返回空元素。P0 探针里 `parse_element` 失败按元素单列的姿态保留。

### L3 语义适配（ADR-053 Q4 同源）

`EleData → NamedAttrMap` 的转换必须与写库侧同源抽取，不做第二实现。P0 已经把语义规格定死：

- 词属性按 attlib dict **反哈希成 `WordType`**（对齐 DB 读侧视图，`0` → 空串）；
- 「仅含不可见字节的串」归一为空串（P0 的 `empty_decode_artifact` 分类）；
- `TYPEX`/`UNIPAR`/`SPAMAP` 等生成不消费的键保留 direct 原值（超集无害）；
- `SESNO` 按元数据处理（与 `REFNO`/`TYPE` 同）。

P0 探针转正为这个转换器的回归测试。

## 阶段

每阶段的「验收」都是可执行判据，不是形容词。

### D0 · 可行性探针 —— **已完成**（2026-08-29）

- `src/bin/direct_attmap_probe.rs`；dbnum 8000（120 样本）+ 7333（80 样本）**0 真值冲突、0 单侧缺元素**。
- 性能（debug）：direct 5.0–11.0 ms/元素 vs DB 13.0–15.9 ms/元素。
- 遗留：CATA 侧对拍未做（转 D5）。

### D1 · 会话与定位层：`DirectMdb` / `DirectDb`（db1~dbN）

> **硬前置（2026-08-30 审核回写）**：
> ① specs/034 **T402**——引擎 `db2/session.rs::open_at(path, sesno)`，并经 vendor
> `old-parse-pdms-db/src/paged.rs` 透传为 `PagedDbSession::open_at`；两种 `TimePoint` 共用一条实现。
> ② **pdms-io.git 本地 patch 入口已打开**（模板在根 `Cargo.toml` 注释段）——否则 engine-v2 里的
> T402 在上游提交 + 升 rev 之前 gen-model 根本消费不到，D1 无从联调。
> ③ **CATA 时点规则已定稿**（`pdms-io-v2-core3d-alignment.md` §2 末）——D1 的会话池要同时容纳
> DESI 与 CATA 会话，时点语义先定死再动工。
> D1 本身不涉分类/遍历判据，满足以上三条即可先行，不必等 T501。

交付物

- 新增 `src/data_interface/direct/mod.rs`、`direct/store.rs`、`direct/db.rs`。
- `DirectStore`：`DashMap<dbnum, Arc<DirectDb>>` 会话池 + `Arc<dyn CataDbLocator>` + `dict::NounClassifier` 单例。
- `TimePoint` 两态贯通；`DirectDb::page_stats()` 暴露 `bytes_read / index_pages_read / record_pages_read`。
- 库清单来源二选一（D1 先做 a，D3 补 b）：
  a. 复用 `dbnum_watermark` + `project_paths` 扫描（与 `InMemoryCataLocator::build_for_project` 同一条）；
  b. 从 SYS 库读 `MDB.CURD`（纯文件侧，见 D3）。

验收

- 新增 `src/bin/direct_db_probe.rs`：`--mdb ALL` 打印 db1~dbN 一行一条（number / name / type / extract / path / sesno / world refno）。
- AMS 的 `ALL` MDB 至少列出 DESI 7997/7998/7999/8000；每个库能取出 `world()` 且 refno 与库里 `pe` 的 WORL 一致。
- 单库打开的 `bytes_read ≤ 文件 1%`（只读头 + 会话页 + 索引根，不该碰记录页）。

风险与对策

- 多 extent（`_0002+`）库当前会被 `first_extra_extent()` 逼回 legacy 全文件读。
  → D1 先**显式拒绝**（`Err` 并点名文件），不静默退化；本机 E3D3.1 的 1002 个 dabacon **0 个**多 extent，不阻塞。
- 页大小自适应有坑（引擎把文件头 `0x34` 的「字」当「字节」，真库 490 个里 17 个中招，`ams7329_0001` 读出 `sesno=0`）。
  → 沿用 `page_size_hint: Some(0x800)`，并在 `DirectDb::open` 断言 `page_size == 0x800`。

### D2 · 元素句柄层：`DbElement`

> **前置门（硬，2026-08-30 审核回写）**：specs/034 **T501** 完成之前本阶段不开工——
> 即 engine-v2 侧 P0–P4 经上游提交、`vendor/old-parse-pdms-db` 升 `pdmsdb_engine_v2` rev
> 并暴露 `db4::Core3dSemantics` 与 `core3d_model` 之后，D2 才动工。
> 理由：D2 的分类/遍历/攀爬（`element_type` 分类、`members`/游标族的收集与下潜判据、
> `significant_owner`/`climb`）按 T503 必须是**转调 `db4::Core3dSemantics` 的薄封装**
> （「gen-model 内不得存在第二份判据实现」，`rg` 抽查作验收）；T501 之前开工只能本地再实现
> 一遍判据，违反宪法 II，到 T503 还得拆掉重写。
> 开发期可用 pdms-io.git 本地 patch 提前联调，但 D2 的**验收**必须在钉回正式 rev 后复跑。

交付物

- `src/data_interface/direct/element.rs`：上文 L2 的全部读侧方法；其中分类/遍历/攀爬一律
  转调 `db4::Core3dSemantics`，本仓不实现判据（specs/034 T503）。
- `src/data_interface/direct/convert.rs`：`EleData → NamedAttrMap` 同源转换（Q4），P0 的四条归一规则内建。
- 游标族（`first_member/next/previous/member(i)`）**不物化 children 列表**。
- 表达式：`get_expression/evaluate_*` 复用 `parse_pdms_db::parse_explict_tools`（放本阶段末尾，可切到 D4）。

验收

- `direct_attmap_probe` 改走 `DbElement::attmap()`（而不是裸 `whole_attmap.merge()`），
  8000/7333 复跑仍 **0 真值冲突**，且「词哈希归一」「空字节串」两类残差归零（因为转换器已内建）。
- 新增单测：跨库 `get_element()`（DESI 元素 → CATA SCOM）能跳到正确 dbnum。
- `members()` 返回 N 个句柄时 `record_pages_read` 增量为 **1**（只读了自己那一条记录）。

### D3 · 两个索引：名字查找 与 反向引用

这是「免入库」真正缺的两块拼图，`Db.FindElement(name)` / `MDB.FindElement(name)` 都卡在这里。

交付物

- **name→refno 索引**：逐库遍历叶子页一次产出，落磁盘缓存 + 文件指纹校验（与 `scan_identity_ref0s` **同一次遍历**顺带产出，不额外付 I/O）。
- **反向引用索引**：同一次遍历里把每个元素的 outbound refs（`CATR`/`SPRE`/`PRTREF`/`OWNER`…）反转落盘。
  文件里没有反向边，这是 direct 唯一必须「预建」的东西——也正是 ADR-053 里 back-ref 被标成「部分」的那一格。
- **纯文件侧 MDB 成员**：从 SYS 库（`amssys`）读 `MDB` 元素的 `CURD`，替掉 `src/mdb.rs` 走 TiDB 的实现。

验收

- `DirectDb::find_element("/1RX-RM03-R301")` 与 SurrealDB 里 `pe` 按 name 查的结果一致（抽 200 个名字）。
- 反向索引与 Surreal 侧反向边对拍：随机 200 个 refno 的入边集合一致。
- 索引构建耗时与体积入档；二次打开（指纹命中）耗时 < 50 ms/库。

判据（这一阶段决定 D6 值不值得做）

> 如果 name/反向索引的构建成本 ≈ 现在的全量解析入库成本，那「免入库」就只是换了个存储格式，
> 不值得做。D3 结束必须给出这两者的耗时/体积对比数字，作为 ADR-054 的输入。

### D4 · 生成链接入（direct read-context）

交付物

- aios_core 仿 `active_staging_reads()` 加 `active_direct_reads()`（task-local），provider trait 定义在 aios_core、实现在 gen-model。
- 收口函数入口路由（ADR-053 已盘出的清单）：
  `get_named_attmap`(16) / `get_world_transform`(9) / `query_single_by_paths`(5) /
  `query_multi_deep_versioned_children_filter_inst`(5) / `query_group_by_cata_hash`(4) / `get_cat_refno`(4) /
  `get_children_named_attmaps`(4) / `get_type_name`(3) / `get_children_pes`(3) / `query_filter_children`(3) /
  `query_filter_deep_children_atts`(2) / `get_or_create_cata_context`(2) / 表达式求值(3)。
- 与 staging 读上下文互斥断言（ADR-053 R6）。
- 配置：`DbOption.toml` 加 `model_gen_mode = "db" | "direct"`，默认 `db`。

验收

- `model_gen_mode = "db"` 时所有查询路径**逐字节不变**（不进上下文即旧路径）。
- direct 上下文内未覆盖的查询**显式报错**，不静默回落 DB；`/health` 暴露 provider 命中/未覆盖计数。
- aios_core 改动在 patch-on 与 patch-off（升 rev 后）两态都 `cargo check` EXIT=0。

### D5 · 对拍与基准

交付物

- `src/bin/direct_gen_smoke.rs`（`cata_smoke` 同款）：同一批生成根，db/direct 各跑一遍，逐元素比 inst/geo hash。
- 覆盖：BRAN（管件+TUBI）、EQUI（图元）、GENSEC（扫掠+目录闭包）、含 UDA/表达式属性的根、跨库引用根、**CATA 侧 attmap 对拍**（补 D0 遗留）。
- 基准矩阵：单根 / 百根 / 整 dbnum × db vs direct × 冷/热。

验收

- inst/geo hash **逐元素一致**，未覆盖查询计数 = 0。
- direct 批量端到端不慢于 DB 模式；单点冷读显著快于 DB 冷查询。不达标要有归因，不许含糊过去。

### D6 · 免入库形态（Q1-B，**需 ADR-054 先裁决**）

要动的不是解析，是这四样对「库侧全量视图」的依赖：

1. **增量水位 / durable pending / 暂存窗口**（ADR-017/021/025/037）——现在靠库里的 `dbnum_watermark` 与 pending 表；
2. **房间/空间后处理**——ADR-053 已声明留在 Surreal；
3. **MDB 成员判定与 watch 范围**（`update_scope.rs`）；
4. **反向引用消费方**（ADR-002/003 的 B 工作流）。

交付物（若 ADR-054 通过）

- `TimePoint::Latest` 的直生入口：不经摄入，按文件最新会话直接生成。
- 水位与 pending 的文件侧等价物（或明确保留库侧簿记，只把「数据」搬走）。

验收

- 空库冷启动：不做任何全量解析入库，直接对一批生成根产出 inst/geo，与 DB 模式 hash 一致。

## 文件清单

新增

- `src/data_interface/direct/mod.rs` / `store.rs` / `db.rs` / `element.rs` / `convert.rs`（D1–D2）
- `src/data_interface/direct/name_index.rs` / `backref_index.rs`（D3）
- `src/bin/direct_db_probe.rs`（D1）、`src/bin/direct_gen_smoke.rs`（D5）
- `docs/adr/ADR-054-...md`（D6 前置）

改

- `../vendor/old-parse-pdms-db/src/paged.rs`：`PagedDbSession::open_at(path, sesno)` 透传
  （**D1 硬前置**；引擎侧实现 = specs/034 T402 的 `pdmsdb_engine_v2::db2/session.rs::open_at`）
- `../vendor/old-aios-core`：`active_direct_reads()` + 收口函数路由（D4，随后升 rev）
- `src/data_interface/cata_closure.rs`：定位器与闭包引擎复用出口（D1/D3）
- `src/mdb.rs`：MDB 成员改文件侧（D3）
- `DbOption.toml` + `src/options.rs`：`model_gen_mode`（D4）

复用不改

- `refno_index.rs`、`dict.rs`、`pdmsdb_engine_v2`、RocksDB 产物写入、房间/空间管线。

## 实施原则

- **零回归**：`model_gen_mode` 默认 `db`；不进 direct 上下文时行为逐字节不变。
- **fail loud**：未覆盖的查询、定位不到的 refno、多 extent 库，一律显式报错，不静默降级（降级会让对拍假绿）。
- **同源优先**：DB 模式已有的语义（名字映射、qualifier、UDA、children 序）只写一次。
- **语义红线**：direct 只改「数据从哪读」，不改生成算法、`cata_hash` 复用、产物写入与房间管线。
- **可观测**：每个 `DirectDb` 暴露页读统计；`/health` 暴露模式与 direct 覆盖率。
- 改 Rust 跑 `cargo fmt` + `cargo check`；aios_core 走 `scripts/Toggle-LocalDeps.ps1 -On` 本地 patch → 上游 → 升 rev，不得带 patch 推 main。

## 风险

| # | 风险 | 对策 |
|---|---|---|
| R1 | 转换语义漂移（qualifier/UDA/表达式） | Q4 同源转换 + P0 探针转正为回归测试 |
| R2 | 跨库 owner 链（DESI→SITE 库） | ref0 定位器兜底；`get_world_transform` 上溯逐级 attmap，浅且有页缓存 |
| R3 | 查询面盘点遗漏（espec/管件特化长尾） | fail loud + 覆盖率计数；D5 让长尾现形 |
| R4 | `PdmsIO` 是 `&mut self`，并发争用 | 按 dbnum 分锁的会话池；D5 量化锁等待，超阈值再分片/只读句柄 |
| R5 | 文件与水位竞态 | `TimePoint::Pinned` 天然免疫「读到未应用会话」；文件替换沿用文件身份守卫 |
| R6 | 与 staging 读上下文叠加 | 入口断言互斥 |
| R7 | **多 extent 库退化为全文件读** | D1 显式拒绝并点名；真要支持需页式引擎补 extent 寻址 |
| R8 | **页大小探测误判**（490 个真库文件里 17 个） | `page_size_hint: 0x800` + 打开时断言 |
| R9 | **名字/反向索引的构建成本可能吃掉免入库的收益** | D3 结束给出与现全量入库的耗时/体积对比，作为 ADR-054 的裁决输入 |

## Non-Goals

- 写侧 API（`SetAttribute`/`Create*`/`Delete`/`Claim`/`SaveWork`）——本计划只读。
- 房间/空间后处理 direct 化。
- fork 世界（rs-core v0.3.2）整库替换（`scope-a-full-crate-swap-feasibility.md` 维持 A0 结论）。
- D6 之前不动 SurrealDB 的数据权威地位。
