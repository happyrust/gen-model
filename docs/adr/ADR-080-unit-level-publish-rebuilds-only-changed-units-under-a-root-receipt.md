# ADR-080：单元级发布——只重建变了的单元，局部清单在**根回执**之下落地

状态：**草案**（2026-09-16 用户拍板「那就把它做了：立单元级发布的 ADR，把 CAS / 回执 / scoped delete 设计出来」；
本文把设计摆出来，**D2 / D3 / D7 / D9 四条要用户逐条过目才算已接受**）；**A1 / A2 / D5 / D4（闸）/ A4 / D6（门 + removals 守卫）/ A3 /
D1（真生产方 `UnitDeltaProducer`）已落地，真库探针 5 窗 5 根对拍全等；D9 开关与生产分流已接线（默认关，`4ee0000ab` 随另一路会话的
在途改动落库，未单列 changelog）；D10（只被 `no_model` 候选碰到的根空 `Partial` 只推回执）2026-09-17 落地；D7 / A5 / A6 全语料 / A7 未实施**
（同日会话 opus-5-10 按用户「按 A1 开工」「接着做 D2」执行；接班会话 fable-5-1-21 按「接着做 D5」「接着做 D4」「接着做 D6 / A3」
「接真的生产方」执行；fable-5-1-10 按用户 0917「开 ADR-078（即本文，今 ADR-080）那条：RegenRoot 只重建变了的单元，把『touched 但 0 单元』的根变便宜」做 D10）。
**★0917 改号 ADR-078 → ADR-080**（用户拍板，会话 opus-5-18）：本文 0916 立稿时误用了 ADR-078，而那个号 0915 已经给了
`ADR-078-projects-come-from-the-mdb-and-project-dirs-are-found-in-search-roots.md`（MDB 闭包，Accepted），两份同号并存了一天。
改号连带把 gen-model 的 `e3d_model_service.rs` / `model_memory_store.rs` / `model_db_adapter.rs` / `model_write_behind.rs` /
`batch_worker.rs` / `manual_update.rs` / `window_root_plan.rs` / `model_update_plan.rs` / `options.rs`、`changelog.md`、
计划 §8，以及 vendor `e3d-model/src/increment.rs` 里 **104 处**引用一起换过来（含 0916–0917 的历史 changelog 条目，不留双号并存）；
MDB 闭包那一份的 **11 处**引用逐行核过后原样留在 ADR-078（`options.rs` 的 `project_search_paths` 四处、`e3d_model_service.rs`
路由目录 D5 一处、`manual_update.rs` 候选项目一处、changelog 0915 五条）。此后代码注释里凡提「单元级 / `Partial` / `UnitDelta`」
的是 ADR-080，凡提「MDB 闭包 / 搜索根 / `included_projects`」的是 ADR-078。
**两处初稿被动手时的源码推翻，已就地改口**：D3 多列了一处 `room_resident.rs`（它校的是生成器产出，不是投影状态）；
D2 把不变量放错了层（存储本来就只删点名的，「缺席 = 删除」在上游 `generate_refs`）。理由分别记在 D3 与 D2。
日期：2026-09-16
关联：
- **实施** ADR-056 **D3 末句**（「单元级落库（`execute_plan` 直写）另立 ADR」）——D3 把执行粒度钉在根级，
  同时把这条口子留给本文。本文是 D3 的**第二个例外**，第一个是 ADR-066（刚体前移）。
- **不动** ADR-056 D1（模型失败不阻断水位）、D9（eager 只对受影响根）、N6（只有一套变更检测）、
  N7（模型面不以 `pe` 行为前置）；**改写** N3（「模型发布**根级**原子」→ 见 D4）。
- **沿用** ADR-014（分支原子替换，以 CAS 发布的形态）、ADR-054（生成时点 = 文件最新；凭证单调）、
  ADR-060（模型行 id 就是记录 id；写入只有先删后插）、ADR-065（模型活在 dbnum 任务里）、
  ADR-066（便宜路的闸 / 回退 / 落款三件套，本文逐条复用）、ADR-072（发布阶段按写标记与 sesno 回滚）、
  ADR-073（房间随同一事务收尾）。
