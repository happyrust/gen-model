# ADR-010：房间归属增量更新——AABB 差异驱动 + 混合粒度 + 共享判定谓词

> **2026-08-09 修订（取代 2026-08-05 的暂存房间方案）**：稳态窗口只在 kv-mem
> 计算数据与模型。尾事务把房间重算意图、空间收敛意图和水位一起持久化；提交成功后
> 依次完成空间树收敛、释放 kv-mem，再按本任务精确 scope 从 RocksDB 计算房间。
> 单目标失败保留 durable pending，任务记为 `partial`，空闲轮继续恢复；历史积压不在
> 当前任务的立即轮中顺带消费。房间拓扑与关系不再预载进 kv-mem。

状态：已接受。落地进度——

- 缺陷修复：**D1**（`update_world_transforms` 补 AABB 刷新 + `update_inst_relate_aabbs_by_refnos`
  按旧值清理 R 树旧条目）、**D8**（`sync_aabb_tree_with_db` 按条目数与库对账，取代原先
  只看 `is_empty()` 的重建条件，已实跑 45→403）、**D9**（`world_trans` 改回写
  `trans:⟨hash⟩` 记录链接，此前写裸对象会让所有 `world_trans.d` 读者取到 none）。
- 本 ADR 第 3 条（**共享判定谓词**）：已落地。`room_predicate.rs` 是唯一实现，
  `cal_room_refnos` 改为调它，4 个单测 + 夹具端到端均通过，行为与重构前一致。
- 本 ADR 第 5 条（**多归属排序**）：已落地。`cal_room_refnos` 改为返回带强度的
  `RoomMember`，`save_room_relate` 写 `inside_count` / `center_dist`；SQL 侧新增
  `fn::room_relate_of` / `fn::room_num_of` 两个共享函数（`common.surql`），
  `fn::room_code`（hd 与 hh 两份）与三处 `fn::get_room_number` 全部改为调它。
  夹具实测分数正确：完全在内的 8/8，跨界件 4/8、两室距离各 450。
- 本 ADR 第 6 条（**两树合一**）：已落地。`rs-core-pin` 侧删除 `GLOBAL_ROOM_AABB_TREE`
  与 SQL 已损坏的 `load_room_aabb_tree`，`query_room_panel_by_point` 改为在
  `GLOBAL_AABB_TREE` 上按 `noun == 'PANE'` 过滤，同时把原先的 `.unwrap()` 换成 `?`。
  两仓均通过编译、5 个不连库单测通过。**夹具端到端已在一次性内存实例上复跑通过**
  （此前欠的那次复跑：当时本机全部 SurrealDB 实例被本会话之外的一方关停，
  详见审计报告 §4.7）。
- 本 ADR 第 4 条的**删除例外**：已落地。`delete_inst_relate_subtree`（`DeleteCleanup`
  与补偿路径共用的入口）在级联删几何之后，再清一遍房间归属：`room_relate` 与
  `room_panel_relate` 的**两个方向**都删——作为成员是入边，作为面板还有出边；不按 noun
  分情况，因为 `pe.noun` 此刻已随软删一起不可靠，而对非面板元素那两条子句本就是空操作。
  同时把被删元素从空间树上摘掉，为此在 `rs-core-pin` 新增 `remove_by_refnos`（缺陷 D4）
  ——`rstar` 的 `remove` 要求按整值相等匹配旧包围盒，而删除路径手里没有那个值，
  留在树里的话 `locate_intersecting_bounds` 会继续把它当候选，重算就会把一个已经不存在
  的构件算进某间房。
  刻意**不**与 `delete_inst_relate_cascade` 合成一个事务：那个函数同时服务于重生成时的
  「先删旧几何再写新几何」，而那条路径上元素还活着，它的房间边不该被动。两段各自幂等，
  中间崩了 `DeleteCleanup` 任务会重试。
  `update_aabbs` 自身那个写反的条件仍未修（gen-model 侧已在唯一调用点绕过），本轮只加
  了 `remove_by_refnos`，没有动它。
- 本 ADR 第 9 条（**验收对拍**）：已落地并跑通，这是本 ADR 唯一的硬标准。
  `live_room_fixture_parity` 守全量侧；新增的 `live_room_incremental_parity` 守
  「增量收敛结果 == 全量重建结果」：全量建基线（6 条边）→ 把一个构件从 A 房搬进
  B 房 → 刷新包围盒拿到变更集并入队（顺带断言排出来的正是
  `room_recalc_element_4000000001_20` 这一行）→ 只跑元素分支 → 在同一份数据上
  再跑一遍全量 → 逐边比较。两侧相等**之外**还断言搬家确实发生了（B 房收着它、
  A 房不再收着），否则两边同时算成空集也会「相等」。
  整间分支另有 `live_room_panel_move_parity`（搬动面板，跨界构件掉出该房），同轮冲突规则
  另有 `live_room_panel_task_absorbs_element_task_in_the_same_round`（面板任务与其成员的
  元素任务同轮入队，跑完整第三阶段，断言边集一条不变且两行队列都被消费）。删除路径另有
  `live_room_delete_clears_membership`：先删一个普通构件（只有入边），再删一整块面板
  （还有出边与 `room_panel_relate`），并确认两者都从空间树上摘掉了。
  五条用例都在 fork 版 SurrealDB 的一次性内存实例上实跑通过；它们**只能逐个运行**
  ——`SUL_DB` 是进程级全局而每个用例各建一个 tokio 运行时，第一个用例结束时连接的
  后台任务就死了，连接函数里已把这条约束写成明确的报错。
- 本 ADR 第 8 条（**先清后写**）：**全量侧已落地，缺陷 D6 随之关闭**。
  `save_room_relate` 与 `room_panel_relate` 的写入都改为「先 DELETE 本端旧边、
  再整批 RELATE」包在一个事务里（`wrap_in_transaction` 提为 `pub(crate)` 供两侧复用），
  后者补上确定的 `{room}_{panel}` record id，两处 `SUL_DB.query` 都补了 `.check()`。
  7 条不连库单测钉住四条性质：DELETE 排在所有 RELATE 之前且同处一个事务、
  成员集为空时仍然发那条 DELETE（面板挪走后旧成员必须掉）、边 id 由两个端点推出、
  渲染结果对 `HashMap` 的遍历顺序不敏感（对拍与重放都押在这条上）。
  连带清掉三处静默：`build_room_relations` 的三个 `.unwrap()` 改为按面板聚合失败原因
  再统一上抛，单块面板算不出来不再拖垮其余 123 间；`cal_room_refnos` 里
  `query_insts` 的 `unwrap_or_default()` 与两处 `let Ok(..) else { continue }`
  改为带上下文上抛；启动调用点（`lib.rs:241`）相应从 `.unwrap()` 改为打印告警——
  房间归属是可事后重建的派生数据，不该让一次面板失败顶掉 `async_watch` 之前的整个启动。
  另修一处两表对不上的老问题：`build_room_panels_relate_common` 此前只返回一个列表，
  命名校验没通过的房间照样被写进 `room_relate`、而它的 `room_panel_relate` 被跳过；
  改为返回 `RoomPanelMap`，把「排除集须覆盖所有面板」与「只有合规房间参与归属计算」
  分成两个字段。
- 本 ADR 第 1/2/7 条与第 8 条的元素分支（**队列骨架**）：已落地。
  `ModelWorkAction` 新增 `RoomRecalcElement` / `RoomRecalcPanel`；`record_id` 按
  action 分支，房间任务的行 id 是 `{action}_{target}`、**不带 dbnum**；复活语义随之
  改为由队列 revision 驱动——行既然不带 dbnum，同一块面板会被不同库的会话轮流触发，
  跨库比较 sesno 没有意义，而房间任务的入队条件本身就是「AABB 真的变了」，每一次入队
  都是全新的重算理由。`dbnum` 与 `source_end_sesno` 仅保留为追踪字段；当前几何刷新层
  拿不到可靠来源，统一写 0，不参与排序、去重或复活。
  `drain` 由两阶段变三阶段，新增的 `drain_rooms` 排在 regen 之后；手动路径
  （`manual_update`）当时在单元重生成之后补了同一阶段——该安排已被 ADR-011
  合流取代：手动与自动共用一条数据批次队列，房间收敛统一在 worker 空闲轮的
  `room_round`（ADR-011 §8），不再挂在手动路径里。
  两条重算分支落在 `room_model.rs`：整间分支 `recalc_panel_membership` 复用
  `cal_room_refnos` + 先清后写并返回本次写入的成员集合；元素分支
  `recalc_element_membership` 从全局树按 `noun == 'PANE'` 取候选、调共享谓词
  `element_in_panel`、再按「删该构件的所有入边 → 写回」落库。两条分支共用判定、
  共用 `{panel}_{element}` 边 id、都是先清后写，因此在同一份数据上收敛到同一个边集
  ——§8 的同轮冲突规则（整间分支已写过的构件，其元素任务被吸收跳过）因此只是省一次
  网格加载与点检测，不再是正确性前提。一处前提要记住：整间分支的元素包围盒取自
  R 树，元素分支取自库，两者由同一次刷新维护，树与库漂移时这条收敛保证同样会漂。
  行级去重不必在内存里再做：不带 dbnum 的 record id 已经保证同一目标只占一行。
  7 条不连库单测守着这些性质，其中一条钉住「每种 action 恰好被一个 drain 阶段消费」
  ——漏掉一种，那种任务入队后就永远躺在表里，不报错也不执行。
  **当前这条队列是空转的**：还没有任何地方入队房间任务，那是第 4 条的事。
