# 2026-09-04 old-parse-pdms-db 退役清单（第二张清单）与落地状态

- 承接 `2026-09-03-old-pdms-io-retirement-inventory.md` §6：那份清单把 `parse_pdms_db` 排在阶段 8，
  并要求「单独一张清单」。本文就是那张清单——按 `parse_pdms_db` 在本仓的**用法性质**分组清点，
  逐组给替身，并记录 2026-09-04 已经落地的部分。
- 事实基线：2026-09-04 凌晨工作树（HEAD `9c849a86`，`e3d-direct`）。清点前 `rg parse_pdms_db src/`
  命中 **34 个文件**（清单 §6 写 31，是 09-03 的数）；另有 `tests/` 5 个、`python/src` 3 处。
- 与 old-pdms-io 的关系：old-pdms-io 2026-09-04 已由另一条会话（fable-5-1-24）整体请出本仓
  （连 optional 都不是，`Check-LegacyPdmsIo.ps1` 四条全绿）。`parse_pdms_db` 因此只剩本仓一条
  直接依赖边（`cargo tree --workspace -e normal --locked -i parse_pdms_db --depth 1` = `aios-database`）。

## 0. 完成判据（与 old-pdms-io 同款，主语换成 `parse_pdms_db`）

1. `Cargo.toml` 里 `parse_pdms_db` 变成 `optional = true`，只由 `legacy_parse_pdms_db` 打开；
2. `cargo tree --workspace -e normal --locked -i parse_pdms_db` 匹配不到任何包；
3. 源码护栏钉住 `src/` 下除 feature 门后的探针外不含 `parse_pdms_db::`。

## 0.0 落地状态（2026-09-04，会话 fable-5-1-22 → fable-5-1-29）

| 判据 | 状态 | 说明 |
|---|---|---|
| 1 optional + 门 | ⏳ | feature `legacy_parse_pdms_db` 已声明（今天是**空门** `[]`），8 个探针 bin 与 `full_attributes_real` / `pdms_record_boundary` 两个测试已挂 `required-features`；依赖本身仍无条件——被 §3 挡住 |
| 2 默认依赖图无它 | ⏳ | 直接依赖边只剩 `aios-database` 一条（python 侧已清零）；边本身要等 §3 |
| 3 源码护栏 | ✅（不许再长） | `data_interface::mod.rs::legacy_parse_pdms_db_guard` 三条 + `scripts/Check-LegacyParsePdmsDb.ps1` 四条：lib 侧只许 `versioned_db/{database,member_prune}.rs` + `ele_data.rs` 门后转换提到它；`src/bin/` 与 `tests/` 里读它的目标**都必须挂门**（B7 之后夹具构建器与集成测试的点名表已撤——默认 features 下它们一处都不读 legacy 解析器） |

lib 侧（默认 features）的 `parse_pdms_db::` 代码行：**34 个文件 → 2 个文件**（`versioned_db/database.rs`、
`versioned_db/member_prune.rs`），全部属于 §3 那一条基线全量解析路。`src/bin/` 与 `tests/` 在默认 features
下是 **0 处**（读它的 8 个探针 + 2 个测试全在门后）。

## 0.1 收口（2026-09-04，B8–B9 一次做完；本清单到此关闭）

**三条判据全绿，而且第 1 条是按更强的形态收的**：用户 2026-09-04 08:44 在退役收口方案
（`2026-09-04-legacy-parser-retirement-endgame-plan.md`）Q1 上拍 **A = 整体删**，不走「optional + 门」这个中间站——
依赖、`[patch]`、`legacy_parse_pdms_db` feature、8 个取证探针 bin、2 个门后测试、`ele_data.rs` 的
`From<legacy EleData>` 一起删；`Cargo.lock` 重解后少 5 个包（`parse_pdms_db`、`pdmsdb_engine_v2`、`dynfmt`、
`hash32-derive`、`erased-serde 0.3.31`），**只少不多**。

