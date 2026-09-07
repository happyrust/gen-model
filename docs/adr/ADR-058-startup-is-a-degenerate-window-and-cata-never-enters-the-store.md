# ADR-058：初始化启动收敛为退化窗口；CATA 不入持久层；监听限定域约束启动

状态：**已接受**（2026-09-03 用户拍板，grill-with-docs 六轮收口。原话三句：「因为我们是采用的 direct 方式，
不需要管 CATA 数据了」「direct 模式就是尽可能的轻便，只需要将模型数据写回到 surreal，和读取到的增量解析的
DESI 数据」「初始化启动时，也需要限定监听的范围的，也是需要只针对我们 watch dbnums 进行增量更新」，
末轮追补「之后一直盯着文件自动追」与「限定主要是限制范围，方便调试使用」）；
**实施中**（`docs/plans/2026-09-03-startup-initialization-refactor-plan.md` S1–S5；
**S1 已落地**：D4 / D5 与实施约束 1 / 2 已在码上成立；**D7 门的部分已落地**（2026-09-03 D7-lite，从 S3
拆出：模型门不再看 `data_ready`、启动复核排在数据等待之前；`Catalogue` 相删除与第二车道仍未实施）；
编排层 P1–P3 修复已落地（2026-09-03，`startup-orchestration-p1-p3-fix-plan` T1–T10）；S2 的 `/health` 半边已落地
（`/health.initialization.startup` 带覆盖面与复核结果，横幅 / 界面半边未做）；**D8 的代码面已落地**（2026-09-04 S5-1～3：
`sync_live` 不再被 `direct_read_mode` 钉住、启动序列删除 direct 专用的模型门 / 房间分支、实施约束 3 的护栏翻成
`direct_read_mode_only_selects_the_data_read_route`；**未 live**，db 形态退役 / git tag 未做）；**direct 已是默认读模式**
（2026-09-04 10:27 用户拍板「AIOS_DATA_READ_MODE=direct 改成默认的，全部默认切换过来」：`AIOS_DATA_READ_MODE` 不设即 direct，
只有显式 `=db` 才退回 legacy 读路由，认不出的值按未设置处理并出声——Q3 由此部分收口，变量本身留到 db 形态整体退役时一并删）；其余未实施。
2026-09-04 审核与后续顺序见 `docs/plans/2026-09-04-startup-increment-audit-and-next-plan.md`）
日期：2026-09-03
关联：
- **承接** ADR-056（N7「模型面不以 `pe` 为前置」、D9「eager 只对受影响根」、实施约束 9「选根根集从文件枚举」
  是本 ADR 的全部前提；本 ADR 把它们从稳态窗口延伸到启动）、ADR-057（模型面纯函数层与状态副作用的 crate 边界）、
  ADR-054（生成时点 = 显式指定或文件最新）、ADR-053（direct 模式生成读）；
- **修订** ADR-025（阶段屏障：Catalogue 阶段取消，模型面不再等 `data_ready`，§7 的「数据批次只提交数据 +
  水位 + 模型意图」不动）、ADR-048（监听限定域上弦启动工作：到达面扩到启动的全部三件事，性质仍是调试开关）、
  `ADR-004-on-demand-cata-parsing-port` 与 ADR-034（按需 CATA 与 watch-scope CATA 依赖闭包：随 D1 整体退役；
  注意本仓 ADR-004 有两份，退役的是 on-demand-cata 那一份，`ADR-004-db-noun-classifier-data` 不动）、
  ADR-008（目录反向传播：`ref_rev` 与 `cascade_expand` 的机制不变，改的只是「变更的 CATA refno 从哪儿来」，见 D2）；
- **不动** ADR-001（`applied_sesno` 是数据水位）、ADR-021（水位须有数据支撑；D3 保留基线正是为了它一个字不改）、
  ADR-050（`model_update_pending` 进程本地）、ADR-051（模型开关不请求启动全量生成）、ADR-014（分支原子替换）；
- 证据：本文件「背景」一节的现码引用，全部为 2026-09-03 工作树实测。

## 背景

