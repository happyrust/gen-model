# 2026-09-03 开发计划：初始化启动按新架构重构 —— 启动收敛为退化窗口、CATA 退出持久层

决策依据：`docs/adr/ADR-058-startup-is-a-degenerate-window-and-cata-never-enters-the-store.md`
（2026-09-03 已接受，grill-with-docs 六轮收口）。
前置：ADR-056（已接受，spec 035 P1 已落地）、ADR-057（提议）。
接在 `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` 的 P 序列后面：**本计划的 S2 依赖
该计划 P2-1/P2-2 的 `WindowRootPlan` 与凭证前移已经可用**（`window_root_plan.rs` 现码已具备）。

## 0. 一句话

把启动从「自己一套 `pe` 打头的覆盖扫描」改成「一次退化的窗口选根」，顺手把 CATA 整个请出持久层，
让 direct 形态跟 db 形态走同一条启动路。

## 1. 前提核实（2026-09-03 工作树实测，可按行号复查）

| # | 事实 | 位置 |
|---|---|---|
| F1 | 生产（direct）启动跳过全部模型复核 | `lib.rs:795` `if startup_data_ready && !direct_read_mode` |
| F2 | direct 强制不起 watcher（**2026-09-04 S5-1 已删**） | `lib.rs:592` `sync_live = configured_sync_live && !direct_read_mode` |
| F3 | direct 跳过启动房间重建 | `lib.rs:952` |
| F4 | 进程起来即清空模型积压，补偿只有 F1 那条 | `lib.rs:1190` `clear_stale_at_process_start` |
| F5 | 启动复核的标尺是水位不是文件 | `model_update_pending.rs:1607` 取 `state.applied_sesno` |
| F6 | 覆盖物化是 `pe` 打头的全表遍历 | `resource/surreal/gen_root.surql:44` `select value id from pe where dbnum = …` |
| F7 | 凭证 vs 文件的比较代码已存在，但只喂界面 | `model_update_pending.rs:1778` `model_source_floor_by_dbnum`，唯一调用点 `/dbnums` |
| F8 | 文件枚举根集的纯函数已就位 | `generation_root.rs:647` `enumerate_generation_roots` |
| F9 | 窗口选根 / 四桶分流 / 护栏已就位 | `window_root_plan.rs`（模块头即口径） |
| F10 | `ModelTarget.catalogue` 是文件身份摘要，不读入库 CATA | `e3d_model_service.rs:80` `Vec<ModelFileTarget>` |
| F11 | `ref_rev` 的边从引用侧建，不需要 CATA 行 | `manual_update.rs:944` `{in: referrer, out: referenced}` |
| F12 | 模型侧的域外准入已经在拒 | `handlers.rs:1063`、`model_rebuild.rs:63`、`model_update_pending.rs:1611` |

## 2. 目标形态

```
启动
 ├─ 全局候选扫描（读文件头，不受限定域收窄）→ 登记文件身份 / 同号重复 / Ref0 归属裁决
 ├─ 数据面链：Meta（SYS/DICT）→ Design（DESI）        ┐ 并行
 └─ 模型面链：文件身份就绪即可开工                      ┘
      ├─ 一级：GROUP BY 取每库凭证下限，与 file_latest 比    ← 一条语句，448 库一次
      ├─ 二级：只对落后的库开退化窗口 S=凭证下限 T=文件最新
      │        → WindowRootPlan 四桶（regen / delete / advance / lazy）
      └─ gen_root 零行的库：整个进 lazy，一根不排
 └─ 房间：仍排在模型之后
之后：watcher 一直盯着文件自动追（限定域只收窄「哪些观察变成数据批次」）
```

## 3. 分阶段计划

### S1 — 启动复核换判据 ✅ 已落地（2026-09-03）

**做了什么**

1. `window_root_plan::reconcile_startup_window(dbnum, db_type, settled_floor, target)`：
   `S` = 该库安定凭证的下限、`T` = 文件最新（`model_source::latest_source_version`），
   选根整条转调稳态的 `build_model_update_plan_from_window`——**一条选根判据都没新增**（ADR-058 S1）。
   启动特有的只有三件：左端从凭证取而不是从水位取、未安定的行无条件重排
   （`PersistedRootState::is_settled`，因为上一进程的积压刚被 ADR-050 清掉）、结果自己
   `enqueue_plan` + `advance_root_credentials`（稳态那条由数据尾事务带走，启动没有尾事务）。
   开窗失败（凭证会话不在链上、文件读不了）退到「全部持久根按过期处理」，与比例护栏同一去向。
