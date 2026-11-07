# 数据库架构版本（`storage_version:current`）收尾与升级流程计划 · 2026-10-28

依据是同日两份只读侦察（RECON-NEXT-STEP、PLAN-CLOSURE）与 Plannotator 三轮批注（§9）。所有结论带 `file:line`；行号以 2026-10-28 的工作树为准（gen-model `refactor` @ `0de678915`，rs-server `new_standard` @ `af40970`，vendored core `old/vendor/old-aios-core` @ `5f6d004`；`gen-model/vendor/shared` 是指向 `old/vendor` 的 JUNCTION）。

本文件只是计划，不改代码。工作区里 generator 与 NameChanged 两条工作线的未提交改动不在范围内，任何工作包不得触碰 `generator_reconcile.rs` / `generator_identity.rs` / `manual_update.rs`（新逻辑只调它们已有的 pub 接口）。

## 0. 一句话

**`storage_version:current` 是整个数据库的架构版本，只在进程启动时做一次全局核对，不和任何表、任何查询耦合。** 不匹配 → gen-model（aios-database）在启动日志、`/health`、运维面板上提示并提供一键升级（可指定 dbnum，默认全部；升级 = 逐库清旧数据 → 重新解析 → 重新生成模型，复用现有 `intent=reinitialize`），全部库完成后才写版本记录；rs-server 启动时同样全局核对，不匹配则拒绝启动并提示用 aios-database 升级数据库。**ptset 侧的版本检测（surql THROW、aios_core 读点集时的 check、rs-server 的 409 映射）全部移除。** 代码侧先补 `/health` 可刷新和配置关闭改 409，再做升级流程、写入点迁移、ptset 检测移除。

## 1. 现状与依据

### 1.1 语义（三轮批注后）

- 代码与文档目前把它写成「模型面行形状版本」并把检测挂在读点集路径上（`storage_version_reconcile.rs:1-30`、`task_end.rs:173-178`、`gy_common.surql:10-14` `:25-39`、ADR-091 标题与 `:86-92` `:107-109`）。批注：版本指**整个数据库的架构**；**不要和 ptset 做版本检测**，启动时做全局检测，不对就提示用户用 aios-database 升级。
- 记录形状不变（整库一条、`version / shape / rebuild`）；改的是谁写、何时写、在哪核对、不匹配怎么办。
- 现状写入点与语义冲突：任一 dbnum 的模型水位盖章就写（`task_end.rs:153-178`）。

### 1.2 写侧（现状）

- `render_watermark_update` 在 `end.model_stamps()` 为真时置 `stamps_storage_version = true`（`src/data_interface/task_end.rs:153-178`），语句由 `render_storage_version_record()` 生成（`:239-250`），排在水位 `UPDATE` 之前（`:223-229`；`:381`、`:426-430`）。迁走后 `:1347-1394` 的「五句」计数回到四句，`:1363` / `:1375` 文案同步。
- 行上不带版本：`ptset_rows.rs:21-24`、`:301-305`；`model_db_adapter.rs:9430-9433`。继续有效（文案要改，见 C8）。

### 1.3 核对与读侧（现状，其中 ptset 耦合的部分将移除）

