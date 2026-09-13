# 增量模型生成（Incremental Model Generation）

本上下文描述：当 E3D/PDMS 设计数据发生增量变化（新增 / 修改 / 删除 / 搬迁）后，如何以**尽量小而正确**的范围重新生成受影响的三维模型，而不是全量重算。术语在此统一，实现见 `src/data_interface/`（数据面、队列、模型面状态）、`src/fast_model/`（空间树、房间几何筛选）与 `vendor/e3d-model`（模型面纯函数层，经 `e3d-io` 直读库文件）。

> 本文件仅为词汇表（glossary），不含实现决策与流程；决策见 `docs/adr/`，规格见 `docs/specs/`，计划见 `docs/plans/`。

## Language

**增量模型生成 (Incremental Model Generation)**：
在已有全量模型基础上，仅对本次数据变化影响到的部分重新生成模型，与「全量生成」相对。
_Avoid_: 局部更新、局部刷新

**增量阶段控制 (Increment Stage Control)**：
分别许可数据增量、模型增量两个顺序阶段执行的调试配置（`data_incremental` / `model_incremental`）。控制的是消费，不是发现或删队列：上游关闭时下游不得越过，重新开启后从水位继续。房间归属不是第三个阶段：`room_membership` 只决定每个模型根持久化时是否顺带计算、写入房间边；关闭不阻断模型根与 `model_sesno`，重新开启也不会按模型水位发现历史房间欠账，需显式模型重建。原阶段开关 `room_incremental` / `AIOS_ROOM_INCREMENTAL` 已随房间流程退役（ADR-073 D7）。
_Avoid_: 流水线模式、跳步开关、增量总开关、把 `room_membership` 当模型消费门、三阶段

**生成根 (Generation Root)**：
一次增量重算所选定的**根参考号**——从它开始（含其子树）重新生成模型。生成根以 `refno` 为唯一身份，所属数据库由 RefNo 位解码得到（见「RefNo 库归属」），`dbnum` 不属于生成根身份；一个变化元素归一到恰好一个生成根，同一生成根在一个批次内只生成一次。生成根有两种口径：交付单元根、正常颗粒根。
_Avoid_: 重生成根、regen root、目标根

**RefNo 库归属 (RefNo Database Affiliation)**：
一个合法 refno 属于哪个 `dbnum`，由它自己的 `word0` **按位解出**：`w0 & 0x1FFF | (w0 >> 13) & 0x3E000`（`vendor/e3d-io` `RefNo::dbno()`，与 core.dll `DB_Element::dbno` / `sub_5B05260` 同式，2.1 / 3.1 一致；证据 `docs/evidence/2026-09-12-ida-core-dabacon-refno-routing.md`）。bits 13..25 是 extent 号，所以同一库每个 extent 一个不同的 `word0`——`word0` 不是 `dbnum`，但也**不需要任何表**来反查。gen-model 里唯一的解法是 `data_interface::refno_route::refno_dbnum()`（`tests/refno_native_guards.rs` 守着，位公式别处不得再写）；解不出（`word1` 为负、dbno 为 0）是 `InvalidRefNo`，与「不在 MDB」分开报。
_Avoid_: Ref0 库归属、ref0 反查、ref0 → dbnum 表、把 word0 当作 dbnum、逐库试查

**dbnum 路由目录 (dbnum Route Directory)**：
`dbnum → 文件`的那张表（`data_interface::mdb_route_directory::MdbRouteDirectory`），只从当前 MDB 的 SYS/CURD 成员清单与每个成员文件的 60 字节头造（`from_membership`），再并入依赖项目（`included_projects`）的 CATA / PROP 文件头（`add_dependency_dir`），**不读任何元素**。它回答的是「这个 dbnum 的文件在哪、是什么类型、用哪份模板」（`pin` / `pin_for(preferred_project)`，`sesno: None` = 开库时冻最新），不回答「这个 refno 在哪个库」——那是位解码的事。同号跨项目按 `preferred_project` 选同类型副本并核头，MDB 声明路径胜出，落选的只记 `shadowed`。属性服务、树服务、探针三处的定位器都从它造；解出的 dbnum 不在目录里是 `DbNotInMdb`（声明了没文件是 `NoFileForDbnum`），**不回退扫描**。
_Avoid_: 定位器（泛指时）、ref0 定位器、`InMemoryCataLocator`（生产路径）、骨架的 `locator_parts()`

**跨库属性 (Cross-database Attribute)**：
引用类属性（`SPRE` / `CATR` / `LSTU` / `PSPE` / `HSTU`、`OWNER`）的值是一个裸 refno；「跨库」不发生在取值那一步（取值只拷两个 word，零 I/O），而发生在**顺着它去读目标元素**那一步：位解码得 dbnum → 路由目录给文件 → 开库核头（文件自报 `db_mark` 必须等于解出的 dbnum，否则 `DbNumberMismatch`）→ 在目标库自己的索引上点查。跨库 `OWNER` 上溯（ZONE → SITE）与生成根定库（`model_source::dbnum_of_root`：解码 + 核成员是 DESI / ISOD + 只在那一个库点查一次）走同一条路。成本与 MDB 元素总数无关：读一个属性只开触达的库。真库对账口径是 `direct_attmap_probe --owner-climb` 的「解码 = 文件自报」账本（ams8000：1313/1313）。
_Avoid_: 跨库引用解析（当指建表时）、换库反查、CATA 闭包（那是生成期把目录依赖**落库**的另一件事）

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
某 `dbnum` 已整体落库的会话号上界，只承诺数据已提交，不承诺模型或房间派生已经完成。稳态增量窗口的尾事务将数据与水位原子收口；模型 / 房间在同一个 dbnum 任务里随后算完并序列化（ADR-065 / ADR-071，不再有「模型窗口意图」行）。数据提交失败不推进也不回退，与扫描观察值 `file_latest_sesno` 严格区分、互不替代。
_Avoid_: 水位、sesno 水位（泛指时）

**生成源版本 (Model Source Version)**：
一个库在当前投影下用来生成模型的那一版数据：调用方显式指定的会话号 / 时刻（历史投影、增量窗口右端），或——未指定时——库文件此刻自报的最新会话。它不是应用水位：水位只说数据已摄入，生成时点不从它取；完成凭证记的是模型按哪一版生成，判「当前」看它是否**不早于**要求的版本，不判等值。它也**不是模型水位**（ADR-060）：这一个记「这份几何按哪一版**文件**算出来的」、判不早于，那一枚记「到哪一版**数据窗口**为止数据与模型一致」、判等值；模型按文件最新生成到更新的版本时，两者取值本就不同。
_Avoid_: 生成水位、把 applied_sesno 当生成时点、拿它当库级追平判据

**模型水位 (Model Watermark)**：
某 `dbnum` 上与应用水位并排、写在同一行（`dbnum_watermark:{dbnum}.model_sesno`）的第二枚水位：模型已经追平到哪一版**数据窗口**（ADR-060）。它是**被写下的承诺**而非查询时算出的派生量——只在本任务的全部根生成并序列化成功、该库无未安定 `gen_root` 行、且 `T <= applied_sesno` 时，由该 dbnum 任务收尾的那一笔事务写下（ADR-071 D4；此前是模型排空的收口在根发布 CAS 之后写），单调推进。永不超过应用水位；按需 `ensure`、启动退化窗口生成的根都**不推进**它。`room_membership=true` 时承诺包含房间归属，房间算不出会使该根失败；`room_membership=false` 时只承诺模型几何，根与水位照常推进，库上仍没有第三枚房间水位，主动降级由 `/health.room_membership` 单独表达。判「这个库现在是不是最新一致」用等值：`applied_sesno > 0 且 model_sesno == applied_sesno 且无未安定根`；该结论是否包含房间必须与 `room_membership` 并读。落后是常态不是告警（ADR-056 D1/N4）。生成根按四格分类且互不重叠：**已追平 / 还在赶（拦水位）/ 已放弃（死信或 `http_422`，不拦水位但必须并排陈述）/ 没生成过（懒生成，不拦水位）**——已放弃根若算进「还在赶」，一个坏根就让整库的灯永红且看不出被谁卡。**它是仪表不是闸门**：没有任何代码路径可以等它、轮询它、或以它为条件放行另一条链，两条链的异步性一个字不改（ADR-060 M7）。
_Avoid_: 模型凭证下限（那是它的校验者 `settled_floor`）、生成水位、模型进度、把它当「模型有多新」的答案、拿它当同步点

**数据支撑 (Data Backing)**：
应用水位的对偶约束：`applied_sesno > 0` 时，该 `dbnum` 必须在 `pe` 里存在至少一行数据，或持有与当前应用水位相等的空基线凭据。判定用存在性查询而不是计数；通常在「基线还是增量」的路由决策上使用，启动重扫遇到水位恰好追平文件时也在入队裁决前使用，以免数值早退漏掉幽灵水位。查询失败必须上浮为批次失败或形成可见的本轮扫描失败，不得吞成「有数据」或「没有数据」中的任何一种默认值。
_Avoid_: 行数校验、pe 计数、数据存在性（泛指时）

**空基线凭据 (Confirmed Empty Baseline Credential)**：
一次成功的全量基线明确解析为合法空库时，与应用水位在同一事务写入的 `confirmed_empty_baseline_sesno`。它只在等于当前 `applied_sesno` 时证明零行是合法结果；非空基线清除它，水位继续推进后旧值自然失效。
_Avoid_: 空库标记、零行豁免、空水位