2. `model_update_pending::startup_coverage_floors()`：一次请求两条 GROUP BY，拿全部库的安定凭证
   下限与未安定根数。
3. `reconcile_model_coverage_at_startup` 重写为两级：一级比下限与文件最新，追平的库连窗都不开；
   二级只对落后的库调 `reconcile_startup_window`。
4. 删掉「限定域逐库 sync / 未限定域只核有凭证记录的库」两条分支；`sync_and_seed_model_coverage`
   与 `fn::sync_gen_roots` 退出启动路径（前者仍服务人工 `POST /model/rebuild`）。
5. **S5 的第一行提前落地**：删掉 `lib.rs` 上 `if startup_data_ready && !direct_read_mode` 的 direct 门。
   不删的话这一整步在生产形态下不会执行，A1 / A2 一条都关不掉。核对读的是文件与 `gen_root`，
   direct 正是它的主场。护栏与 `direct_mode_starts_the_batch_worker_but_not_the_watcher`（2026-09-04 随 S5-3
   翻成 `direct_read_mode_only_selects_the_data_read_route`）同一手法：往前找最近的 `if`，断言它不含
   `direct_read_mode`。

**验收门（全绿）**

- `startup_reconcile_reads_neither_pe_backed_coverage_nor_the_watermark`：函数全文不含
  `sync_gen_roots` / `sync_and_seed_model_coverage`，**也不含 `applied_sesno`**（F5 那个标尺错位），
  且右端必须取 `latest_source_version`。
- `startup_reconcile_opens_a_window_only_for_the_libraries_that_lag`：钉住
  「一次取下限 → 不落后就跳过 → 才开窗」的书写顺序，证明是两级而不是逐库开窗。
- `startup_floors_take_the_settled_minimum_and_count_the_unsettled_separately`：对 `mem://` 真跑
  那两条聚合——`include_str!` 钉得住调用顺序，钉不住 SurrealQL 本身，而它算错的后果是落后的库
  被静默跳过。
- `settled_means_the_same_thing_wherever_it_is_judged`：「安定」的三处用户（挑左端、SQL 前移守卫、
  一级聚合）共用同一份状态名单，并各自都带着「凭证大于 0」那半条。
- `cargo test --lib`：1335 passed / 0 failed。

**真库查数时补的一个洞（本来会让 S1 变成空转）**

`gen_root` 行的库归属在建行时从来没写过顶层 `dbnum`——`render_upsert` 只写
`desired_target.dbnum` 这个嵌套字段，顶层那一列历史上只有 `fn::sync_gen_roots` 写。
ams 现场实测 2245 行里 **2244 行 `dbnum = NONE`**。而按库取数的每一处都是
`WHERE dbnum != NONE`，于是这些行当面消失，不报错、只是少算；`fn::sync_gen_roots`
退出启动路径之后，这个洞会从「历史行看不见」变成「新根永远看不见」。
已修：建行时写顶层 `dbnum`，启动核对开头跑一次幂等回填。

**还没做（S1 的尾巴）**

- 零解析库门（`pe` 零行的库全程零 `pe` / `pe_owner` 查询）要真库才验得了，未验证。
- 与旧口径的差集对拍（新排出的根集 ≡ 旧口径减去「从未生成过的根」）同样要真库，未验证。
- **真库起一次看启动退化窗口的日志**，未做——需要停掉现场那个实例。

### S2 — 冷库与可观测性（1–2 人日）◐ `/health` 半边已落地，横幅 / 界面半边未做

状态回填（2026-09-04，来自 `2026-09-04-startup-increment-audit-and-next-plan.md` §1）：第 2 / 4 条的 **`/health` 半边
已在码上**——`StartupCoverage{scope, in_scope, candidates, no_credential_dbnums}` 与
`StartupReconcile{result: ok|floors_failed|unreadable|window_failed, unreadable, window_failed}` 进 `InitializationSnapshot`
→ `/health.initialization.startup`（`initialization_phase.rs` 快照结构、`handlers.rs` health）。**没做的**：启动横幅与
「已收敛」那行不读 `startup.reconcile.result`、横幅没有「限定域内 N 库 / 全部候选库 N 库」那一行、`web/ops.html`
未渲染 `startup`；`no_credential_dbnums` 口径有错（来源是 `dbnum_state` 全部 db_type，不是 MDB 的 DESI，审核 #6）；
`unreadable` 混了「不在当前 MDB」与读失败（审核 #7）。第 1 / 3 条状态未核。

