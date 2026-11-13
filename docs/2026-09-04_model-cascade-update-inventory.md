# 2026-09-04 模型增量「联动更新」梳理度核对（gen-model × core.dll / Core3D.dll）

- 问题：**现在模型增量更新的联动更新情况，是否梳理整理完毕了？**
- 手段：只读代码核对 + **ida-bridge 活桥**取证（`core.dll` = `idalib-33060`、
  `Core3D.dll` = `idalib-25128`，均在 `D:\ida_scratch\plant3\`）。本文新增地址全部本轮实读，可按地址复查。
- 状态：**审核 + 当天三处收口**——`CONTEXT.md` 加「联动更新」/「图形行为」两条词条；
  第 10 类落 ADR-061；e3d-model 补 gb==2 护栏（§2.6，唯一的代码改动，**测试与注释**）。
  未跑服务、未提交。
- 承接：`docs/plans/2026-09-02-increment-update-audit-and-next-plan.md`（core 取证 + 两套规划器）、
  `docs/2026-09-01_increment-implementation-review.md`（凭证时效 F1–F4）、
  `docs/2026-08-04_container-transform-cascade-gap.md`（容器变换级联的误诊与修正）、
  `docs/plans/2026-09-04-startup-increment-audit-and-next-plan.md`（启动复核 15 条，#4 = CATA 零感知）。

---

## 0. 一句话结论

**开工时没有梳理完。** 十一类联动里当时六类已闭环、两类只有机制没有验收、三类是在案的洞；
更要紧的是**这份清单本身在仓里不存在**——每一类各自散在 4–5 份文档里，没有一处把「有哪些联动、
各自谁负责、验收在哪」摆成一张表，所以「漏了哪一类」这个问题当时在仓内无法回答。

本文（§1）补上这张表，并当天收掉三条：第 10 类 `XRPNTR` 从「连状态都没有」落成 **ADR-061**
（明确不做 + 四条复议触发条件）；核实第 11 类**其实早已落地**（此前误记为「在案未做」，§2.6）；
第 11b 类（gb==2 层级容器永不成件）**补上护栏**——那 98 个里有 10 个带字典 `primitive` 位，
今天不成件是巧合不是保证。
**收口后真正没闭环的只剩第 4 类（CATA→DESI），外加第 3、5 类缺验收门。**

本轮 ida-bridge 另有**五条新取证**（§2），其中三条直接改写既有认知：core 的 GETWORK 重画路
**只吃 DESI 与 ISOD 两种库**（CATA 根本不进这条路）、`graphicsBehaviour == 2` 的语义、
以及 `graphicsBehaviour == 3` **全产品无任何代码路径**——09-02 文档 §5 开放问题 4 就此两半都结掉。

---

## 1. 联动更新分类表

「联动更新」= 元素 A 变了、**A 自己以外**的模型也必须跟着重算。按触发边分十一类：

| # | 联动类型 | gen-model 承担者 | 状态 | 依据 / 缺口 |
|---|---|---|---|---|
| 1 | 容器位姿 → 整棵子树（ZONE/SITE 的 `POS`/`ORI`） | `ModelWorkAction::Transform` → `update_world_transforms`（世界变换 + AABB + 空间树 + 房间归属） | ✅ 闭环 | `2026-08-04_container-transform-cascade-gap.md`；预览口径已与执行合一（`partition_operation_impacts` 单一事实源） |
| 2 | 删除 → 全部子孙 | `propagate_deletes_to_descendants` + `topmost_deleted_refnos` + `DeleteCleanup` | ✅ 闭环，且与 core 同口径 | core `DB_Compare::scanOld`：被删子树里**仍活着的后代算搬迁不算删除**；我们的 Deleted 桶来自索引键集差，搬走的键仍在 → by construction 一致 |
| 3 | 改挂 → 新旧两个根都要重算 | ADR-009；老规划器两端都入队 | ◐ 老路已闭环，新路未接 | e3d-model 侧的单元键按 refno，不会重发旧根 manifest；要靠 ledger `Reparented(old_owner)` 补排，即 09-02 计划 P1-1，**未做** |
| 4 | **CATA → 引用它的 DESI 根** | `build_cata_cascade_plan` → `CascadeExpand` → `expand_live_reverse_cascade`（`ref_rev` 反查） | ⚠️ **未闭环，但不再是哑的** | 老规划器有；direct 按需路只剩 `ModelTarget.catalogue` 指纹失配（= 全库根重算，不是反查）。启动复核此前对 CATA **完全零感知**（09-04 审核 #4）——本文当天补上**点名**（§2.7）：启动横幅与 `/health.startup.reconcile.catalogue_cascade_degraded` 说出「上一次发布用的目录版本与此刻文件上的不是同一版」。**反查本身（S4 / 09-02 计划 P1-4）仍未做**，这是清单上唯一剩下的洞。**★0922 外项目 CATA（GitHub #42）**：闭包项目的库目录就是监控目录，其 CATA 正本与主项目同路进 `catalogue_observed` → Full sweep 收尾核对，**观察得到**；两条边界——MDB 没声明、只有 `Dependency` 路由的 CATA 核对解不出文件（落 `unreadable`、不级联）；同号跨项目的正本选法（sweep 按 `catalogue_project_priority`、生成按 `preferred_project`）不是同一函数。见 ADR-078 追记 2026-09-22 |
| 5 | 连接/邻居（`HREF`/`TREF`/`CREF`/`CONN` → 邻居件与隐含管段） | 同一套 `ref_rev` 通用反查（`value_ref_targets` 收所有 `RefU64` 类属性，不走白名单） | ✅ **已结：构造性不可达 + 292 库实测零反例**（§2.8 / §2.9） | 探针 `probe_chain_for_connection_change` 扫 **292 个 DESI 库 / 44 个连接改动窗口：0 个能判别**——变的一律是 **BRAN 自己的 `HPOS`/`TPOS`**。core 侧取证坐实 **`HPOS`/`TPOS` 是存储属性、没有注册任何伪属性插件**：接管时 `MDR_ApplicationManager` 把位置回写进 BRAN 记录，图形侧读的也是这个存储值 ⇒ 画的与存的是同一版，「接头动了、管路没跟着动」这条分歧不可达。探针留作回归 |
| 6 | 派生几何单元（BRAN/HANG 的隐含直管段）的成员纯位姿 | `reroute_derived_geometry_units`（把便宜路径改判成整根 regen） | ✅ 闭环 | issue #5；排在 `mask_details_to_regen` 之前，钉了顺序 |
| 7 | 房间归属（改名 / PANE 搬迁 → 整间重算） | `collect_room_structural_triggers` → `RoomRecalcPanel` / `RoomRecalcElement` | ✅ 闭环 | ADR-010 D12；枚举失败降级为 durable 全量重建信号（ADR-040） |
| 8 | 空间树 / AABB 回填 | `PostRegenAabb` + `update_inst_relate_aabbs_by_refnos_with_spatial_tree` | ✅ 闭环 | ADR-045 |
| 9 | UDA 定义依赖递归 | ❌ 不做 | ✅ **已记录为决策** | core 有（§2.4）：`DB_UserChanges::attributeModified` 沿依赖递归，边来自 `DB_Uda::internalReadData` 灌的反向依赖表。今天 UDA 不参与几何，明确放弃 |
| 10 | `XRPNTR` 交叉引用指针（正向：`el` → `el.XRPNTR`） | ❌ 不做 | ✅ **已记录为决策**（ADR-061，本文当天补） | 四条理由：那条递归属会话内编辑账本、不属 GETWORK 刷新（§2.1–§2.3 实读的 GETWORK 全程**没有任何依赖递归**）；`XRPNTR` 的 DCHC = 0；1878-noun 快照里**无属主**；反向那一侧 `ref_rev` 本就通用覆盖，缺的只是 core 那条正向边。带四条复议触发条件 |
| 11 | `graphicsBehaviour` 中间祖先（几何数据容器不独立成件） | e3d-model `category::graphics_ignored_below`（gb==1），三处共用：`nearest_unit` 跨过 gb==1 祖先丢其下单元 / `collect_unit_subtree` 不进 gb==1 之下 / `pipeline::push_members` 把 gb==1 成员整棵计进 `consumed` | ✅ **已落地并已提交**（e3d-model `5da8510`；本文核实） | 09-02 计划 S5 / P1-2b **已做完**，含矩阵 `gb` 列、`graphics_behaviour()`、109-noun 护栏。本文当天补的是 **gb==2 那一档护栏**（见下行与 §2.6） |
| 11b | `graphicsBehaviour == 2` 层级容器永不单独重画 | e3d-model 由 `classify` 收成 `List` / `Composite` 承担（无单独分派） | ✅ **本文当天补上护栏** | core `0x1052f2e0` 的 `if (gb != 2)` 门（§2.3）。字典给 98 个 gb==2 里的 **10 个**打了 `primitive` 位（`CSURF` `PLTGRD` `HRPANE` `STRLNG` `CPANEL` `INTFRM` `HICPAN` `ENVLOP` `HTFEAT` `STRFLT`）——今天靠 `classify` 的字典兜底臂不成件，是**巧合不是保证**。新测试 `graphics_behaviour_two_containers_are_never_units` 钉住 |
| **12** ★0906 | **结构连接**（`SCTN`/节点：`CREF` ↔ `JOIS`/`JOIE`、`CFRA` ↔ `PFRE`，双向核对后联动对端） | 旧 PE 路径：`ref_rev` 按 schema ELEMENT 型收边，对端交付单元级联（超集）；**direct 文件窗口路径：★0907 已接线** `src/data_interface/structural_connection.rs`（正向读 + 回指核对 + `STRU` 门，对端 owner 链上的根并进 `touched`），开关 `model_structural_connection_cascade` 默认关。**★1112 同库「带目录正体的 `PJOI` ↔ 认领它的 `SCTN`」两个方向、以及「PNOD 挪了 → 它名下的主关节」改由 e3d-model `plan_update` 自己的关节级联在单元这一层接住（与 FIXING ↔ GENSEC、SJOI ↔ SCTN 同一条腿，开关关着也重算）** | ~~❌~~ ◐ **direct 已补、灰度中**（0907 真库坐实缺口，同日接线：`aps7200_0001` 3→4 regen 6→8、`aps250110_0001` 29→30 regen 251→258、九钉死窗口两态逐字相同）——今天几何上不显（e3d-model 第 4 阶段关节负体未接），第 4 阶段落地即成正确性项 | core `change/ADSTCH`（Core3D `0x1022D774`），从变更元素**正向读** `CREF`/`JOIS`/`JOIE`/`CFRA` 再核对回指——不需要反向表，direct 规划器补一步正向读即可对齐。取证与规则表：`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §3 / §6。**真库普查 `docs/evidence/2026-09-07-cross-root-reference-census.md`**：486 库 55,503 条边，**21,161 条回指成立且对端不在变更元素任何根之下**（`PJOI ↔ SCTN` 14,577、`SJOI ↔ SCTN/GENSEC` 4,484、`FIXING ↔ GENSEC` 2,100，100% 在 `STRU` 下），10 个库有只改一端的窗口（推荐 `aps7200_0001` 3→4、`aps250110_0001` 29→30）。→ 联动对齐计划 **C2 立项**。**★0922 跨库对端（GitHub #33）：读得到、上卷不到**——生产 `DbSet@T` 注册 MDB 全部 DESI，`backref_holds` 对别的库照样读到（AvevaPlantSample 98 库 12 条跨库 `FIXING.CREF ↔ GENSEC.JOIE` 边 12/12 回指成立），但 `collect` 拿的 `owners` / `roots_T` 只有窗口那一个文件，对端根 0/12 进 `touched`、记在 `far_end_without_root` 里与「对端已删」混格；对端库自己的索引能找到根 12/12。覆盖它要跨 dbnum 工作交接（与房间 `RoomRoots` 停放同型），翻默认（#34）前先拆账。**★0930 拆账已做**：会级联的跨库对端记进 `far_end_cross_db`、回执逐条点名，不再混进 `far_end_without_root`（只记账、不上卷）。**★1001 跨库交接已做（#34 前置 2）**：计划带着跨库对端（`ModelUpdatePlan::cross_db_elements`），窗口落地之后 `src/data_interface/cross_db_handoff.rs` 在对端库的文件最新会话上折根（owner 链上每一个根），给那个库排一条只带名单的 `ModelOnly`（目录级联同一条路）；#33 探针复跑：12 个跨库对端全部折得出根（8 个对端库、18 个根，`FRMW` / `STRU` / `SUPPO`），与对端库自己算的逐字相同。同日复跑：同库 FIXING↔GENSEC 由 e3d-model `plan_update` 自己的关节级联接住（`joint_consumer_of` / `joint_fixings_of`，09-27 / 09-29），开关关着也重算——`aps250110_0001` 29→30 两态都是 258 根；`SJOI` / `PJOI` → SCTN 仍只有本开关补得上（`aps7200_0001` 3→4 关 6 / 开 8），SCTN 关节负体 e3d-model 未接、几何仍不显 |
| 13 ★0906 | 设备连接表（`EQUI` / `ELCONN` 变了 → ~~`CLIST` 里的元素~~ **★0907 C6：`ELCXRF` = 引用表反向找 `STAREF`/`ENDREF` 指向它的元素 = 接在它上面的 `CABLE`**，全部重画） | direct 文件窗口路径：**★0907 同库版已接线** `src/data_interface/cable_endpoint.rs`（变更的 `EQUI` / `ELCONN` / `CWBRAN` 到 T 端文件自带的 `STAREF` / `ENDREF` 反向表点查引用它的电缆，电缆 owner 链上的根并进 `touched`），开关 `model_cable_endpoint_cascade` 默认关；**★1001 跨库版已接线（#35）**：同一批键到 MDB 其余设计成员各自的 `STAREF` / `ENDREF` 表点查（`read_foreign_referrers`，成员名单同 P4），别的库里的电缆交给跨库交接（`cross_db_handoff`）；打不开的成员点名、不判退化。真库（APS 98 库）：电缆正向读出跨库端点边 196 条（= C6 普查数）；假定 8 个端点 `ELCONN`（250154 / 250160）变了，各自查回电缆库 250176 的那根 `CABLE` 并折回它的根。旧 PE 路径：`CABLE.STAREF`/`ENDREF` 是 schema ELEMENT 型 → 进 `ref_rev` → 电缆单元级联（超集） | ~~❓ 待核~~ ~~❌ direct 缺~~ **◐ direct 同库版已补、灰度中（0907 C8）**：`zdj7200_0001` 假定 `EQUI 23584/10206` 变了 → 6 个电缆根与正向读反推的集合相等；现有窗口两态 regen 逐字相同。**语料里「只改端点、电缆不动」的自然窗口 = 0**（0907 普查补档 §7：85 库 3935 步，同库电缆端点边 444 条全跨根；验收只能靠假定源或自造窗口）。几何上暂不显（电缆几何没按路由落地：`CABLE` 现走 cata_model、`EXTGEO` 段未接），电缆几何落地即成正确性项 | core `change/EVALAT` 规则 F（~~`GATREF(CLIST)`~~ **`GATREF(ELCXRF)`**，0906 把 E/F 属性写反、0907 改正）；`ELCXRF` 是 core.dll 伪属性 `DB_PseudoELCXRF_DESI` = `BACKRE{ENDREF, STAREF}` → `DB_RefTableIterator` 逐库；证据 §8.6。**语料**：ELCONN 1607 / CABLE 116；`CABLE.STAREF`·`ENDREF` 边 36 条同库（→ `EQUI`，全部跨根）+ 196 条跨库（→ 六个电气库）；CABLE 与 ELCONN 在语料里从不同库 |
| 14 ★0906 | 桥架 / 网格族（~~`CWBRAN` `ELCXRF` 正向~~ **★0907 C6：`CWBRAN` 的 `CLIST` 伪属性 = 路过这条分支的电缆，底下同一张引用表反向查 `EXTGEO.STAREF`/`ENDREF`**、`REFGRD` 正向、`CTRAY` / `CGRDCP` / `CGRDLP` ~~反向引用者~~ **★0907 成员**） | 无 | 登记不排期；**E 那条并入 #13 的 C8 提案**（同一张引用表、同两个属性，只是从路由节点那头查；语料 `EXTGEO.STAREF`/`ENDREF` → `HATTA`/`TATTA`/`RATTA`/`RNODE` 408 条同库跨根 + 152 跨库） | 桥架族 `gb==3`，归几何覆盖度（§2.5）；规则 B–E、G 见同上文 §2.2（★0907 C/D/G 已改成成员表，证据 §8.1；E 见 §8.6） |
| 15 ★0906 | 孔洞 / 负实体（PANE 或设备子件 `EQUC` 变了 → ~~引用它的~~ **它成员里的**孔洞构件 `HOLC` 重算；孔洞自己变了 → 自身 + owner） | owner 子树内的孔由整根重生成覆盖 = **与 core 同一口径**（core 的 `DFIND` 是成员导航，不查引用者，证据 §8.1） | ~~✅ 语料上不可达（留探针，0907）~~ **✅ 构造性覆盖（0907 C7）**；真库普查另证跨根孔洞引用 = 0，两边一致 | core `change/EVALAT` §2.3 第 3 条 + 规则 J / P；`INCOMP` 读 noun 位 `HOLC`（就是核对表叫 `IsNegative` 的 `0x92663`）。**真库普查（同上文）**：12 个负实体 noun 发出的 9,329 条引用**没有一条指向 PANE 或设备子件**（去向是 `TMRREF`/`ORRF`/`RREF`），跨根只有 20 条 `NCYL.RREF → SUBS`（共外层根），零窗口。HOLC 位本身未导，用 `positive_equivalent != 0` 代理；出现 Hole Management 语料再复跑 `cross_root_reference_census` |
| 15b ★0930 | Hole Management 穿孔件（`PFIT` 引用被穿件——管道等挪位，孔要跟着动；GitHub #43） | 无专门路径：穿孔件与被穿件同根时整根重生成覆盖，跨根时 direct 不会把穿孔件的根拉进来 | **登记不排期**：普查今天 = 0 | `cross_root_reference_census` 加了 `PFIT` 一列（模块头第 7 条）。**★0930 全语料**（E3D3.1 六个工程 AMS / APS / SCB / TEST / YCYK / ZDJ，487 个 DESI 库、85 库 4146 步）：`PFIT` 发出 632 条引用，629 条指向目录 / 属性库（`SPRE` / `MATR`），3 条同库 `PFIT.CREF → FITT`（`zdj7209`，是结构连接边，归 #12 / C2），**没有一条指向 DESI 设计元素（被穿件）**；PFIT 窗口 0。HOLC 仍按 12 个负实体 noun 代理：9,329 条引用、跨根 20 条 `NCYL.RREF → SUBS`，与 0907 一致。**复议触发**：普查 PFIT 列出现指向 DESI 设计元素的边、或 `pfit_dst_only` 窗口 > 0 → 立项（形状同 #12：被穿件变了，把穿孔件的根并进 `touched`） |
| 16 ★1111 | 支撑件的管外径（`ANCI` 一族经 `COMPRE` 指着被支撑的构件，读它的 `LSTU` / `HSTU` 走到管子的目录件；被支撑的构件换了规格，支撑件要跟着重算；GitHub #143） | direct 文件窗口路径：`reverse_reference::DESIGN_CASCADE_TARGET_READS`——设计侧一跳（P4）查到 `COMPRE` 的引用者之后，只在被引用件这两格窗口两端取值不同时才算：同库支撑件的根并进 `touched`，别的库里的支撑件随计划交给跨库交接（`cross_db_handoff`）——反查超点查预算的窗（成批换规格）`COMPRE` 表整张走一遍，交接照常；启动目录复核（P3）的设计侧多走一跳（经 `LSTU` / `HSTU` 表命中的设计件 → 经 `COMPRE` 指着它的支撑件）。挂在 `model_design_reference_cascade`（默认开）下，没有单独的开关 | ◐ **已接线，未上真机** | APS 样例工程 234 条 `COMPRE` 边（224 条跨库：支撑库指着管道库），AMS 0 条；被指构件只挪位置 / 改名不触发。被指构件所在的库不在监听范围时没有窗口、支撑件不跟（与 #12 / #13 的跨库同一限制）。e3d-io 在 `COMPRE` 的目标上读哪几格由测试 `the_conditional_list_matches_what_e3d_io_reads_on_the_referenced_component` 钉住 |

