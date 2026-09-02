# 新版 pdms-io（`pdmsdb_engine_v2` db1~db5）对齐 Core3D.dll 的开发计划

> 状态：**已拍板**（2026-08-29 grill 确认：Q1–Q8 全采推荐项）。
> 定稿产出：`docs/adr/ADR-055-pdms-io-v2-core3d-semantics.md`（决策）、
> `specs/034-core3d-semantics/`（spec + plan[Constitution Check] + tasks）。
> 会话上下文：`.context/会话-2026-08-29-direct直读可行性核查.md`。
> 已镜像到 `d:\work\plant-code\pdms-io-fork\docs\plans\` 与
> `d:\work\plant-code\pdms-io-fork-engine-v2\docs\plans\`（实施在 engine-v2 工作副本）。
> 实施注意：engine-v2 工作副本 HEAD（`348d187`）已领先本文撰写基准 rev（`13a17e1f`）
> 两个提交，**P4 的两条根因（`0x34` 按字解释、页大小假匹配拒绝）已落地**，
> P4 相应缩为「真库回归验证 + `open_at(sesno)`」，详见 `specs/034-core3d-semantics/plan.md`。
> **2026-08-30 审核回写**：§2 末新增「CATA 读数的时点规则」（审核挂账 W1，此前两份计划均缺）；
> T402 已在 gen-model 侧 `direct-dbelement-read-api.md` 定为 D1 硬前置、T501 定为 D2 前置门。
> 审核档案：`上下文/会话-2026-08-29-模型生成重构审核.md`。

## 0. 对象与现状（先把事实摆齐，不摆观点）

### 0.1 新版 pdms-io 是什么

- 工作副本 `d:\work\plant-code\pdms-io-fork`，分支 `dev-3.1`。
- gen-model 通过 `parse_pdms_db` 消费：
  `pdmsdb_engine_v2 = { git = "https://github.com/happyrust/pdms-io.git", rev = "13a17e1f…" }`。
- **db1~db5 = 五层引擎分层**（提交 `d53ffc99`：「基于 core.dll 逆向分析，按 db5→db4→db3→db2→db1 分层架构从零构建」）：

| 层 | 现有文件 | 职责 |
|---|---|---|
| `fortran_io/` | `direct_access` `file_ops` `retry` | 替代 Fortran 直接存取 I/O |
| `db1/` | `page_store` `page_lock` | pfno 池化页缓存 + LRU + 脏页写回 + 预读 + 页锁 |
| `db2/` | `header` `session` `db_lookup` `extract` | 头、会话链、库查找、extract |
| `db3/` | `index` `iter` `delete` | B 树 Search/Insert/Split/Delete/Iterator |
| `db4/` | `element` `attrs` `explicit_attrs` `ce` `refs` `page_layout` `record_reader` `record_writer` | 元素记录、属性分派、CE 导航栈、引用 |
| `db5/` | `mark` `refresh` `compact` | 库级操作 |
| `compare/` | `core_dll_oracle` `core_dll_runtime` `legacy_oracle` | 对拍口径（**目前只对 core.dll**） |

- 公开面（`lib.rs`）：`EngineV2` `DbHandle` `PageId` `RefNo` `RecordLoc` `SearchHit` `SessionSnapshot`
  `ElementHandle` `ElementRefs` `NavDirection` `AttrInfo/AttrType/AttrValue` `ElementBuilder`
  `ElementRecordView` `PageReadStats` `ExtractManager`。
- 仓内既有 `goals/`：`e3d31-core-io-rewrite`、`e3d31-rust-readonly-io`、`e3d31-coredll-ffi-oracle`、
  `e3d31-multi-extent`、`e3d31-attribute-parsing`、`e3d31-writeback`。

### 0.2 「以 Core3D.dll 为准」要对齐的是什么

Core3D 是 core.dll 的**消费者**（`teach/learning-records/0009`：Core3D 从 core 导入 4859 个符号、
从 sgl5NET 导入 113 个、从 libgm 导入 124 个）。它不碰页与 B 树，它定义的是**元素怎么被用**：

| Core3D 的用法 | 出处 | 新版 pdms-io 里对应谁 |
|---|---|---|
| `DB_Noun::getField(id, &out)` 位表分类（significant / primitive / negative） | 核对表 §2 R0-1/R0-2 | **db4 缺** |
| `Members(e, mode)` 三模遍历，**收集与下潜是两个独立判据** | R11 | **db4 缺** |
| `SignificantOwner(e)` 含自身、无深度上限、按位终止 | R14 | **db4 缺** |
| `DB_Element::climb(e, NOUN)` 按 noun 找祖先（XGEOM 门） | R2 | **db4 缺** |
| `DB_DB::type(e.getDB()) == 1`（DESI 门） | R1 | db2 有 `db_lookup`，**类型语义未对齐** |
| `DB_Element::isValid(e)` | R3 | `ElementHandle` 无有效性语义 |
| `DSAVE`/`DRESTO` 指针栈 + `NXTITM` 游标（不物化列表） | `teach/0009` §三 | `db4/ce.rs` **只有裸栈**，`NavDirection` 定义了没人用 |

### 0.3 本仓已有的 Core3D 资产（不要重造）

- `docs/specs/core3d-partial-update-conformance.md`：**R0–R29 逐条规则**，带地址、可重验 SQL、判定符号，
  并给出换版本后的地址重定位办法（`ida-bridge exec … WHERE name LIKE '%PartialUpdateDesiMgr%'`）。
- `docs/specs/core3d-partial-update-test-cases.md`：C 编号用例集。
- `src/data_interface/core3d_reference.rs`：**可执行参考模型**（`NounBits` / `SearchMode` /
  `significant_owner` / `members` / `is_pending` / `ancestor_deletes` / `absent_primitives` /
  `granularity_expansion`），文件头明写「不是生产路径，是给生产当契约」。
- `tests/fixtures/core-noun-granularity-e3d31.json`：**1931 noun 的三张位表**，两位分开存、`core_sha256` 钉版本。
- 警示（核对表 §1.4）：**Hex-Rays 在 `PartialUpdateDesiMgr` 上系统性出错**，凡涉及判据的函数必须读指令流；
  伪码会把 `getField(id,&out)` 的出参比较整个丢掉，后果是**丢分支**不是少参数。

---

## 1. 拷问：Q1–Q8（待拍板）

每题给选项、推荐项、理由。**未拍板前不动代码。**

### Q1 · 语义权威怎么分

| 选项 | 内容 |
|---|---|
| **A（推荐）** | **分层定权威**：db1–db3（页 / 会话 / B 树）继续以 **core.dll** 为准；db4–db5 及其上（元素分类、成员遍历、祖先攀爬、库类型门）以 **Core3D.dll** 为准 |
| B | 全栈都以 Core3D 为准 |
| C | 维持现状，只对 core.dll |

**推荐 A。** Core3D 根本不碰页和 B 树——它是 core.dll 的调用方。拿它当页层权威没有证据来源，
而 db1–db3 现有实现的对拍基准（`core_dll_oracle`）已经建立，换基准等于把已验证的东西重置。

### Q2 · noun 位表放哪、从哪来

| 选项 | 内容 |
|---|---|
| A | 位表快照硬编进 pdms-io（db4 新增 `noun_bits` 常量表） |
| **B+C（推荐）** | **接口在 pdms-io（`trait NounBitSource`），实现两路**：生产走 gen-model 已导的快照 `core-noun-granularity-e3d31.json`；对拍走 core.dll FFI 现取（`compare/core_dll_runtime.rs` 已有通道） |
| C | 只走 FFI 现取 |

**推荐 B+C。** 核对表 R0-2 已经证明 `primitive` 的第二位**跨版本会换**（2.10 是 `0xA18B8`，3.1 里一次都搜不到）。
硬编进 crate 会把版本漂移变成静默错误；而 FFI 现取是唯一能自证的口径，适合当对拍基准、不适合当生产依赖
（生产不该要求装着 E3D）。快照必须带 `core_sha256`，加载时校验不上要**报错**不要回落。

### Q3 · `Members(mode)` 放哪层、抄到什么程度

| 选项 | 内容 |
|---|---|
| **A（推荐）** | db4 实现三模遍历，**严格照 R11**：显式栈 LIFO、收集判据与下潜判据分离、不物化列表 |
| B | db4 只给 children 迭代器，三模留给调用方 |

**推荐 A。** R11 是反直觉的：mode 0 下，**非 significant 的子节点会挡住它下面的 significant 孙节点**
——这不是遍历副作用，是判据本身。放上层等于让每个调用方各写一遍、各错一遍。
mode 2（Negative）挂在死代码上（R16：`m_granularityMode` 恒 0），**实现但标记为 `#[doc(hidden)]` + 不给生产调用方**，
理由见 `core3d_reference.rs` 的原话：照着死代码建模只会引诱下一个人去实现它。