- **证据**：`docs/evidence/2026-09-16-unit-level-rebuild-measure.md` 与同名 `-addendum.md`（S7 探针
  `probe_unit_level_rebuild_ratio`，606 窗 / 6122 regen 根 / 抽样 978 / 进统计 792 根 / 3672 单元）；
  取证文档 `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §5 表 G7 与 §7 P3；
  开发计划 `docs/plans/2026-09-15-attribute-change-class-alignment-dev-plan.md` S7 / §8 Q4。

## 背景

### core 有三档粒度，我们只有一档

core 的 `UPGRPH` 对类别 4 的变化分三种落法：顶层件整件 `REPELE`（`TOPDEL` 删段 + `ADDDES` 重生成）、
顶层件之下的单个图元 `SADREP` → `css/SEGREP`（**只重建 ref 那一个图元的段**）、目录件 / 几何集 `REPALL`。
我方按 ADR-056 D3 只有「根级整根重算」一档：一个根里但凡有一件要重算，整根所有单元全部重新生成、
全部重新 upsert。差距表 G7 记的就是这一条，性质是**多算**（正确性无缺口，只是慢）。

### S7 量出来的白算量

同一份探针、同一条语料（ams8000_0001 整链）、只换分母：

| 口径 | 根 | 单元 | 按单元加权「一字没变」 | 对 70% 阈值 |
|---|---:|---:|---:|---|
| 真·重算（稳态窗 ∧ S 端已有这根） | 488 | 2670 | **86.5%** | 过 |
| 再剔整根白算（稳健性检查） | 233 | 1440 | **75.0%** | 过 |
| 稳态窗 · 含新根 | 721 | 3366 | 68.6% | 不过 |
| 全部合计（再掺退化窗） | 792 | 3672 | 66.5% | 不过 |

判等口径 = 两端同 `GeometryId` ∧ `ParityInvariants` 全等（noun / 体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵，
旋转分量绝对 1e-6、其余相对 1e-6），与 S4 对拍门同一把尺子。**收益高度集中在少数大根**，几乎全是 `BRAN`
（380 根扛 2812 单元，占比 75.9%；对比 `EQUI` 256 根才 273 单元）——收益模型必须按单元算，按根数拍会拍高。

### 已经有的那一半，和真正缺的那一半

单元级的**判定与生成**其实已经躺在仓里，只是没有生产调用点：

| 层 | 现状 | 粒度 |
|---|---|---|
| 选件 | e3d-model `plan_update(S→T)` → `UpdatePlan { regenerate, remove, regenerate_derived, derived_stale, full_rebuild }` | **单元级**（已有） |
| 生成 | e3d-model `execute_plan(target, plan) -> IncrementOutcome { upserts, removals, report }` | **单元级**（已有，仅作可执行规格） |
| 投影入口 | `e3d_model_service::apply_geometry_delta(dbnum, sesno, upserts, removals, …)` | **单元级**（已有，唯一生产外调用点是已死的 `apply_window`） |
| 存储 | `ProjectionCommit { tube_branch_resets, removals: Vec<record_id>, upserts, receipt }`；durable 侧 ADR-060 按记录 id 先删后插 | **记录级**（已有） |
| 房间 / 空间 overlay | `ScopeState::room_changes: BTreeMap<dbnum, BTreeMap<record, GeometryChange>>` + `flush_room_spatial` | **记录级**（已有） |
| **回执** | `RootReceipt { source_sesno, model_target, manifest_hash, geometry_count, published_via }` | **根级** ← 缺口 |
| **发布原子边界** | 一次 `generate_roots` = 一个 cohort，整批 `model_write_behind::publish` | **根级 cohort** ← 缺口 |
| **清单语义** | 「upserts 就是这根的全部；`prior \ generated` 就是要删的」，`IncompleteRoot::Discard` 专为它设的护栏 | **根级** ← 缺口（最危险的一条） |

所以本 ADR 要定的不是「怎么判断哪个图元变了」——那一半现成。要定的是**局部清单怎么在一个根级回执之下
安全落地**：三处判据（清单、回执、对账）今天都默认「一次提交 = 这根的全部」，直接把局部产出喂进去，
后果是**静默删件**，不是报错。

### 为什么不能照搬 ADR-066

刚体前移看起来已经是「不整根重算的发布」，但它**仍然 upsert 这根的全部记录**（`republish_records` 把
`records_of_root` 全量左乘 Δ 后整批写回）。它省的是网格生成，不是记录写入。它必须这么做，原因在
`model_db_adapter::persist_root_impl`：

```rust
records.iter().all(|record| {
    record.source_sesno == receipt.source_sesno
        && record.ancestors.contains(&packed_root)
}),
"root {root} projection contains records outside its receipt scope"
```

**记录的 `source_sesno` 必须与回执逐字相等**。局部提交之后，没动的那些记录还停在旧会话上，回执却前移到 T ——
落盘那一刻就撞这条断言。这是单元级发布真正的第一道墙，D3 专门处理它。

## 决策

### D1　单元 = `GeometryId`；判定不新造一套

一个「单元」就是投影里的一条记录，键是 `GeometryId`：`Element { refno }`（挂在自己 refno 上的实体）
与 `ImpliedTube { container_refno, route_ordinal, from_refno, to_refno }`（隐式管身）两种，与
`ProjectedGeometry.geometry_id`、`manifest_hash` 的去重键、S7 探针的 join 键完全同一个。

谁要重建**直接吃 `plan_update` 的产出**（`regenerate` / `remove` / `regenerate_derived` / `derived_stale`），
不引入第二套变更检测（ADR-056 N6）。选根一个字不改：`plan_window_roots` 照旧产根，本 ADR 只改「拿到一根之后
在根内做什么」。

★0916 落地（会话 fable-5-1-21，用户「接真的生产方：把 plan_update + execute_plan + bake + project_geometry_delta
接成 produce，先用 #[ignore] 真库探针跑一根」）：

- **真生产方 `e3d_model_service::UnitDeltaProducer`**：`open(service, dbnum, S, T)` 一窗算一次——`collect_window` →
  两端 `generation_input`（与整根重算共用缓存的 `DbSet` / 索引）→ `plan_update`；**计划口径与选根那一步同一份**
  （元数据门开着走 `plan_update_with(change_metadata_tier)`，关着走裸 `plan_update`），否则选出来要局部发布的根与
  切给它的子计划对不上。`UpdatePlan` 会被 `execute_plan` 整个吃掉、又不 `Clone`，所以拆成四个集合放着，
  `produce(root, old_receipt)` 逐根切子计划：`regenerate` / 活着的派生容器按 **T 端**属主表的祖先链归根，
  `remove` / 退休的容器按 **S 端**属主表归根，`derived_stale` 跟着它的容器；祖先链读挂按错误上抛（静默漏掉
  就是陈旧几何）。然后 `execute_plan(target, 子计划)` → `bake_elements` → `project_geometry_delta(…, None, owners@T)`
  → `UnitDelta`。改挂的单元同一个 record_id：新根名下 upsert 时存储先按 id 删旧记录再插，旧根名下不留鬼影。
- **`execute_plan` 从此有了 gen-model 侧的第一个调用者**（此前只有 e3d-model 自己的集成测试）；生产开关（D9）
  未立，今天调它的只有真库探针。
- **入口**：`E3dModelService::republish_roots_by_unit_delta_from_window(dbnum, S, T, units)` 把生产方接进执行体；
  每个 unit 的 `source_start_sesno` 必须就是这一窗的 S——闸对回执的基线与切计划的基线要是同一个。
- **真库探针 `probe_unit_publish_one_root_against_regen`**（`#[ignore]`，`from_current` 的 MDB + E3D 模板目录）：
  一窗里挑一根 regen 根，S 端整根重算灌进本地投影当基线（回执停在 S、目标是 S 的），走生产方 + 执行体，
  再与 T 端整根重算比**整根 `manifest_hash`**，不等时按 `geometry_id` 逐条报 DIFF / EXTRA / MISSING。
  ams8000_0001 五窗五根**全部逐字相同**：609→610 与 614→615（S4 造的 NBOX 挪位窗，EQUI `24384/24776`，2 件）、
  45→46（改挂窗，新根 EQUI `24384/25803`，S 端 0 件 → 1 件）、195→196（级联窗，EQUI `24384/26186`，1 件）、
  **249→250（管路窗，BRAN `24384/26229`，16 件：重建 1 单元 + 1 派生容器，delta 5 upsert / 3 removal / 1 branch reset，
  管段门放行）**。这一根单元路 0.02 s 对 regen 0.07 s；根都很小，A7 的净收益要在大根上另量。

### D2　局部清单必须**显式**，不能靠调用方自觉

★0916 动手时的修正：**存储本来就不拿缺席当删除**。`ScopeState::apply_removals` 只删
`ProjectionCommit::removals` 与 `tube_branch_resets` 点名的那些。所以「未提及 = 删除」这条
**不在存储里，在上游**——`generate_refs` 拿 `上一版有的 \ 这一版生成的` 算 `removals`，那条推导
只有在 `upserts` 确实是这根全集时才成立。初稿把 `CommitScope` 设计成一个「存储照它决定删不删」的
开关，是把不变量放错了地方。

落地形态：

```rust
pub enum CommitScope {
    /// `upserts` 是这根的全集：缺席 = 删除，上游可以做差。今天所有生产提交都是这一档。
    #[default]
    WholeRoot,
    /// 只点名了这几件：缺席什么也不证明，要删什么必须显式写进 `removals`。
    Partial,
}
```

- **枚举不带 `root`**：`ProjectionCommit` 的 `receipt` 已经带着根，再放一份就有两个真相。
- **默认 `WholeRoot`**：它就是今天的语义，也让磁盘上那些没有这一格的旧 `model_artifact`
  反序列化后落在它被写下时的语义上（`#[serde(default)]` + 一条测试钉住）。
  初稿的实施约束 2 写「不给默认值」，是照「存储按它删」那个错设计定的；现在这一格不决定删除，
  给默认值反而是对的。
