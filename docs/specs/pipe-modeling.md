# 管道建模功能规格（gen-model 后端）

状态：草稿（待评审后转 ready-for-agent）
日期：2026-10-16
前端：OpenCADStudio（UI spec 见 `docs/specs/pipe-modeling-ui.md`，不在本文件范围）
后端：`D:\work\plant-code\old\gen-model`
逆向依据：`docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md` §15-§18（本仓快照，全部行为断言带 E3D 3.1 源文件行号；上游原文在仓外逆向库 `E:\reverse\e3d` 的 docs 目录，托管层精确参照在其 csharp-code 目录）

## Problem Statement

gen-model 目前只消费 E3D 会话产物（`e3d_collector` 窗口差分 → `pe`/`ATT_*` 落库 → 交付单元生成模型），**不能创建/编辑管道设计数据**。E3D 的管道建模（建管、布件、连接、开孔、分段、制造检查）语义已全部逆向成文档，但没有对应实现。需要一个权威的后端管道建模服务，使 OpenCADStudio（及后续任何客户端）能以与 E3D 一致的语义完成管道设计。

## Solution

新建 workspace 成员 crate `pipe-model`（暂名），提供领域服务 API：输入操作 + 上下文（等价 CE），输出 `pe`/`ATT_*` 行级变更批次与结构化结果，由现有持久链写库——**产出行格式与 `e3d_collector` 产出的属性行同构**，下游交付单元重算/房间/级联无需感知数据来自编辑还是来自 E3D 会话。

服务按 E3D PML 对象边界组织（PIPINGPROCESSDATA→ProcessData 写序、BRANCHCONNECTION→连接契约、PFEVALUATESPOOLMEMBER→断管算法、pfGetTubeParts→管件段谓词、PFWELDCONTROL→焊接流），全部行为断言可回溯到逆向文档行号。

## User Stories

### A. 管/支管生命周期

1. 作为管道工程师，我要在某个 ZONE/EQUI 下创建 PIPE 并同时指定规格（PSPE）、公称孔径、保温/伴热规格、温度/压力/斜率参照，以便一次完成建管——等价 `NEW PIPE` + `PIPINGPROCESSDATA.updateElement` 全写字段。
2. 创建 PIPE 后自动建首个 BRAN：HBOR=TBOR 继承管孔径、自动生成支管名（PIPEBRANCH 规则）、`SELECT HSTU` 选头管——服务直接返回已可编辑的支管。
3. 作为工程师，我要在写库前被告知不可写的原因（无写权限/属主层级不合法），而不是得到一个半成品 PIPE。
4. 改名 PIPE 或 BRAN 时级联重命名所有以前缀匹配的子元素（E3D `changeName` 语义）。
5. 改保温/伴热规格时按 E3D 级联语义：所有 ISPEC/TSPEC 等于旧值的子元素一起改，不是只写节点本身。
6. BRAN 改 PSPE 后自动重选 HSTU。
7. BORE 只写 PIPE；BRAN 孔径只走 HBOR/TBOR。

### B. 支管端编辑与连接

8. 作为工程师，我要分别编辑 BRAN 头/尾的孔径、位置、方向、连接型（HCON/TCON），写入顺序与 E3D 一致：`xBOR → xPOSITION → xDIRECTION → xCONNECTION → LHEAD/LTAIL=TRUE`；LHEAD/LTAIL 是"端已定义"标志。
9. 连接支管端到另一支管的端（`CONNECT PH TO PT OF <bran>`）或元件/管嘴的 P 点（`CONNECT P<end> TO P<n> OF <elem>`）。
10. 连接成功后从结果回读实际 HCON/HBOR（引擎推导值），而不是信界面输入。
11. 连接失败时提供 E3D 兜底语义：几何对齐降级（写 TPOS=pos of elem/TDIR=p1/TBORE=lBore/TCONN='BWD'）与 FCONN 强接+按最小管段长度（默认 100mm）占位拉开——两种策略可配置、返回结果标明走了哪条降级路径。
12. 连接法兰型端点时自动补件：按连接型查目录连接表选兼容 GASK+FLAN，头端 `gask before 1 → flange → connect`、尾端反向；已有边界件（ltlen==0）时复用不重建。
13. 连接后自动执行跨支管连接型对齐：读对端支管边界件的 LCONN/ACONN 互写两端 HCON/TCON；边界间有管段（atlen/ltlen>1）时不动。
14. 断开连接：HREF/TREF 置 nulref 并保持端几何可继续编辑；重连走同一连接接口。

