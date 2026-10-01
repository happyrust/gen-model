# 2026-09-06 计划：模型增量的「联动查询」换源——照 core，从 dab 引用表读，不再维护派生索引

- 日期：2026-09-06（会话 fable-5-1-24）。**状态：已批准**（用户 2026-09-06「计划没问题，按推荐项拍 Q1–Q4，直接开工 P0」，
  共识 d-635；Plannotator 会话以面板拍板代替）。**P0、P1、P2、P3 已完成**（见 §3 各自的「落地」段；P1 = e3d-io `4eeecf9`；
  P3 = `reverse_reference.rs` + `reconcile_catalogue_versions` 接线，**未 live**，主门按前置问题的实测答案要改到 `ams7000_0001`），
  P4 起未开工。P3 对零引用表目录库的退路已拍 **(ii)**（共识 d-665）：
  SPCO 层走 DESI 侧表点查，SCOM 层在零表目录库上退化为该库依赖者全部过期并点名。
- 问题：**现在的增量更新，是否已经处理了模型增量更新的「联动查询」（反向引用查询：谁引用了变了的元素）的更新处理？**
- 取证：`docs/evidence/2026-09-06-dab-ref-table-write-path.md`（本轮 ida-bridge，core 写侧）、
  `docs/evidence/2026-09-04-dab-ref-table-layout.md`（读侧，§5 两条本轮闭合）。
- 承接（不重复）：`docs/2026-09-04_model-cascade-update-inventory.md`（十一类联动，#4 是唯一的洞）、
  `docs/plans/2026-09-04-cata-reverse-cascade-via-dab-ref-table-plan.md`（P0–P4；本计划**取代**它的 P1–P4 排期）、
  ADR-003（`ref_rev`）、ADR-008（CATA 反向级联）、ADR-058 D2 / Q1（CATA 触发源换成文件差分；e3d-io 读反向表是同一件事的另一条路）、
  `docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` G5（读透跳过摄入链）。

---

## 0. 一句话回答

**没有处理完，而且现有的那一半正站在要被抽掉的地基上。**

| 联动查询的哪一半 | 现状 | 判定 |
|---|---|---|
| **A. 查询本身**（变更元素 → 谁引用它 → 上卷到根） | 老规划器（`build_model_update_plan`，非 direct 或非 DESI 窗口）有：`resolve_unit_rollup` 读 `ref_rev` 反查、`direct_root_allowed` 级联到引用者的交付单元；CATA 窗口 `build_cata_cascade_plan` → `CascadeExpand` → `expand_live_reverse_cascade`（`ref_rev` live 反查） | ◐ **只在老路上有** |
| | **direct + DESI（今天的默认）** 走 `window_root_plan::build_model_update_plan_from_window` → e3d-model `plan_update`：owner 链闭包 + 变换子树 + CTRAY 整条，**不做任何反向引用查询**（不读 `ref_rev`，也不读 dab 引用表） | ⚠️ 新路**没有** |
| | **direct 下 CATA**：没有数据批次 → `build_cata_cascade_plan` 根本不会被调；只剩 `ModelTarget.catalogue` 指纹失配 → 该库全部根过期 + 启动 `catalogue_cascade_degraded` 点名（09-04 §2.7）。ADR-058 D2「CATA 文件差分 → `ref_rev` 反查」**未落地** | ❌ inventory #4 的洞**原样** |
| **B. 查询所依赖的索引的更新**（`ref_rev` 维护） | `apply_one`：persist → invalidate → `maintain_reverse_index`（每元素 `DELETE <el>->ref_rev; INSERT RELATION`）→ 失败进 `enqueue_ref_rev` 补偿 → `repair_reverse_index_for`；~~CATA→CATA 边靠 `cata_ref_rev` 闭包任务（D8-A）补~~ **更正（2026-09-07）**：`cata_ref_rev` 从来只落目录**行**（`pe` / noun / UDA / `pe_owner`），一条边都没写过；CATA→CATA 边只有 `rebuild_reverse_index`（`bin/rebuild_ref_rev`）会建。该任务已随 ADR-064 Q2 退役，目录行改由 `cata_dependency` 在水位之前落 | ✅ 有，但**只在摄入链上** |
| | 读透形态（默认 `data_face`）的 G5 定了「跳过摄入链」；~~ADR-058 S3 定了 `cata_ref_rev` 一并退役~~（2026-09-07 已退役，ADR-064 Q2）→ **G5 之后 `ref_rev` 没有维护者** | ⚠️ 正在失去地基 |
| **C. 换源的准备** | e3d-io `ReferenceTable` / `Engine::referrers_of`（会话钉住的表根、4 字键 `(target, referrer)`、543 库 / 906 表 / 3.7M 条 / 0 乱序门）**已实现**（~~工作树未提交~~ P1 已入库 `4eeecf9`）；gen-model `direct_store::referrers_of(dbnum, attribute, target)` **已包好** | ✅ 就位；~~**仓内零调用方**~~ **★0907 已有两个真调用方**：`data_interface/reverse_reference.rs`（本计划 P3，CATA 侧闭包 + DESI 侧一跳，自己开 `ReadOnlyEngine` 逐库、逐表点查 `referrers_of`）与 `data_interface/cable_endpoint.rs`（联动对齐计划 C8 同库版：`EQUI`/`ELCONN`/`CWBRAN` 变了 → T 端 `STAREF`/`ENDREF` 表点查电缆，直接走 `ReadOnlyEngine::referrers_of`，开关 `model_cable_endpoint_cascade` 默认关）。后者是 P4「direct DESI 规划器接同一个源」的一个**专用实例**（只查电缆两条边、只查本库）；~~P4 那条通用一跳（kind-16 属性、MDB 逐库并）仍未接~~ **★0907 P4 已接**：`reverse_reference::DesignReferenceCascade`（同模块第二个入口，MDB 逐库并集，行为只有 `POSREF`，开关 `model_design_reference_cascade` 默认关，见 §3 P4 落地段）。`direct_store::referrers_of` 这层包装本身仍无人调——三个调用方都直接用 `ReadOnlyEngine::referrers_of` |

