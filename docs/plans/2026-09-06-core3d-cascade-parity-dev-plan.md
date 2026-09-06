# 开发计划：联动更新对齐 Core3D `change/EVALAT`（DESI 三维模型增量）

> 日期：2026-09-06 晚
> 范围（用户 22:10 划定）：**只做 DESI 三维模型的增量更新绘制。** CATA → DESI 那一类不在本计划内（归 ADR-058 实施计划）。
> 依据：`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md`（core 侧联动规则表，本计划的 oracle）、
> `docs/2026-09-04_model-cascade-update-inventory.md`（十一类 + 本轮新增 #12–#15，联动边的唯一索引）、
> `docs/specs/core3d-partial-update-conformance.md`（R1–R29）、`docs/plans/2026-09-06-core3d-partial-update-conformance-dev-plan.md`（W0–W4，本计划**不重复**其中任何一项）、
> `docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md`（反向引用表读法，本计划只**消费**它）
> 方法：先把 core 的每一条联动规则对到我们两条规划路径（direct 文件窗口 / 旧 PE）上判「有 / 无 / 超集」（§1），
> 再只对 direct 路径上的「无」立项；每项都先取证「真库里这条边存在吗」，数字为零就不写代码。
> 涉及：`src/data_interface/{window_root_plan,generation_root,manual_update,model_impact}.rs`、`src/data_interface/direct_store.rs`、`tests/`；
> C1 / C5 的探针 `src/bin/cross_root_reference_census.rs`，产物 `docs/evidence/2026-09-07-cross-root-reference-census.{md,json,log}`

## 0. 一句话

Core3D 的联动不是「依赖递归」，是一张 **noun × 属性 → 目标** 的硬编码规则表（`change/EVALAT`），外加结构连接的
双向核对（`change/ADSTCH`）。把它逐条对到我们身上：**owner 链上卷那一半我们全有**（而且 direct 路径按 `significant`
枚举比 core 的 TOPF 更细）；**引用那一半在旧 PE 路径上由 `ref_rev` 超集覆盖，在 direct 路径上一条都没有**。
direct 是今天 DESI 的生产路径，所以真正的活只有一件：**给 direct 窗口规划器补引用类联动**——先正向（结构连接，
不需要反向表），再反向（孔洞 / 通用），每一步先量真库。

## 1. 现状核对：core 的每条联动规则 × 我们的两条路径

「direct」= `window_root_plan.rs` 文件窗口规划器（`roots_S`/`roots_T` 枚举 + 祖先闭包）；「旧 PE」= `manual_update.rs`
rollup（`resolve_element_generation_root` + `ref_rev` 反查 + `build_unit_rollup`）。判词只信 grep。