### C. 元件生命周期

15. 作为工程师，我要按元件类型（FLAN/GASK/TEE/ELBO/BEND/REDU/VALV/OLET/ATTA/FTUB/COUP/SHU/PCOM/CAPA/NSTF/COMP…）在支管上创建元件：定 SPRE → 经规格答卷定 SPCO → 定向——三步结果都在返回里。
16. 元件创建支持三种定位语义：接前驱件（Connect）、管上点（精确投影/跨管投影回支管）、自由点（轴线求交 + 距邻件 < 2×管径自动吸附，吸附距离可配）。
17. 成组插入的元件组可整组翻转：成员逆序重排 + 绕组首尾连线（平行时取角平分线方向、夹角×2；非平行时取与局部轴正交方向+180°）逐件翻转偏移旋转——等价 `flip('CREATE')`。
18. 单件翻转 `FLIP ORI` 时正确处理 LSTU：前驱件的 leave tube 传名、多 LSTU 规格选不出时报可重选错误、单 LSTU 规格自动 `sel lstu`。
19. 逆流创建（against-flow）遇到孔径不等时交换 arrive/leave，不报硬错。
20. 元件删除/重选规格：重选走 `SPREF 直写` →失败回落 `SELECT WITH SPEC <答卷>` 两档；BEND 类换规格后校验半径自洽。
21. ATTA 类元件在规格变更期先置 SPKBRK 钉住防漂移，完成后复位。
22. TEE/OLET 的 off-leg（第三腿）换规格时：算 `offleg = 6 - (arrive+leave)` 的 P 点号，并把被接支管的 HBOR/HPOS 一并回写。
23. 元件替换后若两侧被拆断，执行 COMPONENTRECONNECT 等价语义：先连上游再连下游。

### D. 开孔/支管连接（Tapping）

24. 在管段（TUBI）上开孔：自动选 PTAP（`/General-Tappings` 目录语义），写入 through/orientation/leave tube/CREF/depth/bore，并把支管头接到 PTAP——等价 `createPtapConnection`。
25. 在元件上开孔：建 PIPCA 子件，DCONNECT BWD 语义、位置/方向/孔径随宿主算。
26. TAP 型连接件支持"管上开孔 + COUP + 支管两端"的两段式流程，阶段中间态（半成品支管）可识别、可回滚。
27. 开孔后写支管头/尾位置与连接型（HPOS/HREF/HCON 或 TPOS/TREF/TCONN），与原支管恢复 CE 一致。

### E. 分段/合并

28. 在指定元件处把 BRAN 一分为二（`splitpipe.pmlfnc` 全文级语义，§18.17.1）：切点边界在元件**到侧**——切点元件含在迁移集、成新支管首成员；`moveTo` 双模（`PIPE`=新建复制管 `COPY ATT OF`+新支管名 `<新管>/B1`；`BRANCH`=同管直挂+名 `<管>/B<mCount+1>`）；管名 `-Split(n)` 先剥旧后缀再撞名自增（`handle (41,12)`）；互连按 `CONN PH TO FIRST MEM`（新支管 HREF=切点元件）+`CONN PT TO PH OF`（旧支管 TREF=新支管）+ **HCON/TCON 交叉互换**（两侧各继承对方 CONN 算出值——内核依赖，批内显式 warning 不伪造）。
29. 合并两支管：源成员迁入目标（支持跨库确认）、恢复头尾连接数据、源支管变空即删、源管全空可级联删——等价 `mergeTwoBranches`，整批一个事务。
30. 切点合法性按 `splitpipe` 函数级语义：仅拒**非直属成员与首件**（`sequence eq 1 AND atle eq 0`，首件切点使旧支管空）；**末件分段函数级放行**（PML 源码标 `REVISIT` 已知缺口）产出单成员新支管——UI 层另有拦截属表单侧职责。