**core 的口径**（本轮实读，evidence §1–§3）：引用表由 dabacon 在 `db_put_reference` **写时同步维护**（删 `(旧目标, 自己)`、
插 `(新目标, 自己)`、表不存在当场建），表根随会话页版本化；读侧 `DB_MDB::findElements` 逐库游标点查，**零维护**。
GETWORK 刷新路径不调引用表。所以 core 里「联动查询的更新处理」这件事**不存在**——它被文件格式吞掉了。

**结论**：gen-model 该做的不是把 `ref_rev` 接进新规划器，而是**照 core：反向查询直接读文件里那张表**——
`ref_rev` 从「正确性依赖」降为「UI 便利」，CATA→DESI 反查（#4）与 direct DESI 路径的反向查询用同一个源一次接上。

## 1. 现状核对（按代码，可复查）

| 事实 | 位置 |
|---|---|
| direct + DESI 走窗口规划器，其它走老规划器 | `increment_pipeline.rs:1017-1040` |
| 窗口规划器的输入只有两端 `DbSet` + `gen_root`，反向引用零命中 | `window_root_plan.rs` 模块头；e3d-model `increment.rs` 模块头（级联只有变换子树、派生管身、CTRAY） |
| 老规划器的反查：`OwnershipSnapshot::ref_reversal` ← `ref_rev`；`direct_root_allowed` | `manual_update.rs:802-816`、`:880-1020` |
| CATA 反向级联执行器 | `model_update_plan.rs:792-839`（种子）、`manual_update.rs:2088-2141`（`expand_live_reverse_cascade`） |
| `ref_rev` 维护与补偿 | `increment_pipeline.rs:1148-1170`、`:1848-1855`；`side_effect_pending.rs`（`RefRevMaintain` / `CataRefRev`） |
| e3d-io 反向表读法 | `vendor/e3d-io/src/index/reference.rs`、`src/engine.rs:345-372`、`src/session/mod.rs:88-125`（`TableRoot`） |
| gen-model 包装、零调用 | `direct_store.rs:590-606` |
| 直读默认、CATA 不入库 | ADR-058 D1 / D2 / S3；`options::direct_read_mode()` |
| 读透跳过摄入链（未做） | 09-05 kv-mem 计划 G5 |

## 2. 原则（照抄 core，三条）

1. **反向查询从文件读**：`(库, 会话, 属性, 目标 refno) → 引用者集合`，按库点查、跨库并集（与 `DB_MDB::findElements` 同构）。
2. **不维护第二份事实**：不再为规划正确性维护 `ref_rev`；它若继续存在，只服务 UI（目录属性展示），且**不许**再被任何选根代码读。
3. **护栏不对称**：反查失败 / 表不可读 / 变动件数超阈 → 退到今天的行为（该库全部根过期），并点名
   `catalogue_cascade_degraded`；「判不了 → 全部过期」，宁多算不漏算。

## 3. 分期

### P0 · 覆盖差表 + 取证收口（0.5–1 天）

- 把 `ref_rev` 的建边资格（`reference_edge_eligible`：schema ELEMENT 引用 + curated `DependencyCascade`）
  与文件表的属性集合（描述符 kind 16；e3d-io 真库测试列出 17 个）**做一张差表**，落
  `docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`。差集预期 = kind 4 的连接类引用
  （`HREF`/`TREF`/`CREF`…）。对差集里每个属性写一句「e3d-model 几何是否读它跨单元」——今天 grep 的结果是
  **一个都不读**（`polyhedron.rs` 的 `VXREF` 在 POHE 单元内部；`route.rs` 用存储的 `HPOS`/`TPOS`），把这句钉成测试。
- e3d-io 侧：确认 `record/descriptor.rs` 是否暴露 kind；不暴露就用 `DbView::tables()` 的实测名单代替。
- 09-04 evidence §5 两条改「已闭合」（指向本轮 evidence §2）；09-04 计划 P0 标完成。
- **验收**：差表落盘；护栏测试 `e3d_model_reads_no_cross_unit_design_reference_outside_file_tables` 绿。