**幽灵水位 (Ghost Watermark)**：
`applied_sesno > 0`、该 `dbnum` 在 `pe` 里零行且没有匹配空基线凭据的状态——应用水位声称落库过，库里却没有数据支撑（多由 `dbnum_info_table` 播种回填或历史解析中断留下）。处置是路由到首次导入重建基线，不从水位往后接增量；它不是文件异常，不进 `FileAnomaly`，扫描阶段也不删任何数据。
_Avoid_: 假水位、虚水位、水位漂移

**模型水位失去支撑 (Unbacked Model Watermark)**：
模型水位的对偶约束（ADR-060 D6，形状复刻 ADR-021）：支撑值 `supported = 有未安定根 ? 0 : min(settled_floor, applied_sesno)`，`model_sesno` 与它不等就改写成它。**对账是双向的**——下调是「失去支撑」（水位比证据高），上调是「热路径漏记」（模型跑在数据前面那一次跳过、或收口前被 `clear_stale_at_process_start` 清掉那一次）；上调不是凭空盖章，`settled_floor` 就是证据、`applied_sesno` 封顶。落点在启动退化窗口那一轮，两个方向在日志与 `/dbnums` 的 `warnings` 里用**不同措辞**点名。承诺（`model_sesno`）与校验者（`startup_coverage_floors` 的 `settled_floor`）分工固定：只有前者露给读者。
_Avoid_: 凭证不一致、模型水位漂移、把 `settled_floor` 当第二枚水位露出去、把上调当成盖章

**幽灵模型水位 (Ghost Model Watermark)**：
`model_sesno > 0` 而该 `dbnum` 的 `gen_root` 零行——水位声称模型追平过，模型面却一行记录都没有。处置是清零 + 出声，按 ADR-058 D6 走懒生成；**不删数据、不阻断、不触发重建**。它与幽灵水位不同：那一个意味着数据没了、必须重建基线，这一个只意味着模型没了，而模型本来就可以按需重新生成。
_Avoid_: 幽灵水位（那是数据面的）、模型缺失、按幽灵水位那套处置

**兼容接管 (Legacy Adoption)**：
对「已有数据的库」做兼容初始化时，模型水位那一半的缺省处置（ADR-060 D10，用户 2026-09-10 拍板「模型水位 + 房间水位默认按数据水位保持一致」；其中房间那半句随 `room_sesno` 退役，接管本身保留，ADR-073 D7）：`applied_sesno > 0`、`model_sesno` 从未写过、且旧管线留有模型投影（`inst_relate` / `tubi_relate`）的库，启动时把 `model_sesno` **填成** `applied_sesno`（只填空、不贴时刻），此后在对账里是一致、在启动核对里按 `legacy_adopted` **跳过恢复 Task**——不再为「兼容一个已有模型的库」付一次全库模型重建。数据在、模型一行没有的库不接管：显式记 `model_sesno = 0`，照旧由模型面生成。它是数据面 pe 兼容播种（ADR-001）在模型面的对偶，排在索引与 `anc/dbnum` 回填之后、启动模型核对之前。`NONE` 与显式 `0` 分开：后者是判定，播种不改写。
_Avoid_: 模型水位播种（只说了接管那一格）、把接管当成盖章 / 完成凭据、把数据在模型无的库也接管、把显式 0 当成没写过

**库一致性判决 (Dbnum Consistency Verdict)**：
对一个 `dbnum` 回答「此刻数据与模型是否一致」的**单点结论**，三态：**一致**（应用水位已建立、模型水位与之相等、且没有「还在赶」的根，ADR-060 D4）／**落后**（前述任一不成立，附四格计数）／**不判**（没有可判等的水位对：读透形态下数据面不摄入、应用水位恒 0；或摄入形态下数据水位尚未建立）。「不判」是中性陈述，不是落后、不是告警。结论由同一行上的两枚水位与该库生成根的实时四格（已追平 / 还在赶 / 已放弃 / 没生成过）合成；「已放弃」与「没生成过」不否决结论，但必须与它**并排陈述**（ADR-060 D5）。它是读者（面板、健康页、批次回执、启动日志）唯一该消费的一致性口径——任何读者都不得自己拿水位列、凭证下限或队列计数重新拼一个。房间归属只在 `room_membership=true` 时属于这项承诺；关闭时“一致”仅指数据与模型几何，读者必须同时展示 `room_membership=false`，不得把它解释成房间也一致。
_Avoid_: 模型追平（只说了一半）、`model_caught_up`（行内那一半的名字）、把 `settled_floor` 露出来当判决、面板自己算一致、把「不判」画成落后

**重建批次 (Reinit Batch)**：
检出文件回退、发现残留写入标记（见「未完成写入重初始化」，ADR-073 D3）或人工点名重新解析（`POST …/data/reinitialize`，ADR-073 D5）后入队的数据批次，按首次导入形状（窗口 `1..file_latest_sesno`）排进同一条数据批次队列。扫描只分类入队、不删数据；worker 出队后在冻结点复核（回退仍成立 / 标记确在 / 人工请求且无身份歧义），才执行 `wipe_dbnum_for_reinit` 整库清空（水位行清值不删行、两枚水位归零、spatial epoch 同阶段递增），随后按首次导入重新解析，应用水位随重建归零再重新建立。它是**唯一**的恢复形态：没有按 sesno 的部分回退（ADR-073 D3）。
_Avoid_: 重建任务、对齐批次、realign 批次

**本期执行范围 (Update Scope)**：
增量摄入认哪些库的**唯一 MDB 口径**（`update_scope.rs`）：当前 MDB 的 CURD 里 STYP=DESI/ISOD 的库号，取**声明口径**而不是「已解析出 SITE 的库」——MDB 列了却从没导入过的库正是 `initialization_required` 那一档，该出现在范围里等人确认。库侧 `MDB` 表为空（全新命名空间）时回落到 `init_mdb` 已 `install` 的文件侧成员名单（同一判据 `DESI_STYP | ISOD_STYP`）。SYS meta（SYST/DICT/COMM/MISC/GLB/GLOB）不受它门控——成员名单本身就存在这些库里，挡掉它们范围定义会自己冻住。`manual_db_nums` / `exclude_db_nums` 自 2026-08-06 起不再参与这个判定（issue #10），只供全量模型生成与按需基线解析。它与**监听限定域**是**叠加**关系，不是二选一：数据批次入队走 `in_scope_with` 四道门串联（外项目运行态 sys 永不进 → 监听限定 → 调试限定 → MDB 范围），`[库一致性]`、模型凭证核对、`/health` 与按需 `sync_and_seed_model_coverage` 同样套监听门，所以**增量数据面的有效范围 = MDB 声明的 DESI ∩ watch_dbnums**（2026-09-08 共识 d-25）。限定时 `[init] N 个库被 watch_dbnums 监听限定跳过`、`[库一致性]` 只出域内几行，是**预期输出而非缺陷**；审核与改动都不得拿「MDB 全量」去要求数据面。已知且允许的残余：范围之内声明了、盘上却没有文件的设计库，文件驱动的 `sweep_dirs` 在三个「跳过」桶里都不会碰到它，只在整面重扫收尾点名一句（「没有解析到文件」，不记 blocker、不改范围定义），此外只有骨架装载的 `problems` 与手动预览的 `not_in_project` 看得见它。
_Avoid_: 增量范围（泛指时）、MDB 范围（当指监听限定域时）、按 MDB 全量核对数据面、把 `watch_dbnums` 当范围定义、把 `manual_db_nums` 当范围

**监听限定域 (Watch Scope)**：
把增量摄入的主 DESI 数据批次收窄到指定 dbnum 的调试开关（`DbOption.toml` 的 `watch_dbnums`，命令行 `serve --watch-dbnum` 压过它）。它限制的是范围、便于调试，不是生产形态的服务边界；但**初始化启动同样受它约束**——限定时启动的数据摄入、模型凭证复核与派生工作只覆盖域内的库，未限定时启动照常覆盖全部候选库。它叠在**本期执行范围**之上而不是替代它：有效范围是两者的交集（d-25），域外的库在 sweep 里落「监听限定跳过」那个桶，**不**进「不在 MDB 声明名单里」也**不**进「声明了却没解析到文件」那两句——它们不是没文件、不是不在名单，是本轮没有问过。SYS meta（SYST/DICT/GLB/GLOB）不受限；两者都没给时判定与引入前逐位相同。监听限定域不是依赖隔离域：被监听 DESI 的生成根实际引用到的 CATA 文件仍参与身份与项目优先级裁决，但 CATA 只从文件读（`ModelTarget.catalogue` 文件身份摘要与 `E3dDbResolver`），不解析进持久层。与**调试限定域 (Debug Scope)**（`--debug-dbnum`，只能从命令行来、额外带六个裁决点的链路追踪）是两个独立开关，与已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 无关。三种「跳过」（MDB 范围判定 / 调试限定 / 监听限定）在日志、回执与 `/health` 上各有各的嗓音，不得混同。
_Avoid_: 监听白名单、dbnum 过滤、增量范围（指 MDB 声明范围时）、watch 名单、服务边界（它不是）

