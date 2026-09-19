# ADR-073：房间随根生成、不再是一条流程；任何未完成写入一律整库重初始化

状态：**已接受，2026-09-11 修订房间总开关语义，2026-09-16 修订房间输入不全的处置（降级不判死）**（2026-09-10 用户拍板：「房间只保留几何与筛选算法，流程整套删除」（共识 **d-757**）；「任何未完成的写入
重启后一律按整个 dbnum 清理并重新初始化，不再按 sesno 回退」（共识 **d-747**，三条口径）。本文由协同组「房间」t-803 起草，
是 t-735 文档清点里拆出的「刀 1」——后续 ADR / CONTEXT / 计划改写都以本文为尺子。措辞待指挥官 / 用户过目；
**号到提交时复核**——ADR-060 曾撞号两次。）**实施中**：worktree `gen-model-refactor`，分支 `refactor/retire-publish-stage`，
协同组「房间」S1–S4 工单（S1 删房间流程整文件、S2 数据面与控制面退场、S3 外围收口、S4 空目录从零跑通 ams 1112）。
日期：2026-09-10
勘察与清点：R0 勘察（t-703，恢复改整库重初始化的落点与清理清单）；文档清点 t-735（哪些段落随本 ADR 作废，含待拍表）。
关联：
- **取代** ADR-072 D2（回退到水位）与 D3（崩溃不分面）的恢复动作——恢复只剩「整库重初始化」一种（D3）。
- **修订** ADR-071 D1 / D3 / D4 / D5 / D8 / 不变量 S3——三枚水位退成两枚、房间边随模型块落、崩溃语义改重初始化（D1 / D2 / D3）。
- **修订** ADR-065 D1 / D8——房间不再是任务末的一段，而是每个根生成的一部分（D1）；G2 (i)「房间失败 = 根不安定」保留并收进 D2。
- **作废** ADR-067 全篇（常驻房间模型：换代、参照、启动对比、生命周期、结构触发目标集）——其 D2「在册面板几何驻留内存」
  是否作为算法输入形态保留，见 §未决 U3。
- **作废** ADR-040 全篇（房间失效正确性闭合：保守失效集、`RoomRecalc*`、CWALL/CFLOOR 结构枚举、`room_build:main` 降级标志）。
- **不动** ADR-021（回退默认整库重建——本 ADR 把它扩成**唯一**恢复路）、ADR-056 D1（正常的模型失败不阻断数据水位）、ADR-064
  （元素批写法）、ADR-057（e3d-model 纯函数）、ADR-060 D1–D9（模型水位是被写下的承诺；D10 只退 `room_sesno` 半句，见 D7）、
  ADR-066 D5 的原则（位姿前移的构件与面板照算房间、房间成功是该根成功的前置）。
- **待拍** ADR-010（房间归属增量更新）——整条退休还是拆出算法部分，见 §未决 U2。
- 证据：`artifacts/room-increment-latest-20260910/`（7997 移动 @163 / 恢复 @165：只生成 2 根，任务末整库房间段仍花 9.1 / 20.2 s，
  每轮重发 59,691 条归属）；d-429 的可行性依据（`MemoryRoomCalculator::calculate_with_points` 逐记录独立、无跨记录聚合）；
  R0 勘察实位（开工前写入标记检查在 `batch_worker.rs` 的 `rollback_if_writing` 那条路；`fast_delete::rollback_to_watermark`
  的 wipe 分支提前 return，镜像作废 / 空间树置脏 / 普查作废三句在整库路上一句没跑到）。

## 2026-09-11 修订：kv-mem 房间启动屏障与完整根 ACK

用户批准决策 **d-381**：`room_membership=true` 的 spawned/embedded kv-mem 每次冷启动都是空结果库，
必须依次完成 resident 输入、MDB 全部可读 DESI 的模型根、逐根房间归属与 process-store 发布，
全部成功后才允许 `initialization.model_ready=true` / `features.room.status=available`。范围不受
`watch_dbnums` 限定；`startup_autorun=false` 也不跳过。失败时服务保留 health/诊断能力，但普通
模型完整产出入口返回 `room_bootstrap_in_progress|room_bootstrap_unavailable`，不能制造“几何完成、
房间未计算”的成功根。

这要求把两个概念拆开：

- `ModelReadRoute` 只回答从投影还是数据库读取；
- `ModelPublicationPolicy` 回答写到哪里。ingest 初始化未收口时仍是 `ProjectionOnly`；read-through
  kv-mem 是 `ProcessStore{durable:false}`，复用 D1 的逐根模型行 + 两类房间边 + `gen_root` 同事务写法。

`RootReceipt` 继续表示候选几何。process-store 事务成功后，进程内另记
`RootPublicationAck{source_sesno,room_generation,medium,...}`；ready 根在房间开启时必须同时匹配
冻结源与 resident generation。几何重生成、删除、源换代与 resident 换代会使旧 ACK 失效。
ACK 不持久、不参与 ingest 水位判决，进程退出即消失，因此它不是第三枚房间水位。

`POST /dbnums/{dbnum}/model/rebuild` 按模型路由分派：Database 保持 durable worker；Memory 复用同一个
整库内存执行器，不再向 read-through 中没有消费者的 durable queue 排永久 queued 任务。
`e3d.room.lookup` 保留版本三元组语义（read-through 仍为 `null/null/unsupported`），另加
`calculation_status` 区分 `ready_empty` 与 `not_computed/initializing/failed`。

## 2026-09-11 修订：房间总开关不阻断模型

用户进一步拍板：`room_membership=false` 的含义是**只关闭房间归属计算，不影响模型更新**。本节在
开关关闭时取代 D1 / D2 与 W2 / W3 中“房间是模型完成前置”的部分；开关开启时原规则不变。

- `frozen_room_calculator` 在开关关闭时返回 `Ok(None)`，不是错误。生成根照常持久化、凭证照常安定，
  `model_sesno` 照常前移；根事务不写新的 `room_relate` / `room_panel_relate`，模型对账删除掉的来源旧边仍清。
- `model_sesno` 的承诺随开关显式分档：开时包含房间归属，关时只包含模型几何。库上不恢复第三枚
  `room_sesno`；`/health.room_membership=false` 是“当前一致性不含房间”的必备伴随信号。
- 关闭期间既不会产生失败根，也不会留下模型水位缺口，因此重新打开后**不能**靠启动模型覆盖复核自动发现
  历史房间欠账。重新打开只影响之后重生成的根；要补齐关闭期间缺失或陈旧的房间边，运维显式执行该库
  `POST /api/v1/dbnums/{dbnum}/model/rebuild`。
- 这是有意接受的降级：关闭时材料表可能读到旧房间号或无房间号，但模型几何与模型水位继续工作。启动横幅
  与 `/health` 必须把这一点说清楚，不得把 `model_sesno == applied_sesno` 单独表述成“房间也一致”。
- `e3d.room.lookup` 在关闭时仍返回存量边供诊断，但增加 `room_membership=false`，并返回
  `publication_status="disabled"`、`published_version=null`、`stale=null`；此分档修订 D4
  “直接用 model_sesno 表达房间版本”的旧约束。

## 2026-09-16 修订：房间输入不全 = 降级，不判死；durable 侧补一格房间就绪承诺（D14）

起因：0.1.25 部署实例（2026-09-15）把 `room_membership` 开着，MDB 缺 6003 SCBDESI 的文件 → 常驻房间模型建不起来
→ 每个任务 `frozen_room_calculator()` Err → 1112 `18/18`、7997 `6772/6772` 全部根「room calculator unavailable」，
`model_sesno` 永不前进、现场无从发现。当时只把出厂模板改回 `false`，房间门政策留待本节
（`docs/plans/2026-09-15-mdb-project-closure-and-room-default-dev-plan.md` §8 Q1）。用户 2026-09-16 拍板：先把「能开起来不炸」做出来。

源码里把「一处输入缺失」放大成「全库瘫痪」的有两道闸，都不是 D2 说的逐根：

- **启动·整份**：`room_topology::latest_frozen_sources` → `mdb_membership::require_design_files()`，MDB 任一 DESI 没文件 →
  整份 Err → resident `Blocked` → 之后每个任务开工冻不出计算器。
- **开工·整份**：`MemoryRoomCalculator::new` → `PanelIndex::ensure_complete()`，在册面板任一块没几何 → Err。它与 2026-09-09
  审核 W1 的修法（从未生成的面板不阻代）自相矛盾：`warm_meshes` 放行、`new()` 又拦。此外 `warm_meshes` 对「有几何行却世界
  包围盒不可用」与「`.mesh` 读不出」仍整份 Err，`preload_panel_projection` 一块面板生成失败整份 Err——同一形状。

D2 的论证原文是「不整枚扣住 `model_sesno`：一次持久的房间失败会把整库模型永远钉在旧版重算」，这两道闸的实际效果正是整枚扣住。
本节把它们改成**降级**：

- **D8　缺 DESI 文件 → 按有的文件建。** 常驻模型只用搜索根里找得到的 DESI；缺的库记 `missing_dbnums`，生命周期进
  `Degraded { problems }`。缺失库里的房间与面板不在册：别的库的构件对这些房间的归属算不出、也不写；缺失库自己没有文件也就
  没有任务，不产生冲突。文件到位后重启即补全（启动重新枚举 MDB）。
- **D9　在册面板无可用几何 → 不进索引、旧边受保护、不阻断。** 四种来源同一处置：从未生成、启动预生成失败、有几何行但世界
  包围盒不可用（原 `unusable_panels`）、`.mesh` 读不出 / 三角化失败（原 mesh-warm 阻代）。面板不进 `PanelIndex`；
  `render_root_room_edges` 把它们当 `protected_panels`——每根一句 `LET $room_protected_panels = [...]`，每个构件的
  `DELETE {element}<-room_relate` 带 `WHERE in NOT IN $room_protected_panels`，指向它们的存量边本轮不删不写：陈旧可恢复、
  抹平不可（`render_element_relate_statements` 第三参本来就是为此设计的，逐根路径此前传空）。留痕：`missing_panels` /
  `missing_panels_by_dbnum` / `problems`。这些面板自己的 `room_panel_relate` 那一半照常（拓扑不依赖几何）。
- **D10　`Degraded` 放行计算器。** `ensure_ready` 只拦 `Loading` / `Blocked`；`Degraded` = 模型在、带着已点名的缺口，
  `frozen_room_calculator` 照常冻计算器。`Blocked` 仍是「房间体制本身没起来」（MDB 声明读不了、E3D 服务起不来、预热 worker 崩），
  维持 D2：本批根按「房间算不出来」记失败、不排队。
- **D11　承诺与仪表。** `Degraded` 下 `model_sesno == applied_sesno` 的含义是「模型几何一致；在册且有几何的面板的房间归属一致；
  `problems` 点名的面板 / 库不在承诺内」。`/health.features.room.status` 出 `degraded` 并带 `problems`；
  `/health.room_build.resident` 出 `missing_dbnums`；启动横幅一句。读者判「房间也一致」必须并读这一格（与 2026-09-11 那节
  `room_membership=false` 的伴随信号同一条纪律）。
- **D12　常驻模型在每个任务开工前对齐到此刻的文件版本；代号 = 计算输入的摘要。** 此前常驻模型启动建一次、进程活多久用多久
  （换代点 `ensure_current` 自 ADR-067 作废后没有生产调用方）：新增的 PANE、搬走的面板、前进了的 DESI 都进不了它，逐根算出的
  归属按启动那一刻的面板集算，且不报错。现在每个 dbnum 任务的模型半边开工前（批次执行体、ModelOnly 执行体——启动追平
  `model_sesno → 文件最新` 与 `model/rebuild` 都走它，2026-09-17 审核补上、初始化双 lane、读透侧 ensure 准入）
  经 `model_db_adapter::refresh_room_reference_before_task` 探一次：`room_topology::latest_frozen_sources` 只走 MDB 成员清单 +
  每个 DESI 文件的最新会话 + 冻结令牌（**不装 schema、不起 E3D 服务**），composite 与装好的那份相同 = 常路、零重建；变了才按文件
  重建（`room-groups-v2` 磁盘工件 / GroupCache 按文件令牌、`room-input-cache` 面板几何按文件的 `initialization_target_hash`，
  没变的文件只付重开校验的钱），在 `lock_spatial_serial` 里整份替换。已开工的任务手里的 `Arc` 不受影响——D1「任务期间不换代」
  说的是每个任务自己那份引用；并发开工共用一把重建锁，后到的等完再探一次。重建失败**不**让任务失败：留旧代继续算、`Degraded`
  记一条 `stage=refresh`；启动时 `Blocked` 而这次成了就是自愈。原因持续时不每个任务都整份重建一遍（2026-09-17 审核 P3-6）：
  同一文件版本 composite 下连续失败按指数退避（`room_resident::RebuildBackoff`，30s 起翻倍、10min 封顶），冷却期内的开工回
  `Deferred`、按上一次的结论继续（生命周期与 `last_failure` 不再动）；composite 变了 = 输入变了，立刻再试；建成了清账。**不是
  永久跳过**——暂时性故障冷却期一到照样自愈；启动装载器失败不记这本账，进程起来后第一个任务总要亲自试一次。
  **一任务一台**（2026-09-17 B/T1）：对齐之后执行体紧接着冻**一次**（`model_db_adapter::TaskRoomCalculator::freeze()`，三态
  `Disabled` / `Frozen(Arc)` / `Unavailable(原因)`），显式传给单元工作单 → 批量重生成与逐根重试 → 生成 → bake；此前是 write-behind
  每次 flush 各冻一台，任务中途常驻换代（`/model/ensure` 并发对齐）会让同一任务各批根的边不是同一代算的（P2-3 的补丁从此只是
  富余安全网）。`Unavailable` 时单元工作单入口就把全部单元按 D2 记失败（`room calculator unavailable for this task`，与此前 flush
  时同一句），不白跑几何。flush 侧 `frozen_room_calculator()` 只剩「没带边的根」的兜底；没有任务语境的 process-store 发布仍按组冻一次。
  会话口径是文件最新（ADR-054 Q1），不是本任务的 T——排队期间
  文件又前进了的话常驻比本窗几何新一版，下一窗追上即合一（ADR-067 D3 当年接受的同一口径）。
  **代号**（ACK 里的 `room_generation`、`required_room_generation`、生命周期的 `generation`）改为 `ResidentRoomModel::input_version`
  ——拓扑 + 在册视图 + 有几何面板的几何的摘要，与 `MemoryRoomCalculator::resident_generation` 同一个字串；文件 composite 只当
  「要不要重建」的探针键（`manifest.key`）。理由：拿文件 composite 当代号，任一 DESI 前进就把全 MDB 的根 ACK 作废成
  `not_computed`，而绝大多数前进不动任何房间 / 面板。房间 / 面板真变了 → 代号变 → 旧 ACK 作废，这是 d-381 早已接受的语义
  （「resident 换代会使旧 ACK 失效」）；受影响成员的边由 §未决 U1 的传播（`RoomOnly`）补，不在本条。