- **真正的不变量放在推导发生的地方**：做差抽成 `e3d_model_service::whole_root_removals`，
  函数文档写明「只有 `generated` 是全集时这句才成立」，并用一条 `include_str!` 守卫钉住
  **生产代码里只许有这一处**拿缺席推删除。

`Partial` 下另外两条不变：
1. `IncompleteRoot::Discard`（生成出残缺清单时保留上一版）在 `Partial` 下失去意义——它防的正是
   「把部分清单当全清单发」。换成 D8 的硬回退顶上。
2. 一根被删空（最后一件也删了）**不走 `Partial`**，退回根级 `DeleteCleanup`。

### D3　`source_sesno` 改成「这条记录最后一次真正生成的会话」；对账断言从 `==` 放宽到 `<=`

这是本 ADR 唯一一处**改既有语义**的地方，也是绕不过去的一处。

- `ProjectedGeometry.source_sesno` 今天事实上承担两件事：① 这条记录是哪一版几何；② 这根整体推进到哪了。
  单元级发布把这两件事拆开：**②归回执**（`RootReceipt.source_sesno`），**①留在记录上**。
- 相应地，**两处**对账断言 `record.source_sesno == receipt.source_sesno`
  （`RootShare::load` 与 `persist_root_impl`，`model_db_adapter.rs`）改成 `<=`，并保留
  `record.ancestors.contains(&packed_root)` 那一半不变（越界记录仍然必须报错）。
  **比回执新的记录仍是错**：那意味着有人把几何投进来却没盖章，这根的全集就不是回执说的那一份。
- **`room_resident.rs` 那处 `row.source_sesno == source.target_sesno()` 不动**（★0916 动手时核出来的，
  初稿把它一并列进来是错的）：它校的是 `generate_frozen_roots` 的**产出**，而产出永远来自一次新鲜生成、
  每条都盖着 `batch.source_sesno`，压根不经过投影里那些没动的旧记录。放宽它只会白削掉一道还成立的检查。
  同理 `room_model.rs` 读的是活投影、本来就是 `<=`，也不动。
- `ModelMemoryStore` 里 `published_sesno` 取的是本库记录 `source_sesno` 的**最大值**——语义不变，但它不再等于
  「每条记录都在这个会话」。需要「每条都在」的读者只有上面那两处。

**取舍**：另一条路是照 ADR-066 那样「局部生成、整根 upsert」——不改任何语义，但每次提交仍要写整根的记录，
S7 那 86.5% 只省在生成侧、不省在写入侧，而且大根（`BRAN` 2812 单元）的写入正是热点。
**推荐走放宽 `<=`**；`==` 那条保守路作为 Q2 的回退位留着。

### D4　CAS 与原子边界：不新增锁，把 cohort 的定义从「一次 `generate_roots`」改成「一次发布调用」

- 串行化仍是 `db_generation_lock(dbnum)` + 进入时 `ensure_not_older_than_persisted(dbnum, sesno)`，
  与 regen / transform 两条路**同一把锁**。ADR-056 N3 的「根级原子」改读：
  **一次 `model_write_behind::publish(commits)` 是一个原子边界**，一批里可以混 `WholeRoot` 与 `Partial`、
  可以跨多根（改挂两端根、一个 PIPE 下几条 BRAN 仍然要一起落地）。
- 幂等闸照搬 ADR-066 的 `republish_gate`，判据换成回执 vs 计划基线：
  - 回执已在 T → 窗口重放，**跳过**，按已完成计（`replayed`）；
  - 回执不在规划基线 S 上 → 计划是对着别的状态算的，**回退整根 regen**（`FallbackBaselineMismatch`）。
  这是唯一防「局部提交叠加在半截状态上」的东西，**没有它就没有本 ADR**。
- `RootPublicationAck::matches` 的四元组（`source_sesno` / `geometry_count` / `manifest_hash` / `model_target`）
  **一个字不改**——D5 保证这四项在局部提交后仍是整根口径。

★0916 落地（会话 fable-5-1-21，用户「接着做 D4：把 `republish_gate` 搬过来当 `Partial` 的幂等闸，闸先于读
`records_of_root`，护栏测试钉住 replayed / 回退两端」）：

- **执行体骨架已立**：`E3dModelService::republish_roots_by_unit_delta(dbnum, T, units, produce)`——与刚体路同一把
  `db_generation_lock(dbnum)`、同一道 `ensure_not_older_than_persisted`（不新增锁），本体拆成无锁的
  `republish_roots_by_unit_delta_on(store, mesh_dir, T, units, produce, publish)`，好在本地存储上把顺序钉住。
  每根的顺序钉死为 **完整性前置 → 闸 → 产出 → `Partial` 提交**（实施约束 5）。
- **闸就是 ADR-066 那一道**：`republish_gate(old_receipt.source_sesno, unit.source_start_sesno, T)`，同一个函数、
  同一个 `RepublishGate` 枚举，不另造判据。`SkipAlreadyAtTarget` → `replayed`（跳过、按已完成计）；
  `FallbackBaselineMismatch` → `fallbacks`，原因写明「回执在 X、计划对着基线 S 算、目标 T」。两种否决都在
  **叫生产方之前**：一件几何都不求、一个字都不发。
- **执行体自己不读任何记录、不自己算章**：删什么由产出（`UnitDelta::removals`）点名，整根的章由存储在写锁里盖
  （D5），所以闸后面没有第二个能读到旧状态的窗口。这一条用 `include_str!` 守卫钉住：执行体正文里
  `records_of_root(` / `geometry_ids_of_root(` / `root_snapshot(` / `manifest_hash(` 一个都不许出现。
- **`produce` 是注入的**：`FnMut(&UnitPublishUnit, RefNo, &RootReceipt) -> Result<UnitDelta>`。接上 `execute_plan`
  之前由调用方给（测试里就是 `project_geometry_delta` 出来的局部产出），接上之后就是它；报错即回退并记原因（D8）。
  `UnitDelta` 只有 `tube_branch_resets` / `removals` / `upserts` / `model_target` 四格——`scope` 与回执由执行体盖，
  生产方替不了它决定这一笔是不是局部。
- **仍没有生产调用点**（D9 开关未立）；实施约束 4（管段完整性门）与 7（transform 优先于 unit）还没接进执行体。

### D5　回执仍是**根级**，`manifest_hash` 提交后从投影重算

不拆单元级回执。理由两条：
1. 回执的两个用处——「目标没变就不必重算」（`cached_root_target_matches`）与自检
   （`root_projection_complete`：记录数 == `geometry_count` ∧ mesh 文件都在）——都要整根口径；
2. `model_target`（design pin + catalogue pins + 模板摘要 + 生成器指纹 + 细分档）是整根共用的，
   拆到单元上就是复制 N 份。

局部提交后的回执这样盖：

```
manifest_hash, geometry_count = manifest_hash(store.records_of_root(Current, root) 应用本次 delta 之后)
published_via = "unit"          // 第三档，与 "regen" / "transform" 并列，只作观测，不进任何判据
source_sesno  = target_sesno    // 整根前移
model_target  = 本轮重算出来的（不沿用旧的）
```

