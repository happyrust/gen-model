# 重构计划：以 RefNo 原生路由取代全 MDB 元素骨架

- 日期：2026-09-12
- 状态：Plannotator 已批准；阶段 0 本地基线已实施，退出门「位解码 corpus 零分歧」已在本机全部 E3D 2.1/3.1
  语料（1,947 库 / 38,891,013 元素）上实测通过；阶段 1–2 + 3A 由执行切片
  `docs/plans/2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` 承接——**W1–W7 已提交**
  （`bb16105d2` W1+W2 / `5cc5e3e04` W3 / `425ca98d8` W4–W6+W8 / `7f501ab0d` W7），即阶段 1（路由目录）、
  阶段 2A（`dbnum_of_root`）、2B（`DirectStore`）、3A（属性服务）**已落地**，3B 只换了树服务的定位器
  （`ensure()` 仍在，WORL 枚举与 noun / owner 查询留给阶段 4 切片）；AMS `/ALL` 端到端（初始化 → plant-ui 显示模型）
  与 ams8000 探针对账（1313/1313）已验，见切片进度段；core.dll 取证 `docs/evidence/2026-09-12-ida-core-dabacon-refno-routing.md`；
  §10 五项拍板已定（见 §10）；JEU 现场补采不可在本机完成（调试 / 验收项目为 AMS，见 §7.4）
- 目标工程：`gen-model-refactor`
- 目标版本：0.1.23 后续构建
- 触发现场：JEU `/ALL` 含 479 条 CURD，其中 417 个 DESI/ISOD 文件；当前启动头扫描
  1,221,858 个元素约耗时 68 秒。

## 1. 结论

跨库属性、owner、noun 与模型根定位不需要先扫描 MDB 的全部元素。

有效 E3D RefNo 自带 dbnum。正确路由是：

1. 从 RefNo 的 `word0` 解出 dbnum；
2. 在当前 MDB/依赖上下文的 `dbnum -> DbFilePin` 目录中选择文件；
3. 校验文件头自报的 dbnum/type；
4. 以明确会话打开单库；
5. 用该库 B+tree 按 RefNo 点查元素。

全 MDB 元素骨架可以作为显式全局查询的可选缓存，但不得是服务启动、健康检查、单元素属性读取
或单根模型生成的前置条件。

## 2. 已验证证据

### 2.1 E3D 2.1 `core.dll`

IDA Bridge 只读查询确认：

- `DB_Element::dbno` `0x1049e870` 调用 `db_get_dbno_from_reference`。
- 后者最终进入 `sub_1064EF60`，核心表达式为：

  ```text
  dbno = (word0 & 0x1FFF) | ((word0 >> 13) & 0x3E000)
  ```

- `DB_Element::getDB` `0x1049e8c0` 执行“解 dbno -> `DB_DB::findDB(dbno)`”。
- `DB_DB::findDB(int)` `0x10475390` 查询进程内 `DBLookup_` 平衡树，不扫描元素。
- `DB_MDB::internalOpenDB` `0x10559680` 从 MDB 数据库定义读取 `DBNO/ISTYP`，构造并打开单库。
- `DB_Element::dabGetAtt` `0x104a0790` 经 `elGotoCPP` 按目标 RefNo 定位，再读取目标属性。
- `DB_DB::getWorld` `0x1047ce00` 与 `getWorldMembers` `0x1047ceb0` 提供数据库根和成员导航，
  不要求先建立全 MDB 的 noun/owner 表。

#### 2.1.1 证据附录：MDB 定义如何决定“开哪些库”（2026-09-12 反编 `internalOpenDB` 链）

来源：`D:\ida_scratch\plant210\core.dll.i64`（E3D 2.1），IDA Bridge 只读 SQL/decompile。
hash 常量用仓库同款 `db1_dehash` 解码（`STAMP`/`ALL`/`INVALD` 均可读，自证）。

**结论**：MDB 定义只给候选清单（`CURD`，DBSET 会展开）。真正“开哪些库”在
`DB_MDB::openMDB -> internalOpenDB` 里再过三道闸：当前模块的 DB 类型模式表、模块 × ISTYP
硬编码规则、库文件版本号。R/RW 由用户 TEAM 归属（`GETMOD`）+ 模块表 + 项目/Global/
MULTIWRITE/批处理/stamp 决定。**整条链没有任何 RefNo/元素级信息参与**，路由目录只需
`dbnum -> 文件`。

**A. 从 MDB 元素到会话清单（选 MDB 时）**

- `DB_MDBSetUp::set` `0x1055bcb0`：`DB_DB::findDB(254)` openShare 打开 COMMS 库，在用户 comms
  元素上写 `MDBF`（MDB ref）/`MSTAMP`/`NOSAVE`，然后 `update`。
- `DB_MDBSetUp::update` `0x1055b5f0`：读 MDB 伪属性 `CURRFU`
  （`DB_PseudoCURRFULLonMDB::get` `0x105C06B0`：遍历 `CURD`，noun 为 DB 直接收，否则视为 DBSET
  经 `DBSLIS` `0x105C03B0` 递归展开）；>5000 条报错 39/38。每个 DB 调 `sysdblib/GETMOD`
  `0x105D4AA0` 算模式 int；`writeBack` `0x1055b280` 把 `UCURD`（DB 列表）与 `DBMO`
  （1=R / 2=RW）写回 comms 元素并 `saveWork`。
- `GETMOD`：DGOTO 失败 -> 0；默认 1(R)；DB 非外来（`ISDBFR` = `DBPROJ` 项目 ≠ 当前项目）->
  CLIMB 到 TEAM 取 `REF`，`GOUSER` 到当前 USER 读 `SECU`（GENERAL=1 / FREE=2）作模式，再在
  USER 的 `TMLI` 里找 `TEMF == team`，找到 -> 2(RW)。外来项目的库恒 R。
