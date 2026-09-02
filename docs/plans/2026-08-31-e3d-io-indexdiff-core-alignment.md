# e3d-io `IndexDiff` 对齐 core.dll::DB_IndexTableCompare 开发计划

> 日期：2026-08-31。状态：**已定稿**——用户拍板「三个决策点全按推荐项定」：
> §5-1 取 A（`(dbno, sesno)` 二级钉点）、§5-2 取三组宽窗口、§5-3 取 A（复用 increment_fixture）。
> 来源：外部草案 `PLAN (17).md` + 本次对 `vendor/e3d-io` 与 gen-model 消费线的逐条核实。
> 骨架沿用草案的 A–E 阶段划分；与草案的出入集中在 §0 修订记录。
>
> 权威与证据：
> - `上下文/会话-2026-08-30-core-dll索引分析.md`：`elementsChanged/Deleted/InsertedBetween` →
>   `DB_IndexTableCompare` = 主索引双根归并；**删除=键集差；变更检测全链路不读 flag（已证）**
> - `docs/evidence/2026-08-30-dabacon-index-page-layout-adjudication.md`：AoS/free_dwords/bit-12 裁决，
>   429 库 940 694 条叶值 flag 恒 1
> - `docs/plans/2026-08-30-core-dll-api-alignment.md` G 组：机制 ✅（t-327 三道门全绿）、门面 🔶
> - `docs/plans/2026-08-30-e3d-io-index-capability-gaps.md`：索引篇 P0–P4 已收官，本文是它的续篇
> - ADR-009/ADR-032：用户语义分类（OWNER=Moved、成员级 kind、UDA 旧键）的既有裁决——
>   属 E 段之后的独立分类器层，不进 `IndexDiff`

---

## 0. 对草案的修订记录（审核结论）

草案的问题诊断全部核实成立；以下六处是本稿对草案的修订或钉准，逐条可批注：

| # | 修订 | 依据 |
|---|---|---|
| R1 | 「按 RefNo 顺序输出 Modified、Inserted、Deleted」措辞澄清为：**归并流按 RefNo 单调序、逐键带类型**（现实现即如此），不是按类型分三段输出；门面各 API 按类别投影且保序 | `diff.rs::step` 是键序归并；Core 游标（opcode 266/270）同 |
| R2 | 「验证同库、跨库身份」收窄为**结构性排除**：受检入口挂在单个已开库（一条链）上、收两个 sesno，跨库比较根本无法表达，无需运行时防御 | 差分本就在一个 `PageCache` 里跑，两个文件的根页号互不可解 |
| R3 | D 段「按对应端点构造句柄」缺机制：`DbSet` 现在每库只钉**一个** sesno（`OpenDb.pinned_sesno`），Deleted 句柄在目标端点读必然 `NotFound`。列为决策点 §5-1，推荐按 `(dbno, sesno)` 开二级钉点 | `db_element.rs` L119–129、L714–733 |
| R4 | E 段钉准现状：gen-model 今天**零处**消费 `IndexDiff`，E 段是 direct 模式（ADR-053）的全新消费线。草案「保留旧版标签用于日志识别」无对象——代码里不存在任何旧算法标签；`v2-position` 的 v1 语义（按含 flag 的 `RecordLoc` 全字段比较）从未被任何窗口记录过，无迁移、无需保留 | 全仓 grep 无 `IndexDiff` 消费点、无既有算法版本标签 |
| R5 | flag-only 诊断在真实语料上**恒为零**（940 694 条 flag 全 1），语料门要把「diagnostics 恒零」本身钉成断言；触发诊断的用例只能合成 | 裁决证据 + `RecordLoc::flag` 文档 |
| R6 | 剪枝校验清单去重：key 严格递增（`Enumerator::take`）、父子 level（`read_node`）、循环+重复子页（`visited`→`Cycle`）、页数上限（`TooManyPages`）**已有**；本轮真正新增 = 继承上下界校验 + `SharedPagesOnly` 中间档 | `diff.rs` / `cursor.rs` 现码 |

顺带收益：Core 权威门（§4）零差异通过时，一并闭合两条挂账——
B9「比较器两根 == 会话页 COW 根」（索引分析·未闭合项，现为中等置信推断）与
V1「packed 文件态 >>12 vs 内存态 >>13 复核」（同处）。证据落盘时点名这两条。

---

## 1. 目标与验收口径

将 `vendor/e3d-io` 的会话索引差分对齐 `core.dll::DB_IndexTableCompare`：

- 增删依据两端 RefNo 键集差（无墓碑，不看 owner.children——已证语义，现实现已对）。
- **修改依据记录位置 `(page, offset_words)`；`flag` 不参与修改判定**（现实现比较含 flag 的
  `RecordLoc` 全字段，是本计划要修的语义缺陷；full-walk 参考实现同病）。
