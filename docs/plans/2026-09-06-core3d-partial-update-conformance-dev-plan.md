# 开发计划：Core3D 局部更新核对表收口（R1–R29 之后还剩什么、按什么顺序做）

> 日期：2026-09-06
> 依据：`docs/specs/core3d-partial-update-conformance.md`（2026-09-04 版，R1–R29 + §8/§9）、
> `docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md`、
> `docs/plans/2026-08-27-model-impact-core3d-parity-plan.md`（五次修订版，本计划**接替**它的 §3 顺序表）、
> `docs/specs/core3d-partial-update-test-cases.md`（C0-1 … C4-3）、ADR-058、ADR-060
> 方法：先对现码逐条核实核对表的「我方」栏（§1），再排任务；草稿送 Oracle（`@steipete/oracle` 0.18.0，
> ChatGPT Pro）审阅（§8），再走 Plannotator 人工评审（§9）。
> 涉及：`src/options.rs`、`src/data_interface/{model_impact,generation_root,model_update_plan,manual_update,
> model_update_pending,update_scope,window_root_plan}.rs`、`src/fast_model/model_db_adapter.rs`、`tests/staged_regen_e2e.rs`
>
> **2026-09-06 晚 · Oracle 审阅后修订。** Oracle（GPT-5.5 Pro，会话 `core3d-conformanc-dev-plan-3`）的意见
> 逐条落在 §8；采纳的已经改进正文，逐处标 `★O`。同时把 §1 按 20:48 的工作树重核了一遍——
> 白天几条活（T2b 接线、R28、W1.3、W2.1）已被别的会话做掉，状态跟着改。

## 0. 一句话

核对表 §9 的「错」档已经空了。剩下的活分五类。★O **不能说「没有一类是缺了会让模型错」**：
W1.2 关的是 P1 判出的唯一真漏——`AIDTEX` 直接挂 SITE/ZONE/WORL 下时一个工作项都不产生
（08-27 计划 §2/P1），今天开关已接线但**默认关**。**22:30 收窄**：这条漏只在**旧 PE 路径**（兼容数据模式）上成立；
direct 文件窗口按 `noun_is_significant` 枚举，AIDTEX 本来就是根（W1.2 测试钉住）。其余四类才是「慢 / 判据 / 文档 / 取证」。

| 类 | 内容 | 本计划编号 |
|---|---|---|
| 文档失真 | 核对表有两处与现码相反 / 引用不存在的测试；用例集 C2-3 与 R19/R24 冲突 | W0 |
| 做好了没接 / 没开 | R2 门默认关（direct 路径只观察）、T2b 开关已接线但契约测试缺、R22 已在 testbed 验过 | W1 |
| 判据决策 | T2c 第二批怎么开、「视图切换」在我们这边是什么、P5 的定性、R12/R14 的深度上限 | W2 |
| 慢 | ~~R28 空生成（已完成）~~、T3.3 祖先抢占去重 | W3 |
| 新缺口取证 | `DDES_GraphicsMngr` / GETWORK 通道、`toplevel` 位交叉验证 | W4 |

原 08-27 计划的 §3 顺序表到此作废，以本文 §4 为准。

## 1. 现状核对（2026-09-06 对现码，不信文档只信 grep）

核对表每一条「我方」栏都重新在工作树里找过落点。白天那版：**两处对不上，一处引用悬空**，其余成立。
20:48 重核：R9 / R28 两行的「现码事实」变了（别的会话已落地），R12/R14 一行是 Oracle 提醒后补的。

| 规则 | 核对表说 | 现码事实 | 判词 |
|---|---|---|---|
| R2 XGEOM 门 | 已接线，默认关，可灰度 | `options.rs:1242 model_xgeometry_gate()` 默认 false；消费点 `manual_update.rs:4682`、`model_update_plan.rs:930`（**都在旧 PE 路径**）；两条单测在 `manual_update.rs:7054/7091`；direct 文件窗口只做双端**观察**出 warning（`window_root_plan.rs:320 xgeometry_gate_observed`），不筛工作项；真库普查 486 库 **0 实例**（`docs/evidence/2026-09-04-xgeom-census.json`） | ✅ 成立。~~可直接默认开~~ ★O 改判：**先正样本，再谈默认**（W1.1）——零实例只证语料不触发，且 direct 路径翻默认值是空操作 |
| R9 T2b | 「`model_significant_over_point` 已接入旧选根路径；默认关」 | **20:48 重核：已接线。** 消费点 `generation_root.rs:372`（`model_significant_over_point() && is_loop_container_noun && noun_is_significant` → 自身为 Normal 根），测试 `significant_over_point_only_promotes_aidtex_when_enabled`（`generation_root.rs:1418`，两态各一断言）。**仍缺**：C1-1/C1-2/C1-3 一条没写；「44 个 point noun 逐个跑、开关两态恰好只差 `AIDTEX`」那条没写；`options.rs:1263` 注释引用的 `the_significant_over_point_override_only_moves_aidtex` **不存在**（实际叫上面那个名字）；`model_impact.rs:1523 every_dictionary_point_container_is_skipped_as_a_generation_root` 仍是「44 个 point noun 全部 loop container」口径。★O 另一个问题：接线用的是 `noun_is_significant`（快照外**保守取真**），开关开着时一个快照外的 point noun 也会被抬成根——这改变了 C1-3「未知 noun 走旧兜底」的边界 | ◐ 接线成立、契约测试缺、判据接口要换（W1.2 + W0.2） |
| R12 / R14 上卷与 `SignificantOwner` ★O | ◐ 形状相同判据不同：名单、深度上限 32、遇粗层级容器即停 | 旧路径 `generation_root.rs:357 resolve_element_generation_root` 仍是名单 + `MAX_ANCESTOR_DEPTH = 32`；direct 文件窗口按 `noun_is_significant` 枚举根、闭包按祖先封闭（`window_root_plan.rs:1–22`）。**T2b 只翻转 point 那一档，不能据此说 R12/R14 已对齐** | ◐ 成立。两条不变量没人量过：真库 owner 链最深多少、有没有 owner 环（W2.4） |
| R22 缺失图元回收 | 按根对账与两个生产调用点已落地 | 完整性门要求匹配回执；嵌套根、不完整投影、完整空投影均有自动测试；`live_testbed_complete_projection_reclaims_only_the_absent_primitive` 已在 8019 RocksDB 独立临时库验证“无回执不删、完整回执只删孤儿” | ✅ 完成（W1.3） |
| R6 Refresh 清队列 | 已落地；视图切换那条无对应物 | `model_update_pending.rs:114 discard_pending_for_full_rebuild(dbnum)`；`model_rebuild.rs:256` 钉顺序 | ✅ 成立。「什么算切换视图」待定（W2.2） |
| **R1 / P5 CATA 反向级联** | 「闸已开：`update_scope.rs` 现在 `db_type == "CATA" \|\| (DESI && …)`」，注释与代码相反 | **相反。** `update_scope.rs:284 admits` 只认 `DESI \| ISOD` 且须 MDB 声明；`:330` 测试断言 `!scope.admits("CATA", 8000)`；注释（:280–283）与代码一致、引用 ADR-058 D1。ADR-058（2026-09-03 已接受）S3：**CATA 永不进持久层**；D2：CATA 变更的级联触发源改为**对 CATA 文件做会话索引差分** → `ref_rev` → `cascade_expand`。回执字段 `catalogue_cascade_degraded` 已在（`initialization_phase.rs:189`、`model_update_pending.rs:2940`） | ❌ **核对表这一行失真**。P5「开 `admits` 的闸」这条路已被 ADR-058 关掉；剩下的问题是 D2 有没有实施完（W2.3） |
| R17–R21 去重 | T3.3 未做 | `model_update_plan.rs:155` 仍是 `BTreeMap<(ModelWorkAction, String), ModelWorkItem>`，同 refno 才合并 | ✅ 成立（W3.2） |
| R28 目标消失不空跑 | 核对表 §9 已改 ✅：direct 由 `roots_T` 枚举排除，兼容路径保留终态存活复核 | **20:48 重核：成立。** `window_root_plan.rs:15–17`：`regen = touched ∩ roots_T`，`delete = roots_S \ roots_T`（根已删**或还在但不再是根**都归这一格——正是 ★O 提的「不在 `roots_T` ≠ 元素被删」那一档）；三条护栏任一触发即 `degraded`（`:1156`，测试 `guards_degrade_credential_advance_into_regenerating_every_persisted_root`）。未落的只有原用例名 C2-5 `regen_root_is_dropped_when_the_target_disappears` 与回执计数 `skipped_vanished` | ✅ 已完成，W3.1 改为收尾（用例改名对齐 + 读失败路径钉住） |
| R10 块内成员清理 | 不可比；守「记录 id 与根无关」 | 两条护栏在（见 R22 行） | ✅ 成立，本计划不再立项 |
| §8 `toplevel` 位 | 离线字典值未与 live `getField` 对过 | `old-parse-pdms-db::dict::FIELD_TOPLEVEL` 有；快照 `tests/fixtures/core-noun-granularity-e3d31.json` 只有三张表 | ✅ 成立（W4.2） |
| §8 GETWORK 通道 | 从未梳理 | 无任何文档 / 代码引用 `DDES_GraphicsMngr` | ✅ 成立（W4.1） |

