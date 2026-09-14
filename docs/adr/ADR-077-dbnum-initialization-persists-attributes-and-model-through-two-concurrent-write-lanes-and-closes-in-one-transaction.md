# ADR-077：dbnum 初始化用两条并发写入 lane 直接落盘属性与模型，任务收尾一笔盖两枚水位

状态：**已接受**（2026-09-14 grill-with-doc 两轮 Q1–Q21 用户全按推荐拍板，原话：「简化我们的实现，不要过度设计，
第一目标是把属性数据和模型都能正确的落盘。在初始化 dbnum 时，dbnum task 能高效的将属性数据和模型数据都写回到
rocksdb。属性数据的写回和模型数据的写回可以分开，并发的写，有个专门做数据写入到 rocksdb 的后台，可以专门处理传
过来的 channel data」；随后「/implemen 开始实现」；S1–S5 落地、8000 真库实测过目后同日「把 ADR-077 状态改成
『已接受』」。**号到提交时复核**——ADR-074 / 075 落盘时尚在暂存区，ADR-076 与本文同日。）
日期：2026-09-14
关联：
- **落实** ADR-076 D4（属性写接共用写入队列）、D5（收尾一笔盖水位）、D11（初始化模型直写 RocksDB、不进投影）。
  ADR-076 D11 起草时把「整库生成完 `gen_root` 0 行」解读成「回执只活在投影里」；本文 D5 指出那是持久档回执渲成
  `UPDATE`（不建行）的实现问题，`render_receipt_with_target(create_missing = true)` 现成就是 `UPSERT`。
- **不动** ADR-071 D1（任务是隔离单元、任务内不回读库）、ADR-072 D1 / W1（写入标记与水位同一笔）、ADR-073 D1 / D3
  （逐根房间归属；未完成写入整库重来）、ADR-074（两档存储形态）。
- **不动增量路**：`ModelWriteBehind`、`POST /model/ensure` 的整库 ensure、ADR-076 S1–S7 本文一行不改（D1）。
- 词汇：`CONTEXT.md` 新增「落盘写者」「写入 lane」「初始化双 lane」三条（S6）。

## 背景

### 初始化今天是串行的，而且串行点是一次没必要的库读

`manual_update::initialize_dbnum_baseline` 的顺序：`baseline_ingest::ingest_file`（e3d-io 流式解析 → 元素批 SQL →
`flume` 有界队列 100 → `sql_write_queue::spawn_writer` 单写者）→ `finalize_baseline` 推 `applied_sesno` →
`baseline_model_plan` → **`load_baseline_nodes` 分页读 `pe` 表**枚举交付单元根（7997 上 6,772 根，约 2/3 零几何）→ 把 UnitTask 交回
dbnum 任务 → `run_unit_worklist` → `generate_and_persist_roots`（生产端并行生成 → `mpsc(1)` 切片 → `persist_memory_batch`
→ `ModelMemoryStore` 投影 → `ModelWriteBehind` 后台刷 RocksDB）→ `enqueue_model_watermark_write_behind` 由后台盖
`model_sesno`。

模型面开工要等属性面落完，**唯一**的硬耦合就是那次读 `pe` 枚举根——而枚举根从文件就能做
（`generation_root::enumerate_generation_roots(DbSet, …)`，`model_dbnum_ensure::capture_snapshot` 已在用）。

### 初始化的模型今天绕了两层不需要的东西

初始化的每一行都先 `try_commit_current` 进投影、再由 write-behind 渲成 SQL 刷库。投影为增量的「算完即可见」而存在，
初始化期间读路由本来就落在 Memory 且投影为空（任务在飞），没有人读它；write-behind 的去重 / 退避 / 死信 / settlement
为「持久化与任务解耦」而存在，初始化任务自己等落盘就都用不上。代价是一份没人读的整库内存投影，以及上一轮审核点出的
一串 write-behind 缺陷（回执消失时空转、事务无超时、`Blocked` 根钉死整库水位）。（起草时把 7997 峰值 1.5 GB 也记在
投影账上；2026-09-15 补量拆项后那 1.5 GB 的大头是 e3d-io 池级元素缓存（约 720 MB），投影不是主体——见 evidence。）

### 事实（2026-09-14 核过）

- 生产 schema 里 `inst_relate` / `tubi_relate` / `pe_owner` 都是 `SCHEMALESS`，没有 `TYPE RELATION … ENFORCED`：
  模型边**不检查** `pe` 顶点是否存在，两面谁先落都行。
- 属性面与模型面写的表 / 键集合不相交：`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` vs `inst_relate` / `tubi_relate` /
  `inst_geo` / `inst_info` / `aabb` / `trans` / `geo_relate` / `gen_root` / `room_*` / `spatial_epoch`。