**初始化准入门 (Initialization Admission Gate)**：
自动发现要不要收**从未初始化**库（任何来源都解析不出应用水位，`initialized = false`）的那道门
（`DbOption.toml` 的 `auto_initialize_new_dbnums`，**默认 false**；环境变量
`AIOS_AUTO_INITIALIZE_NEW_DBNUMS` 双向可压，`serve --auto-initialize-new-dbnums` 单次打开）。全部自动
发现路（启动重扫 / 周期对账 / watch 文件事件 / scope-refresh / 共享盘重挂）共用 `discover_batch` 里的
单点判定 `automatic_discovery_admits`：已初始化 ∨ 开关开 ∨ 已在队列，三者皆否才拦（已入队的行不被
后续重扫撤下——接受了的手动初始化要活过 Meta → Design 清单刷新）。默认形态下新库不入队：重扫收尾
汇总一句、`/health` 点名最近一轮拦下的库（重扫快照，整面覆写、子集补挂只并入）、`/dbnums` 逐行标
`awaiting_manual_initialization`（按水位现算），由 plant-ui 手动预览 / 执行（`/update/preview` +
`/update/execute`，不看本门）或规格工具（`bin/initialize_ams_dbnums`）初始化。开着时是历史行为：重扫
把新库按首次导入自动入队（`needs_initial_load` → `initialize_dbnum_baseline`）。它门的是**冷初始化
准入**：不是范围（MDB 声明 / 监听限定域）、不是消费（`startup_autorun` / 增量阶段控制）；已初始化库
的维护（增量窗口、回退重建 Reinitialize、幽灵水位修复）与按需 ensure 一概不归它管；SYS meta 不豁免
——全新部署由一次手动全范围执行引导（SYS meta 永远随批）。
_Avoid_: 新库自动初始化开关（当指这道门时，词条以此为准）、初始化白名单、冷启动开关、把 `startup_autorun` 当它用、把它当范围定义

**模型增量类型限定 (Model Incremental Type Scope)**：
把**模型增量的消费**圈到指定类型生成根的调试开关（`DbOption.toml` 的 `model_incremental_types = ["BRAN", "EQUI"]`，环境变量 `AIOS_MODEL_INCREMENTAL_TYPES=BRAN,EQUI` 压过它；不写 / 空表 = 不限类型，判定与引入前逐位相同）。它是**消费门，不是计划门**：任务的单元清单里只有 `regen_root` / `delete_cleanup` 两种单元按 `noun` 放行，名单外类型的根照常规划、照常进清单、`gen_root` 照常标 stale，只是本进程不生成它——因此凭证不前移、重启后随退化窗口重排、名单一撤即照常生成（判据 `model_type_scope::admits`；模型意图队列表随 ADR-072 D5 退役后，放行点从「领行」改到任务执行该单元那一刻）；在计划层摘掉工作项反而是错的（不标 stale = 下一窗给陈旧几何前移凭证，且永不回补）。数据增量（窗口收集、`pe` / `ATT_*` 写回、水位推进）一律不受影响；`Transform`、`CascadeExpand`、`PostRegenAabb`、按需 `ensure` 与 noun 未知（空）的行不看名单；房间归属随根算，没有单独的房间行（ADR-073 D1）。与**监听限定域**（按库圈数据批次与模型工作单，动数据面）、**最小交付单元**配置 `delivery_unit_types`（决定生成根怎么选，改变根的归并）三者互不代替。护栏与监听限定同一套：启动横幅、`/health` 的 `model_incremental_types` 栏、preview / execute 回执 `warnings`、数据窗口计划 `warnings` 按类型点名本窗暂扣的根、worker 空闲轮首次一句，都点名配置键与来源，且措辞与监听限定 / MDB 范围判定无交集。
_Avoid_: 模型类型过滤、noun 白名单、类型筛选、MDU 限定（指 `delivery_unit_types` 时）、模型范围（指监听限定域时）

**候选库文件 (Candidate Database File)**：
监控目录里被认可为「一个 AVEVA 库文件」的物理文件：既不在扩展名/系统文件黑名单里，文件名又合 AVEVA 库命名（三位项目前缀 + 库号[ + 四位序号]，或 `<前缀>sys/com/mis`）。判定只看名字，不读文件头。人手复制的副本（`… copy`、`…_old`、带日期后缀的备份）与正本头部一字不差，只有命名规则分得开它们；把副本当候选会让同一个 `dbnum` 拿到多个候选而整库被判「同号重复」阻断。同项目主库（无后缀）与唯一 `_NNNN` 抽取是抽取树的父层与叶子，归并成一个逻辑库，不是同号重复；多个 `_NNNN` 仍是 Duplicate。自动发现与手动扫描必须共用同一个判定。
_Avoid_: 库文件、数据库文件（泛指时）、排除规则

**文件观察 (File Observation)**：
收集层为一个候选库文件记下的、某一时刻的文件系统事实：路径、修改时刻、大小、观察时刻与观察来源。它只陈述文件层面看得见的东西，不含库号、库类型与最新会话号——那三样都要读文件头，是分析层的裁决。文件变化与周期全集轮产出同一种观察，区别只在覆盖面是子集还是全集；同一路径的多条观察可合并成最新一条，因为它描述的是状态而不是发生过的事。
_Avoid_: 文件事件、变更消息、notify 事件

**登记文件身份 (Registered File Identity)**：
系统为一个 `dbnum` 认可的数据库类型与物理文件身份。它只在首次登记或确认唯一、同类型且水位不回退的路径迁移时改变；阻断异常中的候选文件不是新的登记身份。
_Avoid_: 最近扫描文件、观察到的文件身份

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

**dbnum 任务 (Dbnum Task)**：
一个 `dbnum` 的一次增量就是一个任务：数据窗口、模型重算（`room_membership=true` 时每根算完几何紧接着算房间归属，关闭时跳过房间，ADR-073 D1）、凭证前移与模型水位收尾在同一个任务里一气做完（ADR-065 D1）；任务末没有整库房间段。模型欠账由应用水位与模型水位回答，关闭期间的房间欠账不进水位，靠开关状态与显式模型重建处置。稳态任务跨库并行、同库串行；初始化意图的任务按入队先后串行。它同时是**隔离单元**：同一 `dbnum` 的库行在任一时刻只有它的任务一个写者，所以写库不需要 CAS、ACK 或收口（ADR-071 D1）。
_Avoid_: 数据批次（当指含模型与房间的整个执行体时）、模型 drain、pending 任务、模型积压

**任务内序列化 (In-Task Serialization)**：
dbnum 任务在结束前把本任务在内存里算出的数据行、模型行、可选的房间边与两枚水位**同步**写进 SurrealDB，写库期间由写入标记标出；库里的这份是内存结果的落盘副本，不是计算输入、不是调度中枢，任务内不回读库，任务与库之间没有队列（ADR-071 / ADR-072）。`room_membership=true` 时每根的 `room_relate` / `room_panel_relate` 变更与模型行同块同事务；关闭时该段为空，模型行仍按同一路径提交（合块可跨根、一根不拆）。数据水位在数据尾事务写，模型水位在任务收尾的一笔事务写，写入标记的 `finished` 与它同一笔。
_Avoid_: 异步备份、backup lane、发布队列、发布 run、ACK、收口、per-root CAS、发布阶段、文件轮次、房间水位、任务末房间段

**写入标记 (Write Marker)**：
一个 `dbnum` 在库里的一行写入状态：任务第一笔写库前置 `writing{S,T}`，任务收尾那一笔与水位同事务置 `finished`。它回答「这个库有没有一次没写完的任务」，不回答「欠什么」——欠什么仍由两枚水位回答（ADR-072 D1 / ADR-073 D2）。见到它停在 `writing` 的唯一处置是整库重初始化（见「未完成写入重初始化」，ADR-073 D3）；正常收尾的 `Partial` 也置 `finished`，不算没写完。
_Avoid_: attempt 行、增量尝试记录、pending 状态、锁

**未完成写入重初始化 (Unfinished-Write Reinitialization)**：
启动水化段或任务开工前见到某库的写入标记停在 `writing{S,T}`（五类意图一律），对该库**只有一种处置**：本批就地转 `Reinitialize`——`wipe_dbnum_for_reinit` 整库清空（数据、模型、房间边、`gen_root`、两枚水位归零、`state_revision + 1`、spatial epoch 递增），再按首次导入重新解析当前文件、整库重生成模型（含逐根房间归属）；进程内同时作废该库的水位镜像、`GeometryBaseline`、一致性普查缓存与模型投影，空间树置脏（ADR-073 D3）。不按 `(dbnum, sesno > S)` 谓词删痕迹、不排 S→T 重放、不分面。它与「重建批次」是同一条 worker 路（清库仍只在冻结点，ADR-021 §3），差别只在触发源是残留标记而不是文件回退。正常收尾的 `Partial`（标记已置 `finished`、某一面水位没盖）**不**算未完成写入，照 ADR-071 D3 由下一次任务重做。
_Avoid_: 回退到水位、按 sesno 回退、部分回滚、分面回退、还原到快照、把 `Partial` 当成未完成写入

**库读白名单 (Database Read Allow-list)**：
对 SurrealDB 的读只允许水位 / sesno / 写入状态这类控制面信息、启动水化一次、字典一次；其余对库的一切操作是写或删。「先删后插」的删那一半也是谓词删，不是查出全集再点名（ADR-072 D5）。
_Avoid_: 对账读（泛指时）、回读确认、探针

