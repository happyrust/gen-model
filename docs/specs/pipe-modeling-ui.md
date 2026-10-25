# 管道建模 UI 规格（OpenCADStudio 前端）

状态：草稿（待评审）
日期：2026-10-19
后端：`docs/specs/pipe-modeling.md`（pipe-model crate 服务 API，本 spec 只消费该接口）
视觉契约：`E:\reverse\e3d\docs\2026-10-15_e3d-pipe-modeling.pen`（仓外逆向库；15 屏 + 四区网格 + 图例，导出图在其 `docs/shots/`）
行为依据：`docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md`（本仓快照）§16 交互机制、§17 子界面/操作步骤、§18 数据对象契约、§19 属性总表
实现仓库：OpenCADStudio（本文件只定义需求与接缝，不规定其内部模块划分）

## Problem Statement

后端 spec 解决了"编辑语义与落库行"，但没有任何前端能发起这些操作。E3D 的管道建模界面不只是表单——它有拾取包调度（EDGPACKET）、编号标记层（AID）、位置控件往返、表驱动任务状态机（setTask+warning-Done 重试）、四级严重度消息、与 CE/流程方向联动的视图高亮。需要一份 UI 规格，使 OpenCADStudio 能以 E3D 等价交互把 `pipe-model` 服务能力暴露给工程师。

## Solution

按画布（.pen）四区结构实现：

- **工作台骨架**：Piping Ribbon（Create/Modify/Route/Fabrication/Tools 五组真实命令）+ Design Explorer 树（含 FABRICATION 区）+ 3D 视口 + 停靠面板 + 命令行 + 状态栏（CE/MDB/单位/遍历方向/拾取态/Undo 计数/PIPINGSETTINGS 摘要）。
- **交互内核**（本 spec 的核心，非装饰层）：拾取包调度器、TUBING 过滤、选择=CE 单源、AID 编号标记层、PositionControl 拾取回填、任务状态机、四级消息、enhance/高亮。
- **子表单**：14 个面板（屏 02–15），全部是"收集参数 → 调用 pipe-model 服务 → 渲染结果/定位数据 → 高亮/标记"的薄壳；**UI 不直接写库，也不含建模算法**。

UI 对服务返回的要求已由后端 spec 保证：每个操作返回 `MutationBatch`+受影响元素集合+定位数据（端点 pos/dir/bore、管段中心点），UI 只负责画出来。

## User Stories

### A. 工作台骨架

1. 作为工程师，我打开"管道建模"工作台后看到 Piping Ribbon 五组命令、左侧元素树、中央视口、右侧停靠面板、底部命令行与状态栏——与画布屏 01 一致。
2. 元素树展示 WORL→SITE→ZONE→PIPE→BRAN→元件与 FABRICATION 区（FMGRP/PSPOOL/PPIECE）；树选择即设置 CE，状态栏同步显示 `CE <name> <type>`。
3. 命令行显示最近执行的等价 DB 命令序列（`NEW PIPE`、`PSPEC`、`CONNECT PH TO …`），只读回显不解析输入（解析为后续项）。
4. 状态栏常显：CE、MDB/DB、单位制、遍历方向（FORWARDS/BACKWARDS）、当前拾取模式+Esc 提示、Undo/Redo 计数、PIPINGSETTINGS 摘要（snap 2×OD·minTube 100mm·autoCreate）。

### B. 交互内核（复刻 E3D 机制，均为验收点）

5. **拾取包**：UI 发起拾取时注册"拾取包"（元素类型过滤、提示串、回调目标、Esc 语义）；拾取期间源面板失活，拾取结束/取消后复活——等价 `EDGPACKET`+`edgCntrl`+`.close` 回调。
6. **拾取归一化**：拾到 TUBI 管段自动解到宿主元件（IL TUB 语义）；拾到 P 点（TEE/OLET/NOZZ 等的 `PPOINT`）在目标属性为"连接点"时直接转连接候选并弹确认——定位拾取与连接拾取走同一通道。
7. **AID 标记层**：给元素/P 点画编号圆点、箭头、文本标签，每个标记有独立号可单独清除；与"选择/增强高亮"是分开的视觉层。
8. **高亮/导航**：选中行/元素 → UNENHANCE ALL → ENHANCE 目标 → 可选 zoom 到体积；与拾取包挂起态正确互斥（拾取中不刷新视图）。
9. **PositionControl**：E/N/U 三轴控件 + 单位 + 拾取回填按钮；填入控件→回读→重组 POSITION 是统一路径（拾取值先经控件舍入/吸附再走服务）；防科学计数法显示（P-16651 语义）。
10. **任务状态机**：面板内 task 表驱动（hide 全部→按名 show）；上下文不合法时切 warning 态，Done 按钮绑定重试闭包回到失败点——等价 `setTask`/`warningDone.callback`/`resumeTask`。
11. **消息四级**：confirm（可继续）/question（YES/NO 分支）/error（阻断）/info；对应服务错误四级 `NeedsConfirmation/RequiresChoice/Blocking/Raised`。
12. **遍历方向**：FORWARDS/BACKWARDS 是显式会话态，影响连接语义、插入顺序、元件默认朝向；切换写状态栏并作用于后续操作上下文。