ADR-056 / ADR-057 已经把**稳态增量窗口**搬到新架构上：`window_root_plan.rs` 的模块头写得很清楚——输入只有文件
（`DbSet@S` / `DbSet@T`、`plan_update`、`enumerate_generation_roots`）与模型面自己的状态（`gen_root` 行），
「一行 `pe` 都不读」。

**启动那条路一步没动，而且生产跑的 direct 形态干脆绕过了它。** 2026-09-03 审核四条：

| # | 发现 | 现码位置 |
|---|---|---|
| A1 | **生产形态的启动没有任何模型复核**。`if startup_data_ready && !direct_read_mode` 把整个 `reconcile_model_coverage_at_startup` 挡在 direct 之外；`direct_read_mode` 又把 `sync_live` 钉成 false，watcher 也不起。direct 启动的全部内容是：开模型门、装空间树、跳过房间 | `lib.rs:795`、`lib.rs:592`、`lib.rs:820`、`lib.rs:952` |
| A2 | **ADR-050 承诺的「重启靠 `gen_root.source_end_sesno` < 文件最新重新发现」没有实现者**。进程起来先 `clear_stale_at_process_start` 清空 `model_update_pending`，唯一的补偿就是 A1 里被跳过的那条。凭证与文件的比较代码确实存在（`model_source_floor_by_dbnum`），但它只挂 `/dbnums` 给界面看，不产生任何工作 | `lib.rs:1190`、`model_update_pending.rs:1778` |
| A3 | **非 direct 那条复核路的标尺是水位，不是文件**。`sync_and_seed_model_coverage` 拿 `state.applied_sesno` 当「要求时点」。ADR-054 已经把生成时点从水位解耦到文件最新——文件往前走了、数据还没摄入的库，启动复核会判它「模型完整」 | `model_update_pending.rs:1607`、`gen_root_credential_is_current` |
| A4 | **它还是 `pe` 打头的**。`fn::sync_gen_roots` → `fn::gen_root_cover` 第一句就是 `select value id from pe where dbnum = $dbnum and deleted != true`。这正是 ADR-056 实施约束 9 点名要下线的东西，也是函数注释自己写的「448 库的现场付不起」的原因。文件枚举的替身 `enumerate_generation_roots` 早就在，启动路径没用上 | `resource/surreal/gen_root.surql:44`、`generation_root.rs:647` |

一句话：稳态窗口已经搬到新架构，启动还留在旧架构，而生产形态绕过了旧架构那一版。

同一轮里另外确认了两件事，它们决定了 CATA 能整个不入库：

- `ModelTarget.catalogue` 是 `Vec<ModelFileTarget>`，每项只有 `{file, session, digest}`——它从来没读过一行入库的
  CATA 数据（`e3d_model_service.rs:80`）。「CATA 变了 → 该库根过期」这条兜底在 direct 下本来就自足。
- `ref_rev` 的边是从**引用侧**建的：`build_reverse_index_statements` 遭的是本窗口解出来的元素，写
  `{in: referrer, out: referenced}`（`manual_update.rs:944`）。解 DESI 就能得到 CATA→DESI 的反查，
  被引用的 CATA refno 有没有 `pe` 行无关。

## 决策