> **落地（2026-09-06）**：
> - 差表 `docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`——783 库实测 **75 张**反向引用表
>   （DESI 侧 40 / CATA 侧 19），`ref_rev` 资格 603 个名字；e3d-model 几何读 **15** 个引用属性，12 个有表，
>   3 个没有：`VXREF`（单元内）、`HSRO` / `LSRO`（仅 stub 备选、E3D 自己也不索引）。**F \ R 在 DESI / CATA 侧为空**。
> - 护栏改名落在 e3d-model：`vendor/e3d-model/tests/reference_reads_guard.rs`——每个 `get_element` / `get_ref` /
>   `get_ref_array` 读到的名字必须归进「有表 / 单元内 / 在案缺口（带理由）」三档，间接读点逐处登记；
>   2 passed，负例（把 `VXREF` 从名单摘掉）红并点名。
> - **Q4 修正一格**：DESI→DESI 有表且 e3d-model 真读的几何边只有一条 **`POSREF`**（电缆桥架成员位置参照，
>   DESI:31 库 / 7 109 条、目标全在本库）。P4 不是纯护栏：`POSREF` 的引用者要并进 `touched`，其余属性不动。
> - e3d-io 描述符不暴露 kind，F 用实测表名代替；判「是不是引用表」的可靠判据是 `referrers_out == 0`
>   （`TYPEX` / `OCONET` / 5 张 PADD 表同形不同义），写进 P1。

### P1 · e3d-io 收尾（1 天）

- 提交工作树里的 `src/index/reference.rs` / `tests/reference_table_real.rs` 改动（先看 `git status`，那边有别人的编辑）。
- 加一个 MDB 级并集辅助：`referrers_of_across(dbs: &[&mut DbView], name_hash, target)`——或留在 gen-model 侧做并集
  （推荐后者：e3d-io 保持单库纯函数，与 `findElements` 分层一致）。
- 成本门：`ams7351_0001`（334 万候选）上点查 vs 全表遍历耗时比，写进 `reference_table_real.rs`（`#[ignore]`）。
- **验收**：`cargo test -p e3d-io reference_table` 绿；成本门数字进 CHANGELOG。

> **落地（2026-09-06，e3d-io 提交 `4eeecf9`）**：`reference.rs` 只剩 rustfmt 改动（功能早已在 HEAD），连同
> `tests/reference_table_real.rs` 四条新测试与 CHANGELOG 一条一起提交；工作树里别人的 WIP 一个字没动（CHANGELOG 只
> 暂存了自己那一个 hunk）。并集辅助按推荐留在 gen-model 侧（P3），e3d-io 保持单库纯函数。
> - **判据**：`a_reference_table_is_told_apart_by_its_referrers_all_being_local`——引用表 = 4 字键、无值、
>   **引用者全是本库 refno**（`db_put_reference` 把边记在引用库里）；`TYPEX` 与 7 张 Draft 表同形不同义
>   （`KNOWN_LOOKALIKES`）。`ams8000_0001`：引用表 {CATR HSTU ISPE LSTU SPRE}，lookalike {TYPEX}，别种 {NAME SPAMAP}。
>   语料门（`#[ignore]`）改为按判据走全部具名表：543 库 / **913 张引用表 / 58 个名字 / 8 个 lookalike 名**，无新面孔。
> - **成本门**：`a_point_query_costs_the_height_of_the_tree_not_the_table`——语料里最大的引用表是
>   `acp7320_0001` 的 `CATR`（744 925 条）：整表遍历 **7 907 页**，点查 **4 页**（2 个引用者）；门钉在 ≤ 16 页且 ≤ 遍历的 1/100。
> - **★ 偏差，改写 P3 的护栏**：`ams7351_0001`（1.2 GB / 334 万键，计划原想拿它做成本门）**一张引用表都没有**
>   ——目录里只有 `NAME` 与 `TYPEX`，`CATR` / `GMRE` 全无。钉成 `a_database_can_state_no_reference_table_at_all`：
>   `ReferenceTable::open` 回 `NoSuchTable`，调用方**必须**先问 `table_root`，把「这个库不留这张表」当成独立结果，
>   不能当「没人引用」。含义：P3 的 CATA 侧闭包对这种目录库算不出 SCOM←SPCO 那一跳，只能（i）对该库退路 ②
>   扫 SPCO 建内存表，或（ii）把该库的每个变动目录件都当「可能被引用」→ 走 DESI 侧 `SPRE`/`HSTU`/`LSTU` 表时用
>   变动件本身（SPCO 层）点查、SCOM 层变动退化为该库依赖者全部过期并点名 `catalogue_cascade_degraded`。**P3 开工前先定 (i)/(ii)。**

### P2 · CATA 触发源：文件差分，不靠数据批次（2 天）

- ADR-058 D2 的正文：CATA 文件会话前进（watcher / 启动复核）→ `collect_window(cata DbSet@S, @T)` →
  变动目录件集合 `changed_catalogue: BTreeSet<RefNo>`（Added / Modified / Deleted 都算；Added 唤醒此前解不出的引用）。
- 挂点：`batch_worker` 的文件轮次今天只对 DESI 落意图行；给 CATA 加一种**只有模型面、没有数据批次**的窗口工作
  （名字建议 `CataCascadeWindow`，进 `side_effect_pending` 同一张表同一条重试通道）。
- **验收**：`ams5052_0001 184→185` 出恰好 1 个 Modified 目录件（09-04 §2.10 已探明）。

