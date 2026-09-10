# ADR-067：常驻房间模型——库是存储与参照，不是计算源

状态：**已接受**（2026-09-08 夜用户过目通过：「ADR-067 过目通过，状态改『已接受』」。T4 / 08 重构计划 S8 的开工门随之只剩 T1–T3 与 completion S2/S3；**号到提交时复核**——ADR-060 曾撞号两次）。
日期：2026-09-08
基座：**ADR-065 D8**（房间随 dbnum Task 收尾就地重算，读实时几何）——D8 只有六行，本 ADR 落搬家的全部约束。
设计与拍板全录：`docs/plans/2026-09-07-resident-room-model-plan.md`（grill 八轮，前五轮 + 第 6/7/8 轮 Q1–Q23 全部按推荐；§3 P4.5 = 搬家设计）、
`docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md`（启动对比详设，S1–S4 已落地）、
`docs/plans/2026-09-08-room-resident-model-completion-plan.md`（生命周期 / 覆盖 / eager warm；**几何方向基准**）。
共识 **d-21**（2026-09-08 夜用户裁决：几何数据驻留、发生修改时就地替换——翻案第 6 轮 Q1 (a)「不驻留、每次重读」）。
关联：
- **修订** ADR-048 D3「限定域不得把启动全量房间重建拖回来」→「不得拖回**全量修复**」——启动对比是定向的、
  成本随变更量走，不受 D3 约束；源码钉 `the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild` 随之改口径。
- **继任** ADR-062（已被 ADR-065 修订为退役：发布视图三表、文件轮次屏障）——可见性与「边 == 拓扑」的承诺
  从发布屏障改由**参照**（D4）与 ADR-010 §9 硬验收接手。
- **不动** ADR-010（整间 / 元素 / 同轮吸收三条判定规则；硬验收「增量收敛 == 全量重建，逐边」）、ADR-040（失效正确性）、
  ADR-054（生成时点 = 文件最新；与 D3「键只前进」相容——Task 的 T 就是它冻结的文件时点）。

## 背景

房间归属的两条分支（整间 / 元素）每轮都要面板侧输入：房间→面板拓扑与在册面板几何。今天拓扑按 `(dbnum, sesno, composite)`
缓存但任一库变会话就全部文件重读（ams7997 实测 6–7 s / 文件），几何逐面板各查一次、三角网各读一次 `.mesh`；
`room_published_topology` / `room_published_inst` 既当存储又当读源，ADR-065 D8 拆掉发布视图后这条读法失去宿主。
另一个教训是冷启动死锁的形状：模型按文件最新装、`panels` 全空，而几何变化不一定伴随会话变化——键不变就永远复用没几何的那份，
全库 `NoGeometry` → G2 (i) 扣死 `model_sesno`。结论：**计算源是进程内常驻模型（缓存），库里只留存储与参照**；
缓存的新鲜由「产生变化的那个 Task 主动替换」保证，不靠键被动重载。

## 决定

### D1　一份真值，一份缓存

常驻模型 = 文件侧房间分组（拓扑：`groups` / `map` / `digest` / `room_files` / 各库会话）+ **在册面板几何 `panels`** +
三角网缓存 `MeshCache`。它是缓存不是第二份真值：换代只在 `lock_spatial_serial` 下整份原子替换（留两代），不做逐条修补；
真值在设计文件与库行里。形态只有一份（`ResidentForm::Published` 随 ADR-065 三表退役）；回答归属的入口只经换代点
（`ensure_at` / `current`）取当前代——源码钉 `membership_paths_only_take_the_model_through_for_membership` 收窄为单形态，
禁直摸 `resident_rooms()`。

### D2　几何驻留，发生修改时就地替换（共识 d-21；第 8 轮 Q21 (b) / Q17 (a)）

