# 核对表：模型增量更新 vs Core3D `PartialUpdateDesiMgr`

> 对象：`D:\AVEVA\Everything3D3.1\Core3D.dll`（32 位，ida-bridge `idalib-32268`）
> 取证：`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`（叙述体）
> 计划：`docs/plans/2026-08-27-model-impact-core3d-parity-plan.md`
> 用例：`docs/specs/core3d-partial-update-test-cases.md`
> 我方：`src/data_interface/{model_impact,generation_root,model_update_plan,update_scope}.rs`

## 0. 这份文档是干什么的

证据文档是**叙述**——它讲清楚了 core 怎么想。这份是**核对表**：把同一批事实拆成
**逐条可判真假的规则**，每条都带三样东西——core 的确切行为、能重新验证它的地址或 SQL、
我方今天的对应物。要么打勾，要么记一条缺口，不留"大概是这样"。

三种用法：

1. **改代码前**：找到你要动的那条规则，先看 core 到底怎么做，再决定抄不抄。
2. **改代码后**：把该条的"核对办法"跑一遍。每条规则在
   [用例集](core3d-partial-update-test-cases.md)里都有一个编号对应的用例。
3. **换版本后**（E3D 升级）：§1 的地址表整个失效，但**规则编号不变**——
   按 §1 的重定位办法把地址重取一遍，规则逐条重验。

判定符号：✅ 已对齐 ｜ ◐ 形状相同判据不同 ｜ ❌ 缺 ｜ ⚪ core 侧不成立或不可比

> **★0907 标题符号按 §9 现状重标了一遍**（之前只改正文与 §9，标题没跟上，读标题会错判）：
> R2 / R6 / R22 / R28 → ✅（R2 接线但开关默认关、无正样本；R6 视图切换那条待拍板；R22 收尾两条测试待补）；
> R11 → ⚪（第一次随 R10 带着改被驳回；同日单独过了判据：direct 枚根不挡、真库 5,366 个被挡嵌套根，差别只落在 core 的段抹除上，见 R11 末段）；
> R17–R21 / R24 保留 ❌ 但标「慢」（§9 定性多做不是漏做，W3.2 先量再定）；R9 / R12 / R13 / R14 保留 ◐ 并注明各自卡在哪。
> 「✅」不等于「生产已启用」：R2 与 R9 的开关今天都默认关。

> **本轮（2026-08-28）用 ida-bridge 复核指令流，改了证据文档三处、新增六条它没有的规则。**
> 更正见 §7，逐条在正文里标 `★更正` / `★新增`。
>
> **2026-09-04 二次复核**（`docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md`，
> ida-bridge `idalib-25128`）。四处改动，逐条标 `★0904`：
> **R23 离线结案**（不必再找 live 进程，`PDMS_Idlist_base` 的构造函数写死了那一位）；
> **R6 补第三个队列作废点**（视图切换，`LoadSheet`）；
> **§1.3 补两个有语义的方法**（`ExistsIDList` / `LoadSheet`）与 §1.5 一节新增（这个类的宿主是
> DRAFT 图纸，决定哪半边规则能往服务端搬）；
> **§9 R22 的判词作废**——我方落库侧一直有按根对账，孤儿行不是「只能等整库重建」。

## 1. 对象与地址（重定位的起点）

### 1.1 单例字段布局

| 偏移 | 名字 | 类型 | 读出处 |
|---|---|---|---|
| `+0x05` | `m_inDraw` | bool | `Update` 重入保护；pass 2 置 1，`Finish` 后清 0 |
| `+0x08` | `m_view` | `DB_Element`(20B) | 当前视图，`SetView`/`ResetView`。★0904 只能是**图纸里的 `NOUN_VIEW`**，见 §1.5 |
| `+0x1C` | `m_enabled` | bool | `UpdateOn` |
| `+0x1D` | `m_muted` | bool | `Mute` / `UnMute` |
| `+0x20` | `m_granularityMode` | int | **恒 0**，全类无 setter（§4.8） |
| `+0x24` | `m_queue.begin` | ptr | 待办队列 |
| `+0x28` | `m_queue.end` | ptr | |

### 1.2 队列记录布局（stride 24 字节）

| 偏移 | 内容 |
|---|---|
| `+0x00` | `DB_Ref`（12 字节） |
| `+0x0C` | 元素句柄第 3 字 |
| `+0x10` | 元素句柄第 4 字 |
| `+0x14` | `ModelState`（int） |

push 走 `sub_1047BA50`。stride 证据：`IsPresent` 的 `v4 += 24`、`AncestorDeletes` 的
`add esi, 18h`（`0x1047C155`）。

### 1.3 方法地址（E3D 3.1，`Core3D.dll`）

| 地址 | 方法 | 角色 |
|---|---|---|
| `0x1047BA50` | `push_back(Record)` | 队列写入 |
| `0x1047BC40` | 构造函数 | 唯一写 `+0x20 = 0` 的地方 |
| `0x1047BE10` | `AbsentPrimitives` | 缺失图元回收 |
| `0x1047BF80` | `AddIDList` | 把元素并进视图 ID 清单并写库 |
| `0x1047C060` | `AncestorDeletes` | 删除类的祖先标记 |
| `0x1047C200` | `ChangedModelToUpdate` | 入口：消费 QCHGLS |
| `0x1047C2E0` | `DeletedModelToUpdate` | 入口：删除（别名 `DeletedModelNotify`） |
| `0x1047C350` | `DrawModel` | 发 PML `PUPDES` |
| `0x1047C670` | `EraseModel(DB_Element&)` | 按 significant 位分派 |
| `0x1047C6D0` | `EraseModel(DB_Ref&)` | 按队列里有无 state 2 分派 |
| `0x1047C720` | `ErasePrimitive(DB_Element&)` | |
| `0x1047CB80` | `ErasePrimitiveFromCandidateModel` | |
| `0x1047CCF0` | `ErasePrimitiveFromUnknownModel` | |
| `0x1047CF00` | `EraseSignificant(DB_Ref&)` | |
| `0x1047D050` | `Exists` | "还有没有可画的东西"（递归） |
| `0x1047D230` | `ExistsCPS` | 查已绘制段 |
| `0x1047D360` | `ExistsIDList` ★0904 | `if (!idlist[+0x18]) return false; return idlist->exists(e)`——R22/R23 那句 `idlist.active && idlist.exists(p)` 的本体 |
| `0x1047D380` | `Finish` | 清空队列 + 通知 |
| `0x1047D650` | `GetIDList` | 取视图 ID 清单 |
| `0x1047D8C0` | `GranularityExpansion` | **粒度中心算法** |
| `0x1047DC00` | `IsAncestorDeletesPresent` | 队列里有无 state 2 |
| `0x1047DC30` | `IsDecendantPresent` | 队列里有无该 state 的子孙 |
| `0x1047DD20` | `IsDeletesPresent` | 队列里有无 state 3 |
| `0x1047DD50` | `IsManagedStructure` | XGEOMETRY 判定本体 |
| `0x1047DD90` | `IsNegative` | 字段 `0x92663`，int ≠ 0 |
| `0x1047DDD0` | `IsPending` | **去重中心算法** |
| `0x1047E020` | `IsPresent` | 队列里有无 `(elem, state)` |
| `0x1047E070` | `IsPrimitive` | `0xA103E` ∨ `0xBBD5ADC` |
| `0x1047E0D0` | `IsSignificant` | `0x5657A0A` |
| `0x1047E110` | `LoadSheet` ★0904 | 换视图的唯一真入口；**视图变了就清空队列**（R6）；随后走 `DLL_draft` 那一串 |
| `0x1047E240` | `Members` | 三模遍历 |
| `0x1047E590` | `ModelToUpdate` | **内部总入口** |
| `0x1047E650` | `NegativeMembers` | `Members(mode=2)` |
| `0x1047E670` | `NewModelNotify` | 入口：新建（无 ID 清单副作用） |
| `0x1047E6E0` | `NewModelToUpdate` | 入口：新建（**有** ID 清单副作用） |
| `0x1047E7E0` | `PrimitiveMembers` | `Members(mode=1)` |
| `0x1047E800` | `Refresh` | **清空整个队列** |
| `0x1047E9C0` | `SignificantMembers` | `Members(mode=0)` |
| `0x1047E9E0` | `SignificantOwner` | 含自身的向上攀爬 |
| `0x1047EAB0` | `Update` | 三遍消费队列 |

### 1.4 换版本后怎么把地址取回来

方法名带完整修饰名，符号在导出表里，所以不用靠特征码：

```powershell
ida-bridge exec <client> --sql "SELECT start_ea, name FROM funcs WHERE name LIKE '%PartialUpdateDesiMgr%' ORDER BY start_ea"
```

字段 id 靠字节搜（`push imm32`）：

```powershell
# 0x5657A0A significant / 0xA103E primitive-A / 0xBBD5ADC primitive-B / 0x92663 negative
ida-bridge exec <client> --sql "SELECT '0x5657A0A' AS id, address, name_at(func_start(address)) AS func FROM bin_search WHERE pattern = '68 0A 7A 65 05' UNION ALL SELECT '0xA103E', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 3E 10 0A 00' UNION ALL SELECT '0xBBD5ADC', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 DC 5A BD 0B' UNION ALL SELECT '0x92663', address, name_at(func_start(address)) FROM bin_search WHERE pattern = '68 63 26 09 00'"
```