**内存镜像 (In-Memory Mirror)**：
进程内每个 `dbnum` 一份的两枚水位、`gen_root` 凭证下限、安定根集与失败根集的工作副本：启动时从库反序列化一次，任务收尾写库成功后更新；运行期一切比对（要不要开窗、S 是多少、哪些根安定、四格）都读它，不再逐次查库（ADR-071 D5 / D7，ADR-073 D2）。真值仍在库里，镜像只是它在内存中的等价物。
_Avoid_: 本地账本、缓存（泛指时）、第二份真值、水位探针

**ModelOnly 任务 (ModelOnly Task)**：
只有模型半边、没有数据半边的 dbnum 任务：起点是该库的安定凭证下限、终点是文件最新会话，排进同一条数据批次队列、由同一个 worker 执行（ADR-065 D5）；`room_membership=true` 时模型半边含每根房间归属，关闭时只生成模型。启动退化窗口排出的算初始化意图、阻就绪；`model/rebuild` 与读透文件事件排出的算稳态、不阻就绪。重新打开房间开关不会制造模型水位缺口，历史房间欠账需显式 `model/rebuild`。同族的 **`RoomOnly`** 是生成路径上的**内部**意图：同一条逐根路径、跳过几何生成，用已有几何对该库全部根重算房间归属并重写两张边；它只在面板（房间定义）自身变化时由本窗变更集触发（d-819，见「逐根房间归属」），不是对外契约、不引入房间水位，失败语义同逐根。
_Avoid_: 模型恢复批次、退化窗口任务、全库过期入队、model drain 页、房间任务、`rooms/rebuild` 对外口

**联动更新 (Cascade Update)**：
元素 A 变了、**A 自己以外**的模型也必须跟着重算的那些边。全仓一共十一类（容器位姿→子树、删除→子孙、改挂→新旧两根、CATA→引用它的 DESI、连接/邻居、派生几何单元、房间定义→该库全部根的房间归属、空间树/AABB，加上明确不做的 UDA 依赖递归与 XRPNTR、以及在案未做的 `graphicsBehaviour` 中间祖先）。房间那一类的现行形态（ADR-073 D1，d-819）：构件自己的归属随它的根一起算、不是联动边；面板 / 房间节点的增、删、改名、换属主出现在本窗变更集里时，给该库排一条内部 `RoomOnly`，用已有几何对该库全部根重算归属——不重建结构触发账本、不另设待办行。**唯一索引是 `docs/2026-09-04_model-cascade-update-inventory.md` §1 那张表**——新增或改动任何一条联动边，先改那张表。它不是「一次变更要重算多少东西」（那是扇出），也不是「谁是生成根」（那是上卷）。core 侧的对应物是 `change/EVALAT` 那张 noun × 属性 → 目标的规则表（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md`）：owner 链上卷那一半我们全有，**引用类联动（结构连接 `CREF`↔`JOIS`/`JOIE`、孔洞反向引用）在 direct 文件窗口路径上缺**，真库普查坐实其中结构连接是洞（清单 #12，对齐计划 `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` C2）；0907 结构连接那一半已接线——`src/data_interface/structural_connection.rs` 从变更元素正向读、核对回指、把对端的生成根并进 `touched`（开关 `model_structural_connection_cascade` 默认关）；孔洞那一半 core 自己也只看 owner 子树成员（`DFIND` 是 dabacon 成员导航，`EVALAT` 自己没有反向引用调用），整根重生成构造性覆盖、不接；core 侧仅剩的反向尾巴是电缆两条——`CWBRAN` 的 `CLIST`、`EQUI`/`ELCONN` 的 `ELCXRF` 都是伪属性、底下是引用表反向查 `STAREF`/`ENDREF` 指向的 `CABLE`（0907 C6，证据 §8.6），同日 C8 同库版接线——`src/data_interface/cable_endpoint.rs` 到 T 端文件自带的 `STAREF` / `ENDREF` 反向表点查引用者、把电缆的生成根并进 `touched`（开关 `model_cable_endpoint_cascade` 默认关；跨库边待跨库版），电缆几何落地前几何上不显。
_Avoid_: 级联（泛指时）、扇出、反查、依赖传播

**图形行为 (Graphics Behaviour)**：
noun 字典字段 `graphicsBehaviour`（字段号 5099119，`DB_Noun` 偏移 180），E3D 3.1 的 1931 个 noun 分布 0/1/2/3 = 1652/109/98/72。四档语义：`0` 普通可画件；`1` 几何数据容器（`LOOP` `PFACE` `PSURF` `D*`…，本身不是独立可画项）；`2` 层级容器（`WORL` `SITE` `ZONE` `ROOM` `STRU*`…，永不单独重画）；`3` 照画、但画法由应用侧专用绘制路径提供（电缆桥架 / 船体型材 / 辅助图元 / 标注 / 网格）。**只有 `1` 和 `2` 需要守卫**：全产品没有一处把它与 `3` 比较，`3` 对每个消费点都与 `0` 等价（取证见 `docs/2026-09-04_model-cascade-update-inventory.md` §2.3 / §2.5）。`3` 标出的是几何覆盖度问题，不是增量问题。
_Avoid_: gb 标志（当指语义时）、图形忽略位、XGEOMETRY

**模型生成结果 (Model Generation Outcome)**：
某个生成根最近一次成功完成的生成结论，包括存在可绘制模型或确定没有可绘制几何。它是已经发生的成功事实，不是任何待办或重试记录；生成失败不会用失败结果覆盖它。
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

**切片流水线 (Slice Pipeline)**：
基线初始化的并行形态。并行单位是**生成根**——它本来就有 per-根锁这一现成粒度；切片（ZONE）只决定「一次把多少数据拉进来」，是内存分片单位，不决定几件事同时做（按片并行要整片等最慢的根，长尾吃掉加速比）。一片一个 `mem://` 实例，片内的根共享本片实例。根级与根内 fan-out 共用同一个几何并发闸，额度默认等于物理核数，不再各自写死宽度。
_Avoid_: ZONE 并行、按片并行、分片并发

**内存批切片 (Memory Slice)**：
e3d 生成路径上生产端一次交给消费端的那一份内存批。装配端按源顺序收下已算完的根，攒够**任一预算**即封片交出：`model_regen_execution_group` 那一档根数（发布 cohort 宽度），或 `model_memory_slice_elements` 那一档元素数。单根不拆——根是发布的原子单位。两端隔一条**深度 1 的有界** channel（ADR-041 §8）：无界一样不阻塞，代价是内存无界，那是被否决的形态。与上一条「切片流水线」的 ZONE 切片不是一件事：那个是基线初始化的数据分片单位，这个是稳态生成路径的内存预算单位。
_Avoid_: 分块、chunk、execution group（指内存预算时）、ZONE 切片

**生成窗口 (Generation Window)**：
生成端 worker 最多能跑在装配端前面多少根（`model_regen_execution_group`）。它同时是两件事的答案：**无栏并行**——worker 不在 cohort 边界上停下来等这一批里最慢的那根（几何负载长尾，设栏等于每批都赔上尾巴），只有队头能挡住人；**内存有界**——在飞的已生成几何最多一个窗口，加上正在攒的那一片。窗口满时 worker 把许可**还回去**再退出，由装配端滑动窗口后重新派出，绝不攥着许可等（那是 ADR-052 开篇那个容量倒置）。装配严格按源顺序，所以切片边界与 `Required` 裁决与「谁先算完」无关，`geometry_workers = 1` 逐字节复现串行路径。
_Avoid_: 批、wave、in-flight（泛指时）、并发度（那是闸额度）

**几何 CPU 执行域 (Geometry CPU Domain)**：
几何计算跑的那个私有线程池（线程数 = 几何并发闸额度，**不是**全局 Rayon 池——那个被 `room_model` / `db_model` 的 `par_iter` 共用，宽度不归闸管，接进来就是第二套并发口径）。唯一入口 `run_gated_cpu`，只收**同步闭包**：闭包里写不出 `.await`，「攥着许可去 await 另一个过闸子任务」这条会死锁的写法在类型层就不成立。等库、等锁一律留在 async 侧，不占额度。fan-out 宽度只来自 `resident_workers(件数) = min(额度, 件数)` 个**常驻 worker 从共享队列动态领取**，不是静态均分（几何负载长尾，一块里摆两个重 BRAN 就把其余 worker 空在那儿），也不是「每件一个 task 挤信号量」。
_Avoid_: 线程池（泛指时）、rayon 池、spawn_gated_leaf（已退役）、静态均分

**insts_flat 失效协议 (insts_flat Invalidation Protocol)**：
维护 `inst_relate.insts_flat` 这份图遍历缓存的规则：**开销与变更量成正比，不与库规模成正比**。它取代的是不带 dbnum、谓词不可索引的全表清扫——那种形态对生成根数是平方级的，实测维护开销达到它所服务的那件事的 2.9 倍。缓存本身不容置疑（它把加载从 253.3s 压到 2.73s），被否掉的是维护方式。
_Avoid_: 平表清扫、缓存刷新、sweep（泛指时）

## 数据窗口写回（Window Write-back）