- 归并流按 RefNo 单调序、逐键带类型（Modified/Inserted/Deleted）；流耗尽 = Core 的 Finished。
- 剪枝实现必须与完整双树遍历结果一致（现有门，保持）。
- **Core.dll 受控窗口逐项零差异后才标记 Core-aligned**；Rust full-walk 仅是自洽基线，不是权威。

范围：底层候选差分 + `DbSet` 差分门面。属性级（`AttributesChangedBetween`）、OWNER、
成员顺序等用户语义分类是后续独立层，不进 `IndexDiff`（对应 core 侧它们也在
`DB_Element::attributesChangedBetween`/`DB_MemberCompare`，不在 IndexTableCompare 里）。

## 2. 公共 API 与类型调整

`src/index/cursor.rs`（`RecordLoc` 所在处）：

```rust
/// Core 修改判定所用的语义位置：flag 之外的两个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordPosition {
    pub page: PageId,
    pub offset_words: u32,
}

impl RecordLoc {
    pub const fn position(self) -> RecordPosition;
    // flag 字段本就 pub，不再加 raw_flag() 别名（草案有，裁掉：一个事实两个名字）。
}
```

`RecordLoc` 保留原始全字段相等语义（`entry()` 回投、测试断言都在用）；差分代码显式走
`position()` 比较，语义差异在类型上可见。

`src/index/diff.rs` 新增 Core 语义结果：

```rust
pub enum IndexCandidate {
    Modified { refno: RefNo, base: RecordPosition, target: RecordPosition },
    Inserted { refno: RefNo, target: RecordPosition },
    Deleted  { refno: RefNo, base: RecordPosition },
}

pub struct FlagOnlyDrift {
    pub refno: RefNo,
    pub position: RecordPosition,
    pub base_flag: u16,
    pub target_flag: u16,
}

pub struct DiffDiagnostics {
    /// 位置相同、仅 flag 不同的键数。语料上恒 0（R5），非零即值得人看。
    pub flag_only_drifts: u64,
    pub first_flag_only_drift: Option<FlagOnlyDrift>,
}

/// 一次完成的差分：先拿全量结果，再产生副作用（B 段契约）。
pub struct CompletedIndexDiff {
    pub changes: Vec<IndexCandidate>,
    pub tally: DiffTally,
    pub diagnostics: DiffDiagnostics,
    pub receipt: DiffReceipt,
}

/// 成功结果必须携带的对拍凭据（B 段退出门）。
pub struct DiffReceipt {
    pub base_sesno: u32,
    pub target_sesno: u32,
    pub base_root: PageId,
    pub target_root: PageId,
    pub pruning: Pruning,
    pub io: IoStats,
}
```

兼容策略：

- 保留现有 `KeyChange`、Iterator、`tally()`；根页直传构造器保留并在文档里明确为低层接口
  （会话入口是常规门），供测试与工具直连两根。
- `KeyChange` 增加 `candidate()` 投影（全字段 → 语义位置的唯一换算点）。
- 增加 `IndexDiff::finish(self) -> Result<CompletedIndexDiff, IndexTreeError>`——耗尽迭代器，
  任一步失败整体失败，无部分结果可取。
- 会话级受检入口落在 `ReadOnlyEngine`（它拥有链与页缓存）：
  `diff_sessions(base_sesno, target_sesno, pruning) -> Result<CompletedIndexDiff, _>`，
  一次解析两个端点根；单库结构（R2），校验：两个 sesno 都在本文件链上、base 在 target 之前
  （链序为准，不是数字大小——链不保证连号）、根页 extent 可解析。
- `DbSet` 增加 `elements_changed_between / elements_inserted_between /
  elements_deleted_between / has_element_changed_between`，薄封装同一份
  `CompletedIndexDiff`，不重复实现差分算法（形状见 D 段；端点机制 §5-1 已定 A）。

## 3. 实施阶段与退出门

### A. 修改语义修正

1. 引入 `RecordPosition` 与 `position()`。
2. `diff.rs::step` 的 `base.loc == target.loc` 改为 `position()` 比较。
3. 同步修改 `tests/index_diff_real.rs::changes_by_full_enumeration` 的
   `newer.loc != record.loc`——参考实现不得复制 flag 缺陷（草案 A3，核实确有此病）。
4. flag-only 变化计入 `DiffDiagnostics`，不产生候选变化。

**退出门：** flag-only（合成）、真实位置变化、相同根、增删混合测试通过；
现有 `KeyChange` 调用方源码兼容（`e3d-io` 内 grep 确认唯一比较点就在 `step`）。

### B. 原子结果与会话契约

1. 实现 `IndexCandidate`、`DiffDiagnostics`、`finish()`、`DiffReceipt`。
2. `ReadOnlyEngine::diff_sessions` 受检入口（校验清单见 §2）；拒绝倒序窗口、缺失会话、
   不可解析 extent；错误报文点名 sesno 与文件。