> **Hex-Rays 在这个类上系统性出错，凡涉及判据的函数一律读指令流。**
> `getField(id, &out)` 的出参它认不出来，于是把调用渲染成无参、把 `out` 的比较整个丢掉。
> 后果不是"少了个参数"，是**丢分支**：`GranularityExpansion` 的整条 significant 分支
> （`0x1047D91A` 的 `jz`）在伪码里根本不存在；`Exists` 丢了递归那一半；`IsNegative` 直接反编译失败。
> 判定这类函数是否被坑，看伪码里有没有孤零零的 `DB_Noun::getField(vN);` 后跟 `return 0`。

### 1.5 这个类的宿主是 DRAFT 图纸 ★0904

`SetView`（`0x1047E890`）只在参数是 `NOUN_SHEE` 且 `ATT_PICORF` 有效时，经 `LoadSheet` 真正换视图；
传别的元素直接把 `m_view` 赋成默认构造的无效元素（与 `ResetView` `0x1047E840` 同一段代码）。
`LoadSheet` 随后走 `DLL_draft::cmfdsh` → `cmcrsh` → `cmrgob` → `cmldpc` → `WWPicture::makeCurrent`。
`DrawModel` 发的 `PUPDES` 也是往这条路上发。

**这决定了这份核对表哪半边能往我们的服务端搬**：

- **能搬**：R9–R21 那半边——两个 noun 位、`GranularityExpansion`、`SignificantOwner`、
  `AncestorDeletes`、`IsPending`。它们是 DB 级的联动规则，与宿主无关，继续当 oracle。
- **搬之前要先答一个问题**：R6 / R8 / R27 / R29 那半边（队列生命周期、`m_inDraw` 重入、三遍消费、
  `Finish`）以及 R7/R22/R23 的 `PDMS_Idlist2`，都是**图纸视图**语境的产物。
  往服务端搬之前得先说清楚「我们这边什么东西扮演 VIEW、什么东西扮演 ID 清单」——
  R7 已经提过这个问题，这里补上它的出处。
  ★0906 两个问题今天的答案：「什么算切换视图」的三个候选与判词见 R6（开发计划 W2.2，建议登记「无对应物」）；
  「什么扮演 ID 清单」由 R22 回答——对账源是库里实际存着的行，不是一份清单，所以 R23 那一档在我们这边不存在。

## 2. 判据：两个 bool 位

| 判据 | 地址 | 字段 id | 取值 |
|---|---|---|---|
| `IsSignificant(e)` | `0x1047E0D0` | `0x5657A0A` / `90536458` | bool |
| `IsPrimitive(e)` | `0x1047E070` | `0xA103E` / `659518`，为假再试 `0xBBD5ADC` / `196958940` | bool ∨ bool |
| ~~`IsNegative(e)`~~ | `0x1047DD90` | `0x92663` / `599651`，字段名 **`HOLC`**（逆 hash，★0907 补） | int ≠ 0，**本链无活的消费者**；但 core.dll `nounlib/INCOMP`（`0x561C1B0`）读的是**同一个位**，`change/EVALAT` 规则 J / P 与 §2.3 第 3 条（孔洞联动）靠它——见 `docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §2.2a。这个位仍未导出快照；0907 真库普查用字典 `positive_equivalent != 0`（12 个负实体 noun）代理 |

**R0-1 取值走出参，不走返回值。**
三个判据都是"把栈上 out 清零 → `getField(id, &out)` → 读 out"，`getField` 自己的 bool 返回值
（"这个 noun 登没登记这个字段"）被丢弃。按 core 的口径，**字段未登记 = 该位为假**。
`IsPrimitive` 两次调用共用一个 out 且只清零一次——安全，因为第一次为真就不会有第二次。

- 核对：`SELECT address, disasm FROM instructions WHERE func_ea = 0x1047e070 ORDER BY address`
- 我方：✅ 已导（P0，2026-08-28）。`tests/fixtures/core-noun-granularity-e3d31.json`，
  通道是 `core.dll!DB_Noun::getField(id, &out)` + `findNoun(hash)`，照 core 的口径清零取出参。
  1931 个 noun 三张表 unknown / not_found 全为 0，所以"未登记 = 该位为假"这条在本快照上
  没有被触发过。细节见 `docs/evidence/2026-08-28-core-noun-granularity-export.md`。

**R0-2 `primitive` 不是跨版本固定谓词。**
`0xA103E` 稳定；它的搭档会换——2.10 的 `MassProperties::PopulateCSGtree` 配 `0xA18B8`，
而 `0xA18B8` 在 3.1 里一次都搜不到。快照必须**分开存两位并记来源版本**，
不要在导出时就合成一个 `primitive` 布尔。

- 我方：✅ 两位分开存（快照 `fields` 下就是 `significant` / `primitive_a` / `primitive_b`
  三个键，没有合成键），来源版本由 `core_sha256` 钉住。生产侧的查询
  `generation_root::core_primitive_bits` 也返回 `(a, b)` 而不是一个布尔，
  `noun_is_primitive` 才取或——想知道"是哪一位说了算"永远问得出来（T2a）。

**R0-3 ★0904 还有第三个位，这份表里没有：`toplevel`（`0xA187C` / 661628）。**

`significant` 在 `Core3D.dll` 里 8 个读点，**7 个在本类里**，第 8 个是 `sub_104C7000`——
**`DES_DrawList` 一次都不读它**。3D 绘制侧用的是 `toplevel`，它有自己一整族消费者
（`getAncestorTOPFElement` / `getChildTOPFElements` / `getPromotedTOPFElement` /
`getTOPFElements` / `isAtOrBelowTOPF` / `promoteElements` / `wantToAddRemoveTopf`）。
`getAncestorTOPFElement`（`0x10515FC0`）的形状与 `SignificantOwner`（R14）一模一样
——含自身、沿 owner 链上行、无深度上限、命中即返回——**只是问的位不同**。

离线对账（`output/noun_caps_full.json` × 位表快照）：`toplevel` 211 / `significant` 127 /
交集 93 / 只 toplevel 118（其中 68 个同时是 primitive：`GENSEC` `SCTN` `PANE` `WALL` `FLOOR`
`STWALL` `DOOR` `CT*` `HR*` `WL*` …）/ 只 significant 34（其中 8 个是 gb==2 容器，含 `ROOM`）。
四个 MDU：`BRAN`/`HANG`/`EQUI` 两位都真，`SUPPO` 两位都假。

**★0904 晚追完：两个位管的不是同一件事，我们用 `significant` 选根是对的。**
`ExistsCPS`（`0x1047D230`）里「已绘制段」是按 `SignificantOwner`（`0x1047D286`）找的
——**core 自己的几何段就按 significant 索引**；而 `getAncestorTOPFElement` 的 19 个调用方里
13 个是 `get/setPrimitiveOrTOPF{Colour,Edges,Transparency,Visibility,VisProps,…}`，
**TOPF 管的是绘制清单成员与显示属性归属，没有一个几何重算的消费者**。

三层：已绘制段（`significant`）／局部更新块（`significant`）／绘制清单项（`toplevel`）。
`toplevel` 位仍然导进了快照（离线字典字段，`old-parse-pdms-db::dict::FIELD_TOPLEVEL`），
但它是**显示属性那一层的**账，不是几何粒度缺口。对账（211/127/交集 93）、486 个真库的普查
与「不许当已证」见 `docs/evidence/2026-09-04-noun-toplevel-reconciliation.md`（§0' 是改判那一节）。

**★0906 一个补充：`toplevel` 在 3D 侧确有一个几何相关的消费者——GETWORK 的重画单位。**
`sub_1052F2E0` 对每个变更元素先 `desdblib/FNDTOP`（按 `nounlib/LISTOP` = `toplevel` 位上卷到顶层可画件），
再把顶层件交给 `change/EVALAT` 进 QCHGLS；`significant` 要到 DRAFT 侧 `GranularityExpansion` 消费队列时才出场
（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §4 / §5）。这不改上面的判词：重画单位是 TOPF，
几何段索引仍是 `significant`，两者都在 owner 链上、TOPF 更粗——按 `significant` 选根只会比 core 更细，不会漏。

**★0907 按真库元素对拍完了（联动对齐计划 C5，`docs/evidence/2026-09-07-cross-root-reference-census.md` §5）：**
486 库 1,961,172 个元素里，按 `LISTOP` 口径 core 会单独重画、而 direct 枚举下**一个根祖先都没有**且 core 判 primitive 的，
只有 **268 个**——成簇的只有船体族 `BLOCK → CPANEL`（160）/ `CSURPX → CPROF`（82）/ `BLEVEL → BNDLIN`（5），
全在 AvevaMarineSample；W1.2 顺带看到的「BOX 直挂 ZONE 不出根」在语料里没有实例（primitive 直挂 ZONE 只有 `RPLA`×2、`CNODE`×3）。
**判：不改判据、不加白名单**；船体族登记为几何覆盖度议题（今天根本不生成，不是增量问题）。

## 3. 入口与门（R1–R8）

### R1 只处理 DESI 库 ✅

`DB_DB::type(e->getDB()) == 1`，在 `ModelToUpdate` 里（`0x1047E590`）。
我方等价物：`UpdateScope::admits(db_type, dbnum)`。

### R2 XGEOMETRY 子树整体排除 ~~❌~~ ★0904 已接线 ✅（开关 `model_xgeometry_gate` 默认关、语料零正样本；direct 路径只观察不筛，见 W1.1）

`DB_Element::climb(e, NOUN_XGEOM).isNull()` 必须为真才继续。
凡是有 XGEOMETRY 祖先的元素，core **一律不进**局部更新——显式几何走另一条路。
`NOUN_XGEOM` 取自全局（`0x1047DD5E`），字典短名 `XGEOM`，hash `7739277`
（`output/noun_layout.json:11666`）。判定本体是 `IsManagedStructure` `0x1047DD50`。

`NewModelToUpdate` / `NewModelNotify` / `DeletedModelToUpdate` 三个入口**自己先查一遍**，
`ModelToUpdate` 再查一遍——这是唯一被重复检查的门。`ChangedModelToUpdate` 不查：
它的每一条都来自 QCHGLS，逐条转交 `ModelToUpdate` 时才过门。

- ~~我方：**无**。计划 T2.2。~~
- **★0904 已接线（默认关）。** 落点就是计划 §3 定的那个：
  `manual_update::xgeometry_gates(&OwnershipSnapshot, candidates)`——那里是唯一**不额外打库**
  就有完整 pre/post owner 链的地方。`ResolvedUnitRollup` 带 `xgeometry_gated` 出来，
  `build_model_update_plan` 与**预览**（`manual_update` 的 preview 分支）**共用同一道门**
  ——少走一边就回到 2026-08-04 那个「预览说的不是执行要做的」的老毛病。
  候选是位姿目标 ∪ 删除目标，它们不在 `regen_details` 里（已被掩掉），所以单独传进去、
  也单独种进 `load_base_graph` 的 seeds；不种就是静默漏门。
  **门只筛模型工作项，不碰 `details`**（`details` 还驱动 `design_refnos`，那是数据层）。
  pre/post 口径：新建看 post、删除看 pre、**修改两态都在 XGEOM 底下才挡**
  ——搬进搬出的那一次必须照常处理。两条测试钉着：
  `the_xgeometry_gate_lets_a_move_across_the_boundary_through`（四种两态组合各一条）与
  `the_xgeometry_gate_climbs_the_whole_owner_chain`。
  **开关 `DbOption.model_xgeometry_gate` 默认 false，关着时行为一字不变**，可灰度。
- **★0904 真库普查：这批语料里 `XGEOM` 一个实例都没有。**
  5 个真项目、486 个 DESI 库全扫（`noun_census_probe --nouns XGEOM`，产物
  `docs/evidence/2026-09-04-xgeom-census.json`），**实例 0**。两条推论，方向相反，都要记住：
  1. **开这个开关在本语料上零风险**——它一次都不会触发，行为与关着完全一样。
     「灰度」在这里没有实际内容，可以直接默认开。
  2. **也正因如此，开了不等于验证过**——没有正样本。唯一的正样本是那两条单测
     （`the_xgeometry_gate_lets_a_move_across_the_boundary_through` 覆盖四种两态组合）。
     要真验，得在 E3D 里造一棵 XGEOMETRY 子树（挂几个 BOX，再搬进搬出一次），
     或者拿一个真用显式几何的项目来跑。
  这也顺带解释了这条缺口为什么一直没咬人：**这批项目根本不用显式几何。**

### R3 元素必须 `isValid` ★新增 ◐

`ModelToUpdate` 有一道 `DB_Element::isValid(e)`，四个公共入口都没有。
证据文档 §3 的门清单漏了这条。

- 我方：◐ 我们从持久层取 `pe`，取不到就 `None`，形状等价但不是同一件事——
  core 判的是**元素句柄有效**，我们判的是**库里有没有这一行**。

### R4 三个开关的检查顺序 ★更正 ✅

证据文档 §3 把顺序写成"enabled → view → muted → XGEOM → DESI"，实际是：

```
ModelToUpdate(e, state):
    if !m_enabled                      return    # +0x1C
    if !m_view.isOK()                  return    # +0x08
    if !e.isValid()                    return    # ★ R3
    if e.getDB().type() != 1           return    # R1
    if !climb(e, NOUN_XGEOM).isNull()  return    # R2
    if m_muted                         return    # +0x1D
    if IsPending(e, state)             return    # R17–R20
    GranularityExpansion(e, state)