统计（2026-09-04 收口后）：**8 闭环（含 #5 / #11 / #11b）/ 2 已记录为决策·不做（#9、#10）/
1 有机制、验收门本语料压不出来（#3，已留证）/ 1 洞（#4 的反查那一半）**。
★0907 追加：**#12 由「候选」坐实为洞**（真库 21,161 条边 + 真实窗口，C2 立项）→ **同日 C2 接线落地**（`structural_connection.rs`，开关 `model_structural_connection_cascade` 默认关，两真库窗口 + 九窗口两态验过）；**#15 ~~判语料上不可达~~ → 同日 C7 改判构造性覆盖**（core `DFIND` 是成员导航，`EVALAT` 里没有反向引用查询，证据 §8.1）；~~#13 / #14 状态不变~~
**同日 C6：#13 由 ❓ 定为「反向边、direct 缺」→ 对齐计划 C8（几何上暂不显）**，#14 的 E 条并入——`CLIST` / `ELCXRF` 都是伪属性、底下是
`DB_RefTableIterator(STAREF | ENDREF)`；0906 把 E/F 两条的属性写反了，已在证据 §2.2 / §8.6 改正。**同日 C8 同库版接线落地**
（`cable_endpoint.rs`，开关 `model_cable_endpoint_cascade` 默认关；#13 ❌ → ◐），跨库版待立项。#14 其余状态不变。
**同日普查补档**（`docs/evidence/2026-09-07-cross-root-reference-census.md` §7）：同库电缆端点边 444 条（F 36 / E 408）全跨根、
反向全不覆盖，但 85 库 3935 步里「只改端点、电缆根没被碰」的窗口 **0**（另 15 边·步电缆当时还没建，不算）——#13 的真库验收暂无自然窗口。

