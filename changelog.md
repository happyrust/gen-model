# 变更记录

## 2026-08-20

### 修复

- 修复布尔成品已写入 `booled_id`、但 `insts_flat` 尚未回填时查看端仍加载正体原语的问题：Manifold/OCC 布尔成功路径与平表补扫现在同步优先写入单位变换的成品实例；旧库回退查询同样优先读取 `booled_id`。`=24381/36945` 不再加载带 `Z×234` 变换的正体圆柱，右视图恢复为半球。

## 2026-08-19

### 新增

- 引入 ADR-037 dabacon 快照完整性契约：`pdms-io` 以单一打开句柄生成稳定文件身份
  `SnapshotToken`，净窗口、冻结会话和模型计划终态存在性复核共享同一文件世代与
  target root；`parse_pdms_db` 增加不依赖属性字典的最小元素身份解码。

- `watch_dbnums` 限定模式增加权威 CATA 引用清单与元素级引用闭包：完整扫描继续裁决
  CATA 身份、重复文件和项目优先级，但不排全量 Catalogue 批次；模型窗口只索引被监听
  DESI 与裁决后的 CATA 文件。任务与健康接口新增依赖阶段、文件、解析/缺失计数和
  300 秒停滞截止时间。

- 暂存窗口按 sesno 拆窗（ADR-017 修订二，形态 C：预算式定窗 + 触顶收窄兜底）。
  第一层：`AIOS_STAGING_WINDOW_MAX_SESSIONS` 会话预算在**执行侧**收窄应用窗口右端，
  切点只落在真实 SAVEWORK 边界（会话号稀疏，取第 N 个真实会话而非按号算术）；预览与
  看门狗仍看整段待应用窗口。第二层：资源废弃档位触顶不再只记阻断，把该 dbnum 的会话
  预算收窄一档（减半、地板 1 个会话）再交还，预算 1 仍触顶才是真阻断；收窄记录追平
  `file_latest` 时清除。截断窗口提交成功后余量**立即重排**（不等下一轮重扫），
  `file_latest_sesno` 与并入基线沿用冻结批次那份——余量不是新观察，上界也绝不改写
  `FileObservation.file_latest_sesno`（源码钉）。相位纪元批次（`epoch_id > 0`）一律
  不参与收窄（ADR-025 phase totals 按批次记账，截断算不算相位完成未厘清）。提交侧
  一行未改：水位部分推进与 `align_end_sesno` 本来就容纳更窄的右端。原子单元变小
  （子窗口间可见真实保存点的中间态、跨子窗口的根会重复生成）待业主签字。

- 增量链增加独立阶段控制：`data_incremental`、`model_incremental`、
  `room_incremental` 分别控制数据、模型、房间消费，缺省均开启，并支持对应
  `AIOS_*_INCREMENTAL` 环境变量覆盖。关闭数据阶段仍扫描入队；关闭模型阶段时数据、
  水位与 durable 模型计划照常提交，模型积压留待恢复；下游不得越过未完成上游。
  `/api/v1/health` 同时暴露三个最终生效值，供单阶段调试直接确认运行姿态。

- 净窗口收集下沉到 pdms-io：`session_index_diff`（会话索引双根差分）与 `net_window`
  （净三态 → 操作流合成）整体从 `src/data_interface/` 迁到 `pdms_io::session_index_diff`
  / `pdms_io::net_window`，上层按新路径直接引用，`data_interface` 不做转发。理由是
  **被它替代的逐会话回放本来就在那一层**（`PdmsIO::collect_increment_eles` 一家，
  `legacy_session_replay` feature 门后），把替代品建在上一层等于同一个问题两份实现
  分居两个 crate；更要紧的是 `walk_tree` 复刻的是 pdms-io 的 `btree_search_optimized_recursive`
  路由规则（同键首见者胜、升序前缀、哨兵最左分支、`[本键,下一键)` 区间），复刻件与
  正本隔着 crate 边界时编译器帮不上忙，正本一改这边不会红、只会悄悄口径漂移（后果是
  漏报删除）。两个模块本就零 `crate::` 依赖（只用 `aios_core` + `pdms_io`），迁移是
  平移不是解耦。纯单测 24 条跟着下沉，在 pdms-io 里原样通过。
  **留在本仓**的是批次层的关切：`IncrementPipeline::collect_window`（会话页清单截取、
  `net_caliber_warning` 口径标注、`CollectedWindow` 形状、唯一入口源码断言），以及三条
  跨结构 live 对拍——它们的参照臂 `collect_changes` 是 legacy 回放外加两个 Save Work
  终稿补丁的包装、计时对象是生产入口 `collect_window`，在 pdms-io 里够不着，现迁入
  `increment_pipeline.rs` 的 `cache_tests`。性质 h/i（`db8000_session_pairs`）不动，
  改调 pdms-io 的收集器——它是这次平移的验收面：20 条全绿，其中
  `index_diff_matches_replay_folding_on_every_case_window` 与
  `net_window_collector_matches_replay_ops_on_every_case_window` 证明净口径逐案例
  仍与回放折叠一致。行为零改动，不涉及口径变更。

### 修复

- 修复 manifold 路径生成的 `.mesh` 法线数组为空，导致 plant-ui 将同一 EXTR 端盖
  按三角形渲染出随机明暗的问题：Manifold 输出现在展开为带面法线的硬边网格；回送
  CSG 前按变换后坐标焊接顶点，兼顾 E3D 外观与闭合拓扑。增加 dbnum=8000 会话 239
  V 形 EXTR 回归，验证法线、绕向、闭合性和 CSG 往返。

- 修复 D: AMS 直接图形文档中 `Limits CE` 无响应：该文档暴露的视图类型为 `G3D`，
  原命令只接受 `GM3D` 并静默返回；启动修复现在同时接受两种 3D 视图，并在本地
  Drawlist 为空时先加入、更新当前元素，再执行 limits 与刷新。修复脚本带原件备份、
  幂等标记和启动前验证，避免 E3D 补丁漂移被静默跳过。

- 修复 D: AMS 启动后 Steelwork 命令刷新反复报告 PML `(2,751)`：shadow
  `PMLCOMMANDMANAGER` 先创建 `STEELWORKGSETTINGS`，并将自动命令事件注册移到支持命令
  装载之后；Design 完全就绪后再运行带 trace 的全局初始化宏，启动失败时直接阻断而非
  把缺失变量留给每次 CE 刷新重复报告。

- 非白名单终稿解析失败、已选中索引 child 读取失败和层级未下降现在硬失败，不再构造
  可推进水位的不完整窗口；MNUM 仅按代码白名单记录结构化诊断。基线 chunk 失败会
  停止后续调度、等待 writer 收口并清空本轮全部已调度 dbnum；冻结 token 跨队列传递，
  同文件 append 仍只读冻结长度与 target，同 sesno 路径换代和空模型计划旁路均在提交前
  拒绝；初次捕获用前后 length/header 复核阻止 append 期间混合世代。CATA 扫描失败、
  空扫描、旧格式空缓存、Required 未解析根或 closure `missing` 均不再形成成功缓存，
  下一次触发会重新扫描。

- 修复复制大量节点后暂存写回把 167 条 journal/869 行合成单事务而永久停在
  `commit`：改为按条数、字节和预计行多维分块，增加块级进展、SQL 指纹和 120 秒
  单查询停滞边界，水位仍只由尾事务推进。

- 修复 dbnum=8000 净窗口删除漏判：父元素 `children_changed` 净减少现在使用
  目标会话 OWNER 成员表仲裁，区分删除与跨 OWNER 搬迁，并沿基会话展开
  不可达子树；控制台新增“成员补删”计数。新增停服维护工具
  `db_window_repair`，以独立 staging journal 纠正已提交窗口并保持水位不变。

- 增量 worker 控制台补齐可观测信息：检测保存时打印保存时间与 sesno 会话区间，
  各执行阶段打印当前窗口，完成时打印新增、修改、删除及合计数量；暂存提交超过
  10 秒时持续输出等待心跳，避免长事务期间看起来没有响应。

- 修复 `scripts/e3d/run_ams_gui.bat` 只启动裸 `des.exe`、未完成 AMS 模型树与
  DrawList 初始化的问题：启动流程现复用 repaired AMS 链，显式绑定
  `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` 的项目 evars，隔离用户/工作目录，
  跳过串入 YCYK 的通用修复宏，并以 graphics finisher 与 runtime health probe 验证
  MDB、模型树、3D 文档和编辑运行时。

- 收口 Oracle 二次审核缺陷：CATA 缓存使用实际分窗右端并按 Ref0 冲突精确阻断；
  维护纠正复用冻结快照；epoch 激活与任务冻结共用激活门；尾事务使用稳定 commit token
  幂等确认并区分 `commit_tail/commit_reconcile`；planner 对包装后 SQL、行数和条目数执行
  硬上限。manifold 失败改为双策略：暂存窗口 `Required` 阻断水位，窗口外
  `BestEffortFallback` 保留诊断与旧几何。三个依赖仓库和 manifold-csg 均固定已发布 revision。

- 新增布尔降级账本 `geom_error` 表：跳过之后 `model_update_pending` 那一行不再产生，
  控制台那句也会滚走，于是「哪些件的洞没切」事后无从查起。现在每次跳过按
  `(kind, target)` 归行落库——`kind` 为 `bool_pos` / `bool_neg`，带栽在哪块几何
  （`geom`）、累计次数、首末时刻与最近一句错误，同一元素下次布尔做成时自动销账。
  `SELECT * FROM geom_error ORDER BY last_seen_at DESC` 直接可查，`/api/v1/health`
  新增 `geom_errors` 摘要（表空是 `null`）。写的是 `SUL_DB` 而不是暂存路由：诊断账本
  要在窗口回滚后依然留着，累加计数本身也不满足 journal 的幂等要求。

- CATA 引用元素改用确定性 `CONTENT` replacement，UDA 与 owner 集合先清后写；缓存键
  同时包含源窗口右端和 CATA 文件指纹，且只在 DESI 水位提交成功后发布。依赖错误、
  `missing > 0` 与连续 300 秒无实质进展现在使暂存窗口失败并保持水位。周期 reconcile
  遇到运行任务时保留活动 epoch，新的 manifest 延后到任务终态后的重扫激活。

- 修复启动 epoch 延后模型阶段时绕过 CATA Required 依赖，以及 CATA replacement 用
  `FOR $row` 被 staging journal ReplaySafe 拒绝的问题；replacement 改为显式记录目标，
  UDA 以 500 元素为上限合并事务。8000 live 重放解析 404、缺失 0，水位 33→232，
  journal 由 1284 条降至 404 条，失败重放与成功提交均保持水位/暂存原子语义。

- 修复会话预算枚举对 `pdms_io::PdmsIO::get_nearest_large_sesno` 现行 `Option<i32>`
  返回值仍按旧 `Result` 形状解构造成的测试编译失败；缺少后继会话时现在按既有语义
  结束稀疏会话枚举，预算切点仍只落在真实 SAVEWORK 边界。

- 修正 issue #5 真实房间 live 用例的计划断言：生产计划在 BRAN `RegenRoot` 后还会为
  被移动管件排 `PostRegenAabb`，旧断言只接受单个重生成工作项，导致正确计划被误报；
  现在同时钉住整根重生成与靶件 AABB 后处理，仍拒绝管件自己的 `Transform` 路线。

- 修正 `test_cal_rooms` 把旧 AMS 切面的 124 间房/147 块面板写死为永久不变量的问题：
  常规 live 对拍只要求房间与面板集合非空，精确切面计数改由
  `AIOS_EXPECT_ROOM_COUNT` / `AIOS_EXPECT_ROOM_PANEL_COUNT` 显式钉住；核心判据仍是单构件
  增量重算与同轮全量基线逐边相等。测试的空间树前置也改走生产启动同款
  `load_project_tree_verified`，让空间状态机进入 Ready；旧 `load_aabb_tree` 只读文件却不
  完成发布门，当前状态机会正确报 `SPATIAL_TREE_NOT_READY`。

- 生产布尔的空差集不再静默吞件（specs/009 T025 的静态半、ADR-029 决策 3 对齐到
  manifold 生产路径）：`apply_insts_boolean_manifold_single` 与
  `apply_cata_neg_boolean_manifold` 在 `subtract_negatives` 之后各加一道门——差集
  网格为空（verts/idx < 3）就不写 `.mesh`、不写 `booled_id` / `inst_geo`，标记
  `bad_bool` 并出声，与 OCC 对拍路径「切洞结果为空不覆盖已有 booled_id」同一条
  不变量。顺手拆掉同函数里恒假的 `found_need_occ` 分支与恒真的 `success` 死代码。
  源码断言 `empty_difference_is_bad_bool_not_a_silent_swallow` 钉住两条路径。

- 挤出截面的 FRADIUS 倒角不再静默变直角：`tessellate_extrusion` 之前把轮廓顶点 z
  （倒角半径）直接丢掉，带倒角的 `PrimExtrusion` 全部出直角、无警告、不回退。
  现在每环先过 aios-core `wire::gen_polyline_original`（OCC 路径同一份权威实现，
  z>0 顶点换成圆弧）再按 `Extrusion::tol()` 的弦高容差 `arcs_to_approx_lines`
  折线化；首环建不出即失败，孔环建不出跳过（与 `gen_occ_wires` 同一容错口径）。
  样条轮廓（`CurveType::Spline`）没有 libgm 等价实现，回 `None` 走 OCC，不再拿
  控制点连线冒充。新增体积对拍（四角 r=20 方截面 vs 解析值 1%）、带孔轮廓不破、
  样条回退三条单测。

### 新增

- 增量监听限定域：新增 `DbOption.toml` 的 `watch_dbnums` 与 `aios-database serve
  --watch-dbnum 7998,8000`（命令行压过配置），把增量摄入的数据批次圈到指定 dbnum，
  调试时不必再让整个项目 287 个 DESI 陪跑。SYS meta（SYST/DICT/GLB/GLOB）不受限
  ——MDB 的成员名单就存在那些库里，圈掉只会得到「什么都没发现」的假现场。两者都
  没给时判定与本特性引入前逐位相同（`an_unset_watch_scope_leaves_the_scope_verdict_untouched`
  钉住）。它与 `--debug-dbnum` 各管各的（那个额外带链路追踪、刻意进不了配置文件），
  与早已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 也无关。
  **因为形状与坑出 issue #10 的手写名单一样，护栏比 `--debug-dbnum` 只多不少**：
  跳过理由、重扫聚合、回执声明三处都点名 `watch_dbnums` 并说清限定来自配置还是
  命令行（配置里的名单能躺一个月，命令行的进程一停就没了，两者的处置不同），
  与 MDB 范围判定、调试限定两种嗓音两两无交集；启动横幅同时挂在 `run_cli` 与
  Python `full_init` 上，`/health` 新增 `watch_dbnums` / `watch_dbnums_origin` 两栏。
  新增单测 12 条，含两条源码顺序断言（重扫三个桶的分桶次序、两个入口回执的声明
  位置）。

