# ADR-066：只动方位的窗口按「刚体前移」发布——不重算网格，但仍是一次根级发布

状态：**已接受（2026-09-08 23:2x，用户按 19:50 修订版拍板）**。**同日 19:50 修订**（会话 fable-5-1-64，读源码后）：D1 加规则 5（级联扩展进来的根不走便宜路）；
D3 的祖孙锚点改取**最内层**、盒的规则改成「纯平移加 t / 含旋转读网格顶点」；D6 的「算不出 Δ」改成规划期判定；Q1 据此关闭。
**2026-09-15 修订**（会话 fable-5-1-21，属性变化分类对齐 core 计划 S2 / S3）：D1 规则 3 补第四类排除——P23 摆放派生 noun——与第五类
——被属主消费的几何（负体 / 剖面数据 / 几何集；非单元在消费型属主下；`EVALAT` 规则 M 结构件的严格祖先），见规则 3 末注。
**2026-09-17 修订**（会话 fable-5-1-37）：D1 规则 2「子树里另有非锚点候选 → 整根 `regen`」只数账本去向 `RolledUp` 的候选，
去向 `no_model` 的（两票元数据门放行 / 没有模型归属）不是锚点也不挡路，见规则 2 末注。**同日 Q6 落地**（用户拍板，会话
fable-5-1-37 / fable-5-1-42 接力）：只 `CACHID` 跳动的候选进 `no_model` 第三档 `CacheStamp`——挪一件时被顺带盖戳的子树成员从此不再挡规则 2，
见规则 2 末注（含对同条 Q5 段「只有 `CACHID` 变照旧 `regen`」半句的更正）。
**2026-09-19 修订**（用户 18:15 拍板 A，会话 fable-5-1-36 起手、fable-5-1-6 接力）：`SPAMAP` 与 `CACHID` **同档进戳表**
`PLACEMENT_STAMP_ATTRIBUTES`（计划 §8 Q6 题 B / N2 t-334：同是 3.1 运行库 schema 簿记键、字典无票、生成器不读、四库全量 1718 根 0 红），
规则 2 的戳例外与 Q6 第三档从此都是两个名字；**同日凭证前移比例护栏的分子改按净候选数**（毛候选 − 只戳候选 `cache_stamp`），见规则 2 / 规则 4 末注。
**同日 21:59 验收 1 的对拍门复跑**（会话 fable-5-1-16，`probe_rigid_exclusion_windows` 全语料 3157 窗，报告 `docs/evidence/2026-09-19-rigid-exclusion-parity-spamap.md`）：
`SPAMAP` 进表后 `is_placement_only` 多认的 885 个候选里 39 个成了合格锚点、其余 846 个仍被规则 3「路由容器之内」排除；合格锚点 163 → 186、对拍 152 根
**不等·锚点 0**，门照旧过。少的 16 个是 ams7997 HVAC 的 `BEND` / `ATTA` / `DAMP`——e3d-model 09-18 `f93fd66` 让 HVAC BRAN 也派生隐式直段之后改判「路由容器之内」，
与 `SPAMAP` 无关。`model_transform_cheap_path` 默认仍关。
**同日 23:5x–00:4x 执行体两处「待补」补齐**（会话 fable-5-1-37）：D3 含旋转时读回记录自己的网格、顶点乘到新世界系重算盒（`rigid_motion_mesh_bounds`，
隐式管身的 `local_transform` 已折进世界矩阵不再乘），此前一律 `bail!` 回退；D4 回执落款改按 pin@T 现算（`rigid_motion_target_at`：与 regen / 单元级同一条
`hydrate_published_dependencies → model_target` 路，设计窗之外任一格变了回退 regen），此前沿用旧回执。执行体仍未在真投影上跑过（S4b B7 / B8 / 验收 2 / 3 照旧零），见 S4b 计划 §9。
**2026-09-20 02:1x–02:4x 验收 1 的生产记录半边首跑**（会话 fable-5-1-37 起手、fable-5-1-47 接力，`probe_rigid_motion_gate3_for_window`，报告 `docs/evidence/2026-09-20-s4b-gate3-rigid-motion-parity.md`）：
**ams8000 195→196 过 S4b §5 门 3**（`record_id` / 1e-9 / 1e-3 / `mesh_id` 复用 / 回执与目标都在 T），另 6 窗 12 根同过，含两窗**含旋转**（D3 读网格重算盒与 regen@T 逐位相同）。
三条发现，全部已拍板落地（fable-5-1-51 08:5x）：**嵌套根双乘已修**——嵌套显著根名下记录互相包含、执行体逐根串行把共有记录**乘了两次 Δ**（ams7997 156→157 / ams1112 724→725 红；P0 幂等闸按根判护不住），
`republish_records` 加**按记录**的幂等闸（`source_sesno == T` 不再乘、比规划基线新的整根回退 regen）后两窗全跑都过（14 + 3 / 1090 + 31 记录，|Δ| ≤ 9.1e-13、盒差 0）；
纯平移下 Baked `mesh_id` 也漂（本地化残差进 SHA，内容逐位同）——§5「纯平移必复用」对 Baked 立不住，**§5 门 3 改口**：Canonical 按 id、Baked 一律按内容判（顶点 / 法线 < 1e-3、索引同 → 观察项）；
`world_bounds` 1e-3 mm 这一轮全过但最大 0.000977 mm = 1 ulp、离线 2.3%，坐标 ≥ 16384 mm 段同样的差就判死，**§5 门 3 改口**：`|Δ| ≤ max(1e-3 mm, 2·ulp_f32(max(|a|,|b|) + |t_c|))`。
两处改口后 7 窗 19 根复跑全过（含旋转两窗的 Baked 按内容判也 0 不等）。
`model_transform_cheap_path` 默认仍关；live #2 / 验收 3 未跑。
**2026-10-14 更新**（分支 `feat/issues-36-50-34`）：live #2 真机链验证清单与验收 3 性能测试方案已备好
（`docs/evidence/2026-10-14-adr066-live2-verification-checklist.md` /
`docs/evidence/2026-10-14-adr066-live2-performance-test-plan.md`），待真机环境执行；
翻默认值仍待 live #2 与验收 3（Task 时长省 ≥ 30%）双绿。
逐条理由与实位见 `docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md` §2。
计划 `docs/plans/2026-09-08-external-increment-refactor-plan.md` §7.1 **G5 已拍 (a) 搬**，
本文是那份「小 ADR」：把 ADR-056 D7 早就决定保留、却从未在 direct 路径上落地的 `Transform` 便宜路径，
放进 ADR-065 之后的世界（根级 CAS 发布 + 进程内投影 + 凭证落款 + Task 内联 + 房间随 Task 收尾）。
它是 ADR-056 **D3「模型增量执行粒度 = 根级整根重算」的唯一例外**。
日期：2026-09-08
关联：
- **实施** ADR-056 D7（「`ModelWorkAction::Transform` 便宜路径保留，判据改吃 e3d-model `ElementDiff`」）——D7 定了判据，
  没定执行物；direct 成为默认（2026-09-04）之后窗口规划器只产 `RegenRoot / DeleteCleanup / RoomRecalcPanel`，D7 一直是纸面。