1. `gen_root` 零行的库：`roots_T \ roots_S` 整个进 `lazy`，**一根不排**（ADR-058 D6）。
2. `/health` 与启动横幅新增两项：「本次启动的覆盖面」（限定域内 N 库 / 全部候选库 N 库，两种说法分开，
   ADR-058 D9）与「无凭证记录的库」清单。**不统计根数**（实施约束 7）。
3. 有界预热开关 `startup_warm_roots`（默认 0 = 关）落地但不默认开启。
4. **启动复核的结果要能从 `/health` 与横幅上分出「排过了」与「排成功了」**（2026-09-03 由
   `startup-orchestration-p1-p3-fix-plan` §6 登记过来）。现状：`reconcile_model_coverage_at_startup`
   外壳在内函数任一出口之后都 `settle_startup_reconcile()`——`startup_coverage_floors()` 读失败、
   若干库取不到文件最新（`unreadable > 0`）、开窗失败都算「落地」，空闲轮随后 `mark_model_ready`，
   「持久模型工作单与 AABB 阶段已收敛」横幅照打，而 `/health` 只有 `startup_reconcile_pending` 一位。
   做法：协调器 settle 时带上结果（`ok` / `floors_failed` / `unreadable=N` / `window_failed=N`），
   `InitializationSnapshot` 序列化后 `/health` 自动带出；结果非 `ok` 时横幅改口为
   「已收敛（覆盖未确认：…）」。同一处把 direct 形态说清：direct 分支复核落地后立刻
   `mark_model_ready()`，不等复核排出的行被消化（ADR-058 D8），`model_ready` 在两种形态下含义不同。

**验收门**：冷库启动后 `model_update_pending` 为空、`/health` 明说该库按需生成、一次 `ensure` 能出模型；
复核失败的启动 `/health` 能看出「覆盖未确认」，横幅不再单说「已收敛」。

### S3 — CATA 退出持久层（1 周）

1. 删除 CATA 整库基线与 `Catalogue` 阶段（`initialization_phase.rs` 的阶段枚举与屏障、
   `batch_scheduler` 的阶段冻结、ADR-025 决策 1/2 对应实现）。
2. 删除 `cata_closure.rs` 的解析与闭包路径（含 `watch_scope` 的 CATA 分支 `cata_closure.rs:336/369/711`、
   `preload_cata_for_roots` 与 `enqueue_cata_ref_rev` 补偿）。
   **保留** CATA 的文件身份裁决：项目优先级、同号选主、`CataDbLocator`（`E3dDbResolver` 的输入）。
3. 阶段序列改成两条并行链（ADR-058 D7）：模型面不等 `data_ready`；房间仍在模型之后。
   **门的部分已提前落地（2026-09-03，D7-lite，用户拍板从 S3 拆出）**：
   `model_generation_allowed` 去掉 `data_ready` 前置、`install_manifest` 装好清单即开 `model_phase_open`、
   `configure_model_bootstrap` 不再按 `data_ready` 算门、`run_cli` 的 `reconcile_model_coverage_at_startup`
   提到 `wait_for_data_ready` 之前；`mark_model_ready`（收敛宣告）与 `postprocess_allowed`（房间）仍等
   `data_ready`。护栏 `startup_waits_for_data_then_models_before_rooms` 改为
   `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last`（钉 `worker < reconcile < data`、
   `data < model_ready < room`），协调器新增三条单测（清单开门 / 数据相被挡不关门 / 全量前置仍关门）。
   `cargo test --lib` 1351 passed / 0 failed / 100 ignored；未 live 验证。
   **S3 里这一条剩下的**：`Catalogue` 相删除随第 1 条走；单消费者的 `DataQueued` 让位（模型积压只在数据
   队列空隙消化）不在 D7 范围，真正双车道是 D10-B。

**验收门**