## 2. 范围

**做**：上表五类。

**2026-09-06 22:10 用户划定范围：「我只关注 DESI 的三维模型的增量更新绘制。」** 据此：
W2.3（CATA 反向级联 / ADR-058 D2 实施到哪）**移出本计划**——它不是 DESI 模型增量的事，归 ADR-058 的实施计划；
核对表 §9 CATA 行已改成「设计而非缺口」，不再指望本计划回填 R1 注记。W4.2（`toplevel` 位）是显示属性层的账，
不阻塞任何 DESI 增量项，降为最后。其余各项都在范围内。

**不做（沿用 08-27 计划 §1 的理由，加三条新的）**：

- **不抄 R23 的擦除分支**——ID 清单不可用时 core 把整棵子树的 primitive 判缺失并擦掉刚画好的东西；
  我们把「对账源不可用」当**前置条件失败**（不回收、出声）。W1.3 要顺带核这一条真的是这么写的。
- **不抄 R7 / R8 / R27 / R29**（ID 清单落库、`m_inDraw` 重入、三遍消费、`Finish`）——它们是 DRAFT 图纸
  视图语境的产物（证据 §4），我们没有 VIEW 也没有 PGP。
- **T3.2 维持撤销**——R10 在我们这边结构上不可能（ADR-060 D1/N1）。谁要把根塞回记录 id，先过那两条护栏。
- 不取消 `TransformOnly`、不取消 `Unknown → Regen` 兜底、不做负实体上卷、不改重画执行、
  不碰 `PostSetRefListAttribute` back-ref——理由见 08-27 计划 §1。

## 3. 任务

每项四栏：改哪、怎么改、怎么验、怎么退。**S/M/L 是相对大小，不是工期承诺。**

### W0 —— 文档失真修正（S，纯文档，先做）✅ 已完成（2026-09-06 22:05）

落点：核对表 §9 CATA 行 / R9 行 / R6 行、R9 正文、R6 正文（新增三候选判词表）、§1.5；`options.rs:1263` 注释；
用例集 C2-3 全文重写、C0-3 与 C0-1 预告、§0 「消费者不存在」一段、§7 R15 行。
顺带发现：§1 那行引用的核对表 CATA 旧文（「闸已开…」）在 20:26 之前已被别的会话改成「架构变更 ⚪」版，
W0.1 是在那版上再收口（补 R1 编号、`update_scope.rs:291/330/280` 落点、D2 链与 W2.3 指针）。

| # | 改哪 | 怎么改 |
|---|---|---|
| W0.1 | 核对表 §9 最后一行（CATA） | 改成 ADR-058 口径：`admits` **不放行** CATA 是设计而非缺口；CATA 级联走 D2（文件侧差分 → `ref_rev` → `cascade_expand`）；「注释待修」那半句删掉（注释现在是对的） |
| W0.2 | 核对表 §9 R9 行、`options.rs:1263` 注释 | 注释里的 `the_significant_over_point_override_only_moves_aidtex` 改成实际存在的 `significant_over_point_only_promotes_aidtex_when_enabled`；「44 个 point noun 恰好只差 AIDTEX」那条标成**待写**（W1.2 交付物），不要让文档引用一条不存在的测试 |
| W0.3 | 核对表 R6、§1.5 | 补一句「视图切换的我方定义见 W2.2 决策」，把三个候选（切 MDB / 改 watch scope / 切投影域）与 W2.2 的判词写进去 |
| W0.4 ★O | 用例集 C2-3 | **期望写反了。** C2-3 要求「NOZZ 的删除被 EQUI 的祖先标记吸收」，但 R19 `IsPending(e, Deleted)` 只看 key 自身的 state 3/4、**不上行**，R24 明说 state 2 只有 `IsPending(·, Changed)` 一个读者——core 会把 `(EQUI,3)` 与 `(NOZZ,3)` 两条都留在队列里。改写成「**生产计划的删行效果等价**」：两条 `DeleteCleanup` 与一条的最终模型行集合相同；不再拿它当 core 契约 |
| W0.5 ★O | 用例集 C0-3 | 「快照 `fields` 下**恰好**三个键」这条与 W4.2 要加的第四张表 `toplevel` 冲突。先在 C0-3 注一句「W4.2 落地时同一次改成恰好四个键」，别让两份文档各说各话 |

- **验收**：核对表每一条「我方」栏的落点都能在工作树 grep 到（§1 那张表就是验收脚本的雏形）；
  用例集里每条「期望」都能在核对表里找到支持它的规则编号（C2-3 今天找不到）。
- **退**：文档，无。

### W1 —— 接线与开闸（每项单独可上线、单独可回滚）

**W1.1 R2 门：先造正样本，再谈默认值（S 取证 + S 改默认）** ★O 改判

原稿建议「直接默认开」，理由是 486 库 0 实例、开关两态逐字相同。Oracle 的反对成立，而且 20:48 重核又多出一条：

