# core.dll API 对标矩阵：读侧 API 面 ↔ e3d-io/gen-model direct 实现

> 状态：**S1 已落地**（2026-08-30 21:4x，会话 DR6W；盘点=AJXG）。用户 19:38 拍板
> S1 放 **e3d-io crate**，已落 `vendor/e3d-io/src/db_element.rs`（`DbSet`+`DbElement`+
> `MemberCursor`+`DbFileResolver`）+ `tests/db_element_facade.rs`（真语料 6 测：
> 身份/导航原序/typed getter 逐属性对齐抽取/名字往返/跨库 resolver 跳转/错模板拒开；
> lib 174 绿）。落点的具体化见 §5。
> 用户指令：「我需要的是对标 core.dll 里的 api 函数」——把 direct 直读的查询面按 core.dll
> 的 API 函数逐一对标，而不是自造查询接口。
> 与既有计划的关系：本矩阵是 `.planning/2026-08-30-direct-read-model-generation` **Phase 2
> 的具体化**（Phase 2 七项全部落在本矩阵的某一行里）；API 面清单沿用
> `docs/plans/direct-dbelement-read-api.md` 的 RE 存量（那份计划基于 pdms-io-v2 栈，
> 本矩阵把同一 API 面**重新对标到 e3d-io 栈**——用户已拍板不对接 pdms-io）。

## 1. 口径：对标什么、不对标什么

- **读侧 only**。写侧（`Set*`/`Create*`/`Delete`/`Copy*`/`Claim`/`SaveWork`/`ChangeType`/
  `DatalProtection`）与租约并发一律 Non-Goal，与 direct-dbelement 计划一致。
- **三层 API 面**，对标以①为主、②为锚：
  1. **`Aveva.Core.Database` 公开面**（.NET 反射转储，`.ida_scratch/e3d_dbelem_api.txt`
     35K / `e3d_mdb_api.txt` / `e3d_netapi{,2}.txt`）——core.dll 之上的官方读 API，
     函数语义即用户可观测行为，是对标的**行为规格**。
  2. **core.dll 内部 C++ 函数**（生成链取数点）——已有锚点见 §2；全量清单由并行会话
     GL3D（ida-bridge core.dll 全流程分析）产出后回填本表「C++ 锚点」列。
  3. **Core3D 消费面**——已证 Core3D 只吃 core.dll 解析好的数值（查 getReal/evaluate=空，
     见《会话-2026-08-30-恢复cursor会话-e3dio直读协作.md》），不构成独立对标面。

## 2. 已坐实的锚点（指令级证据）——**分两个映像轴，勿并列**

> ⚠️ 关键更正（GL3D 2026-08-30 用 `_allroutines.json`/`out_elemfuncs.json`/`_imports_core3d-retrace.json` 坐实）：
> 涉及**三个模块**，地址不可并成一列比较——
> - **core.dll（`0x5xxxxxx`）**：dabacon + 全 FORTRAN 数据库库（getattlib/exprlib/pplib/catdblib…）
>   + `DB_Attribute`/`DB_Element` C++ 真身。**这是读 API 的落点**。
> - **模型生成/interop 模块（`0x10xxxxxx`）**：`build/ELMODL` 等生成 FORTRAN + `DBE_Base`/`BasLinkedBase`
>   C++ + `0x100c66xx` 的 `getAtt` **jmp 桩**（跳回 core.dll）。该模块 **import core.dll(4859 处)+libgm+libfl**，
>   是独立模块；物理模块名待活桥段表定论（早前称 Core3D，N5OY 记的「core.dll.i64」大概率是这颗 idb 的命名）。
> - **libgm（几何内核）**：`gm_CreateCombination`(CSG 布尔)/`gm_CreateFacetStructure`(网格化)/`gm_AddMember`/
>   `gm_CreateTransform`。`GM*`(GMCFST/GMDRAW/GTGEOM) 是它的 FORTRAN 门面。**属生成算法层，不是数据读 API**
>   → 见 §3 末「几何内核边界」，本读侧矩阵不覆盖。
>
> 对标矩阵的行为规格对齐 .NET 面与 **core.dll 0x5 的 FORTRAN getter**；`0x10` 只是「谁在调」，libgm 是「几何怎么算」。

### 2a. 模型生成/interop 模块（`0x10xxxxxx`，「谁调用读 API」；import core.dll+libgm）

