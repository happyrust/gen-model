# 删除窗口不再留模型孤儿：开发计划（2026-09-18 改小重立版——只留覆盖账告警）

> 来源：2026-09-15 ADR-076 S5 真进程实测（`docs/evidence/2026-09-15-adr076-increment-durable-boundary.md` §7）。
> 现场：AMS 8000 上 `DELETE EQUI`（EQUI + 1 个 BOX）+ SAVEWORK，窗口报 `deleted=2`、`applied`、水位推到 T、标记 `finished`——
> 数据面对（两行都成了 `deleted = true` 的墓碑，活行回到基线 6,637），**模型面留下四行孤儿**：
> `inst_relate:24384_26481`、`inst_info:24384_26481`、`aabb:24384_26481`、`trans:24384_26481`，`model_sesno` 停在删除前那一会话。
> 被测二进制 `8adec7169`；成因与 ADR-079 之后的 live 复验见下节。
> 状态：**待批准（2026-09-18 改小重立版）**。用户 09-18 13:3x 拍板：「按更正后的口径把 09-15 计划改小重立：只留 D3 覆盖账
> （delete 桶为空而账本 remove 非空时出 warning），把 D1 / D2 划掉」。09-15 版的 §1.1–1.3 / D1 / D2 / S0–S3 已划掉，
> 原文见 `git show 13695dd3d:docs/plans/2026-09-15-delete-window-model-orphans-dev-plan.md`。本版代码事实对到 **`13695dd3d`**（09-18 HEAD）。

## 成因更正（2026-09-18）

起因：09-18 对 8000 / 7999 / 7997 / 1112 四库 24 987 行存量模型记录逐行对现生成（plant3d-web
`docs/verification/component-to-wall-surface-clearance-2026-09-17/README.md` §7.8），`in.deleted = true` 的模型行**全库只剩**
`inst_relate:24384_26481` 这一条（8000 里 6 643 个已删元素其余都干净，7999 / 7997 / 1112 各 0）。顺着它把 603→604 那一窗按
`8adec7169` 源码 + 窗口日志重新对了一遍，09-15 版 §1 的机制对不上：

1. **`DeleteCleanup` 根本没排。** DESI 库的模型计划走 `build_effective_model_update_plan` → `window_root_plan.rs::build_model_update_plan_from_window`
   （`8adec7169` 当时 `:1129–1130`），`DeleteCleanup` 只从 `plan_window_roots` 的 `delete = roots_S \ roots_T` 那一桶来（模块头表格）。
   `8adec7169` 的 `roots_S` 是 `load_persisted_roots_with_settlement(dbnum)` 一次 `gen_root` 表读（`:1012`），而增量新建的根从头到尾没进过
   `gen_root`（证据 §6.2：始终 952 条，`gen_root:24384_26480` 不存在），于是 `delete = ∅`——窗口日志原文
   `roots_T=952 roots_S=952 touched=0 regen=0 delete=0`。`delete_model_subtree` 一次都没被调，没有失败可报，任务 `applied` / `finished`。
   09-15 版 §1.1 图里「规划 → `DeleteCleanup` → `delete_model_subtree`」那一支在这一窗上**不存在**。
2. **证据 §7 第 1 条把 `units regen=0 remove=1` 读成「排了 `DeleteCleanup`」，读错了。** 那一句是变更账本（e3d-model `UpdatePlan` / `IncrementReport`）
   的 `totals_line`，`remove=1` 数的是候选里「target 端不存在」的删除单元（`UpdatePlan.remove`），跟根规划的 `delete` 桶是两个数。
   同一窗在今天 HEAD 上重放，账本行仍是 `candidates=3 (+0 ~1 -2) … units regen=0 remove=1`，而根规划的 `delete` 从 0 变成 1（见第 4 条）。
   **这两个数并排躺在同一行日志里、没人比它们——这就是本版要补的那一条。**