- 元素 diff 与 core.dll 的对照收口成两条显式已知边界（ADR-032 +
  `docs/evidence/2026-08-19-core-element-diff-boundary-audit.md`），行为未变。原先怀疑
  的五条分歧查证后三条不成立：OWNER 走 `elementIncluded` 我们已按 ADR-009 做了；属性
  宇宙用 schema 表还是键并集在结果上等价；core 按 `DB_Attribute::type()` 分十二类的比较
  **每一类最终都是精确比较**（`D3_Vector::operator==` 就是三个 double 逐个比，没有
  epsilon），分类只因为它拿到的是 typed 值。UDA 的 `isUdaUnset` 需要
  `hasAttributeChangedBetween` 第八参为真，而 `elementsChangedBetween` 传 0——core 自己在
  这条链上就关着。剩下两条记为边界：**A** 成员差分只有整表三态，没有 `DB_MemberCompare`
  的逐成员 kind（`kind == 3` → `elementReordered`），不实现是因为 `ChangeBucket::Reordered`
  今天既没人产也没人读，新增守卫 `the_reordered_bucket_has_no_producer_and_no_consumer`
  钉住这两头、任一被打破即红并指回 ADR（两条回退各自实测变红：`src/` 下多一处提及会被
  点名，`user_change_buckets` 里多一行产出会报「提及数 2 ≠ 1」）；**B** 没有 `DB_Uda::oldToNew` 的旧键归一化——它
  不是值的语义归一化而是键迁移重映射（值 > `0x171FAD39` 时查 `DB_Attribute::findOldKey` /
  `DB_Noun::findOldKey` 换成当前 id），门是 `ityp ∈ {51, 52}`，且同一调用挂在
  `DB_Element::getAtt` 七个重载与 `getInt` 上——**是读路径归一化而非 diff 语义**，真要对齐
  落点在 parse 层。受影响的属性是可枚举的九个（ityp 51：`GTYP`/`USYSTY`/`QUES`/`ATNA`/
  `AKEY`/`CURTYP`/`ATTSET`；ityp 52：`BASETYPE`/`DBELET`），全是 `TYPE=6`/`SIZE=1`，其中
  `GTYP` 就挂在我们解析用 schema 的 55 个 noun 上；但重映射只在**值** > `0x171FAD39`（即指向
  用户自定义的 UDA/UDET）时才动手，暴露面收窄到「用了 UDET/UDA 且定义重编号过」的项目。
  现有 db8000 语料是常规模型数据、本就不含这类事件，探针跑出 0 也只能证明「本语料未观测
  到」，故按 ADR-002「仅在发现与 core.dll 分歧时才对齐」两条都不预先实现。配套把
  `AttrInfo` 加上 `#[serde(default)] pub ityp: Option<i32>`（`None` = 尚未采集，不是取值 0）：
  `all_attr_info.json` 是 JSON 而非 bincode，旧文件不带该键照样反序列化，行为零变化；仓外
  唯一构造点 `noun_layout.rs` 填 `None`。ityp 的数据其实已经在
  `output/noun_attr_fields.json`（`NounLayoutExport.cs` 顺带产出的 57 字段字典转储，4271 个
  属性、ITYP 零缺失），不需要 live E3D 采集。同时记下一条待决项：
  `core-primary-list-e3d31.json` 的 `core_sha256` 与本机 E3D 3.1 `core.dll` 实际哈希对不上
  （字节数精确相同、文件早于采集两个月未改动），而现有断言把该字段钉成硬编码字面量，
  结构上抓不到这类溯源漂移。

- 逐会话实体回放升级为跨仓编译隔离：`old-pdms-io` 与主仓新增默认关闭的
  `legacy_session_replay` feature，生产构建不再编译回放 API；Python 调试绑定、诊断
  探针与 replay oracle 显式启用。以无 feature compile-fail 和生产 check 取代可被
  helper 绕过的 `include_str!` 字符串禁调，净窗口、HTTP DTO、水位和暂存协议不变。

### 修复

- ADR-017 phase-1 审计的四处收口。① **写回的确定性失败原来没有出口**：它无条件走
  `retry_until_recovered`，那个函数返回 `(T, u32)` 而不是 `Result`——4 次快重试之后每
  30 秒重放同一份 journal，永远不返回；而这一整段持 `STAGED_COMMIT_SERIAL`，于是一条被
  持久层确定性拒绝的语句会把 `fast_delete`、提交后空间收敛与其余 dbnum 一起停摆，外在
  表现是「增量跑了、模型没变、重启还是同一区间」，控制台每 30 秒刷同一行。现在按
  `staged_writeback_failure_is_transient` 分流：断连与写冲突照旧无限等（必然自愈），其余
  判死——记 `window_block`（reason 带原始错误）、DROP 窗口、放锁、批次转 Failed；水位没动、
  持久层零痕迹，journal 随窗口丢，恢复路径与崩溃同一条。journal 入口早有
  `ReplayUnsafeRejection` 判死确定性拒绝，这是它在写回端缺失的对偶物。② **而「确定性写回
  失败」不是假想**：暂存库刻意不装 `update_dbnum_event`、持久库装，这道有意的不对称制造了
  一类「暂存全绿、写回逐条被拒」的语句——pe 上一旦生效的是那版对数组形制 record id 用
  `array::at` 的旧实现，整窗口写不回去，而发现它的时刻是解析与生成都已白跑之后。
  `create_window` 现在进程内一次性读回事件定义验指纹（好版含 `string::split`），坏版直接
  拒绝开窗、不建实例；读不到按瞬时故障放行，只缓存成功，排毒后下一次开窗自动放行。
  ③ **资源废弃档位没有可执行的出口**：`Abandon` 只在语句入口 bail，冒泡上来与普通失败无从
  区分，而这类失败重算不会好（同一会话区间必然再次触顶，窗口只被吸收扩大、没有按 sesno
  拆窗的机制）。现在按档位单独记 `window_block`，reason 带测得的字节 / 行数、生效上限并点名
  `AIOS_STAGING_ABANDON_BYTES` / `AIOS_STAGING_ABANDON_ROWS`。④ 退役
  `sweep_orphan_staging_databases_on`：每窗口一个独立 `mem://` 实例之后跨窗口孤儿库不存在，
  生产侧无调用方，留着只会让人以为还有一层兜底回收。回归四条（三条实测过回退变红）：
  `a_deterministic_writeback_failure_returns_instead_of_holding_the_lock`、
  `only_transport_and_conflict_count_as_transient_writeback_failures`、
  `a_rejected_writeback_records_a_block_and_releases_the_window`（源码钉：记阻断 → 放窗口 →
  交还终态）、`the_writeback_schema_gate_runs_before_the_window_instance`（源码钉：兼容门
  必须排在建实例之前）。ADR-017 同步修订，并把两处「文档与实现不一致」写进正文：规则④的
  水位行只读拷入例外，以及 §6 之外那档更宽的模型让位口径（`epoch_id > 0`，重扫排出来的稳态
  DESI 也在内，这批的模型走窗口外逐根直写——决策 1 的整窗口原子在这条路径上按 ADR-025 §7
  被交换掉了）。已知仍未解决并留在台账：拆窗机制不存在；资源计量用 SQL 文本字节做摄入代理，
  `estimate_write_rows` 对带 WHERE 的集合写按 1 行计，两处都低报。

- 去除跨仓构建对宿主 `protoc` 的依赖：`dpcsync` 删除 `build.rs` 与
  `prost-build`，检入字节一致的 `prost` 生成文件；`old-pdms-io` 钉住该提交，主仓
  删除未使用的直接 `dpcsync` 依赖。PROTOC/PROTOC_INCLUDE 均未设置时三仓构建与
  回归通过，依赖树中不再出现 `prost-build`。

## 2026-08-18

### 新增

- 增量窗口收集统一到净窗口单一口径（ADR-031）。`collect_window` 不再读灰度开关、不再走逐会话回放；预览与执行共用 `collect_net_window`（与 core.dll `elementsChangedBetween` 同思想的双根 B+ 差分）。`net_window_collection` / `AIOS_NET_WINDOW` 退役，残留设置在 `run_cli` 与 `aios_db.full_init` 打显式告警——配置层没有 `deny_unknown_fields`，删字段会让 `net_window_collection = false` 被安静忽略，字面意思与实际相反。`collect_changes` 降为 legacy 诊断入口（性质 h/i、live 对拍、逐会话取证），生产链四个函数体的源码断言禁调。口径升级对外可见（原 ADR-022 §5，现无条件生效）：改了又改回不再触发 regen；加了又删不再留墓碑行；删了又建判净修改；逐会话明细退出主口径。回退手段是 `git revert` 单路径提交，不是解开配置键。T11b 改净臂单跑；执行层双臂 A/B 退役为历史证据（2026-08-13 两轮全绿仍在档）。T18 release 完整收集按协议入档（记录项，非门）：testbed 8000 高复触窗 warm median 10ms vs 53ms ≈5.3×，Add 地板窗 128ms vs 908ms ≈7.1×；SYST 250206 列为上线后现场复测。

- 两条启动检查 live 用例（`increment_manager::tests`，live 8019），钉住「已解析的库旁边新增一个库」这个现场形状。此前只有 08-17 那条单库用例，够不着的正是下面两件事：
  - `live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one`——**两条路由在同一份清单里不串味**。存量库（默认 8000，`applied=file_latest` 且 pe 有数据支撑）与从未解析的新库（默认 7998）放进同一个一次性监控目录：一轮启动重扫里存量库一行都不排（`discover_batch` 的水位早退），新库排 `apply_window` 窗口 `1..=file_latest`、worker 走 `needs_initial_load` → `initialize_dbnum_baseline`、回执含「首次按需初始化完成」，收尾断言存量库水位一格没动。存量库那侧的两个前提（追平、有支撑）写成断言而不是注释——差一个会话是普通增量、pe 零行是幽灵水位，两种都会入队，「一行都不排」也就无从断言。库号可配（`AIOS_STARTUP_APPLIED_DBNUM` / `AIOS_STARTUP_NEW_DBNUM`），默认走秒级的 7998，真靶按现场口径设 7999。
  - `live_scope_refresh_baselines_a_db_the_mdb_just_declared`——**MDB 才是增量范围的定义**。库文件全程躺在监控目录里，变的只有 MDB：把目标库从当前 MDB 的 `CURD` 里摘掉 → 重扫一行都不排，而且**连观察值都不写**（范围门排在 `record_observation` 之前，这条断言正是它的证据）；装回 CURD → `resweep_for_scope_change` 的 `scope-refresh` 重扫把它发现出来，照样走首次导入基线。复刻的是生产里「有人往 MDB 里加一个库」——那些刚进范围的设计库自己没有任何文件变更事件，不重扫就得等下次重启。夹具只动 `CURD`（`DBLS` 不碰，摘除态仍是「库存在、只是本期不在成员名单里」的合法现场），原样 CURD 存进 `queue_control:test_mdb_curd_backup` 再按原样写回；主体断言包在 `isolate_panic` 壳里，保证无论红绿都先扶正 MDB，用例开头另无条件还原一次以自愈上一轮的中断。
  夹具助手一并抽出共用（`locate_watched_db` / `isolated_watch_dir` / `queued_row` / `restore_registered_path`）：隔离目录这一手不是图省事——沙箱里躺着二十多个范围内却从未解析的 DESI 与一批 CATA，整面重扫会把它们一起排进多相位清单，而相位屏障要靠生产 worker 的重扫循环才走得完，`drain_queue_until_empty` 单独消化不了。两条各跑两个靶、**2026-08-18 四轮全通过**（默认 7998 靶 13.13s / 13.23s，现场口径 7999 靶 75.86s / 72.37s，窗口 `1..=120`，@8019），跑后实测沙箱完全还原（CURD 71 项、备份行 0、7999 水位回到 120 且 pe 有支撑、8000 停在 209 未动），台账已同步。

### 修复

- core.dll 净窗口的第二层“候选元素两端属性/成员 diff”收敛为单一实现：将
  `diff_ele_data` 提取到本地 `old-pdms-io`，legacy 会话回放与生产净窗口共同调用，
  删除 `net_window.rs` 的复刻分支并保留 re-export 兼容原符号路径。共享纯函数覆盖
  普通属性、显式属性、UDA（按 hash）和有序 children；vendor 新增 2 条纯单测，
  主仓净窗口 13 项、pipeline 48 项、会话夹具 20 项、两条真实文件对拍及 issue-019
  全链 `2 passed` 均通过。以后任一桶语义修正只改一处，不再产生净路径/回放漂移。

- core.dll `DB_Noun::primaryList` 门控从恒 `true` 改为权威快照驱动。通过已初始化
  E3D 3.1 进程直接调用 `db_get_element_info(noun_hash, 297853135)`，冻结 1931 个
  noun：1879 resolved（true 1142 / false 737）、52 unknown；resolved false 现在
  真正抑制 DB_UserChanges 成员事件，unknown 单列并保守为 true。快照钉住 core.dll
  SHA，采集脚本可复跑；B-EVT-03 同时覆盖 DAMP=true、TP=false、ROD=unknown 与
  `user_change_buckets` 实际调用。净窗口三态、children 两端持久化、模型 Regen 与
  公开 DTO 均未改变。

- 净窗口审核缺口闭环：退役键探测从 `DbOptionExtFields` 整体反序列化中拆出，独立读取原始 TOML，键值无论布尔/字符串/整数都报警；配置缺失、读取或语法错误显式报告，`AIOS_NET_WINDOW` 用 `var_os` 覆盖空值与非 Unicode 值，CLI/Python 接线断言收窄到函数体。Python 全链签名与 T11b 改用已跟踪 issue-019 baseline@24/final@26 作为固定真值，严格钉住 `changed_elements=3`、会话 `[25,26]`、水位 26、a/m/d=`0/1/2`、精确两个墓碑与活行恰减 2；移除运行时同源 oracle 和可变切点。正常档 `2 passed`，强制空跑在起点活行断言准确变红，清变量立即复跑通过，原 db8000 SHA 无损恢复。固定窗口还揭示 preview 会漏掉“冻结页存在但净操作为空”的会话，现改为从冻结会话页清单预建 `sessions[]` 并补纯单测。live 台账、ADR-031 回执、Web API 规格与证据同步。