### F. 规格/孔径变更

31. 批量改元件规格：逐件 `spref=` 直写，失败回落 `SELECT WITH SPEC <choice>` 答卷路径；结果按元件报告成功/失败/降级。
32. 变更时列表给出"可改/不可改/已改"三态着色所需的判定数据（写权限、规格成员资格、当前匹配态）。

### G. 斜管

33. 支管级斜率：按 leg 分段应用 fall/angle，报每 leg 结果与不可斜原因。
34. 元件级斜率：DIR/NOMDIR 写序、TEE 类 `ori and p3` 定向、角度不可达返回显式错误（not silently clamp）。

### H. 焊接

35. 按边界自动造焊缝：`NEW WELD BEFORE|AFTER CHOOSE SPEC <spec> WITH <cond> SHOP T|F STYP <type>` 语义；无规格路径退化为 `SELECT SPEC /WELDGAP` + 实测 gap（aPos.distance(lPos)）写 DESP NUM 1。
36. 焊接规格选不出时降级为无 SHOP 条件重试一次；仍失败则收集为错误不中断批次。
37. 余量超限时产生 excess 警告，可由调用方选择整批回滚（UNDODB 等价物）。
38. 焊缝删除/重编号接口。

### I. 制造分段与校验（PSPOOL/PPIECE）

39. 作为制造工程师，我要对一条 PIPE 生成 PSPOOL：断管算法逐行等价 `PFEVALUATESPOOLMEMBER`（asfbreak/lsfbreak/csfbreak+shop 标志前/后向扫描，href/tref/cref 跨支管续扫，TEE off-leg 递归）。
40. 生成 PPIECE：边界谓词逐行等价 `pfGetTubeParts`（TUBI 长度、BEND shop/NCuts 含纯斜接片 ACTNCU-1、ATTA spool-break、FTUB skey、shop/offline、支管边界六类）。
41. 每个 PPIECE 指派弯管机/焊机（BENDMC/WELDMC，默认 `/BENDING-MACHINE-1(3D)`、料长上限 6000 可配），执行校验序列，结果写 PPFMCV/PPMODI/FEDEXS/MINFED/PPCUTL/AEXCESS/LEXCESS/LOOS。
42. 校验结果分级：Valid / ValidWithWarning / Invalid——**Release 只在全选管 ValidWithWarning 时允许**（`enableReleaseButton` 门控语义）。
43. 管段三级名回写（SPLH/SPLN/SPLT = belrfa 语义）与 PSTYP(SHOP/SITE/FITT) 标记。
44. 校验消息能映射回定位数据（元件集合 + 中心点），供 UI 高亮。

### J. 批量与设置

45. CSV/表格导入：按表头校验 → 逐行 `NEW <type>` 或更新既有 → 逐列按现值类型写属性（TEXT/DBREF 补 `/` 前缀）→ 行级 SUCCESS/FAIL + 错误串——等价 `PIPELOADER`。
46. 导入可驱动支管接管嘴（`CONNECT PHEAD/PTAIL TO P1 of <nozzle>`，cref 占用校验）。
47. 全局管道默认值表（snapDist=2×OD、minTube=100mm、autoAlign/autoCreate=T、roundLaserTolerance=1deg、isoMode=3 等 §18.4 全表）持久化、按用户/全局两档加载、成员级覆盖。

### K. 通用语义