| FORTRAN/C++ | 地址 | 语义 | 来源 |
|---|---|---|---|
| `add/ADDDES` | `0x1021d005` | 设计库子树 DFS 遍历入口（DBEFOR→DFIND→DRETUR） | GL3D §1 |
| `build/MODCMP` | `0x10251012` | 单元素按 noun 分类挑建模方式（I*COM 谓词 switch） | GL3D §1 |
| `build/ELMODL` | `0x1025277e` | 元素建模主循环（DSAVE/DRESTO 指针栈遍历子树） | teach/0009 |
| `build/NXTITM` | `0x102547f5` | 成员游标：`*a2==0` 取首、`-1` 结束，**不物化列表** | teach/0009 |
| `build/SGDRAW`→`GMDRAW` | `0x102556de`→`0x10254b01` | 逐类分派→逐元素几何（GMDRAW=libgm 门面） | teach/0009 |
| `cachegml/GTGEOM` | `0x10341d2e` | 取几何枢纽（目录 GML→libgm `gm_Create*`） | GL3D §1 |
| `DBE_Base::evaluate` 家族 | `0x108e966c`~ | 表达式求值 C++ OO 包装（多半 delegate 到 core.dll `exprlib/EXEV*`，待活桥证） | N5OY / GL3D |
| `CSG_BaseCSGTree::getBodyPlan/SymbolicRepresentation` | `0x100c672a/6730` | CSG 实体/符号表示（负几何/布尔） | GL3D §5 |

### 2b. core.dll 数据库读侧（`0x5xxxxxx`，**对标落点**）

| FORTRAN 库/C++ | 地址区 | 语义 | ↔ 矩阵组 |
|---|---|---|---|
| `getattlib/GAT*`（GATRF/GATLOG/GATRE/GATIN/GATSTR/GATPOS/GATDIR/GATOR/GATTYP/GATID/GATDAT） | `0x5d218a8+` | typed 属性 getter 全家族 | **C**（typed getter） |
| `getattlib_all/glb/sys/tra`（GAL*/GLB*/SYS*/TRA*） | `0x5d20f30+` | 分范畴（全/全局/系统/草图）取属性 | C / H（显式隐式面） |
| `exprlib/EXEV*`（EXEVST/LG/F/PE/RA/PO/DR/OR/AX/RU/RD） | `0x5d20070+` | **参数表达式求值** | **F**（Evaluate\*） |
| `exppdms/EX*`（EXINT/REAL/LOGL/TEXT/POS/DIR/ORI/REF）+ `GATPAR/GTGETN` | `0x5d1f820+` | PDMS 表达式取值 + 目录参数 | F / D |
| `pplib/CGTCT2·CSTCAT·CGTCPR·CATFGT` | `0x5d1fe98+` | 目录(CATA)分类/参数读 | **D**（引用跳转，cata 链） |
| `catdblib/GTGINS·GATINS·GTSINS` | `0x5d1ff34+` | 目录几何集实例读取（GTGEOM 原料） | D / F |
| `ppointlib/EXPPOI·EXPLNE·PPEVST` | `0x5d223e0+` | P-point/连接点求值 | D（管件 arrive/leave） |
| `dbreflib/EQREF·NULREF·NULIFY·UTRFAS` | `0x5d1dcdc+` | ref 工具（判空/规范/转换） | A（RefNo）/ D |
| `idlib/IDQFOR·IDQNXT·IDSNXT·FORWAR·BACKWA` | `0x5d21f30+` | 树导航（首/下一/前/后） | **B**（FirstMember/Next/Members） |
| `DB_Attribute::category/catparam/allowedRanges/allowedValues/allowedUDAValues` | `0x58ce090+` | 属性元数据（范围/枚举/UDA） | H（schema/AtDefault）|
| `DB_IndexTableCompare`（← `DB_DB::elementsChanged/Deleted/InsertedBetween`） | — | 双根 B+ 树归并差分；删除判据=键集差，无墓碑 | **G**（差分） |
| `ATNLOG sub_55BC98B` | `0x55bc98b` | 属性槽解码 `value = page[base+atgtdf_index]` | C（e3d-io 已复刻） |
| `sub_5AF6840` | `0x5af6840` | 库控制块→表根（主索引 +12/+16、数据表 +20/+24） | I（Db 层） |
| dabacon opcode 266/270 | — | 游标 begin/advance（有序枚举） | B / G |

