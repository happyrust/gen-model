# 会话上下文 — 2026-08-30 · 恢复 cursor 会话 53082496（zhimo 协作组 opus-5-22）

> 本会话：BajieAsk-agent-1-9817ec41。
> 恢复对象：cursor composer `53082496-1254-4485-a4e4-e4e09eed9dd1`（转录 227 条，
> `C:\Users\dpc\.cursor\projects\d-work-plant-code-old-gen-model\agent-transcripts\...jsonl`）。
> 原会话身份：zhimo-mcp 协作组「重构」的成员 **opus-5-22**（zhimo sessionId se-08642ac1708cf5ac），
> 中断前刚被提升为**指挥官**（前任 opus-5-21 离线）。
> ⚠️ 本 Cursor 会话**没有注册 zhimo-mcp 工具**（只有 BajieAsk），无法接回 zhimo_chat 循环 /
> 指挥官职责；能续接的是**工作本身**（两仓工作区状态完好，见下）。

## 任务状态：进行中 — 新任务（13:57 下达）

**任务**：① 审核协作组的实现（e3d-io + gen-model 直读侧）；② 用 ida-bridge 分析 core.dll
的模型生成整个流程；③ 制定完整开发计划，使后续能直接用数据接口驱动模型生成。

### 计划步骤
1. [ ] 审核 e3d-io 实现（index/diff、engine、record/descriptor、provider trait）
2. [ ] 审核 gen-model 直读实现（direct_store / direct_attmap / 探针）
3. [ ] 核查 ida-bridge 服务与既有 core.dll / core3d.dll 资产
4. [ ] ida-bridge 分析 core.dll 模型生成全流程（取数点/语义）
5. [ ] 覆盖矩阵：生成链数据接口需求 vs 现有直读实现
6. [ ] 产出完整开发计划（docs/plans/）
7. [ ] 汇报

### ida-bridge 环境（继承 08-29 档案，已核验过的事实）
- CLI：`ida-bridge`（on PATH，checkout `C:\Users\dpc\.agents\tools\ida-bridge` 的 .venv）
- IDA 9.2 @ `D:\IDA Professional 9.2`；headless idalib 用系统 Python312（`IDA_BRIDGE_IDALIB_PYTHON`）
- Windows 移植注意：CLI venv 与系统 Python 拆分，**勿往系统 Python 装 websockets/ida-bridge**
- 上一会话（cbc559f4）记录 server 曾运行（PID 48180）、`.ida_scratch` 存量丰富

### ida-bridge 现状（本会话 13:5x 核实）
- server 运行中（PID 48180）；已连接 13 个 idalib 客户端，关键：
  - `idalib-48392` → `D:\AVEVA\Everything3D3.1\core.dll.i64`（**主目标**）
  - `idalib-32268` → `D:\AVEVA\Everything3D3.1\Core3D.dll.i64`
  - `idalib-32872` → `D:\AVEVA\Everything3D2.10\Core3D.dll.i64`
  - 另有 libgm/libgeom（3.1 与 2.10）各一
- `.ida_scratch/` 有 7 月做的大量资产：`_routines_core3d.json`(87K)、`_core3d_funcs.json`、
  `_imports_core3d-retrace.json`(747K)、`out_dbelem*/out_attr*/out_noun*`、`e3d_dbelem_api.txt`(35K) 等
- 用法：`ida-bridge exec <client_id> --sql "..."`（优先 SQL）；写 IDAPython 前先加载 ida-docs skill

## 审核结论（阶段一，14:0x）

### 已读文档
- `docs/plans/direct-mode-model-generation.md`（ADR-053 落地，P0 完成、P1-P4 待办；基于 pdms-io）
- `docs/plans/direct-dbelement-read-api.md`（D0-D6 门面，DirectMdb/DirectDb/DbElement，基于 pdms-io-v2）
- `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md`（**已落地路线**：e3d-io 五层重写 P-1~P4 全绿；§9 路线抉择推荐 C 分层收口）
- `docs/plans/2026-08-30-e3d-io-gen-model-gap.md`（**关键**：G1-G13 能力缺口体检，全部真跑数字）

### 关键判断：两条计划轨道，实现已切到 e3d-io
- 老计划（direct-mode/direct-dbelement）基于 `old-pdms-io`/`pdmsdb_engine_v2`，接口面 D0-D6 是**设计蓝图**
- 实际交付走的是 **`e3d-io`**（core.dll 同构重写，L0-L4 完成，429 库四道硬门全绿）+ gen-model 侧
  `DirectStore`/`direct_attmap` 接线（t-354）
