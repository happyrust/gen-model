# 增量模型生成（Incremental Model Generation）

本上下文描述：当 E3D/PDMS 设计数据发生增量变化（新增 / 修改 / 删除 / 搬迁）后，如何以**尽量小而正确**的范围重新生成受影响的三维模型，而不是全量重算。术语在此统一，实现见 `src/data_interface/` 与 `src/fast_model/`。

> 本文件仅为词汇表（glossary），不含实现决策与流程；决策见 `docs/adr/`，规格见 `docs/specs/`，计划见 `docs/plans/`。

## Language

**增量模型生成 (Incremental Model Generation)**：
在已有全量模型基础上，仅对本次数据变化影响到的部分重新生成模型，与「全量生成」相对。
_Avoid_: 局部更新、局部刷新

**生成根 (Generation Root)**：
一次增量重算所选定的**根参考号**——从它开始（含其子树）重新生成模型。生成根以 `refno` 为唯一身份，所属数据库由 Ref0 库归属映射得到，`dbnum` 不属于生成根身份；一个变化元素归一到恰好一个生成根，同一生成根在一个批次内只生成一次。生成根有两种口径：交付单元根、正常颗粒根。
_Avoid_: 重生成根、regen root、目标根

**Ref0 库归属 (Ref0 Database Affiliation)**：
项目内每个 Ref0 唯一归属一个 `dbnum`，而一个 `dbnum` 可以拥有多个 Ref0；因此 `dbnum → Ref0` 是一对多关系，`Ref0 → dbnum` 可唯一反查，但 Ref0 本身不是 `dbnum`。同一 Ref0 出现在不同 `dbnum` 中属于库归属冲突，不存在可自动选择的有效归属。
_Avoid_: Ref0 数据库号、把 Ref0 当作 dbnum

**Ref0 库归属缺失 (Unresolved Ref0 Affiliation)**：
一个合法 Ref0 当前没有可用的 `dbnum` 归属。它表示模型依赖暂时无法定位，不等同于元素不存在，也不表示该依赖可以跳过。
_Avoid_: Refno 不存在、忽略的外部引用

**Ref0 库归属冲突 (Conflicting Ref0 Affiliation)**：
同一 Ref0 在项目文件索引中同时归属多个 `dbnum`，违反 Ref0 可唯一反查的不变量。系统不得猜选其中一个归属；只阻断命中该 Ref0 的工作，其他无冲突 Ref0 继续可用。
_Avoid_: 重复 Ref0、任选一个 dbnum

**最小交付单元 (Minimum Delivery Unit, MDU)**：
一类**具备独立成败与独立交付语义**的模型单元类型。类型集合由项目配置决定：默认 `BRAN / HANG / SUPPO / EQUI`，`delivery_unit_types` 可完全取代默认集合、`append_delivery_unit_types` 可在默认集合上追加；层级容器 `WORL/WORLD/SITE/ZONE` 与管件 `FTUB` 恒被拒绝。当变化元素自身或其最近祖先命中该类型时，以该单元为生成根；`FTUB` 及其子件在正常颗粒路径中也必须继续上溯，不能成为生成根。
_Avoid_: 交付单元类型、delivery type、内置交付单元

**交付单元根 (Delivery-Unit Root)**：
生成根的一种：变化元素**最近的** MDU 类型的自身或祖先；嵌套时取最近者。
_Avoid_: 单元根、unit root

**正常颗粒 (Normal Granularity)**：
生成根的另一种：当变化元素**没有** MDU 祖先时采用的默认口径，等同于自动 watch 路径长期使用的 significant owner 口径（而非「整个 ZONE 兜底」，也非「跳过」）。
_Avoid_: 常规颗粒、默认颗粒、ZONE 兜底

**Significant Owner（重要属主）**：
正常颗粒根的解析规则：从元素属主起、跨越 loop 容器上溯到最近的**非 SITE/ZONE** 属主作为根；若该属主是 SITE/ZONE（过粗），改用元素自身作根；元素自身即 SITE/ZONE/WORL 时不生成（不整区重算）。
_Avoid_: 显著属主、主属主

