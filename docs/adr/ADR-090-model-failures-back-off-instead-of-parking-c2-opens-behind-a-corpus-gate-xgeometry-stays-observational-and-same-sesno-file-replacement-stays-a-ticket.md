# ADR-090：Phase 2 四项拍板——模型失败退避重试不 park、C2 结构连接级联翻开（带语料门）、DESI 窗 XGEOMETRY 仅观察、同 sesno 换文件先落文留票

状态：**已接受**（2026-10-27 用户 Phase 2 拍板；四项都是 `docs/specs/incr-gen-audit/review.md` §2.4 / §6 第 3 条列出的「需用户决策」项，本文把结论写成可直接当依据的口径。**本文不改代码**；D1 / D2 要改代码，落地时在本文追记，D3 / D4 是书面口径 + 留票。）
日期：2026-10-27
裁决的四项：
- **D1** GitHub #72：模型侧生成失败**不** park，改退避重试（数据侧 `DATA_BATCH_PARK_AFTER = 1` 不动）。
- **D2** `model_structural_connection_cascade`（C2）翻开，前置是一道与 ADR-080 D9 ③ 同口径的语料门。
- **D3** DESI 窗口的 XGEOMETRY 子树「仅观察」，不当门。
- **D4** 「同 sesno 换文件」（路由世代账本的 `FileReplaced`）先落文留票，不升格为数据面重建。
关联：ADR-011（单队列单消费者）、ADR-065（模型工作在 dbnum 任务内，pending 退役）、ADR-073（房间 `RebuildBackoff`）、ADR-076 D6（刷盘退避）、ADR-079（生成根账本）、ADR-080 D9 / D10（单元级发布的门与分桶）、ADR-089 D2（文件身份 = 冻结锚）、核对表 R2（`docs/specs/core3d-partial-update-conformance.md`，不在本文改）。行号按 2026-10-27 工作树（含未提交改动）。

## 背景

增量链路审核（`incr-gen-audit/review.md`）把四件「代码已定型、口径没写」或「代码已写好、开关让它不生效」的事挑出来要用户拍板。它们共同的形状是：不写下来，后来的人会把现状当成设计（D3 / D4）或把设计当成现状（D1 / D2）。本文每项都按「决策 → 现状（文件:行号）→ 选项 → 为何选它 / 为何不选另一条 → 影响面 → 前置 / 后续」写。

## 决定

### D1　模型侧生成失败：退避重试，不 park（GitHub #72）

**决策**：模型根生成失败不引入与数据面对称的 park；改为**按根指数退避重试**，保留现有「同错连败判死」的终态；数据面「一次即停」不动。

**现状**：
- 数据面：`DATA_BATCH_PARK_AFTER = 1`（`src/data_interface/batch_worker.rs:51`），同右端失败一次即 `parked()`（`:431-432`），不自动重跑（`:453-458`、`:977`）；复活靠文件长出新会话或人工执行（`:455-456`）；失败原因写 `logs/batch-failures-*.jsonl`，`/health.batch_failures.parked`（`:49`、`:405`）。
- 模型面：根级失败只记进程内账本 `RootOutcome::Failed`（`src/data_interface/model_root_ledger.rs:41-42`）：`record_failed`（`:148-198`）同一句错误连败 `DEAD_ROOT_STREAK = 5`（`:34`）判死转 `Refused`（不再拦水位、不进 regen，ISSUE-029）；`record_transient_failure`（`:204-223`）不计连败。`Failed` 的根 `is_chasing`（`:66-67`）：下一窗并进 regen（`src/data_interface/window_root_plan.rs:1901`、`:2221`）、启动 `carry_unsettled_generation` 接续（`:2429`、`:2688`）；任务终态 `Partial`，`model_sesno` 不盖（`src/data_interface/task_end.rs:34 all_roots_ok`）。**没有退避**：每一窗 / 每次启动对账都立即重试，瞬时故障（几何 / IO）下同一根每轮白算一次，`/health` 只见 `chasing` 计数（`model_root_ledger.rs:93 LedgerCensus`、`:327 census`）。
- 两处易误读的点：`batch_worker.rs:45 MAX_ATTEMPTS = 5` 是空闲轮 panic / 刷盘预算（`:42-44`），不是模型根重试预算；`batch_worker.rs:1248` 注释「模型失败自有 durable pending 的重试账与死信门槛」是退役前的话（pending 已随 ADR-065 退役）。
- #72 票面：零代码，用户 10-27 曾「暂不拍板」（`docs/plans/2026-10-27-open-issues-audit.md:26`），本文即拍板。