- **零样本只证明这批语料不触发，不证明门是对的。** 两条单测覆盖四种两态组合，但从没有一个真实 XGEOMETRY 子树
  走过预览 → 执行 → 落库这整条链。warning 出声不是验收。
- **direct 路径上翻默认值是空操作。** `model_xgeometry_gate()` 的消费点只有旧 PE 规划器
  （`model_update_plan.rs:930`）与 `manual_update.rs:4682`；direct 文件窗口（今天 DESI 的生产路径）只做双端**观察**
  （`window_root_plan.rs:320 xgeometry_gate_observed` → warning），**不筛工作项**。默认开只改旧路径的行为。

改成两步：

1. **正样本（取证，不改码）**：在 E3D 里造一棵 XGEOMETRY 子树（挂几个 BOX），做四件事各存一个窗口——
   子树内新建、子树内修改、搬出、搬进。跑 direct 与旧路径两遍，记：观察 warning 的 `xgeometry_gated` 名单、
   预览与执行的工作项是否一致、显式几何那一侧到底由谁更新。产物进 `docs/evidence/`。
2. **默认值（改码，S）**：正样本过了才改 `options.rs:1242 model_xgeometry_gate()` 的默认；同时决定 direct 路径
   要不要从「观察」升成「筛」（这是另一处代码，不是翻默认值）。开关与环境变量 `AIOS_MODEL_XGEOMETRY_GATE` 保留。

- 验（第 2 步）：`the_xgeometry_gate_*` ×2 仍绿；`options.rs:1977` 那组默认值断言改口径；正样本四个窗口
  在 direct 与旧路径上工作项集合一致；`noun_census_probe --nouns XGEOM` 复跑仍为 0（语料没变）。
- 退：配置或环境变量关掉，行为回到今天。
- **不阻塞任何别的项**（Oracle A）。

**W1.2 T2b 收口：把接好的线钉成契约，并把判据接口换对（M）** ★O · **W0 之后第一个做** ✅ 已完成（2026-09-06 22:30）

落点：`generation_root.rs` 新增 `significant_point_container_is_its_own_root(noun, significant_bit)`，
`resolve_element_generation_root` 在 MDU 判定之后调用它，判据 `core_significant_bit(noun) == Some(true)`
（`significant_bit` 作参数只为让 C1-3 构造「快照外」那一态）。测试五条新增 + 一条改口径，全绿：
`significant_bit_resolves_to_itself_and_to_nearest_significant_owner` / `delivery_unit_outranks_the_significant_bit` /
`unknown_noun_falls_back_to_the_hand_written_list` / `the_significant_over_point_override_only_moves_aidtex` /
`direct_enumeration_already_lists_aidtex_under_zone_as_exactly_one_root`；`model_impact.rs` 的
`every_dictionary_point_container_is_skipped_as_a_generation_root` → `every_non_significant_point_container_is_skipped_as_a_generation_root`。
`cargo test --lib -- generation_root::tests …`：29 passed。**开关仍默认关**——默认开是下一步的拍板，不在本条里。

**做的时候发现两件事，改了本条的定性：**

1. **「唯一真漏」只在旧 PE 路径成立。** direct 文件窗口的 `enumerate_generation_roots_in_subtree` 按
   `noun_is_significant` 枚举、**不看** point 容器也不看这个开关——`AIDTEX` 直接挂 ZONE 下在 direct 路径上
   **本来就是**一个 Normal 根（新测试钉住）。今天 DESI 的生产路径是 direct，所以 §0 那句「生产上这条漏还在」
   要收窄成「兼容数据模式（旧 PE 规划器）上还在」。原稿要的「计划层用例：AIDTEX 直挂 ZONE 产生恰好 1 条
   `RegenRoot`」在旧路径上要走 `build_model_update_plan`（打库），没有纯内存入口，**以 direct 枚举那条代替**；
   旧路径的 RegenRoot 留给 live 用例。
2. **「开关开 + 快照外」那一态今天用真 noun 构造不出来**：1931 个 noun unknown = 0，名单兜底的
   LOOP/PLOO/VERT/PAVE 与三个 `*DATU` 全在快照里。所以 C1-3 第二段直接钉谓词（位表答 `None` → 不抬根），
   这也是把 `significant_bit` 做成参数的原因。

顺带看到、**没动**的一处 direct/旧路径分歧：BOX 直挂 ZONE 下，旧路径解到自己（owner 是粗层级 → 自身），
direct 枚举因 `core_significant_bit("BOX") == Some(false)` **不出根**。真库里 primitive 直挂 ZONE 是否存在没查，
登记待议，不在 W1.2 内。

原稿的三件事（保留作对照）：

- **换判据接口（改码，S）**：`:374` 的 `noun_is_significant(noun)` 换成
  `core_significant_bit(noun) == Some(true)`。理由：前者对快照外的 noun **保守取真**，开关开着时一个没导到的
  point noun 会被抬成根——「未知 noun 走旧兜底」这条边界被改了。这一步只影响快照外的 noun，
  E3D 3.1 快照 unknown 为 0，所以今天行为不变、换版本那天才显出差别。
- **补契约测试（同一次提交）**：
  - C1-1 `significant_bit_resolves_to_itself_and_to_nearest_significant_owner`
  - C1-2 `delivery_unit_outranks_the_significant_bit`
  - C1-3 `unknown_noun_falls_back_to_the_hand_written_list`——**两态都要断**：开关关着逐位相同；
    **开关开着、noun 不在快照里，也逐位相同**（这条就是上面换接口的验收）
  - `the_significant_over_point_override_only_moves_aidtex`：44 个 point noun 逐个跑，开关开与关的结果
    **恰好**只有 `AIDTEX` 不同（`options.rs:1263` 注释已经引用了这个名字，W0.2 把它标成待写）
- **改口径**：`model_impact.rs:1523 every_dictionary_point_container_is_skipped_as_a_generation_root` 改成
  「所有**非 significant** 的 point 容器」，`AIDTEX` 作为唯一例外显式列出。
- 验：上面五条 + 已有的 `significant_over_point_only_promotes_aidtex_when_enabled` +
  `all_dictionary_nouns_have_a_total_incremental_update_policy` 仍绿；一条计划层用例「`AIDTEX` 直接挂 ZONE 下时
  产生一条 `RegenRoot`」（今天默认关 = 零工作项——这是 P1 判出的唯一真漏，也是本计划唯一的正确性项）。
- 退：开关默认关 → 灰度 → 默认开，三步各自可回。默认开的前置就是上面五条全绿。

**W1.3 R22 真库验证 —— ✅ testbed 已过（2026-09-06），剩收尾（M，需要一台跑着的 testbed）**

**完成（2026-09-06）**：`live_testbed_complete_projection_reclaims_only_the_absent_primitive`
已在 8019 RocksDB 沙箱通过；测试使用随机独立数据库并在结束时删除，覆盖完整回执删除与无回执保护。
下面是 Oracle 审阅后**仍要补**的部分。