### Q4 · CE 导航栈补不补齐

| 选项 | 内容 |
|---|---|
| **A（推荐）** | 补齐：`NavDirection` 真正驱动导航，加 `climb(noun)` / `owner_chain()` / `significant_owner()`，对齐 `DSAVE`/`DRESTO` 语义 |
| B | CE 栈保持纯数据结构，导航全在上层 |

**推荐 A。** `db4/ce.rs` 现在只有 `push/pop/back/peek`，而 `NavDirection{Owner,FirstMember,LastMember,NextSibling,PrevSibling}`
枚举定义了却**没有任何实现使用**——这是半成品，不补齐它，上层就一定会绕过 CE 自己走 owner，
`DSAVE`/`DRESTO` 那套「子树遍历不物化」的收益就拿不到。

### Q5 · 「以 Core3D 为准」怎么证伪（最关键的一题）

| 选项 | 内容 |
|---|---|
| **A（推荐）** | **三层 oracle**：`legacy_oracle`（旧 pdms-io）+ `core_dll_oracle`（FFI）+ **新增 `core3d_oracle`**——把 `core3d_reference.rs` 的可执行参考模型作为期望值，用 `core3d-partial-update-test-cases.md` 的 C 编号用例驱动 |
| B | 只做真库端到端对拍 |