> **落地（2026-09-06）**：
> - 新模块 `src/data_interface/cata_cascade_window.rs`（文件侧纯函数，不碰库）：`collect_catalogue_window[_bounded]`
>   = e3d-io `diff_sessions` → 逐候选 `ParsedElement::parse` 读 noun / owner（模板无关）→ **剔除 `relocated_only`**
>   （两端字节相同、只是搬了位置的「Modified」不是变更）→ T 端 `reference_tables_at`（判据与 P1 同一条：
>   4 字键、无值、采样引用者全是本库 refno；`TYPEX` 被挡）。`CatalogueWindowReport` 是回执 / `/health` 形状，
>   收不了的窗口带 `error`（**未知**，不是零）；候选超上限只报总数（`truncated_candidates`，导入级窗口）。
> - **验收门过了**：`ams5052_0001 184→185` = 1 个 Modified、0 增删、noun **`SDTE`**（数据表项，属主链通向 DTSE←SCOM，
>   即 P3 要走 CATA 侧 `DTRE` 闭包的形状），反向表 `[BRREF CATR DRSSRE DTRE GMRE GSTR NGMR NOMREF PRTREF PSTR PTRE REDREF]`。
> - **挂点与计划不同，且更省**：读透（默认）形态下 `increment_manager::sweep_watch_dirs` 对**任何**文件事件都不造数据批次，
>   只在 MDB 骨架前移时调 `reconcile_model_coverage_at_startup`——启动与文件事件汇到同一处。所以 P2 的触发源就挂在
>   `model_update_pending::reconcile_catalogue_versions`：对每个**过期**目录库（S = `gen_root.published_model_target.catalogue`
>   记在案的最旧会话，T = 文件最新）收一窗，横幅逐库一行（`banner_line`），`/health.startup.reconcile` 新增
>   `catalogue_windows`。**没有新建 `CataCascadeWindow` 任务种类**，也没有新水位：S 由根自己的落款派生，P3 重算 + 凭证前移
>   之后落款自然追到 T。摄入形态的 CATA 数据批次照旧走 `build_cata_cascade_plan`（老路，`ref_rev` 在那形态下仍被维护）。
> - **仍不排工作**：P3 未接，`catalogue_cascade_degraded` 的语义不变；这一步只把「变了哪些目录件、能不能反查」摆到台面上。
> - **验证受阻，如实记**：模块 5 条测试在仓外同源 crate 上跑过（`ams5052` 门、`ams7351` 零表、上限截断、失败回执、横幅），
>   `cargo check --lib --tests` 只剩**别人 WIP 的两处编译错误**（`direct_tree.rs:249` 重复 `dbnum_of`、`handlers.rs:1014`
>   `anyhow::Error: Serialize`，两文件 14:35 起未再动），本模块、挂点与 `StartupReconcile` 新字段**零告警零错误**；
>   仓内 `cargo test` 等那两处修好再跑（新增源码钉 `stale_catalogue_windows_reach_both_exits_and_the_banner`）。**未 live**。

### P3 · 反查接线：CATA 侧闭包 + DESI 侧一跳 + 上卷（3 天）

- 新模块 `data_interface/reverse_reference.rs`（纯函数为主）：
  1. **CATA 侧闭包**：变动目录件 → 在**目录库自己的表**里反查（`CATR` / `GMRE` / `PTRE` / `DTRE` / `ISPE`…：
     SCOM ← SPCO 走 CATA 库的 `CATR` 表；GMSE ← SCOM 走 `GMRE` 表），直到集合不再增长；目录中间件**只走过、不成根**
     （`expand_live_reverse_cascade` 已有这条纪律，照抄）。
  2. **DESI 侧一跳**：对 MDB 内每个 DESI 库、对它 `DbView::tables()` 里的每张表，`referrers_of(name_hash, target)` 并集
     → 设计元素集合。不写属性白名单：**表在就查**（表的存在本身就是「这个属性有反向引用」的权威声明）。
  3. **上卷**：`resolve_element_generation_root` 在 `DbSet@T` 上 → 生成根集合 → `RegenRoot`；其余根凭证前移
     （与 `window_root_plan` P2-2 共用同一份「根受影响集」，不另写）。
- 护栏：任一库表不可读 / 变动件数 > 阈值（先取 09-04 §2.10 的 50）→ 该库全部根过期 + `catalogue_cascade_degraded`；
  回执与 `/health.startup.reconcile` 复用 09-04 §2.7 已落好的栏位。
- **验收**：`ams5052_0001 184→185` 只有引用它的设计件所在根进 `regen`，其余根 `advance`；
  **先答前置问题**「`ams5052` 有没有被本语料的 DESI 库引用」——P3 第 2 步的并集就是答案，为空则主门改
  `ams7000_0001`（DESI 门本就钉它当 `PROJECT_CATALOGUE`）。