- `rebuild_dbnum_info_from_pe` 把整个库的 pe 行拉回客户端再在 Rust 里按 Ref0 分组（`SELECT record::id(id) AS key, sesno FROM pe WHERE dbnum = N`），目录库量级直接把 ws 连接打死：2026-08-18 现场 ams7351 有 3,345,853 行，语句吊了 9 分钟后 router 任务连同 channel 一起没了，报 `read PE stats dbnum=7351 failed: Internal error: receiving from an empty and closed channel`，把前面那趟 **2.6 小时**的全量解析整个作废（数据其实已经全部落库、统计也由事件维护到位，死在的是这次纯多余的回读），批次 failed 后按相位门连坐把 catalogue 之后的 design 相位一起堵死。两处一起改：① 聚合下沉到服务端 `GROUP BY ref0`，返回行数等于 Ref0 个数（个位数），传输量与内存与库大小无关——SQL 抽成 `pe_stat_groups_sql()` 单一事实来源供测试与生产共用；② `sync_total_async_threaded` 结尾不再无条件重算——这条路径**不摘事件**（摘事件的是 `sync_pdms` / `sync_sys_only`），统计一路由 CREATE 分支维护到位，先用两条索引支撑的便宜计数问一句「对不对得上」（`classify_stats_settlement`），对得上就只补事件写不出的身份字段（`UPDATE`，无 DELETE，事件那条行原地留着），对不上才付全量重算的代价。回归测试三条：`stats_rebuild_aggregates_server_side_one_row_per_ref0`（两个 Ref0 五行 pe，聚合必须返回 2 行；退回逐行回读即报 `missing field 'ref0'`）、`stamping_identity_keeps_event_maintained_stats_in_place`（靠事件写的 `updated_at` 区分「原地补写」与「删了重建」；退回无条件重算即红）、`absent_or_mismatched_stats_still_pay_for_a_full_rebuild`（统计整体缺席时两侧和同为 0，不许被认成「对得上」）。三条都实测过回退变红。

- 初始化批次让位模型相位时，`defer_model_phase` 分支无条件打「初始化数据与水位已收口」，**失败批次照打**：2026-08-18 现场 ams7351 数据批次 failed，日志里却先宣告收口、下一行才是失败记账，排查只能绕到 `/api/v1/tasks/<id>` 回执里才拿到真错。改为按 `applied` 分叉，没推上水位的批次在同一行说清楚「未收口、模型工作不领取、原因见回执」。

- `pe` 表统计事件 `update_dbnum_event` 的 DELETE 分支在统计行缺席时把整条删除语句打死：写法是 `UPSERT type::thing('dbnum_info_table', $ref_0) MERGE { count: count - 1, ... } WHERE count > 0`，而 UPSERT 在目标行不存在时走的是**创建**路径，`WHERE` 拦不住，`NONE - 1` 当场报 `Cannot perform subtraction with 'NONE' and '1'`。统计行缺席是常态而非异常——`sync_pdms` / `sync_sys_only` 为了性能先 `REMOVE EVENT` 再写 pe，那批行天生没有统计行（2026-08-18 现场：ams5100 有 236 条 pe、零条统计行）——于是 `fast_delete` 的 Ref0 range DELETE 必炸，首次按需初始化与整库重建都过不去，批次 failed 后还按相位门连坐阻断同相位其余库（现场 dbnum=5100 卡死 meta 相位，design 相位的增量窗口排在后面永远轮不到）。改为 `UPDATE ... count?:0 - 1`：`?:0` 对齐 CREATE 分支 2026-08-06 审计定下的 NONE 免疫惯例，`UPDATE` 在 SurrealDB 2.x 只改已存在的行、缺行空转——不能让删除事件凭空造一条统计行，那条 MERGE 压根不写 `dbnum`，造出来的行连 `DELETE dbnum_info_table WHERE dbnum = N` 都清不掉；缺席的统计交给 `rebuild_dbnum_info_from_pe` 重算。事件 SQL 抽成 `dbnum_event_sql()` 单一事实来源供测试与装载共用，回归测试 `deleting_pe_rows_without_a_stats_row_neither_fails_nor_fabricates_one` 走 mem 引擎复刻现场形状（事件装载**前**写 pe，再删），已实测：回退成旧写法即报同一个 `TrySub("NONE", "1")`。

- 净窗口收集（ADR-022）在一切真实库文件上必现失败：cea58087（08-14）把三种真实文件常态升成了整窗硬错误，回退为「跳过 + 记账」并把回归钉进单测。三处分别是——① 索引树下降时**子页读不动**、② **子页层级不低于父层级**：这两种都是回收页残留的形状，生产点查 `filter_index_data` 对它们同样静默跳过，点查到不了的分支本就不参与触达集，跳过整枝不损完整性，现在计入 `unreadable_child_pages` / `level_anomalies`（索引**根页**读不动仍是硬错误，那是证明不了完整性）；③ **终稿记录解析失败**：全窗 1..=230 实测 64 条字典缺项的系统记录必现（首例 `16192_1` 报 `MNUM not exist in attr_info_map`），而回放路径对同一批记录同样以 `None` 操作落空、从未入库，硬失败等于每个含系统段的窗口整批打死，现在跳过 + `unparseable_finals` 计数 + **聚合**警告（逐条刷屏会把回执淹掉）。三处容忍都不许静默：形状进 stats、明细随回执透出。单测 `level_regressions_and_routing_anomalies_are_counted_and_flags_stay_blind`、`unreadable_child_pages_are_skipped_with_a_count_and_a_bad_root_is_fatal`、`an_unparseable_final_is_skipped_counted_and_aggregated` 各自带回归背景注释，改回硬错误即红。ADR-022 与 specs/003（Edge Cases + FR-8）同步修订并附修订说明。两条 ams8000 纯文件 live 用例复验通过、台账已同步。

- 增量更新审核（2026-08-18）五项收口。① watcher 重扫把被 `--debug-dbnum` 圈掉的库混进「不在 MDB 声明名单」聚合——对它们那句是**事实性错误**（在名单里，只是被调试限定圈掉，范围判定本轮根本没问过），正是 issue #10 的嗓音混同：D7 护栏一只钉了 `skip_reason` 的两种说法无交集，没钉 sweep 真的走它（`skip_reason` 在生产路径上无人调用）。现分成两个聚合桶各说各话（保留聚合防刷屏），调试桶点名 `--debug-dbnum` 并自证「不是 MDB 范围判定」，分桶判定复用 `debug_scope_admits` 与 `skip_reason` 同序（调试门在前），源码形状回归 `the_sweep_keeps_debug_exclusions_out_of_the_scope_bucket` 钉住。② 净窗口两种容忍形状 `unreadable_child_pages` / `level_anomalies` 只进 stats，而 stats 在 `collect_window` 拼完口径 warning 后即被丢弃——批次回执上「跳过整枝」实际静默，只有 python 探针的 `to_json` 能看见，违 spec-003 FR-8「任何一种容忍都不许静默」。现随口径标注透出 `不可读子页(t/b)` / `层级异常(t/b)`（t/b = target/base，与台账叙述口径一致），口径标注抽成 `net_caliber_warning` 纯函数，单测 `the_net_caliber_warning_carries_the_tolerated_shape_counts` 钉住，从 warning 里删掉计数即红。③ `debug_scope::trace` 的载荷改为闭包惰性构造：json! 实参是急切求值的，此前八个追踪点在未启用时也逐次构造 JSON（扫描点每轮全面重扫逐文件走到），调度器入队点还无条件取 `InitializationCoordinator` 的 allows+snapshot 两把锁——与「未启用零成本」的自述不符；闭包在 debug_scope 锁外执行以免锁序问题，惰性由 `tracing_is_inert_until_the_switch_is_set` / `tracing_ignores_dbnums_outside_the_debug_scope` 用会 panic 的载荷闭包钉住。④ `mode_notice` 说「其余 DESI 一律跳过」但豁免名单是 COLD_START_DB_TYPES（SYST/DICT/GLB/GLOB），CATA 一样被圈掉——措辞改为如实点名「DESI、CATA 等非 SYS meta」（行为未变：调试限定本就只该放行 SYS meta，CATA 是 ADR-025 正式数据阶段，要不要豁免属设计裁决，本轮不动）。⑤ `aios-database trace` 子命令写死 `curl.exe`，在 CentOS 7 部署目标上必挂——按平台 cfg 选 `curl`/`curl.exe`。

## 2026-08-17

### 新增

- live 用例 `live_startup_sweep_baselines_a_never_parsed_db`（`increment_manager::tests`，live 8019）：钉住 ADR-023 §4 生产缺省形状——范围内**从未解析**的库（无水位行、无统计行、无 pe 行，`delete_dbnum_fast` DropRow 制造，对应「新库文件第一次进入监控目录」）被启动重扫自动发现入队（上弦后 queued 不挂起、窗口 `1..=file_latest`），worker 冻结点 `needs_initial_load(0, latest)` 路由进 `initialize_dbnum_baseline`，终态 succeeded 且回执含「首次按需初始化完成」，水位推到 `file_latest`、pe 有数据支撑。与既有幽灵水位用例的分界：那条留着撒谎的登记行，这条连登记行都没有。watcher 指向只含目标库副本的一次性目录以收窄清单（全目录重扫会把沙箱里其它未解析库一并入队，多相位清单要靠生产 worker 的相位重扫循环才能走完，`drain_queue_until_empty` 单独消化不了——首轮红跑实测确认）；结尾以正本路径补一次扫描裁决，`PathMigrated` 自动迁移还原登记路径。**2026-08-17 通过**（测试体 10.0s），证据 `docs/evidence/2026-08-17-never-parsed-auto-baseline-live.md`，台账已同步。

### 修复

- db7999 设备九场景夹具首次全绿（9/9 PASS，四平面断言全过，隔离副本 `test-increment`）。此前两轮全红于 `saved session N is absent from data task merged_sesnos`，`--debug-dbnum 7999` 追踪定位为**夹具缺陷而非引擎回归**：`execute_fixture_and_wait` 在 SAVEWORK 之后轮询 `POST /update/preview` 等窗口张开，而预览唯一的写操作 `record_observation` 会把 merged_sesnos 冻结基线推到本窗口右端（红轮 trace 里全部 19 条入队记录 frozen_prev==右端），并入名单按规格恒空。就绪门改为本地读镜像文件头（`file_latest_sesno`，镜像拷贝本就同步、无需轮询），门票与预期会话号落盘 `execute-gate.json`；删除只剩这一个调用方的 `find_observed_window`。源码形状回归测试 `the_execute_gate_reads_the_file_header_not_preview` 钉住「就绪门不得咨询 preview」。红轮诊断与绿轮证据：`test-increment/runs/fixture7999-20260817-{145932-trace,154724-gatefix}/`，台账已同步。
- 初始化执行过程审核（ADR-025 链路）四项收口：
  1. F6 重扫读不出 DESI 最新会话号时此前只 warn 就跳过——清单缺着这个库照样宣告 `data_ready`、模型门照开，库持续读不动时外面毫无痕迹（DICT/CATA 头不可读却是阻断 Meta 的，同一种「观察不完整」两副面孔）。现在读失败记进对应阶段 blockers，该阶段保持可见地不就绪，共享盘瞬态靠周期对账重扫（默认 300s）恢复即解。源码钉 `sweep_skips_always_leave_a_phase_blocker`。
  2. 批次终态阻断数据阶段的判据从任务标签改为数据窗口本身（`batch_failure_blocks_data_phase`）：数据 Applied 而模型/副作用失败的 Partial 不再 `mark_failed`——那些失败在 durable pending 的重试账与死信门槛里，再关数据门只会让同阶段其余库连坐一个对账周期；数据批次 Failed 折成的 Partial（有单元成功）照旧阻断。单测钉 `only_an_unsettled_data_window_blocks_the_data_phase`。
  3. 新增数据批次连败账本（进程内，`/health` 的 `batch_failures`）：确定性失败此前会被周期对账重扫以每 300s 一次的节奏无上限重跑。同 dbnum 同右端连败到 `MAX_ATTEMPTS` 后重扫停止自动重跑（park，记阶段 blocker 可见），文件长出新会话或人工执行（POST /update/execute）清零复活，成功即清零；panic 路径记同一本账。单测钉 park/复活/重数三条出路。
  4. 启动主线等待模型阶段收敛时每 60s 播报一次仍在等什么（收敛在空闲轮里，任一环失败按 30s 退避，此前主线干等像挂死）；worker 里与 `run_cli` 重复的队列暂停恢复播报静默化（独立入口兜底保留，失败仍出声）。并入名单的基线（上一次扫描观察值）过去由 worker 执行体到冻结点再现读，而入队扫描早已把 `file_latest_sesno` 推到本窗口右端，「相对预览新增合并的会话」于是永远算不出来（2026-08-16 pipeline-f5 现场，`restore-execute-receipt.json` 里 `merged_sesnos: []`）。改为在入队时冻结基线并随队列行传递：发现方（execute 端点 / F6 sweep）在 `record_observation` 覆盖之前从裁决取 `previous_file_latest_sesno`，经 `DiscoveredBatch → DataBatch → FrozenBatch` 一路带到 `execute_one_dbnum`；同 dbnum 排队合并时基线只认最早那一次观察（取最小值）。回归测试三层钉住：批次队列纯规则（`the_baseline_freezes_at_the_earliest_observation`）、调度器冻结快照（`the_frozen_job_carries_the_earliest_enqueue_baseline`）、执行体源码断言（`the_merge_baseline_is_frozen_at_enqueue_not_reread_at_execution`，执行体再出现 `previous_file_latest_sesno` 即红）。

## 2026-08-16

### 新增

- 扫掠体自建网格器 `src/fast_model/sweep_mesh.rs`（WP-C 的 C0/C1/C2/C3 内核）：截面 → 2D 闭合环 → 三角网格，三支分派直接用 `SweepSolid::do_solid_segments()`（Core3D `DB_Gensec` 的权威判定，本模块不另立一套）。截面语义不重写——倒角与弧段复用 aios-core `wire::gen_polyline_original`（OCC 路径用的同一个函数），弧转折线用 cavalier_contours 的 `arcs_to_approx_lines`，端盖用 earcutr 做带孔多边形三角剖分（结构截面 L/C/I 是凹的，扇形三角化会填掉凹口）。摆放变换逐行对应 `gen_occ_spro_wire` / `gen_occ_sann_wire`，斜切端面沿用 `get_face_mat4`。360° SANN 按外环加内孔一次成形，两段半圆弧拼（`bulge = tan(θ/4)` 在 360° 处发散，单段圆表达不出来），另有一条测试与两个半环之和对拍以满足 FR-006。**尚未接进 `tessellate_libgm_param`**：生产接线要等直墙/斜切墙/弧墙的 RVM 门，本轮只到纯函数验证。
- `src/fast_model/mesh_assert.rs`：网格体检断言抽成 test-only 共享模块，`mesh_primitives` 与 `sweep_mesh` 共用同一套判据。
- 依赖新增 `cavalier_contours`（与 aios-core 同一 gitee fork）与 `earcutr`，两者本就在 `Cargo.lock` 里，只多了两条依赖边。
- `mesh_primitives` 单测从「非空」升级为「可用于布尔的实体」：结构体检统一走 `assert_solid_mesh`（法线齐备且是单位向量、无零面积三角、随附 AABB 与顶点一致、顶点焊接后每条有向边正反各一次即闭合可定向、散度定理算出的有向体积为正即三角朝外），再逐个原语与解析包围盒和解析体积对拍（球冠 `πh²(3R−h)/3`、半椭球 `⅔πr²h`、圆环 `2π²Rr²`、棱台 prismatoid 公式等）。7 种原语共 22 条。

