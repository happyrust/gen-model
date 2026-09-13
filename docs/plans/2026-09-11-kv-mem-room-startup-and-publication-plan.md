# kv-mem 房间初始化与逐根发布闭环开发计划

- 日期：2026-09-11
- 状态：Plannotator 已批准（决策 d-381）；P0–P3 已落地，P4 live / 发布包验收进行中
- 用户决策：`d-381`（取代草案阶段 d-379）——`room_membership=true` 时，kv-mem 必须像 external 一样先启动房间初始化，再进入模型根生成、房间归属计算与结果发布
- 依据：2026-09-11 spawned-mem 实测、Oracle 独立评估、ADR-073
- 当前工作树注意事项：`model_dbnum_ensure.rs` 及其 HTTP 路由是正在进行中的未提交改动；本计划以其现有 single-flight、冻结源和任务快照为基座，不另造第二套整库内存执行器

## 0. 本次评审需要拍板的目标形状

本计划把用户的“先启动房间初始化”解释为以下强语义：

1. `room_membership=true` 且介质为 `spawned-mem` / `embedded-mem` 时，每次进程启动面对的都是空结果库，因此必须先完成：
   - MDB 骨架；
   - 常驻房间输入模型；
   - MDB 全部可读 DESI 库的模型根生成；
   - 每根房间归属计算；
   - 模型行、两类房间边与根完成凭证向本进程 kv-mem 的原子发布。
2. 上述过程完成前可以提前开放 `/health` 供观察，但不得：
   - 把 `initialization.model_ready` 置为真；
   - 把 `features.room.status` 报成 `available`；
   - 让普通 `model/ensure` 或 `model/rebuild` 绕过启动屏障并制造“几何已完成、房间未计算”的根。
3. 初始化范围是 MDB 全部可读 DESI，不受 `watch_dbnums` 限定。监听限定只控制运行期增量，不改变房间候选参照系与开箱初始化完整性。
4. 初始实现按 dbnum 串行。它与 external 单消费者的顺序性一致，也避免 29 个库同时生成造成内存峰值。
5. `startup_autorun=false` 不跳过这道初始化。显式打开 `room_membership=true` 本身就是要求房间可用；若想快速启动，应关闭 `room_membership`。

如果“房间初始化”只指 resident 输入预热，本计划应在评审中被拒绝：当前代码已经完成该步骤，且实测 resident 为 215 间房 / 230 块面板，但端到端房间功能仍不可用。

## 1. 已确认的现状与断点

### 1.1 已工作的部分

- `run_read_through_core` 已在 MDB 骨架完成后调用 `prepare_room_resident_at_startup`。
- spawned-mem 实测：
  - `room_membership=true`；
  - resident `status=ready`；
  - 215 间房、230 块在册面板；
  - 230/230 面板有几何，网格预热 230/230。
- `ModelMemoryStore` 已保存模型记录、根回执、按来源索引、房间空间索引和房间变化摘要。
- `PersistTarget::process()` 已正确区分 `RocksDb`、`SpawnedMem` 与 `EmbeddedMem`；`persist_roots_batched` 能向三种介质写相同的模型行、房间边和 `gen_root` 回执。
- 正在开发的 `model_dbnum_ensure` 已提供：
  - 单 dbnum 固定源冻结；
  - 固定生成根集合；
  - `(kind, dbnum)` single-flight；
  - 任务进度与短期根快照；
  - memory-routed 整库异步执行入口。

### 1.2 端到端断点

1. `run_read_through_core` 在 resident 初始化后立即：
   - `configure_model_bootstrap(false)`；
   - `mark_data_ready_without_manifest()`；
   - `open_model_phase()`；
   - `mark_model_ready()`。
   它没有执行 MDB 全库模型与房间初始化。
2. `model_read_route::Memory` 同时承担了两种不同含义：
   - durable 库尚未就绪时，只允许进程内临时几何、禁止碰持久库；
   - kv-mem 本身就是当前进程的正式但非持久发布目标。
   这两种含义被 `scope_memory_only` 合并。