| 判据 | 状态 | 实据 |
|---|---|---|
| 1 optional + 门 | ✅ 以「整体删」超额满足 | manifest 里既没有依赖也没有 feature——门不存在，因为被门住的东西不存在了 |
| 2 默认依赖图无它 | ✅ | `cargo tree --workspace -e normal --locked -i parse_pdms_db` → `did not match any packages`；`-i pdms_io` 同 |
| 3 源码护栏 | ✅ 翻面 | `legacy_parse_pdms_db_guard` 从「只许在门后」改成「manifest 与源码一处都不许有」；`Check-LegacyParsePdmsDb.ps1` 五判并进 CI（`windows-tests.yml`） |

§3 那条阻塞（基线全量解析）由 B8.2–B8.4 解掉：基线换成 e3d-io 的**退化窗口**
（`e3d_collector::stream_baseline` + `versioned_db::baseline::render_baseline_batches`），与增量路一份收集、
一份渲染；`member_prune.rs` 与 `versioned_db/pe.rs` 两个文件删除。**代价与副作用逐条记在 changelog.md
2026-09-04「移除」第二条**，包括两处与方案写法不同的地方（保留 `INSERT IGNORE` 批的理由、WORL 写全三样行）。

**仍未闭合的不是本清单的事**：B8.5 第 3–6 条的真库外部裁决（5 库形状 diff、448 库耗时与峰值内存、
E3D TTY 抽样、`live_ams8000_*` 回归）——TTY 值 / 成员序对照完成前不宣称外部等价。

## 1. 依赖现状

| 项 | 值 |
|---|---|
| 声明 | `Cargo.toml` `parse_pdms_db = { git = "…/old-parse-pdms-db.git", rev = "005936a6" }`，非 optional |
| patch | `[patch."…/old-parse-pdms-db.git"] parse_pdms_db = { path = "../vendor/old-parse-pdms-db" }`（LOCAL-DEPS 开着） |
| 传递链 | `parse_pdms_db → aios_core`（同一份 patch 到 `../vendor/old-aios-core`）、`parse_pdms_db → pdmsdb_engine_v2`（git `efd2e19`）；把它摘掉，`pdmsdb_engine_v2` 一起走 |
| feature | `legacy_parse_pdms_db = []`（空门；基线换底座那天改 `["dep:parse_pdms_db"]`） |
| python | `python/Cargo.toml` / `python/src` 已无 `parse_pdms_db`（随 old-pdms-io 一起清） |

## 2. 按用法性质清点（34 文件全表）

状态列：✅ = 已换 e3d-io / 本仓实现；🔒 = 挂 `legacy_parse_pdms_db` 门（legacy 参照臂，默认不编译）；
⏳ = 基线路，随 §3 一起走。

### A. 数据模型类型 `EleData` —— 面最广，纯 DTO，六个字段全是 `aios_core` 类型

| 文件 | 原用法 | 现状 |
|---|---|---|
| `data_interface/e3d_collector.rs` | `use parse_pdms_db::parse::EleData`（收集器产物） | ✅ `super::ele_data::EleData` |
| `data_interface/ele_ops.rs` | `use parse_pdms_db::parse::*`（`PersistElementState::Add/Modified(EleData)`） | ✅ `pub use crate::data_interface::ele_data::EleData` |
| `uda_table.rs` | `full_attribute_view(&parse_pdms_db::parse::EleData, …)` | ✅ 本仓 `EleData` |
| `data_interface/increment_pipeline.rs` ×2、`increment_manager.rs`（`sql` 门）、`model_impact.rs`、`staging/parity.rs` ×2、`issue10_direct_add_node.rs`、`direct_window_replay_parity.rs` | 测试 / 门后代码里的 `EleData` | ✅ 本仓 `EleData` |

落点：新模块 `src/data_interface/ele_data.rs`——结构、派生、访问器（`att_map` / `explicit_attmap` /
`uda_atts` / `refno_enum` / …）与 legacy 逐字相同；`impl From<parse_pdms_db::parse::EleData>` 只在
`legacy_parse_pdms_db` 门后（给 `refno_attr_probe`、`full_attributes_real` 这类有意拿 legacy 解码器做
独立对照的地方用）。

### B. 文件头三个字 `DbBasicInfo` / `parse_file_basic_info`