- **D13　面板自身的变化 → 本库整库房间边重算（`RoomOnly`，落地 d-819，收 §未决 U1）。** 逐根路径只算「本根在哪些面板里」；
  面板那一侧变了（PANE 的几何 / 属主、FRMW / SBFR 改名、`project_hd` 下 CWALL / CFLOOR 搬迁），它名下**别的根**的成员边与
  `room_num` 一个都点不着。两条触发、同一种处置：① **规划期**——窗口规划器看本窗变更集（e3d-model `ChangeLedger`），碰到房间
  定义族 `PANE / FRMW / SBFR / CWALL / CFLOOR` 的任一条目（增 / 删 / 改属主 / 改成员 / 改属性都算，`Unresolved` 不算；**只按 noun
  判，不解析 `detail` 散文**——DESC 之类只改元数据的面板改动多触发一次整库重算，接受）→ 给本库排一条 `ModelWorkAction::RoomOnly`
  工作项（`target_refno = dbnum=<n>`，一窗一条，随 `side_actions` 交给任务，不入队）；② **执行期**——开工前对齐（D12）换了房间
  代号（别的库的面板 / 房间变了、进了常驻），本库的成员边同样可能陈旧；③ **电平型再武装**（2026-09-17 审核 P1-2 补）——开工前对齐
  **之后**读库上 D14 那一格 `model_room_generation`，与此刻常驻代号不等（含库上没有那一格）= 这个库欠一次整库重算
  （`room_recompute::durable_alignment_owed` → `library_alignment_owed`）。①② 是边沿型：原因只活在本任务里，拆窗（大窗口的常态）、
  单元没生成、`settlement_failed`、刷盘没齐、重算写库失败都会把它丢掉，而下一窗的账本不再含那条 PANE 变更、再探又是 `Unchanged`
  ——边永久陈旧到下一次不相关的 PANE 变化；③ 按库上的事实判，直到某个任务把 `Aligned` 盖上才安静，重启后仍记得（代价：从没盖过
  那一格的库第一个任务多跑一遍 O(投影) 重算）。常驻说不出代号（开关关 / 还在装 / `Blocked`）没有可对齐的目标，③ 不触发；水位行
  读不出也不触发（读错不是「欠着」的证据）。处置：任务在本批根**刷盘之后**（durable ACK 齐了；本任务的根都已落库，同库别的根
  ——`/model/ensure` 按需生成的——可能仍在 write-behind 队列里，两边对同一构件都是先删后插、同一台计算器下幂等）、**收尾之前**，
  收尾时新冻一台计算器（此刻这一代；开工时对齐过、任务期间又换了代就用新的——比开工那台更新，D1「任务期间不换代」说的是每个根
  自己那份引用），对本库**投影里**全部有回执的根按**现有几何**重算归属、重写两张边
  （`model_db_adapter::recompute_dbnum_room_edges`：与逐根路径同一套渲染 → 「整库重算 == 从零初始化」逐边相等的硬验收不多一条
  口径），不重生几何、不动模型行、不动空间树，提交全过后把这些根 ACK 的 `room_generation` 对齐到本台计算器。算不出的根按 D2
  记失败（账本 `failed`、单元清单补一条 `Failed`）、任务 `Partial`、`model_sesno` 不盖；刷盘没齐（那批根已判死）就不跑。
  **覆盖面要自证，不在投影里的根回读库上几何**（2026-09-17，U1 第二半）：投影只装本进程生成过的根——初始化的模型行不进投影
  （ADR-077 D5）、落盘形态冷重启后投影是空的——所以「本库全部有回执的根」≠「本库全部根」。处置分两步：**先**拿 `DbSet@T` 的
  根清单（`window_root_plan::enumerate_target_roots`，规划器同一条枚举）交给 `recompute_dbnum_room_edges`，清单里投影没有回执的根
  逐根经 `model_record_reader::durable_records_of_root` 读回库上的 `inst_relate` / `tubi_relate` 行（有世界盒与世界变换的），折成
  `room_model::durable_room_inputs`（一行一条或多条实例，「网格 → 世界」= 世界变换 × 实例本地变换）、走**同一个**
  `calculate_for_root` / `render_root_room_edges`（算法只认 `RoomInput` 这几样：构件、世界盒、网格、变换——两种来源同一形状的语句）；
  读回一行几何都没有的根（从没生成过 / 没有几何）算覆盖到、什么都不写；这些根没有进程内 ACK，就绪由 D14 那一格作答。**再**拿回执里
  的 `covered_roots`（投影里的 + 回读的）对一遍清单：每一根都在 = `WholeLibrary`，D14 那一格可盖；清单枚举不出来 = 覆盖面未知、
  `PartialCoverage`——只算了投影那一半，触到的根边已更新、`model_sesno` 照盖（本任务的根都成了），但 D14 那一格**不盖**（替没算过的
  根说话正是 D14 要挡的事），③ 在下一个任务再点（枚举失败多是暂时的）；点名了却没覆盖到本不该出现（回读按清单逐根走），出现了也
  如实不盖。回读一根失败（库读不出）与写库失败同一处置：`Failed`、水位不盖。此前为「同一代下反复点、每次都覆盖不全」设的进程内去重
  （`settle_partial_coverage` / `PARTIAL_COVERAGE_SETTLED`）随之退役：落盘形态下 ③ 在每个库上多跑一次就把那一格盖上、自己安静。
  代价：O(本库根数) 次按 `anc` / `in` 索引的库读（每根两条 SELECT），只在真触发时付。仍是内部意图：不入队、没有 HTTP 口（§不做）。
  **没收的一半**：跨库成员（面板在 A、成员在 B）只在 A 变了之后**第一个开工**的库上被 ② 带到，其余库要等自己下一次碰到触发
  （③ 把这一条也接住了：A 变了 → 常驻换代 → 每个库下一个任务都欠）——U3 的跨库那一问仍开着。
  **2026-09-17 追记（D，反向传播）：整库降为兜底，常路是「面板变了 → 拿它的新旧盒问谁在里面 → 只重算命中的根」。** 用户拍板
  （grill 两轮，五题全按推荐）：拆三类 / 按根 / 全局树 + 不可用退整库 / 先同库 / 定向成功即盖 `Aligned`。落法：
  ① **面板变化结构化**——`ResidentRooms::install()` 对着被换下的那一代算 `PanelDelta { removed, renumbered, recalc }`（两代同时在手的
  唯一时刻）：前两格来自 `TopologyDiff`，`recalc` = 拓扑差点到的 ∪ **两边都在册但几何变了的**（`ResidentPanel.world_aabb` /
  实例集按位比——PANE 只动几何时拓扑差是空的，而它名下别的根正是这时候陈旧），每块带旧盒 / 新盒（旧盒管「离开的」、新盒管
  「进来的」）；随 `RoomReferenceRefresh::Rebuilt { panels, previous_generation }` 交给执行体。② **准入**（`room_recompute::scope_for_task`，
  纯函数）：本任务真换了代 + 带着非空面板变化 + 说得出上一代 + 库上 D14 那一格**正是上一代**——库上的边全部对齐到上一代、中间没欠
  别的，那么上一代 → 这一代之间面板动了什么就是欠账的全部，定向；缺任一条（库上更早 / 从没盖过、`Unchanged` 却欠着——重启后 /
  别的库先换的代、面板变化为空、第一代、重建失败）整库。③ **反查**（`data_interface::room_reverse`）：命中源三路——全局空间树
  `GLOBAL_AABB_TREE::locate_intersecting_bounds`（真实元素、全库；消费者门 `ensure_spatial_ready`，不就绪退整库）、投影里本库的隐式
  管身记录按盒扫、库上 `tubi_relate` 的 `aabb.d` 范围过滤（**管身不进树**——`projected_tree_entry` 对 `tube.is_some()` 回 `None`，
  只查树会漏直管穿房的 BRAN，这是拆单时多出来的 Q6，按默认「库上范围查询」办；慢了再给 `tubi_relate` 补 `aabb_d` 平副本）；
  `noun == PANE` 剔掉；元素 → 根 = `anc` ∪ 自身 ∩ 本库 `DbSet@T` 根清单（嵌套根两根都算，多算一根是幂等重写）；折不出根的元素
  如实进 `unresolved` → 反查不完整 → 退整库；跨库命中只数不算（U3 a 档，b 档「给别的库排根级工作项」等数据再定）。
  ④ **处置**：`removed` 清它全部出边（`{panel}->room_relate`）与 `room_panel_relate` 入边、`renumbered` 改列
  （`render_renumber_statements`）——不查空间、不算几何，渲成面板级语句随逐根块同一串事务（起初作 prelude 进第一笔；2026-09-19 F1 起
  作 epilogue 进最后一笔，F2 起末尾再补 `recalc` 面板自己的 `room_panel_relate`）；`recalc` 命中的根经
  `recompute_dbnum_room_edges(…, RootScope::Targeted { roots, panel_statements })`：投影里的按集合筛、其余回读库，其余一字不改
  （同一个判定入口、同一套渲染、同一种装块）。⑤ **盖章**：命中根全算齐 = `RoomRecomputeVerdict::Targeted(重算那台计算器的代号)`
  → `LibraryRoomClaim::Recomputed` → D14 那一格盖 `Aligned`（不盖的话下一个任务 ③ 看到库上 ≠ 常驻照样整库扫、定向白做）；任一根
  算不出 / 反查读库失败 / 写库失败 = `Failed`，不盖、③ 再武装、下一个任务整库（M1 之后账本里那一段还在，下一个任务沿账本**重走同一段**
  仍是定向——F1 2026-09-19 为此把面板级删挪到最后一笔、停放改在反查完成就做，重走才完整）；树不就绪 / 根清单枚举不出来 / 有折不出根的 / 覆盖面
  对不上 = 退整库（多算不是漏算）。**不变的**：③ 电平型触发原样（重启后 / 从没盖过那一格只整库一次，盖上就安静）；整库那一路
  `run_room_only_recompute` 一字不改，是兜底；D1 钉「元素分支不依赖空间树」照旧只钉正向，反查是另一个入口。**代价**：一窗内两个
  库都有任务时，只有先开工的那个库走定向（它的库上代号是上一代），后开工的库看到库上 ≠ 常驻 且 `Unchanged` → 整库——U3 b 档才收。
  **2026-09-17 晚追记（U3 b，跨库停放）：上面那条代价收了——反查命中的别的库不再只数，停放成根级工作项由那个库的下一个任务消费。**
  用户拍板「停放槽先进程内，按 U3-1→U3-6 一项一笔」。落法：① **反查不分库**——库上 `tubi_relate` 范围查询去掉 `dbnum =`、投影管身
  一把读锁扫全 MDB（`ModelMemoryStore::for_each_record`，不复制），命中按 `RefNo::dbno()` 分桶：本库的折根，别的库的进
  `AffectedRoots::foreign = dbnum → (元素 → 祖先链)` **存元素不折根**（根清单在那个库的 `DbSet@T` 上）；祖先链在这里就带上——
  管身的来源是 BRAN 那种容器，`inst_relate` 里没有它的行，那个库回库取不到（拆单时按「B 的任务补 `anc`」写的，落地时改成停放方带上）。
  ② **停放槽**（`room_reverse::park / pending / clear`，进程内 `RwLock<BTreeMap<dbnum, Vec<RoomRoots>>>`）：A 库的任务**结论定为
  `Targeted` 之后**才把 `foreign` 的每一桶连同这一段换代（`from` = 它开工前对齐的上一代，`to` = 它重算那台计算器的代号）停进那个
  库的槽（`park_foreign_hits`）；同 `from → to` 并成一条（面板挪走又挪回得到同一对代号）、空条不停；退整库 / `Failed` 不停放。
  `/health.room_build.parked` 一格 `{dbnum: {entries, elements, oldest_age_ms, segments}}`。③ **准入链**（`scope_for_task` 从
  「四条同时成立」改成链式）：从库上 D14 那一格出发，每一步接一段——本任务自己的换代（`previous_generation → 常驻代号`，面板变化
  非空）或停放物的 `from → to`——接到此刻常驻代号就定向（`RoomRecomputeScope::Targeted { swap: Option<PanelSwap>, parked }`）；
  自己那一段优先（新鲜的反查）、库上已越过的一段不要、链断 / 绕圈 / 从没盖过 / 常驻说不出代号 → 整库；今天的四条准入是链长 1
  的特例。两个执行体在读过库上那一格之后 `parked_for_task` 取槽：库上欠着才取，库上已与常驻同代顺手清槽（停放物说的都是越过
  的代）。④ **消费**：`run_targeted_room_recompute` 把停放的元素沿带来的祖先链与本库根清单求交（`fold_parked`，不回库；同一元素几段
  都有只折一次、祖先链取最长的），与自己那一段反查命中的根并起来 `RootScope::Targeted`；本任务没换代就只有停放那一路、没有面板级
  语句（停放方已按面板写过——面板级语句天然跨库）；折不出根的元素退整库。`run_room_recompute` 按结论清槽：`WholeLibrary` /
  `Targeted` 才清（说得了「整库对齐到这一代」），`Failed` / `PartialCoverage` / `Disabled` 留给 ③。**不变的**：③ 电平型触发原样——
  停放槽进程内、重启即丢，丢了是**多算不漏算**（③ 兜底整库）；要不要落库等 live 看到重启后整库的频率再定。**仍欠**：没命中的库
  （C）看不到 g1 → g2 那一段的「无事发生」，`Unchanged` 且槽空 → 仍整库；给全 MDB 的库停放空段要知道库的全集（常驻的
  `expected_dbnums` 有），本轮没做。
  **2026-09-19 追记（H1，定向路一任务一台）：定向重算用的是本任务开工冻的那台计算器，准入链的终点、边、盖的代号三者
  同一代。** 审核（`output/show-me-room-increment.html` §06 H1）读出一条漏算：scope 按开工时读到的常驻代号定，收尾却
  `frozen_room_calculator()` **新冻一台**「此刻这一代」——P2-3 自己点名的那条竞态（一个 DESI 前进 + 用户同时点「显示模型」，
  `/model/ensure` 并发对齐在任务中途把常驻 g2 → g3、面板 Q 又动了）下：反查只用 P（g1 → g2）的盒、命中根按 g3 算、
  `Targeted(g3)` 盖 D14 ← g3，Q 命中的根从此永久陈旧，③ 看到库上 g3 == 常驻 g3 就哑了；`park_foreign_hits(g1 → g3)` 还把
  同一个洞停放给别的库。整库那一路没事（每一根都算，更新的计算器也诚实）。落法：① `RoomReferenceRefresh::Rebuilt` 多一格
  `generation`（装上那一代的 `input_version`，这一段换代的 `to`）；② 「此刻常驻代号」只从冻好的那台取——
  `TaskRoomCalculator::generation()`，两个执行体 `durable_alignment(job.dbnum, rooms.generation())`，不再另读一次生命周期
  （对齐、冻、读数是三次独立的读，ensure 可以插在任意两次之间）；③ `scope_for_task` 的准入链必须**恰好**接到 `resident`
  ——自己那一段 `g1 → g2` 接不到冻的 g3 就整库（定向只有一台计算器，链上各段的边都得是它这一代算的），除非账本里有 g2 → g3
  那一段可以续（见下条 M1）；`PanelSwap` 带 `to_generation`，停放给别的库的那一段以它收尾；④ 冻的那台随
  `DataBatchTaskResult::room_calculator` 交给收尾段，`run_room_recompute(job, &request, task_rooms.as_ref(), …)`，定向那一路
  `run_targeted_room_recompute(…, expected_generation = scope.targeted_generation(), rooms, …)` **不新冻**，入口先对
  `expected_generation == rooms.resident_generation()`，不等 / 没冻出（`None`）退整库；整库那一路 `run_room_only_recompute`
  一字不改、照旧收尾新冻。**不变的**：三条触发、③ 电平型原样；中途换代的那一段（g2 → g3）由下一个任务的 ③ 接住——库上盖的
  是 g2，常驻 g3，欠着；账本 / 槽里有 g2 → g3 就定向，否则整库（多算不漏算）。
  **2026-09-19 追记（M1，换代段账本）：面板变化不再只活在做了重建的那一次调用里——谁重建谁记，任何任务的准入链都能接。**
  审核（同页 §06 M1）：`PanelDelta` 只随 `RoomReferenceRefresh::Rebuilt` 回给做了重建的那一次 `refresh_before_task`；
  `/model/ensure`（UI「显示模型」）先探到 composite 变了、做了重建 → 拿到那一段的是它，随后每个库的任务都是 `Unchanged` +
  库上欠着 + 槽空 → 链断 → **全 MDB 每个库各整库一次**；截断窗口（大窗口的常态）跑不到收尾，自己那一段随任务丢掉，下一窗
  又是 `Unchanged` → 整库；没被命中的库看不到「这一段与你无关」→ 整库（上条「仍欠」）。N 个 DESI 库一次面板变化最坏 N 次整库，
  定向只在「批次任务自己重建 + 一窗追平 + 被命中」三件同时成立时省钱。落法：① **账本**——`room_resident::SwapLedger`
  （`ResidentRooms` 上的一个字段，与两代模型同一把锁）：`ResidentRooms::install()` 对着被换下的那一代算出**非空** `PanelDelta`
  就记一段 `PanelSwap { from = 旧代 input_version, to = 新代 input_version, panels }`——`refresh_before_task`、初始化 lane、
  `/model/ensure` 都经 `install`，谁重建谁记；空段不记（换代另有原因，这一段说不了成员边，准入照旧整库）、同 `from → to`
  替换旧的、容量 64 满了丢最早的；进程内、重启即丢（③ 兜底）。`PanelSwap` 随之搬到 `room_resident`（`room_recompute` 只
  `pub(crate) use`）。② **准入改成路径搜索**——`scope_for_task(library, resident, swaps, parked)` 不再收 `refresh`：从库上那一代
  出发，沿账本段 / 停放段深度优先找到 `resident`（本任务冻的计算器代号）的第一条路（`walk_chain`），同一起点**停放段优先**
  （元素是别的库反查时已找出来的、免一次空间查询；同两代的账本段是同一问题的另一种问法，答案等价）、一条路断了换另一条、
  绕圈靠已访问集挡；一条都接不上 = 整库。结果 `RoomRecomputeScope::Targeted { swaps: Vec<PanelSwap>（链序）, parked, generation }`
  ——链上可以有**多段**换代段（对齐 g1 → g2 + ensure g2 → g3 + 冻 g3 = 两段都反查、按 g3 算，H1 那条竞态从「整库」升成「定向」）。
  ③ **消费**——`run_targeted_room_recompute(…, swaps, parked, expected_generation, rooms, …)`：每一段各 `affected_roots` 一次、
  根并起来，面板级语句按链序拼成 prelude（改号是逐段接力的，A→B 再 B→C）；结论 `Targeted` 之后**按段**停放
  （`park_foreign_hits(dbnum, &swap.from, &swap.to, foreign_of_that_swap)`，别的库从它库上那一代起才接得上）。两个执行体在
  `parked_for_task` 之后 `swaps_for_task(alignment.as_ref())` 取账本（库上欠着才取），传进 `request_for_task(…, &swaps, &parked)`；
  原因多一句「沿换代段账本接了 N 段：…」。`/health.room_build.resident.swap_ledger = { entries, capacity, oldest_age_ms, segments }`。
  **收掉的**：上条「仍欠」（没命中的库 C）——C 的任务沿账本接段、反查一次得到空命中就 `Targeted`（只写面板级语句）、盖代号，
  不再整库；ensure 先重建 / 截断窗口丢段两种丢法同样收掉。**不变的**：③ 电平型原样、停放槽原样（现在是省一次反查的捷径，
  不再是跨库唯一的传播路）、整库兜底原样。
  **2026-09-19 追记（M2，折不出根的元素按「在 T 上还在不在」分两半）。** 审核（同页 §06 M2）：反查命中却折不出根
  （`anc ∪ 自身 ∩ DbSet@T 根清单 = ∅`）的元素此前任一个就退整库，而最常见的来源是**本窗刚删掉的根**——树 / 库上的残留
  还没刷掉、被面板的盒扫到，它在 T 上没有边要维护（对账删除那一半清），却把每一次定向都拉回整库。落法：
  `window_root_plan::missing_at_target(dbnum, T, refnos)` 拿同一个 `DbSet@T`（`build_set` 走生成输入缓存）问 `element_exists`
  ——dabacon 没有墓碑位，「不存在」就是这个时点键集里没有它；`run_targeted_room_recompute` 把反查命中的与停放链上折不出根的
  元素合起来问一次，`split_unresolved_by_existence` 分两半：已不存在的忽略（stdout 一句点名）、还在 T 上的才是「反查不完整」
  退整库；问不出来（读文件失败）也退整库——不把「不知道」当「不在」。**不变的**：`room_reverse` 仍如实把折不出根的进 `unresolved`、
  不静默丢；分类在调用方。
  **2026-09-19 追记（L1，保护集成员带着上一次可用的盒）。** 审核（同页 §06 L1）读出一条窄漏算，D9 保护集的设计后果：一块面板
  「有几何、有边 → 某一代几何不可用进保护集（旧边受保护留着）→ 再一代几何可用但**位置变了**」，`PanelDelta::between` 的旧盒取自
  旧代 `panels`，旧代里它在保护集 → `old = None` → 反查只问新盒，旧位置里那些成员的陈旧边没人清（三步连发才触发）。落法：
  `ResidentRoomModel::protected_bounds: BTreeMap<面板, PanelBounds>`——`ResidentRooms::install()` 在算面板变化**之前**从被换下的那一代
  给这一代 `missing_panels` 里每一块带一份 `last_known_bounds`（旧代有几何 = 旧代的盒；旧代也在保护 = 旧代带着的那份；跨几代都带），
  有几何的不带、不在册的不带（`removed` 按面板清边不问空间）、从没可用过的没有可带；`PanelDelta::between` 的旧盒改走
  `ResidentRoomModel::last_known_bounds`（有几何 = 此刻的盒，保护中 = 带着的盒），新盒只认新代此刻有几何的。重新有几何之后它不在
  保护集、那份盒随之丢掉。不进摘要、不进 `input_version`——它不是这一代的输入，是「离开的成员在哪」的备忘。
  **（同日退役，见下 L1-P 末段：旧盒不再是反查的输入，`protected_bounds` / `last_known_bounds` / `inherit_protected_bounds` 已删。）**
  **2026-09-19 追记（L1-P，离开的成员从库上按面板取）。** 复审 L1 读出它的漏洞：上一次可用盒只活在进程内，而**丢盒的方向是漏算**
  （账本丢了是多算——③ 兜底整库；盒丢了却让那一段照常定向、只问新盒）。形状：面板 P 有几何、有边 → `.mesh` 读不出进保护集 → **重启**
  → 本进程第一代 P 仍在保护集、没有上一代可带 → P 恢复且挪位 → 那一段 `old = None`，与「从没可用过」同一处置——只问新盒，
  P 旧位置里成员的陈旧边没人清。第一版落法（`2ba772b25`，L1-R）是保守门：第一代就在保护集的面板记成「旧盒说不出」，它恢复那一段
  `walk_chain` 不接、整库兜底——漏算换多算。**同日改成精确做法、门随之撤掉**：「离开的成员」根本不必问空间——此前算在 P 名下的每一个成员
  都在库上（`{P}->room_relate` 的 `out` 端，`in` 是 `unique_room_relate` 的前缀、走索引），`room_reverse::durable_panel_members(db, panels)`
  按 `PanelDelta::leaving_panels()`（`recalc` 里动了 / 恢复了 / 换了房的每一块 + `removed` 不在册了的每一块；`renumbered` 不进）一块一句
  取齐，与树上命中同一处置进 `resolve`（祖先链投影里有的从投影拿、没有的回库取——`inst_relate` 没有行的管身来源再按 `in` 到
  `tubi_relate` 取一遍）；别的库的成员同样进 `foreign` 停放。这个集合精确（就是要重算的那些边的另一端）、不靠旧盒、重启也不丢。
  **顺手收掉一条同族漏算**：`removed`（不在册了）的面板此前只清边不重算，它名下的成员若还在别的面板盒里，该落到那间房却没人算
  ——现在它也在 `leaving_panels` 里，成员的根照样重算。`AffectedRoots` 多 `member_hits` / `durable_member_lookups` / `durable_member_ms`，
  `summary()` 多一格 `member_hits=`、`durable_timing()` 多一句「面板现存成员 P 块 Qms」。
  **旧盒退役（同日第二步）**：`1e7dbfd91` 那一笔先留着旧盒 ∪ 新盒的空间查询（旧盒给在途还没落库的根），随后拿掉——「离开的」由成员那一路
  答得更准，在途未落库的根若与挪走的面板有关，随它自己那一台计算器落库、由 ③ 兜底，不为它多扫一遍旧盒。`PanelDelta::query_boxes()`
  只回 `recalc` 的**新盒**；`PanelMove::old` 只给日志 / 用例看「从哪挪到哪」（取旧代 `current_bounds`，保护中 / 不在册 = `None`）；
  L1 的 `protected_bounds` / `last_known_bounds` / `inherit_protected_bounds` 删掉，`install` 不再带盒——模型上没有任何跨代带盒的字段，
  「旧盒 None 是从没可用过还是说不出」这个问题不再存在。反查的形状定型为：**新盒问空间「谁进来了」，库上成员边答「谁离开了」**。
  **2026-09-19 追记（M2-S，还在 T 上却折不出根的再分一层）。** M2 分完之后 `stuck` 里还有一种常客：元素在 `DbSet@T` 上、祖先链上每一级也都在，
  却没有一级是本库的生成根——它的根本就不在生成范围（`model_type_scope` 之外 / 历史遗留几何）。此前它让每一次盒扫得到它的定向都退整库，
  而整库也不算它（根不在清单里）——持久多算、零收益。落法：反查 / 停放折不出根的元素连同祖先链一起回（`fold_to_roots` 第二个返回值带
  `anc`，`AffectedRoots::unresolved_ancestors`，`fold_parked` 第二个返回值同形）；`run_targeted_room_recompute` 在 `stuck` 非空时再问一次
  `missing_at_target`（这回问的是链上的每一级），`split_stuck_by_ancestry`：链非空且每一级都在 = 链现行、范围之外，忽略（stdout 点名）；
  链空（没取到）/ 有一级已不在 T 上（改过属主、链陈旧，说不了它现在归哪个根）= 仍卡住，退整库。问不出来照旧退整库。改了属主进了在册根
  的元素不靠这里：那个根本窗就在变更集里、随根重生成时算归属。
  **2026-09-19 追记（L3，定向成功只清消费过的段）。** 审核（同页 §06 L3）：`parked_for_task` 在开工时快照本库的槽，收尾
  `room_reverse::clear(dbnum)` 清的却是槽里**此刻**的全部——任务跑着的时候别的库停进来的段（或往同一段并进来的新元素）没折过根就
  随手丢了（不漏算：账本里还有那一段，③ 欠着照样接得上，只是白丢一次免反查的捷径；今天单个批次 worker 顺序消费，撞上的机会小）。
  落法：`room_reverse::clear_consumed(dbnum, consumed) -> (撤掉, 剩下)`——按 `from → to` 对条、去掉快照里有的元素、去空的整条撤；
  `run_room_recompute` 按 `(结论, 范围)` 分派：`(Targeted, Targeted { parked })` 走 `clear_consumed(job.dbnum, parked)`（`parked` 就是
  接进准入链的快照），`WholeLibrary` 仍整槽 `clear`（覆盖一切；中途停进来的若说的是更后面的代，③ 欠着、账本还有那一段）。
  `parked_for_task` 里「库上已与常驻同代顺手清槽」不变——那时停放物说的都是越过的代。
  **2026-09-19 追记（L2，只加量具）。** 审核（同页 §06 L2）：`durable_tube_hits` 每次定向 O(全部 `tubi_relate` 行) 的链接解引用，
  ADR 列的缓解（补 `aabb_d` 平副本）live 从没量过。**不做缓解**：平副本要么回填全部存量行、要么查询分两半（新行走平副本、老行照旧解
  引用），前者是一次迁移、后者没有数据说明省得下——没有数就不动查询。只加量具：`AffectedRoots::durable_tube_ms / durable_ancestor_lookups /
  durable_ancestor_ms`（`resolve` 里两次库读各计时），`run_targeted_room_recompute` 按段累加，「定向房间重算完成」那一行多一句
  「（库上管身范围查 Nms、回库取祖先链 K 个 Mms）」；`summary()` 不变。缓解等 live 的这个数说话。
  **2026-09-19 追记（F1，定向失败不丢证据：面板级语句挪到最后一笔、停放在反查完成就做）。** 第三遍复审读出 M1 的一条连带：
  D 追记 ⑤ 写的「`Failed` → 不盖、③ 再武装、**下一个任务整库**」在 M1 之前成立（自己那一段随任务丢、下一任务 `Unchanged` → 链断 →
  整库），M1 之后账本里那一段还在，`swaps_for_task` 欠着就整本给，下一个任务照样接链——**仍是定向**。而定向的面板级语句（`removed`
  的 `DELETE {P}->room_relate` / `<-room_panel_relate`）装在第一笔事务，`recompute_dbnum_room_edges` 逐笔提交、失败即返：第一笔提交后
  若后面某笔写库失败（`Err` → `Failed`，且不 `mark_generation_failed`），P 名下成员的边已经删了、没提交那几笔里的根新边没写；
  下一个任务重走同一段时 `durable_panel_members(P)` 回空、P 不在册也没新盒——这些成员若该落到别的房间（房间套房间）永远没人算，
  也没有重生成兜底。跨库那一半更直接：`Failed` 不停放，B 库下一任务沿账本接段时证据已被 A 的第一笔删掉。落法两条：
  ① 面板级语句从 `pack_publication_transactions` 的 `prelude` 挪到 `epilogue`（**最后一笔**）——三类语句与逐根块的先后无关（重算的根
  不会插向不在册面板的边、改号 UPDATE 幂等、面板拓扑边按面板先删后插），中途失败留下的是「重算了、面板级还没删」的多算形状，
  证据还在、下一个任务重走同一段就补齐；② `park_foreign_hits` 从「结论定为 `Targeted` 之后」改到**反查完成、折不出根的分类过关之后、
  `recompute_dbnum_room_edges` 之前**、不以结论为条件——反查本身已经完整，本库算没算齐、覆盖面对不对、退不退整库都不改变别的库欠
  什么；树不就绪 / 反查读库失败 / 仍卡住的 `return None` 都在它之前，反查说不了完整时到不了停放；同 `from → to` 再停一次是并集，重试
  幂等。⑤ 那句改口：`Failed` = 不盖、③ 再武装、下一个任务沿账本**重走同一段**（定向），靠 ①② 保证重走是完整的。
  **2026-09-19 追记（F2，定向路补 `recalc` 面板自己的 `room_panel_relate`）。** 同一遍复审：`render_panel_delta_statements` 只处理
  `removed`（删两边）/ `renumbered`（改列），`recalc`（新在册 / 换了房 / 几何变了）面板的「房间 → 面板」边只有它**自己的根**被重算时才写
  （`render_root_room_edges` 对面板元素那一句），而反查命中的是**成员**的根——面板自己的根多半不在集合里（PANE 从树命中里剔掉、它的
  兄弟元素也不一定在盒里）。形状：FRMW / SBFR 改名进关键字（NAME 在 `DATA_ONLY_ATTR_NAMES`、`attribute_is_metadata_only` 放行，根不重
  生成）→ 面板新在册 → 代号变 → 定向：成员的 `room_relate` 齐了，这些面板的 `room_panel_relate` 一条没有；换了房同理（旧边留着、新边没有）。
  整库路 / 从零初始化都会写，实施约束 8「两张边 == 从零初始化」在第二张表上不成立（今天没有生产读者——`fn::room_code` / `room_num_of` /
  `get_room_nodes` 只读 `room_relate`——所以看不见）。落法：`MemoryRoomCalculator::render_panel_topology_statements(panel)`（与逐根路径对
  面板元素写的同一句：先删指向它的旧入边，在册再写回本代的），`model_db_adapter::render_recalc_panel_topology_statements(rooms, panels)`
  对链上每一块 `recalc` 面板渲一次（按本任务计算器的在册视图 = 链尾那一代；面板 refno 序、去重），拼在各段面板级语句之后、同进最后一笔。
  **2026-09-19 追记（F3，常驻代号回到旧值时中间那一代生成的根没人补——投影里 ACK 的代号就是欠账的证据）。** 同一遍复审的第三条：
  代号是输入摘要、**不单调**（D12）。形状：库上盖 g1 → 一块 PANE 的 `.mesh` 暂时读不出（重建 → g2；`PanelDelta::between` 只遍历
  `new.panels`，进保护集这一段是**空段、不进账本**）→ 期间「显示模型」`/model/ensure` 按 g2 重生成若干根（准入拿 ACK 代号对
  `required_room_generation()`，不等即重生成，是常态；ensure 本身只发到投影——`publication_policy(true)` 在摄入形态是
  `ProjectionOnly`——边是 **write-behind 刷盘那一刻**按 `room_stage` 那台计算器写进库的，那时常驻仍是 g2；一次没盖章的
  `Failed` / `PartialCoverage` 批次任务同形）：P 不在索引，新进 P 的根拿不到 P 的边 → `.mesh` 恢复（重建 → g1，摘要同值）
  → 下一个批次任务：refresh `Unchanged`、库上 g1 == 冻的 g1 → ③ 不欠、账本没有 PANE 变更 → `request_for_task` 回 `None`。那几根的
  ACK 是 g2；`e3d.room.lookup` 第一层不认、第二层「库上 g1 == 常驻 g1 且账本没欠」→ 回 `ready`，边却是 g2 算的。面板挪走又挪回
  （撤销）同形。落法三条：① **投影里本库每一根 ACK 的房间代号进准入**（`room_recompute::acked_roots_for_task` → `AckedRoot { root,
  room_generation }`，只收回执与 ACK 对得上的、发布时没算房间的记 `None`；两个执行体在 `swaps_for_task` 之后取、`request_for_task(…, &acked)`
  传进去）：`scope_for_task(library, resident, swaps, parked, acked)` 在**库上 == 常驻**时，代号 ≠ 这一代的根就是欠账本身——
  `Targeted { swaps: [], parked: [], stale_roots, generation }`（**空段定向**），一根都没有才回整库（被 ①/② 问到的那种）；链接得通时
  代号**不在链上任一代**（库上那一代 ∪ 各段两端 ∪ 链尾）的根一并进 `stale_roots`——链上各段说的是「这几代之间面板动了什么」，它那一代
  与链的差链说不了；在链上的不点名（各段都反查过，没命中就是边没变）。`request_for_task` 第四句原因「投影里 N 根的 ACK 房间代号不在
  准入链上……点名重算」，库上同代、前三条都没点亮时它单独触发；库上同代却被 ①/② 问到仍整库（它们说不出欠什么，整库覆盖点名的根）。
  `run_targeted_room_recompute(…, stale_roots, …)` 把点名的根与反查命中、停放折出的并起来进 `RootScope::Targeted`。
  ② **盖章那一刻把链上的 ACK 一并对齐到链尾**（`model_db_adapter::align_room_acks_along_chain(store, dbnum, on_chain, generation)`，
  在覆盖面对上之后、结论 `Targeted(g)` 之前）：投影里 ACK 代号在链上任一代、又没被命中的根，它那一代到链尾之间面板动了什么各段都
  反查过、没命中就是边没变——与库上那一格「整库对齐到这一代」是同一句话的逐根形态。没有这一步，定向之后没命中的根 ACK 全停在上一代，
  下一个任务 F3 会把整个投影点名一遍（O(投影)，定向白做）、读侧会把它们全否掉。链**外**的代号不动——那正是「盖章之后中间那一代写的」
  证据；没算房间的不动；候选前移了的 `record_publication_ack` 拒收、由它自己的刷盘另记。`recompute_dbnum_room_edges` 对投影里一条
  几何都没有的根（空根）也对齐 ACK（没有边可陈旧，什么都不写）——否则它永远挂着旧代号、每个任务再点名一次。整库那一路每一根都算、
  ACK 由它自己对齐，不走链。有了 ①②，本库「ACK ≠ 库上代号」只剩一种含义：盖章之后另一代写的。③ **读侧一票否决**：
  `query_service::room_calculation_status` 的第 1 层从两态改成三态 `ProcessAck::{Missing, Current, Stale}`——有 ACK 而代号不是此刻
  常驻这一代（或发布时没算房间）= `Stale`，`room_edges_ready(Stale, …)` 恒 `false`，库上那一层再对也不替它说话，回 `not_computed`，
  下一个任务的空段定向点名重算它；`Missing` 照旧由第 2 层作答，`Current` 照旧直接就绪。**代价**：中间那一代生成的根，即使边碰巧没变
  （不在那块面板附近）也点名重算一次、在被算齐之前查询回 `not_computed`——多算不漏算；每个任务多一趟 O(本库投影根数) 的读锁扫描。
  **没收的**（同日由下条 F3 落库收）：ACK 是进程内的——中间那一代生成的根若在下一个任务之前随进程重启丢了 ACK（或
  `drop_cached_root` 退役），证据随之消失，库上 g1 == 常驻 g1 之下没人再点名它（与账本 / 停放槽「重启即丢」同一形状，但这里 ③
  兜不住，因为库上与常驻同代）。
  **2026-09-19 追记（F3 落库，`dbnum_watermark.model_room_stray_generations`：盖章之后按哪几代写过根，重启不丢）。** 用户拍板
  B（不逐根落——那是把 ADR-079 刚退役的 `gen_root` 换个名字请回来；也不单开小表——借水位行现成的镜像读与清库路径）：水位行多一格
  **集合** `model_room_stray_generations: [代号]`——「库上 `model_room_generation` 盖章**之后**，本库有根按这几代写过边」。
  ① **写**：刷盘一组按 `rooms` 这一代写边、而这一代 ≠ 库上那一格时，`model_db_adapter::stray_room_generation_statement` 渲一句
  `UPDATE dbnum_watermark:⟨dbnum⟩ SET model_room_stray_generations = array::union(model_room_stray_generations ?? [], ['g'])`，作
  `persist_group` 的 **epilogue 随本组的行同一笔事务**提交（一条几何都没有的组不带——没有边可陈旧）；库上那一格走水位镜像
  （`DbnumState::read`，生产库）/ 点读（旁路连接），提交成功后 `invalidate_mirrored_on`。**不写**的几种：没有水位行、库上从没盖过
  （`None`——③ 本来就欠着，整库一次就盖上）、与盖章同代（稳态：一个字都不写）、那一格已含这一代（每 (库, 代) 一次；不靠进程内
  去重——盖章清掉之后再按这一代写就得再记）；读不出水位行只 warn 不写。② **清**：`task_end::render_watermark_update` 的 `Aligned(g)`
  与 `Cleared` 两臂一并 `= []`（盖到这一代 = 库上每条边都与这一代一致，此前记的另几代已被覆盖），`Untouched` / 模型没成不碰——与
  `model_room_generation` 同一道 `all_roots_ok` 门、同一条「说不了整库的任务不碰」纪律。③ **读**：`room_recompute::durable_alignment`
  本来就读这一行——`DurableAlignment { library, resident, stray }`，`owed()` = 库上 ≠ 常驻（③ 原句）**或** `stray` 里有与库上不同代的
  （`stray_generations_owed`：「库上记着盖章之后本库有根按另 N 代写过边……欠一次，F3 落库」）；`request_for_task` 里 `library_current`
  随之为假 → `scope_for_task`：进程内还有代号不等的 ACK → 空段定向点名（F3 原路，精确）；重启后没有 ACK → 整库（多算不漏算）→
  盖章 → 那一格清空。`/dbnums.room_alignment` 多一格可选 `stray_generations`（空时不出，形状对既有读者不变）。**代价**：稳态零；
  换代期每 (库, 代) 一句 UPDATE 随刷盘同事务；水位行多一个写者（此前只有 `task_end` 与初始化 / 清库）——它写的是簿记不是水位，
  W3「不是第三枚水位」照旧成立，rocksdb 上与收尾那一笔同行撞事务由刷盘既有的重试退避接住。**不动的**：F3 进程内那一路原样；
  `persist_root_impl`（单根、不写边）不带这一句；ADR-079「逐根成败只活在进程里」一字不改——这一格是每库一份、集合、盖章即清。
  **2026-09-19 晚 F3 收口总表（`76a4dd480` / `3c932e17d` / `35e022318`）。** 三笔加起来收了什么、还欠什么，一处看全（各行的细节在上面
  两条追记与钉 33 / 34 里，这里只列结论）：

  | # | 洞 / 疑点 | 收法 | 落点 | 状态 |
  | --- | --- | --- | --- | --- |
  | 1 | 常驻代号回到旧值，中间那一代（write-behind 按 g2 刷进库）写过的根没人补——③ 不欠、账本没段、读侧回 `ready` | ① 投影里 ACK 房间代号进准入：库上 == 常驻时代号 ≠ 这一代的根走**空段定向** `Targeted { swaps: [], parked: [], stale_roots }`；链接得通时代号不在链上任一代的根一并点名 | `76a4dd480`，钉 33，`a_stale_ack_in_an_aligned_library_is_the_debt_itself_and_goes_targeted_on_its_own` | 已收 |
  | 2 | 原建议字面落会**回退**：普通定向只刷命中根的 ACK，盖章后其余根停在上一代 → 下一任务点名整个投影、读侧全否 | ② 定向盖章时 `align_room_acks_along_chain` 把链上没命中的根一并对齐到链尾；`recompute_dbnum_room_edges` 对空根也对齐 | `76a4dd480`，钉 33，`mem://` `a_root_generated_in_a_transient_generation_is_repaired_when_the_resident_returns` | 已收 |
  | 3 | 读侧有 ACK 而代号 ≠ 常驻，第 2 层却替它说「就绪」 | ③ `ProcessAck::{Missing, Current, Stale}`，`Stale` 一票否决回 `not_computed` | `76a4dd480`，钉 33 | 已收 |
  | 4 | ACK 是进程内的：中间那一代写过的根在下一个任务之前随**重启**丢 ACK，库上 == 常驻之下没人再点名 | 水位行 `model_room_stray_generations`：刷盘按 ≠ 库上那一代写边时随本组最后一笔**同事务**记、`Aligned` / `Cleared` 清、`durable_alignment` 读 → 重启后整库一次 | `3c932e17d`，钉 34，`a_stray_generation_survives_a_restart_on_the_watermark_row_and_rearms_the_recompute` | 已收 |
  | 5 | 疑点：合块事务里一句失败**不**回滚其余语句（至少 mem 上如此）？ | **误判**。`bin/surreal.exe` 2.1.4+45013fc9（与 SDK 同一提交、同一台 executor）在 memory 与 rocksdb 各跑同一组探针：`BEGIN … COMMIT` 里任一句报错（`THROW` / 数组 `+`）整笔零提交、其余各句回 `not executed due to a failed transaction`；当时看见 A2 的边落库，是那一版 epilogue 还没进同一笔事务 | `35e022318`，`publication_error_retains_the_cause_after_rollback_errors` 连回滚本身也钉住 | 已核 |
  | 6 | 「进程活着却丢 ACK」是不是第二个洞 | 不是：投影没有别的逐出口——`drop_cached_root` 只在 `abandon_undurable_roots`（刷盘判死）那一路，且下一窗并进 regen 重刷、按当时常驻重写边，自愈 | 源码核，没改 | 已核 |
  | 7 | F3 追记里「ensure 按 g2 生成……写了边」说得不准 | 改口：ensure 在摄入形态 `ProjectionOnly`，边是 write-behind 刷盘那一刻 `room_stage` 那台计算器写的 | `3c932e17d`（本条 F3 追记） | 已收 |
  | 8 | `PartialCoverage` / `Failed` 批次任务（盖 `model_sesno` 不盖 D14）按任务那一代写的边，常驻回到库上那一代后同一个洞 | 同一条写路径：任务的刷盘同样经 `persist_roots_batched_unacked_impl`，代号 ≠ 库上就记 stray；不盖章所以不清，下一任务照欠 | `3c932e17d` | 已收（同路径，未单独钉） |
  | 9 | live：`.mesh` 暂时读不出 → 点「显示模型」→ 恢复 →（重启）→ 下一窗「ACK 代号不在链上点名 N 根」/「… F3 落库」+「整库房间重算完成」，`/dbnums.room_alignment.stray_generations` 两窗之间非空、之后消失；稳态下那一列恒空 | — | 需真库 | **待 live** |
  | 10 | rocksdb 上水位行两个写者（刷盘 epilogue 与 `task_end` 收尾那一笔）同行撞事务 | 设计上由刷盘既有的重试退避接住 | 未在 rocksdb 上实测 | **未实测** |
  | 11 | stray 里记着的一代在投影里**一根 ACK 都没有作证**（重启前按 g2 写过 → 重启丢 ACK → 下一任务前又有一段 g3 刷了根 → 常驻回 g1）：`scope_for_task` 只点名 g3 那几根，盖章把整格清掉，g2 那几根的陈旧边就此无人认领 | 建议规则：`stray`（去掉与库上同代的）里有任一代在 `acked` 里找不到见证 → 整库（多算不漏算）；约 10 行 + 一条纯函数钉 | 未落 | **未收**（窄：两段中间代跨一次重启、中间没有批次任务） |
  | 12 | 代价：中间那一代生成的根即使边没变也点名重算一次、算齐前查询回 `not_computed`；每任务多一趟 O(本库投影根数) 读锁扫描；换代期每 (库, 代) 一句 UPDATE | 接受（多算不漏算；稳态零） | — | 接受不收 |