### C. 建管（屏 01 右侧 Pipe Editor · Create Pipe task）

13. 填名→规格级联→Apply：`namePipe` 校验激活 Apply；成功创建后**自动转到 Edit Branch task**（E3D 真实流程），并把新支管行标记。
14. Head/Tail 区字段：BOR/CON/DIR 下拉 + PositionControl + HSTU 清单；写入顺序由后端保证（xBOR→xPOSITION→xDIRECTION→xCONNECTION→Lx=TRUE），UI 按返回结果回读显示。
15. Connect End task：选端行（HEAD/TAIL/Pn）→ 拾取目标（接管嘴/元件/支管）→ 结果表显示实际 HCON/HBOR/HREF（引擎推导回读，不信输入）。
16. 连接失败显示降级结果：几何兜底（pos/dir/bore 已写）或 FCONN+minTube 占位，提示可 [Done] 重试。

### D. Component Editor（屏 02）

17. Create/Modify/Hull Routing 三模式 + With/Against Flow；调色板 14 主元件+附加列表（与屏 02 集合一致）。
18. Select 区：规格/SType/BOR(PBOR1)/ANGL 即"答卷"字段；SPCO 结果表点击行预览；答卷改写规则（PBOR0→PBOR1 归一、TEE arrive=3 时 answers[2] 同步）由后端 resolver 决定，UI 展示最终选择链。
19. 三种定位：Connect（接前驱）/At Position（TUBI 精确投影，跨管投影回本支）/3D Position（轴线求交 + snapDistance 内自动吸附，吸附事件明示）。
20. Orientation/Position Through 区：Thru 七键、LDIR/LSTU、PositionControl；逆流+孔径不等时展示 arrive/leave 已交换。
21. Modify 链：拾 TUBI 反解宿主；非当前规格元件进 LIMBOSPEC 提示（confirm 后重选）；多孔径件 arrive=2/1 单选互锁；flip 三态（FLIP 单件 / ORI / CREATE 成组镜像反转）按返回结果重绘。

### E. 分合（屏 06）

22. SEGMENT/FEATURE/PLANE/MERGE 四模式；SEGMENT 起点包→终点包双拾取，Esc 即提交 split；FEATURE 模式另需选去向（moveTo：新复制管 / 同管新支管——`splitpipe` 的 PIPE|BRANCH 两路，§18.17.1）。
23. 预览画 cut 线（切点在元件到侧，切点元件归新支管作首成员）+"成员原序迁入"注解；结果展示 `-Split(n)` 名（剥旧后缀+撞名自增）、双侧 CONN 恢复、法兰 return-ref 互换结果。
24. FEATURE 模式切点处可插装配（buildInTube 语义）并作为锚点；MERGE 源管清空即级联删。

### F. 规格/孔径变更（屏 07 + 屏 11）

25. 四模式（Spec/Bore/Insulation/Tracing）；受影响元件表 Old→New SPCO 三态着色（可改/不可改/已改+原因）。
26. `!!selectSpecification` 子表单：规格列表+类型列表+questions/answers 答卷表+解析链可视化（表直查→TEE 兜底→SELE 遍历→default/tdefault/EQUA→SPCO）。
27. 保温/伴热走旁路提示（级联写 ISPEC/TSPEC 同值集，不经问卷）。

### G. Slope（屏 08）

28. Legs 表（From→To/Len/Δ°）选中行高亮视口对应腿；AID 预览（0.8×OD 圆柱+关节球+斜率箭+锚点双箭）随输入即时重画，重画前先清 aidNo。
29. Undo/Redo 为表单态快照双栈（**不是** DB undo），与后端事务语义分开；Apply 失败消息走 `!!pipeSlopeMessageForm` 等价消息表。

### H. Weld Handling（屏 05 + 屏 13）

30. 焊缝表（#/Weld/Between/Shop/Gap/SType/Spec/Status）；行选→树/视口双导航；CHOOSE SPEC 命令串与降级（无 SHOP 条件重试、/WELDGAP+实测 gap）在"Command emitted"区回显。
31. 超规格焊缝统计（spec of spref ≠ pspec of pipe）alert；手工焊 wldNumber=0 单列；非 upDate 模式触发整批 renumber。
32. `!!pfModifyWelds`：纯参数收集弹窗，提交回父表单的 UNDOABLE 批（自身不写库）。