- `DBSLIS` 的展开体 `sub_105C0010`：以逻辑树迭代 DBSET 成员——`DBSTL` 取 `DBSTF` 递归进子 DBSET
  （已访问集合防环），其余成员读 `DBF`（SYS 模板里带 `DBF` 的成员 noun 是 **`DBL`**；`DBLI` 是
  TEAM 下带 `UDATAB/UDAFTB/UDASTB` 的另一种元素，与 DBSET 无关）；库、集合各有一个已访问集合去重。
  入口先跑 `sub_105BD020`：DBSET 的 `DBCRITERIA`（整型词数组编码的表达式）非空时，对
  `DB_System::getAllDbs` 全部库求值 `DB_Expression::result`，命中的也并入。
- 旁证：`DEFERD`(MDB) = `DBLS` − `CURD`（`0x105B9BD0`）；`DBLS` 是存储属性，空则回退 `MEMB`
  （`0x105B97A0`）。DEFER/CURRENT 命令改的是 comms 的 `UCURD/DBMO`
  （`deferDB` `0x1055b3d0` / `currentDB` `0x1055b8f0`），不改 MDB 本体。

**B. `DB_MDB::openMDB(readOnly, checkWorld)` `0x1055a430`**

1. `internalCloseMDB`；`this+649 = readOnly`；类型表为空则 `getTypeMode`。
2. 从 comms 元素读 `UCURD`、`DBMO`、`MSTAMP`、`NOSAVE`（comms 元素缺失 -> 错 43/16）。
3. `DB_System::replaceWithWorkingDBs(user, UCURD)` `0x10564a00`：用户 working DB（extract）若
   `EXTOWN` 等于清单里某库，就地替换成该 extract。
4. 逐条 `internalOpenDB(dbEle, stampEle, CCOPIE[i], DBMO[i], NOSAVE)`；任一返回 false ->
   整个 MDB 关闭、失败。
5. `internalOpenProductDBs` `0x10556030`：`DB_Project::productDBs` 全部 `openRead` + `addToList`。
6. `openTransDB` `0x1055a370`：仅 Global 项目（`glbdblib/ISGLOB`），`currentTransDB` 以 RW 开。
7. `GFWORL(defaultType)`：默认类型库必须有 WORLD，否则错 41/56（checkWorld 时）。

**C. `DB_MDB::internalOpenDB` `0x10559680` 单库判定顺序**

- a. `ISTYP = getInt(dbEle, ATT_ISTYP)`。≤0 -> 错 43/70，返回 false（整 MDB 失败）；
  >20 -> 返回 true（静默不开）。
- b. **闸 1 模块类型表**：`typeMode[ISTYP-1]` 不是 `"RW"` 也不是 `"R"` -> 返回 true，不构造不开。
  表来自 `getTypeMode` `0x10559240`：`DB_System::getModule()` 当前 MODU 元素 -> 成员 CMMO 的
  `TYNU`(1..20) -> `MODE` 字串；`MODE == "Default"` 的 TYNU 记为默认类型 `this+616`；
  20 项初值全 `"N"`。
- c. `DB_DB::construct(dbEle)` `0x10480960`：按 `DBNO` 查 `DBLookup_`，已有且 DB 元素 ref 一致则
  复用；否则 new + `setFromDbele` `0x1047b5a0`，读 `DBNO / CLAI / ISTYP / EXTNO(0->1) / EXTOWN /
  LCTROL / LVAR / ISDBPR / DACC / NAMN / FINO / ISDBFR / AREA / LREFO / LCONSU / LSVHIS / PROJ`，
  `constructFileName(AREA, FINO)` 得文件名；DBNO=0 且 noun LOC -> 0x1FFF。
- d. stamp：`sysdblib/STMPDE` `0x105D58D8`：stamp 元素 TYPE 必须是 STAMP，遍历成员，
  `STLSF` -> DB 的 `DBNO` 与 `EXTNO` 都匹配 -> `STSESS` 即打开会话号 v57；`db+761 = (v57 != 0)`。
- e. 基准模式由 `DBMO[i]`：1 -> `"R"`；2 -> `typeMode == "RW" ? "RW" : "R"`；其它 -> `"N"`。
- f. **闸 2 模块 × 类型硬编码**（`UQMODN` = `AVEVA_ModuleManager` 当前模块号）：
  - 1 DESI / 13 ALYS：模块 97/98/99（SMM/Diagrams/Tags）读 DB `LREFO`：false 时非集成模式 ->
    43/540 -> `"N"`，集成模式 -> 许可检查 `licence.checkDB(db, &this+0x2A0, mdb)`（见 C.i'）通过 ->
    `"R"` 否则 43/541 -> `"N"`；true 且请求 RW -> `"RW"`。其它模块：84 Dars / 68 Lexicon /
    1 Admin / 81 Paragon / 17 Specon -> 强制 `"R"`；0 Monitor -> `"N"`；其余（含 78 Design）保持基准。
  - 2 CATA / 4 PROP / 17 XPRD：Monitor -> `"N"`，其余基准。
  - 6 ISOD / 7 PADD / 15 MANU：97/98/99：集成 -> `"R"`，否则 43/540 -> `"N"`；其它模块同 DESI
    的“其它模块”规则。
  - 9 ENGI：99 Tags -> 基准；非集成 -> 43/540 -> `"N"`；97 SMM -> `"R"`；其它非 0 模块 ->
    许可检查 -> `"R"` / 43/543 -> `"N"`；Monitor -> `"N"`。
  - 14 SCHE：98/97 -> 基准；集成：99 -> `"R"`，其它非 0 -> 许可检查 -> `"R"` / 43/542 ->
    `"N"`，Monitor -> `"N"`；非集成 -> 43/540 -> `"N"`。
  - 其余 3 COMM 5 MISC 8 DICT 10 SYST 11 GLOB 12 TRAN 16 NSEQ 18 CONF 19 TASK 20 RDFN：保持基准。
