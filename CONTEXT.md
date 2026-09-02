# 增量模型生成（Incremental Model Generation）

本上下文描述：当 E3D/PDMS 设计数据发生增量变化（新增 / 修改 / 删除 / 搬迁）后，如何以**尽量小而正确**的范围重新生成受影响的三维模型，而不是全量重算。术语在此统一，实现见 `src/data_interface/` 与 `src/fast_model/`。

> 本文件仅为词汇表（glossary），不含实现决策与流程；决策见 `docs/adr/`，规格见 `docs/specs/`，计划见 `docs/plans/`。

## Language

**增量模型生成 (Incremental Model Generation)**：
在已有全量模型基础上，仅对本次数据变化影响到的部分重新生成模型，与「全量生成」相对。
_Avoid_: 局部更新、局部刷新

**增量阶段控制 (Increment Stage Control)**：
分别许可数据增量、模型增量、房间增量三个顺序阶段执行的调试配置。控制的是消费，不是发现或删队列：上游关闭时下游不得越过，重新开启后从 durable 工作继续。
_Avoid_: 流水线模式、跳步开关、增量总开关

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

**扫掠体 (Sweep Solid)**：
与 Core3D `DB_Gensec` 同一类几何：目录截面沿路径扫成的实体，对外几何参数是 PrimLoft。公开生成步骤用 `DB_Gensec` 方法的 Rust 蛇形命名。
_Avoid_: loft 体、GENSEC 几何（GENSEC 是元素，扫掠体是它的几何）、单位 loft

**单位几何 (Unit Geometry)**：
去掉实例姿态后可被多个实例共享的规范内容。可复用直线扫掠的单位几何是目录截面；长度、BANG、PLAX、镜像属于实例变换。
_Avoid_: 单位体（当指扫掠的可复用身份）、unit shape、单位 loft

**实例变换 (Instance Transform)**：
把单位几何放到世界里的位姿与缩放。不是元素子树的 world transform。
_Avoid_: geo_relate 变换、world transform、单位缩放

**单位网格身份 (Unit Mesh Identity)**：
内容寻址键，标识一份可共享的单位几何。Core3D 没有持久化的这一层，是相对几何内核的唯一差别。
_Avoid_: geo_hash、mesh hash、loft hash

**libgm 面片口径 (libgm Facet Caliber)**：
曲面原语的段数由该实例的真实半径与弦高容差算出，规则只有 `libgm_discretise` 一份（照抄 libgeom 的 `d2_numberOfSegmentsForCircle` / `d2_numberOfSegmentsForPartRev`），每个原语喂哪个半径逐条钉死。它不是画质旋钮：`doFacetCancellation` 只消全等重叠，共面的两层侧壁差一段，共面抵消就整个放弃、布尔结果里留一层内壁——段数因而是布尔能否收敛的前置条件。段数随之进入单位网格身份，同一原语按段数裂成多份网格（4 的倍数，落在 `[8, 1000]`）。
_Avoid_: 默认段数、细分级别、写死 32 段

**规范挤出 (Canonical Extrusion)**：
由目录截面沿单位轴挤出固定长度得到的三维网格，供多实例以缩放复用。它是单位几何（截面）的可绘制形式，不是单位几何本身。
_Avoid_: 单位体、unit solid（当单位几何已定为截面时）

**斜切平面 (Mitre Plane)**：
由端面法向与该段切向推导出的工作平面；与切向垂直或平行则视为无斜切。不是元素上的 `DRNS`/`DRNE` 属性本身。
_Avoid_: 斜切、is_sloped、端面方向（当指属性时）

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
某 `dbnum` 已整体落库的会话号上界，只承诺数据已提交，不承诺模型或房间派生已经完成。稳态暂存窗口将数据、水位与 durable 模型计划原子收口；模型/房间阶段可随后消费。数据提交失败不推进也不回退，与扫描观察值 `file_latest_sesno` 严格区分、互不替代。
_Avoid_: 水位、sesno 水位（泛指时）