```

语义上无差别（全是无副作用的早退），但按这份顺序排查线上问题才对得上。

### R5 QCHGLS 的 change code 被丢弃 ✅

`ChangedModelToUpdate`（`0x1047C200`）按**步长 3** 遍历 `change/QCHGLS`，
每条 `HGETIA(..., count=2)` **只取前两个字**（`DB_Ref`），第三个字（change code）不读。
每一条都当 `Changed(0)` 处理。

**推论**：DCHC 码的全部作用发生在上游——决定元素**进不进** QCHGLS；
一旦进了，重画范围只由 noun 位决定。**core 这一层没有 `TransformOnly`。**

★0915 **这句只对 DRAFT 侧的 `PartialUpdateDesiMgr` 成立。** 3D 画列表那一侧（`change/UPDATN` → `DES_DrawListManager::updateGraphics`
→ **`change/UPGRPH`** `0x1022EAEE`）读的是 QCHGLS 的第三个字：类别 3（`POS`/`ORI`/`BFORI`）→ `css/STRTRA` / `css/SEGTRA` / `change/TRASCN`
**只改图形段矩阵**（`WSTRAM`/`LVTRAM` → `GLSMAT`），类别 4 → `add/REPELE` 整件 / `change/SADREP`→`css/SEGREP` 单图元 / `add/REPALL` 全部重建，
类别 1/2 只 `COLRES`。即 **core 的 3D 侧确有「只动方位」的便宜路径**——它就是我方刚体前移（ADR-066）的 oracle，例外名单（P23 noun、
管件两侧隐含管段、`EVALAT` 规则 M/J/P/R/S 转成类别 4 落到属主）见 `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §4。
DRAFT 侧丢第三个字的取证本身不变。

★0906 那个「上游」找到了：**`change/EVALAT`**（Core3D `0x1022C679`），DCHC 的读点是 `change/IDCHNG`（`0x1022E302`）
→ core.dll `ATAINT(属性)`；类别 0 根本不进队列（= R13「双假丢弃」的上游），类别 1 = INTUBE 还会连带前一个元件。
`EVALAT` 同时是 noun 专项联动规则表（上卷 / owner / ~~正向引用 `CLIST`·`ELCXRF`~~ **伪属性 `CLIST`（CWBRAN）·`ELCXRF`（EQUI/ELCONN），底下是引用表反向查 `STAREF`/`ENDREF` 指向的电缆**（★0907 C6，证据 §8.6；0906 把两个属性写反了） / ~~反向引用 `DFIND`~~ 成员向下 `DFIND`（★0907 实读：`DFIND` = dabacon 成员导航，`EVALAT` 自己没有反向引用调用） / 结构连接 `ADSTCH`），
写 QCHGLS 的是 `change/EVALST`（步长 3、同键取 max 类别）。全表见 `docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §2–§3；
对我方的意义（direct 路径缺引用类联动）归 `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md`，不在本核对表里重复。
★0907 其中结构连接 `ADSTCH` 那一条 direct 已接线（`src/data_interface/structural_connection.rs`，正向读 + 回指核对 + `STRU` 门，
开关 `model_structural_connection_cascade` 默认关）；孔洞那条 core 走的是 owner 子树成员表（`DFIND` = 成员导航），整根重生成构造性覆盖、不需要接；
电缆那两条（`CWBRAN` → `CLIST`、`EQUI`/`ELCONN` → `ELCXRF`，反向到 `CABLE`）★0907 同库版已接线（`src/data_interface/cable_endpoint.rs`，
点查 T 端文件自带的 `STAREF` / `ENDREF` 反向表，开关 `model_cable_endpoint_cascade` 默认关；跨库边待跨库版），电缆几何未按路由落地、暂不显。

- 我方：✅ 分层等价。`OperationEffectSummary.max_dchc` 保留原始码没问题，
  但不该让它调节重生成范围。`TransformOnly` 是我们**主动多留**的便宜路径（省不是漏），
  按计划 §1 不取消。

### R6 `Refresh(当前 VIEW)` 清空整个待办队列 ★新增 ~~★0904 改判 ◐~~ ★0907 重标 ✅（`Refresh` 那条已落地 `discard_pending_for_full_rebuild`；视图切换判「无对应物」，待拍板 W2.2）

```
Refresh(e):                                     # 0x1047E800
    if e.type() == NOUN_VIEW and e == m_view:
        m_queue.end = m_queue.begin             # 整队丢弃