48. 每个变更操作是命名事务（UNDOABLE 等价物），事务内任何失败整批回滚并返回错误明细；Apply 与"Apply+关闭"是同一服务调用，不回滚已提交事务。
49. 操作前置校验（写权限、属主类型合法、CE 上下文合法）统一返回结构化错误，供 UI 走 warning-Done 重试流。
50. 服务返回的每个结果含受影响元素集合与辅助定位数据（端点 pos/dir/bore、管段中心点等），供 UI 画标记/高亮——后端不画图但给齐画图数据。

### L. 制造断点编辑与管端处理（§18.16 `pfModifySpool`/`pfFabricationPipeEnd*` 取证）

51. **Spool 断点是双形态数据模型**，两种表示表达同一语义、生成与校验算法必须同时识别：
    - **No-weld 断点 = 元素标志位**：`ASFBREAK`（arrive 侧）/`LSFBREAK`（leave 侧）/`CSFBREAK`（TEE·OLET 的 cref 侧）/`TSFBREAK`（打在**对方 BRAN** 上）。相邻两元件间断点成对写：`firstComp.lsfbreak + secondComp.asfbreak`。
    - **Weld 断点 = 真 WELD 件**：`/WELDGAP` 规格、`STYPE FWS`（现场焊）、`DESP NUM 1 = 0.001m`（写 0 不显示形状，E3D 伪零惯例）、`SHOP=false`。
52. **断点候选规则**：TUBI 需 `itlength ≥ 1mm`、ATTA 需 `spkbrk=true` 方可作断点宿主；邻接面为 BRAN/TUBI 无条件候选，邻接元件仅当其 `shop=false` 时可打现场断点；OLET 仅 P3 侧；TEE off-leg 断点 ppoint = `6-(arrive+leave)`；跨支管 cref 连接断点成对写（TEE/OLET 的 `csfbreak` + 对方 BRAN 的 `tsfbreak`），成对清除时尊重另一端 cref 仍连的状态。
53. **断点设置/删除/替换**：Weld 断点建序 = `NEW WELD`（BRAN 上 `BEFORE 1`、方向 FORWARD/BACKWARD 控制插入侧）→ `SELECT WITH SPEC /WELDGAP STYPE FWS` → `DESP NUM 1 0.001m` → `LSTU` 继承所选件 → backward 模式 arrive/leave 互换 → `CONNECT`，失败 (7,3) 走 `FCONNECT`，再失败 `DELETE WELD` 回滚 → `ORIENTATE`。两形态互遇时**confirm 替换**（No-weld 遇现场焊按位距 <0.002 判同位→删焊打标志；Weld 遇标志→清标志建焊）；候选探测位距 ≤0.5mm。
54. **端/进给余量编辑**：piece 边界对 `arriveBoundary.LEXCESS` + `leaveBoundary.AEXCESS` 直写（元素无该属性视作 0）；`LOOSE` 标志件与默认散件余量值联动；BEND 弯管进给余量写 `USEFED`。
55. **FTUB 螺纹/斜切管端**：FTUB 是规格元件非几何操作——`SPREF=/THREADPIPEEND` 或 `/SLANTEDPIPEEND` + 孔径名址；`DESP[1..5]` = 角度（螺纹=0）/管外径/流向（头+1 尾-1）/连接型/重量；斜切校验 `OD/tan(90°-α) < 管长` 否则拒绝；FLSO 属主管长扣法兰厚度（最小边）；属主规格在 `/*LIMBOSPEC` 下→拒绝并提示 reinstate；`LSTU` 继承序=head→HSTU OF BRAN、tail→LSTU OF PREV（缺则 HSTU）；重量 `desp[5] = 0.5 × uwei × fitlen`（uwei 取 LSTU 的 `cmpref.uwei`，RPRO 表达式面需解属性引用）。
56. **焊件批量属性编辑**：`weldList` 批改 `WELDTYPE`（SHOP/FIELD/SITE → `SHOP` 布尔）与 Fab.Type（`MANU`/`MACH`/`ORBI`）；焊缝规格来源可切 `fromSpec`/`fromConfig`（`/WELDGAP` 档）。