本文落盘当天做掉的：#10 从「连状态都没有」收成 ADR-061；核实 #11 早已落地（此前误记为
「在案未做」）；#11b 的护栏补上；#4 的「点名」半步落地（§2.7）；#5 **彻底结掉**——
先证实本语料压不出验收窗口并留下探针（§2.8），再从 core 侧证实 `HPOS`/`TPOS` 是存储属性、
这条分歧构造性不可达（§2.9）。
**剩下真正没收口的只有 #4 的反查本身一条。**

---

## 2. 本轮 ida-bridge 新增取证

09-02 那份文档取的是**数据层**（`DB_Compare` / `DB_UserChanges` / `DB_RawChanges` 的桶与判据）。
本轮补的是**图形层**——即 core 拿到一份变更集之后，究竟把它扩散成哪些重画，这正是「联动更新」的
权威口径。四条都是既有文档里没有的。

### 2.1 ★ 找到 GETWORK 的级联入口：`DDES_GraphicsMngr::postGetWork`

Core3D `sub_1052E9F0(dbs, rawChanges, op)`（`0x1052e9f0`），按 `op` 三分（`MTR_Entry` 的
埋点字符串就是函数名，直接坐实）：

| `op` | 埋点名 |
|---|---|
| 4 | `DDES_GraphicsMngr::postGetWork` |
| 5 | `DDES_GraphicsMngr::postUndo` |
| 6 | `DDES_GraphicsMngr::preRedo` |