```

视图自己被刷新时，所有排着的增量工作项**作废**——反正整个视图要重画。

**★0904 队列作废其实有三个点，不是两个。** 除 `Refresh` 与 `Finish`（R29）之外，
**视图切换**也整队丢弃，在 `LoadSheet`（`0x1047E110`）里：

```
0x1047E143  call DB_Element::operator!=      # 传进来的 picture 与当前 m_view 不同才继续
0x1047E174  cmp  eax, [NOUN_VIEW]
0x1047E185  mov  eax, [ebx+24h]              # m_queue.begin
0x1047E188  mov  [ebx+28h], eax              # m_queue.end = begin —— 整队丢弃
```

所以队列的生命周期是三条边：一次 `Update`、一次 `Refresh`、一次**视图切换**。

- 我方：**`Refresh` 那一条已落地**（T3.4，2026-08-28）——
  `model_update_pending::discard_pending_for_full_rebuild(dbnum)`，由 `model_rebuild::start`
  在 `sync_and_seed_model_coverage` **之前**调用，`stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work`
  钉着顺序，三条边界（`room_recalc_*` / 别的 dbnum / `dbnum = 0`）有意留下。
  **视图切换那一条没有对应物**，而且我们这边「什么算切换视图」本身还没定义（见 §1.5）。
  **登记为待议，不是缺口**。

★0906 「什么算切换视图」的三个候选与判词（开发计划 W2.2；建议登记「无对应物」、不落代码，待拍板）：

| 候选 | 我方动作 | 判词 |
|---|---|---|
| a. 切 MDB | `UpdateScope::resolve` 换 MDB | 换 MDB 必然整库重建，已被上面 `Refresh` 那条（`discard_pending_for_full_rebuild`）覆盖，不必另立 |
| b. 收窄 watch scope | `watch_scope::admits(dbnum)`（`watch_scope.rs:126`）收窄（ADR-058 D9 的调试准入开关） | **不采**。core 敢整队丢弃是因为新视图会从头画；我们收窄后**没有任何东西替被丢掉的 pending 补种**，再扩域时那些库的模型静默陈旧。要采得先证两件事——在途窗口与收窄互不干扰、再扩域有补窗——今天两条都没有 |
| c. 切投影域 | `ModelMemoryStore` 进程内投影 → 重启 | 重启本来就清（ADR-050 `clear_stale_at_process_start`），不必另立 |

三个候选要么已被 R6 的 `Refresh` 对应物覆盖（a、c），要么前置不成立（b）。
`narrowing_the_watch_scope_discards_out_of_scope_pending` 那条测试**不写**；若日后 b 的两个前置补齐，另开一条。

### R7 新建元素会被并进视图 ID 清单并写库 ★新增 ⚪

```
NewModelToUpdate(e):                            # 0x1047E6E0
    ...门...
    if !IsPending(e, New):
        idlist = GetIDList()
        if idlist.active and !idlist.exists(e):
            owner = SignificantOwner(e)
            if owner.isOK(): AddIDList(owner)   # 0x1047BF80 → addElement + writeDB
    ModelToUpdate(e, New)
```

`AddIDList` 会 `PDMS_Idlist2::writeDB()`——**这是一次落库副作用**。

另有 `NewModelNotify`（`0x1047E670`）走同一个 state 1 但**没有**这段副作用。
证据文档 §1 只列了 `NewModelToUpdate`，把这两个入口混为一谈了。

- 我方：⚪ 不可比——ID 清单是 E3D 视图的绘制清单，我们没有对应概念。
  **但它决定了 R22 的判据**（见下），所以移植 `AbsentPrimitives` 时必须先想清楚
  "我们这边什么东西扮演 ID 清单"。

### R8 `Update` 的重入保护 ★新增 ⚪

`Update` 入口检查 `m_inDraw`（`+0x05`）；pass 2 每次 `DrawModel` 前置 1，
`Finish` 之后清 0。因为 `DrawModel` 发的是 PML 命令，会同步回调进来。

- 我方：⚪ 我们的重生成不是同步回调，`model_concurrency.rs` 另有一套。不构成缺口，
  但说明**队列消费期间不能接受新的入队**这条约束在 core 侧是硬的。

## 4. 粒度：`GranularityExpansion`（R9–R16）

指令流还原（`0x1047D8C0`，伪码不可用——见 §1.4）：

```
GranularityExpansion(e, state):
    if IsSignificant(e):                              # 0x1047D915 cmp / 0x1047D91A jz
        push(e, state)                                # 0x1047D942
        AncestorDeletes(e, state)                     # 0x1047D94B
        AbsentPrimitives(e, state)                    # 0x1047D954
        for m in Members(e, Significant):             # 0x1047D97E  mode 0
            push(m, state == Deleted ? 3 : 4)         # 0x1047D9B4 cmp ebx,3 → var_14 = 3|4
            AncestorDeletes(m, state)                 # 0x1047D9C9
            AbsentPrimitives(m, state)                # 0x1047D9D2
        return                                        # 0x1047DA41 jmp 过尾部的 AncestorDeletes

    if !IsPrimitive(e):                               # 0x1047DA4E
        return                                        # 0x1047DA55 jz —— 什么都不做

    if m_granularityMode == 0:                        # 0x1047DA5B（恒真，§4.8）
        if state == Deleted: push(e, Deleted)         # 0x1047DA65 / 0x1047DA89
        else:
            owner = SignificantOwner(e)               # 0x1047DA99
            push(owner, state)                        # 0x1047DAC1
            AbsentPrimitives(owner, state)            # 0x1047DACD
    else:                                             # 死代码，0x1047DAD7
        push(Members(e, Negative) ? SignificantOwner(e) : e, state)

    AncestorDeletes(e, state)                         # 0x1047DBD3