3. **09-15 版 §1.2 说的「`topmost_deleted_refnos` 收敛 + `pe_owner` 子树走不下去」不在这一窗的路上。** 那是老规划器
   `model_update_plan.rs::build_model_update_plan`，`build_effective_model_update_plan` 里只给 CATA 用（DESI → 窗口规划器，CATA → 老规划器，
   其余空计划；`8adec7169` 与今天 HEAD 同样）。而且就算 `DeleteCleanup(EQUI 26480)` 排上了，ingest 档 `on_demand_model.rs::delete_model_subtree`
   除了 `collect_pe_subtree_refnos` / `delete_inst_relate_subtree` 那一路，还对每个 lock root 执行 `render_spooled_scope_deletion(root, &[root], &[])`
   ——按 `anc CONTAINS packed(root) AND model_sesno != NONE` 删，**不依赖 `pe_owner` 边**（这两处调用 09-07 / 09-09 就在，`8adec7169` 已有）。
   那句谓词在 8009 上只读重放：`by_anc = ["24384_26481"]`，命中孤儿。边断了也删得掉——§1.2「防住了墓碑，没防住边」描述的边确实断了
   （两个方向实测都空），但它只影响 `delete_inst_relate_subtree` 那一路，而那一路在增量上只是兜底。
4. **ADR-079（`0247da413`，09-15 23:53，S5 实测当晚）已把成因关掉；2026-09-18 13:21 live 复验。** `gen_root` 退役后 `roots_S` = `DbSet@S` 枚举
   （`build_model_update_plan_from_window`「`roots_S` 来自 `DbSet@S`」），增量新建的根自然在里面。在 HEAD `13695dd3d` 上跑探针
   `probe_transform_classification_for_window`（`AIOS_TRANSFORM_WINDOW=603:604`、`AIOS_MODEL_TRANSFORM_CHEAP_PATH=1`、
   `AIOS_E3D_SCHEMA_ROOT=vendor\e3d-io\schema`，只读 `ams8000_0001`，`cargo test --lib … -- --ignored --nocapture`，编 25 s、跑 0.5 s）：
   `roots_S=953 roots_T=952 touched=1 regen=0 delete=1 advance=952 lazy=0 degraded=None`；候选表 `24384/26480 EQUI Deleted: target 端不存在`、
   `24384/26481 BOX Deleted: target 端不存在`、`24384/24775 ZONE MembersChanged 成员 +0 -1`。同一窗、同一份文件，`8adec7169` 是
   `roots_S=952 … delete=0`；多出来的那一根就是 EQUI `26480`，落 `delete` 桶 → `DeleteCleanup(26480)` → 第 3 条的 anc 墓碑清掉 BOX 那几行。
5. **残留已清。** 8000 水位 `applied_sesno = model_sesno = 619`，603→604 那窗不会重排，孤儿不会自愈。2026-09-18 12:2x 走 `:8022` 的
   `DELETE /api/v1/model/subtree?refno=24384/26481&confirm=24384/26481` → 200 `{"scope":"exact_subtree","status":"deleted"}`；
   `inst_relate` / `inst_info` / `aabb` / `trans:24384_26481` 与 `geo_relate:[inst_info:24384_26481, …]` 五行都没了，8000 `inst_relate` 2 529 → 2 528，
   `pe` 两行墓碑照旧。删前原样备份在 plant3d-web `…/8022-swap-2026-09-18/orphan-24384_26481-backup-before-delete.json`。

**对 09-15 版的影响**（已按此改小，见下文）：D1（全集排）/ D2（一窗一批）针对的是一个在增量路上走不到的断点，作为「修 S5 孤儿」**不成立**；
仍站得住的只有 D3 的覆盖账——S5 那一窗如果有「账本 `remove` 非空、根规划 `delete` 桶为空」的告警，孤儿当场就会被看见，而不是三天后靠全库对账翻出来。
09-15 版 §1.4 / §5「可能同源的另一条」与本条**就是同一件事**，且已随 ADR-079 关掉。「新建的根不在 `roots_S`」这一类漏洞已经从结构上不存在；
剩下的删除形态是**成员删了、根还在** → `regen`（整根重算，旧行被根级墓碑清掉），`live_ams8000_pinned_windows_land_in_the_expected_buckets`
的 269→270「删 FTUB → regen 1」钉着它，四库对账 0 孤儿也印证它。

## 0. 一句话

**账本说要删的模型单元，自身或它 S 端属主链上必须有一级落在 `delete` 或 `regen` 桶里；都没有，就在这一窗的回执里点名出声。**
不动选根、不动 `DeleteCleanup` / `RegenRoot` 的执行路径、不动数据面墓碑口径；`collect` 里多取一条 S 端属主链，分桶后一次纯比对 + 一行 warning。

