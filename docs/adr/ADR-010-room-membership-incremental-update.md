# ADR-010：房间归属增量更新——AABB 差异驱动 + 混合粒度 + 共享判定谓词

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
  2. **偏离本 ADR §4 的第二个例外**：「形状变了但包围盒恰好不变」的元素**不**入队。
     原文要求几何重生成成功但 AABB 未变的元素仍入队一次，但那等价于「把每个重生成过
     的元素都入队」——一个 BRAN 重生成会连带它全部构件，其中绝大多数根本没动。
     真正要区分的是「实体变了而包围盒没变」，这需要比几何哈希，而刷新包围盒这一层
     手里没有。残留风险很窄：仅当一个**跨面板边界**的构件内部几何改变、且包围盒逐位
     不变时，它的第二轮逐点判定结果才可能变而无人重算。要补的话，正确的位置是几何
     写入层带出一个「实体确实变了」的信号，而不是在这里放宽成全量入队。
- 排序落地时查出 **D11**：hd / hh 两份 surql 无条件按文件名顺序加载，`_hh` 永远覆盖
  `_hd`，与 Rust 侧编译的 `project_hd` 错位。本轮两份都改了排序，但门控未修。
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
    后续项。
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
    **路径带项目名仍未做**（cwd 相对硬编码：换目录静默空树、多项目共用 cwd
    互相覆盖），记为后续项。
  - **纯位姿移动不再覆坏管段变换**：`update_world_transforms` 的子树收集此前
    把管段行一并捞进来，用**元素**的世界变换覆盖管段行的「单位圆柱 → 世界管段」
    缩放矩阵，管段会被画成分支原点处的单位圆柱。现按 out 排除
    `inst_info:⟨1⟩/⟨2⟩` 的行。代价：纯 POS/ORI 移动后管段（视觉与房间归属）
    停在旧位置，滞后到该分支下次重生成才追上——管段无独立几何源，位姿层无从
    重推其变换，这是已知代价而非缺陷（TUBI 语料注释同口径）。
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
    接线，记为后续项。
  - 验证：gen-model `cargo test --lib` 266 通过；rs-core-pin `accel_tree` 4 条新
    单测通过；七条 live 夹具用例（含新增的管段用例）在一次性内存实例上逐个实跑
    全部通过。

日期：2026-07-27（2026-07-28 两轮增补）
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
- **D12（已知缺口，2026-07-28 记载，本轮不实现）**：非几何的房间结构变更没有任何
  触发器。房间节点改名（FRMW/SBFR 的 NAME 变更）与 PANE 挂靠层级变化（OWNER 变更）
  都不改变任何 AABB → 第 4 条的触发源不点火 → `room_relate.room_num` 与
  `room_panel_relate` 保持陈旧，直到下次启动的全量重建。第 4 条原有的两个例外
  （删除、形状变而包围盒不变）都没覆盖这一类。20+ 材料表 surql 经 `fn::room_code`
  直接读 room_num，陈旧期间房间号列错误；当前以「启动时全量重建会自愈」为兜底，
  属最终一致的已知代价。
  触发设计草图（后续轮次实现）：在 `build_model_update_plan` 追加两条规则——
  ① 房间类 noun（命中 `room_key_word` 关键字 + 名称正则的 FRMW/SBFR）的 NAME 属性
  变更 → 为其名下全部 PANE 入队 `RoomRecalcPanel`。需要在计划层引入「房间→面板」
  查询（目前那一层不碰房间概念），这是本项未随手实现的原因；
  ② PANE 的 OWNER 变更（搬迁语义，ADR-009 口径）→ 为该 PANE 自身入队
  `RoomRecalcPanel`（新旧两个属主对应的房间都会经该面板的整间分支收敛）。