3. `model_refresh` 在 `memory_only` 下提前 `continue`，因此跳过：
   - `persist_roots_batched`；
   - `MemoryRoomCalculator::calculate_for_root`；
   - `room_relate` / `room_panel_relate`；
   - 数据库里的根完成回执；
   - 房间失败到逐根失败回执的映射。
4. `e3d.room.lookup` 只读 `SUL_DB` 的 `room_relate`。几何只在 `ModelMemoryStore` 时，lookup 必然为空。
5. durable 专用的 `/dbnums/{dbnum}/model/rebuild` 在 read-through 下仍向 batch queue 入队；`run_read_through_core` 没有 batch worker，实测任务永久停在 `queued`。
6. `features.room.status` 只因 SUL_DB 可连就报 `available`，没有综合 resident、启动初始化、逐根发布能力和执行器状态。
7. `ModelMemoryStore::RootReceipt` 在几何投影提交时已经出现；后续房间计算或 kv-mem 发布失败时，receipt 仍可能让整库 ensure 把该根判为 ready。

### 1.3 额外 correctness 风险

`model_db_adapter::room_stage` 在一组根写库前用共享 `rendered_ids` 跨根认领记录，而真正提交已改为逐根事务。若祖先根 A 认领共享记录后事务失败，嵌套根 B 可能不再携带该记录却成功盖章。扩大到启动全 MDB 初始化前，必须先补故障注入测试并消除“成功根依赖另一个尚未提交的根”。

## 2. 目标启动时序

### 2.1 room_membership=true

```text
连接 / 拉起 kv-mem
  → 安装内置 Surreal 函数
  → 构建 AiosDBManager 与 MDB 骨架
  → 提前启动只读 health 服务，状态 = room_initializing
  → prepare_room_resident_at_startup
  → resident ready；冻结本次启动的 room generation
  → 按 MDB databases() 顺序逐库执行 memory dbnum bootstrap
       冻结该库文件身份与 sesno
       枚举固定生成根集合
       生成最终模型投影
       按根计算房间归属
       每根一个 kv-mem 事务：
         模型行 + room_relate + room_panel_relate + gen_root 回执
       提交成功后写进程内 root publication ACK
  → 全部库成功
  → settle startup room barrier
  → mark_model_ready
  → features.room.status = available
  → plant-ui / ensure / rebuild 正常开放
```

### 2.2 room_membership=false

保持当前快速路径：

- 不建 resident；
- 不做启动全库模型生成；
- 模型按需只进 `ModelMemoryStore`；
- `model_ready` 可在骨架就绪后成立；
- `e3d.room.lookup` 可返回旧边供诊断，但状态必须为 `disabled`；
- 模型几何和模型水位不被房间开关阻断。

### 2.3 启动失败

- resident loading：health 显示 `initializing`，模型完整产出入口返回 503 `room_bootstrap_in_progress`。
- resident blocked / unsupported 且房间开关为真：启动屏障进入 `blocked`，不置 `model_ready`。
- 单库部分根失败：
  - 成功根保留；
  - 失败根不写 publication ACK；
  - 该 dbnum 任务终态为 `partial` 或 `failed`；
  - 项目级启动屏障不放行；
  - health 点名 dbnum、失败根样本与阶段。
- 服务保持可诊断，不因一个根失败直接退出进程。

## 3. 设计改动

### 3.1 把“读源路由”与“发布目标”拆开

保留 `ModelReadRoute { Memory, Database }`，它只回答模型从哪里读。

新增独立的发布策略，建议放在 `model_read_route.rs` 或新建 `model_publication_route.rs`：

```rust
enum ModelPublicationPolicy {
    ProjectionOnly,
    ProcessStore {
        medium: PersistMedium,
        durable: bool,
    },
}
```

判定规则：