> 2026-09-18 14:1x 落地时更正了一个前提：e3d-model `UpdatePlan.remove` 记的是**被删的模型单元**（几何挂在它身上的那一级——
> BOX / CYLI / FTUB…），**不是**生成根。S5 那窗 `remove = {BOX 24384/26481}`，不是 `{EQUI 26480}`；24→26 是 `{BOX 24779}`，
> 270→271 是 8 个成员。所以「`remove` 与 `delete` 直接比」比不上，「`delete` 为空而 `remove` 非空就告警」也会在 269→270
> （删 FTUB、BRAN 整根 `regen`）上误报。改成按 S 端属主链找「谁来清它的旧行」：`delete` 里的根按 `anc` 清整根，`regen` 里的根
> 整根重算、旧行随根级墓碑退掉——两者都算盖住。

## 1. 现状（代码事实，`13695dd3d`）

### 1.1 同一窗里有两份「删了什么」，并排打出来，没人比

```text
build_model_update_plan_from_window（window_root_plan.rs:1050，DESI 增量的唯一模型规划入口）
  ├─ WindowRootSources::collect（:774）→ sources.plan : e3d_model::UpdatePlan      变更账本（L3）
  │       .remove : BTreeSet<(u32, u32)>   被删的模型单元（S5：{BOX 24384/26481}；24→26：{BOX 24779}；270→271：8 个成员）
  │       .report.units_to_remove          同一个数；totals_line（vendor/e3d-model/src/increment.rs:413）里的 `remove=`
  └─ plan_window_roots(roots_T, roots_S, impact)（:212，纯函数）→ plan.delete : Vec<PersistedRoot>   = roots_S \ roots_T
          → into_model_update_plan（:173）→ work_items 里的 DeleteCleanup 只从这一桶来
          → warnings[0] = "dbnum=…: window S→T roots_T=… roots_S=… touched=… regen=… delete=… advance=… lazy=… | <totals_line>"（:1112–1123）
```

S5 那一窗（`8adec7169`）就是 `delete=0` 与 `remove=1` 并排躺在 `warnings[0]` 同一行里，三天没人看见。

### 1.2 warnings 去哪儿

`plan.warnings` → `increment_pipeline.rs:981 warnings.extend(model_plan.warnings.iter().cloned())`（任务路）/
`manual_update.rs:4407`（预览路）→ 任务回执 `warnings`，同一行也进服务日志。本条不用新开通道。

### 1.3 旧行是谁清的（本版不动）

- `DeleteCleanup`：`batch_worker.rs::run_model_side_actions`（:2762）在规划出来那一刻就地执行 → `mgr.delete_model_subtree(refno)`
  （`on_demand_model.rs:62`）→ ingest 档对每个 lock root：`delete_persisted_geometry_root` + `render_spooled_scope_deletion(root, &[root], &[])`
  （anc 墓碑，不靠 `pe_owner` 边）→ `delete_inst_relate_subtree(&[refno], 300)` → `forget_root`。边断不断都删得掉（成因更正第 3 条）。
- `RegenRoot`：整根重算，发布时按根清旧行再落新行——成员删了、根还在，旧成员的行随之退掉。四库对账 8000 里 6 643 个已删元素
  0 孤儿（S5 那条之外）就是这条路的实证。

### 1.4 为什么账本 `remove` 里的每一件都该被这两桶之一盖住

一件模型单元 u 在 S 端存在、T 端不存在，它的 S 端属主链上必有一个生成根 R（`unit_types` 判的，`roots_S` 用同一份口径枚举）。
R 在 T 端还在 → u 的删除让 R 进闭包 → R ∈ `touched ∩ roots_T` = `regen`；R 在 T 端也没了 → R ∈ `roots_S \ roots_T` = `delete`；
闭包不完整 / 退化 → `roots_S ∩ roots_T` 全进 `regen`，`delete` 照算。所以**正常情况下差集恒空**。非空只有两种可能：

- **`roots_S` 漏了 R**——S5 型：`gen_root` 里没有增量新建的 EQUI，R 既不在 `delete` 也不在 `regen`（`roots_T` 里也没有）。
  结构上已随 ADR-079 消失，但这正是要防的那一类：`roots_S` 的来源以后再换一次，这条告警是唯一会当场叫的东西；