- 护栏：`src/` 下没有任何路径把 CATA 元素写进 `pe` / `ATT_*`（ADR-058 S3）。
- 现场门：一个从未解析过 CATA 的部署，`ensure` 一个引用大量目录构件的 BRAN 能出正确几何
  （模型面从文件读，本来就不该依赖它）。
- 启动时长对拍：同一现场，S3 前后的启动到「模型门打开」时长，把省下的 CATA 基线时间记进证据文档。

### S4 — CATA 级联换触发源（1–2 周，可与 S3 并行准备）

ADR-008 的机制（`ref_rev` 反查 + `cascade_expand` 种子 + 无深度上限收敛）**一个字不改**，换的只是
「变更的 CATA refno 从哪儿来」——此前来自 CATA 数据批次解出来的 `EleOperationData`，S3 之后那条没有了。

1. 对 CATA 文件做会话索引差分（`collect_window` 在该 CATA 的 `DbSet` 上，纯文件侧），得到变更的
   CATA refno 集合。触发点挂在扫描发现该 CATA 文件 `file_latest_sesno` 前进时。
2. 把这个集合喂进 ADR-008 既有的 `ref_rev` 反查与 `cascade_expand` 路径，上卷到根。
3. `ref_rev` 覆盖不到时退到 `ModelTarget.catalogue` 指纹失配 → 该库全部根过期，回执记
   `catalogue_cascade_degraded`（ADR-058 实施约束 5：**接上之前不许宣称级联精确**）。

**验收门**：改一个被 N 个设计元素引用的目录构件，重生成的根数 = 那 N 个元素上卷后的根数，
不是该库全部根。降级路径能被主动触发并在回执里看到名字。

### S5 — 形态合一（2–3 人日）◐ 1–3 已落地（2026-09-04，未 live），4 只走了「direct 成默认」半步

1. 删 `lib.rs:592` 的 `!direct_read_mode`：启动全量扫一次，之后 watcher 一直盯着自动追（ADR-058 D8）。✅
2. `direct_read_mode` 只保留「数据面读哪儿」这一个含义；启动序列里其余两处分支
   （开模型门与跳过房间）删除。**模型复核那道门已在 S1 一并删掉。** ✅
3. 翻转钉住旧行为的测试：从「`sync_live` 在 direct 下恒 false」改钉「`direct_read_mode` 在 `lib.rs` 里
   只出现在数据面读路由的选择上」。✅
4. db 形态退役，退路留 git tag（ADR-056 D6 同一口径）。未做（与 ADR-058 Q3 `AIOS_DATA_READ_MODE` 一起收）。
   **2026-09-04 10:27 后走了半步**：direct 成为默认——`AIOS_DATA_READ_MODE` 不设即 direct，只有显式 `=db` 才退回 legacy，
   认不出的值出声（`changelog.md` 同日「数据读取模式默认 direct」）；变量整体删除与 `sync_pdms` legacy 分支仍等退役 / git tag。

**落地记录（2026-09-04）**，全部在 `src/lib.rs` `run_cli`：

- `let sync_live = db_option.sync_live.unwrap_or(false);`——`configured_sync_live` 这个中间量连同 `&& !direct_read_mode`
  一起删；direct 的那行播报改成「watcher / 模型复核 / 房间与 db 形态同一条启动路，watcher 由 sync_live 决定」。
- 删除 `if direct_read_mode { open_model_phase(); mark_model_ready(); wake }` 分支：direct 与 db 形态同走
  `if startup_data_ready { … wait_for_model_ready 循环 }`，收敛由 worker 空闲轮宣告，主线不再自己 `mark_model_ready`。
- 删除 `else if direct_read_mode { "direct 模式跳过启动房间重建" }`：direct 走同一道 `skip_startup_room_build()` 三门
  （止血口 → `startup_autorun` → 库侧凭据对账），房间面在 direct 下本来就能从文件读（`room_model.rs`
  `load_room_panel_groups_by_mode`）。
- 顺手关审核 #14：direct 下 `total_sync / incr_sync / only_sync_sys / sync_history` 不再静默忽略，打一行
  「direct 形态不跑 sync_pdms…」；legacy `sync_pdms` 分支本身不动（它是 db 读路由的一部分）。