- ingest + database-routed：`ProcessStore { durable: true }`；
- ingest + 基线未就绪 / 数据任务在飞：`ProjectionOnly`，继续保护 durable 初始化单写者；
- read-through + spawned-mem / embedded-mem：`ProcessStore { durable: false }`；
- 不允许通过把 read-through 改成 ingest 绕过问题。

`memory_only` 不再等价于“不写任何数据库”。它只表示“不写 durable 权威库”；是否向当前非持久 process store 发布由新策略决定。

### 3.2 让 memory dbnum ensure 复用逐根事务发布

调整 `ModelRefreshPolicy::generate_roots_report_on_source`：

1. 生成仍先提交结构化几何到 `ModelMemoryStore`。
2. 对 `ProjectionOnly` 保留当前跳过。
3. 对 `ProcessStore`：
   - 在任务边界冻结一次 room calculator；
   - 把本次根集合送入 `persist_roots_batched(PersistTarget::process(), ...)`；
   - 将房间计算失败、SQL 失败和事务失败逐根并入 `TargetedGenerationReport.failures`；
   - 不把失败根放入 `completed`。
4. spawned-mem 仍返回 `durable=false`；写进 kv-mem 不得被描述成可跨重启保存。
5. 读透不摄入 `pe` / `ATT_*` / `pe_owner`。需要验证 Surreal 在只有 relation 端点和模型行时的查询形状；若 `pe` 空节点导致 `$pe<-room_relate` 不可读，应改边读取谓词，不得为房间查询重新引入全属性摄入。

### 3.3 引入“完整根产出 ACK”，不要让候选 receipt 冒充成功

`RootReceipt` 继续表示几何投影候选与缓存身份，不直接承担“房间已发布”的新含义。

在 `ModelMemoryStore` 增加进程级、非持久的 `RootPublicationAck`：

```rust
struct RootPublicationAck {
    source_sesno: u32,
    room_membership: bool,
    room_generation: Option<String>,
    medium: PersistMedium,
}
```

规则：

- 几何重新生成、删除、源换代时先使旧 ACK 失效。
- 只有 `persist_roots_batched` 的该根事务成功后才写 ACK。
- `room_membership=true` 时，ACK 必须带本任务冻结的 resident generation。
- `room_membership=false` 时不要求 room generation。
- `model_dbnum_ensure::root_ready` 改为：
  - receipt 不早于冻结源；
  - 若要求房间，则 publication ACK 同源、同 resident generation；
  - 几何记录数与 receipt 相符。
- 失败后可保留候选几何供预览，但任务、health 与 ensure 回执必须明确 `complete=false`，下一次执行不能因旧 receipt 直接跳过。

这不是第三枚房间水位：它不落 durable 控制面，不参与 ingest 水位判决，只是当前进程中一个根的完整产出确认。

### 3.4 让整库 memory executor 支持启动屏障与强制房间补齐

扩展现有 `model_dbnum_ensure`，不新造第二套执行器：

- 增加启动调用形态，例如：

```rust
enum EnsurePurpose {
    OnDemand,
    StartupRoomBootstrap { room_generation: String },
    ForcedRebuild,
}
```

- `capture_snapshot` 继续冻结同一文件用于根枚举与生成。
- `StartupRoomBootstrap` 的 ready 判据使用完整根 ACK；几何已有但房间 ACK 缺失时，仍进入执行。
- 生成器可命中几何缓存，但必须重新走逐根 room + process-store 发布，不能只因 receipt 存在就收口。
- 为内部启动编排增加 `start_and_wait` / completion channel，不在启动主线轮询 REST 或 TaskRegistry。
- single-flight 扩展为同 dbnum 所有 memory 写者共用；单根 ensure、整库 ensure、文件刷新与 rebuild 不得并发写同一 dbnum。
- 任务 detail 加：
  - `purpose`；
  - `room_generation`；
  - `room_ready_roots`；
  - `failed_roots`；
  - `durable=false`。

### 3.5 在 run_read_through_core 建立项目级启动屏障

修改 `run_read_through_core`：