### 修复

- `gen_elliptical_dish` 母线参数写反：代码用了 `x=r·cos t, z=h(1−sin t)`，与自身注释相反，生成的碟上下颠倒——底圈半径落在 z=height，z=0 处只有一个点，而底面圆盘仍按半径 r 铺，网格既破洞又带零面积三角。改回 `x=r·sin t, z=h·cos t`，随之修正法线、三角绕向（母线自下而上，绕向与球体相反）与顶点处的退化三角。
- `gen_rectangular_torus` 侧面法线全是 `Vec3::ZERO`（注释写着「后面按面片计算」但没有下文），着色会全黑。改为四个侧面各自持有顶点，法线按面给，硬边不再被平均。
- 两个环面的端面 cap 绕向与外法线反了：起始端面本该朝 −φ、末端朝 +φ，实现把两者对调，切出来的扇环端盖朝内。同时补上负角度扫掠——`CTorus::check_valid` 只要求 `angle.abs() > 0`，负角沿 −φ 扫掠会让整体内外翻转，现在按扫掠方向翻转绕向。
- `gen_pyramid` 顶面退化成一条棱（`xtop=0` 而 `ytop>0` 的楔形）时会留下零面积三角和破洞：改为四边形统一出三角、逐个丢弃退化面，点退化与线退化走同一条路径；亚微米级边长先归零，避免留下狭长三角。
- `tessellate_libgm_param` 各分支加 `covered()` 收口：`check_valid()` 放行但仍然出空网格时报错而不是返回 `Ok(Some(空))`——空网格传下去只会表现为「模型悄悄少了一件」。

- ADR-028 抽取树收尾三件：`pe_owner` 批写入补上 `INSERT RELATION IGNORE`（边 id 显式，父层补缺重放叶子已写过的边必须幂等，否则重复 id 会把整次补缺同步打成失败）；`collapse_extract_families` 输出按（项目, 库号）排序钉住跨进程扫描序（原 HashMap 迭代序随机）；F6 重扫给「抽取树父层被叶子代表」补日志、给「文件名库号与文件头不一致」单独文案（原先与多副本共用一句「多个抽取/副本」，单文件 mismatch 时误导）。
- ADR-028 父层补缺静默空转：`collect_project_db_files` 归并抽取家族时会把被叶子 shadow 的主库从解析清单里删掉，而基线的父层补缺（`included_db_files` 点名主库）恰恰要解析它——补缺同步一个文件都不解析却返回 Ok，缺口留在库里。现在被 `included_db_files` 显式点名的 shadow 主库回到清单；补缺同步返回值里若没有目标 dbnum 直接报错、不推进水位。附回归测试 `collect_project_db_files_keeps_explicitly_named_shadowed_master`。（同批核实：pe/属性批写入本就是 `INSERT IGNORE`，父层同步天然只补叶子缺号，不会用父层旧会话覆盖共享 refno。）
- 订正三处与实现相反的默认值注释：`startup_autorun` 与 `room_incremental` 的缺省均已是 `true`（分别在 2026-08-14 与 2026-08-12 翻正），但 `options.rs` 的字段注释与 `/health` 的两条字段注释仍写着「默认 false」。同批把 `startup_autorun` 关闭时的机制描述改准：它挂起的是重扫排出的队列行与空闲轮的持久积压，不是「队列消费者启动即暂停」——那是另一道跨重启保留的暂停闸门。

## 2026-08-15

### 新增

- ADR-029：设计/目录负体布尔改走本地 `manifold-csg`（path `../manifold-csg`），不再调用 aios-core 的 `ManifoldRust` / 旧 `manifold-sys`。OCC 只保留三角化（扫掠体尚无替代）；OCC 布尔不进生产。
- ADR-030 据 Core3D.dll IDA 修订：三角化权威是 libgm `gm_Create*`（挤出/旋转/ruled + 目录原语），不是 OCC BRep；OCC 只是翻译层。
- `specs/009-retire-occ/plan.md` / `tasks.md`：按 libgm 符号拆工作包与带路径任务（B 目录原语 / C 扫掠三支 / D CSG / E 离散）。
- `manifold_tessellate`：单位箱/柱与挤出轮廓走 manifold-csg（对齐 `gm_CreateBox/Cylinder/Extrusion`）；空挤出 hard fail。aios-core 的 `gen_model` 不再捆绑旧 `manifold-sys`，避免与 manifold-csg 链接冲突（需本地 vendor patch）。
- T005/T007：`gen_inst_meshes` 无 `occ`/`manifold` 时 `bail!`（禁止静默跳过）；箱/柱/挤出先 `tessellate_libgm_param`，斜端柱回退 OCC；libgm 路径 AABB/`pts` 取网格八角。

### 修复

- OCC 布尔若切出空网格，不再覆盖已有 `booled_id` 文件。`116569` 的空 60 字节结果已用 manifold 重切恢复（p95=137）。

## 2026-08-14

### 新增

- 抽取树叠加（ADR-028）：同项目主库与唯一 `_NNNN` 归并为一个逻辑库，叶子为水位权威、父层只补缺号；兄弟抽取与人手副本仍 Duplicate。
- Python 解析层暴露 `parse.collapse_extract_files` / `parse.parent_gap_refno_count`；`python/tests/test_extract_tree_offline.py` 离线钉住归并，本机 AMS 7355 实文件再钉头/会话/parent_only=0。
- 新增 ADR-024 与 `specs/005-shape-save-coalescing`，定义模型实例保存的有界合批、先计划后修改、确定性分包和成功后计入产出约束。
- 新增 ADR-026：扫掠体公开步骤按 `DB_Gensec` 蛇形命名；可复用直线无斜切时单位网格身份只键目录截面。
- Python RVM AABB 对拍：`python/scripts/rvm_aabb_compare.py` 先打 AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 WALL/STWALL（SweepSolid），口径对齐 `rvm_gate_c_or_1r345_c.ps1`（3mm / 3%）。
- Mesh 级对拍：`fast_model::shared::two_sided_surface_distance` 双向采样表面距离（mean/rms/p95/Hausdorff，parry3d，无新依赖、进 CI），三角化无关；`rvm_baseline::mesh_compare` 用 rvm-rs `Tessellate` 取 RVM 世界三角、gen 侧 `inst_geo.param` 就地 `gen_occ_shape`+`gen_occ_mesh`（param 为空的布尔/复合几何回退磁盘 `.mesh`）取世界三角。对 1112 CWALL 4 堵 WALL 与 8000 C-OR 管系（FTUB/BEND）做 mesh 对拍（live 8009+occ）：墙与 FTUB 几何忠实；BEND 逐元素多约 100mm，但 BEND+相邻 FTUB 的 union 与 E3D 只差 5.8mm——是元素边界拆分口径不同、装配无害。gen 几何经 mesh 级验证在装配层正确。
- Mesh 对拍扩到 1112 直线 STWALL（双向 ≤0.06mm）与 8000 `/C-IY-1R330-B`（ACP1000 槽盒，35 构件 union 的 gen→rvm p95=4.14 / max=24.9mm）；E3D 槽体外壳比 gen 管段大约 100mm 高，rvm→gen 大是范围差。
- Mesh 对拍 `gen_world_mesh` 与生产 `query_valid_insts` 对齐：有 `booled_id` 时加载切洞后网格，不再回退未开洞正挤出。1112 三堵大体量 GWALL 重跑 NXTR 布尔后 gen→rvm p95 从 870/786/591 降到 0.1/9.3/137。

### 修复

- 管道 FTUB 续测关闭三处增量缺口：staged 初始化尾事务保留未执行的 `RegenRoot`，模型 drain 优先最新真实保存而非历史更新时间，Add 复用旧世代 Refno 时先清理全部旧 owner/children 边；F5 同步换成当前文件可达的 FTUB 夹具并增加工程控制库与依赖 Refno 前置检查。
- 管道增量续测修复两处可重放缺陷：L3 变更宏现在识别带保存注释的 `SAVEWORK`，TTY 夹具显式传递目标 DB 与项目并按会话事实分类；水位仍为 0 的中断基线会先清除未提交 PE 再全量解析，避免 `INSERT IGNORE` 永久保留陈旧行。
- 模型实例纯数据写入的源码顺序守卫改为跟随当前 SavePlan 入口，并统一 CRLF/LF 后再切片；Windows 与 CI 现在验证同一段生产保存路径。
- 四份 e2e 探针（`staged_pane_replay_probe` / `staged_regen_e2e` / `staged_transform_e2e` / `issue7_e2e_increment`）的 `DiscoveredBatch` 补上 `phase`/`epoch_id`，`Run-LiveBatch.ps1` 的 `cargo build --lib --tests` 预编译不再被挡住。
- 模型持久工作改为每页至多 16 根、逐根执行，并在认领前及每根前后检查初始化 epoch、模型门和数据队列；数据到达时以 `model_drain/yielded` 收口，未执行行不改变状态、错误或重试次数，健康状态补充让位原因、耗时和 attempts 变化量。
- E3D 变更宏按 DONE、ALIVE、退出码及保存前后会话号分类；已保存但未确认的运行继续验证而不重放 apply，只有已知的保存前启动失败允许一次重试。L3 夹具统一采用 `preview → SAVEWORK → execute`，并验证保存会话进入 `merged_sesnos`。
- L3 的 Plant UI 运行改用隔离设置文件和仓库根工作目录；自动化按 `tree_item + refno` 定位，数据批次后等待关联 `model_drain` 与 pending 收敛，不再把数据任务的 `units` 当作模型完成证据。

- `inst_relate` 世界包围盒对 `PrimLoft` 圆弧扫掠按环扇取样（含世界轴交叉），不再把局部 AABB 当盒子做 8 角变换。
- 扫掠体斜切改为相对该段切向的垂直/平行抑制（`1e-6`），不再用世界 `±Z`；BANG/PLAX/镜像/路径方向进实例变换。目录路径组合 `get_trans` 旋转，SavePlan loft 夹具改用非 Z 切向方切。

- 合并生成器小尾批的重复元数据查询与 SQL/journal 写入；NaN 和持久化 ID 冲突在删除旧模型前响亮失败。
- 启动、回退重建及同轮稳态更新统一为 `SYS/DICT → CATA → DESI → 模型 → 房间`：
  完整清单按 epoch 安装，Watcher 事件只触发防抖全量重扫；早期阶段失败、身份阻断
  或目录不可读会关闭后续数据与模型门。队列、健康状态和手动回执新增阶段/epoch/
  blocker/shadowed 观测字段。
- 新增 `catalogue_project_priority`，跨项目 DICT/CATA 同 dbnum 由显式项目顺序选主；
  同项目重复、未知配置或无优先级候选整阶段阻断，被遮蔽文件不写 observation 与水位。
- 全量 `sync_pdms` 改为全局 Meta、Catalogue、Design 三次 await；启动提前开放 Web 与
  data-only worker，最后一个 DESI 水位完成后才执行全量模型、持久模型工作、AABB 与房间。
- 启动重扫现在会在应用水位恰好追平文件时校验数据支撑：`pe` 零行且没有匹配
  空基线凭据的库按首次导入入队，由 worker 重建基线；合法空库凭据与水位同事务
  收口。生产缺省 `startup_autorun` 同步翻为 `true`，未解析库和异常水位库无需再等
  下一次文件保存触发。
- 增量正确性阻断：数据队列新增 `apply_window` / `reinitialize` 显式意图，回退到
  零会话也会以 `0..=0` 到达 worker；排队重建意图占优、运行中保留后继，冻结点
  复核仍判 Rollback 才清库。空文件清库后直接 Applied 且水位保持 0。
- `fast_delete` 的统计/持久队列、spatial epoch 与水位清零纳入同一显式事务；
  关系和 Ref0 区间删除继续独立幂等，水位更新保持事务末句。
- 幽灵水位清库会从 `dbnum_info_table` 的 record id 恢复真实 Ref0，并与 PE 前缀
  取并集后删除对应区间；不再把 dbnum 数值冒充 Ref0，PE 零行时也能清理派生行。
- 净窗口把不可读子页、层级不下降、终稿解析失败和 last-touch 缺失升级为整窗
  错误，杜绝残缺触达集落库推进水位；Modified 基版本失败仍保守降级为 Add。
- `ref_rev_maintain` 补偿载荷改为非空、全量严格解析，任一非法 refno 均进入失败
  记账并保留队列行，不再以空修复调用静默销账。
- 收集接口统一为 `CollectedWindow`，把冻结窗口的实际会话页清单与操作流、口径和
  warnings 一起贯穿预收集、崩溃重放及成功回执；空保存、自抵消和稀疏会话现在
  正确进入 `merged_sesnos` 及平行保存时刻。Replay 清单与操作共用一次文件打开，
  两种模式首条 warning 固定自报口径，且后续计划失败也会保留该 warning。
- 基线入口开始硬消费共享 `ScanGate`：身份阻断和回退重建均在计数/解析/水位前退出；
  范围外 CATA 改为跨 scope 收齐全部候选后复用 watcher 的同项目 dbnum 判重，重复组
  零 observation、撤销旧 locator 路径，并在预览/入队回执列出全部路径。

## 2026-08-13

### 新增