| # | core 规则（证据 §） | 走的边 | direct | 旧 PE | 判词 |
|---|---|---|---|---|---|
| 1 | 类别 = 字典 DCHC（`IDCHNG`）；DCHC=0 不进队列（§2.1） | — | `classify_operation_impact` 按 `dchc_change_classes.json` + 兜底 | 同 | ✅ 同源（ADR-002） |
| 2 | `FNDTOP` 上卷到 `TOPF` 顶层件（规则 A/N/R，§4） | owner 链 | 按 `noun_is_significant` 枚举根 + 祖先闭包 | `resolve_element_generation_root` 名单 | ✅ 形状同，粒度不同（核对表 R0-3 判：我们按 `significant` 是对的；TOPF 是显示层） |
| 3 | 元件 → owner（规则 P：`PIPC`/`HNGC`/`HOLC` 元件、`PLTGRD`/`INTFRM`）；`SUBCOM` → 祖父（Q） | owner 链 | 祖先闭包 | MDU / 第一个非容器 owner | ✅ |
| 4 | `CLIMBM({PANE,GENSEC,SCTN,BPANEL})` 上卷（规则 M） | owner 链 | ~~`PANE`/`GENSEC`/`SCTN` 在 `significant` 位表里为真 → 是根~~ **★0907 改正：四个全是 `significant=false` / `toplevel=true`**（位表实查），direct 路径**不**把它们当根，归最近的显著祖先（`FRMW` / `SBFR` / `STRU` / `SUBS`，位表均为真）——比 core 的 CLIMBM 目标粗一级，但同在 owner 链上、被整根覆盖 | 同上 | ✅ 覆盖成立、粒度更粗（即 09-04 toplevel 对账 §2.3a 说的「粒度差不是漏」）；`BPANEL` 已查：同样 false/true（C0.5 ✅） |
| 5 | 点类 noun → 所属几何件（规则 S，`POPF` = 字典 `point`） | owner 链 | 点容器不成根、归 owner | `is_loop_container_noun` 上卷 | ✅（就是 T2b 那条线） |
| 6 | INTUBE：类别 1 → 前一个元件（规则 H）；删除 / 改挂先按 INTUBE 再 REDRAW（§2.2b） | 分支序 | 整根 BRAN 重生成 | `reroute_derived_geometry_units` 整根 | ✅ 超集（issue #5） |
| 7 | 结构连接 `ADSTCH`：`CREF`↔`JOIS`/`JOIE`、`CFRA`↔`PFRE` 双向核对 → 对端 / 对端顶层件 + `RECON0`（§3） | **正向引用**（从变更元素读出去） | **无** | `ref_rev`：`CREF`(35 noun)/`JOIS`/`JOIE`(4)/`CFRA`(3)/`PFRE`(1) 都是 schema ELEMENT 型 → 进反向索引 → 对端交付单元级联（不核对回指 = 超集） | ❌ **direct 缺**（清单 #12） |
| 8 | 孔洞：PANE / 设备子件变了 → 引用它的 `HOLC` 构件（§2.3-3）；孔洞自己变了 → 自身 + owner（J/P） | **反向引用** | **无**（owner 子树内的孔由整根覆盖；跨 owner 的不覆盖） | `ref_rev` 超集 | ❌ **direct 缺**（清单 #15），前提是真库里有跨 owner 的孔洞引用 |
| 9 | `EQUI`/`ELCONN` → `GATREF(CLIST)` 连接表全部重画（规则 F） | 正向数组 | 无 | 无（`CLIST` 不是存储属性，`ref_rev` 收不到） | ❓ 清单 #13：先定 `CLIST` 是什么 |
| 10 | 桥架 / 网格：`CWBRAN` `ELCXRF`、`CTRAY` / `CGRDCP` / `CGRDLP` 反向、`CLNPNG` 两层（B–E、G） | 正向 + 反向 | 无 | 部分（ELEMENT 型的进 `ref_rev`） | 登记不排期（清单 #14，`gb==3` 族归几何覆盖度） |
| 11 | 网格件只标**画列表里有的**（`DES_DrawList::exists`） | — | 不可比（我们没有画列表） | — | ⚪ |
| 12 | 顶层件的 owner 是 `TMPL` 时改取 owner（§2.3-1） | owner 链 | 模板不进 DESI 增量 | 同 | ⚪ |
| 13 | GETWORK / Undo / Redo 同一条路（§5） | — | 我们只吃 SAVEWORK 区间 | 同 | ⚪（ADR-061 理由 1 不变） |
| 14 | 在 EVALAT 里**没有**的：管道连接 HREF/TREF、CATA→DESI、UDA、XRPNTR | — | — | — | ✅ 09-04 结论全部保持（#5 构造性不可达、#4 我们比 core 强、#9/#10 不做） |

统计：core 侧 14 条规则组里 **8 条 ✅ / 3 条 ⚪ 不可比 / 2 条 ❌ direct 缺（#7 结构连接、#8 孔洞）/ 1 条 ❓（#9 CLIST）**。
**两条 ❌ 都是同一件事的两面：direct 路径没有任何引用类联动。** 区别只在 #7 用正向读就够（不用等反向表），#8 必须反向。

## 2. 范围

**做**：§1 的 #7、#8 在 direct 路径上的取证与接线；#9 的定性；文档收口。

**不做**：
- CATA → DESI（用户划定；且 core 自己也不做，无对照物）。
- 不改生成根判据（`significant` vs `TOPF`）——核对表 R0-3 已判，本计划只补一次对拍取证（C5）。
- 不复刻 `RECON0` 重连——那是编辑器动作，不是刷新。
- 不实现 UDA / `XRPNTR` 依赖递归（ADR-061）。
- 不碰旧 PE 路径的 `ref_rev`（它已是超集）。

## 3. 任务

### C0 —— 文档收口（S，纯文档）