> **DBE_Base::evaluate 归属已澄清**（GL3D 2026-08-30 用 `_imports_core3d-retrace.json`）：
> `0x108e966c` 落在 **`0x10` 模型生成/interop 模块**（与 `ELMODL`/`BasLinkedBase` 同簇），
> **不在** core.dll 的 `DB_Attribute` 簇（0x58ce）。推断分工（待活桥 `decompile 0x108e966c` 证）：
> core.dll `exprlib/EXEV*`（0x5，FORTRAN）是**底层 PDMS 表达式求值器**；`DBE_Base::evaluate`（0x10，C++）
> 是 .NET `Evaluate*` 的 **OO 包装，内部 delegate 到 EXEV***。→ **G4 修法先押修法 A（字符串方言对齐
> EXEV*）**，与既有 Phase 3 一致；仅当反编译显示 DBE 自带独立结构化逻辑（不调 EXEV*）才升修法 B。

## 3. 对标矩阵

图例：✅ 已对标（语义等价、有验证）｜🔶 数据/机制在但**门面缺**或语义有差｜❌ 缺｜🚫 记账不做。
「我方」列：`E` = e3d-io（`vendor/e3d-io`）、`G` = gen-model direct 层（`src/data_interface/`）。
每组标注 **core.dll 原生落点**（0x5 FORTRAN 库，§2b），即该组 .NET API 的原生实现位置。

### A. 身份与元信息

**core.dll 原生落点**：`getattlib/GATTYP`（类型码）、`GATID`（id）；`dbreflib/NULREF·EQREF`（ref 判空/判等）；`DB_Attribute::category`（0x58ce090+，属性元数据）。

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `RefNo()` / `Ref` / `DbNo()` | E `RefNo`(2×u32) + G `DirectStore::dbnum_of`（ref0 定位器） | ✅ |
| `ElementType` / `GetActualType()` | E `extraction.noun_hash` + attlib 反哈希 `noun_name` | ✅ |
| `get_Db()` | G `DirectStore` 池按 dbnum 取会话 | ✅ |
| `IsValid` / `IsNull` | `find_element` 返回 `Option`（None=不存在） | 🔶 语义等价但无句柄门面 |
| `IsDeleted` | 无墓碑位（已证），单元素删除=活树键集不含它；跨时点判删走 index diff | 🔶 语义差需写进文档与测试 |
| `IsDescendant(DbElement)` | owner 链上溯可实现（owner 在记录头，单库实测不跨库） | ❌ 门面缺 |

### B. 导航（对齐 `NXTITM` 游标语义：不物化列表）

**core.dll 原生落点**：`idlib/IDQNXT·IDSNXT·FORWAR·BACKWA`（0x5d21f30+，树导航）；`dbreflib/EQREF·NULREF`（0x5d1dcdc+，ref 判等/判空）。Core3D 侧调用者 = `add/ADDDES` 的 `DBEFOR→DFIND→DRETUR` + `build/NXTITM`。

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `Owner` | E 记录头 `owner: RefNo`（extraction 已带） | 🔶 数据在、门面缺 |
| `Members()` / `Members(type)` | E `ParsedElement.members` **原序**（G3 约束：不 sort 不 dedup） | 🔶 数据在、门面缺 |
| `FirstMember/LastMember/Member(i)/Next()/Previous()` | 无游标门面；members 向量在手，游标可薄封装（NXTITM 语义） | ❌ |
| `Db.World` / `WorldMembers()` / `MDB.GetFirstWorld` | E noun_catalog 有 `is_world`；WORLD owner==0（tty.rs 已注）；无门面 | ❌ 需定 world 定位法（扫 noun 或索引首键，落地时核实） |

### C. 单属性 typed getter（`GetString/GetAsString/GetInteger/GetDouble/GetBool/GetDate`、`Get*Array`、`GetPosition/GetOrientation/GetDirection` + qualifier/下标重载）

**core.dll 原生落点**：`getattlib/GAT*`（0x5d218a8+，逐 typed getter：GATRF=ref、GATIN=int、GATRE=real、GATSTR=串、GATPOS/GATDIR/GATOR=位姿、GATTYP=类型）；ELMODL 里 `ATNINT/ATNLOG/GATSTR` 落到这里。属性槽解码 `ATNLOG sub_55BC98B`（e3d-io 已复刻）。