## Implementation Decisions

1. **新 workspace 成员 crate `pipe-model`**（与 `python`、`tools/aios-patch` 同级，`default-members` 不变）。crate 内部按域分模块：pipeline（A+B）、component（C）、tapping（D）、splitmerge（E）、specchange（F）、slope（G）、weld（H）、fabrication（I）、loader（J）、settings（J）；对外只暴露服务 trait + 输入/输出类型。
2. **写库出口唯一**：服务不直接写 SurrealDB。每个操作返回 `MutationBatch`（元素 `NEW`/`DELETE`/`RENAME` + 属性行写入 + 成员关系 `INCLUDE`/`REMOVE`），形状与 `e3d_collector` 落库行同构；由调用方（web_service 或后续编辑会话层）走现有 `persist_writer` lane 提交。这样既复用水位/事务/冲突语义，也让"编辑产生的行"与"会话摄入的行"在下游不可区分——交付单元重算、联动边、房间归属全部沿用现机制。
3. **撤销边界**：`MutationBatch` 即原子单位；服务层另暴露 `undo_group(name) → [batches]` 组合器，对应 E3D `undo.add/endUndoable`。跨操作回滚（如 excess 警告整批滚）是显式 API，不靠客户端拼。
4. **上下文对象**：调用传入 `Context { element, dbnum, session }` 等价 CE；服务内部维护 head/tail/flow 等工作态，返回结果带新 CE。E3D 的 `Forwards/Backwards MODE` 全局遍历方向在 crate 内显式建模为参数，不进全局态。
5. **规格解析器（最大黑盒）单独立项模块 `spec_resolver`**：E3D 侧 `SELECT WITH <CHOICE>`/`RESELECT WITH SPEC`/`sel hstu` 全在原生层；本 crate 定义 resolver trait + 最小实现（SPCO 按 SKEY/答卷串查 cata 记录、PTSE/GMSE 取 P 点/几何集、bore 表过滤），接口签名与调用点固定，实现可迭代加深。
6. **连接语义忠实度**：CONNECT 失败的降级路径（几何兜底、FCONN+minTube 占位）保留为显式返回 `ConnectOutcome::{Connected, Degraded(Geometric), Forced{gap}}`，不做静默降级——E3D 静默，我们显式。
7. **分段算法翻译基线**：`PFEVALUATESPOOLMEMBER.evaluate` 与 `pfGetTubeParts` 的 PML 行级伪码已在逆向文档 §15.9/§15.10，翻译时以行为单位对照；托管 `PFPipe.LoadSpools`/`PFPipeSpool.LoadPipePieces`/`ValidatePipePiece`（csharp-code）为精确参照，与 PML 版冲突处以托管版为准。
8. **机器/设置默认值**：`initializeFabrication` 的默认机表与 `PIPINGSETTINGS.loadDefaultValues` 全表作为 crate 内 `const`/配置结构落地，字段名保留 E3D 名（FMBEND/FMBDIM/FMSSBK/FMWCON…）。
9. **错误分级**：服务错误类型四级——`NeedsConfirmation`（UI confirm）、`RequiresChoice`（YES/NO 分支）、`Blocking`（本操作终止）、`Raised`（可冒泡到调用方批处理）——与 E3D alert.confirm/question/error/`error <expr>` 一一对应。
10. **静默容错点位清单**：E3D 中刻意静默的路径（规格 dbRef 化失败跳过、连接后属性回读失败、级联收集为空）在服务里全部转为结构化 warning 字段返回，不吞。
11. **阶段交付**（单 spec、body 内分阶段）：A+B（管/支管/端/连接）→ C（元件）→ D+E（开孔/分合）→ F+G+H（规格变更/斜管/焊接）→ I（制造分段校验）→ J（导入/设置）。每阶段独立可验收，crate 内模块边界即阶段边界。

### 接口草图（决策精度用，非最终实现）

核心类型形状——这里编码的是"契约决策"，实现时可改名不改义：