- 本 ADR 第 4 条（**AABB 差异触发**）：已落地，队列开始转了。
  `update_inst_relate_aabbs_by_refnos` 改为返回变更集 `Vec<AabbChange>`——新旧两个值
  它本来就同时握着，比一下几乎零成本，此前只算不比、外面拿不到任何信号。没有旧值
  （几何是刚生成的）同样算变。两个调用点各自把变更集转成房间任务入队：
  `increment_manager.rs` 的 TransformOnly 路径（纯 POS/ORI 移动，即「设备从 A 房挪到
  B 房」）与 `occ_generate.rs` 的 regen 路径。按 `noun` 分流，PANE 进整间分支、其余
  进元素分支。任务的 dbnum 跟着 refno 走、会话号取 0——两个触发点都在几何刷新那一层，
  本来就不知道自己属于哪次会话，而房间任务的复活本来也不看会话号。
  **三处刻意的收窄**：
  0. `gen_spatial_tree` 关着时一条都不排。那个开关同时管着全量重建与空间树对账，
     关着时跑增量不只是徒劳——元素分支是「先删该构件的所有入边再写回」，候选面板
     取自那棵没人维护的树，捞不到候选就只剩下那条 DELETE，等于把上一次全量建出来的
     边悄悄抹掉。
  1. regen 路径只在**定向**重生成时入队（`debug_root_refnos.is_some()`，也就是
     `gen_all_geos_data` 区分两条分支用的同一个信号）。全量生成会把整库元素都算成
     「包围盒从无到有」，逐个入队等于给每个元素排一次房间重算，而全量生成本来就以
     `build_room_relations` 的整体重建收尾。
  2. **2026-08-20 由 ADR-040 关闭**：「形状变了但包围盒恰好不变」不再作为例外。
     定向增量的输入已经由工作计划收窄到实际重写/变换目标，这些目标全部保守入队；
     普通全量/维护刷新仍只按 AABB 变更集触发，因而不会制造全库房间任务。保守集合还
     保证直写崩溃重试不依赖“几何前后仍能再次比较出差异”。
- 排序落地时查出 **D11**：hd / hh 两份 surql 无条件按文件名顺序加载，`_hh` 永远覆盖
  `_hd`，与 Rust 侧编译的 `project_hd` 错位。本轮两份都改了排序，但门控未修。
  **（2026-08-04 已修：`run_cli` 在 `define_common_functions` 之后按 `project_hd`
  feature 重放 hd 版 `fn::room_code`，把覆盖矫正回来；`project_hh` 构建无需处理。
  见 `2026-08-04_room-incremental-audit-and-gap-closure.md`。）**
- 2026-07-28 增补（复审后修复：AABB 刷新加固、落盘时机、吸收封闭性、D10）——
  - **AABB 刷新加固**：`update_inst_relate_aabbs_by_refnos` 是增量→房间链路的唯一
    交汇点，此前健壮性远低于两侧：两处 `.unwrap()` 在传输错误上 panic（生产日志
    `_7997_service4.err.log` 有 rs-core-pin `geom.rs` 同类实证，且 panic 后队列行
    既不删除也不 mark_failed，空闲轮无限重试）；UPDATE 缺 `.check()`，语句级失败
    静默吞掉，库里 aabb 停旧值而内存树已更新；反序列化失败 `let Ok(..) else
    { continue }` 静默丢整块；每块先写 `inst_relate.aabb` 指针、**全部结束后**才
    `INSERT IGNORE INTO aabb`——与 D9 教训相反的顺序，崩溃落在窗口内指针悬空。
    已改为：错误全部上抛（分块失败即中止，整批靠幂等重放收敛，与增量主链路同一
    纪律）；两类写入接 `execute_surreal_checked` 获得写冲突重试；每块**先存
    `aabb:⟨hash⟩` 记录、再更新指针**；内存树只在该块 DB 写入成功后才更新。
    `save_aabb_to_surreal` / `save_transforms_to_surreal` 改为返回 `Result`；
    `gen_meshes_in_db` / `process_meshes_update_db` 的同类 `.unwrap()` 一并清除。
    写入顺序有源码钉住测试（`aabb_write_order_records_before_pointers`）。
    注：`gen_inst_meshes` 里 `inst_geo.aabb` 指针有同构的顺序问题，本轮未动，记为
    后续项。**（2026-08-09 已修：vec3/aabb 记录写入挪进每个 mesh 任务、先于其
    `inst_geo` 指针 update；join 之后的全局补写删除。跨任务重复由 INSERT IGNORE
    幂等吸收，不再依赖共享 map 去重——别的任务替你去了重不等于替你把记录写进了库。
    源码钉 `mesh_records_land_before_inst_geo_pointers_inside_each_task`。）**
  - **落盘时机（第 7 条挂起项，已决）**：空间树新增脏标记 `AABB_TREE_DIRTY`，
    两处树变更（AABB 刷新、`remove_by_refnos` 删除清理）后置位；`batch_worker`
    空闲轮收尾（room_round 之后）若脏则 `serialize_to_bin_file`，成功清位、失败
    保留脏位下轮重试，每轮至多一次序列化。全量路径 `gen_all_geos_data` 收尾落盘
    不变（改走无条件 `persist_aabb_tree`）。残余窗口收窄为「树变更后到下个空闲轮
    之间崩溃」的分钟级。
  - **同轮吸收改为封闭性检查（修正第 8 条冲突规则）**：吸收不再只看「整间分支已
    写过该构件」，另要求封闭条件成立：`(构件现存 room_relate 入边的面板集) ∪
    (构件当前包围盒在空间树命中的在册 PANE 候选集) ⊆ 本轮已重算面板集`。不封闭
    （旧边或候选伸出本轮范围）则元素分支照跑。旧边集对本轮全部候选构件一条批量
    SELECT 查完；候选集纯内存树查询；查询失败按「封闭性未知、不吸收」保守处理。
    修复的缺陷：构件同轮从面板 B 搬进面板 A 且 A 在本轮重算而 B 不在时，旧规则
    直接吸收、跳过元素分支「删该构件所有入边」的清理，B→构件的陈旧边无人清除，
    构件同时挂两间房，违背第 9 条「增量==全量」唯一硬标准。纯谓词
    `absorption_is_closed` 有单测；live 夹具新增
    `live_room_cross_panel_move_defeats_absorption` 钉住「跨面板搬家 + 同轮入队」。
  - **D10 已修**：`room_key_word` 补 `#[serde(alias = "room_keyword")]`
    （rs-core-pin `options.rs`），旧键名 toml 不再静默退回默认值。