1. 在任何可对外模型生成前，根据 `room_membership` 配置 `full_model_required`。
2. health 可提前启动，但初始化状态先进入 `room_initializing`。
3. resident 完成后，从 `MdbMemoryStore::databases()` 取 MDB 全部已加载 DESI，按 dbnum 排序。
4. 逐库调用 `model_dbnum_ensure::start_and_wait(... StartupRoomBootstrap ...)`。
5. 累计项目级进度：
   - dbnums total / done / failed；
   - roots total / ready / failed；
   - 当前 dbnum 与阶段；
   - room edges written；
   - elapsed / RSS。
6. 仅全部成功时 `settle_startup_rooms`、`mark_model_ready`。
7. 失败时保持 health 与 ops 可用，不打印“启动完成”横幅。

`InitializationCoordinator::mark_model_ready` 必须同时检查 `!startup_rooms_pending`。当前只检查 data、model gate 与 startup reconcile，无法表达本计划的硬前置。

### 3.6 修正 rebuild 路由

`POST /api/v1/dbnums/{dbnum}/model/rebuild` 按 model route 分派：

- Database：保留 durable `model_rebuild::start` 与 batch queue。
- Memory：调用 `model_dbnum_ensure` 的 `ForcedRebuild`，不得向没有消费者的 durable queue 入队。
- 启动房间屏障尚未完成：
  - 同一 dbnum 的请求复用启动任务并返回其 task id；或
  - 返回 409/503 并指向启动 task。
  两种形态只选一种；建议复用同一 single-flight 任务。

保留 `/dbnums/{dbnum}/model/ensure` 作为 memory 专用整库确保接口；两条端点必须共享执行器和同库准入，不能形成两个竞争写者。

### 3.7 修正 health、query 与能力声明

保留现有版本三元组语义：

- read-through + enabled：`published_version=null`、`stale=null`、`publication_status=unsupported`；
- disabled：`null / null / disabled`；
- ingest：继续按 `model_sesno` 与在飞任务给 `current/updating`。

新增与版本语义正交的计算状态：

```text
room_membership                 配置开关
room_build.resident.status      输入模型状态
room_bootstrap.status           idle / initializing / ready / partial / blocked
room_bootstrap.progress         dbnum 与根进度
room_publication.status         available / unavailable
room_rebuild_executor           memory / durable / unavailable
```

`features.room.status=available` 只有在以下条件都成立时出现：

- 配置开启；
- resident ready；
- startup room bootstrap ready；
- process store 可连接；
- memory/durable 对应执行器可用。

`e3d.room.lookup` 增加 `calculation_status` 或等价字段，至少区分：

- `ready_nonempty`；
- `ready_empty`；
- `not_computed`；
- `initializing`；
- `failed`；
- `disabled`。

不得把 `publication_status=unsupported` 改成 `ready` 来承载这个区别。

### 3.8 消除跨根未提交依赖

第一版以正确性优先：

- 去掉 `room_stage` 的组级 `rendered_ids` 跨根认领；
- 每个根事务携带自己的完整记录与房间边；
- 相同 record id 的重复 UPSERT 依靠幂等语义收敛；
- 对账与回执仍按根自己的全集。

后续若性能证明重复写不可接受，再增加“仅对已经成功提交的共享记录复用”优化；不能在事务成功前把认领传播给别的根。

## 4. 开发里程碑

### P0：冻结当前 WIP 基线并先钉红测试

1. 先合并或明确接管当前未提交的 `model_dbnum_ensure`、handler 和 route 改动，避免重写同一文件。
2. 加接口诚实护栏：
   - memory route 的 `/model/rebuild` 不得进入 durable queue；
   - 执行器不可用时不返回会永久 queued 的 202；
   - `features.room` 不得仅凭 SUL_DB connected 报 available。
3. 加四条稳定红测：
   - 空 kv-mem + room enabled + 已知室内根：几何生成后 lookup 必须非空；
   - room enabled 但 resident blocked：根不得 complete；
   - memory rebuild 必须进入 running/terminal，不能 queued 无消费者；
   - 祖先根事务失败、嵌套根随后成功时，嵌套根仍有完整行与房间边。