- **会话索引差分：db 文件 sesno 窗口净增删改秒级判定**
  （`data_interface/session_index_diff` + `aios_db.parse.net_changes` +
  `python/testbed/net_changes_probe.py`）。每个会话页都带当时的索引根
  （copy-on-write B-tree），取窗口两端的根做双根差分：目标树只下降「页号 >
  base 会话末页」的新页、共享子树整枝剪掉，base 树按共享根集合剪——IO 正比于
  变更量，与窗口内会话数解耦。判定**纯文件**：不查库、不逐会话解析记录，窗口
  由调用方显式给定（源码断言钉死零 `SUL_DB`）。存在性口径与生产 B+ 点查逐字
  对齐，三条规则均由真实 ams8000 实测逼出并钉成回归单测：同键子指针首见者胜
  （Save Work 重写子树留下的陈旧指针，跟进会捞出 1.9 万条已被发布抛弃的临时
  记录）、路由不看 flag（flag=0 的首见指针才是发布后的子树）、键范围路由
  （回收页残留条目键在本叶范围之外，点查不可达）。验收：模块纯单测 11 条 +
  `db8000_session_pairs` 性质 h（差分 ≡ 回放折叠，台账腿由性质 e 闭环）+
  Python 离线档 3 条（issue-019 夹具）+ live 对拍 4 窗口差分 ≡ 生产点查零分歧
  （全窗口 695ms vs 回放 10.8s，debug 15–34×）；探针 `--verify` 全量窗口审计
  154 条差异全部点查仲裁归因为回放旧口径盲区（漏报存在 67 / 孤儿腿误报 86 /
  误判 1）。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`，
  live 台账 D 组两条已登记。同日补测 amssys（SYST 8191，169 会话）：10.6×，
  回放折叠净集 **43%（818 条）与生产点查仲裁的两端状态不符**（孤儿 Deleted 腿
  653 为主）；点查是**同源判定基准**（列出/归类分歧，非独立证明全是旧口径盲区），
  删除判据的独立性另由 core.dll 键集差佐证。
- **ADR-022 + specs/003：增量窗口收集改用会话索引差分（净窗口；已接受，P0 已落地、默认 off 灰度中，核心机制层已由 live IDA 闭合，翻默认余下受验收 5 结果层门阻断）**。
  工具层对拍收口后的引擎采纳决策：执行体与预览的收集阶段由
  `collect_net_changes` 接管（逐会话回放退为诊断工具），输出形状兼容——每
  refno 恰一条 `EleOperationData`，净修改由 base/终稿两端版本**一次 diff**
  合成（属性差量 + children 两端 + old/new owner，diff 实现与回放同源单一
  权威）；下游模型计划 / ref_rev / MySQL / 渲染零改动。灰度开关
  `net_window_collection`（默认 off）+ `AIOS_NET_WINDOW`，预览与执行同谓词。
  四条明示行为变化：改了又改回不再 regen、加了又删不留墓碑行、删了又建判净
  修改、逐会话明细退出预览主口径。窗口起点仍由水位给出（ADR-001），回退/
  幽灵水位仍走 ADR-021 整库重建，跨库级联仍走 ref_rev（ADR-003）。
  - 实现落地（同日晚，P0 引擎接线）：`net_window::collect_net_window`（净三态 →
    同形状操作流合成；`diff_ele_data` 忠实复刻 vendor 内联 diff，九桶 + children
    两端 + noun）+ `IncrementPipeline::collect_window` 唯一派发点（预览、执行体、
    崩溃恢复重收集、worker 尾段重收集四处接入，源码断言禁直调回放）；
    `NetEntry::base_loc` 让净修改直读两端版本不付点查；净口径回执首条警告自报
    口径与计数。真实文件逼出第三条口径对齐：字典缺项系统记录（ams8000 全窗口
    64 条）终稿解析失败按回放同口径跳过 + 计数 + 聚合警告，不整批硬失败。
    验收：`db8000_session_pairs` 性质 i（净收集 Modified 负载与回放**逐桶相等**，
    全部案例窗口全绿·样本为各窗口实际 Modified 条目非 test binary 计数）+ live
    负载对拍（6,499 条 Add 渲染逐字符相等，全窗口净收集 1.24s vs
    回放 10.9s）+ lib 710 passed + 离线档 65 passed。已知偏差记 evidence
    「引擎接线」节（`merged_sesnos` 会话页清单口径留待翻默认值前落地）。
  - **live A/B 全链路执行（同日深夜，切默认值前的最后一道证据，已收口）**：
    `python/tests/test_net_window_ab.py`（房间增量档，opt-in
    `$env:AIOS_NET_AB='1'; .venv\Scripts\python.exe -m pytest
    tests/test_net_window_ab.py -q -s`，@8071 一次性内存库）。testbed 8000
    （基线 6,542 行）同一起点、同一窗口 105..=209（净三态 +6/-51/~16，其中原样
    重写 7），off/on 各走一遍完整执行（暂存窗口 + 窗口内生成 + 提交 + 水位收口）：
    **终态逐维等价**——水位 / 共同活行 6,543（逐字段）/ noun 属性表 / pe_owner
    6,542 边 / pending / dbnum_info 记账恒等式全部相等；仅有的偏差全部归因：
    净臂多持 2 个文件真值元素（回放连同旧基线的最终索引漏报，点查仲裁站净一边）、
    13 条 ref_rev 边为回放对 7 个原样重写元素的顺手重建（§5.1 家族，重置后空
    ref_rev 店放大）。窗口全链路耗时回放 35.0s vs 净 11.0s（3.2×），收集阶段
    差分自报 154ms。连续两轮全绿（各 3 分 16 秒）；全量绑定档 83 passed +
    1 skipped（36.4s）。证据同文件「live A/B 全链路执行」节。
  - **M1 正确性闭环（同日，T20 / T11b / T19 / T18a 落地；T13 阻塞未闭）**：
    - **T20 合成器纯单测**：`collect_net_window` 抽出纯合成内层
      `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄成
      `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误文案留在合成器），**七条
      纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc`
      缺失硬失败 + 原样重写计数（原样重写**不是降级**，是正常判定的正常结果）。
      纯提取不伪称先红：安全网是性质 i + 既有 live 对拍，新测试有效性由**逐分支变异
      抽检**证明（5 处准确红，变异代码不入库）。`net_window` lib 13 passed / 0 failed /
      1 ignored（ignored 是需真实 ams8000 的 live，**本轮未跑**）+ `db8000_session_pairs`
      集成目标 20 passed（含性质 i，是用例数不是覆盖窗口数）+ Python 离线
      66 passed / 20 deselected。ADR-022 验收 1 就此满足。
    - **T11b 存量库删除等价直证**：补上「起点早于删除会话、库内确有活行」的形态——
      原 A/B 删除腿是空跑（被删元素在基线本就无行）。切点 K=24、窗口 25..=209，
      文件层净删除 oracle 4 条，起点确为活行且净口径**真立碑 2 条**
      （`24384_24778`/`24384_24779`，⊆ oracle），共同活行 6,536 逐字段一致、
      **0 未归因**，live 118s 全绿；`AIOS_T11B_FORCE_EMPTYRUN=1` 强制空跑变异准确变红。
      存量基线由 `python/tests/_session_snapshot.py`（`session_cut.rs` 的 Python 镜像，
      与 Rust `db_session_fixture inspect` 双向对拍）切 @K 得到；文件换入换出走**同卷
      临时文件 + fsync + `os.replace` 原子替换** + `pristine` 备份 + `finally` SHA 校验，
      收尾源文件 16,504,832 字节无损恢复。**删除判据是纯文件**（core.dll
      `elementsDeletedBetween` 键集差的复刻）；**DB 查询只验证窗口前活行与窗口后墓碑
      两个状态，不作删除判据**，也不用 `search_latest_refno` 点查自证。
    - **T19 qualifier 恢复对拍（非阻断，CLOSED）**：断言落 `db8000_session_pairs.rs`
      性质 i 的 Modified 分支，两臂 `qualified_changes` 逐项相等，集成绿，**未扩公开
      DTO**。强度如实标：当前 issue-019 夹具两案例都是删除、数组属性零变化，这条现在
      是 **empty == empty**，**不是 qualifier 语义已覆盖的证据**，价值只在防回归。
    - **T18a release 方向性单点测量（n=1，非性能门）**：高复触窗 104..=209（106 会话，
      a/d/m = 6/51/16，回放 `ops_total` 215，复触率 2.95）完整净收集 3ms vs 回放 53ms
      ≈ **17.7×**，该窗 raw 两臂发散 72 条全部归因回放旧口径盲区、点查零分歧；对照
      Add 地板窗 1..=209（复触率 1.05）126ms vs 792ms ≈ 6.3×（形态决定，不作判定）。
      **结论仅限**「在动机形状上 ADR-022 决策 4 不需修订」；T18 正式统计
      （1 warmup + ≥5 次 / median·min·p95 / warm 判定 cold 另报）与 **250206 SYST
      现场硬门仍未完成**。另：A/B probe 的 4.4× 已明确降级为「净差分 vs 回放完整收集
      的混层下界参考，非门证据」。
    - **T13 Added 夹具 BLOCKED（不得标完成）**：仓内**不存在**同时满足「Added > 0」
      且「raw 净集 == 回放折叠集」的真实窗口——带 Added 的窗口都伴随回放旧口径盲区，
      raw 两集不等，性质 h/i 指过去必红。须用受控 E3D 录 `scratch-create` 案例
      （新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）；**不得**为点亮
      它放宽 h/i 断言。**M1 Exit gate 因此仍未通过，M2（T17/T12/T18/T15）不得启动。**
  - **决策澄清（同日，评审后最小补写，不改决策主体）**：ADR-022 新增「算法来源
    与正确性边界」——会话索引差分**不是** core.dll `DB_DB::elementsChangedBetween`
    的复刻，而是 gen-model 吃 dabacon 追加式 CoW B+ 树形状推出来的加速。证据边界
    同时写死：core31-retrace 证据只显示其**外层语义**是元素 /（属性, qualifier）
    级的三阶段六桶差分、外层未见索引根双根页差分，但
    `attributesChangedBetween` / `elementsDeletedBetween` /
    `elementsInsertedBetween` 的页级实现**未逆向**，故**不断言**内核内部绝不触及
    索引根；core.dll 继续是属性/桶语义的唯一权威，本路径的索引差分不援引它作为
    算法来源。正确性契约写明：端点存在性以生产 B+ 点查可达性为 oracle、净三态
    由两端 leaf `(pgno, offset)` 集合差定义、净修改仍用两版本 `diff_ele_data` 对齐
    core.dll 语义，正确性靠三重对拍而非「复刻内核」。同时记两条机制层未闭合风险
    （叶 `flag` 的取值语义与取值全集未逆向、当前口径本就不依赖 flag；删除是移除
    leaf 还是墓碑 flag；`is_start_page` 只是索引条目起始哨兵行为、底层位定义未知
    ——三者均无 live IDA 证据，现有零分歧只证结果层，且**差分≡生产点查是同源**
    （二者都不看 flag），不能当 flag 机制的独立证明），并把翻
    `net_window_collection` 默认值的门写成验收 5：要么 (a) 在可达 core.dll/idb 上
    闭合机制；要么 (b) **显式接受机制层未闭合的残余风险**并补一份**结果层**样本——
    其独立 oracle 必须是**生产可见终态**（E3D/权威库侧对同一元素在删除/重建后的
    在场与否），而非同源的点查仲裁或带旧口径盲区的回放，样本覆盖已观察 flag 取值/
    删除重建/Added-Deleted-Modified 三态，走 (b) 机制层仍标未闭合。默认 off 的
    诊断与灰度不受此门阻断。
    **⚠ 本条的「机制层未闭合 / 无 live IDA 证据 / 只证结果层」口径已于同日晚被
    live IDA 逆向推翻，以下一条为准。**
  - **机制层闭合（同日晚，live IDA 逆向，推翻上条保守口径）**：
    `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`
    （ida-bridge / idalib，core.dll 3.1，SHA `3c1f…417d`，符号系二进制自带 MSVC
    修饰名、非猜名）证实 core.dll 会话变更枚举（`DB_DB::elementsChanged /
    Deleted / InsertedBetween` → `DB_IndexTableCompare`，dabacon 比较引擎 opcode
    266/270，主索引表 `13387743`）**本就是双根 B+ 索引归并差分**——与 gen-model
    `session_index_diff` **同思想**（gen-model 是纯文件重实现 + 共享子树剪枝，
    非逐指令复刻内核代码）。三处旧「机制未闭合」悬案就此闭合：① 删除 = 键在旧根
    不在新根的**集差、非墓碑 flag**（kind=3）；② 变更检测**全链路**（页取 + begin +
    双根归并）**不读 / 不按 flag 过滤**（flag 在链路外是否另有可见性门未闭合，不写
    功能性否定，report §4.5/C3/C4）；③ `0x80000001` 是**页内键哨兵**（核内以
    `-2147483647` 作键边界识别）。**残留（不阻断翻 on，仅登记）**：`flag` 自身位
    编码（存在 / 偏移 / 位宽 / 枚举）与 **flag 在变更检测链路之外是否另有可见性 /
    过滤门**均未逆向（report C3/C4，有意收口）——可断言的只是「权威变更检测链路
    不以 flag 作门」，不写「flag 全无功能」。据此把 ADR-022 / spec / plan / tasks
    的翻默认门从「(a) 闭合机制 / (b) 接受残余风险」改写为**结果层门**（存量库删除
    A/B、Added 独立夹具、批次冻结快照、会话页清单、SYST 性能——性能门当前**未达**，
    debug 完整收集仅 8.8× / probe 4.4×）。qualifier 维：core.dll 变更粒度含
    `(attr, qualifier)`，gen-model `ModifiedElement` 按属性名聚合会丢 qualifier；
    这是回放与净路径**共享的既有形状限制**、切臂不新增，翻 on 不阻断但**非无条件
    安全**，待评估（tasks T19）。

- **ADR-021 + specs/002：水位必须有数据支撑，回退默认整库重建（去档位）**。
  ADR-001 的「失败不推进水位」管的是写的一侧；读的一侧有两个洞。其一，
  `needs_initial_load` 只问水位不问数据，「水位非零、`pe` 零行」被判成正常
  增量，从 `applied+1` 起重放，`1..applied` 静默缺失（看得见数据的
  `baseline_needs_full_parse` 在 `initialize_dbnum_baseline` 内部，够不着
  路由）。其二，文件回退默认只阻断等人，而阻断会静默消失——`file_latest`
  一旦涨回 `applied` 之上，被替换的那段差异永久丢失。
  - 现场实证：8009 上 dbnum 7350 / 7353 / 7741 的 `applied_sesno` 为
    208 / 101 / 94 而 `pe` 零行；同日 8 个库因文件在 08-12 19:04 被整批换成
    更旧副本而回退阻断。证据 `.scratch/realign-20260813-114321`。
  - 决策要点（评审决议 2026-08-13）：回退**默认整库重建**——扫描只分类入队
    重建批次，worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`（整库清空 +
    水位行清值不删行 + 统计与队列残留清空 + spatial epoch 递增），随后按首次
    导入全量解析；`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN`、
    `POST /dbnums/{dbnum}/realign` 端点与 `aios_db.sync.realign` 绑定全部
    移除。幽灵水位（`applied>0` 零数据）由 `needs_initial_load` 的数据支撑
    维度路由到基线（判据落在路由、不落入队门——空库会无限重解析）。
    `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 照旧阻断。
  - 实现落地（同日）：`needs_initial_load` 增加数据支撑维度 +
    `dbnum_has_any_pe_row` 存在性探针（只在有增量窗口要跑时付一次，读失败上浮为
    批次 Failed）；`scan_and_check_file` 返回三态 `ScanGate`（放行/阻断/重建），
    sweep 与 watch 对回退构造 `reinit_batch`（applied=0 形状）入队；
    `fast_delete::wipe_dbnum_for_reinit`（与快删同源的三阶段删除，元数据阶段改为
    统计清空 + epoch 递增 + 水位清值不删行且置尾作提交点）；执行体
    `execute_one_dbnum` 冻结点复核仍判回退才清库，清库失败计 Failed 幂等重放；
    预览 `blocked`/`initialization_required` 与执行体同谓词。
    `FileAnomaly::auto_realignable` 更名 `requires_reinit`。拆除面：
    `WatermarkRealign` 档位与环境变量、`realign_rolled_back_dbnum` /
    `realign_dbnum_checked`、HTTP `POST /dbnums/{dbnum}/realign`、
    `aios_db.sync.realign` 绑定、`AiosClient.realign_dbnum`、
    `python/tests/test_watermark_realign.py`（由 `test_rollback_reinit.py` 接棒，
    见下）。
  - Python 闭环用例 `python/tests/test_rollback_reinit.py`（房间增量档，@8071
    一次性内存库）：走与服务同一台机器（`incr.execute_manual` 子集），模块级
    引导一次（SYS meta 解析撑起 MDB 范围 + 7998 首次基线），三条用例分别钉
    回退整库重建（幸存位/幽灵位标记行都必须物理消失）、幽灵水位路由到基线
    （行数回到完整基线，增量重放做不到）、类型变更照旧阻断（水位与数据纹丝
    不动）。conftest 导入期补 `RUST_MIN_STACK=16777216`（执行链在默认线程栈
    上溢出，与 testbed 脚本同一惯例）。全套 `pytest -q` 80 绿（36.5s）。
  - live 首跑抓出并修复一个真缺陷：增量形状（start_sesno>1）的批次先开 ADR-017
    暂存窗口、执行体改道基线后窗口缺 finalize plan 而 failed——`batch_worker`
    开窗前新增冻结点预判 `batch_reroutes_to_initial_load`（applied=0 / 回退 /
    幽灵水位一律不开窗），与执行体共用同一个数据支撑探针。
  - 验证：CI 口径受影响模块单测 155 绿；live
    `live_rollback_wipe_clears_the_dbnum_for_reinit`（4.7s @8019）与
    `live_rollback_and_ghost_watermark_reinit_end_to_end`（22.3s @8019，两幕）
    通过，台账与 `docs/evidence/2026-08-13-adr021-rollback-reinit-live.md` 留痕；
    Python 离线档 62 绿。
  - 「在水位行上记录来源（基线收口 / 增量收口 / info 表播种）」与
    「`applied_sesno_time` 交叉核验（停机窗口内回退又长回去）」记为后续项。