- **例外于** ADR-056 D3（根级整根重算）；**不动** D1（模型失败不阻断水位）、N3（模型发布根级原子）、N7（模型面不以 `pe` 行为前置）。
- **沿用** ADR-014（根级 CAS 发布）、ADR-054（生成时点 = 文件最新；凭证单调）、ADR-060（水位由凭证探针推进）、
  ADR-065 D1 / D4 / D8（在 Task 里算完；失败留痕；房间随 Task 收尾）、计划 §7.1 **G2 (i)**（房间失败 = 牵涉的根不安定）。
- **退役** legacy 执行物 `AiosDBManager::update_world_transforms` / `get_inst_relate_nodes_in_subtree` /
  `refresh_world_transform_products`（`increment_manager.rs`）——它们走 `pe_owner` 图与 `aios_core::get_world_transform`，
  违反 N7，且不动 CAS / 投影 / 落款（见背景）。随计划 S4 删老 PE 规划器一起删，**但只能在本 ADR 的执行物落地之后**。
- 证据：同日会话 fable-5-1-22 12:52 / 13:02 两轮分析（三类变化在 direct 上只剩一类）；`element_diff.rs:131-166`
  （`placement_input_changed` 与 `is_placement_only` 是两个问题）；`increment_manager.rs:4644-4830`（legacy 执行物的形状与它为什么
  排除管段行）；`model_memory_store.rs`（`ProjectedGeometry.world_transform / world_bounds / ancestors`、`RootReceipt`、`commit`）；
  `model_db_adapter.rs:1090-1092`（`desired_revision` CAS）；`window_root_plan.rs:889-944`（凭证前移守卫）。

## 背景

一窗 S→T 里的变化分三类：**不影响模型**（`NAME / DESC / PURP / LCHKDA`）、**只动方位**（`POS / ORI`）、**动几何**（其余）。
老 PE 规划器分得开：`classify_operation_impact` → `Skip / Transform / Regen`，`Transform` 走 `update_world_transforms` 只刷子树的
世界矩阵 + AABB + 空间树 + 房间归属，网格不动。direct 窗口规划器（今天 external 在跑的那条）没有这一层：e3d-model L2 只回答
「有没有属性变了」，`POS / ORI` 一变就上卷成 `regenerate`，而且 `placement_input_changed` 触发 `collect_unit_subtree`——**整棵
子树的单元**都进计划，每个被波及的根都 `RegenRoot`。移一个 ZONE = 名下全部根整根重算（几何求值 + 发布 CAS + AABB + 房间），
单位网格内容寻址只省了网格文件那一份。