**选项**：(a) 对称 park——模型失败一次即停该库模型面，等新会话 / 人工；(b) 退避重试——按根指数退避，到期前不并进 regen，判死线不动；(c) 维持现状（无退避立即重试）。

**为何选 (b)、不选另两条**：模型失败多为瞬时（几何布尔、网格 IO、依赖库未入库），(a) 会让**一个坏根冻结整库模型面**——数据面 park 的对象是整批、失败是文件 / 数据级，模型面的失败是根级，对称过来比数据面更危险；(c) 让对账轮空转，整库模型面长期落后却没有任何「正在退避」的信号。

**影响面**：`model_root_ledger` 的 `RootEntry` 多「下次可试时刻 / 退避档」（进程内，不落库，ADR-079 不变）；`chasing_roots` 的三个消费点（`window_root_plan.rs:1901`、`:2221`、`:2688`）按退避期过滤；`/health` 账本普查多一格「退避中」；回放 `ams8000-equi-fail-inject`（INC-F05 / H05）与测试钩 `AIOS_TEST_FAIL_ROOTS`（`src/data_interface/model_refresh.rs:67-82`）要配退避时钟（测试钩可置零）。现成口径可沿用：ADR-073 房间 `room_resident::RebuildBackoff`（30 s 起翻倍、10 min 封顶）与 ADR-076 D6 刷盘退避。

**前置 / 后续**：初值取 `RebuildBackoff` 同档（30 s 起、翻倍、封顶 10 min），改成配置项与否和命名在落地时追记；判死线 `DEAD_ROOT_STREAK = 5` 不动（退避是在判死之前拉长间隔，不是替代终态）；#72 票面按本条改口；`batch_worker.rs:1248` 那句过时注释随落地改。

**追记 2026-11-11（GitHub #72 落地后的两处补口，用户定）**：

1. **`ModelOnly` 的失败怎么记账。** `ModelOnly` 没有数据窗口，此前它一有根没生成成就被记成数据批次失败、第一次就 park，退避到期也没人重排。现在看「本任务没生成成、且根账本上还在赶」的根数（`batch_worker::data_failure_booking`）：只要有一根，数据批次失败账原样不动、不停跑，退避到期由稳态补偿重排；一根都没有——失败的根都已判死，或败在没有逐根的账可托付的地方（冻结重扫、规划、收尾那一笔）——照旧停跑。用户确认：**失败的根全部判死后仍停**（判死就是「重试多少次都一样」，不停的话稳态补偿每个对账周期空转一遍）。
2. **整库重排的次数上限。** 模型水位从没盖过的库（`model_sesno <= 0`），稳态补偿每次重排都是整库（起点 0 的 `ModelOnly`）——按根退避在这种库上等于每等完一段就把全库重算一遍（7997 一遍约 8 分钟）；同一错误要到第 5 次判死才停，错误每次不一样则停不下来。所以另设一道：整库重排**连着**失败到 `model_whole_library_retry_limit`（`DbOption.toml`，环境变量 `AIOS_MODEL_WHOLE_LIBRARY_RETRY_LIMIT` 压过；缺省 **3**，`0` = 不限）就停下等人。3 的理由：第 1 次可能是一时的，隔 30 s 的第 2 次盖得住「慢一点才恢复」，第 3 次还败就不是等一等能好的；它故意小于判死线 5——判死线管的是只重算欠账根的便宜重试，这一条管的是每次重算全库的贵重试。盖过水位的库重试不是整库重排，不数这一条。

两处都**不改本条的结论**（根级失败不 park，根账本照旧按根退避、同错判死）：停的是「这个库不再自动排 `ModelOnly`」。停法与数据面是同一本失败账（`BatchFailureLedger`），不是第二套——只是进来的原因另记（`batch_worker::ParkCause`：`data_batch` / `model_only` / `whole_library_retry_limit`，谁先到谁算），`/health.batch_failures.<dbnum>.cause` 与失败日志的 `park_cause` 看得出；没到上限时的次数在 `/health.whole_library_retries`。解除：数据面停下来的库照旧（新会话 / `POST /api/v1/update/execute` / 重启）；模型侧停下来的库**2026-11-12 起也是这三条**（用户定：人工执行是通用的「再试一次」）——这种库数据多半已经追平，`update/execute` 在「已是最新」那里同样清账、清失败根的退避（连败数不动，判死线照旧），并当场按稳态补偿的同一判据重排它的模型补算，回执 `model_retry_released` 说清了什么、排上了没有；`POST /api/v1/dbnums/{dbnum}/model/rebuild` 仍可用（从头重算全库；它排的任务不过 park 那道闸，成了账就清，不成仍停着）。此前「已是最新」在清账之前就早退，模型侧停下来的库人工那一条只有整库重排。清账后整库重排的次数从头数。规格 `web-service-api.md` §4.3 / §4.8.1 / §4.8.2。