- **增量模型生成单元测试总纲**：重写
  `docs/2026-08-06_model-increment-unit-test-plan.md`，把 S0–S13、U1–U13、
  暂存窗口 I1–I9、房间 RI-1–RI-15、离线夹具与 live / E3D 边界收进同一入口；
  明确 P0/P1/P2 待补项、具体文件落点、“回退即红”条件、Constitution Check
  和分波次门禁。当前源码枚举快照为 765 项（82 ignored），`http_api` 为 776 项
  （82 ignored）；长期状态仍以源码枚举和 live 台账为准，不再复制漂移总数。

### 修复

- **`inst_geo` 几何参数双变体深合并毒化共享单位行（live A/B 抓出的真缺陷）**：
  `render_inst_geo_merge`（2026-08-13 `276aa5f6` 用 `UPSERT … MERGE` 替换
  `INSERT IGNORE`）忽略了「不同 `PdmsGeoParam` 变体可以合法共享同一记录 id」——
  普通 LCylinder 与非切角 SCylinder 的单位网格同为单位圆柱，
  `hash_unit_mesh_params` 按设计同返 `CYLINDER_GEO_HASH`，两个变体先后 MERGE 把
  `param` 深合并成 `{PrimLCylinder, PrimSCylinder}` 双键对象，enum 反序列化永久
  失败，**所有**引用该共享行的根从此生成不出来（A/B run4 实测：2,229 根批量重
  生成全灭 + 逐根重试全灭，`decode mesh parameters failed`）。改为
  `render_inst_geo_upsert`：`param` 整值 `SET` 覆盖——行缺失补齐、半成品修复、
  meshed/aabb/pts 派生字段保留，且对已被旧写法打坏的双键行**自愈**（下次参数
  刷新整值盖掉即恢复可解，2026-08-13 后跑过生成的持久店无需手工修）。回归：
  `a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale`（回退 MERGE
  写法当场红）+ 半成品修复用例改跟新入口 + 源码钉
  `production_inst_geo_writes_replace_param_wholesale`（禁 MERGE 回潮）。受影响
  面：lib 定向 12 条全绿、`db8000_session_pairs` 20/20 全绿、全量绑定档 83
  passed + 1 skipped。
- **`room_model` 无 project 特性构建编译修复（响亮拒绝）**：`configured_match_room_fn` /
  `load_room_panel_map` / `load_room_panel_map_from_pe` / `build_room_panels_relate` /
  `build_room_panels_relate_common` / `load_room_panel_groups` 此前只有 `project_hd` /
  `project_hh` 两条 cfg 分支，两者皆未启用（CI 单测组合 `ws,gen_model,manifold`）时
  `configured_match_room_fn` 无返回值（E0308）、`let sql` 门控外的取用点找不到 `sql`
  （E0425×2），整个 lib 编译不过。按宪法「禁止填近似值」改为**响亮拒绝**：无 project
  特性时各入口 `anyhow::bail!` 明示「需要 project_hd 或 project_hh」，原实现体整体入
  `cfg(any(...))`；`configured_match_room_fn` 同样门控（无 project 时其调用方已全部
  bail，不再被引用）。附回归单测
  `room_subsystem_loaders_loudly_refuse_without_a_project_feature`（仅在无 project 组合
  编译运行，断言两个 loader 返 Err 且提示特性名）。
- **增量流程文档一致性修复（2026-08-13 流程审计定案，纯文档面）**：
  - 宪法 v1.0.0 → **v1.1.0**（`.specify/memory/constitution.md`）：I 条回退语义按 ADR-021
    改写（回退默认整库重建、仅 `TypeChanged`/`Duplicate`/`Missing`/`ForeignProject` 身份
    歧义阻断、补「承诺必须有数据支撑」读侧对偶），附加约束「并发模型」按 ADR-011
    2026-08-09 修订改写（一个派发器 + 至多 8 个在飞批次）；Governance 增修订记录
    （动机 / 受影响 ADR / 迁移路径），Last Amended 2026-08-13。
  - AGENTS.md 水位段与配置段对齐（消除「回退阻断」与「回退默认整库重建」同文矛盾，
    补数据支撑一条），队列门控段的「同一个 worker」补派发器限定。
  - ADR-021 状态「提议中」→「已接受（2026-08-13 评审决议）」。
  - ADR-011 2026-08-09 修订下游同步：`docs/specs/web-service-api.md` §2/§4.3/§6、
    `specs/002-watermark-data-backing/spec.md` Assumptions、ADR-021 引言——「单 worker /
    一个消费者」措辞补「一个派发器、默认 1、可配至 8 在飞」限定，行为描述不变。
  - `specs/002-watermark-data-backing/` 补 `plan.md`（含 Constitution Check：I 条冲突
    处置 = 本次修宪）与 `tasks.md`（按已落地事实事后补记留痕，每条带文件路径）。
  - live 台账（`docs/2026-08-12_live-test-ledger.md`）：合计修正 86→**92**——A 27→28
    （08-13 新增的端到端用例漏计）；新增 E 组补录 tests/ 目录 5 条集成 `#[ignore]`
    待验行（staged_regen / staged_transform / staged_pane_replay / room_rebuild_repair /
    gen_one_root；`db8000_session_pairs` 的命中经复核是文档注释、无真实 ignore 用例），
    口径行同步扩展到 `tests/**`；C 组 issue7_e2e 行加注「旧语义现场，ADR-021 后需按
    新语义重估重跑」。
  - 房间增量测试计划（`docs/plans/2026-08-12-room-incremental-live-test-plan.md`）§7
    增 08-13 重估行：db1112 的 F6 阻断判词被 ADR-021 取代——部署新二进制后首轮重扫
    将排整库重建批次，Phase C 前置需纳入重建时序、代价与重新定标。
  - CONTEXT.md 增词条：数据支撑 / 幽灵水位 / 重建批次（各带 _Avoid_ 清单）。

- **fix(incr)：副作用补偿队列补齐死信可观测 / 人工复活（`/update/side-effects/retry`）/
  done 行清扫三出路，并将 `room_panel_relate` 纳入整库重建的 Ref0 区间清库（补
  `room_relate` 漏删的姐妹边）**。逆向核实确认：`room_panel_relate` 的 id 形态
  `{room_refno}_{panel}` 可按 Ref0 区间寻址，而房间重算只对现存实体先清后写、从不
  整表清空，整库重建后的孤儿边无人回收（ADR-010 D4 幽灵同类）；修复为
  `fast_delete.rs` `RANGE_TABLES` 增表 + 回归测试
  `the_wipe_deletes_room_panel_relate_alongside_room_relate`，ADR-021 §4 口径同步补记。

- **fix(incr)：重扫路径读不出文件最新会话号时不再吞成 0（消除假回退告警与失实的整库
  重建播报）；CATA 定位器读登记表失败上浮、缺 `db_type` 的库计入 missing 并告警；
  MySQL 镜像 NAME 改参数绑定、DBNO 缺失告警；MQTT 同步去重查询插值统一过
  `escape_surql_str`；各附回归测试**。

## 2026-08-12

### 新增

- **db8000 会话对回归进 CI：夹具格式的七类性质断言，数据驱动**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` 阶段三 + 四）：
  - 新增 `tests/db8000_session_pairs.rs`（18 passed）：对 `aios-session-fixture-v1`
    夹具里的**每个案例**跑七类断言——档案完整性、窗口切片、时点一致性、并集律、
    净变化折叠、快照差分对账、历史还原。不硬编码任何 sesno 或 refno。
  - **不等真实录制**：夹具来源两条，`AIOS_SESSION_FIXTURE` 指现成目录，缺省则
    从 issue-019 的 final 现场 `pack` 一份。后者是真实 db8000 会话链上的真数据
    （阶段一自检已证明现切台账与当年独立录制逐字节相等），只是案例集小。真实
    录制到货后改指环境变量即可，断言不动。
  - a) 与 g) 直接复用 `pipeline::verify_fixture`——它本就在做那两件事，重写会
    产生两套必须永远一致的口径。
  - f) 快照差分是新增的通用 oracle：`read_raw_records` + `parse_raw_ele_data`
    在 before/after 两份现切快照间逐元素比对（存在性 + noun/name/owner +
    **children** + 属性表），与净结果互证。**children 必须进比对**：实测
    issue-019 的删除序列里，父件与祖父的属性表一个字节都没动，Modified 的信号
    全在 children 列表上；只比属性会把它们误判成「增量说变了但文件没变」。
    噪声属性白名单目前为空——方案预判的 CACHID 类漂移在这条链上没有出现，
    常量留着是机制不是占位。
  - CI：`db8000_session_pairs` 接进同一 job；失败时 upload-artifact 传完整断言
    输出与夹具台账（新增 `AIOS_SESSION_FIXTURE_KEEP` 让合成夹具落在工作区，
    否则临时目录跑完就没了、远程红了无从对账）。
  - job 更名 `db8000-model-increment` → `offline-increment-regression`：它现在
    跑五步离线回归，早已不止 db8000 的模型增量。**配了分支保护必需检查项的话
    需要同步旧名字。**
  - 远程首跑（run 31572427572，2026-08-12 15:03 dispatch）：
    `offline-increment-regression` **首跑即绿，28m58s**——五步（issue-019 夹具、
    通用切割自检、session-pairs 回归、记录边界解析、删除清理 lib 用例）在
    GitHub Actions 上全过。同 run 的 `python-bindings` 在 wheel 冒烟一步红
    （runner 侧 DLL 解析，与离线回归无关），修复走 `4ddf32b9` 的 DLL 探针
    传递闭包，验证 run 另行跟进。

- **db8000 录制清单补齐到 6 类变更形态，并加了一道离线闸**（方案阶段二的离线前置；
  录制本身仍等生产空窗）：
  - 新增 8 个宏、5 个案例：`scratch-create`（added）→ `data-rename` /
    `transform-move` / `geometry-resize`（各 modified，apply+restore）→
    `delete-box`（deleted）。`-CheckOnly` 静态审查 12 条腿 / 7 案例全过。
  - **不动生产元素，改为自建 scratch 元素**：restore 腿必须把值放回原状，而
    生产元素当前的 POS / XLEN 离线不可知，照方案原文写就得先占一次空窗做探针。
    自建元素让每个 before/after 值都自决，整套宏离线可写可评审；末位 delete
    收尾后库在逻辑上回到原样。副产品是 `added` 净形态——原案例表里一个都没有。
  - 新增 `recording_manifest_survives_the_sesno_assignment_it_will_get`：按录制
    脚本的 sesno 分配规则预演清单，`plan_cases` 必须接受，`expected_net.element`
    必须已声明、宏文件必须在场。这类错原本要等占完空窗、录完一整轮才在打包时
    炸，现在 `cargo test` 就拦（防伪已验）。

- **live 用例台账 + 首批点亮**（7-27 测试计划 Gate 3 的首次执行）：
  `docs/2026-08-12_live-test-ledger.md` 给全仓 82 个 `#[ignore]` 用例建档
  （四类目标要求 + 最近通过记录），`scripts/Run-LiveBatch.ps1` 按清单逐项
  独立进程定靶批跑（`DB_OPTION_FILE` 与 `AIOS_LIVE_*` 两套寻址同源派生、
  恰一命中、逐项超时、JSON 报告）。批次 1（自建夹具类 26 项）全部得出结论：
  **23 项首次取得可复现通过**（12 @ testbed 8019；11 项 room_fixture @
  一次性空库 8071 专用清单——房间覆盖率闸门在带真实基线的库上对「只灌夹具」
  的树必拒，空库上夹具行自然对得上分母、闸门语义原样保留），3 项阻塞定性在案
  （积压前置 / 缺陷面板数据依赖 / 断言写死生产 MDB 语义）。顺带修三处测试
  腐化：白名单落地前的夹具命名（first/second 进不了判重）、状态机落地前的
  两个崩溃恢复用例缺测试装载模式声明。另补 IU-S8-05/S12-02 顺序钉（部分失败
  后缓存仍失效、水位不推进）与 IU-S0-05 的 warning 半边——7-27 矩阵点名的
  L0 缺口至此全部关闭或有台账去处。

- **房间增量默认打开（`room_incremental` 缺省值 `false` → `true`）**：`options.rs`
  的 `effective_room_incremental` 缺省翻真，`DbOption.toml` 同步写成 `true`，
  单测由 `room_incremental_is_off_unless_someone_asks_for_it` 改成
  `..._is_on_unless_someone_turns_it_off`，并补一条「认不出的环境变量值退回新
  默认」。
  - 2026-08-10 取假是为了压住现场那 2580 个查不到几何的房间目标（每页 256 个
    付两次全量查询、约 88 秒，把模型侧真正的失败埋进日志）。那批目标已经收干净
    （现场 `/update/pending-units` 的 `room_units` 为空），维持关闭的代价此刻更
    贵：关着时房间归属**只在删除路径**还会被清理（`helper.rs` 的
    `delete_room_membership` 从不看这个开关），搬家后的重算全靠启动全量重建
    回补，而那条兜底路径排在 `startup_autorun` 之后（`skip_startup_room_build`
    的三道门次序）——默认部署两个开关都关着，等于没有回补通道。
  - 门本身一处没动：两个写入点（直写事务的 `room_recalc` 语句、暂存窗口收口的
    `merge_room_recalc_changes`）与一个消费点（`room_round`）照旧读同一个函数，
    翻的只是缺省值。显式写了 `room_incremental = false` 的配置（`python/tests/
    DbOption-ci.toml`、`python/testbed/*`）行为不变；要临时关一次用
    `AIOS_ROOM_INCREMENTAL=0`，不必改文件。