- 两套文档的「接口面清单」（get_named_attmap 等收口函数、DbElement 读侧方法）仍是有效需求规格

### 代码审核 — direct_store.rs（合格，设计扎实）
- 三处权威单一来源（时点/ref0 归属/属性形状）划分清晰，注释即规格
- 并发形态正确：`DashMap<dbnum, Arc<Mutex<DbSession>>>`，锁会话前先放分片锁（避免死锁），
  文件 I/O 在 DashMap 外完成
- 时点语义严谨：DESI 库 pin `applied_sesno`；CATA 库 `sesno=None`=开库解一次冻住（对齐
  DB 侧 OnDemandDbSession），并有 `pinned_sesno` 读回、FileIdentity 守卫（文件被换→报错）
- fail-loud 到位：NotPinned/UnresolvedRef0/NoFileForDbnum/FileReplaced 各有其类，测试锁死语义
- 唯一环境耦合：`TEMPLATE_DIR_DEFAULT` 硬编 `E:\reverse\e3d\shadow_e3d31_aps_all`（P2/P4 才进
  DbOption，当前用环境变量兜底，可接受）

### 代码审核 — direct_attmap.rs（合格，形状权威取 DB schema）
- 立场正确：形状权威 = DB 读侧 `named_attr_info_map[noun][key].default_val`，coerce 转不过去
  不猜、记 shape_conflicts；REFNO/OWNER/TYPE/SESNO 特判与 DB 同序
- P0 四类残差处置齐全（词反哈希/DB 读损耗键保留/历史缺键保留/SESNO 不产出）
- 侧通道设计好：outside_schema/unset/shape_conflicts/view_divergence/undecoded 是回执不是日志
- **已知缺口 BANG**（唯一硬伤）：e3d-io 描述符层把角度属性定型成 Word，实为 i32 百分度，
  属 e3d-io record 层缺口（不在 gen-model 写入面）；转换器正确地记 shape_conflict 不猜

### e3d-io 侧缺口（来自 gap 文档，已被 t-354 部分闭合）
- G1 跨库引用：DirectStore 侧解（已做 pin_from_locator）✅
- G2 会话 pin：ReadOnlyEngine::open_at 已加（t-354 交付）✅
- G3 children 顺序：0.26% 非 refno 序，转换器须用原序不排序（**待接线时遵守**）
- G4 表达式求值：**未解**——渲染器有但分派器私有、只出显示文本不求值、方言与现有 eval 不同（目录几何硬阻塞）
- G8 反向引用：无反向索引（视用法）
- BANG（G/descriptor 定型）：e3d-io record 层缺口，未修

### 待办（本任务重点）
- core.dll 模型生成**全流程**（gap 文档只覆盖了「读」，没覆盖「生成算法怎么把数据变几何」）
- 产出覆盖矩阵 + 完整开发计划

## core.dll / Core3D 生成流程（本会话 14:xx，活体 3.1 二进制核实）

### GMDRAW 跳过的 5 个 noun 已解码
`PNOD`/`SNOD`（管道节点）、`JLDATU`/`PLDATU`/`ENDATU`（J 线/P 线/端点基准点）——全是**非几何的连接/基准点**，无可见几何，GMDRAW 跳过合理。闭合 teach/0009 §六 一条遗留项。

### 活体 Core3D.dll（idalib-32268，E3D 3.1）与 teach 记录逐一吻合——目录几何路径
- `CSG_TreeBuilder::getCSGTree(DB_Element&, CSG_TreeBuilderOptions&, int&, D3_Transform&)` @ `0x10715b30`（设计元件顶层入口）
- `CSG_BaseCSGTree::getCSGTree` @ `0x10715a60`
- 分派：`CSG_BasicPrimitive::findPrimitive(DB_Noun*)` @ `0x107266f0`、`addBasicPrimitive` @ `0x107260c0`、`found(DB_Noun*)` @ `0x10726730`
- 逐 noun 图元：`CSG_Basic{BOX,CON,CTO,CYL,DIS,EXT,POL,PYR,REV,RTO,RUL,SLC,SNO}::getPrimGeom(DB_Element&)` @ `0x10726a90`+
- 轮廓装配：`getProfileFromDB(DB_Element&, DB_Element&, D3_Transform&, D2_Profile*)` @ `0x10410b30`
- 开孔：`CSG_PrimitiveUtilities::addHolesBelowPrimitive` @ `0x10726150`、`addHolesBelowTemplate` @ `0x107263a0`
- 管道：`MDR_BranchVisualisationManager::getCSGTree` @ `0x105e9aa0`、`MDR_SegmentVisualisationManager::getCSGTree` @ `0x105e9c80`
- 过滤：`CSG_TreeBuilderOptions::isWanted / isPlineWanted`