```

**这一段证据文档 §6 写对了**，本轮从指令流逐跳复核通过。

### R9 粒度由 noun 位决定，不由类型名单决定 ◐（★0906 T2b 已接线、`model_significant_over_point` 默认关；direct 按位枚举，MDU 名单是我方主动加的一层）

- 我方：**判定链仍是手写名单** `DEFAULT_DELIVERY_UNIT_TYPES`（BRAN/HANG/SUPPO/EQUI）+
  `COARSE_HIERARCHY_NOUNS` + `is_loop_container_noun`。P1 的对账已量化这个差
  （`docs/evidence/2026-08-29-noun-granularity-reconciliation.md`）：方向几乎全是多做，
  真漏只有 `AIDTEX` 一个 noun，故分叉点判为**加层**而不是换判据。
  T2a 已把位表接进生产（`noun_is_significant` / `noun_is_primitive`，快照外保守取真），
  ~~**但还没有消费者**——第一个是 T2b。~~ ★0906 第一个消费者已落：T2b
  （`generation_root.rs::significant_point_container_is_its_own_root`，`model_significant_over_point` 默认关，
  判据 `core_significant_bit == Some(true)`）。契约测试 C1-1 / C1-2 / C1-3 与「恰好只差 AIDTEX」已落，见 §9 本行。

### R10 significant 变化 → 整块重画 + 块内 significant 后代逐个抹掉 ~~❌~~ ★0904 改判 ⚪

块内后代入队时的 state 是 `state == Deleted ? 3 : 4`，两者在 `Update` 里**都只 Erase 不 Draw**。
"块级颗粒"的确切含义：不是"重画子树里每一个"，是"**重画块，顺手删掉块内那些自己也曾单独成块的行**"。

- ~~我方：**无**。同一几何可能在模型里出现两份。计划 T3.2。~~
- **★0904 改判 ⚪ 不可比：这条重复在我们这边结构上不可能。**

  先看 core 抹的到底是什么。`EraseSignificant(DB_Ref& ref)`（`0x1047CF00`）：

  ```
  root = GetDesiCPS()                    # 0x1047CF3F
  seg  = root.findSegment(ref, …)        # 0x1047CF64  ← 按这个 ref 找它自己的段
  if seg.isOK(): seg.deleteSegment()     # 0x1047CF7A  ← 整段删掉
  ```

  **core 的绘制段是按 ref 建的**（§4.2）：一个 significant 后代 X 单独被画过时，
  库里有一个键为 X 的段。祖先块 A 重画时，A 的段会把整棵子树的图元都装进去——
  **X 的段不删，同一份几何就同时躺在两个段里**。R10 就是删这个。

  **我们没有这个问题，因为键的形状不同。** ADR-060 D1/N1：行 id **就是元素的记录 id**
  （`<word0>_<word1>`，隐式管身是身份摘要），**不带根**——D1 那一轮正是把
  `direct_` / `derived_` 这些前缀全去掉了。所以不论 X 的几何是被 A 的投影产出的
  还是被 X 自己的投影产出的，**写的都是同一行**（先删后插，`INSERT IGNORE`），
  一个元素在每张表最多一行。**「同一几何两份」需要键里带根才可能发生。**

  **这条判词的有效期 = 「记录 id 与根无关」这条不变量的有效期。** 谁哪天为了按根
  scoped delete 方便，把根塞回 id 里（那正是 ADR-060 之前的形状），R10 立刻从
  「不可比」变回「缺」。ADR-060 的 N1 就是守这个的。

### R11 `SignificantMembers` 只穿过 significant 链 ★精确化 ~~❌~~ ★0907 单独过判据后改 ⚪（direct 枚根不挡、5,366 个嵌套根两边口径不同，但差别只落在 core 的段抹除上；理由与有效期同 R10，见本条末段）

`Members(e, mode=0)`（`0x1047E240`）：种子是 `e` 的**直接成员**，用显式栈做 LIFO；
每弹出一个成员，**不 significant 就既不收集也不下潜**（`0x1047E37E` / `0x1047E381`）。

**推论（可判真假）**：非 significant 的子节点会**挡住**它下面的 significant 孙节点——
后者不会被收集。这不是遍历的副作用，是判据本身。

三个模式的完整规则：

| mode | thunk | 收集 | 下潜 | 指令 |
|---|---|---|---|---|
| 0 Significant | `0x1047E9C0` | significant 成员 | **只穿过 significant 成员** | `0x1047E37E`–`0x1047E3ED` |
| 1 Primitive | `0x1047E7E0` | primitive 成员 | **全部成员，整棵子树** | `0x1047E3F9`–`0x1047E46C` |
| 2 Negative | `0x1047E650` | primitive **且** negative | 只穿过 primitive **且非 negative** 的成员 | `0x1047E475`–`0x1047E4EC` |

mode 2 的"收集了就不再下潜"是本轮新看清的（`0x1047E48C jmp` 直接跳到下一轮）。
它挂在死代码上（§4.8），登记备查。

返回值是 `out` 非空。

- **★0907 我方（单独过了一遍判据，不随 R10 带；此前一次随 R10 改 ⚪ 被驳回，这次补了代码与真库两头的证据）**：
  direct 枚根 `enumerate_generation_roots_in_subtree`（`generation_root.rs`）**不挡**——成员一律入栈，只有「进了交付单元」
  这一位向下传；非 significant 子节点 C 下面的 significant 孙节点 G 照样成根（注释里「显著节点名下的显著节点也是根」说的就是这个）。
  core 的 `Members(A, Significant)` 会被 C 挡住、收不到 G。**真库量了一次**（486 个 DESI 库，临时探针按同一段枚举口径复刻，
  位全部取自快照 `core-noun-granularity-e3d31.json`、没走兜底；产物 `docs/evidence/2026-09-07-r11-nested-root-shape.log`）：
  根 181,611 = 顶层（没有根祖先）95,049 + 嵌套 86,562；嵌套里中间链全 significant、core 也收得到的 81,196，
  **被非 significant 中间节点挡住的 5,366**（78 个库，它们的子树共 58,615 个元素）。形状集中在
  `HPANEL → [HCTOUT] → HCLIP` 3,008、`FRMW → [GENSEC/JLDATU/PLDATU/FIXING] → TMPL` 910、`CWBRAN → [CTMTRL/CTSTRA/CTFEAT] → TMPL` 546、
  `FRMW → [SCTN/CMFI] → TMPL` 291、`HANDRA → [POSTSE/HRPOST] → TMPL` 270、`TRNCEL → [TRNCUC/TRNCUS] → TRNCUR` 53
  （`TMPL` 在快照里 significant = toplevel = true，成根是 core 自己的位说的）。
- **判 ⚪ 的理由**：(1) 这条规则在 core 里唯一的活消费者是 R10 的块内成员抹除——`Members(·, Significant)` 只在 `GranularityExpansion`
  的 significant 分支里被调，效果是「A 重画时**不**抹被挡住的 G 的段」；我方行按元素 id 落（ADR-060 D1/N1），A 的整棵子树投影重写的
  就是 G 那些行本身，没有「段」可抹、也不会出现两份——与 R10 同一个理由。(2) 「G 是不是独立单位」两边一致：core 的
  `GranularityExpansion(G)` 只看 G 自己的位、不看父节点，G 变了照样 `push(G)`；我方 G 也是根。(3) 剩下的差别只有「A 变了时 G 的子树
  被多算一遍」——那是嵌套根重复覆盖（`duplicate_coverage_elements`，W3.2 量化），是「慢」不是缺。
  **有效期同 R10**：谁把根塞回行 id，R10 与这条一起回 ❌。

### R12 primitive 且非 significant 的变化 → 上卷到 SignificantOwner ◐（旧 PE 路径名单 + 深度上限 32；W2.4 先量真库 owner 链深度与环再定）

并对该 owner 做 `AbsentPrimitives`。

- 我方：◐ `resolve_element_generation_root` 形状相同（MDU 优先 → 跨 loop 容器 →
  第一个非容器 owner → 自身），但判据是名单、且有深度上限 32。

### R13 既非 significant 又非 primitive → 完全丢弃 ◐（第一批 1272 noun 的丢弃门已落、`model_entry_drop_no_geometry` 默认关；第二批 223 项未评审；开门前置四条见开发计划 W2.1）

**连 `AncestorDeletes` 都不做**（`0x1047DA55` 的 `jz` 直接跳到函数尾）。

- 我方：◐ 已落默认关闭的 `model_entry_drop_no_geometry` 第一批门，只丢 1272 个
  字典无 `geomset` / `extrusion` / 非零 `graphics_behaviour` 的已知普通根 noun；未知、
  删除、类型变化保守保留，数据明细不动。另 223 个带几何代理的 noun 尚未评审，因此
  尚未达到 core 的全量丢弃口径。真实 ams8000 266→267 基准中 legacy 为 2→1 work
  items、91→0 额外重算单元；direct 该窗本来就是 1→1，九个固定窗在开关两态均通过。
  见 `docs/evidence/2026-09-06-no-geometry-entry-gate/README.md`。

### R14 `SignificantOwner` 含自身、无深度上限 ◐（同 R12：`MAX_ANCESTOR_DEPTH = 32` + 遇粗层级容器即停，W2.4 量过再定）

```
SignificantOwner(e):                     # 0x1047E9E0
    cur = e
    while cur.isOK():
        if getBool(cur, 0x5657A0A): return cur    # 0x1047EA54 jnz
        cur = cur.owner()                          # 0x1047EA5D
    return cur                                     # 无效引用
```

三点与我方不同：**从元素自己开始判**；终止条件是 noun 位而不是"遇到 SITE/ZONE/WORL"；
**没有深度上限**，也没有 loop 容器特例——loop 容器本来就不 significant，自然被跨过。

- 我方：◐ `MAX_ANCESTOR_DEPTH = 32`，且 `collect_chain` 遇到粗层级容器就停。
  深度上限是**防御性**的（防 owner 环），不是语义——但它会让超深链静默截断。

### R15 删除的 primitive 不上卷 ✅（语义一致，实现不同）

`state == Deleted` 时 push 自己，不 push owner。

- 我方：删除集收敛到最顶端 + `delete_inst_relate_subtree`。形状不同，需实测比对。

### R16 负实体上卷是死代码 ⚪

`m_granularityMode`（`+0x20`）恒为 0：全类地址区间 `0x1047BC40`–`0x1047ED00` 里
`+20h` 只有**一处写**——构造函数 `0x1047BCCB` 的 `mov dword ptr [ebx+20h], 0`，
其余六处全是读；53 个方法无 setter；唯一构造点 `sub_10425CD0` 也只写 0。

连带：`IsNegative` 与 `Members(Negative)` 在本类内无活调用者；
字段 `0x92663` 无消费者，**P0 不必导 negative 表**。`ExistsCPS` 里
`cmp dword ptr [esi+20h], 1`（`0x1047D2FD`）那条同理不可达。

## 5. 去重：`IsPending`（R17–R20）

`0x1047DDD0`，在 `ModelToUpdate` 里作为最后一道门。**三个 state 三套判法，不是一套。**

> ★0907：R17–R21 与 R24 的标题保留 ❌（我方仍是同 refno 才合并、没有祖先抢占与 `AncestorDelete` 标记），
> 但 §9 把这一族定性为「**慢**」——多做不是漏做；做不做由 W3.2 先量 `nested_roots` / `duplicate_coverage_elements` 再定。

### R20 去重键：非 primitive 元素按它的 SignificantOwner 判 ★新增 ❌（慢）

```
key = IsPrimitive(e) ? e : SignificantOwner(e)
```

三个 state 共用这一步。也就是说 core 的去重**先归一化到块**，再在块的层面比。

- 我方：❌ 我们的键是 `(action, target_refno)`，`target` 已经是解析后的生成根，
  形状上接近，但我们对 `Skip` / `TransformOnly` 类目标不做这个归一。

### R17 `state == Changed(0)`：沿 owner 链找 New 或 Changed，再看子孙 New，再看祖先删除标记 ★更正 ❌（慢）

```
cur = key
while cur.isOK():
    if queue has (cur, New):      return true     # 0x1047DE4x 内层扫描
    if queue has (cur, Changed):  return true
    cur = cur.owner()