- gen-model 启动核对（保留并加强）：`report_and_log`（`storage_version_reconcile.rs:129-149`）启动调一次（`startup_reconcile.rs:411-426`）；`LAST_REPORT` 是 `OnceLock`（`:118-132`）；`/health.storage_version` 从它读（`handlers.rs:264-274`），不重启不刷新。运维面板内嵌 `web/ops.html`（`handlers.rs:282`，磁盘副本优先 `:467-471`）。
- rs-server 启动（要新增核对）：只有连通性探针 `verify_external_rocksdb`（`rs-server/src/db_mode.rs:64-99`，`main.rs:192-198`）；测试闸门 `init_test_db` `:121-147`。
- **ptset 耦合的检测（将移除，C8）**：
  - surql：`resource/surreal/gy_common.surql:25-39` 的 `fn::pe_ptset_of` 里 `:29` 手抄常量、`:31` 读记录、`:34-36` THROW；`:10-14` `:23-24` `:27-28` 注释。
  - aios_core：`rs_surreal/geom.rs` `ptset_map_projection` 里的版本投影与 `query_*_point_map` 的 `check(dbnum, stored)` → `ModelStorageVersionMismatch`（本轮因 root 绑定读不到文件，按合入说明）。错误类型本身保留给启动核对用，`dbnum` 字段与「该库需 POST …/model/rebuild」文案要改成全局语义。
  - rs-server：`error.rs:47-53` `StorageVersionMismatch` 变体、`:56-104` `from_anyhow` / `parse_mismatch_text`、`:106-124` 409 映射，及它们的测试 `:134-200`；`dq_bran.rs` / `heat_dissipation.rs` 里接了 `Error::from_anyhow` 的调用点（上一轮 grep 命中）。启动即拒绝后运行期不会再出现这个错误，这套映射成为死代码，删。
  - gen-model 测试里为 THROW 铺的底：`model_db_adapter.rs:9461-9471` 在内存库里写记录让长度函数通过——移除 THROW 后不再需要。
- 运行中的 aios-database（12:36 启动的旧二进制）把旧的两层版 `fn::pe_ptset_of` 灌回了库；函数体随二进制快照走（`embedded_surql.rs:47-50`、`:69-87`），换二进制重启即被改回。

### 1.4 升级可以复用的现有路径

- 「重新解析」已有单库入口 `POST /api/v1/dbnums/{dbnum}/data/reinitialize`（`handlers.rs:3379-3421`，路由 `mod.rs:688`）：`enqueue_manual_update_with(project, Some(mdb), Some(&[dbnum]), ManualEnqueueOptions { reinitialize: true })`（`:3404-3412`；`manual_update.rs:4551-4557`），worker 冻结点复核后 `wipe_dbnum_for_reinit` 整库清空（`fast_delete.rs:817-821`）→ 首次导入 → 整库模型 → 房间（`:3383-3384`）。约束：`watch_scope` 外 400（`:3394-3398`）；读透形态 409（`:3399-3403`）；`reinitialize` **只许与显式 dbnum 名单同用**（`batch_scheduler.rs:244-249`）——「默认全部」要由升级流程先解名单。
- 任务终态：`TaskRegistry::global().get(task_id)` + `state.is_terminal()`（`task_registry.rs:100-105`）；用法见 `model_rebuild.rs:36-45`。
- `mark_model_ready`（`initialization_phase.rs:793-807`，`batch_worker.rs:5078-5084`）单独不能当写入条件：老库按水位也是「追平」。
- `POST /api/v1/dbnums/{dbnum}/model/rebuild`（`mod.rs:608-609`；`handlers.rs:3214-3249`；`model_rebuild.rs:69-138`）保留作日常工具，不是升级手段。`model_full_rebuild_enabled=false` 落 500（`:70-72` → `mod.rs:263-287`）。

### 1.5 测试

- rs-server `extraction_fixes_7997_test.rs`：`ptset_read_is_versioned_7997`（`:229-258`）断言的正是读点集时的 THROW/拒绝——**随 ptset 检测移除而删**；`ptset_alignment_7997`（`:265-303`）保留，它只读点集，前置条件改为「库已升级到当前架构版本」（否则 rs-server 根本起不来，`init_test_db` 也会拒）。今天它红是因为库里没有记录。
- rs-server `error.rs:134-200` 五条 409 映射测试随映射删；换成 `db_mode` 里启动核对的纯函数测试。
- gen-model：`storage_version_reconcile.rs:163-206`；`task_end.rs:1347-1394`；`model_db_adapter.rs:9444-9471`。缺：`/health` 刷新、配置关闭状态码、升级流程、写入点。

### 1.6 库内实测（Lead，2026-10-28）