### D2　C2 `model_structural_connection_cascade` 翻开，前置是一道语料门

**决策**：C2 默认改开，让 ADR-080 D10 的分桶在默认配置下生效；但**翻默认前必须先过一道语料门**（与 ADR-080 D9 ③「净收益为正」同口径，外加结构连接级联的正确性样本）。门没过之前默认值不动。

**现状**：
- `model_structural_connection_cascade()` **默认 false**（`src/options.rs:2388-2409`，字段 `:257-259`）；C8 `model_cable_endpoint_cascade` 默认开（`:2418-2443`，2026-10-17 翻开）；P4 `model_design_reference_cascade` 默认开（`:2453-2478`，2026-09-30 翻开）。
- D10 分桶的前置 `no_model_only_cascades_off`（`src/data_interface/window_root_plan.rs:1826-1846`）：三条级联都开才分桶，任一关则整窗一根不点名（`:1848-1851`）。所以默认配置下 D10 不生效——纯戳窗（`CACHID` / `SPAMAP`）的根照旧整根 regen，已写好的分桶代码一次都没跑到（ADR-080 追记 2026-10-27 已指出「是 C2 关，不是三条都关」）。
- ADR-080 D9（单元级发布开门槛，ADR-080:335-344）三条：① 两态对拍全绿 ② 全语料覆盖（`AIOS_POSREF_DIRS` 五目录）③ 净收益为正；2026-10-01 追记 A6 过、A7 整窗不过，D9 不翻（ADR-080:500）。C2 不是 D9 的开关，但它是 D10 生效的最后一道门，所以翻它要用同一把尺子。
- GitHub #34（结构连接级联默认关）卡外部前置：e3d-model SJOI / PJOI 的 `CUTP` / `CUTB` 端切 + 真机 SAVEWORK 看对端切割；10-24 追加验收「MDS 关节 FIXING → 被截 GENSEC 所在根进 regen」（`docs/plans/2026-10-27-open-issues-audit.md:19`）。

**选项**：(a) 翻开 C2 默认（带语料门）；(b) 保持关，等 #34 外部前置全部到齐；(c) 放宽 D10 前置为「C8 + P4 开即分桶」。

**为何选 (a)、不选另两条**：D10 分桶已实现且是性能主路，(b) 等于让已写好的代码长期不生效；(c) 放宽的是正确性条件——条件 ③ 靠级联目标集把跨根引用者挡在桶外（`window_root_plan.rs:1826-1829` 注释：级联关着那一格是空的，条件 ③ 形同虚设），用户 2026-09-17 已拍「级联关着 ⇒ 不分桶」安全优先，本文不推翻。

**影响面**：`options.rs:2388-2409` 默认值与注释、`:257-259` 字段注释、`:3795-3799` 单测、`DbOption.toml` 模板与向导；`window_root_plan.rs:1830-1846` 不改（前置条件本身保留）；ADR-080 D10 追记「默认配置下已生效」；核对表 ★0907 与 `incr-gen-audit/spec.md` §P 的默认值句子（都在 `docs/specs/`，不在本文改）；翻开后结构库窗口的 regen 根集可能变大（关节改 → 被截杆根），这正是 #34 要的行为。

**前置 / 后续**：语料门 = 一条 L2 / L3 用例，在真语料上证明（i）结构连接级联正确：改关节后被截杆所在根进 regen、几何与整库重建逐条相等（O1 / O2）；（ii）D10 分桶在纯戳窗上只推回执、且一窗端到端时延不慢于不分桶（D9 ③ 口径）。门过了再改 `options.rs` 默认值，并在本文与 ADR-080 追记。#34 的外部前置（端切）不是翻开的前置：翻开只让「关节改 → 杆重算」这条边生效，端切的正确性仍归 #34。

### D3　DESI 窗口的 XGEOMETRY：仅观察，不当门