- **空间树一致性闭环：V2 单文件快照、进程状态机、空间串行锁与降级自愈**（方案
  `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`，D1–D8 已定；
  ADR-010 2026-08-12 增补（二）、ADR-017 2026-08-12 补记）：
  - 快照介质：`accel_tree_{project}.bin` + `.meta.json` 退役，改为单文件
    `accel_tree_{project}.snapshot`（V2：树载荷 + SHA-256 自校验 + project/namespace
    身份 + 双字段指纹，原子 rename 发布）。读侧全套校验任一失败即指针重建，不回落
    旧格式；旧文件仅作一次性迁移候选（双指纹匹配且无 pending → verdict=`migrated`），
    首次 V2 发布成功后**删除**——旧二进制对 bin 缺失是无条件重建，任何回退自动安全。
  - 状态机 `spatial_state.rs`：8 态；房间消费者（启动全量重建、RoomRecalc、空闲
    房间轮）仅 Ready/ReadyEmpty 放行（`SPATIAL_TREE_NOT_READY`，durable 行保留），
    解析/生成/重放/重建/`model.spatial.bounds` 不受门禁。启动判据修正：pending
    优先仅对可读快照成立、进入 ReplayRequired 立即重放（不等派发门）、
    「树非空即 preloaded」收窄为显式夹具标记。
  - 空间串行锁 `SPATIAL_STATE_SERIAL`（`STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    → GLOBAL_AABB_TREE`）：staged 提交后收敛、direct 写路径、重建换树/发布、快照
    落盘、Python `spatial.*` 同一串行线（修掉 Python reconcile/persist 与 worker
    并发动树的竞态）；journal 写回与尾事务不持锁。
  - 指针重建：record-range 分页（fork 兼容套件双跑钉住页间无漏无重）；口径
    current-only（排除版本化数组 id 行、`in.deleted` 软删行，Rust 侧排除
    NaN/Inf/反向 AABB 并计数采样）；分页读锁外、stamp 前后比对 + 换树 + 发布锁内，
    三连漂移/查询失败进 DegradedBlocked。房间覆盖率分母同口径。
  - 降级自愈：后台 revalidator（30s 指数退避至 5min）只管 DegradedReuse/
    DegradedBlocked，恢复 Ready 唤醒调度器。
  - 崩溃注入：`AIOS_FAILPOINT=<name>` 五个注入点覆盖方案 §8 崩溃窗口。
  - **对外契约变化**：/health `spatial_tree` 九键作废换十五键（台账 G-02 契约
    迁移，形状钉随迁）；`startup_verdict` 枚举改
    reused/replayed/rebuilt/migrated/degraded/preloaded；Python
    `spatial.persist(force=True)` 在非 Ready/ReadyEmpty 拒绝。
  - 沙箱验收（testbed @8019，六场景：首启重建/快路径复用/截断/删除/rename 前
    崩溃注入/崩后收敛）全过，证据
    `docs/2026-08-12_spatial-tree-consistency-acceptance.md`；E3D 侧场景
    （TTY 复制恢复对拍、伪造旧 epoch、房间边对拍）留 runbook 待跑。

- **db8000 会话快照夹具通用管线阶段一**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` §1）：
  切割（`session_cut`）/格式（`aios-session-fixture-v1`）/打包（`archive_util`）
  /管线（`pipeline`）四个模块接上 `db_session_fixture` bin——`pack` 把
  「recording.json + 源 DB 文件」打成只入库最终文件的夹具（台账 sesno 逐切过
  sesno+存在性验证闸、散列入 manifest、6 MiB 预算、收尾即复验），`verify` 对
  夹具目录零外部依赖离线复验（解 zip → 逐台账**现切** → SHA256/大小对账 +
  验证闸，与阶段三回归同一套裁决）。阶段一验收由
  `tests/db_session_fixture_selfcheck.rs` 钉住：用通用模块从 issue-019 zip 的
  final（sesno 26）现切 24/25/26，字节散列与该夹具 manifest 台账逐一相等——
  「任意历史可从最终文件精确还原」在真实 db8000 会话链上成立。同一测试还覆盖
  **pack 往返**（真实源文件 → 夹具 → 复验全绿；台账散列与 issue-019 独立录制
  的那份相等；台账改一位后复验必须变红），因为阶段二的 E3D 录制是一次性的、
  pack 出错要再占一个生产空窗重录。issue-019 专用实现保持冻结不动。该测试已
  接进 CI（`windows-tests.yml` 的 `db8000-model-increment` job，参数与
  issue-019 步骤逐字同款），同批把一直漏在门外的离线解析边界用例
  `--test pdms_record_boundary` 也接了进来。

- **阶段二录制工具**（同方案 §2，待生产空窗执行）：
  `scripts/e3d/Record-Db8000SessionChain.ps1` + 清单驱动的
  `scripts/e3d/db8000_recording_cases.json`（加案例 = 加一对宏 + 一行），投递走
  ADR-019 的 `l3_suite --check-driver`。录制一次性且占生产空窗，所以三道闸都当场
  验：触碰 E3D 前静态审宏（恰好一个 `SAVEWORK`、无 `QUIT`/`FINISH`/`MERGE`/
  `PURGE`、`ALPHA LOG` 成对、`Q REF`+`Q NAME` 齐全）、每条腿后要求 sesno 恰好 +1、
  refno 从宏日志的 `Ref`/`Name` 相邻对回读。配套给 `db_session_fixture` 加
  `inspect` 子命令（只读打印会话链 JSON，与切割同一份解析）。`-CheckOnly` 只读档
  已对真实 `ams8000_0001` 实跑通过（baseline_sesno=210），检查器的拒绝面亦验过。

### 修复

- **直写路径的空间树变更补上 epoch 痕迹，消除崩溃后的静默漂移**（方案
  `docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`，
  ADR-010 2026-08-12 增补）：
  - 钉死不变量——**凡是改变了「树应有内容」的已提交变更，都在同一事务内 bump
    `spatial_epoch:current`**。此前只有 durable 增量与暂存窗口尾事务 bump，
    全量生成 / `manual_update_aabbs` 的普通直写刷新与删除清理两条路既不写
    `spatial_reconcile` 意图行、也不 bump：树同步完、空闲轮落盘前崩溃，重启时
    sidecar 与库指纹相等，启动判据按 Reuse 复用一棵陈旧的树，而 /health 的
    `drift` 恒为 false，无人可见。删除路径的后果尤其重——启动全量房间重建会把
    被删构件按旧包围盒重新收编进 `room_relate`（ADR-010 D4 借崩溃复活，而
    `DeleteCleanup` 任务早已 done，没有重放会再清一次）。
  - `update_inst_relate_aabbs_by_refnos_mode` 的直写事务门控从
    `durable_room_trigger && !chunk_changes.is_empty()` 放宽为
    `!chunk_changes.is_empty()`；`durable_room_trigger` 从此只决定「要不要随事务
    发布 `room_recalc` 任务」，不再决定「要不要事务与 bump」。重算值与树上旧值
    逐位相等的重刷仍走普通写、不 bump——没动树的提交不该作废别人的树文件。
  - `delete_room_membership` 的窗口外分支改为按块「取写锁 → 锁下探测这些 refno
    在不在树上 → 在则把房间边删除与 bump 包成一个事务、不在则照旧普通写 →
    摘树 → 标脏」。探测在锁下做，「要不要 bump」与「树到底动没动」由同一个快照
    裁决。暂存分支不变（意图行 + bump 仍由窗口尾事务收口）。
  - 普通直写分支补上写锁，跨度 [变更判定 → 事务 → 树同步]（durable 增量的全跨度
    锁不变）。顺带关掉一个此前没盘到的交错窗口：并发的删除清理挤在事务与同步
    之间时，刚摘掉的条目会被这里同步回树上，成为要等下次指针重建才自愈的幽灵。
  - 行为变化：全量生成、`manual_update_aabbs`、删除清理的直写提交现在会推进
    spatial epoch（按块，一次全量生成约产生「条目数/100」次 bump）。多次 bump
    语义无害（判据只比相等），代价是这些路径跑过之后，下次启动的全量房间重建
    对账凭据（`room_build:main`）会判为「空间状态已变」而照跑一次。

### 变更

- **`aios_db.model.export_obj` 改为整树单文件导出，子树收集走 anc 索引**
  （2026-08-12 增量审查修复计划 P3）：
  - 对外契约变化：此前每个实例根一个 `{refno}.obj`，现在整棵子树合成一个
    `{refno}.obj`、内部按「实例_geo_hash」分 `o` 组，`files` 恒为单元素数组；
    交付单元根（EQUI/BRAN…）自身没有直接实例行也能导出整树。
  - 子树实例收集只走 `anc CONTAINS`（`idx_inst_relate_anc` 索引查询，anc 含
    自身故根自己的实例行同谓词圈住），不再 OR 无索引的 `in = …` 臂——那会把
    整条谓词退化回全表扫（preload.rs 实测账：1.57s vs 3.1ms）。
  - 响亮失败取代静默空集：refno 解析失败直接报错（此前静默成 0、谓词永不命中）；
    空结果时按 rs-core `inst_relate_anc_ready` 同口径探一次 `anc = NONE`，
    存量未回填的库给自愈指引（启动一次 gen-model 回填）而不是谎报「没有实例」。
  - testbed 全链路（`python/testbed/run_full_loop.py`）导出步骤补形状断言：
    单文件、`o` 组数 == 导出实例数、triangles > 0、无缺失 mesh。

- **`aios_db.full_init` 增加同工程活服务探测**（行为变化：以前能起的场景现在
  可能被拒）：拿锁之后探本机 `http_api_addr` / 8022 / 9099 的 `/api/v1/health`，
  响应是合法 health JSON **且** `project` 与本配置一致就报错退出，
  `full_init(..., force=True)` 显式跳过。动机是单实例锁按「项目根」隔离，两个
  部署包各持各的锁却写同一个工程时锁根本不挡（实测踩过：`test-worklspace`
  的包在 9099、本仓库在 8022）。判据只认 project 名——`/health` 不报「它连的是
  哪个 SurrealDB」，所以隔离沙箱若与生产**重名**会被误伤，用 `force=True` 放行
  （`python/tests/conftest.py` 就是这么做的，并在注释里写明三条资源如何独立）。

- **互踩探测精确化：/health 补报库端点，探测端按「同库」而不是「同名」判**
  （上一条落地当天就被自己的测试沙箱误伤，这是补上的另一半）：
  - `/health` 的 `sul_db` 新增第六键 `endpoint`（配置的 `v_ip:v_port` 原样
    字符串），形状钉与 spec §4.1 同步；`sul_db` 其余五键语义不变。
  - `full_init` 的探测升级为三层判据：`project` 不同 → 无关；服务端报了
    `sul_db.endpoint` → 端点（localhost↔127.0.0.1 归一后）或 `namespace`
    不同都放行——同名工程各写各的库不构成互踩；老版本服务端（≤0.1.18）不报
    端点 → 分不清仍按最坏情况拦，拒绝文案会写明是哪种判法。判定函数是纯函数，
    8 条单元测试钉住（`cargo test -p aios-py --lib`，含对实测 9099@0.1.13
    响应形态的老服务端分支）。
  - `python/tests/conftest.py` 的 `force=True` 暂留：本机 9099 还跑着 0.1.13，
    等同机部署升到带 `endpoint` 的版本即可撤。

- **`aios_db.spatial.tree_status` 的文档与存根不再复述键面**：改为「原样透出
  /health `spatial_tree` 那份渲染，键面以 Rust 侧渲染半边为唯一权威」。此前注释
  与 `.pyi` 各抄了一份九键清单，而 G-02 契约迁移正把它往十五键上带——两处各说
  一套，过期是必然。Python 面只钉判漂移要用的稳定核（`entries` / `file_epoch` /
  `db_epoch` / `drift` / `startup_verdict`），全集的形状钉留在 Rust 侧一处。

- **`aios_db.db.inst` 去掉全表扫，改三段式取边**：① `anc CONTAINS`
  （`idx_inst_relate_anc` 索引查询，anc 含自身故一跳圈住整棵子树）；② 空结果
  回落 `array::flatten(SELECT VALUE ->inst_relate FROM [pe:…])` 图跳，只取元素
  自己那一跳（preload.rs 的实测账：`in` 谓词全表扫 1.57s vs 图跳 3.1ms），兜住
  `anc` 未回填的存量库与直接 `RELATE` 出来的测试夹具；③ 两条都空且库里还有
  `anc = NONE` 行时响亮报错——「查不全」不能被读成「没有」。refno 解析失败也
  改为直接报错（此前 `unwrap_or_default()` 静默成 0，谓词永不命中）。与
  `export_obj` 的差别是多了第 ② 段：那边空结果本就是错误条件，这边空是合法答案。

### 新增

- **`aios_db` 补齐测试支撑面导出，并新开房间增量 pytest 轨**：
  - 新增 `aios_db.fixture`（`create` / `drop` / `move_body` / `refnos`），直通
    `src/fast_model/room_fixture.rs` 的合成房间夹具（1 间 `/ZZ-R-K100` + 2 块
    PANE + 5 个盒形构件，其一骑在重叠区，保留 refno 段 4000000001）——与 Rust
    `room_fixture` live 轨共用同一套数据，两侧断言可互相印证。会写 pe/FRMW/
    inst_*/geo_relate/aabb/vec3 多张表并落 `zzfx_*.mesh`，**只对一次性测试库使用**。
  - 新增 `room.enqueue(changes)`（按 `model.update_aabbs` 的返回形态入队房间
    重算，PANE 走整间分支、其它走元素分支，不受 `room_incremental` 开关门控）、
    `model.delete_subtree(refnos)`（DeleteCleanup 补偿任务同一入口的级联删除）、
    `spatial.tree_status()`（空间树九键指纹，与 /health `spatial_tree` 同源、
    现读现比）、`model.update_aabbs(..., durable=True)`（生产 TransformOnly /
    定向 regen 走的直写事务路径：AABB 指针、`room_recalc` 任务与 spatial epoch
    同事务提交）。
  - 新增 `python/tests/`：对 conftest 自起的一次性内存 SurrealDB
    （`bin/surreal.exe` @8071，进程退出零残留）跑「房间增量收敛 == 全量重建」
    的**逐边**对拍，覆盖构件搬家、面板整间、空刷负例、删除清边留痕、durable
    直写五条；配置 `tests/DbOption-roomtest.toml`（`room_key_word=["ZZ-R-"]`
    只圈夹具房）。conftest 会把仓库根同名空间树文件挪开再还原，不毁真项目产物。
  - 类型存根补齐（新增 `fixture.pyi`，`model` / `room` / `spatial` /
    `__init__` 同步新入口），`py.typed` 对外契约不再漂移。

- **绑定的离线测试档进 CI**（`python/tests -m offline`，60 条）：解析层对着仓内
  `issue-019` 的 db8000 会话快照（与 Rust `db8000_two_delete_fixture` 同一份
  数据、同一串删除序列）、三层硬守护在干净子解释器里逐条验、`.pyi` 与运行时的
  名字集合逐模块对齐、HTTP 客户端对着打桩服务验 12 条 REST 路由与报文形状。
  这一档不连 SurrealDB、不碰 E3D 装机、不扫项目目录，秒级跑完。
  - `.github/workflows/windows-tests.yml` 新增 `python-bindings` job：复用
    `windows-binary.yml` 的 OCCT / protoc provisioning（绑定按 Q7 钉死「与服务
    同一套默认 feature」，必须有 OCCT）→ `maturin build` → 装 wheel → 跑离线档
    → wheel 作 artifact 上传。原 `db8000-model-increment` job 不动。
  - conftest 按选中集合裁定本进程那一份 DbOption（进程级 OnceCell，换库只能换
    进程）：有房间档用例就用 `DbOption-roomtest`，纯离线档用新增的
    `DbOption-ci`。离线用例在任一配置下都成立，两档同跑不冲突。
  - 新增连接层行为用例（`test_connection_layer.py`）：`db.inst` 三段式的每一段、
    `owner_chain` 的自 own 终止、`members`、`spatial.tree_status` 十五键形状钉。

- **版本护栏自锁**：`aios_client.EXPECTED_SERVER_VERSION` 是手抄常量，此前
  `chore(release)` 升 `Cargo.toml` 时没有任何东西提醒同步它——护栏自己先漂移，
  从「提醒你版本不一致」退化成「对着新服务端瞎报警、对着老服务端不报警」。
  离线档新增一条对表用例，bump 忘改常量时 CI 立刻红，红处文案直接写修法。
  `sul_db.endpoint` 恰好是 0.1.19 起才有的键，这条对表马上就有实际意义。

- **空间树一致性闭环落地后的绑定侧跟进**（对方工作流 `445e3cd1` 合入后复核，
  绑定重建 + 全套复跑，77 条绿）：
  - conftest 的树产物搬挪清单补上 V2 单文件快照 `accel_tree_{project}.snapshot`
    ——房间档跑在一次性内存库上，但空间树落盘写的是**仓库根**、文件名与真项目
    同款；清单漏项就会拿测试产物顶掉真项目的快照（介质迁移期间已实际残留过一个）。
    并加源码钉：从 `aabb_tree.rs` 反查 `accel_tree_{}` 的全部后缀，与清单比对，
    下次换介质忘了跟进直接红。
  - `spatial.tree_status` 的断言从「稳定核子集」收紧为**逐键全等**。钉的不是
    契约本身（那在 Rust 渲染半边旁），而是绑定透传没掉键——最容易掉的是取值为
    null 的几个（`snapshot_sha256` / `pending`），掉了不报错，只会让照着 /health
    写的脚本在绑定上撞 KeyError。
  - 钉住「Python 夹具路径免标记通过消费者门」：Rust live 夹具要显式调
    `mark_spatial_tree_fixture_preloaded()`，Python 这条路不需要——`full_init`
    走正经装载器，空库上从指针重建出空树即进 ready 态。省得下次门禁收紧时对着
    一堆 `SPATIAL_TREE_NOT_READY` 猜是不是缺了那个标记。

- **`scripts/smoke_m1..m5.py` 标注退役**：五个脚本全部钉在仓库根 `DbOption` +
  8009 正式库 + `D:/AVEVA/...` 真实工程上，而 8009 的数据目录已决定不修，照原样
  跑必失败。不删（它们是 M1–M5 的验收口径记录），改为每个脚本头注写明「历史
  验收记录 / 为何跑不了 / 等价物在哪」，README 脚本表合并成一行同款提示。
  多数段落已被两档 pytest 覆盖；`parse.noun_dict` 依赖 E3D 装机的 `attlib.dat`，
  没有自动化等价物，头注里点名说清。

- **`aios_client` 版本漂移护栏**：`health()` 比对服务端 `version` 与内置
  `EXPECTED_SERVER_VERSION`（现 0.1.18），不一致抛一次 `AiosVersionWarning`
  （同一个 client 不刷屏），`AiosClient(..., expected_version=None)` 关掉。
  回应实测踩过的 0.1.13 绑定对着 0.1.16 部署包查半天的坑——只告警不报错，跨版本
  多数字段仍通用，硬拦会把「凑合能用」变成「完全不能用」。

## 2026-08-11

### 变更

- **层级查询优化 P3（gen-model 份额）：退役 `inst_relate`/`tubi_relate` 的
  `zone_refno` 列**（方案 `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`
  P3，as-built 记录 `docs/2026-08-07_journal-fn-dependency-audit.md` §4）：
  - 写侧全退：建行字面量（普通行 + TUBI 行）不再写 `zone_refno`，
    `ResolvedInstMeta` 的 zone 槽位与 `resolve_inst_meta` 的 noun 预取移除；
    回填 `backfill_inst_relate_anc` 与 OWNER 搬迁重算
    `render_anc_repair_statements` 不再连带 `zone_refno = fn::find_ancestor_type(...)`
    ——每行一次的 9 跳 owner 上溯从回填成本中整个消失，`fn::find_ancestor_type`
    自此离开 inst 写入链（函数定义保留给材料表等读侧）。
  - 收口探针收窄：`desi_finalize_preflight` / `selfcheck_surreal_functions`
    只探剩余的收口硬依赖 `fn::anc_u64`。
  - 索引迁移：`INST_RELATE_INDEX_SQL` 前两行 `REMOVE INDEX IF EXISTS` 在启动/建窗时
    摘除旧 zone_refno 索引的两个历史名字（`idx_inst_relate_zone_refno` 本仓 F1
    修复后建的、`inst_relate_zone_refno_index` plant-ui rs-core `define_pe_index`
    建的，AMS 实库两者并存实测在案）。存量行旧值保留不删，只是不再写入、不再被
    索引；「索引不存在 / 表都不存在 / 重复摘除」三种 no-op 情况由双跑用例
    `dual_inst_relate_anc_u64_contains_index_agrees` 连 INFO 终态一起钉住。

### 新增

- **`fn::zone_u64` / `fn::site_u64`（common.surql，P3 读侧便捷层）**：从 `anc`
  链尾 O(1) 定位 ZONE/SITE，与元素深度无关——链尾打包值 ref1==0 即 WORL 的
  自适应偏移，判据与 Rust 解析器同源；「含自身」语义与退役的
  `fn::find_ancestor_type` 口径一致，短链/空链返回 NONE。反向圈行（某 ZONE 下
  全部实例）仍走 `anc CONTAINS` 索引查询，不用它。两种链尾真实形态（悬空 WORL
  收尾 / 0_0 哨兵被滤止于 SITE）由 `zone_and_site_helpers_locate_from_the_anc_tail`
  与双跑用例 `dual_anc_u64_functions_execute_and_agree` 双引擎钉住。

## 0.1.18 - 2026-08-11

### 变更

- **空间树启动初始化改为分层判据**（方案与决策记录
  `docs/2026-08-11_spatial-tree-startup-init-plan.md`，ADR-010 2026-08-11 增补）：
  - sidecar 指纹从单一 epoch 数值扩成 **(epoch 值, 库侧 bump 时刻 `updated_at`)**
    双字段，两个字段都与库相等才直接复用树文件（评审要求：与数据库对时间戳；
    库快照回滚恰好撞回同一计数也认得出来）。旧版 sidecar 缺新字段按失配走，
    一次自愈后补齐。
  - 指纹失配但库里还有待重放空间意图 → 复用文件、交给 worker 出队前的意图重放
    自愈（不再像旧 epoch 校验那样每次崩溃重启都全量重建）；失配且无意图 →
    只读指针重建（直写崩溃 / 换文件 / 回滚库）。
  - 树文件缺失/损坏从「空树等人工」改为**自动指针重建**（决策 D1）；库侧诊断
    查询失败降级复用文件 + 告警（D2）；两处启动调用点统一为「告警降级空树、
    不阻断启动」（D3）。
  - /health 新增 `spatial_tree`：文件/库两侧指纹现读现比、`drift`、条目数与
    本次启动裁决（reused / healed_by_replay / rebuilt / empty / preloaded /
    reused_degraded）。

### 修复

- AMS/8000 房间增量灰度闭环：
  - `inst_geo` 的确定性落库从忽略重复改为 `UPSERT ... MERGE`，保留已有 mesh/AABB，
    同时补齐缺失参数并在显式重生成时清除 `bad`；重复执行同一生成批次可收敛。
  - 无圆角、共线回折的房间面板不再走一次性删点失败分支，统一使用逐交点修复器；
    加入 AMS 真实 PLOOP 参数回归。
  - `startup_autorun=false` 时，显式 `POST /update/execute` 即使所选 dbnum 已追平，
    也会为本进程上弦并放行 durable 模型/房间积压，避免人工 canary 永久停在
    `up_to_date`。
  - `Run-RoomE3DE2E.ps1` 新增复用现有 9099 服务的 `db8000-equi-copy` 案例；
    TTY 宏对 `=24384/24776` 执行 probe/apply/restore，并核对水位、新 EQUI、
    `inst_relate`、pending/dead-letter 与空间补偿。

- `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值（1/true/yes/on）：旧实现判
  `is_ok()`，部署模板写 `=0` 想关闭，实际每次启动都强制全量指针重建。三态解析
  收口在 `batch_worker::parse_explicit_flag`，与 `GEN_MODEL_DIRECT_INCREMENT`
  的 P2-1 纪律同款。