- `pe` 229434、`pe_owner` 324841、`inst_relate` 51624、`pe_ptset` 13276（旧形状）；`storage_version:current` 不存在。SurrealDB 2.1.4 fork `127.0.0.1:8009`，ns `1516`，db `AvevaMarineSample`；aios-database `0.0.0.0:8022`。

### 1.7 文档

- ADR-091 标题、`:86-92`、`:99-103`、`:107-109`（409 + THROW）、`:117-122` 都要改；文件名含 `per-dbnum`。`docs/plans/2026-10-27-…-plan.md:184-193`（两层表述、409 映射、A5 两条用例）。`docs/specs/web-service-api.md:547` state 枚举过时、无 `/health.storage_version`、无升级端点。

## 2. 目标与完成定义

1. 版本核对只有一种：进程启动时读 `storage_version:current` 与 `MODEL_STORAGE_VERSION` 比。gen-model 不匹配 → 日志 + `/health.storage_version.upgrade_required = true` + 面板横幅与按钮，启动不阻塞；rs-server 不匹配 → 拒绝启动，错误文本含稳定码和「请用 aios-database 升级数据库」。
2. 读点集路径（surql 长度函数、aios_core `query_*_point_map`、rs-server 消费点）不再做任何版本判断；没有行就是 NONE / 空集。
3. 一键升级：`POST /api/v1/storage-version/upgrade`，体 `{dbnums?: [u32]}`，缺省 = 范围内全部设计库；逐库排 `reinitialize`；进度在 `/health.storage_version.upgrade` 与 `/tasks`；全部请求的库 `Succeeded` 且请求集 = 全范围时才写 `storage_version:current`；部分库升级不写。
4. `storage_version:current` 不再随单库水位盖章写；全新库首次初始化完成时自动写（D-7）。
5. `/health.storage_version` 可刷新；`model_full_rebuild_enabled=false` 是 409。
6. 本机库走完：老库被点名 → 面板一键升级 → 记录写上 → `/health` 绿 → rs-server 正常启动 → `ptset_alignment_7997` 绿。
7. ADR-091 二次修订落档；文档与代码一致。

## 3. 阶段与依赖

| 阶段 | 内容 | 依赖 | 性质 |
|---|---|---|---|
| P0 | V0 固定老库证据；D-7 / D-8 / D-9 拍板 | 无 | 只读验证 + 拍板 |
| P1 | C2 health 可刷新；C3 配置关闭改 409（SV-C2 / SV-C3 工单已出；SV-C1 撤回） | 互不依赖 | 需改代码 |
| P2 | C7 升级流程；C4 从 task_end 移除写入；C5 rs-server 启动核对；C8 移除 ptset 侧版本检测（surql + aios_core + rs-server 映射 + 测试）；C6 文案 | C7 用 C2 的 `refresh_if_stale`；C4 动 `task_end.rs`，排在 generator 线收口后；C4 与 C7 同批发版；C8 跨三个仓，aios_core 先发 | 需改代码（待出工单） |
| P3 | V1–V4 现场验证 | P1 + P2 部署 | 只读验证（V1 走升级入口写库） |
| P4 | W1–W4 文档与测试文案 | P2 定稿 | 文档 |

现场验证排在 P2 之后：现状代码下对单库 rebuild / reinitialize 都会在水位盖章时写出语义错误的记录。

## 4. 工作包

### P0

**V0 · 固定「老库」证据**：`SELECT * FROM storage_version:current;` → 空；`GET http://127.0.0.1:8022/health` 记下 `storage_version`（预期 `stored: null, caught_up: false`）；`SELECT count() FROM pe_ptset GROUP ALL;` 记下 13276 作升级前基线。不再验 THROW（它要被移除）。

### P1 · 代码（工单已出）