| 能力 | 我方现状 | 判定 |
|---|---|---|
| 全量属性一次转换 | G `direct_attmap::to_named_attmap`（形状权威=DB schema `default_val`，8000/7333 对拍收口） | ✅ |
| 单属性解析 | E `resolve_attribute`（engine.rs，含 SYNO 链） | 🔶 在 e3d-io 层，未接到 G |
| typed getter 门面 | ❌ 无——消费方现在只能拿整张 `NamedAttrMap` 自己挑 | ❌ |
| qualifier / 数组下标重载 | `direct_attmap` **无 qualifier 处理**（rg 零命中）；DB 栈 `whole_attmap` 有 qualifier 语义 | ❌ 缺口，UDA/qualifier 属性面需专项 |
| `GetAsString`（带单位格式化） | 角度已按 ANGL 定标（BANG 修复）；通用单位格式化未做 | 🔶 |

### D. 引用属性跳转（跨库）

**core.dll 原生落点**：`pplib/CGTCT2·CSTCAT·CGTCPR`（0x5d1fe98+，目录分类/参数）；`catdblib/GTGINS·GATINS`（0x5d1ff34+，目录几何实例）；`getattlib/GATRF`（ref 型属性读）。

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `GetElement(attr)` / `GetElementArray(attr)` | G `NamedAttrValue` Refno 变体 + `DirectStore::attrs(refno)` 自动跨库（locator pin，实测 92 跳 5052 库） | 🔶 机制全在、门面缺 |
| 存量引用链 1–3 跳（CATR/SPRE/PRTREF→SCOM/SPRF/SFIT/JOIN） | Phase 2 清单第 2 项（`get_cat_refno` 直读版） | ❌ 本矩阵 S1 的验收用例 |

### E. 名字查找

**core.dll 原生落点**：dabacon 主 refno 索引（**无独立 name→refno 表**；名字属性走 `getattlib/GATSTR` 读 NAME）。我方 `direct_index` 的 name 索引是**自建加速结构**，非文件原生——这正是 ADR-053 里被标「部分」的那格。

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `Db.FindElement(name)` / `ElementExists` | G `direct_index::find_named`（NAME 原文→refnos，重名不折叠；指纹缓存 65ms/1.1ms） | 🔶 索引在、句柄门面缺；名字规范化（`/` 前缀）需对齐 DB 侧 |
| `MDB.FindElement(name)`（跨库聚合） | 逐库 indexes 聚合可实现 | ❌ 薄封装 |

### F. 表达式求值（`GetExpression` / `Evaluate*` 全家族）

**core.dll 原生落点**：`exprlib/EXEV*`（0x5d20070+，参数表达式求值）+ `exppdms/EX*·GATPAR`（0x5d1f820+）+ `ppointlib/EXPPOI·EXPLNE`（0x5d223e0+，P 点/连接点）；C++ `DBE_Base::evaluate` 家族的归属与内部展开待活桥核实（见 §2 脚注）。

| 能力 | 我方现状 | 判定 |
|---|---|---|
| `Evaluate{Double,Bool,String,Position,Orientation,Direction,Element,...}` | ❌ G4 未解——原生落点 = core.dll `exprlib/EXEV*`（0x5 FORTRAN），我方现有 resolve+tiny_expr 吃 pdms-io 字符串方言 | ❌ **Phase 3 原样，先押修法 A**（对拍先行→字符串对齐 EXEV*→分歧大再 B 结构化 DBE）；GL3D 已定 DBE@0x10 多半 delegate 到 EXEV*，支持先做 A |
| `GetExpression(attr)` | e3d-io 有渲染器但分派器私有、只出显示文本 | 🔶 Phase 3 3b 公开 `rendered_by_shape` |

### G. 会话与差分