3. 遍历中任一页读取或结构验证失败 → 整次失败（`finish` 的 Err 分支），不返回可提交的部分结果。
4. 剪枝失败**不**自动切换完整遍历后继续提交；完整遍历只作诊断复跑（人工/测试用），
   避免「快路径悄悄换慢路径、错误被吞」。

**退出门：** 错误窗口无部分结果（含中途 `Malformed`/`Cycle` 合成用例）；
所有成功结果携带完整 `DiffReceipt`。

### C. 剪枝加固

```rust
pub enum Pruning {
    /// 完整双树归并，长期参考实现（现 Disabled，语义不变）。
    Disabled,
    /// 只跳过两根同名的相同 PageId 子树（COW 直接付账的那一种）。
    SharedPagesOnly,
    /// 再启用范围捷径（Entry×Subtree 的 below() 提前判定）。现 Enabled 的完整行为。
    Enabled,
}
```

- 新增 = `SharedPagesOnly` 中间档（把现 Enabled 的两种剪枝拆开，可二分定位回归）+
  **继承上下界校验**：子树携带从父链继承的 `(lower, upper)`；sentinel 继承父下界，
  下一个 separator 提供上界；展开子树时校验其 separator/叶键落在界内——
  防「separator 谎报 least key 导致范围捷径误判」这一现有校验网唯一漏的谎（R6）。
- 既有校验（key 严格递增、父子 level、循环/重复子页、页数上限）不重写、不搬家。
- 「读页数 == 两树页集对称差」保留为**当前语料的性能回归门**，不升格为所有合法 COW
  变换的通用正确性定义（合法写法可以整页重写同内容页，届时该门按语料重校准）。

**退出门：** 三种模式在全部语料上输出完全一致；损坏树（越界 separator 合成用例）明确报错；
对称差门在 `Enabled` 与 `SharedPagesOnly` 下各自成立（后者预期读数 ≥ 前者，等式按各自口径断言）。

### D. Core API 门面

`DbSet` 上提供 Core 风格薄封装（G 组矩阵「🔶 门面」项落地）：

- `elements_changed_between` → Modified 候选按 target 端点构造句柄。
- `elements_inserted_between` → 按 target 端点构造句柄。
- `elements_deleted_between` → 按 **base** 端点构造句柄。端点机制（§5-1 已定 A）：
  `DbSet` 池键扩为 `(dbno, sesno)`，默认钉点即 `(dbno, pinned_sesno)`、既有行为不变；
  差分门面为 base 端点开二级钉点，`DbElement` 增加可选端点会话绑定（缺省 = 池默认钉点），
  Deleted 句柄绑 base、读旧值走 base 树。
- `has_element_changed_between` → 同一份 `CompletedIndexDiff` 过滤单键，不另写点查算法。
- 输出保持 RefNo 序（归并流原序投影）。
- 后续 `AttributesChangedBetween` 通过两端记录解码实现，不修改 L2 差分分类（记账，不在本轮）。
- `*_since` 变体（Core 也有）= `between(sesno, latest)` 语法糖，本轮不做，留一行注释指路。

**退出门：** `tests/db_element_facade.rs` 覆盖会话差分、删除端点句柄可读旧值、
单键查询、错误传播（倒序窗口/缺失会话经门面报出，不吞）。

### E. gen-model 集成（direct 模式消费线，ADR-053）

1. 仅消费 `CompletedIndexDiff`，禁止边迭代边推进水位——先全量后副作用，
   与 `increment_pipeline` 「整窗口」结构一致（请求区间左端 `applied_sesno + 1`）。
2. 差分失败 → 窗口失败，`applied_sesno` 不推进（对齐 staging 尾事务语义）。
3. 候选交给独立的用户语义分类器处理属性、OWNER、primaryList（ADR-009/ADR-053 层，
   不在 e3d-io）。
4. 窗口记录携带算法标识 `index-candidate-v2-position`（首个入库标签；v1 = 修正前的
   全字段 `RecordLoc` 比较语义，从未入库，无迁移——R4）。

**退出门：** direct 增量窗口消费 e3d-io 差分后产出的净窗口，与
`scripts/e3d/increment_fixture/` 受控窗口的已固化预期一致（apply/restore 宏对 +
fixture-manifest，Core 语义预期由 ADR-009 与 `docs/specs/core3d-partial-update-test-cases.md`
钉死）；失败注入用例水位原地。

## 4. 测试与对拍

### 自动测试（合成，随 `cargo test` 常跑）