**生成源版本 (Model Source Version)**：
一个库在当前投影下用来生成模型的那一版数据：调用方显式指定的会话号 / 时刻（历史投影、增量窗口右端），或——未指定时——库文件此刻自报的最新会话。它不是应用水位：水位只说数据已摄入，生成时点不从它取；完成凭证记的是模型按哪一版生成，判「当前」看它是否**不早于**要求的版本，不判等值。
_Avoid_: 生成水位、模型水位、把 applied_sesno 当生成时点

**数据支撑 (Data Backing)**：
应用水位的对偶约束：`applied_sesno > 0` 时，该 `dbnum` 必须在 `pe` 里存在至少一行数据，或持有与当前应用水位相等的空基线凭据。判定用存在性查询而不是计数；通常在「基线还是增量」的路由决策上使用，启动重扫遇到水位恰好追平文件时也在入队裁决前使用，以免数值早退漏掉幽灵水位。查询失败必须上浮为批次失败或形成可见的本轮扫描失败，不得吞成「有数据」或「没有数据」中的任何一种默认值。
_Avoid_: 行数校验、pe 计数、数据存在性（泛指时）

**空基线凭据 (Confirmed Empty Baseline Credential)**：
一次成功的全量基线明确解析为合法空库时，与应用水位在同一事务写入的 `confirmed_empty_baseline_sesno`。它只在等于当前 `applied_sesno` 时证明零行是合法结果；非空基线清除它，水位继续推进后旧值自然失效。
_Avoid_: 空库标记、零行豁免、空水位

**幽灵水位 (Ghost Watermark)**：
`applied_sesno > 0`、该 `dbnum` 在 `pe` 里零行且没有匹配空基线凭据的状态——应用水位声称落库过，库里却没有数据支撑（多由 `dbnum_info_table` 播种回填或历史解析中断留下）。处置是路由到首次导入重建基线，不从水位往后接增量；它不是文件异常，不进 `FileAnomaly`，扫描阶段也不删任何数据。
_Avoid_: 假水位、虚水位、水位漂移

**重建批次 (Reinit Batch)**：
检出文件回退后入队的数据批次，按首次导入形状（窗口 `1..file_latest_sesno`）排进同一条数据批次队列。扫描只分类入队、不删数据；worker 出队后在冻结点复核仍判回退，才执行 `wipe_dbnum_for_reinit` 整库清空（水位行清值不删行、spatial epoch 同阶段递增），随后按首次导入重新解析，应用水位随重建归零再重新建立。
_Avoid_: 重建任务、对齐批次、realign 批次

**监听限定域 (Watch Scope)**：
把增量摄入的主 DESI 数据批次收窄到指定 dbnum 的调试开关（`DbOption.toml` 的 `watch_dbnums`，命令行 `serve --watch-dbnum` 压过它）。SYS meta（SYST/DICT/GLB/GLOB）不受限；两者都没给时判定与引入前逐位相同。监听限定域不是依赖隔离域：被监听 DESI 的生成根实际引用到的 CATA 文件仍参与身份与项目优先级裁决，并以元素级引用闭包随该 DESI 窗口解析；这类部分解析不建立或推进 CATA 的完整应用水位。与**调试限定域 (Debug Scope)**（`--debug-dbnum`，只能从命令行来、额外带六个裁决点的链路追踪）是两个独立开关，与已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 无关。三种「跳过」（MDB 范围判定 / 调试限定 / 监听限定）在日志、回执与 `/health` 上各有各的嗓音，不得混同。
_Avoid_: 监听白名单、dbnum 过滤、增量范围（指 MDB 声明范围时）、watch 名单

**候选库文件 (Candidate Database File)**：
监控目录里被认可为「一个 AVEVA 库文件」的物理文件：既不在扩展名/系统文件黑名单里，文件名又合 AVEVA 库命名（三位项目前缀 + 库号[ + 四位序号]，或 `<前缀>sys/com/mis`）。判定只看名字，不读文件头。人手复制的副本（`… copy`、`…_old`、带日期后缀的备份）与正本头部一字不差，只有命名规则分得开它们；把副本当候选会让同一个 `dbnum` 拿到多个候选而整库被判「同号重复」阻断。同项目主库（无后缀）与唯一 `_NNNN` 抽取是抽取树的父层与叶子，归并成一个逻辑库，不是同号重复；多个 `_NNNN` 仍是 Duplicate。自动发现与手动扫描必须共用同一个判定。
_Avoid_: 库文件、数据库文件（泛指时）、排除规则