- 做什么：C3-1 `absent_primitives_are_reclaimed_before_redraw`，落 `tests/staged_regen_e2e.rs`。
  窗口：某生成根下**删掉一个 primitive**，同时**改根上另一个属性**（保证根会重画）。
  断言：`model_refresh` 定向收尾后，被删 primitive 的 record id 在每张模型表里**零行**；
  反向对照：跳过对账时同一窗口留下孤儿行（记成回归基线，不是期望）。★O 对照组要**人工种一行孤儿**再跑，
  把 `DeleteCleanup` 的贡献隔离开——不能预设「关对账就必留孤儿」，那可能是 `DeleteCleanup` 顺手清掉的。
- ★O **差集两边哪边危险，原稿写反了。** 令 E = 库里挂在这根下的行（`projected_geometry_ids_on`），
  P = 本轮投影出的 record id 集合，删的是 E − P。**E 查询失败归空 → 差集为空 → 什么都不删，安全；
  P 因生成失败 / 分块截断归空 → 差集 = E → 全删，这才是要防的那一侧。** 2026-09-06 的完整性门
  （有匹配根回执、记录数与来源会话一致、祖先范围正确才允许把缺席解释为删除）守的正是 P 侧。
  R23 那条纪律于是要改成两句：P 侧不完整 → 不回收、**也不得盖「完整成功」凭证**；E 侧不可用 → 不回收、出声，
  同样不得盖凭证。两条各一个测试，今天只有 P 侧的
  （`an_unreceipted_partial_projection_does_not_delete_nested_root_rows`），E 侧的要补。
- ★O 已有的嵌套根护栏是**直接喂投影**给适配器，只证明「对账口径一致」，**不证明生成完整**——
  `generate_subtree_with_cache` 真把 D 名下的元素产出来了这件事，要靠 C3-1 这种走完生成器的用例来证。
- ★O 后续场景（Oracle E 点名的五个）：冷启动 `restore_projection_on` 回灌后再对账、跨根搬移、旧会话并发写回、
  分块失败重试、域外行不变。**本计划只承诺前两个**（ams8000 九窗口里的「BOX 改挂 / 成员移动」可复用），
  后三个登记不排期。
- 验：C3-1 绿 + P 侧、E 侧两条护栏绿 + 真库 `removals` 明细逐条能解释（每一条要么是被删 primitive，要么是人工孤儿）。
- 退：★O 「只是验证」不算退路——对账在真库上是删行。W1.3 的验证只在**冻结会话的独立模型库副本**上跑
  （8019 testbed 就是这么做的：随机独立库、结束即删），跑前先导出被删 id 与房间边的预览清单；
  若哪天要在生产库上开，退路是**快照恢复或对该 dbnum 补一次窗**，写进操作手册再开。

### W2 —— 判据决策（需要拍板，拍完才动码）

**W2.1 T2c 入口丢弃门，第一批（M）**

**完成（2026-09-06，默认关闭）**：`model_entry_drop_no_geometry` 已同时接入 legacy
操作分区与 direct 窗口账本闭包；1272 项集合由测试钉死，未知 noun、Deleted、
TypeChanged 保守保留，`details` / `design_refnos` 不动。C1-6 生产侧开关两态已落。
真实 ams8000 266→267 基准中 legacy work items 由 2 降为 1、额外重算单元由 91 降为 0，
10/10 实际模型单元仍覆盖且 `unexplained=0`；direct 该窗本来就是 1→1，九个固定窗口
开关两态均通过。原始 JSON 与复现见
`docs/evidence/2026-09-06-no-geometry-entry-gate/README.md`。第二批 223 项未实现。

- 什么：既非 significant 又非 primitive 的 noun 变化**不产生模型工作项**（R13）。这是唯一落在**漏判侧**
  的一步，所以分两批、单独开关、单独灰度，不与 W1 捆在一次发布里。
- 第一批：1272 个「字典里 `geomset` / `extrusion` / `graphics_behaviour` 三项皆空」的 noun
  （名单出自 P1 证据 §2.1）。它们落在多做侧，丢弃是省不是漏。
- 改哪：`model_impact.rs::classify_operation_impact` 之前（或 `partition_operation_impacts` 入口）加门，
  开关 `model_entry_drop_no_geometry`，默认关。**门只筛模型工作项，不碰 `details`**（`design_refnos` 是数据层）。
- 验：C1-6 那「另外半条」——同一输入下 core 参考模型队列空、我方开门后工作项也空、关门时仍 `Regen`；
  一条**队列噪音基准**：同一真实窗口（取 `.fable1-e2e` 或 testbed 的一份会话）开门前后 `work_items` 数对比，
  数字写进证据，没有量化收益就不上第二批。→ **已做**：ams8000 266→267，legacy 2→1、91→0。
- ★O **从「默认关」走到「开」的前置，比上面那条基准硬得多。** 「自身无几何 = 丢弃是省不是漏」这句
  **没有被证明**：一个 noun 自己不出几何，不等于它的变化不影响后代或引用它的元素。开启前要：
  1. **冻结名单与来源版本**（1272 项 + `core_sha256`），改名单 = 重走本条；
  2. **影子统计**：开关关着跑 N 个真实窗口（至少覆盖 ams8000 / ams7999 / 1112 三库），只**记**「开着会丢掉哪些
     工作项」，按 noun × 变更类别（新建 / 修改 / 删除 / 改挂）列表；
  3. **逐 record 对拍**：对每个窗口固定 S/T，开关两态各跑一遍，再与「T 端全量重生成」比：几何、变换、
     存活 record 集合三者**完全一致**，且 `details` / `design_refnos` 逐字节相同；
  4. 影子统计里出现过、对拍没覆盖到的 noun × 类别组合，**不丢**——从名单里摘出来等第二批一起评审。
  现有的「10/10 实际模型单元仍覆盖、`unexplained=0`」是第 3 条的弱化版（只看覆盖，不看逐 record 一致），不够。
- 第二批：223 个「字典认为有几何能力」的 noun **逐条评审表**（noun / 几何能力来源 / core 两位 / 我方判词），
  评审表成文后**单独**拍板，不在本计划内承诺。
- 退：开关。

**W2.2 「视图切换」在我们这边是什么（决策，S）**

core 的队列有三条作废边：一次 `Update`、一次 `Refresh`、一次**视图切换**（`LoadSheet`）。前两条我们各有对应物
（执行完即消费 / `discard_pending_for_full_rebuild`），第三条没有定义。三个候选：

| 候选 | 对应我方动作 | 判词 |
|---|---|---|
| a. 切 MDB | `UpdateScope::resolve` 换 MDB | 已被「整库重建」覆盖——换 MDB 必然重建，走 R6 那条 |
| b. 改 watch scope（ADR-058 D9：调试开关） | `watch_scope::admits(dbnum)` 收窄 | ★O **原稿建议采纳，改判不采**。D9 是**调试用的准入开关**，不是「这个视图作废了」；core 的 `LoadSheet` 之所以敢整队丢弃，是因为新视图会**从头画**——我们收窄 watch scope 之后**没有任何东西替被丢掉的 pending 补种**，再把域扩回来时那些库的模型就静默陈旧。要采 b 得先证两件事：在途窗口与收窄互不干扰、再扩域时有补窗；今天两条都没有 |
| c. 切投影域（`ModelMemoryStore` 进程内投影） | 重启 | 重启本来就清（ADR-050 `clear_stale_at_process_start`），不必另立 |