- 多写者实测（zone-partitioned plan）：**模型面**4 / 8 写者在 RocksDB 上更慢，撞的是 `spatial_epoch:current` / 共享
  `inst_geo` / 房间边——那是同一面多写者；属性 vs 模型两面并发**没量过**。
- `sql_write_queue` 三条纪律现成：单写者、有界、`WRITE_WINDOW < QUEUE_CAPACITY`；`pe_batch::execute_on` 带 16 次
  冲突退避重试，**无超时**。
- 房间归属的面板来自 `room_resident`（文件冻结的 resident 模型），空间树收敛 `converge_tree_after_persist` 吃内存
  盒子——两者都不依赖投影。`wipe_dbnum_for_reinit` 一次清数据 + 模型 + 房间边 + `gen_root` + 两枚水位。
- `PUBLICATION_STATEMENT_HARD_CAP = 8 MiB`；`render_receipt_with_target(create_missing = true)` 渲的就是
  `UPSERT gen_root SET dbnum=…, desired_revision=desired_revision ?? 0, status='Generated', publication_status='ready', …`。

## 决策（D1–D12，对应 grill 两轮 Q1–Q21）

### D1　范围：只动初始化这一条路

`BatchIntent::Initialize` / `Reinitialize` 落到 `initialize_dbnum_baseline` 的那一段。增量（ADR-076 S1–S7、
`ModelWriteBehind`）、`POST /model/ensure` 的整库 ensure、烟测 `e3d_persist_smoke` 都不动（烟测只加一档给 D3 量数）。
写者按「增量以后也能接同一条模型 lane」定形状——它就是 ADR-076 D2 要 `await` 的那道栅栏——但本轮一行增量代码不改。

### D2　落盘写者：一个模块、两条 lane、每条 lane 一个单写者、两条 lane 并发

`data_interface/persist_writer`（`sql_write_queue` 改名 + 泛化）：消息从 `String` 变成 `WriteJob { sql, label }`，
`label` 只给失败样例归因（元素批写 `element batch #k`，模型写 `model slice roots=[a,b,…]`）。属性 lane = 现成那条
原样；模型 lane = 同一套机器的第二个实例。每条 lane 一条有界 channel + 一个单写者；两条 lane 并发跑。
`ModelWriteBehind` 不参与初始化。写者**不认识根**：合块与拆分在模型侧渲染时做完，写者只执行文本。

写者**逐笔取、逐笔写、不攒窗**：旧写者 `chunks(WRITE_WINDOW = 20)` 攒够 20 笔才动手，于是要靠
`WRITE_WINDOW < QUEUE_CAPACITY` 防死锁；深度 2 的模型 lane 在那个形状下必死锁。写者本来就一笔一笔发库，攒窗没有
收益，去掉之后那条死锁纪律随之消失（ADR-076 D4 里提到的那条，到此作废）。

否掉的两案：(b) 一个写者轮询两条 channel——串行，两面的落库时间相加；(c) 两面各自维护一套机器——第三种写库纪律。

### D3　两 lane 并发默认开，落地前在真库量一次；有冲突不做配置键

键不相交、边不查顶点，理论上互不冲突，但没实测过。落地切片里先在全新 RocksDB 上量「同库双 lane 并发」的总时长与
写者的冲突重试次数（尺子是 `persist_writer::WriterOutcome::conflict_retries`，工具是 `initialize_ams_dbnums --with-model`
——与 dbnum 任务同一个编排，不另造烟测档）。若出现冲突重试：第一刀把每笔模型事务里的 `spatial_epoch:current += 1`
摘掉、初始化只在收尾那一笔 +1（初始化期间没人读树）；仍撞就退回 (b)「一个写者两条 channel」——十行的改动，
**不做配置键**。

**2026-09-14 实测（AMS 8000，两轮）：零冲突、零超时，墙钟 = 属性 lane（22–25 s），模型 lane 7–8 s 整个被盖住。
并发，不退。** 2026-09-15 补量 AMS 7997（170,085 元素、6,772 根、模型 424 笔 / 99.5 MB）：仍零冲突、零超时，
墙钟 150.5 s = 属性 lane，模型半边约 58 s 收工、整个被盖住（lane 账原先记到排空、被数据半边的收工时刻顶成 110 s，
已改为记到最后一笔写完）。详见 `docs/evidence/2026-09-14-adr077-dual-lane-initialization-8000.md`。

### D4　模型面的消息单元是生成侧已有的切片；一片一笔；初始化走 `WriteMode::Initial`