### P1：发布策略与完整根 ACK

1. 拆读源与发布目标。
2. `ModelMemoryStore` 增加 publication ACK 及失效规则。
3. `model_refresh` 在 volatile process store 下调用 `persist_roots_batched`。
4. 房间失败与数据库失败逐根回传并保持可重试。
5. 去掉跨根未提交去重。

P1 完成门：单 dbnum memory ensure 可在真实 `mem://` 和 spawned-mem 上完成 `root → model rows → room edges → ACK → lookup`。

### P2：kv-mem 启动房间初始化屏障

1. 扩展 `model_dbnum_ensure` purpose 与 completion channel。
2. `run_read_through_core` 串行跑 MDB 全部 DESI。
3. `InitializationCoordinator` 把 startup rooms 纳入 `mark_model_ready`。
4. health 暴露项目级进度与失败。
5. `room_membership=false` 快路径对拍不变。

P2 完成门：每次 spawned-mem 冷启动都从空库完成全 MDB 房间初始化后才 model_ready。

### P3：端点与客户端契约

1. `/model/rebuild` 按 route 分派。
2. `/model/ensure`、整库 ensure 与启动任务共享同库 single-flight。
3. query 增加 calculation status，但保留 read-through 的版本三元组。
4. health capabilities 区分 worker 与 memory executor。
5. 更新 Web API 文档、CONTEXT 与 ADR-073 的模式说明。

### P4：live、性能与发布包

1. 隔离端口跑 AMS spawned-mem 全流程。
2. 固定一个明确室内正例和一个室外负例；正例需记录 `room_refno / panel_refno`，不能只凭历史名称猜。
3. 记录：
   - resident 时间；
   - 各 dbnum 根数、房间边数、耗时；
   - 总启动时间；
   - 峰值 RSS；
   - 最大单根事务字节；
   - partial / failed 根样本。
4. 浏览器验证 plant-ui 在 `model_ready` 前显示初始化进度，完成后房间查询可用。
5. 重新生成 runtime 包并做一次真正开箱验证。

## 5. 测试矩阵

### 5.1 纯单元

- 发布策略真值表：
  - ingest database；
  - ingest temporary memory fallback；
  - read-through spawned-mem；
  - read-through embedded-mem。
- ACK：
  - 同源同 room generation 才 ready；
  - 几何重生成、删除、resident 换代后失效；
  - publish 失败不产生 ACK；
  - room disabled 不要求 room generation。
- 初始化状态：
  - startup rooms pending 时 `mark_model_ready=false`；
  - settle 后才可 ready；
  - blocked 保留可诊断状态。
- rebuild route：
  - memory 不碰 batch queue；
  - database 仍走 durable queue；
  - 并发请求只创建一个任务。
- query 状态：
  - ready empty 与 not computed 分开；
  - read-through 始终保留 `unsupported` 版本状态；
  - disabled 不冒充当前。
- 跨根事务失败故障注入。

### 5.2 mem 集成

- 完全空的 mem，不预建 `pe`：
  - 一个明确室内构件；
  - 一个多归属构件；
  - 一个室外构件；
  - lookup 与直接查 `room_relate` 一致。
- 模型行、两类房间边与 `gen_root` 同事务：
  - 模型 SQL 失败；
  - 房间 SQL 失败；
  - COMMIT 失败；
  - 三种都不能留下完整 ACK。
- 房间输入 loading / blocked / unsupported。
- `room_membership=false`：
  - 模型成功；
  - 不写新边；
  - 删除来源仍清旧边。
- false → true：
  - 旧几何 receipt 在；
  - publication ACK 缺；
  - ForcedRebuild 必须补边，不能缓存命中后直接成功。
- kv-mem 进程重启：
  - 模型、边与 ACK 全失效；
  - 下一启动重新初始化；
  - 不报告 durable。