| 文件 | 原用法 | 现状 |
|---|---|---|
| `data_interface/increment_manager.rs` | 60 字节头 → `DbBasicInfo{db_type, ses_pgno, db_no}`（自动重扫） | ✅ `db_file_header::DbBasicInfo::read` |
| `data_interface/manual_update.rs` | 同上（手动候选扫描） | ✅ `DbBasicInfo::from_header_bytes` |
| `data_interface/cata_closure.rs` | `scan_db_header` 用 e3d-io 读、却用 legacy 结构回传 | ✅ `DbBasicInfo::read`（并把库类型从 `descriptor().noun` 改回 `+0x20` 类型字，见 §5） |
| `versioned_db/database.rs:738` | CATA 候选枚举读头 | ✅ `DbBasicInfo::from_header_bytes` |

落点：`db_file_header::DbBasicInfo`（`from_descriptor` / `from_header_bytes` / `read`），三个字直接取
e3d-io `DbDescriptor` 的 `db_mark` / `db_type_name()` / `latest_ses_pgno`——正是 legacy `parse_file_basic_info`
数的 `+0x08` / `+0x20` / `+0x28` 三个偏移，`DbDescriptor::MIN_BYTES` 恰好也是 60。三条单测钉住偏移一致性。

### C. noun 字典（`parse_pdms_db::dict`）

| 文件 | 原用法 | 现状 |
|---|---|---|
| `data_interface/model_impact.rs` | `default_noun_capabilities().get(n).point`（生产）+ 三条契约测试 | ✅ `crate::noun_dict` |
| `fast_model/shared.rs` | `default_noun_capabilities().positive_equivalents()`（负体名单） | ✅ |
| `fast_model/coverage_audit.rs` | `default_noun_classifier()` + `uncovered_geometry_nouns` | ✅ |

落点：`src/noun_dict.rs` + `resource/dict/{noun_flags,noun_caps}.json`（与 `old-parse-pdms-db@88ac733`
仓根两份逐字节相同）。搬的是**消费面**（`NounFlags` / `NounClassifier` / `NounCapabilities` /
`NounCapabilityTable` / 两个全局默认表 / `routing_coverage_nouns` / `uncovered_geometry_nouns`）；
`attlib.dat` 解析器 `AttrDataFile`（导出器）**没搬**——它的长期去处（e3d-attlib 已是 attlib.dat 的新解析器）
是一条独立 ADR。五条单测：两份快照都解出且 noun 集一致、`hash_value` 与快照 hash 一致、
BOX/CYLI/… primitive 而 WORL/SITE/ZONE/EQUI 不是、两表几何 flag 一致、未覆盖名单 ⊂ 几何且 ∩ 覆盖名单 = ∅。

### D. 索引 / 属性读法

| 文件 | 原用法 | 现状 |
|---|---|---|
| `data_interface/extract_family.rs` | `parse_file_db_index_data` 两次 → `refno_table_map` 键集差（父层缺号数） | ✅ `ReadOnlyEngine::indexed_refnos`（活索引，见 §5） |
| `data_interface/mdb_membership.rs` | 整库 `parse_file_db_basic_data` + `gen_ref_type_pos_table` + 逐记录 `parse_raw_ele_data`，找 TYPE=MDB 且 NAME=/x 的元素读 `CURD`，再读每个 DB 的 `DBNO/PROJ/STYP/NAME` | ✅ `DbSet`（`sysvir.dat` 模板）`find_named` + `DbElement::extraction` → `direct_attmap::to_named_attmap`；对 `NamedAttrMap` 的读法逐字不变 |
| `bin/l3_suite.rs` | `parse_file_db_basic_data` → `refno_table_map.contains_key`（场景前置 refno 存在性） | ✅ `ReadOnlyEngine::search_index` |
| `bin/l3_suite/fixture.rs` | `parse_db_basic_info`（头）+ `parse_file_db_basic_data`（WORL refno） | ✅ `DbBasicInfo::read` + Header 档扫描找 `WORL` |

### E. 两个纯函数

