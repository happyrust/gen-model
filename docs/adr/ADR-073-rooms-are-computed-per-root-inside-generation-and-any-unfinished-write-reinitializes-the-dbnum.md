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
  （`render_renumber_statements`）——不查空间、不算几何，渲成 prelude 进第一笔事务（删在第一笔）；`recalc` 命中的根经
  `recompute_dbnum_room_edges(…, RootScope::Targeted { roots, panel_statements })`：投影里的按集合筛、其余回读库，其余一字不改
  （同一个判定入口、同一套渲染、同一种装块）。⑤ **盖章**：命中根全算齐 = `RoomRecomputeVerdict::Targeted(重算那台计算器的代号)`
  → `LibraryRoomClaim::Recomputed` → D14 那一格盖 `Aligned`（不盖的话下一个任务 ③ 看到库上 ≠ 常驻照样整库扫、定向白做）；任一根
  算不出 / 反查读库失败 / 写库失败 = `Failed`，不盖、③ 再武装、下一个任务整库；树不就绪 / 根清单枚举不出来 / 有折不出根的 / 覆盖面
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
    作废 → `not_computed`，不拿上一代的边冒充这一版的结论。只在**落盘**形态下作数：进程内嵌 `mem://` 活不过重启，
    库与水位行一起消失，那一档仍只认进程内 ACK（d-381 的启动屏障要的就是本进程逐根算完那一句）。
  - **看**：`/dbnums` 每行多一格可选的 `room_alignment{generation, resident_generation, current}`（DESI 行才有，spec §4.7）
    ——`current: false` 就是「这个库此刻只能回 `not_computed`」的解释，是这句话的唯一出处。那句「成不成立」与查询侧共用
    一处判据（`query_service::library_room_alignment_is_current`）：各写一遍就会出现面板说「对齐了」而同一个库的查询回
    `not_computed`。它是现状陈述不是告警——要点亮，给该库跑一次 `model/rebuild` 或重初始化。
  - **它不是第三枚水位**：没有自己的推进规则、不参与任何放行 / 恢复判决，只是模型水位的一个属性（「这一版的边对齐到哪一代」），
    W3 说的 `room_*` 五列一个都没回来。库上这一格与真实的边之间只有一个缺口：关着开关跑过一轮之后再打开——那正是
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
    规划失败的早退仍 `None`）；两个执行体里触发 ③ 与定向准入共用的读数 `room_recompute::durable_alignment(job.dbnum)`（库上那一格 +
    此刻常驻代号，一次读）落在 `refresh_room_reference_before_task(job.dbnum)` **之后**、`request_for_task(` 之前，并以
    `alignment.as_ref(),` 真的传进去（对齐之前读到的常驻代号是上一代，比对会白点一次或漏点一次；2026-09-17）；`execute_frozen_batch`
    的 DESI 收尾段里 `run_room_recompute(job, &request, &mut result, file_latest_sesno)`（定向 / 整库的分派，D 追记）在 `await_units_durable(` 之后、
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
      panel_statements }` 投影里的按集合筛、回读清单 = 集合 ∖ 投影、`panel_statements` 作 `pack_publication_transactions` 的 prelude
      （钉 `a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_first`——`mem://`：A、B 同在 R1，面板搬家
      只点名 A → A 的边删、ACK 换代，B 的边与 ACK 一字不动，不在册的 777 的出边与 `room_panel_relate` 入边随第一笔清掉；再点名 B →
      与整库同一终态；`panel_level_statements_clear_unregistered_panels_and_renumber_rooms_only`）。
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
      一处**、在 `Some(uncovered) if uncovered.is_empty() => {` 那一臂里、`Some(RoomRecomputeVerdict::Targeted(generation))` 之前，
      `run_room_only_recompute` 里没有（钉 `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer` 扩）。
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
  没有自己的推进规则，不构成第三枚水位。
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