第一类同日已搬进 direct（元数据门 `model_skip_metadata_only_changes`，见 changelog）。第二类没搬，原因不是判据——
e3d-model `ElementDiff::is_placement_only` 现成，四项排除（类型 / 属主 / 成员 / `opaque`）写得比老路严——而是**执行物**：

- 老执行物动的是 `inst_relate` 行的 `world_trans` 指针：不过根级 CAS、不动 `ModelMemoryStore` 投影（`/model/records` 与
  `memory_root_current` 读的是它）、不改 `RootReceipt` / `published_model_target`、不前移 `gen_root.source_end_sesno`。
  在 ADR-065 的世界里这四个地方各留一个洞：投影与库行分叉；根的凭证停在 S、下一窗 `generation_root_cache_current` 判它过期又重算一遍；
  `desired == published` 的凭证前移守卫看不见「位姿刷新在飞」。
- 它读 `pe_owner` 子树与 `aios_core::get_world_transform`（持久层 / direct 读路由），违反 ADR-056 N7；`pe` 零行的库上它选不出节点。
- 它把隐式管段行从刷新集里排除、并要求子树里每个 BRAN / HANG 另排一条 `RegenRoot`（`increment_manager.rs:4723-4739`）——因为它是
  **逐元素重算世界矩阵**，管段的世界矩阵是「单位圆柱 → 世界管段」的缩放矩阵，按元素属性算不出来。

计划 S4 要删老 PE 规划器，`Transform` 这一档若不先搬就从「默认不可达」变成「代码里也没有了」。用户 2026-09-08 拍板 **(a) 搬**，先写本 ADR。

## 决策

### D1　谁走便宜路：**根**级判定，条件全部在文件侧

窗口规划器 `plan_window_roots` 多一个去向 **`transform`**（与 `regen / delete / advance / lazy` 并列）。一个根进 `transform` 当且仅当：

1. 它**已持久化且安定**（`roots_S ∩ roots_T`，`gen_root` 完成态、`publication_status = ready`、`desired == published`）——
   没生成过的根没有可前移的记录，照旧 `lazy`；在飞 / 失败的根照旧 `regen`。