- 完成横幅的 direct 标签改口；`ensure_batch_worker` 上方注释改口。
- 护栏 `direct_mode_starts_the_batch_worker_but_not_the_watcher` → `direct_read_mode_only_selects_the_data_read_route`：
  钉 `let sync_live = db_option.sync_live.unwrap_or(false);`、不含 `&& !direct_read_mode;`、worker 之前（去注释）
  `direct_read_mode` 只出现在绑定 / 一行播报 / 两条 legacy sync 门这四行、worker 之后到 `run_cli` 结束一次都不出现、
  worker 不由它把门、主线没有 `mark_model_ready()`。
- 验证：`rustfmt --check`（只 `lib.rs`）通过；`cargo check --lib --tests` 通过；
  `cargo test --lib -- startup direct_read_mode initialization model_coverage` **59 passed / 0 failed / 3 ignored**；
  全量 `cargo test --lib` **1414 passed / 1 failed / 103 ignored**，那 1 条是另一会话半改 `web/ops.html` 丢了
  `id="verdict"` 锚点（`the_embedded_panel_is_real_and_a_disk_copy_still_wins`），与本批无关。**未 live**。

**验收门**：direct 形态下起服务 → 改一个 E3D 元素 → SAVEWORK → 无人工干预，模型在窗口内更新。
代码面今天已具备；**live 门未过**——第一次要在 `watch_dbnums` 限定到一两个库的前提下跑（§4 风险行），
并顺带做 §S1 尾巴那次「真库起一次看启动退化窗口日志」。注意 direct 从此也走 `wait_for_data_ready`：
它没有超时退出口（审核 #10），Meta 一个库 blocked 主线就一直等（Web 与按需生成不受影响）。

## 4. 风险与对策

| 风险 | 对策 |
|---|---|
| S1 换判据后启动排出的根集与旧口径不一致，且不易分辨是修好了还是漏了 | S1 验收门的对拍要求「差集逐条可解释」；差集里只允许出现「从未生成过的根」（D6 有意不排） |
| S3 删 CATA 解析后，某个没被识别出来的消费者静默拿到空结果 | 先加护栏定位消费者（`rg` 出全部 CATA 表读点）再删；删除与护栏同一个提交，不留「先删后补」窗口 |
| S4 没接上之前，一次目录改动触发整库重生成，现场以为是性能退化 | 实施约束 5：必须在回执与 `/health` 上有名字；S3 的发布说明里写明这是已知降级与它的收口时间点 |
| S5 watcher 接回来后，direct 形态第一次面对真实文件事件流 | 先在限定域下跑（D9 正是为此存在），确认稳定再放开 |
| 与 spec 035 P3（拆 staging）冲突：基线路径借着 `staging` 的 `mem://` 读上下文 | ADR-058 实施约束 6：P3 拆 staging 时它**搬家而不是删除**，搬家后名字不得再带 `staging` |

## 5. 验收总门

1. **启动只有一份选根判据**：`include_str!` 护栏证明启动与稳态调的是同一个 `WindowRootPlan`。
2. **启动零 `pe` 前置**：`pe` 零行的库启动全程能选根、前移、判落后，日志零 `pe` / `pe_owner` 查询。
3. **CATA 零入库**：护栏证明没有任何路径把 CATA 元素写进 `pe` / `ATT_*`。
4. **两级过滤成立**：448 库一次 GROUP BY，`collect_window` 调用次数 ≤ 落后库数。
5. **冷库不排活**：`gen_root` 零行的库启动后 `model_update_pending` 为空，`ensure` 能出模型。
6. **形态合一**：direct 下改元素 → SAVEWORK → 模型自动更新，无人工干预。
7. **级联精确或明说降级**：S4 落地前每一次整库过期都在回执里有名字。

## 6. 不做 / 留档

- **不做**：启动全量模型生成（ADR-051 已拒）、有界预热默认开启（ADR-058 D6 留作开关）、
  放弃 DESI 基线（ADR-058 D3 已拒，理由是 ADR-021 三条判据的重写成本）。
- **留档**：ADR-058 Q1（CATA 级联走 `ref_rev` 还是 dab 反向表）、Q2（预热排序）、
  Q3（`AIOS_DATA_READ_MODE` 何时删——2026-09-04 起 direct 已是默认，它只剩 `=db` 退路一个用途）。