- **C2** `/health` 可刷新（`storage_version_reconcile.rs` + `handlers.rs`）：`OnceLock` → `RwLock<Option<_>>`；`needs_refresh` / `refresh_if_stale`；`health` 先刷再读。C7 写入记录后也调它。
- **C3** `model_full_rebuild_enabled=false` → 409 `unsupported`（`model_rebuild.rs` + `web_service/mod.rs`）。
- **SV-C1 撤回**：它钉的是 surql 里的版本常量与 THROW，两者都要移除（C8）。

### P2 · 代码（待出工单）

**C7 · 升级流程**（新模块 `src/data_interface/storage_upgrade.rs` + `web_service/mod.rs` 路由 + `handlers.rs` 一个 handler + `web/ops.html`；不改 `manual_update.rs`）

- 端点 `POST /api/v1/storage-version/upgrade`，体 `ProjectReq` 身份字段 + `dbnums: Option<Vec<u32>>`。缺省名单 = 范围内全部设计库（MDB 闭包的 DESI 库 ∩ `watch_scope`），解出显式名单再传（`batch_scheduler.rs:244-249`）。读透形态 409（同 `handlers.rs:3399-3403`）。
- 执行：`enqueue_manual_update_with(project, Some(mdb), Some(&list), ManualEnqueueOptions { reinitialize: true })`（`manual_update.rs:4551-4557`），`arm_auto_work()`（同 `handlers.rs:3414`）；202 回 `{target_version, requested, receipt}`（`receipt` 沿用 `ManualEnqueueReceipt`，`batch_scheduler.rs:253`）。
- 进度记录 `storage_upgrade:current = { target_version, requested, done, failed: [{dbnum, reason}], started_at, finished_at }`，持久在库里。完成判定：每个请求 dbnum 的 reinitialize 批次到 `Succeeded`（`task_registry.rs:100-105`）；判定点放 worker 空闲轮或任务终态回调，不在 handler 里等。
- 写入点：`done == requested` 且 `requested == 全范围名单` 时 `UPSERT storage_version:current CONTENT {version, shape, rebuild}`（复用 `StorageVersionRecord::current()`），然后 `refresh_if_stale()`；部分名单只更新进度记录。
  **2026-10-29 演练修正**：这里的「全范围」必须是 **MDB 声明的全部设计库**，不是
  `MDB ∩ watch_dbnums`。否则 `watch_dbnums=[8000]` 只升级 8000 就会写整库版本记录，
  把其余仍旧形状的库一块冒充成新版。监听限定进程永远不写版本记录。
- `/health.storage_version` 增加 `upgrade_required: bool`、`upgrade: { state: idle|running|failed|done, requested, done, failed, started_at }`。
- 面板 `web/ops.html`：`upgrade_required` 为真显示横幅（库里版本 / 期望版本 / 将清空并重建的库清单）+「全部升级」按钮 + 可选 dbnum 输入；二次确认后 POST；运行中显示进度。
- 启动日志（`startup_reconcile.rs:414-421`）改为「数据库架构版本不匹配（库里 X / 期望 Y）：请在运维面板或 `POST /api/v1/storage-version/upgrade` 升级，升级会清空并重新解析、生成以下库：[…]」。
- 测试：名单解析（缺省 = 全部；显式名单越界 400）；完成判定纯函数（部分完成不写、全部完成且全范围才写、有 failed 不写）；handler 源码钉（走 `enqueue_manual_update_with` 且 `reinitialize: true`）。

**C4 · 从 task_end 移除写入**（`task_end.rs`）：删 `:149-151`、`:173-178`、`:223-229`、`:232-250`；`:1347-1394` 改四句；补反向钉「task end SQL 不含 storage_version」。与 C7 同批发版；排在 generator 线收口后。

**C5 · rs-server 启动核对**（rs-server `src/db_mode.rs`、`src/main.rs`）：`verify_external_rocksdb` 探针之后（`db_mode.rs:94-96`）读 `SELECT version FROM ONLY storage_version:current`，与 `aios_core::model_storage::MODEL_STORAGE_VERSION` 比；不匹配 `bail!`，文本 = 稳定码 + 库里/期望版本 + 「请用 aios-database 升级数据库（运维面板或 `POST /api/v1/storage-version/upgrade`）」；`init_test_db`（`:121-147`）走同一条路。纯函数 `storage_version_verdict(stored: Option<u32>, expected: u32) -> Result<(), String>` + 三态单测（缺记录 / 旧版 / 新版都拒）。调试跨版本启动见 D-8。

