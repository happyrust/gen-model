# 增量模型生成（Incremental Model Generation）

本上下文描述：当 E3D/PDMS 设计数据发生增量变化（新增 / 修改 / 删除 / 搬迁）后，如何以**尽量小而正确**的范围重新生成受影响的三维模型，而不是全量重算。术语在此统一，实现见 `src/data_interface/`（数据面、队列、模型面状态）、`src/fast_model/`（发布、空间树、房间归属）与 `vendor/e3d-model`（模型面纯函数层，经 `e3d-io` 直读库文件）。

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
某 `dbnum` 已整体落库的会话号上界，只承诺数据已提交，不承诺模型或房间派生已经完成。稳态增量窗口的尾事务将数据、水位与模型窗口意图原子收口；模型/房间阶段可随后消费。数据提交失败不推进也不回退，与扫描观察值 `file_latest_sesno` 严格区分、互不替代。
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
把增量摄入的主 DESI 数据批次收窄到指定 dbnum 的调试开关（`DbOption.toml` 的 `watch_dbnums`，命令行 `serve --watch-dbnum` 压过它）。它限制的是范围、便于调试，不是生产形态的服务边界；但**初始化启动同样受它约束**——限定时启动的数据摄入、模型凭证复核与派生工作只覆盖域内的库，未限定时启动照常覆盖全部候选库。SYS meta（SYST/DICT/GLB/GLOB）不受限；两者都没给时判定与引入前逐位相同。监听限定域不是依赖隔离域：被监听 DESI 的生成根实际引用到的 CATA 文件仍参与身份与项目优先级裁决，但 CATA 只从文件读（`ModelTarget.catalogue` 文件身份摘要与 `E3dDbResolver`），不解析进持久层。与**调试限定域 (Debug Scope)**（`--debug-dbnum`，只能从命令行来、额外带六个裁决点的链路追踪）是两个独立开关，与已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 无关。三种「跳过」（MDB 范围判定 / 调试限定 / 监听限定）在日志、回执与 `/health` 上各有各的嗓音，不得混同。
_Avoid_: 监听白名单、dbnum 过滤、增量范围（指 MDB 声明范围时）、watch 名单、服务边界（它不是）

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
持久化保存的独立模型任务：按需生成的保护记录、跨窗口派生工作与房间任务积压。稳态增量窗口内成功完成的生成根不会成为待重试单元；只有窗口未收敛的工作随尾事务持久化。任务以动作与目标为唯一身份，新触发以 revision 收口。
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
单个 BRAN 的模型关系替换要么完整保留旧版本，要么完整提交新版本；已删除 BRAN 的新版本是空关系集，仍存在但暂时无法生成的 BRAN 继续保留旧版本。在 e3d-model 生成路径上它以根级 CAS 发布的形态成立（ADR-056 N3）：一个根一次发布，`ensure_not_older_than_persisted` 保证旧窗口不覆盖新版本。
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

## 数据窗口写回（Window Write-back）

**稳态增量窗口 (Steady-state Increment Window)**：
在已有基线之上按会话区间推进的常规数据批次（dbnum × 会话区间）；它走数据窗口直写，也是模型凭证前移与模型窗口意图的适用范围。基线 / 冷启动 / 全量生成路径不是稳态窗口（见「分块解析基线」）。
_Avoid_: 增量批次（泛指时）、暂存窗口

**冻结源 (Frozen Source)**：
一次收集冻住的「哪个文件、哪一代、哪个会话」（`data_interface::frozen_source::FrozenSource`）：稳定文件身份（Windows 卷序列号 + 文件索引 / Unix dev + inode）、`dbnum` 与库类型、冻住的目标会话与打开长度。收集与持久化之间它必须还是同一个——同文件追加接受，原子替换 / dbnum 漂移 / 会话或长度回退一律拒绝；纯数据窗口（零模型候选）同样过这道门。它是 e3d-io 底座上的商品，2026-09-03 起取代 old-pdms-io 的 `SnapshotToken` / `DabaconSnapshot`。
_Avoid_: 快照 token、冻结快照（指 `DabaconSnapshot` 时）、文件指纹（那是 `direct_store::FileIdentity` 的 len+mtime）