if key.isOK() and IsDecendantPresent(key, New): return true    # 0x1047DFD4
if key.isOK() and IsPresent(key, AncestorDelete=2): return true
return false
```

`IsDecendantPresent(key, s)`（`0x1047DC30`）= 队列里存在一条 state 为 `s` 的记录，
它沿 owner 链能走到 `key`（即该记录是 `key` 的**子孙或它自己**）。

证据文档 §9 把三个 state 归纳成"沿 owner 链逐级上行"，只有这一个 state 是这样。

### R18 `state == New(1)`：沿 owner 链只找 New ❌（慢）

`key` 本身不 OK 就直接返回假，不做后面两步。

### R19 `state == Deleted(3)`：只看自身，**不上行** ★更正 ❌（慢）

```
return IsPresent(key, Deleted) or IsPresent(key, MemberOfChangedSignificant=4)
```

没有 owner 链遍历。证据文档的框架式描述在这里会误导实现者。

### 我方现状（R17–R20 共同）

`BTreeMap<(ModelWorkAction, String), ModelWorkItem>` —— **同一个 refno 才合并**。
祖先已经排了整根重生成、后代还各排一条，在我们这边是两条工作项。
**这是"多做"不是"漏做"**，所以计划把 P3.3 排在最后。

## 6. 收尾与执行（R21–R29）

### R21 `AncestorDeletes`：判据对了，终止条件写反了 ★更正 ❌（慢，随 R17–R20 归 W3.2）

```
AncestorDeletes(e, state):                        # 0x1047C060
    if state not in {Deleted, MemberOfChangedSignificant}: return   # 0x1047C079/07E
    cur = e.owner()
    while cur.isOK():
        if getBool(cur, 0xA103E)                  # 0x1047C0E6 jnz → 标记
           or getBool(cur, 0xBBD5ADC)             # 0x1047C109 jnz → 标记
           or getBool(cur, 0x5657A0A):            # 0x1047C131 jz  → 跳过本级
            if not (queue has (cur, AncestorDelete)):   # 0x1047C140 cmp [esi+14h],2
                push(cur, AncestorDelete=2)             # 0x1047C192
        cur = cur.owner()                         # 0x1047C1A6 —— ★ 无条件继续
```

判据 = **`IsPrimitive(anc) ∨ IsSignificant(anc)`**（证据文档 §8 这条对）。

**但"命中已标记祖先时整条上行链终止"是反的。** 逐跳追下来：
`0x1047C14F jnz → 0x1047C1F4`（`mov al, 1`）→ `0x1047C1F6 jmp → 0x1047C15F`
（`test al, al`）→ `0x1047C161 jnz → 0x1047C197` —— `0x1047C197` 正好落在
push 调用（`0x1047C192`，5 字节）之后，**继续往下走到 `0x1047C19D` 取 owner 并循环**。

所以命中只是**跳过这一级的 push**，上行链照走到顶。**整条 owner 链上每一个
primitive-或-significant 的祖先都会被标记**，不是只标最近的那一个。

按证据文档原话实现，删除路径的祖先标记会少一大半，R24 的第二个消费者也就跟着失效。

- 我方：❌ 无对应物。计划 T3.3 引用的是错的那一版，已同步更正。

### R22 `AbsentPrimitives`：缺失图元回收 ~~❌~~ ~~★0904 改判 ◐~~ ★0906 ✅（按根对账已接两个生产调用点 + 8019 testbed 闭环，§9 / §10；W1.3 收尾：C3-1 与 E 侧护栏两条测试待补）

```
AbsentPrimitives(e, state):                       # 0x1047BE10
    if state in {Deleted, MemberOfChangedSignificant}: return   # 只在非删除类动作
    idlist = GetIDList()
    for p in Members(e, Primitive):               # mode 1：整棵子树的 primitive
        present = idlist.active and idlist.exists(p)
        if not present: push(p, AbsentPrimitive=5)
    idlist.release()
```

即：**重画一个块之前，把块内"模型里有、当前 ID 清单里没有"的图元行清掉。**
这是线上偶发"孤儿 mesh 行"该由谁负责的答案。

- ~~我方：**无**。计划 T3.1，最高优先。~~
- **★0904 我方有，而且口径比 core 强。** `model_db_adapter::persist_root_in_chunks`
  落一根之前先做一次**按根对账**：`projected_geometry_ids_on(db, root)`（库里挂在这根下的已投影几何）
  减去本轮投影出的 `record_id` 集合，差集就是 `removals`，连房间边一起清
  （清边排在删几何那一块、排重算排在盖章那一块，源码里有注释钉着为什么不能对调）。
  `a_reprojected_root_resets_its_branch_tubes_and_drops_stale_geometry`（`e3d_model_service.rs`）
  钉着这条形状。**比 core 强在对账源**：core 比的是一份可能不活跃的 ID 清单（R23 那个坑），
  我们比的是库里实际存着的行，没有「清单不可用」这一档。
- **真正的待办不是实现它，是接线**：ADR-060「未做」第三条写着
  「适配器还没有生产调用点——什么时候落库、按根还是按批，由上层编排决定」，
  而旧发布事务那套 `existing_geometry_ids` 已经从 `e3d_model_service.rs` 撤掉。
  也就是说落库侧此刻在切换中间态。**T3.1 应当从「实现缺失图元回收」重述为
  「把适配器接上生产编排，并验按根对账在真库上确实删掉了孤儿行」。**

### R23 ID 清单不活跃时，全部 primitive 判为缺失 ★新增 ★0904 已结案 ⚪

`idlist.active`（`PDMS_Idlist2 +0x18`）为假时 `present` 直接取 0——
于是**整棵子树的 primitive 全部入队 state 5**，在 pass 3 被逐个擦掉。
而 pass 2 刚刚把这个块画完。

`GetIDList`（`0x1047D650`）在视图没有绑定清单元素时返回一个"空清单"对象，
`+0x18` 应当就是这个"有没有绑定"的位。

~~**这条不下判词。** 移植 R22 之前必须在 live 进程上看一眼实际取值。~~
**★0904 静态就判死了，不必找 live 进程**（证据
`docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md` §1）：

```
PDMS_Idlist_base::isValid()          0x1041AE10:  mov al, [ecx+18h]; retn   ← +0x18 就是 isValid
PDMS_Idlist_base::PDMS_Idlist_base() 0x10479192:  mov word ptr [esi+18h], 100h
                                                  ← 默认构造：+0x18 = 0（假）
PartialUpdateDesiMgr::ExistsIDList() 0x1047D364:  cmp byte ptr [ecx+18h], 0 → 假就直接返回 false
```

两个 `PDMS_Idlist2` 构造（`0x104C5930` 带元素 / `0x104C5AC0` 默认）本身都不写 `+0x18`，
只清 `+0x1C/+0x20/+0x24` 后转交基类——那一位完全由基类的两个构造决定。
所以 `GetIDList` 走 else 分支（VIEW 无有效 `IDLN`）时返回的清单 `isValid` **恒为假**，
`AbsentPrimitives` 于是把整棵子树的 primitive 全判缺失。**是构造函数写死的，不是概率问题。**

**给移植的纪律**：照抄这条 = 照抄一个会把刚画好的东西擦掉的分支。
我们这边的对应物应当把「清单不可用」当成**前置条件失败**（不做回收、出声），
而不是当成「全部缺失」。

### R24 `AncestorDelete(2)` 有**两个**消费者 ★更正 ❌（慢：第一个消费者随 R17 归 W3.2；第二个属擦除策略，我方 ⚪ 见 R25）

证据文档 §2 说 state 2"不处理——只作为 `IsPending`/`IsPresent` 的抢占标记"。
第一个消费者对，但漏了第二个：

1. **`IsPending(e, Changed)`** → `IsPresent(key, 2)`（R17 最后一步）。
   注意**只有 Changed 这一个 state 看它**，New 和 Deleted 都不看。
2. **`EraseModel(DB_Ref&)`（`0x1047C6D0`）** → 队列里**只要存在任意一条** state 2 记录，
   图元擦除就从 `ErasePrimitiveFromUnknownModel` 切到 `ErasePrimitiveFromCandidateModel`：

```
EraseModel(ref):
    if EraseSignificant(ref): return true
    if IsAncestorDeletesPresent():                      # 内联的 0x1047DC00
        return ErasePrimitiveFromCandidateModel(ref)    # 0x1047CB80
    else:
        return ErasePrimitiveFromUnknownModel(ref)      # 0x1047CCF0
```

这是**全局条件**，不是按元素判——一次 `Update` 里只要有任何祖先删除标记，
所有走 `DB_Ref` 重载的擦除都换策略。

### R25 `EraseModel` 是两个函数，按 significant 位分派 ★新增 ⚪

| 重载 | 地址 | 分派 | 谁调用 |
|---|---|---|---|
| `EraseModel(DB_Element&)` | `0x1047C670` | `getBool(e, 0x5657A0A)` 真 → `EraseSignificant(e.ref())`，假 → `ErasePrimitive(e)` | pass 1 的 state 0/1；pass 3 的 state 5 |
| `EraseModel(DB_Ref&)` | `0x1047C6D0` | 见 R24 | pass 1 的 state 3/4 |

指令：`0x1047C683 push 5657A0Ah` → `0x1047C6A5 jz` → 两条路。

- 我方：⚪ 我们的模型行删除不分这两类。**但它给出一条设计约束**：
  "块的行"和"图元的行"在 core 里是两套存储、两条擦除路径。
  移植 R10（块内成员清理）与 R22（缺失图元回收）时，删的**不是同一种行**。

### R26 `Exists` 对 significant 元素递归全子树 ★新增 ⚪

```
Exists(e, idlist):                                # 0x1047D050
    if IsPrimitive(e):
        if ExistsCPS(e, mode=1):                       return true
        if idlist.active and idlist.exists(e):         return true
    if not getBool(e, 0x5657A0A):                      return false   # 0x1047D0E7
    for m in Members(e, Primitive):                                   # 0x1047D112
        if Exists(m, idlist):                          return true    # 0x1047D127 递归
    return false