- flag 不同但位置相同：无 Modified，diagnostics 计 1、`first_flag_only_drift` 带全字段。
- page 或 offset 不同：产生 Modified。
- 相同根、空树、单侧耗尽、增删改混合。
- 根高度升降、sentinel 嵌套、首部 sibling 插入。
- 动态 key/value 宽度、unsigned RefNo 边界、`free_dwords` 残留。
- key 乱序、level 错误、页循环、不可读页、越界 separator、缺 extent。
- 属性测试生成合法小树，要求三种剪枝模式结果一致。

验证命令（`vendor/e3d-io` 下）：

```powershell
cargo test --locked --test index_diff_real -- --nocapture
cargo test --locked --test db_element_facade -- --nocapture
cargo test --locked --lib
```

### 真实语料门（`--ignored` 手动跑，报告落盘）

- 常规门：429 库每库最新两个会话（现有 `..._across_the_whole_corpus` 扩到三模式 + 全枚举四方对拍）。
- 发布门：全部相邻会话对，外加每库 oldest/latest、oldest/midpoint、midpoint/latest 三个宽窗口。
  成本量级是常规门的会话链长度倍，只做发布前手动门。
- 每对比较 full enumeration、`Disabled`、`SharedPagesOnly`、`Enabled` 四方一致。
- 报告实际库数、窗口数、三类变化数、flag drift（**期望恒 0**，非零即门红——R5）、
  剪枝页数和失败项；缺失样本不得静默计为通过。

### Core.dll 权威门

复用既有受控窗口设施（`scripts/e3d/increment_fixture/` 的 apply/restore 宏对 +
`docs/specs/core3d-partial-update-test-cases.md` 的窗口方法学），建立：
单修改、单插入、单删除、混合、no-op save、修改后恢复、插入即删除、删除重建、根高度变化。
Core 侧经 PMLNET 调 `elementsChanged/Deleted/InsertedBetween` 逐项吐出，与 Rust 侧逐条比较：

- 状态类型
- RefNo
- 旧/新 `(page, offset)`（若 Core API 面不吐位置，则该列降级为状态+RefNo 对拍，位置改由
  no-op save 与修改恢复两个用例间接钉住——现场按 API 实际能力二选一，证据里写明选了哪个）
- 输出顺序
- Finished（流耗尽行为）

发布条件为零差异，并在 `docs/evidence/` 固化命令、输入、Core 原始输出、Rust 输出和哈希；
证据同时点名闭合 B9 与 V1（§0 顺带收益）。

## 5. 决策点（已拍板 2026-08-31：全取推荐项）

1. **Deleted 句柄的端点机制（R3）→ 已定 A**：`DbSet` 池键扩为 `(dbno, sesno)`，差分门面为
   base 端点开二级钉点，Deleted 句柄绑定端点会话。理由：`AttributesChangedBetween` 后续
   反正要两端读，机制一次建对；默认池行为不变，二级钉点仅差分门面内部使用。
   （落选：B. 返回 `Vec<(RefNo, RecordPosition)>` 不给句柄——偏离 .NET 面形状，门面只算
   半落地；C. 门面直接吐 `IndexCandidate`——等于不做门面。）
2. **发布门宽窗口组合 → 已定**：每库取 oldest/latest、oldest/midpoint、midpoint/latest
   三组（全部两两组合是 O(n²)，429 库不现实）。
3. **E 段验收夹具 → 已定 A**：复用 `increment_fixture` 既有受控窗口与已固化预期——
   apply/restore 宏已配对、预期已按 Core 语义裁定过，E 段不依赖新搭建，也不把进度
   绑死在 E3D 环境可用性上。

## 6. 提交序列与默认决策

依赖顺序（每步可独立验证、独立回退）：

1. `fix(index): compare semantic record positions`（A1–A3）
2. `test(index): cover flag-only index drift`（A4 + 合成用例）
3. `feat(index): expose diff candidates and diagnostics`（B1）
4. `feat(index): finalize session diffs atomically`（B2–B4）
5. `fix(index): harden subtree pruning`（C）
6. `feat(db): expose core-compatible session diff APIs`（D）
7. `test(index): add core oracle parity gates`（§4 权威门 + 语料门扩展）
8. `feat(diff): classify user-visible element changes`（E，gen-model 侧）

默认决策（草案照单保留，两处已按核实钉准）：

- 不改磁盘格式，不重定义 `RecordLoc` 相等性。
- 多 extent：索引树内出现不可解析 extent 在证据闭合前明确报错
  （io 层 P4 自动 attach 已落地的常规路径不受影响）。
- 剪枝可独立回退到 `Disabled`；位置语义只在 Core 新证据推翻「全链路不读 flag」结论时回退。
- 保留当前工作区所有无关修改；实施仅触碰 `vendor/e3d-io` 的 index、engine/DbSet 门面、
  对应测试，及 gen-model E 段新消费线文件。
- AoS 解码结论（bit-12 裁决）为实现依据；旧报告中的 SoA 描述视为已被后续证据修正。