## 2026-08-06

### 新增

- **执行范围缓存 + 周期对账重扫**（现场：数据批次执行中 SUL_DB 连接抖动，watcher 的
  范围解析报 `receiving from an empty and closed channel`，整批文件事件被丢弃且无重试）：
  - 文件事件路径的 MDB 范围解析改走进程内单槽缓存（`UpdateScope::resolve_cached`）。
    名单只在 SYS meta 批次落库时才变，那一刻与 `SCOPE_DIRTY` 同点显式失效；TTL 兜底
    `AIOS_SCOPE_CACHE_SECS`（默认 300s，0 关闭）。SUL_DB 瞬时不可用时暖缓存放行并告警，
    冷缓存与配置错误（mdb_name 没填 / MDB 名不存在）维持 fail-closed 上抛。
    启动重扫、重挂补扫、周期对账与手动路径仍每次真查（fresh），它们就是缓存的刷新点。
  - watcher 事件循环新增周期对账重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s，0 关闭）：
    按间隔整面重比「文件最新会话号 vs applied 水位」，把连接抖动、服务重启等一切来源
    丢掉的文件事件在一个周期内追回；入队按水位判定天然幂等，与启动重扫共用
    `sweep_watch_dirs`。

- **issue #10 复现套件**（`src/data_interface/staging/issue10_add_node.rs`，仅测试编译）：
  用真实渲染与真实暂存窗口（`stage_parsed_window` → `register_staged_finalize` →
  `commit_registered_to`）在 mem 引擎上模拟 E3D「复制 BRAN 并 SAVEWORK」的连续增量，
  钉住三条路径——连续多个窗口写回后新增节点必须出现在模型树（含父成员序边重建）；
  窗口因生成重试耗尽被阻断时「检测得到、树不动、水位原地」，吸收重置后重算收敛；
  journal 被持久层确定性拒绝（坏版 `update_dbnum_event` 对字符串 id 的 pe 行报
  `array::at` 类型错）时写回整体回滚、零半写，排毒后同一份 journal 重放收敛。
- **批次执行的阶段日志**（issue #12）：完成行补上 sesno 窗口与墙钟完成时间，在 E3D 里
  SAVEWORK 的人可以直接对上「屏幕上这批日志是不是我刚才那次保存触发的」；模型计划按
  action 分组计数（形如 `regen_root=3 transform=12`）而不再只报总数；交付单元、批量
  重生成、房间归属重算各自报耗时与成败；生成根列表超过 8 个截断并报出总量。

### 变更

- `DbOption.toml`：`manual_db_nums` 由 `[7998, 8000]` 放宽到 `[7997, 7998, 7999, 8000]`，
  纳入 issue #10 的 E3D 实测窗口（基线库 `.surreal/ams-7997-e3d-test-20260805`，7997
  applied=92）。取证结论就地记在配置注释里：issue 截图中的 `/1WCC0211` 属于 7999，而
  7999 一直被排除在手动窗口外（applied=3、file=41），库里的树是旧全量同步的残留。
  实测跑完可收窄回 `[7998, 8000]`。
- 房间轮次日志不再只在「队列跑空」那一轮打印，距上轮超过保底间隔触发的那轮同样报出
  目标数与死信数。

### 内部

- `attempts::record_window_block_at_on` 提升为 `pub(crate)`，`StagedFinalize` 增加
  `Debug`，供复现套件构造阻断现场。
- `src/bin/manual_scan_probe.rs`、`src/test/mod.rs` 仅 `cargo fmt` 排版整理。

---

## 历史记录

1、添加自动增量更新文件的修改，启动时会检查当前数据库和E3d数据库的一致性
## 2026-08-14

### 新增

- Python 解析层新增 `aios_db.parse.net_window(path, start, end, detail=False)`：复用生产
  `net_window::collect_net_window`，只读 dabacon 文件即可得到属性语义上的净增删改，
  并透出 `unchanged_rewrites` / warnings。与原 `parse.net_changes`（索引记录位置触达
  三态）分工明确：E3D TTY 的 apply + restore 合并窗口会过滤已恢复的业务属性，
  同时如实保留 E3D 自增的 `CACHID` 等保存期元数据，不需要反查 SurrealDB。
- 新增 `scripts/e3d/Test-TtyNetWindow.ps1`：自动执行 FTUB apply / 解析器断言 /
  restore / 合并窗口断言，`finally` 保证恢复腿执行，并产出 baseline 副本、语义 diff、
  命令退出状态与 rollback 验证记录。
## 2026-08-14

### 新增

- 定义严格的 SYS/DICT → CATA → DESI → 模型 → 房间初始化阶段与跨项目元件库优先级契约。

### 修复

- 初始化模型生成将受数据就绪门控，避免 DESI 或模型越过尚未完成的元数据、元件库阶段。