面板几何（世界 AABB / 实例变换 / `geo_hash`）与三角网**随代驻留**。保鲜不靠键，靠替换点：**dbnum Task 房间收尾**，
锁外对本库全部在册面板 `query_valid_insts` 一条 SELECT 整段重查（自愈：没被产物清单点到的漂移一并收正）+ 对新网格键
warm；锁内合成新代原子交换——本库几何段整段替换，**其它库的会话 / 令牌 / 几何原样继承当前代**。本 Task 重生成 / 删除 /
位姿变化的定向名单只驱动「哪些面板要重算」（目标集），不驱动「缓存换哪几条」。无当前代（初始化没建起来）= `Blocked`，
按 D6 扣根，**Task 内不探文件、不现建基线**（启动只探文件最新一次）。冷启动由初始化 eager 装载兜住：全 MDB 拓扑 + 几何 +
三角网 warm（completion 计划第一~三阶段），就绪门见 D8。`MeshCache` 键 = `(geo_hash, 世界矩阵位模式)`，内容寻址不可变、
跨代 `Arc` 复用。半份几何（一块面板一行可用一行坏）整块判不了：不进 `panels`、只进 `missing`，两条路（常驻装载 / 行查询）同判。

### D3　换代键与锁纪律（Q2 / Q5 / Q10 / Q11 / Q12 / Q16）

键 = 各 DESI「上次房间收尾的会话」，本库 = 本 Task 的冻结 T，**只前进不后退**（启动已装到更新会话时用更新的那份，回执点名）。
`ensure_current` 拆两个入口：`ensure_at(dbnum, T)`（Task 用，只前进本库）与 `current()`（启动对比用，只读不推）；
探文件最新只在房间初始化那一次。锁纪律：**锁外备料**（按 `(dbnum, T)` 预热分组、本库几何重查、warm、分片 `capture_at(T)`）、
**锁内换代 + 写边 + 写参照**；稳态 Task 跨 dbnum 并行，房间收尾在 `lock_spatial_serial` 下串行（同进程两份模型不并发写边）。
`GroupCache` 逐代按 `(dbnum, sesno)` 命中、**命中即提升**进当前代（否则两代淘汰在第二个 Task 换代时把没变的文件全淘掉）、只留两代。

### D4　参照与四个写点（Q9 / d-42 / Q4 / Q18 / Q19）

参照 = `room_topology_source` 分片（按令牌内容寻址、幂等）+ `room_build:main.resident_reference{tokens[], digest, settled_at}`，
语义：「库里的归属边就是按这份拓扑算出来的」。**只在这句话成立的瞬间写**，写点恰四处：
① 启动对比应用全成；② 显式全量重建全成；②' **真冷启动**初始化 Task 全成（条件：范围 = **MDB 全部 DESI**，每库本进程都跑了
基线 Task 且全部成功、未受阻降级——限定域生效时恒不满足，走「无参照 → 显式全量」臂，Q19）；③' 单个 Task 房间收尾全成 ∧
启动对比无尾巴（`startup_compare_tail() == None`），只前进**本 dbnum** 的令牌。③' 的进锁段是**读-改-写**：重读当前参照与
当前代、只换本库令牌、摘要从当前代现算，**禁止把出锁前的参照快照带进锁**（并发窗口唯一的危险形状：会把邻库刚写的令牌倒回去）。
写失败只出声、不拦 Task / 不拦启动（退路：下次启动 `NoReference` → 全量一次，多算不漏算）。进程内闸 `startup_compare_tail`
缺省「有尾巴」——启动对比没跑完之前 ③' 一律不写。

### D5　启动对比替全量；全量降为显式修复入口（Q3 / Q7 / d-42 / Q10）

启动房间段拿 `current()` 键（**不探文件最新**）对参照，臂表：先摘要（等 → `Unchanged` 零写入）、再同键（同键不等 =
`IntegrityMismatch`，库被动过 → 标 `rebuild_required` 走全量）、再按参照令牌从分片合成旧视图做 `TopologyDiff`
（`Apply`：删 / 整间重算 / 改列三类；删与重算只走 `recalc_panel_membership_*`，改列走 `render_renumber_statements`，
边 id 不变只改 `room_num`）。全量重建只剩**六条显式入口**：无参照 / 读参照失败（与无参照同判）/ 同键不等 / 参照不可读 /
持久 `rebuild_required` / `AIOS_ROOM_FULL_REBUILD=1`。门：止血口与空树平凡最强（对比与全量一起跳）；
`startup_data_ready` / `spatial_compute` / `model_incremental` 三道照旧；`startup_autorun` **只门全量臂**（对 ADR-048 D3 的修订）。

### D6　失败语义（ADR-065 G2 (i) / D4；Q3 / Q6 / Q13）