| # | 改哪 | 怎么改 |
|---|---|---|
| C0.1 | 09-04 清单 §1 | ✅ 已加 #12–#15（本轮） |
| C0.2 | 核对表 W4.1 结论 / R5 注记 | ✅ 0907：R5 加了「上游 = `change/EVALAT`（Core3D `0x1022C679`），DCHC 读点 `change/IDCHNG` → `ATAINT`」；§8 GETWORK 那段划掉「一次都没梳理过」并指向新证据 §5；R0-3 补「GETWORK 重画单位 = TOPF」一段 |
| C0.3 | `CONTEXT.md`「联动更新」词条 | ✅ 0907：加了 core 侧对应物一句与 direct 路径缺引用类联动（**CONTEXT.md 同时载着别的会话未提交的词条，本轮不提交它，等那边一起**） |
| C0.4 | 核对表 R0 表 | ✅ 0907：`IsNegative` 行补 `HOLC` 与 `INCOMP` 读同一位；§8「同族字段语义未追」一条划掉并填上五个名字 |
| C0.5 | 查一眼 | ✅ `BPANEL` 在 `significant` 位表里为 **false**（`toplevel` true）——与 PANE/GENSEC/SCTN 一样，§1 #4 已按此改正 |

- 验收：每条落点能 grep；无代码。

### C1 —— 真库里到底有多少「跨 owner 的引用边」（M，只读取证，决定 C2/C3 立不立）✅ 2026-09-07

**结论**（`docs/evidence/2026-09-07-cross-root-reference-census.md`，探针 `src/bin/cross_root_reference_census.rs`，486 库 18.8 s）：

| 组 | 数字 | 判 |
|---|---|---|
| 1. 结构连接 | 55,503 条边；回指成立 47,556；**回指成立 ∧ 对端不在变更元素任何根之下 21,161**（100% 在 `STRU` 下，`PJOI ↔ SCTN` 14,577 / `SJOI ↔ SCTN·GENSEC` 4,484 / `FIXING ↔ GENSEC` 2,100） | **> 0** |
| 2. 孔洞 | 12 个负实体 noun（HOLC 代理）发出 9,329 条引用，**0 条指向 PANE / 设备子件**；跨根 20 条 `NCYL.RREF → SUBS`（共外层根） | **= 0** |
| 3. 改动窗口 | 79 库 3,907 步：结构跨根边**只改持属性一端** 783 边·步，落在 12 个 (库, 步) / 10 个库——`aps7200_0001` 3→4（94 × `SJOI.CREF → SCTN`）、`aps250110_0001` 29→30（193 × `FIXING.CREF → GENSEC`）等；孔洞 0 | **有窗口** |

→ **C2 立项**（§7 拍板 1 的两个门槛都过了）；**C3 不立项**，清单 #15 改「语料上不可达（留探针）」。
顺带：全语料引用边 70% 在**隐式区**（noun 模板定长槽位），C2 读 `CREF`/`JOIS`/`JOIE`/`CFRA` 必须走模板解码（`DbElement::get_ref`），不能只读显式尾块。

- 做什么：用 e3d-io 对 5 项目 486 个 DESI 库扫一遍，统计三组数字：
  1. **结构连接**：`SCTN`/`GENSEC`/节点上的 `CREF`/`JOIS`/`JOIE`/`CFRA`/`PFRE` 非空且**目标不在同一生成根子树内**的条数（按 direct 的根枚举口径判「同一根」）；再统计其中**回指成立**（`X.JOIS == el` 等）的比例；
  2. **孔洞**：`HOLC` 位为真的 noun 实例里，引用目标（哪个属性指向被打孔件，先从字典找 ELEMENT 型属性）跨根的条数；
  3. **改动窗口**：在 §1 #7/#8 的边上，语料里有多少 SAVEWORK 窗口**只改了一端**（这决定 C2/C3 有没有验收窗口——09-04 清单 #3/#5 都栽在这上面）。
- 交付：`docs/evidence/2026-09-XX-cross-root-reference-census.md` + JSON。
- 判：跨根边 = 0 → C2/C3 不立项，清单 #12/#15 改判「语料上不可达（留探针）」；> 0 且有窗口 → 立 C2（正向）；孔洞跨根 > 0 → 立 C3（反向）。
- 退：无改动。

### C2 —— direct 规划器补结构连接正向级联（M，**C1 已判立项，2026-09-07**）