- 2026-07-28 增补 2（第二轮审核后修复：隐含直管段、差异基线、落盘容错）——
  - **D13（新发现，已修）：隐含直管段（TUBI/BOXI）整体绕过房间增量链路。**
    管段的 `inst_relate` 行挂在 BRAN/HANG 名下（`cata_model::insert_tubi`），`aabb`
    由生成层算好后**插入时直接写死**，out 指向共享单位几何（`inst_info:⟨1⟩/⟨2⟩`，
    其 `->geo_relate` 没有可用的 `aabb`/`pts`）。链路上三处各自把它漏掉：
    ① 刷新层 `replace=false` 的 `and aabb=none` 过滤它、`replace=true` 又因
    `geo_aabbs` 为空 `continue`——**从未进过空间树**，两条房间分支的候选与成员盒
    都取自树，管段因此从未参与过房间归属；② `query_deep_visible_inst_refnos`
    对 BRAN 根只返回子元素、不含分支自身，重生成的刷新集里压根没有管段行；
    ③ 本仓 SurrealDB fork 的 `INSERT RELATION` 撞已有 id 时**静默保留旧行**
    （8009 实测：重复插入返回旧行、无报错），而 regen 的删除集只有
    `inst_info_map` 的键，管段行不在其中——重生成后连库里的 aabb/world_trans
    都停在第一次生成的值。三处齐修：`save_instance_data(replace)` 的删除集并入
    `inst_tubi_map` 键（共享单位几何有引用计数守卫，删单条不伤别的分支）；
    `query_deep_visible_inst_refnos` 补分支自身；刷新层对「geo 侧重算不出而行内
    有既有指针」的行以指针值为当前真值，照常进树、照常参与变更判定。
    live 夹具新增 `live_room_tubi_row_enters_tree_and_tracks_regen` 钉住
    回填 / 幂等重生成 / 跨房搬迁三段语义。
  - **变更判定基线改为空间树上的旧值**（修正 §4 的实现语义）：定向重生成走
    `save_instance_data(replace)` 的「先删行再重插」，行内 `old_aabb` 在刷新时刻
    恒为 none，按旧口径「无旧值算变」等于**根下每个元素每次重生成都排一次房间
    任务**——差异信号被结构性摧毁，只有 TransformOnly 路径的 diff 是真实的。
    树上的条目跨过删行重插存活，才是房间系统上一次真正看到的状态：恰有一条且
    逐位相等 → 没变；没有条目 → 首次见到，回填（管段的一次性补账正靠它）；
    多条 → 历史堆叠残留，重算一次收敛。判据是纯函数 `tree_box_changed`，有单测。
    树同步收成 rs-core-pin 的新原语 `sync_refnos`（一次遍历按 refno 摘旧插新并
    返回旧条目），`update_aabbs` 写反的去重条件与 `replace` 不重置 `ids` 一并修掉
    ——「已知未修、调用点绕过」的旧账就此关闭。
  - **regen 路径的包围盒刷新强制 `replace=true`**（D2 在该路径的真正修复）：
    `process_meshes_update_db_deep` 此前把 `replace_mesh` 配置透传给刷新，默认
    false 时存量行（含管段）整批被 `and aabb=none` 滤掉。mesh 文件按内容寻址，
    replace 与否只该影响文件重写，不该影响包围盒——与 `update_world_transforms`
    强制 true 同一个理由。
  - **落盘容错补齐**（§6 配套修当时只落了两树合一与 query 的 `?`，此处两条
    实为**未落地**，本轮补上）：`serialize_to_bin_file` 改为临时文件 + 原子
    rename——空闲轮反复重写 17MB 文件，原地重写的「写半截崩溃」窗口每轮都开；
    `deserialize_from_bin_file` 内部的 `bincode .unwrap()` 改为 `?`——损坏文件
    此前会在启动路径 panic 成 crash loop（`load_aabb_tree` 的 `unwrap_or_default`
    兜不住 panic）；`load_aabb_tree` 对缺失/损坏打告警而非静默空树。
    ~~**路径带项目名仍未做**（cwd 相对硬编码：换目录静默空树、多项目共用 cwd
    互相覆盖），记为后续项。~~ **已做（2026-08-04）**：rs-core 的读写硬编码裸文件名
    且反向索引重建私有，采用搬运语义——加载前把 `accel_tree_{project}.bin` 放到
    裸名上（只有裸文件则首次迁移），落盘成功后归档回项目名，归档失败上抛由脏位
    驱动重试。已知限制：多项目**并发**共用同一 cwd 时裸文件仍是竞态窗口。
    **（2026-08-06 搬运语义整体退役：文件 IO 收归 gen-model 直接读写项目名文件，
    竞态窗口随裸文件一起消失，见下方增补 4。）**
  - **纯位姿移动不再覆坏管段变换**：`update_world_transforms` 的子树收集此前
    把管段行一并捞进来，用**元素**的世界变换覆盖管段行的「单位圆柱 → 世界管段」
    缩放矩阵，管段会被画成分支原点处的单位圆柱。现按 out 排除
    `inst_info:⟨1⟩/⟨2⟩` 的行。代价：纯 POS/ORI 移动后管段（视觉与房间归属）
    停在旧位置，滞后到该分支下次重生成才追上——管段无独立几何源，    位姿层无从
    重推其变换，这是已知代价而非缺陷（TUBI 语料注释同口径）。
    **（2026-08-05 已关闭，见下方增补：那不是代价，是 issue #5。）**
  - **网格失败不再静默清边**：两条重算分支对 `.mesh` 读失败/三角化失败此前一律
    `continue`，「判不了」被当成「不在里面」，先清后写随即把存量边抹掉且无日志。
    现在：面板（或元素分支的某块候选面板）**一个网格都不可用**升级为错误——任务
    保留重试；部分不可用打告警继续。顺带删掉 `cal_room_refnos` 从未使用的
    `inside_tol` 参数（`lib.rs` 顶上的 `#![allow(warnings)]` 一直压着它）。
  - **刷新集查询去缓存**：`query_deep_visible_inst_refnos` / `query_deep_neg_inst_refnos`
    摘掉 `#[cached]`。两者按**生成根**为键缓存重生成的刷新集，而增量管线的
    `clear_all_caches_batch` 只按「变更元素 + 其属主」失效，够不着这两份快照；
    分支加了新构件之后，同根的下一次重生成会拿着**旧成员表**跑——新构件的 mesh
    不生成、aabb 不落库、房间不触发，无任何报错。真正贵的子树遍历在
    `query_deep_children_refnos` 里另有缓存（在失效列表内），每根多付的几条查询
    相对整根重生成是噪音。顺带把 `GET_SELF_AND_OWNER_TYPE_NAME` 补进
    `clear_all_caches_batch` 的失效列表——它的值含属主类型，OWNER 搬迁后不失效
    会让根解析拿旧属主类型走错 BRAN 成员分支。
    **残余**：`QUERY_DEEP_CHILDREN_REFNOS` 按子树根为键，失效集只到变更元素的
    直接属主——深层后代变更时高层根（ZONE 级正常颗粒根）的子树快照仍会陈旧；
    正确的修法是在 `build_model_update_plan` 算出生成根后按根失效，属计划层
    接线，记为后续项。**（2026-08-09 已接线：`ModelUpdatePlan::regen_root_refnos`
    并入增量失效集（`increment_pipeline.rs` 的 collect → extend 顺序有源码钉
    `cache_invalidation_extends_to_the_plans_regen_roots`）；直写路径在生成前
    失效，暂存路径同一份集合随提交 / 废弃清除。）**
  - 验证：gen-model `cargo test --lib` 266 通过；rs-core-pin `accel_tree` 4 条新
    单测通过；七条 live 夹具用例（含新增的管段用例）在一次性内存实例上逐个实跑
    全部通过。
- 2026-08-05 增补（issue #7 审核后加固：**两条分支对空间树的依赖方向是相反的**）——
  - 这是第 6 条「两树合一」之后才成立、此前没有写下来的不变量：整间分支拿**面板
    自己的**包围盒去树上捞成员，面板在不在树上都算得出来；元素分支只能反过来，
    在树上按 `noun == "PANE"` 找候选。于是存在一种**只打中增量**的坏状态——树里
    一块在册面板都没有（空树、`accel_tree.bin` 来自没生成过结构库的那一次、
    `sync_aabb_tree_with_db` 的数量快路径放行、或 `run_cli` 直入不经过 `run_app`
    那次对账）：启动时的全量重建照样写得出 `room_relate`，而**每一个**元素任务
    都会捞不到候选，把该构件的存量归属边按「不属于任何房间」清掉，且任务算成功、
    队列行删除、日志一行没有。rs-core 的 `load_aabb_tree` 注释记的就是这个模式，
    但消费端此前没有对应的防线。
  - **修法是把这个依赖拆掉，不是给它加一道门禁**：在册面板只有百来块（本项目 124
    间房 / 147 块），一次 `query_insts` 就能整轮复用。新增 `PanelIndex`
    （`room_model.rs`）——一轮 drain 加载一次在册面板的库内几何，元素分支的候选改为
    在这些**库里的**面板包围盒上做相交筛选。相交口径与整间分支逐字一致：rstar 的
    `locate_in_envelope_intersecting` 与 parry 的 `Aabb::intersects` 同样是闭区间。
    于是第 4 条那个「树里得有 PANE」的隐含前提整个消失，空候选集由构造而可信。
    代价是候选筛选从 R 树的 O(log n) 变成面板数的线性扫描，在这个量级上是噪音；
    顺带省掉了原来每个元素任务各发一次的候选面板 `query_insts`。
  - **反方向的同一条纪律**：整间分支仍然用树（少量面板 × 大量构件，这是树的正当
    用途），但它也是先清后写——树空着时每块面板都算出 0 个成员，一次启动就能把整库
    房间归属抹平。`build_room_relations` 因此加了空树拒跑：判不了就不写，上抛交给
    `lib.rs` 那个已有的降级告警，启动不受影响，陈旧的存量边留着比被清成空强。
  - 同轮补掉三处静默：构件查不到实例、世界包围盒不可用各打一行日志（两条路本不该
    走到，元素任务的入队条件就是「包围盒确实变了」）；`enqueue_room_recalc` 在
    `gen_spatial_tree` 关闭且确实有变更时按进程告警一次，否则那条路上队列里从此不会
    出现任何 `room_recalc` 行、房间轮每轮早退、泳道空着，现场只看得到「模型动了、
    房间号不动」；`drain_rooms` 在「在册房间一块可用面板都没有」时说一声。
  - 回退即红的测试：相交口径（含贴面与跨界）、元素分支源码里不许再出现
    `GLOBAL_AABB_TREE` / `load_aabb_tree`、全量重建的空树判定必须排在任何一次重算与
    写入之前、`gen_spatial_tree` 关闭必须留下痕迹。
    `cargo test --lib --features http_api` 343 通过 / 0 失败。
  - **仍未闭合**：issue #7 的根因仍未确定——本轮拆掉的是它最可能的成因并让其余成因
    可见，但没有在报告人的库上复现过。见
    `docs/2026-08-05_issue-7-room-incremental-audit.md`。