**推荐 A。** Core3D 的 `PartialUpdateDesiMgr` **不能直接被我们调用**：它要 view、要 `PDMS_Idlist2`、
`DrawModel` 发的是 PML 命令。端到端对拍拿不到它的中间态，测不出「members 的下潜判据错了」这种问题。
而参考模型已经把规则写成能跑的代码——按核对表的原话，规则写成代码配上用例，**下一次读错就会红，不会一路带到生产**。

### Q6 · 调度语义的归属边界

| 选项 | 内容 |
|---|---|
| **A（推荐）** | pdms-io 只做**读语义**（R0/R1/R2/R3/R9/R11/R12/R14/R26）；队列、去重、三遍消费（R6/R17–R25/R27–R29）留在 gen-model 的增量引擎 |
| B | 把 `PartialUpdateDesiMgr` 整个搬进 pdms-io |

**推荐 A。** 那套队列是绑在 E3D 视图上的：`AddIDList` 会 `PDMS_Idlist2::writeDB()`（落库副作用）、
`DrawModel` 发 `PUPDES … FORCE SUPPRESS`、`Refresh` 认的是 `NOUN_VIEW`。我们没有视图这个概念（核对表 R7/R8/R27 都判 ⚪）。
搬过来会把一个 IO 库变成半个渲染管线，而且那些规则的消费者在 gen-model
（`model_impact` / `model_update_pending` / `generation_root`）。

### Q7 · 多 extent

| 选项 | 内容 |
|---|---|
| **A（推荐，排 P2）** | db2 的 extract/extent 寻址补齐，db1 页层能跨 extent（`goals/e3d31-multi-extent` 已立项） |
| B | 本轮只显式拒绝多 extent 库 |

**推荐 A 但排在 P2。** 理由两条：`Db.ExtractNumber` 是 AVEVA API 的一等公民；
gen-model 侧现在遇到 `_0002+` 会**静默回落 legacy 全文件读**（`on_demand_db.rs::first_extra_extent`），是个悬崖。
不排 P0 的理由：本机 E3D3.1 的 **1002 个 dabacon 文件里 0 个多 extent**，当前不触发。
P0/P1 期间的处置是**显式拒绝并点名文件**，不静默退化。