**稳态增量窗口 (Steady-state Increment Window)**：
在已有基线之上按会话区间推进的常规数据批次（dbnum × 会话区间）；它走数据窗口直写，也是模型凭证前移的适用范围。基线 / 冷启动 / 全量生成路径不是稳态窗口（见「分块解析基线」）。
_Avoid_: 增量批次（泛指时）、暂存窗口

**冻结源 (Frozen Source)**：
一次收集冻住的「哪个文件、哪一代、哪个会话」（`data_interface::frozen_source::FrozenSource`）：稳定文件身份（Windows 卷序列号 + 文件索引 / Unix dev + inode）、`dbnum` 与库类型、冻住的目标会话与打开长度。收集与持久化之间它必须还是同一个——同文件追加接受，原子替换 / dbnum 漂移 / 会话或长度回退一律拒绝；纯数据窗口（零模型候选）同样过这道门。它是 e3d-io 底座上的商品，2026-09-03 起取代 old-pdms-io 的 `SnapshotToken` / `DabaconSnapshot`。
_Avoid_: 快照 token、冻结快照（指 `DabaconSnapshot` 时）、文件指纹（那是 `direct_store::FileIdentity` 的 len+mtime）

**净窗口收集器 (Net Window Collector)**：
数据面稳态增量窗口**唯一**的收集器（`data_interface::e3d_collector`，2026-09-03 起为 e3d-io 底座）：`IndexDiff(base@S, target@T)` 给每个 refno 的净三态，两端记录经 `to_named_attmap` 定形后 `diff_ele_data` 合成，每 refno 恰一条 `Add / Deleted / Modified`，挂 last-touch 会话。同一 refno 换了 noun 按 `Add` 写终稿并显式删旧 noun 行（`RetiredTypeRow`）；`shape_conflicts` 非空整窗阻断；不做任何成员关系仲裁——两根索引差分本身就是终态。old-pdms-io 的同名实现 2026-09-04 已整体删除（连 `legacy_pdms_io` 门都没了）；收集器对不对的**判据是 E3D TTY**——TTY 宏录下的会话链（`db8000_session_pairs` 夹具、issue-019 快照）与 TTY 现场往返（`Test-TtyNetWindow.ps1` / `verify_db8000_bran_window.ps1`），`shadow_collector_parity` 的 e3d-model L2 臂只是两条 e3d-io 合成路径互查，不是判据。
_Avoid_: 会话回放（`collect_changes`，已删）、拿第二套解析器当对拍参照、`net_window`（指 old-pdms-io 模块时）、成员补删

**数据窗口直写 (Direct Window Write-back)**：
稳态增量窗口唯一的数据写回路径（ADR-056）：CATA 依赖块（见「元素批」/「依赖入库」）→ `persist_latest_main_data` 按 `tx_limits` 分块事务写属性行（元素批：先删后插）→ `invalidate_caches` → `maintain_reverse_index`（失败入补偿队列）→ 窗口语句批 → `finalize_attempt` 尾事务最后推进应用水位。任一块失败水位不动、整窗口按同一区间幂等重放；非水位读者可见秒级中间态，以水位为准的读者把「半个写回」视为不存在。不经任何暂存介质。
_Avoid_: 暂存写回、journal 重放、应急直写（它已是唯一路径）、direct 模式（那是数据面读路由的名字——2026-09-04 起不设 `AIOS_DATA_READ_MODE` 即 direct、只有显式 `=db` 才退回 legacy，见 ADR-053 / ADR-058 D8——与这条写回路径不是一回事）、逐元素 UPSERT（2026-09-06 起不是它的写法）

**元素批 (Element Batch)**：
rocksdb 形态下元素数据行（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` 边）**唯一**的写法（`data_interface::element_batch`，plan 2026-09-06 §3 D1/D2，共识 d-29）：一块 = 前置块（Add / 墓碑自己的旧出向边）之后，「按 id 批量删边（id 区间）→ `ATT_UDA` → noun 行，再每表一条 `INSERT IGNORE`（边表 `INSERT RELATION IGNORE`）」。**`pe` 行不删**——它是图顶点，删它会级联删掉 `pe_owner` / `ref_rev` / `tubi_relate` 这些别的块、别的窗口、模型面挂在它身上的边——改为一块一条批量 `INSERT INTO pe [..] ON DUPLICATE KEY UPDATE <gen_sur_json 全部字段>` 原地整行替换。两档：`Fresh`（基线 / 父层补缺，只插、已有行赢）与 `Replace`（增量 / 依赖行版本前移）。切块上限 `tx_limits`（2 MiB / 5000 行 / 1000 元素，`DbOption` 三键可调），每笔事务带 pe 统计观测。控制面单行（水位、`gen_root`、队列、统计、`datacenter_version`、`cata_dependency_state`）不在此列，仍是尾事务 / 回执块里的点写。
_Avoid_: 逐元素 UPSERT / UPDATE、`ATT_<noun>`（属性表名就是 noun 本身）、删 `pe` 行、`PEJson / AttJson / PERelateJson`（已退役的按表消息）

**墓碑 (Tombstone)**：
被删元素在 `pe` 上留下的那一行：`deleted = true`、`sesno` 是删除落地的会话、noun / owner / name 照基侧（`DbSet@S`）保留——`datacenter` 的 Deleted 分支、ADR-036 成员补删、`cleanup_deleted_by_pe_state` 读的都是它们。它的 `<NOUN>` / `ATT_UDA` 行与在 owner 名下的边随删除块消失（此前软删把它们留在库里）。基侧读不到时退化成只带 id / dbnum / sesno / deleted 的最小墓碑并在回执出声。
_Avoid_: 软删（当指「只翻 pe.deleted、别的行不动」的旧写法时）、删除记录

**模型凭证前移 (Credential Advance)**：
数据窗口 S→T 提交后，对 e3d-model `plan_update(S→T)` 判定**未被触到**、已安定且落后于 T 的生成根，一条语句批量把 `gen_root.source_end_sesno` 推到 T（行上记 `credential_advanced_from`），不重生成、不动几何 / manifest / revision。护栏：候选数超过索引键数的 30%，或 `plan_update` 报 `unresolved` 非空，即整窗放弃前移、全部根照旧过期，回执记 `credential_advance_degraded`。它动的是**根级**凭证，与库级的**模型水位**（ADR-060）不是一回事：前移只说「这份几何到 T 为止仍然有效」，水位说「整库到 T 为止已经追平」。
_Avoid_: 凭证刷新、水位前移、跳过生成、与模型水位混用

**窗口阻断 (Window Block)**：
稳态增量窗口**数据侧**确定性失败的终态（收集器报错、写回被持久层确定性拒绝）：该 `dbnum` 的水位停止推进、告警常驻，直到源数据或持久层修复后按同一区间重放。模型生成失败**不阻断数据窗口**（ADR-056 D1）：根级失败只留痕、不排队（ADR-065 D4），数据水位照常推进。房间归属开启时算不出会让该根成为模型失败；房间开关关闭时不运行计算，因此不产生这类失败。
_Avoid_: 死信降级、跳过坏根、模型阻断

**pe_owner 边 (Owner Edge)**：
父子关系在库里的唯一形状：`{ id: pe_owner:[owner, 槽位], in: child, out: owner }`（`ele_ops::pe_owner_edge`）。方向以读侧为准——rs-core `{owner}<-pe_owner` 取 `in` 当 children、`->pe_owner.out` 取 owner，唯一索引 `n` 也建在 `(in, out)` 上。全仓只此一个写法：基线路 2026-09-04 换成 e3d-io 退化窗口后经 `ElementRows` 调同一个函数，`cata_closure` 的成员块渲染与之逐字节同形（有源码钉对比两者的 INSERT）。2026-09-04 之前增量路的终态渲染器写反成 `in = owner, out = child`，写出的子树在树面板上读不到、`DELETE {pe}->pe_owner` 清的也不是自己的 children 边；受影响的库靠 `POST /model/rebuild` 重建，不手改数据。
_Avoid_: 父子边（泛指时）、`owner` 字段（`pe` 行上没有这个字段）、按 `out` 取 children

**分块解析基线 (Chunked Baseline Parsing)**：
基线是 e3d-io 的**退化窗口**（base 为空、target = 冻结目标会话），不是第二条解析路：`e3d_collector::stream_baseline` 枚举目标索引，每 `sync_chunk_size`（默认 10 万）个元素交一块给 `versioned_db::baseline::render_baseline_batches` 渲成**元素批**（`Fresh`，每条队列消息 = 一笔事务），经有界队列交单写者落库；块交出去就释放，峰值内存随块不随库。编排只有一处：`versioned_db::baseline_ingest::ingest_file`（**按单个文件**：路径 + 点名的 dbnum / db_type + 目标会话；文件头身份不符一行都不落；不动水位；解析 / 写者 / CATA 闭包 / 统计任一失败先 `wipe_dbnum_for_reinit` 再 Err；统计收尾无条件 `rebuild_on`）——2026-09-07 起 legacy 全量同步壳 `sync_total_async_threaded`（多类型 / 多文件 / `included_db_files` 过滤）与空库 SYS 引导 `bootstrap_sys_metadata` / `bin/sync_sys_only` 一并退役：SYS 元数据（SYST/COMM/MISC/GLB/GLOB → DICT）不是单独一道引导，而是初始化的一部分——服务侧由首轮重扫当普通库入队首载（`needs_initial_load` 对 SYS meta 同样成立）、之后 watcher 按会话增量追（增量对它们只落行与水位，不刷几何），不起服务的规格工具走 `AiosDBManager::initialize_project_sys_metadata`（只补没有水位的库，顺序 SYS 五类 → DICT）。行形状与语句形状都与增量路共用（`ElementRows` + `element_batch`），`Fresh` 只 `INSERT IGNORE`——`manual_update` 的父层补缺靠「已有行赢」保住叶子 extract 里正在改的版本，换 `Replace` 会改掉这条语义。流式收集时顺手攒每个 DESI 元素的目录链引用，文件收尾一次闭包把依赖的 CATA 元素经同一条队列入库（见「依赖入库」）。它**不走提交协议**：基线不开窗口，窗口买的是原子提交与崩溃恢复，不是内存有界。2026-09-04 随 `parse_pdms_db` 退役消失的三件事：把冻结前缀整个读成一个 `Vec<u8>`（`FrozenSource::read_frozen_bytes`）、`mem://` 暂存实例与它的读路由、`member_prune` 事后裁成员（目标索引差分本身就是终态）；`pe.sesno` 从链头 0 变成元素真实的 last-touch 会话。
_Avoid_: 暂存窗口（基线不开窗口）、全量解析 / `DbBasicData`（随 `parse_pdms_db` 退役）、`sync_total_async_threaded` / `baseline_sync_options` / `bootstrap_sys_metadata` / `bin/sync_sys_only`（2026-09-07 退役）、SYS 引导 / SYS 同步（它不是一道单独的步骤，说「SYS 元数据首载」）、按需解析（那是元件库的旧词）、`pe_chunk` / `att_chunk`（2026-09-06 起不参与切块）