与 ADR-066 刚体路同一做法（它也是 `manifest_hash(&moved)` 重算）。

**这是本 ADR 最大的一处「没省干净」**：digest 是 O(根内记录数) 的序列化 + SHA256，不是 O(改动数)。
在 2812 单元的大 `BRAN` 上，它和省下来的生成时间是同一个量级还是低一个量级，**没量过**——见 D9 验收门 ③ 与 Q1。

★0916 落地（会话 fable-5-1-21，用户「把『应用 delta + 重算 `manifest_hash` + 盖回执』做成写锁内的一个原子操作」）：

- **盖章的位置在存储的写锁里。** `ScopeState::apply_upserts_and_receipt` 落完 upserts 之后，`scope == Partial` 且带
  回执的提交由 `ScopeState::stamp_partial_receipt` 按 `record_ids_under(root)` **此刻**的全部记录重算
  `manifest_hash` / `geometry_count`，再入 `receipts`——与 `apply_removals`（含 `tube_branch_resets`）、upserts
  在同一次 `write()` 之内，中间插不进任何别的写入者。记录集合与 `root_projection_complete` 同一把尺子，
  盖出来的 `geometry_count` 正好是它随后要核的那个数。
- **为什么不能在锁外算。** 调用方拿 `records_of_root`（读锁）→ 叠 delta → 算章 → `publish`（写锁），读与写之间
  另一批提交、`evict_missing_root`、`clear_dbnum` 都插得进来，章对的是一个已经不存在的版本，而且没有任何一处报错。
- **配方仍是一份。** `e3d_model_service::manifest_hash_of`——`manifest_hash` 的借用形、同一个函数体，
  只是不要求记录连成一片（锁里的记录散在 `BTreeMap` 里，整根克隆一遍再喂切片就是白扛一次 O(n) 复制）。
  regen / transform / unit 三条路对同一批记录算出同一个章，D9 ① 对拍门比的正是它。
- **调用方的分工。** `Partial` 回执里 `manifest_hash` / `geometry_count` 是占位、会被覆盖；
  `source_sesno`（= T）、`model_target`（本轮重算的）、`published_via`（`"unit"`）照调用方填的。
- **返回盖完的那份。** `ModelMemoryStore::commit` / `try_commit_current` 改为返回 `Vec<(RefNo, RootReceipt)>`
  （实际入库的回执，按提交序）。`model_write_behind::publish_commits` 原先在**提交前**从 `commits` 抄回执去登记
  durable 身份——对 `Partial` 就是拿占位值登记，ACK 永远对不上、这根永远 pending，且不报错；现改用返回值。
  这也是 D4 那句「`RootPublicationAck::matches` 四元组一个字不改」能成立的前提。
- **`WholeRoot` 一字不动**（实施约束 9）：回执照调用方写的入库。`cached_root_commit` 与「目标变了、清单没变」
  两条路的 `upserts` 是空的，替它们重算就是把一根完整的投影盖成空章。
- **上面那条 O(根内记录数) 的账没变**：省掉的只是克隆，序列化 + SHA256 仍按整根算，Q1 照旧挂着。

### D6　scoped delete：删的粒度早就是 record_id，本条定的是「谁有资格进 `removals`」

- `Partial` 下 `removals` 的**唯一**合法来源：`UpdatePlan.remove` 映射出的 `GeometryId`，
  加派生容器旧槽位与新槽位做差（`execute_plan` 今天就是这么算的）。
- **`tube_branch_resets` 与 `Partial` 冲突**，要单独立规矩：它是「先清光这条 BRAN 的全部隐式管身再插」的
  **整段重写**语义。一次 `Partial` 若触到某条 BRAN 的管身，则该 BRAN 的管身按整段处理，
  **计划必须把这条 BRAN 的全部管段都放进 `upserts`**；做不到就整根回退 regen。
  漏一段 = 静默丢管，且两边都不报错。
- 历史投影域（`ProjectionScope::Historical`）不走 `Partial`，照旧整根。

★0916 落地（会话 fable-5-1-21，用户「接着做 D6 / A3：在执行体的产出与提交之间加管段完整性门」）：

- **门在哪**：`e3d_model_service::tube_segment_completeness(&UnitDelta) -> Result<(), String>`，执行体
  `republish_roots_by_unit_delta_on` 里夹在 `produce` 与 `publish` 之间；不成就 `fallbacks` 一条
  「tube segment completeness: …; falling back to regen」，**不发**（D8）。
- **账从哪来**：`UnitDelta` 新加一格 `tube_segments: BTreeMap<(word0, word1), usize>`——本次触到的每条 BRAN 在 T 端
  **应有**的管段数，生产方按 `route_tubes_of` **另记一账**、不从 `upserts` 里数回来（数回来就是自证）。
  实施约束 4 那句「== `execute_plan` 对 C 算出的槽位数」按源码改口为「== `execute_plan` 对 C **真建成**的管段数」：
  `execute_plan` 的 `kept` 只收真建成的管身，退化槽位不出几何，拿 `route_slot_keys` 的全部槽位数去核会误拦。
- **核三件事**：① `upserts` 里带了 C 的段，C 就必须在 `tube_branch_resets` 里（不重置，旧段留在新段旁边成鬼影）；
  ② 触到的每条 C（重置的 ∪ 带了段的）都要在 `tube_segments` 里有账；③ 带上的段数 == 账上的段数（少了是漏段、
  多了是账错，都不发）。只数段数，不核 `route_ordinal` 连不连续——序号有洞是退化槽位的正常形状。
  账上 0 段 + 只重置不带段 = 容器在 T 端退休的合法整段清空。
- ~~**`removals` 的合法来源那半边（本节第一条）还没在执行体里核**~~ ★0916 接生产方时落地：
  `removals_trace_back_to_the_plan(&RootPlanSets, &outcome.removals)`——产出里每一条 removal 都要回溯到子计划的三处
  之一：`remove` 的单元、`regenerate` 里画不出 / 画失败而被擦掉的单元（`execute_plan` 对 `Skipped` / `Err` 都发
  Remove，「先擦后画，画不出保持擦掉」）、派生容器 base 端的旧管段键 `derived_stale`。回溯不到就 `bail!`，
  执行体回退（D8）。本节第一条的措辞按源码补了中间那一处：原写「唯一合法来源 = `remove` + 派生差集」漏了
  画不出的重建单元。
- **管段账的来源**（`UnitDelta::tube_segments`）：e3d-model `IncrementReport` 新加 `tube_segments: BTreeMap<String, usize>`
  ——`execute_plan` 在 `route_tubes_of` 返回处逐容器记「这一轮真建成几段」（退休 / 类型已变的容器记 0，读挂的不记），
  与 `upserts.push` 分开；`accounts_for` 多一条恒等式：各值之和 == `tube_upserts`、条数 == 建成 + 退休容器数。
  e3d-model 真库门 `an_increment_says_what_two_full_generations_say` 六窗跑过（22 min），账全平。

### D7　durable 落盘：第一版**仍按整根刷**，不跟着 `Partial` 切

`persist_root_impl` 今天从 `records_of_root(root)` 取全量记录落盘。本 ADR **不改它**：