**文件观察 (File Observation)**：
收集层为一个候选库文件记下的、某一时刻的文件系统事实：路径、修改时刻、大小、观察时刻与观察来源。它只陈述文件层面看得见的东西，不含库号、库类型与最新会话号——那三样都要读文件头，是分析层的裁决。文件变化与周期全集轮产出同一种观察，区别只在覆盖面是子集还是全集；同一路径的多条观察可合并成最新一条，因为它描述的是状态而不是发生过的事。
_Avoid_: 文件事件、变更消息、notify 事件

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

## 模型面与数据面（Model Face & Data Face）

**模型面 (Model Face)**：
从库文件的某一会话算出三维模型产物的那一面，含全量生成与窗口差分；对「(库文件, 会话) + 显式配置」是纯函数，不解释「最新」、不认识水位。
_Avoid_: 生成器（泛指时）、fast_model（当指概念时）、模型管线

**数据面 (Data Face)**：
把库文件的属性数据摄入持久层并推进应用水位的那一面；与模型面并行而非在前，两者只共用同一份文件窗口差分。
_Avoid_: 解析管线、增量管线（泛指时）、数据增量（当与模型增量混称时）

**模型面状态 (Model-Face State)**：
模型面在持久层里属于自己的记录——完成凭证、发布版本、产物行、反向索引——与设计数据本身无关；它和随之而来的副作用（发布、排队、派生面通告）不属于模型面的纯函数部分。
_Avoid_: 模型元数据、gen_root 表（当指概念时）、模型缓存

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

**模型实例保存合批 (Shape Save Coalescing)**：
几何生产者连续发出的多个小 `ShapeInstancesData`，在固定实例数 / 几何 occurrence / 源批数 / 估算字节 / 等待时间上限内合成一次确定性保存。保留全部原始批，禁止调用 `ShapeInstancesData::merge`。与「合批重生成」（一次调用承载多个生成根）不是同一件事。
_Avoid_: 批量保存、merge 合批、Save merge、合批写入（与合批重生成混用时）

**保存计划 (SavePlan)**：
第一次 scoped delete 或写入前完成的不可变计划：校验（NaN、normal/tubi 重叠、同 ID 内容冲突）、元数据解析、去重、排序和 SQL 分包。只有成功 `SaveOutcome` 里的 refno 才计入本轮产出。
_Avoid_: 保存方案、写计划、保存脚本

**保存模式 (SaveMode)**：
实例保存的路径区分：`TargetedReplace` 保留定向 scoped cascade delete；`FullBuild` 不预删。定向与全量共用同一保存 receiver 与去重规则。
_Avoid_: 保存策略、写模式、保存档位

**revision 收口 (Revision-safe Settlement)**：
以待重试单元行的队列内部单调 `revision` 为条件完成删除或标记失败。revision 对不上意味着该任务已因新工作重新入队，本次执行结果不足以清除它；来源 `dbnum/sesno` 只用于追踪，不参与 revision、跨库排序、去重或复活判断。
_Avoid_: 收尾、结算、settle

**切片流水线 (Slice Pipeline)**：
基线初始化的并行形态。并行单位是**生成根**——它本来就有 per-根锁这一现成粒度；切片（ZONE）只决定「一次把多少数据拉进来」，是内存分片单位，不决定几件事同时做（按片并行要整片等最慢的根，长尾吃掉加速比）。一片一个 `mem://` 实例，片内的根共享本片实例。根级与根内 fan-out 共用同一个几何并发闸，额度默认等于物理核数，不再各自写死宽度。
_Avoid_: ZONE 并行、按片并行、分片并发