- g. 收尾降级：RW 且（MDB readOnly 或 stamp v57 ≠ 0）-> `"R"`；RW 且 `DACC == 2`(MULTIWRITE) ->
  `"R"` 并置 `db+868 = 1`（走 claim）；RW 且批处理 `UQBATC` 且非 CCOPIE 副本 -> `"R"`；RW 且
  Global 项目且 `ISPRIM` 为 false -> `"R"`。
- h. 实开：`"RW"` -> `openNosave`/`openWrite`；`"R"` -> `openRead(db, v57)`；`"N"` -> 不开、标丢弃；
  其它字串 -> close+delete 返回 false。开失败：`isErrorAcceptable` 否 -> 记错返回 false
  （整 MDB 失败）；是 -> 报错并丢弃该库。
- i. `LREFO` 为真且 `DB_Licence::refDesiTest(db, mdb)`（`sub_101B4550` -> `0x101B8570`，名字来自
  trace 串）不通过 -> 43/544 -> 丢弃。`refDesiTest` 的判定：`db` 非空、`ISTYP == 1`（DESI）、DB 元素
  有效且 `LREFO` 为真，然后遍历该库 WORLD 的直接成员（`dabFindMember FIRST` / `dabFind NEXT`），
  每个成员 noun 必须在许可单例的“参考世界”名单里：`GPWL STLNKW SYGPWL CYMWRL LINKWL RLWL DOWL
  AREAWL GRIDWL`（`DB_Licence::initialise` `0x101B8A20` 填入）。任一成员 noun 不在名单（例如 SITE）
  或前置条件不满足 -> 返回 0 -> 丢弃。即 LREFO 标记的是“只放参考世界的 DESI 库”，SMM/Diagrams/
  Tags 借它以 R 模式挂设计参考。
- i'. 许可单例与“表达式规则”的澄清：`sub_101B8E70` 返回懒建的 `DB_Licence` 单例（无符号，类名来自
  trace 串 `DB_Licence::initialise` / `DB_Licence::refDesiTest`）。C.f 里 DESI/ENGI/SCHE 的“许可检查”
  `sub_101B45A0(db, &mdb+0x2A0, mdb)` 直接 `call 0x101B8360`，该函数体只有 `return 1`（COMDAT 折叠，
  IDA 随手标成 `DB_OldExpressionDummy::result`）；`openMDB` 里的 `sub_101B4580(&mdb+0x2A0)` 直接
  `call 0x10026CE0` = `mov al,1; retn 4`。两处在 2.1 `core.dll` 里都是**编译期恒真**，因此
  43/541、43/542、43/543 在此版本是死代码；`mdb+0x2A0` 只是一个在 `instance`/`internalInit` 里清零、
  被这两个桩忽略的 int。附录早先写的“表达式规则 this+672”应按此读。
- i''. 记账字段（供 `getWarning` `0x10558b50` / `getDBStatusInfo` `0x10556110` 汇报）：`mdb+0x2A4`
  `map<int dbno, string reason>`（`sub_10558A20` = `operator[]`，`sub_10005140` = 赋值）；`mdb+0x2B4`
  `set<int>` 未开库 dbno（`sub_101B8740` = `set::insert`）；`mdb+0x2C4` `set<int>` 因版本过旧被改为 R 的
  dbno；`mdb+0x2D4` `set<int>` 因版本过新被丢弃的 dbno。`getWarning`：未开库 >1 -> 43/548 按 reason 归并
  计数，==1 -> 43/547；版本两集合非空 -> 143/80 嵌 143/81|82、143/83|84。
- j. **闸 3 版本**：`upgdno < 12010104` 且写开 -> `dword_115B629C` 未设时关掉改 `openRead` +
  143/88；`upgdno > 12010501` -> 143/89，丢弃。
- k. 留下的：`DB_DB::j6343` + `push_back` 到 `this+0x258` 的 `vector<DB_DB*>`
  （`getDB(int)` `0x10553a70` / `ndbs` / `addToList` 同一向量）。丢弃：`close` + 虚析构。

**D. 两张表**

- ISTYP（`unk_10C8C360`）：1 DESI · 2 CATA · 3 COMM · 4 PROP · 5 MISC · 6 ISOD · 7 PADD · 8 DICT ·
  9 ENGI · 10 SYST · 11 GLOB · 12 TRAN · 13 ALYS · 14 SCHE · 15 MANU · 16 NSEQ · 17 XPRD · 18 CONF ·
  19 TASK · 20 RDFN —— 与 Rust `DICT_STYP = 8 / DESI = 1 / ISOD = 6` 吻合。
- 模块号（`AVEVA_ModuleManager` ctor `0x10689af0`）：0 Monitor · 1 Admin · 2 GlobalAdmin ·
  3 Dbutil · 4 Make · 5 Rescue · 17 Specon · 27 Propcon · 33 Isodraft · 35 Isodraw · 37 Dop ·
  40 Draw · 68 Lexicon · 78 Design · 79 Draft · 81 Paragon · 83 Spooler · 84 Dars · 90 HullDesign ·
  91 MarineDrafting · 92 Outfitting · 93 OutfittingDraft · 95 Toolbox · 97 SchematicModelManager ·
  98 Diagrams · 99 Tags · 102 Core · 103 Core3D · 104 ProjectCreation · 105 SteelFabricator ·
  106 StructuralDetailedDesign。