**C8 · 移除 ptset 侧版本检测**（三个仓，aios_core 先发）

- aios_core（vendored `rs_surreal/geom.rs`、`rs_surreal/model_storage.rs`）：`ptset_map_projection` 只投 `pe_ptset` 行，不带版本；`query_*_point_map` 不再调 `check`；`ModelStorageVersionMismatch` 保留给启动核对，去掉 `dbnum` 语义（或改为可缺省），Display 和 `rebuild_hint()` 改成「请用 aios-database 升级数据库」；`StorageVersionRecord::current()` 的 `shape` / `rebuild` 文案改成整库语义。
- gen-model：`gy_common.surql:25-39` 回到单语句 `select value ptset from only type::thing('pe_ptset', record::id($pe))`，删 `:10-14` `:23-24` `:26-36`；`model_db_adapter.rs:9461-9471` 删；`ptset_rows.rs:21-24` 文案改；`embedded_surql` 不加钉。
- rs-server：`error.rs:47-53` 变体、`:56-104`、`:106-121` 409 分支与 `:134-200` 测试删；`dq_bran.rs` / `heat_dissipation.rs` 里 `Error::from_anyhow` 调用点恢复原来的兑底；`extraction_fixes_7997_test.rs:216-258` 删，`:265-266` ignore 理由改为「库已升级到当前架构版本」，`:35-40` 模块注释同步。

**C6 · 文案**：`storage_version_reconcile.rs:1-30` 模块注释、`rebuild_hints`（`:57-65`）改成升级入口。

### P3 · 现场验证（P1 + P2 部署后）

- **V1** 启动 aios-database：日志有升级提示；`/health.storage_version.upgrade_required == true`；面板有横幅。点「全部升级」（或 `POST /api/v1/storage-version/upgrade` 体 `{"project":"AvevaMarineSample","mdb":"ALL","namespace":"1516"}`）→ 202；`/tasks` 里每个库一条 `intent=reinitialize`。同时启动 rs-server 必须被拒，错误文本含升级提示。
- **V2** 过程中 `storage_version:current` 仍空、`storage_upgrade:current.done` 逐个增长；全部 `Succeeded` 后记录出现；`SELECT count() FROM pe_ptset GROUP ALL;` 与抽样一行（无 `storage_version` 字段）；`RETURN fn::pe_ptset_of(pe:⟨24381_100817⟩);` 返回对象或 NONE，永不 THROW。
- **V3** 不重启 `GET /health` → `caught_up: true`、`upgrade.state: done`；rs-server 启动正常；反向：手工 `DELETE storage_version:current;` 后启动 rs-server 必须拒绝，验完写回。
- **V4** `cargo test --bin rs-server extraction_fixes_7997 -- --ignored --nocapture`：`ptset_alignment_7997` 绿；`cargo test --bin rs-server -- --test-threads=1` 全绿。

**2026-10-29 丢弃库演练**：V1–V3 的等价路径已在独立 SurrealDB（`18009`，memory 后端，
`watch_dbnums=[7352]`）上跑通，未触碰现场 8009/8022：启动点名 → 一键升级 → 写
`storage_version:current` → `/health` 转绿 → rs-server 启动；删记录后 rs-server exit 1 拒绝，
`warn` 模式只告警。证据与复现见 `docs/verification/storage-version-upgrade-rehearsal-2026-10-29.md`。
真库规模（20 个主项目归属设计库）升级已于同日下午在 `ams-full3` 副本上验证通过，见下条。