**core.dll 原生落点**：`DB_IndexTableCompare`（← `DB_DB::elementsChanged/Deleted/InsertedBetween`），双根 B+ 树归并，删除判据=键集差、无墓碑。

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `Db.ElementsChanged/Deleted/InsertedSince/Between` | E `index::diff`（t-327：428 对相邻会话与全量枚举集合差逐键相等，成本门精确式） | ✅ 机制、🔶 门面（差分结果→句柄数组的薄封装） |
| `HasElementChangedSince/Between` | diff 结果单键查询 | 🔶 薄封装 |
| `AttributesChangedSince`（属性级） | `open_at` 两时点各读 attmap 对比可实现 | ❌ 门面缺（增量链暂不消费，记账） |
| `MDB.ChangesSinceSessions`（跨库聚合） | 逐库 diff 聚合 | ❌ 记账，增量主链仍走旧栈（ADR-031/036） |

### H. schema 与默认值

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `AtDefault(attr)` | 文件只存非默认值：extraction 区分「文件真存值」vs 默认（direct_index 出边已用此语义） | 🔶 语义在、门面缺 |
| `IsAttributeValid(attr)` | DB schema `named_attr_info_map[noun]` 键集判定 | 🔶 |
| `GetAttributes()` | `named_attmap` 键集 | ✅ |
| `GetAttributeAllowedValues/Ranges` | attlib 有 ATGTDF 定义域信息（部分） | 🚫 生成链不消费，记账 |
| UDA（`:UDA` 属性、`UdaCatalog::read`） | E `uda_catalog.rs` 在；**sesno pin 残留**（Phase 2 第 6 项：带 sesno 重载） | 🔶 |

### I. MDB / Db 层

| .NET API | 我方现状 | 判定 |
|---|---|---|
| `MDB.GetDBArray()/GetDB(n)` | G `DirectStore` 池 + `CataDbLocator`（dbnum→文件路径） | 🔶 机制在；「MDB 成员清单」现靠 DbOption/水位表，纯文件侧（SYS 库 `MDB.CURD`）未做 |
| `Db.Number/Type/ExtractNumber` | E 文件头 `DbDescriptor` | ✅（Number 已用；Type/Extract 落地时核实字段） |
| `Db.Name` | 文件头不解析库名（rg 零命中）；现由定位器/DbOption 提供 | 🔶 记账 |
| `Db.DbItem`（库在 SYS 库里的元素） | ❌ | 🚫 第一轮不做 |

### J. 明确不做（记账）

写侧全家族、`Claim/Release/SaveWork/Refresh`、`GetRule/VerifyRule/RuleMasters`（规则系统，
生成链不消费）、`GetAttributeBlobSegment`、`IsValidNameFormat/Parse`（命令行工具面）、
`BindingElement/GetBoundElement`（Schematics 绑定）、`DatalProtection`。
任何一格若日后被生成链证实要消费，翻开重判，不留死账。

### 几何内核边界（libgm，**本读侧矩阵不覆盖**）

GL3D 坐实几何真正的算子在独立模块 **libgm**：`gm_CreateCombination(GM_Operation)`=CSG 布尔、
`gm_CreateFacetStructure`=网格化、`gm_AddMember`/`gm_CreateTransform`。调用链
`catdblib/GTGINS`(core.dll 0x5 取目录几何实例) → `cachegml/GTGEOM`(0x10 枢纽) → **libgm `gm_Create*`**。
这属于**生成算法层**（数据读到之后怎么算几何），不是「读数据 API」，因此不进本矩阵——
本矩阵的边界严格是 DbElement/MDB/Db 的**读侧数据接口**。libgm 的对标归属：
- gen-model 已复刻：`manifold_bool.rs`/`manifold_csg.rs`（CSG）、`libgm_discretise.rs`/`sweep_mesh.rs`（facet）；
- 存量逆向资产：根目录 `.scratch_libgm210.json` / `.scratch_libgm31.json`；
- 几何产物对拍归 `.planning/...direct-read-model-generation` 的 **Phase 4/5**（direct vs DB 逐字段比 CSG 参数/顶点），不在 Phase 2。

## 4. 缺口汇总 → 实施顺序（建议）

矩阵里所有 ❌/🔶 归成四步，S1 是 Phase 2 的载体：

- **S1 · `DbElement` 句柄门面**（新文件，不碰他人 M 文件）：
  `(refno, dbnum, Arc<DirectStore>)` 惰性句柄；身份（A 全部）+ 导航游标（B：owner/members
  原序/first/next/member(i)，NXTITM 语义不物化）+ typed getters（C：over `named_attmap`
  缓存，逐 NamedAttrValue 变体投影）+ 引用跳转（D：`get_element(attr)` 走池自动跨库）
  + 名字（E：`find_element` 接 direct_index）。
  验收 = Phase 2 前 4 项改写成 DbElement 用法后 direct/DB 双跑对拍（键集+**序列**）；
  `get_cat_refno` 1–3 跳、`get_world_transform` owner 链折叠作为门面之上的收口函数直读版。