- **D14　durable 侧的房间就绪承诺：`dbnum_watermark.model_room_generation`（`e3d.room.lookup` 的第二层判据）。**
  起因：2026-09-16 rocksdb 真库复跑（`docs/evidence/2026-09-16-room-membership-live-and-real-file-acceptance.md` §6 F2）——
  三个库的边、房间号、版本三元组全对，`e3d.room.lookup` 的 `calculation_status` 却恒 `not_computed`。判据是 2026-09-11
  那节定的**进程内根 ACK**，而 ACK 按 ADR-079 只活在进程里：库是上一次运行灌的，冷重启后一条 ACK 都没有，
  `ready_empty` / `ready_nonempty` 在生产存储形态下**不可达**。缺的不是事实，是够得着的判据字段。
  处置：水位行上加一格 `model_room_generation` = 「这个库的房间边整库对齐到哪一代常驻模型」，取值就是根 ACK 里那个
  `room_generation`（`ResidentRoomModel::input_version`，D12）。
  - **写**：随 `model_sesno` 由收尾那一笔同盖，与库级目录落款（ADR-079 D4）同一条纪律——只有能替**整库**说话的任务才盖：
    初始化（每个根都是这台计算器算的）、整库重排的 ModelOnly（起点水位为 0 / 缺席）、本任务跑过整库房间重算（D13）**且覆盖到
    `DbSet@T` 的每一根**（`batch_worker::RoomRecomputeVerdict::WholeLibrary`，2026-09-17 补：投影里的根按投影几何、不在投影里的
    根回读库上几何——落盘形态下也盖得上；根清单枚举不出来、覆盖面未知是 `PartialCoverage`，`model_sesno` 照盖、这一格原样不动
    ——替没算过的根说「对齐到这一代」就是说谎）。
    普通增量窗口只重算被触到的根，别的根的边仍是上一代算的，说不了这句，那一格**原样不动**；`all_roots_ok` 为假一律不写；
    `room_membership=false` 下盖模型水位时**清空**（这一版水位只承诺几何，不能留着替旧边说话）。这一格还是 D13 触发 ③ 的读数：
    与此刻常驻代号不等 = 欠一次整库重算——所以「盖」与「不盖」都要诚实，盖早了 ③ 就哑了。
    **盖的代号是边真正用的那一代，不是收尾那一刻的常驻代号**（2026-09-17 审核 P2-3）：批次 worker 单消费者，但 `/model/ensure` 与
    初始化 lane 都会并发调开工前对齐，一个 DESI 前进 + 用户同时点「显示模型」就会在任务中途换代，拿收尾那一刻的
    `required_room_generation()` 盖，盖上的可能是一个边不是它算的代号。三路各有出处：初始化拿双 lane 开工冻的那台
    `rooms.resident_generation()`（`dbnum_initialize`，原本就对）；整库房间重算拿**重算那台计算器**的代号
    （`RoomRecomputeVerdict::WholeLibrary(代号)`，边是它算的、ACK 已对齐到它，此刻常驻是哪一代无关）；整库重排的 ModelOnly 在开工
    对齐**之后**记一次代号（`DataBatchTaskResult::room_generation_at_start`），收尾与此刻比——相等 = 每次刷盘冻的都是这一代，盖它；
    不等 = 中途换过代、各批根的边不是同一代算的，`Untouched`（下一个任务由 ③ 补一次整库重算）；开工没记下也不盖。
  - **读**：`e3d.room.lookup` 的就绪判据变成两层——进程内该根的 ACK 命中，**或**库上这一格与此刻常驻模型同一代且本进程的
    账本里这根没欠着（ADR-079 的一票否决）。两层用同一个代号，所以也是同一句判据：房间 / 面板真变过 → 代号变 → 两层同时
    作废 → `not_computed`，不拿上一代的边冒充这一版的结论。本进程替这根记着**另一代**的 ACK（有 ACK 而代号 ≠ 常驻、或发布时
    没算房间）同样一票否决（D13 追记 F3，2026-09-19）：库上与常驻同代时那枚 ACK 只可能是盖章之后中间那一代写的（盖章时链上的
    ACK 都已对齐到盖的那一代），第 2 层替不了它说话。只在**落盘**形态下作数：进程内嵌 `mem://` 活不过重启，
    库与水位行一起消失，那一档仍只认进程内 ACK（d-381 的启动屏障要的就是本进程逐根算完那一句）。
  - **看**：`/dbnums` 每行多一格可选的 `room_alignment{generation, resident_generation, current}`（DESI 行才有，spec §4.7）
    ——`current: false` 就是「这个库此刻只能回 `not_computed`」的解释，是这句话的唯一出处。那句「成不成立」与查询侧共用
    一处判据（`query_service::library_room_alignment_is_current`）：各写一遍就会出现面板说「对齐了」而同一个库的查询回
    `not_computed`。它是现状陈述不是告警——要点亮，给该库跑一次 `model/rebuild` 或重初始化。
  - **它不是第三枚水位**：没有自己的推进规则、不参与任何放行 / 恢复判决，只是模型水位的一个属性（「这一版的边对齐到哪一代」），
    W3 说的 `room_*` 五列一个都没回来。旁边那一格 `model_room_stray_generations`（D13 追记 F3 落库，2026-09-19：「盖章之后本库有根
    按哪几代写过边」，刷盘按另一代写边时同事务记、盖章即清）同理——是这一格的附注，不是水位。库上这一格与真实的边之间只有一个缺口：关着开关跑过一轮之后再打开——那正是
    2026-09-11 那节已经接受的降级（重新打开只影响之后重生成的根，补齐要显式 `model/rebuild`），而 `Cleared` 让这一格
    在那种局面下说「没有承诺」而不是说谎。