| 文件 | 原用法 | 现状 |
|---|---|---|
| `fast_model/mesh_generate.rs` | `use parse_pdms_db::parse::round_f32`（**未使用**） | ✅ 删 |
| `pcf/bran.rs` | `use …::times_keep_f32_two_decimal_place`（**未使用**；`pcf` 整个模块在 `lib.rs` 里是注掉的） | ✅ 删 |

### F. 探针 bin（legacy 参照臂 / 直接读字节取证）

`attr_info_compat_probe`、`cata_closure_probe`、`cata_parse_probe`、`curd_parse_probe`、
`noun_layout_parse_probe`、`refno_attr_probe`、`refno_index_probe`、`world_probe` —— 🔒 全部挂
`required-features = ["legacy_parse_pdms_db"]`。`legacy_v2_read_parity` / `db8000_copy_probe` /
`incr_fold_probe` 已随 old-pdms-io 被删。

### G. 曾读 legacy 页式读法（`paged::PagedDbSession`）的夹具与测试 —— ✅ B7 已换（2026-09-04，fable-5-1-29 接手收尾）

| 目标 | 原用法 | 现状 |
|---|---|---|
| bin `db8000_two_delete_fixture`、bin `db_session_fixture`（`pipeline.rs`） | `PagedDbSession::{open, snapshot().sesno, read_raw_records}` 三连校验切出来的快照 | ✅ `db_file_header::snapshot_presence(path, refnos) -> SnapshotPresence{sesno, present}`（`ReadOnlyEngine::open` + 每个 refno 一次 `find_element`）；`db8000_two_delete_fixture::noun_at` 用同一点查 + 记录头第 3 字 `e3d_attlib::db1_dehash` 取 noun。两条仍是默认编译的夹具**构建器** |
| `tests/db8000_session_pairs.rs` | `PagedDbSession::open(snapshot)` + `read_raw_records` 取原始记录，`parse_raw_ele_data` 做 `element_view` | ✅ `element_view(&EleData)`：`DirectSchema::open_from_env` + `DbFileHeader::read` + `e3d_collector::open_pinned_set`（钉在快照头指针会话）+ `read_ele_data`——与生产 `collect_window` 同一底座。**差分 oracle（性质 c / f）现在对拍的就是 e3d-io 解码出的数据面终态** |
| `tests/db8000_two_delete_fixture.rs`、`tests/db_session_fixture_selfcheck.rs` | 同第一行校验切片 | ✅ `snapshot_presence` |
| `tests/pdms_record_boundary.rs` | 直接测 legacy 解析器的记录边界（`parse_raw_ele_data` / `parse_ele_membs` / `parse_ele_children`） | 🔒 挂 `required-features`（legacy crate 自己的单测放错了仓，随依赖一起删） |
| `tests/full_attributes_real.rs` | 有意拿 legacy 解码器对照 `q att` 清单 | 🔒 挂 `required-features`（转换在 `ele_data` 门后） |

护栏第 3 / 4 条随之从「点名表只许缩短」改成「读它就必须挂门」：`src/bin/` 与 `tests/` 默认 features 下
一处都不读 legacy 解析器（`legacy_parse_pdms_db_guard::every_{bin,integration_test}_on_parse_pdms_db_is_gated`、
`Check-LegacyParsePdmsDb.ps1` 第 3 / 4 条）。

### H. 基线全量解析 —— ⏳ 唯一还挡着判据 1/2 的东西，见 §3

`versioned_db/database.rs`：`use parse_pdms_db::parse::*`、`read_full_basic_data` →
`parse_db_basic_data(bytes, file_name, project) -> aios_core::db::DbBasicData`、`parse_file_with_chunk(db_basic,
…, chunk_refnos, ses_range_map, ignore_world_refno) -> PdmsDbData{total_attr_map, type_ele_map, db_no, …}`；
`versioned_db/member_prune.rs`：`parse_ele_membs(&bytes[pos - 4..])`（元素自己的成员块）；两文件的
`GEN_MODEL_OWNER_FIXTURE` 测试用 `parse_file_db_basic_data`。

## 3. 剩余阻塞：基线全量解析（`sync_total_async_threaded`）

### 3.1 它是什么