`postGetWork` 的四步：

```
1. 从入参 db 列表里，只挑 DB_DB::type ∈ {1, 6} 的库           ← §2.2
2. DB_RawChanges::trim(rawChanges, 那个库列表, trimmed)         core.dll 0x5984650
3. if (全局开关 byte_10E3C904) sub_1052EFD0(trimmed)            ← 新建件建图形
4. sub_1052F2E0(trimmed)                                        ← 变更件重画扩散，§2.3
```

第 3 步 `sub_1052EFD0`（`0x1052efd0`）遍历变更集，**只对
`DB_RawChanges::isElementTopLevelCreate` 为真的元素**建图形——即新建子树只建顶，
与 e3d-model `ledger` 的抢占口径一致。

### 2.2 ★ core 的 GETWORK 重画**只吃 DESI 与 ISOD**，CATA 不进这条路

`DB_DB::type` 的枚举值本轮从 `DB_DatabaseTypes::LoadDatabaseTypes_Generated`
（core.dll `0x5a2a9bb`）逐条读出（`DB_DatabaseType` 构造的第 2 个实参就是类型号）：

| 号 | vir 模板 | 库 | | 号 | vir 模板 | 库 |
|---:|---|---|---|---:|---|---|
| **1** | `desvir` | **Design** | | 8 | `dicvir` | Dictionary (UDAs) |
| 2 | `catvir` | Catalogue | | 9 | `engvir` | Engineering |
| 3 | `comvir` | Communications | | 10 | `sysvir` | System |
| 4 | `provir` | Property | | 11 | `glbvir` | Global |
| 5 | `misvir` | Miscellaneous | | 12 | `trnvir` | Transaction |
| **6** | `isovir` | **Isodraft** | | 13 | `alyvir` | Analysis |
| 7 | `padvir` | Draft | | 14 | `schvir` | Schematics |

（后续 15–20 = `manvir` / `nsqvir` / `xprvir` / `cfgvir` / `tasvir` / `rdfvir`。）

所以第 1 步的 `{1, 6}` = **DESI + ISOD**。**目录库（2）的会话推进根本不会进入 GETWORK 的重画扩散**。

对我们的意义（这条要写进结论）：

- 它**独立第二次**证实了 09-02 §1.4 从依赖表侧得到的结论——core 里目录件变了对设计件的影响
  **不是**靠变更扩散做的，只能靠缓存整体失效 + 用户重新导航；
- 因此 gen-model 的 CATA 反向级联（表 #4）**是比 core 更强的能力，不是补 core 的欠账**。
  这反过来意味着：**它没有 core 侧的对照物可以对拍**，它的正确性只能靠我们自己的验收门守。
  今天那道门在 direct 按需路和启动复核上都是缺的（09-04 审核 #4），这就是表 #4 判「未闭环」的理由。

### 2.3 ★ 变更 → 重画的完整判据：`sub_1052F2E0`（Core3D `0x1052f2e0`）

第一段，逐个变更元素（逐指令读出的谓词，`gb(x)` = `DB_Noun::getField(x.actualType(), 5099119)`）：

```
重画 el  ⟺  el.type ∉ {WORL, MNUM}
             ∧ ( gb(el) ∉ {1, 2}
               ∨ ( gb(el) == 1 ∧ gb(el.owner()) ∉ {1, 2} ) )
```

三条可直接落成守卫的事实：

1. **`gb == 2` 的元素永不单独重画。** `2` = 层级容器（`WORL` `SITE` `ZONE` `ROOM` `STRU*`
   `HPANEL`…，98 个 noun），容器自身不是可画项。**`gb == 3` 在本判据里与 `0` 同待遇**（照常重画）
   ——`gb == 3` 的完整结论见 §2.5，09-02 文档 §5 开放问题 4 就此**两半都结掉**。
2. **`gb == 1` 的元素不是无条件丢弃，而是降级看属主一层**：属主是普通可画件（gb==0）时它照样重画。
   这与 `DES_DrawList::isGraphicsIgnoredBetween`（`0x1051c7d0`，上溯到顶层单元的**多层**walk）
   不是同一条规则——两条并存，作用在不同环节。P1-2b 落守卫时要按环节分开抄，别把两者合成一条。
3. `WORL` 与 `MNUM` 两个 noun 硬编码排除。

第二段做**祖先抢占**：对结果集逐个取 `owner`，在同一集合里 lower_bound 查找，**只有 owner 不在
集合里的元素**才发事件（`sub_1022C679`，事件号 `331445106`）。
~~——**未逐指令核**：该集合的填充点 Hex-Rays 没还原出来（`v46` 在第一段里看不到插入），
第二段的输入究竟是「第一段重画过的元素」还是别的集合，本轮没坐实。不得当已证。~~
**★0906 两处都坐实了**（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §5）：
(1) 填充点在 `0x1052f481`（`ecx = &v46` 的 `std::set::insert`），插进去的是 **`desdblib/FNDTOP(el)`**——按
`toplevel` 位折叠出的顶层可画件，不是 `el` 自己；(2) `sub_1022C679` **不是发事件，就是 `change/EVALAT`**
——DCHC 驱动的联动规则表，以 REDRAW=4 为类别对每个存活顶层件跑一遍（含 ~~EQUI `CLIST`~~ **EQUI/ELCONN 的 `ELCXRF`、CWBRAN 的
`CLIST`**（★0907 C6：两个都是伪属性、底下是引用表反向查电缆，0906 写反了属性，证据 §8.6）、~~设备子件 / PANE 的引用者~~ **设备子件 / PANE
的成员**（★0907 C7）等规则）。所以本节开头「没有任何依赖递归」只对过滤器自己成立，**对整条 GETWORK 路不成立**。