- **不改**：逐根失败（构件包围盒不可用、候选面板网格取不到）仍 = 该根失败（D2）；`room_membership=false` 那一档（2026-09-11）不动；
  D4 库层资源不动；§未决 U2、U4 原样，U1 由 D13 收，U3 的「冷启动 PANE 未生成怎么办」由 D9 保护集 + D12 开工前对齐一起接住
  （面板几何到位的下一个任务开工时进常驻），跨库那一问仍开着。

取代关系：本节在**输入不全**这一形态下取代 D2 里「常驻模型建不起来则本轮的根都算不出房间」的适用；W3「全部根（含各自的房间归属）
成功」在 `Degraded` 下按 D11 解读。ADR-067 D6 / 共识 D-A (a)「有几何行却不可用的面板挡参照」随参照机制一并作废（D9）。

源码钉（续实施约束 1–9）：

10. `MemoryRoomCalculator::new` 不调 `ensure_complete`；`render_root_room_edges` 在 `missing_panels` 非空时输出
    `LET $room_protected_panels` 与 `WHERE in NOT IN $room_protected_panels`，为空时语句与此前逐字相同。
11. `room_topology::latest_frozen_sources` 不再因缺 DESI 文件返回 Err；缺失清单随返回值走到 `ResidentRoomModel::missing_dbnums`。
12. `ResidentRoomModel::warm_meshes` 只对预热 worker 崩溃返回 Err；面板级失败搬进 `missing_panels` + `problems`，
    `preload_panel_projection` 的单面板生成失败同样只留痕。
13. `ensure_ready`：`Degraded` → `Ok`。
14. `mem://` 真测：(a) 在册面板缺几何 → 该面板旧边保留、其余根照常安定、`model_sesno` 前进；(b) 缺一个 DESI → 其余库照常生成，
    `/health.features.room.status = degraded`。
15. `refresh_room_reference_before_task` 出现在 `execute_frozen_batch_body` **与 `execute_model_only_body`** 各自的
    `run_model_side_actions` / `run_unit_worklist` 之前（两个执行体各钉一段，函数体到顶格 `}` 为止，后一个函数的字样不替前一个作证；
    ModelOnly 那一处 2026-09-17 审核补上——此前启动追平 / `model/rebuild` 拿的是未对齐的常驻）、
    `dbnum_initialize::model_half_inner` 的 `frozen_room_calculator()` 之前、`model_dbnum_ensure` 准入的 `required_room_generation()`
    之前（`explicit_room_generation().is_none()` 门内）；`latest_frozen_sources` 全文不含 `E3dModelService::from_current` /
    `from_frozen_sources(` / `open_from_env`；`resident_generation()` 与 `required_room_generation()` 同为 `input_version`
    （钉 `the_task_start_refresh_precedes_every_calculator_freeze_and_stays_cheap`、
    `the_resident_generation_follows_the_calculation_input_not_the_file_composite`）。实施约束 1 里「`execute_frozen_batch_body`
    全文不含 `room_resident`」照旧——对齐经 `model_db_adapter` 转一手，执行体不直接摸常驻模型。
16. `mem://` 真测：(c) 启动后某 DESI 前进、新增一块 PANE → 下一个任务开工日志有「文件版本变了，常驻房间模型按文件重建」、
    `/health.room_build.resident.last_swap.note = task-start refresh`、该任务的根对新面板算得出归属；(d) 只前进一个不含房间 /
    面板的 DESI → 重建后代号不变、此前的根 ACK 仍 `ready`。
17. `build_model_update_plan_from_window` 从 `sources.plan.ledger.entries()` 调 `room_recompute::room_definition_change`、排
    `room_only_work_item`；`execute_frozen_batch_body` 经 `room_recompute::request_for_task` 交出「要不要」；`execute_model_only_body`
    经同一句交出（`Some(&room_reference)` 两条触发都带；`reconcile_startup_window` 与稳态窗口共用同一条规划入口，`RoomOnly` 工作项
    随 `side_actions` 进来——此前这里硬编码 `room_recompute: None`，停机期间的 PANE 改动经启动追平进来就被静默吃掉，2026-09-17 审核补上；
    整库重排（`whole_library_model_only`，起点 0）例外：每一根都刚用对齐后的计算器算过、承诺已由那一句盖上，不再空转一遍；
    规划失败的早退仍 `None`）；两个执行体里触发 ③ 与定向准入共用的读数 `room_recompute::durable_alignment(job.dbnum, rooms.generation())`
    （库上那一格 + 本任务冻的计算器代号，一次读；H1 2026-09-19 起常驻代号只从冻好的那台取，执行体不含 `resident_room_generation()`）
    落在 `refresh_room_reference_before_task(job.dbnum)` 与 `TaskRoomCalculator::freeze()` **之后**、`request_for_task(` 之前，并以
    `alignment.as_ref(),` 真的传进去（对齐之前读到的常驻代号是上一代，比对会白点一次或漏点一次；2026-09-17）；`execute_frozen_batch`
    的 DESI 收尾段里 `let task_rooms = result.room_calculator.clone();` →
    `run_room_recompute(job, &request, task_rooms.as_ref(), &mut result, file_latest_sesno)`（定向 / 整库的分派，D 追记；定向复用本任务
    冻的那台，H1）在 `await_units_durable(` 之后、
    `model_outcome_of(&flush` 之前、`flush.all_acked()` 门内，算不齐 `outcome.all_roots_ok = false`，收尾分开问两句
    `RoomRecomputeVerdict::rooms_ok`（水位）与 `::speaks_for_library`（D14 那一格）；`run_room_only_recompute` 函数体里
    `library_generation_roots(dbnum, target_sesno)` 落在 `recompute_dbnum_room_edges(` **之前**并以 `expected.as_deref()` 真的传进去
    （根清单是回读库的靶子），全文不含 `settle_partial_coverage(` / `rearm_library(`，`room_recompute` 生产段不再定义
    `PARTIAL_COVERAGE_SETTLED`（去重表 2026-09-17 退役）；`manual_update` 的 side_actions 过滤只拦 `RoomRecalc*`（`is_room_recalc` 不含
    `RoomOnly`）（钉 `the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end`、
    `a_change_to_any_room_definition_noun_triggers_and_nothing_else_does`、`a_task_recomputes_rooms_on_either_trigger`、
    `the_library_owes_a_recompute_until_its_durable_alignment_matches_the_resident`、
    `the_library_recompute_reads_back_the_durable_geometry_of_roots_outside_the_projection`）。
18. `recompute_dbnum_room_edges(…, library_roots: Option<&[RefNo]>)` 走 `calculate_for_root` + `render_root_room_edges` 同一套渲染
    ——函数体里各恰一处（两种来源共用的那一段）：重算后的边与逐根落库逐字相同、算不出的根边原样进 `failed`、投影里根的 ACK 代号
    对齐；点名的根里投影有回执的不回读（`!projected_set.contains(&packed_refno(*root))`），其余经
    `model_record_reader::durable_records_of_root(` 读库、再经 `room_model::durable_room_inputs(&rows)` 折形状（先读后折）；回执
    `covered_roots` = 投影里有回执的根 + 回读的根，按 refno 序、只认本库；这里不枚举文件（钉
    `recomputing_a_librarys_room_edges_reuses_the_per_root_rendering_and_refreshes_acks`、
    `a_root_outside_the_projection_gets_its_room_edges_recomputed_from_the_durable_rows`——`mem://` 真测：A 经逐根路径落库后从投影
    消失、面板搬家 → 点名 `[A]` 回读重算、旧边删掉、模型行不动、`acks_refreshed == 0`；搬回来边逐字回来；点名里没生成过的根算覆盖
    到、别的库的根不认；`a_partial_coverage_recompute_keeps_the_watermark_but_never_claims_library_alignment`——`RoomRecomputeVerdict`
    四态的两句真值表 + `room_recompute_uncovered` 的顺序无关 / 点名 / 空库 / 未知四例）。
    `mem://` 真测 (e)：改一块 PANE 的 NAME 以外的属性 / 把一块 PANE 换属主 → 该窗任务日志有「整库房间重算完成」、别的根在这块
    面板里的成员边随之更新；(f) 只改 BRAN → 不触发。