> **落地（2026-09-06，两段）**
>
> **P3-a 反查本体**（会话 fable-5-1-24 → 33）：`src/data_interface/reverse_reference.rs`——`expand_reverse_cascade(sources, seeds,
> resolve_root)`，纯文件侧：种子 ∪ 目录祖先链（`SDTE` 变了真正被引用的是属主 `DTSE`）→ 对 MDB 每个目录库的每张反向表点查到集合
> 不增长（d-665 不设深度上限；中间件只走过不成根）→ 对每个设计库的每张表点查闭包全部目标 → `resolve_element_generation_root`
> 上卷。退路 (ii) 按 d-665：零表目录库上 `DESIGN_REFERABLE_NOUNS`（`SPCO`/`SPEC`/`TABITE`）层种子照常走设计侧，其余层让该库
> `degraded` 并点名。`ReverseCascade` 带 `tables_used` / `point_queries` / `unrooted` / `unreadable` 全套账。
>
> **P3-b 接线**（会话 fable-5-1-76）：`model_update_pending::reconcile_catalogue_versions` 收完窗之后接
> `cascade_stale_catalogue_windows`：
> - `split_catalogue_seeds`：窗口收不了（`error`）/ 导入级窗口（候选超 20 000 未逐件读）/ 变动件数 > **50**
>   （`CATALOGUE_CASCADE_SEED_CAP`，取 09-04 §2.10）→ 该目录库退化；其余窗口的增 / 改 / 删目录件全进种子。
> - 反查在 `spawn_blocking` 里：`SourceDb` 来自 `current_mdb_sources()`（DESI + CATA），过期目录库钉窗口 T 端、其余取最新；
>   上卷用一份 `E3dModelService::build_set(_, None)` 的 `DbSet@最新` + `resolve_root_from_set`（与稳态窗口选根同源）。
> - **排工只对落款仍在旧目录上的根**：`roots_behind_catalogue_on(X, T)` 一条语句问出 `gen_root` 里
>   `published_model_target.catalogue[WHERE dbnum = X AND session < T]` 非空的行（按设计库分）。命中根 ∩ 这份名单 → `RegenRoot`
>   （`source_end_sesno` = 设计库文件最新，ADR-054）；命中但有 `gen_root` 行且落款已到 T 的记 `already_current`（重启前已重算过，
>   不重排）；命中却没有 `gen_root` 行的记 `lazy_roots`（从未生成过，ADR-058 D6 懒生成，启动不排）；限定域外 / 发布队列阻断的库跳过。
>   退化库（窗口级 + 反查自判）的**整份名单**都排（`enqueued_degraded`）。`enqueue_plan` 走同一张 `model_update_pending`，
>   排完 `BatchScheduler::wake()`。
> - **回执**：`/health.startup.reconcile.catalogue_cascade`（`CatalogueCascadeReport`：只有数与库号，不带清单；`null` = 没有过期库，
>   带 `error` = 跑了没跑成且一根没排——**未知**不是零）；横幅在逐库窗口行之后加一行 `banner_line()`。
>   `catalogue_cascade_degraded` **改义**：P3 前 = 「目录前移了、启动不为此排工作」；现在 = 「前移了**且**反查没能精确回答」
>   （某库退化或整个反查失败）——精确命中只排引用根的启动这一位为**假**。`stale_catalogue_dbnums` 仍列全部前移库。
>   `result` 的取重照旧不看目录那一格（源码钉 `the_catalogue_flag_is_named_separately_and_never_downgrades_the_result` 改成钉新语义）。
> - 反查任一步失败（MDB 成员读不到、某个库文件打不开、上卷失败）→ 回执 `error`、一根不排，行为与 P3 前逐位相同，只是会说出来；
>   唯一兜底仍是按需 `ensure` 的目录指纹失配。
>
> **验收门实测**：`the_ams5052_change_closes_over_the_catalogue_and_is_looked_up_in_every_design_db` 绿——
> `ams5052_0001 184→185` 的 `SDTE` 种子闭包 5 个（`SDTE→DTSE→…`，CATR / DTRE 都点查了），语料全部 DESI 库点查 3 190 次，
> **设计引用者 0**。即前置问题的答案是**「`ams5052` 没有被本语料的 DESI 库引用」**，按本节原文主门应改到 `ams7000_0001`——
> 那一窗（S = 已发布根记在案的 7000 会话、T = 文件最新）**还没有跑过 live**，「只有引用它的根进 regen、其余根不动」仍是**未验证**。
>
> **有意没做、待拍板**：「其余根凭证前移」——未受影响的根的 `published_model_target.catalogue[X].session` 仍停在 S，所以下一次启动
> 会再收同一窗、再反查一次（点查便宜；命中根这时已在 T 上，记 `already_current` 不重排，**不会重复排工**），但目录那一行会一直
> 在横幅上出现，直到那些根因别的原因重算。把它推到 T 等于在目录段上做一次「凭证前移」——它与 §5「不动 `ModelTarget.catalogue`
> 指纹」是不是一回事、`digest` 要不要一并改，要先定；定了之后照 `render_credential_advance` 的守卫（安定 + `ready` +
> `desired == published`）写一条 `UPDATE gen_root SET published_model_target.catalogue[WHERE dbnum = X].session = T` 即可。
>
> **验证（如实）**：`cargo check --lib --tests` 零告警零错误（09-06 21:10）；新增单测 `catalogue_seeds_split_degraded_windows_from_seeds`
> / `no_stale_catalogue_means_no_cascade_report` / `roots_behind_a_catalogue_are_those_still_stamped_before_its_target`（mem://，
> 数组过滤真跑）/ `the_cascade_report_carries_counts_not_lists_and_names_every_bucket`；两条源码钉改成 LF 归一（这棵树 CRLF，
> P2 那条 `\n` 探针在仓内本来就红）。**未 live**：`cascade_stale_catalogue_windows` 全链（`from_current` → 反查 → `enqueue_plan`）
> 没有在真库上跑过。

### P4 · direct DESI 窗口规划器接同一个源（2 天）

- `build_model_update_plan_from_window`：对 `plan_update` 报出的变更元素，多问一句「谁经 kind-16 属性引用它」
  （同一 `reverse_reference.rs`，DESI 库对 DESI 库：`NGMR` / `PSTR` / `GSTR` / `POSREF`… 有表就查），
  引用者上卷到根并入 `touched`。P0 的差表若证实今天 e3d-model 不读任何跨单元设计引用，这一步**只加护栏不加行为**
  ——但入口要在，等几何覆盖扩到读跨单元引用那天，反查已经在原位。