**insts_flat 失效协议 (insts_flat Invalidation Protocol)**：
维护 `inst_relate.insts_flat` 这份图遍历缓存的规则：**开销与变更量成正比，不与库规模成正比**。它取代的是不带 dbnum、谓词不可索引的全表清扫——那种形态对生成根数是平方级的，实测维护开销达到它所服务的那件事的 2.9 倍。缓存本身不容置疑（它把加载从 253.3s 压到 2.73s），被否掉的是维护方式。
_Avoid_: 平表清扫、缓存刷新、sweep（泛指时）

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

**分块解析基线 (Chunked Baseline Parsing)**：
基线解析按遍历分批装入 `mem://` 实例、整批写回后释放，而不是把冻结前缀整个读成一个 `Vec<u8>` 常驻内存。它只取「实例 + 读路由」这一半（`init_staging_schema` + `StagingReadContext` + `with_staging_reads`），**不走提交协议**——基线不开窗口，窗口买的是原子提交与崩溃恢复，不是内存有界。
_Avoid_: 暂存窗口（基线不开窗口）、流式解析、按需解析（那是元件库的）

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

**会话索引差分 (Session Index Diff)**：
每个会话页都带当时的索引根（copy-on-write）；取窗口两端的根做双根差分，得到窗口两端的 refno 存在性净三态。
_Avoid_: 索引对比、index diff、净窗口差分

**目标成员存活 (Target Membership Liveness)**：
只针对父元素成员表净减少候选的逻辑存活裁决：候选在目标会话是否仍被某个有效 OWNER 的成员表包含。旧物理记录仍在索引里不等于元素仍存活；同窗口被其他 OWNER 接纳则属于搬迁，不属于删除。
_Avoid_: 索引存活、OWNER 字段存活、记录存在即存活

**逐会话回放 (Session Replay Collection)**：
按窗口内每个会话认领该会话新写记录，再逐 refno 对相邻版本与属主做属性差分的收集方式。
_Avoid_: 回放收集（当指生产口径时）、逐会话 collect

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
全库元素世界包围盒的进程级 R\* 树（`GLOBAL_AABB_TREE`），房间候选查询与整间分支的成员盒都取自它。带脏标记（`AABB_TREE_DIRTY`）：AABB 刷新与删除清理置位，worker 空闲轮收尾若脏则原子写回项目树文件 `accel_tree_{project}.bin` 并盖空间版本号章；启动时版本号一致才信文件，否则从库指针重建。库中 `inst_relate.aabb` 指针是它唯一的重建真值源。
_Avoid_: R 树（泛指时）、AABB 树、加速树

**refno 反查 (Refno Lookup)**：
按 refno 问空间树「这批盒子在不在树上、旧值是什么」的能力，读路径与写路径共用 `AccelerationTree` 内部那张 `refno_index`（纯内存派生，不进树文件，加载后重建）。读路径若退回 `tree.iter()` 全量遍历，每个生成根前后各扫两遍整棵树，代价对库规模是平方级——并行化只是把平方项除以核数。禁止在调用方另建一份 refno → 包围盒映射：那会造出第二份「树上现在有什么」的真值，漂移的后果不是变慢，是把已经不在那儿的构件算进某间房，而这类错误没有任何东西会报出来。
_Avoid_: 遍历树查找、tree.iter() 找、自建 refno 索引

**空间版本号 (Spatial Epoch)**：
库侧单调递增的空间提交计数：每条携带空间意图的尾事务与水位、意图同事务 +1。树文件 sidecar 记录落盘时刻的版本号；启动时与库侧不相等即判文件陈旧、改从库指针重建。只比相等、不表达次数，重复递增无害。
_Avoid_: 树版本、时间戳校验

**指针重建 (Pointer Rebuild)**：
从库中已提交的 `inst_relate.aabb` 指针分页整树重建空间树的只读路径；不重算几何、不回写库，与「重算修复」（重算几何并回写指针，手工工具）相对。
_Avoid_: 全量重算（此场景）、树重建（泛指时）