### 5.3 HTTP / live

- `/health` 状态序：
  - core ready；
  - resident initializing；
  - room bootstrap running；
  - model ready / room available。
- `/dbnums/{dbnum}/model/ensure`：
  - 固定 roots 总数；
  - `units_done` 单调；
  - 终态可查询；
  - roots ready 判据包含房间 ACK。
- `/model/rebuild`：
  - memory task 真执行；
  - `events_seen>0`；
  - 最终 terminal；
  - 不出现 durable queue 行。
- 单根 `/model/ensure force=true`：
  - 返回前房间完整产出已提交；
  - `durable=false`；
  - lookup 与边一致。
- 源文件在初始化期间变化：
  - 旧冻结任务不得盖成新源 ready；
  - 下一轮按新源重跑。
- persistent external / spawned-rocksdb 对照不退化。

## 6. 预计修改文件

- `src/lib.rs`
  - read-through 启动屏障与 MDB 全库串行编排。
- `src/data_interface/initialization_phase.rs`
  - startup rooms 进入 `mark_model_ready` 硬门与进度状态。
- `src/data_interface/model_read_route.rs`
  - 拆读源与发布目标，或迁出独立 publication route。
- `src/data_interface/model_refresh.rs`
  - volatile process-store 发布、逐根失败与 ACK。
- `src/data_interface/model_dbnum_ensure.rs`
  - purpose、强制补房间、completion channel、启动调用。
- `src/fast_model/model_memory_store.rs`
  - RootPublicationAck、失效与查询。
- `src/fast_model/model_db_adapter.rs`
  - ACK 回调边界、跨根去重修复、故障注入。
- `src/web_service/handlers.rs`
  - route-aware rebuild、health capability、room bootstrap 进度。
- `src/web_service/mod.rs`
  - 路由契约测试。
- `src/query_service.rs`
  - calculation status 与 lookup 完整性。
- `src/options.rs`
  - 仅在需要显式启动并发/超时参数时修改；不增加房间水位或独立流程开关。
- `CONTEXT.md`、ADR-073、Web API 文档、release notes。

## 7. 验证顺序

1. 每一里程碑先跑对应精确测试。
2. 格式与静态检查：

```powershell
cargo fmt --all -- --check
cargo test --lib model_read_route
cargo test --lib model_dbnum_ensure
cargo test --lib model_memory_store
cargo test --lib model_db_adapter
cargo test --lib initialization_phase
cargo test --lib query_service
```

3. P3 后跑全库：

```powershell
$env:CARGO_TARGET_DIR='D:\Rust\target-portable-0.1.21'
cargo test --lib -- --skip damaged_
```

4. P4 使用隔离目录、隔离 HTTP / Surreal 端口和外部 AMS 项目只读源；不得复用当前 9099 / 8009。
5. 生成发布包后再次从 zip 解压，不用源码目录资源，完成浏览器开箱验证。

## 8. 风险与回退

### 8.1 启动时间与内存

AMS 7999 单库已有 4435 个权威根，全 MDB 明显更大。初版串行可控但可能让首次 ready 等待数分钟。不得用固定短超时把合法慢任务判失败；必须持续上报进度。性能优化在正确性验收之后。

### 8.2 双份模型存储

kv-mem 模式下模型同时存在 `ModelMemoryStore` 和 process SUL_DB。前者仍是模型 API 读源，后者承载统一 SQL 模型行、房间边与数据库函数。health 必须明确 `durable=false`，避免把“写进数据库”误解为“跨重启保存”。

### 8.3 候选几何可见窗口

几何生成后、完整根事务提交前，候选几何可能已在内存。允许预览，但必须在 API 中标明未完成；不得让 receipt、ready roots 或启动完成门把它当完整产出。

### 8.4 结构变化传播

ADR-073 U1 中房间/面板自身改名、换属主等不改成员几何的传播仍是独立问题。本计划只保证：