```rust
/// 调用方传入的编辑上下文，等价 E3D 的 CE + 工作态。
/// flow = Forwards/Backwards 遍历方向（E3D 全局 MODE 的显式化）；
/// end 指定头/尾端操作面。
struct Context { element: Refno, dbnum: u32, flow: Flow, end: Option<BranchEnd> }

/// 服务唯一产出物：与 e3d_collector 落库行同构的变更批。
/// 每个 op 一个命名撤销边界；batches 内顺序即提交顺序。
struct MutationBatch {
    name: String,                       // undo 组名（等价 UNDOABLE description）
    ops: Vec<MutationOp>,               // Create{noun,under,attrs} / Delete / Rename{cascade}
                                      // / SetAttrs / Include{into,order} / Remove
    requires_confirm: Vec<Confirm>,     // 提交前需 UI 确认的分支（四级错误语义入口）
}

/// 连接结果显式分档——E3D 静默降级，本服务显式返回。
enum ConnectOutcome {
    Connected,                          // CONNECT 成功，附回读的 HCON/HBOR 等引擎推导值
    Degraded(GeometricFallback),        // (42,94) 路径：pos/dir/bore/conn 几何对齐
    Forced { min_tube_reserved: bool }, // FCONN + MOVE minTubeLength 占位
}

/// 元件创建/重选结果：答卷串与最终 SPCO 分离返回，供 UI 回放或记忆。
struct ComponentOutcome { element: Refno, spref: Refno, spco: Option<Refno>,
                          choice_answers: Option<String>, warnings: Vec<PipeWarning> }

/// 制造校验状态机——Release 门控只认 ValidWithWarning（E3D 侧要求全选管均为该档才可放行）。
enum FabStatus { Invalid(Vec<FabMessage>), ValidWithWarning(Vec<FabMessage>), Valid, Released }

/// 错误四级（对应 alert.confirm / alert.question / alert.error / error raise）。
enum PipeError { NeedsConfirmation(..), RequiresChoice(..), Blocking(..), Raised(..) }
```

不变量（跨全部阶段守护）：写库前必过写权限与属主层级校验；`LHEAD/LTAIL` 仅由端编辑写序置位；连接成功后两端 HCON/TCON 必须互写一致（resolveConnections 语义）；分段后成员总数守恒；`MutationBatch` 内任何 op 失败整批不提交。

## Testing Decisions

- **唯一接缝**：`pipe-model` crate 的服务 API——操作进、`MutationBatch`+结构化结果出。不打 crate 内部函数、不测 SurrealDB 提交（那是 persist_writer 的既有测试面）。
- **夹具**：`mem://` 或临时 SurrealDB 实例上铺 `pe`/`ATT_*` 行构造 PIPE→BRAN→元件树（用 `data_interface::ele_data::EleData` 同源结构）；规格侧用固定 cata 行夹具。
- **黄金断言**：每个操作用例断言产出行集合与 E3D 语义等价——例如"建管"用例断言 PIPE/BRAN 两元素 + HBOR/TBOR/HCON/HDIR/HPOS/LHEAD 行齐；"连接"用例断言双端 HCON/TCON 互写、GASK+FLAN 创建顺序、"法兰端自动补件"ltlen 边界；"分段"用例断言成员归属、-Split(n) 命名、双侧连接恢复。
- **判定标准**：E3D TTY 会话对夹具（`tests/db8000_session_pairs` 既有约定）——同一操作在真 E3D 的 DB 前/后快照差分 = 本服务 MutationBatch 渲染后的行差分。分段/校验算法另加逐行对拍用例（输入同一支管成员表，输出边界集合与 PML 伪码逐步断言一致）。
- **既有先例**：`docs/specs/*` 与 `tests/` 下"红钉契约测试"惯例——先写操作契约钉（行为红→绿），再填实现；纯函数真值表（边界谓词、off-leg 计算、连接型对齐四种方位组合）单独成表测试。
- **不测**：UI 面板、EDG 拾取、AID 标记（UI spec 的范围）；ROUTER 自动路由（本期 Out of Scope）。