- 2026-08-05 增补 2（issue #5：隐含直管段不跟随管件移动，**07-28 记的那条「已知代价」
  其实是缺陷**）——
  - 链路：挪一个管件 → `POS` → `classify_operation_impact` 判 `TransformOnly` → 计划层
    给管件自己排 `Transform` → `update_world_transforms` 的子树收集**显式排除**管段行
    → 管段的 `world_trans` / `aabb` 停在旧值。整条 BRAN 被挪动时同样如此：管件跟着走、
    管段留在原地；房间归属也跟着停在旧位置。
  - 07-28 那次排除本身是对的：管段没有自己的 `pe`，`inst_relate` 行挂在 BRAN/HANG 名下、
    `out` 指向共享单位几何，`world_trans` 是 `insert_tubi` 按成员 arrive/leave 点推导的
    「单位圆柱 → 世界管段」缩放矩阵；拿元素的世界变换覆盖它会把管段画成分支原点处的
    单位圆柱。错的是把剩下那半当成可接受的代价——它只在该分支下次重生成时才自愈，
    也就是说改尺寸会顺带修好，单纯挪位置永远修不好。
  - 修法：**别在位姿层重推管段变换，而是让这类变更别走便宜路径**。计划层新增
    `reroute_derived_geometry_units`——生成根落在 `DERIVED_GEOMETRY_UNIT_NOUNS`
    （`BRAN` / `HANG`）的位姿目标，从 transform 集移进 regen 集，由既有的交付单元
    rollup 排出 `RegenRoot`。与 `is_loop_container_noun` 同一条道理（点容器的 POS 是
    属主网格的输入，所以直接判 `Regen`），区别只在判据落在属主链上、
    `classify_operation_impact` 看不到，只能在计划层做。EQUI/SUPPO 这类没有派生几何的
    单元不受影响，继续走便宜路径。
  - 两处次序是硬约束：改判必须夹在 `partition_operation_impacts` 与
    `mask_details_to_regen` 之间。排在掩码之后的话，改判过去的目标会被掩成非
    `model_affecting`，rollup 看不到它、不建 `RegenRoot`，而它同时已经从 transform 集里
    被摘走——这一次移动会凭空消失。有源码断言钉着。
  - 生成根解析失败保持原判并告警，不掐数据窗口（与房间面板枚举失败同一口径）。
  - 代价：挪一个管件从「一次变换刷新」变成「一条分支重生成」。BRAN 本来就是 ADR-012
    定义的最小交付单元，重生成一条分支正是为这种情况准备的。
  - 测试：单元判据真值表、改判与掩码的次序源码断言；真实会话 live 用例
    `live_projams_real_attribute_sessions_plan_and_execute_distinctly` 里那条 FTUB.POS
    的期望从 `Transform` 改为 `RegenRoot`（不再钉死根 refno，改为断言根的 noun 带隐含
    直管段）。`cargo test --lib --features http_api` 346 通过 / 0 失败。
  - **（2026-08-06 补完：上面只修了一半，见下方增补 2b。）**

- 2026-08-06 增补 2b（issue #5 的**容器侧**：判据没落在属主链之上的那一半）——
  - 增补 2 问的是「**目标自己的生成根**是不是 BRAN/HANG」。挪管件、挪整条分支都命中，
    但挪**分支之上**的任何东西都不命中：`PIPE` / `STRU` 的生成根是它自己（正常颗粒），
    `ZONE` / `SITE` / `WORL` 按契约根本解析不出生成根。这些目标继续走 `Transform`，
    `update_world_transforms` 刷整棵子树却按 `out=inst_info:⟨1⟩/⟨2⟩` 排除管段行——
    容器动了，脚下每条分支的管段全部停在旧位置。与 issue #5 报的是同一个缺陷，只是
    落在层级树的上一层。
  - 这不是假想路径：容器位姿变更排 `Transform` 刷整棵子树是实测行为
    （2026-08-04 AMS 会话 35，ZONE `24384/22400`，子树 67 个模型节点，见
    `docs/2026-08-04_container-transform-cascade-gap.md`）。真库里
    ZONE `24383/66457`（`/1WCC-PIPE-RX`，117 条 PIPE / 187 条 BRAN）下就躺着
    `tubi_relate:[pe:24383_66459, 0]`，`world_trans` 记着世界坐标
    `[-5001.45, 10705.81, 5701.67]`。
  - 修法沿用同一条判据，只是多落一个位置：位姿目标**保留**便宜路径（子树里非管段的
    实例仍靠它刷），同时把子树里每个 `DERIVED_GEOMETRY_UNIT_NOUNS` 单元排进重生成
    （`derived_geometry_units_under` → `append_derived_geometry_units` 并进 rollup 单元表）。
    并进 `units` 而不是直接追加工作项：`units` 同时是 `RegenRoot` 工作项与执行阶段
    生成工作单（`collect_unit_tasks`）的来源，只补一边等于只做一半。
  - 子树遍历不是新增开销：执行阶段的 `update_world_transforms` 对同一批目标走的就是
    同一棵 `collect_pe_subtree_refnos`。扫的是**改判前**的整份目标——自己已经改判的
    目标其子树里再嵌一个派生几何单元的话，重生成外层不会重推内层。
  - **预览同步对齐**：`preview_one_dbnum` 此前只做 `partition_operation_impacts`，
    没有复刻增补 2 的改判——管件移动在预览里显示成便宜路径（执行阶段其实整根重生成），
    容器牵出的那批分支则根本不出现。现在预览逐步复刻执行序列（分区 → 改判 → 掩码 →
    rollup → 并入），有源码断言钉着次序。
  - `DeliveryUnitSummary` 新增 `owner_moved`（`serde(default)`，向后兼容）：这类单元
    变更计数全为 0 但仍 `will_generate`，标志位把「计数是 0 正是它的语义」说清楚。
  - 容器解析不出生成根不再告警：那是设计如此，脚下的管段由子树扫描兜住。只有真正
    断链的目标才值得喊一声。
  - 测试：`pose_target_regenerating_itself…` 真值表（PIPE/STRU/ZONE/SITE/WORL/EQUI/
    NOZZ/SUPPO/断链各一行）、`the_subtree_scan_picks_exactly…`、
    `no_pose_change_anywhere_leaves_implied_tubing_behind`（扫全树，两条判据必须严丝
    合缝）、`derived_units_join_the_worklist_without_shadowing_the_rollup`、两条次序
    源码断言，以及 live 用例
    `live_issue5_moving_a_container_regenerates_the_branches_beneath_it`（真库：挪 PIPE
    得到 `RegenRoot(BRAN) + Transform(PIPE)`；挪 ZONE 得到 >100 条 `RegenRoot`）。
    临时关掉子树扫描复跑该 live 用例，挪 PIPE 只剩一条 `Transform`、零 `RegenRoot`
    ——正是修复前的形态。`cargo test --lib --features http_api` 468 通过 / 0 失败。

- 2026-08-05 增补 3（**「面板没有几何」不再被折成「面板里没有构件」**）——
  - 上一轮把元素分支对空间树的依赖拆掉之后，剩下的同类缺口在**面板自己这一侧**：
    `cal_room_refnos` 在 `query_insts` 返回空时 `return Ok(Default::default())`，
    调用方紧接着先清后写，于是一块在册面板一旦丢掉几何行，它这间房的存量归属边就被
    静默清空。同一个函数对「有实例但网格读不出来」是 `bail!`——同一个决策点，两种
    处置。而全量重建这一侧**连成员变化日志都没有**（增量两条分支反倒都有）。
  - 返回类型改为 `PanelMembers { Computed(..) | NoGeometry }`，把「算出来是空集」与
    「压根算不了」在类型上分开。全量重建跳过 `NoGeometry` 且**不写**，在收尾汇总
    「N / M 块在册面板没有几何，已跳过（未清除存量边）」；增量整间分支上抛保留重试
    ——它的入队条件是这块面板的包围盒刚变过，此刻却没有几何，本身就是信号。
  - 全量重建补收尾汇报：重建前一条 `GROUP BY panel` 查出存量成员数，收尾报出写入边数、
    没有几何的面板数、以及**从非空掉到 0** 的面板（先清后写这条路上唯一会造成数据损失
    的转变，此前从来没人统计）。
  - `PanelIndex` 带出 `missing_panels()`：在册却没能进索引的面板（查不到实例或包围盒
    不可用）。房间轮的覆盖率提示从「一块都没有才出声」改为「缺了几块就报几块」——
    147 块里只有 12 块有几何（本 ADR §9 记的那次实测）此前一声不响。
  - 回退即红：`NoGeometry` 在全量侧必须 `continue` 且排在任何 `save_room_relate` 之前、
    在增量整间分支必须 `bail!` 且排在写回之前；房间轮的覆盖率提示不得退回全 0 判据。
    `cargo test --lib --features http_api` 358 通过。