19. D14：`task_end::render_watermark_update` 对 `model_room_generation` 只有三种动作（`Untouched` / `Cleared` / `Aligned`），
    与 `model_sesno` 同在 `all_roots_ok` 门内；「起点为 0 的 ModelOnly」这句资格判定全文只有一处定义
    （`whole_library_model_only`），库级目录落款与房间代号共用它；`room_generation_stamp_of(now, LibraryRoomClaim)` 的代号出处：
    `Recomputed(代号)` 盖载荷（`now` 是什么、取不取得到都不影响）、`Rebuilt { started_at }` 只在 `started_at == now` 时盖 `now`
    （不等 / 没记下 / 取不到 → `Untouched`）、`Ok(None)` 一律 `Cleared`；`WholeLibrary(rooms.resident_generation().to_string())` 在
    源码里、ModelOnly 执行体的 `let room_generation_at_start =` 落在开工前对齐之后、`run_unit_worklist(` 之前，收尾从
    `result.room_generation_at_start.as_deref()` 取（2026-09-17 P2-3，钉 `only_a_task_that_speaks_for_the_whole_library_stamps_the_durable_room_generation`）；`query_service::room_calculation_status` 的函数体里
    同时出现 `publication_ack(`、`room_edges_ready(` 与 `model_root_ledger::entry(`，`room()` 里的库上那一层按本次查询的
    dbnum 现读（`Self::library_room_generation(dbnum).await`）。钉
    `the_room_generation_stamp_follows_the_model_watermark`、
    `only_a_task_that_speaks_for_the_whole_library_stamps_the_durable_room_generation`、
    `the_whole_library_eligibility_is_decided_in_one_place`、
    `the_durable_alignment_answers_for_roots_this_process_never_published`、
    `the_durable_layer_needs_a_persistent_store_and_the_same_generation`、
    `the_room_status_reads_both_the_process_ack_and_the_durable_alignment`、
    `the_dbnum_row_carries_the_durable_room_alignment_as_one_optional_cell`（`/dbnums` 那一格与两枚水位同一趟读回，
    「成不成立」只许有一处判据）。
    live：rocksdb 真库冷重启（不重摄入）后 `e3d.room.lookup` 对有边的构件回 `ready_nonempty`。
20. D12 退避（P3-6）：`refresh_before_task` 函数体里 `RoomReferenceRefresh::Deferred {` 落在 `latest_frozen_sources().await` 之后、
    `initialize_file_backed_at_startup(` 之前，判据只看探针给的 composite（`backoff.remaining(&probe.composite, started)`）；
    `Ok` 臂 `set_rebuild_backoff(None);`、`Err` 臂 `RebuildBackoff::after_failure(` 对着 `&probe.composite,` 记账，生产代码里
    `set_rebuild_backoff(` 恰三处（定义 + 清账 + 记账）、`after_failure(` 恰一处；`cooldown(n) = 30s × 2^(n-1)` 封顶 10min、
    `remaining` 对不同 composite 为 `None`、冷却期一到为 `None`、换 composite 从 1 数起（钉
    `the_task_start_refresh_backs_off_before_rebuilding_and_settles_the_ledger_on_both_outcomes`、
    `a_failed_rebuild_backs_off_under_the_same_composite_and_rearms_when_the_files_move`）。
21. D1 追记（B，2026-09-17）——房间在 bake 时算、边随回执、flush 只渲染：
    - 取点只有一处定义：`room_model::world_points_of(` 全仓恰一处，`memory_model_world_points`（回读）与 `e3d_model_service::bake_element`
      （手上的 `mesh`）都调它；`bake_element` 不含 `world_mesh.vertices`；「网格 → 世界」只经 `room_model::projected_mesh_to_world`
      （`ProjectedGeometry::mesh_to_world` 与 `baked_room_input` 同一条），`baked_room_input` 与 `project_geometry_delta` 共用
      `geometry_source_refno` / `geometry_record_id` / `persisted_world_transform`（钉 `component_points_from_the_in_memory_mesh_equal_the_disk_read_back`、
      `bake_time_room_input_matches_the_projected_record`、`bake_computes_room_edges_from_the_mesh_it_is_about_to_write`、
      端到端 `baking_computes_the_same_room_edges_the_flush_would_and_fails_the_root_when_it_cannot`——真 cube manifold 下 bake 的边 ==
      flush 回读算的边，含跨面板边、真走点检查的那件）。
    - 一任务一台：两个执行体 `TaskRoomCalculator::freeze()` 落在 `refresh_room_reference_before_task(job.dbnum)` 之后、`run_unit_worklist(`
      之前且以 `&rooms,` 传入；`run_unit_worklist` 里 `rooms.unavailable()` 的判死早于三路分流、`task_rooms.clone()` 恰两处（批量 + 逐根）、
      函数体不含 `frozen_room_calculator()`；`produce_slices → spawn_generation_worker → bake_snapshot → bake_elements → bake_element` 逐层
      显式传递、工作线程里不自己冻（钉 15 扩、`the_unit_worklist_uses_the_task_room_calculator_and_fails_fast_when_it_is_unavailable`）。
    - 边随回执：`RootReceipt.room`（`#[serde(default)]`，旧产物 `None`）只有 `persist_memory_batch` 一处挂（`receipt.room = carried_room;`
      在 `project_geometry_delta(` 之后），刚体前移 / 单元级发布的回执 `room: None`；`bake_snapshot` 用 `rooms.resident_generation()` +
      `geometry_source_refno(&element.geometry_id)` 全部构件装 `carry_root_edges`（钉 `carried_room_edges_ride_only_on_the_regen_receipt`、
      `carried_room_edges_round_trip_and_list_every_element_of_the_root`）。
    - flush 三道闸：`room_stage` 先 `carried_edges_for(`（`carried.generation != resident_generation` / `elements.len() != listed.len()`
      任一即 `None`），`calculate_for_root(` 只剩兜底一臂、`render_root_room_edges(` 恰一处；带来的与现算渲出的语句逐字相同
      （钉 `room_membership_is_computed_per_root_before_the_write_and_rides_the_same_transaction` 扩、
      `carried_edges_pass_only_when_generation_and_element_set_both_match`、`room_stage_renders_carried_edges_and_recomputes_when_a_gate_fails`）。
    - D2 前移：`produce_slices` 非 `Required` 的 `Err` 臂只 `slice.failures.push`、不 `slice.push`
      （钉 `a_root_whose_rooms_cannot_be_computed_is_a_dead_letter_and_never_published`）。
    live 待补：ams 1112 一个增量窗 `[model-write-behind] … room_carried=N room_recomputed=0`，硬验收「增量收敛后两张边 == 从零初始化」照跑。
22. D13 追记（D，反向传播，2026-09-17）：
    - 面板变化：`ResidentRooms::install()` 里 `PanelDelta::between(old, &model, &diff)` 对着被换下的那一代算、与 `TopologyDiff` 同一次
      （生产代码里 `TopologyDiff::between(` 仍恰两处，钉 P3-7 扩）；两个启动装载器把 `InstallOutcome { model, diff, panels }` 接进
      `StartupInitOutcome`，`refresh_before_task` 把 `outcome.panels` 装进 `Rebuilt { panels, previous_generation }`（钉
      `install_reports_which_panels_moved_and_where_their_boxes_are`：只动几何 → 拓扑差空、`recalc` 带新旧盒、原样的面板不进；
      旧代没几何新代有了 → 只有新盒；改号 / 移出在册 → 不反查、`removed` 不带盒）。
    - 准入：`scope_for_task` 四条同时成立才 `Targeted`（钉 `a_task_goes_targeted_only_when_this_swap_is_the_librarys_whole_debt` 八种
      整库局面的真值表；三条触发同时点亮仍定向）。
    - 反查：`room_reverse::affected_roots` 先 `ensure_spatial_ready()`（不过 → `TreeUnavailable`），树读锁只罩取候选；
      `tree_element_hits` 剔 `PANE`、跨盒去重；`resolve` 合三路命中、按 `RefNo::dbno()` 分库只数、`durable_ancestors` 按记录 id 批量取
      `anc`、`fold_to_roots` 交根清单（钉 `hits_fold_to_every_library_root_on_their_ancestor_chain`、
      `tree_candidates_exclude_panels_and_deduplicate_across_boxes`、`mem://`
      `resolve_joins_tree_projection_and_durable_tubes_into_library_roots`——投影管身 + 库上管身 + 树上不在投影里的元素回库取 `anc`
      折成四根，别的库按 dbnum 在库里就筛掉，折不出根的进 `unresolved`；`the_box_predicate_is_a_closed_interval_test_per_axis`）。
      本模块不写树（写点白名单钉照旧只认三处）。
    - 处置：`recompute_dbnum_room_edges(…, library_roots, RootScope)`——`All` 与今天逐字同形（钉 18 全部照旧）；`Targeted { roots,
      panel_statements }` 投影里的按集合筛、回读清单 = 集合 ∖ 投影、`panel_statements` 作 `pack_publication_transactions` 的 **epilogue**
      （起初是 prelude，F1 2026-09-19 改到最后一笔，见钉 32；钉 `a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_last`
      ——`mem://`：A、B 同在 R1，面板搬家只点名 A → A 的边删、ACK 换代，B 的边与 ACK 一字不动，不在册的 777 的出边与 `room_panel_relate`
      入边随最后一笔清掉；再点名 B → 与整库同一终态；`panel_level_statements_clear_unregistered_panels_and_renumber_rooms_only`）。
    - 分派与盖章：`run_room_recompute` 对 `Targeted(delta)` 先 `run_targeted_room_recompute`，其中 `library_generation_roots(` →
      `affected_roots(target.db(), store, dbnum, delta, &library_roots)` → `recompute_dbnum_room_edges(` 顺序固定，`TreeUnavailable` /
      `unresolved` 非空在重算之前 `return None` 退 `run_room_only_recompute`；成功 `Some(RoomRecomputeVerdict::Targeted(generation))`，
      `library_generation()` 对 `WholeLibrary | Targeted` 同样回代号；算不出的根两路同走 `settle_room_recompute_failures`（钉
      `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer`、
      `a_partial_coverage_recompute_keeps_the_watermark_but_never_claims_library_alignment` 加 `Targeted` 一行）。
    live 待补：ams 1112 挪一块 PANE → 该窗任务日志「定向房间重算完成 … 反查 roots=N tree_hits=… tube_hits=… cross_library=…」、只有命中的根
    重写了边、`/dbnums` 的 `room_alignment.current == true`；下一窗不再整库；改一块 PANE 的 DESC → 面板变化为空、由 ① 走整库
    （只改元数据多算一次，D13 ① 早已接受的代价）。
23. D13 追记（U3 b，跨库停放，2026-09-17 晚）：
    - 反查不分库：`durable_tube_hits` 的查询 `FROM tubi_relate WHERE aabb.d != NONE AND (` 且函数体不含 `dbnum =`（钉
      `the_durable_tube_query_is_not_filtered_by_library`）；`resolve` 按 `RefNo::dbno()` 分桶，别的库的命中三路（树 / 投影管身 / 库上管身）
      都进 `foreign`、带祖先链、取不到的留空链（`mem://` 钉 `resolve_joins_tree_projection_and_durable_tubes_into_library_roots` 扩：
      `24382/8`（树，回库取 anc）、`24382/9`（库上管身）、`24382/10`（投影管身）进 `foreign[7998]`，`24383/8` 空链进 `foreign[7999]`）。
    - 停放槽：`park` 同段并条、空条不停、`pending` 不取走、`clear` 按库清、`parked_health` 只列有停放物的库（钉
      `the_parking_slot_merges_the_same_segment_and_clears_per_library`）；`park_foreign_hits(` 在 `run_targeted_room_recompute` 里**只此
      一处**、在折不出根的最后一层分类（`if !still_stuck.is_empty() {`）之后、`recompute_dbnum_room_edges(` 之前、不以结论为条件（起初在
      `Some(uncovered) if uncovered.is_empty() => {` 那一臂里，F1 2026-09-19 提前，见钉 32），`run_room_only_recompute` 里没有
      （钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 扩）。
    - 准入链：`scope_for_task(refresh, library, resident, parked)` 纯函数真值表（钉
      `the_admission_walks_the_parked_chain_from_the_library_generation_to_the_resident`：只停放链 / 停放链 + 自己换代 / 两段接起来按链序 /
      库上已越过的一段不要 / 自己那一段优先于同一对代号的旧停放物；链断两种 / 绕圈 / 从没盖过 / 常驻说不出代号 / 自己那一段面板变化
      为空而链要靠它 / 自己那一段的上一代不在链上 / 槽空却欠着 → 整库；`request_for_task` 接了停放链多一句「别的库的面板变化停放到本库
      N 段：…」）；今天的四条准入照旧（钉 `a_task_goes_targeted_only_when_this_swap_is_the_librarys_whole_debt` 随签名改）。两个执行体
      `refresh_room_reference_before_task` → `durable_alignment` → `parked_for_task(job.dbnum, alignment.as_ref())` → `request_for_task(…, &parked)`
      顺序固定（钉 `the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end` 扩）。
    - 消费：`run_targeted_room_recompute` 里 `library_generation_roots(` → `affected_roots(…, &swap.panels, …)`（`None => AffectedRoots::default()`）
      → `fold_parked(parked, &library_roots)` → `.chain(parked_roots.iter())` 并根 → `recompute_dbnum_room_edges(` 顺序固定，
      `if !parked_unresolved.is_empty() {` 在重算之前 `return None;`，覆盖面对的是并起来的根；`run_room_recompute` 里
      `room_reverse::clear(job.dbnum)` 在 `let verdict = match` 之后、守着 `WholeLibrary(_) | Targeted(_)`，`batch_worker.rs` 里
      `room_reverse::clear(` 只此一处（同一条钉扩）。`fold_parked` 纯函数（钉 `parked_elements_fold_to_the_library_roots_on_their_carried_ancestor_chains`）。
    - `mem://` 两库端到端（钉 `a_panel_move_parks_the_other_librarys_hits_and_its_next_task_recomputes_only_those_roots`）：A 库面板 R1
      搬家 → A 的反查折出 A1、B1 进 `foreign[B]` → A 定向只重写 A1（B 的两条边一字不动）→ 停放 → B 的任务 `Unchanged` + 库上 g1 +
      常驻 g2 + 槽里 g1→g2 → 定向只有停放链 → 折成 B1 → 只重写 B1、B2 的边与 ACK 一字不动 → 与整库重算 B 同一终态 → 清槽；槽空 /
      链断 → 整库。变异一次：`resolve` 不分库（别的库的命中当本库折）→ 该钉与 `resolve_joins_…` 同时红，撤回复绿。
    live 待补：ams 两个 DESI 库都有任务的一窗里挪一块跨库面板 → A 的日志「已为 dbnum=B 停放 N 个元素（g1 → g2）」、B 的日志
    「定向房间重算完成 … 停放链 1 段 N 个元素 → M 根」且 B 不再整库；`/health.room_build.parked` 在两个任务之间非空、之后为 `{}`；
    Q6 那条不带库过滤的管身范围查询的解引用成本（行数 × 库数）。
24. D13 追记（H1，定向路一任务一台，2026-09-19）：
    - `RoomReferenceRefresh::Rebuilt { generation, .. }` 由 `refresh_before_task` 填装上那一代的 `input_version`；`PanelSwap { to_generation }`；
      `RoomRecomputeScope::targeted_generation()` = 链尾那一段的 `to`（自己那一段总在链尾，否则停放链末段）。
    - 准入：`scope_for_task` 取自己那一段时 `to != resident → WholeLibrary`（钉 `a_task_goes_targeted_only_when_this_swap_is_the_librarys_whole_debt`
      加两行：Rebuilt 到 g2 却冻 g3 → 整库；槽里有 g2 → g3 也不替它续；`the_admission_walks_the_parked_chain_…` 加一行：停放 g1→g2 + 自己
      g2→g3 + 冻 g4 → 整库；两条链式用例断言 `targeted_generation()`）。
    - 执行体：`TaskRoomCalculator::generation()`；两个执行体 `refresh → freeze → durable_alignment(job.dbnum, rooms.generation()) →
      parked_for_task → request_for_task` 顺序固定、`room_calculator: rooms.frozen().cloned(),` 随回执、函数体不含
      `resident_room_generation()`；收尾 `let task_rooms = result.room_calculator.clone();` → `run_room_recompute(… task_rooms.as_ref() …)`；
      分派处 `let targeted = match task_rooms {` / `Some(rooms) => {` / `request.scope.targeted_generation(),` / `rooms,`，`None` 退整库，
      分派处与 `run_targeted_room_recompute` 函数体都不含 `frozen_room_calculator()`；`run_targeted_room_recompute` 入口
      `if expected_generation != Some(rooms.resident_generation()) {` 在 `library_generation_roots(` 之前 `return None;`，停放以
      `&swap.to_generation,` 收尾（钉 `the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end` 扩、
      `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 扩）。
    - `mem://` 端到端（钉 `a_mid_task_generation_swap_keeps_the_targeted_recompute_on_the_task_calculator`）：A1 / A2 / A3 各在 R1 三代
      的盒里，库上 g1；T1 对齐 g1 → g2、冻 g2、中途常驻到 g3 → 准入按冻的 g2 定向（按 g3 就整库）、反查 P 命中 A1 / A2 按 g2 算
      （A1 删边、A2 补边、A3 不动）、盖 g2 之后 `library_alignment_owed(g2, g3)` 仍欠（盖 g3 就不欠——反例）；T2 Unchanged + 库上 g2 +
      常驻 g3 + 槽空 → 整库按 g3 → 只剩 R1 → A3，== 从零按 g3。
    - `DataBatchTaskResult::room_calculator` 不进回执（`task_receipt_exposes_the_final_model_verdict_and_caught_up_time` 扩）。
    live 待补：ams 一窗里挪一块 PANE、同时点「显示模型」让 ensure 中途换代 → 该窗日志「定向房间重算完成（… 计算器 <开工那一代> …）」
    或「准入链终点 … 不是本任务冻的计算器代号 … 退整库」，下一窗 ③ 再点一次；`/dbnums` 的 `room_alignment.current` 在两窗之间为 `false`。