### 2.4 `DES_DrawList::hasGraphicsIgnoreFlag`（Core3D `0x1051a920`）

`isGraphicsIgnoredBetween` 的兄弟：只问**元素自己**的 noun `gb == 1`，不上溯。
09-02 文档只有上溯那一个。两个都在 `DES_DrawList` 私有段，说明 core 在画列表这一层同时需要
「我自己是几何数据容器吗」与「我头上有几何数据容器吗」两个问题——P1-2b 的
`graphics_behaviour(noun)` 要同时支撑这两问。

### 2.5 ★ `graphicsBehaviour == 3`：**全产品无任何代码路径**，对级联规则零影响

09-02 文档 §5 开放问题 4 的后半。分两侧查，结论一致。

**代码侧（穷举，不是抽样）**：

| 查法 | 结果 |
|---|---|
| 全 E3D 2.10 安装扫 `push 4DCE6Fh`（`68 6F CE 4D 00`），**868 个模块 / 0.65 GB** | **只有 `Core3D.dll` 命中**（2 处；3.1 版 Core3D 是 4 处） |
| 全安装扫导出符号 `?graphicsBehaviour@DB_Noun@@QBEHXZ` 的导入方 | **零**——只有 `core.dll` 自己的导出表里有这个名字，没有任何模块 import 它 |
| 3.1 版 Core3D 的 4 个读点 | 全在 §2.3 / §2.4 的三个函数里，**只与 `1` 和 `2` 比较** |
| core.dll 内部 | 常量 `5099119` 只出现在 `DB_Noun::ReadData`（`0x58d6f45`，把字典字段写进 `this+180`）与 `DB_Noun::getField`（`0x58d8d69`，取 `this+180` 原样返回，无变换）；命名 accessor `DB_Noun::graphicsBehaviour`（`0x58d9760`）**没有任何内部调用方** |

⇒ **产品里没有一处把 `graphicsBehaviour` 与 `3` 比较。** 对每一个消费点，`3` 与 `0` 完全等价：
**照常作为独立可画项重画，也不会让子孙被忽略。**

**数据侧（那 72 个 noun 是什么）**：拿 `noun_flags.json`（1931 noun，分布 0/1/2/3 = 1652/109/98/72）
与 e3d-model 的 `data/noun-family-matrix.json` 对表，`gb == 3` 几乎正落在
**「应用侧接口件」**那几个族上：

| e3d-model family | 数量 | noun |
|---|---:|---|
| `ICABCO`（电缆 / 桥架 / 附着点） | 17 | `CABLE` `CWBRAN` `CT{STRA,BEND,TEE,RISE,JOIN,SUPP,COUP,CROS,REDU}` `PULLN` `JNODE` `RNODE` `HATTA` `RATTA` `TATTA` |
| `IPFCOM`（型材 / 船体结构件） | 20 | `IPLATE` `ISTIFF` `ISTRU` `ISTRUR` `IPILLR` `ISEAM` `IEDGE` `IHOLE` `INOTCH` `ICLIP` `ICTOUT` `IBNDRY` `ILIM` `INODE` `IFCPLA` `RLADDR` `SLADDR` `RLGATE` `HRGATE` `LDRRUN` |
| `IFCOMP` | 3 | `SPLO` `IPANE` `ISECT` |
| `IFCOMP` + `IPFCOM` | 1 | `STWELD` |
| **（无 family）** | **31** | `AID{ARC,CIR,LIN,POI,TEX}` `SRF{CUR,LIM,RFL,SUR,TRA}` `GEN{NC,NG,PC,PG}` `GRID{CY,PL}` `REF{GAR,GLN}` `ICR{CUR,PNT}` `HANDRA` `WKSUR` `MLABEL` `LINDIM` `ENVLIM` `HULCON` `TRNCUR` `IDLWLD` `RSFINS` `CNODE` `DTJOB` |

（另：72 个里 59 个 `primitive == true`。）

语义读作：**`3` = 「照画，但画法由应用侧的专用绘制路径提供，不走标准目录 / 图元几何管线」**
——电缆桥架、船体型材、扶手爬梯、辅助图元、标注、网格、参考线、附着点。

**对我们的两条结论**：

1. **P1-2b 的守卫不用管 `gb == 3`。** 它不是级联问题：`gb == 3` 既不改变上卷归属、也不让子孙被忽略，
   把它当 `0` 就是与 core 逐位一致。P1-2b 要挡的只有 `1`（几何数据容器）与 `2`（层级容器）。
2. **`gb == 3` 是几何覆盖度问题，不是增量问题。** 它标出的是「e3d-model 会不会造这个件」
   （`CTBEND` / `IPLATE` / `HANDRA` 各有专用画法），归 e3d-model 的几何覆盖线，不归本文这张表。
   e3d-model 的 family 分类已经自发地把 41/72 收进 `ICABCO` / `IPFCOM` / `IFCOMP`，
   剩下 31 个无 family 的（辅助图元 / 曲面 / 标注 / 网格）是覆盖度的下一个问题面。

顺带：09-02 计划 P1-2b 第一条「`noun-family-matrix.json` 导出加 `gb` 列」**已经做掉了**
（矩阵里 `gb` 列在，分布与 `noun_flags.json` 逐值相等），该条可以划掉。

### 2.6 落到 e3d-model：`gb` 守卫的现状与本文补的那一档

核实结果：**09-02 计划 P1-2b 已经做完并且已提交**（e3d-model `5da8510`
「feat(increment): close the ledger, guard graphicsBehaviour==1, …」），比计划写的还全——

| P1-2b 计划条目 | 现状 |
|---|---|
| `noun-family-matrix.json` 导出加 `gb` 列 | ✅ 在（`FamilyMatrixRow.gb`，老版本缺列按 0 读） |
| `category::graphics_behaviour(noun) -> Option<u8>` | ✅ 在 |
| gb==1 的 109 个 noun `!is_model_unit && !is_derived_unit && classify != List` | ✅ 有测试 `graphics_ignored_containers_are_never_units_nor_lists`，109 这个数字写死 |
| `nearest_unit` 跨过 gb==1 祖先 | ✅ `if depth > 0 && graphics_ignored_below(&noun) { unit = None; route_container = None; }`——**只看祖先**，与 core 的 `isGraphicsIgnoredBetween` 同口径 |
| `collect_unit_subtree` 不进 gb==1 之下 | ✅ |
| （计划里没写）`pipeline::push_members` 把 gb==1 成员整棵计进 `consumed` | ✅ 全量与增量共用同一判据 |

所以表 #11 此前记的「❌ 缺 / 在案未做」是**错的**，本文已改。

**本文当天补的是 gb==2 那一档。** core 的判据里 `gb == 2` 有独立的一道门
（`if (Int != 2)`，§2.3），语义是「层级容器永不单独重画」。e3d-model 这边由 `classify`
把 `WORL` / `SITE` / `ZONE` / `ROOM` / `STRU*` 收成 `List` / `Composite` 顺带承担了，
**没有任何东西钉住它**。而它不是空转：把 98 个 gb==2 noun 与矩阵的 `primitive` 位对表，
**有 10 个同时带 `primitive`**——