消息单元 = `GeneratedMemoryBatch`（若干交付单元根，按 `model_regen_execution_group` / `model_memory_slice_elements`
封片）。SQL 在任务里（消费端）渲好，写者只执行文本。一片一笔事务；渲出来超 8 MiB 就按根拆成几笔、根不劈；单根超
8 MiB 在渲染侧判死信、不进队列。初始化前必 wipe、库里没有旧行，所以走 `WriteMode::Initial`（纯 `INSERT IGNORE`），
**不做整片删、不读 `slice_sources_on`**。交付单元根的碎由切片合块补回（实测 512 根合块 32 笔 15.7 s；逐根 149 笔多付
5–6 s）。

### D5　初始化模型不进投影；回执、空间树、房间归属三件事在同一片里做

生成结果直接渲 SQL 交模型 lane，**不** `try_commit_current` 进 `ModelMemoryStore`、不登记 write-behind。随之：
(a) 根回执写成 durable `gen_root` 行——`render_receipt_with_target(create_missing = true)`，与该根的行同一笔；
**零几何根也落回执**（`published_geometry_count = 0`），否则重启核对把它当「没生成过」重排；
(b) 空间树按片 `converge_tree_after_persist`（盒子来自内存切片，不依赖投影）；写者只认文本、没有逐笔回执，所以盒子
在任务里攒着、两 lane 都排空成功后收敛一次；
(c) 房间归属用任务开工冻一次的 `frozen_room_calculator`（ADR-073 D1：任务期间不换代）。
死信根沿用增量失败根的现有写法（`mark_generation_failed` 那一句 `status='failed', last_error=…`），不新发明一种失败回执。

**生成输入跑完即弃（2026-09-15 补）**：模型半边一结束，`model_half` 就 `forget_generation_input(dbnum)` 把本次的
`DbSet`（含池级元素缓存）+ 整库属主表从进程级 `GENERATION_INPUT_CACHE` 放掉——初始化用冻结源摘要当键，之后没人再取，
不放就占到进程退出、多库按 dbnum 累加（`room_resident` 早就这么做）。7997 A/B：模型半边收工那一刻驻留从 ~1.35 GB 掉到
~420 MB，峰值不变（1.54 GB 是生成期间元素缓存的价钱，见 evidence「跑完即弃」）。有界元素缓存另议。

### D6　模型根从文件枚举；根粒度仍是交付单元根

`load_baseline_nodes` 读 `pe` 是两 lane 唯一的串行点。改成 `generation_root::enumerate_generation_roots(DbSet, WORL 根,
unit_types)` 从文件枚举（`model_dbnum_ensure::capture_snapshot` 同一条路），模型面开工不等属性面。根粒度**不趁机切到
ZONE 根**——那牵涉回执 / 覆盖判定 / 增量归根三处口径，是另一个决策。

### D7　收尾一笔：两 lane 排空、属性面计数核对通过之后，`task_end` 同时盖 `applied_sesno` + `model_sesno` + 写入标记

`finalize_baseline` 不再在摄入一完就先推 applied。两枚水位一个写者、一种崩溃语义（ADR-076 D5 的形状）。
`TaskEnd` 多一格 `data`（`applied` 的推进 + `confirmed_empty_baseline_sesno`），渲在写入标记与模型水位那句之前，
模型水位那句的 `WHERE applied_sesno >= T` 闸因此在同一笔里成立。Ref0 统计快照也进这一笔。

### D8　失败两档，不加第三档

- **lane 失败**（解析器错、写者一笔事务 16 次重试后仍败或非可重试错、CATA 闭包错）→ 关两条 channel → 等两个写者排空
  → `wipe_dbnum_for_reinit` → 任务 Err，下一轮从 0 重来。基线「任何失败先清库再 Err」原样，只是从一条 lane 扩到两条。
- **单根生成失败**（死信）不算 lane 失败：该根 `gen_root` 记 failed、任务 `Partial`、`model_sesno` **不盖**、
  `applied_sesno` 照盖、写入标记照收（ADR-065 D4 / ADR-073 W6），后续靠懒生成 / `model/rebuild`。

**不做二分**：库确定性拒绝一笔已渲好、已校验的 `INSERT IGNORE` 实测里没出现过；失败样例带 `label` 里的根名单够诊断；
真出现再补。渲染期问题（单根 > 8 MiB、SavePlan 校验）= 根死信。

### D9　队列上界：属性 lane 100 笔；模型 lane 深度 2（按片）；常量不做配置

生成 → 渲染那条 `mpsc(1)` 不动；渲染 → 写者深度 2（一片在库里写、一片渲好等着、生成侧在攒第三片）。

### D10　事务超时只在写者这一处