25. D13 追记（M1，换代段账本，2026-09-19）：
    - `room_resident::PanelSwap { from_generation, to_generation, panels }`（`room_recompute` 里 `pub(crate) use` 它）；`SwapLedger`
      纯函数：`record` 空段 / 同两代不记、同 `from → to` 替换（位置不变）、满了丢最早的（`DEFAULT_CAPACITY = 64`），`segments` 按记入
      先后，`health` 报 `entries / capacity / oldest_age_ms / segments`（钉 `the_swap_ledger_keeps_the_latest_segments_and_replaces_a_repeated_pair`）；
      `ResidentRooms { swaps: SwapLedger }`，`install()` 里 `PanelDelta::between(` 之后、`slot.current` 换代之后 `self.swaps.record(`，
      段的两端是两代的 `input_version`（钉 `install_reports_which_panels_moved_and_where_their_boxes_are` 扩：两次换代两段、第一代不记、
      第三代那一段 == install 回的 `panels`）；`room_resident::swap_segments()` 是取数口。
    - `scope_for_task(library, resident, swaps, parked)`（不再收 `refresh`）深度优先 `walk_chain`，同一起点停放段先于账本段；
      `RoomRecomputeScope::Targeted { swaps, parked, generation }`，`targeted_generation()` 回 `generation`（钉
      `a_task_goes_targeted_only_when_the_ledger_explains_the_librarys_whole_debt`：库上更早 / 从没盖过 / 冻不出 / 空段 / 账本空 /
      段不从库上起 / 账本没有后一段 → 整库；账本两段接起来 → 两段定向；`Unchanged` 的任务沿别人记的段定向；`request_for_task` 多一句
      「沿换代段账本接了 N 段」；`the_admission_walks_the_parked_chain_from_the_library_generation_to_the_resident`：停放段优先、
      一条路断了换另一条、绕圈 / 链断 / 空段 → 整库）。
    - 执行体：`parked_for_task` 之后 `room_recompute::swaps_for_task(alignment.as_ref())`、`request_for_task(…, &swaps, &parked)`
      （钉 `the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end` 扩）；`run_targeted_room_recompute` 收
      `swaps: &[PanelSwap]`，`for swap in swaps {` 各反查、`for (swap, foreign) in swaps.iter().zip(&foreign_by_swap) {` 按段停放，
      面板级语句按链序拼（钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 扩）。
    - `mem://`（钉 `a_mid_task_generation_swap_keeps_the_targeted_recompute_on_the_task_calculator` 扩 ④）：T2 `Unchanged`、库上 g2、
      冻 g3、槽空、账本有 ensure 记的 g2 → g3 → 沿账本定向、反查 Q 命中 A2 / A3、按 g3 算 → 与整库按 g3 同一终态；账本空才整库。
    live 待补：ams 两个 DESI 库、UI 先点「显示模型」让 ensure 做那一次重建 → 两个库的任务日志都是「定向房间重算完成（… 换代段 1 段 …）」、
    `/health.room_build.resident.swap_ledger.entries ≥ 1`，没有一个库整库；没被命中的库日志「换代段 1 段 … 反查 roots=0 …」。
26. D13 追记（M2，折不出根的元素按在 T 上还在不在分两半，2026-09-19）：
    - `window_root_plan::missing_at_target(dbnum, target_sesno, refnos)`：`E3dModelService::from_current()` → `build_set(dbnum, Some(T))` →
      逐个 `element_exists`，回不存在的那几个；空输入不开文件。
    - `batch_worker::split_unresolved_by_existence(candidates, gone) -> (stuck, ignored)` 纯函数：按候选序、去重（钉
      `unresolved_hits_that_no_longer_exist_at_the_target_are_ignored_and_the_rest_block_the_targeted_path`）。
    - `run_targeted_room_recompute` 函数体：`fold_parked(` 之后把 `affected.unresolved` 与 `parked_unresolved` 合成 `unresolved`，
      `window_root_plan::missing_at_target(` → `split_unresolved_by_existence(&unresolved, &gone)` → `if !stuck.is_empty() {` 才
      `return None;`，三处都在 `recompute_dbnum_room_edges(` 之前；函数体不再含 `if !affected.unresolved.is_empty() {` /
      `if !parked_unresolved.is_empty() {`（同一条钉 + `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer`
      的退整库清单改口）。
    live 待补：ams 一窗里删一个根 + 挪一块面板（面板盒扫得到那个根的构件）→ 该窗日志「N 个命中的元素在 DbSet@T 上已不存在 … 忽略」
    且仍「定向房间重算完成」，不再退整库。
27. D13 追记（L1，保护集成员带着上一次可用的盒，2026-09-19；**同日随 L1-P 退役**——下面几条只作历史，`protected_bounds` /
    `last_known_bounds` / `inherit_protected_bounds` 已不存在，`PanelDelta::between` 的旧盒取旧代 `current_bounds`、只给日志看，
    钉 `a_panel_that_regains_geometry_…` 改名为 `…_is_looked_up_by_its_new_box_and_its_durable_members`，见钉 30）：
    - `ResidentRoomModel::protected_bounds`、`last_known_bounds(panel)`（有几何 = 此刻的盒，保护中 = 带着的盒，其余 `None`）、
      `inherit_protected_bounds(previous)`（只给 `missing_panels` 里的带）；`ResidentRooms::install(mut model)` 里
      `model.inherit_protected_bounds(old)` 在 `PanelDelta::between(` 之前；`PanelDelta::between` 两处旧盒都走 `old.last_known_bounds(`，
      新盒只认 `new.panels`。
    - 四代用例（钉 `a_panel_that_regains_geometry_after_protection_brings_its_last_usable_box_as_the_old_one`）：8 有几何 [20, 21] →
      不可用（进保护集，带着 [20, 21]，这一段面板变化为空、不进账本）→ 仍不可用（盒继续带）→ 有几何且挪到 [40, 41] → 那一段
      `recalc = [8: old [20, 21] → new [40, 41]]`、`query_boxes` 两个、账本里那一段同样两个盒；重新有几何之后 `protected_bounds` 空。
      反例：从没可用过的面板有了几何，旧盒仍 `None`。
    live 待补：ams 一块 PANE 先让它的 `.mesh` 读不出（进保护集）、再恢复并挪位 → 该窗日志「定向房间重算完成 … recalc=1 …」且反查
    `tree_hits` 覆盖旧位置里的成员（它们的陈旧边被删）。
28. D13 追记（L3，定向成功只清消费过的段，2026-09-19）：
    - `room_reverse::clear_consumed(dbnum, consumed) -> (撤掉, 剩下)`：按 `from → to` 对条、按元素去、去空整条撤、槽空撤键；空快照不动槽
      （钉 `clearing_the_consumed_segments_keeps_what_was_parked_while_the_task_ran`：快照两段、任务中并进一个元素 + 停进一段 → 收尾
      撤 1 剩 2、并进来的元素与新段都在、`parked_by` 不改；再清一次 0；剩下的各自消费后槽空、`/health` 里没有它）。
    - `run_room_recompute` 分派处 `match (&verdict, &request.scope) {`：`(RoomRecomputeVerdict::Targeted(_), RoomRecomputeScope::Targeted { parked, .. }) => {`
      走 `room_reverse::clear_consumed(job.dbnum, parked)`，`(RoomRecomputeVerdict::WholeLibrary(_) | RoomRecomputeVerdict::Targeted(_), _) => {`
      走 `room_reverse::clear(job.dbnum)`，两处都在 `let verdict = match` 之后；`batch_worker.rs` 里 `room_reverse::clear(` 与
      `room_reverse::clear_consumed(` 各只此一处（钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 改口）。
      `mem://` 两库端到端里 B 收尾改按快照 `clear_consumed(LIB_B, &parked) == (1, 0)`（钉 `a_panel_move_parks_…` 改口）。
29. D13 追记（L2，只加量具，2026-09-19）：
    - `AffectedRoots { durable_tube_ms, durable_ancestor_lookups, durable_ancestor_ms }`、`durable_timing()`；`resolve` 里 `durable_tube_hits(`
      与 `durable_ancestors(` 各计一次时；`summary()` 一字不改（钉 `resolve_joins_tree_projection_and_durable_tubes_into_library_roots` 扩：
      四个回库取 anc、`durable_timing()` 的形状）。
    - `run_targeted_room_recompute` 按段累加三格，「定向房间重算完成」那一行 `反查 {}（{}）` 第二格是 `affected.durable_timing()`。
    live 待补：7997 那种库上一次定向的「库上管身范围查 Nms」——N 明显压过 `durable_read_ms` / 总耗时才做 `aabb_d` 平副本（并决定回填还是分两半查）。
30. D13 追记（L1-P，离开的成员从库上按面板取，2026-09-19；取代同日 `2ba772b25` 的 L1-R 门——`unknown_bounds` / `old_box_unknown` /
    `fully_known` 已撤，`walk_chain` 的过滤回到 `!swap.panels.is_empty()`）：
    - `PanelDelta::leaving_panels()` = `recalc` 里每一块 ∪ `removed`，按面板 refno 序去重（`renumbered` 不进）；`query_boxes()` 只回
      `recalc` 的新盒（`filter_map(|entry| entry.new)`；第一步 `1e7dbfd91` 时还是旧盒 ∪ 新盒，第二步拿掉）。`ResidentRoomModel` 上没有
      `protected_bounds`；`ResidentRooms::install(model)` 不再 `inherit_*`；`PanelDelta::between` 两处 `old` 都是 `old.current_bounds(`。
      钉 `a_panel_that_regains_geometry_after_protection_is_looked_up_by_its_new_box_and_its_durable_members`：8 [20, 21] → 保护
      （`current_bounds(8) == None`、这一段空）→ 仍保护（7 挪了，`query_boxes == [2.0]`）→ 8 恢复到 [40, 41]：`recalc = [8: None → [40, 41]]`、
      `query_boxes == [40.0]`、`leaving_panels == [8]`、账本末段一个盒；从没可用过的 8 有几何同形。钉 `install_reports_…` 第二代
      `query_boxes == [5.0, 20.0]`（7 的旧盒 [0, 1] 不问）。
    - `room_reverse::durable_panel_members(db, panels) -> BTreeSet<打包 refno>`：每块一句 `SELECT VALUE out FROM {panel}->room_relate;`；
      `affected_roots` 先 `ensure_spatial_ready`、再 `leaving_panels` → `durable_panel_members`（计时进 `durable_member_lookups / durable_member_ms`），
      盒与成员都空才早退；树只在盒非空时读。`resolve(db, store, dbnum, boxes, tree_hits, member_hits, library_roots)` 多一个参数：投影那一遍
      `tube_hit || tree_hits.contains || member_hits.contains` 都带祖先链；回库取 `anc` 的 `unknown` = 树上 ∪ 成员里投影没有的（去重）；
      分库折根的 `hits` = 树 ∪ 管身 ∪ 成员。`durable_ancestors` 第二遍：`inst_relate` 没有行的按 `in` 到 `tubi_relate`（`WHERE in IN [...]`，
      `idx_tubi_relate_in`），同一来源多行只算一次。`AffectedRoots { member_hits, durable_member_lookups, durable_member_ms }`；
      `summary()` 形状 `roots= tree_hits= tube_hits= member_hits= cross_library= foreign_libraries= unresolved=`；`durable_timing()` 三段。
    - 钉 `resolve_joins_tree_projection_and_durable_tubes_into_library_roots`：面板 7 名下四条成员边（投影里的 100、`inst_relate` 带 anc 的 310、
      盒远在 [50, 51]、只有 `tubi_relate` 两行的管身来源 500、别的库的 24382/20）→ `member_hits = 4`、根仍 A/B/C/D、`durable_ancestor_lookups = 7`、
      foreign[7998] 多 24382/20、`cross_library = 5`；只给成员不给树与盒 → 根 A/C/D、foreign 只有 24382/20、`unresolved` 空。
      `mem://` 端到端（钉 `a_panel_that_regains_geometry_elsewhere_drops_its_stale_members_through_the_durable_edges`）：R1 收着 A1、
      恢复几何挪到 x 2000–3000、`PanelMove { old: None, new }`——只问新盒一根都命中不到（此前的漏算形状），带成员 `member_hits = 1`
      → 只重算 A1 → 陈旧边删掉、与整库同一终态。`mem://` 两库端到端（钉 `a_panel_move_parks_…`）：`durable_panel_members(R1) ==
      树的答案模拟 {A1, B1}`，`member_hits = 2`；
      钉 `a_mid_task_generation_swap_…` 两段各自 `{A1}` / `{A2}`。钉 `install_reports_which_panels_moved_…`：第二代 `leaving_panels = [7, 8]`、
      第三代（9 不在册）`= [9]`；钉 `a_panel_that_regains_geometry_…` 第四代 `= [8]`。
    live 待补：ams 一块 PANE `.mesh` 读不出 → 重启服务 → 恢复 `.mesh` 并挪位 → 该窗日志「定向房间重算完成 … member_hits=N …」且旧位置
    成员的陈旧边被删（不再靠旧盒）；同窗把一块面板移出关键字（不在册）→ 它名下成员的根出现在 recalc 的根里、边落到别的房间。
31. D13 追记（M2-S，还在 T 上却折不出根的再分一层，2026-09-19）：
    - `room_reverse::fold_to_roots` 第二个返回值 `Vec<(i64, Vec<i64>)>`（元素 + 祖先链）；`AffectedRoots::unresolved_ancestors: BTreeMap<RefnoEnum, Vec<RefnoEnum>>`
      与 `unresolved` 同一批；`fold_parked` 第二个返回值 `Vec<(RefnoEnum, Vec<RefnoEnum>)>`。
    - `batch_worker::split_stuck_by_ancestry(stuck, ancestors, gone_ancestors) -> (still_stuck, out_of_scope)`：链非空且每一级都不在 `gone` 里
      → `out_of_scope`，其余（链空 / 表里没有 / 有一级在 `gone`）→ `still_stuck`；顺序照 `stuck`。
    - `run_targeted_room_recompute` 顺序：`if !stuck.is_empty() {` → `window_root_plan::missing_at_target(`（问链上每一级）→
      `split_stuck_by_ancestry(&stuck, &unresolved_ancestors, &gone_ancestors)` → `out_of_scope` 非空 stdout 点名 → `if !still_stuck.is_empty() {`
      才 `return None;`（`stuck` 非空本身不再退整库；问不出来照旧退）。钉 `stuck_hits_whose_whole_ancestor_chain_still_exists_are_out_of_scope_and_ignored`
      （四例 + 顺序钉）；钉 `unresolved_hits_that_no_longer_exist_…` 与 `the_targeted_recompute_falls_back_…` 的 `if !stuck.is_empty() {` →
      `return None;` 钉照旧成立（`return None;` 在块内）。
    live 待补：盒扫得到 `model_type_scope` 之外的历史几何时，该窗日志「… 祖先链每一级也都在，却没有一级是本库的生成根 … 忽略」且仍
    「定向房间重算完成」。
32. D13 追记（F1 / F2，定向失败不丢证据 + `recalc` 面板自己的拓扑边，2026-09-19）：
    - `recompute_dbnum_room_edges` 函数体里 `pack_publication_transactions("", &pieces, panel_statements, false)`——面板级语句作 epilogue 进
      最后一笔、prelude 为空；`RoomEdgeRecompute.panel_statements` 计数不变。
    - `run_targeted_room_recompute` 函数体里 `park_foreign_hits(` 只此一处，落在 `if !still_stuck.is_empty() {` 之后、`recompute_dbnum_room_edges(`
      之前、`Some(uncovered) if uncovered.is_empty() => {` 之前（不以结论为条件）；`run_room_only_recompute` 里没有。
    - `MemoryRoomCalculator::render_panel_topology_statements(panel)`；`model_db_adapter::render_recalc_panel_topology_statements(rooms, panels)`
      按面板 refno 序去重、在册先删后插、不在册只删、空输入空串；`run_targeted_room_recompute` 里
      `render_recalc_panel_topology_statements(` 以 `swap.panels.recalc.iter().map(|entry| entry.panel)` 喂、拼在各段
      `render_panel_delta_statements(&swap.panels)` 之后；`render_panel_delta_statements` 本身仍不含 `recalc`。
    - 钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 改口（`classified < park < recompute`、
      `park < decided`、epilogue 字面）；`mem://` 钉 `a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_last`
      （改名）、新 `recalc_panel_topology_statements_follow_the_calculators_registered_view`（纯函数五例）、新
      `a_targeted_recompute_writes_the_recalc_panels_own_topology_edge`（R2 新在册、只点名成员 A → 陈旧 999 → R2 删掉、601 → R2 写上、
      别的面板的现行边不动、成员边照常；再跑一遍幂等）。
    live 待补：把一间 FRMW 改名进关键字（不动它的面板）→ 该窗「定向房间重算完成 … 面板级语句 N」且 `SELECT * FROM pe:⟨P⟩<-room_panel_relate`
    对每块新在册面板各一条、房间号对；定向中途写库失败一次（收尾段停一下 surreal 让某笔事务失败）→ 下一窗同一段重走仍
    「定向房间重算完成 … member_hits=N」、不在册面板名下成员落到别的房间。
33. D13 追记（F3，ACK 代号不在链上的根点名重算 + 盖章时对齐链上 ACK + 读侧一票否决，2026-09-19）：
    - `room_recompute::AckedRoot { root, room_generation }`、`acked_roots(store, dbnum)`（只收 `ack.matches(&receipt)` 的；`room_membership`
      为假记 `None`）、`acked_roots_for_task(dbnum, alignment)`（`alignment` 为 `None` 回空）；`RoomRecomputeScope::Targeted { stale_roots }`；
      `scope_for_task(library, resident, swaps, parked, acked)`：库上 == 常驻 → `stale_acked_roots(acked, {resident})` 非空则空段定向、
      空则整库；链接得通 → `on_chain` = 库上那一代 ∪ 各段 `from` / `to` ∪ 链尾，`stale_roots = stale_acked_roots(acked, &on_chain)`；
      `request_for_task(…, acked)`：`library_current && !reasons.is_empty()` → 整库，否则 `scope_for_task`；F3 那一句在账本 / 停放两句
      之前；`if reasons.is_empty() { return None; }` 在范围定了**之后**（F3 单独触发）。两个执行体 `swaps_for_task` 之后
      `room_recompute::acked_roots_for_task(job.dbnum, alignment.as_ref())`、以 `&acked,` 传进 `request_for_task`（钉
      `the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end` 扩：`swaps < acked < request`）。
    - `run_room_recompute` 分派 `RoomRecomputeScope::Targeted { swaps, parked, stale_roots, .. }` → `run_targeted_room_recompute(…, stale_roots, …)`，
      函数体里 `.chain(stale_roots.iter())` 并根；`align_room_acks_along_chain(store, dbnum, &on_chain, &generation)` 只此一处、落在
      `Some(uncovered) if uncovered.is_empty() => {` 之后、`Some(RoomRecomputeVerdict::Targeted(generation))` 之前；`run_room_only_recompute`
      里没有（钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 扩）。
    - `model_db_adapter::align_room_acks_along_chain`：`room_membership` 为假 / 代号 `None` / 代号 == 链尾 / 代号不在 `on_chain` 都不动，
      其余只改代号一格、`record_publication_ack` 拒收不计；`recompute_dbnum_room_edges` 对 `records.is_empty()` 的投影根进 `empty_projected`、
      与 `recomputed` 一起对齐 ACK（`acks_refreshed` 计入）。
    - `query_service::ProcessAck { Missing, Current, Stale }`；`room_edges_ready(ack, chasing_here, aligned_on_store)`：`Current` → true、
      `Stale` → false、`Missing` → `!chasing_here && aligned_on_store`；`room_calculation_status` 函数体里三态字面都在（钉
      `the_durable_alignment_answers_for_roots_this_process_never_published` 扩四例、
      `the_room_status_reads_both_the_process_ack_and_the_durable_alignment` 扩）。
    - 钉 `a_stale_ack_in_an_aligned_library_is_the_debt_itself_and_goes_targeted_on_its_own`（纯函数：库上同代两根点名 / 都对得上整库 /
      链上 g1、g2 不点、gx 与 None 点 / 停放段两端算在链上 / 链断不看 / `request_for_task` F3 单独触发、① / ② 同代仍整库、链接得通三句、
      常驻说不出不看）；`mem://` 钉 `a_root_generated_in_a_transient_generation_is_repaired_when_the_resident_returns`（库上 g1 发 A1 →
      ensure 按 g2 发 A2 与空根 A3 → 回 g1：`acked_roots` 读出 g2 两根 → 空段定向只点名它们 → 按 g1 重算，A2 的陈旧边删掉、A1 不动、A3
      什么都不写但 ACK 对齐（`acks_refreshed = 2`）→ 再问不欠 → == 整库按 g1；`align_room_acks_along_chain` 链上 g1 → g2、链外 gx 与
      没算房间的不动、再来一遍 0）。
    live 待补：ams 让一块 PANE 的 `.mesh` 暂时读不出 → 点「显示模型」让 ensure 按那一代生成几根 → 恢复 `.mesh` → 下一窗日志
    「定向房间重算完成 … 换代段 0 段 无（库上已与常驻同代，欠账只有点名的根——空段定向）… ACK 代号不在链上点名 N 根」、之后
    `e3d.room.lookup` 对那几根回 `ready_*`，两窗之间对它们回 `not_computed`（此前是错的 `ready`）；一次普通定向之后日志
    「链上 K 代、没命中的 M 根 ACK 房间代号一并对齐到 …」且下一窗**不**再把整个投影点名一遍。