库第一次纳入时的整库落库（ADR-058 D3：「DESI 保留基线：库首次纳入按 1..file_latest 整库解一遍，
之后全是增量」）。今天的形状是 legacy 解析器的字节级流水线：

```
FrozenSource::capture(path)              e3d-io 底座：dbnum / 库类型 / 目标会话 / 冻结长度
  → read_frozen_bytes()                  按冻结长度读整个文件
  → parse_db_basic_data(bytes,…)         → DbBasicData{bytes, ses_pgno, world_refno, children_map, refno_table_map{refno→(pos,noun)}}
  → member_prune(children_map, bytes)    按元素自己的成员块（parse_ele_membs）裁掉越界父子边
  → all_refnos.chunks(n) 并行
      parse_file_with_chunk(db_basic, chunk, ses_range_map, …) → PdmsDbData{total_attr_map{refno→NamedAttrMap}, type_ele_map}
      preserve_unparsed_pe_metadata(…)   解不出属性的元素只保留 pe 元数据
  → 写 pe / ATT_* / 反向索引，sesno 由 ses_range_map（legacy_session_ranges）按元素所在页落区间
```

### 3.2 两条可走的路

| 路 | 做法 | 代价 / 风险 |
|---|---|---|
| **(a) 收敛成退化窗口**（ADR-058 D4 的数据面对应物；`2026-09-03-single-read-two-lanes-plan.md` 的方向） | 基线 = `collect_window(file, S=0, T=file_latest)`，走 `e3d_collector` → `EleData` → `ele_ops` 终态渲染，与稳态增量**同一条码路**；`sync_total_async_threaded` 整个下线 | 一次读整库的 `DbSet@T`（每元素 `extraction` + `to_named_attmap`），要量 448 库的耗时；`pe.sesno` 的口径从「元素所在页落进哪个会话区间」变成「终稿记录所在页反查的 last-touch 会话」（`e3d_collector` 今天对 Add 就是这么给的）——**要对拍**，429 库行级零差是门（09-03 清单 §8）；`member_prune` 的越界父子边问题在 e3d-io 的 `member_refnos()` 下不再存在（成员表本来就是元素自己的） |
| (b) 原地换底座 | `read_full_basic_data` 用 `ReadOnlyEngine::indexed_refnos` 造 `refno_table_map`、`scan_elements(Full)` 造 `children_map`；`parse_file_with_chunk` 换成 `DbSet` 逐元素 `to_named_attmap`；`ses_range_map` 照旧 | 保留了两套写路（基线 / 增量）与 `DbBasicData` 这个字节级中间态，换完仍是「两条路两个解析器」；只值得在 (a) 被否决时做 |

建议走 (a)：它同时是 ADR-058 D3/D4 与 single-read-two-lanes 计划的实施项，不是本清单私自加的活。
**前置**：09-03 清单 §A 的 T404 三条硬门（ams7999 45→46 / ams1112 721→722 / 429 库行级零差）——
`e3d_collector` 已是生产收集器，但基线路一旦也压上去，整库对拍就是必答题。

### 3.3 换完之后要做的收尾（判据 1/2）

1. `Cargo.toml`：`parse_pdms_db = { …, optional = true }`、`legacy_parse_pdms_db = ["dep:parse_pdms_db"]`；
   `[patch."…/old-parse-pdms-db.git"]` 段与 LOCAL-DEPS 注释里的 parse_pdms_db 句子一并删；
   `pdmsdb_engine_v2` 随之离开 `Cargo.lock`（patch 关着重生成 lock，属于那个提交）。
2. `Check-LegacyParsePdmsDb.ps1` 第 1 条改成 `did not match any packages`（同 `Check-LegacyPdmsIo.ps1`）；
   护栏 `LIB_ALLOWLIST` 缩成只剩 `ele_data.rs`。
3. ~~§G 的 5 个测试 / 2 个构建器改 e3d-io 读法或删~~ ✅ B7 已做（§G）；剩 `pdms_record_boundary.rs`（🔒 门后）随依赖一起删。
4. `db_file_header::legacy_session_ranges` 的注释（「基线全量解析…按元素所在页落进哪个区间」）随基线路一起改写。