### Q8 · 写侧

| 选项 | 内容 |
|---|---|
| **A（推荐）** | 本轮只读；`db4/record_writer.rs`、`db5/{mark,refresh,compact}`、`goals/e3d31-writeback` 冻结 |
| B | 读写一起推 |

**推荐 A。** 「不预先入库、直接读文件生成」这个目标全在读侧；写侧同时动会让对拍基线不稳。

---

## 2. 计划（按 Q1–Q8 全采推荐项展开；拍板结果不同则本节重排）

### P0 · `core3d_oracle`：先把尺子做出来

没有尺子，后面每一步都是「我觉得对」。

- 在 `crates/pdmsdb_engine_v2/src/compare/` 新增 `core3d_oracle.rs`。
- 把 gen-model 的 `src/data_interface/core3d_reference.rs` **提升为共享 crate**
  （建议 `crates/core3d_model/`，gen-model 与 pdms-io 都依赖它），避免两处各留一份漂移。
- 引入 `core3d-partial-update-test-cases.md` 的 C 编号用例作为数据驱动夹具。
- `trait NounBitSource { fn significant(&self, noun_hash: u32) -> bool; fn primitive(&self, noun_hash: u32) -> (bool, bool); }`
  两个实现：`SnapshotBits`（读 `core-noun-granularity-e3d31.json`，校验 `core_sha256`）、
  `CoreDllBits`（FFI 现取，复用 `core_dll_runtime.rs`）。

**验收**：两个实现对 1931 个 noun 逐个比对**全等**；快照 `core_sha256` 对不上时加载**报错**（不回落）。

### P1 · db4 元素语义层（Core3D 对齐的主体）

新增 `crates/pdmsdb_engine_v2/src/db4/core3d.rs`：

```rust
pub trait Core3dSemantics {
    fn is_valid(&self, e: RefNo) -> bool;                       // R3
    fn db_type(&self, e: RefNo) -> DbKind;                      // R1：DESI == 1
    fn climb(&self, e: RefNo, noun: u32) -> Option<RefNo>;      // R2：XGEOM 门用
    fn is_significant(&self, e: RefNo) -> bool;                 // R0-1
    fn is_primitive(&self, e: RefNo) -> bool;                   // R0-1，两位取或
    fn significant_owner(&self, e: RefNo) -> Option<RefNo>;     // R14：含自身、无深度上限
    fn members(&self, e: RefNo, mode: SearchMode) -> MembersIter<'_>;  // R11：游标，不物化
    fn exists(&self, e: RefNo) -> bool;                         // R26：递归全子树
}
```

要点（每条都对应核对表的一条规则，实现时必须回引规则号）：

- `members` 返回**迭代器不是 Vec**（对齐 `NXTITM` 游标语义，`teach/0009`）；
  内部显式栈 LIFO；**收集判据与下潜判据分开写成两个闭包**，不许合并（R11）。
- `significant_owner` **从元素自己开始判**、终止条件是 noun 位、**不设深度上限**；
  环保护用 visited 集合，不用深度计数（gen-model 侧 `MAX_ANCESTOR_DEPTH = 32` 会静默截断超深链，别照抄）。
- `is_primitive` 两位分别可查（`primitive_bits(e) -> (bool, bool)`），取或的那个函数另给
  ——「想知道是哪一位说了算永远问得出来」（核对表 R0-2 我方栏的既定口径）。
- 「字段未登记 = 该位为假」（R0-1），但要能统计命中次数，不许静默。

**验收**：`core3d_oracle` 的 C 用例全绿；`members(mode=0)` 必须复现
「非 significant 子节点挡住 significant 孙节点」这条（这是最容易实现错的一条，单独一个用例钉死）。

### P2 · CE 导航栈补齐（Q4）

- `NavDirection` 落地成真正的导航：`ce.navigate(dir)` 走 db3 索引 + db4 记录，不整树加载。
- 补 `DSAVE`/`DRESTO` 语义：`save_position()` / `restore_position()`，子树遍历用它而不是复制路径。
- `owner_chain()` 迭代器；`climb(noun)` 基于它。