- 冷启动从零完整初始化；
- 被生成/重建根的房间归属正确；
- 已知删除和刚体/普通重生成走相同逐根规则。

不得借本计划恢复旧房间流水线。

### 8.5 回退

功能仍受现有 `room_membership` 开关保护。若性能或现场兼容性不满足，关闭该键应恢复当前快速模型模式；不需要数据迁移。已经发布的 runtime 包不得默认打开该键，直到 P4 全部通过。

## 9. 最终验收清单

- [x] room enabled 的 kv-mem 启动代码路径先 resident、再全 MDB 根、再 model_ready（live 待 P4）。
- [x] room disabled 的启动和按需模型行为定向测试不变。
- [x] 空 kv-mem 不摄入属性表也能写出可查询的房间边。
- [x] 一个成功根的模型行、房间边和 DB 回执在同一事务。
- [x] 内存完整 ACK 只在事务成功后出现。
- [x] 已计算为空与未计算在接口上可区分。
- [x] read-through 的版本三元组仍是 `null / null / unsupported`。
- [x] memory rebuild 不再进入无消费者的 durable queue。
- [ ] external / spawned-rocksdb 的 batch worker 与水位语义不退化。
- [x] 无独立房间队列、无第三枚房间水位、无全属性摄入。
- [x] 跨根事务失败不会产生不完整成功根。
- [ ] AMS 正例、负例、重启、并发和故障注入全部通过。
- [ ] runtime zip 浏览器开箱验证通过。

## 10. 明确不做

- 不通过启动 durable batch worker 来“顺手解决” kv-mem。
- 不把 `publication_status=unsupported` 改成 `ready`。
- 不新增 `room_sesno`、房间独立队列或旧式项目级房间流程。
- 不让 `watch_dbnums` 缩小房间候选参照系或开箱初始化范围。
- 不为房间查询重新摄入全量 `pe` / `ATT_*` / `pe_owner`。
- 不在本计划中解决 ADR-073 U1 的所有结构传播问题。

## 11. 落地记录

P0 基线固定为 `refactor/retire-publish-stage@b96436c2b` 上的共享 WIP；接管现有未提交
`model_dbnum_ensure` / handler / route，不 reset、不提交其它会话的 release、setup、房间或写入重构。
先补 6 条红门，确认实现前 6/6 失败：启动房间屏障、读/写路由拆分、完整 ACK、health 诚实状态、
memory rebuild 分派、祖先根失败后的嵌套根完整性。

P1–P3 实现：

- `ModelPublicationPolicy` 将读源与发布目标拆开；read-through kv-mem 走
  `ProcessStore{durable:false}`，ingest 临时 memory fallback 保持 `ProjectionOnly`。
- `RootPublicationAck` 绑定 dbnum、源会话、几何条数、房间开关、resident generation 与介质；
  候选投影变动、清库、恢复、receipt 失效时同步失效。逐根 process-store 事务成功后才写 ACK。
- 删除 `room_stage` 的跨根未提交认领；每根携带完整记录，重复 id 由幂等写收敛。故障注入证明祖先根
  事务回滚后嵌套根仍独立写下模型行、房间边和回执。
- `model_dbnum_ensure` 增加 on-demand / startup / forced 三种 purpose、completion 通知与进度；
  ready 由 receipt + 记录数 +（需要时）完整 ACK 共同判定。
- `run_read_through_core` 在 HTTP 可观测后建立硬屏障：resident → MDB 全部 DESI 按 dbnum 串行
  `start_and_wait` → 全成功才 `mark_model_ready`。health 暴露库/根进度、边数、耗时、RSS 与最大事务字节。
- `/model/rebuild` 按 route 分派；单根 ensure 在启动屏障或同库整库任务期间不并发写。
- `e3d.room.lookup` 改直接按 relation.out 读取，不依赖空 kv-mem 中存在 `pe` 行；新增
  `calculation_status`，版本三元组不变。`model/records` 增加 `complete/incomplete_roots`。

验证记录随 P4 最终结果继续补充。