- ACK 的身份靠整根 `manifest_hash`（D5），整根刷天然对得上；
- write-behind 是后台的，不在前台时延上；
- 真正的大头是几何生成（CPU + 网格烘焙），S7 量的也是这一块。

代价：一次只改一件的窗口，落盘那一段仍写整根的行。要不要让 write-behind 跟踪脏记录 id、只刷改动那几条，
**先量再说**（Q3）。**这一条是有意的半程**，不是遗漏。

### D8　任何一件失败 → **整根回退 regen**，同一个 Task 内

不允许发布「部分成功的局部清单」。`execute_plan` 报告里出现 `regen_failed`、`full_rebuild` 退化、
管段凑不齐（D6）、闸判 `FallbackBaselineMismatch`（D4）——任一条命中就把这根丢回整根重算，
与 ADR-066 D6 同款：**回退永远正确，只是不省**。回退要记原因，进 `UnitTask` 回执的 `published_via`
观测列与窗口报告，不静默。

★0916 落地：生产方 `UnitDeltaProducer::produce` 的 `bail!` 就是回退，执行体接住原因写进 `fallbacks`。五种：
① 窗计划退化成整库重建（`full_rebuild`，Q4——空产出不是「没变」）；② 计划期有读挂（`unresolved` /
`derived_stale_unreadable`）——分类不可信，整窗一根都不许走局部；③ 旧回执没带目标、或不可变目标在**设计窗之外**
变了（`same_target_outside_the_design_window`：项目 / 设计文件身份 / 目录依赖含会话 / 模板 / 生成器指纹 / 细分档，
只有 `design.session` / `design.digest` 前移是局部该看到的）——core 对目录 / 几何集变化走 `REPALL`，一窗 DESI
差分覆盖不到；④ `execute_plan` 报 `regen_failed` / `derived_failed`；⑤ removal 回溯不到子计划（D6）。
执行体自己再加两种：管段门不过（D6）、`publish` 之前的闸 / 完整性前置。

### D9　开关 `model_unit_level_publish`，**默认关**；开门要过对拍门

与 `model_transform_cheap_path` 同一纪律（ADR-066 D7）。开门槛三条，缺一不可：

1. **两态对拍全绿**：同一窗分别走 unit 路与 regen 路，两端投影记录按 `GeometryId` 配对、
   `ParityInvariants` 全等，且**整根 `manifest_hash` 逐字相同**（沿用 S4 / S7 的尺子与容差）；
2. **语料覆盖**：不止 ams8000 一条链——S7 的抽样是 16%、单链，本门要全语料
   （`AIOS_POSREF_DIRS` 五个目录，与 S6 全语料门同口径）；
3. **净收益为正**：一窗端到端时延（含 D5 的 digest 重算与 D7 的整根落盘）不慢于 regen 路。
   ③ 不过就是 D5 / D7 的代价吃掉了收益——那时回头做 Q1 / Q3，不是硬开。

### D10　「touched 但 0 单元」的根：只被 `no_model` 候选碰到的 regen 根在单元路上发一份**空** `Partial`，只推回执

★0917 落地（会话 fable-5-1-10；用户「开 ADR-078（即本文，今 ADR-080）那条：RegenRoot 只重建变了的单元，把『touched 但 0 单元』的根变便宜」，
接计划 §8 Q6 ★0917 第二手拍板 A「种子不动，收益等 ADR-080」，决策库 `d-365`）。

**它接的账**：Q6 第一手（元数据门第三档 `CacheStamp`，e3d-model `5d92b97` + gen-model `bf405d5ad`）把只跳 `CACHID` 的候选
从上卷里摘掉了——那些根名下**要重建的单元归零**；但两票门三档的候选**照旧做闭包种子**（不变量「根 r 不在闭包里 ⇒ r 子树里
什么都没动」靠它守着，d-365 拍了不动），所以根仍 `touched` → `RegenRoot`，整根重算一遍、几何与 S 端逐字相同、只有回执前移。
语料里这样的根 9172 / 263612（3.5%），其中 8486 是零单元根（HANG / SUPPO / SCTN 无 builder）；整库缓存重建的扫写窗
（aps7344 108→109：1259 戳 / 1261 候选）整窗都是它。D9 之前的执行体对这种根也不省：生产方切出的子计划为空，按「空清单
不是没变」回退整根 regen——对这一类根那一退是白算。

**判据（规划器，`window_root_plan::classify_no_model_only_roots`）**：`plan.regen` 里的根 r 点名为 `no_model_only`，五条缺一不可——
① 窗口没退化；② r ∈ `settled`（S 端已发布且安定，ADR-079）；③ r 是闭包**直接**命中的，且**不在**任何一条级联的目标集里
（C2 `far_end_roots` / C8 `referrer_roots` / P4 `referrer_roots`——按各自的目标集取，不用 `touched \ direct`：一根可以既被直接命中
又是级联对端）；④ 归到 r 名下的账本候选至少一个；⑤ 归到 r 名下的候选**全部**是账本去向 `no_model`（`CandidateKind::NoModel`）。
归因只看两个方向（候选就是根、候选的 T 端链或 S 端链经过根），**不看根之上的候选**——与刚体路 `classify_one` 差这一条，故意的：
祖先闭包按构造不因根之上的种子把根收进来；根之上的改动若真波及根下几何，e3d-model 的级联会把根下单元放进动作集、子计划就不空；
反过来把 `WORL` 的 `UPDT` 一类每窗必跳的祖先候选算进来，这条路在真库上一次都打不着。候选表与刚体路**共用**
（`collect_rigid_motion_sources`，任一开关开着就收；刚体开关关着时不算 `Δ`、只动摆放的候选记 `Other`——对 D10 它本就不是 `no_model`）。

**随计划走**：`ModelUpdatePlan::unit_publish_no_model_only_roots`（`a/b` 名单，`unit_publish_base_sesno` 为 `None` 时恒空；
`#[serde(default)]`）→ `UnitPublishHint::no_model_only` → `UnitPublishUnit::no_model_only`。`regen` 的子集加提示，`work_items` / `units`
一字不改；warnings 多一行 `unit-level publish — no_model_only roots=N of regen=M`。

**生产方**（`UnitDeltaProducer::produce(root, old_receipt, no_model_only)`）：子计划为空时，不带提示照旧 `bail!` 回退（原因里加一句
「the planner did not mark it no_model_only」）；带提示则再对账本复核一遍 `root_touched_only_by_no_model_candidates`（`WindowPlanSets`
从此留着 `ledger.dispositions()`；规划器与生产方是两次独立算的计划，实施约束 12——两边都得点头），成立就回一份**空** `UnitDelta`
（只带本轮 `model_target`）。**设计窗之外的目标核在它之前**（实施约束 11）：目录依赖会话 / 模板 / 生成器指纹 / 细分档任何一格变了，
这根照旧回退 regen——Q6 量表没实现的「#8 目录侧」那一列，这里天然有。