### 关键架构结论：表达式求值在 core.dll，不在 Core3D
- **Core3D 查 getReal/DesignParam/evaluate/catExpr = 空**——Core3D 只消费**已解析好的数值**，`getPrimGeom(DB_Element&)` 拿到的属性值是 core.dll 已就地求好的。
- **core.dll（idalib-48392）才是求值器**：多态 `DBE_Base::evaluate(DB_Element& context, DBE_* out, MR_Message*)` 家族（DBE = DataBase Expression），带类型化变体 `DBE_Value/DBE_StringValue/DBE_PositionValue/DBE_OrientationValue/DBE_DirectionValue/vector<…>/DB_Attribute/DB_Noun/DB_DateTime/DB_Blob` @ `0x108e966c`–`0x108e96c6`；**context 元素供参**（设计元件供 DESPARAM）。
- PML 桥：`getRealFromPML` @ `0x10aea758`、`getRealFromPMLinDB` @ `0x10ae9f70`、`getRealAndUnitsFromPML` @ `0x10aea794`。
- 智能文本：`EvaluateIntelligentText::getEvaluatedIntelligentText / internalEvaluate` @ `0x104ca850`+。
- 集合求值：`DB_Collection::evaluate / evaluateOnElement` @ `0x10ae8b9c`；伪属性实数取值：`DB_PseudoAttPlugger::addGetRealPlug`。

### aios_core 求值侧比对
- `tiny_expr`（tinyexpr 移植）**只是纯算术核**：`+ - * / ^ %`、数学函数、常量、f64 变量；**不认识 PDMS 目录语法**（PARAM/属性引用）。
- 目录 PARAM/属性代入在 `expression::resolve`（`resolve_gms`/`resolve_axis_params`，见 `query_cata.rs`）这层，位于 tiny_expr 之上，吃的是 **pdms-io 序列化出的字符串方言**。

### G4 的实质与两条修法
- core.dll 求 **二进制 DBE 树**（context=设计元件）；aios_core 求 **字符串方言**（pdms-io 产的）。
- **修法 A（字符串对齐，先做）**：e3d-io 把目录表达式属性渲染成**与 pdms-io 完全一致**的字符串方言，gen-model 继续用 resolve+tiny_expr。便宜但脆（须逐 op 对齐）。配差分门：同批目录元素，DB 模式串 vs e3d-io 渲染串逐条并排。
- **修法 B（结构化 DBE，若 A 分歧显著再上）**：e3d-io 暴露 DBE token/树，gen-model 内建小求值器镜像 `DBE_Base::evaluate`（算术叶子复用 tiny_expr）。最忠实、与字符串方言解耦。
- 结论：**A 先做→量化分歧→分歧大再 B**。

## 原会话干了什么（按时间）

### t-327 · e3d-io P3 L2 双根差分 — 已交付、指挥官已批
- 仓：`D:\work\plant-code\old\vendor\e3d-io`（未提交，工作区状态）
- 新增 `src/index/diff.rs`（476 行，IndexDiff/DiffTally/KeyChange/Pruning）+ `tests/index_diff_real.rs`
- 改 `src/index/mod.rs`（挂模块+re-export）、`src/index/cursor.rs`（read_node/located 放宽为 pub(super)）、`tests/index_cursor_real.rs`（源序看守收 diff.rs）
- 验证：429 真库 428 对相邻会话，差分 == 两树全量枚举集合差（17 万变化）；剪枝成本门是**精确式**：读页数 == 两树页集合对称差；206 测试全绿
- 经验：共享 CARGO_TARGET_DIR（D:\Rust\target）多 agent 并发会撞 stale rlib；当时用 `D:\Rust\target-t327` 隔离；**别在 e3d-io 跑仓级 cargo fmt**（会重排历史漂移文件，点名文件格式化）