- **属主链 / 闭包读挂**——链读不出来按空链记，那一件会被判成没盖住而出声（宁可多叫一次）。

## 2. 决策

- ~~**D1 计划侧交全集**~~——**划掉**（2026-09-18）。它针对的 `topmost_deleted_refnos` 只在 CATA 老规划器里，不在 DESI 增量路上；
  DESI 的 `DeleteCleanup` 本来就按根排、一根一条。
- ~~**D2 执行侧一窗一批**~~——**划掉**（2026-09-18）。往返次数不是问题所在；`delete_inst_relate_subtree` 已按 300 分块。
- **D3 覆盖账（唯一保留，口径 = 规划期按 S 端属主链比对）**：
  - `collect` 里多取一份 `removed_units: Vec<RemovedUnit { refno, noun, ancestors_s }>`——账本 `remove` 的每一件，noun 取账本 `Deleted`
    条目，属主链沿 `DbSet@S` 上溯（现成的 `owner_chain_in_set`，不含自身、最近属主在前）。两端 `DbSet` 都在手，之后不再开文件。
  - 分桶（含 `carry_unsettled_generation`）之后纯函数 `uncovered_removed_units(&removed_units, &plan.delete, &plan.regen)`：
    自身或链上任一级 ∈ `delete` ∪ `regen` 即盖住；差集按账本键序。
  - 非空就 push 一条 warning（`delete_coverage_gap_warning`）：

    ```text
    dbnum={dbnum}: delete-coverage gap — the change ledger removes {n} model unit(s) that no DeleteCleanup root and no RegenRoot covers:
    24384/26481 BOX, …; their model rows are not cleaned by this window (roots_S={} roots_T={} delete={} regen={} remove={})
    ```

    点名最多 12 件，带账本 noun。用户 09-18 的口径「`delete` 桶为空而账本 `remove` 非空时出 warning」在这里体现为：`delete` 与 `regen`
    都空而 `remove` 非空 → 差集 = 全集，全部点名；`regen` 非空时按链判（269→270 不误报）。
- **D4 只告警、不判 `Partial`、不进 `/health`**（承 09-15 版 Q2 / Q3 的推荐）：模型行残留不影响数据面正确性；它是单窗的账，不是常驻状态。
- **D5 观测面顺手带上**：`probe_transform_classification_for_window` 的分桶行加 `remove= gap=`（非空时点名）；
  `live_ams8000_pinned_windows_land_in_the_expected_buckets` 每窗打 `remove= gap=` 并断言 gap 为空。

## 3. 切片（S0–S2 已落地，2026-09-18 14:1x）

| # | 内容 | 落点 | 验收 | 状态 |
| --- | --- | --- | --- | --- |
| S0 | **纯函数 + 用例**：`RemovedUnit` / `uncovered_removed_units` / `delete_coverage_gap_warning`；S5 的数（`remove = {BOX 26481, 链 EQUI 26480 → ZONE → SITE}`、`delete = []`、`regen = []`）→ 点名 `24384/26481 BOX`；EQUI 进 `delete` 或 BRAN 进 `regen` → 空 | `window_root_plan.rs` | §4.1 前两条 | **done** |
| S1 | **接线**：`collect` 里 `collect_removed_units(&plan, base)`；`build_model_update_plan_from_window` 里比对 + push warning；源码钉 | `window_root_plan.rs` | 源码钉用例绿；`cargo test --lib` 不退步 | **done** |
| S2 | **观测**：探针与真库门加 `remove= gap=`，真库门每窗 `assert!(gap.is_empty())` | `window_root_plan.rs`（tests） | 603→604 → `delete=1 remove=1 gap=0`；九个钉死窗口全绿 | **done** |
| S3（可选） | **真库一建一删**（09-15 版 §4.3 的五个计数）：E3D TTY 建 EQUI + BOX → 删 → 回执无 gap warning、五个计数回基线 | — | 需要 E3D TTY，按 09-15 S5 的 mac 改 | 未做 |

## 4. 验证与验收

### 4.1 单测（名字即判据）