**Loop 容器 (Loop Container)**：
`LOOP / PLOO / VERT / PAVE` 等**自身不是几何生成根**的层级容器；解析生成根时需跨过它继续上溯到 PANE/EXTR 等真正的生成体。
_Avoid_: 环容器

**父模型输入 (Parent-Model Input)**：
自身不作为独立交付模型、但其数据参与父元素几何或坐标计算的子元素，例如 GENSEC 的 `SPINE / POINSP / JLDATU / PLDATU / ENDATU`；其变化归并到父级生成根。
_Avoid_: 辅助几何、漏生成类型

**模型影响 (Model Impact)**：
一次元素操作对模型的处理动作三态：`Regen`（重生成几何）/ `TransformOnly`（仅更新 world transform，网格不变）/ `Skip`（纯业务元数据，不处理）。由单一权威 `classify_operation_impact` 判定，取「宁多勿漏」。
_Avoid_: 几何影响、is_geometry_update

**净变化 (Net Change)**：
一个 `refno` 在整个待更新会话窗口内所有操作合并后的最终结果：`Added / Modified / Deleted / Cancelled`（新增后删除=无净变化）。
_Avoid_: 合并变化、final op

**搬迁 (Move / Relocation)**：
元素 `OWNER` 变化导致其归属的生成根在更新前后不同；原生成根与新生成根**两端都需重生成**。
_Avoid_: 移动、迁移

**应用水位 (applied_sesno)**：
某 `dbnum` 已整体落库的会话号上界。稳态增量窗口按「提交单元」推进：该窗口的数据与其全部模型生成一起写回成功才推进；基线路径（豁免暂存）按数据与生成工作计划原子收口推进。失败不推进也不回退，与扫描观察值 `file_latest_sesno` 严格区分、互不替代。
_Avoid_: 水位、sesno 水位（泛指时）

**候选库文件 (Candidate Database File)**：
监控目录里被认可为「一个 AVEVA 库文件」的物理文件：既不在扩展名/系统文件黑名单里，文件名又合 AVEVA 库命名（三位项目前缀 + 库号[ + 四位序号]，或 `<前缀>sys/com/mis`）。判定只看名字，不读文件头。人手复制的副本（`… copy`、`…_old`、带日期后缀的备份）与正本头部一字不差，只有命名规则分得开它们；把副本当候选会让同一个 `dbnum` 拿到多个候选而整库被判「同号重复」阻断。自动发现与手动扫描必须共用同一个判定。
_Avoid_: 库文件、数据库文件（泛指时）、排除规则

**登记文件身份 (Registered File Identity)**：
系统为一个 `dbnum` 认可的数据库类型与物理文件身份。它只在首次登记或确认唯一、同类型且水位不回退的路径迁移时改变；阻断异常中的候选文件不是新的登记身份。
_Avoid_: 最近扫描文件、观察到的文件身份

**待重试单元 (Pending Model Unit)**：
持久化保存的独立模型任务：按需生成的保护记录、跨窗口派生工作与房间任务积压。稳态增量窗口内部成功完成的生成根不会成为待重试单元；只有窗口未收敛的工作随提交单元持久化。任务以动作与目标为唯一身份，新触发以 revision 收口。
_Avoid_: 重试任务、pending task

**冻结吸收 (Freeze-time Absorption)**：
数据批次在执行起点重扫得到更高上界时，将该上界已经覆盖的后继排队区间并入当前运行批次。完全覆盖的后继任务以“已被当前批次吸收”成功终止，部分覆盖的后继区间从冻结上界之后继续。
_Avoid_: 幽灵任务、重叠批次、空跑补批

**关联展开链 (Association Expansion Chain)**：
设计元素 →(`SPRE`/`CATR`/SPREF)→ 目录构件(`SCOM`/`SPCO`) →(几何集 `GMSET`)→ 用设计参数(`DESP`/`PARA[]`)展开出图元 的正向链；决定"一个元素画成什么形状"。core.dll 在段重建时按此链现场展开。
_Avoid_: 目录链、catalogue chain