**建议决策：视图切换在我们这边登记为「无对应物」**——三个候选要么已被 R6 覆盖（a、c），要么前置不成立（b）。
`record_decision` 一条，核对表 R6 / §1.5 照 W0.3 补判词；`narrowing_the_watch_scope_discards_out_of_scope_pending`
**不写**。若日后 b 的两个前置补齐，另开一条。

**W2.3 P5 定性（决策 + 核实，S）** ~~★O 提前做~~ **✂ 2026-09-06 22:10 移出本计划**（用户：只关注 DESI 三维模型增量更新；
CATA 级联归 ADR-058 实施计划。下文保留作交接材料，不再排期。）

- 核对表 §9 那一行按 W0.1 改掉之后，真正要答的是：**ADR-058 D2 实施到哪了**——「CATA 文件会话索引差分 →
  `ref_rev` → `cascade_expand`」这条链今天有没有生产入口？ADR-058 状态栏写着「其余未实施」。
- 做法：沿 `expand_live_reverse_cascade`（`manual_update.rs:2088`）与 `build_cata_cascade_plan`
  （`model_update_plan.rs:801`）往上找调用方；若触发源仍是「CATA 数据批次」（ADR-058 之前的形状），
  D2 未实施——**另开计划**，不并进本计划（它是 ADR-058 的实施项，不是核对表的）。
- 交付：一段核实结论写进核对表 R1 注记；D2 若未实施，给出它的计划入口链接。
- ★O **提前做**（Oracle A）：它是 S、只读、不依赖任何别的项，而结论决定 CATA 改动今天到底有没有兜底在接——
  这比 W1.1 的默认值重要。顺带看一眼 `catalogue_cascade_degraded` 在真实回执里出现过没有
  （`initialization_phase.rs:189`、`model_update_pending.rs:2940`）。

**W2.4 R12 / R14：深度上限 32 与粗层级容器即停，量一次再定（S，只读）** ★O 新增

- 为什么：旧路径 `resolve_element_generation_root` 有 `MAX_ANCESTOR_DEPTH = 32`、遇 SITE/ZONE/WORL 即停；
  core 的 `SignificantOwner` 无上限、只认位（R14）。T2b 只翻转 point 那一档，**没有回答**「深度上限会不会把
  真库里的链截断」「有没有 owner 环让无上限循环不终止」这两个问题。
- 做法：用 `noun_census_probe` 同款的全库扫描，对 5 个项目 486 个 DESI 库统计 owner 链最大深度分布、
  是否存在环（有环则 core 那条 while 也不会终止，说明 core 依赖别处保证）。产物一份 JSON + 一段结论。
- 判：最大深度远小于 32 → 上限是纯防御、留着；接近或超过 → 立项改成「按位终止 + 环检测」。
  **不据此改判据**，只决定立不立项。

### W3 —— 「慢」类（省时间，不修正确性，排最后）

**W3.1 R28 目标消失不空跑 —— ✅ 已完成（direct 路径），改为收尾（S）**

- 现状（20:48 重核）：direct 文件窗口由 `roots_T` 枚举决定 `regen`；`roots_S \ roots_T` 整体归 `delete`
  （`window_root_plan.rs:15–17`），护栏触发即 `degraded`；兼容 PE 路径保留执行前终态存活复核。
- ★O Oracle 的三个反问，逐条对：
  - 「不在 `roots_T` ≠ 元素被删」——对，还有「还在但不再是根（改挂 / 类型变）」这一档，它落在 `delete` 格，
    旧根名下的几何一并清，**不是**跳过；
  - 「跳过须确认清理已覆盖」——`delete` 格就是清理，`lazy` 格（`roots_T \ roots_S` 且未波及）才是真跳过，
    它跳过的是从未生成过的根，没有旧几何要清；
  - 「读取失败须重试」——读失败走 `degraded`（`advance` 清空、`roots_S ∩ roots_T` 全 regen），是**多做**不是漏；
    但今天没有一条测试直接以「T 端读失败」为输入。
- 收尾：把 C2-5 的用例名对齐到现实现（或在用例集里注明由 `guards_degrade_credential_advance_into_regenerating_every_persisted_root`
  与四格枚举测试覆盖）；补一条「T 端读失败 → degraded 而非 lazy」的测试；回执里的 `skipped_vanished` 不再需要。
- 退：无改动。

**W3.2 T3.3 祖先抢占去重（L，先量化再决定做不做）**

- 先做基准：C2-2 的输入（同一窗口改 EQUI + 其下 1000 个 NOZZ），记今天 `work_items` 数与规划耗时。
  **数字不难看就不做**——这条从头到尾是省时间。
- ★O **基准的第一步是先证明冗余存在。** NOZZ 挂在 EQUI（MDU）下时，我方 `delivery_root` 本来就把它们
  解析到同一个键 `(RegenRoot, EQUI)`——C2-2 那个输入今天可能**已经是 1 条**，收益是零。真正可能 N>1 的是
  **嵌套显著根**（显著节点名下的显著节点各自成根）。direct 窗口已经在记这两个数：`nested_roots` 与
  `duplicate_coverage_elements`（ams8000 255→256 上分别 ≥2 与 >0），执行耗时用 `last_root_duration_ms`。
  基准就拿这三个指标在九个钉死窗口上的读数——没有一个窗口 `duplicate_coverage_elements > 0`
  且重复那部分耗时可观，W3.2 就关掉。
- ★O C2-3 不能当验收（见 W0.4）：删除路径 core 自己就不吸收，改验「两条删除项与一条的最终模型行集合相同」。
- 若做，实现要点全部已在 `core3d_reference.rs` 有契约，照抄即可，别再从证据文档伪码里读：
  - 键先归一化 `IsPrimitive(e) ? e : SignificantOwner(e)`（R20，C1-10）
  - 三个 state **三套判法**：Changed 沿 owner 链上行、New 只找 New、Deleted **不上行**（R17–R19，C1-9）
  - 删除路径配 `AncestorDeletes`：判据 `primitive ∨ significant`，命中已标记祖先**只跳过该级 push、上行照走到顶**
    （R21，C1-7/C1-8——这条证据文档第一版写反过）
- 改哪：`model_update_plan.rs:155 insert_item` 那张 `BTreeMap<(ModelWorkAction, String), _>` 之后加一遍
  「被祖先 `RegenRoot` 覆盖则吸收」。
- 验：C2-2 `ancestor_regen_absorbs_descendant_work_items`（N → 1，输入改成嵌套显著根而不是 EQUI+NOZZ）、
  C2-3 改写后的「删行效果等价」版本。
- 退：开关 `model_ancestor_dedup`，默认关。

### W4 —— 新缺口取证（只读 IDA / 只导数据，不改生产码）

**W4.1 GETWORK 通道梳理（M，需要 ida-bridge 上 `Core3D.dll`）** ◐ 2026-09-06 22:50 部分完成