- 什么：窗口里变更元素若是 `SCTN`/`GENSEC`/`PANE`/`BPANEL`/节点族（或在其下，`CLIMBM` 那四种），读它的
  `CREF`/`JOIS`/`JOIE`/`CFRA`（T 侧），逐个到目标读回指属性（`JOIS`/`JOIE`/`CREF`/`PFRE`）**等于自己**才算，
  把目标的生成根并进 `regen`。**只正向读、不建反向表**——与 core `ADSTCH` 同形。
- ★0907 C1 给的边界：持属性的 noun 只有 **`SCTN` / `GENSEC` / `STWALL` / `WALL`（`JOIS`/`JOIE`）、`PJOI` / `SJOI` / `SUBJ` /
  `FIXING`（`CREF`）、`CMPF`（`CFRA`）**；`STRU` 门（src 的 owner 链上有 `STRU`）在语料里对缺口边 100% 成立，可以照 core
  一样加上当护栏。读属性走 `DbElement::get_ref` / `get_ref_array`（隐式区），不读显式尾块。
- ★0907 几何显不显：e3d-model 第 4 阶段（SCTN 关节负体）未接，今天对端不重算几何也不会错；**验收在规划器层**，
  接线不等第 4 阶段——等到那时再补就是正确性缺口。
- 改哪：`window_root_plan.rs` 的 touched → roots 映射之后、`regen = touched ∩ roots_T` 之前加一步「连接扩展」；
  开关 `model_structural_connection_cascade`，默认关。
- 验：
  - 契约测试（纯内存）：SCTN A `CREF`→节点 J，J `JOIS`→A：改 A → `regen ⊇ {root(A), root(J)}`；J `JOIS`→别人（回指不成立）→ 只有 `root(A)`；开关关 → 只有 `root(A)`。
  - 真库窗口（C1 找到的）：**`aps7200_0001` 3→4**（94 个 `SJOI` 单独改、`CREF` 指的 `SCTN` 不在同一根下）与
    **`aps250110_0001` 29→30**（193 个 `FIXING` 单独改 → `GENSEC`）：开关开，对端根 ∈ `regen`；开关关，不进。
    先用 `changed_elements` 逐记录确认这两步不是「搬家未改字节」。
  - 九个钉死窗口开关两态**逐字相同**（它们没有结构连接改动）。
- 退：开关。

### C3 —— direct 规划器接通用反向级联（L，~~C1 孔洞跨根 > 0 才做~~ **★0907 不立项**：孔洞跨根 = 0、零窗口；机制归既有计划）

- 什么：`direct_store::referrers_of`（已实现、零调用方）接进 direct 规划器：对窗口里每个变更元素，
  把「引用它的元素」的生成根并进 `regen`。这同时覆盖 #15 孔洞、#12 的超集、以及（本计划外的）#4。
- 依赖：`docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md` 的接线与 `ref_rev` 降级排期——**本计划不重复它，只登记消费关系**。
  另有一条矛盾要那条计划先答：dab 引用表「只覆盖 kind-16 属性、`HREF`/`TREF`/`CREF` 无表」（`2026-09-06-dab-ref-table-write-path.md`）
  与 core `DFIND`（dabacon opcode 43）能找到 `HOLC` 引用者——`DFIND` 查的是哪张表，读了才知道反向表能不能服务 #15。
- 验：孔洞窗口（C1 找到的）对端进重算集；`referrers_of` 空结果不改变任何现有窗口。
- 退：开关，默认关。

### C4 —— INTUBE 前一元件的契约（S，只补测试）✅ 2026-09-07：**已经钉着，不必再补**

- 什么：core 删除 / 改挂先以 INTUBE 评估前一元件再 REDRAW（§2.2b）。我们靠整根 BRAN 重生成覆盖——补一条计划层用例钉住
  「删掉 BRAN 中间一个元件 → 该 BRAN 进 `regen`（而不是只 `DeleteCleanup`）」，防止将来有人把删除做成「只删行」。