每笔 `tokio::time::timeout(30 s + 每 MiB 8 s，上限 180 s)`——就是那段死代码 `publication_timeout_for` 的口径。超时按
Retryable 重试一次、再败按 lane 失败。两 lane 同一条。一次 WS 挂起 = 初始化永远收不了尾，2026-09-06 真发生过。

### D11　收尾核对不加库读

属性面照旧 `baseline_parse_matches`（pe 行数 − WORL vs 解析数）与 `baseline_parse_confirmed_empty`，在两 lane 排空后做；
模型面**不**数 `gen_root`——任务手里的根清单 + 每笔写者回执就是证据（ADR-071 D1）。

### D12　旧路同一切片内删掉，不留双路

`baseline_model_plan` / `load_baseline_nodes` / `baseline_work_items`；`BaselineOutcome` 回传 units 的那一半；
`execute_frozen_batch_body` 为 Initialize 拿 units 生成的那段；`finalize_baseline` 的「摄入完先推 applied」；
初始化路上的 `enqueue_model_watermark_write_behind`。`ingest_file` 对外签名保留（`baseline.rs` / 父层补缺还在用）。

## 后果

1. **初始化 = 解析 ∥ 生成 ∥ 两写者 → 核对 → 一笔收尾。** 任务返回即两面都在 RocksDB 里、两枚水位与写入标记同一笔。
2. **模型面开工不等属性面。** 两面的落库时间取 max 而不是相加；实测数见 plan。
3. **初始化不再把整库在投影里过一手。** 7997 补量：双 lane 单进程峰值 1,542 MB，其中元素缓存约 720 MB、只属性面
   481 MB、模型半边自身约 340 MB——投影那一截不在了，但 1.5 GB 的主体从来是元素缓存，压内存要走 zone plan 那条
   「缓存淘汰 / 跑完即弃」。
4. **write-behind 的三条已知缺陷不再落在初始化路上**（空转、无超时、`Blocked` 钉水位）；增量路上它们仍在，归 ADR-076。
5. **初始化期间读路由照旧落 Memory（空）**：UI 看不到正在初始化的库，与今天一致；完成后 `model_sesno == applied_sesno`
   → 读库。
6. 观测：任务 stage 多 `ingest_and_generate` → `drain_lanes` → `verify` → `task_end`；每条 lane 排空时一行日志
   （笔数 / 字节 / 耗时 / 冲突重试次数）；`/health` 不加字段。

## 实施步骤

见 `docs/plans/2026-09-14-dual-lane-initialization-dev-plan.md` S1–S6。

## 验收

- `cargo test --lib` 不退步（2026-09-14：2063 passed / 108 ignored，改动前基线 1904 + 另一会话合入 + 本轮新增；
  2026-09-15 复跑 2063 passed——源码钉 `the_halves_only_feed_the_lanes` 改成按行找函数尾，文件被编辑器存成 CRLF 时不再误报）。
- 源码钉：初始化编排不得出现 `ModelMemoryStore` / `ModelWriteBehind` / `load_baseline_nodes`；水位那一笔必须在两 lane
  `drain` 之后；解析 / 生成那两半不拥有写者、不清库、不推水位。
- `mem://` 单测：两 lane 并发往同库写、行都在；空库上发一根 → `gen_root` 恰一行且 `publication_status='ready'`；
  零几何根也有回执；单根 > cap → 死信不进队列；语句错记账、`StopWriting` 不再发后面的；收尾一笔在没有水位行的库上
  同时落 applied + model + `finished`（数据闸同一笔成立）。
- 真库（`initialize_ams_dbnums --with-model 8000`，全新 RocksDB，两轮）：零冲突、零超时；`gen_root` 952 全 `ready`；
  两枚水位 574、标记 `finished`；`READBACK` 六项与旧路逐字相同（D3 门，记 `docs/evidence/`）。
- 真进程（服务 release，全新 RocksDB，8000）：两 lane 在飞时 kill → 库里 `writing{0→574}`、无水位 → 重起：启动标记扫描排
  Reinitialize → 整库清 2000 行 → 重新初始化 7.8 s → 任务 `succeeded / in_sync`、952 根、水位 574/574、`finished` →
  再重起：8000 无任何任务（不排 ModelOnly）。记同一份 evidence。
- 大库（`initialize_ams_dbnums --with-model 7997`，全新 RocksDB，`watch_dbnums` 换成 `[7997]` 才点得到——前一晚
  「未找到」是这道门，不是 MDB）：6,772 根全 `ready`、零冲突、零超时、水位 184/184、`finished`；`COUNTS` 四项与旧路
  烟测 `READBACK` 逐字相同；峰值 RSS 1,542 MB（只属性面 481 / 关元素缓存 822），拆项见 evidence「AMS 7997」。
- **未做**：7999 / 1112 未量（同一道门 / 同号副本，见 evidence）。