34. D13 追记（F3 落库，`model_room_stray_generations`，2026-09-19）：
    - `DbnumState.model_room_stray_generations: Vec<String>`（`#[serde(default)]`，行结构 `Option<Vec<String>>`，`MODEL_WATERMARK_COLUMNS`
      末尾多这一列，两处装配 `unwrap_or_default`）。
    - `task_end::render_watermark_update`：`RoomGenerationStamp::Aligned(_)` 与 `Cleared` 两臂各多一句 `model_room_stray_generations = []`，
      `Untouched` 不碰；说不了整库的任务渲出的 SQL 不含 `model_room_stray_generations`（钉 `the_room_generation_stamp_follows_the_model_watermark`
      扩：种子 `['g2']`，普通窗口原样、`Aligned` / `Cleared` 清空；`the_tail_is_one_transaction_and_stamps_the_model_watermark_under_the_data_gate`
      多一句 `!sql.contains("model_room_stray_generations")`）。
    - `model_db_adapter`：`room_alignment_row_on(db, dbnum)`（生产库走 `DbnumState::read` 镜像、旁路连接点读）；
      `stray_room_generation_statement(db, dbnum, generation)`——没有行 / 库上 `None` / 与库上同代 / 已含 → 空串，否则
      `UPDATE dbnum_watermark:⟨dbnum⟩ SET model_room_stray_generations = array::union(model_room_stray_generations ?? [], ['g'])`（`+` 不行：
      这一版 SurrealDB 不给数组做加法）；`note_stray_generation_written` 提交成功后 `invalidate_mirrored_on`。`persist_roots_batched_unacked_impl`
      在分组之前 `stray_room_generation_statement(db, dbnum, rooms.resident_generation())` 算一次，`persist_group(db, &live, &stray_epilogue)` /
      二分重试 / 多写者三处都带、各自成功之后 `note_stray_generation_written`；`persist_group(db, group, epilogue)` 里
      `let epilogue = if pieces.is_empty() { "" } else { epilogue };` → `pack_publication_transactions(&prelude, &pieces, epilogue, touches_geometry)`，
      不再有 `""` 那一档（钉 `room_membership_is_computed_per_root_before_the_write_and_rides_the_same_transaction` 改口）。
    - `room_recompute::DurableAlignment { stray }`、`stray_generations_owed(library, stray)`（去掉与库上同代的；非空 = 欠）、`owed()` 两句
      `or_else`；`durable_alignment` 从同一次 `DbnumState::read` 取 `model_room_stray_generations`（钉
      `the_library_owes_a_recompute_when_the_watermark_remembers_writes_at_another_generation`：同代不算 / 另一代欠 / ③ 句在前 /
      重启后 `request_for_task` 整库只此一句 / 进程内有 ACK 空段定向两句）。
    - `manual_update::RoomAlignmentColumns.stray_generations`（`skip_serializing_if = "Vec::is_empty"`），与两枚水位同一趟读回（钉
      `the_dbnum_row_carries_the_durable_room_alignment_as_one_optional_cell` 扩：空不出、非空多一格）。
    - `mem://` 钉 `a_stray_generation_survives_a_restart_on_the_watermark_row_and_rearms_the_recompute`：没有行 / 没盖过 / 同代都不写；
      按 g1 发 A1 那一格不动；按 g2 刷 A2 与它的行同一笔事务记下 `[g2]`、`failed` 为空、已含不再写；换一个空 `ModelMemoryStore` 模拟重启
      → `DurableAlignment { g1, g1, [g2] }.owed()` 含「F3 落库」→ `request_for_task` 整库 → 两根都回读库上几何 → A2 的陈旧边删掉；
      源码钉 epilogue / 三处 `note_stray_generation_written`。
    - `publication_error_retains_the_cause_after_rollback_errors`（`35e022318`）：`BEGIN … COMMIT` 里一句报错——`THROW` 或表达式算不出（数组 `+`）——
      整笔零提交，`SELECT * FROM publication_probe` 为空。「随本组的行同一笔事务」的前提；memory 与 rocksdb 同一台执行器，`bin/surreal.exe`
      两边核过（D13 收口总表第 5 行）。
    live 待补：`.mesh` 暂时读不出 → 点「显示模型」→ 恢复 → **重启服务** → 下一窗日志「库上记着盖章之后本库有根按另 1 代写过边 …
    欠一次，F3 落库」+「整库房间重算完成」、`/dbnums.room_alignment.stray_generations` 在两窗之间非空、之后消失；稳态下
    `SELECT model_room_stray_generations FROM dbnum_watermark` 恒空。

## 背景

ADR-065 把「模型 + 房间在 dbnum 任务里算完」定为口径，ADR-067 为它造了常驻房间模型（两代换代、库侧参照、启动对比、六态生命周期），
ADR-071 / ADR-072 又把任务改成同步写库带写入标记、中断按 `(dbnum, sesno > S)` 回退。到 2026-09-10 晚，房间这一面仍是：

- **任务末一段整库计算**：`calculate_and_commit_memory_rooms` 按整个 dbnum 重算全部构件与面板的归属并原子发布——7997 只生成 2 根
  也要重发近六万条边；它要求常驻模型「有当前代」，建不起来就扣住本任务全部根（ADR-067 D6）。
- **第三枚水位**：`room_sesno` 及同族五列（`room_sesno_time` / `room_settled_at` / `room_input_version` / `room_required_input` /
  `room_result_version`）、`RoomInputVersion`（为取一个哈希把整库记录序列化成 JSON）、`RecoveryStage::Room`、取数源路由的
  `room_lagging` 理由——全是为「房间是独立阶段」服务的控制面。
- **结构触发与常驻换代**：FRMW/SBFR 改名、PANE 换属主、CWALL/CFLOOR 搬迁三条结构触发（ADR-010 D12 / ADR-040 D4）不改任何几何，
  几何路点不着，于是要一套「先换代常驻模型、再跨库重算受影响范围」的分流（d-429 的硬约束）。
- **按 sesno 回退**：ADR-072 D2 的回退算法八步——不删 `pe` 顶点（d-570）、卫星行要带 `dbnum + sesno`、谓词索引量测
  （`docs/evidence/2026-09-10-rollback-predicate-index-measurement.md`）、跨库副作用不回退、回退后短暂高于水位的残行由
  `DELETE …/data/above/{watermark}` 人工兜底——为一个异常事件维护了一整套细粒度机器。

用户的判断（2026-09-10）：房间只保留几何与筛选算法，流程整套删除；写入没写完就整个 dbnum 清掉重新初始化，不要按 sesno 做部分回退；
现在不要把思路搞得太复杂。

## 决策

### D1　房间随根生成，不再是一条流程

一个生成根的完成 = **生成最终几何 → 算它的房间归属（模型计算之后，用当前候选面板做点包含筛选）→ 模型行与房间边在同一笔事务写入**。
房间归属是根产出的一部分，与 `inst_relate` / `tubi_relate` 行同块同事务落库（合块允许跨根，一根不拆）。任务末**没有**整库房间计算，
任务收尾**没有**房间段；没有 `RoomRecalcPanel` / `RoomRecalcElement` 目标、没有房间轮、没有发布视图 / 结果目录工件 /
常驻模型换代 / 参照落款 / 结构触发。`RoomOnly` 只作为生成路径上的**内部**意图存在（一次只算房间不重生几何的根级工作），不是对外契约。

**2026-09-17 追记（B）：「模型计算之后」落在 bake 那一刻，边随根回执走，flush 只渲染。** 此前逐根归属在 write-behind
flush 时算（`model_db_adapter::room_stage`：每次 flush 冻一台计算器、拿投影记录回读盘上 `.mesh` 取判定点）。现在
整根重生成的生成 worker 在 `e3d_model_service::bake_element` 里、网格还在手上时就按元素算（`MemoryRoomCalculator::edges_for_record`），
判定点取自**就要写盘的那份网格** × 同一个 f32 变换（`room_model::world_points_of`，与回读路同一个函数——不是为算盒子折出来的
f64 `world_mesh`，那会差几个 ulp、让贴面板边的点翻面，破 ADR-010 §9「增量 == 全量逐边相等」）；输入配方 `baked_room_input`
与投影记录同三个函数（`geometry_source_refno` / `geometry_record_id` / `persisted_world_transform`）加 `projected_mesh_to_world`
（标准图元 = 世界 × 本地；**标准隐式管身不再乘本地**——`persisted_world_transform` 已把它折进存下的世界变换，库上 `tubi_relate`
行回读的实例变换也是单位阵；此前投影记录与 bake 两侧一致地多乘了一次，跨面板边的标准管身判定点落在网格之外、与回读路算的边对不上，
2026-09-17 修，钉 `a_canonical_implied_tube_has_the_same_mesh_to_world_on_the_projected_and_durable_paths`）。
发布前按根收拢（`merge_root_edges`）成 `RootReceipt.room = CarriedRoomEdges { 代号, 构件集, 边 }` 随投影与 `model_artifact` 落盘；
flush 的 `room_stage` 过三道闸（代号 == 此刻计算器代号、构件集 == 本根投影记录的构件集、计算器在）就只 `render_root_room_edges`，
任一不过（刚体前移 / 单元级局部发布 / 旧产物 / 中途换代 / 嵌套根共享记录）照旧回读现算——两条路渲出的语句逐字相同，硬验收不多一种
写点形状。算不出 = 这件几何 `Err` → 生成 worker 记这一根死信、不发布（D2 前移到 bake，文案带「room membership」）。
`BatchedPersist.room_carried / room_recomputed` 与 `[model-persist]` / `[model-write-behind]` 日志说这一批边是带来的还是现算的；
稳态下 `recomputed` 应为 0。不动的：D13 `RoomOnly` 整库重算、`model/rebuild`、`/model/ensure`、初始化双 lane、刚体路、单元路——
全走 flush 兜底。

**为什么不是别的做法**：
- 不保留「任务末按库整段重算」（ADR-065 D8 / ADR-067）：它的成本是 O(整库) 而不是 O(本窗变更)，且必须有一枚房间水位与一份
  常驻当前代才能表达「算到哪」「用的哪份面板」——D2 要退的东西全是它的附属物。
- 不保留独立房间队列 / 房间轮（ADR-010 §7、ADR-011 §8 / §10）：那是 ADR-065 已经否掉的第三份「欠什么」的账；房间进了根的完成态之后，
  `gen_root` 的安定与否就替房间说话（d-429 与 d-427 共享的前提：「完成」由根级事实表达）。
- 不做 d-429 记的「结构触发窗口先换代常驻模型再跨库重算」分流：它保住的是常驻换代与结构触发两套机制，而用户拍的是两套都不要。
  结构变化在逐根路径上的传播是本 ADR 明确留下的空白（§未决 U1），不用旧机制填。

### D2　房间失败 = 该根失败；三枚水位退成两枚

某根的房间归属算不出来或写不进去 = **该根本轮未完成**：`gen_root` 非安定、凭证不前移、`model_sesno` 不越过、失败记录写房间步骤与原因；
不排队、不自动重试（ADR-065 D4 / G2 (i) 原样）。`model_sesno` 的承诺从此**包含房间**：「这个库到 T 为止数据与模型一致」里的模型
含房间边。库上只剩 `applied_sesno` 与 `model_sesno` 两枚水位；`room_sesno` 及同族五列、`RoomInputVersion`、`RecoveryStage::Room`、
取数源路由的 `room_lagging`、`/dbnums` 的房间格全退。

**为什么不是别的做法**：
- 不留第三枚水位（ADR-071 D4「`room_sesno` 房间成功才推」）：一枚水位量的是一个阶段的推进，阶段没了水位就是无主的格——
  它只会在「模型到了 T、房间没盖上」这种新口径下不可能出现的状态里亮灯。
- 不把房间失败降成告警、让根照样安定：那样 `model_sesno == applied_sesno` 说的一致就不含房间，材料表读到的房间号可能陈旧而
  仪表全绿——ADR-010 §9「增量 == 全量」的硬验收在这一种失败形态上就无人守。
- 不整枚扣住 `model_sesno`（「任一根房间失败则整库水位不动」）：ADR-065 实施进度里已经论证过，一次持久的房间失败会把整库模型永远钉在旧版重算；
  按根扣是既有口径。

### D3　恢复：任何未完成写入 → 整库 Reinitialize

写入标记（ADR-072 D1）保留，它回答的仍是「这个库有没有一次没写完的任务」。**处置只有一种**：启动水化段或任务开工前见到
`dbnum_write_state.status = writing`（五类意图 `initialize / apply_window / model_only / room_only / reinitialize` 一律），
**本批就地转 Reinitialize**——改本批 `intent` 为 `Reinitialize` 并带 `reinit_requested`，走 `execute_one_dbnum` 冻结点的
`wipe_reinit`（`wipe_dbnum_for_reinit` 整库清空：数据、模型、房间边、`gen_root`、两枚水位归零、`state_revision + 1`、spatial epoch 递增），
再按首次导入重新解析当前文件 → 整库模型（含逐根房间）。不再按 `(dbnum, sesno > S)` 谓词删除、不再排 S→T 重放。

进程内连带（R0 实位，实施时必补）：整库 wipe 之后必须作废该库的水位镜像、`GeometryBaseline`、一致性普查缓存，清空模型投影
（`restore_dbnum` 一侧），空间树置脏——原 `rollback_to_watermark` 的 wipe 分支提前 return，这几句一句没跑到。

**边界**：正常收尾的 `Partial`（某一面写库失败，但任务走到了 `task_end`、标记已置 `finished`）**不**触发重建，照 ADR-071 D3：
该面水位不推进、留痕、内存照常服务、下一次该库任务从库水位重做。「未完成写入」专指 `writing` 残留。

**为什么不是别的做法**：
- 不按 sesno 回退到水位（ADR-072 D2）：它要求 noun 行 / `ATT_UDA` 行带 `dbnum + sesno`、要为 `pe` 顶点开级联例外（d-570）、
  要量谓词索引、要接受回退后短暂高于水位的残行并留一个人工 `DELETE …/data/above` 兜底、跨库副作用还得单独论证幂等——
  这是 ADR-021 §2 拒绝过的「缝合式对齐」在恢复路上的重现。整库重建之后库就是当前文件的函数，没有两段历史要缝。
  回退本身是异常事件，正确性与简单性优先于代价（ADR-021 §2 同一论证）。
- 不「本批失败再另排一条 Reinitialize」：清库钉在 worker 执行体里（ADR-021 §3，源码钉禁止入队路径含 `wipe_`），就地转意图不破这条纪律，
  也不多一次排队往返（d-747 第 1 条）。
- 不分面回退（只清崩掉的那一面）：ADR-072 D3 已否——面与面之间的一致性正是水位要承诺的东西。
- 不把 `Partial` 也算成未完成写入：`Partial` 是任务**走完**后的诚实结果，欠什么两枚水位说得清、下一窗补得上；
  为它付一次整库重建是把「异常恢复」误用到「正常失败」上。

### D4　库层房间资源不动

`room_relate` / `room_panel_relate` 两张边表保留，写者是 D1 的逐根生成路径。SurrealDB 侧的房间函数与资源文件**一律不动**：
`fn::room_code`（hd / hh 两版）、`fn::room_relate_of`、`fn::room_num_of`、`resource/surreal/get_room_nodes.surql`
及 `embedded_surql` 的灌入清单；`src/api/room_code.rs`（走 MySQL 的老路）与 `src/plug_in/room_setting.rs` 不动。
查询工具 `e3d.room.lookup` **保留**（plant-ui 的 `q room` 在用）；它原先回的 `published_version` 就是 `room_sesno`
（ADR-072 D4），D2 之后房间承诺并入模型水位，该格随实施对齐到 `model_sesno`，`stale` 照旧 = 该库有任务在飞。

**为什么不是别的做法**：
- 不删表删函数：20 多个材料表 surql 经 `fn::room_code` / `fn::get_room_number` 读这两张边（ADR-010 背景），它们是 DB 层的定义，
  不报错也不拦任何东西；表有新写者，不是死表。删它们动的是 DB 资源与 MySQL 老路，与「删房间流程」不是一件事（d-757）。
- 不给查询工具换名或换形状：读口的消费者在别的仓，本轮不动跨仓契约。

### D5　运维端：`DELETE …/data/above/{watermark}` 退役，GET 半边保留

`DELETE /api/v1/dbnums/{dbnum}/data/above/{watermark}`（按水位删残留，`prune_above_watermark`）退役——它是 D3 否掉的那种
部分裁剪的人工形态，替代是 `POST /api/v1/dbnums/{dbnum}/data/reinitialize`（排一条整库重建批次，清库在 worker 冻结点做，
`docs/plans/2026-09-10-recompute-contracts-rooms-rebuild-and-data-reinitialize.md` §4.1）。**GET 半边保留**：它只 `SELECT`，
是清库之后自证「真的干净了」的唯一现成只读工具，删它没有替代。`specs/002-watermark-data-backing/spec.md` FR-012 相应改为
只约束 GET（FR-012a 已按 d-747 补入）。`DELETE /api/v1/dbnums/{dbnum}/data`（整库快删）照旧。