- **验收**：`increment_planner_parity` 对拍：老规划器（`ref_rev`）与新规划器（文件表）在 `ams8000_0001`
  若干窗口上的根集合，差异只能出现在 P0 差表列出的属性上，且逐条可解释。

> **落地（2026-09-07）**：`data_interface/reverse_reference.rs` 里的 `DesignReferenceCascade`——与 P3 的
> `expand_reverse_cascade` 同模块、同判据（`reference_tables_of`）、同点查（`ReadOnlyEngine::referrers_of`），
> 分开的只是种子从哪来（一窗的变更设计元素 vs 变动目录件）与结果排给谁。挂在 `WindowRootSources` 上，
> 形状与 C2 `structural_connection` / C8 `cable_endpoint` 一致；开关 `model_design_reference_cascade`
> （`AIOS_MODEL_DESIGN_REFERENCE_CASCADE`）~~**默认关**~~（**2026-09-30 起默认开**，GitHub #32，见本节末），
> 关着时连 MDB 成员名单都不解。
>
> - **逐库并集**：`E3dModelService::design_sources()` 给出 MDB 的全部设计成员（含各自的生成会话，ADR-054 Q1），
>   本库换成钉住窗口 T 端的那一份。一张反向表只答它自己那个库的引用者（表在引用方的库里），跨库引用者
>   只有逐个开成员库才找得到——这正是 C8「只查本库」留下的那一半。
> - **行为只有 `POSREF` 一条**（Q4 经 P0 差表修正）：`DESIGN_CASCADE_ATTRIBUTES` 就这一个名字，它的引用者
>   上卷成根并进 `touched`；其余设计侧表照查，但**只落进 `edges_by_attribute` 记账**。加一个名字就是多重算
>   一批根，依据只能是「e3d-model 几何真读它」。
> - **判不了 ≠ 没有引用者**（§2 原则 3）：成员库打不开、点查读挂、或这一窗要发的点查次数超
>   `DESIGN_CASCADE_QUERY_BUDGET`（20 万；开完库、读完表清单之后、发第一次点查之前算，超了一次都不发）
>   → `degraded` 交给 `WindowImpact::cascade_degraded` → **放弃凭证前移**，已持久化的根全部重算，
>   不静默取空集。预算钉在**点查次数**（变更元素数 × 要开的库里反向表总数）而不是变更元素数上：
>   逐库并集把成本乘上了成员库个数，只数变更元素说明不了成本——这一条是被下面那 93 窗对拍逼出来的。
> - **在案缺口**：`POSREF` 命中但引用者在**别的**设计库里时只计数（`foreign_referrers`）不排工——本窗口的
>   `roots_T` 是本库的，跨库排工不在 P4 范围。P0 差表说 `POSREF` 目标 0 跨库，所以这一格今天该恒为 0，
>   出现即是新事实。
>
> **验证**：`cargo test --lib reverse_reference` **18 passed / 2 ignored**——三态契约（`POSREF` 引用者把它的根
> 从 `advance` 拉进 `regen` / 非行为属性只记账不改四桶 / 开关关时逐字相同）都走**生产同一段**
> `plan_window_roots`；另有退化两路（成员库打不开、点查读挂）、超上限不点查、本库钉 T 端、跨库引用者记账、
> 悬空引用者、行为名单就一条。全库 `cargo test --lib --features http_api` 1764 passed / 0 failed。
>
> **真库两条门都跑了，而且是整个语料的批量**（`reverse_reference::tests::live`，`#[ignore]` 手动）。
> 带 `POSREF` 表的设计库由测试自己在 5 个项目目录里扫（写死一个不行，只挑第一个也不行）：
> 扫出 **31 个**，与 P0 差表那句「DESI:31 库」逐字对上——`ams000` 16 个、`aps000` 11 个、`acp000` 1 个、
> `ZDJ000` 3 个、`acw000` 0 个。
>
> - **31 个库逐个答得出**（`live_every_posref_table_answers_its_own_referrers`）：每个库拿自己 `POSREF`
>   表里现成的一条边当假定的变更源（语料里没有「只改被参照件」的自然窗口，与 C8 同一手法），
>   反查回来的根与正向沿 owner 链算出的期望**逐字相等**，31 个库无一例外，`query_failures` 全 0。
>   这一条**真的走了逐库并集**：peers 取同目录其余带 `POSREF` 表的库（`dbs_queried` 16 / 11 / 2 …），
>   而 `foreign_referrers` **全语料为空**——P0 差表那句「`POSREF` 目标 0 跨库」现在有了跨库点查的实证，
>   而且这条门就是它哪天不成立时的报警器。观测车道顺带看见 `STAREF` / `ENDREF`（C8 那两张）也有边。
> - **93 窗两态逐字相同**（`live_existing_windows_never_lose_a_root_with_the_cascade_on`，31 库 × 最后 3 步）。
>   断言分两层，弱的那层才是真契约：`regen_off ⊆ regen_on`（级联只加不减），以及无事可加时逐字相同。
>   最大的几窗：`ams251155_0001 2→9` 1 841 个变更元素 / 9 205 次点查 / `POSREF:430` → 91 个根；
>   `aps250110_0001 29→30` 7 139 个变更元素 / **71 390 次点查** / `TMRREF:1282`（观测车道）→ 0 个 `POSREF` 根。
>   **两态不同的 0 窗。**
> - **成本实测**：整趟 93 窗对拍 **200 s**，其中点查量最大的那一窗 71 390 次——把它从「判不了」改成真查
>   之后，整趟反而从 214 s 降到 200 s。点查是 O(树高)，成本淹没在索引扫描里，量不出来。
>
> **这批对拍改了一处实现**：预算原本钉在变更元素数上（2 000），`aps250110_0001 29→30` 那 7 139 个元素因此
> 判不了 → 放弃凭证前移 → `regen` 从 251 涨到 495，**白重算 244 个根**。而它真查也只要 71 390 次、量不出来。
> 所以改成钉**点查次数**（`DESIGN_CASCADE_QUERY_BUDGET` = 20 万，开完库读完表清单后、发第一次点查前算）：
> 它才是真成本，也才是逐库并集会乘大的那个量。改完 93 窗全部逐字相同。
>
> **未做（如实）**：
> - 本节写的 `increment_planner_parity` 对拍**没做**，而且它的前提已经没了：老规划器读的 `ref_rev` 在读透
>   形态下由 ADR-063 D2 停止维护、D3 直接报错，对拍只在摄入形态下才有意义。要做得先决定在哪一形态下对。
> - 两态门只证了「开关不改今天任何一窗的结果」，**没有**证「改了被参照件、参照它的根会进 `regen`」——
>   语料里没有这种自然窗口，要么自造一个会话，要么等真项目里出现。
> - 跨库排工、以及 P5 那条 live 验收，都还在原地。
>
> **默认翻开（2026-09-30，GitHub #32）**：`model_design_reference_cascade` 由默认关改为**默认开**——代码默认
> （`options::effective_default_on_switch`：环境变量 > 配置 > `true`）、`DbOption.toml`、出厂模板
> `scripts/templates/DbOption.factory.toml` 三处同为 `true`，单测 `the_design_reference_cascade_defaults_on_and_stays_switchable`
> 钉住真值表与「模板 = 代码默认」。依据是上面三道门在当天的树上重跑全绿：
> - `live_every_posref_table_answers_its_own_referrers`：查过 31 个库，「表在但零边」的 0 个；
> - `live_existing_windows_never_lose_a_root_with_the_cascade_on`：对拍 93 窗，两态不同的 0 窗（215 s）；
> - `live_ams8000_pinned_windows_land_in_the_expected_buckets` 在 `AIOS_MODEL_DESIGN_REFERENCE_CASCADE=1` 与 `=0`
>   下各跑一遍都过，9 个钉死窗口的 touched / regen / delete 两态逐字相同。
>
> 翻开时补了一处只有默认开才会撞上的失败面：`E3dModelService::design_sources()` 原来对每个未钉住的设计成员问
> 最新会话，一个问不出（文件不在、被占）就 `?` 让这一窗规划报错——默认关时这条路不走，默认开之后一个成员出事
> 会让**所有**设计库的窗口规划一起停。现在问不出交 `sesno: None`，由反查按文件最新自己开；开不了记进
> `unreadable_dbs`，按 §2 原则 3 整窗退化并点名（单测
> `an_unreadable_design_member_degrades_the_reference_cascade_instead_of_failing_the_window`）。
>
> 回退：配置 `model_design_reference_cascade = false`，或环境变量 `AIOS_MODEL_DESIGN_REFERENCE_CASCADE=0`
> （压过配置，不必改文件）。上面「未做」三条不因翻开而变：「改了被参照件 → 引用者进 `regen`」的正向证据仍只有
> 单测三态契约、语料里没有自然窗口；跨库排工仍记账不排。