已答（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md`）：第 1 条——过滤器 `sub_1052F2E0` 的输出去了
`change/EVALAT`（`0x1022C679`，DCHC 驱动的联动规则表）→ `EVALCD/EVALST` → **QCHGLS**（`dword_10E98540`，步长 3、类别取 max）；
第 2 条——重画单位是 **TOPF**：每个变更元素先经 `desdblib/FNDTOP` 按 `toplevel` 位（`nounlib/LISTOP` 读 `0xA187C`）折叠；
第 4 条——变更集 = `DB_RawChanges::trim` 后的集合，`postGetWork` / `postUndo` / `preRedo` 三入口共用同一条路，
UNDO/REDO 与 GETWORK 口径完全相同。**未答**：第 3 条（`gb==1/2` 与 `is_loop_container_noun` / `COARSE_HIERARCHY_NOUNS` 名单的对应）。
**新出的活**：EVALAT 里有核对表与 09-04 清单都没登记的联动边——结构连接 `ADSTCH`（清单新 #12，direct 路径缺口候选）。

- 为什么：核对表整份讲 `PartialUpdateDesiMgr`（DRAFT 图纸侧）。**`GETWORK` 才是「别人 SAVEWORK 了，我把变更
  收进来」——跟 gen-model 干的事最像的就是它**（证据 §3.2），一次都没梳理过。
- 查什么（按证据 §3.1 的入口）：
  1. `DDES_GraphicsMngr::postGetWork`（`0x1052EA35` → `0x1052EAF5`）→ 过滤器 `sub_1052F2E0` 的**输出**去了哪
     （只知道它产出一个有序 `std::set<DB_Element>`）；
  2. `DES_DrawListRefresh::{beforeGetwork, afterGetwork, update, change}`（`0x1052DF20`–`0x1052E480`）
     ——GETWORK 之后重画的**单位**是 TOPF 项还是 significant 段；
  3. 过滤器里 `gb == 1`「只留最上面那个数据容器」与 `gb == 2`「层级容器整个跳过」，与我方
     `is_loop_container_noun` / `COARSE_HIERARCHY_NOUNS` 名单的对应关系——这可能是名单的**第二个数据来源**。
  4. ★O **变更集从哪来**：`sub_1052F2E0` 遍历的那个 `std::set<DB_Element>` 是谁填的、填的是 QCHGLS 还是别的
     来源；以及 `postGetWork` / `postUndo` / `preRedo` 三个入口**共用同一个过滤器**这一点对我们意味着什么
     （UNDO/REDO 在我们这边对应哪一种窗口）。
- 交付：`docs/evidence/2026-XX-XX-ida-core3d-getwork-channel.md`，★O 每条结论都带**可复查的三样**：输入（地址 +
  指令流）、分支（判据在哪一跳）、输出的消费者（谁读了这个结果集）。如出现 DB 级联动规则，追加到核对表
  作 **R30+**（编号续接，不改 R1–R29）。
- 明确**不**交付：不据此改任何判据——先当第二个 oracle 对账，对账结果再立项。★O 具体两条：
  **不**另立一套按 TOPF 选根；**不**把过滤器的 `gb` 跳过直接套成我们的丢弃门——过滤器跳过一项只说明
  GETWORK 重画不以它为单位，不证明它的几何影响可以丢。

**W4.2 `toplevel` 位交叉验证（S，需要一次 live E3D 进程）**

- 重导 `core-noun-granularity-e3d31.json` 时加第四张表 `toplevel`（`0xA187C` / 661628），走同一条
  `DB_Noun::getField(id, &out)` 通道，与 `output/noun_caps_full.json` 的离线值逐 noun 对。
- 验：1931 个 noun 零差异，或差异逐条落判；C0-1 的自洽断言扩到四张表；★O **C0-3「恰好三个键」同一次改成
  「恰好四个键」**（W0.5 先在用例集里预告），否则重导那天 C0-3 先红。
- **不改判据**：证据 §4.1 已判 `toplevel` 是显示属性归属层，几何粒度仍按 `significant`。

## 4. 顺序与依赖

```
W0 ✅ ──▶ W1.2 ✅（旧 PE 路径的正确性项；开关仍默认关）──▶ W2.1 开启前置（影子统计 + 逐 record 对拍）──▶ 第二批评审表 → 单独拍板
W2.3 核实   ✂ 移出本计划（CATA 不在「DESI 三维模型增量更新」范围内，2026-09-06 22:10）
W2.4 量深度 随时，只读
W1.1        正样本取证 → 再改默认；不阻塞任何项
W1.3        ✅ testbed 已过；剩 E 侧护栏 + 走完生成器的 C3-1
W2.2 决策   建议登记「无对应物」，不落代码
W3.1        ✅ 已完成，剩收尾（用例名对齐 + 读失败测试）
W3.2        先证冗余存在（三个观测指标），数字决定做不做
W4.1        并行，只需 IDA；结论回灌核对表
W4.2        并行，需一次 live E3D；与 C0-3 同改
```

- ★O 排序改了两处：**W1.2 提到 W1 之首**（它关的是唯一真漏，其余 W1 项都是「多做 / 已验」）；
  **W2.3 提前**（S、只读、结论决定 CATA 改动今天有没有兜底）。W1.1 从「先做」降为「不阻塞」。
- W2.1 的代码已在（默认关），它等的不再是 W1.2 的接线，而是自己的开启前置——两者仍**不捆一次发布**，
  后者是唯一落在漏判侧的一步。
- 需要外部条件的只有三项：W1.1 正样本（E3D 造子树）、W4.1（IDA）、W4.2（live E3D）。其余全部离线可做。
- 每项单独可停：W0 与 W4 不留半成品；W1–W3 每项一个开关。

## 5. 验收总表（与用例集编号对齐）

★O 每一格都写成**能证伪的断言**，不写「绿 / 有数字 / 成文」。

| 任务 | 用例 | 今天 | 交付后（可证伪的写法） |
|---|---|---|---|
| W1.1 | 正样本四窗口（子树内新建 / 修改 / 搬出 / 搬进）× 两条路径；`the_xgeometry_gate_*` ×2；默认值断言 | 默认关；正样本 0 个 | 四窗口在 direct 与旧路径上的 `work_items` 集合**逐项相同**，`xgeometry_gated` 名单与预期名单相等；随后默认值断言改口径 |
| W1.2 ✅ | C1-1 / C1-2 / C1-3（两态）/ `…only_moves_aidtex` / 改口径的 point 容器测试 | ~~接线在，契约测试**不存在**~~ 全部已落（22:30） | ✅ 47 个 point noun（44 + 3 `*DATU`）× 两形状，两态对称差 **恰好 = {AIDTEX}**；✅ 快照外 noun 两态**逐位相同**，且谓词在位表答 `None` 时为假；◐ 「`AIDTEX` 直挂 ZONE 恰好 1 条 `RegenRoot`」以 direct 枚举「恰好 1 个 Normal 根」代替（旧路径要打库，留 live） |
| W1.3 | C3-1 + P 侧 / E 侧两条护栏 + 人工孤儿对照 | P 侧护栏 ✅（testbed）；E 侧、C3-1 不存在 | 被删 primitive 的 record id 在每张模型表 **0 行**；人工孤儿 **恰好 1 行**被回收；E 侧失败时 `removals` 为空**且**无「完整成功」凭证 |
| W2.1 | C1-6 后半条 + 噪音基准（已做）+ 开启前置 | ✅ 默认关；legacy 2→1、91→0 | 开启前置：N 个窗口两态与 T 端全量重生成的几何 / 变换 / 存活 record 集合**逐 record 相等**，`details` 逐字节相同；不等的 noun × 类别摘出名单 |
| W2.2 | `record_decision` 一条 | — | 决策文本里三个候选各有一句「为何不成立」；**不新增代码与测试** |
| ~~W2.3~~ ✂ | ~~核对表 R1 注记~~ | 移出本计划（22:10） | ~~注记回答一句可判真假的话：「CATA 文件差分 → `ref_rev` → `cascade_expand` 今天有 / 没有生产入口，入口在 `文件:行`」~~ 归 ADR-058 实施计划 |
| W2.4 | 深度分布 JSON | — | 486 库 owner 链最大深度 = 一个具体数字；环 = 0 或列出 refno |
| W3.1 | 四格枚举测试 + 「T 端读失败 → degraded」 | 四格有；读失败那条不存在 | 读失败输入下 `degraded_reason().is_some()` 且 `regen ⊇ roots_S ∩ roots_T` |
| W3.2 | 三个观测指标 → C2-2（嵌套根输入）/ C2-3（删行等价版） | 指标在，基准数字没记 | 九个钉死窗口的 `duplicate_coverage_elements` 与重复耗时占比写成表；若立项，N → 1 且删行集合两版相同 |
| W4.1 | 证据文档 + R30+ | — | 每条结论带 输入地址 / 判据跳转 / 输出消费者 三栏；缺一栏不算成文 |
| W4.2 | C0-1 扩四表 + C0-3 改「恰好四键」 | 三表 | 1931 noun 的 `toplevel` live 值与离线值差异数 = 0，或每条差异各有一行判词 |

## 6. 风险

| 风险 | 在哪 | 怎么挡 |
|---|---|---|
| R2 门（W1.1 第 2 步之后）第一次真碰到 XGEOM 子树 | W1.1 | 先有正样本四窗口才改默认；warning 出声；开关可关；两条单测覆盖四种两态组合 |
| T2b 翻转的不止 AIDTEX（换版本重导后） | W1.2 | `…only_moves_aidtex` 钉死「恰好一个」，换版本时它先红 |
| T2c 第一批名单里混进有几何的 noun | W2.1 | 名单来源固定为 P1 证据 §2.1 的三项皆空；开关默认关；先基准 |
| 按根对账把嵌套根的行删掉（R10 反向） | W1.3 | 已有护栏 + 绊线；W1.3 在真库上再看一次 `removals` 明细 |
| T3.3 按「一套判法」实现 | W3.2 | 契约在 `core3d_reference.rs`，C1-7 反例断言会红 |
| GETWORK 通道查出与 DRAFT 侧相反的规则 | W4.1 | 只取证不改码；两侧冲突时先记录再拍板 |
| ★O 按根对账在真库上是**删行**：P 侧（本轮投影）失败 / 截断归空 → 差集 = E → 全删 | W1.3 | 完整性门守 P 侧（已在）；E 侧失败不回收不盖凭证（要补）；验证只在冻结会话的独立库副本上做，先导出删除预览；生产库退路 = 快照恢复或补窗 |
| ★O 嵌套根护栏直接喂投影，不证明生成完整 | W1.3 | C3-1 走完生成器；`removals` 明细逐条可解释 |
| ★O T2b 开着时快照外 noun 被保守抬成根 | W1.2 | 判据换 `core_significant_bit == Some(true)`；C1-3 加「开关开 + 快照外」那一态 |
| ★O T2c「自身无几何 = 可丢」未被证明 | W2.1 | 默认关不动；开启前置 = 冻结名单 + 影子统计 + 逐 record 对拍 |
| ★O watch scope 收窄后丢 pending，再扩域无人补种 | W2.2 | 不采 b；登记「无对应物」 |
| ★O C2-3 把不是 core 的行为当 core 契约 | W0.4 / W3.2 | 改写成删行效果等价；参考模型里**不**加这条 |

## 7. 需要拍板的事

★O 五条建议里三条改了方向（1、3 反转，2 加前置），新增第 6 条。

1. **W1.1 默认值**——~~建议开~~ **改为：不改默认，先造正样本**（零实例只证语料不触发；direct 路径翻默认值是空操作）。
2. **W2.1 第一批（1272）**——代码已在、默认关。**要拍的是「开启前置」**：接受「冻结名单 + 三库影子统计 +
   逐 record 对拍全量@T」这一套为开启门槛？接受则照此排期，不接受就一直默认关。第二批等评审表。
3. **W2.2 视图切换**——~~建议 b~~ **改为：登记「无对应物」，不落代码**（b 的两个前置——在途隔离、再扩域补窗——今天都没有）。
4. **W3.2 做不做**——先证冗余存在：九个钉死窗口的 `duplicate_coverage_elements` 与重复耗时占比出来再定。
5. **W4.1 谁来做、什么时候**——需要有人开 ida-bridge；不阻塞 W1–W3。
6. **W2.4 是否立项量 owner 链深度**——建议立（S、只读），它是 R12/R14 那两格 ◐ 能不能收口的前提。

## 8. Oracle 审阅（2026-09-06 18:48–18:58，已取回）

> 送审：`oracle` 0.18.0，browser 引擎，模型 GPT-5.5 Pro，会话 `core3d-conformanc-dev-plan-3`
> （前两次 `core3d-conformanc-dev-plan` / `-2` 因浏览器侧发送失败报错，第三次成功；
> 更早一次 `core3d-conformanc-plan-review` 用 GPT-5.6 Sol 带 9 个附件 181k token，同样发送失败）。
> 输入 6 份附件、59,134 token；输出 350 token；耗时 9m36s。
> 原文：`C:\Users\dpc\.oracle\sessions\core3d-conformanc-dev-plan-3\artifacts\transcript.md`
> （副本 `%LOCALAPPDATA%\Temp\oracle-core3d-plan-review.md`）；ChatGPT 会话已归档。
> 提问按 A 顺序 / B 遗漏 / C 五个拍板项 / D 验收 / E 风险 五段，并约束「不要建议照抄 DRAFT 视图语境那一半、
> 不要建议取消 TransformOnly 与 Unknown→Regen」。
>
> 下表每条：Oracle 原意 → 采纳与否 → 理由（能核的都对着附件 / 工作树核过）。正文里对应改动标 `★O`。

| # | 段 | Oracle 的意见（摘要） | 采纳 | 理由 / 落点 |
|---|---|---|---|---|
| 1 | A | 草稿 §0「没有一类是缺了会让模型错」应删：08-27 计划 P1 已确认 `AIDTEX` 漏排 | **采纳** | 核对：P1 §2.2 原话「是真缺口，但很窄」。开关今天默认关，漏还在。§0 改写，W1.2 升为唯一正确性项 |
| 2 | A | W0 后优先 W1.2；W1.1 不应阻塞其余项 | **采纳** | §4 顺序改。W1.1 的理由本来就只有「零实例」 |
| 3 | A | W1.3 拆成「安全护栏 → 隔离真库验证」 | **已是这么做的** | 白天 8019 testbed 用随机独立库、结束即删；§3 W1.3 把这条从「顺带」写成规则 |
| 4 | A | W2.3 提前核实 CATA 兜底是否真被触发 | **采纳** | S、只读、无依赖；结论比 W1.1 默认值重要。§4 提前 |
| 5 | B | R12/R14 缺独立处置：最近显著祖先、32 层截断、owner 环；T2b 只改 point，不能据此说上卷已对齐 | **采纳（缩小为取证）** | 新增 W2.4：量真库深度分布与环，只决定立不立项，不改判据。理由：没有数据前改判据就是从名单换成另一种猜 |
| 6 | B | W4.1 应补变更集来源与 UNDO/REDO 共用路径；不另立 TOPF 选根、不直接套用 gb 丢弃门 | **采纳** | W4.1 查什么加第 4 条，「不交付」加两条具体禁令 |
| 7 | C1 | R2 门不改全局默认；先正样本验证新改删、跨界、预览/执行一致；warning 不是验收 | **采纳** | 重核又补一条：direct 路径只观察不筛（`window_root_plan.rs:320`），翻默认值对生产路径是空操作。W1.1 改成「正样本 → 再改默认」 |
| 8 | C2 | T2c 先影子统计不启用丢弃；「省不是漏」未被证明；开启要冻结名单、逐 noun × 类别、逐 record 对拍全量@T、`details` 不变、未覆盖不丢 | **采纳为开启前置** | 代码已在且默认关，不回退；现有「10/10 覆盖、unexplained=0」只是弱化版。W2.1 加四步前置，§7.2 改问法 |
| 9 | C3 | 不选 b：D9 是调试准入不是投影作废；收窄没有补种，须先证在途隔离与再扩域补窗 | **采纳，反转原建议** | 核对：core 敢整队丢是因为新视图从头画，我们收窄后没人补种。W2.2 改为登记「无对应物」，不写那条测试 |
| 10 | C4 | 去重先基准；同 EQUI 下 NOZZ 可能已归到一个键，先证 N>1 冗余存在 | **采纳** | 核对：`delivery_root` 确实把 NOZZ 归到 EQUI 的键，C2-2 原输入可能今天就是 1 条。基准改用 direct 窗口现成的 `nested_roots` / `duplicate_coverage_elements` / `last_root_duration_ms` |
| 11 | C5 | GETWORK 由逆向证据维护者负责，W0 后并行，不据此缩减工作 | **采纳（无改动）** | 与原稿一致 |
| 12 | D | W1.2 应以 `core_significant_bit(noun) == Some(true)` 覆盖 point，开关开时未知 noun 也走旧兜底；保守取真接口会改变这条边界 | **采纳** | 核对：`generation_root.rs:374` 用的正是保守取真的 `noun_is_significant`。今天快照 unknown = 0 所以行为不变，但换版本那天边界就错了。W1.2 加「换接口 + C1-3 两态」 |
| 13 | D | W3.1「不在 `roots_T`」≠ 元素被删；跳过须确认 T 端不存在且清理已覆盖；读取失败须重试 | **采纳（映射到现设计）** | 核对：「还在但不再是根」落 `delete` 格并清旧几何，读失败走 `degraded`（多做）。缺的是一条以「T 端读失败」为输入的测试，W3.1 收尾补 |
| 14 | D | 草稿引用的 C2-3 不能当 core 契约：用例集要祖先标记吸收 Deleted，但 R19/R24 说 Deleted 不看 state 2；改验生产计划删行效果等价 | **采纳** | 核对 R19：`IsPending(e, Deleted)` = `IsPresent(key,3) ∨ IsPresent(key,4)`，不上行；R24：state 2 只有 `IsPending(·, Changed)` 读。**用例集写错了**，新增 W0.4 改写 C2-3，W3.2 验收跟着改 |
| 15 | D | W1.3 另加人工孤儿，隔离 `DeleteCleanup` 的影响 | **采纳** | 否则「关对账必留孤儿」是预设不是观察。写进 W1.3 对照组 |
| 16 | D | W4.1 须交付可复查的输入 / 分支 / 输出消费者 | **采纳** | 写成交付物三栏，缺一栏不算成文 |
| 17 | D | W4.2 同步 C0-3「恰好三表」的契约 | **采纳** | 新增 W0.5 预告 + W4.2 同改 |
| 18 | E | 草稿把差集写反：E = 库中行、P = 本轮投影，删 E − P；E 查询失败归空**不会**全删，P 归空才会 | **采纳，原稿确有错** | 原稿 W1.3 说「`projected_geometry_ids_on` 失败 → 空集 → 全删」，方向反了。改成 P 侧 / E 侧各一条纪律与测试；E 侧那条今天没有 |
| 19 | E | 两端不可用或投影不完整均不得回收、不得盖完整成功凭证；合法完整空投影应清旧行 | **采纳** | 后半句已由 `reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection` 钉住；「不盖凭证」写进 W1.3 |
| 20 | E | 只在冻结会话的独立库副本操作、先备份、预览删除 id / 房间边；验冷启动、跨根搬移、旧会话并发写回、分块失败重试、域外不变；护栏直接喂投影不证明生成完整；「只是验证」不是退路 | **采纳** | W1.3「退」改写；C3-1 走完生成器；五个场景列为 W1.3 后续，本计划先落前两个（冷启动、跨根搬移——都有现成窗口） |

**没有采纳的**：无。Oracle 没有建议照抄 DRAFT 侧规则、也没有建议取消 `TransformOnly` / `Unknown→Regen`，
约束条款生效。它没有评论 W0 与 W2.3 的内容本身，也没有指出 §1 表里任何一行的事实错误（§1 的 R9 / R28 两行
是我们自己按 20:48 工作树改的，不是它指出的）。

## 9. Plannotator 评审记录（已结束：聊天里口头通过，无批注）

> `plannotator annotate docs/plans/2026-09-06-core3d-partial-update-conformance-dev-plan.md --json`
> 2026-09-06 晚，Oracle 意见回填之后发起；本节记：批注原文 → 改动落点。未取回则注明。
>
> - 第 1 次 21:08–21:14：`{"decision":"dismissed"}`，**无批注**（标签页关闭，随后会话交接）。
> - 第 2 次 21:16–21:19 由接班会话（fable-5-1-75）重开，**同样无批注**：`~/.plannotator/history/gen-model/annotate-…-704b20df/`
>   下只有文档快照 `001.md`、没有 `submissions/`；会话登记 21:19:38 被清、草稿同秒标删。随后又一次会话交接。
> - 第 3 次 21:52 由 fable-5-1-83 重开，进程挂后台等待提交；文档内容与第 1 次快照相比只多了本节这几行。
>   **21:58 用户在聊天里直接批「计划我看过没意见，按 §4 顺序从 W0 开始做」**，Plannotator 进程随即关闭，
>   三次都没有产生批注。评审结论：**通过，无改动**；§7 六条拍板项按本文建议执行，未被单独否决。