**为什么不是别的做法**：
- 不留 DELETE 当兜底：它服务的「回退后残行」这一状态不再产生；留着就是给「部分裁剪」开门，而部分裁剪不满足 ADR-021 §4
  Ref0 区间快删的整库前提。
- 不连 GET 一起删：清库后要对着库看一眼「水位之上还有没有行」，没有第二个只读口能答这个问题。

### D6　调试阶段不做旧库迁移；退役表只在开发库清理

新形态从零建库；已有的旧库不写迁移脚本、不做兼容读旧列——需要的话整库 Reinitialize 一次。七张退役表
（`model_update_pending` / `incr_side_effect_pending` / `published_file_input` / `increment_update_attempt` /
`room_published_inst` / `room_published_topology` / `room_topology_source`）的 `REMOVE TABLE IF EXISTS` 只作开发库清理，
留在 `run_cli` 水化段那组迁移里（ADR-072 D6）。`room-result-cache` / `room-input-cache` 那堆按 key 的 sha256 命名的磁盘工件
**接受存量垃圾并写进文档**：房间流程整套删除后它不再新增，不为删它给 `model_artifact` 加索引层（d-747 第 3 条）。

**为什么不是别的做法**：
- 不写迁移：调试阶段没有需要保住的库；迁移脚本是为一种此后不再出现的形状付出的一次性代价，且要连带兼容读旧列。
- 不给工件加索引层：内容寻址的工件名列不出前缀，要删就得另建一份「谁写过什么」的账——那正是 ADR-071 拆掉的形状。

### D7　保留与不在本轮

- **`legacy_adopted` 接管保留**（ADR-060 D10，d-277）：已有数据的库启动时 `model_sesno` 按 `applied_sesno` 填空、`legacy_adopted`
  跳过恢复 Task 照旧；只退 `room_sesno = room_sesno ?? applied` 那半句。理由：接管是数据面 pe 兼容播种（ADR-001）在模型面的对偶，
  与房间流程无关；D6「不做迁移」说的是不改旧库的形状，不是拒绝认一个已有模型的库。
- **凭证前移不在本轮**（d-427 / ADR-056 P2-2 / CONTEXT「模型凭证前移」原样）：它是另一个焊点（`model_sesno = min(安定根凭证)`），
  混进来会让本轮「删流程 + 改恢复」的验收失焦；ADR-072「不做」里指向它的那句仍然成立。
- **`room_incremental` / `AIOS_ROOM_INCREMENTAL` 开关随流程退**（ADR-033 的三阶段收成两阶段）：开关门的是「消费房间目标」这个阶段，
  阶段没了开关只剩一个永远不被问的布尔；`data_incremental` / `model_incremental` 不动。

## 取代关系（只在本文声明，那几篇的标注是下一刀的事）

| 被取代的段 | 它今天说什么 | 本文哪条接手 |
|---|---|---|
| ADR-072 D2 回退到水位 | 见 `writing{S,T}` → 按 `(dbnum, sesno > S)` 谓词删、三水位 `min(·, S)`、排 S→T 重放；d-570 不删 `pe` 顶点 | D3（整库 Reinitialize；谓词回退整段退） |
| ADR-072 D3 崩溃不分面 | 崩在模型 / 房间中途 `applied_sesno` 一并退回 S | D3（结论「不分面」保留，动作改整库 wipe；`Partial` 边界照 ADR-071 D3） |
| ADR-071 D1 任务内序列化 | 「数据行、模型行、房间边与三枚水位」；Task 末尾「模型行 → 房间 → 收尾一笔盖三水位」 | D1（房间边随模型块同事务）/ D2（两枚） |
| ADR-071 D3 写库失败 | 「任一面（数据 / 模型 / 房间）写库中途失败 = 该面水位不动、留痕」 | D2（面只剩数据 / 模型，房间失败按根记）；`Partial` 不触发重建的边界由 D3 引用它 |
| ADR-071 D4 水位写点 | `room_sesno`（房间成功才推）在任务收尾事务写 | D2（`room_sesno` 退） |
| ADR-071 D5 运行期比对读内存镜像 | 镜像含三水位、「房间 CAS 前提」 | D2（两枚；无房间前提） |
| ADR-071 D8 取数源两档 | 判据「任务在飞 + 三水位相等」 | D2（两水位相等；`room_lagging` 退） |
| ADR-071 不变量 S3 | 「行幂等、水位最后 → 任务中途死掉 → 重放收敛」（ADR-072 已改「写入标记回退」） | D3（写入标记 → 整库重初始化） |
| ADR-065 D1 任务执行体 | 「→ 房间重算（任务末尾）→ 凭证前移 → complete_model_task」 | D1（房间进根；凭证前移不在本轮，D7） |
| ADR-065 D8 房间随任务收尾 | 任务末就地判定与写边、ADR-010 三条规则不变、结构触发由窗口规划器算出、常驻房间模型增补段 | D1（逐根）；G2 (i) 失败留痕收进 D2；三条规则与结构触发见 §未决 U1 / U2 |
| ADR-067 全篇 | 常驻房间模型：缓存两代换代（D1–D3）、参照四写点（D4）、启动对比替全量（D5）、失败语义（D6）、目标集 / 护栏 / 回执 `rooms` 段（D7）、六态生命周期（D8）、参照系 + `room_resident_model` 开关（D9） | 作废；D6 被 D2 吸收；D2「在册面板几何驻留」见 §未决 U3；D5 的全量入口见 §未决 U4 |
| ADR-040 全篇 | 保守房间失效集（D1）、恢复检查点（D2）、`RoomRecalc*` 与 epoch 同事务（D3）、CWALL/CFLOOR 结构枚举（D4）、`room_build:main rebuild_required`（D5） | 作废；D1「有几何的目标全部进房间失效集」在逐根路径上按构造成立，不另立条 |

连带受影响、**不在本表取代清单里**、留待下一刀逐条标注的句子：ADR-072 D4 / D5 中「常驻房间模型按 `room_sesno` 认代」「`published_version` = `room_sesno`」
「启动水化装常驻房间模型几何」；ADR-072 W1「下一次启动 / 开工会**回退**它」；ADR-062 文头「崩溃恢复改由写入标记 + 按 sesno 回退到水位承担」；
ADR-060 D10 的 `room_sesno` 半句与后果「房间不在这枚水位的承诺范围内」；ADR-021 L59 保留 `DELETE …/data/above` 那半句；ADR-033 三阶段；
ADR-011 §8 / §10；ADR-048 D3；ADR-041 §5 与「不做」；CONTEXT.md 词条清单见 t-735 清点表 §1.1。

## 不变量

- **W1 没有独立房间流程**：生产路径不存在任务末按库整段重算房间的入口、不存在房间目标行 / 房间轮 / 结构触发 / 常驻模型换代 /
  参照落款 / 发布视图；`execute_frozen_batch_body` 不含房间阶段。
- **W2 房间边与模型行同事务**：任一成功根的 `room_relate` / `room_panel_relate` 变更与它的模型行在同一 `BEGIN … COMMIT` 块里；
  源码里不存在「模型行已提交、房间边稍后单独提交」的路径。
- **W3 两枚水位**：`dbnum_watermark` 行无 `room_*` 列；`model_sesno` 只在本任务全部根（含各自的房间归属）成功时推进。
  D14 的 `model_room_generation` 是模型水位的属性（「这一版的边对齐到哪一代」），与 `model_sesno` 同一笔写、同一个门，
  没有自己的推进规则，不构成第三枚水位；它旁边的 `model_room_stray_generations`（F3 落库）是这一属性的附注——刷盘按另一代写边时
  同事务记一笔、盖章即清，同样不参与任何放行 / 恢复判决。
- **W4 `writing` 只有一种出路**：`status = writing` 的行，要么它的任务在飞（`task_registry`），要么下一次启动 / 开工把该库本批转
  Reinitialize；`src/` 里不存在按 `sesno > S` 删除的回退路径。
- **W5 库层房间资源不动**：D4 点名的三个函数、`get_room_nodes.surql`、两张边表的定义不在任何 `REMOVE` 清单里，`embedded_surql`
  灌入清单不少一项。
- **W6 `Partial` 不重建**：`task_end` 置 `finished` 的任务，无论哪一面失败，都不会被下一次启动 / 开工排 Reinitialize。

## 后果

- **热路径**：任务末的房间段（7997 每窗 9.1 / 20.2 s、每轮重发近六万条边）消失；房间成本变成每个生成根里一次候选面板筛选 + 点包含判定，
  与本窗变更量成正比。`RoomInputVersion` 那笔整库 JSON 序列化随之消失。
- **崩溃恢复**：从「一行状态 + 一次谓词删除 + 重跑」变成「一行状态 + 整库 wipe + 首次导入 + 整库模型」。代价是全量的
  （ADR-021 后果：7350 一个库约 2 小时），且该库在重建完成前是空的（回退形态下是 S 版）。接受：崩溃是异常事件，只有一种恢复形态。
- **可观测**：`/dbnums` 少 `room_sesno` 一族的格，`write_state` 保留；取数源路由的理由只剩在飞 / `model_lagging`；
  Task 回执的 `rooms` 段随任务末房间段消失（回执按根记成败，房间失败在根的失败记录里说原因；跨仓 plant-ui 一句同步）。
- **代码**：房间流程整套删除（任务房间段、常驻模型、发布 / 提交、结果目录、启动水化、结构触发、房间水位、`RoomOnly` 对外契约）；
  数据面回退删除路径与 `/data/above` DELETE 半边删除；保留的是几何与筛选算法（共享谓词、面板索引、点包含）与 D4 的库层资源。
- **材料表**：`fn::room_code` 一族照旧读边；边的新鲜度从「房间水位」改由 `model_sesno` 与根的安定与否表达。
- **`room-result-cache` 存量工件**：接受为垃圾，不再新增，不做回收（D6）。

## 实施约束（源码钉）

1. `execute_frozen_batch_body` 全文不含 `rooms` 阶段、`calculate_and_commit_memory_rooms`、`room_resident`、`room_structural_trigger`、
   `RoomRecalc`；房间归属的写点只在 `persist_roots_batched` 一类的合块落库入口里，与该根的模型行同一事务（W1 / W2）。
2. `dbnum_watermark` 的读写渲染器不含 `room_sesno` / `room_sesno_time` / `room_settled_at` / `room_input_version` /
   `room_required_input` / `room_result_version`；`RecoveryStage` 无 `Room` 变体；`model_read_route` 的理由字面值里没有 `room_lagging`（W3）。
3. `write_state::rollback_if_writing`（或其后继）见 `writing` 只做一件事：把本批转 `Reinitialize`（`intent` + `reinit_requested`）；
   `fast_delete` 里不存在按 `sesno > S` 的删除渲染；wipe 之后镜像作废、`GeometryBaseline` 失效、一致性普查作废、模型投影清空、
   空间树置脏五件事都在同一条路上跑到（W4，R0 实位）。
4. `web_service` 路由表里 `/api/v1/dbnums/{dbnum}/data/above/{watermark}` 只挂 GET；`prune_above_watermark` 的删除半边不再被任何 handler 调用。
5. `REMOVE TABLE IF EXISTS` 清单恰为 D6 的七张表；`embedded_surql` 灌入清单与 `resource/surreal/get_room_nodes.surql` 逐字节不变（W5）。
6. `options.rs` 无 `room_incremental` 键、无 `AIOS_ROOM_INCREMENTAL` 环境变量；`data_incremental` / `model_incremental` 不动（D7）。
7. `mem://` 真测：（a）ApplyWindow 在数据第 k 块 / 模型第 k 块 / `task_end` 前 kill → 重启 → 见 `writing` → 本批转 Reinitialize →
   wipe → 首次导入 → 逐表（`pe` / noun / `pe_owner` / `inst_relate` / `gen_root` / `room_relate` / `room_panel_relate` /
   `dbnum_watermark` / `dbnum_write_state`）与一次成功相等；ModelOnly 与初始化各一组；（b）某根房间算不出 → 该根非安定、
   `model_sesno` 不越过、其余根照常安定；（c）`Partial` 收尾后重启 → 不排 Reinitialize（W6）。
8. 硬验收沿用 ADR-010 §9 的形状：一组增量收敛后的 `room_relate` / `room_panel_relate` 与同一份数据上一次从零初始化的结果逐边相等
   ——对照物是什么见 §未决 U4，在拍板前用「从零初始化」当对照。
9. live：空目录从零跑通 ams 1112（S4 工单）：`/dbnums` 每行 `write_state.status = finished`、无 `room_sesno` 格、`applied = model`；
   `ams.stdout.log` 无房间段耗时行；材料表 `fn::room_code` 对生成过面板的房间回得出房间号。

## 未决（不在本文拍，逐条单开决策卡）

- **U1　面板自身变化的传播怎么收敛。**（**2026-09-16 由 D13 收**：候选形态 ② 的库内那一半——变更集含房间定义族 / 开工前对齐换代
  → 本库 `RoomOnly` 用现有几何整库重算两张边；跨库那一半仍开着，归 U3。**2026-09-17 补了触发 ③ 电平型再武装、并把「整库」
  的覆盖面说实了**：D13 只够得着投影里的根，生产落盘形态下初始化过的根不在投影里（ADR-077 D5）。**第二半 2026-09-17 拍板收掉**：
  整库重算对不在投影里的根**回读库上几何**（`model_record_reader::durable_records_of_root` 读 `inst_relate` / `tubi_relate`，
  折成 `room_model::durable_room_inputs`，走同一个 `calculate_for_root` / `render_root_room_edges`），靶子是 `DbSet@T` 的根清单，
  落盘形态下也说得了「整库」、盖得上 D14 那一格；不选 ④「接受滞后到整库重建」。**2026-09-17 晚 D 追记：库内那一半从「整库」收成
  「反向传播」**——面板新旧盒查全局树 + 管身范围查询 → 只重算命中的根，整库退成兜底（见 D13 追记 D）。U1 的库内部分至此**关**；
  跨库那一半仍归 U3——反查已把跨库命中数进了回执，b 档「给别的库排根级工作项」等数据再定。下面是拍板前的原文。）FRMW/SBFR 改名（房间号 / 进出 `room_key_word`）、PANE 换属主、`project_hd` 下 CWALL/CFLOOR 换属主
  三种变化都**不改任何几何**：逐根路径只算「本根在哪些面板里」，面板变了、它名下成员（别的根）的边与 `room_num` 不会被碰到——这是
  逐根路径的天然盲区，而原来填它的结构触发（ADR-010 D12 / ADR-040 D4）已退。已见到的候选形态（只列不选）：① 面板所在根重生成时连带
  重算该面板现有成员（要反查「谁在这块面板里」，读源是内存还是库要过 ADR-072 D5 白名单）；② 结构变化时对该库排一次全库 `ModelOnly`
  或整库 Reinitialize；③ 边上不存 `room_num`，房间号在查询时由面板所属房间派生（改 `fn::room_code` 一族，触碰 D4「库层不动」）；
  ④ 接受滞后到下一次整库重建并写进文档。
- **U2　ADR-010 整条退休还是拆出算法部分。** §1 最终一致、§2 混合粒度（整间 / 元素分支）、§4 AABB 触发、§7 队列、§8 同轮吸收、D12 结构触发
  随流程退；§3 共享判定谓词、§5 多归属排序（`inside_count` / `center_dist`）、§6 两树合一、§9 硬验收「增量 == 全量逐边」是算法与验收，
  与新口径同向。是「文头标已作废 + 算法条目搬进新 ADR」还是「原地修订、只退流程条」，连带决定 CONTEXT「整间分支 / 同轮吸收」两词条的去留
  与 U1 里「PANE 自身变化成员还重算吗」的答案。
- **U3　ADR-067 D2「在册面板几何驻留内存」作为算法输入形态留不留。** 逐根筛选需要一份候选面板集（世界 AABB + 网格），来源可以是
  ADR-067 D2 那种驻留的面板索引（去掉换代 / 参照 / 生命周期，只剩「生成 PANE 时就地替换它的几何」）、也可以是 `ModelMemoryStore`
  按 `noun == PANE` 现取、或空间树按 noun 过滤（ADR-010 §6 的形状）。三者的差别在冷启动（PANE 尚未生成时构件先生成怎么办）与跨库
  （面板在 A 库、成员在 B 库）两处，本文不选。（2026-09-17 D 追记：跨库那一问现在有数据口——反查回执的 `cross_library` 数的是命中却
  不属本库的元素；a 档只数不算，b 档「给命中的别的库排根级工作项、由那个库的下一个任务消费」等真库上看见这个数再定。
  **2026-09-17 晚 U3 b 追记：跨库那一半关**——反查不分库、命中的别的库停放成根级工作项 `RoomRoots`、由那个库的下一个任务沿准入链
  消费（D13 追记 U3 b）；停放槽进程内、重启即丢，③ 兜底。**算法输入形态那一半仍开**（驻留面板索引 / 投影现取 / 树按 noun 过滤，
  冷启动那一处）；另欠一小块：没命中的库看不到「这一段与你无关」，仍由 ③ 整库一次。）
- **U4　「全量重建房间」入口还存不存在。** `AIOS_ROOM_FULL_REBUILD` / `build_room_relations` / ADR-067 D5 六条显式入口 / ADR-048 D3
  「不覆盖启动全量房间重建」——按 D1 它们都没有宿主。若不再有任何全量入口，ADR-010 §9 硬验收的对照物就只能是「同一份数据从零初始化的结果」
  （实施约束 8 暂按此），且「房间算错了想整库重算一次」的运维动作 = 该库整库 Reinitialize；若保留一个只重算房间边的入口，它就是 D1 说的内部
  `RoomOnly` 的全库形态，要不要给它 HTTP 口与 d-283 ① 的关系一并定。

## 不做

- 不为结构变化的传播（U1）复活结构触发、常驻换代或任何第二条房间账——在 U1 拍板前保持空白。
- 不写旧库迁移、不做兼容读 `room_*` 旧列（D6）。
- 不退凭证前移（d-427）、不动 `legacy_adopted`（D7）。
- 不做任何按 `sesno` 的部分回退 / 部分裁剪（D3 / D5）；不给任务与库之间加队列 / 写者 / 收口（d-285 / d-300 原样）。
- 不删 D4 点名的 DB 层房间资源、不改 `e3d.room.lookup` 的名字与形状。
- 不给房间开新的 HTTP 触发口；`RoomOnly` 只在生成路径内部（U4 若拍出全库形态再议）。