2. 它子树里本窗**全部**候选都是「锚点」：`ElementDiff::is_placement_only`（只有 `POS / ORI` 变，类型 / 属主 / 成员 / `opaque` 都没动），
   或者是锚点的后代（后代自己一个字没动、只是被祖先带着走）。子树里**任何一个**候选不满足 → 整根 `regen`（regen 吞掉 transform）。
   **（2026-09-16 修订 · 同计划 §8 Q5，两票齐后用户拍板接线）** 判「摆放之外还动了别的」时**忽略戳**——e3d-model
   `element_diff::PLACEMENT_STAMP_ATTRIBUTES`，今天只有 `CACHID`：它是 Design 应用随每次图形重建写下的图形缓存 `+1` 计数，
   不是几何输入（S8 普查 453 库 2961 窗，13724 次跳动全是 `0→1` / `n→n+1`；生成器一处不读，读取集合把它单列成 `stamp_literals`，
   两仓守卫钉住不许混进那一票）。于是 `[POS, CACHID]` / `[ORI, CACHID]` 算「只动了摆放」，**只有 `CACHID` 变**照旧不算
   （摆放一项都没动，是整库缓存重建的扫写，照旧 `regen`；要不要另给它开一档是计划 §8 Q6，不在本 ADR）。
   不接这一下的代价：3.1 真库上这条判据几乎永远为假——有值元素上 88 次含 `POS`/`ORI` 的变化**全部**同时跳 `CACHID`，
   便宜路径打不着 EQUI / BRAN / SCTN。两票都在：生成器不读（读取集合护栏）+ S4 对拍门把它剔出比较集再判，多出 84 个合格锚点、
   139 根对拍、108 个有判别力的两态全等、0 红（`docs/evidence/2026-09-16-rigid-exclusion-parity-ignore-cachid.md`）。
   本条其余四项排除、`placement_input_changed`（戳不是矩阵输入，不触发级联）、`PLACEMENT_ATTRIBUTES` 与开关默认值一律不动。
   **（2026-09-17 修订 · 会话 fable-5-1-37）** 「子树里任何一个候选不满足」**只数账本去向 `RolledUp` 的候选**。去向 `no_model`
   的候选（e3d-model `Disposition::NoModel`：两票元数据门放行的——子元素只改了 `NAME` / `DESC` 那类——或改动落在没有模型归属的
   元素上）e3d-model 没把它折进任何单元 / 派生容器，没有几何跟着它变：它不是锚点，也**不把根踢回 `regen`**
   （`rigid_motion::CandidateKind::NoModel`，去向由 `ChangeLedger::dispositions` 读出）。不接这一下的代价：真实编辑几乎必带这种
   候选——「挪 EQUI + 子元素改个名」两件事单独发生分别是 `transform` 与 `skip`，落在同一窗却整根 `regen`。三条边界：
   根里**只有** `no_model` 候选（没有一个 Δ）照旧 `regen`——零锚点的 `transform`（只推回执不动记录）是选根的另一道题，不在本条；
   `Unresolved` 照旧挡路（那时窗口本就退化，规则 4）；第三条**同日已改**：只 `CACHID` 跳动的子元素原本**不是** `no_model`
   （计划 §8 Q6 未落地）、仍挡路——E3D 3.1 挪一件会给整棵子树带 `CACHID` 的元素各盖一次戳，ZONE / STRU / 带 SUBE 的 EQUI 挪位因此打不着
   这条路。**（2026-09-17 Q6 落地 · 用户拍板）** 戳现在是元数据门第一票的**唯一例外**：字典按构造收不到 `CACHID`（3.1 运行库 schema
   里的项目 UDA，不是「还没补上」而是永远不会有），第一票改由 e3d-model 的戳表 `PLACEMENT_STAMP_ATTRIBUTES` 给
   （gen-model `model_impact::attribute_is_cache_stamp`），第二票照旧问读取集合（守卫 `stamp_literals ∩ names = ∅`，两仓各钉一次）。
   只有戳跳了的候选记 `no_model` 第三档 `NoModelTier::CacheStamp`（回执 `cache_stamp`，与前两档互斥、都含在 `no_model` 里），
   不上卷、不级联、账本去向 `NoModel`——于是它对规则 2 不再挡路；戳混在别的放行属性里不改档（跟着 `NAME` 记老档、跟着 `DUTY` 记新档），
   `[POS, CACHID]` 仍是锚点那条路（`is_placement_only`），不是元数据。上一段「只有 `CACHID` 变照旧不算（摆放一项都没动）」
   说的是**它不是锚点**，仍成立；「照旧 `regen`」那半句自本日起作废。窗 C 复跑见 changelog 2026-09-17。
   **（2026-09-19 · `SPAMAP` 同档 · 用户拍板）** 戳表加第二个名字 `SPAMAP`（e3d-model `PLACEMENT_STAMP_ATTRIBUTES = ["CACHID", "SPAMAP"]`）：
   3.1 运行库 schema 里的空间索引簿记键（`INTVEC`，字典无票），Design 应用随图形 / 空间索引重建逐次 `+1`，生成器一处不读（两仓
   `stamp_literals` 守卫同款钉住），S8 语料 453 库里单独跳 11666 次；两态实证与 `CACHID` 同一把尺子——四库全量 1718 根 0 红
   （归 `SPAMAP` 增量的 1034 根）、全语料命中 36 库（30 库是 `CACHID` 一根都没剔到的），四门与 `CACHID` 同向、无一格反向
   （`docs/evidence/2026-09-16-q6-spamap-denominator-audit.md` §4.1）。后果与 `CACHID` 完全平行：本条规则 2 判「摆放之外还动了别的」时
   忽略它（`[POS, SPAMAP]` / `[POS, CACHID, SPAMAP]` 算只动了摆放）、只 `SPAMAP` 跳记第三档 `CacheStamp`、不挡规则 2；
   **另加一手 `CACHID` 当初没有的**：ams7997 那类库里 5819 个只 `SPAMAP` 候选把 52→53 / 54→55 两窗候选数顶过索引键数 30% 的线
   （7328 / 8535 > 5993 / 7652），整窗按规则 4 退化 regen 216 根、B0 剔种子一根救不回——于是**比例护栏的分子改按净候选数**
   `candidates − cache_stamp`（gen-model `WindowImpact::cache_stamp` / `net_candidates`，元数据门关着时恒 0、逐位回到之前），
   净掉后 5161 / 5534 不再退化，216 根量到红 0。名单仍只有 e3d-model 那一份；接线记录见 changelog 2026-09-19。