## Out of Scope

- OpenCADStudio UI（面板、拾取包调度、标记/高亮、命令行回显）——**独立 spec，画布定稿后发**。
- `ROUTER`/`pipeSimpleAutoRoute` 自动布管内核——E3D 也是原生黑盒，本期只留服务接口空槽；接口契约已定稿（证据文档 §18.11：`route(branch, viaPoints, config{direction,prspace,prrounding,clash}) → BSTAT 0-10`）。
- 规格库/目录库（CATA/SPECO/答案串 schema）的**创建与维护**——resolver 只读它们。
- 等轴图输出（isometrics/SPLDRG）、NC/DSTV 文件输出、PipingTables 材料表生成。
- `pipeeasyeditor`、`gphreppipeedit` 报表式改管、`spl*` 老 spooler 界面——次要入口，后端服务就绪后按需补。
- 把变更写回 E3D `.bdb/.sys` 会话文件格式（同步出口是另一条链路）。

### 开放问题（实现前需拍板）

1. **伪属性供给**：E3D 侧 `:PFCutLength`、制造状态等伪属性由 Core3D `DB_Pseudo*` 注册。本服务要么自算等价物，要么在 `pe`/`ATT_*` 上设扩展表——倾向后者（显式列，好对拍），待评审确认。
2. **COCODATA 连接表**：法兰自动补件依赖目录连接表（按连接型取兼容件清单）。cata 侧对应物是连接型目录记录，实现前需确认 gen-model 已摄入该数据面；若未摄入，补件功能降级为"报告缺件建议"而非自动创建。
3. **答卷串格式**：PML 侧选择链已摸清（证据文档 §18.6）：元件存 `questions`/`answers` 数组（PBOR0/PBOR1/STYPE…），三级回落 = 表直查→PBOR 槽位改写→SELE 树默认路径（`default`/`tdefault`/`EQUA` 链）。编码已破解（证据文档 §18.8）：`CHOICE WITH <spco>` 求值返回 `SPEC /名 Q A Q A…`；PBOR 答案词首字符即孔位号（`PBOR 0100`→键 `PBOR0`+值 100）；`CHOICE QUESTION/ANSWER/MAXANS i` 为 CE 作用域伪表达式；answers 源自 SELE 节点 `tAnswer`/`answer`+`owner.question&qualifier`，**不落库**——resolver 把 questions/answers 作调用参数即可，命中后可按 questionIndex 规则自导出答卷串回显。
4. **LIMBOSPEC**：非当前规格元件的限制策略（UI 层 warning+失活）后端落地为"重选需带规格白名单"，白名单来源（项目 piping specs 集合）需确认数据面可用性。
5. **refno 分配**：新建元素的 refno 生成策略——E3D 由库分配；本仓 `pe` 行沿用解析期 refno 形态，编辑期新元素 refno 空间需定规（建议独立段并在行上标 source=edit，避免与摄入行撞号；与"行同构"约束不冲突——形态同构、号段隔离）。

## Further Notes

- gen-model 现架构是"摄入→生成"单向；本 spec 首次引入"编辑→落库"方向。`MutationBatch` 与 `e3d_collector` 行同构是关键约束：不许出现只有编辑器才认得的行形态。
- E3D 语义中所有 `handle ANY` 静默点、`handle (<code>)` 专用降级路径、`golabel` 重试循环（如 -Split(n) 命名），翻译时保留语义但改为显式返回值——这些是逆向文档里已逐行标注的点。
- `pipeloader` 的 :421 行 errors/SUCCESS 反直觉判定已记录在文档 §18.3，实现时按行为复刻（以 E3D 实测结果为准），不照抄注释。
- 相关文档：`docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md`（行为断言唯一索引，本仓快照）；仓外 `E:\reverse\e3d` 的 docs 目录有架构层结论报告，csharp-code 目录有分段/校验的托管层精确参照。