```
CSURF  PLTGRD  HRPANE  STRLNG  CPANEL  INTFRM  HICPAN  ENVLOP  HTFEAT  STRFLT
```

今天它们都落在 `classify` 的字典兜底臂（`Composite` / `Unknown`）上因而不成件，但那是
**巧合不是保证**：谁照着 `primitive` 位往 `classify` 里补一条 `Primitive(_)` 臂，就会让这些
容器静默出几何——与 `SBFI` 那次静默漏件同型，只是方向相反（那次是少建，这次是多建）。

新增测试 `category::tests::graphics_behaviour_two_containers_are_never_units`：98 这个数字写死，
全部 gb==2 断言 `!is_model_unit && !is_derived_unit`，那 10 个逐个点名。
同时把 `graphics_behaviour` 的文档注释里那句**已经过期的**「2 / 3 的含义未读，不要拿来分派」
换成四档语义与 core 判据原文，并写明 **`3` 不该有对应护栏**——为它加分派才是分歧（§2.5）。

### 2.7 落到 gen-model：启动复核的目录那一格（第 4 类的「便宜的半步」）

09-04 启动增量审核 #4 说的是：一级过滤只比 `gen_root.source_end_sesno` 与 **DESI** 文件最新，
非 DESI 窗口返回空计划——**目录库前移了，启动一根都不排，而且一声不吭**。
direct 形态下连 CATA 数据批次都没有，唯一的兜底是按需 `ensure` 撞上 `ModelTarget.catalogue`
指纹失配，那要等有人点到那个根。

本文当天做掉审核里写的「便宜的半步」——**只点名，不排工作**（`model_update_pending.rs`）：

| 件 | 做法 |
|---|---|
| 上一次发布落在哪一版目录 | `published_catalogue_sessions_on`：读 `gen_root.published_model_target.catalogue`，`CATA dbnum → 记在案的**最旧**会话`。一条 `GROUP BY catalogue` 服务端去重（一个库的全部根共用同一份 `ModelTarget`，几万行塌成个位数行），不是逐库点查 |
| 此刻文件上是哪一版 | `model_source::latest_source_version(dbnum)`，在 `spawn_blocking` 里一次问完 |
| 判据 | 纯函数 `stale_catalogue_dbnums(recorded, latest)`：`now > 记录值` → 过期；`latest == None`（不在当前 MDB / stat 不到）→ **单独一栏 unreadable，不算过期**；`now <= 记录值` → 追平 |
| 有多大声 | 启动横幅一行（点名 dbnum + 明说「不为此排任何模型工作」+ 人工出口）；`/health.startup.reconcile` 新增 `catalogue_cascade_degraded` / `stale_catalogue_dbnums` / `unreadable_catalogue_dbnums` 三栏 |

两条刻意的克制，都写进了注释与测试：

1. **目录那一格不参与 `result` 的取重。** `result` 说的是「这次复核本身跑成没跑成」；目录前移是
   一个已知的、在案的能力缺口，混进去会让每一次有目录变更的启动都报成失败态，人就再也分不清
   哪次是真出岔子。守卫：`the_catalogue_flag_is_named_separately_and_never_downgrades_the_result`。
2. **「已确认过期」与「不知道」分两栏。** 合成一句就是拿未知冒充结论。
   守卫：`a_catalogue_that_cannot_be_read_is_unknown_not_stale`。

踩到的一个坑记在这里（注释里也写了）：`GROUP ALL` + `array::flatten` 是**错的**——聚合函数
逐行求值之后才被 GROUP 收集，拿回来的是「数组的数组」，而且它不报错，只在反序列化那一步
变成一句看不出所以然的类型不匹配。

### 2.8 第 5 类的验收门：找过了，**本语料压不出来**

第 5 类此前判「有机制、无专门验收」，剩下的活是「找一个真库窗口」。找了，结论是**找不到**，
而且这是个有数的结论，不是没找着。

新探针 `e3d-model/tests/increment_real.rs::probe_chain_for_connection_change`
（`#[ignore]`，与既有三条探针同款：只做 `collect_window` + `plan_update`，一件几何都不建）。

它为什么值一条自己的探针，而不是复用 `probe_chain_for_cascade`：BRAN 的头尾位置从
`HREF`/`TREF` 指向的 NOZZ 上取，而 **NOZZ 挂在 EQUI 子树里，跟 BRAN 不在同一条 owner 链上**。
变换级联走的是链**内**的后代，这条走的是链**外**的引用——结构上不是一件事。

命中判据（打 ★）：窗口里有 NOZZ 候选 / 有候选的连接类属性变了 / 有「名下带 NOZZ 的元素挪了位」，
**而 `plan.regenerate_derived` 为空**（一条路由容器都没进重算集）。

`PROBE_LIBS=ALL_DESI` 全语料实测（**292 个 DESI 库，7 秒**）：

| 数 | 含义 |
|---:|---|
| 44 | 连接改动窗口 |
| **0** | 命中（★）——**每一个窗口都有路由容器进了重算集** |
| **0** | NOZZ 自己的记录变过的窗口 |
| **0** | 「名下有 NOZZ 的元素挪了位」的窗口 |

44 个窗口的形状高度一致：变的是 **BRAN 自己的 `HPOS` / `TPOS`**（例：ams8000 277→278 的
`24384/26281`、213→214 的 `24384/23257`）。**E3D 把接好之后的头尾位置回写在 BRAN 的记录里**
——BRAN 因此是它自己的索引候选，路由容器照常进重算集，「接头动了、管路没跟着动」这个形状
在本语料里**一次都没有发生过**。

这与第 3 类改挂那条是同一类结论（`Shape::Reparented` 的文档里已经有过一次）：
**没有语料窗口能把这条联动做成判别性验收。**

### 2.9 ★ 第 5 类结掉：`HPOS`/`TPOS` 是**存储属性**，不是由 `HREF` 解出的伪属性

§2.8 留的那个 core 侧问题当轮查掉了。四条实读证据：

| # | 事实 | 地址 |
|---|---|---|
| 1 | `ATT_HPOS` / `ATT_TPOS` 是 core.dll 导出的 `DB_Attribute const*` 全局 | core.dll `0x641fcac` / `0x641ff90`（`.data`）；Core3D `0x10ae8c34` / `0x10ae9888`（`.idata`） |
| 2 | **core.dll 里 `ATT_HPOS` 只有一条 xref，而且是「写」**——初始化那一下（`sub_56D3710`）。**零次读**，所以 core.dll 内唯一的 `addGetPosPlug` 注册点 `sub_5A60CA0` 没有给它挂伪属性 | 写点 `0x56d379a`；注册点 `0x5a60f14` |
| 3 | **Core3D 的三个位置伪属性注册点，一个都不碰 `ATT_HPOS`**（把两张 xref 表一交，函数集合不相交） | 注册点 `sub_10425CD0` / `sub_105EF3F0` / `sub_1062CCE0`；`ATT_HPOS` 读点见下 |
| 4 | **写它的是「接 / 断」那组操作**：`MDR_ApplicationManager::disconnectHead` / `disconnectTail` 对 `ATT_HPOS` 调 `DB_Element::putAtt`；读它的一路是普通 `DB_Element::getAtt`，包括 `GFX_GraphicsManager::Update`、`FAB_PipePieceManager::isValidTubeLength`、`PDMS_PipeElement::getAPOS` | `0x105df4d0` / `0x105df96b`；`0x1079722d` |