**AABB 变更集 (AABB Change Set)**：
包围盒刷新时与空间树旧值比对产出的空间差异集合。普通全量/维护刷新只按它触发；定向增量还会保守纳入实际重写/变换的几何目标。面板层级、房间命名等变化由独立结构触发源补充。首次进入空间树也视为变化。
_Avoid_: 包围盒差异、aabb diff

**房间重建要求 (Room Rebuild Requirement)**：
结构面板枚举失败后随数据水位原子写入 `room_build:main` 的持久降级标志。它表示已提交数据上的增量面板目标不完备，下一次成功的全量房间重建必须覆盖并清除此标志；当前进程内不承诺自动启动全量重建。
_Avoid_: 普通 warning、在线重建任务

## 实机端到端测试（Live E2E Testing）

**金基线对 (Golden Baseline Pair)**：
E3D 项目副本与其对应 Surreal 数据快照组成的**成对**不可变基线；恢复时两边必须一起恢复，保证「文件会话号 与 应用水位」恒成对，不触发会话回退阻断。以版本号整体演进，不做单边修补。
_Avoid_: 基线备份（单边）、快照（泛指时）

**场景宏对 (Scenario Macro Pair)**：
一个测试场景的 apply / restore 两个 PML 宏：apply 施加变化并 SAVEWORK，restore 以再一次 SAVEWORK 把值改回。restore 不是回滚——它本身就是一次反向真实增量，顺带验证反向路径。
_Avoid_: 回滚宏、撤销脚本

**哨兵日志 (Sentinel Log)**：
PML 宏经 ALPHA LOG 分段写出的、带哨兵标记行的日志文件；是编排器判定宏「活着 / 完成 / 死在哪一段」的唯一信号。分段是因为 ALPHA LOG 缓冲到 END 才落盘，一段失败只吞掉该段。
_Avoid_: 宏日志（泛指时）、输出文件

**双侧对拍 (Dual-side Parity)**：
同一属性在 E3D 会话内查询到的源头真值与增量落库后库侧值的比对。抓「解析层把值写歪」一类只有拿到源头真值才能发现的缺陷。
_Avoid_: 数据校验（泛指时）、一致性检查（泛指时）
# 严格初始化阶段（Strict Initialization Phase）

无监听限定域时，增量初始化与同轮稳态更新的固定顺序是 Meta（SYS/DICT）→ Catalogue
（CATA）→ Design（DESI）→ Model。监听限定域非空时，不建立全量 Catalogue 批次，顺序是
Meta → Design（内部先完成该窗口所需的 CATA 引用闭包）→ Model。阶段完成以 manifest、
水位和数据支撑为准，不以队列暂时为空为准。

_Avoid_: 类型排序、启动排序、初始化优先级（这些说法没有表达阶段屏障与失败阻断）。

# 阶段纪元（Phase Epoch）

一次完整候选扫描安装的可重建版本。新扫描发现更早阶段目标时产生新纪元；旧纪元完成事件
不得满足新目标。

_Avoid_: 扫描批次、队列轮次。

# 数据就绪（Data Ready）

Meta、Catalogue、Design 均完成最终复查的状态。只有数据就绪后才能产生新的模型写入。

_Avoid_: 队列空、初始化尝试完成。

# 提交回执 (Commit Receipt)

暂存窗口稳定生成的 `commit_token`。尾事务以它为幂等保护，并与水位写入同一事务；客户端
超时时用该值区分“服务端已提交”和“尚未提交”。

_Avoid_: 请求 ID、重试 ID。

# 依赖缓存上下文 (Dependency Cache Context)

暂存 CATA 闭包缓存的显式来源边界：源 dbnum 与本窗口实际 `effective_end_sesno`。不得以文件
最新会话或活动窗口全局状态代替。

_Avoid_: 最新会话缓存、活动窗口缓存。

# 纪元激活门 (Epoch Activation Gate)

manifest/epoch 安装与任务冻结共用的短临界区；固定锁序为 activation gate → scheduler queue →
coordinator，保证旧纪元任务不会越过新纪元激活边界。

_Avoid_: 初始化锁、队列锁（泛指时）。