**验收**：一次深度 N 的子树遍历，`PageReadStats.record_pages_read` 与元素数同阶，
不随栈深度出现二次增长；`NavDirection` 五个方向各有一个 round-trip 用例。

### P3 · db2 库类型与 extent（Q1 的 R1 + Q7）

- `db_lookup` 暴露 `DbKind`，对齐 Core3D 的 `DB_DB::type(db) == 1 → DESI`。
- extent 寻址：db2 `extract.rs` 解析 extent 链，db1 页层按 `(extent, pgno)` 定址。
- 在补齐前，打开多 extent 库**显式报错并点名文件**。

**验收**：造一个双 extent 夹具，跨 extent 的 refno 能定位并解析；
gen-model 侧 `on_demand_db.rs::first_extra_extent` 的 legacy 回退可以删掉（改为断言不再触发）。

### P4 · 页大小与会话时点的硬化

- **页大小**：现在靠 `page_size_hint: Some(0x800)` 从外面钉死。根因是引擎把文件头 `0x34`
  （按 4 字节**字**计的页大小）当字节数、把 512 排进候选，接受判据又只有「`u32_be(pgno × candidate) == 3`」一条
  ——真库 **490 个文件里 17 个中招**（`ams7329_0001` 读出 `sesno=0`，权威读 221）。
  → 把 `0x34` 按「字」正确解释，探测器只作为兜底并要求**两条独立判据同时成立**。
- **会话时点**：`latest_session()` 只能钉「打开那一刻的最新会话」。补
  `open_at(path, sesno)`，让「按 `applied_sesno` pin」（ADR-053 Q3）与「读最新」共用一条实现。

**验收**：那 17 个文件不给 hint 也读出正确 `page_size` 与 `sesno`（真库回归用例，
`paged.rs` 里已有 `real_dabacon_files_that_trap_the_detector_still_read_2048_pages` 可直接扩）。

### P5 · 上游联动（gen-model 侧）

- `parse_pdms_db::paged::PagedDbSession` 暴露 `open_at` 与 `Core3dSemantics`。
- gen-model 的 `direct-dbelement-read-api.md` 计划里的 `DbElement` 门面**改为薄封装**：
  分类/遍历/攀爬一律转调 db4 的 `Core3dSemantics`，不在 gen-model 侧再实现一遍。
- `generation_root` 的名单判定（`DEFAULT_DELIVERY_UNIT_TYPES` / `COARSE_HIERARCHY_NOUNS`）
  接上位表——但按核对表 R9 的既定结论**加层而不是换判据**（对账已量化：方向几乎全是多做，真漏只有 `AIDTEX` 一个 noun）。

**验收**：`direct_attmap_probe` 走新门面复跑 8000/7333 仍 0 真值冲突；
`tests/model_impact.rs` 与 `core-noun-granularity-e3d31.json` 的对账结论不变。

### 补 · CATA 读数的时点规则（2026-08-30 审核挂账 W1，gen-model D1 动工前置）

**背景**：CATA 库不入 `dbnum_watermark`（增量水位只登记 DESI），生成链却重度依赖 CATA
（`get_or_create_cata_context` / `query_group_by_cata_hash` / `get_cat_refno`，经 gen-model
`cata_closure` 按需解析）。direct 模式下 CATA 读数用什么时点，此前两份计划都没写，这里定死。

**规则（默认口径，逐条与现行 DB 模式同源，不发明新时点）**：

1. **CATA 一律 `TimePoint::Latest`（打开时刻的文件最新会话），不 pin 水位。**
   现行实现 `OnDemandDbSession::open` → `PagedDbSession::open(path)` 就是这个语义，
   CATA 从未有过 sesno pin；direct 模式照抄现状。
2. **同一次生成/闭包运行内复用同一打开会话**（`cata_closure` 的 per-dbnum session map 语义，
   D1 会话池沿用），运行内快照天然稳定；不同运行之间允许看到更新后的 CATA。
3. **文件身份守卫**：沿用 `locator_fingerprint`（leaf|parent 的 size+mtime 指纹）与
   `dependency_manifest_fingerprint`（任一依赖文件变化即闭包缓存失效）；打开时保留
   fail-closed 断言 `paged.snapshot().sesno == authoritative.token().target_sesno()`。