### I. Fabrication（屏 03/09/10/15）

33. Fab Check：PIPE→PSPOOL→PPIECE 树表（CutLen/Feed/Mach/PPFMCV）；纯斜接片高亮母弯管+段中心 AID TEXT；PIPCA 随父高亮。
34. 消息区（W/E/I）点击行 → 高亮对应元件集合+中心点（托管 renderer 等价）；Release 只在全选管 ValidWithWarning 时可用，执行走 UNDOABLE。
35. Machine Manager：FM* 树 + 字段页 + FMBDIM/FMWCON 表（三机型维度）。
36. Spooler：ISODEP→ISOREG→SPLDRG 树 + SPOOL 橙/FIELD 绿双色 tag + member 行；Pick mode/写权限门控；`!!pfSpoolDrwgMemberCreate` 入口。
37. NC Output：机器/范围/格式/输出文件 + 选项（SSBK/FMBPLN/RELES）+ 预览块；validatePipePiece 未过禁止导出。

### J. 导入与设置（屏 12）

38. Data Import：文件/Sheet/表头校验；行级表（Action NEW/UPDATE/CONNECT、nameCheck/ownerCheck/typeCheck/boreCheck、CONNECT 行连喷嘴 P1）；行态 SUCCESS/ERROR+错误串。
39. 保留 E3D :421 行"错误文本非空仍标 SUCCESS"反直觉行为为可配置开关，默认按 E3D 复刻并标注。
40. PIPINGSETTINGS 默认值面（snapDist/minTube/auto*/isoMode…）全局+用户两档。

### K. 制造断点与管端编辑（§18.16 `pfModifySpool`/`pfFabricationPipeEnd` 取证）

41. **Modify Spool 面板**：断点模式双选（Weld / No-weld）→ EDG 双拾取元件（`TYPE of OWNER of OWNER eq 'PIPE'` 过滤、Esc 退出）→ 画断点面（AID plane 按 spool 循环色 + 标志断点画标记）→ confirm "OK to add/remove?" → 提交一个 UNDOABLE 批；`replaceSplitType` 替换流把 confirm 文案换成"replace weld/no-weld?"变体。
42. **断点显示态**：Show Spool Limits 切换全管断点可视化——Weld 断点画平面、No-weld 断点画标志面（TEE/OLET 的 cref 侧单独面）；双色 tag（SPOOL 橙/FIELD 绿）与 §I.36 同源。
43. **End/Feed Excess**：Show End/Feed Excess 切 AID TEXT 标注（piece 首/末件位）；Set End Excess / Set Feed Excess 弹数值框（`lExcess`/`aExcess`/`usefed` chip），`validateExcessValue` 非法值归零；`loose` 标志件与默认散件余量联动显示。
44. **Fabrication Pipe End 面板**（`pfFabricationPipeEnd`）：螺纹/斜切管端参数收集——head/tail 选端、angle（斜切角）、clearance、refPlane 拾取、threadInfo（型@述）、note（含 `{deg}` 占位替换）；提交回宿主批；斜切角显示走 AID 弧线+角度文本（`displaySlantedAngle`）。
45. **Modify Welds 细粒度**（`pfModifyWelds`）：从焊缝表行选得 `weldList`→批收集（Weld Type `SHOP/FIELD/SITE`、Fab.Type `MANU/MACH/ORBI`、来源 `Spec./Config.` 单选）→ 提交回父表单批；弹窗自身不写库（与决策 8 子表单协议一致）。

## Implementation Decisions