**反向引用 (Back-reference)**：
E3D 为可被引用元素维护的"谁引用了我"的存储型逆指针（`BREF`/`SPBREF`/`SCBREF`/`TABREF`/`DBREF` 等属性，由 `DB_ElementChangesPlugger::PostSetRefListAttribute` 维护）。用于从被改动的目录/规格元件**反查**需重生成的设计实例，是「关联模型也要更新」判定的权威来源。
_Avoid_: 逆引用、backref、被引用列表

**变更集 (Change Set)**：
按会话区间算出的元素变更集合（对应 core.dll 的 `DB_UserChanges`，由 `DB_DB::elementsChangedBetween` 产出）。core.dll 分五类：`elementCreated`/`elementDeleted`/`attributeModified`/`elementReordered`/`elementIncluded`；gen-model 现映射为 `Add`/`Modified`/`Deleted`（reorder 并入 `children_changed`，无 include）。
_Avoid_: 增量集、delta、diff

**模型变更通告 (Model Change Notice)**：
某个 refno 的模型产物已经落库、与观看端手上那份不再相同的对外告知；三种口径与「模型影响」同源：重生成 / 仅位姿 / 已删除。它只陈述「变了」，不指示「谁该重画」——重画与否由观看端按自己的可见性判断，服务端不认识任何一个场景。
_Avoid_: 局部刷新、增量推送、刷新事件

## 模型生成执行（Model Generation Execution）

**模型生成结果 (Model Generation Outcome)**：
某个生成根最近一次成功完成的生成结论，包括存在可绘制模型或确定没有可绘制几何。它是已经发生的成功事实，不是待重试单元；生成失败不会用失败结果覆盖它。
_Avoid_: 生成任务状态、pending 终态、实例计数

**分支原子替换 (Branch-Atomic Replacement)**：
单个 BRAN 的模型关系替换要么完整保留旧版本，要么完整提交新版本；已删除 BRAN 的新版本是空关系集，仍存在但暂时无法生成的 BRAN 继续保留旧版本。在稳态增量窗口中它是写回内部与基线/全量路径的实现单位，窗口对外的原子边界见「提交单元」。
_Avoid_: 生成根快照事务、先删后补

**合批重生成 (Batched Root Regeneration)**：
一次生成调用同时承载多个生成根的重算方式，与「逐根重生成」相对。整批同成同败；批失败后回退逐根，以定位真正坏掉的那个根。
_Avoid_: 批量生成、批处理生成、多根生成

**Fresh 根 / 重试根 (Fresh Root / Retry Root)**：
合批准入的二分：`attempts == 0` 且 refno 可解析的生成根是 fresh 根，可以进批；其余是重试根，只能单独跑。判据实现为纯谓词 `joins_regen_batch`。
_Avoid_: 新根、干净根、失败根

**revision 收口 (Revision-safe Settlement)**：
以待重试单元行的队列内部单调 `revision` 为条件完成删除或标记失败。revision 对不上意味着该任务已因新工作重新入队，本次执行结果不足以清除它；来源 `dbnum/sesno` 只用于追踪，不参与 revision、跨库排序、去重或复活判断。
_Avoid_: 收尾、结算、settle

## 暂存与写回（Staging & Write-back）

**提交单元 (Commit Unit)**：
一次性整体写回持久层的工作范围。稳态增量窗口的提交单元 = 该窗口的解析数据 + 其全部生成根的模型产物 +（当轮算成的）房间归属；写回成功之前，持久层看不到其中任何一笔。
_Avoid_: 批次事务、整体落盘（泛指时）

**稳态增量窗口 (Steady-state Increment Window)**：
在已有基线之上按会话区间推进的常规数据批次（dbnum × 会话区间），是暂存与整窗口写回的适用范围；基线 / 冷启动 / 全量生成路径豁免暂存。
_Avoid_: 增量批次（泛指时）