- 2026-08-06 增补 4（**空间树的管理归位：指针驱动收敛、epoch 校验、文件 IO 收归本仓**）——
  - 背景：树承担三重角色（查询加速、`tree_box_changed` 的变更判定基线、整间分支的
    成员盒来源），它不是纯缓存而是第二真值源。ADR-017/W1 把「提交后收敛」做成
    durable + fail-closed 之后，剩下的结构性负担集中在三处，本轮一并收掉。
  - **提交后收敛改为指针驱动（不再重算几何、不再写库）**：
    `apply_deferred_spatial_mutations` 的 refresh 分支此前复跑
    `update_inst_relate_aabbs_by_refnos(.., true)`——对主库把几何 AABB 整个重算并
    重写一遍，可窗口 journal 重放刚把同样的值落成主库真值。现改为
    `sync_tree_from_committed_pointers`：按 refno 分块读回已提交的
    `(refno, noun, aabb.d)`（口径与刷新层进树一致：`world_trans.d != none and
    aabb.d != none`）→ `sync_refnos` 进树。收敛跑在「失败即停止出队」的关键路径
    上（I7），现在是纯读 + 树同步 + 落盘，时长与失败面都显著缩小。房间触发不受
    影响——AABB 房间目标在窗口内并入 finalize plan 随尾事务持久化（W3.3），收敛
    只负责树本身。
  - **epoch 校验取代条数对账，指针重建取代全量重算**：库侧新增单例记录
    `spatial_epoch:current`，每条携带空间意图的尾事务顺带 `+1`
    （`render_spatial_epoch_bump`，与意图、水位同一事务；无空间意图的提交不
    bump）。树文件旁新增 sidecar `accel_tree_{project}.meta.json`（epoch、条数、
    落盘时间；epoch 在写文件**之前**读，方向偏保守）。启动改走
    `load_project_tree_verified`：sidecar epoch 与库相等才信文件，否则
    `rebuild_tree_from_pointers` 分页读指针 bulk-load 整树并立即落盘盖章——只读
    不写，取代旧兜底 `manual_update_aabbs(true)`（全库重算几何并回写整个
    `inst_relate.aabb` 列，又慢又重）。条数对账 `sync_aabb_tree_with_db` 与
    `manual_update_aabbs` 都退役为手工诊断/修复工具（指针重建覆盖不了「指针本身
    缺失/陈旧」的补账场景，那仍是重算路径的正当用途）。窗口重试导致的多次 bump
    无害：epoch 只比相等不表达次数，至多多做一次指针重建。
  - **文件 IO 收归 gen-model，裸名搬运退役**：`AccelerationTree` 本身
    `Serialize + Deserialize`，反向索引等派生字段 `#[serde(skip)]`，反序列化后由
    `ensure_refno_index` 在首次按 refno 操作时自愈（本仓有单测钉住这条假设）。
    gen-model 直接原子读写 `accel_tree_{project}.bin`（tmp + rename），
    `stage_project_aabb_tree_file` / `archive_project_aabb_tree_file` 与 rs-core 的
    `load_aabb_tree` / `serialize_to_bin_file` 一并退出生产路径；多项目并发共用
    cwd 的裸文件竞态随之消失。
  - **残余（自觉记录）**：直写紧急路径（`GEN_MODEL_DIRECT_INCREMENT=1`）不产生
    空间意图也不递增 epoch，崩溃丢掉的内存树变更 sidecar 认不出来——该环境变量
    存在时启动无条件走指针重建；另备 `AIOS_FORCE_SPATIAL_REBUILD=1` 作运维强制
    重建开关。树内 `mesh_cache` 不随 regen 失效的问题只影响交互拾取，本轮未动。
    **（后续演进：直写事务已随增量加固补上 epoch bump；启动判据的摇摆与定案见
    2026-08-11 增补。）**
  - 回退即红的测试：收敛路径不得出现 `update_inst_relate_aabbs_by_refnos`、指针
    重建只读不写、启动信任判据必须是 epoch 相等且失配收敛到指针重建、epoch bump
    与意图同事务先于水位、无空间意图不 bump、反序列化后索引自愈（防同 refno 堆叠）。
- 2026-08-11 增补（**启动信任判据定案：双字段指纹 + 意图重放优先 + 兜底指针重建**，
  方案与决策记录见 `docs/2026-08-11_spatial-tree-startup-init-plan.md`）——
  - 背景：v0.1.7（提交 `cd3ea9e9`）曾把启动改成「无条件复用树文件、仅显式
    `AIOS_FORCE_SPATIAL_REBUILD` 重建」，动机是旧 epoch 校验在「崩溃后带着待重放
    空间意图启动」时必然失配、每次都触发全量指针重建，而其实意图重放就能便宜自愈。
    但一并丢掉的是三道在编的防线：静默陈旧树 + 启动全量房间重建（= 历史「重启回退
    room_relate」的复发向量）、直写崩溃检测、文件缺失/损坏自愈。
  - 定案为分层判据（`load_project_tree_verified`）：内存树非空保持不动 → 强制
    重建环境变量（改真值解析）→ 文件缺失/损坏**自动**指针重建 → 指纹
    `(epoch 值, 库侧该 epoch 的 updated_at)` 双字段与库相等才直接复用；失配但
    `has_pending_spatial_work` 为真 → 复用文件交给意图重放自愈（不再重建，消除
    v0.1.7 要解的那个痛点）；失配且无意图 → 只读指针重建（直写崩溃 / 换文件 /
    回滚库）。完备性依据：`reconcile_spatial_pending` 是「树同步 → 落盘 → 才销账」，
    「文件 + 待重放意图」对暂存路径无遗漏。
  - 指纹从单一 epoch 数值扩成双字段（评审反馈：**要与数据库对时间戳**）：时间戳与
    计数由 `render_spatial_epoch_bump` 同一事务写入、同源于库端时钟，库快照回滚
    恰好撞回同一计数的边界也认得出来。sidecar `TreeFileMeta` 新增
    `db_epoch_updated_at`（serde 缺省空串，旧版文件自动按失配走一次自愈补齐）。
  - 决策（决策卡确认）：D1 文件缺失/损坏自动重建；D2 库侧诊断查询失败降级复用
    文件 + 告警；D3 两处启动调用点统一「告警降级空树、不阻断启动」；D4 不加
    「回到盲信」逃生舱。
  - 修复 `AIOS_FORCE_SPATIAL_REBUILD` 的 `is_ok()` 判定（部署模板写 `=0` 想关闭，
    实际每次启动都强制全量重建）：与 `GEN_MODEL_DIRECT_INCREMENT` 的 P2-1 同款
    三态真值解析，收口在 `batch_worker::parse_explicit_flag`。
  - 可观测性：/health 新增 `spatial_tree`（文件/库两侧指纹现读现比、drift、
    entries、本次启动裁决 reused / healed_by_replay / rebuilt / empty /
    preloaded / reused_degraded）。
  - 回退即红的测试：快路径必须比双字段指纹、失配必须先问意图重放且意图查询先于
    裁决、文件缺失必须自动重建、强制重建不得回到 `is_ok`、默认路径仍禁条数对账与
    几何重算重写、盖章指纹必须在写文件之前读、判据真值表（含快照回滚撞计数与
    旧版 sidecar 两个边界）、旧格式 sidecar 解析缺省空串。

- 2026-08-12 增补（**直写路径的空间树变更补上 epoch 痕迹**，方案
  `docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`）——
  - 背景：2026-08-11 的分层判据押在一条前提上——「凡是改变了『树应有内容』的已提交
    变更，都会在库侧留下可检测的痕迹」。该方案 §3 的论证原文是「直写路径不产生意图，
    其崩溃丢失自然落入 3c 被重建接住」，但这句话**只对会 bump epoch 的 durable 直写
    分支成立**。盘点全仓 `GLOBAL_AABB_TREE` 的写变更点，另有两处既不写意图行、
    也不 bump：普通直写刷新（`occ_generate.rs`，全量生成与 `manual_update_aabbs`
    走这里）与直写删除清理（`helper.rs` 的 `delete_room_membership` 窗口外分支）。
    共同失效形态是「变更已提交 → 任务标记完成（不再重放）→ 内存树已同步、仅剩脏
    标记 → 空闲轮落盘前崩溃 → 重启时指纹相等 → Reuse 复用陈旧文件」，而
    `/health` 的 `drift` 恒为 false，无人可见。删除路径尤其重：被删构件的旧包围盒
    留在树上，启动全量房间重建按旧位置把它重新收编进 `room_relate`——D4 修掉的缺陷
    借崩溃复活，且 `DeleteCleanup` 任务已 done，没有任何重放会再清一次。
  - **不变量（本增补钉死）**：直写路径动树之前，先让库里「说得出树该变」——变更与
    epoch bump 同事务提交，事务成功后才推进内存树；崩溃恢复统一交给启动判据
    （失配 + 无意图 → 指针重建）。`spatial_epoch:current` 的语义因此从「携带空间
    意图的尾事务顺带 +1」扩成「一切改变树应有内容的已提交变更都 +1」。
  - 恢复通道按决策 D1 取 **bump-only（3c 重建兜底）**，不给直写路径发明
    `spatial_reconcile` 意图行：后者恢复更便宜，但需要为非窗口来源造意图行 id
    （现有 id `spatial_reconcile_{dbnum}_{end_sesno}` 是窗口专属）并把 (dbnum, sesno)
    一路下传到 `helper.rs`；而 2026-08-11 的 D 系列已裁决「直写崩溃 → 重建接住」，
    重建只读、分页、量级已实测，崩溃窗口本身是小概率事件。
  - 落地：直写事务门控从 `durable_room_trigger && !chunk_changes.is_empty()` 放宽为
    `!chunk_changes.is_empty()`，`durable_room_trigger` 从此只决定「要不要随事务发布
    房间任务」；删除清理按块走「锁下探测 → 边删除与 bump 同事务 → 摘树 → 标脏」，
    探测放在锁下使「要不要 bump」与「树到底动没动」由同一快照裁决；普通直写分支补
    写锁跨 [判定 → 事务 → 同步]（决策 D2 取窄跨度，读输入段的几何 join 仍在锁外）。
  - **顺带关闭一个方案没盘到的交错窗口**：普通直写分支此前只在 `sync_refnos` 那一
    瞬取锁，并发的删除清理挤在「读输入 → 同步」之间时，刚被摘掉的条目会被这里同步
    回树上，成为要等下次指针重建才自愈的幽灵条目。锁跨度扩到判定之前后不再成立。
  - 已知代价：全量生成按块 bump（决策 D3；「整次只 bump 一次」经论证不可行——空闲轮
    可能在 bump 后落盘一次，使后续块的变更重新回到无痕迹状态）。连带效果是这些路径
    跑过之后 `room_build:main` 凭据判为「空间状态已变」，下次启动会照跑一次全量房间
    重建——方向正确（此前它对同数漂移是瞎的），但会多花一次重建时间。
  - 回退即红的测试：直写事务/bump 不得再由 `durable_room_trigger` 门控且唯一允许
    不 bump 的是「逐位相等」那一支；普通直写分支的锁必须先于变更判定、事务、同步；
    删除事务渲染必须把两个方向的边删除与 bump 包进同一事务；删除路径的探测必须在
    锁下、bump 必须先于 `remove_by_refnos`、暂存分支一条 bump 都不许有。