房间重算排在牵涉根凭证前移**之前**。某根牵涉的房间重算失败 = 该根本轮未完成：`gen_root` 非安定、凭证不前移、`model_sesno`
不越过、失败记录写房间步骤与原因；**不排队、不自动重试**（ADR-065 D4），再被算到的路：下一窗触到 / `model/ensure` /
启动对比 / 显式全量。**结构触发的面板失败扣面板所属的根**——即使元数据门（NAME-only 不重生成）放行了凭证，也从本窗
凭证前移集合剔掉（元数据门「凭证前移不少收」的唯一例外）。**常驻模型建不起来** = 本 Task 全部触到的根扣住，回执点名
`resident_blocked`。**无几何面板不算失败**：留痕（回执 `no_geometry` 按库、`/health` 点名未监听库）、不扣根、不挡参照；
面板进排除集、存量边受保护（钉 `a_panel_without_geometry_is_never_written_as_an_empty_member_set`）。
两份欠账并存、各答一问：根凭证答「哪些根的房间没算成」（按根），参照答「边按哪份拓扑算过」（按库）。

### D7　目标集、目标应用内核与护栏（Q20 / 第 4 轮 Q8 / P4 M3 / Q14）

Task 房间目标集 = F-A 账本结构触发（**唯一触发源**，窗口规划器算出）∪ 本 Task 发布产物折出的元素重算与清边
（`room_publication_effects` 的「产物 → 目标」逻辑在内存里直接交房间收尾，不再落表）。Task 收尾与启动对比应用**共用
「目标应用内核」**（目标 → 写边 → 收失败；写边只经 `recalc_panel_membership_*`，禁拼 `DELETE` / `RELATE ` SQL），编排各留各。
diff 不产目标：常驻模型的 `TopologyDiff` 只在启动对比（D5）与护栏出手——护栏 = 换代 diff 非空而本 Task 面板目标为零 →
限流告警、**只告警不标脏**（Task 手上是完整目标集，分页假阴性按构造消失；标脏与否等 live #2 观测后再议）。
Task 回执加 `rooms` 段（swap / targets / computed / no_geometry 按库 / failed 与被扣的根 / reference_advanced / elapsed）；
`/health.room_build.resident.last_swap` 按 Task 记。

### D8　生命周期与就绪门（completion S1，已落地；Q22）

进程级六态 `Disabled{source} / Loading{phase} / Ready{generation} / Degraded{generation, problems} / Blocked{problems} /
Unsupported{reason}`；ingest 与 read-through **共用同一段启动预备**（空间树 → loading → 房间初始化 → ready / degraded /
blocked），read-through 不再绕过初始化。会写房间边的**队列消费入口**在「有体制而无可信当前代」（Loading / Degraded /
Blocked）时返回结构化 `RESIDENT_MODEL_UPDATING`，由调用方按可重试处置；`Disabled` / `Unsupported` = 没有体制，放行走
兼容路。**显式全量修复入口不装门**——它是 resident 坏掉时的修复退路，装门即死锁。**Task 侧不引入等待**（Q22）：顺序钉
保证初始化先行，稳态收尾只认两态——有当前代 → 干活；`Blocked` → 按 D6 扣根。

### D9　房间是参照系，不是增量的一部分（第 5 轮 Q17–Q20）

常驻模型范围 = MDB **全部** DESI（`design_sources()`），初始化在启动最前段（空间树之后、watcher / worker 之前），只认
`spatial_compute`，不受 `startup_data_ready` / 上弦位 / `startup_autorun` / `watch_dbnums` 任何一道门；房间三份文件不识
`watch_scope`（源码钉 `room_files_never_consult_the_watch_scope`）。写归属边的启动对比与全量修复才受门（D5）。
开关 `room_resident_model` 默认开（2026-09-08 翻转，提交待落）；关 = 每 Task 文件遍历拓扑 + 行查询几何 + 直读 `.mesh`
（显式诊断退路，live #2 之后另立一笔删关路与开关）。

## 后果

- **热路径**：缓存命中时房间收尾毫秒级（assemble + 一条 SELECT + `MeshCache` 命中）；换代成本只有会话变了的那个库付，
  且文件遍历与分片 `capture` 在锁外（60 MB 库 7 s，120 s 窗口预算内；live #2 实测锁等待）。