4. **对拍口径（gen-model D5 / T505）**：db 与 direct 两侧必须吃同一打开会话（同 sesno）；
   对拍窗口内 CATA 文件被替换（指纹变化）即 fail loud 重跑，不得静默续跑。
5. **升级路径**：将来若 CATA 进水位（需 ADR 增补），本规则升级为 `Pinned(applied_sesno)`，
   走 T402 `open_at` 同一条实现，门面不改。

**为什么不 pin**：CATA 是目录库，语义是「当前目录」，DB 模式读的从来就是打开时刻最新；
给 CATA 发明水位 pin 会造出一个 DB 模式不存在的第三种时点，对拍反而失去同源基准。

---

## 3. 实施原则

- **规则号回引**：每个实现 Core3D 语义的函数，文档注释必须写清它对应核对表的哪条 R 编号。
  规则改了要能反查到代码，代码改了要能反查到证据。
- **判据读指令流，不读伪码**：核对表 §1.4 已经证明 Hex-Rays 在这个类上会**丢分支**。
  新开 ida-bridge 会话时，凡涉及判据的函数一律 `SELECT address, disasm FROM instructions WHERE func_ea = …`。
- **死代码要标注不要实现**：mode 2 / `IsNegative` / `m_granularityMode ≠ 0` 分支登记备查，不进生产 API。
- **fail loud**：位表校验失败、多 extent、页大小断言不过，一律报错，不回落。
- **只读**：写侧冻结（Q8）。
- 改 Rust 跑 `cargo fmt` + `cargo check`；pdms-io 改动走升 rev 流程，不得带本地 patch 推 main。

## 4. 风险

| # | 风险 | 对策 |
|---|---|---|
| K1 | **位表跨版本漂移**（`primitive` 第二位 2.10↔3.1 不同） | 两位分开存 + `core_sha256` 钉版本 + 加载校验报错；FFI 通道作为自证口径 |
| K2 | **伪码丢分支**导致又抄错一遍（`AncestorDeletes` 终止条件已经错过一次） | 规则先落进可执行参考模型 + C 用例，实现照用例写 |
| K3 | `core3d_reference.rs` 在 gen-model 与 pdms-io 两处各留一份，漂移 | P0 提升为共享 crate，单一来源 |
| K4 | 页大小探测在真库上仍会误判 | 双判据 + 那 17 个文件的真库回归用例 |
| K5 | db4 语义层与现有 `parse_pdms_db` 的解析路径产生第二实现 | 语义层只做**分类与遍历**，记录解析仍走 `record_reader`；不复制解码逻辑 |
| K6 | 多 extent 补齐牵动 db1 页定址，回归面大 | 排 P2，先显式拒绝；改动带双 extent 夹具 |
| K7 | ida-bridge 当前不可用（`.cursor/mcp.json` 为空） | 先吃 `.ida_scratch` 与 `ida_exports/3.1/` 存量；需要新地址时再配 MCP，核对表 §1.4 已给重定位 SQL |

## 5. Non-Goals

- 写回（`e3d31-writeback`）与 db5 的 `mark/refresh/compact`。
- 把 `PartialUpdateDesiMgr` 的队列/去重/三遍消费搬进 pdms-io（Q6=A）。
- Negative 成员遍历与 `m_granularityMode ≠ 0` 分支的生产化（死代码）。
- 视图 / ID 清单 / PML 相关的一切（R7/R8/R22/R23/R27 里判 ⚪ 的部分）。

## 6. 拍板后要补的文档（2026-08-29 已全部完成）

1. [x] `docs/adr/ADR-055-pdms-io-v2-core3d-semantics.md`（决策速查表 = Q1–Q8 结论 + Considered Options）。
2. [x] `specs/034-core3d-semantics/spec.md`（要什么、怎样算成功，不写实现）。
3. [x] `specs/034-core3d-semantics/tasks.md`（每条带具体文件路径 + `[P]` 并行标记）。
4. [x] Constitution Check 与 Complexity Tracking 落在 `specs/034-core3d-semantics/plan.md`
   （本文件保留拷问全文作为决策背景，阶段执行以 specs/034 为准）。