3. 锚点及其到根的路上**没有路由容器**（e3d-model 上卷规则 1 里「会补管的容器」：BRAN / HANG 等，管段几何由成员两端点派生），
   锚点**不是点 / 环容器下的元素**（`graphicsBehaviour == 1` 那类祖先之下的点：它的 `POS` 是属主网格的输入，e3d-model 上卷规则 1
   本来就把它并进属主单元），也**不在 CWBRAN 子树里**（桥架件的摆放是伪属性，`cable_tray::placement_matrix` 读兄弟，上卷规则 5）。
   这三类里的位姿变化是几何变化，照旧整根 `regen`——issue #5 / ADR-056 D7「生成根不是路由容器」原样保留。
   **（2026-09-15 修订 · 开发计划 `2026-09-15-attribute-change-class-alignment-dev-plan.md` D1-a / S2）** 第四类：锚点自己的 noun 在
   **P23**（e3d-model `category::PLACEMENT_DERIVED_NOUNS`：`DRAW CLNTIL STRFLT INFITT PLTFRM ELFITT STRLNG HVACFI RAIL DOOR WINDOW FPFITT
   LINDIM FLRCOV MLABEL FIXTUR HRPOST HRTERM HRGATE HRPANE KICKPL CTSUPP FURNIT`）里——core 自己的 3D 侧对字典类别 3（`POS`/`ORI`）默认只改
   图形段矩阵（`css/STRTRA`/`SEGTRA`），**唯独**这 23 个 noun 走 `add/REPELE` 整件重建（Core3D `change/UPGRPH` `0x1022EAEE`，取证
   `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §4）：几何由摆放派生（门窗开孔、栏杆立柱随相邻件、桥架支架随
   托盘、标注随被标对象），左乘 Δ 的网格是错的。判在 `window_root_plan::anchor_noun_exclusion`，与桥架件那条同位；名单随 E3D 版本重取。
   **第五类（同计划 D1-b / S3）：被属主消费的几何。** 三层判，全在 T 端、只看 noun（`window_root_plan::anchor_lineage_exclusion`）：
   (a) 锚点自己是负体（`Category::is_negative`：`NBOX NCYL NXTR …`，属主正体拿它做 CSG）、剖面 / 路径数据（`ProfileData`：`PLOO PAVE LOOP VERT
   SPINE CURVE RPATH POINTR`——注意 `PLOO` 的 gb 是 0，原文「gb==1 容器之下」那条接不住 `PANE → PLOO → PAVE`）、几何集成员；
   (b) 锚点不是模型单元（`is_model_unit` 假）而直接属主消费成员（`Category::consumes_members`：正体 / 目录件 / 复合件 / `TMPL`）——
   `PLOPEN`/`WLOPEN`/`BPOPEN` 开孔在 PANE / WALL / BPANEL 下、`CMPF` 负体包、分类表没认领的 noun 在正体下；单元（BOX 在 TMPL 下、NOZZ 在 EQUI 下）
   与纯容器属主之下的非单元（SUBE 在 EQUI 下）不受此条影响；
   (c) core `EVALAT` 规则 M 的**严格祖先**半边：锚点的 T 端祖先含 `PANE`/`GENSEC`/`SCTN`/`BPANEL` → 那个结构件重建——`FIXING` 今天**是**模型单元
   （2026-09-14 起走目录构件 GMRE），只有这一条接得住「FIXING 在 SCTN 下挪位」；我们今天不把它的 `TMPL` 负体扣到宿主上，但 core 那一票是「重建」，
   少算方向两票缺一不放行。结构件**自己**挪位不在此列（它的网格只依赖自身属性 + 目录）。
   五类排除的原因文本各不相同，回执 `transform_fallback_reason` 原样带出。开关 `model_transform_cheap_path` 仍默认关，S4 对拍样本过了再翻（D7）。
4. 窗口没有退化（`WindowImpact::degraded_reason() == None`）。退化时一切进 `regen`，与今天一样。
   **（2026-09-19 末注）** 退化四因里的「候选数超过索引键数 30%」自本日起按**净候选数**判（`candidates − cache_stamp`，
   只有戳跳了的候选不计——它们去向 `NoModel`、不上卷不级联，对闭包可不可信一个字都没说；`cache_stamp` 为 0 时逐位同前）。
   其余三因（闭包不完整 / `unresolved` / 设计反查判不了）不动。
5. **（19:50 修订）** 它不是由级联扩展（C2 结构连接 / C8 电缆端点 / P4 `POSREF` 反查，`WindowRootSources::impact` 的
   `expand_touched`）加进 `touched` 的——那三条是「别处动了、我这里的几何随之变」，不是刚体运动，照旧 `regen`。
   判据在 `expand_touched` **之前**算，扩展进来的根若与 `transform` 候选重合，`regen` 吞掉。

锚点在根**之上**（挪 ZONE / SITE / STRU / PIPE）时，规则 2 对它子树里的每个安定根都成立，于是这些根**全部**进 `transform`——这就是
「移一个 ZONE 不再整区重算」。ADR-056 D9「只对受影响根 eager」不变：`transform` 与 `regen` 一样只对被波及的根。

判据只读两端 `DbSet`（e3d-model `ElementDiff` + `transform::world_matrix`），一行 `pe` 不读（N7）。

### D2　位姿刷新**是一次发布**：同一把锁、同一个 CAS、revision 照走

- 执行体与 `RegenRoot` 同在 dbnum Task 里（ADR-065 D1），持同一把 `db_generation_lock(dbnum)`，先过 `ensure_not_older_than_persisted`。
- 每根一次根级 CAS（`gen_root.desired_revision` 递增 → 发布 → `published_revision` 跟上，`model_publish_superseded` 语义不变）。
  在飞的位姿刷新对凭证前移守卫（`desired == published`）而言与在飞的重生成**同样**「未安定」——那道守卫不需要认识新动作。
- 产物是一份 `ProjectionCommit`：`upserts` = 该根被波及的记录（`record_id` / `geometry_id` / `mesh_id` / `shape` / `tube` **不变**，
  只换 `world_transform` 与 `world_bounds`），`removals` 与 `tube_branch_resets` 为空，`receipt` 重盖。走**同一条** `ModelMemoryStore::commit`
  + `model_db_adapter` 落库路——不另开写口（D4 的「同步」由此免谈：投影与库行本来就是同一份 commit 的两个去向）。

### D3　执行物：对投影记录**左乘一个刚体增量**，不逐元素重算

对锚点 `a`：`Δ = W_T(a) · W_S(a)⁻¹`，两端矩阵由 e3d-model `transform::world_matrix` 在 `DbSet@S / @T` 上算（文件侧）。
根 `r` 里 `ancestors ∋ a` 的每条记录（`ancestors` 含自身）：`world_transform' = Δ · world_transform`。
**盒（19:50 修订）**：`bake_element` 存的 `world_bounds` 是世界网格顶点的**紧** AABB，不是本地盒过矩阵——所以
`Δ` 是纯平移时 `world_bounds' = world_bounds + t`（精确）；`Δ` 含旋转时任何「盒过矩阵」都松、过不了对拍门，
要读网格顶点重算（`PlantMesh::des_mesh_file(mesh_dir/<mesh_id>.mesh)`，非管段 Canonical 先乘 `local_transform`）。
Baked / Canonical 在这条规则下没有区别，不必补本地盒字段。