⇒ **`HPOS`/`TPOS` 没有注册任何 get / put 伪属性插件**，`DB_Element::getAtt(el, ATT_HPOS)` 取的就是
记录里存着的那个值。接管的时候由 `MDR_ApplicationManager` 把解出来的位置**回写进 BRAN 记录**，
之后所有人（包括图形侧）读的都是这个存下来的值。

**这就把第 5 类结掉了**，而且是从两头夹住的：

- **接头动了、E3D 回写了 BRAN** → BRAN 记录变 → 它是自己的索引候选 → 我们照常重算。
  这正是 §2.8 那 44 个窗口的形状。
- **接头动了、E3D 没回写** → 那 E3D 自己也照着那个陈旧的 `HPOS` 画（图形侧读的是同一个存储值），
  **我们跟它画的是同一个东西**——不是分歧。

也就是说：只要画的那一版与存的那一版是同一个（证据 2/3/4 说它是），**「接头动了、我们的管路没跟着动」
这条分歧在 SAVEWORK 语义下构造性地不可达**。第 5 类从「有机制、无验收」结成
「**已结：构造性不可达 + 292 库实测零反例**」，探针留作回归。

**不许当已证**：`PDMS_HposElement::getHPOS`（Core3D `0x106690c0`）是一次 vtable 派发（槽 `+64`），
本轮**没有**把那个槽解出来；上面的结论走的是「伪属性注册表里没有它」这条路，不是「读了那个槽」。
两条路指向同一个答案，但它们不是同一条证据。

### 2.10 第 4 类反查的验收窗口：**不缺，31 个**

第 4 类剩下的是反查本身（S4 / 09-02 计划 P1-4）。动手之前先问一句「有没有窗口能验收」——
毕竟 #3 与 #5 都是栽在这上面。答案是**不缺**。

新探针 `probe_catalogue_chains`（`e3d-model/tests/increment_real.rs`，`#[ignore]`）：
逐个目录库打链长，`PROBE_CATA_ALL_WINDOWS=1` 走完整条链，把「只改了个位数件」的窗口打 ★
（判据：`modified > 0 且整窗候选 ≤ 50`——只卡增删会把 8 万个 Modified 的导入窗口也标成小改动）。

实测（**30 个 CATA 库，全部 ≥2 环**，4 秒）：

| 库 | 形状 |
|---|---|
| **`ams5052_0001`（链 34 环）** | **24 个窗口只改 1–3 件**，例如 `184→185` 恰好 **1 个 Modified**、`186→187` 2 个、`183→184` 3 个 |
| `ams6890_0001`（链 11 环） | 4 个小窗口：`269→270` 1 件、`268→269` 6 件、`267→268` 16 件、`265→266` 19 件 |
| `ams5054_0001`（链 11 环） | `42→43` 恰好 1 件 |
| `acp7041_0001`（链 3 环） | `2→8` 共 2 个候选 |
| 其余 26 个库 | 链头一律是「一次性导入」（`1→N` 或 `2→N`，几万到上百万个 Inserted），**不能拿来当门** |

合计 **31 个小改动窗口**。`ams5052_0001 184→185`（恰好一个 Modified 目录件）就是 P1-4 那句
「改一个 SCOM 参数的 CATA 窗口，只有引用它的 BRAN 成员所在根被重算」的天然形状。

**所以第 4 类与 #3 / #5 不是一回事**：它不是「没有窗口」，是**东西还没做**。挡在前面的是反查本身
（e3d-io 的 dab 反向引用表读法，或开库时一次全库扫描建内存反向表——09-02 计划 P2-2 / P1-4 的
①②两选）。

**下一个要问的问题（本轮没答）**：`ams5052` 有没有被本语料里的哪个 DESI 库引用？没有的话这道门
是空转。这一问本身就需要反查表，所以它是 P1-4 第一步的副产品，不是前置。

### 2.11 ★ core 的 CATA→DESI 反查怎么做：**读 dab 文件自带的引用表，不扫库**

第 4 类那条反查，core 侧的做法本轮查清了，顺手**结掉 09-02 文档 §5 开放问题 3**
（「`sub_5AAF4E0` / `sub_5AAE610` 是哪两个 dab 原语未从 opcode 名表反查」）。

两个包装函数都是 dabacon **opcode 派发**：

```
sub_5AAF4E0(dbno, attId, targetRef, &handle)  →  sub_5ADB6B0(326, …)
sub_5AAE610(handle, out_a, out_b)             →  sub_5ACABF0(328, …)
```

opcode 名表在 `off_6003900`（`const char*` 数组，`sub_5ACABF0` 里
`v5 = off_6003900[a1]` 就是取名字用的；**不是** `.ida_scratch/_opcodes.py` 里那张 6 字节内联表，
那是另一个 IDB 的基址）。解出来是：

| opcode | 短名 | 长名 |
|---:|---|---|
| 325 / **326** | `DFTSRC` | **`db_start_ref_table_search`** |
| 327 / **328** | `DFTNXT` | **`db_get_next_ref_table_entry`** |
| 192 | — | `db_finish_table_search` |

同族还有 `122 db_start_int_table_search` / `132 db_start_name_table_search` /
`260 db_start_sys_table_search`——**dabacon 有一整族「表搜索」原语，引用表只是其中一张。**

于是 `DB_MDB::findElements(dbType, refAttr, target)`（`0x59f4720`）的真实形状是：

```
for db in MDB:                                  # 按 db_type 过滤
    db_start_ref_table_search(db.number, refAttr.id, target.ref, &handle)   # 326
    while db_get_next_ref_table_entry(handle, &entry, &key):                # 328
        if entry.key != target: break           # 键一变就收工 → 表按 (属性, 目标 ref) 有序
        yield entry.referrer
    db_finish_table_search(handle)              # 192
```

`DB_RefTableIterator::increment`（`0x5a1b710`）里那句 `DB_Element::operator<` 判据加上
「键一变就 `db_finish_table_search`」，说明**这张表是按 (属性, 目标 ref) 排好序的**，
搜索返回的是游标而不是集合——B 树 / 有序索引的形状。表**按库文件**存（外层逐 db 迭代）。

**对 P1-4 的意义**：09-02 计划给反查源留了两条路——① e3d-io 实现 dab 反向引用表读法、
② 开库时一次全库扫描建内存反向表，计划里定的是「先做 ②」。现在有了正面证据：
**core 走的是 ①，而且是文件自带的有序表，不是扫出来的。** 这不推翻「先做 ②」的排期
（② 不需要新的文件格式知识，落地快），但它把 ① 从「也许可行」变成「一定存在、且是权威口径」，
`ams7351` 那种 334 万候选的目录库上，②（全库扫描）与 ①（点查有序表）不是一个量级。

**不许当已证**：本轮只坐实了**原语身份与调用 / 终止形状**，**没有**读出引用表的**页布局**——
e3d-io 要实现 ① 还缺这一步（09-02 计划 P2-2 的取证因此只完成了一半）。

### 附：本轮地址表