**净窗口收集器 (Net Window Collector)**：
数据面稳态增量窗口**唯一**的收集器（`data_interface::e3d_collector`，2026-09-03 起为 e3d-io 底座）：`IndexDiff(base@S, target@T)` 给每个 refno 的净三态，两端记录经 `to_named_attmap` 定形后 `diff_ele_data` 合成，每 refno 恰一条 `Add / Deleted / Modified`，挂 last-touch 会话。同一 refno 换了 noun 按 `Add` 写终稿并显式删旧 noun 行（`RetiredTypeRow`）；`shape_conflicts` 非空整窗阻断；不做任何成员关系仲裁——两根索引差分本身就是终态。old-pdms-io 的同名实现 2026-09-04 已整体删除（连 `legacy_pdms_io` 门都没了）；收集器对不对的**判据是 E3D TTY**——TTY 宏录下的会话链（`db8000_session_pairs` 夹具、issue-019 快照）与 TTY 现场往返（`Test-TtyNetWindow.ps1` / `verify_db8000_bran_window.ps1`），`shadow_collector_parity` 的 e3d-model L2 臂只是两条 e3d-io 合成路径互查，不是判据。
_Avoid_: 会话回放（`collect_changes`，已删）、拿第二套解析器当对拍参照、`net_window`（指 old-pdms-io 模块时）、成员补删

**数据窗口直写 (Direct Window Write-back)**：
稳态增量窗口唯一的数据写回路径（ADR-056）：`persist_latest_main_data` 按 TX_CHUNK 分块事务写属性行（幂等 UPSERT / 先删后插 / 软删 UPDATE）→ `invalidate_caches` → `maintain_reverse_index`（失败入补偿队列）→ 窗口语句批 → `finalize_attempt` 尾事务最后推进应用水位。任一块失败水位不动、整窗口按同一区间幂等重放；非水位读者可见秒级中间态，以水位为准的读者把「半个写回」视为不存在。不经任何暂存介质。
_Avoid_: 暂存写回、journal 重放、应急直写（它已是唯一路径）、direct 模式（那是 `AIOS_DATA_READ_MODE=direct` 的生成读侧，见 ADR-053）

**模型凭证前移 (Credential Advance)**：
数据窗口 S→T 提交后，对 e3d-model `plan_update(S→T)` 判定**未被触到**、已安定且落后于 T 的生成根，一条语句批量把 `gen_root.source_end_sesno` 推到 T（行上记 `credential_advanced_from`），不重生成、不动几何 / manifest / revision。护栏：候选数超过索引键数的 30%，或 `plan_update` 报 `unresolved` 非空，即整窗放弃前移、全部根照旧过期，回执记 `credential_advance_degraded`。
_Avoid_: 凭证刷新、水位前移、模型水位、跳过生成

**模型窗口意图 (Model Window Intent)**：
数据窗口尾事务与应用水位同事务写下的模型侧工作声明：本窗口受影响根集（`plan_update` 经 `touches_roots` 判定，含搬迁两端根与被删根的 `DeleteCleanup`）及其 revision，随尾事务 upsert 进 `model_update_pending`（进程本地账本，ADR-050）。它只说「这一窗要生成什么」，不承诺已生成；eager 生成的范围就是它，其余根前移凭证后懒生成、等按需 `ensure`（ADR-056 D9）。
_Avoid_: 模型任务（泛指时）、全库过期入队、模型计划（当指 e3d-model 的 `UpdatePlan` 时）

**提交后收敛 (Post-commit Reconciliation)**：
水位与派生意图已经共同提交，但全局空间状态尚未持久化完成的过渡阶段。该阶段可跨重启恢复，并在完成前阻止下一数据批次进入。
_Avoid_: 普通积压、空闲轮副作用