隐式管段**不再需要排除**：它的 `world_transform` 是世界系矩阵，锚点在 BRAN 之上时管段随分支刚体移动，`Δ · T_tube` 就是新矩阵——
老路排除它是因为「逐元素重算」算不出，不是因为它不能刷。锚点在 BRAN **之内**（挪一个管件）时管段真的变形，那是 D1 规则 3 的 `regen`。

同一窗同一根上多个锚点：每条记录只乘**一个** `Δ`——它**最近的锚点祖先**（最内层）的那个（19:50 修订，原文写「最外层」是错的）。
理由：`Δ_a` 由两端**世界**矩阵算出，已含 `a` 之上所有锚点的运动。记录 `p` 在 `E` 下、`E` 在 `Z` 下，同窗 `Z` 与 `E` 都动：
`Δ_E · W_S(p) = W_T(E) · W_S(E)⁻¹ · W_S(E) · L(E→p) = W_T(p)` ✓；`Δ_Z · W_S(p) = W_T(Z) · L_S(Z→E) · L(E→p) ≠ W_T(p)` ✗（漏了 `E` 自己的运动）；
两个都乘则重复计算。`ProjectedGeometry.ancestors` 是「自身 → 顶层」序，第一个命中锚点集的就是最内层——实施约束 3 钉它。
兄弟锚点各自的 `Δ` 只作用于各自子树；不在任何锚点之下的记录矩阵不变、只前移 `source_sesno`（同根记录的 `source_sesno` 必须一致，
`persist_root_chunks_impl` 断言它）。

### D4　落款与凭证：与重生成**同款**

发布成功后 `gen_root.source_end_sesno = T`、`published_model_target = ModelTarget@T`（`design.session = T`，目录落款不变）、
`RootReceipt { source_sesno: T, model_target, manifest_hash: 按更新后的记录重算, geometry_count: 不变 }`、`publication_status = ready`、
`published_revision = desired_revision`。这样 `generation_root_cache_current` 判它当前、凭证前移守卫放它过、ADR-060 探针把它算进安定下限——
与一次 `regen` 落地后一模一样。行上另加一格**只供观测**的 `published_via: 'regen' | 'transform'`，不参与任何判决。