```

伪码里这后半段整个不见了（`getField(v6); return 0;`）。语义："这个元素或它子树里的
任何图元，现在还有没有可画的东西"。

### R27 `Update` 三遍 + 一次收尾 ⚪

```
Update():                                          # 0x1047EAB0
    if !m_enabled or !m_view.isOK() or queue.empty() or m_inDraw or m_muted: return
    idlist = GetIDList()

    pass 1: for rec in queue:                      # 就地遍历，可删元素
        if rec.state <= 1:                         # Changed / New
            if !EraseModel(elem(rec)) and !Exists(elem(rec), idlist):
                从队列中就地删除该条，不前进游标        # R28
        elif rec.state in {3, 4}:
            EraseModel(ref(rec))                   # ★ 另一个重载，见 R25

    pass 2: for rec in queue: if rec.state <= 1: m_inDraw = 1; DrawModel(rec)
    pass 3: for rec in queue: if rec.state == 5: EraseModel(elem(rec))
    Finish(); m_inDraw = 0
    idlist.release()
```

`DrawModel`（`0x1047C350`）不是函数调用，是发一条 PML：

```
DLL_PMLCommand::Run("PUPDES " + <view ref> + " MODEL " + <element ref> + " FORCE SUPPRESS")
```

- 我方：⚪ 我们跑自研生成管线，重画动作不可比、也不需要比。
  **但三遍的顺序是可比的**：先全擦、再全画、最后清缺失图元。

### R28 pass 1 的"擦不动且不存在 → 从队列删除" ~~⚪~~ ★0906 ✅（direct 由 `roots_T` 枚举排除、兼容路径终态复核；W3.1 收尾：「T 端读失败 → degraded」那条测试待补）

`!EraseModel(rec) && !Exists(rec, idlist)` 同时成立时，**该条被就地移出队列**，
于是 pass 2 不会画它。意思是：**不画本来就不在视图里的东西。**

- 我方：✅ direct 文件窗口从 target 侧 `roots_T` 枚举生成根，窗口内已经消失的目标不会进入
  `RegenRoot`；兼容 PE 路径继续保留执行前终态存活复核。因此两条生产路径都不会为已消失目标
  执行空生成。

### R29 `Finish` 清空队列 ⚪

```
Finish():                                          # 0x1047D380
    hasDeleted = queue 里存在 state == 3 的记录
    m_queue.end = m_queue.begin                    # 清空
    if hasDeleted: <视图侧的一次额外通知>
    <从 Resolver 取服务，回调 vtable[1](m_view)>