- 2026-09-02 增补（**e3d-model 接管生成后触发链断裂，房间副作用并回 e3d 发布事务**；
  ADR-056 / ADR-057 D2 下的房间面）——
  - 背景：第 4 条「AABB 真的变了」的实现一直住在旧生成器的 AABB 刷新里
    （`aabb_refresh.rs::update_inst_relate_aabbs_by_refnos_mode` → `render_room_recalc_upserts`，
    ADR-040 §1/§3 的保守口径与同事务纪律也在那里）。`RegenRoot` 的执行端换成
    `ModelRefreshPolicy::generate_roots` → `E3dModelService` 之后，e3d 发布事务直写 `aabb` 行、
    bump spatial epoch、同步空间树，却**从不排 `RoomRecalc*`**，也不清被移除几何的
    `room_relate` / `room_panel_relate`（`append_geometry_representation_cleanup` 只删
    `geo_relate` / `inst_info` / `inst_relate` / `aabb` / `trans`）。于是稳态下房间几乎只剩启动
    全量重建一条路，而被 e3d 移除的元素（尤其是 PANE）留下悬空边，`fn::room_relate_of`
    照样把它取出来（`helper.rs` 早已写明的缺陷形态）。
  - 修法（`src/fast_model/room_publication.rs`，gen-model 侧——ADR-057 D2「房间/空间树归
    gen-model」）：`room_publication_effects(upserts, removals, pre_e3d_sources)` 把一次发布折成
    两笔，`render_room_publication_effects` 渲染进**同一个发布事务**（ADR-040 §3）：
    ① **重算**——upsert 的每个 `GeometryId::Element` 来源按 noun 分流 `RoomRecalcPanel`（PANE）/
    `RoomRecalcElement`，AABB 变没变都排（ADR-040 §1 定向保守口径）；隐式管身不排（房间系统
    不读 `tubi_relate`，对容器排元素任务只会把容器的存量入边清成空集）。② **清边**——被移除的
    `Element` 几何、以及 pre-e3d 清理删掉行却没拿到新几何的旧来源，两个方向一并删
    （`render_room_membership_delete`，每 300 个来源一条）；清边**不看** `room_incremental`
    （删除例外，与 `delete_inst_relate_subtree` 同口径）。挂点：`generate_refs`（每根，
    `generate_roots` 定向 → 重算+清边；`generate_dbnum` 全库 → 只清边，第 4 条收窄 1 不变）与
    `apply_geometry_delta`（窗口 / 删根路径）。
  - 回退即红：`room_publication::tests` 6 条——分流/去重/管身跳过、移除与孤儿旧来源清边而
    重写来源不清、开关与全库策略只关重算不关清边、300 分块、`mem://` 真引擎门
    `deleting_a_pane_leaves_no_dangling_room_edges`（删一块 PANE：出边 / `room_panel_relate`
    入边 / 被移除成员入边全清，未碰的面板与成员原样，重写的成员排进 `room_recalc_element`
    行，库里没有 pe 记录的来源清边不报错）、源码钉 `both_publication_paths_carry_the_room_effects`
    （两个入口都在 `prepare_geometry_delta` 之前折算、`publication_transaction` 之前渲染）。
  - **房间拓扑的文件侧替身（同日 R3）**：`src/fast_model/room_topology.rs`——
    `collect_room_groups(roots, hierarchy, lookup)` 是与生成根枚举同形状的纯遍历（只认
    `SubtreeElement{noun,name,members}`，每元素恰一次 lookup，成环/重复/跨库成员不挂死、
    读不到整体报错），hd 口径 `FRMW` → 子 + **任何中间 noun 下的**孙 `PANE`、hh 口径 `SBFR` →
    直接子 `PANE`，逐字对齐 `load_room_panel_groups` 那两条 SQL；`room_panel_groups` 做关键字
    过滤（**空关键字不匹配**，与触发侧同口径）与房间号（NAME 按 `-` 切的最后一段），产物直接
    喂既有的 `room_panel_map_from_groups`（命名校验、`all_panels` 排除集语义不变）。
    `load_room_panel_map_from_files` 遍历 `E3dModelService::design_sources()`（每个设计库在
    其生成会话上 `scan_index` + `build_set`），原始分组按 `(dbnum, sesno, 层级)` 进程内缓存。
    `room_model::load_room_panel_groups_by_mode` 按 `direct_read_mode()` 路由，
    `load_room_panel_map`（增量两条分支、`RegenRoot` 缺陷修复）与 `build_room_panels_relate_common`
    （启动全量重建）都走它——direct 模式下房间子系统从「0 间房、静默盖章」变成能算。
    回退即红：`room_topology::tests` 4 条纯测 + 源码钉 `room_model_routes_both_topology_entries_by_read_mode`
    + 真文件门 `live_ams8000_room_groups_are_structurally_sound`（ignored）。
    **DB 读模式不变**：仍读 noun 表 + `pe_owner`；零解析库在 DB 模式下仍是 0 间房（合并两源要
    先定去重口径，另议）。`panels_under_rooms`（规划器结构触发）仍读 `pe`，随 P2-1 尾巴换源。
  - **仍未闭合（同日审核 F3 / F5 / F6，见 ADR-056 计划 P2-7 追记）**：第二轮逐点兜底读
    `inst_geo.pts`，e3d 发布不写 `pts`，跨界构件对 e3d 几何一律判不在（要么改读 `.mesh`
    顶点，要么发布补 `pts`）；scoped 房间 drain 随暂存路径拆掉，只剩空闲轮；
    启动重建凭据以 spatial epoch 对账，每个 e3d 根发布都 bump，重启必全量。

日期：2026-07-27（2026-07-28 两轮增补，2026-08-05 三轮增补，2026-08-06 一轮增补，
2026-08-11 一轮增补，2026-08-12 一轮增补，2026-09-02 一轮增补）
关联：`docs/2026-07-27_room-incremental-audit-report.md`（缺陷取证 D1–D7）；
`docs/2026-07-27_room-incremental-implementation-report.md`（变更清单、验证证据、残留风险）；
`src/fast_model/room_model.rs`；`src/data_interface/model_update_pending.rs`；
`src/data_interface/increment_manager.rs`（`update_world_transforms`）；
`rs-core-pin/src/accel_tree/acceleration_tree.rs`；`rs-core-pin/src/room/`；
ADR-009（增量影响判定口径）

## 背景

增量更新链路已经覆盖了「解析 → 落库 → 水位 → 模型重生成」，但**房间归属
（`room_relate`）完全不在其中**：`build_room_relations` 只在 `run_cli` 进入
`async_watch` 之前跑一次全量，且那次全量只增不删。约 20 个材料表 surql 经
`fn::room_code` / `fn::get_room_number` 读这些边，房间数据陈旧会直接、且只会，
体现在材料表的房间号列上。

房间计算依赖的 AABB 空间树也只带上了一半，且当前配置下不生效。取证见审计报告的
D1–D7，其中两条是本 ADR 的硬前置：

- **D1**：`TransformOnly` 路径只写 `world_trans`，不重算 `aabb`、不碰树。纯 `POS`/`ORI`
  移动——正是「设备从 A 房挪到 B 房」——因此完全丢失。
- **D2**：`replace_mesh = false` 使包围盒 SQL 追加 `and aabb=none`，存量元素永不刷新。

D1 与 D2 相互独立，修好任何一个另一个依然成立。