**决策**：DESI 生产窗（`build_model_update_plan_from_window`）对 XGEOMETRY 子树的处理口径定为**仅观察**：只计数、只进告警，不改工作项、不推凭证。PE / CATA 路与手动路的现有门保留。

**现状**：
- DESI 窗：`observe_xgeometry_candidates`（`src/data_interface/window_root_plan.rs:1406-`）按账本条目数 matched / unreadable → `WindowRootSources.xgeometry_observed`（`:692`、`:1514`、`:1731`）→ 告警句「XGEOMETRY observation only — matched= unreadable=; window work and credential advancement were unchanged」（`:2300-2304`）。没有任何工作项被它挡掉。
- PE / CATA 路：`build_model_update_plan`（`src/data_interface/model_update_plan.rs:892`）有门：位姿目标与删除目标过 XGEOMETRY 门，挡下的进日志「XGEOMETRY 门挡下 N 个模型工作目标」（`:961-975`）；手动路 `manual_update.rs:579 xgeometry_gates`（`:558-569`、`:1924-1931`）按核对表 R2 挡。
- 入口开关 `model_xgeometry_gate` **默认 false**（`src/options.rs:208`、`:2157`）。
- 核对表 R2 正文写「预览与 `build_model_update_plan` 共用筛门」，与 DESI 窗「只观察」不一致（`incr-gen-audit/review.md` §2.1；T-P-7）。没有书面口径说这是有意的。

**选项**：(a) DESI 窗升门（挡 XGEOMETRY 子树的工作项、照常推凭证）；(b) 维持仅观察，落书面口径；(c) 删掉观察。

**为何选 (b)、不选另两条**：DESI 生产窗被 XGEOMETRY 污染的证据不足——回放语料与真机取证里没有一条「DESI 窗里 XGEOMETRY 子树真变了」的正样本，升门就是把 core 的 R2 口径当生产门而没有两态对拍，漏挡是静默的（`manual_update.rs:2012` 注释已点出「一律判假 = 静默漏门，不是保守」）；(c) 丢掉的是将来升门要的正样本计数。

**影响面**：无代码。文档：核对表 R2 正文改口「PE / CATA `build_model_update_plan` 可筛；DESI `from_window` 只观察」（T-P-7，`docs/specs/`，不在本文改）；`model_xgeometry_gate` 的语义保持「PE / CATA 入口门」。

**前置 / 后续**：升门（T-N-4）的前置是正样本：一条 DESI 窗里有 XGEOMETRY 子树变化的回放语料（matched > 0），加两态对拍（挡 / 不挡的投影记录与整库重建相等）。在那之前，`xgeometry_observed` 计数仍进告警句，作为正样本的探针。

### D4　「同 sesno 换文件」：先落文留票，不升格数据面重建

**决策**：路由世代账本的 `FileReplaced`（会话号不变、文件身份变）**维持只动模型回执**，摔入面不增加对应异常、不升格为数据面重建；本条把两个同名概念的边界写清，升格留票。

**现状（两个同名、不同物）**：
- ① `db_generation::GenerationChange::FileReplaced`（`src/data_interface/db_generation.rs:120-130`）：2026-09-12 路由目录世代账本（母计划不变量 7）的三类变化之一；`classify`（`:185-204`）：会话号变大 → 前进 / 回滚，变小 → 回滚，**不变且 `FileIdentity{len, modified}` 变 → 换文件**（`:199-201`）。消费者：`mdb_route_directory::poll_generations`（`src/data_interface/mdb_route_directory.rs:674-`）→ `lib.rs:2119 targeted_model_refresh`：只动该 dbnum 的模型（已发布根逐根点查、没了的删子树、还在的重生成、作废该库回执，`lib.rs:2108-2117`）。`mdb_route_directory.rs:671-673` 自述已知边：SAVEWORK 写到一半被看见会先报一次 `FileReplaced` 再 `SessionAdvanced`。读侧另有 `DirectStoreError::FileReplaced`（`src/data_interface/direct_store.rs:215`，`src/web_service/mod.rs:1653` 消费）。
- ② INC-B07「同路径换文件」那一版的 `FileReplaced` / `file_identity` / `refresh_file_identity` / `adopt_observed_identity` 已于 2026-10-27 撤回（`tests/fixtures/increment_cases.json:21`；`dbnum_state.rs` 无该变体），理由：拿 `{长度}:{修改时刻}` 当文件身份与 ADR-089 D2 冲突——E3D 每次会话改写每个打开库的文件头，mtime 变、会话号不变，生产上会被判成换文件并清库重建。同 dbnum 换了**文件名**改由 `FileAnomaly::NameChanged`（`src/data_interface/dbnum_state.rs:592`、`:829`）阻断。
- 摔入面看不到：`FileAnomaly`（`dbnum_state.rs:560-592`：Rollback / Duplicate / Missing / PathMigrated / NameChanged / TypeChanged / ForeignProject）里没有「同 sesno 换文件」，`scan_gate_for`（`src/data_interface/increment_manager.rs:3248-3262`）因此不可能对它裁决 → 数据面不重建，只有模型回执被作废。