### t-354 · e3d-io 接线 gen-model（DirectStore + 属性转换器）— 已提交待审（六验收过五）
交付物（全部未提交，工作区状态，已核实在盘）：
1. `gen-model/Cargo.toml`：加 `e3d-io` / `e3d-attlib` **path 依赖**（vendor 并排，注释说明为何不是 git+rev）
2. `e3d-io/src/engine.rs`：`ReadOnlyEngine::open_at(path, sesno)`（走 walk_chain 找会话，open/open_with_cache/open_at 收成 open_selected）
3. `src/data_interface/direct_store.rs`：dbnum 池化引擎、钉 applied_sesno；CATA 库不入水位表→`pin_from_locator` 开库时解一次冻住；`pinned_sesno` 可读回；FileIdentity 守卫（文件被换→FileReplaced 报错阻断）；错误手写 Display（仓约定不用 thiserror）
4. `src/data_interface/direct_attmap.rs`：ElementExtraction→NamedAttrMap（ADR-053 Q4）；**形状权威 = DB schema `named_attr_info_map[noun][key].default_val`**；REFNO/OWNER/TYPE 特判；词属性 db1_dehash；不认识的键入 outside_schema；定不了型入 shape_conflicts/view_divergence 侧通道，不瞎猜
5. `src/bin/direct_attmap_probe.rs`：探针从 pdms_io 移植到 e3d-io 链路，加 --dump-keys、跨库引用验证
- 验证：12 单测全过（含并发）；对拍 dbnum 8000 200 样本（真值冲突全部归因 BANG×44 + CACHID/DESC 各 1）、7333 零真值冲突；直读比 DB 快 ~110×；跨库引用 92 跳进目录库 5052（自动 pin，冻结 sesno 189）
- **卡住的一条验收**：BANG（弯头角度，几何相关）被 e3d-io 描述符层定型成 Word，实际是 i32 百分度（raw −9000 ↔ DB −90.0）；属 `e3d-io/src/record/`（不在 opus-5-22 写入面），已带证据转给 opus-5-20
- 实测认知：**owner 链不跨库**（6605/6605 本库，opus-5-20 独立复测同结论）；跨库走**命名引用**（SPRE/LSTU/PSPE/CATR，82% 指向目录库 5052）——验收⑤原文「owner 上溯跨库」在这套数据上不存在

### t-355 · CATA 侧 direct 接入 — 领错退回
- 池子误派给 opus-5-22；实际归 opus-5-19（他已在做，时点语义 OnDemandDbSession→DabaconSnapshot 已跟到底）；未动一行，complete_task 注明退回后又以指挥官身份 reject 掉

### 中断点（原会话最后状态）
- opus-5-22 刚接任指挥官：组内只剩自己在线；盘面 1 todo（非本组）+ 5 done + 7 in review
- in review 含自己的 t-354（自审有利益冲突，已向用户挂起）+ t-27/28/29（别的工作流残留）
- 正在等用户表态时会话断掉