| 符号 / 函数 | 模块 | 地址 |
|---|---|---|
| `DDES_GraphicsMngr::{postGetWork, postUndo, preRedo}`（三合一） | Core3D | `0x1052e9f0`（`op` 4/5/6） |
| 新建件建图形（`isElementTopLevelCreate` 过滤） | Core3D | `0x1052efd0` |
| 变更 → 重画扩散（gb 判据 + 祖先抢占） | Core3D | `0x1052f2e0`（gb 读点 `0x1052f36f` / `0x1052f3cd`） |
| `DES_DrawList::hasGraphicsIgnoreFlag`（自身 gb==1） | Core3D | `0x1051a920`（gb 读点 `0x1051a95d`） |
| `DES_DrawList::isGraphicsIgnoredBetween`（上溯 gb==1） | Core3D | `0x1051c7d0`（既有） |
| `DB_DatabaseTypes::LoadDatabaseTypes_Generated`（类型号枚举） | core.dll | `0x5a2a9bb` |
| `DB_Noun::graphicsBehaviour`（读 `this+180`） | core.dll | `0x58d9760` |
| `DB_RawChanges::trim(vector<DB_DB*>&, DB_RawChanges&)` | core.dll | `0x5984650` |
| `DB_RawChanges::{isElementTopLevelCreate, isElementTopLevelDelete}` | core.dll | `0x5983b80` / `0x5983bd0` |
| `ATT_HPOS` / `ATT_TPOS` 全局（§2.9） | core.dll `.data` | `0x641fcac` / `0x641ff90`（Core3D `.idata` `0x10ae8c34` / `0x10ae9888`） |
| `ATT_HPOS` 在 core.dll 里唯一的 xref（初始化写点） | core.dll | `0x56d379a`（`sub_56D3710`） |
| 位置伪属性注册点（**都不碰 `ATT_HPOS`**） | core.dll / Core3D | `0x5a60f14`（`sub_5A60CA0`）/ `sub_10425CD0`、`sub_105EF3F0`、`sub_1062CCE0` |
| `MDR_ApplicationManager::disconnectHead` / `disconnectTail`（对 `ATT_HPOS` `putAtt`） | Core3D | `0x105df4d0` / `0x105df96b` |
| `GFX_GraphicsManager::Update`（读存储的 `ATT_HPOS`） | Core3D | `0x1079722d` |
| `PDMS_HposElement::getHPOS`（vtable 槽 `+64`，**本轮未解**） | Core3D | `0x106690c0` |
| `DB_MDB::findElements`（逐 db 跑引用表游标，§2.11） | core.dll | `0x59f4720` |
| `DB_RefTableIterator::{startSearch, increment}` | core.dll | `0x5a1d090` / `0x5a1b710` |
| 包装函数 → dabacon opcode：`sub_5AAF4E0`→326、`sub_5AAE610`→328 | core.dll | `0x5aaf4e0` / `0x5aae610` |
| opcode 派发器（`v5 = off_6003900[a1]` 取名） | core.dll | `0x5adb6b0` / `0x5acabf0` |
| **opcode 名表**（`const char*` 数组，**不是** 6 字节内联表） | core.dll | `off_6003900`（326=`db_start_ref_table_search`、328=`db_get_next_ref_table_entry`、192=`db_finish_table_search`） |

---

## 3. 要补什么才算「梳理完毕」

按代价从小到大。前两条是纯文档，**2026-09-04 当天已做掉**：

1. ~~**把 §1 这张表接进 `CONTEXT.md`**，作为「联动更新」的词条与唯一索引。~~
   ✅ **已做**：`CONTEXT.md`「模型生成执行」章新增两条词条——**联动更新 (Cascade Update)**
   （十一类摘要 + 指定本文 §1 为唯一索引，新增或改动任何一条联动边先改那张表）与
   **图形行为 (Graphics Behaviour)**（0/1/2/3 四档语义 + 「只有 1 和 2 需要守卫」）。
2. ~~**给表 #10（`XRPNTR`）一个明确状态。**~~
   ✅ **已做**：`docs/adr/ADR-061-xrpntr-forward-dependency-is-not-replicated.md`——不复刻，
   四条理由（层次不对 / DCHC=0 / 快照无属主 / 反向已通用覆盖）+ 四条复议触发条件。
   清单上不再有「连决策都没有」的边。
3. ~~**表 #5 补一道验收窗口**~~ ✅ **已结**：本语料压不出验收窗口（§2.8，292 库 / 44 窗口 /
   0 判别，探针留仓），而 core 侧取证（§2.9）证实 `HPOS`/`TPOS` 是**存储属性、没有伪属性插件**
   ——画的与存的是同一版，这条分歧构造性不可达。不排期。
4. **表 #4 —— 清单上唯一剩下的洞。** ~~启动复核先把「CATA 文件身份变了」点名~~
   ✅ **点名当天已做**（§2.7）。**剩下的是反查本身**（S4 / 09-02 计划 P1-4）：对 CATA 窗口跑
   `plan_update` 得变了的目录件集合，再反查「哪些设计件经 SPRE/CATR 指向它们」。
   验收窗口**不缺**——§2.10 实测 31 个，`ams5052_0001 184→185` 恰好只改一个目录件。
   挡在前面的只有反查表本身（09-02 计划 P1-4 的 ①e3d-io dab 反向引用表 / ②开库时全库扫描建
   内存表，计划里定的是先做②）。
   **2026-09-06 进展**：① 的读法 e3d-io 已实现（`ReferenceTable` / `Engine::referrers_of`，543 库门），
   gen-model `direct_store::referrers_of` 已包好但**零调用方**；core 写侧取证补齐
   （`docs/evidence/2026-09-06-dab-ref-table-write-path.md`：引用表由 `db_put_reference` 写时同步维护，
   仅 kind-16 属性有表，`HREF`/`TREF`/`CREF` 无表）。接线与 `ref_rev` 降级的排期见
   `docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md`（待评审）。
5. ~~**表 #11 按 09-02 计划 P1-2b 落**~~
   ✅ **无需排期**：核实后 P1-2b **早已在 e3d-model 工作树里做完**（§2.6 逐条对照），
   本文当天补上缺的那一档 gb==2 护栏并把过期注释改掉。09-02 计划里的 P1-2b 整条可以划掉。

## 4. 不许当已证

- §2.3 第二段的输入集合填充点未坐实（见该节）。
- §2.2 的类型号来自 `LoadDatabaseTypes_Generated` 的构造实参位置（第 2 个实参），
  未与 `DB_DatabaseType` 的成员布局逐字段对齐；`{1, 6}` 到 `{DESI, ISOD}` 的映射据此成立，
  若该实参不是类型号则本节结论作废。表中 1/2 号与 `desvir`/`catvir` 的对应是**旁证成立**的
  （同一次调用里紧邻的 `PMLTypeString_Assign(…, "desvir")`）。
- §2.5 的穷举扫的是**本机装的 E3D 2.10**（868 模块）；3.1 的完整安装不在本机（只有
  `core.dll` / `Core3D.dll` / `libgeom.dll` 三个副本），所以「全产品无 `== 3` 比较」这句话在 3.1 上
  是**由这三个模块 + 2.10 全量**推出来的，不是 3.1 全量扫出来的。另：`core.dll` 内部若有人直接读
  `DB_Noun + 180` 而不经 `getField`，本查法看不见（命名 accessor 无内部调用方这条是查过的）。
- 本文只核「有没有这条联动、谁负责、有没有门」，**没有跑任何一条联动的实测**。
  表里所有「✅ 闭环」都是「有代码 + 有测试/实测留档」的意思，不是本轮复验过。