### 已退役词条（retired，ADR-056）

以下词条只为读 2026-08-05 ～ 2026-09-02 之间的 ADR / 计划 / 证据时对照使用，**现行代码与文档不得再用**：

- **提交单元 (Commit Unit)**、**暂存库 (Staging Database)**、**暂存工作集 (Staged Working Set)**、**语句日志 (Statement Journal)**、**水位门控写回 (Watermark-gated Write-back)**、**commit-time-only 语句 (Commit-time-only Statement)**——ADR-017 kv-mem 暂存窗口的构件，随暂存层退役；现行口径见上方「数据窗口直写」。
- **窗口阻断的模型侧含义**（生成根重试穷尽 → 整窗口水位停止、持久层零痕迹）——被 ADR-056 D1 取代，模型失败与水位解耦。
- **成员关系仲裁补删 (ADR-036 Membership Reconciliation)**、**幽灵新增丢弃**——old-pdms-io 净窗口收集器在索引差分之外按成员表补判的删除 / 丢弃；2026-09-03 收集器换成 e3d-io 底座后随之退役（两根索引差分即终态，`membership_deleted` 恒 0）。现行口径见上方「净窗口收集器」。
- **模型窗口意图 (Model Window Intent)**——数据窗口尾事务写进 `model_update_pending` 的模型侧工作声明；2026-09-10 随 `model_update_pending` 整体退役（ADR-071 D9 / ADR-072 D6），任务的计划只在内存。
- **提交后收敛 (Post-commit Reconciliation)**——「水位已提交、空间状态尚未持久化完成、阻止下一批进入」的过渡阶段，就是 publish 屏障的名字；2026-09-10 随发布阶段退役（ADR-072 D4），空间树随模型行提交就地收敛。

### 已退役词条（retired，ADR-065 / ADR-071 / ADR-073）

以下词条只为读 2026-07-27 ～ 2026-09-12 之间的 ADR / 计划 / 证据时对照使用，**现行代码与文档不得再用**（现行口径见「逐根房间归属」「未完成写入重初始化」「任务内序列化」「RefNo 库归属」）：

- **回退到水位 (Rollback to Watermark)**——见到 `writing{S,T}` 按 `(dbnum, sesno > S)` 谓词删痕迹、三水位退回 S、排 S→T 重放（ADR-072 D2，含 d-570「不删 `pe` 顶点」）；2026-09-10 随「任何未完成写入 → 整库重初始化」退役（ADR-073 D3），连同 `DELETE …/data/above/{watermark}` 那半个运维端点（ADR-073 D5；GET 半边保留）。「三水位」「房间水位」两个说法随之一并退。
- **房间增量 (Room Increment)**——「从文件更新轮次发布到 `RoomRecalcPanel` / `RoomRecalcElement` 结算」的整条流程，含几何目标 + 结构触发、发布屏障、消费与结算；随房间流程整套退役（ADR-073 D1）。硬验收「增量收敛 == 全量重建，逐边」不退，挂在「逐根房间归属」上。
- **已发布房间几何 (Published Room Geometry)**——`room_published_inst` 里由 `published_file_input` 清单选中的版本化行，房间增量的两条分支只读它；三表已 `REMOVE TABLE`（ADR-072 R5 / D6，ADR-073 D6）。
- **同轮吸收 (Same-round Absorption)**——同一轮内整间分支已覆盖某构件时跳过其元素任务的省算优化；没有「轮」也没有元素任务（ADR-073 D1）。
- **房间重建要求 (Room Rebuild Requirement)**——结构面板枚举失败时随数据水位写入 `room_build:main` 的持久降级标志、只由下一次成功的启动全量房间重建清除（ADR-040 D5）；结构触发账本与启动全量房间重建都退（ADR-073 D1；ADR-040 作废）。
- **文件更新轮次 (File Update Round)**、**已发布派生输入 (Published Derivation Input)**、**空间发布版本 (Published Spatial Version)**——ADR-062 文件轮次屏障 / 发布视图 / manifest 的三个构件（冻结终点 `T` 的更新单位；空间树与房间算法可消费的最后成功文件版本集合；候选空间变更经意图重放、快照落盘、切树后的可消费版本）；ADR-065 D8 起退役、ADR-072 R3 删除。空间树今天随模型块提交就地收敛，快照头记各库 `(applied_sesno, model_sesno)`（ADR-072 D4，ADR-073 D1）。
- **待重试单元 (Pending Model Unit)**、**revision 收口 (Revision-safe Settlement)**——`model_update_pending` 里持久化保存的独立模型任务（按需生成保护记录、跨窗口派生工作、房间任务积压，以动作 + 目标为身份）与「以该行队列内部单调 `revision` 为条件完成删除 / 标记失败」的收口规则；表与 drain 随 ADR-065 D3 / ADR-071 D9 / ADR-072 D6 整体退役（ADR-065 实施约束 4 点名的搬家），失败只留痕不排队（ADR-065 D4）。
- **Ref0 库归属 (Ref0 Database Affiliation)**、**Ref0 库归属缺失 (Unresolved Ref0 Affiliation)**、**Ref0 库归属冲突 (Conflicting Ref0 Affiliation)**——「项目内每个 Ref0 唯一归属一个 `dbnum`、`Ref0 → dbnum` 靠全库扫描建表反查、同一 Ref0 落两库即冲突」那套说法；2026-09-12 随 RefNo 原生路由退役（母计划 `docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`，切片 W1–W7 提交 `bb16105d2` / `5cc5e3e04` / `425ca98d8` / `7f501ab0d`）：dbnum 从 refno 位里解，core.dll 里从来没有这张表；「缺失」变成 `InvalidRefNo` / `DbNotInMdb` / `NoFileForDbnum` 三种明确错误，「冲突」在位解码下不存在（同号跨项目的文件歧义由「dbnum 路由目录」的 `preferred_project` 与 `initialization_phase` blocker 处置）。现行口径见上方「RefNo 库归属」「dbnum 路由目录」「跨库属性」。`DirectStoreError::UnresolvedRef0`、`model_source::DBNUM_BY_REF0`、`InMemoryCataLocator::build_for_project` 生产依赖已删；`MdbMemoryStore.affiliations` 本体留母计划阶段 6 删。

## 依赖入库（Dependency Ingestion）——2026-09-06 起，CATA 按 DESI 依赖入库