**2026-10-29 旧库副本部分升级**：在 `ams-8009-fresh` 的副本（7997 / 8000 有真实数据、无版本记录，
MDB 声明 71 库 / 29 个元素库）上显式 `dbnums:[8000]`：202、`full_scope=false`，约 42 秒升级完成（任务 37.4 秒），
`storage_version:current` 仍空，rs-server 仍 `exit 1` 拒绝——部分范围不写整库记录，符合 D-1/D-4。
证据见 `docs/verification/storage-version-upgrade-rehearsal-2026-10-29.md` §3.5。

**2026-10-29 旧库副本全范围升级（通过，V1–V3 在真实数据规模上补齐）**：同一副本全量
请求（不带名单）`requested=20 / full_scope=true`（范围修正：MDB 声明的 29 个元素库中
9 个归属外部项目、不在本持久库数据域，`declared_dbnums` 按主项目归属收窄为 20），
约 23 分钟全部 `Succeeded`：`storage_upgrade:current` = `done=[20] failed=[]` →
`storage_version:current` 落盘 `version=1` → `/health` `caught_up=true /
upgrade_required=false` → rs-server（18023）`stored=1 expected=1` 正常监听。
真库规模的「升级→写记录→rs-server 转绿」链路至此完整验证。代价记录：宿主机
`63GB RAM + 23GB pagefile ≈ 86.4GB` commit 硬上限、基线 ~78GB，前 4 轮被系统内存压力
随机带走（含现场 8009/8022 两个服务，均已恢复）；成功轮利用现场 aios 冷缓存窗口
（commit 56–62GB）跑完。证据与失败根因见同文档 §3.6。

### P4 · 文档

- **W1** `docs/plans/2026-10-27-…-plan.md:184-193` 改为整库架构版本 + 启动核对 + 升级流程；删「409 映射」「THROW 守卫」「A5 两条用例」的表述，指向本计划。
- **W2** `docs/specs/web-service-api.md`：新增 `POST /api/v1/storage-version/upgrade`；`/health` 节补 `storage_version` 全部字段；`:547` state 枚举 `new | merged`；4.5.1A 补 409 `unsupported` 并说明单库 rebuild 不写版本记录；`:1263` 删掉「没有行 = … model/rebuild 补齐」里与版本有关的暗示。
- **W3** ADR-091 二次修订：标题与三轴表改数据库架构版本；「为什么不自动重建」改「不静默重建，提示后一键升级」；「后果」里 409 与 THROW 那两条改为「启动拒绝 / 启动提示，运行期不再有版本错误」；写入时机、核对位置入决策；「未做的事」里「没有逐库清单」改为「进度在 storage_upgrade:current」；文件名保留并加一句说明。
- **W4** 并入 C8（rs-server 测试文案）。

## 5. 决策

已定：

- **D-0** C3 状态码 409 `unsupported`（依据 `rooms_rebuild` 先例 `handlers.rs:722-726`）。
- **D-1** `storage_version:current` 是整个数据库的架构版本；只在整库在新架构下初始化完成 / 全部库升级完成时写（C4 迁走）。
- **D-3（第三轮批注后改定）** 版本检测只在进程启动时做全局一次（gen-model 和 rs-server）；**不和 ptset 或任何表做版本检测**，surql THROW、aios_core 读点集时的 check、rs-server 409 映射全部移除（C8）。
- **D-4** 不匹配时提示升级；升级可指定 dbnum，默认全部；升级 = 清理旧数据 → 重新解析 → 重新生成模型，复用逐库 `reinitialize`；「整库完成」由升级流程记账（`storage_upgrade:current`）。
- **D-5** 提示渠道 = 启动日志 + `/health` 标红 + 运维面板横幅一键升级；不做控制台交互、不做启动参数自动升级。
- **D-6** rs-server 启动不匹配拒绝启动，提示用 aios-database 升级。
- 不让 P1 两单改 `task_end.rs` / `startup_reconcile.rs`。

待拍板：