**E. 对本计划的含义**

- 路由目录只需 `dbnum -> 文件`；“开哪些库”完全由 SYS/COMMS 元数据决定，§2.1 结论成立。
- 潜在漏库（已补）：`mdb_membership.rs` 原来直接读 `CURD`，现场 MDB 若用 DBSET 会把整组库读成一条
  没有 `DBNO` 的坏声明。2026-09-12 起 `read_declaration` 先过 `expand_curd_dbsets`：DBSET 条目在原位
  换成 `DBL.DBF` 指向的库，`DBSTL.DBSTF` 嵌套集合递归、集合与库各只进一次；`DBCRITERIA` 非空只报
  problem 不求值（E3D 会按表达式补库，Rust 侧没有 `DB_Expression`）。本机 7 个 E3D 3.1 项目的 SYS 库
  都没有 DBSET（只有空的 `DBSTWL`），行为靠 `FakeSys` 单元用例钉住，真库回归为 AMS `/ALL` 71 条原样。
- Design(78) 非集成模式下，E3D 对 DESI/CATA/PADD/PROP/DICT 等保持基准，不会因模块规则被踢；
  但 ENGI/SCHE 被置 `"N"` 不开。Rust 现在只取 DESI/ISOD/DICT，与此不冲突；以后若做 SCHE/ENGI
  跨库属性，要知道 E3D 本尊在 Design 里没开它们。
- 会话级 `UCURD` 可偏离 `MDB.CURD`：DEFER/CURRENT 命令、working extract 替换（`EXTOWN`）。
  Rust 以 `CURD` 为准等价于“全 CURRENT、无 extract”的基线。
- `upgdno > 12010501` E3D 拒开；Rust 未做此检查，可选作一致性护栏。
- 许可闸在 2.1 `core.dll` 里恒真（C.i'），唯一实际生效的许可逻辑是 `refDesiTest`（C.i），且只在
  SMM/Diagrams/Tags 模块的 LREFO 库上触发；对 Design(78) 路径没有影响。
- 仍未提取：消息 43/540–548 与 143/80–89 的文本（在外部消息文件，不在 `core.dll`）。

### 2.2 E3D 3.1 `core.dll`

- `DB_Element::dbno` `0x5931280` 仍走 `db_get_dbno_from_reference`。
- `DB_Element::getDB` `0x5939520` 仍是“dbno -> `DB_DB::findDB`”。
- `DB_DB::findDB(int)` `0x5902e80` 仍查询 `DBLookup_` 树。

### 2.3 当前 Rust 实现

正确机制已经存在：

- `vendor/e3d-io/src/refno.rs::RefNo::dbno()` 与 IDA 恢复的公式一致，并覆盖大 dbnum 与多 extent。
- `vendor/e3d-io/src/db_element.rs::DbSet::db_slot()` 未命中时仅向 `DbFileResolver` 请求目标
  dbno 的 `DbFilePin`。
- `DbSet` 打开后校验文件自报 dbno；元素存在性走目标库索引点查。
- `E3dModelService` 已用 `DbSet + E3dDbResolver` 按需打开目录依赖。
- `current_mdb_source_inventory()` 已能从 `MdbMembership` 直接建立
  `dbnum -> (type, project, path)`，不需要元素扫描。

错误的旧旁路仍然存在：

- `DirectStore::dbnum_of()` 仍依赖 `CataDbLocator::resolve_ref0()`。
- `MdbMemoryStore` 为满足该接口，逐元素重建 `ref0 -> dbnum`。
- `model_source::dbnum_of_root()` 第一次遇到一个 root 时逐个打开全部 DESI 并尝试索引点查。
- `DirectTreeService` 为 noun/owner/根枚举绑定整份 `MdbMemoryStore`。
- read-through 启动同时存在主流程 `ensure()` 与 Web 属性服务预热，缺少 single-flight。
- `refresh_if_stale()` 发现任一库 sesno 变化后重扫整个 MDB。

旧代码说“`get_0()` 不是 dbnum”本身没错；错误在于只考虑了“直接当 dbnum”与“全局反查表”
两种方案，漏掉了“按 RefNo 位布局解码 dbnum”。

## 3. 目标与非目标

### 3.1 目标

- JEU 正常模式下先启动 HTTP，再做任何元素级读取。
- `room_membership=false` 时，冷启动不执行 `scan_elements(Header|Named|Full)`。
- 单元素属性、祖先链与单根生成只打开实际触达的数据库。
- 一个 DESI 的 TTY `SAVEWORK` 只作废该 dbnum 的会话与缓存，不重建 417 库骨架。
- 保持现有会话冻结、文件身份、MDB 范围、项目优先级和错误可诊断性。
- 删除旧的 `ref0 -> dbnum` 全局映射与跨全部 DESI 试查。
- 健康接口报告“路由目录、已开库、缓存和失效状态”，不再把“全量骨架已装”当健康条件。

### 3.2 非目标

- 不改变 RefNo 位格式。
- 不改变 SYS/CURD 的 MDB 成员语义。
- 不把同号 dbnum 的跨项目选择简化为“随便取一份”。
- 不在本轮实现全局名称搜索的新索引格式；该功能允许显式触发独立索引任务。
- 不用第二套解析器长期双跑。
- 不把 8188 或任何现场 dbnum 写死。

## 4. 必须保持的不变量