## 决策

### 1. 一致性目标：最终一致

房间重算作为一种新的 `ModelWorkAction` 接入 `model_update_pending`，复用既有的持久化
队列、重试、`MAX_ATTEMPTS` 死信与批量语义。**不进水位事务**：重算需要读 mesh 做点包含
判断，会把水位事务拖到秒级以上；而且在几何重生成之前算出来的结果本身就是错的。

代价明确记录在案：材料表在收敛窗口内可能读到旧房间号。

### 2. 粒度：混合

- 元素几何变更 → **反向定位**，只处理该元素的归属；
- PANE / 房间节点自身变更 → **整间重算**（面板一动，整间成员全变，元素级无法表达）。

对应两个 action：`RoomRecalcElement` 与 `RoomRecalcPanel`。

### 3. 判定语义：抽共享谓词，只保留一份

新增 `element_in_panel(panel_mesh, element_aabb, element_pts) -> bool`，封装现有正向口径：

> AABB 8 顶点全部在内 → 是；部分在内 → 取实际几何点（`where !booled`），任一点在内 → 是；
> 一个顶点都不在内 → 否。

`cal_room_refnos` 改为循环调用它，反向路径也只调它。
`query_room_panel_by_point` 降级为只服务交互式拾取，**退出增量链路**。

理由：正向（8 顶点 + 逐点兜底、`ORIENTED | MERGE_DUPLICATE_VERTICES`）与反向
（单点、只有 `ORIENTED`、命中即返回）本来就是两套规则，会对同一个 (元素, 面板) 组合
给出不同答案。共享谓词让不一致在结构上不可能发生。

### 4. 触发源：AABB 真的变了

在重算包围盒那一步比对新旧值，**变了才入队**；同时给 `TransformOnly` 路径补上这一步
（修 D1 的同一处改动）。

选它而不是复用 `OperationImpact != Skip`：不依赖属性分类调得准不准（`Unknown` 保守触发
会拉进大量实际没动的变更），PANE 自身移动会自然触发（再按 noun 分发到整间分支），
纯数据变更天然不触发。

两个例外单独处理：

- **删除**：没有新 AABB。由 `DeleteCleanup` 路径直接删除指向该元素的所有 `room_relate` 边，
  不入房间队列。
- **形状变了但包围盒恰好不变**：几何重生成成功但 AABB 未变的元素，仍按元素分支入队一次。

### 5. 多房间归属：保留多边 + 确定性排序

数据模型保持多边不变（一个件横跨两间房本来就会有两条边）。在边上增加两个字段：

- `inside_count: int` —— 元素 AABB 落在该 panel 内的顶点数（0–8）；
- `center_dist: float` —— 元素 AABB 中心到 panel world AABB 中心的欧氏距离。

两者在共享谓词里本来就要算，零额外几何开销。

`fn::room_code` / `fn::get_room_number` 改为
`ORDER BY inside_count DESC, center_dist ASC, room_num ASC LIMIT 1`，
第三项保证全序，杜绝平局时的不确定性。

改动收敛在 5 处 surql 定义（`fn_query_room_code.surql`、其 `_hh` 变体、三份重复的
`common.surql`），20 多个材料表 surql 一行不用改。

### 6. 空间树：两棵合并成一棵

删除 `GLOBAL_ROOM_AABB_TREE` 与 SQL 已经写坏的 `load_room_aabb_tree`（D5）。
反向候选改为在 `GLOBAL_AABB_TREE` 上按 `noun == 'PANE'` 过滤——`RStarBoundingBox`
本来就带 `noun` 字段，`QueryRay` 已在用 `filter_nouns` 做同类过滤。

只有一处增量维护需要写对，也就消除了两棵树互相漂移的可能。

配套修 D3 / D4 / 落盘容错：

- `update_aabbs` 的 remove 改为**按 refno 删**，不再按整值相等删；
- 新增 `remove_by_refnos`，供 `DeleteCleanup` 路径调用；
- `deserialize_from_bin_file` 的 `.unwrap()` 改为降级重建，缺文件要告警而非静默空树；
- 落盘路径带上项目名，不再是硬编码相对路径。

### 7. 队列接入细节

- **阶段**：房间任务是 `drain` 的**第三阶段**，排在 `regen_root` 之后。现有两阶段
  （`drain_non_regen` → regen）的顺序是因为 `cascade_expand` 会反过来入队 regen；
  房间任务依赖「几何与 AABB 都已落定」，只能更靠后。
- **去重**：必须像 `joins_regen_batch` 那样**按 target 在内存去重**，但不套用它的
  批量执行逻辑。
- **`record_id` 去掉 dbnum**：房间任务用 `room_recalc_panel_{target}` /
  `room_recalc_element_{target}`，`dbnum` 与 `source_end_sesno` 仅作追踪；来源未知时写 0；
  UPSERT 递增队列 revision，不比较跨库 sesno。
  理由：一个 panel 天然跨库，带 dbnum 会让同一 panel 在一轮里出多行（同一间房重算多遍），
  失败后又只能被同 dbnum 的新会话复活——那是审计里 **B6** 的放大版。
  代价：`record_id()` 需要按 action 分支，B5 的复活子句顺序守护
  （`revival_clauses_run_before_the_watermark_field_they_read`）要跟着适配。

### 8. 陈旧边的删除：先清后写，不做差分

两条路径都是「先 DELETE 再 RELATE」，整体包在一个事务里
（沿用 `increment_pipeline.rs:90` 的 `wrap_in_transaction`）：

- 整间分支：删该 panel 名下**所有** `room_relate` 出边，再整批写回；
- 元素分支：删指向该元素的**所有** `room_relate` 入边，再写回本次算出的边。

先清后写天然幂等、可重放；差分更新要先读旧集合再比对，多一次往返，并发下更难保证。

**冲突规则**：同一轮 drain 内，若某 panel 已进入整间分支，落在该 panel 内的元素任务
被吸收跳过——两条路径的删除范围不同（一个按 panel 出边，一个按元素入边），
同时跑会互相踩。
（2026-07-28 修正：吸收另需**封闭性检查**成立——构件现存入边面板与其当前树候选
面板都落在本轮已重算面板集合内，否则元素分支照跑。只看「已写过该构件」在同轮
跨面板搬家时会留下未重算面板的陈旧边，见落地进度。）

### 9. 验收口径：与全量重建逐边对拍

唯一硬标准是「增量收敛结果 == 全量重建结果」：同一份数据上先跟增量收敛，
再跑一遍（幂等化后的）`build_room_relations`，逐边比较 `room_relate` 集合。
可自动化、不依赖截图，且天然把「共享谓词两边一致」一并测了。

前提是先造一个**不依赖 E3D 授权**的最小合成库，沿用现有 live 测试的
`4000000001/…` 保留 refno 段惯例：

- 1 个房间节点（`project_hd` 下为 `FRMW`），NAME 需同时满足 `room_keyword`（当前为 `-R-`）
  与 `match_room_name_hd` 的 `^[A-Z]\d{3}$`；
- 2 个 PANE 面板，各带一个盒状 mesh，世界位置相邻并留一段重叠区；
- 5 个构件：2 个完全在 A 内、2 个完全在 B 内、1 个横跨 A/B 边界（覆盖多归属与排序）；
- 每个构件都要有真实的 `inst_relate` / `inst_info` / `geo_relate` / `inst_geo` 与 `pts`，
  否则第二轮点检查取不到东西，等于没测。

对拍脚本：全量建基线 → 移动 1 个构件跨房 → 增量收敛 → 再全量 → 比对边集合。

**夹具已落地并跑通**（`src/fast_model/room_fixture.rs`）：全量侧
（`live_room_fixture_parity`）的 6 条边全部命中，两轮判定都覆盖到了——完全在内的走
AABB 八顶点，跨界的落第二轮逐点兜底且被两室同时收录。增量侧
（`live_room_incremental_parity`）在同一套夹具上跑「搬家 → 增量 → 再全量 → 逐边比较」，
两侧一致。

两个踩过的坑记在这里：`pe.owner` 与 `inst_relate.generic` 是 `GeomInstQuery` 的
非 Option 字段，缺了 `query_insts` 会反序列化失败、而 `cal_room_refnos` 把它
`unwrap_or_default()` 吞掉，整间房静悄悄算成 0；另外本仓的 SurrealDB 客户端是 fork 版，
官方 `surreal` 二进制握不上手，一次性实例需自行构建 fork 版服务端。

## 结果 / 约束

- 房间号在收敛窗口内可能滞后。这是第 1 条决策的自觉代价，不是缺陷。
- D1 的修复落地时发现它与 D3 在实践上是耦合的：给 `TransformOnly` 补刷新必须用
  `replace_exist = true`，而那恰好是 `update_aabbs` 的反向条件产生重复条目的场景。
  当前的处理是在 `update_inst_relate_aabbs_by_refnos` 内先按**旧值**把 R 树旧条目删掉
  再调 `update_aabbs`（`AccelerationTree.tree` 是 `pub`，无需改 `rs-core-pin`）。
  `update_aabbs` 本身的缺陷仍在，只是在这个调用点被绕过了；它当前没有别的调用方。