**窗口阻断 (Window Block)**：
稳态增量窗口**数据侧**确定性失败的终态（收集器报错、写回被持久层确定性拒绝）：该 `dbnum` 的水位停止推进、告警常驻，直到源数据或持久层修复后按同一区间重放。模型生成失败**不阻断窗口**（ADR-056 D1）：根级失败进 `model_update_pending` 重试账，水位照常推进。
_Avoid_: 死信降级、跳过坏根、模型阻断

**pe_owner 边 (Owner Edge)**：
父子关系在库里的唯一形状：`{ id: pe_owner:[owner, 槽位], in: child, out: owner }`（`ele_ops::pe_owner_edge`）。方向以读侧为准——rs-core `{owner}<-pe_owner` 取 `in` 当 children、`->pe_owner.out` 取 owner，唯一索引 `n` 也建在 `(in, out)` 上。全仓只此一个写法：基线路 2026-09-04 换成 e3d-io 退化窗口后经 `ElementRows` 调同一个函数，`cata_closure` 的成员块渲染与之逐字节同形（有源码钉对比两者的 INSERT）。2026-09-04 之前增量路的终态渲染器写反成 `in = owner, out = child`，写出的子树在树面板上读不到、`DELETE {pe}->pe_owner` 清的也不是自己的 children 边；受影响的库靠 `POST /model/rebuild` 重建，不手改数据。
_Avoid_: 父子边（泛指时）、`owner` 字段（`pe` 行上没有这个字段）、按 `out` 取 children

**分块解析基线 (Chunked Baseline Parsing)**：
基线是 e3d-io 的**退化窗口**（base 为空、target = 冻结目标会话），不是第二条解析路：`e3d_collector::stream_baseline` 枚举目标索引，每 `sync_chunk_size`（默认 10 万）个元素交一块给 `versioned_db::baseline::render_baseline_batches` 渲成写者消息，经有界队列交单写者 `INSERT IGNORE` 批量落库；块交出去就释放，峰值内存随块不随库。行形状（`pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner`）与增量路共用 `ElementRows`，语句动词沿用 legacy 基线的 `INSERT IGNORE`——`manual_update` 的父层补缺靠「已有行赢」保住叶子 extract 里正在改的版本，换 `UPSERT` 会改掉这条语义。它**不走提交协议**：基线不开窗口，窗口买的是原子提交与崩溃恢复，不是内存有界。2026-09-04 随 `parse_pdms_db` 退役消失的三件事：把冻结前缀整个读成一个 `Vec<u8>`（`FrozenSource::read_frozen_bytes`）、`mem://` 暂存实例与它的读路由、`member_prune` 事后裁成员（目标索引差分本身就是终态）；`pe.sesno` 从链头 0 变成元素真实的 last-touch 会话。
_Avoid_: 暂存窗口（基线不开窗口）、全量解析 / `DbBasicData`（随 `parse_pdms_db` 退役）、按需解析（那是元件库的）

### 已退役词条（retired，ADR-056）

以下词条只为读 2026-08-05 ～ 2026-09-02 之间的 ADR / 计划 / 证据时对照使用，**现行代码与文档不得再用**：

- **提交单元 (Commit Unit)**、**暂存库 (Staging Database)**、**暂存工作集 (Staged Working Set)**、**语句日志 (Statement Journal)**、**水位门控写回 (Watermark-gated Write-back)**、**commit-time-only 语句 (Commit-time-only Statement)**——ADR-017 kv-mem 暂存窗口的构件，随暂存层退役；现行口径见上方「数据窗口直写」。
- **窗口阻断的模型侧含义**（生成根重试穷尽 → 整窗口水位停止、持久层零痕迹）——被 ADR-056 D1 取代，模型失败与水位解耦。
- **成员关系仲裁补删 (ADR-036 Membership Reconciliation)**、**幽灵新增丢弃**——old-pdms-io 净窗口收集器在索引差分之外按成员表补判的删除 / 丢弃；2026-09-03 收集器换成 e3d-io 底座后随之退役（两根索引差分即终态，`membership_deleted` 恒 0）。现行口径见上方「净窗口收集器」。