1. **编号内生**：有效 RefNo 的 dbnum 只由 `RefNo::dbno()` 给出。
2. **路径有上下文**：dbnum 只回答编号；项目、文件与模板由当前 MDB/依赖上下文解析。
3. **文件自证**：打开文件后必须校验头部 dbnum/type 与请求一致。
4. **时点冻结**：一次模型根、属性快照或历史请求不得混用不同 session。
5. **范围封闭**：解出的 dbnum 不在当前 MDB/合法依赖目录时响亮失败。
6. **按需成本**：普通请求成本与触达库/元素数量成正比，不与 MDB 总元素数成正比。
7. **定向失效**：文件变化只失效对应 dbnum；换文件、回滚和 session 前进必须能区分。
8. **项目歧义显式**：同 dbnum 多项目候选仍按目标设计库项目、MDB 声明和既有优先级裁决。
9. **全量操作显式**：只有用户请求全局搜索、全 MDB 模型/房间操作时才能遍历全量，并报告进度。
10. **HTTP 不等数据预热**：端口、`/health` 与设置页不得等待元素缓存。

## 5. 目标模块边界

### 5.1 `MdbRouteDirectory`

新增轻量路由目录，建议放在 `src/data_interface/mdb_route_directory.rs`。

职责：

- 持有 `Arc<MdbMembership>`。
- 按 dbnum 保存 `db_type/project/path/shadowed`。
- 从 RefNo 解码 dbnum，并验证它属于当前 MDB或允许的依赖项目。
- 为 `DbSet` 生成 `DbFilePin`。
- 维护每个 dbnum 的文件身份与已知 session generation。
- 暴露数据库级统计，不保存逐元素 noun/owner/NAME/属性。

建议接口：

```rust
pub struct MdbRouteDirectory { /* no per-element maps */ }

impl MdbRouteDirectory {
    pub fn from_membership(
        membership: Arc<MdbMembership>,
        schema: Arc<DirectSchema>,
    ) -> anyhow::Result<Self>;

    pub fn route_refno(&self, refno: RefNo) -> Result<DbRoute, RouteError>;
    pub fn route_dbno(&self, dbno: u32) -> Result<DbRoute, RouteError>;
    pub fn pin(&self, dbno: u32, at: SessionSelector) -> Result<DbFilePin, RouteError>;
    pub fn element_databases(&self) -> impl Iterator<Item = &DbRoute>;
    pub fn refresh_path(&self, path: &Path) -> Result<RouteRefresh, RouteError>;
}
```

`DbRouteDirectory` 实现 `e3d_io::db_element::DbFileResolver`。项目相关选择仍调用现有
`select_dependency_file` 规则，不复制优先级。

### 5.2 `UdaCatalogProvider`

`MdbMemoryStore` 目前顺便持有 UDA catalog。把它拆成独立 provider：

- 输入只用 `MdbMembership::dictionary_paths()` 与 schema；
- 按字典文件身份缓存；
- DICT 变化只刷新 UDA catalog；
- 不触发任何 DESI 元素扫描。

### 5.3 按需元素缓存

统一以 `DbSet` 作为跨库句柄宿主：

- RefNo 创建句柄零 I/O；
- 第一次访问 noun/owner/属性时打开目标 db；
- 第一次元素访问走目标库 B+tree；
- 同一请求或同一 provider 内缓存已触达元素；
- 缓存按 dbnum generation 失效。

初期可复用 `DbSet::with_element_cache()`；上线前必须加入命中数、条目数和按 dbnum 失效指标。
若长期浏览使缓存无界，再以实测决定 LRU 上限，不在本轮先造复杂淘汰器。

## 6. 分阶段实施

### 阶段 0：证据与防回归基线

只加测试和计数，不改生产路由。

- 给 E3D 2.1/3.1 语料做 corpus 对账：每个已读元素的 `refno.dbno()` 必须等于文件描述符 dbnum。
- 覆盖：
  - `24572/115 -> 8188`；
  - primary/secondary extent 均解到同一 dbnum；
  - 大编号 `251000`；
  - 非法/空 RefNo；
  - 解出 dbnum 但不属于当前 MDB。
- 给 `ReadOnlyEngine::scan_elements`、数据库 open、索引 point lookup 增加测试计数器。
- 冻结现有 `/health`、`/dbnums`、tree、attributes、model ensure 响应契约。
- 保存 JEU 当前基线：启动 68 秒、417 库、1,221,858 元素、进程 RSS、首个属性与首个根耗时。

退出标准：位解码 corpus 零分歧；否则停止本计划并先解释例外 RefNo。

**结果（2026-09-12）**：E3D 2.1 883 库 16,154,107 元素 + E3D 3.1 1,064 库 22,736,906 元素，`refno.dbno()` 与描述符
`db_mark` 分歧 **0**，最大 dbno 251,712。注意仓内 `refno_dbno_corpus.rs` 的全语料门因文件名按字节 `split_at(3)` 在中文
文件名上 panic，从未在本机跑通——修法与实测细节见执行切片 §10.1 / W0。JEU 的 RSS / 首属性 / 首根三项本机不可采
（见 §7.4），不阻塞阶段 1。

### 阶段 1：建立轻量路由目录

- 从 `MdbMembership` 构造 `MdbRouteDirectory`。
- 复用 `current_mdb_source_inventory()` 的文件选择逻辑，消除第二份目录构建。
- 为同号跨项目候选建立确定性测试。
- 打开文件时继续执行 `DbNumberMismatch` 和模板类型校验。
- 从 descriptor + page 1 extent table 生成 `/dbnums.ref0s`，不扫描元素。

退出标准：仅读 SYS/CURD 与每库必要头页即可完整给出 dbnum、路径、类型、session、extent refs。