| # | 决策点 | 结论 |
|---|---|---|
| D1 | 持久层里还剩什么 | Surreal 只存两样：**模型面自己的状态**（`gen_root` 凭证与 CAS、产物行、mesh、`ref_rev`）与**增量摄入的 DESI 属性数据**。**CATA 不解析进持久层**——模型面经 `E3dDbResolver` 从文件读，级联判据不需要 CATA 行（见背景两条实测）。整库 CATA 基线、按需 CATA 闭包、惰性兜底、部分解析全部退役。**2026-09-06 修订（ADR-064 D5）**：第三样是**DESI 依赖的 CATA 行**——每批 DESI 写回连带其目录链闭包（`CataClosureResolver`，容器白名单）里的 CATA 元素同批入库，`pe.dependency = true`，服务属性面板 / `ref_rev` / 依赖备份；整库 CATA 基线与惰性兜底仍退役，闭包解析器本身保留并收窄 |
| D2 | CATA 变更的级联判据 | **ADR-008 的机制不变，换的只是触发源**。此前「变更的 CATA refno」来自 CATA 数据批次解出来的 `EleOperationData`；CATA 不再产生数据批次，改由**对 CATA 文件做会话索引差分**（`collect_window` 在该 CATA 的 `DbSet` 上，纯文件侧）得到。往下照旧：`ref_rev` 反查设计元素 → `cascade_expand` / 上卷到根 → 只重生成命中的根。`ref_rev` 覆盖不到时退到 `ModelTarget.catalogue` 指纹失配 → 该库全部根过期，**回执必须点名**（`catalogue_cascade_degraded`），不得静默 |
| D3 | DESI 还做不做基线 | **做**。库第一次纳入时按 `1..file_latest` 整库解析，之后全是增量。因此 `applied_sesno`「该 dbnum 已整体落库的会话号上界」的原义，以及 ADR-021 的**数据支撑 / 空基线凭据 / 幽灵水位**三条判据，一个字不改。放弃基线省下的是一次性成本，换来的是把三条已经稳定的判据推倒重写——那不是轻便，是把复杂度从启动挪进每一次判断。**实施注记（2026-09-04）**：基线本身也收敛到 D4 那条路上了——`parse_pdms_db` 整体退役后它就是 e3d-io 的**退化窗口**（base 为空、target = 冻结目标会话），走 `e3d_collector::stream_baseline` + `versioned_db::baseline::render_baseline_batches`，与稳态窗口共用同一套枚举与同一份行渲染（`ele_ops::ElementRows`）；「做不做基线」这个决策不变，变的是它不再是第二条解析路 |
| D4 | 启动的模型复核机制 | **收敛到与稳态窗口同一条路**：启动 = 一次退化的窗口，`S` = 该库 `gen_root.source_end_sesno` 的下限、`T` = 文件最新，跑同一个 `collect_window` + `touches_roots`，同一套四桶分流（regen / delete / advance / lazy）与同一套护栏。`sync_and_seed_model_coverage` 与 `fn::sync_gen_roots` 从启动路径下线；后者降为 `increment_planner_parity` 的对拍 oracle（ADR-056 约束 9 已经这么写） |
| D5 | 启动复核的覆盖面 | **两级过滤**。第一级：一次请求两条 GROUP BY 拿每库的**安定凭证下限**与**未安定根数**，跟文件最新会话比。第二级：只有落后（下限早于文件最新，或存在未安定的行）的库才开窗差分。448 库一次取数，不开 448 个窗口；追平的库连差分都不跑。今天那条「未限定域只核 `gen_root` 有行的库、有行才逐库 `sync_gen_roots`」的分支随之删除。「安定」= 状态在完成态名单里且凭证大于 0，与凭证前移的 SQL 守卫同一口径——未安定的行没有可用凭证，既进不了下限，又必须无条件重排（上一进程的积压刚被 ADR-050 清掉） |
| D6 | `gen_root` 零行的库 | 启动**一根不排**，`roots_T \ roots_S` 整个进 `lazy`，等按需 `ensure`（ADR-056 D9 的启动对应物）。`/health` 与启动横幅明说「本库无凭证记录，模型按需生成」——**不预先枚举根数**，那本身就是 D5 要避免的开销。有界预热做成默认关的显式开关，不做默认行为（ADR-051 已经拒绝过「开关即全量」） |
| D7 | 阶段屏障 | 塌成**两条并行链**：「文件身份解析 → 模型面」与「Meta → Design 摄入」。`Catalogue` 阶段整个取消（D1）。模型面**不等** `data_ready`——串行那一版的唯一理由是「生成器要读 `pe`」，而 ADR-056 N7 已经把这个理由拆了。房间仍排在模型之后（它吃 `inst_relate.aabb` 与空间树，那是模型产物） |
| D8 | 形态与 watcher | `direct_read_mode` 只保留「数据面读哪儿」这一个含义，不再兼管 watcher / 模型复核 / 房间。`sync_live = configured_sync_live && !direct_read_mode`（`lib.rs:592`）删除：**启动全量扫一次，之后 watcher 一直盯着自动追**。db 形态退役，退路留 git tag（与 ADR-056 D6 同一口径）。**实施注记（2026-09-07）**：legacy 那条路本体也删干净了——`sync_pdms` 与 `total_sync` / `incr_sync` / `only_sync_sys` / `sync_history` 四个开关整条退役。direct 成为默认形态之后它一次都没被走到过（仓里 30 份配置全写 `false`），而它与水位、模型、`gen_root` 凭证全程脱钩，「解析完了却什么都没建立」这个坑随路一起没了。留下的只有 `database::bootstrap_sys_metadata`（空库引导 SYS 元数据，`bin/sync_sys_only` 与 l3 空店引导要它——DESI 解析经 `MDB` / `WORL` 反查世界 refno，没有 SYS 的空库每个 dbnum 都解成「只有根、0 个元素」）与 `sync_total_async_threaded`（首载 / 重建的单文件基线写者，由数据批次 worker 调）。`AIOS_DATA_READ_MODE=db` 这个退路仍在（Q3 未决）。**实施注记二（2026-09-07）**：`sync_live` 这个开关本身也退役了——它把「起不起 watcher」与「做不做启动数据重扫 + 等 `data_ready`」绑在一个布尔上，而「一个库都不监听」这个状态没人真要：监听范围本来就由 `watch_dbnums` 说了算（键缺失与空表**不区分**，都等于本 MDB 声明的全部 DESI，共识 d-207），所以它也接不住 `sync_live` 的关闭态。现在 watcher **无条件起**，启动重扫的去留由 `data_face` 单独回答（`read-through` 跳过重扫、直接 `mark_data_ready_without_manifest`，watcher 仅作脏信号）。`/health.sync_live` 一并去掉——它只能为真。**实施注记三（2026-09-07 晚）**：上面那句「留下的只有 … 与 `sync_total_async_threaded`」里的后一半也退役了（`docs/plans/2026-09-07-retire-sync-total-async-threaded-plan.md`）——基线摄入只剩 `versioned_db::baseline_ingest::ingest_file` 一个按文件的入口（收集 / 渲染 / 执行 / CATA 闭包 / 统计五个部件与增量共用），`bootstrap_sys_metadata` 逐文件调同一个入口。D3 的「保留基线」一个字不变，变的只是它从此连编排层也没有第二份 |
| D9 | 监听限定域的到达面 | 它**是调试开关、不是服务边界**（性质不变），但**启动同样受它约束**：限定时启动的数据摄入、模型凭证复核与派生工作只覆盖域内库；未限定时覆盖全部候选库，那才是生产口径。收窄发生在「哪些观察变成数据批次」这一层，**不在扫描层**——`dbnum` / 库类型 / 最新会话号都要读文件头才知道（`CONTEXT.md`「文件观察」），扫描与身份裁决必须保持全局，否则会丢掉域内库的路径迁移与它自己的同号重复判定。域外库的 `ensure` / `rebuild` 继续拒绝（现码 `handlers.rs:1063`、`model_rebuild.rs:63`），回执照 `CONTEXT.md` 要求明说本进程监听范围被收窄 |