### D5　派生面与房间：与重生成同一条收尾

`world_bounds` 变了 → 空间树刷新走 `publish_completed_file_round` 今天那条；房间按 ADR-065 D8 / 计划 G2 (i)：被前移的构件与面板进本 Task
的房间重算，房间重算成功是该根凭证前移的前置，失败则该根**不安定**（与 regen 的根同一条路）。「设备从 A 房挪到 B 房」正是这一档的典型用例，
房间那一半不许省。

### D6　失败即回退成 `regen`，同一个 Task 内

**（19:50 修订）** `Δ` 在**规划期**算（两端 `DbSet` 当时都在手里）并随计划携带，执行体一个文件都不读；所以「两端任一端读不到锚点」
是规划期的判定——该根直接进 `regen` 桶。执行期的回退只剩三种：投影里缺该根的记录（`root_projection_complete` 为假）、
旋转时网格文件读不到、发布事务失败（`mixed model target publication` / superseded / 持久层错）——任一情况把内存投影恢复成旧记录 + 旧回执，
该根在**本 Task 内**改走 `RegenRoot`，不留半套。回执 `units` 上这根记 `kind = regen` 与原因（`transform_fallback_reason`）。
不写待重试表（ADR-065 D4）。

### D7　开关与默认值：**先关后开**

新开关 `model_transform_cheap_path`（`DbOption.toml`；环境变量 `AIOS_MODEL_TRANSFORM_CHEAP_PATH` 压过）。它是「少算」方向的开关，
**默认 `false`**，直到 §验收 的对拍门在 ams8000 与 live #2 上过了再翻默认；关着时 `transform` 桶整体并回 `regen`，逐位等于今天。
元数据门（`model_skip_metadata_only_changes`）与本开关彼此独立。

## 后果

- **省的**：整区平移 / 设备挪房这类现场，从「名下全部根整根重算（几何求值 + 发布 + AABB + 房间）」降为「投影记录矩阵左乘 + 一次根级发布 +
  AABB + 房间」；网格文件、`geometry_id`、`record_id` 一个都不变，客户端只需重载矩阵与盒。
- **付的**：规划器多一个桶与三条排除规则；执行体多一条路；`plan_window_roots` 的真值表变宽（实施约束 2）。
- **不变的**：正确性口径——`transform` 落地后的记录必须与「同一根在 T 上整根重算」的记录**逐条相等**（`record_id` / `mesh_id` 相同，
  矩阵与盒在容差内相等）；这是验收的硬门，也是 D7 默认关着的理由。
- **对 S4 的意义**：老 PE 规划器与 legacy 执行物可以删了——三类变化在 direct 上从此都有对应物（Skip = 元数据门，Transform = 本 ADR，Regen = 今天）。

## 实施约束

1. **顺序**：本 ADR 过目 → 计划 §4 加一步 **S4b「Transform 档搬家」**（规划器桶 + 执行体 + 开关 + 对拍门）→ 过门 → 再删老 PE 规划器与
   `update_world_transforms` 一族。S4 的「删」排在 S4b 之后；删之前先改 `occ_generate.rs:976` 那条钉
   （`targeted_regen_and_transform_use_the_incremental_aabb_entrypoint` 以 `refresh_world_transform_products(` 为锚，要求 transform 走
   durable 增量 AABB 入口、不在指针提交后单独入队房间重算）——改成钉本 ADR 的执行体，判据不变。
2. **源码钉**：`plan_window_roots` 只把 `roots_S ∩ roots_T` 且安定的根放进 `transform`；`regen ∩ transform = ∅`；退化时 `transform = ∅`；
   路由容器 / 点容器 / CWBRAN 三条排除各红一次；执行体全文无 `pe_owner` / `get_world_transform` / `collect_pe_subtree_refnos`（N7）；
   开关关着时 `transform` 桶为空、计划与今天逐位相同。
3. **纯函数**：根的分桶判据、「记录取最近锚点祖先」、纯平移判定、记录前移、回执 `manifest_hash`（与 `project_geometry_delta`
   **同一份**配方，抽出来两处共用）都是纯函数，`mem://` 之外单测（形状：锚点即根、锚点在根之上、祖孙锚点取**最内层**、兄弟锚点各自 `Δ`、
   锚点在 BRAN 内 / CWBRAN / 点容器各红一次、同根另有非位姿候选、删除候选在 base 链上、级联扩展进来的根、未安定根、退化窗口、开关关）。
4. **`mem://` 真跑**：一根两条 Canonical + 一条 Baked + 一段隐式管段的夹具，锚点在根之上 → `transform` 发布 → 逐条对拍「整根重算 @T」；
   再造一次 CAS 冲突（另一个 revision 抢先）→ 回退 `regen`、投影与库行都没有半套。