1. **选择/上下文单源**：CE 是全局会话态（元素+dbnum+flow+end）；树、视口拾取、面板结果行三路都写同一个 CE；UI 调服务时把它装进 `Context` 参数。E3D 的 CE/MODE 全局态在本前端也是显式 store，不散落在面板里。
2. **拾取包调度器**为独立 UI 服务：注册{过滤词法（元素类型/属主类型/TUBING）、提示、目标回调、Esc 语义、挂起视图刷新}；一次只允许一个活跃包；拾取进行中面板只读。E3D 的字符串回调 `!!form.method(ret)` 在本前端为具名回调引用。
3. **位置控件规范**：所有"写坐标"的字段只经由 PositionControl 组件（E/N/U+单位+拾取钮），拾取值经控件格式/舍入后再交服务；保证 `distance`/`real` 转换一致（P-16651 语义），UI 不各自拼串。
4. **AID/高亮分层**：`aid` 层（编号圆点/箭头/文本）与 `enhance` 层（选择高亮）独立；每个标记独立号、可点名清；AID 原语不参与拾取命中。
5. **任务状态机**为面板通用基类：`setTask <name>` 驱动 show/hide，`warning(task,callback)` 提供 Done=重试闭包、Back=resume 原态；上下文守卫在进 task 前跑。
6. **四级消息映射**：`NeedsConfirmation`→confirm 条、`RequiresChoice`→YES/NO、`Blocking`→error 条、error `Raised`→全局错误区+命令行红字；UI 不降级错误语义。
7. **撤销边界对齐服务**：UI 的"一次应用"= 后端一个 `MutationBatch`；表单态 undo（如 Slope 参数列表、排列草案）用 presenter 快照双栈，标注为"表单态回滚≠DB undo"；批量操作失败由服务整批不提交，UI 显示 requires_confirm 明细再决定是否重发。
8. **子表单协议**：E3D 的全局单例+直写成员传参（`!!pipeTappings.branchInsert` 等）在本前端改为**显式参数 props**；弹窗禁止自己写库，提交回宿主面板的 UNDOABLE 批（pfModifyWelds 模式）。
9. **属性 chip 标注**：画布每个字段右侧的属性 chip（HBOR/HCON/SPCO/PPCUTL…）保留为开发版式常驻 hint——UI 即"字段→属性映射表"，评审与逆向对照都靠它。
10. **命名/命令回显**：底部命令行区显示后端等价命令序列（服务返回 `display_commands` 时原样渲染）；行尾 `|` 光标提示"只读回显"，直到命令解析功能立项。
11. **画布即契约**：15 屏是验收图——屏间距/分组/入口标注/图例色板是最终版式基准；实现时截图比对，不因技术栈改布局。

## Testing Decisions

- **接缝**：UI↔服务的唯一边是 `pipe-model` API；UI 测试用 mock 服务返回固定 `MutationBatch`/结果对象，断言面板把行/定位数据画对、错误四级走对分支、命令串原样回显。
- **交互内核单测**：拾取包注册/抢占/取消、TUBING 过滤、P 点分支转连接、AID 编号分配/清除、setTask/warning-Done 状态迁移表——纯状态机测，不开 3D。
- **黄金对拍（UI 侧）**：给出 E3D 会话等价的返回对象，断言 UI 显示值与后端语义一致（HCON/HBOR 回读、降级标记、ValidWithWarning 门控、超规格焊缝计数）。
- **视觉验收**：逐屏对照 .pen 画布（含分区横幅/入口标注/图例），用截图清单而非逐像素比对。
- **不测**：后端算法正确性（后端 spec 的面）；3D 渲染/相机；命令行解析（本期不实现）。

## Out of Scope

- `pipe-model` 服务实现本身（后端 spec）。
- ROUTER/XROUTER 自动布管交互（"Route Pipe"按钮占位 disabled 标注）。
- 等轴图查看器、材料表、应力接口面板。
- `cdcompalp`（流向变更）——E3D 源里未定位到表单文件，待定运行时注册路径后再立项。
- 命令行解析执行（CLI 模式）、宏/脚本录制。
- plant3d-web 既有查看器功能（只读校审界面）的本体改造——本工作台在其内嵌为独立 dock 区，与校审树共用 3D 视图但操作面独立。

### 开放问题

1. **拾取包与校审界面共享视口**：plan3d-web 的 3D 视图已有选区/批注语义，建模拾取包进入时独占鼠标路径或叠加，需定仲裁规则（建议：建模会话激活时拾取包独占，退出归还）。
2. **AID 渲染载体**：现视口用 overlay 层还是 Three.js sprite——决定 aidNo 清除语义实现。
3. **状态栏 CE 与校审树 CE 是否同源**：建议同源但加"建模态"前缀防止语义混淆。
4. **命令串回显格式**：服务是否需要返回 display_commands 字段（E3D 的 `q ev` 等价回放），或 UI 只展示结构化摘要。
5. **EdgPosition/EdgPick 包优先级**：同一区域多个可拾取目标时的排序策略（E3D 有 priority 字段），实现前确认服务返回是否携带可选优先级。

## Further Notes

- E3D 在拾取、连接、规格选择的"静默容错"点已在逆向文档逐行标注；UI 侧全部转为消息/提示条目，不吞。
- `feedback`/`branConnect`/`setTask` 等字符串回调在 E3D 是运行时绑定，本前端用显式回调类型替代，避免字符串求值。
- 画布 `.pen` 与截图在仓外逆向库（`E:\reverse\e3d\docs\` + `docs/shots/`），评审时以导出图为标准；脚本 `tools/pen-01…17.js` 可重放。
- 与后端 spec 的对应：本 spec 每条 User Story 都有后端 Story 编号锚点（A/B/C/D/E/F/G/H/I/J/K 分组相同）；评审时按组逐行核对接口参数是否足够 UI 还原。