**执行体**（`republish_roots_by_unit_delta_on`）：空 delta（不插、不删、不重写管段）**只许来自带 `no_model_only` 提示的 unit**，
别的根回出空 delta 是生产方的 bug，回退不发（守卫位置：管段门之后、提交之前，`include_str!` 钉住）。放行的空 delta 照常以
`Partial` 提交：存储在写锁里按原记录重算章（逐字不变）、回执推到 T、`published_via = "unit"`；记进 `UnitPublishReport::receipt_only`
（`completed` 的子集，只作观测），`ModelUnitResult.message` 带一句「touched only by no_model candidates … receipt advanced … (D10)」。
D2 那条「可能删空」判据顺带改口：`partial_delta_may_empty_the_root` 只对**真删了东西**的空 upsert 清单判真——什么都不删的空清单
留下的记录就是原来那些，**零记录的根照旧零记录**，不是删空（改口之前 `0 removals ≥ 0 records` 恒真，8486 个零单元根一个都走不成）。

**代价**（这类根一根的账）：`plan_update` 一窗一份（摊）、闸、`model_target` 核、D5 的 O(根内记录数) digest、D7 的整根落盘
（零记录的根 = 只写回执）。省下的是整根的 DFS + 几何生成 + 整根 upsert。

**前置（用户 0917 拍板「级联关着 ⇒ D10 不分桶」，安全优先）**：d-365 分析里 (e) 类「被引用的东西变了、引用者只被盖戳」分两半——
**目录半边**（目录件 / SPRF 变了）由生产方的 `model_target` 核封住；**设计半边**（跨根设计引用 `JOIS` / `JOIE` / `CREF` / `CFRA` /
`STAREF` / `ENDREF` / `POSREF` 的引用者只被盖戳）今天靠戳做种子整根 regen「碰巧」收住，D10 之后只有级联的目标集能把引用者的根挡在桶外
（条件 ③）。所以**三条级联 `model_structural_connection_cascade` / `model_cable_endpoint_cascade` / `model_design_reference_cascade`
都开着，规划器才分桶**（`WindowRootSources::no_model_only_cascades_off`；有关着的整窗一根都不点名，warnings 报一句为什么，候选表也不为
单元级收——默认配置下 D10 不生效，空子计划照旧回退整根 regen）。源码钉 `d10_refuses_to_classify_unless_all_three_cascades_are_on`
把三处（前置看齐三条 `enabled`、分桶吃它、`collect` 按三个开关收表）钉住。**级联开着时的账**：aps250209 16→17 从 373 / 373 降到
**350 / 373**、aps7343 6→7 从 100 / 100 降到 **26 / 100**——差掉的正是级联判成引用者对端的根（GENSEC / SCTN 的 `JOIS` / `JOIE`），
它们照旧整根 regen；`2013286753/40174`（50 件 HANDRA / GENSEC / STRFLT）就在其中。

## 后果

- ADR-056 **N3 改写**为「一次发布调用是一个原子边界」（D4）；D3 从「根级整根重算」降为「默认档」，
  例外现在有两个：ADR-066（刚体前移）与本文（单元级）。★0917 已落（会话 fable-5-1-18）：ADR-056 状态行补「被 ADR-080 修订」、
  N3 改写（原文留在括号里）、D3 行末追记指向本文、关联段 ADR-014 那句随 N3 改口。
- `e3d-model` 的 `execute_plan` **从「可执行规格」转正为生产执行体**；`model_refresh.rs`
  「`execute_plan` 半边（单元级落库，D3 已否）届时摘除」那句注释作废，改引本 ADR。★0917 已落：`apply_window` 的文档注释改口，
  `#[allow(dead_code)]` 与「在那之前不删」不动。
- `CONTEXT.md`「模型影响」与「模型发布」两处补词条：`单元级发布 (Unit-Level Publish)`、
  `局部清单 (Partial Manifest)`，并点明与 core `SEGREP` 的对应。★0917 已落：`单元级发布` 紧跟「模型影响」之后（core 对应物
  `css/SEGREP` 而非 `add/REPELE`），`局部清单` 紧跟「分支原子替换」之后（那条里引 N3 的一句随 N3 改口）。
- `RootReceipt.published_via` 出现第三个取值 `"unit"`；面板与 `/health` 的分档要跟着加一列。（★0917 未做：今天 regen / transform
  也没有分档列，`handlers.rs` 只原样透出 `published_via`；等有人要看再加，不算本 ADR 落地的前置。）
- S7 的结论段与计划 §8 **Q4 收口**：按「真·重算」口径 86.5% > 70%，故立本 ADR。
  引用那个数**必须带口径**，只写一个数必被后人读反。

## 实施约束（逐条核，不得静默绕开）

1. **先改语义，后接执行体**。D3 的三处 `==` → `<=` 与它们的护栏测试先落地、先绿，再谈接
   `execute_plan`。顺序反过来的话，第一次 `Partial` 提交会撞落盘断言，而现场只看得见一句
   "projection contains records outside its receipt scope"。
2. ~~**`CommitScope` 是必填**，不给默认值。~~ ★0916 作废，见 D2 的修正：这一格不决定存储删不删，
   默认 `WholeRoot` 才是对的（它就是今天的语义，也是旧序列化产物应有的读法）。
3. **拿缺席推删除只许有一处**：`include_str!` 护栏
   `only_whole_root_removals_derives_a_deletion_from_absence` 钉住生产代码里只有
   `whole_root_removals` 做这件事，且 `prior[…]` 只有「喂给它」与「判空」两个用处。
4. **管段完整性门**（D6）：`Partial` 的 `upserts` 里凡出现 `ImpliedTube { container_refno: C, .. }`，
   就断言该容器 C 在本次提交里的管段数 == `execute_plan` 对 C ~~算出的槽位数~~ **真建成的管段数**
   （★0916 改口：退化槽位不出几何，`kept` 只收真建成的；账由生产方另记进 `UnitDelta::tube_segments`）；
   对不上直接回退，不发。已落地：`tube_segment_completeness`，见 D6 的 ★0916 段。
5. **闸先于动记录**（D4）：`republish_gate` 同款判定必须在读 `records_of_root` **之前**，
   与 ADR-066 `republish_roots_by_rigid_motion` 的顺序一致。
6. **不动选根**：`window_root_plan` 的 `plan.regen` / 凭证前移 / 护栏退化一个字不改；
   本 ADR 只在「拿到一根之后」生效。
7. **两条便宜路的优先序要定死**：一根同时满足 transform（ADR-066）与 unit（本文）时走哪条——
   建议 transform 优先（它连生成都不做），unit 兜底；写进代码注释与测试，不靠调用顺序碰运气。
8. **对拍门是 `#[ignore]` 手动门**，不进 CI 默认集（它要 E3D 模板目录与真库），
   但必须在 `changelog.md` 留跑过的数字与日期。
9. **回退位保留到开关默认开之后一个版本**：`model_unit_level_publish=false` 时行为必须**逐位**等于今天。
10. **`Partial` 回执的章只许存储盖**（D5）：调用方不得在锁外自算 `manifest_hash` / `geometry_count` 填进
    `Partial` 回执当真值——填了也会被覆盖；要登记 durable 身份、要对 ACK，一律用 `commit` /
    `try_commit_current` 返回的那份。`WholeRoot` 反之：存储不替它算，调用方对自己写的章负责。