## 新不变量

- **S1 启动没有自己的选根判据**：唯一判据是 `WindowRootPlan`。启动与稳态的区别只有 `S` 从哪儿来
  （稳态取提交前水位，启动取凭证下限），分桶、护栏、退化理由一个字不改。
- **S2 启动不以 `pe` 为前置**（承接 ADR-056 N7）：`pe` 零行的库在启动同样能选根、前移凭证、判「模型落后」。
  启动路径全程零 `pe` / `pe_owner` 查询。
- **S3 CATA 永不进持久层**：任何把 CATA 元素写进 `pe` / `ATT_*` 的路径都是边界破坏。CATA 只以
  **文件身份**（`ModelTarget.catalogue`）与**被引用 refno**（`ref_rev` 的 `out` 端）出现在库里。
  **2026-09-06 修订（ADR-064 D5）**：收窄为「CATA **只按 DESI 依赖入库**」——每批 DESI 写回连带它目录链闭包里的
  CATA 元素经 `element_batch`（`Fresh`）入 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`，行带 `pe.dependency = true`，
  每库一行 `cata_dependency_state`（不是水位）。仍是边界破坏的：整库 CATA 基线、CATA 进 `applied_sesno`、
  `dependency = true` 以外的 CATA 行。模型面照旧从文件读（`E3dDbResolver`），本条不改 S2 / D4。
- **S4 收窄不在扫描层**：候选库枚举、文件头读取、登记文件身份与同号重复裁决恒为全局；`watch_scope::admits`
  只出现在「观察 → 数据批次」与模型工作准入这两处。
- **S5 启动不排从未生成过的根**：一窗（含启动这一窗）入 `model_update_pending` 的根数必须等于受影响根数，
  「全库过期根整体入队」在启动路径上同样不得出现（ADR-056 约束 10 的启动对应物）。

## 实施约束（实施时逐条核，不得静默绕开）

1. **`reconcile_model_coverage_at_startup` 全文不得含 `sync_gen_roots`**（`include_str!` 护栏；ADR-056 约束 9
   本来就要求，本 ADR 把它从「稳态窗口的护栏」扩成「启动的护栏」）。同一护栏加钉：该函数全文不含
   `applied_sesno`——启动的标尺是文件最新，不是水位（A3）。
2. **两级过滤要能证明是两级**：第一级必须是一条语句（`model_source_floor_by_dbnum` 那一条 GROUP BY），
   护栏钉「启动复核对 `collect_window` 的调用次数 ≤ 落后库数」，不得退化成逐库开窗。
3. **`lib.rs:592` 的 `!direct_read_mode` 删除**要连着改钉它的测试
   （`startup_serves_manual_execution_without_a_watcher` 一类断言 `sync_live` 在 direct 下恒 false 的那条）；
   翻过来钉「`direct_read_mode` 在 `lib.rs` 里只出现在数据面读路由的选择上」。
   **2026-09-04 已做**：钉它的那条实为 `direct_mode_starts_the_batch_worker_but_not_the_watcher`，翻成
   `direct_read_mode_only_selects_the_data_read_route`——`run_cli` 体内（去注释）`direct_read_mode` 在 worker 之前只许
   出现在绑定 / 一行播报 / legacy `sync_pdms` 读路由的两条门上，worker 之后一次都不出现；完成横幅把读模式当标签
   打一行，不在管辖内。
4. **D1 的退役面要一次拆干净**：`cata_closure.rs` 的整库基线与闭包解析、`ADR-025` 的 `Catalogue` 阶段、
   `watch_scope` 的 CATA 闭包分支（`cata_closure.rs:336/369/711`）、`preload_cata_for_roots` 的
   `enqueue_cata_ref_rev` 补偿（ADR-056 D8 追记）一并处理。CATA 的**文件身份裁决**（项目优先级、同号选主、
   `CataDbLocator`）**保留**——那是 `E3dDbResolver` 的输入，不是解析。
   **2026-09-07**：`enqueue_cata_ref_rev` 补偿已退役（ADR-064 Q2；闭包解析器按 ADR-064 D5 保留并收窄，
   目录行只由 `cata_dependency` 在水位之前落）。
5. **D2 落地前不许宣称级联精确**：`ref_rev` 反查那条路没接上之前，CATA 变更走整库过期是**已知降级**，
   必须在回执与 `/health` 上有名字；不得因为「反正会重生成」就不报。
6. **D3 不给自己开后门**：~~基线路径今天借 `staging` 目录的 `mem://` 读上下文（ADR-042 分块解析基线）。
   spec 035 P3 拆 `staging` 时它要搬家而不是删掉。~~ **2026-09-04 起这条后门自己没了**：基线换成 e3d-io
   退化窗口后不再借任何 `mem://` 读上下文，`stream_baseline` 按 chunk 直接交给有界队列（ADR-042
   「不走提交协议」照旧成立，只是「分块」的实现从暂存实例变成流式块）。spec 035 P3 拆 `staging` 时
   基线这一侧无需搬家。