**选项**：(a) 升格为数据面异常（新 `FileAnomaly` 变体 → `ScanGate::Reinit` 整库重建）；(b) 先落文 + 留票，不升格；(c) 不管。

**为何选 (b)、不选另两条**：同 sesno 换文件在生产里罕见；(a) 需要新的世代账本语义——数据面的身份判据不能含 mtime（ADR-089 D2，B07 撤回的正是这条），而路由账本今天的 `FileIdentity` 就是 `{len, modified}`（`db_generation.rs:33-48`），直接把它升格等于把 B07 撤回的误判放大成整库重建；(c) 会让两个同名概念继续被混。

**影响面**：无代码。`mdb_route_directory.rs:671-673` 的「已知边」保留；ADR-089 追记互指本条；诊断文案里「FileReplaced」一律指路由世代账本，不再指 B07。

**前置 / 后续（留票要求）**：升格前要备齐：(1) 数据面的「换文件」判据，不含 mtime（例如冻结锚 / 会话页散列，ADR-089 D2 口径）；(2) 一条能构造「同 sesno 换文件」的回放语料（B07 撤回时 `ams8000-file-replaced` 用例一并撤了）；(3) 数据面世代与路由世代的关系（谁是谁的上游）。票面归 GitHub，本文不开票。

## 后果

- D1 落地前：模型失败仍立即重试、无退避；落地后：同根连败间隔指数拉长，判死终态不变，整库模型面不会因一根而停。
- D2 门过前：默认配置下 D10 继续不生效；门过后：纯戳窗只推回执，结构库窗口的 regen 根集多出被截杆。
- D3：DESI 窗对 XGEOMETRY 的行为有了书面口径，核对表 R2 要改口，升门有了前置。
- D4：两个 `FileReplaced` 不再被混；摔入面仍看不到同 sesno 换文件，这是有意的、带票的缺口。

## 实施约束（给落地 D1 / D2 的人）

1. D1 的退避状态只在进程内账本，不落库、不进水位行（ADR-079：水位是唯一 durable 模型事实）；重启即清零退避，启动 `carry_unsettled_generation` 照旧接续。
2. D1 不改 `DATA_BATCH_PARK_AFTER`、不改 `DEAD_ROOT_STREAK`；`record_transient_failure` 的「非根自身之过」也走退避，但不计连败。
3. D2 翻默认那一次必须同步：`options.rs` 注释与单测、`DbOption.toml` 模板、ADR-080 追记、核对表 ★0907——别再留下「注释说默认关、代码默认开」的漂移（`review.md` §2.3 已列过一批）。
4. D3 / D4 若将来改口（升门 / 升格），在本文追记并把前置条件逐条勾掉，不开新 ADR。

## 不做

- 不把数据面 park 改成退避（数据失败是文件 / 数据级，一次即停的理由见 `batch_worker.rs:453-458`）。
- 不放宽 D10 的「三条级联都开」前置。
- 不在 DESI 窗升 XGEOMETRY 门；不删观察计数。
- 不给摔入面加「同 sesno 换文件」异常；不改路由账本的 `FileIdentity`。

## 未确认（写本文时没有实测）

- 路由账本的 `FileReplaced` 在「E3D 登录只改文件头、不加会话」的情形下是否会被误报（`FileIdentity{len, modified}` 的 mtime 变、sesno 不变 → `classify` 判换文件）：按 B07 撤回记录与 ADR-089 §背景推断会，未实测；若会，那是 D4 不升格的又一条理由，也是留票要记的现象。
- DESI 窗 `xgeometry_observed` 在回放语料上的实际计数未统计（本文只说「无正样本」，不说「恒为 0」）。
- D1 退避初值（30 s / 翻倍 / 10 min）是沿用 `RebuildBackoff`，没有按模型失败分布量过。