**暂存库 (Staging Database)**：
常驻内存数据库实例中按提交单元建立的独立 database；承载该单元的暂存工作集。批内生成失败可在原工作集重试，跨批次或进程崩溃后重新构造完整窗口；提交或废弃后整库删除。
_Avoid_: 内存库（泛指）、镜像库

**暂存工作集 (Staged Working Set)**：
一个提交单元在暂存库里的全部数据：按需解析出的设计 / 目录数据（含 CATA 引用闭包与祖先链）、按工作项预载的既有模型产物行、本单元新产出的产物。数据来源是增量解析与其 CATA 依赖解析，不从持久层复制设计数据。
_Avoid_: 工作集缓存、快照

**语句日志 (Statement Journal)**：
提交单元内按执行顺序累积的全部持久层写语句。写回 = 按原序分块事务重放，水位收口排在最后一个尾事务；任何一块失败整单元不生效，按窗口幂等重放。
_Avoid_: redo log、写缓冲

**水位门控写回 (Watermark-gated Write-back)**：
写回的原子性口径：重放过程可中断，但应用水位只在全部块成功后的尾事务里推进；以水位为准的读者把「半个写回」视为不存在。
_Avoid_: 硬原子写回、单事务写回

**提交后收敛 (Post-commit Reconciliation)**：
水位与派生意图已经共同提交，但全局空间状态尚未持久化完成的过渡阶段。该阶段可跨重启恢复，并在完成前阻止下一数据批次进入。
_Avoid_: 普通积压、空闲轮副作用

**commit-time-only 语句 (Commit-time-only Statement)**：
只在写回时对持久层全库执行、不在暂存库执行（或仅对工作集行执行）的全局扫描 / 修补语句；暂存库只需对本单元的读正确，终态正确性由写回时的全库执行保证。
_Avoid_: 延迟语句、异步语句

**窗口阻断 (Window Block)**：
稳态增量窗口内任一生成根重试穷尽仍失败时的终态：整个 `dbnum` 的水位停止推进、该窗口在持久层零痕迹、告警常驻；唯一解除方式是修复源数据后重存，新会话并入同一窗口重算。不存在自动或人工的降级提交。
_Avoid_: 死信降级、跳过坏根

## 按需解析元件库（On-demand CATA）

**按需解析（元件库 / CATA）(On-demand CATA Parsing)**：
只解析「本次生成真正引用到」的元件库元素，而非整库解析 CATA；与「整库解析」相对。
_Avoid_: 懒解析、增量解析 CATA、局部解析

**引用闭包 (Reference Closure)**：
从一组种子参考号出发，沿引用关系（横向出向引用 + 纵向 owner 链与容器子树）传递可达、并收口到元件库类型的参考号集合；是按需解析要解析的最小 refno 集。
_Avoid_: 依赖闭包、引用图、传递闭包（泛指时）

**部分解析 (Partial Parse)**：
给定 dbnum + refno 子集，仅解析这些元素（靠 refno→文件偏移索引定点），不整库解析。
_Avoid_: by-refno 解析、按号解析、随机解析

**生成根闭包 (Generation-Root Closure)**：
以一个生成根（见「生成根」）子树的出向引用为种子求得的引用闭包；在该根重生成前主动一次性解析落库。
_Avoid_: 根闭包、主动闭包

**惰性兜底 (Lazy Fallback)**：
生成期命中尚未解析的元件库参考号时，即时对其小闭包做部分解析并落库、随后重试原查询，保证不静默缺模型；用于覆盖引用闭包跟不到的非存储引用边。
_Avoid_: 惰性解析、按需补齐、lazy load

**闭包漏边 (Closure Miss)**：
引用关系不体现为存储型 `RefU64` 引用（如几何表达式里按名引用 `DTAB`/`CATREF`）时，引用闭包无法跟到的边；由惰性兜底覆盖。
_Avoid_: 漏引用、R2 残余（泛指时）