- **D-7 · 全新库的首次写入。** 空库从零初始化完成时（`mark_model_ready` 首次翻真，且此前既无版本记录也无模型行）是否自动写？(a) 自动写（推荐）；(b) 新库也走升级入口。「此前无模型行」的判据要定。
- **D-8 · 开发机跨版本启动。** 是否给环境变量（如 `AIOS_STORAGE_VERSION_CHECK=warn`）允许只告警？推荐有，默认严格。
- **D-9 · 升级中的库对外怎么标。** `/health` 和面板是否单独标「升级中」；rs-server 在升级期间按 D-6 不允许启动（记录还没写）。

## 6. 风险

- C8 跨三个仓（aios_core → gen-model → rs-server），顺序错了会出现「读侧还在 check、surql 已不 THROW」或反之的中间态；按 aios_core 先发、两个消费仓同日更新 rev。
- 升级 = 整库重新解析 + 生成，比 `model/rebuild` 重得多；面板按钮必须二次确认并列出将被清空的库。
- `reinitialize` 不接受全范围（`batch_scheduler.rs:244-249`），升级流程要自己解名单；名单来源要在 C7 工单里写死并测。
- 读透形态没有数据面可清（`handlers.rs:3399-3403`），升级入口在该形态下 409 并说明路径。
- C4 与 C7 必须同批发版；只发 C4 记录永远写不上，只发 C7 单库盖章提前把记录写上。
- rs-server 拒绝启动后，升级完成前整个 rs-server 不可用；部署顺序要写进发版说明。
- 移除读点集时的 check 后，如果有人绕过启动核对（D-8 的 warn 模式）读老库，长度函数会对旧行静默算出值——这是 D-8 默认严格的理由。

## 7. 验收命令

```
# gen-model
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api storage_version
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api storage_upgrade
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api health_refreshes_the_storage_version_report
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api model_rebuild
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api disabled_full_rebuild
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api task_end
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api pe_ptset_rows_land
cargo check --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api

# rs-server
cargo test --bin rs-server -- --test-threads=1
cargo test --bin rs-server extraction_fixes_7997 -- --ignored --nocapture

# 现场
GET  http://127.0.0.1:8022/health
POST http://127.0.0.1:8022/api/v1/storage-version/upgrade   {"project":"AvevaMarineSample","mdb":"ALL","namespace":"1516"}
GET  http://127.0.0.1:8022/tasks
```

## 8. 不做的事

- 不做数据迁移：旧数据一律清空重建。
- 不在任何表、任何查询、任何 SurrealQL 函数里做版本判断；不把版本写回行上或水位行上。
- 不做控制台交互问答、不做启动参数自动升级。
- 不动 generator / NameChanged 两条线的文件；不改 `manual_update.rs`。

## 9. Plannotator 门禁记录

- 首轮：`annotated`，「不需要针对 ptset 这个表，我说的这个版本是指的我们整个数据库的架构版本」。追问后定：写入时机 = 整库初始化/全部库重建完成；核对 = 启动核对 + `/health`。`.planning/2026-10-28-storage-version-closure/.gate-result.json`。
- 第二轮：`annotated`，「运行起来时，检查当前连接的数据库，如果版本不匹配，就需要提示是否自动升级，自动升级运行输入 dbnum 或者默认全部，升级就是清理旧的数据库数据，然后重新解析和生成模型数据」。追问后定：日志 + `/health` + 面板一键；复用逐库 `reinitialize`，默认全部；rs-server 不匹配拒绝启动。`.gate-result-r2.json`。
- 第三轮：`annotated`，「不要和 ptset 做版本检测。而是启动时做全局检测，如果不正确，就要提示用户，让他使用 aios-database 升级数据库」。据此：D-3 改定为只有启动全局核对；新增 C8 移除 ptset 侧全部版本检测；SV-C1 工单撤回；V0 不再验 THROW。`.gate-result-r3.json`。
- 第四轮：`{"decision":"approved"}`（无批注）。`.gate-result-r4.json`。本计划自此为已批准版；D-7 / D-8 / D-9 仍待拍板，在出 C4–C8 工单前定。