- ~~落点：`window_root_plan.rs` tests；现有 `only_units_with_implicit_tubing_regenerate_on_a_member_move` 只覆盖移动，不覆盖删除。~~
  **★0907 核了：立项前提不成立。** 那条「只覆盖移动」的测试是 `model_update_plan.rs` 里的 noun 谓词测试，不是规划器用例；
  规划器层的删成员契约**早就在** `window_root_plan.rs::live_ams8000_pinned_windows_land_in_the_expected_buckets` 的
  269→270 窗口里（db8000 scratch 支管 `DELETE FTUB 4`：`regen == 1`、`delete == []`、工作项恰好 1 条 —— 即 `RegenRoot(BRAN)` 一条、
  没有单独的 `DeleteCleanup`），证据 `docs/evidence/2026-09-02-db8000-bran-scratch-tty-increment.md` 腿 4，
  `increment_planner_parity` 的 CI 语料同一窗。0907 在本机真文件上跑了这条 live 门：**ok（1 passed，4.1 s）**。
- 可选加固（不排期）：那一窗再断言 regen 的那一根就是 `24384/26261`；等 `window_root_plan.rs` 不在别的会话手里时顺手加。
- 退：无改动。

### C5 —— `TOPF` 顶层件集 vs direct 根集 离线对拍（S，只读）✅ 2026-09-07（随 C1 一次扫完，按**真库元素**而不是 noun 对拍）

- 什么：用位表快照离线算：对 1931 noun，`LISTOP` 口径（`TOPF` ∧ ≠WORL，+ RPLA/PVOL/DRAW/FIXING 特例）为真而
  `significant` 为假的 noun（118 个里的一部分），逐个判「它在 direct 枚举里会归到哪个根」——**有没有一个 noun 是
  core 会单独重画、而我们既不成根也不归任何根的**（W1.2 顺带看到的「BOX 直挂 ZONE 不出根」就是这一档的疑点）。
- **结论**（证据 §5）：1,961,172 个元素里 core 判 primitive 却没有任何根祖先的只有 **268 个**，成簇的只有船体族
  `BLOCK → CPANEL`（160）/ `CSURPX → CPROF`（82）/ `BLEVEL → BNDLIN`（5），全在 AvevaMarineSample；
  「primitive 直挂 ZONE」只有 `RPLA`×2、`CNODE`×3，**BOX 直挂 ZONE 在语料里不存在**。TOPF 而无根的其它 noun
  （`APPLDA` `CWAY` `GPSET` `CPROTR` `BLOCK` `ASMBLY` …）自身不出几何。
- 判：**不改判据，不加白名单**（§7 拍板 4 的前半句用不上）；船体族登记为几何覆盖度议题（它们今天根本不生成，不是增量问题），
  出现船体语料再立项。结论已进核对表 R0-3。
- 退：无改动。

### C6 —— `CLIST` 定性（S，只读；电缆语料才有内容）

- 什么：`CLIST` 不在存储属性表里——查 core.dll 伪属性注册（`addGetPlug` 一族）或 `GATREF` 的取法，定它是什么、
  由谁算；`ELCONN` 在本语料 486 库里有没有实例（`noun_census_probe --nouns ELCONN CWBRAN CTRAY`）。
- 判：无实例 → 清单 #13 改「语料上不可达」；有 → 另开计划。

### C7 —— 证据补洞（S，IDA 只读，不阻塞任何项）

`DFIND`（opcode 43）查哪张表；`DB_Element::hashValue` 是否 = noun hash；EVALAT 两个未解 noun（`271245643` / `269526523`）；
`UPDATN` / `UPDATD` 之后到 `DES_DrawList` 的那一跳（`sub_10507C30` / `sub_1050EA70` 是谁）。产物回灌证据文档 §7。

## 4. 顺序与依赖

```
C0 文档 ✅(0907) ──▶ C1 真库普查 ✅(0907：结构 21,161 条跨根边 + 12 个窗口；孔洞 0) ──┬─▶ C2 结构连接正向级联 ★立项，下一步
                                                                                    └─▶ C3 通用反向级联接线 ✗ 不立项（孔洞 = 0）
C4 INTUBE 删除契约   ✅(0907 核实：live 门 269→270 早已钉着，跑绿)
C5 TOPF 对拍         ✅(0907，随 C1 扫完：无根 primitive 268 个，全是船体族；不改判据)
C6 CLIST 定性        随时，只读
C7 证据补洞          随时，IDA
```

- **C1 是闸**：09-04 清单 #3 / #5 都是先立项后发现语料里压不出窗口，这次反过来。**0907 闸已过**：结构连接那一半两个门槛都满足。
- C2 不依赖 C3，也不依赖反向表——这是把 #12 从 #4 那个洞里拆出来单独做的理由。
- 与 W 计划的关系：W1.2（T2b）✅ 已完成；W2.1/W2.4/W3.x/W4.1 照旧；本计划所有项都在 W4.1 的「GETWORK 通道」结论之上，不与它们争同一段代码。