- **内存**：拓扑 KB 级 + 几何行 + 三角网（面板网格是箱体 TriMesh，ams 量级 KB–MB；live #2 记 RSS 与缓存字节）。
- **正确性**：拓扑与本 Task 刚生成的几何在同一会话对齐（ADR-062 想保、D8 用最终一致放弃的那一半，不靠发布视图拿回来）；
  漏写参照的代价恒为「下次启动多比一次」（幂等）；崩溃任意点 → 参照没写 → 再比一次，多算不漏算。
- **可观测**：`/health.room_build.resident`（lifecycle / reference / startup_compare / last_swap / last_failure /
  panels_ready / mesh_keys_*）；水位上「房间扣的根」在 Task 回执点名。
- **代价**：别的库一个文件坏了（建不起来）全 MDB 的 `model_sesno` 卡到它修好——可见、诚实（Q13 拍板接受）。

## 实施约束（源码钉与硬门）

1. 新宿主 `src/fast_model/room_task.rs`（Q15）：`batch_worker.rs` 只留一行调用、不含 `room_resident::`；全文不得出现
   `latest_frozen_sources`（Q17）；本库几何整段重查在锁外、合代交换在锁内（Q21）；③' 进锁段读-改-写、只换本库令牌（Q18）；
   结构触发面板失败剔凭证前移集合（Q6）；建不起来时本 Task 触到的根一个不前移（Q13）。
2. 搬家同名重钉：锁在换代前；diff 不丢、护栏在换代之后；③' 两前提、零 I/O 的闸排在查询之前（原 `model_update_pending.rs`
   三钉随文件退、在新宿主重钉）。
3. `GroupCache`：`the_group_cache_keeps_only_two_generations` 加「跨代命中即提升、连续两次换代不丢没变文件的命中」（Q16）。
4. 两态逐边门（ADR-065 实施约束 2）：夹具「关」侧 `query_valid_insts_on(db)`、「开」侧 `load_latest_on(db, 分组)` 装整份
   （拓扑 + 几何），比几何输入、拓扑来源与网格路径三样（P2-b 形状）。
5. 硬验收不变（ADR-010 §9）：增量收敛 == `AIOS_ROOM_FULL_REBUILD=1` 全量重建，`room_relate` / `room_panel_relate` 逐边；
   live #2 八条场景（07 计划 P5 改排）+ 冷启动就地替换 `mem://` 回归门 + ③' 双库互不覆盖 `mem://` 门。
6. 既有钉不动：`room_files_never_consult_the_watch_scope`、`the_full_rebuild_entry_never_takes_the_resident_model`、
   `a_panel_without_geometry_is_never_written_as_an_empty_member_set`、`resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher`（顺序与禁门部分）。

## 实施进度（2026-09-08 夜）

已落地（提交号见 07 / 08 两份计划的落地记）：P1 本体（`room_resident.rs`）、P2-a/P2-b（换代点 + 几何接缝 + 两态门，
`ba0fa50ae` / `24b2b43ce`）、P3-init/P3-b（初始化 + `lib.rs` 接线，`5fc5f7417` / `b7a7922ff`）、启动对比 S1–S3
（`dbe5f4670`）、P4（护栏 + 闸，`e7628f886`）、completion S1（生命周期 / 双启动路径 / UPDATING 门，工作树未提交）、
`warm_meshes`（S3 半截）。待做：S2 严格覆盖 + S3 指标；**T4 搬家 = 08 计划 S8 = completion S4 同一笔**（本 ADR 过目后开工）；
live #2 验收；默认翻转与三份计划文档落显式提交；CONTEXT.md 词条（§9 新稿，等文件离开在途改动集）。

输入面审核（2026-09-09，`docs/2026-09-09-local-first-input-audit.md` §4）：`room_task.rs` 的计算输入已只剩常驻模型 +
`ModelMemoryStore` + `.mesh`，启动建模两形态都走文件；仍偏离目标的是 `freeze_source_shards` 同步写分片、`ensure_current`
只被 legacy drain 调且 ingest 面从库重建（与启动文件版双轨）、写边 `room_result` 同步写库、room_task.rs:49 `missing_panels`
整库拒算与 D6 相反。三水位的 DB CAS 按用户同日拍板「水位以库中值为准」留库、不算偏离（`LocalDbnumState` 的 `room_sesno`
只能是镜像）。