## 当前盘面（本会话 13:4x 核实）
- e3d-io：M engine.rs / index/{cursor,mod}.rs / tests/index_cursor_real.rs；?? index/diff.rs、tests/index_diff_real.rs（另有他人 examples/*、record_l3 等混在工作区）
- gen-model：M Cargo.toml、data_interface/mod.rs 等大量（多 agent 混合）；?? direct_store.rs、direct_attmap.rs、direct_attmap_probe.rs 均在
- 两仓均未提交；SurrealDB 8009 端口当时活着（未复核）
- 同日相邻 BajieAsk 会话（各自有档案）：`会话-2026-08-30-direct属性直读python绑定.md`（agent-1e0bfaa9，进行中）、`会话-2026-08-30-数据接口覆盖分析.md`（agent-cbc559f4，进行中）

## 新目标（用户 2026-08-30 下午拍板）
**gen-model + e3d-io 直读 .dat 生成模型，不对接 pdms-io。** 先修 BANG 定型缺口，再用 Plannator 出完整开发计划。

## BANG 定型缺口修复 —— ✅ 已完成并验证（本会话）

### 根因
角度属性存单字时是 **i32 百分度**（`SCTN.Bangle` 存 −9000，`q att` 打 −90；`SSLC.Pxbshear` 存 1150 打 11.5）。attlib 对 `BANG` 等若干角度属性**没有 ATGTDF 定义**，`data_type` 定不了型；唯一权威是目录 `UNIT==ANGL`（`db1_hash("ANGL")==773119`）。e3d-io 描述符层不看目录单位 → 落成无符号 `Word{raw:4294958296}`，t-354 对拍 44 条 BANG 真值冲突全部由此而来。

### 修法（角度表烘焙进 e3d-attlib，descriptor 查表缩放）
1. `vendor/e3d-attlib/tools/gen_angle_attrs.py`（新）：从 `e3d-io/catalog/e3d31/noun_attr_fields.json` 抽 `UNIT==773119` 的属性 → 生成静态表。
2. `vendor/e3d-attlib/src/angle_attrs_table.rs`（生成）：**125 个**角度属性 hash，按 hash 排序；`BANG(679457)` 在内。
3. `vendor/e3d-attlib/src/angle_attrs.rs`（新）：`is_angle_attr(hash)` 二分查表 + 5 单测；`lib.rs` re-export。
4. `vendor/e3d-io/src/record/descriptor.rs`：`StorageShape::Word` 标量分支，`data_type` 非 Int/Bool 且 `is_angle_attr(hash)` → `Real(raw as i32 as f64 / 100.0)`；否则维持 Word。+2 单测（−9000→−90.0；同位模式无角度单位仍是 Word）。

### 验证
- e3d-attlib angle_attrs 5 测全绿；e3d-io **167 lib 测全绿**（含新 2 条）。
- 途中一度见 `record::tests` 2 条失败——**瞬时假象**：并行 agent 正在改 `page/mod.rs`/`record/mod.rs`，编译撞上中间态；稍后单跑与全跑均绿。工作区确认多 agent 并发在飞（examples/tests/lib.rs 等文件列表在命令间隙变化），我的 diff 只有 descriptor.rs 的角度分支 +52 行，互不重叠。

### 端到端对拍复跑（16:0x，收口确认 ✅）
`direct_attmap_probe --dbnum 8000 --sample 200`（SurrealDB 8009 活，pin sesno 264）：
- **真值冲突 46 → 2：44 条 BANG 冲突全部消失**。值不匹配键只剩 `{CACHID:1, DESC:1}`——正是 t-354 已知的与 BANG 无关残差（`24384_26250 CACHID: direct="2" db=""`；`24384_24945 DESC: direct="" db="安全壳通风换气系统"`），Phase 1 归因收口。
- 样本含 BEND×15 + ELBO×6（BANG 携带大户）全部干净；200 样本：完全一致 2｜归一后一致 10｜direct 超集 186。
- 性能：direct 143µs/元素 vs DB 16022µs（≈112×）；会话池自动 pin 30 个库。
- 探针退出码 1 是「零冲突门」被那 2 条残差挡住，非 BANG 问题。
- **跑法教训**：隔离 CARGO_TARGET_DIR 首跑会触发 `manifold-csg-sys` build 脚本联网 git clone manifold3d（本次网络断流 early EOF 直接 panic）；**用共享 `D:\Rust\target`（热缓存 1.06s 完成构建）**，t-354 的缓存都在。

## Plannator 计划 —— 已立
- **Plannator 是什么**（本会话查明）：本工作区的计划门禁流程。计划文档 = `.planning/<日期>-<slug>/task_plan.md`（`# Plannator 开发计划:<题>` 格式，范例见 plant-io/rs-mbd）；门禁 = `plannotator annotate <plan> --gate --json`（浏览器批注 UI + Approve 按钮，结构化 JSON 落 `--result-file`）。流程：写计划 → gate 批注 → 按批注修订 → 落盘。
- **计划落点**：`gen-model/.planning/2026-08-30-direct-read-model-generation/task_plan.md`
- **骨架**（详见计划本体）：P0 已有资产盘点（G1/G2/G9/G10/G11 已被 t-354 解掉，BANG 今日修复）→ P1 落盘提交+对拍收口 → P2 生成链数据源切到 DirectStore/NamedAttrMap → P3 G4 目录表达式（对拍先行→方言映射→分歧大再上 DBE 结构求值）→ P4 端到端直读生成试点+双跑门（比序列不比集合，G3）→ P5 批量收口+覆盖矩阵终态。

## 可续接的方向（历史，已被新目标取代）
1. ~~修 BANG 定型缺口~~ ✅ 本会话完成
2. **把 t-327/t-354/BANG 的改动落成提交**（现在全是工作区状态，怕丢）→ 进计划 Phase 1
3. 续接同日另两个进行中的任务（python attmap 绑定 / 数据接口覆盖分析）
4. 其它新指令

## 工作日志
- 13:36 收到恢复指令；BajieAsk 存储无此 ID → 定位到 cursor agent-transcripts jsonl（435KB/227 条）
- 13:4x 分批解析转录还原全程；核实两仓 git 状态与 t-354/t-327 交付文件均在盘；建本档案
- 14:xx ida-bridge 活体核实 core.dll/Core3D 生成流程；解码 GMDRAW 跳过 noun；定 G4 修法 A/B
- 15:xx 用户拍板新目标（直读生成，不接 pdms-io；先修 BANG；Plannator 出计划）
- 15:xx BANG 修复落地（e3d-attlib 角度表 + descriptor 缩放），e3d-io 167 lib 测全绿
- 16:xx 查明 Plannator（.planning/task_plan.md + plannotator gate CLI）；写计划、提交门禁