### P5 · `ref_rev` 降级（1 天 + 一份 ADR）

- ADR-063：`ref_rev` 不再是正确性依赖。选根代码全文不含 `ref_rev`（源码钉）；`maintain_reverse_index` /
  `enqueue_ref_rev` / `repair_reverse_index_for` ~~/ `cata_ref_rev`~~（2026-09-07 已随 ADR-064 Q2 退役，不在 P5 范围里）
  归入摄入形态的 UI 便利，读透形态不跑；
  `expand_live_reverse_cascade` 的 `fetch_ref_rev_edges` 换成文件表并集，或整个执行器随 `CascadeExpand` 种子一起被 P2/P3 取代。
- `docs/2026-09-04_model-cascade-update-inventory.md` #4 改「✅ 闭环」，#5 补「core 无表（kind 4），与我们同口径」。
- **验收**：默认形态（`spawned-mem` + `read-through` + direct）下改一个 SCOM 参数的 CATA 窗口，只有引用它的根重算——
  **live**，不只靠 `mem://`。

> **落地（2026-09-07，部分）**：ADR-063 已写
> （`docs/adr/ADR-063-ref-rev-is-a-ui-convenience-not-a-correctness-dependency.md`），D1 / D2 / D3 已实施：
> 默认窗口规划器全文无反查（源码钉）；`maintain_reverse_index` 与补偿入队在读透形态整段跳过；
> **生产侧反查入口 `fetch_ref_rev_edges` 在读透形态直接报错**——这一条是 D2 的安全前提，不维护之后
> 空集与「真的没有引用者」长得一样，静默返回就是漏算，报错让调用方走既有的 `cascade_deferred` 退化路。
> 源码钉 `ref_rev_is_a_ui_convenience_and_never_answers_root_selection` 把三条一起盯住；全库 1751 passed。
>
> **没动**：`resolve_unit_rollup` / `direct_root_allowed` / `expand_live_reverse_cascade` 三个读点仍读
> `ref_rev`——换成文件表并集是 P4 那条通用一跳的活，那条还没接，先删读点等于把能力从老路拿掉而新路
> 还没有。inventory #4 / #5 的改口径、以及上面那条 live 验收，一并留到 P4 落地之后。
>
> **补记（2026-09-07，P4 落地之后）**：那三个读点**仍然没动**，理由变了——P4 的通用一跳已经在原位，但它
> 的**行为**只有 `POSREF` 一条（P0 差表：设计→设计、e3d-model 几何真读的就这一条）。老规划器那三个读点
> 走的是 `ref_rev` 的全量边（603 个名字的资格），拿 `POSREF` 一条去换是**收窄**，不是等价替换。要换得先
> 回答「老路那些边里今天还有哪些在影响几何」——那是 P5 收口时该做的一次逐条对拍，不是 P4 的活。