5. **房间**：沿 P2-b 那套 `mem://` 两态逐边门，位姿前移后的面板 / 构件归属 == 全量重建。
6. **CONTEXT.md**：「模型影响 (Model Impact)」词条改口（三类在 direct 上各自的落点）；新词条「刚体前移发布 (Rigid-motion Republish)」；
   `ModelWorkAction::Transform` 语义改为本 ADR。

## 验收

1. **对拍门**（`#[ignore]` 手动，ams8000）：沿会话链逐窗找出 D1 判成 `transform` 的根，对每根算两态——本 ADR 的 `Δ` 前移 vs 在 T 上整根重算——
   `record_id` / `mesh_id` 集合相同，`world_transform` 逐元素 `|Δ| < 1e-6`，`world_bounds` 同容差；语料里若没有自然的纯位姿窗，用
   `AIOS_TRANSFORM_WINDOW=base:target` 指定用户在 E3D 里造的一窗（挪一个 ZONE 的 POS）。
   **2026-09-16 修订注**（会话 fable-5-1-49，属性变化分类对齐 core 计划 S4）：门建在 gen-model `window_root_plan::tests::probe_rigid_exclusion_windows`
   （`#[ignore]`），范围从 ams8000 扩到五个目录全语料，判决用 `rigid_anchor`（D1 规则 3 全部排除），对拍是**文件侧**：A = e3d-model `generate_subtree` 在 T 上整根重算，
   B = 在 S 上整根生成、每件记录左乘最近锚点的 `Δ`（D3 的「祖孙锚点取最内层、兄弟锚点各自 Δ」同口径），比 `GeometryId` 集合与体积 / 三角数 / 亏格 / 包围盒 /
   世界矩阵（1e-6）；不是生产投影记录的 `record_id` / `mesh_id`——那一半仍是验收 4 的 `mem://` 真跑。首跑（453 库 2961 窗）：合格锚点 56 根，27 个有判别力的
   两态全等、0 红；被排除的锚点若照 Δ 前移，负体 / 规则 M / 路由容器三条各有干净样本不等（排除有内容）。语料里没有纯位姿窗的排除（P23、开孔、CWBRAN、
   点环容器、桥架件、剖面、几何集）仍等造窗。另：`AIOS_RIGID_PROBE_IGNORE_CACHID=1` 把 `CACHID` 剔出 `is_placement_only` 再判，多出的 84 个锚点
   （整台 EQUI 挪位为主）108 根全等——计划 §8 Q5 的第二票。
2. **live #2**（计划 §5）加一类变化：7997 挪一个 ZONE 的 POS → SAVEWORK → 名下根走 `transform`（日志 / 回执 `units[].kind`）、
   `/dbnums` ≤ 120 s `in_sync`、房间边逐边 == `build_room_relations`、plant-ui 里几何整体平移且网格不重载。
3. **量**：同一窗关 / 开开关各跑一次，记 Task 时长与生成器 CPU 时间——这是 G5 原本「先量再定」那一步，改在实施后补量；
   若省不到 Task 时长的 30%，本 ADR 的默认值维持 `false`、只作可选项。
4. `cargo test --lib`（带计划 §5 的 `--skip`）不降。

## 不做

- 改挂（`owner_changed`）两端照旧整根重算（ADR-009）；类型变更、`opaque` 同。
- BRAN / HANG 内部的位姿变化、点容器里的点、CWBRAN 子树：`regen`（D1 规则 3）。
- 历史快照投影（`ProjectionScope::Historical`）不受影响：它是按需重算的，不做前移。
- 不给 `applied_sesno` / 水位任何新语义；不引入第二份「欠什么」的表（ADR-065 D2）。
- 不搬老执行物的「`pe_owner` 子树筛选 + 逐元素 `get_world_transform`」——那是被本 ADR 替换掉的形状，不是被搬的形状。

## 开放问题

- ~~**Q1** Baked 记录的本地盒：从网格清单现取，还是补进 `ProjectedGeometry`~~ **已关闭（19:50）**：难点不在 Baked / Canonical 而在旋转——
  `bake_element` 的 `world_bounds` 是世界网格顶点的紧 AABB（`e3d_model_service.rs:1535-1543`），本地盒过矩阵对含旋转的 `Δ` 一律偏松。
  D3 改成「纯平移加 `t`；含旋转读网格顶点」，两种形状都不需要本地盒，不补字段。
- **Q2** `published_via` 要不要进 `/dbnums` 四格的陈述（「已追平：其中 N 根为前移」）——先只落行，看 live #2 用不用得上。