7. **D6 的横幅不许统计根数**：`/health` 与启动横幅只说「无凭证记录」，不说「N 根未生成」——后者要枚举全库根，
   正是 D5 要省掉的那笔钱。
8. **D9 的三种「跳过」不得混同**（`CONTEXT.md` 既有要求）：MDB 范围判定 / 调试限定 / 监听限定在日志、回执与
   `/health` 上各有各的嗓音。启动横幅新增一行「本次启动的覆盖面」，把「限定域内 N 库」与「全部候选库 N 库」
   分开说。

## 取舍

- **放弃的**：ADR-025 的 `Catalogue` 阶段屏障与「模型面等 `data_ready`」这条串行。前者的用户随 D1 归零，
  后者的唯一理由随 ADR-056 N7 消失。代价是 CATA 精确级联从「数据面维护的 `ref_rev` 天然在场」变成
  「D2 要专门接一条文件差分」，接上之前是可观测降级。
- **换来的**：启动与稳态**只有一份选根判据、一份变更检测、一份护栏**；`fn::sync_gen_roots` 这条对生成根数
  平方级的 `pe` 遍历彻底离开热路径；direct 形态从「什么都不做」变成「跟 db 形态同一条路，只是读底座不同」，
  A1–A4 四条一次收掉。
- **没有换来的**：启动仍然不保证「模型全部追平」。D6 之后从未生成过的根一直到有人看它才生成——这是
  ADR-056 D9 已经付过的价，本 ADR 只是把它在启动上也认下来，并要求它可观测。