11. **设计窗之外的目标变化不走局部**（★0916 接生产方时加）：旧回执的 `model_target` 与本轮的相比，除 `design.session` /
    `design.digest` 之外任何一格不同（目录依赖会话、模板摘要、生成器指纹、细分档、项目 / 文件身份），或旧回执
    根本没带目标（legacy / ambiguous，`clear_dbnum` 对它也是 fail closed），一律回退整根 regen。一窗 `plan_update`
    只看 DESI 差分，看不见目录件改了形——那是 core `REPALL` 的那一档。
12. **生产方与选根用同一份计划口径**（★0916）：`UnitDeltaProducer::open` 与 `WindowRootSources::collect` 都按
    `model_skip_metadata_only_changes()` 决定走 `plan_update_with(change_metadata_tier)` 还是裸 `plan_update`。
    两边口径一旦分叉，选出来要局部发布的根与切给它的子计划就对不上——多半表现为空 delta 盖一个 T 章。

## 验收

| 门 | 内容 |
|---|---|
| A1 ✅ | ~~三处~~ **两处** `==` → `<=` 改完（`model_db_adapter.rs` 的 `RootShare::load` 与 `persist_root_impl`；`room_resident.rs` 那处经核**不该改**，见 D3），护栏测试 `a_record_may_lag_its_root_receipt_but_never_lead_it` 钉住两端（陈旧放行 / 超前逐字核 `outside its receipt scope`）。★0916 会话 opus-5-10：gen-model `cargo test --lib` **2129 绿 / 0 红 / 114 ignored**、e3d-model **320 绿 / 0 红**、`rustfmt --check` 干净、行尾 LF 未变 |
| A2 ✅ | `Partial` 提交不删未提及记录：`a_partial_commit_only_removes_what_it_names`（一根三件只重建一件，另两件原地不动；显式点名才删得掉）+ `a_partial_commit_that_resets_a_branch_still_clears_every_segment_of_it`（钉住 D6 那条相反的规矩确实存在）+ `a_commit_serialized_before_the_scope_field_reads_back_as_whole_root`（旧产物读回 `WholeRoot`）+ `only_whole_root_removals_derives_a_deletion_from_absence`（做差独此一处）。★0916 会话 opus-5-10：gen-model **2134 绿 / 0 红 / 114 ignored**、e3d-model **320 绿**、`rustfmt --check` 我改到的每一段都干净（文件里剩下的差异逐个对过 HEAD，全是旧的） |
| D5 ✅ | 局部回执在写锁里盖章、返回盖完的那份：`a_partial_receipt_is_stamped_from_the_root_as_it_stands_after_the_delta`（占位不入库；章 == 对提交后 `records_of_root` 跑配方；数 == 记录数；与直接对 `[a, c']` 算的逐字相同；返回值 == 入库值）+ `a_whole_root_receipt_is_stored_exactly_as_the_caller_wrote_it`（约束 9）+ `a_partial_receipt_never_describes_a_different_version_than_its_records`（写者 2000 次 `Partial` 提交，读者每一次 `root_snapshot` 的章与记录都对得上）+ write-behind 侧 `a_partial_publish_registers_the_receipt_the_store_actually_stamped`。把 stamp 关掉后三条针对性测试全红（mutation 核过）。★0916 会话 fable-5-1-21：gen-model `cargo test --lib` **2138 绿 / 0 红 / 114 ignored**、e3d-model **320 绿 / 0 红**、`rustfmt --check` 我改到的每一段都干净（三个文件剩下的差异按行号对过 HEAD，全是旧的）、行尾 LF 未变 |
| A3 ✅ | 管段完整性门：构造缺一段的 `Partial`，必须回退 regen 且报出原因，**不得**发布。`a_partial_delta_missing_one_tube_segment_falls_back_before_publishing`（一条 BRAN 两段 + 一件元素、回执停在 S；只带 0 号段来：`fallbacks` 一条、原因含 `BRAN 24383/721` / `carries 1 tube segment(s), plan expects 2` / `falling back to regen`、发布口一次没被叫到、`root_snapshot` 前后逐字相同；补齐两段再来：走成局部提交，两段整段换新、元素原地不动、回执前移到 43 / 数 3）+ 对门本身的 `the_tube_segment_gate_rejects_unreset_unaccounted_and_short_branches`（不重置就带段 / 重置了没账 / 少一段 / 只重置不带段 四种拦下；重置 + 有账 + 段数对上放行，序号 0 / 2 有洞不算错；没触管身与「账上 0 段的整段清空」都放行）+ 守卫 `the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records` 多核一格：产出 < 管段门 < 提交。把门短路成恒放行后 A3 那条红（mutation 核过）。★0916 会话 fable-5-1-21：gen-model `cargo test --lib` **2143 绿 / 0 红 / 114 ignored**、e3d-model **320 绿 / 0 红**、`rustfmt --check` 我改到的每一段都干净、行尾 LF 未变 |
| A4 ✅ | 幂等：同一个 `Partial` 计划连提两次，第二次判 `replayed`，投影一字不动。`a_unit_root_on_its_baseline_publishes_a_partial_commit_and_replays_idempotently`（第一次：`Partial` / `"unit"` / `source_sesno` 前移到 T / 章由存储盖 / 没点名那件停在旧会话上 / 提交后仍 `root_projection_complete`；第二次：`replayed`，生产方与发布口一次都没被叫到，`root_snapshot` 前后逐字相同）+ `the_unit_gate_sorts_replays_and_baseline_mismatches_before_anything_is_produced`（重放 / 基线不符 / 残缺 / 解析不了 / 生产方报错五种一次排开：只有过了闸的那根叫到生产方，没有一根走到发布，投影一字不动，原因逐条核文本）+ 守卫 `the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records`（完整性前置 < 闸 < 产出 < 提交；执行体不读记录不算章；与刚体路同锁同闸）。把闸的 `continue` 拿掉后两条行为测试全红（mutation 核过）。★0916 会话 fable-5-1-21：gen-model `cargo test --lib` **2141 绿 / 0 红 / 114 ignored**、e3d-model **320 绿 / 0 红**、`rustfmt --check` 我改到的每一段都干净、行尾 LF 未变 |
| A5 | 落盘：`Partial` 之后 `persist_root` 不报「outside its receipt scope」，回读记录逐字等于 regen 路 |
| A6 ◐ | 对拍门（D9 ①②）全语料全绿。★0916 会话 fable-5-1-21 只跑了**单根探针** `probe_unit_publish_one_root_against_regen`（`#[ignore]`）：ams8000_0001 五窗五根整根 `manifest_hash` 逐字相同——609→610 / 614→615（EQUI `24384/24776`，2 件）、45→46（改挂，新根 EQUI `24384/25803`）、195→196（级联，EQUI `24384/26186`）、249→250（BRAN `24384/26229`，16 件，含管段整段重写）。全语料多根 / 多窗那一档没跑，A6 仍开着。配套单测：`a_removal_that_does_not_trace_back_to_the_plan_is_refused`（D6 第一条三处来源放行、计划外单元 / 别的容器旧键拒收）、`a_target_change_outside_the_design_window_disqualifies_a_partial_delta`（设计会话前移放行；目录 / 生成器 / 模板 / 细分档各变一格都拒）、`a_degraded_or_unreadable_window_plan_admits_no_partial_publish`（Q4）。gen-model `cargo test --lib` **2146 绿 / 0 红 / 115 ignored**、e3d-model **321 绿**（新增 `per_container_tube_segments_must_split_the_tube_upserts`）、e3d-model 真库门 `an_increment_says_what_two_full_generations_say` 等三条 22 min 全绿 |
| A7 | 净收益（D9 ③）：一窗端到端时延不慢于 regen 路，数字进证据档 |
| A8 ✅ | D10：只被 `no_model` 候选碰到的根发空 `Partial` 只推回执。规划器 `no_model_only_roots_need_every_one_of_the_five_conditions`（五条各缺一条都不点名；`Other` / `Anchor` / S 端链上的删除都挡；级联目标集里的挡）+ 生产方守卫 `the_producer_guard_attributes_ledger_candidates_to_the_root_by_owner_chain`（两个方向归因、根之上的不算、`RolledUp` / `Unresolved` 点名、归不到一个就拒）+ 执行体 `a_no_model_only_root_publishes_an_empty_partial_that_only_advances_the_receipt`（不带提示的空 delta 回退不发；带提示的以 `Partial` 发出：三条记录原地、`source_sesno` 停在 S、回执到 T、章与 S 逐字相同、`receipt_only` 点名、再提是重放）+ `an_empty_partial_on_a_zero_record_root_is_not_an_emptied_root`（D2 判据改口；零记录根走成）+ `merging_the_worklist_keeps_the_unit_publish_hint` / `collect_unit_tasks` 的名单测试；守卫 `the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records` 多钉一格：空清单守卫在管段门之后、提交之前。把空清单守卫短路 / 把 D2 判据改回旧形，两条行为测试全红（mutation 核过）。★0917 会话 fable-5-1-10：gen-model `cargo test --lib -- e3d_model_service:: window_root_plan:: rigid_motion:: manual_update:: model_update_plan:: batch_worker:: …` **432 过 0 败 31 ignored**。**真库探针**（`probe_unit_publish_one_root_against_regen`，`DB_OPTION_FILE=db_options/DbOption-aps.toml` + `AIOS_UNIT_PUBLISH_PROBE_FILE` 挂文件 + `AIOS_MODEL_UNIT_LEVEL_PUBLISH=1`）：aps7343 6→7 规划器 `no_model_only 100 / regen 100`（== Q6 量表的 100 多算根），根 `23727/3` STRU 零单元 receipt-only、章 == regen@7，PARITY OK；aps250209 16→17 `373 / 373`，根 `2013286753/40174` STRU 50 件 receipt-only 0.04 s、章 `1c6dc19a…` 与 regen@17（0.86 s）逐字相同，PARITY OK；aps250209 15→16 混合窗 `311 / 373`（== Q6 的 311 多算 / 62 真变），根 `40094` 4 件 PARITY OK（以上三跑在「级联关着 ⇒ 不分桶」写死**之前**，级联关着）。**规则写死之后**（三条级联同开）：aps250209 16→17 `350 / 373`，`40174` 被判级联对端、回退 regen（原因「target of a cascade expansion」），`40094` 4 件 receipt-only PARITY OK；aps7343 6→7 `26 / 100`，`23727/3` PARITY OK；级联关着时 `0 / 373`、原因「cascade(s) off: …」、空子计划回退 regen |