- **S2 · 差分门面**（G 组薄封装 t-327 diff：`elements_changed_between` 等 + 单键 Has*）。
- **S3 · 表达式**（F 组 = 既有 Phase 3，不动顺序：差分先行，禁猜方言）。
- **S4 · MDB 完整化**（I 组：SYS 库 CURD 成员清单、World 定位、Db.Name；含 UDA sesno pin 残留）。

## 5. 决策点（已拍板，2026-08-30 19:38）

1. **门面落点 = e3d-io crate**（用户否了 gen-model 推荐项）。落点的具体化，
   把「schema/locator 倒灌」的代价这样消化（DR6W 实施）：
   - **schema 不搬家**：门面只用文件侧原生 schema（attlib.dat + `*vir.dat`），typed
     getter 按 `DescriptorValue`（文件自报形状）投影；DB 侧 `NamedAttrMap` 定形仍是
     gen-model `direct_attmap` 的职责——一个形状两处权威必然分叉，所以 e3d-io 不复制。
   - **跨库不倒灌 locator**：`RefNo::dbno()`（core.dll `sub_5AEB6B0` 位布局）自解库号，
     `DbSet` 池按库号找已开库；未注册 fail loud，可选 `DbFileResolver` trait 让
     gen-model 拿 `CataDbLocator` 注入按需补开（= `pin_from_locator` 同款语义）。
   - **pin 语义对齐**：`sesno: Some(n)`=open_at / `None`=开库冻最新；文件身份守卫
     （FileReplaced）是工程语义，留在 DirectStore 不下沉。
2. **开工时机 = 即开**：C++ 锚点列不阻塞行为对标，GL3D 清单出来后继续回填本表。

### S1 已落地清单（e3d-io，全部新文件不碰他人 M 文件）

- `src/db_element.rs`：A 身份（refno/db_no/exists/element_type/stored_name/
  is_null/is_descendant_of）+ B 导航（owner/members 原序游标/member(i) 1 起数/
  first/last/next/previous/members_of_type，NXTITM 语义）+ C typed getter
  （attribute/get_string/integer/double/bool/ref/各数组/position/direction/
  orientation，异形报 `TypeMismatch` 不静默、未知属性报 `UnknownAttribute` 不给 None、
  Unset 归一 None）+ D 跨库跳转（get_element/get_element_array 走池+resolver）+
  E 名字（`DbSet::find_named` = Named 档全树扫，原生语义无 name 表；加速留给消费方索引）。
- `src/lib.rs`：+`pub mod db_element` 与 re-export（两行，lib.rs 本干净无他人改动）。
- `tests/db_element_facade.rs`：期望值全部从引擎原语现场推导不写死 refno；
  跨库用例 = ams8000 引用属性跳 ams5052（resolver 自动补开，实测断言池内 5052 出现）。
- 待接：Phase 2 的 `get_cat_refno`/`get_world_transform` 等收口函数改写成门面用法
  + direct/DB 双跑对拍（键集+序列）——gen-model 侧，下一步。

## 6. 证据与引用

- `.ida_scratch/e3d_dbelem_api.txt`（435 行 DbElement 反射面，2026-07-26 转储）、
  `e3d_mdb_api.txt`（MDB/Db/DbType）、`e3d_netapi{,2}.txt`
- `teach/learning-records/0009-batch-model-generation-pipeline.md`（ELMODL/NXTITM 调用树）
- `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（差分/删除判据）
- `docs/plans/direct-dbelement-read-api.md`（pdms-io-v2 版蓝图，API 面清单的前身）
- `docs/plans/2026-08-30-e3d-io-gen-model-gap.md`（G1–G13）、
  `.planning/2026-08-30-direct-read-model-generation/task_plan.md`（Phase 2 清单）
- 我方现状代码：`vendor/e3d-io/src/engine.rs`（open_at/find_element/resolve_attribute/
  scan_elements）、`src/data_interface/direct_store.rs`/`direct_attmap.rs`/`direct_index.rs`