## 4. 顺序

| # | 内容 | 前置 | 状态 |
|---|---|---|---|
| B1 | `EleData` 本仓化（§A） | 无 | ✅ 2026-09-04 |
| B2 | 文件头三个字（§B） | 无 | ✅ |
| B3 | 两个未使用的纯函数 import（§E） | 无 | ✅ |
| B4 | noun 字典消费面 + 快照（§C） | 无 | ✅ |
| B5 | extract / MDB / l3_suite 换 e3d-io 索引与属性读法（§D） | 无 | ✅ |
| B6 | feature 门 + 探针 bin 圈起来 + 两道护栏（§F、§0 判据 3） | B1–B5 | ✅（空门） |
| B7 | §G 夹具 / 测试换 e3d-io 读法 | 无（`tests/db8000_*` 的 old-pdms-io hunk 由 fable-5-1-24 先落，B7 在其上改读侧） | ✅ 2026-09-04（fable-5-1-22 起手、fable-5-1-29 收尾验证） |
| B8 | 基线收敛成退化窗口（§3 路 a） | T404 三条硬门 | ✅ 2026-09-07（分两半落地：**收集器 + 渲染器**早在 B8.2 / ADR-064 D4 就已与增量共用；**编排半边**按 `docs/plans/2026-09-07-retire-sync-total-async-threaded-plan.md` 收口——`sync_total_async_threaded` 壳整个删除，基线只剩 `versioned_db::baseline_ingest::ingest_file` 一个按文件的入口。T404 三门没有再压一遍：编排层重构不改数据形状，判据是「同样输入落同样的行」，真库对拍见该方案 §5） |
| B9 | optional + lock 重生成 + 护栏收紧（§3.3） | B8（B7 已完） | ✅ 2026-09-04（`06bb56fa`；**未等 B8，先于其落地**——依赖、feature、7 个残余探针与门后测试全删，`Cargo.lock` −91/+1，两道门进 `windows-tests.yml:117/:124`。10:59 实跑 `cargo tree -i parse_pdms_db` = `did not match any packages`、`Check-LegacyParsePdmsDb.ps1` exit=0 六判全过） |

## 5. 本轮引入的语义差别（对拍 / 排障时先看这里）

1. **活索引 vs 全页扫描。** legacy `parse_file_db_index_data` / `gen_ref_type_pos_table` 取文件里**每一页**
   索引页的叶条目，活的死的都算（e3d-io 在 429 库语料上量过：940 694 条对 789 831 条，16% 来自没有
   任何会话能到达的树）。换成 `indexed_refnos` / `search_index` 后，`parent_gap_refno_count`
   （`manual_update` 的「父层缺号 → 补解析父层」判定）与 l3_suite 的场景前置 refno 检查看到的都是
   **最新会话的活集合**——数字会变小，且这才是问题原义。
2. **不是描述符页的文件。** legacy `parse_file_basic_info` 对任何 60 字节都给一个结构（`db_type` 空串）；
   `DbBasicInfo::from_header_bytes` 交给 `DbDescriptor::parse` 裁决，类型字非 0 / 页大小非法直接 `None`。
   三条调用路径此前都把空 `db_type` 当跳过，行为等价；差别只是自动重扫会对这类文件打一行
   「跳过无法读取 E3D 数据库头的文件」（候选本来就先过 `is_candidate_db_file` 名字白名单，几乎不出现）。
3. **`cata_closure::scan_db_header` 的库类型字。** 旧码反哈希的是 `descriptor().noun`（e3d-io 现版里是 `+0x04`
   第 0 页自身的 noun 字），现在与另外三条路一样读 `+0x20` 的 `db_type_hash`。
4. **`mdb_membership::resolve` 多了一个运行期前提**：需要 `AIOS_E3D_TEMPLATE_DIR`（或默认目录）下的
   `attlib.dat` + `sysvir.dat`（`DirectSchema::open_from_env`）。direct 形态本来就要它（`e3d_collector` /
   `generation_root` 同一份），db 形态按 ADR-058 D8 退役，所以不算新负担；但没有模板目录的机器上
   `resolve` 会从「能读」变成「读不了」，错误文本会点名缺的文件。