```

队列在 `Finish` 里清空，不在 pass 3 之后逐条清。配合 R6（`Refresh` 也清空），
队列的生命周期是"一次 `Update` 或一次 `Refresh`"。

## 7. 本轮对证据文档的更正

| 位置 | 原文 | 实际 | 影响 |
|---|---|---|---|
| §2 | state 2「不处理」 | 有第二个消费者：切换图元擦除策略（R24） | 移植删除路径时会漏掉一整条分支 |
| §3 | 门顺序 enabled→view→muted→XGEOM→DESI，五道 | 实际六道且顺序不同，多一道 `isValid`（R3/R4） | 排查线上问题时对不上 |
| §8 | 「命中已标记祖先时**整条上行链终止**」 | 只跳过该级 push，**上行照走到顶**（R21） | **计划 P3.3 会实现错**，祖先标记少一大半 |
| §9 | 三个 state 都「沿 owner 链上行」 | 只有 Changed 上行；Deleted 只看自身（R17/R19） | 删除去重会做成过度合并 |
| §1 | 只列 `NewModelToUpdate` | 还有 `NewModelNotify`，差一个落库副作用（R7） | — |

新增六条证据文档没有的规则：R3、R6、R7、R8、R20、R23，以及 R25/R26/R28 三条实现约束。

## 8. 仍未钉死的

- `IsPrimitive` 两个位各自的**字典名**未知——只知道 `0xA103E` 跨版本稳定。
- ~~R23 的 `idlist.active == false` 语义，必须在 live 进程上看。~~ ★0904 已结案，见 R23。
- ★0904 **`graphicsBehaviour` 的消费面已经查完，并且带出一条我们没梳理过的通道。**

  字段 `0x4DCE6F` 在 `Core3D.dll` 里四处 `push`、三个函数，`core.dll` 里**零命中**：
  `DES_DrawList::isGraphicsIgnoredBetween`（已记）、`DES_DrawList::hasGraphicsIgnoreFlag`
  （`0x1051A920`，逐元素判 `== 1`、不上行）、`sub_1052F2E0`。比较值只有 `1` 与 `2`，
  **没有一处比 3**——`gb == 3` 那 72 个 noun 在 Core3D 里没有代码读。

  `sub_1052F2E0` 是 **`DDES_GraphicsMngr` 的变更过滤器**：`sub_1052E9F0` 里三个 `MTR_Entry`
  名字串与三次调用一一对应——`postGetWork`（`0x1052EA35` → `0x1052EAF5`）、
  `postUndo`（`0x1052EB7B` → `0x1052EB9D`）、`preRedo`（`0x1052EBAC` → `0x1052EBCE`）。
  它遍历一个 `std::set<DB_Element>` 变更集，**gb==2 的层级容器整个跳过**，
  **gb==1 的数据容器只留最上面那个**（属主也是 gb 1/2 就跳），再过 `WORL`/`MNUM` 与
  「`db_go_to_element` 切不切得到 CE」，剩下的插进一个有序结果集。
  `0x1052F486` 不是分支语义，就是 `std::_Tree` 迭代器 `++`（= 跳过这一条）。

  **新缺口**：核对表整份讲的是 `PartialUpdateDesiMgr`（DRAFT 图纸侧，§1.5）。
  `DDES_GraphicsMngr` + `DES_DrawListRefresh` + `DES_DrawList` 的 TOPF 家族
  （`getAncestorTOPFElement` / `getChildTOPFElements` / `getElementsAtOrBelowTOPF` /
  `getPromotedTOPFElement` / `getPrimitiveOrTOPF*`，加 `DES_DrawListManager::findTopLevelElement`
  与 `promoteElement(s)`）是**3D 模型视图侧**，而 **`GETWORK` 正是「别人 SAVEWORK 了，
  我把变更收进来」——跟 gen-model 干的事最像的就是它**。~~这条通道一次都没梳理过。~~
  细节见 `docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md` §3.1–3.3。
  **★0906 已梳理**（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §5）：过滤器第二段对每个存活元素
  `FNDTOP` 折叠到 `toplevel` 顶层件，owner 不在集合里的直接调 `change/EVALAT(top, noun, REDRAW=4)`；
  `postGetWork` / `postUndo` / `preRedo` 三入口同一条路。重画单位 = TOPF（R0-3 ★0906 补充），联动规则 = `EVALAT`（R5 ★0906）；
  与我方逐条对表见 `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` §1。
- ★0904 **R10 的反向风险已验：今天不咬人，但它靠两个互不相干的事实撑着。护栏已补两条。**

  问的是：按根对账（R22）会不会把**嵌套生成根**的行当孤儿删掉？
  （`enumerate_generation_roots_in_subtree` 是会出嵌套根的——显著节点名下的显著节点也是根。）

  两边都读过了，**答案是不会**，理由是这两条：

  | # | 事实 | 出处 |
  |---|---|---|
  | 1 | 祖先链是**全库**的：`scan_index` 扫整库建 `owners`，`ancestor_chain` 一路走到库顶。所以嵌套根 D 的记录，`anc` 里**带着**祖先根 A | `e3d_model_service.rs::scan_index` / `ancestor_chain` |
  | 2 | 生成是**整棵子树**的：`generate_subtree_with_cache` 从根 DFS 到底，**没有「遇到嵌套根就停」这回事** | `vendor/e3d-model/src/pipeline.rs` |

  两条合起来：重投影 A 会把 D 名下的元素**一并重新产出**，于是 `records_of_root(A)`
  与库里 `anc CONTAINS A` 的集合口径一致，差集里只剩真孤儿。

  **但这个安全是巧合出来的，不是设计出来的**：

  - 事实 2 一旦被「优化」掉（**「祖先根生成时跳过嵌套根，省重复计算」是个太自然的想法**，
    而且 core 的 R10 干的正是这件事），`persist_root(A)` 立刻开始静默删掉 D 的行；
  - 事实 1 一旦改成按根子树建 owners（省内存），D 的记录就不再挂在 A 下，对账反过来**漏删**。
  - `ModelMemoryStore` 仍是进程内投影，但生产启动已有 `restore_projection_on` 从持久模型行与
    `gen_root` 回灌。恢复不完整或回执缺失时，2026-09-06 的完整性门会禁止差集删除。

  **护栏已补**（★0904 晚，`model_db_adapter::tests::reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows`）：
  一个 A ⊃ D 的嵌套根形状，提交 A 的整子树投影，`persist_root_on(A)` 之后断言 **D 名下的行还在**、
  只有真孤儿进 `removals`；反方向 `persist_root_on(D)` 同样一行不许删。它盯的是**两边同一个口径**
  ——库那边 `anc CONTAINS <根>`、存储那边 `records_of_root`（同一张祖先索引）。任一边收窄成
  「只算直属这一根的」，这条立刻红。

  **完整性门测试**（`model_db_adapter::tests::an_unreceipted_partial_projection_does_not_delete_nested_root_rows`）：
  投影收窄且没有匹配回执时，不查询差集、不删除 D 的行。另由
  `reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection` 钉住错配回执失败与
  完整空投影清理两条边界。
- `ExistsCPS` / `WWSegment` / `PDMS_Idlist2` 的内部结构——core 自己的绘制存储，
  移植时只需知道它们是"已绘制段"和"应绘制清单"两个集合，内部布局不必抄。
- ~~§4 末尾登记的同族字段（`847458`、`606263`、`602413`、`595979`、`599813`）语义未追。~~ ★0906 已解（逆 hash + core.dll 谓词实读）：
  `606263` = `EQUC` 设备子件（`IECOMP`）、`602413` = `PIPC` 管道元件（`IPCOMP`）、`595979` = `HNGC` 支吊架元件（`IHCOMP`）、
  `599813` = `HULC` 船体构件、`847458` = `IMAP`（后两个本轮无消费者）。`0x92663` = `HOLC`（`INCOMP`）。
  见 `docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md` §2.2a。
- 2.10 `sub_102C25F0` 里 `*(this+9) == 779672` 那条早退——它属于另一个类，
  但同样在读 significant 位，可能是一条例外规则。
- `PostSetRefListAttribute`（core.dll `0x5EAA7A3`）的 back-ref 表结构——ADR-003 那条线。

## 9. 缺口汇总（按"缺了会不会导致模型错"排序）

**★0904 整表按现码 + 活桥重核过一遍。改判的理由都写在对应规则里。**

**「错」这一档现在是空的。** 原来的四条各自有了去处：R22 是已实现（卡接线）、
R6 已落地、R2 已接线、R10 判为不可比（键的形状让它不可能发生）。
剩下的都是「慢」「判据」「待议」，没有一条是「缺了会导致模型错」。

| 级别 | 规则 | 缺口 | 计划项 |
|---|---|---|---|
| ~~错~~ ⚪ | R10 | ~~块内成员清理——同一几何可能出现两份~~ **★0904 改判不可比**：core 的段按 ref 建、祖先段与后代段会装同一份几何；我们的行 id 不带根，一个元素最多一行，重复结构上不可能（ADR-060 D1/N1） | T3.2 撤销；改成守住「记录 id 与根无关」这条不变量 |
| ~~错~~ ✅ | R2 | ~~XGEOMETRY 门——显式几何被卷进增量~~ **★0904 已接线**：`xgeometry_gates` 挂在 rollup 的 `OwnershipSnapshot` 上，计划层与预览共用；`model_xgeometry_gate` 默认关，可灰度 | T2.2 完成，待 live 灰度 |
| 慢 | R17–R20 | 祖先抢占去重——多做不是漏做 | T3.3（含 R21 更正） |
| ~~慢~~ ✅ | R28 | ~~目标已消失仍跑一次空生成~~ direct 由 target 根枚举排除，兼容路径由终态复核排除 | 已完成 |
| 判据 | R9/R12/R14 | 名单 vs 位表 | ★0906 `model_significant_over_point` 已接入旧选根路径（`generation_root.rs::significant_point_container_is_its_own_root`，由 `resolve_element_generation_root` 调用）；默认关，开启时 core 判显著的 point 容器自己成根。**判据已换成 `core_significant_bit(noun) == Some(true)`**（W1.2）：快照外的 noun 开关开着也走名单兜底。契约测试已落（`generation_root.rs` tests）：C1-1 `significant_bit_resolves_to_itself_and_to_nearest_significant_owner`、C1-2 `delivery_unit_outranks_the_significant_bit`、C1-3 `unknown_noun_falls_back_to_the_hand_written_list`（含「开关开 + 快照外」一态）、`the_significant_over_point_override_only_moves_aidtex`（44 point noun + 3 `*DATU` × 两形状，两态对称差恰好 = {AIDTEX}）。**direct 文件窗口不看这个开关**：`enumerate_generation_roots_in_subtree` 按 `noun_is_significant` 枚举，`AIDTEX` 直接挂 ZONE 下本来就是一个 Normal 根（`direct_enumeration_already_lists_aidtex_under_zone_as_exactly_one_root`）——P1 的「AIDTEX 零工作项」只在旧 PE 路径成立。R12/R14 的深度上限 32 与 owner 环：W2.4 先量真库再定立不立项 |
| ~~错~~ ~~接线~~ ✅ | R22 | ~~孤儿 mesh 行只能等整库重建~~ **按根对账已实现且比 core 强**；两个生产调用点及 RocksDB testbed 真库删除闭环均已验证 | T3.1 完成 |
| ~~错~~ ✅ | R6 | ~~`Refresh` 不清队列~~ **已落地**（`discard_pending_for_full_rebuild`）。视图切换那一条：三个候选（切 MDB / 收窄 watch scope / 切投影域）的判词见 R6 正文 ★0906 表 | T3.4 已完成；视图切换按开发计划 W2.2 建议登记「无对应物」、不落代码，待拍板 |
| 设计而非缺口 ⚪ | R1 | ★0906 CATA **不进** `UpdateScope::admits`（`update_scope.rs:291` 只认 `DESI \| ISOD` 且须本 MDB 声明；`:330` 测试钉着 `!admits("CATA", …)`；`unrestricted` 点名入口除外）——这是 ADR-058 S3「CATA 永不进持久层」的**设计**，不是待开的闸；`:280` 注释与代码一致、引用 ADR-058 D1。CATA 变更的反向级联按 ADR-058 D2 走**文件侧会话索引差分 → `ref_rev` → `cascade_expand`**，触发源不在 `admits` | 本轮不重开 CATA 入库。D2 那条链今天有没有生产入口，由开发计划 W2.3 核实后回填 R1 注记；若未实施，另开 ADR-058 实施计划，不并进核对表 |

## 10. 2026-09-06 生产链收尾复核

- 预览与执行通过 `build_effective_model_update_plan` 共用规划器分派：direct DESI 均走文件
  `S→T` 窗口规划器，兼容数据模式继续走 PE 规划器。预览不再为 direct DESI 复刻旧 rollup。
- 文件窗口已加入 XGEOMETRY 双端观察：新建看 target、删除看 base、修改必须两端都在
  XGEOM 内才算命中；断链单列 `unreadable`。本轮只输出 warning，不筛工作项、不前移凭证，
  `model_xgeometry_gate` 仍默认关闭。
- R22 的删除对账增加完整性门：有匹配根回执、记录数与来源会话一致、记录祖先范围正确时，
  才能把“本轮投影缺席”解释为删除。无回执的部分投影只允许安全 upsert；带回执的完整空投影
  仍可清除旧行。
- `ModelMemoryStore` 已有 `restore_projection_on` 回灌入口，旧文中“没有从库回灌路径”的描述失效。
  嵌套根仍依赖完整祖先链与整棵子树生成；本轮不实施 R17–R20 的跨根任务压缩。窗口 warning
  记录 `nested_roots` 与 `duplicate_coverage_elements`，执行耗时沿用模型 drain 的
  `last_root_duration_ms`，先取得重复成本基线，避免压缩时遗漏后代根的凭证、manifest 和发布结算。
- R28 在 direct 窗口路径由 `roots_T` 枚举提供目标存活证明；兼容路径保留既有终态存活复核，
  不新增第二次文件查询。

### 10.1 真实文件验收（2026-09-06）

- ams8000 九个钉死窗口全部通过：嵌套显著根修改、EQUI 位姿、BOX 改挂、EQUI 子树删除，
  以及 BRAN 新建、整根移动、成员移动、成员删除、整根删除。
- ams7999 45→46 通过：direct 文件窗口可规划新建 STRU 根，不触发旧读取器的终态复核故障。
- 录制 E3D 窗口矩阵通过；ams8000 255→256 同时验证 `nested_roots >= 2` 且
  `duplicate_coverage_elements > 0`，观测指标在真实嵌套根结构上有信号。
- 8019 RocksDB testbed 的独立临时数据库通过 R22/R23 闭环：无回执部分投影删除数为零；
  匹配回执完整投影恰好回收一条缺席 primitive，被删来源在模型表和 `geo_relate` 中均无残留，
  保留投影仍在；测试结束自动删除临时数据库。