**dabacon 字典 (dabacon Dictionary)**：
E3D 的数据字典（schema 源），定义每种元素类型(noun)的属性布局与分类 flag。core.dll 运行期从 dict DB 建成内存表、按 `(nounHash, fieldId)` 读；gen-model 的 `all_attr_info.json` 只是其中「noun→属性」部分的 bincode 快照，不含分类 flag。
_Avoid_: 字典、schema 库

**noun 分类 flag (Noun Classification Flag)**：
dabacon 字典里描述某 noun 图形语义的布尔/枚举字段：`primitive`(#659518) / `geomset`(#859903) / `extrusion`(#663225) / `isPointsetPoint`(#290555737) / `graphicsBehaviour`(←5099119)。决定「是否几何、按哪种画法生成」；语义在字典、不在 core.dll 二进制。
_Avoid_: 类型标志、noun flag

**会话索引 (Session B-tree Index)**：
E3D db 文件「最新会话」内置的一棵 B-tree，把 `refno` 映射到其元素记录的页 / 偏移；据此可 O(log n) 单点定位（`find_refno_entry`）或遍历叶子直接建 `refno→偏移`表（`gen_ref_type_pos_table_from_index`），无需全文件扫描。
_Avoid_: refno 索引树、btree、索引区

**索引优先建表 (Index-first Table Build)**：
构建 `refno→文件偏移`表时优先走会话索引、解码失败才回退全缓冲扫描（`gen_ref_type_pos_table_scan`）；干净库两者相等，编辑库索引得最新会话存活集、更小且更权威（scan 会多收已删、漏活元素、指旧副本），是「部分解析 / 按需解析」定位的底座。
_Avoid_: 索引建表、index-first

## 房间归属（Room Membership）

**房间归属 (Room Membership)**：
构件与房间的从属关系，经面板物化为两类边：`room_relate`（面板→构件，带 `inside_count`/`center_dist` 排序字段）与 `room_panel_relate`（房间→面板）。材料表 surql 经 `fn::room_code` / `fn::room_num_of` 消费；是可事后重建的派生数据。
_Avoid_: 房间关系、房间隶属

**面板 (Panel / PANE)**：
房间的几何载体：`FRMW/SBFR 房间节点 → CWALL/CFLOOR → PANE` 层级中带盒状 mesh 的 PANE 元素。归属判定以面板 mesh 为容器做点包含检查；房间节点自身不参与几何判定。
_Avoid_: 板、墙板（泛指时）

**整间分支 (Panel Branch)**：
房间重算的粗粒度分支：面板自身变化时重算**该面板的全部成员**（先删面板全部 `room_relate` 出边、再整批写回）。对应 `RoomRecalcPanel` / `recalc_panel_membership`；成员元素的包围盒取自空间树。
_Avoid_: 面板分支、全房重算

**元素分支 (Element Branch)**：
房间重算的细粒度分支：单个构件变化时只重算该构件自己的归属。候选面板来自当前数据库中的在册面板索引，构件包围盒同样取自数据库。
_Avoid_: 构件分支、反向分支

**同轮吸收 (Same-round Absorption)**：
同一轮计算内整间分支已经覆盖某构件时，跳过该构件元素任务的省算优化。只有构件现存面板与当前在册面板候选都已由本轮覆盖时才允许吸收。
_Avoid_: 任务吸收、任务去重（此场景）

**空间树 (Spatial Tree)**：
全库元素世界包围盒的进程级 R\* 树（`GLOBAL_AABB_TREE`），房间候选查询与整间分支的成员盒都取自它。带脏标记（`AABB_TREE_DIRTY`）：AABB 刷新与删除清理置位，worker 空闲轮收尾若脏则序列化到 `accel_tree.bin`；启动时 `sync_aabb_tree_with_db` 按条目数与库对账。
_Avoid_: R 树（泛指时）、AABB 树、加速树

**AABB 变更集 (AABB Change Set)**：
包围盒刷新时与空间树旧值比对产出的几何差异集合，是房间增量计算的几何触发源；面板层级、房间命名等结构变化由独立的结构触发源补充。首次进入空间树也视为变化。
_Avoid_: 包围盒差异、aabb diff