**结果（2026-09-12，`bb16105d2`）**：`MdbRouteDirectory::from_membership / add_dependency_dir / pin_for(preferred_project)`，
实现 `e3d_io::DbFileResolver` 与 `CataDbLocator` 兼容壳（`dbnum_of_ref0` 恒 `None`）；AMS `/ALL` 71 条 CURD → 70 条路由 /
29 个元素库，构造 355 µs，70 个文件头自证 0 不符。**未做**：`current_mdb_source_inventory()` / `E3dDbResolver` 派生自目录
（执行切片 §9 Q1 定为下一片）、`/dbnums.ref0s` 从 extent table 生成（阶段 6）。

### 阶段 2：先删除两条最贵的错误路由

#### 2A. `model_source::dbnum_of_root`

- 删除“遍历全部 DESI 逐库试查”。
- 改为 `root.dbno()`。
- 用 `MdbRouteDirectory` 验证目标库存在且类型为 DESI/ISOD。
- 可选地只在目标库做一次 `element_exists(root)`，用于区分“编号合法但元素不存在”。
- 删除 `DBNUM_BY_REF0`。

#### 2B. `DirectStore`

- `dbnum_of()`、`locator_dbnum()`、`dbnum_hint()` 改用同一个 RefNo 解码 helper。
- `pin_from_locator()` 改从 `MdbRouteDirectory`/`DbFileResolver` 获取目标 pin。
- 删除 `UnresolvedRef0`；替换为 `InvalidRefNo`、`DbNotInMdb`、`NoFileForDbnum` 三类明确错误。
- 保留文件身份与 session 冻结语义。

退出标准：单根/单属性路径不再调用 `CataDbLocator::resolve_ref0()`，也不打开非目标 DESI。

**结果（2026-09-12）**：2B `5cc5e3e04`——`DirectStore::dbnum_of / locator_dbnum / dbnum_hint` 只经 `refno_route::refno_dbnum()`，
`UnresolvedRef0` → `InvalidRefNo / DbNotInMdb`，`open_session` 加 `DbNumberMismatch` 核头；真库门 DESI 1112 上 25 条跨库引用换库读通、
仅开 2 个会话。2A `425ca98d8`——`dbnum_of_root` 位解码 + 核 DESI/ISOD 成员 + 目标库一次 `find_element`，删 `DBNUM_BY_REF0`。
退出标准成立：`tests/refno_native_guards.rs` 守 `resolve_ref0( / dbnum_of_ref0( / .get_0()` 不再出现在 `direct_store.rs` 生产代码、
`dbnum_of_root` 不遍历 pins 且只开一库。探针 `7f501ab0d` 同步退役 `InMemoryCataLocator::build_for_project`，ams8000 对账 1313/1313。

### 阶段 3：树与属性服务脱离全 MDB 骨架

#### 3A. `ElementAttributeService`

- 构造参数改为 `MdbRouteDirectory + UdaCatalogProvider`。
- 删除 `mdb_memory_store::ensure()`。
- 用共享 `DbSet`/resolver 完成跨库属性读取。
- freshness 从“整份 skeleton fingerprint”改为“本服务已触达 dbnum 的 generation 集”。

**结果（2026-09-12，`425ca98d8`）**：`ElementAttributeService::open` 不再调 `mdb_memory_store::ensure()`，定位器
`MdbRouteDirectory::from_membership + add_dependency_dir`，UDA 字典 `load_uda_catalog` 直接读 DICT；`is_current` /
`snapshot_epoch` 改为成员文件指纹（长度 / 修改时刻 / 最新会话）——比「已触达 dbnum 的 generation 集」粗，按 dbnum 定向失效
留阶段 5。`UdaCatalogProvider` 未做（切片 §8-3：只读 DICT 不缓存）。同一提交里 3B 只换了 `DirectTreeService` 的定位器，
`ensure()` 与 `Arc<MdbMemoryStore>` 字段仍在（WORL 枚举、noun / owner 查询），归阶段 4 切片。

#### 3B. `DirectTreeService`

- 删除 `Arc<MdbMemoryStore>` 字段。
- `ancestors()` 逐级调用 `DbElement::owner()`，跨库自动 resolver。
- noun 从已点查的 `ParsedElement` 获取。
- children 继续读父元素存储的成员表。
- generation roots 复用现有 `enumerate_generation_roots_in_subtree`，数据源换成 `DbSet`。

#### 3C. 数据库 WORLD/根枚举 spike

先验证再选实现：

- 对全部本机 E3D 2.1/3.1 语料验证 WORLD 的稳定定位方式；
- 对照 IDA 的 `DB_DB::getWorld/getWorldMembers`；
- 优先实现单库索引点查或确定性 WORLD RefNo；
- 若格式库暂时无法复现原生索引，只允许“第一次展开该 dbnum 时扫描该单库”，不得回退为启动扫全 MDB。

**spike 结果（2026-09-12，1,947 库）**：每库恰好一个 WORL，owner 为 0；全部 1,905 个非 SYS/COMM/MISC 库的 WORL 都是
`RefNo(descriptor.primary_extent_ref, 0)`，即 `(dbno | 0x2000) / 0`，0 例外，`find_element` 一次点查命中。例外仅项目
管理库 COMM(8190) `16382/1`、MISC(8189) `16381/1`、SYST(8191) `24575/3`，它们不是 CURD 元素库。每库另有 1 个无主的
`MNUM` 系统元素（noun_hash 803020），不是根。**结论：采用确定性 WORLD RefNo，上面的“首次展开单库扫描”兜底不需要。**
细节见执行切片 §10.2。

退出标准：树首页只读取数据库根/直接成员；展开哪个库才读取哪个库。