5. **`db8000_session_pairs` 等 §G 项没动**：它们仍是 legacy 读法，默认仍编译、仍跑。

## 6. 归属（给提交拆分用）

里程碑 B 本轮新增 / 改动的文件（工作树里与里程碑 A 的 old-pdms-io 退出、以及 fable-5-1-24 的
「old-pdms-io 整体退出」混在同一棵树上；按 hunk 归属时以本表为准）：

- 新文件：`src/data_interface/ele_data.rs`、`src/noun_dict.rs`、`resource/dict/noun_flags.json`、
  `resource/dict/noun_caps.json`、`scripts/Check-LegacyParsePdmsDb.ps1`、本文。
- 只有 B 的 hunk：`src/uda_table.rs`、`src/pcf/bran.rs`、`src/fast_model/mesh_generate.rs`、`src/fast_model/shared.rs`、
  `src/fast_model/coverage_audit.rs`、`src/data_interface/cata_closure.rs`、`src/data_interface/extract_family.rs`、
  `src/data_interface/mdb_membership.rs`、`src/bin/refno_attr_probe.rs`。
- 与 A / 同伴混在一个文件里的 B hunk：`Cargo.toml`（`legacy_parse_pdms_db` feature + 8 个 `[[bin]]` + 1 个
  `[[test]]`）、`src/lib.rs`（`pub mod noun_dict`）、`src/data_interface/mod.rs`（`pub mod ele_data` +
  `legacy_parse_pdms_db_guard`）、`src/data_interface/db_file_header.rs`（`DbBasicInfo` 段 + 三条单测）、
  `src/data_interface/{ele_ops,e3d_collector,increment_pipeline,increment_manager,manual_update,model_impact,
  staging/parity,issue10_direct_add_node,direct_window_replay_parity}.rs`（import 换行 / 60 字节头改读法）、
  `src/versioned_db/database.rs`（只有 738 行附近的读头一处）、`src/bin/l3_suite.rs`、`src/bin/l3_suite/fixture.rs`、
  `tests/full_attributes_real.rs`。
- `Cargo.lock` 一字未动（`--locked` 全程通过）；ADR / CONTEXT 未改。

## 7. 本轮验证（2026-09-04）

- `cargo check --workspace --all-targets --locked`（默认 features）与 `--features legacy_parse_pdms_db` 都过；
  `cargo check -p aios-py --locked` 在 workspace 检查里一并过。
- `cargo test --lib --locked`：`noun_dict` 5 条、`db_file_header` 6 条、`legacy_parse_pdms_db_guard` 3 条、
  `legacy_pdms_io_guard` 2 条、`ele_ops` 全部、`mdb_membership` 9 条（纯函数）、`extract_family` 全部、
  `model_impact` 三条字典契约、`fast_model::{shared,coverage_audit}`、`uda_table`、`staging::parity`、
  `issue10_direct_add_node`、`direct_window_replay_parity`、`increment_pipeline::fold_tests::added_refno_*`
  —— 85 条通过、0 失败（live / 夹具依赖的按原样 ignored）。
- `scripts/Check-LegacyParsePdmsDb.ps1` 四条全绿；`scripts/Check-LegacyPdmsIo.ps1` 四条仍全绿。
- `cargo tree --workspace -e normal --locked -i parse_pdms_db` = `parse_pdms_db ← aios-database ← aios-py`（预期，§3 之前不变）。
- **未验证**：`mdb_membership::resolve` 与 `extract_family::parent_gap_refno_count`、l3_suite 三处的 e3d-io 读法
  没有在真库上跑（本机无 `D:\AVEVA\…\ams000`、无 `GEN_MODEL_OWNER_FIXTURE`）；它们的对照物是
  legacy 读法的同一份 `NamedAttrMap` / 活索引，逻辑逐字保留，但**数值层面属于「未验证」**，
  下次有真库时先跑 `extract_family::tests::live_extract_tree_ams7355_refno_sets`（已改成同时断言公开函数）与
  `mdb_dict_probe --project AvevaMarineSample --mdb /ALL`。
