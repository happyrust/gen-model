# ADR-073：房间随根生成、不再是一条流程；任何未完成写入一律整库重初始化

状态：**已接受，2026-09-11 修订房间总开关语义**（2026-09-10 用户拍板：「房间只保留几何与筛选算法，流程整套删除」（共识 **d-757**）；「任何未完成的写入
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

- **U1　面板自身变化的传播怎么收敛。** FRMW/SBFR 改名（房间号 / 进出 `room_key_word`）、PANE 换属主、`project_hd` 下 CWALL/CFLOOR 换属主
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
  （面板在 A 库、成员在 B 库）两处，本文不选。
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