- **D3 的代价**：库首次纳入仍要付一次整库解析。选择保留它，是因为放弃它要重写 `applied_sesno` 的语义与
  ADR-021 的三条判据，而那三条判据今天正确、稳定、有测试；用一次性成本换掉它们不划算。

## 后果

- `CONTEXT.md`：「按需解析元件库（On-demand CATA）」整章标 retired（按需解析 / 引用闭包 / 部分解析 /
  生成根闭包 / 惰性兜底 / 闭包漏边）；「监听限定域」补「初始化启动同样受它约束」与「CATA 只从文件读」
  （2026-09-03 已改）；「严格初始化阶段」改写为两条并行链，Catalogue 一段删除。
- `ADR-025`：决策 2 的 `Catalogue` 一句、决策 1 的五段顺序、决策 7 的「模型 drain 仅在 `data_ready` 后」
  按 D7 加对照注记，与 2026-09-03 这一条并列（体例照该文件 2026-09-03 已有的 ADR-056 对照块）。
- `/health`：新增「启动覆盖面」与「无凭证记录的库」两项；`increment_mode` 之外不再需要 `data_read_mode`
  影响启动语义的说明（D8）。
- 实施与验收门见 `docs/plans/2026-09-03-startup-initialization-refactor-plan.md`；每阶段 `changelog.md` 一条。

## 开放问题

- **Q1** D2 的 CATA 文件差分与 ADR-056 Q3（e3d-io 直接读 dab 的 `BREF` / `SPBREF` / `SCBREF` 反向引用表）
  是两条通往同一件事的路。前者靠 `ref_rev`（需要 DESI 已摄入），后者不需要任何库。取到 dab 反向表结构后
  再定谁是权威，届时另起 ADR。
- **Q2** D6 的有界预热若日后要默认打开，排序按什么——库规模、最近访问、还是界面声明的可见集？
  等有真实的首显时延投诉再议，不预先设计。
- **Q3** D8 之后 db 形态退役，`AIOS_DATA_READ_MODE` 这个环境变量本身还有没有存在意义？倾向在 P4
  （ADR-056 的收集器换底座）收口时一并删掉，那时两条读路径合一。
  **2026-09-04 部分收口**：direct 已是默认（`options::direct_read_mode()` 非显式 `db` 即 direct；`data_read_mode_notice()`
  对认不出的值出声），变量只剩「显式退回 legacy」一个用途；整体删除仍与 db 形态退役 / git tag（refactor plan S5-4）一起收。