**依赖入库 (Dependency Ingestion)**：
每一批 DESI 写回（基线、稳态窗口、模型根发布）都以本批 DESI 元素为 seed 算**依赖闭包**，把闭包里的 CATA 元素与 DESI 行一起写进 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`（`data_interface::cata_dependency`，plan 2026-09-06 §3 D5–D7，共识 d-29；修订 ADR-058 D1 / S3，见 ADR-064）。seed 只取目录链引用属性（`SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU HSRO LSRO`，与 e3d-model `reference_reads_guard` 同源）；闭包复用 `CataClosureResolver`（容器子树只展开几何 / 点集 / 轮廓容器，owner 链到 WORL，只收 CATA、排除本批 DESI）；行经元素批 `Fresh` 入库，`pe.dependency = true` 标它是按依赖入库的——**不代表该 CATA 库整库在场**，整库 CATA 基线仍不做。数据窗口里它排在 DESI 块与水位之前（闭包解析出错拦水位，「库不在 MDB / 元素不在」只 warning）；基线经同一条写者队列；模型根发布前独立事务、只 warning。一个 DESI 元素落库而它的目录链在库里缺行，算缺陷。
_Avoid_: 按需解析 CATA（旧章的词，指惰性兜底那条路）、CATA 基线、CATA 水位

**依赖状态 (Dependency State)**：
`cata_dependency_state:{dbnum} { sesno, rows }`——这个目录库的依赖行是按哪一版目录文件渲的、累计送入了多少行（`rows` 是量级：`Fresh` 命中也计、前移重渲不计；精确数查 `pe … dependency = true`）。**不是水位**：不进 `dbnum_watermark`、不参与 ADR-021 的判据；`/dbnums` 露成 `dependency_rows` / `dependency_sesno`（没有依赖行的库整格不写）。目录文件前移时（文件最新 > `sesno`，只对本批闭包碰到的目录库、排在本批 `Fresh` 之前）由 `cata_cascade_window` 差分出变动目录件，与已入库依赖行求交，被改的 `Replace` 重渲、被删的落墓碑，然后推进 `sesno`；差分不可用（会话不在链上 / 候选超 20,000）退化成该库全部依赖行重渲，回执点名 `cata_dependency_full_refresh`。
_Avoid_: CATA 水位、`applied_sesno`（那是 DESI 的）

## 按需解析元件库（On-demand CATA）——旧章（retired，ADR-058 D1；2026-09-06 由「依赖入库」接替）

> 2026-09-03 ～ 2026-09-06 之间 CATA 不解析进持久层：模型面经 `E3dDbResolver` 从文件读，级联判据用 CATA 文件的
> 会话索引差分加 `ref_rev` 反查。2026-09-06 起 CATA **按 DESI 依赖入库**（上一章「依赖入库」，ADR-064 修订 ADR-058
> D1 / S3）——模型面仍从文件读，入库的依赖行服务属性面板 / `ref_rev` / 依赖备份。本章六个词条只为读
> 2026-07-27 ～ 2026-09-03 之间的 ADR / 计划 / 证据时对照使用，**现行代码与文档不得再用**（现行词是「依赖入库」
> 与「依赖闭包」）。CATA 的**文件身份裁决**（项目优先级、同号选主、`CataDbLocator`）从来不在退役之列——那是
> `E3dDbResolver` 与依赖闭包的输入，不是解析。
> **2026-09-07（ADR-064 Q2）**：本章描述的那套代码已从树上删除——生成根闭包播种（`collect_design_subtree_outbound` /
> `run_cata_closure_pass_for_refnos` / `ensure_cata_parsed_for_roots`）、resolve 层惰性兜底入口 `try_lazy_cata_fallback`、
> 依赖缓存 `CataDepCache` / `preload_cata_for_roots`、`cata_ref_rev` 补偿队列、env 开关 `AIOS_CATA_CLOSURE_MODE`、
> 开/关几何冒烟 `bin/cata_smoke`。留下的只有依赖闭包的引擎（`CataDbLocator` / `CataClosureResolver`，供「依赖入库」调用）
> 与 `ensure_cata_refnos_parsed` 的薄壳（落库走「依赖入库」，仅 staging 预加载还引用，随 spec 035 P3 一起走）。

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
构件与房间的从属关系，经面板物化为两类边：`room_relate`（面板→构件，带 `inside_count`/`center_dist` 排序字段）与 `room_panel_relate`（房间→面板）。材料表 surql 经 `fn::room_code` / `fn::room_num_of` 消费；是可事后重建的派生数据。写者只有生成路径：每根模型算完紧接着算它的归属，边与该根的模型行同事务落库（见「逐根房间归属」，ADR-073 D1）。两张表的定义与 `fn::room_code`（hd / hh 两版）/ `fn::room_relate_of` / `fn::room_num_of` / `get_room_nodes.surql` 一律不动（ADR-073 D4）。
_Avoid_: 房间关系、房间隶属

**逐根房间归属 (Per-root Room Membership)**：
房间归属的现行算法形态（ADR-073 D1 / D2）：一个生成根的完成 = 生成最终几何 → 用当前候选面板做点包含筛选、算出该根下每个构件属于哪些面板 → 模型行与 `room_relate` / `room_panel_relate` 边在同一笔事务写入。房间不是独立阶段：没有房间目标、房间轮、发布 / 结果目录、常驻模型换代、结构触发账本与房间水位；任务末没有整库房间段。某根的归属算不出来或写不进去 = 该根本轮未完成（非安定、凭证不前移、`model_sesno` 不越过），不排队、不自动重试。面板自身的变化（FRMW/SBFR 改名命中关键字、PANE 换属主、CWALL/CFLOOR 搬迁）不改几何、逐根路径点不着：按 d-819 由本窗变更集触发，给该库排一条内部 `RoomOnly`（见「ModelOnly 任务」），用已有几何对该库全部根重算归属并重写两张边，不重生成几何。硬验收沿用 ADR-010 §9：一组增量收敛后的两张边与同一份数据从零初始化的结果逐边相等。
_Avoid_: 房间增量（流程含义，已退役）、房间任务、房间阶段、房间收尾、房间刷新、房间同步

**模型发布策略 (Model Publication Policy)**：
与“模型从哪里读”正交的写目标判决。`ProjectionOnly` 只用于 ingest 库尚未就绪时的临时内存投影，避免与 durable dbnum 单写者竞争；read-through 的 spawned/embedded kv-mem 虽然读取源也是 memory，却是本进程正式的 `ProcessStore{durable:false}`，模型行、可选房间边与数据库根回执仍逐根同事务写入，只是不跨重启保存。不得再把 `scope_memory_only` 解释成“不写任何数据库”。
_Avoid_: 模型读路由（那只回答读取源）、把 kv-mem process store 叫持久库、memory-only 等于 projection-only

**完整根发布确认 (Root Publication ACK)**：
当前进程内对“候选几何已经完整发布”的确认：源会话、几何条数、房间开关、resident generation 与介质必须匹配。`RootReceipt` 只证明几何候选投影存在；模型行/房间边/process-store 回执事务成功后才写 ACK。几何重生成、删除、源换代或 resident 换代先使旧 ACK 失效。ACK 不落控制面、不推进水位、进程退出即丢，因此不是第三枚房间水位。
_Avoid_: 根回执（候选几何）、房间水位、durable ACK、发布队列

**kv-mem 房间启动屏障 (Kv-mem Room Bootstrap Barrier)**：
`room_membership=true` 时 spawned/embedded kv-mem 每次冷启动从空结果库开始，先冻结 resident generation，再按 dbnum 顺序覆盖 MDB 全部可读 DESI 的模型根与逐根房间发布，全部成功后才置 `model_ready` / `features.room.available`。它不受 `watch_dbnums`、`startup_autorun` 限制；失败保留 health 与诊断服务，但模型完整产出入口不放行。`room_membership=false` 保持快速路径。
_Avoid_: 房间阶段、房间队列、启动预热（只说 resident）、把 watch 范围当 MDB 完整性范围

**面板 (Panel / PANE)**：
房间的几何载体：`FRMW/SBFR 房间节点 → CWALL/CFLOOR → PANE` 层级中带盒状 mesh 的 PANE 元素。归属判定以面板 mesh 为容器做点包含检查；房间节点自身不参与几何判定。
_Avoid_: 板、墙板（泛指时）

**整间分支 (Panel Branch)**：
房间归属的粗粒度算法口径：给定一块面板，拿它的世界盒去空间树捞候选成员、逐个做点包含判定，得到**该面板的全部成员**（先删面板全部 `room_relate` 出边、再整批写回）；成员元素的包围盒取自空间树。它不再是一种调度单位——面板自身变化按 d-819 走该库整库 `RoomOnly`（ADR-073 D1）；作为算法条目的去留随 ADR-010 的处置一并定（ADR-073 §未决 U2）。
_Avoid_: 面板分支、全房重算

**元素分支 (Element Branch)**：
房间归属的细粒度算法口径，也是逐根路径实际走的那条：一个构件生成完，只算它自己属于哪些面板（先删它的全部 `room_relate` 入边、再写回本次算出的边），与它的模型行同事务落库（ADR-073 D1）。候选面板与构件包围盒都来自任务手里的内存几何——任务内不回读库（ADR-071 D1 / ADR-072 D5）；候选面板集的驻留形态见 ADR-073 §未决 U3。
_Avoid_: 构件分支、反向分支

**空间树 (Spatial Tree)**：
全库元素世界包围盒的进程级 R\* 树（`GLOBAL_AABB_TREE`），房间候选查询与整间分支的成员盒都取自它。带脏标记（`AABB_TREE_DIRTY`）：模型块提交后按内存投影的世界盒就地收敛并置位、删除清理置位（ADR-072 D4），空闲轮收尾原子写回 V3 单文件快照 `accel_tree_{project}.snapshot`，头部记各库的 `(applied_sesno, model_sesno)` 数对。启动时快照头与水位镜像逐库相等才复用；不等即从 `inst_relate.aabb` 指针重建——没有发布清单、文件轮次或空间门禁那一档（ADR-072 D4，ADR-073 D1）。
_Avoid_: R 树（泛指时）、AABB 树、加速树、树文件 `.bin`（V1 旧格式，已迁移删除）

**refno 反查 (Refno Lookup)**：
按 refno 问空间树「这批盒子在不在树上、旧值是什么」的能力，读路径与写路径共用 `AccelerationTree` 内部那张 `refno_index`（纯内存派生，不进树文件，加载后重建）。读路径若退回 `tree.iter()` 全量遍历，每个生成根前后各扫两遍整棵树，代价对库规模是平方级——并行化只是把平方项除以核数。禁止在调用方另建一份 refno → 包围盒映射：那会造出第二份「树上现在有什么」的真值，漂移的后果不是变慢，是把已经不在那儿的构件算进某间房，而这类错误没有任何东西会报出来。
_Avoid_: 遍历树查找、tree.iter() 找、自建 refno 索引

**空间版本号 (Spatial Epoch)**：
库侧单调递增的空间提交计数：每笔改变「树该有什么」的已提交事务（模型块提交、删除清理、整库清空）同事务 +1，没有单独的空间意图行（ADR-071 D9）。快照头记录落盘时刻的版本号；启动时与库侧不相等即判文件陈旧、改从库指针重建。只比相等、不表达次数，重复递增无害。
_Avoid_: 树版本、时间戳校验

**指针重建 (Pointer Rebuild)**：
从库中已提交的 `inst_relate.aabb` 指针分页整树重建空间树的只读路径；不重算几何、不回写库，与「重算修复」（重算几何并回写指针，手工工具）相对。
_Avoid_: 全量重算（此场景）、树重建（泛指时）

**AABB 变更集 (AABB Change Set)**：
包围盒刷新时与空间树旧值比对产出的空间差异集合。普通全量/维护刷新只按它触发；定向增量还会保守纳入实际重写/变换的几何目标。面板层级、房间命名等不改几何的变化不经它：按 d-819 给该库排一条内部 `RoomOnly` 整库重算归属（ADR-073 D1）。首次进入空间树也视为变化。
_Avoid_: 包围盒差异、aabb diff

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
房间归属随每个根的生成一起算，不是模型之后的一段（ADR-073 D1）。阶段完成以 manifest、水位和数据支撑为准，不以队列暂时为空为准。

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

## 本地发布（Local Release）

> 面向「把已发布的服务端 + Web 打成一个可分发压缩包」这件事的词汇；实现见 `scripts/New-LocalRelease.ps1`，口径见 `docs/adr/ADR-068`。

# 本地发布包 (Local Release Package)

`ams-local-<版本>` 一档：把某次已发布的后端 `aios-database.exe` 与 plant-ui Web 静态资源打成解压即跑的压缩包。runtime 档默认 `store_mode=embedded-mem`：kv-mem 在后端进程内，无 SurrealDB 子进程与数据库端口，浏览器读面统一走同源服务 API；包根仍保留本仓 fork 的 `bin\surreal.exe`，需要外部 SQL / 探针时可显式切回 `spawned-mem`。房间兼容页仍是直连库的保留功能，embedded 下必须明确报“不可直连”，不得退回配置里的旧 TCP 端口。`-full` / 带 workspace 档把依赖资源装进包根 `workspace\`，默认仍是 `spawned-mem`。两档首次打开都由向导配置 E3D 路径；需要持久化时显式改 `spawned-rocksdb` / `external`。版本号以两仓 `Cargo.toml` 为准，同名 zip 拒绝覆盖（`-Force` 才覆盖）。

_Avoid_: 安装包、部署包（泛指时不指这一档）、发行版。

# 依赖资源包 (Dependency Resource Bundle)

本地发布包 `-full` 档里 `workspace\` 那部分：`assets`（含 `meshes` / `fonts`）、`config`、`resource`、`search-index`、accel 快照、`DbOption.toml`、本仓 fork 的 `surreal.exe` + 起库脚本。**它是「资源进包」的那一半**；**库数据（SurrealDB `rocksdb://ams.db`）与源模型不在其内**（ADR-068 D3）——带它够离线看已生成模型的三维，但属性 / 搜索 / 补齐 / 更新仍要连回原库。