### 阶段 4：启动链真正懒加载

执行切片：`docs/plans/2026-09-12-lazy-startup-dev-plan.md`（已批准，2026-09-12）。它把 3B/3C 与阶段 5 的最小形态
（按 dbnum 轮询会话号）并进同一个包——只删预热不给树脱骨架，第一次点树仍扫全 MDB；只删预热不改 2 s 循环，
第一次 DESI 保存会把骨架建回来。理由与拍板项见该文件 §0.3 / §8。

- 正常模式先完成 HTTP bind，再启动任何可选预热。
- 删除 `run_cli` 与 `run_read_through_core` 的无条件 `mdb_memory_store::ensure()`。
- 删除 `serve()` 中绑定端口前的 `element_attributes.get()` 预热。
- provider 首次请求采用 single-flight，避免并发首次请求重复开库。
- `room_membership=false` 时启动不得读取 DESI 元素。
- `room_membership=true` 的启动房间任务按 dbnum 流式处理；不要求先构造全 MDB noun/owner 表。
- 保留已经完成的 HTTP 地址后备：
  `http_api_addr -> server_release_ip -> 127.0.0.1:9099`。

退出标准：JEU 端口可用与 `/health` 返回不等待 122 万元素；日志中无“DESI 骨架 N 个元素”。

### 阶段 5：按 dbnum 刷新与缓存失效

- watcher 事件先归一成 `(path, dbnum, old_generation, new_generation)`。
- 只替换变化 dbnum 的 `DbFilePin`。
- 调用 `DbSet` 的定向缓存清理；现有实现已按 `refno.dbno()` 清某库缓存，可直接复用。
- DirectTree/ElementAttribute provider 仅在触达 generation 已变化时重建对应 session。
- DESI TTY 保存只让该 dbnum 的已生成 roots 失效并重生成。
- CATA/DICT 变化按依赖关系处理：
  - DICT 只刷新 UDA catalog；
  - CATA 先按已记录依赖 roots 定向失效；
  - 若依赖证据不完整，可暂时对已生成 roots 保守失效，但不得重扫全部 DESI 元素。
- 取消 read-through 每 2 秒“全部库头检查 + 整份骨架重建”；保留低频、分批的漂移审计作为 watcher
  漏事件兜底。

退出标准：一个 JEU DESI `SAVEWORK` 的日志只出现该 dbnum 的 reopen/invalidate，不出现 417 库重载。

### 阶段 6：迁移健康接口与删除旧模块

- `/health.core` 改为：
  - route directory 状态与 generation；
  - 路由库总数；
  - 当前已开 db 数；
  - 已缓存元素数；
  - stale/failed dbnum 列表；
  - 最近一次定向刷新。
- `/dbnums` 从 `MdbRouteDirectory` 生成冷行；`ref0s` 来自 extent table。
- `/model/records` 的 `session_vector` 只报告该结果实际钉住的 source/dependency sessions，
  不再冒充全 MDB 快照。
- 删除：
  - `MdbMemoryStore.elements`；
  - `affiliations`；
  - `by_noun`；
  - `refresh_if_stale()` 整份重建；
  - 启动 skeleton 日志；
  - `CataDbLocator::dbnum_of_ref0/resolve_ref0` 的生产依赖；
  - `model_source` 的全 DESI 试查。
- 若 `MdbMemoryStore` 已无职责，删除整个模块；UDA 与路由不得继续借其名字寄生。

退出标准：生产代码中不存在为“找 dbnum”调用 `scan_elements` 或逐库 `find_element(root)` 的路径。

### 阶段 7（2026-09-12 已定为独立阶段）：单根生成不再整库扫 owner 表

阶段 0–6 完成后，普通请求链上剩下的唯一单库全扫是 `e3d_model_service::generation_input()` →
`scan_index(&pin.file, sesno)`：为一个根生成模型时对**目标库**做一次 Header 全扫，建 `SourceIndex { roots, owners }`，
整库 owner 表随 `GeneratedSnapshot` 进 publication，按 `(dbnum, sesno, identity)` 缓存；kv-mem 的 `model_dbnum_ensure`
也走它。它不违反 §7.5 的门（不开全部 DESI），但违反不变量 6 的字面（成本 ∝ 触达元素）：单根首次生成 = 该库全部
元素头 + 一张整库 owner 表。改法：publication 需要的 owner 从生成时已解析的元素记录头取；`roots` 只在整库 ensure
才需要，按库流式。**已定**：作为独立的阶段 7，在阶段 6 之后另起执行切片；阶段 0–6 的任何切片不碰它（执行切片 §9 Q16）。

`MdbMemoryStore` 每个消费者的零扫描替代对账表见执行切片 §10.3（结论：能不全库扫描）。

## 7. 测试与验收

### 7.1 单元/属性测试

- RefNo 解码与 extent corpus。
- dbnum 不在 MDB、文件缺失、文件自报号不一致、类型不一致。
- 同 dbnum 多项目时按目标设计项目选择。
- resolver miss 不猜测。
- pin 更新只清目标 dbnum 缓存。
- 并发首次请求 single-flight。
- session 前进、文件替换、回滚分别产生正确 generation。

### 7.2 源码护栏

- 启动函数不得出现 `scan_elements(`。
- `model_source::dbnum_of_root` 不得遍历 `current_mdb_sources()` 的全部 pins。
- `DirectStore::dbnum_of` 必须调用共享 RefNo 解码 helper。
- HTTP bind 之前不得调用属性/树 provider 的 `get()`。
- read-through 文件事件不得调用全 MDB rebuild。
- 生产路由不得出现现场常量 `8188`。