## 5. 验收总表

| 任务 | 可证伪的写法 |
|---|---|
| C0 | 每处新增引用的 `文件:行` / 地址在工作树与 IDB 里 grep / 查得到 |
| C1 | 三组数字各是一个具体整数；跨根边 = 0 时清单 #12/#15 判词改成「语料上不可达」 —— ✅ 0907：**21,161 / 0 / 783（12 窗口）**；#15 已改判，#12 坐实为洞 |
| C2 | 契约三态（回指成立 / 不成立 / 开关关）断言集合恰好相等；真库窗口对端根 ∈ `regen`；九窗口两态逐字相同 |
| C3 | 孔洞窗口对端根 ∈ `regen`；`referrers_of` 为空时所有现有窗口计划字节相同 |
| C4 | 删中间元件 → `RegenRoot(BRAN)` 恰好 1 条且 `DeleteCleanup` 不单独存在于 `regen` 之外 —— ✅ 已由 live 门 269→270 钉着（0907 跑绿） |
| C5 | 列出「core 单独重画、我们不归任何根」的 noun 名单，或明确为空 —— ✅ 0907：船体族 `CPANEL`/`CPROF`/`BNDLIN`（247）+ 零星 21 个，见证据 §5 |
| C6 | `CLIST` 的来源一句话 + `ELCONN` 实例数 |

## 6. 风险

| 风险 | 在哪 | 怎么挡 |
|---|---|---|
| 把 `ADSTCH` 的双向核对做成单向 → 多算对端 | C2 | 契约第二态（回指不成立 → 不并根）钉死 |
| 正向读 `CREF` 等要打 T 侧文件，窗口规划变慢 | C2 | 只对结构 noun 读、只读四个属性；`last_root_duration_ms` 前后对比 |
| 反向级联接线把 #4 CATA 顺手带进 direct，越出用户范围 | C3 | 开关按引用来源库类型分：本计划只放 DESI→DESI 边 |
| 语料里没有验收窗口 | C1 | 先量再立；零就登记不做 |
| `hashValue` 不是 noun hash → 规则表读错对象 | 证据 | C7 读实现；规则表所有 noun 比较都建立在这个假设上 |

## 7. 需要拍板的事

1. **C1 的门槛**：跨根结构连接边 > 0 就立 C2，还是要求「> 0 且有只改一端的窗口」？（建议后者——没窗口的实现没法验收。）
   **★0907 数字替你答了**：两个门槛都过（21,161 条边、12 个窗口），不必再拍。
2. **C2 开关的最终态**：默认关灰度 → 默认开。建议与 W2.1 一样先默认关，九窗口两态相同 + 真库窗口过了再翻。
3. **C3 要不要等 dab-ref-tables 计划**：等（干净）vs 先用开库全扫的内存反向表（快）。建议等——C1 若判孔洞跨根 = 0，C3 根本不用做。
   **★0907：孔洞跨根 = 0，C3 不做**，这条不必拍。
4. **C5 若查出漏归 noun**：是补 direct 枚举（按 noun 加白名单）还是改判据？建议补白名单、不改判据（核对表 R0-3 口径）。
   **★0907：查出来的只有船体族（247 个元素，单一语料），不加白名单、不改判据；登记为几何覆盖度议题。** 要翻这条直接说。

## 8. Plannotator 评审记录

> `plannotator annotate docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md --json`，2026-09-06 晚发起。
> 批注原文 → 改动落点记在这里；未取回则注明。
>
> - 第 1 次 23:02:54–23:03:01、第 2 次 23:03:30–23:03:37：都在 **6 秒内** 回 `{"decision":"dismissed"}`，无批注
>   （标签页一打开就关了，或浏览器侧没接住）。不再自动重开，等用户说一声再发起，或由用户在自己终端里跑同一条命令。
> - 2026-09-06 23:07 用户在聊天里直批：「把今天…的改动分两个 commit 提了，开始实现」——**口头通过、无批注**，不再开 Plannotator。
>   §7 四条按「没意见 = 照建议执行」推进；0907 的 C1 数字又替其中三条作了答（见 §7 各条 ★0907）。