## 不做

- **不拆单元级回执、不做单元级 CAS**（D5 给了理由）。
- **不让 durable 刷盘跟着切**（D7，第一版有意留整根）。
- **不动选根与凭证前移**（实施约束 6）。
- **不把 `manifest_hash` 改成可增量合并的结构**——那是 Q1 的事，本 ADR 先用重算。
- ~~**不碰「整根白算」那 46.1%**~~ ★0917 改口：S7 量到真·重算 488 根里有 255 根一件都没变却被整根重算，
  按单元占 1230/2670。原以为那全是**选根**的题（S6 元数据门 / 计划 §8 Q6）——Q6 第一手把其中「只被 `no_model` 候选碰到」
  的那一类的单元归零之后，剩下的正是「touched 但 0 单元」，D10 在单元路上把它们变成只推回执。选根侧（不做闭包种子）
  按 d-365 不动；D10 覆盖不到的是级联 / 恢复 / 目录名单选进来的空子计划根（照旧整根 regen，见 D10 判据 ③ ④）。

## 开放问题

- **Q1** D5 的整根 digest 重算是 O(根内记录数)。在 `BRAN` 那种 2812 单元的大根上，它会不会把省下的
  生成时间吃回去？→ D9 ③ 先量；真成瓶颈就把 manifest 改成按 `GeometryId` 排序的 Merkle / 可增量摘要，
  另起小 ADR。
- **Q2** D3 走「放宽 `<=`」还是保守的「局部生成、整根 upsert」？本文推荐前者，后者作为回退位。
  用户如果不愿动 `source_sesno` 的语义，改走后者，本 ADR 其余各条不变、收益打折。
- **Q3** write-behind 要不要跟踪脏记录 id、只刷改动那几条（D7 的另一半）？先量整根刷在真窗口上的占比。
- **Q4** `execute_plan` 的 `full_rebuild` 退化档返回**空产出**。调用方必须识别它并走整库重建——
  把空产出当「这一窗没变」发布出去就是静默缺件。这条在 `increment.rs` 已有注释，
  但生产接线时要有一条测试钉住，算本 ADR 的门还是算 e3d-model 自己的门？
  ★0916 答：算本 ADR 的门，落在 gen-model 侧——`UnitDeltaProducer::produce` 第一件事就是
  `WindowPlanSets::admits_partial_publish()`（`full_rebuild` 非空或计划期有读挂就 `bail`，D8 ① ②），
  根本不会去调 `execute_plan`；单测 `a_degraded_or_unreadable_window_plan_admits_no_partial_publish` 钉住。
  e3d-model 那边保留 `increment.rs` 的注释与「空产出 + `degraded_to_full`」的报告形状，不另加门。
- **Q5** S7 的数只来自 ams8000 一条链、抽样 16%。要不要在动工前先把全语料跑一遍，
  免得 86.5% 是这条链的特例（`BRAN` 占比 380/488）？
- ~~**Q6**（★0917 D10 派生）D10 对「跨根设计引用的引用者只被盖戳」这一形状的安全靠 C2 / C8 / P4 级联把引用者的根放进目标集
  （条件 ③）。三条级联今天都默认关。要不要写死「级联关着 ⇒ 规划器不分桶 D10」（安全，但默认配置下 D10 一根都打不着），
  还是让 D9 开门的三道门里加一条「C2 / C8 同开」，或干脆接受这条已知差异？~~ ★0917 **已拍：写死成规则**（用户原话「把『级联关着 ⇒
  D10 不分桶』写死成规则（安全优先，默认配置下 D10 不生效）」），落在 D10 的「前置」段与 `no_model_only_cascades_off`。