### 7.3 现有回归

- `web_service::tests`
- direct tree TTY 对拍
- direct attribute/UDA parity
- model generation root parity
- CATA dependency closure
- MDB Global resolver
- E3D 2.1 与 3.1 schema pack
- room membership 开/关两档
- runtime embedded-mem 与 spawned-rocksdb 两档

### 7.4 离线验收（AMS 为准；JEU 仅现场）

**2026-09-12 起调试 / 验收项目为 AMS**（`DbOption.toml`：E3D 3.1 `AvevaMarineSample` / MDB `ALL` / `room_membership=false` /
`watch_dbnums=[7998, 8000]`）。JEU 在另一台机器上、本机不可访问，下列步骤在 AMS 上逐条执行，数字按 AMS 实测。
量级差别要先说清：AMS `/ALL` 只声明 28 个 DESI/ISOD（2026-09-05 实测骨架 611,345 元素 / 0.72 s / 19.8 MiB，
`lib.rs` `run_cli` 注释），远小于 JEU 的 417 库 / 68 s——所以「HTTP 不等骨架」这类门在 AMS 上**只能靠计数器**
（`read_stats.scan_calls == 0`、`database_opens`）与日志断言，不能靠计时体感；下面的 route count 在 AMS 上是 28，
不是 417。JEU 只在拿到现场机器时按同一清单补跑。步骤 3–4 用现成夹具
`scripts/e3d/db8000_bran_ftub_move_apply.mac` / `..._restore.mac`（apply + restore 业务净变化为空），验 8000 刷新、7998 不动。
只跑 3.1；2.1 的 AMS 在阶段 4 收尾补一次端到端。

1. 启动 `room_membership=false`：
   - HTTP 可访问；
   - 启动期 DESI element scan = 0；
   - route count = 417；
   - open db count 初始接近 0。
2. 点开一个树根并读取一个元素属性：
   - 只打开目标 DESI 与实际跨库依赖；
   - 属性与 E3D TTY 相同。
3. 对目标元素执行一次 TTY 修改和一次 `SAVEWORK`：
   - 只刷新目标 dbnum；
   - 模型更新到新 sesno；
   - 其它 dbnum 的 pin/cache generation 不变。
4. 再执行一次恢复 `SAVEWORK`：
   - 属性、owner、模型几何恢复；
   - apply + restore 业务净变化为空。
5. 开启 `room_membership=true` 单独验收房间，不与普通属性/模型链混为一轮。

### 7.5 性能门

- JEU HTTP 监听不等待 68 秒骨架。
- 普通冷启动 DESI 元素扫描数为 0。
- 单属性读取打开数据库数应为 1 加真实依赖数。
- 单根生成不得打开 417 个 DESI。
- 单 DESI 保存后不得扫描其它 416 个 DESI 的元素。
- 常驻内存随实际触达元素增长，不在启动时直接分配约 40 MiB 骨架。
- 所有指标进入日志与 `/health`，不能只靠人工计时。

## 8. 发布策略

1. 先合入阶段 0–2，保留旧骨架但停止让属性/模型路由依赖它；用 parity 证明新路由。
2. 再合入阶段 3–4，把启动改为懒加载。
3. 完成阶段 5 后才宣称“TTY 增量不做全 MDB 重扫”。
4. 阶段 6 删除旧模块与兼容字段，避免两套路由长期并存。
5. 用新的唯一 build_id 生成 runtime ZIP 和二进制 patch；旧 0.1.23 dirty 包不覆盖。
6. 离线机先备份 EXE、配置与日志；不需要备份或修改 E3D 源数据库。

## 9. 风险与应对

- **RefNo 例外格式**：阶段 0 corpus 必须先证明；未知格式 fail closed，不回到全 MDB 猜测。
- **跨项目同号库**：编号解码后仍必须经过上下文 resolver，不允许全局 `dbnum -> 任意文件`。
- **按需首击延迟**：只发生在被访问的单库；可在 HTTP ready 后后台预热常用 dbnum，不恢复启动阻塞。
- **缓存无界**：先量 touched-element 曲线，再决定 LRU；任何淘汰都以可重读为前提。
- **watcher 漏事件**：低频分批 header audit 兜底，不用 2 秒全 MDB 重扫。
- **房间全量需求**：房间任务按库流式读取并显式报告进度，不复活全局元素骨架。
- **API 兼容**：（2026-09-12 作废，见 §10-2）`skeleton_elements` 无前端消费者，阶段 6 直接删；新增 route/cache 指标
  与删除在同一阶段落地，冻结测试同步改。

## 10. 审阅者拍板（已定，2026-09-12）

1. 渐进路线：**接受**——先替换路由，再删除骨架；执行切片按 W0 → W4（2A 前置）→ W5/W6 → W7/W8 → W9 推进。
2. `/health.core.skeleton_elements`：**阶段 6 直接删除**，不留兼容版。plant-ui 与 plant3d-web 都不读它，唯一"消费者"是
   `tests/refno_native_api_contract.rs` 的冻结清单，阶段 6 同步改那条测试。§9 风险项里"至少保留一版"作废。
3. 全局 NAME 搜索：**显式后台索引任务**，不进启动与普通请求链（与摘要版 PLAN 一致）。
4. 临时包：**否**。路由与启动改造完成、测试通过后再出新 build_id 的包。
5. header 漂移审计：**默认关闭**（摘要版 PLAN 假设项）；周期与每轮库数留到阶段 5 按 AMS 实测定。

其余十五项执行口径（切片范围、工作树、AMS 基线、目录边界、同号裁决、generation 定义、护栏形态、提交拆法）记录在
执行切片 §9，此处不重复。