- `a_removed_unit_with_no_cleanup_root_above_it_is_named`
- `a_removed_unit_under_a_delete_or_regen_root_raises_no_gap`
- `the_window_planner_reports_delete_coverage_gaps`（源码钉：比对排在 `plan_window_roots` 与 `carry_unsettled_generation` 之后、
  `into_model_update_plan` 之前；属主链在 `collect` 里取）

### 4.2 回归（2026-09-18 14:1x 实跑）

- 主工作树里 `vendor/e3d-model` 正被另一条会话改到编不过（`pipeline.rs` / `route.rs` 每几分钟一变），本轮在 `%TEMP%\gm-iso`
  里用 **e3d-model `32bfebc`（HEAD）+ gen-model-model-cache HEAD + 本文件** 单独编（其余 vendor 与 `plant-ui` 用 junction 指回原处，
  `CARGO_TARGET_DIR` 共用 `D:\Rust\target`）。
- `cargo test --lib`：**2 290 passed / 5 failed / 119 ignored**；同一棵树、换回 HEAD 原文件：**2 287 / 5 / 119**——多出的 3 条就是
  §4.1，失败的 5 条两边同一批、都不在本文件改动的路上（`model_impact` × 2 与本文件 × 2 是 `CACHID` 元数据档的口径变了——
  `metadata_only_tier` 现在把只戳 `CACHID` 判成 `CacheStamp` 档、用例还按旧口径断言，另一条会话手里 `model_impact.rs` 在途；
  `web_service::review::platform` × 1 与模型面无关）。
- 九个钉死窗口 + 603→604 探针：gap 全 0——`603→604: roots_S=953 roots_T=952 touched=1 regen=0 delete=1 … remove=1 gap=0`；
  `24→26 删 EQUI 子树: delete=[24778 EQUI] remove=1 gap=[]`、`269→270 删 FTUB 4: regen=[26261 BRAN] remove=1 gap=[]`、
  `270→271 删整条支管: delete=[26261 BRAN] remove=8 gap=[]`，其余六窗 `remove=0`。要看 warning 真的打出来，只有单测那条 S5 数字
  ——真库上这一类已经不再发生，这是好事，不是验收缺口。
- 09-15 版 §4.2 那两条口径照旧：`direct_window_replay_parity.rs` 的墓碑用例不变（不碰数据面）；按需删除入口 `DELETE /api/v1/model/subtree` 行为逐字不变。

### 4.3 真库（S3，可选）

同 09-15 版 §4.3 表；今天 8000 的基线：`pe` 活行 6,637、`inst_relate` **2,528**（09-18 清孤儿后），`inst_info` / `aabb` / `trans` 不留被删元素的行。

## 5. 不做（本轮）

- **执行侧逐行覆盖账**（`DeleteCleanup` 到底清了多少行）：要改 `render_spooled_scope_cleanup` 的 `RETURN` 并把计数穿三层返回值；
  anc 墓碑不靠边，这一层今天没有已知漏洞。有下一条实例再立。
- 选根、`DeleteCleanup` 执行路径、数据面墓碑口径、`noun` 名词表行的已知残留（`direct_window_replay_parity.rs:449` 指的那条）、投影淘汰。

## 6. 风险与取舍

- **假阳性**：只在 `roots_S` 漏根或属主链读挂时出声；`regen` 算盖住是为了「删成员、根还在」这一最常见的删除形态不误报。
  若哪天出现「链上有根、根却既没 regen 也没 delete」之外的新形态，先看是不是 `unit_types` 口径分叉。
- **退化窗**：`plan.degraded` 时 `delete` 桶照算（`roots_S \ roots_T`）、`roots_S ∩ roots_T` 全进 `regen`，比对照做、只会更容易盖住。
- **成本**：`remove` 条数 × 一次 S 端属主链（几次 owner 读，删除窗口通常个位数到几十件）+ 一次 `BTreeSet` 查找 × 链长；
  不进热路径、不加序列化字段。

## 7. 待定 → 已定

- ~~**Q1**：差集里的单元若正好落在某个 `regen` 根名下要不要免告警？~~ **已定为「算盖住」**（就是「删成员、根还在」的常态，
  269→270 钉着）。
- ~~**Q2**：warning 要不要点到 noun？~~ **已点**：noun 取账本 `Deleted` 条目（`RemovedUnit.noun`），账本没记的只出 refno。