## 按需解析元件库（On-demand CATA）——已整章退役（retired，ADR-058 D1）

> CATA 不再解析进持久层：模型面经 `E3dDbResolver` 从文件读，级联判据用 CATA 文件的会话索引差分加
> `ref_rev` 反查。本章六个词条只为读 2026-07-27 ～ 2026-09-03 之间的 ADR / 计划 / 证据时对照使用，
> **现行代码与文档不得再用**。CATA 的**文件身份裁决**（项目优先级、同号选主、`CataDbLocator`）不在退役之列
> ——那是 `E3dDbResolver` 的输入，不是解析。

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

初始化不是一条链，是**两条并行链**：模型面在文件身份解析完成后即可开工，数据面走
Meta（SYS/DICT）→ Design（DESI）。Catalogue 阶段已取消——CATA 不入持久层，模型面从文件读；
模型面也不再等 `data_ready`，串行那一版的唯一理由是「生成器要读 `pe`」，那个理由已经不在了。
房间仍排在模型之后。阶段完成以 manifest、水位和数据支撑为准，不以队列暂时为空为准。

实施状态（2026-09-03）：**门的部分已落地**（`model_generation_allowed` 不再看 `data_ready`，
`install_manifest` 装好清单即开模型门，启动模型复核排在数据等待之前）；**尚未落地**的是
`Catalogue` 相删除 / CATA 退出持久层（refactor plan S3）与第二车道——worker 仍是单消费者，
模型积压只在数据队列空隙消化（`DataQueued` 让位），字面意义的双链同时跑挂 D10-B。

_Avoid_: 类型排序、启动排序、初始化优先级（这些说法没有表达阶段屏障与失败阻断）、
Catalogue 阶段（已取消）、Meta → CATA → DESI → Model（已退役的五段顺序）。

# 启动退化窗口（Startup Degenerate Window）

启动时对一个库做的一次窗口选根：左端取该库生成根凭证的下限，右端取文件最新会话，其余与稳态增量窗口
共用同一套选根、分桶与护栏——**启动没有自己的选根判据**。它只产生模型侧工作，不摄入数据、不推进应用水位；
凭证已经不早于文件最新的库连窗都不开。

_Avoid_: 启动复核、覆盖扫描、模型完整性扫描（这些说法没有表达它就是一次窗口）。

# 阶段纪元（Phase Epoch）

一次完整候选扫描安装的可重建版本。新扫描发现更早阶段目标时产生新纪元；旧纪元完成事件
不得满足新目标。

_Avoid_: 扫描批次、队列轮次。

# 数据就绪（Data Ready）

Meta、Catalogue、Design 均完成最终复查的状态。只有数据就绪后才能产生新的模型写入。

_Avoid_: 队列空、初始化尝试完成。

# 提交回执 (Commit Receipt)

稳态增量窗口稳定生成的 `commit_token`。尾事务以它为幂等保护，并与水位写入同一事务；客户端
超时时用该值区分“服务端已提交”和“尚未提交”。

_Avoid_: 请求 ID、重试 ID。

# 依赖缓存上下文 (Dependency Cache Context)

窗口内 CATA 闭包缓存的显式来源边界：源 dbnum 与本窗口实际 `effective_end_sesno`。不得以文件
最新会话或活动窗口全局状态代替。

_Avoid_: 最新会话缓存、活动窗口缓存。

# 纪元激活门 (Epoch Activation Gate)

manifest/epoch 安装与任务冻结共用的短临界区；固定锁序为 activation gate → scheduler queue →
coordinator，保证旧纪元任务不会越过新纪元激活边界。

_Avoid_: 初始化锁、队列锁（泛指时）。