合计 **约 2 周**；P0–P3 是主线（把 #4 补上），P4–P5 是把新路与老路合成一条。

## 4. 需要拍板

| # | 问题 | 推荐 | 备选 |
|---|---|---|---|
| Q1 | `ref_rev` 的去向 | **降级为 UI 便利**：摄入形态照旧维护，读透形态不维护，选根代码禁读（P5） | 保留双源、规划时以 `ref_rev` 为主文件表兜底（多一份事实要对齐，不推荐）；立刻整表退役（UI 目录属性要另找源，先不动） |
| Q2 | direct 下 CATA 变更的触发源 | **文件会话前进 → `collect_window`**（ADR-058 D2 原文），走 `side_effect_pending` 通道 | 只在按需 `ensure` 撞指纹时反查（懒；启动后第一次点根前 UI 看到的是旧几何） |
| Q3 | CATA 侧闭包的深度 | **不设上限**（目录库内的引用链短，SCOM←SPCO←设计 两跳到头；集合不增长即停） | 固定两跳（漏 GMSE←SCOM←SPCO 三跳的形状） |
| Q4 | P4 的行为 | **只加护栏 + 入口**，不改行为（差表证实今天没有跨单元设计引用输入） | 现在就把 kind-16 设计引用者并进 `touched`（多算，但今天没有验收窗口能判别） |

> Q1–Q4 **2026-09-06 已按推荐项拍板**（共识 d-635）。Q4 经 P0 差表修正：**`POSREF` 一条例外**，其引用者并进
> `touched`（见 P0 落地段）；其余仍是只加入口与护栏。

## 5. 不做什么

- 不复刻 core 的 UDA 依赖递归与 `XRPNTR` 正向扩散（ADR-061）。
- 不给 kind 4 的连接类引用（`HREF`/`TREF`/`CREF`）造反向表——core 没有，我们的几何也不读跨单元（09-04 #5，P0 钉成护栏）。
- 不动 `ModelTarget.catalogue` 指纹：它是「这根照哪版目录算的」的记录，反查是选根，两件事不合并（09-04 计划 §4 原文）。
- 不在本计划里碰 #3（改挂新旧两根）——那是 09-02 计划 P1-1 的 ledger `Reparented` 补排。

## 6. 风险与退路

| 风险 | 处置 |
|---|---|
| 文件表覆盖 < `ref_rev`，而差集里某个属性确实是几何输入 | P0 差表就是查这个；查出来就对那几个属性保留 `ref_rev` 边（混合），其余走表 |
| CATA 侧闭包在大目录库上慢 | 点查是 O(树高 + 命中)，`ams7351` 334 万候选正是选文件表的理由；P1 成本门量它 |
| 反查把范围收窄 → 陈旧几何 | 护栏不对称：判不了一律全部过期；主门 `ams5052_0001 184→185` + 对拍门 P4 |
| e3d-io 工作树被别的会话改着 | P1 第一步先 `git status`，只做加法；改前看行尾（这棵树 CRLF/LF 混用，`include_str!` 护栏会因此变红） |
| G5 先于本计划落地 → 默认形态下 `ref_rev` 无人维护、CATA 级联静默退化为全库过期 | 已经是 ADR-058 实施约束 5 允许的「已知降级」且回执点名；本计划 P2/P3 是解，两边排期要对齐 |

## 7. 不许当已证

- 「e3d-model 不读任何跨单元设计引用」今天只有 grep（`"NGMR"` 零命中、`VXREF` 单元内、`HPOS`/`TPOS` 存储值），P0 才把它钉成测试。
- 「表在就查」假定 kind-16 表名 = 引用属性名——写侧 `sub_5AFDF10(attId, …)` 坐实了表名 = attId，但**没有**验证每张
  `DbView::tables()` 列出的表都是引用表（e3d-io 模块注释已记 `TYPEX` 同形不同义；`ReferenceTable::decode` 会拒收
  `value_dwords != 0` 的表，拒收即跳过，不会误读）。
- 09-04 §2.10 的 31 个小窗口只数了候选，没看那些候选是不是设计侧真会引用的目录件——P2 的验收才回答。
- 本计划没有跑任何一条联动的实测；所有「有 / 没有」都是读码结论。