_Avoid_: 数据包、库包、离线数据（这些会误读成含库数据）。

# 冒烟门 (Smoke Gate)

本地发布组包后、压 zip 前的一道自动校验。runtime 档两腿：A 腿以出厂配置进入设置模式；B 腿以本机真配置和 `embedded-mem` 全启动，要求 `health` 200、首页含 plant-ui、`/health.sul_db.medium="embedded-mem"` 且后端没有 SurrealDB 子进程。带 workspace 档三腿：A 同前；B 以 `spawned-mem` 全启动；C 改 `spawned-rocksdb`@18010，要求 `medium:"spawned-rocksdb"`、`durable:true` 且 `workspace\.surreal\` 真落盘。端口被占或起不来则跳过 / 失败并在 `release.json.smoke` 与产物旁标「未冒烟」，不静默放行。

_Avoid_: 冒烟测试（泛指）、健康检查（那是运行期 `/health`，不是发布门）、验收（那是 Live E2E Testing）。

# HTTP 监听地址 (HTTP Bind Address)

`DbOption.toml` 的 `http_api_addr`：服务实际绑定的 socket 地址。`0.0.0.0:<端口>` 表示全部 IPv4 接口；它不应该被当作浏览器链接展示。

_Avoid_: 对外地址、分享地址、把 `0.0.0.0` 当成可导航主机名

# HTTP 对外展示地址 (HTTP Public Address)

`DbOption.toml` 的 `http_public_addr`：日志、自动打开与分享链接使用的可导航地址，不参与 socket 绑定。留空时从监听地址派生；首跑向导与重启轮询始终使用回环地址，不消费本键。

_Avoid_: 监听地址、绑定地址、反向代理监听

# 配置向导 (Setup Wizard)

exe 内嵌的单文件页 `/setup.html`（磁盘副本优先，模式同 `/ops.html`）：五步引导——①数据源（项目目录探测）→②存储（三档：连已有库带连通 + 版本硬闸 / 新建本机持久库带端口预检 / 试用内存库）→③服务（端口预检）→④生成与监听（初始化两档单选）+ 保存并重启 →⑤初始化（复用 `/dbnums` + `/update/preview` + `/update/execute` 的薄壳）。配置真值仍是 `DbOption.toml` 文件（`toml_edit` 外科手术写盘，注释与未知键保留，先落 `.bak`）；`/api/v1/setup/*` 按 **TCP 对端**回环限定，对外暴露须另加认证（ADR-069 D4 硬边界）。

_Avoid_: 安装向导（没有安装动作）、配置中心 / 配置页（真值在文件，不在服务里）、首跑向导（向导也服务于日常改配置）。

# 设置模式 (Setup Mode)

启动臂的一档：**配置无效**（`project_path` 缺 / 不存在，或需要端点的档位——`external` / `spawned-rocksdb` / `spawned-mem`——`v_ip` 空 / `v_port=0`）时，进程不连库、不扫描、不起 watcher / 队列，只绑 HTTP 供配置向导与 `/health`（`mode:"setup"`）。当前出厂模板靠空 `project_path` 进入此模式；kv-mem 地址已预填为 `127.0.0.1:8009`。判据是配置文件自身的形状，**不做网络探测**——库地址填了但连不上照旧走正常启动失败，不盖真故障。配置修好经重启口交接后，新进程自然走正常启动。

_Avoid_: 安全模式 / 降级模式（正常启动链的失败语义没改）、维护模式（没有这一档）。

# 首跑哨兵 (First-Run Marker)

workspace 里的 `.first-run-pending` 文件：在场 = 向导没走完，首页 `/` 302 到 `/setup.html`；向导「完成」时删除（幂等，删了无需重启即恢复正常首页）。由 `New-LocalRelease.ps1` 组包时落下。**只管跳转，不影响启动臂**——向导中途重启后要正常启动才能初始化，这是它与「设置模式」必须拆成两个判据的原因（ADR-069 D3）。

_Avoid_: 锁文件（不互斥任何东西）、初始化标记（初始化完成与否由库水位说了算，不由它）。

# 自带持久库 (Spawned RocksDB Store)

`store_mode = "spawned-rocksdb"`（ADR-070）：服务每次启动用包内 surreal（fork 2.1.x，版本闸不放宽）在 `v_ip:v_port` 拉起一台 **rocksdb 后端**的库，数据落 `workspace\.surreal\<project_code>`（换代号 = 新目录新库，旧目录留盘人工清），退出收尸只杀进程、**数据活得过重启**；署名 / 心跳 / 四格裁决 / 孤儿回收与 `spawned-mem` 同一套。「创建数据库实例」= 向导第②步选这一档填参数，不是一次性管理动作——一次性动作起的实例没有属主，机器重启后就没了。`v_ip=0.0.0.0` 允许（局域网共库），凭据默认 root/root、暴露前必须改。

_Avoid_: 内嵌库（有真端口，别的进程连得上）、外置库 / external（那档不由本进程拉起）、创建实例接口（没有这个口，它是一档配置）。

# 引导子集 (Guided Subset)

配置向导管的那部分 `DbOption.toml` 键：`project_path` / `included_projects` / `project_name` / `mdb_name` / `module` / `project_code` / `surreal_ns` / `store_mode` / `v_ip` / `v_port` / `http_api_addr` / `http_public_addr` / `gen_model` / `gen_mesh` / `watch_dbnums` / `manual_db_nums`。向导的职责是「把非改不可的引导完」，不是替代 toml——子集之外的键留给手改（`PUT /api/v1/setup/config` 也只认这个子集，`deny_unknown_fields`）。

_Avoid_: 全量配置编辑器、配置模板（那是出厂模板，另一个东西）。