- ~~落盘尚未处理~~ **已决（2026-07-28，见落地进度）**：落盘时机放在队列层——空间树
  带脏标记，`batch_worker` 空闲轮收尾统一落盘。复审时确认此前缺口的实际后果比
  「收敛保证会漂」更重：重启加载旧 bin 后 `sync_aabb_tree_with_db` 只对账**数量**
  （搬动不改条数，快路径直接放行），随后 `run_cli` 无条件的 `build_room_relations`
  全量重建会用**树上的旧位置**把重启前已收敛的 `room_relate` 边改写回搬家前的状态
  ——是主动回退而非缓慢漂移。仍不在 `update_world_transforms` 里落盘，理由不变
  （`execute_item` 逐 refno 调用会变成序列化风暴）。
- 本 ADR 的前置是 D1–D5 的修复。其中 D1（`TransformOnly` 补 AABB 刷新）与
  D3/D4（树的删除语义）跨 `gen-model` 与 `rs-core-pin` 两个仓，
  而两边都有大量未提交改动（复核时 gen-model 217 / rs-core-pin 5），
  动手前需先确认其他写入方的状态。
- 验收的前置不是「库里得有几何」——实测（审计报告 §4.1）表明工作库 8009 上几何是齐的
  （906 个 `inst_relate.aabb`、584 个带 `pts` 的 `inst_geo`、733 个 `.mesh`）。
  真正卡住验收的是另外两条，都得先解决：
  1. ~~**D8**——`load_aabb_tree` 的库侧 bulk-load 被注释掉了，只反序列化
     `accel_tree.bin`；而 `manual_update_aabbs` 只在树为空时才触发，文件里只要有
     几条就永远不会重建。~~ **已修并实跑验证**：树从 45 条填到 403 条，
     二次运行不再重建（§4.2）。403 就是当前几何数据能支撑的上限。
  2. **面板没有几何**——本项目库其实有一套完整合规的房间：124 个 FRMW 命中生效关键字
     `-RM`、末段全部满足 `^[A-Z]\d{3}$`（形如 `/1RX-RM03-R301`）、且全部挂着 PANE，
     层级 `FRMW → CWALL/CFLOOR → PANE` 也正落在现有查询的子 + 孙两层覆盖内。
     卡点是 `inst_relate WHERE in.noun = 'PANE'` 为 **0**——只生成了管路库 7997，
     结构库从未参与生成，于是 `cal_room_refnos` 在 `query_insts` 那一步就空手而归（§4.3）。
     补生成结构库之后，**真实数据上的对拍验收是可行的**；第 9 条的合成夹具因此是
     「不依赖生成流程、跑得快、可进 CI」的常备手段，而非唯一出路。
  3. 另有 **D10**（已修 2026-07-28）：`DbOption` 的字段名是 `room_key_word`，而旧 toml
     写的是 `room_keyword`，键名对不上导致该配置从未生效，实际一直用默认值 `-RM`
     （本项目上恰好是对的，但这是巧合）。现已补 `#[serde(alias = "room_keyword")]`，
     两种键名均生效。
- 第 5 条给 `room_relate` 加了两个字段，旧数据没有。全量重建一次即可补齐；
  在补齐之前 `ORDER BY` 会退化为按 `room_num` 排序，仍然是确定的。
- **D12（已知缺口，2026-07-28 记载）→ 已实现（2026-08-04）**：非几何的房间结构
  变更此前没有任何触发器。房间节点改名（FRMW/SBFR 的 NAME 变更）与 PANE 挂靠层级
  变化（OWNER 变更）都不改变任何 AABB → 第 4 条的触发源不点火 → `room_relate.room_num`
  与 `room_panel_relate` 保持陈旧，直到下次启动的全量重建；20+ 材料表 surql 经
  `fn::room_code` 直接读 room_num，陈旧期间房间号列错误。
  草图中的两条规则已按原样落进 `build_model_update_plan`
  （`collect_room_structural_triggers` + `panels_under_rooms`，见
  `2026-08-04_room-incremental-audit-and-gap-closure.md`）：
  ① FRMW/SBFR 的 NAME 变更且**新旧任一**名字命中 `room_key_word`（改进房间与改出
  房间都要重算；名称正则的合规校验仍归重算路径，计划层不重复）→ 名下全部 PANE
  （子 + 孙两层）入队 `RoomRecalcPanel`；关键字未配置时不触发。
  ② PANE 的 OWNER 变更（搬迁语义，ADR-009 口径，复用 `owner_change`）→ 为该 PANE
  自身入队 `RoomRecalcPanel`（新旧两个属主对应的房间都会经该面板的整间分支收敛）。
  ③ `project_hd` 的 CWALL/CFLOOR OWNER 变更 → 按固定两层拓扑枚举其直接 PANE 子元素。
  面板枚举失败降级为告警不掐数据窗口，并随水位原子标记 `room_build:main` 需要全量
  重建；该标记只由成功的启动全量重建清除。
  live 验证：`live_room_structural_triggers_enqueue_panel_recalc`（一次性内存实例
  实跑，含真库子 + 孙面板查询），夹具 pe 行随之补齐 `name` 字段（计划层 OWNER 图
  加载的非 Option 坑，与 `pe.owner` / `generic` 同构）。

## 2026-08-12 增补（二）：空间一致性闭环——V2 单文件快照、状态机与空间串行锁

方案 `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`（吸收同日
epoch trace 方案，决策 D1–D8）。要点与对本 ADR 既有决策的影响：

- **快照介质换代**：`accel_tree_{project}.bin` + `.meta.json` 双文件退役，改为
  `accel_tree_{project}.snapshot` **单文件 V2**（树载荷 bincode 段 + SHA-256 自校验
  + project/namespace 身份 + 双字段指纹），tmp+sync+rename 原子发布。读侧全套校验
  （反序列化/版本/身份/哈希/条目数）任何一环失败一律指针重建，**不回落旧格式**。
  旧文件在首次 V2 发布成功后删除（D3）：旧二进制对 bin 缺失是无条件重建，回退自动
  安全；留着旧文件反而给「回退 + 恰有 pending → HealByReplay 复用冻结文件」开静默
  陈旧窗口。
- **进程态状态机**（`spatial_state.rs`）：Uninitialized/Loading/Ready/ReadyEmpty/
  ReplayRequired/Rebuilding/DegradedReuse/DegradedBlocked。空间消费者（启动全量
  房间重建、RoomRecalc 消费、空闲房间轮）仅在 Ready/ReadyEmpty 放行，错误码
  `SPATIAL_TREE_NOT_READY`，durable 行保留待重试；解析/生成/durable 重放/指针重建/
  `model.spatial.bounds` 直查不受门禁。覆盖率闸门（第 9 条验收口径的运行时前哨）
  第一道换成状态门，`ReadyEmpty`（已验证空库）不再被 `>0` 判据误报。
- **启动判据修正**（取代 2026-08-11 分层判据的两处边界）：pending 优先仅在快照可读
  且校验通过时成立，快照不可用一律重建（完备集论证只对可读快照成立）；进入
  ReplayRequired 时**立即**重放一次，不等 worker 派发门（queue_paused 部署下 Ready
  否则永远不来）；「内存树非空即 preloaded」的盲信短路删除，收窄为显式夹具标记。
- **空间串行锁** `SPATIAL_STATE_SERIAL`（锁序
  `STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE`）：staged 提交后
  收敛、direct 指针事务→树同步、重建换树/发布、快照落盘、Python `spatial.*` 全部
  纳入同一串行线——顺带修掉 Python reconcile/persist 与 worker 并发动树的既有竞态。
  journal 写回与窗口尾事务**不**持此锁（尾事务不动树，崩溃安全靠 pending 行）。
- **指针重建协议**：LIMIT/START 分页退役，改 record-range 分页（页间无漏无重由
  fork 兼容套件双跑钉住）；口径 current-only（排除版本化数组 id 行与 `in.deleted`
  软删行，Rust 侧排除 NaN/Inf/反向 AABB）；分页读在锁外，stamp 前后比对 + 换树 +
  发布在锁内，三连漂移/查询失败进 DegradedBlocked；房间覆盖率分母同口径。
- **降级自愈**：后台 revalidator 只管 DegradedReuse（重跑启动装载）与
  DegradedBlocked（重试重建），30s 指数退避至 5min，恢复 Ready 唤醒调度器。
- **可观测性契约迁移**：/health `spatial_tree` 九键作废，换十五键
  （state/ready/startup_verdict/format_version/entries/usable_pointer_rows/
  invalid_pointer_rows/pending/file_epoch/db_epoch/drift/snapshot_sha256/
  last_verified_at/last_rebuild_attempts/last_error）；`startup_verdict` 枚举改为
  reused/replayed/rebuilt/migrated/degraded/preloaded。
- **既知边界**：快照 `tree_sha256` 只护单文件完整性；跨进程「同一集合」对拍不能
  比载荷字节（`AccelerationTree` 序列化含 HashMap 段，迭代序随每进程 SipHash 种子
  变化），走 entries/usable 口径与逐边对拍（沙箱实测见
  `docs/2026-08-12_spatial-tree-consistency-acceptance.md`）。
